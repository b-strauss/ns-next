# Memory — object lifetime, identity, collection

> **Status:** partial. Objects live in an arena owned by one `Heap`,
> addressed by 32-bit handles; activations live on an activation stack in
> the same `Heap` and reach the frame arena only when a block captures one.
> Identity, layout ownership and the seam that routes every access through
> `Heap` are built. Nothing reclaims an arena slot: there is no collector,
> and no root discipline for one to use; the discipline is designed, in
> "Planned: checked roots and a collector".
> **Code:** `crates/ns-runtime/src/heap.rs` (the arenas, the activation
> stack, the handle types, every accessor), `types.rs` (`Value`, `Object`,
> `Frame`), `mixin.rs` (layout, and the one remaining iterative `Drop`).

## What exists

**The class owns the slot layout**, and slot access resolves through the
*current* layout, so a layout change plus instance migration is expressible
rather than something the representation forecloses. Layout lives on
`MixinApplication`; see [object-model.md](object-model.md).

**Identity is stable and hashed from a counter.** Every object's
`identity_hash` is drawn from a monotonic counter the first time it is asked
for, never derived from its address. This is required by identity-keyed
collections, by image roots, and by `become:`; an address-derived hash would
break the moment an object moved or an identity was swapped.

**Objects live in an arena and activations on a stack, not in
`Rc<RefCell<_>>`.** One `Heap` owns `objects: Vec<Object>`, the activation
stack every running activation lives on, and `frames` — the arena of *reified*
activations, the ones a block captured or the image decoder rebuilt. An
`ObjRef` or a `FrameRef` is a 32-bit index into an arena, an `ActivationRef`
into the stack. The representation is described below, and the stack in
[execution.md](execution.md), "Activations". Nothing reclaims an arena slot: a
cycle is no longer the special case that leaks, because everything in an arena
is retained, deliberately, until a collector exists. The stack is the exception:
an entry is popped when its activation returns, and one that was never reified
leaves nothing behind. That is not a problem the image format inherits: the
format is defined against the *logical* value model rather than against this
representation, so replacing it again would not break images.

### The representation: an arena of typed handles

The model is the Nova JavaScript engine's: a `Value` is a Rust enum carrying
either its data inline or a 32-bit index into a typed arena, with no `Rc` and no
`RefCell` anywhere in it. Nova is the useful comparison rather than V8 because it
solves this problem in Rust, where the borrow checker is available as an
enforcement mechanism and a C++ engine's conventions are not.

```rust
Value  = Nil | Bool(bool) | SmallInt(i64) | Float(f64) | Obj(ObjRef)   // Copy
ObjRef        { index: u32 }   // private to heap.rs; 4 bytes in a release build
FrameRef      { index: u32 }   // likewise; ClassRef is an alias for ObjRef
ActivationRef { index: u32 }   // likewise, into the activation stack

Heap { objects: Vec<Object>, frames: Vec<FrameSlot>, stack: ActivationStack,
       next_identity_hash: u32 }
```

What that removed is not an abstract elegance but part of the measured cost of
the previous model, itemized in [execution.md](execution.md)'s measurements
section: the `Rc` box and its reference-count traffic on every frame link and
every shared handle, and a `RefCell` borrow on every single register read and
write. A register access is now a slice index into an owned `Vec`. Over
`bench_loop.ns` on a release build that measured as **19.4–19.6 ms before,
16.4–16.8 ms after** — about 15%. The rest of the cost of an activation nothing
captures — a `Vec` apiece for its arguments and temporaries, and an arena entry
— went with the activation stack. A captured activation still takes an arena
entry for its reified frame, and two vectors when its divorce copies the
registers out. [execution.md](execution.md), "What the activation stack moved",
has the figures.

`ObjRef`, `FrameRef`, `ActivationRef` and `Value` are `Copy`. A handle has no
destructor and nothing to clone, which is why `Value` could become `Copy` at all,
and it is what makes an arena entry cheap to hand around.

Three properties follow from an index that did not follow from a pointer.

**A slot's index is known before the slot is filled.** Building an object that
names itself — a bootstrap class whose own class is itself — used to need
`Rc::new_uninit` and `Rc::from_raw`, the only `unsafe` in the crate. `Heap`
reserves the index, then writes the record that mentions it, in safe Rust.
`heap.rs` contains no `unsafe`.

**There is no reference chain to unwind.** See "Iterative teardown" below.

