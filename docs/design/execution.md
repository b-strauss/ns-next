# Execution — activations, the VM

> **Status:** built, including the trampoline; nothing re-enters the run loop
> from Rust.
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> arena, the `ActivationRef` and `FrameRef` handles, every read and write of an
> activation), `types.rs` (`Frame`, `Continuation` and the state its variants
> carry), `vm.rs` (the run loop), `interpreter/` (the send decisions, `Step`,
> every push, the depth limit, the host entry), `builtins/boolean.rs`,
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
every `tests-fixtures/*.ns` against its `.expected` stdout and an empty stderr
(`crates/ns-cli/tests/run_fixtures.rs`), the golden exit status, stdout and
stderr of the programs that cannot be fixtures
(`crates/ns-cli/tests/program_outcomes.rs`), and `run-corpus` over the runnable
corpus. The answers recorded before the retirement are verdicts the oracle and
the VM agreed on. A change that alters one is a behaviour change, to be
justified as one — never recorded because the code now produces it. The
reference lowering ([compiler.md](compiler.md), "The reference
lowering") is the differential oracle that replaced it.

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
(`tests-fixtures/closure_variables.ns` pins this). So a reified frame here keeps
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
    current_class: Option<ClassRef>,
    kind: FrameKind,           // Closure(Rc<ClosureData>) carries home and captured_frame
    base: u32,                 // first register in `values`
    num_args: u32,
    pc: usize,
    slot_base: usize,
    code: Rc<CompiledCode>,    // the body it runs, which the loop resumes it in
    then: Continuation,        // what the loop does with its value
    counted: bool,             // whether it counts against the depth limit
    reified: Option<FrameRef>, // set once the activation is married
}

struct FrameSlot {             // one entry of the frame arena
    frame: Frame,              // types.rs; public fields, and what an image records
    live: Option<u32>,         // the stack entry this frame is married to
}

