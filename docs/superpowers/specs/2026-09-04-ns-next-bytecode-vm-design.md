# ns-next — Bytecode Compiler and VM, Milestone 2 Design

Status: approved for planning
Date: 2026-09-04
Author: Benjamin

## Summary

Milestone 2 replaces Milestone 1's AST tree-walking interpreter as the
primary execution path with a bytecode compiler and a register-based
bytecode VM, and adds a versioned, architecture-neutral image
serialization format with save/load CLI support. It implements the M2
row of `2026-09-04-ns-next-liveness-images-vm-design.md`'s "What to
build in, and when" table: the safepoint abstract-state map, a
statement→PC table per method, architecture-neutral versioned bytecode,
a defined and versioned image format bootstrapped from source, and
stable call-site identity.

**Read `2026-09-01-ns-next-runtime-design.md`, `2026-09-04-ns-next-liveness-images-vm-design.md`,
and this document in full before implementing.** This milestone builds
directly on the Milestone 1 resolver and object model — `ResolvedModule`,
`Symbol`, `Object`/`ObjectData`/`ClassData`, `Value`, and `dispatch.rs`'s
three lookup algorithms are reused unchanged; only the *evaluator* changes.

**Object model stays `Rc<RefCell<Object>>` for Milestone 2** (settled
2026-09-04, see the project's Nova-architecture-roadmap decision): the
image format is defined against the *logical* value model
(`Nil | Bool | SmallInt | Float | Obj`, slot layouts, classes), not
against this physical representation, specifically so that a later
arena/handle rewrite (paired with a real GC, Milestone 3) does not break
image compatibility.

## Goals

- Compile every method and block body reachable from a `ResolvedModule`
  into register-based bytecode.
- Execute that bytecode with a VM that preserves every M1 dispatch and
  activation semantic exactly (virtual sends, `private`/lexical/`super`
  dispatch rules, non-local return, closures) — this milestone changes
  *how* execution happens, not *what* Newspeak programs observe.
- Inline the standard Smalltalk control-flow selectors
  (`ifTrue:`/`ifTrue:ifFalse:`/`ifFalse:ifTrue:`/`whileTrue:`/`whileFalse:`/`and:`/`or:`)
  at compile time, guarded by a runtime receiver-class check that falls
  back to a real virtual send — performance is a first-order concern for
  this project (project decision, 2026-09-04), and control-flow inlining
  is baseline-compiler work in every mainstream Smalltalk implementation,
  not a later JIT-tier optimization.
- Record a statement→PC table and an abstract-state map at every
  statement-boundary safepoint, sufficient to reconstruct a full `Frame`.
- Give every send instruction a stable call-site identity with a
  reserved (currently unused) inline-cache slot, so Milestone 3 can
  attach cache state without a bytecode format change.
- Define and version a canonical, architecture-neutral image
  serialization format; implement save (`ns-next snapshot`) and load
  (`ns-next load`).
- Keep the Milestone 1 tree-walker as a permanently maintained reference
  execution path (project decision, 2026-09-04), selectable via
  `--engine`, used as a differential-testing oracle for the bytecode VM.
- Add `ns-next disasm <file> <selector>` per the liveness spec's VM
  developer-tooling table.

## Non-Goals (Milestone 2)

- Inline caches actually caching anything (the slot is reserved and
  wired through the VM's dispatch call, but Milestone 2 always misses it
  and does a full `dispatch.rs` lookup — Milestone 3 fills it in).
- Any JIT tier, deoptimization, or unboxed/tagged-pointer values.
- A real garbage collector, `become:`, or the arena/handle object-model
  rewrite (Milestone 3, paired together — see Summary).
- Actors / concurrency.
- A debugger, mirrors, or `ns-next repl`. Consequently: **image snapshot
  in Milestone 2 captures a quiesced world** — the state after a
  top-level send has returned (classes, objects, global/persistent
  state) — not a world paused mid-execution with a live, pending send
  stack. The general Frame-chain serialization mechanism is still
  implemented and round-trip tested (it is required by the format and by
  later milestones), exercised in tests via a synthetic paused state, but
  no Milestone 2 CLI flow triggers a *mid-execution* snapshot — there is
  no breakpoint/debugger mechanism yet to pause execution at an arbitrary
  point. Revisit this scope note when the debugger (a later milestone)
  needs it.
- Register allocation beyond a simple scheme (see "Bytecode" below) —
  no liveness-based reuse or coalescing. Register *reuse within a
  statement* is in scope (see below); cross-statement liveness analysis
  is not.
- Format migration on version mismatch (refuse only; migration tooling
  is future work if it's ever needed).

## Architecture

### Crate layout

New modules inside the existing `ns-runtime` crate, mirroring its
Milestone 1 structure — no new crate (matches the project's existing
"one embedding, don't split prematurely" stance from the liveness spec's
tooling section):

```
crates/ns-runtime/src/
  resolver/            # unchanged (M1)
  dispatch.rs           # unchanged (M1) — reused by the VM's Send handling
  interpreter/           # unchanged (M1) — kept as the --engine=tree-walk path
  compiler/              # NEW — mirrors resolver/
    mod.rs                # compile_module / compile_method entry points
    lower.rs               # Stmt/Expr -> Instr lowering, register assignment
    control_flow.rs        # special-selector recognition + inlined Branch/Jump emission
  bytecode.rs            # NEW — Instr enum, CompiledMethod, CallSite
  vm.rs                  # NEW — mirrors interpreter/ — the register VM's run loop
  image/                 # NEW — mirrors resolver/ in spirit (split by concern)
    mod.rs                 # snapshot()/load() entry points, format version, header
    encode.rs              # canonical value/object/class/method encoding
    decode.rs              # canonical decoding + version check
  ns-cli/src/main.rs      # add `--engine`, `disasm`, `snapshot`, `load`
```

### `bytecode.rs`

`Instr` is a plain Rust enum (like `ns_syntax::ast`'s node types) —
dispatched by the VM via `match`, not hand-packed bytes. "Architecture-
neutral, versioned" is a property of the **image encoding** of a
`CompiledMethod` (owned by `image::encode`/`decode`), not a requirement
that the in-memory runtime representation itself be a byte array. Manual
byte-level opcode packing at the interpreter-dispatch level is a
technique that pays off once a JIT or a very hot interpreter loop
demands it (later milestone territory); building it now would add real
complexity for no measurable Milestone 2 benefit and would fight the
project's performance-priority principle by spending effort where it
doesn't yet move the needle. If a future milestone's profiling shows
`match`-dispatch is the bottleneck, revisit then.

Register model: each `Frame` carries a `Vec<Value>` register file —
literally `Frame.temps`, extended, not a separate concept — indices
`0..n` are the resolver's `Local`/param slots (unchanged from M1's
`Binding::Local` numbering, so the compiler needs no separate local-slot
renumbering), indices `n..` are compiler-allocated scratch registers for
intermediate expression values. Allocation scheme: a simple bump
allocator reset to `n` at the start of every statement (Newspeak
statements are the unit both `pc` and safepoints already work at, so
scratch registers never need to stay live across a statement boundary) —
registers *within* one statement's expression tree are freed and reused
as each subexpression's consumer is emitted, avoiding register-count
blowup on deeply nested expressions, but no cross-statement liveness
analysis (Non-Goals).

Opcodes (one variant per `Instr`), matching the M1 resolver's `Binding`
variants and `dispatch.rs`'s lookup shapes directly:

- `LoadLiteral { dst, literal }`, `LoadNil/True/False/Self { dst }`
- `Move { dst, src }`
- `LoadLocal`/`StoreLocal { reg, slot }` (`Binding::Local`)
- `LoadOuterLocal`/`StoreOuterLocal { reg, depth, index }` (`Binding::OuterLocal`) —
  reads/writes through the `home`/enclosing `Frame` chain, same walk M1's
  interpreter already does.
- `SendLexical { depth, selector, receiver_reg, arg_regs, dst, call_site }`
  (`Binding::LexicalSend`) — resolves the kth-enclosing-object exactly as
  `dispatch.rs` does today, then applies the private/`lookupProtected`
  rule.
- `SendPublic { selector, receiver_reg, arg_regs, dst, call_site }` — an
  ordinary (explicit-receiver) send, `lookupPublic`.
- `SendSuper { selector, receiver_reg, arg_regs, dst, call_site }`
  (`Binding::SuperSend`).
- `MakeClosure { dst, compiled_block, captured_frame }`
- `Return { reg }` / `NonLocalReturn { reg, home }` — see "Non-local
  return" below; semantics unchanged from M1.
- `MakeArray { dst, elem_regs }`
- `BranchIfFalse { cond_reg, target }`, `Jump { target }` — emitted only
  by control-flow inlining (below); nothing else in the compiler emits a
  branch.

`CompiledMethod { instrs: Vec<Instr>, num_registers: u16, statement_pc: Vec<(StmtIndex, PcRange)>, call_sites: Vec<CallSite>, source: Span }`.

### Control-flow inlining (`compiler/control_flow.rs`)

Triggers only at **compile time**, on the syntactic shape of the send —
never as a general runtime heuristic: selector is one of the fixed
special selectors above **and** every block-position argument is a
syntactic block literal (`Expr::Block`) directly at the call site (not a
variable, not the result of another send). If either condition fails,
the compiler emits an ordinary `SendPublic`/`SendLexical` — no attempt to
inline. This is a pure compile-time gate; the runtime guard below is a
*second*, independent check that only exists for call sites that already
passed this gate.

When the gate passes, the compiler emits: a receiver-class runtime guard
(`is the receiver's class exactly `True`/`False`?`) followed by
`BranchIfFalse`/`Jump` around the two blocks' bodies lowered inline
(their statements compiled directly into the enclosing method's
instruction stream — no `MakeClosure`, no new `Frame`), and — critically —
an **else branch that falls back to a real, ordinary virtual send** with
the original selector and the (now-materialized, since the guard failed)
block arguments, when the guard fails. This preserves "every send is
virtual" exactly: a program that overrides `ifTrue:ifFalse:` on a
non-`Boolean` receiver, or sends it to something that merely responds to
`ifTrue:ifFalse:` without being `True`/`False`, observes identical
behavior to an uninlined send — just slower, on the now-rare path.
`whileTrue:`/`whileFalse:` inline as an actual back-edge `Jump` (a real
loop in the bytecode, not repeated closure invocation) under the same
gate and guard, re-checked every iteration (the guard is cheap — a class
pointer comparison — so re-checking it each iteration is not a
Milestone-2-relevant cost).

Testing must cover the fallback path explicitly, not just the common
case: e.g. `3 ifTrue: [...] ifFalse: [...]` (non-Boolean receiver) must
still produce the identical `doesNotUnderstand:` a fully-virtual send
would.

### Call-site identity and inline-cache slots

`CallSite { call_site_id: u32 }` — one entry per `Send*` instruction,
index-addressed (the instruction's own position serves as its stable
identity within a `CompiledMethod`). `vm.rs`'s dispatch path threads a
`&mut CallSite` through every send, but Milestone 2 never reads or
writes anything into it beyond existence — it is inert. This exists
purely so Milestone 3 can start writing IC state into an already-present
slot instead of changing `Instr`'s shape (which would be an image
format break).

### `vm.rs`

A `match`-dispatch loop over `Instr`, one `CompiledMethod` + `pc` +
register file per `Frame` — the same `Frame` type from `types.rs`
(`receiver`, `args`, `temps` as the register file, `pc`, `sender`,
`home`, `current_class`, `uncontinuable`), with `kind: FrameKind` gaining
no new variant (a bytecode-method activation is still `FrameKind::Method`/
`FrameKind::Closure`/`FrameKind::TopLevel` — only *how* it's driven
changes). `pc` in a bytecode `Frame` is now a bytecode instruction index;
the statement-index meaning M1 gave `pc` is recovered on demand via
`CompiledMethod.statement_pc` wherever something (image snapshot, a
future debugger) needs the statement-level view.

Nested sends still recurse through Rust's call stack (`Vm::run_frame`
calls itself for a nested activation), exactly as M1's tree-walker did —
Milestone 2 does not need an explicit Rust-level trampoline; it only
needs the `Frame` chain (already required, already present) to be
walkable independently of that Rust recursion, which it already is.

