# Memory — object lifetime, identity, collection

> **Status:** partial. Objects live in `Rc<RefCell<_>>` and reference cycles
> leak; there is no collector. Identity, layout ownership, the seam that
> routes every access through `Heap`, and the iterative teardown a
> collector-free heap needs are built.
> **Code:** `crates/ns-runtime/src/types.rs` (`Value`, `Object`, the `Drop`
> implementations), `heap.rs` (the seam), `mixin.rs` (layout).

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

**Objects are `Rc<RefCell<Object>>` and cycles leak.** This is the Milestone 1
model, accepted and documented rather than overlooked. It is not a problem the
image format inherits: the format is defined against the *logical* value model
rather than against this representation, so replacing it does not break images.

### Iterative teardown

Rust has no iterative drop glue, so releasing a deep `Rc` graph recurses and a
sufficiently deep one aborts the process. Since an image is untrusted input and
its graph depth is attacker-controlled, that is not acceptable: `Object`, `Frame`
and `MixinApplication` each have a hand-written iterative `Drop`.

The alternative — a depth ceiling — was rejected. A ceiling excludes valid
programs from working, and a Newspeak program can legitimately chain a million
arrays or a million activations.

The same reasoning applies to traversal rather than teardown: the image encoder
walks the graph over an explicit stack, and `metaclass_of` walks its chain
iteratively, because an image controls both depths.

### The seam

`crates/ns-runtime/src/heap.rs` and `crates/ns-runtime/src/types.rs` are the
only two places that know what a handle is. Every read and write of an
`Object` or a `Frame` goes through a `Heap`
accessor, so replacing the store behind them touches that file rather than
the 193 call sites (109 under `src`, 84 in the integration tests) the
workspace held before the seam existed.

The accessors copy out rather than lend out. That follows what the call
sites already did — borrow, clone one field, drop the borrow — and it is
what keeps the arena from needing a rooting discipline at every site that
calls back into the interpreter while holding a reference. Mutating
accessors take `&mut self` even though the present `Rc<RefCell<_>>` store
does not require it, so that the arena is a change of implementation rather
than a second pass over every caller.

The seam's containment is partly enforced by the type system and partly by
discipline. `ObjRef`'s and `FrameRef`'s inner `Rc<RefCell<_>>` is
`pub(crate)`, so the crate boundary is real: `ns-cli` and the
integration-test crates under `crates/ns-runtime/tests/` cannot reach it at
all, and the compiler refuses any attempt. But `types.rs` and `heap.rs` are
both plain top-level modules of `ns-runtime` (`lib.rs`'s `pub mod types` and
`pub mod heap`), sharing no private parent, so `pub(crate)` is the narrowest
visibility available — every other module in the crate, `vm.rs`,
`interpreter/`, `image/` included, can also write `.0.borrow()` and have it
compile. Containment within `src/` is therefore the grep gate this
migration ran at every step, not a compiler guarantee: nothing stops a new
call site inside the crate from reaching around the seam except someone
checking.

**Follow-up, not done now:** nesting `types` and `heap` under a shared
private parent module would let the field be `pub(in crate::that_parent)`
rather than `pub(crate)`, closing this gap for real — every module outside
the parent would then fail to compile rather than merely being asked not
to. Not done in this plan: it ripples through every `crate::types::` import
path in the crate, and the arena rewrite restructures this area anyway, so
whoever plans that rewrite should decide this deliberately rather than
inherit an unexamined gap.

Where a site needs several fields of one object or frame at once, a batched
accessor answers them from a single borrow rather than the caller taking
several — `receiver_and_current_class`, `object_snapshot`,
`frame_register_shape` and `frame_snapshot` are the present examples. This
is not only style: `dispatch_lexical_send`'s head, on every
implicit-receiver send, once took one borrow yielding both the receiver and
the current class; splitting that into two separate accessor calls measured
as a real cost, recovered by batching them back into one (see
[execution.md](execution.md)'s measurements section).

## Planned: an arena, handles, and a real collector

Replacing `Rc<RefCell<_>>` with an arena, a handle-shaped value representation and
a real collector is the next substantial change here, and it is the largest single
item in Milestone 4. It is one change, not three: the object representation, the
activation representation and the root discipline are decided together because
each one's answer constrains the other two.

**The collector has to move objects.** `become:`-style identity swap and object
forwarding are requirements, not conveniences: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically swaps identity. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

### The representation: an arena of typed handles

The model is the Nova JavaScript engine's: a `Value` is a Rust enum carrying
either its data inline or a 32-bit index into a typed arena, with no `Rc` and no
`RefCell` anywhere in it. Nova is the useful comparison rather than V8 because it
solves this problem in Rust, where the borrow checker is available as an
enforcement mechanism and a C++ engine's conventions are not.

What that removes is not an abstract elegance but the measured cost of the
present model, itemized in [execution.md](execution.md)'s measurements section:
three heap allocations per activation, reference-count traffic on every frame
link and every shared `Rc`, a `RefCell` borrow on every single register read and
write, and pointer-chasing where a contiguous array would do.

An arena is also closer to the shape the image format already writes. Handles are
architecture-neutral by construction, which is requirement 4 in
[liveness-and-reflection.md](liveness-and-reflection.md), and identity hashes
already come from a counter rather than an address, so nothing about identity
depends on where an object sits.

### The root discipline: checked by the compiler, not by convention

A moving collector must not only *find* every root but *update* it, and roots
held in Rust locals across a collection safepoint are the hard case: under `Rc`
those references keep their objects alive and never need updating, and under a
moving collector they are roots the collector cannot see and addresses it cannot
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

- **Whether an application cache entry must be weak.** Applications are shared and
  cached globally; nothing collects them today because nothing collects at all.
  The collector has to decide whether an application whose classes are all
  unreachable is itself collectable.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. That is
  the same knot as `Mixin`'s lack of interior mutability; see
  [liveness-and-reflection.md](liveness-and-reflection.md).
- **Whether a handle is globally unique or arena-relative.** Actors are designed
  to get one heap each so that pauses stay local
  ([liveness-and-reflection.md](liveness-and-reflection.md)), which makes a
  handle's encoding a question that milestone will ask. Nothing in Milestone 4
  needs the answer; it needs only to leave room for one.

## Decisions

- **Identity hashes come from a counter, not an address**, so identity survives
  moving and snapshotting.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements. Conservative scanning with pinning was considered
  and rejected on the same grounds.
- **Iterative `Drop` rather than a depth ceiling**, because a ceiling would
  exclude valid programs and the input controls the depth either way.
- **The tree-walker survives the collector.** Its Rust-held intermediates are
  covered by the scope discipline, not by retiring the engine.
- **The root discipline is enforced by the type system**, not by convention, and
  the ergonomic cost of that is accepted rather than traded away.
