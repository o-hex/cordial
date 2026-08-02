# The WebView surface — what the engine actually asks the platform for

Marketplace, Profile, Friends, Communities, Create, Blog, Learn, gift cards, Help
& Safety and most link-opening in Roblox's Android client are web content rather
than engine-rendered UI. None of it works under Cordial. This is the map of what
would have to exist for it to.

**The headline is a correction.** The obvious plan — implement
`android.webkit.WebView` and its client classes in the framework layer, backed by
WebKitGTK — is aimed at the wrong boundary. The engine never calls
`android.webkit` at all. What it calls is a much smaller Roblox-owned surface,
and that is what this document inventories.

## How this was established

Four sources, in descending order of authority.

A live run of the current build, today, reaching the landing page:

```bash
CORDIAL_LOG_LEVEL=v CORDIAL_STUB_QUIET=1 ./target/release/cordial-run \
    --lib-dir ~/.cache/cordial-trace/lib/x86_64 --apk <base.apk> \
    --host-libc --game-activity --dump-classes classes.cpp --run 45
```

`just client --run 30` does the same thing and finds the APK itself.

The binary was `cordial-load` until 312451e renamed it to `cordial-run`, and a
stale `target/release/cordial-load` survives the rename and still executes. Two
runs of this investigation were spent measuring code that was never in the
binary under test. `native/CMakeLists.txt`'s build script warns about the
neighbouring version of this trap; the check that settles it is
`grep -ac '<a string you just added>' target/release/cordial-run`.

`--dump-classes` is `libjnivm`'s `GenerateClassDump`: every class and member
Roblox reached for through JNI during that run, whether or not Cordial answered
it. Note that "Constructed Unresolved symbol" lines do **not** appear at runtime
in an ordinary build — the `LOG` call sites in `libjnivm` are behind a
compile-time `JNI_TRACE`, which `native/CMakeLists.txt` leaves off. The class
dump carries the same information without a rebuild and is what was used here.

The engine's own log from the project owner's signed-in session,
`~/.local/share/cordial/instances/default/data/files/appData/logs/*_Player_*.log`,
which reached `APP_READY (Home)`.

`docs/traces/waydroid-roblox-startup.log.gz`, the logcat capture of the same APK
on real Android. It is startup-only and stops at the landing page, so it says
nothing about opening a web view. It does carry two things that matter: the
`EnableAndroidWebViewService4` flag value, and the cookie handler firing.

Declared prototypes read out of the shipping dex's `method_ids`/`proto_ids`
tables, and exported symbol names read out of `libroblox.so`'s dynamic symbol
table. Both are observation of a shipped binary, which AGENTS.md permits
explicitly. No decompilation was read.

## 1. The engine does not use `android.webkit`

Verified two ways, and worth stating plainly because it changes the design.

`libroblox.so` contains eleven `android/…` class descriptors as strings:
`android/app/ActivityThread`, `android/content/Context`,
`android/content/res/Configuration`, `android/os/Build`, `android/os/Build$VERSION`,
`android/os/Debug`, `android/os/LocaleList`, `android/view/KeyEvent`,
`android/view/MotionEvent`, plus two prefixes. None is `android/webkit`.

The class dump from the run above lists **64** classes Roblox reached for. None
is under `android/webkit` either, and none matches `cookie`, `credential`,
`token`, `keystore`, `sharedpref` or `account` in any case.

**64, not 2501.** That number is circulating and it is the generated file's line
count, not a class count — the dump is C++ source, and each class costs roughly
nine lines of namespace declaration, class body and hook registration. Measured
across three runs of the same build: 2501 lines / 64 classes, 2510 lines / 65
classes, 2501 lines / 64 classes. The two move together, nine lines to the
class. Count with

```bash
grep -cE '^class jnivm::' classes.cpp
```

The one class that varied between those runs was
`com/roblox/engine/jni/NativeInputInterface`, so the set is not quite fixed
run to run even at the same point in startup.

The distinction matters for sizing this work: 2501 makes the Java surface look
like a rewrite of Android, and 64 is what it actually is at this point in
startup. The conclusion drawn from the larger number — that no cookie or
credential class is requested during bring-up — is unaffected and holds on
this dump too.

