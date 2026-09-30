#!/usr/bin/env python3
"""Move a playing stream between PipeWire sinks and check where it is linked.

Drives `audio_probe`'s `--switch` mode (native/audio_probe.cpp), which plays a
stream through the same `CallbackStream` / `PlaybackStream` code the engine uses
and calls `set_output_device` the way the live-settings socket does. This script
is the observer the probe cannot be: it reads the stream's links out of `pw-dump`
after each change, because the stream's own opinion of where it is playing is
the reading that was wrong (see the comment on `set_output_device`).

It creates two null sinks of its own, never touches the default sink, plays at
-54 dBFS (and at zero for the step that returns to the system default, so
nothing is ever sent to the desktop's real output), and unloads what it made.

    clang++ -std=c++17 -DCORDIAL_HAVE_PIPEWIRE=1 -I/usr/include/pipewire-0.3 \\
        -I/usr/include/spa-0.2 native/opensles.cpp native/pipewire_backend.cpp \\
        native/aaudio.cpp native/alsa_backend.cpp native/pulse_backend.cpp \\
        native/oss_backend.cpp native/aaudio_input_callback.cpp \\
        native/audio_probe.cpp -ldl -lpthread -o /tmp/audio_probe
    tools/audio-switch-e2e.py /tmp/audio_probe
"""

import json
import os
import subprocess
import sys

A, B = "cordial-e2e-a", "cordial-e2e-b"


def run(*cmd):
    return subprocess.check_output(cmd, text=True).strip()


def default_sink():
    return run("pactl", "get-default-sink")


def linked_sink(stream_prefix):
    """node.name of the sink each of the stream's links ends on (a set)."""
    dump = json.loads(run("pw-dump"))
    nodes = {o["id"]: o["info"]["props"] for o in dump
             if o["type"] == "PipeWire:Interface:Node"}
    out = set()
    for o in dump:
        if o["type"] != "PipeWire:Interface:Link":
            continue
        info = o["info"]
        src = nodes.get(info["output-node-id"], {})
        dst = nodes.get(info["input-node-id"], {})
        if src.get("node.name", "").startswith(stream_prefix):
            out.add(dst.get("node.name"))
    return out


def one_run(probe, api, prefix, steps, seconds, hold_still=False):
    """Returns a list of (label, expected, observed) and the probe's exit code."""
    env = dict(os.environ, CORDIAL_AUDIO_SINK=A)
    # A control run plays for the same time with nothing asked of it.
    cmd = [probe, api, "--seconds", str(seconds)]
    for s in steps:
        cmd += ["--switch", s]
    default = default_sink()
    expected = [("initial", A)] + [(s or "default", s or default) for s in steps]
    proc = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, text=True)
    results, seen = [], 0
    for line in proc.stdout:
        line = line.rstrip()
        print("   probe:", line)
        if line.startswith("SETTLED"):
            label, want = expected[seen]
            if hold_still:
                want = A
            got = linked_sink(prefix)
            results.append((label, want, got))
            seen += 1
    code = proc.wait()
    return results, code


def main():
    probe = sys.argv[1] if len(sys.argv) > 1 else "/tmp/audio_probe"
    before = default_sink()
    modules = []
    try:
        for name in (A, B):
            modules.append(run("pactl", "load-module", "module-null-sink",
                               f"sink_name={name}",
                               f"sink_properties=device.description={name}"))
        failures = 0
        for api, prefix, steps in (
            ("aaudio-play", "cordial-aaudio-", [B, A, ""]),
            ("play", "cordial-audioplayer-", [B, A, ""]),
        ):
            print(f"== {api}: A -> B -> A -> default")
            results, code = one_run(probe, api, prefix, steps, 3)
            for label, want, got in results:
                ok = got == {want}
                failures += 0 if ok else 1
                print(f"   {'ok  ' if ok else 'FAIL'} {label:8} want {want} got {sorted(map(str, got))}")
            if code != 0:
                print(f"   FAIL probe exited {code}")
                failures += 1
            if len(results) != len(steps) + 1:
                print("   FAIL not every step was observed")
                failures += 1

        # Control: the same timeline with the sink asked to be what it already
        # is. The stream must stay on A, or the observations above say nothing
        # about the change having caused them.
        print("== control: aaudio-play, sink 'changed' to A while on A")
        results, code = one_run(probe, "aaudio-play", "cordial-aaudio-", [A], 3, hold_still=True)
        for label, want, got in results:
            ok = got == {A}
            failures += 0 if ok else 1
            print(f"   {'ok  ' if ok else 'FAIL'} {label:8} stays on {A}; got {sorted(map(str, got))}")

        after = default_sink()
        if after != before:
            print(f"FAIL the default sink changed: {before} -> {after}")
            failures += 1
        print("PASS" if failures == 0 else f"FAIL ({failures})")
        return 1 if failures else 0
    finally:
        for m in reversed(modules):
            subprocess.call(["pactl", "unload-module", m])


if __name__ == "__main__":
    sys.exit(main())