**A handle is only meaningful against the heap that minted it.** See "A handle
is arena-relative" below.

An arena is also closer to the shape the image format already writes. Handles are
architecture-neutral by construction, which is requirement 4 in
[liveness-and-reflection.md](liveness-and-reflection.md), and identity hashes
already come from a counter rather than an address, so nothing about identity
depends on where an object sits.

### A handle is arena-relative

An index is meaningful only against the `Heap` that allocated it. That was the
open question the arena had to settle, and this is the answer: not globally
unique, arena-relative.

Reading a handle through the wrong heap is the one new failure mode the arena
introduces, and it is silent — an index minted by another heap is in range and
names a different, perfectly valid object, so the read answers the wrong thing
rather than failing. A `debug_assertions`-only guard makes that explicit rather
than conventional: each `Heap` carries a distinct id, each handle records the id
of the heap that minted it, and every accessor that takes a handle asserts the two
match — 63 sites in `heap.rs`, 17 of them through an `ActivationRef`, whose check
also compares a generation, because a stack index is reused as soon as its entry
is popped. In a release build neither the fields nor the checks exist, and a
handle is exactly its 32-bit index.

The room the actor milestone needs is still there. Actors are designed to get one
heap each so that pauses stay local
([liveness-and-reflection.md](liveness-and-reflection.md)), which will ask how a
handle encodes *which* heap. A handle is 32 bits and its field is private to
`heap.rs`, so widening it to carry a heap tag is a change to one module — and the
debug guard is already the place that knows a handle has an origin at all.

One deliberate escape exists, and it is opaque rather than a hole.
`Heap::object_identity_key` and `Heap::frame_identity_key` are `pub(crate)` and
return the index as a `usize` for the three places that need a handle as a
`HashMap`/`HashSet` key rather than a comparison: `image/encode.rs`,
`image/decode.rs` and `interpreter/nested.rs`, seven call sites in all. Every
one stores it, compares it, or looks it up; none does arithmetic on it, and none
assumes it is an index into anything. So the property that matters survives — a
handle's *encoding* is still `heap.rs`'s alone to change — but the narrower
claim that nothing outside the module ever sees the bits is not true, and these
two are where to look first if the encoding changes.

They sit on `Heap`, not on the handle, so that the origin check applies to them
like it does to every accessor. Handed a handle and no heap, the operation has
nothing to check against, and two handles from different arenas then produce the
same key and collide silently in a caller's map. Folding the debug-only `origin`
field into the key instead was tried and reverted: that makes the collision
impossible in exactly the build that would have reported it and leaves it
exactly as possible in the build that ships.

### Iterative teardown

Rust has no iterative drop glue, so releasing a deep reference graph recurses and
a sufficiently deep one aborts the process. Since an image is untrusted input and
its graph depth is attacker-controlled, that is not acceptable.

Under the arena, objects and activations have no such graph: an `Object` holds
handles, a handle holds an index, and dropping the `Heap` drops flat `Vec`s — the
two arenas and the activation stack's two vectors. The hand-written iterative
`Drop` implementations they used to need are gone.

**`MixinApplication` still owns a real chain** — `superclass:
Option<Rc<MixinApplication>>` — and keeps its hand-written iterative `Drop`
(`crates/ns-runtime/src/mixin.rs`). It is the only *iterative* `Drop` left in
the crate, which is not the same as the only recursive graph: `Mixin`'s
`nested: Vec<Rc<Mixin>>` and `class_side: Option<Rc<Mixin>>` links drop
recursively too.

The alternative — a depth ceiling — stays rejected **for the application
chain**, because that depth is genuinely unbounded: an application chain is as
long as the class chain it mirrors, `image::decode::rebuild_applications` builds
one link per class record, and a Newspeak program can legitimately declare a
deep inheritance chain. Refusing one would exclude a program the runtime already
runs from source.

A ceiling is the right answer for the `Mixin` tree, and `image/mixins.rs` uses
one (`MAX_MIXIN_DEPTH`). The difference is where the bound comes from: nesting
depth is already bounded by `ns-syntax`'s parser, so the ceiling is derived from
what the front end accepts rather than chosen, and it excludes nothing the
runtime would otherwise run. That file names the three walks it protects, of
which `Rc<Mixin>`'s own drop glue is one. A ceiling is rejected where it would
refuse valid programs, not on principle.

The same reasoning applies to traversal rather than teardown: the image encoder
walks the graph over an explicit stack, and `metaclass_of` walks its chain
iteratively, because an image controls both depths.

