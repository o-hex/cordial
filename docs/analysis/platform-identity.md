# Which platform Cordial tells Roblox it is on

**What changed:** `NativeUserJavaInterface.getPlatformName()` answered `"Android"`
and now answers `"Linux"`, overridable with `CORDIAL_PLATFORM_NAME=<name>`.

**What is established:** the engine asks for that string, four times per cold
start, once inside each app-bridge call. `Linux` is one of the engine's own
platform names. It is the only platform-shaped *value* in the Java surface
examined here — the four parameter classes carry none, and the `AConfiguration_*`
route is ruled out at the symbol level.

**What is not:** that this changes anything. Twenty-five controlled runs found
**no behavioural difference that reproduced** — the engine reads the string and
never prints it, and the one candidate effect (§3b) appeared three times and then
failed to repeat across twenty-one more runs. The claim that experiences see a
desktop client is `INFERRED` and needs the check in §4, which wants somebody who
can sign in and enter a place.

---

## 1. Why `Linux` rather than `Windows`, and why not `Android`

`Android`, `AndroidTV`, `Linux`, `MetaOS`, `SteamOS`, `Windows` and `XBoxOne` are
standalone tokens in libroblox.so's string table, in the region that holds
`Enum` item names. They are the engine's own vocabulary, not words invented here.

So the choice is between three answers, and only one of them is true:

* `Windows` would be a lie, and the sort that invites the service to route the
  client down a path built for a client that does not exist here.
* `Android` is *also* a lie. The host is a desktop Linux machine with a keyboard
  and a mouse and no touchscreen. Cordial already tells the engine the last of
  those three through `PlatformParams.isTouchDevice`, and the engine reads it
  (§2e) — so answering `Android` contradicted a value the engine had actually
  taken from us.
* `Linux` is what the machine is, said in a word the engine already knows.

AGENTS.md's rule that a stub must never lie is what decides this, and it decides
it in both directions. The value is defensible on that ground alone, separately
from whatever it fixes.

## 2. Where the platform identity is, and where it is not

### 2a. It is not a parameter field Cordial was failing to answer

Read out of the shipping APK's dex rather than from Cordial's C++, the four
parameter classes declare, in full:

| Class | Declared members |
|---|---|
| `model/DeviceParams` | 21 fields: `appBuildVariant appVersion country cpu64Bit deviceName deviceSku deviceTotalMemoryMB displayPhysicalHeightPixels displayPhysicalWidthPixels displayResolution isChrome isLowRamDevice largeMemoryClass lowMemoryKillerBackgroundAppThreshold lowMemoryKillerForegroundAppThreshold manufacturer memoryClass networkType osVersion socModel testDeviceName` |
| `model/PlatformParams` | 7 fields: `assetFolderPath dpiScale isKeyboardDevice isMouseDevice isTouchDevice viewportHeightMm viewportWidthMm` |
| `autovalue/InitParams` | `baseURL buildVariant deviceParams platformParams userAgent isPotato isTablet isVrDevice vrContext` |
| `autovalue/StartAppParams` | `appStarterPlace appStarterScript appUserId isUnder13 membershipType platformParams selectedTheme surface username vrContext` |

`native/init_params.cpp` already answers every one of those. There is no
platform-identity field among them that Cordial was leaving null. Candidate
eliminated.

### 2b. It is not `AConfiguration`

`android/content/res/Configuration` carries `touchscreen`, `keyboard`,
`keyboardHidden`, `hardKeyboardHidden`, `uiMode` and `navigation`, which is
exactly how Android would express "this is a TV" or "this has a hard keyboard" —
and the engine does ship both `Android` and `AndroidTV`, so *something* picks
between them at runtime.

It is not this. The engine's undefined-symbol list contains nine
`AConfiguration_*` names and only nine:

```text
AConfiguration_new           AConfiguration_getLanguage      AConfiguration_getScreenSize
AConfiguration_delete        AConfiguration_getCountry       AConfiguration_getNavHidden
AConfiguration_fromAssetManager  AConfiguration_getScreenWidthDp  AConfiguration_getScreenHeightDp
```

`AConfiguration_getKeyboard`, `getTouchscreen`, `getUiModeType` and
`getNavigation` are not imported at all, so no call to them can exist. This is a
fact about the dynamic symbol table at load time, not a reading of the code.
Consistent with that, no `AConfiguration` stub was hit on any run in this pass —
only the two ZSTD tracing hooks were, on every run.

### 2c. It is not `android.view.InputDevice`

Already established elsewhere and re-confirmed here only in the negative: nothing
in `android/view/` beyond `KeyEvent`, `MotionEvent` and `Surface` appears in the
string table. Not re-derived.

