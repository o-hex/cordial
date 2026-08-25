#!/usr/bin/env python3
"""End-to-end test of Cordial's Roblox text editor, with real keys.

Why this exists rather than `tools/text-entry-check.sh`, which already tests
typing: that script asserts on the *path* -- a spec arrived, the editor was
placed somewhere with a size, the client survived -- and leaves "did the right
characters end up in the box" to a human looking at a screenshot. Every failure
this project has actually shipped in the text editor would have passed it. A
guard that inserted every character twice, a select-all that silently destroyed
the selection, a click that put the caret in the wrong place: all of them leave
a live client with a correctly placed editor.

Three instruments here are different from that script's, and each replaces one
that could not see a failure:

  * **sway, not cage.** cage advertises no `zwp_text_input_manager_v3` and its
    seat never gains a keyboard. Cordial reads seat capabilities once, at
    `open()`, so under cage it never binds its own `wl_keyboard` -- and the
    guard that stops GDK and Cordial both inserting the same character never
    runs. `docs/NEXT.md` labelled that guard `INFERRED` for exactly this
    reason.

  * **A virtual keyboard that is held open from before the client starts**, so
    the seat looks like a real one for the whole run and that guard is under
    test. `wlrctl` creates its keyboard and exits in the same breath, which is
    a race it sometimes wins.

  * **The `textbox` devctl verb**, which reports what the field actually
    contains. There was no readback at all before: the editor's change signal
    prints nothing, the `text ->` trace prints a byte count, and
    `cordial_screenshot` photographs the engine's swapchain, which cannot see a
    GTK widget.

Input is driven inside a nested compositor on its own `WAYLAND_DISPLAY`, which
is the one form AGENTS.md permits -- never at the developer's session.

Usage:  tools/text-input-e2e.py [--profile NAME] [--keep]
"""

import argparse, json, os, re, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOLDERS = os.environ.get("CORDIAL_HOLDER_BIN", "/tmp/cordial-wl-holders")
BOX = "cordial"                       # the distrobox with sway, grim and the holders
OUT = os.environ.get("CORDIAL_E2E_OUT", "/tmp/cordial-text-e2e")


def sh(argv, **kw):
    return subprocess.run(argv, capture_output=True, text=True, **kw)


def in_box(cmd):
    """Run a shell command inside the container that has the compositor."""
    return sh(["distrobox", "enter", BOX, "--", "bash", "-lc", cmd])


