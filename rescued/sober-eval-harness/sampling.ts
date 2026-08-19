/**
 * Deterministic stratified sampling for the calibration run. No randomness -- reproducibility
 * matters more than true-random sampling here, since a calibration run
 * should be re-runnable with `--force` and land on the SAME 40 cases every
 * time, otherwise "re-calibrate after fixing the judge prompt" would silently
 * grade a different sample and the before/after comparison would be
 * meaningless.
 *
 * Strategy: bucket by (state, expectedResolution length tercile), sort so
 * same-bucket cases sit adjacent, then take evenly-spaced indices across the
 * whole sorted list. This spreads the sample across every bucket roughly
 * proportional to that bucket's share of the corpus, without needing to hand
 * -tune per-bucket quotas.
 *
 * @module sober-corpus/sampling
 */

import type { EvalCase } from './types'

type LengthBucket = 'short' | 'medium' | 'long'

function lengthBucket(resolution: string): LengthBucket {
  if (resolution.length < 300) return 'short'
  if (resolution.length < 900) return 'medium'
  return 'long'
}

function stratumKey(evalCase: EvalCase): string {
  return `${evalCase.state}:${lengthBucket(evalCase.expectedResolution)}`
}

/**
 * Selects `sampleSize` cases from `cases`, stratified by state x resolution
 * -length bucket, evenly spaced across the stratified ordering. Deterministic:
 * same input always produces the same sample. Returns fewer than `sampleSize`
 * if `cases` is smaller.
 */
export function stratifiedSample(cases: readonly EvalCase[], sampleSize: number): EvalCase[] {
  if (cases.length <= sampleSize) {
    return [...cases]
  }

  const sorted = [...cases].sort((a, b) => {
    const ak = stratumKey(a)
    const bk = stratumKey(b)
    if (ak !== bk) return ak < bk ? -1 : 1
    return a.issueNumber - b.issueNumber
  })

  const step = sorted.length / sampleSize
  const pickedIndices = new Set<number>()
  for (let i = 0; i < sampleSize; i++) {
    let idx = Math.floor(i * step)
    while (pickedIndices.has(idx) && idx < sorted.length - 1) {
      idx++
    }
    pickedIndices.add(idx)
  }

  return [...pickedIndices]
    .map((idx) => sorted[idx]!)
    .sort((a, b) => a.issueNumber - b.issueNumber)
}

/** Summarizes a sample's composition for the calibration report (state / length-bucket / label mix). */
export function summarizeSample(cases: readonly EvalCase[]): {
  byState: Record<string, number>
  byLengthBucket: Record<LengthBucket, number>
  distinctLabels: number
  labelCounts: Record<string, number>
} {
  const byState: Record<string, number> = {}
  const byLengthBucket: Record<LengthBucket, number> = { short: 0, medium: 0, long: 0 }
  const labelCounts: Record<string, number> = {}

  for (const c of cases) {
    byState[c.state] = (byState[c.state] ?? 0) + 1
    byLengthBucket[lengthBucket(c.expectedResolution)]++
    for (const label of c.labels) {
      labelCounts[label] = (labelCounts[label] ?? 0) + 1
    }
  }

  return { byState, byLengthBucket, distinctLabels: Object.keys(labelCounts).length, labelCounts }
}
