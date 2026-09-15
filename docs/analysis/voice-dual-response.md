# Voice permission delivery and callback capture

2026-09-14, Roblox 2.738.0.1397, custom Cordial 0.14.2.

Mocktail's permission bridge publishes the protocol response before resolving
the async callback. Cordial previously only resolved the callback. Its
`AUTHORIZED` answer therefore did not establish that all consumers received
the permission result.

Source: Mocktail `src/runtime/roblox_permissions_bridge.cc` at
`d76d75b7f8d6a3df8e4f86a8c3dee54a9571a8d5`, introduced by
`859f8f2b8feaa7490dd827ae2fa71c9c554040ad`.

## Observed control

Protocol publication followed by async resolution is now the default for
PermissionsProtocol. `CORDIAL_PERMISSIONS_DUAL_RESPONSE=0` restores the old
async-only route; `1` explicitly selects the default. The diagnostic launcher
used `1` while this was still an experiment, so the controlled run did not
depend on changing the normal launcher.

In place 14236925335, dual delivery displayed a microphone icon and logged
`WebRtcAudioManager.init`. Relaunching the same binary and profile with the
switch set to `0` removed the icon and produced only async permission responses,
without WebRTC audio initialisation. Enabling it again restored both. This was
an on/off/on comparison across separate server joins. Unmuting through the
development MCP reproduced the callback input refusal in both enabled runs.

## Blocker reached by the experiment

Dual delivery reached a new explicit failure:

    refusing an input stream that installed a data callback

PipeWire showed Cordial audio output but no capture node. At that point voice
transmission did not work: the AAudio bridge only implemented blocking input
reads and refused callback-driven capture. The comment claiming the engine
necessarily used blocking reads was an inference from its imports and was wrong.

## Automated checks

Callback input now consumes the existing `CaptureStream` ring. Native checks
establish 48 kHz mono S16 delivery in whole 480-frame callbacks, callback STOP
teardown, pause/stop/restart cancellation, refusal of close from within the
callback, and continued blocking-read support. Both callback-driver and AAudio
ABI checks passed 100 release repetitions and 25 AddressSanitizer repetitions.
Transport checks establish protocol publication before async resolution and
preserve async-only rollback. The isolated release workspace build passed;
all-feature workspace tests reported 1,102 passed, zero failed and 14 ignored.

## Tester manual test

After the callback fix, a tester confirmed audible microphone transmission
to another player in a real voice-enabled game. This establishes the end-to-end
user outcome that the native harness and the permission on/off/on control cannot
establish independently.
