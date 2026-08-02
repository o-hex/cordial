# Updating the Roblox build

**Status:** implemented. `crates/cordial-update` is the logic and
`crates/cordial-shell/src/updater.rs` is the header-bar button and the settings,
built 2026-08-03. The decision it was waiting on is
[ADR-015](../adr/ADR-015-fetching-the-roblox-build.md), and it is accepted.

**The settings are one dropdown and two switches, not two dropdowns.** This
document argued for two dropdowns and the argument is kept below, along with
what happened to it: the owner specified the other shape, twice, and the
contradiction that argument warned about is now something the settings page says
out loud rather than something the types forbid.

**Three things in this design turned out not to be true when the code was
pointed at Roblox**, and all three are corrected in place below rather than left
for somebody to rediscover: there is no working version endpoint for the Android
build, "unmetered" is a state an ordinary desktop does not report, and Sober —
which this document twice treated as the project that knew where the APK came
from — documents its route perfectly clearly, and that route is Google Play.
Measured 2026-08-02 and 2026-08-03, and
`cargo run -p cordial-update --example update_probe` re-measures all of it in one
command.

Cordial ships no Roblox build and never will, so today the APK arrives because
somebody installed Sober and let it download one. That works and it is a poor
dependency: the install instructions currently tell people to install a
different Roblox client first, in order to obtain a file.

This is the design for fetching it directly.

## The control, in one button

A single button in the shell's header bar, immediately left of Settings. It is
**always present**. Its icon says which of three states it is in.

| state | icon | what clicking it does |
|---|---|---|
| nothing newer known | download | which build is installed, its changelog, and what is and is not knowable about whether it is current |
| update available | download, with attention styling | the new version's changelog, and an **Update** button |
| manual, nothing checked yet | refresh (circular arrow) | checks once, now; if that finds an update the button becomes the row above |

**All three are built, and the middle one is harder to reach than it reads.**
"Update available" is a comparison, and Cordial has only one of its two operands
for most people: the newest engine major comes from the release notes, which
work, and the installed version comes from `cache::recorded_version`, which is
`None` unless Cordial fetched the build itself. So the ordinary state is the
first row, and its dialog says *whether this build is current cannot be
established* rather than "up to date" — the endpoint that would settle it
answers 500, and rounding that to "you are current" is the exact failure this
document exists to prevent.

The download icon is `system-software-install-symbolic` rather than
`software-update-available-symbolic`. The second is the icon that *means* an
update is waiting, and wearing it in all three states would be the attention
styling drawn instead of written.

**Always present, rather than appearing when an update exists.** A control that
comes and goes is hard to find at the moment you want it, and it moves
everything beside it when it arrives. The cost of keeping it is one icon; the
cost of hiding it is that nobody can answer "what version am I on" without
finding a menu. Showing the current version and its changelog is useful on its
own, which is why the first state is not a dead end.

The *Manual* state deliberately still offers a check — opening the window is one.
Turning off automatic checking is a statement about background network use, not a
refusal to ever know.

## Settings

**Auto update** — one dropdown:

- *Update in background* — check, and fetch what it finds, subject to the two
  switches below.
- *Ask* — check, and when Cordial starts with an update waiting, **open the
  changelog with an Update button on it**. A dialog rather than a badge: the
  point of asking is that somebody is asked, and a quiet state change is what the
  other two modes already leave behind.
- *Manual* — no request of any kind until the header-bar button is pressed, at
  which point it checks once.

**Download on Wi-Fi** and **Download on metered connection** — one switch each.
Wi-Fi on and metered off is the default.

### Why the third option is called Manual

Not *Disabled*, and the word is the setting's meaning. It turns off every
automatic behaviour — no background check, no dialog on launch, no download —
and it does not turn the feature off, because the button still checks on demand.
This document already says why in its own words: turning off automatic checking
is a statement about background network use, not a refusal to ever know.
*Disabled* contradicts that sentence; *Manual* states it.

### The two-dropdown argument, and what happened to it

This section used to specify a second dropdown — *Download over: any connection
/ unmetered connections only* — and argued for it like this, which is kept
because it was not wrong:

> Two dropdowns rather than a mode plus two independent toggles. Three controls
> that can each be set independently produce combinations with no defined meaning
> — "update in the background, but never download" is a setting that has to
> either lie or explain itself, and a settings page that can express a
> contradiction will eventually be asked to honour one.

**The owner specified the dropdown-plus-two-switches shape, twice, and it is
their call.** The contradiction is real and reachable: both switches off with
*Update in background* selected can never download anything. What the argument
above was actually objecting to is a page that expresses a contradiction
*silently*, so the contradiction is named instead —
`cordial_update::settings::NEVER_DOWNLOADS` is the sentence, the settings page
shows it as a warning row that appears exactly when both switches are off, and
`may_download` gives the same words back when a download is held.

