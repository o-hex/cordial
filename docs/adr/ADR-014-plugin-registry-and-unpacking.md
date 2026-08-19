# ADR-014: Plugins are published through a signed static index, and unpacked as hostile

**Status:** accepted
**Related:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-006](ADR-006-plugin-events-and-first-party.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-010](ADR-010-plugin-asset-overlays.md), [ADR-013](ADR-013-per-profile-configuration.md)

## Decision

Five things, which are separable but arrive together.

**1. `plugin.json` gains `version` and `dependencies`.** The version is
semantic, `major.minor.patch`. A dependency names **another Cordial plugin** by
id, with a requirement written in one of exactly two forms: `=1.2.0` for that
version and nothing else, `^1.2.0` for anything compatible with it.

**2. A distribution archive is a `.tar.zst`** — a tar of the plugin directory's
contents, zstd-compressed, with `plugin.json` at its root.

**3. An index is one static JSON file** listing id, name, version, requested
capabilities, dependencies, an `https` download URL and a `sha256:` content
hash for each published release. It is designed to be served straight out of a
git repository. Nothing in Cordial names a URL for one, and nothing assumes
there is only one.

**4. Resolution is offline, deterministic, and refuses by name** — a missing
dependency, an unsatisfiable requirement, a cycle, and a plugin in the plan
asking for a capability the user has not granted are four distinct, named
failures, not one "could not install".

**5. Unpacking assumes the archive is hostile.** Every rule is a refusal with a
name rather than a skipped entry, and each one has a test that fails when the
check is deleted.

`crates/cordial-plugins/{registry,resolve,unpack}.rs`, and — the marketplace
foundations described further down — `crates/cordial-plugins/{source,sign,marketplace}.rs`
and the Marketplace section of `crates/cordial-shell/src/settings.rs`.

## Why the manifest stays `plugin.json`

The obvious alternative is to put all of this in a `package.json`, since plugins
are TypeScript on Deno ([ADR-008](ADR-008-plugins-are-typescript-on-deno.md)) and
that file already exists in the ecosystem's muscle memory. It is refused, and
the reason is `dependencies`.

**One key cannot honestly carry two meanings.** In npm, `dependencies` means npm
packages. Here it means other Cordial plugins — things with ids, capability
grants, an install order and a start order. A plugin may perfectly well need
both: a Cordial plugin that declares the events it subscribes to, and a
JavaScript library to parse something. A single field naming both kinds is a
field where the reader cannot tell which is which, and the resolver cannot
either.

VS Code took the other route, overloading `package.json` with a
`contributes`/`activationEvents`/`extensionDependencies` layer, and it is the
most complained-about part of its extension model precisely because the file
answers to two authorities. Nobody can tell by looking whether a given key is
npm's or the editor's, and the tools that validate one ignore the other.

So: `plugin.json` is Cordial's, entirely. If an author wants a `deno.json`
beside it for their JS runtime's benefit, that is the runtime's business.
Cordial does not read it, does not validate it, and does not care.

**The two-operator requirement language is deliberate and a bare version is
refused.** `"1.2.0"` means *exactly 1.2.0* in npm and *^1.2.0* in Cargo. An
author arriving from either ecosystem would write it expecting their own
meaning, and be wrong half the time with nothing to tell them. Refusing a bare
version and naming both forms in the error costs one character to fix and
removes the ambiguity permanently. Everything else — `>=`, `~`, `*`,
comma-separated lists — is refused too: every operator in the language is one a
user has to understand before they can tell what an install will do.

**`version` is optional, and that is not an oversight.** Every plugin installed
before this existed has no such key, and making it required would have made
`discover` refuse all of them at once. That presents to a user as every plugin
they had silently vanishing, with the directories still sitting there looking
correct — the failure class [ADR-013](ADR-013-per-profile-configuration.md)'s
migration exists to prevent. An unversioned plugin still loads. It simply
cannot be published or depended upon, and the resolver says so by name rather
than inventing a version for it.

## Why `.tar.zst`

Better ratio and better speed than gzip, with mature Rust crates for both
halves, and it is what the rest of the Linux packaging world has moved to.

**Zip was considered and is worse here, for a reason that is not about
compression.** Zip carries Unix mode bits in an extension field that producers
populate inconsistently — some write them, some write zeros, some write
Windows attributes instead. That does not make zip *safer* despite the
temptation to read it that way; it makes the same archive unpack differently
depending on what wrote it, which is the property you least want in the format
you are hardening against. Tar states the mode plainly, so the setuid refusal
below has something definite to refuse.