`android/webkit/WebView`, `WebViewClient` and friends *do* appear in
`docs/analysis/framework-classes.txt`, and that is not a contradiction: that file
is the dex's referenced-type table, meaning the APK's **Java** code uses them.
Roblox's Java code is what drives a `WebView`, and Cordial does not run Roblox's
Java code — it stands in for it. So `WebView`, `WebViewClient`,
`WebChromeClient`, `WebSettings`, `CookieManager` and `ValueCallback` are not on
Cordial's implementation list at all. The distinction is the one
`observed-java-surface.md` opens with: which APIs the app references versus which
ones the engine reaches for.

What Cordial has to implement instead is the Roblox-owned interface that sits
between the engine and that Java code — the same relationship as
`NativeGLJavaInterface`, which Cordial already implements most of.

## 2. Two transports exist; the older one is the live one

**The legacy transport** is static methods on
`com.roblox.engine.jni.NativeGLJavaInterface`, called from the engine's
`DataModelBindings`. `native/android_classes.cpp` already implements part of this
class.

**The newer transport** is the universal-app message bus:
`com.roblox.protocols.webview.WebViewProtocol` subscribes to protocol messages
and the engine publishes open/close/mutate requests as JSON. Its Java side is
almost entirely a shell over native methods `libroblox.so` exports —
`Java_com_roblox_protocols_webview_WebViewProtocol_initializeAndroidWebViewProtocol`,
`…_getProtocolName`, `…_getOpenWindowId`, and so on — so a host that wanted to
speak it would call those exports itself rather than reimplement them.

The waydroid capture settles which is live:

```
07-31 22:50:57.420 D rbx.JNIRobloxSettings: nativeInitializeNativeFlags: ... 8: EnableAndroidWebViewService4 = false
```

The message-bus path is off. `EnableWebViewTelemetry`, `AndroidWebViewFocusFix`
and `EnableWebViewCameraPermission` are all true in the same capture, which is
what you would expect of a shipping legacy path with telemetry around it. So the
legacy `NativeGLJavaInterface` route is the one to implement, and the message-bus
protocol is where this will move when Roblox flips that flag — not something to
build first.

That flag value is a server-controlled rollout, not a constant. Re-read it from a
fresh capture before relying on it.

## 3. The call inventory

Signatures are the dex's own declarations. "Answered" is what
`native/android_classes.cpp` does today.

### 3a. Engine to platform, on `com/roblox/engine/jni/NativeGLJavaInterface`

| Method | Signature | Answered today |
|---|---|---|
| `openNativeOverlay` | `(Ljava/lang/String;Ljava/lang/String;)V` | hooked, silently discarded |
| `onAppBridgeNotification` | `(Ljava/lang/String;Ljava/lang/String;)V` | unresolved |
| `onDataModelNotificationCallback` | `(Ljava/lang/String;Ljava/lang/String;)V` | unresolved |
| `getWebViewUserAgent` | `()V` | unresolved |

`openNativeOverlay` is the direct one: url and title, and it corresponds to the
engine's `BrowserService::openNativeOverlay`, whose own diagnostic strings say it
refuses non-Roblox urls. `libroblox.so` also carries
`[FLog::DataModelBindings] openWebView_: Url:{}, Title:{}.`, which is the same
two arguments on the C++ side of the same boundary.

`getWebViewUserAgent()V` returns void because it is a *request*, not a getter.
The platform is expected to answer asynchronously by calling back into the engine
through `com.roblox.engine.jni.NativeGLInterface.setWebviewUserAgent(Ljava/lang/String;)V`,
whose implementation `libroblox.so` exports as
`Java_com_roblox_engine_jni_NativeGLInterface_setWebviewUserAgent`. Answering it
truthfully means having a web view whose user agent you can ask for; until then
the honest state is unanswered, and the engine falls back to whatever
`InitParams.userAgent` gave it.

`onAppBridgeNotification` and `onDataModelNotificationCallback` are both
`(String type, String data)`. The owner's signed-in session shows the C++ side of
the second one firing with exactly the payloads that drive app navigation:

```
onDataModelNotification: Received type(APP_READY, 10), data(Home)
onDataModelNotification: Received type(APP_READY, 10), data(More)
onDataModelNotification: Received type(PURCHASE_ROBUX, 8), data({"animated":true})
```

and the waydroid capture shows the Java side receiving the same thing on real
Android (`rbx.datamodel: onDataModelNotification() type:APP_READY data:Landing`),
which is what establishes that this callback is where those events cross the
boundary rather than dying inside the engine.

`APP_READY` is not itself a web-view request. `PURCHASE_ROBUX` is closer to one:
Buy Robux is web content on Android. The precise mapping from notification type
to "open this url" lives in Roblox's Java and was not read.

