/**
 * Editing a filed report from Discord: open the dialog, and apply what comes back.
 *
 * The rules, in the order they bite:
 *
 *   1. **Who.** The reporter recorded in the issue's own marker, or a moderator.
 *      Anybody else gets an ephemeral refusal. Checked when the dialog opens
 *      *and again on submit*, because a modal's `custom_id` is client-supplied
 *      and the submit is a fresh interaction that proves nothing about the first.
 *   2. **Not stale.** The dialog carries a fingerprint of what it showed. If the
 *      issue is different on submit, nothing is saved.
 *   3. **Only the reporter's words.** Title, labels and the form's free-text
 *      fields. The Diagnostics block, the credit line and the hidden marker are
 *      carried through untouched -- see `issue_body.ts`.
 *   4. **Audit before change.** The comment recording who, when and the
 *      previous text is posted *first*. If it cannot be, nothing is edited. If
 *      the edit then fails, the comment is amended to say so, so the record
 *      never claims something that did not happen.
 *
 * What this does **not** do: it does not make concurrent edits safe in the
 * database sense. Between the fingerprint check and the `PATCH` there is one
 * network round trip, and an edit that lands inside it is overwritten -- but
 * that is the audit comment's job, since the text it replaced is in it.
 */
import type { Context } from "./interactions.ts";
import { ACTION_ROW, BUTTON, LABEL_SELECT_ID, labelPickerComponent } from "./issue_forms.ts";
import { EPHEMERAL, ResponseType } from "./discord.ts";
import {
  assembleBody,
  changedFields,
  formFromBody,
  heading,
  parseBody,
  reporterFromBody,
  threadFromBody,
} from "./issue_body.ts";
import {
  auditComment,
  type AuditEntry,
  auditThreadLine,
  cleanTitle,
  editableFields,
  editFieldComponent,
  fieldsForPart,
  fingerprint,
  inferForm,
  partCount,
  TITLE_ID,
  titleComponent,
  tooLongToEdit,
} from "./edit.ts";
import { offerLabels, planLabelEdit, resolveSelection, type Who } from "./labels.ts";
import { canEditReport, isModerator, type Member } from "./permissions.ts";

interface Editor {
  id: string;
  tag: string;
  member?: Member;
}

const refusal = (content: string) => ({
  type: ResponseType.MESSAGE,
  data: { content, flags: EPHEMERAL },
});

const NOT_YOURS = "Only the person who filed this report, or someone who helps run this server, " +
  "can edit it. You can still comment on it, which is what the other buttons are for.";

/** Reject if `work` takes longer than `ms`, so a slow GitHub cannot eat Discord's three seconds. */
function within<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not answer in time`)), ms);
  });
  return Promise.race([work, late]).finally(() => clearTimeout(timer));
}

function whoFor(editor: Editor, context: Context): Who {
  return isModerator(editor.member, context.moderatorRoleIds) ? "moderator" : "reporter";
}

/**
 * Answer the Edit button with a dialog, or with a reason there is not one.
 *
 * This **cannot defer**: a modal is only a valid first response, so the issue
 * and the label list are fetched inside Discord's three seconds. Both are
 * started together and bounded, so the worst case is the slower of the two and
 * not their sum; and a label list that is not there in time costs the dialog its
 * label menu rather than the dialog.
 */
export async function openEditor(
  context: Context,
  editor: Editor,
  number: number,
  part: number,
): Promise<unknown> {
  if (!Number.isInteger(number) || !Number.isInteger(part) || part < 0) {
    return refusal("That button has lost its issue number.");
  }

  const labelsPending = part === 0 && context.labels
    ? context.labels.get(700).catch(() => null)
    : Promise.resolve(null);
  let issue;
  try {
    issue = await within(context.github.issue(number), 1600, "GitHub");
  } catch (error) {
    console.error(`edit #${number}: ${error}`);
    return refusal(
      "GitHub did not answer in time, so the editor could not open. Press **Edit** again.",
    );
  }

  const moderator = isModerator(editor.member, context.moderatorRoleIds);
  if (!canEditReport(editor.id, reporterFromBody(issue.body), moderator)) {
    return refusal(NOT_YOURS);
  }

  const forms = await context.forms();
  const form = inferForm(forms, issue.body, formFromBody(issue.body));
  const parsed = form && issue.body ? parseBody(form, issue.body) : null;
  const fields = form ? editableFields(form, parsed) : [];
  const parts = partCount(fields);
  if (part >= parts) return refusal("There is nothing more to edit on this report.");

  const components: unknown[] = [];
  let withLabels = false;
  if (part === 0) components.push(titleComponent(issue.title));
  for (const block of fieldsForPart(fields, part)) {
    components.push(editFieldComponent(block, parsed!));
  }

  if (part === 0) {
    const known = await labelsPending;
    if (known) {
      const who: Who = moderator ? "moderator" : "reporter";
      const { offered, omitted } = offerLabels({
        known,
        who,
        allow: context.reporterLabels ?? [],
        formSlug: form?.slug,
        pinned: issue.labels,
      });
      if (offered.length) {
        withLabels = true;
        const note = moderator
          ? "Any label. Deselect to remove."
          : "Descriptive labels only; maintainers set the rest.";
        components.push(labelPickerComponent(offered, {
          description: omitted ? `${note} ${omitted} more not shown.` : note,
          selected: issue.labels,
        }));
      }
    }
  }

  const hash = await fingerprint(issue);
  return {
    type: ResponseType.MODAL,
    data: {
      custom_id: `cordial-edit:${number}:${part}:${hash}${withLabels ? ":l" : ""}`,
      title: (`Edit #${number}` + (parts > 1 ? ` (${part + 1} of ${parts})` : "")).slice(0, 45),
      components,
    },
  };
}