### 2d. It *is* asked for as a string, once per app-bridge stage

`com.roblox.engine.jni.user.NativeUserJavaInterface.getPlatformName()Ljava/lang/String;`
is declared in the dex and Cordial hooks it. Instrumented with an `fprintf` and
run, the engine calls it **four times** on a cold start, at:

```text
  initStorageManagerNativeV3 ok
[probe] NativeUserJavaInterface.getPlatformName called
  init params set
...
  nativeUpdateAdapterInit ok
[probe] NativeUserJavaInterface.getPlatformName called
  app bridge initialised
  [cookies] restored 0 domain(s) from .../cookies
[probe] NativeUserJavaInterface.getPlatformName called
  Lua app DataModel started
  task scheduler foregrounded
[cordial] app start as nobody signed in
[probe] NativeUserJavaInterface.getPlatformName called
  app started with surface
```

One call inside each of `nativeAppBridgeSetInitParams`, `nativeAppBridgeV2Init`,
`nativeAppBridgeStartLuaAppDM` and `nativeAppBridgeV2StartAppWithParams`.

It fires with nobody signed in, at app-bridge lifecycle boundaries rather than
alongside the identity mirrors, which is what argues it is a property of the
client rather than of the account. That is an argument, not a proof: the flag
names `FFlagAddPlatformNameToProfileHeader`,
`DFFlagConsumePlatformNameOverAlternateName` and
`FFlagFixInExperienceNilPlatformNames` in Roblox's own settings document all read
as the *user's* platform name, the console-gamertag sense, and this method sits
on a class whose other members are all account fields. Both readings point the
same way for the value — a Linux desktop's platform name is Linux either way —
but which of the two it is has **not** been established.

### 2e. `isKeyboardDevice` and `isMouseDevice` are never read

This was the premise the whole task started from — "the peripherals are already
described as desktop and the client still behaves as mobile, so something else
carries the platform identity". Half of that premise is false, and it was worth
one launch to find out.

Registered as getter functions rather than plain field hooks, so that each read
is observable (`CORDIAL_TRACE_PARAM_READS=1`), one cold start gives:

| Field | Times read |
|---|---|
| `DeviceParams.osVersion` | 1 |
| `DeviceParams.deviceName` | 1 |
| `PlatformParams.dpiScale` | 3 |
| `PlatformParams.isTouchDevice` | 2 |
| **`PlatformParams.isKeyboardDevice`** | **0** |
| **`PlatformParams.isMouseDevice`** | **0** |

The reads land at the same app-bridge boundaries `getPlatformName` does —
`dpiScale` and `isTouchDevice` inside `nativeAppBridgeSetInitParams` and
`nativeAppBridgeV2Init`, `dpiScale` again inside
`nativeAppBridgeV2StartAppWithParams`.

**The control is inside the same run and it fired.** `DeviceParams.deviceName`'s
getter was called, and the engine then printed the value it was given:

```text
[FLog::Graphics] Vulkan Android Device: Cordial
```

So the probe was working, the receiver was live, and Cordial's own values were
what came back. `isKeyboardDevice` and `isMouseDevice` staying at zero is a
result, not a broken instrument.

Two things follow, and the second is the important one:

* Setting those two fields to the desktop answer has never told the engine
  anything. `native/init_params.cpp`'s own header claimed all three "decide which
  input scheme and which UI layout the engine picks"; that claim is now corrected
  in place, and so is the stale null-receiver note in
  [unresolved-java.md](unresolved-java.md).
* The engine **is** told there is no touchscreen — `isTouchDevice=false`, read
  twice — and behaves as a mobile client anyway. So the mobile input scheme is
  not the touch flag either. Whatever tells this engine "there is a keyboard and
  a mouse here", it is neither of the two fields named for it nor the one that is
  actually read.

That is what makes `getPlatformName` the remaining candidate rather than a
speculative one: it is the only platform-shaped value in this surface that the
engine both asks for and Cordial can choose.

### 2f. Things that look like the platform decision and are not

Two of the most visible "Roblox thinks you're mobile" symptoms are properties of
the APK's *content*, and no platform string can move them:

```text
[FLog::CreatorOutput] Info: DataModel Loading rbxasset://places/Mobile.rbxl
[FLog::Graphics] Loaded 3022 shaders from pack vulkan_mobile variant default
```

`rbxasset://places/Mobile.rbxl` is the only app-shell place the APK ships
(`Maquettes.rbxl` is the other and is not an app shell), and
`shaders_vulkan_mobile.pack` and `shaders_glsles3.pack` are the only shader packs
in it. The Android build has no desktop app shell to load and no desktop shader
pack to pick. **Cordial's app shell will look like the mobile app shell whatever
it reports as its platform**, because it is the mobile app shell.

