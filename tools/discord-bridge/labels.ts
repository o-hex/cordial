/**
 * GitHub labels in the report form: what is offered, who may pick what, and how
 * the list is kept without a database.
 *
 * ## The permission model, and why it is a list of what *is* allowed
 *
 * A report form that lets anybody attach any label lets anybody type
 * `confirmed`, `wontfix` or `priority: high` onto their own report, and a
 * tracker where triage labels can be self-applied has stopped meaning anything
 * by them. So the default is the other way round:
 *
 *   - a **reporter** may choose only labels matching an **allowlist** of
 *     descriptive patterns -- what *part* of the thing is affected, never what
 *     the project has decided about it. The default is `area:*`, `platform:*`,
 *     `compositor:*` and `gpu:*`; `GITHUB_REPORTER_LABELS` replaces it.
 *   - a **moderator** (see `permissions.ts`) may apply any label that exists.
 *   - a short **protected** set can never be chosen by a reporter, **whatever
 *     the allowlist says**. It is in code rather than in configuration on
 *     purpose: the failure it guards against is an allowlist widened to `*` in
 *     a hurry, and a guard that the same hurried edit can remove guards nothing.
 *
 * Labels the form's template already applies (`bug`, `enhancement`) are the
 * maintainer's own words and are always applied; they do not go through this.
 *
 * ## Why the list is cached rather than fetched per press
 *
 * Opening a modal must be the *first* response to an interaction and Discord
 * allows three seconds for it, so a GitHub round trip is only affordable when
 * it is rare. The list changes a few times a month, so a ten-minute TTL costs
 * nothing visible. **A failed refresh keeps serving the last good list** and
 * retries soon, rather than pinning an empty state for ten minutes -- and with
 * no list at all the form simply has no label picker and the report still
 * files, which is the whole point of treating labels as optional.
 */

/** Discord allows at most 25 options in one select menu. */
export const MAX_SELECT_OPTIONS = 25;

export interface Label {
  name: string;
  description?: string | null;
}

/**
 * Descriptive, not decisive: where in Cordial a problem lives. These only
 * match labels that exist, so a repository without them offers reporters no
 * picker at all rather than a wrong one.
 */
export const DEFAULT_REPORTER_LABELS = ["area:*", "platform:*", "compositor:*", "gpu:*"];

/**
 * Never selectable by a reporter, even if the allowlist matches them.
 *
 * These are the project's decisions about a report (confirmed, rejected,
 * ranked, routed), as opposed to the reporter's description of it.
 */
export const PROTECTED_LABELS = [
  "confirmed",
  "wontfix",
  "invalid",
  "duplicate",
  "priority*",
  "severity*",
  "security*",
  "triage*",
  "good first issue",
  "help wanted",
];

/**
 * Which labels to show first when more than 25 are eligible, per form.
 *
 * A bug report is most usefully tagged by graphics stack and compositor, a
 * request for a Roblox feature by area, a new-build breakage by platform. This
 * is an ordering and not a filter: anything not named still follows, in name
 * order, until the menu is full. Keyed by template slug, so a form added later
 * simply gets the default ordering until somebody says otherwise.
 */
export const FORM_LABEL_ORDER: Record<string, string[]> = {
  bug_report: ["gpu:*", "compositor:*", "platform:*", "area:*"],
  broken_feature: ["area:*", "platform:*"],
  roblox_update: ["platform:*", "area:*"],
  feature: ["area:*"],
  finding: ["area:*"],
};

const DEFAULT_ORDER = ["area:*", "platform:*", "compositor:*", "gpu:*"];

function globToRegExp(pattern: string): RegExp {
  const escaped = pattern.trim().replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
  return new RegExp(`^${escaped}$`, "i");
}

/** Case-insensitive, with `*` as the only wildcard. GitHub treats label names so. */
export function matches(pattern: string, name: string): boolean {
  return globToRegExp(pattern).test(name);
}

/**
 * Parse `GITHUB_REPORTER_LABELS`.
 *
 * Unset or blank is the default list. `none` (or `-`) means reporters may pick
 * nothing, which is a legitimate setting and has to be expressible: an empty
 * environment variable is indistinguishable from an unset one on most hosts.
 */
