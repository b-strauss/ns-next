# Liveness, reflection, and concurrency

> **Status:** planned. Nothing in this document is built. What *is* built is the
> groundwork every part of it depends on — reified activations, the safepoint
> statement→program-counter table, stable call-site identity, source and provenance retention,
> identity hashes — each described in the document for its own aspect.
> **Code:** none yet.

This is the constraints document. It binds every milestone even though it
implements none of them: the point is that the runtime's shape must not foreclose
these, and several earlier decisions exist only because of requirements recorded
here.

## Requirements

1. **Maximum liveness.** Classes, methods, activations, the compiler and the
   module loader are all reified and manipulable at run time. There is no hard
   compile-time/run-time boundary: loading a module is evaluating it in the live
   system.
2. **Edit-and-continue debugging.** The debugger changes existing code at run
   time with no restart. A method with local temporaries can be edited *while an
   activation of it is live on the stack*, including adding new locals, and that
   same activation continues in the new version.
3. **Mirror-based reflection with a MirrorBuilder.** Reflection is
   capability-style mirrors onto classes, objects and activations. A
   MirrorBuilder mutates classes at run time — add and remove instance slots and
   methods, change superclasses — while instances are already live in the heap.
4. **Portable, serializable images.** A snapshot of the live world must resume in
   the same logical state on a *different CPU architecture*.
5. **Images are optional; source is first-class.** Working from source with no
   image at all is fully supported — unlike classic Smalltalk, which had no
   source files and trapped all state in the image.
6. **Performance must eventually approach Java.** Not soon, but the architecture
   must not foreclose it.
7. **Real concurrency, not one global event loop.** Genuine parallelism across
   cores, with raw shared-memory threads explicitly not the goal.

## The unifying principle

**The bytecode and interpreter level is the single source of truth for program
state.** Any JIT is strictly an accelerator that must always be able to
reconstruct interpreter-level frames — it must support deoptimization.

This one decision is what makes requirements 2, 3, 4 and 6 compatible rather than
contradictory. There is one mechanism — **an abstract-state map recorded at every
safepoint**: bytecode index, virtual temp values, receiver, arguments, sender
link — and it has four consumers:

| Consumer | Uses the map to… |
|---|---|
| Debugger | reconstruct an inspectable, editable view of a suspended activation |
| JIT deopt | rebuild an interpreter frame when a speculation is invalidated |
| Image snapshot | serialize a running activation architecture-neutrally |
| MirrorBuilder | reach activation state during class mutation |

So the debugger, the MirrorBuilder and snapshotting are clients of **one
reflective core** over the reified interpreter model. Snapshot means deoptimize
everything to interpreter level, then serialize the logical graph. Load means
deserialize, begin interpreting, re-JIT as code warms. Entering the debugger on a
frame forces it, and usually its callers, to deoptimize first — debugging and
JIT-optimizing the same frame simultaneously is explicitly not a goal.

## Caches and their dependency keys

Method dictionaries are mutable at run time, so dispatch resolves through the
dictionary or through a cache that is invalidated when the dictionary changes.
**Never bake a method-body pointer into anything durable.**

Every cache — inline caches, global lookup caches, later JIT speculative
assumptions — carries a **dependency key**. A "class shape changed", "method
changed" or "superclass changed" event flushes every cache keyed to the affected
class, and for a superclass change to its whole subtree.

The key is the **mixin application**, not the class object; see
[dispatch.md](dispatch.md) for why the obvious choice is wrong. Compiled
initializers must be flushed along with everything else on a shape change,
because `InitializeSlot` is layout-dependent.

The inline caches ([dispatch.md](dispatch.md), "3b — The caches") are built
for this: an entry holds no handle, so invalidating all of them
(`Interpreter::invalidate_caches`) is resetting the heap's cache table and
emptying its global cache, with every code family's range kept for the
activations still running; a debug-build stress mode,
`NS_NEXT_STRESS_CACHES`, does exactly that at every safepoint. Nothing calls
it outside that mode and the tests yet, since nothing mutates a method table
at run time. Scoping an invalidation to
the applications a change affects comes with the first mutation that needs
it. The dispatch id a cache guards on is a copy in the object header
([dispatch.md](dispatch.md), "The dispatch id"), so `become:` must exchange
it along with the class, and a class change must restamp it, as
`Heap::set_class_application` already does when a class's application
changes; otherwise a cache hit would run the old class's method on the
object.

## Debugger

1. **Suspend.** An uncaught error or a breakpoint reifies the activation chain as
   a chain of context mirrors.
2. **Inspect.** Read receiver, arguments, temps and the sender chain through
   mirrors.
