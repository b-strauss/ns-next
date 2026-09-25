# Object model — values, objects, mixins, classes

> **Status:** built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `types.rs` (`Object`, `ObjectData`, `Value`),
> `mixin.rs`, `interpreter/instantiate.rs`, `interpreter/nested.rs` (the
> nested-class accessor and `build_applied_levels`, §6.2's levels),
> `interpreter/kernel.rs` (the builtin classes, filled from `Kernel.ns`),
> `interpreter/value_class.rs` (§3.1.1's conditions on a value class).

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
because a handle means nothing against another heap. It is a strong root
until the collector, which makes it weak ([memory.md](memory.md), "What the
collector traces"). This is what every reference does -- Primordial Soup's
weak `symbolTable` (`KernelForPrimordialSoup.ns`), SOMns's
`Symbols.symbolFor`, Squeak's symbol table -- and it keeps a symbol an
indexable object for a kernel where `Symbol` is a subclass of `String`, as
the references declare it; `Kernel.ns` still puts `Symbol` directly under
`Object`. Rejected: a symbol as an immediate `Value` variant, which no
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

`ClosureData { parameters, code, captured_frame: FrameReference, home: FrameReference }`
captures its defining frame for closure semantics and its home activation for
non-local return. `parameters` and `code` are `Rc`s shared with the `MakeClosure`
instruction that made the closure; the payload itself is inline in the closure
object, never behind an `Rc` of its own, because a handle lives only where the
tracer can rewrite it in place ([memory.md](memory.md), "What the collector
traces") and an `Rc` shared by several holders is not such a place. So a block's
activation (`FrameKind::Closure`), a native `whileTrue:` or `do:` loop's state
and a decided block step name the closure object, and reach its body and its
two frames through it. **`home` is a `FrameReference`, not an `ObjectReference`** — it names the
lexically enclosing *method* or initializer activation, which is what
`raise_return` walks to. `types.rs` names it an activation at the field too,
because the enclosing *object* is the tempting misreading and both are handles.

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
    metaclass: Option<ClassReference>,          // built on first class-side send
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
  `Float`, and `String`, `Symbol` and `UndefinedObject` (§5.1.3 names
  `nil`'s class so). `Number` is empty; it is the
  place §3.1.1 names, and where the numeric tower goes when there is one
  ([core-library.md](core-library.md), "Gap: an integer has no `/`").
  `Array`, `OrderedCollection`, `Block` and `Platform` stay directly under
  `Object`, since their instances are not values. The VM mirror's class,
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
  refused. `Array new` and `OrderedCollection new` build their own payload
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
  value class: an immediate, a `String`, a `Symbol`, or an
  instance whose class inherits `Value`. An `Array` stored there would make the
  whole value mutable through it, and two copies of the value would stop being
  equal the moment one array was written. The decision and its text live in
  `interpreter/instantiate.rs`; `vm.rs` only calls it.

  The rule is about the slots a class **declares**. The hidden nested-class
  cache slot ("The nested-class accessor" above) is written by the accessor,
  not by `InitializeSlot`, so it never meets the check — and should not: it
  holds a class object, which is no value, but it is the implementation of a
  member §6 defines rather than state of the value. A value class may declare a
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
  the mutable-slot refusal is), then the mutable slot when the chain inherits
  `Value`, then the identity method. `apply_mixin` looks for the first level
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
that slot at `OrderedCollection` rather than `String`, so the class it
instantiates is no value class in an ordinary module; its recorded outcome
did not change.

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

The metaclass mixin is the class side *together with the primary factory
method* (§6.3: "The class side, together with the primary factory method,
defines the mixin for the metaclass"). The metaclass is created lazily, on
the first class-side send or reflective access — `X new` is one, since the
factory is one of its members. Class-side declarations are kept and dispatched
as metaclass behaviour rather than parsed and dropped.

**The primary factory is an entry of that method table**, under the declared
factory's selector (`new` for a declaration that writes none, §6's
unnumbered introduction, before §6.1 begins), found by
the ordinary lookups like any other member — as both references build it
(Primordial Soup's `astForPrimaryFactory:`, SOMns's
`MixinBuilder.assemblePrimaryFactoryMethod`). It carries no body: what it
does is instantiate the class object the message was sent to, which
`interpreter::resolve::act` runs through `instantiate::decide_factory`. The
three special cases that existed while it was not an entry are gone with
nothing in their place — the factory check ahead of the lookup, the `Factory`
target's re-check, and the re-check ahead of a `super` send's
`doesNotUnderstand:` target — because a flat metaclass puts a factory entry
on the receiver's own metaclass and nowhere else, which the class-side
dispatch id determines. `primary_factory_selector` survives only where §6.1
still needs it, in the superclass clause's check.

The entry is derived, never stored twice: `instantiate::build_class_side`
inserts it as a class side is built from source, `image::mixins::decode`
derives it for a row another row names as its class side, and
`Mutator::metaclass_of` puts it in the otherwise empty mixin a class that
declares no class side gets. One definition of what it is
(`mixin::factory_entry`), so the three cannot disagree about which selector
instantiates a class. Nothing about it reaches the image: like a slot's
accessors (§6.3.2), it is rebuilt on the way out from the factory the mixin
table already carries.

**A metaclass is flat: a class side is not inherited.** §3.2 states it and says
what it is not — "All metaclasses are instances of class Metaclass, and are
direct subclasses of Class. Metaclasses may not be subclassed. […] This differs
from Smalltalk, where the metaclass would be a subclass of the metaclass of the
class' superclass. The metaclass hierarchy in Newspeak is deliberately kept very
flat" — and §6 gives the construction: a class is an instance of "a metaclass
that is the application of the metaclass mixin defined by the class' class side
to the class Class". So a subclass answers a class-side message only if its own
class side declares it, and a class-side `super` send looks in `Class`. Since
ns-next reifies no `Class` (below), a metaclass's superclass is `Object`'s
behaviour, which is where the class-side chain already falls through.

`Mutator::metaclass_of` builds exactly this: one application, for the
class's own metaclass mixin, with no superclass application — so an ordinary
class-side send that misses on the metaclass never continues onto an
ancestor's, which is what "not inherited" means for the lookup that finds
members. A miss falls through to the class object's own behaviour, the same
answer §5's ordinary lookup already gives. The metaclass's own *class-object*
superclass link, read only by a `super` send, still points at `Object`'s
behaviour: §5.10 makes S the superclass of the activation's current class,
which §3.2 makes `Class` for a metaclass, and `Object`'s behaviour is what
stands in for `Class` here — so a class-side `super new` no longer
instantiates, because `Object` declares no factory for it to find.

A metaclass that reaches an image — as a class-side activation's
`current_class`, captured in a frame — is rebuilt with the same asymmetry:
its record names the superclass `Object`'s class object like any other, and
`image::decode::rebuild_applications` recognizes a record whose mixin is some
declaration's class side and applies it with no superclass application
([image-format.md](image-format.md), "What is *not* serialized").

Every class object has a metaclass, a class that declares no class side
included: the factory lives on it. The synthesized mixin such a class gets is
memoized per class mixin, for the reason the application cache is keyed by the
superclass's application (§6.2) — a fresh mixin per class object would give
every enclosing instance its own behaviour and send every `X new` site
megamorphic.

**Deviation from §3.2, scheduled for step 6's third plan.** The specification also makes
every class formally an instance of its metaclass, which is in turn an instance
of `Metaclass` and a subclass of `Class`. That reified hierarchy is not built:
what exists is the lazily created metaclass above, which carries class-side
behaviour and nothing more, and a class object's own class is the builtin
`Object` class. It lands with step 6's third plan, the next part of the
core library in Newspeak ([core-library.md](core-library.md)), which is
where `Class`, `Behavior` and `Metaclass` become declared kernel classes
rather than objects hand-built in Rust — both references declare them exactly there
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`) — and which sits immediately before the
reflection work that cannot be done without them: a mirror's test for a
metaclass is `Metaclass = classOf: reflectee`, which needs the instance-of
relation reified. Nothing built so far depends on it. The design is settled
(below, "Planned: `Behavior`, `Class` and `Metaclass`"); see also
[liveness-and-reflection.md](liveness-and-reflection.md).

One consequence is load-bearing elsewhere: `Object::class` is not an
`Option<ClassReference>`, because every class object's own class is the
builtin `Object` class, which exists from the first moment of the bootstrap.
The image decoder relies on that: it allocates every record as a placeholder
pointing at the builtin `Object` before it reads the record that says what
the class really is. The planned hierarchy keeps that: the builtin `Object`
stays the placeholder's class, and decoding a class record overwrites it with
the record's metaclass, as it already overwrites every other placeholder
field.

### Planned: `Behavior`, `Class` and `Metaclass`

**Status: designed with the user on 2026-09-25, not built.** Step 6's third
plan builds it; when it lands, this subsection becomes the description of
what exists and the deviation paragraph above goes.

**Where a class object's state lives.** In `ClassData`, as today. The three
kernel classes carry behaviour only, as SOMns's do (`Class = Object ()`,
`Metaclass = Class ()()` in `prior-art/somns/core-lib/Kernel.ns`, the state in
the Java `SClass`) and as `KernelForJS.ns`'s do. Primordial Soup instead makes
a class object an ordinary instance whose first six slots the VM knows by
position (`Behavior` in `KernelForPrimordialSoup.ns`); taking that shape would
move lookup, the caches, the collector and the image format onto slot indices
and read every lookup step out of a `Value` slot, for no conformance gain:
§3.2 fixes the relations — instance-of, subclass-of, flatness — and says
nothing about how a class object is stored.

**The hierarchy** is Primordial Soup's:

```
Object
  Behavior      what a class and a metaclass share
    Class       the superclass of every metaclass
    Metaclass   the class of every metaclass
```

The specification names neither `Behavior` nor `Metaclass`'s superclass.
`Behavior` is there so that what only a real class has — being instantiable
by its declaration, a name of its own — sits on `Class` and a metaclass does
not inherit it: §3.2's "Metaclasses may not be subclassed", and a metaclass
that could be instantiated would give a class a second metaclass, which
§3.2's "unique" rules out. SOMns's `Metaclass = Class` would make every
metaclass carry `Class`'s protocol and override it away again. None of the
three inherits `Value`: whether a class object is a value is decided per
object (below), and under the strict enforcement of §3.1.1 a subclass of
`Value` refuses every instance that is not one.

**The relations.** A class `Foo`'s metaclass is the application of `Foo`'s
metaclass mixin — its class side together with its primary factory, §6.3 —
**to `Class`**: §6's "the application of the metaclass mixin defined by the
class' class side to the class `Class`", taken literally. Today's metaclass
has no superclass application and a miss falls through to `Object`'s
behaviour; that asymmetry goes. The metaclass becomes an ordinary
application whose superclass application is `Class`'s, so the ordinary lookup
walks class side, `Class`, `Behavior`, `Object`, and a class-side `super`
send lands in `Class`. It is still flat: its superclass is always `Class`,
never the metaclass of `Foo`'s superclass. `Foo`'s header `class` is its
metaclass, and every metaclass's header `class` is `Metaclass`. `Metaclass`
is a kernel class with a metaclass of its own, whose class is `Metaclass`
again, which closes the loop without a regress. A class that declares no
class side keeps the synthesized, memoized mixin that holds only the factory
entry.

**Eager, not lazy.** `new_class_object` builds the metaclass together with
the class object, so the header is right from the start. The alternative —
a header that points somewhere provisional until something asks for the
metaclass — would make every direct reader of an object's class a source of
silent wrong answers for class objects. SOMns builds the two together, as
Squeak's Newspeak does. `ClassData.metaclass`, the lazy cache, goes: the
metaclass is the header `class`. `ClassData` gains `this_class`, a
metaclass's link back to its class, which `Metaclass>>name` needs, as
Primordial Soup's `Metaclass` carries `thisClass`. The cost is one more class
object per class object — for a nested class, once per enclosing instance,
on the first access — measured by the usual protocol
([execution.md](execution.md), "What removing the re-entries moved").

**The bootstrap** creates class objects — `Top`, `Object`, `Behavior`, the
`Kernel` module's own — before `Class` and `Metaclass` exist. They get the
builtin `Object` as a provisional header `class` and are repaired once
`Class` and `Metaclass` are built, as Primordial Soup's and Squeak's
bootstraps do. After the bootstrap the invariant holds without exception —
every class object's class is a metaclass, every metaclass's class is
`Metaclass` — and a test pins it.

**Dispatch becomes uniform.** A class object's dispatch id is its class's
instances id, the metaclass's, the same rule as for every other object; the
class side's "plus one" special case in the stamping goes, as
[dispatch.md](dispatch.md) announces ("Step 6 makes the stamping uniform").
The inline caches still guard on the header id and change in nothing.

**The protocol is minimal, as Primordial Soup's is.** Reflection is a
mirror's job, which the reflection step after this one builds; a class does
not answer `superclass`, `selectors` or its methods at the base level, so
holding a class grants no reflective capability. `Class>>name` answers the
mixin's name through a new VM-mirror leaf and `Class>>printString` answers
`name`; `Metaclass>>name` answers `thisClass name, ' class'`, reading
`this_class` through a leaf, and `Metaclass>>printString` answers `name`.
`Object` gets **no accessor** for `Behavior`, `Class` or `Metaclass`: none of
the three references declares one, §3.2's accessors are for the classes of
the built-in literals and the array factories, and the mirror code that will
need the names lives in the kernel or the platform, where it names them
directly. `interpreter/kernel.rs`'s accessor-set test stays as it is.

**Instantiation is refused.** The three are reachable anyway —
`3 class class` is a metaclass and its `class` is `Metaclass` — so refusing
to instantiate them is needed whatever is nameable. `Mutator::apply_mixin`
records per application whether the chain inherits `Behavior`
(`inherits_behavior`, beside `inherits_value`), and
`instantiate::decide_factory` refuses every such class with an ordinary
Newspeak error under its factory selector, its text in
`interpreter/instantiate.rs`: class objects are made only by evaluating a
declaration. That covers `3 class class class new` and any subclass of
`Class` a program manages to declare.

**A class object is a value exactly when its enclosing object is**, as SOMns
decides it (`SClass.isValue`). A module definition has no enclosing object
and is always one, which is what §6.4 requires ("Module definitions are value
objects"). A nested class of a value is one too: its only state is its
enclosing object (§6.1) and, where its superclass clause depends on the
enclosing instance, its superclass, so §3.1.1's condition on enclosing
objects is what decides it. A metaclass is a value exactly when its class is.
Two consequences:

- **Step 4 of the value conditions** ("A value object's four conditions",
  above) accepts a class object that is a value in a slot of a value's
  instance, and still refuses one that is not.
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

  `Behavior` declares `=`, `==` (answering `self = other`, as §3.1.1 asks of
  a value's identity method) and `hash`. `=` starts with a leaf that answers
  at once when the two are identical, when either is no value or when their
  applications differ, and declines only for two value class objects of one
  application with distinct enclosing or superclass objects, which Newspeak
  then compares with `=` — a leaf may not send, exactly as for `Value>>=`.
  The common `self class == other class` stays one leaf and no activation.
  `hash` of a value class object is derived from its application and the
  hashes of its enclosing and superclass objects; every other class object
  keeps its identity hash.

**The image format.** A metaclass is never a record of its own; it is
derived, never stored twice, as the factory entry is. A reference to a
metaclass anywhere in the graph — which today can only be a class-side
activation's `current_class`, and afterwards can be any slot holding
`Foo class` — is written as "the metaclass of" a class reference, a record id
or a builtin class tag. The decoder allocates a class record's metaclass
placeholder beside the class's own, which keeps a metaclass's identity across
a round trip and ends the case `image::decode` documents today, a decoded
metaclass built a second time beside the one the class mints. That changes
the amplification bound ([memory.md](memory.md)), which is recomputed and
stays proportional to the input through `bounded_count`.
`rebuild_applications`'s flat special case becomes the ordinary application
over `Class`. Verification additionally checks that every class object's
class is a metaclass whose `this_class` is that class object. The new wire
forms — the metaclass reference and builtin class tags for `Behavior`,
`Class` and `Metaclass` — take the next free numbers, and nothing is
renumbered. `read_program_builtin_class` refuses the three new tags wherever
a program *names* a class, as it refuses `Top`'s, since no source module can
name them; as values in the graph they are accepted, since
`3 class class class` reaches them.

**What changes in the goldens.**
`goldens/class-side/class_side_super_reaches_object.ns` becomes
`class_side_super_reaches_class.ns`: a class-side `super printString` now
answers `Class`'s, the class's name. New goldens pin `Foo class printString`
answering `Foo class`, the loop `Foo class class == Foo class class class class`,
the equality and hash of value class objects over two equal enclosing values,
a value slot accepting a value class object and refusing one that is not,
and the refusal of `Metaclass new`. Every other golden whose output changes
is justified one by one before it is updated.

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
`heap.rs`. The representation, what it measured, and the root discipline —
all three of its parts built, the lifetime brand included, and the collector
still to come — are designed in [memory.md](memory.md), "Checked roots and a
collector".

Two consequences belong here rather than there, because they are about this
document's subject.

**An activation is an object too, when something needs it to be.** A running
activation is an entry on the `Heap`'s activation stack, and it becomes a
first-class `Frame` — an entry in the same `Heap`'s frame arena, addressed by a
`FrameReference` with the same privacy as an `ObjectReference` — only when a block captures it.
So an activation nothing captures costs no allocation and leaves nothing behind,
and one that is captured is stored the way an object is. See
[execution.md](execution.md), "Activations".

**Planned: `become:` becomes expressible**, now that the collector's object
forwarding exists. Adding or removing an instance slot on a class with live
instances requires allocating new-shape instances, copying fields and
swapping identity, which the layout model already anticipates: the class
owns the layout and access resolves through the current one. The arena
supplies one precondition — identity is an index the `Heap` owns, not a
pointer a caller holds — and the mark-compact collector supplies the other:
swapping identity safely means finding and rewriting every reference, which
is exactly what its forwarding table already does for a collection
([memory.md](memory.md), "The shape: mark, compact, forward"). What is still
missing is `become:` itself — the primitive that swaps two identities
outside a collection and drives the same forwarding on demand — so the
MirrorBuilder in [liveness-and-reflection.md](liveness-and-reflection.md)
stays blocked on that, not on the collector.

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
- **Identity hashes come from a counter, not from addresses, and are assigned on
  first request**, so identity survives a moving collector and a snapshot, and
  allocation order is unobservable.
