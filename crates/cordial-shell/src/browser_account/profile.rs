use super::transport::{AccountId, SessionCookie};
use std::io::Read;
use std::path::Path;

const MAX_IDENTITY_BYTES: u64 = 16 * 1024;
const MAX_COOKIE_BYTES: u64 = 1024 * 1024;

#[derive(serde::Deserialize)]
struct SavedIdentity {
    schema: u64,
    #[serde(rename = "userId")]
    user_id: AccountId,
}

/// Exact profile files that established an automatic account match.
///
/// Deliberately carries opaque bytes rather than parsed credentials. The
/// launcher compares them after acquiring the profile lock and never exposes,
/// formats or logs them.
pub(crate) struct ProfileMatch {
    name: String,
    identity: Vec<u8>,
    cookies: Vec<u8>,
}

impl ProfileMatch {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn still_matches(&self, name: &str, profile_dir: &Path) -> bool {
        self.name == name
            && read_limited(&profile_dir.join("identity"), MAX_IDENTITY_BYTES).as_deref()
                == Some(self.identity.as_slice())
            && read_limited(&profile_dir.join("cookies"), MAX_COOKIE_BYTES).as_deref()
                == Some(self.cookies.as_slice())
    }
}

pub(super) fn matching_profile_with(
    root: &Path,
    account: AccountId,
    mut authenticate: impl FnMut(&SessionCookie) -> Option<AccountId>,
) -> Option<ProfileMatch> {
    let mut matches = std::fs::read_dir(root).ok()?.filter_map(|entry| {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            return None;
        }
        let name = entry.file_name().into_string().ok()?;
        if !cordial_shell::profile::is_valid_name(&name) {
            return None;
        }
        let identity_path = entry.path().join("identity");
        let cookie_path = entry.path().join("cookies");
        let identity_before = read_limited(&identity_path, MAX_IDENTITY_BYTES)?;
        let identity: SavedIdentity = serde_json::from_slice(&identity_before).ok()?;
        if identity.schema != 1 || identity.user_id != account {
            return None;
        }
        let cookies_before = read_limited(&cookie_path, MAX_COOKIE_BYTES)?;
        let session = session_from_store(&cookies_before)?;
        if authenticate(&session) != Some(account) {
            return None;
        }
        // The client reads these files after routing. A periodic cookie flush or
        // identity save during the HTTP check makes that future read a different
        // session, so fail closed instead of launching from the stale decision.
        (read_limited(&identity_path, MAX_IDENTITY_BYTES)?.as_slice() == identity_before
            && read_limited(&cookie_path, MAX_COOKIE_BYTES)?.as_slice() == cookies_before)
            .then_some(ProfileMatch {
                name,
                identity: identity_before,
                cookies: cookies_before,
            })
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

#[cfg(test)]
pub(crate) fn snapshot_for_test(name: &str, profile_dir: &Path) -> Option<ProfileMatch> {
    Some(ProfileMatch {
        name: name.to_string(),
        identity: read_limited(&profile_dir.join("identity"), MAX_IDENTITY_BYTES)?,
        cookies: read_limited(&profile_dir.join("cookies"), MAX_COOKIE_BYTES)?,
    })
}

fn read_limited(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= usize::try_from(limit).ok()?).then_some(bytes)
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
