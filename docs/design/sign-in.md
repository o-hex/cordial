# Sign-in — what it would actually take

**Status: discovery only. Nothing here is implemented.** No credentials were
used, entered, or created while writing this. This document exists to turn
"there is no sign-in" from a known gap into a scoped, evidence-backed plan.

**Related:** `docs/design/instances-and-launch.md` (a related, independently
written and *not yet verified* theory about a `roblox://` ticket-launch path —
referenced in §4, not duplicated).

---

## How to read this document

Every claim below is tagged:

- **[verified: log]** — read directly out of `docs/traces/waydroid-roblox-startup.log.gz`,
  the capture of this exact APK running on real Android. This is the strongest
  evidence available in this project, per `docs/NEXT.md`'s standing rule.
- **[verified: dex]** — read directly out of the shipping APK's own dex
  declarations (class names, method names, signatures) using
  `dexsig.py`/`dexsig_cls.py`, or out of `libroblox.so`'s exported symbol table
  with `readelf`. This is the host app's own contract, not a guess about it.
- **[verified: cordial]** — read directly out of Cordial's current source.
- **[inferred]** — a plausible reading of the above that was not itself
  observed running. Flagged explicitly every time, with the reasoning shown so
  it can be checked rather than trusted.

The capture is of a **logged-out** cold start. Absence of some behaviour in it
(no captcha, no full login POST) is not evidence that behaviour doesn't exist
elsewhere in the app — only that this particular run didn't trigger it. Said
again inline wherever it matters.

---

## 1. What does the engine expect?

There are **two separate channels** the engine uses for identity, not one.
Cordial's docstring in `native/android_classes.cpp` only names the first.

### 1.1 `NativeUserJavaInterface` — queried on demand **[verified: cordial]**

`native/android_classes.cpp:225-265` already stubs this correctly for a
logged-out user: `getUserId` → 0, `getUsername`/`getDisplayName`/
`getAlternateName` → `""`, `getIsUnder13` → false, `getMembershipType` → 0,
`getHasRobloxSubscription` → false. The engine calls these when it needs to
know who's signed in (e.g. building a `rbxthumb://` avatar URL), and Cordial's
existing comment is correct that a fabricated non-zero id would be worse than
an honest zero.

### 1.2 `StartAppParams` — pushed once, at app-start time **[verified: cordial]**

`native/init_params.cpp:695-752` defines `com.roblox.engine.jni.autovalue.StartAppParams`,
the object handed to `nativeAppBridgeV2StartAppWithParams` when the engine's
app-shell actually starts. It **already has the fields**:

```cpp
std::shared_ptr<String> username;
jlong appUserId = 0;
jboolean isUnder13 = false;
jint membershipType = 0;
```

All four are currently hard-zeroed in `StartAppParams::Create()`
(`init_params.cpp:730-732`). This is a second, independent place identity has
to be threaded through — it is not automatically consistent with whatever
`NativeUserJavaInterface` answers, because it's a different call at a
different time. Any real implementation has to fill in both.

### 1.3 What would actually be different if these were real

**[verified: log]** The logged-out capture shows the exact symptom the task
description calls out:

```
1338: rbxthumb://type=AvatarHeadShot&id=&w=48&h=48&filters=circular  → "invalid format"
1375: rbxthumb://type=AvatarHeadShot&id=-1&w=48&h=48&filters=circular → "The requested Ids are invalid, of an invalid type or missing."
```

**This is an important correction to the premise in the task description.**
The exact string *"The requested Ids are invalid, of an invalid type or
missing"* is what the **real, logged-out Android client** produces too, for
`id=-1`. It is not a Cordial-specific defect — it is the correct, faithful
behaviour of a client with no signed-in user. Cordial's `getUserId() → 0`
already reproduces the *class* of failure the real client has in this state
(the real client's sentinel appears to be `-1`, not `0` — a one-line, low-risk
alignment worth making so failure messages match exactly, but not
behaviourally different).

**[verified: log]** Also from the capture, three lines up the stack from the
thumbnail failures:

```
1291: HTTP error url: https://apis.roblox.com/attribution/v1/events/post-authentication
1299: HttpResponse ... status:401 ... url: https://users.roblox.com/v1/users/authenticated/app-launch-info
1331: HTTP error body: {"errors":[{"code":9002,"subcode":0,"message":"Authentication token is missing"}]}
1337: HTTP error body: {"sessionId":"00000000-0000-0000-0000-000000000000","status":403,"message":"Unauthorized."}
```

So a real session changes things at the **network layer**, not primarily at
the `NativeUserJavaInterface`/`StartAppParams` layer. The engine calls
`/v1/users/authenticated/app-launch-info` and other `authenticated/*`
endpoints itself, over its own HTTP client, and those calls **succeed or fail
based on the cookie the engine's HTTP client is carrying** — not based on what
`getUserId()` returns. `getUserId()`/`StartAppParams.appUserId` look like they
mirror an already-established server-side session rather than establish one.
**[inferred]** This means the cookie (§2) is the actual source of truth, and
`NativeUserJavaInterface`/`StartAppParams` are secondary, presentation-layer
mirrors of it — filling them in without a real cookie would produce a client
that *displays* a fake identity while still getting 401s from every
`authenticated/*` endpoint, which is worse than the current honest state, not
better.

