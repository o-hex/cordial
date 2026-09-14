use super::*;
use std::num::NonZeroU64;

fn account(id: u64) -> AccountId {
    AccountId(NonZeroU64::new(id).unwrap())
}

#[test]
fn ticket_is_removed_without_changing_private_server_parameters() {
    // Given a desktop link whose ticket includes encoded and literal plus signs.
    let raw = "roblox-player:1+launchmode:play+gameinfo:ABC%2BDEF+GHI%3D+placelauncherurl:https%3A%2F%2Fassetgame.roblox.com%2Fgame%2FPlaceLauncher.ashx%3FplaceId%3D42%26accessCode%3Dprivate+browsertrackerid:9";
    // When extracting the credential for redemption.
    let ticket = LaunchTicket::parse(raw).unwrap();
    // Then the join retains its parameters but cannot reuse that credential.
    assert_eq!(ticket.secret, "ABC+DEF+GHI=");
    assert_eq!(ticket.join_url, "roblox-player:1+launchmode:play+placelauncherurl:https%3A%2F%2Fassetgame.roblox.com%2Fgame%2FPlaceLauncher.ashx%3FplaceId%3D42%26accessCode%3Dprivate+browsertrackerid:9");
}

#[test]
fn ticket_text_that_resembles_a_field_is_entirely_removed_from_the_join() {
    // Given a synthetic ticket containing text shaped like an unknown launch field.
    let raw = "roblox-player:1+launchmode:play+gameinfo:ABC+credential:suffix+placelauncherurl:x";
    // When extracting the credential for redemption.
    let ticket = LaunchTicket::parse(raw).unwrap();
    // Then no part of that credential remains in the ticketless launch.
    assert_eq!(
        ticket.join_url,
        "roblox-player:1+launchmode:play+placelauncherurl:x"
    );
}

#[test]
fn ambiguous_or_non_play_tickets_are_not_redeemed() {
    // Given malformed, duplicate, absent and non-player credentials.
    for raw in [
        "roblox-player:1+launchmode:play+gameinfo:a+gameinfo:b+placelauncherurl:x",
        "roblox-player:1+launchmode:play+gameinfo:+placelauncherurl:x",
        "roblox-player:1+launchmode:play+gameinfo:%00+placelauncherurl:x",
        "roblox-player:1+launchmode:play+gameinfo:%GG+placelauncherurl:x",
        "roblox-player:1+launchmode:edit+gameinfo:a+placelauncherurl:x",
        "roblox://experiences/start?placeId=42",
        "https:1+launchmode:play+gameinfo:a+placelauncherurl:x",
    ] {
        // When parsing, then no redemption can occur.
        assert!(LaunchTicket::parse(raw).is_none());
    }
}

#[test]
fn exact_saved_account_is_selected_instead_of_last_used_profile() {
    // Given two independently signed-in profiles.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "last-used", 12);
    save_profile(root.path(), "browser", 34);
    let (endpoint, server) = authenticated_server("SESSION-34", Some(34));
    // When identifying the browser's account.
    let selected = profile::matching_profile_with(root.path(), account(34), |cookie| {
        transport::authenticated_at(cookie, &endpoint)
    });
    // Then only its profile is returned.
    assert_eq!(selected.as_ref().map(ProfileMatch::name), Some("browser"));
    server.join().unwrap();
}

#[test]
fn stale_saved_session_cannot_match_newer_identity() {
    // Given a profile whose identity was saved for account B before its account A
    // cookie was replaced by the periodic cookie flush.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "browser", 34);
    std::fs::write(
        root.path().join("browser/cookies"),
        "# cordial cookie store v1 -- a live Roblox session. Treat it as a password.\n\
.roblox.com\t.ROBLOSECURITY=SESSION-A; RBXEventTrackerV2=tracking\n",
    )
    .unwrap();
    let (endpoint, server) = authenticated_server("SESSION-A", Some(12));
    // When the saved session is checked over HTTP, then its authenticated account
    // rather than the newer identity metadata controls automatic launch.
    let selected = profile::matching_profile_with(root.path(), account(34), |cookie| {
        transport::authenticated_at(cookie, &endpoint)
    });
    assert!(selected.is_none());
    server.join().unwrap();
}

#[test]
fn duplicate_saved_accounts_require_manual_choice() {
    // Given two profiles for the same account.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "first", 34);
    save_profile(root.path(), "second", 34);
    // When matching, then neither directory wins by ordering.
    assert!(
        profile::matching_profile_with(root.path(), account(34), |_| Some(account(34))).is_none()
    );
}