`UpdateSettings::plan` is still total, which is the property the two-dropdown
shape was chosen for: every combination of the three controls and each of
NetworkManager's four answers maps to exactly one `Plan`, and
`every_setting_combination_has_exactly_one_plan` is the test that would have to
be told what a fourth control meant.

### Wi-Fi is not something Cordial can see

There is no radio in any of this. NetworkManager's `Metered` property is the only
question asked, and it is about who pays for the bytes rather than about the link
layer. *Download on Wi-Fi* therefore governs every connection that is not
metered, a wired desktop included, and *Download on metered connection* governs
the rest; between them they cover everything, which is what makes "both off" mean
"never". The switch's own row carries that sentence, because the alternative is
somebody unplugging an ethernet cable to find out.

## Metered connections

`org.freedesktop.NetworkManager`'s `Metered` property, over D-Bus. Same
mechanism as GameMode and the Secret Service, so no new dependency.

**It has four values, not two**: yes, no, guess-yes, guess-no. A phone hotspot
commonly reports *guess-yes*. Both guesses must be treated as metered — reading
a guess as "not metered" is how somebody's data allowance pays for a 115 MB
download they never asked for. Unknown is also metered, for the same reason, and
so is any number a later NetworkManager adds that this code does not recognise.

**What this costs is larger than it reads, and it was not obvious until it was
measured.** An ordinary desktop on a wired or wireless LAN does not report `NO`:

```text
$ busctl --system get-property org.freedesktop.NetworkManager \
    /org/freedesktop/NetworkManager org.freedesktop.NetworkManager Metered
u 4
```

Four is `GUESS_NO`. So on that machine — a perfectly ordinary one, nothing
unusual about its connection — the default settings do **not**
background-download. A guess is metered, so it is *Download on metered
connection* that governs an ordinary LAN, and that switch is off by default. It
checks, and waits to be asked, saying which of the four answers it got. That is
the rule doing exactly what it says and it is the safe direction, but anyone who
reads "Download on Wi-Fi, on" as "downloads for most people" will be wrong. The
dialog's connection row is where that is made legible: it prints
`Metered::describe` and then names which of the two switches is the one
governing this connection.

Whether `NO` is a high enough bar is a fair thing to argue about — the
alternative is treating `GUESS_NO` as unmetered and `GUESS_YES` as metered,
which is what the guess is *for*. It is not argued here because the design and
ADR-015 both say guesses are metered, and quietly implementing the other reading
is the thing AGENTS.md says not to do. If it should change, change it here and
in `cordial_update::metered::is_metered`, whose test names the case.

## Checking is cheap; downloading is not

Check on launch, in the background, after the window is up. One request against
a version endpoint costs nothing, and Sober doing it every launch is not the
waste it appears to be — the mistake would be doing it *synchronously*, where a
slow or absent network delays the window.

The download is the expensive half and the only part the settings above govern.

### There is no version endpoint for the Android build

This section used to assume one, and it was wrong. Measured against
`https://clientsettingscdn.roblox.com/v2/client-version/<binaryType>`:

| binaryType | answer |
|---|---|
| `WindowsPlayer` | 200 `{"version":"0.732.23.7321040","clientVersionUpload":"version-145f189a6a974303","bootstrapperVersion":""}` |
| `MacPlayer`, `WindowsStudio64` | 200, same shape |
| `AndroidApp` | **500** `{"errors":[{"code":3,"message":"Error while fetching version information."}]}` |
| `iOSApp`, `UWPApp` | 500, the same error |
| `AndroidPlayer`, `AndroidStudio` | 400 `{"errors":[{"code":2,"message":"Invalid binaryType."}]}` |

The 400-versus-500 split settles the obvious follow-up: `AndroidApp` is not a
name to keep hunting for alternatives to. An unrecognised name is refused *as* an
unrecognised name, and `AndroidApp` is accepted and then fails to produce a
version — the same as the other two platforms Roblox ships through an app store.
It is also the name `client_settings` already established on the same host, by
the same experiment, for the flag document.

So the version check fails today, and it fails **saying so**, naming the URL and
quoting what came back. That is the required behaviour rather than a gap:
ADR-015 accepts that these endpoints are Roblox's to change without notice and
requires a message naming what could not be reached instead of something that
appears to work. A check that reported "up to date" here would tell the user they
were current while Roblox refused their build server-side, which is the exact
failure this whole document exists to prevent.

### The changelog is the half that works

Roblox's release notes are on the DevForum, which is Discourse, which serves any
page as JSON:

```text
$ curl -sSL https://devforum.roblox.com/c/updates/release-notes.json
→ 200, redirected to .../release-notes/62.json
→ topic_list.topics[]: {"id":4763851,"slug":"release-notes-for-732",
                        "title":"Release Notes for 732", "created_at":"2026-07-29T…"}
```

