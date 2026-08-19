#!/usr/bin/env tsx
/**
 * Evaluation harness over the derived Sober issue corpus
 * (`data/eval-set.jsonl`, 446 cases as of the corpus this was built against).
 * Produces TWO scores per case (this corpus is Cordial-facing prior art, not
 * an abstract diagnostic-reasoning benchmark):
 *
 *   1. Diagnostic score: a subject model is shown ONLY `problemStatement`
 *      (never `expectedResolution`) and asked to diagnose root cause + fix,
 *      as a maintainer of a Linux Roblox-compatibility project. A separate
 *      judge call then grades that diagnosis against the real maintainer's
 *      resolution: correct / partial / wrong / unjudgeable (the last meaning
 *      the GROUND TRUTH was too thin to grade against, not that the
 *      candidate was wrong -- see model.ts's judge prompt).
 *
 *   2. Cordial-applicability triage: independent of what the subject
 *      diagnosed, classifies whether the PROBLEM CLASS plausibly applies to
 *      `luohoa97/cordial` (a real, different-architecture Roblox-on-Linux
 *      runtime the same team maintains) -- applies / maybe / sober-specific
 *      -- grounded in Cordial's actual README/HANDOVER.md (see
 *      cordial-context.ts). Where "applies", carries across Sober's concrete
 *      fix as reusable prior art.
 *
 * Resumable and idempotent the same way `fetch.ts` is: `data/eval-results.jsonl`
 * is keyed by issue number (storage.ts's `loadEvalResults`/`saveEvalResults`),
 * written atomically after EVERY completed case, not batched at the end. A
 * case counts as "done" only once subject+judge+triage all succeed -- a
 * killed or failed case is simply absent from the map and gets retried
 * automatically on the next invocation. Cases run with bounded concurrency
 * (`--concurrency`, default 4): a kill mid-run can lose at most that many
 * in-flight cases, never previously-completed ones.
 *
 * Usage:
 *   pnpm sober-corpus:evaluate                                 # process all not-yet-done cases
 *   pnpm sober-corpus:evaluate -- --strategy=stratified --limit=40   # calibration sample
 *   pnpm sober-corpus:evaluate -- --limit=100                  # cap this run to 100 new cases
 *   pnpm sober-corpus:evaluate -- --force --limit=40           # re-run (overwrite) 40 cases, e.g. after a prompt change
 *   pnpm sober-corpus:evaluate -- --force --issues=10,11,35    # re-run (overwrite) SPECIFIC issue numbers -- for
 *                                                               # recalibrating the exact same sample after a prompt
 *                                                               # fix, so a before/after comparison is apples-to-apples
 *   pnpm sober-corpus:evaluate -- --subject-model=openai/gpt-4o-mini --judge-model=openai/gpt-4o-mini
 *
 * @module sober-corpus/evaluate
 */

import { emitEvent } from './github-graphql'
import { diagnoseIssue, DEFAULT_MODEL, judgeDiagnosis, triageForCordial } from './model'
import { stratifiedSample, summarizeSample } from './sampling'
import { loadEvalResults, loadEvalSet, saveEvalResults } from './storage'
import type { DiagnosticScore, EvalCase, EvalResult, TriageClass } from './types'

interface Args {
  limit?: number
  strategy: 'sequential' | 'stratified'
  force: boolean
  concurrency: number
  issues?: number[]
  subjectModel: string
  judgeModel: string
  triageModel: string
}

