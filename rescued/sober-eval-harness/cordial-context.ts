/**
 * Cordial architecture facts, embedded into the triage prompt so the triage
 * model can classify Cordial-applicability from real, sourced facts rather
 * than guessing from the word "Roblox" + "Linux". This corpus exists FOR
 * Cordial -- an open-source native Roblox-on-Linux runtime -- as prior art,
 * not as an abstract diagnostic-reasoning benchmark.
 *
 * Sourced from this repo's own README.md and docs/HANDOVER.md (read
 * directly, not indexed). Claims HANDOVER.md itself marks `INFERRED` (not
 * directly observed, only reasoned about) are kept marked `INFERRED` here
 * too -- do not launder an inference into a stated fact by dropping the
 * qualifier.
 *
 * @module sober-corpus/cordial-context
 */

/**
 * Cordial's known open/closed GitHub issues at the time this triage step was
 * built (`gh issue list -R luohoa97/cordial --state all`, 2026-08-05). Only 7
 * issues exist -- small enough to pass the whole list into every triage call
 * rather than building a search index. Re-check this list if it goes stale;
 * there is no live lookup here by design (an offline eval script should not
 * depend on a GitHub API call succeeding for every one of 446 triage calls).
 */
export const CORDIAL_KNOWN_ISSUES: ReadonlyArray<{ number: number; state: 'OPEN' | 'CLOSED'; title: string }> = [
  { number: 7, state: 'OPEN', title: 'Fullscreen clips and letterboxes until the workspace is switched away and back' },
  { number: 6, state: 'OPEN', title: 'Unify the two implementations of the profile lock' },
  { number: 5, state: 'OPEN', title: 'NativeSettingsInterface: preferences file and app policy file are never set' },
  { number: 4, state: 'CLOSED', title: 'X11 only: no native Wayland surface' },
  { number: 3, state: 'OPEN', title: 'Flatpak build needs network access, which makes it unreproducible and Flathub-ineligible' },
  { number: 2, state: 'OPEN', title: 'User-Agent and base URL do not match what the real client sends' },
  { number: 1, state: 'CLOSED', title: 'Mouse scroll wheel is dropped instead of mapped to ACTION_SCROLL' },
]

/**
 * The architecture/context block embedded verbatim in the triage system
 * prompt. Deliberately written as prose the model can reason from, not a
 * bare fact list -- the judgment call ("does this problem class survive the
 * architecture difference") needs the reasoning connective tissue, not just
 * keywords.
 */
export const CORDIAL_ARCHITECTURE_CONTEXT = `
Cordial (luohoa97/cordial) is an open-source, GPL-3.0, native Roblox-on-Linux
runtime. Its architecture is fundamentally different from a Wine-based
approach like Sober's: Cordial loads Roblox's own Android x86-64 build
(libroblox.so, the same binary Roblox ships for Android phones) directly on
Linux, through a ported AOSP bionic linker, a bionic/glibc shim, a JNI VM
(libjnivm) in place of Android's ART, and a custom framework layer answering
the platform calls the client makes. There is no Windows/Win32 API
translation, no Wine prefix, no DirectX-to-Vulkan shim, no emulator, no
container, no virtual machine -- it is a from-scratch Android-compatibility
runtime, not a Windows-compatibility one. This means: any Sober problem whose
root cause is specifically about WINE (Wine prefixes, WINEDLLOVERRIDES,
DXVK/VKD3D, .NET/DirectX translation quirks, Proton versions, winetricks,
Windows-side registry/config) is almost certainly Sober-specific and does NOT
transfer to Cordial, because Cordial has none of those components.

Where the two DO share real risk surface -- because both are native Linux
Flatpak applications talking to the same host stack, regardless of what runs
inside:
- GPU/graphics: Cordial renders via Vulkan (primary) or GLES2, directly
  against the host's Mesa/Vulkan drivers, same as any native Linux Vulkan
  app -- Mesa bugs, Vulkan validation errors, ASTC/texture-format support,
  and present-mode (FIFO vs MAILBOX) issues are directly relevant.
- Windowing/compositor: Cordial's shell is GTK4/libadwaita
  (AdwApplicationWindow), targeting Wayland as its primary/developed backend;
  X11 still starts via Flatpak's fallback socket but is explicitly NOT
  developed further (per Cordial's own ADR-011). So a Wayland-specific bug
  (fractional scaling, HiDPI blur, surface sizing on fullscreen/workspace
  switch, pointer lock/capture through compositor portals) is squarely
  relevant; an X11-only bug is lower priority (Cordial treats X11 as a
  fallback, not a target).
- Flatpak sandbox: Cordial ships as org.cordial.Cordial, a Flatpak, with the
  same class of portal/permission surface as any sandboxed Linux app
  (filesystem access, XDG data dir remapping, network/D-Bus portal grants).
  Sober's Flatpak permission-override issues are directly relevant.
- Audio: Cordial bridges OpenSL ES to PipeWire (native Linux audio), not
  through Wine's audio translation (winepulse/winealsa). A PipeWire routing
  or device-selection bug plausibly transfers; a Wine-audio-stack bug does
  not. NOTE: Cordial's own audio path is itself unverified inside a real
  experience (INFERRED risk, not confirmed working) -- treat matching audio
  reports as high-value, not just plausible.
- Input: Cordial receives keyboard as Linux evdev keycodes and mouse via
  compositor pointer-lock APIs, natively -- not through a Windows input
  layer. Keyboard-mapping and pointer-capture bugs are relevant; in fact
  Cordial has two OPEN, UNRESOLVED gaps of exactly this kind (see below).
- Networking/sign-in: Cordial signs in via a device "Quick Sign-in" code
  flow, independent of any embedded browser or Wine-side cookie jar. Generic
  network/HTTPS/User-Agent/API-version-mismatch bugs are relevant (Cordial
  has an open issue, #2, of exactly this shape); a bug specifically about a
  Wine-embedded browser or Windows credential storage is not.
- Launch arguments / FastFlags: Cordial has its own FastFlag override system
  (profile-scoped flags.json) conceptually similar to Sober's launch-args
  workarounds -- a Sober issue whose fix is "set this FastFlag" is generally
  a strong match; the exact syntax differs but the underlying flag and cause
  usually still applies.

Cordial's own known, currently OPEN problems (useful for spotting an exact
match, not just a category match) -- cite the issue number in your rationale
when a case's problem class overlaps one of these:
${CORDIAL_KNOWN_ISSUES.map((i) => `  #${i.number} [${i.state}] ${i.title}`).join('\n')}

Additional known-open gaps from Cordial's own maintainer handover notes (not
filed as separate GitHub issues, but real and current):
- Typing into text fields draws nothing until the field loses focus (no
  EditText-equivalent overlay yet).
- Roblox is never told it may capture the pointer in first person, so the
  cursor walks off the window in that mode (a real, unresolved pointer-lock
  gap -- INFERRED that a granted lock would actually move the camera, since
  that specific link has not been observed end-to-end).
- Web views (Marketplace, Profile, Communities) are unimplemented.
- Textures/meshes/font sometimes render wrong; leading suspect is the
  MAILBOX-vs-FIFO present-mode substitution, unconfirmed.

When classifying, weigh the ARCHITECTURE difference above, not the surface
similarity of "Roblox" + "Linux". A Sober issue about a Wine registry key is
sober-specific even though it superficially "runs Roblox on Linux". A Sober
issue about Wayland fractional scaling is applies even though Sober's own
implementation of scaling is unrelated to Cordial's.
`.trim()
