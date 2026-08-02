# ADR-013: Configuration belongs to the profile; code belongs to the machine

**Status:** accepted
**Extends:** [ADR-012](ADR-012-profiles-and-instances.md)
**Related:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-005](ADR-005-flag-service.md), [ADR-007](ADR-007-host-resources-are-brokered.md)

## Decision

ADR-012 says a profile is "one account's Roblox storage, plugin set and flag
overrides". Only the first of those was true in code: storage moved into
`profiles/<name>/` and everything else stayed global. This finishes it, and
draws the line by what a thing *is* rather than by which directory it started
in.

**Per profile**, under `$XDG_DATA_HOME/cordial/profiles/<name>/`:

| | |
|---|---|
| `flags.json` | the user's own FastFlag overrides |
| `plugin-grants.json` | what each plugin is allowed to do |
| `plugins/<id>/settings.json` | what each plugin remembers |

**Global**, unchanged:

| | |
|---|---|
| `$XDG_DATA_HOME/cordial/plugins/<id>/` | installed plugin code, and its own `flags.json` |
| `$XDG_CONFIG_HOME/cordial/shell.json` | the shell's appearance |

Installing a plugin once is right; a plugin is software on the machine. What it
may do, and what it knows, are decided per account.

The shell's appearance stays global because it is application chrome and not
identity. Switching profiles must not flash the window from dark to light — that
would be the window announcing something about the user's accounts, which is
neither a thing they asked for nor a thing the window is for.

**A plugin's settings are brokered, never handed over.** Two capabilities,
`settings.read` and `settings.write`, each scoped to the plugin's own id, which
Cordial takes from its record of which process is on the other end of the pipe.
The methods have no id parameter at all. The current document is delivered in a
handshake push, `cordial/init`, before the plugin has asked for anything.

**One argument decides the lot.** The profile is passed to the client on the
command line; nothing else about the layout is. Everything above is resolved
from that one directory.

## Why grants had to move, specifically

This is the part that is a security property rather than tidiness.

Grants lived at `~/.config/cordial/plugin-grants.json` — one list, every
account. So approving a plugin in a throwaway profile *while trying it out* also
approved it, silently and permanently, in the profile someone actually plays on,
against the account they care about. Nothing about the approval said so. The
user was asked one question and a different, larger one was answered.

ADR-003's default deny is only worth something if the thing being denied is the
thing the user was asked about. A global allow-list that happens to be consulted
from inside a profile is not that; it is the old design wearing a profile's
clothes. Per profile, an approval means what it looked like it meant, and a
profile made expressly to run something untrusted stays untrusted.

The same argument applies more weakly to flags, and it still applies: a `DFFlag`
set while debugging on a test account was silently still set on the account
someone plays, and flags are exactly the setting people change temporarily and
forget.

## Why plugin settings are a capability and not a directory

The obvious implementation is to give each plugin a directory and let it write.
ADR-007 forbids it: a plugin never receives a socket, a descriptor or a path,
because the whole containment argument is that Cordial holds the resource and
performs the effect. A path is a channel. Given one, a plugin's reach is bounded
by whatever the filesystem permits rather than by what was granted, and the Deno
process is deliberately started with no file access at all so that the broker
having a hole in it is not sufficient to reach the machine.

So Cordial owns the file and the plugin exchanges a document. Same shape as
`presence.set`: the effect, never the channel.

**Two capabilities, not one**, for the reason ADR-006 splits `events.declare`
from `events.publish`. A plugin that only reads its configuration should not
have to be trusted to rewrite it, and a user approving "remember which panel I
had open" has not thereby approved "discard everything I set".

**The id is an absent parameter rather than a checked one.** A settings API that
took the plugin id is the natural way to write one, and it would let any plugin
holding `settings.read` address every other plugin's document. Expressing the
scope as a parameter that does not exist means there is no check to reorder,
skip or forget — the same reasoning `events.rs` gives for constructing a
namespace from the caller's id rather than accepting one.

## Why the profile is an argument and the settings are not

The client is told which profile to run on the command line, and reads
everything else out of it. The alternative — passing flags, grants or settings
in as arguments — was rejected for two reasons.

It duplicates the source of truth. Two places to say where a value comes from is
one place too many, and the one that drifts is always the one nobody reads.

More importantly, **an argument is fixed at `exec` and settings are not**.
ADR-005 exists because the `DFFlag`/`DFInt`/`DFString` families are re-read while
the client runs; changing one mid-session is the point of them. A value passed
on the command line could never express a change made five minutes into a
session, so a design that passed settings that way would be correct exactly
until the first thing anybody wanted to do with it.

## Migration

An existing global `flags.json` or `plugin-grants.json` is **moved** into the
profile, once, and only when that profile does not already have one — the same
guard and the same tone as `profile::migrate_legacy_layout`. Leaving them to be
ignored would present as every plugin having silently lost its permissions and
every override having silently stopped working, with the old files still sitting
there looking correct. That is the class of failure ADR-012's own migration
exists to prevent.

**Moved rather than copied, into whichever profile first goes looking.** There
is no record of which profile a global file was meant for, because it was meant
for all of them — the thing being fixed. Copying into every profile would
faithfully rebuild the global allow-list. In practice the profile is `default`,
since that is where ADR-012's migration lands existing storage.

## Consequences

**Accepted:** files a user may want to edit by hand now live in a directory
forced to `0700` and under `$XDG_DATA_HOME` rather than `$XDG_CONFIG_HOME`,
which is not where anyone would look for them first. The alternative is
splitting one account's state across two roots and inventing a rule for which
half a given file belongs to, which is worse: a profile is meant to be one
object a user can copy, keep or delete whole. Nothing here rests on what else a
profile directory does or does not contain — this ADR's argument is about who
approved what, and it holds whether or not the profile also holds a credential.

**Accepted:** copying a profile copies its grants. That follows from grants
being part of the profile and is the behaviour a user duplicating a profile
would expect, but it means a copy inherits approvals rather than starting at
default deny.

**Accepted:** a plugin's settings survive it being uninstalled, because
uninstalling removes the code from the machine and the document lives in each
profile. A stale document is cheap; deleting a user's configuration because they
briefly removed a plugin is not.

**Accepted:** the environment overrides `CORDIAL_FLAGS` and
`CORDIAL_PLUGIN_GRANTS` are global by nature — setting one makes a single file
serve every profile, which is the arrangement this ADR ends. They are kept for
tests and side-by-side development and are documented as development switches,
not as a supported configuration.

**Rejected: per-profile plugin installation.** It would mean several copies of
the same code on disk and a plugin update that applies to some accounts and not
others. Installing is a machine-level act; approving is an account-level one.

**Rejected: putting the shell's appearance in the profile.** It is chrome, not
identity, and a window that changes theme when you switch account is reporting
on something it has no business reporting on.

**Open: a plugin's own `flags.json` is still global, and still unconditional.**
`flags::collect` reads every installed plugin's flags at launch without
consulting grants at all, so a plugin that was granted nothing in this profile —
or in any profile — still contributes startup flags to every launch. That
predates this change and is not made worse by it, but per-profile grants make
the gap easier to see: "approved here" and "affects this profile" ought to be
the same statement and currently are not. Fixing it means deciding whether flag
contribution is gated by `flags.write` at launch time, which is an ADR-005
question rather than an ADR-013 one.

## What would change this

If plugin settings ever needed to be shared deliberately between profiles — a
theme a user wants everywhere — that wants an explicit copy or an export, not a
second global location. The moment there are two places a setting can live, the
question "which one am I editing?" has no answer a user can check.
