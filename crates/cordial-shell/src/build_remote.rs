//! The address a build's git remote points at, made safe to compile into a
//! binary and to print in a public bug report.
//!
//! Shared by `build.rs`, which stamps it, and the library, which reads it and
//! carries the tests: a build script's own `#[cfg(test)]` code is never run by
//! `cargo test`, and the one thing here that must not go wrong is a token
//! embedded in a release. `https://user:ghp_...@github.com/owner/repo` is a
//! form `git remote add` accepts and a CI job may well have, and this is what
//! keeps it out of the binary.
//!
//! Only a network address survives. A remote that is a local path (a clone of a
//! clone) names somebody's directory, usually under a home named after them,
//! and says nothing a reader could use.

/// `raw` as `https://host/owner/repo`, or `None` if it is not a network address
/// or carries nothing after the host.
///
/// Called by `build.rs`. The library compiles this file only to test it, and
/// would otherwise warn that nothing calls it.
#[allow(dead_code)]
pub fn clean(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (host, path) = if let Some((scheme, rest)) = raw.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        // Userinfo, `user` or `user:password`, is dropped whole.
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let keep_port = matches!(scheme, "https" | "http");
        let host = if keep_port { host_port } else { host_port.split(':').next()? };
        (host.to_string(), path.to_string())
    } else {
        // scp-like: `git@github.com:owner/repo.git`. A path (`/srv/x`, `./x`,
        // `../x`) has no `host:` before its first slash and is refused.
        if raw.starts_with('/') || raw.starts_with('.') || raw.starts_with('~') {
            return None;
        }
        let (left, path) = raw.split_once(':')?;
        if left.contains('/') {
            return None;
        }
        let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
        (host.to_string(), path.to_string())
    };
    let ok_host = !host.is_empty()
        && host.contains('.')
        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    if !ok_host {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    if path.is_empty() || path.contains(char::is_whitespace) || path.contains('@') {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn an_https_remote_is_kept_and_a_token_in_it_is_not() {
        assert_eq!(clean("https://github.com/luohoa97/cordial"), Some("https://github.com/luohoa97/cordial".into()));
        assert_eq!(clean("https://github.com/luohoa97/cordial.git\n"), Some("https://github.com/luohoa97/cordial".into()));
        assert_eq!(
            clean("https://x-access-token:ghp_secret@github.com/a/b.git"),
            Some("https://github.com/a/b".into())
        );
        assert_eq!(clean("https://someone@github.com/a/b/"), Some("https://github.com/a/b".into()));
        let cleaned = clean("https://user:hunter2@example.org/a/b").unwrap();
        assert!(!cleaned.contains("hunter2") && !cleaned.contains('@'), "{cleaned}");
    }

    #[test]
    fn ssh_forms_become_the_https_address() {
        assert_eq!(clean("git@github.com:a/b.git"), Some("https://github.com/a/b".into()));
        assert_eq!(clean("ssh://git@codeberg.org:2222/a/b.git"), Some("https://codeberg.org/a/b".into()));
        assert_eq!(clean("ssh://git@github.com/a/b"), Some("https://github.com/a/b".into()));
        assert_eq!(clean("git://example.org/a/b"), Some("https://example.org/a/b".into()));
    }

    /// A local path names somebody's directory and helps nobody.
    #[test]
    fn a_local_path_is_not_a_remote_worth_printing() {
        assert_eq!(clean("/home/someone/src/cordial"), None);
        assert_eq!(clean("../cordial"), None);
        assert_eq!(clean("./cordial"), None);
        assert_eq!(clean("~/cordial"), None);
        assert_eq!(clean("file:///home/someone/cordial"), None);
        assert_eq!(clean("C:/x"), None);
        assert_eq!(clean(""), None);
        assert_eq!(clean("https://github.com"), None);
        assert_eq!(clean("https://github.com/"), None);
    }
}
