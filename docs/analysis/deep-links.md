# Deep links: what a `roblox://` URL has to reach, and what happens when it does

**Status:** established by running, on Roblox for Android 2.732.1043
(`com.roblox.client`), against build `0.3.0-18-g4cb7624-dirty` — a working tree,
because this is the change that introduced the path, and other agents were
editing `crates/cordial-shell/` in the same tree at the time. Ten launches, all
on `--profile agent-deeplink`, none signed in. Sources: the shipping APK's dex
(`tools/dex_method.py`), `readelf --dyn-syms` over `libroblox.so`, the
`AndroidManifest.xml`, `docs/traces/waydroid-roblox-startup.log.gz`, Roblox's
own `ClientSettings` document, and the engine itself.

**Bottom line up front.** The engine never asks anybody for a URL. The URL is
delivered *to* it, and the delivery that works is a message-bus publish:

```text
MessageBus.publishRaw("Linking.detectURL", "{\"url\":\"roblox://…\"}")
```

after which the app shell answers on `Game.launch` with the place named in the
link. Everything else that looked like the entry point is not.

---

## 1. The engine asks for nothing

The starting hypothesis was Android's: `Activity.getIntent()` returning an
`Intent` whose `getData()` is the URL, read from native code over JNI.
`docs/analysis/framework-classes.txt` does list `Landroid/content/Intent;`,
`IntentFilter` and `IntentSender`, which is what made it worth checking.

It is not what happens, on three independent counts:

* **The Waydroid capture.** Every `Intent` line in
  `docs/traces/waydroid-roblox-startup.log.gz` belongs to pids 7363/7387/8049 —
  Google Play services' `BoundBrokerSvc` and the Google app — and none to
  Roblox's own pid 9880. Roblox's only `Intent`-shaped lines are two flag names,
  `UseExplicitStorePackageForIntents` and `EnableAtomicDeferredIntentProcessing`.
  (Search it with `zgrep`; the file is gzipped and a plain `grep -r` over
  `docs/traces/` finds nothing and looks like a clean negative.)
* **A full Cordial launch.** No `Intent`, `Uri` or `getIntent` appears anywhere
  in the run, and `--dump-classes`' 22-class inventory
  (`docs/analysis/observed-java-surface.md`) contains no `android/content/*`
  class at all.
* **The direction of the API.** Every URL-carrying symbol in `libroblox.so` is a
  `Java_*` export — something Java calls, not something the engine asks for.

So the deep link is not a question the engine asks. It is a statement Cordial
has to make. That is the same shape as the cookie jar (`native/cookies.cpp`):
Cordial is the Java side of this app, and the inward calls are the interface.

## 2. The manifest says which schemes exist

`base.apk`'s `AndroidManifest.xml` declares `com.roblox.client.ActivityProtocolLaunch`
alongside the schemes `roblox`, `robloxmobile` and `robloxglobal`, and App Links
for `https` on `www.roblox.com` and `ro.blox.com`. **`roblox-player` is not
among them** — it is the desktop scheme, which is why Sober handles it and why
§6 below matters.

## 3. The four inward calls, and what each one answered

All exported by `libroblox.so` and declared in the dex:

| Native | Signature | What it did, measured |
|---|---|---|
| `JNIBaseUrlProtocol.maybeHandleColdStartProtocolLaunch` | `(String)Z` | returned **false** for a game link |
| `JNIWebLoginProtocol.maybeHandleColdStartProtocolLaunch` | `(String)Z` | returned **false** for a game link |
| `JNILinkingProtocol.nativeReportReceived` | `(String,String)V` and `(String,String,Z)V` | returned cleanly, **changed nothing observable** |
| `MessageBus.publishRaw` | `(String,String)V` | **this is the one** — §5 |

The two `maybeHandle…` natives are worth keeping in the sequence even though
they say no: they are asked first, they answer honestly, and they are the
base-URL-switch and web-login special cases. A link Cordial cannot place is a
link one of them may yet claim, and their boolean is the only self-describing
answer in this whole area.

`nativeReportReceived` is not called by Cordial. It accepted the URL without
complaint and moved neither `Game.launch` nor `isColdStartDeeplinkToGame()`, and
its second argument would have to be invented — the neighbouring
`getAttributionUrlKey` and the AppsFlyer `DeepLinkResult` classes in the same
dex suggest it is the advertising-attribution channel, which is **INFERRED** and
was not chased.

## 4. The protocol's own vocabulary, read out of the running engine

`JNILinkingProtocol` is otherwise a wall of zero-argument `String` getters.
Calling them (`CORDIAL_DEEPLINK_PROBE=1`) is how the protocol was learned from a
running engine rather than guessed at from symbol names:

```text
getProtocolName             -> "Linking"
getOpenURLId                -> "openURL"
getOpenURLRequestId         -> "Linking.openURLRequest"
getOpenURLResponseId        -> "Linking.openURLResponse"
getDetectURLId              -> "Linking.detectURL"
getPendingURLId             -> "Linking.pendingURL"
getRegisterURLId            -> "Linking.registerURL"
getIsURLRegisteredId        -> "isURLRegistered"
getIsURLRegisteredRequestId -> "Linking.isURLRegisteredRequest"
getHandleEngineURLId        -> "Linking.handleEngineURL"
getHandleLuaURLId           -> "Linking.handleLuaURL"
getHandlePlatformURLId      -> "Linking.handlePlatformURL"
getUrlKey                   -> "url"
getMatchedUrlKey            -> "matchedUrl"
getAttributionUrlKey        -> "attributionUrl"
JNIExperienceProtocol.getLaunchId -> "Game.launch"
```

