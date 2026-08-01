//! Exercises the AT-SPI bridge (`android::accessibility`) with synthetic
//! nodes, with no Roblox APK or `libjnivm` involved at all.
//!
//! Why this exists: the bridge was written and this session had no Roblox
//! APK available to observe what the engine actually populates (see
//! `native/accessibility.cpp`'s and `android/accessibility.rs`'s own header
//! comments). AGENTS.md's rule against reporting an unobserved result still
//! applies to *this* half of the work — the AT-SPI-facing half does not need
//! Roblox to be exercised for real, only real node data to look at, and this
//! binary is the seed for it: three fake nodes (a button, a checkbox, a
//! label), injected straight into the same C++ registry
//! `AccessibilityNodeInfo`'s real hooks would have written to, via the
//! `cordial_accessibility_test_seed_node` escape hatch that exists
//! specifically for this. Nothing here should ever be mistaken for a claim
//! about what Roblox does — see the printed banner below, which says so.
//!
//! Run it, then in another terminal:
//!
//! ```text
//! busctl --user call org.a11y.Bus /org/a11y/bus org.a11y.Bus GetAddress
//! gdbus introspect --address "unix:path=<from above>" \
//!     --dest <this process's unique name, printed below> \
//!     --object-path /org/a11y/atspi/accessible/root
//! ```
//!
//! or open Accerciser and look for "Cordial" in the application list.

use cordial_linker_sys::accessibility as ffi;

fn main() {
    eprintln!("==================================================================");
    eprintln!("accessibility_probe: SYNTHETIC test fixtures only.");
    eprintln!("This does not run Roblox and proves nothing about what Roblox's");
    eprintln!("engine populates — it exists only to verify the AT-SPI-facing half");
    eprintln!("of the bridge (crates/cordial-runtime/src/android/accessibility.rs)");
    eprintln!("against a real AT-SPI bus and a real screen reader.");
    eprintln!("==================================================================");

    ffi::test_clear();
    let button = ffi::test_seed_node(
        "Roblox.TextButton",
        "Sign In",
        "",
        100,
        100,
        260,
        140,
        ffi::state_bit::ENABLED
            | ffi::state_bit::FOCUSABLE
            | ffi::state_bit::VISIBLE_TO_USER
            | ffi::state_bit::CLICKABLE,
    );
    let checkbox = ffi::test_seed_node(
        "Roblox.CheckBox",
        "Remember me",
        "",
        100,
        160,
        260,
        190,
        ffi::state_bit::ENABLED
            | ffi::state_bit::FOCUSABLE
            | ffi::state_bit::VISIBLE_TO_USER
            | ffi::state_bit::CHECKABLE,
    );
    let label = ffi::test_seed_node(
        "Roblox.TextLabel",
        "",
        "Cordial connects Roblox to your desktop.",
        100,
        60,
        400,
        90,
        ffi::state_bit::VISIBLE_TO_USER,
    );
    eprintln!(
        "seeded nodes: button={button} checkbox={checkbox} label={label} (see the banner above)"
    );

    cordial_runtime::android::accessibility::start();

    eprintln!("bridge started; sleeping so an external gdbus/accerciser probe has something to look at.");
    eprintln!("Ctrl-C to exit.");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
