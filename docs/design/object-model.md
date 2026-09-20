# Object model — values, objects, mixins, classes

> **Status:** built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `types.rs` (`Object`, `ObjectData`, `Value`),
> `mixin.rs`, `interpreter/instantiate.rs`, `interpreter/nested.rs`.

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
indexable object for step 6's kernel, where `Symbol` is a subclass of
`String`. Rejected: a symbol as an immediate `Value` variant, which no
reference does and which every string primitive, and later the Newspeak
kernel, would have to accept as a non-object. An image carries a symbol as a
record; the decoder enters each one into the loading heap's table and refuses
a second record of the same text
([image-format.md](image-format.md), "Verification").

**An `ObjectReference` is an index, not a pointer.** Every `Object` lives in one
`Heap`'s arena and is reached only through a `Heap` accessor; the index is
private to `crates/ns-runtime/src/heap.rs`, so the compiler refuses to read it
anywhere else. `Value` is therefore `Copy` and a handle has no destructor. What
that representation buys, what it costs while nothing reclaims a slot, and why a
handle means nothing against a different `Heap` are in [memory.md](memory.md).

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
    methods: IdentityMap<Symbol, MethodEntry>,  // compiled methods, slot accessors, primitives
    slots: Vec<MixinSlot>,        // name, mutability, accessibility
    initializer_expressions: Vec<ast::Statement>,
    factory: Option<Rc<ast::MessagePattern>>,
    nested: Vec<Rc<Mixin>>,       // the nested declarations, for their accessors
    class_side: Option<Rc<Mixin>>,
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
   yielding a class object; take its application.
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

**Deviation from §3.2, scheduled for step 6.** The specification also makes
every class formally an instance of its metaclass, which is in turn an instance
of `Metaclass` and a subclass of `Class`. That reified hierarchy is not built:
what exists is the lazily created metaclass above, which carries class-side
behaviour and nothing more, and a class object's own class is the bootstrap
`Object` placeholder. It lands with the core library in Newspeak
([core-library.md](core-library.md)), which is where `Class`, `Behavior` and
`Metaclass` become declared kernel classes rather than objects hand-built in
Rust — both references declare them exactly there
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`) — and which sits immediately before the
reflection work that cannot be done without them: a mirror's test for a
metaclass is `Metaclass = classOf: reflectee`, which needs the instance-of
relation reified. Pulling it earlier would cost a metaclass per class object,
built eagerly where it is lazy today, and a change to the decoder's placeholder
scheme (below); nothing before step 6 depends on it. See
[liveness-and-reflection.md](liveness-and-reflection.md).

One consequence is load-bearing elsewhere: `Object::class` is not an
`Option<ClassReference>`, because every class object's own class is the bootstrap
`Object` placeholder. The image decoder relies on that: it allocates every
record as a placeholder pointing at the bootstrap `Object` before it reads the
record that says what the class really is. Building §3.2's hierarchy would make
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

**Planned: `become:` becomes expressible**, once there is a collector. Adding or
removing an instance slot on a class with live instances requires allocating
new-shape instances, copying fields and swapping identity, which the layout model
already anticipates: the class owns the layout and access resolves through the
current one. The arena supplies the precondition — identity is an index the
`Heap` owns, not a pointer a caller holds — but not the mechanism. Swapping
identity safely means finding and rewriting every reference, which is object
forwarding in a moving collector, and no collector exists yet
([memory.md](memory.md)). Until one does, the MirrorBuilder in
[liveness-and-reflection.md](liveness-and-reflection.md) stays blocked on it.

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
- **Identity hashes come from a counter, not from addresses, and are assigned on
  first request**, so identity survives a moving collector and a snapshot, and
  allocation order is unobservable.
