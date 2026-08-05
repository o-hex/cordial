/**
 * Minimal GitHub GraphQL client for the Sober issue corpus fetcher.
 *
 * Why GraphQL instead of REST: REST's `/issues` + `/issues/{n}/comments`
 * shape needs one request per issue just for comments — 2,194 issues would
 * be 2,194+ requests. GraphQL nests `comments` inside `issues` in a single
 * query, so the whole repo can be paged in a few dozen requests. REST's
 * `/issues` endpoint also returns pull requests mixed in with issues (a PR
 * is an issue in GitHub's data model); GraphQL's `repository.issues`
 * connection does not, which is why this corpus can never leak a PR without
 * the query itself changing to ask for one.
 *
 * Rate limiting: GitHub's GraphQL API is POINT-based, not request-based
 * (verified live against a token: `rateLimit { cost }` on a 100-issue page
 * with full comments+labels reported `cost: 2`, matching the documented
 * formula — sum of `first`/`last` values per connection, divided by 100,
 * rounded, minimum 1). Every query below requests `rateLimit { remaining
 * resetAt }` inline so budget is tracked for free, with no extra request.
 * Budget is 5,000 points/hour for a PAT; a full cold run of this fetcher
 * against vinegarhq/sober costs well under 100 points.
 *
 * @module sober-corpus/github-graphql
 */

const GITHUB_GRAPHQL_URL = "https://api.github.com/graphql";
const MAX_RETRIES = 8;
const SECONDARY_LIMIT_BASE_BACKOFF_MS = 30_000;
// Stop proactively once budget gets this low, rather than racing the primary limit to zero.
const RATE_LIMIT_SAFETY_MARGIN = 50;

export interface RateLimitInfo {
  limit: number;
  cost: number;
  remaining: number;
  resetAt: string;
}

export interface CommentNode {
  authorAssociation: string;
  createdAt: string;
  body: string;
}

export interface IssueNode {
  number: number;
  title: string;
  body: string;
  state: "OPEN" | "CLOSED";
  stateReason: string | null;
  authorAssociation: string;
  createdAt: string;
  updatedAt: string;
  closedAt: string | null;
  labels: { nodes: Array<{ name: string }> };
  comments: {
    totalCount: number;
    pageInfo: { hasNextPage: boolean; endCursor: string | null };
    nodes: CommentNode[];
  };
}

export interface IssuesPage {
  pageInfo: { hasNextPage: boolean; endCursor: string | null };
  nodes: IssueNode[];
  rateLimit: RateLimitInfo;
}

let cachedToken: string | null = null;

/** Resolves a GitHub token from `GITHUB_TOKEN` or `GH_TOKEN`, falling back to `gh auth token`. Cached after first resolution. */
export function resolveGitHubToken(): string {
  if (cachedToken) {
    return cachedToken;
  }
  const envToken = (Deno.env.get("GITHUB_TOKEN") ?? Deno.env.get("GH_TOKEN"))?.trim();
  if (envToken) {
    cachedToken = envToken;
    return cachedToken;
  }
  try {
    const result = new Deno.Command("gh", { args: ["auth", "token"] }).outputSync();
    if (result.success) {
      const token = new TextDecoder().decode(result.stdout).trim();
      if (token) {
        cachedToken = token;
        return cachedToken;
      }
    }
  } catch {
    // fall through to the error below
  }
  throw new Error(
    "No GitHub token available. Set GITHUB_TOKEN or GH_TOKEN, or run `gh auth login` so `gh auth token` succeeds.",
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function sleepUntil(isoTimestamp: string, bufferMs = 2000, onWait?: (waitMs: number) => void): Promise<void> {
  const waitMs = new Date(isoTimestamp).getTime() - Date.now() + bufferMs;
  if (waitMs > 0) {
    onWait?.(waitMs);
    await sleep(waitMs);
  }
}

interface GraphQLResponse<T> {
  data?: T;
  errors?: Array<{ type?: string; message: string }>;
}

/** Emits a structured, single-line JSON event to stderr, so progress on stdout and machine-greppable events stay in separate streams. */
export function emitEvent(event: string, fields: Record<string, unknown> = {}): void {
  console.error(JSON.stringify({ event, ...fields }));
}

/** POSTs a GraphQL query, handling primary rate-limit throttling, secondary (abuse-detection) backoff, and transient 5xx retries. */
async function graphqlRequest<T>(
  query: string,
  variables: Record<string, unknown>,
): Promise<{ data: T; rateLimit: RateLimitInfo }> {
  const token = resolveGitHubToken();

  for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
    const res = await fetch(GITHUB_GRAPHQL_URL, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
        "User-Agent": "cordial-sober-corpus-fetcher",
      },
      body: JSON.stringify({ query, variables }),
    });

    if (res.status === 403 || res.status === 429) {
      const retryAfterHeader = res.headers.get("retry-after");
      const waitMs = retryAfterHeader
        ? Number(retryAfterHeader) * 1000
        : SECONDARY_LIMIT_BASE_BACKOFF_MS * 2 ** attempt;
      emitEvent("sober_corpus.rate_limit.secondary", { attempt, waitMs, status: res.status });
      if (attempt === MAX_RETRIES) {
        throw new Error(`GitHub GraphQL secondary rate limit exceeded after ${MAX_RETRIES} retries`);
      }
      await sleep(waitMs);
      continue;
    }

    if (res.status >= 500) {
      if (attempt === MAX_RETRIES) {
        throw new Error(`GitHub GraphQL request failed after ${MAX_RETRIES} retries: HTTP ${res.status}`);
      }
      const waitMs = 2000 * 2 ** attempt;
      emitEvent("sober_corpus.retry.transient_error", { attempt, status: res.status, waitMs });
      await sleep(waitMs);
      continue;
    }

    if (!res.ok) {
      const bodyText = await res.text().catch(() => "");
      throw new Error(`GitHub GraphQL request failed: HTTP ${res.status} ${bodyText.slice(0, 500)}`);
    }

    const body = (await res.json()) as GraphQLResponse<T>;

    if (body.errors && body.errors.length > 0) {
      const rateLimited = body.errors.some((e) => e.type === "RATE_LIMITED");
      if (rateLimited && attempt < MAX_RETRIES) {
        emitEvent("sober_corpus.rate_limit.primary_graphql_error", { attempt });
        await sleep(60_000);
        continue;
      }
      throw new Error(`GitHub GraphQL errors: ${JSON.stringify(body.errors)}`);
    }

    if (!body.data) {
      throw new Error("GitHub GraphQL response had no data and no errors");
    }

    const rateLimit = (body.data as Record<string, unknown>).rateLimit as RateLimitInfo | undefined;
    if (rateLimit && rateLimit.remaining < RATE_LIMIT_SAFETY_MARGIN) {
      emitEvent("sober_corpus.rate_limit.proactive_throttle", {
        remaining: rateLimit.remaining,
        resetAt: rateLimit.resetAt,
      });
      await sleepUntil(rateLimit.resetAt, 2000, (waitMs) =>
        emitEvent("sober_corpus.rate_limit.sleeping", { waitMs }));
    }

    return {
      data: body.data,
      rateLimit: rateLimit ?? { limit: 0, cost: 0, remaining: 0, resetAt: new Date().toISOString() },
    };
  }

  throw new Error("unreachable: graphqlRequest exhausted retries without returning or throwing");
}

