#!/usr/bin/env -S deno run --allow-read=.github/ISSUE_TEMPLATE
/**
 * Do the issue forms still fit the modals?
 *
 * This is the CI half of the arrangement in `templates.ts`. The bridge fetches
 * the templates at runtime so an edit needs no redeploy, which moves the
 * failure to a user pressing a button; running the same parse here means the
 * ordinary way to find out stays "before it ships".
 *
 * Prints what is dropped, because a cap nobody prints reads as full coverage.
 */
import {
  checkShortDescriptions,
  layoutWithLabels,
  MODAL_MAX_COMPONENTS,
  parseForm,
} from "./issue_forms.ts";
import { TEMPLATE_DIR } from "./repo.ts";

const DIR = TEMPLATE_DIR;

const names: string[] = [];
for await (const entry of Deno.readDir(DIR)) {
  if (entry.isFile && entry.name.endsWith(".yml") && entry.name !== "config.yml") {
    names.push(entry.name);
  }
}
names.sort();

if (!names.length) {
  console.error(`no issue forms under ${DIR}`);
  Deno.exit(1);
}

const forms = names.map((name) =>
  parseForm(name.replace(/\.ya?ml$/, ""), Deno.readTextFileSync(`${DIR}/${name}`))
);
checkShortDescriptions(forms);

for (const form of forms) {
  const dropped = form.dropped.length
    ? `  dropped (optional, offered by a follow-up): ${form.dropped.map((b) => b.id).join(", ")}`
    : "";
  console.log(
    `${form.slug.padEnd(18)} ${form.placed.length}/${MODAL_MAX_COMPONENTS} components${dropped}`,
  );
}

// With the label picker in the dialog it takes one of the five slots. A form
// that cannot spare it still works -- the picker is left out -- but that is a
// silent loss of a feature, so say so; and a layout whose leftovers outgrow the
// follow-up dialog would drop fields without a word, which is an error.
let failed = false;
for (const form of forms) {
  const layout = layoutWithLabels(form);
  if (!layout) {
    console.log(`${form.slug.padEnd(18)} cannot spare a slot for the label picker; it is left out`);
    continue;
  }
  console.log(
    `${form.slug.padEnd(18)} with the label picker: ${layout.placed.length}+1 components` +
      (layout.dropped.length
        ? `  dropped (optional, offered by a follow-up): ${
          layout.dropped.map((b) => b.id).join(", ")
        }`
        : ""),
  );
  if (layout.dropped.length > MODAL_MAX_COMPONENTS) {
    console.error(
      `${form.slug}: with the label picker, ${layout.dropped.length} optional fields overflow ` +
        `into a follow-up dialog that holds ${MODAL_MAX_COMPONENTS}`,
    );
    failed = true;
  }
}
if (failed) Deno.exit(1);
