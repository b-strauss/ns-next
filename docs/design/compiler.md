# Compiler — AST to bytecode

> **Status:** built, including the reference lowering it is checked against.
> **Code:** `crates/ns-runtime/src/bytecode.rs` (the instruction set and the
> compiled-code shapes), `compiler/mod.rs` (entry points), `compiler/lower.rs`
> (lowering, register allocation, the scope stack), `compiler/control_flow.rs`
> (inlining), `compiler/reference.rs` (the reference lowering). CLI: `ns-next
> disasm`.

This is the producer side. How the bytecode is then run — activations, the VM,
the dispatch loop — is [execution.md](execution.md).

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
  class name has no `Binding`: the compiler resolves the name to its builtin
  class at compile time.
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
- `SendInlinable { selector, receiver_reg, arg_regs, dst, call_site }` — the
  same send, decided exactly as `SendPublic` is, marking one the optimized
  lowering would have inlined. Only the reference lowering emits it, and only
  the depth count reads the mark; see "How the limit counts".
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
  expression. The one layout-dependent instruction in the set; see "One body
  per mixin".

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
shape; `kind` tags which. An activation of it is pushed with
`num_registers - num_args` temporaries.

A compiled body lives on the declaration it belongs to. A method's is on its
`MixinMethod` (`code`); a mixin's instance initializer and its superclass clause's
two bodies are on the `Mixin` (`initializer_code`, `superclass_clause_code`,
`superclass_message_code`) — the clause's because a nested class's superclass
clause is code too, evaluated by the accessor once per enclosing instance, and
its message's because §6.2 evaluates the message's arguments on every
instantiation. While the tree is built from source, `MixinMethod::from_decl`
compiles each method and `compile_mixin_bodies` a mixin's three bodies, before
the mixin is wrapped in the `Rc` every application shares; `image::mixins::decode`
attaches the bodies a row names from the image's code table. So a method without
a body cannot exist, and nothing looks code up beside a declaration: a send that
found a method has found its code. That is how the prior art holds it — Primordial
Soup's `Method::bytecode()`, SOMns's `SInvokable.invokable`. Lowering turns every
resolver binding into an instruction, so no bindings map outlives compilation.

### One body per mixin

One mixin backs every application of it, so a body is compiled **once per mixin**,
stored on it, and shared by every application. §6.2: "they all share their code and
structural definition with the mixin." Storing it per class object would recompile
the same body per enclosing instance, since a class object is per-instance.

**`InitSlot`'s `own_index` is therefore mixin-relative.** An absolute field index
baked into shared code would be wrong the moment a second application inserts
different inherited slots ahead of it. The VM resolves `own_index` against the
executing activation's `slot_base` — the application's `own_slot_base` for that
level — at run time.

A bug here is silent wrong output under one application and correct output under
another. `crates/ns-runtime/tests/vm_initializer.rs` applies one mixin to two
superclasses of different shapes, and initializes a three-level chain, to pin
both.

Compiled initializers are layout-dependent, so a class-shape change must flush
them along with every other cache keyed to the class. See
[liveness-and-reflection.md](liveness-and-reflection.md).

## Registers

Each activation carries `args ++ temps` as one unified register space addressed
by a single `u16`: on the activation stack it is one window of registers, and in
a divorced `Frame` index `< num_args` is `args[index]`, index `>= num_args` is
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

`lower_operand` copies when anything lowered afterwards can write the register.
An assignment through a setter copies the local its value was read from even
when the consumer is the very next instruction, because the setter send itself
runs user code before that consumer reads — code that can write the local
through a closure that captured the frame. `^x := y`, with a setter that assigns
`y`, would otherwise answer `y`'s new value, where the language answers the
value `y` had ([dispatch.md](dispatch.md), "Setter sends").