### The seam

`crates/ns-runtime/src/heap.rs` is the only place that knows what a handle is.
Every read and write of an `Object`, a `Frame` or a running activation goes
through a `Heap` accessor, so replacing the store behind them touches that file
rather than the 193 call sites (109 under `src`, 84 in the integration tests) the
workspace held before the seam existed. Replacing `Rc<RefCell<_>>` with the
arena is the evidence that this works: **no call site outside `heap.rs` and
`types.rs` changed** for either the frame conversion or the object one.

What the seam hides is the store behind a handle, not the handle types. Those
are part of every signature on purpose: a function that means *the running
activation* takes an `ActivationRef`, and anything that stores one takes a
`FrameRef`, which is what makes storing a stack handle a compile error
([execution.md](execution.md), "Representation"). Changing a handle type is
therefore a change to its callers; changing what stands behind one is not.

The accessors copy out rather than lend out. That follows what the call
sites already did — borrow, clone one field, drop the borrow — and it is
what keeps a rooting discipline from being needed at every site that calls
back into the interpreter while holding a reference.

**The seam is compiler-enforced.** `ObjRef`, `FrameRef`, `ActivationRef` and the
`ClassRef` alias are declared in `heap.rs` with a **private** inner field, so
reading a handle's index from anywhere else — any other module of `ns-runtime`
included — fails to compile. There is no grep gate to remember and no
visibility to widen: an accessor that is missing has to be added in `heap.rs`,
which is where the store is. The one way out is the pair of
`pub(crate)` identity-key accessors described above, which hand the index over
as an opaque key and are used as one — a named exception rather than an
unenforced boundary, and one the origin guard still covers, since they are
accessors on `Heap` like any other.

Where a site needs several fields of one object, frame or activation at once, a
batched accessor answers them together rather than the caller taking several —
`activation_receiver_and_current_class`, `object_snapshot`,
`frame_register_shape` and `frame_snapshot` are the present examples. This is
not only style: the lexical-send head (`sends::decide_lexical_send` today), on
every implicit-receiver send, once took one `RefCell` borrow yielding both the
receiver and the current class;
splitting that into two separate accessor calls measured as a real cost,
recovered by batching them back into one (see [execution.md](execution.md)'s
measurements section). On the activation stack those are two field reads of one
entry rather than two borrows, so the cost batching removes is smaller than it
was; whether it still pays is a measurement nobody has taken.

### Known rough edges in the seam

Carried deliberately rather than fixed. Recorded so that whoever reshapes this
file next starts from a list rather than from rediscovery.

- **The accessor names grew by accretion.** `class_of_obj` reads an *object*
  while `class_application` and its neighbours read a *class* — the same prefix
  for opposite directions. `array_at`/`array_len` and
  `instance_field`/`instance_field_count` are parallel operations under
  divergent names. The activation side is uniform: an accessor through a
  `FrameRef` is `frame_*` and one through an `ActivationRef` is
  `activation_*`. The exceptions are verbs that put the noun second — `set_frame_pc`,
  `set_activation_pc`, `replace_frame` — and the stack operations themselves,
  `push_activation`, `pop_activation`, `top_activation` and `reify`.
- **The `heap` parameter sits first in some signatures and last in others**,
  across `image/`, `compiler/` and `interpreter/`.
- **Copy-out costs, and the accessor that could now avoid them.**
  `builtins/text.rs`'s string dispatch clones the payload on every send, and
  `object_snapshot`/`frame_snapshot` clone each payload twice per encoded
  record. The arena did not remove these, and was never going to: copy-out is a
  property of an accessor's *return type*, not of the store, and `str_value`
  still returns an owned `String`. What the arena changed is that a *borrowing*
  accessor became expressible at all. Under `Rc<RefCell<_>>` one would have had
  to hand back a `Ref` guard, which is unusable at a site that calls back into
  the interpreter; under the arena an accessor can lend out `&str` tied to the
  `&Heap` borrow, and the borrow checker enforces the rest. Whether to add one
  is a measurement, and neither site named here is on a measured path.

## Planned: checked roots and a collector

What would change: a collection could run — only at a safepoint in the run
loop — and move objects, and every handle the runtime holds would be either
where the collector can find and rewrite it or a compile error. Designed on
2026-09-13 as step 2 of Milestone 4, in three parts that each land green and
measured on their own: **2a** removes every re-entry of the run loop from Rust
([execution.md](execution.md), "Planned: no re-entry from Rust"); **2b** adds
the safepoint, the enumeration of everything the collector traces, host roots
and a stress mode that moves every object at every safepoint; **2c** brands
every movable handle with a lifetime, so that holding one across a safepoint
is a compile error. The collector itself is step 5.

