/**
 * The pure half of editing a report from Discord: what the dialog contains,
 * how it is split when a form has more fields than a modal holds, how a change
 * is fingerprinted, and what the audit trail says.
 *
 * Nothing here touches the network. `edit_flow.ts` does, and this is what it is
 * made of, so the rules that matter -- a modal never exceeds five components, a
 * stale dialog is detected, the audit comment cannot be broken out of by what
 * it quotes -- are tested without a Discord token.
 *
 * ## Why a report is edited in parts
 *
 * A modal holds five components. The editor needs a title and, usually, a label
 * menu, which leaves three for the reporter's own words, and `bug_report` alone
 * has eight editable fields. Rather than offer only the first three and call the
 * rest GitHub's problem, a reply carries an **Edit more fields** button -- the
 * same shape the filing flow already uses for its leftover optional fields, for
 * the same reason: a modal submit cannot itself open a modal, but the message it
 * leaves behind can carry a button that does. Each part is its own edit with its
 * own audit entry, so no single dialog is ever ambiguous about what it changes.
 */
import { type FormBlock, type IssueForm, LABEL, maxLengthFor, TEXT_INPUT } from "./issue_forms.ts";
import { fieldToComponent } from "./issue_forms.ts";
import { heading, isEditable, type ParsedBody } from "./issue_body.ts";

/** `custom_id` of the title box. No template field can share it. */
export const TITLE_ID = "cordial-title";

/** Title, labels, and these. */
export const FIRST_PART_FIELDS = 3;
export const LATER_PART_FIELDS = 5;

/** GitHub's own limit on an issue title. */
export const TITLE_MAX = 256;

/**
 * The fields an editor can be offered, in template order.
 *
 * A field whose current answer is longer than its modal box can hold is left
 * out: Discord rejects a pre-filled value over `max_length`, and truncating it
 * to fit would be the dialog quietly deleting the end of somebody's log.
 * Such a field is kept as it is and can still be changed on GitHub.
 */
export function editableFields(form: IssueForm, parsed: ParsedBody | null): FormBlock[] {
  if (!parsed) return [];
  return form.fields.filter((block) => {
    if (!isEditable(block)) return false;
    const current = parsed.sections.get(block.id!)?.value ?? "";
    return current.length <= maxLengthFor(block.type);
  });
}

/** Fields that exist but are too long to edit here, so the dialog can say so. */
export function tooLongToEdit(form: IssueForm, parsed: ParsedBody | null): FormBlock[] {
  if (!parsed) return [];
  const offered = new Set(editableFields(form, parsed).map((b) => b.id));
  return form.fields.filter((b) =>
    isEditable(b) && !offered.has(b.id) && parsed.sections.has(b.id!)
  );
}

export function fieldsForPart(fields: FormBlock[], part: number): FormBlock[] {
  if (part === 0) return fields.slice(0, FIRST_PART_FIELDS);
  const start = FIRST_PART_FIELDS + (part - 1) * LATER_PART_FIELDS;
  return fields.slice(start, start + LATER_PART_FIELDS);
}

export function partCount(fields: FormBlock[]): number {
  return 1 + Math.ceil(Math.max(0, fields.length - FIRST_PART_FIELDS) / LATER_PART_FIELDS);
}

/** One field of the form, pre-filled with what the issue says now. */
export function editFieldComponent(block: FormBlock, parsed: ParsedBody): unknown {
  const component = fieldToComponent(block) as {
    component: Record<string, unknown>;
  };
  const section = parsed.sections.get(block.id!);
  const trimmed = (section?.value ?? "").trim();
  // `_No response_` is GitHub's marker for an empty optional answer, not text
  // somebody wrote, so it is not put in the box for them to delete.
  if (trimmed && trimmed !== "_No response_") component.component.value = trimmed;
  // A field the issue never had -- an issue filed on the web, or an optional
  // one left blank -- is not made mandatory by being edited. Requiring it would
  // make fixing a typo in the title impossible without inventing an answer.
  if (!section) component.component.required = false;
  return component;
}

export function titleComponent(title: string): unknown {
  return {
    type: LABEL,
    label: "Title",
    component: {
      type: TEXT_INPUT,
      custom_id: TITLE_ID,
      style: 1,
      required: true,
      max_length: TITLE_MAX,
      value: title.slice(0, TITLE_MAX),
    },
  };
}

/**
 * A short, stable digest of what an editor was shown.
 *
 * It rides in the modal's `custom_id` and is recomputed on submit. **If the
 * issue changed in between -- a maintainer retitled it, another editor saved --
 * the digests differ and the edit is refused rather than applied on top.**
 * That is what stops "never silently overwrite" being true only of the audit
 * log: two people editing from stale dialogs would otherwise each be recorded
 * faithfully while the second quietly erased the first.
 *
 * Sixteen hex characters keep the whole `custom_id` far inside Discord's
 * hundred; this guards against an accident, not an attacker, because the
 * permission check is separate and re-run.
 */
export async function fingerprint(
  issue: { title: string; body: string | null; labels: readonly string[] },
): Promise<string> {
  const text = JSON.stringify([issue.title, issue.body ?? "", [...issue.labels].sort()]);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest).slice(0, 8)].map((b) => b.toString(16).padStart(2, "0")).join(
    "",
  );
}