The copy is skipped only where nothing reads the value at all, and every
position says which it is rather than inferring it: `lower_expr_for` takes a
`ValueUse`, and is the one place that acts on it. An instruction reads the
value of a `^`, of a slot initializer (`InitSlot`), of a block's last statement
(its `ImplicitReturn`) and of an inlined block's last statement — a branch of
`ifTrue:` and its relatives, the block of `and:`/`or:`, a loop condition — when
the inlined form's own value is read; an inlined form in a discarded position
passes that on to its blocks, and a loop body is never read. Every other
statement is discarded, including every statement of a method or an
initializer, which fall off their end answering `self`, and an initializer's
top-level `^`, which returns nothing. Getting this wrong is silent wrong
output with exit status 0, not a crash — which is why it is an invariant in
`CLAUDE.md` rather than a comment.

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
its side effects before the native could report anything — an observable
difference. Instead the natives define the behaviour and the inlined form
reproduces it: both send `mustBeBoolean` to the offending value, an ordinary
virtual send, and answer its result. No class implements `mustBeBoolean`, so the
inlined loop and the native produce the identical `doesNotUnderstand:` through
the shared path, and a user class that does implement it is observed identically
by both.

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
compiles to `NonLocalReturn`, and nothing catches it — the same known gap as a
`^` in a block that runs while its initializer is still running
([execution.md](execution.md), "Non-local return").

### Testing the fallbacks

Coverage has to include the paths the gate is designed to leave alone, not just
the common case: a non-Boolean receiver must still produce the identical
`doesNotUnderstand:` a fully virtual send would; a user class that *implements*
`ifTrue:ifFalse:` must have its method run; a non-Boolean `whileTrue:` condition
must produce the identical `mustBeBoolean` error.

Those paths are pinned by recorded answers, which both lowerings must produce:
`crates/ns-cli/tests/program_outcomes.rs` holds the exit
status, stdout and stderr of the non-Boolean receiver, the non-Boolean loop
condition, a block held in a variable that takes the native loop and a `^` out
of an inlined loop body; `tests-fixtures/control_flow_fallback.ns` runs a user
`ifTrue:ifFalse:`; and the compiler's own tests (`compiler/lower.rs`,
`crates/ns-runtime/tests/compiler.rs`) pin the guarded shape the gate emits.

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

A program-level error the language raises at evaluation time is therefore
**deferred to execution**: it is reported when, and only if, execution reaches
it, so a method that is never run never fails. Two literals carry that deferral:

- `Literal::MalformedInt` — an integer the lexer accepted but the parser rejects,
  such as one out of `i64` range or a fractional radix literal. The compiler
  lowers it to `MalformedInt`; the VM re-runs `parse_int_literal` when the
  instruction executes and raises its error text, and only if execution actually
  reaches it.
- `Literal::UnresolvedIdent`.

## Reading the output

`ns-next disasm <file.ns> <selector>` prints one body's instructions, compiling
only — nothing is instantiated or run — and addressing code through mixins,
since that is what code lives on. It also accepts an artifact in place of a
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

## The reference lowering

The compiler has a second, deliberately naive lowering, `compiler/reference.rs`,
and every program must behave byte for byte the same under both. It is the
differential oracle the retired tree-walker used to be
([execution.md](execution.md), "One engine"), and it is meant to stay. Every
optimization the optimized lowering gains later — registers reused across
statements, a liveness analysis, more inlined selectors, eventually a JIT that
can be compared against both — is checked against a naive baseline that does
not move.

### What it checks, and what it cannot

It checks what the two lowerings do differently: register allocation and
liveness (the class of defect "The value-register invariant" above guards
against), control-flow inlining with its guard and fallback, the lexical-depth
rewrite inside an inlined block, and the lowering of `^` there. Because it is
written independently rather than as a mode of `Lowerer`, it also checks
everything else a lowering decides: cascades, argument order, assignments and
their setter sends, the `^` rule per code kind, how a body ends, the
initializer's shape, and closure compilation.

What it cannot check is anything the two share: the instructions' semantics in
the VM, dispatch, the natives, `InitSlot` addressing, and the definitions both
take from `interpreter/`. Golden tests of exit status, stdout and stderr cover
those. Nor can it catch a misreading of the specification both lowerings were
written from; four of the six silent defects Milestone 3's review found were of
that kind, identically wrong under both engines.

### An independent module, not a mode

`compiler/reference.rs`, written from the AST and the resolver's bindings. It
shares with `lower.rs` only what must have one definition or is purely
mechanical: the bytecode types, `lower_literal` (with its deferred
`MalformedInt`), `selector_of`, the parameter counting, the span helpers,
`instantiate::builtin_kind_from_name`, and `nested::superclass_clause_expr` and
`superclass_clause_message_args_expr` — the one definition of what a superclass
clause evaluates.

