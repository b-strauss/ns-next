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

1. **Decide which gates apply** from `CLAUDE.md`'s table "The gates", the
   rows for the end of a batch, against `git diff --stat master...HEAD` (or the
   batch's commits). Only Markdown changed → no gates; go to step 3.
2. **Run the gates** the table names: for a batch that changed the runtime
   `cargo xtask gates end-of-batch` (`images`, `caches`, `benchmarks`,
   `release-checks`, `codegen`, `stress`, and in Docker `linux` and
   `aarch64`), and `plugin-verification` beside it for one
   that changed the plugin. The stress modes act only in debug builds: MOVES
   moves every object at the run loop's safepoints, CACHES vacates every inline
   cache at every safepoint, COLLECT collects at every safepoint, SCAVENGE
   verifies the remembered set and scavenges at every safepoint, WIDE encodes
   every instruction in its wide form, SUSPEND suspends every activation into
   its frame and runs on from the frames, JIT compiles every body at its first
   entry and enters it at every push even when its machine code exits at once, EXIT makes every poll in
   machine code take its exit, ZONE compacts the code zone at every safepoint,
   freeing every body no activation runs -- every other compaction keeping
   each body that has optimized code, which it slides -- OPTIMIZE arms every body's
   optimization budget at 0, so that the next safepoint compiles its optimized
   code once its baseline code first spends -- under it the back-off after an
   abandonment or a trap is void, the budget times 4 to the version still 0 --
   DEOPT does what OPTIMIZE does and
   makes optimized code leave at every n-th interrupt point it passes, n
   drawn afresh at each safepoint, and INVALIDATE does what OPTIMIZE does and
   makes optimized code leave for a safepoint at every tenth interrupt point
   it passes, which frees every body's optimized copies -- a body freed
   often sitting out ever more rounds -- before it installs
   what was requested since; beside them the suite runs once with
   `NS_NEXT_NO_JIT=1`, the interpreter alone. `stress` runs the thirteen suites at once,
   because each ends in a long tail of a few depth-limit tests that leaves most
   cores idle and the tails overlap (measured 2026-10-01 for the first three:
   277 s, MOVES the longest, against 703 s, the sum of the three per-mode times,
   one after another), each into `.temp/stress-<MODE>.log` (`NO_JIT` for the
   run without the JIT), and prints each
   summary line; each must report every test passed, and `run-corpus` "ran 6,
   failed 0, skipped 205". Report failures with their output.
3. **Open work.** Remove from `docs/OPEN-WORK.md` every item the batch closed;
   add every item a review, measurement or design discussion left open, with a
   pointer to where it is explained.
4. **Progress.** Update `docs/PROGRESS.md`: the milestone's entry and the
   milestone table; and `docs/OPEN-WORK.md`'s "Next", which must agree with
   the table.
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
  `docs/OPEN-WORK.md`'s "Next"), which is where the gaps are.
- **Running `verifyPlugin` for a batch that never touched the plugin.** It
  downloads several IDEs for nothing.
- **Leaving an open item only in a PROGRESS.md entry.** It belongs in
  `docs/OPEN-WORK.md`.
