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

## Confirmed again on the shipped build, 2026-09-15

Repeated on the merged code rather than the combined development build, in the
same place 14236925335, joined by `--join-url
roblox://experiences/start?placeId=<id>` and connected through that game's own
Connect control. The maintainer confirmed voice working. Line 456 of the run:

    I/Cordial-Audio  WebRtcAudioManager.init reports success: WebRtcAudioTrack
    implements the downlink and WebRtcAudioRecord implements the uplink, so a
    voice session can both send and receive.

`refusing an input stream that installed a data callback` — the blocker this
change was written against — did not recur, and the run answered every Android
call it made (`stubs called: 2 distinct of 650`, both ZSTD tracing).

**Grep for `WebRtcAudioManager`, not for the voice channels.** Setting
`FLogVoiceChatLogs=7` and `FLogVoiceChatControlPlaneTracingLogs=7` in the
profile's `flags.json` produced nothing: the string `VoiceChat` does not appear
once in 522 lines of a session where voice demonstrably worked. Searching for
the vocabulary in issue #47 — `voice_service_init`, `VoiceChatInternal`,
`ClientMuteUnmuteOperation` — returns empty on a *healthy* run and reads as a
total failure of voice. That mistake was made here before the log was searched
for the string this document already named. The positive marker is emitted by
Cordial's own `Cordial-Audio` layer, and a run that only reaches Home does not
contain it, so its presence is attributable to the join and the connect.
