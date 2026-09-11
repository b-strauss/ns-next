# Execution — activations, the VM

> **Status:** built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> arena, the `ActivationRef` and `FrameRef` handles, every read and write of an
> activation), `types.rs` (`Frame`), `vm.rs`, `interpreter/` (the send
> machinery and activation construction).

This is the consumer side. What produces the bytecode — the instruction set,
lowering, register allocation, inlining — is [compiler.md](compiler.md).

## One engine

The bytecode VM is the only engine. A module's code is compiled when the module
is loaded, or decoded from an artifact, and every Newspeak-level body — method,
block, initializer, superclass clause — runs as compiled code in
`vm::run_code`.

The Milestone 1 tree-walking interpreter, which ran beside the VM as a
differential oracle, was retired on 2026-09-11. A recursive AST evaluator keeps
its computation on the Rust stack, so it could take part in none of what the
trampoline ("Planned: the trampoline" below) exists for — suspending and
serializing a running computation, debugger stepping, edit-and-continue on a
live activation — and it constrained the design wherever it still had to agree.
The oracle was also blind to defects in the orchestration the two shared: four
of the six silent defects Milestone 3's review found were identically wrong
under both.

Until the reference lowering lands there is no second way to run a program and
compare against, so behaviour is protected by recorded answers: every
`tests-fixtures/*.ns` against its `.expected` stdout and an empty stderr
(`crates/ns-cli/tests/run_fixtures.rs`), the golden exit status, stdout and
stderr of the programs that cannot be fixtures
(`crates/ns-cli/tests/program_outcomes.rs`), and `run-corpus` over the runnable
corpus. The answers recorded before the retirement are verdicts the oracle and
the VM agreed on. A change that alters one is a behaviour change, to be
justified as one — never recorded because the code now produces it. The
reference lowering ([compiler.md](compiler.md), "Planned: the reference
lowering") is the planned differential oracle.

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
}