/** Pick the template an issue was filed from, when its marker does not say. */
export function inferForm(
  forms: IssueForm[],
  body: string | null | undefined,
  recordedSlug: string | null,
): IssueForm | null {
  if (recordedSlug) {
    const recorded = forms.find((f) => f.slug === recordedSlug);
    if (recorded) return recorded;
  }
  const headings = [...(body ?? "").matchAll(/^### (.*)$/gm)].map((m) => m[1].trimEnd())
    .filter((h) => h !== "Reported from Discord");
  if (!headings.length) return null;
  // The one form whose field names contain every heading in the body. Two that
  // both fit are not guessed between: editing against the wrong template
  // rewrites sections, so that is a reason to refuse.
  const fits = forms.filter((f) => {
    const names = new Set(f.fields.map(heading));
    return headings.every((h) => names.has(h));
  });
  return fits.length === 1 ? fits[0] : null;
}

/** Collapse a title to one line, since a title with a newline is not one. */
export function cleanTitle(raw: string): string {
  return raw.replace(/\s+/g, " ").trim().slice(0, TITLE_MAX);
}

// ---------------------------------------------------------------------------
// The audit trail
// ---------------------------------------------------------------------------

export interface AuditEntry {
  number: number;
  editor: { id: string; tag: string };
  /** `reporter` edited their own report; `moderator` edited somebody else's, or used moderator rights. */
  as: "reporter" | "moderator";
  at: Date;
  title?: { before: string; after: string };
  fields: { label: string; before: string; after: string }[];
  labels?: { added: string[]; removed: string[] };
}

/** GitHub rejects a comment over 65,536 characters; stay well under. */
const COMMENT_BUDGET = 60_000;

/** Inline code that cannot be closed early by the text inside it. */
function inline(text: string, limit = 200): string {
  const flat = text.replace(/`/g, "'").replace(/\s+/g, " ").trim();
  return "`" + (flat.length > limit ? flat.slice(0, limit - 1) + "…" : flat) + "`";
}

/**
 * A fenced block that what it quotes cannot break out of.
 *
 * Reporters paste logs, and logs contain triple backticks. A fixed fence would
 * end at the first one and render the rest of the quoted text -- including any
 * markup in it -- as part of the audit comment itself, which is the one place
 * that has to be unambiguous about what was said by whom.
 */
export function fenced(text: string): string {
  const longest = Math.max(0, ...[...text.matchAll(/`+/g)].map((m) => m[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

export function utc(at: Date): string {
  return at.toISOString().replace("T", " ").replace(/\.\d+Z$/, " UTC");
}

export function describeChanges(entry: AuditEntry): string[] {
  const lines: string[] = [];
  if (entry.title) {
    lines.push(`**Title:** ${inline(entry.title.before)} → ${inline(entry.title.after)}`);
  }
  for (const f of entry.fields) {
    lines.push(
      `**${f.label}:** ${f.before ? `${f.before.length}` : "empty"} → ` +
        `${f.after ? `${f.after.length}` : "empty"} characters`,
    );
  }
  if (entry.labels && (entry.labels.added.length || entry.labels.removed.length)) {
    const bits: string[] = [];
    if (entry.labels.added.length) {
      bits.push(`added ${entry.labels.added.map((l) => inline(l)).join(", ")}`);
    }
    if (entry.labels.removed.length) {
      bits.push(`removed ${entry.labels.removed.map((l) => inline(l)).join(", ")}`);
    }
    lines.push(`**Labels:** ${bits.join("; ")}`);
  }
  return lines;
}

/**
 * The comment left on the GitHub issue for every edit.
 *
 * It names who, when and what, and it **carries the text that was replaced**, so
 * an edit made from Discord cannot destroy anything: the previous words are in
 * the thread of the issue itself, under the editor's name, whatever happens to
 * the body afterwards. GitHub's own edit history records a body change too, but
 * attributes it to the App; this comment is where the Discord user is.
 */
export function auditComment(entry: AuditEntry): string {
  const role = entry.as === "moderator" ? ", as a moderator" : ", the reporter";
  const head = `**Edited from Discord** by ${inline(entry.editor.tag, 80)} ` +
    `(\`${entry.editor.id}\`${role}) at ${utc(entry.at)}.`;
  const summary = describeChanges(entry).map((l) => `- ${l}`).join("\n");

  const previous: string[] = [];
  if (entry.title) previous.push(`**Title**\n\n${fenced(entry.title.before)}`);
  for (const f of entry.fields) {
    previous.push(`**${f.label}**\n\n${fenced(f.before || "(empty)")}`);
  }

  const open = "<details>\n<summary>Previous text of what changed</summary>\n\n";
  const close = "\n</details>";
  const used = head.length + summary.length + open.length + close.length + 8;
  let body = previous.join("\n\n");
  if (used + body.length > COMMENT_BUDGET) {
    body = body.slice(0, Math.max(0, COMMENT_BUDGET - used - 200)) +
      "\n\n*(Cut here for length. GitHub's own edit history keeps the full previous body.)*";
  }
  return `${head}\n\n${summary}\n\n${open}${body}${close}`;
}

/** The one-line record posted in the Discord thread. */
export function auditThreadLine(entry: AuditEntry, commentUrl?: string): string {
  const bits: string[] = [];
  if (entry.title) bits.push("title changed");
  if (entry.fields.length) {
    bits.push(`${entry.fields.map((f) => f.label).join(", ")} changed`);
  }
  if (entry.labels?.added.length || entry.labels?.removed.length) {
    const l = entry.labels!;
    bits.push(
      "labels " +
        [...l.added.map((x) => `+${x}`), ...l.removed.map((x) => `−${x}`)].join(" "),
    );
  }
  const line = `**Edited** by ${entry.editor.tag} (${entry.editor.id}) at ${utc(entry.at)}: ` +
    `${bits.join("; ")}.` + (commentUrl ? ` Previous text: <${commentUrl}>` : "");
  return line.length > 1900 ? line.slice(0, 1899) + "…" : line;
}
