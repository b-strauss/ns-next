# Execution — activations, both engines, the VM

> **Status:** built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the frame arena, the `FrameRef`
> handle, every read and write of an activation), `types.rs` (`Frame`),
> `vm.rs`, `interpreter/` (the shared orchestration and the three engine
> hooks).

This is the consumer side. What produces the bytecode — the instruction set,
lowering, register allocation, inlining — is [compiler.md](compiler.md).

## Two engines, permanently

`ns-next run` defaults to `--engine=bytecode`. `--engine=tree-walk` is the
Milestone 1 interpreter, kept **permanently** as a differential oracle rather than
as a transitional convenience.

**For every program both engines must produce identical exit status, stdout and
stderr.** `crates/ns-cli/tests/differential.rs` enforces this over every
`tests-fixtures/*.ns` plus inline sources, with no exemptions, and `run-corpus`
does the same over the runnable corpus. If a change makes the engines disagree,
the tree-walker is the reference — fix the compiler or the VM.

Two engines that must agree byte for byte is a verification property this project
funds on purpose. When the moving collector lands, the tree-walker gets its own
handle-based root discipline rather than being retired; see [memory.md](memory.md).

The one thing the tree-walker cannot do is run from a loaded image: `image::load`
decodes mixins and class objects, never AST-carrying bindings, so there is no
source for it to execute. `Interpreter::set_engine(Engine::TreeWalk)` refuses on a
loaded image rather than running it against the wrong data.

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

`temps` is the register file, addressed as described in
[compiler.md](compiler.md). It is a growable `Vec` on purpose: adding a local slot
to a running method means growing the active frame's `temps`, initializing the new
entries to nil, and remapping the `pc` — the edit-and-continue requirement.

`kind` distinguishes a method, closure, initializer and top-level activation. An
initializer frame must be able to name the code it is executing so that `pc` is
interpretable, since the statement→PC table is reached through the frame's kind.

`pc` means a statement index under the tree-walker and an instruction index under
the VM. Frames are serialized, so this is a format-visible ambiguity and the frame
carries a discriminator rather than leaving it implicit.

**The frame chain is walkable independently of the Rust stack**, and always has
been. That is what the debugger will reflect over, what an image snapshot
serializes, and what a precise collector will scan.

## The VM and the three hooks

`Interpreter` carries `engine: Engine`, either `TreeWalk` or
`Bytecode(CompiledModule)`. Everything in `interpreter/` that decides *what* to
run is shared and identical under both. Exactly three places decide *how* a body
runs:

1. **Method body** — `invoke_method`, after building the frame.
2. **Block body** — `invoke_closure`, dispatching on `ClosureData.body`, which is
   `ClosureBody::Ast` or `ClosureBody::Compiled`. A closure therefore always runs
   with the engine that created it, and "under `--engine=bytecode` no
   Newspeak-level code is tree-walked" is enforced by the type rather than by a
   flag. Every native that invokes a block — `ifTrue:ifFalse:`, `do:`, `and:`,
   `whileTrue:`, `value` — goes through the same unchanged path and simply lands
   on the compiled arm.
3. **Instance initializer** — `instantiate`'s per-level "run this level's own slot
   initializers and initializer expressions" step. A class with no compiled
   initializer and no slots is a no-op under both; a user class missing from the
   cache is a compilation-coverage bug and panics.

`vm.rs` is therefore *only* a `match`-dispatch loop over `Instr` —
`run_code(interp, frame, code, bindings)`, one compiled body plus `pc` plus a
register file per frame. Its send handlers call straight back into the shared
orchestration, which re-enters `run_code` through hook 1 or 2 for any user-defined
target.

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** Every error a program can observe is
produced by the same shared code under both engines. That is what makes the
tree-walker a byte-for-byte oracle including stderr, rather than a merely similar
one — and it is why the shared-orchestration rule in `CLAUDE.md` forbids copying
anything out of `interpreter/` into `vm.rs`.

Block invocation is not special-cased into an opcode: `Block value` is still a
native method, the sender's frame still becomes the closure activation's sender,
and a wrong argument count is still the same error.

## Where `pc` is stored

`frame.pc` is written **only by the instructions that can hand the frame to an
observer**: every send, since a nested activation runs with this frame as its
sender; `MakeClosure`, since the frame escapes into the object graph where the
image encoder can serialize it; and `NonLocalReturn`, since control leaves with a
signal.

