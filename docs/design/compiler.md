# Compiler — AST to bytecode

> **Status:** built.
> **Code:** `crates/ns-runtime/src/bytecode.rs` (the instruction set and the
> compiled-code shapes), `compiler/mod.rs` (entry points), `compiler/lower.rs`
> (lowering, register allocation, the scope stack), `compiler/control_flow.rs`
> (inlining). CLI: `ns-next disasm`.

This is the producer side. How the bytecode is then run — activations, the two
engines, the dispatch loop — is [execution.md](execution.md).

## The instruction set

`Instr` is a plain Rust enum dispatched by `match`, not hand-packed bytes.
"Architecture-neutral and versioned" is a property of the *image encoding* of
compiled code — a tag byte and a fixed operand layout per instruction, with the
tag numbers fixed for good; see [image-format.md](image-format.md) — not a
requirement on the in-memory representation. Manual opcode packing is a technique
that pays off once a JIT or a very hot interpreter loop demands it; building it
now would add real complexity for no measurable benefit. Revisit only if
profiling shows `match` dispatch is the bottleneck.

Every register operand is a `u16`; every jump target is an instruction index. The
set follows the resolver's `Binding` variants and the lookup shapes directly, so
that lowering is mostly a transliteration rather than a translation.

**Loads and moves**

- `LoadLiteral { dst, literal }` — the literal is already lowered:
  `Nil | Bool | SmallInt | Float | Str | Sym | Char | Array | MalformedInt |
  UnresolvedIdent`, with integer and float text parsed at compile time rather
  than at each execution. The last two are the deferred cases; see "Compilation
  cannot fail".
- `LoadSelf { dst }`.
- `LoadBuiltinClass { dst, kind }` — serves the convention that a bare builtin
  class name has no `Binding`, resolving at compile time what the tree-walker's
  `eval_ident` resolves in its fallback.
- `Move { dst, src }`.
- `StoreLocal { src, slot }` — a `Binding::Local` **write**. A read needs no
  instruction at all: **the local's register *is* its value.** That is the fact
  the value-register invariant below depends on.
- `LoadOuterLocal { dst, depth, index }` / `StoreOuterLocal { src, depth, index }`
  — `Binding::OuterLocal`, walking the `captured_frame` chain exactly as
  `frame_at_depth` does.

**Sends** — each carries a `call_site`; see "Call-site identity".

- `SendPublic { selector, receiver_reg, arg_regs, dst, call_site }` — an ordinary
  explicit-receiver send, resolved with `lookupPublic`.
- `SendLexical { depth, selector, arg_regs, dst, call_site }` — `Binding::LexicalSend`.
  **No receiver register**: the receiver is `self` by construction, and the
  kth-enclosing object is computed at run time.
- `SendSuper { selector, arg_regs, dst, call_site }` — receiver is `self`, lookup
  starts at the current activation's `current_class`'s superclass.

**Construction**

- `MakeClosure { dst, params, code }` — `code` is the block body's own compiled
  `CompiledCode`, nested inside the enclosing code; nested block literals nest
  again. `captured_frame` and `home` are run-time values the VM supplies from its
  current frame, never operands.
- `MakeArray { dst, elem_regs }`.
- `InitSlot { src, own_index }` — writes one of `self`'s own instance slots.
  Emitted **only** by initializer compilation, for a slot's own initializer
  expression. The one layout-dependent instruction in the set; see "Keyed by
  mixin".

**Leaving a body**

- `Return { reg }` — a `^` in a method or initializer body.
- `ImplicitReturn { reg }` — a body that fell off its end: a method returns
  `self`, a block its last expression's value. The VM treats it exactly like
  `Return`; it is kept distinct so `disasm` output stays legible.
- `NonLocalReturn { reg }` — a `^` inside a block body.

**Branches**

- `Jump { target }`, `BranchIfFalse { reg, target }`, `BranchIfTrue { reg, target }`,
  `JumpIfNotBoolean { reg, target }`.

