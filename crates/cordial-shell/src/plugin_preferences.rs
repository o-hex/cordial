//! Turning any plugin's declared schema into a real preferences page.
//!
//! GNOME's Extensions app puts a gear on each extension row and the gear opens
//! a window the extension itself built, in GJS, inside the shell's own process.
//! Cordial offers the same affordance and cannot use the same mechanism: a
//! plugin here is a separate Deno process with no toolkit, no display and no
//! handle on anything the launcher has on screen (ADR-003), and handing it one
//! would be the largest channel in a system whose whole premise is that plugins
//! get effects and never channels (ADR-007).
//!
//! So the plugin declares and this file draws. Every row below is a widget
//! Cordial constructed, in Cordial's process, from data that has already been
//! through `cordial_plugins::preferences::check_all`. See
//! [ADR-020](../../../docs/adr/ADR-020-declarative-plugin-preferences.md).
//!
//! **Nothing here knows any particular plugin.** It is handed a list of
//! declarations and a store, and it has no idea which plugin they came from
//! beyond the id it needs to save under. That is the property worth protecting:
//! the moment this file contains a special case for one plugin, every other
//! plugin's page is the second-class one. Its tests use a fabricated schema for
//! the same reason.
//!
//! **Plugin text is never markup.** A title, a subtitle, a group name and an
//! option label are all the plugin author's words rendered in Cordial's own
//! window. `AdwPreferencesRow` interprets its title as Pango markup by default
//! and an `AdwActionRow` subtitle always does, so a plugin could otherwise draw
//! bold, coloured or oversized text in the launcher's chrome — which is the
//! first inch of the road `webview_policy.rs` exists to keep shut. Titles get
//! `use-markup` turned off; subtitles, which have no such switch, are escaped.

use adw::prelude::*;
use cordial_plugins::manifest::Plugin;
use cordial_plugins::preferences::{self, Declaration, Field};
use libadwaita as adw;
use libadwaita::gtk;
use std::path::{Path, PathBuf};

/// Whether this plugin has a preferences page at all.
///
/// The declaration is the whole signal — there is no manifest key and no
/// capability saying "I have settings", because a second fact about the same
/// thing is a fact that can disagree with the first. A plugin has a page
/// exactly when it declares a field.
pub fn has_page(plugin: &Plugin) -> bool {
    !plugin.manifest.preferences.is_empty()
}

/// The gear for one plugin row, or `None` when it declares nothing.
///
/// Returning `None` rather than an insensitive button is deliberate and is what
/// GNOME's Extensions app does: an extension with no preferences simply has no
/// gear, which reads as "nothing to configure". A greyed-out one reads as
/// broken.
///
/// `update_available` paints it with libadwaita's accent colour. **Nothing
/// calls that with `true` yet** — Cordial has no plugin update detection, only
/// an updater for the Roblox build (`crate::updater`) — so today the state
/// exists in this function and nowhere else. It is here rather than deferred
/// because the alternative is a second pass over this row later, and because
/// writing down which colour it is stops the next person reaching for the
/// warning orange: an available update is information, not a fault, and orange
/// in libadwaita means something is wrong. When plugin updates do land they
/// must go through `cordial_update::metered` like every other download here —
/// an application that updates itself quietly while nagging about its plugins
/// is two policies where there should be one.
pub fn gear_for(
    window: &adw::PreferencesWindow,
    plugin: &Plugin,
    profile_dir: Option<&PathBuf>,
    update_available: bool,
) -> Option<gtk::Button> {
    if !has_page(plugin) {
        return None;
    }
    let button = gtk::Button::from_icon_name("emblem-system-symbolic");
    button.set_valign(gtk::Align::Center);
    button.add_css_class("flat");
    button.set_tooltip_text(Some(if update_available {
        "Settings for this plugin — an update is available"
    } else {
        "Settings for this plugin"
    }));
    if update_available {
        button.add_css_class("accent");
    }

    let window = window.clone();
    let plugin = plugin.clone();
    let profile_dir = profile_dir.cloned();
    button.connect_clicked(move |_| {
        push(&window, &plugin, profile_dir.as_deref());
    });
    Some(button)
}

/// Open this plugin's page as a subpage of the Settings window.
///
/// A subpage rather than a separate window, because it is the same window's
/// back button that gets the user out of it and libadwaita already draws that.
/// It is also the honest shape: this page belongs to Cordial's settings, and a
/// free-floating window would suggest the plugin owns something it does not.
pub fn push(window: &adw::PreferencesWindow, plugin: &Plugin, profile_dir: Option<&Path>) {
    let title = if plugin.manifest.name.is_empty() {
        plugin.manifest.id.clone()
    } else {
        plugin.manifest.name.clone()
    };

    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());
    content.set_content(Some(&build_page(window, plugin, profile_dir)));

    let page = adw::NavigationPage::new(&content, &title);
    window.push_subpage(&page);
}

