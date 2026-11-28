---
name: handling-review-findings
description: Use when an ns-next review, re-review, measurement or test run has returned findings, residuals, Minors or suggestions and each must be fixed, recorded or handed on — after a task review, a final whole-branch review, a fix wave's re-review, or when a skill says to "park", "ledger" or allow "no second fix wave".
---

# Handling review findings in ns-next

## Overview

Every finding is sorted into exactly one of four kinds, and the kind alone
decides what happens to it. Neither its size nor what fixing it costs
enters the decision.

**Violating the letter of this is violating its spirit.** A finding of the
branch's topic that was not fixed on the branch is a defect shipped, under
whatever name: parked, ledgered, deferred, "left for the user to decide".

## The four kinds

| Kind | Test | What happens |
|---|---|---|
| **In topic** | The branch's own work caused it, or it lies in what the branch works on: its code, tests, docs, doc comments, plan's subject | **Fixed on the branch now**, Minors, nits, names and residuals included. No question to the user. |
| **Pre-existing, off topic** | Older than the branch, in something the branch does not work on | **Asked**: name it to the user and ask where it goes (this branch, the next fitting batch, or `docs/OPEN-WORK.md`). Neither fixed silently nor filed on your own. |
| **Future topic** | Belongs to work not begun yet (a feature that does not exist, a later batch) | **Written into `docs/OPEN-WORK.md`**, neutrally, under the topic it belongs to. Never dropped. |
| **Wrong** | Verified false against the code | Answered with the evidence, in the review loop and in your report. |

`docs/OPEN-WORK.md` takes only the third kind and what the user assigns
there. A known gap of the branch's own work never goes there.

## Rules that override other skills

- The superpowers instructions to **"park with rulings"**, to allow
  **"no second fix wave"** and to stop at a fix-loop **cap** do not apply to
  in-topic findings. A residual after the re-review gets one more fix
  commit, and that commit gets its own scoped re-review, until none is
  left; "When the rounds end" below says what each further round fixes.
- **Cost is never a reason.** A fixture to regenerate, a gate to re-run, a
  long stress run, a user waiting for the pull request: none of these moves
  a finding out of "fixed now". Do not offer the user the choice of not
  fixing it either.
- **The fix's gates run as `CLAUDE.md`'s gate table says.** A commit that
  changes more than Markdown runs `commit`. A change to the runtime after
  the end-of-batch gates ran runs them again, a comment in a shipped `.ns`
  included. Do not skip one because "nothing it exercises changed".
- Bundle the in-topic fixes of one review into one commit where they fit.
  That is how the rounds stay few, not by leaving findings.

## When the rounds end

There is no fixed number of rounds, and the rounds do not run on without
end either. Before each further round, look at what the last re-reviews
found and decide by the problem, never by the count:

| What the re-reviews show | What the next step is |
|---|---|
| **Converging**: fewer findings each round, unrelated slips (a name, a line, a stale sentence) | One more fix commit, re-reviewed |
| **A chain**: a finding is the same defect a round before fixed in another shape (one more case where two implementations answer apart, one more path past a check) | Stop fixing cases. Name the one cause and fix the cause: the invariant tested over every shape at once, an audit of every place it must hold, or a structure that removes the duplication. If fixing the cause is a design decision -- it changes semantics or reaches past the branch's plan -- lay out the options to the user and let them decide |
| **Drifting**: each round's findings lie farther from the branch's topic | Sort them again by the table above; what is older and off topic goes to the user |

"Each round found a real defect, so another round is justified" holds only
while the defects are unrelated. Two rounds in a row on the same cause mean
the next fix must be the cause's.

## Red flags — stop and fix instead

- "It's only a Minor / a name / a comment"
- "That would mean regenerating the fixture / re-running the gates"
- "The skill allows no second fix wave"
- "I'll park it as a ruling and the user can decide"
- "Merge today matters more"
- "Pre-existing, so I'll just put it in OPEN-WORK"
- "The suggestion is speculative, drop it"
- "The loop ends when a re-review is clean", said of the third finding with one cause
- "Three rounds is enough" (or "too many") -- the count decides nothing

## Common mistakes

| Mistake | Right |
|---|---|
| Residual Minors parked in the ledger after the final re-review | One more fix commit, re-reviewed |
| An unrelated old defect filed in OPEN-WORK without asking | Ask the user where it goes |
| A future-topic suggestion dropped | One neutral OPEN-WORK line under its topic |
| End-of-batch gates skipped after a "comment-only" runtime change | Run them; the table decides, not the guess |
| Offering "push now, or fix first?" for an in-topic finding | Fix first; ask only about the push |
| Patching the next case of a defect a round already fixed in another shape | Name the cause, fix it once for every shape |
| Stopping because of the number of rounds | Decide by what the findings show |
