# Compiler — AST to bytecode

> **Status:** built, including the reference lowering it is checked against.
> **Code:** `crates/ns-runtime/src/bytecode.rs` (the instruction set and the
> compiled-code shapes), `bytecode/compact.rs` (the `CodeBuilder` and the
> eight-byte form a body is stored in), `compiler/mod.rs` (entry points,
> the initializers' prologue and `factory_body`),
> `compiler/lower.rs` (lowering, register allocation, the scope stack, the converted register
> layout), `compiler/capture.rs` (the capture analysis closure conversion runs
> ahead of the optimized lowering), `compiler/control_flow.rs`
> (inlining, and `recognize_mirror_send`), `compiler/primitive_method.rs`
> (which kernel methods are primitive methods), `compiler/reference.rs` (the
> reference lowering). CLI: `ns-next-analyzer disasm`.

This is the producer side. How the bytecode is then run — activations, the VM,
the dispatch loop — is [execution.md](execution.md).

## The instruction set

`Instruction` is the vocabulary a lowering emits: a plain Rust enum, which
both lowerings and the image decoder hand to a `CodeBuilder`
(`bytecode/compact.rs`). The builder encodes each instruction at once into
the form a body is stored and run in -- one eight-byte `CompactInstruction`
per instruction, with side tables for whatever does not fit -- and every
tool, `disasm`, the image encoder, the decoder's verification and the tests,
reads a body back as `Instruction`s through an exact decoded view
([execution.md](execution.md), "The compact instruction encoding"). One
instruction is one slot, so an instruction index is the same number in
either form. "Architecture-neutral and versioned" is a property of the
*image encoding* of compiled code — a tag byte and a fixed operand layout per
`Instruction`, with the tag numbers fixed for good; see
[image-format.md](image-format.md) — not of the in-memory form. The stored
form became packed when the profile of the loop over locals showed what the
enum cost the loop: every dispatch decoded its discriminant through a niche,
and `program_counter` and the code pointer were spilled to `vm::run`'s stack
frame ([execution.md](execution.md), "Where the 3% goes").

Every register operand is a `u16`; every jump target is an instruction index.
The eight-byte form narrows a register to a byte where it fits and takes a
wide form where it does not, which the vocabulary never shows. The
set follows the resolver's `Binding` variants and the lookup shapes directly, so
that lowering is mostly a transliteration rather than a translation.

**Loads and moves**

- `LoadLiteral { destination, literal }` — the literal is already lowered:
  `Nil | Boolean | SmallInteger | Float | String | Symbol | LargeInteger |
  UnresolvedIdentifier | BodyTooLarge(limit)`, with integer and float text parsed at
  compile time rather than at each execution. `LargeInteger` carries an
  integer literal beyond a `SmallInteger`'s 61 bits as its digits (an
  `ArbitraryInteger`); `UnresolvedIdentifier` and `BodyTooLarge` are the
  deferred cases, see "Compilation cannot fail".
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
identity". Each names its arguments as contiguous slots: `argument_base` and
`argument_sources`, the arguments being the registers `argument_base ..
argument_base + argument_sources.len()`. The send reads its arguments
where they are when its sources are consecutive registers -- one argument
always is -- and otherwise, before it decides, copies each source that is
not its slot into the slot, so a named local passed as an argument is
copied by the send itself, and only where the send cannot read it in
place. Nothing but the send reads a slot: a setter assignment's value is
the register its argument came from. Both lowerings allocate the slots
after every scratch register the send also reads -- its receiver, a mirror
send's subject -- and before they lower any argument, then lower each
argument into its slot ([execution.md](execution.md), "Planned:
overlapping register windows"). An assignment through a setter is a send
of one argument like any other: its slot is allocated before its value,
which is lowered into it, so `sum:: sum + n` writes the sum into the slot,
and the register the value is in is both the setter's source and the
assignment's value.

- **The optimized lowering** (`Lowerer::lower_expression_into`) produces
  in its slot a send it does not inline, whose `destination` is the slot;
  an inlined integer leaf (`n - 1`), whose fast path and fallback send both
  write their destination, the slot; and a literal, `self`, a read of
  another frame's local or of a vector, a closure and an array, each
  loaded into the slot. The send copies none of them. A named local's
  source is its own register, below the slots, which the send reads there
  or copies -- or, when an argument after it can write that local, the
  protective `Move` into the slot, which the send then does not copy ("The
  value-register invariant"). Every other argument's source is the
  register its value is in, which the send reads there or copies: the result register of
  an inlined conditional, `and:`/`or:`, loop or mirror send, and an
  assignment's or a cascade's value, which are above the slots unless the
  value is a named local's register. So the slots are not always the top of
  what is live at the send: a source the send reads from above them is
  live until it does. Every other register above the slots -- whatever
  an argument was evaluated with -- is dead by then.
- **The reference lowering** produces every argument in its slot, a named
  local by the `Move` that is its read, so each of its sources is its slot
  and no register above the slots is live at the send (`check_shape`).

- `SendPublic { selector, receiver_register, argument_base, argument_sources,
  destination, call_site }` — an ordinary explicit-receiver send, resolved
  with `lookupPublic`.
- `SendInlinable { selector, receiver_register, argument_base,
  argument_sources, destination, call_site }` — the same send, decided
  exactly as `SendPublic` is, marking one the optimized lowering would have
  inlined. Only the reference lowering emits it, and only the depth count
  reads the mark; see "How the limit counts".
- `SendLexical { depth, selector, argument_base, argument_sources,
  destination, call_site }` — `Binding::LexicalSend`. **No receiver
  register**: the receiver is `self` by construction, and the kth-enclosing
  object is computed at run time.
- `SendSuper { selector, argument_base, argument_sources, destination,
  call_site }` — receiver is `self`, lookup starts at the current
  activation's `current_class`'s superclass.
- `SendMirror { primitive, mirror_register, subject_register, argument_base,
  argument_sources, destination, failure }` — a send to the VM mirror whose
  failure block is lowered inline at `failure`: it runs the mirror row
  `primitive`, bound at compile time, with no lookup and no call site, and
  jumps to `failure` when the leaf declines. Emitted only into `Kernel.ns`,
  which no image carries, so it has **no wire tag**; see "Mirror sends".

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

**Instantiation** ([object-model.md](object-model.md), "Instantiation";
"The three initializer bodies" below)

- `AllocateInstance { destination }` — a factory body's allocation of an
  instance of the class object it runs on, after every refusal.
- `InitializeInstance { instance, destination }` — a factory body's run of
  that class object's own level's initializer on `instance`, with the
  body's own arguments.
- `CheckAppliedClause { steps, selector }` — §6.1's check of a `<:` clause
  against the primary factory of the class it named, whose level is
  `steps` above the running initializer's.
- `InitializeSuperclass { steps, selector, argument_base, argument_sources,
  destination }` — §6.1's check of the superclass clause, the arity check,
  then the initializer of the class `steps` above, on `self`, its arguments
  in slots as a send's are.
- `InitializeAppliedLevel { steps, argument_base, argument_sources,
  destination }` — the own part of the level a `<:` clause added `steps`
  above, on `self`, its arguments in slots as a send's are.

Each reads another level of `self`'s class chain, never another frame's
registers, so each answers 0 in `bytecode::outer_reach_of` and
`compact::narrow_outer_reach`.

**Leaving a body**

- `Return { register }` — a `^` in a method or initializer body.
- `ImplicitReturn { register }` — a body that fell off its end: a method returns
  `self`, a block its last expression's value. The VM treats it exactly like
  `Return`; it is kept distinct so `disasm` output stays legible.
- `NonLocalReturn { register }` — a `^` inside a block body.
- `SignalNonBoolean { register }` — an inlined `whileTrue:`/`whileFalse:`'s
  guard-failure path: signals `NonBooleanReceiver` for the condition's answer
  in `register`. Nothing after it runs, so it ends a path as a return does.

**Branches**

- `Jump { target }`, `BranchIfFalse { register, target }`,
  `BranchIfTrue { register, target }`,
  `JumpIfNotBoolean { register, target }`, `IntegerBinary { operation,
  destination, left, right, target }`.

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
    body: EncodedBody,         // the eight-byte slots and their side tables:
                               // instruction(index), instructions() and
                               // instruction_count() read it back as
                               // Instructions; the run loop reads slot(index)
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
`register_count - argument_count - HEADER_REGISTERS` registers above its
header (`CompiledCode::registers_above_header`), and a block's activation is
then handed its closure's `copied_count` values into the registers just above
its header's numbers.

`outer_reach` is derived from the instructions, by the compiler and by the
image decoder alike (`CompiledCode::from_builder`, off the encoding:
`bytecode::compact::narrow_outer_reach` for an eight-byte slot,
`bytecode::outer_reach_of` for the wide table), and decides whether the
activation that creates a closure keeps its registers when it returns
([execution.md](execution.md), "Converted representation").
`copied_count` is on the wire; `outer_reach` and `family_size` are not.
A body is made from the `CodeBuilder` a lowering or the decoder emitted it
into (`CompiledCode::from_builder`), or from a list of instructions encoded
at once (`CompiledCode::new`), and either way derives both from what was
emitted.

A method's or a block's own statements are its local slot initializers first,
one per declared local that has one, each `initializer_expression; StoreLocal`
into the slot's own register, in declaration order (§3.6), then its
`CodeBody.statements` -- the same order an instance initializer's own slot
initializers precede its `initExprs` ("One body per mixin" below). A local
with no initializer is skipped: its register keeps the **nil** every register
starts with.

A compiled body lives on the declaration it belongs to. A method's is on its
`MixinMethod` (`code`); a mixin's three initializer bodies ("The three
initializer bodies" below) and its superclass clause's body are on the
`Mixin` (`initializer_code`, `own_initializer_code`,
`standalone_initializer_code`, `superclass_clause_code`) — the clause's
because a nested class's superclass clause is code too, evaluated by the
accessor once per enclosing instance, and it answers the superclass and
every `<:` receiver as one array ([object-model.md](object-model.md),
"Mixin application"); and a factory's is on its factory entry, synthesized
("The three initializer bodies" below). A clause's message has no body of
its own: §6.2 and §6.3 evaluate its arguments on every instantiation, in
the initializer of the declaration whose clause it is, so they are lowered
into that declaration's full and standalone bodies. While the tree is
built from source, `MixinMethod::from_declaration` compiles each method and
`compile_mixin_bodies` a mixin's per-mixin bodies, before the mixin is wrapped in
the `Rc` every application shares; on load, `image::mixins::build` attaches
the bodies a row names from the image's code table, after `mixins::read` has
checked the rows and `modules::decode_modules` has read the module section,
and before `verify_mixins` reconciles them. So a method without a body cannot
exist, and nothing looks code up beside a declaration: a send that found a
method has found its code. That is how the prior art holds it — Primordial
Soup's `Method::bytecode()`, SOMns's `SInvokable.invokable`. Lowering turns
every resolver binding into an instruction, so no bindings map outlives
compilation.

### One body per mixin

One mixin backs every application of it, so a body is compiled **once per mixin**,
stored on it, and shared by every application. §6.2: "they all share their code and
structural definition with the mixin." Storing it per class object would recompile
the same body per enclosing instance, since a class object is per-instance.

**`InitializeSlot`'s `own_index` is therefore mixin-relative.** An absolute field index
baked into shared code would be wrong the moment a second application inserts
different inherited slots ahead of it. The VM resolves `own_index` against the
executing activation's slot base — the own `own_slot_base` of the
application it was entered at, which for an initializer is the level it
initializes and for a lazy slot's getter its defining class
(`Heap::activation_slot_base`) — at run time, after checking the
activation's recorded guard ([object-model.md](object-model.md), "What a
slot access checks").

A bug here is silent wrong output under one application and correct output under
another. `crates/ns-runtime/tests/vm_initializer.rs` applies one mixin to two
superclasses of different shapes, and initializes a three-level chain, to pin
both.

Compiled initializers are layout-dependent, so a class-shape change must flush
them along with every other cache keyed to the class. See
[liveness-and-reflection.md](liveness-and-reflection.md).

### The three initializer bodies

A declaration `class C f: a = S ms <: M1 m1 <: … <: Mn mn ( |slots| exprs )`
compiles three initializer bodies, each of kind `Initializer` and each
taking the factory's arguments, which run instantiation's steps 2 to 6
([object-model.md](object-model.md), "Instantiation"):

- **The own part** (`own_initializer_code`): `C`'s own slot initializers and
  `initExprs`, then `self` -- nothing of its clauses. It is what a level a
  `<:` clause adds runs for the mixin it applies (`InitializeAppliedLevel`).
- **The full body** (`initializer_code`): a prologue, then the own part. The
  prologue (`Lowerer::lower_instance_initialization_prologue`, the
  reference lowering's `Body::prologue`) checks every `<:` clause's
  selector against the class it names, the nearest level -- the last
  clause's -- first (`CheckAppliedClause`, one statement), before anything
  runs; evaluates `ms`'s arguments and initializes the superclass with them
  (`InitializeSuperclass`, one statement), which checks §6.1's selector
  only once the arguments have run; then, from the lowest level up,
  evaluates each `mk`'s arguments and runs that level's own part
  (`InitializeAppliedLevel`, one statement each). Each statement has its own
  scratch registers: nothing is live across them. Levels are counted from
  the one the initializer is entered at: `<:` clause `k` of `n` names the
  level `n - k + 1` steps up, the superclass clause's class `n + 1`. A
  body-less declaration (`Mixin::has_body`) runs at its class object, the
  level its last clause adds, so its levels are counted one lower, from 0,
  and its own part is empty. A declaration that writes no superclass clause
  initializes `Object` through `new`, one step up (§6.1). The clause
  arguments are lowered inline, in the factory scope the resolver gave
  them, as the references lower them, and the capture analysis sees them
  first, in the order they are lowered, so a block in a clause argument
  captures a factory parameter like any block in the body
  (`a_block_in_a_clause_argument_captures_the_factory_parameter`).
- **The standalone body** (`standalone_initializer_code`): the full body
  without the `<:` clauses' checks and levels, the superclass one step up,
  for a level of `C`'s mixin reached from outside a declaration that applies
  it. Only a declaration that writes `<:` clauses and a body has one:
  without clauses it would be the full body, and a body-less declaration's
  class object is never reached from outside its declaration.

A class side has no declaration of its own to read clauses from, so its
full body is its own part, the same body. Both lowerings emit the same
shapes, each with its own register allocation
(`the_reference_lowering_emits_the_same_prologue`). The four instructions
the prologue and the factory emit carry no outer reach and address no own
slot, and each is one call into `interpreter/instantiate.rs`, which decides
and raises every error: `vm.rs` runs the three that may start an
activation -- `InitializeInstance`, `InitializeSuperclass` and
`InitializeAppliedLevel` -- through the same `send` helper as a send,
writing a value answered at once to the instruction's destination or
pushing the activation decided, whose answer goes there too and is not
used.

**A factory's body is synthesized, not lowered.** `compiler::factory_body`
of the factory's arity is three instructions whatever the arity: it
allocates an instance of the class object the factory was sent to
(`AllocateInstance`), runs that class's initializer on it with the
factory's own arguments, the body's argument registers, so it names no
list (`InitializeInstance`), and returns the instance (`Return`), not the
initializer's answer. It depends on nothing but the arity, and needs no
AST and no resolver; `mixin::factory_entry` attaches it to every factory
entry, whoever builds one -- both lowerings' source path, the embedded
shipped code or the image decoder -- so no image carries one and none can
forge one, and the reference lowering has no shape of its own to check
here. A host's `Interpreter::instantiate` runs `factory_body` of its
argument count, so a class object a host built by hand, with no
metaclass, instantiates too.

**The `u16` ceilings.** A step count is a `u16`. It never overflows,
because the resolver refuses more than 1,024 `<:` clauses ("Limits"
below) and `compiler::superclass_steps` asserts it. A factory of more than
255 parameters is past the argument limit. A factory of 65,534 or more has
no room for its two registers after its arguments either. In both cases its
body is `body_too_large`'s and raises "factory too large" with the limit
it passed, the register ceiling first.

## Registers

Each activation carries its arguments, then the thirteen numbers of its frame
header (`bytecode::HEADER_REGISTERS`, held equal to `frame_header::COUNT`),
then its temporaries and scratch, as one unified register space addressed by a
single `u16`: registers `0 .. argument_count` are the arguments,
`argument_count .. argument_count + HEADER_REGISTERS` are never named by an
instruction, and everything else starts at `argument_count +
HEADER_REGISTERS`. On the activation stack it is one window, the header's words
where those numbers are, so every register is one addition from the frame's
start, and a send's argument slots are its callee's arguments, the callee's
header laid where they end ([execution.md](execution.md), "Planned:
overlapping register windows"); a frame object has a field for every number
but the header's, its arguments and then its registers above the header
(`Heap::frame_field_of_register`). The resolver's `Binding::Local` numbers the arguments and
then the temporaries with no gap, so a temporary's register is its resolver
index plus the header's numbers, which `Layout` and the reference lowering's
`local_register` add; a block's `OuterLocal` index is mapped the same way,
against the argument count of the body that declares it, and is untouched by
the scratch registers a bytecode frame adds — scratch sits after the named
locals. A body's `register_count` is one past its highest register number, so
at least `argument_count + HEADER_REGISTERS`, but for a body past the register
ceiling with too many arguments for that, whose registers are its arguments
alone (`compiler::body_too_large`); the header's numbers count against the
ceiling like any other. Its `local_count` is the first scratch register --
one past the arguments, the header's numbers, the copied values, the
temporaries and the vector register -- which both lowerings record and an
image carries: every send's slots start at or above it, and every outer
local a block names lies below it in the body that declares it, which is
what lets the image decoder refuse an outer access into another frame's
scratch, where a callee laid on its slots has its live header
([image-format.md](image-format.md), "Verification").

**A converted block's layout inserts its copied values after the header's
numbers.** Under the optimized lowering a block's registers are its
parameters, the header's numbers, then the values it was handed when the
closure was created, then its temporaries, then one hidden register holding
its indirection vector when it has one, then scratch; a method's, an
initializer's or a clause's are the same without the copied values
([execution.md](execution.md), "Closure conversion"). A resolver index is no
longer a register index, so `Layout` (`compiler/lower.rs`) is the one place
that maps one to the other: `local_register` pushes every temporary up by the
header's numbers and the copied count, and a local the capture analysis put
in the vector keeps its resolver index while its register simply stays nil,
so every `Binding` stays valid. A fallback closure and anything nested in it
get a by-reference `Layout` instead — no copied values, no vector, every local
in the register its resolver index names, a temporary's past the header's
numbers — because such a closure reaches the frame whose common path it
stands in for by reference.

**Every register above a send's argument slots is dead when the send
executes**: the callee's header and window go over them. The bump allocator
makes it true -- nothing is allocated below a slot after it, and what an
argument is computed with lands above -- and both lowerings check it on every
body in a debug build (`reference::check_leftovers`, which runs the leftover
analysis, "What a callee leaves"): no instruction after a send reads a
register above its slots before something writes it again, along any path --
a loop's back edges and the entry state included. The one value a lowering keeps across a send is a setter
assignment's, which is the setter's slot or a named local below it: a value
left in a register above the slot, an inner assignment's or an inlined
conditional's, is copied into the slot (`lower_setter_assign`).

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
construction, so its class is always `Closure` and there is no receiver ambiguity to
fall back from; what the guard checks is the **condition block's value** each
iteration. A non-Boolean condition cannot fall back to re-sending `whileTrue:` to
a materialized closure, because that would re-run the condition body and duplicate
its side effects before the native could report anything — an observable
difference. Instead the natives define the behaviour and the inlined form
reproduces it: both signal a `NonBooleanReceiver` carrying the offending value,
built by the one function `interpreter::unwind::non_boolean_condition`. The
inlined loop's guard-failure path is the single instruction `SignalNonBoolean`,
which ends the path as a return does; the native loop's continuation raises the
same error, so class, text, selector and receiver are identical under both
lowerings. A loop's behaviour is the library's to define, so there is no message
a program could implement to answer a non-Boolean condition.

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
must signal the identical `NonBooleanReceiver`.

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
list: an integer's `/` answers a `Fraction` unless the divisor divides, so it
has no `SmallInteger` answer to inline; it is `Number`'s send.

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
`builtins/numbers/` and stays there: the promotion of a result that does
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
retiring tag 33 and taking the next free instruction tag (41 today, per
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
  core-library source (`Kernel.ns`, `PlatformFactory.ns`) ever has one (a
  cascade to `vmMirror` is not marked, and compiles as ordinary sends);
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
`bytecode::outer_reach_of` answers 0 for it.

**It has no wire tag**, and the encoder treats one as unreachable. It is
emitted only for a `MirrorSend`, so only into shipped core-library source,
and no image carries shipped code: an image names every shipped object, a
builtin class among them, by its declaration, and the bootstrap of the
loading process supplies it ([image-format.md](image-format.md), "What is
*not* serialized"). A tag would be a wire form nothing legitimate writes, and
a decoder that accepted one would hand an artifact a way to run any leaf with
no lookup. The embedded shipped code carries the kernel's `SendMirror`s in a
codec of its own, which only the boot reaches with the binary's own bytes
([core-library.md](core-library.md), "The embedded shipped code").

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
family's. A send's `CallSite` is stored in its `SendSite`, in the body's
`send_sites` table, whichever of its eight-byte or wide forms the send
took. Neither the number nor the size is on the wire: the decoder
assigns every family's numbers itself, in this same emission order, once the
whole code table is read, and derives the size from them with the same
function, so a family is dense by construction rather than checked for it
([image-format.md](image-format.md), "Verification"). It numbers them in
place through `EncodedBody::for_each_emitted_mut`, which visits every send's
call site and every block in emission order, whatever form each took -- a
`MakeClosure` kept in the `wide` table included.

The number indexes the running heap's cache table from the activation's
`feedback_base`; a send whose number is at or past its body's `family_size`
runs uncached ([dispatch.md](dispatch.md), "Code families" and
"`site_index`'s rule"). **`CallSite::UNCACHED`** (`u32::MAX`) is the number
no family reaches, and the reference lowering emits it on every send, so
everything it compiles runs the full lookup on every send, records nothing,
and is the uncached oracle the cached optimized lowering is checked against
("The reference lowering" below). Its `family_size` is 0. `ns-next-analyzer disasm`
prints a send's number as `@N`, and `UNCACHED` as `@-`.

## What a callee leaves

A send's callee is laid on its argument slots, so after it returns, every
register of the sender above the slots may hold what the callee left
there: its header's words and its temporaries, and in a shipped method's
scratch the VM mirror. `bytecode::leftovers` derives, from a body's code
alone, which scratch registers -- those at and above `local_count` -- may
hold a word the body did not write, at each instruction. It is the base of
three readers still to come:

- the image decoder, which refuses a read of such a register (`check`,
  which keeps nothing);
- a debug build, which checks every compiled body;
- the collector's pre-pass, which nils those registers so that neither a
  push nor a pop need write them. It asks `tainted_at` for every frame pc
  of a code it meets, answered as sorted disjoint ranges from one pass.

**The rule.**

- **At entry** every scratch register is tainted, so that a push need fill
  none of them. Registers below `local_count` -- the arguments, the
  header's numbers and the named locals -- are never tainted: no callee is
  laid below the scratch, and the push fills them.
- **A send** other than `SendMirror` taints the registers at and above its
  slot end. A mirror send's leaf lands above its sender's window.
- **A send writes none of its slots for certain.** The run loop reads one
  argument, or consecutive ones, where they stand (`vm.rs`,
  `argument_window`), and a primitive or an accessor answers without a
  push. So a slot keeps the status it had: clean where its producer wrote
  it, tainted where an earlier callee went over it or since entry.
- **A write** cleans a register, a send's destination after the callee
  returned. **A read** of a tainted register is refused (`Read`).
- **A join** is tainted where either path is.

The register effects are `bytecode::registers_read`, `register_written`,
`jump_target`, `falls_through` and `successors`. Each is exhaustive over
`Instruction`; the reference lowering's checks and the decoder share them.
A load of `Literal::BodyTooLarge` does not fall through: it always raises,
and an `Error` is not resumable. So the return after it, in the body a body
past a limit compiles to, which reads a scratch register nothing wrote, is
never reached.

Every body the goldens, the core library and `benchmarks/` compile to,
under both lowerings (18,496 bodies), reads no register the rule taints,
none tainted since entry either (`every_golden_body_stays_within_the_bound`).

**The pass.** One pass in instruction order over one current bitset of
`register_count` bits, rounded up to 64. A state stored at every
instruction would be quadratic in compiled code; a state stored at every
pending jump target is bounded by how many can be pending at once.

- **A forward jump** ORs the current state into its target's pending
  bitset, allocated when the target first gets one. A jump past the body
  is left to the decoder's own check.
- **Reaching a pending target** ORs its bitset into the current state, or
  takes it over when the previous instruction does not fall through. The
  bitset is then freed, kept for the next forward jump to take, so the pass
  never holds more than its peak. An instruction no path reaches is
  skipped, and answers no tainted register.
- **A loop head**, the target of a backward jump, takes the state it is
  reached with ORed with `[floor, register_count)`, where `floor` is the
  least slot end of a non-mirror send between the head and its last back
  edge.
  - Loops must nest, no more than `LOOP_LIMIT` may be open at once, and
    a jump into a loop other than at its head is `PastTheBound`. So the
    loop's code cannot taint below the floor, and every later iteration's
    state is within the head's: the back edge needs no second pass. A
    debug build asserts it at every back edge.
  - Over-tainting the head is sound. Neither lowering reads a register at
    or above the floor before rewriting it in the same iteration: every
    scratch register a loop reads across iterations was allocated before
    the loop, below every slot inside it.
- **A body of more than `INSTRUCTION_LIMIT` instructions** is
  `PastTheBound` before anything is allocated.

**The oracle.** A test-only fixpoint over explicit sets (`oracle` in
`leftovers/tests.rs`), tainting every scratch register at entry, holds the
pass to the rule: equal at every instruction of a body without a loop, and
never less with one. It runs over 4,000 crafted bodies whose forward jumps
cross freely, 300 more of up to 260 registers, so that every bitset
operation crosses words, and every compiled body of up to 400
instructions.

**The bound on pending targets.** The analysis refuses a body that keeps
more than `PENDING_LIMIT` targets pending at once, stated and derived
once, in `leftovers.rs`. A target is pending while a jump to it is open,
so no more targets are pending than jumps are open:

- **The reference lowering** emits no jump (`check_shape`).
- **The optimized lowering** opens forward jumps only in four inlined
  forms (`lower.rs`; `finish`'s target check lists every emitter):

  | Form | Open over the code in its blocks | Open over its own instructions |
  |---|---|---|
  | `emit_guarded_branch` (`ifTrue:` and its relatives, `and:`, `or:`) | two: the guard and the false branch over the true arm; the guard and the true arm's exit over the false arm; both exits over the fallback, a send of closures | three, as the true arm's exit opens |
  | `lower_while` | two: the guard and the exit branch over the body. The condition has none of this loop's | two |
  | `lower_mirror_send` | one: the jump past the failure block | two |
  | `lower_integer_binary` | none, since its operands are lowered before it | two, around the fallback send |

- **What lies outside a form's jumps:** everything evaluated before a
  form's first jump (its receiver, its operands) has none of its jumps
  open, and all code a form encloses lies in one of its block literals.
- **What the parser allows:** a block literal is one of the parser's
  guarded productions, of which at most `MAX_DEPTH = 128` may be open
  (`crates/ns-syntax/src/parser/mod.rs`). A form needs one production
  more than the blocks around it, for its blocks or its operands, so its
  own instructions lie inside at most 127.
- **So** the most that can be open is a guarded branch's three at that
  depth: `PENDING_LIMIT = 2 × 127 + 3 = 257`.
- **Pinned and measured.** `the_parser_accepts_no_deeper_nesting` holds
  `PARSER_NESTING_LIMIT` to what the parser accepts. A `whileTrue:`
  nested in the body of another 125 deep, the deepest the parser accepts,
  keeps 250 pending (`the_deepest_nesting_stays_within_the_bound`).

**The bound on open loops.** The analysis refuses a body that keeps more
than `LOOP_LIMIT` loops open at once, nested each in the next, stated and
derived once, in `leftovers.rs`:

- Every loop either lowering emits is `lower_while`'s, with one back edge;
  the reference lowering emits none.
- A loop's code is its condition's and its body's block literals, so a
  loop nested in another lies inside one of the outer loop's block
  literals, and the innermost of `depth` nested loops inside `depth - 1`
  of them.
- Its own instructions lie inside at most 127 block literals, as above. So
  `LOOP_LIMIT = PARSER_NESTING_LIMIT = 128`.
- Measured: 2 open at once over every compiled body, and 125 for the
  125-deep `whileTrue:` nesting.

**The bound**, for a body of at most 65,535 registers, so of at most
1,024 words a bitset:

- **Memory:** at most `PENDING_LIMIT + 1 = 258` bitsets, the pending ones
  and the current one, of `ceil(register_count / 64)` words each: at most
  258 × 8 KiB = 2,064 KiB, 2.02 MiB, per body, freed with it. Beside them
  the pass keeps the body's loops, one twelve-byte entry per backward jump,
  smaller than the instructions it is read from, and the at most 128 loops
  it is inside. A debug build also keeps a copy of each open loop's head
  state for its back edges' assertion, found by binary search among the
  loops: at most `258 + 128 = 386` bitsets, 3.02 MiB, per body.
- **Work:** at most `3 × ceil(register_count / 64) + c` word operations per
  instruction. An instruction takes at most three bitset operations: the
  OR of a reached target's bitset (taking it over is a swap); a loop
  head's floor; and one more, a send's taint or a forward jump's OR or
  copy, never both, since the one send that jumps, `SendMirror`, taints
  nothing. `c` covers the bit tests of the at most `ARGUMENT_LIMIT + 2`
  registers an instruction reads, a write's bit, and the search and the
  insertion among at most 257 pending targets. Finding the loops sorts the
  backward jumps, whose logarithm `INSTRUCTION_LIMIT` bounds by 20. Nothing
  is charged per target beyond that, and no state is undone.

Measured, the largest figures:

| Bodies | Bitsets at once | Pending at once | Bytes of bitsets | Word operations per instruction |
|---|---|---|---|---|
| every compiled body, both lowerings | 15 (`CD.ns`) | 14 (`CD.ns`) | 144 (`three_hundred_registers.ns`, 323 registers) | 12, at 6 words a bitset; at most 2 per word |
| the 125-deep `whileTrue:` nesting | 251 | 250 | 14,056 | 14, at 7 words; 2 per word |
| the flat case at N = 2,000, a tuple of inlined conditionals | 4 | 3 | 4,032 | 252, at 126 words; 2 per word |
| 257 nested forward jumps over 65,535 registers, crafted | 258 | 257 | 2,113,536 | 1,024, at 1,024 words; 1 per word |

**The tests:**

- `every_golden_body_stays_within_the_bound`, a unit test, since it needs
  the shipped modules resolved as the boot resolves them. It asserts that
  each source it cannot compile is a golden recording a parse or resolve
  error.
- `the_flat_worst_case_stays_within_the_bound`.
- `the_deepest_nesting_stays_within_the_bound`.
- `the_most_a_body_holds_is_258_bitsets_of_the_widest_register_file`.
- The hand-built cases in `leftovers/tests.rs`, among them
  `scratch_is_tainted_at_entry` and
  `a_send_taints_from_its_slot_end_and_writes_none_of_its_slots`.

## Compilation cannot fail

**Compiling an already-resolved module can never raise a `NewspeakError`.**
Compilation is a total, deterministic lowering; a compiler-invariant violation is
`panic!` or `debug_assert!`, not a program-level error.

A program-level error the language raises at evaluation time is therefore
**deferred to execution**: it is reported when, and only if, execution reaches
it, so a method that is never run never fails. Two literals carry that
deferral. `Literal::UnresolvedIdentifier` is an identifier the resolver left
unbound, a module root's clause receiver, which the root rule decides and
which never runs ([object-model.md](object-model.md), "The root rule").
Every other name is bound, a predefined class name included.

`Literal::BodyTooLarge(limit)` is a body past one of the compiler's limits
("Limits" below), `limit` naming which one: `BodyLimit::Registers`, the
register ceiling; `Arguments`, more than 255 parameters, or a send or a
clause message of more than 255 arguments; `Instructions`, more than 2^20
instructions. A register is numbered by a
`u16`, so a body can use at most `u16::MAX` (65,535) of them: its
parameters, the values a block is handed, its named temporaries, the vector
register and the scratch registers its expressions need. Both lowerings count
them with checked arithmetic in every build -- `named_locals_fit` before a
body is laid out (the capture analysis neither declares nor walks a body that
fails it), `Layout::converted`'s checked sums, and `fresh` and
`allocate_scratch` as it is lowered -- because a wrapped index would be
silently wrong code. A body past the ceiling is marked too large, nothing it
emits is kept, and it compiles to the one body `compiler::body_too_large`
builds: `LoadLiteral` of `Literal::BodyTooLarge` into the register after its
arguments, then `ImplicitReturn` of that register, of the same kind and arity,
with no copied value and statement 0 covering both instructions. Its
`register_count` is its arguments and that one register; a body whose every
register is an argument loads into its last argument's register instead, since
the literal raises before anything is written. When it runs it raises
`method too large: it needs more than 65535 registers`, with `block`,
`initializer`, `factory` or `superclass clause` in place of
`method` for the other kinds (`interpreter/literals.rs`,
`body_too_large_error`, which `vm::make_value` only calls). Past the other
two limits the message ends `it takes or sends more than 255 arguments` or
`it needs more than 1048576 instructions`. The image carries the limit, one
literal tag for each, so a loaded body raises what it raised from source.
When a body passes several limits, the first one its lowering meets is the
one named. That is the register ceiling for a body whose named locals do
not fit, which is judged before anything else. Each body is judged
on its own: a block past the ceiling makes only that block raise, its creator
hands it no copied value and compiles as it would around any block, and a
sibling method still runs. A block with more parameters than a `u16` counts
takes `u16::MAX` of them, and its `MakeClosure` names as many
(`compiler::parameters_of`), so a call with more meets the block's arity
check, which raises too. A method or a factory with more than 65,535
parameters compiles and raises the same way; its factory body and its
initializers, whose arity saturates too, are still handed every factory
argument, since they raise before they read one
(`CompiledCode::stands_for_too_many_arguments`, which `push_step` asks of
a factory body). An image cannot carry a method, factory or block of more
than 255 parameters. `image::snapshot`, and so `ns-next build`, refuses
the program at the body's arity with `image encoding cannot write a body
arity of 256: it is past the limit of 255 arguments` (`encode::arity_field`,
which also holds a closure's parameter count and a method's and factory's
arity to the limit). A body past the register ceiling saturates its arity
at 65,535 and meets the same refusal. The decoder refuses each of these
where it reads the count. A register list of more than 65,535 entries is
refused by its 16-bit field instead, with `image encoding cannot write a
register list of 65536: it does not fit the image format's 16-bit field`.
Under the optimized lowering, a tuple whose operands all read one
parameter's register fits the ceiling and still meets that refusal. Both
refusals are errors, never aborts ([image-format.md](image-format.md), "The
instruction encoding", "Verification").

The two lowerings meet the ceiling at different sizes. The reference lowering
never reuses a register, so every value costs it one; the optimized lowering
reuses its scratch registers from statement to statement. A method of 65,537
statements answers under the optimized lowering and raises under the reference
lowering (`crates/ns-runtime/tests/compact_encoding.rs`), and a body of more
than 65,535 named temporaries raises under both. A body between the two
ceilings is the one known case where the two lowerings' outcomes differ, and
it is accepted: the ceiling is an implementation limit no shipped, measured or
golden program comes near, far above the 8-bit temporary count of Primordial
Soup's method header (`prior-art/primordialsoup/vm/object.h`, `NumTemps`) and
the 6-bit one of Cog's (`temporaryCountOfMethodHeader`,
`prior-art/opensmalltalk-vm/src/spur32.cog/cointerp.c`), and the
specification states no limit (its text searched for "limit", "maximum", "at
most" and "implementation-defined").

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

## Limits

These are the limits a construct of Newspeak source meets in this
implementation. The specification states none (its text searched for
"limit", "maximum", "at most" and "implementation-defined"), so each one
is a property of this implementation, set well above everything measured.
The measured maximum is the largest over `corpus/modules`, the benchmarks
and the core library; a golden that probes a limit is named where it goes
further. Past a compile-side limit a body compiles to
`Literal::BodyTooLarge(limit)` and raises only when it runs ("Compilation
cannot fail"). Past a source-side limit the program is refused before it
compiles. The image decoder refuses each one where it reads the count, before it
reserves anything for it ([image-format.md](image-format.md),
"Verification").

| Construct | Limit | Enforced by | Measured maximum |
|---|---|---|---|
| Instructions per body | 2^20 = 1,048,576, `bytecode::INSTRUCTION_LIMIT` | both lowerings, `compiler::emitting_passes_a_limit` before every emit: `BodyTooLarge(Instructions)`; `CodeBuilder::emit` asserts it; decoder `InstructionCountTooLarge` | 990, both lowerings (`NewspeakGrammar.ns`) |
| Parameters of a method, block or factory; arguments of a send or a clause message | 255, `bytecode::ARGUMENT_LIMIT` | both lowerings, `compiler::parameter_limit_passed` and `emitting_passes_a_limit` (every `Instruction::argument_slots` list): `BodyTooLarge(Arguments)`; `factory_body`; encoder `arity_field`; decoder `ArgumentCountTooLarge` | method 12 and send 13 (`CAPI.ns`), block 12 (`NewspeakParsing.ns`), factory 7 |
| `<:` clauses per superclass clause | 1,024, `mixin::CLAUSE_INDEX_LIMIT` | a resolve error (`resolver/classes.rs`, `clause_count_error`); decoder `ClauseCountTooLarge`, `ClauseIndexTooLarge` | 1 (core library); goldens 3 |
| Registers per body | 65,535, the `u16` that numbers them | both lowerings: `BodyTooLarge(Registers)` ("Compilation cannot fail"); decoder: the `u16` header field and `verify_body` | 329 optimized (`LanguageUiDescriptionRegistry.ns`, a tuple of 306), 956 reference (`KernelTests.ns`) |
| Source nesting depth | 128, `ns-syntax`'s `MAX_DEPTH` | a parse error ([frontend.md](frontend.md)); block nesting, outer reach and class nesting derive from it, and the decoder's `MAX_BLOCK_NESTING` | 14 (`Newspeak2V8Compilation.ns`) |
| Literals and constants per body | none | each is an instruction, so the instruction limit bounds them; a table past 16 bits takes the wide form | 152 table literals (`LanguageUiDescriptionRegistry.ns`) |
| Slots per class | none | an `own_index` is a `u32`, and an object's word count has an overflow word | 147 (`NewspeakGrammar.ns`) |
| Cascade length | none | a cascade holds one receiver register, and the instruction limit bounds its messages | 86 (`Browsing.ns`) |
| Message chain length | none | every length is valid Newspeak, and the instruction limit bounds it. The compiler still recurses along a chain and overflows at about 20,000 links, so the instruction limit is not yet the real bound there (`OPEN-WORK.md`) | 17 (`NewspeakGrammar.ns`) |
| String and symbol literal length | none | only the module's size, which a `Span`'s `u32` offsets bound, and the image's `u32` length | 36,775 bytes (`DeploymentManager.ns`) |

The instruction limit keeps every instruction index, jump target and table
index within 20 bits, and it bounds the work of every pass over one body. It
does not bound the code of a nested inlined conditional. That conditional's
fallback closure compiles its blocks again, which multiplies the bodies and
call sites of one code family with each level of nesting, but each body
stays small: about 17 instructions in the ten-deep construct of
`OPEN-WORK.md`. So a small source can compile to a family of a million
bodies that no limit on one body refuses. With the argument limit, an
argument count fits a byte, and every program that compiles can be
packaged. The clause limit
replaces the two ceilings that existed before it: the class object's
23-bit clause field, which keeps its headroom, and the initializer's
`u16` step count.

## Reading the output

`ns-next-analyzer disasm <file.ns> <selector>` prints one body's instructions, compiling
only — nothing of the program is instantiated or run; the interpreter it compiles
on instantiates only the shipped core library — and addressing code through mixins,
since that is what code lives on. It also accepts an artifact in place of a
`.ns` file, decoding rather than compiling; a deployed binary ships no source
at all, so that path is what makes `disasm` usable on one. See
[image-format.md](image-format.md)'s "Reading an artifact".

For every body at once rather than one, `ns-next-analyzer inspect <artifact> --full`
disassembles the whole code table with each body attributed to the declaration
that owns it. `disasm` is the way in when you know which body you want;
`inspect` is the way in when you do not. `disasm --initializer` prints a
mixin's three initializer bodies -- the full one, `<Name> own part` and
`<Name> standalone` when it has one -- and the factory entry's body, as
`method <Name> factory <selector>`.

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
mechanical: the bytecode types, `lower_literal`, `selector_of`, the parameter
counting, the span helpers, and `nested::superclass_clause_expression` and
`superclass_clause_message_arguments_expression` — the one definition of what
a superclass clause evaluates.

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
3. Every read of a local is a `Move` into a fresh register (or into the
   argument slot it is read for, rule 4), and every read of an outer local
   a `LoadOuterLocal`. There are no inlined scopes, so the hop
   count is the resolver's depth unchanged.
4. Every send is a real send, receiver then arguments from left to right.
   Nothing is inlined. Every send the optimized lowering inlines — exactly the
   sends `control_flow::recognize` accepts, and the mirror sends
   `recognize_mirror_send` accepts — is a `SendInlinable`, and every other
   explicit-receiver send a `SendPublic` ("How the limit counts" below). A
   mirror send is therefore an ordinary send to the mirror object here, its
   failure block a real closure ("Mirror sends"). A send's argument slots
   are allocated after its receiver, and each argument is produced in its
   slot -- the instruction that computes its value writes the slot, and a
   named local's read is the `Move` into it -- so every source is its slot
   and the send copies nothing: the optimized lowering's copy inside the
   send is what the two are compared on.
5. An assignment evaluates its value into a fresh register, or into the
   slot it is an argument for, then stores it or sends the setter, whose
   one slot is that same register: nothing above it is live by then. The
   assignment's value is that register, never the local.
6. A cascade evaluates its receiver once into a fresh register and sends every
   message to it; its value is the last send's, produced in the slot when
   the cascade is an argument.
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
changing under its consumer; and a send's sources are its own slots, so
every `Move` it emits is the read of a named local. The register
ceiling is checked in every build (`Body::new`, `fresh`), as it is in
`lower.rs`, and a body past it compiles to the raising body of "Compilation
cannot fail"; the shape check does not run on a body it replaced. (The
optimized lowering's call-site counter is checked in every build too; see
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
The switch itself is `LoweringArgument` in `crates/ns-cli/src/main.rs` and,
for `disasm`, its copy in `crates/ns-analyzer/src/main.rs`, each converted
into `ns_runtime::Lowering`. `ns-next` passes it to `program::load_program`,
which `run` and `build` go through for every module of a program, the
manifest included; `ns-next-analyzer disasm` compiles one source file through
`Interpreter::compile_module` on an interpreter bootstrapped with the switch,
takes the switch as an `Option` and refuses it for an artifact, which
carries the code it was built with.

A command that dropped the switch would compare the optimized lowering with
itself, and no program's output would show it. The tests therefore read the
code instead: `disasm`'s listing (`crates/ns-analyzer/tests/lowering.rs`), and
the inspector's full listing of an artifact `build` wrote
(`crates/ns-cli/tests/lowering.rs`), have no branch under the reference
lowering and one under the optimized. `run` writes nothing to read; it is
covered by handing the switch to the same `program::load_program` call.

### The harness

- `crates/ns-cli/tests/run_goldens.rs` and `golden_round_trips.rs` run every golden
  under both lowerings against the same recorded answer. Under the reference
  lowering `golden_round_trips.rs` also takes the encoder and decoder over code with
  no jumps and many registers.
- `run-corpus` runs every row four ways — `run` of its generated application,
  and `build` then `load` of it, under each lowering — with no switch, because
  a gate must not be optional. A row fails if any of the four differs, and the
  report names which. The gate's counts stay per row.
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
value as it is, and leaves for the two loops, which signal `NonBooleanReceiver`
for a non-Boolean loop condition); a mirror send's failure block runs the same code
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
factory, initializer level and superclass clause — except a block activation
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
  omits exactly these activations and no others. On a non-Boolean receiver
  both lowerings make the same real send with real closures, and from there
  count alike; a non-Boolean loop condition signals its `NonBooleanReceiver`
  from the same depth under both. That holds for an
  instance of a user subclass of `True`, which reaches `True`'s quick methods:
  they mark the block as a control-flow step only for a Boolean receiver, so
  its block is counted under both lowerings
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
  holds for every program either lowering compiles, and a chain of exempt
  activations is then bounded by how deeply blocks nest in the artifact —
  which the decoder caps at the parser's ceiling of 128
  ([image-format.md](image-format.md), "Verification"), so an artifact stacks
  no more exempt activations above a counted one than source can. A block that
  fails the condition is counted like any other.

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
and its body arms, for a factory, an initializer level and a superclass
clause, record `counted: true`. The flag
rides on the stack frame, and the push refuses a counted push at the limit:
`Mutator::push_above` and `Mutator::push_on_slots` each compare the depth,
whichever of the two `Mutator::push_placed` chooses. A block is exempt when it carries
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

## Readers of compiled code take the mixin

Compiled code lives on a module's mixin tree, so what reads only the code
takes mixins, never a class object. `image::encode_code_table`,
`image::mixins::plan` and `image::mixins::encode` take the module roots'
mixins and no `Heap`. `ns-next-analyzer disasm` takes the root declaration from
`Interpreter::compile_module` for a source -- compiled with the chosen
lowering exactly as `load_module` compiles it, but applied to nothing -- or
from `LoadedModule::declaration` for an artifact. None of them holds a
handle, so none can read one through a heap that did not mint it
([memory.md](memory.md), "A handle is space-relative").

## Decisions

- **`Instruction` is a Rust enum, the vocabulary every lowering and tool
  speaks; a body is stored and run in eight-byte slots** (2026-10-02),
  encoded as it is emitted and read back through an exact view, since the
  loop-over-locals profile showed what dispatching the enum cost
  ([execution.md](execution.md), "The compact instruction encoding").
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
- **`whileTrue:` with a non-Boolean condition signals `NonBooleanReceiver`**
  (`SignalNonBoolean`) rather than falling back to a re-send, because a re-send
  would duplicate the condition's side effects and the inlined loop would
  diverge from the native one.
- **Code lives on the mixin and `InitializeSlot` is mixin-relative**, because one mixin
  backs many applications with different inherited layouts.
- **Instantiation is compiled code: a synthesized factory body and three
  initializer bodies per mixin, reached by five dedicated instructions**
  (decided with the user on 2026-10-22; [object-model.md](object-model.md),
  "Instantiation", "Decisions"). The clause messages' arguments are lowered
  inline into the initializers, as the references lower them, and the
  factory body is synthesized from the arity alone, so no image can carry
  one; no initializer is in a method table, so no message reaches one
  ("The three initializer bodies").
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