**The collector has to move objects.** `become:`-style identity swap and object
forwarding are requirements, not conveniences: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically swaps identity. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

A moving collector must not only *find* every root but *update* it, and roots
held in Rust locals across a collection are the hard case: under `Rc` those
references kept their objects alive and never needed updating, and under a
moving collector they are roots the collector cannot see and indices it cannot
rewrite. The design answers that in three layers: where a collection may
happen, what may be on the Rust stack when it does, and what the compiler
checks.

### Where a collection can happen: only at a safepoint in the run loop

An allocation will never collect. It will count what it allocates and, past a
threshold, *request* a collection, which runs at the next safepoint of the run
loop. `vm::run` reaches one at every switch to another activation — a push, or
a caller resumed — and at every backward jump, so every call and every loop
iteration passes one. Between two instructions `run` holds the running
`ActivationRef`, its `Rc<CompiledCode>` and its `pc`, and no handle the
collector would move; every live value is in a register on the activation
stack or in one of the places "What the collector traces" below lists. The
interpreter's reusable argument buffer is empty there — a send gathers into
it, decides and pushes within one instruction — so it is not a root, and a
debug assertion at the safepoint says so.

The first four systems below chose this placement; the last four collect
elsewhere, and their right-hand column is what that costs their runtime code:

| System | Where a collection can happen | What its runtime code does about the handles it holds |
|---|---|---|
| Cog/Spur | Allocation past `scavengeThreshold` only sets `needGCFlag` and forces an interrupt check, and goes on into eden's reserve; the scavenge runs at the next interrupt check, a send or a backward jump. `forceInterruptCheck` sets `stackLimit` to all ones, so the stack check every frame build already makes is the poll (`src/spur64.cog/cointerp.c`). | Little: no primitive calls back into Smalltalk. |
| Nova | `Vm::inner_execute` checks `agent.check_gc()` at the top of the instruction loop (`nova_vm/src/engine/bytecode/vm.rs`); an allocation only adds to `alloc_counter`. | `GcScope`/`NoGcScope` lifetimes and `Scoped` roots, because nearly every builtin can call JavaScript (`GARBAGE_COLLECTOR.md`). |
| CPython ≥ 3.12 (not in `prior-art/`) | Moved its cycle collector from allocation to the eval breaker (gh-97922), because collecting at an allocation ran it where the VM was inconsistent. | Reference counts. |
| piccolo on gc-arena (not in `prior-art/`) | Only outside `Arena::mutate`. The whole VM is stackless — a Rust callback returns a `Sequence` rather than calling Lua — so control returns outside `mutate` between steps. | Nothing to root: a generative `'gc` lifetime makes a pointer that outlives `mutate` a compile error. |
| V8 (not in `prior-art/`) | Any allocation. | `Handle<T>` through a `HandleScope`, checked by the `gcmole` static analysis and by debug-only `DisallowGarbageCollection` scopes; now moving to direct handles, which require conservative stack scanning. |
| HotSpot (not in `prior-art/`) | Safepoint polls between bytecodes, at compiled code's method entry and exit and non-counted back-edges, and allocation slow paths — precise because the JIT emits an oop map at each. | The VM's own C++ holds oops in `Handle`s under a `HandleMark`, with a debug-only `NoSafepointVerifier`. |
| Primordial Soup | Any allocation: `Heap::AllocateNormal` scavenges when new space is full (`vm/heap.cc`). | An eight-slot `HandleScope` (`vm/heap.h`), by convention; no primitive calls back into Newspeak. |
| SOMns | The JVM's safepoints. | Nothing: Java references are precise through the JIT's stack maps. |

Allocation as a collection point is rejected. Every allocating Rust path — each
native, the compiler, the image decoder with its maps of placeholders — would
have to root what it holds across every allocation, which is the burden V8 and
HotSpot's runtime carry with handles and verifiers; and a JIT would need a
stack map at every inline allocation and could not keep a handle in a register
across one. HotSpot affords it for Java code only because its JIT emits the
maps, and ns-next's Rust code has none. What the run-loop placement costs is
overshoot: a request made during a step waits for the next safepoint, so the
heap can pass its threshold by what one step allocates. A large object going
straight to a large-object space and a small reserve absorb that, as they do in
Cog and Primordial Soup.

