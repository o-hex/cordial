# ADR-017: A local, incremental corpus of Sober's issue tracker

**Status:** accepted
**Related:** [ADR-008](ADR-008-plugins-are-typescript-on-deno.md), [ADR-015](ADR-015-fetching-the-roblox-build.md)

## Context

[vinegarhq/sober](https://github.com/vinegarhq/sober) is a different project
running Roblox on Linux. It is closed source, so its GitHub repo is purely
an issue tracker — but that tracker is prior art in a narrow, valuable
sense: real users hitting real problems on the same engine Cordial loads,
with a maintainer's actual answer attached to most of them. When a new
Cordial issue looks like a graphics glitch, an audio failure, or a login
hang, the useful question is often "has someone already hit this against
Sober, and what fixed it" — and until now the only way to answer that was
to search GitHub by hand, every time.

This needed a fetcher, not a one-off scrape: the corpus is worth keeping
current as Sober's tracker grows, and re-fetching all 2,000+ issues on every
run would be wasteful and slow for no reason a checkpoint cannot avoid.

## Decision

`tools/sober-corpus/` is a small set of Deno TypeScript scripts —
`fetch.ts`, `github-graphql.ts`, `storage.ts`, `types.ts`, `pii.ts`,
`derive.ts`, `triage.ts` — that pull vinegarhq/sober's issues into a local,
PII-redacted, gitignored corpus a maintainer can grep. `just
sober-corpus-fetch` runs it; `just sober-corpus-derive` builds a filtered
triage set from the raw corpus. See `tools/sober-corpus/README.md` for the
day-to-day usage and file layout; this ADR is the why, not the how.

**GraphQL, not REST.** REST's `/issues` + `/issues/{n}/comments` shape
needs a request per issue just for comments; GraphQL nests `comments`
inside `issues`, so the whole repo pages in a few dozen requests instead of
thousands. It also matters for correctness, not only cost: REST's
`/issues` endpoint returns pull requests mixed in with issues, because a PR
*is* an issue in GitHub's data model. GraphQL's `repository.issues`
connection does not, so this fetcher structurally cannot leak a PR into the
corpus — there is no filter to forget, because the query never asks for
one.

**Incremental by construction, not by a bolted-on diff.** Issues page
newest-updated-first. Every completed pass records the maximum `updatedAt`
seen as a high-water mark; the next pass pages backwards from the newest
issue and stops the instant it reaches one already at or before that mark —
everything past that point is unchanged. A checkpoint is written after
*every page*, not just at the end, so a kill mid-run (`SIGKILL` included)
loses at most the in-flight page; the next run resumes from the saved
cursor rather than starting over. A fully up-to-date corpus costs one
GraphQL request per re-run.

**PII redaction before anything touches disk, unconditionally.** These are
other people's bug reports, routinely pasted straight from a terminal —
`inxi`/`neofetch`/`journalctl` dumps, stack traces, shell history — which
means emails, `/home/<user>/` paths, IP addresses, and shell-prompt/`Host:`
hostnames show up constantly. `pii.ts` strips all of it, plus
secret-shaped substrings (bearer tokens, provider API keys, JWTs, URL
userinfo credentials), on write, before a record is ever appended to
`raw.jsonl`. This matters more here than a typical internal log-redaction
pass would: Cordial is a public repository, and this corpus is third-party
users' content with no license grant to Cordial. Redaction is defence in
depth; the actual thing that keeps this data off GitHub is the next
paragraph.

**The corpus never leaves the machine.** `tools/sober-corpus/data/` is
gitignored. 17 MB (at the corpus size measured for this ADR) of other
people's issue text does not belong in this repo's history, redacted or
not — it is a local triage aid, not a redistributed artefact, in the same
sense ADR-015 draws for a fetched Roblox build: fetching something for
local use is not the same act as shipping it.

**Deno, not Node.** This reaffirms ADR-008 rather than opening a new
question: Deno is already a Cordial dependency for the plugin runtime, so a
Deno tool here adds no new toolchain, no `package.json`, no
`node_modules`. A Node/`tsx` port would have added exactly the dependency
ADR-008 chose not to carry, for a tool that gains nothing from being Node.
Deno's built-in `fetch` also means the GraphQL client needs no HTTP
library at all.

## What this deliberately does not do

An earlier version of this idea paired the fetched corpus with an
LLM-scoring harness — evaluate a subject model's diagnosis against the
maintainer's real one, judge it, and triage which findings apply to
Cordial. That harness (`evaluate.ts`, `model.ts`, `sampling.ts`, and a
Cordial-specific context builder) is not part of this change. It called an
external LLM API, which is out of scope here on its own terms, and was
explicitly not something to build. What ships is the fetcher and the
maintainer-reply quality filter (`triage.ts`) that makes the raw corpus
worth searching by hand — nothing that scores or judges anything
automatically.

## Evidence

**Measured, this session:** a cold run against vinegarhq/sober's live
tracker: 91.9s, 22 pages, ~44 GraphQL points of a 5,000/hour budget, 2,195
issues. A warm re-run immediately after, with nothing changed upstream: 1
page, 0 new/changed, ~2 points, 4.1s — confirming the incremental path is
near-free, not merely designed to be.

**Measured:** killing the fetcher mid-run with `SIGKILL` (confirmed dead:
process exit 137) left `checkpoint.json` recording an interrupted pass at
page 3 / 300 issues, matching `raw.jsonl`'s 300 lines exactly. Re-running
printed "Resuming an interrupted pass: 3 page(s) / 300 issue(s) already
done this pass" and continued from page 4 rather than restarting — the
checkpoint scheme works as designed, not merely as intended.

**Measured:** `grep -o '/home/\[redacted-user\]' raw.jsonl | wc -l` found
1,786 redactions across the full corpus; a targeted search for `/home/`
substrings *not* followed by the redaction marker turned up exactly one
non-match, and it was not a leak — the underlying text was a user's own
malformed `$HOME` environment variable containing the literal string
`"me"` (quotes included), not a real username, in an issue reporting a
Flatpak path-parsing bug.

**Measured:** `gh api "repos/vinegarhq/sober/pulls?state=all&per_page=100"
--paginate` lists the repository's 20 real pull requests (open and
closed); cross-referencing those numbers against all 2,195 fetched issue
numbers found zero overlap, confirming the GraphQL `issues` connection
this fetcher uses does not leak pull requests into the corpus.

## Consequences

**Accepted:** the corpus is a maintenance surface tied to Sober's tracker
staying reachable and GitHub's GraphQL schema staying stable. Neither is
under Cordial's control; if either breaks, the fetcher should fail loudly
naming what it could not reach, the same standard ADR-015 sets for the
build fetcher.

**Accepted:** this is a best-effort redaction pass, not a guarantee.
Freeform pasted logs can leak PII in shapes no regex list fully covers
(`pii.ts`'s module doc says so directly). It covers the shapes actually
observed in this tracker's issues; it is not a substitute for keeping the
output out of the repository, which the `.gitignore` entry does
unconditionally regardless of redaction quality.

**Rejected, for now:** the LLM-scoring harness described above. Nothing
here forecloses building it later as its own, separately-decided piece of
work — this ADR only covers the fetcher and the local triage set it
produces.

## What would change this

If Sober's tracker or GitHub's GraphQL API changes shape in a way that
breaks pagination or the point-cost model this fetcher assumes. If the
corpus needs to be shared beyond one machine, which would need a real
decision about where it lives and who can read it — not simply removing
the gitignore entry.
