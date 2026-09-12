# Execution — activations, the VM

> **Status:** built, including the trampoline.
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> arena, the `ActivationRef` and `FrameRef` handles, every read and write of an
> activation), `types.rs` (`Frame`, `Continuation`, `WhileLoop`), `vm.rs` (the
> run loop), `interpreter/` (the send decisions, `Step`, every push, the two
> depth bounds, re-entry from Rust), `builtins/boolean.rs` and
> `builtins/block.rs` (the control-flow natives as trampoline steps),
> `crates/ns-cli/src/main.rs` (the interpreter thread and its stack).

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
  trampoline" below) before it reaches the heap, so no new push site can skip
  the check. Three places call it, all in `interpreter/`: `push_step`, for the
  method or block a send decided (and the block `do:` invokes); and, for the
  paths that re-enter the run loop from Rust, `run_own_initializer` and
  `nested::run_clause_body`. An activation nothing reifies leaves nothing
  behind; only a block the compiler did not inline reifies one, since an
  inlined `ifTrue:`/`and:`/`whileTrue:` argument creates no closure.
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
  `Interpreter::send` and `Interpreter::instantiate` debug-assert that they leave
  the stack as deep as they found it, because a leaked entry is otherwise silent:
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
  `decide_send`, `invoke_closure`, `builtins::dispatch_native`,
  `builtins::call_block` and the `builtins/` modules have no `sender`
  parameter. It was removed rather than retyped to an `ActivationRef`: its only
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
`counted`, whether it counts against the depth limit ("The trampoline" below).

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
  `Step`. `Step::Value` — a native leaf, a slot access, a nested class, a
  finished instantiation — is written to the send's `dst`, and the caller goes
  on. `Step::Method` and `Step::Block` are pushed by `Interpreter::push_step`,
  which checks the arity and the depth limit, and the loop switches to the new
  activation at `pc` 0, its window filled straight from the buffer. A send
  allocates no vector once the buffer has grown to the widest one.
- **A return** (`vm::finish`) pops the activation and applies its
  continuation. `Continuation::Deliver`, every send's, hands the value to the
  caller: `resume_caller` reads the `pc` the caller stored at its send, writes
  the value to that send's `dst` and continues at the next instruction — or,
  when the popped activation was `base`, the loop leaves `run` with it. The
  continuations the control-flow natives use are applied by
  `Interpreter::resume`, which calls into `builtins/`; it may answer a value,
  or another activation — a native loop's next block, or the method a
  non-Boolean condition's `mustBeBoolean` send finds — which
  `Interpreter::push_continuation` pushes in the popped activation's place
  ("The trampoline" below).
- **A signal** — a Newspeak error, or a non-local return — goes to
  `vm::unwind`, which pops what it has to and either returns from the
  non-local return's target or leaves `run` with the signal ("Non-local
  return" below).

Four kinds of body run, and each reaches the loop one way:

1. **Method body** — pushed by `push_step` for a `Step::Method`. A method
   activation catches a `^` that targets it (`Interpreter::catches_return`).
2. **Block body** — pushed by `push_step` for a `Step::Block`, running
   `ClosureData.code`. `value` and the control-flow natives answer the block as
   a step; `do:`, the one native that still calls a block from Rust, goes
   through `Interpreter::invoke_closure`, which builds the same step and
   re-enters the loop with it.
3. **Instance initializer** — `run_own_initializer`, one per initializing level
   of `run_initializer_levels`, which runs that level's compiled initializer:
   its own slot initializers and initializer expressions. A class with no
   compiled initializer and no slots is a no-op; a user class whose mixin
   carries no initializer is refused on load and panics if built any other
   way.
4. **Superclass clause and its message arguments** — `nested::run_clause_body`,
   for the nested-class accessor and for `instantiate`'s deferred clause message.

