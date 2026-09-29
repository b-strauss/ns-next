---
name: making-design-decisions
description: Use when an ns-next architecture or semantics question needs an answer or a set of options for the user — how a mechanism should work, which of several designs to build, whether the specification requires something, or whether a measurement argues against a technique other engines use.
---

# Making design decisions in ns-next

## Overview

A design answer in this repository rests on three things read before any
recommendation is formed: the specification, the prior art, and the run-time
cost. The recommendation comes last, once, and does not move afterwards. The
principles that decide between options are in `docs/design/README.md`,
"Principles".

## The order

1. **The specification.** Search all of `docs/references/newspeak-spec.md`,
   not only the section whose heading matches — a rule often sits on an
   unrelated page (§3.2 decides how the primary factory is found). Cite the
   PDF's section. The older papers in `docs/references/` are background only;
   where the specification is silent, the canonical implementation
   (`prior-art/newspeak/`) decides. Before calling the specification silent,
   state how widely it was searched. Where cheap, probe the runtime's current
   behaviour with a small program before calling something a deviation.
2. **The prior art.** For each relevant system under `prior-art/`, say what it
   actually does, with file and line citations: Cog/OpenSmalltalk for
   activations and dispatch, Primordial Soup and SOMns for Newspeak itself,
   Strongtalk for inline caches and deoptimization, Nova for Rust arenas and
   handles. V8 and HotSpot where the design documents already cite them.
   `prior-art/PROVENANCE.md` says what each of the ten is for. A plain clone
   does not fetch them: `git submodule update --init <name>`, by name, since
   `opensmalltalk-vm` alone is ~255 MB. Nothing there is compiled or linked;
   copying code out of one is a licensing decision (`nova` is MPL-2.0, the
   most restrictive), made so far only for `num-bigint` and `num-integer`
   under MIT, with notices in `THIRD-PARTY-NOTICES.md` and a header in every
   adapted file.
3. **The constraints the repository already fixes.** Read the design document
   for the aspect (`docs/design/README.md` maps them) and `CLAUDE.md`'s
   invariants; an option that breaks one is not an option.
4. **The cost of each option**: the hot-path cost, the memory, and what it
   forecloses later (the JIT, edit-and-continue, images). Compare by the
   finished system, not by the next step's gain.
5. **One recommendation**, stated once, with the reasoning above behind it.

## Quick reference

| Situation | What to do |
|---|---|
| A measurement says a prior-art technique does not pay | First establish why the prior art uses it, and whether the measurement covered those reasons; state the measurement's scope before concluding |
| An option keeps a specification deviation | Mark it as a defect that option leaves in place, not as "smaller scope" |
| "How do we stop a program from doing X" | First try arranging the shipped Newspeak classes (nesting, access control, facets); a runtime check only where the language cannot express it |
| An option needs `unsafe` | Not an argument by itself; name the guarantee it gives up or the complexity it adds |
| A construct is rare | Irrelevant; valid syntax behaves consistently |
| Asked "did you miss anything?" before finalizing | Go section by section through the documents being extended or replaced, not an impressionistic pass |

## Common mistakes

- **Options derived from the ns-next tree alone.** Prior art repeatedly changed
  the answer rather than decorating it; a one-line nod per option is not the
  research.
- **A recommendation attached to each round of options.** When the research
  happens between rounds, the recommendation shifts, and a shifting
  recommendation is worse than none. Finish steps 1–4 first.
- **"Correct within the current model."** Worth nothing when the model itself
  deviates from the specification.
- **Settled vs. proposed.** When summarizing, list as decided only what the
  user decided; own proposals stay marked open.

## Afterwards

A decision lands in the living design document for that aspect, in the
present tense, and anything it leaves open goes into `docs/OPEN-WORK.md`.
