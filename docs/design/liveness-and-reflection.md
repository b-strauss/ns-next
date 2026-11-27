# Liveness, reflection, and concurrency

> **Status:** mirrors that read are built, road step 5a ("Mirrors that
> read"): `platform mirrors` hands out `ObjectMirror` and `ClassMirror`,
> through which a program reads the structure of its classes, mixins,
> methods and objects, and no mirror hands out a value it reached through
> a shipped level. The VM's half of live editing is built, road step 5b1:
> every version of a mixin answers the one id it was first registered
> under, so a fresh mixin is installed under an old one's id ("An edit
> replaces the mixin under its id"), and `become:` exists as an eager bulk
> forward behind one VM-mirror leaf, with the recorded guards that keep a
> forwarded receiver from being written in the old layout
> ([object-model.md](object-model.md), "`become:` is an eager bulk
> forward", "What a slot access checks"). A MirrorBuilder and an atomic
> install are built, road step 5b2a ("A MirrorBuilder and an atomic
> install"): `ClassDeclarationMirror>>asBuilder` stages edits of a
> program's class -- its header, its methods on either side, its lazy
> slots -- as source, and `install` recompiles the class from its
> module's edited text and commits with no safepoint, every
> application of the edited mixin re-applied and the instances whose
> layout changed migrated by one bulk forward, or changes nothing.
> The complete install is built, the first of hot swap's three batches
> ("The complete install"): several builders install as one
> (`installAtomically:`) through three VM-mirror leaves, a superclass
> change evaluates the new clause for every class object the program
> reaches, in their creation order, nested classes are added, replaced
> and removed -- a removed one living on, obsolete, until nothing
> reaches it, and carried by an image while something does --, a class
> is renamed and its access changed in place, a module's identity
> changes in place, `ClassDeclarationBuilder fromSource:` declares a
> module at run time, and the install vacates only the cache entries
> naming what it re-applied.
> Every declaration holds its own text, and an edited program is written
> to an image and loads back edited, the code an older version runs
> included, and a program loaded from an image with source can be edited,
> road step 5b2b ("Edited programs in images").
> A full collection reclaims the ids live editing leaves behind -- dead
> applications and their guard ids, superseded mixin versions, the mixin
> trees and code ids nothing names, the caches' ranges -- so a program
> that edits and installs in a loop keeps its tables at a bounded size,
> road step 5b2c ("What the id has to carry").
> **Planned:** actors; the debugger and edit-and-continue are
> [debugger.md](debugger.md)'s, whose first three stages are built: the
> debugging API at the safepoint -- pausing a running program from
> another thread, reading its stack and values, continuing and
> terminating it --, the metadata that names and places a stopped frame
> and its variables, and breakpoints and stepping -- line, method and
> exception breakpoints, step over, into and out, run to cursor.
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
> `mixin_objects`, `Kernel.ns`'s private class `Mixin`), the mixin table's
> rows (`heap/tables.rs`; `Heap::mixin_id`, `replace_mixin`,
> `enclosing_mixin_id`, `nested_of`, `assembled_text`, `declaration_source`,
> `module_source`; `Mutator::replace_mixin`), their reclamation
> (`interpreter/reclaim.rs`, `heap/reclaim.rs`), a declaration's text
> (`mixin.rs`'s `DeclarationText`), the edit and the install
> (`interpreter/edit.rs`, `interpreter/install.rs`, and under
> `interpreter/install/` the preparation's cache and epoch, `prepared.rs`,
> and the superclass clauses, `superclasses.rs`), the cache check after
> an install (`interpreter/inline_cache/verify.rs`), the image's
> `OBJECT_MIXIN` (`image/encode.rs`, `image/decode.rs`), and the
> debugger, which reads a stopped run's activations and values and
> patches breakpoints into the bodies it runs
> (`crates/ns-runtime/src/debugger/`, its tests
> `crates/ns-runtime/tests/debugger.rs`; [debugger.md](debugger.md)).

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
deserialize, begin interpreting, re-JIT as code warms. The debugger reads
only interpreter frames: the exit that leaves machine code for a stop
materializes every compiled frame of its stretch, and a stop at a poll of
optimized code, whose exit defers the blocks it eliminated, is deoptimized
into baseline code before the debugger reads it ([debugger.md](debugger.md),
"The stop"); a frame the debugger writes, which is planned, will go on
interpreted ([debugger.md](debugger.md), "Writing a frame") — debugging
and JIT-optimizing the same frame simultaneously is explicitly not a goal.

## Caches and their dependency keys

Method dictionaries are mutable at run time, so dispatch resolves through the
dictionary or through a cache that is invalidated when the dictionary changes.
**Never bake a method-body pointer into anything durable.**

Every cache — inline caches, global lookup caches, later JIT speculative
assumptions — carries a **dependency key**. A "class shape changed", "method
changed" or "superclass changed" event vacates every cache entry keyed to the
affected class, and for a superclass change to its whole subtree, and nothing
else.

The key is the **mixin application**, not the class object; see
[dispatch.md](dispatch.md) for why the obvious choice is wrong. Compiled
initializers must be flushed along with everything else on a shape change,
because `InitializeSlot` is layout-dependent.

The inline caches ([dispatch.md](dispatch.md), "3b — The caches") are built
for this: an entry holds no handle, so invalidating all of them
(`Interpreter::invalidate_caches`) is resetting the heap's cache table and
emptying its global cache, with every code family's range kept for the
activations still running; a debug-build stress mode, `NS_NEXT_STRESS_CACHES`,
does exactly that at every safepoint. The bulk forward's leaf invalidates
every cache the same way after its walk (`InlineCaches::invalidate`, from
`heap::world::forward_identity`). The atomic install ("A MirrorBuilder and
an atomic install", "The complete install" below, decision 6) scopes its
invalidation to what it changed: before it replaces anything it gathers
the guards and indices of the applications it re-applies -- every class
object whose chain holds a replaced declaration, or that a changed clause
re-parents -- and the declaration versions it replaces (`StaleIds`), and
vacates exactly the entries naming one -- a site's or a polymorphic
slot's guard, a lexical rule's enclosing application, a global entry, a
slot target's level and recorded guard, the verified pair, each with its
baseline cell -- and the nested-class accessor entries naming a replaced
version, whether or not it forwarded, and keeps every other one warm
(`InlineCaches::forget_stale`; [dispatch.md](dispatch.md),
"Invalidation"); it frees the optimized copies keyed on a re-applied
application, found through the list each application keeps of the
copies depending on it ([execution.md](execution.md), "Invalidation"). The
install stress mode checks after every commit that every entry it kept
is what a fresh lookup decides. Nothing else mutates a method table at
run time. A full collection that reclaims a guard
id or an application index vacates exactly the entries that name it by the
same criterion (`r[vm.dispatch.freed-ids-scrubbed]`;
`InlineCaches::forget_freed`), so
an id minted again never meets an entry filled for the application it named
before. The dispatch id a cache guards on is a copy in the object header
(`r[vm.dispatch.header-guard-stamped]`; [dispatch.md](dispatch.md), "The
dispatch id"), so `become:` must not let a stale guard hit, and a class
change must restamp it, as `Heap::class_set_application` already does when a
class's application changes; otherwise a cache hit would run the old class's
method on the object. The bulk forward meets the first by construction: it
is eager, so nothing names a forwarder once it ends and every reference names
the new object, which keeps its own id, and every inline cache entry it
could make stale is vacated after it
([object-model.md](object-model.md), "`become:` is an eager bulk forward").
The caches also rest on a guard naming one application whose chain does
not change while they persist -- the private and super slot accessors'
verified pair most directly ([object-model.md](object-model.md), "What a
slot access checks") -- so whatever changes a class object's application
or chain under an unchanged guard, as the install does, invalidates the
caches after it (`r[vm.dispatch.invalidation-after-change]`).

## Debugger

The debugger has a document of its own, [debugger.md](debugger.md): what it
has to do, the nine decisions its design rests on, and the order of building
it. Its debugging API is part of the reflective core this document
requires, with two clients: the remote agent an IDE talks to, and
`ActivationMirror`, through which a program reaches activations;
edit-and-continue is described there ("Hot swap and edit-and-continue").

Two things already in place serve it and are not to be regressed: the
statement→program-counter table on every `CompiledCode`, through which
edit-and-continue maps a frame's `program_counter` between an old and a
new body (the debugger's stop locations, finer, are compiled again on
demand, [debugger.md](debugger.md), decisions 4 and 5), and a chain of running activations
walkable independently of the Rust stack — the activation stack — on which the
trampoline runs every send without recursing through the Rust stack
([execution.md](execution.md), "The trampoline"), and with no Rust frame
between the host and the loop, since nothing re-enters it from Rust ([execution.md](execution.md),
"No re-entry from Rust").

The reified frame is not the *default* representation of an activation; it is
the one a block, and later the debugger, asks for. Nothing here loses a
capability by that: the debugger reads a stopped stack's frames where they
are -- on the activation stack, or in the frame objects a suspension left --
without reifying any, and will reify a frame where it has to hold one --
to evaluate in it, to restart it, to move it onto edited code
([debugger.md](debugger.md)).

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
  through one bulk forward → vacate the cache entries naming the class.
- *Add, remove or replace a method* → update the method dictionary → vacate
  the cache entries naming the class.
- *Change a superclass* → re-resolve lookup for the subtree, vacate its
  cache entries, re-lay-out instances if inherited slots changed.

All three are built ("A MirrorBuilder and an atomic install", "The
complete install" below).

### Mirrors that read

A program that holds the platform reads the structure of classes, mixins,
methods and objects through mirrors, and nothing it can reach that way
leads to a shipped module's root or the VM mirror. This is road step 5a:
road step 5 is split in two (decided with the user on 2026-10-17), this
read side first, then the MirrorBuilder and the atomic install, road step
5b below (its VM half built by 5b1, the rest split into 5b2a's
MirrorBuilder and atomic install, 5b2b's edited programs in images and
5b2c's reclaiming of ids),
which build on it --
the same capability, the same mirrors and the same boundary at the shipped
modules. The decisions below were settled with the user on 2026-10-17, or
taken while the step was built and stated here as what the code does.

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
by the classes it is nested in. A class mirror's `name` is the class's
own, as `Class>>name` answers it (`Heap::class_name`, read through the
metaclass; a metaclass's is its class's followed by `' class'`), not its
mixin's: the class object of a declaration without a body,
`class M3 = Base new <: M1 tag: 3.`, is the level its last clause adds and
applies `M1`'s mixin, yet is named `M3` (§6: "The class has a name, given
by the class expression"), and a level a `<:` clause adds is named for the
class the clause names. Its `mixin` is `M1`'s, whose `name`, `declaration`
and `asBuilder` are `M1`'s: the class object *is* the level applying it
([object-model.md](object-model.md), "Mixin application";
`goldens/library/mirror/a_body_less_classs_name_is_its_declarations.ns`). `accessModifier` answers `#public`,
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

**A mirror sends its reflectee nothing** (`r[semantics.reflection.mirrors]`).
An object mirror's `getClass`,
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
rule (`r[vm.mirror.mirrors-withhold-shipped-values]`) is structural, by
level: a mirror reads a value only through a level whose mixin is not
shipped (`Mixin::shipped`, a class side's that of its instance side;
`r[vm.mirror.shipped-decided-by-root]`).

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
The same family holds 5b1's forward, the MirrorBuilder's check,
`reflectionMixin:checkEdit:ifFail:`, and the install's three rows,
`reflectionPrepareInstall:ifFail:`,
`reflectionInstall:evaluateClause:epoch:ifFail:` and
`reflectionInstall:commitEpoch:superclasses:ifFail:`, the table's last
rows ("A MirrorBuilder and an atomic install", "The complete install"
below).

**A mixin is reified as a kernel-private object.** A mixin is an immutable
Rust value in the heap's mixin table, named by its `MixinId`
("An edit replaces the mixin under its id"
below), so a `MixinMirror` needs a heap object to stand for it as its
reflectee -- as both prior arts' mixins are heap objects
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1463-1475`).
That object is an instance of `Kernel.ns`'s private class `Mixin`
(`BuiltinClass::Mixin`), format `FixedPointers`, of one field, the
`MixinId` as a `SmallInteger`. There is one per mixin per heap, made on
first demand by `Mutator::reified_mixin` and kept in a table on the heap
indexed by the id (`Heap`'s `mixin_objects`, forwarded by every move),
weak in a full collection (`r[vm.heap.reified-mixin-canonical]`): while
anything reaches the object it stays the
one object of its mixin and keeps the mixin's id, so `==` is the mixin's
identity for every comparison a program can make and the object survives
an edit that installs a new mixin under the id; once nothing reaches it,
nothing can compare it, and the next reflection makes a fresh one
([memory.md](memory.md), "Weak references").
`Heap::mixin_object_id` reads an object as a mixin only when it is the one
the table holds for the id its field names, so no other object of the
class ever stands for a mixin. `Mixin new` is refused (the factory body's
allocation, `instantiate::allocate_for_factory`),
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

**A mixin knows its enclosing mixin and its declaration's text.** A mixin
holds its nested mixins, not the one it is nested in; `enclosingMixin` and
a class side's `instanceMixin` need the upward link, so each builder --
from source, from the embedded shipped code, from an image -- sets
`Mixin::enclosing`, a `Weak` link set once after the enclosing mixin's
`Rc` exists (`Mixin::set_enclosing_of_children`), a class side's to its
instance side's mixin; `Heap::mixin_id` copies the link into the
mixin's row as the enclosing mixin's id when it registers the mixin, and
the leaves read it through the id (`Heap::enclosing_mixin_id`), so that a
replaced mixin keeps the enclosing mixin ("An edit replaces the mixin
under its id" below). Every declaration holds its own text, its nested
declarations cut out (`Mixin::text`, [object-model.md](object-model.md),
"Source retention and provenance"): a program's declaration a copy of its
own, a shipped one its module's one text per thread, and a class side its
instance side's; a builtin's mixin, the VM mirror's and a mixin of an
image without source hold none, and `source` answers `nil`. A method's
source span is its body's `CompiledCode::source`, on both codecs' wire,
sliced from the text of the version that holds the method, so an edit of
a nested class leaves every method of the class enclosing it answering
its own text. A declaration mirror's `source` is its module's current
text, assembled from every declaration's current version, sliced at the
declaration's span (`Heap::declaration_source`), so it holds every edit
nested in it. A text and a span read from an image are untrusted: a slice
or an assembly that does not fit answers `nil`, never a panic.

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

### A MirrorBuilder and an atomic install

A program holding `platform mirrors` edits a class -- its header's slots,
factory and initializer, its instance-side and class-side methods, its
lazy slots -- through a `ClassDeclarationBuilder`, and `install` makes the
edit visible at once to every application of the edited mixin, migrating
the instances whose layout changed, or changes nothing. This is road step
5b2a, the second plan of road step 5b of the road to a high-performance VM
(decided with the user on 2026-10-05; split from road step 5b1 on
2026-10-18, and into 5b2a and 5b2b on 2026-10-18, the design settled with
the user that day; built on 2026-10-18,
[its plan](../superpowers/plans/2026-10-18-2138-mirror-builder-and-atomic-install.md)).
Road step 5b1 built what it stands on: every version of a mixin answers
one id, so a fresh mixin is installed under an old one's id ("An edit
replaces the mixin under its id" below), and `become:` as an eager bulk
forward, with the recorded guards that keep an activation or a cached
private or super accessor from writing a forwarded receiver in the old
layout ([object-model.md](object-model.md), "`become:` is an eager bulk
forward", "What a slot access checks"). It follows Primordial Soup's
`Mirrors.ns` (`prior-art/primordialsoup/newspeak/Mirrors.ns`, the builders
at `:566-824` and `:1310-1573`, `AtomicInstaller` at `:264-563`) and the
canonical Newspeak's `prior-art/newspeak/AtomicInstaller.ns`, and takes
its behaviour from their `MirrorBuilderTesting.ns` and
`AtomicInstallTesting.ns`; no code is copied. The specification asks only
that several reflective changes apply "simultaneously and atomically ...
if any change fails, none take place" (§8.2, `newspeak-spec.pdf` p. 47)
and that a change to a mixin affect all of its applications (§6.2;
`r[semantics.reflection.atomic-install]`); what
happens to existing instances and running activations it leaves to the
implementation.

**The protocol, in `MirrorsFactory.ns`.** `ClassDeclarationMirror>>asBuilder`
answers a `ClassDeclarationBuilder` over the reflected mixin. The builder
checks its empty edit as it is made, so `asBuilder` signals an `Error`
with the VM's diagnostic, and answers no builder, for a shipped mixin --
a mixin of a shipped core-library module, `Kernel.ns` and the mirrors
module among them, which also seals the VM mirror's class ("The VM
mirror's class must be sealed against it" below) -- and for a mixin
whose module has no text, a program loaded from an image written with
`--strip-source` among them. A program loaded from an image with source
is edited like one read from source ("Edited programs in images" below).
The builder answers `source`, `header`, `instanceSide`, `classSide`,
`declaration` and `install`:

- `source`, the staged declaration's text, from its access modifier (or
  `class`) to its closing parenthesis, with every staged edit spliced in:
  a replacing member stands where the member it replaces stood, an added
  one last on its side. With nothing staged it is what the declaration
  mirror's `source` answers.
- `header`, a `ClassHeaderBuilder`: `source`, the staged header, and
  `source: aString`, which replaces the class header -- its factory, its
  slots and its initializer, its name and access, which a rename and an
  access change install, its superclass and `<:` clauses, which a
  superclass change installs, and for a module's root its `:module:`,
  which a change of the module's identity installs ("The complete
  install" below, decisions 1, 4, 5 and 9); `name: aSymbol` stages a new
  name, in place of the header's. The header is the
  declaration's text from its start up to, not including, the
  parenthesis that opens its members, whitespace before that parenthesis
  included, so a header given to `source:` starts exactly where the
  declaration does, at its access modifier or `class`, with no
  whitespace or comment before it -- for a module's root at its first
  metadata comment, so that the header holds its `:module:`; a root's
  `header source` is then not a prefix of its `source`, which starts at
  the class. A root header given without its `:module:` makes the module
  undeclared: its id becomes its class's name, without a version.
- `instanceSide` and `classSide`, each a `MixinBuilder`: `methods`, a
  `MutableMemberGroup` with `addFromSource: aString` -- adds the method,
  or replaces the one of the same selector -- and `removeMirrorNamed:
  aSymbol`, which removes a method and does nothing for a selector the
  side does not declare; on the instance side `lazySlots`, a group with
  the same two messages for lazy slots, since a lazy slot is a member
  (`ast::Member::LazySlot`) and so not reached through the header, while
  the class side's `lazySlots` signals an `Error`, as §6.3's class side
  declares none; on the instance side `nestedClasses`, a group with the
  same two messages for nested classes, as Primordial Soup's
  (`Mirrors.ns:1475-1501`), its `addFromSource:` adding a class or
  replacing the one of the same name and its `removeMirrorNamed:`
  removing one, while the class side's signals `nestedClasses: a class
  side declares no nested classes`; and `slots`, which reads the live
  side's eager slots, since slots are edited through the header, as in
  both prior arts.

`ClassDeclarationBuilder fromSource: aString` -- the class
`platform mirrors` hands out as `ClassDeclarationBuilder`, which a
program holding a builder reached as `aBuilder class` already -- answers
a builder over a module no module holds yet, whose `install` declares it
("The complete install" below, decision 7); it stages nothing before
that install. A builder's `declaration` answers the declaration mirror
of what it stands for, `nil` for one from source not installed yet.

The builder stages its edits apart from the live system: its state is
Newspeak strings and symbols -- the header's text, and for each side the
member operations in the order they were staged -- so nothing outside the
object space is created, and nothing a program observes changes before
`install`. Every `addFromSource:`, `removeMirrorNamed:`, `source:` and
`name:` checks the whole staged state with it added, through the leaf
`reflectionMixin:checkEdit:ifFail:`, which runs the install's preparation
without its commit (below), and signals an `Error` with the diagnostic,
leaving the builder as it was, for a syntax error, a member text that is
not one member of the group's kind, a member that collides with a slot, a
nested class, the factory or a member of another kind, or a refused
header; so an `install` after a failed edit installs only the edits that
succeeded. `install` is the install of the builder alone, as
`installAtomically:` installs several ("The complete install" below), and
afterwards the builder stands for the installed state, with nothing
staged, and can be edited again. The builder's `source` and its header's
`source` answer the texts the last successful check or install answered,
so the builder never reads Newspeak syntax itself: the extent of a header
is the parse's, which a scanner written in Newspeak would have to follow
every rule of the lexer to agree with. Because each check and the install
start from the module's current text, an install another builder made
meanwhile is under this one's: two edits that each pass their own check
can still fail together at the second install.

**The edit on the wire, and the check's answer.** The check takes the
reified mixin of either side of the edited declaration -- a class side
stands for its instance side's declaration -- and an `Array`,
`{ header. instanceOperations. classOperations. newName }`, which
`interpreter::edit::Edit::from_value` decodes: `header` a `String`, or
`nil` to keep it; each operations entry an `Array` of two-element
`Array`s applied in order, `{#addMethod. aString}`, `{#removeMethod.
aSymbol}`, `{#addLazySlot. aString}`, `{#removeLazySlot. aSymbol}`,
`{#addClass. aString}` and `{#removeClass. aSymbol}`, the class side's
method operations only; `newName` a `Symbol`, or `nil` to keep the
name. It answers `{true. declarationText. headerText}` on success and
`{false. diagnostic}` on failure, one shape for both, because a leaf
passes no argument to its `ifFail:` block (`interpreter/mirror.rs`'s
`failure_step`), so a diagnostic has to travel in the answer; it
declines -- the block runs -- only for a subject that is no reified
mixin and for an edit `from_value` cannot read, which the builder
reports as `a MirrorBuilder takes a member or a header as a String and
a name as a Symbol`. It prepares through the install's one-entry cache
("The complete install" below), so that an install of exactly the state
the builder last checked compiles nothing again. Its row of `MIRROR` is
reported as `checkEdit:`, and the install's three rows are the table's
last ([core-library.md](core-library.md), "The VM mirror is the whole
Rust boundary").

**Compiling: the module's text is what an edit edits.** The resolver
binds an implicit-receiver send's depth at compile time
(`Binding::LexicalSend { depth }`), from the member names of every class
on the lexical chain (`resolver/classes.rs`'s `own_member_names`), so
adding or removing a method or a slot of a class changes the bindings of
the unchanged bodies of that class and of every class nested in it --
though not of an enclosing or a sibling class, whose member sets hold a
nested class's name only. Primordial Soup reuses every unchanged method,
which it can because all its slot access goes through accessors and its
sends are bound late
(`prior-art/primordialsoup/newspeak/Intermediates.ns:77`); here, an unchanged body sent
an implicit selector whose declaring class changed would send at the
wrong depth, silently. So `Mutator::prepare_edit` (`interpreter/edit.rs`)
recompiles the edited declaration and everything nested in it, and it
does so from the module's text
(`r[vm.compiler.edit-recompiles-from-current-text]`):

1. Every module has a *current text*, assembled from the texts of its
   declarations' current versions, each hole filled by the declaration
   nested there (`Heap::module_source`; [object-model.md](object-model.md),
   "Source retention and provenance"): the text it was read in until an
   install, and with every install in it after. No table keeps it, so no
   text is stored twice, and a program loaded from an image with source
   has one as a program read from source has. The preparation parses the
   current text and finds the edited declaration by its path of names
   from the module's root class; a module with no text is refused
   (`<name> has no source text to edit`), and so is a text that does not
   hold the declaration, as an image's crafted text may not. Every entry
   of an install that edits the same module is spliced into one text, in
   the builders' order, each starting from the text the one before it
   left and finding its declaration by the names that text holds, an
   earlier entry's rename in effect ("The complete install" below,
   decision 8).
2. It splices the staged edits into the text by the spans of the parse,
   byte offsets that always fall on character boundaries: the instance
   side's operations in order, then the class side's, then the header,
   last, since every operation before it finds the declaration by its
   name. A member text is first read alone in a scratch module and must
   be exactly one member of the group's kind that ends where the members
   do, or a text that closed the group early would splice members, or a
   class side, of its own into the declaration; it replaces the member of
   its kind and name, attached comments included, or is appended before
   its side's closing parenthesis, and a class-side method of a class
   that declares no class side opens one. A removal cuts the member and
   its comments; a nested class's removal cuts the whole declaration. The
   header replaces the text from the declaration's start to its members'
   parenthesis, and must end where the members begin. A new name, the
   header's or the one `name:` staged, is written into the declaration's
   name, and for an annotated root into the last part of its `:module:`
   id, the rest of the comment's payload untouched. A superclass clause or
   `<:` clauses that differ, compared as text -- so a change of whitespace
   inside one counts -- make the install evaluate the new clauses
   (decision 1 below).
3. It parses the new text whole (`ns_syntax::parse`) and resolves the
   whole module (`resolver::resolve`, never `resolve_kernel`, since only
   shipped source records mirror sends), so every binding on the lexical
   chain comes from one text and no new entry into the parser or the
   resolver is needed.
4. It compiles the edited declaration's subtree through the existing
   `build_mixin` path (`build_root_mixin` for a module's root): the
   instance side, the declared class side and every nested mixin, each
   fresh and registered under no id. A nested mixin is paired with the
   current version of the old nested mixin of the same name under the same
   parent, a renamed one with its old id by the name it had; a fresh name
   is a declaration added, registered at the commit under the id of the
   declaration it is added to (`PreparedInstall::added`), and an old name
   the fresh tree lacks a declaration removed (`PreparedInstall::removed`;
   decisions 2 and 3 below). A rename or an access change of a nested
   class compiles its enclosing declaration's subtree instead, since the
   enclosing class's bindings and its accessor name the class (decision
   5). Every mixin of the subtree whose fresh version declares
   no class side, while its old one had a registered class side, gets a
   fresh synthesized class side under that class side's id
   (`PreparedModule::synthesized`), so every metaclass of the subtree keeps
   its mixin's id.
5. It answers the fresh mixins with the ids they replace, each holding
   its own declaration's text cut from the new text, and the
   declaration's and its header's text, sliced from the new text by the
   spans of its parse. No re-parse is needed for a declaration's source
   afterwards. A mixin outside the subtree keeps its version and its
   text, which holds no byte of the edited declaration -- that is a hole
   in it -- so its methods' spans still slice it, and the module's text
   assembled after the install holds the edit where the hole is. The
   next edit, of any class of the module, starts from that text and so
   carries every earlier edit.

Every failure -- a syntax error, a resolver error, a refused header, a
refused mixin -- comes from these steps, or from the install's
preparation and commit before anything is replaced ("The complete
install" below), and is answered as a `String`, never a panic or a
`NewspeakError`; compiling the resolved module stays a total lowering. A
replacement of a shipped mixin's code would bypass the compile-time
binding of its `SendMirror`s and primitive methods, so the preparation
refuses one on the Rust side too (`Mutator::editable_declaration`),
beside `asBuilder`. A header that drops a slot an added or kept method
sends to is no failure: the send is bound late, and fails only when it
runs. The diagnostics, read from `interpreter/edit.rs`,
`interpreter/install.rs`, `interpreter/install/prepared.rs`,
`interpreter/install/superclasses.rs` and `MirrorsFactory.ns`, with
`<name>` the edited mixin's `reflected_name` and `<n>` the 1-based line
of the diagnostic's start in the text it was found in. **A diagnostic's
line counts in the module's composed text**: its current text with the
install's edits of that module spliced in, in the builders' order --
those up to the one refused, for an error found while splicing, and all
of them for the whole module's parse and resolution -- a text that no
builder's `source` shows, since a builder shows its own declaration
alone.

| Where | Text |
|---|---|
| a mixin unknown to the heap | `the edited mixin is unknown to this heap` |
| a shipped mixin | `<name> belongs to the shipped core library, which no MirrorBuilder edits` |
| a declaration an install removed, or one nested in it | `<name> was removed by an install, and an obsolete class is not edited` |
| a class side named directly | `<name> is a class side; an edit names its instance side` (the leaves map a class side to its instance side first, so no program reaches it) |
| a mixin whose module has no text -- one of an image written with `--strip-source` | `<name> has no source text to edit` |
| a declaration its module's text does not hold | `<name>'s source text does not hold its declaration`; after the edit, `<name>'s edited source does not hold its declaration` |
| a member text of the wrong kind | `addFromSource: of <name>'s methods takes one method declaration`; `addFromSource: of <name>'s lazy slots takes one lazy slot declaration`; `addFromSource: of <name>'s nested classes takes one class declaration`, for a text of no class declaration, of two, or of one followed by a member |
| a member added to, or a header replaced on, a mixin application with no body | `<name> is a mixin application without a body, which an edit cannot add members to`; `<name> is a mixin application without a body, whose header an edit cannot replace` |
| a syntax error | `<name>'s edited source does not parse: <message> (line <n>)`, the message the parser's, or `the header must declare the class and end where its members begin` for a header that does not end where the members begin |
| a new name that is no identifier | `<name> cannot be renamed to <new name>, which is no identifier` |
| a rename to a member name the enclosing class already has | `<enclosing> already has a member named <new name>` |
| a resolver error | `<name>'s edited source does not resolve: <message> (line <n>)`, `<name>` the innermost edited declaration the error falls in |
| two entries naming one declaration | `two builders edit <name>` |
| an entry whose declaration, or one it is nested in, another entry removes or replaces | `the builder of <name> edits a class the builder of <other name> removes`, or `replaces` |
| a module text that is not one class declaration -- none, as an empty text or a comment alone, two, or one followed by a member | `fromSource: takes one class declaration` |
| a module text that does not parse or resolve | `the module's source does not parse: <message> (line <n>)`; `the module's source does not resolve: <message> (line <n>)` |
| a new identity the loader's rules refuse -- a root's new `:module:` or rename, or a module from source | the loader's own text (`modules::declared_identity`, `check_unreserved`, `check_distinct`): `` `<id>` does not end in the class's name `<class>` ``, `` `<text>` is no version: <reason> ``, `` `<id>` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships ``, ``two modules are `<id>@<version>`: <path> and <path>`` and ``two modules are `<id>` and one of them has no version: <path> and <path>``, a module without a path named `a module declared from source` (`modules::PATHLESS_MODULE`) |
| an install or a module load between the preparation and the commit | `an install ran between the preparation and the commit of <name>'s install; nothing was installed`, `<name>` the first entry's |
| a class object of a declaration whose clauses changed, made while they were evaluated and still reached at the commit | `a class object of <name> was made while its superclass clauses were evaluated; nothing was installed` |
| a new clause that answers no class | `the superclass clause of <name> answered <description>, which is no class` (`Heap::describe`) |
| a new clause that makes a class inherit from itself | `the superclass clause of <name> answers a class that inherits from <name>` |
| an instance whose layout would change that the forward refuses | `cannot migrate an instance of <name>, which is not an ordinary object` |
| an element of `installAtomically:`'s collection, or a builder of an Installer, that is no instance of the mirrors module's own `ClassDeclarationBuilder` | `installAtomically: requires ClassDeclarationBuilders`; `an install requires ClassDeclarationBuilders` (each an `ArgumentError` carrying the element, `MirrorsFactory.ns`) |
| an entry or an edit the leaves cannot read | `a MirrorBuilder takes a member or a header as a String and a name as a Symbol` (`MirrorsFactory.ns`, when a leaf declines) |
| a side, a header or a name staged on a builder from source before its install | `a ClassDeclarationBuilder from source stages nothing before its install` (`MirrorsFactory.ns`) |
| the class side's lazy slots or nested classes | `lazySlots: a class side declares no lazy slots`; `nestedClasses: a class side declares no nested classes` (`MirrorsFactory.ns`) |

A value class whose re-parented chain breaks §3.1.1's conditions is no
refusal of the install: the commit decides the class object's
value-class verdict again, as its creation does
(`redecide_value_class`), and only an instantiation of it signals, with
the text an instantiation of such a class always signals.

**The commit runs nothing and has no safepoint**
(`r[vm.heap.install-commit-runs-nothing]`).
`Mutator::commit_install` (`interpreter/install.rs`) takes the prepared
install, then finds what it reaches, then commits, so no program observes
a half-installed edit, not even a mourned ephemeron's handler at a
safepoint; the superclass clauses an install changes run before it, as
program code between two of the install's leaves ("The complete install"
below):

- *Prepare*: the compilation above, of every module the install edits,
  and of every module it declares from source.
- *Find*: one walk of the object space (`Heap::classes_and_instances_over`)
  collects every class object whose application chain holds an
  application of a replaced id -- a subclass in another module, a `<:`
  clause's level, a metaclass, and the class object of a nested mixin in
  every enclosing instance -- or that a changed clause re-parents, and
  every object whose class is one of them. §6.2 requires every
  application to see the edit; the walk finds them without a list of
  each application's class objects. A clause answer that is no class
  and a cycle are refused before it, and an instance whose layout may
  change -- a replaced level of its class's chain declares another slot
  list or another nested list, or the chain gains or loses a level --
  and which the bulk forward would refuse is refused here, the last
  place anything fails.
- *Replace*: every fresh mixin under its old id (`Mutator::replace_mixin`),
  the declared class side included, and every fresh synthesized class
  side under the old one's id through the same call, which also forgets
  the old class side's applications, so a metaclass over the unchanged
  `Class` is not answered the old application from the cache; every
  added declaration registered under the declaration it is added to, and
  every removed one made obsolete (`Heap::make_obsolete`; decision 3
  below); every module declared from source registered with its tree of
  ids (`Heap::mixin_id`). Each fresh mixin brings its own text, so the
  module's current text, assembled through the ids, holds the edit from
  then on.
- *Re-apply*, superclasses first, by the length of each chain: each class
  object found gets an application of its id's current version over its
  superclass's application now (`Heap::class_install_application`), so it
  keeps its identity, as every class object does
  ([object-model.md](object-model.md), "Everything stays editable"). A
  class object whose clauses did not change keeps its superclass, its
  clause not evaluated again, as in both prior arts; one whose clauses
  changed is re-parented over what its new clauses answered
  (`Heap::class_set_superclass`, its `<:` levels built anew), and decides
  its value-class verdict again. The application cache misses for each,
  since `replace_mixin` forgets the id's keys and a subclass's superclass
  application is new.
- *Record successors*: an old application whose level keeps its slot
  base and its own slots, by name and in order, records the new one as
  its successor (`Heap::record_successor`), and every earlier predecessor
  of it is pointed at the newest, so an activation entered before the
  install finds its level in one step ("What an activation entered
  before the install does next" below).
- *Migrate*: each instance of a class object whose fields changed is
  allocated in the new shape, and its fields are copied by what they are
  named (`r[vm.heap.migration-by-declared-name]`; `mixin.rs`'s `FieldName`
  and `LevelName`, `Mutator::field_names_of_class`): a slot by the level
  declaring it and its name, a hidden nested-class slot by the level
  caching it and the nested declaration's id. One chain may apply a mixin
  twice through `<:` clauses, so a level is named by its owner -- the
  declaration whose clause applies it, read off the level's class object
  (`Heap::class_applied_declaration`), or whose own mixin it is -- that
  owner's occurrence counted from the instance's class down, the level's
  mixin and that mixin's occurrence among the owner's levels in the text
  order of its clauses, all read before the commit re-parents anything.
  A count from the chain's root would shift when a superclass gains or
  loses a clause applying a mixin a subclass's clause applies too, and
  hand the subclass's untouched level the lost one's values; a
  re-parenting rebuilds only the levels below the class it changes, so a
  count from the top shifts none above it. Where an owner's own clauses
  change, its first remaining `<: M` takes the values of its first
  former one, in text order (the goldens
  `a_lost_mixin_level_leaves_a_subclass_level_its_values` and
  `a_gained_mixin_level_starts_nil_beside_a_subclass_level`). A slot the edit adds, or moves to another level, or of a level a
  re-parented chain gains, is `nil`, and its
  initializer does not run, so a new lazy slot is computed on its first
  read; a slot of a level the chain loses, and the hidden slot of a
  removed nested class, goes. The level is part of a slot's name because a subclass's slot may
  shadow a superclass's of the same name: the canonical
  `AtomicInstallTesting.ns` (`testAddShadowedSlotSubclass` and
  `testAddShadowedSlotSuperclass`, `:77-145`) asserts that the two are kept
  apart, where taking the last slot of a name would overwrite the
  shadowed one. The fields are written through `Heap::instance_set_field`
  and nothing is sent. The instances that keep their shape -- among them
  every class object whose metaclass was re-applied -- are restamped with
  their class's new guard in the same pass (`Heap::stamp_dispatch_ids`),
  rather than by a walk per class as `Heap::class_set_application` does.
- *Forward*: one bulk forward, `heap::forward_identity`
  (`r[runtime.forward-identity]`), replaces every
  migrated instance by its new-shape copy, keeping its identity hash
  (`r[runtime.memory.identity-hash]`), and vacates the cache entries
  naming an application the install re-applied or a declaration version
  it replaced (`heap::Invalidation::Stale`); when nothing migrated the
  install vacates the same entries itself (`inline_cache::vacate_stale`),
  since class objects' applications changed under unchanged guards
  ("Caches and their dependency keys" above; decision 6 below). Then it
  frees every optimized copy keyed on a re-applied application
  (`jit::invalidate_applications`), drops the debugger's compiled
  metadata, whose spans count in the old text, sets the edited modules'
  breakpoints again on the new bodies, and raises the install epoch. Only
  instances are forwarded, never a class object or a mixin. The 5b1 leaf
  `elementsOf:forwardIdentityToElementsOf:ifFail:` stays, without a
  caller in shipped code; the install calls the forward on the Rust side,
  because a Newspeak caller would leave a window, between the rebuilt
  classes and the forward, in which a safepoint could run a program's
  code against classes whose instances have not migrated.

**What an activation entered before the install does next**
(`r[vm.dispatch.entered-level-successor]`). A method's
body and a closure keep the code they were running and finish with it,
as in both prior arts, whose installers touch neither; their sends after
the install find the new code. Code that addresses its own slots -- an
initializer, a lazy slot's getter -- records the application it was
entered at, and its own-slot accesses check that the receiver's chain
still holds it ([object-model.md](object-model.md), "What a slot access
checks"). The install builds new applications even for an edit that
changes only methods, since an application holds the mixin version it
was built from; the cold path of that check (`Heap::receiver_has_level`)
also accepts the entered level's successor, so such an activation runs to
its end when its level's slots did not change, and raises the existing
error when they did. The hot path is unchanged. This keeps a debugger's
edit of a method from failing an initializer it is stopped in, which
edit-and-continue needs.

**What the step changed of 5b1's places keyed by address.**
`builtins/activation.rs`'s `inherits_mixin_of` and
`Heap::with_level_applying_mixin_of` compare mixins with `Heap::same_mixin`
-- by reference first, then by id -- so an old and a new application of
one id match: a handler for an exception class catches an instance made
before its class was edited. `reflection.rs`'s `instance_side`,
`Mixin::reflected_name` and `is_shipped` keep following the builder's
`Weak` link, which every fresh version holds to its own fresh enclosing
version: a renamed or re-accessed class answers its fresh version's name
and access, its class side its fresh instance side's, and no edit
changes the shipped flag. The image encoder plans its rows through the ids and names every
mixin by its id since road step 5b2b ("Edited programs in images"
below).

**What pins it.** The goldens under `goldens/library/mirror-builder/`,
held to their outcomes under both lowerings, written after the two
prior-art suites rather than copied: adding, replacing and removing
instance-side and class-side methods, an added method invisible until
`install`, removing a missing one changing nothing; adding, changing,
removing and replacing slots through the header, with old instances
`nil` in a new slot, new ones initialized, the other slots kept, a
superclass's new slot seen by a subclass's instances, a shadowing slot
added to a subclass and a slot added to a superclass under a shadowing
one each starting `nil`, a mixin applied twice keeping both levels'
slots, the identity hash kept and a hostile `=` never sent; an added lazy
slot computed on its first read; a subclass in another module and an
`Error` subclass migrated; two class objects of one nested mixin both
seeing an edit (§6.2), and a nested class object surviving its enclosing
class's slot edit; an implicit send of a nested class rebound when an
enclosing class gains the method; an enclosing class's edit keeping a
nested class's; the builder's `source`; a failed edit leaving the builder
as it was, and a failed install changing nothing -- with two builders
over one class, since one builder's checks include everything it staged;
the refusals; and running activations -- a method that installs
finishing with its old code, an initializer interrupted by an install
running on or raising, an old closure keeping its code. Every sweep runs
them, the round trip through `build` then `load` included. Rust tests pin the
splices and the refusals (`interpreter/edit.rs`: a second edit keeping
the first, a splice around multi-byte text keeping every character,
every refusal's diagnostic, a failed preparation changing nothing), the
install (`interpreter/install.rs`: a failed install changing no version,
application or guard, every level over an edited mixin re-applied, a
migrated instance replaced in every holder, the synthesized class side's
id, a class object's current application differing from an activation's
entered level), the successor chain over several
edits (`interpreter/mod.rs`), the leaves under both lowerings
(`builtins/reflection.rs`) and the id match of `inherits_mixin_of`
(`builtins/activation.rs`). What the complete install added is pinned
with it ("The complete install" below).

Not built: a shape-changing `setClass:`
([object-model.md](object-model.md), "`become:` is an eager bulk
forward", "Why eager"); editing kernel methods (the open question
below); and `ActivationMirror` and edit-and-continue
([debugger.md](debugger.md), "Hot swap and edit-and-continue").

### The complete install

The install changes everything a class declaration says, atomically
through the MirrorBuilder: a superclass, a nested class added, replaced
or removed, a class renamed or its access changed, a method added,
changed or removed, the factory and the initializer, a module's
identity; several builders install as one; and a module is declared from
source at run time. It is batch **A** of the debugger's hot swap, which
is split into three batches (decided with the user on 2026-11-22):
**A**, the complete install in the VM and `MirrorsFactory.ns`; **B**,
hot swap in the debugger -- saved module texts installed over DAP, the
plugin and the MCP server, breakpoints set again, frames running old
code marked, "Create Method" and the forced `nil`
([debugger.md](debugger.md), "Hot swap and edit-and-continue",
"Exceptions"); **C**, developing a whole application in the debugger,
from an empty world: a run without a program, evaluation without a
stopped frame, new modules joining the project. Batch A is built
(2026-11-27,
[its plan](../superpowers/plans/2026-11-27-1554-complete-install.md));
B and C are planned. The Newspeak implementations are the models
(`prior-art/newspeak/AtomicInstaller.ns`,
`prior-art/primordialsoup/newspeak/Mirrors.ns`); JVMTI's
`RedefineClasses`, which permits only method bodies to change, is not.
The rules are the book's: `r[semantics.reflection.atomic-install]`,
`r[runtime.install]`, `r[runtime.install.superclass]`,
`r[runtime.install.epoch]` and `r[runtime.install.module]`, and among
the VM invariants `r[vm.heap.install-commit-runs-nothing]`,
`r[vm.heap.obsolete-mixin-out-of-tree]` and
`r[vm.dispatch.invalidation-after-change]`. Its decisions, taken with
the user on 2026-11-22 (1 to 8) and 2026-11-27 (8's interface, 9, 10 and
11), as built:

1. **A superclass change re-evaluates the clause for every class object
   the program reaches.** A nested class's superclass is computed per
   enclosing instance (§6, `newspeak-spec.pdf` pp. 39-40), so the new
   superclass and `<:` clauses -- differing from the current ones as
   text (`clauses_as_written`) -- are evaluated before the commit, once
   per class object of the declaration, in the order the class objects
   were made (decision 10), each as program code in the enclosing object
   its declaration was evaluated in, as part of the class that declares
   it, against the unchanged world, caching nothing into a hidden slot
   (`Mutator::clause_evaluation`, `Step::ClauseEvaluation`,
   `interpreter/install/superclasses.rs`). A body-less declaration's
   class object, the level its last clause adds, runs its clauses in its
   declaration's own scope; a metaclass and any other `<:` level runs
   none. A clause that signals ends the install with what it signals; an
   answer that is no class, or one that makes a class inherit from
   itself, is refused; either way nothing changes. The commit, which
   still runs no code and polls no safepoint, re-parents each class
   object over its answer, keeping its identity, its `<:` levels built
   anew (`Mutator::reparent_body_less` for a body-less one), decides its
   value-class verdict again (`redecide_value_class`), and migrates its
   instances by `FieldName` -- a slot of a level the chain loses goes,
   one of a level it gains starts `nil`, no initializer runs again. A
   `<:` level elsewhere that names a re-parented class applies what that
   class applies once re-parented, in that class's enclosing object and
   declaring class, as `nested::build_applied_levels` would apply it
   now -- its last clause's mixin once it lost its body, its own once it
   gained one -- so the walk takes the chains over such a level too
   (`Mutator::follows_a_reparenting`), and the commit re-applies each
   class object after its superclass and after the class its clause
   named (`Mutator::reapply_order`; the goldens
   `a_level_naming_a_class_that_loses_its_body_applies_what_it_applies`
   and `a_level_naming_a_class_that_gains_a_body_applies_its_mixin`). A
   module's root evaluates nothing: its class object takes the superclass
   its fresh `root_shape` names, `Object`, `Value`, or `Value` applied
   over `Object`. Only class objects the program reaches are evaluated
   or refused: the Installer has a full collection served before the
   preparation it keeps and again before the commit ("The three leaves"
   below), since the walk of the space meets dead class objects too, and
   a clause's side effects would otherwise depend on the collector.
   Anything installed, or any module loaded, between the preparation and
   the commit fails the commit, told by the install epoch, and so does a
   class object of a changed declaration made while the clauses ran that
   the program still reaches at the commit, whose own clauses were never
   evaluated (`r[runtime.install.epoch]`). Neither prior art does this:
   `AtomicInstaller.ns` keeps the old superclass (`:187-188`) and its
   class comment names the change as unaddressed (`:4-6`), as does
   Primordial Soup (`Mirrors.ns:433-434`).
2. **Nested classes are paired by their path of names.** The same name is
   the same declaration -- its id, class objects and instances kept, its
   instances migrated, an `addFromSource:` of an existing name included,
   unlike Primordial Soup's fresh mixin (`Mirrors.ns:1475-1501`) -- a new
   name a new mixin, registered under the declaration it is added to
   (`PreparedInstall::added`), a missing one removed
   (`PreparedInstall::removed`). A nested class's text is read alone
   first and must be one class declaration, a leading `///` comment or
   metadata comment and trailing whitespace allowed, never two nor one
   followed by a member.
3. **A removed class lives on, obsolete, and the image carries it.** Its
   accessor and its hidden slots go, the enclosing instances migrating,
   so a send of its name is not understood; its id leaves its enclosing
   declaration's nested list and roots a tree of mixin ids of its own,
   its nested declarations and class sides in it, in no module's tree
   (`Heap::make_obsolete`, `Heap::is_obsolete`;
   `r[vm.heap.obsolete-mixin-out-of-tree]`); its class objects, its
   instances and its subclasses -- one in another module included --
   keep running its last version, `outer` inside it reaching its
   enclosing instance, as `AtomicInstaller.ns:234-240` leaves it. A full
   collection frees an obsolete tree on its own once nothing applies,
   reifies or holds any of its ids, whether or not its former module
   lives; a module freed first leaves the obsolete root naming no
   enclosing row ([object-model.md](object-model.md), "An application has
   two ids"). No builder edits an obsolete declaration, nor one nested in
   it (`<name> was removed by an install, and an obsolete class is not
   edited`). The image writes every obsolete declaration its graph
   reaches -- a class object applying, declaring or adding through a
   `<:` clause a mixin of it, a reified mixin of it, a closure or a frame
   running a body of its last or a superseded version -- as an obsolete
   tree after the modules' trees, with its last version's code and its
   text, and loads it back obsolete with no enclosing mixin
   (`Heap::make_loaded_obsolete`), so a running world with its heap stays
   serializable ([image-format.md](image-format.md), "Edited programs in
   the image"; "Planned: suspend and resume"); a loaded obsolete class's
   reflected name is its own, `Inner`.
4. **A rename keeps the class's identity**, after Primordial Soup's
   `ClassHeaderBuilder>>name:` (`Mirrors.ns:764-798`): a new name in the
   header, or `name:`, renames; a name that is no identifier and a
   member of that name in the enclosing class are refused; the accessor
   changes its selector, the old name is no longer understood, and the
   class objects, the instances and the classes nested in it stay,
   unlike `AtomicInstaller.ns:55-58`. A renamed child is paired with its
   old id by the name it had, and a later entry of the same install finds
   it by its new one. A hot swap from text cannot tell a rename from a
   removal and an addition, so batch B takes a rename from its client.
5. **A nested class's access can change.** The accessor carries the
   access in the enclosing class's method table, and its bindings name
   the class, so an access change or a rename makes the enclosing
   declaration part of the install, its subtree compiled again
   (`SplicedEdit::widens`) and re-applied without migration.
6. **The invalidation is scoped to the applications the install
   re-applies.** Before it replaces anything, the commit gathers the
   guards and indices of the applications it replaces and the versions
   of the declarations it replaces (`StaleIds`, `Mutator::stale_after`),
   and vacates exactly the entries naming one -- sites, polymorphic
   slots, the global cache, baseline cells, the verified pair -- by
   `InlineCaches::forget_freed`'s criterion (`InlineCaches::forget_stale`),
   and the nested-class accessor entries naming a replaced version,
   whose guard, the enclosing object's application, the install leaves
   alone; kept, such an entry would create the class of the old version
   and hold the superseded version alive. Every other entry stays warm.
   The optimized copies keyed on a re-applied application are found
   through a list per application of the copies depending on it, as
   HotSpot's dependencies (`code/dependencies.hpp:105-170`) and V8's
   `DependentCode` (`objects/dependent-code.h:37-70`) are, not by a scan
   of every copy (`JitState::dependents`, `copies_depending_on`;
   [execution.md](execution.md), "Invalidation"). The install traces one
   `caches` event, `vacated cause=edit`. The install stress mode,
   `NS_NEXT_STRESS_INSTALL`, the sixteenth, checks every remaining entry
   against a fresh lookup after each commit ([dispatch.md](dispatch.md),
   "Invalidation"; "Caches and their dependency keys" above).
7. **A top-level declaration from source is a module.**
   `ClassDeclarationBuilder fromSource:` checks a module's text as its
   install would, and the install declares it: one class declaration, a
   leading comment and trailing whitespace allowed, parsed, resolved with
   `resolver::resolve`, never `resolve_kernel`, and its `:module:` read
   by `modules::declared_identity` and held to `check_unreserved` and,
   with every identity the same install makes, to `check_distinct`
   against the world, as the loader holds a project's, an id and version
   the world already has refused. The commit registers its root with its
   tree of ids, its text, every character of it, its current text, and
   raises the install epoch, as a module's load does; the builder then
   stands for its declaration, which `declaration` reflects. One without
   `:module:` is undeclared: its id is its class's name, without a
   version. Such a module has no path, since no file holds it
   (`ModuleIdentity::path` is an `Option`), and is named `a module
   declared from source` in the loader's messages
   (`modules::PATHLESS_MODULE`); the image's module section writes it
   with the origin tag `ORIGIN_NONE` (4) ([image-format.md](image-format.md),
   "Module section"; [modules.md](modules.md), decision 1). The manifest
   a running application holds is unchanged, and so are the tools'
   module lists -- `ns-cli`'s `known_modules`, the trace filter, the DAP
   agent -- which learn no module from source; wiring a new module into
   either, and a class object of one made by Newspeak code, are batch
   C's ([OPEN-WORK.md](../OPEN-WORK.md)).
8. **`installAtomically:` installs several builders as one.** Each
   builder's operations are spliced into its module's text in the order
   given, the builders of one module composing on one text; every module
   is prepared and every clause evaluated, then one walk, one commit and
   one invalidation; two builders of one declaration, or one whose
   declaration another removes or replaces, are refused, naming both --
   the check `MirrorsForSqueak.ns:1698` marks `#BOGUS` as missing -- and
   any failure changes nothing. Its interface is Primordial Soup's
   `nestedClasses` group (`addFromSource:`, `removeMirrorNamed:`), the
   header builder's `name:`, `ClassDeclarationBuilder fromSource:`, and
   `installAtomically:` on the mirrors facet. The collection is copied
   first, and an element that is not exactly an instance of the mirrors
   module's own `ClassDeclarationBuilder` is refused before any builder
   is read: the commit hands a builder from source its reified mixin,
   which a program's subclass could otherwise keep. A builder hands its
   entry (`entryFor:`) and takes its installed texts (`installed:by:`)
   only for this module's exact `Installer`, which no other object
   receives, so the reified mixin is handed to no program object.
9. **An install changes a module's identity in place.** A module's
   identity is its path ([modules.md](modules.md), decision 1), which no
   install changes, so a text whose `:module:` names another id or
   version, or whose root class has another name, is the same module
   with other attributes: the root mixin's new version carries a new
   `ModuleIdentity` of the same path, and its mixin ids, class objects
   and instances are kept, every other change of the install reaching the
   running instances. A `name:` renaming an annotated root rewrites the
   last part of the id in its `:module:` too, the rest of the payload
   untouched, as the names of the classes nested in it follow (decision
   4), after Primordial Soup's `name:`, which renames a top-level class
   in place (`Mirrors.ns:764-798`). A module named by `newspeak.json`'s
   `as` keeps that id across an install, a rename replacing its last
   part, and takes the version its text declares, as
   [modules.md](modules.md)'s decision 5 gives the version to the text.
   The new identity is held to the loader's rules before the commit --
   [modules.md](modules.md)'s decisions 2, 5 and 9: a well-formed
   version, an id whose last part is the class's name, none under
   `newspeak.`, and no other module of the world of that id and version,
   or of that id where either has none (`Heap::module_identities`) --
   with the loader's messages, and a failure changes nothing. Nothing on
   a send's path reads an identity, so it costs the hot path nothing,
   adds no invalidation to decision 6's and leaves every warm entry warm;
   the image writes the current identity under the unchanged path. The
   manifest a running application holds is unchanged (decision 7): it
   answers the module under the id it was assembled with. A breakpoint
   naming the old id or version is batch B's to set again. Neither making
   a new module beside the old, which would leave the install's other
   changes out of the running program and put two modules of one path
   into the image, nor refusing the change, JVMTI's answer
   (`jvmtiRedefineClasses.cpp:1405-1407`), is taken.
10. **The clauses run in creation order.** Every class object carries a
    creation stamp, a `SmallInteger` drawn from a heap counter whenever
    one is made (`class_field::CREATION_STAMP`, after every field machine
    code reads; `Heap::class_creation_stamp`), which no collection, move
    or image changes, so the order in which an install evaluates clauses
    -- ascending stamps -- does not depend on where the collector laid
    the class objects out. The image's class, body-less class and
    metaclass records carry the stamp; the decoder holds the stamps
    unique, by sorting them once every record is read, and below 2^59
    (`CLASS_STAMP_LIMIT`), so that a loaded world keeps stamps to draw
    above the image's highest, and the loader moves the counter past the
    highest (`r[image.load.class-stamps]`). Chosen over a weak
    creation-order registry, a new table to keep through collections,
    moves and reclaims, whose order after a load would be the graph's,
    and over an unspecified order, which would make a program's
    behaviour depend on the collector.
11. **Every class object has one layout.** Each carries an applied
    level's fields -- the class its `<:` clause named and its
    declaration's scope, `nil` on an ordinary class object -- so twelve
    fields, 112 bytes ([object-model.md](object-model.md), "The object
    layout"), and a declaration that gains or loses its body of its own
    is installed in place: each class object is rewritten, keeping its
    identity (decision 2), becoming the level its last clause adds or the
    class of its own body over the levels its clauses add, and its
    instances migrate by field name. Chosen over forwarding class
    objects, a new kind of forward that would weaken the lock that the
    bulk forward forwards instances only, and over refusing the change,
    a carve-out. The image record writes only the stamp new: a record's
    kind says where those fields are non-nil.

**The three leaves.** A leaf runs no Newspeak code, so a superclass
clause is evaluated between two of them, and the install is three
`MIRROR` rows, appended after `block:valueWithArguments:ifFail:`, reported
as `prepareInstall`, `evaluateClause:epoch:` and
`commitEpoch:superclasses:`, which only `MirrorsFactory.ns`'s `Installer`
and `ClassDeclarationBuilder` send ([core-library.md](core-library.md),
"The VM mirror is the whole Rust boundary"; `builtins/reflection.rs`).
Each takes the install's entries, an `Array` with one entry a builder, in
the builders' order, `{subject. edit}`: `subject` the reified mixin of
either side of the edited declaration and `edit` the check's four-element
`Array` ("The edit on the wire, and the check's answer" above), or, for
a module declared from source, `nil` and the module's text, a `String`
(`InstallEntry::Edit`, `InstallEntry::Module`):

- `reflectionPrepareInstall:ifFail:` prepares the install and commits
  nothing (`Mutator::prepare_install`), answering `{true. epoch.
  clauses}` -- the install epoch the commit is to name, and an `Array`
  of `{classObject. enclosingObject}`, one per class object of a
  declaration whose clauses change, in creation order
  (`Mutator::clause_class_objects`) -- or `{false. diagnostic}`.
- `reflectionInstall:evaluateClause:epoch:ifFail:` takes one of those
  class objects and the epoch, and answers a step activating the fresh
  clause body on the enclosing object, its current class the class
  object's declaring class (`Heap::class_declaring`), as the
  interpreter's own clause activation does, with a plain delivery of the
  answer; it declines, and its block runs, for a moved epoch or an
  object that is no such class object.
- `reflectionInstall:commitEpoch:superclasses:ifFail:` takes the epoch and
  an `Array` of `{classObject. answer}` pairs, and commits
  (`Mutator::commit_install`), answering `{true. results}`, per entry
  `{declarationText. headerText}`, or for a module declared from source
  `{reifiedMixin. declarationText. headerText}`, or `{false.
  diagnostic}` with nothing changed.

Each declines, and the Installer signals `a MirrorBuilder takes a member
or a header as a String and a name as a Symbol`, for entries it cannot
read; the commit also declines pairs that are not each a distinct class
object the preparation answers. The class objects travel through
Newspeak in the `Array`s alone, since no handle may be held across the
safepoints between the leaves (`r[vm.handle.nothing-carried-across-safepoint]`):
what the leaves keep is one prepared install (`PreparedInstallCache`),
the epoch, the entries and what they prepared to, `Rc<Mixin>`s, ids and
texts, which a later preparation or commit of the same entries at the
same epoch takes instead of compiling again, the check's included. A
commit and a full collection's reclaim of ids drop it, since the ids it
names may be minted again. The install epoch, a `u64` on the `Mutator`,
is raised by every commit and every module that joins the heap
(`load_root_class`, and a module declared from source); it is not the
debugger's text revision, which moves without an install too.

`Installer>>install` makes the three calls: it refuses a builder that
is not exactly this module's `ClassDeclarationBuilder`, gathers the
entries, and, when a first preparation -- in a method of its own, so
that its answer dies with it -- says that clauses change, has a full
collection served at a safepoint (`vmMirror garbageCollect:`); it then
prepares, evaluates each clause, has another full collection served when
there were any, so that a class object a clause made and dropped is no
class object the commit refuses, commits, and hands each builder its
texts. An install of no builders answers at once, preparing and
committing nothing and leaving the epoch, so that it fails no install it
runs within.

**What it costs.** Nothing on a send's path: the stamp and the applied
level's fields lie after every field machine code reads, and no
identity, epoch or obsolete mark is read by a send. A class object is
112 bytes where it was 80 to 104. An install that keeps every clause
costs what it cost before but for a narrower invalidation, so the
warm-up after it is that of the entries it vacated alone; one that
changes a clause costs two full collections and one activation per
class object the program reaches. An image grows by eight bytes per
class or metaclass record and four for the mixin table's obsolete count.

**What pins it.** The goldens under `goldens/library/mirror-builder/`,
held to their outcomes under both lowerings and through `build` then
`load`: two builders installing as one, two of one class refused, a
failed atomic install changing nothing, an atomic install taking only
builders; a nested class added, one with a clause, one replaced with its
instances migrated, one removed running on obsolete, a builder of a
removed class refused; a renamed nested class answering its new name, a
rename to an existing member refused, an access change installed; a
renamed root rewriting its `:module:` id, a version change installed in
place, an `as` id kept, a colliding identity refused; a superclass
change re-parenting every class object, a `<:` clause added, a slot of a
lost level going and one of a gained level starting `nil`, a clause that
signals changing nothing, one answering no class and a cycle refused, a
clause that installs failing the commit, a class object made during the
evaluation failing it, a clause running only for the class objects a
program reaches, the clauses running in the order their class objects
were made; and a module declared from source, one without metadata named
by its class, one of a taken identity refused, a text of two
declarations refused, a module and an edit installing as one. Rust
tests pin the scoped invalidation
(`an_install_vacates_only_entries_naming_a_reapplied_application`,
`interpreter/inline_cache.rs`) and its trace
(`an_install_is_traced_vacating_the_caches_once`, `tests/trace.rs`), the
INSTALL check against corrupted entries of every kind
(`interpreter/inline_cache/verify.rs`), the obsolete class's reclaim
(`an_unreachable_obsolete_class_is_reclaimed`, `interpreter/reclaim.rs`)
and load (`an_obsolete_class_loads_obsolete`, `interpreter/install.rs`),
the texts of two declarations and the splice around multi-byte text
(`a_nested_class_text_of_two_declarations_is_refused`,
`a_nested_class_splice_keeps_multibyte_text`), the stamps' decoding
(`image/decode.rs`) and the leaves under both lowerings
(`builtins/reflection.rs`).

**Its limits.** The INSTALL check compares a lexical site's non-private
entry, whose key does not name N, with a lookup made without N's private
member, which the global entry of the same send is checked with, and
skips megamorphic sites, sites of loose ranges and entries whose key no
live object has. What the batch leaves open is in
[OPEN-WORK.md](../OPEN-WORK.md): an install from inside a clause of a
creation in progress, a re-parenting onto or off a builtin with a native
representation, and a debugger evaluation at the collection's safepoint.

### Edited programs in images

A program a MirrorBuilder edited is written to an image and loads back
edited, and a program loaded from an image with source is edited like one
read from source. This is road step 5b2b (split from road step 5b2a on
2026-10-18 and, on 2026-10-19, from road step 5b2c's reclaiming of ids, the
details decided with the user that day; built on 2026-10-19,
[its plan](../superpowers/plans/2026-10-19-1702-edited-programs-in-images.md)).
Every golden, those under `goldens/library/mirror-builder/` included,
goes through `build` then `load` again.

- **Every declaration holds its own text, its nested declarations cut
  out** (decided with the user on 2026-10-19), so no version keeps
  another's text alive, every span slices the text of its own version, and
  a module's text is assembled from its declarations' current versions
  ("Compiling: the module's text is what an edit edits" above;
  [object-model.md](object-model.md), "Source retention and provenance").
- **The image's rows are the declarations' current versions**, named by
  id, and **the code an older version runs is in the code table**, a frame
  naming its body by code id (decided with the user on 2026-10-19): a
  method a closure or a frame still runs after an install replaced or
  removed it runs its old code after a load
  ([image-format.md](image-format.md), "Edited programs in the image").
- **A removed declaration the graph reaches is an obsolete tree** after
  the modules' trees, which loads obsolete ("The complete install"
  above, decision 3), and a module declared from source is a module
  record without a path (decision 7).
- **An image carries its program's source by default** (decided with the
  user on 2026-10-19), each declaration's current text once, and
  `ns-next build --strip-source` leaves it out
  ([image-format.md](image-format.md), "Source").
- **A loaded program is edited from the text the image carries.** The
  loader never parses; the edit assembles the module's text and parses it,
  as for a program read from source. A program of a stripped image is
  refused (`<name> has no source text to edit`), and so is one whose
  crafted text lacks the declaration (`<name>'s source text does not hold
  its declaration`); a crafted text that holds it is edited into what it
  says, which is a valid program. Both refusals change nothing
  (`a_loaded_program_whose_text_lacks_the_declaration_is_refused`,
  `a_loaded_program_without_source_is_refused`, `interpreter/install.rs`).
- **An old body's text is not carried yet.** In the live heap a superseded
  version keeps its text as long as something holds it; nothing yet asks a
  body for its text -- a method mirror reads the current version's
  methods -- so an image carries the texts of the rows it writes, and the
  link from a body to its text comes with the activation mirror that will
  read it ([OPEN-WORK.md](../OPEN-WORK.md)).

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
both, so the two paths would disagree; the MirrorBuilder's refusal of
every edit to a shipped mixin, in `asBuilder` and again in
`Mutator::prepare_edit`, is what seals it
(`r[vm.mirror.shipped-mixins-never-edited]`). **Open:** how a replaced *kernel*
method is compiled, should editing kernel methods ever be offered. Its
primitive-method mark and its `SendMirror` rest on
`Binding::MirrorSend`, which only shipped core-library source's
resolution records, and
recording it for an edit would hand that edit the whole Rust boundary
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary").

### An edit replaces the mixin under its id

A `Mixin` stays an immutable Rust value outside the object space, and its
identity is its `MixinId`, the row of the heap's mixin table
(`heap/tables.rs`); an edit builds a fresh, immutable `Mixin` and installs
it under the old mixin's id, so everything that names the mixin by id sees
the edited one at once (decided with the user on 2026-10-05). The id's half
is built by road step 5b1 (2026-10-18,
[its plan](../superpowers/plans/2026-10-18-1236-become-and-recorded-guards.md)),
the install that edits a live class through it by road step 5b2a ("A
MirrorBuilder and an atomic install" above).

**Every version answers one id** (`r[vm.dispatch.mixin-id-one-per-mixin]`).
`Heap::mixin_id` answers the id a mixin
-- any version of it -- was first registered under, registering the
enclosing mixins not registered yet first, outermost first; the row holds
the current version (`Heap::mixin`) and the id of the mixin it is declared
in (`Heap::enclosing_mixin_id`); each version holds its own text.
`Heap::replace_mixin` makes a fresh mixin the
current version of an id: the fresh one's address answers the id from
then on, as every earlier version's still does, since the versions it
superseded are held (`Tables::superseded`), and the id keeps the enclosing
mixin it was registered with. It panics, as a compiler-invariant
violation, on a replacement that already has an id, on a shipped current
version, and on a class side replacing an instance side or the other way
round. `Mutator::replace_mixin` calls it and forgets the applications and
the synthesized class side built from the previous version, so the next
application of the id applies the replacement; applications already built,
and the class objects naming them, keep the version they were built from,
which the install rewrites ("A MirrorBuilder and an atomic install"
above). The application cache is keyed by
the id and applies the id's current version on a miss.

§6.2 requires an edit to a mixin to affect all of its applications, so the
edit has to reach the mixin's identity, not a class object: the tests'
`install_mixin` (`tests/image.rs`), which re-applies a *fresh* mixin,
re-points one class object and leaves the old application's other class
objects, every subclass and every existing instance running the old code.

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

**What the id has to carry.** On the id now: the hidden nested-class slot
(`MixinApplication::nested_slot`, keyed by `MixinId`), the shipped world's
two maps (`ShippedWorld::index_by_mixin`, `declared_by_mixin`), the
application cache, a mixin's enclosing link, which the reflection leaves
read through the id, a mixin's nested declarations, which registration
takes out of the declared tree into the id's row (`Heap::nested_of`), so
that every reader -- the lookup of a nested-class accessor, the
reflection leaves, the install, the kernel phase, the shipped world, the
image encoder -- reaches each one's current version through its id, a
module's current text, assembled through those ids
(`Heap::assembled_text`), the image encoder's
rows and every record naming a mixin (`image/mixins.rs`'s `plan` and
`MixinIds`), the names an install migrates a field
by (`FieldName`), and `builtins/activation.rs`'s `inherits_mixin_of` and
`Heap::with_level_applying_mixin_of`, which compare mixins with
`Heap::same_mixin`, by reference and then by id, so an old and a new
application of one id match. A synthesized class side keeps its id across
an edit: the install replaces it under the old one's id rather than
letting the next application mint a fresh one. Still on a mixin's address
or its builder's link: `reflection.rs`'s `instance_side`,
`Mixin::reflected_name` and `is_shipped`, which follow the builder's
`Weak` link and stay so -- a name and an access are the version's own,
which a rename or an access change gives a fresh version, and the
shipped flag no replacement changes. Until a heap registers a mixin, its
nested declarations are the tree its builder declared
(`Mixin::declared_nested`), which the compiler, the shipped-code decoder,
the image's mixin builder and an edit's pairing read; registering a mixin
registers its whole declared tree, and a mixin is registered in at most
one heap (`r[vm.handle.mixin-registered-in-one-heap]`). A full collection
reclaims what live editing leaves behind by
the ids the live objects name, gathered while marking, as HotSpot unloads
classes (decided with the user on 2026-10-03; built by road step 5b2c on
2026-10-20, [its plan](../superpowers/plans/2026-10-20-1318-reclaiming-ids.md);
[object-model.md](object-model.md), "An application has two ids", has the
rules and the order): the applications nothing names, with their guard
ids, and the successors of dead applications; the application cache's
entries for them, so the cache is weak ([memory.md](memory.md), "Weak
references"); a module's tree of mixin ids, as one, once no live
application, class object, reified mixin object or Rust holder names any
of it, and an obsolete declaration's tree by the same rule, on its own
("The complete install" above, decision 3); a superseded version once only the table holds it and no closure,
frame or activation runs code compiled in it; and every code id and cache
range nothing names. Every table mints its smallest free row first and
gives its trailing free rows back.

**The image follows.** Because a mixin is a table row rather than a graph
object, an edited mixin reaches an image only because the encoder takes
the rows from the mixins the program names now, through the ids, rather
than from what its modules declared (decided with the user on 2026-10-05,
built with road step 5b2b; [image-format.md](image-format.md), "Edited
programs in the image").

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
§3.1.1's identity method as a send of `=`, so a value has no observable
identity (`r[semantics.value.identity]`; [core-library.md](core-library.md)).
All of §3.1.1's second bullet is enforced (`r[semantics.value]`,
`r[semantics.value.conditions]`;
[object-model.md](object-model.md), "A value object's four conditions"). So
a value class lives in a module that declares itself a value, `= Value` as
`Kernel.ns` does, or with §6.2's mixin application, `= Object <: Value`.

A class object is a value exactly when its enclosing object is, so a module
definition, which has none, is one (§6.4), and so is a class nested in a
value; such a class has no identity either, its `=` and `hash` derived from
its state ([object-model.md](object-model.md), "Class sides and
metaclasses").

What is missing is the boundary: there are no actors
(`r[semantics.gap.actors]`), so nothing copies a value across one. Neither is
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
- **Tier 1 — baseline JIT.** One code template per instruction, emitted
  by the repository's own macro assembler (road step 8, built;
  [execution.md](execution.md), "The baseline JIT's foundation").
- **Tier 2 — optimizing JIT.** Speculative inlining driven by IC feedback, escape
  analysis, unboxing, guarded by deopt (road step 9, designed, its
  first two stages, the foundation and speculation without inlining,
  built; [execution.md](execution.md), "Planned: the optimizing JIT",
  "The optimizing tier's foundation", "Speculation").
- **Deopt metadata** — safepoint ↔ machine-code position ↔ abstract interpreter
  state, under the contract of [execution.md](execution.md), "Planned: the
  deoptimization contract": every observer runs after machine code's exit to
  the run loop (`r[vm.machine.observers-after-exit]`), which materializes every
  compiled frame as interpreted ones.
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