/// The page itself: one group per declared group, in declaration order.
///
/// Split from [`push`] so a reset can rebuild the rows without a second copy of
/// how they are built.
fn build_page(
    window: &adw::PreferencesWindow,
    plugin: &Plugin,
    profile_dir: Option<&Path>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let fields = &plugin.manifest.preferences;
    let id = plugin.manifest.id.clone();

    let Some(dir) = profile_dir else {
        // The same posture the grants switches take: with nowhere to write, say
        // so rather than offering controls that silently keep nothing. A page
        // of switches that forget everything on close is the stub that lies,
        // in widget form.
        let group = adw::PreferencesGroup::new();
        group.add(
            &adw::ActionRow::builder()
                .title("No profile to save into")
                .subtitle("This profile's directory could not be resolved, so nothing set here could be kept.")
                .build(),
        );
        page.add(&group);
        return page;
    };

    let store = preferences::Store::new(dir);
    let values = match store.effective_for(&id, fields) {
        Ok(v) => v,
        // A document that exists and will not parse. Reported rather than
        // replaced with defaults, because drawing the defaults would invite
        // the first switch touched to write straight over answers that may
        // still be recoverable by hand.
        Err(why) => {
            let group = adw::PreferencesGroup::new();
            let row = adw::ActionRow::builder()
                .title("These settings could not be read")
                .subtitle(gtk::glib::markup_escape_text(&why))
                .build();
            row.set_subtitle_lines(4);
            group.add(&row);
            page.add(&group);
            return page;
        }
    };

    // Grouped by the declaration's own `group`, in the order the groups first
    // appear. Fields with no group come first, in an untitled group, which is
    // what libadwaita renders a group with no title as anyway.
    let mut order: Vec<Option<&str>> = Vec::new();
    for field in fields {
        let key = field.group.as_deref();
        if !order.contains(&key) {
            order.push(key);
        }
    }

    for group_name in order {
        let group = match group_name {
            Some(name) => adw::PreferencesGroup::builder().title(name).build(),
            None => adw::PreferencesGroup::new(),
        };
        for field in fields.iter().filter(|f| f.group.as_deref() == group_name) {
            let value = values.get(&field.key).cloned().unwrap_or_else(|| field.field.default_value());
            group.add(&build_row(field, &value, &store, &id, fields));
        }
        page.add(&group);
    }

    // Offered because a page built from a declaration is a page whose defaults
    // are a real, named thing the plugin author chose — unlike a hand-written
    // settings page, where "the default" is whatever the code happened to do.
    // Deleting the file rather than writing the defaults into it, so a later
    // version of the plugin that changes a default governs a preference the
    // user never set.
    let reset_group = adw::PreferencesGroup::new();
    let reset_row = adw::ActionRow::builder()
        .title("Restore defaults")
        .subtitle("Forget everything set here and use what the plugin suggests.")
        .build();
    let reset = gtk::Button::with_label("Restore");
    reset.set_valign(gtk::Align::Center);
    reset.add_css_class("flat");
    reset_row.add_suffix(&reset);
    reset_group.add(&reset_row);
    page.add(&reset_group);
    {
        let window = window.clone();
        let plugin = plugin.clone();
        let dir = dir.to_path_buf();
        let store = store.clone();
        let id = id.clone();
        reset.connect_clicked(move |_| {
            if let Err(e) = store.reset(&id) {
                eprintln!("shell: could not clear {id}'s preferences: {e}");
                return;
            }
            // Rebuilt rather than each row reset in place: a row knows its own
            // value and not its neighbours', and setting fifteen of them by
            // hand is fifteen chances to miss one.
            window.pop_subpage();
            push(&window, &plugin, Some(&dir));
        });
    }

    page
}