struct StackEntry {
    receiver: Value,
    current_class: Option<ClassRef>,
    kind: FrameKind,           // Closure(Rc<ClosureData>) carries home and captured_frame
    base: u32,                 // first register in `values`
    num_args: u32,
    pc: usize,
    slot_base: usize,
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
  allocates nothing. Four sites push: `invoke_method`, `invoke_closure`,
  `run_initializer_level` and `nested::run_clause_body`, all in
  `interpreter/`. An activation nothing reifies leaves nothing behind; only a
  block the compiler did not inline reifies one, since an inlined
  `ifTrue:`/`and:`/`whileTrue:` argument creates no closure.
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
`invoke_closure` reads the block's receiver and current class from.

**Pop.** Every site that pushes pops on *every* exit, error paths included, and
only the top entry can be popped. If the entry was reified, its frame is
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
  `ActivationRef`: `run_code`, `dispatch_lexical_send`, `dispatch_super_send`,
  `read_local`/`write_local`, `read_outer_local`/`write_outer_local`.
  `FrameRef` stays for closures, `Frame.home`, the image, and `frame_at_depth`
  from depth 1 on: the first hop goes from a closure activation to its
  `captured_frame`, the rest follow `FrameRef` links.
- **No send or block invocation takes a sender.** `Interpreter::send`,
  `invoke_method`, `invoke_closure`, `builtins::dispatch_native`,
  `builtins::call_block` and the `builtins/` modules have no `sender` parameter.
  It was removed rather than retyped to an `ActivationRef`: its only use was to
  fill `Frame.sender`, and once a live activation's sender is the entry below it,
  nothing read it. No native takes an activation handle at all.
- `run_code` receives the `ActivationRef` of the activation one of the four push
  sites pushed for it, and never pushes or pops that activation itself.
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
[compiler.md](compiler.md). The top activation's window can grow in place, the new
entries nil (`Heap::grow_activation_temps`); the initializer path uses that,
because it pushes its activation before it knows the compiled body's register
count. An entry below the top cannot grow, since its window ends where the next
one's begins. A dead frame's `temps` is a growable `Vec` (`Heap::frame_grow_temps`).
Adding a local slot to a running method — growing its register file, initializing
the new entries to nil and remapping the `pc`, the edit-and-continue requirement —
therefore means taking the activation off the stack first; see "How this meets the
liveness requirements" below, which is not built.

`kind` distinguishes a method, closure, initializer, superclass-clause,
superclass-message and top-level activation. An initializer activation must be
able to name the code it is executing so that `pc` is interpretable, since the
statement→PC table is reached through its kind.

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
Everything in `interpreter/` decides *what* to run. Four places run a body, each
by pushing an activation, calling `run_code` and popping it on every exit:

1. **Method body** — `invoke_method`, which also catches a `^` that targets its
   own activation.
2. **Block body** — `invoke_closure`, running `ClosureData.code`. Every native
   that invokes a block — `ifTrue:ifFalse:`, `do:`, `and:`, `whileTrue:`,
   `value` — goes through it.
3. **Instance initializer** — `run_initializer_level`, `instantiate`'s per-level
   step, which runs that level's compiled initializer: its own slot initializers
   and initializer expressions. A class with no compiled initializer and no
   slots is a no-op; a user class whose mixin carries no initializer is refused
   on load and panics if built any other way.
4. **Superclass clause and its message arguments** — `nested::run_clause_body`,
   for the nested-class accessor and for `instantiate`'s deferred clause message.

`vm.rs` is *only* a `match`-dispatch loop over `Instr` —
`run_code(interp, act, code)`: one compiled body, a `pc` in a local, and the
activation's register window. Its send handlers call straight back into
`interpreter/`, which re-enters `run_code` through `invoke_method` or
`invoke_closure` for any user-defined target.

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** Every error a program can observe is
produced in `interpreter/` and `builtins/`. That keeps dispatch in one place,
which the reference lowering and a later JIT will reach unchanged — and it is
why `CLAUDE.md`'s "`vm.rs` holds no dispatch logic" forbids copying anything out
of `interpreter/` into `vm.rs`.

Block invocation is not special-cased into an opcode: `Block value` is still a
native method, the closure's activation is pushed above the one that invoked it —
its sender, by position — and a wrong argument count is still the same error.

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

Nothing can read `pc` while a straight-line instruction executes, because
`run_code` never yields between them. Keeping the store out of the straight-line
arms takes a write off the hot path for a value nothing observes there.

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

That signal propagates out of `run_code` through every intervening Rust frame,
native primitives included, until the `invoke_method` or
`nested::run_clause_body` whose activation is the target catches it, by
`ActivationRef::same`. Because there is one stack, a live target is always below
the activation raising the return, and nothing is pushed while the signal
propagates, so the target's index cannot have been reused by the time a catcher
compares it.

**An initializer home follows the same rule.** A block created during
initialization — `| blk = [ ^2 ]. |` — whose `^` runs after initialization has
finished raises `cannotReturn`, as `newspeak-spec.pdf` §7.2 requires, because
the initializer's frame was divorced when its activation returned. A `^` into an
initializer that is still running escapes: `run_initializer_level` catches no
return, so the signal reaches the top of the entry send as "internal: non-local
return escaped the entry send", a known gap.

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
invokes a block. On a release build, on 2026-09-11, their medians were about
16.5, 20.5 and 38 ms.

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
that, every register read and write in `run_code` took a `RefCell` borrow, which
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
before. Migrating `interpreter/eval.rs` broke that measurably: `bench_loop.ns`
went from 20.07 ms to 22.77 ms, about 13% (13.45%).

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
what offsets that saving has not been attributed, since no profile was taken.
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

## Planned: the trampoline

What would change: sends stop recursing on the Rust stack. Every running
activation already lives on the activation stack; the trampoline makes that stack
the only place a running computation lives, by turning `run_code` →
`interp.send` → `invoke_method` → `run_code` into a loop that pushes and pops.
This is step 1b of Milestone 4, and it lands the way the stack did: against a
green suite and with its own before-and-after measurement.

### What 1a left for it

- **Arguments are still a `Vec`.** `Interpreter::send` takes `args: Vec<Value>`
  and a push copies them into the callee's window, so the VM's `collect` still
  allocates one vector per send, native sends included.
- **Execution is still recursive** on the Rust stack, so recursion depth is
  bounded only by the native stack, and there is no depth limit: a deep enough
  recursion aborts the process instead of raising a Newspeak error.
- **The VM does not cache an activation's `base`.** Every register read or write
  goes through `Heap::activation_local`/`activation_set_local`, which read the
  entry's `base` again. Caching it belongs to the loop the trampoline builds,
  which owns the activation it runs.

### The trampoline (1b)

**The trampoline falls out of the stack rather than driving it.** The activation
stack is what a trampolined `run_code` needs, and it exists, so replacing the
Rust-stack recursion with a loop that pushes is the same change viewed from the
other side. Getting there requires splitting `Interpreter::send` so that it
*decides* without *executing*: lookup, `doesNotUnderstand:`, native dispatch and
the nested-class accessor stay in `interpreter/`, unchanged in what they
produce, but they return a description of what to activate instead of
activating it. That keeps `vm.rs` free of dispatch logic — what moves into the
loop is the act of running a body, never the decision of which body. The loop
then copies the arguments straight from the caller's argument registers into the
callee's window, which is where `collect`'s vector disappears.

Two things the trampoline settles on its own:

- **`Block value` stops being a callback.** It is a native today that calls
  `invoke_closure`, which re-enters `run_code`. Once the loop owns the stack,
  activating a closure is what the loop already does, so the Rust frame in the
  middle disappears without anything being rewritten in Newspeak.
- **Recursion depth becomes a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded artifact.
  How the limit counts has to be settled first. Counting stack entries does
  **not** mean the same thing under two ways of running one program when one of
  them activates blocks the other inlines, as the reference lowering will
  ([compiler.md](compiler.md), "Planned: the reference lowering"). That
  lowering's design decides it. Measured on Windows's 1 MB main-thread stack
  before any of this, the recursion depth reachable today is 818 for recursion
  inside an `ifFalse:` block (release build), and 307 through `do:` — below the
  1000 that Newspeak's own `KernelWeakTests.ns` (`deepStack: 1000`) expects.

`Signal::Return` needs re-implementing against the stack. It already names its
target by `ActivationRef`, but it still propagates through Rust `?` until the
`invoke_method` that owns the target catches it; under the trampoline it becomes
an unwind of the activation stack to the target entry. The semantics are
unchanged, and this is the part of the change the golden outcomes have to be
trusted hardest on: the reference lowering lands first, but it runs on the same
VM, so it cannot cross-check the trampoline's unwind of `Signal::Return` — both
ways of running a program would unwind through the same code.

### How other systems bound recursion depth

The limit's design should start from what the prior art does with a recursion
that outgrows its stack:

| System | Model | Depth limit |
|---|---|---|
| Primordial Soup | Fully trampolined: a closure's `value` primitive is `ActivateClosure`, which pushes a frame and returns to `Interpret`, so Newspeak code never recurses on the C stack. The frame stack is 1024 slots (`kStackSlots`, `vm/interpreter.h`); on overflow `StackOverflowOrInterrupt` flushes every frame but the top one into heap activations (`FlushAllFrames`, `vm/interpreter.cc`). | None. Depth is bounded by the heap; `vm/virtual_memory_*.cc` treats a failed allocation as `FATAL`. |
| Cog | Stack pages. `handleStackOverflow` (`src/spur64.stack/interp.c`) moves frames to the next page, first divorcing that page's frames into heap contexts if it is in use (`divorceFramesIn`). | None. Depth is bounded by memory. |
| Nova | Recursive on the Rust stack. `Agent::check_call_depth` (`nova_vm/src/ecmascript/execution/agent.rs`) counts execution contexts. | 3500 — "experimental number that caused stack overflow on local machine" — then a `RangeError`. |
| SOMns | A Truffle AST interpreter, recursive on the Java stack. | None. A `StackOverflowError` is a JVM failure; one of its test scripts raises the thread stack (`-Xss4096k`). |

Newspeak's own tests assume the Smalltalk behaviour: `KernelWeakTests.ns`'s
`deepStack: 1000` exists to overflow Primordial Soup's frame stack and force a
flush.

What that means for the limit, without deciding it. The Smalltalk-lineage VMs
make depth a question of memory, not a count, and a trampolined VM can do the
same, since its activation stack is a `Vec` on the heap. A fixed count like
Nova's is what a runtime falls back on when the stack its evaluator recurses on
cannot grow. After 1b some paths still recurse on the Rust stack — the natives
that call back into Newspeak, instantiation, the nested-class accessor ("What
stays recursive" below) — and each level of them still costs native stack:
derived from the depth reachable on a 1 MB stack before the trampoline,
recursion through `do:` cost roughly 3.4 KB of native stack per Newspeak level
in a release build and 13.4 KB in a debug one. Whatever the limit counts, those
paths need either a bounded depth or a native stack sized for it.

### What stays recursive, and why that is not a hole

The natives that call back into Newspeak — `builtins::call_block`'s callers —
keep their Rust frames for now. That is a barrier for a future JIT rather than a
correctness problem, and it is addressed by moving them out of Rust rather than
by trampolining around them; see [core-library.md](core-library.md). Their
Rust-held values are covered by the scope discipline in
[memory.md](memory.md).

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
  suspended computation; golden outcomes protect behaviour until the reference
  lowering becomes the differential oracle.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, so the reference lowering and a JIT share it. Splitting `send` into a
  decision and an activation preserves this: the decision does not move.
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