### Non-local return

Unchanged from M1: a closure's `home: FrameRef` is the enclosing method
activation; `NonLocalReturn` walks to `home`, checks `uncontinuable`
(raising a `cannotReturn` `NewspeakError` if set), and marks every frame
on the continuation chain `uncontinuable` on the way out — identical
logic to M1's interpreter, now invoked from the `NonLocalReturn` opcode
handler instead of from AST evaluation of a `^` statement inside a
block.

### Safepoints and the statement→PC table

A safepoint exists at the start of every statement's compiled
instruction range (`CompiledMethod.statement_pc` records exactly this
boundary). The abstract-state map at a Milestone-2 safepoint is the
`Frame` itself — `receiver`, `args`, `temps` (register file), `pc`, plus
the chain via `sender`/`home` — nothing more is needed yet, because
Milestone 2 has no register reuse across statements and no unboxing to
reconstruct around. This is intentionally the same shape M1 already
reified; Milestone 2 formalizes it (via the PC table) rather than
inventing a new one. Later milestones' deopt metadata (Milestone 4-5)
and debugger frame-editing (a later milestone) build on this same table
without changing it.

### Bytecode compilation cannot raise a `NewspeakError`

Compiling an already-`resolve()`d module is a total, deterministic
lowering — every name has already been resolved, every class member set
already validated by the M1 resolver. A compiler failure (e.g. an
unreachable `Binding` variant, a register-allocation invariant violated)
is an `ns-runtime` bug, not a malformed Newspeak program: it uses
`panic!`/`debug_assert!`, same rule as M1's interpreter invariants —
never a `NewspeakError`.