A flag inside `Lowerer` was the alternative, and was rejected on two counts. It
would check only the four differences above and leave the rest shared, the
blindness the tree-walker had for the orchestration it shared with the VM. And
every later optimization in `Lowerer` would need a "not in reference mode"
guard, whose omission would silently compare the optimized lowering with
itself — nothing would notice. As a separate module the oracle cannot be
switched off by an optimization. The price is that every new language construct
is lowered twice.

For the same reason, a pass that rewrites code after lowering — a peephole
pass, superinstructions — applies to the optimized lowering's output only and
must never rewrite the reference lowering's. It would change the baseline after
`check_shape` had checked it, and every comparison from then on would be
against code nothing had checked.

### What it emits

1. The named locals keep the layout the resolver assigned — arguments, then
   temporaries — because blocks address their enclosing variables by those
   indices.
2. **Every value gets a fresh register, and no register is reused anywhere in
   the body.** The scratch allocator is not reset at statement boundaries, so
   every scratch register is written by exactly one instruction, and a named
   local is written only by an assignment to it — this body's `StoreLocal` or
   an inner block's `StoreOuterLocal`. A defect in which the optimized lowering
   overwrites a register that is still live cannot happen here, so it shows as
   a divergence.
3. Every read of a local is a `Move` into a fresh register, and every read of
   an outer local a `LoadOuterLocal`. There are no inlined scopes, so the hop
   count is the resolver's depth unchanged.
4. Every send is a real send, receiver then arguments from left to right.
   Nothing is inlined. Every send the optimized lowering inlines — exactly the
   sends `control_flow::recognize` accepts — is a `SendInlinable`, and every
   other explicit-receiver send a `SendPublic` ("How the limit counts" below).
5. An assignment evaluates its value into a fresh register, then stores it or
   sends the setter. The assignment's value is that register, never the local.
6. A cascade evaluates its receiver once into a fresh register and sends every
   message to it; its value is the last send's.
7. A block literal is a `MakeClosure` whose body the reference lowering
   compiles too.
8. `^` follows the code kind alone: `Return` in a method, `NonLocalReturn` in a
   block, evaluate-and-discard at the top of an initializer, `Return` in a
   superclass clause or its message. There is no "inside an inlined block"
   case. A body that falls off its end follows the rules of `finish_body`,
   written again rather than shared.
9. An initializer is each own slot's initializer followed by `InitSlot`, then
   the `initExprs`.
10. `statement_pc`, `call_sites` and `num_registers` follow the same rules as in
    `lower.rs`.

Its `finish` checks that shape in a debug build, which is what the test suite
runs (`check_shape`): no branch instruction, since only inlining emits one;
every scratch register written by exactly one instruction; a named local
written only by `StoreLocal`; and a named local read only by `Move`, every other
operand being a scratch register — rule 3, the property that keeps a value from
changing under its consumer. The ceilings — at most `u16::MAX` temporaries,
block parameters, named locals, registers and instructions in a body — are
checked in every build (`Body::new` and its callers, `fresh`, `emit_send`),
because an index that wrapped would be silently wrong code. A method's or a
factory's parameter count comes from the shared `num_args_of` and
`factory_num_args`, whose narrowing rests on the parser's bound instead. The
optimized lowering is not yet as strict: `lower.rs`'s `alloc_scratch` adds
without a check, and `next_call_site` guards the call-site id with a
`debug_assert!` only, so a body past either ceiling is silently wrong in a
release build. Aligning it with `fresh` and `emit_send` is open.

### Selecting it

`Lowering { Optimized, Reference }` lives in `compiler/mod.rs`, and the compiler
entry points take it. An `Interpreter` is given one when it is constructed —
`Interpreter::bootstrap()` is optimized, `Interpreter::bootstrap_with(Lowering)`
chooses — and `load_module` compiles with it, so one interpreter never mixes the
two. A decoded artifact runs the code it carries. The image does not record
which lowering produced that code, because nothing would read it.