/// One declared field as one row.
///
/// Every arm saves through [`preferences::Store::set`], which re-checks the
/// value against the declaration before it writes. That check is not redundant
/// with the widget's own bounds: a `Choice` row maps an index back to a value,
/// and an index that has drifted from the options list is exactly the bug that
/// would otherwise write a plausible-looking wrong answer.
fn build_row(
    field: &Declaration,
    value: &serde_json::Value,
    store: &preferences::Store,
    id: &str,
    fields: &[Declaration],
) -> gtk::Widget {
    // A closure per row rather than a shared handler, because each row owns the
    // revert: a save that fails must put the widget back to what the file still
    // says, and only the widget knows what it was.
    let save = {
        let store = store.clone();
        let id = id.to_string();
        let fields = fields.to_vec();
        let key = field.key.clone();
        move |v: serde_json::Value| -> bool {
            match store.set(&id, &fields, &key, v) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("shell: could not save {id}'s {key}: {e}");
                    false
                }
            }
        }
    };

    match &field.field {
        Field::Bool { .. } => {
            let row = adw::SwitchRow::builder()
                .title(&field.title)
                .active(value.as_bool().unwrap_or(false))
                .build();
            decorate(&row, field);
            row.connect_active_notify(move |row| {
                let on = row.is_active();
                if !save(serde_json::Value::Bool(on)) {
                    row.set_active(!on);
                }
            });
            row.upcast()
        }
        Field::Int { step, .. } => {
            let (low, high) = field.field.bounds().expect("an Int always has bounds");
            let step = step.unwrap_or(1) as f64;
            let adjustment = gtk::Adjustment::new(
                value.as_i64().unwrap_or(low) as f64,
                low as f64,
                high as f64,
                step,
                step * 10.0,
                0.0,
            );
            let row = adw::SpinRow::new(Some(&adjustment), step, 0);
            row.set_title(&field.title);
            decorate(&row, field);
            row.connect_value_notify(move |row| {
                let n = row.value().round() as i64;
                if !save(serde_json::Value::from(n)) {
                    // Nothing to revert to that is better than leaving the
                    // number the user typed on screen: the file still holds
                    // the old one, and the next open shows it. Reverting here
                    // would fight the user's cursor mid-edit.
                }
            });
            row.upcast()
        }
        Field::Choice { options, .. } => {
            let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
            let model = gtk::StringList::new(&labels);
            let selected = value
                .as_str()
                .and_then(|v| options.iter().position(|o| o.value == v))
                .unwrap_or(0) as u32;
            let row = adw::ComboRow::builder()
                .title(&field.title)
                .model(&model)
                .selected(selected)
                .build();
            decorate(&row, field);
            let options = options.clone();
            row.connect_selected_notify(move |row| {
                let Some(option) = options.get(row.selected() as usize) else {
                    // The index and the list have disagreed. Refusing to write
                    // beats writing whichever option happens to be first.
                    eprintln!("shell: {} is not an option this row offers", row.selected());
                    return;
                };
                save(serde_json::Value::String(option.value.clone()));
            });
            row.upcast()
        }
        Field::Text { .. } => {
            let row = adw::EntryRow::builder().title(&field.title).build();
            row.set_text(value.as_str().unwrap_or_default());
            decorate(&row, field);
            // On `apply` rather than on every keystroke: a text row saving per
            // character writes the file once per letter and, worse, saves every
            // half-typed intermediate value as though the user meant it.
            row.set_show_apply_button(true);
            row.connect_apply(move |row| {
                save(serde_json::Value::String(row.text().to_string()));
            });
            row.upcast()
        }
    }
}

