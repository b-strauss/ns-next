# Liveness, reflection, and concurrency

> **Status:** mirrors that read are built, road step 5a ("Mirrors that
> read"): `platform mirrors` hands out `ObjectMirror` and `ClassMirror`,
> through which a program reads the structure of its classes, mixins,
> methods and objects, and no mirror hands out a value it reached through
> a shipped level. **Planned:** a MirrorBuilder and an atomic install, road
> step 5b ("Planned: road step 5b, MirrorBuilder and atomic
> installation"); the debugger, edit-and-continue and actors after it.
> What else is built is the groundwork every part of it depends on —
> reified activations and the kernel-private activation objects the
> exceptions are built on ([core-library.md](core-library.md),
> "Exceptions"), the safepoint statement→program-counter table, stable
> call-site identity, source and provenance retention, identity hashes,
> §3.2's reified class hierarchy — each described in the document for its
> own aspect.
> **Code:** `crates/ns-runtime/core-library/platform/MirrorsFactory.ns`
> (the mirrors), `crates/ns-runtime/src/builtins/reflection.rs` (their
> leaves), the reified mixin (`Mutator::reified_mixin`, `Heap`'s
> `mixin_objects`, `Kernel.ns`'s private class `Mixin`), `Mixin::enclosing`
> and `Mixin::module_text` (`mixin.rs`), the image's `OBJECT_MIXIN`
> (`image/encode.rs`, `image/decode.rs`).

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
5b's atomic install will call it after its bulk forward. Scoping an
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
forward", road step 5b).

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

### Mirrors that read

A program that holds the platform reads the structure of classes, mixins,
methods and objects through mirrors, and nothing it can reach that way
leads to a shipped module's root or the VM mirror. This is road step 5a:
road step 5 is split in two (decided with the user on 2026-10-17), this
read side first, then the MirrorBuilder and the atomic install, road step
5b below, which build on it -- the same capability, the same mirrors and
the same boundary at the shipped modules. The decisions below were settled
with the user on 2026-10-17, or taken while the step was built and stated
here as what the code does.

**The specification asks for the capability, not the protocol.** §1 makes
mirrors capabilities ("Given access to the appropriate mirrors (and only
given such access), a running program can both introspect and modify
itself", `newspeak-spec.pdf` p. 3), §8.2 asks that mirrors examine "the
structure of class declarations and methods, including the bodies of
methods" and "individual objects", §4.3 makes metadata available "via
mirrors" (p. 21); no mirror class but `MessageMirror` is named, nor what a
mirror answers, nor how a program obtains one, nor how mirrors meet access
control (searched for mirror, reflect, reflectee, every mirror class name,
capability, platform, metaclass, `class`, `mixin`, enclosing object,
private, protected, access control, introspect). The protocol follows the
canonical implementation and Primordial Soup, which agree on it.

**The mirrors are a shipped module, handed out as `platform mirrors`.** The
seventh shipped module, `platform/MirrorsFactory.ns`, is built on first
demand like the transcript, the clock and the typed arrays, over the VM
mirror and the kernel, of which its root keeps only `Error` and
`ArgumentError`; it is a value, so every platform shares the one module
and its classes. The platform's lazy `mirrors` answers the module's
`Mirrors` facet, a value with one class accessor for each of the two
classes a program makes mirrors with, `ObjectMirror` and `ClassMirror`,
whose factory `reflecting:` is where a program enters, and through which
a program subclasses them. Like every class of a shipped factory, both
are private classes of the module's root, whose one public member is
`build`. Every other mirror class -- `AbstractClassMirror`,
`NavigatedClassMirror`, `MixinMirror`, `ClassDeclarationMirror`,
`MethodMirror`, `SlotDeclarationMirror`, `SlotMirror` and `MirrorGroup` --
is reached only by navigating from those two. This is both
prior arts' shape (`prior-art/newspeak/RuntimeForSqueak.ns:99`,
`public mirrors = Mirrors usingPlatform: self vmMirror: vmm`;
`prior-art/primordialsoup/newspeak/RuntimeWithMirrors.ns:27`) and the
corpus's (`platform mirrors ClassMirror`). Primordial Soup also has a
runtime without mirrors; ns-next has one platform, and the platform is
itself the capability: a module that does not pass it on passes on no
mirror. The module's root holds the VM mirror in an immutable eager slot,
checked when its instance is recorded as every shipped module's given the
mirror is (lock (2), [core-library.md](core-library.md), "The VM mirror is
the whole Rust boundary").

**The protocol.** Primordial Soup's selectors
(`prior-art/primordialsoup/newspeak/Mirrors.ns`), with the canonical
`ClassMirror` split -- a class mirror answers the class's whole inherited
member set, superclass first, a mixin mirror only the mixin's own:

- `ObjectMirror reflecting: anObject`: `reflectee`, `getClass` (a class
  mirror), `getSlot: aSymbol` (the slot's value), `slots` (a `MirrorGroup`
  of `SlotMirror`s, each a `name` and a `value`, an `ObjectMirror`), `=`
  (by the reflectee's identity) and `hash`.
- `ClassMirror reflecting: aClass` (an `ArgumentError` carrying the
  argument for anything that is not a class or metaclass): `reflectee`,
  `name`, `simpleName`, `mixin` (a `MixinMirror`), `superclass` (a class
  mirror, `nil` for `Top`, which has none -- `Object`'s superclass `Top` is
  a kernel-private class, and its mirror holds it back like any shipped
  class reached by navigation), `enclosingObject` (an `ObjectMirror`, on
  `nil` for a module's class), `isMeta`, `slots` (eager slot
  declarations), `methods` and `nestedClasses` -- each a `MirrorGroup`,
  every level's members, from `Top` down to the class, a `<:` level's
  where the level stands in the chain, a method a subclass overrides at
  both levels -- `reflects:`, `=` and `hash`.
- `MixinMirror`: `name` (`'Klass'`, the class side `'Klass class'`),
  `simpleName`, `isMeta`, `enclosingMixin` (`nil` for a module's root, a
  class side's that of its instance side), `classMixin` and
  `instanceMixin`, `declaration` (a `ClassDeclarationMirror`), `slots` and
  `lazySlots` (each empty for a class side), `methods` and `nestedClasses`
  (empty for a class side) -- own members only -- `reflects:`, `=` and
  `hash`.
- `ClassDeclarationMirror`: `name`, `instanceSide`, `classSide` (the
  synthesized one of a declaration that declares none), `enclosingClass`
  (`nil` at a module's root), `accessModifier`, `source`, `=` and `hash`.
- `MethodMirror`: `name` (the selector, a Symbol), `accessModifier`,
  `definingMixin`, `source`, `=` and `hash`.
- `SlotDeclarationMirror`: `name`, `isMutable`, `isLazy`,
  `accessModifier`, `definingMixin`, `=` and `hash`.
- `MirrorGroup`: `size`, `isEmpty`, `do:`, `collect:`, `detect:ifNone:`,
  `findMirrorNamed:` (`nil` when absent; a Symbol and a String of one text
  name the same mirror), `includesMirrorNamed:`, and `asArray` (a copy),
  in a fixed order: slots in declaration order, superclass first; methods
  by their position in the source, then by selector; nested classes in
  declaration order.

Every mirror but a `SlotMirror` and a `MirrorGroup` also answers its
`isKindOf...Mirror` test. `name` and
`simpleName` agree: a class's name is its declaration's, never qualified
by the classes it is nested in. `accessModifier` answers `#public`,
`#protected` or `#private`, the default already applied (§6.3.1: an
unmarked member is protected); a class declaration's is its accessor's, a
class side's its instance side's. A method mirror covers the methods a
declaration writes: never a slot's accessors, a lazy slot's getter, the
factory, or a nested class's accessor, which the declaration of the slot,
the class and the nested class already describe. A method's `source` is
its declaration from the access modifier to the closing parenthesis, the
comments before it left out; a class declaration's is its `Mixin::span`,
from its access modifier, or `class` when it has none; either is `nil`
when the code was built without its source. Mirrors read private and
protected members alike, as both prior arts' do (`Mirrors.ns:1628`,
`prior-art/newspeak/MirrorsForSqueak.ns:1220-1222`): holding the
capability is the access check. Not in this step, each an item of
[OPEN-WORK.md](../OPEN-WORK.md): `evaluate:`, `perform:`, `instances`,
`referringObjects`, the activation and thread mirrors, metadata (§4.3), a
lazy slot's value through `getSlot:`, and the indexed elements of an
`Array` or a `String` in `slots`.

**A mirror sends its reflectee nothing.** An object mirror's `getClass`,
`=` and `hash`, a class mirror's whole walk, and a mixin mirror's reads ask
the VM mirror, so a reflectee that overrides `=`, `hash` or `class`, or is
a proxy, reflects as itself; refusing a slot name reads the name's text
through the VM too (`reflectionSymbolText:ifFail:`), so a refusal runs none
of the program's code.

**A mirror forces no future.** A §6.3.2 future is such a proxy: a mirror
reflects the future object itself -- its class `FutureImplementation`, a
kernel class whose slots a mirror does not read, so `slots` is empty and
`getSlot:` refuses every name; its identity and its hash -- and never the
value it stands for. The failure path that sees through a future for every
other leaf ([object-model.md](object-model.md), "Where Rust sees a future")
skips the reflection family, so a reflection leaf that declines for a
future declines, and the mirrors compare by identity and read a Symbol's
text through leaves of that family (`reflectionObject:identicalTo:ifFail:`,
`reflectionSymbolText:ifFail:`) rather than the kernel's rows, which see
through one. So `ClassMirror reflecting:` a future signals its
`ArgumentError`, a future given `getSlot:` as a name is no Symbol, and a
navigated mirror anchored on a future reflects `FutureImplementation`. A
mirror's reflectee and anchor are slots like any other: once the program
itself resolves the future, the next full collection replaces them with
its value, as it replaces every reference in an object field
([memory.md](memory.md), "A resolved future is traced as its value"), and
the mirror reflects the value from then on. Otherwise a mirror read would
run the program's computation, and `getSlot:` would answer the resolved
value's slot while `slots` listed none.

**No mirror hands out a value it reached through a shipped level.** Both
prior arts let whoever holds the mirrors reach everything
(`prior-art/primordialsoup/newspeak/Mirrors.ns:1575-1579` reflects any
object; the canonical platform even hands its VM mirror out through
`platform squeak`). Here that would hand a program the VM mirror -- a
`TranscriptFactory`'s `vmMirror` slot holds it -- and the shipped roots:
a facet class's enclosing object is its module's root instance, and a
private shipped class carries its enclosing object's authority. So the
rule is structural, by level: a mirror reads a value only through a level
whose mixin is not shipped (`Mixin::shipped`, a class side's that of its
instance side).

- **A mirror reads only the slot a name names.** An object mirror's
  `slots` lists each name once, under the slot the object's own code sees
  -- the one the nearest level declaring the name declares -- superclass
  level first and in declaration order within a level, and leaves the name
  out when that level declares it lazy or is a core-library level;
  `getSlot:` reads the same slot and refuses then, never reading a farther
  slot the nearest one shadows. A program's subclass of `List` reads its
  own slots, not `List`'s. Primordial Soup's `getSlot:` cannot reach a
  shadowed slot either (`Mirrors.ns:1627-1628`, deprecated for it); its
  `slots` lists every level's, which here would put a shadowed value under
  a name that names another slot.
- `getSlot:` with a name that is no Symbol signals an `ArgumentError`
  carrying the name, `'getSlot: requires a Symbol'`; a name it may not read
  an `Error`, `'getSlot: no slot named <name> that a mirror may read'`.
- `enclosingObject` of a class whose mixin is shipped signals an `Error`,
  `'enclosingObject: the enclosing object of a core-library class is not
  reflected'`.
- **A class mirror reached by navigation holds the way to its class, never
  the class.** `getClass` and `superclass` answer a `NavigatedClassMirror`
  holding an anchor the program gave -- the object an object mirror
  reflects, or the class `ClassMirror reflecting:` was given -- whether
  the way takes the anchor's class first, and the number of superclass
  steps after that; every use walks the way afresh through the VM, by
  identity, sending the anchor and the counts nothing. Its `reflectee`
  hands the class out only when its mixin is the program's own and signals
  an `Error` otherwise, `'reflectee: a core-library class reached by
  navigation is not handed out'`; the `ClassMirror` a program made hands
  its class back whatever it is, so a program that can name `List` gets
  `List` back, and one that reaches a private shipped class by `getClass`
  gets its structure and not the class. Holding the way rather than the
  class is what lets a program holding a navigated mirror on `Top` or a
  private core-library class be written to an image: the mirror's slots
  hold only what the program gave and two immediates, so lock (4) -- no
  private shipped class in a value position
  ([image-format.md](image-format.md), "Shipped names") -- stands
  unloosened.
- **`ClassMirror>>=` is by class**, by identity, however each mirror was
  reached; two applications of one mixin are two classes. It hands the
  class it compares to no code but the module's own: it sends `reflects:`,
  public for the purpose, only to an instance of the module's
  `ClassMirror` or `NavigatedClassMirror`, or, for a mirror the program
  made, of the receiver's own class. So it is deliberately asymmetric for a
  program's subclass of `ClassMirror` against a navigated mirror: the
  subclass's mirror trusts the module's navigated mirror and answers true
  for one on its class, but the navigated mirror trusts no class a program
  declared and answers false, since such a class's `reflects:` could keep a
  class the program was never given. `MixinMirror>>=` is by the reified
  mixin, trusting the module's `MixinMirror` and the receiver's own class
  alike; a class declaration mirror compares by its instance side, and a
  method or slot declaration mirror by its name and defining mixin.
- What stays readable on a shipped class or mixin is structure: names,
  selectors, access modifiers, slot declarations, the superclass chain as
  further mirrors, and source. None of it is a value that carries
  authority.

Behind the Newspeak rule, the leaves that answer a slot's value or an
enclosing object refuse the VM mirror and every recorded shipped module
instance whatever the level (`Mutator::is_withheld`), so a defect in
`MirrorsFactory.ns` cannot hand one out. The price is that a mirror shows
no element of a `List`; the list's public protocol still does.

**The leaves.** The mirrors read through one family,
`Primitive::Reflection(ReflectionPrimitive)` (`builtins/reflection.rs`),
nineteen rows appended to `MIRROR`, beside the existing
`objectClass:ifFail:`, `objectHash:ifFail:` and
`behaviorSuperclass:ifFail:`, which never force a future a mirror hands
them: the first two answer for any object, and the third is given only a
class the walk reached. A class leaf takes a class or a metaclass
(`reflectionMixinOf:`, `reflectionIsMetaclass:`,
`reflectionEnclosingObjectOf:`), an object leaf any object
(`reflectionSlotNamesOf:`, `reflectionSlotOf:named:`), and a mixin leaf a
reified mixin (`reflectionMixinName:`, `IsMeta:`, `Enclosing:`,
`InstanceSide:`, `ClassSide:`, `Slots:`, `Methods:`, `Nested:`,
`Access:`, `Source:`, `IsShipped:`, and `reflectionMixin:methodSource:`,
whose reported selector is `methodSource:`), beside identity
(`reflectionObject:identicalTo:`) and a Symbol's text
(`reflectionSymbolText:`); each ends in `ifFail:` and
declines -- answers nothing, so the failure block runs -- for anything
else, and none raises: what a program sees is the `Error` or
`ArgumentError` the mirrors module signals. Slots and methods come back as
`Array`s of `Array`s (`{name. isMutable. accessModifier. isLazy}`,
`{selector. accessModifier}`), which the module wraps in mirrors. A
declared method is a compiled method whose selector is not a lazy slot's
name of the same mixin, ordered by its body's source start, then its
selector, so the order does not depend on the method table's hashing.

**A mixin is reified as a kernel-private object.** A mixin is an immutable
Rust value in the heap's mixin table, named by its `MixinId`
("Planned with road step 5: an edit replaces the mixin under its id"
below), so a `MixinMirror` needs a heap object to stand for it as its
reflectee -- as both prior arts' mixins are heap objects
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1463-1475`).
That object is an instance of `Kernel.ns`'s private class `Mixin`
(`BuiltinClass::Mixin`), format `FixedPointers`, of one field, the
`MixinId` as a `SmallInteger`. There is one per mixin per heap, made on
first demand by `Mutator::reified_mixin` and kept in a strong table on the
heap indexed by the id (`Heap`'s `mixin_objects`, a root of every
collection, forwarded by every move), so `==` is the mixin's identity and
the object will survive an edit that installs a new mixin under the id.
`Heap::mixin_object_id` reads an object as a mixin only when it is the one
the table holds for the id its field names, so no other object of the
class ever stands for a mixin. `Mixin new` is refused (`decide_factory`),
as `Symbol new` is; no program can name the class; the object answers
nothing of its own, and only `MirrorsFactory.ns` reads it, through the
leaves. A class side is a mixin of its own, the synthesized one of a class
that declares no class side included (`Mutator::metaclass_mixin`, one per
class mixin). The image writes a reified mixin as a record of its own,
naming a mixin row or, for a shipped mixin, the shipped class whose
mixin it is, by its declaration -- the declaration of a core-library class
not yet built included, which the loader then builds
([image-format.md](image-format.md), "A reified mixin is a record of its
own"). The bare id as a `SmallInteger` was rejected: after a load it names
another mixin, or none.

**A mixin knows its enclosing mixin and its module's source.** A mixin
holds its nested mixins, not the one it is nested in; `enclosingMixin` and
a class side's `instanceMixin` need the upward link, so each builder --
from source, from the embedded shipped code, from an image -- sets
`Mixin::enclosing`, a `Weak` link set once after the enclosing mixin's
`Rc` exists (`Mixin::set_enclosing_of_children`), a class side's to its
instance side's mixin; `Mixin::enclosing_mixin` reads it. A method's
source span is its body's `CompiledCode::source`, on both codecs' wire;
the text is `Mixin::module_text`, the one `Rc<str>` every mixin of the
module shares: a source module's `ResolvedModule::source`, a shipped
module's text (one per module per thread, `shipped_module_text`, whether
the boot compiles the source or decodes the embedded code), an image's
module text when the image carries source, shared with the host's
`LoadedModule::source`, and `None` otherwise -- for a builtin's mixin,
the VM mirror's, and an image without source -- when `source` answers
`nil`. A synthesized class side has no text of its own and reads its
instance side's. A span read from an image is untrusted: every slice goes
through `mixin::source_text`, which answers a slice only where the span
lies within the text and on character boundaries, `nil` otherwise.

**`platform mirrors` is exempt from the on-demand rule** (decided with
the user on 2026-10-18), as `platform kernel` and `platform typedArrays`
are: it hands out the classes of a core-library module and
reaches no host ([core-library.md](core-library.md),
"The on-demand rule").

**The mirror table is past 256 rows.** `MirrorPrimitive` is a `u16`, so
`MIRROR` may hold 65,536 rows and holds 267; `Primitive` grew from two
bytes to four in the padding every holder already had, so none grew
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary").

What pins it: the goldens under `goldens/library/mirror/`, run under both
lowerings and, in the `images` profile, through `build` then `load`; the
leaves' unit tests in `builtins/reflection.rs`; `tests/platform.rs`, which
builds the module once on first demand, reads a program's own source
through a mirror, and walks from the platform through every facet and
every class the facets hand out, finding no value a mirror answers but
the classes the walk was given; and `tests/image.rs`, which round-trips reified
mixins -- a program's, a shipped one's and one of a class not yet built --
and a program holding mirrors, and refuses every crafted mixin record.

### Planned: road step 5b, MirrorBuilder and atomic installation

Would change: a minimal MirrorBuilder exists, with an atomic install, as road
step 5b of the road to a high-performance VM, whose building block is
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
- **The builder is reached from the mirrors of road step 5a**, through
  `platform mirrors`, and a reified mixin keeps its identity across the
  install, since the install keeps the `MixinId`; the `Weak` link to an
  enclosing mixin (road step 5a) moves to the id with the rest of the
  mixin's identity ("What the id has to carry" below).
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

**The instance-of relation a class mirror reads exists.** A class
mirror's `isMeta` asks the VM whether its class is a metaclass
(`reflectionIsMetaclass:`), and its `getClass` of a class answers the
class's metaclass, which needs every class object's class to be its
metaclass and every metaclass's class to be `Metaclass`, as §3.2 says.
Both hold: `Behavior`, `Class` and `Metaclass`
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
both, so the two paths would disagree; road step 5b's refusal of every edit
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
nested-class slot, the image encoder's row ids and the shipped world's
declarations not yet built (`ShippedWorld::declared_by_mixin`) are keyed
by `mixin_key` too. The application cache, today strong and growing with
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
