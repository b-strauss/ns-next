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
| M2 — Bytecode + VM | In design | — | TBD | TBD |
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

## M2 — Bytecode + VM (in design)

Replaces the M1 tree-walker with a bytecode compiler and interpreter.
Required by `2026-09-04-ns-next-liveness-images-vm-design.md`: a safepoint
abstract-state map, a statement→PC table per method, architecture-neutral
versioned bytecode, a defined and versioned image serialization format
(bootstrap from source), and stable call-site identity. Object model
stays `Rc<RefCell<Object>>` for M2 — the arena/handle rewrite is a later
milestone. Design in progress; this row and the table above get filled in
once the spec and plan are written.
