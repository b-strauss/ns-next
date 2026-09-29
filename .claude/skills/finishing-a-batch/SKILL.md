---
name: finishing-a-batch
description: Use when a stretch of ns-next work is about to be handed back, merged into master, or declared finished — after the last commit of a batch, before a merge, or before deleting a scratch workspace under .temp/.
---

# Finishing a batch of ns-next work

## Overview

The per-commit gates in `CLAUDE.md` do not finish a batch. A batch is
finished when the end-of-batch gates have passed and a fresh session, with no
conversation history, could continue from the repository alone. Until both
hold, the work is in progress.

## Checklist

Create a todo for each item.

1. **Decide which gates apply**, from `git diff --stat master...HEAD` (or the
   batch's commits):
   - Only Markdown changed → no gates; go to step 3.
   - Anything else → the end-of-batch group in `CLAUDE.md`, "Working in this
     repository".
   - `editors/jetbrains/` changed → additionally
     `./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin`.
     Otherwise skip it.
2. **Run the gates** exactly as `CLAUDE.md` lists them: the three stress modes
   at once (each into `.temp/stress-<mode>.log`), `cargo nextest run -p
   ns-lsp`, and `run-corpus` plain and under each stress mode ("ran 6, failed
   0, skipped 205"). Read every summary line; a mode that failed is named at
   the end of the stress run. Report failures with their output.
3. **Open work.** Remove from `docs/OPEN-WORK.md` every item the batch closed;
   add every item a review, measurement or design discussion left open, with a
   pointer to where it is explained.
4. **Progress.** Update `docs/PROGRESS.md`: the milestone's entry, and "Where
   the next session starts", which must agree with the milestone table and
   with `docs/OPEN-WORK.md`.
5. **Fresh-session check**, concretely:
   - every new module or file that cuts across a subsystem appears in
     `docs/design/README.md`'s map and in the status block of each document
     describing that subsystem, not only the one that introduced it;
   - every status block still names the code as it now is;
   - `CLAUDE.md`'s crate descriptions still match.
6. **Stale statements.** Anything the batch made untrue is deleted or rewritten
   as current fact — never kept and labelled historical.
7. **Scratch space.** Before deleting anything under `.temp/`, write deferred
   findings and decisions carried forward into the repository documents.

## Common mistakes

- **Trusting the task reviews for step 5.** Each looked at one diff; none
  followed the reader's entry path (`docs/design/README.md`, then
  `docs/PROGRESS.md`), which is where the gaps are.
- **Running `verifyPlugin` for a batch that never touched the plugin.** It
  downloads several IDEs for nothing.
- **Leaving an open item only in a PROGRESS.md entry.** It belongs in
  `docs/OPEN-WORK.md`.