/** The button under a reply that leads to the next dialog, if there is one. */
function moreButton(number: number, part: number, parts: number): unknown[] {
  if (part + 1 >= parts) return [];
  return [{
    type: ACTION_ROW,
    components: [{
      type: BUTTON,
      style: 2,
      label: "Edit more fields",
      custom_id: `cordial-edit-open:${number}:${part + 1}`,
    }],
  }];
}

/**
 * Apply a submitted edit. Runs after the interaction was deferred, so it
 * replies by editing the original message.
 */
export async function submitEdit(
  context: Context,
  interaction: { token: string; channel_id?: string },
  editor: Editor,
  custom: { number: number; part: number; hash: string; withLabels: boolean },
  submitted: { values: Record<string, string>; selections: Record<string, string[]> },
): Promise<void> {
  const { number, part } = custom;
  const link = `[#${number}](${context.repoUrl}/issues/${number})`;
  const say = (content: string, components: unknown[] = []) =>
    context.discord.editOriginal(interaction.token, { content, components });

  if (!Number.isInteger(number) || !Number.isInteger(part) || part < 0) {
    return await say("That dialog has lost its issue number. Nothing was saved.");
  }

  const issue = await context.github.issue(number);

  const moderator = isModerator(editor.member, context.moderatorRoleIds);
  const filer = reporterFromBody(issue.body);
  if (!canEditReport(editor.id, filer, moderator)) return await say(NOT_YOURS);

  // The fingerprint is checked after the permission, so a refusal never reveals
  // whether somebody else's report changed.
  if (await fingerprint(issue) !== custom.hash) {
    return await say(
      `${link} changed after you opened the editor -- somebody else edited it, or a ` +
        `maintainer did on GitHub. **Nothing was saved**, so nothing of theirs was ` +
        `overwritten. Press **Edit** again to start from what it says now.`,
    );
  }

  const forms = await context.forms();
  const form = inferForm(forms, issue.body, formFromBody(issue.body));
  const parsed = form && issue.body ? parseBody(form, issue.body) : null;
  const fields = form ? editableFields(form, parsed) : [];
  const parts = partCount(fields);

  // What the dialog was allowed to change this time, and nothing else. A field
  // the submission does not mention is left alone -- absent is not the same as
  // emptied, and treating it as emptied would let a dropped value delete text.
  const edits: Record<string, string> = {};
  for (const block of fieldsForPart(fields, part)) {
    const value = submitted.values[block.id!];
    if (value === undefined) continue;
    // Only a field that is *there* is protected from being emptied; one the
    // issue never had was not required of the editor either (see
    // `editFieldComponent`).
    if (block.validations?.required && parsed?.sections.has(block.id!) && !value.trim()) {
      return await say(
        `**${heading(block)}** is required and cannot be left empty. Nothing was saved.`,
      );
    }
    edits[block.id!] = value;
  }

  let newTitle: string | undefined;
  if (part === 0 && submitted.values[TITLE_ID] !== undefined) {
    const cleaned = cleanTitle(submitted.values[TITLE_ID]);
    if (!cleaned) return await say("The title cannot be empty. Nothing was saved.");
    if (cleaned !== issue.title) newTitle = cleaned;
  }

  const changed = parsed && form ? changedFields(parsed, form, edits) : [];
  const newBody = changed.length && parsed && form ? assembleBody(form, parsed, edits) : undefined;

  // Labels: only if this dialog had the picker, and only judged against the
  // list the picker could have been built from.
  let labelChange: { next: string[]; added: string[]; removed: string[] } | undefined;
  let labelNote = "";
  if (custom.withLabels && part === 0) {
    const known = context.labels ? await context.labels.get(2000) : null;
    if (!known) {
      labelNote = " Labels were left as they were, because the label list could not be read.";
    } else {
      const who = whoFor(editor, context);
      const allow = context.reporterLabels ?? [];
      const { offered } = offerLabels({
        known,
        who,
        allow,
        formSlug: form?.slug,
        pinned: issue.labels,
      });
      const chosen = resolveSelection(
        submitted.selections[LABEL_SELECT_ID] ?? [],
        known,
        who,
        allow,
      );
      const plan = planLabelEdit(issue.labels, offered, chosen.applied);
      if (plan.added.length || plan.removed.length) labelChange = plan;
    }
  }

  const moreFields = moreButton(number, part, parts);

  if (newTitle === undefined && !changed.length && !labelChange) {
    return await say(`Nothing changed on ${link}, so nothing was saved.${labelNote}`, moreFields);
  }

  const entry: AuditEntry = {
    number,
    editor: { id: editor.id, tag: editor.tag },
    as: filer === editor.id ? "reporter" : "moderator",
    at: new Date(),
    title: newTitle === undefined ? undefined : { before: issue.title, after: newTitle },
    fields: changed.map((c) => ({ label: heading(c.block), before: c.before, after: c.after })),
    labels: labelChange ? { added: labelChange.added, removed: labelChange.removed } : undefined,
  };

  // The record first. If it cannot be written the edit does not happen, which is
  // the whole content of "never silently overwrite".
  const audit = await context.github.comment(number, auditComment(entry));

  try {
    await context.github.updateIssue(number, {
      ...(newTitle === undefined ? {} : { title: newTitle }),
      ...(newBody === undefined ? {} : { body: newBody }),
      ...(labelChange ? { labels: labelChange.next } : {}),
    });
  } catch (error) {
    // The comment says an edit happened; amend it so it stops saying so.
    try {
      if (audit?.id !== undefined) {
        await context.github.editComment(
          audit.id,
          `> **This edit was not applied** -- GitHub refused it. Nothing on the issue changed.\n\n` +
            auditComment(entry),
        );
      }
    } catch (second) {
      console.error(`could not amend the audit comment on #${number}: ${second}`);
    }
    throw error;
  }

  // Two durable records and a log line. The thread copy is for the people in
  // Discord who cannot see GitHub's comments without leaving; the log is for
  // the operator. Neither carries the previous text -- the comment does.
  console.log(JSON.stringify({
    audit: "edit",
    issue: number,
    editor: entry.editor.id,
    as: entry.as,
    title: Boolean(entry.title),
    fields: entry.fields.map((f) => f.label),
    labels: entry.labels ?? null,
  }));

  const thread = threadFromBody(issue.body) ?? interaction.channel_id;
  if (thread) {
    try {
      await context.discord.post(thread, auditThreadLine(entry, audit?.html_url));
      if (newTitle !== undefined) {
        await context.discord.renameThread(thread, `#${number} ${newTitle}`.slice(0, 100));
      }
    } catch (error) {
      // The issue and its comment are the record. A thread that cannot be
      // posted into -- archived and un-restorable, deleted -- must not undo an
      // edit that has already been logged where it matters.
      console.error(`thread for #${number}: ${error}`);
    }
  }

  const unreachable = form && parsed ? tooLongToEdit(form, parsed) : [];
  const parts_ = [
    newTitle === undefined ? null : "the title",
    changed.length ? `${changed.length} field${changed.length === 1 ? "" : "s"}` : null,
    labelChange ? "the labels" : null,
  ].filter(Boolean).join(", ");
  await say(
    `Updated ${parts_} on ${link}. The change is logged on the issue and in its thread, ` +
      `with the previous text.${labelNote}` +
      (unreachable.length
        ? `\n\n${unreachable.map((b) => heading(b)).join(", ")} is too long to edit in a dialog; ` +
          `change that on GitHub.`
        : ""),
    moreFields,
  );
}
