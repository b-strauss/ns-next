# ns-next — Bytecode Compiler and VM, Milestone 2 Design

Status: approved for planning (revised 2026-09-05 — see "Open Questions
Resolved During Planning" at the end for the completeness corrections made
while writing the implementation plan)
Date: 2026-09-04
Author: Benjamin

## Summary

Milestone 2 replaces Milestone 1's AST tree-walking interpreter as the
primary execution path with a bytecode compiler and a register-based
bytecode VM, and adds a versioned, architecture-neutral image
serialization format with save/load CLI support. It implements the M2
row of `2026-09-04-1153-ns-next-liveness-images-vm-design.md`'s "What to
build in, and when" table: the safepoint abstract-state map, a
statement→PC table per method, architecture-neutral versioned bytecode,
a defined and versioned image format bootstrapped from source, and
stable call-site identity.

**Read `2026-09-01-2252-ns-next-runtime-design.md`, `2026-09-04-1153-ns-next-liveness-images-vm-design.md`,
and this document in full before implementing.** This milestone builds
directly on the Milestone 1 resolver and object model — `ResolvedModule`,
`Symbol`, `Object`/`ObjectData`/`ClassData`, `Value`, and `dispatch.rs`'s
three lookup algorithms are reused unchanged; only the *evaluator* changes.
"Evaluator" is meant precisely: `interpreter/`'s dispatch *orchestration*
(`Interpreter::send`, `doesNotUnderstand:`, the lexical/super send
functions, `try_factory_send`/`instantiate`) is shared by both engines and
gains one small engine hook at each of its three "run this body" points
(method body, block body, instance initializer); only the AST-walking
*evaluation* of those bodies is what the bytecode VM replaces. See
"`vm.rs` and the engine hook" below and the resolution notes at the end.

