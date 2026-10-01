//! `Game.launch` -> `nativeAppBridgeV2StartGameWithParam`: the Java half of a
//! join, for an engine brought up through the app bridge alone.
//!
//! Pressing Play on the Quest build showed a loading screen for half a second
//! and then nothing, with no Java call, no stop and no FLog line. The app
//! shell had asked for the launch and nobody answered. On Android the answer
//! is Java's: `ActivityNativeMain` subscribes to `Game.launch` on the message
//! bus before it starts the Lua app, and the payload travels through
//! `ExperienceSession` to `nativeAppBridgeV2StartGameWithParam` (the dex's
//! call graph, docs/vr/play-button.md). Sober's log of a join on the phone
//! build shows that native, then `[FLog::SingleSurfaceApp] launchUGCGame`.
//!
//! Measured on 2740.927 without the headset: a `roblox://` deep link makes the
//! app shell publish `Game.launch` naming the place, and with nothing
//! subscribed no `launchUGCGame` follows, in 90 seconds.
//!
//! **Only under `--app-bridge`.** On the phone build under AGDK a published
//! `Game.launch` joins with nothing subscribed (docs/analysis/app-bridge.md
//! §9, 8/8), so the engine answers it itself there, and a second answer from
//! here would be a second join.
//!
//! The payload is parsed by key, and a key is carried only when
//! `StartGameParams` has an accessor of the same name. Anything else is named
//! in the log and dropped, rather than mapped onto a field by guesswork.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::{Mutex, OnceLock};

use cordial_linker_sys::game_activity::StartGame;

/// `JNIExperienceProtocol.getLaunchId()`, read from both the phone build
/// (docs/analysis/deep-links.md §4) and the Quest build 2.740.927.
const GAME_LAUNCH: &str = "Game.launch";

unsafe extern "C" {
    fn cordial_messagebus_subscribe(
        f: *mut c_void,
        message_id: *const c_char,
        sink: Option<extern "C" fn(*const c_char)>,
        err: *mut c_char,
        n: usize,
    ) -> c_int;
    fn cordial_take_game_left() -> c_int;
}

/// What the call needs besides the payload, fixed at [`arm`].
struct Armed {
    start_game: usize,
    /// `nativeAppBridgeV2LeaveGame`, if exported.
    leave_game: Option<usize>,
    /// The session-lifecycle natives `ExperienceSession` calls as it starts
    /// (`f0`: resume, then fragment start) and stops (`g0`: pause, then
    /// fragment stop), by the dex's call order; `None` where not exported.
    session: [Option<usize>; 4],
    /// `nativeAppBridgeV2StartAppWithParams`, if exported.
    start_app: Option<usize>,
    assets: String,
    width: i32,
    height: i32,
}

static ARMED: OnceLock<Armed> = OnceLock::new();

/// Payloads that arrived on the engine's thread, waiting for the looper.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Set once a `Game.launch` has arrived here. Once something subscribes, the
/// bus's `getLastRaw` no longer reads it back, so `deeplink::tick` asks this.
static ARRIVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the app shell has published a `Game.launch` this module received.
pub fn arrived() -> bool {
    ARRIVED.load(std::sync::atomic::Ordering::Acquire)
}

/// The bus calls this on whichever engine thread published. Calling back into
/// the engine from inside its own dispatch is how the pump gets re-entered, so
/// this only queues; [`tick`] makes the call from the looper.
extern "C" fn on_launch(json: *const c_char) {
    // SAFETY: the C side passes a NUL-terminated string that outlives the call.
    let Some(raw) = (unsafe { crate::ffi_util::borrow_request(json) }) else {
        println!("[launch] {GAME_LAUNCH} published with a null payload");
        return;
    };
    PENDING.lock().expect("no panics under this lock").push(raw.to_string_lossy().into_owned());
    ARRIVED.store(true, std::sync::atomic::Ordering::Release);
}

