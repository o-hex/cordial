# rescued

Material kept off to one side, deliberately not part of the project. Nothing
under here is built, imported, or referenced by anything in the tree — it
sits here because it was worth keeping around rather than deleting outright,
not because it belongs in the codebase proper.

This whole directory is gitignored (see `.gitignore`) and must never be
committed.

## sober-eval-harness/

An LLM-scoring harness for the Sober issue corpus (`tools/sober-corpus/`):
it ran a model's diagnosis of a real Sober issue against the maintainer's
actual resolution and graded the result. It was cancelled before it shipped
— no OpenRouter-backed scoring in this project — so it was never wired into
`tools/sober-corpus/`. It's kept here only in case that decision is
revisited later, not as something to run or maintain in the meantime.
