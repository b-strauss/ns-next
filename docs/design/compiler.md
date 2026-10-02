# Compiler — AST to bytecode

> **Status:** built, including the reference lowering it is checked against.
> **Code:** `crates/ns-runtime/src/bytecode.rs` (the instruction set and the
> compiled-code shapes), `compiler/mod.rs` (entry points), `compiler/lower.rs`
> (lowering, register allocation, the scope stack, the converted register
> layout), `compiler/capture.rs` (the capture analysis closure conversion runs
> ahead of the optimized lowering), `compiler/control_flow.rs`
> (inlining, and `recognize_mirror_send`), `compiler/primitive_method.rs`
> (which kernel methods are primitive methods), `compiler/reference.rs` (the
> reference lowering). CLI: `ns-next disasm`.

This is the producer side. How the bytecode is then run — activations, the VM,
the dispatch loop — is [execution.md](execution.md).

## The instruction set

`Instruction` is a plain Rust enum dispatched by `match`, not hand-packed bytes.
"Architecture-neutral and versioned" is a property of the *image encoding* of
compiled code — a tag byte and a fixed operand layout per instruction, with the
tag numbers fixed for good; see [image-format.md](image-format.md) — not a
requirement on the in-memory representation. Manual opcode packing is a technique
that pays off once a JIT or a very hot interpreter loop demands it. The profile
of the loop over locals found that point: every dispatch decodes the enum's
discriminant through a niche, and `program_counter` and the code pointer are
spilled to `vm::run`'s stack frame ([execution.md](execution.md), "Where the 3%
goes"). A compact encoding is planned, designed against the one-word value
([execution.md](execution.md), "Planned: the compact instruction encoding").

Every register operand is a `u16`; every jump target is an instruction index. The
set follows the resolver's `Binding` variants and the lookup shapes directly, so
that lowering is mostly a transliteration rather than a translation.

**Loads and moves**

- `LoadLiteral { destination, literal }` — the literal is already lowered:
  `Nil | Boolean | SmallInteger | Float | String | Symbol | LargeInteger |
  UnresolvedIdentifier`, with integer and float text parsed at compile time
  rather than at each execution. `LargeInteger` carries an integer literal
  beyond a `SmallInteger`'s 61 bits as its digits (an `ArbitraryInteger`);
  `UnresolvedIdentifier` is the deferred case, see "Compilation cannot fail".
- `LoadSelf { destination }`.
- `LoadEnclosingObject { destination, depth }` — `outer N` with no message
  after it, `Binding::EnclosingObject`: the kth enclosing object, found as a
  `SendLexical` finds its receiver, with no send.
- `Move { destination, source }`.
- `StoreLocal { source, slot }` — a `Binding::Local` **write**. A read needs no
  instruction at all: **the local's register *is* its value.** That is the fact
  the value-register invariant below depends on.
- `LoadOuterLocal { destination, depth, index }` / `StoreOuterLocal { source, depth, index }`
  — `Binding::OuterLocal`, walking the `captured_frame` chain exactly as
  `frame_at_depth` does.

A bare predefined class name has no load of its own. The resolver binds a
name no enclosing declaration declares `LexicalSend { depth: 0 }`, so `String`
lowers to `SendLexical` of `String` at depth 0 — `self String`, answered by
`Object`'s accessor ([dispatch.md](dispatch.md), "Binding variants") — in a
method body and in a nested class's superclass clause alike, under both
lowerings. `LoadBuiltinClass`, which loaded the class as a constant and
ignored a member of that name, is retired, and so is its wire tag 2
([image-format.md](image-format.md)).

**Sends** — each carries a `call_site`; see "Safepoints and call-site
identity".

- `SendPublic { selector, receiver_register, argument_registers, destination, call_site }` — an ordinary
  explicit-receiver send, resolved with `lookupPublic`.
- `SendInlinable { selector, receiver_register, argument_registers, destination, call_site }` — the
  same send, decided exactly as `SendPublic` is, marking one the optimized
  lowering would have inlined. Only the reference lowering emits it, and only
  the depth count reads the mark; see "How the limit counts".
- `SendLexical { depth, selector, argument_registers, destination, call_site }` — `Binding::LexicalSend`.
  **No receiver register**: the receiver is `self` by construction, and the
  kth-enclosing object is computed at run time.
- `SendSuper { selector, argument_registers, destination, call_site }` — receiver is `self`, lookup
  starts at the current activation's `current_class`'s superclass.
- `SendMirror { primitive, mirror_register, subject_register, argument_registers, destination, failure }`
  — a send to the VM mirror whose failure block is lowered inline at
  `failure`: it runs the mirror row `primitive`, bound at compile time, with
  no lookup and no call site, and jumps to `failure` when the leaf declines.
  Emitted only into `Kernel.ns`, which no image carries, so it has **no wire
  tag**; see "Mirror sends".

**Construction**

- `MakeClosure { destination, parameters, code, copied }` — `code` is the block
  body's own compiled `CompiledCode`, nested inside the enclosing code; nested
  block literals nest again. `copied` names the registers of *this* body whose
  values the new closure is handed, in the order the block's own registers take
  them ([execution.md](execution.md), "Closure conversion"); it is empty under
  the reference lowering and for a fallback closure. `captured_frame` is a
  run-time value the VM supplies from its current activation, never an operand,
  and a closure's `home` is derived from it rather than stored.
