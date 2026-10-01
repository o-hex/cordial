/**
 * Who counts as a moderator, and who may edit a report.
 *
 * Three things in the bridge are gated on being somebody who helps run the
 * server rather than somebody who filed a report: **Mark as completed**,
 * applying any GitHub label, and editing a report somebody else filed. They
 * share one definition so that there is exactly one answer to "is this person a
 * moderator", and so changing it is one edit and one ADR paragraph rather than
 * three subtly different checks.
 *
 * ## Why Discord permissions first, a role second
 *
 * `member.permissions` in an interaction is computed by Discord *for the
 * channel it happened in*, overwrites included, so it needs no extra request
 * and cannot be stale. Manage Messages, Manage Threads and Administrator are
 * what the completed button already used, so that behaviour is unchanged.
 *
 * A role is additive and exists for the server whose moderators do not hold
 * those permissions (a "Triage" role, say). It is read from `member.roles`,
 * which Discord also supplies for free. **A role can only widen who is a
 * moderator, never narrow it**: narrowing would mean an administrator could be
 * refused the button by a misconfigured list, which is worse than the list
 * being too generous in a place the administrator already trusts.
 */

const ADMINISTRATOR = 1n << 3n;
const MANAGE_MESSAGES = 1n << 13n;
const MANAGE_THREADS = 1n << 34n;

/** The slice of an interaction's `member` this module reads. */
export interface Member {
  /** A decimal bitfield string, as Discord sends it. */
  permissions?: string;
  roles?: string[];
}

export function holdsModeratorPermission(bits: string | undefined): boolean {
  if (!bits) return false;
  let held: bigint;
  try {
    held = BigInt(bits);
  } catch {
    // A malformed bitfield is not a permission. Refusing is the safe reading.
    return false;
  }
  return (held & (MANAGE_MESSAGES | MANAGE_THREADS | ADMINISTRATOR)) !== 0n;
}

export function isModerator(member: Member | undefined, roleIds: readonly string[] = []): boolean {
  if (!member) return false;
  if (holdsModeratorPermission(member.permissions)) return true;
  return (member.roles ?? []).some((role) => roleIds.includes(role));
}

/**
 * Parse `DISCORD_MODERATOR_ROLE_IDS`: role ids separated by commas or spaces.
 *
 * Returns what it rejected instead of silently skipping it. **A typo in a list
 * that grants power must be loud**, and "nobody got the extra permission and
 * nothing said why" is the quiet version of that failure.
 */
export function parseRoleIds(value: string | undefined): { ids: string[]; invalid: string[] } {
  const ids: string[] = [];
  const invalid: string[] = [];
  for (const token of (value ?? "").split(/[\s,]+/).filter(Boolean)) {
    if (/^\d{1,32}$/.test(token)) ids.push(token);
    else invalid.push(token);
  }
  return { ids, invalid };
}

/**
 * May this person edit this report?
 *
 * The filer, or a moderator. The filer is **read from the issue's own marker**
 * (`reporterFromBody`) and passed in here, never taken from the button: a
 * `custom_id` is client-supplied. An issue with no recorded filer has no
 * owner, so only a moderator may edit it -- the same shape as closing, where a
 * web-filed issue cannot be closed from Discord by a stranger.
 */
export function canEditReport(
  userId: string | undefined,
  filerId: string | null,
  moderator: boolean,
): boolean {
  if (moderator) return true;
  return Boolean(userId && filerId && userId === filerId);
}