/// Subscribe to `Game.launch`. Call once the bus exists (where
/// `webview::arm` is called), and only for an app-bridge bring-up.
///
/// `CORDIAL_NO_GAME_LAUNCH=1` is the control: the subscription is not made
/// and the engine is left as it was before this module existed.
pub fn arm(symbol: impl Fn(&str) -> Option<*mut c_void>, assets: &str, width: i32, height: i32) {
    if std::env::var_os("CORDIAL_NO_GAME_LAUNCH").is_some() {
        println!("  launch: not subscribing to {GAME_LAUNCH} (CORDIAL_NO_GAME_LAUNCH)");
        return;
    }
    let Some(start_game) =
        symbol("Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2StartGameWithParam")
    else {
        println!("  launch: nativeAppBridgeV2StartGameWithParam is not exported; Play will do nothing");
        return;
    };
    let Some(subscribe) = symbol("Java_com_roblox_universalapp_messagebus_MessageBus_doSubscribeRaw") else {
        println!("  launch: MessageBus.doSubscribeRaw is not exported; Play will do nothing");
        return;
    };
    let leave_game = symbol("Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2LeaveGame").map(|p| p as usize);
    let session = SESSION_NATIVES.map(|n| symbol(&format!("Java_com_roblox_engine_jni_NativeGLInterface_{n}")).map(|p| p as usize));
    let start_app = symbol("Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2StartAppWithParams").map(|p| p as usize);
    if ARMED
        .set(Armed { start_game: start_game as usize, leave_game, session, start_app, assets: assets.to_owned(), width, height })
        .is_err()
    {
        println!("  launch: arm() called twice; keeping the first");
        return;
    }
    let id = CString::new(GAME_LAUNCH).expect("literal");
    let mut err = vec![0u8; 512];
    // SAFETY: `subscribe` is the export named above; every buffer outlives the call.
    let rc = unsafe {
        cordial_messagebus_subscribe(subscribe, id.as_ptr(), Some(on_launch), err.as_mut_ptr() as *mut c_char, err.len())
    };
    if rc == 0 {
        println!("  launch: subscribed to {GAME_LAUNCH}; a launch will call nativeAppBridgeV2StartGameWithParam");
    } else {
        let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
        println!("  launch: subscribing to {GAME_LAUNCH} failed: {}", String::from_utf8_lossy(&err[..end]));
    }
}

/// `ExperienceSession`'s lifecycle natives, in [`Armed::session`]'s order.
const SESSION_NATIVES: [&str; 4] =
    ["nativeOnExperienceSessionResume", "nativeOnFragmentStart", "nativeOnExperienceSessionPause", "nativeOnFragmentStop"];

/// Whether the session lifecycle is played at all; `CORDIAL_NO_GAME_LIFECYCLE=1`
/// is the control, leaving launch and leave as they were before it.
fn lifecycle() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_NO_GAME_LIFECYCLE").is_none())
}

/// Call the session natives at `which` (indices into [`SESSION_NATIVES`]).
fn session(armed: &Armed, which: &[usize]) {
    for &i in which {
        let Some(native) = armed.session[i] else {
            println!("[launch] {} is not exported; skipped", SESSION_NATIVES[i]);
            continue;
        };
        // SAFETY: a `()V` static native of `NativeGLInterface`, resolved under
        // its own name in `arm`; the shim calls any such native the same way.
        match unsafe { cordial_linker_sys::game_activity::appbridge_leave_game(native as *mut c_void) } {
            Ok(()) => println!("[launch] {}", SESSION_NATIVES[i]),
            Err(e) => println!("[launch] {} failed: {e}", SESSION_NATIVES[i]),
        }
    }
}

/// Set by [`request_leave`]: that leave's ending follows the call itself,
/// since the engine reports no `gameDidLeave` for it.
static LEAVE_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set by [`request_leave`], taken by [`tick`].
static LEAVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asks the looper to call `nativeAppBridgeV2LeaveGame`, as Android's
/// `ExperienceSession` does when the player leaves. Only the development
/// control surface asks: what the engine publishes when the player presses
/// Leave is not established here (docs/vr/play-button.md).
pub fn request_leave() -> Result<(), String> {
    match ARMED.get() {
        None => Err("not armed (only under --app-bridge)".into()),
        Some(a) if a.leave_game.is_none() => Err("nativeAppBridgeV2LeaveGame is not exported".into()),
        Some(_) => {
            LEAVE_ASKED.store(true, std::sync::atomic::Ordering::Release);
            LEAVE.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }
    }
}

