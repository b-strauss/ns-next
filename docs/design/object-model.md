# Object model — values, objects, mixins, classes

> **Status:** built; the road to a high-performance VM -- an eight-byte
> value, the object layout and `become:` among its steps -- is planned
> ("Planned: the road to a high-performance VM").
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `types.rs` (`Object`, `ObjectData`, `Value`),
> `mixin.rs`, `interpreter/instantiate.rs`, `interpreter/nested.rs` (the
> nested-class accessor and `build_applied_levels`, §6.2's levels),
> `interpreter/kernel.rs` (the builtin classes, filled from `Kernel.ns`),
> `interpreter/value_class.rs` (§3.1.1's conditions on a value class),
> `interpreter/future.rs` and `compiler/simultaneous.rs` (§6.3.2's
> simultaneous slots).

## Values and objects

```rust
Value = Nil | Boolean(bool) | SmallInteger(i64) | Float(f64) | Object(ObjectReference)   // Copy
ObjectReference = a 32-bit index into the `Heap`'s object arena; ClassReference is an alias

Object { class: ClassReference, identity_hash: u32, dispatch_id: DispatchId, data: ObjectData }

ObjectData = Instance(Vec<Value>) | String(String) | Symbol(Symbol)
           | Array(Vec<Value>) | Closure(ClosureData) | Class(Box<ClassData>)
```

`dispatch_id` is the id an inline cache guards the object's sends on, a copy
stamped from its application's ids, and a class object's data is boxed to keep `Object` at 56
bytes in a release build with it ([dispatch.md](dispatch.md), "The dispatch
id").

`nil`, `true`, `false` and small integers are `Value` variants rather than heap
objects, which is why nothing resembling an object store is needed to name them
in a serialized image.

**A symbol is a heap object, and a canonical one**: each `Heap` keeps a
canonical table, `Vec<Option<ObjectReference>>` indexed by the number inside
a `Symbol` -- which the interner assigns densely -- and `Mutator::new_symbol`
answers the table's object, allocating it only the first time. It is the one
way to make a symbol, so a symbol literal, `asSymbol` and every selector
handed to `doesNotUnderstand:` all reach the same object for the same text,
which is what §5.1.6 and §3.1.1 make two equal symbols. The table is per heap
because a handle means nothing against another heap. It is weak: the
collector clears an entry whose symbol nothing else holds ([memory.md](memory.md),
"Weak references"). This is what every reference does -- Primordial Soup's
weak `symbolTable` (`KernelForPrimordialSoup.ns`), SOMns's
`Symbols.symbolFor`, Squeak's symbol table -- and it keeps a symbol an
indexable object for a kernel where `Symbol` is a subclass of `String`, as
the references declare it; `Kernel.ns` still puts `Symbol` directly under
`Value`, beside `String` rather than below it, which the object layout
changes ("Planned: the object layout" below). Rejected: a symbol as an immediate `Value` variant, which no
reference does and which every string primitive, and later the Newspeak
kernel, would have to accept as a non-object. An image carries a symbol as a
record; the decoder enters each one into the loading heap's table and refuses
a second record of the same text
([image-format.md](image-format.md), "Verification").

**An `ObjectReference` is an index, not a pointer.** Every `Object` lives in one
`Heap`'s arena and is reached only through a `Heap` accessor; the index is
private to `crates/ns-runtime/src/heap.rs`, so the compiler refuses to read it
anywhere else. `Value` is therefore `Copy` and a handle has no destructor. What
that representation buys, how the collector reclaims a slot once nothing
reaches it, and why a handle means nothing against a different `Heap` are in
[memory.md](memory.md).

**The identity hash is drawn from a monotonic counter the first time `hash`
asks for one**, never derived from an address and never from the arena index,
and 0 means "not yet assigned" (`Heap::ensure_identity_hash`). An address- or
index-derived hash would break the moment a moving collector relocates the
object or a `become:` swaps identities, and identity has to be stable across a
snapshot; see [memory.md](memory.md). Assigning it lazily, as Cog/Spur
(`hashBitsOf:`) and Primordial Soup (`Object_identityHash`) do, keeps
allocation order unobservable: the reference lowering allocates closures the
optimized one does not, closure conversion allocates an indirection vector the
reference lowering does not, and a JIT's escape analysis will add and remove
allocations the same way, so a hash drawn when the object is
allocated would change a program's `hash` values under any of them. The encoder writes the
stored value without assigning one, and the decoder keeps every value, 0
included.

That stability across a snapshot holds for every object the image carries as a
record, and also for **the objects it names by tag** — a builtin class, the
module's root class and the `platform` singleton — even though none of them is
a record: the image records their hashes separately, in a closing table of the
graph section, precisely because nothing about a record would otherwise carry
one; see [image-format.md](image-format.md), "Identity hashes are preserved
verbatim".

`ClosureData { code, captured_frame: FrameReference, copied, feedback_base }`
captures its defining frame, and derives its home activation for non-local
return from it; `copied` holds what closure conversion handed it -- a copied
variable's value or an enclosing body's indirection vector
([execution.md](execution.md), "Closure conversion"). `code` is an `Rc` shared
with the `MakeClosure` instruction that made the closure, and its parameter
count is the body's; the payload itself is inline in the closure
object, never behind an `Rc` of its own, because a handle lives only where the
tracer can rewrite it in place ([memory.md](memory.md), "What the collector
traces") and an `Rc` shared by several holders is not such a place. So a block's
activation (`FrameKind::Closure`), a native `whileTrue:` or `do:` loop's state
and a decided block step name the closure object, and reach its body and its
captured frame through it. **The home is a `FrameReference`, not an
`ObjectReference`** — it names the lexically enclosing *method* or initializer
activation, which is what a non-local return aims at, and a frame records it
(`Frame.home`); the enclosing *object* is the tempting misreading, since both
are handles.

**Never `{:?}` a `Value`.** `ObjectReference`'s hand-written `Debug` prints its arena
index only, so this cannot recurse -- but it renders Rust internals such as
`ObjectReference(#42)`, useless in a message meant for a Newspeak programmer.
`Heap::describe` answers "the String \"foo\"" instead.

## Three layers, not one

Milestone 1 had a single `ClassData` carrying identity, behaviour and layout
together. That is wrong, and the language specification says why directly.
`newspeak-spec.pdf` §6:

> Every instance of a class C has a distinct nested class for each nested class
> declaration given in the declaration of C and its superclasses. [...] each
> nested class has one piece of state — its enclosing object, and so there must
> be a distinct class per instance of the enclosing class.

An enclosing object is per-instance state, so a class that has one cannot be
shared between instances of its enclosing class. §2.3 adds the other half: a
nested class declaration can be overridden by a slot in a subclass of its
enclosing class, and that slot can be assigned at run time —

```newspeak
class ChildOfOuter = Outer () (
    public class Nested = Inner () ()
)
class GrandChildOfOuter = ChildOfOuter( | public Inner ::= String. |) ()

g Nested.                (* a subclass of String, not of Inner *)
(g Inner: Array) Nested. (* still the same subclass of String *)
```

— so "all nested classes must be compiled as mixins, as the superclass cannot be
reliably known at compilation time". And §6.2 names the sharing axis: "Any change
made to the structure or code of a mixin effects all of its applications. [...]
they all share their code and structural definition with the mixin." That
sentence is simultaneously the performance strategy and the edit-and-continue
requirement.

Hence three layers:

```rust
struct Mixin {                    // one per declaration, shared for its lifetime
    name: Symbol,
    methods: IdentityMap<Symbol, MethodEntry>,  // compiled methods, slot accessors;
                                                // primitives on the VM mirror's alone
    slots: Vec<MixinSlot>,        // name, mutability, accessibility
    initializer_expressions: Vec<ast::Statement>,
    factory: Option<Rc<ast::MessagePattern>>,
    nested: Vec<Rc<Mixin>>,       // the nested declarations, for their accessors
    class_side: Option<Rc<Mixin>>,
    mixin_applications: Vec<MixinApplicationClause>,  // its `<:` clauses (§6.2)
}

struct MixinApplication {         // (mixin, super application) -> behaviour
    mixin: Rc<Mixin>,
    superclass: Option<Rc<MixinApplication>>,
    own_slot_base: usize,         // where this level's own slots start
    slot_count: usize,            // inherited slots included
}

struct ClassData {                // one per enclosing instance, per level
    application: Rc<MixinApplication>,
    enclosing: Option<ObjectReference>,
    declaring_class: Option<ClassReference>,    // the declaration chain `outer` walks
    superclass: Option<ClassReference>,         // parallel to the application chain
    builtin: Option<BuiltinClass>,
    this_class: Option<ClassReference>,         // a metaclass's class; its own metaclass is its header class
    applied_clause: Option<Box<AppliedClause>>, // the `<:` clause that added this level
}
```

**The application cache is keyed by (mixin identity, superclass *application*
identity), never by the superclass class object.** If the superclass is itself a
nested class, its class object is per-instance, so a class-object key would never
hit: every instance would mint its own application, which is wrong per §6.2 and
would leave every call site megamorphic because no two instances would ever share
behaviour.

**Identity-keyed tables hash with `hash::IdentityMap`.** A mixin's `methods`, an
application's `nested_slot` and the interpreter's `applications` and
`metaclass_mixins` are keyed by an identity the runtime assigned — an interned
symbol's index, a mixin's or application's address — which no input chooses, so
SipHash's resistance to chosen keys buys nothing there. They hash with one
multiply per word and a rotation that brings the mixed high bits down to the
bucket index (the FxHash scheme rustc's own tables use).

### Two parallel chains, both needed

- The **class-object chain** carries identity and, at every level, that level's
  enclosing object plus the class object of the level that lexically declares it
  (`declaring_class`).
- The **application chain** carries code and layout, and is shared.

`own_slot_base`, `slot_count` and method lookup live on `MixinApplication`
rather than on `ClassData`, because a superclass contributes slots and can
differ between instances of the same enclosing class.

**A slot is reached through its accessors, which are method-table entries.**
§6.3.2 has every slot declaration define a getter and, for a mutable slot, a
setter, each with the slot's accessibility; they are entries of the declaring
mixin's `methods` beside the compiled methods, found and overridden like any
method ([dispatch.md](dispatch.md), "Slot accessors"). An entry names its slot by
position among the mixin's *own* slots, so one entry serves every application:
the absolute field is the defining application's `own_slot_base` plus that
position — the same mixin-relative addressing `InitializeSlot` uses, and for the
same reason. Nothing maps a slot name to a field; nothing but an accessor needs
to.

**A lazy slot (§6.3.3) is an own slot like any other**, laid out at its
position and starting `nil`, with two differences. The instance initializer
skips it; its getter is a compiled method in the mixin's `methods`, with no fast
path, which opens with `ReturnSlotIfFilled` -- answering a filled slot from
its own activation -- and otherwise evaluates the initializer and stores it
with `InitializeSlot` ([dispatch.md](dispatch.md), "Slot accessors"), and a mutable one's setter is an ordinary accessor entry.
And it is outside a value's state: `mixin::declared_slots_of` leaves it out of
`MixinApplication::declared_slots`, so the state-derived `=` and `hash` skip it
([core-library.md](core-library.md), "Lazy slots first").

## The nested-class accessor

A nested declaration implicitly defines a method with the same name and
accessibility in its enclosing class. §6:

> The first time this method is invoked on a given receiver r, it evaluates the
> class declaration, using r as the enclosing object, and returns the resulting
> class as its result. Subsequent calls on r always return the same class object.

The accessor body, in five steps:

1. Read the hidden slot reserved for this declaration in the receiver's layout.
   If it is not nil, return it.
2. Otherwise evaluate the superclass clause **in the receiver's context**,
   yielding a class object; take its application. A declaration with `<:`
   clauses evaluates their receivers in the same activation, left to right
   after the superclass's, and builds the levels they add ("Mixin
   application" below) before step 3, which then applies the declaration's
   own mixin to the top one.
3. Look up (mixin, that application) in the application cache, creating the
   application if absent.
4. Allocate the class object `{application, enclosing: r, superclass}`.
5. Store it in the hidden slot and return it.

Caching in a hidden slot rather than a side table is what §6 describes — "the
class is effectively cached within r" — and it makes the fast path a slot read
instead of a hash lookup.

**Inherited nested declarations get their own slots.** §6 says a distinct nested
class exists for each declaration given in the declaration of C *and its
superclasses*, so the hidden slots are part of the absolute layout an application
computes, contributed at the level that declares them, exactly like ordinary
inherited slots. They are derived from a mixin's `nested` list rather than
declared, so nothing stores them separately. The hidden slot is keyed by the
declaration's mixin, not by its name: keying by name would make an override and
the declaration it overrides share one slot.

**§2.3's override behaviour needs no special case.** Because the accessor is an
ordinary method, a slot declaration in a subclass overrides it under the normal
lookup rules, and reassigning that slot later cannot disturb an already-cached
nested class. That this falls out of ordinary dispatch is evidence the layering
is right.

**The accessor lives in `interpreter/`**, beside dispatch. It evaluates Newspeak
code — the superclass clause, compiled like any other body — which runs as a
`Step::SuperclassClause` continuing with `Continuation::NestedClass`, the way
instantiation runs the clause's deferred message as a `Step::ClauseMessage`
([execution.md](execution.md), "No re-entry from Rust"); steps 3 to 5 run when
the clause returns. While it runs, the class's own cache slot holds a marker
that its creation is underway, which is what detects a cyclic definition. It
is deliberately not
synthesized bytecode: what it does
around that code — caching the class object per enclosing instance, detecting a
cyclic definition, applying the mixin — is dispatch-side work, and a
compiler-side copy of it would drift from the interpreter's in exactly the way
the rule that `vm.rs` holds no dispatch logic exists to prevent.

## Module loading

Loading is two-stage. `load_module` builds the **mixin tree** — root declaration,
every nested declaration, every class side — as pure structure with no
evaluation. Only the root class is then created, as the application of the root
mixin to its superclass, with `enclosing = nil` (§3.3: a module definition's
enclosing object is nil, which is why a module has no state of its own).

Building the tree also compiles it — every method, initializer and superclass
clause, nested classes and class sides included — onto the declarations
themselves: a method's body onto its `MixinMethod`, a mixin's initializer and
clause bodies onto the `Mixin`, before the mixin is shared
([execution.md](execution.md), "The VM"), so a class `load_module` returns is
already runnable. A mixin decoded from an image carries the bodies its row
names instead, reconciled on load by `image::decode::verify_mixins`. Nothing it
does evaluates Newspeak code, and compiling a resolved module is total — a
program-level error is deferred to the execution of the body that raises it
([compiler.md](compiler.md), "Compilation cannot fail") — so loading stays
total.

## The builtin classes

A builtin class — `Object`, `Integer`, `Array` and the rest of
`BuiltinClass::ALL` — is an ordinary class object whose `builtin` field names
its kind, and whose behaviour is a nested class of `Kernel.ns`
([core-library.md](core-library.md), "The bootstrap: placeholders first,
kernel second"). The bootstrap builds each class object first, over a
placeholder mixin, because a literal needs its class before any Newspeak has
been read, and then gives it its kernel class's application with
`Heap::set_class_application`. So the class object is the same one every
literal has had from the first instruction on, and its behaviour is the
kernel's.

- **Superclasses follow the kernel's clauses.** `Top` has none — §3.2's
  empty class, which declares nothing so that all code, `Object`'s included,
  resides in a mixin, and which only `Object` may name as its superclass.
  `Object` is below it, and every other class is below `Object`, as §3.2
  requires ("except for `Top` all classes are subclasses of class
  `Object`"). `Value` is below `Object`, and below `Value` are §3.1.1's value
  classes: `Boolean` with `True` and `False` (decided with the user on
  2026-09-22, as both references declare them; the conditionals and logic
  are `True`'s and `False`'s own methods, [core-library.md](core-library.md),
  "`True` and `False` are quick methods"), `Number` with `Integer` and
  `Float`, `String`, `Symbol` and `UndefinedObject` (§5.1.3 names
  `nil`'s class so); the platform and its members, `PlatformFactory.ns`'s classes,
  are values too ([core-library.md](core-library.md), "The platform").
  `Number` is empty; it is the
  place §3.1.1 names, and where the numeric tower goes when there is one
  ([core-library.md](core-library.md), "Gap: an integer has no `/`").
  `Array` and `Block` stay directly under `Object`,
  since their instances are not values, and so does `Behavior`, with `Class`
  and `Metaclass` below it ("Class sides and metaclasses" below). The VM mirror's class,
  which `Kernel.ns` does not declare, sits below `Value`, so the mirror is a
  value ("A value object's four conditions" below). The clause is read by name
  at bootstrap, never run, and the class object's `superclass` link is set to
  match its application's.
- **A program reaches a builtin class through `Object`'s accessors** (§3.2):
  `Kernel.ns`'s `Object` declares one public method per nameable builtin,
  `public String = ( ^outer Kernel String )`, and a bare `String` is
  `self String` by §5.7, so a class that declares or inherits a member of
  that name reaches its own ([core-library.md](core-library.md), "The
  predefined classes are accessors on `Object`").
- **`Top` is not nameable.** It is the one class of `Kernel.ns` declared
  private, so §6.3.1 keeps it out of every other module's reach; `Object`
  declares no accessor for it among §3.2's accessors for the predefined
  classes, so a bare `Top` is `self Top` and ends in `doesNotUnderstand:`
  (`goldens/errors/top_is_not_a_name.ns`); and the image decoder refuses its tag wherever an artifact names a class, as it
  refuses the mirror's ([image-format.md](image-format.md), "Verification").
  A program can reach it only as `Object`'s superclass, which is what §3.2
  allows and nothing more.
- **A builtin class's enclosing object is the kernel instance** — the one
  instance of `Kernel.ns`'s root class, instantiated with the VM mirror — and
  its `declaring_class` is the kernel class, as for any nested class. That is
  what makes a kernel method's lexical send of `vmMirror` reach the kernel's
  slot. Each of the kernel instance's hidden nested-class slots holds the
  builtin class itself (`MixinApplication::nested_slot`), so the nested-class
  accessor `Array` inside kernel code answers the builtin `Array` rather than
  a second class object of the same mixin.
- **Its factory is `new`, with no arguments.** A kernel class declares none,
  so its mixin has §6's default, and the arity check applies: a host's
  `Interpreter::instantiate` of a builtin class with factory arguments is
  refused. `Array new` builds its own payload
  (`instantiate::decide_factory`), and the VM mirror's class is refused
  outright — there is exactly one mirror.
- **A user subclass of a builtin inherits its methods but not its payload.**
  `class MyArray = Array new` answers `Array`'s kernel methods, whose leaves
  decline on an instance that carries no array, and each method's failure
  block reaches `Object`'s ([core-library.md](core-library.md), "The shape of
  a kernel method").

## Instantiation

A superclass clause `[prefix] receiver [message]` — `Object new`, or `balance: b`
in `= Object new (...)` — has its two parts evaluated at **different times**
(§6.1, §6.3):

- The **receiver** part alone (`Object`) is the class determined by the
  inheritance clause, evaluated once when the class declaration is evaluated, by
  the same name resolution as any other identifier. Class names are not a
  separate namespace. Its value fixes the actual superclass directly — there is
  no "evaluate `Object new`, allocate an instance, take its class" step.
- The trailing **message** part (`new`) is deferred and evaluated fresh on every
  instantiation, in the context of that instantiation's own instance-initializer
  activation, so the subclass's own factory parameters are in scope when it runs.

Sending the factory message to a class object then does this:

1. **One** instance `o` is allocated — distinct identity, every slot declared by
   the class and its superclasses initially nil. There is no separate instance
   per inheritance level; `o` is threaded through every step below.
2. An instance-initializer activation `i` is created, with the factory message's
   arguments.
3. In the context of `i`, the superclass clause's deferred message is evaluated,
   and the result is sent to the superclass's own instance initializer, invoked
   on the same `o`. This recurses up the chain.
4. Once the chain above has run, this class's own slot clauses execute on `o`, in
   declaration order.
5. This class's own initializer expressions execute on `o`.
6. `i` returns `o` to the factory method, which returns it to the caller.

Step 3 is why a class body has no access to the factory's actual parameters
except through slots explicitly extracted in the initializer: only `i` sees them.

**Each level runs in its own activation, with its own arguments.** Milestone 1
walked the superclass chain upward and ran every level's slots and initializer
expressions in the *subclass's* activation with the subclass's factory arguments,
which §6.2 does not permit.

### A value object's four conditions

§3.1.1 makes an object a value object when "it is deeply immutable ... and its
class inherits from class `Value`", and its second bullet spells that out as
four conditions: all its slots "are immutable, have been initialized and
contain value objects"; "its enclosing objects (3.3) are all value objects";
and "its class inherits from class `Value` and does not define an identity
method (`==`)". All four are enforced strictly (designed with the user on
2026-09-23). Two are about an instance's slots and are decided as the
instance is built, the one place every instance of every class comes from,
whether its class was loaded from source, materialized as a nested class or
rebuilt from an image. Of the other two, the identity method is a property of
the *application* and is decided once, where `Mutator::apply_mixin` builds
it — before any class object exists, which is what covers a module's root
class along with every nested one — and the enclosing objects are fixed per
*class object*, decided once where one is created. **Every one of them is
refused where an instance would come into being**, and so is a fifth: a
module root that breaks the root rule ("Mixin application", at its end),
which decides what a module that declares itself a value may write. Every
refusal is an ordinary Newspeak error named under the primary factory of the
class being instantiated, defaulted to `new`, raised at run time — a class
nothing instantiates never fails, as a method nothing runs never does — and
every text lives in `interpreter/instantiate.rs`, so both lowerings raise the
same errors.

- **Step 1 refuses a mutable slot anywhere in the chain.** A class that
  inherits `Value` and has one could never have a value object for an
  instance, so there is no instance to make and nothing is allocated.
  `Mutator::apply_mixin` answers both questions once per application —
  whether the chain reaches `Value` (`inherits_value`) and the first mutable
  slot of the whole chain, whatever the level (`first_mutable_slot`) — and
  folds the refusal into `instantiation_refusal` only when the chain inherits
  `Value`, because the same mixin applied over a superclass that is no value
  declares an ordinary mutable slot and is perfectly legal. The slot is
  looked for above the `Value` level too, not only below it: a `<:` clause
  puts `Value`'s level below an arbitrary class, so in `P → [Value] → Foo` a
  `| counter ::= 0 |` of `Foo`'s is state of every `P`, and `P new` would
  otherwise answer a "value" with a mutable slot
  (`goldens/kernel/a_mutable_slot_above_the_value_level_is_refused.ns`).
- **Step 4 refuses a declared slot that is not a value.** *Deeply* immutable
  means "all objects reachable from it are also deeply immutable", so
  `InitializeSlot` refuses to store anything but a value in an instance of a
  value class: an immediate, a `String`, a `Symbol`, an
  instance whose class inherits `Value`, or a class object that is a value —
  one whose enclosing object is ("Class sides and metaclasses" below;
  `instantiate::is_a_value`). An `Array` stored there would make the
  whole value mutable through it, and two copies of the value would stop being
  equal the moment one array was written. The decision and its text live in
  `interpreter/instantiate.rs`; `vm.rs` only calls it.

  The rule is about the slots a class **declares**. The hidden nested-class
  cache slot ("The nested-class accessor" above) is written by the accessor,
  not by `InitializeSlot`, so it never meets the check — and need not: the
  class object it holds has the value as its enclosing object and so is a
  value itself, and it is the implementation of a member §6 defines rather
  than state of the value. A value class may declare a
  nested class without ceasing to be one.

**Both of the class-level conditions are decided cold and refused only where
an instance is made**, but at two different moments, since one is a property
of the application and the other of the class object. SOMns decides both at
class creation
(`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`), and
this runtime did too until §2.3's own example ruled it out for the
enclosing-object condition (decided with the user on 2026-09-24): with
`GrandChildOfOuter`'s slot `Inner ::= String`, "`g Nested` returns a subclass
of `String`" — a value class whose enclosing object `g` is mutable, which the
specification creates and never instantiates
(`goldens/spec/spec_2_3_superclass_of_a_nested_class.ns`, which passes as
transcribed). §3.1.1 defines when an *object* is a value; what must never
exist is an instance that passes for a value and is not one. The texts are
"cannot instantiate `C`: a value class may not define ==, and `D` defines it"
and "cannot instantiate `C`: a value class's enclosing objects must be
values, and its enclosing object, …, is not one" (or "`L`'s enclosing object"
for a level above `C`).

- **No identity method** is decided once per *application*, cold, when
  `Mutator::apply_mixin` builds it — the same place `first_mutable_slot` is
  decided, and folded into the same field, `instantiation_refusal`
  (`InstantiationRefusal::IdentityMethod`), right after it: root clause
  defect, then the superclass's own refusal (inherited unchanged, exactly as
  the mutable-slot refusal is), then `Behavior`'s refusal of every class
  object ("Class sides and metaclasses" below), then the mutable slot when
  the chain inherits `Value`, then the identity method. `apply_mixin` looks for the first level
  below `Value` that declares an instance-side `==` while the chain inherits
  `Value`, excluding `Value`'s own mixin. Only levels **below** `Value`
  count: an `==` above it, `Object`'s included, is overridden by `Value>>==`,
  so the class does not define it, and `Value>>==` itself is the declaration
  §3.1.1's next sentence allows. A level a `<:` clause adds below `Value`
  counts like any other
  (`goldens/kernel/an_identity_method_from_an_applied_mixin_is_refused.ns`).
  Deciding it on the application rather than the class object is what covers
  a module's root along with every nested class: `Mutator::load_root_class`
  builds no class object through `nested::after_clause`, but it still calls
  `apply_mixin` — the one place every application in the system is built,
  root, nested or decoded alike — so a value module whose root class defines
  `==` is refused exactly as a nested one would be
  (`goldens/kernel/a_value_module_defining_identity_is_refused.ns`), and
  `decide_instantiation` refuses it from `instantiation_refusal`, the same
  field the root clause defect and the mutable slot are refused from.
- **Enclosing objects are values** is decided once per *class object*, cold,
  by `interpreter/value_class.rs`'s `check_value_class`, which
  `interpreter/nested.rs`'s `after_clause` calls after `apply_mixin` answers
  the application, and only for an application that inherits `Value`; the
  verdict is recorded on the class object (`ClassData::value_class_refusal`,
  a `ValueClassRefusal`), and `decide_instantiation` refuses an instance of a
  class that carries one, beside the refusals it makes from
  `instantiation_refusal`. A class that does not inherit `Value` pays one
  flag test at creation, and a value class one more test per instantiation.
  The class's own enclosing object is checked first, then the walk climbs
  the new class object's superclass chain. At each level: a class that inherits `Value` and was created as a
  class of its own — by `after_clause`, as a builtin or as a module root, not
  a level a `<:` clause added — stops the walk **only if it carries no
  recorded refusal of its own**, because it was checked the same way when it
  was created and everything above it holds by induction. If it does carry
  one, the walk does not trust it: a refused class still exists, since the
  refusal is recorded rather than raised, so the new class inherits the
  fault instead — a `NonValueEnclosingObject` with the `described` text taken
  from that level's own verdict, naming the level that verdict names, or the
  level the walk stopped at when it names none
  (`goldens/kernel/a_value_subclass_of_a_refused_value_class_is_refused.ns`).
  So a fault inherited over several levels still names the level whose
  enclosing object is at fault, not the last class that passed it on
  (`goldens/kernel/a_value_two_levels_below_a_refused_value_class_names_the_faulty_level.ns`).
  Any other level — one that does not inherit `Value`, or a `<:` level, which
  is never created through `after_clause` — has its enclosing object checked
  with `instantiate.rs`'s `is_a_value`, the predicate the slot check uses,
  and the walk continues. Checking every level and not only the class's own
  enclosing object matters because an inherited method reaches its own
  level's enclosing objects through `outer`: in `class P = Foo new <: Value`,
  `P` inherits `Value`, `Foo` does not, and a method of `Foo` whose enclosing
  object is mutable would make `P`'s instances reach mutable state
  (`goldens/kernel/a_value_over_a_class_with_a_mutable_enclosing_object_is_refused.ns`).
  Requiring `Foo` itself to inherit `Value` would be stricter than §3.1.1: a
  slotless `Foo` in a value module meets every condition, and
  `Foo new <: Value` is how a value variant of it is made
  (`goldens/kernel/a_value_over_a_slotless_class_in_a_value_module_is_accepted.ns`).
  A module's root class has `nil` for its enclosing object and a chain of
  builtins with at most `<: Value`, so its enclosing objects hold by
  construction, and `load_root_class` does not call the check.

  A decoded class object starts with no verdict — `ClassData::new` defaults
  `value_class_refusal` to `None`, the wire carries none, and a legitimate
  snapshot can hold a class that was created carrying one — so
  `image::decode::rebuild_applications` recomputes it for every decoded
  class whose application inherits `Value`, which has an enclosing object and
  which is neither a metaclass nor an applied level, using the same
  `check_value_class`. It does this in a pass of its own, after every decoded
  class's real application is installed, not folded into the pass that
  installs them: the walk reads an *enclosing* class's application too
  (`is_a_value`, `Heap::describe`), which is a different axis than the
  superclass chain the install pass orders, so an enclosing class can still
  hold its placeholder application at the point a superclass chain reaches
  it. The recompute pass itself still runs superclass first — reusing the
  install pass's own order — since the walk reads an ancestor's
  `value_class_refusal`, which has to already be the recomputed one. A
  crafted artifact gets the verdict its decoded structure implies, like any
  other decoded class; a ready-made *instance* decoded from an image is not
  re-checked ("What is still unchecked" in [core-library.md](core-library.md)).

**`Kernel` and the VM mirror are values, as in SOMns** (decided with the user
on 2026-09-23). `Kernel.ns`'s module is declared
`class Kernel vmMirror: theMirror = Object <: Value (…)`, the header
`prior-art/somns/core-lib/Kernel.ns` writes, and its one slot is immutable.
The mirror's class sits below `Value` (`interpreter/kernel.rs`,
`superclass_of_kind`), so the mirror is a value by the ordinary rule — no
slots, no `==` of its own, its class inherits `Value` — rather than by an
exception in `check_value_slot`; SOMns asserts the same
(`vm/ObjectSystem.java`, `assert vmMirror.isValue()`). Every capability
guarantee of the mirror is unchanged by it: nothing names it, the encoder
refuses it, `decide_factory` refuses to instantiate its class, and
`SendMirror` has no wire tag. With the kernel instance a value, every
builtin value class passes the enclosing-object condition by construction;
the builtin classes are built by the bootstrap rather than by the
nested-class accessor, so `interpreter/kernel.rs`'s
`the_kernel_and_the_mirror_are_values` pins that the kernel instance and the
mirror are values ([core-library.md](core-library.md), "The bootstrap:
placeholders first, kernel second").

**A value class needs a value module.** Since a value class's enclosing
object must be a value, a value class nested in an ordinary module is refused
on instantiation
(`goldens/kernel/a_value_class_in_an_ordinary_module_is_refused.ns`). Every
golden and inline test module that declares a value class therefore declares
itself a value, `= Object <: Value`, and keeps its recorded outcome — the
rule decided with the user on 2026-09-23 for the two goldens declaring
`class Text = String new`, extended to every such module. A value's slot may
not hold the platform or the transcript, so such a module reads the
transcript where it uses it, or keeps its former module methods in a nested
`Script` class that is no value. `goldens/nesting/nested_override.ns`, which
instantiates a class whose superclass clause reads an overridden slot, starts
that slot at `List` rather than `String`, so the class it
instantiates is no value class in an ordinary module; its recorded outcome
did not change.

## Simultaneous slots

A `|| ... ||` slot clause (§6.3.2), in a class header or as a code body's
temporaries, initializes its slots simultaneously through pull futures, so
well-founded mutual recursion between them resolves, where a `| ... |`
clause initializes them in declaration order. The code is
`compiler/simultaneous.rs` (the desugaring both lowerings share),
`interpreter/future.rs` (what a future is, and forcing it), `Kernel.ns`'s
private `Future`, and four instructions, `MakeFuture`, `ResolveFuture`,
`LoadOwnSlot` and `InitializeSlotProvisionally` (wire tags 36 to 39).
Designed with the user on 2026-09-28; every decision below was taken with
them unless it says otherwise
([the plan](../superpowers/plans/2026-09-28-1152-ns-next-simultaneous-slots.md)).

**What §6.3.2 says, and all it says.** A simultaneous slot declaration with
a right-hand side *e* "initializes the slot to the value of *p*
`computing:` *e*, where *p* is the class `` Past`Future ``", a future "that
will compute the expression *e* on demand"; "All these futures are resolved
once the last slot declaration in the simultaneous slot clause has been
executed", and `` Past`Future `` "implements a pipelined promise so that any
well founded mutual recursion between simultaneous slots will resolve
properly". The specification says nothing else about futures — neither what
a slot holds once its future is resolved, nor whether a future that escaped
is observable. What it does say, in §5.5's commentary on `doesNotUnderstand:`,
is the intent that a forwarding proxy be indistinguishable from its target:
"Otherwise it would be possible to distinguish between an object and a
proxy."

**What the prior art does.** Both canonical compilers
(`NewspeakCompilation.ns`, `Newspeak2SqueakCompilation.ns`, and
`Newspeak2PrimordialSoupCompilation.ns` in `prior-art/primordialsoup/`) wrap
each initializer of a simultaneous clause in `Future computing: [e]` and do
nothing at the clause's end: the slot keeps its future forever, forced on
demand. Squeak's `Future` is an ordinary `Object` with a forwarding
`doesNotUnderstand:`, so `=`, `==` and `class` answer the future's own.
Primordial Soup's is a `Proxy`, which overrides every public member of
`Object` as protected so that §5.5's `lookupPublic` stops there and every
message reaches `doesNotUnderstand:`; its gaps are an asymmetric `==`
(`future == value` is true, `value == future` false), the identity
primitives, and a primitive that receives a future as an argument. Both
signal "Divergent evaluation" for a future forced while it is being
computed. Both mark this the *pull* future, as distinct from the *push*
future of actors and `<-:` (`KernelForSqueak.ns`, `Future`'s comment): it is
synchronous, computed on the forcing thread in the forcing send, with no
event loop. SOMns parses `|| ... ||` and does not support it.

**The four rules.**

1. **Once the clause is done, no slot of it holds a future.** Its end forces
   each slot's future in declaration order and writes the value back, so
   every read afterwards is a direct slot read. This goes beyond the prior
   art, which keeps the future, and is how ns-next reads "resolved". A slot
   a sibling's initializer assigned holds whatever it was given, resolved
   the same way if that is a future; a slot with no initializer stays
   **nil**.
2. **Every future a clause made is resolved when the clause is done** —
   including one that escaped during it, into an array, a closure or another
   object, since it is the same object the slot held. What remains after
   the clause is resolved future objects wherever they escaped to.
3. **A resolved future is observable by no program.** Messages are
   forwarded, as Primordial Soup's `Proxy` does; `==` and the identity hash,
   the equality leaves and a leaf's arguments see through it ("Where Rust
   sees a future" below). The collector replaces a reference to one with its
   value in every object field it traces, so a resolved future lives at most
   until the next collection; removing escaped futures at the clause's end
   instead would be `becomeForward:`, a walk of the whole heap on every
   instantiation of such a class, for nothing a program can observe.
4. **A value never holds a future** (§3.1.1's deep immutability). A future
   about to be stored in a slot of a value is forced first, and its value is
   stored and checked — so `|| p = Pair first: q second: 1. q = 3. ||`
   builds a `Pair` holding `3`, where refusing the future would fail a
   well-founded program. The clause's own slots in a value class are the one
   exception, and only for the first write ("The lowering" below).

A future still being computed is observable, as §6.3.2 intends: touching one
forces it, and forcing one that is already being computed — ill-founded
recursion — raises an ordinary, catchable `Error`, "divergent evaluation of
a simultaneous slot's initializer". **A clause abandoned by an exception** —
an initializer raises and a handler outside the instantiation takes it —
leaves the futures it had not resolved as they are, wherever they escaped
to: the next message forces one and runs its initializer then, and one
whose initializer raised raises again for the same cause. A forcing whose
activation an unwind pops, or which the run loop refuses to push, leaves its
future unresolved again rather than stuck resolving (`Mutator::abandon`).
Nothing answers wrongly, and it costs nothing; marking them broken would put
an `ifCurtailed:` around every simultaneous clause.

**The future.** `Kernel.ns`'s private `Future`, which no program can name
(as `Activation`), follows Primordial Soup's `Proxy`: every public member of
`Object` — `class`, `==`, `=`, `hash`, `printString`, the nil protocol — is
overridden as protected, and `doesNotUnderstand:` forces the future through
the mirror row `futureValue:ifFail:` (`builtins/future.rs`) and forwards the
message to the value with `Message>>sentTo:`. A member added to `Object`
without its override fails
`future_overrides_every_public_member_of_object_as_protected`. Its state is
two private slots, `computation` — its block until it is resolved, then its
value — and `state`, 0 unresolved, 1 resolving, 2 resolved, which only
`interpreter/future.rs` reads and writes. It reads them as untrusted, since
crafted code can store into them once it holds a future: any other state,
an unresolved future without a closure, or a resolved one whose value is a
future is `FutureState::Malformed`, and forcing it is an ordinary error
([image-format.md](image-format.md), "Futures in a graph"). Forcing is a trampoline step that
activates the future's block under `Continuation::Future`, never a call
back into Newspeak; once the block returns, `vm::finish` hands its value to
`Mutator::future_computed`, which records it and either delivers it to the
send that forced the future (`AfterForcing::Deliver`) or runs the
instruction that met the future again (`AfterForcing::Reexecute`;
[execution.md](execution.md), "Running an instruction again after forcing
a future"). A block that answers another unresolved future forces that one
next, the waiting forcing chained behind it, so both resolve to its value —
§6.3.2's pipelined promise, as Primordial Soup's `resolve` loops. Nothing
bounds that chain's length, so every walk of it loops rather than recursing
once per link. No message a program sends forces a future, since a public
`resolve` would tell an escaped future from its value, and there is no
class-side factory: the runtime makes every instance (`Mutator::new_future`,
which `MakeFuture` calls). It carries builtin class tag 30 because the
collector, the leaves and the image must recognize it. The first future a
heap makes sets `Mutator::futures_made`, which is never cleared, and the
two checks that would otherwise run for every program — the leaves' search
among their operands and the collector's replacement — stay behind that
flag.

**The lowering.** The parser keeps the `simultaneous` flag on each slot
declaration, of a class's slot clause and of a code body's temporaries
alike. One desugaring before both lowerings (`compiler/simultaneous.rs`;
[compiler.md](compiler.md), "The simultaneous slot clause"), so the
reference lowering stays an oracle for it, turns

```
|| a = e1. b ::= e2. c. ||
```

into the writes `a := ⟨future of [e1]⟩` and `b := ⟨future of [e2]⟩` in
declaration order, then `a := ⟨resolved a⟩` and `b := ⟨resolved b⟩`: the
futures are made by the instruction `MakeFuture` and resolved by
`ResolveFuture`, which forces its operand when it is a future and answers
anything else unchanged — instructions, not sends, so no program can shadow
them. A class's clause end reads each slot back with `LoadOwnSlot`, which
addresses it as `InitializeSlot` does, not through a getter a subclass
could override. The resolver scopes each initializer as its future's
block's body (`resolve_simultaneous_initializer`), and the blocks capture
siblings that the clause's end writes again, so the capture analysis puts
them in their scope's indirection vector by itself
([execution.md](execution.md), "Closure conversion"). In a value class, the
first write — the future — is `InitializeSlotProvisionally`, which skips
§3.1.1's slot check; the write at the clause's end is an ordinary
`InitializeSlot` and is checked. A value whose clause is done therefore
meets §3.1.1 exactly as strictly as one whose clause is sequential.

**Where Rust sees a future.** Nothing on a program's paths looks for one
until the program has made one.

- *As a receiver* it needs nothing: every send reaches its
  `doesNotUnderstand:`. The inlined conditionals fall back to a real send
  on a receiver that is no Boolean, and `IntegerBinary` on anything but two
  small integers; the compiler inlines neither `==` nor `isNil`.
- *As a leaf's argument or subject*, a future is looked for only on the
  failure path of the three ways a leaf runs — `mirror::execute_sent` (a
  send to the mirror), `send_inlined` (`SendMirror`) and `execute_method` (a
  primitive method): the leaf declined or raised — and only once
  `futures_made` is set: `future::after_a_failed_leaf`, inlined into each
  caller, is that flag test and nothing more, and the search itself is the
  cold `see_through_a_failed_leaf` behind it (`send_inlined` tests the flag
  itself and goes to the cold `mirror::see_through_a_failed_row`);
  `builtins::execute` itself is the plain dispatch to the leaf. A resolved one is replaced by its value and
  the leaf runs again, in Rust. An unresolved one cannot be forced in Rust,
  since nothing re-enters the loop: the answer is a step that activates the
  future's block under `Continuation::Future` with `AfterForcing::Reexecute`,
  which records that activation's value and runs the caller's instruction
  again instead of the next one (`vm::reexecute`); the second time the
  future is resolved. This rests on an invariant written on
  `builtins::execute` and checked leaf by leaf: **a leaf that declines or
  raises has changed nothing** ([dispatch.md](dispatch.md), "Primitives").
- *The leaves that answer "no" rather than declining* — identity and the
  equality leaves of `Object`, `Value`, `Integer`, `Float`, `String`
  and `Symbol`, and `inheritsMixinOf:` — decline instead when that answer
  is "no" and an operand is a future (`future::declines_for_a_future`,
  behind the same flag), so the failure path does the same;
  `Primitive::answers_false_to_a_stranger` names them, matching every leaf
  by name so that a new one does not compile until someone decides.
  `Behavior`'s equality leaf, `behavior:sharesApplicationWith:ifFail:`, is
  not among them: it declines for an operand that is no class object, a
  future among them, so the failure path sees through one without it. `==`
  is symmetric, unlike Primordial Soup's.
- *The leaves that keep or render an operand* and would take a future for a
  value of its own — `do:`'s block, `whileTrue:`'s and `whileFalse:`'s body,
  and the selector and description an unhandled exception's termination
  reports — decline for a future, so the failure path sees through it. Every
  other leaf that answers for any operand stores or forwards it
  (`at:put:`'s element, `value:`'s arguments, `sentTo:`'s receiver,
  `returnValue:`'s value), and a future there is the value it stands for to
  whoever reads it later. On a leaf's failure path, though, every future
  operand is seen through: an unresolved one the leaf would only have stored
  is forced first, and one still being computed raises the
  divergent-evaluation error in place of the leaf's own error.
- *`InitializeSlot` into a value* looks for a future only on its refusal
  path (`check_value_slot`, then `future::value_for_a_value_slot`): it
  stores and checks a resolved one's value, and forces an unresolved one
  through the same `Reexecute` (`vm::force_and_reexecute`).
- *A host* that passes a future straight to `Interpreter::send` has no
  instruction to run again: a resolved one is seen through in Rust as
  anywhere else, and an unresolved one is forced, after which the run ends
  with an error saying so (`vm::reexecute`,
  `interpreter::future::host_passed_a_future`); a host holds a future only if
  a program handed it one during a clause.

**The collector** replaces a reference to a resolved future with its value in
object fields — slots, array elements, closure copies, indirection vectors —
and never in registers or host roots: a forcing activation that is returning
has the future as `self`, and would otherwise read its own slots from the
value ([memory.md](memory.md), "A resolved future is traced as its value").

**The image** never holds a resolved future: the encoder writes its value in
its place. An unresolved one, which outlives its clause only when the clause
was abandoned, is an ordinary record of its class with its closure, and the
decoder refuses the class's tag wherever a graph names a class, as it
refuses `Activation`'s ([image-format.md](image-format.md), "Futures in a
graph").

**What pins it.** Under `goldens/spec/`:
`spec_6_3_2_simultaneous_slots_resolve_mutual_recursion` and
`spec_6_3_2_simultaneous_temporaries_resolve_mutual_recursion`, a class's
clause and a code body's; `spec_6_3_2_a_data_cycle_resolves`
(`|| first = {second}. second = {first}. ||`, the elements identical to the
arrays after the clause) and `spec_6_3_2_a_data_cycle_survives_an_image`;
`spec_6_3_2_a_captured_sibling_is_read_after_the_clause`;
`spec_6_3_2_a_future_is_an_argument_like_its_value`,
`spec_6_3_2_a_future_is_a_block_argument_like_its_value` and
`spec_6_3_2_a_future_is_equal_and_identical_to_its_value`, from either side,
and `spec_6_3_2_every_equality_sees_a_future_as_its_value` (`Float`'s and a
value's `=`, and `on:do:`'s handler class);
`spec_6_3_2_ill_founded_recursion_is_an_error`;
`spec_6_3_2_a_value_class_initializes_simultaneously`, accepted, and refused
for a slot that is no value; `spec_6_3_2_a_future_passed_into_a_value_is_forced`;
`spec_6_3_2_a_mutable_slot_a_sibling_assigns`;
`spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch`;
`spec_6_3_2_an_unhandled_error_is_described_by_a_futures_value`; and
`spec_5_1_8_a_code_body_declares_simultaneous_temporaries`. Every golden
runs under both lowerings and through the image round trip. Unit tests
cover the parser's flag (`ns-syntax`'s declaration tests); forcing, chains,
abandonment, the host's two cases and the collector's replacement
(`tests/future.rs`); the four instructions driven by hand (`tests/vm.rs`);
the image's two cases and its refusals (`tests/image.rs`); and the wire tags
(`tests/opcode_tags.rs`). Three of the four `goldens/bench/` fixtures are
level; `bench_megamorphic.ns` measured about 4% slower, a layout effect of
the dispatch arm the four instructions add, still open
([execution.md](execution.md), "What simultaneous slots cost").

## Mixin application

§6.2 lets a superclass clause apply one or more mixins to the superclass it
names:

```newspeak
class C factory: x = S ms <: M1 m1 <: M2 m2 ( … ) ( … )
class Tagged = Object new <: Marker withTag: 'x'.
```

Each `<:` clause is §6.1's `inheritanceClause`, the production the superclass
clause itself is — a receiver naming a class, with an optional message
defaulting to `new` — and `<:` is one lexer token. With at least one clause the
whole body may be replaced by a `.`, which declares nothing: the applied mixins
supply everything. `ClassDeclaration::mixin_applications` carries the clauses
in written order, and the declaration's `Mixin::mixin_applications` one
`MixinApplicationClause` each — its selector, whether its message has
arguments, and that message's compiled body. Type arguments on a clause's
receiver (`<: Collection[E]`) are parsed and dropped, as they are on a
superclass clause ([frontend.md](frontend.md), "What is parsed and dropped").

**Each clause adds one class level between the declaration and its
superclass**, lowest first, so the last clause's mixin is the nearest to `C`.
§6.2 gives the level four properties, and the level has each one:

- **Its mixin is the named class's mixin** — that class's own declaration, not
  its superclass chain, which is what §6.2's "a class *C* whose mixin *M*"
  applies.
- **Its superclass is the level below**: `S` for the first clause, the
  previous level for each later one.
- **Its enclosing object is the named class's** — "The enclosing object of *I*
  is the enclosing object of *C*" — **and so is its declaring class.** A method
  of the applied mixin was written where the named class is declared, so
  `outer` inside it reaches the mixin's own enclosing objects, never the
  applying declaration's (`goldens/mixin/mixin_outer_reaches_the_mixins_enclosing_object.ns`).
- **Its primary factory selector is the clause's selector** — "Class *I* has a
  primary factory method (6) with the same selector as *m*" — which must name
  the named class's primary factory: a clause sending anything else is §6.1's
  run-time error, raised when `C` is instantiated
  (`goldens/mixin/mixin_clause_naming_the_wrong_factory_fails.ns`). A clause
  whose receiver does not evaluate to a class object is §6.1's other run-time
  error, raised where the levels are built.

**The levels are built in one place, `interpreter/nested.rs`'s
`build_applied_levels`**, called by `Mutator::load_root_class` for a module's
root class and by the nested-class accessor's `after_clause` for a nested one.
A nested declaration's `<:` receivers are evaluated in its superclass clause's
own activation, which answers `{superclass. mixin…}` when there are clauses
(`nested::superclass_clause_expression`), so one activation fixes every class
the levels are built from, with the bindings and the cyclic-creation marker the
superclass clause already had. Each level is an ordinary class object
(`Mutator::new_applied_level`) whose `ClassData::applied_clause` names the
applying declaration and the clause's position in it — boxed, so a class
object grows by one word — rather than carrying a copy, because two clauses of
one declaration can be identical and the image names a level's clause by row
and position.

**So lookup, `super`, `outer`, the dispatch ids and the inline caches need
nothing new.** A level is a class object in the class-object chain and an
application in the application chain like any other, so the three lookups walk
through it ([dispatch.md](dispatch.md), "The three lookup algorithms"), a
`super` send from `C` reaches the applied mixin and one from the applied mixin
reaches the level below, and the dispatch ids and inline caches, which are
per application, see one more application with an id of its own. The
application cache is keyed as for any class, by (mixin,
superclass application), so two enclosing instances whose `C`s apply the same
mixin over the same superclass share each level's `MixinApplication`, as §6.2's
"they all share their code and structural definition with the mixin" requires.

**Initialization order follows §6.2's text**, generalized to a chain, and
matches SOMns (`MixinBuilder.assembleInitializationMethod`): `C`'s instance
initializer evaluates `ms` and runs `S`'s initializer — and everything above
`S` — with its arguments; then evaluates `m1` and runs `M1`'s initializer with
its arguments; and so on to `mn` and `Mn`; and then runs `C`'s own slots and
initializer expressions. Every clause message is compiled in `C`'s factory
scope and evaluated with `C`'s factory arguments (§6.3 makes the factory's
parameters visible in the superclass clause), and each `mk` is evaluated just
before `Mk`'s initializer runs, not on the way up. The walk climbs as for any
class, checks each clause's selector against the named class's factory as it
climbs, defers each `<:` message (`Instantiation::deferred`) and evaluates it
on the descent; a frame running one has kind
`FrameKind::MixinApplicationMessage`, naming the level its clause added.
Because a `<:` clause's selector is checked against the named class's primary
factory on the way up, a misnamed clause fails before `ms` and the
superclass initializers run -- the policy the superclass clause already
follows, whose selector is checked before its arguments are evaluated. An
applied mixin's own superclass clause is never evaluated: only its mixin is
applied (`goldens/mixin/mixin_initializer_order.ns`), and so are none of its
own `<:` clauses
(`goldens/mixin/mixin_applied_mixins_own_clauses_are_not_applied.ns`).

**A declaration without `<:` takes the path it took before**, plus one test
per climbed level, per descent and per nested-class creation; the `<:`
machinery lives in cold, out-of-line helpers. What that costs is measured in
[execution.md](execution.md), "What mixin application cost". The image
carries the clauses on each row and the levels as class records
([image-format.md](image-format.md), "Sections"). The goldens are under
`goldens/mixin/`, with §6.2's own example in
`goldens/spec/spec_6_2_a_mixin_application_derives_a_class.ns`.

**At a module's root, `<: Value` is allowed as a rule: it is how a module
declares itself a value.** `class M … = Object <: Value (…)` makes the
module's class inherit `Value`, which is §3.1.1's condition for its instances
to be values, and it is the form §3.1.1's other conditions need — a value
class's enclosing objects must be values, and the outermost one is the module
— and the one SOMns's `Kernel.ns` writes. It is a sanctioned form, not a side
effect of how a root clause is resolved
(`goldens/mixin/mixin_module_declares_itself_a_value.ns`).

**The root rule: only `Object`, `Value` or `Object <: Value`.** A module's
root class may omit its superclass clause (which is `Object`), name `Object`
or `Value` unprefixed, or write `Object <: Value`; its message part is
unaffected and checked at instantiation like any other. Everything else is a
root defect: any prefix, any other name — the other builtins, `Top` and a
name nothing declares included — and any other `<:` clause, `Value <: Value`
and several clauses among them. A root class has no enclosing object to
evaluate a receiver in: §6.1 evaluates its clause "as if it was a top level
expression", where it "will almost certainly fail", so the only classes a
root can reach are the ones the rule names. The corpus has such roots —
`Newspeak2SqueakCompilation`'s `= NewspeakCompilation usingPlatform:
platform`, `UISessionManager`'s `= NewspeakObject new` — which the
Squeak-hosted implementation accepts; none is on `corpus/runnable.toml`.

**Every root defect is deferred to instantiation** (decided with the user on
2026-09-23): one rule, one path, and the deferred one, because a module whose
root is wrong still loads, so its class exists to be inspected and fixed in a
live system before anything instantiates it. The resolver binds none of a
root clause's receivers (`resolver/mod.rs`), so `= Missing new` is no resolve
error; compiling stays total, since a receiver with no binding lowers to the
deferred `UnresolvedIdentifier`, and a root's clause receiver is never run. Nested declarations keep their resolve errors. The
rule is syntactic, so `Mutator::build_root_mixin` — which builds every
module's root mixin, `Kernel.ns`'s included — records the defect on the root
mixin (`instantiate::root_clause_defect`, `Mixin::root_clause_defect`,
naming the clause as written and the reason); `load_root_class` builds a
defective root over `Object` with no level, so `load_module` stays total; and
`apply_mixin` folds the defect into `instantiation_refusal`, inherited by
every class built on the root, the one cold `Option` `decide_instantiation`
tests for both refusals it makes before allocating — the mutable slot and the
root defect. `run` and `load` always instantiate the root, so every program
with a defective root meets the error at start, under both lowerings with the
same text: "cannot instantiate `M`: a module's root class may inherit only
Object or Value, or write Object <: Value, and its superclass clause names
`…`" (or "its mixin application clause *n* names `…`", or "it applies Value to
Value"). **The encoder refuses a module with a root defect**, at `build` and
`snapshot`, with "image encoding cannot serialize `M`:" and the same reason:
such a root can never be instantiated, so an artifact of it could only fail,
and refusing it needs no wire field and no tag. The decoder's root-section
reader refuses what the encoder cannot write
([image-format.md](image-format.md), "Verification"), so a crafted artifact
cannot bring the rule back. `goldens/errors/root_*` pin one defect each.

## Class sides and metaclasses

§3.2 reifies the class hierarchy: "Each class is an instance of a unique
metaclass. All metaclasses are instances of class Metaclass, and are direct
subclasses of Class. Metaclasses may not be subclassed. […] The metaclass
hierarchy in Newspeak is deliberately kept very flat", and §6 gives the
construction — a class is an instance of "a metaclass that is the application
of the metaclass mixin defined by the class' class side to the class Class".
ns-next builds it literally (designed with the user on 2026-09-25, "Decisions"
below): every class object's class is its own metaclass, every metaclass's
class is `Metaclass`, and `Behavior`, `Class` and `Metaclass` are declared
classes of `Kernel.ns`.

**The hierarchy** is Primordial Soup's:

```
Object
  Behavior      what a class and a metaclass share: =, ==, hash
    Class       the superclass of every metaclass: name, printString
    Metaclass   the class of every metaclass: name, printString, =, ==
```

The specification names neither `Behavior` nor `Metaclass`'s superclass.
`Behavior` is there so that what only a real class has — a name of its own —
sits on `Class` and a metaclass does not inherit it: §3.2's "Metaclasses may
not be subclassed". SOMns's `Metaclass = Class` would make every metaclass
carry `Class`'s protocol and override it away again. None of the three
inherits `Value`: whether a class object is a value is decided per object
(below), and under the strict enforcement of §3.1.1 a subclass of `Value`
refuses every instance that is not one. They carry builtin class tags 20, 21
and 22 (`BuiltinClass::Behavior`, `Class`, `Metaclass`), the next free
numbers ([image-format.md](image-format.md)).

**Where a class object's state lives.** In `ClassData`: its application,
enclosing object, declaring class and superclass, and for a metaclass
`this_class`, the link back to its class. The three kernel classes carry
behaviour only, as SOMns's do (`Class = Object ()`, `Metaclass = Class ()()`
in `prior-art/somns/core-lib/Kernel.ns`, the state in the Java `SClass`) and
as `KernelForJS.ns`'s do. Primordial Soup instead makes a class object an
ordinary instance whose first six slots the VM knows by position (`Behavior`
in `KernelForPrimordialSoup.ns`); taking that shape would move lookup, the
caches, the collector and the image format onto slot indices and read every
lookup step out of a `Value` slot, for no conformance gain: §3.2 fixes the
relations — instance-of, subclass-of, flatness — and says nothing about how a
class object is stored.

**The metaclass mixin is the class side *together with the primary factory
method*** (§6.3: "The class side, together with the primary factory method,
defines the mixin for the metaclass"). Class-side declarations are kept and
dispatched as metaclass behaviour rather than parsed and dropped. A class
`Foo`'s metaclass is the application of that mixin **to `Class`**
(`Mutator::attach_metaclass`, `interpreter/metaclass.rs`): an ordinary
application whose superclass application is `Class`'s, a class object whose
superclass link is `Class` and whose enclosing object and declaring class are
`Foo`'s, since §6.3 writes a class side inside the same declaration as its
instance side. Its application is the memoized one for (class side, `Class`),
so every class object of one declaration — one per enclosing instance — gets
a metaclass of one application, for the reason the application cache is
keyed by the superclass's application (§6.2): a metaclass per application of
its own would send every `X new` site megamorphic.

**A metaclass is flat: a class side is not inherited.** A metaclass's
superclass is always `Class`, never the metaclass of `Foo`'s superclass, so
the ordinary lookup from a class object walks its own class side, then
`Class`, `Behavior` and `Object`, and a subclass answers a class-side message
only if its own class side declares it
(`goldens/class-side/class_side_is_not_inherited_by_a_subclass_with_none_of_its_own.ns`).
§5.10 makes a `super` send's S the superclass of the activation's current
class, which for a class-side method is `Class`: a class-side
`super printString` answers `Class>>printString`, the class's name
(`goldens/class-side/class_side_super_reaches_class.ns`), and a class-side
`super new` does not instantiate, because nothing above the metaclass
declares a factory.

**Every class object's class is its metaclass, and every metaclass's class is
`Metaclass`.** `attach_metaclass` sets both headers: the class object becomes
an instance of its metaclass, and the metaclass an instance of `Metaclass`.
`Metaclass` is a kernel class with a metaclass of its own, whose class is
`Metaclass` again, which closes the loop without a regress
(`goldens/class-side/the_class_of_a_class_is_its_metaclass.ns`:
`Foo class class == Foo class class class class`). There is no separate
metaclass field: `Heap::class_metaclass` and `Mutator::metaclass_of` read the
header, and answer `None` for a metaclass, whose class is `Metaclass`. A
metaclass never gets a metaclass of its own, which `attach_metaclass`
debug-asserts.

**Eager, not lazy.** `new_class_object` builds the metaclass together with
the class object, so the header is right from the start. The alternative — a
header that points somewhere provisional until something asks for the
metaclass — would make every direct reader of an object's class a source of
silent wrong answers for class objects. SOMns builds the two together, as
Squeak's Newspeak does. The cost is one more class object per class object —
for a nested class, once per enclosing instance, on the first access.

**The bootstrap** creates class objects — every builtin, the kernel's `Value`
level and the `Kernel` module's own class — before `Class` and `Metaclass`
are filled. They carry the builtin `Object` as a provisional header `class`
until the end of the kernel's fill, where
`Mutator::give_existing_class_objects_metaclasses` gives each its metaclass,
`Object`'s and `Metaclass`'s included, and sets `metaclasses_ready`, from
which point `new_class_object` builds one with every class object. When
`wire_kernel_in` gives a builtin its lexical context, its metaclass gets the
same ([core-library.md](core-library.md), "The bootstrap: placeholders first,
kernel second"). After the bootstrap the invariant holds without exception;
`interpreter/metaclass.rs`'s `every_builtin_has_a_metaclass_in_its_declaration_context`
pins that every builtin has a metaclass linked back to it and sharing its
enclosing object.

`Object::class` is not an `Option<ClassReference>`, because the builtin
`Object` class exists from the first moment of the bootstrap and serves as
every provisional header. The image decoder relies on that the same way: it
allocates every record as a placeholder pointing at the builtin `Object`
before it reads the record that says what the class really is, and decoding
a class record overwrites that with the record's metaclass, as it overwrites
every other placeholder field.

**The primary factory is an entry of the metaclass mixin's method table**,
under the declared factory's selector (`new` for a declaration that writes
none, §6's unnumbered introduction, before §6.1 begins), found by the
ordinary lookups like any other member — as both references build it
(Primordial Soup's `astForPrimaryFactory:`, SOMns's
`MixinBuilder.assemblePrimaryFactoryMethod`). It carries no body: what it
does is instantiate the class object the message was sent to, which
`interpreter::resolve::act` runs through `instantiate::decide_factory`. A
flat metaclass puts a factory entry on the receiver's own metaclass and
nowhere else, and `resolve::resolved_for` honours a factory only when the
lookup found it on the receiver's own class, so no special case sits ahead of
or behind the lookup ([dispatch.md](dispatch.md), "Resolve, then act").
`primary_factory_selector` survives only where §6.1 still needs it, in the
superclass clause's check.

The entry is derived, never stored twice: `instantiate::build_class_side`
inserts it as a class side is built from source, `image::mixins::decode`
derives it for a row another row names as its class side, and
`Mutator::metaclass_mixin` puts it in the otherwise empty mixin a class that
declares no class side gets — memoized per class mixin
(`Mutator.metaclass_mixins`), so every class object of one declaration shares
it. One definition of what it is (`mixin::factory_entry`), so the three
cannot disagree about which selector instantiates a class. Nothing about it
reaches the image: like a slot's accessors (§6.3.2), it is rebuilt on the way
out from the factory the mixin table already carries.

**Dispatch is uniform.** A class object's dispatch id is its class's
instances id — its metaclass's — the same rule as for every other object;
there is no class-side id and no class-side lookup chain, and the inline
caches guard on the header id as they do for an instance
([dispatch.md](dispatch.md), "The dispatch id"). A class side's own
`doesNotUnderstand:` handles every send its class object does not understand,
since §5.5's handler is "defined for the class of the receiver" and that
class is the metaclass
(`goldens/class-side/a_class_side_does_not_understand_handles_its_class_objects_sends.ns`).

**The protocol is minimal, as Primordial Soup's is.** Reflection is a
mirror's job, which the reflection step builds; a class does not answer
`superclass`, `selectors` or its methods at the base level, so holding a class
grants no reflective capability. `Class>>name` answers the mixin's name
through the leaf `className:ifFail:` and `Class>>printString` answers `name`;
`Metaclass>>name` answers `thisClass name, ' class'`, reading `this_class`
through the leaf `metaclassThisClass:ifFail:`, and `Metaclass>>printString`
answers `name` (`builtins/behavior.rs`;
`goldens/class-side/a_class_and_its_metaclass_print_their_names.ns`:
`Foo`, `Foo class`, `Metaclass`, `Metaclass class`). `Object` has **no
accessor** for `Behavior`, `Class` or `Metaclass`: none of the three
references declares one, §3.2's accessors are for the classes of the built-in
literals and the array factories, and the mirror code that will need the
names lives in the kernel or the platform, where it names them directly.
`interpreter/kernel.rs`'s accessor-set test pins the set.

**Instantiation is refused.** The three are reachable anyway —
`3 class class` is a metaclass and its `class` is `Metaclass` — so refusing to
instantiate them is needed whatever is nameable. `Mutator::apply_mixin` folds
`InstantiationRefusal::ClassObject` into the `instantiation_refusal` of
`Behavior`'s application — recognized by the mixin the kernel phase records
as `Mutator.behavior_mixin` before `Behavior` is filled — and every
application below it inherits the refusal from its superclass, so
`decide_instantiation` refuses `Class`, `Metaclass`, every metaclass and any
subclass of them a program manages to declare with an ordinary Newspeak error
under its factory selector: "cannot instantiate `C`: a class object is made
only by evaluating its declaration" (`interpreter/instantiate.rs`;
`goldens/class-side/a_class_object_cannot_be_made_by_instantiation.ns`, which
sends `3 class class class new`). A metaclass itself declares no factory, so
`Foo class new` is not understood
(`goldens/class-side/a_metaclass_does_not_understand_new.ns`).

**A class object is a value exactly when its enclosing object is**, as SOMns
decides it (`SClass.isValue`; `instantiate::class_object_is_a_value`). A
module definition has no enclosing object and is always one, which is what
§6.4 requires ("Module definitions are value objects"). A nested class of a
value is one too: its only state is its enclosing object (§6.1) and, where
its superclass clause depends on the enclosing instance, its superclass, so
§3.1.1's condition on enclosing objects is what decides it. A builtin class is
one, since its enclosing object is the `Kernel` instance, a value. A
metaclass shares its class's enclosing object and so its verdict. Two
consequences:

- **Step 4 of the value conditions** ("A value object's four conditions",
  above) accepts a class object that is a value in a slot of a value's
  instance, and still refuses one that is not
  (`goldens/kernel/a_value_modules_nested_class_is_a_value.ns`,
  `goldens/kernel/a_values_slot_refuses_a_class_that_is_no_value.ns`).
- **A value class object has no identity.** Its `=` and `hash` are derived
  from its state: two class objects are equal when they share their mixin
  application, their enclosing objects are `=` and their superclass class
  objects are `=`. The application alone is not enough, because it is keyed
  by the superclass's *application*, so two classes can share one while their
  superclass class objects differ (§6.1: "the superclass of a nested class
  might be distinct in different instances, as it might be a parameter to a
  module"). This is an inference, not a sentence of the specification: §2.3's
  `Outer new Inner = Outer new Inner` is false for an ordinary `Outer`,
  because the class's only state is its enclosing object (§6.1) and two
  ordinary instances are not equal; §3.1.1 makes two equal values the same
  value ("no other object is equal to it"), and §6.1 has the same receiver
  answer the same class. So the equality of a nested class follows the
  equality of its enclosing object: false for two `Outer new` of an ordinary
  `Outer`, true for two of a value `Outer` whose slots are equal, false again
  for `(Outer x: 1) Inner = (Outer x: 2) Inner`. Without it two values equal
  by §3.1.1 could be told apart through their nested class. SOMns compares
  values by identity (`EqualsPrim.doValues`), and so would answer false; it
  does not derive a value's equality from its state at all, which ns-next
  does, so it is no guide here.

  The comparison is `builtins::value`'s walk, extended by class objects as a
  node kind rather than a second walk beside it. The classes of every pair of
  nodes are compared as a pushed pair of their own rather than by identity,
  since `a Nested new = b Nested new` must hold for `a = b` although the two
  classes are distinct objects. Two identical class objects are equal at
  once; two value class objects of one application push the pair of their
  enclosing objects and the pair of their superclass class objects; two
  metaclasses push the pair of their `this_class`, since a metaclass's
  enclosing object is its class's and its superclass always `Class`;
  anything else is unequal. The walk's node budget and its handling of
  cycles cover class objects with that. Where the walk declines — an
  enclosing value whose class brings its own `=` — Newspeak compares, as
  `Value>>=`'s fallback does, since a leaf may not send: `Behavior`'s
  fallback asks whether the two share an application and reads their
  enclosing objects and superclasses, through three mirror rows, and
  `Metaclass`'s compares the two classes.

  `Behavior` declares `=` and `==` and `hash`; `Metaclass` declares `=` and
  `==` again, with its own fallback. **`==` is the same forwarder as `=`**,
  to the same leaf (`value:sameAs:ifFail:`), rather than a method that sends
  `=`: it answers what `=` answers, as §3.1.1 asks of a value's identity
  method, and costs no activation, so the common `self class == other class`
  stays one leaf. For a class object that is no value the leaf answers
  identity.

  **The hash of a class object that is a value is derived from its mixin's
  name alone** — a metaclass's from its display name, its class's name
  followed by ` class`. Equal class objects share an application, hence a
  mixin, hence a name, so it is a valid hash, and unlike one that folds in
  the enclosing object's hash it never unfolds a builtin class's enclosing
  `Kernel` instance, whose `vmMirror` slot the walk declines. The name, not
  the application's dispatch id, because a name survives a copy and an image
  round trip and a dispatch id is minted afresh on load. Classes of one mixin
  under different enclosing objects collide, which is coarse, not wrong.
  Every other class object keeps its identity hash (`Behavior>>hash`'s
  fallback). None of the references decides either question: SOMns compares
  values by identity (`EqualsPrim.doValues`) and hashes a class by identity,
  Newspeak on Squeak's `Value` overrides neither `=` nor `hash`, and
  Primordial Soup has no `Value` — each shares values between actors in one
  address space rather than copying them, which §3.1.1 allows ("copied freely
  … or shared") and which makes identity unobservable as a difference.

**The image format.** A metaclass stores nothing twice: everything about it
— its mixin, its application, its enclosing object and declaring class — is
derived from its class, as the factory entry is. A reference to one can sit
anywhere in the graph — a class-side activation's `current_class`, any slot
holding `Foo class` — and every class record's own header names one. How it
is written depends on its class:

- **The metaclass of a class record is a record of its own**, object body
  `OBJECT_METACLASS`, whose body is only the class reference back to its
  class record. It gets a placeholder in the decoder's first pass like any
  record and is referred to by an ordinary id, so a slot can name it before
  its class record is decoded, its identity hash travels in its record, and
  the amplification bound ([memory.md](memory.md)) does not move: it is a
  record of at least the minimum size, one placeholder like any other. The
  alternative — a "metaclass of record N" reference with no record — would
  have had to patch every place such a reference can sit once the class
  records are decoded, a mechanism across the whole decoder to keep safe
  against a crafted image.
- **The metaclass of a prebuilt class** — a builtin, the module's root class
  or one of the root's levels, none of which is a record — is named by tag,
  `CLASS_METACLASS` followed by that class's own tag, and its identity hash
  travels in the closing hash table (`HASHED_METACLASS`, followed by the
  class tag), as the builtin classes' own do.

The decoder applies each metaclass record over `Class` as `attach_metaclass`
does (`rebuild_metaclasses`) and holds each class record and its metaclass
record to each other both ways (`pair_metaclasses`, `MetaclassPairing`), which
keeps a metaclass's identity across a round trip: the metaclass a slot holds
is the one its class answers. The three new builtin tags are refused wherever
a program *names* a class — a superclass, a `<:` clause's mixin — as `Top`'s
is, since no source module can name them; as values in the graph they are
accepted, since `3 class class class` reaches `Metaclass`
([image-format.md](image-format.md), "What is *not* serialized",
"Verification").

## Source retention and provenance

Every class and method keeps enough information to recover its exact source text
and where it came from — needed by the debugger and by any export back to `.ns`,
and cheap to carry.

- Each loaded module keeps its full original source as one `Rc<str>`, attached to
  the module rather than duplicated per node.
- Every declaration already carries a `Span`, so source text for any node is
  recovered on demand as `span.text(&module_source)`, with no per-node copy.
- `SourceOrigin` is attached once per module. Today it is always
  `SourceOrigin::File(PathBuf)`. It is an enum specifically so a later milestone
  can add a variant for code entered live in the debugger without changing the
  type its call sites depend on.

## Arena allocation and handles

`Rc<RefCell<Object>>` was the Milestone 1 model. It is gone: an object is an
entry in one `Heap`'s arena and an `ObjectReference` is a 32-bit index into it, private to
`heap.rs`. The representation, what it measured, the root discipline with
its lifetime brand, and the mark-compact collector are designed in
[memory.md](memory.md), "Checked roots and a collector" onward. The layout
that replaces the arena is "Planned: the object layout" below.

Two consequences belong here rather than there, because they are about this
document's subject.

**An activation is an object too, when something needs it to be.** A running
activation is an entry on the `Heap`'s activation stack, and it becomes a
first-class `Frame` — an entry in the same `Heap`'s frame arena, addressed by a
`FrameReference` with the same privacy as an `ObjectReference` — only when a block captures it.
So an activation nothing captures costs no allocation and leaves nothing behind,
and one that is captured is stored the way an object is. See
[execution.md](execution.md), "Activations".

## Planned: the road to a high-performance VM

Would change: the object representation becomes the one the reference
engines with a JIT share -- Cog's Spur, Strongtalk and HotSpot, with
Primordial Soup beside them -- adapted where Newspeak's nesting requires it,
and the JIT tiers follow on it. Decided with the user on 2026-09-29; the canonical order is [OPEN-WORK.md](../OPEN-WORK.md), "The road
to a high-performance VM". Three `Planned:` sections below and four in
[memory.md](memory.md) describe its representation steps. The road, each
step landing green on its own:

1. **The memory layer** -- a crate of its own that reserves one contiguous
   address range and commits it on demand ([memory.md](memory.md),
   "The memory layer").
2. **An eight-byte value**, on today's arena, with `LargeInteger`
   ("Planned: an eight-byte value" below; [core-library.md](core-library.md),
   "Planned: large integers").
   - **After road step 2, the compact instruction encoding**, designed
     against the new value ([execution.md](execution.md), "Planned: the
     compact instruction encoding").
3. **The object space** -- one header word, the class word, the fields
   inline ("Planned: the object layout" below; [memory.md](memory.md),
   "Planned: the object space").
4. **Generations** ([memory.md](memory.md), "Planned: generations").
5. **`become:` through forwarders** ("Planned: `become:` through forwarders"
   below).
6. **Inline allocation in the interpreter** ([memory.md](memory.md),
   "Planned: inline allocation").
7. **The execution model for compiled code** ([execution.md](execution.md),
   "Planned: JIT tiers").
   - **Between road steps 7 and 8, real Newspeak bodies for `whileTrue:`,
     `whileFalse:` and `do:`** ([core-library.md](core-library.md), "Planned:
     real bodies for control flow and `do:`, before the baseline JIT").
8. **A baseline JIT**, and
9. **an optimizing JIT** ([execution.md](execution.md), "Planned: JIT tiers").

## Planned: an eight-byte value

Would change: `Value` becomes one 64-bit word whose low three bits are its
tag, instead of today's 16-byte enum of a tag and an 8-byte payload (`Nil`,
`Boolean`, `SmallInteger(i64)`, `Float(f64)`, `Object`). Road step 2,
built on today's arena before the object space, since
a 32-bit arena index already fits the word.

| Low bits | Meaning |
|---|---|
| `000` | an object handle: today the arena index shifted left by three, so its low bits are `000`, from road step 3 the object's byte offset in the space, so that the space's base plus the word is its address |
| `001` | a `SmallInteger` of 61 bits |
| `010` | a constant -- `nil`, `true`, `false`, with room for more |
| `100` | a `SmallFloat64` |
| `110` | free |
| `011`, `101`, `111` | never produced; kept so that bit 0 alone tells a `SmallInteger` |

The tags are Spur's (`smallIntegerTag` 1, `smallFloatTag` 4, three tag bits,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:2779` for the three tag bits, `:2836` for `smallIntegerTag`, `:2784` for `smallFloatTag`): Spur
writes a `SmallInteger` as `(value << 3) + 1` (`integerObjectOf`,
`:41159`) and tests it by bit 0 alone. Spur's `Character` tag, `010`, is
taken for the constants: ns-next has no characters, and
`nil`, `true` and `false` stay immediates as they are today rather than
becoming heap singletons as in Spur and Soup. A `SmallFloat64` is Spur's
encoding (`floatObjectOf:`, `smallFloatValueOf:`): an eight-bit subset of
the exponent -- IEEE exponents 896 with a non-zero mantissa through 1151,
about 2^-127 to just under 2^129, plus ±0.0 (`isSmallFloatValueBits`, `:41232`) --
with the sign rotated down beside the tag. Every other float -- NaN,
the infinities, denormals and magnitudes outside that range -- is a boxed
`Float` object. Soup boxes every float, which costs an allocation per
floating-point result; Spur's split keeps the common range immediate. A
finite float in the `SmallFloat64` range is always immediate, so every float
has one representation, as every integer does. Both are §5.1.1's binary
float; the `FixedPointNumber` that section names for non-integral literals
(the recorded deviation, [core-library.md](core-library.md)) would be a class
of the numeric tower, not a tag.

**Why.** LLVM copies today's `Value` as a tag byte and two overlapping
quadwords, so every `Value` that passes through a memory temporary -- a
result answered by an out-of-line function, a joined `Result`,
`LoadLiteral`'s `make_value` -- is a store-forwarding stall, 28% of
`vm::run`'s samples on the send path ([execution.md](execution.md), "Where
the 3% goes"); fixing it site by site had to inline the whole path to the
register write and nearly doubled `vm::run` ("A send's value delivered where it
is decided: tried and halted", there). A value read and written as one word cannot
stall that way, on Windows x64 it comes back in `RAX`, and registers, fields,
arrays, argument buffers and copied closure values take half the memory, with
`Step`, `Result<Step, Signal>` and the continuations shrinking alongside.
Small-integer arithmetic works on the tagged words with one overflow test, as
Cog's and Soup's does, which is what a JIT's tiers build on.

**Integers become unbounded.** A `SmallInteger` that overflows its 61 bits
becomes a `LargeInteger`, and a result that fits again becomes a
`SmallInteger` ([core-library.md](core-library.md), "Planned: large
integers"). That is what §5.1.1's "integers are not limited in size"
requires, so the deviation row for `i64` integers in
[README.md](README.md) goes, and the goldens that record an overflow error
change to record the promoted result, each established against the
specification before it is updated. A `LargeInteger` and a boxed `Float`
are value objects (§3.1.1), so their `==` and their hash come from their
state, as every value's do, never from the header's identity hash.

**Literals.** An integer literal beyond 61 bits keeps its digits in the
Rust-side literal data of its code (`Literal`, which holds no handle), and
`LoadLiteral` allocates the `LargeInteger` each time it runs -- a value
needs no identity, so a fresh one per execution is correct. The compact
instruction encoding takes this as an input ([execution.md](execution.md),
"Planned: the compact instruction encoding"). A float literal outside the
`SmallFloat64` range does the same with a boxed `Float`. Since every
integer literal is then representable, `Literal::MalformedInteger` and its
wire tag 8 retire ([compiler.md](compiler.md);
[image-format.md](image-format.md), "Planned: the image under the new
representation").

**The debug build keeps its exact check.** Today a debug handle carries its
heap's id and its move epoch in full, and every accessor compares both
([memory.md](memory.md), "The brand, checked by the compiler rather than by convention"). The Rust-side `Value` keeps doing so:
in a debug build it is wider than a word, the word plus both numbers, exactly
as now, and in a release build it is the bare word. What has to be exactly
eight bytes in both builds is a word stored in a heap object -- a field, an
element, a frame's register -- so that the layout is the same in both. Such
a word cannot go stale the way a Rust-side handle can while every
collection walks every object: the collector reads each object by its format
and rewrites every field, so it has no field to overlook the way it could
overlook one a Rust type forgot to trace, and a word read out of the heap is
given the current heap id and epoch. So that no stale value can be laundered
that way, every store accessor checks, from this step on, that the Rust-side
value it stores carries the current heap id and epoch before it becomes a
word. Under generations (road step 4) a
scavenge walks only the young objects and the remembered ones, so an old
object whose store missed the barrier would keep a stale young word; that is
what `NS_NEXT_STRESS_SCAVENGE`, with the vacated survivor space and the vacated eden region made no-access, catches
([memory.md](memory.md), "Planned: generations"). The check
therefore stays exact where the mistake it catches can be made, with the
brand checking the rest at compile time.

**What changes.** Every pattern on `Value::…` -- about 1,050 sites, some 550
in the crates' sources and the rest in their tests -- goes through
constructors and accessors; `heap.rs`'s handle code, `trace.rs`, the image
codec and the leaves change with it. The image gains wire tags for a
`LargeInteger`, each the next free number; a float, boxed or not, stays an
`f64` on the wire and carries no record, since it has no identity, and the
decoder decides whether it is immediate ([image-format.md](image-format.md),
"Planned: the image under the new representation").

## Planned: the object layout

Would change: an object stops being a slot in an index arena (56 bytes in a
release build) with
its variable part in a separate Rust allocation, and becomes a run of words
in one space -- a header word, a class word, then its fields -- addressed by
its byte offset. Road step 3; the space, its
allocation and its collector are [memory.md](memory.md), "Planned: the
object space". The header word plus class word (option A), frames as heap
objects and strings in two widths were decided with the user on 2026-09-29;
the rest is derived.

```
[header word][class word][field 1][field 2] ...     8-byte aligned, at least 16 bytes
```

**The header word**, after Spur's (`headerForSlots:format:classIndex:`) and
HotSpot's compact object headers (JEP 450, a product feature from JDK 25 as
JEP 519), which fit a class reference, the identity hash, the GC age and
the lock bits into one 64-bit word:

| Bits | Holds |
|---|---|
| 0-21 | the application id -- the inline caches' guard ([dispatch.md](dispatch.md), "The dispatch id") |
| 22-26 | the format |
| 27-30 | the age, for the generations (road step 4) |
| 31 | the remembered bit (road step 4) |
| 32 | the negative bit of a `LargeInteger`, as Primordial Soup's `kNegativeBit` (`prior-art/primordialsoup/vm/object.h:48`) |
| 33 | the mark bit |
| 34-55 | the identity hash, drawn lazily from the counter; 0 means none yet |
| 56-63 | the number of words after the header -- the class word and the fields; 255 means the real count is in an overflow word *before* the header |

The word count sits in the top byte, as Spur's does, so the overflow word
before a large object's header carries 255 in its top byte and the real
count in its low 56 bits, and a linear walk tells it from a header.

**The identity hash is 22 bits wide**, drawn from a counter that wraps
within them, as Strongtalk's wraps within 20 (`markOop.cpp`), and skips 0.
Hashes are therefore not unique once the counter has wrapped, which a hash
need not be; the decoder advances the counter past the largest hash an image
holds and refuses a hash of 2^22 or more, where it refuses `u32::MAX` today
([image-format.md](image-format.md), "Identity hashes are preserved
verbatim").

**An application has two ids.** Its *guard id*, 22 bits wide, is the
inline caches' guard: it stands in object headers, in inline- and
global-cache entries and in recorded guards (see "What a slot access
checks" below), and nowhere else. Running out of guard ids keeps today's
rule: an application without one runs uncached ([dispatch.md](dispatch.md),
"The dispatch id"). Its *full-width index* into the Rust-side table of
`MixinApplication`s is what a class object names and what every identity
comparison of applications uses -- the application cache's key and value,
`behavior:sharesApplicationWith:` (`Kernel.ns:116`), which class-object
equality uses, and the global cache's `Lexical` key, which, as today, never
records a `NONE` guard ([dispatch.md](dispatch.md), "3b — The caches").
So an application whose guard id is `NONE` stays reachable and distinct
from every other. Each id has its own reuse rule, and the collector can tell
both, so reusing either is safe: a guard id is reclaimed once no object
header, no inline- or global-cache entry and no recorded guard names it; a
full-width index once no class object, no application-cache key or value
and no other application's superclass link names it. A mixin id is
reclaimed once the `Mixin` itself is dropped -- its table entry freed, no
declaration, enclosing mixin or code holding it -- and no application, no
application-cache key or value and no class object's applied-clause field
names it. A class's change of application restamps every instance's header
by a heap walk, as `Heap::set_class_application` walks the arena today
(`crates/ns-runtime/src/heap.rs:2046-2062`), before the old guard id can
become reclaimable.

**The class word** is an ordinary value naming the object's class object.
An immediate has no header; its class and its guard come from a small table
indexed by its tag, and for the constant tag by the constant, since `nil`,
`true` and `false` share a tag but have three classes and three guards --
as Spur's class table's first page holds the tag classes.

**Why a class word, and not Spur's one word.** Spur's header holds a class
*index*, which is both the object's class and the inline caches' tag. That
does not carry over: a nested class has a class object per enclosing
instance, all of them sharing one mixin application, so a guard on the class
would make every send site that sees instances from two enclosing objects
polymorphic -- the mistake "Three layers, not one" and `CLAUDE.md` warn
about. The guard has to be the application id, and the class object still
has to be one load away, because `class` and every `outer` send -- every
implicit send to an enclosing scope -- read it, and a JIT cannot fold it to a
constant when it differs per instance. A one-word alternative was weighed: a
22-bit application id and a 32-bit class index sharing the header, the
identity hash moved out and appended when a hashed object first moves, as
Project Lilliput explores for four-byte headers beyond the eight-byte ones
JEP 519 ships. It saves 8 bytes an object and costs
`class` and `outer` a second load, through the class table, plus the most new
mechanism. Speed on a hot path went first; the accessors in `heap.rs` keep
either layout open.

**Formats**, after Spur's five-bit field (`formatOf:`):

| Format | Objects |
|---|---|
| fixed pointer fields | ordinary instances, class objects |
| indexable pointers | `Array` |
| frame | a reified frame: fixed fields, then its registers |
| closure | fixed fields, then its copied values |
| byte-indexed | a one-byte string or symbol |
| 32-bit-indexed | a wide string or symbol |
| 64-bit-indexed | a `LargeInteger`'s magnitude (its sign in the header's negative bit), a boxed `Float` |
| forwarded | a forwarder -- `become:`'s ("Planned: `become:` through forwarders") and the scavenger's for a young object it has copied -- whose class word holds the new location |
| free | a gap the collector leaves, walkable like an object |

The byte and 32-bit formats carry the number of unused units at the end in
their low bits, as Spur's do, so an exact length needs no length field. A
forwarder needs no field of its own: the class word, which every object has
-- a header and a class word are the 16-byte minimum -- takes the target, as
Spur's forwarders take their first slot.

**What becomes a heap object.** A class object keeps in fixed fields
everything of today's `ClassData` that holds per class object: its enclosing
object, declaring class, superclass, for a metaclass the class it describes
(`this_class`), its builtin tag (`ClassData.builtin`, which `act` re-reads
on every send), its value-class refusal (a `SmallInteger` index into a Rust-side table of
refusals, or `nil`, since today's `ValueClassRefusal` holds a Rust `String`
and a `Symbol`, `types.rs:105-112`), for a level a `<:` clause added the
clause that added it (`ClassData.applied_clause`: the applying declaration's
mixin id, the clause index and the named builtin tag packed into one
`SmallInteger`, or `nil`, since the `Rc<Mixin>` it holds today cannot live
in the space), and its application as a `SmallInteger` full-width index into
a Rust-side table of `MixinApplication`s, separate from the 22-bit guard id
in instances' headers; the behaviour stays Rust. Eight fields, so a class
object is 80 bytes with its header and class word. The application cache is keyed by the mixin's id and
the superclass application's full-width index rather than by `Rc`
addresses, which lets it become weak ([memory.md](memory.md), "Weak
references"). A closure keeps its code id, captured frame and feedback base
in fixed fields and its copied values inline; a code id indexes a Rust-side
table of compiled code that is append-only per heap, its entries never freed
or reused, since compiled code is shared by mixins and reclaiming it belongs
with live editing. Each entry also records the code's owning `MixinMethod`,
where it has one, so a method frame's code id names its method too -- the
declaration the encoder writes today (`image/encode.rs`) and `frame_body`
reads. A reified frame keeps its
sender, home, receiver, current class, program counter, code id, kind, the
kind's operand -- the closure of a block frame, or the class of an
initializer, superclass-clause, superclass-message or mixin-application-message
frame, a handle the trace and the barrier see --
`uncontinuable`, and the index of the activation it is married to, if any
(today `FrameSlot`'s `live`), in fixed fields and its registers inline, and the kernel-private
`Activation` objects the exception machinery holds become these frame
objects themselves; the running activations stay on the activation stack
until road step 7. A frame object is allocated at `reify` with its code's
full register count, whatever the code; whether the divorce copies the
registers into it stays decided per activation from the block's
`outer_reach`, as today. So no frame object ever grows, and suspension, the
debugger and edit-and-continue find room for the registers in every reified
frame. Edit-and-continue adding locals divorces the affected activation
into a newly allocated, larger frame object -- allocated old and born
remembered by the rule in [memory.md](memory.md), "Planned: generations" --
the VM rewriting every reference to the old frame by a walk of every root the
collector traces (`trace.rs`) and every heap object, since `become:` never
forwards a frame; execution continues from the divorced frame, which needs a divorced
frame put back on the stack, the piece suspend and resume need too
([execution.md](execution.md), "How this meets the liveness requirements"). The cost is a few more words per
reified frame, and `bench_block.ns`, which reifies a frame per iteration, is
the workload that shows it.
While a frame is married its registers live on the stack and the trace skips
the object's inline copies. `FrameReference` stays, now the type of a handle
to a frame object, so an `ActivationReference` is still never stored. The
frame arena, its second forwarding table and every separate register vector
go.

**Strings have two widths.** One byte per code point while every code point
fits Latin-1, 32 bits per code point otherwise; either way the elements are
code points, so `size` and `at:` are one access. §5.1.5 makes a string's
elements "integers representing the encoding of the string in Unicode normal
form C" and leaves the representation to the implementation. This is Spur's
`ByteString` and `WideString`; Java's compact strings (JEP 254) and V8 do the
same with a UTF-16 wide form, and Python (PEP 393) adds a middle width of two
bytes, which can be added here later behind the same interface. Primordial
Soup stores UTF-8 bytes and answers a byte from `at:`
(`String_at`, `vm/primitives.cc`), which would change what `size` answers
for any text beyond ASCII. Source files, the transcript and files stay UTF-8,
converted at the boundary. A grapheme cluster -- one emoji with a skin-tone
modifier is two code points -- is never an element; counting or iterating
them is a library protocol above the string, since the segmentation rules
change with every Unicode version and allow no constant-time indexing.
`Symbol` becomes a subclass of `String`, as in every reference kernel, and
`Mutator::new_symbol` stays the only way to make one: instantiating `Symbol`
is refused, and `String new` answers an empty one-byte string, each pinned by
a golden. `=` stays class-tight both ways, as today: `'foo' = #foo` and
`#foo = 'foo'` both answer `false`, so `Symbol` overrides `=` and `hash` and
`String>>=` answers `false` for a `Symbol` argument, pinned by a golden. Today nothing refuses it --
`decide_factory` (`interpreter/instantiate.rs`) special-cases only the VM
mirror's class and `Array`, so `Symbol new` answers an instance with no text,
and `String new` answers an instance that understands no `size`
([OPEN-WORK.md](../OPEN-WORK.md)).

**Handles.** An `ObjectReference` becomes the object's byte offset, 64 bits
wide, so the space is bounded by its reservation rather than by a handle's
width. HotSpot's compressed oops store a reference in 32 bits and so cap a
heap at 32 GB; that saving exists because a Java field of reference type
holds nothing else. A Newspeak field holds any value -- an integer, a float,
an object -- so it is a full 64-bit word whatever the handle's width, as in
Spur. Accessors may lend a string's units or an object's fields as a slice
instead of copying them out. Every field, element and register accessor
checks its index against the object's own word count, not only against the
space's bounds, so no write through one object -- a crafted image's
by-reference register write included -- can reach the next object's header;
an out-of-range index that a valid program produces raises an ordinary
Newspeak error, never a panic.

**What a slot access checks.** A slot access that is not guarded by the
receiver's own header reads or writes a field at an index decided without
looking at the receiver: the own-slot instructions `InitializeSlot`,
`InitializeSlotProvisionally`, `LoadOwnSlot` and `ReturnSlotIfFilled`, which
address `slot_base + own_index` in `self` with no send (`bytecode.rs`,
`vm.rs`), and a slot accessor acted on through `Chain::Private` or
`Chain::Super` -- a private lexical send, one to an enclosing object
included, resolves on the lexical class to a field index
(`resolve_lexical`, `interpreter/resolve.rs`; `decide_slot_accessor`), and
a super send on the superclass. Each compares the receiver's header guard
with an expected guard, a *recorded guard*: for the own-slot instructions,
the one the activation recorded from the receiver's header when it was
entered -- not the running level's own application, which differs from the
receiver's on every inherited level; for a private or super slot accessor,
the one the cache entry recorded when it was filled. One compare, the hot
path. On a mismatch, or when either guard is `NONE`, a cold path checks,
through the receiver's class word and the full-width index, that the
receiver's application inherits the accessing level: if it does -- as after
an edit that changed only methods and restamped the header -- the access
proceeds; if not, it raises an ordinary Newspeak error. The reference
lowering, which has no cache, always takes the cold check. So after a
`self become:` inside an initializer or a lazy slot's initializer, the rest
of that activation cannot write the old layout into an object of another
shape, and a receiver forwarded to an object of another class never
receives a write meant for the class it replaced. Recorded guards join the
guard id's reuse rule ("An application has two ids" above).

## Planned: `become:` through forwarders

Would change: `become:` exists, as Spur implements it, with lazy
forwarders. Road step 5.

`become:` turns the old object into a forwarder: the forwarded format, the
target in its class word, and a reserved application id -- one of the 22-bit
ids -- that is never any send's guard, so every send whose guard is the
receiver's own header misses its inline cache on a forwarder. A super send
and a private lexical send guard on the running method's or N's application
instead, not on the receiver's header, so a forwarded receiver could hit
there: they check the receiver's header for the forwarded format before
using a hit and take the resolve path when it is forwarded, as Spur's
explicit read barrier in super sends does (`ensureReceiverUnforwarded`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:6637-6643`: "To
maintain the invariant that all receivers are unforwarded we need an
explicit read barrier in the super send cases"). A forwarded receiver is
unforwarded in one shared step at the top of each send entry, before resolve
and act -- `decide_send` (`interpreter/mod.rs`), `decide_lexical_send` and
`decide_super_send` (`interpreter/sends.rs:26`, `:78`), and the caches'
`miss` and their hit check for super and private lexical sends -- so the
receiver `act` gets is the followed one and the two lowerings cannot
diverge; that step cleans up the receiver, the stack and the
registers, as Spur's `handleForwardedSendFaultForTag:` does. The scavenger
and the compaction rewrite every reference they meet to the target, and the
forwarder is garbage afterwards.

**One forwarding rule for both causes.** A reference to a forwarded object is
followed along the whole chain, whatever made each link -- `become:` or the
scavenger -- and the object at its end is copied only if it is young and not
already in the survivor space being filled, as Spur's `scavengeReferentsOf:`
does (`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:43278`: "a
forwarding pointer could be because of become: or scavenging"; the
`isReallyYoungObject:` test at `:43316-43322`). Marking likewise
follows a forwarder to its target.

**Where a forwarder can be met.** Every heap accessor reads the object's
header anyway, for the per-object bounds check; a forwarder's format sends
it down a cold path that follows the forwarder, so every structural read --
a class word read by the value-condition check, the derived equality and
hash walks, the image encoder, `Heap::describe`, a host root -- sees the
target. After a `become:` the VM also follows forwarders eagerly in the
activation stack's registers and receivers, in the host roots and in the
values `Mutator` holds, as Spur's post-become action does. `==` follows
forwarders when its two handles differ, and a leaf that meets one in an
argument fails and is retried after following it -- inside
`interpreter::mirror::send_inlined` and `builtins::execute`, before a
`SendMirror`'s failure block is taken -- as Spur's primitives unforward
their arguments on failure.

**What `become:` refuses.** An immediate has no object to forward, and
`become:` refuses one, as Spur does. It also refuses the objects the VM reads
structurally on its hottest paths -- class objects, whose fields lookup and
`outer` read, closures, whose captured frame the VM reads, and frame
objects, whose sender and home the unwinder reads -- and symbols, whose
canonicity §5.1.6 requires and which the weak symbol table reads -- and
every other value object: strings, large integers, boxed floats and every
instance of a value class. §3.1.1 makes a value immutable, and a `become:`
on one would change, for every holder, what it holds -- a mutation of the
value, even within one heap; with actors it would also reach only the
references in its own heap and leave copies elsewhere unchanged. Spur
refuses immutable objects the same way (`PrimErrNoModification` in
`become:with:twoWay:copyHash:`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:45699`, `:45737`);
SOMns, a Newspeak with values and actors, has no `become:` at all, and
Primordial Soup forwards only in its MirrorBuilder's installation of an edit
(`installAll`, `prior-art/primordialsoup/newspeak/Mirrors.ns:309-326`),
which is where the VM-internal migration below happens too. Refusing values
was decided with the user on 2026-09-29. The same refusals keep "The VM
mirror is a capability" in `CLAUDE.md`: every instance a platform path names
is a value -- `Platform`, `Transcript` and `KernelNamespace` in
`PlatformFactory.ns` and `Collections` in `CollectionsFactory.ns` are all
declared `= Value new` -- and every class a platform path names is a class
object, so a program's `become:` refuses each of them in either direction
without a rule of its own. A program can reach, and `become:`, kernel-made
non-value objects -- a VM error's `Error` or `MessageNotUnderstood` and its
`Message` -- and that is harmless: `become:` never moves contents between
classes, the kernel activations holding them are frame objects and refused,
a kernel method whose receiver has been forwarded to a program object fails
the slot-access check ("What a slot access checks" above) instead of
writing kernel-private state into it, and shipped code reaches the mirror only lexically, through its class
objects' enclosing chain and the roots of `Kernel`, `PlatformFactory` and
`CollectionsFactory`, each declared `= Object <: Value` and so a value, and
the mirror itself is unreachable. Every object on a path from shipped code
to the mirror is refused. So
only instances of non-value classes and arrays are ever forwarded by a
program.

**Everything stays editable.** The refusal is of a program's own `become:`,
not of editing. A class object keeps its identity when its class is edited:
the VM writes its fields -- a new application id when methods or slots
change, a new superclass, a new value refusal -- and every reference to it
stays valid; when the slots change, its instances migrate by allocating the
new shape, copying the fields and forwarding the old instances with
`become:`, which instances admit. A live frame gains locals by being divorced
into a newly allocated, larger frame object, allocated old and born
remembered, with a walk of every root the collector traces (`trace.rs`) and
every heap object by the VM rewriting every reference to the old frame, since `become:` never forwards
a frame, and execution continuing from the divorced frame ("Planned: the
object layout" above).
Closures keep their old code, as in Smalltalk, and new ones get the new code.
Editing a value class migrates its instances the same way, inside the VM,
which no program can observe, since a value has no identity.

Both one-way and two-way `become:`, each with the choice whether the
identity hash moves with the identity (Spur's
`become:with:twoWay:copyHash:`). The eager alternative walks the whole heap
on every `become:`: Primordial Soup's one-way `Heap::BecomeForward`
(`prior-art/primordialsoup/vm/heap.cc:1072`) and Strongtalk's two-way
`oopPrimitives::become`, which also deoptimizes every process first
(`prior-art/strongtalk/vm/prims/oop_prims.cpp:60-77`). It is reached as a
leaf on the VM mirror, which is what the MirrorBuilder in
[liveness-and-reflection.md](liveness-and-reflection.md) needs to add or
remove slots of a class with live instances.

## Decisions

- **Three layers rather than one**, because §6 requires a distinct class object
  per enclosing instance while §6.2 requires behaviour to be shared per mixin.
  A single `ClassData` cannot do both.
- **The application cache is keyed by the superclass's application.** Keying by
  the superclass class object was the obvious choice and is wrong; it is recorded
  here because the failure is silent — correct output, no sharing, every call
  site megamorphic.
- **The hidden nested-class slot is keyed by mixin, not by name.** Keying by name
  makes an override and the declaration it overrides collide in one slot.
- **The accessor is interpreter code, not synthesized bytecode**, so there is one
  copy of it for every lowering to reach.
- **A mixin application's levels are ordinary class objects**, one per `<:`
  clause, so lookup, `super`, `outer`, the dispatch ids and the caches reach
  them with no new machinery; only building them, initializing them and
  carrying them through the image are new ("Mixin application" above).
- **All four of §3.1.1's value conditions are enforced strictly, and each is
  refused where an instance is made** (designed with the user on 2026-09-23;
  refusing at instantiation rather than at class creation decided on
  2026-09-24, because §2.3's own example creates a value class with a mutable
  enclosing object and never instantiates it). The two class conditions are
  still *decided* once each, cold — the identity method per application,
  where `Mutator::apply_mixin` builds it, and the enclosing objects per class
  object, where one is created ("A value object's four conditions" above).
- **A module's root may name only `Object` or `Value`, or write
  `Object <: Value`, and every other root is a defect deferred to
  instantiation** (decided with the user on 2026-09-23), so a module with a
  wrong root still loads and can be fixed live ("Mixin application", "The
  root rule").
- **§3.2's class hierarchy is reified as `Behavior`, `Class` and
  `Metaclass` in `Kernel.ns`** (nine decisions taken with the user on
  2026-09-25, built by
  [its plan](../superpowers/plans/2026-09-25-1331-ns-next-class-behavior-metaclass.md);
  "Class sides and metaclasses" above). A class object is a value exactly
  when its enclosing object is, a metaclass when its class is. A class
  object's state stays in `ClassData`; the kernel classes carry behaviour
  only. Metaclasses are built eagerly with their class, and `ClassData`
  links a metaclass back to its class (`this_class`). The hierarchy is
  `Object` → `Behavior` → {`Class`, `Metaclass`}, none of them below
  `Value`. The protocol is minimal — `name` and `printString` on `Class`
  and `Metaclass` — `Object` declares no accessor for any of the three, and
  instantiating anything below `Behavior` is refused. A value class
  object's equality is derived from its state — its application, its
  enclosing object, its superclass; a metaclass's from its class — by
  extending `builtins::value`'s walk with class objects as a node kind, and
  `Behavior` declares `=`, `==` and `hash`. Its hash is derived from its
  mixin's name alone. In the image, a class record's metaclass is a record
  of its own (`OBJECT_METACLASS`), a prebuilt class's is named by tag
  (`CLASS_METACLASS`, `HASHED_METACLASS`), and the amplification bound does
  not move. Two further decisions came up while building it: `==` on
  `Behavior` and `Metaclass` is the same forwarder as `=`, to the same leaf,
  so it costs no activation; and the hash of a value class object is taken
  from its mixin's name, not from a dispatch id, which is minted afresh on
  load.
- **§6.3.2's simultaneous slots are initialized through pull futures**
  (decided with the user on 2026-09-28,
  [its plan](../superpowers/plans/2026-09-28-1152-ns-next-simultaneous-slots.md);
  "Simultaneous slots" above). A clause's end writes each slot's value back,
  so no slot of a finished clause holds a future; a future that escaped is
  observable by no program once it is resolved — messages are forwarded,
  and identity, equality and every leaf see through it — and the collector
  replaces it with its value in object fields; a value never holds one. The
  future is a kernel-private class after Primordial Soup's `Proxy`, forced
  as a trampoline step, and an instruction or a leaf that meets an
  unresolved one forces it and runs again, which rests on a leaf that
  declines or raises having changed nothing.
- **Identity hashes come from a counter, not from addresses, and are assigned on
  first request**, so identity survives a moving collector and a snapshot, and
  allocation order is unobservable.
- **The value will be one tagged 64-bit word with unbounded integers through
  `LargeInteger`** (planned; decided with the user on 2026-09-29; "Planned: an eight-byte value" above). Spur's tags and the
  immediate floats in Spur's range are derived.
  It lands on today's arena, before the object layout, so each step is
  checked against the goldens on its own.
- **An object will be a header word, a class word and its fields** (planned;
  decided with the user on 2026-09-29; "Planned: the object layout" above).
  The header's guard is to be the application id, because a class-index
  guard would make nested classes megamorphic; the class object gets a word
  of its own rather than a class index packed beside the guard, so that
  `class` and `outer` stay one load.
- **Strings will store code points in one of two widths**, Latin-1 or 32
  bits, with UTF-8 only at the boundaries (planned; decided with the user on
  2026-09-29; "Planned: the object layout" above).
- **A program's own `become:` will refuse value objects** (decided with the
  user on 2026-09-29; "Planned: `become:` through forwarders" above): a value
  is immutable, and forwarding it would change what every holder holds. Editing
  a value class still migrates its instances inside the VM.
- **Reified frames will become heap objects** in the same space, and the
  frame arena will go (planned; decided with the user on 2026-09-29).
  Closures and class objects follow them there, a derived part of the same
  layout.
