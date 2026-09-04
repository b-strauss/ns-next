# ns-next — Liveness, Images, and VM Architecture Constraints

Status: draft for review
Date: 2026-09-04
Author: Benjamin

## Summary

This document is a **cross-cutting constraints spec**. It does not describe a
single milestone; it states the properties the `ns-next` runtime and VM must
preserve across *every* milestone, from the Milestone 1 tree-walking
interpreter to a future optimizing JIT. It exists because these properties are
cheap to design for early and expensive — sometimes impossible — to retrofit.

`ns-next` is a native Rust runtime for Newspeak (Gilad Bracha's Smalltalk
descendant). The Milestone 1 design is in
`2026-09-01-ns-next-runtime-design.md`; the frontend (lexer, AST, parser, CLI
`parse` / `parse-corpus`) is implemented. This spec constrains the runtime crate
(`ns-runtime`) that comes next, and the VM milestones after it.

## Requirements

1. **Maximum liveness.** Classes, methods, activations, the compiler, and the
   module loader are all reified and manipulable at runtime. There is no hard
   compile-time / run-time boundary: loading a module is evaluating it in the
   live system.

2. **Edit-and-continue debugging.** The debugger changes existing code at
   runtime with no restart. In particular: a method that has local
   slots/temporaries can be edited *while an activation of it is live on the
   stack* — including adding new local slots — and that same activation
   continues in the new version of the method.

3. **Mirror-based reflection with a MirrorBuilder.** Reflection is
   capability-style mirrors onto classes, objects, and activations. A
   MirrorBuilder mutates classes at runtime — add/remove instance slots and
   methods, change superclasses — while instances of those classes are already
   live in the heap.

4. **Portable, serializable images.** A snapshot of the entire live world
   (object graph + every live activation + VM-level state) must resume in the
   exact same logical state on a *different CPU architecture*. The image is
   architecture-neutral; resumption is bit-exact at the level of observable
   program state.

5. **Images are optional; source is first-class.** It must be fully possible to
   work from source code with no image at all — unlike classic Smalltalk, which
   had no source files and trapped all state in the image. The system supports
   *both* source code and serializable images, and neither is second-class.

6. **Performance must eventually approach Java.** Not in Milestone 1, but the
   architecture must not foreclose it.

## The unifying principle

**The bytecode / interpreter level is the single source of truth for program
state.** Any JIT is strictly a runtime accelerator that must always be able to
reconstruct interpreter-level frames — i.e. it must support deoptimization.

This one decision is what makes requirements 2, 3, 4, and 6 compatible instead
of contradictory. Concretely, there is one mechanism — **an abstract-state map
recorded at every safepoint** (bytecode index, virtual temp values, receiver,
argument values, sender link) — and it has four consumers:

| Consumer | Uses the abstract-state map to… |
|---|---|
| Debugger | reconstruct an inspectable/editable view of a suspended activation |
| JIT deopt | rebuild an interpreter frame when a speculation is invalidated |
| Image snapshot | serialize a running activation architecture-neutrally |
| MirrorBuilder | (via the mirror API) reach activation state during class mutation |

The debugger, the MirrorBuilder, and image snapshot are all clients of **one
reflective core** over the reified interpreter model.

- **Snapshot** = deoptimize everything to the interpreter level, then serialize
  the logical object graph (including reified activations).
- **Load** = deserialize, begin interpreting, re-JIT hot code as it warms
  again.
- **Debugger entry on a frame** forces that frame (and usually its callers) to
  deoptimize to the interpreter first. Debugging and JIT-optimizing the *same*
  frame simultaneously is explicitly not a goal.

## Reified activations

Newspeak-level activation state must never live only in Rust locals or the Rust
call stack. Each activation is an explicit, heap-reachable structure:

```
Frame {
    receiver:  Value,
    args:      Vec<Value>,       // or folded into temps
    temps:     Vec<Value>,       // indexable AND growable
    pc:        Pc,               // explicit; statement position recoverable
    sender:    Option<FrameRef>, // the reified call chain
    home:      Option<FrameRef>, // for a block: its method activation, for non-local return
    method:    MethodRef,        // the *current* method object, re-resolved, never inlined permanently
}
```

- **`temps` is a heap `Vec<Value>` addressed by index and able to grow.** Adding
  a local slot to a running method means growing the active frame's `temps`,
  initializing the new entries to `nil`, and remapping the PC.
- The M1 tree-walker may use Rust recursion for control flow, but the Frame
  chain above must exist and be walkable independently of the Rust stack.
- From M2 on, frames may run on a native stack for speed (see Cog's
  "married/divorced contexts") but must be *reifiable on demand* into the
  structure above whenever reflection, debugging, or snapshot touches them.

