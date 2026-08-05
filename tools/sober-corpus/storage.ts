/**
 * Disk I/O for the Sober issue corpus: the checkpoint file and raw.jsonl.
 *
 * Data lands in `tools/sober-corpus/data/` (gitignored — see the repo
 * `.gitignore` entry and ADR-017). It must never be committed: these are
 * third-party GitHub users' bug reports with no license grant to Cordial,
 * kept strictly as a local triage aid, never a redistributed artefact.
 *
 * Both `raw.jsonl` and `checkpoint.json` are written via write-to-temp-then-
 * rename so a kill mid-write can never leave a half-written file behind —
 * rename is atomic on the same filesystem, so a reader always sees either
 * the old complete file or the new complete file, never a partial one.
 *
 * `raw.jsonl` is always rewritten in full from the in-memory `Map<number,
 * IssueRecord>` (sorted by issue number) rather than appended to. This is
 * what makes "idempotent, no duplicate records" trivial: the Map is the
 * single source of truth, keyed by issue number, so re-processing the same
 * issue on a resumed or re-run pass just overwrites its entry — there is no
 * append-then-dedupe step to get wrong.
 *
 * @module sober-corpus/storage
 */

import type { Checkpoint, EvalCase, IssueRecord } from "./types.ts";
import { emptyCheckpoint } from "./types.ts";

export const DATA_DIR = `${import.meta.dirname}/data`;
export const RAW_JSONL_PATH = `${DATA_DIR}/raw.jsonl`;
export const CHECKPOINT_PATH = `${DATA_DIR}/checkpoint.json`;
export const EVAL_SET_PATH = `${DATA_DIR}/eval-set.jsonl`;

function ensureDataDir(): void {
  Deno.mkdirSync(DATA_DIR, { recursive: true });
}

function existsSync(path: string): boolean {
  try {
    Deno.statSync(path);
    return true;
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) {
      return false;
    }
    throw error;
  }
}

/** Writes `contents` to `filePath` atomically (temp file + rename on the same directory/filesystem). */
function writeFileAtomic(filePath: string, contents: string): void {
  ensureDataDir();
  const tmpPath = `${filePath}.tmp-${Deno.pid}-${Date.now()}`;
  Deno.writeTextFileSync(tmpPath, contents);
  Deno.renameSync(tmpPath, filePath);
}

export function loadCheckpoint(): Checkpoint {
  ensureDataDir();
  if (!existsSync(CHECKPOINT_PATH)) {
    return emptyCheckpoint();
  }
  const raw = Deno.readTextFileSync(CHECKPOINT_PATH);
  return JSON.parse(raw) as Checkpoint;
}

export function saveCheckpoint(checkpoint: Checkpoint): void {
  writeFileAtomic(CHECKPOINT_PATH, `${JSON.stringify(checkpoint, null, 2)}\n`);
}

/**
 * Loads the current corpus into a Map keyed by issue number. Handles a
 * corpus file written by an OLDER version of this script that used
 * append-only writes (defensive: last-line-wins on duplicate keys), even
 * though the current writer never produces duplicates itself.
 */
export function loadRawCorpus(): Map<number, IssueRecord> {
  const map = new Map<number, IssueRecord>();
  if (!existsSync(RAW_JSONL_PATH)) {
    return map;
  }
  const contents = Deno.readTextFileSync(RAW_JSONL_PATH);
  for (const line of contents.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }
    const record = JSON.parse(trimmed) as IssueRecord;
    map.set(record.number, record);
  }
  return map;
}

export function saveRawCorpus(corpus: Map<number, IssueRecord>): void {
  const sorted = [...corpus.values()].sort((a, b) => a.number - b.number);
  const contents = sorted.map((record) => JSON.stringify(record)).join("\n");
  writeFileAtomic(RAW_JSONL_PATH, contents.length > 0 ? `${contents}\n` : "");
}

/** Writes the derived triage set (derive.ts's output). Not incrementally checkpointed — a re-run regenerates it whole from raw.jsonl, which is cheap and always safe. */
export function saveEvalSet(cases: EvalCase[]): void {
  ensureDataDir();
  const contents = cases.map((c) => JSON.stringify(c)).join("\n");
  Deno.writeTextFileSync(EVAL_SET_PATH, contents.length > 0 ? `${contents}\n` : "");
}
