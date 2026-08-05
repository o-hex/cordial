# Sober issue corpus

A local, incrementally-updated copy of [vinegarhq/sober](https://github.com/vinegarhq/sober)'s
issue tracker. Sober is another project running Roblox on Linux — closed
source, so its GitHub repo is purely an issue tracker — which means it is
prior art: real users hitting real problems on the same engine Cordial
loads, with a maintainer's actual answer attached to most of them. The point
is to give a Cordial maintainer something to search when triaging a new
issue: has someone already hit this against Sober, and what did Sober's
maintainer say. See [ADR-017](../../docs/adr/ADR-017-sober-issue-corpus.md)
for the full reasoning.

This is not code review or reverse engineering of Sober — Sober has no
source to read, only issue text. It is not customer-facing and never
committed; see "The data is gitignored" below.

## Running it

```
just sober-corpus-fetch    # pulls new/changed issues since the last run
just sober-corpus-derive   # derives the triage set from the raw corpus
```

`fetch` needs a GitHub token. It reads `GITHUB_TOKEN` or `GH_TOKEN` from the
environment first; if neither is set, it shells out to `gh auth token`. The
token is never written to disk or logged.

A cold run pages through every issue in the repo (a few dozen GraphQL
requests, well under 100 points of a 5,000/hour budget). Every later run is
incremental: it pages from the newest issue backwards and stops the instant
it reaches one it has already seen, so a fully up-to-date corpus costs one
request. A run can be killed at any point — `SIGKILL` included — and the
next run resumes from the last completed page rather than starting over.

## What it produces

- `data/raw.jsonl` — one JSON object per Sober issue: title, body, state,
  labels, and the full comment thread, each comment tagged with the
  commenter's GitHub `authorAssociation` (OWNER/MEMBER/COLLABORATOR reads
  as "maintainer"; nothing else does). No usernames are stored — only
  whether a reply was authoritative, never who wrote it.
- `data/checkpoint.json` — the incremental fetcher's resumption state: the
  high-water mark from the last completed pass, and the in-progress cursor
  if a run was interrupted.
- `data/eval-set.jsonl` — `derive`'s output: one entry per issue that got a
  *substantive* maintainer reply (see `triage.ts` for what counts as
  triage noise — "duplicate of #123", "please fill the template" — and gets
  filtered out), pairing the problem statement with the maintainer's actual
  resolution. This is the file worth grepping when triaging a new Cordial
  issue.

## The data is gitignored

`data/` never leaves this machine. These are other people's GitHub bug
reports with no license grant to Cordial, and Cordial is a public repo.
Every issue and comment body is PII-redacted on write, before anything
touches disk (`pii.ts`) — emails, `/home/<user>/` paths, IPv4 addresses, and
shell-prompt/`Host:`-style hostnames. That redaction is defence in depth,
not a reason to commit: the repo's `.gitignore` entry for `tools/sober-corpus/data/`
is what actually keeps this off GitHub.

## Issues only, never pull requests

GitHub's REST `/issues` endpoint returns pull requests mixed in (a PR is an
issue in GitHub's data model). This fetcher uses the GraphQL API's
`repository.issues` connection instead, which does not, so nothing here
needs a PR filter bolted on after the fact.

## Files

| File | Job |
|---|---|
| `fetch.ts` | Entry point. Pages the GraphQL API, redacts, writes the checkpoint and raw corpus. |
| `github-graphql.ts` | The GraphQL client: auth, retries, rate-limit accounting. |
| `storage.ts` | All disk I/O — atomic writes, corpus/checkpoint load and save. |
| `types.ts` | Shared types for a raw issue record, the checkpoint, and a derived case. |
| `pii.ts` | Secret and PII redaction, run on every title/body/comment before it is written. |
| `triage.ts` | Heuristic filter for "maintainer replied but said nothing" comments. |
| `derive.ts` | Entry point. Builds `eval-set.jsonl` from `raw.jsonl`; never calls GitHub. |

Deno, not Node: this is a Deno TypeScript tool because Deno is already a
Cordial dependency for the plugin runtime (ADR-008), so nothing new gets
installed to run it. `deno fmt`'s default line width was not applied
uniformly — the existing plugin code under `plugins/` isn't `deno fmt`-clean
either, and forcing it here would have broken the aligned progress-log
strings and comments for no benefit.
