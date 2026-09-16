use super::transport::{AccountId, SessionCookie};
use cordial_shell::secrets::{self, Kind, SnapshotRequest, Store};
use std::path::Path;

const MAX_IDENTITY_BYTES: usize = 16 * 1024;
const MAX_COOKIE_BYTES: usize = 1024 * 1024;

#[derive(serde::Deserialize)]
struct SavedIdentity {
    schema: u64,
    #[serde(rename = "userId")]
    user_id: AccountId,
}

/// Exact saved values that established an automatic account match.
///
/// Deliberately carries opaque bytes rather than parsed credentials. The
/// launcher compares them after acquiring the profile lock and never exposes,
/// formats or logs them.
pub(crate) struct ProfileMatch {
    name: String,
    store: Store,
    identity: Vec<u8>,
    cookies: Vec<u8>,
}

impl ProfileMatch {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn store(&self) -> Store {
        self.store
    }

    pub(crate) fn still_matches(&self, name: &str, profile_dir: &Path) -> bool {
        self.name == name
            && secrets::snapshot(SnapshotRequest {
                store: self.store,
                profile_dir,
                kind: Kind::Identity,
                max_bytes: MAX_IDENTITY_BYTES,
            })
            .as_deref()
                == Some(self.identity.as_slice())
            && secrets::snapshot(SnapshotRequest {
                store: self.store,
                profile_dir,
                kind: Kind::Cookies,
                max_bytes: MAX_COOKIE_BYTES,
            })
            .as_deref()
                == Some(self.cookies.as_slice())
    }
}

pub(super) struct ProfileQuery<'a> {
    pub(super) root: &'a Path,
    pub(super) account: AccountId,
    pub(super) store: Store,
}

pub(super) fn matching_profile_with(
    query: ProfileQuery<'_>,
    mut authenticate: impl FnMut(&SessionCookie) -> Option<AccountId>,
) -> Option<ProfileMatch> {
    let mut matches = std::fs::read_dir(query.root).ok()?.filter_map(|entry| {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            return None;
        }
        let name = entry.file_name().into_string().ok()?;
        if !cordial_shell::profile::is_valid_name(&name) {
            return None;
        }
        let profile_dir = entry.path();
        let identity_before = secrets::snapshot(SnapshotRequest {
            store: query.store,
            profile_dir: &profile_dir,
            kind: Kind::Identity,
            max_bytes: MAX_IDENTITY_BYTES,
        })?;
        let identity: SavedIdentity = serde_json::from_slice(&identity_before).ok()?;
        if identity.schema != 1 || identity.user_id != query.account {
            return None;
        }
        let cookies_before = secrets::snapshot(SnapshotRequest {
            store: query.store,
            profile_dir: &profile_dir,
            kind: Kind::Cookies,
            max_bytes: MAX_COOKIE_BYTES,
        })?;
        let session = session_from_store(&cookies_before)?;
        if authenticate(&session) != Some(query.account) {
            return None;
        }
        // The client reads these values after routing. A periodic cookie flush
        // or identity save during the HTTP check makes that future read a
        // different session, so fail closed instead of launching from the stale
        // decision.
        (secrets::snapshot(SnapshotRequest {
            store: query.store,
            profile_dir: &profile_dir,
            kind: Kind::Identity,
            max_bytes: MAX_IDENTITY_BYTES,
        })?
        .as_slice()
            == identity_before
            && secrets::snapshot(SnapshotRequest {
                store: query.store,
                profile_dir: &profile_dir,
                kind: Kind::Cookies,
                max_bytes: MAX_COOKIE_BYTES,
            })?
            .as_slice()
                == cookies_before)
            .then_some(ProfileMatch {
                name,
                store: query.store,
                identity: identity_before,
                cookies: cookies_before,
            })
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

#[cfg(test)]
pub(crate) fn snapshot_for_test(
    name: &str,
    profile_dir: &Path,
    store: Store,
) -> Option<ProfileMatch> {
    Some(ProfileMatch {
        name: name.to_string(),
        store,
        identity: secrets::snapshot(SnapshotRequest {
            store,
            profile_dir,
            kind: Kind::Identity,
            max_bytes: MAX_IDENTITY_BYTES,
        })?,
        cookies: secrets::snapshot(SnapshotRequest {
            store,
            profile_dir,
            kind: Kind::Cookies,
            max_bytes: MAX_COOKIE_BYTES,
        })?,
    })
}

fn session_from_store(bytes: &[u8]) -> Option<SessionCookie> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut session: Option<SessionCookie> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let (_, jar) = line.split_once('\t')?;
        let jar = unescape(jar);
        for pair in jar.split(';').map(str::trim) {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            if name != ".ROBLOSECURITY" {
                continue;
            }
            let candidate = SessionCookie::from_value(value)?;
            match &session {
                Some(current) if current != &candidate => return None,
                Some(_) => {}
                None => session = Some(candidate),
            }
        }
    }
    session
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