### 3b. Adjacent, unresolved, and on the same class

These are pre-cached by the engine at `JNI_OnLoad` and were unresolved in the run
above. None is WebView, but they sit in the same table and the same argument
applies to each — resolving matters for its own sake.

| Method | Signature | Honest completion |
|---|---|---|
| `gameLoadedCallback` | `(J)V` | no reply expected; a no-op is honest |
| `saveImageToAlbum` | `(Ljava/lang/String;)V` | reply failure via `NativeGLInterface.nativeImageSavedToAlbumFinished(Ljava/lang/String;ZLjava/lang/String;)V` |
| `getMobileAdvertisingId` | `()V` | reply path is `NativeGLInterface.setMobileAdvertisingId(Ljava/lang/String;)V`; Cordial has no advertising id and must not invent one |
| `onExtendedAnalyticsRecvCallback` | `([BI)V` | no reply expected |
| `onVrSessionStateUpdate` | `(I)V` | no reply expected |
| `promptNativePurchase` | `(JLjava/lang/String;)V` | Play Billing does not exist here |
| `promptNativePurchaseWithPaymentSessionId` | `(JLjava/lang/String;Ljava/lang/String;)V` and a 3-string overload | as above |

### 3c. The message-bus route, for when the flag flips

Exports on `libroblox.so`, all called *from* Java on Android, so a host speaking
this protocol calls them directly:

```
Java_com_roblox_protocols_webview_WebViewProtocol_initializeAndroidWebViewProtocol
Java_com_roblox_protocols_webview_WebViewProtocol_signalJavascriptCallback
Java_com_roblox_protocols_webview_WebViewProtocol_getProtocolName
Java_com_roblox_protocols_webview_WebViewProtocol_get{Open,Close,Mutate}WindowId
Java_com_roblox_protocols_webview_WebViewProtocol_getIsAvailableId
Java_com_roblox_protocols_webview_WebViewProtocol_getHandleWindowCloseId
Java_com_roblox_protocols_webview_WebViewProtocol_get{Url,Title,WindowType,IsVisible,
    HideHeader,BackButtonVisible,ShowDomainAsTitle,SearchType,SearchParams,Available}Key
Java_com_roblox_protocols_webview_DomainAllowListChecker_{checkDomainAllowList,
    enableDomainAllowListChecker,isKnownTrustedDomain}
```

The window is described by a JSON object whose keys those `get…Key` calls name.
`libroblox.so`'s `FLog::WebViewConfigLogs` strings confirm the shape without any
guessing: url (required), title, animated, visible, windowType, searchType,
searchParams. Delivery is `Java_com_roblox_universalapp_messagebus_MessageBus_*`
— `doSubscribeProtocolMethodRequestRaw`, `publishProtocolMethodResponseRaw`,
`subscribe`, `setRequestHandlerRaw` — returning a
`com.roblox.universalapp.messagebus.Connection`, which is the one class of this
group Cordial's run already reaches (it is a return type, so the engine caches
its method ids up front).

`DomainAllowListChecker` is worth noting for the security shape: the engine, not
the host, decides whether a url may be opened, and it exposes that decision to
the platform rather than trusting it. A Cordial implementation should ask it and
honour the answer, not open whatever it is handed.

## 4. Cookies — the part that carries the session

A web view that opens `roblox.com` and is not signed in is useless, so the
platform's cookie jar and the engine's have to be shared. On Android that is
`android.webkit.CookieManager`; on the engine side it is:

| Export | Declared as |
|---|---|
| `Java_com_roblox_universalapp_cookie_JNICookieManager_getCookie` | — |
| `…_JNICookieManager_setCookie` | — |
| `…_JNICookieManager_setCookiesFromDisk` | — |
| `…_JNICookieManager_convertCookiesToNetscape` | — |
| `…_JNICookieProtocol_updateOnSetCookieHandler` | `updateOnSetCookieHandler(Lcom/roblox/universalapp/cookie/JNICookieProtocol$OnSetCookieHandler;)V` |
| `Java_com_roblox_engine_jni_NativeSettingsInterface_nativeGetCookiesForDomain` | `(Ljava/lang/String;)Ljava/lang/String;` |
| `…_nativeGetCookiesInNetscapeFormat` | `(Ljava/lang/String;)Ljava/lang/String;` |
| `…_nativeSetMultipleCookies` | `(Ljava/lang/String;Ljava/lang/String;)V` |