The poll will be one compare of a flag the heap owns. If a measurement shows
its price on the send path, Cog's trick is the fallback: a request lowers the
limit `Interpreter::push` already compares the activation depth against, so a
send pays nothing it does not pay today.

### No Rust frame is live below a safepoint

The run loop is re-entered from Rust today, on four paths: the host's `send`,
`do:`, instantiation's initializer levels and clause messages, and the
nested-class accessor's clause ([execution.md](execution.md), "What stays
recursive"). A Rust frame below a nested run holds values across that run's
safepoints, and rooting them is what Nova's `Scoped` values are for. ns-next
removes the frames instead (step 2a): `do:`, instantiation and the accessor
become trampoline steps with continuations, as `whileTrue:` already is, and
Rust enters the loop only as its outermost caller, the host. At every
safepoint the Rust stack then holds the run loop and the host and nothing
else — piccolo's property — and what is left to root is only what the host
keeps.

### The brand: checked by the compiler, not by convention

Step 2c. Every handle a collector can move — `ObjRef`, `ClassRef`, `FrameRef`,
and so `Value` and every type that contains one (`ObjectData`, `ClassData`,
`Frame`, `FrameKind`, `Continuation`, `Step`, `Signal`, `NewspeakError`) — will
carry a lifetime `'gc`, invariant so that it can be neither shortened nor
extended. The only way to obtain one is `Interpreter::enter`, which runs a
closure generic over `'gc` — `for<'gc> FnOnce(&mut Mutator<'gc>) -> R`,
gc-arena's `mutate` — whose result type cannot name `'gc`, so nothing branded
leaves it. `Mutator<'gc>` offers everything a step does: heap access,
allocation, the send decisions, the natives, the compiler, the image codec. It
offers nothing that reaches a safepoint: `vm::run` and the safepoint exist only
on `Interpreter`, outside every `enter`, and each instruction the loop executes
is one `enter`. What the loop carries from one instruction to the next names
no brand — the running `ActivationRef`, its code and its `pc` — and the value
a run ends with leaves it as a host `Root` (below). Two mistakes then fail to
compile rather than corrupt the heap under collection pressure: holding a
handle across a safepoint, and a native re-entering the run loop.
`compile_fail` doctests pin both, and a third: storing a branded handle in a
host's own field.

`ActivationRef` stays unbranded: the collector moves no stack entry, and its
generation check already covers an index reused after a pop.

The heap stores the brand-erased form. Rebranding a handle copies its `u32`,
and a payload is copied into the arena when it is allocated; both conversions
are private to `heap.rs`, and there is **no public `unbind`** — which is where
Nova's discipline leaves a hole that its own documentation closes by
convention ("NEVER unbind a `Value` into a local variable"). `heap.rs` stays
free of `unsafe`. Should a payload conversion measure as a cost, the fallback is
one audited lifetime cast of the kind Nova's `Bindable` makes, decided by that
measurement.

It costs nothing at run time: the lifetimes are gone after monomorphization,
and `enter` is a call through a thin wrapper around `&mut Interpreter` that
should inline away — a before-and-after measurement of all three benchmarks is
a gate of step 2c. Its price is ergonomic, and that price is accepted. The
stress mode below keeps running under it: the brand covers what Rust holds,
the stress mode what the tracer knows.

### Host roots

A host keeps values across runs: `ns-next bench` sends to the same instance 22
times, and `snapshot` encodes after its send. The heap will keep a root table
the collector traces. A `Root` is an `Rc`-shared slot the table also holds —
gc-arena's `DynamicRootSet` model — so a root whose last handle outside the
table is dropped is swept at the next collection, and releasing one needs no
access to the heap. `Interpreter::send`, `instantiate` and `image::load` take
and answer `Root`s, a Newspeak error reaches the host with its receiver as a
`Root`, and a host reads through `enter`; `heap_ref()` and `heap_mut()` go.
Step 2b adds the table and converts the CLI and the tests to it, checked by the
stress mode below; step 2c makes it the only way a host can hold a value.

### What the collector traces

- The bootstrap: `builtin_classes` and `platform_singleton`.
- The activation stack: each entry's receiver, registers, `current_class`, the
  `ClassRef` its `FrameKind` names, and the state its continuation carries.
