# Execution — activations, the VM

> **Status:** built, including the trampoline; nothing re-enters the run loop
> from Rust, the loop polls a safepoint, each instruction it executes is
> one entry of the step state, the sends it executes go through inline
> caches, except the reference lowering's, and the optimized lowering converts
> closures, so only by-reference code keeps a returning frame's registers.
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> arena, the `ActivationReference` and `FrameReference` handles, every read and write of an
> activation, the copied values a closure carries and whether a frame keeps its
> registers), `heap/world.rs` (the step state at rest, and its entry),
> `types.rs` (`Frame`, `ClosureData`, `Continuation` and the state its variants
> carry), `vm.rs` (the run loop and its safepoint poll), `interpreter/` (the
> inline caches, the send decisions, `Step`, every push, the depth limit, the host entry, the
> safepoint's slow path, and the indirection vector's reads and writes in
> `interpreter/locals.rs`), `compiler/capture.rs` (the capture analysis) and
> `compiler/lower.rs` (the converted register layout),
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
recorded in the `.toml` beside it (`crates/ns-cli/tests/run_goldens.rs`), and
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

One difference decided the shape of this design, and closure conversion has
since closed most of it. Cog's and Primordial Soup's closures **copy** the
values they capture and box the variables they write
(`pushClosureNumArgs:copiedValues:` and `pushRemoteTemp` in Cog; `PushClosure`
and `PushIndirectLocal` in Primordial Soup), so nothing ever reads a dead
frame's temporaries and a widowed context needs none of them. The optimized
lowering now does the same ("Closure conversion" below): a captured variable is
copied into the closure, or lives in its scope's indirection vector, which the
declaring body and every block that needs it share. A counter block returned
from the method that declares `n` still goes on writing `n`
(`goldens/language/closure_variables.ns` pins this) — through the vector rather
than through the frame.