## Why the index is a static file, and why no URL is written down

A static file in a git repository is **auditable** — every change to what is on
offer is a diff a human can read, and the history is the audit log — **forkable**
— a user who dislikes what is in an index can maintain their own, and it is the
same file in the same format — and **cheap**, because it is a file. This is
Homebrew's tap arrangement rather than a package server, and the reason is the
same one: a registry that is a service is a registry only its operator can
check.

**Who hosts an index and who decides what goes in one is a policy question, and
it is not answered here.** It belongs to whoever maintains Cordial, and they
have not decided. What this design does instead is refuse to prejudge it: a
curated list and a self-hosted index are indistinguishable to the code, and
combining two indexes that publish the same id and version pointing at
different bytes is **refused rather than resolved by precedence**. Precedence
*is* the policy question. Whichever order were picked, an index a user added
for one plugin would silently be deciding where a different plugin's bytes come
from.

## Signing: the mechanism is implemented; the key is still nobody's to ship

The intended scheme is a **detached minisign signature (Ed25519)** beside the
index as `index.json.minisig`, checked against a key shipped with Cordial
before the JSON is parsed at all. Chosen because it is boring: one small
well-specified format, an existing Rust implementation, a public key that fits
on a line, and no infrastructure. OpenPGP was rejected for bringing a keyring
and a trust model nobody wants to operate; Sigstore for bringing a network
dependency to an operation whose entire appeal is that it is a static file.
SSH signatures (`ssh-keygen -Y sign`, with an `allowed_signers` file) would also
have done and are a reasonable thing to argue for, since the index lives in git
already.

**Correcting what this ADR said before:** it said "none of it is implemented",
and as of the marketplace foundations (below) that is no longer true of the
*mechanism*. `crate::sign::verify` checks a minisign signature against exactly
the bytes handed to it, in the order this section always specified — before
`serde_json` runs — using the `minisign-verify` crate the paragraph above
already named as the plan. `Index::parse_unverified` is still there, unchanged,
for exactly the case its name always covered: a caller with no signature to
check, which is still every caller in this codebase today, because nothing here
configures a key.

**What is still true, and is the part that actually matters:** no key is
shipped with Cordial, none is hardcoded anywhere, and nothing falls back to
trusting an index that has none configured. Implementing the check without
shipping a key was deliberate, not an oversight — see "Browsing and installing
from a source" below. Shipping a key means naming whose index it belongs to,
which is the hosting decision this ADR has never made and still has not. Until
that decision exists, `crate::marketplace::install` refuses every plan built
from an index it was not handed a key to verify, and an index opened with no
key is exactly as trustworthy as the transport it arrived over — which is what
this paragraph said when there was no check at all, and remains true of every
index nobody has configured a key for.

The per-entry content hash is **not** a substitute and must not be read as one.
It protects the *download* against a mirror that serves different bytes than
were published. It cannot protect against a tampered *index*, because a tampered
index carries a matching hash for whatever it is pointing at.

## Browsing and installing from a source — the foundations, 2026-08-20

The marketplace half of this ADR — browsing and installing from somewhere,
rather than only being able to reason about an index file already on disk —
did not exist. It now does, in three additions that change nothing about the
decisions above; they are the plumbing those decisions were waiting for.

**`crate::source::IndexSource`** is where "who hosts an index" plugs in. Two
calls — fetch the index document (and its signature, if the source has one),
fetch one entry's archive — because those are the only two things ADR-014
ever needs moved over a network. `crate::source::LocalFileSource` is the one
implementation today: a directory holding `index.json`, an optional
`index.json.minisig`, and an `archives/` directory. This is not a stand-in for
an HTTPS source waiting to be written; the "why the index is a static file"
section above already designs an index to be "served straight out of a git
repository", and a local clone of one is that arrangement with the network
step removed. `cordial-plugins` still takes on no HTTP client dependency —
doing that now, ahead of a host being named, would itself be a form of the
policy call this ADR keeps declining to make.

**`crate::sign`** is the signature check described above, as code.

**`crate::marketplace`** is what an installer actually calls: open a source
against an optional trusted key (`open`), and resolve a request against the
result, fetch what the plan needs, and install every step through
`crate::unpack::install` — never a second extraction path — while refusing
outright, before a single byte is fetched, if the index behind the plan was
never verified (`install`). `crates/cordial-shell/src/settings.rs`'s
Marketplace section in the Plugins page is the UI wired to this: a directory
and an optional pasted key, a listing of what it offers with each entry's
requested capabilities shown in the same words the per-plugin grant switches
use, and an Install button that is refused — visibly, with the reason stated
on it — for every entry until a key is configured and verifies.