## Image format (`image/`)

**Not a memory dump** — an explicit, self-describing, versioned
encoding decoded on load, per the liveness spec:

- **Header**: magic bytes, `format_version: u32` (starts at `1`).
  **Refuse to load on any version mismatch** in Milestone 2 — no
  migration path yet (Non-Goals).
- **Canonical encodings**: little-endian for all integers, IEEE-754
  `binary64` for `Float`, UTF-8 for every string and `Symbol` (`Symbol`s
  re-interned on load, not assumed to keep their M1 `u32` id), `SmallInt`
  fixed at the `i64` range (matches the M1/M2 `Value` model — no bignums
  yet).
- **Root/special-objects table** at a fixed position: `nil`, `true`,
  `false`, every builtin class (the `BuiltinClass` set from `types.rs`),
  and the class table — preserves their identity across a save/load
  round-trip, per the liveness spec's Image Format section.
- **Identity hashes** are preserved verbatim (already monotonic,
  already stable per M1's design — nothing new needed here beyond
  serializing the field).
- Every `ClassData`/`MethodDecl`'s source `Span` + the owning module's
  `Rc<str>` source and `SourceOrigin` are serialized — the M1 runtime
  spec's "Source Retention and Provenance" section already requires
  carrying this; Milestone 2 is the first milestone that actually writes
  it out.
