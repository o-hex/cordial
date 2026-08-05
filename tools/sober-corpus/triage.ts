/**
 * Heuristic classifier for "triage noise": a maintainer comment that closes
 * or dismisses an issue without actually diagnosing anything — "duplicate
 * of #123", "please fill the template", a bare "wontfix". These pass the
 * "has a maintainer reply" bar but carry no ground-truth diagnostic content,
 * so including them in the derived set would surface triage boilerplate to
 * a Cordial maintainer instead of a real diagnosis. Quality of the set
 * matters more than volume here (see ADR-017).
 *
 * This is a pattern-based heuristic, not a classifier trained on labelled
 * data — it will have both false positives (a genuinely short but correct
 * fix, e.g. "Set `WINEDEBUG=-all`, that's the crash") and false negatives (a
 * long-winded comment that still says nothing). It is deliberately
 * conservative: patterns target specific, common triage phrasings rather
 * than a generic word-count cutoff, to avoid discarding real short answers.
 *
 * @module sober-corpus/triage
 */

const BARE_CLOSING_PHRASES = new Set([
  "closing",
  "closed",
  "duplicate",
  "dup",
  "wontfix",
  "won't fix",
  "not planned",
  "stale",
  "no longer relevant",
  "invalid",
  "works for me",
  "cannot reproduce",
  "can't reproduce",
  "fixed",
  "done",
  "resolved",
  "thanks",
  "thank you",
]);

function wordCount(body: string): number {
  return body.trim().split(/\s+/).filter(Boolean).length;
}

/** True if `body` is triage boilerplate rather than a substantive diagnosis/fix. */
export function isTriageComment(rawBody: string | undefined | null): boolean {
  const body = (rawBody ?? "").trim();
  if (body.length < 8) {
    return true;
  }

  const normalized = body.toLowerCase().replace(/[.!\s]+$/, "");
  if (BARE_CLOSING_PHRASES.has(normalized)) {
    return true;
  }

  const words = wordCount(body);

  // "duplicate of #123" / "dup of #456" — a link, not a diagnosis.
  const looksLikeDuplicateReference = /\bdup(?:e|licate)?\b/i.test(body) && /#\d+/.test(body);
  if (looksLikeDuplicateReference && words < 20) {
    return true;
  }

  // "please fill out the issue template" — a process nudge, not an answer.
  const looksLikeTemplateRequest = /please\s+(fill|use|complete|follow)\b/i.test(body) && /\btemplate\b/i.test(body);
  if (looksLikeTemplateRequest && words < 40) {
    return true;
  }

  // "please share your logs" — still gathering information, not resolving.
  const looksLikeInfoRequest =
    /please\s+(provide|share|post|attach|upload|paste)\b/i.test(body) &&
    /\b(log|logs|screenshot|screenshots|steps|info|information|trace|output)\b/i.test(body);
  if (looksLikeInfoRequest && words < 30) {
    return true;
  }

  return false;
}