**Nothing but control-flow inlining emits a branch.** Every other construct in the
language is a send, so a jump in the instruction stream is always the trace of an
inlining decision. `JumpIfNotBoolean` is the inlining guard: for the unboxed
`Value::Bool` representation, "is the receiver's class exactly `True`/`False`?" is
precisely "is this register a `Bool` at all".

## Compiled code

```rust
CompiledCode {
    kind: CodeKind,           // Method | Block | Initializer
    instrs: Vec<Instr>,
    num_args: u16,
    num_registers: u16,       // args + named temps + scratch
    statement_pc: Vec<(usize, Range<usize>)>,   // safepoints
    call_sites: Vec<CallSite>,
    source: Span,
}
```

A method body, a block body and an instance initializer all compile to the same
shape; `kind` tags which. `Frame.temps` is sized to `num_registers - num_args`.

`CompiledModule` holds `methods`, `initializers`, `superclass_clauses` and
`superclass_messages` — the last two because a nested class's superclass clause is
code too, evaluated by the accessor once per enclosing instance — plus the
resolver's bindings map, carried so the hooks can pass it to the shared
orchestration.

### Keyed by mixin

One mixin backs every application of it, so a body is compiled **once per mixin**
and reused across every application. §6.2: "they all share their code and
structural definition with the mixin." Keying by class object would recompile the
same body per enclosing instance, since a class object is per-instance.

**`InitSlot`'s `own_index` is therefore mixin-relative.** An absolute field index
baked into shared code would be wrong the moment a second application inserts
different inherited slots ahead of it. The VM resolves `own_index` against the
executing `Frame.slot_base` — the application's `own_slot_base` for that level —
at run time.

The tree-walker has no compiled `InitSlot` to share, so it resolves the same slot
by name instead of by index. A bug here shows up as the two engines disagreeing,
not as a single wrong answer.

Compiled initializers are layout-dependent, so a class-shape change must flush
them along with every other cache keyed to the class. See
[liveness-and-reflection.md](liveness-and-reflection.md).

## Registers

Each `Frame` carries `args ++ temps` as one unified register space addressed by a
single `u16`: index `< num_args` is `args[index]`, index `>= num_args` is
`temps[index - num_args]`. That is exactly the args-then-temps split the
resolver's `Binding::Local` already used, so the compiler needs no separate
renumbering, and a block's `OuterLocal` indices are untouched by the scratch
registers a bytecode frame adds — scratch sits after the named locals.

Registers above the named locals are compiler-allocated scratch from a bump
allocator **reset to the first scratch index at the start of every statement**.
Statements are the unit `pc` and safepoints already work at, so scratch never
needs to stay live across a statement boundary. There is no cross-statement
liveness analysis, and reusing registers within one statement's expression tree is
permitted but not done: Newspeak method bodies are not deeply nested in practice
and `temps` is a plain growable `Vec`, so a few extra live entries per statement
cost nothing measurable.

**An inlined block's statements are not statements of the enclosing code for
allocation purposes.** They are lowered as sub-expressions of the enclosing
statement, so the allocator resets only to the watermark at the inlined block's
start — never to the statement base, which would clobber the enclosing statement's
still-live registers: the guard register, the branch's result register, an
already-evaluated sibling argument.

### The value-register invariant

**An expression's value register must still hold that value when its consumer
executes.** This follows directly from "a read of a local needs no instruction":
because a named local's register *is* its value, that register is live storage,
not scratch, and anything that writes the local writes the expression's value too.

`lower_operand` copies when anything lowered afterwards can write the register;
`lower_expr_consumed_immediately` is the counterpart for positions where nothing
is lowered afterwards. Getting this wrong is silent wrong output with exit status
0, not a crash — which is why it is an invariant in `CLAUDE.md` rather than a
comment.

## Control-flow inlining

