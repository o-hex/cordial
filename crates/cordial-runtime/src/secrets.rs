//! Session secret storage shared with the launcher.
//!
//! Backend implementation lives in `cordial-shell`, the lowest existing crate
//! both entry points can depend on without introducing a dependency cycle.

use std::sync::OnceLock;

pub use cordial_shell::secrets::{
    erase, load, save, snapshot, usable, where_kept, Kind, SnapshotRequest, Store,
};

/// Decide once, say so once.
pub fn active() -> Store {
    static ACTIVE: OnceLock<Store> = OnceLock::new();
    *ACTIVE.get_or_init(|| {
        cordial_shell::secrets::configured(&crate::profile::active().join("cookies"))
    })
}
