use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

#[test]
fn redeemed_session_identifies_account_without_forwarding_ticket_again() {
    // Given a real HTTP peer accepting one redemption and one identity lookup.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for step in 0..2 {
            let mut socket = accept(&listener);
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
                headers.push_str(&line);
            }
            if step == 0 {
                assert!(headers.starts_with("POST /redeem "));
                assert!(headers
                    .to_lowercase()
                    .contains("rbxauthenticationnegotiation: 1"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(payload["authenticationTicket"], "FAKE-TICKET");
                socket.write_all(b"HTTP/1.1 200 OK\r\nSet-Cookie: other=ignored; Path=/\r\nSet-Cookie: .ROBLOSECURITY=FAKE-SESSION; Domain=.roblox.com; Secure; HttpOnly\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            } else {
                assert!(headers.starts_with("GET /authenticated "));
                assert!(headers.contains(".ROBLOSECURITY=FAKE-SESSION"));
                assert!(!headers.contains("FAKE-TICKET"));
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n{\"id\":34}").unwrap();
            }
        }
    });
    let endpoints = Endpoints {
        redeem: &format!("{base}/redeem"),
        authenticated: &format!("{base}/authenticated"),
    };
    let ticket = LaunchTicket {
        secret: "FAKE-TICKET".into(),
        join_url: "unused".into(),
    };
    // When using the production HTTP adapter against that peer.
    let result = lookup_at(&ticket, &endpoints);
    // Then the authenticated account, not ticket contents, determines identity.
    assert_eq!(result.map(|id| id.0.get()), Some(34));
    server.join().unwrap();
}

#[test]
fn refused_redemption_does_not_retry_or_request_identity() {
    // Given one rejection with a valid-looking cookie that must not be trusted.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut socket = accept(&listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket.write_all(b"HTTP/1.1 403 Forbidden\r\nSet-Cookie: .ROBLOSECURITY=FAKE\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    });
    let endpoints = Endpoints {
        redeem: &base,
        authenticated: "http://127.0.0.1:1/must-not-request",
    };
    let ticket = LaunchTicket {
        secret: "FAKE".into(),
        join_url: "unused".into(),
    };
    // When redeeming, then no account is claimed from the error response.
    assert!(lookup_at(&ticket, &endpoints).is_none());
    server.join().unwrap();
}

#[test]
fn refused_saved_session_cannot_claim_an_account() {
    // Given the authenticated-user endpoint rejects a structurally valid saved session.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut socket = accept(&listener);
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .unwrap();
    });
    let cookie = SessionCookie::from_value("FAKE-SAVED-SESSION").unwrap();
    // When validating it, then no identity is inferred from the cookie's shape.
    assert!(authenticated_at(&cookie, &endpoint).is_none());
    server.join().unwrap();
}

fn accept(listener: &TcpListener) -> std::net::TcpStream {
    // A regression that skips a request must fail instead of leaving join()
    // blocked forever. The deadline is a liveness guard, not synchronisation.
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((socket, _)) => return socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "expected HTTP request"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}
