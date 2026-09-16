//! Separate the one-use credential from the join that must survive its consumption.

pub struct LaunchTicket {
    pub(super) secret: String,
    pub join_url: String,
}

impl LaunchTicket {
    pub fn parse(raw: &str) -> Option<Self> {
        crate::deep_link::accept(raw).ok()?;
        let (scheme, payload) = raw.split_once(':')?;
        if !scheme.eq_ignore_ascii_case("roblox-player") {
            return None;
        }
        let (version, _) = payload.trim_start_matches('/').split_once('+')?;
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }

        // The accepted ticket bytes include literal pluses and colons. Only
        // fields the desktop player protocol actually defines can delimit it;
        // accepting any identifier here left credential suffixes in the join.
        let fields: Vec<_> = raw
            .match_indices('+')
            .filter_map(|(offset, _)| {
                let (key, _) = raw[offset + 1..].split_once(':')?;
                matches!(
                    key,
                    "launchmode"
                        | "gameinfo"
                        | "placelauncherurl"
                        | "launchtime"
                        | "browsertrackerid"
                        | "robloxLocale"
                        | "gameLocale"
                        | "channel"
                        | "LaunchExp"
                )
                .then_some((offset, key))
            })
            .collect();
        let tickets: Vec<_> = fields
            .iter()
            .enumerate()
            .filter(|(_, (_, key))| *key == "gameinfo")
            .collect();
        let [(index, (start, _))] = tickets.as_slice() else {
            return None;
        };
        let end = fields
            .get(index + 1)
            .map_or(raw.len(), |(offset, _)| *offset);
        let modes: Vec<_> = fields
            .iter()
            .enumerate()
            .filter(|(_, (_, key))| *key == "launchmode")
            .collect();
        let [(mode_index, (mode_start, _))] = modes.as_slice() else {
            return None;
        };
        let mode_end = fields
            .get(mode_index + 1)
            .map_or(raw.len(), |(offset, _)| *offset);
        if &raw[mode_start + "+launchmode:".len()..mode_end] != "play" {
            return None;
        }
        let encoded = &raw[start + "+gameinfo:".len()..end];
        if encoded
            .split('%')
            .skip(1)
            .any(|part| part.len() < 2 || !part.as_bytes()[..2].iter().all(u8::is_ascii_hexdigit))
        {
            return None;
        }
        let secret = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .ok()?
            .into_owned();
        if secret.is_empty() || !secret.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return None;
        }
        Some(Self {
            secret,
            join_url: format!("{}{}", &raw[..*start], &raw[end..]),
        })
    }
}