class Devctl:
    """The one-line-in, one-line-out development control socket."""

    def __init__(self, path):
        self.path = path

    def send(self, line):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(20)
        s.connect(self.path)
        s.sendall((line + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        s.close()
        reply = buf.decode().strip()
        if reply.startswith("err "):
            raise RuntimeError(f"devctl {line!r}: {reply}")
        return reply[3:].strip() if reply.startswith("ok") else reply

    def textbox(self):
        """Parse the `textbox` reply into a dict; `text` stays a real string."""
        line = self.send("textbox")
        head, _, text = line.partition(" text=")
        d = dict(p.split("=", 1) for p in head.split() if "=" in p)
        # `redacted` emits Rust's `{:?}`, which for these strings is JSON.
        try:
            d["text"] = json.loads(text) if text.startswith('"') else None
        except Exception:
            d["text"] = None
        d["raw_text"] = text
        for k in ("gen", "rev", "chars", "bytes", "caret"):
            if k in d:
                d[k] = int(d[k])
        return d


class Holder:
    """A persistent virtual input device, driven a line at a time."""

    def __init__(self, binary, display, args=()):
        self.p = subprocess.Popen(
            ["distrobox", "enter", BOX, "--", "bash", "-lc",
             f"export WAYLAND_DISPLAY={display}; exec {binary} {' '.join(args)}"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        # The holder prints "ready" once the device exists. Waiting for it is
        # the difference between a keyboard the seat has and one it is about
        # to have; every key sent before it is silently dropped.
        deadline = time.time() + 20
        while time.time() < deadline:
            if self.p.poll() is not None:
                raise RuntimeError(f"{binary} exited: {self.p.stderr.read()}")
            line = self.p.stderr.readline()
            if line.startswith("ready"):
                return
        raise RuntimeError(f"{binary} never became ready")

    def cmd(self, line, settle=0.15):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        self.p.stdout.readline()          # "ok" per command, so we never sleep blind
        time.sleep(settle)

    def close(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
        except Exception:
            pass
        try:
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


class Case:
    """One assertion, recorded whether it passed or not.

    Nothing here retries and nothing relaxes on failure. The whole value of
    this file is that a result which needed a second chance is a failure.
    """

    def __init__(self):
        self.results = []

    def check(self, name, got, want):
        ok = got == want
        self.results.append((ok, name, got, want))
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}: got {got!r}"
              + ("" if ok else f", want {want!r}"), flush=True)
        return ok

    def note(self, name, value):
        print(f"        {name}: {value}", flush=True)

    @property
    def failed(self):
        return [r for r in self.results if not r[0]]


SWAY_CFG = """xwayland disable
output HEADLESS-1 mode {w}x{h}
default_border none
default_floating_border none
focus_follows_mouse no
{extra}
exec sh -c 'printf %s "$WAYLAND_DISPLAY" > {stamp}'
"""


def start_sway(width, height):
    """A headless sway, which reports the display it bound rather than being guessed at.

    sway picks its own socket name -- `WAYLAND_DISPLAY` on its command line
    names the display it would *connect* to, not the one it serves -- so it is
    asked from an exec block. Guessing cost two runs.
    """
    stamp, cfg = "/tmp/cordial-e2e-display", "/tmp/cordial-e2e-sway.cfg"
    for f in (stamp, cfg):
        if os.path.exists(f):
            os.unlink(f)
    with open(cfg, "w") as fh:
        fh.write(SWAY_CFG.format(w=width, h=height, stamp=stamp,
                                 extra=os.environ.get("CORDIAL_E2E_SWAY_EXTRA", "")))
    # Detached from this process's stdout. Inheriting it means the compositor
    # holds the pipe open after the script exits, and anything reading the
    # script through a pipe -- `| tail`, a harness, an agent -- waits forever
    # for an EOF that only arrives when the compositor dies. One whole run's
    # results were lost that way before this line existed.
    subprocess.Popen(
        ["distrobox", "enter", BOX, "--", "bash", "-lc",
         f"exec env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless "
         f"WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1 "
         f"sway -c {cfg} > /tmp/cordial-e2e-sway.log 2>&1"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 25
    while time.time() < deadline:
        if os.path.exists(stamp) and os.path.getsize(stamp):
            return sway_pid(cfg), open(stamp).read().strip()
        time.sleep(0.2)
    raise RuntimeError("sway never reported a display; see /tmp/cordial-e2e-sway.log")


def sway_pid(cfg):
    """The compositor's own pid, so cleanup kills it rather than its launcher.

    Killing the `distrobox enter` wrapper leaves sway running, and a stale
    compositor holding a display is indistinguishable from a fresh one until
    the seat comes back with no keyboard on it.
    """
    for pid in in_box("pidof sway").stdout.split():
        argv = in_box(f"tr '\\0' ' ' < /proc/{pid}/cmdline").stdout
        if cfg in argv:
            return int(pid)
    return None


def profile_holder(profile):
    """The pid holding this profile's lock, or None.

    Never `pgrep -f`: the pattern matches the shell running it, and that has
    killed this session's own process five times in one day. `pidof` matches
    the executable, which the engine's thread rename does not touch.
    """
    out = sh(["pidof", "cordial-run"]).stdout.split()
    for pid in out:
        try:
            argv = open(f"/proc/{pid}/cmdline").read().split("\0")
        except OSError:
            continue
        if "--profile" in argv and argv[argv.index("--profile") + 1] == profile:
            return int(pid)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default="default",
                    help="a signed-in profile; must not be one another client holds")
    ap.add_argument("--keep", action="store_true", help="leave the client and compositor up")
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--height", type=int, default=800)
    args = ap.parse_args()

    os.makedirs(OUT, exist_ok=True)
    log_path = os.path.join(OUT, "client.log")
    binary = os.path.join(ROOT, "target/release/cordial-run")
    apk = os.environ.get(
        "CORDIAL_APK",
        os.path.expanduser("~/.var/app/org.vinegarhq.Sober/data/sober/packages/"
                           "x86_64/com.roblox.client/base.apk"))
    lib = os.environ.get("CORDIAL_LIB_DIR", os.path.expanduser("~/.cache/cordial/lib/x86_64"))

    for path, what in ((binary, "build -- cargo build --release first"), (apk, "APK"), (lib, "library directory")):
        if not os.path.exists(path):
            sys.exit(f"FAIL: no {what} at {path}")
    for tool in ("wl-keyboard-holder", "wl-pointer-holder"):
        if not os.path.exists(os.path.join(HOLDERS, tool)):
            sys.exit(f"FAIL: no {tool} -- run tools/build-wl-holders.sh in the container")
    held = profile_holder(args.profile)
    if held:
        sys.exit(f"FAIL: profile {args.profile!r} is already open in pid {held}. "
                 f"Pick another with --profile; do not kill a client someone is using.")

    sway = client = kbd = ptr = None
    case = Case()
    try:
        sway, display = start_sway(args.width, args.height)
        print(f"== nested sway on {display}, {args.width}x{args.height}")

        # Both devices before the client, so the seat advertises a keyboard and
        # a pointer at the moment Cordial reads its capabilities. This is the
        # whole reason the double-insert guard has never been exercised.
        kbd = Holder(f"{HOLDERS}/wl-keyboard-holder", display)
        ptr = Holder(f"{HOLDERS}/wl-pointer-holder", display, [str(args.width), str(args.height)])
        caps = in_box(f"SWAYSOCK=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1) "
                      f"swaymsg -t get_seats -r").stdout
        print(f"== seat: {caps.strip()[:200]}")
        if '"capabilities":3' not in caps.replace(" ", ""):
            sys.exit("FAIL: the seat does not advertise both a keyboard and a pointer; "
                     "every reading below would be taken with Cordial's key path switched off")

        env = dict(os.environ)
        env.update(
            WAYLAND_DISPLAY=display,
            GDK_BACKEND="wayland",
            CORDIAL_DEV_CONTROL="1",
            CORDIAL_TRACE_TEXT="1",
            # The readback. Safe here and nowhere else: this harness types its
            # own known strings into a search box and never touches a sign-in
            # field. It also puts the clipboard in the log, so the log stays in
            # /tmp and is not something to paste into an issue.
            CORDIAL_TRACE_TEXT_SHOW_PASSWORDS="1",
        )
        log = open(log_path, "w")
        client = subprocess.Popen(
            [binary, "--lib-dir", lib, "--apk", apk, "--host-libc",
             "--game-activity", "--run", "0", "--profile", args.profile],
            env=env, stdout=log, stderr=subprocess.STDOUT)
        print(f"== client pid {client.pid}, log {log_path}")

        deadline = time.time() + 120
        ready = None
        while time.time() < deadline:
            if client.poll() is not None:
                sys.exit(f"FAIL: the client exited with {client.returncode} before it was ready; "
                         f"see {log_path}")
            text = open(log_path, errors="replace").read()
            # Specifically Home or Landing. The shell announces several routers
            # before either -- `PlatformAccountRouter` arrives first and is not
            # a screen with a search box on it -- and taking the first `app
            # ready` cost a whole run clicking at a page that had not loaded.
            m = re.search(r"app ready: (Home|Landing)", text)
            if m:
                ready = m.group(1)
                break
            time.sleep(1)
        if not ready:
            sys.exit(f"FAIL: the app shell never became ready; see {log_path}")
        print(f"== app ready: {ready}")
        # Ready is not laid out, and nothing in the log marks that.
        time.sleep(14)

        dev = Devctl(os.path.expanduser(
            f"~/.local/share/cordial/profiles/{args.profile}/devctl.sock"))
        print(f"== {dev.send('info')}")

        # **Is this client actually running?** About a third of launches hit the
        # startup freeze -- the engine reaches Home, presents a frame or two and
        # stops (docs/NEXT.md §0). Every assertion below would then fail, and
        # they would fail describing the text editor, which is not what broke.
        # A test that blames the wrong thing is worse than one that refuses.
        def presents():
            return int(re.search(r"presents=(\d+)", dev.send("info")).group(1))
        first = presents()
        time.sleep(6)
        if presents() == first and first < 10:
            sys.exit(f"SKIPPED: this launch hit the startup freeze -- presents stuck at "
                     f"{first}. That is docs/NEXT.md §0 and not a text-entry result. "
                     f"Run it again; it is about a third of launches.")
        run_cases(case, dev, kbd, ptr, log_path, args, display)
    finally:
        if not args.keep:
            for h in (kbd, ptr):
                if h:
                    h.close()
            if client and client.poll() is None:
                client.terminate()
                try:
                    client.wait(timeout=15)
                except Exception:
                    client.kill()
            if sway:
                in_box(f"kill {sway}")

    print()
    if case.failed:
        print(f"BROKEN -- {len(case.failed)} of {len(case.results)} assertions failed:")
        for _, name, got, want in case.failed:
            print(f"  {name}: got {got!r}, want {want!r}")
        sys.exit(1)
    print(f"PERFECT -- {len(case.results)} assertions, none failed.")


TEXT_KEYS = set(range(2, 14)) | set(range(16, 28)) | set(range(30, 42)) | set(range(44, 54)) | {57}


def settle(dev, want_rev=None, tries=40):
    """Wait for the mirror to catch up with the widget, then read it.

    The `textbox` verb reports the buffer `adopt_editor_text` mirrors from the
    widget, which is one signal-hop behind it. Polling the revision counter is
    how that hop is waited out; sleeping a fixed amount instead is how a
    harness starts reporting the previous keystroke's answer.
    """
    last = None
    for _ in range(tries):
        last = dev.textbox()
        if want_rev is None or last["rev"] != want_rev:
            return last
        time.sleep(0.05)
    return last


def focus_box(case, dev, tries=5, geometry_timeout=4.0):
    """Click the home search bar until a TextBox takes focus and says where it is.

    Focus and geometry are two events, not one, and the gap between them is the
    whole reason this waits rather than reading once. Clicking the search bar
    opens a modal, and the engine focuses that modal before it has laid out --
    `showKeyboard` volunteers `x=0 y=0 w=0 h=0` and the real numbers only arrive
    from `nativeGetTextBoxInfo` about a second later. Reading at the instant of
    focus gets the zeroes every time, and then every coordinate computed from
    them is nonsense; that misread three assertions in the first full run.

    The returned box is the first one with a usable size. How long that took is
    reported, because a second of it is a second the user is looking at a box
    with no editor on it.
    """
    for attempt in range(1, tries + 1):
        dev.send("click 674 28")
        for _ in range(20):
            time.sleep(0.25)
            if dev.textbox()["focus"] != "none":
                break
        else:
            print(f"        attempt {attempt} focused nothing, retrying", flush=True)
            continue
        start = time.time()
        while time.time() - start < geometry_timeout:
            box = dev.textbox()
            if box["focus"] == "none":
                break                       # it blurred again; click once more
            if box["x"] != "none" and float(box["w"]) > 0 and float(box["h"]) > 0:
                box["geometry_wait"] = round(time.time() - start, 2)
                return box
            time.sleep(0.1)
    box = dev.textbox()
    box["geometry_wait"] = None
    return box


def run_cases(case, dev, kbd, ptr, log_path, args, display):
    print("\n-- 1. a click focuses a TextBox and the editor gets real geometry")
    box = focus_box(case, dev)
    case.check("a TextBox has focus", box["focus"] != "none", True)
    if box["focus"] == "none":
        shot = os.path.join(OUT, "no-focus.png")
        in_box(f"WAYLAND_DISPLAY={display} grim {shot}")
        try:
            dev.send(f"screenshot {os.path.join(OUT, 'no-focus-swapchain.png')}")
        except Exception as e:
            case.note("swapchain capture", f"refused: {e}")
        case.note("what was on screen", shot)
        return
    case.note("geometry", f"x={box['x']} y={box['y']} w={box['w']} h={box['h']} "
                          f"after {box.get('geometry_wait')}s")
    case.check("the box has a real width", box["x"] != "none" and float(box["w"]) > 0, True)
    case.check("the box has a real height", box["x"] != "none" and float(box["h"]) > 0, True)
    log = open(log_path, errors="replace").read()
    placed = re.findall(r"text editor placed from ([a-z ]+) x=", log)
    case.check("the editor ends up on the box", placed[-1] if placed else None,
               "engine geometry")
    # The editor must never be *seen* anywhere else on the way there. A focus
    # whose geometry has not arrived yet used to drop the editor into a bar at
    # the bottom of the window for about a second and then jump it back up,
    # which is visible and is a large part of what "the text field looks wrong"
    # means.
    case.check("the editor never flashed in the fallback bar",
               placed.count("fallback bar"), 0)

    # Clicking the search bar opens a modal, and the modal is the box that
    # focuses before it has laid out. Watching the placement source across that
    # transition is the only way to see the flinch this fix is about: by the
    # time anything settles, the editor is on the modal and looks fine.
    sources, deadline = [], time.time() + 4
    while time.time() < deadline:
        sources.append(dev.textbox().get("placed"))
        time.sleep(0.1)
    seen = sorted(set(s for s in sources if s))
    case.note("placement sources across the modal opening", " ".join(seen) or "none")
    case.check("the editor was never dropped into the placed bar",
               [s for s in seen if s == "fallback"], [])

    def type_(s):
        kbd.cmd(f"type {s}")

    def key(k):
        kbd.cmd(f"key {k}")

    def state():
        return settle(dev)

    print("\n-- 2. a clean start")
    key("ctrl+a"); key("BackSpace")
    s = state()
    case.check("the box empties", (s["text"], s["chars"], s["caret"]), ("", 0, 0))

    print("\n-- 3. typing five keys inserts five characters (the double-insert guard)")
    type_("hello")
    s = state()
    case.check("text after typing 'hello'", s["text"], "hello")
    case.check("caret after typing 'hello'", s["caret"], 5)

    print("\n-- 4. backspace")
    key("BackSpace")
    s = state()
    case.check("text after backspace", s["text"], "hell")
    case.check("caret after backspace", s["caret"], 4)

    print("\n-- 5. Home, and insertion at the caret rather than the end")
    key("Home")
    case.check("caret after Home", state()["caret"], 0)
    type_("X")
    s = state()
    case.check("text after inserting at the start", s["text"], "Xhell")
    case.check("caret after inserting at the start", s["caret"], 1)

    print("\n-- 6. End")
    key("End")
    case.check("caret after End", state()["caret"], 5)
    type_("!")
    s = state()
    case.check("text after appending", s["text"], "Xhell!")

    print("\n-- 7. shift-arrow selection, replaced by the next character")
    key("shift+Left"); key("shift+Left")
    type_("?")
    s = state()
    case.check("selection was replaced, not appended", s["text"], "Xhel?")
    case.check("caret after replacing a selection", s["caret"], 5)

    print("\n-- 8. select all, then overtype")
    key("ctrl+a")
    type_("abcdefgh")
    s = state()
    case.check("text after select-all and overtype", s["text"], "abcdefgh")
    case.check("caret after select-all and overtype", s["caret"], 8)

    print("\n-- 9. clicking positions the caret (a real click, through the compositor)")
    extent = re.search(r"extent=(\d+)x(\d+)", dev.send("info"))
    sw, shh = int(extent.group(1)), int(extent.group(2))
    header = args.height - shh
    case.note("surface", f"{sw}x{shh} inside a {args.width}x{args.height} output, "
                         f"header {header}px")
    box = settle(dev)
    if box["x"] == "none" or float(box["w"]) <= 0:
        case.check("the box still reports where it is, for the click test",
                   False, True)
        return
    bx, by, bw, bh = (float(box[k]) for k in ("x", "y", "w", "h"))
    row = int(by + bh / 2) + header

    def click_at(surface_x):
        ptr.cmd(f"move {int(surface_x)} {row}")
        ptr.cmd("click")
        time.sleep(0.6)
        return dev.textbox()

    left = click_at(bx + 2)
    case.check("clicking the left edge puts the caret at the start", left["caret"], 0)
    case.check("clicking inside the box keeps it focused", left["focus"] != "none", True)
    right = click_at(bx + bw - 4)
    case.check("clicking past the last character puts the caret at the end",
               right["caret"], 8)
    case.check("clicking did not change the text", right["text"], "abcdefgh")

    print("\n-- 9b. a double-click selects a word, and the next key replaces it")
    key("ctrl+a")
    type_("alpha beta")
    s2 = state()
    case.check("a string with a space in it survives", s2["text"], "alpha beta")
    # Land on the first word. A quarter of the way in is inside "alpha" for any
    # plausible font, and the assertion below does not depend on which
    # character it lands on -- only on the word boundary GTK finds from it.
    ptr.cmd(f"move {int(bx + bw * 0.08)} {row}")
    ptr.cmd("click")
    ptr.cmd("click", settle=0.05)
    time.sleep(0.5)
    type_("Z")
    s2 = state()
    case.check("double-click selected the word, and typing replaced just that word",
               s2["text"], "Z beta")

    print("\n-- 9c. Delete removes forwards, and cut removes the selection")
    key("ctrl+a")
    type_("abcdef")
    key("Home")
    key("Delete")
    case.check("Delete removes the character after the caret", state()["text"], "bcdef")
    key("ctrl+a"); key("ctrl+x")
    time.sleep(0.5)
    case.check("cut empties the field", state()["text"], "")

    print("\n-- 10. copy and paste")
    type_("abcdefgh")
    key("ctrl+a"); key("ctrl+c"); key("End")
    key("ctrl+v")
    time.sleep(1.0)
    s = state()
    case.check("paste appends the copied text exactly once", s["text"], "abcdefghabcdefgh")

    print("\n-- 11. Escape gives the box up")
    key("Escape")
    time.sleep(1.0)
    case.check("Escape blurs the TextBox", dev.textbox()["focus"], "none")

    print("\n-- 12. focusing again starts clean and still takes keys")
    box2 = focus_box(case, dev)
    case.check("the box takes focus a second time", box2["focus"] != "none", True)
    if box2["focus"] != "none":
        key("ctrl+a")
        type_("second")
        s = state()
        case.check("typing works after a refocus", s["text"], "second")

    print("\n-- 12b. with no box focused, keys reach the game again")
    # Everything after this mark is deliberately sent with nothing focused, so
    # step 13 stops reading here rather than counting this on purpose.
    focused_phase = len(open(log_path, errors="replace").read())
    key("Escape")
    time.sleep(1.0)
    before = len(re.findall(r"pass_key_event down=\w+ code=17 ",
                            open(log_path, errors="replace").read()))
    key("w")
    time.sleep(0.6)
    after = len(re.findall(r"pass_key_event down=\w+ code=17 ",
                           open(log_path, errors="replace").read()))
    # The suppression is only correct if it stops. A guard stuck on is the
    # failure that looks like "movement is broken" and never like a text bug.
    case.check("'w' reaches the game once the box has been given up", after > before, True)

    print("\n-- 13. no character reached the game while a box had focus")
    log = open(log_path, errors="replace").read()[:focused_phase]
    suppressed = len(re.findall(r"pass_key_event suppressed", log))
    forwarded = [int(m) for m in re.findall(r"pass_key_event down=\w+ code=(\d+)", log)]
    leaked = sorted({c for c in forwarded if c in TEXT_KEYS})
    case.note("keys", f"{suppressed} suppressed, {len(forwarded)} forwarded to the game")
    case.check("text keys were suppressed at all", suppressed > 0, True)
    case.check("no text key reached the game's key handler", leaked, [])

    print("\n-- 14. the client is still alive")
    case.check("the client survived the whole run", dev.send("ping"), "")


if __name__ == "__main__":
    main()