- `MakeVector { destination, size }`, `LoadVector { destination, vector, index }`
  and `StoreVector { vector, index, source }` — a body's indirection vector,
  the `Array` its captured-and-written locals live in, and one slot's read and
  write. Emitted by the optimized lowering only, `MakeVector` once as the
  body's first instruction. A vector's size and kind are run-time values, so
  `LoadVector` and `StoreVector` check them and raise an ordinary error rather
  than trusting the register ([execution.md](execution.md), "Converted
  representation").
- `MakeArray { destination, element_registers }`.
- `InitializeSlot { source, own_index }` — writes one of `self`'s own instance slots.
  Emitted **only** by initializer compilation, for a slot's own initializer
  expression, and by a lazy slot's getter (§6.3.3). One of the two
  layout-dependent instructions in the set; see "One body per mixin".
- `ReturnSlotIfFilled { own_index }` — a lazy slot's getter's first
  instruction: returns `self`'s own slot at `own_index` from the method when it
  holds anything but `nil`, and falls through to the initializer otherwise.
  Addressed as `InitializeSlot` is; the other layout-dependent instruction.

**Leaving a body**

- `Return { register }` — a `^` in a method or initializer body.
- `ImplicitReturn { register }` — a body that fell off its end: a method returns
  `self`, a block its last expression's value. The VM treats it exactly like
  `Return`; it is kept distinct so `disasm` output stays legible.
- `NonLocalReturn { register }` — a `^` inside a block body.

**Branches**

- `Jump { target }`, `BranchIfFalse { register, target }`, `BranchIfTrue { register, target }`,
  `JumpIfNotBoolean { register, target }`, `IntegerBinary { operation, destination, left, right, target }`.

**Nothing but control-flow inlining, arithmetic inlining and a mirror send's
inlined failure block emits a branch.** Every other construct in the language
is a send, so a jump in the instruction stream is always the trace of one of
the three inlining decisions; `SendMirror` is a branch too, to its failure
block ("Mirror sends").
`JumpIfNotBoolean` is the control-flow guard: since `true` and `false` are
immediates, "is the receiver's class exactly `True`/`False`?" is precisely
"is this register a `Boolean` at all". `IntegerBinary` is both the arithmetic
guard and the operation itself: it computes in place and falls through on
success, or jumps to `target` -- the ordinary send -- when either operand is
not a `SmallInteger` or the operation declines ("Arithmetic inlining" below).

## Compiled code

```rust
CompiledCode {
    kind: CodeKind,           // Method | Block | Initializer
    instructions: Vec<Instruction>,
    argument_count: u16,
    register_count: u16,       // arguments + copied values + named temporaries
                               // + the vector register + scratch
    copied_count: u16,         // values a block's activation is handed, between
                               // its parameters and its temporaries; 0 otherwise
    outer_reach: u16,          // how many frames out this body or a block it
                               // creates can read or write by reference
    statement_program_counter: Vec<(usize, Range<usize>)>,   // safepoints
    family_size: u32,          // one past the highest call-site number in this body and its blocks
    source: Span,
}
```

A method body, a block body and an instance initializer all compile to the same
shape; `kind` tags which. An activation of it is pushed with
`register_count - argument_count` temporaries, and a block's activation is
then handed its closure's `copied_count` values into the registers just above
its parameters.

`outer_reach` is derived from the instructions, by the compiler and by the
image decoder alike (`bytecode::outer_reach_of`), and decides whether the
activation that creates a closure keeps its registers when it returns
([execution.md](execution.md), "Converted representation").
`copied_count` is on the wire; `outer_reach` and `family_size` are not.

A method's or a block's own statements are its local slot initializers first,
one per declared local that has one, each `initializer_expression; StoreLocal`
into the slot's own register, in declaration order (§3.6), then its
`CodeBody.statements` -- the same order an instance initializer's own slot
initializers precede its `initExprs` ("One body per mixin" below). A local
with no initializer is skipped: its register keeps the **nil** every register
starts with.

A compiled body lives on the declaration it belongs to. A method's is on its
`MixinMethod` (`code`); a mixin's instance initializer and its superclass clause's
two bodies are on the `Mixin` (`initializer_code`, `superclass_clause_code`,
`superclass_message_code`) — the clause's because a nested class's superclass
clause is code too, evaluated by the accessor once per enclosing instance, and
its message's because §6.2 evaluates the message's arguments on every
instantiation. A declaration that applies mixins carries one more message
body per `<:` clause, on that clause's `MixinApplicationClause`
(`message_code`), compiled exactly as the superclass clause's message is —
kind `SuperclassMessage`, in the declaration's own factory scope and with its
factory's arity, since §6.2 evaluates every clause message with the applying
class's factory arguments — and its superclass clause body answers the
superclass and every `<:` receiver as one array
([object-model.md](object-model.md), "Mixin application"). While the tree is built from source, `MixinMethod::from_declaration`
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

**`InitializeSlot`'s `own_index` is therefore mixin-relative.** An absolute field index
baked into shared code would be wrong the moment a second application inserts
different inherited slots ahead of it. The VM resolves `own_index` against the
executing activation's slot base — the own `own_slot_base` of its
`current_class`, which for an initializer is the level it initializes and for a
lazy slot's getter its defining class (`Heap::activation_slot_base`) — at run
time.

A bug here is silent wrong output under one application and correct output under
another. `crates/ns-runtime/tests/vm_initializer.rs` applies one mixin to two
superclasses of different shapes, and initializes a three-level chain, to pin
both.

Compiled initializers are layout-dependent, so a class-shape change must flush
them along with every other cache keyed to the class. See
[liveness-and-reflection.md](liveness-and-reflection.md).

## Registers

Each activation carries `arguments ++ temporaries` as one unified register space addressed
by a single `u16`: on the activation stack it is one window of registers, and in
a divorced `Frame` index `< argument_count` is `arguments[index]`, index `>= argument_count` is
`temporaries[index - argument_count]`. That is exactly the args-then-temps split the
resolver's `Binding::Local` already used, so the compiler needs no separate
renumbering, and a block's `OuterLocal` indices are untouched by the scratch
registers a bytecode frame adds — scratch sits after the named locals.

**A converted block's layout inserts its copied values between the two.** Under
the optimized lowering a block's registers are its parameters, then the values
it was handed when the closure was created, then its temporaries, then one
hidden register holding its indirection vector when it has one, then scratch; a
method's, an initializer's or a clause's are the same without the copied values
([execution.md](execution.md), "Closure conversion"). A resolver index is no
longer a register index, so `Layout` (`compiler/lower.rs`) is the one place
that maps one to the other: `local_register` pushes every temporary up by the
copied count, and a local the capture analysis put in the vector keeps its
resolver index while its register simply stays nil, so every `Binding` stays
valid. A fallback closure and anything nested in it get a by-reference
`Layout` instead — no copied values, no vector, every local in the register its
resolver index names — because such a closure reaches the frame whose common
path it stands in for by reference.

Registers above the named locals are compiler-allocated scratch from a bump
allocator **reset to the first scratch index at the start of every statement**.
Statements are the unit `program_counter` and safepoints already work at, so scratch never
needs to stay live across a statement boundary. There is no cross-statement
liveness analysis, and reusing registers within one statement's expression tree is
permitted but not done: Newspeak method bodies are not deeply nested in practice
and `temporaries` is a plain growable `Vec`, so a few extra live entries per statement
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
An assignment through a setter copies the local its value was read from — when
anything can write that local at all, the next paragraph — even
when the consumer is the very next instruction, because the setter send itself
runs user code before that consumer reads, and that code can write the local
through a closure. `^x:: y`, with a setter that assigns `y`, would otherwise
answer `y`'s new value, where the language answers the value `y` had
([dispatch.md](dispatch.md), "Setter sends").

**Which sends can do that is now a question about the register, not about the
send.** Closure conversion left exactly two ways user code can write a named
local of a running activation: an assignment lowered into that same body,
including inside an inlined block, and — for a local some inlinable branch of
the body assigns — the fallback closure that branch's guard failure creates,
which any send may have kept and may run at any later time. A converted closure
writes no register of another activation at all: a captured written variable
lives in the indirection vector, whose read lands in a scratch register as a
snapshot, and a copied register is read-only by construction. So the lowering
asks `a_send_can_write(register)` per register rather than treating every send
as a write, and `writes_register` walks an expression without descending into
real block literals ([execution.md](execution.md), "Fewer protective copies").
The reference lowering is unaffected: it reaches everything by reference and
copies every read of a named local unconditionally, which `check_shape` asserts
on every body it emits ("What it emits" below). A mistake in the narrower rule
is silent wrong output at exit status 0, so the reference lowering is the
oracle for it and `goldens/language/block/` pins the cases by hand —
`a_fallback_closure_writes_a_local_a_send_reads.ns` for the one register shape
`a_send_can_write` answers `true` for, read as an operand, as an argument and as
a setter assignment's value.

The copy is skipped only where nothing reads the value at all, and every
position says which it is rather than inferring it: `lower_expression_for` takes a
`ValueUse`, and is the one place that acts on it. An instruction reads the
value of a `^`, of a slot initializer (`InitializeSlot`), of a block's last statement
(its `ImplicitReturn`) and of an inlined block's last statement — a branch of
`ifTrue:` and its relatives, the block of `and:`/`or:`, a loop condition — when
the inlined form's own value is read; an inlined form in a discarded position
passes that on to its blocks, and a loop body is never read. Every other
statement is discarded, including every statement of a method or an
initializer, which fall off their end answering `self`, and an initializer's
top-level `^`, which returns nothing. Getting this wrong is silent wrong
output with exit status 0, not a crash — which is why it is an invariant in
`CLAUDE.md` rather than a comment.

**A read-only local is never copied.** A setter send passes by every local
that declares no setter ([dispatch.md](dispatch.md), "Setter sends"), so a
parameter's register and an immutable local's are written by their
initialization and by nothing after it -- no assignment binds to them, in
this activation or through a closure. The copy guards against a write that
can then no longer happen, so `materialize` skips it for such a register,
whatever `rest` holds; parameters are the commonest operands there are.
`Lowerer` tracks which registers are read-only (`read_only_locals`, built in
`Lowerer::new` and indexed by **register**): every parameter, every immutable
local, and, after closure conversion, every copied register, the vector
register and the now-unused register of a variable that lives in the vector.
`lower_assign`'s `debug_assert!`s cover every write into a
named local, this frame's own or an enclosing one's: the two `StoreLocal`
branches check a write into the *current* frame's own registers
(`Binding::Local`, and `Binding::OuterLocal` at `hops == 0`), and the
`StoreOuterLocal` branch (`hops != 0`) checks a write from a nested block
into an *enclosing* frame's register the same way, against that frame's own
`read_only_locals` table; `check_writable` carries both, and skips a vector
slot, which is writable by construction because only a variable something
writes is ever put in a vector. `lower_block_literal` hands each nested
`Lowerer` the `read_only_locals` of every `Frame` scope enclosing it
(innermost last), and the `Layout` of each, alongside the scope stack it
already passed down, so a `StoreOuterLocal` at any depth can check the same
precondition the same-frame case always could. In every case the resolver
never binds an assignment to a parameter or an immutable local, so the only
write to a read-only local's register is its own initialization, a statement
of its own that no expression's `rest` can contain.

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
([execution.md](execution.md), "Unwinding").

### Testing the fallbacks

Coverage has to include the paths the gate is designed to leave alone, not just
the common case: a non-Boolean receiver must still produce the identical
`doesNotUnderstand:` a fully virtual send would; a user class that *implements*
`ifTrue:ifFalse:` must have its method run; a non-Boolean `whileTrue:` condition
must produce the identical `mustBeBoolean` error.

Those paths are pinned by recorded answers, which both lowerings must produce:
`goldens/language/control-flow/` holds the non-Boolean receiver and the non-Boolean loop
condition and a block held in a variable that takes the native loop,
`goldens/language/block/` a `^` out of an inlined loop body;
`goldens/language/control-flow/control_flow_fallback.ns` runs a user
`ifTrue:ifFalse:`; and the compiler's own tests (`compiler/lower.rs`,
`crates/ns-runtime/tests/compiler.rs`) pin the guarded shape the gate emits.

