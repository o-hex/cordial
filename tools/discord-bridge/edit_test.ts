import { TEMPLATE_DIR } from "./repo.ts";
import { assert, assertEquals, assertStringIncludes } from "jsr:@std/assert@^1.0.8";
import { type IssueForm, parseForm } from "./issue_forms.ts";
import { parseBody, renderIssueBody } from "./issue_body.ts";
import {
  auditComment,
  type AuditEntry,
  auditThreadLine,
  cleanTitle,
  editableFields,
  fenced,
  fieldsForPart,
  fingerprint,
  FIRST_PART_FIELDS,
  inferForm,
  LATER_PART_FIELDS,
  partCount,
  tooLongToEdit,
} from "./edit.ts";

const forms: IssueForm[] = [...Deno.readDirSync(TEMPLATE_DIR)]
  .filter((e) => e.isFile && e.name.endsWith(".yml") && e.name !== "config.yml")
  .map((e) =>
    parseForm(e.name.replace(/\.yml$/, ""), Deno.readTextFileSync(`${TEMPLATE_DIR}/${e.name}`))
  );
const bug = forms.find((f) => f.slug === "bug_report")!;

/** Every field of a form answered, so every section exists. */
function fullyFilled(form: IssueForm) {
  const values: Record<string, string> = {};
  for (const b of form.fields) {
    if (b.id) values[b.id] = b.type === "dropdown" ? (b.attributes?.options?.[0] ?? "x") : "text";
  }
  return renderIssueBody(form, { values, reporter: { id: "42", tag: "t" } }, "999");
}

Deno.test("the diagnostics block and dropdowns are never offered for editing", () => {
  for (const form of forms) {
    const parsed = parseBody(form, fullyFilled(form));
    assert(parsed, `${form.slug} should parse`);
    const ids = editableFields(form, parsed).map((b) => b.id);
    assert(!ids.includes("diagnostics"), `${form.slug} offers diagnostics`);
    for (const b of form.fields.filter((f) => f.type === "dropdown")) {
      assert(!ids.includes(b.id), `${form.slug} offers dropdown ${b.id}`);
    }
  }
});

Deno.test("no dialog part ever holds more than a modal does, and the parts cover every field", () => {
  for (const form of forms) {
    const fields = editableFields(form, parseBody(form, fullyFilled(form)));
    const seen: string[] = [];
    for (let part = 0; part < partCount(fields); part++) {
      const here = fieldsForPart(fields, part);
      // Part 0 also carries a title and a label menu: five in all.
      const total = part === 0 ? here.length + 2 : here.length;
      assert(total <= 5, `${form.slug} part ${part} has ${total} components`);
      assert(here.length <= (part === 0 ? FIRST_PART_FIELDS : LATER_PART_FIELDS));
      seen.push(...here.map((b) => b.id!));
    }
    assertEquals(seen, fields.map((b) => b.id), `${form.slug}: every field is in exactly one part`);
  }
});

Deno.test("bug_report is edited in two parts: title, labels and three fields, then five", () => {
  const fields = editableFields(bug, parseBody(bug, fullyFilled(bug)));
  assertEquals(fields.length, 8, "nine fields, one of them diagnostics");
  assertEquals(partCount(fields), 2);
  assertEquals(fieldsForPart(fields, 0).map((b) => b.id), [
    "what-happened",
    "what-expected",
    "repro",
  ]);
  assertEquals(fieldsForPart(fields, 1).length, 5);
});

Deno.test("an answer too long for its box is left out rather than truncated into it", () => {
  const body = renderIssueBody(bug, {
    values: {
      "what-happened": "ok",
      "what-expected": "ok",
      "repro": "ok",
      "diagnostics": "d",
      "runs-attempted": "x".repeat(1200), // a one-line field is capped at 1000
    },
    reporter: { id: "1", tag: "t" },
  }, "9");
  const parsed = parseBody(bug, body);
  assert(parsed);
  assert(!editableFields(bug, parsed).some((b) => b.id === "runs-attempted"));
  assertEquals(tooLongToEdit(bug, parsed).map((b) => b.id), ["runs-attempted"]);
});

Deno.test("a body that cannot be parsed offers no fields, only title and labels", () => {
  assertEquals(editableFields(bug, null), []);
  assertEquals(partCount([]), 1);
});

Deno.test("the fingerprint moves with the title, the body and the labels, and not their order", async () => {
  const base = { title: "[Bug]: a", body: "text", labels: ["bug", "area:x"] };
  const same = await fingerprint({ ...base, labels: ["area:x", "bug"] });
  assertEquals(await fingerprint(base), same);
  assert(await fingerprint({ ...base, title: "[Bug]: b" }) !== same);
  assert(await fingerprint({ ...base, body: "texts" }) !== same);
  assert(await fingerprint({ ...base, labels: ["bug"] }) !== same);
  assert(same.length <= 16 && /^[0-9a-f]+$/.test(same));
});