#[test]
fn missing_session_or_unknown_identity_schema_cannot_match() {
    // Given an identity without cookies and a future schema with cookies.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "signed-out", 34);
    std::fs::remove_file(root.path().join("signed-out/cookies")).unwrap();
    save_profile(root.path(), "future", 34);
    std::fs::write(
        root.path().join("future/identity"),
        r#"{"schema":2,"userId":34}"#,
    )
    .unwrap();
    // When matching, then stale or unreadable state cannot select a profile.
    assert!(
        profile::matching_profile_with(root.path(), account(34), |_| Some(account(34))).is_none()
    );
}

#[test]
fn session_changed_during_validation_cannot_launch_from_stale_result() {
    // Given a matching profile whose periodic cookie save runs during validation.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "browser", 34);
    let cookies = root.path().join("browser/cookies");
    // When the original session authenticates but is replaced before selection.
    let selected = profile::matching_profile_with(root.path(), account(34), |_| {
        std::fs::write(
            &cookies,
            "# cordial cookie store v1 -- a live Roblox session. Treat it as a password.\n\
.roblox.com\t.ROBLOSECURITY=SESSION-CHANGED\n",
        )
        .unwrap();
        Some(account(34))
    });
    // Then the stale authenticated result cannot authorise the changed profile.
    assert!(selected.is_none());
}

#[test]
fn matched_profile_snapshot_rejects_identity_byte_changes_after_lookup() {
    // Given snapshot evidence for a matching profile.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "browser", 34);
    let dir = root.path().join("browser");
    let matched = profile::snapshot_for_test("browser", &dir).unwrap();

    // When equivalent identity data is rewritten with different bytes.
    std::fs::write(dir.join("identity"), r#"{ "schema": 1, "userId": 34 }"#).unwrap();

    // Then the pre-lock snapshot no longer authorises automatic launch.
    assert!(!matched.still_matches("browser", &dir));
}

#[test]
fn matched_profile_snapshot_rejects_cookie_byte_changes_after_lookup() {
    // Given snapshot evidence for a matching profile.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "browser", 34);
    let dir = root.path().join("browser");
    let matched = profile::snapshot_for_test("browser", &dir).unwrap();

    // When the cookie store changes after lookup, even without changing its session.
    let cookies = std::fs::read(dir.join("cookies")).unwrap();
    let mut changed = cookies;
    changed.push(b'\n');
    std::fs::write(dir.join("cookies"), changed).unwrap();

    // Then the pre-lock snapshot no longer authorises automatic launch.
    assert!(!matched.still_matches("browser", &dir));
}

#[test]
fn failed_saved_session_validation_requires_manual_choice() {
    // Given a candidate profile whose saved session is refused by Roblox.
    let root = tempfile::tempdir().unwrap();
    save_profile(root.path(), "browser", 34);
    let (endpoint, server) = authenticated_server("SESSION-34", None);
    // When matching through the authenticated-user endpoint.
    let selected = profile::matching_profile_with(root.path(), account(34), |cookie| {
        transport::authenticated_at(cookie, &endpoint)
    });
    // Then transport failure cannot authorise automatic launch.
    assert!(selected.is_none());
    server.join().unwrap();
}

fn save_profile(root: &Path, name: &str, id: u64) {
    let dir = root.join(name);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("identity"),
        format!(r#"{{"schema":1,"userId":{id}}}"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("cookies"),
        format!(
            "# cordial cookie store v1 -- a live Roblox session. Treat it as a password.\n\
.roblox.com\t.ROBLOSECURITY=SESSION-{id}; RBXEventTrackerV2=tracking\n"
        ),
    )
    .unwrap();
}

fn authenticated_server(
    expected_session: &'static str,
    account_id: Option<u64>,
) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/authenticated", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(!line.is_empty(), "expected complete HTTP headers");
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.starts_with("GET /authenticated "));
        let cookie = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("cookie").then_some(value.trim())
        });
        assert_eq!(
            cookie,
            Some(format!(".ROBLOSECURITY={expected_session}").as_str())
        );
        match account_id {
            Some(id) => {
                let body = format!(r#"{{"id":{id}}}"#);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            None => socket
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .unwrap(),
        }
    });
    (endpoint, server)
}