const ISSUES_PAGE_QUERY = `
query SoberIssuesPage($owner: String!, $name: String!, $pageSize: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    issues(first: $pageSize, after: $cursor, orderBy: { field: UPDATED_AT, direction: DESC }) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        title
        body
        state
        stateReason
        authorAssociation
        createdAt
        updatedAt
        closedAt
        labels(first: 25) { nodes { name } }
        comments(first: 100) {
          totalCount
          pageInfo { hasNextPage endCursor }
          nodes { authorAssociation createdAt body }
        }
      }
    }
  }
  rateLimit { limit cost remaining resetAt }
}`;

export async function fetchIssuesPage(params: {
  owner: string;
  name: string;
  pageSize: number;
  cursor: string | null;
}): Promise<IssuesPage> {
  const { data, rateLimit } = await graphqlRequest<{
    repository: { issues: { pageInfo: IssuesPage["pageInfo"]; nodes: IssueNode[] } };
  }>(ISSUES_PAGE_QUERY, params);

  return {
    pageInfo: data.repository.issues.pageInfo,
    nodes: data.repository.issues.nodes,
    rateLimit,
  };
}

const ISSUE_REMAINING_COMMENTS_QUERY = `
query SoberIssueRemainingComments($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      comments(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes { authorAssociation createdAt body }
      }
    }
  }
  rateLimit { limit cost remaining resetAt }
}`;

/**
 * Fetches comments beyond the first 100 for a single issue. Genuinely rare
 * but not hypothetical: a full cold run against vinegarhq/sober hit this
 * path for 3 of 2,195 issues (#297 with 321 comments, #1580 with 166, #1209
 * with 109), so this is exercised by real data, not defensive code nobody
 * has seen run.
 */
export async function fetchRemainingComments(params: {
  owner: string;
  name: string;
  number: number;
  cursor: string;
}): Promise<CommentNode[]> {
  interface CommentsConnection {
    pageInfo: { hasNextPage: boolean; endCursor: string | null };
    nodes: CommentNode[];
  }
  interface RemainingCommentsResponse {
    repository: {
      issue: {
        comments: CommentsConnection;
      };
    };
  }

  const collected: CommentNode[] = [];
  let cursor: string | null = params.cursor;
  for (;;) {
    const result: { data: RemainingCommentsResponse; rateLimit: RateLimitInfo } = await graphqlRequest<
      RemainingCommentsResponse
    >(ISSUE_REMAINING_COMMENTS_QUERY, {
      owner: params.owner,
      name: params.name,
      number: params.number,
      cursor,
    });
    const comments: CommentsConnection = result.data.repository.issue.comments;
    collected.push(...comments.nodes);
    if (!comments.pageInfo.hasNextPage) {
      break;
    }
    cursor = comments.pageInfo.endCursor;
  }
  return collected;
}