Every entry is titled `Release Notes for NNN`, and `NNN` is the engine major —
the `732` in `0.732.23.7321040`, and the `Version=732` the client logs about
itself in `docs/traces/`. So the newest release-notes major is the newest engine
Roblox has shipped, and the button has something true to show even while the
version endpoint does not answer.

One trap, written down because a 403 from a public forum reads as something
else: a topic must be fetched by **slug and id both**. `/t/4763851.json` answers
403; `/t/release-notes-for-732/4763851.json` answers 200.

### Where the APK comes from: Roblox does not publish one

This section used to be headed "not established", which reads as an
investigation that ran out of time. It has now been done, on 2026-08-03, and the
answer is not "we could not find it" — it is that **Roblox publishes no Android
build outside app stores**. Three places where a public artefact would surface,
and it is absent from all three:

| where a public Android artefact would be | what is there |
|---|---|
| `setup.rbxcdn.com/DeployHistory.txt` | 200, 7210 lines. Product names are `Studio`, `Studio64`, `WindowsPlayer`, `RccService`, `Client`, `MFCStudio`, `StudioBeta`. `android` and `apk` occur **zero** times |
| `setup.rbxcdn.com/android/DeployHistory.txt` | **403** `AccessDenied` — what that bucket says about a prefix it does not have |
| `roblox.com/download`, Android section | Google Play and the Amazon Appstore. No file linked |

Together with the 500 for `AndroidApp` above, that is the whole answer. The
probe re-asks the first two on every run, with the desktop path as the control,
because a claim that nothing re-measures decays without anyone noticing — and
the day Roblox does publish an Android path, this is the line that changes.

### What Sober does, since it was the project assumed to know

**The previous version of this section said Sober "documents nothing about where
it gets it". Half of that was right and the useful half was wrong.** Sober is
closed source — `vinegarhq/sober` contains a README, an icon and issue
templates, and no code — but it documents its distribution route plainly, in its
own licence and privacy notices:

