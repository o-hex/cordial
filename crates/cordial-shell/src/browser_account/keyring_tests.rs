use super::*;
use cordial_shell::secrets::{self, Kind, Store};
use std::num::NonZeroU64;
use std::path::PathBuf;

fn account(id: u64) -> AccountId {
    AccountId(NonZeroU64::new(id).unwrap())
}

struct KeyringProfileCleanup {
    dir: PathBuf,
}

impl Drop for KeyringProfileCleanup {
    fn drop(&mut self) {
        let _ = secrets::erase(Store::Keyring, &self.dir, Kind::Identity);
        let _ = secrets::erase(Store::Keyring, &self.dir, Kind::Cookies);
    }
}

#[test]
fn keyring_match_rejects_claim_time_secret_changes() {
    // Given a unique synthetic profile stored in the real desktop keyring.
    if let Err(why) = secrets::usable() {
        println!("skipped: {why}");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("browser");
    std::fs::create_dir(&dir).unwrap();
    let _cleanup = KeyringProfileCleanup { dir: dir.clone() };
    secrets::save(
        Store::Keyring,
        &dir,
        Kind::Identity,
        r#"{"schema":1,"userId":34}"#,
    )
    .unwrap();
    secrets::save(
        Store::Keyring,
        &dir,
        Kind::Cookies,
        "# cordial synthetic test\n.roblox.com\t.ROBLOSECURITY=SYNTHETIC-34\n",
    )
    .unwrap();
    let matched = profile::matching_profile_with(
        profile::ProfileQuery {
            root: root.path(),
            account: account(34),
            store: Store::Keyring,
        },
        |_| Some(account(34)),
    )
    .unwrap();
    assert_eq!(matched.store(), Store::Keyring);

    // When identity changes after HTTP validation but before launch owns the lock.
    secrets::save(
        Store::Keyring,
        &dir,
        Kind::Identity,
        r#"{"schema":1,"userId":35}"#,
    )
    .unwrap();

    // Then exact keyring evidence cannot authorise the changed profile.
    assert!(!matched.still_matches("browser", &dir));
}
