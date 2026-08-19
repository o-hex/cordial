/**
 * Model access for the Sober corpus eval harness (`evaluate.ts`).
 *
 * Credential source: `OPENROUTER_API_KEY`, read from the environment (falling
 * back to a local `.env`-style file, see `loadOpenRouterKey()`). This script
 * calls `@openrouter/ai-sdk-provider`'s `createOpenRouter` directly, with no
 * billing/metering wrapper -- it is offline, internal tooling with no
 * customer or spend to attribute, so there is nothing for a metering layer
 * to bill against.
 *
 * A local `.env` file (not the repo root `.env`) is where `OPENROUTER_API_KEY`
 * lived in the dev environment this was originally built against. Since this
 * script runs via `tsx` with no dotenv loader wired up, `loadOpenRouterKey()`
 * reads it directly the first time it's needed, falling back to
 * `process.env` (so a real deployment can just export the var instead of
 * relying on a checked-out `.env` file).
 *
 * Model choice: `openai/gpt-oss-20b:free` -- NOT `openai/gpt-4o-mini`, which
 * this module used initially before a real run revealed the configured
 * `OPENROUTER_API_KEY` has zero purchased credit balance (`is_free_tier:
 * true`, `total_credits: 0` via `GET /api/v1/credits`). Topping up that
 * balance is a real-money decision that belongs to whoever owns the account,
 * not something this offline script should spend unilaterally -- so it stays
 * on the free tier, and a weaker free model is a known tradeoff, not a
 * reason to reach for a paid model without asking.
 *
 * TWO REAL LIMITS HIT BUILDING THIS, both worth knowing before re-running:
 *
 * 1. `generateObject` (the AI SDK's strict-schema structured-output helper)
 *    is UNUSABLE with `openai/gpt-oss-20b:free` on OpenRouter as of this
 *    writing -- it failed on effectively every call with "no object
 *    generated: could not parse the response" / "response did not match
 *    schema" / "the model did not return a response". A standalone repro
 *    isolated why: asked (via `generateObject`'s own auto-injected
 *    schema-following instructions) to classify sentiment, the model
 *    replied with the bare word `Positive` -- not JSON at all. The SAME
 *    model, given an explicit "respond with ONLY a JSON object like {...}"
 *    instruction written directly into the prompt text, replied cleanly
 *    with `{"sentiment":"positive"}`. This is why `judgeDiagnosis`/
 *    `triageForCordial` below use `generateText` + a hand-written JSON-shape
 *    instruction + manual `JSON.parse`/zod validation (`generateJsonObject`),
 *    not `generateObject`. If a future model swap restores `generateObject`
 *    compatibility, that's a nice simplification to make then -- don't
 *    assume it works without testing first, this model didn't.
 *
 * 2. OpenRouter's free-tier DAILY request quota: accounts with $0 purchased
 *    credit get **50 free-model requests/day**; adding just $10 in credits
 *    (kept even if the balance later drops) raises that to **1,000/day**
 *    (there's also a 20 req/min ceiling, hit less often in practice).
 *    Verified against OpenRouter's own support docs, not assumed. Failed
 *    attempts count against the quota too. This account (`total_credits: 0`)
 *    hit the wall partway through a 42-case calibration re-run (~3 calls/case)
 *    after a day of iteration/testing had already spent most of the quota --
 *    the error is unambiguous (`"Rate limit exceeded: free-models-per-day.
 *    Add 10 credits to unlock 1000 free model requests per day"`) and NOT
 *    something retries fix; it clears on OpenRouter's own daily reset. At 50
 *    requests/day and ~3 calls/case, this harness's 446-case eval set would
 *    take roughly 25+ calendar days to run to completion on the free tier
 *    with zero credits -- a real planning constraint for whoever resumes
 *    this, not a one-off hiccup. Whether to buy the $10 credit unlock is a
 *    real-money decision for whoever owns the OpenRouter account, not this
 *    script's to make.
 *
 * @module sober-corpus/model
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createOpenRouter } from '@openrouter/ai-sdk-provider'
import { generateText } from 'ai'
import { z } from 'zod'
import { CORDIAL_ARCHITECTURE_CONTEXT, CORDIAL_KNOWN_ISSUES } from './cordial-context'
import type { DiagnosticScore, EvalCase, TriageClass } from './types'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

/** Default model for all three roles (subject/judge/triage), override per-role via env or CLI. Free tier -- see module docstring. */
export const DEFAULT_MODEL = 'openai/gpt-oss-20b:free'