`Game.launch` is the important one: it is what the app shell publishes when it
wants an experience launched, so it is the observable that says a link was
understood rather than merely accepted. `MessageBus.getLastRaw(id)` reads it
back, which is how any of this is checkable without implementing a `RawCallback`
class for the engine to call into.

## 5. What works

Publishing the URL on `Linking.detectURL` during bring-up — after
`nativeAppBridgeV2InitWithParams`, which is what builds the protocol machinery,
and before `nativeAppBridgeStartLuaAppDM`:

```text
[deeplink] (cold start) Game.launch before publishing: None
[deeplink] (cold start) isColdStartDeeplinkToGame before publishing: Some(false)
[deeplink] (cold start) published Linking.detectURL
[deeplink] (cold start) Game.launch after publishing: None
...
[roblox] app ready: PlatformAccountRouter
[deeplink] (app ready) Game.launch is:
  Some("{\"placeId\":1818,\"referralPage\":\"DeepLink\",\"joinAttemptId\":\"fe7bec78-…\"}")
[deeplink] the app shell asked to launch an experience; the link reached the engine
```

with `--join-url 'roblox://experiences/start?placeId=1818'`. Reproduced on two
consecutive runs (`joinAttemptId` differs, `placeId` and `referralPage` do not).
The engine parsed the URL itself: `placeId` and `referralPage: "DeepLink"` are
its words, not Cordial's, and Cordial passed the URL through as one opaque
string.

**Two things this pins down.**

*The publish is what carries it.* The control is
`CORDIAL_DEEPLINK_NO_PUBLISH=1` — same link, same launch, same everything, with
the one `publishRaw` call suppressed. `Game.launch` stays empty and
`isColdStartDeeplinkToGame()` stays false at both sampling points.

*The effect is asynchronous, and cold start is early enough.* Immediately after
the publish, nothing has changed: the Lua app shell does not exist yet. By the
first `APP_READY` the answer is there. So the message is queued and acted on
when the shell comes up, and Cordial does not need to wait to publish — only to
report. Publishing a second time after `APP_READY` was tried and produces a
*second* `Game.launch` with a second `joinAttemptId`, which is why the code
publishes once and reads back later rather than publishing twice.

`isColdStartDeeplinkToGame()`, the eleven-byte getter `ActivityNativeMain`
consults on Android between initialising the app bridge and starting the app
shell, goes `false -> true` across the same delivery. It is a second, independent
witness to the same event.

## 6. What does not work: `roblox-player://`

The engine carries its own pattern for what a game link looks like, as the
client setting `FStringGameLaunchLinkURL`:

```text
(?:(?:https?://\w+\.roblox(?:labs)?\.com…/games/start\?)|(?:roblox(?:mobile)?://(?:experiences/start\?)?))
(?:(?:id=\d+)|(?:placeid=\d+)|(?:launchData=…)|(?:linkCode=…)|(?:accessCode=…)|
 (?:reservedServerAccessCode=…)|(?:joinAttemptId=…)|(?:joinAttemptOrigin=…)|…)+
```

`roblox(?:mobile)?://` — and no `roblox-player`. Measured: the same link under
`roblox-player://experiences/start?placeId=1818` produces **no** `Game.launch`
and leaves the flag false.

This matters more than it sounds. `roblox-player://` is what roblox.com's play
button emits on desktop, in a different format again
(`roblox-player:1+launchmode:play+gameinfo:<ticket>+placelauncherurl:<url>+…`),
and it is the handler Cordial is taking over from Sober. **Taking that handler
away from Sober without handling it is the failure this whole investigation was
told to avoid**, so Cordial prints a warning naming the pattern when it is given
one, rather than accepting the click and going quiet.

Translating the desktop format to `roblox://placeId=…` looks possible — the
`placelauncherurl` parameter carries the place id — but the same link also
carries a one-time `gameinfo` authentication ticket that the Android client does
not use, and none of that has been tested. It is **not implemented**, and it is
the obvious next piece of work.

## 7. Verified, inferred, and not established

**Verified by running, this session:**

- The engine asks for no `Intent`, `Uri` or URL (§1).
- Both `maybeHandleColdStartProtocolLaunch` natives return false for a game link.
- `nativeReportReceived` changes nothing observable.
- Every string in §4, read from the running engine.
- `Linking.detectURL` + `{"url": …}` produces `Game.launch` naming the place,
  twice, with a suppressed-publish control that produces neither it nor the flag.
- `isColdStartDeeplinkToGame()` moves false -> true across the same delivery.
- `roblox-player://` produces neither.

**Inferred, not verified:**

- That `nativeReportReceived` is the advertising-attribution channel. Its
  neighbours in the dex say so; nothing was measured.
- That `Linking.detectURL` is the *right* message rather than merely a
  sufficient one. The three siblings were published immediately after it in one
  run and produced no further `Game.launch`, which is consistent with detectURL
  having already done the work, and does not prove the others are inert.
- That `JNIBaseUrlProtocol.init(Context)` needs calling at all. It is driven
  with a bare object stand-in, succeeds, and asks nothing of it.

**Not established:**

- **Whether any of this actually joins an experience.** `Game.launch` is the app
  shell *asking*. A join needs a signed-in account, and no account was used —
  every run here ends at `app ready: Landing`, which is where a signed-out
  client belongs. This is the single largest gap and it cannot be closed without
  an account.
- What the desktop `roblox-player://` format would have to be translated into,
  or whether translating it is sound (§6).
- Whether `launchData`, `linkCode`, `accessCode` and the rest survive the trip.
  Only `placeId` was exercised; the engine's own pattern lists the others, so
  they are expected to, which is not the same as having seen it.
