# Object model — values, objects, mixins, classes

> **Status:** built, a value as one tagged word, the object layout (road
> step 3a, "The object layout") and the object space with byte-offset
> handles (road step 3b, "Handles into the object space") included; the rest
> of the road to a high-performance VM -- mirrors and atomic installation
> over an eager `become:`, inline allocation and the JIT tiers -- is planned ("Planned: the road to a
> high-performance VM").
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `heap/value.rs` (`Value`, `ValueKind`, `Word`, the
> `SmallInteger` bounds and the arithmetic on tagged words),
> `heap/header.rs` (the header word, `Format`), `heap/object.rs` (the block
> of words an object is, the fixed fields of a class object, a closure and
> a frame, tracing by format), `heap/records.rs` (the seam's value types),
> `heap/tables.rs` (the per-heap tables of mixins, applications and code),
> `mixin.rs`, `interpreter/instantiate.rs`, `interpreter/nested.rs` (the
> nested-class accessor and `build_applied_levels`, §6.2's levels),
> `interpreter/kernel.rs` (the builtin classes, filled from `Kernel.ns`),
> `interpreter/value_class.rs` (§3.1.1's conditions on a value class),
> `interpreter/future.rs` and `compiler/simultaneous.rs` (§6.3.2's
> simultaneous slots).

## Values and objects

```rust
Value      = one tagged 64-bit word (heap/value.rs)                  // Copy, opaque
ValueKind  = Nil | Boolean(bool) | SmallInteger(i64) | SmallFloat(f64) | Object(ObjectReference)
Word       = the bare word a heap object stores, 8 bytes in every build
ObjectReference = the 64-bit byte offset of an object in the `Heap`'s object space; ClassReference is an alias

an object = its words in the space, end to end with the others:
            [header word][class word][field 1][field 2] ...   ("The object layout")
```

**A value is one tagged word, Spur's.** Its low three bits say what the
rest is:

| Low bits | Meaning |
|---|---|
| `000` | an object handle: the object's byte offset in the space, a multiple of 8, so that the space's base plus the word is its address |
| `001` | a `SmallInteger` of 61 bits, `(value << 3) \| 1` |
| `010` | a constant: `nil`, `false` and `true` are the payloads 0, 1 and 2, with room for more |
| `100` | a `SmallFloat64` |
| `110` | free: never produced, kept for a future immediate |
| `011`, `101`, `111` | never produced, so that bit 0 alone tells a `SmallInteger` |

The tags are Spur's (`smallIntegerTag` 1, `smallFloatTag` 4, three tag bits,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:2779` for the three tag
bits, `:2836` for `smallIntegerTag`, `:2784` for `smallFloatTag`): Spur writes a
`SmallInteger` as `(value << 3) + 1` (`integerObjectOf`, `:41159`) and tests it
by bit 0 alone. Spur's `Character` tag, `010`, is taken for the constants:
ns-next has no characters, and `nil`, `true` and `false` are immediates rather
than heap singletons as in Spur and Soup, which is why nothing resembling an
object store is needed to name them in a serialized image. A `SmallFloat64` is
Spur's encoding (`floatObjectOf:`, `smallFloatValueOf:`): an eight-bit subset of
the exponent -- IEEE exponents 896 with a non-zero mantissa through 1151, about
2^-127 to just under 2^129, plus ±0.0 (`isSmallFloatValueBits`, `:41232`;
`is_small_float_bits`) -- with the sign rotated down beside the tag. A release
`Value` is the word and the zero-sized brand, eight bytes
(`a_release_value_is_one_word`).

**Why one word.** LLVM copied the 16-byte enum `Value` this replaced as a
tag byte and two overlapping quadwords, so every `Value` that passed through
a memory temporary -- a result answered by an out-of-line function, a joined
`Result`, `LoadLiteral`'s `make_value` -- was a store-forwarding stall, 28%
of `vm::run`'s samples on the send path ([execution.md](execution.md),
"Where the 3% goes"); fixing it site by site had to inline the whole path to
the register write and nearly doubled `vm::run` ("A send's value delivered
where it is decided: tried and halted", there). A value read and written as
one word cannot stall that way, on Windows x64 it comes back in `RAX`, and
registers, fields, arrays and copied closure values take half the memory;
`Signal` and `EvaluationResult` shrank from 48 to 40 bytes with it. What it
moved is [execution.md](execution.md), "What the eight-byte value moved".

**`Value` is opaque.** Outside `heap/value.rs` no code sees the word. A
value is made with `Value::nil()`, `Value::boolean`, `Value::small_integer`
(a `debug_assert!` that the integer fits) and `Value::object`, a float only
by `Mutator::new_float` and an integer from an `ArbitraryInteger` only by
`Mutator::new_integer`; it is read with `is_nil`, `as_boolean`,
`as_small_integer` and `as_object`, each answering an `Option`, and an
exhaustive match goes through `value.kind()`, the `ValueKind` view, which
needs no heap. `SmallFloat` is the immediate float only: code that reads a
float asks `Heap::float_value`, which answers a boxed one too. Tests compare
through the accessors (`value.as_small_integer() == Some(3)`).

**Each number has one representation.** A `SmallInteger` is every integer
from `SMALL_INTEGER_MIN` to `SMALL_INTEGER_MAX`, -2^60 to 2^60 - 1; any
other is an object of the kernel class `LargeInteger`, of the 64-bit-indexed
format: its words are the magnitude's digits, its sign is the header's
negative bit, and it holds no handle.
`Mutator::new_integer` decides which ([core-library.md](core-library.md),
"Large integers"). A finite float in the `SmallFloat64` range is immediate;
every other float -- NaN, the infinities, denormals and magnitudes outside
that range -- is a boxed `Float`, an instance of the builtin `Float` whose
one 64-bit-indexed word holds the `f64`'s bits and no handle, and
`Mutator::new_float` decides which by Spur's range test. Soup boxes every
float, which costs an allocation per floating-point result; Spur's split
keeps the common range immediate. Both are §5.1.1's binary float; the
`FixedPointNumber` that section names for non-integral literals (the
recorded deviation, [core-library.md](core-library.md)) would be a class of
the numeric tower, not a tag. A `Fraction` is an object too, of the
fixed-pointer format, its two fields the numerator and the denominator in
normal form, each a `SmallInteger` or a `LargeInteger`
([core-library.md](core-library.md), "The numeric tower"). A boxed
`Float`, like a `LargeInteger` and a `Fraction`, is a value object
(§3.1.1), so its `=` and its hash come from its value, never from the
identity hash: every number compares and hashes by its exact value, across
the four classes, and a NaN equals a NaN either way.

**Small-integer arithmetic works on the tagged words**, as Cog's and Soup's
does, and is what a JIT's tiers build on. The operations are methods on
`Value` (`small_integer_sum` and its siblings), called by `vm.rs`'s
`IntegerBinary` arm and by `builtins/numbers/integer.rs` alike, so the rule has one
copy and `vm.rs` holds no logic of its own. One test, `left & right & 1`,
checks both operands; addition and subtraction work on the words with one
overflow test, since an `i64` overflow of the tagged word is exactly a
61-bit overflow; comparisons compare the words; multiplication untags one
operand, as Cog's and Soup's does; and `//` untags both, since a floor
quotient of a tagged word is not the tagged floor quotient. An inline cache
reads an immediate's dispatch id from a table indexed by its tag
(`immediate_index`), and for the constant tag by the constant.

**Literals.** An integer literal beyond 61 bits keeps its digits in the
Rust-side literal data of its code (`Literal::LargeInteger`, which holds no
handle), and `LoadLiteral` allocates the `LargeInteger` each time it runs --
a value needs no identity, so a fresh one per execution is correct
([compiler.md](compiler.md), "Compilation cannot fail"). A float literal
outside the `SmallFloat64` range does the same with a boxed `Float`, through
`Mutator::new_float`.

**The debug build keeps its exact check.** A debug handle carries its heap's
id and its move epoch in full, and every accessor compares both
([memory.md](memory.md), "The brand, checked by the compiler rather than by
convention"). The Rust-side `Value` does too: in a debug build it is the word
plus both numbers, 16 bytes, zero for an immediate, and in a release build it
is the bare word. What is exactly eight bytes in both builds is a `Word`, what
a heap object stores -- its class word, a field, an element of an `Array`, a
register (the activation stack's and a reified frame's), a copied closure
value and every fixed field of a class object, a closure and a frame -- so
that the layout is the same in both. Only `Heap::load` and `Heap::store`
convert: a load gives the word the heap's current id and epoch, and a store
checks, in a debug build, that the value carries both before it becomes a
word, so a stale handle is caught where it is stored
(`a_stale_handle_is_caught_where_it_is_stored`) and cannot be laundered into a
field. Both are crate-private, since a word carries no brand
([memory.md](memory.md), "The brand"). A stored word cannot go stale the way a
Rust-side handle can, because every collection and every stress move rewrites
it in place, through `Tracer::word`, which the collector hands every word an
object's format says is a pointer ([memory.md](memory.md), "What the
collector traces"). What still holds a Rust-side value or handle -- a stack
entry's receiver, a continuation's state, a host's `Root` -- keeps its brand
and its `Trace`.
A scavenge walks only the young objects and the remembered ones, so an old
object whose store missed the write barrier keeps a stale young word; that
is what `NS_NEXT_STRESS_SCAVENGE`'s check of the remembered set before each
scavenge, and a debug build's poison over the nursery a scavenge vacated,
catch ([memory.md](memory.md), "Generations").

A `String`'s words hold its code points in one of two widths and no handle
([core-library.md](core-library.md), "Strings are code points in two
widths"; "Strings have two widths" below).

The guard in an object's header word is the id an inline cache guards the
object's sends on, a copy stamped from its application's ids
([dispatch.md](dispatch.md), "The dispatch id").

**A symbol is a heap object, and a canonical one**: each `Heap` keeps a
canonical table, `Vec<Option<ObjectReference>>` indexed by the number inside a
`Symbol` -- which the interner assigns densely -- and `Mutator::new_symbol`
answers the table's object, allocating it only the first time. It is the one way
to make a symbol, so a symbol literal, `asSymbol` and every selector handed to
`doesNotUnderstand:` all reach the same object for the same text, which is what
§5.1.6 and §3.1.1 make two equal symbols. The table is per heap because a handle
means nothing against another heap. It is weak: the collector clears an entry
whose symbol nothing else holds ([memory.md](memory.md), "Weak references").
This is what every reference does -- Primordial Soup's weak `symbolTable`
(`KernelForPrimordialSoup.ns`), SOMns's `Symbols.symbolFor`, Squeak's symbol
table -- and it keeps a symbol an indexable object, so that `Symbol` is a
subclass of `String` in `Kernel.ns`, as the references declare it: a symbol's
words are a string's, in the same two widths, and only its class tells it
from one ("Strings have two widths" below). The symbol a symbol object is,
`Heap::symbol_value`, is the table's inverse, kept beside it per heap and
keyed by the object's byte offset, so the object stores nothing but its code points
and no lookup reads them. Rejected: a symbol
as an immediate, a tag of its own, which no reference does and which every
string primitive, and later the Newspeak kernel, would have to accept as a
non-object. An image carries a symbol as a record; the decoder enters each one
into the loading heap's table and refuses a second record of the same text
([image-format.md](image-format.md), "Verification").

**An `ObjectReference` is an offset, not a pointer.** Every object lives in one
`Heap`'s object space and is reached only through a `Heap` accessor; the offset is
private to the `heap` module -- `crates/ns-runtime/src/heap.rs` and its
submodules, `heap/value.rs` among them, which puts it into a word and takes it
out again -- so the compiler refuses to read it anywhere else. `Value` is
therefore `Copy` and a handle has no destructor. What that representation buys,
how the collector reclaims an object once nothing reaches it, and why a handle
means nothing against a different `Heap` are in [memory.md](memory.md).

**The identity hash is drawn from a counter the first time `hash` asks for
one**, never derived from an address and never from the handle's offset, and 0
means "not yet assigned" (`Heap::object_ensure_identity_hash`); it is 22 bits
of the header word, and the counter wraps within them ("The identity hash is
22 bits wide" below). An address- or
offset-derived hash would break the moment a moving collector relocates the
object or a `become:` forwards one object's identity to another, and
identity has to be stable across a snapshot; see [memory.md](memory.md). Assigning it lazily, as Cog/Spur
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

**A closure is an object of the closure format**: its code id, its captured
frame and its feedback base in three fixed fields, then its copied values
("The object layout" below). It captures its defining frame, and derives its
home activation for non-local return from it; the copied values are what
closure conversion handed it -- a copied variable's value or an enclosing
body's indirection vector ([execution.md](execution.md), "Closure
conversion"). The code id names, in the heap's table of compiled code, the
body shared with the `MakeClosure` instruction that made the closure, and its
parameter count is the body's; everything is inline in the closure object,
never behind an `Rc` of its own, because a handle lives only where the tracer
can rewrite it in place ([memory.md](memory.md), "What the collector traces")
and an `Rc` shared by several holders is not such a place. So a block's
activation (`FrameKind::Closure`), a native `whileTrue:` or `do:` loop's state
and a decided block step name the closure object, and reach its body and its
captured frame through it. **The home is a `FrameReference`, not an
`ObjectReference`** — it names the lexically enclosing *method* or initializer
activation, which is what a non-local return aims at, and a frame records it
(its home field); the enclosing *object* is the tempting misreading, since
both are handles.

**Never `{:?}` a `Value`.** `ObjectReference`'s hand-written `Debug` prints its
offset only, so this cannot recurse -- but it renders Rust internals such as
`ObjectReference(@336)`, useless in a message meant for a Newspeak programmer.
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

g Nested.                /// a subclass of String, not of Inner
(g Inner: Array) Nested. /// still the same subclass of String
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
    index: ApplicationIndex,      // its row in the heap's table of applications
    mixin: Rc<Mixin>,
    superclass: Option<Rc<MixinApplication>>,
    own_slot_base: usize,         // where this level's own slots start
    slot_count: usize,            // inherited slots included
}

a class object                    // one per enclosing instance, per level;
                                  // an object of the class format, eight fields:
    0 enclosing                   // object or nil
    1 declaring class             // the declaration chain `outer` walks
    2 superclass                  // parallel to the application chain
    3 this class                  // a metaclass's class; its own metaclass is its header class
    4 builtin                     // SmallInteger position in BuiltinClass::ALL, or nil
    5 value-class refusal         // nil, or an Array of the offender and the level's Symbol
    6 applied clause              // the `<:` clause that added this level, packed, or nil
    7 application                 // SmallInteger ApplicationIndex
```

The layout and its reasons are "The object layout" below; `heap/object.rs`'s
`class_field` names the fields, and every read and write goes through the
`class_*` accessors of `heap.rs`.

**The application cache is keyed by (the mixin's id, the superclass
*application*'s full-width index), never by the superclass class object.** If
the superclass is itself a nested class, its class object is per-instance, so a
class-object key would never hit: every instance would mint its own
application, which is wrong per §6.2 and would leave every call site
megamorphic because no two instances would ever share behaviour. Both ids are
the heap's ("An application has two ids" below).

**Identity-keyed tables hash with `hash::IdentityMap`.** A mixin's `methods`, an
application's `nested_slot`, the interpreter's `applications` and
`metaclass_mixins` and the heap's address maps of mixins and code are keyed by
an identity the runtime assigned — an interned symbol's index, a mixin's id or
address, an application's index — which no input chooses, so
SipHash's resistance to chosen keys buys nothing there. They hash with one
multiply per word and a rotation that brings the mixed high bits down to the
bucket index (the FxHash scheme rustc's own tables use).

### Two parallel chains, both needed

- The **class-object chain** carries identity and, at every level, that level's
  enclosing object plus the class object of the level that lexically declares it
  (`declaring_class`).
- The **application chain** carries code and layout, and is shared.

`own_slot_base`, `slot_count` and method lookup live on `MixinApplication`
rather than on the class object, because a superclass contributes slots and can
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
skips it; its getter is a compiled method in the mixin's `methods`, with no
fast path, which opens with `ReturnSlotIfFilled` -- answering a filled slot
from its own activation -- and otherwise evaluates the initializer and stores
it with `InitializeSlot` ([dispatch.md](dispatch.md), "Slot accessors"), and a
mutable one's setter is an ordinary accessor entry. And it is outside a
value's state: `mixin::declared_slots_of` leaves it out of
`MixinApplication::declared_slots`, so the state-derived `=` and `hash` skip
it ([core-library.md](core-library.md), "Lazy slots first").

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
`Heap::class_set_application`. So the class object is the same one every
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
  "`True` and `False` are quick methods"), `Number` with `Integer`,
  `Float` and `Fraction`, `String` with `Symbol` below it, and `UndefinedObject` (§5.1.3 names
  `nil`'s class so); the platform and its members, `PlatformFactory.ns`'s classes
  but `Stopwatch`, are values too ([core-library.md](core-library.md), "The
  platform"). `Number` is the place §3.1.1 names, and holds what the four
  numeric classes share, the numeric tower's rows among it
  ([core-library.md](core-library.md), "The numeric tower").
  `Array` and `ByteArray`, whose instances are not values, stay under
  `Object`, below `Collection` and the level the kernel mixin
  `IndexedCollection` adds for each (`Collection <: IndexedCollection`;
  [core-library.md](core-library.md), "The sequence protocol is a mixin");
  `Closure` stays directly under `Object`, and so does `Behavior`, with `Class`
  and `Metaclass` below it ("Class sides and metaclasses" below), and so do
  `Proxy`, with the runtime's `FutureImplementation` below it, and the
  public `Future`, which has no instances ("Simultaneous slots" below). The
  error classes `ArgumentError`, `CannotReturn`, `NonBooleanReceiver` and
  `CannotInstantiate` are below `Error` ([core-library.md](core-library.md),
  "Exceptions"). The VM mirror's class,
  which `Kernel.ns` does not declare, sits below `Value`, so the mirror is a
  value ("A value object's four conditions" below). The clause, `<:`
  clauses included, is read by name at bootstrap, never run, and the class
  object's `superclass` link is set to match its application's. The
  kernel's other classes -- `StringBuilder` and `DataView` -- are no
  builtins but ordinary classes of `Kernel.ns`, and the typed arrays are
  ordinary classes of the shipped module `TypedArraysFactory.ns`
  ([core-library.md](core-library.md), "`ByteArray`, `StringBuilder` and
  the views").
- **A program reaches a builtin class through `Object`'s accessors** (§3.2):
  `Kernel.ns`'s `Object` declares one protected method per nameable builtin,
  `protected String = ( ^outer Kernel String )`, and a bare `String` is
  `self String` by §5.7, so a class that declares or inherits a member of
  that name reaches its own, and `other String` from outside reaches
  `doesNotUnderstand:` ([core-library.md](core-library.md), "The
  predefined classes are accessors on `Object`", "`class` and the class
  accessors are protected").
- **`Top` is not nameable.** It is declared private in `Kernel.ns`, as
  only `Activation`, `NonBooleanReceiver`, `FutureImplementation`,
  `IndexedCollection`, `LittleEndianDataView` and `BigEndianDataView` are
  besides it, so §6.3.1 keeps it out of every other module's reach; `Object`
  declares no accessor for it among §3.2's accessors for the predefined
  classes, so a bare `Top` is `self Top` and ends in `doesNotUnderstand:`
  (`goldens/language/builtin-names/top_is_not_a_name.ns`); and the image
  decoder refuses its tag wherever an artifact names a class, as it refuses
  the mirror's ([image-format.md](image-format.md), "Verification"). A program
  can reach it only as `Object`'s superclass, which is what §3.2 allows and
  nothing more.
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
  outright, with a `CannotInstantiate` carrying nil rather than the class a
  program must not be handed — there is exactly one mirror.
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
refused where an instance would come into being**. Every such refusal is an
ordinary Newspeak error named under the primary factory of the class being
instantiated, defaulted to `new`, raised at run time — a class nothing
instantiates never fails, as a method nothing runs never does — and every
text lives in `interpreter/instantiate.rs`, so both lowerings raise the same
errors. The root rule ("Mixin application", at its end), which decides what a
module that declares itself a value may write, is not among them: a root that
breaks it is a resolve error, refused before anything runs.

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
  (`goldens/library/value/a_mutable_slot_above_the_value_level_is_refused.ns`).
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
(`goldens/language/class/nesting/spec_2_3_superclass_of_a_nested_class.ns`, which passes as
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
  (`goldens/library/value/an_identity_method_from_an_applied_mixin_is_refused.ns`).
  Deciding it on the application rather than the class object is what covers
  a module's root along with every nested class: `Mutator::load_root_class`
  builds no class object through `nested::after_clause`, but it still calls
  `apply_mixin` — the one place every application in the system is built,
  root, nested or decoded alike — so a value module whose root class defines
  `==` is refused exactly as a nested one would be
  (`goldens/library/value/a_value_module_defining_identity_is_refused.ns`), and
  `decide_instantiation` refuses it from `instantiation_refusal`, the same
  field the root clause defect and the mutable slot are refused from.
- **Enclosing objects are values** is decided once per *class object*, cold,
  by `interpreter/value_class.rs`'s `check_value_class`, which
  `interpreter/nested.rs`'s `after_clause` calls after `apply_mixin` answers
  the application, and only for an application that inherits `Value`; the
  verdict is recorded on the class object (its value-class refusal field,
  written by `Mutator::record_value_class_refusal` from a
  `ValueClassRefusal`: an `Array` of the offending enclosing object and the
  name of the level at fault, "The object layout" below), and
  `decide_instantiation` refuses an instance of a class that carries one,
  rendering the offender with `Heap::describe` only then, beside the
  refusals it makes from
  `instantiation_refusal`. A class that does not inherit `Value` pays one flag
  test at creation, and a value class one more test per instantiation. The
  class's own enclosing object is checked first, then the walk climbs the new
  class object's superclass chain. At each level: a class that inherits
  `Value` and was created as a class of its own — by `after_clause`, as a
  builtin or as a module root, not a level a `<:` clause added — stops the
  walk **only if it carries no recorded refusal of its own**, because it was
  checked the same way when it was created and everything above it holds by
  induction. If it does carry one, the walk does not trust it: a refused class
  still exists, since the refusal is recorded rather than raised, so the new
  class inherits the fault instead — a `NonValueEnclosingObject` with the
  offender taken from that level's own verdict, naming the level that
  verdict names, or the level the walk stopped at when it names none
  (`goldens/library/value/a_value_subclass_of_a_refused_value_class_is_refused.ns`).
  So a fault inherited over several levels still names the level whose
  enclosing object is at fault, not the last class that passed it on
  (`goldens/library/value/a_value_two_levels_below_a_refused_value_class_names_the_faulty_level.ns`).
  Any other level — one that does not inherit `Value`, or a `<:` level, which
  is never created through `after_clause` — has its enclosing object checked
  with `instantiate.rs`'s `is_a_value`, the predicate the slot check uses, and
  the walk continues. Checking every level and not only the class's own
  enclosing object matters because an inherited method reaches its own level's
  enclosing objects through `outer`: in `class P = Foo new <: Value`, `P`
  inherits `Value`, `Foo` does not, and a method of `Foo` whose enclosing
  object is mutable would make `P`'s instances reach mutable state
  (`goldens/library/value/a_value_over_a_class_with_a_mutable_enclosing_object_is_refused.ns`).
  Requiring `Foo` itself to inherit `Value` would be stricter than §3.1.1: a
  slotless `Foo` in a value module meets every condition, and
  `Foo new <: Value` is how a value variant of it is made
  (`goldens/library/value/a_value_over_a_slotless_class_in_a_value_module_is_accepted.ns`).
  A module's root class has `nil` for its enclosing object and a chain of
  builtins with at most `<: Value`, so its enclosing objects hold by
  construction, and `load_root_class` does not call the check.

  A decoded class object starts with no verdict — its refusal field is
  `nil`, the wire carries none, and a legitimate
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
`class Kernel vmMirror: mirror = Value (…)`, a value as
`prior-art/somns/core-lib/Kernel.ns`'s `Object <: Value` is, and its one
slot is immutable.
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
(`goldens/library/value/a_value_class_in_an_ordinary_module_is_refused.ns`). Every
golden and inline test module that declares a value class therefore declares
itself a value, `= Object <: Value`, and keeps its recorded outcome — the
rule decided with the user on 2026-09-23 for the two goldens declaring
`class Text = String new`, extended to every such module. A value's slot may
not hold the platform or the transcript, so such a module reads the
transcript where it uses it, or keeps its former module methods in a nested
`Script` class that is no value. `goldens/language/class/nesting/nested_override.ns`, which
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
private `FutureImplementation`, and four instructions, `MakeFuture`, `ResolveFuture`,
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
recursion — raises an ordinary, catchable `Error`, "divergent evaluation: a
future was forced while it was being computed", a text that names the
future rather than a slot because a program's own `Future computing:`
diverges the same way. **A clause abandoned by an exception** —
an initializer raises and a handler outside the instantiation takes it —
leaves the futures it had not resolved as they are, wherever they escaped
to: the next message forces one and runs its initializer then, and one
whose initializer raised raises again for the same cause. A forcing whose
activation an unwind pops, or which the run loop refuses to push, leaves its
future unresolved again rather than stuck resolving (`Mutator::abandon`).
Nothing answers wrongly, and it costs nothing; marking them broken would put
an `ifCurtailed:` around every simultaneous clause.

**The future.** `Kernel.ns`'s private `FutureImplementation`, which no
program can name (as `Activation`), is a subclass of the public `Proxy`,
after Primordial Soup's: `Proxy` overrides every public member of `Object`
— `==`, `=`, `hash`, `yourself`, `identityHash`, `printString`, the nil
protocol, `halt` — as protected, and `Object` itself declares `class` and
the class accessors protected, so every message from outside reaches
`doesNotUnderstand:`, and
`FutureImplementation`'s forces the future through the mirror row
`futureValue:ifFail:` (`builtins/future.rs`) and forwards the message to
the value with `Message>>sendTo:`. A member added to `Object` without its
override in `Proxy` fails
`a_proxy_overrides_every_public_member_of_object_as_protected`. Its state is
two private slots, `computation` — what computes it until it is resolved,
then its value — and `state`, 0 unresolved, 1 resolving, 2 resolved,
which only `interpreter/future.rs` reads and writes once the future is
made. It reads them as untrusted, since crafted code can store into them
once it holds a future: any other state, or a resolved future whose value
is a future, is `FutureState::Malformed`, and forcing it is an ordinary
error ([image-format.md](image-format.md), "Futures in a graph"). Forcing
is a trampoline step under `Continuation::Future`, never a call back into
Newspeak: it activates the computation when it is a block, and otherwise
sends it `value` (`Mutator::send_value_then`); once that returns,
`vm::finish` hands its value to
`Mutator::future_computed`, which records it and either delivers it to the
send that forced the future (`AfterForcing::Deliver`) or runs the
instruction that met the future again (`AfterForcing::Reexecute`;
[execution.md](execution.md), "Running an instruction again after forcing
a future"). A block that answers another unresolved future forces that one
next, the waiting forcing chained behind it, so both resolve to its value —
§6.3.2's pipelined promise, as Primordial Soup's `resolve` loops. Nothing
bounds that chain's length, so every walk of it loops rather than recursing
once per link. A program has no message of its own that forces a future,
since a public `resolve` would tell an escaped future from its value: a
future is forced only where the runtime needs the value it stands for -- a
message sent to it, an instruction that must not hold one, or a leaf that
could not answer for it as an operand. A simultaneous clause's futures are
made by the runtime (`Mutator::new_future`, which `MakeFuture` calls), of
a block. `Future computing:`, a program's way to make one, sends
`FutureImplementation`'s factory `computing:` instead -- the one kernel
builtin with a factory of its own -- whose slot initializer stores the
computation as it is; its initializer tells the runtime a future exists
through the mirror row `futureMade:ifFail:`, which sets
`Mutator::futures_made`. `Future` is a public class with that one
class-side method and no instances, and `Future new` — or the
instantiation of a subclass, which does not inherit `computing:` since a
class side is not inherited (§3.2) — signals a `CannotInstantiate` from
its initializer. Storing is no send, so a future handed to `computing:`
is not forced at the call but when the new future is, which sends it
`value` — the canonical implementation's timing, whose `Future` likewise
stores its argument in a slot and sends it `value` when it resolves; the
specification is silent on it. A future still being computed by then is
the divergent-evaluation error, raised by that forcing
(`goldens/library/object/a_future_handed_to_future_computing_is_forced_with_the_new_one.ns`,
`a_future_handed_to_future_computing_is_its_value.ns`).
A program cannot subclass `FutureImplementation` itself: an instance of a
subclass would be a future the runtime does not know, which it neither
forces, sees through, nor replaces, so a forwarding object a program wants
is a subclass of `Proxy` instead. Such a proxy is pure Newspeak: it
forwards every message it is sent, but as an argument it is itself —
`target == proxy` answers `false` — as in Primordial Soup; only
`FutureImplementation` gets the runtime's symmetric see-through, so every
future a program can hold keeps "The four rules" above.
`FutureImplementation` declares no `isKindOfFuture`: answering it would
tell a future from its value, so that message is forwarded to the value
like any other. `FutureImplementation` is a builtin
(`BuiltinClass::FutureImplementation`) because the collector, the leaves
and the image must recognize it; `Proxy` and `Future` are builtins only
because every kernel class is one. The first future a
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

**Where Rust sees a future.** Nothing on a program's paths looks for an
instance of `FutureImplementation` until the program has made one.

- *As a receiver* it needs nothing: every send reaches
  `FutureImplementation>>doesNotUnderstand:`. The inlined conditionals fall back to a real send
  on a receiver that is no Boolean, and `IntegerBinary` on anything but two
  small integers; the compiler inlines neither `==` nor `isNil`.
- *As a leaf's argument or subject*, a future is looked for only on the
  failure path of the three ways a leaf runs — `mirror::execute_sent` (a send
  to the mirror), `send_inlined` (`SendMirror`) and `execute_method` (a
  primitive method): the leaf declined or raised — and only once
  `futures_made` is set: `future::after_a_failed_leaf`, inlined into each
  caller, is that flag test and nothing more, and the search itself is the
  cold `see_through_a_failed_leaf` behind it (`send_inlined` tests the flag
  itself and goes to the cold `mirror::see_through_a_failed_row`);
  `builtins::execute` itself is the plain dispatch to the leaf. A resolved one
  is replaced by its value and the leaf runs again, in Rust. An unresolved one
  cannot be forced in Rust, since nothing re-enters the loop: the answer is a
  step that runs the future's computation under `Continuation::Future` with
  `AfterForcing::Reexecute`, which records that activation's value and runs
  the caller's instruction again instead of the next one (`vm::reexecute`);
  the second time the future is resolved. This rests on an invariant written
  on `builtins::execute` and checked leaf by leaf: **a leaf that declines or
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
  (`at:put:`'s element, `value:`'s arguments, `sendTo:`'s receiver,
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
its place. An unresolved one — of an abandoned clause, or a
`Future computing:` nothing has touched yet — is an ordinary record of its
class with its closure, and the
decoder refuses the class's tag wherever a graph names a class, as it
refuses `Activation`'s ([image-format.md](image-format.md), "Futures in a
graph").

**What pins it.** Under `goldens/language/slot/simultaneous/` (the temporaries' under
`goldens/language/method/`):
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
(`tests/opcode_tags.rs`). Three of the four `goldens/tool/bench/` fixtures are
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
`MixinApplicationClause` each — its message (a `ClauseMessage`, the
selector and whether it has arguments, the same record `Mixin::superclass`
holds for the superclass clause) and that message's compiled body. Type
arguments on a clause's receiver (`<: Collection[E]`) are kept in the syntax
tree and ignored, as they are on a superclass clause
([frontend.md](frontend.md), "What is kept but ignored, and what a reprint
drops").

**Each clause adds one class level between the declaration and its
superclass**, lowest first, so the last clause's mixin is the nearest to `C`.
§6.2 gives the level four properties, and the level has each one:

- **Its mixin is the named class's mixin** — that class's own declaration, not
  its superclass chain, which is what §6.2's "a class *C* whose mixin *M*"
  applies.
- **Its superclass is the level below**: `S` for the first clause, the
  previous level for each later one.
- **Its enclosing object is the named class's** — "The enclosing object of *I*
  is the enclosing object of *C*" — **and so is its declaring class.** A
  method of the applied mixin was written where the named class is declared,
  so `outer` inside it reaches the mixin's own enclosing objects, never the
  applying declaration's
  (`goldens/language/class/mixin/mixin_outer_reaches_the_mixins_enclosing_object.ns`).
- **Its primary factory selector is the clause's selector** — "Class *I* has a
  primary factory method (6) with the same selector as *m*" — which must name
  the named class's primary factory: a clause sending anything else is §6.1's
  run-time error, raised when `C` is instantiated
  (`goldens/language/class/mixin/mixin_clause_naming_the_wrong_factory_fails.ns`). A clause
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
(`Mutator::new_applied_level`) whose applied-clause field names the
applying declaration's mixin id and the clause's position in it, packed into
one `SmallInteger` with the builtin the clause names ("The object layout"
below), rather than carrying a copy, because two clauses of one declaration
can be identical and the image names a level's clause by row and position.
A superclass clause of more than 2^23 `<:` clauses is a resolve error, since
the position has 23 bits.

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
applied (`goldens/language/class/mixin/mixin_initializer_order.ns`), and so are none of its
own `<:` clauses
(`goldens/language/class/mixin/mixin_applied_mixins_own_clauses_are_not_applied.ns`).

**A declaration without `<:` takes the path it took before**, plus one test
per climbed level, per descent and per nested-class creation; the `<:`
machinery lives in cold, out-of-line helpers. What that costs is measured in
[execution.md](execution.md), "What mixin application cost". The image
carries the clauses on each row and the levels as class records
([image-format.md](image-format.md), "Sections"). The goldens are under
`goldens/language/class/mixin/`, with §6.2's own example in
`goldens/language/class/mixin/spec_6_2_a_mixin_application_derives_a_class.ns`.

**At a module's root, `<: Value` is allowed as a rule: it is how a module
declares itself a value.** `class M … = Object <: Value (…)` makes the
module's class inherit `Value`, which is §3.1.1's condition for its instances
to be values, and it is the form §3.1.1's other conditions need — a value
class's enclosing objects must be values, and the outermost one is the module
— and the one SOMns's `Kernel.ns` writes. It is a sanctioned form, not a side
effect of how a root clause is resolved
(`goldens/language/class/mixin/mixin_module_declares_itself_a_value.ns`).

**The root rule: only `Object`, `Value` or `Object <: Value`.** A module's
root class may write every combination of `Object` and `Value` that works
(decided with the user on 2026-09-30), and exactly these seven root clauses:
none (which is `Object`), `Object`, `Object new`, `Value`, `Value new`,
`Object <: Value` and `Object new <: Value`. Everything after the header's
`=` is held to the rule; the primary factory before it (`new`,
`packageUsing: manifest`, `usingPlatform: platform`, …) is not. Everything
else is a root defect: any prefix, any other name — the other builtins,
`Top` and a name nothing declares included — any message but the superclass
clause's `new` (`Object foo`, `Object new: 3`,
`goldens/language/module/root_superclass_sending_other_than_new_is_refused.ns`), any
other `<:` clause, a message sent to the `<:` clause's `Value`, and a second
`<:` clause. `Value <: Value` and `Value new <: Value` stay refused:
applying `Value` over `Value` adds nothing. A root class has no
enclosing object to evaluate a receiver in: §6.1 evaluates its clause "as if
it was a top level expression", where it "will almost certainly fail", so the
only classes a root can reach are the ones the rule names. The corpus has
such roots — `Newspeak2SqueakCompilation`'s `= NewspeakCompilation
usingPlatform: platform`, `UISessionManager`'s `= NewspeakObject new` — which
the Squeak-hosted implementation accepts; none is on `corpus/runnable.toml`.

**Every root defect is a resolve error** (decided with the user on
2026-09-30), for every module root, the shipped modules' included, before
anything runs: `resolver::root_clause_defect` classifies the root's clauses
and the resolver reports the first defect at the offending clause's span, so
the editor underlines exactly that clause. The specification would leave the
failure to evaluation; checking it statically is a recorded deviation
([README.md](README.md), "Deviations from the language specification"),
taken because a module root is not an ordinary class and a program must not
run when its artifact cannot be built — deferring it to instantiation let a
project module nothing instantiated run under `run` while `build` refused it.
`run` and `build` print it alike, as "resolve error: a module's root class
may inherit only Object or Value, or write Object <: Value, and its
superclass clause names `…`" (or "its superclass clause sends `s` to `…`,
not new", "its mixin application clause *n* names `…`", "its mixin
application clause *n* sends `s` to Value", "it applies Value to Value"). The
resolver binds none of a root clause's receivers (`resolver/mod.rs`); the
rule decides them, and a root's clause receiver is never run. Nested
declarations keep their own resolve errors.

**The instantiation-time refusal stays, for what does not come through the
resolver.** `Mutator::build_root_mixin` records the same verdict on the root
mixin (`Mixin::root_clause_defect`); `load_root_class` builds a defective
root over `Object` with no level, so `load_module` stays total; and
`apply_mixin` folds the defect into `instantiation_refusal`, inherited by
every class built on the root, refused with "cannot instantiate `M`:" and the
same reason. From source this is unreachable; it is reached only by a
`ResolvedModule` a host assembles past `resolve`, whose fields are public
(`tests/value_conditions.rs`,
`a_defective_root_built_past_the_resolver_refuses_only_an_instance`). **The
encoder refuses a module with a root defect**, in `image::package` and
`image::snapshot` alike, with "image encoding cannot serialize `M`:" and the
same reason, so the format needs no wire field and no tag for one; the
decoder's root-section reader refuses what the encoder cannot write
([image-format.md](image-format.md), "Verification"), so a crafted artifact,
which is untrusted input, cannot bring the rule back.
`goldens/language/module/root_*` pin one defect each.

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
refuses every instance that is not one. They are builtins
(`BuiltinClass::Behavior`, `Class`, `Metaclass`), and an image names them
by their declarations in `Kernel.ns` like every shipped class
([image-format.md](image-format.md), "Shipped names").

**Where a class object's state lives.** In the class object's own eight
fields, which only the VM reads ("The object layout" below): its
application, enclosing object, declaring class and superclass, and for a
metaclass `this_class`, the link back to its class. The three kernel classes
declare no slot for any of it and carry behaviour only, as SOMns's do
(`Class = Object ()`, `Metaclass = Class ()()` in
`prior-art/somns/core-lib/Kernel.ns`, the state in the Java `SClass`) and as
`KernelForJS.ns`'s do. Primordial Soup instead makes a class object an
ordinary instance whose first six slots `Behavior` declares and the VM knows
by position (`KernelForPrimordialSoup.ns`). The fields here sit by position
too, but in a format of their own that no Newspeak declaration names, so no
slot accessor reaches them, the image carries a class object as a class
record rather than as an instance, and whether an object is a class is read
from its header alone. §3.2 fixes the relations — instance-of, subclass-of,
flatness — and says nothing about how a class object is stored.

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
(`goldens/language/class/class-side/class_side_is_not_inherited_by_a_subclass_with_none_of_its_own.ns`).
§5.10 makes a `super` send's S the superclass of the activation's current
class, which for a class-side method is `Class`: a class-side
`super printString` answers `Class>>printString`, the class's name
(`goldens/language/class/class-side/class_side_super_reaches_class.ns`), and a
class-side `super new` does not instantiate, because nothing above the
metaclass declares a factory.

**Every class object's class is its metaclass, and every metaclass's class is
`Metaclass`.** `attach_metaclass` sets both headers: the class object becomes
an instance of its metaclass, and the metaclass an instance of `Metaclass`.
`Metaclass` is a kernel class with a metaclass of its own, whose class is
`Metaclass` again, which closes the loop without a regress
(`goldens/language/class/class-side/the_class_of_a_class_is_its_metaclass.ns`
for the one level a program sees, a class side's `self class`;
`crates/ns-runtime/tests/representation.rs`,
`the_class_of_a_class_is_its_metaclass_and_every_metaclass_is_a_metaclass`,
for the chain, which only the host reads, since `class` is protected). There is no separate
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

An object's class word is never `nil`, because the builtin
`Object` class exists from the first moment of the bootstrap and serves as
every provisional header. The image decoder relies on that the same way: it
allocates every record but a frame's as a placeholder of the size its first
pass read, pointing at the builtin `Object`, before it reads the record that
says what the class really is, and filling a class record in place
overwrites that with the record's metaclass, as it overwrites every other
placeholder field ([image-format.md](image-format.md), "Two passes").

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

**Dispatch is uniform.** A class object's dispatch id is its class's instances
id — its metaclass's — the same rule as for every other object; there is no
class-side id and no class-side lookup chain, and the inline caches guard on
the header id as they do for an instance ([dispatch.md](dispatch.md), "The
dispatch id"). A class side's own `doesNotUnderstand:` handles every send its
class object does not understand, since §5.5's handler is "defined for the
class of the receiver" and that class is the metaclass
(`goldens/language/class/class-side/a_class_side_does_not_understand_handles_its_class_objects_sends.ns`).

**The protocol is minimal, as Primordial Soup's is.** Reflection is a
mirror's job, which the reflection step builds; a class does not answer
`superclass`, `selectors` or its methods at the base level, so holding a class
grants no reflective capability. `Class>>name` answers the mixin's name
through the leaf `className:ifFail:` and `Class>>printString` answers `name`;
`Metaclass>>name` answers `thisClass name, ' class'`, reading `this_class`
through the leaf `metaclassThisClass:ifFail:`, and `Metaclass>>printString`
answers `name` (`builtins/behavior.rs`;
`goldens/language/class/class-side/a_class_and_its_metaclass_print_their_names.ns`:
`Foo`, `Foo class`; `representation.rs`'s
`a_class_and_its_metaclass_print_their_names_along_the_chain`: `Metaclass`,
`Metaclass class`). `Object` has **no
accessor** for `Behavior`, `Class` or `Metaclass`: none of the three
references declares one, §3.2's accessors are for the classes of the built-in
literals and the array factories, and the mirror code that will need the
names lives in the kernel or the platform, where it names them directly.
`interpreter/kernel.rs`'s accessor-set test pins the set.

**Instantiation is refused.** The three are reachable anyway — a class
side's `self class` is a metaclass, a host can hand a program any of them,
and road step 5's mirrors will reach every one — so refusing to
instantiate them is needed whatever is nameable. `Mutator::apply_mixin` folds
`InstantiationRefusal::ClassObject` into the `instantiation_refusal` of
`Behavior`'s application — recognized by the mixin the kernel phase records
as `Mutator.behavior_mixin` before `Behavior` is filled — and every
application below it inherits the refusal from its superclass, so
`decide_instantiation` refuses `Class`, `Metaclass`, every metaclass and any
subclass of them a program manages to declare with an ordinary Newspeak error
under its factory selector: "cannot instantiate `C`: a class object is made
only by evaluating its declaration" (`interpreter/instantiate.rs`;
`representation.rs`'s `a_class_object_cannot_be_made_by_instantiation`, which
sends `Metaclass` `new` from the host). A metaclass itself declares no
factory, so `Foo metaclass new`, a class-side `metaclass` answering `self
class`, is not understood
(`goldens/language/class/class-side/a_metaclass_does_not_understand_new.ns`).

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
  (`goldens/library/value/a_value_modules_nested_class_is_a_value.ns`,
  `goldens/library/value/a_values_slot_refuses_a_class_that_is_no_value.ns`).
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
  method, and costs no activation, so `Value>>valueEquals:`'s comparison of
  `self class` with the other object's class, read through the mirror,
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
- **The metaclass of a shipped class or level**, which is no record, is
  named by its own shipped name (`CLASS_SHIPPED`, of kind metaclass or level
  metaclass), and its identity hash travels in the closing hash table
  (`HASHED_SHIPPED`), as the shipped classes' own do. A module's root class
  and its levels are records, so their metaclasses are records too.

The decoder applies each metaclass record over `Class` as `attach_metaclass`
does (`rebuild_metaclasses`) and holds each class record and its metaclass
record to each other both ways (`pair_metaclasses`, `MetaclassPairing`), which
keeps a metaclass's identity across a round trip: the metaclass a slot holds
is the one its class answers. `Behavior`, `Class` and `Metaclass` are
refused wherever a class is made from them — a superclass, a `<:` clause's
mixin — since a class over one of them could never be instantiated; as
values in the graph they are accepted, since they carry no authority, a
program still reaches a metaclass (a class side's `self class`) and a host
can hand a program any of them, which a narrower rule would refuse to
build
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
- `SourceOrigin` is attached once per module: `SourceOrigin::File(PathBuf)`
  for a source read from a file, `SourceOrigin::Embedded(String)` for one
  compiled into the binary, named by the file it was read from at build time
  (`Manifest.ns`), so that an image does not claim a path that exists nowhere
  at run time (`types.rs`). It is an enum specifically so a later milestone
  can add a variant for code entered live in the debugger without changing the
  type its call sites depend on.

## Handles into the object space

`Rc<RefCell<Object>>` was the Milestone 1 model, and an arena of boxed
blocks with 32-bit index handles the next. Both are gone: an object is its
words in one `Heap`'s object space, in memory the heap reserves and commits
through `crates/ns-memory` ("The object layout" below), and an
`ObjectReference` is the 64-bit byte offset of its header from the space's
base, private to the `heap` module. The representation, what it measured,
the root discipline with its lifetime brand, and the mark-compact collector
are designed in [memory.md](memory.md), "Checked roots and a collector"
onward, and the space in [memory.md](memory.md), "The object space".

**A handle is 64 bits wide**, so the space is bounded by its reservation
rather than by a handle's width. HotSpot's compressed oops store a reference
in 32 bits and so cap a heap at 32 GB; that saving exists because a Java
field of reference type holds nothing else. A Newspeak field holds any value
-- an integer, a float, an object -- so it is a full 64-bit word whatever
the handle's width, as in Spur. With the words in one space, every field,
element and register accessor reads through the object's own block, which
ends where its header's count says, not only through the space's bounds, so
no access through one object reaches the next object's header.

Two consequences belong here rather than there, because they are about this
document's subject.

**An activation is an object too, when something needs it to be.** A running
activation is an entry on the `Heap`'s activation stack, and it becomes a
first-class frame -- an object of the frame format in the same space,
addressed by a `FrameReference` with the same privacy as an
`ObjectReference` -- only when something reifies it: a block capturing it, or
the exception machinery naming it. So an activation nothing reifies costs no
allocation and leaves nothing behind, and one that is reified is stored the
way every other object is. See [execution.md](execution.md), "Activations".

## Planned: the road to a high-performance VM

Would change: the object representation becomes the one the reference engines
with a JIT share -- Cog's Spur, Strongtalk and HotSpot, with Primordial Soup
beside them -- adapted where Newspeak's nesting requires it, and the JIT tiers
follow on it. Decided with the user on 2026-09-29; the canonical order is
[OPEN-WORK.md](../OPEN-WORK.md), "The road to a high-performance VM". The
`Planned` sections below and three in [memory.md](memory.md) describe its
representation steps still to come. The road, each step landing green on its
own:

1. **The memory layer** -- a crate of its own that reserves one contiguous
   address range and commits it on demand ([memory.md](memory.md),
   "The memory layer").
2. **An eight-byte value**, on the arena of the time -- built: its first plan,
   `LargeInteger` ([core-library.md](core-library.md), "Large integers"),
   and its second, the tagged word ("Values and objects" above).
   - **After road step 2, the compact instruction encoding** -- built:
     eight-byte instructions designed against the one-word value
     ([execution.md](execution.md), "The compact instruction encoding").
3. **The object space** -- one header word, the class word, the fields
   inline, in two plans, both built: **3a, the representation** on the
   arena ("The object layout" below), and **3b, the space**
   ([memory.md](memory.md), "The object space").
4. **Generations** -- built ([memory.md](memory.md), "Generations").
5. **Mirrors and atomic installation**, built on `become:` as an eager bulk
   forward ("Planned: `become:` as an eager bulk forward" below;
   [liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors and
   MirrorBuilder"; decided with the user on 2026-10-05).
6. **Inline allocation in the interpreter** ([memory.md](memory.md),
   "Planned: inline allocation").
7. **The execution model for compiled code** ([execution.md](execution.md),
   "Planned: JIT tiers").
   - **Between road steps 7 and 8, real Newspeak bodies for `whileTrue:`,
     `whileFalse:` and `do:`** ([core-library.md](core-library.md), "Planned:
     real bodies for control flow and `do:`, before the baseline JIT").
8. **A baseline JIT**, and
9. **an optimizing JIT** ([execution.md](execution.md), "Planned: JIT tiers").

## The object layout

Every object is a run of words -- a header word, a class word, then its
fields -- in exactly the layout road step 3 settles. The header word plus
class word (option A), frames as heap objects and strings in two widths were
decided with the user on 2026-09-29; the rest is derived. Road step 3 is
built in two plans (decided with the user on 2026-10-03), as the one-word
value was built on the arena first, and both are built. **3a, the
representation**, fixed the final layout on the arena: every object kind is
a format, class objects, closures and reified frames are such objects, there
is no separate store for frames, and the image's frames are object records
([image-format.md](image-format.md), "Frames are object records"). **3b,
the space**, changed only where the words live: every object lies in the
space in `ns-memory`'s reservation, with byte-offset handles, bump
allocation and sliding compaction ([memory.md](memory.md), "The object
space"; "Handles into the object space" above), so a guard read is one load
from the space's base plus the handle.

```
[header word][class word][field 1][field 2] ...     8-byte aligned, at least 16 bytes
```

The block's layout is decided in `heap/object.rs` (the fixed fields of a
class object, a closure and a frame, the string packing, and
`pointer_ranges`); `Heap::allocate_block` is the one place a new object's
header is written, in place in the space; `heap/header.rs` holds `Header`
and `Format`.

**The header word**, after Spur's (`headerForSlots:format:classIndex:`) and
HotSpot's compact object headers (JEP 450, a product feature from JDK 25 as
JEP 519), which fit a class reference, the identity hash, the GC age and
the lock bits into one 64-bit word:

| Bits | Holds |
|---|---|
| 0-21 | the application id -- the inline caches' guard ([dispatch.md](dispatch.md), "The dispatch id"); 0 is `NONE`, 1 `VACANT`, 2 reserved (`FORWARDED`), and the counter mints from 3 |
| 22-26 | the format |
| 27-30 | the age: the scavenges a young object has survived, at most 15; 0 for a new object and meaningless for an old one ([memory.md](memory.md), "The age table and the tenuring threshold") |
| 31 | the remembered bit: set on an old object in the remembered set, never on a young one ([memory.md](memory.md), "The remembered set and its limit") |
| 32 | the negative bit of a `LargeInteger`, as Primordial Soup's `kNegativeBit` (`prior-art/primordialsoup/vm/object.h:48`) |
| 33 | the mark bit, set while a collection runs and cleared as compaction moves the object ([memory.md](memory.md), "Marking") |
| 34-55 | the identity hash, drawn lazily from the counter; 0 means none yet |
| 56-63 | the number of words after the header -- the class word and the fields; 255 means the real count is in an overflow word *before* the header |

The word count sits in the top byte, as Spur's does, so the overflow word
before a large object's header carries 255 in its top byte and the real
count in its low 56 bits, and a linear walk tells it from a header. The
object space is the first thing that walks the words linearly, and the
first user of the overflow word: an object of more than 254 words after its
header stores 255 there and takes the overflow word
([memory.md](memory.md), "The object space").

**The reserved guard values sit at the low end**, as Primordial Soup's
reserved class ids do (`kIllegalCid = 0`, `kForwardingCorpseCid = 1`,
`kFreeListElementCid = 2`, `kFirstLegalCid = 3`,
`prior-art/primordialsoup/vm/object.h:64-68`), and Spur keeps its own
puns below 32 (`isFreeObjectClassIndexPun` 0, `isForwardedObjectClassIndexPun`
8, `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:2815-2817`):
`DispatchId::NONE` is 0, `VACANT` 1 and `FORWARDED` 2, and the counter mints
from 3 below 2^22, answering `NONE` once it is exhausted (`mixin.rs`). So a
header word left zero by a missed initialization carries `NONE`, which is
never cached, rather than a guard some application was minted, and the
debug build's recomputation in `Heap::dispatch_id` reports it (decided with
the user on 2026-10-03). `FORWARDED` was reserved as the guard of a forwarder
that outlives a `become:`; with `become:` an eager bulk forward, no send ever
meets a forwarder ("Planned: `become:` as an eager bulk forward" below), so
nothing needs it as a guard, and whether it stays reserved is left to road
step 5's plan.

**The identity hash is 22 bits wide**, drawn from a counter that wraps
within them, as Strongtalk's wraps within 20 (`markOop.cpp`), and skips 0.
Hashes are therefore not unique once the counter has wrapped, which a hash
need not be; the decoder advances the counter past the largest hash an image
holds and refuses a hash of 2^22 or more (`IdentityHashTooWide`;
[image-format.md](image-format.md), "Identity hashes are preserved
verbatim").

**An application has two ids.** Its *guard id*, 22 bits wide, is the
inline caches' guard: it stands in object headers and in inline- and
global-cache entries -- and, from road step 5, in recorded guards (see
"Planned with road step 5: what a slot access checks" below) -- and nowhere
else. Running out of guard ids keeps the rule of
[dispatch.md](dispatch.md), "The dispatch id": an application without one
runs uncached. Its *full-width index*, an `ApplicationIndex` into the heap's
table of `MixinApplication`s, is what a class object names and what every
identity comparison of applications uses -- the application cache's key,
`behavior:sharesApplicationWith:` (`Kernel.ns`), which class-object equality
uses, and the global cache's `Lexical` key, which never records a `NONE`
guard ([dispatch.md](dispatch.md), "3b — The caches"). So an application
whose guard id is `NONE` stays reachable and distinct from every other.

The tables are `heap/tables.rs`'s, one set per heap because the objects
naming them are: a `MixinId` for a mixin, an `ApplicationIndex` for an
application and a `CodeId` for a compiled body, each registered on first
sight (`Heap::mixin_id`, `register_application`, `code_id`). They are
append-only. Each id has its own reuse rule, and the collector can tell
both of an application's, so reusing either will be safe: a guard id is
reclaimable once no object header, no inline- or global-cache entry and no
recorded guard names it; a full-width index once no class object, no
application-cache key or value and no other application's superclass link
names it. A mixin id is reclaimable once the `Mixin` itself is dropped --
its table entry freed, no declaration, enclosing mixin or code holding it --
and no application, no application-cache key or value and no class object's
applied-clause field names it. A class's change of application restamps
every instance's header by a heap walk (`Heap::class_set_application`)
before the old guard id can become reclaimable. None of it is reclaimed:
nothing drops a mixin before live editing, and the application cache, keyed
by the ids, stays strong; the reuse rules and a weak cache come with the
first edit (decided with the user on 2026-10-03), which is road step 5's
atomic install ([OPEN-WORK.md](../OPEN-WORK.md), "Step 5, mirrors and
atomic installation").

**The class word** is an ordinary stored word naming the object's class
object, written through `Heap::store` and read through `Heap::load` like a
field. An immediate has no header; its class and its guard come from tables
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

**Formats**, after Spur's five-bit field (`formatOf:`), with the codes
`Format` gives them:

| Code | Format | Objects |
|---|---|---|
| 0 | fixed pointer fields | ordinary instances; a `Fraction`'s two parts |
| 1 | indexable pointers | `Array` |
| 2 | frame | a reified frame: ten fixed fields, then its registers |
| 3 | closure | three fixed fields, then its copied values |
| 4 | 64-bit-indexed | a `LargeInteger`'s magnitude (its sign in the header's negative bit), a boxed `Float` |
| 5 | forwarded | a forwarder -- the scavenger's for a young object it has copied, and from road step 5 the bulk forward's ("Planned: `become:` as an eager bulk forward") -- whose class word holds the new location; each exists only while the scavenge or the forward that made it runs, so no send, accessor or reader outside the two ever meets one |
| 6 | free | a gap a collector leaves, walkable like an object; no object has it, since the sliding compaction and the scavenge leave no gap; a debug build writes it as the poison over the nursery a scavenge vacated, and `Heap::block` refuses it there ([memory.md](memory.md), "Finding a missed barrier: the scavenge stress mode") |
| 7 | class | a class object: eight fixed fields |
| 8-9 | 32-bit-indexed | a wide string or symbol |
| 16-23 | byte-indexed | a one-byte string or symbol |
| 24-31 | aligned bytes (`AlignedBytes`) | a `ByteArray`: three reserve words, then its bytes, which begin at the first 32-byte address in the block ([memory.md](memory.md), "Every mover realigns a `ByteArray`'s data") |

The byte, aligned-byte and 32-bit formats carry the number of unused units
at the end in their low bits, as Spur's do, so an exact length needs no
length field. An aligned-byte object is its own format rather than a byte
one because every reader that meets the byte format takes it for a string.
A
code no format uses reads as free, so a corrupt word walks as a gap rather
than panicking. A forwarder needs no field of its own: the class word, which
every object has -- a header and a class word are the 16-byte minimum --
takes the target, as Spur's forwarders take their first slot.

**A class object has a format of its own**, where Spur's and Primordial
Soup's class objects are ordinary pointer objects, because whether an object
is a class is asked of untrusted objects: the image decoder's verification
and its lookups ask it of every object a record names
([image-format.md](image-format.md), "Verification"). With a format of its
own, `Heap::is_class` reads the object's header alone, rather than following
its class word to the class and that class's to the metaclass, through
objects a crafted image may not have made what they claim to be.

**The format decides what the collector traces**: `object.rs`'s
`pointer_ranges` names, for marking and for `trace_block`'s forwarding
alike, the class word and, by the header's format alone, every field of
the two pointer formats, a class object's
object fields, a closure's captured frame and copied values, and a frame's
fixed fields and, once it is no longer married, its registers -- never the
digits of a `LargeInteger`, a `Float`'s bits, a string's code points or a
`ByteArray`'s bytes, which would read as handles ([memory.md](memory.md),
"What the collector traces").

**Class objects, closures and frames are objects.** A class object keeps in
eight fixed fields (`object.rs`'s `class_field`) everything that holds per
class object: its enclosing object, declaring class, superclass, for a
metaclass the class it describes (`this_class`), its builtin tag (a
`SmallInteger`, the class's position in `BuiltinClass::ALL`, or `nil`; `act`
re-reads it on every send, [dispatch.md](dispatch.md), "Primitive methods"),
its value-class refusal (`nil`, or a two-element `Array` the VM alone reads:
the offending enclosing object and the name of the level at fault as a
`Symbol`, or `nil` for the class's own enclosing object), for a level a `<:`
clause added the clause that added it (the applying declaration's mixin id,
the clause index and the named builtin tag packed into one `SmallInteger`, or
`nil`, since an `Rc<Mixin>` cannot live in the words), and its application
as a `SmallInteger` full-width index, separate from the 22-bit guard id in
instances' headers; the behaviour stays Rust. A class object is 80 bytes
with its header and class word. The builtin tag, the applied clause and the
application are numbers, the other five fields objects or `nil`, and the
tracer sees only those. The applied clause packs the named builtin into a
6-bit field, as its position in `BuiltinClass::ALL` plus one (0 for none),
which caps the builtins: `mixin.rs`'s compile-time assertion keeps
`BuiltinClass::ALL` shorter than 63, so there are at most 62.

The refusal is held as heap data rather than as an index into a Rust-side
table of refusals, because class objects are made at run time, one per
enclosing instance, so a table that only grows would grow with every class
object a loop makes, live editing or not, and one the collector reclaimed
would be a weak table of its own. Its message is rendered by
`Heap::describe` when `new` refuses, not at creation, and reads the
same as a message rendered at creation would: the offender is never a value,
and what `describe` says of a non-value -- "an instance of X", "an Array of
n", "a Closure", "the class X" -- does not change while it lives. Holding the
offender keeps nothing alive: it is the enclosing object of a level of this
class object's own superclass chain. An image carries no refusal; the
decoder recomputes it (decided with the user on 2026-10-03; "A value
object's four conditions" above).

The applied clause packs the mixin id into bits 0-31, the clause index into
bits 32-54 and the named builtin's tag plus one into bits 55-60, 0 meaning
none, which fills the `SmallInteger`'s 61 bits; the 61-bit pattern is
sign-extended, so a tag that sets bit 60 packs into a negative
`SmallInteger` (`AppliedClause::pack`, `unpack`, `mixin.rs`). A superclass
clause of more than 2^23 `<:` clauses is therefore refused by the resolver,
as an ordinary resolve error naming the limit (`CLAUSE_INDEX_LIMIT`), and an
image whose clause index reaches 2^23 by the decoder
(`ClauseIndexTooLarge`). This is a limit a valid program can reach, against
the rule that no valid construct is carved out because it is rare
([README.md](README.md), "Principles"), taken deliberately (decided with
the user on 2026-10-03): no source near eight million clauses in one
superclass clause is in prospect, and a second field would cost every class
object eight bytes for it.

The application cache is keyed by the mixin's id and the superclass
application's full-width index rather than by `Rc` addresses, which lets it
become weak ([memory.md](memory.md), "Weak references") when live editing
needs it to. A closure keeps its code id, captured frame and feedback base in
three fixed fields (`closure_field`) and its copied values inline; a code id
indexes the heap's table of compiled code, append-only, its entries never
freed or reused, since compiled code is shared by mixins and reclaiming it
belongs with live editing. Each entry also records the code's owning
`MixinMethod`, where it has one, so a method frame's code id names its
method too -- the declaration the encoder writes and `frame_body` reads.

**A reified frame** is an object of the frame format whose class is the
kernel-private `Activation`. Ten fixed fields (`frame_field`) keep its
sender, home, receiver, current class, program counter, code id (`nil` for a
top-level frame), kind (the kind's wire tag), the kind's operand -- the
closure of a block frame, or the class of an initializer, superclass-clause,
superclass-message or mixin-application-message frame, a handle the trace
sees -- `uncontinuable`, and the index of the stack entry it is married to,
`nil` once divorced; its registers follow inline. The `Activation` object
the exception machinery holds is this frame object itself, so the same
activation answers the same object every time; the running activations stay
on the activation stack until road step 7. A frame object is allocated at
`reify` with its code's full register count, whatever the code; whether the
divorce copies the registers into it is decided per activation from the
block's `outer_reach` ([execution.md](execution.md), "Closure conversion").
So no frame object ever grows, and suspension, the debugger and
edit-and-continue find room for the registers in every reified frame.
Edit-and-continue adding locals will divorce the affected activation into a
newly allocated, larger frame object -- allocated old and born remembered by
the rule in [memory.md](memory.md), "Generations" -- the VM
rewriting every reference to the old frame by a walk of every root the
collector traces (`trace.rs`) and every heap object, the same walk as the
bulk forward of road step 5 ("Planned: `become:` as an eager bulk forward"
below); execution continues from the divorced frame, which needs a
divorced frame put back on the stack, the piece suspend and resume need too
([execution.md](execution.md), "How this meets the liveness requirements").
The cost is a few more words per reified frame, and `bench_block.ns`, which
reifies a frame per iteration, is the workload that shows it. While a frame
is married its registers live on the stack and the trace skips the object's
inline copies. `FrameReference` is the type of a handle to a frame object,
so an `ActivationReference` is still never stored, and a frame has no store,
forwarding table or register vector of its own.

**Strings have two widths.** A string holds one byte per code point while
every code point fits Latin-1, 32 bits per code point otherwise -- the
byte-indexed and 32-bit-indexed formats, eight or two units to a word, least
significant first, the last word's unused units zero, so that equal texts
have equal words (`object.rs`'s `pack_string` and `pack_code_points`, the
writers, and `PackedString`, the one reader of the layout). Either way the
elements are code
points, so `size` and `at:` are one access, read in place. §5.1.5 makes a
string's elements "integers representing the encoding of the string in
Unicode normal form C" and leaves the representation to the implementation.
This is Spur's `ByteString` and `WideString`; Java's compact strings (JEP
254) and V8 do the same with a UTF-16 wide form, and Python (PEP 393) adds a
middle width of two bytes, which can be added here later behind the same
interface. Primordial Soup stores UTF-8 bytes and answers a byte from `at:`
(`String_at`, `vm/primitives.cc`), which would change what `size` answers
for any text beyond ASCII. Source, the transcript and the image are UTF-8,
converted at the boundary; `CodePoints` (`code_points.rs`) is the Rust-side
form a string is copied out to and built from. A grapheme cluster -- one
emoji with a skin-tone modifier is two code points -- is never an element;
counting or iterating them is a library protocol above the string, since
the segmentation rules change with every Unicode version and allow no
constant-time indexing ([core-library.md](core-library.md), "Strings are
code points in two widths").

`Symbol` is a subclass of `String`, as in every reference kernel: a symbol's
words are a string's, and only its class tells it from one.
`Mutator::new_symbol` is the only way to make one: instantiating `Symbol` is
refused, and `String new` answers an empty one-byte string
(`goldens/library/string/symbol_new_is_refused.ns`,
`string_new_answers_an_empty_string.ns`). `=` is class-tight both ways:
`'foo' = #foo` and `#foo = 'foo'` both answer `false`, since `Symbol>>=` is
identity and `String>>=` answers `false` for a `Symbol` argument
(`a_symbol_is_a_string.ns`).

**What an accessor lends.** A string's units are read in place, and a
`LargeInteger`'s digits are lent in place to `=`, `hash`, the comparisons and
the sign tests; arithmetic still copies its operands' digits out
([core-library.md](core-library.md), "Large integers"). Every field,
element and register accessor indexes the object's own block, so no access
through one object -- a crafted image's by-reference register write
included -- can reach another object's words, and an out-of-range index that
a valid program produces raises an ordinary Newspeak error, never a panic.

### Planned with road step 5: what a slot access checks

Would change: a slot access that is not guarded by the receiver's own header
checks a recorded guard. Such an access reads or writes a field at an index
decided without looking at the receiver: the own-slot instructions
`InitializeSlot`, `InitializeSlotProvisionally`, `LoadOwnSlot` and
`ReturnSlotIfFilled`, which address `slot_base + own_index` in `self` with
no send (`bytecode.rs`, `vm.rs`), and a slot accessor acted on through
`Chain::Private` or `Chain::Super` -- a private lexical send, one to an
enclosing object included, resolves on the lexical class to a field index
(`resolve_lexical`, `interpreter/resolve.rs`; `decide_slot_accessor`), and
a super send on the superclass. Each will compare the receiver's header
guard with an expected guard, a *recorded guard*: for the own-slot
instructions, the one the activation recorded from the receiver's header
when it was entered -- not the running level's own application, which
differs from the receiver's on every inherited level; for a private or super
slot accessor, the one the cache entry recorded when it was filled. One
compare, the hot path. On a mismatch, or when either guard is `NONE`, a cold
path checks, through the receiver's class word and the full-width index,
that the receiver's application inherits the accessing level: if it does --
as after an edit that changed only methods and restamped the header -- the
access proceeds; if not, it raises an ordinary Newspeak error. The reference
lowering, which has no cache, always takes the cold check. The forward is
eager ("Planned: `become:` as an eager bulk forward" below): when an atomic
install migrates the instances of a class whose layout changed, the walk
rewrites the receiver of every running activation on one of them to its
new-shape replacement, while the activation goes on running the method or
initializer it was entered in, compiled against the old layout. So the rest
of such an activation cannot write the old layout into an object of another
shape, and a receiver forwarded to an object of another class never
receives a write meant for the class it replaced. Recorded guards join the
guard id's reuse rule ("An application has two ids" above).

The recorded guard and this check come with road step 5, not with road
step 3: the atomic install's bulk forward is the first way a running
activation's receiver can change shape, and edit-and-continue the next --
today the decoder's in-place fills touch only its own
placeholders and `Heap::class_set_application` runs only while the kernel
boots -- so before it there is nothing for them to catch. The recorded
guard will live in the activation stack's entry alone; a reified frame has
no field for it. An activation resumed from a frame (road step 7) starts
with `NONE`, so its first own-slot access takes the cold check, and a cold
check that passes records the receiver's current header guard, so the
accesses after it take the hot path again. Reading the guard afresh from the
header on resume instead would defeat the check: after a bulk forward it
would read the replacing object's own guard, which always matches that
object.
One cold check per resumed activation buys eight bytes in every reified
frame, which `bench_block.ns` reifies once per iteration (decided with the
user on 2026-10-03).

## Planned: `become:` as an eager bulk forward

Would change: `become:` exists as one bulk, one-way forward of an array of
old objects to an array of new ones, done eagerly, as Primordial Soup does,
and only mirrors reach it: it is a leaf on the VM mirror, and its one
caller is the shipped mirrors module's atomic install. Road step 5, whose
consumer is that install ([liveness-and-reflection.md](liveness-and-reflection.md),
"Mirrors and MirrorBuilder"). Eager forwarding and its reach only through
mirrors were decided with the user on 2026-10-05.

**The forward.** Primordial Soup's `Heap::BecomeForward`
(`prior-art/primordialsoup/vm/heap.cc:1072-1229`) validates both arrays,
turns each old object into a forwarding corpse after copying its identity
hash into the new one, then forwards the class ids (`ForwardClassIds`), the
roots (`ForwardRoots`) and the whole heap (`ForwardHeap`), rebuilding the
remembered set as it goes; the interpreter clears its lookup cache in the
prologue every such pass runs (`GCPrologue`,
`prior-art/primordialsoup/vm/interpreter.cc:1906-1908`), and primitive 183,
`Heap_becomeForward` (`prior-art/primordialsoup/vm/primitives.cc:2271-2281`),
is its only entry. In ns-next the forward takes two arrays of equal size
and validates every pair before it changes anything ("What the forward
refuses" below); it copies each old object's identity hash into the new
one, so that an identity-keyed collection still finds what it holds; it
turns each old object into a forwarder -- the forwarded format, the new
object in its class word, the scavenger's forwarder ([memory.md](memory.md),
"Generations") -- and then one walk over every root the collector traces
(`trace.rs`) and every heap object rewrites every reference to an old object
into its new one, through the same knowledge of pointer words the collector
uses (`heap/object.rs`'s `pointer_ranges`), keeping the remembered set as
the collector's own rewrites do. Last, the inline caches are flushed, all of
them (`Interpreter::invalidate_caches`); scoping the invalidation to the
applications an install affects stays deferred
([liveness-and-reflection.md](liveness-and-reflection.md), "Caches and their
dependency keys"). The forwarders exist only while the forward runs, as the
scavenger's exist only while a scavenge does: once the walk ends nothing
names one, and they are garbage. The walk is the one the divorce of a live
frame for edit-and-continue needs ("The object layout" above), so the two
share one mechanism.

**What the eager forward spares.** Since no forwarder outlives the forward,
nothing outside it ever meets one. No send entry unforwards its receiver; a
super or private lexical send's cache hit needs no check of its receiver for
the forwarded format; no heap accessor has a cold path that follows a
forwarder; `==` compares two handles and nothing more; no leaf fails on a
forwarded argument to be retried after following it; no application id has
to be a forwarder's never-matching guard ("The reserved guard values sit at
the low end" above); and the image encoder, `Heap::describe` and the host
roots see only live objects. Spur's lazy forwarders, which stay in the heap
after `become:with:twoWay:copyHash:`, pay for every one of these: a read
barrier on send through the forwarder's class index, an explicit one in
super sends (`ensureReceiverUnforwarded`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:6637-6643`: "To
maintain the invariant that all receivers are unforwarded we need an
explicit read barrier in the super send cases"), and primitives that
unforward their arguments on failure. Each reader that forgets its check is
silent wrong output at exit status 0, and a JIT would carry every check
into generated code.

**Why eager.** The only consumer, the atomic install, finds the instances it
migrates and the class objects it updates by a walk of the heap anyway --
there is no registry of subclasses or instances -- so one more walk per
install costs little, while lazy forwarders cost every send, every `==`,
every leaf and later the JIT a check. Spur chose lazy forwarding because
Squeak uses `become:` often, on few objects; Strongtalk is eager too, its
`oopPrimitives::become` two-way and deoptimizing every process first
(`prior-art/strongtalk/vm/prims/oop_prims.cpp:60-77`). Lazy forwarding can
be added later behind the same bulk-forward seam, if a measurement shows
`become:` frequent ([OPEN-WORK.md](../OPEN-WORK.md), "Open design
questions"). The cost accepted is that a change of one object's class to a
class of another shape would pay a whole walk for one object. The canonical
Newspeak's `ObjectMirror>>setClass:` forwards nothing: it calls
`changeClassOf:to:`, which rewrites the class word in place
(`prior-art/newspeak/MirrorsForSqueak.ns:1364-1366`), and Primordial Soup
offers no `setClass:` at all, its tests of it empty
(`prior-art/primordialsoup/newspeak/MirrorTesting.ns:772-775`). A
shape-changing `setClass:` is deferred beyond road step 5.

**Only mirrors reach it.** No program sends `become:`: `Object` declares no
such method, and the forward is a leaf on the VM mirror, which only the
shipped core-library modules hold (`CLAUDE.md`, "The VM mirror is a
capability"). The specification has no `become:`; §8.2 (`newspeak-spec.pdf`
p. 47) makes mirrors the capabilities for reflection and asks of them that
"multiple reflective changes to the program can be applied simultaneously
and atomically", that "it is possible to change the class of an individual
object", and access to the current activation -- reflective edits, which
are what the forward serves, and nothing a program does to its own objects.
Primordial Soup sends its forward only privately, from its MirrorBuilder's
installation of an edit (`installAll`,
`prior-art/primordialsoup/newspeak/Mirrors.ns:309-326`), and SOMns, a
Newspeak with values and actors, has no `become:` at all.

**What the forward refuses.** It refuses an immediate on either side, which
has no object to forward, and a split or a chain -- an old object named
twice, or an object both forwarded and a target -- as Primordial Soup's
validation does (`heap.cc:1078-1113`), which accepts a merge, several old
objects forwarded to one new one. Beyond the leaf's own validation, what is
forwarded is decided by its one caller: the atomic install forwards only
the instances it migrates, each to its new-shape replacement, and those are
instances of the classes the edit names, never of a shipped mixin, since
the mirrors refuse every edit to one
([liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors and
MirrorBuilder"). So no class object, closure, frame object or symbol, and
no string, large integer or boxed float, is ever forwarded, and no object
on a path from shipped code to the VM mirror: every instance a platform
path names is an instance of a shipped mixin, and so is every kernel-made
object a program can reach, a VM error's `Error` or `MessageNotUnderstood`
and its `Message` among them. A kernel method whose receiver is an instance
of a user subclass the install migrated fails the slot-access check
("Planned with road step 5: what a slot access checks" above) rather than
writing the old layout into the new shape. Whether the leaf also checks the
shipped mixins itself, as a second lock behind the mirrors' refusal, is
left to road step 5's plan.

**Everything stays editable.** A class object keeps its identity when its
class is edited: the VM writes its fields -- a new application id when
methods or slots change, a new superclass, a new value refusal -- and every
reference to it stays valid, so the install forwards no class object and
no mixin, where Primordial Soup's forwards its old mixins and classes to
the new ones after the instances (`Mirrors.ns:320-326`). When the slots
change, the instances migrate: the install allocates each in the new shape,
copies its slots by name, leaves a slot the edit added `nil` without running
its initializer, keeps its identity hash, and sends nothing to a user object
while it does; then one bulk forward replaces all of them at once. A live
frame gains locals by being divorced into a newly allocated, larger frame
object, allocated old and born remembered, through the same walk, and
execution continues from the divorced frame ("The object layout" above).
Closures keep their old code, as in Smalltalk, and new ones get the new
code. A value class's instances migrate the same way, inside the VM, which
no program can observe, since a value has no identity.

## Decisions

- **Three layers rather than one**, because §6 requires a distinct class object
  per enclosing instance while §6.2 requires behaviour to be shared per mixin.
  A single structure for both cannot do both.
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
  `Object <: Value`** (decided with the user on 2026-09-23), **and every other
  root is a resolve error** (decided with the user on 2026-09-30), so a
  program whose artifact `build` could never write never runs under `run`
  either ("Mixin application", "The root rule").
- **§3.2's class hierarchy is reified as `Behavior`, `Class` and
  `Metaclass` in `Kernel.ns`** (nine decisions taken with the user on
  2026-09-25, built by
  [its plan](../superpowers/plans/2026-09-25-1331-ns-next-class-behavior-metaclass.md);
  "Class sides and metaclasses" above). A class object is a value exactly
  when its enclosing object is, a metaclass when its class is. A class
  object's state stays in the VM-read fields of the class object; the
  kernel classes carry behaviour only. Metaclasses are built eagerly with
  their class, and a metaclass's `this_class` field links it back to its
  class. The hierarchy is
  `Object` → `Behavior` → {`Class`, `Metaclass`}, none of them below
  `Value`. The protocol is minimal — `name` and `printString` on `Class`
  and `Metaclass` — `Object` declares no accessor for any of the three, and
  instantiating anything below `Behavior` is refused. A value class
  object's equality is derived from its state — its application, its
  enclosing object, its superclass; a metaclass's from its class — by
  extending `builtins::value`'s walk with class objects as a node kind, and
  `Behavior` declares `=`, `==` and `hash`. Its hash is derived from its
  mixin's name alone. In the image, a class record's metaclass is a record
  of its own (`OBJECT_METACLASS`), a shipped class's is named by its shipped
  name (`CLASS_SHIPPED`, `HASHED_SHIPPED`), and the amplification bound does
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
  future is a kernel-private subclass of the public `Proxy`, after Primordial
  Soup's, made by a clause or by `Future computing:` and forced
  as a trampoline step, and an instruction or a leaf that meets an
  unresolved one forces it and runs again, which rests on a leaf that
  declines or raises having changed nothing.
- **Identity hashes come from a counter, not from addresses, and are assigned on
  first request**, so identity survives a moving collector and a snapshot, and
  allocation order is unobservable.
- **The value is one tagged 64-bit word, with a 61-bit `SmallInteger` over
  the `LargeInteger` that makes integers unbounded** (decided with the user
  on 2026-09-29; "Values and objects" above; [core-library.md](core-library.md),
  "Large integers"). Spur's tags and the immediate floats in Spur's range
  are derived. It needed no object space: it was built when a handle was a
  32-bit arena index, and the 64-bit byte offset of road step 3b fits the
  word as well, a multiple of 8 under the object tag.
- **An object is a header word, a class word and its fields** (decided with
  the user on 2026-09-29, built by road step 3a on 2026-10-03; "The object
  layout" above). The header's guard is the
  application id, because a class-index guard would make nested classes
  megamorphic; the class object gets a word of its own rather than a class
  index packed beside the guard, so that `class` and `outer` stay one load.
  A class object is a format of its own, so that whether an untrusted
  object is a class is read from its header alone.
- **Strings store code points in one of two widths**, Latin-1 or 32
  bits, with UTF-8 only at the boundaries (decided with the user on
  2026-09-29, built on 2026-10-01, as the byte- and 32-bit-indexed formats
  on 2026-10-03; "The object layout" above).
  `Symbol` is a subclass of `String`, refused by `new`, and `=` stays
  class-tight both ways.
- **`become:` is an eager bulk forward that only mirrors reach** (decided
  with the user on 2026-10-05; "Planned: `become:` as an eager bulk forward"
  above). One walk of the roots and the heap per forward, as Primordial
  Soup's, rather than Spur's lazy forwarders, because its one consumer, the
  atomic install, walks the heap anyway, while a forwarder that outlives the
  forward would cost every send, `==`, leaf and generated code a check. No
  program sends `become:`: the specification has none, and §8.2 makes
  reflective change a mirror's. Road step 5 is mirrors and atomic
  installation, the forward its building block, so the step ends with its
  consumer. Editing a value class migrates its instances inside the VM.
- **A handle is the 64-bit byte offset of the object's header in the
  space**, never 0, so an optional handle is one word (decided with the
  user on 2026-09-29, built by road step 3b on 2026-10-03; "Handles into the
  object space" above): no handle width caps the heap, and base plus handle
  is the address generated code will use.
- **Reified frames are heap objects**, of the frame format in the same
  space, and there is no separate store for frames (decided with the user
  on 2026-09-29, built on 2026-10-03). Closures and class objects are
  objects of their own formats too, a derived part of the same layout; the
  kernel's `Activation` object is the frame object itself.
- **Road step 3 is built as two plans, the representation (3a) on the
  arena and then the space (3b), both built**, and five details of the
  layout are settled (decided with the user on 2026-10-03; "The object
  layout" above): the recorded guard and the slot-access check come with
  road step 5, the guard in the activation stack's entry
  alone and a resumed activation starting with `NONE`; the applied clause
  is one `SmallInteger` of mixin id, clause index and builtin tag, 32, 23
  and 6 bits, the clause count's limit reported as a resolve or decode
  error -- a limit taken knowingly against the rarity rule; the reserved
  guards are `NONE` 0, `VACANT` 1 and `FORWARDED` 2; a value-class
  refusal is heap data, not a Rust-side table; and the ids are minted per
  heap, key the application cache and are reclaimed by nothing until live
  editing.