3. **Edit and continue.** Recompile the edited method — new bytecode, new temp
   count, new statement→program-counter table. For every affected context, the active one and
   any pending frames running the same method, grow `temporaries` if slots were added,
   set the new ones to nil, and remap the program counter through the statement→program-counter table.
   Growing an activation requires taking it off the stack into its reified
   frame first, because a running activation is an entry in a stack where an
   entry in the middle cannot grow ([execution.md](execution.md), "How this
   meets the liveness requirements"). That is the same move as forcing a frame out
   of JIT-optimized form before debugging it, one level down, so the requirement
   is met by the existing principle rather than in tension with it.
   **Resume only at statement boundaries**, which is what keeps program-counter remapping
   tractable: restart the current statement, or the whole method with receiver
   and arguments preserved, rather than resuming mid-expression.
4. **Other context-mirror operations:** restart frame, force return value, step
   into/over/out.
5. **Breakpoints** by bytecode patching, or by a per-method debug flag that
   forces interpretation with a per-statement callback.

Two things already in place serve this and are not to be regressed: the
statement→program-counter table on every `CompiledCode`, and a chain of running activations
walkable independently of the Rust stack — the activation stack — on which the
trampoline runs every send without recursing through the Rust stack
([execution.md](execution.md), "The trampoline"), and with no Rust frame
between the host and the loop, since nothing re-enters it from Rust ([execution.md](execution.md),
"No re-entry from Rust"). Two things still block it. Nothing can suspend an
activation yet: there is no yield path out of the run loop
([image-format.md](image-format.md), "Planned: suspend and resume"). And an
activation's `program_counter` is currently stored only at observable points, which the
debugger milestone may need to revisit.

The reified frame is not the *default* representation of an activation; it is
the one a block, and later a debugger, asks for. Nothing here loses a capability
by that: every operation in this section already begins by reifying the chain it
works on.

## Mirrors and MirrorBuilder

A **mirror** is a read/write handle onto a class, an object or an activation,
operating through the same reified model the debugger uses. Reflection as a
capability handed out by the platform, rather than a property every object
carries, is the design in `mirrors.pdf`, and it is what would let a deployed
artifact stay reflective while inspection is prevented by withholding the mirror
instead of by stripping metadata.

A **MirrorBuilder** applies class changes:

- *Add or remove an instance slot* → new layout → migrate every live instance via
  `become:` → flush caches keyed to the class.
- *Add, remove or replace a method* → update the method dictionary → flush caches
  keyed to (class, selector).
- *Change a superclass* → re-resolve lookup for the subtree, flush its caches,
  re-lay-out instances if inherited slots changed.

Edit-and-continue is then a special case: a mirror on the suspended activation,
plus a method replacement, plus the ability to resume.

**The VM mirror's class must be sealed against it.** The compiler binds a
kernel mirror send to its leaf at compile time (`Instruction::SendMirror`),
and a primitive method carries its leaf on the method, both on the ground
that the mirror's class and table are fixed at bootstrap
([dispatch.md](dispatch.md), "Primitives"). A MirrorBuilder that added or
replaced a method there would be seen by a sent mirror send and bypassed by
both, so the two paths would disagree. **Open:** how a replaced *kernel*
method is compiled. Its primitive-method mark and its `SendMirror` rest on
`Binding::MirrorSend`, which only `Kernel.ns`'s resolution records, and
recording it for an edit would hand that edit the whole Rust boundary
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary").

### The obstacle: `Mixin` has no interior mutability

Editing a live class's method table today re-applies a *fresh* mixin. Existing
instances keep their old application and go on running the old code. §6.2
requires an edit to a mixin to affect all of its applications, so this is not a
detail to paper over.

Whoever builds `MirrorBuilder` has to choose between interior mutability on
`Mixin` and a mixin-versioning scheme — and either way must decide what happens
to the application cache, which today grows without bound because every
re-application mints an entry nothing evicts.

This is also the point where the artifact format has to move: behaviour created
at run time does not round-trip through an image, because compiled code is a
table of compiler output rather than a graph of objects. See
[image-format.md](image-format.md).

## Concurrency: actors, not threads

Newspeak already has an answer, visible in the vendored corpus
(`corpus/modules/Actors.ns`, `ActorsForJS.ns`) and in the grammar: the `<-`
eventual-send operator, parsed and currently erased, is the syntactic surface of
an actor/vat model with asynchronous sends and promises, in the lineage of Mark
Miller's E.

The language-level model, as the corpus shows it: an actor never holds a direct
reference to another actor's objects, only a far reference; cross-actor sends
enqueue a pending delivery in the target's mailbox and immediately return a
promise; the promise is resolved later through a resolver; each actor runs its
own sequential mailbox loop. **This eliminates data races by construction, with
no locks in the language.**

**Why this need not be a JavaScript-style event loop.** The reference
implementations multiplex every actor onto a *single* host loop only because
their hosts — one Squeak VM, one browser thread — are themselves
single-threaded. That is a property of those hosts, not of the semantics.
`ns-next` is a fresh native VM without the constraint, so actors should get real
parallelism: one logical event loop per actor, and across actors an **M:N
scheduler** multiplexing many lightweight actors onto an OS thread pool sized to
the core count — the shape of Java's virtual threads, BEAM's process scheduler,
or goroutines. Many small genuinely parallel loops, not one global one.

How it interacts with the rest:

- **`become:` and live class mutation** — instance data is actor-isolated, which
  narrows the blast radius of a migration. Code stays shared and goes through the
  dependency-keyed invalidation above.
- **GC** — an actor is a natural per-heap scope, so pauses can stay local to one
  actor instead of the whole world, the way per-process heaps keep BEAM's pauses
  small.
- **Images** — an actor is the natural snapshot unit. A whole-world snapshot is
  the union of every actor's state plus its mailbox and in-flight promises. The
  treatment of in-flight cross-actor promises at snapshot time still needs
  design.
- **Debugger** — suspend and edit-and-continue scope naturally to one actor's
  frame chain. Whether other actors keep draining while one is paused is open;
  the answer should be yes, to keep the "not a single global loop" property under
  debugging too.

**Explicit non-goal:** raw OS threads sharing one mutable heap. Classic
Smalltalk's single-threaded green scheduler worked around exactly the hazard this
solves properly — a highly mutable, live-editable, globally reachable object
graph is treacherous under shared-memory concurrency. Narrowly scoped data
parallelism over immutable data could be a deliberate later exception, never the
default.

### Value objects exist; the boundary they cross does not

§3.1.1 defines a deeply immutable "value object" — numbers, strings, symbols,
characters, booleans, and any class inheriting `Value` whose slots are all
immutable value objects. That is the mechanism letting an object cross an actor
boundary by copy rather than by far reference (§3.7).

The notion itself is built. `Kernel.ns` declares `Value` with the value classes
beneath it; `Value` derives `=` and `hash` from an instance's state and declares
§3.1.1's identity method as a send of `=`, so a copy of a value is equal to, and
hashes like, its original — a value has no observable identity
([core-library.md](core-library.md)). Both of §3.1.1's conditions are enforced
as a value object is built: instantiating a class that inherits `Value` and
declares a mutable slot is refused, and an initializer that stores anything but
a value in such an instance's slot is refused
([object-model.md](object-model.md), "A value object's two conditions").

What is missing is the boundary: there are no actors, so nothing copies a value
across one, and a module definition is not yet a value object (§8.1). Neither is
a looser "provably non-aliased or frozen data" notion to be substituted when
concurrency arrives — the vocabulary above is the specification's own.

## Performance and the tiered model

Near-Java performance for a fully live dynamic language comes from **inline-cache
feedback, speculative optimization and deoptimization** — the V8 and HotSpot
path — not from whole-program ahead-of-time assumptions, which liveness forbids
anyway.

- **Tier 0 — interpreter.** Always correct, always the deopt and snapshot target.
- **Tier 1 — baseline JIT.** One code template per bytecode op. Candidate
  backends: copy-and-patch stencils, or Cranelift for more headroom.
- **Tier 2 — optimizing JIT.** Speculative inlining driven by IC feedback, escape
  analysis, unboxing, guarded by deopt.
- **Deopt metadata** — safepoint ↔ machine-code position ↔ abstract interpreter
  state.
- **A snapshot discards all JIT state**; warm-up re-happens after load, which is
  acceptable because snapshotting is rare. No native code ever enters an image.

## Tooling for the VM itself

Distinct from the Newspeak-level debugger, the implementation needs its own
introspection — the equivalent of `d8`: bytecode disassembly, inline-cache
transition traces, deoptimization reasons. `ns-next disasm` is the first piece of
this and exists; the rest arrives with the mechanisms it would observe.

## Decisions

- **The interpreter level is the source of truth**, and every JIT must
  deoptimize to it. This is what makes liveness and speed compatible.
- **One safepoint abstract-state map serves four consumers**, rather than a
  separate mechanism per feature.
- **Actors with an M:N scheduler**, not a global event loop and not shared-memory
  threads.
- **Images serialize the logical object graph, not heap layout**, so GC changes
  cannot break image compatibility.