## Arithmetic inlining

The same idea as control-flow inlining, applied to the nine binary integer
primitives: `+`, `-`, `*`, `//`, `<`, `>`, `<=`, `>=` and `=`. A send of one
of them is worth an instruction of its own because a leaf send is the
cheapest kind of send there is — no user method body to run, nothing to do
but read two registers and answer — so its activation is almost all of its
cost. Cog makes the arithmetic and comparison selectors bytecodes of their
own — `bytecodePrimAdd` and its neighbours — for the same reason, and
[core-library.md](core-library.md) names it as this step's model.

**The step was sequenced ahead of the core library's move to a VM mirror**
([core-library.md](core-library.md)). With the library Newspeak over a
mirror, `Integer>>+` is a kernel method, and every arithmetic send the fast
path does not absorb costs a send to it; without the fast path, moving the
library to Newspeak would have paid that on every one. See
[execution.md](execution.md), "What leaf inlining moved", for what it
measured: every one of the eight benchmark sources 11.5% to 31.0% faster.

**Recognition is syntactic and shares control-flow inlining's gate.**
`recognize_integer_binary` (`compiler/control_flow.rs`) answers an
`IntegerOperation` for a binary selector in the list and `None` otherwise;
it sits beside `recognize`, behind the same `is_ordinary_send`, so a `super`
or lexical send is never inlined — their lookups do not start where an
explicit-receiver send's does. Nothing about the operands is inspected:
whether they are integers is a run-time question, and asking it at compile
time would be a type inference this compiler does not do. `/` is not in the
list because an integer has no `/` primitive at all.