`run`, `snapshot`, `build` and `disasm` take `--lowering <optimized|reference>`,
defaulting to optimized. `disasm` of one method under both is the first tool to
reach for when the oracle fires. `bench` does not take it, since timing the
oracle means nothing, and `load` and `inspect` decode rather than compile.
The switch itself is `LoweringArg` in `crates/ns-cli/src/main.rs`, converted
into `ns_runtime::Lowering` and passed to `source::bootstrap`, which every
compiling command goes through; `disasm` takes it as an `Option` instead and
refuses it for an artifact, which carries the code it was built with.

A command that dropped the switch would compare the optimized lowering with
itself, and no program's output would show it. `crates/ns-cli/tests/lowering.rs`
therefore reads the code instead: `disasm`'s listing, and `inspect --full` of an
artifact from `build` and from `snapshot`, have no branch under the reference
lowering and one under the optimized. `run` writes nothing to read; it is
covered by handing the switch to the same `source::bootstrap` call.

### The harness

- `crates/ns-cli/tests/run_fixtures.rs`, `program_outcomes.rs` and
  `build_load.rs` run every program under both lowerings against the same
  recorded answer. Under the reference lowering `build_load.rs` also takes the
  encoder and decoder over code with no jumps and many registers.
- `run-corpus` runs every row four ways — from source and as a built binary,
  under each lowering — with no switch, because a gate must not be optional. A
  row fails if any of the four differs, and the report names which. The gate's
  counts stay per row.
- `crates/ns-runtime/tests/reference_lowering.rs` loads every corpus module the
  resolver accepts, under both lowerings, compiling without running. It
  exercises "Compilation cannot fail" and the reference lowering's shape
  assertions over the whole corpus rather than over the six modules that run.

Running every program under both lowerings took the suite (`cargo test
--workspace`, debug build) from 6.43 s to 9.09 s: every fixture, golden outcome
and build/load pair runs twice, and `run-corpus`'s rows four ways instead of
two — more process spawns and compiles, not a slower lowering.

While every program the suites run has a recorded answer, "both produce the
recorded answer" is the comparison. Comparing the two runs directly with each
other is needed only once programs without a recorded answer join — generated
ones — which is outside this step.

`ns-runtime`'s integration tests stay on the optimized lowering. They test
dispatch, the natives, instantiation and the image, which the two lowerings
share; the reference lowering has tests of its own in `reference.rs`.

### Keeping it invisible to a program

Nothing the reference lowering does differently may be observable. Two things
were, and each is settled in the design of the aspect it belongs to rather than
by exempting it from the comparison:

- **The identity hash.** Were it drawn from its counter at allocation, the
  closure the reference lowering allocates for every `ifTrue:` the optimized one
  inlines would shift every later object's hash. Identity hashes are lazily
  assigned, as in Cog/Spur (`hashBitsOf:`, which calls `newHashBitsOf:` when the
  bits are 0) and Primordial Soup (`Object_identityHash`, which sets
  `header_hash` when it is 0): an object draws its number from the counter when
  it is first asked for one. That makes allocation order unobservable in
  general, which closure conversion and a JIT's escape analysis need as well.
  See [object-model.md](object-model.md).
- **The recursion-depth limit**, below.

Everything else was checked and agrees: all eight inlined forms exist as natives
that follow the rules the inlined code reproduces (`builtins::boolean::truthy`
for `and:` and `or:`, `mustBeBoolean` for a non-Boolean loop condition); a
method's `^` inside a block the optimized lowering inlines is a `Return` there
and a `NonLocalReturn` here, whose unwind ends at the method's activation, with
the same result, and
`cannotReturn` and the initializer gap behave alike; and a real block nested
inside such a block has one more frame in its chain here, which holds no
variables, since the inlining gate admits none, and the same `home`. A
snapshot's bytes differ by construction, because its code table holds different
code: what is compared is program outcomes, never artifacts.

### How the limit counts

The trampoline makes recursion depth an ordinary Newspeak error
([execution.md](execution.md), "The trampoline"). A limit that counted
every activation would trip at different depths under the two lowerings, because
the reference lowering activates blocks the optimized one inlines:
`n = 0 ifTrue: [0] ifFalse: [n * (self fact: n - 1)]` costs one activation per
level optimized and two under the reference lowering.