**Object model stays `Rc<RefCell<Object>>` for Milestone 2** (settled
2026-09-04, see the project's Nova-architecture-roadmap decision): the
image format is defined against the *logical* value model
(`Nil | Bool | SmallInt | Float | Obj`, slot layouts, classes), not
against this physical representation, specifically so that a later
arena/handle rewrite (paired with a real GC, Milestone 4) does not break
image compatibility.

## Goals

- Compile every method body, every block body, **and every class's
  instance initializer** (its slot initializers and `initExprs` — see
  "Class instantiation" below) reachable from a `ResolvedModule` into
  register-based bytecode. Under `--engine=bytecode`, no Newspeak-level
  code is evaluated by the tree-walker.
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
  reserved (currently unused) inline-cache slot, so Milestone 4 can
  attach cache state without a bytecode format change.
- Define and version a canonical, architecture-neutral image
  serialization format; implement save (`ns-next snapshot`) and load
  (`ns-next load`).
- Keep the Milestone 1 tree-walker as a permanently maintained reference
  execution path (project decision, 2026-09-04), selectable via
  `--engine`, used as a differential-testing oracle for the bytecode VM.
- Add `ns-next disasm <file> <selector>` per the liveness spec's VM
  developer-tooling table.
- Complete the Milestone 1 core library's control-flow surface so every
  inlined selector has a genuine virtual implementation to fall back to in
  *both* engines: native `Block>>whileTrue:`/`whileFalse:` and
  `Boolean>>ifFalse:ifTrue:` (see "Open Questions Resolved During
  Planning" — without these, the tree-walker could not serve as the
  differential oracle for loops).

## Non-Goals (Milestone 2)

- Inline caches actually caching anything (the slot is reserved and
  wired through the VM's dispatch call, but Milestone 2 always misses it
  and does a full `dispatch.rs` lookup — Milestone 4 fills it in).
- Any JIT tier, deoptimization, or unboxed/tagged-pointer values.
- A real garbage collector, `become:`, or the arena/handle object-model
  rewrite (Milestone 4, paired together — see Summary).
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
  statement* is permitted but not required (see below); cross-statement
  liveness analysis is not in scope.
- A `--bench` mode / the tree-walker-vs-bytecode microbenchmark the
  liveness spec's tooling section mentions without assigning it a
  milestone. Deliberately deferred: the differential harness already
  proves both engines agree, and a benchmark harness is better built once
  Milestone 4's inline caches give it something to measure against.
- The Milestone 1 corpus-parse deferral (`expr -1` lexed as a negative
  literal, one blacklisted file) that the M1 spec said to "revisit in
  Milestone 2". Deferred again, explicitly: this milestone does not touch
  `ns-syntax` at all (Global Constraint in the plan), and the fix is a
  lexer-rule change independent of anything the VM needs. Revisit when
  the frontend is next opened.
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
  interpreter/           # SHARED dispatch orchestration (M1) + the engine hook;
                         #   its AST-walking body evaluation is the --engine=tree-walk path
  builtins/              # unchanged (M1) except three new control-flow natives (see Goals)
  compiler/              # NEW — mirrors resolver/
    mod.rs                # compile_module / compile_method / compile_initializer entry points
    lower.rs               # Stmt/Expr -> Instr lowering, register assignment
    control_flow.rs        # special-selector recognition + inlined Branch/Jump emission
  bytecode.rs            # NEW — Instr enum, CompiledCode, CompiledModule, CallSite
  vm.rs                  # NEW — the register VM's run loop only (no dispatch logic of its own)
  image/                 # NEW — mirrors resolver/ in spirit (split by concern)
    mod.rs                 # snapshot()/load() entry points, format version, header
    encode.rs              # canonical value/object/class/code/frame encoding
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
literally `Frame.args ++ Frame.temps`, extended, not a separate concept.
A register number is one `u16` index into that unified space: index
`< num_args` is `Frame.args[index]`, index `>= num_args` is
`Frame.temps[index - num_args]` — exactly the args-then-temps split M1's
`read_local`/`write_local` already implement for `Binding::Local`, so the
compiler needs no separate local-slot renumbering. Indices
`0..num_args+named_temps` are the resolver's `Local`/param slots;
indices above that are compiler-allocated scratch registers for
intermediate expression values. Allocation scheme: a simple bump
allocator reset to the first scratch index at the start of every
statement (Newspeak statements are the unit both `pc` and safepoints
already work at, so scratch registers never need to stay live across a
statement boundary). Reusing registers *within* one statement's
expression tree as each subexpression's consumer is emitted is
permitted but not required in Milestone 2 (revised 2026-09-05: the
baseline compiler uses the per-statement bump allocator only — Newspeak
method bodies are not deeply nested in practice and `Frame.temps` is a
plain growable `Vec`, so a few extra live entries per statement cost
nothing measurable; revisit only if profiling ever shows register-file
size matters). No cross-statement liveness analysis (Non-Goals).
**An inlined block's statements are not statements of the enclosing
code for allocation purposes** (added 2026-09-05): they are lowered as
sub-expressions of the enclosing statement, so the allocator is reset
only to the watermark at the inlined block's start between them — never
to the statement base, which would clobber the enclosing statement's
still-live registers (the guard register, the branch's result register,
an already-evaluated sibling argument). See "Control-flow inlining".

Opcodes (one variant per `Instr`), matching the M1 resolver's `Binding`
variants and `dispatch.rs`'s lookup shapes directly. Every register
operand is a `u16`; every jump target is an instruction index:

- `LoadLiteral { dst, literal }` — `literal` is a lowered
  `bytecode::Literal` (`Nil | Bool | SmallInt | Float | Str | Sym | Char
  | Array | MalformedInt`), int/float text already parsed at compile
  time. `MalformedInt(text)` (added 2026-09-05) is the one deferred case:
  an integer literal the lexer accepted but M1's `parse_int_literal`
  rejects (out of `i64` range, a fractional radix literal) is a
  *Newspeak-program-level* error that M1 raises as a `NewspeakError` when
  the literal is evaluated — never at load time. Since compilation cannot
  raise a `NewspeakError` (below), the compiler lowers such a literal to
  `MalformedInt` and the VM re-runs the shared `parse_int_literal` when
  the instruction executes, producing the identical error text the
  tree-walker does (and, like the tree-walker, only if execution actually
  reaches it).
- `LoadSelf { dst }`, `LoadBuiltinClass { dst, kind: BuiltinClass }` —
  the latter serves M1's "bare builtin class name has no `Binding`"
  convention (`eval_ident`'s fallback) at compile time.
- `Move { dst, src }`
- `StoreLocal { src, slot }` (`Binding::Local` write; a read needs no
  instruction — the local's register *is* its value)
- `LoadOuterLocal { dst, depth, index }`/`StoreOuterLocal { src, depth, index }`
  (`Binding::OuterLocal`) — through the `captured_frame` chain, same walk
  M1's `frame_at_depth` already does.
- `SendLexical { depth, selector, arg_regs, dst, call_site }`
  (`Binding::LexicalSend`) — resolves the kth-enclosing-object exactly as
  `dispatch.rs` does today, then applies the private/`lookupProtected`
  rule. No receiver register: the receiver is `self` by construction.
- `SendPublic { selector, receiver_reg, arg_regs, dst, call_site }` — an
  ordinary (explicit-receiver) send, `lookupPublic`.
- `SendSuper { selector, arg_regs, dst, call_site }`
  (`Binding::SuperSend`) — receiver is `self`, lookup starts at the
  current activation's `current_class`'s superclass.
- `MakeClosure { dst, params: Rc<Vec<Symbol>>, code: Rc<CompiledCode> }`
  — the block body's *compiled* code (nested `CompiledCode` for a nested
  block literal); `captured_frame`/`home` are runtime values supplied by
  the VM from its current frame when the instruction executes, never
  instruction operands. See "Blocks and closures" below.
- `MakeArray { dst, elem_regs }`
- `InitSlot { src, field_index }` — writes `self`'s instance field
  `field_index`; emitted **only** by instance-initializer compilation for
  a slot's own initializer expression (see "Class instantiation" below).
  This is the one layout-dependent instruction in the set: `field_index`
  is the absolute index `load_class` assigned to the slot
  (`ClassData.slot_index`). Milestone 4's class-shape-change flush must
  therefore invalidate compiled initializers along with every other
  cache keyed to the class — recorded here so that milestone does not
  have to rediscover it. Slot reads/writes from *method* bodies are not
  affected: they are `SendLexical`s resolved through `lookup_slot` at
  run time, exactly as in M1.
- `Return { reg }` — a method or initializer body's `^`/end.
  `ImplicitReturn { reg }` — a body that fell off its end (a method
  returns `self`, a block its last expression's value); distinct from
  `Return` only so that `disasm` output and the M1 `uncontinuable`
  marking discipline stay legible — the VM treats both as "hand `reg`
  back to the caller".
  `NonLocalReturn { reg }` — a `^` inside a block body; unwinds to the
  activation's `home` via M1's `raise_return` (see "Non-local return").
- `Jump { target }`, `JumpIfNotBoolean { reg, target }`,
  `BranchIfFalse { reg, target }`, `BranchIfTrue { reg, target }` —
  emitted only by control-flow inlining (below); nothing else in the
  compiler emits a branch. `JumpIfNotBoolean` is the inlining guard
  ("is the receiver's class exactly `True`/`False`?" — for M1's unboxed
  `Value::Bool` that is precisely "is this register a `Bool` at all").

```
CompiledCode {
    kind: CodeKind,                 // Method | Block | Initializer
    instrs: Vec<Instr>,
    num_args: u16,
    num_registers: u16,             // args + named temps + scratch; Frame.temps is sized to num_registers - num_args
    statement_pc: Vec<(usize, Range<usize>)>,  // statement index -> instruction range (safepoints)
    call_sites: Vec<CallSite>,
    source: Span,
}
CompiledModule {
    methods: HashMap<usize, Rc<CompiledCode>>,      // keyed by Rc<ast::MethodDecl> pointer identity
    initializers: HashMap<usize, Rc<CompiledCode>>, // keyed by ClassRef pointer identity
    bindings: Rc<HashMap<Span, Binding>>,           // the module's resolver output, carried for the shared orchestration's signatures
}
```

(Revised 2026-09-05: `CompiledMethod` renamed to `CompiledCode` because a
method body, a block body, and a class's instance initializer all
compile to the same shape; `CodeKind` tags which. The method cache is
keyed by the loaded method's `Rc` pointer — the exact `Rc` every
`dispatch.rs` `Lookup::Method` result hands back — so there is no
name-based index to keep in sync and no two same-named methods on
different classes can collide.)

### Control-flow inlining (`compiler/control_flow.rs`)

Triggers only at **compile time**, on the syntactic shape of the send —
never as a general runtime heuristic: the send is an ordinary
explicit-receiver send (never a `LexicalSend`/`SuperSend` — an implicit
receiver for `ifTrue:` is meaningless), its selector is one of the fixed
special selectors above, **and** every block-position argument (plus,
for `whileTrue:`/`whileFalse:`, the receiver) is a syntactic block
literal (`Expr::Block`) directly at the call site (not a variable, not
the result of another send) that declares **no parameters and no
temporaries** (revised 2026-09-05: the no-temporaries rule is new — a
block with its own `| t |` gets a fresh `t` per invocation in M1, and a
closure created inside a loop body would observe that; a register can
only be one `t`, so such blocks are simply not inlined and keep M1's
semantics exactly; temp-vector machinery is later-milestone work). If
any condition fails, the compiler emits an ordinary `SendPublic` — no
attempt to inline. This is a pure compile-time gate; the runtime guard
below is a *second*, independent check that only exists for call sites
that already passed this gate.

**Lexical addressing inside an inlined block** (added 2026-09-05): the
resolver assigned `Binding::OuterLocal { depth, index }` by counting
*scopes* — every block literal is one scope — while the VM walks
*frames* via `captured_frame`, and an inlined block has a scope but no
frame. The compiler therefore keeps a scope stack (`Frame` for a code
body's own scope, `Inlined` for each inlined block being lowered) and
rewrites every local reference: the runtime hop count for `depth d` is
the number of `Frame` scopes among the innermost `d` entries; a hop count
of `0` is a plain register read/write of the current frame (the same
"no instruction" case as `Binding::Local`), otherwise
`LoadOuterLocal`/`StoreOuterLocal` with the reduced depth. The rewrite
applies equally to a *nested real block* compiled inside an inlined
region (its `MakeClosure` captures the enclosing *frame*, so a reference
that the resolver counted through the inlined scope is one hop shorter
at run time). `Binding::Local` inside an inlined scope cannot occur (the
gate admits no names there) and is a compiler invariant violation.
`LexicalSend`/`SuperSend` are unaffected — they resolve against the
frame's `receiver`/`current_class`, which an inlined block shares.

**`^` inside an inlined block** compiles to whatever a `^` at the top of
the *enclosing* code would: `Return` in a method body, `NonLocalReturn`
in a block body. In an instance initializer it compiles to
`NonLocalReturn` (see "Class instantiation" — M1 lets such a return
escape `instantiate`, and both engines must agree).

**Statement→PC granularity**: an inlined block's statements are part of
the enclosing statement's instruction range; `CompiledCode.statement_pc`
records only the code body's own statements (a later debugger wanting
finer stepping inside inlined blocks extends the table under a
`format_version` bump).

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
gate, re-checked every iteration (the guard is cheap — a tag check — so
re-checking it each iteration is not a Milestone-2-relevant cost). Their
guard is different in kind, though (clarified 2026-09-05): the receiver
is a block literal *by construction* (the gate), so its class is always
`Block` and there is no receiver-class ambiguity to fall back from —
what the guard checks is the *condition block's value* each iteration.
A non-`Boolean` condition value cannot fall back to "re-send
`whileTrue:` to a materialized closure": that would re-run the
condition body (its side effects twice) before the native could report
anything, an engine-observable difference. Instead the natives define
the behavior and the inlined form reproduces it: native
`Block>>whileTrue:`/`whileFalse:` send **`mustBeBoolean`** (an ordinary
virtual send, the classic Smalltalk convention) to the offending value
and return that send's result as the loop expression's value; the
inlined loop's `JumpIfNotBoolean` target does exactly the same
`SendPublic mustBeBoolean` on the guard register. No class implements
`mustBeBoolean` in the M1 core library, so by default both engines
produce the identical `doesNotUnderstand: #mustBeBoolean` error through
the shared DNU path — and a user class that does implement it observes
identical behavior under both. A `whileTrue:`/`whileFalse:` whose
condition terminates the loop normally evaluates to `nil` in both.

Testing must cover the fallback paths explicitly, not just the common
case: e.g. `3 ifTrue: [...] ifFalse: [...]` (non-Boolean receiver) must
still produce the identical `doesNotUnderstand:` a fully-virtual send
would, a user class that *implements* `ifTrue:ifFalse:` must have its
method run, and a non-Boolean `whileTrue:` condition must produce the
identical `mustBeBoolean` error.

### Call-site identity and inline-cache slots

`CallSite { call_site_id: u16 }` — one entry per `Send*` instruction,
index-addressed (`call_site_id` is the `Send*` instruction's own index
into `CompiledCode.instrs`, so it is stable and reconstructible with no
separate counter to drift). Each `Send*` instruction carries its
`CallSite` by value and `CompiledCode.call_sites` lists them in
instruction order, but Milestone 2 never reads or writes anything into
it beyond existence — it is inert. This exists purely so Milestone 4 can
start writing IC state into an already-present slot instead of changing
`Instr`'s shape (which would be an image format break).

### `vm.rs` and the engine hook

(Revised 2026-09-05. The original draft of this section had `vm.rs`
reimplement the dispatch orchestration — send, `doesNotUnderstand:`,
lexical/super dispatch, native dispatch — in parallel to `interpreter/`,
and left instance initializers on the tree-walker under every `--engine`
value. Both are corrected here; see "Open Questions Resolved During
Planning".)

**One engine field, three hook points.** `Interpreter` gains
`engine: Engine` where

```
enum Engine { TreeWalk, Bytecode(CompiledModule) }
```

`Interpreter::bootstrap()` defaults to `TreeWalk`, so every Milestone 1
test and every existing caller keeps its exact behavior;
`Interpreter::set_engine(Engine::Bytecode(compiled))` switches after
`load_module`/`compile_module` have run. Everything in `interpreter/`
that decides *what* to run stays shared and is unchanged in behavior:
`Interpreter::send` (including `try_factory_send`),
`does_not_understand`, `dispatch_native_or_dnu`/
`dispatch_native_inheriting_object` (the native/builtin path, including
the `Object`-inheritance step), `eval::dispatch_lexical_send`,
`eval::dispatch_super_send`, `raise_return`, `invoke_method`'s frame
construction and `Signal::Return` catching, `invoke_closure`'s arity
check and frame construction, and `instantiate`'s six-step algorithm.
Exactly three places decide *how* a body runs, and each consults
`engine`:

1. **Method body** — `invoke_method`, after building the `Frame`:
   `TreeWalk` sizes `temps` to `body.temps.len()` and runs
   `run_statements` (M1); `Bytecode` looks the method up in
   `CompiledModule.methods` by `Rc` pointer, sizes `temps` to
   `num_registers - num_args`, and runs `vm::run_code`.
2. **Block body** — `eval::invoke_closure`, after the arity check and
   frame construction: dispatches on `ClosureData.body` (see "Blocks and
   closures" below) — an AST body runs the M1 statement loop, a compiled
   body runs `vm::run_code`.
3. **Instance initializer** — `instantiate`'s per-level "run this
   class's own slot initializers and `initExprs` on this frame" step
   (M1's `run_own_slots` + `run_own_init_exprs`): `TreeWalk` runs them
   as before; `Bytecode` looks the class up in
   `CompiledModule.initializers` by `ClassRef` pointer, grows the shared
   initializer frame's `temps` to fit that level's `num_registers`, and
   runs `vm::run_code`. A class with no compiled initializer and no
   slots/`initExprs` (every `BuiltinClass`) is a no-op in both engines;
   a user class missing from the cache is a `compile_module` coverage
   bug (`panic!`).

`vm.rs` is therefore *only* a `match`-dispatch loop over `Instr` —
`run_code(interp, frame, code, bindings) -> EvalResult` — one
`CompiledCode` + `pc` + register file per `Frame`. Its `Send*` handlers
call straight back into the shared orchestration (`interp.send`,
`dispatch_lexical_send`, `dispatch_super_send`), which in turn re-enters
`vm::run_code` through hook 1 or 2 for any user-defined target. The VM
has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch, and no `NewspeakError` text of its own — every error a program
can observe is produced by the same shared code under both engines,
which is what makes the tree-walker a byte-for-byte oracle (including
stderr) rather than a merely-similar one. Six private `eval.rs`
functions become `pub(crate)` so `vm.rs` and `compiler/` can call them
(revised 2026-09-05 — the earlier list named only the first two):
`dispatch_lexical_send`, `dispatch_super_send`, the `home` computation
for a new closure (factored out as `home_of(frame)`), `read_local`/
`write_local`/`frame_at_depth` (the *definition* of the register file's
args-then-temps addressing — the VM must not carry a second copy of it),
and `parse_int_literal` (the compiler parses literals once with it, and
the VM's `MalformedInt` path re-runs it for the identical error text).
`instantiate::method_name` (the pure `MessagePattern` → selector helper)
likewise becomes `pub(crate)` so the image's frame table can name a
`Method` frame's method. Nothing else in `interpreter/` changes
visibility.

The `Frame` type is the M1 one from `types.rs` (`receiver`, `args`,
`temps` as the register file, `pc`, `sender`, `home`, `current_class`,
`uncontinuable`). `kind: FrameKind` gains **one** variant,
`Initializer(ClassRef)` (revised 2026-09-05 — the original text said
"no new variant"): a running instance initializer must be able to name
the code it is executing so that `pc` is interpretable
(`CompiledCode.statement_pc` is reached through the frame's `kind` — a
method's via the method cache, a closure's via its `ClosureData.body`,
an initializer's via the initializer cache), which the M1 stand-in
`FrameKind::TopLevel` could not do. Both engines build initializer
frames as `Initializer` (the frame is constructed in shared
`instantiate` code), and the two M1 match sites that group `TopLevel`
with `Method` (`raise_return`'s target and a new closure's `home`) treat
`Initializer` the same way (corrected 2026-09-05: `run_statements`'
implicit-return match is `Method` alone versus everything else, and it
is only ever reached from `invoke_method`, so it never sees an
`Initializer` frame and needs no arm). `TopLevel` itself stays in the
enum — `instantiate` no longer builds it, but the image format's frame
table still names it and a later REPL/top-level activation will use it.
`pc` in a bytecode `Frame` is a bytecode instruction index;
the statement-index meaning M1 gave `pc` is recovered on demand via
`statement_pc` wherever something (image snapshot, a future debugger)
needs the statement-level view.

Nested sends still recurse through Rust's call stack (`vm::run_code` →
`interp.send` → hook → `vm::run_code`), exactly as M1's tree-walker did —
Milestone 2 does not need an explicit Rust-level trampoline; it only
needs the `Frame` chain (already required, already present) to be
walkable independently of that Rust recursion, which it already is.

### Blocks and closures

`ClosureData.body` becomes an enum (revised 2026-09-05; M1 had
`body: Rc<ast::CodeBody>`):

```
enum ClosureBody { Ast(Rc<ast::CodeBody>), Compiled(Rc<CompiledCode>) }
```

The tree-walker's `eval_block_literal` builds `Ast`; the VM's
`MakeClosure` builds `Compiled` from the instruction's `code` operand.
`invoke_closure` (hook 2) dispatches on this enum, so a closure always
runs with the engine that created it and the invariant "under
`--engine=bytecode` no Newspeak-level code is tree-walked" is enforced by
the type, not by a runtime flag: every native primitive that invokes a
block (`ifTrue:ifFalse:`, `do:`, `and:`/`or:`, `whileTrue:`, `Block
value`/`value:`) still goes through the unchanged
`builtins::call_block` → `invoke_closure`, and simply lands on the
compiled arm. `params`, `captured_frame`, `home`, and `bindings` are
unchanged. A block body is compiled as its own `CompiledCode` (`kind:
Block`, `num_args` = block parameter count) nested inside its enclosing
code's `MakeClosure` instruction; nested block literals nest again. A
block's `Binding::OuterLocal { depth, index }` reads walk
`captured_frame` `depth` times and address `index` in the unified
register space, exactly as M1's `frame_at_depth`/`read_local` do — the
scratch registers a bytecode frame adds sit *after* the named locals, so
M1's index numbering is untouched.

Block invocation is not otherwise special-cased: `Block value` is still
a native `Block` method, not a VM opcode — the sender's `Frame` still
becomes the closure activation's `sender` (M1 finding I6), and a wrong
argument count is still the same `NewspeakError`.

### Non-local return

Unchanged from M1: a closure's `home: FrameRef` is the enclosing method
(or initializer) activation. A block body's `^` compiles to
`NonLocalReturn`, whose handler calls M1's `raise_return` on the block's
own frame — which walks to `home`, checks `uncontinuable` (raising a
`cannotReturn` `NewspeakError` if set), marks the target
`uncontinuable`, and returns `Signal::Return(home, value)`. That signal
propagates out of `vm::run_code` through every intervening Rust frame
(native primitives included — `builtins::call_block` forwards it
unchanged, as in M1) until the shared `invoke_method` that owns `home`
catches it, exactly as it catches the tree-walker's. (When `home` is an
*initializer* frame, nothing catches it — see "Class instantiation";
that is an M1 gap both engines share.) A
method body's own `^` compiles to plain `Return` and never raises a
signal at all — `invoke_method` marks the frame `uncontinuable` on every
exit path regardless, so the observable M1 discipline (a block invoked
after its home returned gets `cannotReturn`) is preserved without a
signal round-trip on the common path.

### Class instantiation

(Added 2026-09-05 — the original design was silent on the M1 spec's
"Class Instantiation" section; see "Open Questions Resolved During
Planning".)

The six-step algorithm from the M1 spec (`newspeak-spec.pdf` §6.3) is
shared orchestration in `instantiate`: factory-arity check, allocate one
instance with every slot `nil`, build the instance-initializer
activation (`FrameKind::Initializer(class)`, `args` = the factory
arguments, `current_class` = the class), run the superclass chain
deepest-first, run this class's own slot initializers in declaration
order, run its `initExprs`, return the instance. Only the per-level
"slot initializers + `initExprs`" body is engine-specific (hook 3).

`compile_initializer(class, bindings) -> CompiledCode` (`kind:
Initializer`, `num_args` = the class's own factory parameter count —
the resolver already scopes slot initializers and `initExprs` in exactly
that activation, with the factory parameters as `Binding::Local(i)`)
lowers, in order: for each of the class's own slots that has an
initializer expression, the expression followed by `InitSlot { src,
field_index }` with the slot's absolute index from `ClassData.slot_index`
(M1's `run_own_slots` writes the field directly — never through a setter
send — and the compiled form does the same); then each `initExprs`
statement, with a top-level `Stmt::Return` treated as `Stmt::Expr`
(evaluate, discard) exactly as M1's `run_own_init_exprs` does; then
`LoadSelf` + `ImplicitReturn`. A `^` *inside a block* in initializer
code is different (clarified 2026-09-05, verified against
`instantiate.rs`): M1 raises a `Signal::Return` whose target is the
initializer frame, and `instantiate` neither catches it nor ever marks
that frame `uncontinuable` — the signal escapes to the CLI as
"internal: non-local return escaped instantiation". Both engines keep
that behavior verbatim: a real block's `^` is `NonLocalReturn` as
anywhere else, an *inlined* block's `^` in initializer code is also
`NonLocalReturn` (a plain `Return` would make hook 3 swallow it), and
`raise_return` on an `Initializer` frame targets the frame itself.
The statement→PC table numbers each slot
initializer and each `initExpr` as one statement in that order — so a
safepoint exists at every slot initializer boundary and every
`initExpr` boundary, the same granularity M1's `pc` had there.
`compile_module` compiles the root class's initializer alongside its
methods and stores it in `CompiledModule.initializers`.

**Every Milestone 1 known gap in instantiation is carried forward
unchanged, in both engines**, so the tree-walker remains a valid oracle
for instantiation too: the superclass clause's deferred factory message
is still never evaluated; every level of the chain still runs in the
subclass's frame (so a superclass's initializer sees the subclass's
factory arguments — and, for the register file, `Frame.args.len()` is
the subclass's arity: only the root class is ever compiled, and
`instantiate`'s arity check guarantees its `num_args` equals
`args.len()`); a `^` in a block escapes uncatched (above); nested
classes are still not loaded; only builtin superclass names resolve.
The compiled initializer reproduces M1's
behavior exactly, gaps included — closing any of those gaps is a
separate, later change that must land in `instantiate`'s shared
orchestration (and in the resolver), not in one engine.

### Safepoints and the statement→PC table

A safepoint exists at the start of every statement's compiled
instruction range (`CompiledCode.statement_pc` records exactly this
boundary; for an instance initializer, every slot initializer and every
`initExpr` counts as one statement — see "Class instantiation"). The abstract-state map at a Milestone-2 safepoint is the
`Frame` itself — `receiver`, `args`, `temps` (register file), `pc`, plus
the chain via `sender`/`home` — nothing more is needed yet, because
Milestone 2 has no register reuse across statements and no unboxing to
reconstruct around. This is intentionally the same shape M1 already
reified; Milestone 2 formalizes it (via the PC table) rather than
inventing a new one. Later milestones' deopt metadata (Milestone 5-6)
and debugger frame-editing (a later milestone) build on this same table
without changing it.

### Bytecode compilation cannot raise a `NewspeakError`

Compiling an already-`resolve()`d module is a total, deterministic
lowering — every name has already been resolved, every class member set
already validated by the M1 resolver. Determinism is load-bearing for
the image format below: the same source under the same format version
compiles to the same instructions, so an image's code table and a
recompilation of its stored source can be cross-checked. A compiler failure (e.g. an
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
- **Root/special-objects table**, realized as follows (revised
  2026-09-05, making the mechanism explicit): `nil`, `true`, `false` are
  unboxed `Value` scalars in the M1 object model — they have no heap
  identity to preserve, and are encoded as scalar tags. Every builtin
  class and the module's root user class are referenced by a **class
  tag** (`Builtin(BuiltinClass)` or `UserRoot`) wherever a class object
  appears (an object's `.class`, a class-valued slot, a frame's
  `current_class`), never structurally serialized; on load a tag resolves
  to the live class the bootstrap / the reloaded module produced. This
  preserves exactly the identities the liveness spec's root table exists
  for — the builtin classes and the class table — with the bootstrap
  itself acting as the fixed-position table. It also follows from
  Milestone 1's documented "no metaclasses" deviation (every class
  object's own `.class` is one shared placeholder), which this milestone
  carries forward unchanged. The `platform` singleton
  (`Interpreter::platform()`, memoized per interpreter and stored in
  slots by essentially every module — `| t = platform transcript. |`)
  is the one other well-known object with identity to preserve, and
  gets its own special-object tag resolved to the loading interpreter's
  own singleton (added 2026-09-05). Transcript stream objects are
  ordinary instances (M1 mints a fresh one per `platform transcript`).
- **Identity hashes** are preserved verbatim (already monotonic,
  already stable per M1's design). After a load, the interpreter's
  allocation counter is advanced past the largest hash the image
  contained, so objects allocated after a load never collide with loaded
  ones.
- **Source and provenance**: the owning module's full source text and
  its `SourceOrigin` are serialized — the M1 runtime spec's "Source
  Retention and Provenance" section already requires carrying this;
  Milestone 2 is the first milestone that actually writes it out.
  Classes and method ASTs are *not* structurally serialized: `load`
  re-runs the ordinary parse → resolve → `load_module` pipeline on the
  stored source, which recreates every `ClassData`/`MethodDecl` (with its
  `Span`) exactly as `run` would — this *is* the "bootstrap from source"
  principle applied to user code. The liveness spec's "an image can
  always be exported back to `.ns` files" holds trivially — the stored
  text *is* the file — but no `export` command ships in Milestone 2
  (not in its milestone row).
- **Code table** (revised 2026-09-05 — the bytecode *is* part of the
  image, per this document's `bytecode.rs` section and the liveness
  spec's Image Format section; an earlier plan draft had dropped it):
  every `CompiledCode` reachable from the module — each method, the
  instance initializer, and every nested block body — is encoded in a
  flat, id-addressed table (blocks before the code that creates them, so
  `MakeClosure`'s `code` operand encodes as a `code_id` already decoded),
  plus a root list mapping each method selector and the initializer to
  its `code_id`. On load the table is decoded and installed as the
  `CompiledModule` (methods re-keyed to the reloaded `Rc<MethodDecl>`s
  by selector, the initializer to the reloaded root class) — the image's
  bytecode is what runs after a load, not a recompilation. The
  `format_version` therefore also versions the compiler's codegen: any
  change to `Instr` or to what the compiler emits bumps it (a serialized
  frame's `pc` is only meaningful against the bytecode it was recorded
  in).
- **Object table**: every heap object reachable from the roots —
  instances, strings, symbols, arrays, and closures — with two-pass
  (placeholder, then fill) decoding so cycles and shared references
  round-trip with identity preserved. A closure encodes its `params`,
  its `ClosureBody::Compiled` code as a `code_id`, and its
  `captured_frame`/`home` as frame-table ids; a `ClosureBody::Ast`
  closure (one created by the tree-walker) is a snapshot error, not a
  silent omission — `snapshot` always runs under the bytecode engine, so
  none can exist in a world it snapshots. `ClosureData.bindings` is
  never serialized: there is exactly one module, and every decoded
  closure receives the reloaded module's bindings map.
- **Frame table**: every `Frame` reachable from the roots (via closures'
  `captured_frame`/`home`, and transitively via `sender`/`home`/
  `captured_frame` from there) — `receiver`, `args`, `temps`, `pc`,
  `sender`, `home`, `current_class` (class tag), `kind` (`Method` as the
  root class's selector, `Closure` as an inline closure record,
  `TopLevel`, `Initializer` as a class tag), `uncontinuable`. Frames and
  objects reference each other freely, so both tables are placeholder-
  allocated before either is filled. This is the "serialize reified
  activations" mechanism the liveness spec requires; in Milestone 2 a
  real `snapshot` of a quiesced world reaches frames only through
  closures still stored in slots (see Non-Goals), and the mechanism is
  exercised in full by tests over a synthetic paused frame chain.
- **Bootstrap from source, not from a shipped base image**: `ns-next run`
  and `ns-next snapshot` both start by bootstrapping the core library
  and `Object` hierarchy from Rust primitives exactly as M1's bootstrap
  already does — there is no pre-built "base image" file Milestone 2
  ships or depends on. `ns-next load` also runs that same bootstrap
  (builtin classes have no image representation beyond their tag), then
  reloads the module from the image's source, then decodes the code,
  object, and frame tables against those live classes.

Overall layout: header, source text, `SourceOrigin`, code table, object
table, frame table, roots.

### `ns-cli` additions

- `ns-next run <file.ns> [--send selector] [--engine=bytecode|tree-walk]`
  (`--engine` defaults to `bytecode`; `tree-walk` selects the M1
  tree-walking evaluation, for differential testing and as a fallback).
  The engine choice is made *before* instantiation, so under `bytecode`
  the root class's instance initializer runs as bytecode too; everything
  else `run` does — bootstrap, load, instantiate with `platform`, send
  the entry selector with `[platform, nil]`, print/exit — is one shared
  code path with the engine as its only parameter.
- `ns-next snapshot <file.ns> [--send selector] <out.img>` — bootstrap,
  load, and (if `--send` given) run `<file.ns>` exactly as `run` would
  under the bytecode engine, then serialize the resulting quiesced world
  (roots: the root instance, and the entry send's result if any) to
  `<out.img>`.
- `ns-next load <image> [--send selector]` — deserialize `<image>`,
  optionally send one further message to its root instance with the
  same `[platform, nil]` argument convention `run` uses, print the
  result the same way `run` does.
- `ns-next disasm <file.ns> (<selector> | --initializer)` — compile
  only, print the `CompiledCode`'s instructions in a human-readable form
  (mnemonic + operands + the statement-index each range belongs to);
  `--initializer` selects the root class's compiled instance initializer
  instead of a method. Nested block bodies are printed indented under
  their `MakeClosure`. Close to free: it's a formatter over
  `CompiledCode`, no new compilation path.
- `ns-next parse` / `parse-corpus` are unchanged.

## Data Flow

```
ResolvedModule (M1, unchanged)
  -> Interpreter::bootstrap + load_module (M1, unchanged)      -> root ClassRef
  -> compiler::compile_module(&root_class, &bindings)          -> CompiledModule
  -> interp.set_engine(Engine::Bytecode(compiled))
  -> interp.instantiate(..) / interp.send(..)   (shared orchestration; hooks 1-3
       route every method / block / initializer body to vm::run_code)
       -> Frame chain, register-based execution, dispatch.rs reused for
          every non-inlined send
       -> Value | NewspeakError
  -> image::snapshot(source, origin, &compiled, roots) -> Vec<u8>  (canonical, versioned)
  -> image::load(bytes) -> (Interpreter with Engine::Bytecode installed, root class, roots)
```

## Error Handling

Unchanged principles from M1: `NewspeakError` for Newspeak-program-level
failures (uncaught exception, `doesNotUnderstand:`, primitive failure),
propagated as the M1 `Signal` `Result` through `vm::run_code`'s
recursion exactly as M1 propagated it through the tree-walker's
recursion; the CLI renders an uncaught error identically under both
engines because the rendering and every error-producing site are shared
code. `panic!`/`debug_assert!` only for `ns-runtime`-internal invariant
violations (compiler bugs, VM dispatch-loop bugs).

**An image is untrusted input** (amended 2026-09-06, after
implementation; this paragraph replaces an earlier one that called a
version-matching image "assumed well-formed, not a hostile input format
in Milestone 2's scope"). That earlier assumption holds only while an
image never leaves the process that wrote it, which `ns-next
snapshot`/`load` makes false: an image is a file the user hands the
runtime, possibly from another machine, an older build, or a damaged
disk — and the liveness spec's portable, architecture-neutral images
make it an interchange format by design. So **no byte sequence may
cause a panic or a process abort**: every id, tag, count and length is
validated before it indexes, allocates or slices, recursion on
attacker-controlled depth is bounded, and every failure is a
`DecodeError` the CLI renders as a message. This was already what the
Milestone 2 implementation plan required of `decode.rs` ("a
corrupt-but-version-matching image is a `DecodeError` (never a panic) at
the byte level"); the amendment brings this document in line with its
own plan and with the shipped code.

Byte-level validity is not structural coherence, so `image::load` runs
two verification passes over the decoded material **before installing
the engine**, and both report a `DecodeError`:

- `verify_code_table` — every jump target is a valid instruction index
  in its own body, every register index is below that body's
  `num_registers`, every `MakeClosure` code id resolves.
- `verify_graph`/`verify_closure` — every decoded frame's
  `args`/`temps` lengths agree with the code that would run in it, and
  a closure's captured frame is an activation of its body's lexical
  parent.

**Scope, stated so later milestones do not over-rely on it:** these are
targeted checks on exactly the paths that could otherwise reach an
unchecked index at run time. They are not a full bytecode verifier —
there is no type-safety or stack-discipline proof. **Open point:**
`verify_closure`'s captured-frame rule currently rests on Milestone 1's
single-user-class invariant (see "Image format" on the `UserRoot` class
tag). The milestone that loads nested classes must revisit it, or it
will start refusing legitimate images.

## Testing

- **Differential testing**: every `tests-fixtures/*.ns` runs under both
  `--engine=bytecode` and `--engine=tree-walk`; exit status, stdout,
  **and stderr** must match byte for byte (stderr too, because every
  error text comes from shared orchestration — see "`vm.rs` and the
  engine hook"). No fixture is exempt. The retained M1 tree-walker is
  the oracle here — this is the concrete payoff of the "keep it as a
  reference path" decision. The fixture set gains one fixture that
  exercises instance initialization (slot initializers reading factory
  parameters, `initExprs`), one for `whileTrue:`, and one for the
  control-flow fallback path. Every `tests-fixtures/*.ns` must still
  exit `0` (M1's `run_fixtures.rs` asserts that for every fixture), so
  the fallback fixture uses a user class that *implements*
  `ifTrue:ifFalse:`; the error-producing fallbacks (non-Boolean receiver
  → `doesNotUnderstand:`, non-Boolean `whileTrue:` condition →
  `mustBeBoolean`) are differential tests over inline sources in the
  harness, compared the same way (exit status, stdout, stderr).
- **Bytecode snapshot tests** (`insta`, via `ns-next disasm`), one per
  representative code shape (plain keyword method, block with non-local
  return, inlined `ifTrue:ifFalse:`, inlined `whileTrue:`, the
  non-inlined fallback case, an instance initializer) — catches
  accidental codegen changes.
- **Control-flow-inlining fallback tests**: non-Boolean receiver to an
  inlined selector still produces the same `doesNotUnderstand:` (or
  correct dispatch, if some other class legitimately implements the
  selector) as the uninlined path — verified by running the same
  fixture under both engines (differential testing already covers this
  as long as at least one fixture exercises it; add one if none does).
- **Image round-trip tests**: `snapshot` then `load` a variety of
  object graphs (empty/simple/deeply nested instances, arrays, shared
  references, a compiled module's whole code table, closures captured in
  a synthetic paused frame chain) and assert identical observable state
  — including identity-hash preservation, class-tag resolution to the
  *same* live `ClassRef`, and `decode(encode(code)) == code` for every
  `CompiledCode` (`Instr` derives `PartialEq` for exactly this).
- **`disasm` tests**: snapshot-test the human-readable output for a
  representative method set.

## Decisions

- Register-based bytecode, `Frame.temps` doubles as the register file —
  no new activation-record concept, extends M1's directly.
- **Dispatch orchestration is shared, not duplicated**: `interpreter/`'s
  send/DNU/lexical/super/native/instantiate logic serves both engines;
  the engine differs only at the three body-execution hooks. Method
  bodies, block bodies, **and instance initializers** all compile to
  bytecode — under `--engine=bytecode` the tree-walker evaluates
  nothing.
- The bytecode is part of the image (code table), so a loaded image runs
  the code it was saved with; `format_version` covers codegen.
- `Instr` is a `match`-dispatched Rust enum, not a hand-packed byte
  array; "architecture-neutral, versioned" is satisfied by the image
  encoding layer, not the in-memory representation.
- Control-flow special selectors are inlined in the **baseline**
  compiler (not deferred to a later JIT tier), guarded by a runtime
  receiver-class check with a correctness-preserving fallback to a real
  virtual send — see Goals for the performance-priority rationale.
- Call-site identity and an inert IC slot are established now so
  Milestone 4 needs no bytecode format break.
- Tree-walking interpreter (M1) is retained permanently as
  `--engine=tree-walk`, used as the differential-testing oracle.
- Image format defined against the logical value model; object
  representation (`Rc<RefCell<Object>>`) can still change later
  (Milestone 4) without an image-compatibility break.
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

Deferred during implementation and review (recorded 2026-09-06, after
the merge). None blocked the milestone; each was judged not worth fixing
before it has a caller that cares. The first two are the ones a later
milestone will actually run into:

- **A profiler or debugger needs `frame.pc` back on every instruction.**
  `vm::run_code` stores it only where control can leave the loop (every
  `Send*`, `MakeClosure`, `NonLocalReturn`), which is exactly the set of
  frames an observer can reach in this milestone — the argument is in
  `run_code`'s doc comment. A normally-returned frame therefore carries
  the `pc` of its last send, not of its return, and a sampling profiler
  or a step-debugger would read a stale value. Either restore the store
  or add an explicit safepoint poll when that arrives. Note also that
  `pc` means a statement index under the tree-walker and an instruction
  index under the VM, with nothing on the frame to say which
  (`types.rs`, `Frame.pc`); Milestone 3 (nested classes and source-free
  images) resolves that rather than inheriting it, because frames are
  serialized and the ambiguity is therefore format-visible.
- **No golden-bytes test pins the image wire format.** Every tag test is
  encode-then-decode against the same tables, so shifting both sides
  together would go unnoticed while `FORMAT_VERSION` stays at 1. That is
  harmless only while no image outlives the build that wrote it; couple
  the golden-bytes fixture to the first `FORMAT_VERSION` bump.
  Relatedly, `the_code_table_encoding_is_deterministic` would pass
  without the sort that makes encoding deterministic (both encodes share
  one `HashMap` instance); compiling the module twice would make it
  prove its name.
- `decode` does not enforce the `Frame` invariants `types.rs` documents
  (`home` only for `FrameKind::Closure`, `current_class` `None` only for
  `TopLevel`). A crafted section can produce an impossible-state frame;
  no reader unwraps on either field today, so it is inert rather than a
  panic path.
- `encode_graph_section` leaves partial bytes in the `Writer` when it
  returns `Err`. The only caller drops the writer, but the function does
  not say the caller must.
- `image::decode`'s `own_registers` allocates a `Vec` per instruction
  during verification (load-time only, linear in code size).
- `ns-next snapshot` prints `wrote <path>` on stdout rather than stderr,
  so a caller parsing its stdout sees tool chatter.

## Open Questions Resolved During Planning

Recorded 2026-09-05, while writing (and then re-checking) the
implementation plan against every named section of the M1 runtime spec
and the liveness spec, and against the actual M1 source. Each entry
names the gap, why it was a gap, and the correction now reflected in the
sections above. The defect class being eliminated is *spec silence the
plan had to guess at*; where the plan had guessed, the guess is
retracted here.

- **Class Instantiation — GAP, RESOLVED.** The original design never
  said what happens to the M1 spec's "Class Instantiation" section
  (slot initializers, `initExprs`, the six-step algorithm). The first
  plan draft guessed "leave it on the tree-walker under every
  `--engine`", which contradicts the project's performance-priority
  decision (instantiation runs a real expression body per slot and per
  `initExpr`, on every `new`) and the Goals' "no Newspeak-level code is
  tree-walked under `--engine=bytecode`". Resolution: instance
  initializers compile to `CompiledCode { kind: Initializer }` and run
  through hook 3 (see "Class instantiation"), with every M1
  instantiation gap carried forward identically in both engines.
- **Block bodies — GAP, RESOLVED.** The first plan draft compiled only
  method bodies: `MakeClosure` carried the block's AST and every block
  ran through the tree-walker's `invoke_closure` under both engines,
  with no `NonLocalReturn` opcode — silently dropping the Goals' "every
  block body" and this document's own "Non-local return" section. Root
  cause was the same "don't touch `interpreter/`" constraint that
  produced the instantiation gap. Resolution: `ClosureBody::Compiled`,
  hook 2, `NonLocalReturn` (see "Blocks and closures").
- **Shared orchestration instead of a parallel `Vm` — CHANGED.** The
  first draft's `vm.rs` re-implemented `send`/DNU/lexical/super/native
  dispatch (~200 lines duplicated "by design"). Once hooks 1-3 exist the
  duplication has no justification and one real cost: two copies of
  every error text to keep identical. Resolution: `Interpreter.engine`
  plus three hooks; `vm.rs` is the run loop only. The native/builtin
  dispatch path (`Lookup::Native` → `dispatch_native_inheriting_object`
  → `builtins::dispatch_native`, including the `Object`-inheritance
  step and the block-invoking primitives' `sender` threading) is
  therefore shared verbatim, not re-implemented — re-verified against
  `interpreter/mod.rs` and `builtins/mod.rs` after this revision.
- **`FrameKind::Initializer` — CHANGED** (this document previously said
  `FrameKind` gains no new variant). See "`vm.rs` and the engine hook".
- **Bytecode in the image — GAP, RESOLVED.** This document's
  `bytecode.rs` section assigned the encoding of compiled code to
  `image::encode`/`decode` and the liveness spec says bytecode "is part
  of the image", but the Image format section never listed it and the
  first plan draft dropped it ("the compiled-method cache is never part
  of the image"). Resolution: the code table (see "Image format");
  `format_version` covers codegen.
- **Frame-chain and closure serialization — GAP, RESOLVED.** Non-Goals
  and Testing require the Frame-chain mechanism to be implemented and
  round-trip tested via a synthetic paused state; the first plan draft
  rejected any reachable closure with an error and encoded no frames at
  all. Resolution: the object table encodes closures, the frame table
  encodes frames, both placeholder-allocated for mutual references (see
  "Image format").
- **Root/special-objects table — CLARIFIED.** Realized as scalar tags
  for `nil`/`true`/`false` (no heap identity in the M1 value model) and
  class tags resolved against the bootstrap for every class; the
  property the liveness spec wants (identity of well-known objects
  survives a round-trip) holds, the mechanism is stated.
- **Identity-hash counter after `load` — GAP, RESOLVED.** Preserving
  hashes verbatim is not enough: a freshly bootstrapped interpreter's
  counter would hand out hashes the image already uses. `load` advances
  the counter past the image's maximum.
- **`whileTrue:`/`whileFalse:`/`ifFalse:ifTrue:` — GAP, RESOLVED.** The
  M1 core library has no native implementation of these; the first plan
  draft therefore excluded the `whileTrue:` fixture from differential
  testing ("bytecode-only capability") — which is precisely an
  engine-observable semantic difference, and even an intra-engine one
  (a literal-block receiver would loop, a variable receiver would
  `doesNotUnderstand:`). Resolution: add the three natives to
  `builtins::block`/`builtins::boolean` (Goals), so every inlined
  selector's guard-failure fallback and every non-inlined send of it
  behave identically in both engines and the differential harness
  exempts nothing. The natives' non-Boolean-condition behavior is the
  `mustBeBoolean` send (see "Control-flow inlining"), chosen precisely
  because the inlined loop can reproduce it without re-running the
  condition block.
- **Inlined blocks and lexical addressing — GAP, RESOLVED** (found
  2026-09-05 re-checking the first plan draft's inlining code against
  `resolver/scope.rs` and `eval.rs`'s `frame_at_depth`). The draft
  lowered an inlined block's statements with the ordinary per-statement
  allocator reset (clobbering the enclosing statement's live registers),
  emitted its `Binding::OuterLocal` depths unchanged (one hop too many
  for every reference crossing the inlined scope, wrong whenever the
  enclosing frame is a closure), would have mapped a block-local `| t |`
  onto the enclosing method's register `t`'s index, and pushed the
  inlined statements into `statement_pc` under the wrong indices.
  Resolution: the gate rejects blocks with temporaries; the compiler
  keeps a scope stack and rewrites depths; inlined statements are
  sub-expressions of the enclosing statement for both allocation and the
  PC table; `^` inside an inlined block follows the enclosing code's
  kind. All stated in "Control-flow inlining".
- **`whileTrue:` guard failure re-ran the condition — GAP, RESOLVED.**
  The first draft's fallback materialized the condition closure and
  re-sent `whileTrue:`, running the condition body a second time (side
  effects included) before erroring — engine-observable. Resolution: the
  `mustBeBoolean` convention, shared by natives and inlined loops.
- **Malformed integer literals — GAP, RESOLVED.** The first draft's
  compiler `panic!`ed on a literal `i64::from_str` rejects; the lexer
  accepts `99999999999999999999`, and M1 reports it as a `NewspeakError`
  at evaluation. Resolution: `Literal::MalformedInt` defers the shared
  `parse_int_literal` error to execution.
- **`platform` singleton identity across a round-trip — GAP,
  RESOLVED.** Added to the special-object tags (see "Image format");
  without it a loaded module's `platform`-holding slots would point at
  an object distinct from `load --send`'s `platform` argument.
- **Carried forward, verified in `interpreter/mod.rs`:**
  `invoke_method` performs no arity check (M1's `ns-cli` always sends
  `[platform, nil]` regardless of the entry selector's arity, and
  `read_local`'s args-then-temps split makes a surplus argument shift
  nothing that a fixture observes). Shared code, identical in both
  engines; a fix belongs in `invoke_method`, not in one engine.
- **`CompiledMethod` → `CompiledCode`, and opcodes beyond the original
  list — CHANGED.** `LoadBuiltinClass`, `ImplicitReturn`, `InitSlot`,
  `JumpIfNotBoolean`, `BranchIfTrue` were not in the first opcode list;
  `MakeClosure`'s operands and `SendLexical`/`SendSuper`'s lack of a
  receiver register are now stated precisely; `Literal::MalformedInt`
  was added on 2026-09-05. All are recorded in the `bytecode.rs` section
  so the plan and any future reader share one list.
- **Testing-harness fit — CLARIFIED.** M1's `run_fixtures.rs` asserts
  exit `0` for every `tests-fixtures/*.ns`; the first draft's fallback
  fixture exited non-zero. See "Testing". Also noted for the plan: M1's
  `Signal` deliberately has no `Debug` impl, so tests unwrap
  `EvalResult`s through a helper, never `.expect()`.
- **Register reuse within a statement — SOFTENED** from "in scope" to
  "permitted, not required"; see the `bytecode.rs` section for why.
- **`disasm` and the initializer — CLARIFIED.** `--initializer` selects
  the compiled instance initializer; nested block code is printed
  indented.
- **`load --send` arguments — CLARIFIED.** Same `[platform, nil]`
  convention as `run`; an earlier draft sent no arguments, which would
  have read uninitialized registers for `main:args:`-shaped entry
  points.
- **Carried forward unchanged from the M1 spec, verified against the
  source, no M2 disposition previously stated:** the Resolver
  (`Binding` variants, the `Span`-keyed bindings map, the
  "bare builtin name has no binding" convention, factory parameters as
  `Local`s in initializer scope); the Object Model (`Value`, `Object`,
  `ObjectData`, `ClassData` — the only type change is `ClosureData.body`);
  Source Retention and Provenance; the three `dispatch.rs` lookups and
  `kth_enclosing_object`; the Core Library (plus the three natives
  above); `ns-cli`'s `parse`/`parse-corpus`; the M1 Error Handling and
  Testing conventions; all three "Documented Deviations" (no
  metaclasses — which the image's class-tag scheme relies on; no value
  objects; `self`/`super`/`outer`/`true`/`false`/`nil` not
  lexer-reserved); and the M1 Non-Goals this milestone does not lift
  (bignums, typechecker, foreign interop). The lexer `-42` deferral and
  `--bench` are explicitly deferred again (Non-Goals).
- **Liveness-spec requirements re-checked after the revision:** Req. 1
  (the compiler is an ordinary runtime function over a live
  `ClassRef`; `CompiledModule` is a mutable cache keyed by method
  identity, so a replaced method gets a new entry); Req. 2 (statement→PC
  table + growable `temps`, which hook 3 already exercises); Req. 3
  (nothing bakes a method-body pointer into a durable object — a
  closure holding its own `CompiledCode` is the block's *identity*,
  exactly as its AST was in M1; `InitSlot`'s field index is the one
  layout dependency, flagged for M4's flush); Req. 4-5 (image format
  above; `run` never snapshots); Req. 6 (register VM, baseline
  inlining); Req. 7 (no new global state — `Engine` lives on the
  `Interpreter`; the `thread_local!` symbol interner is an M1 property
  the image's re-interning already accommodates); the Debugger and
  Mirror non-goals are not foreclosed (frames stay heap-reified,
  `Instr` in a `Vec` is patchable for trap opcodes, dispatch stays
  dictionary-driven); the M2 rows of both milestone tables are
  delivered in full.