**[verified: log]** Two more facts from the same run that constrain the
picture:

- `ActivityNativeMain.onResume(): IsLoggedIn = false` and
  `navigateToMainScreen: IsLoggedIn = false` (lines 941, 961) — the real app
  has an explicit `IsLoggedIn` boolean it branches the UI on, checked before
  any of the native calls above run.
- `initializeLuaAppWithLoggedInUser` (line 1031) and `userDidLogin` (line
  1191, an `[FLog::SingleSurfaceApp]` event) both fire **unconditionally**,
  even in this logged-out run. **These names are misleading if read as proof
  of authentication** — they are generic app-shell lifecycle stages the client
  reaches regardless of session state, not evidence that a real login
  happened. Do not treat either as a signal to test against.

---

## 2. Where does the session actually live?

### 2.1 The exported contract **[verified: dex + readelf]**

`com.roblox.engine.jni.NativeSettingsInterface` (all confirmed as real
`Java_com_roblox_engine_jni_NativeSettingsInterface_*` exports in
`libroblox.so`, with signatures read from the dex):

```
nativeGetCookiesForDomain(String) → String
nativeGetCookiesInNetscapeFormat(String) → String
nativeSetMultipleCookies(String, String) → void
nativeSetUserId(String) → void            (not previously documented anywhere in this repo)
nativeIsLuaLoginEnabled() → boolean
```

These are natives **the engine implements and Java calls** — the same
direction as the `nativeSetFilesDirectory`/`nativeSetDeviceInfo` family
Cordial already drives from `native/init_params.cpp` and
`crates/cordial-runtime/src/bin/load.rs`. Nothing here is currently called by
Cordial; grep confirms zero references to any of these five names anywhere in
`native/` or `crates/` before this document.

### 2.2 The reverse direction — the engine pushes cookies out, too

**[verified: dex + readelf]** `com.roblox.universalapp.cookie.JNICookieProtocol.updateOnSetCookieHandler(OnSetCookieHandler)`
is also a real exported native (`Java_..._JNICookieProtocol_updateOnSetCookieHandler`).
The real app's implementation of that handler,
`CookieProtocol$OnSetCookieHandlerImpl.onSetCookie(String[], String)`, is a
**plain Java class** (not a native) — meaning the engine, on receiving a
`Set-Cookie` from its own HTTP client, calls back **into Java** through
whatever handler object was registered.

**[verified: log]** This is directly observed firing, twice, during the
logged-out run:

```
1222: Flushed WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
1223: OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
1355: Flushed WebViewCookieHandler with Cookies from URL https://apis.roblox.com/account-switcher/v1/getLoggedInUsersMetadata
1356: OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/account-switcher/v1/getLoggedInUsersMetadata
```

So the contract has **three** legs, not one, and all three are exercised even
with no user signed in (this traffic is device/tracking cookies, not auth
cookies — but the plumbing is identical for both):

| Direction | Mechanism | Verified how |
|---|---|---|
| Java **pulls** cookies out of the engine | `nativeGetCookiesForDomain`/`nativeGetCookiesInNetscapeFormat` | dex + readelf signatures; `InitHelper: updateCookiesFromEngine` log line (§2.3) |
| Engine **pushes** cookies out to Java | `JNICookieProtocol.updateOnSetCookieHandler` registers a callback; engine `CallMethod`s it | dex + readelf; log lines 1222-1225, 1355-1356 |
| Java **injects** cookies into the engine | `nativeSetMultipleCookies(domain, cookies)` | dex + readelf signature only — not observed firing in this (logged-out, no WebView-login) capture |

**[inferred]** The purpose of this three-way sync: the engine's own HTTP
client and Android's `WebView`/`CookieManager` are two separate cookie jars on
real Android, and the app keeps them consistent so that (a) a cookie the
engine's network layer receives (e.g. after login) is visible to any WebView
the app opens next (communities, captcha, help), and (b) a cookie a WebView
obtains (e.g. the user just typed a password into a login page) is fed back
into the engine so its own HTTP client starts authenticating too. This
explains why `nativeSetMultipleCookies` exists at all — it is the write path
for exactly that second case, and per the task's framing, is the one Cordial
would need to call to hand the engine a real session.

### 2.3 Timing — cookie sync starts before the app shell even exists

**[verified: log]** The earliest cookie-related line in the entire capture is
at `ActivitySplash`, before `ActivityNativeMain` is even created:

```
300: E InitHelper: [l0.S()-160]: updateCookiesFromEngine: Invalid cookie format: []
```

This fires right after crashpad initialization, well before
`nativeAppBridgeSetInitParams`. **[inferred]** The obfuscated caller
(`l0.S()`, logged under the `InitHelper` tag) is almost certainly calling one
of `nativeGetCookiesForDomain`/`nativeGetCookiesInNetscapeFormat` this early
and getting an empty result back (`[]`), which it logs as a format error
rather than treating as fatal — i.e. **calling these natives before the
engine has anything to say is expected and handled gracefully**, not a
precondition Cordial has to satisfy before calling them.

### 2.4 A loose end, flagged rather than resolved