pub struct ActivationRef {
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
  function, `Interpreter::push`, which refuses a push past the depth limit ("The
  depth limit" below) before it reaches the heap, so no new push site can skip
  the check. It is called from one place, `Interpreter::push_step`, for the
  method, block or body a decision answered. An activation nothing reifies
  leaves nothing behind; only a block the compiler did not inline reifies one,
  since an inlined `ifTrue:`/`and:`/`whileTrue:` argument creates no closure.
- **Register access through an `ActivationRef`** is `values[entries[i].base +
  r]`, with no branch on the handle's kind or on an args/temps split; a release
  build still bounds-checks both slice indexes. A debug build also checks that
  the register lies inside the entry's own window: the neighbouring
  activation's register is in bounds of `values`, so reading it would not
  otherwise fail. An entry's `base` is fixed for its whole life and everything
  is addressed by index, so `values` reallocating as it grows moves nothing
  anyone holds.
- **Two handle types, not one tagged handle.** `ActivationRef` names a live
  stack entry; `FrameRef` names a reified frame in the arena and is the only
  activation handle anything *stores*: `ClosureData.captured_frame` and `home`,
  `Frame.home` and every image record take a `FrameRef`, so storing a stack
  handle is a compile error rather than a convention. A tagged `FrameRef` would
  have touched only `heap.rs`, but it would put a branch on every register
  access and leave "a stack handle was stored" to a debug-build assertion — the
  silent failure class the arena's private index was built to exclude.
- **An `ActivationRef` is 4 bytes in a release build.** In a debug build it
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
  `live` is set, the frame's own `args`/`temps` are empty and a register read or
  write through the `FrameRef` (`Heap::frame_local`/`frame_set_local`) reaches the
  stack slot. That branch exists only on the path from a block to its method's
  variables — `read_outer_local`/`write_outer_local`, behind the VM's
  `LoadOuterLocal`/`StoreOuterLocal`.
- **The accessor prefix names the handle.** `activation_*` reads or writes a live
  activation through an `ActivationRef`; `frame_*` a reified frame through a
  `FrameRef`.

### Lifecycle

**Reify.** `Heap::reify(act) -> FrameRef` is idempotent: the first call creates
the arena `Frame` — receiver, `kind`, `current_class`, `pc`, and `home` when the
activation is a closure's — marries it to the entry, and records it in
`entry.reified`; every later call answers the same handle. Its caller is closure
creation, the VM's `MakeClosure`: directly for the new closure's
`captured_frame`, and through `home_of` for its `home`. `home_of` passes a
closure activation's own `home` on and reifies any other activation as its own
home. Every block creation reifies the activation that creates it, whether or
not the block reads one of its variables, because `captured_frame` is also where
`Interpreter::push_step` reads the block's receiver and current class from.

**Pop.** The run loop pops every activation it runs, on *every* exit, error
paths included (`vm::run`), and only the top entry can be popped. Popping hands
back the entry's continuation (`Popped`), which the loop applies to the
activation's value ("The VM" below). If the entry was reified, its frame is
**divorced**: the registers are copied into the frame's `args`/`temps`, its `pc`
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

**Only a `FrameRef` is ever stored.** An `ActivationRef` is valid until its entry
is popped, and the next activation at that depth reuses the index. Everything that
outlives a call — a closure, another frame, an image record — holds a `FrameRef`.

### Signatures

- Wherever a function means *the running activation* it takes an
  `ActivationRef`: `vm::run` and the loop's helpers, `sends::decide_lexical_send`,
  `sends::decide_super_send`, `read_local`/`write_local`,
  `read_outer_local`/`write_outer_local`, `raise_return`.
  `FrameRef` stays for closures, `Frame.home`, the image, and `frame_at_depth`
  from depth 1 on: the first hop goes from a closure activation to its
  `captured_frame`, the rest follow `FrameRef` links.
- **No send or block invocation takes a sender to record.** `Interpreter::send`,
  `decide_send`, `builtins::dispatch_native` and the `builtins/` modules have
  no `sender` parameter. It was removed rather than retyped to an `ActivationRef`: its only
  use was to fill `Frame.sender`, and once a live activation's sender is the
  entry below it, nothing read it. No native takes an activation handle at all.
  The one function handed the sending activation is `Interpreter::push_step`,
  which reads its reified frame for the depth count's exemption and stores
  nothing.
- `vm::run` receives the `ActivationRef` of an activation its caller has just
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
`temps` is a growable `Vec` (`Heap::frame_grow_temps`). Adding a local slot to a
running method — growing its register file, initializing the new entries to nil
and remapping the `pc`, the edit-and-continue requirement — therefore means
taking the activation off the stack first; see "How this meets the liveness
requirements" below, which is not built.

`kind` distinguishes a method, closure, initializer, superclass-clause,
superclass-message and top-level activation. An initializer activation must be
able to name the code it is executing so that `pc` is interpretable, since the
statement→PC table is reached through its kind. A stack entry also holds its
`code` directly, because the loop resumes a caller in it after a callee returns;
`then`, the `Continuation` the loop applies to its value when it is popped; and
`counted`, whether it counts against the depth limit ("The depth limit" below).

`pc` is an instruction index into the activation's compiled body.

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

`vm::run(interp, base)` is the run loop. It is handed an activation its caller
has just pushed — its *base* — and runs it and everything it activates, one
instruction at a time (`execute`, a `match` over `Instr`), with the running
activation, its code and its `pc` in locals. It never recurses: a send pushes
the callee and switches to it, a return pops the callee and switches back to
the caller. It pops everything it ran, `base` included, on every path out — a
return from `base`, an error, or a non-local return aimed below `base` — so the
stack is exactly as deep afterwards as it was before `base` was pushed. The
depth `base` sits at, the run's *floor*, is how the loop tells its own
activations from its callers'.

- **A send** (`vm::send`) gathers its arguments from the caller's registers
  into the interpreter's reusable buffer, stores the caller's `pc`, and asks
  `interpreter/` what the send means: `Interpreter::decide_send`,
  `sends::decide_lexical_send` or `sends::decide_super_send`, which answer a
  `Step`. `Step::Value` — a native leaf, a slot access, a nested class or an
  instantiation that runs no code — is written to the send's `dst`,
  and the caller goes on. Anything else is an activation: `Step::Method`,
  `Step::Block`, or one of the bodies an instantiation or a nested-class
  accessor runs, `Step::Initializer`, `Step::InitializerThen`,
  `Step::ClauseMessage` and `Step::SuperclassClause`. It is
  pushed by `Interpreter::push_step`, which checks a method's or block's arity
  and the depth limit, and the loop switches to the new activation at `pc` 0,
  its window filled straight from the buffer. A send allocates no vector once
  the buffer has grown to the widest one.
- **A return** (`vm::finish`) pops the activation and applies its
  continuation. `Continuation::Deliver`, every send's, hands the value to the
  caller: `resume_caller` reads the `pc` the caller stored at its send, writes
  the value to that send's `dst` and continues at the next instruction — or,
  when the popped activation was `base`, the loop leaves `run` with it. Every
  other continuation — a control-flow native's, `do:`'s, the nested-class
  accessor's, an instantiation's — is applied by `Interpreter::resume`, which
  calls into `builtins/` or `interpreter/`; it may answer a value, or another
  activation — a native loop's next block, `do:`'s block for the next element,
  an instantiation's next body, or the method a non-Boolean condition's
  `mustBeBoolean` send finds — which `Interpreter::push_continuation` pushes
  in the popped activation's place ("The trampoline" below).
- **A signal** — a Newspeak error, or a non-local return — goes to
  `vm::unwind`, which pops what it has to and either returns from the
  non-local return's target or leaves `run` with the signal ("Non-local
  return" below). Every activation it pops hands its continuation to
  `Interpreter::abandon`, which undoes what that continuation had begun.

Four kinds of body run, and each reaches the loop one way:

1. **Method body** — pushed by `push_step` for a `Step::Method`. A method
   activation catches a `^` that targets it (`Interpreter::catches_return`).
2. **Block body** — pushed by `push_step` for a `Step::Block`, running
   `ClosureData.code`. `value`, the control-flow natives and `do:` answer the
   block as a step.
3. **Instance initializer** — `descend_levels`, one per initializing level of
   the superclass chain, answers that level's compiled initializer: its own
   slot initializers and initializer expressions. While a level below it still
   has code to run, it is answered as a `Step::InitializerThen`, whose
   activation continues with `Continuation::Instantiate`; the walk's last
   initializer is a `Step::Initializer`, whose activation delivers its own
   value, the instance ("No re-entry from Rust" below). A class with no
   compiled initializer and no slots is a no-op; a user class whose mixin
   carries no initializer is refused on load and panics if built any other way
   (`Interpreter::initializer_body`).
4. **Superclass clause and its message arguments** — the nested-class
   accessor's clause is a `Step::SuperclassClause`, continuing with
   `Continuation::NestedClass`; an instantiation's clause message is a
   `Step::ClauseMessage`, continuing with `Continuation::Instantiate`.

Rust enters the loop in one place, `Interpreter::run_from_host`, for a host's
`send` or `instantiate`, and only from outside it: it pushes what the decision
answered and runs `vm::run` on it. No decision, native or continuation runs
code from Rust, and a debug build checks that the loop is never entered while
it runs ("No re-entry from Rust" below).

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** It pushes, pops, switches, writes
registers and reads instructions' operands; which body runs next is always
decided in `interpreter/` or `builtins/`, a native loop's next block included.
Every error a program can observe is produced there. That keeps dispatch in
one place, which the reference lowering and a later JIT will reach unchanged —
and it is why `CLAUDE.md`'s "`vm.rs` holds no dispatch logic" forbids copying
anything out of `interpreter/` into `vm.rs`.

Block invocation is not special-cased into an opcode: `Block value` is still a
native method, which answers the closure as a step; the closure's activation is
pushed above the one that sent `value` — its sender, by position — and a wrong
argument count is still the same error.

## Where `pc` is stored

An activation's `pc` is written **only by the instructions that can hand the
activation to an observer**: every send, since a nested activation runs above it;
`MakeClosure`, since the activation escapes into the object graph — the store
comes just before `reify`, which copies it into the frame it creates; and
`NonLocalReturn`, since control leaves with a signal.

The stores go to the stack entry (`Heap::set_activation_pc`), so they keep the
activation's `pc` current for as long as it runs; a frame married to it holds no
`pc` of its own that anything reads meanwhile. When the activation returns, the
divorce gives the frame the last `pc` stored — often a later send's, not the
`MakeClosure` that reified it.

The loop itself reads it back. While a callee runs, the caller's only record of
where it stopped is the `pc` its send stored; when the callee returns,
`vm::resume_caller` reads that `pc` (`Heap::activation_pc`), writes the value to
the destination register of the send it names, and resumes the caller at the
next instruction.

Nothing can read `pc` while a straight-line instruction executes, because the
loop never switches activations between them. Keeping the store out of the
straight-line arms takes a write off the hot path for a value nothing observes
there.

Whether a debugger will force `pc` at every instruction is an open question for
the milestone that builds one; it is a deliberate trade, not an oversight.

## Non-local return

A closure's `home` is the reified frame of the method, initializer,
superclass-clause or superclass-message activation that lexically encloses it.
`Signal::Return` carries an `ActivationRef`, its target, and `raise_return` picks
it: for a closure activation, its closure's `home` — the stack entry that frame is
married to (`Heap::frame_activation`) while its activation is still running, and
`cannotReturn` if it is dead, either divorced because the activation returned or
built by the image decoder, which never marries a frame. For any other activation
the target is the activation itself.

The loop unwinds the activation stack to it (`vm::unwind`). A target that is
still live (`Heap::activation_is_live`), sits at or above the run's floor and
catches a return (`Interpreter::catches_return`: a method, a superclass
clause or its message) ends the unwind: every activation above it is popped,
and the target returns the value through the ordinary return path, so a
reified frame is divorced exactly as on any return. The popped activations'
continuations are not applied, so a `^` out of an `and:` block, a native
loop's body or a `do:` block abandons the coercion or the loop; they go to
`Interpreter::abandon` instead, which unmarks a nested class's creation in its
cache slot and drops every other. Any other signal — an error, or a
return whose target catches none — pops every activation of the run, `base`
included, through `abandon` in the same way, and leaves `vm::run` with it, for
the host that started the run.

Because there is one stack, a live target is always below the activation
raising the return, and nothing is pushed while the signal propagates, so the
target's index cannot have been reused by the time the unwind compares it
(`ActivationRef::same`). In a release build that is also all
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

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvalResult`.** `Signal`
deliberately carries no `Debug` impl of its own -- every field it holds
could support one today, but leaving `Signal` without one means
`EvalResult` (`Result<Value, Signal>`) has none either, and
`Result::unwrap`/`expect` require `E: Debug`. Calling either, or writing
`{:?}` on an `EvalResult`, is therefore a compile error, not a lapse a
reviewer has to catch. Tests unwrap through a local `expect_ok` helper.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping and
compiling excluded — and reports the median of ten timed runs after a discarded
warm-up. Its three fixtures each run a 50,000-iteration `whileTrue:` loop, which
the compiler inlines: `bench_loop.ns` sums the counter, `bench_send.ns` adds a
user-method send per iteration and `bench_block.ns` a method that creates and
invokes a block. On a release build, on 2026-09-13, with nothing re-entering
the loop from Rust, their medians were about 11.1, 14.0 and 23.6 ms ("What
removing the re-entries moved" below).

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
allocations (the `Rc<RefCell<Frame>>` block and the `args` vector `collect`
builds), reference-count traffic on `sender`, `method`, `bindings` and the
defining class, and a `HashMap` lookup per invocation just to find the compiled
body. A method with temporaries also allocated a `temps` vector. Row two's
`^x` has none (`ns-next disasm` gives `tick:` one register, its argument, where
row three's has three), so its `temps` vector stayed empty and never allocated,
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
same `ns-next bench tests-fixtures/bench_loop.ns` — ten timed runs after a
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

What did **not** move was the rest of the activation cost. `Frame.args` and
`Frame.temps` were still a `Vec` allocation apiece on every activation that had
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
binaries run the same native `+`. That is the `temps` allocation described under
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
  | the bounds, marked by `StepArgs::ControlFlow` | 11.2 | 14.1 | 23.8 |

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
self). Both are the lexical-send slot lookup PROGRESS.md lists as left for the
inline caches; a cached getter symbol and a hasher for `u32`-shaped keys would
remove most of it sooner.

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
above. Medians in ms from the first sitting, each group's mean, and the change
of the mean in both sittings:

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
benchmark's path explains it.

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
  executed code, not bisected further.
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
cache slot the check is one read, and `accessor-recursion` in
`crates/ns-cli/tests/program_outcomes.rs` takes 0.3 s again.

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
in `interpreter/` (`Interpreter::decide_send`, `sends::decide_lexical_send`,
`sends::decide_super_send`), unchanged in what they produce, but they answer a
`Step` — a value, a method to activate, or a block to activate and what to do
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
  through `Interpreter::resume`, when it pops the activation, so a loop's
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
  below), and moves to Newspeak after the inline caches
  ([core-library.md](core-library.md)).
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
  `KernelWeakTests.ns` (`deepStack: 1000`) expects. The `deep-stack` golden
  outcome (`crates/ns-cli/tests/program_outcomes.rs`) runs exactly that
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

- **L is checked on every push.** `Interpreter::push`, the one function every
  activation the runtime pushes goes through, refuses a counted push when
  `Heap::counted_depth` has reached the limit. The heap keeps that count as a
  running total on push and pop, so an error path — which pops through
  `vm::unwind` like any other — leaves it exact. Whether an activation counts
  is recorded on its stack entry. Only the block arm of
  `Interpreter::push_step` ever records an exemption; a method, and every body
  an instantiation or a nested-class accessor runs — an initializer level, a
  superclass clause or its message — are counted by construction.
- **The exemption is marked by the arguments.** A block a control-flow native
  activates carries `StepArgs::ControlFlow` — it has no arguments, and only
  those natives build that variant — and `push_step` exempts it when the send
  was a `SendInlinable` and the block's `captured_frame` is the reified frame
  of the activation that sent it ([compiler.md](compiler.md), "How the limit
  counts"). A native loop's next block goes through
  `Interpreter::push_continuation`, which exempts it only if the one it
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
an explicit list, as a state machine `Interpreter::advance_instantiation`
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
stack entry, and `Interpreter::resume` applies it when the activation returns.
The semantics and the order of every observable effect are what they were
when these paths ran in nested runs of the loop.

- **`do:`** (`builtins/collections.rs`) walks a snapshot of the elements. An
  empty collection, or an argument that is not a block, answers the receiver at
  once: a non-block stands for its own value, so handing it the elements would
  observe nothing. Otherwise `do_step` answers `Step::Block` for the first
  element with `Continuation::DoElement`, whose `DoLoop` carries the receiver,
  the block, the snapshot and the next index; `resume` hands it to
  `after_element`, which activates the block for the next element or answers
  the receiver. `dispatch_array` and `dispatch_ordered_collection` check for
  `do:` first and answer its step themselves, and lift every other selector's
  value into a step (`leaf_step`), so `builtins::dispatch_native` keeps one arm
  per class. Each element's activation is counted, like any block a `value`
  send activates; a `^` in it unwinds like any other activation's, and its
  arity is checked by the same `check_block_arity`. Its argument is a
  one-element vector, allocated per element: a `StepArgs` variant that carried
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
  `Interpreter::abandon` sets it back to nil when the activation is popped
  (`nested::end_creation`), before `after_clause` caches the class there. An
  access that finds the marker is the cyclic-definition error; the slot is
  already keyed by the enclosing object and the declaration, the pair a cycle
  is, so the check is the one read the accessor makes anyway. The marker is
  `false`, a primitive the image format already has: nothing but the accessor
  reads a hidden slot, so no Newspeak code sees it, and no image holds one,
  since only a host snapshots, between runs; an image that puts `false` in a
  hidden slot itself gets a cyclic-definition error, the same kind of
  wrong-but-safe answer as any other non-class value it puts there. The reset
  replaces only the marker: the decoder does not bound `InitSlot`'s field
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
  `try_factory_send` makes a factory send an ordinary decision,
  `decide_instantiation`; `Interpreter::instantiate` is the host entry, shaped
  like `send`.

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
(`Interpreter::initializer_body`), and a clause's are its declaration's — so
none needs a boxed description of its activation; boxing one per body, beside
a fresh box for the walk's state, made a chain of two levels with code 15–18%
slower than running it in a nested run of the loop had been
(`bench_new2.ns`, `bench_newarg.ns`). Each is smaller than `Step::Method`, on
purpose: an `Initializer` with an optional continuation, as large as
`Step::Method`, left `Result<Step, Signal>` at the 56 bytes the hot path was
measured at and still made every benchmark 4–6% slower, none of which
instantiates in its loop — the same effect `StepArgs::ControlFlow`'s comment
describes, presumably the discriminant's niche. Splitting it into
`Initializer` and `InitializerThen`, which takes the receiver from its state,
measured level with before. `a_decision_stays_the_size_the_hot_path_was_measured_at`
(`interpreter/mod.rs`) pins the size; it cannot see the niche. Every body is
counted.

**One host entry.** `Interpreter::send` and `Interpreter::instantiate` decide,
and hand what they decided to `Interpreter::run_from_host`, which answers a
value as it is and otherwise pushes the activation and runs `vm::run` on it.
It is the one place Rust enters the loop. A debug build checks there that the
loop is not already running — nothing it calls can reach `run_from_host` —
that the stack is left as deep as it was found, and that no nested class's
creation outlives the run. That the loop is never nested is a debug assertion
now; it becomes structure in step 2c, where nothing a native can reach offers
`run` at all ([memory.md](memory.md), "The brand").

`vm::send` still takes the interpreter's reusable argument buffer and gives it
back, because the decision needs `&mut Interpreter` while it reads the
arguments. No decision runs code any more, so none takes a second, fresh
buffer of its own. `Interpreter::reenter`, `invoke_closure`,
`builtins::call_block`, the count of re-entries and its bound, and that
bound's error, `stack overflow: too many nested native calls`, are gone with
the re-entries.

**Three golden outcomes changed.** `do-recursion`, `instantiation-recursion`
and `accessor-recursion` in `crates/ns-cli/tests/program_outcomes.rs` ended in
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
none of the changed paths in their loops, are 1.4–1.9% slower, an open
finding.

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
performs on every stack page before a snapshot — and a divorced frame's `temps`
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
- **`pc` is stored only at observable points**, not at every instruction — a
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
- **A live activation and a reified frame are two handle types**, `ActivationRef`
  and `FrameRef`, so that storing a stack handle is a compile error and a
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
- **A nested class's creation is marked in its own cache slot** ("No re-entry
  from Rust"), not listed beside the stack: the slot is keyed by exactly the
  pair a cyclic definition is, so the check is the read the accessor makes
  anyway, where the list was scanned in full at every level of a deep chain of
  creations.