Rust enters the loop in one place, `Interpreter::reenter`: it pushes one
activation, runs `vm::run` on it and counts the re-entry against the
native-depth bound. The host's `Interpreter::send`, `invoke_closure`,
`run_own_initializer` and `run_clause_body` all come through it — the last three
are the paths in "What stays recursive" below. A decision can itself re-enter —
an instantiation, or a nested-class accessor running its clause — and finishes
before the send it decides goes on; it takes a fresh argument buffer, since the
running send's is in use.

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
continuations are not applied, so a `^` out of an `and:` block or a native
loop's body abandons the coercion or the loop. Any other signal — an error, or
a return whose target is below this run — pops every activation of the run,
`base` included, and leaves `vm::run` with it, for the Rust frame that
re-entered the loop to pass on: through `do:`'s native, which passes a
`Signal::Return` through untouched (`builtins::call_block`), through an
instantiation or a nested-class accessor, to the run below, which asks the
same question.

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
no return, so the run it belongs to pops it and leaves with the signal still
naming it, each run below finds the target no longer live and passes the
signal on, and it reaches the top of the entry send as "internal: non-local
return escaped the entry send", a known gap
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
invokes a block. On a release build, on 2026-09-12 with the trampoline, their
medians were about 11.3, 14.0 and 24.0 ms ("What the trampoline moved" below).

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
- **The two bounds.** The first version marked a control-flow native's block
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

The interpreter thread's stack comes from the native cost of one re-entry of
the loop from Rust, measured in a debug build by the address of a local at
native depths 1 and 101: 9,280 bytes a level through `do:`, 8,256 through
instantiation and 6,816 through the nested-class accessor. At the native-depth
bound of 1,000 the largest is about 8.9 MiB; twice that, 17.7 MiB, rounds up to
`INTERPRETER_STACK_BYTES = 32 MiB` (`crates/ns-cli/src/main.rs`). The goldens
that recurse to the bound through `do:` and through instantiation run on it in a
debug build and end in the error rather than an abort.

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

## The trampoline

Sends do not recurse on the Rust stack. Every running activation lives on the
activation stack, and the run loop ("The VM" above) is the only place a running
computation lives, apart from the three paths "What stays recursive" below
names, which re-enter it from Rust. This was step 1b of Milestone 4, and it
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
  lowerings. `do:` is still a callback; it becomes a step of the same kind in
  "Planned: no re-entry from Rust" below, and moves to Newspeak after the
  inline caches ([core-library.md](core-library.md)).
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

### Two bounds, not one

Decided 2026-09-12. Once sends and the control-flow natives no longer recurse,
two resources can still run out, and they are bounded separately, the way Lua
and CPython bound them:

| Bound | Counts | Value | Error text |
|---|---|---|---|
| L | activations, by the rule in [compiler.md](compiler.md), "How the limit counts" | 100,000 | `stack overflow: recursion too deep` |
| N | re-entries of the run loop from Rust — the paths in "What stays recursive" below | 1,000 | `stack overflow: too many nested native calls` |

The two are `MAX_ACTIVATION_DEPTH` and `MAX_NATIVE_DEPTH` in
`crates/ns-runtime/src/interpreter/mod.rs`. L bounds memory: an activation is a
stack entry and a window of registers in two `Vec`s, so L can be far larger
than any native stack would allow. It bounds it up to a factor: above each
counted activation sit at most as many exempt ones as blocks nest — each
exempt block was created by the activation below it, so a chain of them
descends the lexical nesting — and blocks nest at most 128 deep, the parser's
ceiling in source and the decoder's, derived from it, in an artifact
([image-format.md](image-format.md), "Verification"). So the stack holds at
most 129 × L entries, 12.9 million, whose entries alone take about 1.3 GB in a
release build (104 bytes each, before their registers). Only code that carries
`SendInlinable` gets there — the reference lowering's, or an artifact's —
recursing through 128 nested control-flow blocks at every level; the optimized
lowering never emits it, so under it the stack holds at most L entries. N
bounds the native stack, and is the only bound that depends on it. Both agree
under both lowerings, because the control-flow natives are trampoline steps, so
no Rust re-entry happens under one lowering and not the other. Neither error
text names its number, as Lua's,
CPython's, V8's and Nova's do not, so that changing a value later changes no
recorded answer. Both unwind to the entry send like every other Newspeak error;
nothing catches them yet.

- **L is checked on every push.** `Interpreter::push`, the one function every
  activation the runtime pushes goes through, refuses a counted push when
  `Heap::counted_depth` has reached the limit. The heap keeps that count as a
  running total on push and pop, so an error path — which pops through
  `vm::unwind` like any other — leaves it exact. Whether an activation counts
  is recorded on its stack entry. Only the block arm of
  `Interpreter::push_step` ever records an exemption; a method, an
  initializer level (`run_own_initializer`) and a superclass clause or its
  message (`nested::run_clause_body`) are counted by construction.
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
  reason must not build `ControlFlow`. `&` and `|` share `and:`'s and `or:`'s
  native and so carry the mark too, which is harmless: no lowering emits a
  `SendInlinable` for them, since `control_flow::recognize` accepts only the
  eight inlined selectors, and a crafted artifact that marks one meets the
  captured-frame condition like any other.
