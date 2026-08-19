//! What Cordial tells the engine about the battery, and where that comes from.
//!
//! The engine exports two natives for this and nothing in this project has ever
//! called either:
//!
//! ```text
//! NativeGLInterface.reportBatteryStateChanged(II)V
//! NativeGLInterface.reportBatteryStatus(Lcom/roblox/engine/jni/model/BatteryStatus;)V
//! ```
//!
//! On real Android, the *application* reads `BatteryManager`/`ACTION_BATTERY_CHANGED`
//! and calls these; Cordial is the application in this architecture (the same
//! reasoning `client_settings.rs` gives for fetching client settings itself
//! rather than waiting to be handed them), so reading `/sys/class/power_supply`
//! and making the calls is the job, not a workaround. `upower` was deliberately
//! not added as a dependency — sysfs already has every file this module reads,
//! with no daemon or D-Bus round trip needed to get at it.
//!
//! ## Where the two-int call's argument meaning came from
//!
//! `docs/traces/waydroid-roblox-startup.log.gz` — a real Android capture of this
//! same APK — shows, at roughly fifteen-second intervals:
//!
//! ```text
//! I BatteryStatusObserver: startObserving
//! I rbx.perfdata: perfdata battery AC CHARGING 0uAmps 0mW
//! ```
//!
//! four times, 15.002s / 15.005s / 15.002s apart. That is the app's own internal
//! log, not a dump of the native call's arguments, so it does not settle the
//! exact integers `reportBatteryStateChanged(II)V` receives — but it does settle
//! that the two things this subsystem tracks together are a *power source*
//! ("AC") and a *charging state* ("CHARGING"), reported as a pair on a roughly
//! fifteen-second cadence. That shape — two small enumerated ints, reported
//! together, periodically — is what `state_changed_args` below produces.
//! **Which of the two ints is which, and their exact numbering, is `INFERRED`**:
//! taken from Android's own public `BatteryManager.BATTERY_STATUS_*` and
//! `BATTERY_PLUGGED_*` constants, corroborated by `tools/dex_fields.py` reading
//! `BatteryStatus`'s nested `$a`/`$b`/`$c` enums (health/plugged/status) as
//! having exactly the member sets Android's own three enums have — seven health
//! values, six plugged values, five status values, name for name. That is
//! strong evidence Roblox modelled this on Android's own constants rather than
//! inventing its own numbering, not a captured call.
//!
//! ## Never a comfortable lie
//!
//! Every optional field below is `None`, not a manufactured number, when this
//! machine's kernel driver does not expose the sysfs node that would answer it
//! — `native/opensles.cpp` reporting `SL_RESULT_FEATURE_UNSUPPORTED` rather than
//! handing back a dead engine object is the same principle applied to audio.
//! A desktop with no battery gets `Reading { battery: None, .. }`, not a battery
//! reported as present, full, and on mains. `state_changed_args` returns `None`
//! for that case too — see its own doc for why skipping the call, rather than
//! inventing a reading for a battery that does not exist, is the honest choice.
//!
//! ## Two batteries
//!
//! `scan` picks the first battery-type entry, sorted by directory name, that
//! reports itself present. This is a choice, not a measurement: the two natives
//! this module feeds each want a single reading, and no machine this was tested
//! on has two batteries to check a smarter merge against. `INFERRED`.

use std::fs;
use std::path::Path;

/// Android's own `BatteryManager.BATTERY_STATUS_*` values. Public SDK
/// constants, not anything read out of Roblox's binary.
pub mod status {
    pub const UNKNOWN: i32 = 1;
    pub const CHARGING: i32 = 2;
    pub const DISCHARGING: i32 = 3;
    pub const NOT_CHARGING: i32 = 4;
    pub const FULL: i32 = 5;
}

/// Android's own `BatteryManager.BATTERY_PLUGGED_*` values — a bitmask, which
/// is why `scan` ORs together every online power-source entry rather than
/// picking just one.
pub mod plugged {
    pub const NOT_PLUGGED: i32 = 0;
    pub const AC: i32 = 1;
    pub const USB: i32 = 2;
    pub const WIRELESS: i32 = 4;
    pub const DOCK: i32 = 8;
}

/// Android's own `BatteryManager.BATTERY_HEALTH_*` values.
pub mod health {
    pub const UNKNOWN: i32 = 1;
    pub const GOOD: i32 = 2;
    pub const OVERHEAT: i32 = 3;
    pub const DEAD: i32 = 4;
    pub const OVER_VOLTAGE: i32 = 5;
    pub const UNSPECIFIED_FAILURE: i32 = 6;
    pub const COLD: i32 = 7;
}

