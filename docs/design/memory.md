# Memory — object lifetime, identity, collection

> **Status:** partial. Objects live in `Rc<RefCell<_>>` and reference cycles
> leak; there is no collector. Identity, layout ownership and the iterative
> teardown a collector-free heap needs are built.
> **Code:** `crates/ns-runtime/src/types.rs` (`Value`, `Object`, the `Drop`
> implementations), `mixin.rs` (layout).

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

## Planned: an arena, handles, and a real collector

Replacing `Rc<RefCell<_>>` with an arena, a tagged machine-word value
representation and a real collector is the next substantial change here.

**The collector has to move objects.** `become:`-style identity swap and object
forwarding are requirements, not conveniences: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically swaps identity. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

### What a moving collector costs each engine

A moving collector must not only *find* every root but *update* it, and the two
engines are in very different positions.

**The VM keeps every intermediate in the frame.** `run_code` writes each result
through `write_local` into `frame.temps`, and frames are heap objects chained by
`sender` and `home`. A precise collector reaches every root of a running
activation by walking that chain — which is what the reified frame chain exists
for.

**The tree-walker keeps intermediates in Rust locals.** `eval_expr` returns a
value up the Rust stack, a dynamic array literal collects into a `Vec<Value>`
there, and `eval_send` holds the evaluated receiver in a Rust variable across
`eval_args`, which allocates, and then holds receiver and arguments across the
send itself. Under `Rc` those references keep their objects alive and never need
updating. Under a moving collector they are roots it cannot see and addresses it
cannot rewrite.

Keeping the tree-walker past that point is possible but not free: it needs a
handle layer — a shadow stack in the shape of V8's `HandleScope` or JNI local
references — registering every intermediate in a heap-side list the collector
scans and fixes up. The mechanism is well understood. The cost is that the
discipline has to hold across all of `interpreter/eval.rs` and
`interpreter/instantiate.rs`, be re-established on every later change, and that a
missed handle is heap corruption visible only under collection pressure.

The rest of the swap is not the problem: an arena and handles hit both engines
the same way, and the shared orchestration in `interpreter/` is ported once.

### Decision: the tree-walker is kept and gets the handle layer

The extra engineering was weighed and accepted deliberately. Two independent
engines that must agree byte for byte are a verification property worth a
permanent cost, and V8 is the existence proof that an interpreter can hold its
roots in handles next to a moving collector without the design coming apart.

So this is not a choice between the collector and the tree-walker. Root
discipline is implemented for both: the reified frame chain is the root set for
the VM, a handle layer is the root set for the tree-walker's Rust-stack
intermediates, and the collector scans and updates both.

This does not extend to images. The tree-walker executes the AST, so it runs only
when execution starts from `.ns` source, where the AST is present by
construction. A loaded image runs bytecode. That asymmetry is the boundary of
what an AST engine can do, not a step toward retiring it.

### Open: what the collector owns

- **Whether an application cache entry must be weak.** Applications are shared and
  cached globally; nothing collects them today because nothing collects at all.
  The collector has to decide whether an application whose classes are all
  unreachable is itself collectable.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. That is
  the same knot as `Mixin`'s lack of interior mutability; see
  [liveness-and-reflection.md](liveness-and-reflection.md).
- **A trampolined VM is a prerequisite**, so that a running activation's roots
  are reachable from the frame chain alone rather than from the Rust stack. See
  [execution.md](execution.md).

Upstream's own collector takes exactly two roots — the object store and the
current activation — which is the shape this one should end up with too, with
the bootstrap standing in for the object store.

## Decisions

- **Identity hashes come from a counter, not an address**, so identity survives
  moving and snapshotting.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements.
- **Iterative `Drop` rather than a depth ceiling**, because a ceiling would
  exclude valid programs and the input controls the depth either way.
- **The tree-walker survives the collector** with its own handle-based root
  discipline.
