import { assert, assertEquals } from "jsr:@std/assert@^1.0.8";
import {
  DEFAULT_REPORTER_LABELS,
  type Label,
  Labels,
  matches,
  MAX_SELECT_OPTIONS,
  mayApply,
  offerLabels,
  parseAllowlist,
  planLabelEdit,
  resolveSelection,
} from "./labels.ts";

const names = (labels: Label[]) => labels.map((l) => l.name);
const L = (...n: string[]): Label[] => n.map((name) => ({ name }));

/** What a tracker that has grown a taxonomy looks like. */
const repo = L(
  "bug",
  "enhancement",
  "confirmed",
  "wontfix",
  "priority: high",
  "priority:low",
  "security",
  "good first issue",
  "area:graphics",
  "area:input",
  "platform:flatpak",
  "compositor:sway",
  "gpu:nvidia",
);

Deno.test("the default allowlist is descriptive labels only, and none is unusual", () => {
  assertEquals(parseAllowlist(undefined), DEFAULT_REPORTER_LABELS);
  assertEquals(parseAllowlist("   "), DEFAULT_REPORTER_LABELS);
  assertEquals(parseAllowlist("area:*, gpu:*\nquestion"), ["area:*", "gpu:*", "question"]);
  // An empty variable is indistinguishable from an unset one, so "nothing" has
  // to be a word.
  assertEquals(parseAllowlist("none"), []);
});

Deno.test("matching is case-insensitive and * is the only wildcard", () => {
  assert(matches("area:*", "Area:Graphics"));
  assert(matches("priority*", "priority: high"));
  assert(!matches("area:*", "subarea:x"), "anchored, not a substring");
  assert(!matches("a.b", "axb"), "dots are dots, not regex");
});

Deno.test("a reporter picks only allowlisted labels, and never the protected ones", () => {
  const allow = DEFAULT_REPORTER_LABELS;
  for (const ok of ["area:graphics", "platform:flatpak", "compositor:sway", "gpu:nvidia"]) {
    assert(mayApply(ok, "reporter", allow), ok);
  }
  for (const no of ["confirmed", "wontfix", "priority: high", "priority:low", "bug", "security"]) {
    assert(!mayApply(no, "reporter", allow), no);
  }
});

Deno.test("an allowlist widened to * still cannot hand a reporter the triage labels", () => {
  // The guard that matters: somebody in a hurry sets GITHUB_REPORTER_LABELS=*.
  for (const protectedLabel of ["confirmed", "wontfix", "priority: high", "good first issue"]) {
    assert(!mayApply(protectedLabel, "reporter", ["*"]), protectedLabel);
  }
  assert(mayApply("question", "reporter", ["*"]));
});

Deno.test("a moderator may apply any label", () => {
  for (const l of repo) assert(mayApply(l.name, "moderator", []), l.name);
});

Deno.test("the picker shows a reporter the allowed labels and not the template's own", () => {
  const { offered, omitted } = offerLabels({
    known: repo,
    who: "reporter",
    allow: DEFAULT_REPORTER_LABELS,
    formSlug: "bug_report",
    exclude: ["bug"],
  });
  // bug_report prefers gpu, then compositor, then platform, then area.
  assertEquals(names(offered), [
    "gpu:nvidia",
    "compositor:sway",
    "platform:flatpak",
    "area:graphics",
    "area:input",
  ]);
  assertEquals(omitted, 0);
});

Deno.test("a moderator's picker offers everything except what the template applies", () => {
  const { offered } = offerLabels({
    known: repo,
    who: "moderator",
    allow: [],
    formSlug: "bug_report",
    exclude: ["bug"],
  });
  assert(offered.some((l) => l.name === "confirmed"));
  assert(!offered.some((l) => l.name === "bug"));
});

Deno.test("past twenty-five the form's own groups win, and the cut is reported", () => {
  const many = L(
    ...Array.from({ length: 30 }, (_, i) => `area:a${String(i).padStart(2, "0")}`),
    "gpu:amd",
    "gpu:intel",
    "compositor:kwin",
  );
  const { offered, omitted } = offerLabels({
    known: many,
    who: "reporter",
    allow: DEFAULT_REPORTER_LABELS,
    formSlug: "bug_report",
  });
  assertEquals(offered.length, MAX_SELECT_OPTIONS);
  // For a bug report the graphics stack outranks area, so those three survive
  // a cut that drops the tail of the area list.
  assertEquals(names(offered).slice(0, 3), ["gpu:amd", "gpu:intel", "compositor:kwin"]);
  assertEquals(omitted, 33 - 25, "the count that did not fit is returned, not hidden");
  assert(!names(offered).includes("area:a29"));
});