- **Bootstrap from source, not from a shipped base image**: `ns-next run`
  and `ns-next snapshot` both start by bootstrapping the core library
  and `Object` hierarchy from Rust primitives exactly as M1's
  `install_core_library`/bootstrap step already does — there is no
  pre-built "base image" file Milestone 2 ships or depends on. `ns-next
  load` is the only command that skips bootstrap, because the loaded
  image already contains a fully-formed object graph (including the
  bootstrapped classes) from a prior `snapshot`.

### `ns-cli` additions

- `ns-next run <file.ns> [--send selector] [--engine=bytecode|tree-walk]`
  (`--engine` defaults to `bytecode`; `tree-walk` selects the M1
  interpreter unchanged, for differential testing and as a fallback).
- `ns-next snapshot <file.ns> [--send selector] <out.img>` — bootstrap,
  load, and (if `--send` given) run `<file.ns>` exactly as `run` would,
  then serialize the resulting quiesced world to `<out.img>`.
- `ns-next load <image> [--send selector]` — deserialize `<image>`,
  optionally send one further message to its top-level object, print the
  result the same way `run` does.
- `ns-next disasm <file.ns> <selector>` — compile only, print the
  `CompiledMethod`'s instructions in a human-readable form (mnemonic +
  operands + the statement-index each range belongs to). Close to free:
  it's a formatter over `CompiledMethod`, no new compilation path.

