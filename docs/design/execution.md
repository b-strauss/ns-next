# Execution — activations, the VM

> **Status:** built, including the trampoline; nothing re-enters the run loop
> from Rust, the loop polls a safepoint, each instruction it executes is
> one entry of the step state, and the sends it executes go through inline
> caches, except the reference lowering's.
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> arena, the `ActivationReference` and `FrameReference` handles, every read and write of an
> activation), `heap/world.rs` (the step state at rest, and its entry),
> `types.rs` (`Frame`, `Continuation` and the state its variants
> carry), `vm.rs` (the run loop and its safepoint poll), `interpreter/` (the
> inline caches, the send decisions, `Step`, every push, the depth limit, the host entry, the
> safepoint's slow path), `builtins/boolean.rs`,
> `builtins/block.rs` and `builtins/collections.rs` (the natives that answer
> trampoline steps), `crates/ns-cli/src/main.rs` (the interpreter thread and
> its stack).

This is the consumer side. What produces the bytecode — the instruction set,
lowering, register allocation, inlining — is [compiler.md](compiler.md).

## One engine

The bytecode VM is the only engine. A module's code is compiled when the module
is loaded, or decoded from an artifact, and every Newspeak-level body — method,
block, initializer, superclass clause — runs as compiled code in the run loop,
`vm::run`.

The Milestone 1 tree-walking interpreter, which ran beside the VM as a
differential oracle, was retired on 2026-09-11. A recursive AST evaluator keeps
its computation on the Rust stack, so it could take part in none of what the
trampoline ("The trampoline" below) exists for — suspending and
serializing a running computation, debugger stepping, edit-and-continue on a
live activation — and it constrained the design wherever it still had to agree.
The oracle was also blind to defects in the orchestration the two shared: four
of the six silent defects Milestone 3's review found were identically wrong
under both.

Behaviour is protected by recorded answers, which both lowerings must produce:
every `goldens/<category>/*.ns` against the exit status, stdout and stderr
recorded in the `.toml` beside it (`crates/ns-cli/tests/run_fixtures.rs`), and
`run-corpus` over the runnable corpus. The answers recorded before the
retirement are verdicts the oracle and the VM agreed on. A change that alters
one is a behaviour change, to be justified as one — never recorded because the
code now produces it. The reference lowering ([compiler.md](compiler.md), "The
reference lowering") is the differential oracle that replaced it.

## Activations

A Newspeak-level activation is never represented only by Rust locals or the Rust
call stack. A running activation is an entry on one **activation stack** that the
`Heap` owns. A first-class `Frame` in the `Heap`'s frame arena exists for it only
once something must hold it past the call that runs it — today, only a block
being created.

### The model, and the one place ns-next differs from it

The model is Cog's, and Primordial Soup implements it for Newspeak
(`prior-art/primordialsoup/vm/interpreter.cc`). Frames live on a stack and carry
an empty activation slot. `EnsureActivation` allocates the heap `Activation`
only when something asks for one — `PushClosure`, `thisContext`, an error — and
*marries* it to its frame, so that reading the activation reads the live frame.
`HasLivingFrame` detects an activation whose frame has since returned. Cog does
the same: `marryFrameSP` on closure creation, `isWidowedContext` for the dead
case (`prior-art/opensmalltalk-vm/src/spur64.stack/interp.c`).

One difference decides the shape of this design. Cog's and Primordial Soup's
closures **copy** the values they capture and box the variables they write
(`pushClosureNumArgs:copiedValues:` and `pushRemoteTemp` in Cog; `PushClosure`
and `PushIndirectLocal` in Primordial Soup), so nothing ever reads a dead
frame's temporaries and a widowed context needs none of them. ns-next's closures
read an enclosing activation's locals **by reference**, through
`captured_frame`, including after that activation has returned — a counter block
returned from the method that declares `n` goes on writing `n`
(`goldens/language/closure_variables.ns` pins this). So a reified frame here keeps
its registers when its activation returns ("Pop" below). Removing that need is a
step of its own, "Planned: closure conversion" below.

### Representation

```rust
// heap.rs -- every field private to the module
struct ActivationStack {
    values:  Vec<Value>,       // every live activation's registers, contiguously
    entries: Vec<StackEntry>,  // one per live activation, innermost last
    counted: usize,            // how many entries count against the depth limit
}

struct StackEntry {
    receiver: Value,
    current_class: Option<ClassReference>,
    kind: FrameKind,           // Closure(ObjectReference): the closure object, whose payload
                               // carries home and captured_frame
    base: u32,                 // first register in `values`
    argument_count: u32,
    program_counter: usize,
    slot_base: usize,
    code: Rc<CompiledCode>,    // the body it runs, which the loop resumes it in
    then: Continuation,        // what the loop does with its value
    counted: bool,             // whether it counts against the depth limit
    reified: Option<FrameReference>, // set once the activation is married
}

struct FrameSlot {             // one entry of the frame arena
    frame: Frame,              // types.rs; public fields, and what an image records
    live: Option<u32>,         // the stack entry this frame is married to
}

pub struct ActivationReference {
    index: u32,
    #[cfg(debug_assertions)] origin: u32,
    #[cfg(debug_assertions)] generation: u32,
}
```

- **The stack belongs to the `Heap`**, because `heap.rs` is already the one place
  that stores activations, and because it is half of the collector's eventual
  root set ([memory.md](memory.md)).
- **A push** (`Heap::push_activation`) copies the arguments into `values`, fills
  the temporaries with nil and appends a `StackEntry`. Both vectors keep their
  capacity across pops, so once they have grown to the program's depth a push
  allocates nothing. Every activation the runtime pushes goes through one
  function, `Mutator::push`, which refuses a push past the depth limit ("The
  depth limit" below) before it reaches the heap, so no new push site can skip
  the check. It is called from one place, `Mutator::push_step`, for the
  method, block or body a decision answered. An activation nothing reifies
  leaves nothing behind; only a block the compiler did not inline reifies one,
  since an inlined `ifTrue:`/`and:`/`whileTrue:` argument creates no closure.
- **Register access through an `ActivationReference`** is `values[entries[i].base +
  r]`, with no branch on the handle's kind or on an args/temps split; a release
  build still bounds-checks both slice indexes. A debug build also checks that
  the register lies inside the entry's own window: the neighbouring
  activation's register is in bounds of `values`, so reading it would not
  otherwise fail. An entry's `base` is fixed for its whole life and everything
  is addressed by index, so `values` reallocating as it grows moves nothing
  anyone holds.
- **Two handle types, not one tagged handle.** `ActivationReference` names a live
  stack entry; `FrameReference` names a reified frame in the arena and is the only
  activation handle anything *stores*: `ClosureData.captured_frame` and `home`,
  `Frame.home` and every image record take a `FrameReference`, so storing a stack
  handle is a compile error rather than a convention. A tagged `FrameReference` would
  have touched only `heap.rs`, but it would put a branch on every register
  access and leave "a stack handle was stored" to a debug-build assertion — the
  silent failure class the arena's private index was built to exclude.
- **An `ActivationReference` is 4 bytes in a release build.** In a debug build it
  carries `origin`, like every handle, and a `generation` drawn from a counter at
  each push and recorded on the entry. A handle read after its entry was popped
  and the index reused would otherwise answer a different, perfectly valid
  activation; the generation check turns that into a panic at the read.
  `Interpreter::run_from_host`, the one host entry, debug-asserts that it leaves
  the stack as deep as it found it, because a leaked entry is otherwise silent:
  a later activation would sit on top of a dead one's registers.
- **A reified frame is a `FrameSlot`**: the `Frame` itself, and the entry it is
  married to. `Frame` is declared in `types.rs` with public fields, because the
  image encoder and decoder build and read whole records; the marriage is kept on
  the slot instead, so that nothing outside `heap.rs` can read or forge it. While
  `live` is set, the frame's own `arguments`/`temporaries` are empty and a register read or
  write through the `FrameReference` (`Heap::frame_local`/`frame_set_local`) reaches the
  stack slot. That branch exists only on the path from a block to its method's
  variables — `read_outer_local`/`write_outer_local`, behind the VM's
  `LoadOuterLocal`/`StoreOuterLocal`.
- **The accessor prefix names the handle.** `activation_*` reads or writes a live
  activation through an `ActivationReference`; `frame_*` a reified frame through a
  `FrameReference`.

### Lifecycle

**Reify.** `Heap::reify(activation) -> FrameReference` is idempotent: the first call creates
the arena `Frame` — receiver, `kind`, `current_class`, `program_counter`, and `home` when the
activation is a closure's — marries it to the entry, and records it in
`entry.reified`; every later call answers the same handle. Its caller is closure
creation, the VM's `MakeClosure`: directly for the new closure's
`captured_frame`, and through `home_of` for its `home`. `home_of` passes a
closure activation's own `home` on and reifies any other activation as its own
home. Every block creation reifies the activation that creates it, whether or
not the block reads one of its variables, because `captured_frame` is also where
`Mutator::push_step` reads the block's receiver and current class from.

**Pop.** The run loop pops every activation it runs, on *every* exit, error
paths included (`vm::run`), and only the top entry can be popped. Popping hands
back the entry's continuation (`Popped`), which the loop applies to the
activation's value ("The VM" below). If the entry was reified, its frame is
**divorced**: the registers are copied into the frame's `arguments`/`temporaries`, its `program_counter`
takes the entry's last stored one, `live` becomes `None`, `uncontinuable` becomes
true and `sender` becomes `None`. Then `values` is truncated to the entry's
`base`. An entry that was never reified leaves nothing behind, so storage for
activations nothing captured is bounded by the deepest the program has been
rather than by every activation it has ever made.

**Sender.** A live activation's sender is the entry below it — structural, not a
stored field, so no call site can forget to pass it on, at a block boundary or
anywhere else. Nothing reads the sender of a live activation yet, so there is no
accessor for it; the debugger will add one. A dead frame's `sender` is `None`:
Smalltalk's semantics for a returned context, and what Primordial Soup does on
return (`zap->set_sender(nil)`) and lazily in `HasLivingFrame`. `Frame.sender`
remains because the image format records one per frame, so only a frame the
decoder rebuilt can have one set.

**Only a `FrameReference` is ever stored.** An `ActivationReference` is valid until its entry
is popped, and the next activation at that depth reuses the index. Everything that
outlives a call — a closure, another frame, an image record — holds a `FrameReference`.

### Signatures

- Wherever a function means *the running activation* it takes an
  `ActivationReference`: `vm::run` and the loop's helpers,
  `inline_cache::send_lexical` and `send_super`, `sends::decide_lexical_send`,
  `sends::decide_super_send`, `read_local`/`write_local`,
  `read_outer_local`/`write_outer_local`, `raise_return`.
  `FrameReference` stays for closures, `Frame.home`, the image, and `frame_at_depth`
  from depth 1 on: the first hop goes from a closure activation to its
  `captured_frame`, the rest follow `FrameReference` links.
- **No send or block invocation takes a sender to record.** `Interpreter::send`,
  `decide_send`, `builtins::execute` and the `builtins/` modules have
  no `sender` parameter. It was removed rather than retyped to an `ActivationReference`: its only
  use was to fill `Frame.sender`, and once a live activation's sender is the
  entry below it, nothing read it. No native takes an activation handle at all.
  The one function handed the sending activation is `Mutator::push_step`,
  which reads its reified frame for the depth count's exemption and stores
  nothing.
- `vm::run` receives the `ActivationReference` of an activation its caller has just
  pushed — its *base* — and pops it, with everything it pushes above it, on
  every path out.
- The image encoder only ever runs between sends, when nothing is live:
  `image::snapshot` refuses to start while any activation is on the stack, and
  `Heap::frame_snapshot` debug-asserts that no encoded frame is married. The
  decoder builds frames that are never married. The format knows nothing of the
  stack: a frame record describes a reified frame, and nothing else is
  serialized ([image-format.md](image-format.md)).

### What an activation carries

`current_class` is required by §3.6: the current class is the class that actually
**defines** the running method, not the receiver's concrete class, and it is what
`self`, `outer` and `super` sends inside that method resolve relative to. It is
set by the dispatch step that pushes the activation and never derived from the
receiver at read time, which is what makes chained `super` sends through a deep
hierarchy resolve one level at a time.

The registers — arguments first, then temporaries — are addressed as described in
[compiler.md](compiler.md). Every activation is pushed with its body's whole
register file, since the code is read before the push — the initializer's
included — and a live window never grows: an entry below the top could not
grow anyway, since its window ends where the next one's begins. A dead frame's
`temporaries` is a growable `Vec` (`Heap::frame_grow_temporaries`). Adding a local slot to a
running method — growing its register file, initializing the new entries to nil
and remapping the `program_counter`, the edit-and-continue requirement — therefore means
taking the activation off the stack first; see "How this meets the liveness
requirements" below, which is not built.

`kind` distinguishes a method, closure, initializer, superclass-clause,
superclass-message and top-level activation. An initializer activation must be
able to name the code it is executing so that `program_counter` is interpretable, since the
statement→program-counter table is reached through its kind. A stack entry also holds its
`code` directly, because the loop resumes a caller in it after a callee returns;
`then`, the `Continuation` the loop applies to its value when it is popped; and
`counted`, whether it counts against the depth limit ("The depth limit" below).

`program_counter` is an instruction index into the activation's compiled body.

`feedback_base` is where the activation's code family starts in the heap's
inline-cache table ([dispatch.md](dispatch.md), "Code families"): a send's
entry is that base plus its call-site number. `push_step` sets it — from the
cache entry that chose a method, from the closure for a block (a fresh range
for a decoded closure, which has none), from the family map otherwise — and
it is `NOT_ASSIGNED` only for a family whose range
could not be allocated, whose sends then run uncached. It is a number, not a
handle, and it is never serialized.

**The chain of running activations is walkable independently of the Rust
stack**: it is the activation stack, innermost last. That is what the debugger
will reflect over and what a precise collector will scan. An image snapshot
serializes reified frames only, and is taken when nothing is running
([image-format.md](image-format.md)).

## The VM

Every body a program runs hangs off the declaration it belongs to — a method's
off its `MixinMethod`, an initializer's and a superclass clause's off the
`Mixin` — so what dispatch finds is what runs; nothing looks code up beside it.
Everything in `interpreter/` decides *what* to run; `vm.rs` runs it.

`vm::run(interpreter, base)` is the run loop. It is handed an activation its caller
has just pushed — its *base* — and runs it and everything it activates, one
instruction at a time (`execute`, a `match` over `Instruction`), with the running
activation, its code and its `program_counter` in locals. **Each instruction is
one entry** (`Interpreter::enter`, [memory.md](memory.md), "The brand"):
`execute` runs inside it with a `&mut Mutator`, and `run` itself — its locals,
the switch to another activation and the safepoint poll — is outside every
one, so none of `run`'s locals can name a brand. A run ends inside its last
entry: `vm::finish` or `vm::unwind` roots its answer for the host
(`Mutator::to_host`), and `run` answers that `HostResult`, never a branded
value. It never recurses: a send pushes
the callee and switches to it, a return pops the callee and switches back to
the caller. It pops everything it ran, `base` included, on every path out — a
return from `base`, an error, or a non-local return aimed below `base` — so the
stack is exactly as deep afterwards as it was before `base` was pushed. The
depth `base` sits at, the run's *floor*, is how the loop tells its own
activations from its callers'.

- **A send** (`vm::send`) gathers its arguments from the caller's registers
  into the step state's reusable buffer, stores the caller's `program_counter`, and asks
  `interpreter/` what the send means: `inline_cache::send_public`,
  `send_lexical` or `send_super`, which answer from the call site's cache or
  run the lookup ([dispatch.md](dispatch.md), "3b — The caches"), or, for
  the reference lowering's `SendInlinable`, `Mutator::decide_send`. Each
  answers a `Step`. `Step::Value` — a native leaf, a slot access, a nested class or an
  instantiation that runs no code — is written to the send's `destination`,
  and the caller goes on. Anything else is an activation: `Step::Method`,
  `Step::Block`, or one of the bodies an instantiation or a nested-class
  accessor runs, `Step::Initializer`, `Step::InitializerThen`,
  `Step::ClauseMessage` and `Step::SuperclassClause`. It is
  pushed by `Mutator::push_step`, which checks a method's or block's arity
  and the depth limit, and the loop switches to the new activation at `program_counter` 0,
  its window filled straight from the buffer. A send allocates no vector once
  the buffer has grown to the widest one.
- **A return** (`vm::finish`) pops the activation and applies its
  continuation. `Continuation::Deliver`, every send's, hands the value to the
  caller: `resume_caller` reads the `program_counter` the caller stored at its send, writes
  the value to that send's `destination` and continues at the next instruction — or,
  when the popped activation was `base`, the loop leaves `run` with it. Every
  other continuation — a control-flow native's, `do:`'s, the nested-class
  accessor's, an instantiation's — is applied by `Mutator::resume`, which
  calls into `builtins/` or `interpreter/`; it may answer a value, or another
  activation — a native loop's next block, `do:`'s block for the next element,
  an instantiation's next body, or the method a non-Boolean condition's
  `mustBeBoolean` send finds — which `Mutator::push_continuation` pushes
  in the popped activation's place ("The trampoline" below).
- **A signal** — a Newspeak error, or a non-local return — goes to
  `vm::unwind`, which pops what it has to and either returns from the
  non-local return's target or leaves `run` with the signal ("Non-local
  return" below). Every activation it pops hands its continuation to
  `Mutator::abandon`, which undoes what that continuation had begun.

Four kinds of body run, and each reaches the loop one way:

1. **Method body** — pushed by `push_step` for a `Step::Method`. A method
   activation catches a `^` that targets it (`Mutator::catches_return`).
2. **Block body** — pushed by `push_step` for a `Step::Block`, which names the
   closure object; the activation runs the `code` of that object's
   `ClosureData`. `value`, the control-flow natives and `do:` answer the block
   as a step.
3. **Instance initializer** — `descend_levels`, one per initializing level of
   the superclass chain, answers that level's compiled initializer: its own
   slot initializers and initializer expressions. While a level below it still
   has code to run, it is answered as a `Step::InitializerThen`, whose
   activation continues with `Continuation::Instantiate`; the walk's last
   initializer is a `Step::Initializer`, whose activation delivers its own
   value, the instance ("No re-entry from Rust" below). A class with no
   compiled initializer and no slots is a no-op; a user class whose mixin
   carries no initializer is refused on load and panics if built any other way
   (`Mutator::initializer_body`).
4. **Superclass clause and its message arguments** — the nested-class
   accessor's clause is a `Step::SuperclassClause`, continuing with
   `Continuation::NestedClass`; an instantiation's clause message is a
   `Step::ClauseMessage`, continuing with `Continuation::Instantiate`.

Rust enters the loop in one place, `Interpreter::run_from_host`, for a host's
`send` or `instantiate`, and only from outside it: it pushes what the decision
answered and runs `vm::run` on it. No decision, native or continuation runs
code from Rust, and a debug build checks that the loop is never entered while
it runs ("No re-entry from Rust" below).

**The safepoint.** `run` polls one flag the heap owns (`vm::poll`) at every
switch to another activation — a push, or a caller resumed — and at every
backward jump, so every call and every loop iteration passes one, a loop that
makes no send included; a forward jump does not poll. When the flag is set the
poll calls `Interpreter::safepoint` (`interpreter/safepoint.rs`), out of line,
which is where a collection will run ([memory.md](memory.md), "Where a
collection can happen"); today only the debug-build stress mode sets it, and
moves every object and every reified frame there. That is sound because of
what the loop's locals are, which the compiler holds it to, since each
instruction's entry has ended before the poll: the running `ActivationReference`, which indexes the
stack, and no stack entry moves; its `Rc<CompiledCode>`, which holds no
handle; its `program_counter`; and the run's floor, a stack depth. Every value the loop
works on is read from a register or an operand when an instruction needs it
and written back before the next, so between two instructions every handle is
in a register on the stack, in a stack entry or the continuation it owns, in an
arena record or in a root, where the tracer rewrites it. The reusable argument
buffer, which a send fills and empties within one instruction, is empty at
every safepoint.

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** It pushes, pops, switches, writes
registers and reads instructions' operands; which body runs next is always
decided in `interpreter/` or `builtins/`, a native loop's next block included.
Every error a program can observe is produced there. That keeps dispatch in
one place, which the reference lowering and a later JIT will reach unchanged —
and it is why `CLAUDE.md`'s "`vm.rs` holds no dispatch logic" forbids copying
anything out of `interpreter/` into `vm.rs`.

**The inline caches keep it that way** ([dispatch.md](dispatch.md), "3b —
The caches"). `run` holds the running activation's `feedback_base` in a local
beside its code, `Flow::Switch` carries it, and a send passes it, its code's
`family_size` and the instruction's `CallSite` to `interpreter/inline_cache.rs`,
which probes the cache there; `MakeClosure` copies it into the closure. `vm.rs`
learns three numbers and no dispatch. **The per-instruction step is forced
inline** (`#[inline(always)]` on the closure `run` hands to
`Interpreter::enter`): every cached send inlines its hit path into it, and
past LLVM's inlining threshold the step becomes a function of its own, so
that every instruction pays a call and returns its `Flow` through memory —
6.6–13.6% on every benchmark when it happened ("What the inline caches
moved"). The step sits near that threshold; anything added to a send's hit
path lands in it.

Block invocation is not special-cased into an opcode: `Block value` is still a
native method, which answers the closure as a step; the closure's activation is
pushed above the one that sent `value` — its sender, by position — and a wrong
argument count is still the same error.

## Where `program_counter` is stored

An activation's `program_counter` is written **only by the instructions that can hand the
activation to an observer**: every send, since a nested activation runs above it;
`MakeClosure`, since the activation escapes into the object graph — the store
comes just before `reify`, which copies it into the frame it creates; and
`NonLocalReturn`, since control leaves with a signal.

The stores go to the stack entry (`Heap::set_activation_program_counter`), so they keep the
activation's `program_counter` current for as long as it runs; a frame married to it holds no
`program_counter` of its own that anything reads meanwhile. When the activation returns, the
divorce gives the frame the last `program_counter` stored — often a later send's, not the
`MakeClosure` that reified it.

The loop itself reads it back. While a callee runs, the caller's only record of
where it stopped is the `program_counter` its send stored; when the callee returns,
`vm::resume_caller` reads that `program_counter` (`Heap::activation_program_counter`), writes the value to
the destination register of the send it names, and resumes the caller at the
next instruction.

Nothing can read `program_counter` while a straight-line instruction executes, because the
loop never switches activations between them. Keeping the store out of the
straight-line arms takes a write off the hot path for a value nothing observes
there.

Whether a debugger will force `program_counter` at every instruction is an open question for
the milestone that builds one; it is a deliberate trade, not an oversight.

## Non-local return

A closure's `home` is the reified frame of the method, initializer,
superclass-clause or superclass-message activation that lexically encloses it.
`Signal::Return` carries an `ActivationReference`, its target, and `raise_return` picks
it: for a closure activation, its closure's `home` — the stack entry that frame is
married to (`Heap::frame_activation`) while its activation is still running, and
`cannotReturn` if it is dead, either divorced because the activation returned or
built by the image decoder, which never marries a frame. For any other activation
the target is the activation itself.

The loop unwinds the activation stack to it (`vm::unwind`). A target that is
still live (`Heap::activation_is_live`), sits at or above the run's floor and
catches a return (`Mutator::catches_return`: a method, a superclass
clause or its message) ends the unwind: every activation above it is popped,
and the target returns the value through the ordinary return path, so a
reified frame is divorced exactly as on any return. The popped activations'
continuations are not applied, so a `^` out of an `and:` block, a native
loop's body or a `do:` block abandons the coercion or the loop; they go to
`Mutator::abandon` instead, which unmarks a nested class's creation in its
cache slot and drops every other. Any other signal — an error, or a
return whose target catches none — pops every activation of the run, `base`
included, through `abandon` in the same way, and leaves `vm::run` with it, for
the host that started the run.

Because there is one stack, a live target is always below the activation
raising the return, and nothing is pushed while the signal propagates, so the
target's index cannot have been reused by the time the unwind compares it
(`ActivationReference::same`). In a release build that is also all
`Heap::activation_is_live` rests on — it compares the index with the stack's
depth, where a debug build also checks a generation — so anything that runs
code while a signal propagates, an `ensure:` or an `on:do:` handler, would
have to carry the generation into every build first, or a release build would
land the return in whichever activation reused the target's index.

**An initializer home follows the same rule.** A block created during
initialization — `| blk = [ ^2 ]. |` — whose `^` runs after initialization has
finished raises `cannotReturn`, as `newspeak-spec.pdf` §7.2 requires, because
the initializer's frame was divorced when its activation returned. A `^` into an
initializer that is still running escapes: an initializer activation catches
no return, so the unwind pops it with everything else down to the floor of the
host's run and leaves with the signal, and it reaches the top of the entry
send as "internal: non-local return escaped the entry send", a known gap
(`crates/ns-runtime/tests/trampoline.rs` pins it under both lowerings).

A method body's own `^` is an ordinary `Return` and raises no signal. The
observable discipline — a block invoked after its home has returned gets
`cannotReturn` — comes from the pop, which divorces a reified home on every exit
path, without a signal round-trip on the common path.

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvaluationResult`.** `Signal`
deliberately carries no `Debug` impl of its own -- every field it holds
could support one today, but leaving `Signal` without one means
`EvaluationResult` (`Result<Value, Signal>`) has none either, and
`Result::unwrap`/`expect` require `E: Debug`. Calling either, or writing
`{:?}` on an `EvaluationResult`, is therefore a compile error, not a lapse a
reviewer has to catch. The same holds for the `HostResult` a host receives
(`Result<Root, HostSignal>`, [memory.md](memory.md), "Host roots"): `Root`,
`HostSignal` and `HostError` have no `Debug` either. Tests unwrap through a
local `expect_ok` helper.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping and
compiling excluded — and reports the median of ten timed runs after a discarded
warm-up. Its three fixtures each run a 50,000-iteration `whileTrue:` loop, which
the compiler inlines: `bench_loop.ns` sums the counter, `bench_send.ns` adds a
user-method send per iteration and `bench_block.ns` a method that creates and
invokes a block. On a release build, on 2026-09-16, with the inline caches,
their medians were about 10.1, 11.1 and 19.4 ms ("What the inline caches
moved" below).

Never judge performance from a debug build.

Every figure in this section is a median of a whole-program benchmark on one
machine. Only numbers measured against each other in one sitting are compared
here; a figure carried across sessions says more about the machine's load than
about the code.

### What a send costs, and where it goes

Milestone 4's design rests on a decomposition rather than on the aggregate. The
decomposition below was measured on the pre-arena `Rc<RefCell<_>>`
representation, and it is what chose the milestone's order. "What the activation
stack moved" below re-takes it against the arena and against the stack, in one
sitting; "What the arena moved" re-measures the aggregate only.
Three separate fixtures, shaped like `bench_loop.ns` but not it -- each with
the body `[n < 50000] whileTrue: [<first statement>. n := n + 1]` -- isolate
the pieces, because each differs from the next by exactly one send. Row
one's `sum := sum + 1` reads a literal where `bench_loop.ns` itself reads
the variable `n` (`sum := sum + n`), so its 19.02 ms is a separate
measurement that happens to land close to `bench_loop.ns`'s own ~19.6 ms
at the time, not the same run reported twice:

| First statement | Method it calls | bytecode median | ns/iteration |
|---|---|---|---|
| `sum := sum + 1` | — | 19.02 ms | 380.5 |
| `sum := self tick: sum` | `public tick: x = (^x)` | 22.44 ms | 448.7 |
| `sum := self tick: sum` | `public tick: x = (^x + 1)` | 27.28 ms | 545.7 |

Rows one and three each pay three native sends per iteration; row two pays two,
because its method body has no `+`. So the third minus the second is one native
`+` send: **~97 ns**. The third minus the first is exactly one user-method send,
both paying three native sends: **~165 ns**. The second minus the first, 68 ns,
is then what a user-method send costs beyond a native one. It is the difference
of the other two by construction, so it is not an independent check on them.

So **building an activation cost about 68 ns, roughly 41% of a user-method
send**, and it was a fixed cost: a two-argument send measured the same as a
one-argument one. What was in it was the representation, not the work: heap
allocations (the `Rc<RefCell<Frame>>` block and the `arguments` vector `collect`
builds), reference-count traffic on `sender`, `method`, `bindings` and the
defining class, and a `HashMap` lookup per invocation just to find the compiled
body. A method with temporaries also allocated a `temporaries` vector. Row two's
`^x` has none (`ns-next disasm` gives `tick:` one register, its argument, where
row three's has three), so its `temporaries` vector stayed empty and never allocated,
and row three's allocation landed in the ~97 ns attributed to `+`. On top of
that, every register read and write in the VM took a `RefCell` borrow, which
was spread across the straight-line instructions of all three rows rather than
isolated in any one of them.

Two caveats. `U - N` is an incremental difference, not a profiler attribution:
the native path pays a `match selector.as_str()` string comparison chain the
user path does not. And these are medians of a whole-program benchmark.

### What the arena moved

Replacing `Rc<RefCell<_>>` with an arena of 32-bit handles, for objects and
activations together (`docs/design/memory.md`, "The representation"), was
measured before and after in one sitting: the pre-arena binary was built from
this branch's merge-base into a throwaway worktree, and both binaries ran the
same `ns-next bench goldens/bench/bench_loop.ns` — ten timed runs after a
discarded warm-up — three times each, alternating.

| Run | Before, bytecode | After, bytecode |
|---|---|---|
| 1 | 19.38 ms | 16.45 ms |
| 2 | 19.54 ms | 16.82 ms |
| 3 | 19.60 ms | 16.53 ms |

**The VM gained about 15%**, from ~19.5 ms to ~16.5 ms. The three
before-figures and the three after-figures do not overlap, and the split is
larger than either group's own spread, so this is a real move rather than noise.

What moved is precisely two of the itemized costs above: the `Rc` box and its
reference-count traffic on `sender`, `home` and every shared handle, and the
`RefCell` borrow on every register read and write — which is why the gain is
spread across the straight-line instructions rather than concentrated in sends.

What did **not** move was the rest of the activation cost. `Frame.arguments` and
`Frame.temporaries` were still a `Vec` allocation apiece on every activation that had
arguments or temporaries, the `HashMap` lookup per invocation that finds the
compiled body survived, and building an activation was still an allocation rather
than an index bump. The activation stack took the two vectors and the allocation;
see "What the activation stack moved" below. The lookup went when compiled code
moved onto the declaration it belongs to (see "The VM").

### A 13% regression, found and recovered

Routing every `Object`/`Frame` access through `Heap` (`docs/design/memory.md`,
"The seam") was designed to cost nothing: `--release` inlines an accessor
call away, and each one does the same borrow-clone-drop the call site did
before. Migrating the lexical-send helper (`dispatch_lexical_send`, now
`sends::decide_lexical_send`) broke that measurably: `bench_loop.ns` went from 20.07 ms to 22.77 ms, about 13%
(13.45%).

The cause was `dispatch_lexical_send`'s head, which every implicit-receiver
send runs. Before the migration it took one `RefCell` borrow and read both
the receiver and the current class out of it; the migration replaced that
with two separate accessor calls, each its own borrow. `bench_loop.ns` sends
roughly 250,000 lexical sends per run, so the extra borrow pair cost on the
order of 10 ns each — enough, at that call count, to account for the whole
regression. A batched accessor restored the single-borrow shape and recovered
the cost in full; it is `Heap::activation_receiver_and_current_class` today.

The companion result is what makes the number legible rather than alarming:
the identical pattern — one extra `RefCell` borrow per call — also appeared
in `dispatch::walk`, on the per-inheritance-level path method lookup walks,
and cost nothing measurable there. The difference is not that one borrow is
expensive and the other cheap; it is executions per benchmark run. A lexical
send runs 250,000 times in this benchmark, an inheritance level far fewer.
The lesson generalized to the arena migration that followed: a per-access cost is
only worth chasing where the access count is large, and the way to tell
which sites those are is to measure, not to guess from the shape of the code.

### What the activation stack moved

Moving activations onto the stack was measured against the arena in one sitting,
on 2026-09-10: the before-binary was built from the branch's merge-base in a
throwaway worktree, and both binaries ran `ns-next bench` — ten timed runs
after a discarded warm-up — over five sources, three rounds, alternating
the binaries within each round. Three sources are fixtures: `bench_loop.ns`,
`bench_send.ns` (row three of the decomposition above, `tick: x = ( ^x + 1 )`)
and `bench_block.ns` (a method that creates and invokes a block per iteration).
The other two are the decomposition's rows one and two, which differ from
`bench_send.ns` by one statement each and are not fixtures. Medians per round,
and the middle of the three as ns per iteration (`median_ms * 1e6 / 50000`):

| Bytecode | Before (arena), rounds 1–3 | After (stack), rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.27, 16.50, 16.43 ms | 16.07, 16.22, 16.57 ms | 328.6 → 324.4 |
| `bench_send.ns` | 23.49, 23.46, 23.58 ms | 20.68, 20.51, 20.51 ms | 469.8 → 410.2 |
| `bench_block.ns` | 38.83, 38.99, 39.16 ms | 38.46, 40.84, 39.24 ms | 779.8 → 784.7 |
| row one, `sum := sum + 1` | 16.16, 16.18, 16.24 ms | 15.97, 15.98, 16.26 ms | 323.5 → 319.5 |
| row two, `tick: x = ( ^x )` | 19.18, 20.07, 19.28 ms | 17.84, 17.83, 17.90 ms | 385.6 → 356.9 |

The decomposition, re-taken from the middle medians (ns per iteration):

| | Row one | Row two | Row three | User send (3 − 1) | 2 − 1 | 3 − 2 |
|---|---|---|---|---|---|---|
| Bytecode, arena | 323.5 | 385.6 | 469.8 | 146.3 | 62.1 | 84.2 |
| Bytecode, stack | 319.5 | 356.9 | 410.2 | **90.7** | **37.3** | 53.4 |

**A user-method send fell from ~146 ns to ~91 ns, about 38%**, and
`bench_send.ns` from 23.5 ms to 20.5 ms, about 13%. The three
before-medians and the three after-medians do not overlap, and the per-round
differences agree with each other: 146.7, 145.7 and 146.8 ns before, 94.1, 90.7
and 85.0 ns after. What a user send costs beyond a native one (row two minus row
one) fell from ~62 ns to ~37 ns — per round 60.4, 78.0 and 60.9 before, 37.5,
37.1 and 32.8 after.

Row three minus row two fell as well, from ~84 ns to ~53 ns, although both
binaries run the same native `+`. That is the `temporaries` allocation described under
the decomposition: under the arena, row three's `tick:` allocated a temporaries
vector and row two's did not, so the arena binary's 3 − 2 carries that allocation
and its 2 − 1 leaves it out. The stack allocates for neither. That attribution is
an inference from the two bodies' register counts; no profile was taken. It also
means the arena binary's 2 − 1 understated what an activation with temporaries
cost, so 3 − 1 is the figure to compare.

`bench_loop.ns` and row one did **not** move: the compiler inlines `whileTrue:`,
so the loop builds no activation for the stack to make cheaper, and the medians
overlap.

**`bench_block.ns` did not move.** Every iteration creates a
block, so every iteration still reifies `bump:`'s activation into the frame arena,
and divorcing it copies its registers into two vectors when it returns. The stack
took the block's own activation out of the arena, yet the median did not move;
a profile taken later attributes that to allocation ("What the profiles show"
below).
What does show is memory: peak working set over this source fell from 966.9 MB
to 446.0 MB ([memory.md](memory.md)).

The caveats are the arena subsection's: whole-program medians, incremental
differences rather than a profiler attribution, and a native path that pays a
string-comparison chain the user path does not. The pre-arena figures above —
~165 ns a user send, ~68 ns an activation — come from another sitting and are not
compared with these.

### What retiring the tree-walker moved

The retirement was measured against the branch's merge-base in one sitting, on
2026-09-11, the same way: the before-binary built in a throwaway
worktree, `ns-next bench` over the three fixtures, three rounds, alternating the
binaries within each round. Bytecode medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 |
|---|---|---|
| `bench_loop.ns` | 16.58, 16.53, 16.43 | 16.57, 16.38, 16.51 |
| `bench_send.ns` | 20.75, 20.68, 20.83 | 20.67, 20.48, 20.44 |
| `bench_block.ns` | 39.40, 39.91, 39.04 | 37.65, 37.61, 38.48 |

Nothing got slower. `bench_loop.ns` did not move. `bench_send.ns` moved about
1% — the two groups just fail to overlap — which is at the edge of noise.
**`bench_block.ns` got about 4% faster**, from ~39.4 ms to ~37.6 ms, by more than
either group's spread. That source creates and invokes a block every iteration,
and the block is what changed: a closure no longer carries the resolver's
bindings map or a body enum, so creating one no longer clones a shared `Rc` and
invoking one no longer matches on the body, and the closure object it allocates
is smaller — an `Object` went from 64 to 56 bytes in a release build. That
attribution is an inference from what the change removed; no profile was taken.

### What step B moved

Step B, the behaviour-preserving cleanup that followed the retirement
([clean-up-with-one-engine](../superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md)),
was measured whole against its merge-base in one sitting, on 2026-09-12, the
same way: the before-binary built in a throwaway worktree, `ns-next bench` over
the three fixtures, three rounds, alternating the binaries within each round.
Bytecode medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.08, 16.17, 16.12 | 15.31, 15.32, 15.49 | 322.3 → 306.4 |
| `bench_send.ns` | 20.55, 20.28, 19.95 | 18.65, 19.06, 19.15 | 405.6 → 381.2 |
| `bench_block.ns` | 37.38, 38.29, 38.68 | 29.94, 29.74, 30.02 | 765.8 → 598.8 |

On every source the after-group lies entirely below the before-group.
`bench_loop.ns` got about 5% faster, `bench_send.ns` about 6%, and
**`bench_block.ns` about 22%**, from ~38.3 ms to ~29.9 ms. Every image the
fixtures build or snapshot is byte-identical between the two binaries (44
artifacts), as a change that preserves behaviour requires.

Each change that touched a hot path was also measured on its own, against the
commit before it, in its own sitting. Middle medians in ms, `bench_loop.ns` /
`bench_send.ns` / `bench_block.ns`:

- **Builtin classes indexed by discriminant** (a `Vec` indexed by
  `BuiltinClass` instead of a hashed map): 16.59 / 20.70 / 38.93 → 16.04 /
  19.90 / 37.44. Faster on all three, by 3–4%, with no overlap between the
  groups: a send to an integer or a boolean finds its class through that
  accessor, and so does every closure allocation.
- **The runtime's own symbols interned once, as constants**: 15.95 / 20.16 /
  37.46 → 16.03 / 20.07 / 37.71. **Nothing moved**; the groups overlap on every
  source, because none of the three fixtures reaches a changed site inside the
  timed send.
- **A closure behind an `Rc`**: 16.06 / 19.96 / 37.47 → 15.71 / 19.89 / 31.26.
  **`bench_block.ns` about 17% faster**: `value:` bumps a reference count
  where it used to copy `ClosureData` out of the heap and wrap it in a fresh
  `Rc`, and `MakeClosure` shares its parameter names instead of cloning them.
  `bench_loop.ns` moved about 2%; `bench_send.ns` did not move.
- **Compiled code on the declaration** (see "The VM"): 15.86 / 19.81 / 31.26 →
  16.14 / 19.56 / 30.59. `bench_send.ns` and `bench_block.ns` about 1–2%
  faster, with no overlap, from the hashed lookup of the compiled body that
  every user-method send no longer does. `bench_loop.ns`'s first three rounds
  did not separate the two groups; three more rounds put the after-binary
  lower each time, not slower.
- **Handles passed by value**: 15.58 / 18.75 / 29.67 → 15.41 / 19.13 / 29.93.
  **`bench_loop.ns` about 1% faster**: its after-range [15.23, 15.42] lies
  entirely below its before-range [15.46, 15.70] -- the groups just fail to
  overlap. `bench_send.ns` and `bench_block.ns` did not move. `bench_send.ns`'s
  first three rounds only just overlapped; three more narrowed the gap to
  0.17 ms, inside the before-group's own spread. A release build had already
  inlined the indirection away.

The per-change figures come from five different sittings, so they are not
compared with each other or summed into the whole; the whole-step table above
is the one comparison in a single sitting. The per-change attributions are
inferences from what each change removed; no profile was taken.

### What the trampoline moved

The trampoline ("The trampoline" below) was measured whole against the commit
it started from, `04e5fc0`, in one sitting, on 2026-09-12, the same way: the
before-binary built in a throwaway worktree, `ns-next bench` over the three
fixtures, three rounds, alternating the binaries within each round. Bytecode
medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.96, 16.40, 16.53 | 11.23, 11.27, 11.28 | 330.6 → 225.3 |
| `bench_send.ns` | 20.48, 20.33, 20.20 | 14.13, 13.95, 13.87 | 406.7 → 279.0 |
| `bench_block.ns` | 30.77, 30.17, 31.14 | 23.66, 24.38, 23.97 | 615.3 → 479.4 |

On every source the after-group lies entirely below the before-group, by far
more than either group's spread: **`bench_loop.ns` about 32% faster,
`bench_send.ns` about 31%, `bench_block.ns` about 22%.** The optimized lowering
emits the same instructions as before, so every `build` and `snapshot` artifact
of it is byte-identical between the two binaries: `compared 46 artifacts, 0
differing`, over the 23 fixtures. A reference-lowering artifact differs by
construction, since it now carries `SendInlinable`, and was not compared.

Each change on the hot path was also measured on its own, against the commit
before it, in its own sitting. They come from several sittings, so they are not
compared with each other or summed into the whole; the attributions are
inferences from what each change removed, and no profile was taken.

- **Deciding before running, still recursive.** Splitting every send into a
  decision (`Step`, `decide_send` and its two relatives) and a run first made
  every source 6–11% slower: each `Send*` arm made two out-of-line calls where
  it had made one. Answering a `Step::Value` in place, with only the method
  case behind a call, recovered part of it. Middle medians in ms,
  `bench_loop.ns` / `bench_send.ns` / `bench_block.ns`: 15.50 / 19.17 / 30.89
  before, 17.18 / 21.14 / 32.67 split, 16.67 / 20.11 / 31.72 with the value
  answered in place — still 3–8% slower, accepted as the cost of a shape the
  loop was about to consume.
- **The loop.** Against the commit before the split and against its own
  parent: 15.76 / 19.22 / 30.26 and 16.81 / 20.34 / 31.69 → 11.57 / 14.38 /
  25.99. Faster on every source against both, by about 27%, 25% and 14% against
  the commit before the split. `bench_loop.ns`, which makes no user-method send
  in its loop, gained most: every native-leaf and slot send used to allocate a
  `Vec` for its arguments, and the reusable buffer removes that allocation.
  `bench_send.ns` also loses the Rust recursion and the second call per method
  send.
- **Blocks as steps.** 11.45 / 14.30 / 26.12 → 11.15 / 13.91 / 23.58.
  **`bench_block.ns` about 9% faster**: `value:` is a push, where it was a
  native call, a nested run of the loop and a pop. `bench_loop.ns` and
  `bench_send.ns`, which activate no block in their loops, moved about 2.5%,
  outside both groups' spread but not explained by anything removed from their
  paths — code layout, most likely, and not something to bank on.
- **The two bounds** — L, and a bound on re-entries of the loop from Rust that
  the trampoline still had and that went with the re-entries ("No re-entry
  from Rust" below). The first version marked a control-flow native's block
  with a `bool` field on `Step::Block`, and measured 4–8% slower on every
  source, `bench_loop.ns` included, whose loop activates nothing. Taking the
  pieces out one at a time found the field itself to be the cause, with
  nothing reading it; the counting logic, the counted flag on a stack entry,
  the arm split in the loop and the iterative superclass walk each measured
  nothing:

  | Variant | `bench_loop.ns` | `bench_send.ns` | `bench_block.ns` |
  |---|---|---|---|
  | parent commit | 11.1 | 13.9 | 23.4 |
  | the bounds, marked by a field | 12.0 | 14.7 | 24.4 |
  | the same, with the counting, the counted flag and the iterative walk taken out | 12.0 | 15.1 | 24.5 |
  | and the field taken out too | 11.2 | 14.1 | 23.4 |
  | the bounds, marked by `StepArguments::ControlFlow` | 11.2 | 14.1 | 23.8 |

  `size_of::<Result<Step, Signal>>()` is 56 bytes with and without the field,
  so the likely mechanism is the niche a `bool` offers for `Step`'s
  discriminant, changing how every decision's result is encoded and tested;
  that was not checked at the assembly level. With the mark on the arguments
  instead, two sittings put every source level with the parent — 11.11 /
  13.97 / 23.46 against 11.23 / 14.07 / 23.60 in the first — `bench_loop.ns`
  about 1% higher in both, inside the after-group's spread.
- **Caching the running activation's register base, measured and reverted.**
  The loop re-reads an entry's `base` on every register access; holding it in
  a local beside the running activation, refreshed on every switch, measured
  no source faster and none slower over six rounds (group ranges in ms, before
  against after: `bench_loop.ns` 10.98–11.32 against 10.97–11.50,
  `bench_send.ns` 13.75–14.16 against 13.71–14.20, `bench_block.ns`
  22.95–23.81 against 23.13–24.00). A change that makes nothing faster is not
  kept, so it was reverted: one index into a hot entry is already cheap enough
  that avoiding it does not show.

The interpreter thread's stack, `INTERPRETER_STACK_BYTES = 8 MiB`
(`crates/ns-cli/src/main.rs`), is sized for what still recurses on the Rust
stack. Nothing re-enters the run loop from Rust ("No re-entry from Rust"
below), so that is the front end and the compiler, whose depth the parser's
nesting ceiling bounds. The suite runs the deepest nesting the parser accepts
through every stage on libtest's default 2 MiB threads in a debug build —
`the_deepest_module_the_parser_accepts_round_trips`,
`the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs` and
`the_deepest_literal_array_the_parser_accepts_survives_a_round_trip` in
`crates/ns-runtime/tests/image.rs` — and the thread gets four times that.

The suite (`cargo test --workspace`, debug build, test binaries already built)
went from 9.36 s, 617 tests, to 12.82 s, 637 tests, in two runs alike. 3.1 s of
the difference is `crates/ns-runtime/tests/trampoline.rs`, whose boundary test
runs three recursions to 99,999 and to 100,000 counted activations under each
lowering, and whose other tests recurse 40,000 and 50,000 deep or run into the
limit.

### What the profiles show

Two binaries were profiled on 2026-09-12, both release builds with debug
information: the commit the trampoline started from (`04e5fc0`) and the
trampoline's head (`641b8d2`). Each ran `ns-next bench` over `bench_block.ns`
and `bench_send.ns` with 500 iterations under `samply record` (ETW sampling at
1 kHz, Windows 11). samply resolved the system libraries but not ns-next's own
frames, which were resolved afterwards against the PDB through `dbghelp`. Only
samples under the timed entry send (`timed_send`) are counted, because parsing,
compiling and, above all, dropping the heap at exit also land in a profile — for
`bench_block.ns` before the trampoline, the drop of the never-reclaimed arena
was 29% of all samples. The figures are inclusive shares of the timed samples;
`ntdll` has no public symbol names here, so its time is charged to the nearest
ns-next caller.

| Where the time goes | `bench_block.ns`, before | `bench_send.ns`, before | `bench_block.ns`, head | `bench_send.ns`, head |
|---|---|---|---|---|
| a send's argument vector (`vm::collect`), almost all heap allocation and free | 16.7% | 31.1% | — | — |
| `value` as a native callback (`builtins::call_block`) | 12.1% | — | — | — |
| reifying the frame (`Heap::reify`), mostly `RtlCopyMemory` | 9.6% | — | 12.5% | — |
| popping, including the divorce (`Heap::pop_activation`) | 8.9% | 0.2% | 13.1% | 0.4% |
| `Symbol::intern` | 7.1% | 9.3% | 5.8% | 11.9% |
| deciding a send (`Interpreter::decide_send`) | — | — | 10.8% | 16.7% |

**This answers what offset the activation stack's saving on `bench_block.ns`**
("What the activation stack moved" above): allocation. The stack took the
block's own activation out of the frame arena, but every send still built an
argument vector, every block creation still reified its method's activation
into an arena that grows by doubling — the `RtlCopyMemory` under `reify` is
that growth copying every frame so far — and every return still divorced the
frame into two fresh vectors, while `value:` ran through a native callback and
a nested run. Removing the closure copy in step B took one piece of that; the
trampoline took the argument vector and the callback.

What is left on `bench_block.ns` is mostly the reification and the divorce —
about a quarter of the timed samples — which is what closure conversion
("Planned: closure conversion") and a collector ([memory.md](memory.md)) are
for. On `bench_send.ns` the profile shows a cost no measurement had
isolated: `Symbol::intern` is 11.9% of the timed samples. `self tick: sum` is a
lexical send whose selector has a setter's shape, so
`sends::decide_lexical_send` interns the getter name `tick` — hashing its text —
on every send to find out whether the receiver has a slot of that name. The
method and slot tables add SipHash on `Symbol` keys (`hash_one<Symbol>`, 5.1%
self). Spec-conformant lookup removed both: a slot's accessors are method-table
entries, so no send interns a name, and the tables hash identity keys with one
multiply ("What spec-conformant lookup moved" below).

### 3a's head, before the caches (2026-09-16)

Before building the caches, 3a's head (`60ac69e`, the same code as the plan's
`START`, `49a2a38`) was profiled to read the caches' ceiling off the send path
they will actually sit in front of, rather than assume it. One release build
with debug information and the protocol's flags (`+crt-static`, the
function-alignment pin), `samply record --save-only` at 500 iterations per
source (ETW sampling at 1 kHz, Windows 11), over the seven sources of "What
removing the re-entries moved" below: the three fixtures and the four
throwaway `target\bench-lookup` sources. `--save-only` defers all
symbolication to the viewer, so the saved profile carried no symbol names for
any module, ns-next.exe included; every ns-next.exe frame was resolved
afterwards against `target\release\ns_next.pdb` through `wholesym`, the
library samply's own local symbol server uses for the same job, including its
inline site records. At this optimization level `vm::run`,
`sends::decide_lexical_send`, `sends::decide_super_send` and
`dispatch::private_member` carry no standalone symbol at all: LLVM inlined
each of them into `Interpreter::run_from_host`, and only the PDB's inline
chain still names them, at their own program counters inside it.
`Mutator::decide_send`, `dispatch::lookup_public`, `lookup_protected`,
`metaclass_of`, `builtins::execute`, `decide_slot_accessor`, `nested_class`,
`decide_instantiation`, `push_step`, `push_activation`, `pop_activation` and
`reify` each still stand as their own symbol. Reading every call site confirms
`lookup_public`, `lookup_protected`, `private_member` and `metaclass_of` are
reached only from inside `Mutator::decide_send`, `sends::decide_lexical_send`
or `sends::decide_super_send`, so their samples are already inside those three
functions' union and add nothing on top of it. As in the 2026-09-12 profile,
only samples under `timed_send` are counted and a system-module frame
(`ntdll`, `KernelBase`) with no name of its own is charged to the nearest
ns-next.exe caller on the stack.

| Source | Timed samples | Deciding a send | `builtins::execute` | `push_step`/`push_activation` | `pop_activation`/`reify` | `vm::run` self | **Lookup ceiling** |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5,742 | 52.8% | 12.4% | 0.0% | 0.0% | 4.2% | **19.2%** |
| `bench_send.ns` | 6,282 | 48.6% | 12.1% | 5.8% | 0.5% | 3.7% | **19.8%** |
| `bench_block.ns` | 12,361 | 29.0% | 5.9% | 6.1% | 27.0% | 2.5% | **15.0%** |
| `bench_do.ns` | 3,802 | 37.9% | 13.9% | 15.2% | 0.6% | 2.5% | **13.5%** |
| `bench_new.ns` | 3,598 | 55.8% | 8.2% | 8.6% | 0.6% | 2.4% | **22.4%** |
| `bench_new2.ns` | 4,121 | 51.6% | 7.6% | 12.7% | 0.8% | 3.0% | **19.8%** |
| `bench_newarg.ns` | 5,387 | 41.5% | 5.4% | 11.9% | 1.1% | 2.6% | **15.7%** |

The lookup ceiling is "deciding a send" minus the samples where deciding also
touches `builtins::execute`, `decide_slot_accessor`, `nested_class` or
`decide_instantiation` — the primitive itself, the accessor and nested-class
answers a slot or nested declaration reaches directly, and instantiation's own
work, none of which a cache removes. On every source the ceiling sits at less
than half of "deciding a send": `bench_new.ns`'s instantiation-heavy path
spends most of its deciding time inside `decide_instantiation` itself, not the
method-table walk a cache would shortcut, and `bench_block.ns`'s repeated
reification and divorce — 27.0%, the largest single share on any source — is
what "Planned: closure conversion" and a collector ([memory.md](memory.md))
are for, not the caches. `bench_loop.ns`'s `+` and `<` never leave the
primitive path, so it pushes no activation at all (0.0%) and pops none;
`bench_send.ns`'s single `tick:` call an iteration pushes one (5.8%) and,
since `tick:` creates no closure, pops it without reifying (0.5%, against
`bench_block.ns`'s 27.0%, where every iteration creates one).

### What removing the re-entries moved

Taking the last re-entries of the loop from Rust away ("No re-entry from Rust"
below) was measured whole against the commit it started from, `be3eb40`, in
two sittings on 2026-09-13: the before-binary built in a throwaway worktree,
`ns-next bench <source> --iterations 30` over seven sources, five rounds,
alternating the binaries within each round. Besides the three fixtures, four
throwaway sources exercise the paths that changed and no fixture does, each
printing the sum it accumulates:

- `bench_do.ns`: 50,000 block activations through `do:` — a 5,000-iteration
  `whileTrue:` loop over `#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum := sum + e]`.
- `bench_new.ns`: 20,000 instantiations of
  `class Cell = Object new ( | v ::= 1. | )`, each followed by `v`.
- `bench_new2.ns`: the same with two levels of slots,
  `class Base = Object new ( | u ::= 1. | )` and
  `class Cell = Base new ( | v ::= 1. | )`.
- `bench_newarg.ns`: the same with a clause message that has an argument,
  `class Base x: a = Object new ( | u ::= a. | )` and
  `class Cell = Base x: 1 ( | v ::= 1. | )`, each instantiation followed by
  `u`.

They are not fixtures; the first two are written out in the step's
[plan](../superpowers/plans/2026-09-13-0149-ns-next-no-reentry.md),
"Measurement protocol", and the other two are `bench_new.ns` with the classes
above.

Every binary a measurement compares — the current tree included — is built
with the same explicit flags, and those pin every function start to a
64-byte boundary. In PowerShell, before each build and for the measurement
only:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS
```

`RUSTFLAGS` replaces the configuration's flags rather than adding to them,
which is why `+crt-static` is repeated there; a Linux or macOS build carries
the alignment flag alone. Why the pin: step 2c's brand commit changed no
instruction of the run loop and still measured 1–2% slower than its parent,
because smaller cold code ahead of the hot functions moved every one of them
into a different 64-byte phase ("What 2c moved" below). With every function
start pinned, a comparison measures code, not placement. Why the shipped
build does not carry it: the pin costs about 110 KB of padding, 5.6% of the
release binary, and measured no faster — the unpinned build was level or up
to 1.8% faster than the pinned one — so `.cargo/config.toml` carries only
`+crt-static`, and a build from the configuration alone is the one that
ships, not the one that is compared. Before timing, check that no binary
imports `VCRUNTIME140.dll` (`dumpbin /dependents`) and that each prints the
expected output for every source; a linker map (`-C link-arg=/MAP:<file>`)
shows every function of the crate at a multiple of 64 — the only symbols off
the boundary are the static C runtime's own objects.

Medians in ms from the first sitting, each group's mean, and the change of
the mean in both sittings:

| Source | Before, rounds 1–5 | mean | After, rounds 1–5 | mean | Change, sitting 1 / 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.95, 11.03, 10.96, 10.98, 11.05 | 11.00 | 11.35, 11.34, 11.08, 11.15, 11.10 | 11.20 | **+1.9% / +1.4%**, after entirely above in both |
| `bench_send.ns` | 14.25, 13.86, 13.86, 13.81, 13.74 | 13.90 | 13.87, 13.93, 13.93, 13.98, 13.94 | 13.93 | +0.2% / +0.8%, groups overlap in both |
| `bench_block.ns` | 23.12, 23.14, 22.90, 23.12, 22.92 | 23.04 | 23.31, 23.53, 23.45, 23.39, 23.37 | 23.41 | **+1.6% / +1.4%**, after entirely above in the first |
| `bench_do.ns` | 7.85, 7.90, 7.87, 8.00, 7.84 | 7.89 | 7.88, 7.74, 7.78, 7.78, 7.75 | 7.79 | −1.3% / −1.3%, groups overlap in both |
| `bench_new.ns` | 7.30, 7.27, 7.28, 7.26, 7.29 | 7.28 | 7.11, 7.19, 7.37, 7.15, 7.13 | 7.19 | −1.2% / −2.7%, after entirely below in the second |
| `bench_new2.ns` | 8.17, 8.19, 8.26, 8.16, 8.23 | 8.20 | 8.16, 8.09, 8.08, 8.16, 8.15 | 8.13 | −0.9% / −1.2%, after entirely below in both |
| `bench_newarg.ns` | 10.37, 10.60, 10.51, 10.66, 10.40 | 10.51 | 10.10, 10.04, 10.13, 10.18, 10.10 | 10.11 | −3.8% / −3.2%, after entirely below in both |

**Instantiation is faster than it was with the re-entries**, on all three
shapes, and `do:` and `bench_send.ns` are level with it. What decided that is
isolated, each change measured against its parent in a sitting of its own
(below): two boxes per instantiation body, the box and the list of levels
every instantiation allocated, and a call to `Continuation`'s drop glue on
every return. An initializer now runs in the same loop — `push_step`, a
switch, and `finish` — where it ran in a nested run of it, and once the walk
allocates nothing per instantiation, that is the cheaper of the two. `do:`
running its block as a step instead of in a nested run did not make it clearly
faster, although the design expected it to: −1.3% in both sittings, the groups
overlapping.

**Open finding: `bench_loop.ns` and `bench_block.ns` are 1.4–1.9% slower**,
the after-group entirely above the before-group in three of the four
comparisons. Neither runs `do:`, an instantiation or the accessor in its loop:
`bench_loop.ns` makes only native-leaf and slot sends, and `bench_block.ns`'s
`whileTrue:` is inlined and each iteration makes lexical sends for the slots
and `bump:`, public sends of `<`, `+` and `value:`, and one block creation.
What they run of the changed code is `Interpreter::decide_send` and
`sends::decide_lexical_send`, whose factory-send and nested-class branches now
answer a `Step` (neither is taken here); `builtins::dispatch_native`, whose
`Array` and `OrderedCollection` arms answer a `Step` themselves, with no arm
added; `Interpreter::push_step` (`bench_block.ns` takes its block arm, for
`value:`), whose match has four body arms more; and the types every one of
those sends passes through — `Step`, with four variants more, and the
`Continuation` `vm::finish` reads, with three more, whose `Deliver` it handles
inline, so the loop never reaches `Interpreter::resume`. This is the uniform
shift the first change of the step already showed on code it did not change
(below), an artifact of inlining and register allocation as far as that
investigation went. It is not isolated, and nothing removed from either
benchmark's path explains it. Code placement is the likely mechanism — a
change elsewhere moving the hot functions into another 64-byte phase — but
that was verified only for 2c, whose investigation found it and is why a
measurement now pins function alignment ("What 2c moved" below, and the
protocol above). The comparison with `be3eb40` is carried through
every measurement up to step 3; step 2b's ("What 2b moved" below) puts
`bench_loop.ns` at +1.1% in both of its sittings, the groups overlapping, and
`bench_block.ns` 2.5% faster than `be3eb40`.

Each change was measured against its parent on the way, in sittings of its
own, so its figures are not compared with these:

- **`do:` as a step.** It moved every source by a uniform 2–4%, `bench_loop.ns`
  included, which runs no `do:`. A control — the parent commit plus only the
  new `Continuation::DoElement` variant — did not reproduce it. In the
  post-LTO assembly `vm::finish` is out of line in every build, so the
  continuation machinery never sat in the loop; in the changed build
  `object_class::dispatch` was inlined into `dispatch_native` and `run`'s
  register allocation differed, doing the same work. Moving the continuation
  handling out of `finish` and marking `do_step` `#[inline(never)]` recovered
  nothing. It was accepted as an LLVM inlining and layout artifact on unchanged
  executed code, not bisected further. Code placement is the likely mechanism,
  verified only for 2c ("What 2c moved" below).
- **The nested-class accessor as a step** made no source slower; by the
  protocol's rule `bench_send.ns`, `bench_block.ns` and `bench_new.ns` came out
  faster, with nothing on their paths to explain it.
- **Instantiation as a state machine.** The first version made `bench_new.ns`
  about 15.6% slower. Counting allocations found the cause: two more boxes per
  instantiation, the walk's state and a boxed description of the body (4
  allocations, 296 bytes, per `Cell new`, against 2 and 144). Letting the last
  initializer deliver the instance and carrying it inline as
  `Step::Initializer` restored the count to 2 and 144 bytes on that shape and
  left about +1.8%. A chain of two levels with code still boxed both for every
  body but the last, which showed only once `bench_new2.ns` and
  `bench_newarg.ns` joined the measurement: 15.4% and 18.3% slower than
  `be3eb40`.
- **Every body inline.** Carrying `Step::InitializerThen`,
  `Step::ClauseMessage` and `Step::SuperclassClause` inline instead of a boxed
  body description: `bench_new2.ns` −5.2%, `bench_newarg.ns` −8.0%, both
  entirely below, the other five sources level. The first version gave
  `Step::Initializer` an optional continuation, which made it as large as
  `Step::Method`; `Result<Step, Signal>` stayed at 56 bytes, yet the four
  sources that instantiate nothing got 4–6% slower, all entirely above ("Every
  body is inline in its `Step`" below).
- **Reusing the walk's box.** Taking the walk's box and its list of levels
  from the interpreter's spares instead of allocating both: `bench_new.ns`
  −3.0%, `bench_new2.ns` −8.8%, `bench_newarg.ns` −12.8%, all entirely below;
  the others level.
- **`Continuation`'s drop glue.** With three boxed variants, the drop glue of
  `Continuation` stayed out of line, and `vm::finish` called it on every
  return for the `Deliver` it left in the popped entry. Taking the `Deliver`
  and forgetting it moved `bench_block.ns`, `bench_new.ns`, `bench_new2.ns`,
  `bench_send.ns` and `bench_do.ns` by −0.1% to −1.4% in two sittings, none
  slower, the groups overlapping.
- **The in-progress marker** in the cache slot, replacing the list of
  creations underway, moved no source (−1.2% to +1.1%, overlapping); it was
  made for the test suite's time and a crafted binary's (below).
- **Comparing `do:` as a well-known symbol**, instead of through the interner,
  was tried and not kept: `bench_block.ns`, `bench_new.ns` and `bench_new2.ns`
  got 1.7–2.1% slower, none of which sends to a collection in its loop.

The suite (`cargo test --workspace`, debug build, test binaries already built)
takes 12.5 s for 662 tests, in two runs alike — as long as before the step,
12.5 s for 644. For a while it took 25 s: two tests recurse through the
nested-class accessor to the activation limit, about 50,000 creations underway
at the deepest, and the cyclic-definition check scanned a list of every
creation underway at each level, quadratic in the depth, where the bound on
re-entries had stopped such a recursion at 1,000 levels. With the marker in the
cache slot the check is one read, and
`goldens/limits/recursion_through_the_nested_class_accessor_is_bounded_by_the_activation_limit.ns`
takes 0.3 s again.

### What 2b moved

Step 2b ([memory.md](memory.md), "Checked roots and a collector") put three
things into a release build: the safepoint poll in the run loop ("The VM"
above), a closure's payload inline in its object, and host roots at the host
boundary; the tracing, the move and the stress mode exist only in a debug
build. It was measured whole against the commit it started from, `4882a2c`,
and against `be3eb40`, in two sittings on 2026-09-13, by the protocol of "What
removing the re-entries moved" above: both before-binaries built in throwaway
worktrees, `ns-next bench <source> --iterations 30` over the same seven
sources, five rounds, alternating the three binaries within each round.
Medians in ms from the first sitting, and each group's mean:

| Source | `be3eb40`, rounds 1–5 | mean | `4882a2c`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.22, 11.16, 11.12, 11.34, 11.21 | 11.21 | 11.64, 11.19, 11.40, 11.28, 11.37 | 11.38 | 11.32, 11.44, 11.30, 11.32, 11.28 | 11.33 |
| `bench_send.ns` | 14.07, 13.93, 14.09, 14.21, 13.87 | 14.03 | 14.03, 14.19, 14.12, 14.13, 14.15 | 14.12 | 14.01, 14.35, 14.10, 14.23, 14.11 | 14.16 |
| `bench_block.ns` | 23.26, 23.30, 23.52, 23.45, 23.17 | 23.34 | 23.87, 23.70, 23.52, 24.03, 23.64 | 23.75 | 22.76, 22.70, 22.67, 22.83, 22.82 | 22.76 |
| `bench_do.ns` | 7.92, 8.17, 7.96, 7.98, 8.05 | 8.02 | 8.00, 8.10, 7.90, 7.88, 8.22 | 8.02 | 7.96, 7.97, 7.80, 8.00, 7.81 | 7.91 |
| `bench_new.ns` | 7.46, 7.39, 7.49, 7.49, 7.37 | 7.44 | 7.44, 7.27, 7.41, 7.27, 7.31 | 7.34 | 7.32, 7.45, 7.46, 7.40, 7.42 | 7.41 |
| `bench_new2.ns` | 8.34, 8.41, 8.47, 8.38, 8.50 | 8.42 | 8.24, 8.33, 8.26, 8.35, 8.42 | 8.32 | 8.48, 8.44, 8.32, 8.31, 8.22 | 8.35 |
| `bench_newarg.ns` | 10.66, 10.45, 10.57, 10.59, 10.60 | 10.57 | 10.20, 10.24, 10.22, 10.27, 10.29 | 10.24 | 10.26, 10.21, 10.24, 10.26, 10.11 | 10.22 |

The change of the after-group's mean in both sittings:

| Source | Against `4882a2c`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|
| `bench_loop.ns` | −0.4% / −0.9%, overlapping | +1.1% / +1.1%, overlapping |
| `bench_send.ns` | +0.3% / −0.5%, overlapping | +0.9% / +1.4%, overlapping |
| `bench_block.ns` | **−4.2% / −3.9%**, after entirely below in both | **−2.5% / −2.6%**, after entirely below in both |
| `bench_do.ns` | −1.4% / −1.1%, overlapping | −1.4% / −1.3%, overlapping |
| `bench_new.ns` | +1.0% / +0.7%, overlapping | −0.4% / −1.4%, the groups touching at 7.46 in the second |
| `bench_new2.ns` | +0.4% / +0.8%, after entirely above in the second, by 0.01 ms | −0.8% / −0.6%, overlapping |
| `bench_newarg.ns` | −0.3% / −0.4%, overlapping | −3.4% / −3.2%, after entirely below in both |

**No source is slower than at `4882a2c` beyond the sittings' noise, and
`bench_block.ns` is about 4% faster**, which is the closure split's (below).
One group lay outside the rest: in the second sitting `bench_new2.ns`'s
after-group sat entirely above its parent-group, 8.37–8.44 against 8.30–8.36
ms. The first sitting overlapped, and a third, run to settle it, put it level
(−0.2%, overlapping).

**Against `be3eb40`, nothing is slower beyond the groups' spread.**
`bench_loop.ns`, which 2a left 1.4–1.9% slower with its after-group entirely
above, is +1.1% in both sittings, overlapping; `bench_block.ns`, which 2a left
1.4–1.6% slower, is 2.5% faster. `bench_send.ns` is +0.9% and +1.4%,
overlapping, where 2a measured +0.2% and +0.8%. That is 2a's figure read in
other sittings rather than something 2b added: 2b's binary is level with
`4882a2c`, and the unchanged `4882a2c` itself sat +0.6% and +1.9% above
`be3eb40` in these two sittings, as the parents of the changes below sat +0.9%
to +2.3% above it in theirs. Every other source is level with `be3eb40` or
faster.

Two changes were measured against their parents on the way, each in a sitting
of its own, so their figures are not compared with these:

- **A closure's payload inline in its object** ([memory.md](memory.md), "What
  the collector traces"): `bench_block.ns` −4.4% and `bench_newarg.ns` −0.8%,
  both entirely below, the rest level. `MakeClosure` no longer allocates an
  `Rc` per closure, and a popped block activation no longer drops one. A
  release `Object` stays 56 bytes: the inline `ClosureData` is 24 bytes, within
  the payload's 48.
- **The safepoint poll.** Its first measurement found no source slower, and
  measured nothing: with an empty slow path in a release build, fat LTO had
  deleted the poll's call, then its branch and the flag's load
  ([memory.md](memory.md), "Where a collection can happen"). With the slow path
  serving the request by clearing the flag, `vm::run`'s release assembly
  compares the flag at every switch and every backward jump and calls the
  safepoint when it is set, and the re-measurement found no source slower than
  its parent: −3.0% on `bench_block.ns` to +0.9% on `bench_new2.ns`, every
  group overlapping. Cog's fallback, folding the request into the depth check,
  was not needed.

Host roots changed what `bench` times, slightly: the timed span now fetches the
send's arguments from their roots and roots its result, an `Rc` allocation and
a push onto the table against a run of milliseconds. Every after-figure above
includes it.

The suite (`cargo test --workspace`, debug build, test binaries already built)
takes 12.7 s, measured at 682 tests, against 12.5 s for 662 before the step. Under
`NS_NEXT_STRESS_MOVES=1` it takes 190.2 s: almost half of that is
`crates/ns-runtime/tests/trampoline.rs`'s depth-limit test, and most of the
rest the CLI tests that spawn `ns-next` per fixture or golden
([memory.md](memory.md), "Checking the roots before a collector exists: the
stress mode").

### What 2c moved

Step 2c ([memory.md](memory.md), "The brand, checked by the compiler rather
than by convention") put one thing into a release build: the step state
behind `World::enter`, entered once per instruction the run loop executes
("The VM" above). The brand itself is a zero-sized lifetime and changes no
layout: `Object`, `FrameSlot` and `Result<Step, Signal>` kept their release
sizes, 56, 136 and 56 bytes. The step was measured whole against the commit
it started from, `a5d1e05`, and against `be3eb40`, in two sittings on
2026-09-14, by the protocol of "What removing the re-entries moved" above,
every binary built with every function start pinned to a 64-byte boundary
and `+crt-static` — the protocol's flag set; "The pin" below says where the
rule comes from — the before-binaries in throwaway worktrees through
`RUSTFLAGS`, the head from a configuration that carried the alignment flag
at the time (it no longer does; the protocol above says why), `ns-next
bench <source> --iterations 30` over the same seven sources, five rounds,
alternating the binaries within each round. A fourth binary ran in the same
rounds — `a5d1e05` as its own configuration builds it, unpinned — to show
what the pin itself does. Medians in ms, and each group's mean, from the
first sitting:

| Source | `be3eb40`, rounds 1–5 | mean | `a5d1e05`, rounds 1–5 | mean | `a5d1e05` unpinned, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.95, 11.01, 11.06, 11.12, 11.22 | 11.07 | 11.20, 11.14, 11.24, 11.19, 11.21 | 11.20 | 11.15, 11.17, 11.19, 11.16, 11.21 | 11.17 | 11.30, 11.27, 11.26, 11.24, 11.21 | 11.26 |
| `bench_send.ns` | 13.63, 13.77, 13.74, 13.99, 13.73 | 13.77 | 14.08, 14.16, 14.17, 14.14, 14.03 | 14.12 | 13.89, 14.02, 14.15, 14.20, 14.00 | 14.05 | 14.04, 14.14, 14.38, 14.25, 14.19 | 14.20 |
| `bench_block.ns` | 23.01, 23.35, 23.21, 23.10, 23.11 | 23.16 | 22.60, 23.12, 22.62, 22.53, 22.52 | 22.68 | 22.40, 22.50, 22.49, 22.71, 22.88 | 22.59 | 22.56, 22.66, 22.61, 22.47, 22.98 | 22.66 |
| `bench_do.ns` | 7.86, 8.07, 8.08, 8.13, 8.07 | 8.04 | 7.72, 7.79, 7.70, 7.71, 7.78 | 7.74 | 7.80, 7.70, 7.73, 7.68, 7.73 | 7.73 | 7.73, 7.77, 7.79, 7.78, 7.71 | 7.76 |
| `bench_new.ns` | 7.34, 7.40, 7.38, 7.36, 7.43 | 7.38 | 7.37, 7.39, 7.37, 7.33, 7.58 | 7.40 | 7.25, 7.30, 7.32, 7.26, 7.38 | 7.30 | 7.30, 7.36, 7.27, 7.34, 7.34 | 7.32 |
| `bench_new2.ns` | 8.44, 8.36, 8.32, 8.28, 8.54 | 8.39 | 8.25, 8.40, 8.39, 8.31, 8.48 | 8.36 | 8.21, 8.23, 8.17, 8.23, 8.22 | 8.21 | 8.32, 8.22, 8.40, 8.31, 8.35 | 8.32 |
| `bench_newarg.ns` | 10.36, 10.50, 10.51, 10.56, 10.61 | 10.51 | 10.29, 10.34, 10.20, 10.23, 10.23 | 10.26 | 10.09, 10.04, 10.25, 10.10, 10.09 | 10.11 | 10.09, 10.15, 10.40, 10.08, 10.29 | 10.20 |

And from the second:

| Source | `be3eb40`, rounds 1–5 | mean | `a5d1e05`, rounds 1–5 | mean | `a5d1e05` unpinned, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.02, 11.03, 11.07, 11.10, 10.96 | 11.03 | 11.36, 11.16, 11.24, 11.19, 11.32 | 11.25 | 11.29, 11.26, 11.22, 11.20, 11.11 | 11.21 | 11.20, 11.25, 11.27, 11.18, 11.21 | 11.22 |
| `bench_send.ns` | 13.70, 13.83, 13.80, 13.77, 13.84 | 13.79 | 14.23, 14.06, 14.26, 14.11, 14.11 | 14.16 | 14.15, 13.97, 14.13, 13.99, 14.05 | 14.06 | 14.09, 14.15, 14.13, 14.07, 14.03 | 14.09 |
| `bench_block.ns` | 23.32, 23.17, 23.27, 23.31, 23.39 | 23.29 | 22.39, 22.54, 23.38, 23.03, 22.90 | 22.85 | 22.86, 22.60, 22.54, 22.94, 22.44 | 22.68 | 22.47, 22.50, 22.74, 22.53, 22.44 | 22.54 |
| `bench_do.ns` | 7.87, 7.94, 7.93, 8.11, 7.97 | 7.96 | 7.65, 7.79, 7.72, 7.75, 7.76 | 7.73 | 7.79, 7.89, 7.70, 7.70, 7.83 | 7.78 | 7.77, 7.83, 7.80, 7.74, 7.66 | 7.76 |
| `bench_new.ns` | 7.35, 7.43, 7.33, 7.43, 7.34 | 7.37 | 7.29, 7.40, 7.34, 7.34, 7.33 | 7.34 | 7.40, 7.23, 7.45, 7.27, 7.40 | 7.35 | 7.45, 7.35, 7.32, 7.31, 7.25 | 7.34 |
| `bench_new2.ns` | 8.43, 8.32, 8.32, 8.38, 8.26 | 8.34 | 8.31, 8.39, 8.23, 8.32, 8.21 | 8.29 | 8.21, 8.28, 8.30, 8.12, 8.34 | 8.25 | 8.35, 8.34, 8.30, 8.28, 8.28 | 8.31 |
| `bench_newarg.ns` | 10.40, 10.31, 10.40, 10.42, 10.49 | 10.41 | 10.28, 10.20, 10.30, 10.16, 10.38 | 10.26 | 10.09, 10.18, 10.04, 10.14, 10.05 | 10.10 | 10.14, 10.15, 10.20, 10.16, 10.27 | 10.18 |

The change of the after-group's mean in both sittings, and what the pin
itself does — the unpinned `a5d1e05` against the pinned one:

| Source | Against `a5d1e05`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 | `a5d1e05` unpinned against pinned, sitting 1 / 2 |
|---|---|---|---|
| `bench_loop.ns` | +0.5% / −0.3%, overlapping | +1.7% / +1.7%, after entirely above in the second | −0.2% / −0.4%, overlapping |
| `bench_send.ns` | +0.6% / −0.4%, overlapping | +3.1% / +2.2%, after entirely above in both | −0.4% / −0.7%, overlapping |
| `bench_block.ns` | −0.1% / −1.4%, overlapping | −2.2% / −3.2%, after entirely below in both | −0.4% / −0.8%, overlapping |
| `bench_do.ns` | +0.2% / +0.4%, overlapping | −3.5% / −2.5%, after entirely below in both | −0.1% / +0.6%, overlapping |
| `bench_new.ns` | −1.1% / +0.0%, overlapping | −0.8% / −0.5%, overlapping | −1.4% / +0.2%, overlapping |
| `bench_new2.ns` | −0.5% / +0.2%, overlapping | −0.8% / −0.4%, overlapping | −1.8% / −0.5%, unpinned entirely below in the first |
| `bench_newarg.ns` | −0.5% / −0.8%, overlapping | −2.9% / −2.1%, after entirely below in the second | −1.4% / −1.6%, overlapping |

**Every source is level with `a5d1e05`**: the after-group overlaps its
parent-group on all seven sources in both sittings, the change of the mean
between −1.4% and +0.6%. A zero-sized lifetime and an entry that inlines away
cost nothing at run time, as the design expected.

**Against `be3eb40`**, the after-binary's shift is the parent's, read again:
`a5d1e05` itself sits +2.5% and +2.7% above `be3eb40` on `bench_send.ns`
and +1.1% and +2.0% on `bench_loop.ns` in these sittings — 2a's shift on
those two sources ("What removing the re-entries moved" above) — and the
after-binary +3.1% / +2.2% and +1.7% / +1.7%, level with its parent. No
source's shift against `be3eb40` grew past what 2b left by more than either
group's spread: `bench_send.ns`, which 2b left at +0.9% to +1.4%, grew by
0.24 and 0.11 ms against `be3eb40`-group spreads of 0.36 and 0.14 ms, within
them, just so in the second sitting. `bench_block.ns` is 2.2% and 3.2%
faster than `be3eb40`, `bench_do.ns` 2.5–3.5%, `bench_newarg.ns` 2.1–2.9%,
and `bench_new.ns` and `bench_new2.ns` are level with it.

**What the pin itself does** is the last column: the unpinned `a5d1e05`
reads −0.1% to −1.8% against the pinned one, the groups overlapping on every
source in both sittings but one (`bench_new2.ns` in the first, the unpinned
group entirely below by 1.8%). So the unpinned binary is, if anything,
slightly faster: the pin fixes the phase of every function start, not the
phase the compiler's own layout gives the loop headers inside a function,
and `a5d1e05`'s hot functions happened to sit well. What the pin buys is
that a change elsewhere can no longer move them, which is the history below.
It costs about 110 KB of padding — 2,091,520 against 1,979,904 bytes, 5.6%,
some 4,600 functions each padded to the next boundary. Padding that buys
repeatability and no speed is why the shipped build does not carry it: the
pin belongs to the measurement protocol, where every compared binary gets
it through `RUSTFLAGS`, and `.cargo/config.toml` stays as it was.

**The pin.** Before it, the same step measured 1–2.5% slower on the block
and instantiation benchmarks, and finding out why is where the pin comes
from. Two sittings of the same protocol on 2026-09-14, with the binaries as
their configurations built them, put `bench_new2.ns` at +1.8% and +2.5%
against `a5d1e05`, entirely above in both, `bench_block.ns` at +2.5% / +1.8%
and `bench_newarg.ns` at +1.2% / +1.7%, each entirely above in one; the
loop, the send and `do:` level. A bisection sitting over the step's commits
— the four sources that moved, five rounds, the change of each group's mean
against `a5d1e05` — placed the shift on the brand commit:

| Source | `d27e40b` (the entry) | `7b41649` (the names written out) | `e59dc52` (hosts through `enter`) | `1549fb0` (the brand) | `2caf3e0` (no cast) |
|---|---|---|---|---|---|
| `bench_send.ns` | +0.7% | +0.8% | +0.6% | +1.6% | +1.0% |
| `bench_block.ns` | +0.5% | +1.1% | +1.4% | +2.8% | +3.1% |
| `bench_new2.ns` | −0.3% | +0.0% | +0.7% | +1.6% | +1.5% |
| `bench_newarg.ns` | +0.5% | +0.1% | +0.1% | +1.3% | +1.6% |

`e59dc52` to `1549fb0` is the one step between neighbours that separates
groups, though the brand commit's diff is types and signatures apart from
`rebrand` and small reshapes the lifetimes required, and its binary 512
bytes smaller. The post-LTO assembly of both commits, split into functions
and diffed, explained it: the hot path is instruction for instruction
identical — `Interpreter::run_from_host` with `vm::run` inlined (2,470
instructions), `builtins::dispatch_native` (3,082), `vm::finish`,
`Mutator::decide_send`, `push_step`'s block and method arms,
`Heap::push_activation`, `pop_activation` and `reify`, the lookups — and
only cold host code differs: the `for<'gc>` closure through
`Interpreter::enter` needs a smaller stack frame in the host entries —
`Interpreter::send`, ns-cli's `entry::instantiate_root` and its command
dispatcher shrink by 16, 16 and 256 bytes, `Interpreter::instantiate` grows
by two instructions — and the `CREATION_IN_PROGRESS` constant that became a
function stores 2 bytes where it stored 16, in `push_step`'s cold
nested-class arm (−16 bytes). The linker map showed every function behind
them moved by 224 or 240 bytes — 32 or 16 modulo 64 — so every hot entry
point, and with it every loop header and branch target in the run loop, sat
in a different 64-byte line than before. Rebuilding both commits with
`-C llvm-args=-align-all-functions=6` changed no instruction and brought
them level in two sittings (`bench_block.ns` from +1.5% / +1.8% to −0.3% /
−0.1%, `bench_send.ns` from +1.5% / +0.8% to +0.4% / +0.2%). That is the
mechanism, and the decision, 2026-09-14, is that the pin belongs to the
measurement protocol and to nothing else: a change to cold code must not
move the hot loop's phase *in a comparison*, so every binary a measurement
compares is built with the alignment flag through `RUSTFLAGS` ("What
removing the re-entries moved" above, the protocol), while the shipped
build stays unpinned, since the pin measured no faster and costs 5.6% of
the binary ("What the pin itself does" above). The tables above are that
measurement, every binary aligned.

One sitting of its own, so its figures are not compared with these: **the
per-instruction entry** (Task 1 of the step, `d27e40b`, the step state moved
behind `World::enter` with no lifetime yet), against `a5d1e05` in one
sitting on 2026-09-14, unpinned: level on every source, every group
overlapping its parent's — `bench_loop.ns` +0.1%, `bench_send.ns` −0.3%,
`bench_block.ns` −0.9%, `bench_do.ns` +0.4%, `bench_new.ns` +0.9%,
`bench_new2.ns` −0.0%, `bench_newarg.ns` +0.5%. So the entry inlines away,
as `#[inline(always)]` asks.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 13.1 s at 691 tests, and 190.3 s under `NS_NEXT_STRESS_MOVES=1`,
where 2b left 12.7 s and 190.2 s ([memory.md](memory.md), "Checking the
roots before a collector exists: the stress mode").

### What spec-conformant lookup moved

The step that made name resolution and lookup follow the specification
([dispatch.md](dispatch.md), "Resolution", "Lookup" and "Slot accessors")
changed three things a release build runs whenever it decides a send. A slot's
accessors are entries of the method table, found by the ordinary lookups, so
the lexical-send slot pre-check is gone and with it the `Symbol::intern` of a
getter name on every setter-shaped lexical send ("What the profiles show"
above); in its place a lexical send asks N's own table for §5.9's private rule
before `lookupProtected` asks the receiver's. The explicit-receiver, lexical and
super lookups apply the specification's verdicts. And the method tables, the
nested-class slot maps and the interpreter's application caches hash their
identity keys with one multiply and a rotation instead of SipHash
([object-model.md](object-model.md), "Identity-keyed tables hash with
`hash::IdentityMap`").

It was measured whole against the commit it started from, `59061ec`, and
against `be3eb40`, in two sittings on 2026-09-15, by the protocol of "What
removing the re-entries moved" above: every binary built through `RUSTFLAGS`
with `+crt-static` and the function-alignment pin, the before-binaries in
throwaway worktrees, `ns-next bench <source> --iterations 30` over the seven
sources, five rounds, the order of the three binaries rotating from round to
round. Three of the four throwaway sources had to change. `bench_new.ns`,
`bench_new2.ns` and `bench_newarg.ns` declared a slot and a getter method of
one name in one class body (`| v ::= 1. |` beside `public v = ( ^v )`), which
the resolver now refuses as a duplicate member. Their slots are renamed
(`held`, `heldU`, `heldV`) and the methods keep their names and bodies, so every
binary does the same work, a method activation and a slot read per
instantiation. Making the slots public and dropping the methods instead would
have left both before-binaries, which answer no explicit-receiver slot send,
with a `doesNotUnderstand:`.

These are the three as every later measurement uses them, written under
`target\bench-lookup\` (ignored by git) beside the unchanged `bench_do.ns` of
the [2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol". Each prints `20000`:

```
Newspeak3
'Bench'
class BenchNew usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Cell = Object new (
		| held ::= 1. |
	) (
		public v = ( ^held )
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new v. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

```
Newspeak3
'Bench'
class BenchNew2 usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Base = Object new (
		| heldU ::= 1. |
	) (
		public u = ( ^heldU )
	) : ()
	class Cell = Base new (
		| heldV ::= 1. |
	) (
		public v = ( ^heldV )
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new v. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

```
Newspeak3
'Bench'
class BenchNewArg usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Base x: a = Object new (
		| heldU ::= a. |
	) (
		public u = ( ^heldU )
	) : ()
	class Cell = Base x: 1 (
		| heldV ::= 1. |
	) (
		public v = ( ^heldV )
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new u. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

Medians in ms, and each group's mean, from the first sitting:

| Source | `59061ec`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.22, 11.41, 11.45, 11.54, 11.42 | 11.41 | 11.39, 11.22, 11.10, 11.09, 11.23 | 11.21 | 11.29, 11.20, 11.32, 11.21, 11.19 | 11.24 |
| `bench_send.ns` | 14.51, 14.09, 14.26, 14.18, 14.17 | 14.24 | 13.73, 13.95, 13.98, 13.90, 14.05 | 13.92 | 12.65, 12.51, 12.36, 12.52, 12.31 | 12.47 |
| `bench_block.ns` | 22.61, 22.61, 22.99, 23.17, 22.93 | 22.86 | 23.76, 23.69, 23.39, 23.31, 23.37 | 23.50 | 21.29, 21.15, 21.43, 21.06, 21.09 | 21.20 |
| `bench_do.ns` | 7.74, 7.84, 7.76, 7.91, 7.95 | 7.84 | 7.94, 8.07, 8.30, 8.04, 8.01 | 8.07 | 7.43, 7.58, 7.83, 7.48, 7.41 | 7.54 |
| `bench_new.ns` | 7.41, 7.46, 7.36, 7.39, 7.45 | 7.41 | 7.42, 7.48, 7.58, 7.46, 7.35 | 7.46 | 6.83, 6.76, 6.92, 6.76, 6.77 | 6.81 |
| `bench_new2.ns` | 8.40, 8.37, 8.45, 8.37, 8.36 | 8.39 | 8.37, 8.39, 8.38, 8.39, 8.34 | 8.37 | 7.95, 7.83, 7.81, 7.77, 7.90 | 7.85 |
| `bench_newarg.ns` | 10.33, 10.33, 10.35, 10.43, 10.35 | 10.36 | 10.73, 10.62, 10.66, 10.57, 10.55 | 10.63 | 9.80, 9.87, 9.92, 10.03, 9.92 | 9.91 |

And from the second:

| Source | `59061ec`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.30, 11.34, 11.32, 11.43, 11.34 | 11.34 | 11.13, 11.13, 11.30, 11.30, 11.15 | 11.20 | 11.27, 11.33, 11.36, 11.33, 11.17 | 11.29 |
| `bench_send.ns` | 14.19, 14.29, 14.33, 14.30, 14.23 | 14.27 | 13.76, 13.90, 14.00, 13.81, 13.99 | 13.89 | 12.41, 12.46, 12.57, 12.53, 12.51 | 12.50 |
| `bench_block.ns` | 22.74, 23.07, 22.88, 22.78, 23.18 | 22.93 | 23.40, 23.39, 23.47, 23.31, 23.42 | 23.40 | 21.20, 21.16, 21.07, 21.18, 21.04 | 21.13 |
| `bench_do.ns` | 7.79, 8.01, 7.86, 7.75, 7.83 | 7.85 | 8.12, 8.08, 7.88, 7.96, 7.95 | 8.00 | 7.41, 7.46, 7.49, 7.44, 7.46 | 7.45 |
| `bench_new.ns` | 7.49, 7.46, 7.37, 7.47, 7.35 | 7.43 | 7.66, 7.55, 7.52, 7.63, 7.48 | 7.57 | 6.77, 6.82, 6.75, 6.85, 6.90 | 6.82 |
| `bench_new2.ns` | 8.36, 8.34, 8.35, 8.43, 8.30 | 8.36 | 8.30, 8.37, 8.43, 8.38, 8.35 | 8.37 | 7.84, 7.75, 8.06, 7.75, 7.76 | 7.83 |
| `bench_newarg.ns` | 10.27, 10.34, 10.30, 10.27, 10.28 | 10.29 | 10.72, 10.58, 10.66, 10.54, 10.70 | 10.64 | 9.76, 9.80, 9.87, 9.79, 9.79 | 9.80 |

The change of the after-group's mean in both sittings:

| Source | Against `59061ec`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|
| `bench_loop.ns` | −1.5% / −0.5%, overlapping | +0.3% / +0.8%, overlapping |
| `bench_send.ns` | −12.5% / −12.4%, after entirely below in both | −10.4% / −10.1%, after entirely below in both |
| `bench_block.ns` | −7.3% / −7.8%, after entirely below in both | −9.8% / −9.7%, after entirely below in both |
| `bench_do.ns` | −3.8% / −5.1%, after entirely below in the second | −6.5% / −6.8%, after entirely below in both |
| `bench_new.ns` | −8.1% / −8.2%, after entirely below in both | −8.7% / −9.9%, after entirely below in both |
| `bench_new2.ns` | −6.4% / −6.3%, after entirely below in both | −6.2% / −6.4%, after entirely below in both |
| `bench_newarg.ns` | −4.3% / −4.7%, after entirely below in both | −6.8% / −7.9%, after entirely below in both |

**No source is slower than either commit, and six are faster than both.**
`bench_send.ns` gained most, 12.5% against `59061ec` and 10% against
`be3eb40`. Every one of its iterations makes a setter-shaped lexical send,
`self tick: sum`, which interned `tick` every time it ran. The others gained
3.8–9.9%, every after-group but one entirely below its before-group in both
sittings. `bench_loop.ns` makes only slot sends and native leaf sends, and it
is level with both. So the shift against `be3eb40` that 2a left on the send and
the loop, and that 2c carried ("What 2c moved" above), is gone: `bench_send.ns`
now sits 10% below `be3eb40` and `bench_loop.ns` level with it.

**The accessors alone were slower, and the hasher is what made the step
faster.** One sitting of its own, the same protocol, measured the step's last
two code commits against their parents. Its figures are not compared with the
tables above:

| Source | `3cf7e3e` mean | `431da35` mean | `7b1901c` mean | Accessors, `431da35` against `3cf7e3e` | Hasher, `7b1901c` against `431da35` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.87 | 14.02 | 11.26 | +18.1%, entirely above | −19.7%, entirely below |
| `bench_send.ns` | 14.55 | 15.46 | 12.44 | +6.3%, entirely above | −19.5%, entirely below |
| `bench_block.ns` | 23.47 | 24.21 | 21.26 | +3.1%, entirely above | −12.2%, entirely below |
| `bench_do.ns` | 8.13 | 8.53 | 7.44 | +5.0%, entirely above | −12.8%, entirely below |
| `bench_new.ns` | 7.62 | 7.96 | 6.84 | +4.5%, entirely above | −14.0%, entirely below |
| `bench_new2.ns` | 8.61 | 8.93 | 7.81 | +3.8%, entirely above | −12.6%, entirely below |
| `bench_newarg.ns` | 10.66 | 11.14 | 9.83 | +4.5%, entirely above | −11.8%, entirely below |

Making the accessors method-table entries (`431da35`) slowed every source,
`bench_loop.ns` most. Before it, a lexical slot send made one SipHash probe of
the application's slot map, and a setter-shaped one interned its getter name
first. After it, every lexical send made two probes of the method table: N's
for the private rule, then the receiver's for `lookupProtected`. The loop makes
300,000 slot sends, four getters and two setters an iteration, and 2.15 ms more
is about 7 ns a send, of the order of one SipHash probe of a 4-byte key. That
is an inference from what the change did; no profile was taken. The hasher
(`7b1901c`) made every probe a multiply, and took every source past where it
started.

**Sparing the second probe measured level, so the lexical path keeps it.**
When N is the receiver's class, which is the common case for a slot send,
`lookupProtected`'s first level is N and it answers what N's own table
already answered. Two variants reused the first probe, each measured against
`7b1901c` in one sitting of its own. The first passed the receiver's class to
the private-member probe, which answered a visible member when that class was
N. It came out level on every source, −0.4% to +2.2%, every group
overlapping, so with the hasher the second probe costs nothing measurable. The
second returned the probe's finding as a three-way enum. It measured 3.4–14.7%
slower on every source, entirely above on six. That variant changed only how a
result travelled between two functions, so the lexical send's decision is
sensitive to codegen. The inline caches replaced that decision on the hot
path, and their measurement closes the finding ("What the inline caches
moved" below).

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.7 s at 710 tests, and 189.9 s under `NS_NEXT_STRESS_MOVES=1`.

### What primitives as method-table entries moved

The step that made every builtin's native an entry of its class's method
table, found by the ordinary lookups instead of matched by
`selector.as_str()` ([dispatch.md](dispatch.md), "Primitives"), was measured
against the commit it started from, `883431b` ("start"), and against
`be3eb40`, by the protocol of "What removing the re-entries moved" above:
every binary — start's and `be3eb40`'s built in throwaway worktrees, the
current tree's built in place — through the same `RUSTFLAGS`, `+crt-static`
and the function-alignment pin; `ns-next bench <source> --iterations 30`
over the same seven sources, five rounds, the order of the three binaries
rotating from round to round; two whole sittings, each a complete pass of
the timing loop run one after the other in the same session, with nothing
else running on the machine during either.

Sitting 1, means in ms:

| Source | `be3eb40` mean | start mean | after mean | after − start | after − `be3eb40` | vs start | vs `be3eb40` |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1225 | 11.1931 | 11.2503 | +0.51% | +1.15% | overlapping | overlapping |
| `bench_send.ns` | 13.8138 | 12.4300 | 12.4673 | +0.30% | −9.75% | overlapping | entirely below |
| `bench_block.ns` | 23.2705 | 21.2777 | 21.2112 | −0.31% | −8.85% | overlapping | entirely below |
| `bench_do.ns` | 7.9111 | 7.4094 | 7.3474 | −0.84% | −7.12% | overlapping | entirely below |
| `bench_new.ns` | 7.4628 | 6.8485 | 6.8266 | −0.32% | −8.53% | overlapping | entirely below |
| `bench_new2.ns` | 8.3601 | 7.7574 | 7.6929 | −0.83% | −7.98% | overlapping | entirely below |
| `bench_newarg.ns` | 10.5532 | 9.7983 | 9.7168 | −0.83% | −7.93% | overlapping | entirely below |

Sitting 2, means in ms:

| Source | `be3eb40` mean | start mean | after mean | after − start | after − `be3eb40` | vs start | vs `be3eb40` |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1329 | 11.2409 | 11.3064 | +0.58% | +1.56% | overlapping | overlapping |
| `bench_send.ns` | 13.8035 | 12.4302 | 12.6491 | +1.76% | −8.36% | overlapping | entirely below |
| `bench_block.ns` | 23.4623 | 21.1425 | 21.2321 | +0.42% | −9.51% | overlapping | entirely below |
| `bench_do.ns` | 7.9702 | 7.4758 | 7.3564 | −1.60% | −7.70% | overlapping | entirely below |
| `bench_new.ns` | 7.4407 | 6.7809 | 6.6963 | −1.25% | −10.00% | overlapping | entirely below |
| `bench_new2.ns` | 8.3527 | 7.7316 | 7.7927 | +0.79% | −6.71% | overlapping | entirely below |
| `bench_newarg.ns` | 10.6841 | 9.8072 | 9.5929 | −2.18% | −10.21% | entirely below | entirely below |

**Both sittings agree: no source's after-group lies entirely above its
start-group in either one** (`bench_newarg.ns` is even entirely below
start in the second sitting — faster than start, not merely level). Against
`be3eb40`, six of the seven sources are faster by about 7–10% in both
sittings, entirely below throughout; `bench_loop.ns` is level, +1.15%/+1.56%,
its group overlapping in both. `bench_loop.ns` makes three native sends and
six lexical slot sends per iteration — 150,000 native sends and 300,000 slot
sends in all — and no method send or instantiation, so it is the source that
exercises the rebuilt decision path hardest with nothing else to gain from —
level with `be3eb40` there is this step's whole answer for that path, where
the other six sources also gain from lookup and hashing work earlier steps
already made faster.

**What the first shape of this decision cost, and what recovered it.** The
commit that put every native behind the ordinary lookup, `e4ec1bd`, first
measured 5.5% to 22.4% slower than start on all seven sources, in two
sittings — an order of magnitude past the 1–4% codegen jitter every earlier
step's measurement had allowed for, and reproduced in a bisection across the
step's four commits. `bench_loop.ns` was hit hardest, +20.9%/+20.4% against
start, consistent with the change removing a fast path only a native send
had used. The cause was not the extra method-table probe a builtin's send now
makes: priced directly with a diagnostic build that added a redundant
`black_box(methods.get(black_box(selector)))` to every level of every walk,
the extra probe cost 0.4% for nine of them a `bench_loop.ns` iteration makes,
about 0.05% each — the tables' hasher is a multiply and a rotation, not
SipHash, which is why. The cost was the shape every lookup's answer travelled
through on its way back to its sender:

- **A 64-byte settled enum on every send's return path.** Every decision —
  slot, method, nested class, primitive — answered `Settled { Answered(...),
  Found(Lookup) }`, 64 bytes against `Lookup`'s own 24, so a slot or method
  send that never touches a primitive still built, returned and re-matched the
  wider value. Removing it, so each of the five callers that can reach a
  `Lookup::Primitive` matches its `Lookup` directly, measured 6.5%
  (`bench_loop.ns`) and 5.1% (`bench_send.ns`) faster — the largest single
  piece (`8a05621`).
- **The lookup rule as a run-time value.** The three per-lookup verdict
  closures had become one shared `walk` taking a `Rule` at run time; naming
  the rule as a literal again, so each of the three lookups gets its own
  specialized walk with `Rule::verdict` folded into a constant test, measured
  2.0%/1.7% faster (`d295cac`).
- **`walk` stepping to the superclass before probing the level it stood
  on.** A lookup that answers where it starts — a slot accessor, a method the
  receiver's own class declares, a builtin's own primitive, which is most
  sends — paid an `Rc<MixinApplication>` clone and drop for a level it never
  read. Moving the step to the three places the search actually continues
  measured 2.5%/1.6% faster (`8eb6360`); this one predates the primitive step
  and would have cost the same regardless of it.
- **The decision split across a function boundary.** The three finishers
  marked `#[inline(always)]`, with their continuations broken out into
  `#[cold]` helpers so only the hot half duplicates into the senders, measured
  1.9%/2.6% faster (`0e71299`).
- **`doesNotUnderstand:` competing for room in the hot match.** Marking it and
  its `super` counterpart `#[cold]` measured a further 0.6–1.3% across three
  sources — the weakest of the five kept changes on its own numbers, but
  consistent in direction, and what turned `bench_send.ns` and `bench_do.ns`
  from entirely above start into overlapping it (`1abe627`).

Four further variants were measured and rejected: outlining the primitive
arm into `#[inline(never)]` helpers; letting the class-object branch finish
its own send instead of falling through an `Option`; `#[inline(always)]` on
the two lookup entry points themselves, which measured 6.6%/6.2% slower, the
worst of the four; and a batched heap read answering a lookup level's
application and its superclass together, which grows the walk's loop state
from two to three words and measured 2.0%/0.9% slower. What is on the send
path now is exactly the four kept changes above plus the one predating this
step; nothing about the design changed — a primitive still fails exactly as
"Primitives" ([dispatch.md](dispatch.md)) describes, into the same
`dispatch::lookup_above`.

**What the next step must not repeat.** 3b rebuilds this same decision path
to insert a cache in front of it, as `resolve` then `act`
([dispatch.md](dispatch.md), "3b — The caches"). A `resolve` that hands its
caller a decision through a settled enum the way this step's first shape did,
or a cache hit and miss sharing a function boundary the uncached path had
just been split out of, would pay the same 15–20% this step paid before it
was found and removed. 3b's own measurement watched for this shape
specifically, and met a relative of it: the split into `resolve` and `act`
pushed the run loop's step out of line ("What the inline caches moved"
below).

### What the inline caches moved

3b, the inline caches ([dispatch.md](dispatch.md), "3b — The caches"), was
measured in four parts, each against the commit it started from, and then
whole, against 3a's head (`49a2a38`, "start") and `be3eb40`. Every part used
the protocol of "What removing the re-entries moved" above: every binary built
with the same `RUSTFLAGS` (`+crt-static` and the function-alignment pin),
the before-binaries in throwaway worktrees; each binary checked to print the
expected answer for every source first; `ns-next bench <source> --iterations
30` over the seven sources, five rounds, the order of the binaries rotating
from round to round; two whole sittings, nothing else running. The import
check and the inlining checks below used `dumpbin` from the Visual Studio
Build Tools, which is installed but not on `PATH`: `/dependents` on every
binary, and `/disasm` on a release copy built with debug information to see
which functions the optimizer kept whole. "Entirely below" and "entirely
above" are the protocol's gate: all five after-medians below, or above, all
five of the group compared with.

**`feedback_base` on its own** (`f8fdcfa`, against its parent `401817a`).
Every activation carries the base, `vm::run` holds it in a local and
`Flow::Switch` carries it, and nothing reads it yet. Change of the mean,
sitting 1 / sitting 2:

| Source | vs parent | vs start | vs `be3eb40` |
|---|---|---|---|
| `bench_loop.ns` | +0.44% / −1.39% | +0.01% / −0.59% | +1.05% / +1.17% |
| `bench_send.ns` | +1.31% / +0.27% | +0.58% / +0.42% | −9.42% / −8.73% |
| `bench_block.ns` | +0.10% / −0.51% | −0.13% / −2.36% | −9.12% / −8.99% |
| `bench_do.ns` | −0.46% / −0.05% | −0.31% / +0.13% | −6.77% / −6.96% |
| `bench_new.ns` | +0.94% / +0.13% | −0.24% / +0.04% | −8.96% / −8.83% |
| `bench_new2.ns` | +1.30% / +1.33% | +0.76% / +0.88% | −6.37% / −7.00% |
| `bench_newarg.ns` | +1.02% / +1.51% | +1.39% / +2.21% | −8.31% / −8.08% |

No group lies entirely above its parent's in either sitting, so the field
was kept as designed — after `slot_base` in the stack entry, loaded by the
switch that needs it — and none of the prepared variants (the base answered
by the push beside the activation, another place in the entry, `run` loading
it instead of `Flow::Switch` carrying it) was needed. `Result<Step, Signal>`
stayed 56 bytes: `Step::Method`'s base fits its padding.

**Resolve, then act** (`7d1f21d` and `1f4d6c6`, against `f8fdcfa`). The
split alone made every source 6.6–13.6% slower, entirely above in both
sittings. `dumpbin /disasm` showed why: before the split, the run loop's
per-instruction step, the lexical and `super` sends inlined into it, was part
of `run_from_host`; after it, the step was a function of its own,
`vm::run::closure$1`, so every instruction — not only every send — paid a
call and returned its `Flow` through memory. Four variants were measured
against the same parent, two sittings each:

| Variant | Change | `bench_loop.ns` | Sources entirely above the parent in both sittings |
|---|---|---|---|
| the split | the refactoring alone | +13.3% / +13.6% | all seven (+6.6% to +13.6%) |
| 1 | `#[inline(always)]` on the three resolves | +16.2% / +15.1% | all seven; the step still out of line, so this tested nothing |
| 2 | `#[inline(never)]` on `resolve_lexical` and `resolve_super` | +4.8% / +5.0% | `bench_loop.ns`, `bench_new2.ns` |
| 3, kept | `#[inline(never)]` on `decide_super_send`; the step inline again | +3.8% / +4.3% | `bench_loop.ns` |
| 4 | 3, `resolve_public` forced inline and the lexical fall-through behind a cold wrapper | +3.4% / +4.9% | `bench_loop.ns`, `bench_new.ns` |

Variant 3 against its parent, sitting 1 / 2: `bench_send.ns` +0.98% /
+1.94%, `bench_block.ns` +1.18% / +3.14%, `bench_do.ns` +1.43% / +2.26%,
`bench_new.ns` +1.28% / +2.32%, `bench_new2.ns` +2.32% / +1.67%,
`bench_newarg.ns` +1.02% / +0.82%; entirely above for `bench_loop.ns` in
both sittings, and for `bench_block.ns`, `bench_do.ns` and `bench_new2.ns` in
one sitting each. The gate was not met on `bench_loop.ns`, and the miss was
accepted and carried into the caches' own measurement: the split exists only to put the caches between `resolve` and
`act`, and the caches had to leave `bench_loop.ns` no slower than 3a's head.

**The caches** (`ead0d85` and `a702875`, against `1f4d6c6`). On the caches'
first shape the step had fallen out of line again. Four reshapings were
tried and checked with `dumpbin` only, untimed, and each left
`vm::run::closure$1` a function of its own: `send_super` kept out of line;
the uncached fallbacks moved into cold functions; `materialize`'s level walk
moved into one out-of-line function; the handler arms and `Unhandled` routed
through one cold call. The step is called once, but its size exceeded even
the inliner's bonus for a single call site. Kept instead: `#[inline(always)]`
on the step closure, after which `vm::run` is 4,910 instructions with no call
per instruction. Means in ms, and the change of the after-mean, sitting 1 /
sitting 2:

| Source | parent | after | vs parent | vs start | vs `be3eb40` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.668 / 11.762 | 10.058 / 10.136 | −13.80% / −13.82% | −9.97% / −10.68% | −9.81% / −8.75% |
| `bench_send.ns` | 12.651 / 12.681 | 11.123 / 11.210 | −12.07% / −11.60% | −11.21% / −10.28% | −19.50% / −19.20% |
| `bench_block.ns` | 21.367 / 21.341 | 19.372 / 19.471 | −9.33% / −8.76% | −8.48% / −8.09% | −17.02% / −16.58% |
| `bench_do.ns` | 7.464 / 7.522 | 6.866 / 6.891 | −8.01% / −8.39% | −7.10% / −8.19% | −13.89% / −14.73% |
| `bench_new.ns` | 7.055 / 7.034 | 6.269 / 6.333 | −11.15% / −9.97% | −7.54% / −6.95% | −16.10% / −15.28% |
| `bench_new2.ns` | 7.934 / 7.893 | 7.289 / 7.297 | −8.13% / −7.56% | −5.58% / −5.04% | −12.42% / −12.82% |
| `bench_newarg.ns` | 9.919 / 9.888 | 9.148 / 9.145 | −7.77% / −7.51% | −5.81% / −5.58% | −14.79% / −13.74% |

Every source's after-group lies entirely below the parent's, start's and
`be3eb40`'s, in both sittings. The first shape of a cache — the design's —
was not timed, since its step was out of line.

**The step whole** (`c9c3c0f`, after invalidation and the stress mode), with
an attribution binary: start plus only the `#[inline(always)]` on the step
closure, and nothing of the caches. Means in ms:

| Source | `be3eb40` | start | attribution | after | after vs start | after vs `be3eb40` | attribution vs start |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1697 / 11.1888 | 11.4004 / 11.3034 | 11.4019 / 11.2734 | 10.1160 / 10.0541 | −11.27% / −11.05% | −9.43% / −10.14% | +0.01% / −0.27% |
| `bench_send.ns` | 13.8400 / 13.8667 | 12.5251 / 12.5185 | 12.6298 / 12.5398 | 11.1194 / 11.1351 | −11.22% / −11.05% | −19.66% / −19.70% | +0.84% / +0.17% |
| `bench_block.ns` | 23.4688 / 23.3867 | 21.2699 / 21.1557 | 21.0996 / 21.2546 | 19.3142 / 19.5689 | −9.19% / −7.50% | −17.70% / −16.32% | −0.80% / +0.47% |
| `bench_do.ns` | 8.0435 / 7.9979 | 7.3997 / 7.3612 | 7.4823 / 7.4336 | 6.8773 / 6.9189 | −7.06% / −6.01% | −14.50% / −13.49% | +1.12% / +0.98% |
| `bench_new.ns` | 7.4893 / 7.5281 | 6.8036 / 6.7761 | 6.8229 / 6.8017 | 6.2731 / 6.3027 | −7.80% / −6.99% | −16.24% / −16.28% | +0.28% / +0.38% |
| `bench_new2.ns` | 8.3419 / 8.3924 | 7.7106 / 7.7195 | 7.8412 / 7.7087 | 7.3149 / 7.2420 | −5.13% / −6.19% | −12.31% / −13.71% | +1.69% / −0.14% |
| `bench_newarg.ns` | 10.6245 / 10.6709 | 9.6230 / 9.6057 | 9.6614 / 9.8064 | 9.1644 / 9.1958 | −4.77% / −4.27% | −13.74% / −13.82% | +0.40% / +2.09% |

**The step is 4.3–11.3% faster than 3a's head and 9.4–19.7% faster than
`be3eb40`**, every after-group entirely below both in both sittings.
`bench_loop.ns`, level with `be3eb40` before this step, is now clearly below
it.

**The forced inlining explains none of it.** The attribution binary is
within ±2.1% of start on every source, every group overlapping, the sign
changing between sittings on three sources: on 3a's code the optimizer inlined
the step anyway, so forcing it does nothing. The caches account for the whole
gain; the forcing is what keeps it, once the caches' hit path makes the step
too large for the inliner — the standing hazard [dispatch.md](dispatch.md)
records.

**The 16-byte entry** (an experiment measured against `c9c3c0f` and not
merged): an index into side
tables of methods and mixins instead of an `Rc`, which makes a `Target` 12
bytes and an entry 16. Change of the mean, sitting 1 / sitting 2:
`bench_loop.ns` +2.02% / +1.36%, `bench_send.ns` +0.84% / +2.33% (entirely
above in the second sitting), `bench_block.ns` +0.45% / +0.92%,
`bench_do.ns` +2.72% / +2.03%, `bench_new.ns` +0.22% / −1.42%,
`bench_new2.ns` +0.08% / +0.14%, `bench_newarg.ns` +0.06% / +0.14%; every other
group overlapping. No source is faster. A plausible cause, not verified: a
hit reads a second, separately allocated vector where the 24-byte entry
already holds the pointer. The side tables also grow with every genuine miss,
not with distinct methods, so a megamorphic site cycling through more
receiver shapes than the global cache holds grows them without bound until an
invalidation — a regime none of the seven sources reaches, and one the cache
stress mode cannot show, since it empties the tables at every safepoint. The user
decided to keep the 24-byte entry.

**What the caches hold** (`--cache-statistics`, a release build of
`c9c3c0f`). An entry is 24 bytes, a polymorphic block 96, a global-cache
entry 32, so the global cache is 32 KB whatever the program.

| Source | Sites (filled) | Global entries filled |
|---|---|---|
| `bench_loop.ns` | 15 (14) | 10 of 1024 |
| `bench_send.ns` | 15 (14) | 11 |
| `bench_block.ns` | 16 (15) | 12 |
| `bench_do.ns` | 12 (11) | 9 |
| `bench_new.ns` | 15 (14) | 12 |
| `bench_new2.ns` | 16 (15) | 13 |
| `bench_newarg.ns` | 16 (15) | 13 |
| `TestActor.ns`, `TextModule.ns` | 0 | 0 |
| `MirrorTestingModel.ns` | 1 (1) | 1 |
| `Ducts.ns`, `NewspeakASTs.ns`, `V5BytecodeSimulation.ns` | 2 (2) | 2 |

The corpus rows are each `corpus/runnable.toml` module run with its entry
selector. No site went polymorphic or megamorphic anywhere: every site these
programs reach sees one receiver id.

**Codegen sensitivity.** Spec-conformant lookup found a lexical send's
decision sensitive to how its result travelled, 3.4–14.7% for a variant that
changed nothing else ("What spec-conformant lookup moved" above). The cached
lexical path does not show it: `bench_send.ns`'s two after-sittings are
0.14% apart and `bench_block.ns`'s 1.3%, inside the jitter every measurement
allows, and a codegen-only change — the attribution binary — moved both by
less than 1%, every group overlapping start's. That closes the finding.

The suite (`cargo test --workspace`, debug build) passes under both stress
modes: about 3 min 9 s under `NS_NEXT_STRESS_MOVES=1` and about 1 min 5 s
under `NS_NEXT_STRESS_CACHES=1`, which invalidates at every safepoint but
moves nothing.

### What the step before 3c moved

The five refinements the user decided after 3b ([dispatch.md](dispatch.md),
"Decisions", "Decided by the user … built before 3c") landed together, and
were measured together and one at a time, on 2026-09-18, against the commit
the step starts from (`f99a29f`) and against `be3eb40`. The protocol is the
one above: every binary, the current tree included, built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the earlier commits in throwaway worktrees; no binary imports
`VCRUNTIME140.dll`; every binary checked to print the expected answer for
every source first; `ns-next bench <source> --iterations 30`, five rounds,
the order of the six binaries rotating from round to round; two whole
sittings, nothing else running. An eighth source joins the seven:
`goldens/bench/bench_megamorphic.ns`, added with the least-recently-used
groups, whose `value` site sees five receiver classes in turn and is the only
source that drives a megamorphic site — 50,000 iterations, the count the
other fixtures use.

**The step is 2.1–4.1% faster on the three instantiation sources and
0.2–3.6% slower on the other five.** Change of the mean against `f99a29f`,
sitting 1 / sitting 2, with the protocol's gate:

| Source | `f99a29f`, rounds 1–5 | mean | after, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 9.91, 9.99, 9.93, 10.05, 9.89 | 9.955 | 10.13, 10.28, 10.29, 10.58, 10.30 | 10.317 | **+3.64% / +0.79%** | entirely above in both |
| `bench_send.ns` | 11.12, 11.01, 11.10, 11.06, 11.16 | 11.090 | 11.22, 11.25, 11.27, 11.28, 11.64 | 11.334 | **+2.20% / +2.37%** | entirely above in both |
| `bench_block.ns` | 19.19, 19.22, 19.43, 19.53, 19.44 | 19.362 | 19.44, 20.16, 19.48, 19.41, 19.83 | 19.664 | +1.56% / +1.61% | entirely above in the second |
| `bench_do.ns` | 6.89, 6.92, 6.84, 6.85, 6.81 | 6.862 | 6.88, 6.92, 7.25, 6.89, 6.92 | 6.973 | +1.62% / +0.19% | overlapping in both |
| `bench_new.ns` | 6.27, 6.25, 6.26, 6.25, 6.23 | 6.252 | 6.17, 6.19, 6.03, 6.03, 6.08 | 6.097 | **−2.49% / −2.05%** | entirely below in both |
| `bench_new2.ns` | 7.23, 7.16, 7.35, 7.24, 7.17 | 7.229 | 7.11, 7.04, 7.00, 7.18, 6.97 | 7.062 | **−2.31% / −2.40%** | entirely below in the second |
| `bench_newarg.ns` | 9.12, 9.26, 9.18, 9.22, 9.17 | 9.190 | 8.94, 8.85, 8.84, 8.93, 9.10 | 8.932 | **−2.81% / −4.10%** | entirely below in both |
| `bench_megamorphic.ns` | 16.51, 16.15, 16.19, 16.18, 16.25 | 16.254 | 16.37, 16.80, 16.82, 16.39, 16.77 | 16.628 | +2.30% / +1.78% | overlapping in both |

**Every source stays well below `be3eb40`**, the comparison carried through
every measurement up to step 3: `bench_loop.ns` −6.7% / −8.5%,
`bench_send.ns` −18.2% / −17.6%, `bench_block.ns` −15.4% / −16.1%,
`bench_do.ns` −13.0% / −14.0%, `bench_new.ns` −17.6% / −17.3%,
`bench_new2.ns` −15.5% / −15.1%, `bench_newarg.ns` −15.2% / −15.2%,
`bench_megamorphic.ns` −7.1% / −7.8%, every group entirely below in both
sittings.

**`vm::run`'s step is still inline.** `dumpbin /disasm` on a release build
with debug information, on all five binaries — the base, the three
intermediate checkpoints and the head — finds no `vm::run::closure` function
in any of them: `vm::run` is one function of 4,910 instructions at
`f99a29f`, then 4,860, 4,878, 4,878 and 4,926. It gained 16 instructions
across the whole step and never approached the cliff that splitting
`resolve` from `act` hit ("A standing hazard" in
[dispatch.md](dispatch.md)).

Each change was measured against its parent in the same two sittings, which
is what attributes the movement. Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `f99a29f` | after 1–3 | after 4 | after 5 | after 6 |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.060 / 11.123 | 9.955 / 10.093 | 9.972 / 9.973 | 10.181 / 10.304 | 10.201 / 10.176 | 10.317 / 10.173 |
| `bench_send.ns` | 13.855 / 13.753 | 11.090 / 11.072 | 11.164 / 11.062 | 11.415 / 11.358 | 11.301 / 11.363 | 11.334 / 11.334 |
| `bench_block.ns` | 23.247 / 23.356 | 19.362 / 19.297 | 19.440 / 19.399 | 19.492 / 19.570 | 19.678 / 19.564 | 19.664 / 19.607 |
| `bench_do.ns` | 8.013 / 8.012 | 6.862 / 6.879 | 7.053 / 7.107 | 7.174 / 7.126 | 7.140 / 7.153 | 6.973 / 6.893 |
| `bench_new.ns` | 7.400 / 7.375 | 6.252 / 6.228 | 6.001 / 6.022 | 6.007 / 6.059 | 6.042 / 6.026 | 6.097 / 6.100 |
| `bench_new2.ns` | 8.354 / 8.311 | 7.229 / 7.230 | 6.984 / 6.955 | 7.073 / 7.032 | 7.031 / 6.991 | 7.062 / 7.056 |
| `bench_newarg.ns` | 10.526 / 10.510 | 9.190 / 9.297 | 8.873 / 8.872 | 8.954 / 8.935 | 8.881 / 8.897 | 8.932 / 8.915 |
| `bench_megamorphic.ns` | 17.897 / 17.891 | 16.254 / 16.209 | 16.196 / 16.248 | 16.441 / 16.395 | 16.438 / 16.431 | 16.628 / 16.498 |

- **Flat metaclasses, the primary factory as an entry of the metaclass
  mixin's method table, and the resolver's class-side name check** (`a466c5d`,
  against `f99a29f`). **Instantiation is 3.3–4.6% faster**: `bench_new.ns`
  −4.02% / −3.31%, `bench_new2.ns` −3.40% / −3.80%, `bench_newarg.ns`
  −3.45% / −4.57%, all three entirely below in both sittings. `X new` is now
  an ordinary cached send of an entry found by the ordinary lookup, where it
  was a special case tested ahead of every lookup. Of the five sources that
  instantiate nothing in their loop, four are level — `bench_loop.ns`,
  `bench_send.ns`, `bench_block.ns` and `bench_megamorphic.ns`, −1.19% to
  +0.67%, one group entirely below. The fifth, `bench_do.ns`, is **2.78% /
  3.31% slower, entirely above in both sittings**, which nothing on its path
  explains: it neither instantiates nor sends to a class object in its loop.
  It is the uniform shift on unchanged executed code that this repository has
  recorded three times ("What 2c moved" and the two findings in "What
  removing the re-entries moved", both above), and the function-alignment
  pin, which removes a 64-byte phase shift, does not remove it.
- **The lexical global-cache key** (`da8beed`, against `a466c5d`). **This is
  the change that costs the send benchmarks**: `bench_loop.ns` +2.10% /
  +3.32% and `bench_send.ns` +2.25% / +2.68%, each entirely above in the
  second sitting; `bench_megamorphic.ns` +1.51% / +0.91%, entirely above in
  the second; the other five overlapping, +0.10% to +1.71%, none below. A
  control binary bisected it into its two halves; see "Which half of the
  lexical key costs the send benchmarks" below.
- **Call-site numbers off the wire** (`c9097d1`, against `da8beed`). Nothing
  beyond noise anywhere, −1.24% to +0.95%, every group overlapping except
  `bench_loop.ns` entirely below in the second sitting and `bench_newarg.ns`
  entirely below in the first. That is the expected answer: the change is in
  the encoder, the decoder and the opcode table, and a run from source
  touches none of it. The step's instruction count did not change at all,
  4,878 before and after.
- **The least-recently-used groups** (`393311f`, against `c9097d1`). Nothing
  beyond noise either, −0.25% to +1.23% on seven sources, every group
  overlapping; `bench_do.ns` is −2.34% / −3.65%, entirely below in the second
  sitting, which nothing on its path explains and which gives back most of
  what the flat-metaclass and factory change cost it. On
  `bench_megamorphic.ns`, the source added for this comparison, +1.16% /
  +0.41%, overlapping in both.

  **What that source can and cannot settle.** Its megamorphic site sends
  every hit to the global cache, so it measures the *probe*: one hash and one
  cache line, against 3b's two hashes at two independent places in the table.
  It cannot measure the *replacement* rule. `--cache-statistics` after a run
  reports 29 sites, 26 filled, one megamorphic, and 27 of 1024 global slots
  used, five of them the megamorphic site's own. With 27 keys over 512
  groups the expected number of collisions is under one, and a group would
  have to hold three keys before anything is evicted at all, so on the hot
  path every read almost certainly answers at the first slot and the
  replacement rule almost certainly never runs. Deciding least-recently-used
  against 3b's insert on numbers needs cache pressure — many more hot keys
  than any fixture has, or
  a debug build's small `NS_NEXT_GLOBAL_CACHE_ENTRIES` — and no measurement
  has supplied it, so the rule stands on Strongtalk's precedent
  ([dispatch.md](dispatch.md), "The global cache").

**What the caches hold** (`--cache-statistics`, the release build of the
step's head). Unchanged from 3b on the seven sources — 12–16 sites, 9–13
global entries, no site polymorphic or megamorphic — and, for the eighth:

| Source | Sites (filled) | Polymorphic blocks | Megamorphic sites | Global entries filled |
|---|---|---|---|---|
| `bench_megamorphic.ns` | 29 (26) | 0 of 1 allocated | 1 | 27 of 1024 |

The one block was allocated when the site went polymorphic and given back
when it went megamorphic, which is why the table holds one and uses none.

#### Which half of the lexical key costs the send benchmarks

The lexical key's commit does two separable things. **The key's shape**: the
global key gains a fourth component, N's application, so `GlobalEntry` and
`SendKey` each carry a `DispatchId` more, `probes`, `holds`, `get` and
`insert` each take an argument more, and `cached` and `miss` take a
`lexical_of` closure that answers it — all of which `vm::run`'s step inlines.
**The two probes**: `send_lexical_first` reads two global-cache rows before
it resolves, which is what the wider key makes safe, and which is the point
of the change.

A control binary separates them: the commit with the two probes cut out of
`send_lexical_first` and everything else kept. `send_lexical_first` is
`#[inline(never)]`, so the cut cannot touch the step — and does not:
`vm::run` is 4,878 instructions in the control and in the commit, and the two
functions' instruction streams are identical once relocated addresses are
normalized, against 4,860 in the parent. Anything the control measures
against the commit is therefore the probes and code placement; anything it
measures against the parent is the 18 instructions the step gained. Two
sittings, all three binaries in each, the same protocol.

| Source | control vs parent (the key's shape) | commit vs control (the two probes) | commit vs parent (both) |
|---|---|---|---|
| `bench_loop.ns` | **+1.89% / +1.48%**, above in the second | −0.07% / +0.04%, overlapping in both | +1.82% / +1.52% |
| `bench_send.ns` | **+2.25% / +1.66%**, above in both | +0.41% / +0.57%, overlapping in both | +2.67% / +2.23% |
| `bench_block.ns` | +0.10% / +0.21%, overlapping | +1.23% / +0.49%, overlapping in both | +1.33% / +0.70% |
| `bench_do.ns` | −2.18% / +0.34% | +3.45% above / +1.45% overlapping | +1.20% / +1.80% |
| `bench_new.ns` | +1.31% / +0.39%, overlapping | +0.47% / +1.38%, overlapping in both | +1.78% / +1.77% |
| `bench_new2.ns` | +0.34% / +0.44%, overlapping | −0.08% / −0.30%, overlapping in both | +0.26% / +0.13% |
| `bench_newarg.ns` | +0.46% / −0.00%, overlapping | +0.24% / +0.46%, overlapping in both | +0.70% / +0.45% |
| `bench_megamorphic.ns` | **+1.00% / +1.54%**, above in both | +0.52% / −0.38%, overlapping in both | +1.52% / +1.16% |

**The key's shape carries the cost; the probes carry none of it.** The
control alone reproduces the whole of `bench_loop.ns`'s loss and most of
`bench_send.ns`'s and `bench_megamorphic.ns`'s, and adding the probes on top
of it moves no source beyond the noise. That confirms by measurement what the
site counts only argued: the probes run once per still-vacant lexical site,
and no source fills more than sixteen sites in a 50,000-iteration run.

So the mechanism is the widened key inside the inlined step, not work
executed per send — the step's 18 extra instructions, on a path every send
runs. It is not entry size: `GlobalEntry` is 32 bytes and `CacheEntry` 24 at
the parent, at the control and at the commit alike, the fourth component
having fitted in padding the struct already had. Two limits worth stating.
The control also deletes a cold function, `global_hit`, which no longer has a
caller, so code placement differs between the control and the commit as well
as the probes; the conclusion rests on that pair measuring *null*, which a
placement effect could in principle have cancelled, and on the parent
comparison measuring positive where the step demonstrably differs. And
"the widened key" is still four things at once — the wider entry, the wider
key, the extra arguments and the extra closure parameter — which no further
binary separates.

The step's head no longer has the fourth component as a separate field: the
least-recently-used groups folded it into the rule as
`CacheRule::Lexical(DispatchId)` and gave `GlobalEntry` its field back, at no
measured cost either way. What the control measured is the cost of the key
knowing N at all, in whichever of the two shapes it is carried.

## The trampoline

Sends do not recurse on the Rust stack. Every running activation lives on the
activation stack, and the run loop ("The VM" above) is the only place a running
computation lives, and nothing re-enters it from Rust ("No re-entry from Rust"
below). This was step 1b of Milestone 4, and it
landed the way the stack did: against a green suite, with its own
before-and-after measurement ("What the trampoline moved" above).

### Deciding, then running

**The trampoline falls out of the stack rather than driving it.** The activation
stack is what a trampolined run loop needs, so replacing the Rust-stack
recursion with a loop that pushes is the same change viewed from the other
side. Getting there split every send so that it *decides* without *executing*:
lookup, `doesNotUnderstand:`, native dispatch and the nested-class accessor stay
in `interpreter/` (`inline_cache::send_public`, `send_lexical` and
`send_super`, over the lookups in `resolve.rs`), unchanged in what they
produce, but they answer a `Step` — a value, a method to activate, or a block to activate and what to do
with its value — instead of activating it. That keeps `vm.rs` free of dispatch
logic: what moved into the loop is the act of running a body, never the
decision of which body. The loop gathers a send's arguments from the caller's
registers into one reusable buffer, and a push copies them from there into the
callee's window, so a send allocates no vector of its own.

Three things the trampoline settles on its own:

- **`Block value` is not a callback.** It is a native that answers the closure
  as a `Step::Block`; activating a closure is what the loop already does, so no
  Rust frame sits in the middle, and nothing was rewritten in Newspeak.
- **Nor are the control-flow natives** (decided 2026-09-12).
  `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:` and `ifFalse:ifTrue:` on a Boolean
  answer the block to activate, exactly as `value` does, and its value is the
  send's (`Continuation::Deliver`). `and:` and `or:` do the same with
  `Continuation::Truthy`, which coerces the block's value by
  `builtins::boolean::truthy`'s rule when it returns; `whileTrue:` and
  `whileFalse:` on a block answer the condition block with
  `Continuation::WhileCondition`, which, when it returns, activates the body or
  ends the loop (`builtins::block::after_condition`), and the body's
  `Continuation::WhileBody` activates the condition again. The continuation
  rides on the block activation's stack entry and `vm::finish` applies it,
  through `Mutator::resume`, when it pops the activation, so a loop's
  condition and body replace each other on the stack instead of nesting. This
  is Primordial Soup's model with one activation
  fewer: there `True>>ifTrue: then = (^then value)` is a Newspeak method whose
  `value` is primitive 156, `ActivateClosure`, which pushes a frame and returns
  to `Interpret` (`vm/interpreter.cc`), so nothing in that VM ever re-enters the
  interpreter from C. What decided it is the depth rule below: a callback native
  costs native stack per level under the reference lowering only, growing with
  the static nesting of `and:` and `while` blocks, so no fixed native stack
  would guarantee that the count trips before the native stack under both
  lowerings. `do:` answers its block as a step too ("No re-entry from Rust"
  below), and moves to Newspeak in step 6 of M4's order of
  work ([core-library.md](core-library.md), [PROGRESS.md](../PROGRESS.md)).
- **Recursion depth is a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded
  artifact. The limit counts every activation except a block activation that a
  builtin control-flow native performs for a `SendInlinable`, the send only the
  reference lowering emits where the optimized one inlines. That makes the
  count agree under both lowerings at every point of every program, so the
  reference lowering checks an overflow too ([compiler.md](compiler.md), "How
  the limit counts"). `SendInlinable` and the counter arrived together.
  Measured on Windows's 1 MB main-thread stack before the trampoline, the
  reachable recursion depth was 818 inside an `ifFalse:` block (release
  build), and 307 through `do:` — below the 1000 that Newspeak's own
  `KernelWeakTests.ns` (`deepStack: 1000`) expects. The golden
  `goldens/limits/thousand_deep_recursion_runs.ns` runs exactly that
  recursion, under both lowerings.

### The depth limit

Decided 2026-09-12. Once sends and the control-flow natives no longer recurse
and nothing re-enters the run loop from Rust ("No re-entry from Rust" below),
a recursion runs out of one resource only, the memory its activations take,
and one counted bound limits it:

| Bound | Counts | Value | Error text |
|---|---|---|---|
| L | activations, by the rule in [compiler.md](compiler.md), "How the limit counts" | 100,000 | `stack overflow: recursion too deep` |

It is `MAX_ACTIVATION_DEPTH` in `crates/ns-runtime/src/interpreter/mod.rs`.
L bounds memory: an activation is a stack entry and a window of registers in
two `Vec`s, so L can be far larger than any native stack would allow. It
bounds it up to a factor: above each counted activation sit at most as many
exempt ones as blocks nest — each exempt block was created by the activation
below it, so a chain of them descends the lexical nesting — and blocks nest at
most 128 deep, the parser's ceiling in source and the decoder's, derived from
it, in an artifact ([image-format.md](image-format.md), "Verification"). So the
stack holds at most 129 × L entries, 12.9 million, whose entries alone take
about 1.3 GB in a release build (104 bytes each, before their registers). Only
code that carries `SendInlinable` gets there — the reference lowering's, or an
artifact's — recursing through 128 nested control-flow blocks at every level;
the optimized lowering never emits it, so under it the stack holds at most L
entries. The limit agrees under both lowerings, because the control-flow
natives, `do:`, instantiation and the nested-class accessor are all trampoline
steps: no recursion costs native stack under one lowering and not the other.
The error text does not name its number, as Lua's, CPython's, V8's and Nova's
do not, so that changing the value later changes no recorded answer. It
unwinds to the entry send like every other Newspeak error; nothing catches it
yet.

- **L is checked on every push.** `Mutator::push`, the one function every
  activation the runtime pushes goes through, refuses a counted push when
  `Heap::counted_depth` has reached the limit. The heap keeps that count as a
  running total on push and pop, so an error path — which pops through
  `vm::unwind` like any other — leaves it exact. Whether an activation counts
  is recorded on its stack entry. Only the block arm of
  `Mutator::push_step` ever records an exemption; a method, and every body
  an instantiation or a nested-class accessor runs — an initializer level, a
  superclass clause or its message — are counted by construction.
- **The exemption is marked by the arguments.** A block a control-flow native
  activates carries `StepArguments::ControlFlow` — it has no arguments, and only
  those natives build that variant — and `push_step` exempts it when the send
  was a `SendInlinable` and the block's `captured_frame` is the reified frame
  of the activation that sent it ([compiler.md](compiler.md), "How the limit
  counts"). A native loop's next block goes through
  `Mutator::push_continuation`, which exempts it only if the one it
  replaces was; the loop forwards that entry's flag and decides nothing. The
  mark rides on the arguments rather than in a field of its own
  because the field measured 4–8% slower ("What the trampoline moved" above);
  so a future native that activates a block with no arguments for any other
  reason must not build `ControlFlow`. `do:`'s block has an argument, so it is
  always counted. `&` and `|` share `and:`'s and `or:`'s
  native and so carry the mark too, which is harmless: no lowering emits a
  `SendInlinable` for them, since `control_flow::recognize` accepts only the
  eight inlined selectors, and a crafted artifact that marks one meets the
  captured-frame condition like any other.
- **The CLI runs every command on a thread it spawns** with an explicit stack
  (`std::thread::Builder`, `INTERPRETER_STACK_BYTES` in
  `crates/ns-cli/src/main.rs`): 8 MiB, four times the 2 MiB libtest threads
  the suite runs the deepest nesting the parser accepts on, since what still
  recurses on the Rust stack is the front end and the compiler, bounded by the
  parser's nesting ceiling ("What the trampoline moved" above). It is
  reserved, not committed, so only what a program uses is backed by memory.
  The whole command runs there, not only its sends, because the symbol
  interner is thread-local (`symbol.rs`) and every symbol a command interns
  must be interned on the thread that uses it, so the thread would be needed
  for that alone. The goldens that recurse through
  `do:`, instantiation and the nested-class accessor — `do-recursion`,
  `instantiation-recursion` and `accessor-recursion`, run in a debug build —
  end at L like any other recursion, and pin that the error, not an abort, is
  what a program sees.

Why the limit is a count and not a memory bound, as Cog and Primordial Soup
have, or a stack-pointer check, as V8 and HotSpot have: both trip at different
depths under the two lowerings, whose activations differ in number and size,
and the second also between a debug and a release build.

### A superclass chain is walked iteratively

`climb_levels` and `descend_levels` (`interpreter/instantiate.rs`) walk it with
an explicit list, as a state machine `Mutator::advance_instantiation`
resumes whenever a body it answered returns: each level's factory arity
checked and its clause message evaluated from the class upward, each level's
arguments kept, then the initializers from the top down — the order of every
observable effect a per-level recursion had. Recursing once per level would
escape the limit: a level with no initializer code pushes no activation, so
the count would not see a long chain of such levels, and its length is an
image's to choose — the same exposure `metaclass_of` and `dispatch::walk` were
made iterative for.

### What the golden outcomes are trusted on

`Signal::Return` is an unwind of the activation stack to its target entry
("Non-local return" above), with the semantics it had when it propagated
through Rust `?`. This is the part of the trampoline the golden outcomes are
trusted hardest on: the reference lowering runs on the same VM, so it cannot
cross-check the unwind — both ways of running a program unwind through the
same code.

### How other systems bound recursion depth

The limit's design started from what the prior art does with a recursion that
outgrows its stack:

| System | Model | Depth limit |
|---|---|---|
| Primordial Soup | Fully trampolined: a closure's `value` primitive is `ActivateClosure`, which pushes a frame and returns to `Interpret`, so Newspeak code never recurses on the C stack. The frame stack is 1024 slots (`kStackSlots`, `vm/interpreter.h`); on overflow `StackOverflowOrInterrupt` flushes every frame but the top one into heap activations (`FlushAllFrames`, `vm/interpreter.cc`). | None. Depth is bounded by the heap; `vm/virtual_memory_*.cc` treats a failed allocation as `FATAL`. |
| Cog | Stack pages. `handleStackOverflow` (`src/spur64.stack/interp.c`) moves frames to the next page, first divorcing that page's frames into heap contexts if it is in use (`divorceFramesIn`). | None. Depth is bounded by memory. |
| Nova | Recursive on the Rust stack. `Agent::check_call_depth` (`nova_vm/src/ecmascript/execution/agent.rs`) counts execution contexts. | 3500 — "experimental number that caused stack overflow on local machine" — then a `RangeError`. |
| SOMns | A Truffle AST interpreter, recursive on the Java stack. | None. A `StackOverflowError` is a JVM failure; one of its test scripts raises the thread stack (`-Xss4096k`). |
| Lua 5.4 (not in `prior-art/`) | Lua-to-Lua calls run in the VM loop without recursing in C; C functions and metamethods re-enter it. | Two: the Lua stack at 1,000,000 slots (`LUAI_MAXSTACK`), and nested C calls at 200 (`LUAI_MAXCCALLS`) — "stack overflow" and "C stack overflow". |
| CPython ≥ 3.12 (not in `prior-art/`) | Since 3.11 a Python-to-Python call consumes no C stack. | Two: `sys.setrecursionlimit` (default 1000) counts Python frames only, and C recursion — builtins, extensions — is guarded separately; `RecursionError`. |
| V8, HotSpot | Frames on the native stack. | The stack pointer against a limit (V8's `--stack-size`, HotSpot's `-Xss`), so the reachable depth depends on frame sizes and the compilation tier. |

Newspeak's own tests assume the Smalltalk behaviour: `KernelWeakTests.ns`'s
`deepStack: 1000` exists to overflow Primordial Soup's frame stack and force a
flush.

The Smalltalk-lineage VMs make depth a question of memory, not a count, and a
trampolined VM could do the same, since its activation stack is a `Vec` on the
heap. ns-next counts instead, because a memory bound would trip at different
depths under its two lowerings — the reference lowering's activations are more
numerous and hold more registers — and the reference lowering is meant to check
an overflow too. What the count includes is decided in
[compiler.md](compiler.md), "How the limit counts". A fixed count like Nova's is
otherwise what a runtime falls back on when the stack its evaluator recurses on
cannot grow. The systems that took their calls off the native stack — Lua,
CPython from 3.11 — separate the two resources that can then run out, with a
generous bound on the language's own depth and a tight one on native
re-entries; Primordial Soup and Cog need no second bound because nothing in
them re-enters the interpreter from native code. ns-next is in their position:
nothing re-enters its run loop from Rust ("No re-entry from Rust" below), so
one bound, L ("The depth limit" above), is all it needs.

### No re-entry from Rust

Rust enters the run loop only as its outermost caller — a host's `send` or
`instantiate`, a stateless binary's entry among them — and never from inside a
running computation. `do:`, instantiation and the nested-class accessor, the
three paths that used to run Newspeak code from Rust, are trampoline steps.
This was step 2a of Milestone 4, decided 2026-09-13, and it is what the root
discipline stands on ([memory.md](memory.md), "No Rust frame is live below a
safepoint"): between two instructions no Rust frame between the host and the
loop holds a value.

All three follow the pattern `whileTrue:` set: a decision answers an activation
together with a `Continuation`, the continuation rides on the activation's
stack entry, and `Mutator::resume` applies it when the activation returns.
The semantics and the order of every observable effect are what they were
when these paths ran in nested runs of the loop.

- **`do:`** (`builtins/collections.rs`) walks a snapshot of the elements. An
  empty collection, or an argument that is not a block, answers the receiver at
  once: a non-block stands for its own value, so handing it the elements would
  observe nothing. Otherwise `do_step` answers `Step::Block` for the first
  element with `Continuation::DoElement`, whose `DoLoop` carries the receiver,
  the block, the snapshot and the next index; `resume` hands it to
  `after_element`, which activates the block for the next element or answers
  the receiver. `collections::execute` matches the enumerated
  `CollectionPrimitive` a lookup found ([dispatch.md](dispatch.md),
  "Primitives"): `Enumerate` (`do:`) answers `do_step`'s step directly, and
  every other primitive's leaf result is lifted into a step through
  `leaf_step`. `Array` and `OrderedCollection` share this one function; which
  of them answers `at:put:` and which `add:` is decided by their own lists in
  `builtins/primitive.rs`, not here. Each element's activation is counted, like any block a `value`
  send activates; a `^` in it unwinds like any other activation's, and its
  arity is checked by the same `check_block_arity`. Its argument is a
  one-element vector, allocated per element: a `StepArguments` variant that carried
  one value would avoid that, and would change `Step`'s layout, which the
  trampoline measured as a 4–8% risk ("What the trampoline moved" above).
- **The nested-class accessor** (`interpreter/nested.rs`) answers the class as
  a value when the receiver's cache already holds it, and also when the
  declaration has no superclass clause, whose superclass is `Object` with no
  code to run. Otherwise it answers the clause's activation, a
  `Step::SuperclassClause`, pushed with the declaring class as its current
  class and continuing with `Continuation::NestedClass`. The boxed
  `NestedClassCreation` it carries holds the enclosing object, the index of
  its cache slot, the declaration and the declaring class; `resume` hands it to
  `nested::after_clause`, which does steps 3 to 5 — the application, the class
  object, the cache ([object-model.md](object-model.md)) — and answers the
  class. While the clause runs, the class's cache slot itself says that its
  creation is underway: `push_step` writes a marker into it once the clause's
  push has succeeded (`nested::begin_creation`), and `resume` or
  `Mutator::abandon` sets it back to nil when the activation is popped
  (`nested::end_creation`), before `after_clause` caches the class there. An
  access that finds the marker is the cyclic-definition error; the slot is
  already keyed by the enclosing object and the declaration, the pair a cycle
  is, so the check is the one read the accessor makes anyway. The marker is
  `false`, a primitive the image format already has: nothing but the accessor
  reads a hidden slot, so no Newspeak code sees it, and no image holds one,
  since only a host snapshots, between runs; an image that puts `false` in a
  hidden slot itself gets a cyclic-definition error, the same kind of
  wrong-but-safe answer as any other non-class value it puts there. The reset
  replaces only the marker: the decoder does not bound `InitializeSlot`'s field
  index, so an image's clause body can overwrite the slot, and a debug build
  counts the marks rather than asserting on the slot's contents, which such an
  image could turn into a panic. `abandon` is `resume`'s counterpart for a
  continuation whose activation `vm::unwind` throws away rather than returns
  from: the loop forwards every popped entry's continuation to it and decides
  nothing, as `vm::finish` forwards to `resume`.
- **Instantiation** (`interpreter/instantiate.rs`) is a state machine.
  `decide_instantiation` allocates the instance and walks the superclass chain
  ("A superclass chain is walked iteratively" above) with its state, an
  `Instantiation` — the instance, the levels reached with their factory
  arguments, and the phase: climbing at a level and its superclass, or
  descending at an index. The walk does the synchronous parts itself — each
  level's arity check, the clause selector check, a level with no code — and
  answers an activation as soon as code must run. A clause message's
  arguments are a `Step::ClauseMessage`, and an initializer with a level below
  it that still has code a `Step::InitializerThen`; each carries the state
  into its activation's `Continuation::Instantiate`, and
  `advance_instantiation` carries the walk on when the body returns. The
  walk's last initializer is a `Step::Initializer`, and its
  activation delivers its own value, since that value is the instance: both
  lowerings end an instance initializer by answering `self` and discard a
  top-level `^` (`compiler::lower`'s `finish_body`, `compiler::reference`'s
  `end`), and `new_answers_the_instance_its_initializers_ran_on`
  (`crates/ns-runtime/tests/instantiate.rs`) pins it under both lowerings,
  from a host's `new` and from a program's. A decoded image's initializer is
  not held to that: `new` then answers whatever that code answers, which is
  wrong but safe — no panic, the image's own program running. An instantiation
  with no code at all answers the instance as a `Step::Value`.
  A factory send is an ordinary decision: the lookup finds the metaclass
  mixin's `MethodEntry::Factory` and `act` runs `decide_factory`, which
  reaches `decide_instantiation` ([dispatch.md](dispatch.md), "Resolve, then
  act"). `Interpreter::instantiate` is the host entry, shaped like `send`.

  The state is always boxed, and the box is reused: the walk runs in a box
  taken from the interpreter's spares (`walk_state`), and when the walk ends —
  at its last initializer, or with no code left — the box goes back, emptied
  of every handle, with its list of levels keeping its room (`retire_walk`).
  So once one instantiation has run, the next allocates the instance's fields
  and nothing else, one with code at several levels included; allocating the
  box and the list per instantiation measured 3–13% slower on the
  instantiation benchmarks. The spares are capped at 16 boxes of 64 levels'
  room, because the boxes held at once grow with how deeply instantiations
  nest and the list with the superclass chain, an image's to choose.

**Every body is inline in its `Step`.** `Step::Initializer`,
`Step::InitializerThen`, `Step::ClauseMessage` and `Step::SuperclassClause` each
carry only what their push cannot find from their level or declaration — the
code and slot base of an initializer are its level's, found when it is pushed
(`Mutator::initializer_body`), and a clause's are its declaration's — so
none needs a boxed description of its activation; boxing one per body, beside
a fresh box for the walk's state, made a chain of two levels with code 15–18%
slower than running it in a nested run of the loop had been
(`bench_new2.ns`, `bench_newarg.ns`). Each is smaller than `Step::Method`, on
purpose: an `Initializer` with an optional continuation, as large as
`Step::Method`, left `Result<Step, Signal>` at the 56 bytes the hot path was
measured at and still made every benchmark 4–6% slower, none of which
instantiates in its loop — the same effect `StepArguments::ControlFlow`'s comment
describes, presumably the discriminant's niche. Splitting it into
`Initializer` and `InitializerThen`, which takes the receiver from its state,
measured level with before. `a_decision_stays_the_size_the_hot_path_was_measured_at`
(`interpreter/mod.rs`) pins the size; it cannot see the niche. Every body is
counted.

**One host entry.** `Interpreter::send` and `Interpreter::instantiate` fetch
what the host's roots hold, decide, and hand what they decided to
`Interpreter::run_from_host`, which answers a value as it is and otherwise
pushes the activation and runs `vm::run` on it; what the run answers goes back
to the host rooted ([memory.md](memory.md), "Host roots").
It is the one place Rust enters the loop. A debug build checks there that the
loop is not already running — nothing it calls can reach `run_from_host` —
that the stack is left as deep as it was found, and that no nested class's
creation outlives the run. That the loop is never nested is also structure,
which the debug assertion only confirms: a decision, a native or a
continuation is handed only a `&mut Mutator`, which offers no way back to the
`Interpreter` and so no `run` at all ([memory.md](memory.md), "The brand").

`vm::send` still takes the step state's reusable argument buffer and gives it
back, because the decision needs `&mut Mutator` while it reads the
arguments. No decision runs code any more, so none takes a second, fresh
buffer of its own. `Interpreter::reenter`, `invoke_closure`,
`builtins::call_block`, the count of re-entries and its bound, and that
bound's error, `stack overflow: too many nested native calls`, are gone with
the re-entries.

**Three recorded outcomes changed.** The three goldens now under
`goldens/limits/` that recurse through `do:`, through instantiation and through
the nested-class accessor ended in
`stack overflow: too many nested native calls`, the bound on re-entries, and
end in `stack overflow: recursion too deep`. Each of those recursions is made
only of counted activations now, so it ends at L like any other recursion;
the limit the old text reported no longer exists. That is why the change is
right — it was not recorded because the code produced it. The interpreter
thread's stack was re-derived from what still recurses on the Rust stack
("The depth limit" above).

The step was measured whole against the commit it started from: "What
removing the re-entries moved" above. Instantiation, `do:` and `bench_send.ns`
are level with it or faster; `bench_loop.ns` and `bench_block.ns`, which run
none of the changed paths in their loops, came out 1.4–1.9% slower, an open
finding that step 2b's measurement puts at +1.1%, overlapping, and −2.5%
("What 2b moved" above).

### How this meets the liveness requirements

Reification is not a compromise against
[liveness-and-reflection.md](liveness-and-reflection.md); it is that document's
own unifying principle applied one level down. The interpreter level is the
source of truth and a JIT is an accelerator that must be able to deoptimize to
it. A stack is an accelerator over the reified activation model, and
reification is its deoptimization.

That resolves the one requirement that looks like a conflict. Edit-and-continue
must be able to add a local to a method while an activation of it is live, and
only the top entry's register window can grow in place. The answer is that
editing a method takes its affected activations off the stack first — reified
and divorced while still running, the operation Cog's `divorceAllFrames`
performs on every stack page before a snapshot — and a divorced frame's `temporaries`
is a growable `Vec`. Continuing an activation from its divorced frame needs a
way to put the frame back on the stack, which does not exist yet; it is the
same missing piece as resuming a decoded frame
([image-format.md](image-format.md), "Planned: suspend and resume"). The document already accepts that entering the debugger forces the
frames it touches out of their fast representation; this is the same step, one
level below the JIT.

## Planned: closure conversion

What would change: a block stops reaching its method's variables through the
method's frame. A captured variable that is only read is copied into the closure
when the closure is created; one that is written lives in a small heap box that
the method and the block share. The resolver decides which is which, so every
lowering sees the same conversion. Placed after the inline caches and before the
collector, whose retention it affects most.

The activation stack keeps ns-next's by-reference model and pays for it with a
divorce: a reified frame copies all its registers into the arena when its
activation returns, because a surviving block may still read them. Conversion
removes that need. It does not remove reification — a block with `^` still needs
its home activation, and so does the debugger — which is why it can come after
the stack rather than before it: it changes where variables live, not what an
activation is.

**Every Smalltalk and Newspeak VM that moved to stack frames made this change to
do it.** Cog's closure compiler introduced copied values and remote temps
(`pushClosureNumArgs:copiedValues:`, `pushRemoteTemp`) precisely so frames could
live on a stack, and Primordial Soup does the same (`PushClosure`,
`PushIndirectLocal`). Reaching the frame by reference is what systems do whose
frames live in the heap anyway — Smalltalk-80's Blue Book, ns-next before the
activation stack — or whose JIT can remove the cost: SOMns materializes the frame
for every block
(`prior-art/somns/src/som/interpreter/nodes/literals/BlockNode.java`,
`frame.materialize()`) and relies on Graal's partial escape analysis, which
removes the materialization only when the block is inlined. Outside that
lineage, V8's context allocation, CPython's cells and Scheme's stack-based model
are the same conversion, and Lua's upvalues are a per-variable middle ground: an
open upvalue points at its stack slot and is closed — that one variable copied —
when the function returns.

What it changes in cost, for a method that creates a non-inlined block:

| | By reference (today) | Converted |
|---|---|---|
| Per activation that creates a block | a reified frame, plus a copy of every register at return | the closure's copied values, plus a box per written captured variable |
| A block reading a captured variable | one hop per nesting level, then a live-or-dead branch | a closure field, or closure → box; independent of depth |
| The method reading its own captured variable | a register | a register if only read; through the box if written |
| What a surviving block retains | the whole frame, including registers it never reads | exactly what it captured |
| A later JIT | every local a block can reach stays in memory once a block exists | copied values are ordinary values; escape analysis can remove a box |

For the interpreter the difference is small — the stack's large gain, that an
activation creating no block allocates nothing, is the same under both — so the
case for conversion is the collector's space safety and the JIT's freedom, not
the interpreter's speed. `bench_block.ns`, which creates a block per iteration,
is the benchmark that would show what it saves. What it costs is breadth: the
resolver's capture analysis, the compiler's instructions for boxes and copied
values, and the closure record in the image format.

## Planned: JIT tiers

Beyond inline caches nothing is designed. The tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md), and the constraint that
binds it is that no native code may ever enter an image.

## Decisions

- **One engine.** The tree-walker was retired because it could not represent a
  suspended computation; golden outcomes protect behaviour, and the reference
  lowering is the differential oracle.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, so the reference lowering and a JIT share it. A send is split into a
  decision and an activation, which preserves this: the decision stays in
  `interpreter/`, and only running what it decided is the loop's.
- **`program_counter` is stored only at observable points**, not at every instruction — a
  deliberate hot-path trade the debugger milestone may have to revisit.
- **Activations are stack entries, reified on demand**, because the
  always-reified model cost about 41% of a send, before the arena, for a structure
  most activations never expose. The arena took the `Rc` box and the `RefCell`
  borrow, about 15% of `bench_loop.ns`. The stack took the two vector allocations
  and the reification of every activation nothing captures. Measured against the
  arena in one sitting, a user-method send fell from ~146 ns to ~91 ns, and
  `bench_send.ns` from 23.5 ms to 20.5 ms. Peak working set
  over `bench`'s twenty-two entry sends of `bench_loop.ns` fell from 340.7 MB to
  4.6 MB.
- **A live activation and a reified frame are two handle types**, `ActivationReference`
  and `FrameReference`, so that storing a stack handle is a compile error and a
  register access never branches on which kind it holds.
- **A returned activation has no sender**, as in Smalltalk, Cog and Primordial
  Soup. A live one's sender is the entry below it, by construction. The `sender`
  parameters that once threaded through every send and block invocation were
  removed rather than retyped, because once the sender is structural nothing
  read them.
- **Blocks reach their method's variables by reference until closure conversion
  lands**, and a reified frame keeps its registers when its activation returns
  to allow it. The reification mechanism itself is permanent: `^` and the
  debugger need it with or without conversion.
- **Sends run on the activation stack, not the Rust stack**: one loop pushes,
  pops and resumes, and it pops everything it runs on every path out.
- **The control-flow natives are trampoline steps and depth has one bound**,
  L = 100,000 activations, counted alike under both lowerings, because a
  callback native would make the reference lowering's native stack grow with
  source nesting; with no re-entry from Rust left, nothing else can run out
  first ("The depth limit").
- **Rust does not re-enter the run loop** (decided 2026-09-13, "No re-entry
  from Rust"): `do:`, instantiation and the nested-class accessor are
  trampoline steps, so that no Rust frame is live below a safepoint and the
  root discipline has nothing to root but the host's values. The bound on
  re-entries went with them.
- **The run loop's per-instruction step is forced inline** (2026-09-16,
  confirmed by the user 2026-09-17), since
  the inline caches' hit path makes it too large for the inliner to take on
  its own, and a step out of line costs every instruction a call; forcing it
  alone measured no faster ("What the inline caches moved"). Planned for
  later: one `Interpreter::enter` per stretch between two safepoints rather
  than one per instruction, so that no per-instruction boundary exists
  ([dispatch.md](dispatch.md), "Decisions").
- **The run loop polls a safepoint at every switch and every backward jump**
  ("The VM"): one compare of a flag the heap owns and a cold call when it is
  set, at the points where the loop's locals hold no handle a move would
  change ([memory.md](memory.md), "Where a collection can happen"). The slow
  path serves the request by clearing the flag, which is also what keeps the
  optimizer from deleting the poll.
- **A nested class's creation is marked in its own cache slot** ("No re-entry
  from Rust"), not listed beside the stack: the slot is keyed by exactly the
  pair a cyclic definition is, so the check is the read the accessor makes
  anyway, where the list was scanned in full at every level of a deep chain of
  creations.