**What this still does not do, on purpose.** No default index directory, no
shipped key, no HTTPS fetcher, and — see [ADR-001](ADR-001-in-process-hooking.md)
and [ADR-003](ADR-003-plugin-isolation.md) — nothing that would let an index
install anything other than a Cordial plugin archive through the existing
hardened unpacker: there is no broader "run this" primitive here for a
listing to point at, which matters more once a marketplace exists than before
one did, because a marketplace is exactly the shape a distribution channel for
one would try to wear.

## Resolution, and how its order relates to ADR-006's start order

They are the same order, deliberately.

[ADR-006](ADR-006-plugin-events-and-first-party.md) already establishes that
plugins have a start order: `events.subscribe` filters at subscribe time
against types that have already been declared, so a subscriber whose declarer
has not started is refused rather than parked. That ADR describes dependency
resolution for first-party plugins as "resolved once, shared, not restarted per
dependent" — an ESM import graph.

The install order this ADR produces is a topological order of the same graph,
dependencies first, and it is the order to start them in as well. Producing two
orders from one graph would be two chances to disagree, and the disagreement
would surface as a plugin that installs cleanly and then fails to subscribe on
every launch until something unrelated changed the enumeration order. The one
difference is a filter, not a second order: steps already installed at exactly
the planned version need no download, but they are still in the order, because
what is already on disk still has to start first.

**A plugin lives at `plugins/<id>/`, so exactly one version of it can win.**
Two dependents wanting incompatible versions is refused, naming both
requirements and what was on offer. Installing two copies was rejected: it
would give one id two event namespaces and two grant entries, and ADR-006's
registry attributes an event type to an id, not to a directory.

**Capability approval is a second call, on purpose.** A user cannot approve a
plan they have not been shown, and the plan is what resolution produces. So
`resolve` builds it and `Plan::refuse_ungranted` refuses it, and the combined
`plan` is what an installer calls. The refusal that matters is the transitive
one: installing A must never quietly bring in B holding `assets.override`.
Nothing here widens a grant to make an install smoother. A dependency's
capabilities are the user's to grant, exactly as ADR-003 and ADR-013 say, and
arriving as somebody else's dependency is precisely the case where that is
easiest to lose sight of.

**The index's claims are checked against the archive.** An index repeats each
plugin's capabilities and dependencies so a user can be shown them before
anything is downloaded. That is only honest if the archive is then held to it,
so the extracted `plugin.json` is compared against the entry it was installed
as — id, version, capabilities and dependencies — and a mismatch refuses the
install. Without that check an entry could ask for `log`, be approved for `log`,
and unpack a manifest requesting `assets.override`, and the approval the user
gave would have been for a different plugin than the one on disk.

## Unpacking: what is refused, and why each

Every one of these is a refusal with a name. An unpacker that silently skips
the entry it did not like produces a plugin directory that is subtly not what
was published, and the person debugging it has nothing to go on.

**An entry is a regular file or a directory, and nothing else.** Symlinks and
hard links are refused *whether or not the target looks like it stays inside*.
`plugin.json -> ../../../etc/passwd` is the obvious case, but `a -> .` followed
by `a/b` is the same attack written as two entries, neither of which is wrong
on its own. Deciding which links are safe means simulating the filesystem the
archive is building; refusing all of them means never having to be right about
that. Device nodes and FIFOs are refused because a plugin has no use for one and
creating one is how an unpacker becomes interesting.

**A path has no `..`, is not absolute, and still lands inside the destination
once normalised.** The third is redundant given the first two and is kept
anyway; it is the check that still holds if somebody later decides a `..` in the
middle of a path is harmless because it cancels out.

**Setuid and setgid are refused rather than stripped.** Stripping would install
the archive anyway. An archive asking for setuid is telling you something about
itself worth stopping for.

**Files are written `0644` and directories `0755` regardless of what the
archive asked for.** Cordial never executes anything out of a plugin directory —
it runs `deno run` against the entry module — so an executable bit could only
ever be useful to something else.

**Entry count and total uncompressed size are both capped.** Zstd compresses a
few gigabytes of zeroes into a few hundred bytes, so the size of what was
downloaded says nothing at all about the size of what is being written. The cap
is enforced against what each header declares *and* against what actually comes
out of the decompressor, so an archive whose headers disagree with its contents
runs into one or the other.

