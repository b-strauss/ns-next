# Core library and the platform

> **Status:** partial — the core library is built as Newspeak source,
> `Kernel.ns`, over one VM mirror, §3.2's `Behavior`, `Class` and
> `Metaclass` and §8.6's exceptions included, and the platform is a facet
> of the shipped module `PlatformFactory.ns` with lazy members, its
> `collections` the facet of the shipped module `CollectionsFactory.ns`
> with a Newspeak `List`; missing is `Integer>>/`, and the surface grows on
> demand, not speculatively.
> **Code:** `crates/ns-runtime/core-library/Kernel.ns` (the library),
> `crates/ns-runtime/core-library/PlatformFactory.ns` (the platform),
> `crates/ns-runtime/core-library/CollectionsFactory.ns` (the collections
> module), `crates/ns-runtime/core-library/Manifest.ns` (a project's
> manifest, which is no shipped module: "The manifest"),
> `interpreter/kernel.rs` (the bootstrap's kernel phase),
> `interpreter/core_library.rs` (the table of shipped modules and the
> platform's boot),
> `interpreter/mirror.rs` (how a send to the mirror runs), `builtins/` (the
> leaves; `builtins/primitive.rs`'s `MIRROR` is the mirror's table;
> `builtins/activation.rs` and `builtins/message.rs` the exceptions',
> `builtins/module.rs` `moduleNamed:`, `builtins/future.rs` the pull
> future's forcing),
> `interpreter/unwind.rs` (the one unwinder, and a VM error signalled),
> `compiler/primitive_method.rs`, `compiler/unwind_marker.rs`, `platform.rs`.

The core library is Newspeak source the runtime ships:
`crates/ns-runtime/core-library/Kernel.ns`, compiled into the binary
(`include_str!`) and compiled again at every bootstrap, never carried in an
artifact. Its methods reach Rust through exactly one object, the VM mirror,
as §8.3.1 requires, and the leaves — the arithmetic, the comparisons,
`size`, `at:put:`, `printString`, `show:` — stay Rust. Keeping the library
out of the artifact is what lets an image stay small and keeps a hostile
image from replacing a builtin class; it also means a modified library is a
different `ns-next` binary rather than a different artifact. See the
comparison in [image-format.md](image-format.md).

## What exists

`Kernel.ns` declares one nested class per builtin class but the mirror's:

```
Top                     (private; no superclass, declares nothing)
└── Object              class, ==, =, hash, printString, doesNotUnderstand:,
    │                   halt, and an accessor per nameable builtin (String, ...)
    ├── Value           =, ==, hash, and the two slot-reading leaves
    │   ├── Boolean     ├── True
    │   │               └── False
    │   ├── Number      ├── Integer
    │   │               │   └── LargeInteger
    │   │               └── Float
    │   ├── String
    │   ├── Symbol
    │   └── UndefinedObject
    ├── Array
    ├── Block           ..., on:do:, ensure:, ifCurtailed: (marked methods)
    ├── Exception       signal, signal:, pass, return:, retry, resume:, ...
    │   ├── Error       └── MessageNotUnderstood
    │   └── Halt
    ├── Message         selector, arguments, sentTo:
    ├── Activation      (private; a running activation, for the kernel only)
    └── Behavior        =, ==, hash (a class object's, derived from its state
        │               when it is a value)
        ├── Class       name, printString, handles:
        └── Metaclass   name, printString, =, ==
```

The mirror's class is the one builtin `Kernel.ns` does not declare; the
bootstrap builds it below `Value`, so the mirror is a value ("The bootstrap:
placeholders first, kernel second" below).

`Behavior`, `Class` and `Metaclass` are §3.2's reified class hierarchy: every
class object's class is its own metaclass — its class side applied to
`Class` — and every metaclass's class is `Metaclass`. They carry behaviour
only; a class object's state stays in the runtime's `ClassData`, and their
leaves are in `builtins/behavior.rs`: a class's name
(`className:ifFail:`), a metaclass's class (`metaclassThisClass:ifFail:`),
and the three parts of a class object's state `Behavior>>=`'s fallback
compares. `Object` declares no accessor for any of the three, and nothing
below `Behavior` can be instantiated
([object-model.md](object-model.md), "Class sides and metaclasses").

That is §3.2's shape and §3.1.1's, taken literally. §3.2 says "a class is
either the empty class `Top` or the application of a mixin to another class",
and "only the class `Object` may have `Top` as its superclass", so `Top`
exists, declares nothing, and is the one class `Kernel.ns` keeps private —
every other class says `public`, because §6.3.1 makes an undeclared member
protected and a protected class is unreachable from any other module, which
is where a program's `Integer` or `String` has to come from. §3.1.1 says a
value object's "class inherits from class `Value`", so every class whose
instances are values hangs below `Value`, and `Number` sits between it and
`Integer` and `Float` because §3.1.1 names `Number` among them.

**This is not SOMns's shape, and the difference is the one rule SOMns does
not keep.** `prior-art/somns/core-lib/Kernel.ns` has `Top` and `Value`, and
its value classes hang below `Value` exactly as they do here — its `=` is
even the same leaf, `vmMirror value: self sameAs: other`. What it does
differently is insert a class between: `Top` → `Thing`, which declares
`doesNotUnderstand:`, `asString`, `isNil` and the rest, and then `Object`
and `Value` as **siblings** under `Thing`, `Object` declaring only `==` and
`~=` and `Value` only `=`, `<>` and `copy`. So in SOMns a value is not an
`Object`. §3.2 is explicit that it must be — "except for `Top` all classes
are subclasses of class `Object`" — so `Value` is a subclass of `Object`
here, and the behaviour SOMns puts on `Thing` is on `Object`. The cost of
the rule is that `Value` must *override* what `Object` declares, `=`, `==`
and `hash`, rather than declaring them where nothing else has; the benefit
is that `3 doesNotUnderstand: …` and `'a' printString` need no second
declaration, and that a program may assume of any receiver what §3.2 says it
may.

Almost every method is a one-line forwarder to the mirror ("The
shape of a kernel method" below): `Object`'s `class`, `==`, `=`, `hash`,
and `printString`, beside its thirteen
accessors for the predefined classes, `Object` to `Block`, each
`^outer Kernel <Name>` ("The predefined classes are accessors on `Object`"
below); `Integer`'s `+ - * // \\ < >
<= >= =`, `bitAnd:`, `bitOr:`, `bitXor:`, `<<`, `negated`, `abs`, `asFloat`,
`printString` and `asString`, beside `bitInvert`, which is ordinary Newspeak,
Primordial Soup's `^-1 - self`, and reaches no leaf; `LargeInteger`'s the same
leaves and `hash` ("Large integers" below);
`Float`'s `+ - * / < > <= >= =`, `hash`, `sqrt`, `asStringFixed:`,
`printString` and `asString`; `String`'s `size`,
`,`, `=`, `hash`, `asSymbol` and `printString`; `Symbol`'s `asString`, `=`,
`hash` and `printString`; `Array`'s `do:`, `at:put:`, `size` and `at:`; `Block`'s `value` to `value:value:value:`,
`numArgs`, `whileTrue:` and `whileFalse:`. `PlatformFactory.ns`'s `Transcript`
forwards `show:` and `newLine` the same way, and its `Stopwatch` its clock
reading ("The platform" below). `Number` reaches no leaf of its own: its
`to:do:` and `to:by:do:` are ordinary Newspeak (below).
`UndefinedObject` reaches no leaf either: its nil protocol is quick methods
and its `printString` an ordinary one ("The nil protocol" below). `True` and
`False` reach no leaf at all:
their conditionals, `not`, `&`, `and:`, `|` and `or:` are quick methods
("`True` and `False` are quick methods" below), and their `printString` and
`hash` ordinary ones. `Boolean` declares nothing of its own either.
`Object`'s protected `doesNotUnderstand:` and `halt`, the exception
classes, `Block`'s `on:do:`, `ensure:` and `ifCurtailed:` and `Message`
are the exception machinery, ordinary Newspeak over the
activation leaves ("Exceptions" below).

The other exceptions are `hash` on `Integer` (`^self`) and `asString` on
`String` (`^self`), both quick methods ("`True` and `False` are quick
methods" below), and `hash` on `True` and `False` (`^1` and `^0`), ordinary
methods: the value each answers is
already at hand, and a row for it would only repeat what `Kernel.ns` can
say. Every value class
declares the `hash` its own `=` demands — §3.1.1 lets a value be copied
across an actor boundary, so a copy has to hash like its original, and none
of them may inherit `Object`'s identity hash. `String`'s, `Symbol`'s and —
since 2026-09-23 — `Float`'s `=` each require an argument of the receiver's
own class: each class hashes by its own content, so a cross-class equality
would break the rule, as `Float`'s widening did when it made `1.0 = 1` true
while `1.0 hash` and `1 hash` differ ("What is still unchecked" below).

**`to:do:` and `to:by:do:` are sent, where Primordial Soup inlines them.**
Both are `Number`'s, Newspeak bodies after Primordial Soup's
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:2107`,
`:2118`): an inclusive bound, a negative step counting down, and `nil` as
the answer, so an `Integer`, a `LargeInteger` and a `Float` receiver share
one body and a range crossing into `LargeInteger` keeps counting
(`goldens/kernel/number_iteration.ns`). They are no leaf, because each
iteration sends `value:` to the block and `<=` or `>=` and `+` to the
counter, and a Rust native that calls back into Newspeak is the ceiling
"The core library as Newspeak source over a VM mirror" below rules out.
Primordial Soup's compiler inlines both whenever the block is a literal
(`:inlineBytecode:`; `tryInlineToDo:` in
`prior-art/primordialsoup/newspeak/NewspeakCompilation.ns`), so its method
runs only for what it cannot inline. Ours sends them to the receiver as
any other message: inlining without a guard would skip the lookup of
`to:do:` itself, so an inlined form here, like the inlined
conditionals, would need a guard on the receiver and the send as its
fallback ([compiler.md](compiler.md)). Whether that pays is to be decided
by measuring the Are We Fast Yet benchmarks, which loop with both
([OPEN-WORK.md](../OPEN-WORK.md)). The increment is `current + step`
rather than Primordial Soup's `step + current`. An `Integer` receiver
with a `Float` bound or step counts too, since an integer's arithmetic
and ordering meet a `Float` argument: `1 to: 2 by: 0.5 do:` answers `1`,
`1.5`, `2`, the counter becoming a `Float` once a `Float` step is added
(`goldens/kernel/an_integer_counts_to_a_float_bound`).

**`asStringFixed:` rounds as Primordial Soup's double-conversion does.**
`Float>>asStringFixed:` writes a count of fraction digits from 0 to 20,
and raises for any other count, as Primordial Soup's primitive fails and
its method raises (`goldens/errors/as_string_fixed_refuses_more_than_twenty_digits.ns`).
A tie rounds up on the exact binary value, where Rust's formatter would
round it to even, so `0.125 asStringFixed: 2` is `'0.13'` and
`1.005 asStringFixed: 2`, just below the tie in binary, is `'1.00'`; the
sign is the value's even when the digits round to zero (`-0.04
asStringFixed: 1` is `'-0.0'`); and outside ±1e21, and for a NaN or an
infinity, it answers the text `printString` does (`fixed_text` in
`builtins/numbers.rs`; `goldens/arithmetic/as_float_and_as_string_fixed.ns`).
The Are We Fast Yet harness prints its scores with it.

**A `Symbol`'s hash is its interner number** (`text::symbol_hash`): §5.1.6
makes equal symbols one object, so it agrees with `Symbol>>=` by
construction and is a dense small integer. It is stable only within one
process — the interner numbers symbols in the order they are first seen —
so it does **not** survive a snapshot: the same symbol in a reloaded image
can hash to another number, and a hash a program recorded in an artifact
cannot be compared against one computed after the load.

**Planned: strings of code points in two widths.** Would change: a `String`
stops being a Rust `String` of UTF-8, whose `size` counts code points by
walking the bytes, and stores its code points in the object space, one byte
each while every one fits Latin-1 and 32 bits each otherwise, so `size` and
an `at:` are one access; `Symbol` becomes a subclass of `String` sharing both
formats, and `=` stays class-tight both ways as today (`'foo' = #foo` and
`#foo = 'foo'` answer `false`), so `Symbol` overrides `=` and `hash`, pinned
by a golden. UTF-8 stays the encoding at every boundary -- source, the
transcript, files. The design and the comparison with the reference engines
are [object-model.md](object-model.md), "Planned: the object layout".

Four further rows serve `Value` itself rather than one builtin:
`value:sameAs:ifFail:` (reported `=`), `valueHash:ifFail:` (`hash`),
`valueSlotCount:ifFail:` (`slotCount`) and `value:slotAt:ifFail:`
(`slotAt:`). The first two are the equality and hash `builtins/value.rs`
derives from an instance's state, for a class that inherits `Value`'s `=`
rather than declaring one; they decline anything else, and `Value`'s
`ifFail:` block answers instead. Neither walk recurses or calls back into
Newspeak.

The last two are the first *reflective* leaves the mirror carries: given an
object, how many slots its class's application declares, and what the slot
at an index holds. They exist because the Newspeak fallback has to read
state from a receiver it cannot send a private selector to, but they are
the same two questions an object mirror asks
([liveness-and-reflection.md](liveness-and-reflection.md)), and the mirror
work is meant to reuse them rather than declare a second pair. What they do
not yet answer is a slot's *name*, which `Value` does not need and a mirror
does; that is the shape to extend when it is built.

`Value` is where §3.1.1's "a value object is globally unique, in the sense
that no other object is equal to it" is written down. It declares `=` as a
forwarder to `value:sameAs:ifFail:`, `hash` to `valueHash:ifFail:`, and the
identity method `==` as `^self = other` — an ordinary method, since §3.1.1
requires an identity method a value's class declares to "return the same
results as `Value`'s equality method", and a send of `=` makes that true by
construction for every subclass. Where a leaf declines — a slot whose class
brings its own `=`, a shape it does not handle, its node budget — the
failure block runs `valueEquals:` or `valueHashOfSlots`, which walk the
slots from Newspeak and send `=` or `hash` to each. Those two read a slot
through `valueSlotCountOf:` and `valueSlotOf:at:`, which name their subject
rather than forwarding `self`: a private member is reachable only through a
lexical send (§5.9), so the *other* side's state cannot be read by sending
it a private selector, and both sides have to go through the same leaf.
Naming the subject is also what costs those two the primitive-method fast
path, which only the declining path pays.

**A value's state is the slots its classes *declare*, and nothing else.**
An instance's field vector holds more than that: each level lays its own
slots down at its `own_slot_base` and then one hidden cache slot per nested
class declaration ([object-model.md](object-model.md), "The nested-class
accessor"), so the declared slots of a chain are interleaved with hidden ones
rather than forming a prefix. A nested class's cache slot is not state — §6
gives every instance a class object of its own for each nested declaration,
so two equal values hold *different* objects there as soon as each has been
sent its accessor, and counting the slot made them answer `false` at exit
status 0. Which indices are state is decided once, when the application is
built: `MixinApplication::declared_slots`, computed in `apply_mixin` beside
`nested_slot` and `inherits_value` because nothing about an application
changes afterwards. Everything that asks borrows that one list through one
wrapper — both Rust walks, and the `slotCount` and `slotAt:` leaves the
Newspeak fallback reads a slot through. That is what keeps the leaf and the
fallback comparing the same slots in the same order — if they disagreed, `=`
would answer one thing when the leaf decided and another when it declined —
and it is why the declining path, which asks once per slot read, allocates
nothing to ask.
`goldens/kernel/a_values_nested_class_is_not_part_of_its_state.ns` pins it,
including the case where only one of the two instances has been touched and
a class that *inherits* the declaration from a value superclass while
declaring a slot of its own after it.

**What `==` on a value now costs.** It used to be one primitive method:
`Object`'s `==`, whose leaf compares two handles and answers. It is now an
*activation* of `Value>>==`, which sends `=` — a second dispatch, to a
primitive method whose leaf walks the two values' state — and, where that
leaf declines, a third activation for `valueEquals:` and a send of `=` per
slot below it. So the cheapest case is two sends and a state walk where it
was one send and a pointer comparison, and the declining case is Newspeak
code in a loop.

That is not a cost worth removing by hand: §3.1.1's agreement rule leaves
no cheaper answer that is also the right one, since a value has no identity
for `==` to report. It is a cost worth *recording*, because it is on the
path of every `==` a program writes about a number, a string or a symbol —
the common shapes decide inside the leaf, without unfolding anything, and
the later tiers are where it is meant to come back: `Value>>==`'s body is
one send, exactly the shape an inliner folds into its callee, and the leaf
is a primitive a JIT can emit inline for a receiver whose type feedback
says `SmallInteger`. Until then, a program that compares immediates in a
hot loop pays two dispatches where Milestone 1 paid one. **The measurement
does not bound this**: none of the eight benchmark sources sends `=` or
`==` in its loop, which was recorded before measuring
([execution.md](execution.md), "What `Top`, `Value` and `Number` cost"). A
source that does is what would show it, and there is none yet.

**§3.1.1's four conditions are enforced, and where.** §3.1.1's second
bullet asks four things of an object taken to be a value: that all its slots
"are immutable, have been initialized and contain value objects", that "its
enclosing objects (3.3) are all value objects", and that "its class inherits
from class `Value` and does not define an identity method (`==`)". Each is
refused rather than silently allowing a value that is not one, and each where
an instance would come into being; [object-model.md](object-model.md), "A
value object's four conditions", has the whole design:

- **A mutable slot anywhere in a value's chain means no instance.**
  `Mutator::apply_mixin` computes `inherits_value` and the first mutable slot
  of the whole chain (`first_mutable_slot`) once, when it builds the
  application; `decide_instantiation` refuses before it allocates anything,
  and names both the class asked and the class that declares the slot, since
  a subclass inherits the fault. The slot counts above the `Value` level too,
  where a `<:` clause can put a class with slots. §6.3.2's immutable slot is
  what makes this checkable at all: a value class may declare
  `| held = … |`, never `| held ::= … |`.
- **A slot of a value may hold only a value.** `Instruction::InitializeSlot`
  asks `instantiate::check_value_slot` on every store into an instance whose
  class inherits `Value`; anything that is not itself a value — an `Array`,
  an ordinary instance, a class object — is refused with the slot named.
  Deep immutability is then transitive by induction: whatever a value's slot
  holds was itself checked when it was made.
- **A value class may not define `==`.** Decided once per application, cold,
  by `Mutator::apply_mixin` — the same place `first_mutable_slot` is decided
  — and folded into `MixinApplication::instantiation_refusal`, refused at
  instantiation alongside the root clause defect and the mutable slot. Being
  a property of the application rather than of any class object built from
  it is what covers a module's root the same way it covers a nested class:
  `load_root_class` calls `apply_mixin` too, even though it builds no class
  object through `nested::after_clause`. `Value`'s own `==` is the
  declaration the specification's next sentence allows ("If the class
  `Value` declares an identity method, that method must return the same
  results as `Value`'s equality method").
- **A value class's enclosing objects must be values.** Decided once per
  class object, cold, where one is created (`interpreter/value_class.rs`,
  from `nested::after_clause`), recorded on it (`ClassData::value_class_refusal`)
  and refused at instantiation, because §2.3's own example creates a value
  class whose enclosing object is mutable and never instantiates it (decided
  with the user on 2026-09-24). The enclosing objects of every level of the
  class chain count, not only the class's own, since an inherited method
  reaches its own level's through `outer`.

Every refusal names **the primary factory of the class being instantiated**,
defaulted to `new` — the selector the program sent. A store in an inherited
level's initializer is reported under it too, although that level runs in its
own activation with its own factory (§6.2): the superclass clause sends that
one, the program does not.

**So a value class lives in a value module.** A class declared in a module
lives inside that module's instance (§3.3), so a value class's module has to
be a value too: it declares itself one with §6.2's mixin application,
`class M = Object <: Value (…)`, as SOMns's
`class Kernel vmMirror: vmMirror = Object <: Value (`
(`prior-art/somns/core-lib/Kernel.ns`) does and as `Kernel.ns` does here. A
value class nested in a module written `= Object new (…)` is refused on
instantiation, and every golden that declares a value class declares its
module a value.

**What is still unchecked.**

- **A value class nothing creates is never checked, and one nothing
  instantiates is never refused.** A value class's identity method is
  decided when its application is built, and its enclosing objects when its
  class object is created, but both only recorded; they, the mutable slot
  and the slots' contents are refused only where an instance comes into
  being. A class that inherits `Value` and
  declares a mutable slot is a defect the moment it is written, and it is
  reported the moment something asks for an instance — never before. That is
  what §2.3 requires: its own example creates such a class and never
  instantiates it. It also keeps class creation, and the public
  `load_module`, total.
- **A ready-made instance decoded from an image is not re-checked.** The
  decoder rebuilds an instance's slots from the artifact's bytes without
  going through `InitializeSlot`, so a crafted image can hand back something
  whose class inherits `Value` while a slot holds an `Array`. That is a
  wrong answer for a crafted artifact, not a crash: the decoder stays
  panic-free, and the derivation declines any shape it cannot rule on rather
  than guessing ([image-format.md](image-format.md), "Verification", on what
  the two passes do and do not guarantee). A decoded *class object* is
  different: its enclosing-object condition is recomputed on load, with the
  same `check_value_class`, once every decoded class has its real
  application (`image::decode::rebuild_applications`), so a snapshot of a
  refused value class decodes refused; its identity-method condition needs
  no recompute of its own, since `rebuild_applications` builds every decoded
  class's application through the same `Mutator::apply_mixin` a class built
  from source goes through, which folds that condition in as it goes. A new
  instance of any decoded class meets every check an instance made from
  source does.
- **`=` and `hash` are expected to agree, not checked to.** A subclass that
  overrides one and not the other breaks the contract silently, as it does
  in every Smalltalk.
- **Identity is not globally unique.** §3.1.1's "globally unique, in the
  sense that no other object is equal to it" is met for `=` and `==`, which
  the derivation makes state-based, but two equal `String`s remain two
  distinct objects to the collector. Canonicalization is not required for
  the derived equality and was left out deliberately.
- **`1 = 1.0` and `1.0 = 1` both answer `false`** (decided with the user on
  2026-09-23). `Float>>=` widened an `Integer` argument until then, which
  made `1.0 = 1` true while `1.0 hash` and `1 hash` differ — the one rule a
  `hash` has — and made `Float`'s own `=` disagree with the derived
  equality, which asks the classes first. Every value class's `=` is
  class-tight now, `Float`'s like `String`'s and `Symbol`'s. **Only
  equality is class-tight:** a `Float` receiver's `+`, `-`, `*`, `/`, `<`,
  `>`, `<=` and `>=` take an integer argument of either class and convert it to
  the nearest `double`, exact up to 2^53 (`argument_float` in
  `builtins/numbers.rs`), so `1.5 < 2` answers; an integer receiver's `+`,
  `-`, `*`, `<`, `>`, `<=` and `>=` convert themselves to a `double` the
  same way for a `Float` argument, so `1 < 2.0` and `1 + 1.5` answer too,
  while `=` stays class-tight. So this is a stopgap, not the answer: see
  "Gap: an integer has no `/`" below, where the tower it needs is
  recorded.
The builtin value classes' own immutability rests on there being no mutating
leaf for a `String` or a `Symbol` — their state is a Rust
payload, not slots, so neither check above is about them. And the actor
boundary that gives all of this its purpose does not exist
([liveness-and-reflection.md](liveness-and-reflection.md), "Value objects
exist; the boundary they cross does not").

The leaves are the 105 rows of `builtins/primitive.rs`'s `MIRROR` table. A row
names the **mirror selector** (`collection:at:put:ifFail:`), the **reported
selector** — the kernel selector every error text of that leaf names,
`at:put:` — and the `Primitive` it runs. Every mirror selector ends in
`ifFail:`; its arguments are the subject (the kernel method's `self`), the
leaf's own arguments, and the failure block.

### The platform

The platform a module is handed (§8.3) is an instance of the nested class
`Platform` of `crates/ns-runtime/core-library/PlatformFactory.ns`, a shipped
core-library module beside `Kernel.ns` (decided with the user on
2026-09-25, designed on 2026-09-26, and given the factory pattern below on
2026-09-27):

```newspeak
class PlatformFactory usingVmMirror: theMirror kernel: theKernel = Object <: Value (
    | private vmMirror = theMirror. private kernelModule = theKernel. |
) (
    lazy private collectionsModule = (vmMirror moduleNamed: #CollectionsFactory ifFail: [nil]) usingKernel: kernelModule.
    private class Platform = Value new () (
        lazy public transcript = Transcript new.
        lazy public collections = collectionsModule newCollections.
        lazy public kernel = KernelNamespace new.
        lazy public time = Time new.
        public Platform = ( ^outer PlatformFactory Platform )
        public Transcript = ( ^outer PlatformFactory Transcript )
        public KernelNamespace = ( ^outer PlatformFactory KernelNamespace )
        public Time = ( ^outer PlatformFactory Time )
        public Stopwatch = ( ^outer PlatformFactory Stopwatch )
    )
    private class Transcript = Value new () ( show: and newLine, through vmMirror )
    private class KernelNamespace = Value new () ( Exception, ..., each through kernelModule )
    private class Time = Value new () ( public Stopwatch = ( ^outer PlatformFactory Stopwatch ) )
    private class Stopwatch = Object new ( | private cumulativeNanoseconds ::= 0. private startNanoseconds ::= nil. | ) (
        elapsedNanoseconds, start, and the clock reading, through vmMirror )
    public newPlatform = ( ^Platform new )
) : ()
```

and `platform collections` is the facet of a second shipped module,
`crates/ns-runtime/core-library/CollectionsFactory.ns`, written the same
way (2026-09-28, "The collections module (plan 2)" below):

```newspeak
class CollectionsFactory usingKernel: theKernel = Object <: Value (
    | private kernelModule = theKernel. |
) (
    private class Collections = Value new () (
        public Collections = ( ^outer CollectionsFactory Collections )
        public List = ( ^outer CollectionsFactory List )
    )
    private class List = Object new ( | protected size_ ::= 0. protected data ::= Array new: 4. | ) ( ... )
    public newCollections = ( ^Collections new )
) : ()
```

**The factory pattern.** Every shipped module is written this way: an
`XFactory` root holds what is privileged -- here the VM mirror and the
kernel instance, in private slots, since neither a lazy slot nor a method
may name a factory parameter (§6) -- and is never handed to anyone. What a
program receives, and may instantiate, are the root's nested classes, its
facets. Each reaches the privileged state lexically, as a
member of the root, never through a slot of its own, so the facets keep
`new` and hold no privileged state in slots of their own -- `Transcript`,
`KernelNamespace`, `Time` and the collections module's `Collections`
declare none, `Stopwatch`'s two slots hold only its own readings, and
`Platform`'s four lazy slots hold only facets: `t class new` makes a
transcript equal to `t`, and `platform class new` a platform that writes
like the first
(`goldens/kernel/a_second_platform_writes_like_the_first`). The platform
hands out instances of the facets by lazy slots and their classes by class
accessors, methods whose whole body is `^outer PlatformFactory <Name>`.
Because the root is unreachable, no program can instantiate it over forged
arguments, so the pattern needs no runtime refusal beside it: the guarantee
is made by how the shipped classes are arranged, not by a check in the VM
([README.md](README.md), "Principles"). A new shipped module starts from
this shape, not from the prior-art one in which the root is the namespace
handed out.

**The factory's nested classes are private, and its one public member
makes the facet.** A class accessor and a lazy slot's initializer reach a
private class of the root by a lexical send, which may find a private
member (§5.9), so a program still gets every class it is meant to have,
through the facet; but a public send to the root instance finds nothing
but `newPlatform`, `^Platform new`, which the host sends at boot. That is
a second line of defence behind the root instance being unreachable ("No
program reaches a shipped module's root" below): even a holder of the
root could make only another platform from it, never name a class that
reaches the mirror (`tests/platform.rs`,
`the_platform_factory_answers_only_new_platform`,
`the_platforms_class_accessors_reach_the_private_classes`).
`CollectionsFactory.ns` follows the same pattern: its root holds the kernel
instance, its nested `Collections` and `List` are private, and its one
public member, `newCollections`, answers the facet `platform collections`
is (`the_collections_factory_answers_only_new_collections`).

**The members are lazy slots** (§6.3.3, "Lazy slots first" below): each is
built on its first use, so a program pays only for the members it asks for,
and the host's boot runs no member's initializer.

**One collections module serves every platform.** `PlatformFactory`'s root
keeps the collections module in its private lazy slot `collectionsModule`,
which fetches `CollectionsFactory`'s root class through the mirror's
`moduleNamed:` ("The module table and the boot" below) and instantiates it
with the kernel instance, on the first `platform collections` of any
platform; `Platform`'s `collections` answers that instance's
`newCollections`. So every platform's facet comes from one factory and
hands out one `List` class, and the facets, values, are all equal
(`goldens/collections/every_platform_shares_one_collections_module`). The
facet names its own class, `^outer CollectionsFactory Collections`, as
`Platform>>Platform` does, and `List` by `^outer CollectionsFactory List`:
class accessors the compiler recognizes in every shipped module but the
kernel, so the platform paths `collections Collections` and `collections
List` needed no wider recognition.

**The boot.** `Interpreter::install_platform`, run by `bootstrap_with`
right after `install_kernel`, takes `PlatformFactory` from the table of
shipped modules ("Where the library source lives, and how modules are
found" below), instantiates it with `usingVmMirror: mirror kernel: kernel`
through the ordinary `Interpreter::instantiate`, checks that its `vmMirror`
is an immutable, eager slot holding the mirror
(`interpreter::core_library`'s `check_factory_holds_the_mirror`, as
`interpreter::kernel` checks the kernel's), then sends it `newPlatform`
as an ordinary host send and keeps the answer as the platform
(`Mutator::platform_singleton`, traced like every root). Each step is a run
entered from the outside, as the kernel's instantiation is, so the stress
modes see it. `Mutator::platform_value` only reads the platform, so asking
for it, at bootstrap or in `image::load`, runs no Newspeak. The boot does
run a little shipped code -- `newPlatform` and `Platform`'s superclass
clause `Value new`, whose sends take room in the inline caches -- so a
freshly bootstrapped interpreter no longer starts with an empty cache
table (`tests/inline_caches.rs` pins what it leaves). The kernel instance
is the second parameter, as in Primordial Soup (`Platform internalKernel:
ik`), rather than a mirror leaf answering the kernel (SOMns's
`vmMirror kernelObject: nil`): handing over an object needs no Rust, and
the dependency stands in the factory's signature. `image::load` depends on
what `PlatformFactory.ns` runs at load: it resolves the image's platform
paths -- each step a unary send that runs a lazy member's initializer the
first time, or a class accessor or nested class's accessor and, on the
class's first use, its superclass clause -- before it restores the identity
hashes the image recorded, so a hash drawn on the way could equal one the
graph hands back. Nothing the boot, a member's initializer or a path's sends
run assigns one; a debug build asserts that the counter stood still across
path resolution, and a member whose initializer hashes something -- a hashed
collection built eagerly, say -- would have to be kept off every path or the
order changed.

**The transcript leaf does not check its receiver.** `Transcript`'s `show:`
and `newLine` are mirror forwarders like the kernel's —
`PlatformFactory.ns` is resolved with `resolve_kernel`, and `vmMirror` is a
member of its root, so their `vmMirror` sends are mirror sends and the two
methods are primitive methods — and their leaf (`platform.rs`) reads
nothing off the receiver, only the string argument. The capability is the
mirror, which only shipped source reaches; `Transcript` is no builtin, so
the VM could not recognize its class anyway. A user subclass of
`Transcript`, reached as `platform Transcript`, therefore inherits a
working `show:` (`tests/platform.rs`,
`a_subclass_of_the_transcript_class_writes_too`), and so does a subclass of
the platform's class (`a_subclass_of_the_platform_class_writes_too`).

**`show:` writes a string; `print:` writes any object** (decided with the
user on 2026-09-27). The leaf refuses an argument that is no `String` as an
ordinary Newspeak error ("show: requires a String, got 42"), catchable like
any other, where it used to write nothing and answer the transcript; it
never declines, so both forwarders' failure blocks are `[nil]`, as the
kernel's `terminate:` forwarder's is. `print: object = ( ^self show: object
printString )` is Newspeak beside it, as canonical Newspeak's transcript has
both (`prior-art/newspeak/RuntimeForJS.ns`); it was added with a golden
rather than on a corpus module's demand. `show:` does not send
`printString` itself, since a `String`'s `printString` quotes it
(`goldens/kernel/the_transcript_prints_any_object`,
`goldens/errors/show_of_something_that_is_no_string_is_an_error`).

**The namespace members are ordinary methods.** `KernelNamespace`'s five
answer the kernel instance's nested classes. The class-accessor fast path
("The predefined classes are accessors on `Object`" below) is recognized
only in the form `^outer Kernel <Name>`, which no module but the kernel can
write, so each costs one activation more than it did as a kernel accessor
(`interpreter/kernel.rs`,
`the_platform_modules_members_have_the_fast_path_they_should`). A
namespace member is asked for once, typically in a slot initializer. The
platform's class accessors, and the collections facet's `Collections` and
`List`, carry no fast path either: the compiler recognizes them
(`MixinMethod::class_accessor`) for the image's platform paths alone, and
no send reads the mark
(`the_platforms_class_accessors_are_recognized_without_a_fast_path`).

Being values, the platform and its members have no identity: two
transcripts are equal and so identical, two platforms too, and a
transcript kept in an image is the loading platform's after the load. None
has an accessor on `Object`, since a program reaches them through the
platform it is given, and the platform declares no accessor for a
predefined class either: `platform Array` reaches `Object`'s (§3.2), which
it inherits like every object
(`goldens/kernel/the_platform_and_its_parts_are_values`).

**`platform time` is Primordial Soup's `Time` facet, narrowed to its
`Stopwatch`.** Both came in for `benchmarks/are-we-fast-yet/AreWeFastYet.ns`,
which measures with them ("The on-demand rule" below). `Time` is a value
like the other members, and its one method, `Stopwatch`, is a class
accessor like the platform's own, which answers the same class; an image
names that class by its shortest path, `Stopwatch`, never `time Stopwatch`
(`tests/image.rs`, `a_class_the_platform_hands_out_is_named_by_its_accessor`).
`Stopwatch` answers only what the harness sends, `start` and
`elapsedNanoseconds`, with Primordial Soup's meaning
(`prior-art/primordialsoup/newspeak/Time.ns`): an
unstarted stopwatch reads 0, and `start` on a running one changes nothing
(`goldens/kernel/a_stopwatch_measures_elapsed_time`).

**A `Stopwatch` is an object, not a value.** It keeps its start and its
accumulated time in mutable slots, which §3.1.1 forbids a value, so its
class is `Object new`, as the collections module's `List` is, and it is
the one class of `PlatformFactory.ns` that is not a value. So two
stopwatches are two objects, and an image writes one as an ordinary record
of the class the path `Stopwatch` names, readings included
(`a_stopwatch_is_a_record_of_its_path_named_class`). Its slots hold no
privileged state: the clock is reached through the root's `vmMirror`,
lexically, by the private `currentMonotonicNanoseconds`, a mirror forwarder
over the row `clockMonotonicNanoseconds:ifFail:` whose leaf
(`platform.rs`) reads nothing off its receiver and never declines, as the
transcript's does not.

**The clock's origin is per process.** The leaf answers nanoseconds since an
`Instant` taken on the first reading in the process, shared by every
interpreter in it, so two interpreters' stopwatches agree; an `i64` holds
292 years of nanoseconds, and the conversion saturates rather than wraps.
Primordial Soup's primitive reads the operating system's monotonic clock,
whose origin is per boot. So a **started stopwatch does not survive a
snapshot into another process**: loaded into a fresh process, its start
was taken against another origin, and its elapsed time there is wrong --
negative whenever the loading process is younger than the start was. An
unstarted stopwatch, and a started one loaded in the process that wrote
it, read correctly.

**No program reaches a shipped module's root.** The platform's class is
`PlatformFactory`'s nested `Platform`, whose factory is `new`; the factory
that takes the mirror belongs to the root, and nothing a program can name
answers the root's instance or its class -- the platform's class accessors
answer nested classes only, and the root instance is only ever the
enclosing object of the classes a program is handed. So no program can
build a second root instance over something that is not the mirror, and no
runtime refusal is needed: the bootstrap instantiates `Kernel` and
`PlatformFactory` through the same `Interpreter::instantiate` a host uses,
and a shipped root with a defect of its own -- a mutable slot under `Value`
-- is refused to it like to any program
(`mirror.rs`'s `a_defective_shipped_root_is_refused_to_the_bootstrap`); a
root clause the root rule refuses is a resolve error under `resolve_kernel`
as under `resolve`.
Until 2026-09-27 the platform was the module's root instance itself, and the
runtime refused every instantiation of a shipped root but the bootstrap's;
a second platform over something that is not the mirror would otherwise
have run `Transcript>>show:`'s leaf through the primitive method's fast
path, which never reads `vmMirror`, and failed through its `SendMirror`
fallback, which does. The factory pattern makes the paths agree with plain
Newspeak instead: every object whose code sends to `vmMirror` reaches it
through the one root instance the bootstrap checked.

That the root instance stays unreachable rests on more than naming: it
must never become the answer to anything a program sees, and the one place
the runtime hands a receiver to a program is a `MessageNotUnderstood`,
whose `receiver` slot is public. So no code whose receiver is the factory
instance may ever send it a message it does not understand -- today the
only sends to it are the host's `newPlatform` at boot, `newPlatform`'s own
`Platform`, the class accessors' `outer PlatformFactory <Name>`, each
naming a nested class it declares, the nested classes' superclass
clauses, run with it as receiver, and the lazy `collectionsModule`
initializer's own. `CollectionsFactory`'s instance is held the same way:
only `PlatformFactory`'s private `collectionsModule` slot keeps it, and the
facet's classes have it as their enclosing object; the sends to it are
`newCollections` from `Platform`'s `collections` initializer, the facet's
class accessors' `outer CollectionsFactory <Name>`, and its nested classes'
superclass clauses. Whoever edits either module (or writes another
`XFactory` root) keeps it that way: a send to the root that
can miss, or a method of the root that sends to `self` something it may
not understand, would hand the root, and with it `usingVmMirror:kernel:`
through its class, to any program that catches the error.

**The authority to run a leaf lies with an object, not with code.**
Weighed with the user on 2026-09-27: making the two paths agree the other
way -- a mirror send that never evaluates its receiver, so that shipped
code runs its leaves whatever `vmMirror` holds -- would move that authority
from the mirror object to the shipped code. That is harmless for `Platform`
today, since a second platform can do nothing the program's own `platform`
cannot, but not for a module a program is only handed a facet of, as
Primordial Soup hands out its `internalKernel` only through `Kernel
wrapping:`. Any instance of that module's classes a program reaches gives it
the class through `class`, and a receiver-blind mirror send would let it
build its own instance with every leaf the module uses, without ever holding
the mirror. The factory pattern keeps the object-capability rule Newspeak is
built on: the authority is the root instance, which holds the real mirror by
construction and is never handed out, so the fast path's assumption that
`vmMirror` is the mirror is true rather than trusted. Hiding the class
instead (`class` answering `nil`) was rejected too: the class stays
reachable through mirrors and the debugger, and `Value`'s fallback
equality, `printString` and the tools read `class`.

**A facet module keeps its full-authority classes private.** A module's
root is never handed out, but its nested classes are ordinary Newspeak: a
class object carries its enclosing object's authority, so each instance of
a nested class reaches whatever the root holds through its *real*
enclosing root instance, and anything that reaches the instance reaches the
class through `class` and can make more. So a shipped module that programs
receive only as a restricted facet -- the I/O, network and debugger modules
still to come -- must not make a nested class with the module's full
authority publicly reachable, through a lazy slot, a class accessor or the
class of any object it hands out; the facet's own classes are what a
program may reach.

**An image names the platform and what it reaches by path.** The platform
is never written field by field: the encoder names it, and every shipped
instance or class it reaches, by its path of public members from the
platform ("Platform paths in the image" below); a second platform is an
ordinary record of the class the path `Platform` names.

## The on-demand rule

**`PlatformFactory.ns` and `CollectionsFactory.ns` are widened only on
demand.** An accessor exists because a module on `corpus/runnable.toml` or
a program under `benchmarks/` needs it, and the whitelist entry or the
program that demanded it is named in a comment beside it: the comments of
`Time` and `Stopwatch` name `benchmarks/are-we-fast-yet/AreWeFastYet.ns`.

This is not minimalism for its own sake. A capability nothing on that list or
under `benchmarks/` asks for has no test that would catch it rotting, so it is a liability rather than an
asset. A module that needs something nobody has asked for is a reason to widen
the whitelist deliberately — with a real module, run end to end, its output
recorded — not a reason to build a speculative library.

The `collections` namespace is the worked example. It hands out exactly one
class of the library, `List`, because exactly one is asked for -- `Ducts.ns`,
`NewspeakASTs.ns` and `V5BytecodeSimulation.ns` bind `platform collections
List` -- and `List` answers exactly the six selectors the corpus and the
goldens send: `new`, `add:`, `at:`, `at:put:`, `do:` and `size`. The corpus
also asks that namespace for `Map`, `IdentityMap`, `IdentitySet`, `WeakArray`
and `Collection`. Those name classes the runtime does not have, and inventing
them — or worse, aliasing them onto `List`, which is neither a dictionary nor
a set — would be a collections library built without a caller. Modules
needing them stay off the whitelist.

## Large integers

§5.1.1: "*Note that integers are not limited in size*", and it puts no bound
on the *n* in the digit sum it defines a literal by. So an `Integer` is
unbounded, and **every integer has exactly one representation**: a
`SmallInteger` (`Value::SmallInteger(i64)`, class `Integer`) when it fits
`i64`, a `LargeInteger` object otherwise. `Mutator::new_integer` is the only
way to make a `Value` from an `ArbitraryInteger`, and it decides which, so
every leaf, which answers through it, answers an immediate again once a
result fits. The eight-byte value (road step 2's second plan,
[object-model.md](object-model.md), "Planned: an eight-byte value") moves the
boundary from 64 bits to 61; the rule stays.

**The class.** `LargeInteger` is a kernel class, a subclass of `Integer`,
which stays the class of the immediate integers: §5.1.1 asks only that an
integer "obeys the protocol of class `Integer`", so `3 class` keeps
answering `Integer`. `Object` declares no accessor for it -- a program meets
it only as an integer's class (`interpreter/kernel.rs`'s
`objects_class_accessors_are_exactly_the_nameable_builtins`) -- and an image
names it by builtin class tag 31. It is a value object (§3.1.1): `==` is
`Value`'s, `^self = other`, and `=` and `hash` come from its sign and
magnitude, never from an identity hash, so a value whose slot holds one
compares and hashes by state too
(`goldens/kernel/a_value_with_a_large_integer_slot_compares_by_state.ns`).
With one representation per integer, the class-tight `=` loses nothing: no
`LargeInteger` is ever the same number as an `Integer`. An instance of a
subclass of it, or `LargeInteger new`, holds no digits; every leaf
declines it on the missing payload, never on the class, and the send falls
to `Object`'s methods
(`goldens/kernel/a_subclass_of_large_integer_is_no_number.ns`).

**The leaves.** `LargeInteger` forwards `+ - * // \\ < > <= >= =`,
`bitAnd:`, `bitOr:`, `bitXor:`, `<<`, `hash`, `negated`, `abs`, `asFloat`,
`printString` and `asString` to twenty rows of its own, `largeInteger:add:ifFail:` to
`largeIntegerAsFloat:ifFail:` (`builtins/numbers.rs`,
`execute_large_integer`). `Integer`'s leaves keep
their immediate path and promote where it has no answer: a sum, difference
or product that does not fit `i64` is recomputed in `i128`, where two
`i64`s' always fits, and `i64::MIN // -1`, `i64::MIN negated` and
`i64::MIN abs` answer `2^63`. The inlined `IntegerBinary` never allocates:
it declines a result that does not fit and falls back to the send, whose
leaf promotes ([compiler.md](compiler.md), "Arithmetic inlining";
`goldens/arithmetic/an_overflowing_integer_operation_answers_a_large_integer.ns`).
Both classes' arithmetic and ordering accept an argument of either integer
class (`integer_operand`), so `5 + big` and `big + 5` both answer
(`goldens/arithmetic/large_integer_arithmetic.ns`). A `Float`'s leaves
convert a `LargeInteger` argument to the nearest `double`
(`ArbitraryInteger::to_f64`, rounded to nearest with ties to even, infinite
beyond `f64::MAX`), as Primordial Soup's `LargeInteger::AsDouble`
(`prior-art/primordialsoup/vm/large_integer.cc:1242`) serves, and as they
convert an `Integer` argument; an integer's `+ - * < > <= >=` convert
themselves and a `Float` argument to doubles the same way, whichever integer
class the receiver is, so the two directions agree
(`goldens/arithmetic/a_float_meets_a_large_integer.ns`). An argument that
is neither an integer nor a `Float` is a type mismatch naming "a numeric
argument" (`goldens/errors/a_large_integer_meets_nil.ns`). That is a derived
choice, pinned by goldens, which the numeric tower may revisit ("Gap: an
integer has no `/`" below). `//` and `\\` by zero raise "division by zero"
from either class's leaf
(`goldens/errors/large_integer_floor_division_by_zero_is_uncaught.ns`,
`goldens/errors/floor_modulo_by_zero_is_uncaught.ns`).

**`//` rounds toward negative infinity**, so `7 // -2` and `-7 // 2` are
both `-4`: Primordial Soup's `Integer>>//`, commented "Integer division,
rounded toward negative infinity. The remainder of this division is given by
`\\`" (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1958`),
and `num-integer`'s `div_floor`. `checked_floor_divide`
(`builtins/numbers.rs`) is the one copy of the rule for two `i64`s, which
the leaf and `IntegerBinary` share, and `ArbitraryInteger::floor_divide` the
one for everything else
(`goldens/arithmetic/floor_division_rounds_toward_negative_infinity.ns`).

**`\\` is the remainder of that division**, so it takes the divisor's sign:
`7 \\ -2` is `-1` and `-7 \\ 2` is `1`. For two `i64`s, `floor_modulo`
(`builtins/numbers.rs`) corrects Rust's truncating remainder, taken with
`wrapping_rem` so that `i64::MIN \\ -1` answers `0` where `%` would
overflow. For everything else, `ArbitraryInteger::floor_modulo` derives it
from `floor_divide`, as the dividend less the quotient times the divisor,
rather than adapting `num-integer`'s `mod_floor`, so the quotient and the
remainder can never disagree
(`goldens/arithmetic/floor_modulo_takes_the_divisors_sign.ns`). `\\` has no
inlined fast path: it is no `IntegerBinary` operation, so every `\\` is a
send to the leaf.

**`bitAnd:` is over two's complement**, as Primordial Soup's and Squeak's
are: a negative number's sign bits extend without end, so `-1 bitAnd: x`
is `x` and `-256 bitAnd: 255` is `0`, for either representation in either
position. Two `i64`s are and-ed as they are; `ArbitraryInteger::bit_and`
lays both operands out in two's complement one digit longer than the
longer magnitude, so that the top digit holds the sign, ands the digits,
and turns a negative result back into sign and magnitude. Both are checked
against `i128` and by
`goldens/arithmetic/bit_and_is_twos_complement.ns`; a sign-and-magnitude
`and` would answer `0` for `-1 bitAnd: 2^64`, which is `2^64`. `bitOr:` and
`bitXor:` are over two's complement the same way: `ArbitraryInteger`'s
`bit_and`, `bit_or` and `bit_xor` are one `bitwise` with a different digit
operation, checked against `i128` and by
`goldens/arithmetic/bit_or_and_bit_xor_are_twos_complement.ns`, and
`-1 bitXor: 2^64` is `-(2^64) - 1`.

**`<<` shifts the magnitude and keeps the sign**, which is exact for a
negative receiver: `-9223372036854775808 << 1` is `-18446744073709551616`,
and `-1 << 63` is exactly `i64::MIN`, still an `Integer`
(`goldens/arithmetic/shift_left_widens_into_a_large_integer.ns`). A negative
count raises an error that says the count must be non-negative. A result
whose digits cannot be reserved raises "cannot allocate" rather than
aborting: `ArbitraryInteger::shift_left` reserves with `try_reserve_exact`,
as `Array new:` does, because the count is the program's ([OPEN-WORK.md](../OPEN-WORK.md)
says what that does not yet cover). `0 << n` answers `0` without
allocating. The selector and its meaning are Primordial Soup's
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1599`).

**`asFloat` answers the nearest double**, ties to even: an `Integer`'s by
Rust's `as`, exact up to 2^53, and a `LargeInteger`'s by
`ArbitraryInteger::to_f64`, the conversion a `Float`'s leaves already apply
to an integer argument. A `Float` prints as Rust's `Display` writes it, the
shortest digits that read back as the same double, so `9223372036854775808
asFloat` prints `9223372036854776000`
(`goldens/arithmetic/as_float_and_as_string_fixed.ns`).

**The payload.** A `LargeInteger`'s object carries
`ObjectData::LargeInteger(ArbitraryInteger)`: a sign and a magnitude of
64-bit digits, least significant first, in a `Box<[u64]>` in the Rust heap
beside the arena. It is canonical -- no zero top digit, and zero never
negative -- so derived equality is value equality, and it holds no handle,
so the tracer has nothing to follow in it. The object space (road step 3)
moves the digits inline, into a 64-bit-indexed object with the sign in the
header's negative bit, as Primordial Soup's `kNegativeBit`
(`prior-art/primordialsoup/vm/object.h:48`); one class, not Spur's two
([object-model.md](object-model.md), "Planned: the object layout"). That is
why the arithmetic is ns-next's own rather than a crate's: a crate's numbers
live in its own `Vec` in the Rust heap, which is the payload split the
object space removes, and every operation would copy between the two. The
Smalltalk-family engines write it themselves too -- Primordial Soup's
`vm/large_integer.cc`, about 1,600 lines -- while SOMns delegates to
`java.math.BigInteger`
(`prior-art/somns/src/som/primitives/arithmetic/AdditionPrim.java`), which
lives in the same JVM heap as its other objects; a Rust crate's heap is not
ours. So `crates/ns-runtime/src/arbitrary_integer/` works on `&[u64]` and
`&mut [u64]`, which is what the object space lends, and knows nothing of the
heap or of `Value`.

**The algorithms are adapted from `num-bigint` and `num-integer`**, one file
per concern: `addition.rs` (carries, borrows and comparison),
`multiplication.rs`, `number_theoretic_transform.rs`, `division.rs` and
`conversion.rs` from `num-bigint`, and floor division in `mod.rs` from
`num-integer`. Only portable bodies are taken, with 128-bit intermediates:
upstream's x86 carries through `core::arch` and its `div` through `asm!`
would need `unsafe`, which `ns-runtime` forbids. Multiplication chooses by
the shorter operand's length, at `num-bigint`'s thresholds, which it chose
by benchmark, testing the rows in order:

| Shorter operand | Algorithm |
|---|---|
| up to 32 digits | long multiplication |
| the longer at least twice as long | half-Karatsuba: only the longer is split in halves |
| up to 256 digits | Karatsuba |
| up to 512 digits | Toom-3 |
| above 512 digits | the number-theoretic transform |

The transform computes the product's digit convolution modulo primes above
2^63, in Montgomery form, and combines the residues by the Chinese
remainder theorem: modulo **two primes** when the operands, repacked into
values of at least 43 bits, keep every term of the convolution below the
two primes' product (`bits_per_digit_for`), and modulo **three primes** on
whole digits otherwise. Division by a one-digit divisor is a single pass
(in half-digit steps for a divisor that fits 32 bits, so it needs no `u128`
division); otherwise it is Knuth's **algorithm D**, and above 64 digits in
the divisor and 128 in the dividend it is **Burnikel and Ziegler's**
recursive division, whose blocks of at most 64 digits go to algorithm D.
Decimal text is made by repeated division by `10^19`, the largest power of
ten a digit holds, and parsed a digit's worth of radix digits at a time.

The transform and Burnikel-Ziegler were taken on a decision with the user
(2026-09-29), beyond the four algorithms -- schoolbook, Karatsuba, Toom-3
and algorithm D -- first chosen, so that multiplying and dividing very long
operands does not stay at Toom-3's and algorithm D's cost. **Not taken:**
`num-bigint`'s divide-and-conquer radix conversion, so `printString` of an
integer of *n* digits costs time quadratic in *n*
([OPEN-WORK.md](../OPEN-WORK.md)). The adapted code is checked against
`i128`, against schoolbook multiplication across every threshold
(`every_algorithm_agrees_with_schoolbook` in `multiplication.rs`, beside
the transform's and the division's own cross-checks), against values
computed in Python and pasted into the tests, and by
`goldens/arithmetic/large_integer_algorithms_agree_at_every_size.ns`.

**The licensing.** Both projects are MIT OR Apache-2.0; ns-next takes them
under MIT, whose terms require the notice to travel with the code.
`THIRD-PARTY-NOTICES.md` at the repository root carries both licence texts
and the upstream commits, and every file under `arbitrary_integer/` that
holds adapted code opens with a header naming the upstream file, the
commit, the copyright and what was changed for ns-next. `num-bigint`,
`num-integer` and `num-rational` are submodules under `prior-art/`,
recorded in `prior-art/PROVENANCE.md`, read and never built like the
others. `num-rational` is there for the numeric tower ("Gap: an integer has
no `/`" below): its `Ratio` is the model for `Fraction`, and its exact
conversion of a float into a fraction is what a correct comparison between
an `Integer` and a `Float` needs; nothing is taken from it yet.

## Deviation: a non-integral literal is a binary float

§5.1.1: "Non-integral numbers denote an object that obeys the protocol of class
`FixedPointNumber`" — an exact decimal type, chosen there so that "financial
applications in particular" get "accurate decimal numbers" and users are not
exposed to floating point's "counterintuitive ways". `2.5` here is an `f64`
(`compiler/lower.rs` parses the literal's text straight into one), so the
arithmetic is binary floating point: `(0.1 + 0.2) = 0.3` answers `false` where
an exact decimal type answers `true`.

The specification records the same divergence in its own implementation
("non-integral numbers are currently represented as instances of the Squeak
class `Fraction`. This is a bug"), and it names the open design question —
whether a user chooses the numeric implementation — rather than settling it.
A golden for the rule would have to be skipped behind a whole numeric tower,
so the deviation is recorded here instead.

## Gap: an integer has no `/`

`7 // 2` answers `3`; `7 / 2` ends in `doesNotUnderstand:`. `Kernel.ns`
declares `/` only on `Float`, forwarding to the mirror row
`float:divide:ifFail:`, while `Integer` declares `//` alone, over
`integer:floorDivide:ifFail:`. So `7.0 / 2` works and `7 / 2` does not, which
is asymmetric enough to surprise. `asFloat` does not close the gap: it lets a
program ask for an inexact quotient explicitly, `7 asFloat / 2`, as the Are
We Fast Yet harness computes its scores, but `7 / 2` still answers nothing.

**This is not a specification violation.** The specification mentions `Number`
exactly twice — §3.1.1, listing it among the value objects, and §5.1.1, on
non-integral *literals* — and fixes no arithmetic protocol anywhere. Which
selectors `Integer` answers is a library question, and §8.3 leaves the
platform's modules to provide classes "some of which are standard and mentioned
in this specification". `/` is not one of them. Formally this is the on-demand
rule doing its job: nothing on `corpus/runnable.toml` or under `benchmarks/`
asks for it.

**Step 6's first plan left it open deliberately** (decided with the user on
2026-09-21): moving the library into `Kernel.ns` kept every recorded outcome,
and adding `Integer>>/` would have been a new one. The second plan's `Number`
class did not settle it either: `Number` is the place between `Value` and
`Integer`/`Float` that §3.1.1 names, and **it is where the numeric tower
goes when there is one** — what `Integer` and `Float` share moves up to it,
as `to:do:` and `to:by:do:` have ("What exists" above), and a `Fraction`
would hang beside them. Declaring it before the tower is what makes that a
later addition rather than a later restructuring. **The question still has to
be settled**, and the choice is as
it was. Primordial Soup declares both on
`Integer` (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`):
`/` as a primitive falling back to `^other divideFromNumber: self`, and `//`
beside it, commented "Integer division, rounded toward negative infinity".
That works there because Soup has a `Fraction` class, which its comparison
methods test for. ns-next has no `Fraction` and no exact decimal type, so
writing `Integer>>/` into `Kernel.ns` means choosing: answer a `Float`, which
diverges from Smalltalk and from Soup and quietly makes an exact quotient
inexact; build the numeric tower the deviation above already defers; or keep
the selector absent and say so. What was decided is only that it has to be
settled eventually, deliberately rather than in passing; when, and by which
step, is not.

**The class-tight `=` of 2026-09-23 makes the tower needed, not merely
deferred.** `1.0 = 1` and `1 = 1.0` both answer `false` now, which is
symmetric and keeps the `=`/`hash` contract, but it is not what a program
comparing numbers expects, and no rule inside the library can give a better
answer while an `Integer`'s hash is the integer and a `Float`'s is a hash of
its bits. What resolves it is one piece of work, not two: a numeric tower
under `Number` — the class that now exists to hold it — with cross-type
comparison and a hash that matches it, `/` answering an exact quotient, and
the `Fraction` that needs. `LargeInteger` exists without it ("Large
integers" above) and leaves `Integer>>/`, `Fraction` and the cross-type
comparison to the tower. The contract such a tower
has to meet is written down in Primordial Soup's `testFloatHash`
(`prior-art/primordialsoup/newspeak/KernelTests.ns`): `3 asFloat hash` must
equal `3 hash`, for `-3`, `0` and the extremes alike. Until then the
class-tight `=` is the stopgap that keeps the contract by answering
`false`, and only `=` is class-tight: in both directions, a `Float`'s
arithmetic and ordering convert an integer argument of either class,
`Integer` or `LargeInteger`, and an integer's `+ - * < > <= >=` convert
themselves and a `Float` argument, to the nearest `double`, exact up to
2^53, as Primordial Soup's primitives do (`vm/primitives.cc`,
`IS_FLOAT_OP`). `//` and `\\` take integers only, since `Float` has
neither selector (`builtins/numbers.rs`'s module note; "Large integers"
above).

## The core library as Newspeak source over a VM mirror

The shape of a real collections library and of
the mirror-based reflection protocol the platform would hand out are all open;
the last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md). Beyond that, "widen
when a whitelisted module demands it" still governs what gets added.

**Step 6 also carried the rest of §3.2** (decided 2026-09-18, built
2026-09-25 by its third plan): `Class`, `Behavior` and `Metaclass` are
declared kernel classes, and a class object's own class is its metaclass
("What exists" above). Both references declare exactly these three in their
kernel source (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`), and the reflection work needs them — a
mirror's test for a metaclass is `Metaclass = classOf: reflectee`, which
needs the instance-of relation reified. The flat metaclass hierarchy that
§3.2 also requires came first: it was corrected ahead of 3c, because the
primary factory's special cases depended on it
([object-model.md](object-model.md), "Class sides and metaclasses").

What is decided is *where a builtin may live*, and it is a constraint on future
work rather than a wish. Everything in this runtime must stay optimizable,
builtins included: a builtin must eventually be able to become native code. A
Rust native that calls back into Newspeak cannot. A `do:` written as a Rust
loop that re-enters the interpreter can never have its block inlined into the
loop, never be type-specialized, and never be compiled as a unit — not because the JIT
is immature, but because no compiler optimizes through an opaque Rust function
body. That is a permanent ceiling on the methods that are hottest in real code.

Both reference implementations avoid this rather than accept it. V8 writes the
builtins that involve control flow or call user code in Torque/CodeStubAssembler,
a form its own compiler understands and can call without crossing a C++ boundary,
and keeps plain C++ runtime functions only for the slow paths it is content to
leave opaque. Cog writes `do:` in Smalltalk and keeps only irreducible leaf
operations as primitives.

**The rule: a native that calls back into Newspeak must not stay a Rust native.**
A native that is a leaf computation should stay one, and should get an inlined
fast path instead. No native calls back into Newspeak: each leaf that runs a
block answers it as a trampoline step instead of calling it:

| Selector | How it runs its block |
|---|---|
| `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `ifTrue:`, `ifFalse:`, `and:`, `or:` | Lowered by the compiler for a literal block, with the kernel method kept as the guarded fallback. There is no leaf: `True`'s or `False`'s method is a quick method, and one whose body is `^<parameter> value` answers the block as a trampoline step instead of calling it ("`True` and `False` are quick methods" below; [execution.md](execution.md), "The trampoline") |
| `whileTrue:`, `whileFalse:` | The same, both halves: the leaf is a trampoline step that returns the condition block, and its continuation activates the body or ends the loop |
| `value`, `value:`, … | A trampoline step that returns the block to activate: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | A trampoline step: it answers the block for the first element, and its continuation activates it for each next one ([execution.md](execution.md), "No re-entry from Rust"). `Array>>do:` is a kernel method forwarding to it; a Newspeak loop is "Planned: real bodies for control flow and `do:`, before the baseline JIT" below |

So no native calls back into Newspeak, which the root discipline needs as well:
no Rust frame may be live below a safepoint ([memory.md](memory.md), "No Rust
frame is live below a safepoint"). Everything else the mirror holds — `size`,
`at:`, `class`, `hash`, `printString`, the arithmetic, the comparisons,
`asString`, `asSymbol` — is a leaf, and its computation stays in Rust.

**Every builtin method is a real method** (the user's direction, 2026-09-12;
the mirror decided 2026-09-21), one that lookup, mirrors and the debugger see.
The computation of a leaf did not move; what changed is that it is reachable
as a method, and that Rust is reached through exactly one object. That is what
lets a debugger step into `do:` and edit-and-continue reach library code
([liveness-and-reflection.md](liveness-and-reflection.md)), and what gives the
JIT something to work with: every send inside library code gets a call site
of its own, whose inline cache records what flows through it, where a Rust
native records nothing. Library code is shared by every caller, so those sites
are often megamorphic — `action value:` inside a Newspeak `do:` would see
every block in the program — and the feedback pays off once the JIT inlines
library methods into their callers, not from the caches alone.

### The VM mirror is the whole Rust boundary

**§8.3.1 settles the shape, and it is not a matter of taste.** "The VM mirror
supports the set of operations traditionally implemented as primitives in
Smalltalk. *There is no syntax for a primitive call in Newspeak. A primitive
call is a message send to the VM mirror.*" The caveat below it calls the
canonical implementation's practice of reaching primitives through Squeak
"a bug".

So Primordial Soup's shape is the one ns-next does *not* take, although an
earlier version of this document named it as the target. Soup marks a kernel
method with a primitive pragma in a comment and carries a Newspeak fallback
body for when the primitive fails — which is precisely the primitive-call
syntax §8.3.1 rules out, written into a comment because the grammar has no
place for it. SOMns takes the other shape, and it is the one required and the
one built: `prior-art/somns/core-lib/Kernel.ns` declares
`class Kernel vmMirror: vmMirror = Object` and writes each kernel method as a
send to the mirror — `public + argument` answers
`vmMirror int: self add: argument`, `public == other` answers
`vmMirror object: self identicalTo: other`. The receiver is passed as an
argument, because the mirror is always the receiver.

**The mirror is the one instance of a builtin class, `BuiltinClass::VmMirror`,
whose method table carries every leaf as an enumerated primitive.** The
machinery of step 3a is kept whole: a row is a `MethodEntry::Primitive`
carrying `Primitive::Mirror(row)`, found by the ordinary lookup
([dispatch.md](dispatch.md), "Primitives"). The builtin classes keep their
identity — a value still needs a class for lookup and for its dispatch id —
while their behaviour is `Kernel.ns`. SOMns builds its mirror the same way
(`src/som/vm/ObjectSystem.java`, `constructVmMirror`): an object without
fields over a synthetic class holding the primitives. The mirror's class and
its table are built once, at bootstrap, from the same static table the
compiler reads, which is what lets the compiler bind a mirror send to its row
([compiler.md](compiler.md), "Mirror sends").

**A mirror is a capability**, which is the second reason to prefer it and the
one §8.2 gives for mirror-based reflection generally. Whoever is not handed
the mirror cannot call a primitive. Under a pragma, any method anywhere can
claim one and nothing marks where the VM surface lies. The bootstrap creates
the mirror and hands it to the factories of the shipped core-library
modules alone, `Kernel vmMirror:` and `PlatformFactory
usingVmMirror:kernel:`, whose roots no program reaches ("The platform"
above); a user module would receive it only if a shipped module passed it
on, and none does: each root keeps it in a private slot. The planned
`become:` ([object-model.md](object-model.md), "Planned: `become:` through
forwarders") will keep it that way: every instance a platform path names is
a value (`Platform`, `Transcript`, `KernelNamespace`, `Time` and
`Collections` are declared `= Value new`) and every class it names is a class object, and a
program's `become:` will refuse both in either direction; the kernel-made
non-value objects a program can reach, such as a VM error's `Error` and its
`Message`, lie on no path from shipped code to the mirror, and a kernel
method whose receiver a program forwarded fails the planned slot-access
check ([object-model.md](object-model.md), "What a slot access checks")
rather than writing kernel-private state into a program object; the path to
the mirror runs only
through class objects and the value roots of `Kernel`, `PlatformFactory` and
`CollectionsFactory`, so no program will be able to redirect a reference
that leads shipped code to the mirror. Six things keep it
that way,
and loosening any one of them lets a program claim every primitive:

- Only `resolver::resolve_kernel` records `Binding::MirrorSend`, the mark the
  compiler's recognizers read, and only the shipped core-library modules —
  `Kernel.ns`, and `PlatformFactory.ns` and `CollectionsFactory.ns` from
  `interpreter/core_library.rs`'s table — are resolved with it; a user module goes through `resolve`, so
  copying shipped text gets an ordinary send to whatever that module's own
  `vmMirror` holds.
- The bootstrap checks what the resolver cannot: that `Kernel`'s root class
  declares exactly one slot, the immutable `vmMirror`, and that after
  instantiation it holds the mirror (`interpreter::kernel`), and that
  `PlatformFactory`'s `vmMirror` is an immutable, eager slot holding it
  (`interpreter::core_library`); and no program reaches any shipped module's
  root instance or root class (the factory pattern, "The platform" above),
  so no program can make another root instance, and every object whose
  code sends to `vmMirror` reaches the one the bootstrap checked.
- No program can name the mirror's class: `Object` declares no accessor for
  it or for `Top` (`interpreter/kernel.rs`'s
  `objects_class_accessors_are_exactly_the_nameable_builtins` pins the
  accessor set, and `compiler::primitive_method` recognizes no accessor for
  either), so a bare name for it ends in `doesNotUnderstand:`; and the image
  decoder refuses builtin class tag 14 wherever the graph names a class
  ([image-format.md](image-format.md), "Verification").
- The image encoder refuses an image whose graph reaches the mirror, its
  class or its class's metaclass.

The activation objects the exceptions are built on are a capability of the
same kind — one reads another frame's registers and unwinds the stack — and
are held the same way: `Activation` is private to `Kernel.ns`, `Object`
declares no accessor for it, a program reaches one only through a kernel
method's local, and an image names its class only as the header of an
activation record ("Exceptions" below).
- `decide_factory` refuses to instantiate the mirror's class, so there is
  never a second mirror.
- `SendMirror`, the instruction that runs a leaf with no lookup, is emitted
  only for a `MirrorSend` and has no wire tag, so no artifact can carry one.

The platform keeps its separate role: §8.3 distinguishes the platform object,
which is the environment, from the VM mirror, which is the virtual machine.
Loading a module belongs on the mirror, as it does in SOMns, not on the
platform.

**The cost the design accepted, and what recovers it.** Through the mirror a
leaf send is a send to the kernel method, an activation, a lexical send of
`vmMirror`, the mirror send and the return, where it was one cached send.
SOMns carries this because Graal optimizes it away; ns-next has no JIT yet.
Three things take it back out. The integer leaves are inlined under a type
guard (`Instruction::IntegerBinary`, [compiler.md](compiler.md), "Arithmetic
inlining"), built first for exactly this reason. A kernel method whose whole
body is a mirror send is a primitive method, whose leaf `act` runs without
activating it ([dispatch.md](dispatch.md), "Primitive methods"). And inside a
kernel method the mirror send's failure block is lowered inline
(`Instruction::SendMirror`, [compiler.md](compiler.md), "Mirror sends"). For a
later JIT the mirror's two inlining steps are the easiest case an inliner
meets: `Integer>>+` is a one-expression body, and the send to the mirror is
monomorphic by construction, since the mirror is a single object — its guard
folds away once the first step is inlined. That is Strongtalk's ordinary
procedure, not an edge case.

### The shape of a kernel method

```newspeak
public + other = ( ^vmMirror integer: self add: other ifFail: [^super + other] )
```

Every kernel method that reaches a leaf is this: the mirror selector, with
`self` and then the method's own parameters in order, and a literal failure
block that resends the same message to `super`. The rest reach none — the
quick methods ("`True` and `False` are quick methods", "The nil protocol"),
the accessors for the predefined classes, and the few ordinary methods such
as `True>>printString` or `Value`'s derived equality. The failure path is an explicit `ifFail:` block
rather than something the runtime does implicitly (decided with the user on
2026-09-21): a real send to the mirror, as §8.3.1 requires, whose failure
path is visible Newspeak code, as Cog's method body is. SOMns uses the same
shape for its I/O primitives (`prior-art/somns/core-lib/Files.ns`,
`vmMirror file: self readAt: pos ifFail: errf`).

The one rule `[^super …]` reproduces what a declining native used to do by
itself: a receiver of the wrong shape — an instance of a user subclass of
`Array`, which carries no array, or a platform object sent another platform
object's selector — reaches the superclass's method, `Object`'s, and a
selector `Object` does not have ends in `Object`'s `doesNotUnderstand:`
through §5.10's super failure path, with the same message text.
`goldens/errors/native_that_fails_on_its_receiver_falls_to_objects` and
`goldens/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class`
pin it, and `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`
pins it family by family. `Object`'s own leaves never decline, so its
blocks are unreachable; they keep the shape so that one rule, and one
recognizer, covers every method. The exception machinery's mirror sends
are the exception to the shape: they name another subject — an activation,
an exception — and their failure blocks answer a value (`ifFail: [nil]`,
`ifFail: [false]`) rather than resend to `super`. Every method is `public`
but `Object`'s `doesNotUnderstand:`, which §5.5 makes `protected`, and the
machinery's own: `Exception`'s `hostSelector` and `hostReceiver` and
`MessageNotUnderstood`'s overrides of them (`protected`), `Exception`'s
handler search (`private`) and `Block>>valueOnUnwind:` (`private`) — and
`Metaclass`'s two `thisClass` helpers (`private`).

Two things about the shape are enforced rather than conventional:

- **A literal block may appear in a mirror send only as its final `ifFail:`
  argument.** `interpreter::kernel`'s `check_kernel_shape` refuses anything
  else at bootstrap (`check_mirror_send_arguments`). The reference lowering
  marks a mirror send whose failure block the optimized lowering inlines as a
  `SendInlinable`, and that mark covers the whole send, so it would exempt
  from the depth count a literal block the leaf itself activates — as
  `whileTrue:`'s would be, were a kernel method to pass it one — which the
  optimized lowering counts; the two lowerings would then overflow at
  different depths ([compiler.md](compiler.md), "How the limit counts"). A
  block a leaf activates is therefore passed as a parameter, which neither
  lowering marks.
- **A cascade to `vmMirror` is not a mirror send.** The resolver marks only a
  plain explicit-receiver send, so a cascade compiles as ordinary sends to the
  mirror object — which still run their rows, sent, with a real failure block
  ([compiler.md](compiler.md), "Mirror sends"). `Kernel.ns` uses none.

### `True` and `False` are quick methods

`True` and `False` implement their whole protocol in `Kernel.ns`, one method
per branch, as every prior-art kernel does — SOMns's
(`prior-art/somns/core-lib/Kernel.ns`), Primordial Soup's
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`) and
Newspeak on Squeak's (`prior-art/newspeak/squeak/KernelForSqueak.ns`):

```newspeak
public ifTrue: trueBlock ifFalse: falseBlock = ( ^trueBlock value )   (* True *)
public and: block = ( ^false )                                         (* False *)
```

No method tests its receiver and none reaches the mirror; `Boolean` itself
declares nothing, and each of the two declares the `hash` its `=` demands.

**Every such body is a quick method**, Squeak's and Cog's name for a method
that answers a constant or an argument and is run without an activation. A
kernel method whose whole body is `^true`, `^false` or `^nil`
(`MethodFastPath::Constant`), `^self` (`MethodFastPath::Receiver`),
`^<parameter>` (`MethodFastPath::Parameter`), `^<parameter> value`
(`MethodFastPath::ValueOfParameter`) or `^<parameter> cull: self`
(`MethodFastPath::CullParameter`, "The nil protocol" below) is recognized
by `compiler::primitive_method` and answered by `act` on every send, like
the other two fast paths ([dispatch.md](dispatch.md), "Primitive
methods"). `ValueOfParameter` answers a closure argument as the block to
activate, as a trampoline step, marked as a control-flow step for a Boolean
receiver; any other argument activates the method, whose body then sends
`value` for real. Only shipped core-library source's methods are
recognized, and `NS_NEXT_NO_PRIMITIVE_METHODS` does not switch them off,
since they run no leaf.

The reason is the depth count, not speed alone. The reference lowering sends
`n = 0 ifTrue: [0] ifFalse: [n * (self fact: n - 1)]` as a `SendInlinable`
that reaches `True>>ifTrue:ifFalse:`; activated, that method and the block
its `value` activates would count two activations per level where the
optimized lowering, which inlines the conditional, counts none. As a quick
method it is never activated, and the block step it answers is exempt
exactly as a leaf's was ([compiler.md](compiler.md), "How the limit
counts"). `interpreter::kernel`'s `QUICK_METHODS` table test pins the fast
path of every `True` and `False` method, and
`goldens/kernel/recursion_through_and_or_counts_alike_under_both_lowerings`
the agreement.

Two semantic decisions came with the move (decided with the user on
2026-09-24):

- **`and:`, `or:`, `&` and `|` do not coerce.** `true and: [3]` answers `3`,
  as in every prior-art kernel, under both lowerings — the compiler's
  inlined `and:`/`or:` moves the block's value into the result as it is
  (`goldens/language/and_or_short_circuit`,
  `goldens/language/and_or_answer_their_arguments_value`).
- **An argument that is no block is sent `value`, and nothing answers it**,
  as in Primordial Soup: there is no `Object>>value`, so `true ifTrue: 3`
  ends in `doesNotUnderstand:`
  (`goldens/errors/a_conditional_sends_value_to_its_argument`), while a
  side that decides alone never looks at its argument — `false ifTrue: 3`
  answers `nil`
  (`goldens/language/a_short_circuited_conditional_ignores_its_argument`).

A user subclass of `True` inherits these methods, needing no payload for
them, where a subclass of `Boolean` inherits nothing from it and falls to
`Value`'s and `Object`'s methods, hashing by its state like any other value
(`goldens/kernel/a_subclass_of_true_inherits_its_methods`,
`goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`,
`goldens/kernel/a_subclass_of_boolean_hashes_as_a_value`).

### The nil protocol

§5.1.3 names `nil`'s class `UndefinedObject` and has it answer `isNil`
with true. `Object` declares the rest of the protocol every prior-art kernel
has — `isNil`, `notNil`, `ifNil:`, `ifNotNil:`, `ifNil:ifNotNil:` and
`ifNotNil:ifNil:` — answering for every object that is not `nil`, and
`UndefinedObject` declares the same six the other way round, as SOMns's
(`prior-art/somns/core-lib/Kernel.ns`) and Newspeak's JS kernel
(`prior-art/newspeak/KernelForJS.ns`) do; `nil ifNotNil:` answers `nil`,
`3 ifNil:` answers `3`. `nil printString` answers `'nil'`, as Primordial
Soup's `UndefinedObject` does, where it answered `Object`'s generic
`an Object` before (`goldens/cache/cache_polymorphic_sites`).

**A block for the not-nil case is culled with the receiver** (decided on
2026-09-25, from the corpus): it is handed the receiver when it takes an
argument and nothing when it takes none, Squeak's `cull:`. The prior art
splits here — SOMns sends `ifNotNil:`'s block `value` and culls
`ifNil:ifNotNil:`'s, the JS kernel sends every one `value: self` — and the
canonical Newspeak sources under `corpus/` pass both shapes, 386
`ifNotNil: [:x | ...]` against 126 `ifNotNil: [...]`, which only `cull:`
runs. `Block>>cull:` is an ordinary `Kernel.ns` method saying so, and
`Object`'s bodies are `^notNilBlock cull: self`. A block that takes two
arguments is handed one and fails the arity check as `value:` would; an
argument that is no block is sent `cull:`, which nothing but a block
answers.

Every one of the twelve bodies is a quick method: a constant, `^self`,
`^block value` or `^block cull: self` (`MethodFastPath::CullParameter`,
which activates a closure argument with the receiver or with nothing by its
parameter count, and otherwise activates the method). None of these sends
is inlined by the compiler, so no block they activate is a control-flow
step: both lowerings send them and count the block alike.
`goldens/language/the_nil_protocol`,
`goldens/errors/if_not_nil_culls_its_argument`,
`goldens/errors/if_not_nil_with_a_two_argument_block_is_a_wrong_arity_error`
and `goldens/spec/spec_5_1_3_nil_answers_is_nil` pin it, and
`goldens/kernel/nil_is_the_undefined_object` the class's name.

### The bootstrap: placeholders first, kernel second

SOMns's procedure transferred directly
(`src/som/vm/ObjectSystem.java` pulls each nested mixin definition out of the
compiled kernel module by name and initializes a pre-existing class object
with it). `Interpreter::bootstrap_with` runs it in six steps
(`interpreter/kernel.rs`, `install_kernel`):

1. **Placeholders exist.** `World::new` builds a class object for every
   `BuiltinClass`, each over a placeholder mixin with an empty method table,
   because the runtime must be able to answer what class an integer literal
   has at any moment. `BuiltinClass::ALL` and `Mutator::builtin_class` are
   that table, and `trace.rs` reaches it. A new builtin is appended to
   `BuiltinClass::ALL`, never inserted, because its order is also the wire tag
   order (`image::encode::builtin_class_tag`). `VmMirror`'s class, whose mixin
   already holds the `MIRROR` rows, and its one instance are built here too.
2. **`Kernel.ns` is compiled but not instantiated.** Its root mixin and its
   nested mixins are built with `Mutator::build_mixin`, compiled under the
   interpreter's lowering like any module — the root mixin with
   `Mutator::build_root_mixin`, as every module's root is built, so the root
   rule applies to `Kernel` too — and `check_kernel_shape` checks
   what the phase relies on: one nested class per builtin, named as
   `kernel_class_name` says, each with no slots but the exception classes
   and `Message` ("Exceptions" below) and with the default factory; the
   one immutable `vmMirror` slot on the root; a superclass clause on every
   class but `Top` that is a bare sibling name, read and never run, naming a
   kernel class, from which the chain reaches `Top` without repeating; that
   only `Object` names `Top`, as §3.2 allows; and the literal-block rule
   above. A kernel that breaks it is a defect in the binary, and panics.
3. **Each placeholder gets its kernel application**, in `fill_order`, which
   sorts the kinds by how many superclass links separate them from `Top`, so
   a superclass is always filled before its subclasses. `BuiltinClass::ALL`
   is no longer that order and cannot become it again: a class added later is
   appended, because its position is a wire tag, so `Value` and `Number` sit
   at the end of `ALL` and in the middle of the hierarchy. Each class gets
   the nested mixin named like it, applied to the application of the class
   its clause names — `Top`'s to none, `Object`'s to `Top`'s, and so on down
   — installed with `Heap::set_class_application`, which restamps the
   dispatch id of the few objects that exist this early, and the class
   object's superclass link set to match. `Value`'s *mixin* is recorded just
   before `Value` is filled, and `Behavior`'s just before `Behavior` is, rather than its application after the loop:
   `apply_mixin` asks whether the mixin it is applying is that one, so that
   `Value`'s own application also carries `inherits_value` (§3.2's "subclass
   of" is inclusive, so `Value new` is a value) and `Behavior`'s carries the
   refusal to instantiate a class object, and every class below each reads
   it off its superclass.
   `VmMirror` keeps its own Rust mixin, re-applied over the filled `Value`
   (`superclass_of_kind`); `fill_order` sorts by depth, so `Value` is filled
   first.
4. **The immediate dispatch-id table is refreshed**, because an immediate's
   ids are read off its class's application, which just changed.
5. **The kernel class is built and instantiated** from the host, with the
   mirror as its factory argument: `Kernel vmMirror: theMirror`. Its root is
   `Object <: Value`, so its `Value` level is built first, over the filled
   `Object`, with `nested::build_applied_levels`, as a user root's is. Before
   the instantiation, every class object built so far — each builtin, the
   `Value` level and the kernel's own class — gets its metaclass over the
   filled `Class` (`Mutator::give_existing_class_objects_metaclasses`): until
   here each carried the builtin `Object` as a provisional class, and from
   here on `new_class_object` builds a metaclass with every class object
   ([object-model.md](object-model.md), "Class sides and metaclasses"). The
   instantiation is an ordinary host run, so the stress modes see it.
6. **The instance is wired in.** Every builtin class's `enclosing` becomes the
   kernel instance and its `declaring_class` the kernel class, so a kernel
   method's lexical send of `vmMirror` reaches the slot; each hidden
   nested-class slot of the kernel instance is filled with its builtin class,
   so the nested-class accessor `Array` inside kernel code answers the builtin
   `Array` rather than building a second class object of the same mixin;
   the kernel's own `Value` level, built before the instance existed, is
   given the kernel instance and class as its lexical context too; and
   the instance is kept as `Mutator::kernel`, a root beside the mirror
   ([memory.md](memory.md), "What the collector traces"). Each of those
   classes' metaclasses is given the same lexical context, since a metaclass
   shares its class's (§6.3 writes a class side inside its class's
   declaration). The phase checks that the slot holds the mirror.

The resolved kernel is parsed and resolved once per thread and kept
(`thread_local!`): the test suite bootstraps an interpreter well over a
hundred times, and the AST and bindings are immutable. Each interpreter still
compiles it, because compiled code is numbered against one heap's inline
caches.

**A builtin mixin carries the factory `new`, with no arguments.** A kernel
class declares no factory, so its mixin has §6's default, and
`decide_instantiation`'s arity check applies to it like any class's: a host's
`Interpreter::instantiate` of a builtin class with factory arguments is
refused ("wrong number of factory arguments"), where a placeholder, which
declared no factory at all, let any count through. `Array new` still builds
its own payload (`instantiate::decide_factory`).

**`Kernel` is a value, and so is the mirror** (decided with the user on
2026-09-23). `Kernel.ns` declares
`class Kernel vmMirror: theMirror = Object <: Value (…)`, the header SOMns
writes, so the kernel instance — the enclosing object of every builtin value
class — meets §3.1.1's enclosing-object condition, and every builtin value
class with it. Its one slot holds the mirror, which a value's slot may hold
because the mirror is a value by the ordinary rule: its class sits below
`Value`, and it has no slots and no `==` of its own, as SOMns asserts
(`vm/ObjectSystem.java`, `assert vmMirror.isValue()`). None of the mirror's
capability guarantees above changes with it.
`interpreter/kernel.rs`'s `the_kernel_and_the_mirror_are_values` pins both.
§3.1.1's conditions exist for passing objects across actor boundaries, which
ns-next has none of yet
([liveness-and-reflection.md](liveness-and-reflection.md), "Value objects
exist; the boundary they cross does not"); they are enforced anyway, because
the specification requires them of every value.

### Where the library source lives, and how modules are found

The library is source the runtime ships, compiled at bootstrap as the Rust
bootstrap used to be built, so running a program from source is unchanged:
the bootstrap gained a phase in front of it and nothing above that changed.
A program of several *user* modules does not go through this table: the
host compiles a project's modules in module order and hands them to the
application in its manifest ("The manifest" below).

**The table of shipped modules.** `Kernel.ns` is compiled into the binary
by `interpreter/kernel.rs`; every other shipped module is an entry of
`interpreter/core_library.rs`'s `SHIPPED_MODULES`, a name and its source,
embedded with `include_str!` from `crates/ns-runtime/core-library/` like
the kernel. Today it has two entries, `PlatformFactory` and
`CollectionsFactory`. The table parses and resolves an entry once per
thread, on the first request for it, with `resolve_kernel`, because the
entry is shipped source: the boot asks for `PlatformFactory` alone, so a
program that never reaches `platform collections` never pays for
`CollectionsFactory.ns`. It panics for an entry that does not parse or
resolve, as the kernel's does: that is a defect in the binary, not in a
program, and since a boot no longer resolves every entry, the unit test
`every_shipped_module_resolves` does, so the suite still catches one.
Every interpreter still compiles each module it asks for itself, since
compiled code is numbered against one heap's inline caches. Every entry is
shipped source; a user module would join the table, marked as such by the
host and resolved with `resolve`, only with the module namespace on the
mirror (below). The mirror's
`moduleNamed:` (below) is the leaf through which shipped Newspeak reaches
an entry (`shipped_module_named`), and the host's boot names
`PlatformFactory` directly (`resolved_shipped_module`).

**Planned: the module namespace on the mirror.** For the modules there are,
the namespace belongs on the mirror (decided with the user on 2026-09-21): a
primitive that looks a module up in a table the CLI filled in advance, rather
than a namespace builtin of its own or an invented factory convention.
Nothing would touch the file system at run time, which is what linking
statically means here. Later the same primitive falls back to the file system
on a miss, so dynamic loading extends one primitive and changes neither
`Kernel.ns` nor anything else on the Newspeak side. Its first half is
built: `moduleNamed:` ("The platform and collections as
core-library modules" below) looks a name up in the table the binary
carries. The table itself is still the host's rather than the mirror's,
and the fallback to the file system comes with user modules.

**Start-up was measured, not assumed: the kernel costs about 0.5 ms** —
`ns-next run` of a module that answers at once, 4.07 ms against 3.56 ms
before the move ([execution.md](execution.md), "What the core library's move
cost"). Adding `Top`, `Value` and `Number` and `Value`'s seven methods to it
did not move that measurably, which is the figure to watch as the library
grows ([execution.md](execution.md), "What `Top`, `Value` and `Number`
cost"). That does not call for embedding the compiled kernel as an image,
which stays the next step if the kernel grows until it does, and the
comparison must then include what `image::load`'s two verification passes
cost, since loading is not obviously cheaper than compiling. If it is
embedded, it is generated at build time into the build script's output
directory and included from there, never checked in: a checked-in image that
nobody regenerated decodes into a different, perfectly valid kernel and runs
it silently, which is the failure mode the wire-tag rule exists to prevent.
Embedding does not conflict with [image-format.md](image-format.md)'s "never
for the builtins", because that decision is about what an artifact arriving
from outside may carry; bytes compiled into the binary are as trustworthy as
the Rust beside them.

### The manifest

`crates/ns-runtime/core-library/Manifest.ns` is what a project's root
receives as `packageUsing:`'s argument: a class `Manifest names:
definitions:` whose `doesNotUnderstand:` answers the module definition whose
name is the message's selector, searching its two arrays linearly, and
passes anything else to `super` -- the lookup Newspeak on Squeak's
`Namespacing.ns` and Primordial Soup's `CompilerApp.ns` make over a map. It
lives beside the shipped modules and is embedded the same way, with
`include_str!`, exported as `ns_runtime::MANIFEST_SOURCE`
(`interpreter/core_library.rs`), but it is **not a shipped module**, and
three things follow from that:

- **It holds no mirror.** The host resolves it with `resolve`, never
  `resolve_kernel`, so no `Binding::MirrorSend` is recorded for it, and it
  is in neither `SHIPPED_MODULES` nor the bootstrap's checks of the mirror
  slots. A program that reaches its class, as `manifest class`, gains
  nothing the manifest's own code does not already have.
- **An image carries it.** It is compiled into the program beside the
  project's modules, as the last module in module order, from the origin
  `ORIGIN_EMBEDDED` `Manifest.ns`, so an application that keeps its manifest
  in a slot is written and loaded like any other object. A shipped module's
  instance is never written ("Platform paths in the image" below); the
  manifest must be, and is.
- **It is compiled as user code**, by the interpreter's own lowering, with
  its code on its mixin like any module's, and it is the host, not the
  bootstrap, that instantiates it, once per assembled application, with an
  `Array` of the modules' names and an `Array` of their class objects.

[image-format.md](image-format.md), "Packaging", has the project file, the
module order and how `run`, `build` and `load` use it.

### Exceptions

**Status: built** (designed with the user on 2026-09-25, built the same
day and the next;
[plan](../superpowers/plans/2026-09-25-2244-ns-next-exceptions.md)). The
platform and collections modules below come next; `List` reports its errors
through these.

**Where the machinery lives.** §8.6: "Because Newspeak provides reflective
access to the activation, exception handling is purely a library issue. The
platform provides a standard library that supports throwing, catching and
resuming exceptions, much as in Smalltalk." The only exceptions the
specification names are §5.5's — `Object`'s default `doesNotUnderstand:`
"causes a `MessageNotUnderstood` exception to be thrown" — and §7's
`cannotReturn`. So exceptions are Newspeak in `Kernel.ns` over a few
activation leaves, as Primordial Soup builds them
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`, `Exception`
and `Activation`), not native in the VM as SOMns maps them onto Truffle and
Java exceptions. That keeps the methods optimizable Newspeak, lets a class
decide which exceptions it handles (`handles:`, which Rust could not ask
without calling back into Newspeak), and makes the activation objects the
foundation the reflection step's `ActivationMirror` and the debugger build
on. They are kernel material by the rule for `Kernel.ns` above: delivering
an error is the VM's business.

**The classes**, all under `Object`, none under `Value`:

- `Exception`, with private slots for its signal activation, its handler
  activation, whether it reached its default action (`reachedDefault`),
  whether a search for its handler is running (`searching`) and the
  selector and receiver a VM error arose from (`originSelector`,
  `originReceiver`), and a public `messageText`. Instance side: `signal`,
  `signal:`, `messageText`, `description` (the `messageText`, or else the
  class name), `printString` (the `description`), `isResumable` (`true`),
  `reachedDefaultHandler`, `defaultAction`, `return:` and `return`, `retry`,
  `resume:` and `resume`, `pass`. `return:`, `retry`, `resume:` and `pass`
  sent to an exception no handler is running for signal an `Error`, "…
  sent to an exception that is not being handled".
- `Error` under `Exception`: `isResumable` is `false`, and `resume:` on one
  signals an `Error` of its own, "cannot resume a non-resumable exception",
  as in Squeak.
- `MessageNotUnderstood` under `Error`: `message`, `receiver`, and
  `isResumable` `true`, as in Squeak, so the live system can offer to create
  the missing method and continue (below).
- `Halt` under `Exception`, resumable, signalled by `Object>>halt`, as in
  Squeak rather than SOMns's `vmMirror halt: self` breakpoint primitive: a
  halt and an unhandled error reach the debugger through the same place,
  `defaultAction` on an intact stack.
- `Message`: `selector`, `arguments`, and `sentTo: receiver`, which sends the
  message as an ordinary *public* send (§5.5), as Primordial Soup's
  `Message>>sendTo:` does — a program can obtain and build messages, so a
  send through one must not reach a protected or private method. A message
  built without `arguments:` is sent with none; a selector that is no Symbol
  or arguments that are no Array make `sentTo:` signal an `Error`. It is
  what every `doesNotUnderstand:` handler receives
  ([dispatch.md](dispatch.md), "The reified message is a `Message`").
- `Activation`: private to the kernel, like `Top` (below).

**Class sides are not inherited, so `signal` is declared four times.**
§3.2's metaclasses are flat ([object-model.md](object-model.md), "Class
sides and metaclasses"): a class side is its own mixin applied to `Class`,
not a subclass of its superclass's class side, so a class-side method
declared on `Exception` alone would not reach `Error`. The default
`handles: anException` — "an instance of me or of a subclass", the
`inheritsMixinOf:` leaf — is therefore on `Class`'s instance side, where
every class object answers it and a class side that declares its own
`handles:` overrides it. `signal` and `signal:` (`^self new signal`) are
declared on the class side of each of `Exception`, `Error`,
`MessageNotUnderstood` and `Halt`. A program's own subclass answers
`signal:` only if its own class side declares it; otherwise it is signalled
as `MyError new signal: 'text'`, which the goldens use.

**Programs name them through `platform kernel`** (decided with the user on
2026-09-25), as the corpus does — `private Exception = platform kernel
Exception.` appears 16 times, `platform kernel MessageNotUnderstood` 11 —
and as Primordial Soup's `Kernel wrapping:` facet provides them: the
platform's lazy `kernel` slot holds a value of `PlatformFactory.ns`'s
`KernelNamespace`, whose methods answer `Exception`, `Error`,
`MessageNotUnderstood`, `Halt` and `Message` from the kernel instance the
platform was handed.
`Object` gains no accessor, so its set stays what §3.2 names.

`Block` has `on: exceptionClass do: handler`, `ensure: aBlock` and
`ifCurtailed: aBlock`, three marked methods (below): `on:do:`'s body sets
its `handlerActive` local and answers `self value`; `ensure:`'s runs its
block after `self value` in Newspeak on the normal path, having set its
`complete` local first; `ifCurtailed:`'s answers `self value` and leaves
its block to the unwinder. A block the unwinder runs is activated directly;
an argument that is no block is sent `value` through the kernel's private
`Block>>valueOnUnwind:`, as the normal path sends it.

**The semantics are ANSI's, as Squeak and Primordial Soup implement them.**
`signal` walks the sender chain from its own activation for a marked
`on:do:` activation that is enabled and whose exception class answers
`handles:` true, and runs the handler block **on top of the stack**:
nothing is unwound until the handler decides. The handler block's value
becomes the value of `on:do:`, an implicit `return:`. `return:` unwinds to
the `on:do:` activation and returns the value from it; `retry` unwinds to
it and restarts it at its first instruction, evaluating the protected block
again; `resume:` makes the value the result of `signal`, unwinding what
lies between the handler and the signal; `pass` searches on from the
handler's `on:do:` for the next handler out. A `^` out of a handler block is
a non-local return through `signal` and the handler's activation, and goes
through the same unwinder. An exception signalled in an `ensure:` block
while an unwind is running behaves as any exception does at that point: if
its handler unwinds beyond the running unwind's target, that unwind is
abandoned, and otherwise it continues once the block returns.

**The handler environment is ANSI's and Squeak's: an exception signalled in
a handler is looked for outside the handled block.** While a handler runs,
every `on:do:` the search passed on its way to it — the handler's own, and
every enabled one between it and the signal, the protected block's own —
is disabled, so a second exception is looked for beyond all of them. They
are enabled again under an `ensure:` however the handler is left:
returning, `resume:`, `retry`, `return:`, a `pass` that is resumed, or a
`^` out of it, as Squeak's `Context>>handleSignal:` restores its flag.
Only an activation the search found enabled is collected, so an inner
invocation never re-enables one an enclosing invocation disabled, and a
`pass` continues the first range, so the restores nest in stack order. This
is where ns-next departs from Primordial Soup, whose `handlerActive` flag
disables only the `on:do:` whose handler is running, so that an `on:do:`
inside the protected block could catch an exception its own handler's
handler signals. Squeak reaches the same environment by skipping, during
the search, from a handled exception's `signal` to its handler
(`Context>>findNextHandlerOrSignalingContext`); that would need a marker on
`Exception>>signal`, a leaf to answer the exception from that activation and
a way to read another exception's private slot, which §5.9 allows only
through a lexical send. The cost of the choice made instead: `invokeNextHandler`
walks the senders twice. The first walk only counts the enabled `on:do:`
activations it passes, stopping at the one that handles the exception; the
second walk covers the same range again and fills an `Array new: count`
with them, the handler last, since every handler the first walk passed is
still enabled when the second one runs — `isHandledAt:` disables only the
candidate it tests, and re-enables it in an `ensure:` before the walk moves
on. Invoking the handler then walks that array twice more, to disable and to
enable — the same order as the search itself, so signalling stays linear in
the depth it searches, but one small array allocated per handler invocation,
in place of the growable `OrderedCollection` an in-order walk once needed to
collect while it searched.
`goldens/exceptions/a_handlers_exception_is_looked_for_outside_its_on_do`
and `a_resumed_handler_is_enabled_again` pin it. The candidate `on:do:` is
likewise disabled, under an `ensure:`, while its class's `handles:` runs, so
an exception the test raises — `nil handles:` for `on: nil do:` — is not
offered to that same `on:do:` again, which would nest one search per level
and walk the whole stack each time
(`goldens/exceptions/on_a_non_class_ends_the_run_with_its_handles_error`).

**With no handler**, `signal` sets `reachedDefault` and runs
`defaultAction`: for `Exception`, `Error`, `MessageNotUnderstood` and `Halt`
it ends the run through the `terminate:selector:receiver:description:ifFail:`
leaf. That leaf is total: a `description` that is a String or a Symbol is
the error's text as it is, anything else is rendered as `Heap::describe`
names it, and a selector that is no Symbol is reported as `signal`'s, so no
override can make an unhandled exception carry on and the failure block
never runs. The termination unwinds to the run's floor, running every
pending `ensure:` and `ifCurtailed:` block on the way, as Pharo does when it
terminates a process, and the host receives the error: the CLI prints
"Uncaught Newspeak error: " and the text and exits with status 1. A pending
block's output therefore precedes the error line, which the host prints
once the run has ended; a golden records `stdout` and `stderr` apart, so no
recorded outcome depends on the order. The `HostError`'s selector and
receiver are `Exception`'s protected `hostSelector` and `hostReceiver`:
for an exception the VM raised, the origin it recorded (below), so a host
receives exactly what it received before there were exceptions; for one a
program signalled, `#signal` and the exception. `MessageNotUnderstood`
overrides both with its message's selector and its receiver, and falls
back to `Exception`'s when its `message` is nil — one a program signals
itself (`goldens/exceptions/a_message_not_understood_a_program_signals_ends_the_run`).
`defaultAction` is the one place the debugger will replace; until then a
test stands in for it with `Interpreter::resume_unhandled_exceptions`, a
`#[doc(hidden)]` hook under which the leaf answers nil instead of
terminating, as a debugger that resumed the exception would.

**`doesNotUnderstand:` and creating the missing method.**
`Object>>doesNotUnderstand: message` is Squeak's: it builds a
`MessageNotUnderstood` with the message, the receiver and the text "an
object does not understand #…", signals it, and when the default handler
resumed it (`reachedDefaultHandler`) re-sends the message with
`message sentTo: self`; otherwise it answers the resumption value. The
default handler today ends the program; the live system replaces that one
place with "open the debugger and wait", where creating the method and
proceeding re-sends the message into it — the user's requirement for a live
system, and why `MessageNotUnderstood` is resumable although it is an
`Error`. The re-send is a *public* send, as `sentTo:` always is, so a method
the debugger creates as protected or private would not be reached by it and
the message would fail again; the debugger has to create it public, or
re-send it another way. The text is the one the retired mirror row gave, so
the goldens are unchanged. A program that sends `doesNotUnderstand:` itself
with an argument that is no `Message` fails inside the handler, on
`selector`, as Squeak does.

**The VM side.**

- **Activation objects.** `ObjectData::Activation(FrameReference)`, of the
  private kernel class `Activation`, which no program can name — no
  accessor on `Object`, and its builtin class tag is refused wherever an
  image names a class but the header of an activation record, as `Top`'s is
  everywhere. An activation reads another frame's temporaries and unwinds
  stacks, so it is a capability, and Newspeak hands those out only through
  mirrors, which the reflection step builds on it. The ten leaves
  (`builtins/activation.rs`, rows appended to the end of `MIRROR`): `currentActivation` — the activation of the kernel method
  making the mirror send, reified, answered as a fresh `Activation` object
  each time, so the kernel compares two only when both came from one call
  (`handlerActivation` still the `signalActivation` it was set to) -- two
  calls answer two objects for one frame, so the coming `ActivationMirror`
  needs an identity by frame instead; an
  activation's `sender` — the stack entry below it, or nil
  at the bottom or once it has returned; the three the `on:do:` marker's
  registers answer, `isActiveHandler` (false for a returned activation or
  one with no marker), `handlerClass` and `handlerBlock`, and
  `handlerActive:`, which writes the flag; `returnValue:` and `restart`,
  which answer the signals the unwinder carries out (a returned target is
  §7.2's "cannotReturn: the activation has already returned");
  `inheritsMixinOf:` for `Class>>handles:`; and `terminate…` for
  `defaultAction`. `Message>>sentTo:`'s row, `message:sendTo:ifFail:`, is
  the last row (`builtins/message.rs`), and the row `Object>>doesNotUnderstand:`
  used to forward to retired with the Rust handler it ran, which moved the
  rows after it down by one. The tracer follows the frame reference, so the stress
  modes cover it.
- **Markers.** Whether a method is `on:do:`, `ensure:` or `ifCurtailed:` is
  a property of its compiled code (`bytecode::UnwindMarker`, naming the
  registers of the method's arguments and of its `handlerActive` or
  `complete` local), recognized by selector when the kernel's `Block` is
  compiled (`compiler::unwind_marker`), as the quick methods are; only
  kernel methods carry one, it is never written to an image, and a decoded
  body has none. The unwinder and the leaves read it off each activation's
  code afresh, never from an inline cache.
- **One unwinder**, `interpreter::unwind`, for every cause: non-local
  return, `returnValue:`, `restart` and termination. It pops from the top
  towards its target; at an `ensure:` activation whose block has not run —
  or an `ifCurtailed:` one, which runs only on this path — it marks the
  block run, starts it as a trampoline step and continues once it returns.
  That is the run loop scheduling steps, not a leaf calling back. Popped
  activations' continuations go to `Mutator::abandon`. A normal return needs
  no unwinder and costs nothing; an unwind pays one marker test per popped
  activation ([execution.md](execution.md), "Unwinding"). A Primordial
  Soup-style unwind in Newspeak was weighed and rejected: the non-local
  return would still need a path of its own in the VM (Squeak's
  `aboutToReturn:through:`), two mechanisms doing one thing, and unwinding
  would cost several sends per frame on exception-heavy code.
- **An unwind target is a `FrameReference`, never an `ActivationReference`.**
  Once code runs while an unwind is in progress, a pushed activation can
  reuse a target's stack index, and a release build does not check an
  activation's generation. A reified frame keeps its identity; whether it
  is still live is `Heap::frame_activation`, in every build.
  `Signal::Return` and `Signal::Restart` carry the target's
  `FrameReference`.
- **Every VM error becomes a signalled `Error`.** Where a step answers
  `Signal::Error(NewspeakError)`, the unwinder (`start_signalling`) creates
  an `Error` whose `messageText` is the error's text and whose
  `originSelector` and `originReceiver` are its selector and receiver
  (`Mutator::signal_vm_error`, filling the slots by name as the platform's
  are filled), and pushes `signal` on it above the activation where the
  error arose; nothing is popped. So `on: Error do:` catches an index error
  from `Array>>at:`, a wrong block arity, an instantiation refusal or a
  `cannotReturn`, and an uncaught one ends the host's run with the same
  `HostError` and the same line as before. An `Error` cannot be resumed, so
  that `signal` returns only if the debugger resumes its default action;
  its continuation, `Continuation::VmErrorSignalled`, then ends the run
  with the error. Finer classes (`ArgumentError`, `CannotReturn`, …) come
  when something wants to catch one alone. Until the kernel is installed,
  errors stay host errors, popped to the floor without running any block,
  as there is no `Error` class yet.
- **The depth limit keeps a reserve** of 1,000 activations
  (`DEPTH_RESERVE`), as Python's `RecursionError` and the JVM's yellow zone
  do, because `signal`, the search and the handler are activations too:
  without it an error raised a few activations below the limit could only
  end the run as the limit, uncatchable and under the wrong name. A VM
  error signalled within the reserve's distance of the limit — the limit's
  own error included — takes it, and so does the kernel's
  `Object>>doesNotUnderstand:` activated there, since §5.5 makes a message
  nothing understood the VM's error too; a program's own `signal` does not,
  so one signalled at the limit is reported as the limit, which is itself
  caught. While the reserve is held, counted pushes may go 1,000 past the
  limit. It is held by the frame of the activation the error arose in,
  while that activation runs and directly above it is the error's `signal`
  or the kernel's `doesNotUnderstand:`: a frame, not a stack index, because
  a frame is divorced on its activation's pop and never married again,
  where a later signal at the same index would inherit an index. It is
  given back once that activation above is gone, and released whenever a
  host's run ends. A push past the reserve ends the run at once and
  uncatchably — as a termination, so pending `ensure:` blocks still run.
  If the overrun is the signalling machinery's — the error it holds the
  reserve for is still searching for its handler, after `signal` or a
  `pass`, or running its default action — the run ends with that error, as
  it would have had there been room to signal it; an error the search
  itself raises, a `handles:` nothing understands, belongs to that search,
  so the error reported is the outermost one still searching. An overrun
  inside a handler, or in anything run once a handler was found, ends the
  run with "stack overflow: recursion too deep". Both lowerings keep
  counting alike, and the stack holds at most 129 × (L + `DEPTH_RESERVE`)
  entries ([execution.md](execution.md), "The depth limit").
- **The handler search does not stop at the run's floor.** It follows
  senders to the bottom of the stack, so a handler below the floor of a
  nested run would be a target the unwinder refuses. That is latent while
  runs cannot nest: a host's run is the only one, and nothing re-enters the
  loop ([execution.md](execution.md), "No re-entry from Rust").
- **Actors**, when they come, need no change here: whoever receives the
  signal decides in `defaultAction`, and for an asynchronous message that
  will be breaking the promise (§3.7).

**The image format.** An `Activation` is a record whose body is a frame id
(`OBJECT_ACTIVATION`, body tag 9), validated as every frame reference is.
`Activation`, `Exception`, `Error`, `MessageNotUnderstood`, `Message` and
`Halt` take builtin class tags 23 to 28 (29, `KernelNamespace`'s, is retired:
the class moved into the shipped platform module, now `PlatformFactory.ns`)
([image-format.md](image-format.md), "Verification"). A snapshot in the
middle of an unwind is no more possible than one in the middle of a loop:
the continuations "carry the unwind on" and "end the run with this VM
error" join the list [image-format.md](image-format.md)'s "Planned: suspend
and resume" says that step must encode; the "has this `ensure:` block run"
flag is a local of the frame and travels in its record already.

**Deliberately left out**, to come on demand: `ExceptionSet`,
`Notification` and `Warning`, `outer`, `retryUsing:`, `signalerContext`,
`Object>>error:` and the finer `Error` subclasses. The known gap of a `^`
into an initializer that is still running ("internal: non-local return
escaped the entry send", [execution.md](execution.md), "Unwinding") stays;
the unwind now runs every pending `ensure:` block on its way to the floor,
and the host's outcome is unchanged.

**Tail calls.** An `on:do:` activation is never skipped by a later
tail-call elimination, since its `^self value` is a tail send and `signal`
would no longer find the handler ([execution.md](execution.md), "Planned:
tail calls, skipped but kept").

**Pinned by** the goldens under `goldens/exceptions/` — a handler's value
answered by `on:do:`; `return:`, `retry` (with a counter, so it ends),
`resume:` on an `Exception` and refused on an `Error`, `pass` reaching an
outer handler; an overridden `handles:`; an exception in a handler
searching further out, past the protected block's own `on:do:`, and a
resumed handler enabled again; `ensure:` on a normal end, a `^` from a
block, a `return:` and termination after an uncaught error, `ifCurtailed:`
only on an unwind, and an argument that is no block; an exception in an
`ensure:` block during an unwind; a `^` out of a handler block; an index
error from `Array>>at:` and a `doesNotUnderstand:` caught with
`on: Error do:`, `on: MessageNotUnderstood do: [:e | e resume: 42]`
answering 42, and `sentTo:` a public send; the depth limit caught twice in
a row, an index error and a `doesNotUnderstand:` just below it caught as
themselves, a recursion through `on:do:` counted alike, and a recursive
handler overrunning the reserve; a caught `cannotReturn`; `on: Halt do: [:e
| e resume: 7]` continuing after the `halt`, and an uncaught `halt` ending
the run; and unhandled exceptions whose description is a Symbol or no
string. Rust tests (`crates/ns-runtime/tests/exceptions.rs`) pin the
re-send after the default handler resumed a `MessageNotUnderstood`, a
resumed VM error still ending the run, an uncaught error and an uncaught
`doesNotUnderstand:` just below the limit ending the run as themselves,
and a search and a `pass` that overrun the reserve ending it with the error
being signalled; `interpreter::unwind`'s own tests that an unwind target
survives the reuse of its stack index and that a later signal at the same
index does not inherit the reserve; and `tests/image.rs` the crafted-image
refusals for `OBJECT_ACTIVATION` and tag 23. No golden's recorded outcome
changed; two golden sources that read the old two-element message with
`at:` moved to `Message`'s protocol.

**Measured** 2026-09-26 against `3cbfd31`, the `master` it started from
([execution.md](execution.md), "What exceptions cost"). None of the
`bench` fixtures uses exceptions. The branch first measured slower on
block activation and deep returns -- 4.6% to 6.4% -- from two causes it
located and fixed: a `Signal` grown by 8 bytes once `Terminate` held a
second `NewspeakError` in place, and an arena the collector shrank below
its next limit at every collection. With those and an out-of-line error
in the slot accessor, every fixture and both controls are faster than the
base, by 2.5% (twenty sends deep) to 24.3% (`bench_loop.ns`). Only the `Signal` boxing repairs a cost the branch introduced; the arena shrink and the slot accessor repair defects `master` has too, so being faster than the base is not a property of the exceptions branch, and its net cost against a `master` carrying the same two fixes was not measured -- the per-commit attribution suggests about zero. With
nothing signalled, one `ensure:` and one `on:do:` cost about 145 ns an
iteration over sending `value` to the same blocks; an `Error` signalled
and caught twenty activations up costs about 6.4 µs.

### The platform and collections as core-library modules

**Built in two plans: the first gives lazy slots, the table of shipped
modules and the platform's boot, and platform paths in the image ("The
platform" above describes what exists); the second gives `Array new:
size`, the handler search that keeps no growable collection,
`moduleNamed:` and `CollectionsFactory.ns` with a Newspeak `List` ("The
collections module (plan 2)" below).** `Kernel.ns` is to hold only what the VM
must know — the classes whose instances the VM creates or on which dispatch
depends — and the rest of the library moves into modules the runtime ships.
Primordial Soup splits it this way (`KernelForPrimordialSoup.ns` beside
`Runtime.ns`, whose `Platform` is built from modules such as
`Collections.ns`), and so does SOMns (`core-lib/Platform.ns` loading
`Collections.ns`, `System.ns` and the rest); §8.3 describes the platform as
an object that "provides accessors to various modules".

The two plans:

1. **The platform module**: §6.3.3's lazy slots, the module table,
   `Platform.ns` with `Platform`, `Transcript` and
   `KernelNamespace`, booted with `usingVmMirror:kernel:`, and platform
   paths in the image. `Collections` moves too. Built, and since 2026-09-27
   written in the factory pattern ("The platform" above): the module is
   `PlatformFactory.ns`, whose root takes `usingVmMirror:kernel:` and is
   never handed out, and the platform is its nested `Platform`.
2. **The collections module**: the mirror's `moduleNamed:`, `Array new:`,
   `CollectionsFactory.ns` with a
   Newspeak `List`, a handler search that keeps no growable collection, and
   `OrderedCollection` removed with its leaf and `Heap::array_push`. Built
   (2026-09-28).

#### Lazy slots first (plan 1)

The platform's members become lazy slots (§6.3.3), so that a module is
built on the first `platform collections`, not at every start: once the
platform grows modules for I/O, networking, time and rendering, building
all of them eagerly would make every program pay for all of them. SOMns
builds every member eagerly; this runtime deliberately does not. Lazy slots
are also what the image's platform paths below rest on, and §6.3.3 states
the intent directly: "Lazy slots themselves need never be persisted or
synchronized".

**Built.** The parser keeps the modifier (`SlotDeclaration.lazy`,
[frontend.md](frontend.md)), the slot starts `nil`, and its getter
evaluates the initializer while the slot holds `nil`, storing the result
without a setter and answering it, as §6.3.3 says. An initializer that
answers `nil`, or that does not complete — it signals and a handler
returns, or it returns non-locally — leaves the slot `nil`, so the next
read evaluates it again. `goldens/spec/spec_6_3_3_*` pin first use, the
`nil` answer, the abandoned initializer and one mixin's lazy slot under
two applications at different field offsets;
`goldens/spec/spec_6_a_lazy_slot_cannot_name_a_factory_parameter` pins the
scope rule below, and `goldens/kernel/a_values_lazy_slot_*` the two value
rules. An image carries a lazy slot, filled or empty: the mixin table's
slot record says the slot is lazy, and its getter is one of the row's
compiled methods ([image-format.md](image-format.md), "Mixin table").

**A value's state-derived `=` and `hash` skip lazy slots** (decided with
the user on 2026-09-26): two values whose ordinary slots are equal are
equal, whether or not either has filled a lazy slot yet. A lazy slot is
state derived from the rest — Bracha's "transient fields ... not properly
part of the data's natural representation", purged on synchronization and
recomputed on demand (*Objects as Software Services*, §2.3, the [Bra] of
§6.3.3) — and Primordial Soup's `Serialization.ns` skips its transient
slots for the same reason. No prior art defines a value's equality over
one: canonical Newspeak derives no equality for values, and SOMns, which
does, has no lazy slots. `mixin::declared_slots_of` leaves a lazy slot
out of `MixinApplication::declared_slots`, which both equality walks and
the fallback's `slotCount`/`slotAt:` leaves read. A lazy slot of a value
is still one of its slots for §3.1.1's deep immutability: its getter's
`InitializeSlot` refuses a non-value exactly as an initializer's does,
naming the slot, under the getter's selector.

**A lazy slot's initializer does not see the factory's parameters.** §6:
the primary factory's formal parameters "are in scope in the instance
initializer but not in the rest of the class body", and a lazy slot is
declared in the class body. A class that needs one in a lazy slot copies
it into an ordinary slot first, as `Kernel.ns` already does with
`theMirror`. The resolver resolves a lazy initializer in a fresh
zero-local scope over the class chain, as a unary method's body, so a
parameter's name there is an implicit-receiver send.

**The mechanism** is canonical Newspeak's and Primordial Soup's —
both desugar `lazy n = e` into a hidden cache slot and a getter that
evaluates `e` when the cache holds `nil` (`NewspeakCompilation.ns`,
`astForLazySlotGetter:`; `Intermediates.ns`,
`IntermediateTransientSlotDeclaration`) — with the slot itself as the
cache: the getter is a compiled method whose first instruction,
`ReturnSlotIfFilled` (wire tag 34), returns the field when it holds
anything but `nil`, and which otherwise evaluates `e`, stores it with the
existing `InitializeSlot` and answers it.
`interpreter::instantiate::build_methods` compiles one getter per lazy
slot (`compiler::compile_lazy_slot_getter`, under both lowerings) into the
mixin's method table, where `mixin::slot_accessors` puts no accessor
getter for it; a mutable lazy slot keeps its ordinary setter. The
instance initializer skips lazy slots. Both instructions address the
field at the activation's slot base plus the slot's `own_index`, the base
being the own `own_slot_base` of the activation's `current_class`
(`Heap::activation_slot_base`) -- for the getter its defining class, for
an initializer the level it initializes -- so no activation carries a
base of its own. Every read of a lazy slot therefore activates its
getter, filled or not. A `MethodFastPath::LazySlot` answered a filled one
without an activation, but its arm, inlined into every send site of
`vm::run`, slowed every method send; the getter gave it up
([execution.md](execution.md), "What the platform module cost").

#### The module table and the boot (plan 1)

**Built**, as "The platform" and "Where the library source lives, and how
modules are found" above describe, with one part of the design below not
built: the table is `interpreter/core_library.rs`'s `SHIPPED_MODULES`,
read by the host and by `moduleNamed:`, not yet a table on the mirror, and
its entries carry no shipped flag yet, since every entry is shipped
source; the flag comes with the first user module the table holds. The
mirror's `moduleNamed:` came with `CollectionsFactory.ns` in plan 2, and
the table has two entries. The design as decided:

- **One module table, on the mirror** (the decision of 2026-09-21 in
  "Where the library source lives, and how modules are found"). The host
  fills a table of name to source at bootstrap; the shipped modules —
  `PlatformFactory.ns` and `CollectionsFactory.ns` — sit beside `Kernel.ns`
  in `crates/ns-runtime/core-library/` and are embedded the same way.
  **Loading is unified; trust and visibility are not:** every entry carries
  whether it is shipped core-library source, set by the host and never by
  the module, and only such an entry is resolved with `resolve_kernel`.
  The mirror leaf `moduleNamed:` (the row `moduleNamed:ifFail:`, whose
  subject is the name, `builtins/module.rs`) answers an entry's root class,
  built by `Mutator::load_root_class` on every request, and declines a
  subject that is no `Symbol` or names no entry. It keeps nothing: the
  caller keeps the class it wants, `PlatformFactory`'s root the one
  instance of it in its lazy `collectionsModule`, which Newspeak evaluates
  once. The design had the leaf keep the class, built on its first
  request; decided with the user on 2026-09-28, a lazy slot keeps it
  instead, which needs no new root in `Mutator` and no tracing. Compiling
  is total, so the leaf runs no Newspeak. The same table later holds user modules and falls back to the
  file system. The loader a program is later handed through the platform
  sees user modules only: a program reaches the standard modules as the
  instances the platform built (`platform collections`), never as module
  definitions, whose code was compiled for a trusted context.
- **The boot.** After `install_kernel` the host takes `Platform` from the
  table and instantiates it with `usingVmMirror: mirror kernel: kernel` — an
  ordinary run, as the kernel's instantiation is, so the stress modes see
  it — and keeps the result as a root. Since 2026-09-27 it takes
  `PlatformFactory`, instantiates that, and keeps what its `newPlatform`
  answers instead ("The platform" above). `Mutator::platform_value` reads that
  root instead of building the platform in Rust. The second parameter is
  Primordial Soup's (`Platform internalKernel: ik`), chosen over a mirror
  leaf answering the kernel (SOMns's `vmMirror kernelObject: nil`): handing
  over an object needs no Rust, and the dependency stands in the factory's
  signature. `public` gives no module a way to name the kernel's classes
  (§8.3: a module sees only what is lexically around it, what it inherits
  and what its factory is handed), which is why the platform module needs
  the kernel instance handed to it at all.
- **`Platform.ns`** in plan 1, as first built (since 2026-09-27 the root
  is `PlatformFactory`, never handed out, and this `Platform` is a nested
  class of it; "The platform" above has the current source):

  ```newspeak
  class Platform usingVmMirror: theMirror kernel: theKernel = Object <: Value (
      | private vmMirror = theMirror. private kernelModule = theKernel. |
  ) (
      public class Transcript = Value new () ( show: and newLine, through vmMirror )
      public class Collections = Value new () ( public List = ( ^OrderedCollection ) )
      public class KernelNamespace = Value new () ( Exception, ..., each through kernelModule )
      lazy public transcript = Transcript new.
      lazy public collections = Collections new.
      lazy public kernel = KernelNamespace new.
  )
  ```

  The factory's parameters are copied into private slots, since neither a
  lazy slot nor a method may name them (§6), and the three nested classes
  reach the mirror and the kernel through those slots, so they keep `new`
  and declare no slots: `t class new` still makes a transcript equal to
  `t`. In plan 1 `Collections>>List` answers the kernel's
  `OrderedCollection` through `Object`'s accessor.
- **The transcript leaf takes no receiver.** It checked
  `BuiltinClass::Transcript` so that a user subclass of a builtin declines;
  it reads nothing off the receiver, only the string argument, and once
  `Transcript` is no builtin the VM cannot recognize its class at all. The
  capability is the mirror: the row `transcript:show:ifFail:` keeps its
  shape, since every row takes its subject first, and its leaf checks only
  that `text` is a string. A user subclass of `Transcript` inherits a
  working `show:`, which no golden pinned the other way.
- **Retired:** `BuiltinClass::Platform`, `Transcript`, `Collections` and
  `KernelNamespace`, with builtin class tags 13, 18, 19 and 29.
- **One golden changed:** `goldens/kernel/the_platform_and_its_parts_are_values`
  asked `platform = platform class new`, which `Platform`'s
  `usingVmMirror:kernel:` factory no longer answered; that line was
  replaced, and the other checks kept their recorded output. The factory
  pattern gave the platform's class `new` back, and the golden its
  original checks.
- **`moduleNamed:` came with plan 2.** The host still takes
  `PlatformFactory` from the table itself; `PlatformFactory.ns` fetches
  `CollectionsFactory.ns` through the leaf.

#### Platform paths in the image (plan 1)

**Built** (`image/platform_paths.rs`; [image-format.md](image-format.md),
"Verification"). The design as decided:

An artifact names a shipped object by its **path of public members from
the platform**: the empty path is the platform, `collections` its
collections facet, `collections List` and `collections Collections` that
facet's two classes, `transcript` the transcript, `Transcript` its class,
`Platform` the platform's own. A step is a public slot's getter, a public
nested class's accessor, or a public **class accessor**: a method the
compiler recognized, in shipped source other than the kernel, as exactly
`^outer <Root> <Nested>` naming a nested class of its module's root
(`MixinMethod::class_accessor`, which only the image code reads -- no fast
path, no per-send check). The platform hands out its module's classes that
way, since they are nested classes of `PlatformFactory`, not of the
platform. This replaces the decision of
2026-09-25 to name a class by its module's name and its path in the module,
because two things stand in the way of that on load: nothing in Rust knows
the module instances, which the platform's Newspeak builds; and a nested
class object exists only once its accessor has run, which evaluates its
superclass clause — Newspeak the decoder must never run.

- **The encoder** writes, in a section of its own, the platform paths the
  graph uses, and a record tag (the next free number) that names one of
  them. It writes an object or class it reached by a path as that record,
  and refuses a module instance or a class object compiled from shipped
  source that no path reaches: a module instance holds the kernel, as the
  platform holds the
  mirror, so neither is ever written field by field. `HASHED_PLATFORM`
  stays, for the empty path. What needs a path is a **module instance** or
  a **class object** compiled from shipped source; any other instance of
  a shipped class — a `List`, a second transcript — is an ordinary
  record (below). `CollectionsFactory`'s root instance and its class object
  are exactly such a module instance and such a class object: the facet's
  class accessors name `CollectionsFactory`'s private nested classes, never
  the root that built the facet, so no path names either the root instance
  or its class, and an image that reaches them -- which only a host
  walking `class_enclosing` can -- is refused the same way as the
  kernel's.
- **On load, before the graph is decoded**, the host sends each listed
  path's members to the loading interpreter's platform, each step a public
  slot's, nested class's or class accessor of the shipped instance reached
  so far, sent as an ordinary public send: exactly what the program's own
  `platform collections` may do, and starting a packaged application runs
  its code anyway. Each member must be a symbol, the path length and the
  number of paths are bounded, and a step that is no such accessor, a send
  that fails or an answer that is not a shipped object refuses the image. The decoder then only looks paths up, so it still
  runs no code.
- **A stored platform is a reference, never its state.** A program that
  keeps `platform` in a slot and sends `platform collections` only after a
  load gets the loading platform, whose lazy slot builds the module then;
  a program that kept `platform Transcript` gets the same class through its
  path, since the path's send reaches the same per-instance accessor of
  the same `PlatformFactory` instance. Both routes answer identical
  objects.
- An instance of a shipped class — a `List` with its `data` array, a
  second platform made by `platform class new` — is an ordinary record.
  Its code and layout come from the loading binary, so the decoder checks
  its field count against the class's layout and refuses a mismatch.
- **A module added later needs no tag and no table:** it is nameable by its
  path as soon as the platform declares it. A path the loading binary does
  not have refuses the image, which suits disposable images.
- **Serializing the modules instead** (Primordial Soup's `Serialization.ns`
  writes a fresh runtime into the snapshot) would put code compiled with
  `SendMirror`, which has no wire tag on purpose, into an artifact.
  **Letting the decoder build shipped classes itself**, by restricting
  shipped modules' superclass clauses to bare names as `Kernel.ns`'s are,
  would constrain the library and teach the decoder class construction.
  **Patching placeholders after decoding** would rewrite slots of decoded
  values afterwards. All three were weighed and rejected.

**Open for later:** an object holding external state — an open file or
socket, once I/O exists — cannot be written as data at all, whatever names
its class. That needs its own decision when the first such module arrives.

#### The collections module (plan 2)

- **`Array new: size`**, a leaf answering an array of `size` `nil`s,
  **built**: a class-side forwarder on `Array` (`Kernel.ns`) reaches the
  mirror row `arrayClass:new:ifFail:`, reported selector `new:`
  (`CollectionPrimitive::Allocate`). `Array new` stays, meaning `new: 0`. A
  negative size or a non-`Integer` size is an ordinary `Error`
  ("`new:` requires a size of at least 0, got …" / "… requires an Integer
  size, got …"), and a size that cannot be allocated is one too ("…
  cannot allocate an Array of … elements") — found with
  `Vec::try_reserve_exact` before anything is filled, never a Rust
  allocation abort, since the size is the program's and an abort is not
  catchable. A `List` over a fixed array needs a sized array; this is what
  it grows into.
- **`CollectionsFactory.ns`, built**, in the factory pattern ("The
  platform" above): `class CollectionsFactory usingKernel: theKernel =
  Object <: Value`, a root that holds the kernel in a private slot and is
  never handed out, with the private nested classes a program reaches
  through the platform -- the `Collections` facet and `List` -- as SOMns's
  and Primordial Soup's collections modules are values; it needs no mirror.
  `PlatformFactory`'s root fetches the module (`vmMirror moduleNamed:
  #CollectionsFactory ifFail: [nil]`) and instantiates it with
  `kernelModule`, once, in its lazy `collectionsModule`, so every platform
  shares one module instance and one `List` class; the platform's lazy
  `collections` slot answers that instance's `newCollections`, its one
  public member, and plan 1's `Collections` class has left
  `PlatformFactory.ns`. **How the facet's class is named, settled with the
  user on 2026-09-28:** the facet names its own class with `public
  Collections = ( ^outer CollectionsFactory Collections )`, as
  `Platform>>Platform` does, so its platform path is `collections
  Collections`; the class-accessor recognition (`MixinMethod::
  class_accessor`) is not widened, and `Platform>>Collections` is gone.
  `List` is a private class of the factory, handed out by the facet's class
  accessor `public List = ( ^outer CollectionsFactory List )`, path
  `collections List`; a program may still subclass it
  (`goldens/collections/a_user_subclass_of_list_inherits_its_protocol`).
- **`List`, built,** is Primordial Soup's `List` in this runtime's
  Newspeak: `protected size_`, `protected data`, an `Array` of four to
  start, replaced by one twice as long when it is full -- nothing grows in
  place. Its protocol follows the on-demand rule: `new`, `add:`, `at:`,
  `at:put:`, `do:` and `size`, what the corpus and the goldens ask of
  `OrderedCollection`, plus `at:put:`, which `Array` has. `add:` answers
  the element, and `at:put:` the value, as Smalltalk's and Primordial
  Soup's do, where the kernel's `Array>>at:put:` leaf answers the receiver
  (decided with the user on 2026-09-28). `do:` reads the size once, as
  Primordial Soup's `1 to: size_ do:` does, so an `add:` inside the block
  does not extend the walk while an `at:put:` inside it is seen, and it
  sends `value:` to any argument, as any loop written over `at:` would,
  where the `Array>>do:` leaf treats a non-block as a no-op (decided with
  the user on 2026-09-28;
  `goldens/collections/do_visits_the_elements_a_list_held_when_it_started`).
  An index outside `1..size` -- whatever capacity the array has spare --
  or one that is no `Integer` is an `Error` signalled through
  `kernelModule Error` with `Array>>at:`'s text, "index 3 out of range:
  valid indices are 1..2 (size 2)", so a program sees one error from
  either (`goldens/collections/a_list_index_error_is_caught_as_an_error`).
- **The handler search keeps no `OrderedCollection`, built.** `invokeNextHandler`
  counts the enabled `on:do:` activations it passes, and
  `invokeHandlerAt:disabling:` fills an `Array new: count` by walking the same range
  again, then disables and re-enables them as it does today. A walk that
  kept nothing cannot replace it: once disabled, the handlers this search
  passed look like the ones that were disabled before it. Marking each
  with the exception that disabled it was weighed and rejected: it
  allocates nothing, but an exception signalled again inside its own
  handler makes the marks ambiguous. One small array per handler
  invocation, on the exception path, is the cost. Primordial Soup disables
  only the handler it found; this runtime keeps ANSI's handler environment.
- **Removed:** `OrderedCollection` in `Kernel.ns` and its `Object` accessor
  (§3.2's accessors are for the classes of the literals and the array
  factories), `BuiltinClass::OrderedCollection` with builtin class tag 10,
  the `add:` leaf and `Heap::array_push`. Every `ObjectData::Array` is
  fixed-size once allocated -- nothing grows a payload in place any more
  ([memory.md](memory.md)).
- **Goldens:** those that wrote `OrderedCollection new` moved to
  `platform collections List new` with their recorded outcomes unchanged;
  `goldens/nesting/nested_override` starts its slot at the module's `List`,
  unchanged in output since every ordinary object prints `an Object`;
  `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods` lost
  `MyOrderedCollection`, and one `true an Object` line with it, because
  `List` is no builtin; `the_platform_and_its_parts_are_values`'s
  `c List == OrderedCollection` was replaced by a check that a second
  platform's `List` is the same class. New goldens pin `Array new:`, and
  `goldens/collections/` pins `List`'s protocol, its growth, its index
  errors caught and uncaught, a user subclass of `List`, and one module
  shared by every platform.

#### What changes in the invariants

Done in plan 1: `CLAUDE.md`'s "The VM mirror is a capability" says that
only the shipped core-library modules — `Kernel.ns`, and
`PlatformFactory.ns` from the table — are resolved with `resolve_kernel`,
that no program reaches a shipped module's root (the platform handed out
is an instance of `PlatformFactory`'s nested `Platform`, and the bootstrap
checks the factory's `vmMirror`), and that the platform is never written
field by field: the encoder names it by its path. The on-demand rule
(above) is `PlatformFactory.ns`'s.

Done in plan 2: `CollectionsFactory.ns` joins the shipped modules resolved
with `resolve_kernel` and whose root no program reaches -- its one instance
is kept by `PlatformFactory`'s private lazy `collectionsModule`, and what a
program gets is the facet `platform collections` answers. It declares no
`vmMirror` and needs none. The on-demand rule is its too: every `List`
selector exists because the corpus or a golden asks for it.

#### What is measured

The first `platform collections` now resolves and compiles one more
module, and `List>>add:` is Newspeak where
`OrderedCollection>>add:` was a leaf. Each plan measures, release build and
pinned protocol ([execution.md](execution.md)), the start-up of a module
that answers at once, and the four `goldens/bench/` fixtures; plan 2 also
the start-up of a module that builds one `List`.

**Plan 1, measured 2026-09-27** ([execution.md](execution.md), "What the
platform module cost"): start-up is unchanged, +0.2%, inside the
measurement's own spread — the platform's classes moved out of `Kernel.ns`
rather than being added on top of it, so there is ten lines more for
bootstrap to compile, not a second module's worth. The bench fixtures split:
`bench_loop.ns`, which sends no method in its timed loop, was level;
`bench_block.ns` was 2.6% slower and `bench_send.ns` 4.6%, because
`interpreter/resolve.rs`'s `act_with_base` — inlined into `vm::run` at every
send site — grew one arm for the lazy getter's `MethodFastPath::LazySlot`.
Moving the arm's body out of line did not recover it, nor did taking the
slot base off the activation as well; dropping the fast path did, the
getter answering a filled slot from its own activation
(`ReturnSlotIfFilled`, "The mechanism" above). **Re-measured 2026-09-27
with the fast path gone, all four fixtures are level with `7c6bdbf`**
(-0.7% to +0.4%, every comparison overlapping).

**Plan 2, measured 2026-09-28** ([execution.md](execution.md), "What the
collections module cost"), three builds a side against `6d9974b`: **all
four bench fixtures are level** (`f9d953a`, -0.2% to +0.5%, every
comparison overlapping in both sittings), as they should be, since none
sends a collection message in its timed loop and no send path changed.
**A start that answers at once is level too** (`816367e`, +0.03 ms, +0.2%,
the head's build means among `6d9974b`'s): `interpreter/core_library.rs`
resolves each shipped module on its first request, so a program that never
touches `platform collections` never parses or resolves
`CollectionsFactory.ns`. When the boot still resolved the whole table
(`ee4bdc8`), that start cost 0.11 ms (+0.8%) more, every build of it above
every head build. A start that builds one `List` costs about 0.28 ms
(+2.1%) more than `6d9974b`'s, whose `List` was a builtin class: resolving,
compiling and instantiating `CollectionsFactory.ns`, which the lazy
`collectionsModule` defers to the first `platform collections`, as
intended.

### What this step did not break

The goldens are what made the move verifiable: `Integer>>+`,
`Boolean>>ifTrue:` and the rest moved from Rust to Newspeak, and every golden
stayed byte-identical in `status`, `stdout` and `stderr` under both
lowerings — the reference lowering running uncached as the oracle — the gates
with them: `parse-corpus` at 210/0/1/0, `run-corpus` at ran 6 / failed 0 /
skipped 205, and all three stress modes. The one difference the move makes is
observable only through a program no golden had recorded, below.

### The one observable difference

**A builtin subclass's own `doesNotUnderstand:` is passed by, as §5.10 says**
(decided with the user on 2026-09-22). When a leaf declines on an instance of
a user subclass of a builtin, the kernel method's failure block sends
`super …`, which reaches `Object`; a selector `Object` does not have then ends
in "the `#doesNotUnderstand:` method defined for S" (§5.10) — `Object`'s,
since S is the superclass of the class declaring the `super` send — and not
in a handler the subclass declares. Before the move a declining native passed
its level over and the *receiver's* own lookup continued, which reached that
handler.

It follows from the explicit failure block, and nothing short of the
rejected implicit pass-over could keep the old answer: Newspeak has no send
that continues a public lookup above a given level. No golden had pinned the
old behaviour. `goldens/kernel/a_builtin_subclass_handler_is_passed_by_as_section_5_10_says`
pins the new one: `Handled = Array new`, declaring
`doesNotUnderstand:`, answers `Handled new size` with `Object`'s error, not
with `'handled here'`.

### The ordering, and what it cost

It comes after the
caches, closure conversion and the collector, for the reason below, and it
did not change the image's decision to keep the builtins out of every artifact
([image-format.md](image-format.md), "How upstream compares"): the library is
compiled from source the runtime ships.

**The ordering was the opposite of the obvious one.** A library in Newspeak
costs an activation and a mirror send per leaf where a native cost one send,
and before the inline caches every one of those sends paid a lookup; so the
move came *after* the caches, and after the inlined integer fast path that
was sequenced to cushion it. What Milestone 4 owed this section was only that
it not deepen the investment in Rust callbacks while it rewrote the code
around them; step 2a took the last callback away, as a trampoline step, which
maps directly onto the leaf the kernel's `Block>>value` forwards to.

**Measured, the move cost nothing measurable**, against `e2bb31e`, the
commit before it, over the eight sources in two sittings: with primitive
methods −4.5% to +2.1%, no group entirely above in either sitting. Without
them (`NS_NEXT_NO_PRIMITIVE_METHODS=1`, the same binary) the three sources
that send a leaf the fast path does not absorb in their loop were clearly
slower — `bench_megamorphic.ns` +23.6%, `bench_block.ns` +21.3%,
`bench_do.ns` +5.5% — which is what the primitive methods recover. See
[execution.md](execution.md), "What the core library's move cost", for the
tables and what is left unexplained.

**Giving it the specification's object model cost about as little**,
measured the same way against the commit before it: seven of the eight
sources level, `bench_new2.ns` 3.6% slower and recorded as an open finding
with the change that would recover it
([execution.md](execution.md), "What `Top`, `Value` and `Number` cost").
That measurement also found that `bench_block.ns` spreads 16% between builds
of the same tree, which is a fact about the protocol rather than about this
work.

### The predefined classes are accessors on `Object`

Designed with the user on 2026-09-24 and built the same day
([plan](../superpowers/plans/2026-09-24-2058-ns-next-object-class-accessors.md)).
§3.2 says `Object` "provides a small number of methods available to all
classes", which "also include accessors for predefined classes and objects,
such as the classes of the built-in literals (5.1) e.g., `String`", and the
methods `Array` and `ByteArray`. A bare builtin name is therefore a send.
Each decision below was taken with the user.

**The accessors are exactly the names a program could reach before**:
`Object`, `Value`, `Boolean`, `True`, `False`, `Number`, `Integer`,
`Float`, `String`, `Symbol`, `UndefinedObject`, `Array` and `Block`.
`OrderedCollection` was among them until it was removed (2026-09-28), when
`platform collections List` replaced it. `Top` and the VM mirror's class get none:
§3.2 lets only `Object` name `Top`, as its superclass, and the mirror is a
capability; `Platform` and its members' classes are no kernel classes at
all, and a program is handed the platform through its factory.
Nor do the classes the exceptions added: a program names `Exception`,
`Error`, `MessageNotUnderstood`, `Halt` and `Message` through
`platform kernel`, and `Activation` not at all
("Exceptions" above).
Each is an ordinary public method of `Kernel.ns`'s `Object`,
`public String = ( ^outer Kernel String )`, whose body sends the kernel
instance its nested-class accessor. The `outer Kernel` is required, not
style: inside `Object`, §5.7's innermost declaration of `String` is `Object`
itself, so a bare `^String` would send the accessor to itself forever. This
is wider than both references: SOMns declares only `Object`, `Value` and
`kernel` on its root class ("This works around restrictions by the absense of
a global scope, we use inheritance instead", `prior-art/somns/core-lib/Kernel.ns`)
and has modules fetch the rest from the platform, and Primordial Soup's
`Object` declares none; §3.2's text is the authority, and every name that
worked before keeps working. `interpreter/kernel.rs`'s
`objects_class_accessors_are_exactly_the_nameable_builtins` pins the set,
the three absent names included.

**A bare name is an implicit-receiver send.** The resolver has no builtin
base case: in `resolve_identifier` and in `resolve_superclass_clause`, a name
no local and no enclosing declaration declares binds `LexicalSend { depth: 0 }`
— `self String` — and the receiver's inheritance chain answers it (§2.3.1:
the lexical chain first, then the receiver's chain). A module or a class may
therefore redefine `String` for itself, lexically or by inheritance, as the
specification intends. A nested class's superclass clause `= Object new` is
the same send, run in the clause's activation. A module root's clauses stay
unbound and are decided by the root rule
([object-model.md](object-model.md), "The root rule"), since §3.8 leaves a
top-level expression no receiver. Class-side code reaches the accessors
through its class object's ordinary lookup, which searches the class side,
then `Class`, `Behavior` and `Object`. A
name with no binding — a root clause's receiver, never run, and bare
`super` — lowers to the deferred `UnresolvedIdentifier` under both
lowerings, so compiling stays total ([dispatch.md](dispatch.md), "Binding
variants"). A `super`-prefixed clause receiver, `class N = super Foo (...)`,
is a real super send in both lowerings (§6.1); removing the base case made
that necessary, since `super Object` had reached the builtin by the same
fallback.

**The accessor answers without an activation, as a primitive method runs
its leaf without one** ([dispatch.md](dispatch.md), "Primitive methods").
`MixinMethod.fast_path: Option<MethodFastPath>` has two variants,
`Mirror(MirrorPrimitive)` for a primitive method and
`BuiltinClass(BuiltinClass)` for an accessor. `compiler::primitive_method`
recognizes the accessor while compiling shipped source only —
`ResolvedModule.kernel` says which module is shipped, and only
`resolve_kernel` sets it — from a body that is nothing but `^outer Kernel`
and the name of a kernel class other than `Top`, a form only `Kernel.ns`
can write, and `act` reads the variant off
the method on every send, hit or miss, and answers the builtin class. The
inline cache still records only the method it found; it decides nothing, and
a class that redefines the name reaches its own method through the ordinary
lookup. What remains per use is a self send that hits its cache and a load,
which a JIT can fold to a constant under the receiver-class guard. A kernel
method is never carried by an image, so the format needed nothing for it.

**`Instruction::LoadBuiltinClass` is gone, and its wire tag 2 is retired**
into `RETIRED_OPCODES` (`image/opcodes.rs`), never reused: an old artifact
naming it hits an unknown tag instead of decoding into another program
([image-format.md](image-format.md)). The decoder's
`read_program_builtin_class` stays for the class references a record
carries.

**The mirror's capability lock moved with it.** The lock that no program can
name the mirror's class is now that `Object` declares no accessor for it or
for `Top`, pinned by the kernel test above ("The VM mirror is the whole Rust
boundary" above; `CLAUDE.md`'s list of the six locks).

**What pins it.** An outcome could change only where a program shadows a
builtin name by inheritance: the constant won before, the inherited member
wins now, as §2.3.1 says. No existing golden and none of the runnable corpus
modules had that shape, and no recorded outcome moved. New goldens pin a
module that declares its own `String`
(`goldens/nesting/a_module_that_declares_string_uses_its_own.ns`), an
inherited member named like a builtin
(`goldens/nesting/an_inherited_member_named_like_a_builtin_wins.ns`), an
enclosing member named like a builtin as a superclass
(`goldens/nesting/an_enclosing_member_named_like_a_builtin_is_the_superclass.ns`),
a class-side `Array` (`goldens/class-side/a_class_side_method_names_a_builtin.ns`),
`self Top` and the mirror's class name ending in `doesNotUnderstand:`
(`goldens/errors/top_is_not_a_name.ns`,
`goldens/errors/the_mirrors_class_is_not_a_name.ns`), and the super-prefixed
clause (`goldens/nesting/a_super_prefixed_clause_starts_above_the_declaring_class.ns`).
`crates/ns-runtime/tests/object_class_accessors.rs` pins the recognizer and
the answer under both lowerings.

**Measured** against `909bf40` by the protocol with `-C metadata` layouts
([execution.md](execution.md), "What the value conditions cost"), start-up
included ([execution.md](execution.md), "What the class accessors cost"). The
four fixtures under `goldens/bench/` name no builtin in their loops and are
level, and start-up is level. The throwaway `bench_builtin.ns`, whose loop
names `Array` and `OrderedCollection` every iteration, is 1.5% slower, about
1.4 ns per accessor send against the constant load it replaces. `vm::run`
grew from 6,507 to 6,626 instructions and is still one function.

### The pull future

`Kernel.ns`'s private `Future`, after Primordial Soup's `Proxy`, overrides
every public member of `Object` as protected and forwards through
`doesNotUnderstand:`, which forces the future through the mirror row
`futureValue:ifFail:` (`builtins/future.rs`) and sends the message on to its
value with `Message>>sentTo:`; its state is the two slots `computation` and
`state`, and forcing is `interpreter/future.rs`'s, a trampoline step under
`Continuation::Future`. Like `Activation`, it is private, `Object` declares
no accessor for it, and builtin class tag 30 names it only where the image
must (an unresolved future's record). It has no class side of its own: the
instructions `MakeFuture` and `ResolveFuture` make and resolve the futures
of a simultaneous slot clause. Once a program has made a future, the
callers of a leaf look for one among its operands on the leaf's failure
path (`future::after_a_failed_leaf`), and identity and the equality leaves
decline for one where they would answer "no"
(`Primitive::answers_false_to_a_stranger`). The design is
[object-model.md](object-model.md), "Simultaneous slots".

### Planned: real bodies for control flow and `do:`, before the baseline JIT

Would change: `Block`'s `whileTrue:` and `whileFalse:` and the
collections' `do:` become real Newspeak loops instead of forwarders to
today's leaves. The branches are done: `True`'s and `False`'s methods are
already Newspeak ("`True` and `False` are quick methods" above).

**Kernel method bodies forward; they do not reimplement** (decided with the
user on 2026-09-22). The loops and `do:` are forwarders over
the trampoline-step leaves, as mirror primitives, so `do:`'s loop is still
opaque: nothing can inline its block or compile the loop as a unit. Rewriting
them must happen **before the baseline JIT** — they are what the tiers need to
see through — and pays off **only with one**: before a JIT a Newspeak loop is
more sends and activations than the leaf it replaces, and costs. Its place is
therefore as late as it can come: after the execution model for compiled
code, road step 7, and before the baseline JIT, road step 8
([object-model.md](object-model.md), "Planned: the road to a
high-performance VM"; [execution.md](execution.md), "Planned: JIT tiers").

The step owns three semantic decisions, each of which the leaves settle
today and a Newspeak body would have to settle again, visibly. A fourth,
what a conditional does with an argument that is no block, is settled for
the branches: it is sent `value`, and nothing answers it ("`True` and
`False` are quick methods" above); the loops below would follow it.

- **`do:` iterates over a copy.** `Heap::array_elements` clones the payload
  when the loop starts, so an `add:` inside the block does not extend the
  iteration; a Newspeak loop over `size` and `at:` would see it.
- **`do:` with a non-block argument is a no-op** that answers the receiver
  (`builtins::collections`, `do_step`), rather than sending `value:` to it.
- **`whileTrue:` with a non-block body re-runs the condition**: the leaf
  treats a body that is not a block as its own value and loops on the
  condition alone (`builtins::block`).

## Decisions

- **`True` and `False` are Newspeak, over quick methods** (decided with the
  user on 2026-09-24, built the same day): the Boolean half of "Planned:
  real bodies for control flow and `do:`" moved ahead of the rest; the
  kernel compiler runs `^true`, `^false`, `^nil`, `^<parameter>` and
  `^<parameter> value` bodies without an activation, as Squeak's and Cog's
  quick methods, so the depth count still agrees under both lowerings;
  `and:`, `or:`, `&` and `|` answer their argument's value without coercion,
  as every prior-art kernel does; and an argument that is no block is sent
  `value` with no `Object>>value` to answer it, as in Primordial Soup
  ("`True` and `False` are quick methods" above;
  [plan](../superpowers/plans/2026-09-24-2320-ns-next-boolean-quick-methods.md)).

- **Leaf primitives are native Rust; anything that calls back into Newspeak is
  not**, because a Rust callback is a permanent barrier to optimization. This
  narrows the earlier "native Rust rather than Newspeak source" decision rather
  than reversing it: the reasons for that decision — artifact size and the trust
  boundary — apply to leaves, which are almost all of the library.
- **A leaf is bound as an enumerated primitive, not a function pointer**, so
  a later JIT tier can emit it inline. It is a method-table entry of the VM
  mirror's class, `public` like every row.
- **The core library is Newspeak source over a VM mirror**, as SOMns's is, so
  that every builtin method is a real method: after the caches, closure conversion and
  the collector, and before the JIT tiers, which need the library as real
  Newspeak code to optimize.
- **The VM mirror is the whole Rust boundary** (decided with the user on
  2026-09-21): one object holding every leaf, handed to `Kernel.ns`, and
  nothing else crosses into Rust. §8.3.1 requires it — "There is no syntax for
  a primitive call in Newspeak. A primitive call is a message send to the VM
  mirror" — so Primordial Soup's primitive pragma, which this document
  previously named as the target, is the shape the specification rules out,
  and the specification calls the practice behind it a bug. A mirror is also a
  capability, per §8.2: whoever is not handed it cannot call a primitive.
- **The inlined fast path for leaves came before the library moved**, not
  after. It is independent of what the slow path looks like — the compiler
  emits the operation under a type guard and falls back to the ordinary send —
  so it was measured on its own and then cushioned the move.
- **`Kernel` is `= Object <: Value`, and the mirror's class sits below
  `Value`** (decided with the user on 2026-09-23), as in SOMns: §3.1.1's
  enclosing-object condition, enforced strictly, needs the kernel instance to
  be a value for any builtin value class to have value instances, and the
  kernel's one slot holds the mirror ("The bootstrap: placeholders first,
  kernel second" above).
- **Modules are to be linked statically, through a namespace on the mirror**
  (decided with the user on 2026-09-21, not built): the CLI fills a table in
  advance and a mirror primitive looks names up in it, so nothing touches the
  file system at run time. Dynamic loading later extends that one primitive
  rather than changing the Newspeak side.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **The predefined classes are accessors on `Object`** (decided with the
  user on 2026-09-24, built the same day): one public `Kernel.ns` method per
  nameable builtin, `^outer Kernel <Name>`, answered by
  `MethodFastPath::BuiltinClass` without an activation, and a bare builtin
  name is the self send §5.7 makes it, so a class that declares or inherits
  the name reaches its own. `LoadBuiltinClass` and its wire tag 2 are
  retired — "The predefined classes are accessors on `Object`" above.
- **Integers are unbounded, through a `LargeInteger` written here**
  (decided with the user on 2026-09-29, built the same day): §5.1.1
  requires it, and a bignum crate would keep its digits outside the object
  space. Every integer has one representation, `3 class` stays `Integer`,
  and `//` rounds toward negative infinity, as Primordial Soup's does. The
  algorithms are adapted from `num-bigint` and `num-integer` under MIT, with
  their attribution in `THIRD-PARTY-NOTICES.md` and a header per file,
  including the number-theoretic transform and Burnikel-Ziegler division
  (decided with the user the same day); divide-and-conquer radix conversion
  is not taken ("Large integers" above).
- **Strings will store code points in one of two widths**, Latin-1 or 32
  bits (planned; decided with the user on 2026-09-29; [object-model.md](object-model.md),
  "Planned: the object layout").
- **Exceptions** (nine decisions taken with the user on 2026-09-25, built
  2026-09-25 to 2026-09-26; "Exceptions" above;
  [plan](../superpowers/plans/2026-09-25-2244-ns-next-exceptions.md)):
  Newspeak in `Kernel.ns` over activation leaves, as Primordial Soup, not
  native as SOMns; the scope — `Exception`, `Error`, `MessageNotUnderstood`,
  `Halt`, `Message` and the kernel-private `Activation`, ANSI's protocol
  with `on:do:`, `ensure:` and `ifCurtailed:`, and `ExceptionSet`,
  `Notification`, `outer`, `retryUsing:`, `signalerContext`,
  `Object>>error:` and the finer `Error` subclasses left out;
  `doesNotUnderstand:` is Squeak's, re-sending with `sentTo:` when the
  default handler resumed it; every VM error becomes a signalled `Error`
  with today's text; the depth limit keeps a reserve of 1,000; one unwinder
  in `interpreter/`, running `ensure:` blocks as trampoline steps and aiming
  at a `FrameReference`; `Activation` is `ObjectData::Activation`, reachable
  only through mirror leaves; `halt` terminates in the CLI; and programs name
  the classes through `platform kernel`. Settled while building it: class
  sides are not inherited, so `handles:` is on `Class` and `signal` and
  `signal:` on each kernel exception class's class side; every `on:do:` the
  search passed is disabled while a handler runs, ANSI's environment rather
  than Primordial Soup's one flag; and the reserve is taken by any VM error
  and by the kernel's `doesNotUnderstand:` near the limit, not by a
  program's own `signal`, held by the frame the error arose in, and an
  overrun reports the error still searching or, inside a handler, the
  limit.

The move itself implements nine decisions, each taken with the user on the
date given
([plan](../superpowers/plans/2026-09-22-0107-ns-next-kernel-over-vm-mirror.md)):

1. **A mirror primitive's failure path is an explicit `ifFail:` block,
   inlined by the compiler** (2026-09-21) — "The shape of a kernel method"
   above; [compiler.md](compiler.md), "Mirror sends".
2. **`Integer>>/` stays absent**, and the gap stays recorded as something to
   solve (2026-09-21) — "Gap: an integer has no `/`" above.
3. **Every builtin family moves** (2026-09-21), `do:`, `whileTrue:` and
   `value` included.
4. **The arithmetic fast path's `Jump` saving stays a separate step**
   (2026-09-21) — [compiler.md](compiler.md), "Arithmetic inlining".
5. **Primitive methods** (2026-09-22): a kernel method whose whole body
   forwards `self` and its own parameters to one mirror row runs the leaf
   without an activation, recognized only while compiling `Kernel.ns`, under
   both lowerings — [dispatch.md](dispatch.md), "Primitive methods".
6. **The fast path was built early, with a measurement-only switch**
   (2026-09-22), `NS_NEXT_NO_PRIMITIVE_METHODS=1`.
7. **Kernel method bodies forward; they do not reimplement** (2026-09-22) —
   "Planned: real bodies for control flow and `do:`, before the baseline JIT"
   above.
8. **`True` and `False` are subclasses of `Boolean`** (2026-09-22), as both
   references declare them.
9. **A builtin subclass's own `doesNotUnderstand:` is passed by, per §5.10**
   (2026-09-22) — "The one observable difference" above.