Deno.test("labels already on the issue lead the editor's menu, whatever their rank", () => {
  const { offered } = offerLabels({
    known: L("area:a", "area:b", "gpu:x"),
    who: "reporter",
    allow: DEFAULT_REPORTER_LABELS,
    formSlug: "bug_report",
    pinned: ["area:b"],
  });
  assertEquals(offered[0].name, "area:b");
});

Deno.test("a submitted selection is cut down to what this person may apply", () => {
  const { applied, rejected } = resolveSelection(
    ["GPU:NVIDIA", "confirmed", "no-such-label", "area:input", "area:input"],
    repo,
    "reporter",
    DEFAULT_REPORTER_LABELS,
  );
  // GitHub's own casing comes back, duplicates collapse, and a name that does
  // not exist is refused -- GitHub would create it otherwise.
  assertEquals(applied, ["gpu:nvidia", "area:input"]);
  assertEquals(rejected, ["confirmed", "no-such-label"]);
});

Deno.test("a moderator's selection may include triage labels", () => {
  const { applied } = resolveSelection(["confirmed"], repo, "moderator", []);
  assertEquals(applied, ["confirmed"]);
});

Deno.test("editing labels never touches what the editor was not offered", () => {
  const offered = L("area:input", "gpu:nvidia");
  const plan = planLabelEdit(
    ["bug", "confirmed", "area:input"],
    offered,
    ["gpu:nvidia"], // area:input deselected, gpu:nvidia added
  );
  assertEquals(plan.next, ["bug", "confirmed", "gpu:nvidia"]);
  assertEquals(plan.added, ["gpu:nvidia"]);
  assertEquals(plan.removed, ["area:input"]);
});

Deno.test("an unchanged selection plans no change", () => {
  const plan = planLabelEdit(["bug", "area:input"], L("area:input"), ["area:input"]);
  assertEquals(plan.added, []);
  assertEquals(plan.removed, []);
});

// ---- the cache ------------------------------------------------------------

function source(initial: Label[][]) {
  let calls = 0;
  let now = 1_000;
  const queue = [...initial];
  let failing = false;
  const labels = new Labels(
    () => {
      calls++;
      if (failing) return Promise.reject(new Error("GitHub is down"));
      return Promise.resolve(queue.length > 1 ? queue.shift()! : queue[0]);
    },
    { now: () => now, ttlMs: 600_000, retryMs: 30_000 },
  );
  return {
    labels,
    calls: () => calls,
    advance: (ms: number) => (now += ms),
    fail: (on: boolean) => (failing = on),
  };
}

Deno.test("labels are fetched once and served from the cache until the TTL passes", async () => {
  const s = source([L("a"), L("a", "b")]);
  assertEquals(names((await s.labels.get())!), ["a"]);
  s.advance(599_000);
  await s.labels.get();
  assertEquals(s.calls(), 1);
  s.advance(2_000);
  assertEquals(names((await s.labels.get())!), ["a", "b"]);
  assertEquals(s.calls(), 2);
});

Deno.test("a failed refresh keeps serving the last good list and retries soon", async () => {
  const s = source([L("a")]);
  await s.labels.get();
  s.advance(700_000);
  s.fail(true);
  assertEquals(names((await s.labels.get())!), ["a"], "stale beats nothing");
  assertEquals(s.calls(), 2);
  assert(s.labels.status.failure?.includes("GitHub is down"));

  // Not on every press: the retry is spaced.
  await s.labels.get();
  assertEquals(s.calls(), 2);
  s.advance(31_000);
  s.fail(false);
  await s.labels.get();
  assertEquals(s.calls(), 3);
  assertEquals(s.labels.status.failure, null);
});

Deno.test("with no list at all the answer is null, so the form files without a picker", async () => {
  const s = source([L("a")]);
  s.fail(true);
  assertEquals(await s.labels.get(), null);
  assertEquals(await s.labels.get(), null);
  assertEquals(s.calls(), 1, "and the failure is not retried on every press");
});

Deno.test("simultaneous callers share one fetch", async () => {
  const s = source([L("a")]);
  await Promise.all([s.labels.get(), s.labels.get(), s.labels.get()]);
  assertEquals(s.calls(), 1);
});

Deno.test("a caller on a deadline gets the stale list instead of waiting", async () => {
  let release!: (l: Label[]) => void;
  let first = true;
  const labels = new Labels(() => {
    if (first) {
      first = false;
      return Promise.resolve(L("old"));
    }
    return new Promise<Label[]>((resolve) => (release = resolve));
  }, { ttlMs: 0 });
  await labels.get();
  const started = Date.now();
  const got = await labels.get(20);
  assert(Date.now() - started < 1000);
  assertEquals(names(got!), ["old"]);
  release(L("new")); // let the dangling refresh settle
  await new Promise((r) => setTimeout(r, 5));
});