export function parseAllowlist(value: string | undefined): string[] {
  const text = (value ?? "").trim();
  if (!text) return [...DEFAULT_REPORTER_LABELS];
  if (text === "none" || text === "-") return [];
  return text.split(/[\n,]+/).map((s) => s.trim()).filter(Boolean);
}

export type Who = "reporter" | "moderator";

/** May this person apply this label? */
export function mayApply(name: string, who: Who, allow: readonly string[]): boolean {
  if (who === "moderator") return true;
  if (PROTECTED_LABELS.some((p) => matches(p, name))) return false;
  return allow.some((p) => matches(p, name));
}

function rank(name: string, order: readonly string[]): number {
  const at = order.findIndex((p) => matches(p, name));
  return at === -1 ? order.length : at;
}

export interface Offer {
  /** What goes in the menu, in menu order, at most 25. */
  offered: Label[];
  /** Eligible labels that did not fit, so the form can say so. */
  omitted: number;
}

/**
 * Choose what the select menu shows.
 *
 * Order: labels already on the issue (so an editor always sees what is there),
 * then the form's preferred groups, then name order. Cut at Discord's 25. The
 * count that did not fit is returned rather than dropped, because **a cap
 * nobody mentions reads as the whole list**.
 */
export function offerLabels(options: {
  known: readonly Label[];
  who: Who;
  allow: readonly string[];
  formSlug?: string;
  /** Names the template applies by itself; offering them would be redundant. */
  exclude?: readonly string[];
  /** Names already on the issue, shown first when editing. */
  pinned?: readonly string[];
  limit?: number;
}): Offer {
  const limit = options.limit ?? MAX_SELECT_OPTIONS;
  const order = (options.formSlug && FORM_LABEL_ORDER[options.formSlug]) || DEFAULT_ORDER;
  const exclude = new Set((options.exclude ?? []).map((s) => s.toLowerCase()));
  const pinned = new Set((options.pinned ?? []).map((s) => s.toLowerCase()));

  const eligible = options.known
    .filter((l) => !exclude.has(l.name.toLowerCase()))
    .filter((l) => mayApply(l.name, options.who, options.allow));

  eligible.sort((a, b) =>
    Number(pinned.has(b.name.toLowerCase())) - Number(pinned.has(a.name.toLowerCase())) ||
    rank(a.name, order) - rank(b.name, order) ||
    a.name.localeCompare(b.name)
  );

  return {
    offered: eligible.slice(0, limit),
    omitted: Math.max(0, eligible.length - limit),
  };
}

/**
 * Reduce what a submission claims to what this person may actually apply.
 *
 * **Re-checked at submit, never trusted from the form.** A select's values
 * arrive from the client; Discord validates them against the options it was
 * shown, but the permission is the bridge's to enforce and a second
 * interaction can be forged more easily than a first one is believed. Names
 * come back in GitHub's own casing, and a name that does not exist is rejected
 * rather than passed on -- GitHub would otherwise *create* a label for an
 * unknown name, which is exactly how a stale menu would mint one.
 */
export function resolveSelection(
  selected: readonly string[],
  known: readonly Label[],
  who: Who,
  allow: readonly string[],
): { applied: string[]; rejected: string[] } {
  const byLower = new Map(known.map((l) => [l.name.toLowerCase(), l.name]));
  const applied: string[] = [];
  const rejected: string[] = [];
  for (const raw of selected) {
    const canonical = byLower.get(raw.toLowerCase());
    if (canonical && mayApply(canonical, who, allow)) {
      if (!applied.includes(canonical)) applied.push(canonical);
    } else {
      rejected.push(raw);
    }
  }
  return { applied, rejected };
}

/**
 * The label set after an edit.
 *
 * The editor only controls what they were *offered*. Everything else already on
 * the issue -- `bug`, a maintainer's `confirmed`, an `area:` label beyond the
 * cap -- is preserved untouched, so a reporter tidying a typo cannot strip the
 * project's own triage by deselecting something they could never see.
 */