their licence notice says users download the bundle themselves and that Sober
offers "an option to connect to Google Play in-app to download the bundle
automatically"
([notice.txt](https://sober.vinegarhq.org/notice.txt)), and their privacy notice
describes that same feature as handing back a Google Play download link
([privacy.txt](https://sober.vinegarhq.org/privacy.txt), *Automatic Downloads*).

So **Sober goes to the app store.** It had not found a Roblox URL either; it
went where Roblox put the file, on the user's own Google account, after the user
opted in during onboarding.

VinegarHQ's one open-source component that touches the Android build corroborates
this from the other side. `vinegarhq/custard`, their deployment tracker, queries
`clientsettings.roblox.com/v2/client-version/` for `WindowsPlayer` and
`WindowsStudio64` only, and discovers the Android version by watching
`com.roblox.client` on **Aptoide**, a third-party APK mirror. The project that
would most like that endpoint to answer for Android does not ask it.

### Why neither of Sober's routes is taken here

**Google Play** means holding the user's Google credentials and speaking the Play
protocol as a registered device. ADR-015 permits fetching from *Roblox's own
distribution*, and a store account is not that. A Cordial that asked for a Google
password would be a different program from the one that ADR was written about.

**This makes one sentence of ADR-015 wrong, and it is not this document's to
fix.** ADR-015 argues that fetching is unremarkable because "Sober fetches the
same build from the same place, in the open" — where *the same place* means
Roblox's own distribution, which is the thing the ADR permits. Sober does not.
It fetches from Google Play. The ADR's conclusion survives intact, because
nothing in it turns on Sober's route and Cordial still refuses everything but
Roblox's own distribution, but the supporting sentence is no longer true and
should be corrected in ADR-015 itself rather than only here.

**A mirror is worse rather than easier.** Aptoide serves the file today, and
taking it would mean Cordial quietly fetching an APK from a third party while its
ADR says it fetches from Roblox — with an MD5 supplied by the mirror as the only
thing vouching for bytes the mirror itself supplied, which is not a check, it is
the mirror agreeing with itself.

**So Cordial ships no URL, and the refusal now names the stores rather than
saying "not established".** A user meeting it needs to know the file is
obtainable and where from; a refusal that only says Cordial cannot reads as
Cordial being broken. `CORDIAL_ROBLOX_APK_URL` and `CORDIAL_ROBLOX_APK_SHA256`
point it at a location the user chose. `download::Source::official` remains the
one function to fill in if Roblox ever publishes a deployment path, and the test
asserting there is none should be rewritten to assert what it is rather than
deleted.

Everything downstream of the URL is built and exercised: the download streams
with a byte cap applied to what arrives rather than to `Content-Length`, hashes
as it streams, and only gives the file the name anything looks for once the
digest matches. Verified against a real 1.26 MB HTTPS transfer, with a wrong-hash
control that refused and left the directory empty.

## Verifying what arrives

Whatever is downloaded is verified before it is used, for the same reason the
plugin registry hashes archives: an unverified download is a URL you are
trusting, and a URL is not a claim about content. See
[ADR-014](../adr/ADR-014-plugin-registry-and-unpacking.md), whose extraction
rules apply here in full — the APK is a zip and every refusal in that list is
about zips.

`cordial_update::apk` is that list applied to a zip, and each refusal has a test
that fails when the check is deleted. Two places where a zip is weaker than the
`.tar.zst` ADR-014 chose, both stated in the module rather than discovered later:
an entry carrying **no** Unix mode at all has to be treated as a regular file,
because most zip producers write no Unix attributes and refusing them would
refuse most real APKs — what is refused is an entry whose mode *does* claim to be
something other than a file or a directory. And zip cannot express a hard link,
so there is no field to check and the code says so rather than carrying a refusal
that can never fire and looks like coverage.

Cordial takes exactly one entry out of an APK, `lib/x86_64/libroblox.so`, and
writes it to a path of its own choosing — so nothing the archive says about
*where* an entry goes is ever acted on. The path refusals are still applied to
the whole archive, because they are how a hostile APK gets noticed at all: an
archive built to escape somebody's extractor is not an archive to take one file
out of and shrug about the rest.

### The engine cache is stamped, and now actually is

The extracted engine cache in `~/.cache/cordial/lib/x86_64` is stamped with the
APK it came from, so a new build re-extracts and an unchanged one does not.
Presence alone used to be the whole test, which meant a new Roblox build left
the old engine in place and Cordial ran it against the new APK's assets.

That was still true of `crates/cordial-shell/src/install.rs` when this paragraph
was first written — `justfile`'s `client` recipe had the bug and had it fixed,
and the shell had not — so the sentence above described the justfile and read as
though it described Cordial. It now describes both. `cordial_update::cache`
writes the same string the recipe writes, `stat -c '%s %Y %n'`, because the two
share one cache directory and two formats in one file would mean `just client`
and the shell each seeing the other's stamp as a change and re-extracting 115 MB
in turn.

An unstamped cache counts as stale, so everybody upgrading past this re-extracts
once. The mtime is deliberately not read off the extracted engine: zip preserves
the timestamp stored in the archive, so a file extracted this morning has an
mtime in 1981.

## The decision this needed first

[ADR-015](../adr/ADR-015-fetching-the-roblox-build.md), and it is accepted. What
follows is the reasoning that led to it, kept because the ADR is the decision and
this is where the question came from.

The README says, and it is load-bearing: *Cordial ships no Roblox code, ever. No
APK, asset, or decompiled material committed, vendored, or pasted.*

Fetching at the user's request does not violate the letter of that. Nothing is
committed, nothing is vendored, nothing is redistributed, and the file comes
from Roblox's own servers to the user's own machine. Sober does exactly this in
the open.

But it changes what Cordial *is* — from "bring your own build" to "a thing that
fetches Roblox binaries" — and this project has no arrangement with Roblox and
has been careful not to imply one. That is a posture decision and it belongs in
an ADR that says plainly what this does and does not do: fetches at the user's
request, verifies what it got, never modifies it, never serves it to anyone
else. So the answer exists before the question is asked.

## What the shell half turned out to be

`crates/cordial-shell/src/updater.rs`. The button, its three states, the dialog
behind it, and the settings group on the Roblox page.
`cordial_update::settings::Automatic` exposes `index`/`from_index` for the
dropdown, the same seam `shell_config::AppearanceScheme` already uses; the two
switches are plain booleans on `ShellConfig` and `updater::update_settings` puts
all three back together for `UpdateSettings::plan`.

The check runs **after the window is up**, on a thread of its own, and that is
the one part of this design that is a hard requirement rather than a preference.
It is a `std::thread::spawn` whose answer is collected by a `glib` timeout
polling an `mpsc` receiver: a GTK widget is not `Send`, so the answer has to be
picked up on the thread that owns the widgets whatever carries it, and this crate
gains no async runtime for one request.

Three things the shell half added that this document had not anticipated:

**The Update button has to be honest, and there is nothing for it to do.** It
appears only in the update-available state, and what it opens says where the
build comes from — Google Play, the Amazon Appstore — and offers the file picker.
It never implies a fetch, because there is none to perform.

**Opening the window is itself a check**, in every mode. That is what makes the
refresh icon in *Manual* do what it draws, and it means there is no "not checked"
state inside the dialog: that state lives on the button.

**`CORDIAL_SHELL_PRESENT=settings,update` opens those windows at startup.** A
test seam, and it exists because AGENTS.md forbids synthesising input at the
compositor and Wayland has no window-targeted injection, so "click the button and
photograph the result" is not an available sentence. It goes through the same
action and the same button handler a click does.