The push direction is `CookieProtocol$OnSetCookieHandlerImpl.onSetCookie([Ljava/lang/String;Ljava/lang/String;)V`
— an array of `Set-Cookie` headers and the url they came from — and the waydroid
capture shows it firing during ordinary startup:

```
I OnSetCookieHandlerImpl: Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
```

**This is the authentication cookie.** `.ROBLOSECURITY` flows across this
boundary, and so does `RBXEventTrackerV2` (`libroblox.so` carries
`BrowserTrackerIdRequest: No RBXEventTrackerV2 in cookie.`). Two consequences
that are not optional:

Nothing on this path may be logged with its value. Cordial's own diagnostics
must print at most a name and a count. The same applies to urls crossing
`openNativeOverlay`, which can carry a one-time auth ticket in a query string —
log origin and path, never the query.

WebKitGTK's cookie jar must be non-persistent, or persisted no less carefully
than the engine's own. `WebKitWebsiteDataManager` defaults to ephemeral when
constructed without a base directory, which is the right default here.

## 5. What WebKitGTK answers directly, and what it does not

Assessed against the WebKitGTK 6.0 API. Marked **INFERRED** throughout: none of
this has been run, because the development headers are not installed (§7).

| Needed | WebKitGTK 6.0 |
|---|---|
| Show a url in a window | `webkit_web_view_new` + `webkit_web_view_load_uri`. Direct. |
| Title for the header bar | `webkit:title` property, `notify::title`. Direct. |
| Report a closed window back to the engine | GTK window close signal. Direct. |
| Seed the session cookie | `webkit_website_data_manager_get_cookie_manager` + `webkit_cookie_manager_add_cookie`, fed from `nativeGetCookiesForDomain`. Direct. |
| Receive cookies the web view sets | no add-cookie signal; requires `WebKitWebsiteDataManager` inspection or a `SoupSession` feature. Awkward. |
| Run JavaScript in the view (`BrowserService.ExecuteJavaScript`) | `webkit_web_view_evaluate_javascript`. Direct. |
| Page-to-host messages (`BrowserService.SendCommand`, and the `bs:open` / `bs:execJs` / `lp:openUrl` message types in `DFLog::StratusWebView`) | `webkit_user_content_manager_register_script_message_handler`. Direct. |
| A user agent matching what Roblox expects | `webkit:settings user-agent`, settable. What value it should be is an open question, not an API one. |
| Sit inside the engine's window | it cannot. See below. |

The last row is the architectural constraint and it is worth being explicit
about. [ADR-011](../adr/ADR-011-wayland-and-libadwaita.md) makes the engine's
`wl_surface` a `wl_subsurface` of a GTK toplevel, in one process, because a
subsurface cannot parent across connections. A `WebKitWebView` is a GTK widget in
that same toplevel's widget tree, which means the natural implementation is a
widget in the content slot *beside* the engine's canvas — not a second process.

That differs from Sober, which ships `sober_services` as a separate binary
against libwebkitgtk-6.0/libgtk-4/libadwaita-1/libsoup-3/libjavascriptcoregtk-6.0
and keeps its engine process toolkit-free. Both arrangements work; they are not
interchangeable. Sober's buys crash isolation for the browser and pays for it
with a separate window, and it is available to Sober precisely because its engine
process has no GTK in it. Cordial's engine process already links GTK4 and
libadwaita — ADR-011 §"One connection, therefore one process" — so an in-window
web view is available to Cordial and is the better answer to
"the communities view opens as a separate window", which
[ADR-001](../adr/ADR-001-in-process-hooking.md) lists as a motivating gap.

A separate process becomes the right answer if WebKit's own crashes start taking
the engine down with them. That is a measurement, not a prediction, and nothing
here has measured it.

## 6. Verified, inferred, not established

**Verified, this session:**

- No `android/webkit` class descriptor in `libroblox.so`, and no `android/webkit`
  class in a full startup-to-landing `--dump-classes` capture.
- The 64 classes the engine reached for in that run — and that `--dump-classes`
  output is 2501 *lines*, which is where the figure of 2501 classes came from.
- Which members of `NativeGLJavaInterface` Cordial answers versus leaves
  unresolved. `openNativeOverlay`, `onAppBridgeNotification`,
  `onDataModelNotificationCallback` and `getWebViewUserAgent` are all reached
  for; before this change the first was hooked to an empty body and the other
  three were unresolved.
