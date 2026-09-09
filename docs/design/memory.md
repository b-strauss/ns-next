# Memory — object lifetime, identity, collection

> **Status:** partial. Objects and activations live in arenas owned by one
> `Heap`, addressed by 32-bit handles; identity, layout ownership and the
> seam that routes every access through `Heap` are built. Nothing reclaims
> a slot: there is no collector, and no root discipline for one to use.
> **Code:** `crates/ns-runtime/src/heap.rs` (the arenas, the handle types,
> every accessor), `types.rs` (`Value`, `Object`, `Frame`), `mixin.rs`
> (layout, and the one remaining iterative `Drop`).

## What exists

**The class owns the slot layout**, and slot access resolves through the
*current* layout, so a layout change plus instance migration is expressible
rather than something the representation forecloses. Layout lives on
`MixinApplication`; see [object-model.md](object-model.md).

**Identity is stable and hashed from a counter.** Every object carries an
`identity_hash` assigned monotonically at allocation, never derived from its
address. This is required by identity-keyed collections, by image roots, and by
`become:`; an address-derived hash would break the moment an object moved or an
identity was swapped.

**Objects and activations live in arenas, not in `Rc<RefCell<_>>`.** One
`Heap` owns `objects: Vec<Object>` and `frames: Vec<Frame>`; an `ObjRef` or a
`FrameRef` is a 32-bit index into one of them. The representation is described
below. Nothing reclaims a slot: a cycle is no longer the special case that
leaks, because *everything* is retained, deliberately, until a collector
exists. That is not a problem the image format inherits: the format is defined
against the *logical* value model rather than against this representation, so
replacing it again would not break images.

### The representation: an arena of typed handles

The model is the Nova JavaScript engine's: a `Value` is a Rust enum carrying
either its data inline or a 32-bit index into a typed arena, with no `Rc` and no
`RefCell` anywhere in it. Nova is the useful comparison rather than V8 because it
solves this problem in Rust, where the borrow checker is available as an
enforcement mechanism and a C++ engine's conventions are not.

```rust
Value  = Nil | Bool(bool) | SmallInt(i64) | Float(f64) | Obj(ObjRef)   // Copy
ObjRef   { index: u32 }   // private to heap.rs; 4 bytes in a release build
FrameRef { index: u32 }   // likewise; ClassRef is an alias for ObjRef

Heap { objects: Vec<Object>, frames: Vec<Frame>, next_identity_hash: u32 }
```

