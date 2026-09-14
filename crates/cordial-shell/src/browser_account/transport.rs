use super::LaunchTicket;
use std::time::Duration;

struct Endpoints<'a> {
    redeem: &'a str,
    authenticated: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct AccountId(pub std::num::NonZeroU64);

#[derive(PartialEq, Eq)]
pub(super) struct SessionCookie(String);

impl SessionCookie {
    pub(super) fn from_value(value: &str) -> Option<Self> {
        (!value.is_empty()
            && value
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && !b"\",;\\".contains(&byte)))
        .then(|| Self(format!(".ROBLOSECURITY={value}")))
    }

    fn from_pair(pair: &str) -> Option<Self> {
        Self::from_value(pair.strip_prefix(".ROBLOSECURITY=")?)
    }
}

pub(super) fn lookup(ticket: &LaunchTicket) -> Option<AccountId> {
    lookup_at(
        ticket,
        &Endpoints {
            redeem: "https://auth.roblox.com/v1/authentication-ticket/redeem",
            authenticated: "https://users.roblox.com/v1/users/authenticated",
        },
    )
}

fn lookup_at(ticket: &LaunchTicket, endpoints: &Endpoints<'_>) -> Option<AccountId> {
    let agent = agent();
    let body =
        serde_json::to_vec(&serde_json::json!({"authenticationTicket": ticket.secret})).ok()?;
    let response = agent
        .post(endpoints.redeem)
        .header("RBXAuthenticationNegotiation", "1")
        .header("Content-Type", "application/json")
        .send(body.as_slice())
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut sessions = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|header| {
            let pair = header.to_str().ok()?.split(';').next()?.trim();
            SessionCookie::from_pair(pair)
        });
    let cookie = sessions.next()?;
    if sessions.next().is_some() {
        return None;
    }
    authenticated_with_agent(&agent, &cookie, endpoints.authenticated)
}

pub(super) fn authenticated(cookie: &SessionCookie) -> Option<AccountId> {
    authenticated_at(cookie, "https://users.roblox.com/v1/users/authenticated")
}

pub(super) fn authenticated_at(cookie: &SessionCookie, endpoint: &str) -> Option<AccountId> {
    authenticated_with_agent(&agent(), cookie, endpoint)
}

fn authenticated_with_agent(
    agent: &ureq::Agent,
    cookie: &SessionCookie,
    endpoint: &str,
) -> Option<AccountId> {
    // Redirects are disabled: a response may not choose where this credential
    // is sent. Neither the cookie nor the response body is logged.
    let mut response = agent
        .get(endpoint)
        .header("Cookie", &cookie.0)
        .call()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct AuthenticatedUser {
        id: AccountId,
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(16 * 1024)
        .read_to_string()
        .ok()?;
    serde_json::from_str::<AuthenticatedUser>(&text)
        .ok()
        .map(|user| user.id)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .build()
        .into()
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