**The emission is a guarded fast path with the ordinary send as its
fallback**, the shape `lower_integer_binary` emits:

```
IntegerBinary <operation> dest <- left, right   else -> fallback
Jump -> after
fallback:
SendPublic dest <- left #<selector> (right)
after:
```

The instruction computes only when both operands are `SmallInteger`s and,
for the four arithmetic operations, when the result is one; on either
failure it branches to the fallback. It computes on the tagged words, with
one test of both operands' tag and one overflow test, through the methods
on `Value` the integer leaves share (`small_integer_sum` and its siblings,
[object-model.md](object-model.md), "Values and objects"). The five
comparisons cannot fail once the operands are integers. So a non-integer
operand, a result that does not fit 61 bits and a zero divisor all leave by
the same door, and what they reach is the send the compiler would have emitted
anyway.

**That is the point of the fallback, and the reason it is a real send rather
than an error raised in the VM.** What the fast path declines is decided in
`builtins/numbers.rs` and stays there: the promotion of a result that does
not fit 61 bits to a `LargeInteger` (§5.1.1, [core-library.md](core-library.md),
"Large integers"), and every error message an integer operation can produce
— `doesNotUnderstand:`, division by zero, a type mismatch. The VM arm never
allocates and never raises anything. A second copy of those
errors in `vm.rs` is exactly the drift `CLAUDE.md`'s "`vm.rs` holds no
dispatch logic" exists to prevent, and it would have to be kept in step with
the reference lowering, which compiles the plain send and reaches the
builtin directly. Because the fallback is an ordinary `SendPublic` it also
observes a user class that implements `+` exactly as an uninlined send
would — the same virtuality-preserving argument control-flow inlining
makes.

The fallback carries a call site of its own, so a program that takes it
repeatedly still gets an inline cache there. The fast path carries none: it
is not a send and decides nothing.

