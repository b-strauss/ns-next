# ns-next Progress

Concise, milestone-level progress tracker for the `ns-next` project (a
native Rust runtime for Newspeak). Updated once after each milestone
completes — this is a status summary, not a task list; see
`docs/superpowers/plans/` for the detailed, checkbox-level implementation
plans and `docs/superpowers/specs/` for the design specs behind each
milestone.

| Milestone | Status | Completed | Spec | Plan(s) |
|---|---|---|---|---|
| M1 — Frontend | Complete | 2026-09-01 | [runtime-design](superpowers/specs/2026-09-01-ns-next-runtime-design.md) | [frontend](superpowers/plans/2026-09-01-ns-next-frontend.md) |
| M1 — Runtime | Complete | 2026-09-04 | [runtime-design](superpowers/specs/2026-09-01-ns-next-runtime-design.md), [liveness/VM constraints](superpowers/specs/2026-09-04-ns-next-liveness-images-vm-design.md) | [runtime](superpowers/plans/2026-09-04-ns-next-runtime.md) |
| M2 — Bytecode + VM | Plan written, implementation pending | — | [bytecode-vm-design](superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md) | [bytecode-vm](superpowers/plans/2026-09-04-ns-next-bytecode-vm.md) |
| M3 — Inline caches + GC | Not started | — | — | — |
| M4–M5 — JIT | Not started | — | — | — |

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

## M2 — Bytecode + VM (plan written, implementation pending)

Replaces the M1 tree-walker with a register-based bytecode compiler and
VM, keeping the M1 tree-walker as a permanent `--engine=tree-walk`
differential-testing oracle. Required by
`2026-09-04-ns-next-liveness-images-vm-design.md`: a safepoint
abstract-state map, a statement→PC table per method, architecture-neutral
versioned bytecode, a defined and versioned image serialization format
(bootstrap from source, `ns-next snapshot`/`load`), and stable call-site
identity (with a reserved, inert inline-cache slot per send for M3).
Adds baseline-compiler control-flow inlining (`ifTrue:ifFalse:`,
`whileTrue:`, etc., guarded, with a correctness-preserving fallback to a
real virtual send) per the project's performance-priority decision.
Object model stays `Rc<RefCell<Object>>` for M2 — the arena/handle
rewrite is deferred to M3, alongside the real GC (image format is
defined against the logical value model so that later swap won't break
image compatibility). Design spec committed at `92f511b`; the 15-task
checkbox-level implementation plan (`superpowers/plans/2026-09-04-ns-next-bytecode-vm.md`)
is written and committed. Implementation via `subagent-driven-development`
or `executing-plans` is the next step.