## Object model

- `Object` header points to its class; the **class owns the slot layout**. Slot
  access is resolved through the *current* layout, so a layout change plus
  instance migration is expressible.
- **Stable object identity and an identity hash from the start** — required for
  `IdentityDictionary`/`IdentitySet`, for image roots, and for `become:`.
- **`become:`-style identity swap / object forwarding** must be supported.
  Adding an instance variable to a class with live instances allocates
  new-shape instances, copies fields, and atomically swaps identity. This is a
  real constraint on GC choice:
  - MMTk supports object forwarding.
  - A copying/compacting collector can implement it.
  - A conservative non-moving collector makes `become:` hard — avoid.
- `Value` is `Nil | Bool | SmallInt(i64) | Float(f64) | Obj(ObjRef)` for M1
  (`Rc<RefCell<_>>`, cycles leak — an accepted M1 limitation). A later milestone
  replaces this with a tagged machine-word representation plus a real GC; the
  image format (below) is defined against the *logical* value model, not this
  representation, so that swap does not break images.

## Dispatch and caches

- Method dictionaries are mutable at runtime. Dispatch resolves selector →
  method through the dictionary (or through a cache that is invalidated when the
  dictionary changes). **Never bake a method-body pointer into anything
  durable.**
- Every cache — inline caches, global method-lookup caches, and later JIT
  speculative assumptions — carries a **dependency key** (typically class +
  selector). A global "class shape changed" / "method changed" / "superclass
  changed" event flushes all caches keyed to the affected class, and for a
  superclass change, to its whole subtree.
- Inline caches attach to **call sites**. Every send in the resolved tree / the
  bytecode has a stable call-site identity so caches and type feedback can be
  attached and, on snapshot, discarded.

## Debugger concept

1. **Suspend.** An uncaught Newspeak error or a breakpoint reifies the
   activation chain as a chain of context mirrors (using the reified Frame
   structure above).
2. **Inspect.** Read receiver, arguments, temps, and the sender chain through
   mirrors.
3. **Edit and continue.**
   - Recompile the edited method (new bytecode, new temp count, new
     statement→PC table).
   - For every affected context — the active one and any pending frames running
     the same method — grow `temps` if slots were added, set new temps to
     `nil`, and remap the PC via the statement→PC table.
   - **Resume only at statement boundaries.** This keeps PC remapping tractable:
     the debugger restarts the current statement (or the whole method, with
     arguments and receiver preserved) rather than resuming mid-expression.
   - Continue execution — interpreted; the method re-JITs later if hot.
4. **Other operations** on a context mirror: restart frame, force return value,
   step (into/over/out).
5. **Breakpoints.** Either bytecode patching (replace an op with a trap op) or a
   per-method "debug mode" flag that forces interpretation with a per-statement
   callback.

## Mirrors and MirrorBuilder

- A **mirror** is a read/write handle onto a class, an object, or an activation,
  operating through the same reified model (slot layouts, method dictionaries,
  Frame structures) that the debugger uses.
- A **MirrorBuilder** applies class changes:
  - *Add/remove instance slot* → new slot layout on the class → migrate every
    live instance (`become:`) → flush caches keyed to the class.
  - *Add/remove/replace method* → update the method dictionary → flush caches
    keyed to (class, selector).
  - *Change superclass* → re-resolve method lookup for the subtree, flush its
    caches, and re-layout instances if inherited slots changed.
- The debugger's "edit and continue" is a special case: a mirror on the
  suspended activation plus a method replacement plus the ability to resume.

## Image format

**Not** the classic "image is a snapshot of VM memory / `mmap` the file"
approach — that cannot be cross-architecture. The image is an **explicit,
self-describing, architecture-neutral serialization format that is decoded on
load**, never a memory dump.

- No native code, no JIT-compiled machine code, no raw pointers, no baked-in
  pointer tagging or host endianness in the image.
- **Bytecode is architecture-neutral and versioned**; it is part of the image.
- Canonical encodings: little-endian integers, IEEE-754 `binary64` for floats,
  UTF-8 for strings (interned on load), a fixed `SmallInt` value range (`i64`
  per the M1 spec).
- Preserve identity hashes. Preserve stable identities for well-known objects
  (`nil`, `true`, `false`, the base classes, the class table) via a
  root / special-objects array at a fixed position in the image.
- Serialize **reified activations** (the Frame chain, at bytecode level) so a
  running world — including an open debugger session — round-trips.
- Version the format; refuse or migrate on a version mismatch.
- **Bootstrap the initial image from source** — build the base classes and
  well-known objects from `.ns` plus Rust primitives (as modern Pharo does),
  rather than hand-growing an image that then becomes irreproducible.