**A known cost, not yet paid down.** A *successful* operation executes the
`Jump` past the fallback as well, so the hot path is two dispatches where an
out-of-line fallback block would leave one. It did not show above the noise
in the measurement ([execution.md](execution.md), "What leaf inlining
moved"), and moving the fallback out of line is a follow-up rather than
something the numbers demand.

**Planned: inverting the branch sense to drop the `Jump`.** An out-of-line
fallback block is not the cheaper form -- it would make `statement_program_counter`
ranges non-contiguous. The cheaper form inverts what `target` means:
`IntegerBinary` would jump to `target` on *success*, landing past the send,
and fall through into the immediately following `SendPublic` when it
declines. `vm::run` polls a safepoint only on a backward jump (`target <=
program_counter`), so the forward jump costs no safepoint, and the hot path
becomes one dispatch with no `Jump` instruction at all. **This requires
retiring tag 33 and taking the next free instruction tag (40 today, per
`image/opcodes.rs`) for the changed instruction**, because the
meaning of `target` changes and a stale artifact would otherwise decode into
a different, perfectly valid program and run it silently
(`CLAUDE.md`, "A wire tag is assigned once and never renumbered or reused").
`patch_target`, `finish`, `jump_target`, `check_shape` and `no_branch` need
no change, because the field itself is unchanged.

**Testing.** The paths the guard declines are pinned by recorded answers,
since a fast path that quietly answered where the primitive would have
raised passes every other test there is.
`goldens/library/number/integer/integer_leaves_fast_path_and_fallback_agree.ns` runs
the fast path beside the two ways the guard declines on ordinary values —
a `Float` receiver and a non-integer argument to `=` — and floor division on
the fast path;
`goldens/library/number/large-integer/an_overflowing_integer_operation_answers_a_large_integer.ns`
pins the promotion the fallback carries, and `goldens/library/number/integer/` holds `//` by
zero, which the primitive turns into an error
(`inlined_integer_floor_division_by_zero_is_uncaught.ns`), with the
primitive's own message as the recorded `stderr`. The reference lowering
compiles every one of these as a plain send and is held to the same recorded
answers, which is what makes it the oracle for this step.

## Mirror sends

Every method of the core library is a send to the VM mirror whose last
argument is a literal failure block — `^vmMirror stringSize: self ifFail:
[^super size]` ([core-library.md](core-library.md), "The shape of a kernel
method"). Compiled as an ordinary send, that block would be a closure built on
every call and activated only when the leaf declines. The optimized lowering
instead lowers it inline, the way an inlined conditional's blocks are, behind
an instruction that runs the leaf: `Instruction::SendMirror`.

**Recognition is `control_flow::recognize_mirror_send`**, which answers the
mirror row and the failure block for a send that is:

- marked `Binding::MirrorSend` — which only `resolver::resolve_kernel`
  records, on a plain explicit-receiver send whose receiver is the bare
  identifier `vmMirror` naming the module root's member, so only shipped
  core-library source (`Kernel.ns`, `PlatformFactory.ns`) ever has one (a cascade to `vmMirror` is not marked, and compiles as
  ordinary sends);
- a keyword send whose selector names a `MIRROR` row
  (`builtins::primitive::mirror_primitive_named`); and
- ending in a literal block the inlining gate would accept, with no
  parameters and no temporaries.

Anything else carrying the mark compiles as an ordinary send to the mirror
object, which still runs the row, sent ([dispatch.md](dispatch.md), "A
primitive can fail") — so a kernel source defect the recognizer does not
accept is a slower send, never a compile error. A test checks that every
send to `vmMirror` in `Kernel.ns` carries the mark.

**Three places ask it, the same three that ask `recognize`**, so the lowering
and the capture analysis agree about which block literals are closures:
`lower_send`, which lowers it; `send_writes_register`, the protective-copy
walk, for which the block's body is lowered into this one and can write its
registers like any inlined block's; and `capture::Walker::send`, for which
the block is an inlined scope and no closure. The block is not
`conditional` there: that flag says a fallback closure built from the block
can write this frame later, and a `SendMirror` builds none. Each asks
`is_ordinary_send` first, as it does for `recognize`, although a
`MirrorSend` is never on a `super` or lexical send. Outside the compiler,
`compiler::primitive_method` asks it too, to recognize a primitive method
([dispatch.md](dispatch.md), "Primitive methods"), and the reference
lowering asks it, below.

**The lowering** (`lower_mirror_send`):

```
SendMirror dest <- mirror #<row> (subject, arguments)   else -> failure
Jump -> after
failure:
<the failure block's body, lowered inline>
Move dest <- <its value>
after:
```

The receiver, the subject and the leaf's arguments are lowered in order, each
protected against what is lowered after it, as a send's arguments are. The
block is code at `failure`, not a value: it runs only when the leaf declines,
its value lands in the same destination the leaf writes, and a `^` in it
returns from the kernel method — which is what the failure path of a
primitive method's body should do. Unlike an inlined conditional there is no
fallback send: the mirror's class and method table are fixed at bootstrap
([dispatch.md](dispatch.md), "Primitives"), so nothing but this block can
answer the leaf's failure, and no closure is ever built out of it.

`SendMirror` carries the row, bound at compile time, and no call site: there
is no lookup to cache. The VM arm reads the registers, calls
`interpreter::mirror::send_inlined`, and either jumps to `failure` or
delivers the leaf's answer — a value, or a step such as a block to activate —
exactly as a send's decision is delivered. Whether the receiver register
holds the mirror is `send_inlined`'s check, standing in for the lookup; only
shipped core-library source (`Kernel.ns`, `PlatformFactory.ns`) has a `SendMirror`,
no program reaches either module's root, and the bootstrap checks that each one's
`vmMirror` holds the mirror, so the check's error is unreachable while that
holds. The
instruction reads and writes only its own activation's registers, so
`bytecode::outer_reach_of` does not name it.

**It has no wire tag**, and the encoder treats one as unreachable. It is
emitted only for a `MirrorSend`, so only into shipped core-library source,
and no image carries shipped code: an image names a builtin class by tag and a
shipped object by its platform path, and the bootstrap of the loading process
supplies both ([image-format.md](image-format.md), "What is
*not* serialized"). A tag would be a wire form nothing legitimate writes, and
a decoder that accepted one would hand an artifact a way to run any leaf with
no lookup.

**The reference lowering emits the ordinary send**, with a real closure for
the block, and marks it `SendInlinable` when `recognize_mirror_send` accepts
it, as it marks every send the optimized lowering inlines. So the failure
block's activation under the reference lowering — `execute_sent` answers it
as a control-flow block — is exempt from the depth count under the same
condition as `ifTrue:`'s, which is what the optimized lowering, never
activating it, needs ("How the limit counts" below). Because the mark covers
the whole send, it would exempt a literal block the leaf itself activates
just as well; so `Kernel.ns` passes a literal block only as the final
`ifFail:` argument, which the bootstrap enforces
(`interpreter::kernel`'s `check_mirror_send_arguments`).

## Safepoints and call-site identity

`statement_program_counter` maps each statement index to its instruction range. This is the
safepoint abstract-state map the liveness design requires, and it has four
consumers — debugger, JIT deopt, image snapshot, mirrors — so it is emitted from
the start rather than added when the first of them is built.

An inlined block's statements are part of the enclosing statement's range;
`statement_program_counter` records only the code body's own statements. A debugger wanting
finer stepping inside inlined blocks would extend the table.

**A send's `CallSite` is its number within its code family**
(`bytecode::CallSite { number: u32 }`). A family is one root body — a method,
an instance initializer, a superclass clause or its message, each compiled by
its own top-level `Lowerer` — with every block nested in it. The lowerer
hands out numbers from one counter per family in emission order
(`Lowerer::take_call_site`): a block's lowerer starts where its creator's
counter stands, and the creator continues from where the block's ended, so a
family's sends are numbered densely from 0, blocks included, and no two share
a number. The counter is checked in every build: a family of more than
`u32::MAX - 1` sends is a compiler-invariant panic, never a wrapped number,
since two sends sharing a number would share a cache entry.

Each `CompiledCode` records its `family_size`, one past the highest number
among its own sends and its nested blocks' sizes, 0 when there is none
(`bytecode::family_size_of`). A block is finished before the root that
contains it, so a block's size is its own subtree's, and the root's is the
family's. Neither the number nor the size is on the wire: the decoder
assigns every family's numbers itself, in this same emission order, once the
whole code table is read, and derives the size from them with the same
function, so a family is dense by construction rather than checked for it
([image-format.md](image-format.md), "Verification").

The number indexes the running heap's cache table from the activation's
`feedback_base`; a send whose number is at or past its body's `family_size`
runs uncached ([dispatch.md](dispatch.md), "Code families" and
"`site_index`'s rule"). **`CallSite::UNCACHED`** (`u32::MAX`) is the number
no family reaches, and the reference lowering emits it on every send, so
everything it compiles runs the full lookup on every send, records nothing,
and is the uncached oracle the cached optimized lowering is checked against
("The reference lowering" below). Its `family_size` is 0. `ns-next disasm`
prints a send's number as `@N`, and `UNCACHED` as `@-`.

## Compilation cannot fail

**Compiling an already-resolved module can never raise a `NewspeakError`.**
Compilation is a total, deterministic lowering; a compiler-invariant violation is
`panic!` or `debug_assert!`, not a program-level error.

A program-level error the language raises at evaluation time is therefore
**deferred to execution**: it is reported when, and only if, execution reaches
it, so a method that is never run never fails. One literal carries that
deferral, `Literal::UnresolvedIdentifier`: an identifier the resolver left
unbound, a module root's clause receiver, which the root rule decides and
which never runs ([object-model.md](object-model.md), "The root rule").
Every other name is bound, a predefined class name included.

Every integer literal is representable (§5.1.1), so none is deferred. One that
fits a `SmallInteger`'s 61 bits lowers to `Literal::SmallInteger`, any other to
`Literal::LargeInteger`, which carries its value as an `ArbitraryInteger`, never
its text; `LoadLiteral` makes a fresh `LargeInteger` of it through
`Mutator::new_integer` each time it runs, and a float literal outside the
`SmallFloat64` range a fresh boxed `Float` through `Mutator::new_float`. A radix
or digit that denotes nothing is a parse error, and a fraction or exponent makes
a float, so `parse_integer_literal` (`interpreter/literals.rs`) refusing a
literal the parser accepted is a compiler invariant, and `lower_literal` panics
on it.

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
initializer's shape, and closure compilation. And because every send it
emits is `CallSite::UNCACHED`, it runs every send through the full lookup
while the optimized lowering's sends go through the inline caches, so it
checks the caches too: a hit that did anything a lookup would not shows as
the two lowerings disagreeing ([dispatch.md](dispatch.md), "3b — The
caches").

What it cannot check is anything the two share: the instructions' semantics in
the VM, the lookups themselves, the natives, `InitializeSlot` addressing, and the definitions both
take from `interpreter/`. Golden tests of exit status, stdout and stderr cover
those. Nor can it catch a misreading of the specification both lowerings were
written from; four of the six silent defects Milestone 3's review found were of
that kind, identically wrong under both engines.

### An independent module, not a mode

`compiler/reference.rs`, written from the AST and the resolver's bindings. It
shares with `lower.rs` only what must have one definition or is purely
mechanical: the bytecode types, `lower_literal`, `selector_of`, the parameter counting, the span helpers,
and `nested::superclass_clause_expression` and
`superclass_clause_message_arguments_expression` — the one definition of what a superclass
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
   sends `control_flow::recognize` accepts, and the mirror sends
   `recognize_mirror_send` accepts — is a `SendInlinable`, and every other
   explicit-receiver send a `SendPublic` ("How the limit counts" below). A
   mirror send is therefore an ordinary send to the mirror object here, its
   failure block a real closure ("Mirror sends").
5. An assignment evaluates its value into a fresh register, then stores it or
   sends the setter. The assignment's value is that register, never the local.
6. A cascade evaluates its receiver once into a fresh register and sends every
   message to it; its value is the last send's.
7. A block literal is a `MakeClosure` whose body the reference lowering
   compiles too, with an empty `copied` list: every block reaches its
   enclosing variables by reference, which is the model closure conversion is
   checked against ([execution.md](execution.md), "Verification and the
   reference lowering"). It emits no `MakeVector`, `LoadVector` or
   `StoreVector` at all.
8. `^` follows the code kind alone: `Return` in a method, `NonLocalReturn` in a
   block, evaluate-and-discard at the top of an initializer, `Return` in a
   superclass clause or its message. There is no "inside an inlined block"
   case. A body that falls off its end follows the rules of `finish_body`,
   written again rather than shared.
9. An initializer is each own slot's initializer followed by `InitializeSlot`, then
   the `initExprs`.
10. `statement_program_counter` and `register_count` follow the same rules as in
    `lower.rs`. Every send carries `CallSite::UNCACHED`, and `family_size` is 0.

Its `finish` checks that shape in a debug build, which is what the test suite
runs (`check_shape`): no branch instruction, since only inlining emits one;
every scratch register written by exactly one instruction; a named local
written only by `StoreLocal`; and a named local read only by `Move`, every other
operand being a scratch register — rule 3, the property that keeps a value from
changing under its consumer. The ceilings — at most `u16::MAX` temporaries,
block parameters, named locals and registers in a body — are checked in every
build (`Body::new` and its callers, `fresh`), because an index that wrapped
would be silently wrong code. A method's or a factory's parameter count comes
from the shared `argument_count_of` and `factory_argument_count`, whose
narrowing rests on the parser's bound instead. The optimized lowering is not
yet as strict: `lower.rs`'s `allocate_scratch` adds without a check, so a
body past that ceiling is silently wrong in a release build. Aligning it with
`fresh` is open. (Its call-site counter is checked in every build; see
"Safepoints and call-site identity".)

### Selecting it

`Lowering { Optimized, Reference }` lives in `compiler/mod.rs`, and the compiler
entry points take it. An `Interpreter` is given one when it is constructed —
`Interpreter::bootstrap()` is optimized, `Interpreter::bootstrap_with(Lowering)`
chooses — and `load_module` compiles with it, so one interpreter never mixes the
two. A decoded artifact runs the code it carries. The image does not record
which lowering produced that code, because nothing would read it.

`run`, `build` and `disasm` take `--lowering <optimized|reference>`,
defaulting to optimized. `disasm` of one method under both is the first tool to
reach for when the oracle fires. `bench` does not take it, since timing the
oracle means nothing, and `load` and `inspect` decode rather than compile.
The switch itself is `LoweringArgument` in `crates/ns-cli/src/main.rs`, converted
into `ns_runtime::Lowering` and passed to `program::load_program`, which
`run` and `build` go through for every module of a program, the manifest
included; `disasm` compiles one source file through `source::bootstrap`,
takes the switch as an `Option` and refuses it for an artifact, which
carries the code it was built with.

A command that dropped the switch would compare the optimized lowering with
itself, and no program's output would show it. `crates/ns-cli/tests/lowering.rs`
therefore reads the code instead: `disasm`'s listing, and `inspect --full` of an
artifact `build` wrote, have no branch under the reference
lowering and one under the optimized. `run` writes nothing to read; it is
covered by handing the switch to the same `program::load_program` call.

### The harness

- `crates/ns-cli/tests/run_goldens.rs` and `golden_round_trips.rs` run every golden
  under both lowerings against the same recorded answer. Under the reference
  lowering `golden_round_trips.rs` also takes the encoder and decoder over code with
  no jumps and many registers.
- `run-corpus` runs every row four ways — `run` of its generated
  application, and `build` then `load` of it, under each lowering — with no switch, because a gate must not be optional. A
  row fails if any of the four differs, and the report names which. The gate's
  counts stay per row.
- `crates/ns-runtime/tests/reference_lowering.rs` loads every corpus module the
  resolver accepts, under both lowerings, compiling without running. It
  exercises "Compilation cannot fail" and the reference lowering's shape
  assertions over the whole corpus rather than over the six modules that run.

Running every program under both lowerings took the suite (`cargo test
--workspace`, debug build) from 6.43 s to 9.09 s: every fixture, golden outcome
and build/load pair runs twice, and `run-corpus`'s rows four ways instead of
two — more process spawns and compiles, not a slower lowering. Since the suite
runs under `cargo nextest run`, a test whose two lowering iterations are
independent -- each builds its own interpreter and asserts on its own result --
is written once with `ns_runtime::for_each_lowering!`, which expands the body
into an `optimized` and a `reference` `#[test]` so nextest runs the two in
parallel instead of one test running both in sequence; `run_goldens.rs` splits
its per-golden trial the same way, by hand, since `libtest-mimic` builds its
trials at run time rather than from a macro.

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

Everything else was checked and agrees: all eight inlined forms exist as
kernel methods that follow the rules the inlined code reproduces (`True`'s
and `False`'s quick methods for the six branches, which answer the block's
value as it is, and leaves for the two loops, with `mustBeBoolean` for a
non-Boolean loop condition); a mirror send's failure block runs the same code
whether it is inlined or activated; a
method's `^` inside a block the optimized lowering inlines is a `Return` there
and a `NonLocalReturn` here, whose unwind ends at the method's activation, with
the same result, and
`cannotReturn` and the initializer gap behave alike; and a real block nested
inside such a block has one more frame in its chain here, which holds no
variables, since the inlining gate admits none, and the same `home`. An
artifact's bytes differ by construction, because its code table holds different
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
that a builtin control-flow step performs for a `SendInlinable`, and a mirror
send's failure block activated for one.** The steps are `ifTrue:`,
`ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` and `or:` on a
Boolean, answered by `True`'s and `False`'s `^<parameter> value` quick
methods ([core-library.md](core-library.md), "`True` and `False` are quick
methods"), and `whileTrue:` and `whileFalse:` on a block, answered by their
leaves through their kernel methods; the failure block is the one `interpreter::mirror`'s
`execute_sent` activates when a sent mirror send's leaf declines, which it
marks as a control-flow block. `SendInlinable` is an ordinary public send
that only the reference lowering emits, for exactly the sends
`control_flow::recognize` and `recognize_mirror_send` accept.

- **The count agrees at every point of every program.** The optimized lowering
  omits exactly these activations and no others. On a non-Boolean receiver both
  lowerings make the same real send with real closures, and from there count
  alike; the `mustBeBoolean` send is made alike too. That holds for an
  instance of a user subclass of `True`, which reaches `True`'s quick
  methods: they mark the block as a control-flow step only for a Boolean
  receiver, so its block is counted under both lowerings
  (`goldens/runtime/depth-limit/recursion_through_a_subclass_of_true_counts_alike_under_both_lowerings`).
- **The agreement depends on the kernel's fast paths, which is why they
  apply under both lowerings.** The reference lowering's `SendInlinable` of
  `ifTrue:` reaches `True>>ifTrue:`, a kernel method. Activated, that method
  would be counted, and the literal block its `value` activates would fail
  the captured-frame condition below, since that activation did not create
  it — two counted where the optimized lowering counts none. As a quick
  method it is never activated for a block argument: it answers the block
  straight to the `SendInlinable`'s sender, and the exemption applies as it
  always did; `whileTrue:`'s primitive method does the same through its leaf
  ([dispatch.md](dispatch.md), "Primitive methods").
  `NS_NEXT_NO_PRIMITIVE_METHODS=1`, which compiles the kernel without its
  primitive methods — the quick methods stay — breaks the agreement for the
  loops, and is for measuring only.
- **Depth stays bounded.** A literal block can run again only if the body
  enclosing it runs again, so every recursive cycle contains a counted
  activation, and the uncounted activations per counted one are bounded by the
  source's static nesting depth — at most 128, the parser's ceiling, so the
  stack holds at most 129 entries per counted activation
  ([execution.md](execution.md), "The depth limit").
- **The optimized lowering pays nothing** beyond the counter every rule needs,
  since it never emits `SendInlinable`.
- **A crafted artifact cannot use the exemption to recurse without bound.**
  Code decoded from an artifact can emit `SendInlinable` with any block in the
  argument register — `b:: [true ifTrue: b]` — and an exemption granted by
  selector alone would let that recursion run uncounted until memory ran out.
  So the block must also have been created by the activation that sends: its
  `captured_frame` is that activation's reified frame. Every block the
  reference lowering passes to a `SendInlinable` is a literal created right
  before the send — a mirror send's failure block included, and `Kernel.ns`
  passes no other literal block to one ("Mirror sends") — so the condition
  holds for every program either lowering compiles, and a chain of exempt activations is then bounded by how deeply
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
alone — `b:: [:n | n = 0 ifFalse: [b value: n - 1]]` — to a second, memory-based
bound that does not agree. Of the prior art, the Smalltalk lineage bounds depth
by memory and Nova by a fixed count of contexts ([execution.md](execution.md),
"How other systems bound recursion depth"); none has two lowerings that must
agree, so this rule is ns-next's own.

**Where it is decided.** The block arm of `Mutator::push_step`
(`crates/ns-runtime/src/interpreter/mod.rs`) is the one place an activation is
exempted. Every other push is counted by construction: `push_step`'s method arm
and its body arms, for an initializer level, a superclass clause or its
message, record `counted: true`. The flag
rides on the stack entry, and `Mutator::push` refuses a counted push at the
limit. A block is exempt when it carries
`StepArguments::ControlFlow` — the arguments only the control-flow leaves and a
declining sent mirror send build —
the send that led to it was a `SendInlinable`, and its `captured_frame` is the
sending activation's reified frame. The VM decides a `SendInlinable` exactly as
it does a `SendPublic` and only hands `push_step` the fact that it was one. A
native loop's next block is pushed by `Mutator::push_continuation`, which
exempts it only if the one it replaces was; the VM forwards the popped entry's
flag and decides nothing. `&` and
`|` share `and:`'s and `or:`'s native, so their block carries the mark as
well, but no lowering emits a `SendInlinable` for them, since `recognize` does
not accept them ([execution.md](execution.md), "The depth limit").

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
a frame". The control-flow steps sent from Rust, where their continuation
runs at the floor of the run, answer what they should and leave both depths at
0. The goldens
`goldens/runtime/depth-limit/thousand_deep_recursion_runs.ns` and
`goldens/runtime/depth-limit/unbounded_recursion_is_a_newspeak_error.ns` pin a thousand-deep
recursion through an `ifFalse:` block and the overflow's output, exit status and
error text, under both lowerings.

`SendInlinable`, wire tag 20, arrived with the trampoline, together with the
counter that is its only consumer: without the counter it would mean exactly
`SendPublic`, and nothing could test it. The limit is 100,000
(`MAX_ACTIVATION_DEPTH`), its error text is `stack overflow: recursion too
deep`, and control flow answers trampoline steps, so under neither
lowering does a block it activates cost native stack
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

## The simultaneous slot clause

A `|| ... ||` clause (§6.3.2) is desugared once, before both lowerings and
the capture analysis, by `compiler/simultaneous.rs`: `future_block` turns a
simultaneous declaration's initializer into the block literal
`[initializer]`, whose span is the declaration's own. The capture analysis
and the optimized lowering key a block by its span
(`capture::BodyKey::Block`), and no block literal can share a declaration's
span, since that also covers its name. The resolver scoped the initializer
as that block's body -- `resolve_simultaneous_initializer` pushes one scope
of its own around it -- so a sibling or a factory parameter it reads is
already an outer local, and the capture walker and both lowerings enter the
block in the same order the resolver did.

Each simultaneous initializer lowers to its block, `MakeFuture` of the
closure, and the first write: `InitializeSlotProvisionally` for a class's
slot, which skips a value's slot check, and a plain store for a temporary.
After the last declaration, the clause's end reads back each simultaneous
declaration that has an initializer, in declaration order, resolves it with
`ResolveFuture` and writes it again: `LoadOwnSlot`, `ResolveFuture` and an
ordinary, checked `InitializeSlot` for a slot, and a read of the local,
`ResolveFuture` and a store for a temporary. Every one of these writes is a
statement of its own. The reference lowering emits the same shape in its own
idiom, every read a fresh register.

A simultaneous temporary is written twice whatever its declaration says, so
it is never read-only: the capture analysis counts it mutable, and the
clause's end is a second write after every future's block was created, so a
sibling's block that reads it shares it through the indirection vector
rather than copying the **nil** its register held
(`goldens/language/slot/simultaneous/spec_6_3_2_a_captured_sibling_is_read_after_the_clause`); the
optimized lowering's writable table agrees. `ResolveFuture` runs user code
-- the future's block -- as a send does, and runs again once the block has
answered, reading its source afresh, so its source is protected across it
as an operand is across a send. The design is
[object-model.md](object-model.md), "Simultaneous slots".

## Follow-up: entry points that reach a class object only to find its mixin

The compiler itself no longer has such an entry point: it compiles onto a
`Mixin` while the tree is built, and never sees a class object. The pattern
remains in two places. `image::encode_code_table` and `image::mixins::plan` each
take a `ClassReference` parameter, and each one's first move is to walk from it to the
`Mixin` that backs it — the code they encode lives on that mixin's tree and
nowhere else. Reaching a class object's `mixin` is a read only `heap.rs` may
perform, so each also needs a `&Heap` parameter purely to make that one hop.
Taking the root mixin directly instead would remove the need for a `Heap` there —
there is nothing else on the class object either of them reads.

`ns-cli`'s `disasm` (`crates/ns-cli/src/commands/disasm.rs`) has the identical
shape but not the identical fix: it takes no `ClassReference` parameter to narrow,
since it builds a class object itself
(from source, or from a loaded artifact) and reaches the mixin through its own
`root_mixin_of` helper, needing a `Heap` for the same reason. It reads through
`interpreter.enter` inside the lifetime of the interpreter that built the class
object, because under the arena it must: a handle is an index meaningful only
against its own heap, so a stand-in `Heap` reads the wrong object in a release
build and trips the debug origin guard in a debug one. Its fix is not "take a
`Mixin` instead of a `ClassReference`" but "stop building a class object at all" — a
different, and possibly larger, change.

Recorded here as a follow-up rather than undertaken now, because narrowing the
signatures is a design change to the image module's interface, not a migration.

## Decisions

- **`Instruction` is a Rust enum, not packed bytes**; the loop-over-locals
  profile shows its dispatch cost, and a compact encoding is planned,
  designed against the one-word value ([execution.md](execution.md),
  "Planned: the compact instruction encoding").
- **A local's read costs no instruction**, which makes its register live storage
  and makes the value-register invariant necessary.
- **Inlining is gated syntactically at compile time and guarded again at run
  time**, with a fallback to a real send, so virtuality is never actually lost.
- **A mirror primitive's failure path is an explicit `ifFail:` block, which
  the compiler inlines** (decided with the user on 2026-09-21), behind
  `SendMirror`. Two things follow from that rather than being decided on
  their own: the inlined send needs no fallback send, because the mirror's
  class and table are fixed at bootstrap and nothing but the block can answer
  a leaf's failure; and `SendMirror` needs no wire tag, since only
  `Kernel.ns`, which no image carries, contains one ("Mirror sends").
- **The integer leaves are inlined under a run-time type guard whose
  fallback is the ordinary send**, so the promotion to a `LargeInteger` and
  every error an integer operation can raise — a `doesNotUnderstand:`, a zero
  divisor — are still decided by the primitive in `builtins/` and by nothing
  else ("Arithmetic inlining").
- **`whileTrue:` with a non-Boolean condition sends `mustBeBoolean`** rather than
  falling back to a re-send, because a re-send would duplicate the condition's
  side effects and the inlined loop would diverge from the native one.
- **Code lives on the mixin and `InitializeSlot` is mixin-relative**, because one mixin
  backs many applications with different inherited layouts.
- **Every send carries a call site, numbered per code family as a `u32`**, so
  the inline caches index a per-heap table without any instruction changing
  shape; the reference lowering's `UNCACHED` makes it the uncached oracle
  ("Safepoints and call-site identity"). The id was emitted inert, as the
  instruction's own index, before the caches existed; the caches changed only
  its meaning and width.
- **The reference lowering is an independent module, not a mode of `Lowerer`**,
  so that it checks everything a lowering decides and no optimization can switch
  it off by forgetting a guard.
- **The recursion-depth limit exempts exactly the activations inlining
  removes**, marked by `SendInlinable`, so that it counts the same under both
  lowerings and the oracle compares an overflow too.