Deno.test("the template is read from the marker, and inferred for issues filed before it", () => {
  const filed = fullyFilled(bug);
  assertEquals(inferForm(forms, filed, "bug_report")?.slug, "bug_report");
  // No recorded slug: the headings pick it out.
  const unmarked = filed.replace(" form=bug_report", "");
  assertEquals(inferForm(forms, unmarked, null)?.slug, "bug_report");
  // A recorded slug for a template that no longer exists falls back to headings.
  assertEquals(inferForm(forms, filed, "deleted_form")?.slug, "bug_report");
  // Nothing to go on.
  assertEquals(inferForm(forms, "just words", null), null);
  assertEquals(inferForm(forms, null, null), null);
});

Deno.test("a title is one line and no longer than GitHub allows", () => {
  assertEquals(cleanTitle("  [Bug]:  two\n lines\tand  gaps "), "[Bug]: two lines and gaps");
  assertEquals(cleanTitle("x".repeat(400)).length, 256);
  assertEquals(cleanTitle("  \n "), "");
});

// ---- the audit trail -------------------------------------------------------

const entry: AuditEntry = {
  number: 12,
  editor: { id: "9", tag: "Someone" },
  as: "reporter",
  at: new Date("2026-10-01T12:34:56.789Z"),
  title: { before: "[Bug]: black window", after: "[Bug]: black window on Sway" },
  fields: [{ label: "What happened", before: "It is black.", after: "It is black on Sway 1.10." }],
  labels: { added: ["compositor:sway"], removed: ["area:input"] },
};

Deno.test("the audit comment says who, when and what, and keeps the previous text", () => {
  const comment = auditComment(entry);
  assertStringIncludes(comment, "**Edited from Discord** by `Someone` (`9`, the reporter)");
  assertStringIncludes(comment, "2026-10-01 12:34:56 UTC");
  assertStringIncludes(comment, "**Title:** `[Bug]: black window` → `[Bug]: black window on Sway`");
  assertStringIncludes(comment, "**What happened:** 12 → 25 characters");
  assertStringIncludes(comment, "**Labels:** added `compositor:sway`; removed `area:input`");
  assertStringIncludes(comment, "<details>");
  assertStringIncludes(comment, "It is black.");
  assert(comment.indexOf("<details>") < comment.indexOf("It is black."), "inside the details");
  assert(comment.trimEnd().endsWith("</details>"));
});

Deno.test("a moderator's edit is labelled as one", () => {
  assertStringIncludes(auditComment({ ...entry, as: "moderator" }), "as a moderator");
});

Deno.test("an edit that touched only labels or only the title still reads sensibly", () => {
  const labelsOnly = auditComment({
    ...entry,
    title: undefined,
    fields: [],
    labels: { added: ["gpu:amd"], removed: [] },
  });
  assertStringIncludes(labelsOnly, "added `gpu:amd`");
  assert(!labelsOnly.includes("**Title:**"));
});

Deno.test("quoted text cannot break out of the audit comment", () => {
  const hostile = "```\n</details>\n**Edited from Discord** by `an admin`\n````";
  const comment = auditComment({
    ...entry,
    editor: { id: "9", tag: "evil`name`\n# heading" },
    fields: [{ label: "What happened", before: hostile, after: "x" }],
  });
  // The fence outgrows the longest run of backticks inside the text.
  assert(comment.includes("`````\n" + hostile + "\n`````"), comment);
  // And a name with backticks or newlines stays inside its own inline code.
  assertStringIncludes(comment, "by `evil'name' # heading`");
  assertEquals(fenced("a ``` b"), "````\na ``` b\n````");
});

Deno.test("an enormous previous text is cut under GitHub's limit and says so", () => {
  const comment = auditComment({
    ...entry,
    fields: Array.from({ length: 8 }, (_, i) => ({
      label: `Field ${i}`,
      before: "y".repeat(4000 * 4),
      after: "z",
    })),
  });
  assert(comment.length < 65_000, `${comment.length} characters`);
  assertStringIncludes(comment, "Cut here for length");
  assert(comment.trimEnd().endsWith("</details>"));
});

Deno.test("the thread line names the editor, the time and each kind of change", () => {
  const line = auditThreadLine(entry, "https://github.com/o/r/issues/12#issuecomment-5");
  assertStringIncludes(line, "Someone (9)");
  assertStringIncludes(line, "2026-10-01 12:34:56 UTC");
  assertStringIncludes(line, "title changed");
  assertStringIncludes(line, "What happened changed");
  assertStringIncludes(line, "labels +compositor:sway −area:input");
  assertStringIncludes(line, "<https://github.com/o/r/issues/12#issuecomment-5>");
  assert(line.length <= 1900);
});