## Data Flow

```
ResolvedModule (M1, unchanged)
  -> compiler::compile_module -> HashMap<Symbol, CompiledMethod> per class
  -> vm::Vm::run(entry_class, selector, args)
       -> Frame chain, register-based execution, dispatch.rs reused for
          every non-inlined send
       -> Value | NewspeakError
  -> image::snapshot(&Vm's live object graph) -> Vec<u8>  (canonical, versioned)
  -> image::load(bytes) -> a Vm ready to receive further sends
```

## Error Handling

Unchanged principles from M1: `NewspeakError` for Newspeak-program-level
failures (uncaught exception, `doesNotUnderstand:`, primitive failure),
propagated as a Rust `Result` through `Vm::run_frame`'s recursion exactly
as M1 propagated it through the tree-walker's recursion. `panic!`/
`debug_assert!` only for `ns-runtime`-internal invariant violations
(compiler bugs, VM dispatch-loop bugs, a corrupt-but-version-matching
image — the last is a defensive `debug_assert!` in `decode.rs`, since a
version-matching image is assumed well-formed, not a hostile input
format in Milestone 2's scope).

## Testing

- **Differential testing**: every `tests-fixtures/*.ns` runs under both
  `--engine=bytecode` and `--engine=tree-walk`; results (stdout, return
  value, uncaught-error shape) must match. The retained M1 tree-walker
  is the oracle here — this is the concrete payoff of the "keep it as a
  reference path" decision.
- **Bytecode snapshot tests** (`insta`), one per representative method
  shape (plain unary/keyword method, block with non-local return,
  inlined `ifTrue:ifFalse:`, inlined `whileTrue:`, the non-inlined
  fallback case, `super` send, nested closures) — catches accidental
  codegen changes.
- **Control-flow-inlining fallback tests**: non-Boolean receiver to an
  inlined selector still produces the same `doesNotUnderstand:` (or
  correct dispatch, if some other class legitimately implements the
  selector) as the uninlined path — verified by running the same
  fixture under both engines (differential testing already covers this
  as long as at least one fixture exercises it; add one if none does).
- **Image round-trip tests**: `snapshot` then `load` a variety of
  object graphs (empty/simple/deeply nested instances, arrays, closures
  captured in a synthetic paused state for the Frame-chain path) and
  assert identical observable state — including identity-hash
  preservation and root-table correctness.
- **`disasm` tests**: snapshot-test the human-readable output for a
  representative method set.

## Decisions

- Register-based bytecode, `Frame.temps` doubles as the register file —
  no new activation-record concept, extends M1's directly.
- `Instr` is a `match`-dispatched Rust enum, not a hand-packed byte
  array; "architecture-neutral, versioned" is satisfied by the image
  encoding layer, not the in-memory representation.
- Control-flow special selectors are inlined in the **baseline**
  compiler (not deferred to a later JIT tier), guarded by a runtime
  receiver-class check with a correctness-preserving fallback to a real
  virtual send — see Goals for the performance-priority rationale.
- Call-site identity and an inert IC slot are established now so
  Milestone 3 needs no bytecode format break.
- Tree-walking interpreter (M1) is retained permanently as
  `--engine=tree-walk`, used as the differential-testing oracle.
- Image format defined against the logical value model; object
  representation (`Rc<RefCell<Object>>`) can still change later
  (Milestone 3) without an image-compatibility break.
- No crate split; `compiler/`, `bytecode.rs`, `vm.rs`, `image/` live in
  `ns-runtime`, mirroring its Milestone 1 module structure.

## Open Questions Carried Forward

From the liveness spec's own "Open questions" section, still open after
this design (not blocking Milestone 2, noted here so they aren't lost):

- Image granularity for a future actor model (per-actor snapshots
  composed into a whole-world image) — moot until actors exist; this
  milestone's single-world image format should not need restructuring
  when that lands, since a single actor's world is a degenerate case of
  "the world," but this is not verified here.
- Identity-hash width vs. a future tagged-pointer representation —
  unaffected by this milestone (hashes are serialized verbatim as
  whatever width M1 already uses).
