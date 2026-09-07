# Execution — activations, both engines, bytecode, the VM

> **Status:** built.
> **Code:** `crates/ns-runtime/src/types.rs` (`Frame`), `bytecode.rs`,
> `compiler/`, `vm.rs`, `interpreter/` (the shared orchestration and the three
> hooks).

## Two engines, permanently

`ns-next run` defaults to `--engine=bytecode`. `--engine=tree-walk` is the
Milestone 1 interpreter, kept **permanently** as a differential oracle rather
than as a transitional convenience.

**For every program both engines must produce identical exit status, stdout and
stderr.** `crates/ns-cli/tests/differential.rs` enforces this over every
`tests-fixtures/*.ns` plus inline sources, with no exemptions, and `run-corpus`
does the same over the runnable corpus. If a change makes the engines disagree,
the tree-walker is the reference — fix the compiler or the VM.

Two engines that must agree byte for byte is a verification property this project
funds on purpose. When the moving collector lands, the tree-walker gets its own
handle-based root discipline rather than being retired; see
[memory.md](memory.md).

The one thing the tree-walker cannot do is run from a loaded image: `image::load`
decodes mixins and class objects, never AST-carrying bindings, so there is no
source for it to execute. `Interpreter::set_engine(Engine::TreeWalk)` refuses on
a loaded image rather than running it against the wrong data.

## Activations

A Newspeak-level activation is never represented only by Rust locals or the Rust
call stack:

```rust
struct Frame {
    receiver: Value,
    args: Vec<Value>,
    temps: Vec<Value>,        // the register file, indexable and growable
    pc: usize,
    sender: Option<FrameRef>,
    home: Option<FrameRef>,   // for a block: its method activation
    current_class: ClassRef,  // NOT necessarily the receiver's class
    kind: FrameKind,
    uncontinuable: bool,
    slot_base: usize,
}
```

`current_class` is required by §3.6: the current class is the class that actually
**defines** the running method, not the receiver's concrete class, and it is what
`self`, `outer` and `super` sends inside that method resolve relative to. It is
set by the dispatch step that creates the activation and never derived from the
receiver at read time, which is what makes chained `super` sends through a deep
hierarchy resolve one level at a time.

`pc` means a statement index under the tree-walker and an instruction index under
the VM. Frames are serialized, so this is a format-visible ambiguity and the
frame carries a discriminator rather than leaving it implicit.

## The register file

Each `Frame` carries `args ++ temps` as one unified register space, addressed by
a single `u16`: index `< num_args` is `args[index]`, index `>= num_args` is
`temps[index - num_args]`. That is exactly the args-then-temps split the
resolver's `Binding::Local` already used, so the compiler needs no separate
renumbering, and a block's `OuterLocal` indices are untouched by the scratch
registers a bytecode frame adds — they sit after the named locals.

Registers above the named locals are compiler-allocated scratch, handed out by a
bump allocator reset to the first scratch index at the start of every statement.
Newspeak statements are the unit `pc` and safepoints already work at, so scratch
never needs to stay live across a statement boundary. There is no cross-statement
liveness analysis.

**An inlined block's statements are not statements of the enclosing code for
allocation purposes.** They are lowered as sub-expressions of the enclosing
statement, so the allocator resets only to the watermark at the inlined block's
start — never to the statement base, which would clobber the enclosing
statement's still-live registers: the guard register, the branch's result
register, an already-evaluated sibling argument.

**An expression's value register must still hold that value when its consumer
executes.** A named local's register is live storage, not scratch. `lower_operand`
copies it when anything lowered afterwards can write it, and
`lower_expr_consumed_immediately` is the counterpart for positions where nothing
is lowered afterwards. Getting this wrong is silent wrong output with exit status
0, not a crash.

## Instructions

`Instr` is a plain Rust enum dispatched by `match`, not hand-packed bytes.
"Architecture-neutral and versioned" is a property of the *image encoding* of
compiled code, not a requirement on the in-memory representation. Manual opcode
packing is a technique that pays off once a JIT or a very hot loop demands it;
building it now would add real complexity for no measurable benefit.

The set follows the resolver's binding variants and the lookup shapes directly:

- `LoadLiteral`, `LoadSelf`, `LoadBuiltinClass`, `Move`, `StoreLocal`
- `LoadOuterLocal { dst, depth, index }` / `StoreOuterLocal` — through the
  `captured_frame` chain
- `SendPublic { selector, receiver_reg, arg_regs, dst, call_site }`,
  `SendLexical { depth, … }` (receiver is `self` by construction),
  `SendSuper { … }`
- `MakeClosure { dst, params, code }` — `captured_frame` and `home` are run-time
  values the VM supplies from its current frame, never operands