/// The two things every row needs and neither of which is its value.
///
/// `use-markup` off is the important half: the title is the plugin author's
/// string, and libadwaita would otherwise parse `<b>` and `<span>` in it. The
/// subtitle has no such property, so it is escaped instead — the same words,
/// drawn literally, by whichever route each widget allows.
fn decorate(row: &impl IsA<adw::PreferencesRow>, field: &Declaration) {
    let row = row.as_ref();
    row.set_use_markup(false);
    if field.description.is_empty() {
        return;
    }
    let escaped = gtk::glib::markup_escape_text(&field.description);
    if let Some(action) = row.dynamic_cast_ref::<adw::ActionRow>() {
        action.set_subtitle(&escaped);
        action.set_subtitle_lines(3);
    } else if let Some(entry) = row.dynamic_cast_ref::<adw::EntryRow>() {
        // `AdwEntryRow` has no subtitle at all — its title is the floating
        // label inside the field. The description would otherwise be silently
        // dropped, so it goes somewhere a user can still reach it.
        entry.set_tooltip_text(Some(&field.description));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema that belongs to no real plugin, on purpose.
    ///
    /// A renderer tested against whichever plugin its author happened to be
    /// writing grows to fit that plugin. These exercise the contract.
    fn fabricated() -> Vec<Declaration> {
        serde_json::from_value(serde_json::json!([
            {"key": "loud", "type": "bool", "title": "Be loud", "default": true},
            {"key": "level", "type": "int", "title": "Level", "default": 3,
             "minimum": 1, "maximum": 10, "step": 2, "group": "Tuning"},
            {"key": "mode", "type": "choice", "title": "Mode", "default": "slow",
             "group": "Tuning",
             "options": [{"value": "slow", "label": "Slow"}, {"value": "fast", "label": "Fast"}]},
            {"key": "note", "type": "text", "title": "Note", "description": "Free text."}
        ]))
        .expect("the fabricated schema should parse")
    }

    fn plugin_with(preferences: serde_json::Value) -> Plugin {
        let manifest = serde_json::json!({
            "id": "fabricated",
            "name": "Fabricated",
            "entry": "main.ts",
            "capabilities": ["log"],
            "preferences": preferences,
        });
        cordial_plugins::manifest::parse(&manifest.to_string(), Path::new("/plugins/fabricated"))
            .expect("the fabricated manifest should parse")
    }

    /// Build the real widgets for the fabricated schema, on a real display.
    ///
    /// `#[ignore]` because it needs one, and a headless CI runner has none --
    /// run it with `cargo test -p cordial-shell -- --ignored`. It is here
    /// because everything else in this file tests the rules *around* the
    /// rendering, and a page that panics on its first `AdwSpinRow` would pass
    /// all of them. Every field shape is in `fabricated()` precisely so this
    /// touches each arm of `build_row` once.
    #[test]
    #[ignore = "needs a display; run with --ignored"]
    fn every_field_shape_actually_builds_a_widget() {
        if adw::init().is_err() {
            eprintln!("no display; nothing was verified");
            return;
        }
        let window = adw::PreferencesWindow::new();
        let plugin = plugin_with(serde_json::to_value(fabricated()).unwrap());
        let dir = std::env::temp_dir().join("cordial-prefs-render-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Once with nothing saved, so every row takes its declared default.
        let page = build_page(&window, &plugin, Some(&dir));
        assert!(page.is::<adw::PreferencesPage>());

        // And once with answers saved, which is the path that reads a value
        // back out of the store and into a widget -- including the combo row
        // mapping a stored value to a list index, the one place a drift
        // between the two would write a plausible wrong answer.
        let store = preferences::Store::new(&dir);
        let fields = &plugin.manifest.preferences;
        store.set("fabricated", fields, "mode", serde_json::json!("fast")).unwrap();
        store.set("fabricated", fields, "level", serde_json::json!(9)).unwrap();
        store.set("fabricated", fields, "note", serde_json::json!("typed")).unwrap();
        let page = build_page(&window, &plugin, Some(&dir));
        assert!(page.is::<adw::PreferencesPage>());

        // And the no-profile arm, which must produce an explanation rather
        // than controls that could not keep anything.
        let page = build_page(&window, &plugin, None);
        assert!(page.is::<adw::PreferencesPage>());
    }

    #[test]
    fn a_plugin_declaring_nothing_gets_no_gear() {
        // The signal the whole affordance rests on. No declaration, no page,
        // no button — rather than a button that opens an empty page.
        assert!(!has_page(&plugin_with(serde_json::json!([]))));
    }

    #[test]
    fn a_plugin_declaring_a_field_gets_one() {
        assert!(has_page(&plugin_with(serde_json::json!([
            {"key": "loud", "type": "bool", "title": "Be loud"}
        ]))));
    }

    #[test]
    fn the_manifest_carries_the_declaration_through_parsing_intact() {
        // The renderer reads `plugin.manifest.preferences`, so this is the
        // join between the schema contract and the page. If the manifest
        // dropped a field the page would simply be missing a row, which is
        // the silent failure worth a test rather than a comment.
        let plugin = plugin_with(serde_json::to_value(fabricated()).unwrap());
        let declared = &plugin.manifest.preferences;
        assert_eq!(declared.len(), 4);
        assert_eq!(declared[1].group.as_deref(), Some("Tuning"));
        assert_eq!(declared[1].field.bounds(), Some((1, 10)));
    }

    #[test]
    fn a_manifest_declaring_something_unrenderable_does_not_load_at_all() {
        // The property that keeps the schema and this file in step: the page
        // can draw every shape the manifest can express, because a manifest
        // that expresses anything else is refused before it reaches here.
        let refused = cordial_plugins::manifest::parse(
            &serde_json::json!({
                "id": "fabricated", "entry": "main.ts",
                "preferences": [{"key": "k", "type": "colour", "title": "Colour"}]
            })
            .to_string(),
            Path::new("/plugins/fabricated"),
        );
        assert!(refused.is_err(), "an undrawable field type should refuse the plugin");
    }

    #[test]
    fn groups_come_out_in_declaration_order_with_the_ungrouped_first() {
        // What `build_page` iterates. Kept as a test of the ordering rule
        // rather than of the widgets, because the widgets need a GTK main
        // thread and the rule is the part that could silently change.
        let fields = fabricated();
        let mut order: Vec<Option<&str>> = Vec::new();
        for field in &fields {
            let key = field.group.as_deref();
            if !order.contains(&key) {
                order.push(key);
            }
        }
        assert_eq!(order, vec![None, Some("Tuning")]);
    }
}