What that removed is not an abstract elegance but part of the measured cost of
the previous model, itemized in [execution.md](execution.md)'s measurements
section: the `Rc` box and its reference-count traffic on every frame link and
every shared handle, and a `RefCell` borrow on every single register read and
write. A register access is now a slice index into an owned `Vec`. Over
`bench_loop.ns` on a release build that measured as **19.4–19.6 ms before,
16.4–16.8 ms after** — about 15%. What it did *not* remove is the rest:
`Frame.args` and `Frame.temps` are still a `Vec` allocation apiece per
activation, and those belong to the activation stack
([execution.md](execution.md), "Planned: activations on a stack, reified on
demand").

`ObjRef`, `FrameRef` and `Value` are `Copy`. A handle has no destructor and
nothing to clone, which is why `Value` could become `Copy` at all, and it is
what makes an arena entry cheap to hand around.

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
of the heap that minted it, and all 46 accessor sites assert the two match. In a
release build neither the field nor the check exists, and a handle is exactly its
32-bit index.

The room the actor milestone needs is still there. Actors are designed to get one
heap each so that pauses stay local
([liveness-and-reflection.md](liveness-and-reflection.md)), which will ask how a
handle encodes *which* heap. A handle is 32 bits, its field is private to
`heap.rs`, and nothing outside that module can read its bits — so widening it to
carry a heap tag is a change to one module, and the debug guard is already the
place that knows a handle has an origin at all.

### Iterative teardown

Rust has no iterative drop glue, so releasing a deep reference graph recurses and
a sufficiently deep one aborts the process. Since an image is untrusted input and
its graph depth is attacker-controlled, that is not acceptable.

Under the arena, objects and activations have no such graph: an `Object` holds
handles, a handle holds an index, and dropping the `Heap` drops two flat `Vec`s.
The hand-written iterative `Drop` implementations they used to need are gone.

**`MixinApplication` still owns a real chain** — `superclass:
Option<Rc<MixinApplication>>` — and keeps its hand-written iterative `Drop`
(`crates/ns-runtime/src/mixin.rs`). It is the only such chain left in the crate.

The alternative — a depth ceiling — was rejected, and stays rejected for the
chain that remains. A ceiling excludes valid programs from working, and a
Newspeak program can legitimately declare a deep inheritance chain.

The same reasoning applies to traversal rather than teardown: the image encoder
walks the graph over an explicit stack, and `metaclass_of` walks its chain
iteratively, because an image controls both depths.

### The seam

`crates/ns-runtime/src/heap.rs` is the only place that knows what a handle is.
Every read and write of an `Object` or a `Frame` goes through a `Heap`
accessor, so replacing the store behind them touches that file rather than
the 193 call sites (109 under `src`, 84 in the integration tests) the
workspace held before the seam existed. Replacing `Rc<RefCell<_>>` with the
arena is the evidence that this works: **no call site outside `heap.rs` and
`types.rs` changed** for either the frame conversion or the object one.

The accessors copy out rather than lend out. That follows what the call
sites already did — borrow, clone one field, drop the borrow — and it is
what keeps a rooting discipline from being needed at every site that calls
back into the interpreter while holding a reference.

**The seam is compiler-enforced.** `ObjRef`, `FrameRef` and the `ClassRef`
alias are declared in `heap.rs` with a **private** inner field, so every
attempt to read a handle's index from anywhere else — any other module of
`ns-runtime` included — fails to compile. There is no grep gate to remember
and no visibility to widen: an accessor that is missing has to be added in
`heap.rs`, which is where the store is.

Where a site needs several fields of one object or frame at once, a batched
accessor answers them together rather than the caller taking several —
`receiver_and_current_class`, `object_snapshot`, `frame_register_shape` and
`frame_snapshot` are the present examples. This is not only style:
`dispatch_lexical_send`'s head, on every implicit-receiver send, once took one
`RefCell` borrow yielding both the receiver and the current class; splitting that
into two separate accessor calls measured as a real cost, recovered by batching
them back into one (see [execution.md](execution.md)'s measurements section).
Under the arena those are two slice indexes rather than two borrows, so the cost
batching removes is smaller than it was; whether it still pays is a measurement
nobody has taken.

### Known rough edges in the seam

Carried deliberately rather than fixed. Recorded so that whoever reshapes this
file next starts from a list rather than from rediscovery.

- **The accessor names grew by accretion.** `class_of_obj` reads an *object*
  while `class_application` and its neighbours read a *class* — the same prefix
  for opposite directions. `array_at`/`array_len` and
  `instance_field`/`instance_field_count` are parallel operations under
  divergent names. Most frame accessors carry a `frame_` prefix; `receiver`,
  `local`, `set_local`, `grow_temps` and `mark_uncontinuable` do not.
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

The arena is built. Two pieces of the representation change are not: the root
discipline, and the collector that needs it.

**The collector has to move objects.** `become:`-style identity swap and object
forwarding are requirements, not conveniences: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically swaps identity. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

### The root discipline: checked by the compiler, not by convention

A moving collector must not only *find* every root but *update* it, and roots
held in Rust locals across a collection safepoint are the hard case: under `Rc`
those references kept their objects alive and never needed updating, and under a
moving collector they are roots the collector cannot see and indices it cannot
rewrite.

The mechanism is Nova's `GcScope`/`NoGcScope`: a handle carries a lifetime that
must match the current scope, `NoGcScope` is the proof that no collection can
happen in a region, and a value that must survive a safepoint is scoped
explicitly. Holding an unrooted handle across a safepoint is then a **compile
error** rather than heap corruption that only appears under collection pressure.
It costs nothing at run time — the lifetimes are gone after monomorphization —
and its price is ergonomic: Nova's own experience report is largely a complaint
about the resulting `unbind()` noise. That price is accepted deliberately.

This is what lets the two engines keep their present shape. The tree-walker holds
intermediates in Rust locals — `eval_expr` returns values up the Rust stack, a
dynamic array literal collects into a `Vec<Value>` there, and `eval_send` holds a
receiver across argument evaluation, which allocates. Under a checked scope
discipline that is expressible and verified, rather than a discipline that has to
be re-established by hand on every later change. The same applies to every native
in `builtins/` that holds values across an allocation.

### Considered and rejected: conservative stack scanning

Scanning the Rust stack conservatively and *pinning* whatever ambiguous words
point at is the other way to avoid a handle discipline, and it is what V8 has
been moving toward: it ships in Oilpan, and pages with incoming ambiguous edges
go into a quarantine that is not evacuated. It was rejected here for two reasons.
Pinning is exactly the capability `become:` needs to not have, and after a decade
of work it is still not deployed in V8's own JavaScript heap — which is a fair
signal about how much of the difficulty is hidden.

### What this leaves for the collector itself

With the arena in place and roots checked, the collector's own root set is two
things: the bootstrap, standing in for upstream's object store, and the
activation stack described in [execution.md](execution.md) — a contiguous,
precisely typed array rather than a pointer-chased chain of individually
allocated frames.

### Open: what the collector owns

- **Neither arena reclaims a slot today**, and that is the interim cost the
  milestone's ordering accepts. It is measurable. Peak working set on a release
  build over `bench_loop.ns`, before the arena and after:

  | Command | Entry sends | Before | After |
  |---|---|---|---|
  | `run --engine=bytecode` | 1 | 4.6 MB | 4.6 MB |
  | `run --engine=tree-walk` | 1 | 4.6 MB | 25.4 MB |
  | `bench --iterations 10` | 22 | 4.6 MB | 340.7 MB |

  The shape of that is what matters more than the figures. Retention is
  proportional to every activation and object the process has *ever* created,
  not to what is live: the bytecode engine inlines `whileTrue:` and so builds
  almost no activations for this loop, while the tree-walker builds one per
  iteration and keeps all of them. A long-running process is therefore
  unbounded, and the arena's `Vec` doubling adds a second, visible cost — the
  copy at a doubling point has both buffers resident, which is why the peak
  above steps rather than climbs smoothly, and why the tree-walk median of ~37
  ms sits under a maximum of ~117 ms in the same ten timed runs.

  A corollary the collector's design should know about: a **failed** image
  decode retains its placeholders. `decode_graph_section` allocates
  `object_count` objects and `frame_count` activations up front, because a
  record must be addressable before the records that reference it are read, and
  only then validates each record. A rejected image therefore turns input bytes
  into retained memory for the rest of that `Heap`'s life. `bounded_count`
  keeps the amplification proportional to the file's size rather than to a
  32-bit count, so this is a corollary of the accepted interim cost and not a
  new decoder defect — but "the decode failed, so nothing was kept" is not true
  and a collector must not assume it.
- **Whether an application cache entry must be weak.** Applications are shared and
  cached globally; nothing collects them today because nothing collects at all.
  The collector has to decide whether an application whose classes are all
  unreachable is itself collectable.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. That is
  the same knot as `Mixin`'s lack of interior mutability; see
  [liveness-and-reflection.md](liveness-and-reflection.md).

## Decisions

- **Identity hashes come from a counter, not an address**, so identity survives
  moving and snapshotting.
- **A handle is an arena-relative index, not a globally unique name**, and a
  `debug_assertions`-only origin guard says so at every accessor rather than
  leaving it to convention. It exists because the failure it catches is silent:
  a handle from another heap is in range and names a valid object, so the read
  succeeds with the wrong answer. It costs nothing in a release build, where the
  origin field does not exist.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements. Conservative scanning with pinning was considered
  and rejected on the same grounds.
- **Iterative `Drop` rather than a depth ceiling** for `MixinApplication`, the
  one owned chain left, because a ceiling would exclude valid programs and the
  input controls the depth either way. Objects and activations no longer need
  one: an arena entry holds indices, so there is nothing to unwind.
- **The tree-walker survives the collector.** Its Rust-held intermediates are
  covered by the scope discipline, not by retiring the engine.
- **The root discipline is enforced by the type system**, not by convention, and
  the ergonomic cost of that is accepted rather than traded away.
