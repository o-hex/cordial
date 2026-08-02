//! What a plugin is allowed to do, and nothing else.
//!
//! Capabilities are named, granted per plugin, and checked at the point of use.
//! The list is closed: there is no capability that means "anything", and
//! [ADR-003](../../../docs/adr/ADR-003-plugin-isolation.md) is explicit that a
//! capability handing over the machine is not a capability but the absence of
//! one. That is why there is no `process.spawn`, no filesystem path, and no
//! memory access here.
//!
//! Adding a variant is a design decision, not a convenience. If a plugin needs
//! something, the question is what *narrow, named* effect it needs — not what
//! access would let it arrange the effect itself.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read the resolved FastFlag set, including which layer set each value.
    FlagsRead,
    /// Contribute flags. Startup flags land in the plugin's own `flags.json` and
    /// take effect at the next launch; see ADR-005 for why that is not the same
    /// surface as changing one live.
    FlagsWrite,
    /// Change a `DFFlag`/`DFInt`/`DFString` while the client runs. Deliberately
    /// distinct from `FlagsWrite`: the static families cannot be changed live at
    /// all, and an API that accepted them here would silently do nothing.
    FlagsWriteDynamic,
    /// Emit log lines into Cordial's own output.
    Log,
    /// Observe client lifecycle events — launch, ready, shutdown.
    LifecycleRead,
    /// Publish Discord Rich Presence.
    ///
    /// The effect, not the channel. Cordial owns the connection to Discord's
    /// IPC socket and the plugin sends a presence payload; it never learns the
    /// socket's location, cannot read Discord's state, and cannot send arbitrary
    /// frames. See ADR-007 — a plugin can never hold a host resource, because a
    /// Flatpak permission is app-wide and permanent while a capability is
    /// per-plugin and revocable, and the two cannot be made to mean the same
    /// thing.
    ///
    /// Off unless granted, and privacy-relevant: what someone is playing and
    /// when is not always something they want broadcast.
    PresenceSet,
    /// Post a desktop notification through the freedesktop portal.
    ///
    /// Brokered for the same reason as `PresenceSet`: the plugin sends a summary
    /// and a body, Cordial owns the D-Bus connection. A plugin that held the bus
    /// could talk to every other service on it.
    NotifySend,
    /// Open a URL in the user's browser, through the portal.
    ///
    /// The narrowest useful form of "leave the application". Cordial validates
    /// the scheme before handing it to the portal — `http` and `https` only, so
    /// this cannot become `file://` traversal or a handler-hijack for some
    /// arbitrary registered scheme.
    UrlOpen,
    /// Register a directory of files that resolve before Roblox's own assets
    /// of the same name — see
    /// [ADR-010](../../../docs/adr/ADR-010-plugin-asset-overlays.md).
    ///
    /// Narrow on purpose: this is one filesystem root the plugin owns, checked
    /// ahead of the APK for a name match, not a general filesystem capability.
    /// It cannot write into the APK or into anything Cordial extracts from it
    /// — both stay untouched — and it cannot read anything outside the root it
    /// registers. Uninstalling the plugin (or it giving up the root) makes the
    /// original asset resolve again with nothing to clean up, because nothing
    /// was ever overwritten to begin with.
    AssetsOverride,
    /// Read the plugin's own settings document.
    ///
    /// A plugin has nowhere of its own to keep anything — it runs with no file
    /// access at all — so before this existed it could not remember a single
    /// thing between launches, and the only way to give it one would have been
    /// a path or a descriptor. ADR-007 rules both out, so Cordial holds the
    /// file and the plugin exchanges a document: the effect, never the channel,
    /// exactly as `PresenceSet` owns Discord's socket.
    ///
    /// Scoped to the plugin's own id, which Cordial takes from its record of
    /// which process is on the other end of the pipe rather than from the
    /// request. A field a plugin can set is a field it can set to somebody
    /// else's name, which is why the event registry does not accept one either.
    SettingsRead,
    /// Replace the plugin's own settings document.
    ///
    /// Split from `SettingsRead` for the reason `EventsDeclare` is split from
    /// `EventsPublish`: a plugin that only reads its configuration should not
    /// have to be trusted to rewrite it. A user approving "remember which
    /// panel I had open" has not thereby approved "discard everything I set".
    SettingsWrite,
    /// Register event types under the plugin's own namespace. See ADR-006.
    ///
    /// Separate from `EventsPublish` on purpose: declaring is what makes a
    /// type's origin a fact the registry can check rather than a claim a
    /// plugin makes about itself, and that check is only worth anything if a
    /// plugin cannot skip straight to publishing.
    EventsDeclare,
    /// Broadcast on an event type the plugin declared with `EventsDeclare`.
    ///
    /// Deliberately distinct from declaring: a plugin that could publish on
    /// any string it liked could impersonate another plugin's events, and a
    /// subscriber would have no way to tell. This capability only ever lets a
    /// plugin speak inside a namespace the registry has already attributed to
    /// it.
    EventsPublish,
    /// Receive events, including ones other plugins declared.
    ///
    /// Broader than `EventsPublish` deliberately — hearing something happened
    /// is a different power from being believed when you say it did, and a
    /// plugin that only reacts should not have to be trusted to speak.
    EventsSubscribe,
}