- `MakeArray`
- `InitSlot { src, own_index }` — see below
- `Return`, `ImplicitReturn`, `NonLocalReturn`
- `Jump`, `JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue` — emitted only by
  control-flow inlining; nothing else in the compiler emits a branch

```rust
CompiledCode {
    kind: CodeKind,           // Method | Block | Initializer
    instrs: Vec<Instr>,
    num_args: u16,
    num_registers: u16,
    statement_pc: Vec<(usize, Range<usize>)>,   // safepoints
    call_sites: Vec<CallSite>,
    source: Span,
}
```

`CompiledModule` holds `methods`, `initializers`, `superclass_clauses` and
`superclass_messages`, plus the resolver's bindings map.

### Compiled code is keyed by mixin

One mixin backs every application of it, so a body is compiled once per mixin and
reused across every application. §6.2: "they all share their code and structural
definition with the mixin." Keying by class object would recompile the same body
per enclosing instance.

**`InitSlot`'s `own_index` is therefore mixin-relative.** An absolute field index
baked into shared code would be wrong the moment a second application inserts
different inherited slots ahead of it. The VM resolves `own_index` against the
executing `Frame.slot_base` — the application's `own_slot_base` for that level —
at run time.

The tree-walker has no compiled `InitSlot` to share, so it resolves the same slot
by name instead of by index. A bug here shows up as the two engines disagreeing,
not as a single wrong answer.

## Control-flow inlining

Triggers only at **compile time**, on the syntactic shape of the send, never as a
run-time heuristic. The gate: the send is an ordinary explicit-receiver send with
one of the fixed special selectors, and every block-position argument — plus, for
`whileTrue:`/`whileFalse:`, the receiver — is a syntactic block literal directly
at the call site, declaring no parameters and no temporaries.

The no-temporaries rule matters: a block with its own `| t |` gets a fresh `t`
per invocation, and a closure created inside a loop body would observe that. A
register can only be one `t`, so such blocks are simply not inlined.

When the gate passes the compiler emits a receiver-class run-time guard, branches
around the two bodies lowered inline — no `MakeClosure`, no new `Frame` — and,
critically, **an else branch that falls back to a real virtual send** with the
original selector and now-materialized block arguments. That preserves "every
send is virtual" exactly: a program that overrides `ifTrue:ifFalse:` on a
non-Boolean receiver observes identical behaviour to an uninlined send, just
slower on the rare path.

`whileTrue:`/`whileFalse:` inline as an actual back-edge jump — a real loop in
the bytecode, not repeated closure invocation — with the guard re-checked each
iteration. Their guard is different in kind: the receiver is a block literal by
construction, so what is checked is the *condition block's value*. A non-Boolean
condition cannot fall back to re-sending `whileTrue:`, because that would re-run
the condition body and duplicate its side effects. Instead the natives define the
behaviour and the inlined form reproduces it: both send `mustBeBoolean` to the
offending value, an ordinary virtual send, and answer its result. No class
implements `mustBeBoolean`, so both engines produce the identical
`doesNotUnderstand:` through the shared path.

**Lexical addressing inside an inlined block.** The resolver assigned
`OuterLocal { depth, index }` by counting *scopes*, while the VM walks *frames*,
and an inlined block has a scope but no frame. The compiler keeps a scope stack
and rewrites every local reference: the run-time hop count is the number of frame
scopes among the innermost `depth` entries, and a hop count of zero is a plain
register access. The rewrite applies equally to a real block compiled inside an
inlined region, whose `MakeClosure` captures the enclosing frame.

**`^` inside an inlined block** compiles to whatever a `^` at the top of the
enclosing code would: `Return` in a method body, `NonLocalReturn` in a block
body or an instance initializer.

Testing has to cover the fallback paths explicitly, not just the common case: a
non-Boolean receiver must still produce the identical `doesNotUnderstand:`, a
user class that implements `ifTrue:ifFalse:` must have its method run, and a
non-Boolean `whileTrue:` condition must produce the identical `mustBeBoolean`
error.

## The VM and the three hooks

`Interpreter` carries `engine: Engine`, which is either `TreeWalk` or
`Bytecode(CompiledModule)`. Everything in `interpreter/` that decides *what* to
run is shared and unchanged. Exactly three places decide *how* a body runs:

1. **Method body** — `invoke_method`, after building the frame.
2. **Block body** — `invoke_closure`, dispatching on `ClosureData.body`, which is
   `ClosureBody::Ast` or `ClosureBody::Compiled`. A closure therefore always runs
   with the engine that created it, and "under `--engine=bytecode` no
   Newspeak-level code is tree-walked" is enforced by the type rather than by a
   flag.
3. **Instance initializer** — `instantiate`'s per-level "run this level's own
   slot initializers and initializer expressions" step.

