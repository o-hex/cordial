# ADR-012: A profile is storage; an instance is a window

**Status:** accepted
**Extended by:** [ADR-013](ADR-013-per-profile-configuration.md)
**Related:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-005](ADR-005-flag-service.md), [ADR-006](ADR-006-plugin-events-and-first-party.md)

## Decision

Two words, kept distinct, because conflating them designs the wrong thing:

| | |
|---|---|
| **Instance** | a running Cordial process — a window. Roblox's own usage. |
| **Profile** | a directory holding one account's Roblox storage, plugin set and flag overrides. |

An instance *runs* a profile. Profiles live at
`$XDG_DATA_HOME/cordial/profiles/<name>/`, and **a profile may be held by at most
one instance at a time**, enforced by an advisory lock rather than by convention.

The "plugin set and flag overrides" half of that definition was aspirational
when this was written: only storage actually moved, and grants and flags stayed
global for several months afterwards.
[ADR-013](ADR-013-per-profile-configuration.md) finishes it and records why the
grants file in particular could not stay global — an approval given in a
throwaway profile was silently in force in the profile someone plays on.

Multiple instances is launching the process more than once, each against a
different profile.

## Why the distinction is load-bearing

The earlier path was `cordial/instances/default/run`, which held storage. That is
the wrong word for what is in it, and the wrong word here is not cosmetic: it
invites the reader to conclude that a second window needs a second data
directory, or — worse — that one directory can back two windows. The first is
false and the second is dangerous.

Roblox means *window* by instance. Matching that is not deference to their
vocabulary; it is that users asking for "multi-instance" are asking for more
windows, and the feature should be named after what they asked for.

## Why locking, rather than allowing a profile to be shared

Nothing structurally prevents two Cordial processes opening the same profile.
Unlike Fishstrap on Windows, there is no singleton mutex to defeat — each Cordial
process is genuinely independent, which makes multi-instance nearly free here and
is a real advantage of controlling the storage location ourselves.

That same freedom is the hazard. Two instances on one profile are two processes
writing one `appData` and one cookie store concurrently. Roblox's storage is not
built for that, and the realistic outcomes are a corrupted profile or one session
silently invalidating the other. Neither presents as "you did something
unsupported"; both present as bugs in Cordial.

So the lock is taken on the profile directory for the lifetime of the instance,
`LOCK_EX | LOCK_NB`. A second attempt fails immediately with a message naming the
profile, rather than blocking or racing.

**Advisory, not mandatory, and that is honest.** `flock` does not stop a process
that never asks. It stops *Cordial* from doing it by accident, which is the
actual failure mode — a user double-clicking the launcher twice, not an adversary
bypassing a lock.

## Why not OAuth

Roblox's OAuth 2.0 grants API scopes to third-party applications through the
creator dashboard. It does not issue a play session, and there is no supported
mechanism for a client to authenticate on a user's behalf. Account switching is
therefore not an authentication feature at all: each profile logs in normally and
keeps its own session in its own directory. Nothing about this design needs
Roblox to grant anything, which is also why it cannot be withdrawn.

## On credentials, and why they do not go in a keyring

> **Superseded in part.** The first argument below rested on a factual claim
> about the engine that turned out to be false, and the decision it supported —
> that Cordial never handles a session token — has been reversed. The original
> reasoning is kept in full, because the shape of the mistake is the useful
> part; the correction follows it.

Roblox keeps its session cookie inside the profile. The obvious suggestion is to
put it in the desktop keyring instead, and it is a reasonable instinct, but it is
rejected for three reasons.

**It would make Cordial the custodian of a token it currently never touches.**
The engine writes and reads its own session; Cordial only decides which directory
that happens in. Reading the cookie out, holding it, and writing it back is a
large increase in responsibility for this project, and "Cordial never handles
your credentials" is a property worth more than the encryption would be.

**The protection would be mostly illusory.** The engine reads its cookie from a
file at startup, so a keyring copy would have to be written back to disk before
every launch and would sit there in plaintext for the whole session. Encryption
at rest would apply only while Cordial is not running. Meanwhile the keyring is
unlocked at login, so any process running as the user reads it as easily as it
reads the file.

**The reachable threat is file permissions, and that is fixed instead.**
`create_dir_all` applies the umask, which on a normal desktop yields `0755` — on
a multi-user machine another account could read the session. Profile directories
are therefore forced to `0700`, on creation and on migration, with a test
asserting it. That defends against the case that actually occurs.