## Source and images together

Newspeak modules are already files upstream (unlike Squeak), so `ns-next` is
well positioned for a file-first model.

**Source files are the system of record for code. Images are snapshots of
running state** (heap + activations + whatever code has been loaded or modified
live).

- Every class and method retains its source text and a **provenance record**:
  where the source came from (file + span) or "entered in the debugger at time
  T".
- An image contains source too and can always be **exported back to `.ns`
  files**.
- "Changed live but not yet written back to a file" is a **visible, first-class
  state** — surfaced by tooling, not hidden. This is the specific failure of
  classic Smalltalk that this design rejects.
- A session may be configured to **auto-write live source changes back to
  files** (Tonel/Git-style), so the file tree stays authoritative.
- Working with no image at all is the normal case: `ns-next run` / a live
  session that loads modules from files and never takes a snapshot.

## Performance and the tiered execution model

Near-Java performance for a fully-live dynamic language comes from **inline-cache
feedback + speculative optimization + deoptimization** (the V8 / HotSpot path),
not from whole-program ahead-of-time assumptions — which liveness forbids
anyway.

- **Tier 0 — interpreter.** Always correct, always the deopt and snapshot
  target. Bytecode from M2.
- **Tier 1 — baseline JIT.** One code template per bytecode op. Candidate
  backends to evaluate: copy-and-patch stencils (simple, ~baseline quality) or
  Cranelift (more work, more headroom).
- **Tier 2 — optimizing JIT.** Speculative inlining driven by IC feedback,
  escape analysis, unboxing; guarded by deopt.
- **Deopt metadata** (safepoint ↔ machine-code position ↔ abstract interpreter
  state) is carried from M2 onward.
- A **snapshot discards all JIT state**; warmup re-happens after load. This is
  acceptable — snapshotting is rare.
- The GC (generational, compacting, `become:`-capable; MMTk a strong candidate)
  is a pure runtime concern. **The image serializes the logical object graph,
  not the heap layout**, so GC changes do not affect image compatibility.

## What to build in, and when

| Milestone | Constraint work |
|---|---|
| **M1** — tree-walking interpreter | Reified Frame chain, walkable independently of the Rust stack; growable indexed `temps`. Dispatch through mutable method dictionaries; no durable method-body pointers. Class-owned slot layout; slot access via current layout. Stable identity + identity hash. Retain source + provenance per class/method. |
| **M2** — bytecode + interpreter VM | The safepoint abstract-state map (debugger + deopt + image). Statement→PC table per method. Architecture-neutral, versioned bytecode. Define and version the image serialization format; bootstrap from source. Stable call-site identity. |
| **M3** — inline caches + GC | Dependency-keyed, globally-flushable caches. `become:` / forwarding support in the GC and object model; instance migration. |
| **M4–M5** — JIT | Deopt metadata from every safepoint. Tiered promotion driven by IC feedback. Snapshot tears down JIT state cleanly. |

## Tensions and non-goals

- **Debugging and JIT-optimizing the same frame at once** is not supported;
  debugger entry deoptimizes the frame.
- **Whole-program AOT optimization** is out — it is incompatible with runtime
  class mutation.
- **OS-level process snapshotting (CRIU and similar)** is *not* the image
  mechanism: it is architecture- and build-specific. Images operate at the
  managed-heap / bytecode level only.
- **Milestone 1** still uses `Rc<RefCell<_>>` (cycles leak) and `i64` integers
  (overflow raises) per the M1 spec. This document does not change that; it only
  requires that the M1 object model expose stable identity and a revisable slot
  layout so the later swap to a real GC and a tagged representation does not
  break images or reflection.

## Prior art

- **Cog** (Pharo/Squeak JIT) — "married/divorced contexts": frames run on a
  native stack but are lazily reified into heap context objects when reflection
  or debugging touches them.
- **Pharo Spur** — portable image format and bootstrap-from-source;
  **Fuel / Ston** — object serialization shape.
- **TruffleSqueak / GraalSqueak** — full Smalltalk debugging together with
  near-JVM performance via Graal partial evaluation and deopt: existence proof
  that requirements 2 and 6 combine.

## Open questions

- Register vs. stack bytecode for Newspeak's uniformly message-based semantics.
- Whether M1 should already emit a trivial bytecode (to exercise the
  abstract-state-map design) rather than pure AST walking.
- Image granularity: one monolithic world image only, or also partial images /
  module-scoped snapshots.
- Identity-hash width and its interaction with a future tagged-pointer
  representation.
- Concurrency model (actors are a later milestone) and its effect on what "the
  entire live world" means for a snapshot.
