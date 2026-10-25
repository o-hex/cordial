---
title: "ADR-048: Labels and edits from Discord are allowlisted, attributed and logged before they happen"
---
**Status:** accepted; the label picker ships off, and nothing is verified against a live Discord server or GitHub App
**Date:** 2026-10-01
**Related:** [ADR-030](/adr/ADR-030-reports-arrive-from-discord)

## Context

[ADR-030](/adr/ADR-030-reports-arrive-from-discord) lets somebody without a GitHub
account file a report, comment on it and close it. Two things were missing that
a maintainer needs: reports arrive unlabelled, so triage starts from nothing,
and a reporter who notices a typo, or learns their compositor, has no way to fix
the report short of commenting.

Both are the same shape as the close button: an action on the tracker taken by
somebody the tracker has never heard of. The rule from ADR-030 carries over
unchanged -- **who may act is read from the issue and from Discord's own
statement about the person, never from a button's `custom_id`** -- and the
questions this ADR answers are what that means for labels, which carry
meaning the project has to be able to trust, and for edits, which can destroy
what a reporter wrote.

## Decision

### The picker is off by default

**`DISCORD_LABEL_PICKER=1` turns it on; anything else leaves it off.** A string
select inside a modal is documented but has not been observed, and a modal
Discord rejects fails every report dialog, not only the one with the picker.
That is too large a blast radius to ship unverified. **Turn it on after
confirming in a test server that the modal renders.** Off, the report dialog is
the pre-label dialog exactly (same fields, same slots, nothing moved to the
follow-up), the Edit dialog edits title and text only, no labels are fetched,
and label picks arriving from a dialog opened earlier are ignored. There is no
cheap label path that avoids a modal select, so with it off labels cannot be set
from Discord; moderators use GitHub. Everything below describes the picker on.

### Labels

**A reporter chooses what part of Cordial a report concerns. Only a moderator
may say what the project has decided about it.**

* The report dialog gets a multi-select of the repository's labels, in the
  fifth modal slot. It is optional, and a report never waits on it.
* A **reporter** may pick only labels matching an **allowlist**: `area:*`,
  `platform:*`, `compositor:*`, `gpu:*` by default, replaced by
  `GITHUB_REPORTER_LABELS`. A small **protected** set (`confirmed`, `wontfix`,
  `invalid`, `duplicate`, `priority*`, `severity*`, `security*`, `triage*`,
  `good first issue`, `help wanted`) is refused to reporters **whatever the
  allowlist says**. It lives in code deliberately: the failure it prevents is an
  allowlist widened to `*` in a hurry, and a guard that the same hurry can remove
  prevents nothing.
* A **moderator** may apply any label that exists.
* Labels a template already applies (`bug`) are applied as before and are not
  offered.
* The list is fetched through the bridge's own GitHub client, ten pages at most,
  cached ten minutes. A failed refresh keeps serving the last good list and
  retries after thirty seconds. With no list, the dialog has no menu and the
  report files without labels.
* More than 25 eligible labels: those already on the issue first, then the
  form's own groups (`FORM_LABEL_ORDER` in `labels.ts`: graphics stack and
  compositor for a bug report, area for a feature), then name order. The dialog
  says how many did not fit.
* **The submission is re-checked, not trusted.** A select's values come from the
  client. They are resolved against the cached list and the allowlist again, so
  a forged or stale value is dropped, and a name GitHub does not know is never
  passed on -- GitHub would create it.

**The menu costs a form one optional field.** A modal holds five components and
`bug_report` already used all five. The picker takes the fifth, and the optional
field it displaces moves to the existing "Add the rest" follow-up. A required
field never loses its place to it: if a form cannot spare the slot, it opens
without the picker and `deno task check` says so.

### Moderators

One definition, used for labels, editing others' reports and **Mark as
completed**: a member holding Manage Messages, Manage Threads or Administrator in
the channel (read from the interaction, so it is never stale), **or** holding a
role listed in `DISCORD_MODERATOR_ROLE_IDS`. A role only widens the set. A
malformed id in that variable stops the bridge at startup rather than silently
granting nobody anything.

### Editing

An **Edit** button joins the controls on a thread's first message.

* **Who:** the reporter recorded in the issue's marker, or a moderator. Everyone
  else gets an ephemeral refusal. An issue with no recorded reporter -- filed on
  the web, or before the marker carried one -- can be edited by moderators only.
  The check runs when the dialog opens and again on submit.
* **What:** the title, the labels, and the form's free-text fields. A modal holds
  five components, so a report is edited in parts -- title, labels and three
  fields first, then five at a time behind an **Edit more fields** button.
  **The Diagnostics block, the credit line and the hidden marker are never
  editable** and are carried through byte for byte; an edit cannot reassign a
  report to somebody else or unpair it from its thread. Dropdown answers and any
  answer too long for its box are left as they are.
* **Refusing rather than guessing.** A body is parsed back into fields only if
  re-assembling the parse reproduces it exactly. A body somebody reflowed on
  GitHub, or one that is not the bridge's, offers title and labels only.
* **Stale dialogs are refused.** The dialog carries a fingerprint of the title,
  body and labels it showed; if they differ on submit, nothing is saved. This is
  what stops "never silently overwrite" being true only of the log.
* **Audit before change.** The audit comment is posted on the issue *first*,
  and if it cannot be, the edit does not happen. It records who (Discord display
  name and id, and whether as the reporter or as a moderator), when (UTC), what
  changed (title before and after, characters before and after per field,
  labels added and removed) and **the previous text of everything that changed**,
  in a `<details>` inside a fence that the quoted text cannot close. If the
  edit then fails, the comment is amended to say it was not applied. A one-line
  copy goes in the Discord thread, which is also renamed if the title changed,
  and the operator's log gets a line without the text.
* **Labels** follow the same rules as at filing. An editor only controls what
  they were offered: labels they were not offered -- `bug`, a maintainer's
  `confirmed` -- are preserved, so a reporter tidying a typo cannot strip
  triage by deselecting something they could not see.

## Consequences

**No new GitHub App permission.** Reading labels, changing an issue and editing
comments are all inside Issues: read and write.

**No new required configuration.** `GITHUB_REPORTER_LABELS` and
`DISCORD_MODERATOR_ROLE_IDS` are optional. But the repository has only GitHub's
default labels, none of which a reporter may choose, so **reporters see no
picker until the `area:`, `platform:`, `compositor:` or `gpu:` labels exist.**
That is the safe direction for an unconfigured deployment, and it needs saying.

**Threads opened before this change have no Edit button.** Buttons live on a
message the bridge posted, and it does not go back and rewrite old ones.

**Two requests inside Discord's three seconds.** Opening the report dialog or
the editor cannot defer, so the label list and, for the editor, the issue are
fetched inside that window, concurrently and with deadlines (0.7 s for labels, 1.6 s
for the issue). A slow GitHub costs the dialog its label menu, or the editor its
opening with a "press Edit again". A cold Worker isolate is where that bites.

**Between the fingerprint check and the `PATCH` there is one round trip.** An
edit that lands inside it is overwritten, and its text is in the audit comment of
the edit that overwrote it. That is a window, not a lock, and it is accepted.

**A GitHub label deleted from the repository can linger in the cache for ten
minutes.** It is offered, chosen, and then fails to resolve at submit, so the
report files without it and says so. There is no `label` webhook handler to
invalidate sooner.

**What is not verified.** Whether Discord accepts a multi-select with
`min_values: 0` and pre-selected options inside a modal, and what it sends for
an empty one, is read from its documentation and not observed. The tests use
fakes. The first real run should be treated as the first real run.
