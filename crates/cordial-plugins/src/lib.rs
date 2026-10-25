//! Cordial's plugin host.
//!
//! Plugins are separate processes speaking newline-delimited JSON over stdio,
//! with every call gated by a named capability. See
//! [ADR-003](../../../docs/adr/ADR-003-plugin-isolation.md) for why isolation is
//! by process rather than by a restricted in-process API, and
//! [ADR-005](../../../docs/adr/ADR-005-flag-service.md) for why flag writes are
//! split across two capabilities.
//!
//! What a plugin is allowed to do, and anything it remembers, belong to the
//! profile rather than to the machine — see
//! [ADR-013](../../../docs/adr/ADR-013-per-profile-configuration.md).

// The one `unsafe` this crate ever had was `kill(-pid, SIGKILL)` in `host.rs`,
// and it is gone -- `rustix::process::kill_process_group` now does the same
// syscall safely. `forbid` rather than `deny` so a future contributor cannot
// quietly reopen this with a local `#[allow(unsafe_code)]`; reopening it
// takes removing this line, which is a change
// [ADR-036](../../../docs/adr/ADR-036-unsafe-is-a-boundary-not-a-convention.md)
// asks be justified on its own.
#![forbid(unsafe_code)]

pub mod broker;
pub mod capability;
pub mod consent;
pub mod core_events;
pub mod denials;
pub mod enablement;
pub mod events;
pub mod flag_document;
pub mod grants;
pub mod health;
pub mod host;
pub mod manifest;
pub mod marketplace;
pub mod notify;
pub mod plugin_data;
pub mod preferences;
pub mod presence;
pub mod protocol;
pub mod reconcile;
pub mod sandbox;
pub mod registry;
pub mod resolve;
pub mod settings;
pub mod state;
pub mod sign;
pub mod source;
pub mod unpack;
pub mod urlopen;