**The limit counts every activation on the activation stack — method, block,
initializer level, superclass clause and message — except a block activation
that a builtin control-flow native performs for a `SendInlinable`.** The natives
are `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` and
`or:` on a Boolean, and `whileTrue:` and `whileFalse:` on a block.
`SendInlinable` is an ordinary public send that only the reference lowering
emits, for exactly the sends `control_flow::recognize` accepts.

- **The count agrees at every point of every program.** The optimized lowering
  omits exactly these activations and no others. On a non-Boolean receiver both
  lowerings make the same real send with real closures, and from there count
  alike; the `mustBeBoolean` send is made alike too.
- **Depth stays bounded.** A literal block can run again only if the body
  enclosing it runs again, so every recursive cycle contains a counted
  activation, and the uncounted activations per counted one are bounded by the
  source's static nesting depth — at most 128, the parser's ceiling, so the
  stack holds at most 129 entries per counted activation
  ([execution.md](execution.md), "Two bounds, not one").
- **The optimized lowering pays nothing** beyond the counter every rule needs,
  since it never emits `SendInlinable`.
- **A crafted artifact cannot use the exemption to recurse without bound.**
  Code decoded from an artifact can emit `SendInlinable` with any block in the
  argument register — `b := [true ifTrue: b]` — and an exemption granted by
  selector alone would let that recursion run uncounted until memory ran out.
  So the block must also have been created by the activation that sends: its
  `captured_frame` is that activation's reified frame. Every block the
  reference lowering passes to a `SendInlinable` is a literal created right
  before the send, so the condition holds for every program either lowering
  compiles, and a chain of exempt activations is then bounded by how deeply
  blocks nest in the artifact — which the decoder caps at the parser's
  ceiling of 128 ([image-format.md](image-format.md), "Verification"), so an
  artifact stacks no more exempt activations above a counted one than source
  can. A block that fails the condition is counted like any other.

So the oracle compares even an overflow byte for byte — the output up to it,
the error and the exit status. Two alternatives were rejected. Counting
every activation and exempting an overflow from the comparison, as a JVM leaves
the depth of a `StackOverflowError` to frame sizes and compilation tier, would
leave every overflow unchecked and report false divergences for a recursion
that fits under one lowering and not the other. Counting only method-like
activations would agree under both, but would leave a recursion through blocks
alone — `b := [:n | n = 0 ifFalse: [b value: n - 1]]` — to a second, memory-based
bound that does not agree. Of the prior art, the Smalltalk lineage bounds depth
by memory and Nova by a fixed count of contexts ([execution.md](execution.md),
"How other systems bound recursion depth"); none has two lowerings that must
agree, so this rule is ns-next's own.

**Where it is decided.** The block arm of `Interpreter::push_step`
(`crates/ns-runtime/src/interpreter/mod.rs`) is the one place an activation is
exempted. Every other push is counted by construction: `push_step`'s method arm,
an initializer level's (`run_own_initializer`) and a superclass clause's or
message's (`nested::run_clause_body`) each record `counted: true`. The flag
rides on the stack entry, and `Interpreter::push` refuses a counted push at the
limit. A block is exempt when it carries
`StepArgs::ControlFlow` — the arguments only the control-flow natives build —
the send that led to it was a `SendInlinable`, and its `captured_frame` is the
sending activation's reified frame. The VM decides a `SendInlinable` exactly as
it does a `SendPublic` and only hands `push_step` the fact that it was one. A
native loop's next block is pushed by `Interpreter::push_continuation`, which
exempts it only if the one it replaces was; the VM forwards the popped entry's
flag and decides nothing. `&` and
`|` share `and:`'s and `or:`'s native, so their block carries the mark as
well, but no lowering emits a `SendInlinable` for them, since `recognize` does
not accept them ([execution.md](execution.md), "Two bounds, not one").

**Where it is pinned.** `crates/ns-runtime/tests/trampoline.rs` holds the
boundary under both lowerings: `down:`, `logic:` and `loop:` — a plain
recursion, one through an `or:` block and one through a `whileTrue:` body, the
last two inlined by one lowering and activated, exempt, by the other — run at
99,999 and fail at 100,000, leave the activation stack empty, and leave the
interpreter usable, with the counted depth back at 0. A recursion through
`value` alone ends in the same error, and so does a crafted artifact whose
self-recursive `ifTrue:` is rewritten into a `SendInlinable`, which the
captured-frame condition counts — whether or not the sending block's own
activation is reified, so the condition cannot be weakened to "the sender has
a frame". The control-flow natives sent from Rust, where their continuation
runs at the floor of the run, answer what they should and leave both depths at
0. The golden
outcomes `deep-stack` and `unbounded-recursion`
(`crates/ns-cli/tests/program_outcomes.rs`) pin a thousand-deep recursion
through an `ifFalse:` block and the overflow's output, exit status and error
text, under both lowerings.