function parseArgs(argv: string[]): Args {
  const raw: Record<string, string | boolean> = {}
  for (const arg of argv) {
    if (!arg.startsWith('--')) continue
    const eq = arg.indexOf('=')
    if (eq === -1) {
      raw[arg.slice(2)] = true
    } else {
      raw[arg.slice(2, eq)] = arg.slice(eq + 1)
    }
  }
  return {
    limit: raw.limit !== undefined ? Number(raw.limit) : undefined,
    strategy: raw.strategy === 'stratified' ? 'stratified' : 'sequential',
    force: raw.force === true || raw.force === 'true',
    concurrency: raw.concurrency !== undefined ? Number(raw.concurrency) : 4,
    issues:
      typeof raw.issues === 'string' && raw.issues.length > 0
        ? raw.issues.split(',').map((n) => Number(n.trim()))
        : undefined,
    subjectModel: String(raw['subject-model'] ?? process.env.SOBER_EVAL_SUBJECT_MODEL ?? DEFAULT_MODEL),
    judgeModel: String(raw['judge-model'] ?? process.env.SOBER_EVAL_JUDGE_MODEL ?? DEFAULT_MODEL),
    triageModel: String(raw['triage-model'] ?? process.env.SOBER_EVAL_TRIAGE_MODEL ?? DEFAULT_MODEL),
  }
}

const emptyScoreCounts = (): Record<DiagnosticScore, number> => ({ correct: 0, partial: 0, wrong: 0, unjudgeable: 0 })
const emptyTriageCounts = (): Record<TriageClass, number> => ({ applies: 0, maybe: 0, 'sober-specific': 0 })

async function evaluateCase(
  evalCase: EvalCase,
  args: Pick<Args, 'subjectModel' | 'judgeModel' | 'triageModel'>,
): Promise<EvalResult> {
  const [subjectDiagnosis, triage] = await Promise.all([
    diagnoseIssue(evalCase.problemStatement, args.subjectModel),
    triageForCordial({ evalCase, model: args.triageModel }),
  ])
  const judge = await judgeDiagnosis({
    problemStatement: evalCase.problemStatement,
    subjectDiagnosis,
    expectedResolution: evalCase.expectedResolution,
    model: args.judgeModel,
  })

  return {
    issueNumber: evalCase.issueNumber,
    title: evalCase.title,
    subjectDiagnosis,
    diagnosticScore: judge.verdict,
    judgeRationale: judge.rationale,
    triageClass: triage.triageClass,
    triageRationale: triage.rationale,
    carriedFix: triage.carriedFix,
    relatedCordialIssue: triage.relatedCordialIssue,
    subjectModel: args.subjectModel,
    judgeModel: args.judgeModel,
    triageModel: args.triageModel,
    evaluatedAt: new Date().toISOString(),
  }
}

