/**
 * Shared types for the Sober issue corpus fetcher and eval-set derivation.
 *
 * "Maintainer" throughout this module means GitHub's own `authorAssociation`
 * classification of OWNER, MEMBER, or COLLABORATOR — never a hardcoded
 * username. See ADR-017 for why this corpus exists and what it is for.
 *
 * @module sober-corpus/types
 */

/** GitHub's `CommentAuthorAssociation` enum, verified live against the GraphQL schema. */
export type AuthorAssociation =
  | "OWNER"
  | "MEMBER"
  | "COLLABORATOR"
  | "CONTRIBUTOR"
  | "FIRST_TIME_CONTRIBUTOR"
  | "FIRST_TIMER"
  | "MANNEQUIN"
  | "NONE";

/** GitHub's `IssueStateReason` enum, verified live against the GraphQL schema. */
export type IssueStateReason = "COMPLETED" | "NOT_PLANNED" | "DUPLICATE" | "REOPENED" | null;

export const MAINTAINER_ASSOCIATIONS: ReadonlySet<AuthorAssociation> = new Set([
  "OWNER",
  "MEMBER",
  "COLLABORATOR",
]);

export function isMaintainerAssociation(association: AuthorAssociation): boolean {
  return MAINTAINER_ASSOCIATIONS.has(association);
}

/**
 * A single comment on an issue. Deliberately has no `login`/`author` field —
 * a maintainer triaging a Cordial issue needs to know WHETHER a reply is
 * authoritative (authorAssociation), never WHO wrote it.
 */
export interface CommentRecord {
  authorAssociation: AuthorAssociation;
  createdAt: string;
  /** PII-redacted at write time — see pii.ts. */
  body: string;
}

/** Faithful, redacted capture of one GitHub issue. One JSON object per line in raw.jsonl. */
export interface IssueRecord {
  number: number;
  title: string;
  /** PII-redacted at write time. */
  body: string;
  state: "OPEN" | "CLOSED";
  stateReason: IssueStateReason;
  authorAssociation: AuthorAssociation;
  labels: string[];
  createdAt: string;
  updatedAt: string;
  closedAt: string | null;
  commentCount: number;
  comments: CommentRecord[];
  /** When this snapshot of the issue was captured (not a GitHub field). */
  fetchedAt: string;
}

/** Persisted resumption state. Written after every page, not just at the end. */
export interface Checkpoint {
  schemaVersion: 1;
  /** Max `updatedAt` across the whole corpus as of the last COMPLETED pass. null before the first ever run. */
  highWaterMark: string | null;
  lastCompletedRunAt: string | null;
  totalIssuesInCorpus: number;
  /** Non-null only while a pass is actively running or was interrupted mid-pass. */
  inProgress: InProgressPass | null;
}

export interface InProgressPass {
  /** endCursor to resume `after:` from. null means "start of this pass". */
  cursor: string | null;
  /** highWaterMark frozen at the moment this pass started; stop once we see updatedAt <= boundary. */
  boundary: string | null;
  /** updatedAt of the first (i.e. most-recently-updated) issue seen this pass; becomes the new highWaterMark on completion. */
  candidateHighWaterMark: string | null;
  pagesFetchedThisPass: number;
  issuesWrittenThisPass: number;
  startedAt: string;
}

export function emptyCheckpoint(): Checkpoint {
  return {
    schemaVersion: 1,
    highWaterMark: null,
    lastCompletedRunAt: null,
    totalIssuesInCorpus: 0,
    inProgress: null,
  };
}

/**
 * One derived triage case: a Sober problem report paired with the
 * maintainer's actual resolution — the thing a Cordial maintainer greps for
 * when a new issue looks familiar. "Maintainer" is GitHub's own
 * `authorAssociation` of OWNER/MEMBER/COLLABORATOR; see triage.ts for the
 * substantive-reply filter that decides what survives into this set.
 */
export interface EvalCase {
  issueNumber: number;
  title: string;
  /** Title + body + clarifying non-maintainer comments prior to the first substantive maintainer reply. */
  problemStatement: string;
  /** Concatenated substantive (non-triage) maintainer reply bodies, in order. */
  expectedResolution: string;
  maintainerReplies: Array<{
    authorAssociation: AuthorAssociation;
    createdAt: string;
    body: string;
    precedesClosure: boolean;
  }>;
  state: "OPEN" | "CLOSED";
  stateReason: IssueStateReason;
  labels: string[];
  createdAt: string;
  updatedAt: string;
  closedAt: string | null;
}
