#!/usr/bin/env -S deno run --allow-read=tools/sober-corpus/data --allow-write=tools/sober-corpus/data
/**
 * Derives the triage set from the raw Sober issue corpus (`data/raw.jsonl`).
 * Separate, re-runnable step from `fetch.ts` on purpose: changing the
 * derivation or quality-filter logic should never require re-hitting the
 * GitHub API.
 *
 * A derived case pairs a problem statement (title + body + any
 * non-maintainer comments posted before the first substantive maintainer
 * reply) with the expected resolution (the substantive maintainer
 * reply/replies — the strongest ground-truth signal this tracker has,
 * especially the ones immediately preceding closure). "Maintainer" =
 * GitHub's own `authorAssociation` of OWNER/MEMBER/COLLABORATOR; see
 * ADR-017.
 *
 * Quality filter (see `triage.ts`): an issue only survives into the derived
 * set if at least one maintainer reply is substantive — not pure triage
 * noise ("duplicate of #123", "please fill the template", a bare
 * "wontfix"). This also structurally excludes NOT_PLANNED/DUPLICATE
 * closures that never got a real explanation, without needing a separate
 * stateReason-specific rule: those closures typically ARE the triage
 * comment this filter targets.
 *
 * Usage: just sober-corpus-derive
 *
 * @module sober-corpus/derive
 */

import { loadRawCorpus, saveEvalSet } from "./storage.ts";
import { isTriageComment } from "./triage.ts";
import { isMaintainerAssociation } from "./types.ts";
import type { CommentRecord, EvalCase, IssueRecord } from "./types.ts";

function buildEvalCase(issue: IssueRecord): EvalCase | null {
  const sortedComments = [...issue.comments].sort((a, b) => a.createdAt.localeCompare(b.createdAt));
  const maintainerComments = sortedComments.filter((c) => isMaintainerAssociation(c.authorAssociation));
  const substantive = maintainerComments.filter((c) => !isTriageComment(c.body));

  if (substantive.length === 0) {
    return null;
  }

  const cutoff = substantive[0]!.createdAt;
  const clarifying: CommentRecord[] = sortedComments.filter(
    (c) => !isMaintainerAssociation(c.authorAssociation) && c.createdAt < cutoff,
  );

  const problemParts = [issue.title, issue.body, ...clarifying.map((c) => c.body)].filter(
    (part) => part && part.trim().length > 0,
  );

  return {
    issueNumber: issue.number,
    title: issue.title,
    problemStatement: problemParts.join("\n\n---\n\n"),
    expectedResolution: substantive.map((c) => c.body).join("\n\n---\n\n"),
    maintainerReplies: maintainerComments.map((c) => ({
      authorAssociation: c.authorAssociation,
      createdAt: c.createdAt,
      body: c.body,
      precedesClosure: issue.closedAt !== null && c.createdAt <= issue.closedAt,
    })),
    state: issue.state,
    stateReason: issue.stateReason,
    labels: issue.labels,
    createdAt: issue.createdAt,
    updatedAt: issue.updatedAt,
    closedAt: issue.closedAt,
  };
}

function main(): void {
  const corpus = loadRawCorpus();
  const issues = [...corpus.values()];

  const evalCases: EvalCase[] = [];
  let excludedNoMaintainerReply = 0;
  let excludedOnlyTriageReplies = 0;

  for (const issue of issues) {
    const hasAnyMaintainerComment = issue.comments.some((c) => isMaintainerAssociation(c.authorAssociation));
    if (!hasAnyMaintainerComment) {
      excludedNoMaintainerReply++;
      continue;
    }
    const evalCase = buildEvalCase(issue);
    if (!evalCase) {
      excludedOnlyTriageReplies++;
      continue;
    }
    evalCases.push(evalCase);
  }

  evalCases.sort((a, b) => a.issueNumber - b.issueNumber);
  saveEvalSet(evalCases);

  console.log(`\nDerived triage set from ${issues.length} issues in the raw corpus`);
  console.log(`  Excluded — no maintainer reply at all:        ${excludedNoMaintainerReply}`);
  console.log(`  Excluded — maintainer replied, all triage:    ${excludedOnlyTriageReplies}`);
  console.log(`  Survived filter (cases written):              ${evalCases.length}`);
  console.log(`  -> tools/sober-corpus/data/eval-set.jsonl`);
}

main();