async function main(): Promise<void> {
  const startedAt = Date.now()
  const args = parseArgs(process.argv.slice(2))

  const allCases = loadEvalSet()
  const results = loadEvalResults()

  process.stdout.write(`\nSober corpus evaluation\n`)
  process.stdout.write(`  Eval set: ${allCases.length} case(s) (data/eval-set.jsonl)\n`)
  process.stdout.write(`  Already evaluated: ${results.size} case(s) (data/eval-results.jsonl)\n`)
  process.stdout.write(
    `  Models — subject: ${args.subjectModel}  judge: ${args.judgeModel}  triage: ${args.triageModel}\n`,
  )

  const pool = args.force ? allCases : allCases.filter((c) => !results.has(c.issueNumber))
  const selected = args.issues
    ? pool.filter((c) => args.issues!.includes(c.issueNumber))
    : args.strategy === 'stratified'
      ? stratifiedSample(pool, args.limit ?? pool.length)
      : pool.slice(0, args.limit ?? pool.length)

  if (selected.length === 0) {
    process.stdout.write(`\nNothing to do — all ${allCases.length} case(s) already evaluated.\n`)
    return
  }

  process.stdout.write(
    `  Strategy: ${args.strategy}${args.force ? ' (--force: overwriting existing results for selected cases)' : ''}\n` +
      `  This run: ${selected.length} case(s), concurrency ${args.concurrency}\n`,
  )
  if (args.strategy === 'stratified') {
    const summary = summarizeSample(selected)
    process.stdout.write(
      `  Sample mix — state: ${JSON.stringify(summary.byState)}  length buckets: ${JSON.stringify(summary.byLengthBucket)}  distinct labels: ${summary.distinctLabels}\n`,
    )
  }
  process.stdout.write('\n')

  emitEvent('sober_corpus.evaluate.run_started', {
    poolSize: pool.length,
    selectedCount: selected.length,
    strategy: args.strategy,
    force: args.force,
    subjectModel: args.subjectModel,
    judgeModel: args.judgeModel,
    triageModel: args.triageModel,
  })

  let cursor = 0
  let completedThisRun = 0
  let failedThisRun = 0
  const scoreCountsThisRun = emptyScoreCounts()
  const triageCountsThisRun = emptyTriageCounts()

  async function worker(): Promise<void> {
    for (;;) {
      const myIndex = cursor++
      if (myIndex >= selected.length) {
        return
      }
      const evalCase = selected[myIndex]!
      try {
        const result = await evaluateCase(evalCase, args)
        results.set(evalCase.issueNumber, result)
        // Durability: persist after EVERY completed case, same as fetch.ts persisting after every page —
        // a kill loses at most the in-flight cases across workers, never a previously-completed one.
        saveEvalResults(results)

        completedThisRun++
        scoreCountsThisRun[result.diagnosticScore]++
        triageCountsThisRun[result.triageClass]++

        const relatedNote = result.relatedCordialIssue ? ` (matches cordial#${result.relatedCordialIssue})` : ''
        process.stdout.write(
          `  [${completedThisRun}/${selected.length}] issue #${evalCase.issueNumber}: ` +
            `diagnostic=${result.diagnosticScore} triage=${result.triageClass}${relatedNote}\n`,
        )
        emitEvent('sober_corpus.evaluate.case_completed', {
          issueNumber: evalCase.issueNumber,
          diagnosticScore: result.diagnosticScore,
          triageClass: result.triageClass,
          relatedCordialIssue: result.relatedCordialIssue,
        })
      } catch (error) {
        failedThisRun++
        const message = error instanceof Error ? error.message : String(error)
        process.stderr.write(`  [FAILED] issue #${evalCase.issueNumber}: ${message}\n`)
        emitEvent('sober_corpus.evaluate.case_failed', { issueNumber: evalCase.issueNumber, message })
      }
    }
  }

  const workerCount = Math.max(1, Math.min(args.concurrency, selected.length))
  await Promise.all(Array.from({ length: workerCount }, () => worker()))

  const overallScore = emptyScoreCounts()
  const overallTriage = emptyTriageCounts()
  for (const r of results.values()) {
    overallScore[r.diagnosticScore]++
    overallTriage[r.triageClass]++
  }

  const elapsedS = ((Date.now() - startedAt) / 1000).toFixed(1)
  process.stdout.write(
    `\nDone in ${elapsedS}s. This run: ${completedThisRun} completed, ${failedThisRun} failed (left for next run).\n` +
      `  This run — diagnostic: ${JSON.stringify(scoreCountsThisRun)}\n` +
      `  This run — triage:     ${JSON.stringify(triageCountsThisRun)}\n` +
      `\nAccumulated across all ${results.size}/${allCases.length} evaluated case(s):\n` +
      `  diagnostic: ${JSON.stringify(overallScore)}\n` +
      `  triage:     ${JSON.stringify(overallTriage)}\n` +
      (results.size < allCases.length
        ? `\n${allCases.length - results.size} case(s) remaining. Re-run to continue.\n`
        : '\nAll cases evaluated.\n'),
  )

  emitEvent('sober_corpus.evaluate.run_completed', {
    completedThisRun,
    failedThisRun,
    totalEvaluated: results.size,
    totalCases: allCases.length,
    overallScore,
    overallTriage,
    elapsedS,
  })
}

main().catch((error) => {
  emitEvent('sober_corpus.evaluate.failed', {
    message: error instanceof Error ? error.message : String(error),
  })
  process.stderr.write(`\nEvaluation failed: ${error instanceof Error ? error.message : String(error)}\n`)
  process.exitCode = 1
})