const MAX_ATTEMPTS = 3
const RETRY_BASE_DELAY_MS = 2000

/**
 * Explicit per-role output-token caps. Without these, the AI SDK leaves
 * `maxOutputTokens` unset and providers default to the model's full context
 * max (16384 for this model) -- which OpenRouter reserves against the
 * account's credit balance UP FRONT, regardless of how many tokens the
 * response actually uses. That over-reservation is what turned a low (not
 * zero) balance into a hard wall of "insufficient credits" errors on
 * responses that were actually only a few hundred tokens long. Each cap
 * below is sized generously (2-3x) over the prompts' own stated length
 * targets, not the provider's context max.
 */
const SUBJECT_MAX_OUTPUT_TOKENS = 600 // prompt asks for "under 200 words"
const JUDGE_MAX_OUTPUT_TOKENS = 400 // verdict enum + a 1-2 sentence rationale
const TRIAGE_MAX_OUTPUT_TOKENS = 700 // class + rationale + an optional carriedFix sentence or two

/**
 * `openai/gpt-oss-20b` (the free-tier default, see module docstring) is a
 * reasoning model -- OpenRouter's endpoint metadata for it lists `reasoning`/
 * `reasoning_effort` as supported parameters. Reasoning tokens are billed
 * against the SAME `maxOutputTokens` budget as the final answer, and a first
 * real run against this model showed `generateObject` failing near-100% of
 * the time ("no object generated: could not parse the response" / "did not
 * match schema" / "did not return a response") -- consistent with the model
 * spending its entire output budget on hidden chain-of-thought and leaving
 * nothing for the actual JSON. Explicitly capping reasoning to a small,
 * separate budget and excluding it from the response (per
 * `@openrouter/ai-sdk-provider`'s README, `providerOptions.openrouter.reasoning`)
 * leaves the rest of each call's `maxOutputTokens` for the answer itself.
 */
function reasoningProviderOptions(maxReasoningTokens: number) {
  return {
    openrouter: {
      reasoning: { max_tokens: maxReasoningTokens, exclude: true },
    },
  }
}

/** Parses a simple KEY=VALUE dotenv-style file. */
function parseEnvFile(filePath: string): Record<string, string> {
  if (!fs.existsSync(filePath)) {
    return {}
  }
  const result: Record<string, string> = {}
  for (const rawLine of fs.readFileSync(filePath, 'utf8').split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line || line.startsWith('#')) continue
    const eq = line.indexOf('=')
    if (eq === -1) continue
    const key = line.slice(0, eq).trim()
    let value = line.slice(eq + 1).trim()
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1)
    }
    result[key] = value
  }
  return result
}

let cachedApiKey: string | null = null

/** Resolves `OPENROUTER_API_KEY` from process.env, falling back to a local `.env` file. Throws if neither has it. */
export function loadOpenRouterKey(): string {
  if (cachedApiKey) {
    return cachedApiKey
  }
  if (process.env.OPENROUTER_API_KEY) {
    cachedApiKey = process.env.OPENROUTER_API_KEY
    return cachedApiKey
  }
  const localEnv = parseEnvFile(path.resolve(__dirname, '.env'))
  if (localEnv.OPENROUTER_API_KEY) {
    cachedApiKey = localEnv.OPENROUTER_API_KEY
    process.env.OPENROUTER_API_KEY = localEnv.OPENROUTER_API_KEY
    return cachedApiKey
  }
  throw new Error(
    'No OPENROUTER_API_KEY found in process.env or a local .env file. Set it and re-run, rather than ' +
      'improvising a different provider.',
  )
}

let cachedClient: ReturnType<typeof createOpenRouter> | null = null

