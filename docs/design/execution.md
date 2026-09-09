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
before, on the same ten timed runs. That is the arena's `Vec` doubling, not
dispatch. Nothing reclaims a slot, so the frame arena grows monotonically across
the ten runs, and the copy at a doubling point has both buffers resident and
shows up as one slow run. The cost is an interim one with a collector behind it;
it is recorded in [memory.md](memory.md) with the peak-memory figures, because
that is where the collector's design has to see it.

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

Today every activation is a separate entry in the `Heap`'s frame arena, created
whether or not anything ever looks at it, and each one allocates its own `args`
and `temps` vectors. The arena already took the `Rc` box and the `RefCell` borrow
out of that — about 15% of the benchmark, measured above — but not the
allocations and not the "always reified" part. The plan is the model Cog uses for
Smalltalk: activations live as entries in a stack, cheap to push and pop, and are
**reified into first-class frame objects only when something needs one** — a
debugger, a mirror, `thisContext`, the image encoder, a closure that captures its
activation, or a non-local return target.

Pushing then costs an index bump rather than an arena append plus two vector
allocations; the caller's argument registers *are* the callee's argument window,
so `collect`'s vector disappears; and `sender` is the entry below rather than a
stored handle. Register access is already a slice index rather than a borrow —
that arrived with the arena — so what remains here is the allocation and the
reification, not the borrow flag.

It also puts a bound on something the arena currently leaves unbounded. A stack
entry is popped, so the storage an activation uses is reused the moment it
returns, where the arena retains every activation the process ever made until a
collector exists ([memory.md](memory.md)).

**The trampoline falls out of this rather than driving it.** An explicit
activation stack is what a trampolined `run_code` needs anyway, so replacing the
Rust-stack recursion — `run_code` → `interp.send` → hook → `run_code` — with a
loop that pushes is the same change viewed from the other side. Getting there
requires splitting `Interpreter::send` so that it *decides* without *executing*:
lookup, `doesNotUnderstand:`, native dispatch and the nested-class accessor stay
in `interpreter/`, shared by both engines and unchanged in what they produce, but
they return a description of what to activate instead of activating it. That
keeps the shared-orchestration rule intact — what moves into the loop is the act
of running a body, never the decision of which body.

Two things the trampoline settles on its own:

- **`Block value` stops being a callback.** It is a native today that calls
  `invoke_closure`, which re-enters `run_code`. Once the loop owns the stack,
  activating a closure is what the loop already does, so the Rust frame in the
  middle disappears without anything being rewritten in Newspeak.
- **Recursion depth becomes a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded artifact.

`Signal::Return` needs re-implementing against the stack: it propagates through
Rust `?` today until the `invoke_method` that owns `home` catches it, and becomes
an unwind of the activation stack to the target entry. The semantics are
unchanged and both engines must still agree on them, so this is the part of the
change the differential harness has to be trusted hardest on.

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