- **N is checked on every re-entry.** `Interpreter::reenter` refuses one past
  the bound before anything is pushed, and counts only a re-entry whose push
  succeeded.
- **The CLI runs every command on a thread it spawns** with an explicit stack
  (`std::thread::Builder`, `INTERPRETER_STACK_BYTES` in
  `crates/ns-cli/src/main.rs`): 32 MiB, from the measured native cost of one
  re-entry in a debug build, N times over, twice, rounded up ("What the
  trampoline moved" above). It is reserved, not committed, so only what a
  program uses is backed by memory. The whole command runs there, not only its
  sends, because the symbol interner is thread-local (`symbol.rs`) and every
  symbol a command interns must be interned on the thread that uses it. The
  goldens at N, one per path — `do-recursion`, `instantiation-recursion` and
  `accessor-recursion`, run in a debug build — pin that the error, not an
  abort, is what a program sees. An embedder driving `Interpreter` directly
  owes its thread the same stack.

Why not one count sized against the native stack: every counted activation can
also be a `do:` re-entry, so L would have to fit L re-entries into the native
stack — about 10,000 at most, with a reservation of over 100 MB in a debug
build — and the guarantee would hang on Rust frame sizes that change with every
refactoring and compiler release. Why not a memory bound, as Cog and Primordial
Soup have, or a stack-pointer check, as V8 and HotSpot have: both trip at
different depths under the two lowerings, whose activations differ in number
and size, and the second also between a debug and a release build.

### A superclass chain is walked iteratively

`Interpreter::run_initializer_levels` walks it with an explicit list: each
level's factory arity checked and its clause message evaluated from the class
upward, each level's arguments kept, then the initializers
(`run_own_initializer`) from the top down — the order of every observable
effect a per-level recursion had. Recursing once per level would escape both
bounds: a level with no initializer code pushes no activation and re-enters
nothing, so neither would see a long chain of such levels, and its length is
an image's to choose — the same exposure `metaclass_of` and `dispatch::walk`
were made iterative for.

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
re-entries; Primordial Soup and Cog need no second bound only because nothing
in them re-enters the interpreter from native code. Some paths in ns-next still
recurse on the Rust stack ("What stays recursive" below), and each level of
them costs native stack: 9,280 bytes a level through `do:` in a debug build,
measured on the trampoline ("What the trampoline moved" above). N ("Two bounds,
not one" above) is what bounds them.

### What stays recursive, and why that is not a hole

Three paths keep their Rust frames: `do:`, the one native that still calls
back into Newspeak, through `builtins::call_block` and
`Interpreter::invoke_closure`; instantiation, whose initializer levels and
clause messages each run in a nested run of the loop; and the nested-class
accessor, which runs a superclass clause in one. Each re-entry goes through
`Interpreter::reenter` and is counted against N. None of the three is a
correctness problem today. What makes them go is the root discipline: a Rust
frame below a nested run of the loop holds values across that run's
safepoints, where a moving collector could not see or rewrite them
([memory.md](memory.md), "No Rust frame is live below a safepoint"). All
three run Newspeak code in a fixed order that a continuation can drive, so all
three become trampoline steps; see "Planned: no re-entry from Rust" below.

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
is a growable `Vec`. Continuing an activation from its divorced frame is
re-entry, which does not exist yet; it is the same missing piece as resuming a
decoded frame ([image-format.md](image-format.md), "Planned: suspend and
resume"). The document already accepts that entering the debugger forces the
frames it touches out of their fast representation; this is the same step, one
level below the JIT.

## Planned: no re-entry from Rust

What would change: Rust would enter the run loop only as its outermost caller —
the host's `send` and `instantiate`, and a stateless binary's entry — and never
from inside a running computation. `do:`, instantiation and the nested-class
accessor, the three paths in "What stays recursive" above, become trampoline
steps. This is step 2a of Milestone 4, decided 2026-09-13, and it is what the
root discipline stands on ([memory.md](memory.md), "Planned: checked roots and
a collector"): at a safepoint no Rust frame then holds a value.

All three follow the pattern `whileTrue:` set: a decision answers an activation
together with a `Continuation`, the continuation rides on the activation's
stack entry, and `Interpreter::resume` applies it when the activation returns.
The semantics and the order of every observable effect stay exactly as they
are.

- **`do:`** keeps its snapshot of the elements. An empty collection, or an
  argument that is not a block, answers the receiver at once: a non-block stands
  for its own value, so today's call per element observes nothing. Otherwise the
  native answers `Step::Block` for the first element with
  `Continuation::DoElement`, whose state carries the receiver, the block, the
  snapshot and the next index; `resume` activates the block for the next element
  or answers the receiver. The block's activation is counted, as it is today;
  a `^` in it unwinds like any other activation's, and its arity is checked by
  the same `check_block_arity`. Without a nested run per element this should
  make `do:` faster rather than slower. It removes the callback without writing
  Newspeak, and it does not make the later move to Newspeak harder: a
  trampoline step maps onto a primitive as `Closure>>value`'s does
  ([core-library.md](core-library.md)).
- **Instantiation** becomes a state machine behind `Continuation::Instantiate`.
  Its state is the instance, the levels gathered so far and the phase — upward
  with a level and its arguments, or downward with an index — the walk "A
  superclass chain is walked iteratively" above describes, unchanged.
  `Interpreter::advance_instantiation` does the synchronous parts itself — each
  level's arity check, the clause selector check, a level with no code — and
  answers an activation as soon as code must run: a clause message's arguments,
  or a level's initializer. Such a body is neither a method nor a block, so
  `Step` gains a third variant, `Step::Body`, for an initializer, a superclass
  clause or a superclass message; it is counted, as those are today. The last
  step answers the instance. `try_factory_send` becomes an ordinary decision,
  and the public `instantiate` becomes a host entry shaped like `send`: decide,
  and push and run the loop if the decision needs it.
- **The nested-class accessor** answers the clause's activation with
  `Continuation::NestedClass`, whose state is the receiver, its cache index,
  the declaration and the declaring class; `resume` does steps 3 to 5 — the
  application, the class object, the cache — and answers the class.
  `nested_in_progress` stays a stack but records an `ObjRef` instead of the
  arena index it records today, which would be silently wrong under a moving
  collector. Its entry is removed when the continuation runs or when
  `vm::unwind` throws the activation away: `interpreter/` gains
  `Interpreter::abandon`, the counterpart of `resume` for an abandoned
  continuation, which `unwind` calls for every entry it pops, forwarding and
  deciding nothing — the way `vm::finish` forwards to `resume`.

What goes: `reenter`, `invoke_closure`, `builtins::call_block`, `native_depth`,
`MAX_NATIVE_DEPTH` and its error. No decision runs code any more, so `vm::send`
no longer needs a fresh argument buffer for a decision that re-enters. That
`vm::run` is never nested is a debug assertion in step 2a and becomes structure
in step 2c, where nothing a native can reach offers `run` at all
([memory.md](memory.md), "The brand").

**One bound, not two.** With nothing re-entering the loop from Rust, N has
nothing to count, and each of the recursions it bounded is made only of
counted activations, so each ends at L like any other recursion. Three golden
outcomes in `crates/ns-cli/tests/program_outcomes.rs` — `do-recursion`,
`instantiation-recursion` and `accessor-recursion` — end today in `stack
overflow: too many nested native calls`, and will end in `stack overflow:
recursion too deep`. That is a behaviour change, and it is right because the
limit the old text reported no longer exists; it is not recorded because the
code produces it. `INTERPRETER_STACK_BYTES` was sized from N's measured
native cost, so it is re-derived from what still recurses on the Rust stack —
the parser, the resolver and the compiler, whose depth the parser's nesting
ceiling bounds. The thread itself stays, because the symbol interner is
thread-local.

**Gates.** The golden outcomes, the fixtures and `run-corpus` under both
lowerings, with only the three changes above; and a before-and-after
measurement of the three benchmarks on a release build in one sitting, in which
no source may be slower.

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
- **The control-flow natives are trampoline steps and depth has two bounds** —
  L = 100,000 activations, N = 1,000 re-entries from Rust — because a callback
  native would make the reference lowering's native stack grow with source
  nesting, and a single bound would tie the language's depth to Rust frame
  sizes.
- **Rust is not to re-enter the run loop** (decided 2026-09-13, "Planned: no
  re-entry from Rust"): `do:`, instantiation and the nested-class accessor
  become trampoline steps, so that no Rust frame is live below a safepoint and
  the root discipline has nothing to root but the host's values. N goes with
  them.