function client(): ReturnType<typeof createOpenRouter> {
  if (!cachedClient) {
    cachedClient = createOpenRouter({
      apiKey: loadOpenRouterKey(),
      appName: process.env.OPENROUTER_APP_NAME ?? 'sober-corpus',
      appUrl: process.env.OPENROUTER_APP_URL,
    })
  }
  return cachedClient
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Retries a model call a few times with linear backoff -- transient 429s/5xx are common on shared OpenRouter capacity. */
async function withRetry<T>(label: string, fn: () => Promise<T>): Promise<T> {
  let lastError: unknown
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    try {
      return await fn()
    } catch (error) {
      lastError = error
      if (attempt < MAX_ATTEMPTS) {
        await sleep(RETRY_BASE_DELAY_MS * attempt)
      }
    }
  }
  throw new Error(`${label} failed after ${MAX_ATTEMPTS} attempts: ${lastError instanceof Error ? lastError.message : String(lastError)}`)
}

/**
 * Extracts a JSON object substring from raw model text: strips a Markdown
 * code fence if present, then takes the first balanced-looking `{...}` span.
 * Needed because `generateText` (unlike `generateObject`) gives back
 * whatever the model actually said, with no enforcement.
 */
function extractJsonObjectText(raw: string): string {
  let text = raw.trim()
  const fenceMatch = text.match(/```(?:json)?\s*([\s\S]*?)```/i)
  if (fenceMatch?.[1]) {
    text = fenceMatch[1].trim()
  }
  const firstBrace = text.indexOf('{')
  const lastBrace = text.lastIndexOf('}')
  if (firstBrace !== -1 && lastBrace > firstBrace) {
    text = text.slice(firstBrace, lastBrace + 1)
  }
  return text
}

/**
 * Structured-output helper that does NOT use `ai`'s `generateObject`.
 *
 * Why: a real run against `openai/gpt-oss-20b:free` (this harness's model,
 * see the module docstring) showed `generateObject` failing on effectively
 * every call -- "no object generated: could not parse the response". A
 * standalone repro isolated the cause: asked to classify sentiment,
 * `generateObject`'s auto-injected schema-following instructions got back the
 * bare word `Positive` (not JSON at all), while the SAME model given an
 * explicit "respond with ONLY a JSON object like {...}" instruction in the
 * prompt text itself replied with clean, parseable JSON
 * (`{"sentiment":"positive"}`). This free/open-weight model evidently doesn't
 * reliably honor whatever mechanism `generateObject` uses for this provider
 * route (tool-calling or an implicit schema description), but it DOES follow
 * an explicit, spelled-out instruction in plain prompt text. So: use
 * `generateText` with the JSON shape spelled out in the prompt, then parse
 * and validate the result against the same zod schema by hand, with a small
 * repair step (`extractJsonObjectText`) for code-fenced or prose-wrapped
 * output. Retried the same way as every other call here -- a parse/validate
 * failure just triggers another attempt.
 */
async function generateJsonObject<T>(params: {
  label: string
  model: string
  system: string
  prompt: string
  schema: z.ZodType<T>
  maxOutputTokens: number
  reasoningMaxTokens: number
}): Promise<T> {
  return withRetry(params.label, async () => {
    const { text } = await generateText({
      model: client()(params.model),
      system: params.system,
      prompt: params.prompt,
      maxOutputTokens: params.maxOutputTokens,
      providerOptions: reasoningProviderOptions(params.reasoningMaxTokens),
    })
    const jsonText = extractJsonObjectText(text)
    let parsed: unknown
    try {
      parsed = JSON.parse(jsonText)
    } catch (error) {
      throw new Error(
        `${params.label}: could not parse JSON from model response (${error instanceof Error ? error.message : String(error)}). Raw: ${text.slice(0, 200)}`,
      )
    }
    const result = params.schema.safeParse(parsed)
    if (!result.success) {
      throw new Error(`${params.label}: response did not match expected shape: ${result.error.message}. Raw: ${text.slice(0, 200)}`)
    }
    return result.data
  })
}

