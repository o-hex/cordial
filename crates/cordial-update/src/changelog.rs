//! Roblox's release notes, so the header-bar button has something to show.
//!
//! The up-to-date state of that button is not a dead end — it shows the current
//! version and its changelog — so this is fetched whether or not there is an
//! update, and it is the half of the check that works for the Android build
//! today. See [`version`](crate::version) for why the other half does not.
//!
//! ## Where these come from, measured 2026-08-02
//!
//! Roblox posts them to the DevForum, which is Discourse, which serves every
//! page as JSON if you ask for `.json`:
//!
//! ```text
//! $ curl -sSL -w '%{http_code} %{url_effective}\n' \
//!     https://devforum.roblox.com/c/updates/release-notes.json
//! 200 https://devforum.roblox.com/c/updates/release-notes/62.json
//! → topic_list.topics[]: {"id":4763851,"slug":"release-notes-for-732",
//!                         "title":"Release Notes for 732",
//!                         "created_at":"2026-07-29T18:44:52.923Z"}
//! ```
//!
//! The slug URL is used rather than the numeric `62` it redirects to: a
//! category id is a database key and a slug is the thing Roblox would keep if
//! they reorganised. ureq follows the redirect.
//!
//! **A topic must be fetched by slug *and* id.** `/t/4763851.json` answers
//! **403**; `/t/release-notes-for-732/4763851.json` answers 200. That is not a
//! permission problem to work around, it is Discourse's routing, and it is
//! written down here because a 403 from a public forum reads like something
//! else entirely.
//!
//! ## The title carries the version
//!
//! Every entry is titled `Release Notes for NNN`, and `NNN` is the engine major
//! — the `732` in `0.732.23.7321040`, and the `Version=732` the client logs
//! about itself. That correspondence is what lets this be useful without the
//! version endpoint: the newest release-notes major is the newest engine Roblox
//! has shipped. Entries whose title carries no number — the category's own
//! "About the Release Notes category" pinned post — are dropped rather than
//! shown, since they are not releases.

use crate::http;
use crate::Unreachable;

/// The release-notes category, by slug.
pub const CATEGORY: &str = "https://devforum.roblox.com/c/updates/release-notes.json";

/// One published set of release notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The engine major this announces: `732`.
    pub major: u32,
    pub title: String,
    pub id: u64,
    pub slug: String,
    /// ISO 8601, as Discourse gives it.
    pub created_at: String,
}

impl Release {
    /// The URL its body is fetched from. Slug and id both, because id alone is
    /// a 403.
    pub fn url(&self) -> String {
        format!("https://devforum.roblox.com/t/{}/{}.json", self.slug, self.id)
    }

    /// The URL to open in a browser, for the "read the rest" the popover will
    /// want. Same route without `.json`.
    pub fn web_url(&self) -> String {
        format!("https://devforum.roblox.com/t/{}/{}", self.slug, self.id)
    }
}

/// The body of one set of release notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    pub title: String,
    pub created_at: String,
    /// Discourse's rendered HTML, as posted.
    pub html: String,
}

impl Notes {
    /// A plain-text rendering, for a label.
    ///
    /// Crude on purpose and not a sanitiser: it drops tags and unescapes the
    /// five XML entities, which is enough for a summary line and is not enough
    /// for anything that renders markup. Nothing in Cordial should feed this to
    /// a widget that interprets what comes out — the popover shows text, and
    /// [`Release::web_url`] is the way to read the whole thing.
    pub fn text(&self) -> String {
        strip_tags(&self.html)
    }
}

/// Every release in the category listing, newest first.
pub fn releases() -> Result<Vec<Release>, Unreachable> {
    let value = http::get_json(CATEGORY)?;
    parse_releases(&value).map_err(|why| Unreachable::Malformed { url: CATEGORY.into(), why })
}

/// The newest one Roblox has published.
pub fn latest() -> Result<Release, Unreachable> {
    releases()?.into_iter().max_by_key(|r| r.major).ok_or(Unreachable::Malformed {
        url: CATEGORY.into(),
        why: "the release-notes category listed no topic titled \"Release Notes for <number>\""
            .into(),
    })
}

/// Fetch one release's body.
pub fn notes(release: &Release) -> Result<Notes, Unreachable> {
    let url = release.url();
    let value = http::get_json(&url)?;
    parse_notes(&value).map_err(|why| Unreachable::Malformed { url, why })
}