- Every arena record: an object's class and payload, a frame's fields.
- `nested_in_progress`, whose entries become `ObjRef`s rather than the arena
  indices they are today (step 2a).
- The host root table.

A reified frame is not a root of its own: it is reached through the closure
that captured it, or through the live entry it is married to.

**A handle lives only where the tracer can rewrite it in place**: in an arena
record, in a stack entry, in a continuation that one stack entry owns, or in an
interpreter root — never behind a shared `Rc`. `Rc<ClosureData>` breaks that
today. Its `captured_frame` and `home` are `FrameRef`s, and the same `Rc` hangs
off the closure object, off `FrameKind::Closure` on every activation of the
closure, and off `WhileLoop.condition`. Step 2b splits it: the shared part keeps
`params` and `code`, the two handles move into the closure object's own
payload, and `FrameKind::Closure` and `WhileLoop` refer to the closure object.

### Checking the roots before a collector exists: the stress mode

Step 2b. The brand checks what Rust locals hold; it cannot check that the list
above is complete. A debug-build stress mode, switched on by an environment
variable so that the whole existing suite runs under it unchanged, requests a
move at every safepoint. A move reorders both arenas by a permutation with no
fixed point and rewrites every traced handle: a compacting collection that
treats everything as live. Every handle also carries, in a debug build and
beside the origin it already carries, the heap's move epoch, and every accessor
compares the two, so a handle the tracer missed panics at its first read
instead of perhaps printing a wrong answer. The permutation checks the
forwarding; the epoch checks the completeness. It builds the tracing and
forwarding the collector will need first, under the suite that pins every
program's outcome. V8's `--stress-compaction`, SpiderMonkey's compacting GC
zeal mode and HotSpot's `ScavengeALot` exist for the same reason.

Step 2b's gates are the ordinary suite, the same suite with the stress mode on
(`cargo test --workspace` under the variable), `run-corpus` under it with both
lowerings, and a before-and-after measurement of the three benchmarks on a
release build for the poll, which is the only part a release build contains.
The stress run joins the commands `CLAUDE.md` requires.

### Considered and rejected: Nova's scopes

Nova's `GcScope`/`NoGcScope` with `Scoped` roots was this section's plan until
2026-09-13. It roots what a Rust frame holds across a call that can collect, and
it would have covered the four re-entry paths without removing them. It was
rejected because the paths are to go anyway — core-library's rule that a native
must not call back into Newspeak, and suspension and the debugger, need them
gone ([core-library.md](core-library.md), [image-format.md](image-format.md),
"Planned: suspend and resume") — and once they are gone Nova's machinery is
left standing with nothing to root, its `unbind` a hole in the static guarantee.
A `Scoped` value is also retained for the whole call, past its last use, which
the brand never does. Engineering effort was not the constraint, so the brand is
built directly rather than after it.

### Considered and rejected: conservative stack scanning

Scanning the Rust stack conservatively and *pinning* whatever ambiguous words
point at is the other way to avoid a handle discipline, and it is what V8 has
been moving toward: it ships in Oilpan, and pages with incoming ambiguous edges
go into a quarantine that is not evacuated. It was rejected here for two reasons.
Pinning is exactly the capability `become:` needs to not have, and after a decade
of work it is still not deployed in V8's own JavaScript heap — which is a fair
signal about how much of the difficulty is hidden.

### What this leaves for the collector itself

With the arena in place and roots checked, the collector's own root set is the
list in "What the collector traces" above. Most of it is two things: the
bootstrap, standing in for upstream's object store, and the activation stack
([execution.md](execution.md), "Activations") — a contiguous, precisely typed
pair of arrays rather than a pointer-chased chain of individually allocated
frames. Step 2b leaves the collector the safepoint, the tracing and the
forwarding; what it adds is liveness, reclamation, the allocation counter that
requests a collection, and the spaces — a nursery, a large-object space.

