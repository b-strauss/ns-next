# Liveness, reflection, and concurrency

> **Status:** planned. Nothing in this document is built; mirrors and an
> atomic install are road step 5 ("Planned: road step 5, mirrors and atomic
> installation"). What *is* built is
> the groundwork every part of it depends on — reified activations and the
> kernel-private activation objects the exceptions are built on
> ([core-library.md](core-library.md), "Exceptions"), the safepoint
> statement→program-counter table, stable call-site identity, source and
> provenance retention, identity hashes, §3.2's reified class hierarchy — each
> described in the document for its own aspect.
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
activations still running; a debug-build stress mode, `NS_NEXT_STRESS_CACHES`,
does exactly that at every safepoint. Nothing calls it outside that mode and
the tests yet, since nothing mutates a method table at run time; road step
5's atomic install will call it after its bulk forward. Scoping an
invalidation to the applications a change affects is deferred beyond that
step. The dispatch id a cache guards on is a copy in the
object header ([dispatch.md](dispatch.md), "The dispatch id"), so `become:`
must not let a stale guard hit, and a class change must restamp it, as
`Heap::class_set_application` already does when a class's application changes;
otherwise a cache hit would run the old class's method on the object. The
planned bulk forward meets the first by construction: it is eager, so no
forwarder outlives it and every reference names the new object, which keeps
its own id, and every inline cache is flushed after it
([object-model.md](object-model.md), "Planned: `become:` as an eager bulk
forward", road step 5).

## Debugger

1. **Suspend.** An uncaught error or a breakpoint reifies the activation chain as
   a chain of context mirrors.
2. **Inspect.** Read receiver, arguments, temps and the sender chain through
   mirrors.
3. **Edit and continue.** Recompile the edited method — new bytecode, new temp
   count, new statement→program-counter table. For every affected context, the
   active one and any pending frames running the same method, give it room for
   the added slots, set the new ones to nil, and remap the program counter
   through the statement→program-counter table. A running activation is an
   entry in a stack where an entry in the middle cannot grow, so it is
   divorced into a newly allocated, larger frame object, the VM rewrites every
   reference to the old frame by a walk of every root the collector traces
   (`trace.rs`) and every heap object, the new frame allocated old and born
   remembered, and execution continues from the divorced frame, which needs a
   divorced frame put back on the stack ([execution.md](execution.md), "How
   this meets the liveness requirements"; [object-model.md](object-model.md),
   "The object layout"). That is the same move as forcing a frame out
   of JIT-optimized form before debugging it, one level down, so the
   requirement is met by the existing principle rather than in tension with
   it. **Resume only at statement boundaries**, which is what keeps
   program-counter remapping tractable: restart the current statement, or the
   whole method with receiver and arguments preserved, rather than resuming
   mid-expression.
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
instead of by stripping metadata. §8.2 (`newspeak-spec.pdf` p. 47) asks of
mirrors that "multiple reflective changes to the program can be applied
simultaneously and atomically", transactionally, that it is possible to
change the class of an individual object, and access to the current
activation, and calls mirrors "capabilities for reflection".

A **MirrorBuilder** applies class changes:

- *Add or remove an instance slot* → new layout → migrate every live instance
  through one bulk forward → flush the caches.
- *Add, remove or replace a method* → update the method dictionary → flush the
  caches.
- *Change a superclass* → re-resolve lookup for the subtree, flush its caches,
  re-lay-out instances if inherited slots changed.

### Planned: road step 5, mirrors and atomic installation

Would change: a minimal MirrorBuilder exists, with an atomic install, as road
step 5 of the road to a high-performance VM, whose building block is
`become:` as an eager bulk forward ([object-model.md](object-model.md),
"Planned: `become:` as an eager bulk forward"), so the step ends with the
forward's real consumer (decided with the user on 2026-10-05). It follows
Primordial Soup's `Mirrors.ns` (`prior-art/primordialsoup/newspeak/Mirrors.ns`,
`installAll` at `:309-326`) and the canonical Newspeak's
`prior-art/newspeak/AtomicInstaller.ns`. Its scope:

- **A builder that stages edits apart from the live system**: nothing a
  program can observe changes until the install.
- **Edits of a class's instance slots** -- adding and removing -- **and of its
  methods** -- adding, removing and replacing.
- **An atomic install** (§8.2) that reaches every application of an edited
  mixin and their subclasses, as §6.2 requires of an edit to a mixin. A class
  object keeps its identity; the VM writes its fields. Instances whose layout
  changed are migrated: each is allocated in the new shape and its slots
  copied by name, a slot the edit added is `nil` and its initializer does not
  run, the identity hash is kept, and the migration sends nothing to a user
  object; then one bulk forward replaces every migrated instance at once, and
  the inline caches are flushed, all of them. Only instances are forwarded,
  never a class object or a mixin ([object-model.md](object-model.md),
  "Everything stays editable").
- **Every edit to a shipped mixin is refused** -- a mixin of a shipped
  core-library module, `Kernel.ns` and the mirrors module among them --
  which also seals the VM mirror's class ("The VM mirror's class must be
  sealed against it" below) and keeps every object on a path from shipped
  code to the VM mirror out of the forward.
- **A mirror capability a program obtains from the platform**, which never
  exposes the VM mirror; the exact accessor is for the step's plan.
- **A read side**: at least a `ClassMirror` -- its name, slots, superclass,
  mixin and methods -- and an `ObjectMirror` -- its class and a slot read.
- **An edited program is written to an image and loads back edited**: a
  mixin a MirrorBuilder installed, with its compiled code, lands in a
  serialized image like any mixin a module declared (decided with the user
  on 2026-10-05; "Planned with road step 5: an edit replaces the mixin
  under its id" below; [image-format.md](image-format.md), "Planned:
  edited mixins in the image").

Left to the step's plan: how removing a slot recompiles a mixin whose
declaration is absent, as a decoded mixin's is (`Mixin::declaration` is
`None`), and the questions [image-format.md](image-format.md), "Planned:
edited mixins in the image", leaves open. Deferred beyond the step: a
shape-changing `setClass:` ([object-model.md](object-model.md), "Planned: `become:` as an
eager bulk forward", "Why eager"), changing a superclass, edits of nested
classes, `installAtomically:` over several builders, editing kernel methods
(the open question below), `ActivationMirror` and edit-and-continue, and
scoping the invalidation ("Caches and their dependency keys" above). The step
has no implementation plan yet.

Edit-and-continue is then a special case: a mirror on the suspended activation,
plus a method replacement, plus the ability to resume.

**The instance-of relation a class mirror needs exists.** A mirror tells a
metaclass from a class by `Metaclass = classOf: reflectee`, which needs every
class object's class to be its metaclass and every metaclass's class to be
`Metaclass`, as §3.2 says. Both hold: `Behavior`, `Class` and `Metaclass`
are declared classes of `Kernel.ns`
([object-model.md](object-model.md), "Class sides and metaclasses"). The
classes themselves answer only `name` and `printString`; `superclass`,
`selectors` and the rest are the mirror's to answer, so holding a class
grants no reflective capability.

**The VM mirror's class must be sealed against it.** The compiler binds a
kernel mirror send to its leaf at compile time (`Instruction::SendMirror`),
and a primitive method carries its leaf on the method, both on the ground
that the mirror's class and table are fixed at bootstrap
([dispatch.md](dispatch.md), "Primitives"). A MirrorBuilder that added or
replaced a method there would be seen by a sent mirror send and bypassed by
both, so the two paths would disagree; road step 5's refusal of every edit
to a shipped mixin is what seals it. **Open:** how a replaced *kernel*
method is compiled, should editing kernel methods ever be offered. Its
primitive-method mark and its `SendMirror` rest on
`Binding::MirrorSend`, which only shipped core-library source's
resolution records, and
recording it for an edit would hand that edit the whole Rust boundary
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary").

### Planned with road step 5: an edit replaces the mixin under its id

Would change: a `Mixin` stays an immutable Rust value outside the object
space, and its identity is its `MixinId`, the row of the heap's mixin table
(`heap/tables.rs`); an edit builds a fresh, immutable `Mixin` and installs it
under the old mixin's id, so everything that names the mixin by id sees the
edited one at once (decided with the user on 2026-10-05).

Today nothing edits a live class: only the kernel's bootstrap and tests call
`Heap::class_set_application`, and the tests' `install_mixin`
(`tests/image.rs`) re-applies a *fresh* mixin, which re-points one class
object and leaves the old application's other class objects, every subclass
and every existing instance running the old code. §6.2 requires an edit to a
mixin to affect all of its applications, so the edit has to reach the
mixin's identity, not a class object.

**What Primordial Soup does.** Its installer never mutates a mixin either:
`processClassDeclaration:` builds a fresh `InstanceMixin` and `ClassMixin`
for the edited declaration and each one nested in it
(`prior-art/primordialsoup/newspeak/Mirrors.ns:383-430`), finds the old
mixin's applications through the weak `_applications` it keeps and their
subclasses (`noteUpdateOf:to:`, `:358-368`; `sortClass:`, `:542-563`),
applies the new mixins to fresh classes (`processExistingClass:`,
`:431-449`), and then forwards old mixins, classes and instances to the new
ones in one bulk become (`installAll`, `:308-327`) -- which also rewrites
the enclosing mixin's `_nestedMixins` array that still held the old nested
mixin. Its mixins are heap objects, so `become:` reaches them. In ns-next a
mixin lives outside the object space, so no forward reaches it; the
`MixinId` is the indirection that takes `become:`'s place, and installing
the fresh mixin under the old id is Primordial Soup's forward of a mixin,
done on the Rust side. A class object keeps its identity and the VM writes
its fields ([object-model.md](object-model.md), "Everything stays
editable"), so no class object is forwarded either.

**Why not interior mutability.** A `Mixin` with cells would break the
promise that a method's code is never replaced (`MixinMethod::code`,
`mixin.rs`), would let a running method observe a half-applied edit unless
the install built its own atomicity, and would put shared mutable state
where actors are to share immutable code across heaps; neither Newspeak
implementation mutates a mixin in place.

**Why the mixin stays outside the object space.** Code outside the heap is
read by the interpreter without a heap indirection, is never traced or
moved by the collector, stays put for a JIT, is shared by every application
of its mixin, and keeps the kernel's code out of every image, which is what
keeps an artifact small and a hostile one from replacing a builtin
([image-format.md](image-format.md), "How upstream compares"). HotSpot keeps
its class metadata outside the Java heap for similar reasons. What it costs
is what `become:` would otherwise give for free: an identity of its own
(the id), reclamation of its own, and a heap object to stand for a mixin
where a mirror needs one as its reflectee.

**What the id has to carry.** Some of the mixin's identity is still its
`Rc` address today, and each such place moves to the id: `Heap::mixin_id`
looks a mixin's id up by `mixin::mixin_key`, the `Rc`'s address
(`heap.rs`); a mixin holds its nested declarations as `Vec<Rc<Mixin>>`
(`Mixin::nested`, `mixin.rs`), so an edited nested mixin would otherwise
force a new enclosing mixin up to the module root; and the hidden
nested-class slot and the image encoder's row ids are keyed by
`mixin_key` too. The application cache, today strong and growing with
every re-application, gets the reuse rules of the ids and becomes weak
with this first edit (decided with the user on 2026-10-03;
[object-model.md](object-model.md), "An application has two ids";
[memory.md](memory.md), "Weak references"); a mixin that no class object
names any more is reclaimed by marking the ids that live class objects
name during a full collection, as HotSpot unloads classes.

**The image follows.** Because a mixin is a table row rather than a graph
object, an edited mixin reaches an image only if the encoder takes the
rows from the mixins the program names now rather than from what its
modules declared; that is required (decided with the user on 2026-10-05)
and described in [image-format.md](image-format.md), "Edited mixins in the
image".

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
([core-library.md](core-library.md)). All of §3.1.1's second bullet is
enforced, each condition refused where an instance would come into being: a
mutable slot anywhere in the class chain, a slot initialized with anything but
a value, a class that defines `==` below `Value`, and an enclosing object, at
any level of the class chain, that is not a value
([object-model.md](object-model.md), "A value object's four conditions"). So
a value class lives in a module that declares itself a value, `= Value` as
`Kernel.ns` does, or with §6.2's mixin application, `= Object <: Value`.

A class object is a value exactly when its enclosing object is, so a module
definition, which has none, is one (§6.4), and so is a class nested in a
value; such a class has no identity either, its `=` and `hash` derived from
its state ([object-model.md](object-model.md), "Class sides and
metaclasses").

What is missing is the boundary: there are no actors, so nothing copies a value
across one. Neither is
a looser "provably non-aliased or frozen data" notion to be substituted when
concurrency arrives — the vocabulary above is the specification's own.

## Performance and the tiered model

Near-Java performance for a fully live dynamic language comes from **inline-cache
feedback, speculative optimization and deoptimization** — the V8 and HotSpot
path — not from whole-program ahead-of-time assumptions, which liveness forbids
anyway. The tiers are road steps 7 to 9
([object-model.md](object-model.md), "Planned: the road to a
high-performance VM";
[execution.md](execution.md), "Planned: JIT tiers"), after the object
layout, generations and inline allocation that compiled code builds on.

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
transition traces, deoptimization reasons. `ns-next-analyzer disasm` is the first piece of
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