`SendInlinable`, wire tag 20, arrived with the trampoline, together with the
counter that is its only consumer: without the counter it would mean exactly
`SendPublic`, and nothing could test it. The limit is 100,000
(`MAX_ACTIVATION_DEPTH`), its error text is `stack overflow: recursion too
deep`, and the control-flow natives are trampoline steps, so under neither
lowering does a block they activate cost native stack
([execution.md](execution.md), "The trampoline").

### Status

Built, in the four commits of its
[plan](../superpowers/plans/2026-09-12-1157-ns-next-reference-lowering.md):
lazy identity hashes, `compiler/reference.rs` with `Lowering`, the CLI switch,
and the harness. Every recorded program answers alike under both lowerings.
Probing with it beyond the recorded programs found one defect in the optimized
lowering, since fixed and pinned by a golden outcome: an assignment through a
setter could answer the new value of the local it read ("The value-register
invariant"). What "How the limit counts" needs — `SendInlinable` and the depth
counter that is its only consumer — arrived with the
[trampoline](../superpowers/plans/2026-09-12-1619-ns-next-trampoline.md).

## Follow-up: entry points that reach a class object only to find its mixin

The compiler itself no longer has such an entry point: it compiles onto a
`Mixin` while the tree is built, and never sees a class object. The pattern
remains in two places. `image::encode_code_table` and `image::mixins::plan` each
take a `ClassRef` parameter, and each one's first move is to walk from it to the
`Mixin` that backs it — the code they encode lives on that mixin's tree and
nowhere else. Reaching a class object's `mixin` is a read only `heap.rs` may
perform, so each also needs a `&Heap` parameter purely to make that one hop.
Taking the root mixin directly instead would remove the need for a `Heap` there —
there is nothing else on the class object either of them reads.

`ns-cli`'s `disasm` (`crates/ns-cli/src/commands/disasm.rs`) has the identical
shape but not the identical fix: it takes no `ClassRef` parameter to narrow,
since it builds a class object itself
(from source, or from a loaded artifact) and reaches the mixin through its own
`root_mixin_of` helper, needing a `Heap` for the same reason. It reads through
`interp.heap_ref()` inside the lifetime of the interpreter that built the class
object, because under the arena it must: a handle is an index meaningful only
against its own heap, so a stand-in `Heap` reads the wrong object in a release
build and trips the debug origin guard in a debug one. Its fix is not "take a
`Mixin` instead of a `ClassRef`" but "stop building a class object at all" — a
different, and possibly larger, change.

Recorded here as a follow-up rather than undertaken now, because narrowing the
signatures is a design change to the image module's interface, not a migration.

## Decisions

- **`Instr` is a Rust enum, not packed bytes.** Revisit only on profiling
  evidence.
- **A local's read costs no instruction**, which makes its register live storage
  and makes the value-register invariant necessary.
- **Inlining is gated syntactically at compile time and guarded again at run
  time**, with a fallback to a real send, so virtuality is never actually lost.
- **`whileTrue:` with a non-Boolean condition sends `mustBeBoolean`** rather than
  falling back to a re-send, because a re-send would duplicate the condition's
  side effects and the inlined loop would diverge from the native one.
- **Code lives on the mixin and `InitSlot` is mixin-relative**, because one mixin
  backs many applications with different inherited layouts.
- **Call-site slots are emitted inert from the start**, so wiring up inline caches
  is not a format break.
- **The reference lowering is an independent module, not a mode of `Lowerer`**,
  so that it checks everything a lowering decides and no optimization can switch
  it off by forgetting a guard.
- **The recursion-depth limit exempts exactly the activations inlining
  removes**, marked by `SendInlinable`, so that it counts the same under both
  lowerings and the oracle compares an overflow too.