Closure conversion ([execution.md](execution.md), "Planned: closure
conversion") is scheduled directly before the collector for the collector's
sake. Until it lands, a block that outlives its method keeps that method's
whole reified frame alive — every register, including ones the block never
reads — so the collector would retain more than the program can reach through
its variables. After it, a surviving block holds exactly what it captured.

### Open: what the collector owns

- **Neither arena reclaims a slot today**, and that is the interim cost the
  milestone's ordering accepts. It is measurable. Peak working set on a release
  build, before the arena, after it, and after the activation stack; the last two
  columns were measured against each other in one sitting on 2026-09-10, and the
  arena figures reproduced exactly the ones taken when the arena landed:

  | Command | Entry sends | Before the arena | After the arena | After the activation stack |
  |---|---|---|---|---|
  | `run`, `bench_loop.ns` | 1 | 4.6 MB | 4.6 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_loop.ns` | 22 | 4.6 MB | 340.7 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_send.ns` | 22 | — | 324.9 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_block.ns` | 22 | — | 966.9 MB | 446.0 MB |

  Conditions, because the metric is platform-specific and the numbers are
  useless without them: Windows 11, release build, **peak working set**
  (`PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`) read after the process exits.
  PowerShell's own `Process.PeakWorkingSet64` returns 0 once the process has
  exited on this platform, so the figures come from `psapi!GetProcessMemoryInfo`
  called on the still-open handle. On Linux the comparable metric is
  `VmHWM`/`ru_maxrss`; expect different absolute numbers and the same shape.

  The shape is what matters more than the figures. An activation that nothing
  reified leaves nothing behind, so a program that creates no block per
  iteration now runs in the same 4.6 MB however many activations it builds —
  `bench_loop.ns` and `bench_send.ns`, whose blocks are created once, before
  the loop starts. An `ifTrue:`/`and:`/`whileTrue:` argument the compiler
  inlines creates no block at all, so it reifies nothing either: 300,000 calls
  of `clip: x = ( x < 0 ifTrue: [^0]. ^x + 1 )` peak at 4.6 MB (2026-09-11,
  under the conditions above).

  **What is still unbounded is everything in an arena: every reified frame and
  every object the process has ever created**, not what is live.
  `bench_block.ns` shows it: every iteration creates a block, so every
  iteration reifies one frame and allocates one closure, and nothing gives
  either back. Its peak roughly halved, because the block's own
  activation no longer takes an arena entry, but it still grows with the work
  done. A long-running process that creates blocks is therefore unbounded until
  the collector exists, and closure conversion (above) is what shrinks each
  reified frame's share.

  Peak does not climb smoothly with the work done. Sweeping `bench
  --iterations` over `bench_loop.ns` on the arena binary, before the activation
  stack, when every activation took an arena entry:

  | `--iterations` | 1 | 2 | 4 | 8 | 10 |
  |---|---|---|---|---|---|
  | Peak | 46.4 MB | 88.4 MB | 103.7 MB | 183.2 MB | 340.7 MB |

  The step shape is **consistent with** the arena's `Vec` doubling: the copy at
  a doubling point has the old and the new buffer resident at once, and would
  stall the run that triggers it. The benchmark maxima, measured in the same
  sitting as the last column above, do not contradict it, but they do not
  single it out either: they moved with retention. On `bench_send.ns`, whose
  retention the stack removed (324.9 MB to 4.6 MB above), the roughly doubled
  maxima went away — 40.4–40.8 ms over a 23.5 ms median under the arena,
  20.8–21.6 ms under the stack. The same happened on row two of the send
  decomposition ([execution.md](execution.md), "What the activation stack
  moved"); its peak working set was not measured, so that its retention went
  too is inferred from its shape, which creates no block per iteration.
  `bench_block.ns`, which still grows both arenas every iteration, still has a
  maximum well above its median: 63.8–67.9 ms over medians of 37.6–39.9 ms in
  all six `bench` runs of the 2026-09-11 before-and-after measurement of the
  retirement. `bench` reports only the median, minimum and maximum, so how many
  runs were slow is not measured. Pre-sizing the arenas with `with_capacity` is
  the control that would settle the doubling explanation, and it has not been
  run. This is recorded because a collector's design should know the interim
  may have a latency cost as well as a memory one.

  A corollary the collector's design should know about, stated carefully because
  the careless version is wrong. A **failed** image decode holds its placeholders
  until the load returns. `decode_graph_section` allocates `object_count` objects
  and `frame_count` reified frames up front, because a record must be addressable
  before the records that reference it are read, and only then validates each
  record; nothing reclaims a slot, so a rejected image cannot give them back
  part-way through.

  It is not retained past that. `image::load` owns its `Interpreter` in a local,
  so every `Err` path drops it and both arenas with it — the cost is a transient
  peak during loading, not a leak for the process's life.

  What is worth recording is the *amplification*, and it compounds in a way the
  obvious reading misses. `decode_graph_section` bounds each count with
  `bounded_count`, but both calls divide the **same** `r.remaining()` — only the
  two count words separate them — so one file can max out both at once rather
  than trading one against the other. Against the bytes remaining at the graph
  section, not against the whole file:

  | | `Object` | Frame-arena entry | Combined |
  |---|---|---|---|
  | Record minimum | 6 B | 18 B | — |
  | Release size | 56 B → 9.3x | 136 B → 7.6x | **~16.9x** |
  | Debug size | 80 B → 13.3x | 144 B → 8.0x | ~21.3x |

  A frame-arena entry is a `FrameSlot`: the `Frame` (128 B in a release build,
  136 B in a debug one) and the `live` marriage field beside it, 8 bytes more.

  `bounded_count` is what keeps this proportional to the input at all rather
  than to a 32-bit count — without it a crafted `u32::MAX` would allocate
  billions of placeholders. The residual factor is the price of allocating
  before validating, which the format's back-references require.

  This is also *better* than the model it replaced, which is the part that makes
  it an accepted cost rather than a regression. Under `Rc<RefCell<_>>` each
  placeholder was its own heap allocation with the same lifetime, plus a
  refcount and an allocator round trip per record; the arena makes the same
  placeholders two contiguous `Vec` pushes. The collector's interest here is the
  peak, not a lifetime bug.
- **Whether an application cache entry must be weak.** Applications are shared and
  cached globally; nothing collects them today because nothing collects at all.
  The collector has to decide whether an application whose classes are all
  unreachable is itself collectable.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. That is
  the same knot as `Mixin`'s lack of interior mutability; see
  [liveness-and-reflection.md](liveness-and-reflection.md).
- **MMTk, or a collector built here.** `mmtk-core` is the one Rust collector
  framework that moves objects — SemiSpace, Immix, GenImmix — and it has
  bindings for OpenJDK, Julia and Ruby. It works on addresses
  (`ObjectReference`), so adopting it would replace the arena of 32-bit indices,
  and the runtime would supply the object model and root scanning through its
  binding trait. The Rust crates that do not move (gc-arena, safe-gc, rust-gc,
  the reference-counting ones) are no candidates, because `become:` needs
  forwarding. The root discipline is the same under either answer; the
  question is step 5's.

## Decisions

- **Identity hashes come from a counter, not an address, and are assigned on
  first request**, so identity survives moving and snapshotting — except, for
  now, the hash of an object an image names by tag rather than by record
  ([image-format.md](image-format.md), "Identity hashes are preserved
  verbatim").
- **A handle is an arena-relative index, not a globally unique name**, and a
  `debug_assertions`-only origin guard says so at every accessor rather than
  leaving it to convention. It exists because the failure it catches is silent:
  a handle from another heap is in range and names a valid object, so the read
  succeeds with the wrong answer. It costs nothing in a release build, where the
  origin field does not exist. An `ActivationRef` adds a generation to the same
  guard, for the same reason: a stack index is reused once its entry is popped.
- **Activations live on a stack, not in the arena**, so that what nothing
  captures is not retained. Only a reified frame reaches the arena, and it is
  the one activation form anything may store.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements. Conservative scanning with pinning was considered
  and rejected on the same grounds.
- **Iterative `Drop` rather than a depth ceiling** for `MixinApplication`'s
  chain, because that depth is bounded by nothing the front end enforces, so a
  ceiling would exclude valid programs. Where a depth *is* independently
  bounded — the `Mixin` declaration tree, capped by the parser — a ceiling is
  the right answer and `image/mixins.rs` uses one. Objects and activations need
  neither: an arena entry holds indices, so there is nothing to unwind.
- **A collection happens only at a safepoint in the run loop**, never inside an
  allocation (decided 2026-09-13), so no allocating Rust path roots anything
  and a JIT keeps its freedom across allocations. Cog, Nova, CPython and
  piccolo put it there; V8 and HotSpot's runtime pay for the alternative with
  handles and verifiers.
- **No Rust frame is live below a safepoint**: nothing re-enters the run loop
  from Rust, so there is nothing to root but what the host keeps. Chosen over
  rooting those frames with Nova's `Scoped` values, because the re-entries are
  to go anyway.
- **The root discipline is enforced by the type system**, not by convention: a
  generative lifetime brand, gc-arena's rather than Nova's scopes, with no
  public `unbind`. The ergonomic cost is accepted rather than traded away.
- **The roots' completeness is checked by moving everything**, in a debug-build
  stress mode, before a collector exists, because the brand cannot see a handle
  stored somewhere the tracer does not know about.