Users wanting encryption at rest should use full-disk encryption or
`systemd-homed`, which solve it properly and for everything rather than for one
application's cookie.

### Correction: Cordial is the custodian of the session token

**The engine never writes its cookies to disk.** Both paragraphs above that
describe it doing so — "the engine writes and reads its own session" and "the
engine reads its cookie from a file at startup" — are wrong. A complete
`CORDIAL_TRACE_PATHS=1` inventory of every non-system file the engine opens
contains no cookie jar and no credential store of any kind, and
`grep -rl ROBLOSECURITY` and `grep -rli set-cookie` over real profile trees find
nothing. The engine holds its cookies in memory for the life of the process.

On Android the **Java** side persists them. The Waydroid capture in
`docs/traces/` shows `OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler
with Cookies from URL ...` among nine cookie lines. Cordial has no Java side, so
nothing persisted anything, and the symptom was the one this project keeps
writing down: sign in, quit, restart the same profile, and you are on the
landing page.

This also disposes of the fix that suggests itself first. A shutdown hook cannot
flush a file that is never written — controlled for directly, by alternating
killed and graceful runs over two passes, which produced no file created or
updated at shutdown that a killed run does not also produce. The graceful
teardown descent is real and works and was never the missing piece.

So Cordial now reads the jar out of the engine through
`NativeSettingsInterface.nativeGetCookiesForDomain`, writes it into the profile,
and hands it back on the next launch through `nativeSetMultipleCookies`. **That
makes Cordial the custodian of a live session token, which the decision above
explicitly set out to avoid.** The trade is accepted because the alternative is
not "Cordial handles no credentials" but "Cordial cannot stay signed in", and
the account switcher this ADR is mostly about is not worth much if every profile
is signed out at every launch.

**What protects it.** The store is a single file, `cookies`, inside the profile
directory. It is `0600` and the directory is `0700`, applied wherever a profile
is chosen rather than only where it is locked — a hand-started
`cordial-run --profile <name>` previously got the umask's `0755`, which was
survivable when the directory held only Roblox's storage and is not now. It is
written to a temporary file and renamed, so an interrupted write cannot leave
half a token that still parses. The value is carried in a type whose `Debug`
prints its length, so that no diagnostic can print a session by accident, and
nothing in the implementation logs, prints or traces a cookie value at any
verbosity or behind any flag.

**The keyring is still rejected, on the argument that survives.** Only the first
of the three reasons above depended on the false premise. The second stands and
is now the whole case: the token must be handed to the engine in plaintext on
every launch, so a keyring would encrypt it only during the window in which
nothing is reading it, in exchange for an unlock prompt on every start. The
third stands unchanged and is what is actually implemented.

Users wanting encryption at rest should still use full-disk encryption or
`systemd-homed`.

## Consequences

**Accepted:** the storage path changes, and existing users have a session under
`instances/default`. The change must *move* that directory rather than start
fresh, because a silent reset presents as being logged out for no reason — which
is exactly the class of failure this project keeps writing down. Migration runs
once, when the old path exists and the new one does not.

**Accepted:** a profile is one object holding Roblox storage, plugins and flags
together, rather than three orthogonal selections. "My alt account with my main's
plugins" is not a use case worth the cost of explaining which combinations are
legal.

**Accepted:** the account switcher is a profile switcher. It does not
authenticate or know anything about accounts — it selects a directory. Cordial
never sees a password.

> The rest of this point read "and never stores a session token itself; Roblox
> does that, inside the profile." That is no longer true and was never true of
> the engine: Cordial stores the session token, because nothing else does. See
> the correction above. What is unchanged is the part that matters to this
> ADR — the switcher still only selects a directory, and authentication still
> happens in Roblox's own UI.

**Accepted:** two windows on one account is not supported. It is a real thing
people do while testing, but it requires duplicating a live session deliberately,
and Roblox may invalidate one of them. A profile copy is the honest way to ask
for it, and it should be an explicit action rather than something a second launch
does silently.

**Rejected: separating profiles from instances as independent concepts.** It
doubles the vocabulary and the settings surface to serve combinations nobody has
asked for.

**Rejected: making the lock mandatory** via a mount or a daemon. The failure this
guards against is an accident, and an advisory lock stops accidents. Anything
stronger costs more than the problem.

## What would change this

If Roblox ever ships storage that tolerates concurrent access from two clients,
the lock becomes unnecessary and same-profile multi-instance becomes free.
Nothing suggests that is coming.