`vm.rs` is therefore *only* a `match`-dispatch loop over `Instr`. Its send
handlers call straight back into the shared orchestration, which re-enters
`run_code` through hook 1 or 2 for any user-defined target. The VM has no
dispatch logic, no `doesNotUnderstand:` construction, no native dispatch and no
error text of its own — every error a program can observe comes from the same
shared code under both engines. That is what makes the tree-walker a byte-for-byte
oracle including stderr, rather than a merely similar one.

## Safepoints and `pc`

`CompiledCode.statement_pc` maps each statement index to its instruction range —
the safepoint abstract-state map the liveness design requires. Every
`CompiledCode` carries it, and a stable call-site identity with a reserved inert
inline-cache slot per send, so inline caches need no bytecode format break.

`frame.pc` is stored **only by the instructions that can hand the frame to an
observer**: every `Send*` (a nested activation runs with this frame as its
sender), `MakeClosure` (the frame escapes into the object graph, where the image
encoder can serialize it), and `NonLocalReturn` (control leaves with a signal).
Nothing can read `pc` while a straight-line instruction executes, because
`run_code` never yields between them. Keeping the store out of the straight-line
arms takes a `RefCell` borrow and a write off the hot path for a value nothing
observes there.

An inlined block's statements are part of the enclosing statement's range;
`statement_pc` records only the code body's own statements. A debugger wanting
finer stepping inside inlined blocks would extend the table.

## Non-local return

A closure's `home` is the enclosing method or initializer activation. A block
body's `^` compiles to `NonLocalReturn`, whose handler walks to `home`, checks
`uncontinuable` — raising `cannotReturn` if set — marks the target uncontinuable,
and returns a return signal. That signal propagates out through every intervening
Rust frame, native primitives included, until the shared `invoke_method` that
owns `home` catches it.

A method body's own `^` compiles to plain `Return` and raises no signal at all;
`invoke_method` marks the frame uncontinuable on every exit path regardless, so
the observable discipline is preserved without a signal round-trip on the common
path.

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvalResult`.** `Signal`
deliberately has no `Debug` — it holds a frame cycle. Tests unwrap through a local
`expect_ok` helper.

## Compilation cannot fail

**Compiling an already-resolved module can never raise a `NewspeakError`.**
Compilation is a total, deterministic lowering; a compiler-invariant violation is
`panic!` or `debug_assert!`, not a program-level error.

A program-level error that the tree-walker raises at evaluation time is therefore
*deferred to execution* so both engines report it identically. Two literals carry
that deferral: `Literal::MalformedInt`, for an integer the lexer accepted but the
parser rejects (out of `i64` range, a fractional radix literal), where the VM
re-runs the shared `parse_int_literal` when the instruction executes; and
`Literal::UnresolvedIdent`.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping
and compiling excluded — over `bench_loop.ns`, a 50,000-iteration `whileTrue:`
loop, on a release build. Bytecode median ~19.6 ms against tree-walk ~37.4 ms.

That ratio is **not** a pure dispatch comparison: the baseline compiler inlines
`whileTrue:`, so the bytecode engine does not pay the loop-control overhead the
tree-walker does. The bytecode median alone is the before-number for inline
caches, since every send today is still a full lookup.

Never judge performance from a debug build.

## Planned: trampolined sends

Nested sends recurse through the Rust call stack: `run_code` → `interp.send` →
hook → `run_code`. The Newspeak activation chain therefore exists twice — reified
on the heap and implicitly on the Rust stack. Only the first can be serialized;
only the second can be resumed.

Replacing the recursion with an explicit frame stack in `run_code`, where a send
pushes instead of recursing, is required by three different things: the moving
collector wants to scan roots without searching the Rust stack, the debugger
wants stepping and a suspendable activation, and any actor scheduling wants
activations it can leave lying around. It is also what would make
suspend-and-resume possible; see [image-format.md](image-format.md).

The alternative — keep recursing and unwind through a signal — is not viable,
because native frames that call back into Newspeak would be lost.

## Planned: JIT tiers

Beyond inline caches, nothing is designed. The tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md) under performance, and
the constraint that binds it is that no native code may ever enter an image.

## Decisions

- **`Instr` is a Rust enum, not packed bytes.** Revisit only if profiling shows
  `match` dispatch is the bottleneck.
- **Inlining is gated syntactically at compile time and guarded again at run
  time**, with a fallback to a real send, so virtuality is never actually lost.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, which is what makes the differential harness meaningful.
- **Code is keyed by mixin and `InitSlot` is mixin-relative**, because one mixin
  backs many applications with different inherited layouts.
- **`pc` is stored only at observable points**, not at every instruction, which is
  a deliberate hot-path trade the debugger milestone may have to revisit.