**[verified: readelf, contradicted by dex]** `libroblox.so` also exports
`Java_com_roblox_universalapp_cookie_JNICookieManager_{getCookie,setCookie,setCookiesFromDisk,convertCookiesToNetscape}`
— a *different* class from `JNICookieProtocol`. Searching all three dex files'
raw strings for `JNICookieManager` (not just via `dexsig`, but plain `strings`)
returns **zero matches** — the class does not exist anywhere in this APK's
dex. **[inferred]** This reads as vestigial: an older/renamed native symbol
table entry with no Java-side caller left in this build (native libraries
aren't proguard-stripped the way dex is, so dead exported symbols are
unsurprising). Treat `JNICookieManager` as dead code, not a second live
contract — the three `NativeSettingsInterface` methods plus
`JNICookieProtocol` are the live path.

---

## 3. Is a WebView required?

**Short answer: partially yes, and the part that's required is bigger than a
single captcha screen — but the core username/password submission may not be
one of the WebView parts.** Both halves of that are evidenced below; this
refines rather than simply confirms `docs/findings.md` §5(d)/§3.6.

### 3.1 WebView is loaded unconditionally at process start

**[verified: log]** Line 235, well before `ActivityNativeMain` or any login
UI: `WebViewFactory: Loading com.google.android.webview version 150.0.7871.181`.
Android's system WebView is instantiated during ordinary app startup, not
lazily when a login button is pressed. This alone doesn't prove login *needs*
it, but it means "just don't implement WebView" is not free even before
touching sign-in — communities and captcha (below) already depend on it per
the existing framework inventory.

### 3.2 The captcha flow is a URL loaded in a WebView-hosted Activity

**[verified: dex]** `com.roblox.client.captcha.ActivityFunCaptcha` is a
declared Activity (confirmed previously in `framework-api-inventory.md` §3.5).
Its configuration classes resolve to a URL:

```
LoginCaptchaConfig.getUrl() → String
SignUpCaptchaConfig.getUrl() → String
CaptchaConfig.getUrl() → String            (the common base type)
```

A config object that is just a URL, handed to an Activity whose only declared
purpose is captcha, is consistent with loading a challenge page in a WebView
— nothing here contradicts the existing finding that captcha is WebView-based.
This document did not find the internal fragment's layout (obfuscated names
like `H0(LayoutInflater, ViewGroup, Bundle) → View` don't reveal what view
they inflate without decompiling, which is off-limits per this project's
rules) but the URL-based config plus the WebView's unconditional early load
(§3.1) is corroborating rather than new doubt.

### 3.3 But there's a flag for a non-WebView login path, unexplained in prior docs

**[verified: dex + readelf]** `NativeSettingsInterface.nativeIsLuaLoginEnabled() → boolean`
is a real native. **[inferred]** Roblox's app shell (the "Landing", "Startup"
etc. stages visible throughout the capture) is rendered by the Lua-based
`SingleSurfaceApp`/`UniversalApp`, the same layer that renders the rest of the
UI without a WebView. A flag named `IsLuaLoginEnabled` reads most naturally as
"is the login *form* (username/password fields) rendered by the Lua app shell
rather than a WebView", talking to `auth.roblox.com` (DNS for which is
pre-warmed at startup — line 103: `Pre-warmed DNS for auth.roblox.com`)
directly over HTTPS, which Cordial already supports end-to-end.

If that reading is right, **basic username/password login may not require a
WebView at all** — only the captcha challenge and any federated (Google/Apple)
sign-in option would. This is not verified by running anything (the captured
run never reached a login screen), so treat it as the single most valuable
thing to confirm early if this work is picked up: instrument
`nativeIsLuaLoginEnabled`'s return value and see whether the login form that
appears is native/Lua or a WebView.

### 3.4 The honest expectation regardless

**[inferred]** Roblox's captcha is risk-based and server-decided, not
something the client can predict or skip. A Linux host presenting as a
brand-new, unrecognized Android x86_64 device is close to the profile that
triggers step-up challenges most often. So even if password submission itself
turns out to be WebView-free, **a real user attempting to actually sign in
through Cordial should be expected to hit `ActivityFunCaptcha` on a first
attempt**, and a WebView implementation should be budgeted for, not treated as
optional-in-practice. This matches — and does not overturn —
`framework-api-inventory.md`'s conclusion that WebView "is also on the login
path... not optional."

---

## 4. Is there a path that avoids it?

Three non-web-form surfaces are declared in the APK. None of them turned out
to be a low-effort shortcut on inspection — stated plainly, since that's the
honest answer to the question asked.

### 4.1 Passkeys / `CredentialManager` — already scoped, still large

Covered in depth by `framework-api-inventory.md` §3.6 already: the platform
`android.credentials.CredentialManager` path is real and avoids GMS, but
still means implementing the platform credential-provider contract and
bridging to `libfido2`/`xdg-desktop-portal` — not smaller than WebView, just a
different large piece of work. Nothing new found here; not re-litigated.

### 4.2 "Magic Login" — local-network device pairing, genuinely non-web, unverified in operation

**[verified: dex strings]** Distinct from `JNIAccountProtocol`'s
`getMagicLoginActionKey`/`getMagicLoginMethodName` (small JS-bridge-style
constant getters, same pattern as `FlagJniInterface`), the dex contains a
cluster of unrelated-looking class names:

```
MagicLoginManager
MagicLoginNsdHelperClient / MagicLoginNsdHelperServer
MagicLoginSocketHelperClient / MagicLoginSocketHelperServer
MagicLoginSocketTimeout
Roblox-Magic-Login          (an HTTP header name)
```

**[inferred]** `Nsd` is Android's Network Service Discovery (mDNS) API. This
shape — an NSD advertise/discover pair plus a raw socket helper plus an HTTP
header — reads as a **"sign in using another device that's already logged
in"** flow: an already-authenticated device (e.g. the user's phone) advertises
on the LAN, a new device discovers it and exchanges a session over the local
socket, likely with a server-mediated confirmation (the HTTP header suggests
a request is also sent to Roblox's servers as part of the handshake). This is
the shape of the "sign in with your phone" flow real Roblox ships on
console/TV-class clients.

**This would let Cordial obtain a real session without ever presenting a
password field or a WebView itself** — the credential entry happens on the
peer device, which the user already trusts. It is the most promising
non-WebView lead this investigation found.

**Caveat, stated as strongly as the evidence deserves:** this was found by
`strings`-grepping obfuscated class names in a dex, not by observing it run.
The logged-out capture never triggers this flow (there is no reason it would,
on a cold start), so nothing here confirms it actually works, what the wire
protocol is, or whether it's gated behind a flag/feature that's off in this
build. It needs its own investigation — likely by triggering it on a real
device and capturing the traffic — before it can be relied on for a design.

### 4.3 The `roblox://` launch-ticket path — a different document's open question, not resolved here

`docs/design/instances-and-launch.md` §2 already describes (and explicitly
flags as **unverified**) a theory that pressing "Play" on roblox.com in an
external browser emits a `roblox://`-scheme URI carrying a short-lived
authentication ticket, and that `ActivityProtocolLaunch` consuming it is what
lets multiple Cordial instances each be signed in as a different account
without Cordial ever touching credentials.

**[verified: dex strings]** What this document found narrows that: the
`roblox://` URIs actually declared in the dex are all **game-join** shaped —
`roblox://placeId=%d&reservedServerAccessCode=%s&callId=%s`,
`roblox://experiences/start`, `roblox://placeId=%d&gameInstanceId=%s&callId=%s`
— not a bare authentication URI. `ActivityProtocolLaunch`'s own methods
(`i2(String) → boolean`, `j2() → void`) give no further signature detail
without decompiling.

**[inferred]** This is more consistent with the classic Roblox desktop-client
"join ticket" — scoped to launching into one specific place/server, minted by
a browser session that is *already* fully authenticated — than with a
general-purpose account sign-in mechanism. It may still be genuinely useful
(a join ticket plausibly carries enough for the client to also treat itself as
signed in as that account for the session, which is `instances-and-launch.md`'s
working theory), but this document did not find evidence settling whether a
join ticket alone produces a durable, reusable `.ROBLOSECURITY`-equivalent
cookie, or only an ephemeral per-join credential. **This is the same open
question `instances-and-launch.md` §8 already lists** ("verify the `roblox://`
URI format and whether tickets are genuinely single-use and session-scoped")
— this investigation corroborates that it's worth answering, without
answering it.

### 4.4 Verdict on §4

No path was found that is both (a) confirmed to work and (b) avoids either a
WebView or a second physical/logical device. If forced to rank by plausible
effort: §4.2 (magic login) is the cheapest **if** it turns out to work as
inferred, §4.3 (join ticket) is worth resolving because another design
already depends on it, and §4.1 (passkeys) is real but not small. None of
these should be assumed as the plan without running something first — every
one of them is currently a string in a dex, not an observed behaviour.

---

## 5. What is the minimum viable step, if a real session existed?

This section assumes a real `.ROBLOSECURITY`-style session cookie has been
obtained by some means outside Cordial (e.g. a real browser login performed
by the user, independent of Cordial itself — consistent with `instances-and-launch.md`
§7's "Cordial stores no credentials" stance) and asks what the smallest change
to Cordial would be to make the engine actually use it. It does **not**
propose building any login UI.

### 5.1 The existing call order, and where the new calls slot in

**[verified: cordial]** `crates/cordial-runtime/src/bin/load.rs` already
drives this exact sequence (line numbers as of this session):

```
NativeSettingsInterface.nativeSetFilesDirectory / nativeSetCacheDirectory /
  nativeSetExternalDirectory / nativeSetBaseDataDirectories        (~652-669)
MainGameActivity.nativeSetAssetPath                                 (~676)
NativeSettingsInterface.nativeSetRobloxVersion / nativeSetRobloxChannel (~681,690)
NativeSettingsInterface.nativeSetDeviceInfo                         (~699)
MainGameActivity.nativeAppBridgeSetInitParams                       (~766)
  ...
MainGameActivity.nativePreloadFlagOverrides                         (~957)
MainGameActivity.nativeRetryInit                                    (~1001)
NativeGLInterface.nativeAppBridgeV2InitWithParams                   (~1071)
NativeGLInterface.nativeAppBridgeStartLuaAppDM                      (~1082)
NativeGLInterface.nativeAppBridgeV2StartAppWithParams                (~1116)
```

### 5.2 The concrete proposal

1. **Call `NativeSettingsInterface.nativeSetMultipleCookies(domain, cookies)`
   once the real cookie is available**, before `nativeAppBridgeSetInitParams`
   (~766) — so the engine's own HTTP client is carrying the session before
   the app-bridge sequence that will immediately start hitting
   `authenticated/*` endpoints (§1.3) begins. `nativeSetUserId(String)`
   (§2.1, previously undocumented) should be called in the same neighborhood
   — its exact required timing relative to the cookie call is **not verified**
   and would need to be checked against behaviour, not assumed.
2. **Register a real `OnSetCookieHandler`** via
   `JNICookieProtocol.updateOnSetCookieHandler` (§2.2) so cookies the engine's
   own HTTP client subsequently receives (e.g. a session refresh) are
   observed rather than silently dropped — this is the callback direction
   Cordial has no equivalent of yet, structurally the same shape as the
   existing `NativeHelper::gameActivity_onFlagsFailed` callback pattern
   already used in `native/android_classes.cpp`.
3. **Fill in `StartAppParams`** (`init_params.cpp:730-732`) — `appUserId`,
   `username`, `isUnder13`, `membershipType` — from the same real account,
   right before `nativeAppBridgeV2StartAppWithParams` (~1116).
4. **Fill in `NativeUserJavaInterface`** (`android_classes.cpp:227-250`) —
   `getUserId`, `getUsername`, `getDisplayName`, `getMembershipType`,
   `getHasRobloxSubscription`, `getIsUnder13` — consistently with the same
   account, since the engine can call these at any later point and a mismatch
   with what `StartAppParams` said would be a self-contradicting client.
5. **Verify against the log, not by assumption**: rerun with
   `CORDIAL_MONITOR=1`, capture the engine's own FastLog
   (`$CORDIAL_FILES_DIR/files/appData/logs/*.log` per the project's standing
   diagnostic), and check that `authenticated/*` calls now return 200 instead
   of the 401/403 pattern in §1.3, and that the two `rbxthumb://` failures in
   §1.3 are replaced by a real avatar id.

Nothing in steps 1-4 requires new JNI plumbing beyond what already exists in
`native/init_params.cpp`'s `cordial_call_static_strings`/`cordial_call_bare`
helper family (§2.1's natives all fit the "static native, up to three string
args" shape those helpers already handle) plus one new callback registration
(step 2) shaped like the existing `NativeHelper` pattern. This is a
small, mechanical change **conditioned entirely on having a real cookie to
put in it** — which is the actual blocker, per §4.

---

## 6. Summary

| Question | Answer | Confidence |
|---|---|---|
| What does the engine expect differently when logged in? | Two identity mirrors (`NativeUserJavaInterface`, `StartAppParams`) plus, more importantly, a real cookie flowing through the engine's own HTTP client — the mirrors alone don't unblock `authenticated/*` calls. | Verified (log) for the HTTP-layer part; the mirrors' role is inferred but low-risk to fill in regardless. |
| Where does the session live? | Three-way cookie sync between the engine's HTTP client and Android's WebView `CookieManager`, via `NativeSettingsInterface`'s three cookie natives plus `JNICookieProtocol`'s callback. | Verified (dex, readelf, log) for the contract shape; `nativeSetMultipleCookies` itself was not observed firing. |
| Is a WebView required? | For captcha: yes, and expect it to fire in practice — even a native (`CaptchaNative`) captcha screen was found, so this stays unsettled but the WebView path still needs building regardless (§7.5). For plain username/password: **no** — `FIntLuaAppLoginMethod` (default `"1"`) makes this Lua-rendered by default, confirmed by calling the engine's own native directly and by a `LoginNative` screen with real username/password copy shipped in the app's own content (§7). | Captcha: verified-adjacent, refined but not overturned. Password path: **verified**, resolved from §3.3's "possibly not, unconfirmed." |
| Is there a path that avoids it? | No confirmed one. Local-device "magic login" is the most promising unverified lead; a join-ticket path already assumed by another design doc remains unresolved. | Inferred, both. |
| What's the minimum viable step? | Five mechanical calls into existing/adjacent JNI plumbing, entirely gated on obtaining a real cookie by some means outside Cordial. | Verified for the mechanics; the precondition (§4) is the real blocker. |

**The one thing this document changes about the project's understanding of
its own blocker:** it is not "Cordial has no auth." It is "Cordial has no way
to *obtain* a session, and every way of obtaining one that was found requires
either a WebView, a second device, or an unresolved ticket flow." The stub
code in `NativeUserJavaInterface` is not the blocker — it is honestly
reporting a true fact (nobody is signed in) and would need real data fed to
it regardless of which acquisition path gets built.

---

## 7. Resolving §3.3: does `nativeIsLuaLoginEnabled` mean login can skip a WebView?

§3.3 above flagged `NativeSettingsInterface.nativeIsLuaLoginEnabled()` as the
single most valuable open question, unresolved because the logged-out capture
never reaches a login screen. This section answers it by tracing the dex
bytecode that consumes the native's return value, isolating the exact
FastFlag that controls it, and running Cordial with and against that flag —
a real control, not a guess.

No account was created, no credentials were entered, and nothing here typed
into or submitted a login form.

### 7.1 What `nativeIsLuaLoginEnabled` actually gates **[verified: dex bytecode]**

The dex declares the native (as §2.1 already established), but nothing in
this project had previously traced who calls it or what branches on the
result. Disassembling the dex directly (via `androguard`, reading actual
instructions rather than inferring from names) settles both:

```
Luk/c; a ()Z
  invoke-static Luk/c;->b()Z        ; b() just calls the native and returns it
  move-result v0
  if-nez v0, +0bh                   ; if native said true -> return true
  invoke-static Lel/s;->i()Z        ; else check a second condition...
  move-result v0
  if-eqz v0, +003h
  goto +3h
  const/4 v0, 0
  return v0
  const/4 v0, 1
  return v0

Lel/s; i ()Z
  const/4 v0, 0
  return v0                         ; ...which is hard-coded to `false` in this build
```

So `uk.c.a()` — the actual gate everything else calls — reduces to exactly
`nativeIsLuaLoginEnabled()` in this shipping build; the second disjunct is
dead weight (a debug/staging override compiled out here).

`uk.c.a()` has exactly two callers in the whole APK, both `handleNotification()`-style
methods on Activity subclasses sharing a common base (`com/roblox/client/a`),
both handling the **logout event** (`code == 101`):

- `com.roblox.client.ActivityNativeMain.V(int, Bundle)`
- `com.roblox.client.RobloxWebActivity.V(int, Bundle)`

In both, when `uk.c.a()` is true, the handler additionally:

1. Sets a one-shot static flag (`tj.b.p()` → `Ltj/b;->e = true`), and
2. Spins up an `AsyncTask` (`ActivityNativeMain$l`) that checks whether the
   engine's rendering `Surface` is still alive and, if so, **reuses it**
   (`Vi/e.w`/`Vi/e.x`) instead of tearing it down.

That one-shot flag is consumed later, in `ActivityNativeMain.e0()` — the
handler for the engine's own `onAppStarted` callback — via `tj.b.e()` (a
read-and-clear getter), which re-triggers the same surface-reuse task once
the Lua app shell reports it has restarted.

**What this means:** `nativeIsLuaLoginEnabled` does not gate "render form A
vs form B" directly. It gates a **logout → re-entry rendering-continuity
optimization**: when true, the app keeps its OpenGL rendering surface warm
across a logout and hands it straight back to the restarting Lua app shell,
instead of tearing the surface down and rebuilding it. That behaviour only
makes sense if whatever the user lands on next — the post-logout
landing/login screen — is rendered inside that same Lua/GL surface rather
than by launching a separate WebView-hosting Activity. It is corroborating
evidence for the Lua-login reading, not the direct proof §3.3 was hoping for;
§7.4 below supplies the direct proof.

### 7.2 Which FastFlag controls it, isolated by bisection **[verified: cordial, live run]**

Searching the client-settings document (`clientsettings.orig.json`, a
previously-captured copy) for anything matching `*Login*` and `*Lua*Login*`
turns up ten plausible-looking candidates, none named exactly
`IsLuaLoginEnabled`:

```
FFlagEnableLuaLoginRevamp6 = True
FFlagEnableLuaLoginRevamp8 = True
FFlagLuaAppUsingSecurityQuestionsForLuaLogin2 = True
FFlagDisableAndroidLogInReleaseBuilds_IXP = "1;...;flagbank"   (an IXP bucket string, not a plain bool)
FIntLuaAppLoginMethod = "1"
FIntLuaAppLoginRollout = "100"
FIntLuaLoginGenderSelector = "1"
```

`strings` on `libroblox.so` finds only the exported symbol name itself
(`Java_com_roblox_engine_jni_NativeSettingsInterface_nativeIsLuaLoginEnabled`)
— no flag-name string literal near it, so which flag the native actually
reads could not be settled by inspection. It had to be settled by running it.

**Instrumentation added** (uncommitted, in this worktree only): a new
zero-argument `boolean`-returning JNI call helper —
`cordial_call_static_bare_bool` in `native/init_params.cpp`, wired through
`crates/cordial-linker-sys/src/lib.rs` as `call_static_bare_bool`, invoked
once in `crates/cordial-runtime/src/bin/load.rs` right after flags and
client settings have both been delivered to the engine. It calls
`NativeSettingsInterface.nativeIsLuaLoginEnabled()` directly and prints the
result — `[sign-in probe] nativeIsLuaLoginEnabled() -> <bool>` — nothing
else. This does not drive any UI and does not run any dex/Java bytecode
(Cordial doesn't execute the APK's dex at all — see the caveat in §7.3); it
only reads the engine's own boolean answer.

**A methodology note, for anyone re-running this:** the worktree this was
run from predated `crates/cordial-runtime/src/flags.rs` (the flags-to-client-settings
merge pipeline described in that file's own doc comment as "the mechanism
that demonstrably works," with its own independently-verified control using
`DFFlagRbxTransportUseRtcioRna`). That file, plus the four-line change to
`client_settings.rs` that calls it, existed already on `main` but not on this
branch, so a first attempt at "set a flag in `flags.json` and see if anything
changes" produced two identical `true` results — not because the flag has no
effect, but because the file was being silently ignored by a binary that had
no merge step at all. Once `flags.rs` and the matching `client_settings.rs`
hunk were carried over (uncommitted; both already exist verbatim on `main`,
this only makes this branch's binary match it) and `serde_json` added to
`cordial-runtime`'s `Cargo.toml`, the pipeline worked and gave the results
below. This is exactly the kind of gap the task's own warning about
disassembly-derived conclusions is trying to prevent — the fix was to notice
the run wasn't testing what it claimed to, not to trust the first "no
effect" result.

**Control, then bisection**, each a full `cordial-run --run 25` under
`CORDIAL_MONITOR=1`, reading `[sign-in probe]` from stdout:

| `~/.config/cordial/flags.json` | `nativeIsLuaLoginEnabled()` |
|---|---|
| absent (control) | **true** |
| all 10 candidates forced off/0 | **false** |
| the 5 real `FFlag*`/IXP candidates only | true (no change) |
| the other 5 (3 guessed-name flags + 2 `FInt*`) | **false** |
| just the 3 guessed-name flags (`FFlagIsLuaLoginEnabled`, `FFlagLuaLoginEnabled`, `DFFlagIsLuaLoginEnabled` — none of which exist in the real client-settings document, so these are pure no-ops that happen to get merged in as new, unread keys) | true (no change) |
| just `FIntLuaAppLoginMethod=0` + `FIntLuaAppLoginRollout=0` | **false** |
| just `FIntLuaAppLoginMethod=0` alone | **false** |
| just `FIntLuaAppLoginRollout=0` alone (Method left at its default `1`) | true (no change) |

**Isolated result: `FIntLuaAppLoginMethod` is the controlling flag.** Its
shipped default is `"1"`, which is why the honest, un-overridden control run
already returns `true` — **Lua login is the default in this build**, not
something that needs turning on. Setting it to `0` reproducibly flips
`nativeIsLuaLoginEnabled()` to `false`. `FIntLuaAppLoginRollout` (a
percentage-shaped name, default `"100"`) does not independently affect it —
consistent with it being a server-side experiment-allocation knob rather
than something the client itself branches on directly. None of
`FFlagEnableLuaLoginRevamp6`/`8`, `FFlagLuaAppUsingSecurityQuestionsForLuaLogin2`,
or `FFlagDisableAndroidLogInReleaseBuilds_IXP` — despite all sounding
relevant by name — moved this particular native at all; they likely gate
sub-features of the Lua login *experience* (a UI "revamp," a security-questions
step) rather than the native/WebView choice itself. Ruling those out by
running them, rather than assuming from the name, is itself a finding.

### 7.3 The honest limit of what "observe" could mean here **[verified: cordial]**

The task asked to turn the flag and *observe*, meaning: see whether the
rendered UI changes. It doesn't, not yet, and here is exactly why: **Cordial
does not execute the APK's dex bytecode.** Everything this section traced in
§7.1 — `uk.c.a()`, `ActivityNativeMain.V`, `RobloxWebActivity.V`, `tj.b` —
is Java code that runs on Android's ART VM. Cordial only loads
`libroblox.so` natively and drives its exported entry points directly from
Rust/C++, standing in for the Java caller by hand (exactly the pattern
`cordial_call_static_strings` and friends already use, and the one this
section's new `cordial_call_static_bare_bool` also uses). There is no ART,
so there is no code path that would ever call `uk.c.a()`, branch on it, or
show a login screen as a consequence — regardless of what the native
answers.

So what was actually verified is narrower than "the UI changes": it is that
**the engine's own native call, invoked directly and read directly, answers
`true` by default and can be flipped to `false` by a specific, isolated,
real FastFlag** — a fully mechanical fact about the shipped binary,
independent of any UI. Whether flipping it would visibly change a rendered
screen is not something Cordial can currently observe, because Cordial has
no login-launching UI of its own yet and no dex execution to drive the real
one. That gap is orthogonal to this question and unchanged by this
investigation.

### 7.4 Is there a Lua-rendered login UI in the shipped content? Yes — directly, not just inferred **[verified: rbxm strings]**

`assets/ExtraContent/models/UniversalApp/UniversalApp.rbxm` — part of the
Lua app shell's own content, extracted from the APK — is a Roblox binary
file (`<roblox!` header, zstd-compressed chunks). Decompressing its chunks
(a from-scratch reader was needed; no existing tool in the environment
handles this format, and `androguard`/binary-format libraries don't apply to
it) and running `strings` over the ~46 MB decompressed property chunk finds
a **complete, multi-language localization table for a full authentication
UI**, including, among many others:

```
Authentication.Login.Heading.Login
Authentication.Login.Label.Username
Authentication.Login.Label.Password
Authentication.Login.Label.Email
Authentication.Login.Label.UsernameEmailOrPhoneNumber
Authentication.Login.Label.UsernameEmailPhone
Authentication.Login.Action.Next
Authentication.Login.Action.LogInEmailOneTimeCode
Authentication.Login.Action.SendVerificationEmail
Authentication.Login.Description.PasskeyDescription
Authentication.Login.LinkIllegalChildAccountLinking   (mentions a QR code)
Authentication.CrossDevice.Label.LoginInstructions
Authentication.CrossDevice.Label.ConfirmAndLoginAs
Authentication.CrossDevice.Response.LoginSuccess
```

translated into at least French, Turkish, Italian, Polish, Spanish, and a
China-specific "Luobu"-branded variant. `Authentication.CrossDevice.*`
independently corroborates §4.2's "magic login" lead (an already-signed-in
device confirming a login for a new one) with real shipped copy, not just
suggestively-named dex classes.

More directly still, the same file contains what reads as the app shell's
own registry of named, navigable screens — a flat array of Pascal-style
strings, one screen name each:

```
...SinglePageSignUp MultiPageSignup LuobuSignUpPage Landing Birthday
LoginNative CaptchaNative ViewFriends SearchUsers ShareSheet SdkShare
ReportAbuse ReportScreen AddFriendsPage ConnectionsHub CoHubMyConnections
CoHubAddConnections ScanQrCode GenericOpaqueWebPage ChallengeHybridOverlayPage
ChallengeHybridWebView PhoneVerification EditUsername PassesPage ...
```

`LoginNative` is a real, first-class screen name the app shell's own
navigation system recognises — and it is the *only* login-shaped screen name
anywhere in this file; `LoginWeb`, `LoginWebView`, and `WebViewLogin` all
return zero matches. Screens that genuinely are WebView-hosted are named
accordingly right alongside it in the same list —
`GenericWebPage`/`GenericOpaqueWebPage`, `AddFriendsWebView`,
`ChallengeHybridWebView` — which is good corroboration that this naming
convention is real and consistently applied, not incidental.

This directly confirms the premise §3.3 could only infer: **the shipped Lua
app shell has its own native login screen, `LoginNative`, with full
username/password copy, and it is a named peer of screens that really are
WebView-hosted, distinguished by the same naming convention.** Combined with
§7.2 (Lua login is the shipped default) and §7.1 (the logout path's own
surface-continuity behaviour assumes the next screen is Lua-rendered), the
three independent lines of evidence now agree.

**Caveat:** finding the string `LoginNative` in a screen-name enum proves the
screen is defined and named, not that it is reachable, wired up, or free of
its own dependency on WebView-hosted sub-flows once inside it (e.g. a "log
in with Google" button on that screen could still open a WebView without
that changing the screen's own name or its username/password fields). No
navigation into any screen was attempted — that would require pressing UI
elements this task's constraints put out of scope for anything past
observation.

### 7.5 Does captcha still block it? Narrowed, not settled — and one part cuts against the convenient answer **[verified: rbxm strings]**

The same screen-name list contains `CaptchaNative` immediately next to
`LoginNative` — a native-rendered captcha screen, distinct from the
WebView-shaped `ChallengeHybridWebView`/`ChallengeHybridOverlayPage` entries
in the same list (which read as the natural landing spot for
`ActivityFunCaptcha`'s URL-based `LoginCaptchaConfig`/`SignUpCaptchaConfig`,
per §3.2). Both `LoginNative` and `CaptchaNative` occur exactly once in the
file — as enum entries only; no second reference to either was found, so
there is no evidence here of which triggers when, or that `CaptchaNative` is
actually wired to anything live.

**The part that cuts against the convenient reading:** the same file also
contains `Turnstile`/`turnstile` (Cloudflare's captcha widget) and
`CaptchaV2`/`captchav2` strings. Turnstile is, in every other context it
ships in, a web/JS widget — its presence alongside `CaptchaNative` reads
more like "this build supports more than one captcha backend, selected
server-side by risk assessment" than "captcha has moved off WebView." This
document already said (§3.4) that captcha is risk-based and server-decided,
not something the client predicts — that stands. `CaptchaNative`'s existence
is real and new, and worth someone confirming by actually reaching a captcha
challenge on a real account, but it does not license "a WebView is no longer
needed for captcha." **A WebView implementation should still be budgeted
for.**

### 7.6 What this changes

- **§3.3's open question is now answered, not just narrowed:** plain
  username/password login is Lua-rendered by default in this shipped build
  (`FIntLuaAppLoginMethod = "1"`), the engine's own native agrees when called
  directly, and the shipped Lua content itself independently confirms a
  `LoginNative` screen exists with full username/password copy. §3.3's
  "possibly not" becomes "verified: no, not for the base form."
- **What is still true and unchanged:** §3.2's WebView-based captcha finding
  stands; §7.5 adds a native captcha screen to the picture without replacing
  it. §4's verdict (no confirmed WebView-free *path to a session*, i.e. §2's
  actual blocker) is untouched — this section is about what renders the
  form, not about how Cordial would obtain a real cookie, which remains the
  real blocker per §4 and §6.
- **What is newly true and worth carrying forward:** if a login-driving UI
  is ever built in Cordial, the rendering-technology question that used to
  gate the whole plan (embed a browser, yes or no) has a real, verified
  answer for the base form: **no, not for username/password entry itself.**
  A WebView is still very likely needed somewhere in the full flow (captcha
  most likely, federated/passkey login certainly), so it is not removed as a
  dependency of the project — but it is no longer required to render the
  first, most common screen a user would see.