export function planLabelEdit(
  current: readonly string[],
  offered: readonly Label[],
  selected: readonly string[],
): { next: string[]; added: string[]; removed: string[] } {
  const offeredNames = new Set(offered.map((l) => l.name.toLowerCase()));
  const chosen = new Set(selected.map((s) => s.toLowerCase()));
  const preserved = current.filter((c) => !offeredNames.has(c.toLowerCase()));
  const kept = offered.filter((l) => chosen.has(l.name.toLowerCase())).map((l) => l.name);
  const next = [...preserved, ...kept];
  const had = new Set(current.map((c) => c.toLowerCase()));
  const has = new Set(next.map((c) => c.toLowerCase()));
  return {
    next,
    added: next.filter((n) => !had.has(n.toLowerCase())),
    removed: current.filter((c) => !has.has(c.toLowerCase())),
  };
}

interface Cache {
  labels: Label[];
  fetchedAt: number;
}

export interface LabelSourceOptions {
  now?: () => number;
  ttlMs?: number;
  /** After a failed refresh, how long before trying again. */
  retryMs?: number;
}

/**
 * The repository's labels, cached.
 *
 * `read` is injected -- the GitHub client in production, a function in tests --
 * so the TTL, the stale fallback and the retry spacing are exercised without a
 * network, which is the only way the failure paths get exercised at all.
 */
export class Labels {
  #read: () => Promise<Label[]>;
  #now: () => number;
  #ttl: number;
  #retry: number;
  #cache: Cache | null = null;
  #retryAt = 0;
  #inflight: Promise<Label[] | null> | null = null;
  /** Why the newest refresh failed, while stale or absent. For `/health`. */
  failure: string | null = null;

  constructor(read: () => Promise<Label[]>, options: LabelSourceOptions = {}) {
    this.#read = read;
    this.#now = options.now ?? Date.now;
    this.#ttl = options.ttlMs ?? 10 * 60 * 1000;
    this.#retry = options.retryMs ?? 30 * 1000;
  }

  /** How many are cached and how old, for `/health`. */
  get status(): { count: number; ageSeconds: number | null; failure: string | null } {
    return {
      count: this.#cache?.labels.length ?? 0,
      ageSeconds: this.#cache ? Math.round((this.#now() - this.#cache.fetchedAt) / 1000) : null,
      failure: this.failure,
    };
  }

  /**
   * The labels, or null if there are none to be had.
   *
   * `timeoutMs` bounds how long a caller inside Discord's three-second window
   * will wait on a refresh. On timeout the last good list is served if there is
   * one; the refresh carries on and lands in the cache for the next press.
   */
  async get(timeoutMs?: number): Promise<Label[] | null> {
    const fresh = this.#cache && this.#now() - this.#cache.fetchedAt < this.#ttl;
    if (fresh) return this.#cache!.labels;
    if (this.#now() < this.#retryAt) return this.#cache?.labels ?? null;

    this.#inflight ??= this.#refresh();
    if (timeoutMs === undefined) return await this.#inflight;

    let timer: ReturnType<typeof setTimeout> | undefined;
    const slow = new Promise<"slow">((resolve) => {
      timer = setTimeout(() => resolve("slow"), timeoutMs);
    });
    try {
      const result = await Promise.race([this.#inflight, slow]);
      return result === "slow" ? (this.#cache?.labels ?? null) : result;
    } finally {
      clearTimeout(timer);
    }
  }

  async #refresh(): Promise<Label[] | null> {
    // Yield first, so `get` has stored this promise as `#inflight` before the
    // `finally` below can clear it. Without it a `read` that throws
    // synchronously would clear the slot first and the caller would then store
    // a settled promise there for ever, and the list would never refresh.
    await Promise.resolve();
    try {
      const labels = await this.#read();
      this.#cache = { labels, fetchedAt: this.#now() };
      this.#retryAt = 0;
      this.failure = null;
      return labels;
    } catch (error) {
      this.failure = error instanceof Error ? error.message : String(error);
      console.error(`labels: ${this.failure}`);
      this.#retryAt = this.#now() + this.#retry;
      return this.#cache?.labels ?? null;
    } finally {
      this.#inflight = null;
    }
  }
}
