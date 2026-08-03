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

// ------------------------------------------------- the Creator Hub docs table
//
// The DevForum post announces a release; it does not list what changed. Its body
// is five lines of "732 has landed, enjoy" and a set of links, one of which goes
// to the page below — so a window showing the post was showing the covering note
// and calling it a changelog.
//
// `https://create.roblox.com/docs/release-notes/release-notes-<major>` is the
// actual list, as a table of note/status. **The rendered table is empty in the
// HTML**: every `<td>` in the note column contains one empty `<div>`, because
// the text is written in by script after load. Scraping the table would have
// produced thirty blank rows and looked like Roblox had published nothing.
//
// What does carry it is the `__NEXT_DATA__` blob the page ships for hydration,
// where each entry is `{"ReleaseNotesText", "ReleaseNotesType", "Status"}`.
// Measured on 732, 2026-08-03: thirty entries, twelve Improvements and eighteen
// Fixes, each Live or Pending. That is a document Roblox publishes for people to
// read, fetched at run time and shown — nothing is vendored into this repository.

/// The Creator Hub release-notes page for one engine major.
pub fn docs_url(major: u32) -> String {
    format!("https://create.roblox.com/docs/release-notes/release-notes-{major}")
}

/// One row of the release-notes table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub text: String,
    /// `Improvements` or `Fixes`, as Roblox groups them.
    pub kind: String,
    /// `Live` or `Pending`.
    pub status: String,
}

/// Fetch and read the table for one major.
pub fn docs_notes(major: u32) -> Result<Vec<Entry>, Unreachable> {
    let url = docs_url(major);
    let html = http::get_text(&url)?;
    parse_docs_notes(&html).map_err(|why| Unreachable::Malformed { url, why })
}

/// Pull the entries out of the page's hydration blob.
///
/// Split from the request so the shape stays pinned to a page that was observed,
/// the same arrangement as [`parse_releases`].
pub fn parse_docs_notes(html: &str) -> Result<Vec<Entry>, String> {
    const OPEN: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let start = html.find(OPEN).ok_or("the page carried no __NEXT_DATA__ block")?;
    let rest = &html[start + OPEN.len()..];
    let end = rest.find("</script>").ok_or("__NEXT_DATA__ was not closed")?;
    let value: serde_json::Value =
        serde_json::from_str(&rest[..end]).map_err(|e| format!("__NEXT_DATA__ is not JSON: {e}"))?;

    let mut out = Vec::new();
    collect_entries(&value, &mut out);
    if out.is_empty() {
        return Err("__NEXT_DATA__ carried no ReleaseNotesText entries".into());
    }
    Ok(out)
}

/// Walk the blob for anything shaped like an entry.
///
/// By shape rather than by path: the route to it runs through Next's own
/// `props`/`pageProps` nesting, which is a framework's private arrangement and
/// not something Roblox has promised to keep. The field names are the part that
/// would have to change for this to break, and if they do it fails loudly in
/// [`parse_docs_notes`] rather than showing an empty changelog.
fn collect_entries(value: &serde_json::Value, out: &mut Vec<Entry>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("ReleaseNotesText").and_then(|v| v.as_str()) {
                let text = markdown_to_pango(text);
                if !text.is_empty() {
                    out.push(Entry {
                        text,
                        kind: map
                            .get("ReleaseNotesType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Notes")
                            .to_string(),
                        status: map
                            .get("Status")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
            }
            for v in map.values() {
                collect_entries(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_entries(v, out);
            }
        }
        _ => {}
    }
}

/// One note's text, as Pango markup for the label that shows it.
///
/// These notes are markdown — Roblox writes `**bold**` in them, three of the
/// thirty on 732 — so they go through a real parser rather than the hand-rolled
/// marker-stripping this used to do. That version paired `**` by searching for
/// the next one, which happily matched the stars in `2 ** 8` against an
/// unrelated marker later in the line and deleted everything between them.
///
/// The output is Pango markup because a `GtkLabel` renders it natively, so bold
/// arrives as bold rather than as words with the stars filed off. Everything
/// interpolated is escaped first: these strings come off the network, and a note
/// containing a stray `<` would otherwise turn the label into a parse error and
/// blank the changelog.
///
/// Code spans carry a second syntax that is Roblox's rather than markdown's:
/// `Class.Decal.Rotation` is a link to that property and
/// `Class.Decal.Rotation|Rotation` is the same link with its label after the
/// pipe. [`display_of`] reduces those, and it runs only inside a code span --
/// which is also what keeps the literal in "replaced with `**error-type**`"
/// intact, since the parser hands that back as code rather than as emphasis.
pub fn markdown_to_pango(text: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut out = String::with_capacity(text.len());
    for event in Parser::new_ext(text, Options::empty()) {
        match event {
            Event::Text(t) => out.push_str(&escape(&t)),
            Event::Code(c) => {
                out.push_str("<tt>");
                out.push_str(&escape(&display_of(&c)));
                out.push_str("</tt>");
            }
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),
            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),
            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),
            // A note is one paragraph. Anything block-level collapses to a
            // space rather than a newline, because these are bullet items in a
            // list this crate is building itself.
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(TagEnd::Paragraph) => out.push(' '),
            // Raw HTML in a note is shown as the characters it is made of, not
            // dropped and not interpreted. Dropping it silently loses whatever
            // the note said — a bullet mentioning a `<script>` tag came out
            // with a hole in the middle of the sentence — and interpreting it
            // would let a fetched string choose the markup in a widget.
            Event::Html(h) | Event::InlineHtml(h) => out.push_str(&escape(&h)),
            // Links keep their words and lose their destination: the window has
            // no browser in it and a bare URL in a bullet is noise.
            _ => {}
        }
    }
    out.trim().to_string()
}