**The content hash is verified before anything is decompressed.** Checking
afterwards would mean a tampered archive had already been through the tar parser
and had already put files on disk, and "we deleted them again" is a much weaker
statement than "they were never written".

**Extraction is staged and renamed into place.** A dot-prefixed sibling of the
final directory, which `manifest::discover` does not look at, renamed in only
once the whole archive has been read and its manifest checked. `install` clears
the staging directory when it refuses — but a process killed part way through
clears nothing, and what is left at that moment is a directory holding a real
`plugin.json` and a truncated entry module. The dot prefix is the only thing
standing between that and Cordial loading half a plugin on the next launch.
`is_valid_id` forbids a dot, so such a directory can never collide with a real
plugin's name.

## What this does not protect against

**A plugin that is exactly what it claims to be, and malicious.** Every
mechanism above answers one question — *are these the bytes that were
published, unpacked without escaping the directory they were meant for?* None
of them answers whether those bytes should be trusted. A hash proves provenance.
It does not prove intent, competence, or that the author has not changed their
mind since version 1.2.

**Capabilities remain the entire boundary**, exactly as
[ADR-003](ADR-003-plugin-isolation.md) and
[ADR-007](ADR-007-host-resources-are-brokered.md) say, and the registry does not
move it an inch. A plugin that is granted `assets.override` can overlay a
gameplay-affecting asset ([ADR-010](ADR-010-plugin-asset-overlays.md) is
explicit about this); one granted `presence.set` can broadcast what someone is
playing; one granted `settings.write` can throw their configuration away. Being
in an index changes none of that. It is the same trust decision the user makes
in approving the capability, and it is still theirs to make.

**Nothing here should be built into a UI that implies otherwise.** A storefront
is a shape that says "reviewed" whether or not anybody reviewed anything, and a
verified badge next to a hash is an invitation to read "safe" where the only
claim being made is "unmodified since publication". Whatever the marketplace UI
turns out to be, it has to show what a plugin is asking for and let the user
decide, rather than presenting listing in an index as an endorsement. If review
ever does happen, it will be a human process with a name attached and it should
say who did it — not a property that follows from being downloadable.

**Nor against a dependency chain nobody read.** Resolution makes the whole plan
visible and refuses ungranted capabilities in it, which is the most a resolver
can do. It cannot make somebody read the list.

## Consequences

**Accepted:** an index duplicates what is already in each archive's manifest.
That duplication is load-bearing — a plan has to be built before anything is
downloaded — and it is checked at install time rather than trusted, so the cost
is a mismatch refusal an honest publisher never sees.

**Accepted:** the resolver refuses more than a general-purpose package manager
would. Incompatible requirements on one dependency, and any cycle, stop the
install rather than being worked around. Both are cheap to fix in a manifest and
expensive to debug once installed.

**Accepted:** four new dependencies (`semver`, `sha2`, `tar`, `zstd`), one of
which builds C. The workspace already requires Clang.

**Accepted:** nothing here downloads anything. Fetching is Cordial's to do and a
plugin never holds the channel (ADR-007); keeping the fetch out of the resolver
and the unpacker is also what makes every refusal above reachable from a test
with no network.

**Rejected: installing two versions of one plugin.** One id, one directory, one
event namespace, one grant entry.

**Rejected: treating an entry the unpacker dislikes as a skippable entry.** The
result is a plugin directory that is quietly not what was published.

**Rejected: a `file:` or plain-HTTP download URL in a published index.** The
hash is what makes a download trustworthy, so `https` is defence in depth rather
than the guarantee — but a URL is the one field a fetcher acts on, and the set
of schemes it can be talked into is worth being a short list.

## What would change this

If the project owner names a host for a default index, two things follow and
neither is optional: a key for `crate::sign` to check it against, shipped and
documented as belonging to that specific publisher, and an
`IndexSource` implementation that fetches over HTTPS rather than reading a
local directory. Neither is a large change against what exists now —
`crate::marketplace` already refuses to install without a verified signature,
and `crate::source::IndexSource` is already the seam an HTTPS fetcher drops
into — but both are genuine decisions, made once, by the person who has to
stand behind them, not defaults quietly picked to make the marketplace UI feel
less empty.

If a plugin ever legitimately needs two incompatible versions of a dependency —
which would mean plugins linking each other's code rather than merely starting
in an order — the one-directory-per-id rule is what would have to give, and that
is a much larger change than a resolver tweak.