Two things still read an enclosing activation's registers **by reference**,
through `captured_frame`: the reference lowering, which keeps the by-reference
model as the oracle the conversion is checked against, and the fallback closure
an inlined `ifTrue:` sends when its receiver is not a Boolean ("Fallback
blocks" below). So a reified frame keeps its registers when its activation
returns only when something can still reach them that way, decided per
activation from the block's own `outer_reach` ("Pop" below).

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
  activation handle anything *stores*: `ClosureData.captured_frame`,
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
`entry.reified`; every later call answers the same handle. Its one caller is
closure creation, the VM's `MakeClosure`, for the new closure's
`captured_frame`. A closure stores no `home` of its own: `Heap::closure_home`
derives it from the captured frame — that frame itself when it is a method's or
an initializer's, that frame's own `home` when it is a block's ("The closure
keeps its defining activation"). Every block creation reifies the activation
that creates it, whether or not the block reads one of its variables, because
`captured_frame` is also where `Mutator::push_step` reads the block's receiver
and current class from.

**Pop.** The run loop pops every activation it runs, on *every* exit, error
paths included (`vm::run`), and only the top entry can be popped. Popping hands
back the entry's continuation (`Popped`), which the loop applies to the
activation's value ("The VM" below). If the entry was reified, its frame is
**divorced**: its `program_counter` takes the entry's last stored one, `live`
becomes `None`, `uncontinuable` becomes true and `sender` becomes `None`. Then
`values` is truncated to the entry's `base`.

**The registers are copied into the frame's `arguments`/`temporaries` only when
the entry asks for it**, `StackEntry.retains_registers`, which
`Heap::retain_registers` sets and which `MakeClosure` sets when the block it
creates can reach this frame by reference — `block.outer_reach > 0`
("Converted representation"). Under the reference lowering that is every
activation that creates a closure, so every frame is divorced with its
registers as it always was; under the optimized lowering it is only an
activation whose fallback path actually created a closure. A frame divorced
without its registers answers `nil` for a register read and ignores a write
(`Heap::frame_local`/`frame_set_local`), which is what a crafted image's
by-reference closure over a register-less frame sees
([image-format.md](image-format.md), "Verification"). An entry that was never
reified leaves nothing behind, so storage for activations nothing captured is
bounded by the deepest the program has been rather than by every activation it
has ever made.

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
  other continuation — a native loop's, `do:`'s, the nested-class
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
   `ClosureData`. `value`, the control-flow steps — `whileTrue:`'s and
   `whileFalse:`'s leaves and `True`'s and `False`'s `^<parameter> value`
   quick methods — and `do:` answer the block as a step.
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
It is not a field: `Heap::closure_home` derives it from the closure's
`captured_frame` ("The closure keeps its defining activation").
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
the body `[n < 50000] whileTrue: [<first statement>. n:: n + 1]` -- isolate
the pieces, because each differs from the next by exactly one send. Row
one's `sum:: sum + 1` reads a literal where `bench_loop.ns` itself reads
the variable `n` (`sum:: sum + n`), so its 19.02 ms is a separate
measurement that happens to land close to `bench_loop.ns`'s own ~19.6 ms
at the time, not the same run reported twice:

| First statement | Method it calls | bytecode median | ns/iteration |
|---|---|---|---|
| `sum:: sum + 1` | — | 19.02 ms | 380.5 |
| `sum:: self tick: sum` | `public tick: x = (^x)` | 22.44 ms | 448.7 |
| `sum:: self tick: sum` | `public tick: x = (^x + 1)` | 27.28 ms | 545.7 |

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
| row one, `sum:: sum + 1` | 16.16, 16.18, 16.24 ms | 15.97, 15.98, 16.26 ms | 323.5 → 319.5 |
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
("Closure conversion") and a collector ([memory.md](memory.md)) are
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
what "Closure conversion" and a collector ([memory.md](memory.md))
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
  `whileTrue:` loop over `#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum:: sum + e]`.
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
start pinned, that phase shift is gone — which is not the same as a
comparison measuring code rather than placement; the paragraph below says
what the pin does not cover. Why the shipped
build does not carry it: the pin costs about 110 KB of padding, 5.6% of the
release binary, and measured no faster — the unpinned build was level or up
to 1.8% faster than the pinned one — so `.cargo/config.toml` carries only
`+crt-static`, and a build from the configuration alone is the one that
ships, not the one that is compared. Before timing, check that no binary
imports `VCRUNTIME140.dll` (`dumpbin /dependents`) and that each prints the
expected output for every source; a linker map (`-C link-arg=/MAP:<file>`)
shows every function of the crate at a multiple of 64 — the only symbols off
the boundary are the static C runtime's own objects.

The pin removes the 64-byte phase shift and nothing else. `bench_block.ns` has
since shown a **16% spread between builds of the same tree** with the pin in
place, stable to about 1% within each build, so on that source one build per
commit measures the build rather than the commit; pool several builds of each
tree instead ("What `Top`, `Value` and `Number` cost" below, which found it).
The other seven sources stay within 0.3–2.3% between builds. Pooling several
plain builds varies nothing when the builds are byte-identical, as they are
on the machine these measurements ran on ("What the value conditions cost"
below): build the extra ones with `-C metadata=<word>` added to `RUSTFLAGS`.
That varies code layout only. The runtime's data layout (which symbol ids,
arena indices and dispatch ids a program's objects receive) is not varied by
it, so a difference that follows from data layout survives every such pool.

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
  release `Object` stays 56 bytes: the inline `ClosureData` was 24 bytes when
  this was measured and is 32 since closure conversion, within the payload's 48
  either way.
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
([memory.md](memory.md), "The move stress mode").

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
where 2b left 12.7 s and 190.2 s ([memory.md](memory.md), "The move stress
mode").

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
`target\bench-lookup\` (ignored by git), and after them `bench_do.ns`, first
written out in the
[2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol". All four are spelled with the setter send, which every
compared binary back to `be3eb40` parses; the 2c plan's copy spells `:=`,
which the tree has refused since "M4 — Removing `:=`" ([PROGRESS.md](../PROGRESS.md)),
and is the same program. The three print `20000`:

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
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new v. n:: n + 1].
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
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new v. n:: n + 1].
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
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new u. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`bench_do.ns` prints `275000`:

```
Newspeak3
'Bench'
class BenchDo usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 5000] whileTrue: [#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum:: sum + e]. n:: n + 1].
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
Build Tools, which is installed but not on `PATH` — on this machine, found
through `vswhere.exe` in `C:\Program Files (x86)\Microsoft Visual Studio\Installer\`,
at `C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe`.
From Git Bash it must be invoked as `dumpbin -disasm`, not `/disasm`: MSYS
rewrites a `/option` argument into a Windows path, and the tool then
silently tries to dump a file called `C:\Program Files\Git\disasm`.
`/dependents` on every binary, and `/disasm` (or `-disasm`, from Git Bash) on
a release copy built with debug information to see which functions the
optimizer kept whole. That copy is built with the same
`RUSTFLAGS` as the timed binary, plus `CARGO_PROFILE_RELEASE_DEBUG=true` and
a separate `--target-dir` so it does not disturb the timed build:
`CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p ns-cli
--target-dir target-debuginfo`. "Entirely below" and "entirely above" are the
protocol's gate: all five after-medians below, or above, all five of the
group compared with.

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

### What 3c moved

3c moved the dispatch id a guard reads from the class object into the object
header ([dispatch.md](dispatch.md), "3c — The dispatch id in the object
header"). Two variants were measured on 2026-09-18 against `393311f`, the
last commit before 3c that changed a send's path, and against `be3eb40`:
**C2** (`681126d`), the id in the header, and **C2+** (`5d3aeff`), the id in
the header with `ClassData` boxed out of `ObjectData`, which is what was
kept. `fbd0bef`, the commit the step started from, ran beside them. The
protocol is the one above: every binary, the current tree included, built
with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the earlier commits in throwaway worktrees; no binary imports
`VCRUNTIME140.dll`; every binary checked to print the expected answer for all
eight sources first, `be3eb40` included; `ns-next bench <source>
--iterations 30`, five rounds, the order of the five binaries rotating from
round to round; two whole sittings, nothing else running.

**C2+ is level with `393311f` on every source.** Change of the mean,
sitting 1 / sitting 2, with the protocol's gate; the rounds are sitting 1's
medians in ms:

| Source | `393311f`, rounds 1–5 | mean | C2+, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.59, 10.44, 10.58, 10.55, 10.66 | 10.563 | 10.57, 10.47, 10.34, 10.33, 10.35 | 10.413 | −1.41% / −2.00% | overlapping in both |
| `bench_send.ns` | 11.58, 11.57, 11.58, 11.76, 11.72 | 11.641 | 11.51, 11.61, 11.56, 11.50, 11.59 | 11.554 | −0.75% / −2.78% | entirely below in the second |
| `bench_block.ns` | 20.32, 20.16, 20.20, 20.25, 20.45 | 20.276 | 20.24, 20.39, 20.29, 20.25, 20.29 | 20.291 | +0.07% / +0.15% | overlapping in both |
| `bench_do.ns` | 7.01, 7.04, 7.09, 7.31, 7.10 | 7.110 | 6.99, 7.15, 6.99, 7.04, 7.13 | 7.060 | −0.70% / −1.94% | overlapping in both |
| `bench_new.ns` | 6.30, 6.21, 6.14, 6.22, 6.29 | 6.234 | 6.16, 6.16, 6.15, 6.16, 6.21 | 6.167 | −1.07% / −0.85% | overlapping in both |
| `bench_new2.ns` | 7.17, 7.28, 7.17, 7.15, 7.26 | 7.207 | 7.19, 7.13, 7.26, 7.21, 7.12 | 7.181 | −0.36% / −0.97% | overlapping in both |
| `bench_newarg.ns` | 9.20, 9.12, 9.13, 9.16, 9.20 | 9.161 | 9.10, 9.09, 9.19, 9.20, 9.06 | 9.127 | −0.37% / −2.09% | overlapping in both |
| `bench_megamorphic.ns` | 16.94, 17.25, 17.26, 17.35, 17.12 | 17.183 | 16.67, 16.66, 16.75, 16.68, 16.94 | 16.740 | −2.58% / −1.62% | entirely below in the first |

The means moved down in 14 of the 16 cells, by 0.4–2.8%, but **no source is
entirely below `393311f` in both sittings**, which is what the protocol
accepts as a real change; the two single-sitting results do not repeat in
the other sitting. C2 is level too: −0.25% to −2.44% in 15 of 16 cells and
+0.30% on `bench_do.ns`'s first sitting, entirely below only on
`bench_megamorphic.ns` in the first sitting. `fbd0bef` is level with
`393311f` everywhere, −1.29% to +1.62%, every group overlapping.

Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `393311f` | `fbd0bef` | C2 | C2+ |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.461 / 11.436 | 10.563 / 10.575 | 10.685 / 10.561 | 10.488 / 10.428 | 10.413 / 10.363 |
| `bench_send.ns` | 14.215 / 14.167 | 11.641 / 11.811 | 11.786 / 11.756 | 11.612 / 11.588 | 11.554 / 11.483 |
| `bench_block.ns` | 23.915 / 23.901 | 20.276 / 20.221 | 20.450 / 20.549 | 20.020 / 19.970 | 20.291 / 20.252 |
| `bench_do.ns` | 8.241 / 8.263 | 7.110 / 7.185 | 7.150 / 7.176 | 7.131 / 7.009 | 7.060 / 7.046 |
| `bench_new.ns` | 7.651 / 7.653 | 6.234 / 6.252 | 6.247 / 6.231 | 6.193 / 6.133 | 6.167 / 6.199 |
| `bench_new2.ns` | 8.620 / 8.542 | 7.207 / 7.221 | 7.223 / 7.289 | 7.161 / 7.149 | 7.181 / 7.151 |
| `bench_newarg.ns` | 10.932 / 10.885 | 9.161 / 9.243 | 9.209 / 9.175 | 9.061 / 9.071 | 9.127 / 9.049 |
| `bench_megamorphic.ns` | 18.408 / 18.292 | 17.183 / 17.059 | 16.962 / 17.031 | 16.794 / 16.775 | 16.740 / 16.783 |

**Every source stays well below `be3eb40`**, every group entirely below in
both sittings. C2+: `bench_loop.ns` −9.1% / −9.4%, `bench_send.ns` −18.7% /
−19.0%, `bench_block.ns` −15.2% / −15.3%, `bench_do.ns` −14.3% / −14.7%,
`bench_new.ns` −19.4% / −19.0%, `bench_new2.ns` −16.7% / −16.3%,
`bench_newarg.ns` −16.5% / −16.9%, `bench_megamorphic.ns` −9.1% / −8.3%.

**`vm::run`'s step is still inline.** `dumpbin /disasm` on release builds
with debug information finds no `vm::run::closure` function in `393311f`, C2
or C2+: `vm::run` is one function of 4,926 instructions at `393311f` (the
count "What the step before 3c moved" recorded for the same commit), 4,863
at C2 and 4,878 at C2+.

**Sizes, and what they cost.** `size_of::<Object>()`, the amplification a
crafted image can reach at the graph section ([memory.md](memory.md);
record minima 6 and 18 bytes, a frame-arena entry 136 bytes in release and
160 in debug, unchanged by 3c), and the peak working set (`PeakWorkingSet64`):

| Binary | `Object`, release / debug | Amplification, release / debug | Peak working set, `run-corpus` | Peak working set, `bench_block.ns --iterations 30` |
|---|---|---|---|---|
| `393311f` | 56 B / 96 B | 16.9x / 24.9x | 4.598 MB | 454.699 MB |
| C2 | 64 B / 104 B | 18.2x / 26.2x | 4.598 MB | 466.559 MB |
| C2+ | 56 B / 80 B | 16.9x / 22.2x | 4.590 MB | 454.727 MB |

C2 costs `bench_block.ns`, which allocates heavily, 11.9 MB (2.6%) at its
peak, its 8 bytes an object; C2+ is within 0.03 MB of `393311f`. Boxing
`ClassData` gave back exactly what the header's id took in release, and more
than that in debug, where every handle `ClassData` held also carries its
heap's id and a move epoch. `run-corpus` is too small to show either.

**What the protocol cannot see.** The interpreter's guard is one step among
the run loop's dispatch, decoding and bookkeeping, and a load more or less
before the compare is lost in them. The difference the header makes is to a
compiled guard: one compare against the receiver at a fixed offset, against
two dependent loads, a check that the class is a class object and a branch
for the class side — which is why the header was kept on a measurement that
shows it level ([dispatch.md](dispatch.md), "3c — The dispatch id in the
object header").

### What the specification gaps moved

The specification gaps ([PROGRESS.md](../PROGRESS.md), "M4 — The
specification gaps") touch a send's compiled path in two places: a
parameter's or an immutable local's register is never copied any more
(Task 4, `ff1657d`, riding on Task 3's resolver guarantee), and a symbol
literal now names the heap's one canonical object for its text instead of a
fresh `Symbol` per occurrence (Tasks 5–6, `7135854`, `67ba5c0`, `abd9bb4`).
The other two gaps — the §5.7 member set keyed by a declaration instead of a
class's simple name (Task 1, `df0f347`) and passing a setter send by a
parameter or an immutable local (Task 3, `73921e1`), which the local-slot
declaration fix (Task 2, `717300a`) sits beside — are resolver changes,
ahead of the run loop, and change nothing a send executes.

Measured 2026-09-19: the branch head `abd9bb4` against `22a56f7`, the merge
of 3c, with `be3eb40` carried as the reference. Three throwaway detached
worktrees, one each for `22a56f7`, `be3eb40` and `abd9bb4`; every binary
built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
none imports `VCRUNTIME140.dll` (`dumpbin /dependents`); every binary checked
to print the expected answer for all eight sources first, exit 0 everywhere.
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the three binaries rotating by one position from round to round; two
whole sittings, nothing else running. None of the eight sources declares a
local slot with an initializer or writes a parameter, so Tasks 2–3 change
nothing any of them does and all eight stay comparable with earlier
measurements.

**The head is level with `22a56f7` on every source.** Sitting 1's medians in
ms, both means, the change of the head's mean in sitting 1 / sitting 2, and
the gate in sitting 1 / sitting 2:

| Source | `22a56f7`, rounds 1–5 | mean | head, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.59, 10.39, 10.39, 10.16, 10.14 | 10.336 | 10.43, 10.19, 10.37, 10.41, 10.32 | 10.342 | +0.06% / +1.43% | overlapping in both |
| `bench_send.ns` | 11.48, 11.46, 11.52, 11.32, 11.45 | 11.447 | 11.54, 11.55, 11.60, 11.59, 11.62 | 11.578 | +1.15% / +0.51% | entirely above in the first, overlapping in the second |
| `bench_block.ns` | 19.80, 20.28, 19.95, 20.59, 20.11 | 20.146 | 19.60, 20.24, 20.27, 20.19, 20.42 | 20.146 | +0.00% / +0.21% | overlapping in both |
| `bench_do.ns` | 6.92, 7.07, 6.95, 6.99, 7.17 | 7.020 | 7.02, 6.96, 6.91, 7.03, 7.02 | 6.988 | −0.45% / +1.10% | overlapping in both |
| `bench_new.ns` | 6.15, 6.13, 6.13, 6.24, 6.26 | 6.183 | 6.17, 6.26, 6.27, 6.20, 6.18 | 6.218 | +0.58% / −0.88% | overlapping in both |
| `bench_new2.ns` | 7.17, 7.10, 7.22, 7.27, 7.09 | 7.170 | 7.19, 7.17, 7.18, 7.16, 7.19 | 7.179 | +0.13% / +0.86% | overlapping in both |
| `bench_newarg.ns` | 9.11, 9.11, 9.09, 9.15, 9.12 | 9.116 | 9.07, 8.88, 9.16, 9.10, 9.06 | 9.051 | −0.71% / +0.52% | overlapping in both |
| `bench_megamorphic.ns` | 16.66, 16.69, 16.70, 16.91, 16.74 | 16.742 | 16.66, 16.87, 16.79, 16.76, 16.84 | 16.783 | +0.25% / +0.13% | overlapping in both |

No source is entirely above `22a56f7` in both sittings — the protocol's rule
for a real regression — and none is entirely below in either. The one
single-sitting result, `bench_send.ns` entirely above in sitting 1 (+1.15%),
does not repeat in sitting 2 (+0.51%, overlapping). The means moved by
−0.88% to +1.43%, ten of the sixteen cells up, all within the spread the
protocol treats as level.

Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `22a56f7` | head `abd9bb4` |
|---|---|---|---|
| `bench_loop.ns` | 11.290 / 11.252 | 10.336 / 10.250 | 10.342 / 10.397 |
| `bench_send.ns` | 14.090 / 13.986 | 11.447 / 11.491 | 11.578 / 11.550 |
| `bench_block.ns` | 23.743 / 23.569 | 20.146 / 19.823 | 20.146 / 19.865 |
| `bench_do.ns` | 8.132 / 8.057 | 7.020 / 6.932 | 6.988 / 7.008 |
| `bench_new.ns` | 7.556 / 7.444 | 6.183 / 6.146 | 6.218 / 6.092 |
| `bench_new2.ns` | 8.507 / 8.372 | 7.170 / 7.053 | 7.179 / 7.114 |
| `bench_newarg.ns` | 10.751 / 10.582 | 9.116 / 8.980 | 9.051 / 9.026 |
| `bench_megamorphic.ns` | 18.296 / 18.077 | 16.742 / 16.554 | 16.783 / 16.576 |

**Every source stays well below `be3eb40`**, every group entirely below in
both sittings, for the head and for `22a56f7`:

| Source | head vs `be3eb40` | `22a56f7` vs `be3eb40` |
|---|---|---|
| `bench_loop.ns` | −8.40% / −7.60% | −8.46% / −8.90% |
| `bench_send.ns` | −17.82% / −17.42% | −18.75% / −17.84% |
| `bench_block.ns` | −15.15% / −15.71% | −15.15% / −15.89% |
| `bench_do.ns` | −14.06% / −13.02% | −13.67% / −13.97% |
| `bench_new.ns` | −17.70% / −18.16% | −18.17% / −17.43% |
| `bench_new2.ns` | −15.61% / −15.03% | −15.72% / −15.76% |
| `bench_newarg.ns` | −15.81% / −14.71% | −15.20% / −15.15% |
| `bench_megamorphic.ns` | −8.27% / −8.30% | −8.49% / −8.43% |

**`vm::run`'s step is still inline.** Release builds with debug information
of `22a56f7` and `abd9bb4` (the same `RUSTFLAGS`, plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`, "What the
inline caches moved" above), then `dumpbin /disasm`: no `vm::run::closure`
function in either. `ns_runtime::vm::run` is one function of **4,878
instructions at `22a56f7`** (the count "What 3c moved" records for C2+,
`5d3aeff`) and **4,878 at `abd9bb4`**. With relocated addresses normalized
the two streams differ in 192 lines, all of them displacements: the head's
`rbx`-relative field offsets are 0x18 higher from `+0A8h` on — the heap's new
canonical-symbol-table field — and some absolute data addresses moved; no
instruction was added, removed or changed in kind.

**The copy elision measured no gain on these eight sources.** None of the
eight declares a local slot with an initializer or writes a parameter, so
Task 4's change never fires on any of them; the two constructs it touches are
exercised by the golden suite, not by the benchmarks. What the numbers show
is that the four gaps together cost nothing on a send's hit path, not that
the elision itself is worth anything measured here.

### What closure conversion moved

Closure conversion ([PROGRESS.md](../PROGRESS.md), "M4 — Closure conversion")
changes what every block creation and every block activation does, and it
narrows the protective register copies the lowering emits, so it was measured
in two steps against one baseline: the conversion on its own, then the copies
on top of it.

Measured 2026-09-20. Four binaries, each `cargo build --workspace --release`
with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the three earlier commits in throwaway worktrees:

- `efac7d2` — the `master` the branch started from (the merge of
  `feature/remove-assign`);
- `be3eb40` — the reference carried since step 2c;
- `a3f36a2` — the conversion alone (`feat: convert closures in the optimized
  lowering`);
- `fbd7d9f` — the head, the conversion plus the narrowed protective copies.

The protocol is the one of "What removing the re-entries moved": no binary
imports `VCRUNTIME140.dll` (`dumpbin /dependents`); every binary was checked to
print the expected answer for all eight sources before anything was timed;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the four
binaries' order rotating by one position from round to round, two whole
sittings, nothing else running — 320 timed data lines.

**Where the four out-of-tree sources came from.** `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` were copied from "What
spec-conformant lookup moved" above, not from the
[2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md)'s
measurement protocol, whose copies spell the retired `:=` and no longer parse —
and three of which also declare a slot and a getter of the same simple name,
which the resolver now refuses as a duplicate member (§5.7). That substitution
is what every measurement since the spec-conformant-lookup step has already
made; the next one should take all four from execution.md too.

**Level everywhere.** Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `efac7d2` | `a3f36a2` (conversion) | `fbd7d9f` (head) |
|---|---|---|---|---|
| `bench_loop.ns` | 11.185 / 11.271 | 10.257 / 10.216 | 10.336 / 10.392 | 10.475 / 10.323 |
| `bench_send.ns` | 13.913 / 13.924 | 11.410 / 11.510 | 11.459 / 11.503 | 11.489 / 11.470 |
| `bench_block.ns` | 23.405 / 23.432 | 19.848 / 19.863 | 19.727 / 19.792 | 19.933 / 19.785 |
| `bench_megamorphic.ns` | 17.945 / 17.967 | 16.538 / 16.501 | 16.655 / 16.583 | 16.670 / 16.555 |
| `bench_do.ns` | 8.094 / 8.114 | 6.970 / 7.007 | 7.016 / 6.855 | 6.949 / 7.030 |
| `bench_new.ns` | 7.493 / 7.509 | 6.115 / 6.124 | 6.185 / 6.179 | 6.178 / 6.117 |
| `bench_new2.ns` | 8.506 / 8.403 | 7.135 / 7.117 | 7.163 / 7.090 | 7.115 / 7.171 |
| `bench_newarg.ns` | 10.641 / 10.664 | 8.997 / 9.014 | 9.057 / 9.049 | 9.039 / 9.102 |

The gate is the protocol's: a source has moved only when its five-round range
(min to max of the five medians) lies entirely above, or entirely below, the
baseline's in **both** sittings.

- **The conversion against `efac7d2`**: −2.18% to +1.73%, every source
  overlapping in both sittings but one — `bench_do.ns`, entirely below in the
  second sitting only (−2.18%), which does not repeat in the first (+0.67%).
  No source is entirely above. The conversion is level with `efac7d2` on all
  eight.
- **The copies against the conversion**: −1.00% to +2.56%, every source
  overlapping in both sittings, `bench_block.ns` — the one source that
  exercises a write-after-capture block every iteration — included. Narrowing
  the protective copies moved nothing measurable.
- **The head against `efac7d2`**: −0.39% to +2.12%, every source overlapping in
  both sittings. **No regression.**
- **The head against `be3eb40`**: every source entirely below in both sittings,
  by **6–19%** — `bench_loop.ns` −6.4% / −8.4%, `bench_send.ns` −17.4% /
  −17.6%, `bench_block.ns` −14.8% / −15.6%, `bench_megamorphic.ns` −7.1% /
  −7.9%, `bench_do.ns` −14.2% / −13.4%, `bench_new.ns` −17.6% / −18.5%,
  `bench_new2.ns` −16.4% / −14.7%, `bench_newarg.ns` −15.1% / −14.7%. Every
  gain accumulated since step 2c is still there; the conversion gave none of it
  back.

**What the design expected and what was measured.** "Cost", under "Closure
conversion" below, predicted that a block would stop paying a hop per nesting
level and that an activation would stop copying its registers out. Both happen,
and neither shows in the running time of any of these eight sources. The
profile of the trampoline's head ("What the profiles show" above, the
`bench_block.ns` head column) is the reason to expect little: it put reifying a
frame at 12.5% of the timed samples and popping, the divorce included, at
13.1%, and the conversion removes neither the reification nor the divorce —
only the register copy inside the divorce, and only for an activation no
by-reference closure can read. What is recorded
here is that the conversion is free, not that it is faster.

**`bench_block.ns`'s peak working set rose about 2.6%.** Measured as
[memory.md](memory.md)'s "The interim cost, and the one question still open"
records it — Windows
11, release build, `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` through
`psapi!GetProcessMemoryInfo` on the still-open handle immediately after the
process exits, `bench bench_block.ns --iterations 30`, three runs each:

| Binary | Three-run band |
|---|---|
| `efac7d2` | 454.719–454.754 MB |
| `a3f36a2` (conversion only) | 466.406–467.598 MB |
| `fbd7d9f` (head) | 466.418–466.469 MB |

**The rise belongs to the conversion.** `a3f36a2`'s band lies entirely above
`efac7d2`'s, already at the head's level, and `fbd7d9f`'s band sits inside
`a3f36a2`'s, so the copy-narrowing adds and removes nothing here. The
explanation — a block that writes a captured local now allocates an
indirection-vector `Array` per activation of the body that declares it, in an
arena that reclaims nothing — **is an inference from what changed, not a
measured attribution**: no allocation profile was taken, and the numbers are
consistent with it without singling it out. It is a cost the collector (step 5)
removes; until then a vector is retained exactly as every other object the
process has ever made.

**`vm::run`'s step is still inline.** Release builds with debug information of
`efac7d2` and the head (the same `RUSTFLAGS`, plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin /disasm`: no `vm::run::closure` function in either.
`ns_runtime::vm::run` is one function of **4,878 instructions at `efac7d2`**
and **5,700 at the head**, +16.9% — the three vector instructions and the
two-mode block-creation logic, all still inlined into the one function rather
than split out. That is what the step's own size did; it cost none of the
eight sources any measurable time, and nothing here should be read as saying it
did.

### What the moving collector moved

The moving collector ([PROGRESS.md](../PROGRESS.md), "M4 — The moving
collector"; [memory.md](memory.md), "The shape: mark, compact, forward"
onward) marks from the roots through `trace.rs`'s one edge enumeration,
compacts by sliding each arena's live slots to the front, truncates and
shrinks the arena with hysteresis, and rewrites every handle through a flat
forwarding table; the canonical symbol table becomes weak; a trigger compares
an arena's length against a limit in `push_object` and `reify` and requests a
collection at the next safepoint; and `NS_NEXT_STRESS_COLLECT` collects at
every safepoint as a third gate. `bench --collector-statistics` reports the
counts and pauses the way `--cache-statistics` reports the caches. This is
what ends the arena's never-reclaim interim ([memory.md](memory.md), "The
interim cost, and the one question still open"), and it was measured against
the expectation recorded in [memory.md](memory.md), "How the collector is
measured", written before any of the numbers below existed.

**Measured 2026-09-20, against `dc0ffe5` (the merge of closure conversion
into `master`) and `be3eb40`, by the protocol of "What removing the
re-entries moved" above.** Every binary — `dc0ffe5` and `be3eb40` built in
throwaway worktrees, the head `014f0ff` in the current tree — built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
each checked to print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the three binaries rotating by one position from round to round, two
whole sittings, nothing else running. Two facts that cost real time and are
recorded here for the next measurement to skip past them: `dumpbin` lives at
`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe`
on this machine, found via `vswhere.exe` in
`C:\Program Files (x86)\Microsoft Visual Studio\Installer\`, and is not on
`PATH`; and from Git Bash it must be invoked as `dumpbin -disasm`, not
`/disasm` — MSYS rewrites a `/option` argument into a Windows path, and the
tool then silently tries to dump a file called `C:\Program Files\Git\disasm`.

**`vm::run`'s step is still one function.** A release build with debug
information, `dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`), no `vm::run::closure` of its own, at **5,732
instructions**, against 5,700 at closure conversion — the trigger's
comparison and the safepoint's now-real slow path add 32.

**Peak working set fell as the design predicted, and this is the point of
the step.** `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` through
`psapi!GetProcessMemoryInfo` on the still-open handle, release build, the
`run` and `bench --iterations 10` rows [memory.md](memory.md)'s table
already carried:

| Command | `dc0ffe5` | head |
|---|---|---|
| `run`, `bench_loop.ns` | 4.7 MB | 4.7 MB |
| `bench --iterations 10`, `bench_loop.ns` | 4.8 MB | 4.8 MB |
| `bench --iterations 10`, `bench_send.ns` | 4.8 MB | 4.8 MB |
| `bench --iterations 10`, `bench_block.ns` | **228.8 MB** | **5.6 MB** |

`bench_loop.ns` and `bench_send.ns`, which never collect (below), do not
move. `bench_block.ns` — the one source that allocates a closure and reifies
a frame every iteration, and so is the one source the arena's never-reclaim
interim showed most starkly — falls from 228.8 MB to 5.6 MB, level with the
other two. [memory.md](memory.md)'s own peak-working-set table, spanning
before the arena to after the collector, has the fourth column.

**Throughput is level on seven of the eight sources, entirely below
`be3eb40`, and one source is not level.** Medians in ms per round, then each
group's mean; sitting 1:

| Source | head | mean | `dc0ffe5` | mean | `be3eb40` | mean | head vs `dc0ffe5` | head vs `be3eb40` |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.55, 10.37, 10.53, 10.55, 10.32 | 10.46 | 10.37, 10.48, 10.34, 10.46, 10.45 | 10.42 | 11.35, 11.44, 11.48, 11.38, 11.53 | 11.43 | +0.4% | −8.5% |
| `bench_send.ns` | 11.58, 11.97, 11.59, 11.58, 11.58 | 11.66 | 11.65, 11.81, 11.62, 11.59, 11.66 | 11.67 | 14.15, 14.33, 14.36, 14.12, 14.01 | 14.19 | −0.1% | −17.9% |
| `bench_block.ns` | 21.28, 22.84, 22.62, 22.97, 22.66 | 22.47 | 20.39, 20.02, 20.24, 20.07, 20.21 | 20.19 | 23.70, 24.31, 23.80, 23.79, 23.87 | 23.89 | +11.3% | −5.9% |
| `bench_megamorphic.ns` | 16.78, 16.68, 16.98, 16.73, 16.65 | 16.76 | 16.77, 16.76, 17.05, 16.98, 16.92 | 16.90 | 18.43, 18.20, 18.42, 18.26, 18.29 | 18.32 | −0.8% | −8.5% |
| `bench_do.ns` | 7.07, 7.12, 6.98, 7.03, 6.96 | 7.03 | 6.98, 7.00, 7.12, 7.12, 7.11 | 7.07 | 8.30, 8.25, 8.22, 8.13, 8.37 | 8.25 | −0.5% | −14.8% |
| `bench_new.ns` | 6.30, 6.36, 6.17, 6.23, 6.24 | 6.26 | 6.34, 6.28, 6.39, 6.38, 6.35 | 6.35 | 7.62, 7.64, 7.66, 7.58, 7.58 | 7.62 | −1.3% | −17.8% |
| `bench_new2.ns` | 7.24, 7.41, 7.24, 7.27, 7.23 | 7.28 | 7.20, 7.17, 7.24, 7.26, 7.32 | 7.24 | 8.51, 8.63, 8.52, 8.47, 8.42 | 8.51 | +0.6% | −14.5% |
| `bench_newarg.ns` | 9.28, 9.42, 9.37, 9.49, 9.27 | 9.37 | 9.27, 9.01, 9.28, 9.08, 9.16 | 9.16 | 10.91, 10.95, 10.93, 10.81, 10.71 | 10.86 | +2.2% | −13.8% |

Sitting 2:

| Source | head | mean | `dc0ffe5` | mean | `be3eb40` | mean | head vs `dc0ffe5` | head vs `be3eb40` |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.38, 10.51, 10.48, 10.52, 10.33 | 10.44 | 10.61, 10.66, 10.62, 10.46, 10.42 | 10.55 | 11.44, 11.38, 11.47, 11.54, 11.37 | 11.44 | −1.0% | −8.7% |
| `bench_send.ns` | 11.72, 11.75, 11.58, 11.75, 11.58 | 11.68 | 11.67, 11.78, 11.54, 11.71, 11.67 | 11.67 | 14.25, 14.27, 14.11, 14.30, 14.34 | 14.26 | +0.0% | −18.1% |
| `bench_block.ns` | 22.89, 22.62, 22.60, 23.04, 23.20 | 22.87 | 20.24, 20.10, 20.29, 20.16, 20.39 | 20.24 | 23.93, 23.76, 23.74, 24.26, 24.15 | 23.97 | +13.0% | −4.6% |
| `bench_megamorphic.ns` | 16.96, 16.82, 17.04, 16.81, 16.80 | 16.89 | 16.77, 16.75, 17.03, 16.96, 16.98 | 16.90 | 18.42, 18.25, 18.43, 18.56, 18.43 | 18.42 | −0.1% | −8.3% |
| `bench_do.ns` | 7.11, 7.09, 7.15, 6.96, 7.00 | 7.06 | 7.01, 7.00, 7.13, 7.02, 7.11 | 7.06 | 8.10, 8.11, 8.17, 8.24, 8.21 | 8.17 | +0.1% | −13.5% |
| `bench_new.ns` | 6.19, 6.31, 6.21, 6.28, 6.25 | 6.25 | 6.38, 6.48, 6.41, 6.47, 6.43 | 6.44 | 7.51, 7.62, 7.61, 7.70, 7.67 | 7.62 | −2.9% | −18.0% |
| `bench_new2.ns` | 7.39, 7.33, 7.37, 7.28, 7.37 | 7.35 | 7.30, 7.21, 7.25, 7.41, 7.37 | 7.31 | 8.50, 8.53, 8.64, 8.67, 8.59 | 8.59 | +0.5% | −14.5% |
| `bench_newarg.ns` | 9.30, 9.39, 9.38, 9.49, 9.49 | 9.41 | 9.07, 9.10, 9.07, 9.06, 9.16 | 9.09 | 10.74, 10.92, 10.86, 10.86, 10.88 | 10.85 | +3.5% | −13.3% |

Both sittings pooled, by group mean, with the protocol's gate — a source has
moved only when its five-round range lies entirely above, or entirely below,
the group compared with, in **both** sittings:

| Source | head | `dc0ffe5` | `be3eb40` | vs `dc0ffe5` | vs `be3eb40` | groups overlap vs `dc0ffe5` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.45 | 10.49 | 11.44 | −0.3% | −8.6% | yes |
| `bench_send.ns` | 11.67 | 11.67 | 14.22 | −0.0% | −18.0% | yes |
| `bench_block.ns` | 22.67 | 20.21 | 23.93 | **+12.2%** | −5.3% | **no** |
| `bench_megamorphic.ns` | 16.82 | 16.90 | 18.37 | −0.4% | −8.4% | yes |
| `bench_do.ns` | 7.05 | 7.06 | 8.21 | −0.2% | −14.2% | yes |
| `bench_new.ns` | 6.25 | 6.39 | 7.62 | −2.1% | −17.9% | yes |
| `bench_new2.ns` | 7.31 | 7.27 | 8.55 | +0.6% | −14.5% | yes |
| `bench_newarg.ns` | 9.39 | 9.13 | 10.86 | +2.9% | −13.5% | yes |

**Seven of the eight sources are level with `dc0ffe5`** — `bench_loop.ns`
through `bench_newarg.ns` above, means spanning −2.1% to +2.9% — **and
5.3–18.0% below `be3eb40`.** Six of the seven overlap `dc0ffe5`'s group in
both sittings; `bench_newarg.ns`, the narrowest of the seven at +2.9%
pooled, is entirely above `dc0ffe5` in the second sitting and overlaps it by
only 0.01 ms in the first — and it is the source with the second-most
collections of the eight, 107. `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` are the four
out-of-tree sources written out in "What spec-conformant lookup moved"
above, copied from there rather than reconstructed, as that section
requires.

**`bench_block.ns` is not level, and the honest reading is that the step
cost it +12.2%.** Its pooled mean rose from 20.21 ms to 22.67 ms against
`dc0ffe5` — the one source whose pooled groups do not overlap in either
sitting, and the only one whose difference against `dc0ffe5` runs beyond a
couple of percent — while it stays 5.3% below `be3eb40`. It is the only one
of the eight sources that allocates a closure every iteration, and so the
only one that collects enough to show: `bench --collector-statistics` over
`bench --iterations 10` records

| Source | Collections | Total pause | Longest pause | Live objects | Live frames |
|---|---|---|---|---|---|
| `bench_block.ns` | 269 | 15.479 ms | 0.202 ms | 3,077 | 1,529 |
| `bench_loop.ns` | 0 | 0.000 ms | 0.000 ms | 49 | 0 |
| `bench_send.ns` | 0 | 0.000 ms | 0.000 ms | 49 | 0 |
| `bench_megamorphic.ns` | 0 | 0.000 ms | 0.000 ms | 175 | 0 |
| `bench_do.ns` | 26 | 1.341 ms | 0.088 ms | 4,062 | 1 |
| `bench_new.ns` | 53 | 2.662 ms | 0.070 ms | 4,037 | 0 |
| `bench_new2.ns` | 54 | 2.831 ms | 0.076 ms | 26 | 0 |
| `bench_newarg.ns` | 107 | 5.415 ms | 0.088 ms | 4,056 | 0 |

Pause accounts for roughly 1.4 ms of the 2.46 ms the head adds per round on
`bench_block.ns`; the remaining amount is not accounted for by pause and is
recorded as unexplained rather than guessed at.

Setting each source's pause per entry send against the change actually
measured separates what the collector itself costs from everything else it
moved. The pause is the total over eleven entry sends — one warm-up and ten
timed — and the change is the head's pooled median minus `dc0ffe5`'s:

| Source | Pause per send | Change per send | Not accounted for by pause |
|---|---|---|---|
| `bench_block.ns` | 1.407 ms | +2.460 ms | **+1.053 ms** |
| `bench_newarg.ns` | 0.492 ms | +0.260 ms | −0.232 ms |
| `bench_new2.ns` | 0.257 ms | +0.040 ms | −0.217 ms |
| `bench_new.ns` | 0.242 ms | −0.140 ms | −0.382 ms |
| `bench_do.ns` | 0.122 ms | −0.010 ms | −0.132 ms |

**The last column changes sign.** On the four sources other than
`bench_block.ns` the pause *exceeds* the measured change by 0.13 to 0.38 ms
per send: collecting gives more back there than it costs. Only on
`bench_block.ns` does anything cost beyond the pause. A sign that holds
across four sources is not noise, and it is the part of this measurement
most in need of explaining.

What plausibly gives back: an arena that never passes its limit never
doubles. Under `dc0ffe5` these sources grow both arenas to hold everything
ever allocated — `bench_new.ns` reaches roughly 220,000 objects over eleven
sends — and a growing `Vec` reallocates and copies periodically, in the
mutator, counted in no pause. The head's arenas stay at a few thousand slots.

One candidate this measurement rules out. Freeing the dead payloads is **not**
a mutator cost: `compact_arena`'s `truncate` drops them, inside
`Heap::compact`, inside the span `World::collect` times. Only the subsequent
allocation falls outside it, and allocating from a free list the collection
has just refilled is not obviously dearer than growing into fresh pages.

What fits the sign change better is the *number* of collections rather than
their total cost. `bench_block.ns` collects 269 times against
`bench_new.ns`'s 53 at a similar live object count, and every collection
walks the whole live graph, leaving the mutator's caches holding the
collector's working set instead of the program's. That after-effect scales
with how often a program collects rather than with how long each collection
takes, and it falls outside the measured pause. `bench_block.ns` is also the
only source with a populated frame arena — 1,529 live frames against 0 or 1
elsewhere — so it is the only one whose closures have their
`captured_frame` rewritten at every collection.

Both of those are hypotheses. Two cheap experiments discriminate between
them: a fixture whose blocks capture nothing, which keeps the object churn
and removes the frame reification; and varying `COLLECTION_FLOOR` so that the
same program collects less often but does more per collection, since a cost
that tracks the collection count rather than the pause total is the cache
after-effect rather than the work. `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` collect too — 26 to 107
times over ten iterations — at a fraction of the pause, and stay level with
`dc0ffe5` — so
collecting itself is not what shows on `bench_block.ns`; something about
collecting *269 times in ten iterations*, an order of magnitude more often,
is.

**`bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns` report zero
collections, and those are exactly the three sources whose throughput did
not move.** All three stay under the 4,096-slot floor ("The trigger and the
growth policy", [memory.md](memory.md)), so no allocation in them ever
requests a collection, and the send benchmarks do not feel the trigger's
added comparison in `push_object`. The recorded expectation named
`bench_loop.ns` and `bench_send.ns` by exactly this reasoning, in advance;
`bench_megamorphic.ns`, not named there, turns out to sit under the floor
too and shows the identical pattern — zero collections, no measurable move —
which is the expectation generalizing rather than a coincidence limited to
the two sources it named.

### What leaf inlining moved

The compiler now recognizes the nine binary integer primitives
(`+ - * // < > <= >= =`) at an ordinary explicit-receiver send and emits
`Instruction::IntegerBinary` instead of a send, with the send kept as the
fallback the instruction jumps to when either operand is not a
`Value::SmallInteger` or the `checked_*` operation declines
([compiler.md](compiler.md), "Arithmetic inlining"). This is the step
[core-library.md](core-library.md) sequences ahead of the move to a VM
mirror: once every builtin method is Newspeak over a mirror, a leaf send
that costs an activation is paid for on every arithmetic operation in the
library, so the fast path has to exist first.

**The expectation, recorded before any binary was built.** Sources whose hot
loop does integer arithmetic should improve, because an operation that was a
full send becomes one instruction plus a guard; sources that do none should
not move. Counting the eight sources first turned up the fact that **the
second group is empty**: every one of them counts its iterations with
`n < <limit>` and `n:: n + 1`, so every one gets at least two operations per
iteration out of the send path. The prediction was therefore a ranking by
how much non-arithmetic work surrounds those operations — large (>10%) for
`bench_loop.ns`, whose loop body is arithmetic and slot sends and nothing
else; moderate (4–10%) for `bench_send.ns`, `bench_megamorphic.ns` and
`bench_do.ns`; small (0–4%) for `bench_block.ns`, `bench_new.ns`,
`bench_new2.ns` and `bench_newarg.ns`, whose iterations were expected to be
dominated by a closure allocation or an instantiation. No source was
expected to get slower: the unconditional `Jump` past the fallback that
every *successful* operation also executes makes two dispatches out of one
send, which should show as a smaller gain than the send count suggests, not
as a regression.

**Measured 2026-09-21, against `6f5a5b1`, the commit on `master` the branch
was cut from**, by the protocol of "What removing the re-entries moved"
above: both binaries built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the baseline in a throwaway worktree, neither importing `VCRUNTIME140.dll`
and both checked to print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the two binaries alternating from round to round, two whole sittings,
nothing else running.

**`vm::run`'s step is still one function.** A release build with debug
information, `dumpbin /disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each binary, no `vm::run::closure` of its own.
The baseline measures **5,732 instructions**, the same figure recorded at
the moving collector, and the head **5,866** — the `IntegerBinary` arm adds
134, and the step did not split.

Medians in ms per round, then each group's mean; sitting 1:

| Source | Before (`6f5a5b1`), rounds 1–5 | mean | After, rounds 1–5 | mean | Change |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.29, 10.49, 10.49, 10.36, 10.52 | 10.43 | 7.17, 7.19, 7.42, 7.17, 7.21 | 7.23 | **−30.6%**, entirely below |
| `bench_send.ns` | 11.54, 11.60, 11.76, 11.71, 11.72 | 11.67 | 8.69, 8.58, 8.59, 8.61, 8.77 | 8.65 | **−25.9%**, entirely below |
| `bench_block.ns` | 20.18, 20.60, 20.35, 20.65, 20.59 | 20.48 | 17.96, 18.00, 18.20, 17.85, 17.82 | 17.97 | **−12.3%**, entirely below |
| `bench_megamorphic.ns` | 16.70, 16.81, 16.77, 16.80, 17.06 | 16.83 | 11.77, 11.67, 11.64, 11.75, 11.95 | 11.76 | **−30.1%**, entirely below |
| `bench_do.ns` | 7.09, 7.12, 7.05, 6.98, 7.06 | 7.06 | 5.82, 5.77, 5.84, 5.84, 5.78 | 5.81 | **−17.7%**, entirely below |
| `bench_new.ns` | 6.24, 6.27, 6.30, 6.21, 6.34 | 6.27 | 5.08, 5.00, 4.97, 4.96, 4.97 | 5.00 | **−20.3%**, entirely below |
| `bench_new2.ns` | 7.28, 7.33, 7.39, 7.34, 7.28 | 7.32 | 5.99, 6.05, 6.08, 6.08, 6.05 | 6.05 | **−17.4%**, entirely below |
| `bench_newarg.ns` | 9.39, 10.03, 9.35, 9.59, 9.42 | 9.56 | 8.06, 8.15, 8.13, 8.20, 8.07 | 8.12 | **−15.0%**, entirely below |

Sitting 2:

| Source | Before (`6f5a5b1`), rounds 1–5 | mean | After, rounds 1–5 | mean | Change |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.52, 10.59, 10.39, 10.48, 10.51 | 10.50 | 7.15, 7.30, 7.14, 7.15, 7.27 | 7.20 | **−31.4%**, entirely below |
| `bench_send.ns` | 11.58, 11.63, 11.78, 11.58, 11.83 | 11.68 | 8.57, 8.56, 8.50, 8.58, 8.57 | 8.55 | **−26.8%**, entirely below |
| `bench_block.ns` | 20.39, 20.65, 20.64, 20.33, 20.57 | 20.52 | 18.27, 18.30, 18.34, 18.36, 18.34 | 18.32 | **−10.7%**, entirely below |
| `bench_megamorphic.ns` | 16.72, 16.78, 17.05, 16.77, 16.98 | 16.86 | 11.66, 11.81, 11.95, 11.72, 11.91 | 11.81 | **−30.0%**, entirely below |
| `bench_do.ns` | 7.05, 7.16, 7.21, 7.14, 7.05 | 7.12 | 5.75, 5.89, 5.95, 5.70, 5.89 | 5.84 | **−18.0%**, entirely below |
| `bench_new.ns` | 6.20, 6.31, 6.19, 6.26, 6.30 | 6.25 | 4.99, 5.00, 5.01, 5.00, 5.02 | 5.01 | **−19.9%**, entirely below |
| `bench_new2.ns` | 7.30, 7.33, 7.35, 7.32, 7.34 | 7.33 | 6.11, 6.08, 6.12, 6.14, 6.18 | 6.12 | **−16.4%**, entirely below |
| `bench_newarg.ns` | 9.23, 9.30, 9.25, 9.24, 9.40 | 9.28 | 8.01, 8.11, 8.01, 8.07, 8.14 | 8.07 | **−13.1%**, entirely below |

**Every one of the eight sources is faster, by 11.5% to 31.0%, and every
after-group lies entirely below its before-group in both sittings** — the
protocol's gate, met by all eight. The only earlier step to clear it across
its whole suite was 3b, the inline caches, at 4.3–11.3% over seven sources
("What the inline caches moved" above). Both sittings pooled:

| Source | Before mean | After mean | Change |
|---|---|---|---|
| `bench_loop.ns` | 10.46 | 7.22 | **−31.0%** |
| `bench_megamorphic.ns` | 16.84 | 11.78 | **−30.0%** |
| `bench_send.ns` | 11.67 | 8.60 | **−26.3%** |
| `bench_new.ns` | 6.26 | 5.00 | **−20.1%** |
| `bench_do.ns` | 7.09 | 5.82 | **−17.8%** |
| `bench_new2.ns` | 7.33 | 6.09 | **−16.9%** |
| `bench_newarg.ns` | 9.42 | 8.09 | **−14.1%** |
| `bench_block.ns` | 20.50 | 18.14 | **−11.5%** |

**The expectation's ends were right and its middle was not, and every
magnitude was under-predicted by three to five times.** `bench_loop.ns` did
gain most and `bench_block.ns` least, as named in advance. But the whole
"small" group — `bench_block.ns`, `bench_new.ns`, `bench_new2.ns` and
`bench_newarg.ns` — landed between 11.5% and 20.1%, well inside and above
the band predicted for the "moderate" group, and `bench_new.ns` at −20.1%
came out ahead of `bench_do.ns` at −17.8%, which had been predicted
moderate. `bench_megamorphic.ns` came second overall rather than in the
middle of its group. The prediction's error was in its premise, not its arithmetic:
it assumed an instantiation or a block activation dwarfs three arithmetic
sends, and the aggregate timing deltas read as three arithmetic sends being
of the same order as an instantiation -- an inference from those deltas, not
something isolated; no profile was taken of this step.

**What the ordering does track is the share of a source's per-iteration
sends the step removed, discounted by how expensive the sends left behind
are.** Counting the send instructions `ns-next disasm` prints in each
source's loop, on the path an iteration actually takes:

| Source | sends per iteration before | after | share removed | measured |
|---|---|---|---|---|
| `bench_loop.ns` | 9 | 6 | 33% | −31.0% |
| `bench_megamorphic.ns` | 12 | 7 | 42% | −30.0% |
| `bench_send.ns` | 9 | 6 | 33% | −26.3% |
| `bench_new.ns` | 8 | 5 | 38% | −20.1% |
| `bench_do.ns` | 33 | 21 | 36% | −17.8% |
| `bench_new2.ns` | 8 | 5 | 38% | −16.9% |
| `bench_newarg.ns` | 8 | 5 | 38% | −14.1% |
| `bench_block.ns` | 10 | 7 | 30% | −11.5% |

`bench_loop.ns` is the clean case: its nine sends are six slot sends and
three arithmetic leaves, all of the same order of cost, and removing a third
of them takes off 31%. Wherever the remaining sends are dearer than a leaf
the gain falls away from the share — `bench_do.ns` keeps 50,000 block
activations through `do:`, the `bench_new*` sources an instantiation and a
getter method each, and `bench_block.ns`, the extreme, keeps a method
activation, a `MakeVector`, a `MakeClosure` and a block activation per
iteration for 30% of its sends removed and 11.5% of its time. That reading
is an inference from the instruction counts and the timings; no profile was
taken of this step.

**The `Jump` past the fallback does not show, and the data cannot bound it
better than this.** Every successful operation executes `IntegerBinary` and
then an unconditional `Jump` over the fallback send, so the hot path is two
dispatches where an out-of-line fallback would leave one
([compiler.md](compiler.md), "Arithmetic inlining"). `bench_loop.ns` removed
33% of its sends and gained 31.0%, which is the tightest the suite gets: the
2.3 points between them are everything the extra dispatches plus the guard
plus any second-order effect could account for, and they are not separable
from the difference between a slot send's cost and a leaf send's. Placing
the fallback out of line is therefore a follow-up worth trying and not one
this measurement makes a case for.

### What the core library's move cost

Every builtin method now lives in `crates/ns-runtime/core-library/Kernel.ns` as a
forwarder over the one VM mirror ([core-library.md](core-library.md),
"The core library as Newspeak source over a VM mirror"). A kernel
method whose whole body forwards to one mirror row is a primitive method: a
send of it runs the row's leaf in `act`, without an activation, and
activates the body only when the leaf declines
([dispatch.md](dispatch.md), "Primitive methods").
`NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without them, for measuring only, so the same binary shows what they
recover.

**The expectation, recorded before any binary was built.** With primitive
methods on, against `e2bb31e`: level on every source. A leaf send that `IntegerBinary` does not absorb costs one
cached send plus one field test on the method, where it cost one cached
send; the rest of the path is the same primitive. The one new cost on every
method send is that field test; if a source that sends no leaf moves, that is
where to look. Under `NS_NEXT_NO_PRIMITIVE_METHODS=1`, against `e2bb31e`:
slower on every source that sends a leaf the fast path does not absorb, since
each such send becomes an activation of the kernel method, a lexical send of
`vmMirror` and a `SendMirror`. From their text those are `bench_block.ns`
(`value:`, 50,000 a run), `bench_megamorphic.ns` (`at:`, 50,000 a run) and
`bench_do.ns` (`do:`, 5,000 a run, each running ten block activations the
leaf makes); `bench_loop.ns`, `bench_send.ns` and the three `bench_new*`
sources send `printString`, `show:` and `newLine` only once, after their
loop, and should be level. Start-up, against `e2bb31e`: slower by the
kernel's parse, resolution, compilation and instantiation, which did not
exist.

**Measured 2026-09-22, against `e2bb31e` (the merge of leaf inlining into
`master`) and `be3eb40`, by the protocol of "What removing the re-entries
moved" above.** Every binary -- `e2bb31e` and `be3eb40` built in throwaway
worktrees, the head `52c242b` in the current tree -- built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
none imports `VCRUNTIME140.dll` (`dumpbin -dependents`); each checked to
print the expected answer for all eight sources first, the head in both
configurations; `ns-next bench <source> --iterations 30`, five rounds per
sitting, the four configurations -- `e2bb31e`, `be3eb40`, the head, the head
under `NS_NEXT_NO_PRIMITIVE_METHODS=1`, the same binary with the variable
set for its runs only -- rotating by one position from round to round, two
whole sittings, nothing else running.

**`vm::run`'s step is still one function.** A release build with debug
information of the head and of `e2bb31e` (the timed build's `RUSTFLAGS`
plus `CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each, no `vm::run::closure` of its own. `e2bb31e`
measures **5,866 instructions**, the figure leaf inlining recorded, and the
head **6,385** -- 519 more, for the `SendMirror` arm and the primitive-method
test in every inlined `act`. The step did not split, so no remedy was
needed. Two things the head changed on the send path, checked in the same
listing: `vm::deliver`, `#[inline(always)]` and shared by `send` and
`SendMirror`, has no label of its own, so it is inlined into both; and
`act`'s `Primitive` arm, which now builds its error through
`unwrap_or_else`, keeps that error out of line --
`resolve::impl$2::act_with_base::closure$0` and `resolve::unhandled_error`
are small functions of their own, called from three sites each in
`vm::run`, where `e2bb31e` called the cold `act_above` and
`act_handler_primitive` from four each. What the primitive methods add to
the hot path is visible as calls: `vm::run` calls `builtins::execute` from
eight sites, against four in `e2bb31e` (`mirror::execute_method` is inlined
into each method arm, and the leaf it runs stays out of line), and
`mirror::run_leaf` from one, the `SendMirror` arm.

Medians in ms per round, then each group's mean; sitting 1:

| Source | `e2bb31e`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | Head, rounds 1–5 | mean | Head, no primitive methods, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.21, 7.14, 7.09, 7.20, 7.12 | 7.15 | 11.25, 11.10, 11.33, 11.26, 11.23 | 11.23 | 6.97, 7.13, 7.17, 7.09, 6.94 | 7.06 | 6.87, 7.06, 7.04, 7.04, 6.94 | 6.99 |
| `bench_send.ns` | 8.43, 8.43, 8.51, 8.33, 8.51 | 8.44 | 13.73, 13.92, 14.03, 14.01, 13.95 | 13.93 | 8.16, 8.29, 8.40, 8.38, 8.41 | 8.33 | 8.33, 8.24, 8.40, 8.30, 8.41 | 8.33 |
| `bench_block.ns` | 17.58, 17.81, 17.79, 17.78, 17.83 | 17.76 | 22.98, 23.45, 23.21, 23.35, 23.33 | 23.27 | 16.83, 16.94, 16.84, 16.64, 16.95 | 16.84 | 21.82, 21.58, 21.36, 21.60, 21.43 | 21.56 |
| `bench_megamorphic.ns` | 11.59, 11.52, 11.58, 11.46, 11.61 | 11.55 | 17.96, 18.08, 17.97, 17.96, 18.06 | 18.01 | 11.32, 11.34, 11.43, 11.55, 11.42 | 11.41 | 14.32, 14.29, 14.44, 14.21, 14.30 | 14.31 |
| `bench_do.ns` | 5.59, 5.71, 5.59, 5.64, 5.57 | 5.62 | 8.09, 8.02, 8.14, 8.26, 8.61 | 8.23 | 5.76, 5.72, 5.81, 5.66, 6.23 | 5.84 | 5.98, 5.98, 5.98, 6.01, 5.99 | 5.99 |
| `bench_new.ns` | 4.81, 4.96, 4.92, 4.95, 4.98 | 4.92 | 7.42, 7.50, 7.73, 7.53, 7.47 | 7.53 | 4.81, 4.84, 4.82, 4.79, 4.88 | 4.83 | 4.92, 4.93, 4.94, 4.98, 4.98 | 4.95 |
| `bench_new2.ns` | 5.87, 6.03, 5.91, 5.88, 5.93 | 5.92 | 8.34, 8.36, 8.37, 8.41, 8.38 | 8.37 | 5.84, 5.81, 5.80, 5.79, 5.75 | 5.80 | 5.79, 5.97, 5.83, 5.75, 5.78 | 5.82 |
| `bench_newarg.ns` | 8.16, 8.08, 8.11, 8.18, 8.14 | 8.14 | 10.54, 10.84, 10.79, 10.60, 10.67 | 10.69 | 8.06, 8.13, 8.03, 7.95, 8.03 | 8.04 | 8.25, 8.37, 8.33, 8.29, 8.46 | 8.34 |

Sitting 2:

| Source | `e2bb31e`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | Head, rounds 1–5 | mean | Head, no primitive methods, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.24, 7.03, 7.08, 7.31, 7.20 | 7.17 | 11.17, 11.34, 11.22, 11.22, 11.14 | 11.22 | 7.00, 7.01, 7.10, 6.96, 7.11 | 7.04 | 7.04, 7.15, 7.05, 7.08, 7.03 | 7.07 |
| `bench_send.ns` | 8.65, 8.53, 8.34, 8.38, 8.53 | 8.49 | 13.90, 13.94, 14.10, 13.95, 14.04 | 13.98 | 8.33, 8.26, 8.37, 8.50, 8.33 | 8.36 | 9.38, 8.26, 8.36, 8.34, 8.36 | 8.54 |
| `bench_block.ns` | 17.71, 17.67, 17.81, 17.62, 17.91 | 17.74 | 23.18, 23.31, 23.27, 23.29, 23.33 | 23.28 | 17.85, 16.99, 16.59, 16.86, 17.11 | 17.08 | 21.55, 21.37, 21.56, 21.49, 21.49 | 21.49 |
| `bench_megamorphic.ns` | 11.41, 11.75, 11.59, 11.79, 11.43 | 11.59 | 18.04, 17.95, 18.19, 18.06, 18.02 | 18.05 | 11.44, 11.43, 11.38, 11.38, 11.45 | 11.41 | 14.42, 14.24, 14.33, 14.32, 14.17 | 14.30 |
| `bench_do.ns` | 5.78, 5.74, 5.72, 5.64, 5.66 | 5.71 | 8.00, 8.00, 8.05, 7.94, 8.08 | 8.02 | 5.71, 5.64, 5.85, 5.74, 5.71 | 5.73 | 5.97, 5.92, 5.99, 5.92, 6.00 | 5.96 |
| `bench_new.ns` | 4.91, 4.92, 4.90, 5.00, 4.89 | 4.92 | 7.42, 7.37, 7.55, 7.50, 7.48 | 7.47 | 4.88, 4.81, 4.87, 4.82, 4.81 | 4.84 | 4.91, 4.94, 4.95, 4.98, 4.97 | 4.95 |
| `bench_new2.ns` | 5.99, 5.94, 5.85, 5.94, 5.93 | 5.93 | 8.36, 8.41, 8.36, 8.27, 8.44 | 8.37 | 5.85, 5.81, 5.79, 5.85, 5.88 | 5.84 | 5.72, 5.82, 5.78, 5.86, 5.79 | 5.79 |
| `bench_newarg.ns` | 8.20, 8.19, 8.04, 8.23, 8.15 | 8.16 | 10.61, 10.63, 10.59, 10.53, 10.54 | 10.58 | 7.98, 7.98, 8.01, 8.00, 7.92 | 7.98 | 8.41, 8.31, 8.37, 8.38, 8.36 | 8.36 |

The change of each head configuration's mean against each baseline,
sitting 1 / sitting 2:

| Source | Head against `e2bb31e` | No primitive methods against `e2bb31e` | Head against `be3eb40` | No primitive methods against `be3eb40` |
|---|---|---|---|---|
| `bench_loop.ns` | −1.3% / −1.9%, overlapping | −2.3% / −1.4%, entirely below in the first | −37.2% / −37.3%, entirely below | −37.8% / −37.0%, entirely below |
| `bench_send.ns` | −1.3% / −1.5%, overlapping | −1.3% / +0.6%, overlapping | −40.2% / −40.2%, entirely below | −40.2% / −38.9%, entirely below |
| `bench_block.ns` | −5.2% / −3.7%, entirely below in the first | **+21.4% / +21.1%, entirely above** | −27.6% / −26.6%, entirely below | −7.3% / −7.7%, entirely below |
| `bench_megamorphic.ns` | −1.2% / −1.6%, overlapping | **+23.9% / +23.3%, entirely above** | −36.6% / −36.8%, entirely below | −20.5% / −20.8%, entirely below |
| `bench_do.ns` | +3.8% / +0.4%, overlapping | **+6.5% / +4.4%, entirely above** | −29.1% / −28.5%, entirely below | −27.2% / −25.7%, entirely below |
| `bench_new.ns` | −1.9% / −1.7%, entirely below in the second | +0.5% / +0.6%, overlapping | −35.9% / −35.2%, entirely below | −34.3% / −33.7%, entirely below |
| `bench_new2.ns` | −2.1% / −1.5%, entirely below in the first | −1.7% / −2.2%, overlapping | −30.8% / −30.3%, entirely below | −30.5% / −30.8%, entirely below |
| `bench_newarg.ns` | −1.2% / −2.3%, entirely below in the second | **+2.5% / +2.5%, entirely above** | −24.8% / −24.6%, entirely below | −21.9% / −21.0%, entirely below |

Both sittings pooled, means in ms:

| Source | `e2bb31e` | `be3eb40` | Head | No primitive methods | Head against `e2bb31e` | No primitive methods against `e2bb31e` | No primitive methods against the head |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.16 | 11.23 | 7.05 | 7.03 | −1.6% | −1.8% | −0.2% |
| `bench_send.ns` | 8.46 | 13.96 | 8.34 | 8.44 | −1.4% | −0.3% | +1.1% |
| `bench_block.ns` | 17.75 | 23.27 | 16.96 | 21.52 | −4.5% | **+21.3%** | **+26.9%** |
| `bench_megamorphic.ns` | 11.57 | 18.03 | 11.41 | 14.30 | −1.4% | **+23.6%** | **+25.3%** |
| `bench_do.ns` | 5.66 | 8.12 | 5.78 | 5.97 | +2.1% | **+5.5%** | +3.3% |
| `bench_new.ns` | 4.92 | 7.50 | 4.83 | 4.95 | −1.8% | +0.5% | +2.4% |
| `bench_new2.ns` | 5.93 | 8.37 | 5.82 | 5.81 | −1.8% | −2.0% | −0.1% |
| `bench_newarg.ns` | 8.15 | 10.63 | 8.01 | 8.35 | −1.7% | +2.5% | +4.3% |

**With primitive methods on, moving the core library into Newspeak cost
nothing measurable: the head is −4.5% to +2.1% against `e2bb31e`, and no
head group lies entirely above its `e2bb31e` group in either sitting.** The
expectation said level, and that is what the suite shows: seven sources
between −1.4% and −4.5%, four of them entirely below in one sitting, and
`bench_do.ns` at +3.8% / +0.4%, overlapping, its first-sitting mean carried
by one round at 6.23 ms against four between 5.66 and 5.81. The field test
every method send now makes does not show: `bench_send.ns` activates one
ordinary method per iteration, `tick:`, and is −1.4%. Against `be3eb40` the
head is 24.7% to 40.2% faster on every source, entirely below in both
sittings.

**Without them, the three sources named in advance are the three that got
clearly slower**, entirely above in both sittings: `bench_megamorphic.ns`
+23.6% and `bench_block.ns` +21.3%, each sending one leaf 50,000 times a run
(`at:` and `value:`), and `bench_do.ns` +5.5%, whose `do:` is sent 5,000
times. That is what the primitive methods recover: 26.9% of
`bench_block.ns`'s time, 25.3% of `bench_megamorphic.ns`'s and 3.3% of
`bench_do.ns`'s against the same binary without them. Per send, the
activation, the lexical `vmMirror` send and the `SendMirror` that replace a
primitive method's fast path cost about 58 ns on `bench_megamorphic.ns`
(2.89 ms over 50,000 `at:`) and about 91 ns on `bench_block.ns` (4.56 ms over
50,000 `value:`, whose leaf then activates the block, so the kernel method's
own activation sits beneath the block's on the stack); on `bench_do.ns`, 0.19
ms over 5,000 `do:` is about 38 ns each. Even so, every source without
primitive methods stays 7.5% to 39.5% faster than `be3eb40`, pooled.

**What is left unexplained.** `bench_newarg.ns` is 2.5% slower without
primitive methods in both sittings, entirely above, and `bench_new.ns` 2.4%
slower than the head, entirely above in both, although neither sends a
kernel method in its loop, by its text: `--cache-statistics` counts four more
sites filled without primitive methods on every source, which fits the
`vmMirror` and mirror sends of the activated `printString`, `show:` and
`newLine` each sends once after its loop. Being the same binary, code
placement is not the explanation either; what differs between the two is the
kernel's compiled bodies, and how that reaches an instantiation loop is not
isolated. Nor is why
`bench_block.ns` is 4.5% *faster* with primitive methods than `e2bb31e`,
entirely below in the first sitting: its `value:` went from a primitive
entry of `Block`'s method table, run by `act`'s `Primitive` arm, to a
kernel method whose leaf runs from its `Method` arm, and nothing on that
path is obviously cheaper. Neither was profiled.

**Two costs the primitive methods carry that the suite does not exercise.**
A leaf that declines runs twice: once in `act`'s fast path, and again from
the activated body's `SendMirror` before its failure block runs. It is paid
only on a failure path: every kernel method's failure block resends its
message to `super`, which for all but a few rows (`Integer`'s `=` reaching
`Object`'s) ends in `doesNotUnderstand:`. No figure was taken; the suite runs
no failing leaf. And a cached site
whose target is a primitive method reserves feedback-table room for that
method's family although the fast path never activates it:
`ns-next run --cache-statistics` counts 23 site entries for `bench_loop.ns`
against 15 in `e2bb31e`, 26 against 16 for `bench_block.ns` and 39 against
29 for `bench_megamorphic.ns`, with the same number filled -- 8 to 10
entries of 24 bytes, 192 to 240 bytes a run, for the `printString`, `show:`
and `newLine` sent once.

**Start-up is 0.5 ms slower**, the kernel's parse, resolution, compilation
and instantiation, as expected. The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, wall-clock from the parent process, 50
runs of each binary alternating, in two sittings: `e2bb31e` median 3.56 ms
and 3.55 ms (10th to 90th percentile 3.31–3.71 and 3.46–3.82), the head
4.08 ms and 4.06 ms (3.84–4.35 and 3.96–4.31) -- +0.51 ms, 14%, the head's
10th percentile above `e2bb31e`'s 90th in both. That is under a
millisecond, so embedding the compiled kernel as an image generated at
build time ([core-library.md](core-library.md), "Where the library source
lives, and how modules are found") is not called for by this figure; it stays the named next step if
the kernel grows until it is.

### What `Top`, `Value` and `Number` cost

Giving the core library the specification's object model
([core-library.md](core-library.md), "What exists"; §3.2 and §3.1.1) changed
three things a release build runs. The hierarchy gained two levels -- `Top`
above `Object`, and `Value`, with `Number` under it, between `Object` and the
value classes -- so a *failed* lookup walks two levels more before it reaches
`doesNotUnderstand:`. `Instruction::InitializeSlot` gained a branch, the
deep-immutability check of §3.1.1's second condition
(`instantiate::check_value_slot`). And `==` on a value is no longer `Object`'s
identity leaf but `Value`'s `^self = other`, two dispatches where there was
one.

**The expectation, recorded before any binary was built.** Level on all eight
sources. No send in any of the eight loops fails, so the two extra levels
cannot show. **None of the eight compares values in its loop** -- no `=` and
no `==` appears in any of them, in the loop or outside it -- so the two
dispatches cannot show either, and this measurement does not bound them; what
a value comparison costs is stated from the source instead. That leaves the
`InitializeSlot` branch, which can show only on `bench_new.ns`, `bench_new2.ns`
and `bench_newarg.ns`, the three that run one or two initializers per
iteration, 20,000 iterations each. Every one of their stores is an immediate,
so the check returns on its first test.

**Measured 2026-09-23, `ab692b7` against `b0a771d`, by the protocol of "What
removing the re-entries moved" above.** `b0a771d` is this branch's head before
the first commit of the work (`1dc548a`) and is itself documentation only, so
its tree is the code as it stood before; `ab692b7` is the last commit of the
work, the one the figures below were taken at. `2ad6f30` landed afterwards --
a value's state is the slots its classes declare, so a nested class's cache
slot is no longer compared or hashed -- and touches no path any of the eight
sources runs: none of them declares a value class, and the change is confined
to `builtins/value.rs`'s walks and the two reflective leaves. The figures
stand for the head. Every binary built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
neither imports `VCRUNTIME140.dll` (`dumpbin -dependents`); each checked to
print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds a sitting, the binaries
rotating by one position from round to round, two whole sittings, nothing else
running.

**Five binaries rather than two, and why.** The first pairing -- one build of
the baseline against one build of the head -- put `bench_block.ns` 13.4%
*faster* at the head, every group entirely below, which nothing the change
touches can explain: `bench_block.ns` instantiates nothing, and its loop sends
`<`, `+`, `value:` and `bump:`. Two further builds of the baseline and two of
the head, from the same trees with the same flags, showed what it was. On
`bench_block.ns` alone the five builds sit at five separate levels --
17.21 ms, 18.86 (baseline) and 16.49, 19.13, 19.13 (head) -- each stable to
about 1% across ten rounds, with the fastest and the two slowest all being
head builds. The spread is 16%, it straddles the commit boundary, and it is
therefore a property of the build, not of the change. The other seven sources
show 0.3% to 2.3% between builds of the same tree. `bench_block.ns` is the
longest-running source and the only one whose loop activates a block through a
primitive method; what in the layout decides which level a build lands on was
not isolated. **One build per commit is not enough for `bench_block.ns`**, and
the comparison below pools the builds of each commit instead.

**`vm::run`'s step is still one function.** A release build with debug
information of each tree (the timed build's `RUSTFLAGS` plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each, no `vm::run::closure` of its own. The
baseline measures **6,389 instructions** and the head **6,506** -- 117 more,
the inlined `check_value_slot` in the `InitializeSlot` arm. `check_value_slot`
has no label of its own in either listing, so it is inlined rather than
called; the only new function is the `#[cold]`
`instantiate::non_value_slot_error`, which builds the refusal. The step did not
split, so no remedy was needed.

Means in ms per build, over that sitting's five rounds; sitting 1, then
sitting 2. `base1` and `base2` are the two baseline builds, `head1` to `head3`
the three head builds:

| Source | `base1` | `base2` | `head1` | `head2` | `head3` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.16 | 7.10 | 7.14 | 7.08 | 7.14 |
| `bench_send.ns` | 8.45 | 8.45 | 8.49 | 8.74 | 8.45 |
| `bench_block.ns` | 18.68 | 17.17 | 16.44 | 19.01 | 19.05 |
| `bench_megamorphic.ns` | 11.56 | 11.52 | 11.57 | 11.60 | 11.62 |
| `bench_do.ns` | 5.73 | 5.81 | 5.79 | 5.85 | 5.95 |
| `bench_new.ns` | 5.01 | 5.04 | 4.97 | 4.97 | 4.98 |
| `bench_new2.ns` | 5.80 | 5.90 | 6.02 | 6.01 | 6.05 |
| `bench_newarg.ns` | 8.10 | 8.08 | 8.05 | 8.06 | 7.96 |

| Source | `base1` | `base2` | `head1` | `head2` | `head3` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.14 | 7.14 | 7.18 | 7.17 | 7.15 |
| `bench_send.ns` | 8.53 | 8.45 | 8.54 | 8.50 | 8.51 |
| `bench_block.ns` | 19.05 | 17.25 | 16.54 | 19.24 | 19.21 |
| `bench_megamorphic.ns` | 11.60 | 11.56 | 11.63 | 11.65 | 11.65 |
| `bench_do.ns` | 5.77 | 5.79 | 5.87 | 5.87 | 5.98 |
| `bench_new.ns` | 5.00 | 5.07 | 5.00 | 5.04 | 5.00 |
| `bench_new2.ns` | 5.86 | 5.83 | 6.05 | 6.08 | 6.13 |
| `bench_newarg.ns` | 8.05 | 8.11 | 8.08 | 8.08 | 8.03 |

Both sittings pooled, the baseline's two builds against the head's three:

| Source | Baseline | Head | Change, pooled | Change, sitting 1 / 2 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.13 | 7.14 | +0.1% | −0.1% / +0.3% |
| `bench_send.ns` | 8.47 | 8.54 | +0.8% | +1.3% / +0.3% |
| `bench_block.ns` | 18.04 | 18.25 | +1.2% | +1.3% / +1.0% |
| `bench_megamorphic.ns` | 11.56 | 11.62 | +0.5% | +0.4% / +0.5% |
| `bench_do.ns` | 5.78 | 5.88 | +1.9% | +1.6% / +2.2% |
| `bench_new.ns` | 5.03 | 4.99 | −0.7% | −1.0% / −0.5% |
| `bench_new2.ns` | 5.85 | **6.06** | **+3.6%** | +3.0% / +4.2% |
| `bench_newarg.ns` | 8.08 | 8.05 | −0.5% | −0.8% / −0.2% |

**Seven of the eight are level, and the expectation held for them**: −0.7% to
+1.9%, inside the 0.3–2.3% these sources show between builds of the same tree.
`bench_block.ns`'s +1.2% is not a figure at all -- the build-to-build spread on
that source is 16%, and both the fastest and the slowest builds are head
builds, so this measurement says nothing about it either way.

**Open finding: `bench_new2.ns` is 3.6% slower**, and that is a real
regression, not scatter. All three head builds (6.02, 6.01, 6.05 and 6.05,
6.08, 6.13) lie above both baseline builds (5.80, 5.90 and 5.86, 5.83) in both
sittings, and the source's build-to-build spread is 0.7%. It was one of the
three the expectation named. What does not follow is why it is the only one:
`bench_newarg.ns` runs the same two initializers per instantiation over the
same two levels and is −0.5%, and `bench_new.ns`, with one, is −0.7%. So the
per-store check alone does not explain it -- all three store immediates, where
the check returns on its first test. The other two candidates on that path are
the `Ok`/`Err` match the `InitializeSlot` arm now carries, which is new
control flow inside `vm::run` where there was a straight store, and
`MixinApplication` having grown two fields (`inherits_value` and
the first mutable slot), which `decide_instantiation` reads once per instantiation
for §3.1.1's first condition. Neither was profiled, and nothing was bisected
between them. **What would recover it**: deciding the check once per
activation instead of once per store. The application already knows
`inherits_value`, and the executing activation already reads its level's
application for `slot_base`, so `InitializeSlot` could branch on a flag the
frame carries rather than walking `receiver -> class -> application` on every
store; that is the change to make if the figure ever matters, and it was left
undone because 3.6% on one instantiation benchmark did not warrant it before
`Class`, `Behavior` and `Metaclass` land and change the same path again.

**Start-up did not move measurably.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, wall-clock from the parent process, 100 runs
of each binary over two sittings, the four binaries rotating: baseline medians
5.10 ms and 5.13 ms (10th to 90th percentile 4.89–5.49 and 4.91–5.53), head
4.94 ms and 5.26 ms (4.67–5.36 and 5.00–5.70). The two head builds straddle
the two baseline builds, so the three classes and seven methods the kernel
gained cost less than the scatter between builds. These figures are not
comparable with the core library's move above, which was a different sitting on
a differently loaded machine.

### What mixin application cost

§6.2's mixin application -- one class level per `<:` clause, between a
declaration's own class and its superclass (`nested::build_applied_levels`) --
runs on two paths every program takes: the instantiation walk behind every
`new`, and the nested-class accessor's creation of a class object. Neither
`vm.rs`, `act`, the inline caches nor the compiled code of a send changed.
**A program that writes no `<:` pays one test per climbed level, one per
`descend_levels` call and one per nested-class creation for it**, and nothing
else:

- The climb (`instantiate::climb_levels`) is the ordinary walk, with one test
  per level of whether the superclass is an applied level
  (`Heap::class_is_applied_level`, a read of `ClassData::applied_clause`). The
  first applied level hands the rest of the climb to
  `climb_through_applied_levels`, `#[cold]` and `#[inline(never)]`, which
  keeps the applying declaration's index and defers each `<:` clause's
  message.
- The descent (`descend_levels`) tests whether any message was deferred and
  otherwise is the ordinary descent; `descend_to_deferred_message` and the
  resumption after one (`advance_past_applied_message`) are out of line the
  same way.
- The accessor's `after_clause` tests whether the declaration has a `<:`
  clause; one that does builds its levels in the out-of-line
  `superclass_with_applied_levels`.
- `check_factory_arity`, `clause_message_body` and `argument_list` are
  `#[inline(always)]`, since the cold paths are second callers of each, and
  `ClassData::applied_clause` is boxed, so a class object grows by one word.

**Measured 2026-09-23, `06fd024` against `6833234`**, the commit before
mixin application, by the protocol of "What removing the re-entries moved" above:
three builds of each tree with the pinned flags, none importing
`VCRUNTIME140.dll`, each checked to print the expected answer for every
source; `ns-next bench <source> --iterations 30`, five rounds a sitting, the
binaries rotating, three sittings. The sources are the eight of "What `Top`,
`Value` and `Number` cost" and two more throwaway ones: `bench_newarg2.ns` is
`bench_newarg.ns` with the argument forwarded,
`class Cell y: b = Base x: b + 1 ( | v ::= b. | )`, instantiated as `Cell y: 1`;
`bench_nested.ns` creates 20,000 fresh enclosing objects and instantiates a
nested class of each, `Holder new Inner new v`, so every iteration runs the
accessor's creation path once and two instantiations. A release build with
debug information of each tree puts `vm::run` at 6,508 instructions in both,
one function with no closure of its own. Means in ms, all three sittings
pooled:

| Source | `6833234` | `06fd024` | Change, pooled | Sitting 1 / 2 / 3 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.02 | 7.04 | +0.3% | +0.2% / +0.7% / −0.1% |
| `bench_send.ns` | 8.37 | 8.34 | −0.4% | −0.3% / −0.6% / −0.2% |
| `bench_block.ns` | 18.45 | 18.66 | +1.2% | +1.1% / +0.7% / +1.7% |
| `bench_megamorphic.ns` | 11.44 | 11.46 | +0.2% | +0.4% / −0.0% / +0.1% |
| `bench_do.ns` | 5.73 | 5.72 | −0.0% | −0.0% / +0.2% / −0.2% |
| `bench_new.ns` | 4.17 | 4.19 | +0.5% | +0.3% / +0.9% / +0.4% |
| `bench_new2.ns` | 5.18 | 5.23 | +0.9% | +1.1% / +0.6% / +0.9% |
| `bench_newarg.ns` | 7.34 | 7.18 | −2.2% | −1.7% / −1.4% / −3.4% |
| `bench_newarg2.ns` | 8.27 | 8.39 | +1.4% | +1.5% / +1.1% / +1.5% |
| `bench_nested.ns` | 7.87 | 7.95 | +1.0% | +1.0% / +0.8% / +1.3% |

**All ten are within the protocol's noise**: −2.2% to +1.4%, inside the
0.3–2.3% these sources show between builds of one tree. The five send
sources are level, the groups overlapping, except `bench_block.ns`, which
instantiates nothing and is the source whose build-to-build spread was
measured at 16% above.

**Four instantiating sources still sit above the base**: `bench_new.ns`
+0.5%, `bench_new2.ns` +0.9%, `bench_newarg2.ns` +1.4% and `bench_nested.ns`
+1.0%, every build of `06fd024` above every build of `6833234` in every
sitting, while `bench_newarg.ns` sits 2.2% below. By the criterion of "What
`Top`, `Value` and `Number` cost" that is a real difference, if a small one.
Its cause is not isolated. Placement of data is the likely one: two builds of
the base tree with only an unused field of the new fields' sizes added to
`Mixin` and to `ClassData`, in one sitting of four rounds, moved the five
instantiating sources by −1.8% (`bench_newarg.ns`) to +0.6%
(`bench_nested.ns`), the same direction and much of the size. It is unproven,
though -- one sitting, two builds -- and the one added test per level, per
descent and per nested-class creation is work that remains alongside it.

### What the value conditions cost

Enforcing §3.1.1's four conditions on a value
([object-model.md](object-model.md), "A value object's four conditions")
decides them where a class or an application comes into being and refuses them
at instantiation. Neither `vm.rs`, `act`, the inline caches nor the compiled
code of a send changed. What a release build runs that it did not run before:

- `nested::after_clause` tests `application.inherits_value` once per class
  object created, and only a class that inherits `Value` runs
  `value_class::check_value_class`.
- `Mutator::apply_mixin` folds the root clause defect, the mutable slot and the
  identity method into one `MixinApplication::instantiation_refusal`, once per
  application. `decide_instantiation` already tested that `Option`, where it
  had tested `mutable_value_slot`.
- `decide_instantiation` tests whether `ClassData::value_class_refusal`, a new
  field of the boxed class data, holds a refusal
  (`Heap::class_has_value_refusal`), once per instantiation. It does so only
  for a class whose application inherits `Value`, and reads the refusal
  itself only on the cold error path (`value_class_refusal_error`).
- The kernel is `Object <: Value`, so bootstrap builds one applied level for
  it.
- The image decoder has a second pass that recomputes each decoded class
  object's verdict. No source below loads an image.

**The expectation, recorded before any binary was built.** Level on every
source. The one new test on a path the loops run is the `inherits_value` test
on the value-class read in `decide_instantiation`, and only the instantiating
sources reach it. Start-up is level too: one applied level is small beside the
kernel's compilation.

**Measured 2026-09-24, `d773380` against `cdc0257`**, the merge of mixin
application and this branch's start, by the protocol of "What removing the
re-entries moved" above. Every binary was built with the pinned flags. None
imports `VCRUNTIME140.dll` (`dumpbin -dependents`), and each was checked to
print the expected answer for every source first. The timing is
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round. The sources are the four
fixtures under `goldens/bench/` and the throwaway `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` of that protocol, and one
more throwaway source, `bench_newvalue.ns`. Its module is `Object <: Value`, and
its loop makes 20,000 instantiations of `class Cell = Value new ( | public v =
1. | )`, each followed by `v`. It is the one source that reads the new field.

**Three builds of one tree are one binary.** Building a tree three times
(separate target directories, or separate checkouts at paths of different
lengths) gave executables that differ in 20 to 24 bytes: the link timestamp
and the debug-information identifier. Their code and data are byte for byte
the same. The first three sittings timed three such copies of each tree, and
each tree's copies agreed to within 0.3–0.5% on `bench_block.ns`. The 16%
spread that "What `Top`, `Value` and `Number` cost" found between builds of one
tree did not appear. For builds that actually differ in layout, the protocol
used here builds each tree three ways: unperturbed, and twice with an extra
`-C metadata=<word>` in `RUSTFLAGS` (`layoutprobe` and `secondprobe`). The
extra metadata changes every symbol hash, and with it the link order. About
1.85 MB of the 2.4 MB binary differs between them. The tables below are three
more sittings over those six binaries: `base`, `base-p` and `base-q` from
`cdc0257`, and `head`, `head-p` and `head-q` from `d773380`.

**`vm::run`'s step is still one function.** A release build with debug
information of each tree has one function label matching `vm::run` in each,
and no `vm::run::closure` of its own. The base has **6,508 instructions**,
the head 6,507. `decide_instantiation` grew from 441 to 503 instructions and
`after_clause` from 231 to 433. The new refusal errors
(`instantiation_refusal_error`, `value_class_refusal_error`) are out of line.
Until the review's fix (below), `decide_instantiation` cloned the class's
`Option<ValueClassRefusal>` to test it, and `value_class_refusal_error` was
inlined into it; since `7b2af64` it is **371 instructions**, below the base's
441.

Means in ms per build, over each sitting's five rounds; sittings 4, 5 and 6:

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.13 | 7.04 | 7.05 | 7.07 | 7.04 | 7.01 |
| `bench_send.ns` | 8.29 | 8.53 | 8.32 | 8.36 | 8.31 | 8.64 |
| `bench_block.ns` | 18.66 | 18.60 | 18.45 | 19.58 | 19.33 | 19.28 |
| `bench_megamorphic.ns` | 11.45 | 11.51 | 11.44 | 11.45 | 11.49 | 11.47 |
| `bench_do.ns` | 5.69 | 5.71 | 5.70 | 5.67 | 5.75 | 5.72 |
| `bench_new.ns` | 4.20 | 4.18 | 4.21 | 4.30 | 4.26 | 4.27 |
| `bench_new2.ns` | 5.23 | 5.23 | 5.29 | 5.26 | 5.22 | 5.24 |
| `bench_newarg.ns` | 7.19 | 7.16 | 7.14 | 7.10 | 7.11 | 7.16 |
| `bench_newvalue.ns` | 3.80 | 3.81 | 3.81 | 3.71 | 3.67 | 3.67 |

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.04 | 7.01 | 7.01 | 7.06 | 7.11 | 7.09 |
| `bench_send.ns` | 8.35 | 8.37 | 8.29 | 8.36 | 8.56 | 8.31 |
| `bench_block.ns` | 18.51 | 18.39 | 18.53 | 19.31 | 19.34 | 19.36 |
| `bench_megamorphic.ns` | 11.44 | 11.47 | 11.48 | 11.50 | 11.48 | 11.54 |
| `bench_do.ns` | 5.68 | 5.74 | 5.71 | 5.70 | 5.69 | 5.69 |
| `bench_new.ns` | 4.18 | 4.21 | 4.21 | 4.30 | 4.31 | 4.30 |
| `bench_new2.ns` | 5.21 | 5.22 | 5.28 | 5.27 | 5.22 | 5.33 |
| `bench_newarg.ns` | 7.17 | 7.18 | 7.14 | 7.13 | 7.15 | 7.09 |
| `bench_newvalue.ns` | 3.81 | 3.80 | 3.80 | 3.66 | 3.71 | 3.66 |

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.02 | 7.02 | 7.07 | 7.00 | 7.06 | 6.99 |
| `bench_send.ns` | 8.34 | 8.35 | 8.28 | 8.41 | 8.32 | 8.31 |
| `bench_block.ns` | 18.55 | 18.55 | 18.71 | 19.39 | 19.45 | 19.34 |
| `bench_megamorphic.ns` | 11.49 | 11.42 | 11.50 | 11.45 | 11.42 | 11.44 |
| `bench_do.ns` | 5.68 | 5.74 | 5.70 | 5.68 | 5.69 | 5.68 |
| `bench_new.ns` | 4.17 | 4.16 | 4.19 | 4.30 | 4.25 | 4.28 |
| `bench_new2.ns` | 5.23 | 5.21 | 5.26 | 5.25 | 5.20 | 5.36 |
| `bench_newarg.ns` | 7.19 | 7.18 | 7.14 | 7.16 | 7.16 | 7.07 |
| `bench_newvalue.ns` | 3.82 | 3.78 | 3.80 | 3.67 | 3.69 | 3.66 |

The three sittings pooled, the base's three layouts against the head's three,
and, in the last column, the first three sittings' three identical copies of
each tree, pooled the same way:

| Source | `cdc0257` | `d773380` | Change, pooled | Sitting 4 / 5 / 6 | Sittings 1–3, identical copies |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.04 | 7.05 | +0.1% | −0.4% / +1.0% / −0.3% | +0.0% |
| `bench_send.ns` | 8.35 | 8.40 | +0.6% | +0.7% / +0.9% / +0.3% | −0.1% |
| `bench_block.ns` | 18.55 | **19.38** | **+4.5%** | +4.5% / +4.7% / +4.3% | +3.3% |
| `bench_megamorphic.ns` | 11.47 | 11.47 | +0.0% | +0.0% / +0.4% / −0.3% | +0.4% |
| `bench_do.ns` | 5.71 | 5.70 | −0.2% | +0.2% / −0.3% / −0.4% | −0.5% |
| `bench_new.ns` | 4.19 | **4.28** | **+2.2%** | +1.9% / +2.4% / +2.4% | +2.7% |
| `bench_new2.ns` | 5.24 | 5.26 | +0.4% | −0.2% / +0.8% / +0.7% | +0.4% |
| `bench_newarg.ns` | 7.17 | 7.12 | −0.6% | −0.6% / −0.6% / −0.5% | −0.7% |
| `bench_newvalue.ns` | 3.80 | 3.68 | −3.3% | −3.2% / −3.2% / −3.4% | −2.7% |

**Six of the nine are level**: −0.6% to +0.6%, the groups overlapping.
`bench_newvalue.ns`, the one source that reads the new field, is 3.3%
*faster*, every head layout below every base layout in every sitting. Nothing
on its path got cheaper, so the change does not explain it.

**Open finding: `bench_block.ns` is 4.5% slower and `bench_new.ns` 2.2%
slower**, and both are real by this protocol's criterion. Every head layout
lies above every base layout in all three sittings, and the three identical
copies in the first three sittings show the same (+3.3% and +2.7%). Neither
source falls inside build noise. The spread between one tree's three layouts
is 0.3–0.6% on each, and `bench_block.ns` did not show its 16% build spread in
any of the twelve binaries. The expectation said level, and it did not hold
for these two. Re-measured after the review's fix (below), `bench_new.ns`'s
gap did not reproduce, before the fix or after it, and `bench_block.ns`'s
did.

The causes are not isolated. `bench_block.ns` creates no class and
instantiates nothing in its loop. It makes lexical sends of its slots and
`bump:`, public sends of `<`, `+` and `value:`, and one block creation per
iteration, and `vm::run` is the same size in both trees. What the head changed
that such a loop can reach is data, not code on its path: the kernel instance
is now a value whose class has one applied level more, which is the enclosing
object of every builtin class, `Block` and `Integer` included, and every class
object's `ClassData` grew by `value_class_refusal`. `bench_new.ns` instantiates
a class that is no value. On its path the new `inherits_value` test sits behind
a refusal test it already had, inside a `decide_instantiation` that grew by 62
instructions. Yet `bench_new2.ns` and `bench_newarg.ns`, which run the same
function with one level more, are level or faster. A control that would
separate the kernel's applied level from the Rust changes, the head with
`Kernel.ns`'s root put back to `Object new`, does not bootstrap:
`kernel.rs` asserts the kernel's root is `Object <: Value`. Nothing was
profiled.

**Re-measured 2026-09-24 after the review's fix, `7b2af64`.** The final
review named the clone of `Option<ValueClassRefusal>` on
`decide_instantiation`'s path, and the inlined refusal error behind it, as
the concrete candidate for `bench_new.ns`. The fix tests for the refusal's
presence and keeps both refusal errors `#[cold]` and `#[inline(never)]`. It
was measured in a new session by the same protocol, over `bench_block.ns`
and `bench_new.ns` only: three trees, `cdc0257` (`base`), `d9c3f33` (the
head before the fix, `old`) and `7b2af64` (`head`), each built plain and with
`-C metadata=layoutprobe` and `=secondprobe`, nine binaries rotating by one
position from round to round, five rounds a sitting and six sittings. A
rebuild of the plain base differed from the first in 24 bytes, as before.
`vm::run` is still 6,507 instructions with no `vm::run::closure`.
Means in ms per layout over the six sittings (plain, `-p`, `-q`):

| Source | `cdc0257` | `d9c3f33` | `7b2af64` | `7b2af64` against `cdc0257`, pooled | against `d9c3f33` |
|---|---|---|---|---|---|
| `bench_block.ns` | 17.27, 18.55, 18.53 | 19.43, 19.39, 19.34 | 19.43, 19.32, 19.33 | +6.9% (+6.7 to +7.1 per sitting) | −0.1% |
| `bench_new.ns` | 4.30, 4.22, 4.23 | 4.34, 4.22, 4.22 | 4.34, 4.21, 4.23 | +0.2% (−1.3 to +0.9 per sitting) | +0.0% |

**`bench_new.ns` is level, and the fix is not what made it so.** The head
before the fix measured the same as the head after it, level with the base,
so the +2.2% above did not reproduce in this session for either tree. The
head's plain build lies about 3% above its two other layouts in every
sitting of both measurements here, and the plain build before the fix does
the same; the base's plain build did so in two of the first six sittings and
in none of the second six. The `-C metadata` layouts of all three trees lie
within 0.5% of one another. What the fix did do is shrink
`decide_instantiation` back below the base.

**`bench_block.ns` is still slower, and the fix did not touch it** (−0.1%
against the head before it), as expected of a source that instantiates
nothing. The pooled +6.9% overstates it: the base's plain build is a layout
outlier in this session, 17.27 ms against 18.55 and 18.53 for its other two
layouts, in every one of twelve sittings across both measurements here.
Against the base's two `-C metadata` layouts alone the head is **4.4%**
slower, which is the finding above.

**Control (a), the seeded symbol: refuted as the cause.** The head seeds
`==` as a well-known symbol (`Symbol::IDENTICAL`), which shifts every later
symbol id by one and with it every `IdentityMap<Symbol, …>` bucket. A
throwaway build of the base with the same seed line, in the same three
layouts, measured in six more sittings beside the base and the head:

| Source | `cdc0257` | `cdc0257` + seed | `7b2af64` | head against base + seed, per sitting |
|---|---|---|---|---|
| `bench_block.ns` | 17.30, 18.49, 18.51 | 18.53, 18.50, 18.54 | 19.44, 19.25, 19.26 | +4.3% (+3.6 to +4.6) |
| `bench_new.ns` | 4.23, 4.23, 4.24 | 4.22, 4.23, 4.22 | 4.35, 4.22, 4.24 | +1.0% (+0.1 to +1.9) |

The seeded base's three layouts lie with the base's two `-C metadata` ones,
and the head is as far above them as before, so the shifted symbol ids
explain none of `bench_block.ns`'s gap. The seed did move the base's plain
build from 17.30 to 18.53 ms, a layout change like any other. On
`bench_new.ns` the head's plain build is again the one 3% above the rest.
The review's two other hypotheses remain open ([PROGRESS.md](../PROGRESS.md),
"The value conditions"): the kernel's extra `Value`-level class object,
which shifts every later arena index and dispatch id by one, and hardware
counters.

**Start-up did not move.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, timed wall-clock from the parent process.
There were 25 rounds of the six layout binaries rotating, in two sittings, so
75 runs of each tree a sitting. The base's medians were 4.95 ms and 4.99 ms
(10th to 90th percentile 4.83–5.23 and 4.79–5.20). The head's were 4.93 ms and
4.90 ms (4.80–5.11 and 4.73–5.14). The kernel's applied level costs less than
the scatter.

### What the class accessors cost

Answering a predefined class through an accessor on `Object`
([core-library.md](core-library.md); [dispatch.md](dispatch.md), "Primitive
methods") changed what a release build runs where a program names one. A bare
builtin name such as `Array` was a constant load, `LoadBuiltinClass`; it is now
a self send, `SendLexical` at depth 0, whose inline-cache hit reaches `act`,
and `act` answers the class off the method's `MethodFastPath::BuiltinClass`
without an activation. A nested superclass clause such as `= Object new` sends
`Object` the same way, once per class object it creates. `LoadBuiltinClass`'s
arm is gone from `vm::run`. What every send now pays is one more arm in `act`'s
test of `method.fast_path`, which was a test of `primitive` before.

**Which sources can move.** None of the four fixtures under `goldens/bench/`
names a builtin class in its loop. Each names `Object` only in its module's
superclass clause, which runs once when the module is instantiated, and
`bench_megamorphic.ns` also in its five nested classes' `= Object new`
clauses, each of which runs once when its class object is first created,
before the loop. The throwaway `bench_new.ns`, `bench_new2.ns`,
`bench_newarg.ns` and `bench_newvalue.ns` name `Object` or `Value` only in the
clauses of classes their loops instantiate, and a class object is created once
per enclosing instance, so their loops send no class accessor either. One more
throwaway source therefore exercises the change: `bench_builtin.ns` runs
`sum:: sum + Array new size + OrderedCollection new size + 1` 20,000 times, so
its loop makes 40,000 class-accessor sends (`SendLexical #Array` and
`#OrderedCollection` in its disassembly) where the base made 40,000
`LoadBuiltinClass`es.

**Measured 2026-09-24, `dd1c70c` against `909bf40`**, the merge of the value
conditions and this branch's start, by the protocol of "What removing the
re-entries moved" above, with the layout variation of "What the value
conditions cost": each tree built plain and with `-C metadata=layoutprobe` and
`=secondprobe`, every build with the pinned flags. The measured tree was the
branch head, `96d5a8d`, whose commits after `dd1c70c` change documentation
only. The plain build and a `-C metadata` build of one tree differ in 1.55 to
1.86 million bytes. None of the six imports `VCRUNTIME140.dll`, and each printed
the expected answer for every source first. The timing is
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round, four sittings. The
sources are the four fixtures, the five throwaway sources of "What the value
conditions cost" (`bench_newvalue.ns` with its sum in a local, since its module
is a value and may declare no mutable slot) and `bench_builtin.ns`.

**`vm::run`'s step is still one function, and it grew.** A release build with
debug information of each tree, `dumpbin -disasm`: one function label matching
`vm::run` in each and no `vm::run::closure`. The base has **6,507
instructions**, the head **6,626**, 119 more although an arm was removed. `act`
has no label of its own in either listing, so it is inlined, and the functions
it is inlined into grew alike: `inline_cache::global_hit` from 909 to 926,
`send_lexical_first` from 577 to 601, and each of the three
`inline_cache::miss` instances by 60 to 63. The likely cause of `vm::run`'s
growth is the same one, the new `fast_path` arm at each place `act` is inlined
into it; that was not isolated. Storing the builtin classes in a fixed-size array rather than a
`Vec` would drop `builtin_class`'s bounds check from each of those sites; it
waits on a bootstrap restructuring ([PROGRESS.md](../PROGRESS.md), "The
predefined classes as accessors on `Object`").

Means in ms per layout, the four sittings pooled:

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.29 | 7.29 | 7.23 | 7.23 | 7.19 | 7.19 |
| `bench_send.ns` | 8.61 | 8.58 | 8.68 | 8.53 | 8.63 | 8.49 |
| `bench_block.ns` | 19.02 | 19.88 | 19.83 | 19.20 | 19.04 | 19.10 |
| `bench_megamorphic.ns` | 11.83 | 11.80 | 11.82 | 11.83 | 11.78 | 11.86 |
| `bench_do.ns` | 5.91 | 6.12 | 6.13 | 5.85 | 5.82 | 5.85 |
| `bench_new.ns` | 4.28 | 4.28 | 4.27 | 4.28 | 4.31 | 4.32 |
| `bench_new2.ns` | 5.34 | 5.32 | 5.37 | 5.33 | 5.37 | 5.40 |
| `bench_newarg.ns` | 7.33 | 7.30 | 7.27 | 7.26 | 7.34 | 7.40 |
| `bench_newvalue.ns` | 3.76 | 3.74 | 3.79 | 3.80 | 3.79 | 3.81 |
| `bench_builtin.ns` | 3.93 | 3.94 | 3.94 | 4.02 | 4.00 | 3.97 |

The base's three layouts against the head's three:

| Source | `909bf40` | `dd1c70c` | Change, pooled | Sitting 1 / 2 / 3 / 4 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.27 | 7.21 | −0.9% | −1.4% / −1.1% / −0.7% / −0.2% |
| `bench_send.ns` | 8.63 | 8.55 | −0.9% | −1.8% / −0.8% / +0.0% / −0.9% |
| `bench_block.ns` | 19.58 | 19.12 | −2.4% | −2.2% / −2.3% / −2.5% / −2.5% |
| `bench_megamorphic.ns` | 11.81 | 11.82 | +0.1% | −0.0% / +0.3% / −0.1% / +0.2% |
| `bench_do.ns` | 6.05 | 5.84 | −3.5% | −3.6% / −3.2% / −3.9% / −3.4% |
| `bench_new.ns` | 4.28 | 4.30 | +0.5% | +1.1% / +0.2% / +0.7% / +0.2% |
| `bench_new2.ns` | 5.34 | 5.37 | +0.4% | +0.7% / +0.4% / +0.4% / +0.1% |
| `bench_newarg.ns` | 7.30 | 7.33 | +0.5% | −0.2% / −0.1% / +1.6% / +0.5% |
| `bench_newvalue.ns` | 3.76 | 3.80 | +0.9% | +1.0% / +0.4% / +1.0% / +1.3% |
| `bench_builtin.ns` | **3.94** | **3.99** | **+1.5%** | +1.9% / +1.0% / +0.8% / +2.1% |

**`bench_builtin.ns`, the one source whose loop names a builtin, is 1.5%
slower.** Pooled over the four sittings every head layout (3.97–4.02 ms) lies
above every base layout (3.93–3.94 ms), whose spread is 0.2%; within single
sittings the slowest base layout touches the fastest head layout in two of
the four. That is about 0.06 ms over 40,000 sends, some 1.4 ns for each, the
difference between an inline-cache hit followed by `act` and a constant load.
The loop also allocates two collections and sends `new` and `size` to each,
so the naming is a small part of what it runs; a loop that did little but name
classes would show a larger share.

**The rest are level.** `bench_new.ns` to `bench_newvalue.ns` sit 0.4–0.9%
above the base with the groups overlapping in every sitting; `bench_loop.ns`,
`bench_send.ns` and `bench_megamorphic.ns` are level or below. `bench_block.ns`
and `bench_do.ns` show the head 2.4% and 3.5% *faster*, which is layout, not
the change: the base's two `-C metadata` layouts lie 4% above its plain
build on both, while the head's three layouts and the base's plain build lie
together (19.02 against 19.04–19.20 ms, and 5.91 against 5.82–5.85 ms).
Neither source names a builtin in its loop. The open finding of "What the
value conditions cost", `bench_block.ns` 4.4% slower at the value conditions'
head, is the base of this measurement, and this measurement does not bear on
it: here its plain layout is the fast one.

**Start-up did not move.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, timed wall-clock from the parent process,
25 rounds of the six binaries rotating, in two sittings, so 75 runs of each
tree a sitting. The base's medians were 5.49 ms and 5.58 ms (10th to 90th
percentile 5.21–5.89 and 5.19–6.08), the head's 5.56 ms and 5.63 ms
(5.27–5.91 and 5.32–6.17): 0.05–0.07 ms apart, inside the scatter.

### What `True` and `False` as quick methods cost

Writing `True` and `False` in Newspeak over quick methods
([core-library.md](core-library.md), "`True` and `False` are quick methods")
changed what a release build runs where a Boolean method is *sent* rather
than inlined: `not`, `&`, `|`, and a conditional, `and:` or `or:` whose
argument is not a literal block. Each was a primitive method whose mirror row
ran a leaf; each is now answered by `act` off the method's
`MethodFastPath::Constant`, `Parameter` or `ValueOfParameter`, with no leaf
and no activation. What every method send now pays is three more arms in
`act`'s test of `method.fast_path`; `vm::run` grew from 6,626 to 6,818
instructions and is still one function, with no `vm::run::closure` of its
own (`dumpbin -disasm` on a release build with debug information, both
trees).

**Which sources can move.** The four fixtures under `goldens/bench/` send
no Boolean method in their loops: their conditionals and loops take literal
blocks and are inlined. One throwaway source exercises the change,
`bench_boolean.ns`, a 200,000-iteration `whileTrue:` loop whose body is
`flag:: flag not. flag ifTrue: block. (flag & true) ifTrue: block.
(flag | false) ifFalse: block`, with `block` a variable holding
`[taken:: taken + 1]` — four sent Boolean methods and three sent
conditionals per iteration; it prints `300000` under both trees.

**Measured 2026-09-24, `e8a2884` against `ddd9e4f`**, the `master` it
started from -- before the fix that marks the block a control-flow step only
for a Boolean receiver (`d4e7924`), one test of the receiver on the quick
path, which was not measured separately -- by the protocol of "What removing the re-entries moved" above:
two builds of each tree, every build with the pinned flags, the four
binaries differing byte for byte; `ns-next bench <source> --iterations 30`,
five rounds of all four binaries in turn, after one sitting of five rounds
of one build each. Medians of the five rounds, in ms:

| Source | before | before, second build | after | after, second build | Pooled |
|---|---|---|---|---|---|
| `bench_boolean.ns` | 55.56 | 55.93 | 43.71 | 43.85 | −21.5% |
| `bench_block.ns` | 19.79 | 19.85 | 19.61 | 19.68 | −0.9% |
| `bench_megamorphic.ns` | 11.66 | 11.73 | 11.63 | 11.66 | −0.6% |
| `bench_send.ns` | 8.42 | 8.44 | 8.59 | 8.53 | +1.3% |
| `bench_loop.ns` | 7.06 | 7.08 | 7.30 | 7.13 | +1.4% |

`bench_boolean.ns` is entirely below: every round of either after-build is
faster than every round of either before-build, 43.2–44.2 ms against
55.0–57.0 in the sitting the table reports, and 43.3–44.2 against 55.0–56.3
in the first. The four fixtures are level by the protocol's gate: for each,
the after-rounds overlap the before-rounds — in the table's sitting,
`bench_send.ns` 8.40–8.67
against 8.31–8.69, `bench_loop.ns` 7.11–7.38 against 7.00–7.28, where the
two after-builds of `bench_loop.ns` alone are 2.4% apart.

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
- **Nor are the control-flow steps** (decided 2026-09-12).
  `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` and
  `or:` on a Boolean answer the block to activate, exactly as `value` does,
  and its value is the send's (`Continuation::Deliver`) -- `True`'s and
  `False`'s `^<parameter> value` quick methods answer it, with no leaf
  ([core-library.md](core-library.md), "`True` and `False` are quick
  methods"); `whileTrue:` and
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
  below). Since step 6 each of these is the leaf a `Kernel.ns` method
  forwards to; rewriting them as Newspeak loops and branches is a separate
  step before the JIT tiers ([core-library.md](core-library.md), "Planned:
  real bodies for control flow and `do:`, before the JIT tiers").
- **Recursion depth is a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded
  artifact. The limit counts every activation except a block activation that a
  builtin control-flow step performs for a `SendInlinable`, the send only the
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

Decided 2026-09-12. Once sends and control flow no longer recurse
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
- **The exemption is marked by the arguments.** A control-flow step's block —
  one a `whileTrue:` or `whileFalse:` leaf activates, or one a Boolean
  receiver's `^<parameter> value` quick method answers — and the failure
  block a sent mirror send activates when its leaf declines
  ([compiler.md](compiler.md), "Mirror sends"), carry
  `StepArguments::ControlFlow` — none has arguments, and only those build
  that variant — and `push_step` exempts one when the send
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
image's to choose — the same exposure `dispatch::walk` was made iterative
for.

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
  of them answers `at:put:` and which `add:` is decided by the methods their
  classes declare in `Kernel.ns`, not here. Each element's activation is counted, like any block a `value`
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

## Closure conversion

**Built 2026-09-20**, designed with the user on 2026-09-19. Step 4 of the order
of work ([PROGRESS.md](../PROGRESS.md)), placed after the inline caches and
before the collector, whose retention it affects most. The decisions are listed
under "Decisions" below; this section is what they add up to.

Under the optimized lowering a block does not reach its method's variables
through the method's frame. A captured variable that is never written after the
block reads it is **copied** into the closure when the closure is created; one
that is written lives in its scope's **indirection vector**, which the
declaring body and every block that needs it share. The reference lowering
keeps the by-reference model, as the oracle the conversion is checked against.

Before this, the activation stack paid for the by-reference model with an
unconditional divorce: a reified frame copied all its registers into the arena
when its activation returned, because a surviving block might still read them.
Whether a returning activation copies its registers out is now decided per
activation, from the block's own `outer_reach` ("Converted representation"
below). Conversion did not remove reification — a block with `^` still needs its home
activation, and so do the debugger and the activation mirrors — which is why it
came after the stack rather than before it: it changes where variables live,
not what an activation is.

**Every Smalltalk and Newspeak VM that moved to stack frames made this change to
do it.** Cog's closure compiler introduced copied values and remote temps
(`pushClosureNumArgs:copiedValues:`, `pushRemoteTemp`) precisely so frames could
live on a stack, and Primordial Soup does the same (`PushClosure`,
`PushIndirectLocal`; its compiler's `ClosureRewriter1`,
`prior-art/primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns`, and
the three cases `ActivationMirrorTesting.ns` names: "Not copied", "Copied",
"Indirect"). Reaching the frame by reference is what systems do whose frames
live in the heap anyway — Smalltalk-80's Blue Book, ns-next before the
activation stack, Nova, which follows ECMAScript's environment records to the
letter (`DeclarativeEnvironmentRecord`, a map of bindings per scope, chained) —
or whose JIT can remove the cost: SOMns materializes the frame for every block
(`prior-art/somns/src/som/interpreter/nodes/literals/BlockNode.java`,
`frame.materialize()`) and relies on Graal's partial escape analysis, which
removes the materialization only when the block is inlined.

### The closure keeps its defining activation

A closure keeps `captured_frame`, the reified activation that created it, as in
Cog (`BlockClosure>>outerContext`), Primordial Soup
(`Closure::defining_activation`) and SOMns. Only variable access moves to
copies and vectors. Newspeak's reflective API depends on the link:
`Activation>>enclosingActivation` (`KernelForPrimordialSoup.ns`) and
`ActivationMirror>>enclosingActivation` (`Mirrors.ns`) name a block
activation's creator, the debugger labels a stack entry "`[] in Foo bar`" by
walking that chain (`ActivationMirrorTesting.ns`, `describeActivation:to:`),
the creator is still answered after its method has returned
(`testClosureActivation`, on a block `seven` returned), and a block evaluated
directly in its method has `sender = enclosingActivation`
(`testEnclosingActivationEquality`). Once the collection iterators are
Newspeak loops rather than forwarders to a leaf ([core-library.md](core-library.md),
"Planned: real bodies for control flow and `do:`, before the JIT tiers"),
every block one runs is such an entry.

The alternative — reify nothing for a block without `^`, carry only what it
captured, as the JVM does (a lambda holds its captured values and no frame;
JDWP shows only those) — was weighed and rejected: it saves one frame-arena
entry per block created, not per block run, and it would have weakened
`enclosingActivation` to `nil` or to a stand-in without identity. A variant that
kept the link only while a debugging flag was set was rejected too: a closure
created without it, including every closure in an image serialized from a run
without it, could never be given it back.

What a returned converted frame keeps is its receiver, its program counter, its
`home` and its `uncontinuable` flag, but no registers: a variable nothing
captured is unreachable once its activation returns, and a captured one lives
in a closure or a vector. So what a surviving block retains is what it captured
plus that frame record, not the frame's registers. An activation that created a
fallback closure is the exception and keeps them, which is exactly what
`outer_reach` decides ("Converted representation" below).

The link costs `ClosureData` nothing it did not already hold, and the home is
no longer a field of its own: `Heap::closure_home` derives it from
`captured_frame` — that frame itself when it is a method's, an initializer's or
a superclass clause's, and that frame's own `home` when it is a block's. A
decoded frame can be missing its `home` (the decoder does not require one), so
`closure_home` falls back to the captured frame rather than asserting: a
decoded frame is never married, so `raise_return` treats it as a dead home and
raises `cannotReturn` either way, where an assertion would have aborted the
process on an untrusted image ("An image is untrusted input", `CLAUDE.md`).

### Where the analysis lives

The capture analysis is a pre-pass of the optimized lowering,
`compiler/capture.rs`, not a resolver pass. The resolver counts every block
literal as a scope and cannot tell which blocks `compiler/control_flow.rs`
inlines; a variable read only in an inlined `ifTrue:` block is not captured,
and the loop rule below depends on knowing which blocks are inlined loops. The
pass calls `control_flow::recognize`, so it and the lowering agree on every
block. Neither the resolver nor the reference lowering knows about it.

`analyze(RootBody, &bindings) -> Captures` walks one code family — a root body
(method, initializer, superclass clause or clause message) and every block
literal under it — and answers, per body, keyed by `BodyKey::Root` or
`BodyKey::Block(Span)`:

- **each local's place** (`Place`): `Register`, whether it is captured by
  copy or not captured at all, or `Vector(slot)`, a slot of the body's own
  indirection vector;
- **each real block's copied values** (`Captured`), **in first-reference
  order**: each entry — a copied variable (`Captured::Variable`) or a scope's
  vector (`Captured::Vector`) — takes its place the first time the walk needs
  it, and the two kinds interleave rather than grouping (`Walker::needs`, and
  `finish`, which deduplicates while keeping that order). That order is what
  the block's register layout is built from, so it is part of the answer, not a
  presentation detail. What a block needs includes, **transitively**, whatever
  a block nested in it needs from further out — a middle block carries what an
  inner one needs, as in Cog — and a block that only *writes* an enclosing
  variable needs that scope's vector just as a reading one does, as does every
  real block between the two
  (`goldens/capture/a_nested_block_that_only_writes.ns`);
- **which locals an inlinable branch of the body assigns**
  (`branch_assigned`), which is what "Fewer protective copies" below reads.

**The rule.** Parameters and immutable locals (`=`) are always copied: nothing
but their initialization writes them (§3.6). A mutable local (`| x |`,
`x ::= …`) goes into the vector when

1. a real block nested in its scope writes it, or
2. its own scope writes it after a real block has read it, in the lowering's
   textual order, or
3. its own scope writes it inside an inlined loop — the body or the condition
   of a `whileTrue:` or `whileFalse:` the compiler inlines — that also contains
   the reading block literal, even when the write is textually before it: the
   next iteration's write happens after this iteration's block was created.
4. its own scope writes it inside a block of an inlined `ifTrue:`, `ifFalse:`,
   `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` or `or:` — outside any real
   block — and a real block reads it anywhere in the scope: such a write
   counts as after every capture, whatever its position ("Fallback blocks",
   below).

Rules 1 and 2 are Primordial Soup's (`ClosureRewriter1`'s comment and
`localWriteNode:`). Rule 3 is what ns-next's inlining adds: its inlined blocks
take no temporaries (`lower_inline_block_body`'s gate), so every variable a loop
writes is its method's, and textual order alone would let
`[i < 3] whileTrue: [x:: i. blocks add: [x]. i:: i + 1]` copy `x` and leave the
first block answering the first `x` instead of the last. Every other captured
mutable local is copied — the common "declare, set once, then capture" pattern
costs no vector.

### Fallback blocks

An inlined `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:`
or `or:` keeps a run-time guard whose failure path sends the real message with
real closures (`emit_guarded_branch`, whose `on_fallback` lowers the block
literals through `lower_block_literal`), so that inlining stays a pure
optimization (`goldens/language/control_flow_fallback.ns`). The same block
literal is lowered twice: inlined on the common path, a real closure on the
fallback path. An inlined `whileTrue:` or `whileFalse:` is not affected: its
fallback is `mustBeBoolean`, with no closure.

A fallback closure can outlive the statement that made it — a non-Boolean
receiver's `ifTrue:` may keep the block and run it later — so treating it as a
converted block would put every variable written in any `ifTrue:` branch into a
vector (rule 1), captured or not. Instead **a fallback closure, and every block
nested in it, keeps the by-reference model**: it reads and writes through
`LoadOuterLocal`/`StoreOuterLocal`, reaching the registers of the frame whose
common path it stands in for — that frame's own locals, its copied-value
registers, and, for a vector variable, its vector register followed by
`LoadVector`/`StoreVector`. Because such a closure can write a local of that
frame at any later time, rule 4 counts its writes as after every capture: a
variable written in an inlinable branch goes into the vector only when a real
block also captures it. The common path pays nothing; the fallback pays only
when it runs.

`lower_fallback_send` is the one place this is decided: it sets the `Lowerer`'s
`blocks_by_reference` around the fallback send, so every block literal lowered
underneath it — the fallback closure itself and anything nested in it — gets a
by-reference `Layout` (no copied values, no vector, every local in the register
its resolver index names) instead of a converted one. So the two lowerings of
one literal differ in more than where they sit:
`a_real_block_nested_in_an_inlined_block_is_converted_and_its_fallback_copy_is_not`
(`compiler/lower.rs`) pins both halves of one literal, the converted one
reading a copied register and the fallback one a `LoadOuterLocal` at depth 2.

Rejected: converting fallback closures like any other block (a vector for every
variable any branch writes), and sending `mustBeBoolean` on the fallback path
as Cog does, which would change recorded outcomes and make inlining observable.

### Converted representation

**The vector** is an ordinary `ObjectData::Array` of class `Array`, as Cog's
indirection vector is. No new object kind: the heap, `trace.rs` and the image
already handle it, and Newspeak code never receives it as a value, since only
the new instructions touch it. One vector per body that has vector variables,
not a box per variable — the Smalltalk and Newspeak lineage's choice (Cog,
Primordial Soup); Strongtalk, V8 and .NET instead keep one context per scope
holding every captured variable, read or written, and Lua, CPython, Kotlin
(`Ref.IntRef`/`ObjectRef`), Scala and Groovy box per variable. In today's arena
a box would be one allocation and one dependent load cheaper for a single
variable, because an `Array`'s elements sit in a `Vec`; once objects are laid
out inline by the collector (step 5), both cost one allocation for one variable
and the vector saves `k − 1` for `k`. A small-vector inline representation was
considered and not built.

**Registers.** A method's or initializer's layout is parameters, temporaries,
then one hidden vector register when the body has vector variables, then
scratch. A block's is parameters, **copied values**, temporaries, the hidden
vector register, scratch. A vector variable keeps its resolver index, whose
register then simply stays `nil`, so every `Binding` stays valid. `Layout`
(`compiler/lower.rs`) is where that arithmetic lives — `local_register` pushes
every temporary up by the copied count, `register_holding` answers the register
a given `Captured` entry sits in, and `locate` turns a resolver index into a
`Location` in one frame, which `access_through` lifts into an `Access` by
adding the reading body's distance from that frame.

**Instructions**, emitted by the optimized lowering only:

- `MakeVector { destination, size }` at the start of the body, before any local
  initializer runs, since `| x ::= 3. |` on a vector variable writes the vector.
  A parameter is never a vector variable, so, unlike Cog, nothing is copied into
  the vector on entry. It is instruction 0 and sits outside every
  `statement_program_counter` range, as the terminating return already does:
  a range starts at the instruction count the lowering has already reached, so
  the body's first statement starts at 1, and `disasm` prints `-` where it
  prints a statement number for every instruction a statement owns.
- `LoadVector { destination, vector, index }` and
  `StoreVector { vector, index, source }`: a vector variable's read and write —
  the method through its own vector register, a block through its copy of it.
- `MakeClosure` carries `copied: Vec<u16>`, the registers whose values go into
  the closure. The reference lowering always emits an empty list, and so does
  the optimized one for a fallback closure.

The optimized lowering emits `LoadOuterLocal` and `StoreOuterLocal` only inside
fallback closures ("Fallback blocks"); everywhere else they remain the
reference lowering's.

**Activating a block** writes the closure's copied values into the registers
after its parameters (`Heap::fill_copied_values`, called by `Step::Block` right
after the push), as Cog and Primordial Soup do ("Copied values are included
at the beginning of temporaries"). A block then reads a copied variable as a
plain register, with no instruction at all. Those registers are read-only by
construction: the analysis puts every variable anything writes after capture
into a vector. `self`, `current_class` and `^` go through `captured_frame` as
before. The fill allocates nothing — the values go straight from the object
arena into the register window — and it stops at the end of the activation's
own window, so a crafted image whose `copied` list is longer than the register
file writes nothing above it.

**`ClosureData` is 32 bytes**, so that an `Object` stays at 56 in a
release build (`heap.rs`'s pinned size, the figure `CLAUDE.md`'s amplification
paragraph is computed from). It carries `copied: Box<[Value]>` (16 bytes; an
empty one allocates nothing) in place of two fields that duplicated something
already held: `parameters`, whose only use was its length, which the block
code's `argument_count` already is, and which `Heap::closure_parameter_count`
now reads from there; and `home`, which the captured frame determines
(`Heap::closure_home`, above). The trace impl names `copied`, which the
compiler enforces.

**The divorce.** Whether a returning frame keeps its registers is decided per
activation, at run time. `CompiledCode` carries `outer_reach`, a **depth**, not
a flag: `bytecode::outer_reach_of` takes the maximum of the body's own
`LoadOuterLocal`/`StoreOuterLocal` depths and of each nested `MakeClosure`'s
block reach minus one. So it is 0 for converted code, 1 for a body that creates
a fallback closure reading its own registers, and one more per level above
that. It is computed when the code is built, by the compiler and by the decoder
alike, and is not on the wire.

`MakeClosure` calls `Heap::retain_registers` on the creating activation when
the block it creates has a non-zero reach, and `pop_activation` copies a
reified frame's registers out only for an entry so marked. A depth rather than
a flag is what keeps the chain unbroken through nesting: a closure that reaches
two frames out gives the body that creates it a reach of 1, so that body's own
creation marks *its* activation too, and so on down to the innermost. Under the
reference lowering every block reads something outer at depth ≥ 1, so every
activation that creates a closure is marked and every frame is divorced with
its registers, exactly as before; under the optimized lowering only an
activation that actually created a fallback closure is.

**An untrusted image** can put any value where code expects a vector.
`LoadVector` and `StoreVector` check the value's kind and the index and raise a
run-time error, not a panic, on a mismatch (`interpreter::locals::load_vector`
and `store_vector`, whose error names `indirectionVector`); code the compiler
produced never has one.

### The image format

Tags follow "assigned once, never renumbered or reused"
(`crates/ns-runtime/src/image/opcodes.rs`, pinned by
`crates/ns-runtime/tests/opcode_tags.rs`):

- `MakeClosure`'s payload changed, so tag 11 is **retired** and `MakeClosure`
  is 29; `MakeVector`, `LoadVector` and `StoreVector` are 30, 31 and 32.
  `IntegerBinary` ("What leaf inlining moved", above) took the next number,
  33, and the next free instruction number is now 34.
- A closure's object record carries its copied values and no longer its
  parameter names or home, so `OBJECT_CLOSURE` 4 is retired
  (`RETIRED_OBJECT_BODIES`, the first retired body tag) and the record is 6.

A code record carries `copied_count: u16`, so that a block's register layout is
known from its code alone: `Heap::fill_copied_values` sizes and fills the
window between a block's parameters and its temporaries from it, and the
decoder reconciles every `MakeClosure` and every closure record against it
without needing the creating instruction in hand. `outer_reach`
is *not* on the wire: the decoder derives it with the compiler's own
`outer_reach_of`, which works because a block's code precedes its parent's in
the table.

The decoder's verification, before any decoded code runs:
- every register operand of the three new instructions is within the body's
  `register_count` (`own_registers`, which also lists a `MakeClosure`'s copied
  registers beside its destination);
- a `MakeClosure`'s `copied.len()` equals its block's `copied_count`
  (`CopiedCountMismatch`, raised from `lexical_parents`);
- a closure record's copied values number its code's `copied_count` (the same
  error, raised from `decode_closure_record`, before any value is read);
- `argument_count + copied_count` does not exceed `register_count`
  (`CopiedCountOutOfRange`, in `u32` arithmetic in `decode_code`, asked only
  once `argument_count <= register_count` so that a body whose arity alone
  overruns its register file is still reported as `ArgumentsExceedRegisters`).

Each of those counts sizes its `Vec` through `reserve_for`, which caps the
capacity at the bytes left in the reader — the per-field pattern the format
already uses, so a claimed count allocates in proportion to the input rather
than to a `u16` ([image-format.md](image-format.md), "Verification";
`bounded_count` is what divides the *remainder* between the graph section's two
record kinds, which these fields are not).

A vector's size and kind are run-time values and are checked at run time
(above). `disasm` prints the three new instructions and the registers a
`MakeClosure` copies, through `bytecode::disassemble`, which `inspect` uses
too; `inspect`'s own structural walk reads a closure record's copied values
from the same tags as the encoder and decoder. A
returned converted frame is written with empty `arguments` and `temporaries`,
which is why `verify_graph` accepts a frame with no registers at all; the frame
record itself did not change.

### Fewer protective copies

`materialize` (`compiler/lower.rs`) copies a named mutable local into a scratch
register when something lowered before its consumer may write it. That question
used to be asked per expression class, and **every send** counted as such a
write, because the code it runs could write the local through a closure
(`CLAUDE.md`, "An expression's value register must still hold that value").
Under the optimized lowering a converted closure writes no register of another
activation: a captured written variable lives in a vector, and a vector read
lands in a scratch register as a snapshot; a copied register is read-only. The
one exception is a fallback closure ("Fallback blocks"), which writes by
reference exactly the locals its inlinable branch assigns. A local's register
can therefore be written only by an assignment in its own body, including inside
an inlined block, or — for a local some inlinable branch of the body assigns —
by a fallback closure that any send may run.

So the question is now asked **per register**, not per expression class.
`a_send_can_write(register)` is the whole of "a send runs user code that can
write this local": true only for a named local of this body that the analysis's
`branch_assigned` marks and whose `Place` is a `Register`. In a by-reference
body — a fallback closure, or a real block nested in one — it is true for every
writable register, the conservative branch, since every closure nested in such
a body reaches this frame by reference. `writes_register(expression, register,
inlined_depth)` walks the expression without descending into real block
literals: an assignment writes the register when its value expression does,
when its target resolves to that register in this frame, or when its target is
a setter send and `a_send_can_write`; a send or a cascade writes it when
`a_send_can_write` or when its receiver or any message's arguments do; a
recognized inlinable form walks its receiver and its blocks' statements
instead. An assignment through a setter copies its value only under the same
condition.

`inlined_depth` is the one subtle parameter. The resolver counts every block
literal as a scope, including one an inlinable form inlines, but the matching
`ScopeKind::Inlined` is pushed only while that block is lowered — after
`lower_operand` asks the question — so the walk increments `inlined_depth` as
it descends into an inlined block's statements and subtracts it from the
binding's depth before resolving. It cannot underflow: the inlining gate admits
no block with parameters or temporaries, so no binding inside an inlined block
names that block's own scope. Getting it wrong is a missed copy, which is
silent wrong output at exit status 0, so it is pinned two levels deep both as
lowering tests and as a golden (`goldens/capture/writes_two_inlined_levels_deep.ns`,
whose two loop-only methods cannot be rescued by `branch_assigned`).

`lower_assign`'s `debug_assert!`s are unchanged, and `check_writable` carries
both of them — this frame's own `read_only_locals` table for a register write,
the target frame's for a `StoreOuterLocal` — while skipping a vector slot,
which is writable by construction because only a variable something writes is
ever put in a vector. The invariant in `CLAUDE.md` and
[compiler.md](compiler.md) keeps its statement and has this as its
justification. The reference lowering is unaffected. A mistake here is silent
wrong output with exit status 0; the reference lowering is the oracle for it,
and `goldens/capture/a_fallback_closure_writes_a_local_a_send_reads.ns` pins the
one shape `a_send_can_write` answers `true` for at run time — a register-resident
local an inlined `ifTrue:` assigns, whose fallback closure a later send runs — in
each of the three positions that read such a local across a send: as an operand,
as an argument and as a setter assignment's value.

The goldens `goldens/language/` holds for the same invariant —
`operands_that_mutate_each_other_keep_their_evaluated_values.ns` and
`assignment_through_a_setter_answers_the_value_before_the_setter_ran.ns` — do
*not* cover it, which is why that one was added: the first writes the local
from the same body (`writes_register`'s assignment case, never
`a_send_can_write`), and the second writes it from a real block, which after
conversion makes it a vector variable whose read is already a snapshot.

### Verification and the reference lowering

The reference lowering stays on the by-reference model, so that a defect in the
capture analysis — rule 3 above all — shows as the two lowerings disagreeing,
as a cache defect shows against the uncached reference since 3b. It is an
interim oracle: **the reference lowering is to be removed at some point, and
from then on the recorded golden outcomes are how this is tested.** The goldens
are written with that in mind: `goldens/capture/` was recorded on the
unconverted `master`, before any of this existed, and covers copied and vector
variables; a write before capture, after it, inside a `whileTrue:`, a
`whileFalse:` and a loop condition; a block that writes; nested blocks three
deep that carry a variable through a middle block; a variable read only in an
inlined `ifTrue:`; a vector variable with an initializer; an initializer body
and a slot initializer creating blocks; `^` from a block whose home has
captured variables; a closure surviving its method; a non-Boolean receiver
whose `ifTrue:` keeps its fallback closure and runs it later — reading and
writing a variable another block captured, from inside a method, a block and an
outer block that has returned; and the protective-copy cases above. Each
recorded output was checked against the semantics by hand before it was taken.

Two goldens were added while building, for defects the recorded set did not
cover: `a_nested_block_that_only_writes.ns`, for a block that only *assigns* an
enclosing scope's variable and the middle blocks that have to carry that
scope's vector to it — the capture analysis did not record such a need, and the
shapes failed to compile until it did — and `writes_two_inlined_levels_deep.ns`,
for the `inlined_depth` arithmetic above.

Four more were added once it was built, for what the recorded set had covered
only as lowering tests — which pin what the compiler emits, not what a program
answers, and so would not survive the reference lowering's removal:

- `a_fallback_closure_writes_a_local_a_send_reads.ns`, the run-time case
  `a_send_can_write` exists for ("Fewer protective copies" above);
- `a_block_in_a_fallback_closure_outlives_its_method.ns`, a real block *inside*
  a fallback closure reading a method local two frames out, evaluated after the
  method has returned: what `outer_reach_of`'s folding of a nested
  `MakeClosure` is for, and the only golden in which the method's frame is
  marked by nothing but that folding;
- `a_blocks_own_registers_and_vector.ns`, a block whose layout has all three
  summands of `vector_register = arguments + copied + temporaries` non-zero,
  and rule 3 applied to a block's own local rather than a method's;
- `and_or_fallback_closures_share_variables.ns`, the fallback closure of an
  `and:` and of an `or:`, which only `ifTrue:` had stood for.

### Cost

What it changed, for a method that creates a non-inlined block:

| | By reference (the reference lowering, and a fallback closure) | Converted (optimized lowering) |
|---|---|---|
| Per activation of a body that creates a block | a reified frame, plus a copy of every register at return | a reified frame; the closure's copied values; and, when the body has vector variables at all, one vector — `MakeVector` is the body's first instruction, so that allocation happens on every activation of it, block created or not |
| A block reading a captured variable | one hop per nesting level, then a live-or-dead branch | a register if copied, a register then the vector if written; independent of depth |
| The method reading its own captured variable | a register | a register if copied; through the vector if written |
| What a surviving block retains | the whole frame, including registers it never reads | what it captured, plus the frame record without registers |
| A later JIT | every local a block can reach stays in memory once a block exists | copied values are ordinary values; escape analysis can remove a vector |

**What that was worth, measured: nothing either way on speed, and about 2.6%
more peak working set on `bench_block.ns`.** The design expected the
interpreter to be no slower and the memory to fall; the first held and the
second did not. "What closure conversion moved", in the measurements section
above, is the measurement, with the reasoning for why the memory moved the way
it did and how far that reasoning is evidence.

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
- **Only by-reference code keeps a frame's registers alive.** The reference
  lowering and a fallback closure reach their method's variables by reference,
  and a reified frame keeps its registers when its activation returns only when
  a block it created can do that (`outer_reach`, "Converted
  representation"); converted code copies or shares a vector instead. The
  reification mechanism itself is permanent: `^` and the debugger need it with
  or without conversion.
- **Sends run on the activation stack, not the Rust stack**: one loop pushes,
  pops and resumes, and it pops everything it runs on every path out.
- **Control flow answers trampoline steps and depth has one bound**,
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
- **Closure conversion keeps the defining activation** (decided 2026-09-19,
  "Closure conversion"): a closure keeps `captured_frame` and only
  variable access moves to copies and vectors, because Newspeak's
  `enclosingActivation` and the debugger's "`[] in Foo bar`" need the link;
  dropping it for blocks without `^`, as the JVM does, saves one reification
  per block created and was rejected, as was keeping it only under a
  debugging flag, which a closure created without it could never recover.
- **A captured mutable variable is copied unless it is written after
  capture** (decided 2026-09-19): Primordial Soup's rule, plus a rule for
  writes inside an inlined loop that contains the capturing block — the
  performance-oriented choice over boxing every captured variable that is
  written anywhere, and made more important by step 6, which runs the core
  library's blocks per element.
- **One indirection vector per scope, as an ordinary `Array`** (decided
  2026-09-19), as in Cog and Primordial Soup, rather than a box per variable;
  no inline small-vector representation for now.
- **The capture analysis is the optimized lowering's**, not the resolver's
  (2026-09-19): only the compiler knows which blocks are inlined.
- **The reference lowering keeps the by-reference model as an interim
  oracle** (decided 2026-09-19): it is to be removed at some point, and from
  then on the recorded golden outcomes are how closure conversion, and
  everything else, is tested.
- **A fallback closure keeps the by-reference model** (decided 2026-09-19,
  "Fallback blocks"): the real closure an inlined `ifTrue:`, `and:` or `or:`
  sends when its receiver is not a Boolean reads and writes its frame's
  registers by reference, its writes count as after every capture, and the
  divorce copy is decided per activation by whether it created such a
  closure — so the common path pays nothing and inlining stays unobservable.