Nothing can read `pc` while a straight-line instruction executes, because
`run_code` never yields between them. Keeping the store out of the straight-line
arms takes a write to the frame arena off the hot path for a value nothing
observes there. It used to take a `RefCell` borrow with it; the arena removed
that half, so what this saves is smaller than it was.

Whether a debugger will force `pc` at every instruction is an open question for
the milestone that builds one; it is a deliberate trade, not an oversight.

## Non-local return

A closure's `home` is the enclosing method or initializer activation. A block
body's `^` raises through `raise_return` on the block's own frame, which walks to
`home`, checks `uncontinuable` — raising `cannotReturn` if set — marks the target
uncontinuable, and returns a return signal.

That signal propagates out of `run_code` through every intervening Rust frame,
native primitives included, until the shared `invoke_method` that owns `home`
catches it — exactly as it catches the tree-walker's. When `home` is an
*initializer* frame nothing catches it, a gap both engines share identically.

A method body's own `^` never raises a signal at all. `invoke_method` marks the
frame uncontinuable on every exit path regardless, so the observable discipline —
a block invoked after its home has returned gets `cannotReturn` — is preserved
without a signal round-trip on the common path.

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvalResult`.** `Signal`
deliberately carries no `Debug` impl of its own -- every field it holds
could support one today, but leaving `Signal` without one means
`EvalResult` (`Result<Value, Signal>`) has none either, and
`Result::unwrap`/`expect` require `E: Debug`. Calling either, or writing
`{:?}` on an `EvalResult`, is therefore a compile error, not a lapse a
reviewer has to catch. Tests unwrap through a local `expect_ok` helper.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping and
compiling excluded — over a 50,000-iteration `whileTrue:` loop, on a release
build. Over `bench_loop.ns`: bytecode median ~16.5 ms against tree-walk ~37 ms.

That ratio is **not** a pure dispatch comparison: the compiler inlines
`whileTrue:`, so the bytecode engine does not pay the loop-control overhead the
tree-walker does.

Never judge performance from a debug build.

Every figure in this section is a median of a whole-program benchmark on one
machine. Only numbers measured against each other in one sitting are compared
here; a figure carried across sessions says more about the machine's load than
about the code.

### What a send costs, and where it goes

Milestone 4's design rests on a decomposition rather than on the aggregate. The
decomposition below was measured on the pre-arena `Rc<RefCell<_>>`
representation, and it is what chose the milestone's order. It has not been
re-taken since; "What the arena moved" below re-measures the aggregate only, and
says which of the itemized costs it removed.
Three separate fixtures, shaped like `bench_loop.ns` but not it -- each with
the body `[n < 50000] whileTrue: [<first statement>. n := n + 1]` -- isolate
the pieces, because each differs from the next by exactly one send. Row
one's `sum := sum + 1` reads a literal where `bench_loop.ns` itself reads
the variable `n` (`sum := sum + n`), so its 19.02 ms is a separate
measurement that happens to land close to `bench_loop.ns`'s own ~19.6 ms
above, not the same run reported twice:

| First statement | Method it calls | bytecode median | ns/iteration |
|---|---|---|---|
| `sum := sum + 1` | — | 19.02 ms | 380.5 |
| `sum := self tick: sum` | `public tick: x = (^x)` | 22.44 ms | 448.7 |
| `sum := self tick: sum` | `public tick: x = (^x + 1)` | 27.28 ms | 545.7 |

Rows one and three each pay three native sends per iteration; row two pays two,
because its method body has no `+`. So the third minus the second is exactly one
native `+` send: **~97 ns**. The third minus the first is exactly one
user-method send, both paying three native sends: **~165 ns**. The second minus
the first is then predicted to be their difference, 68 ns, and measures 68.2 —
the decomposition is self-consistent rather than fitted.

So **building an activation costs about 68 ns, roughly 41% of a user-method
send**, and it is a fixed cost: a two-argument send measures the same as a
one-argument one. What is in it is the current representation, not the work:
three heap allocations (the `Rc<RefCell<Frame>>` block, `temps`, and the `args`
vector `collect` builds), reference-count traffic on `sender`, `method`,
`bindings` and the defining class, and a `HashMap` lookup per invocation just to
find the compiled body. On top of that, every register read and write in
`run_code` takes a `RefCell` borrow, which is spread across the straight-line
instructions of all three rows rather than isolated in any one of them.

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

| Run | Before, bytecode | After, bytecode | Before, tree-walk | After, tree-walk |
|---|---|---|---|---|
| 1 | 19.38 ms | 16.45 ms | 37.04 ms | 36.78 ms |
| 2 | 19.54 ms | 16.82 ms | 37.50 ms | 37.23 ms |
| 3 | 19.60 ms | 16.53 ms | 37.13 ms | 37.12 ms |

**The bytecode engine gained about 15%**, from ~19.5 ms to ~16.5 ms. The three
before-figures and the three after-figures do not overlap, and the split is
larger than either group's own spread, so this is a real move rather than noise.

What moved is precisely two of the itemized costs above: the `Rc` box and its
reference-count traffic on `sender`, `home` and every shared handle, and the
`RefCell` borrow on every register read and write — which is why the gain is
spread across the straight-line instructions rather than concentrated in sends.

What did **not** move is the rest of the activation cost, and this is the part
worth stating plainly rather than rounding away. `Frame.args` and `Frame.temps`
are still a `Vec` allocation apiece on every activation, so two of the three heap
allocations per send survive; the `HashMap` lookup per invocation that finds the
compiled body survives; and building an activation is still an allocation rather
than an index bump. Those belong to "Planned: activations on a stack, reified on
demand" below, and the ~68 ns activation cost above is not claimed to have been
re-measured.

**The tree-walker did not move**, and its medians are within noise of each
other — expected, since it allocates activations at a rate the bytecode engine
does not reach for this loop and pays for them in ways the arena did not touch.
Its maximum, however, got worse: ~117 ms in every after-run against ~38 ms
before, on the same ten timed runs. The likeliest explanation is the arena's
`Vec` doubling rather than anything about dispatch — nothing reclaims a slot, so
the frame arena grows monotonically across the ten runs, and the copy at a
doubling point has both buffers resident and would show up as exactly one slow
run. That is an inference from the shape of the peak-memory curve in
[memory.md](memory.md), not a measured cause; pre-sizing the arenas with
`with_capacity` and re-running is the control that would settle it, and it has
not been run. The cost is an interim one with a collector behind it, and it is
recorded in [memory.md](memory.md) beside the peak-memory figures because that is
where the collector's design has to see it.

### A 13% regression, found and recovered

Routing every `Object`/`Frame` access through `Heap` (`docs/design/memory.md`,
"The seam") was designed to cost nothing: `--release` inlines an accessor
call away, and each one does the same borrow-clone-drop the call site did
before. Migrating `interpreter/eval.rs` broke that measurably: `bench_loop.ns`
went from 20.07 ms to 22.77 ms, about 13% (13.45%).

The cause was `dispatch_lexical_send`'s head, which every implicit-receiver
send runs. Before the migration it took one `RefCell` borrow and read both
the receiver and the current class out of it; the migration replaced that
with two separate accessor calls, each its own borrow. `bench_loop.ns` sends
roughly 250,000 lexical sends per run, so the extra borrow pair cost on the
order of 10 ns each — enough, at that call count, to account for the whole
regression. A batched accessor, `receiver_and_current_class`, restores the
single-borrow shape and recovered the cost in full.

The companion result is what makes the number legible rather than alarming:
the identical pattern — one extra `RefCell` borrow per call — also appeared
in `dispatch::walk`, on the per-inheritance-level path method lookup walks,
and cost nothing measurable there. The difference is not that one borrow is
expensive and the other cheap; it is executions per benchmark run. A lexical
send runs 250,000 times in this benchmark, an inheritance level far fewer.
The lesson generalized to the arena migration that followed: a per-access cost is
only worth chasing where the access count is large, and the way to tell
which sites those are is to measure, not to guess from the shape of the code.

## Planned: activations on a stack, reified on demand

What would change: an activation stops being an entry in the `Heap`'s frame arena
with two vectors of its own, and becomes an entry on one activation stack the
`Heap` owns. A first-class `Frame` is created only when something needs one. It
lands in two steps, each against a green suite and its own before-and-after
measurement: **the stack itself** (1a), then **the trampoline** (1b).

Today every activation is a separate arena entry, created whether or not
anything ever looks at it, and each one allocates its own `args` and `temps`
vectors. The arena already took the `Rc` box and the `RefCell` borrow out of
that — about 15% of the benchmark, measured above — but not the allocations, not
the "always reified" part, and not the retention: nothing reclaims an arena
slot, so every activation the process ever made is kept
([memory.md](memory.md)).

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
returned from the method that declares `n` goes on writing `n`. So a reified
frame here has to keep its registers when its activation returns ("Pop" below).
Removing that need is a step of its own, "Planned: closure conversion" below.

### Representation (1a)

```rust
// heap.rs -- every field private to the module
struct ActivationStack {
    values:  Vec<Value>,       // every live activation's registers, contiguously
    entries: Vec<StackEntry>,  // one per live activation
}