Triggers only at **compile time**, on the syntactic shape of the send, never as a
run-time heuristic. The gate: the send is an ordinary explicit-receiver send —
never lexical or super, since an implicit receiver for `ifTrue:` is meaningless —
its selector is one of the fixed special selectors, and every block-position
argument, plus the receiver for `whileTrue:`/`whileFalse:`, is a syntactic block
literal directly at the call site, declaring **no parameters and no
temporaries**. If any condition fails the compiler emits an ordinary send and
does not try.

The no-temporaries rule matters: a block with its own `| t |` gets a fresh `t` per
invocation, and a closure created inside a loop body would observe that. A
register can only be one `t`, so such blocks are simply not inlined and keep their
exact semantics.

When the gate passes the compiler emits a receiver-class run-time guard, branches
around the two bodies lowered inline — no `MakeClosure`, no new `Frame` — and,
critically, **an else branch falling back to a real virtual send** with the
original selector and now-materialized block arguments. The compile-time gate and
the run-time guard are two independent checks, and the fallback is what preserves
"every send is virtual" exactly: a program that overrides `ifTrue:ifFalse:` on a
non-Boolean receiver observes identical behaviour to an uninlined send, just
slower on the rare path.

`whileTrue:`/`whileFalse:` inline as an actual back-edge jump — a real loop in the
bytecode, not repeated closure invocation — with the guard re-checked each
iteration, which is cheap because it is a tag check.

Their guard is different in kind. The receiver is a block literal by
construction, so its class is always `Block` and there is no receiver ambiguity to
fall back from; what the guard checks is the **condition block's value** each
iteration. A non-Boolean condition cannot fall back to re-sending `whileTrue:` to
a materialized closure, because that would re-run the condition body and duplicate
its side effects before the native could report anything — an engine-observable
difference. Instead the natives define the behaviour and the inlined form
reproduces it: both send `mustBeBoolean` to the offending value, an ordinary
virtual send, and answer its result. No class implements `mustBeBoolean`, so both
engines produce the identical `doesNotUnderstand:` through the shared path, and a
user class that does implement it is observed identically under both.

### Lexical addressing inside an inlined block

The resolver assigned `OuterLocal { depth, index }` by counting **scopes** — every
block literal is one scope — while the VM walks **frames** via `captured_frame`,
and an inlined block has a scope but no frame.

The compiler therefore keeps a scope stack, one entry per code body's own scope
and one per inlined block being lowered, and rewrites every local reference: the
run-time hop count for `depth d` is the number of *frame* scopes among the
innermost `d` entries. A hop count of zero is a plain register access, the same
no-instruction case as `Binding::Local`; otherwise it is an outer-local
instruction with the reduced depth.

The rewrite applies equally to a real block compiled inside an inlined region,
whose `MakeClosure` captures the enclosing *frame*, so a reference the resolver
counted through the inlined scope is one hop shorter at run time. A
`Binding::Local` inside an inlined scope cannot occur, because the gate admits no
names there, and is a compiler-invariant violation. Lexical and super sends are
unaffected: they resolve against the frame's receiver and `current_class`, which
an inlined block shares.

### `^` inside an inlined block

Compiles to whatever a `^` at the top of the *enclosing* code would: `Return` in a
method body, `NonLocalReturn` in a block body. In an instance initializer it
compiles to `NonLocalReturn`, and nothing catches it — a gap both engines share
identically.

### Testing the fallbacks

Coverage has to include the paths the gate is designed to leave alone, not just
the common case: a non-Boolean receiver must still produce the identical
`doesNotUnderstand:` a fully virtual send would; a user class that *implements*
`ifTrue:ifFalse:` must have its method run; a non-Boolean `whileTrue:` condition
must produce the identical `mustBeBoolean` error.

## Safepoints and call-site identity

`statement_pc` maps each statement index to its instruction range. This is the
safepoint abstract-state map the liveness design requires, and it has four
consumers — debugger, JIT deopt, image snapshot, mirrors — so it is emitted from
the start rather than added when the first of them is built.