const SUBJECT_SYSTEM_PROMPT = `You are a senior maintainer of a Linux Roblox-compatibility project (the kind of
project that lets Roblox run natively on Linux -- via Wine, a from-scratch
compatibility runtime, or similar; you are not told which approach this
particular project uses). A user has filed the bug report below against your
project's issue tracker. Diagnose the most likely root cause and recommend a
concrete, actionable fix, the way you would actually reply on the tracker.

Be specific: name the subsystem you believe is at fault (windowing/compositor,
GPU driver, audio, input, networking/sign-in, sandbox permissions, a
specific library or flag, etc.) and give a fix someone could actually try.
Commit to your best diagnosis from the information given -- do not pad with
caveats, do not ask clarifying questions, do not hedge with "it could be
several things." Keep it under 200 words.`

export async function diagnoseIssue(problemStatement: string, model: string): Promise<string> {
  const { text } = await withRetry('subject diagnosis', () =>
    generateText({
      model: client()(model),
      system: SUBJECT_SYSTEM_PROMPT,
      prompt: problemStatement,
      maxOutputTokens: SUBJECT_MAX_OUTPUT_TOKENS,
      providerOptions: reasoningProviderOptions(150),
    }),
  )
  return text.trim()
}

const JUDGE_SYSTEM_PROMPT = `You are grading a candidate maintainer's diagnosis of a Linux-Roblox-compatibility
bug report against the ACTUAL maintainer's real resolution from the issue
tracker. Judge whether the candidate reached the same root cause and a
workable fix -- never grade on wording similarity.

Verdicts:
- "correct": the candidate's root cause AND fix substantially match the actual resolution.
- "partial": the candidate named the right subsystem/general direction but missed the specific cause, or got only part of the fix right.
- "wrong": the candidate's diagnosis contradicts, or has nothing to do with, the actual root cause.
- "unjudgeable": use ONLY when the actual resolution text itself is too thin, vague, or non-diagnostic to compare against (e.g. it says "fixed in the next build" with no explanation of what was wrong, or is itself just a triage/closing remark). Never use this because the candidate's answer was bad -- that is "wrong", not "unjudgeable". This verdict measures ground-truth quality, not candidate quality.

Give a one-to-two sentence rationale naming the specific point of agreement or disagreement.

Respond with ONLY a single valid JSON object, no markdown code fences, no text before or after it. Exact shape:
{"verdict": "correct" | "partial" | "wrong" | "unjudgeable", "rationale": "<string>"}`

const judgeSchema = z.object({
  verdict: z.enum(['correct', 'partial', 'wrong', 'unjudgeable']),
  rationale: z.string(),
})

export async function judgeDiagnosis(params: {
  problemStatement: string
  subjectDiagnosis: string
  expectedResolution: string
  model: string
}): Promise<{ verdict: DiagnosticScore; rationale: string }> {
  const object = await generateJsonObject({
    label: 'judge',
    model: params.model,
    schema: judgeSchema,
    system: JUDGE_SYSTEM_PROMPT,
    prompt:
      `PROBLEM REPORT:\n${params.problemStatement}\n\n---\n\n` +
      `CANDIDATE DIAGNOSIS:\n${params.subjectDiagnosis}\n\n---\n\n` +
      `ACTUAL MAINTAINER RESOLUTION (ground truth):\n${params.expectedResolution}`,
    maxOutputTokens: JUDGE_MAX_OUTPUT_TOKENS,
    reasoningMaxTokens: 150,
  })
  return { verdict: object.verdict, rationale: object.rationale }
}