- `EnableAndroidWebViewService4 = false` in the waydroid capture.
- The dex-declared signatures in §3 and §4, and the exported native names in §3c
  and §4.
- `webkitgtk-6.0.pc` is absent; `gtk4` 4.22.4 and `libadwaita-1` 1.9.1 are present.
- The four hooks in §8 register, and `onDataModelNotificationCallback` fires
  three times per run with `APP_READY` carrying `PlatformAccountRouter`,
  `Startup` and `Landing` — identical on three consecutive 40-second runs. The
  control is the same instrument on the previous binary, where all three
  appeared unhooked in the class dump and printed nothing;
  `gameLoadedCallback` is still unhooked and still shows the unhooked shape in
  the same dump, which is what makes that reading a measurement rather than an
  assumption.

**Inferred, not proven:**

- That `openNativeOverlay(url, title)` is the call that opens Marketplace,
  Profile and the rest. It is the only engine-to-platform call on the live
  transport that carries a url, and the engine logs `openWebView_: Url, Title`
  on its own side of the boundary — but nobody has watched it fire, because
  doing so needs a click in a signed-in client (§6, below).
- That the legacy transport is the live one for *this* build. The flag value
  comes from a 2026-07-31 capture on real Android; Cordial's own run is not
  signed in and never gets far enough to exercise either path.
- Everything in §5, which is API reading rather than running.

**Not established, and the honest reason:**

- **Nobody has observed a web-view request under Cordial.** Reproducing one needs
  a signed-in client and a click on Marketplace or Buy Robux. Cordial does not
  persist a session across runs — a fresh run stops at `APP_READY (Landing)` — so
  reaching the signed-in app means a human typing credentials, and driving the
  click is not something an agent may do here (AGENTS.md's caution on synthesised
  input). The `openNativeOverlay` stub in `native/android_classes.cpp` now reports
  when it fires, so the next person to run a signed-in session and click one of
  those entries will see it without instrumenting anything.
- Which notification types on `onDataModelNotificationCallback` correspond to
  which web destinations. That mapping is in Roblox's Java, which was not read.
- What user agent Roblox's web endpoints expect, and whether they behave
  differently for one that does not look like Android's WebView.
- Whether `DomainAllowListChecker` must be initialised before
  `openNativeOverlay` will fire at all.

## 7. Build dependency

Stage 3 of this work — an actual `WebKitWebView` — is **blocked on a package that
is not installed**:

```
$ pkg-config --modversion webkitgtk-6.0
Package webkitgtk-6.0 was not found in the pkg-config search path.
$ pkg-config --modversion gtk4 libadwaita-1
4.22.4
1.9.1
```

The runtime libraries are present (`libwebkitgtk-6.0.so.4`,
`libjavascriptcoregtk-6.0.so.1`, `libsoup-3.0.so.0` all resolve via `ldconfig`,
from `webkitgtk6.0-2.52.5`), so only the development package is missing. On
Fedora that is `webkitgtk6.0-devel`. Installing it is the maintainer's call and
was not done here; `CONTRIBUTING.md` records it beside the other optional
dependencies.

## 8. What exists in the tree now

Four hooks on `NativeGLJavaInterface` in `native/android_classes.cpp`, and
nothing else. They report; none of them acts.

`openNativeOverlay` was already hooked, with an empty body. It now prints the
request. That is the whole of the change to it and it is the point of the
change: a request to show a web page used to vanish leaving no trace anywhere,
so "Marketplace does nothing" and "Marketplace was never asked for" produced
byte-identical output. `onAppBridgeNotification`,
`onDataModelNotificationCallback` and `getWebViewUserAgent` were unresolved and
are now hooked, which matters beyond the reporting: an unresolved JNI call
leaves a pending exception, and the next JNI call on that thread trips over it
somewhere unrelated.

`getWebViewUserAgent` is deliberately left unanswered rather than given a
plausible string, which is the one place in this change where AGENTS.md's rule
about stubs that lie has teeth. The truthful answer is the user agent of the web
view that will render the page; there is no web view, so any value here is a
claim about a browser that does not exist, and it would reach Roblox's servers
as a header describing something that never made the request.

Urls are printed with their query string elided. They can carry a single-use
authentication ticket, and the cookie path beside them carries `.ROBLOSECURITY`
(§4). A truncation rather than an elision would still print the front of a
token, which is why the helper cuts at the `?` rather than at a length.