An inlined block's statements are part of the enclosing statement's range;
`statement_pc` records only the code body's own statements. A debugger wanting
finer stepping inside inlined blocks would extend the table.

`CallSite` is one entry per send instruction, index-addressed: the id is the send
instruction's own index into `instrs`, so it is stable and reconstructible with no
separate counter to drift. Each send carries its call site by value and
`call_sites` lists them in instruction order. **Nothing reads or writes them
today** — they are inert, existing so that inline caches can start writing state
into an already-present slot instead of changing `Instr`'s shape, which would be
an image format break.

## Compilation cannot fail

**Compiling an already-resolved module can never raise a `NewspeakError`.**
Compilation is a total, deterministic lowering; a compiler-invariant violation is
`panic!` or `debug_assert!`, not a program-level error.

A program-level error that the tree-walker raises at evaluation time is therefore
**deferred to execution**, so both engines report it identically and at the same
moment. Two literals carry that deferral:

- `Literal::MalformedInt` — an integer the lexer accepted but the parser rejects,
  such as one out of `i64` range or a fractional radix literal. The compiler
  lowers it to `MalformedInt`; the VM re-runs the shared `parse_int_literal` when
  the instruction executes, producing the identical error text the tree-walker
  does, and only if execution actually reaches it.
- `Literal::UnresolvedIdent`.

## Reading the output

`ns-next disasm <file.ns> <selector>` prints one body's instructions, compiling
only — nothing is instantiated or run — and addressing code through mixins,
since that is what code is keyed by. It also accepts an artifact in place of a
`.ns` file, decoding rather than compiling; a deployed binary ships no source
at all, so that path is what makes `disasm` usable on one. See
[image-format.md](image-format.md)'s "Reading an artifact".

For every body at once rather than one, `ns-next inspect <artifact> --full`
disassembles the whole code table with each body attributed to the declaration
that owns it. `disasm` is the way in when you know which body you want;
`inspect` is the way in when you do not.

These are the first pieces of the VM's own introspection tooling, the equivalent
of `d8`; the rest, inline-cache transition traces and deoptimization reasons,
arrives with the mechanisms it would observe.

## Follow-up: entry points that take a class object when they want its mixin

`compile_module`, `compile_initializer`, `image::encode_code_table` and
`ns-cli`'s `cmd_disasm` each take a `ClassRef` when the only thing each one
does with it is reach the `Mixin` that backs it — "Keyed by mixin" above is
this repository's own invariant, and none of these four sites addresses code
any other way. Reaching a class object's `mixin` is a read only `heap.rs`
may perform, so each of the four also needs a `&Heap` parameter purely to
make that one hop.

Taking the mixin directly instead of the class object would remove the need
for a `Heap` at every one of these sites — there is nothing else on the
class object any of them reads. The pattern surfaced three separate times
while migrating object and frame access onto the heap seam
(`docs/design/memory.md`, "The seam"): `compile_initializer` and
`compile_module` were each building a throwaway `Heap::new()` internally to
make the same hop, which works only because today's `Heap` is a bare counter
and would silently break under the planned arena; `cmd_disasm` carried the
identical shape a third time. Recorded here as a follow-up rather than
undertaken now, because narrowing four public signatures is a design change
to this module's interface, not a migration.

## Decisions

- **`Instr` is a Rust enum, not packed bytes.** Revisit only on profiling
  evidence.
- **A local's read costs no instruction**, which makes its register live storage
  and makes the value-register invariant necessary.
- **Inlining is gated syntactically at compile time and guarded again at run
  time**, with a fallback to a real send, so virtuality is never actually lost.
- **`whileTrue:` with a non-Boolean condition sends `mustBeBoolean`** rather than
  falling back to a re-send, because a re-send would duplicate the condition's
  side effects and the engines would diverge.
- **Code is keyed by mixin and `InitSlot` is mixin-relative**, because one mixin
  backs many applications with different inherited layouts.
- **Call-site slots are emitted inert from the start**, so wiring up inline caches
  is not a format break.