const TRIAGE_SYSTEM_PROMPT = `${CORDIAL_ARCHITECTURE_CONTEXT}

---

You are triaging a Sober (vinegarhq/sober) issue tracker entry -- a Wine-based
Roblox-on-Linux project, UNRELATED to Cordial except for solving the same
end-user problem -- to decide whether its problem class plausibly applies to
Cordial too, using the architecture facts above. Judge this from the PROBLEM
CLASS and Sober's real fix, not from what any candidate diagnosis said.

Classes:
- "applies": Cordial almost certainly hits this too -- a host-level issue (GPU/Mesa/Vulkan, Wayland/X11, audio, Flatpak sandbox, filesystem paths, generic networking, FastFlags/launch-config, generic input handling) that has nothing to do with Wine specifically.
- "maybe": plausible, but depends on Cordial implementation details that cannot be confirmed without reading Cordial's source directly -- say what the uncertainty is.
- "sober-specific": an artifact of Sober's own design/packaging that Cordial's different architecture sidesteps entirely.

CRITICAL ANTI-HALLUCINATION RULE: base your classification ONLY on what the
PROBLEM REPORT and MAINTAINER'S RESOLUTION actually say, never on an assumed
default. Do NOT invent or infer "this is caused by Wine" (or WINEDLLOVERRIDES,
DXVK/VKD3D, Proton, a Windows registry, .NET-on-Wine, Wine's cookie jar, etc.)
unless the resolution or problem text ITSELF names a Wine/Windows-specific
mechanism as the cause. Sober being "a Wine-based project" does not make every
one of its bugs a Wine bug -- most Sober issues are about the host Linux
stack around Wine (GPU drivers, the compositor, the Flatpak sandbox, generic
networking, a JSON config file, a feature request, a support question), not
about Wine's Win32 translation itself. If the resolution text is a driver
issue ("no vulkan"), a version/config mismatch, a file path, a compositor
quirk, or anything that would read identically regardless of what runs
inside the Linux process, classify by THAT actual category (almost always
"applies" or "maybe") -- do not reach for "Wine" as an unstated default just
because Sober happens to be Wine-based. Reserve "sober-specific" for cases
where Wine/Windows-translation is the SUBJECT of the text, or the resolution
is Sober-project-specific in a way with no Cordial analog at all (e.g. a
request about Sober's own release process, its own GitHub repo conventions,
or banter/social replies with no technical content).

Give a one-line rationale that cites the SPECIFIC evidence from the resolution
text (quote or closely paraphrase it), not a generic architecture restatement.
If "applies", also extract the concrete, actionable fix from the maintainer
resolution as "carriedFix" (paraphrase tightly, keep it actionable -- a
sentence or two, not a copy of the whole resolution). Otherwise carriedFix
must be null. If the problem class matches one of Cordial's own known issues
listed above, set relatedCordialIssue to that issue's number; otherwise null.

Respond with ONLY a single valid JSON object, no markdown code fences, no text
before or after it. Exact shape:
{"triageClass": "applies" | "maybe" | "sober-specific", "rationale": "<string>", "carriedFix": "<string>" | null, "relatedCordialIssue": <integer> | null}`

const triageSchema = z.object({
  triageClass: z.enum(['applies', 'maybe', 'sober-specific']),
  rationale: z.string(),
  carriedFix: z.string().nullable(),
  relatedCordialIssue: z.number().int().nullable(),
})

export async function triageForCordial(params: {
  evalCase: Pick<EvalCase, 'title' | 'problemStatement' | 'expectedResolution' | 'labels'>
  model: string
}): Promise<{ triageClass: TriageClass; rationale: string; carriedFix: string | null; relatedCordialIssue: number | null }> {
  const validIssueNumbers = new Set(CORDIAL_KNOWN_ISSUES.map((i) => i.number))
  const object = await generateJsonObject({
    label: 'triage',
    model: params.model,
    schema: triageSchema,
    system: TRIAGE_SYSTEM_PROMPT,
    prompt:
      `SOBER ISSUE TITLE: ${params.evalCase.title}\n` +
      `LABELS: ${params.evalCase.labels.join(', ') || '(none)'}\n\n` +
      `PROBLEM REPORT:\n${params.evalCase.problemStatement}\n\n---\n\n` +
      `SOBER MAINTAINER'S RESOLUTION:\n${params.evalCase.expectedResolution}`,
    maxOutputTokens: TRIAGE_MAX_OUTPUT_TOKENS,
    reasoningMaxTokens: 200,
  })
  return {
    triageClass: object.triageClass,
    rationale: object.rationale,
    carriedFix: object.triageClass === 'applies' ? object.carriedFix : null,
    relatedCordialIssue:
      object.relatedCordialIssue !== null && validIssueNumbers.has(object.relatedCordialIssue)
        ? object.relatedCordialIssue
        : null,
  }
}