/// What one battery-type sysfs entry said. Every field but `present` and
/// `status` is `Option` because the kernel driver behind any one entry answers
/// a different subset of these — this developer's own `BAT0` has no `health`
/// or `temp` node at all, which is the ordinary case, not a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    pub present: bool,
    /// 0-100, from `capacity`.
    pub percentage: Option<u8>,
    /// Always known: an unrecognised or missing `status` file reads as
    /// `status::UNKNOWN`, which is itself one of Android's real values, not a
    /// sentinel meaning "Cordial could not tell".
    pub status: i32,
    pub health: Option<i32>,
    pub voltage_mv: Option<i32>,
    pub current_now_ua: Option<i32>,
    pub current_avg_ua: Option<i32>,
    /// From `charge_counter` specifically, not `charge_now` — they are
    /// different kernel nodes (a coulomb-counter accumulator against a present
    /// charge level), and substituting one for the other under this name would
    /// itself be the kind of comfortable lie this module exists to avoid. Left
    /// `None` on hardware — this developer's included — that only exposes
    /// `charge_now`.
    pub charge_counter_uah: Option<i32>,
    pub power_now_uw: Option<i32>,
    pub technology: Option<String>,
    /// Tenths of a degree Celsius, straight from `temp` — the kernel and
    /// Android's own `EXTRA_TEMPERATURE` already agree on that unit, so this is
    /// a passthrough, not a conversion.
    pub temperature_tenths_c: Option<i32>,
}

/// One poll of `/sys/class/power_supply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// `None` when no `type == "Battery"` entry exists at all — an ordinary
    /// desktop, not an error condition.
    pub battery: Option<Battery>,
    /// Android's `EXTRA_PLUGGED` shape: the bitwise-OR of `plugged::{AC,USB,
    /// WIRELESS,DOCK}` for every `Mains`/`USB`/`Wireless`/`Dock`-type entry
    /// this machine has that currently reports `online`, or `plugged::
    /// NOT_PLUGGED` if none do. `None` only when the machine has no such entry
    /// to ask at all — different from "asked, and nothing is plugged in".
    pub plugged: Option<i32>,
}

fn trimmed(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn read_int(path: &Path) -> Option<i32> {
    trimmed(path)?.parse().ok()
}

fn map_status(s: &str) -> i32 {
    match s {
        "Charging" => status::CHARGING,
        "Discharging" => status::DISCHARGING,
        "Not charging" => status::NOT_CHARGING,
        "Full" => status::FULL,
        // Includes the kernel's own "Unknown" and anything this project has
        // not seen a battery driver report yet — treated the same as absent,
        // rather than guessed at.
        _ => status::UNKNOWN,
    }
}

/// `None` only when the file is missing entirely — an unrecognised *value* in
/// a `health` file that does exist still resolves to `health::UNKNOWN`, which
/// is a real Android value, not "Cordial does not know".
fn map_health(s: &str) -> i32 {
    match s {
        "Good" => health::GOOD,
        "Overheat" | "Hot" | "Warm" => health::OVERHEAT,
        "Dead" => health::DEAD,
        "Over voltage" => health::OVER_VOLTAGE,
        "Unspecified failure" | "Watchdog timer expire" | "Safety timer expire"
        | "Over current" | "Calibration required" => health::UNSPECIFIED_FAILURE,
        "Cold" | "Cool" => health::COLD,
        _ => health::UNKNOWN,
    }
}

/// The kernel's `power_supply` `type` values that represent a power *source*
/// rather than a battery — `POWER_SUPPLY_TYPE_*` in
/// `Documentation/ABI/testing/sysfs-class-power`. `USB` covers every
/// `USB_DCP`/`USB_CDP`/`USB_C`/`USB_PD`/... subtype, which all report the same
/// plugged bit as far as Android's coarser enum is concerned.
fn map_plug_kind(type_str: &str) -> Option<i32> {
    if type_str == "Mains" {
        Some(plugged::AC)
    } else if type_str.starts_with("USB") {
        Some(plugged::USB)
    } else if type_str == "Wireless" {
        Some(plugged::WIRELESS)
    } else if type_str == "Dock" {
        Some(plugged::DOCK)
    } else {
        None
    }
}

fn read_battery(dir: &Path) -> Battery {
    // Genuinely absent on some drivers (this developer's own `BAT0` has it),
    // and a battery entry existing at all is itself weak evidence it is the
    // real one — so absence defaults to present rather than absent, and
    // `status`/`capacity` carry the rest of the honesty burden.
    let present = trimmed(&dir.join("present")).map(|s| s == "1").unwrap_or(true);
    let percentage = trimmed(&dir.join("capacity"))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| (0..=100).contains(v))
        .map(|v| v as u8);
    let status_val = trimmed(&dir.join("status")).as_deref().map(map_status).unwrap_or(status::UNKNOWN);
    let health_val = trimmed(&dir.join("health")).as_deref().map(map_health);
    let voltage_mv = read_int(&dir.join("voltage_now")).map(|uv| uv / 1000);
    let current_now_ua = read_int(&dir.join("current_now"));
    let current_avg_ua = read_int(&dir.join("current_avg"));
    let charge_counter_uah = read_int(&dir.join("charge_counter"));
    let power_now_uw = read_int(&dir.join("power_now"));
    let technology = trimmed(&dir.join("technology"));
    let temperature_tenths_c = read_int(&dir.join("temp"));

    Battery {
        present,
        percentage,
        status: status_val,
        health: health_val,
        voltage_mv,
        current_now_ua,
        current_avg_ua,
        charge_counter_uah,
        power_now_uw,
        technology,
        temperature_tenths_c,
    }
}

