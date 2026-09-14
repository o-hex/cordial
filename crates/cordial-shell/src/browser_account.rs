//! Resolve a browser's account without replacing a saved profile's session.

mod profile;
mod ticket;
mod transport;

use std::path::Path;

pub(crate) use profile::ProfileMatch;
pub use ticket::LaunchTicket;
use transport::AccountId;

/// An unreadable profile cannot establish an account match. Duplicate accounts
/// deliberately leave the choice to the user instead of choosing by directory order.
pub(crate) fn matching_profile(root: &Path, account: AccountId) -> Option<ProfileMatch> {
    profile::matching_profile_with(root, account, transport::authenticated)
}

pub(crate) fn resolve(ticket: LaunchTicket) -> Option<ProfileMatch> {
    let account = transport::lookup(&ticket)?;
    matching_profile(&cordial_shell::profile::root(), account)
}

#[cfg(test)]
pub(crate) use profile::snapshot_for_test;

#[cfg(test)]
mod tests;