/// Set by [`request_join`], taken by [`tick`]: a place id, or 0 for none.
static JOIN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Asks the looper to publish a deep link to `place`, as `--join-url` does at
/// start. Only the development control surface asks, so that joining again
/// after a leave can be tested without a hand on the menu.
pub fn request_join(place: u64) -> Result<(), String> {
    if ARMED.get().is_none() {
        return Err("not armed (only under --app-bridge)".into());
    }
    if place == 0 {
        return Err("placeId 0".into());
    }
    JOIN.store(place, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// Called from the looper each pass. One lock and an empty check on every
/// pass without a launch, and one load each for a leave and a join.
pub fn tick() {
    let pending = std::mem::take(&mut *PENDING.lock().expect("no panics under this lock"));
    for json in pending {
        start(&json);
    }
    if LEAVE.swap(false, std::sync::atomic::Ordering::Acquire) {
        leave();
        // A leave started from this side is one the Java side already knows
        // about, and the engine reports no `gameDidLeave` for it (measured:
        // the game was left and the callback never came), so the rest of the
        // session's ending follows here instead of waiting for it.
        if LEAVE_ASKED.swap(false, std::sync::atomic::Ordering::AcqRel) {
            end_session();
        }
    }
    let place = JOIN.swap(0, std::sync::atomic::Ordering::AcqRel);
    if place != 0 {
        let id = place.to_string();
        let launch = crate::deeplink::HybridLaunch {
            place_id: &id,
            instance_id: None,
            join_attempt_id: None,
            join_attempt_origin: None,
            browser_tracker_id: None,
            access_code: None,
            link_code: None,
            reserved_server_access_code: None,
        };
        match crate::deeplink::publish_hybrid_game_launch(&launch) {
            Ok(()) => println!("[launch] published a deep link to place {place}"),
            Err(e) => println!("[launch] publishing a deep link to place {place} failed: {e}"),
        }
    }
    // SAFETY: a plain exchange on a flag in the C++ shim.
    if unsafe { cordial_take_game_left() } != 0 {
        left();
    }
}

/// The engine has left the game: the part Android's Java side plays next.
///
/// After leaving, the menu never came back and the headset showed nothing
/// more: on Android, `gameDidLeave` ends the `ExperienceSession`, whose stop
/// (`g0`) calls the session pause and fragment stop, whose release (`h0.a`)
/// calls `nativeAppBridgeV2LeaveGame`, and the app view's `surfaceChanged`
/// then hands the app half its surface again
/// (`nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams`, from `ih/a`). That
/// order is the dex's call order; the branches inside those methods have not
/// been read, so it is INFERRED that each step runs on every leave.
fn left() {
    if !lifecycle() {
        return;
    }
    let Some(armed) = ARMED.get() else { return };
    println!("[launch] the game was left; ending the session and giving the app its surface back");
    session(armed, &[2, 3]);
    leave();
    give_the_app_its_surface();
}

/// The ending of a session whose `nativeAppBridgeV2LeaveGame` this side has
/// already called.
///
/// That leave stops the game and leaves the engine at stage `Native`
/// (`leaveUGCGame` ... `setStage: (stage:Native)` in its FLog) with the
/// surface controller stopped and nothing to run: XR frames stopped for good,
/// `RBX Worker B` spun on a core, and the menu never came back.
/// `UpdateSurfaceApp` alone only `update`s a stopped controller. What starts
/// it again is `nativeAppBridgeV2StartAppWithParams`, whose engine side is
/// `startLuaApp` -> `returnToLuaApp` -> `returnToLuaAppInternal: ... App has
/// been initialized, returning from game.` -> `replaceDataModel` -> `start`.
/// On Android its callers are the app view's `surfaceCreated` and the app
/// fragment's hidden-changed override (`ih/a.L2`, `ih/a.N0` -> `ih/e.F`, dex
/// call graph), so the Java side calls it when the menu is shown again; that
/// it does so on every leave is INFERRED. `CORDIAL_NO_APP_RESTART=1` is the
/// control for this call alone.
///
/// Not for [`left`]: a leave the engine starts itself returns to the Lua app
/// on its own (`returnToLuaApp: (stage:UGCGame)` ... `setStage:
/// (stage:LuaApp)`, before `gameDidLeave`, over WiVRn).
fn end_session() {
    if !lifecycle() {
        return;
    }
    let Some(armed) = ARMED.get() else { return };
    println!("[launch] ending the session and giving the app its surface back");
    session(armed, &[2, 3]);
    if std::env::var_os("CORDIAL_NO_APP_RESTART").is_none() {
        restart_app(armed);
    }
    give_the_app_its_surface();
}

fn give_the_app_its_surface() {
    for line in crate::android::surface_params::redeliver(true, false) {
        println!("[launch] {line}");
    }
}

/// `nativeAppBridgeV2StartAppWithParams` again, with what `load.rs` gave it at
/// start: see [`end_session`].
fn restart_app(armed: &Armed) {
    let Some(native) = armed.start_app else {
        println!("[launch] nativeAppBridgeV2StartAppWithParams is not exported; the app is not restarted");
        return;
    };
    // SAFETY: resolved under its own name in `arm`, against the loaded
    // engine, which is never unloaded; the same call `load.rs` makes at start.
    match unsafe { cordial_linker_sys::game_activity::appbridge_start_app(native as *mut c_void, &armed.assets, armed.width, armed.height) } {
        Ok(()) => println!("[launch] nativeAppBridgeV2StartAppWithParams returned"),
        Err(e) => println!("[launch] nativeAppBridgeV2StartAppWithParams failed: {e}"),
    }
}

fn leave() {
    let Some(native) = ARMED.get().and_then(|a| a.leave_game) else { return };
    println!("[launch] calling nativeAppBridgeV2LeaveGame");
    // SAFETY: resolved under its own name in `arm`, against the loaded
    // engine, which is never unloaded.
    match unsafe { cordial_linker_sys::game_activity::appbridge_leave_game(native as *mut c_void) } {
        Ok(()) => println!("[launch] nativeAppBridgeV2LeaveGame returned"),
        Err(e) => println!("[launch] nativeAppBridgeV2LeaveGame failed: {e}"),
    }
}

fn start(json: &str) {
    let Some(armed) = ARMED.get() else { return };
    let (fields, carried, dropped) = match parse(json) {
        Ok(p) => p,
        Err(why) => {
            println!("[launch] {GAME_LAUNCH} arrived ({} bytes) but was not used: {why}", json.len());
            return;
        }
    };
    println!(
        "[launch] {GAME_LAUNCH} arrived: placeId {}, carrying [{}]{}",
        fields.place_id,
        carried.join(", "),
        if dropped.is_empty() { String::new() } else { format!(", not carried [{}]", dropped.join(", ")) },
    );
    // SAFETY: `start_game` was resolved under its own name in `arm`, against
    // the loaded engine, which is never unloaded.
    match unsafe {
        cordial_linker_sys::game_activity::appbridge_start_game(
            armed.start_game as *mut c_void,
            &armed.assets,
            armed.width,
            armed.height,
            &fields,
        )
    } {
        Ok(rc) => {
            println!("[launch] nativeAppBridgeV2StartGameWithParam -> {rc}");
            // `ExperienceSession.f0`, reached from the same Play chain (`k0`).
            if lifecycle() {
                session(armed, &[0, 1]);
            }
        }
        Err(e) => println!("[launch] nativeAppBridgeV2StartGameWithParam failed: {e}"),
    }
}

/// The payload's keys that `StartGameParams` has an accessor for, as that
/// accessor spells them.
const STRING_KEYS: [&str; 13] = [
    "accessCode",
    "callId",
    "eventId",
    "gameId",
    "gameIdToExclude",
    "gameJoinContext",
    "isoContext",
    "joinAttemptId",
    "joinAttemptOrigin",
    "launchData",
    "linkCode",
    "referralPage",
    "reservedServerAccessCode",
];
const LONG_KEYS: [&str; 3] = ["placeId", "conversationId", "referredByPlayerId"];
const INT_KEYS: [&str; 1] = ["joinRequestType"];

/// A payload's fields, the keys that were carried, and the keys that were not.
///
/// A number may arrive as a JSON number or as a string of digits: the Servers
/// list's `launchGame` payload sends `placeId` as a string
/// (docs/analysis/app-bridge.md §9), the deep link's `Game.launch` as a
/// number. A key present with a value of the wrong shape is refused whole,
/// rather than joined with a zero in its place.
fn parse(json: &str) -> Result<(StartGame, Vec<String>, Vec<String>), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let mut f = StartGame::default();
    let mut carried = Vec::new();
    let mut dropped = Vec::new();
    for (k, val) in obj {
        let k = k.as_str();
        if let Some(i) = STRING_KEYS.iter().position(|&s| s == k) {
            let s = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Null => String::new(),
                _ => return Err(format!("{k} is not a string")),
            };
            let slot = match i {
                0 => &mut f.access_code,
                1 => &mut f.call_id,
                2 => &mut f.event_id,
                3 => &mut f.game_id,
                4 => &mut f.game_id_to_exclude,
                5 => &mut f.game_join_context,
                6 => &mut f.iso_context,
                7 => &mut f.join_attempt_id,
                8 => &mut f.join_attempt_origin,
                9 => &mut f.launch_data,
                10 => &mut f.link_code,
                11 => &mut f.referral_page,
                _ => &mut f.reserved_server_access_code,
            };
            *slot = s;
        } else if LONG_KEYS.contains(&k) || INT_KEYS.contains(&k) {
            let n = match val {
                serde_json::Value::Number(n) => n.as_i64(),
                serde_json::Value::String(s) => s.parse::<i64>().ok(),
                _ => None,
            }
            .ok_or_else(|| format!("{k} is not an integer"))?;
            match k {
                "placeId" => f.place_id = n,
                "conversationId" => f.conversation_id = n,
                "referredByPlayerId" => f.referred_by_player_id = n,
                _ => f.join_request_type = i32::try_from(n).map_err(|_| format!("{k} is out of range"))?,
            }
        } else {
            dropped.push(k.to_owned());
            continue;
        }
        carried.push(k.to_owned());
    }
    if f.place_id <= 0 {
        return Err("no placeId".into());
    }
    Ok((f, carried, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload the Quest build's app shell published for a deep link,
    /// with the join attempt id replaced.
    #[test]
    fn the_deep_link_payload_is_carried_whole() {
        let (f, carried, dropped) =
            parse(r#"{"placeId":1818,"referralPage":"DeepLink","joinAttemptId":"00000000-0000-0000-0000-000000000000"}"#)
                .unwrap();
        assert_eq!(f.place_id, 1818);
        assert_eq!(f.referral_page, "DeepLink");
        assert_eq!(f.join_attempt_id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(f.join_request_type, 0);
        assert_eq!(carried.len(), 3);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_key_with_no_accessor_is_named_and_not_mapped() {
        let (f, _, dropped) = parse(r#"{"placeId":"1818","instanceId":"abc","isPlayTogetherGame":false}"#).unwrap();
        assert_eq!(f.place_id, 1818);
        assert_eq!(f.game_id, "");
        assert_eq!(dropped, vec!["instanceId".to_string(), "isPlayTogetherGame".to_string()]);
    }

    #[test]
    fn a_payload_without_a_place_is_refused() {
        assert!(parse(r#"{"referralPage":"DeepLink"}"#).is_err());
        assert!(parse("not json").is_err());
        assert!(parse("[]").is_err());
    }

    #[test]
    fn a_number_of_the_wrong_shape_is_refused_rather_than_zeroed() {
        assert!(parse(r#"{"placeId":1818,"joinRequestType":"soon"}"#).is_err());
        assert!(parse(r#"{"placeId":{"id":1818}}"#).is_err());
    }
}