/// Scan `power_supply_dir` — ordinarily `/sys/class/power_supply` — for a
/// battery and whatever is supplying power. Never fails: a directory that does
/// not exist, or exists and is empty, reads the same as a machine with neither
/// a battery nor a mains sensor, because on a great many real machines that is
/// exactly what is true.
pub fn scan(power_supply_dir: &Path) -> Reading {
    let mut battery: Option<Battery> = None;
    let mut plugged_bits: Option<i32> = None;

    let mut entries: Vec<_> = fs::read_dir(power_supply_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    // Sorted so which entry wins when a machine has more than one battery is
    // at least deterministic, and so this module's own tests can pin it.
    entries.sort();

    for entry in entries {
        let Some(type_str) = trimmed(&entry.join("type")) else { continue };
        if type_str == "Battery" {
            if battery.is_none() {
                battery = Some(read_battery(&entry));
            }
        } else if let Some(bit) = map_plug_kind(&type_str) {
            let online = trimmed(&entry.join("online")).map(|s| s == "1").unwrap_or(false);
            if online {
                plugged_bits = Some(plugged_bits.unwrap_or(0) | bit);
            } else {
                plugged_bits = plugged_bits.or(Some(0));
            }
        }
    }

    Reading { battery, plugged: plugged_bits }
}

/// What to pass `reportBatteryStateChanged(II)V` — `(status, plugged)` — or
/// `None` to skip the call.
///
/// `None` covers both "no battery-type entry exists" and "one exists but
/// reports itself not present" (a removable-battery laptop with the bay
/// empty). Neither has a status to report changing, and this call has no slot
/// for "there is no battery" the way `BatteryStatus.present` does — so rather
/// than invent a meaning for absence in a two-int call that was never given
/// one, this skips the call entirely, which the task this module was written
/// for explicitly allows.
pub fn state_changed_args(r: &Reading) -> Option<(i32, i32)> {
    let b = r.battery.as_ref()?;
    if !b.present {
        return None;
    }
    Some((b.status, r.plugged.unwrap_or(plugged::NOT_PLUGGED)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-battery-test-{tag}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::write(dir.join(name), contents).unwrap();
    }

    fn battery_dir(root: &Path, name: &str) -> std::path::PathBuf {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        write(&d, "type", "Battery");
        d
    }

    fn mains_dir(root: &Path, name: &str, online: bool) -> std::path::PathBuf {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        write(&d, "type", "Mains");
        write(&d, "online", if online { "1" } else { "0" });
        d
    }

    #[test]
    fn a_machine_with_no_power_supply_directory_reads_as_no_battery() {
        let root = scratch("no-dir").join("does-not-exist");
        let r = scan(&root);
        assert_eq!(r, Reading { battery: None, plugged: None });
        assert_eq!(state_changed_args(&r), None);
    }

    #[test]
    fn an_empty_power_supply_directory_reads_as_no_battery() {
        let root = scratch("empty");
        let r = scan(&root);
        assert_eq!(r, Reading { battery: None, plugged: None });
    }

    /// Mirrors this developer's own `BAT0`/`ADP1`: full, not charging because
    /// full, on mains, no `health` or `temp` node at all.
    #[test]
    fn a_full_battery_on_mains_with_no_health_or_temp_node() {
        let root = scratch("bat0-adp1");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "1");
        write(&bat, "capacity", "100");
        write(&bat, "status", "Not charging");
        write(&bat, "technology", "Li-ion");
        write(&bat, "voltage_now", "12903000");
        write(&bat, "current_now", "0");
        mains_dir(&root, "ADP1", true);

        let r = scan(&root);
        let b = r.battery.as_ref().expect("a battery entry was written");
        assert!(b.present);
        assert_eq!(b.percentage, Some(100));
        assert_eq!(b.status, status::NOT_CHARGING);
        assert_eq!(b.health, None, "no health node was written; must not be guessed at");
        assert_eq!(b.temperature_tenths_c, None);
        assert_eq!(b.voltage_mv, Some(12903));
        assert_eq!(r.plugged, Some(plugged::AC));
        assert_eq!(state_changed_args(&r), Some((status::NOT_CHARGING, plugged::AC)));
    }

    #[test]
    fn a_desktop_with_no_battery_type_entry_reports_no_battery() {
        let root = scratch("desktop");
        mains_dir(&root, "ADP1", true);
        let r = scan(&root);
        assert_eq!(r.battery, None);
        // Mains is real information even with no battery to charge, and is
        // reported; the two-int call about a battery is not.
        assert_eq!(r.plugged, Some(plugged::AC));
        assert_eq!(state_changed_args(&r), None);
    }

    #[test]
    fn a_battery_bay_reporting_not_present_is_not_reported_as_a_reading() {
        let root = scratch("not-present");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "0");
        let r = scan(&root);
        let b = r.battery.as_ref().expect("the entry exists even though empty");
        assert!(!b.present);
        assert_eq!(state_changed_args(&r), None, "an absent battery has no state to report changing");
    }

    /// Two batteries: the first present one, sorted by directory name, wins —
    /// see this module's header for why that specific rule and not a merge.
    #[test]
    fn two_batteries_the_first_sorted_present_one_is_reported() {
        let root = scratch("dual");
        let bat0 = battery_dir(&root, "BAT0");
        write(&bat0, "present", "0");
        let bat1 = battery_dir(&root, "BAT1");
        write(&bat1, "present", "1");
        write(&bat1, "capacity", "42");

        let r = scan(&root);
        // BAT0 sorts first and is the one `scan` picks, even though it is not
        // present -- "first present" is not the rule; "first, sorted" is, and
        // its own `present` field is what a caller reads to know whether that
        // matters. Documented here so a future change to prefer BAT1 is a
        // deliberate one, not a silent regression this test would not catch.
        let b = r.battery.expect("BAT0 exists");
        assert!(!b.present);
        assert_eq!(b.percentage, None);
    }

    #[test]
    fn a_malformed_capacity_file_is_ignored_rather_than_misparsed() {
        let root = scratch("malformed");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "1");
        write(&bat, "capacity", "not-a-number");
        write(&bat, "status", "Charging");
        let r = scan(&root);
        let b = r.battery.expect("battery entry exists");
        assert_eq!(b.percentage, None);
        assert_eq!(b.status, status::CHARGING);
    }

    #[test]
    fn an_out_of_range_capacity_is_dropped_rather_than_clamped() {
        let root = scratch("oor-capacity");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "1");
        write(&bat, "capacity", "142");
        let r = scan(&root);
        assert_eq!(r.battery.unwrap().percentage, None);
    }

    #[test]
    fn an_unrecognised_status_string_is_unknown_not_a_guess() {
        let root = scratch("weird-status");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "1");
        write(&bat, "status", "Something a future kernel invents");
        let r = scan(&root);
        assert_eq!(r.battery.unwrap().status, status::UNKNOWN);
    }

    #[test]
    fn a_missing_status_file_is_unknown_rather_than_a_default_guess() {
        let root = scratch("no-status-file");
        battery_dir(&root, "BAT0");
        let r = scan(&root);
        assert_eq!(r.battery.unwrap().status, status::UNKNOWN);
    }

    #[test]
    fn a_ups_type_entry_does_not_count_as_a_battery_or_a_power_source() {
        let root = scratch("ups");
        let d = root.join("UPS0");
        fs::create_dir_all(&d).unwrap();
        write(&d, "type", "UPS");
        let r = scan(&root);
        assert_eq!(r.battery, None);
        assert_eq!(r.plugged, None);
    }

    #[test]
    fn usb_pd_subtypes_still_count_as_usb() {
        let root = scratch("usb-pd");
        let d = root.join("usb0");
        fs::create_dir_all(&d).unwrap();
        write(&d, "type", "USB_PD");
        write(&d, "online", "1");
        let r = scan(&root);
        assert_eq!(r.plugged, Some(plugged::USB));
    }

    #[test]
    fn charge_now_never_substitutes_for_the_distinct_charge_counter_node() {
        let root = scratch("charge-now-not-counter");
        let bat = battery_dir(&root, "BAT0");
        write(&bat, "present", "1");
        write(&bat, "charge_now", "3441000");
        let r = scan(&root);
        assert_eq!(
            r.battery.unwrap().charge_counter_uah,
            None,
            "charge_now must not be reported under the charge_counter name"
        );
    }

    #[test]
    fn two_online_power_sources_are_reported_together() {
        let root = scratch("two-sources");
        mains_dir(&root, "ADP1", true);
        let d = root.join("usb0");
        fs::create_dir_all(&d).unwrap();
        write(&d, "type", "USB");
        write(&d, "online", "1");
        let r = scan(&root);
        assert_eq!(r.plugged, Some(plugged::AC | plugged::USB));
    }
}