/// Read a Discourse category listing.
///
/// Split from the request so the shape stays pinned to a body that was
/// observed. Sorted newest-major first rather than trusting the listing's own
/// order, which is Discourse's to change and is by activity rather than by
/// version.
pub fn parse_releases(value: &serde_json::Value) -> Result<Vec<Release>, String> {
    let topics = value
        .get("topic_list")
        .and_then(|l| l.get("topics"))
        .and_then(|t| t.as_array())
        .ok_or("no topic_list.topics array")?;

    let mut out: Vec<Release> = topics
        .iter()
        .filter_map(|t| {
            let title = t.get("title")?.as_str()?.to_string();
            let major = major_in_title(&title)?;
            Some(Release {
                major,
                title,
                id: t.get("id")?.as_u64()?,
                slug: t.get("slug")?.as_str()?.to_string(),
                created_at: t
                    .get("created_at")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();
    if out.is_empty() {
        return Err(format!(
            "{} topics were listed and none was titled \"Release Notes for <number>\"",
            topics.len()
        ));
    }
    out.sort_by(|a, b| b.major.cmp(&a.major));
    Ok(out)
}

/// Read a Discourse topic.
pub fn parse_notes(value: &serde_json::Value) -> Result<Notes, String> {
    let html = value
        .get("post_stream")
        .and_then(|s| s.get("posts"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|p| p.get("cooked"))
        .and_then(|c| c.as_str())
        .ok_or("no post_stream.posts[0].cooked")?;
    Ok(Notes {
        title: value.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        created_at: value
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        html: html.to_string(),
    })
}

/// `Release Notes for 732` -> `732`.
///
/// The last run of digits in the title, rather than a prefix match on "Release
/// Notes for": the category has been titled several ways over the years and the
/// number is the part that has not moved.
fn major_in_title(title: &str) -> Option<u32> {
    title
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .next_back()
        .and_then(|s| s.parse().ok())
}

/// Tags out, the five XML entities back in.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for c in html.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    // Discourse emits a `<br>` per line, so what is left is a run of blank
    // lines where the markup was.
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the real listing fetched on 2026-08-02, keeping the pinned
    /// category post because dropping it is one of the things being tested.
    const LISTING: &str = r#"{"topic_list":{"topics":[
        {"id":60921,"slug":"about-the-release-notes-category",
         "title":"About the Release Notes category","created_at":"2019-01-01T00:00:00.000Z"},
        {"id":4754888,"slug":"release-notes-for-731",
         "title":"Release Notes for 731","created_at":"2026-07-22T18:00:00.000Z"},
        {"id":4763851,"slug":"release-notes-for-732",
         "title":"Release Notes for 732","created_at":"2026-07-29T18:44:52.923Z"}
    ]}}"#;

    #[test]
    fn the_listing_shape_is_the_one_that_was_observed() {
        let releases = parse_releases(&serde_json::from_str(LISTING).unwrap()).unwrap();
        assert_eq!(releases.len(), 2, "the category's own About post is not a release");
        assert_eq!(releases[0].major, 732);
        assert_eq!(releases[0].id, 4763851);
        assert_eq!(releases[0].slug, "release-notes-for-732");
        assert_eq!(releases[1].major, 731);
    }

    #[test]
    fn a_topic_url_carries_both_slug_and_id() {
        // `/t/<id>.json` answers 403. Measured, and not obvious enough to be
        // left to whoever next edits this line.
        let releases = parse_releases(&serde_json::from_str(LISTING).unwrap()).unwrap();
        assert_eq!(
            releases[0].url(),
            "https://devforum.roblox.com/t/release-notes-for-732/4763851.json"
        );
        assert!(!releases[0].web_url().ends_with(".json"));
    }

    #[test]
    fn a_listing_with_nothing_recognisable_is_a_named_failure() {
        // Not an empty list. "Roblox retitled their release notes" and "there
        // are no release notes" have to look different, or the button says
        // "up to date" forever.
        let e = parse_releases(&serde_json::json!({"topic_list":{"topics":[
            {"id":1,"slug":"x","title":"Something else entirely"}
        ]}}))
        .unwrap_err();
        assert!(e.contains("Release Notes"), "{e}");
    }

    #[test]
    fn a_reshaped_listing_is_refused_rather_than_read_as_empty() {
        assert!(parse_releases(&serde_json::json!({"topics":[]})).is_err());
    }

    #[test]
    fn the_topic_shape_is_the_one_that_was_observed() {
        let topic = serde_json::json!({
            "title": "Release Notes for 732",
            "created_at": "2026-07-29T18:44:52.923Z",
            "post_stream": {"posts": [{"cooked": "<p>Hi all,<br>\nPleased to announce that 732 has landed. Enjoy!</p>"}]}
        });
        let notes = parse_notes(&topic).unwrap();
        assert_eq!(notes.title, "Release Notes for 732");
        assert!(notes.html.starts_with("<p>"));
        assert_eq!(notes.text(), "Hi all,\nPleased to announce that 732 has landed. Enjoy!");
    }

    #[test]
    fn the_plain_rendering_drops_markup_and_restores_entities() {
        let notes = Notes {
            title: String::new(),
            created_at: String::new(),
            html: "<p>Fixed <code>a &lt; b &amp;&amp; c</code></p>".into(),
        };
        assert_eq!(notes.text(), "Fixed a < b && c");
    }

    #[test]
    fn the_major_is_the_number_in_the_title() {
        assert_eq!(major_in_title("Release Notes for 732"), Some(732));
        assert_eq!(major_in_title("Release Notes for Release 645"), Some(645));
        assert_eq!(major_in_title("About the Release Notes category"), None);
    }
}
