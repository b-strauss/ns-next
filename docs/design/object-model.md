# Object model — values, objects, mixins, classes

> **Status:** built, a value as one tagged word, the object layout (road
> step 3a, "The object layout") and the object space with byte-offset
> handles (road step 3b, "Handles into the object space") included, and
> `become:` as an eager bulk forward behind one VM-mirror leaf and the
> recorded guards of a slot access (road step 5b1, "`become:` is an eager
> bulk forward", "What a slot access checks"), the atomic install that
> calls the forward and re-applies every class object of an edited mixin
> (road step 5b2a, "Everything stays editable") -- since the complete
> install over a new superclass too, a removed class living on obsolete --,
> and the reclamation of
> the per-heap ids -- every application, guard id, module tree of mixin
> ids, superseded version and code id nothing names any more, freed in a
> full collection and minted again smallest first, the application cache
> pruned by them (road step 5b2c, "An application has two ids"), inline
> allocation (road step 6, [memory.md](memory.md), "Inline allocation"),
> and `new` as compiled code: a factory body and per-level initializer
> bodies the compiler emits, reached through the inline caches like any
> send ("Instantiation"); the rest of the road to a high-performance VM
> -- the JIT tiers -- is planned ("Planned: the road to a
> high-performance VM").
> **Code:** `crates/ns-runtime/src/heap.rs` (the handle types and every read
> and write of one), `heap/value.rs` (`Value`, `ValueKind`, `Word`, the
> `SmallInteger` bounds and the arithmetic on tagged words),
> `heap/header.rs` (the header word, `Format`), `heap/object.rs` (the block
> of words an object is, the fixed fields of a class object, a closure and
> a frame, tracing by format), `heap/records.rs` (the seam's value types),
> `heap/tables.rs` (the per-heap tables of mixins, applications and code),
> `heap/ids.rs` and `heap/reclaim.rs` (their free rows and what a full
> collection reads to free them), `interpreter/reclaim.rs` (the
> reclamation's decisions),
> `mixin.rs`, `interpreter/instantiate.rs` (what the factory body's and
> the initializers' instructions decide), `compiler/mod.rs`
> (`factory_body`, the initializer bodies), `interpreter/nested.rs` (the
> nested-class accessor and `build_applied_levels`, §6.2's levels),
> `interpreter/sends.rs` (where a lexical walk and a `super` lookup
> start),
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

**A value is one tagged word, Spur's** (`r[vm.heap.value-tags]`). Its
low three bits say what the rest is:

| Low bits | Meaning |
|---|---|
| `000` | an object handle: the object's byte offset in the space, a multiple of 8, so that the space's base plus the word is its address |
| `001` | a `SmallInteger` of 61 bits, `(value << 3) \| 1` |
| `010` | a constant: `nil`, `false` and `true` are the payloads 0, 1 and 2, with room for more |
| `100` | a `SmallFloat64` |
| `110` | free: never produced, kept for a future immediate |
| `011`, `101`, `111` | never produced, so that bit 0 alone tells a `SmallInteger` |

The tags are Spur's (`smallIntegerTag` 1, `smallFloatTag` 4, three tag bits,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:2825` for the three tag
bits, `:2883` for `smallIntegerTag`, `:2830` for `smallFloatTag`): Spur writes a
`SmallInteger` as `(value << 3) + 1` (`integerObjectOf`, `:45697`) and tests it
by bit 0 alone. Spur's `Character` tag, `010`, is taken for the constants:
ns-next has no characters, and `nil`, `true` and `false` are immediates rather
than heap singletons as in Spur and Soup, which is why nothing resembling an
object store is needed to name them in a serialized image. A `SmallFloat64` is
Spur's encoding (`floatObjectOf:`, `smallFloatValueOf:`): an eight-bit subset of
the exponent -- IEEE exponents 896 with a non-zero mantissa through 1151, about
2^-127 to just under 2^129, plus ±0.0 (`isSmallFloatValueBits`, `:45770`;
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

**Each number has one representation**
(`r[vm.heap.number-one-representation]`). A `SmallInteger` is an integer
from `SMALL_INTEGER_MIN` to `SMALL_INTEGER_MAX`, -2^60 to 2^60 - 1; any other
is an object of the kernel class `LargeInteger`, of the 64-bit-indexed
format: its words are the magnitude's digits, its sign is the header's
negative bit, and it holds no handle. `Mutator::new_integer` decides which
([core-library.md](core-library.md), "Large integers"). A finite float in
the `SmallFloat64` range is immediate; every other float -- NaN, the
infinities, denormals and magnitudes outside that range -- is a boxed
`Float`, an instance of the builtin `Float` whose one 64-bit-indexed word
holds the `f64`'s bits and no handle, and `Mutator::new_float` decides
between the immediate and the boxed form by Spur's range test. Soup boxes every
float, which costs an allocation per floating-point result; Spur's split
keeps the common range immediate. Both are §5.1.1's binary float; the
`FixedPointNumber` that section names for non-integral literals -- `Decimal`, the gap
`r[semantics.gap.decimal]` -- would be a class of the numeric tower, not a
tag. A `Fraction` is an object too, of the
fixed-pointer format, its two fields the numerator and the denominator in
normal form (`r[vm.heap.fraction-normal-form]`)
([core-library.md](core-library.md), "The numeric tower"). A boxed
`Float`, like a `LargeInteger` and a `Fraction`, is a value object
(§3.1.1), so its `=` and its hash come from its value, never from the
identity hash: every number compares and hashes by its exact value, across
the four classes, and a NaN equals a NaN either way ([core-library.md](core-library.md),
"A NaN equals a NaN", says why).

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

**The debug build keeps its exact check**
(`r[vm.handle.stored-only-when-current]`).
A debug handle carries its heap's
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
object whose store missed the write barrier (`r[vm.heap.barrier]`) keeps a
stale young word; that
is what `NS_NEXT_STRESS_SCAVENGE`'s check of the remembered set before each
scavenge, and a debug build's poison over the nursery a scavenge vacated,
catch ([memory.md](memory.md), "Generations").

A `String`'s words hold its code points in one of two widths and no handle
([core-library.md](core-library.md), "Strings are code points in two
widths"; "Strings have two widths" below).

The guard in an object's header word is the id an inline cache guards the
object's sends on, a copy stamped from its application's ids
([dispatch.md](dispatch.md), "The dispatch id").

**A symbol is a heap object, and a canonical one**
(`r[vm.heap.symbol-canonical]`): each `Heap` keeps a
canonical table, `Vec<Option<ObjectReference>>` indexed by the number inside a
`Symbol` -- which the interner assigns densely -- and `Mutator::new_symbol`
answers the table's object, allocating it only the first time. A symbol a
program makes from a string (`asSymbol`) whose text nothing has interned takes
no number: `Mutator::program_symbol` answers the object a second, weak table
holds for the text, keyed by the text's hash, or makes one and records it
there, and once something interns the text the canonical table adopts that very
object ([memory.md](memory.md), "Weak references"). Those are the only ways to
make a symbol, which is what §5.1.6 and §3.1.1 need of two equal symbols.
The tables are per heap because a
handle means nothing against another heap. Both are weak: the collector clears
an entry whose symbol nothing else holds.
This is what every reference does -- Primordial Soup's weak `symbolTable`
(`KernelForPrimordialSoup.ns`), SOMns's `Symbols.symbolFor`, Squeak's symbol
table -- and it keeps a symbol an indexable object, so that `Symbol` is a
subclass of `String` in `Kernel.ns`, as the references declare it: a symbol's
words are a string's, in the same two widths, and only its class tells it
from one ("Strings have two widths" below). The symbol a symbol object is,
`Heap::symbol_value`, is the canonical table's inverse, kept beside it per heap
and keyed by the object's byte offset, so the object stores nothing but its code
points and no lookup of a canonical symbol reads them; a program-made symbol
answers the number of its text once something interned it and `None` before,
which names nothing, since every selector, slot name and class name is interned.
A symbol's `hash` is its text's, told apart from a String's, never its number
(`r[vm.heap.symbol-hash-from-text]`). Rejected: a symbol
as an immediate, a tag of its own, which no reference does and which every
string primitive, and later the Newspeak kernel, would have to accept as a
non-object. An image carries a symbol as a record of its text, never a number;
the decoder interns each text and enters each record into the loading heap's
canonical table, and refuses a second record of the same text
([image-format.md](image-format.md), "Verification").

**An `ObjectReference` is an offset, not a pointer**
(`r[vm.handle.space-relative]`). Every object lives in one
`Heap`'s object space and is reached only through a `Heap` accessor; the offset is
private to the `heap` module -- `crates/ns-runtime/src/heap.rs` and its
submodules, `heap/value.rs` among them, which puts it into a word and takes it
out again -- so the compiler refuses to read it anywhere else. `Value` is
therefore `Copy` and a handle has no destructor. What that representation buys,
how the collector reclaims an object once nothing reaches it, and why a handle
means nothing against a different `Heap` are in [memory.md](memory.md).

**The identity hash is drawn from a counter the first time `hash` asks for
one** (`r[vm.heap.identity-hash-moves-with-object]`,
`r[runtime.memory.identity-hash]`), and 0 means "not yet assigned"
(`Heap::object_ensure_identity_hash`); it is 22 bits
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
can rewrite it in place (`r[vm.handle.only-where-rewritable]`;
[memory.md](memory.md), "What the collector traces") and an `Rc` shared by
several holders is not such a place. So a block's
activation (`FrameKind::Closure`) and a decided block step name the closure
object, and reach its body and its
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
shared between instances of its enclosing class
(`r[semantics.class.per-enclosing-object]`). §2.3 adds the other half: a
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
    enclosing: OnceCell<Weak<Mixin>>, // the mixin it is declared in, set once
    text: Option<Rc<DeclarationText>>, // its declaration's own text, nested ones cut out
}

struct MixinApplication {         // (mixin, super application) -> behaviour
    index: ApplicationIndex,      // its row in the heap's table of applications
    mixin: Rc<Mixin>,
    superclass: Option<Rc<MixinApplication>>,
    own_slot_base: usize,         // where this level's own slots start
    slot_count: usize,            // inherited slots included
}

a class object                    // one per enclosing instance, per level;
                                  // an object of the class format, eight fields,
                                  // nine for a level a `<:` clause added,
                                  // eleven for a body-less declaration's class object:
    0 enclosing                   // object or nil
    1 declaring class             // the declaration chain `outer` walks
    2 superclass                  // parallel to the application chain
    3 this class                  // a metaclass's class; its own metaclass is its header class
    4 builtin                     // SmallInteger position in BuiltinClass::ALL, or nil
    5 value-class refusal         // nil, or an Array of the offender and the level's Symbol
    6 applied clause              // the `<:` clause that added this level, packed, or nil
    7 application                 // SmallInteger ApplicationIndex
    8 named class                 // a level a `<:` clause added only: the class it named
    9 declaration enclosing       // a body-less declaration's class object only: its declaration's
   10 declaration declaring class //   enclosing object and declaring class, nil for a root
```

The layout and its reasons are "The object layout" below; `heap/object.rs`'s
`class_field` names the fields, and every read and write goes through the
`class_*` accessors of `heap.rs`.

A mixin holds its nested mixins, and `Mixin::enclosing` is the link back
up, for the mirrors' `enclosingMixin` and a class side's `instanceMixin`: a
nested declaration's names the declaration it is nested in, a class side's
-- declared or synthesized -- its instance side, and a module's root and a
builtin's mixin have none. It is `Weak`, so a tree is no cycle, and set
once by every builder -- from source, from the embedded shipped code, from
an image -- after the enclosing mixin's `Rc` exists
(`Mixin::set_enclosing_of_children`); `Mixin::enclosing_mixin` reads it.
`Mixin::text` is its declaration's own text ("Source retention and
provenance" below). A mixin is no heap object; a mirror's reflectee stands
for it, the one kernel-private `Mixin` object per mixin per heap
([liveness-and-reflection.md](liveness-and-reflection.md), "A mixin is
reified as a kernel-private object").

**The application cache is keyed by (the mixin's id, the superclass
*application*'s full-width index), never by the superclass class object**
(`r[vm.dispatch.application-cache-key]`). If
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

**A slot is reached through its accessors, which are method-table entries**
(`r[semantics.slot.accessors]`): they are entries of the declaring
mixin's `methods` beside the compiled methods
([dispatch.md](dispatch.md), "Slot accessors"). An entry names its slot by
position among the mixin's *own* slots, so one entry serves every application:
the absolute field is the defining application's `own_slot_base` plus that
position — the same mixin-relative addressing `InitializeSlot` uses
(`r[vm.compiler.mixin-relative-slot-index]`), and for the same reason.
Nothing maps a slot name to a field; nothing but an accessor needs to.

**A lazy slot (§6.3.3, `r[semantics.slot.lazy]`) is an own slot like any
other**, laid out at its
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
accessibility in its enclosing class
(`r[semantics.class.per-enclosing-object]`). §6:

> The first time this method is invoked on a given receiver r, it evaluates the
> class declaration, using r as the enclosing object, and returns the resulting
> class as its result. Subsequent calls on r always return the same class object.

The accessor body, in five steps:

1. Read the hidden slot reserved for this declaration in the receiver's layout.
   If it is not nil, return it.
2. Otherwise evaluate the superclass clause **in the receiver's context**
   (`r[semantics.superclass.clause]`), yielding a class object; take its
   application. A declaration with `<:`
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

**A cache hit is a guard and one field load.** Where the hidden slot sits
in the receiver's layout depends only on the receiver's application, so an
inline-cache entry for the accessor records the slot's absolute index in
the layout of the application its guard names
(`Target::NestedClass`'s `field`, decided by `nested::hidden_slot_index`),
as a slot accessor's entry records its field. A hit reads that field; a
filled slot is the answer, with no activation and without the two hash
lookups the accessor's own path makes (`Heap::known_mixin_id`,
`MixinApplication::nested_slot`). An empty slot, one whose creation is
underway, and a send that brings no index take steps 1 to 5 above, which
run once per enclosing object. The guard decides the index, so the entry
records what the lookup decided (`r[vm.dispatch.cache-records-lookup]`,
`r[vm.dispatch.key-covers-every-dependency]`;
[dispatch.md](dispatch.md), "An entry",
"Every target is a function of its key and its site"). The accessor stays
bodiless, a quick method in Cog's sense, as the slot accessors are: a
compiled body would cost the interpreter an activation per `Foo` and make
no JIT tier faster, since the optimizing JIT inlines either form to the
same code and the baseline JIT emits the guarded load at the site, which
it must do for the slot accessors anyway (decision 5 of `new` as compiled
code, "Decisions" below). The reference lowering runs it uncached, as
every send.

**Inherited nested declarations get their own slots.** §6 says a distinct nested
class exists for each declaration given in the declaration of C *and its
superclasses*, so the hidden slots are part of the absolute layout an
application computes, contributed at the level that declares them, exactly
like ordinary inherited slots. They are derived from a mixin's nested
declarations (`Heap::nested_of`) rather than declared, so nothing stores
them separately. The hidden slot is keyed by the
declaration's mixin, not by its name
(`r[vm.dispatch.hidden-slot-by-declaration]`):
keying by name would make an override and
the declaration it overrides share one slot.

**§2.3's override behaviour needs no special case.** Because the accessor is an
ordinary method, a slot declaration in a subclass overrides it under the normal
lookup rules, and reassigning that slot later cannot disturb an already-cached
nested class. That this falls out of ordinary dispatch is evidence the layering
is right.

**The accessor lives in `interpreter/`**, beside dispatch. It evaluates Newspeak
code — the superclass clause, compiled like any other body — which runs as a
`Step::SuperclassClause` continuing with `Continuation::NestedClass`
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
does evaluates Newspeak code, and compiling a resolved module is total
(`r[vm.compiler.total]`; [compiler.md](compiler.md), "Compilation cannot
fail"), so loading stays total.

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
  public `Future`, which has no instances ("Simultaneous slots" below), and
  the private `Mixin`, whose instances stand for mixins
  ([liveness-and-reflection.md](liveness-and-reflection.md), "A mixin is
  reified as a kernel-private object"). The
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
- **A program reaches a builtin class through `Object`'s accessors** (§3.2,
  `r[semantics.builtin-names]`):
  `Kernel.ns`'s `Object` declares one protected method per nameable builtin,
  `protected String = ( ^outer Kernel String )`, and a bare `String` is
  `self String` by §5.7, so `other String` from outside reaches
  `doesNotUnderstand:` ([core-library.md](core-library.md), "The
  predefined classes are accessors on `Object`", "`class` and the class
  accessors are protected").
- **`Top` is not nameable** (`r[vm.mirror.class-unnameable]`). It is
  declared private in `Kernel.ns`, as
  only `Activation`, `NonBooleanReceiver`, `FutureImplementation`, `Mixin`,
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
  (`instantiate::allocate_for_factory`, which the factory body's
  `AllocateInstance` runs), and the VM mirror's class is refused
  outright (`r[vm.mirror.not-instantiable]`), with a `CannotInstantiate`
  carrying nil rather than the class a
  program must not be handed -- there is exactly one mirror.
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
- The trailing **message** part (`new`) is evaluated fresh on every
  instantiation, by the instance initializer of the class whose clause it is,
  so the subclass's own factory parameters are in scope when it runs.

Sending the factory message to a class object then does this
(`r[semantics.instantiation.initializer-order]`, `r[runtime.instantiate]`):

1. **One** instance `o` is allocated — distinct identity, every slot declared by
   the class and its superclasses initially nil. There is no separate instance
   per inheritance level; `o` is threaded through every step below. An
   instance of a class that inherits `Value` is marked under construction
   (the header's bit 32, "The header word" below).
2. An instance-initializer activation `i` of the class's own level is
   created, with the factory message's arguments.
3. In the context of `i`, the superclass clause's message is evaluated, and
   the superclass's own instance initializer is invoked with its arguments on
   the same `o`. This recurses up the chain. A declaration that applies
   mixins then initializes the levels its `<:` clauses added ("Mixin
   application" below).
4. Once the chain above has run, this class's own slot clauses execute on `o`, in
   declaration order.
5. This class's own initializer expressions execute on `o`.
6. `i` returns to the factory method, which clears `o`'s mark
   (`CompleteInstance`) and returns `o` to the caller. An initializer that
   never returns -- an unwind past it, a signal it does not handle --
   leaves `o` marked for good; storing `o` while it is marked is allowed.

Step 3 is why a class body has no access to the factory's actual parameters
except through slots explicitly extracted in the initializer: only `i` sees them.

**Each level runs in its own activation, with its own arguments.** Milestone 1
walked the superclass chain upward and ran every level's slots and initializer
expressions in the *subclass's* activation with the subclass's factory arguments,
which §6.2 does not permit.

**The factory is compiled code with an activation of its own**
(`r[vm.compiler.factory-body-synthesized]`), shaped as
both references shape it (Squeak-Newspeak's `| o | o := self basicNew. o
initializer`…`: args. ^o`, `Newspeak2SqueakCompilation.ns:1991-2025`;
SOMns's `MixinBuilder.assemblePrimaryFactoryMethod`, `MixinBuilder.java:547-600`;
Primordial Soup's `Newspeak2PrimordialSoupCompilation.ns:1907-1961`).
The factory entry of the metaclass mixin ("Class sides and metaclasses"
below) carries `compiler::factory_body` of its arity, four instructions
([compiler.md](compiler.md), "The three initializer bodies"): step 1,
`AllocateInstance` on the class object the factory was sent to; step 2,
`InitializeInstance`, the initializer of that class object's own level on
the new instance with the factory's own arguments; `CompleteInstance`,
which clears the instance's under-construction mark; and the return of
the instance, whose initializer's answer is not used. So `Foo new` is an
ordinary send that the lookup and the inline caches answer
([dispatch.md](dispatch.md), "Resolve, then act"), and a factory activation
(`Step::Factory`, `FrameKind::Factory`) like any method's, which a JIT can
compile and inline like any other code. A class whose levels run no code
costs that one activation; road step 6's `new` of such a class, which
pushed none, is gone with the walk it short-cut, a slowdown accepted until
the baseline JIT inlines the factory (decision 3, "Decisions" below), and
measured in [execution.md](execution.md).

**A factory sent from the run loop is entered past its first two
instructions.** Nothing runs between pushing a factory's activation and
its `AllocateInstance` and `InitializeInstance`, so `Mutator::push_sent`
carries a factory of `compiler::factory_body`'s shape through them where
it pushes it (`Mutator::enter_factory_body`): the instance allocated into
its register, the factory's program counter at `InitializeInstance`, as
that send stores it, and the initializer it decides pushed above it -- or
the instance written to its destination and the factory started at its
`Return`. The frame, its registers, the depth count and every error stay
the run loop's: the initializer is decided first, which changes nothing,
and the allocation made only for a decision that raises nothing, so any
error leaves the factory to the loop from its start, which raises the
allocation's refusal before the decision's error, as the body orders
them; the initializer's push refused at the depth limit raises from the
factory's activation, as `InitializeInstance`'s send does. What is
skipped is a safepoint at which no program code runs on either side
(`r[vm.activation.factory-prefix-uninterruptible]`).

**The allocation decides everything that refuses an instance**
(`r[runtime.instantiate]`), before it
allocates: `instantiate::allocate_for_factory` builds the builtins whose
instances are not ordinary ones -- `Array`, `ByteArray`, `String` and
`Ephemeron` of their own formats -- refuses the VM mirror's class
(`r[vm.mirror.not-instantiable]`), `Symbol` and `Mixin`, then refuses from
the application's `instantiation_refusal`
and the class object's own value refusal ("A value object's four
conditions" below), and only then allocates through
`Heap::allocate_instance_with` with the header the application carries
([memory.md](memory.md), "Inline allocation"). Each refusal keeps the text
and the selector it had before the factory was code. A host's
`Interpreter::instantiate` runs the same factory body
(`decide_instantiation`), so a host and a program make the same instance.

**Machine code makes the ordinary instance inline and leaves every other
case to the same function.** A factory send in machine code is a compiled
call of the factory body, and its `AllocateInstance` bumps eden's cursor
inline ([execution.md](execution.md), "Sends, returns and the exit that
materializes", "The miss leaf"), reading what `allocate_for_factory`
reads: the class object's format, its builtin field and its value
refusal, afresh at every instantiation, and the header of the
application's instances. That header is the second word of the
application's row in the application id table, the one table machine code
reads an application by (`r[vm.machine.application-id-table]`;
[memory.md](memory.md), "The application id table"):
`Heap::register_application` writes it from the application's
`instance_header` when it records the row, and writes 0 where
`allocate_for_factory` would make no ordinary instance of the
application's count -- an instantiation refusal, or a count that needs an
overflow word -- so that a 0 sends the template to the generic leaf,
which runs `allocate_instance` itself. A row never changes after it is
built, since an edit makes a new application, so no instantiation reads a
header that no longer describes its class's layout; a builtin's own
instances, the VM mirror's class among them, and a refusal are decided by
the leaf, which raises nothing itself and exits so that the interpreter
raises the refusal with its text and selector.

**Each level's initializer is a body on its mixin, outside the method
table, reached only by the instructions that initialize** (decision 2;
`r[vm.compiler.factory-body-synthesized]`). The references name it
``initializer`sel:`` and put it in the method table,
where `perform:` with a constructed symbol, or a mirror, reaches it, so a
program could run a finished object's initializer again, a value's
included; here no message reaches one, and no mirror lists the factory,
an initializer or an accessor. Nor is it a `super` send of a hidden
selector: such a send walks on past a superclass whose factory has
another selector and runs some higher initializer, leaving the skipped
level's slots nil without an error, which is what Squeak-Newspeak does;
here every mismatch stays §6.1's error. A level's initializer runs one of
its mixin's three bodies ([compiler.md](compiler.md), "The three
initializer bodies"), chosen by `instantiate::level_initializer_of` and named by
the frame (`FrameKind::Initializer(level, body)`):

- **the full body** at a declaration's own level: `<:` checks, the
  superclass, the applied levels, then its own part;
- **the standalone body** at a level a `<:` clause added that is reached
  from outside the declaration that applied it ("Mixin application"
  below), or the full body when the mixin writes no `<:` clause, which is
  then the same;
- **the own part** at a level a `<:` clause added, run by the applying
  declaration's full body with that clause's arguments.

Each is entered at its level, whose `own_slot_base` its `InitializeSlot`s
resolve against after the recorded guard is checked ("What a slot access
checks" below). The superclass's is invoked from inside the subclass's
activation, by `InitializeSuperclass`, which climbs `steps` levels from the
activation's class, checks that the clause's selector is that class's
primary factory -- after the clause's arguments were evaluated, since §6.3
evaluates the message "in the context of *i*" and then invokes, so their
effects precede §6.1's error
(`goldens/language/class/instantiation/spec_6_1_a_clause_argument_runs_before_the_mismatch_fails.ns`)
-- then checks the arity and enters that level
(`r[semantics.instantiation.initializer-order]`). A verdict precomputed per
application would skip the arguments' side effects. Every instruction that
reads a level validates what a crafted image can make disagree -- a level
that is not there, a body that is missing, an arity -- and raises an
ordinary error, never a panic. Nothing caches the target: it is a few field
reads, and a cache would key on facts of several class objects rather than
of one guard; it is left to the JIT.

**The arity is checked at every level**, against the factory of the
declaration whose body the level runs (`check_factory_arity`: the
mixin's own, or a body-less declaration's), since that body's registers
are laid out for it (`r[vm.activation.push-checks-arity]`). §6.1's selector
check pins the arity for a class with
a body, since a selector's keyword count is its parameter count, but not
for a body-less named class, and not for a decoded image, which can pair a
body with a level whose factory has another arity; the check is what
stands between such a body and a register window of the wrong size.

**A level whose full body would only check is passed, in a loop.**
`Mutator::enter_or_pass` does not enter a declaration's own level when its
declaration needs no initializer code, none of its clauses passes
arguments, and every level its `<:` clauses added has a mixin with no own
code and a factory of no arguments (`r[vm.activation.passed-levels-iterative]`):
it makes the full body's checks itself,
in the body's order and with its texts -- each `<:` clause against the
class it named, nearest level first, then the superclass clause against the
class above them -- and carries on at that class with no arguments. A level
a `<:` clause added and reached from outside is always entered. So `Array
new` is the factory's one activation, and a chain of levels without code
neither nests nor escapes the depth limit's count, whatever its length
(`levels_without_code_are_passed_without_an_activation`). What it asks of a
level it copies out of the level's application in one read
(`LevelFacts::of_mixin` the level's own mixin, computed where the application is
built and checked against the mixin by a debug build's
`Heap::register_application`; `r[vm.dispatch.level-facts-agree]`):
`needs_initializer_code`, the superclass
clause's `clause_selector` and `clause_has_arguments`, the
`applied_clause_count` and `applied_clauses_pass_arguments`, the mixin's own
`factory_arity`, and for a metaclass mixin the factory entry it holds.
Which factory a class object has is not among them: it is read through
the class object's metaclass ("Class sides and metaclasses" below). The
level's application, the clause that added it and its superclass are read
with one look at the class object's words (`Heap::class_level`), and the
body the decision found rides in its `Step::Initializer`, so the push does
not look it up again.

**A chain nests one counted activation per level with code** (decision 6).
Each initializer invokes its superclass's from inside its own activation,
so the levels with code nest, and a chain of more of them than the depth
limit fails with the ordinary stack-overflow error, as any recursion that
deep does (`r[runtime.depth-limit]`;
`goldens/language/class/instantiation/a_chain_deeper_than_the_depth_limit_overflows.ns`;
[execution.md](execution.md), "A superclass chain nests one activation per
level"). A flat chain would need a Rust driver between the levels, which is
the walk compiled `new` replaced.

**The initializer's answer is the instance's** only by construction: both
lowerings end an instance initializer by answering `self` and discard a
top-level `^`, and the factory answers the instance it allocated whatever
the initializer answered. A `^` in a block in a clause argument is an
initializer's `^`: it targets the initializer's activation, which catches
none, so it escapes the instantiation as an initializer expression's does
([execution.md](execution.md), "Unwinding";
`a_return_inside_a_block_in_a_clause_argument_escapes_instantiate`).

**An edit rebuilds them with its mixin.** The MirrorBuilder builds an
edited declaration's mixin as the source path builds every mixin
(`Mutator::build_mixin`), so its three initializer bodies and its factory
entry's body come with it, and the atomic install's re-application
recomputes every per-application fact, `LevelFacts` included, while each
metaclass keeps its class side, tracked by mixin id
([liveness-and-reflection.md](liveness-and-reflection.md), "A
MirrorBuilder and an atomic install").

### A value object's four conditions

§3.1.1 makes an object a value object when "it is deeply immutable ... and its
class inherits from class `Value`" (`r[semantics.value]`), and its second
bullet spells that out as
four conditions: all its slots "are immutable, have been initialized and
contain value objects"; "its enclosing objects (3.3) are all value objects";
and "its class inherits from class `Value` and does not define an identity
method (`==`)". All four are enforced strictly
(`r[semantics.value.conditions]`; designed with the user on
2026-09-23). Two are about an instance's slots and are decided as the
instance is built, the one place every instance of every class comes from,
whether its class was loaded from source, materialized as a nested class or
rebuilt from an image. Of the other two, the identity method is a property of
the *application* and is decided once, where `Mutator::apply_mixin` builds
it — before any class object exists, which is what covers a module's root
class along with every nested one — and the enclosing objects are fixed per
*class object*, decided once where one is created. Every refusal is raised at
run time — a class nothing instantiates never fails, as a method nothing
runs never does — and every
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
exist is an instance that passes for a value and is not one.

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
  the factory body's allocation (`allocate_for_factory`) refuses it from
  `instantiation_refusal`, the same field the root clause defect and the
  mutable slot are refused from.
- **Enclosing objects are values** is decided once per *class object*, cold,
  by `interpreter/value_class.rs`'s `check_value_class`, which
  `interpreter/nested.rs`'s `after_clause` calls after `apply_mixin` answers
  the application, and only for an application that inherits `Value`; the
  verdict is recorded on the class object (its value-class refusal field,
  written by `Mutator::record_value_class_refusal` from a
  `ValueClassRefusal`: an `Array` of the offending enclosing object and the
  name of the level at fault, "The object layout" below), and
  the factory body's allocation (`allocate_for_factory`) refuses an
  instance of a class that carries one,
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

**"Have been initialized": a value is under construction until its factory
returns** (`r[semantics.value.under-construction]`; decided with the user on
2026-11-28). The fourth condition is checked slot by slot as each is
initialized, but §3.1.1 also asks that the slots "have been initialized",
which no single store can tell: an initializer can hand `self` out -- as an
argument, into an `Array`, into a slot of another value -- before its later
slots run. Such an instance read with an uninitialized slot, and its `hash`,
derived from its state, changed once the rest were initialized, so a hashed
collection that had stored it lost it. The instance is therefore marked
under construction by its factory's allocation and unmarked by the
factory's `CompleteInstance` once the initializer has returned normally
("Instantiation" above; "The header word" below); a construction cut short
-- an unwind past it, a signal its initializer does not handle -- never
reaches that instruction, so an instance it handed out stays marked for
good and never becomes a value -- across an image too, which writes it
under a body tag of its own ([image-format.md](image-format.md), "A value
cut short keeps its mark"). A signal its handler resumes does not cut
the construction short: the initializer goes on and the factory returns
(`goldens/library/value/a_construction_resumed_after_a_signal_completes.ns`).
Each factory unmarks only its own instance, so a value whose initializer
builds another of its class completes the inner one first
(`goldens/library/value/a_value_built_inside_another_ones_initializer_completes_on_its_own.ns`).

While an instance is marked, it answers no question whose answer depends
on its state, and a question whose answer is fixed whatever its state is
answered (decided with the user on 2026-11-28: a question of the second
kind has the same answer before and after the factory returns, so
answering it reveals nothing of the partial state and withholds nothing
the instance will have):

- **`Value`'s own `=` and `hash`**, the derived ones, and through them
  `==` and `identityHash`, which `Value` declares as `=` and `hash`. A
  value class that declares an `=` or a `hash` of its own replaces the
  derived one, its refusal included, and decides for itself; its `==` and
  `identityHash` follow it. The equality leaf
  (`builtins/value.rs`) refuses a marked receiver before anything else --
  the numeric tower and the classes included. A marked argument is
  answered `false` where the walk's class comparison finds that the two
  classes cannot be equal -- `nil = aPoint`, `true = aPoint`, as the
  number, String and Symbol leaves already answer `3 = aPoint` and `'abc' =
  aPoint` without reading the argument's state -- and refused where the
  comparison would go on to its slots. Every pair the walk reaches among
  the slots is held to the same rule, its receiver's side as the receiver:
  that side is the slot `valueEquals:` sends `=` to when the leaf
  declines, and that send refuses a marked receiver whose `=` is
  `Value`'s, so the leaf and the fallback never answer one comparison
  apart. A slot pair whose receiver's side is of a class with its own
  `=` is that `=`'s to decide, whatever the other slot holds, so the
  walk asks first, below the root, whether it computes the receiver
  side's `=` itself (`receiver_equality`: `Value`'s, an immediate's, or
  that of a number, a String or a Symbol of the kernel's own class or
  of a class object whose `=` is `Behavior`'s) and declines the pair
  where it does not -- before its tower and its class test, which would
  answer by shape for a `=` that may answer otherwise, and before the
  marks, which are no reason to refuse such a pair: the fallback's send
  decides, refusing or not as that `=` does
  (`goldens/library/value/a_slot_with_its_own_equality_is_compared_by_it_against_any_slot.ns`).
  The classes are compared as `valueEquals:` compares them, by `==` sent
  to the receiver's class: by `Behavior>>==`'s rule, unless that class's
  class side declares `==` of its own, which a flag of its metaclass's
  application says without a lookup (`LevelFacts::declares_identity`,
  `Heap::class_side_declares_identity`); there the walk declines and
  that `==` decides; `valueEquals:` reads every slot through readers
  that refuse a subject under construction (`valueSlotCountOf:asking:`,
  `valueSlotOf:at:asking:`, in the words of the question they are
  asked), so a marked argument is refused there wherever that `==` lets
  the comparison go on to the slots
  (`goldens/library/value/a_class_side_identity_decides_whether_two_values_classes_are_equal.ns`).
  It examines the slots in declaration order, each unfolded before the
  next, and a class pair's enclosing objects and superclasses in
  `behaviorEquals:`'s order, so that a refusal and an unequal pair are
  met in the order that method meets them; and it finds `nil` equal to
  `UndefinedObject new`, as `valueEquals:` does, neither having slots.
  `the_leaves_answer_as_the_methods_they_stand_for_over_every_shape`
  holds the walks to `valueEquals:`, `behaviorEquals:` and
  `valueHashOfSlots` over every pair of every shape. The lookup it
  makes for an instance is the one the walk made for it anyway, handed
  on to `compare_node`. The walk answers whichever it
  meets first, a refusal, an unequal pair or such a decline, in its
  order. The hash leaf refuses its receiver and every marked node it
  unfolds, since hashing reads state, and declines at the first node
  whose `hash` is its own, which `valueHashOfSlots` sends that `hash`
  (`goldens/library/value/a_slot_under_construction_with_its_own_equality_is_compared_by_it.ns`;
  the definition below).
  A refusal is an error, never a decline: a decline would run
  `valueEquals:` or `valueHashOfSlots`, which would compare or hash
  exactly the partial state the refusal withholds
  (`goldens/library/value/a_value_under_construction_compares_unequal_to_another_class.ns`).
- **A value's hash is defined over a breadth-first prefix of its
  unfolding** (`r[runtime.value.hash]`): the first 4096 nodes, visited
  from a queue -- the value first, then each node's slots in declaration
  order -- a node whose `=` and `hash` are both `Value`'s unfolded and
  adding its slot count, any other adding its own `hash`, and the sum
  masked to its low 60 bits after every addition. The leaf
  (`builtins/value.rs`, `derived_hash`) and `Value>>valueHashOfSlots`
  follow it alike, the method asking the leaf which nodes it unfolds
  (`valueHashUnfolds:ifFail:`), so the leaf declines only at a node
  whose `hash` it cannot send and the two never answer apart
  (`equal_values_hash_alike_by_their_breadth_first_unfolding`, and the
  differential test over every shape). Two equal values unfold to the
  same sequence of nodes however their states share structure -- a ring
  of one value and an equal ring of two -- so they hash alike, and a
  value whose state reaches itself hashes by a finite prefix rather
  than not at all. Decided with the user on 2026-11-28 (option B), over
  keeping a cut sum as the leaf's exception and over detecting cycles:
  Bracha's VMs have no structural value hash to follow, and OCaml's
  `Hashtbl.hash` walks a value breadth-first within a bound in the same
  way.
- Every refusal names the marked object's class and the question, through
  one builder, `instantiate::under_construction_error`, in the "cannot
  <action> …: <reason>" form of the other value errors -- `cannot compare
  an instance of Point: it is still under construction, and an instance of
  a value class is a value only once its factory has returned`, `cannot
  hash an instance of Point: …` and `cannot tell whether an instance of
  Point is a value: …` (the wording decided with the user on 2026-11-28).
- **Whether it is a value**, `objectIsValue:ifFail:` (`builtins/weak.rs`,
  after its decline for a future). `false` would be wrong too: the instance
  becomes a value the moment its factory returns, so an `Ephemeron` or a
  `WeakMap` that keyed it would hold a value as a weak key from then on.
  Refusing it there is what lets `Ephemeron>>key:` and `WeakMap>>at:put:`,
  which ask the question, go without a check of their own
  (`goldens/library/value/a_value_under_construction_is_no_weak_key.ns`).

What stays allowed is storing the instance and using it as an enclosing
object. A value module's own slots build its nested values while the module
is still under construction -- `| origin = Point x: 0 |`, each `Point`
nested in the marked module -- and comparing two of them compares their
slots without reaching the module
(`goldens/library/value/a_value_module_builds_its_nested_values.ns`); and two
values may reach each other, an `A` whose slot holds a `B` that holds the
`A` still being built, which §3.1.1's phrasing, a condition on every object
reachable, admits as a cycle. Once both factories have returned, the cycle
compares and hashes as values do
(`goldens/library/value/values_in_a_cycle_compare_once_complete.ns`).
Refusing the store instead, as a value's fourth condition refuses a
non-value, would have outlawed both.

The prior art leaves this open. Bracha's formal semantics defines value
status as the greatest fixed point of the structural conditions,
"initialized immutable slots" among them
(`prior-art/newspeak/formal-semantics/newspeak-semantics.tex`, near its
module definitions), and the actor semantics transfers "only a fully
initialized value" (`actor-semantics.tex`), but neither says what an
instance is before then. Eurasia marks an instance of a value class at its
allocation and records the gap itself: "the end of construction is not
marked, so an initializer that hands out `self` before it finishes can
expose a partly built value", proposing a syntactic value class whose end of
construction the compiler could mark
(`prior-art/newspeak/EURASIA_ACTORS_DESIGN_2026-11-17.md`, section 5). SOMns
decides value status per class and checks the slots once the initializer
has run (`IsValueCheckNode`), with no state for the time in between. Here
the end of construction is the factory's return, which every instantiation
passes through, so no syntax is needed to find it.

**`Kernel` and the VM mirror are values, as in SOMns**
(`r[semantics.value]`; decided with the user
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
refuses it, the factory body's allocation (`allocate_for_factory`)
refuses to instantiate its class, and
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

**The four rules** (`r[semantics.slot.simultaneous]`; the value rule is
also `r[semantics.value]`).

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
   instantiation of such a class, for nothing a program can observe
   (`r[vm.heap.resolved-future-replaced-in-fields]`).
4. **A value never holds a future** (§3.1.1's deep immutability). A future
   about to be stored in a slot of a value is forced first, and its value is
   stored and checked — so `|| p = Pair first: q second: 1. q = 3. ||`
   builds a `Pair` holding `3`, where refusing the future would fail a
   well-founded program. The clause's own slots in a value class are the one
   exception, and only for the first write ("The lowering" below).

A future still being computed is observable, as §6.3.2 intends: touching one
forces it, and forcing one that is already being computed — ill-founded
recursion — raises an ordinary, catchable `Error`, a text that names the
future rather than a slot because a program's own `Future computing:`
diverges the same way. **A clause abandoned by an exception** —
an initializer raises and a handler outside the instantiation takes it —
leaves the futures it had not resolved as they are, wherever they escaped
to: the next message forces one and runs its initializer then, and one
whose initializer raised raises again for the same cause. A forcing an
unwind abandons -- its computation raised, or the run loop refused one of
its pushes -- leaves its future unresolved again rather than stuck
resolving: `resolve` marks the future resolving inside the block its
`ifCurtailed:` protects, and the `ifCurtailed:` undoes the mark. Nothing
answers wrongly, and it costs nothing; marking the clause's futures broken
would put an `ifCurtailed:` around every simultaneous clause.

**The future.** `Kernel.ns`'s private `FutureImplementation`, which no
program can name (as `Activation`), is a subclass of the public `Proxy`,
after Primordial Soup's: `Proxy` overrides every public member of `Object`
— `==`, `=`, `hash`, `yourself`, `identityHash`, `printString`, the nil
protocol, `halt` — as protected, and `Object` itself declares `class` and
the class accessors protected, so every message from outside reaches
`doesNotUnderstand:`, and
`FutureImplementation`'s forces the future by its own `resolve` and
forwards the message to the value with `Message>>sendTo:`. A member added
to `Object` without its override in `Proxy` fails
`a_proxy_overrides_every_public_member_of_object_as_protected`. Its state is
two private slots, `computation` — what computes it until it is resolved,
then its value — and `state`, 0 unresolved, 1 resolving, 2 resolved,
which only a `resolve` writes once the future is made.
**Forcing is that method**, ordinary Newspeak, as the canonical
implementation's `Future>>resolve` is, so that machine code compiles it as
it compiles any method and calls it as it calls any other: a resolved
future answers its value; an unresolved one is marked resolving, its
computation evaluated -- a block, or any other object sent `value` -- and
the value stored with the resolved state; one being computed signals the
divergent-evaluation error. The mark is set inside a block its
`ifCurtailed:` protects, which sets the future unresolved again when an
unwind abandons the forcing, its computation kept. A computation that
answers another future makes that one's value this one's -- §6.3.2's
pipelined promise -- so a resolved future's value is never a future: a
resolved one's value is taken as it is, and an unresolved one is **taken
over**: marked resolving, recorded, and its computation run by the same
`resolve`, in the same activation, and so on along the chain; at the end
the value is recorded in every future taken over, and the `ifCurtailed:`
sets them all unresolved again when an unwind abandons the forcing. One
future cannot send another its private methods, so `resolve` reads and
writes the futures it takes over through leaf-safe rows of the VM mirror
(`builtins/future.rs`): `futureState:ifFail:`, which also answers its own
state, normalized to a small integer, `futureComputation:ifFail:`,
`futureTakeOver:ifFail:`, `future:resolveTo:ifFail:` and
`futureAbandon:ifFail:`, beside `objectIsFuture:ifFail:`, which asks
whether a value is a future without forcing it. A chain of futures each
answering the next is therefore resolved at a depth that does not grow
with its length, as the forcing in Rust was (`tests/future.rs`, two
hundred thousand links), where the canonical `resolve`, which sends the
next future `resolve`, recurses once per link; and the loop is leaf
calls only, so machine code runs it without an exit. Crafted code can
store into the slots once it holds a future, so they are read as
untrusted: any other state, or a resolved future whose value is a
future, signals an ordinary error, "a malformed future: its fields are
no future's" ([image-format.md](image-format.md), "Futures in a
graph"); the leaf that answers the state calls it 3, the runtime
`FutureState::Malformed`, and only a well-formed resolved future is seen
through.
A program has no message of its own that forces a future, since a public
`resolve` would tell an escaped future from its value: a future is forced
only where the runtime needs the value it stands for -- a message sent to
it, an instruction that must not hold one, or a leaf that could not answer
for it as an operand -- and there the runtime pushes `resolve`
(`Mutator::forcing`). A simultaneous clause's futures are
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
  is replaced by its value and the leaf runs again, in Rust. Any other one
  cannot be forced in Rust, since nothing re-enters the loop: the answer is a
  step that pushes the future's `resolve` under `Continuation::Reexecute`,
  whose return drops the value and runs the caller's instruction again
  instead of the next one (`vm::reexecute`); the second time the future is
  resolved, or `resolve` signalled its error. This rests on an invariant written
  on `builtins::execute` and checked leaf by leaf: **a leaf that declines or
  raises has changed nothing** (`r[vm.dispatch.declining-leaf-changes-nothing]`,
  `r[vm.dispatch.future-seen-through]`; [dispatch.md](dispatch.md),
  "Primitives").
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
  value of its own — the selector and description an unhandled exception's
  termination reports — decline for a future, so the failure path sees
  through it. `do:`'s block and a loop's body are no leaf's operands: the
  Newspeak loops send them `value:` and `value`, and a future is seen
  through by that send ([core-library.md](core-library.md), "Real bodies
  for control flow and `do:`"). Every
  other leaf that answers for any operand stores or forwards it
  (`at:put:`'s element, `value:`'s arguments, `sendTo:`'s receiver,
  `returnValue:`'s value), and a future there is the value it stands for to
  whoever reads it later. On a leaf's failure path, though, every future
  operand is seen through: an unresolved one the leaf would only have stored
  is forced first, and one still being computed signals the
  divergent-evaluation error, from its `resolve`, in place of the leaf's own
  error.
- *The reflection leaves* (`Primitive::Reflection`), which only
  `MirrorsFactory.ns` sends, are never seen through: a mirror reflects the
  object it is given, a future the future itself, so a reflection leaf that
  declines for a future declines and no mirror read forces one
  (`future::after_a_failed_leaf`). The family has an identity leaf and a
  Symbol's-text leaf of its own, so that the mirrors need neither of the
  kernel's, which see through a future
  ([liveness-and-reflection.md](liveness-and-reflection.md), "A mirror
  sends its reflectee nothing").
- *`InitializeSlot` into a value* looks for a future only on its refusal
  path (`check_value_slot`, then `future::value_for_a_value_slot`): it
  stores and checks a resolved one's value, and forces any other through
  the same `Continuation::Reexecute` (`vm::force_and_reexecute`).
- *`ResolveFuture`* moves a value that is no future and loads a resolved
  one's value; for any other it calls the future's `resolve` above its
  frame's window and takes the answer into its destination as a send takes
  its callee's (`vm::resolve_future`, `send_destination`), and machine code
  makes the same call as a compiled send ([execution.md](execution.md),
  "Running an instruction again after forcing a future").
- *A host* that passes a future straight to `Interpreter::send` has no
  instruction to run again: a resolved one is seen through in Rust as
  anywhere else, and an unresolved one is forced, after which the run ends
  with an error saying so (`vm::reexecute`,
  `interpreter::future::host_passed_a_future`); a host holds a future only if
  a program handed it one during a clause.

**The collector** replaces a reference to a resolved future with its value in
object fields and never in registers or host roots
(`r[vm.heap.resolved-future-replaced-in-fields]`,
`r[vm.heap.resolved-future-judged-by-value]`): a forcing activation that is
returning has the future as `self`, and would otherwise read its own slots from
the value ([memory.md](memory.md), "A resolved future is traced as its value").

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
(`tests/opcode_tags.rs`). Three of the four `benchmarks/micro/` fixtures are
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
whole body may be replaced by a `.`: such a declaration is the mixin
application itself (decision 8 of `new` as compiled code, "Decisions"
below). `ClassDeclaration::mixin_applications` carries the clauses
in written order, and the declaration's `Mixin::mixin_applications` one
`MixinApplicationClause` each — its message (a `ClauseMessage`, the
selector and whether it has arguments, the same record `Mixin::superclass`
holds for the superclass clause). A clause's message has no body of its
own: its arguments are lowered into the declaration's initializer
([compiler.md](compiler.md), "The three initializer bodies"). Type
arguments on a clause's receiver (`<: Collection[E]`) are kept in the syntax
tree and ignored, as they are on a superclass clause
([frontend.md](frontend.md), "What is kept but ignored, and what a reprint
drops").

**Each clause adds one class level between the declaration and its
superclass** (`r[semantics.mixin.application]`,
`r[semantics.mixin.clause-order]`), lowest first, so the last clause's mixin
is the nearest to `C`.
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
  error, raised where the levels are built. The level's metaclass applies
  the named class's class side, so its factory and its name are the named
  class's (`Heap::class_factory`, `Heap::class_name`), and the level keeps
  the named class itself in a field every other class object leaves nil
  (`class_field::NAMED_CLASS`), which the clause's check reads and the image
  rebuilds the metaclass from. Its own part runs with the clause's
  arguments, their count checked against the factory of the mixin it
  applies, whose registers it is laid out for.

**A declaration without a body is the mixin application it writes**
(`r[semantics.mixin.application]`; §6:
"the mixin can be specified along with the superclass via a mixin
application"; decision 8 of `new` as compiled code). For `class M3 x: x =
Base new <: M1 tag: x + 1.`, `Mixin::has_body` is false, and `M3`'s class
object is the level its last clause adds -- `M1`'s mixin over the levels
below, with `M1`'s enclosing object and declaring class -- with no level of
its own over it. Its metaclass applies `M3`'s own, synthesized, class side,
so its factory is `x:` and its name `M3`. `M3 x: 1` runs `M3`'s full
initializer at that level, which counts the levels from 0: its clauses'
checks, the superclass, and each level's own part, the last one `M1`'s at
`M3`'s own level; it has an empty own part and no standalone body. `class U
= Base new <: M3 x: 5 (…)` checks `x:` against `M3`'s factory, and its level
runs `M1`'s own part with `5`, its arity checked against `M1`'s factory;
`M3`'s own clauses never run there
(`goldens/language/class/mixin/spec_6_a_body_less_declaration_is_the_mixin_application.ns`).
A passed level (`Mutator::passes`) counts a body-less declaration's levels
the same way. The image never applies a body-less row's mixin. §6.2 makes
the result of a mixin application "a class I with mixin M", so a level of
the declaration's own, with an empty mixin, over the applied levels -- what
ns-next built before -- was a deviation: `class U = Base new <: M3 new.`
gained nothing of `M1`. Squeak-Newspeak's accessor applies the last mixin
itself, named for the declaration (`Newspeak2SqueakCompilation.ns:1931-1955`).
The factory is the declaration's signature, not `M1`'s selector, because
the declaration may write one and its parameters are in scope "throughout
the mixin application" (§6); Squeak copies the declaration's factory onto
the application too. The class object has no class side but the one
synthesized for its factory, and inherits none of `M1`'s, since
metaclasses are flat ("Class sides and metaclasses" below).

**A body-less declaration's own code resolves in the declaration's scope.**
The class object is two classes at once: the level, whose enclosing object
and declaring class are `M1`'s so that `M1`'s methods walk `M1`'s lexical
chain, and the declaration `M3`, a member class of the class it is written
in, whose clause messages and arguments were written in `M3`'s scope (§6:
the factory's parameters "are in scope throughout the mixin
application"). So the class object keeps two fields more than a level
uses, `M3`'s own
enclosing object and declaring class (`class_field::DECLARATION_ENCLOSING`,
`Heap::class_declaration_scope`), written by `nested::body_less_class`
once the superclass clause has run and nil for a module's root, which §3.3
gives no enclosing object. A frame running `M3`'s own code -- its full
initializer entered at that class object, or a block created in it, which
the code itself says (`CompiledCode::is_declaration_code`, marked on the
full initializer and its blocks wherever a body-less mixin is built:
`Mixin::mark_declaration_code`; a debug build holds it to the frame's
captured chain, `Heap::activation_runs_declaration_code`) --
starts its lexical walk there (`r[vm.dispatch.lexical-origin-from-code]`;
`interpreter::sends::lexical_origin`,
`dispatch::LexicalOrigin::Declaration`): depth 1 is `M3`'s enclosing
object, deeper levels step out from `M3`'s declaring class, and depth 0 has
no N, since `M3` declares no member and §5.8 makes a self send `outer M3 m`.
Every other frame at that class object -- `M1`'s methods and initializer
bodies -- walks `M1`'s chain, as before. So one class object serves both,
and only the frames running the declaration's own code start elsewhere
(decided with the user on 2026-10-22, decision 8 as amended, "Decisions"
below): a class object of the declaration's own would break `M1`'s methods,
which matter far more than the declaration's clause arguments, and a class
object with only the declaration's context would walk `M1`'s methods out
of the wrong scope. SOMns decides the same per running method: an outer
read finds the lexical class by the running code's mixin, falling back to
the class the mixin was applied to (`OuterObjectRead.getEnclosingClass`,
`lookupClassWithMixinApplied`,
`prior-art/somns/src/som/interpreter/nodes/OuterObjectRead.java:120-133`).
Squeak-Newspeak gets the mixin's side wrong: its mixin application is
created with the declaration's enclosing object (`apply:withName:index:enclosingObject:`
with `self`, `Newspeak2SqueakCompilation.ns:1931-1955`), so `M1`'s methods
walk out of the declaration's scope. Which walk a
frame takes is a function of the code it runs, so it is read off the code,
never off the class object per send, and the lexical sends of `M3`'s own
code, which runs once per instantiation, run uncached: every cached
lexical send starts at its current class
(`goldens/language/class/mixin/spec_6_a_body_less_declarations_clause_resolves_in_its_own_scope.ns`).
`super` takes no part in this (`r[semantics.send.super]`): §5.10 starts it
at "the superclass of *C*",
*C* the current class of the activation (§3.6), and `M3`'s own code is
evaluated by the instance initializer of the class *I* §6.2 makes
("the instance initializer for *I* evaluates *m*<sub>s</sub> ... Then,
*m* is evaluated"), which is the class object itself. So its `super`
starts at the class object's superclass, `Base` in
`class M3 = Base new <: M1 tag: super foo.`, passing over `M1`, as
`M1`'s own methods' `super` does, and is cached like any other
(`interpreter::sends::super_start`, which has no exception;
`goldens/language/class/mixin/spec_5_10_super_in_a_body_less_declarations_clause_starts_above_its_class.ns`).
A body version `class M3 = Base new <: M1 tag: super foo () ()` differs:
its class is a level of its own below the one its clause adds, so its
`super foo` finds `M1`'s. SOMns and Squeak-Newspeak start it at the
superclass too (decided with the user on 2026-10-23, after the final
review of the batch read §5.10 to the letter).
Reflection reports `M3`'s own enclosing object, the declaration's, as §6
gives the class of a declaration "an enclosing object which is eo"
(`Heap::class_reflected_enclosing`, for a mirror's `enclosingObject` and
`Behavior`'s; nil for a root), while the walk of `M1`'s code keeps the
level's
(`goldens/library/mirror/a_body_less_classs_enclosing_object_is_its_declarations.ns`).

§3.1.1's enclosing-object condition (`r[semantics.value.conditions]`) is
decided on both: an instance's
enclosing objects (§3.3, "with respect to S") are `M3`'s, `M1`'s level's
and every level's below, as a body version's are, so a body-less value
class is refused when `M3`'s enclosing object is no value as well as when a
level's is (`value_class::check_body_less_value_class`;
`goldens/library/value/a_body_less_value_class_is_held_to_its_declarations_enclosing_object.ns`),
and the image's rebuild decides it the same way. The class object itself
is a value only when both enclosing objects are
(`instantiate::class_object_is_a_value`;
`goldens/library/value/a_body_less_class_declared_in_a_mutable_object_is_no_value.ns`),
and two body-less class objects are equal only when they are one
declaration's with equal declaration enclosing objects -- two declarations
applying one mixin share an application, yet are two classes, as their
body versions are (`Heap::classes_share_body_less_declaration`;
`goldens/library/value/two_body_less_value_classes_are_two_classes.ns`). The
Kernel's fallback for a declining leaf, `Behavior>>behaviorEquals:`, compares
the same state: `sharesApplicationWith:` also asks for one declaration, and
both enclosing objects are compared, the declaration's through
`behaviorEnclosing:` and the level's through `behaviorLevelEnclosing:`. A body-less
module root, `class R = Object <: Value.`, is the `Value` level over
`Object`, named `R`, with `Value`'s enclosing object and an empty
declaration scope; the image's root rule holds it to exactly that
(`modules::body_less_root_record_defect`;
`goldens/language/module/spec_6_a_body_less_root_is_the_value_level_its_clause_adds.ns`).

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
A superclass clause of more than 1,024 `<:` clauses is a resolve error
(`r[image.limit.clauses]`, `r[image.limit.clause-index]`;
[compiler.md](compiler.md), "Limits"), well within the position's 23
bits.

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

**Initialization order follows §6.2's text**
(`r[semantics.mixin.clause-order]`), generalized to a chain, and
matches SOMns (`MixinBuilder.assembleInitializationMethod`): `C`'s full
initializer checks every `<:` clause's selector against the primary factory
of the class it named, nearest level first (`CheckAppliedClause`);
evaluates `ms` and runs `S`'s initializer — and everything above `S` — with
its arguments (`InitializeSuperclass`); then evaluates `m1` and runs `M1`'s
own part with its arguments at the level `M1`'s clause added
(`InitializeAppliedLevel`); and so on to `mn` and `Mn`; and then runs `C`'s
own slots and initializer expressions ([compiler.md](compiler.md), "The
three initializer bodies"). Every clause message is lowered in `C`'s
factory scope and evaluated with `C`'s factory arguments (§6.3 makes the
factory's parameters visible in the superclass clause), and each `mk` is
evaluated just before `Mk`'s own part runs
(`goldens/language/class/mixin/mixin_initializer_order.ns`).
An applied level's own part is checked against the arity of its mixin's
own factory, for which its registers are laid out
(`r[vm.activation.push-checks-arity]`). Because the `<:` clauses' selectors
are checked first, a
misnamed clause fails before `ms` and the superclass initializers run
(`goldens/language/class/mixin/mixin_clause_naming_the_wrong_factory_fails.ns`),
while the superclass clause's selector is checked once its arguments have
run, as §6.3 orders it ("Instantiation" above). An applied mixin's own
superclass clause is never evaluated there: only its mixin's own part runs,
and none of its own `<:` clauses
(`goldens/language/class/mixin/mixin_applied_mixins_own_clauses_are_not_applied.ns`).

**A level a `<:` clause added, reached from outside, is an ordinary class
of its mixin** (`r[semantics.mixin.standalone]`; decision 4, "Decisions"
below). A program reaches such a
level only through a mirror (`(ClassMirror reflecting: C) superclass
reflectee`). Instantiated directly, or named as the superclass of another
class, it initializes as a class whose mixin is the applied mixin and whose
superclass is the level's actual superclass: it runs its mixin's
standalone body -- the mixin's *own* superclass clause, checked by §6.1
against that superclass, then the mixin's own part -- and never the
mixin's own `<:` clauses, whose levels are not above it. Squeak-Newspeak deletes such a
level's initializer (`prior-art/newspeak/squeak/KernelForSqueak.ns:1055-1065`), so both cases end in a
`doesNotUnderstand:` or silently run a higher initializer; SOMns flattens
the levels into one class, so none is reachable; Primordial Soup has no
`<:`. For `class Below y: y = L tag: …` over the level `L` that `Marker`
adds to `Base k:`, `Below`'s clause message is evaluated and the error is
"the superclass clause of class Marker sends #new, but the primary factory
of class Base is #k:"
(`goldens/language/class/mixin/spec_6_2_a_level_reached_through_a_mirror_is_a_class_of_its_mixin.ns`,
which pins the single and the stacked case and a mixin with `<:` clauses
of its own).

**A declaration without `<:` pays nothing for them**: its full body has no
check and no applied level, and its standalone body does not exist. What
mixin application cost the walk that ran it before `new` was compiled code
is measured in [execution.md](execution.md), "What mixin application cost".
The image carries the clauses on each row and the levels as class records
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

**The root rule: only `Object`, `Value` or `Object <: Value`**
(`r[semantics.module.root-rule]`). A module's
root class may write every combination of `Object` and `Value` that works
(decided with the user on 2026-09-30). Everything after the header's
`=` is held to the rule; the primary factory before it (`new`,
`packageUsing: manifest`, `usingPlatform: platform`, …) is not. Everything
else is a root defect
(`goldens/language/module/root_superclass_sending_other_than_new_is_refused.ns`).
`Value <: Value` and `Value new <: Value` stay refused:
applying `Value` over `Value` adds nothing. A root class has no
enclosing object to evaluate a receiver in: §6.1 evaluates its clause "as if
it was a top level expression", where it "will almost certainly fail", so the
only classes a root can reach are the ones the rule names. The corpus has
such roots — `Newspeak2SqueakCompilation`'s `= NewspeakCompilation
usingPlatform: platform`, `UISessionManager`'s `= NewspeakObject new` — which
the Squeak-hosted implementation accepts; none is on `corpus/runnable.toml`.

**Every root defect is a resolve error** (decided with the user on
2026-09-30; `r[semantics.module.root-rule]`):
`resolver::root_clause_defect` classifies the root's clauses
and the resolver reports the first defect at the offending clause's span, so
the editor underlines exactly that clause. The book's appendix
lists it as a difference from §6.1, taken because a module root is not an
ordinary class and a program must not
run when its artifact cannot be built — deferring it to instantiation let a
project module nothing instantiated run under `run` while `build` refused it.
`run` and `build` print it alike. The
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
encoder refuses a module with a root defect** (`r[image.verify.root-rule]`),
in `image::package` and `image::snapshot` alike, so the format needs no wire
field and no tag for one; the
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
fields -- nine for a level a `<:` clause added, eleven for a body-less
declaration's class object -- which only the VM reads ("The object layout"
below): its application, enclosing object, declaring class and superclass,
for a metaclass `this_class`, the link back to its class, for a level the
class its clause named, and for a body-less declaration's class object its
declaration's enclosing object and declaring class ("Mixin application"
above). The three kernel classes
declare no slot for any of it and carry behaviour only, as SOMns's do
(`Class = Object ()`, `Metaclass = Class ()()` in
`prior-art/somns/core-lib/Kernel.ns`, the state in the Java `SClass`) and as
`platform/JS/InternalKernel.ns`'s do. Primordial Soup instead makes a class object an
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

**A metaclass is flat: a class side is not inherited**
(`r[semantics.class-side]`). A metaclass's
superclass is always `Class`, never the metaclass of `Foo`'s superclass, so
the ordinary lookup from a class object walks its own class side, then
`Class`, `Behavior` and `Object`
(`goldens/language/class/class-side/class_side_is_not_inherited_by_a_subclass_with_none_of_its_own.ns`).
§5.10 makes a `super` send's S the superclass of the activation's current
class, which for a class-side method is `Class`: a class-side
`super printString` answers `Class>>printString`, the class's name
(`goldens/language/class/class-side/class_side_super_reaches_class.ns`), and a
class-side `super new` does not instantiate, because nothing above the
metaclass declares a factory.

**Every class object's class is its metaclass, and every metaclass's class is
`Metaclass`** (`r[vm.heap.class-word-is-metaclass]`). `attach_metaclass`
sets both headers.
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

An object's class word is never `nil` (`r[vm.heap.class-word-never-nil]`),
because the builtin
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
`MixinBuilder.assemblePrimaryFactoryMethod`). **The entry carries its
body**, `compiler::factory_body` of its arity, which instantiates the class
object the message was sent to ("Instantiation" above):
`interpreter::resolve::act` answers it as a factory activation on the
receiver (`Mutator::decide_factory`, `Step::Factory`), with the body the
lookup found, which a cache entry records like a method's. A
flat metaclass puts a factory entry on the receiver's own metaclass and
nowhere else, and `resolve::resolved_for` honours a factory only when the
lookup found it on the receiver's own class, so no special case sits ahead of
or behind the lookup ([dispatch.md](dispatch.md), "Resolve, then act").

**A class object's factory and name are read through its metaclass**
(`Heap::class_factory`, `Heap::class_name`): the factory entry and the name
of the class-side mixin its metaclass applies, or its own mixin's while it
has no metaclass yet. For a class of its own declaration that is its
mixin's, as before; it differs where the metaclass applies another
declaration's class side than the mixin the class object applies: a level a
`<:` clause added has the class side of the class its clause named, and a
body-less declaration's class object its declaration's ("Mixin application"
above). `primary_factory_selector` reads it where §6.1 needs it, in the
checks of the superclass clause and of the `<:` clauses, and an
instantiation refusal names the class and its factory the same way.

The entry is derived, never stored twice
(`r[vm.compiler.factory-body-synthesized]`): `instantiate::build_class_side`
inserts it as a class side is built from source, `image::mixins::build`
derives it on load for a row another row names as its class side, and
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
side's `self class` is a metaclass, and a host can hand a program any of
them — so refusing to
instantiate them is needed whatever is nameable. `Mutator::apply_mixin` folds
`InstantiationRefusal::ClassObject` into the `instantiation_refusal` of
`Behavior`'s application — recognized by the mixin the kernel phase records
as `Mutator.behavior_mixin` before `Behavior` is filled — and every
application below it inherits the refusal from its superclass, so
the factory body's allocation refuses `Class`, `Metaclass`, every metaclass and any
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
- **A value class object has no identity** (`r[semantics.value.identity]`).
  Its `=` and `hash` are derived
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
record to each other both ways (`r[image.verify.metaclass-records]`;
`pair_metaclasses`, `MetaclassPairing`), which
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
  the module rather than duplicated per node (`ResolvedModule::source`).
- **Every declaration holds its own text, its nested declarations cut
  out** (decided with the user on 2026-10-19), rather than its module's
  whole text: `Mixin::text`, a `DeclarationText` (`mixin.rs`) -- the
  declaration's range in the module text it was read in, the spans of the
  declarations nested directly in it, its *holes*, and its text. A
  module's root declaration's range is the whole module, prologue and
  trailing text included, its nested declarations cut out. So no version
  of a declaration keeps another's text alive, in the heap or in an image:
  an edit of a nested class makes a fresh text for that class and its
  subtree only, and the enclosing declaration's text, which holds no byte
  of it, stays the same and valid. Chosen over keeping every module text
  a version still points into, which duplicates whole modules in the heap
  and the image, and over rebasing spans onto the current text when
  writing an image, which leaves the heap's duplicates and drops an old
  body's source.
- **One text, two forms.** A shipped module's declarations share its one
  text per thread (`interpreter/core_library.rs`'s `SHIPPED_MODULE_TEXTS`,
  which exists so that no boot copies the core library), holes and all,
  since a shipped mixin is never edited and so never outlives the text of
  another version; a program's declarations each own a copy of their own
  text with the holes cut out. Both answer the same `slice` and
  `assemble`. A class side, declared or synthesized, shares its instance
  side's text, the same `Rc`, and no builder makes one of its own. A
  hole is a nested declaration and so never empty: every form refuses an
  empty one, which only a crafted image could carry. A builtin's mixin, the VM mirror's and a mixin of an image
  without source hold none, and their `source` answers `nil`.
- **A module's text is assembled, never stored.** `Heap::assembled_text`
  fills each hole of a declaration's text with the assembled text of the
  current version of the declaration nested there, found through its id;
  for a module's root that is the module's current text, every edit in it
  included (`Heap::module_source`), which an edit starts from and a load
  hands the host as `LoadedModule::source`. An unedited module assembles
  to its file byte for byte (`an_unedited_module_assembles_to_its_text`,
  `every_shipped_module_assembles_to_its_text`). A declaration mirror's
  `source` is the assembled text sliced at the declaration's span
  (`Heap::declaration_source`), its start counted from the text's start
  and its end from its end, since an edit nested in it changes only what
  lies between.
- Every declaration already carries a `Span`, and every compiled body its
  `CompiledCode::source`, both in the coordinates of the text its version
  was read in, so source text for any node is recovered on demand, with
  no per-node copy: in the front end as `span.text(&module_source)`, and
  from a mixin through its own text's `slice`, which answers a span only
  within the declaration and in no hole. A text is checked once when it is
  made (`r[image.verify.texts]`), and slicing and
  assembling answer nothing for a span or a hole that does not fit, never
  a panic, since a text and a span read from an image are the image's to
  choose ([image-format.md](image-format.md), "Source"). A mirror's
  `source` reads it ([liveness-and-reflection.md](liveness-and-reflection.md),
  "A mixin knows its enclosing mixin and its declaration's text").
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
ends where its header's count says, not only through the space's bounds
(`r[vm.heap.access-within-object]`).

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
representation steps are built; the steps still to come are planned in
[execution.md](execution.md) and [core-library.md](core-library.md). The
road, each step landing green on its own:

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
   forward ("`become:` is an eager bulk forward" below;
   [liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors and
   MirrorBuilder"; decided with the user on 2026-10-05), in five plans:
   5a, mirrors that read, and 5b1, the VM's half of live editing -- the
   mixin id that carries every version, the forward and the recorded
   guards -- built; 5b2a, the MirrorBuilder and the atomic install,
   built; 5b2b, declaration texts and edited programs in images, built;
   5b2c, the reclaiming of ids, built.
6. **Inline allocation in the interpreter** -- built ([memory.md](memory.md),
   "Inline allocation"; "Instantiation" above).
   - **After road step 6, `new` as compiled code** -- built: the primary
     factory and the initializers run as code the compiler emits, as both
     references do ("Instantiation" above).
7. **The execution model for compiled code** ([execution.md](execution.md),
   "Planned: JIT tiers").
   - **Between road steps 7 and 8, real Newspeak bodies for `whileTrue:`,
     `whileFalse:` and `do:`** -- built ([core-library.md](core-library.md),
     "Real bodies for control flow and `do:`").
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
`pointer_ranges`); a new object's header is written in place in the
space in one of two places, `Heap::allocate_block` on the cold path and
`Space::bump_object` (and its fixed-size form) on the fast one
([memory.md](memory.md), "Inline allocation"); `heap/header.rs` holds `Header`
and `Format`.

**The header word**, after Spur's (`headerForSlots:format:classIndex:`) and
HotSpot's compact object headers (JEP 450, a product feature from JDK 25 as
JEP 519), which fit a class reference, the identity hash, the GC age and
the lock bits into one 64-bit word:

| Bits | Holds |
|---|---|
| 0-21 | the application id -- the inline caches' guard ([dispatch.md](dispatch.md), "The dispatch id"); 0 is `NONE`, 1 `VACANT`, 2 reserved (`FORWARDED`), and the allocator mints from 3 |
| 22-26 | the format |
| 27-30 | the age: the scavenges a young object has survived, at most 15; 0 for a new object and meaningless for an old one ([memory.md](memory.md), "The age table and the tenuring threshold") |
| 31 | the remembered bit: set on an old object in the remembered set, never on a young one ([memory.md](memory.md), "The remembered set and its limit") |
| 32 | the negative bit of a `LargeInteger`, as Primordial Soup's `kNegativeBit` (`prior-art/primordialsoup/vm/object.h:48`); in an ordinary instance (`FixedPointers`), the under-construction mark of an instance of a value class, set by its factory's allocation and cleared by its `CompleteInstance` (`Heap::object_under_construction`, `r[vm.heap.under-construction-only-on-value-instances]`) -- the two meanings share the bit because no object has both formats, and every reader asks the format first |
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

**The reserved guard values sit at the low end**
(`r[vm.dispatch.reserved-guards]`), as Primordial Soup's
reserved class ids do (`kIllegalCid = 0`, `kForwardingCorpseCid = 1`,
`kFreeListElementCid = 2`, `kFirstLegalCid = 3`,
`prior-art/primordialsoup/vm/object.h:64-68`), and Spur keeps its own
puns below 32 (`isFreeObjectClassIndexPun` 0, `isForwardedObjectClassIndexPun`
8, `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:2861-2863`):
`DispatchId::NONE` is 0, `VACANT` 1 and `FORWARDED` 2, and the allocator
(`heap/ids.rs`'s `DispatchIdAllocator`) mints from 3 below 2^22, answering
`NONE` once every id is in use ("An application has two ids" below). So a
header word left zero by a missed initialization carries `NONE`, which is
never cached, rather than a guard some application was minted, and the
debug build's recomputation in `Heap::dispatch_id` reports it (decided with
the user on 2026-10-03). `FORWARDED` is the guard the bulk forward stamps on
each forwarder it makes (`Heap::forward_identity`), never minted, so that a
cache could never hit on a forwarder even if a reader met one -- and none
does, since the forward is eager and leaves no handle naming a forwarder
("`become:` is an eager bulk forward" below).

**The identity hash is 22 bits wide**, drawn from a counter that wraps
within them, as Strongtalk's wraps within 20 (`markOop.cpp`), and skips 0.
Hashes are therefore not unique once the counter has wrapped, which a hash
need not be; the decoder advances the counter past the largest hash an image
holds and refuses a wider one (`r[image.verify.hashes]`, `IdentityHashTooWide`;
[image-format.md](image-format.md), "Identity hashes are preserved
verbatim").

**An application has two ids.** Its *guard id*, 22 bits wide, is the
inline caches' guard: it stands in object headers and in inline- and
global-cache entries -- and in recorded guards (see "What a slot access
checks" below) -- and nowhere
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
sight (`Heap::mixin_id`, `register_application`, `code_id`). Every version
of a mixin answers the id it was first registered under
(`r[vm.dispatch.mixin-id-one-per-mixin]`): the row holds the
current version, the id of the mixin it is declared in and the ids of those
declared in it (`Heap::nested_of`), and `Heap::replace_mixin` installs a
fresh version under an old id, keeping the one it supersedes in
`Tables::superseded`, so the address map that answers a version's id
(`mixin::mixin_key`) never names a dropped one
([liveness-and-reflection.md](liveness-and-reflection.md), "An edit
replaces the mixin under its id"). The ids are per heap and never on the
wire: the image encoder numbers its own records and bodies.

**Each id has its own reuse rule, and a full collection enforces them**
(`r[vm.heap.id-reclaimed-only-when-unnamed]`). A
guard id is reusable once no object header, no inline- or global-cache
entry and no recorded guard -- an activation's, a slot accessor's cache
entry's or the caches' verified pair -- names it; a full-width index once
no class object, no activation's entered level, no application-cache key
or value and no other application's superclass link or successor entry
names it; a mixin id once no application, no class object's applied-clause
field, no reified mixin object and no holder of one of its versions names
it or any other id of its tree -- its module's, or the obsolete tree a
declaration an install removed roots, freed on its own; a code id once no closure or
frame object names it. A class's change of application restamps every
instance's header by a heap walk (`Heap::class_set_application`;
`r[vm.dispatch.header-guard-stamped]`) before
the old guard id can become reusable. `Mutator::reclaim_ids`
(`interpreter/reclaim.rs`) applies the rules. `World::collect` runs it
between marking and compaction, after `prune_canonical_symbols` and
`prune_mixin_objects`: it reads the mark bits, and nothing in it moves or
allocates an object. A scavenge frees no id. The design is Spur's, whose
class table is weak and whose full collection frees the unmarked classes'
indices (`expungeDuplicateAndUnmarkedClasses:`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:60085-60127`), and
Primordial Soup's, which keeps freed class ids on a free list
(`prior-art/primordialsoup/vm/heap.cc:1022-1070`); decided with the user on
2026-10-03, its details on 2026-10-20.

**What names an id is gathered while marking.** `drain_marking` already
matches on each object's format; for a class object it records the rows
its `APPLICATION` and `APPLIED_CLAUSE` fields name, for a closure its
`CODE_ID` and `FEEDBACK_BASE`, for a frame its `CODE_ID`, in the marker's
bit sets (`heap/reclaim.rs`'s `GatheredIds`), so the pass needs no second
walk of the heap. A word that is no `SmallInteger` names nothing, as the
field's readers refuse it. The activation stack holds ids as numbers no
marking reaches, so `Heap::stack_roots` reads every activation's entered
level, recorded guard and feedback base. A Rust holder of a mixin version
-- a host's `LoadedModule`, the world's `value_mixin`, a nested class's
creation underway, a cache entry -- is never listed: the pass compares the
version's `Rc::strong_count` with its holders inside the tables.
`Heap::mixin_objects` is weak ([memory.md](memory.md), "Weak references"):
a reified mixin object that survived the marking keeps its mixin's id, and
one nothing reaches is dropped, since nothing can compare it any more.

**The order.** Each step releases the `Rc`s the next one counts.

1. **Applications.** Live: every gathered index, every activation's
   entered level, and every application whose guard an activation
   recorded -- a receiver forwarded under that guard must never meet it
   minted again -- closed over superclass links and successors. A
   successor entry is kept only while its old index lives, and then keeps
   its new one (`Heap::retain_successors`), so an index minted again never
   inherits a successor. Every other row is freed with its index and its
   guard id (`Heap::free_application`).
2. **Caches**, only when step 1 freed something. The application cache
   loses every entry whose value or superclass key was freed; the inline
   caches, the polymorphic blocks, the global cache and the verified pair
   lose exactly the entries that name a freed guard id or index
   (`InlineCaches::forget_freed`; [dispatch.md](dispatch.md),
   "Invalidation"). A debug build asserts that nothing else holds a freed
   application.
3. **Mixins** (`reclaim_mixins`). A module's mixins live and die as one
   tree (`Heap::module_trees`): its root, its nested declarations at every
   depth and the class sides registered under any of them. A tree lives
   while any of its ids is named by a live application, a gathered applied
   clause or a surviving reified mixin object, or while any of its
   versions, current or superseded, is held outside the tables
   (`Heap::held_mixin_ids`). The tables' own holders are counted
   structurally rather than listed: a version's row or its place in
   `superseded`, the `class_side` of a version of the same tree, and the
   `metaclass_mixins` memo's entry for a synthesized class side of the
   same tree; a holder in another tree counts as one outside. The trees
   and each version's count of holders inside (`Heap::inside_counts`)
   derive from the tables and the memo alone, so they are kept across
   collections (`MixinReclaimMemo`, `interpreter/reclaim.rs`) while the
   tables' generation (`Tables::mixins_changed`, raised by every
   registration, replacement, free, drop and truncation) and the memo's
   (`MetaclassMixins`, through which alone it changes) stay; a collection
   then decides which versions are held by one pass of
   `Rc::strong_count` against the counts. A debug build holds a kept memo
   to the tables' lengths at every collection and to one derived afresh at
   its first reuse and every eighth (the reuses count afresh whenever the
   memo is derived), which the collect stress mode turns into a check at
   millions of collections. The trees are not walked at all while every
   witness of the last walk stands (`Witnesses`): the two generations
   unchanged since, no application freed by step 1, and every id the
   gathered applied clauses, the surviving reified mixins and the held
   versions named then named again, so that every tree that lived then
   lives by the same id now. A tree dies
   whole because a live mixin names the ids of the mixins it is declared
   in and of those declared in it, and a holder that presents a version
   again must find it under the id it had, so a version a host holds
   keeps its whole tree. A dead tree's rows are freed, its superseded
   versions and their `mixin_ids` entries dropped (`Heap::free_mixins`),
   and its memo entries removed; a debug build asserts that no dropped
   version is held anywhere else. Then the superseded versions of live
   ids: one is dropped once only `superseded` holds it and no body that a
   marked closure or frame, or a running activation, names was compiled
   in it (`Heap::running_bodies`, `drop_superseded`), because such a
   version is what admits that code into an image of its module
   (`image::extra_bodies`).
4. **Codes.** Every code id no marked closure or frame, no running
   activation, no live optimized copy that inlined its body
   (`jit::inlined_code_ids`) and no breakpoint that patched its body
   (`Breakpoints::patched_code_ids`; [debugger.md](debugger.md),
   "Breakpoints by body, and reclaimed code ids") names is freed with
   its `code_ids` entry (`Heap::free_unnamed_codes`), and the heap's
   code-table epoch is renewed, so no body's cached id names a freed row
   ([memory.md](memory.md), "A closure's code id without the map"). A
   running activation names its body by id in its stack frame's code word,
   a number no marking follows, so `Heap::stack_roots` lists them (`StackRoots::codes`);
   a suspended one's frame names it, and the side table of suspensions is
   a root. The body lives on with whoever else holds it, and a later push,
   closure or frame registers it again.
5. **Feedback ranges**, whether or not anything above was freed, since a
   body steps 3 and 4 let go of can end a family on its own: a family whose
   root body only the caches hold and whose base no marked closure and no
   activation names is dropped, its sites vacated and its range freed
   (`InlineCaches::forget_families`; [dispatch.md](dispatch.md), "3b — The
   caches").

**The smallest free id is minted first.** Each table is a
`Vec<Option<…>>` with its free rows (`heap/ids.rs`'s `FreeIndices`):
registering takes the smallest free row, else appends, and the pass
truncates every table by its trailing free rows (`truncate_trailing`), so a
table stays dense and a program that edits, installs and collects in a loop
keeps its tables at a bounded size -- Spur likewise lowers its class
table's next index to the smallest freed one. The guard ids follow the same
rule (`DispatchIdAllocator`): the smallest freed id, else the next never
minted, below 2^22. Once every one is in use, `Mutator::mint_dispatch_ids`
answers `NONE`, so the new application runs uncached, and requests a full
collection at the next safepoint (`Heap::request_collection`), as Primordial
Soup collects before growing its class table (`AllocateClassId`,
`heap.cc:1279-1310`): nothing but the old space's growth would otherwise
start one, and a program that makes and drops applications quickly would
run out with plenty of dead ones to reclaim. The feedback ranges take the
first free range that fits ([dispatch.md](dispatch.md), "3b — The caches").
`interpreter/reclaim.rs`'s tests pin it: a hundred edit-install-call-collect
rounds grow no table past its size after the second round, and a guard
minted again misses every cache entry filled for its dead application
under both lowerings.

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
| 5 | forwarded | a forwarder -- the scavenger's for a young object it has copied, and the bulk forward's for each object it forwarded, its guard `FORWARDED` ("`become:` is an eager bulk forward") -- whose class word holds the new location; once the scavenge or the forward that made it ends, nothing names it, so no send, accessor or reader outside the two ever meets one, and a bulk forward's stays in the space as garbage until the next scavenge (a young one) or full collection (an old one) |
| 6 | free | a gap a collector leaves, walkable like an object; no object has it, since the sliding compaction and the scavenge leave no gap; a debug build writes it as the poison over the nursery a scavenge vacated, and `Heap::block` refuses it there ([memory.md](memory.md), "Finding a missed barrier: the scavenge stress mode") |
| 7 | class | a class object: eight fixed fields, nine for a level a `<:` clause added, eleven for a body-less declaration's class object |
| 8-9 | 32-bit-indexed | a wide string or symbol |
| 10 | weak pointers (`WeakPointers`) | a `WeakArray`'s slots, forwarded like any pointer words but not traced for liveness ([memory.md](memory.md), "Weak references") |
| 11 | ephemeron (`Ephemeron`) | an `Ephemeron`: its key, then the value and the finalizer, traced only once the key is reached and set to `nil` when it is not ([memory.md](memory.md), "Weak references") |
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
takes the target, as Spur's forwarders take their first slot
(`r[vm.heap.no-forwarder-reached]`).

**A class object has a format of its own** (`r[vm.heap.class-by-format]`),
where Spur's and Primordial
Soup's class objects are ordinary pointer objects, because whether an object
is a class is asked of untrusted objects: the image decoder's verification
and its lookups ask it of every object a record names
([image-format.md](image-format.md), "Verification"). With a format of its
own, `Heap::is_class` reads the object's header alone, rather than following
its class word to the class and that class's to the metaclass, through
objects a crafted image may not have made what they claim to be.

**The format decides what the collector traces**
(`r[vm.heap.format-decides-pointer-words]`,
`r[vm.heap.non-pointer-words-untraced]`): `object.rs`'s
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
`SmallInteger`, the class's position in `BuiltinClass::ALL`, or `nil`; the
factory body's allocation re-reads it on every instantiation,
[dispatch.md](dispatch.md), "Every target is a function of its key and its
site"),
its value-class refusal (`nil`, or a two-element `Array` the VM alone reads:
the offending enclosing object and the name of the level at fault as a
`Symbol`, or `nil` for the class's own enclosing object), for a level a `<:`
clause added the clause that added it (the applying declaration's mixin id,
the clause index and the named builtin tag packed into one `SmallInteger`, or
`nil`, since an `Rc<Mixin>` cannot live in the words), and its application
as a `SmallInteger` full-width index, separate from the 22-bit guard id in
instances' headers; the behaviour stays Rust. Then three fields only some
class objects use, `nil` on the others: for a level a `<:` clause added the
class object its clause named (`NAMED_CLASS`), and for a body-less
declaration's class object its declaration's enclosing object and declaring
class (`DECLARATION_ENCLOSING`, `DECLARATION_DECLARING_CLASS`; "Mixin
application" above); and last the creation stamp (`CREATION_STAMP`), a
`SmallInteger` drawn from the heap's counter when the class object is made
and kept by every move and by an image, which orders class objects by their
creation -- the order in which an install evaluates superclass clauses
([liveness-and-reflection.md](liveness-and-reflection.md), "The complete
install", decision 10). Every class object has all twelve fields, 112 bytes with
its header and class word, so that an install can turn a class object of a
declaration with a body into a body-less declaration's and back in place,
keeping its identity. The builtin tag, the applied clause, the application
and the stamp are numbers, the other eight fields objects or `nil`, and the
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
`SmallInteger` (`AppliedClause::pack`, `unpack`, `mixin.rs`). The clause
limit, 1,024 `<:` clauses in one superclass clause (`CLAUSE_INDEX_LIMIT`;
[compiler.md](compiler.md), "Limits"), sits far below the 23 bits, which
keep their headroom: the resolver refuses a source past it with an ordinary
resolve error naming the limit, and the decoder an image past it
(`r[image.limit.clauses]`, `r[image.limit.clause-index]`; `ClauseCountTooLarge`,
`ClauseIndexTooLarge`). It is a stated limit of this implementation, set
340 times above the largest golden, and the one bound on a clause count:
the class object's field and the initializer's `u16` step count are never
reached.

The application cache is keyed by the mixin's id and the superclass
application's full-width index rather than by `Rc` addresses, which lets a
full collection prune it by the ids it frees ("An application has two ids"
above). A closure keeps its code id, captured frame and feedback base in
three fixed fields (`closure_field`) and its copied values inline; a code id
indexes the heap's table of compiled code, whose row a full collection
frees once no closure or frame object names it. Each entry also records the code's owning
`MixinMethod`, where it has one, so a method frame's code id names its
method too -- the declaration the encoder writes and `frame_body` reads.

**A reified frame** is an object of the frame format whose class is the
kernel-private `Activation`. Ten fixed fields (`frame_field`) keep its
sender, home, receiver, current class, program counter, code id (`nil` for a
top-level frame), kind (the kind's wire tag), the kind's operand -- the
closure of a block frame, or the class of an initializer, superclass-clause,
superclass-message or mixin-application-message frame, a handle the trace
sees -- `uncontinuable`, and its married word; its registers follow
inline. The married word tells the frame's three states
(`r[vm.activation.marriage]`): a `SmallInteger`
of 0 or more is the offset of the stack frame it is married to, a negative
one, `-(row + 1)`, names the side-table row of its suspended activation,
and `nil` means it has returned ([execution.md](execution.md), "Suspending
and resuming"). The `Activation` object the exception machinery holds is
this frame object itself, so the same activation answers the same object
every time; a running activation is a stack frame, or is suspended into
its frame until something returns into it. A frame object is allocated at
`reify` (`r[vm.activation.frame-object-full-register-file]`)
with its code's full register count but the header's numbers,
whatever the code: a field for each argument, then one for each register
above the header, so register `r` is field `r` below the argument count
and field `r - HEADER_REGISTERS` above the header, and a number the
header's words take, which no instruction names, has none
(`Heap::frame_field_of_register`; [compiler.md](compiler.md), "Registers").
A frame of a body past the register ceiling whose arguments leave no
number above the header holds its arguments alone. Whether the
divorce copies the registers into it is decided per activation from the
block's `outer_reach` ([execution.md](execution.md), "Closure conversion").
Suspension, the debugger and
edit-and-continue find room for the registers in every reified frame.
Edit-and-continue adding locals will divorce the affected activation into a
newly allocated, larger frame object -- allocated old and born remembered by
the rule in [memory.md](memory.md), "Generations" -- the VM
rewriting every reference to the old frame by a walk of every root the
collector traces (`trace.rs`) and every heap object, the same walk as the
bulk forward ("`become:` is an eager bulk forward"
below); execution continues from the new frame as from any suspended one
([execution.md](execution.md), "Suspending and resuming", "How this meets
the liveness requirements").
The cost is a few more words per reified frame, and `bench_block.ns`, which
reifies a frame per iteration, is the workload that shows it. While a frame
is married its registers live on the stack and the trace skips the object's
inline copies. `FrameReference` is the type of a handle to a frame object,
so an `ActivationReference` is still never stored, and a frame has no store,
forwarding table or register vector of its own.

**Strings have two widths** (`r[vm.heap.string-canonical-width]`). A string
holds one byte per code point while
every code point fits Latin-1, 32 bits per code point otherwise -- the
byte-indexed and 32-bit-indexed formats, eight or two units to a word
(`object.rs`'s `pack_string` and `pack_code_points`, the
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
`Mutator::new_symbol` and `Mutator::program_symbol` are the only ways to make
one (`r[vm.heap.symbol-canonical]`): instantiating `Symbol` is refused, and
`String new` answers an empty one-byte string
(`goldens/library/string/symbol_new_is_refused.ns`,
`string_new_answers_an_empty_string.ns`). `=` is class-tight both ways:
`'foo' = #foo` and `#foo = 'foo'` both answer `false`, since `Symbol>>=` is
identity and `String>>=` answers `false` for a `Symbol` argument
(`a_symbol_is_a_string.ns`).

**What an accessor lends.** A string's units are read in place, and a
`LargeInteger`'s digits are lent in place to `=`, `hash`, the comparisons and
the sign tests; arithmetic still copies its operands' digits out
([core-library.md](core-library.md), "Large integers"). Every field,
element and register accessor indexes the object's own block
(`r[vm.heap.access-within-object]`), and an out-of-range index that
a valid program produces raises an ordinary Newspeak error, never a panic
(`r[vm.heap.field-access-total]`).

### What a slot access checks

Built by road step 5b1 (2026-10-18,
[its plan](../superpowers/plans/2026-10-18-1236-become-and-recorded-guards.md)).
A slot access that is not guarded by the receiver's own header checks a
*recorded guard* (`r[vm.dispatch.slot-guard-checked]`,
`r[runtime.slot.guard]`). Such an access reads or writes a field at an index
decided without looking at the receiver: the own-slot instructions
`InitializeSlot`, `InitializeSlotProvisionally`, `LoadOwnSlot` and
`ReturnSlotIfFilled`, which address `slot_base + own_index` in `self` with
no send (`bytecode.rs`, `vm.rs`), and a slot accessor acted on through
`Chain::Private` or `Chain::Super` -- a private lexical send, one to an
enclosing object included, resolves on the lexical class to a field index
(`resolve_lexical`, `interpreter/resolve.rs`; `decide_slot_accessor`), a
super send on the superclass -- and a slot accessor that §5.10 makes the
handler of a super send nothing understands (`Chain::SuperDoesNotUnderstand`),
whose field is decided from the superclass's chain too; and the store of
a nested class into its enclosing object's hidden cache slot, and the
reset of the slot's in-progress marker, at an index decided before the
class's superclass clause ran. The bulk forward
("`become:` is an eager bulk forward" below) rewrites the receiver of every
running activation on a forwarded object to its replacement, while the
activation goes on running the method or initializer it was entered in,
compiled against the old layout; the check is what keeps the rest of such
an activation, and a cached private or super accessor, from writing the old
layout into an object of another shape. The forward can come at any send,
so it can come *before* a push as well as during an activation: a private
lexical send, a super send and an initializer instruction's level bind
their code without looking at the receiver, so nothing about the binding
vouches for the receiver's shape.

**The own-slot instructions.** When `Heap::push_activation` pushes code
that addresses its own slots (`CompiledCode::addresses_own_slots`, derived
from the instructions as `outer_reach` is and never on the wire), the stack
entry records the full-width index of the application it was entered at,
`entered_level` -- the application of its `current_class`, `UNREGISTERED`
with none -- and, as `recorded_guard`, the receiver's guard only when the
receiver's chain contains that level (`Heap::receiver_has_level`, which
answers at its first compare when the receiver's own class is the level
and walks only for an inherited one), `NONE` otherwise, as a cache fill's
`slot_guard` does. Recording the guard unverified would let a receiver
the forward replaced before the push -- by a lazy getter's private or
super send, or by a `<:` clause's message between two levels of an
instantiation -- vouch for itself. Every other push tests that one flag
and records `NONE`. Every own-slot instruction
first calls `interpreter::locals::check_own_slots`: the hot path is one
header load and one compare of the receiver's guard with the recorded one.
On a mismatch, or with `NONE` recorded, the cold path asks whether the
receiver's application chain still contains the entered level
(`Heap::receiver_has_level`, one walk of the receiver's class and its
superclass links comparing full-width indexes): if it does -- the receiver
was forwarded to an instance of a subclass -- the check records the
receiver's current guard, so the accesses after it take the hot path again,
and the access proceeds; if not, it raises an ordinary Newspeak error,
which `on: Error do:` catches (`r[runtime.slot.guard]`), its receiver
`Heap::describe` of the receiver and its class the entered
level's mixin's name; the error's selector is the method's for a lazy
slot's getter and the entered level's factory selector otherwise, as the
§3.1.1 slot refusal's is. An immediate receiver passes, and the access
answers its usual error. A receiver replaced by another instance of the
level's own application keeps the guard and never reaches the cold path.
Machine code makes the same check, the compare inline and the cold path
through a leaf that records the guard in the frame, and reads the slot
base from the entered level's row of the application id table
([execution.md](execution.md), "The baseline JIT's foundation", "Own
slots in machine code").

The level is the one the activation was *entered* at, by its full-width
`ApplicationIndex`, not the class object's *current* application: the
atomic install rewrites a class object's application in place while an
activation entered before it still runs code compiled against the old
mixin's slot list. So the slot base comes from the same place
(`Heap::activation_slot_base` reads the entered level's `own_slot_base`),
and the check and the address agree by construction. An own-slot
instruction in code whose flag is not set -- which only a crafted image
could hold -- runs with `NONE` and `UNREGISTERED` and raises for every
object receiver, as does own-slot code run in an activation with no
`current_class`.

**The entered level's successor** (`r[vm.dispatch.entered-level-successor]`;
road step 5b2a, decided with the user
on 2026-10-18; [liveness-and-reflection.md](liveness-and-reflection.md),
"A MirrorBuilder and an atomic install"). The install builds new
applications for every class it reaches, even for an edit that changes
only methods, since an application holds the mixin version it was built
from. For each old level whose slot base and own slots, by name and in
order, the edit keeps, it records the new application as the old one's
successor (`Heap::record_successor`, into `Tables::successors`), pointing
every earlier predecessor at the newest; the cold path of
`Heap::receiver_has_level` accepts the entered level or its successor; the
hot path is unchanged. A successor entry lives while
its old application does and keeps the new one alive; a full collection
drops it with the old index ("An application has two ids" above).

**Private and super slot accessors** (`r[vm.dispatch.slot-guard-checked]`).
`Lookup::SlotAccessor` answers the
level whose own slot the field is, and `Resolved::Slot` and
`Target::Slot` carry it with a recorded guard. Every fresh lookup -- the
uncached path and a site's miss alike (`resolve::resolved`) -- carries
`DispatchId::VACANT`, which no header holds, so an uncached access always
walks and the reference lowering stays the uncached oracle. A cache fill
records the receiver's guard when the receiver has the level, `NONE`
otherwise (`inline_cache.rs`'s `slot_guard`); public and protected entries
record `NONE` and check nothing. `act`'s slot arm, for a private or super
chain, calls `Mutator::slot_level_holds` (`interpreter/slots.rs`, out of
line): the entry's guard compared with the receiver's, then the caches'
*verified pair*, then `Heap::receiver_has_level`. The verified pair is one
(guard, level) pair per heap (`InlineCaches::verified_slot_level`): a walk
that passed on a cached path records it, it is never recorded or answered
for `NONE`, and invalidating the caches clears it. It records a
verification the walk made and decides nothing, on the same assumption as
an activation's recorded guard. A failed check raises:

> `<receiver> no longer has the slots of <class>, which the slot accessor <selector> addresses`

A super send whose handler is a slot accessor is checked before `act`
(`Mutator::check_super_handler_slot`, from `decide_super_send` and `miss`),
walking every time, and `record` never caches such a handler, so no hit
bypasses the check.

**What the shape cost.** Measured against master (`4c6fe489`) and between
the tasks, three pinned release builds a side: every program within noise,
`bench_megamorphic.ns` at +1.3-2.1%, at the edge of build noise and not
diagnosed ([OPEN-WORK.md](../OPEN-WORK.md), "Next"). Two shapes measured
worse and were not built. Any heap read inlined into `act`'s slot arm slowed
every slot accessor, public ones included, by 7-11%, so the check is out of
line. Without the verified pair a site whose entry recorded one class's
guard walked for every receiver of another: DeltaBlue's `Plan` subclasses
the collections' `List`, whose methods read `List`'s private slots, and the
walk ran about ten million times a run, +3.9%; rewriting the recorded guard
of the entry that hit instead cut the walks but carried the entry's
location through every cached send and cost every program 2.5-7.4%.

**The assumption** (`r[vm.dispatch.invalidation-after-change]`). A guard
names one application, and an application's
chain does not change while the caches persist:
`Heap::class_set_application` on a superclass is the case that changes it
under an unchanged guard, and the atomic install vacates, scoped, every
cache entry and optimized copy decided for an application it re-applies
after it ([liveness-and-reflection.md](liveness-and-reflection.md),
"Caches and their dependency keys"). Recorded guards follow the
guard id's reuse rule: an activation's recorded guard keeps its
application, and so the guard, from being reclaimed, and a collection
that frees a guard vacates every cache entry and the verified pair that
name it ("An application has two ids" above).

**A reified frame has no field for it**
(`r[vm.activation.resume-derives-state]`). The recorded guard and the
entered level live in the activation's stack frame; while the activation
is suspended, its entered level lives in its row of the heap's side table
and goes back into its stack frame when it is resumed
([execution.md](execution.md), "Suspending and resuming"). A resumed
activation starts with `NONE` (`Heap::resume_activation`), so its first
own-slot access takes the cold check, and a passing cold check records the
receiver's current guard. Reading the guard afresh from the header on
resume instead would defeat the check: after a bulk forward it would read
the replacing object's own guard, which always matches that object. One
cold check per resumed activation buys eight bytes in every reified frame,
which `bench_block.ns` reifies once per iteration (decided with the user
on 2026-10-03).

The tests are `builtins/reflection.rs`'s, under both lowerings: an
initializer, a lazy getter, a simultaneous clause, a private and a super
slot read, a cache hit and a super slot handler each raise after their
receiver was forwarded to an object of another shape, and an initializer
forwarded to its own application or to a subclass's instance proceeds. A
private and a super lazy getter pushed after their receiver was forwarded
raise from `ReturnSlotIfFilled`, as does an applied level pushed after
its `<:` clause's message forwarded the instance.

**The nested-class cache slot**
(`r[vm.dispatch.nested-cache-slot-relooked]`). A nested class is cached in a
hidden slot
of its enclosing object, whose index `cache_slot` (`interpreter/nested.rs`)
decides from the enclosing object's layout before the superclass clause
runs, and where `begin_creation` writes the in-progress marker: the serial
of that one creation, a `SmallInteger` taken from
`Mutator::next_creation_serial`, which a class object never is, so any
`SmallInteger` in the slot says a creation is underway
(`is_creation_in_progress`). The clause
is user code on the enclosing object and can forward it. So `after_clause`
looks the slot up again, by the declaration's id in the enclosing object's
current application, before it builds the class, and reads what that slot
holds, as the accessor's first step does. An object that still has the
declaring level -- forwarded to an instance of a subclass, say -- keeps the
one class of the declaration it has: a class already cached there is
answered, a creation of its own underway further down the stack raises the
cyclic-definition error, and only an empty slot takes the class built here,
whose declaring class (`current_declaring_class`) is the one the lookup
chose before the clause ran, if the replacement's chain still holds that
very class object, found by identity -- which keeps the level the
original lookup chose, for `super Inner` one below the topmost declarer
-- and otherwise the level of the replacement's chain whose mixin
declares the declaration, by id -- another class object than the
forwarded object's when the replacement was built in another enclosing
instance -- so that the class's `outer` walk follows the chain of the
object it is cached in. One that has not gets no class, and the access
raises an ordinary Newspeak error in the own-slot check's words, since
the class would have neither a slot to be cached in nor an enclosing
object with the declaring level's slots to
run in:

> `<receiver> no longer has the slots of <class>, which the nested class <name> is cached in`

`<class>` is the declaring level's mixin's name and `<name>` the
declaration's; the error's selector is the declaration's name.
A cache hit reads the slot at the index its entry recorded, without the
two hash lookups, only for a receiver whose layout holds that index --
always on a public or protected site, whose guard is the receiver's
application; on a private one after a compare of the receiver's id with
the guard, N's application; and on a `super` one after a compare with the
running class's application, whose chain contains S, since a hidden slot's
index is the same in every application over the one that declares it
([dispatch.md](dispatch.md), "An entry"); a forwarded enclosing object of
another shape misses the guard or fails the compare, and takes the
accessor's lookup.
`end_creation` resets the marker to `nil` only while the slot it was
written to is still the slot the current layout has (`current_cache`) and
still holds its own serial: a replacement's own field at that index is
left alone, and so is the marker of a creation the replacement has
underway at the same index, whose clause is still on the stack and which
resets it itself -- so a cyclic definition in that creation's clause is
still diagnosed. The tests are `builtins/reflection.rs`'s, under both
lowerings.

## `become:` is an eager bulk forward

`become:` exists as one bulk, one-way forward of an array of old objects to
an array of new ones, done eagerly, as Primordial Soup does, and only
mirrors reach it: it is a leaf on the VM mirror. Built by road step 5b1
(2026-10-18,
[its plan](../superpowers/plans/2026-10-18-1236-become-and-recorded-guards.md));
its caller is road step 5b2a's atomic install, which calls the forward on
the Rust side, from its own leaf, with no safepoint between rebuilding the
classes and forwarding their instances
([liveness-and-reflection.md](liveness-and-reflection.md), "A
MirrorBuilder and an atomic install"), so the leaf has no caller in
shipped code, and only tests reach it, handing the VM mirror to a test
program from the host, which no program can do. Eager forwarding and its reach only through mirrors were
decided with the user on 2026-10-05.

**The forward** (`r[runtime.forward-identity]`,
`r[vm.heap.bulk-forward-atomic]`). Primordial Soup's `Heap::BecomeForward`
(`prior-art/primordialsoup/vm/heap.cc:1072-1229`) validates both arrays,
turns each old object into a forwarding corpse after copying its identity
hash into the new one, then forwards the class ids (`ForwardClassIds`), the
roots (`ForwardRoots`) and the whole heap (`ForwardHeap`), rebuilding the
remembered set as it goes; the interpreter clears its lookup cache in the
prologue every such pass runs (`GCPrologue`,
`prior-art/primordialsoup/vm/interpreter.cc:1906-1908`), and primitive 183,
`Heap_becomeForward`
(`prior-art/primordialsoup/vm/primitives.cc:2271-2281`), is its only entry.
In ns-next the leaf is the `MIRROR` row
`elementsOf:forwardIdentityToElementsOf:ifFail:`, reported
`forwardIdentityTo:` since a row's leaf arity is its reported selector's
(`ReflectionPrimitive::ForwardIdentity`, `builtins/reflection.rs`'s
`forward_identity`). It takes two `Array`s, answers `nil`, and declines --
so the `ifFail:` block runs -- on any refusal, having changed nothing.
`Heap::forward_identity` validates every pair before it writes anything
("What the forward refuses" below); then, pair by pair, the new object
takes the old one's identity hash, so that an identity-keyed collection
still finds what it holds (the last pair's, for a merge), and the old
object becomes a forwarder -- the forwarded format, the guard `FORWARDED`,
the new object in its class word, its word count kept so the space stays
walkable. Then one walk rewrites every handle: `forward_everything`, the
walk the compaction and the move stress mode run, through a fourth
`Relocation`, `IdentityForward`, which answers each old object's
replacement and every other offset unchanged, over every handle the heap
holds -- the activation stack, the reified mixins, the host roots and the
symbol table among them -- and every heap object, through the same
knowledge of pointer words the collector uses (`heap/object.rs`'s
`pointer_ranges`); `world::forward_identity` then forwards the step state's
own roots (`StepRoots`) through the answer. The walk writes words without
the barrier, as the collector's own rewrites do, and keeps the remembered
set itself: after each holder's words are rewritten,
`IdentityForward::holder_done` says whether one of them names a young
object now, and every such old holder is remembered
([memory.md](memory.md), "The write barrier"). Last, the inline caches are
flushed: all of them for the leaf (`InlineCaches::invalidate`, the verified
pair of "What a slot access checks" above included), and for an install's
own forward exactly the entries naming an application it re-applied, whose
instances are all it forwards (`heap::Invalidation::Stale`,
`InlineCaches::forget_stale`; [dispatch.md](dispatch.md), "Invalidation").

It runs inside the leaf, in the middle of a step, not at the next
safepoint: no instruction runs between the forward and the leaf's answer,
so it is atomic. Nothing live moves -- the forwarded objects become garbage
-- so no move epoch changes, as none does for `Heap::class_set_application`'s
heap walk, which also runs inside a step (decided with the user on
2026-10-18). Once the walk ends nothing names a forwarder: a young one
leaves the space at the next scavenge, an old one at the next full
collection. An old forwarder already in the remembered set stays there
until its target is tenured or a full collection runs, its class word the
only word a scavenge reads of it. The walk is the one the divorce of a live
frame for edit-and-continue needs ("The object layout" above), so the two
share one mechanism. The tests are `heap.rs`'s and `heap/world.rs`'s (every
kind of holder rewritten, the hash copied, refusals changing nothing, an
old holder of a young replacement remembered, a forwarder garbage
afterwards) and `builtins/reflection.rs`'s for the leaf.

**What the eager forward spares.** Since nothing names a forwarder once the
forward ends (`r[vm.heap.no-forwarder-reached]`), nothing outside it ever
meets one. No send entry unforwards
its receiver; a super or private lexical send's cache hit needs no check of
its receiver for the forwarded format; no heap accessor has a cold path
that follows a forwarder; `==` compares two handles and nothing more; no
leaf fails on a forwarded argument to be retried after following it; the
`FORWARDED` guard a forwarder carries is never compared against anything
("The reserved guard values sit at the low end" above); and the image
encoder, `Heap::describe` and the host roots see only live objects. Spur's
lazy forwarders, which stay in the heap after
`become:with:twoWay:copyHash:`, pay for every one of these: a read barrier
on send through the forwarder's class index, an explicit one in super sends
(`ensureReceiverUnforwarded`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:6974-6980`: "To
maintain the invariant that all receivers are unforwarded we need an
explicit read barrier in the super send cases"), and primitives that
unforward their arguments on failure. Each reader that forgets its check is
silent wrong output at exit status 0, and a JIT would carry every check
into generated code. What a forward does change under a running activation
-- its receiver, now an object of another shape -- is what the recorded
guards check ("What a slot access checks" above).

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
shape-changing `setClass:` is deferred beyond road step 5b2a (`OPEN-WORK.md`,
"Reflection beyond road step 5b2a").

**Only mirrors reach it** (`r[runtime.forward-identity]`,
`r[runtime.vm-mirror]`). No program sends `become:`: `Object` declares no
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

**What the forward refuses** (`r[vm.mirror.forward-refuses-shipped]`).
`Heap::forward_identity` refuses, as
Primordial Soup's validation does (`heap.cc:1078-1113`), arrays of unequal
sizes and a split or a chain -- an old object named twice, or an object both
forwarded and a target (`ForwardRefusal`) -- and accepts a merge, several
old objects forwarded to one new one. The leaf is a lock of its own besides
(decided with the user on 2026-10-18): every element on either side must be
an ordinary instance (format `FixedPointers`) of a class that is no builtin
and whose application's mixin is not shipped (read as the mirrors read it,
so a class side whose instance side is shipped counts as shipped), and no
reified mixin and no value a mirror withholds -- the VM mirror and the
recorded shipped module instances (`may_be_forwarded`). Every
instance a platform path names is an instance of a shipped mixin, and so is
every kernel-made object a program can reach, a VM error's `Error` or
`MessageNotUnderstood` and its `Message` among them, so no object on a path
from shipped code to the VM mirror is forwarded. The builtin and the
withheld tests are not belt and braces: the VM mirror's class is a builtin
whose mixin is not marked shipped, so the shipped test alone would let it
through, and forwarding a program object to the mirror would hand the
capability to every holder of that object. An instance of a program's
subclass of a shipped class, an `Error` subclass's for one, is forwarded.
The one caller asks for nothing the lock refuses: the atomic install
forwards only the instances it migrates, each to its new-shape replacement,
and those are instances of the classes the edit names, never of a shipped
mixin, since the mirrors refuse every edit to one. A kernel method whose
receiver is an instance of a user subclass the install migrated fails the
slot-access check ("What a slot access checks" above) rather than writing
the old layout into the new shape.

**Everything stays editable** (`r[runtime.install]`,
`r[semantics.reflection.atomic-install]`). A class object keeps its identity
when its class is edited: the VM writes its fields -- a new application id when
methods or slots change, a new superclass, a new value refusal -- and every
reference to it stays valid, so the install forwards no class object and
no mixin, where Primordial Soup's forwards its old mixins and classes to
the new ones after the instances (`Mirrors.ns:320-326`); a mixin is
replaced under its id instead (`Mutator::replace_mixin`;
[liveness-and-reflection.md](liveness-and-reflection.md), "An edit
replaces the mixin under its id"). When the slots change, the instances
migrate (`r[vm.heap.migration-by-declared-name]`): the atomic install
allocates each in the new shape, copies its
fields by what they are named (`mixin.rs`'s `FieldName`) and sends nothing
to a user object while it does; then one bulk forward replaces all of them at
once ([liveness-and-reflection.md](liveness-and-reflection.md), "A
MirrorBuilder and an atomic install"). The install touches no frame: a
running activation finishes with the code it was running. Edit-and-continue,
which is to give a live frame locals, will divorce it into a newly
allocated, larger frame object, allocated old and born remembered,
through the same walk, and continue from the divorced frame ("The object
layout" above). Closures keep their old code, as in Smalltalk, and new
ones get the new code. A value class's instances migrate the same way, inside the VM, which
no program can observe, since a value has no identity.

A superclass change re-parents a class object in place too: the install
evaluates the new clauses for each class object, then writes its new
superclass (`Heap::class_set_superclass`) and builds its `<:` levels
anew, and a declaration that gains or loses a body of its own keeps its
class objects, each rewritten from the class of its own body into the
level its last clause adds or back, which every class object's one
layout of twelve fields allows ("The object layout" above;
[liveness-and-reflection.md](liveness-and-reflection.md), "The complete
install", decisions 1 and 11).

**A removed class lives on obsolete** (`r[vm.heap.obsolete-mixin-out-of-tree]`).
A nested declaration an install removes leaves its enclosing
declaration's nested list, so no application of the enclosing class
reserves it a hidden slot, the enclosing instances migrating without
it, and no module's text holds it; its id roots a tree of its own,
outside every module's, with its nested declarations and class sides
(`Heap::make_obsolete`, `Heap::is_obsolete`). Its row keeps its last
version, so its class objects, its instances and its subclasses -- in
another module too -- go on running it, and `outer` inside it reaches
the enclosing instance its class object holds. A full collection frees
the tree on its own once nothing names any of its ids, by the rule of a
module's tree ("An application has two ids" above), whether or not its
former module lives; a module freed first leaves the obsolete root
naming no enclosing row. An image carries an obsolete tree the graph
reaches, and the loader registers it obsolete with no enclosing row
(`Heap::make_loaded_obsolete`; [image-format.md](image-format.md),
"Edited programs in the image").

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
  first request** (`r[vm.heap.identity-hash-moves-with-object]`), so
  identity survives a moving collector and a snapshot, and
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
  with the user on 2026-10-05; "`become:` is an eager bulk forward"
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
  road step 5b, the guard in the activation stack's entry
  alone and a resumed activation starting with `NONE`; the applied clause
  is one `SmallInteger` of mixin id, clause index and builtin tag, 32, 23
  and 6 bits, the clause count's limit reported as a resolve or decode
  error -- a limit taken knowingly against the rarity rule; the reserved
  guards are `NONE` 0, `VACANT` 1 and `FORWARDED` 2; a value-class
  refusal is heap data, not a Rust-side table; and the ids are minted per
  heap and key the application cache, which a full collection prunes by
  the ids it reclaims (built by road step 5b2c on 2026-10-20, "An
  application has two ids" above).
- **Road step 5b is built as two plans, 5b1 the VM's half of live editing
  and 5b2 the MirrorBuilder, the atomic install, the weak application
  cache, the image's edited mixins and run-time compilation**, 5b1 built
  (four decisions taken with the user on 2026-10-18,
  [its plan](../superpowers/plans/2026-10-18-1236-become-and-recorded-guards.md)),
  and 5b2 split in turn into 5b2a, the MirrorBuilder, run-time
  compilation and the atomic install, built
  ([its plan](../superpowers/plans/2026-10-18-2138-mirror-builder-and-atomic-install.md)),
  and 5b2b, split once more on 2026-10-19 (decided with the user): 5b2b,
  declaration texts and the image's edited mixins, built
  ([its plan](../superpowers/plans/2026-10-19-1702-edited-programs-in-images.md)),
  and 5b2c, the weak application cache and the reuse of ids, built
  ([its plan](../superpowers/plans/2026-10-20-1318-reclaiming-ids.md)).
  The bulk forward runs inside its leaf, in the middle of a step, not at
  the next safepoint, so no instruction runs between the forward and the
  leaf's answer and it is atomic; nothing live moves, so no move epoch
  changes. The leaf comes with 5b1; road step 5b2a's install calls the
  forward on the Rust side, inside its own leaf, so no safepoint falls
  between rebuilding the classes and forwarding their instances (decided
  with the user on 2026-10-18); tests reach the leaf by handing the VM mirror to a test program from
  the host. The leaf is a lock of its own: both sides ordinary instances
  of a class whose mixin is not shipped, besides Primordial Soup's checks
  -- extended while building it to builtin classes and withheld values,
  since the VM mirror's class is a builtin whose mixin is not marked
  shipped ("What the forward refuses" above). Refined while planning: the
  accessing level a cold check asks for is the application an activation
  was *entered* at, by its full-width index, recorded beside the guard,
  not the class object's current application, which the atomic install
  rewrites in place under a running activation; and it is recorded only
  for code that addresses its own slots, so an ordinary send pays one
  flag test at its push ("What a slot access checks" above). Chosen by
  measurement while building it: the private and super accessor check out
  of line, with one verified (guard, level) pair per heap rather than a
  rewrite of the entry that hit.
- **`new` is compiled code** (eight decisions taken with the user on
  2026-10-22, built by
  [its plan](../superpowers/plans/2026-10-22-2051-compiled-new.md);
  "Instantiation", "Mixin application" and "The nested-class accessor"
  above), for performance first -- `Foo new` becomes ordinary sends to
  compiled bodies, which take the inline caches and which the JIT tiers can
  compile and inline, where a Rust walk is opaque to a JIT -- and for
  uniformity with the references second. The documents cite them by number:
  1. The factory is compiled code with an activation of its own, shaped
     like Squeak-Newspeak's: an allocation on the receiver, with every
     refusal and builtin case, the direct invocation of the receiver's own
     level's initializer with the factory's arguments, and the return of
     the instance.
  2. Each level's initializer is a body on its mixin, outside the method
     table, invoked by dedicated instructions -- never a super send of a
     hidden selector, which a program could reach with `perform:` or a
     mirror and which skips a mismatched level silently.
  3. A class without initializer code costs one factory activation until
     the JIT inlines it; road step 6's `new` with no activation goes.
  4. A level a `<:` clause added, reached from outside, is an ordinary
     class of its mixin: its own superclass clause, then its own part.
  5. The nested-class accessor stays bodiless, and its cache entry holds
     the hidden slot's absolute index.
  6. A level's initializer invokes its superclass's from inside its own
     activation, so a chain nests one counted activation per level with
     code, and a chain deeper than the depth limit overflows; levels
     without code are passed in a loop.
  7. The measurements are a record with a tolerance, not a strict gate
     ([execution.md](execution.md)).
  8. A declaration without a body is the mixin application it writes, a
     specification deviation fixed: its class object is the level its last
     `<:` clause adds, named for the declaration, with the declaration's
     factory and no class side of its own. Amended with the user while
     building it: that one class object also records the declaration's
     own enclosing object and declaring class, and only the frames
     running the declaration's own code start their lexical walk there,
     as SOMns decides it per running method; their `super` starts at the
     class object's superclass, as §5.10 says for every code.

  Refined while planning: which factory a class object has, and its name,
  are read through its metaclass; the factory body is synthesized by one
  function, never lowered and never on the wire; each mixin carries three
  initializer bodies, its clause messages lowered inline; and the
  initializer instructions have no cache of their own. Left for later:
  the futures of simultaneous slots, which stay in Rust, and the
  scalarization of values in the optimizing JIT
  ([OPEN-WORK.md](../OPEN-WORK.md)).