`NativeSettingsInterface.nativeOverrideChannelPlatformName(String)` is a real
export and Roblox's own settings document carries
`AndroidOverrideChannelPlatformName = true`, so the real app does call it.
**`INFERRED`**, from the `AndroidApp` literal sitting in the same string block as
`buildVariant` and `viewportHeightMm`: what it takes is the *client-settings
application name*, the `AndroidApp` in
`https://clientsettingscdn.roblox.com/v2/settings/application/AndroidApp` — and
`crates/cordial-runtime/src/client_settings.rs` records that the plausible
alternatives all return HTTP 400 from that endpoint. On that reading it is not
the device platform and pointing it at `Linux` would break the flag fetch, which
is why it was left alone rather than tested.

## 3. What was measured

All runs: same binary, `--host-libc --game-activity`, nobody signed in, cold
(`data/` wiped before each), against a data root of their own. The control is
`CORDIAL_PLATFORM_NAME=Android`, which is the pre-change client in the same
session differing in exactly this string.

### 3a. The engine's own log does not narrate it

A whole-log diff of an `Android` run against a `Linux` run, timestamps, pointers
and durations normalised away, differs in one line: the session id inside an HTTP
error body. 941 lines each, same channels, same stages, same `setStage`
sequence.

A run with `CORDIAL_PLATFORM_NAME=ZQPLATMARKER` puts that marker **nowhere** in
the engine's log or anywhere else under the profile. Whatever the engine does
with this string, it does not print it. So the log is not a usable signal here,
and "the field is set" cannot be verified from it.

### 3b. One behavioural difference appeared and then did not reproduce

The engine writes `appData/LocalStorage/appStorage.json` itself. Two keys appear
there under `Linux` and never under `Android`:

```json
"SessionWithKeyboardAndMouse": "True",
"GamepadInteractionMoreRecentThanKeyboardMouse": "false"
```

Both names are in libroblox.so's string table, in the `UserInputService` /
`FLog::UserInputProfile` cluster (`BindButton`, `GamepadUsage_%s`,
`UserInputService::processGestures`, `WheelEventUsageTelemetry`), not in
`Mobile.rbxl`. They are the engine's own record of what kind of input the session
had. Every other key in the file was byte-identical between platforms except the
per-install `AppInstallationId`.

The first four pairs looked decisive. Twenty-one further runs did not bear it
out:

| Group | `Android` runs, keys present | `Linux` runs, keys present |
|---|---|---|
| first, 40 s | 0 of 3 | **3 of 4** |
| second, 60 s | 0 of 4 | 0 of 4 |
| third, 40 s | 0 of 5 | 0 of 5 |
| **total** | **0 of 12** | **3 of 13** |

(One further `Linux` run segfaulted during teardown and is excluded; so did one
`Android` run, so that failure is not the change's either.)

**This is not a result and must not be read as one.** Three occurrences, all on
one arm, is what a rare event that happens to land on one side of a small sample
looks like; 3-of-13 against 0-of-12 is not distinguishable from chance. The
asymmetry is real in the data and completely unexplained, and the honest summary
is that **no verified behavioural difference was found** — the change is
`INFERRED` in its effect and established only in the sense that the engine reads
the string.

Two things worth recording for whoever picks this up. No pointer or keyboard
input was delivered deliberately on any run: Cordial's X event mask takes
`PointerMotionMask` but not `EnterWindowMask`, so a resting cursor generates
nothing. But **a second agent was launching the same client on the same display
during part of this window** (observed as a concurrent `cordial-run` against a
different `--lib-dir`), which is an uncontrolled confound over focus and pointer
that could not be removed after the fact. A rerun on a quiet machine is cheap and
would settle whether the three are real.

## 4. What would settle it

Everything above stops at the landing page, because a signed-out client never
enters an experience and this pass could not sign one in or click anything.
The claim that actually matters — that experiences see a desktop client — lives
in Lua and needs one command inside a running experience:

```lua
print(game:GetService("UserInputService"):GetPlatform())
print(game:GetService("UserInputService").TouchEnabled,
      game:GetService("UserInputService").KeyboardEnabled,
      game:GetService("UserInputService").MouseEnabled)
print(game:GetService("GuiService"):IsTenFootInterface())
```

Run it under `CORDIAL_PLATFORM_NAME=Linux` and again under
`CORDIAL_PLATFORM_NAME=Android`, in the same session, same experience. If
`GetPlatform()` moves, this is the mechanism. If it does not, the change is still
the truthful answer and the mechanism is still open — and this file should be
corrected to say so.
