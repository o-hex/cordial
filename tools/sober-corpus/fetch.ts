#!/usr/bin/env -S deno run --allow-net=api.github.com --allow-env=GITHUB_TOKEN,GH_TOKEN --allow-run=gh --allow-read=tools/sober-corpus/data --allow-write=tools/sober-corpus/data
/**
 * Incremental, resumable fetcher for every issue + comment thread in
 * `vinegarhq/sober` — another project running Roblox on Linux, whose GitHub
 * repo is purely an issue tracker (Sober is closed-source; there is no code
 * here to read, only real users hitting real problems on the same engine
 * Cordial loads, with maintainer answers attached). Builds a local corpus
 * (`tools/sober-corpus/data/raw.jsonl`, gitignored) a Cordial maintainer can
 * search when triaging a new issue: has someone already hit this, and what
 * did Sober's maintainer say. See ADR-017.
 *
 * Incrementality (the core requirement) works like this: GitHub's `issues`
 * connection is paged newest-updated-first (`orderBy: UPDATED_AT DESC`).
 * Every completed pass records a `highWaterMark` — the max `updatedAt` seen
 * across the whole corpus. The NEXT pass pages from the newest issue
 * backwards and stops the INSTANT it reaches an issue whose `updatedAt` is
 * already <= that high-water mark — everything past that point is
 * already-seen and unchanged, so there's no reason to keep paging. A fully
 * up-to-date corpus therefore costs one GraphQL request per re-run (see
 * `github-graphql.ts` for the point-cost accounting). A checkpoint
 * (`data/checkpoint.json`) is written after EVERY page, not just at the end,
 * so a kill mid-run loses at most the in-flight page — the next run resumes
 * from the saved `endCursor`, not from scratch.
 *
 * Usage:
 *   just sober-corpus-fetch
 *   GITHUB_TOKEN=ghp_xxx just sober-corpus-fetch   # instead of `gh auth token`
 *
 * @module sober-corpus/fetch
 */

import { emitEvent, fetchIssuesPage, fetchRemainingComments, type IssueNode } from "./github-graphql.ts";
import { redactIssuePII } from "./pii.ts";
import { loadCheckpoint, loadRawCorpus, saveCheckpoint, saveRawCorpus } from "./storage.ts";
import type { CommentRecord, IssueRecord } from "./types.ts";

const OWNER = "vinegarhq";
const REPO = "sober";
const PAGE_SIZE = 100;

async function buildIssueRecord(node: IssueNode): Promise<IssueRecord> {
  let commentNodes = node.comments.nodes;
  if (node.comments.pageInfo.hasNextPage && node.comments.pageInfo.endCursor) {
    emitEvent("sober_corpus.fetch.comment_overflow", {
      issue: node.number,
      totalComments: node.comments.totalCount,
    });
    const rest = await fetchRemainingComments({
      owner: OWNER,
      name: REPO,
      number: node.number,
      cursor: node.comments.pageInfo.endCursor,
    });
    commentNodes = [...commentNodes, ...rest];
  }

  const comments: CommentRecord[] = commentNodes.map((c) => ({
    authorAssociation: c.authorAssociation as CommentRecord["authorAssociation"],
    createdAt: c.createdAt,
    body: redactIssuePII(c.body ?? ""),
  }));

  return {
    number: node.number,
    title: redactIssuePII(node.title ?? ""),
    body: redactIssuePII(node.body ?? ""),
    state: node.state,
    stateReason: node.stateReason as IssueRecord["stateReason"],
    authorAssociation: node.authorAssociation as IssueRecord["authorAssociation"],
    labels: node.labels.nodes.map((l) => l.name),
    createdAt: node.createdAt,
    updatedAt: node.updatedAt,
    closedAt: node.closedAt,
    commentCount: comments.length,
    comments,
    fetchedAt: new Date().toISOString(),
  };
}

async function main(): Promise<void> {
  const startedAt = Date.now();
  const checkpoint = loadCheckpoint();
  const corpus = loadRawCorpus();

  console.log(`\nSober issue corpus fetch — ${OWNER}/${REPO}`);
  console.log(`  Corpus on disk: ${corpus.size} issue(s)`);
  console.log(`  High-water mark: ${checkpoint.highWaterMark ?? "(none — first run)"}`);

  if (!checkpoint.inProgress) {
    checkpoint.inProgress = {
      cursor: null,
      boundary: checkpoint.highWaterMark,
      candidateHighWaterMark: null,
      pagesFetchedThisPass: 0,
      issuesWrittenThisPass: 0,
      startedAt: new Date().toISOString(),
    };
    saveCheckpoint(checkpoint);
  } else {
    console.log(
      `  Resuming an interrupted pass: ${checkpoint.inProgress.pagesFetchedThisPass} page(s) / ` +
        `${checkpoint.inProgress.issuesWrittenThisPass} issue(s) already done this pass.`,
    );
  }

  const pass = checkpoint.inProgress;
  let stopped = false;
  let totalCost = 0;

  while (!stopped) {
    const page = await fetchIssuesPage({ owner: OWNER, name: REPO, pageSize: PAGE_SIZE, cursor: pass.cursor });
    totalCost += page.rateLimit.cost;

    for (const node of page.nodes) {
      if (pass.boundary !== null && node.updatedAt <= pass.boundary) {
        stopped = true;
        break;
      }
      if (pass.candidateHighWaterMark === null) {
        pass.candidateHighWaterMark = node.updatedAt;
      }
      const record = await buildIssueRecord(node);
      corpus.set(record.number, record);
      pass.issuesWrittenThisPass++;
    }

    pass.pagesFetchedThisPass++;
    if (!stopped) {
      pass.cursor = page.pageInfo.endCursor;
    }

    // Durability order matters: persist the issues BEFORE advancing the
    // checkpoint, so a kill between the two just re-does (harmlessly
    // overwrites) this page next run rather than silently skipping it.
    saveRawCorpus(corpus);
    saveCheckpoint(checkpoint);

    console.log(
      `  page ${pass.pagesFetchedThisPass}: +${pass.issuesWrittenThisPass} new/changed this pass ` +
        `(corpus now ${corpus.size}) — cost ${page.rateLimit.cost}, ${page.rateLimit.remaining} pts remaining`,
    );

    if (!stopped && !page.pageInfo.hasNextPage) {
      stopped = true;
    }
  }

  checkpoint.highWaterMark = pass.candidateHighWaterMark ?? checkpoint.highWaterMark;
  checkpoint.lastCompletedRunAt = new Date().toISOString();
  checkpoint.totalIssuesInCorpus = corpus.size;
  checkpoint.inProgress = null;
  saveCheckpoint(checkpoint);

  const elapsedS = ((Date.now() - startedAt) / 1000).toFixed(1);
  console.log(
    `\nDone in ${elapsedS}s. Corpus: ${corpus.size} issues. ` +
      `This run: ${pass.issuesWrittenThisPass} new/changed, ${pass.pagesFetchedThisPass} page(s), ` +
      `~${totalCost} GraphQL point(s).\n` +
      `Next: just sober-corpus-derive`,
  );
  emitEvent("sober_corpus.fetch.completed", {
    corpusSize: corpus.size,
    issuesThisPass: pass.issuesWrittenThisPass,
    pagesThisPass: pass.pagesFetchedThisPass,
    pointCost: totalCost,
    elapsedS,
  });
}

main().catch((error) => {
  emitEvent("sober_corpus.fetch.failed", {
    message: error instanceof Error ? error.message : String(error),
  });
  console.error(`\nFetch failed: ${error instanceof Error ? error.message : String(error)}`);
  Deno.exit(1);
});