struct StackEntry {
    receiver: Value,
    current_class: Option<ClassRef>,
    kind: FrameKind,           // as today; Closure(Rc<ClosureData>) carries home and captured_frame
    base: u32,                 // first register in `values`
    num_args: u16,
    pc: usize,
    pc_kind: PcKind,
    slot_base: usize,
    reified: Option<FrameRef>, // set once the activation is married
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
- **A push** copies the arguments into `values`, fills the temporaries with nil
  and appends a `StackEntry`. Once both vectors have grown to the program's
  depth, pushing allocates nothing; it replaces `alloc_frame` and the two
  vectors a `Frame` allocates today.
- **Register access through an `ActivationRef`** is `values[entries[i].base +
  r]`, with no branch. An entry's `base` is fixed for its whole life, and
  everything is addressed by index, so `values` reallocating as it grows moves
  nothing anyone holds; the VM may therefore cache `base` in a local.
- **Two handle types, not one tagged handle.** `ActivationRef` names a live
  stack entry; `FrameRef` names a reified frame in the arena and is the only
  activation handle anything may *store*: `ClosureData.captured_frame` and
  `home`, `Frame.home` and every image record take a `FrameRef`, so storing a
  stack handle is a compile error rather than a convention. A tagged `FrameRef`
  would have touched only `heap.rs`, but it would put a branch on every
  register access and leave "a stack handle was stored" to a debug-build
  assertion — the silent failure class the arena's private index was built to
  exclude.
- **An `ActivationRef` is 4 bytes in a release build.** In a debug build it
  carries `origin`, like every handle, and a `generation` drawn from a counter at
  each push. A handle read after its entry was popped and the index reused would
  otherwise answer a different, perfectly valid activation; the generation check
  turns that into a panic at the read.
- **A reified `Frame`** — the arena type, unchanged in the image format — gains a
  field private to `heap.rs`, `live: Option<u32>`: the entry it is married to.
  While it is set, the frame's own `args`/`temps` are empty and a read or write
  through the `FrameRef` reaches the stack slot. That branch exists only on
  `LoadOuterLocal`/`StoreOuterLocal`, the path from a block to its method's
  variables.

### Lifecycle (1a)

**Reify.** `Heap::reify(act) -> FrameRef` is idempotent: the first call creates
the arena `Frame` — receiver, `kind`, `current_class`, `pc`, `live =
Some(entry)` — and records it in `entry.reified`; every later call answers the
same handle. It has exactly two callers, both closure creation: the VM's
`MakeClosure` and the tree-walker's `eval_block_literal`. `home_of` keeps its
meaning — a closure activation passes its closure's `home` on, any other
activation is its own home, now as its reified frame.

**Pop.** Every site that pushes pops on *every* exit, error paths included:
`invoke_method`, `invoke_closure`, `run_initializer_level` and
`nested::run_clause_body` — the sites that today mark a frame uncontinuable on
every exit. If the entry was reified its frame is **divorced**: the registers are
copied into the frame's `args`/`temps`, `live` becomes `None`, `uncontinuable`
becomes true and `sender` becomes `None`. Then `values` is truncated to `base`.
An entry that was never reified leaves nothing behind, so storage is bounded by
the deepest the program has been rather than by every activation it has ever
made. In a debug build the outermost send asserts the stack is empty again.

**Sender.** A live activation's sender is the entry below it — structural, not a
stored field, so it cannot be forgotten at a block boundary the way a
`sender: None` once could. A dead frame's `sender` is `None`: Smalltalk's
semantics for a returned context, and what Primordial Soup does on return
(`zap->set_sender(nil)`) and lazily in `HasLivingFrame`. Nothing reads the
sender of a live activation yet, so no accessor for it is added until the
debugger needs one.

**Non-local return.** `Signal::Return` carries an `ActivationRef`. `raise_return`
picks the target: for a closure activation, its closure's `home` — the entry it
is married to if it is live, and `cannotReturn` if it is dead (divorced, or
decoded from an image); for any other activation, the activation itself, which
is the tree-walker's method-level `^`. `invoke_method` catches it exactly as
today, by `target.same(&act)`. Because there is one stack, a live target is
always below the activation raising the return, so the unwind cannot miss a
catcher that exists.

**One observable change, identical under both engines.** A block created during
initialization — `| blk = [ ^2 ]. |` — whose `^` runs *after* initialization has
finished now raises `cannotReturn`, as `newspeak-spec.pdf` §7.2 requires.
Today its home is an initializer frame that nothing ever marks uncontinuable, so
the signal escapes to the top as "internal: non-local return escaped the entry
send". A `^` into an initializer that is still running escapes exactly as
before; that gap is recorded under "Non-local return" above and is not this
step's.

### Signatures and the rest of the runtime (1a)

- Wherever a `&FrameRef` means *the running activation* it becomes an
  `ActivationRef`: `run_code`, `run_statements`, the `eval_*` functions,
  `dispatch_lexical_send`, `dispatch_super_send`, `read_local`/`write_local`,
  `Interpreter::send`'s and `invoke_method`'s `sender`, `dispatch_native` and
  every `builtins/` module. `FrameRef` stays for closures, `Frame.home`, the
  image, and `frame_at_depth` from depth 1 on: the first hop goes from a closure
  activation to its `captured_frame`, the rest follow `FrameRef` links.
- The three engine hooks are unchanged apart from receiving an `ActivationRef`.
  Both engines push onto the same stack, because frame construction is shared
  orchestration; the tree-walker gets the same bound on storage.
- **The image encoder** only ever runs between sends, when nothing is live; a
  debug assertion states that no encoded frame is married. Dead frames no longer
  carry a sender chain, so snapshots get smaller. **The decoder** builds frames
  that are never married. The format and its wire tags do not change.

### What 1a leaves for 1b, and for later

- **Arguments stay a `Vec` in 1a.** `send` keeps its `args: Vec<Value>`, and the
  push copies them into the stack window, so the VM's `collect` survives — one
  of the three allocations a send used to make. The trampoline removes it.
- **Execution stays recursive** on the Rust stack, so recursion depth is still
  bounded only by the native stack until 1b.
- **The `HashMap` lookup** that finds a compiled body per invocation stays. The
  inline caches ([dispatch.md](dispatch.md)) remove it, since a cache entry
  carries the body.

### The trampoline (1b)

**The trampoline falls out of the stack rather than driving it.** An explicit
activation stack is what a trampolined `run_code` needs anyway, so replacing the
Rust-stack recursion — `run_code` → `interp.send` → hook → `run_code` — with a
loop that pushes is the same change viewed from the other side. Getting there
requires splitting `Interpreter::send` so that it *decides* without *executing*:
lookup, `doesNotUnderstand:`, native dispatch and the nested-class accessor stay
in `interpreter/`, shared by both engines and unchanged in what they produce, but
they return a description of what to activate instead of activating it. That
keeps the shared-orchestration rule intact — what moves into the loop is the act
of running a body, never the decision of which body. The loop then copies the
arguments straight from the caller's argument registers into the callee's
window, which is where `collect`'s vector disappears.

Two things the trampoline settles on its own:

- **`Block value` stops being a callback.** It is a native today that calls
  `invoke_closure`, which re-enters `run_code`. Once the loop owns the stack,
  activating a closure is what the loop already does, so the Rust frame in the
  middle disappears without anything being rewritten in Newspeak.
- **Recursion depth becomes a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded artifact.
  The limit counts stack entries, so it means the same thing under both engines —
  but the tree-walker stays recursive on the Rust stack, and it must reach the
  limit before it exhausts the native stack or the two engines disagree at
  depth. How that is guaranteed is 1b's to settle.

`Signal::Return` needs re-implementing against the stack. After 1a it already
names its target by `ActivationRef`, but it still propagates through Rust `?`
until the `invoke_method` that owns the target catches it; under the trampoline
it becomes an unwind of the activation stack to the target entry. The semantics
are unchanged and both engines must still agree on them, so this is the part of
the change the differential harness has to be trusted hardest on.

### What stays recursive, and why that is not a hole

The natives that call back into Newspeak — `builtins::call_block`'s callers —
keep their Rust frames for now. That is a barrier for a future JIT rather than a
correctness problem, and it is addressed by moving them out of Rust rather than
by trampolining around them; see [core-library.md](core-library.md). Their
Rust-held values are covered by the scope discipline in
[memory.md](memory.md), the same way the tree-walker's are.

### How this meets the liveness requirements

Reification is not a compromise against
[liveness-and-reflection.md](liveness-and-reflection.md); it is that document's
own unifying principle applied one level down. The interpreter level is the
source of truth and a JIT is an accelerator that must be able to deoptimize to
it. A stack zone is an accelerator over the reified activation model, and
reification is its deoptimization.

That resolves the one requirement that looks like a conflict. Edit-and-continue
must be able to add a local to a method while an activation of it is live, and
`Frame.temps` is a growable `Vec` precisely for that — which a flat register
stack cannot offer for an entry in the middle. The answer is that editing a
method reifies its affected activations first, and a reified frame has exactly
today's flexibility. The document already accepts that entering the debugger
forces the frames it touches out of their fast representation; this is the same
step, one level below the JIT.

## Planned: closure conversion

What would change: a block stops reaching its method's variables through the
method's frame. A captured variable that is only read is copied into the closure
when the closure is created; one that is written lives in a small heap box that
the method and the block share. The resolver decides which is which, so both
engines see the same conversion. Placed after the inline caches and before the
collector, whose retention it affects most.

Activations on a stack (1a) keep ns-next's by-reference model and pay for it
with a divorce: a reified frame copies all its registers into the arena when its
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
frames live in the heap anyway — Smalltalk-80's Blue Book, ns-next before 1a —
or whose JIT can remove the cost: SOMns materializes the frame for every block
(`prior-art/somns/src/som/interpreter/nodes/literals/BlockNode.java`,
`frame.materialize()`) and relies on Graal's partial escape analysis, which
removes the materialization only when the block is inlined. Outside that
lineage, V8's context allocation, CPython's cells and Scheme's stack-based model
are the same conversion, and Lua's upvalues are a per-variable middle ground: an
open upvalue points at its stack slot and is closed — that one variable copied —
when the function returns.

What it changes in cost, for a method that creates a non-inlined block:

| | By reference (1a) | Converted |
|---|---|---|
| Per activation that creates a block | a reified frame, plus a copy of every register at return | the closure's copied values, plus a box per written captured variable |
| A block reading a captured variable | one hop per nesting level, then a live-or-dead branch | a closure field, or closure → box; independent of depth |
| The method reading its own captured variable | a register | a register if only read; through the box if written |
| What a surviving block retains | the whole frame, including registers it never reads | exactly what it captured |
| A later JIT | every local a block can reach stays in memory once a block exists | copied values are ordinary values; escape analysis can remove a box |

For the interpreter the difference is small — the large gain of 1a, that an
activation creating no block allocates nothing, is the same under both — so the
case for conversion is the collector's space safety and the JIT's freedom, not
the interpreter's speed. What it costs is breadth: the resolver's capture
analysis, the compiler's instructions for boxes and copied values, the
tree-walker's matching semantics, and the closure record in the image format.

## Planned: JIT tiers

Beyond inline caches nothing is designed. The tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md), and the constraint that
binds it is that no native code may ever enter an image.

## Decisions

- **The tree-walker is permanent**, and is the reference when the engines
  disagree.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, which is what makes the differential harness meaningful rather than
  approximate. Splitting `send` into a decision and an activation preserves this:
  the decision does not move.
- **A closure runs with the engine that created it**, enforced by the type of its
  body rather than by a run-time flag.
- **`pc` is stored only at observable points**, not at every instruction — a
  deliberate hot-path trade the debugger milestone may have to revisit.
- **Activations are stack entries, reified on demand**, because the
  always-reified model costs about 41% of a send for a structure most activations
  never expose. Moving activations into an arena took part of that — the `Rc` box
  and the `RefCell` borrow, worth about 15% of the benchmark — and left the two
  vector allocations and the reification itself for this step.
- **A live activation and a reified frame are two handle types**, `ActivationRef`
  and `FrameRef`, so that storing a stack handle is a compile error and a
  register access never branches on which kind it holds.
- **A returned activation has no sender**, as in Smalltalk, Cog and Primordial
  Soup. A live one's sender is the entry below it, by construction.
- **Blocks reach their method's variables by reference until closure conversion
  lands**, and a reified frame keeps its registers when its activation returns
  to allow it. The reification mechanism itself is permanent: `^` and the
  debugger need it with or without conversion.
