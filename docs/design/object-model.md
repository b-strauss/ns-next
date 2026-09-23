# Object model — values, objects, mixins, classes

> **Status:** built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `types.rs` (`Object`, `ObjectData`, `Value`),
> `mixin.rs`, `interpreter/instantiate.rs`, `interpreter/nested.rs` (the
> nested-class accessor and `build_applied_levels`, §6.2's levels),
> `interpreter/kernel.rs` (the builtin classes, filled from `Kernel.ns`).

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
  2026-09-22, as both references declare them, so the conditionals and logic
  are `Boolean`'s methods, inherited), `Number` with `Integer` and `Float`,
  and `String`, `Symbol`, `Character` and `Nil`. `Number` is empty; it is the
  place §3.1.1 names, and where the numeric tower goes when there is one
  ([core-library.md](core-library.md), "Gap: an integer has no `/`").
  `Array`, `OrderedCollection`, `Block` and `Platform` stay directly under
  `Object`, since their instances are not values. The clause is read by name
  at bootstrap, never run, and the class object's `superclass` link is set to
  match its application's.
- **`Top` is not nameable.** It is the one class of `Kernel.ns` declared
  private, so §6.3.1 keeps it out of every other module's reach; no bare name
  resolves to it (`instantiate::builtin_kind_from_name` omits it), and the
  image decoder refuses its tag wherever an artifact names a class, as it
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

### A value object's two conditions

§3.1.1 makes an object a value object when "it is deeply immutable ... and its
class inherits from class `Value`", and the two halves of that — the mutable
slot and the slot that is not a value — are enforced as the instance is built,
the one place every instance of every class comes from, whether its class was
loaded from source, materialized as a nested class or rebuilt from an image.
The same bullet of §3.1.1 asks two further things that nothing here checks:
that a value's enclosing objects (§3.3) are values, and that its class does
not define `==` ([core-library.md](core-library.md), "What is still
unchecked"). Both are to be enforced strictly by the next plan, which mixin
application ("Mixin application" below) was built for: a module can now
declare itself a value, `class M = Object <: Value (…)`, so a value class it
declares can have an enclosing object that is one. The two halves enforced
today:

- **Step 1 refuses a mutable slot.** A class that inherits `Value` and declares
  one could never have a value object for an instance, so there is no instance
  to make and nothing is allocated. `Mutator::apply_mixin` answers both
  questions once per application — whether the chain reaches `Value`
  (`inherits_value`) and whether any level of it declares a mutable slot while
  it does (`mutable_value_slot`) — because the same mixin applied over a
  superclass that is no value declares an ordinary mutable slot and is perfectly
  legal.
- **Step 4 refuses a declared slot that is not a value.** *Deeply* immutable
  means "all objects reachable from it are also deeply immutable", so
  `InitializeSlot` refuses to store anything but a value in an instance of a
  value class: an immediate, a `String`, a `Symbol`, a `Character`, or an
  instance whose class inherits `Value`. An `Array` stored there would make the
  whole value mutable through it, and two copies of the value would stop being
  equal the moment one array was written. The decision and its text live in
  `interpreter/instantiate.rs`; `vm.rs` only calls it, so both lowerings raise
  the same error.

  The rule is about the slots a class **declares**. The hidden nested-class
  cache slot ("The nested-class accessor" above) is written by the accessor,
  not by `InitializeSlot`, so it never meets the check — and should not: it
  holds a class object, which is no value, but it is the implementation of a
  member §6 defines rather than state of the value. A value class may declare a
  nested class without ceasing to be one.

Both are ordinary Newspeak errors naming the class and the slot, raised at run
time — a class nothing instantiates never fails, as a method nothing runs never
does.

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
effect of how a root clause is resolved, and it stays allowed when the gap
below is closed (`goldens/mixin/mixin_module_declares_itself_a_value.ns`).

**Gap: every other root `<:` clause is accepted or dropped instead of
refused.** A root class has no enclosing object to evaluate a receiver in, so
`load_root_class` resolves each `<:` receiver as it resolves the root's
superclass (`resolve_superclass_receiver`): by builtin name alone. A root
clause naming another builtin adds a level for it; one prefixed `self` or
`super` that names anything but a builtin adds no level, silently; and
`builtin_kind_from_name` ignores the prefix, so `<: self Value` is taken as
the builtin `Value`. §6.1 evaluates such a clause "as if it was a top level
expression", where it "will almost certainly fail". The root superclass clause
has the same prefix gap — a prefixed one naming anything but a builtin is
dropped, and a prefix before a builtin's name is ignored — while resolving a
builtin such as `Object` by name is what it should do. Plan B closes the gap
([PROGRESS.md](../PROGRESS.md), "Where the next session starts"): it keeps
`<: Value` and makes every other root `<:` clause — prefixed, naming another
builtin, or `<: self Value` — a deferred program error, and refuses a prefixed
root superclass clause the same way. Until then the runtime only over-accepts a program the rule refuses
([README.md](README.md), deviations).

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
relation reified. Pulling it earlier would cost a metaclass per class object,
built eagerly where it is lazy today, and a change to the decoder's placeholder
scheme (below); nothing built so far depends on it. See
[liveness-and-reflection.md](liveness-and-reflection.md).

One consequence is load-bearing elsewhere: `Object::class` is not an
`Option<ClassReference>`, because every class object's own class is the
builtin `Object` class, which exists from the first moment of the bootstrap.
The image decoder relies on that: it allocates every record as a placeholder
pointing at the builtin `Object` before it reads the record that says what
the class really is. Building §3.2's hierarchy would make
a class object's class a real, distinct object and would invalidate it.

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
- **Identity hashes come from a counter, not from addresses, and are assigned on
  first request**, so identity survives a moving collector and a snapshot, and
  allocation order is unobservable.