/// The three characters Pango's parser treats as markup.
///
/// `&` first, or the escapes introduced by the other two get escaped again.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `Class.Decal.Rotation|Rotation` -> `Rotation`; `Class.Decal` -> `Decal`.
fn display_of(reference: &str) -> String {
    if let Some((_, label)) = reference.split_once('|') {
        return label.to_string();
    }
    // Only the documented prefixes are unwrapped. A backticked `true` or a bare
    // code sample has no dots to strip and must survive untouched.
    const PREFIXES: [&str; 6] =
        ["Class.", "Enum.", "Datatype.", "Global.", "Library.", "Property."];
    if PREFIXES.iter().any(|p| reference.starts_with(p)) {
        if let Some(last) = reference.rsplit('.').next() {
            return last.to_string();
        }
    }
    reference.to_string()
}

/// The table as Pango markup, grouped the way Roblox groups it.
///
/// Improvements first and Fixes after, each in the order published, with
/// `Pending` marked because the distinction is the useful part: a Live note
/// describes the build you can be running now and a Pending one does not.
///
/// **Markup, not text.** `Entry::text` is already Pango markup from
/// [`markdown_to_pango`], so everything this adds around it — the headings, the
/// status suffix — is escaped on the way in and the whole string has to go to a
/// label with `use_markup` set. Handing this to one without it shows the tags.
pub fn render_entries(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut kinds: Vec<&str> = Vec::new();
    for e in entries {
        if !kinds.contains(&e.kind.as_str()) {
            kinds.push(&e.kind);
        }
    }
    // Improvements before Fixes when both are present, and anything Roblox adds
    // later after them in the order it arrived, rather than dropped.
    kinds.sort_by_key(|k| match *k {
        "Improvements" => 0,
        "Fixes" => 1,
        _ => 2,
    });

    for kind in kinds {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("<b>{}</b>\n", escape(kind)));
        for e in entries.iter().filter(|e| e.kind == kind) {
            if e.status.eq_ignore_ascii_case("Live") || e.status.is_empty() {
                out.push_str(&format!("  • {}\n", e.text));
            } else {
                out.push_str(&format!(
                    "  • {} <i>({})</i>\n",
                    e.text,
                    escape(&e.status)
                ));
            }
        }
    }
    out.trim_end().to_string()
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

    /// The shape observed on the 732 page, 2026-08-03, nested the way Next puts
    /// it and trimmed to four of the thirty entries. The wrapper levels are kept
    /// deliberately: walking by shape rather than by path is the thing being
    /// tested, because that nesting is the framework's to rearrange.
    const DOCS: &str = r#"<!doctype html><html><body><table><tr><td><div></div></td></tr></table>
        <script id="__NEXT_DATA__" type="application/json">
        {"props":{"pageProps":{"content":{"releaseNotes":[
          {"ReleaseNotesText":"Adds the `Class.Decal.Rotation|Rotation` property to `Class.Texture` and `Class.Decal` instances.","ReleaseNotesType":"Improvements","Status":"Live"},
          {"ReleaseNotesText":"Adds `Enum.PreferredInput.MicroGamepad` for gamepads.","ReleaseNotesType":"Improvements","Status":"Pending"},
          {"ReleaseNotesText":"Fixed a crash when `true` was passed twice.","ReleaseNotesType":"Fixes","Status":"Live"},
          {"ReleaseNotesText":"Fixed audio in some scenes.","ReleaseNotesType":"Fixes","Status":"Pending"}
        ]}}}}
        </script></body></html>"#;

    #[test]
    fn the_notes_come_out_of_the_hydration_blob_not_the_rendered_table() {
        // The table in `DOCS` is the real one's shape: a `<td>` holding one empty
        // `<div>`, because the text is written in by script after load. Scraping
        // it produces blank rows that look like Roblox published nothing, which
        // is why this reads `__NEXT_DATA__` instead.
        let entries = parse_docs_notes(DOCS).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].kind, "Improvements");
        assert_eq!(entries[0].status, "Live");
        assert_eq!(entries[3].kind, "Fixes");
    }

    #[test]
    fn the_rendering_groups_improvements_first_and_marks_what_is_not_live_yet() {
        let out = render_entries(&parse_docs_notes(DOCS).unwrap());
        let improvements = out.find("Improvements").expect("no Improvements heading");
        let fixes = out.find("Fixes").expect("no Fixes heading");
        assert!(improvements < fixes, "{out}");
        // Pending is the useful distinction: a Live note describes the build you
        // can be running now and a Pending one does not. Live is unmarked
        // because marking the ordinary case is noise on every line.
        assert!(out.contains("Adds <tt>MicroGamepad</tt> for gamepads. <i>(Pending)</i>"), "{out}");
        assert!(!out.contains("(Live)"), "{out}");
        assert!(out.starts_with("<b>Improvements</b>"), "{out}");
    }

    #[test]
    fn markdown_becomes_pango_markup_and_a_code_span_keeps_its_stars() {
        // Both of these are real 732 notes, shortened. The second is why a real
        // parser replaced the hand-rolled marker stripping: `**error-type**` is
        // the literal string the bug produced, and the parser hands it back as
        // code rather than as emphasis, so it survives whole.
        assert_eq!(
            markdown_to_pango("Adds a **Use Bounding Boxes** setting for the draggers."),
            "Adds a <b>Use Bounding Boxes</b> setting for the draggers."
        );
        assert_eq!(
            markdown_to_pango("Fixes a bug where types were replaced with `**error-type**`."),
            "Fixes a bug where types were replaced with <tt>**error-type**</tt>."
        );
        // The case the old pairing rule got wrong: it searched for the next
        // `**`, matched the stars in an exponent against an unrelated marker
        // later in the line, and deleted everything between them.
        assert_eq!(markdown_to_pango("A 2 ** 8 shift"), "A 2 ** 8 shift");
    }

    #[test]
    fn a_note_off_the_network_cannot_break_the_label_it_is_drawn_into() {
        // These strings come off the network and land in a widget that parses
        // markup. One stray `<` in a note would otherwise make Pango reject the
        // whole block, which blanks the changelog rather than mangling one line.
        assert_eq!(
            markdown_to_pango("Fixes `a < b` when a & b are <script> tags"),
            "Fixes <tt>a &lt; b</tt> when a &amp; b are &lt;script&gt; tags"
        );
        // An entity in the source round-trips: markdown decodes `&lt;` to a
        // literal `<`, and the escape puts it back, so the label renders the
        // `<` the note meant rather than the six characters that spelled it.
        assert_eq!(markdown_to_pango("a &lt; b"), "a &lt; b");
        // `&` is escaped before the angle brackets, or the escapes those two
        // introduce get escaped a second time and show as `&amp;lt;`.
        assert_eq!(escape("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn the_docs_cross_reference_syntax_survives_the_markdown_parser() {
        let entries = parse_docs_notes(DOCS).unwrap();
        // The reference reduction runs inside code spans, so it still applies
        // once markdown owns the parse -- and what comes out is a `<tt>` run
        // rather than bare words.
        assert_eq!(
            entries[0].text,
            "Adds the <tt>Rotation</tt> property to <tt>Texture</tt> and <tt>Decal</tt> instances."
        );
        assert_eq!(entries[1].text, "Adds <tt>MicroGamepad</tt> for gamepads.");
        assert_eq!(entries[2].text, "Fixed a crash when <tt>true</tt> was passed twice.");
    }

    #[test]
    fn a_page_that_changed_shape_fails_loudly_rather_than_showing_an_empty_changelog() {
        // The failure that matters. If Roblox renames the fields or drops the
        // blob, an empty list reads exactly like a release with nothing in it,
        // and the window would quietly claim Roblox published no changes.
        assert!(parse_docs_notes("<html><body>no blob here</body></html>").is_err());
        let empty = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{}}</script>"#;
        assert!(parse_docs_notes(empty).is_err());
    }

    /// The live page, on request only.
    ///
    /// `cargo test -p cordial-update -- --ignored --nocapture` prints what the
    /// window would show. Ignored by default because it is a network request and
    /// the rest of this file is pinned to captured shapes -- but the whole point
    /// of this module is a page somebody else controls, and a fixture cannot
    /// tell you the day they rename a field.
    #[test]
    #[ignore = "fetches create.roblox.com"]
    fn the_real_page_still_parses() {
        let release = latest().expect("the DevForum listing");
        let entries = docs_notes(release.major).expect("the docs table");
        println!("engine {} -- {} entries", release.major, entries.len());
        println!("{}", render_entries(&entries));
        assert!(!entries.is_empty());
        // Every entry has to survive the parse with something in it: an entry
        // that renders empty is what a changed field name looks like from here.
        assert!(entries.iter().all(|e| !e.text.trim().is_empty()));
    }

    #[test]
    fn the_docs_url_is_built_from_the_major_the_devforum_gave() {
        assert_eq!(
            docs_url(732),
            "https://create.roblox.com/docs/release-notes/release-notes-732"
        );
    }

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