impl Capability {
    /// The wire name, which is what appears in a manifest.
    pub fn name(self) -> &'static str {
        match self {
            Capability::FlagsRead => "flags.read",
            Capability::FlagsWrite => "flags.write",
            Capability::FlagsWriteDynamic => "flags.write.dynamic",
            Capability::Log => "log",
            Capability::LifecycleRead => "lifecycle.read",
            Capability::PresenceSet => "presence.set",
            Capability::NotifySend => "notify.send",
            Capability::UrlOpen => "url.open",
            Capability::AssetsOverride => "assets.override",
            Capability::SettingsRead => "settings.read",
            Capability::SettingsWrite => "settings.write",
            Capability::EventsDeclare => "events.declare",
            Capability::EventsPublish => "events.publish",
            Capability::EventsSubscribe => "events.subscribe",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "flags.read" => Capability::FlagsRead,
            "flags.write" => Capability::FlagsWrite,
            "flags.write.dynamic" => Capability::FlagsWriteDynamic,
            "log" => Capability::Log,
            "lifecycle.read" => Capability::LifecycleRead,
            "presence.set" => Capability::PresenceSet,
            "notify.send" => Capability::NotifySend,
            "url.open" => Capability::UrlOpen,
            "assets.override" => Capability::AssetsOverride,
            "settings.read" => Capability::SettingsRead,
            "settings.write" => Capability::SettingsWrite,
            "events.declare" => Capability::EventsDeclare,
            "events.publish" => Capability::EventsPublish,
            "events.subscribe" => Capability::EventsSubscribe,
            _ => return None,
        })
    }

    /// Every capability, so a UI can present the full set rather than a
    /// hand-maintained copy that drifts.
    pub fn all() -> &'static [Capability] {
        &[
            Capability::FlagsRead,
            Capability::FlagsWrite,
            Capability::FlagsWriteDynamic,
            Capability::Log,
            Capability::LifecycleRead,
            Capability::PresenceSet,
            Capability::NotifySend,
            Capability::UrlOpen,
            Capability::AssetsOverride,
            Capability::SettingsRead,
            Capability::SettingsWrite,
            Capability::EventsDeclare,
            Capability::EventsPublish,
            Capability::EventsSubscribe,
        ]
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_capability_round_trips_through_its_wire_name() {
        // `name`, `parse` and `all` are three hand-maintained lists of one
        // thing. A variant added to two of them and missed in the third fails
        // quietly: a grants file naming it would be refused as unknown, and the
        // user would be told they granted something that does not exist.
        for c in Capability::all() {
            assert_eq!(Capability::parse(c.name()), Some(*c), "{c} does not parse back");
        }
        let names: BTreeSet<&str> = Capability::all().iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), Capability::all().len(), "two capabilities share a wire name");
    }

    #[test]
    fn reading_settings_is_not_writing_them() {
        // Split for the same reason declare and publish are split. A copied
        // arm returning the other's name here would make a grant of
        // settings.read parse as settings.write and silently widen it.
        assert_eq!(Capability::parse("settings.read"), Some(Capability::SettingsRead));
        assert_eq!(Capability::parse("settings.write"), Some(Capability::SettingsWrite));
        assert_ne!(Capability::SettingsRead, Capability::SettingsWrite);
    }
}
