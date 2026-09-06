# ns-next Progress

Concise, milestone-level progress tracker for the `ns-next` project (a
native Rust runtime for Newspeak). Updated once after each milestone
completes — this is a status summary, not a task list; see
`docs/superpowers/plans/` for the detailed, checkbox-level implementation
plans and `docs/superpowers/specs/` for the design specs behind each
milestone.

| Milestone | Status | Completed | Spec | Plan(s) |
|---|---|---|---|---|
| M1 — Frontend | Complete | 2026-09-01 | [runtime-design](superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md) | [frontend](superpowers/plans/2026-09-01-2308-ns-next-frontend.md) |
| M1 — Runtime | Complete | 2026-09-04 | [runtime-design](superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md), [liveness/VM constraints](superpowers/specs/2026-09-04-1153-ns-next-liveness-images-vm-design.md) | [runtime](superpowers/plans/2026-09-04-1351-ns-next-runtime.md) |
| M2 — Bytecode + VM | Complete | 2026-09-05 | [bytecode-vm-design](superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md) | [bytecode-vm](superpowers/plans/2026-09-04-2353-ns-next-bytecode-vm.md) |
| M3 — Nested classes + source-free images | Not started | — | [nesting/mixins/source-free-images](superpowers/specs/2026-09-06-1659-ns-next-nesting-mixins-source-free-images-design.md) | [nesting/mixins/source-free-images](superpowers/plans/2026-09-06-1720-ns-next-nesting-mixins-source-free-images.md) |
| M4 — Inline caches + GC | Not started | — | — | — |
| M5–M6 — JIT | Not started | — | — | — |

## M1 — Frontend (complete, 2026-09-01)

`ns-syntax`: hand-written lexer, recursive-descent + Pratt parser, and AST
for the official Newspeak3 textual syntax, plus the `ns-next parse` /
`parse-corpus` CLI commands. Validated against the vendored corpus
(210/211 files parse; one deferred lexer ambiguity, tracked in
`corpus/blacklist.toml`).

## M1 — Runtime (complete, 2026-09-04)

`ns-runtime`: name resolver (Newspeak's real lexical/`outer`/`super`
scoping), object model (`Rc<RefCell<Object>>`), the three access-control-
aware dispatch algorithms, a tree-walking interpreter with reified
heap-reachable `Frame` activations, class instantiation per the
deferred-superclass-message model, a small Rust-native core library, and
the `ns-next run` CLI command. 57 commits, all tests green.

## M2 — Bytecode + VM (complete, 2026-09-05)

Replaces the M1 tree-walker with a register-based bytecode compiler and
VM, keeping the M1 tree-walker permanently as `--engine=tree-walk` — a
byte-for-byte (stdout, stderr, exit status) differential-testing oracle,
selectable alongside the new default, `--engine=bytecode`. Every method
body, block body, and instance initializer compiles to bytecode and runs
through the shared dispatch orchestration (`interpreter/`'s send/DNU/
lexical/super/native/instantiate logic, unchanged, consulted at three
body-execution hooks) — no Newspeak-level code is tree-walked under the
bytecode engine. Adds baseline-compiler control-flow inlining
(`ifTrue:ifFalse:`, `whileTrue:`, `whileFalse:`, `and:`, `or:`, etc.),
each guarded by a runtime receiver-class check with a correctness-
preserving fallback to a real virtual send, per the project's
performance-priority decision. Adds `ns-next disasm` for human-readable
bytecode listings, and a versioned image serialization format whose
code, object, and frame tables round-trip, exposed via `ns-next
snapshot`/`load`. Required by
`2026-09-04-1153-ns-next-liveness-images-vm-design.md`: a statement→PC
table per method (the safepoint abstract-state map) and stable call-site
identity with a reserved, inert inline-cache slot per send — both
carried by every `CompiledCode` now so Milestone 4 (inline caches) needs
no bytecode format break. Object model stays `Rc<RefCell<Object>>` for
M2 — the arena/handle rewrite is deferred to M4, alongside the real GC
(the image format is defined against the logical value model so that
later swap won't break image compatibility). Design spec committed at
`92f511b`, revised 2026-09-05; implemented on `feature/m2-bytecode-vm`
and merged at `ff151fe` (`cargo test --workspace` green, corpus count
unchanged at 210/0/1/0). A whole-branch review before the merge found
two engine divergences the per-task reviews had missed — a local's
register aliased as an expression's value, and a bare `super` panicking
the compiler where the tree-walker raises a Newspeak error — both fixed
on the branch. Two follow-ups landed on `master` afterwards: the design
spec's "Error Handling" section now treats an image as untrusted input
(`68598b6`, matching what shipped), and an assignment's value is copied
only where it outlives its setter send (`58e5688`).

## Milestone renumbering (2026-09-06)

The milestones after M2 were renumbered when language completeness was
pulled ahead of the performance work. M3 is now nested classes, mixins
and source-free images; inline caches and the GC became M4; the JIT
tiers became M5–M6. References in the design specs and in code comments
were moved with it. The completed plans under `superpowers/plans/` are
execution records of finished work and were left as written.
