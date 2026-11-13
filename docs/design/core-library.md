# Core library and the platform

> **Status:** built — the core library is built as Newspeak source,
> `Kernel.ns`, over one VM mirror, §3.2's `Behavior`, `Class` and
> `Metaclass` and §8.6's exceptions included, and the platform is a facet
> of the shipped module `PlatformFactory.ns` with lazy members, its
> transcript, its `time`, its `collections`, its `typedArrays` and its
> `mirrors` the facets of the shipped modules `TranscriptFactory.ns`,
> `TimeFactory.ns`, `CollectionsFactory.ns`, `TypedArraysFactory.ns` and
> `MirrorsFactory.ns`, the third with
> Newspeak `List`, `Map`, `IdentityMap` and `Set`, the fourth with
> `TypedArray` and its ten classes, the fifth with the mirrors of road
> step 5a ([liveness-and-reflection.md](liveness-and-reflection.md),
> "Mirrors that read"); and the kernel carries Primordial
> Soup's whole core API ("The kernel completed to Primordial Soup's core
> API"): its names and loops, its foundations -- the error classes, `Proxy`
> and a public `Future` --, its collections and strings, its numbers --
> `Fraction` and a symmetric numeric tower among them ("The numeric
> tower") --, its `ByteArray` and `StringBuilder`, with the typed arrays (a
> module of their own) and `DataView` beside them ("`ByteArray`,
> `StringBuilder` and the views"), and its weak references, `WeakArray`,
> `Ephemeron` and `WeakMap` ("Weak references"). `Closure`'s `whileTrue:`
> and `whileFalse:` and `Array>>do:` are Newspeak loops, for the JIT tiers
> to see through ("Real bodies for control flow and `do:`").
> Every member of `Kernel.ns`, `Manifest.ns`, `PlatformFactory.ns`,
> `TranscriptFactory.ns`, `TimeFactory.ns`, `CollectionsFactory.ns`,
> `TypedArraysFactory.ns` and `MirrorsFactory.ns` -- public, protected and
> private -- is documented in place to the standard of Javadoc, by a doc
> comment of two or more `///` lines directly above it, its tags --
> `@param` for each argument in order, `@answers`, `@signals` for each
> Error class, `@see`, and `@factory` for a class whose header names its
> factory -- shown by the editor as Java's sections
> ([editor-support.md](editor-support.md), "Doc comments"); `CLAUDE.md`,
> "Conventions", states the rule, and
> `crates/ns-runtime/tests/core_library_documentation.rs` holds every
> member to a doc comment, its `@param`s, its `@answers` and its
> `@factory`.
> **Code:** `crates/ns-runtime/core-library/Kernel.ns` (the library),
> `crates/ns-runtime/core-library/PlatformFactory.ns` (the platform),
> `crates/ns-runtime/core-library/platform/TranscriptFactory.ns`,
> `TimeFactory.ns`, `CollectionsFactory.ns`, `TypedArraysFactory.ns` and
> `MirrorsFactory.ns` (the transcript, the clock, the collections, the
> typed arrays and the mirrors modules),
> `crates/ns-runtime/core-library/Manifest.ns` (a project's
> manifest, which is no shipped module: "The manifest"),
> `interpreter/kernel.rs` (the bootstrap's kernel phase),
> `interpreter/core_library.rs` (the table of shipped modules and their
> boot), `interpreter/shipped_world.rs` (every shipped object under its
> name),
> `interpreter/mirror.rs` (how a send to the mirror runs), `builtins/` (the
> leaves; `builtins/primitive.rs`'s `MIRROR` is the mirror's table;
> `builtins/activation.rs` and `builtins/message.rs` the exceptions',
> `builtins/module.rs` `moduleInstanceNamed:`, `shippedModuleClassNamed:`
> and `recordShippedModule:`, `builtins/future.rs` the pull
> future's forcing, `builtins/bytes.rs` `ByteArray`'s and the UTF-8
> conversions', `builtins/elements.rs` the typed arrays' and `DataView`'s
> element access, `builtins/weak.rs` `WeakArray`'s, `garbageCollect` and
> `objectIsValue:`, `builtins/reflection.rs` the mirrors'),
> `interpreter/unwind.rs` (the one unwinder, and a VM error signalled),
> `compiler/primitive_method.rs`, `compiler/unwind_marker.rs`, `platform.rs`.

The core library is Newspeak source the runtime ships:
`crates/ns-runtime/core-library/Kernel.ns`, compiled into the binary
(`include_str!`) and, compiled ahead of time, embedded as the code an
optimized boot decodes ("The embedded shipped code" below); no image ever
carries it. Its methods reach Rust through exactly one object, the VM
mirror, as §8.3.1 requires, and the leaves — the arithmetic, the
comparisons, `size`, `at:put:`, `printString`, `show:` — stay Rust. Keeping
the library out of the image is what lets an image stay small and keeps a
hostile image from replacing a builtin class; it also means a modified
library is a different `ns-next` binary rather than a different image. See
the comparison in [image-format.md](image-format.md).

## What exists

The shipped modules document their members in place: every method, slot
and class of `Kernel.ns`, `PlatformFactory.ns`, `TranscriptFactory.ns`,
`TimeFactory.ns`, `CollectionsFactory.ns`, `TypedArraysFactory.ns` and `MirrorsFactory.ns`
-- public, protected and private -- has a `///` comment directly above it saying
what it does and answers, what each argument must be, which `Error` it
signals and when, and what it guarantees about order, identity, copying and
mutation (`CLAUDE.md`, "Conventions"). A comment that explains a decision
rather than the member stays a separate paragraph of the same `///` block,
a bare `///` line between the two. This document gives the reasons behind
them.

`Kernel.ns` declares one nested class per builtin class but the mirror's,
and beside them the ordinary classes `StringBuilder`, `DataView` and its two
byte orders, and the mixin `IndexedCollection`, marked † below
("`ByteArray`, `StringBuilder` and the views"); `TypedArray` and its ten
typed arrays are no part of the kernel but of the module
`TypedArraysFactory.ns`:

```
Top                     (private; no superclass, declares nothing)
└── Object              class (protected), ==, =, hash, identityHash, printString,
    │                   yourself, doesNotUnderstand:, halt, subclassResponsibility,
    │                   and a protected accessor per nameable builtin (String, ...)
    ├── Value           =, ==, hash, and the two slot-reading leaves
    │   ├── Boolean     ├── True
    │   │               └── False
    │   ├── Number      ├── Integer
    │   │               │   └── LargeInteger
    │   │               ├── Float
    │   │               └── Fraction
    │   ├── String
    │   ├── Symbol
    │   └── UndefinedObject
    ├── Collection      do: (abstract), and the read protocol over it: size,
    │   │               includes:, inject:into:, detect:, reduce:, collect:, ...
    │   ├── (IndexedCollection)  † the level the mixin adds under each of the
    │   │   │                    two below: the sequence protocol
    │   │   └── Array
    │   └── (IndexedCollection)
    │       └── ByteArray        bytes, data at a 32-byte address
    ├── StringBuilder   † a String built from pieces, over a ByteArray of UTF-8
    ├── DataView        † a view of a ByteArray at byte positions
    │   ├── LittleEndianDataView  † (private)
    │   └── BigEndianDataView     † (private)
    ├── Closure         ..., on:do:, ensure:, ifCurtailed: (marked methods)
    ├── Exception       signal, signal:, pass, return:, retry, resume:, ...
    │   ├── Error       isResumable false
    │   │   ├── MessageNotUnderstood  message, receiver
    │   │   ├── ArgumentError         argument
    │   │   ├── CannotReturn          result
    │   │   ├── NonBooleanReceiver    receiver (private)
    │   │   └── CannotInstantiate     behavior
    │   └── Halt
    ├── Message         selector, arguments, sendTo:
    ├── Activation      (private; a running activation, for the kernel only)
    ├── Mixin           (private; one mixin as an object, for the mirrors only)
    ├── Proxy           every public member of Object, as protected
    │   └── FutureImplementation  (private; the runtime's future)
    ├── Future          class side computing:; no instances
    └── Behavior        =, ==, hash (a class object's, derived from its state
        │               when it is a value)
        ├── Class       name, printString, handles:
        └── Metaclass   name, printString, =, ==
```

The mirror's class is the one builtin `Kernel.ns` does not declare; the
bootstrap builds it below `Value`, so the mirror is a value ("The bootstrap:
placeholders first, kernel second" below).

`Collection` is the abstract superclass of `Array`, `ByteArray`, the typed
arrays module's `TypedArray` and the collections module's `List`, `Map` and
`Set`. It
reaches no leaf: a subclass defines
`do:`, and every other member is Newspeak written over it, which a subclass
overrides where it can answer faster; `collect:`, `select:` and `reject:`
also need the protected hook `newForCollectUsingAdd:`, since the kernel
cannot name the collections module's `List` and a collection says itself
what they answer. It is a builtin -- `BuiltinClass::Collection` -- only
because `Array`'s and `ByteArray`'s superclass clauses name it.
`Object` declares no accessor for it; a program reaches it through
`platform kernel Collection`.

`Array` takes a sequence's protocol from the kernel mixin
`IndexedCollection`, which its clause applies, `Collection <:
IndexedCollection` ("The sequence protocol is a mixin" below): `,`,
`asArray`, `collect:`, `select:` and `reject:`, `copyFrom:to:`,
`copyWith:`, `copyWithSize:`, `first`, `last`, `indexOf:`, `isEmpty`,
`keysAndValuesDo:`, `replaceFrom:to:with:startingAt:`, `sort:` and
`sortFrom:to:by:`, written over `at:`, `at:put:`, `size` and the protected
`newOfSize:`; `Array` declares its leaves, overrides `copyFrom:to:`,
`replaceFrom:to:with:startingAt:` and `asArray` with faster ones, and has
on its class side `new:` and `withAll:`. Its two range leaves
share one range check, and one error text, with `String>>copyFrom:to:`; the
replacing leaf reads every source element before it stores any, through the
write barrier, so overlapping ranges of one array copy as if through a
buffer, and a source that is no Array is first copied into one in Newspeak,
so every error text is the leaf's. `sortFrom:to:by:` is a stable bottom-up
merge sort: no recursion, so its depth does not grow with the range, and
two Arrays the size of the range as extra memory, a copy of the range and
a buffer, which trade places each pass.

`Object>>subclassResponsibility`, protected, is what an abstract method's
body sends: it signals an `Error` whose `messageText` names the abstract
method -- the class declaring the method that sent it and its selector --
and the class of the receiver, `'Collection>>do: is abstract: Nothing must
implement it'`. The name is read off the sending activation through the
activation leaf `activationMethodName:ifFail:` ("Exceptions" below), which
only the kernel reaches, so a program sees the `Error` and never the
activation.

`Behavior`, `Class` and `Metaclass` are §3.2's reified class hierarchy: every
class object's class is its own metaclass — its class side applied to
`Class` — and every metaclass's class is `Metaclass`. They carry behaviour
only; a class object's state stays in fields only the runtime reads (its
eight fixed fields, nine for a level a `<:` clause added and eleven for a
body-less declaration's class object, [object-model.md](object-model.md),
"The object layout"), and their
leaves are in `builtins/behavior.rs`: a class's name
(`className:ifFail:`), a metaclass's class (`metaclassThisClass:ifFail:`),
and the four parts of a class object's state `Behavior>>=`'s fallback
compares -- whether two share an application and one body-less
declaration (`behavior:sharesApplicationWith:ifFail:`), the enclosing
object reflection reports (`behaviorEnclosing:ifFail:`), the enclosing
object its level's code reaches (`behaviorLevelEnclosing:ifFail:`, the
last row of the mirror's table, which differs from the first only for a
body-less declaration's class object and its metaclass) and the superclass
(`behaviorSuperclass:ifFail:`). `Object` declares no accessor for any of
the three classes, and nothing below `Behavior` can be instantiated
([object-model.md](object-model.md), "Class sides and metaclasses").

That is §3.2's shape and §3.1.1's, taken literally. §3.2 says "a class is
either the empty class `Top` or the application of a mixin to another class",
and "only the class `Object` may have `Top` as its superclass", so `Top`
exists, declares nothing, and is private, as are `Activation`, `Mixin`,
`NonBooleanReceiver`, `FutureImplementation` and `DataView`'s two byte
orders, classes no program may name — every other class says `public`,
because §6.3.1 makes an undeclared member protected and a protected class
is unreachable from any other module, which is where a program's `Integer`
or `String` has to come from. `IndexedCollection` is public for that reason
too, though no program may name it either: the module that is its other
user, `TypedArraysFactory.ns`, reaches it as `kernel IndexedCollection`,
and no facet hands it out. §3.1.1 says a
value object's "class inherits from class `Value`", so every class whose
instances are values hangs below `Value`, and `Number` sits between it and
`Integer`, `Float` and `Fraction` because §3.1.1 names `Number` among them.

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
and `printString`, beside its seventeen
accessors for the predefined classes, `Object` to `Error` (§5.1.8), each
`^outer Kernel <Name>` ("The predefined classes are accessors on `Object`"
below); `Number`'s `+ - * / // \\ quo: rem: < > <= >= =`, `hash` and
`asFloat`, the tower's rows, which `Fraction` inherits ("The numeric tower"
below); `Integer`'s `+ - * // \\ < > <= >= =`, `bitAnd:`, `bitOr:`,
`bitXor:`, `<<`, `>>`, `gcd:`, `digitAt:`, `digitLength`, `asStringRadix:`,
`negated`, `abs`, `asFloat`, `printString` and `asString`, and on its class
side `parse:radix:`, beside `bitInvert`, which is ordinary Newspeak,
Primordial Soup's `^-1 - self`, and reaches no leaf, as `&`, `|`,
`parse:`, the rounding selectors, `numerator`, `denominator`,
`raisedTo:modulo:` and `reciprocalModulo:` reach none; `LargeInteger`'s
the same leaves and `hash`, but for the five it inherits from `Integer`,
whose leaves take either integer -- `>>`, `gcd:`, `digitAt:`,
`digitLength` and `asStringRadix:` ("Large integers" below); `Float`'s `+ - * / < > <= >= =`, `hash`, the mathematics (`sqrt`,
`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan:`, `exp`, `ln`, `log`,
`raisedTo:`), `floor`, `ceiling`, `rounded`, `truncated`, `negated`, `abs`,
`asFloat`, `isNaN`, `isFinite`, `isInfinite`, `asInteger`, its four texts
and `asString`, and on its class side `parse:` ("A Float's text" below);
`Fraction`'s `numerator` and `denominator`, and on its class side
`numerator:denominator:` and `reducedNumerator:denominator:`; `String`'s `size`, `at:`, `copyFrom:to:`, `,`, `=`, `hash`,
`asSymbol`, `printString`, `indexOf:startingAt:` and
`lastIndexOf:startingAt:`, and on its class side `with:` and `withAll:`;
`Symbol`'s
`asString`, `=`, `hash` and `printString`; `Array`'s `at:put:`, `size`,
`at:`, `copyFrom:to:` and `replaceFrom:to:with:startingAt:`; `ByteArray`'s
`size`, `at:`, `at:put:`, `copyFrom:to:`, `replaceFrom:to:with:startingAt:`,
`,`, `indexOf:startingAt:`, `lastIndexOf:startingAt:`, `startsWith:`,
`endsWith:` and `at:putCodePoint:`, and on its class side `new:`;
`String`'s `asUtf8` and `utf8Size` and its class side's
`fromUtf8:from:to:`; `Closure`'s
`value` to `value:value:value:` and `numArgs`. `PlatformFactory.ns`'s `Transcript` forwards `show:` and
`newLine` the same way, and its `Stopwatch` its clock reading ("The platform"
below). `Closure`'s `whileTrue:` and `whileFalse:` and `Array`'s `do:` are
Newspeak loops ("Real bodies for control flow and `do:`" below), `do:`
reading its size through the `collectionSize:` leaf. `Number`'s `to:do:` and `to:by:do:` are
ordinary Newspeak (below), and so are `Number>>max:` and
`Integer>>timesRepeat:`, Primordial Soup's bodies, and `Number`'s `min:`,
`between:and:`, `negated`, `abs`, `raisedTo:`, its mathematics through
`asFloat` and its answers to `isNaN`, `isFinite` and `isInfinite`. `UndefinedObject` reaches
no leaf either: its nil protocol is quick methods and its `printString` an
ordinary one ("The nil protocol" below). `True` and `False` reach no leaf at
all: their conditionals, `not`, `&`, `and:`, `|` and `or:` are quick methods
("`True` and `False` are quick methods" below), and their `printString` and
`hash` ordinary ones. `Boolean` declares nothing of its own either. `Object`'s
protected `doesNotUnderstand:` and `halt`, the exception classes, `Closure`'s
`on:do:`, `ensure:` and `ifCurtailed:` and `Message` are the exception
machinery, ordinary Newspeak over the activation leaves ("Exceptions" below).

The other exceptions are `hash` on `Integer` (`^self`) and `asString` on
`String` (`^self`), both quick methods ("`True` and `False` are quick
methods" below), and `hash` on `True` and `False` (`^1` and `^0`), ordinary
methods: the value each answers is
already at hand, and a row for it would only repeat what `Kernel.ns` can
say. Every value class
declares the `hash` its own `=` demands — §3.1.1 lets a value be copied
across an actor boundary, so a copy has to hash like its original, and none
of them may inherit `Object`'s identity hash. `String`'s and `Symbol`'s `=`
each require an argument of the receiver's own class: each class hashes by
its own content, so a cross-class equality would break the rule. A number's
`=` is the other way round: it compares exact values across `Integer`,
`LargeInteger`, `Fraction` and `Float`, so `1.0 = 1` is true, and every
number's `hash` is the numeric hash of its exact value, so `1.0 hash` and
`1 hash` agree (`builtins/numbers/tower.rs`, `numeric/numeric_hash.rs`).

**`to:do:` and `to:by:do:` are sent, where Primordial Soup inlines them.**
Both are `Number`'s, Newspeak bodies after Primordial Soup's
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:2107`,
`:2118`): an inclusive bound, a negative step counting down, and `nil` as
the answer, so an `Integer`, a `LargeInteger` and a `Float` receiver share
one body and a range crossing into `LargeInteger` keeps counting
(`goldens/language/control-flow/number_iteration.ns`). They are no leaf, because each
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
(`goldens/language/control-flow/an_integer_counts_to_a_float_bound`).

**`asStringFixed:` rounds as Primordial Soup's double-conversion does.**
`Float>>asStringFixed:` writes a count of fraction digits from 0 to 20, and
raises for any other count, as Primordial Soup's primitive fails and its
method raises
(`goldens/library/number/float/as_string_fixed_refuses_more_than_twenty_digits.ns`).
A tie rounds up on the exact binary value, where Rust's formatter would round
it to even, so `0.125 asStringFixed: 2` is `'0.13'` and
`1.005 asStringFixed: 2`, just below the tie in binary, is `'1.00'`; the sign
is the value's even when the digits round to zero (`-0.04 asStringFixed: 1` is
`'-0.0'`); and outside ±1e21, and for a NaN or an infinity, it answers the
text `printString` does (`fixed` in `numeric/float_text.rs`;
`goldens/library/number/float/as_float_and_as_string_fixed.ns`). The Are We
Fast Yet harness prints its scores with it.

**A `Symbol`'s hash is its text's** (`text::symbol_hash`): the content
hash a String of the same text answers, told apart from it by a constant,
since a Symbol never equals a String. §5.1.6 makes equal symbols one
object, so it agrees with `Symbol>>=` by construction. It is never the
interner's number: a symbol a program makes has none until its text is
interned, and its hash must not change when the canonical table adopts it
([memory.md](memory.md), "Weak references"). The interner computes it
once per text it interns and stores it beside the text, so an interned
symbol's `hash` is one probe of the canonical table and one read, as a
`Map` keyed by symbols needs it;
only a program-made symbol whose text nothing has interned hashes its
text, in place. Being the text's, it is the same in every process and
survives a snapshot.

**Strings are code points in two widths.** A `String` is an object of the
byte-indexed format while every code point fits Latin-1, one byte per code
point, and of the 32-bit-indexed format otherwise, 32 bits per code point,
so `size` and `at:` are one access each, read in place from the object's
words. It is canonical -- a string whose code points all fit Latin-1 is
always narrow -- so `=` compares the words directly and `hash` agrees with
it. Every element is a Unicode scalar value, at most U+10FFFF and never a
surrogate, so UTF-8 stays the encoding at every boundary -- source, the
transcript, the image -- and the conversion to it is total; `CodePoints`
(`crates/ns-runtime/src/code_points.rs`) is the Rust-side form a string is
built from and copied out to. `hash` is the hash of the UTF-8 text, the value
it had when a `String`'s payload was a Rust `String`. The design and the
comparison with the reference engines are [object-model.md](object-model.md),
"The object layout", "Strings have two widths".

Three leaves read the code points, each Primordial Soup's selector over
code points rather than its bytes:

- **`at:`** answers the code point at a 1-based index, an `Integer`, where
  Primordial Soup's answers a UTF-8 byte -- the same for ASCII. Its errors
  are `Array>>at:`'s texts: "index 4 out of range: valid indices are 1..3
  (size 3)", and "at: requires an Integer index, got nil".
- **`copyFrom:to:`** is Primordial Soup's, 1-based and inclusive: the range
  lies within the string, and the empty range is the one that stops just
  before it starts. Anything else is an `Error`, "copyFrom: 3 to: 1 out of
  range: valid ranges lie within 1..3, an empty range ends just before it
  starts (size 3)", or "copyFrom:to: requires an Integer index, got …" --
  `Array>>copyFrom:to:`'s texts, from the one range check both leaves share.
- **`String with:`** answers a one-character string of any code point,
  where Primordial Soup's takes a byte; anything that is not a scalar value
  is an `Error`, "with: requires a Unicode code point, an Integer from 0 to
  1114111 outside 55296 to 57343, got …".

`goldens/library/string/a_string_answers_its_code_points` and
`goldens/library/string/a_string_is_its_code_points` pin them in either width,
`goldens/library/string/a_string_index_error_is_caught` their errors.

A String's elements are its code points, so the rest of its protocol is
Newspeak over `size`, `at:` and `copyFrom:to:` that deals in them: `do:`,
`first` and `last` answer Integers, and `isEmpty`, `copyStringFrom:to:`,
`startsWith:` and `endsWith:` follow. **`stringAt:`** answers a new String
of the one code point at an index -- what a `Character` would be elsewhere,
and no `Character` exists: an element is an Integer, and the String of one
is how a program gets text back out of a position. Three more leaves:

- **`indexOf:startingAt:`** and **`lastIndexOf:startingAt:`**, one function
  in `builtins/text.rs`, search for a String or a Symbol in code points,
  either width against either: the index of the first occurrence starting
  at or after the start, or of the last starting at or before it, or 0. The
  start lies in 1 to `size + 1`, and an empty argument is found at the start
  itself. Anything else is an `ArgumentError`, "indexOf:startingAt: 0 out
  of range: valid starts are 1..12 (size 11)" carrying the start, or
  "indexOf:startingAt: requires a String argument, got 3" carrying the
  argument. `indexOf:` and `lastIndexOf:` start at 1 and at `size + 1`, and
  `startsWith:` and `endsWith:` ask `lastIndexOf:startingAt:`, after
  refusing an argument that is no String in their own words.
- **`String withAll:`** answers a new String of an Array's code points or
  of a String's, in the canonical width; an element that is no code point is
  an `ArgumentError` carrying it, "withAll: requires Unicode code points,
  Integers from 0 to 1114111 outside 55296 to 57343, got …". Its class side
  gathers any other collection into an Array (`Array withAll:`) and asks
  again.

The UTF-8 conversions are `asUtf8`, which answers a new `ByteArray`, and
`String fromUtf8:` and `fromUtf8:from:to:`, which decode one strictly;
`utf8Size` counts the bytes `asUtf8` would answer. Every other method of a
`String` takes no `ByteArray` ("`ByteArray`, `StringBuilder` and the
views", "UTF-8"). Primordial Soup's `out`, which
writes a string to the standard output, is not adopted: a program writes
through the platform's `Transcript`.
`goldens/library/string/a_string_searches_and_iterates_its_code_points` and
`goldens/library/string/a_string_search_error_is_an_argument_error` pin
them.

**`Symbol` is a subclass of `String`**, as in every reference kernel: a
symbol's words are a string's, in either width, and only its class tells it
from one, so a symbol answers `size`, `at:`, `copyFrom:to:` and `,` as a
`String` of its text does, the last two answering a `String`
(`goldens/library/string/a_symbol_is_a_string.ns`). `=` stays class-tight
both ways -- `'foo' = #foo` and `#foo = 'foo'` answer `false` -- since
`Symbol>>=` is identity, §5.1.6 making equal symbols one object, and
`String>>=` answers `false` for a `Symbol` argument; `Symbol` overrides
`hash` with the interner number above. `Mutator::new_symbol` is the only
way to make one: `Symbol new` is refused ("cannot instantiate Symbol: a
Symbol is made only by a literal or by asSymbol",
`goldens/library/string/symbol_new_is_refused.ns`), while a program's own
subclass of `Symbol` instantiates as an ordinary instance
(`goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods.ns`);
and `String new` answers the empty string, of the byte format
(`string_new_answers_an_empty_string.ns`). Both are decided in
`instantiate::allocate_for_factory`, which the factory body's
`AllocateInstance` runs and which reads the class object's builtin field on
every instantiation, beside `Array`'s and the VM mirror's class.

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
`goldens/library/value/a_values_nested_class_is_not_part_of_its_state.ns` pins it,
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
  application; the factory body's allocation (`allocate_for_factory`)
  refuses before it allocates anything,
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
  from `nested::after_clause`), recorded on it (its value-class refusal
  field: the offending object and the level's name, which `Heap::describe`
  renders only when `new` refuses) and refused at instantiation, because §2.3's own example creates a value
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
be a value too: it declares itself one, `class M = Value (…)` as every
shipped module's root here does (decided with the user on 2026-10-13), or
with §6.2's mixin application, `class M = Object <: Value (…)`, as SOMns's
`class Kernel vmMirror: vmMirror = Object <: Value (`
(`prior-art/somns/core-lib/Kernel.ns`) does. A
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
- **`1 = 1.0` and `1.0 = 1` both answer `true`** (decided with the user on
  2026-10-06). `=` and the orderings compare `Integer`, `LargeInteger`,
  `Fraction` and `Float` by exact value in both directions, and every
  number hashes as the numeric hash of that value, so equal numbers hash
  alike -- the one rule a `hash` has. The derived equality agrees: it
  compares the classes of every pair first, except that a pair of two
  numbers is decided by the tower, so a value whose slot holds `3` equals
  and hashes like one whose slot holds `3.0`
  (`goldens/library/number/tower/numbers_hash_as_they_compare.ns`). The
  arithmetic is the tower's too: with no `Float` among the operands it is
  exact -- `7 / 2` is the `Fraction` `(7/2)`, `6 / 3` the `Integer` `2`
  -- and with one the other operand is rounded once to the nearest
  `double` and IEEE 754 computes
  (`goldens/library/number/tower/arithmetic_is_exact_without_a_float.ns`).
The builtin value classes' own immutability rests on there being no mutating
leaf for a `String` or a `Symbol` — their state is a Rust
payload, not slots, so neither check above is about them. And the actor
boundary that gives all of this its purpose does not exist
([liveness-and-reflection.md](liveness-and-reflection.md), "Value objects
exist; the boundary they cross does not").

The leaves are the 270 rows of `builtins/primitive.rs`'s `MIRROR` table, the
last two the MirrorBuilder's, `reflectionMixin:checkEdit:ifFail:` and
`reflectionMixin:installEdit:ifFail:` ([liveness-and-reflection.md](liveness-and-reflection.md),
"A MirrorBuilder and an atomic install"). A
row names the **mirror selector** (`collection:at:put:ifFail:`), the
**reported selector** — the kernel selector every error text of that leaf
names, `at:put:` — and the `Primitive` it runs. Every mirror selector ends
in `ifFail:`; its arguments are the subject (the kernel method's `self`),
the leaf's own arguments, and the failure block.

### The platform

The platform a module is handed (§8.3) is an instance of the nested class
`Platform` of `crates/ns-runtime/core-library/PlatformFactory.ns`, a shipped
core-library module beside `Kernel.ns` (decided with the user on
2026-09-25, designed on 2026-09-26, and given the factory pattern below on
2026-09-27):

```newspeak
class PlatformFactory usingVmMirror: mirror kernel: kernel = Value (
    | private vmMirror = mirror. private kernelModule = kernel.
      private Exception = kernel Exception. ... private Collection = kernel Collection. |
) (
    lazy private collectionsModule = vmMirror moduleInstanceNamed: #CollectionsFactory ifFail: [
        | rootClass = vmMirror shippedModuleClassNamed: #CollectionsFactory ifFail: [nil]. |
        rootClass isNil ifFalse: [
            vmMirror recordShippedModule: (rootClass usingKernel: kernelModule) ifFail: [nil] ] ].
    lazy private transcriptModule = ... #TranscriptFactory ... usingVmMirror: vmMirror ...
    lazy private timeModule = ... #TimeFactory ... usingVmMirror: vmMirror ...
    lazy private typedArraysModule = ... #TypedArraysFactory ... usingVmMirror: vmMirror kernel: kernelModule ...
    lazy private mirrorsModule = ... #MirrorsFactory ... usingVmMirror: vmMirror kernel: kernelModule ...
    private class Platform = Value () (
        lazy public transcript = transcriptModule build.
        lazy public collections = collectionsModule build.
        lazy public kernel = KernelNamespace new.
        lazy public time = timeModule build.
        lazy public typedArrays = typedArraysModule build.
        lazy public mirrors = mirrorsModule build.
    )
    private class KernelNamespace = Value () ( public Exception = ( ^outer PlatformFactory Exception ) ... )
    public build = ( ^Platform new )
) : ()
```

and the transcript, the clock, the collections, the typed arrays and the
mirrors are each a shipped module of its own under
`crates/ns-runtime/core-library/platform/`, written the same way --
`TranscriptFactory.ns` and `TimeFactory.ns` given the mirror, since their
leaves are reached through it (decided with the user on 2026-10-13),
`CollectionsFactory.ns` the kernel's classes (2026-09-28, "The collections
module (plan 2)" below), `TypedArraysFactory.ns` both, since the typed
arrays are Newspeak over the mirror's element leaves and over the kernel's
`Collection`, `IndexedCollection` and `ArgumentError` (2026-10-13), and
`MirrorsFactory.ns` both, since the mirrors read through the mirror's
reflection leaves and signal the kernel's `Error` and `ArgumentError`
(2026-10-17, [liveness-and-reflection.md](liveness-and-reflection.md),
"Mirrors that read"):

```newspeak
class TranscriptFactory usingVmMirror: mirror = Value (
    | private vmMirror = mirror. |
) (
    private class Transcript = Value () ( show: and newLine, through vmMirror )
    public build = ( ^Transcript new )
) : ()

class TimeFactory usingVmMirror: mirror = Value (
    | private vmMirror = mirror. |
) (
    private class Time = Value () ( public Stopwatch = ( ^outer TimeFactory Stopwatch ) )
    private class Stopwatch = Object ( | private cumulativeNanoseconds ::= 0. private startNanoseconds ::= nil. | ) (
        elapsedNanoseconds, start, and the clock reading, through vmMirror )
    public build = ( ^Time new )
) : ()

class CollectionsFactory usingKernel: kernel = Value (
    | private ArgumentError = kernel ArgumentError. private Collection = kernel Collection. |
) (
    private class Collections = Value () (
        public Collection = ( ^outer CollectionsFactory Collection )
        public ConcurrentModification = ( ^outer CollectionsFactory ConcurrentModification )
        public IdentityMap = ( ^outer CollectionsFactory IdentityMap )
        public List = ( ^outer CollectionsFactory List )
        public Map = ( ^outer CollectionsFactory Map )
        public NotFound = ( ^outer CollectionsFactory NotFound )
        public Set = ( ^outer CollectionsFactory Set )
    )
    private class List new: capacity = Collection (
        | protected size_ ::= 0. private firstIndex ::= 1. private data ::= Array new: capacity.
          private modifications ::= 0. |
    ) ( ... ) : ( public new = ( ^self new: 3 ) )
    private class Map new: capacity = Collection ( ... ) ( private lazy hashtable_ ::= rehash. ... ) : ( ... )
    private class IdentityMap new: capacity = Map new: capacity () ( ... ) : ( ... )
    private class Set new: capacity = Collection ( ... ) ( private lazy hashtable_ ::= rehash. ... ) : ( ... )
    private class NotFound = Error ( | public element ::= nil. | ) ( ... ) : ( ... )
    private class ConcurrentModification = Error ( | public collection ::= nil. | ) () : ( ... )
    public build = ( ^Collections new )
) : ()

class TypedArraysFactory usingVmMirror: mirror kernel: kernel = Value (
    | private vmMirror = mirror. private ArgumentError = kernel ArgumentError.
      private Collection = kernel Collection.
      private IndexedCollection = kernel IndexedCollection. |
) (
    private class TypedArrays = Value () (
        public TypedArray = ( ^outer TypedArraysFactory TypedArray )
        public Int8Array = ( ^outer TypedArraysFactory Int8Array ) ... Float64Array
    )
    private class TypedArray on: aByteArray = Collection <: IndexedCollection ( ... ) ( ... ) : ( ... )
    private class Int8Array on: aByteArray = TypedArray on: aByteArray () ( ... ) : ( ... )
    ... Uint8Array to Float64Array, nine more
    public build = ( ^TypedArrays new )
) : ()

class MirrorsFactory usingVmMirror: mirror kernel: kernel = Value (
    | private vmMirror = mirror. private Error = kernel Error.
      private ArgumentError = kernel ArgumentError. |
) (
    private class Mirrors = Value () (
        public ObjectMirror = ( ^outer MirrorsFactory ObjectMirror )
        public ClassMirror = ( ^outer MirrorsFactory ClassMirror )
    )
    private class ObjectMirror reflecting: anObject = Object ( ... ) ( ... )
    private class AbstractClassMirror anchor: anObject classFirst: aBoolean steps: anInteger = Object ( ... ) ( ... )
    private class ClassMirror reflecting: aClass = AbstractClassMirror anchor: aClass classFirst: false steps: 0 ( ... ) ()
    private class NavigatedClassMirror anchor: anObject classFirst: aBoolean steps: anInteger = AbstractClassMirror ... () ()
    ... MixinMirror, ClassDeclarationMirror, MethodMirror, SlotDeclarationMirror, SlotMirror, MirrorGroup, private too
    public build = ( ^Mirrors new )
) : ()
```

A superclass clause that sends only `new` omits it, as §6.1 allows ("If
omitted, *m* defaults to `new`"), and every module root is `= Value`
(decided with the user on 2026-10-13).

**The factory pattern.** Every shipped module is written this way: an
`XFactory` root holds what is privileged -- here the VM mirror, and of the
kernel instance the classes its facets hand out or its classes inherit,
each in a private slot, since neither a lazy slot nor a method may name a
factory parameter (§6) -- and is never handed to anyone. A root keeps what
it uses of an object it is handed, so it holds no more authority than it
uses (decided with the user on 2026-10-13): of the kernel, a root keeps the
classes it needs, not the instance. The mirror is kept whole, since every
leaf is reached through it, and so is the kernel instance in
`PlatformFactory`, which uses the instance itself: it builds
`CollectionsFactory`, `TypedArraysFactory` and `MirrorsFactory` over it on their first
demand ("The module table and the boot" below), so its root imports it
into the private slot `kernelModule` (decided with the user on 2026-10-15:
§6.3's import of a factory parameter, rather than a mirror leaf answering
the kernel), hands it to nothing but those three factories, and sends it
nothing but `garbageCollect`, which it understands ("Weak references"
below). What a
program receives, and may instantiate, are the root's nested classes, its
facets. Each reaches the privileged state lexically, as a
member of the root, never through a slot of its own, so the facets keep
`new` and hold no privileged state in slots of their own -- `Transcript`,
`KernelNamespace`, `Time` and the collections module's `Collections`
declare none, `Stopwatch`'s two slots hold only its own readings, and
`Platform`'s six lazy slots hold only facets, so a second transcript or
platform, which only a host can make, is equal to the first and writes like
it (`tests/platform.rs`, `the_platform_and_its_parts_are_values`,
`a_second_platform_writes_too`). The platform hands out instances of the
facets by lazy slots and none of their classes: a program holds only the
facets it is handed, and since `class` is protected it reaches no class of
theirs ("`class` and the class accessors are protected" below). `Time`
hands out `Stopwatch` by a class accessor, a method whose whole body is
`^outer PlatformFactory Stopwatch`, since a program instantiates its
stopwatches itself.
Because the root is unreachable, no program can instantiate it over forged
arguments, so the pattern needs no runtime refusal beside it: the guarantee
is made by how the shipped classes are arranged, not by a check in the VM
([README.md](README.md), "Principles"). A new shipped module starts from
this shape, not from the prior-art one in which the root is the namespace
handed out.

**The factory's nested classes are private, and its one public member,
`build` in every shipped factory, makes the facet.** A class accessor and a lazy slot's initializer reach a
private class of the root by a lexical send, which may find a private
member (§5.9), so a program still gets every facet and class it is meant
to have; but a public send to the root instance finds nothing
but `build`, `^Platform new`, which the host sends at boot. That is
a second line of defence behind the root instance being unreachable ("No
program reaches a shipped module's root" below): even a holder of the
root could make only another platform from it, never name a class that
reaches the mirror (`tests/platform.rs`,
`the_platform_factory_answers_only_build`,
`the_platform_hands_out_none_of_its_modules_classes`).
`CollectionsFactory.ns` follows the same pattern: its root holds the
kernel's `ArgumentError` and `Collection`, every nested class -- `Collections`, `List`, `Map`, `IdentityMap`,
`Set`, `NotFound` and `ConcurrentModification` -- is private, and its one
public member, `build`, answers the facet `platform collections`
is (`the_collections_factory_answers_only_build`). So does
`TypedArraysFactory.ns`: its root holds the VM mirror and the kernel's
`ArgumentError`, `Collection` and `IndexedCollection`, its eleven classes --
the facet `TypedArrays`, `TypedArray` and the ten typed arrays -- are
private, and `build` answers the facet `platform typedArrays`. A program
reaches `TypedArray` and the ten only through that facet (a class accessor
each, `^outer TypedArraysFactory <Name>`), and subclasses them through it;
`platform kernel` hands out none of them, and `IndexedCollection`, which is
public in `Kernel.ns` because a shipped module applies it, is handed out by
no facet. So does `MirrorsFactory.ns`: its root holds the VM mirror and the
kernel's `Error` and `ArgumentError`, every nested class -- the facet
`Mirrors` and every mirror class -- is private, and `build` answers the
facet `platform mirrors`, which hands out `ObjectMirror` and `ClassMirror`
by a class accessor each (`^outer MirrorsFactory <Name>`), through which a
program subclasses them (`the_mirrors_factory_answers_only_build`,
`the_mirrors_facet_is_a_value`).

**The members are lazy slots** (§6.3.3, "Lazy slots first" below): each is
built on its first use, so a program pays only for the members it asks for,
and the host's boot runs no member's initializer.

**One collections module serves every platform.** `PlatformFactory`'s root
keeps the one `CollectionsFactory` instance in its private lazy slot
`collectionsModule`, whose initializer runs on the first `platform
collections` of any platform: it asks the mirror's `moduleInstanceNamed:`
for the recorded instance, and when there is none yet builds the module
over the kernel instance through the mirror's `shippedModuleClassNamed:`
and `recordShippedModule:` ("The module table and the boot" below);
`Platform`'s `collections` answers that instance's `build`. The transcript,
clock, typed arrays and mirrors modules are reached the same way, through the
private lazy slots `transcriptModule`, `timeModule`, `typedArraysModule` and
`mirrorsModule`. So every platform's facet comes from one factory and
hands out one class of each name, and the facets, values, are all equal
(`tests/platform.rs`, `the_platform_and_its_parts_are_values`,
`the_collections_module_is_built_once_on_first_demand`,
`the_typed_arrays_module_is_built_once_on_first_demand`,
`the_mirrors_module_is_built_once_on_first_demand`). The facet hands
out each of the module's classes by `^outer CollectionsFactory <Name>`, a
class accessor the compiler recognizes in every shipped module but the
kernel, which is what lets an image name these private classes as values
("Shipped names in the image" below), and not its own class, so a program
makes no second facet. `Collection` it answers from the root's slot
`Collection`, the kernel's own class.

**The boot.** `Interpreter::install_shipped_modules`, run by
`bootstrap_with` right after `install_kernel`, records the kernel instance
as the first shipped module's, then builds `PlatformFactory`
(`build_shipped_module`): its root class, instantiated through the ordinary
`Interpreter::instantiate` with `usingVmMirror: mirror kernel: kernel`,
then recorded as its module's one instance (`record_pending`,
`interpreter/shipped_world.rs`), after a check that the factory holds the
mirror in an immutable, eager slot `vmMirror`
(`interpreter::core_library`'s `factory_holds_the_mirror`, as
`interpreter::kernel` checks the kernel's). `complete_shipped_world` then
builds every class the kernel's and `PlatformFactory`'s roots declare and
records each under its name, which is what an image names ("Shipped names
in the image" below). Last, it sends `PlatformFactory`'s instance `build`
as an ordinary host send and keeps the answer as the platform
(`Mutator::platform_singleton`, traced like every root). Each step is a run
entered from the outside, as the kernel's instantiation is, so the stress
modes see it. `Mutator::platform_value` only reads the platform, so asking
for it runs no Newspeak. The boot builds no other module.

**Every other module is built on its first demand**, by Newspeak, in
`PlatformFactory`'s lazy module slot: `moduleInstanceNamed:` declines while
the module is unrecorded, `shippedModuleClassNamed:` answers its root class
-- decoded or compiled once (`Mutator::shipped_root_mixin`) and kept as the
module's pending class -- the slot sends it the module's factory, an
ordinary instantiation (`TypedArraysFactory` and `MirrorsFactory` with `usingVmMirror: vmMirror
kernel: kernelModule`, `TranscriptFactory` and `TimeFactory` with
`usingVmMirror: vmMirror`, `CollectionsFactory` with `usingKernel:
kernelModule`), only when the mirror answered a class: neither the mirror
nor the kernel instance is ever the argument of a send to a receiver that
may not understand it, such as the `nil` a declined lookup leaves, since a
`MessageNotUnderstood` hands its message's arguments to whoever catches it.
`recordShippedModule:` records the factory's answer: it refuses anything
but an instance of a pending root class, runs the same check of the
`vmMirror` slot for a module given the mirror, records the instance and
names the classes its initializer already built
(`Mutator::name_built_shipped_classes`). Every class it builds afterwards
is named as its accessor builds it (`interpreter/nested.rs`'s
`after_clause`). Nothing is recorded before the factory answered, so a
demand an unwind interrupts -- the depth limit reached inside the factory,
the one error a shipped initializer can meet -- records nothing, and the
next demand instantiates the same pending class again
(`goldens/runtime/depth-limit/a_module_first_demanded_at_the_depth_limit_is_built_later.ns`).
The build is arranged in Newspeak rather than added to the run loop because
an instantiation in the run loop has no "afterwards" to hang a record on:
its factory delivers the instance straight to the sender
(`Continuation::Deliver`, as a method's return does), and a hook there would
sit on every factory activation, on a path whose `Step` sizes are measured
to the percent. So a
program pays for the modules it uses and no other
([execution.md](execution.md), "What the precompiled shipped code moved",
has the numbers).

The boot and every build run shipped code -- the superclass clauses of each
shipped class built, the factories' eager slots, which take the kernel's
classes they need (`kernel Collection` and its siblings), and `build` --
whose sends take room in the inline caches, so a freshly bootstrapped
interpreter does not start with an empty cache table
(`tests/inline_caches.rs` pins what the boot leaves). Neither draws an
identity hash, so every hash a run draws is the same under `run` and
`load`, and a shipped initializer that hashes something -- a hashed
collection built eagerly, say -- would change that
(`goldens/library/object/identity_hash.ns`; `interpreter/shipped_world.rs`,
`a_boot_draws_no_identity_hash`,
`building_a_module_on_demand_draws_no_identity_hash`). The kernel instance
is the second parameter, as in Primordial Soup (`Platform internalKernel:
ik`), rather than a mirror leaf answering the kernel (SOMns's `vmMirror
kernelObject: nil`): handing over an object needs no Rust, and the
dependency stands in the factory's signature. `image::load` runs nothing
the image describes: before it decodes the graph it builds, by the binary's
own code, the modules and classes the image's shipped-names section names
that the boot has not built (`Interpreter::build_shipped_names`, as a
program's first demand builds them), and then finds every name by table
lookup.

**The transcript leaf does not check its receiver.** `Transcript`'s `show:`
and `newLine` are mirror forwarders like the kernel's —
`TranscriptFactory.ns` is resolved with `resolve_kernel`, and `vmMirror` is a
member of its root, so their `vmMirror` sends are mirror sends and the two
methods are primitive methods — and their leaf (`platform.rs`) reads
nothing off the receiver, only the string argument. The capability is the
mirror, which only shipped source reaches; `Transcript` is no builtin, so
the VM could not recognize its class anyway. So a second transcript, of a
second platform a host makes, writes like the first (`tests/platform.rs`,
`a_second_platform_writes_too`); no program reaches the class to make one
or a subclass of it.

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
(`goldens/library/platform/the_transcript_prints_any_object`,
`goldens/library/platform/show_of_something_that_is_no_string_is_an_error`).

**The namespace members are ordinary methods.** `KernelNamespace`'s
methods answer the kernel instance's nested classes. The class-accessor fast path
("The predefined classes are accessors on `Object`" below) is recognized
only in the form `^outer Kernel <Name>`, which no module but the kernel can
write, so each costs one activation more than it did as a kernel accessor
(`interpreter/kernel.rs`,
`the_platform_modules_members_have_the_fast_path_they_should`). A
namespace member is asked for once, typically in a slot initializer. The
collections facet's class accessors, and `Time`'s, carry no fast
path either: the compiler recognizes them
(`MixinMethod::class_accessor`) for the image's rule of what may be named
where alone ("Shipped names in the image" below), and
no send reads the mark
(`the_platforms_class_accessors_are_recognized_without_a_fast_path`).

Being values, the platform and its members have no identity: two
transcripts are equal and so identical, two platforms too, and a
transcript kept in an image is the loading platform's after the load
(`goldens/library/platform/the_platform_and_its_parts_are_values`, and
`tests/platform.rs`'s test of that name for the second ones a host makes).
None has an accessor on `Object`, since a program reaches them through the
platform it is given.

**`platform time` is Primordial Soup's `Time` facet, narrowed to its
`Stopwatch`.** Both came in for `benchmarks/are-we-fast-yet/newspeak/AreWeFastYet.ns`,
which measures with them ("The on-demand rule" below). `Time` is a value
like the other members, and its one method, `Stopwatch`, is a class
accessor, so an image names that class by its declaration,
`PlatformFactory Stopwatch` (`tests/image.rs`,
`a_class_the_platform_hands_out_is_named_by_its_declaration`).
`Stopwatch` answers only what the harness sends, `start` and
`elapsedNanoseconds`, with Primordial Soup's meaning
(`prior-art/primordialsoup/newspeak/Time.ns`): an
unstarted stopwatch reads 0, and `start` on a running one changes nothing
(`goldens/library/platform/a_stopwatch_measures_elapsed_time`).

**A `Stopwatch` is an object, not a value.** It keeps its start and its
accumulated time in mutable slots, which §3.1.1 forbids a value, so its
class is `Object new`, as the collections module's `List` is, and it is
the one class of the platform's modules that is not a value. So two
stopwatches are two objects, and an image writes one as an ordinary record
of `PlatformFactory Stopwatch`, readings included
(`a_stopwatch_is_a_record_of_its_shipped_class`). Its slots hold no
privileged state: the clock is reached through the root's `vmMirror`,
lexically, by the private `currentMonotonicNanoseconds`, a mirror forwarder
over the row `clockMonotonicNanoseconds:ifFail:` whose leaf
(`platform.rs`) reads nothing off its receiver and never declines, as the
transcript's does not.

**The clock's origin is per process.** The leaf answers nanoseconds since an
`Instant` taken on the first reading in the process, shared by every
interpreter in it, so two interpreters' stopwatches agree; a `SmallInteger`
holds 36 years of nanoseconds, and a longer reading is a `LargeInteger`, the
`u128` converted whole rather than saturated.
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
answers the root's instance or its class -- the class accessors of its
facets answer nested classes only, and the root instance is only ever the
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
through the one root instance whose `vmMirror` slot was checked -- by the
boot, or by `recordShippedModule:` when it recorded the module.

That the root instance stays unreachable rests on more than naming: it must
never become the answer to anything a program sees, and the one place the
runtime hands a receiver to a program is a `MessageNotUnderstood`, whose
`receiver` slot is public. So no code whose receiver is the factory
instance may ever send it a message it does not understand -- today the
only sends to it are the host's `build` at boot, `build`'s own `Platform`,
the sends `outer PlatformFactory <Name>` of `Time`'s class accessor and
`KernelNamespace`'s members, each naming a nested class or a slot it
declares, the nested classes' superclass clauses, run with it as receiver,
and the lazy module slots' initializers' own, `vmMirror` and
`kernelModule`. `CollectionsFactory`'s instance is held the same way: only
`PlatformFactory`'s private `collectionsModule` slot keeps it, and the
facet's classes have it as their enclosing object; the sends to it are
`build` from `Platform`'s `collections` initializer, the facet's class
accessors' `outer CollectionsFactory <Name>`, and its nested classes'
superclass clauses. Whoever edits either module (or writes another
`XFactory` root) keeps it that way: a send to the root that can miss, or a
method of the root that sends to `self` something it may not understand,
would hand the root, and with it `usingVmMirror:kernel:` through its class,
to any program that catches the error. The same holds for a message's
arguments, which a `MessageNotUnderstood` hands out through its `message`:
neither the mirror nor the kernel instance is ever the argument of a send
to a receiver that may not understand it, which is why a lazy module slot
sends its module's factory only to a class the mirror answered, never to
the `nil` a declined lookup leaves.

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

**An image names the platform's classes by their declarations.** The
platform and its facets are ordinary records of their shipped classes, and
every shipped class they reach is named by its module and declaration
("Shipped names in the image" below), so a second platform is a record of
`PlatformFactory Platform` like the first. A module instance -- the
factory's, holding the mirror and the kernel -- is never written.

## The on-demand rule

**`PlatformFactory.ns` is widened only on demand.** `Kernel.ns` and
`CollectionsFactory.ns` are not bound by it: they carry Primordial Soup's
whole core API ("The kernel completed to Primordial Soup's core API"
below; decided with the user on 2026-10-06). An accessor exists because a module on `corpus/runnable.toml` or
a program under `benchmarks/` needs it, and the whitelist entry or the
program that demanded it is named in a comment beside it: the comments of
`Time` and `Stopwatch` name `benchmarks/are-we-fast-yet/newspeak/AreWeFastYet.ns`.
The kernel facet, `platform kernel`, is outside the rule (decided with the
user on 2026-10-11): it only hands out classes of `Kernel.ns`, which the
kernel completion governs, and reaches no host, so `KernelNamespace>>Collection`
names no program. So is the typed arrays facet, `platform typedArrays`
(decided with the user on 2026-10-13): it only hands out the classes of
`TypedArraysFactory.ns`, which the kernel completion governs likewise, and
reaches no host. So is the mirrors facet, `platform mirrors` (decided with
the user on 2026-10-18): it only hands out the classes of
`MirrorsFactory.ns`, whose protocol road step 5a settled
([liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors that
read"), and reaches no host.

This is not minimalism for its own sake. A platform member reaches the host
-- a file, a clock, the network -- and one nothing on that list or under
`benchmarks/` asks for has no test that would catch it rotting, so it is a
liability rather than an asset. A module that needs something nobody has
asked for is a reason to widen the whitelist deliberately — with a real
module, run end to end, its output recorded — not a reason to build a
speculative platform. The core is different: Primordial Soup's API is the
specification it is completed to, and each sub-project pins what it adds
with goldens of its own.

The `collections` namespace hands out `Collection`, `WeakArray` and
`WeakMap`, the kernel's, and the module's `List`, `Map`, `IdentityMap`, `Set`,
`NotFound` and `ConcurrentModification`, each with Primordial Soup's
protocol ("The collections module (plan 2)" and "Weak references" below).
The corpus also asks that namespace for `IdentitySet`, which Primordial
Soup does not have, and it is not there: the send is not understood,
never quietly answered by `Set`, which compares by `=`, or `IdentityMap`
(`tests/platform.rs`,
`platform_collections_does_not_invent_an_identity_set`).

## Large integers

§5.1.1: "*Note that integers are not limited in size*", and it puts no bound
on the *n* in the digit sum it defines a literal by. So an `Integer` is
unbounded, and **every integer has exactly one representation**: a
`SmallInteger` (an immediate of the tagged word, class `Integer`) when it
fits 61 bits, `SMALL_INTEGER_MIN` to `SMALL_INTEGER_MAX` (-2^60 to
2^60 - 1), a `LargeInteger` object otherwise. 61 bits are what the word
leaves beside its tag ([object-model.md](object-model.md), "Values and
objects"). `Mutator::new_integer` is the only way to make a `Value` from an
`ArbitraryInteger`, and it decides which, so every leaf, which answers
through it, answers an immediate again once a result fits.

**The class.** `LargeInteger` is a kernel class, a subclass of `Integer`,
which stays the class of the immediate integers: §5.1.1 asks only that an
integer "obeys the protocol of class `Integer`", so an immediate's `self
class` keeps answering `Integer`. `Object` declares no accessor for it
(`interpreter/kernel.rs`'s
`objects_class_accessors_are_exactly_the_nameable_builtins`), and `class` is
protected, so no program reaches it: a `LargeInteger` answers
`isKindOfInteger` and the whole protocol of `Integer`, and only the host
tells it from one (`crates/ns-runtime/tests/representation.rs`). An image
names it by its shipped name, `Kernel LargeInteger`. It is a value object (§3.1.1): `==` is
`Value`'s, `^self = other`, and `=` and `hash` come from its sign and
magnitude, never from an identity hash, so a value whose slot holds one
compares and hashes by state too
(`goldens/library/value/a_value_with_a_large_integer_slot_compares_by_state.ns`).
With one representation per integer, no `LargeInteger` is ever the same
number as an `Integer`, so the two are never equal. Its `=` and its
orderings go by exact value against every number, and its `hash` is the
numeric hash of its value, so a `LargeInteger` equals the `Float` of its
exact value and hashes like it ("The numeric tower" below). An instance of a
subclass of it, or `LargeInteger new`, holds no digits; every leaf
declines it on the missing payload, never on the class, and the send falls
to `Object`'s methods (`crates/ns-runtime/tests/representation.rs`,
`a_subclass_of_large_integer_is_no_number`, whose module the host hands
`LargeInteger` as a factory argument, since no program can name it).

**The leaves.** `LargeInteger` forwards `+ - * // \\ < > <= >= =`, `bitAnd:`,
`bitOr:`, `bitXor:`, `<<`, `hash`, `negated`, `abs`, `asFloat`, `printString`
and `asString` to twenty rows of its own, the `largeInteger:` rows of `MIRROR`
(`builtins/numbers/large_integer.rs`, `execute_large_integer`). `Integer`'s leaves keep their
immediate path and promote where it has no answer: a sum, difference or product
that does not fit 61 bits is recomputed in `i128`, where two `SmallInteger`s'
always fits, and `SMALL_INTEGER_MIN // -1`, `SMALL_INTEGER_MIN negated` and
`SMALL_INTEGER_MIN abs` answer `2^60`. The sum, difference, product and
comparisons work on the tagged words ([object-model.md](object-model.md),
"Values and objects"). The inlined `IntegerBinary` never allocates: it declines
a result that does not fit and falls back to the send, whose leaf promotes
([compiler.md](compiler.md), "Arithmetic inlining";
`goldens/library/number/large-integer/an_overflowing_integer_operation_answers_a_large_integer.ns`).
Both classes' arithmetic and ordering accept any number (`tower::operand`), so
`5 + big` and `big + 5` both answer
(`goldens/library/number/large-integer/large_integer_arithmetic.ns`). A
`Float`'s arithmetic converts a `LargeInteger` argument to the nearest `double`
(`ArbitraryInteger::to_f64`, rounded to nearest with ties to even, infinite
beyond `f64::MAX`), as Primordial Soup's `LargeInteger::AsDouble`
(`prior-art/primordialsoup/vm/large_integer.cc:1242`) serves, and as it
converts an `Integer` argument; an integer's `+ - *` convert themselves
and a `Float` argument to doubles the same way, whichever integer class the
receiver is, so the two directions agree
(`goldens/library/number/float/a_float_meets_a_large_integer.ns`). The
orderings and `=` convert nothing: they compare exact values, a finite
double being the dyadic rational it is, so 2^53 + 1 is greater than the
double 2^53 it rounds to
(`goldens/library/number/tower/numbers_compare_by_exact_value.ns`). An argument
that is no number is a type mismatch naming "a numeric argument" (`goldens/library/number/large-integer/a_large_integer_meets_nil.ns`).
`//` and `\\` take any number, a `Float` or a `Fraction` included, and by any
zero, `0.0` included, raise "division by zero" carrying the divisor
(`goldens/library/number/large-integer/large_integer_floor_division_by_zero_is_uncaught.ns`,
`goldens/library/number/integer/floor_modulo_by_zero_is_uncaught.ns`,
`goldens/library/number/tower/arithmetic_is_exact_without_a_float.ns`).

**An index, size or count that is a `LargeInteger` is out of range, not of
the wrong class.** It is still an `Integer`, and no index, size or count
reaches one, so `at:`, `at:put:`, `Array new:`, `Array`'s `copyFrom:to:`
and `replaceFrom:to:with:startingAt:`, `String`'s `at:`, `copyFrom:to:`,
the start of `indexOf:startingAt:` and `lastIndexOf:startingAt:`,
`Value`'s `slotAt:` and `asStringFixed:` refuse it in the
words they use for a `SmallInteger` out of range -- "index
18446744073709551616 out of range: valid indices are 1..3 (size 3)", "new:
cannot allocate an Array of … elements" -- never with a type mismatch
(`goldens/library/collection/an_index_or_size_past_every_small_integer_is_out_of_range.ns`).

**`//` rounds toward negative infinity**, so `7 // -2` and `-7 // 2` are
both `-4`: Primordial Soup's `Integer>>//`, commented "Integer division,
rounded toward negative infinity. The remainder of this division is given by
`\\`" (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1958`),
and `num-integer`'s `div_floor`. `checked_floor_divide`
(`builtins/numbers/mod.rs`) is the one copy of the rule for two `i64`s, which
the leaf and `IntegerBinary` share, and `ArbitraryInteger::floor_divide` the
one for every other pair of integers; a pair with a `Fraction` or a `Float`
in it rounds its quotient the same way ("The numeric tower" below)
(`goldens/library/number/integer/floor_division_rounds_toward_negative_infinity.ns`).

**`\\` is the remainder of that division**, so it takes the divisor's sign:
`7 \\ -2` is `-1` and `-7 \\ 2` is `1`. For two `i64`s, `floor_modulo`
(`builtins/numbers/mod.rs`) corrects Rust's truncating remainder, taken with
`wrapping_rem` so that `i64::MIN \\ -1` answers `0` where `%` would
overflow. For every other pair of integers, `ArbitraryInteger::floor_modulo` derives it
from `floor_divide`, as the dividend less the quotient times the divisor,
rather than adapting `num-integer`'s `mod_floor`, so the quotient and the
remainder can never disagree
(`goldens/library/number/integer/floor_modulo_takes_the_divisors_sign.ns`). `\\` has no
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
`goldens/library/number/integer/bit_and_is_twos_complement.ns`; a sign-and-magnitude
`and` would answer `0` for `-1 bitAnd: 2^64`, which is `2^64`. `bitOr:` and
`bitXor:` are over two's complement the same way: `ArbitraryInteger`'s
`bit_and`, `bit_or` and `bit_xor` are one `bitwise` with a different digit
operation, checked against `i128` and by
`goldens/library/number/integer/bit_or_and_bit_xor_are_twos_complement.ns`, and
`-1 bitXor: 2^64` is `-(2^64) - 1`.

**`<<` shifts the magnitude and keeps the sign**, which is exact for a
negative receiver: `-9223372036854775808 << 1` is `-18446744073709551616`,
and `-1 << 63` is exactly `-2^63`, beyond 61 bits and so a `LargeInteger`
(`goldens/library/number/integer/shift_left_widens_into_a_large_integer.ns`). A negative
count raises an error that says the count must be non-negative. A result
whose digits cannot be reserved raises "cannot allocate" rather than
aborting: `ArbitraryInteger::shift_left` reserves with `try_reserve_exact`,
the probe a large `Array new:` makes too, because the count is the program's ([OPEN-WORK.md](../OPEN-WORK.md)
says what that does not yet cover). `0 << n` answers `0` without
allocating. The selector and its meaning are Primordial Soup's
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1599`).

**`asFloat` answers the nearest double**, ties to even: an `Integer`'s by
Rust's `as`, exact up to 2^53, and a `LargeInteger`'s by
`ArbitraryInteger::to_f64`, the conversion a `Float`'s leaves already apply
to an integer argument. A `Float` prints as its shortest text ("A Float's text"), the
digits that read back as the same double, so `9223372036854775808
asFloat` prints `9223372036854776000.0`
(`goldens/library/number/float/as_float_and_as_string_fixed.ns`).

**The representation.** A `LargeInteger` is an object of the 64-bit-indexed
format: its words are the magnitude's 64-bit digits, least significant
first, inline after the header and the class word, and its sign is the
header's negative bit, as Primordial Soup's `kNegativeBit`
(`prior-art/primordialsoup/vm/object.h:48`); one class, not Spur's two
([object-model.md](object-model.md), "The object layout"). It is canonical
-- no zero top digit, and zero never negative -- so derived equality is
value equality, and it holds no handle, so the tracer never reads its
digits, which could look like handles. `=`, `hash`, the comparisons and the
sign tests read the digits in place (`Heap::with_large_integer`), and
arithmetic copies both operands out into `ArbitraryInteger`s
(`Heap::large_integer`) and allocates its result through
`Mutator::new_integer`; lending the operands to the arithmetic as well is
open work ([OPEN-WORK.md](../OPEN-WORK.md)). That is why the arithmetic is
ns-next's own rather than a crate's: a crate's numbers live in its own `Vec`
in the Rust heap, which is the payload split the object layout removed, and
every operation would copy between the two. The
Smalltalk-family engines write it themselves too -- Primordial Soup's
`vm/large_integer.cc`, about 1,600 lines -- while SOMns delegates to
`java.math.BigInteger`
(`prior-art/somns/src/som/primitives/arithmetic/AdditionPrim.java`), which
lives in the same JVM heap as its other objects; a Rust crate's heap is not
ours. So `crates/ns-runtime/src/arbitrary_integer/` works on `&[u64]` and
`&mut [u64]`, which is what the heap lends, and knows nothing of the
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
`goldens/library/number/large-integer/large_integer_algorithms_agree_at_every_size.ns`.

**The licensing.** Both projects are MIT OR Apache-2.0; ns-next takes them
under MIT, whose terms require the notice to travel with the code.
`THIRD-PARTY-NOTICES.md` at the repository root carries both licence texts
and the upstream commits, and every file under `arbitrary_integer/` that
holds adapted code opens with a header naming the upstream file, the
commit, the copyright and what was changed for ns-next. `num-bigint`,
`num-integer` and `num-rational` are submodules under `prior-art/`,
recorded in `prior-art/PROVENANCE.md`, read and never built like the
others. `num-rational`'s `Ratio` was the model for `Fraction`'s arithmetic,
its rounding to a double and its exact comparison with a float ("The
numeric tower" below), but no code is taken from it, so it has no entry
in `THIRD-PARTY-NOTICES.md`.

## Deviation: a non-integral literal is a binary float

§5.1.1: "Non-integral numbers denote an object that obeys the protocol of class
`FixedPointNumber`" — an exact decimal type, chosen there so that "financial
applications in particular" get "accurate decimal numbers" and users are not
exposed to floating point's "counterintuitive ways". `2.5` here is an `f64`
(`compiler/lower.rs` parses the literal's text straight into one), so the
arithmetic is binary floating point: `(0.1 + 0.2) = 0.3` answers `false` where
an exact decimal type answers `true`.

Exact arithmetic exists all the same, through `Fraction` ("The numeric
tower" below): `(1/10) + (2/10) = (3/10)` answers `true`, since dividing
integers is exact, while `0.1 + 0.2 = 0.3` still answers `false`, since a
non-integral literal denotes a `Float` and there is no fraction literal.

The specification records the same divergence in its own implementation
("non-integral numbers are currently represented as instances of the Squeak
class `Fraction`. This is a bug"), and it names the open design question —
whether a user chooses the numeric implementation — rather than settling it.
A golden for the rule would have to be skipped behind a decimal type that
does not exist, so the deviation is recorded here instead.

## A NaN equals a NaN

`Float>>=` is IEEE 754's equality except that every NaN equals every other,
and every NaN hashes alike (`tower::equal` and `tower::hash` in
`builtins/numbers/tower.rs`, the fast path of a `Float` argument in
`builtins/numbers/float.rs`). A `Float` is a value, so its `=` must be an
equivalence, and its `==` answers what `=` does (§3.1.1); IEEE's equality is
not one, since a NaN equals nothing, itself included, so a NaN would not even
be identical to itself. This is JavaScript's SameValueZero, the equality its
`Map` and `Set` use, under which `0.0 = -0.0` stays true and both hash
alike. Julia's `isequal` and Java's `Double.equals` make a NaN equal to a NaN
too, but tell `0.0` from `-0.0`.

The ordering and the arithmetic stay IEEE's: for a NaN `a = b` is true while
`a <= b` is false, and a comparison of a NaN with anything answers `false`.
It differs from Primordial Soup and Squeak, where a NaN equals nothing;
Primordial Soup's `isNaN` is `^(self = self) not`, which no longer answers
`true` here.

The other Newspeak implementations keep IEEE's equality -- Primordial Soup's
`Number_equal` compares `raw_left == raw_right` (`vm/primitives.cc`),
SOMns's `EqualsPrim` `left == right` for two doubles -- and can, because in
them a float's `==` does not follow its `=`: a NaN there is unequal to
itself yet identical to itself. Here §3.1.1 ties the two together, so the
options are only these:

- IEEE's `=`, `==` following it: a NaN is not identical to itself, and every
  `IdentityMap`, `includes:` and "is this still the same object" breaks on
  it. Rejected: identity that is not reflexive is worse than an equality
  that departs from IEEE in one case.
- IEEE's `=`, a float's `==` comparing bits: `==` then no longer answers what
  `=` does, a deviation from §3.1.1. Rejected.
- Two equalities, IEEE's `=` and a second, reflexive one for the
  collections, as Julia's `==` and `isequal`: `==` still has to follow `=`,
  so a NaN is again not identical to itself, and every collection and every
  program has to pick the right one. Rejected.
- One `=`, reflexive, a NaN equal to every NaN: the only option that keeps
  §3.1.1 and a reflexive identity, at the cost of the one departure from
  IEEE. Chosen.

IEEE's NaN unequal to itself served, among other things, as the portable
test for a NaN in languages without one: `x != x`. Here `isNaN` is that
test, a leaf of its own, so nothing needs the self-inequality; a program that
wants IEEE's equality writes `a isNaN not and: [a = b]`. None of the options
costs anything on the hot path.

`Float nan`, `Float infinity` and `Float negativeInfinity` answer the three
values (`0.0 / 0.0`, `1.0 / 0.0`, `-1.0 / 0.0`), on `Float`'s class side
alone: an integer has no infinity however large, nor has a fraction, and
`Number` is abstract, its class side not inherited. They go beyond Primordial
Soup's API, where a program parses the text, and were added with the user on
2026-11-01. There is no literal for them: §5.1.1's grammar has none, and a
literal would change both lexers, the parser and the printer for what a
constant already says; a subclass of `Float` for the NaN was weighed and
rejected too, since a NaN is a result any operation can make, so every float
operation would have to choose the class of its answer. Decided with the user on 2026-10-01 and
confirmed on 2026-11-01;
`goldens/library/number/float/not_a_number_equals_itself` pins it, a NaN found again
as a `Map` key included, and `every_nan_equals_every_other_and_hashes_alike`
in `builtins/numbers/float.rs` NaNs of different bit patterns.

**`isNaN` is a leaf over `f64::is_nan`**, so a NaN is still found: `Float`
overrides `Number>>isNaN`, which answers `false`, so every integer and
every fraction answers it without a method of its own; `isFinite` and
`isInfinite` are shaped the same way (noted with the user on 2026-10-01,
built with the numbers sub-project, "The numeric tower" below). A NaN stays
an ordinary `Float`; a class of its own was considered and set aside:
- a `Float` is one class whether it is an immediate or boxed, and a NaN
  class would make the class of every float depend on its bits;
- every send site that computes with floats would then see two classes, and
  its inline cache would turn polymorphic for all float arithmetic, not
  only for NaN;
- the class of `0.0 / 0.0` would differ from that of `1.0 / 2.0`, and
  `self class == Float` would be false in a method of a value that is an
  IEEE double and computes like one;
- it would save only the test inside `isNaN`; equality is already settled
  by `=` above.

Spur's `SmallFloat64` and `BoxedFloat64`, where NaN and the infinities are
boxed because they fall outside the immediate exponent range, split `Float`
by representation, not by meaning. ns-next boxes the same floats and keeps
one class, `Float`, for both representations
([object-model.md](object-model.md), "Values and objects"), so one `isNaN`
test serves both.

## A Float's text

`Float>>printString`, `asString`, `asStringFixed:`,
`asStringExponential:`, `asStringPrecision:` and `Float class>>parse:` are
leaves over `numeric/float_text.rs`, which is plain Rust with no heap, so
that it is tested as such and so that `Heap::describe` writes a float in an
error message as `printString` does.

The layout is Primordial Soup's, which is what a program written against
that API expects: the shortest digits that read back as the same double,
the closest of them to it, and of two equally close the one ending in an
even digit, as Primordial Soup's printer breaks the tie (Rust's `{:e}`
finds them, but breaks the tie upward, so `float_text.rs` checks a value
of at most 18 significant digits for one: 900719925474099.25 prints as
`900719925474099.2`), written in decimal when the exponent of the first
digit lies from -6 to 20 and in exponent form (`1e+21`, `1e-7`) outside it;
`.0` after a decimal text that would otherwise look integral (`3.0`,
`100000000000000000000.0`); `NaN`, `Infinity` and `-Infinity` by name; a `-`
for every negative value, `-0.0` included. `asStringFixed:`,
`asStringExponential:` and `asStringPrecision:` round half up on the value's
exact binary expansion (800 digits of Rust's `{:e}` are the value itself, not
a rounding), so `0.125 asStringPrecision: 2` is `'0.13'`. A digit count
outside the range of its selector (0 to 20, 0 to 20, 1 to 21), a
`LargeInteger` included, is an `ArgumentError` carrying it.

`Float parse:` reads an optional sign, then `Infinity`, `NaN`, or digits with
an optional point and an optional exponent, and so every text `printString`
writes: a Float read back from its text is the same Float. The grammar is
checked before Rust's parser runs, because that one also takes `inf`, `nan`
and `infinity` in any case; here they are an `ArgumentError`, as is any
other text. The mirror row is `floatClass:parse:ifFail:`, the class as the
subject like `stringClass:with:ifFail:`.

## The numeric tower

`Number` has four representations, and every number is exactly one of
them: an `Integer`, the immediate `SmallInteger`; a `LargeInteger`, every
integer beyond 61 bits ("Large integers" above); a `Float`, immediate or
boxed ([object-model.md](object-model.md), "Values and objects"); and a
`Fraction`, the exact quotient of two integers that do not divide. They
compute with one another symmetrically: whichever is the receiver and
whichever the argument, `=`, the orderings, `hash` and the arithmetic
answer alike (decided with the user on 2026-10-06, the rules on
2026-10-11; "Decisions" below).

**This is a library question, which the specification leaves open.** The
specification mentions `Number` exactly twice -- §3.1.1, listing it among
the value objects, and §5.1.1, on non-integral *literals* -- and fixes no
arithmetic protocol anywhere; §8.3 leaves the platform's modules to provide
classes "some of which are standard and mentioned in this specification".
So the protocol is Primordial Soup's, adopted as the kernel completion
adopts the rest of its core API ("The kernel completed to
Primordial Soup's core API" below), and the tower under it is made
symmetric where Primordial Soup's is not: there `(1/2) = 0.5` is `false`;
`Float>>hash` is `asInteger`, so `1.5 hash = 1 hash`; `Fraction>>hash` is
`numerator bitXor: denominator`, so `1/2` and `0.5` hash apart; and a
`LargeInteger` meets a `Float` through a lossy conversion. `Number`, the
class §3.1.1 names, holds what the four share: the tower's rows, the
counting loops, `max:`, `min:`, `between:and:`, `raisedTo:`, the
mathematics through `asFloat`, and the answers to `isNaN`, `isFinite` and
`isInfinite`.

**`Fraction` is a builtin with a representation of its own**, as
`LargeInteger` is, not a Newspeak class with slots: a subclass of `Number`,
reached as `Fraction`, `Object`'s accessor ("The
predefined classes are accessors on `Object`"). Its object is
of the fixed-pointer format with two fields, the numerator and the
denominator, each an integer value -- a `SmallInteger` or a `LargeInteger`
handle. They are no slots, since the class declares none; nothing writes
them after creation; and the collector traces them as it traces any fixed
pointer field, so neither the format table nor `pointer_ranges` needed an
arm. A fraction is always in **normal form**: reduced by the gcd of its
parts, the denominator above 1, the sign on the numerator, so equal
fractions have equal parts and no fraction equals an integer. The runtime
alone makes one, through `Mutator::new_fraction`, which
`Mutator::new_rational` calls for an exact result whose denominator is not
1, answering an integer otherwise. `Fraction numerator:denominator:` is the
public factory over it: it reduces, and signals an `ArgumentError` for a
zero denominator or an argument that is no integer. `Fraction
reducedNumerator:denominator:` keeps Primordial Soup's contract -- the
caller supplies the normal form -- but checks it, signalling an
`ArgumentError` for a pair not in normal form rather than building a
fraction that breaks `=` and `hash`
(`goldens/library/number/fraction/a_fraction_is_made_in_normal_form.ns`).
What makes an object a fraction is its class's builtin together with
exactly two fields (`Heap::fraction_parts`), so `Fraction new`, which has
no field, and an instance of a program's subclass, whose class is no
builtin, hold no value and are no number: every leaf declines them, as it
declines `LargeInteger new`
(`goldens/library/number/tower/a_fraction_with_no_value_is_no_number_to_a_value.ns`).
A fraction prints as `(3/4)`, and `Heap::describe` writes it so. An image
carries one inline as a value, value tag 10; the decoder refuses anything
not in normal form, and an object record of fields under `Fraction`'s
header ([image-format.md](image-format.md), "A `Fraction` is a value on the
wire too"). `Integer` answers `isKindOfFraction` with `true`, `numerator`
with itself and `denominator` with `1`: an integer is a fraction whose
denominator is 1.

Being a builtin puts the tower's rules -- exact comparison, the hash,
promotion, normalization -- in one Rust module for all four
representations, lets every arithmetic and comparison leaf accept every
number and decline only what is none, makes a fraction normal by
construction, and costs a mixed operation no Newspeak fallback send. It
was chosen on those merits, not on its cost.

**Comparison is exact.** `=` and `< <= > >=` compare any two numbers by
their exact values, a finite double being the dyadic rational it is
(`Rational::from_f64`; against a `SmallInteger` within 2^53, IEEE 754's
own comparison, which is exact there). Through `double`, Primordial Soup's
way, `=` would be no equivalence: `2^53 = 2^53 asFloat` and `(2^53 + 1) =
2^53 asFloat` would both be true while `2^53 = (2^53 + 1)` is false, so `=`
would not be transitive and no `hash` could agree with it. Here 2^53 + 1
is greater than the double 2^53 it rounds to. The infinities lie beyond
every rational; a NaN is ordered with nothing and equals only a NaN ("A
NaN equals a NaN" above). `=` with anything that is no number answers
`false`, never an error; an ordering with it signals an `ArgumentError`,
"< requires a numeric argument, got nil"
(`goldens/library/number/tower/numbers_compare_by_exact_value.ns`).

**The hash is Python's numeric hash** (`numeric/numeric_hash.rs`): a
number's exact value `n/d` modulo the Mersenne prime P = 2^61 - 1, as `n ·
d⁻¹`, signed as `n` is, folded into the `SmallInteger` range by a function
that is the identity on it. Each infinity and NaN hash as a constant of
their own, as does a fraction whose denominator P divides, which has no
inverse; `0.0` and `-0.0` hash as `0`. Reduction modulo a prime respects
sums, products and cancellation, so equal numbers hash alike whatever
their representations -- `3` and `3.0`, `(1/2)` and `0.5` -- which is the
contract Primordial Soup's `testFloatHash` writes down (`3 asFloat hash =
3 hash`) and its own hashes do not keep. Every `SmallInteger` lies inside
(-P, P), so its residue is itself and the fold leaves it alone:
`Integer>>hash` stays the quick method `^self`, and the integers, which
most hashed keys are, pay nothing for the tower
(`goldens/library/number/tower/numbers_hash_as_they_compare.ns`).

**Arithmetic's result types.** `+ - * /` of two integers are exact, `/`
answering an `Integer` when the divisor divides and a `Fraction`
otherwise, so `7 / 2` is `(7/2)` and `6 / 3` is `2`; with a `Fraction`
they are exact and normal, an integer when the denominator becomes 1; with
a `Float` among the operands they answer a `Float`, the other operand
rounded to the nearest double in one correctly rounded step
(`Rational::to_f64`) and IEEE 754 computing. Primordial Soup's `numerator
asFloat / denominator asFloat` rounds twice, and answers NaN once both
parts pass `f64::MAX`; that is not followed.

**Division by zero.** An exact division by zero -- neither operand a
`Float` -- signals an `ArgumentError`, "division by zero", carrying the
divisor; a `Float`'s division stays IEEE's, so `1.0 / 0` and `1 / 0.0`
answer `Infinity`. Primordial Soup's `(1/2) / 0` answers a fraction with
denominator 0; here it signals.

**`//`, `\\`, `quo:` and `rem:` take every pair of numbers**, as
Smalltalk-80's do, where Primordial Soup's primitives take integers (and,
for `//`, floats) only: `a // b` is `(a / b) floor` and `a \\ b` is `a -
(a // b * b)`, which takes the divisor's sign; `quo:` and `rem:` are their
truncating counterparts, `rem:` taking the receiver's sign. With exact
operands the quotients are integers and the remainders exact, and a
`Float` among the operands makes both a `Float`. A zero divisor signals
"division by zero", `0.0` included, as Primordial Soup's `//` does
(`goldens/library/number/tower/arithmetic_is_exact_without_a_float.ns`).

**`raisedTo:`** is `Number`'s, in Newspeak: an integer exponent computes
exactly by repeated squaring, a negative one answering the reciprocal
through `/` -- a `Fraction` for an integer base -- so `0 raisedTo: -1`
signals `/`'s "division by zero"; any other exponent computes through
`asFloat` and `Float>>raisedTo:`, a leaf over `f64::powf`. `Integer` adds
`raisedTo:modulo:` and `reciprocalModulo:`.

**No double-dispatch selectors.** Primordial Soup's sixteen
`addFromNumber:`, `subtractFromNumber:`, `multiplyFromNumber:`,
`divideFromNumber:` and `…FromFraction:` selectors are its mechanism for
mixed operands, public only for being that; here the leaves compute every
combination, so the selectors are left out. An argument that is no number
signals the same kind of `ArgumentError` in arithmetic as in an ordering,
"+ requires a numeric argument, got nil", where Primordial Soup's
arithmetic ends in `doesNotUnderstand: #addFromNumber:`.

**What the rest of the protocol answers.** `Float`'s `floor`, `ceiling`,
`rounded` (half away from zero) and `truncated` answer a `Float`, as
Primordial Soup's do, so `Infinity floor` and `1e300 floor` answer without
an error or a huge `LargeInteger`, and `asInteger` asks for an integer;
`Fraction`'s answer integers. Two additions beyond Primordial Soup's API
let every number answer the same questions: `truncated` on `Integer`
(`^self`) and `Float`, where Primordial Soup has it on `Fraction` alone
while all three answer `floor`, `ceiling` and `rounded`; and `isNaN`,
`isFinite` and `isInfinite` on `Number`, answering `false`, `true` and
`false`, which `Float` overrides ("A NaN equals a NaN" above). A `Float`'s
texts are "A Float's text" above.

**A value's derived equality compares numbers by the tower.** `Value`'s
derived `=` compares the classes of every pair it meets first, a rule of
ns-next's that §3.1.1 does not state; a pair with a number on either side
is decided by the tower instead, before the classes -- two numbers by
exact value, a number and anything else, `Fraction new` included, as
unequal in both directions -- and the derived hash hashes a number by the
numeric hash. So a value whose slot holds `3` equals and hashes like one
whose slot holds `3.0` (`builtins/value.rs`, `compare_node`;
`goldens/library/value/value_hashes_follow_value_equality.ns`).

**The rules live in one place.** `builtins/numbers/tower.rs` holds them
once: `operand` reads a value as a number a single time -- a small
integer, a large integer, a fraction or a float -- and `order`, `equal`,
`hash` and `compute` take any two. The exact rationals and the hash are
plain Rust with no heap, in `numeric/rational.rs` and
`numeric/numeric_hash.rs`. `Number`'s arithmetic, comparisons, `=`, `hash`
and `asFloat` are the `number:` rows of `MIRROR` (`number:add:ifFail:`,
`numberHash:ifFail:`, …), which `Fraction` inherits; `Integer`,
`LargeInteger` and `Float` keep rows of their own for their fast paths and
send every other case to the same functions, so the answers and the error
texts cannot diverge. `gcd:`, `>>`, `digitAt:`, `digitLength`,
`asStringRadix:` and `Integer parse:radix:` take either integer
representation as their receiver and are rows of a family of their own,
`AnyIntegerPrimitive`, so that `IntegerPrimitive` keeps meaning a
`SmallInteger` receiver, which a later JIT can emit inline. A class-side
row takes the class as its subject -- `floatClass:parse:ifFail:`,
`integerClass:parse:radix:ifFail:`,
`fractionClass:numerator:denominator:ifFail:`,
`fractionClass:reducedNumerator:denominator:ifFail:` -- and its leaf
declines any other subject, so a subclass's class side falls to `super`.

**The fast paths, and why they are there.** `Integer`'s leaves, and the
inlined `IntegerBinary`, compute two `SmallInteger`s on the tagged words.
`Float`'s compute a `Float` with a `Float`, and with a `SmallInteger` as
the tower would: the arithmetic through `tower::arithmetic_double`, the
integer as its nearest double, which is what `compute` does with it; the
orderings and `=` through `tower::ordering_double`, for a `SmallInteger`
within 2^53 only (`exact_double`), where IEEE 754's comparison is the
exact one. Both are inlined generic functions, not closures over a
function pointer. `tower::operand` is inlined too, so a value that is no
object -- `nil`, a Boolean -- is refused without a call, and a value's
derived equality reads the second side of a pair only when the first is a
number. Every other case goes to the tower;
`the_float_fast_path_agrees_with_the_tower` holds the fast path to it over
every pair of its samples. The first build of the tower lacked these and
was measurably slower: Richards by 7.5%, since its `NoWork == packet`, with
`NoWork` nil, goes through a value's equality, which then asked the heap of
both sides whether each was a number; Mandelbrot by 6.3% and CD by 2.8%,
since `Float`'s comparison and arithmetic called their operation through a
function pointer, with a store-forwarding stall after the call, and a
`Float` with a `SmallInteger` went through the whole classification.
Measured on 2026-10-11, with these paths: the Are We Fast Yet benchmarks
run as fast as on the tree without the tower, the largest ratio against it
Json's 1.005, within the noise (three pinned builds a side, pooled, by the
protocol of [execution.md](execution.md), "What removing the re-entries
moved"); Mandelbrot and NBody run about 4% faster.

## `ByteArray`, `StringBuilder` and the views

The kernel completion's fifth sub-project ("The kernel completed
to Primordial Soup's core API" below), designed with the user on
2026-10-12 and built on 2026-10-13
([its plan](../superpowers/plans/2026-10-13-1614-byte-array-and-views.md)).
The kernel has `ByteArray`, a builtin of bytes whose data lie aligned to 32
bytes; the UTF-8 conversions on `String`; `StringBuilder`; and two kinds of
view over a `ByteArray` -- `TypedArray`'s ten classes, indexed by element at
one width, and `DataView`, read and written at byte positions in a byte
order chosen when it is made. The typed arrays are no part of `Kernel.ns`:
they are the shipped module `TypedArraysFactory.ns`, reached as
`platform typedArrays` (decided with the user on 2026-10-13), where
`StringBuilder` and `DataView` stay in the kernel. Where Primordial Soup's API does not fit
strings of code points or a 1-based language, the API is changed here
rather than adopted (decided with the user on 2026-10-12); everything else
is Primordial Soup's. The leaves are `builtins/bytes.rs` (`ByteArray`'s and
the UTF-8 conversions', `BytePrimitive`) and `builtins/elements.rs` (the
views' element access, `ElementAccess`).

**The classes, in one table:**

| Class | Kind | Reached through | Superclass |
|---|---|---|---|
| `ByteArray` | builtin, format `AlignedBytes` | `Object>>ByteArray` (§3.2), as `Array` is | `Collection <: IndexedCollection` |
| `StringBuilder` | Newspeak, over a `ByteArray` of UTF-8 | `platform kernel` | `Object` |
| `TypedArray` (abstract) | Newspeak, over a `ByteArray`, in `TypedArraysFactory.ns` | `platform typedArrays` | `Collection <: IndexedCollection` |
| `Int8Array`, `Uint8Array`, `Int16Array`, `Uint16Array`, `Int32Array`, `Uint32Array`, `Int64Array`, `Uint64Array`, `Float32Array`, `Float64Array` | Newspeak, in `TypedArraysFactory.ns` | `platform typedArrays` | `TypedArray` |
| `DataView` (abstract) | Newspeak, over a `ByteArray` | `platform kernel` | `Object` |
| `LittleEndianDataView`, `BigEndianDataView` | Newspeak, private to the kernel | `DataView`'s factories only | `DataView` |
| `IndexedCollection` | a mixin, public so that a shipped module applies it, handed out by no facet | applied with `<:` | `Collection`, which its own members build on |

The views and `StringBuilder` are ordinary Newspeak classes whose state is a
`ByteArray` slot; only `ByteArray` is a builtin. A view's access is one
mirror leaf on its `ByteArray`, the width, the signedness and the byte
order in the selector, so no access branches on them, and nothing outside
`ByteArray` needs a heap format or a mover's attention (decided with the
user on 2026-10-12, against typed arrays as builtin heap objects, which
would forbid a view over an existing `ByteArray`, and against leaves that
take the view itself, which would give Rust the layout of a Newspeak
class). They are no builtins either: an image names them, their private
classes and the levels `IndexedCollection` adds by where they are
declared, through the shipped names ("The bootstrap and the image take
them" below).

### `ByteArray` in the heap, the movers and the image

**Its own format, `AlignedBytes`,** codes 24 to 31
(`heap/header.rs`), the low three bits counting the unused bytes of the
last word as `Bytes`'s do. A `ByteArray` on `Bytes` would be a `String`
everywhere -- `PackedString::new`, `Heap::is_string`, `object_shape`,
`describe` and the encoder decide a string by the format alone -- so the
format is new rather than the class checked. `pointer_ranges` answers only
the class word, as for every raw format.

**Its data lie at a 32-byte address** (decided with the user on 2026-10-12:
AVX2's width, so no 32-byte load crosses a cache line; 16 would let one in
four do so, 64 costs seven words an object), for every `ByteArray`, since
`TypedArray on:` takes any. The layout is the header, the class word,
**three reserve words** and the data words; the data begin at the first
32-byte address at or after the first field word, 0, 8, 16 or 24 bytes
in, **computed from the object's address, never stored**, so no header bit
records it and the object's size never changes when it moves -- the
compaction's forwarding counts words. An offset into the object space is an
address modulo 32, since `ns-memory`'s reservation is page-aligned. `size`
is (field words − 1 − 3) × 8 − the unused bytes. A new `ByteArray` is
zero-filled; `ByteArray new` answers an empty one, as `Array new` does;
one of 32 KiB or more is allocated old, as every object is. A size the
program chose is asked first whether it can be allocated
(`Heap::byte_array_fits`): the block, its reserve and overflow word
included, must fit the old space of the reservation, and the operating
system must grant its words, probed with `try_reserve_exact` as a large
`Array new:` is (`Heap::fields_fit`), since a failed allocation aborts the process where an
`ArgumentError` can be caught -- "new: cannot allocate a ByteArray of …
bytes", as for `,`, `withAll:`, `asUtf8` and a typed array's `new:`.

**Every mover realigns the data.** Whoever copies an `AlignedBytes` object
to another address moves its data within the new block to the new block's
aligned position (`heap/object.rs`'s `realign`, a `copy_within`; the ranges
may overlap): the scavenger's `copy` (`heap/scavenge.rs`), the
compaction's slide (`heap.rs`), and the move stress mode, which shifts
whole regions and so walks each afterwards, realigning every `AlignedBytes`
object from its old header index, the new one less the shift. An
allocation's fields are a range of word indices into the space, whose base
is page-aligned, so the data begin at the first field index rounded up to
a multiple of four. It is the first format that needs a per-object fix-up
in the movers; slack inside the object touches the fewest invariants,
where an aligned bump with `Free` fillers would break the compaction's
dense forwarding. The heap's readers `debug_assert!` the alignment, and the
move stress mode's shifting offsets reach all four positions. The rule is
one of `CLAUDE.md`'s invariants: a mover that skips it leaves the data at
the old offset within the block, wrong bytes at exit status 0
([memory.md](memory.md), "Every mover realigns a `ByteArray`'s data").

**`heap.rs`'s accessors copy out**, as the seam's do: allocating, `size`,
a byte, a range copied, a range replaced (`memmove`), and a value read or
written at a width and byte order. Bytes are no handles, so no write
barrier is involved. `describe` answers "a ByteArray of 3", as an `Array`
describes itself.

**The image** carries a `ByteArray` as the record `OBJECT_BYTE_ARRAY`,
object body tag 11: a u32 length, then the bytes, its class named as every
shipped class is. The decoder's first pass sizes the record from its own
bytes -- a length that more bytes than are left would follow is refused
before anything is allocated -- and the second fills the placeholder; the reserve is not written, and the
decoder places the data at its own address's alignment. The body is held to
its header both ways: a byte-array body under any class but `ByteArray`,
and a `ByteArray` header over any other body, are
`ByteArrayRecordMismatch`, so no `AlignedBytes` object of another class and
no `ByteArray` of another format is ever decoded. `inspect` reads the
record, a truncated or overlong one is a `DecodeError`, and
`tests/opcode_tags.rs` pins the tag ([image-format.md](image-format.md),
"Object table"). A view or a `StringBuilder` is an ordinary instance whose
slot holds the record, its class named as every shipped class is ("The
bootstrap and the image take them" below).

**As a builtin class** `BuiltinClass::ByteArray` is appended to `ALL` and
reaches the list `types.rs` keeps: `kernel_class_name`, the hierarchy
test, `OBJECT_CLASS_ACCESSORS` with `Object>>ByteArray` beside
`Object>>Array`, an `allocate_for_factory` branch for `new`, and
`isKindOfByteArray` as a quick method. The leaves decline an instance of a
subclass, as `Array`'s and `String`'s do.

### The sequence protocol is a mixin

`first`, `last`, `keysAndValuesDo:`, `indexOf:`, `copyFrom:to:`,
`replaceFrom:to:with:startingAt:`, `copyWith:`, `copyWithSize:`, `,`,
`collect:`, `select:`, `reject:`, `sort:`, `sortFrom:to:by:` and
`asArray` -- `Array`'s protocol beyond `Collection`'s -- are written once, over `at:`,
`at:put:`, `size` and a new instance of the receiver's kind, in a kernel
mixin, `IndexedCollection` (named with the user on 2026-10-12), that `Array`,
`ByteArray` and `TypedArray` apply over `Collection` -- the last from its
own module, which is why the mixin is public in `Kernel.ns`, though no facet
hands it out: `class Array =
Collection <: IndexedCollection`. A class overrides what a leaf does faster,
as `Array`'s and `ByteArray`'s `copyFrom:to:` and
`replaceFrom:to:with:startingAt:` do.
Programs see the same selectors and no new level: the kernel has no
public `superclass`, so only a class mirror's `superclass` shows the level
between `Array` and `Collection`, a class whose name is the class its
clause named, `IndexedCollection` -- `Class>>name` answers the name a class
object's metaclass gives it (`builtins/behavior.rs`, `Heap::class_name`),
and an applied level's metaclass applies the named class's class side over
an application of the named class's mixin to the level below
(`interpreter/nested.rs`, `build_applied_levels`), §6.2's "class *I* with
mixin *M*". `IndexedCollection` declares `Collection` its own
superclass, which records that its members build on `Collection`'s
protocol; applying it with `<:` takes only its own members (§6.2). A mixin rather than an abstract
class (decided with the user on 2026-10-12): the shipped library uses the
construct the language has for a protocol shared across classes, compiled
code lives on the mixin and is shared by every application
([compiler.md](compiler.md)), and `List` could apply it later. The
bootstrap builds a builtin over a mixin application rather than falling
back to an abstract class ("The bootstrap and the image take them" below).
The levels `IndexedCollection` adds under `Array`, `ByteArray` and
`TypedArray` are the shipped world's levels: an image names them as
`Kernel Array level 0` and the like, a frame's class and nothing else
([image-format.md](image-format.md), "Shipped names").

The mixin's range helpers check a non-Integer bound through `Array`'s
range leaf, on an empty `Array` made for the check, so that the texts are
`Array`'s; that error's receiver is that `Array`, not the collection the
program sent to ([OPEN-WORK.md](../OPEN-WORK.md)). A user subclass of
`Array` reaches the mixin's methods through `super` like any other
(`goldens/library/builtin-subclass/a_subclass_of_array_takes_the_sequence_protocol`).

### `ByteArray`'s protocol

Decided with the user on 2026-10-12. Primordial Soup's byte methods, the
sequence protocol, and `Collection`'s:

- **Class side.** `new` (empty), `new: size` (zero-filled; a negative, a
  non-integer or an unallocatable size is an `ArgumentError` in `Array
  new:`'s texts), and `withAll: collection`, from a `ByteArray`, an
  `Array`, any collection of integers 0 to 255, or a `String`, which counts
  as its UTF-8 bytes; an element outside 0 to 255 is an `ArgumentError`
  carrying it.
- **Elements.** `at:` answers an integer 0 to 255; `at:put:` answers the
  value, and a value outside 0 to 255 or no integer is an `ArgumentError`
  carrying the value -- Primordial Soup's carries the index -- with the
  byte unchanged. Index errors are `Array`'s texts.
- **The sequence protocol** from the mixin; `collect:` answers an `Array`,
  since the block may answer anything, and `select:` and `reject:` a
  `ByteArray`. `asArray` answers an `Array` of integers, `asByteArray`
  the receiver.
- **Primordial Soup's searches.** `indexOf:`, `indexOf:startingAt:`,
  `lastIndexOf:`, `lastIndexOf:startingAt:`, `startsWith:` and `endsWith:`
  decide by the argument: an integer is an element, a `ByteArray` or a
  `String` a subsequence -- one byte means the same either way. The bounds
  of a start are Primordial Soup's: 1 to `size + 1`, an empty subsequence
  found at the start. Anything else is looked for as an element by
  `indexOf:` and `lastIndexOf:`, each byte sent `=` with it, and is refused
  by `startsWith:` and `endsWith:` with an `ArgumentError`, "startsWith:
  requires an Integer, a ByteArray or a String, got …", as the canonical
  implementation refuses it where the design was silent. A `String` needle
  is encoded again at every candidate position
  ([OPEN-WORK.md](../OPEN-WORK.md)).
- **`replaceFrom:to:with:startingAt:`** takes a `ByteArray` or a `String`
  as its source in its leaf and is right when source and receiver overlap
  (`memmove`); `,` takes either and answers a `ByteArray`. Any other
  source falls to `IndexedCollection`'s method, which stores element by
  element, so an element `at:put:` refuses leaves the bytes before it
  stored, as its comment says: the mixin cannot check every element
  without a copy.
- `=` and `hash` are identity, as Primordial Soup's are and §3.1.1 lists no
  `ByteArray` among the values; a content hash on a mutable object would
  move under a `Map`'s key. `printString` is `Object`'s, "an Object", as
  `Array`'s is, so a refusal text built in Newspeak renders a `ByteArray`
  argument that way ([OPEN-WORK.md](../OPEN-WORK.md)).
  `isKindOfByteArray` and `isKindOfCollection` answer `true`.
- **Not adopted:** `asString` and `copyStringFrom:to:` (`String fromUtf8:`
  instead), `copyByteArrayFrom:to:` (`copyFrom:to:` already answers a
  `ByteArray`), and the width accessors `int8At:` to `float64At:put:`
  (`DataView` instead).

**A `String` argument is its UTF-8 bytes; a `ByteArray` argument to a
`String` is refused** (decided with the user on 2026-10-12): the receiver
decides the unit, and an argument of the other kind is taken only where the
conversion cannot fail. Encoding a string is total, so `bytes startsWith:
'GIF89a'` works as in Primordial Soup -- Newspeak has no byte literal, and a
string stands in for one -- and a leaf encodes while it compares or copies,
allocating nothing, an ASCII string's code points being its bytes. Decoding
can fail, so `String`'s methods take only strings, as they already do, and
a predicate never fails on an encoding; the conversion is written out.

### UTF-8

Decided with the user on 2026-10-12:

- **`String>>asUtf8`** answers a new `ByteArray` of the UTF-8 bytes and
  never fails.
- **`String fromUtf8: bytes`** and **`String fromUtf8: bytes from: start
  to: stop`** decode strictly, as Rust's `str::from_utf8`: a truncated
  sequence, a stray continuation byte, an overlong encoding, a surrogate
  and anything past U+10FFFF are an `ArgumentError` naming the byte
  position, "fromUtf8: invalid UTF-8 at byte 5"; a range that ends inside
  a character is invalid; the range's bounds are `copyFrom:to:`'s, their
  errors under the keyword `from:` ("fromUtf8:from:to: …"); an argument
  that is no `ByteArray` is an `ArgumentError`, "fromUtf8: requires a
  ByteArray, got …". The byte position is the byte's index in the whole
  `ByteArray`, not in the range. A byte-order mark
  stays U+FEFF. Strict, not U+FFFD per bad sequence: nothing is lost
  silently, and a program that wants replacement characters writes them.
- **`String withAll: bytes`** is element-wise, as for any collection:
  every byte a code point, Latin-1, never failing.
- Primordial Soup's `asByteArray`, `copyByteArrayFrom:to:` and `asString`
  are renamed so that the encoding is in the name.

### `StringBuilder`

Primordial Soup's protocol in code points (decided with the user on
2026-10-12), in Newspeak over three private slots: `data`, a `ByteArray` of
UTF-8; `byteCount`; and `codePointCount`.

- `new` (16 bytes) and `new: capacity` (`capacity` bytes, a hint; a
  negative or a non-integer is an `ArgumentError`). Growing replaces `data`
  with a `ByteArray` of 1.5 times its size, or more if needed, so an
  append costs amortised O(1).
- **`add: aString`** first refuses what is no `String` -- a `Symbol` is one
  -- with an `ArgumentError`, the builder unchanged; a `ByteArray` is
  refused too, `add: (String fromUtf8: bytes)` being the written-out form.
  It answers the argument.
- **`addCodePoint: anInteger`**, Primordial Soup's `addByte:` renamed,
  appends one code point; anything that is no Unicode scalar value is an
  `ArgumentError` and nothing is appended (Primordial Soup's leaves a
  stray zero byte). It checks the value itself before it writes, "addCodePoint:
  requires a Unicode scalar value, got …", so the error names
  `addCodePoint:` and not the `ByteArray` store behind it, whose leaf
  reports `at:putCodePoint:`, its own selector, as every row reports the
  method that forwards to it. It answers the argument.
- `writeln: aString` is `add:` and then code point 10, answering the
  receiver. `size` counts code points, `isEmpty`, `isKindOfStringBuilder`.
- `asString` answers `String fromUtf8: data from: 1 to: byteCount`, a new
  string; `asUtf8`, Primordial Soup's `asByteArray` renamed, answers a copy
  of the bytes. `data` is never handed out, so it holds valid UTF-8.
- `=` and `hash` are identity. Two leaves serve it, each a public method
  too: `String>>utf8Size`, a string's UTF-8 length in bytes, its size when
  it is ASCII, and `ByteArray>>at:putCodePoint:`, which writes one code
  point as UTF-8 at a position of a `ByteArray` and answers the bytes
  written, so `addCodePoint:` makes no intermediate string. A program
  names the class as `platform kernel StringBuilder`.
- **A state its factory did not make is refused.** The three slots are
  ordinary slots, so an image can give a builder a count past its data or
  one that is no `Integer`; every member that reads them first sends the
  private `checkState:`, which signals an `ArgumentError` carrying the
  builder, "add: found a StringBuilder whose state is not its own", naming
  the selector sent, so a crafted builder never ends in a
  `MessageNotUnderstood` or wrong text. A builder its own factory made
  always passes.

### The typed arrays

Decided with the user on 2026-10-12:

- **Ten classes under `TypedArray`**, JavaScript's names: `Int8Array`,
  `Uint8Array`, `Int16Array`, `Uint16Array`, `Int32Array`, `Uint32Array`,
  `Int64Array`, `Uint64Array`, `Float32Array` and `Float64Array`.
  `Uint8Array` is all but empty, a view of width one whose elements are a
  `ByteArray`'s, kept so that the family is whole. Not built:
  `Uint8ClampedArray` (JavaScript's canvas case) and `Float16Array` (Rust
  has no stable `f16`; with the GPU API, [OPEN-WORK.md](../OPEN-WORK.md)).
- **A view over a whole `ByteArray`.** `new: size` allocates a zero-filled,
  aligned `ByteArray` of `size` × the width, a size that is no `Integer`,
  is negative or cannot be allocated being an `ArgumentError` carrying the
  size, in the texts of `new:` itself ("new: requires an Integer size, got
  …", "new: requires a size of at least 0, got …", "new: cannot allocate a
  ByteArray of … bytes", the last counting bytes, not elements; decided with
  the user on 2026-10-13, "Size refusals are the view's own" below);
  `new` an empty one; `on:
  bytes` views an existing `ByteArray`, whose size must be a multiple of
  the width, else an `ArgumentError`; `withAll: collection` is `new:` and
  `at:put:`. `bytes` answers the `ByteArray` itself, not a copy: writes
  through any view reach every other. No window with an offset, which
  would also break the alignment the views exist for. `bytesPerElement`
  answers the width on either side.
- **Little-endian, always** -- every target's order, x86-64's and
  aarch64's, so it costs nothing and an image reads the same everywhere. A
  byte order chosen per array would put a branch or a swap on every access
  and hand later bulk leaves non-native data; a big-endian block is read
  through `DataView`, or converted once by a later bulk `byteSwap`.
- **`at:put:` checks the range**: an integer outside the class's range, or
  no integer, is an `ArgumentError` carrying the value, the element
  unchanged -- as Primordial Soup's, Squeak's and Strongtalk's do, where
  JavaScript wraps modulo 2^n. **A float element takes any number,
  converted as `asFloat` converts it**, since `0 = 0.0` in the tower;
  `Float32Array` then rounds to binary32 to nearest, ties to even, a
  finite value past its range becoming an infinity, a NaN staying a NaN.
  It answers the value given, not the rounded one. A float32 is read
  widened exactly; an `Int64Array`'s or `Uint64Array`'s element may read as
  a `LargeInteger`.
- **The sequence protocol** from the mixin, copies being of the
  receiver's kernel class over a new `ByteArray`; `collect:` answers an
  `Array`, `select:` and `reject:` the receiver's kernel class; `indexOf:`
  compares with `=`, so `3` finds `3.0`. Each typed array's protected
  `newOfSize:` names its own class, `outer TypedArraysFactory Int16Array
  new: size`, as `Array`'s and `ByteArray`'s do, and `TypedArray`'s is abstract: a
  class side is flat (§3.2), so a program's subclass of `Int16Array` has
  only the `on:` of its own factory, and asking the receiver's class for
  `new:` would leave every copy of it a `doesNotUnderstand:`. A
  subclass's copies are therefore `Int16Array`s, as an `Array`
  subclass's are `Array`s (`goldens/library/builtin-subclass/`). `size` is the bytes' size over the width.
  `=` and `hash` are identity; `isKindOfTypedArray` and
  `isKindOfCollection` answer `true`.
- **One leaf per width for `at:` and one for `at:put:`**, on the
  `ByteArray` with the element index; `Uint8Array` uses `ByteArray`'s.
  A view's accessor sends to the mirror with its `bytes` slot as the
  subject, so it is an ordinary method and no primitive method; a slot
  that holds no `ByteArray` makes the leaf decline, and the failure block
  sends the protected `noByteArray:` with the selector sent, which signals
  an `ArgumentError` carrying the slot's object, "at:put: found no
  ByteArray under the view, got …" -- `size`, `do:` and every accessor
  alike. A float view converts a Number with `asFloat` before the leaf,
  and hands anything else to the leaf as it is, so the index is checked
  first, as for every other element. `bytes` is a public immutable slot;
  `TypedArray`'s class side has the public `bytesFor:width:`, the
  zero-filled `ByteArray` a `new:` views, and `fill:from:`, what
  `withAll:` fills, since a class side is not inherited and each typed
  array's sends them. A program names the classes as `platform typedArrays
  Int16Array`; they are the module's private classes, which the facet
  `TypedArrays` hands out by a class accessor each, and it hands out nothing
  else.

### `DataView`

Decided with the user on 2026-10-12, in place of Primordial Soup's width
accessors on `ByteArray`:

- **One public class, the byte order given once.** `DataView on: bytes
  byteOrder: #bigEndian` and `DataView new: size byteOrder: #littleEndian`
  answer an instance of the private `BigEndianDataView` or
  `LittleEndianDataView`; any other order, or an argument that is no
  `ByteArray`, is an `ArgumentError`. `byteOrder` answers the symbol.
  Fixing the order in the class keeps every access free of a branch, and
  naming it once at creation keeps it visible where the view is made, where
  Primordial Soup's accessors are little-endian without saying so.
- **The accessors**: `int8At:` and `uint8At:` in `DataView`, a single byte
  having no order; `int16At:`, `uint16At:`, `int32At:`, `uint32At:`,
  `int64At:`, `uint64At:`, `float32At:` and `float64At:` in each concrete
  class, each with its `put:` form.
- **A 1-based byte position**, as `at:` and `indexOf:` count (decided with
  the user on 2026-10-12; Primordial Soup's count from 0), not necessarily
  aligned: `uint32At: 3` is allowed. A position below 1, or one whose last
  byte lies past `bytes size`, is an `ArgumentError` carrying the position,
  "uint32At: position 0 out of range: valid positions are 1..… (size …)",
  or, when the bytes are fewer than the width, so that no position fits,
  "uint32At: position 1 out of range: no position holds 4 bytes (size 3)"
  rather than an empty range `1..0`; a position that is no `Integer` is one
  too, "uint32At: requires an
  Integer position, got …". The values follow the typed arrays' rules:
  integers checked against their range, floats taking any number, `put:`
  answering the value.
- `bytes`, a public immutable slot, answers the `ByteArray`, `size` its
  size; `=` and `hash` are identity; `isKindOfDataView` answers `true`.
  The byte order is decided by identity with `#littleEndian` and
  `#bigEndian`, so a `String` of the same letters is refused. `over:`, the
  class's own factory, checks only the `ByteArray`; a view it makes has no
  byte order, so its wider accessors do not exist, and a program makes one
  with the two class-side messages above. A slot that holds no `ByteArray`
  is refused as a typed array's is, through `DataView`'s own protected
  `noByteArray:`, "int8At: found no ByteArray under the view, got …". A
  program names the class as `platform kernel DataView`; the two
  subclasses are private. `DataView` depends on no typed array: it is
  built over `ByteArray` and the mirror's element leaves, so it stays in
  the kernel.
- **Size refusals are the view's own** (decided with the user on
  2026-10-13). `DataView new:byteOrder:` and a typed array's `new:` check
  the size themselves, in Newspeak (`DataView class>>bytesOfSize:` and
  `TypedArray class>>bytesFor:width:`), and signal an `ArgumentError`
  carrying the size under their own selector: "new:byteOrder: requires an
  Integer size, got …", "new:byteOrder: requires a size of at least 0, got
  …" and "new:byteOrder: cannot allocate a ByteArray of … bytes", and a
  typed array's the same under `new:`, the last counting `size` × the
  width. They no longer borrow `Array new:`'s refusal, whose texts would
  name `new:` of another class. A size that only pretends to be an `Integer`
  -- an object answering `true` to `isKindOfInteger` and `false` to `< 0`
  -- passes the first two checks, and the leaf below declines it; the
  decline is refused as a size that is no `Integer`, under the sent
  selector too. `bytesFor:width:` first refuses, under its own selector, a
  width other than 1, 2, 4 and 8, which no typed array gives, so that its
  decline can stem from the size alone.

  Whether a size can be allocated is decided before allocating, by the mirror
  leaf `byteArrayClass:fits:ifFail:`, which answers a Boolean through
  `Heap::byte_array_fits` and raises nothing. An allocating leaf could not
  do it: a row's reported selector is fixed, and every error text of a leaf
  names it, so a leaf that allocated would refuse under `new:` where
  `new:byteOrder:` was sent; a `ByteArray new:` caught and re-signalled
  was ruled out. The leaf declines a subject that is not `ByteArray`
  itself, a size that is negative and a size that is no `Integer`, which
  its callers have refused first, and answers `false` for a non-negative
  `LargeInteger`. It is the 234th row of the mirror's table.
- **The leaves**: 36 selectors on the `ByteArray`, eight widths in two
  orders, read and write, and four for one byte, all one generic Rust
  function over width, kind and order, which the typed arrays share with an
  element index in place of a byte position.

### Errors and tests

Every error is an `ArgumentError` carrying what was wrong -- the index, the
position, the value, the argument, the size, the element width, or the
byte position of invalid UTF-8 -- in the texts of `Array`'s and `String`'s
existing errors but for the size refusals of `new:byteOrder:` and a typed
array's `new:` and the width refusal of `TypedArray bytesFor:width:`,
which have texts of their own ("Size refusals are the view's own" above),
and none leaves a half-changed state: `at:put:`, `add:` and
`addCodePoint:` check before they write. **A refusal written in Newspeak renders the argument with its
`printString`, a leaf's with `Heap::describe`**: the kernel cannot reach
`describe`, and `String`'s own `startsWith:` already did the same, so
`ByteArray withAll: {'x'}` says "withAll: requires an Integer from 0 to
255, got 'x'", where a leaf's text says `got the String "x"`; unifying the two forms is open with the `printString` of a
`ByteArray` ([OPEN-WORK.md](../OPEN-WORK.md)).

The goldens are under `goldens/library/` -- `byte-array/`, `string/`
(`a_string_converts_to_and_from_utf8`,
`invalid_utf8_is_refused_at_its_byte`), `string-builder/`, `typed-array/`
(each class at its bounds, float32 rounding, the infinities, NaN, shared
storage, and `a_typed_array_refuses_a_size_it_cannot_allocate`) and
`data-view/` (both orders, unaligned positions, bounds,
`every_accessor_round_trips_in_both_orders` over all 36 rows, and
`a_view_refuses_a_size_it_cannot_allocate`) -- and
`builtin-subclass/`'s two for a subclass of `Array` and of `ByteArray`,
under both lowerings. `tests/byte_array_leaves.rs` and
`tests/element_access.rs` call the leaves from the host; heap tests in
`heap.rs` hold the alignment and the bytes through a scavenge, a
compaction and the move stress mode in all four positions; and
`tests/image.rs` round-trips a `ByteArray`, the views and a builder,
refuses a truncated, overlong or mismatched record without a panic, and
refuses a view or a builder whose slots an image crafted with an
`ArgumentError`.

### The bootstrap and the image take them

- **The kernel declares ordinary classes beside its builtins.**
  `check_kernel_shape` (`interpreter/kernel.rs`) holds every builtin to
  one nested class of its name, no slots and a bare superclass clause, and
  leaves every other nested class unchecked: `IndexedCollection`,
  `StringBuilder`, `DataView` and its two are ordinary classes, built by
  their nested-class accessors as any class is. A builtin's clause may carry `<:` clauses, each a bare name of a
  kernel class that is no builtin, whose own clause is a bare builtin name
  and applies nothing (`check_applied_kernel_class`): the bootstrap reads
  them and never runs them. `fill_builtin_classes` builds that class once
  (`applied_kernel_class`), its mixin over the builtin its clause names,
  then each level as `nested::build_applied_levels` builds one -- the
  mixin's application over the level below, a class object made with
  `new_applied_level` -- applies the builtin's own mixin over the topmost
  level, links the superclass to it, and passes the levels to
  `give_existing_class_objects_metaclasses`; `wire_kernel_in` caches the
  applied class in the kernel instance, so `Array`, `ByteArray` and the
  accessor share one `IndexedCollection`, and gives each level and its
  metaclass the kernel's lexical context, as it does for the kernel's own
  `Value` level. `TypedArray`, which is no builtin and lives in
  `TypedArraysFactory.ns`, applies the mixin through its clause like any
  class, `kernel IndexedCollection` having been handed to its root. The
  hierarchy test walks through the level.
- **An image names them by where they are declared**
  ([image-format.md](image-format.md), "Shipped names"; "Shipped names in
  the image" above): the bootstrap builds the kernel's classes whole,
  metaclasses and levels included, and `complete_shipped_world` names each,
  a builtin's levels read off its superclass chain; a typed array's class
  is built and named, levels included, when its accessor first builds it,
  and the loader builds one an image names before it decodes the graph. A
  program holding a `Float32Array`, a `DataView` or a `StringBuilder` is
  written to an image without these classes being builtins; the typed
  arrays are named by their module and declaration, `TypedArraysFactory
  Float32Array`, the others by the kernel's. `LittleEndianDataView` and
  `BigEndianDataView` stand as a record's class or a frame's but never as a
  value, since they are private and no public class accessor answers them.
  `IndexedCollection`, public only so that `TypedArraysFactory.ns` may
  apply it and handed out by no facet, is likewise named only as a record's
  class or a frame's, never as a value or a superclass, and the levels it
  adds as a frame's class only. A started image may name no shipped module
  but the kernel (`image-format.md`, "Shipped names"). Making the sixteen
  classes -- `IndexedCollection`, `StringBuilder`, `DataView` and its two
  orders in the kernel, `TypedArray` and its ten in `TypedArraysFactory.ns`
  -- builtins was rejected: a builtin is a class Rust needs a handle on,
  not a class the image needs a name for. Every boot builds the kernel's
  five, and each of `TypedArraysFactory`'s eleven is built when a program
  first reaches it, from the embedded shipped code on an optimized boot
  ("The embedded shipped code" below).
- **`Primitive` is four bytes** (a `MirrorPrimitive` is a `u16`, so the
  `MIRROR` table may hold 65,536 rows), and every structure holding one keeps
  its size in the padding it had (`resolve.rs`'s
  `a_resolution_stays_the_size_the_hot_path_was_measured_at`). So the
  generic element access of the typed arrays and `DataView`,
  `ElementAccess`, is one family whose payload packs the element kind, the
  byte order, the indexing (element or byte position) and read or write
  into one `u8`, its 54 rows one leaf, not an enum of four fields.
- **An image's slots are untrusted.** A crafted record can give a view a
  `bytes` slot that holds no `ByteArray`, or a `StringBuilder` counts that
  disagree with its data or are no `Integer`s: a leaf that meets the first
  declines and the failure block signals `noByteArray:`'s
  `ArgumentError`, and a builder's `checkState:` refuses the second before
  any member reads its slots -- never a panic, never a
  `MessageNotUnderstood`.
- **Overrides keep the failure paths.** `Array`'s and `ByteArray`'s leaf
  methods fall back to `super`, which reaches `IndexedCollection` before
  `Collection`, so the mixin declares none of `at:`, `at:put:`, `size`,
  `do:`, `=`, `hash` and `printString`, and
  `goldens/library/builtin-subclass/` keeps its output.

## Weak references

The kernel completion's sixth and last sub-project ("The kernel completed
to Primordial Soup's core API" below), designed with the user on
2026-10-16 and built the same day
([its plan](../superpowers/plans/2026-10-16-1645-weak-references.md)).
The kernel has `WeakArray`, `Ephemeron` and `WeakMap`, with Primordial
Soup's protocol, and both collectors, the image and the stress modes know
weak slots and ephemerons. The specification says nothing about weak
references -- the whole of `newspeak-spec.md` was searched for weak,
ephemeron, finalization, garbage, reclaiming and the collector -- so the
canonical implementation decides, and Primordial Soup's kernel
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`, 852-861
and 2553-2718; its collector `vm/heap.cc`) is the API adopted, every body
written here. The leaves are `builtins/weak.rs` (`WeakPrimitive`): four
for `WeakArray`, `garbageCollect`, six for `Ephemeron`'s accessors, and
`objectIsValue:`, the question a weak structure asks of a key.

**The classes, in one table:**

| Class | Kind | Reached through | Superclass |
|---|---|---|---|
| `WeakArray` | builtin, format `WeakPointers`; factory `withoutSlots`, made by its class side's `new:` | `platform kernel`, and `platform collections` | `Collection` |
| `Ephemeron` | builtin, format `Ephemeron`, three private slots | `platform kernel` | `Object` |
| `WeakMap` | Newspeak, over an `Array` of `Ephemeron`s | `platform kernel`, and `platform collections` | `Object` |

`platform collections` hands out `WeakArray` as well because the corpus
asks it there (`corpus/modules/Brazil.ns:24`, `VictoryFuelTesting.ns:5`)
and the canonical Newspeak's collections namespace has it
(`prior-art/newspeak/Collections.ns:40`); both facets answer the one class
(`tests/platform.rs`, `both_facets_answer_the_kernels_weak_array`).
It hands out `WeakMap` too, a collection a program looks for among the
others (decided with the user on 2026-10-16), again the kernel's one class
(`both_facets_answer_the_kernels_weak_map`). Primordial Soup hands all
three out through the kernel alone, and `Ephemeron`, a building block
rather than a collection, stays the kernel facet's.

**`platform kernel garbageCollect` runs a full collection before its
sender's next instruction**, as Primordial Soup's `Kernel>>garbageCollect`
does (`prior-art/primordialsoup/newspeak/Kernel.ns:44`); it is what a
program -- a golden among them -- uses to observe a slot being cleared.
`Kernel>>garbageCollect`'s leaf only requests the collection
(`Heap::request_collection`), and the run loop serves the request at its
next safepoint, the return from the activated method that sent it. That
method is the kernel facet's `garbageCollect`, an ordinary method whose
body is `kernelModule garbageCollect`: `Kernel>>garbageCollect` itself
compiles to a fast path, which activates nothing and so returns through no
safepoint, so a method of its own must stand between the program and the
leaf, or a slot read right after it would still name the object
(`goldens/library/weak/a_weak_array_forgets_an_object_after_a_collection.ns`).
`interpreter/kernel.rs`'s
`the_platform_modules_members_have_the_fast_path_they_should` pins the
facet's method as activated. It is the one message `PlatformFactory`'s
root sends the kernel instance it keeps, and one the instance understands
("The platform" above).

**The protocol, Primordial Soup's:**

- `WeakArray new: size` answers a `WeakArray` of `size` nils. It refuses
  a size that is no `Integer` ("new: requires an Integer size, got …"), a
  negative one ("new: requires a size of at least 0, got …") and one too
  large to allocate ("new: cannot allocate a WeakArray of … elements",
  asked of `Heap::weak_array_fits` before anything is allocated, as
  `ByteArray new:` asks `byte_array_fits`) with an `ArgumentError`
  carrying the size. `WeakArray new` is not understood: the class's
  factory is `withoutSlots`, which answers an instance with no slots at
  all, on which every leaf declines as on a subclass's instance; the
  factory exists only so that it is not `new`, since there is no
  `WeakArray` without a size. `at:`, `at:put:` (answering the value) and
  `size`, indexed from 1 and refused out of range in `Array`'s texts;
  `do:`, `keysAndValuesDo:`, `isEmpty`, `copyWith:` (a new `WeakArray` one
  longer, the element last), `isKindOfArray` and `isKindOfWeakArray`
  answering `true`. It holds any object, a value included: a slot whose
  object is collected reads `nil`, and an immediate -- a `SmallInteger`,
  `nil`, `true`, `false` -- is never collected.
- `Ephemeron new` answers one whose `key`, `value` and `finalizer` are
  `nil`; `key:`, `value:` and `finalizer:` answer the receiver. Its state
  is three private slots, `storedKey`, `storedValue` and
  `storedFinalizer`, the key first, since the collectors read the first
  field as the key, each without an initializer;
  `check_kernel_shape` (`interpreter/kernel.rs`) holds the declaration to
  that order and to no initializer at boot. No method names them: they
  give an `Ephemeron` its three fields and lay out a subclass's instance.
  The accessors are mirror forwarders -- `key` is `^vmMirror
  ephemeronKey: self ifFail: [^super key]`, and so on for `value`,
  `finalizer`, `value:` and `finalizer:` -- which run with no activation;
  `key:` is an ordinary method that checks its argument and then stores
  it through `ephemeron:key:ifFail:`. `key:` refuses a value other than
  `nil` ("A weak collection refuses a value as a key" under "The kernel
  completed to Primordial Soup's core API" below) with an `ArgumentError`
  carrying it, "key: requires an object that is not a value, got "
  followed by its `printString`, signalled in Newspeak once the leaf
  `objectIsValue:ifFail:` has answered `true` -- in Newspeak, not by the
  leaf that stores the key, because the text ends in the key's
  `printString`, which a program's class may define. That leaf sees through a
  future: it declines where it would answer `false` and a future is its
  subject, and the failure path asks again of the future's value
  (`interpreter::future::declines_for_a_future`), so a future standing for
  a value is refused as the value is. `nil` is the key of an ephemeron
  that has none, the state it is made in and the one its collected key
  leaves it in. While its key is an object something else reaches, its
  value and finalizer are reached through it; once the key is reached by
  nothing but ephemerons, the collector sets all three to `nil`.
  `isKindOfEphemeron` answers `true`.
- `WeakMap new`: `at:` (answering `nil` for an absent key), `at:put:`
  (answering the value), `includesKey:`, `removeKey:` (answering the value
  removed, or `nil`), `isKindOfWeakMap`. Keys are compared by identity and
  hashed by `identityHash`; `at:put:` refuses `nil` or a value as a key,
  through the same leaf, with an `ArgumentError` carrying it, "at:put:
  requires a key that is not a value, got " followed by its `printString`,
  where Primordial Soup's ends in a `doesNotUnderstand:`. It has no
  enumeration and no `size`, since what it holds changes with every
  collection. Each entry is an `Ephemeron` in a table probed linearly from
  the key's identity hash, 256 slots to start with and always a power of
  two. A removed or collected entry keeps its place with a `nil` key, so
  that the chains through it stay unbroken, until `at:put:` reuses it --
  having searched the whole chain for the key first, so that no key ends up
  under two entries -- or a rehash drops it. The table is rehashed once
  more than three quarters of it hold entries, and doubled only when more
  than half of it is live, so a map whose keys keep dying does not grow
  without end.

**The finalizer is held, never run** (decided with the user on
2026-10-16). Running it needs a point at which code may run without
interrupting the program: a collection happens at a safepoint inside
whatever method is running, and Primordial Soup -- which clears the slot
and never runs it, a `TODO` in `vm/heap.cc:923` -- Spur, which hands a
fired ephemeron to a finalization process through a semaphore, and
JavaScript, whose `FinalizationRegistry` runs between turns, all answer it
with a scheduler ns-next does not have. The slot is there so that the API
is whole, and running it comes with the actors' event loop
([OPEN-WORK.md](../OPEN-WORK.md)).

**A user subclass gets no payload**, as of every builtin ("The shape of a
kernel method" below). `allocate_for_factory` (`interpreter/instantiate.rs`)
allocates the `Ephemeron` format only for the class whose own builtin is
`Ephemeron`, and `WeakArray new:`'s leaf declines for any subject but
`WeakArray` itself -- unreachable, since a flat class side is sent from
no other, and its failure block sends `new:` on to `super`, which nothing
understands, so it would signal rather than answer `nil`, as the class
factories of `Array`, `ByteArray` and `String` do. So a subclass of
`WeakArray` has no weak slots and its leaves decline, and a subclass of
`Ephemeron` is an ordinary object with the same three slots, held
strongly -- Primordial Soup's "can be
subclassed, but subclasses won't have the ephemeron property"
(`goldens/library/weak/a_subclass_of_ephemeron_holds_strongly.ns`).
`Ephemeron`'s leaves read and write such an instance's three slots
instead of an `Ephemeron`'s fields: those that begin at the own slot base
of the first level of its class's chain applying `Ephemeron`'s mixin,
matched by mixin, since a slot layout is the mixin's, so that a class applying
the mixin with `<:` to a superclass with slots of its own reaches the
mixin's slots too; an object with no such level makes them decline
(`goldens/library/weak/a_subclass_of_ephemeron_reads_and_writes_through_its_accessors.ns`).

**The goldens are `goldens/library/weak/`.** Each asserts only what holds
whether or not an extra collection ran -- a strongly held object stays,
and an object held only weakly reads `nil` after `garbageCollect` -- so
each holds under every stress mode. Heap tests in `heap.rs` and
`heap/object.rs` hold both collectors, a move and the remembered set to
what follows, and `tests/image.rs` and `tests/image_truncations.rs` the
image's records.

### The heap and the collectors

**Two formats**, codes 10 and 11 (`heap/header.rs`): `WeakPointers`, whose
fields are the indexed slots of a `WeakArray`, and `Ephemeron`, whose
first field is the key and whose others -- the value, the finalizer -- are
reached only through a live key. An `Ephemeron`'s fields are no instance
slots: the slot accessors (`Heap::instance_field`,
`Heap::instance_set_field` and the rest) take `FixedPointers` alone, and
the `Ephemeron` format has accessors of its own beside the weak array's
(`Heap::ephemeron_field`, `ephemeron_set_field`, `ephemeron_fields`),
which the mirror leaves, the image encoder and the tests use. That is so
that an ordinary object's slot pays nothing for the format (decided with
the user on 2026-10-16): accessors that took both formats cost Havlak,
CD and DeltaBlueTon80 0.7 to 1.7%, in the mutator rather than the
collectors ([OPEN-WORK.md](../OPEN-WORK.md)).
The slots an `Ephemeron` declares are therefore reached only by its
leaves, and its instantiation writes none of them: they have no
initializer.

**Marking traces strong words; forwarding rewrites every word.**
`pointer_ranges` stays the one enumeration of an object's pointer words,
which forwarding, the moves and the remembered set's verification read;
`strong_pointer_ranges` (`heap/object.rs`) answers what marking and the
scavenger trace: the class word alone of `WeakPointers` and of an
`Ephemeron` whose key is not yet known to survive, `pointer_ranges`
otherwise. A weak word must not keep its object alive, but while the
object lives the word must follow it, or it would name whatever is moved
to its old place. An ephemeron with no fields -- only a crafted image
makes one -- has no key and is traced as a fixed object.

**A full collection** finishes its marking in `Heap::finish_marking`,
between `drain_marking` and `prune_canonical_symbols`, Primordial Soup's
order (`vm/heap.cc:627-682`):

1. Marking puts every `WeakPointers` object it visits on a weak list and
   every `Ephemeron` whose key is an unmarked object on an ephemeron list
   (`mark::Marker`), tracing its class word only.
2. Then each listed ephemeron whose key is now marked has its fields
   pushed, the work list is drained again, and the check repeats, with
   the ephemerons the drain listed, until the work list stays empty. The
   first two checks (`pending::SCANNED_CHECKS`) scan every unresolved
   ephemeron, as Primordial Soup (`vm/heap.cc:853-911`) and Spur
   (`processEphemerons`) do in every round; a fixpoint still running
   after them indexes the ephemerons left unresolved by their key
   (`heap/pending.rs`, `PendingEphemerons`; several may share one). From
   then on every object a drain marks -- only these drains record them,
   and only while the index holds an unresolved ephemeron, so the main
   marking pays nothing -- is looked up in the index once: the
   ephemerons keyed by it have their fields pushed and leave the index.
   Ephemerons a drain visits before their key join the index, or are
   traced at once if their key was marked meanwhile. A chain of
   ephemerons whose keys are reached only through each other's values is
   thus collected whole, and each indexed ephemeron is traced at most
   once and each object looked up at most once, so the fixpoint costs
   what it reaches plus the two scans, linear in such a chain's length,
   where a rescan per round costs its square. A weak map's fixpoint --
   the first check resolves the entries whose keys the main marking
   reached after them, the second finds the dead ones still unmarked --
   ends within the scans and indexes nothing: the index costs a hash
   insertion per ephemeron, which a fixpoint of two rounds never earns
   back.
3. Every ephemeron left unresolved is mourned: its fields are set to
   `nil`.
4. Every word of a listed `WeakPointers` object that names an unmarked
   object is set to `nil`.

Only then may compaction run: its forwarding table answers only marked
objects, and a dead word left in a weak slot would be forwarded to
garbage.

**A scavenge** (`Heap::scavenge`; `Scavenger::finish` in
`heap/scavenge.rs`) does the same within the nursery: a key survives when
it is an immediate, old, or already copied (`vm/heap.cc:848-851`). A
`WeakPointers` object the scavenge copies or finds in the remembered set
has its class word scanned and goes on a weak list; an `Ephemeron` whose
key has not survived goes on the ephemeron list with its class word
scanned alone. After the Cheney loop, each listed ephemeron whose key has
since survived is scanned (an old one that then names a young object
remembered once), the loop is drained again, and the check repeats until
no copy waits to be scanned; the rest are mourned. As in a full
collection the first two checks scan every unresolved ephemeron, and a
fixpoint still running after them indexes the rest by the young object
whose copy would make their key survive -- the key at the end of its
forwarders, and, for a key that is a resolved future, its value as well
(`Scavenger::pending_keys`). Scanning an ephemeron copies what it names,
so a key may survive after its ephemeron was passed over in the same
check: the switch checks each ephemeron once more before indexing it,
and records copies from the first one indexed on. From then on every
object copied -- only the fixpoint records copies, and only while the
index holds an unresolved ephemeron -- is looked up once in the index
and its ephemerons scanned and removed. As in a full collection the
fixpoint is linear in what it reaches, and a weak map's indexes
nothing. Then every young word of a listed weak object
is forwarded to the copy or, its object not copied, set to `nil` -- all
before the nursery is vacated, or the word would name whatever is
allocated there next. An old object leaves the remembered set unless a
word of it, a weak word included, still names a young object afterwards,
so a weak object that held only young referents now gone stops costing a
scan; one that still names a young survivor is remembered once, however
many of its words do.

**A resolved future in a weak word or an ephemeron's key stands for its
value**, in both collectors alike. `Ephemeron>>key:`, `WeakMap>>at:put:`
and `WeakArray>>at:put:` store the future itself, though `objectIsValue:`
and `==` see through it, so the entry lives as long as the value does, not
the proxy. A full collection gets this from
`Heap::replace_resolved_futures`, which runs before it marks and rewrites
every field naming a resolved future, weak ones included, to the value
([memory.md](memory.md), "A resolved future is traced as its value"). A
scavenge has no such pass; instead `Scavenger::key_survives` judges a key
that is a resolved future nothing has copied by its value, and
`see_through_resolved_future` rewrites such a key once it survives, and
such a weak word before it is settled, to the value -- the value's copy
once settled, an old holder remembered if that copy is young. Both apply
one test (`heap::ResolvedFutures`: the future class, the `RESOLVED`
state, a value that is no future). A future the scavenge copied, or an
old one, is kept as itself, and an unresolved future is judged as itself:
`key:` resolves one before it stores it, but a weak slot may hold one
(`goldens/library/weak/a_weak_map_keyed_by_a_resolved_future_keeps_the_entry_for_its_value.ns`).

Neither pass needs the write barrier: they are the collector's own
rewrites ([memory.md](memory.md), "Generations"). Setting a word to `nil`
stores no young object, and the scavenge remembers an old holder itself
when a word it rewrote to a resolved future's value names a young copy. A
program's store into a weak slot goes through the barrier like every
store (`Heap::weak_array_set_element`): an old weak object naming a young
one must be remembered for the scavenge to forward or clear the word.

**The stress modes take them as they are.** The collection and scavenge
stress modes run the passes above at their own safepoints, the move stress
mode forwards every word through `pointer_ranges`, and the scavenge stress
mode's check of the remembered set reads `pointer_ranges` too, so it
counts weak words.

### The image

Two object bodies, the next free numbers (`image/opcodes.rs`):
`OBJECT_WEAK_ARRAY` (12), a `u32` count and that many values, and
`OBJECT_EPHEMERON` (13), a `u32` field count and that many values, the
key first, each under a header that names its kernel class by its shipped
name. **The encoder writes what a collection would keep**, as Primordial
Soup's serializer does
(`prior-art/primordialsoup/newspeak/Serialization.ns:548-561, 718-726,
793-795`; `GraphTable::build` and `GraphTable::weakly_held` in
`image/encode.rs`): a weak word names its object only if the graph reaches
that object strongly otherwise, and is `nil` if not; an ephemeron's value
and finalizer are traced once its key is reached, to a fixpoint, so a key
reached only through another ephemeron's value is found however long the
chain, and an ephemeron whose key never is is written with every field
`nil`. Every round scans in list order, so the encoding stays canonical.

**What takes no record is kept, never cleared.** A weak word or an
ephemeron's key holding an immediate, a shipped name -- which the loading
world holds strongly -- or an object written by value -- a
`LargeInteger`, a boxed `Float` or a `Fraction`, which take no object id
and are written whole wherever they are referenced (§3.1.1) -- is written
as it is, even where nothing else in the graph reaches it. A collection's
timing is unspecified, so keeping more than a collection would is never
wrong, and a value has no identity a load could tell apart. A future is
seen through first. The walk still meets what a weak array's slot or an
ephemeron's key holds: a shipped object there is listed among the shipped
names, and an unnamed shipped class, or the VM mirror, there refuses the
whole image, as a strongly held one does. An ephemeron's value and
finalizer are met only once its key is reached, so an unnamed shipped
class or the mirror that only a mourned ephemeron's value or finalizer
holds refuses nothing; nothing of it is written either way, since the
encoder writes a mourned ephemeron with every field `nil`.

**The decoder holds each body to its class.** It sizes both records from
their own bytes in its first pass, as an `Array`'s, and refuses, in both
passes, `OBJECT_WEAK_ARRAY` under any class but `WeakArray` and any body
under `WeakArray`'s but `OBJECT_WEAK_ARRAY` and the slotless
`OBJECT_INSTANCE` `withoutSlots` makes (`WeakArrayRecordMismatch`), that
instance held to the class's slot count, none
(`WeakArrayInstanceFieldCount`); `OBJECT_EPHEMERON` under any class but
`Ephemeron` and any other body under `Ephemeron`'s
(`EphemeronRecordMismatch`); and an ephemeron record
whose field count is not the class's (`EphemeronFieldCount`), checked
before its values are read. It fills the placeholders in place
(`PlaceholderFormat::WeakArray`, `PlaceholderFormat::Ephemeron`); it
allocates old and never collects while it decodes
([image-format.md](image-format.md), "The image under generations"), so
nothing is cleared at load. `inspect` prints the two records as
"weak array [...]" and "ephemeron {...}".

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
| `whileTrue:`, `whileFalse:` | Lowered by the compiler as a loop for literal blocks. There is no leaf: `Closure`'s method is that same inlined loop, `^[self value] whileTrue: [body value]`, which sends `value` to both blocks ("Real bodies for control flow and `do:`" below) |
| `value`, `value:`, … | A trampoline step that returns the block to activate: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | There is no leaf: `Array>>do:` reads its size through the `collectionSize:` leaf and walks it in an inlined `whileTrue:` loop that sends `at:` and `value:` ("Real bodies for control flow and `do:`" below) |

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

**A mirror is a capability**, which is the second reason to prefer it and
the one §8.2 gives for mirror-based reflection generally. Whoever is not
handed the mirror cannot call a primitive. Under a pragma, any method
anywhere can claim one and nothing marks where the VM surface lies. The
bootstrap creates the mirror and hands it to the factories of the shipped
core-library modules alone, `Kernel vmMirror:` and `PlatformFactory
usingVmMirror:kernel:`, whose roots no program reaches ("The platform"
above); `PlatformFactory` passes it on only to the factories of the shipped
modules it builds on first demand -- `TranscriptFactory`'s, `TimeFactory`'s,
`TypedArraysFactory`'s and `MirrorsFactory`'s -- and a user module would receive it only if a
shipped module passed it on to one, which none does: each root keeps it in
a private slot. `become:` ([object-model.md](object-model.md),
"`become:` is an eager bulk forward") keeps it that way, because no program
sends it: it is a leaf on the mirror, which no shipped module sends,
and the atomic install forwards on the Rust side, only the instances it
migrates, of classes a program edited -- never of a shipped mixin, since
the MirrorBuilder refuses every edit to one, in `asBuilder` and again in
`Mutator::prepare_edit`
([liveness-and-reflection.md](liveness-and-reflection.md), "A
MirrorBuilder and an atomic install"); the leaf itself refuses
every instance of a shipped mixin or a builtin class and every withheld
value, the mirror among them ([object-model.md](object-model.md), "What
the forward refuses"). So no object a shipped name
names, no class object and no kernel-made object a program can reach, such
as a VM error's `Error` and its `Message`, is ever forwarded; the path to
the mirror runs only through class objects and the value roots of the seven
shipped modules, so no program will be able to redirect a reference that
leads shipped code to the mirror. A kernel method running on an instance of
a program's subclass that the install migrated fails the slot-access
check ([object-model.md](object-model.md), "What a slot access checks")
rather than writing the old layout into
the new shape. Six things keep it that way, and loosening any one of them
lets a program claim every primitive:

- Only `resolver::resolve_kernel` records `Binding::MirrorSend`, the mark
  the compiler's recognizers read, and only the shipped core-library
  modules — `Kernel.ns`, and the six entries of
  `interpreter/core_library.rs`'s table, `PlatformFactory.ns`,
  `TranscriptFactory.ns`, `TimeFactory.ns`, `CollectionsFactory.ns`,
  `TypedArraysFactory.ns` and `MirrorsFactory.ns` — are resolved with it; a user module goes
  through `resolve`, so copying shipped text gets an ordinary send to
  whatever that module's own `vmMirror` holds.
- The runtime checks what the resolver cannot: that `Kernel`'s root class
  declares exactly one slot, the immutable `vmMirror`, and that after
  instantiation it holds the mirror (`interpreter::kernel`), and that the
  `vmMirror` of every shipped module given the mirror -- `PlatformFactory`,
  `TranscriptFactory`, `TimeFactory`, `TypedArraysFactory`, `MirrorsFactory` -- is an
  immutable, eager slot holding it, whenever such a module's instance is
  recorded: at boot, by `recordShippedModule:` on its first demand, or by
  the loader (`interpreter::core_library`'s `factory_holds_the_mirror`,
  run by `record_pending`); an instance that fails it is never recorded.
  And no program reaches any shipped module's root instance or root class
  (the factory pattern, "The platform" above), so no program can make
  another root instance, and every object whose code sends to `vmMirror`
  reaches one whose slot was checked.
- The image encoder names a shipped object by its declaration and refuses
  one the rule of what may be named where does not allow
  (`image/shipped_names.rs`; [image-format.md](image-format.md), "Shipped
  names"), so no artifact carries a shipped object's state or the mirror's
  code.
- No program can name the mirror's class: `Object` declares no accessor for
  it or for `Top` (`interpreter/kernel.rs`'s
  `objects_class_accessors_are_exactly_the_nameable_builtins` pins the
  accessor set, and `compiler::primitive_method` recognizes no accessor for
  either), so a bare name for it ends in `doesNotUnderstand:`; the mirror's
  class has no declaration and so no shipped name, and the image decoder
  refuses `Top`, `Activation` and every other shipped name in a position the
  rule does not allow ([image-format.md](image-format.md), "Verification").
- The image encoder refuses an image whose graph reaches the mirror, its
  class, `Top` or their metaclasses, and the factory body's allocation
  (`instantiate::allocate_for_factory`, which `AllocateInstance` runs for
  a program's factory send and a host's `instantiate` alike) refuses to
  instantiate the mirror's class, so there is never a second mirror.
- `SendMirror`, the instruction that runs a leaf with no lookup, is emitted
  only for a `MirrorSend` and has no wire tag, so no image can carry one.
  The embedded shipped code carries the kernel's, in a codec of its own
  that only the boot reaches with the binary's own bytes, so this lock
  stays structural ("The embedded shipped code" below).

Beside the six, the mirrors module's two edit leaves,
`reflectionMixin:checkEdit:ifFail:` and
`reflectionMixin:installEdit:ifFail:`, which only `MirrorsFactory.ns`'s
`ClassDeclarationBuilder` sends, refuse every edit of a shipped mixin
(`Mutator::prepare_edit`, after `asBuilder`), and resolve an edited module
through `resolve`, never `resolve_kernel`, so no edit records a
`MirrorSend` or changes the mirror's class, whose table stays the one the
bootstrap built ([dispatch.md](dispatch.md), "Primitives";
[liveness-and-reflection.md](liveness-and-reflection.md), "The VM
mirror's class must be sealed against it").

The activation objects the exceptions are built on are a capability of the
same kind — one reads another frame's registers and unwinds the stack — and
are held the same way: `Activation` is private to `Kernel.ns`, `Object`
declares no accessor for it, a program reaches one only through a kernel
method's local, and an image names its class only as the header of an
activation record ("Exceptions" below).

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
the accessors for the predefined classes, and the few ordinary methods such as
`True>>printString` or `Value`'s derived equality. The failure path is an
explicit `ifFail:` block rather than something the runtime does implicitly
(decided with the user on 2026-09-21): a real send to the mirror, as §8.3.1
requires, whose failure path is visible Newspeak code, as Cog's method body
is. SOMns uses the same shape for its I/O primitives
(`prior-art/somns/core-lib/Files.ns`,
`vmMirror file: self readAt: pos ifFail: errf`).

The one rule `[^super …]` reproduces what a declining native used to do by
itself: a receiver of the wrong shape — an instance of a user subclass of
`Array`, which carries no array, or a platform object sent another platform
object's selector — reaches the superclass's method, `Object`'s, or for
`Array` `Collection`'s, and a selector neither has ends in `Object`'s
`doesNotUnderstand:` through §5.10's super failure path, with the same
message text.
`goldens/language/send/native_that_fails_on_its_receiver_falls_to_objects`
and
`goldens/runtime/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class`
pin it, and
`goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods`
pins it family by family: `Object`'s `=` and `printString` for each, and
for `Array`'s `size` `Collection`'s, which counts through `do:`, whose
`collectionSize:` leaf declines in turn and reaches `Collection`'s abstract
`do:`, so the golden
ends in the `Error` `subclassResponsibility` signals, 'Collection>>do: is
abstract: MyArray must implement it'. `Object`'s own leaves never decline,
so its blocks are unreachable; they keep the shape so that one rule, and
one recognizer, covers every method. The exception machinery's mirror sends
are the exception to the shape: they name another subject — an activation,
an exception — and their failure blocks answer a value (`ifFail: [nil]`,
`ifFail: [false]`) rather than resend to `super`. Every method is `public`
but `Object`'s `doesNotUnderstand:`, which §5.5 makes `protected`, and the
machinery's own: `Exception`'s `hostSelector` and `hostReceiver` and
`MessageNotUnderstood`'s overrides of them (`protected`), `Exception`'s
handler search (`private`) and `Closure>>valueOnUnwind:` (`private`) — and
`Metaclass`'s two `thisClass` helpers (`private`).

Two things about the shape are enforced rather than conventional:

- **A literal block may appear in a mirror send only as its final `ifFail:`
  argument.** `interpreter::kernel`'s `check_kernel_shape` refuses anything
  else at bootstrap (`check_mirror_send_arguments`), and so does the
  resolving of every other shipped module (`interpreter/core_library.rs`),
  since `TypedArraysFactory.ns` and `MirrorsFactory.ns` hold mirror sends of their own. The reference lowering
  marks a mirror send whose failure block the optimized lowering inlines as a
  `SendInlinable`, and that mark covers the whole send, so it would exempt
  from the depth count a literal block the leaf itself activates — as the
  reference lowering's loop exempts the blocks it sends `value` — which the
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
public ifTrue: trueBlock ifFalse: falseBlock = ( ^trueBlock value )   /// True
public and: block = ( ^false )                                         /// False
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
`goldens/runtime/depth-limit/recursion_through_and_or_counts_alike_under_both_lowerings`
the agreement.

Two semantic decisions came with the move (decided with the user on
2026-09-24):

- **`and:`, `or:`, `&` and `|` do not coerce.** `true and: [3]` answers `3`,
  as in every prior-art kernel, under both lowerings — the compiler's
  inlined `and:`/`or:` moves the block's value into the result as it is
  (`goldens/language/control-flow/and_or_short_circuit`,
  `goldens/language/control-flow/and_or_answer_their_arguments_value`).
- **An argument that is no block is sent `value`, and nothing answers it**,
  as in Primordial Soup: there is no `Object>>value`, so `true ifTrue: 3`
  ends in `doesNotUnderstand:`
  (`goldens/language/control-flow/a_conditional_sends_value_to_its_argument`), while a
  side that decides alone never looks at its argument — `false ifTrue: 3`
  answers `nil`
  (`goldens/language/control-flow/a_short_circuited_conditional_ignores_its_argument`).

A user subclass of `True` inherits these methods, needing no payload for
them, where a subclass of `Boolean` inherits nothing from it and falls to
`Value`'s and `Object`'s methods, hashing by its state like any other value
(`goldens/library/builtin-subclass/a_subclass_of_true_inherits_its_methods`,
`goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods`,
`goldens/library/builtin-subclass/a_subclass_of_boolean_hashes_as_a_value`).

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
`an Object` before (`goldens/runtime/cache/cache_polymorphic_sites`).

**A block for the not-nil case is culled with the receiver** (decided on
2026-09-25, from the corpus): it is handed the receiver when it takes an
argument and nothing when it takes none, Squeak's `cull:`. The prior art
splits here — SOMns sends `ifNotNil:`'s block `value` and culls
`ifNil:ifNotNil:`'s, the JS kernel sends every one `value: self` — and the
canonical Newspeak sources under `corpus/` pass both shapes, 386
`ifNotNil: [:x | ...]` against 126 `ifNotNil: [...]`, which only `cull:`
runs. `Closure>>cull:` is an ordinary `Kernel.ns` method saying so, and
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
`goldens/library/object/the_nil_protocol`,
`goldens/language/control-flow/if_not_nil_culls_its_argument`,
`goldens/language/control-flow/if_not_nil_with_a_two_argument_block_is_a_wrong_arity_error`
and `goldens/library/object/spec_5_1_3_nil_answers_is_nil` pin it, and
`goldens/library/object/nil_is_the_undefined_object` the class's name.

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
   `BuiltinClass::ALL`, never inserted, as `types.rs` asks; no image
   carries its position, since an image names a builtin by its declaration
   ([image-format.md](image-format.md), "Shipped names"). `VmMirror`'s class, whose mixin
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
   builtin's class but `Top` that is a bare sibling name, read and never
   run, naming a kernel class, from which the chain reaches `Top` without
   repeating; that only `Object` names `Top`, as §3.2 allows; `<:` clauses
   on a builtin only as bare names of kernel classes that are no builtins
   and apply nothing themselves; and the literal-block rule above. Every
   other nested class -- `StringBuilder`, the views, `IndexedCollection` --
   is an ordinary class, unchecked, which its accessor builds as any
   class's builds it. A kernel that breaks it is a defect in the binary,
   and panics. An optimized boot decodes the same root mixin from the
   embedded shipped code instead, compiled and checked this way when
   `cargo xtask shipped-code` generated it ("The embedded shipped code"
   below); from here on the two are one path.
3. **Each placeholder gets its kernel application**, in `fill_order`, which
   sorts the kinds by how many superclass links separate them from `Top`, so a
   superclass is always filled before its subclasses. `BuiltinClass::ALL` is
   no longer that order: a class added later is appended, so `Value` and
   `Number` sit
   at the end of `ALL` and in the middle of the hierarchy. Each class gets the
   nested mixin named like it, applied to the application of the class its
   clause names — `Top`'s to none, `Object`'s to `Top`'s, and so on down —
   installed with `Heap::class_set_application`, which restamps the dispatch
   id of the few objects that exist this early, and the class object's
   superclass link set to match. A builtin whose clause applies a kernel
   mixin, `Array = Collection <: IndexedCollection`, is applied over the
   level §6.2 adds for it, built here from the mixin's class, which is
   built once and shared by every builtin applying it ("The bootstrap and
   the image take them" above). `Value`'s *mixin* is recorded just before
   `Value` is filled, and `Behavior`'s just before `Behavior` is, rather than
   its application after the loop: `apply_mixin` asks whether the mixin it is
   applying is that one, so that `Value`'s own application also carries
   `inherits_value` (§3.2's "subclass of" is inclusive, so `Value new` is a
   value) and `Behavior`'s carries the refusal to instantiate a class object,
   and every class below each reads it off its superclass. `VmMirror` keeps
   its own Rust mixin, re-applied over the filled `Value`
   (`superclass_of_kind`); `fill_order` sorts by depth, so `Value` is filled
   first.
4. **The immediate dispatch-id table is refreshed**, because an immediate's
   ids are read off its class's application, which just changed.
5. **The kernel class is built and instantiated** from the host, with the
   mirror as its factory argument: `Kernel vmMirror: mirror`. Its root is
   `= Value`, built by the root rule as a user root's is
   (`resolver::root_shape`, `nested::build_applied_levels`), so its class
   sits directly over the filled `Value`; a `<:` clause would add its level
   the same way. Before the instantiation, every class object built so far
   — each builtin and the kernel's own class — gets its metaclass over the
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
   `Array` rather than building a second class object of the same mixin,
   and so is the slot of each kernel class a builtin's `<:` clause applies,
   `IndexedCollection`;
   the kernel's own `Value` level and each builtin's `IndexedCollection`
   level, built before the instance existed, are given the kernel instance
   and class as their lexical context too; and
   the instance is kept as `Mutator::kernel`, a root beside the mirror
   ([memory.md](memory.md), "What the collector traces"). Each of those
   classes' metaclasses is given the same lexical context, since a metaclass
   shares its class's (§6.3 writes a class side inside its class's
   declaration). The phase checks that the slot holds the mirror.

The resolved kernel is parsed and resolved once per thread and kept
(`thread_local!`): the test suite bootstraps an interpreter well over a
hundred times, and the AST and bindings are immutable. Each interpreter that
boots from source still compiles it, because compiled code is numbered
against one heap's inline caches, and each optimized boot decodes its own.

**A builtin mixin carries the factory `new`, with no arguments.** A kernel
class declares no factory, so its mixin has §6's default, and the
initializer's arity check applies to it like any class's: a host's
`Interpreter::instantiate` of a builtin class with factory arguments is
refused ("wrong number of factory arguments"), where a placeholder, which
declared no factory at all, let any count through. `Array new` still builds
its own payload (`instantiate::allocate_for_factory`), from a program's
send and a host's `instantiate` alike, since both run the factory body.

**`Kernel` is a value, and so is the mirror** (decided with the user on
2026-09-23). `Kernel.ns` declares
`class Kernel vmMirror: mirror = Value (…)`, a value as the header SOMns
writes, `Object <: Value`, makes its own, so the kernel instance — the enclosing object of every builtin value
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

### The embedded shipped code

The shipped modules' compiled code is produced ahead of time and embedded
in the binary (decided with the user on 2026-10-14).
`cargo xtask shipped-code` (`xtask/src/main.rs`) boots an interpreter from
source with the optimized lowering, compiles the root mixin of every shipped module
through the source path -- it needs the mixins, not the world they build,
so it instantiates no module the boot does not -- and writes them, encoded
(`shipped_code::generate`), to
`crates/ns-runtime/core-library/shipped-code.bin`, which is gitignored;
`crates/ns-runtime/build.rs` copies that file into `OUT_DIR`, an empty one
when the tree has none, and `shipped_code::embedded` includes the copy
(`include_bytes!`). An optimized boot decodes a module's root mixin from it
when the module is built -- the kernel's and `PlatformFactory`'s at boot,
every other module's on its first demand ("The module table and the boot"
below) -- instead of parsing, resolving and compiling it; the six modules
are roughly 9,000 lines (`crates/ns-runtime/src/shipped_code/`). From the
root mixins on, a build is one path: `fill_builtin_classes`, each root
class built from its finished mixin, the instantiations,
`complete_shipped_world`, the platform and every later demand run the
binary's own code on the mixins in the same order whichever path produced
them, so neither draws an identity hash and the stress modes see the same
instantiations. Decoding allocates nothing in the heap. Decoding instead of
compiling took the boot from 13.6 ms to 1.6 ms and a hello program's whole
process from 20.8 ms to 5.2 ms; building only `Kernel` and
`PlatformFactory` at boot took the boot from 1.77 ms to 1.22 ms and the
hello's process from 5.24 ms to 4.74 ms, of which the kernel's decode is
0.88 ms, and a first demand of `CollectionsFactory` costs 0.31 ms (release,
the measuring workstation, Windows 11). The two pairs are two measurements,
the eager boot's 1.6 and 1.77 ms taken differently, so each compares only
within its own ([execution.md](execution.md), "What the precompiled shipped
code moved").

**What it holds.** A header -- the magic `NSSC`, a `u16` format version,
the `u64` fingerprint, the module count -- and then per module, `Kernel`
first and the `SHIPPED_MODULES` in order, its name, its section's byte
length and the section: a symbol table in order of first use and the root
mixin with every nested one, each mixin with everything the run time reads
-- name, access, slots, the factory, the superclass clause with its
receiver name and code, every `<:` clause with its receiver name and its
message's selector, whether the declaration writes a body, the three
initializer bodies,
the root shape, `shipped`, the class side, the nested mixins, the span --
and its methods, sorted by selector name so the bytes do not depend on a
`HashMap`'s order. A method is compiled, a slot accessor or a factory, the
last its selector and arity alone: the decoder rebuilds it through
`mixin::factory_entry`, which gives it its synthesized body; a
primitive method occurs only on the mirror's class, which Rust builds, and
the encoder panics on one. A body carries its kind, its counts, its
statement position, its unwind marker, its span and every instruction
whole, `SendMirror` and the call sites included; `family_size` and
`outer_reach` are derived from the instructions through
`CompiledCode::from_builder`, as the compiler derives them, so
`outer_reach_of`'s invariant holds by construction. The layout is
`shipped_code/mod.rs`'s doc comment. The encoder (`shipped_code/encode.rs`)
destructures every type without `..` and matches every enum without `_`,
so a new field or instruction does not compile until its treatment in the
artifact is decided; a field left out says why where it is left out.

**What it leaves out** is what only the compiler and the source path's
checks read: a mixin's, a slot's and a method's `declaration` and the
initializer expressions. A decoded mixin has none, and nothing at run time
reads them. So the kernel phase reads the clauses' receiver names and the
root shape off the mixin rather than the AST, the checks against the AST
-- `check_kernel_shape`, `check_mirror_send_arguments` -- run on the source
path, which is the path `cargo xtask shipped-code` compiles through, and a
test that inspects a declaration boots from source
(`every_builtin_class_is_filled_from_the_kernel`).

**Two paths, chosen once per boot** (`shipped_code::path_for`, kept as the
mutator's `shipped_code_path`; `Mutator::shipped_root_mixin`). The boot
decodes the embedded code when the lowering is the optimized one,
`NS_NEXT_NO_PRIMITIVE_METHODS` is unset and the embedded bytes carry the
magic, the format version and the fingerprint the binary was built with.
Every other boot compiles the shipped source, parsed, resolved with
`resolve_kernel` and checked as "The bootstrap: placeholders first, kernel
second" above and "Where the library source lives, and how modules are
found" below describe. A missing or stale artifact falls
back silently -- a slow boot, correct behaviour -- because a warning would
change every golden's `stderr`, and the gates regenerate the artifact
before they test. Each interpreter decodes its own mixins, as the source
path compiles its own: no interpreter shares a mixin with another.

**Only the optimized lowering is embedded.** The reference lowering
compiles every shipped module it builds from source, so it stays an
oracle that compiles everything afresh, and the goldens, run under both
lowerings, hold the two paths to the same behaviour as well as the two
lowerings. `NS_NEXT_NO_PRIMITIVE_METHODS` is read when a body is compiled,
so code compiled with primitive methods cannot serve a boot without them.
Nor is code compiled without them ever embedded: the fingerprint covers
the sources, not the environment, so such an artifact would carry the
binary's fingerprint and every optimized boot would decode it silently.
`shipped_code::generate` refuses under the variable, which fails `cargo
xtask shipped-code` and the gates with a message rather than write it, and
`the_embedded_shipped_code_keeps_the_mirror_sends_and_fast_paths` holds
the embedded bytes to carrying the kernel's primitive methods.

**The fingerprint.** `build.rs` computes two, FNV-1a over each file's
workspace-relative path and its bytes. `SHIPPED_CODE_FINGERPRINT` covers
everything that can change what the shipped modules compile to: the seven
shipped modules' sources -- `Kernel.ns`, `PlatformFactory.ns` and every
`.ns` under `platform/` -- `ns-runtime`'s and `ns-syntax`'s Rust sources,
`build.rs` itself and both `Cargo.toml`s. An artifact a compiler change
made stale is therefore never decoded; the fingerprint invalidates more
often than necessary -- any edit in those crates -- and that is safe,
since the boot then compiles from source until the artifact is
regenerated. `CORE_LIBRARY_FINGERPRINT`, over the shipped modules' sources
alone, goes into an image's fingerprint instead of the sources themselves,
so an image does not become unloadable with every Rust edit
([image-format.md](image-format.md), "Shipped names");
`the_two_fingerprints_differ` keeps the second from standing in for the
first. Neither covers `Manifest.ns`, which a program compiles as a module
of its own: an edit to it changes neither the shipped code nor the
shipped world. Both cover a shipped module's comments, which are part of
its bytes, so an edit to a doc comment alone changes the image fingerprint
in the wire-format fixture `crates/ns-runtime/tests/fixtures/golden-image.bin`
(`the_wire_format_matches_the_golden_fixture` in `tests/image.rs`): the
same commit regenerates it with `regenerate_the_golden_fixture`, and only
its fingerprint bytes change. `build.rs` names directories, not the artifact,
in `cargo:rerun-if-changed`: Cargo reruns a script whose named file is
missing on every build, and a directory's scan sees the artifact appear.

**Generated, never checked in.** `cargo xtask shipped-code` writes the file
only when its bytes changed, which keeps `build.rs` from rebuilding
`ns-runtime` after an unchanged run, and writes it beside itself and
renames it, so a build running meanwhile embeds the old file or the new
one, never half of one whose valid header would make its every boot panic;
`--check` writes nothing and says whether the file is missing or stale. The
bytes are reproducible -- symbols in order of first use, methods sorted, no
`HashMap` iteration (`generated_shipped_code_is_reproducible`). Its
fingerprint covers the Rust sources, so a checked-in artifact would be
stale after nearly every commit, and a tree without one, a fresh clone,
builds and runs from source. The gates run the task once per call
(`CLAUDE.md`, "The gates"; `xtask/src/main.rs`'s `shipped_code_step` says
why it runs as a build of its own). A release binary built without the
artifact boots from source too, which a later packaging has to rule out
([OPEN-WORK.md](../OPEN-WORK.md), "A release binary without the embedded
shipped code boots from source"). The tests in `shipped_code/mod.rs` run in
the `commit` gate: `the_optimized_boot_decodes_the_embedded_code` fails
when the artifact is missing or stale, saying to run
`cargo xtask shipped-code` -- a bare `cargo nextest run` without it is red, the gate
green, since skipping would leave the embedded path silently untested --
`the_reference_boot_compiles_from_source` is its counterpart, and
`the_embedded_shipped_code_round_trips` re-encodes the decoded bytes, which
with the exhaustive encoder holds the decoder to every field.

**A codec of its own, with no verification.** The artifact's decoder
(`shipped_code/decode.rs`) shares nothing with the image's, and only the
boot reaches it, with the embedded bytes. So the image decoder still has no
arm for `SendMirror`, and the mirror's lock (6) stays structural: no image
byte sequence decodes into one ("The VM mirror is the whole Rust boundary"
above). The other locks hold as they did: the kernel's
`Binding::MirrorSend`s were recorded by `resolve_kernel` when the artifact
was generated, the check of the mirror slots after each module's
instantiation runs on both paths, and the image's locks never see the
embedded code. The bytes are the binary's own, so bytes that do not decode
are a defect of the binary and panic, as a shipped module that does not
parse does; the decoder has no error type and verifies nothing. Embedding
compiled code does not conflict with [image-format.md](image-format.md)'s
"never for the builtins", which is about what an artifact arriving from
outside may carry; bytes compiled into the binary are as trustworthy as the
Rust beside them.

**Rejected:**

- **Compiling each method on its first call.** It touches only the half of
  the boot that compiling took, leaves parsing at every start, and puts a
  "not yet compiled" state into the lookup, the image, the MirrorBuilder's
  atomic install and a later JIT; a large application reaches much of the
  kernel anyway.
- **An embedded heap snapshot of the shipped world**, as Primordial Soup
  boots from one. It would also spare the instantiations, 0.2 ms, but holds
  the mirror and needs a second, trusted path through a heap decoder.
- **Generating Rust source that builds the mixins**: tens of thousands of
  generated lines, and the compile times with them.
- **A `build.rs` in the binary crates with `ns-runtime` as a build
  dependency**, which builds `ns-runtime` twice and leaves its own tests
  without the artifact, or **a compiler crate of its own** that
  `ns-runtime`'s `build.rs` could use, which restructures the interpreter,
  resolver, mixins and bytecode for a build step.
- **A dialect of the image codec**, one encoding per instruction: lock (6)
  would become a check -- the image decoder would have a `SendMirror` arm
  and refuse it by mode -- and the verification an untrusted image needs
  would be switched off for the artifact or paid at every boot.

### Where the library source lives, and how modules are found

The library is source the runtime ships, built as the Rust bootstrap used
to be built -- the kernel and `PlatformFactory` at bootstrap, every other
module on its first demand, each from its compiled code, embedded ahead of
time, on an optimized boot ("The embedded shipped code" above) -- so
running a program from source is unchanged: the bootstrap gained a phase in
front of it and nothing above that changed. A program of several *user*
modules does not go through this table: the host compiles a project's
modules in module order and hands them to the application in its manifest
("The manifest" below).

**The table of shipped modules.** `Kernel.ns` is compiled into the binary
by `interpreter/kernel.rs`; every other shipped module is an entry of
`interpreter/core_library.rs`'s `SHIPPED_MODULES`, a name, its source and
the arguments its factory is given (`ShippedFactory`), embedded with
`include_str!` from `crates/ns-runtime/core-library/` like the kernel.
Today it has six entries, `PlatformFactory`, `TranscriptFactory`,
`TimeFactory`, `CollectionsFactory`, `TypedArraysFactory` and
`MirrorsFactory`, the last five
under `platform/`: the modules the platform hands out a facet of live
there. Where a module's file lies decides nothing: the table names it, and
`moduleInstanceNamed:` and `shippedModuleClassNamed:` find it by that name.
On a boot from source the table parses and resolves an entry once per
thread, on the first request for it, with `resolve_kernel`, because the
entry is shipped source: the boot asks for `PlatformFactory`'s, and every
other entry is asked for when its module is first demanded or an image
names it ("The module table and the boot" below). It panics for an entry
that does not parse or resolve, or whose mirror sends pass a literal block
anywhere but as the final `ifFail:` (the rule of "The shape of a kernel
method" above, which `Kernel.ns` is held to likewise), as the kernel's
does: that is a defect in the binary, not in a program, and the unit test
`every_shipped_module_resolves` checks every entry. Every interpreter that
boots from source compiles each module itself, since compiled code is
numbered against one heap's inline caches, and an optimized boot decodes
its own. Every entry is shipped source; a user module would join the table,
marked as such by the host and resolved with `resolve`, only with the
module namespace on the mirror (below). The mirror's
`moduleInstanceNamed:`, `shippedModuleClassNamed:` and
`recordShippedModule:` (below) are the leaves through which shipped
Newspeak reaches a module's one instance and builds it on first demand, and
the host -- the boot for `PlatformFactory`, the loader for a module an
image names -- takes an entry from the table directly
(`build_shipped_module`, `resolved_shipped_module`).

**Planned: the module namespace on the mirror.** For the modules there are,
the namespace belongs on the mirror (decided with the user on 2026-09-21): a
primitive that looks a module up in a table the CLI filled in advance, rather
than a namespace builtin of its own or an invented factory convention.
Nothing would touch the file system at run time, which is what linking
statically means here. Later the same primitive falls back to the file system
on a miss, so dynamic loading extends one primitive and changes neither
`Kernel.ns` nor anything else on the Newspeak side. Its first half is
built: `moduleInstanceNamed:` ("The platform and collections as
core-library modules" below) looks a name up among the module instances
recorded from the table the binary carries, and `shippedModuleClassNamed:`
builds an entry's root class on its first demand. The table itself is
still the host's rather than the mirror's, and the fallback to the file
system comes with user modules.

**Start-up is measured, not assumed.** Compiling the shipped source cost a
hello program's boot 13.6 ms on the measuring workstation once the library
had grown to six modules of some 9,000 lines, almost all of it parsing,
resolving and compiling; decoding the embedded shipped code cost 1.6 ms for
the whole boot when it built every module, in the same measurement;
building only `Kernel` and `PlatformFactory` at boot measures 1.22 ms
against 1.77 ms for the eager boot in a later one
([execution.md](execution.md), "What the precompiled shipped code moved",
says why the two eager figures differ). Embedded compiled code is decoded
by a codec of its own rather than as an image, so neither `image::load`'s
two verification passes nor lock (6) are touched, and it is generated into
a gitignored file and included through the build script's output directory,
never checked in, and refused unless its fingerprint is the binary's: an
artifact nobody regenerated would decode into a different, perfectly valid
kernel and run it silently, which is the failure mode the wire-tag rule
exists to prevent ("The embedded shipped code" above).

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
  instance is never written ("Shipped names in the image" below); the
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
  activation, whether a `signal` of it has reached its default action
  (`reachedDefault`, which then stays true, as Squeak's
  `reachedDefaultHandler` does), whether a search for its handler is running
  (`searching`, false again once a handler or the default action runs) and the
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
  the search's report just before `defaultAction`, on an intact stack.
- `ArgumentError`, `CannotReturn`, `NonBooleanReceiver` and
  `CannotInstantiate` under `Error`, each carrying what it was signalled
  for in a public slot of its own -- `argument`, `result`, `receiver` and
  `behavior` -- and made with a class-side factory naming it (`value:`,
  `result:`, `receiver:`, `behavior:`). With no `messageText`, the
  `description` is the class's name and that slot's `printString`
  (`ArgumentError: 3`). The slot is `argument`, not `value`, so that an
  `ArgumentError` does not pass for a block, and `behavior`, not `class`,
  whose public getter would override the protected `Object>>class`. `NonBooleanReceiver` is private to
  the kernel: a program catches it as an `Error`, and `platform kernel`
  answers the other three. `CannotInstantiate` is ns-next's own, not
  Primordial Soup's, which marks a class nobody may instantiate by naming
  its primary factory `_cannotInstantiate`; it is public so that a program
  can catch it and signal it for classes of its own. `NotFound`, the
  fifth, is the collections module's ("The collections module (plan 2)"
  below).
- `Message`: `selector`, `arguments`, and `sendTo: receiver`, which sends the
  message as an ordinary *public* send (§5.5) — a program can obtain and
  build messages, so a send through one must not reach a protected or
  private method. A message
  built without `arguments:` is sent with none; a selector that is no Symbol
  or arguments that are no Array make `sendTo:` signal an `Error`. It is
  what every `doesNotUnderstand:` handler receives
  ([dispatch.md](dispatch.md), "The reified message is a `Message`").
- `Activation`: private to the kernel, like `Top` (below).

**Class sides are not inherited, so `signal` is declared on each kernel
exception class.** §3.2's metaclasses are flat
([object-model.md](object-model.md), "Class sides and metaclasses"): a
class side is its own mixin applied to `Class`, not a subclass of its
superclass's class side, so a class-side method declared on `Exception`
alone would not reach `Error`. The default `handles: anException` — "an
instance of me or of a subclass", the `inheritsMixinOf:` leaf, compared by
declaration so that a nested exception class catches its instances under
any enclosing object — is therefore on `Class`'s instance side, where every class object answers it
and a class side that declares its own `handles:` overrides it. `signal`
and `signal:` (`^self new signal`) are declared on the class side of each
of the eight: `Exception`, `Error`, `MessageNotUnderstood`, `Halt` and the
four classes that carry a subject. A program's own subclass answers
`signal:` only if its own class side declares it; otherwise it is
signalled as `MyError new signal: 'text'`, which the goldens use.
Each level of the exception's chain counts as a class of its
application's mixin and, for a declaration without a body, whose class
object is the level its last clause adds and applies that clause's mixin
([object-model.md](object-model.md), "Mixin application"), of its own
declaration too, as does a level a `<:` clause adds for such a class: so
`on: M3 do:` for `class M3 = Error <: M1.` catches `M3`'s instances and
never those of another declaration applying `M1`, with or without a body,
while `on: M1 do:` catches them all, as the classes Squeak-Newspeak's
`isKindOf:` and SOMns's class groups compare would
(`goldens/library/exception/a_body_less_exception_class_catches_its_own_declarations_instances.ns`).

**Programs name them through `platform kernel`** (decided with the user on
2026-09-25), as the corpus does — `private Exception = platform kernel
Exception.` appears 16 times, `platform kernel MessageNotUnderstood` 11 —
and as Primordial Soup's `Kernel wrapping:` facet provides them: the
platform's lazy `kernel` slot holds a value of `PlatformFactory.ns`'s
`KernelNamespace`, whose methods answer `Exception`, `Error`,
`MessageNotUnderstood`, `Halt`, `Message`, `ArgumentError`, `CannotReturn`,
`CannotInstantiate`, `Closure`, `Future`, `Proxy`, `Collection`,
`StringBuilder`, `DataView`, `WeakArray`, `Ephemeron` and `WeakMap` from
the kernel instance the platform was handed, and whose `garbageCollect`
runs a full collection ("Weak references" above); the typed arrays are
`platform typedArrays`'. `NotFound` is answered by
`platform collections`, as Primordial Soup keeps it in its collections
module, and `NonBooleanReceiver` by nothing.
`Exception` and `Error`, the two roots, are `Object`'s accessors as well
(decided with the user on 2026-10-11), so a method may name them bare;
`platform kernel` keeps answering them, as the corpus asks.

`Closure` has `on: exceptionClass do: handler`, `ensure: aBlock` and
`ifCurtailed: aBlock`, three marked methods (below): `on:do:`'s body sets
its `handlerActive` local and answers `self value`; `ensure:`'s runs its
block after `self value` in Newspeak on the normal path, having set its
`complete` local first; `ifCurtailed:`'s answers `self value` and leaves
its block to the unwinder. A block the unwinder runs is activated directly;
an argument that is no block is sent `value` through the kernel's private
`Closure>>valueOnUnwind:`, as the normal path sends it.

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
handler's `on:do:` for the next handler out. The walk goes through one
leaf, `activationNextHandler:`, which answers the next enabled `on:do:`
activation outward and reifies no activation it passes -- as Cog's handler
search marries only the context it finds (`primitiveFindHandlerContext`,
`prior-art/opensmalltalk-vm/src/spur64.sista/cointerp.c:84085-84112`) --
so a signal allocates the same however deep below its handler it is
raised, and a frame a compiled tier ran is never turned back into
interpreted ones for being passed ([execution.md](execution.md),
"Planned: the deoptimization contract", "Observers");
`crates/ns-runtime/tests/exceptions.rs`'s
`a_handler_search_reifies_no_frame_it_passes` pins it under both lowerings.
A `^` out of a handler block is
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
`goldens/library/exception/a_handlers_exception_is_looked_for_outside_its_on_do`
and `a_resumed_handler_is_enabled_again` pin it. The candidate `on:do:` is
likewise disabled, under an `ensure:`, while its class's `handles:` runs, so
an exception the test raises — `nil handles:` for `on: nil do:` — is not
offered to that same `on:do:` again, which would nest one search per level
and walk the whole stack each time
(`goldens/library/exception/on_a_non_class_ends_the_run_with_its_handles_error`).

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
itself (`goldens/library/exception/a_message_not_understood_a_program_signals_ends_the_run`).
An attached debugger stops the program just before `defaultAction` runs
and lets it run when the program goes on: `invokeNextHandler`, having
found no handler, sends the mirror's `exceptionUnhandled:ifFail:` first,
whose leaf decides whether the debugger's exception breakpoints take
the exception and records the stop for the next poll, the push of
`defaultAction` ([debugger.md](debugger.md), "Exceptions"). It answers
true only for a `Halt` the debugger's `Halt` breakpoint stopped, which
`invokeNextHandler` then resumes with nil instead, so that `halt`
answers nil once the program goes on -- so that, under that default
breakpoint, a `Halt` subclass's `defaultAction` override is skipped, and
one whose `isResumable` answers false makes the `resume:` signal "cannot
resume a non-resumable exception"; with no debugger attached it does
nothing and answers false. Likewise `invokeHandlerAt:disabling:` reads
the handler it found, just before it runs it, through
`exceptionCaught:at:ifFail:`, which answers the `on:do:` activation's
handler and reports the catch to the caught-exception breakpoint and a
step in progress, which goes on into the handler block; the report
costs no send of its own. Neither leaf runs Newspeak code, so the
search pushes no activation it did not push before, and the outcome of
every program without a debugger is unchanged. A test resumes an
unhandled exception with `Interpreter::resume_unhandled_exceptions`, a
`#[doc(hidden)]` hook under which the `terminate…` leaf answers nil
instead of terminating, as a debugger that resumed the exception would;
the hook is served before the debugger's breakpoints, so an exception it
resumes stops nothing.

**`doesNotUnderstand:` and creating the missing method.**
`Object>>doesNotUnderstand: message` is Squeak's: it builds a
`MessageNotUnderstood` with the message, the receiver and the text "an
object does not understand #…", signals it, and when the default handler
resumed it (`reachedDefaultHandler`) re-sends the message with
`message sendTo: self`; otherwise it answers the resumption value. A
unary selector that begins with `isKindOf` never gets that far: the method
answers `false` before it makes the exception. The
default handler ends the program; under an attached debugger the program
stops before it runs, and the debugger's planned "Create Method" creates
the method and resumes the exception, which re-sends the message into it — the user's requirement for a live
system, and why `MessageNotUnderstood` is resumable although it is an
`Error`. The re-send is a *public* send, as `sendTo:` always is, so a method
the debugger creates as protected or private would not be reached by it and
the message would fail again; the debugger has to create it public, or
re-send it another way. The text is the one the retired mirror row gave, so
the goldens are unchanged. A program that sends `doesNotUnderstand:` itself
with an argument that is no `Message` fails inside the handler, on
`selector`, as Squeak does.

**The VM side.**

- **Activation objects.** An activation object is the reified frame object
  itself, of the frame format and the private kernel class `Activation`
  ([object-model.md](object-model.md), "The object layout"), which no
  program can name — no accessor on `Object`, and its shipped name is
  refused wherever an image names a class but the header of a frame record
  and a frame's class, as `Top`'s is everywhere but a frame's class. An
  activation reads another frame's temporaries and unwinds stacks, so it is
  a capability, and Newspeak hands those out only through mirrors, which the
  reflection step builds on it. The thirteen leaves (`builtins/activation.rs`,
  rows of `MIRROR`): `currentActivation` — the activation of the kernel
  method making the mirror send, reified, answered as its frame object, so
  two calls for one activation answer the same object and the coming
  `ActivationMirror` has an identity by frame already; an activation's
  `sender` — the activation it returns to, the stack frame below it or the
  suspended frame it returns to (`Heap::frame_sender_frame`;
  [execution.md](execution.md), "Suspending and resuming"), or nil at the
  bottom or once it has returned; an activation's `methodName` --
  `'Shape>>area'`, the class declaring the method it runs, named as
  `Class>>name` and `Metaclass>>name` answer, and the selector its mixin
  holds that code under, or nil for a returned activation or one running a
  block, an initializer or a clause -- for `Object>>subclassResponsibility`;
  `nextHandler`, the next activation outward whose code carries the
  `on:do:` marker and whose handler is active, reifying none it passes,
  or nil past the bottom or for a returned activation; the two the
  marker's registers answer, `handlerClass` and `handlerBlock`, and
  `handlerActive:`, which writes the flag;
  `returnValue:` and `restart`, which answer the signals the unwinder
  carries out (a returned target is §7.2's "cannotReturn: the activation has
  already returned"); `inheritsMixinOf:` for `Class>>handles:`;
  `terminate…` for `defaultAction`; and `exceptionUnhandled` and
  `exceptionCaughtAt:` (`exceptionUnhandled:ifFail:` and
  `exceptionCaught:at:ifFail:`), the search's reports to an attached
  debugger, which decide and record its exception breakpoints' stop and
  do nothing else without one -- the first a leaf machine code calls
  without exiting, the second answering the handler as `handlerBlock`
  does, an exit as it is ([debugger.md](debugger.md), "Exceptions"). `Message>>sendTo:`'s row is
  `message:sendTo:ifFail:` (`builtins/message.rs`);
  `Object>>doesNotUnderstand:` forwards to no row: it is Newspeak that
  signals a `MessageNotUnderstood`. The tracer follows the frame reference,
  so the stress modes cover it.
- **Markers.** Whether a method is `on:do:`, `ensure:` or `ifCurtailed:` is
  a property of its compiled code (`bytecode::UnwindMarker`, naming the
  registers of the method's arguments and of its `handlerActive` or
  `complete` local), recognized by selector when the kernel's `Closure` is
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
  activations' continuations go to `Mutator::abandon`, each pop resuming a
  suspended sender first (`Mutator::pop_for_unwind`). A normal return needs
  no unwinder and costs nothing; an unwind pays one marker test per popped
  activation ([execution.md](execution.md), "Unwinding"). A Primordial
  Soup-style unwind in Newspeak was weighed and rejected: the non-local
  return would still need a path of its own in the VM (Squeak's
  `aboutToReturn:through:`), two mechanisms doing one thing, and unwinding
  would cost several sends per frame on exception-heavy code.
- **An unwind target is a `FrameReference`, never an `ActivationReference`.**
  Once code runs while an unwind is in progress, a pushed activation can
  reuse a target's stack offset, and a release build does not check an
  activation's generation. A reified frame keeps its identity; whether it
  is still running, married to a stack frame or suspended into the frame,
  is `Heap::frame_is_running`, in every build, and the leaves read and
  write a running frame through the frame, whichever it is.
  `Signal::Return` and `Signal::Restart` carry the target's
  `FrameReference`.
- **Every VM error becomes a signalled `Error`.** Where a step answers
  `Signal::Error(NewspeakError)`, the unwinder (`start_signalling`) creates
  an instance of the class the error's `ErrorClass` names -- `Error`, or
  one of the four classes under it that carry a subject, the error's
  `subject` then in that class's slot -- whose `messageText` is the error's
  text and whose `originSelector` and `originReceiver` are its selector and
  receiver (`Mutator::signal_vm_error`, filling the slots by name as the
  platform's are filled), and pushes `signal` on it above the activation
  where the error arose; nothing is popped. The Rust site that raises the
  error decides its class, through the constructor it calls
  (`NewspeakError::error`, `argument_error`, `cannot_return`,
  `non_boolean_receiver`, `cannot_instantiate`), but does not build the
  exception itself: signalling runs Newspeak, which a leaf never calls
  back into, and a leaf that declines or raises has changed nothing
  ([object-model.md](object-model.md), "Where Rust sees a future"), which
  an allocation would break. The optimized lowering, the reference
  lowering and a later JIT all reach the one `signal_vm_error`.
  `NewspeakError` holds its text as a `Box<str>`, 16 bytes rather than a
  `String`'s 24, which frees the word `subject` takes, so `Signal` keeps
  the size `a_signal_fits_in_the_niche_of_its_one_unboxed_error` pins.
  So `on: Error do:`
  catches an index error from `Array>>at:`, a wrong block arity, an
  instantiation refusal or a `cannotReturn`, and an uncaught one ends the
  host's run with the same `HostError` and the same line as before. An
  `Error` cannot be resumed, so that `signal` returns only if the debugger
  resumes its default action; its continuation,
  `Continuation::VmErrorSignalled`, then ends the run with the error.
  `ArgumentError`, `CannotReturn`, `NonBooleanReceiver` and
  `CannotInstantiate` are the finer classes a site may name. The rule is
  the class Primordial Soup signals in the same situation, carrying what
  its slot holds. A leaf that refuses an argument -- an index out of
  range or no Integer, a size, a capacity, a code point, a `String` to
  concatenate or to show, a division by zero, a non-finite `asInteger`,
  `asStringFixed:`'s count, a non-block `do:`, a wrong number of block
  arguments -- signals an `ArgumentError` carrying that argument (the
  divisor for a division by zero, the bound out of range for
  `copyFrom:to:`, the block for its arity, the receiver for `asInteger`).
  A `^` whose home has returned, and a `returnValue:` or `restart` aimed
  at an activation that has -- as `return:`, `resume:` and `retry` on a
  handler whose `on:do:` has returned are -- signal a `CannotReturn`
  carrying the value returned (nil for `restart`, which returns none). A
  `whileTrue:` or `whileFalse:` whose condition answers no Boolean, inlined
  or sent, under either lowering, signals a `NonBooleanReceiver`
  carrying that answer, its selector `value`, the send whose answer was
  no Boolean ([compiler.md](compiler.md), "Control-flow inlining"). An
  inlined conditional is not such a site: a receiver that is no Boolean is
  sent the real message, and one that does not understand it signals
  `MessageNotUnderstood`, because the specification has no inlining and
  Primordial Soup's own tests call its `NonBooleanReceiver` for a
  conditional a cheat (`prior-art/primordialsoup/newspeak/KernelTests.ns:5932`),
  while a loop's behaviour is the library's to define. Every instantiation
  refusal whose text begins "cannot instantiate" signals a
  `CannotInstantiate` carrying the class, and nil for the VM mirror's
  class, so that no path to that class rests on the argument that only
  the mirror's holder can reach the refusal. What stays a plain `Error`
  is the depth limit, a future that diverged (an `Error` in Primordial
  Soup too) or is malformed, a
  superclass or mixin clause that is no class, a wrong number of
  factory or method arguments, the refusal to store a non-value in a
  value object's slot, and every `internal:` error. Until the
  kernel is installed, errors stay host errors, popped to the floor
  without running any block, as there is no `Error` class yet.
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
  or the kernel's `doesNotUnderstand:`: a frame, not a stack offset, because
  a frame is divorced on its activation's pop and never married again,
  where a later signal at the same offset would inherit an offset. It is
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

**The image format.** An `Activation` is a frame record (`OBJECT_FRAME`,
body tag 15), its header `Activation`'s shipped name, `Kernel Activation`
([image-format.md](image-format.md), "Frames are object records").
`Exception`, `Error`, `MessageNotUnderstood`, `Message` and `Halt` are named
by their shipped names like every kernel class
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

**Pinned by** the goldens under `goldens/library/exception/` (the index errors under
`goldens/library/collection/` and `goldens/library/string/`, the depth limit's under
`goldens/runtime/depth-limit/`) — a handler's value
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
answering 42, and `sendTo:` a public send; the depth limit caught twice in
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
refusals for a frame record under another class, the retired activation
body tag 9 and tag 23. No golden's recorded outcome
changed; two golden sources that read the old two-element message with
`at:` moved to `Message`'s protocol.

**Measured** 2026-09-26 against `3cbfd31`, the `master` it started from
([execution.md](execution.md), "What exceptions cost"). None of the `bench`
fixtures uses exceptions. The branch first measured slower on block activation
and deep returns -- 4.6% to 6.4% -- from two causes it located and fixed: a
`Signal` grown by 8 bytes once `Terminate` held a second `NewspeakError` in
place, and an arena the collector shrank below its next limit at every
collection. With those and an out-of-line error in the slot accessor, every
fixture and both controls are faster than the base, by 2.5% (twenty sends
deep) to 24.3% (`bench_loop.ns`). Only the `Signal` boxing repairs a cost the
branch introduced; the arena shrink and the slot accessor repair defects
`master` has too, so being faster than the base is not a property of the
exceptions branch, and its net cost against a `master` carrying the same two
fixes was not measured -- the per-commit attribution suggests about zero. With
nothing signalled, one `ensure:` and one `on:do:` cost about 145 ns an
iteration over sending `value` to the same blocks; an `Error` signalled and
caught twenty activations up costs about 6.4 µs.

### The platform and collections as core-library modules

**Built in two plans and completed by a third: the first gives lazy slots,
the table of shipped modules and the platform's boot ("The platform" above
describes what exists); the second gives `Array new: size`, the handler
search that keeps no growable collection, the mirror's module leaf and
`CollectionsFactory.ns` with a Newspeak `List` ("The collections module
(plan 2)" below); the third names the shipped world in an image by
declaration ("Shipped names in the image" below), and the shipped modules
other than the kernel and the platform's are built on their first demand
("The module table and the boot" below).** `Kernel.ns` is to hold only what
the VM must know — the classes whose instances the VM creates or on which
dispatch depends — and the rest of the library moves into modules the
runtime ships. Primordial Soup splits it this way
(`KernelForPrimordialSoup.ns` beside `Runtime.ns`, whose `Platform` is
built from modules such as `Collections.ns`), and so does SOMns
(`core-lib/Platform.ns` loading `Collections.ns`, `System.ns` and the
rest); §8.3 describes the platform as an object that "provides accessors to
various modules".

The two plans:

1. **The platform module**: §6.3.3's lazy slots, the module table,
   `Platform.ns` with `Platform`, `Transcript` and
   `KernelNamespace`, booted with `usingVmMirror:kernel:`, and a way for an
   image to name what the platform reaches. `Collections` moves too. Built,
   and since 2026-09-27 written in the factory pattern ("The platform"
   above): the module is `PlatformFactory.ns`, whose root takes
   `usingVmMirror:kernel:` and is never handed out, and the platform is its
   nested `Platform`.
2. **The collections module**: the mirror's module leaf, `Array new:`,
   `CollectionsFactory.ns` with a
   Newspeak `List`, a handler search that keeps no growable collection, and
   `OrderedCollection` removed with its leaf and `Heap::array_push`. Built
   (2026-09-28).
3. **Shipped names in the image**: `moduleInstanceNamed:` answering a
   module's one instance, and an image naming every shipped object by its
   declaration. Built (2026-10-13); since 2026-10-15 a module other than
   `Kernel` and `PlatformFactory` is built on its first demand, and the
   loader builds what an image names.

#### Lazy slots first (plan 1)

The platform's members are lazy slots (§6.3.3), so that a member is built
on its first use, not at every start: once the platform grows members for
I/O, networking, time and rendering, building all of them eagerly would
make every program pay for all of them. SOMns builds every member eagerly;
this runtime deliberately does not. The shipped module behind a member is
built on the member's first use too ("The module table and the boot"
below), so a lazy member saves its module, its facet and whatever its
initializer does. Lazy slots are also what lets an image write the platform
as a record whose empty members fill on demand, and §6.3.3 states
the intent directly: "Lazy slots themselves need never be persisted or
synchronized".

**Built.** The parser keeps the modifier (`SlotDeclaration.lazy`,
[frontend.md](frontend.md)), the slot starts `nil`, and its getter
evaluates the initializer while the slot holds `nil`, storing the result
without a setter and answering it, as §6.3.3 says. An initializer that
answers `nil`, or that does not complete — it signals and a handler
returns, or it returns non-locally — leaves the slot `nil`, so the next
read evaluates it again. `goldens/language/slot/accessor/spec_6_3_3_*` pin first use, the
`nil` answer, the abandoned initializer and one mixin's lazy slot under
two applications at different field offsets;
`goldens/language/slot/accessor/spec_6_a_lazy_slot_cannot_name_a_factory_parameter` pins the
scope rule below, and `goldens/library/value/a_values_lazy_slot_*` the two value
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
`mirror`. The resolver resolves a lazy initializer in a fresh
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
being the own `own_slot_base` of the application the activation was
entered at (`Heap::activation_slot_base`) -- for the getter its defining
class, for an initializer the level it initializes -- so no activation
carries a base of its own. Every read of a lazy slot therefore activates its
getter, filled or not. A `MethodFastPath::LazySlot` answered a filled one
without an activation, but its arm, inlined into every send site of
`vm::run`, slowed every method send; the getter gave it up
([execution.md](execution.md), "What the platform module cost").

#### The module table and the boot

**Built**, as "The platform" and "Where the library source lives, and how
modules are found" above describe, with one part of the design below not
built: the table is `interpreter/core_library.rs`'s `SHIPPED_MODULES`,
read by the host, not yet a table on the mirror, and its entries carry no
shipped flag yet, since every entry is shipped source; the flag comes with
the first user module the table holds. The table has six entries,
`PlatformFactory`, `TranscriptFactory`, `TimeFactory`,
`CollectionsFactory`, `TypedArraysFactory` and `MirrorsFactory`; the kernel is booted before
them by `interpreter/kernel.rs`. **The boot builds `Kernel` and
`PlatformFactory`; every other module is built on its first demand**
(`LAZY_SHIPPED_MODULES`), and the mirror's `moduleInstanceNamed:` answers
a module's instance once it is recorded. The design as decided:

- **One module table, on the mirror** (the decision of 2026-09-21 in "Where
  the library source lives, and how modules are found"). The host fills a
  table of name to source at bootstrap; the shipped modules —
  `PlatformFactory.ns` and the five under `platform/` — sit in
  `crates/ns-runtime/core-library/` with `Kernel.ns`, and are embedded the
  same way. **Loading is unified; trust and visibility are not:** every
  entry carries whether it is shipped core-library source, set by the host
  and never by the module, and only such an entry is resolved with
  `resolve_kernel`. The mirror leaf `moduleInstanceNamed:` (the row
  `moduleInstanceNamed:ifFail:`, whose subject is the name,
  `builtins/module.rs`) answers the one instance recorded of a shipped
  module -- `Kernel`'s included -- in the shipped world
  (`interpreter/shipped_world.rs`), the same object on every request, and
  declines a subject that is no `Symbol` or names no recorded module. Two
  more leaves build a module on its first demand, and none of the three
  runs Newspeak: `shippedModuleClassNamed:` (the row
  `shippedModuleClassNamed:ifFail:`, whose subject is the name) answers the
  root class of a lazy shipped module not yet recorded, built once from the
  module's root mixin and kept as its pending class, and declines a name
  that is no lazy shipped module or one already recorded;
  `recordShippedModule:` (the row `recordShippedModule:ifFail:`, whose
  subject is the instance the factory answered) refuses anything but an
  instance of a pending root class, checks the `vmMirror` slot of a module
  given the mirror (lock (2)), records the instance as the module's one
  instance and names it, its class and the classes it already built
  (`record_pending`), and answers it. `PlatformFactory`'s root keeps each
  module's instance in a lazy slot, `collectionsModule` and its four
  siblings. One instance per module is a guarantee of the VM, not of the
  lazy slot: `ShippedWorld::add_module_instance` refuses a second, and only
  an instance of the pending class is recorded. A module an image names is
  built by the loader before the graph is decoded, by the same functions
  (`Interpreter::build_shipped_names`; "Shipped names in the image" below).
  The same table later holds user modules and falls back to the file
  system. The loader a program is later handed through the platform sees
  user modules only: a program reaches the standard modules as the
  instances the platform built (`platform collections`), never as module
  definitions, whose code was compiled for a trusted context.
- **The boot.** After `install_kernel` the host records the kernel
  instance, builds `PlatformFactory` with `usingVmMirror: mirror kernel:
  kernel`, an ordinary run, as the kernel's instantiation is, so the stress
  modes see it; builds every class the two roots declare
  (`complete_shipped_world`); and sends `PlatformFactory`'s instance
  `build`, keeping the answer as a root ("The platform" above). A lazy
  module is instantiated on its first demand with its factory's arguments
  (`ShippedFactory`): `TypedArraysFactory` and `MirrorsFactory` with
  `usingVmMirror:kernel:`,
  `TranscriptFactory` and `TimeFactory` with `usingVmMirror:`,
  `CollectionsFactory` with `usingKernel:`, since it reaches no leaf and so
  is given no mirror. `Mutator::platform_value` reads that root instead of
  building the platform in Rust. The second parameter is Primordial Soup's
  (`Platform internalKernel: ik`), chosen over a mirror leaf answering the
  kernel (SOMns's `vmMirror kernelObject: nil`): handing over an object
  needs no Rust, and the dependency stands in the factory's signature.
  `public` gives no module a way to name the kernel's classes (§8.3: a
  module sees only what is lexically around it, what it inherits and what
  its factory is handed), which is why the platform module needs the kernel
  instance handed to it at all.
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
  and declare no slots. In plan 1 `Collections>>List` answers the kernel's
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
  `KernelNamespace`.
- **One golden changed:** `goldens/library/platform/the_platform_and_its_parts_are_values`
  asked whether a second platform made from the first's class equals it,
  which `Platform`'s
  `usingVmMirror:kernel:` factory no longer answered; that line was
  replaced, and the other checks kept their recorded output. The factory
  pattern gave the platform's class `new` back; since the platform hands
  out none of its module's classes, those checks are the host's
  (`tests/platform.rs`, `the_platform_and_its_parts_are_values`).
- **The host takes `PlatformFactory`'s entry from the table itself**, and
  a lazy module's when an image names it; `PlatformFactory.ns` reaches every
  other module's instance through `moduleInstanceNamed:`, building it first
  through `shippedModuleClassNamed:` and `recordShippedModule:`.

#### Shipped names in the image

**Built** (`image/shipped_names.rs`, `interpreter/shipped_world.rs`;
[image-format.md](image-format.md), "Shipped names"; decided with the user
on 2026-10-12, built 2026-10-13). An artifact names every object the binary
owns -- each shipped module's instance, each class its root declares, their
metaclasses and the levels their `<:` clauses add -- by its module, its
declaration and its kind, resolved by table lookup against the shipped
world before the graph is decoded and without a send; what the boot did not
build of what an image names, the loader builds first, by the binary's own
code. The platform, its facets, a `List`, a `Stopwatch` are ordinary
records of their shipped classes; a second platform a host makes is one
like the first. It replaced plan 1's platform paths, which named a shipped
object by the public members sent from the platform to reach it: the loader
had to send them before it decoded the graph, and a path reached only what
the platform hands out, so a plain kernel class, a private one or the level
a kernel mixin adds had none.

**What an author of a shipped module keeps.** The rule of what may be named
where is computed from the declarations when a module is recorded, so a new
shipped module or class needs no tag and no table, but its shape decides
what an image can carry:

- **Declare every shipped class directly in its module's root.** A name is
  (module, declaration), so only the root's own nested classes have one; a
  class nested deeper has no name, and the encoder refuses a graph that
  reaches it ("compiled from shipped source, and no shipped name names
  it").
- **A class is a value only if it is public or a public class accessor of
  its module answers it**, a method the compiler recognizes in shipped
  source other than the kernel as exactly `^outer <Root> <Name>`
  (`MixinMethod::class_accessor`). A class whose instances the VM signals
  into a program, the private `NonBooleanReceiver`, is no exception: a
  program that catches one holds the instance, and `class` is protected.
  Only such a class, and its metaclass, may
  stand in a slot, an element or a register, or be a program's superclass
  or the mixin a program's `<:` clause applies. A private class without
  such an accessor -- a capability such as `Activation`, a future `File` --
  is a record's class at most: an image may carry its instances, never its
  class object.
- **A module's root and its instance stay out of every value.** The root
  class, its metaclass and its levels may stand only as a frame's class, and
  the instance only as a frame record's receiver or register, and a started
  image, whose frames a program's closures read, names no module instance
  at all: it holds what the factory was handed, the mirror or the kernel.
- **An instance of a shipped class that is no builtin is a plain instance
  with slots**, held on load to the slot count the loading binary lays the
  class out with. An object holding external state -- an open file or
  socket, once I/O exists -- cannot be written as data at all, whatever
  names its class; that needs its own decision when the first such module
  arrives.
- **A build draws no identity hash.** Every class's superclass clause and
  every eager slot of a root runs when its module or class is built, at
  boot, on first demand or in the loader; one that hashes something -- a
  hashed collection built eagerly, say -- changes every hash a run draws.
- **A change to shipped source refuses older images.** The header's
  fingerprint covers every shipped module's source, so an image another
  binary wrote is refused before anything is read; images are disposable.
- **A module costs its decoding and instantiation on its first demand**,
  and the kernel and `PlatformFactory` at every start:
  `CollectionsFactory`'s first demand 0.31 ms and the kernel's decode 0.88
  ms on an optimized boot from the embedded shipped code, and the whole
  compilation on a boot from source ([execution.md](execution.md), "What
  the precompiled shipped code moved"; "The embedded shipped code" above).
  A module the boot builds costs every process, so a new shipped module is
  built on first demand unless the boot itself needs it.

**Rejected, and why the reasons stand.** **Serializing the modules** (as
Primordial Soup's `Serialization.ns` writes a fresh runtime into the
snapshot) would put code compiled with `SendMirror`, which has no wire tag
on purpose, into an artifact, and make every image as trusted as the
binary. **Letting the decoder build shipped classes itself**, by
restricting shipped modules' superclass clauses to bare names as
`Kernel.ns`'s are, would constrain the library and teach the decoder class
construction; the boot, a first demand or the loader builds them instead,
running only the binary's own code. **Patching placeholders after
decoding** would rewrite slots of decoded values afterwards. **Rebuilding a
shipped class as a record** would give a twin, where Rust compares class
objects by identity. **Making every shipped class a builtin** was rejected
too: a builtin is a class Rust needs a handle on, not a class the image
needs a name for.

#### The collections module (plan 2)

- **`Array new: size`**, a leaf answering an array of `size` `nil`s,
  **built**: a class-side forwarder on `Array` (`Kernel.ns`) reaches the
  mirror row `arrayClass:new:ifFail:`, reported selector `new:`
  (`CollectionPrimitive::Allocate`). `Array new` stays, meaning `new: 0`. A
  negative size or a non-`Integer` size is an ordinary `Error`
  ("`new:` requires a size of at least 0, got …" / "… requires an Integer
  size, got …"), and a size that cannot be allocated is one too ("…
  cannot allocate an Array of … elements") — found by `Heap::array_fits`
  before anything is allocated, rather than ending in an abort, since the
  size is the program's and an abort is not catchable: a length below the
  large-object threshold is allocated young, where eden is committed, and
  a larger one is probed by `Heap::fields_fit`, as a `ByteArray`'s size
  is ([OPEN-WORK.md](../OPEN-WORK.md) says what that probe does not yet
  cover). The array is allocated nil-filled with nothing built beside it
  ([memory.md](memory.md), "Arrays without a `Vec`"). A `List` over a fixed array needs a sized array; this is what
  it grows into.
- **`CollectionsFactory.ns`, built**, in the factory pattern ("The
  platform" above): `class CollectionsFactory usingKernel: kernel = Value`,
  a root that keeps the kernel's `ArgumentError` and `Collection` in
  private slots and is never handed out, with the private nested classes a
  program reaches through the platform -- the `Collections` facet, `List`,
  `Map`, `IdentityMap`, `Set`, `NotFound` and `ConcurrentModification` --
  as SOMns's and Primordial Soup's collections modules are values; it needs
  no mirror. `PlatformFactory`'s lazy `collectionsModule` builds it with
  the kernel instance, once, on its first demand, and reaches that instance
  (`vmMirror moduleInstanceNamed: #CollectionsFactory ifFail: [...]`)
  afterwards, so every platform shares one module instance and one class of
  each; the platform's lazy `collections` slot answers that instance's
  `build`, its one public member, and plan 1's `Collections` class has left
  `PlatformFactory.ns`. The facet hands out none of its own class, as the
  platform none of its module's: an image names the facet by its
  declaration, `CollectionsFactory Collections`, as a record's class
  ("Shipped names in the image" above), and no program makes a second one
  (decided with the user on 2026-10-13). `List` is a private class of the
  factory, handed out by the facet's class accessor `public List = ( ^outer
  CollectionsFactory List )`; a program may still subclass it
  (`goldens/library/collection/a_user_subclass_of_list_inherits_its_protocol`).
- **`List`, built,** is a kernel `Collection` written here. Its elements
  live in `data` from `firstIndex` on, `size_` of them, in an `Array` of
  three to start, so that `removeFirst` moves nothing and a queue of
  `add:` and `removeFirst` runs in amortized constant time (DeltaBlue at
  12000 took 52 s with a shifting `removeFirst`, against 0.9 s; decided
  with the user on 2026-10-01). `add:` finds the array full at its end and
  either slides the elements to the front of the same array, when at least
  as many slots lie empty before them as they fill, or copies them into an
  `Array` half as large again plus four; `addFirst:` finds `firstIndex` at
  1 and makes room at the front the same way, by sliding right by half the
  empty slots after the elements, or by copying into a larger `Array` with
  the new room in front. The primary factory is `new: capacity`, so a
  program may write `List new: 15` as a superclass clause; the class
  side's `new` is `new: 3`, and `withAll:` copies
  any collection.
  Its protocol: `size`, `isEmpty`, `at:`, `at:put:`, `first`, `last`,
  `add:`, `addLast:`, `addFirst:`, `addAll:` (a list given itself is read
  as a copy first), `do:`, `keysAndValuesDo:`, `anySatisfy:`, `indexOf:`,
  `remove:` (a `NotFound` for an absent element), `remove:ifAbsent:`,
  `removeFirst`, `removeLast`, `removeAll` (keeping the capacity),
  `removeAllSuchThat:`, `sort:` (`Array>>sort:`'s stable merge sort, whose block
  answers whether its first argument may precede its second),
  `asArray`, `,` and `copyFrom:to:`; the rest is `Collection`'s, written
  over `do:`, and `collect:`, `select:` and `reject:` answer Lists through
  the hook `newForCollectUsingAdd:`
  (`goldens/library/collection/a_list_has_the_whole_protocol`). An empty
  `List`'s `removeFirst` and `removeLast` are `Error`s. Its representation
  is private so that it can change without a subclass noticing; only
  `size_` stays protected, as a subclass's view of the size
  (`goldens/library/collection/a_user_subclass_of_list_inherits_its_protocol`;
  decided with the user on 2026-10-01).
  **Changing a `List` while it is iterated signals.** Every change of the
  number of elements -- `add:`, `addFirst:`, a removal, `removeAll`, the
  compaction `removeAllSuchThat:` ends in -- adds one to the private
  `modifications`; `do:`, `keysAndValuesDo:`, `anySatisfy:` and
  `removeAllSuchThat:` compare it with the count they started at after
  every block that does not end the iteration, the last included, and
  signal a `ConcurrentModification`
  (a class of the collections module under the kernel's `Error`, public
  as `platform collections ConcurrentModification`, carrying the
  `collection`, messageText "the
  collection was modified while it was being iterated"), raised in one
  place, the module's `signalModificationOf:`. A block that ends the
  iteration by returning -- the deciding one of `anySatisfy:`,
  `allSatisfy:`, `noneSatisfy:` or `detect:` -- leaves before the check,
  so a change it makes is not reported; no element is skipped or visited
  twice by it. Storing at an existing
  index and sorting change no count: a `do:` re-reads each element, so a
  store inside its block is seen. `sort:` sorts a copy of the elements
  and writes it back; when its block adds or removes an element, it
  signals the same `ConcurrentModification` once the copy is sorted and
  writes nothing back, so the list stays as the block left it, since the
  copy would bring back a removed element or drop an added one. A store
  its block makes at an existing index changes no count and is
  overwritten when the copy is written back. Without
  the check a change inside `do:`
  would visit an element twice, skip one or end early, silently (decided
  with the user on 2026-10-10;
  `goldens/library/collection/a_list_changed_while_iterated_signals`). An
  index outside `1..size` -- whatever capacity the array has spare -- or
  one that is no `Integer` is an `ArgumentError` carrying the index, with
  `Array>>at:`'s text, "index 3 out of range: valid indices are 1..2
  (size 2)", so a program sees one error from either
  (`goldens/library/collection/a_list_index_error_is_caught_as_an_error`).
- **`NotFound`, built.** The collections module's own class, an `Error`
  carrying the missing key or element in its slot `element`, reached as
  `platform collections NotFound` and built by `NotFound element: key`;
  its `description` is `messageText`, or "NotFound: " and the element's
  `printString` when there is none. `Map>>at:` and `removeKey:` signal it
  with 'key not found: …', `List>>remove:`, `Set>>remove:` and
  `removeAll:` with 'element not found: …'
  (`goldens/library/collection/a_missing_key_signals_not_found`). The
  errors that have no such class stay `Error`s with texts of their own: an
  empty `List`'s `removeFirst` and `removeLast`.
- **`Map` under `Collection`, with removal, built.** Keys are found by
  their `hash`, compared with `=` and kept in insertion order. A `Map`
  is a `Collection` whose elements are its values: `do:`, `includes:`,
  `inject:into:` and the rest visit them, and `collect:`, `select:` and
  `reject:` answer Maps of the same keys, built by the protected
  `newForCollectUsingAdd:`. Its keys and values lie side by side in
  `data_`, in the order the keys were first stored, and `hashtable_` is an
  open-addressing table of their indices in `data_`, 0 marking an empty
  bucket, of `(capacity + 1) * 4 // 3` buckets for room for `capacity + 1`
  entries: never fewer buckets than `data_` has entries, and `insert:`
  rebuilds the map the moment `data_` is full, so no probe runs on a full
  table and every probe ends at an empty bucket. `hashtable_ ::= rehash` is a
  `lazy` slot (§6.3.3), so the table is rebuilt from `data_` whenever it
  is `nil`; that an image writes a lazy slot's value, where §6.3.3 intends
  it never be persisted, is [OPEN-WORK.md](../OPEN-WORK.md)'s "Whether an
  image carries a lazy slot's value". A lookup answers the key's index in
  `data_`, so that no operation reads the table twice. `removeKey:`
  overwrites the key in `data_` with `data_` itself, and the key's bucket
  stays in the probe chain as a tombstone that lookups pass. The public
  protocol never hands the array out (`keys` and `values` answer new
  Arrays), so no key is ever it; a program's subclass reaches it through
  the protected `dataForScan` and probes, and one that stores it as a key
  or hands it on breaks only its own map. The mark costs no slot and no
  allocation, and every test has it as the receiver of `==`, so a key's
  own `==` is never sent it. Growth, when `data_` is full, copies only the
  live entries, in order, into new arrays, which drops the marks and the
  tombstones together; so a key stored again after its removal comes last
  in the order, and a long run of removals and insertions never fills the
  table with tombstones. `removeAll` keeps the capacity, as the Are We
  Fast Yet `Dictionary` does, so that a map emptied and refilled over and
  over, as a loop search's, allocates no new table. Adding or removing a key counts as a
  modification, and an iteration signals `ConcurrentModification` after
  the block that did it; storing under an existing key does not
  (`goldens/library/collection/a_map_changed_while_iterated_signals`). The
  representation -- `size_`, `used_`, `data_`, `hashtable_`,
  `modifications` and the helpers -- is private, but for the three hooks a
  subclass changes the comparison through: the probes `scanFor:in:`, which
  answers a key's index in `data_` or the negated index of the empty
  bucket it belongs in, and `emptyBucketFor:in:`, which a rebuild fills
  the table through, and the accessor `dataForScan`, which answers
  `data_`. `Map`'s own probes send `hash` and `=` inline rather than
  through a hook of their own, since a self-send per probe step costs a
  send and an activation on every lookup. The tables are filled by
  `whileTrue:` loops, which the compiler inlines, rather than by `to:do:`.
- **`IdentityMap`, built,** is a subclass of `Map` that finds keys by
  `identityHash` and compares them with `==`, and inherits everything else
  -- insertion order, removal, the modification count. It overrides three
  protected selectors of `Map`: the two probes, `scanFor:in:` and
  `emptyBucketFor:in:`, written out again with `identityHash` and `==` and
  reading the data array through the inherited `dataForScan`, the probe
  key the receiver of `==`, sent each stored key past the removed marks;
  and `newForCollectUsingAdd:`, so that `collect:`, `select:` and
  `reject:` answer IdentityMaps. It adds three public selectors,
  `at:putReplace:`, `at:ifAbsentPutVal:` and `atOrItself:`, and
  `isKindOfIdentityMap`; its `at:put:` is `Map`'s, which replaces the value
  of a present key. A value as key (§3.1.1) behaves as in a `Map`, since a
  value's `==` is `=` and its `identityHash` its `hash`. It is a subclass
  of `Map`, not a kernel class of its own beside it, because the two differ
  in how a key is found and compared and by those three selectors, not in
  representation, and nothing in the kernel needs one: the atomic install,
  which maps old objects to new ones and must not run a program's `=`,
  does so on the Rust side, inside its one leaf. A class side is not inherited, so
  `IdentityMap` declares its own `new` and `withAll:`
  (`goldens/library/collection/an_identity_map_compares_keys_by_identity`).
- **`Set` under `Collection`, with removal, built; the collections module
  holds no adapted code.** A `Set` finds its elements by their `hash` and
  compares them with `=`, and keeps `Map`'s representation with
  elements for key-value pairs -- the elements in `data_` in adding order,
  `hashtable_` of their indices, `data_` itself as the removed mark
  overwriting a removed element and leaving a tombstone in the probe chain
  -- and `Map`'s modification count. `add:` of an element `=` to a held one
  replaces it in place, which keeps its place and is no modification;
  `addAll:`, `remove:` (a `NotFound` carrying the element, 'element not
  found: …'), `remove:ifAbsent:`, `removeAll:`, `removeAllSuchThat:` (the
  predicate sees every element once, before anything is removed),
  `removeAll` (keeping the capacity), `includes:`, `isEmpty` and `asArray`
  are written over that; `collect:`, `select:` and `reject:` answer Sets
  through `newForCollectUsingAdd:`, so equal results of `collect:` merge.
  A set given itself to `addAll:` or `removeAll:` is read as a copy first.
  Adding or removing an element during `do:` signals
  `ConcurrentModification` after the block that did it
  (`goldens/library/collection/a_set_has_the_whole_protocol`,
  `a_set_changed_while_iterated_signals`). With `List` and `Map` written
  here, `CollectionsFactory.ns` has no licence header and
  `THIRD-PARTY-NOTICES.md` no entry for it; the module is written here, to
  Primordial Soup's API.
- **The handler search keeps no `OrderedCollection`, built.** `invokeNextHandler`
  counts the enabled `on:do:` activations `activationNextHandler:` answers, and
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
  factories), `BuiltinClass::OrderedCollection`,
  the `add:` leaf and `Heap::array_push`. Every `Array` is
  fixed-size once allocated -- nothing grows an object in place
  ([memory.md](memory.md)).
- **Goldens:** those that wrote `OrderedCollection new` moved to
  `platform collections List new` with their recorded outcomes unchanged;
  `goldens/language/class/nesting/nested_override` starts its slot at the module's `List`,
  unchanged in output since every ordinary object prints `an Object`;
  `goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods` lost
  `MyOrderedCollection`, and one `true an Object` line with it, because
  `List` is no builtin; `the_platform_and_its_parts_are_values`'s
  `c List == OrderedCollection` was replaced by a check that a second
  platform's `List` is the same class. New goldens pin `Array new:`, and
  `goldens/library/collection/` pins `List`'s protocol, its growth, its index
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
`vmMirror` and needs none. The on-demand rule is not its: it carries
Primordial Soup's collections API ("The on-demand rule" above).

Done with the typed arrays module (2026-10-13): `TypedArraysFactory.ns`
joins them likewise, kept by `PlatformFactory`'s private lazy
`typedArraysModule`, with the difference that it holds the mirror, and so
the bootstrap's check of the mirror slot covers it as it covers
`PlatformFactory`, `TranscriptFactory` and `TimeFactory`. The on-demand rule
is not its either.

Done with road step 5a (2026-10-18): `MirrorsFactory.ns` joins them the
same way, kept by `PlatformFactory`'s private lazy `mirrorsModule`, holding
the mirror under the same check, and outside the on-demand rule too
([liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors that
read").

#### What is measured

The first `platform collections` now resolves and compiles one more
module, and `List>>add:` is Newspeak where
`OrderedCollection>>add:` was a leaf. Each plan measures, release build and
pinned protocol ([execution.md](execution.md)), the start-up of a module
that answers at once, and the four `benchmarks/micro/` fixtures; plan 2 also
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
**A start that answers at once was level too** (`816367e`, +0.03 ms, +0.2%,
the head's build means among `6d9974b`'s): `interpreter/core_library.rs`
resolved each shipped module on its first request, so a program that never
touched `platform collections` never parsed or resolved
`CollectionsFactory.ns`. When the boot still resolved the whole table
(`ee4bdc8`), that start cost 0.11 ms (+0.8%) more, every build of it above
every head build. A start that built one `List` cost about 0.28 ms
(+2.1%) more than `6d9974b`'s, whose `List` was a builtin class: resolving,
compiling and instantiating `CollectionsFactory.ns`, which plan 2's lazy
`collectionsModule` deferred to the first `platform collections`.

**Shipped names in the image, measured 2026-10-12**
([execution.md](execution.md), "What the collections module cost"): the
boot then built every shipped module and every class its root declares, so
every start paid for `CollectionsFactory.ns`, which had grown to 1,710
lines: about 3.7 ms, a hello program's median start going from about 12.3
ms to about 16.0 ms on the measuring workstation. Building the declared
classes and naming the world adds nothing measurable beside it. The
kernel's `ByteArray`, `StringBuilder` and views added about 4.6 ms more,
measured at `1906a81d`: `Kernel.ns` grew from 4,089 lines to 6,586, and the
median start from about 15.4 ms to about 20.1 ms. The embedded shipped code
took it back: an optimized boot decodes the six modules' compiled code, and
a hello program's boot went from 13.6 ms to 1.6 ms; building every module
but the kernel and `PlatformFactory` on its first demand took it to 1.22
ms, and a first `platform collections` costs 0.31 ms
([execution.md](execution.md), "What the precompiled shipped code moved";
"The embedded shipped code" above).

**The collections written here, measured 2026-10-10** (sub-project 3 of
"The kernel completed to Primordial Soup's core API" below), the
Are We Fast Yet benchmarks, release builds, three builds by three rounds a
side against `eeaa1d90`: Havlak is 6.3% slower (3,491 ms to 3,711 ms) and
DeltaBlue 4.7% (670.5 ms to 702.3 ms), the other fourteen within noise,
-1.7% to +2.3%; Havlak was 23.4% slower before `Map`'s probes sent `hash`
and `=` inline, the tables were filled by `whileTrue:` and the data array
became the removed mark. The remainder is the read of a private slot per
element, a getter send in this VM, and the modification check after every
block, which is all of DeltaBlue's loss; it was accepted with the user on
2026-10-10, and the ways out are [OPEN-WORK.md](../OPEN-WORK.md)'s "The
collections' remaining cost". The Are We Fast Yet benchmarks have since
taken the SOM collection library of their own
(`benchmarks/are-we-fast-yet/newspeak/Som.ns`), as every Are We Fast Yet
port does, so none of them measures these collections any longer.

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
`super …`, which reaches the builtin's superclass; a selector it does not
have then ends in "the `#doesNotUnderstand:` method defined for S" (§5.10)
— `Object`'s, which S, the superclass of the class declaring the `super`
send, inherits — and not in a handler the subclass declares. Before the move a declining native passed
its level over and the *receiver's* own lookup continued, which reached that
handler.

It follows from the explicit failure block, and nothing short of the rejected
implicit pass-over could keep the old answer: Newspeak has no send that
continues a public lookup above a given level. No golden had pinned the old
behaviour.
`goldens/library/builtin-subclass/a_builtin_subclass_handler_is_passed_by_as_section_5_10_says`
pins the new one: `Handled = Array new`, declaring `doesNotUnderstand:`,
answers `Handled new at: 1` with `Object`'s error, not with `'handled here'`.

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
maps directly onto the leaf the kernel's `Closure>>value` forwards to.

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

**The accessors are the names a program could reach before, `Fraction`,
`Exception`, `Error` and `ByteArray`**: `Object`, `Value`, `Boolean`,
`True`, `False`, `Number`, `Integer`, `Float`, `Fraction`, `String`,
`Symbol`, `UndefinedObject`, `Array`, `ByteArray`, `Closure`, `Exception`
and `Error`. `ByteArray` joined them with sub-project 5, as §3.2 names it
beside `Array` ("`ByteArray`, `StringBuilder` and the views" above);
`StringBuilder` and `DataView` did not, and a program names them through
`platform kernel`; the typed arrays did not either, and a program names
them through `platform typedArrays`; nor did `WeakArray`, `Ephemeron` and
`WeakMap`, which a program names through `platform kernel` ("Weak
references" above). `Fraction`
joined them with the numeric tower (decided with the user on 2026-10-11):
it is one of the numbers, and a program that names `Float` bare names
`Fraction` the same way. `Exception` and `Error` joined them the same day,
also decided with the user: they are the roots a program signals and
catches most, they reach no host, and both Newspeak kernels put a root
there (Primordial Soup's `Object` has `Exception`, the Squeak kernel's
`Error`).
`OrderedCollection` was among them until it was removed (2026-09-28), when
`platform collections List` replaced it. `Top` and the VM mirror's class get none:
§3.2 lets only `Object` name `Top`, as its superclass, and the mirror is a
capability; `Platform` and its members' classes are no kernel classes at
all, and a program is handed the platform through its factory.
Nor do the other classes the exceptions and the futures added: a program
names `MessageNotUnderstood`, `Halt`, `Message`, `ArgumentError`,
`CannotReturn`, `CannotInstantiate`, `Future`, `Proxy` and `Collection`
through `platform kernel`, which answers `Exception` and `Error` too, and `Activation`, `NonBooleanReceiver` and
`FutureImplementation` not at all ("Exceptions" above, "The pull future"
below).
Each is an ordinary protected method of `Kernel.ns`'s `Object`,
`protected String = ( ^outer Kernel String )`, whose body sends the kernel
instance its nested-class accessor; protected, so that a bare name, an
implicit-receiver send, reaches it and `other String` does not ("`class`
and the class accessors are protected" below). The `outer Kernel` is required, not
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
([image-format.md](image-format.md)). A class reference a record carries
names a builtin by its shipped name ([image-format.md](image-format.md),
"Shipped names").

**The mirror's capability lock moved with it.** The lock that no program can
name the mirror's class is now that `Object` declares no accessor for it or
for `Top`, pinned by the kernel test above ("The VM mirror is the whole Rust
boundary" above; `CLAUDE.md`'s list of the six locks).

**What pins it.** An outcome could change only where a program shadows a
builtin name by inheritance: the constant won before, the inherited member
wins now, as §2.3.1 says. No existing golden and none of the runnable corpus
modules had that shape, and no recorded outcome moved. New goldens pin a
module that declares its own `String`
(`goldens/language/builtin-names/a_module_that_declares_string_uses_its_own.ns`),
an inherited member named like a builtin
(`goldens/language/builtin-names/an_inherited_member_named_like_a_builtin_wins.ns`),
an enclosing member named like a builtin as a superclass
(`goldens/language/builtin-names/an_enclosing_member_named_like_a_builtin_is_the_superclass.ns`),
a class-side `Array`
(`goldens/language/class/class-side/a_class_side_method_names_a_builtin.ns`),
`self Top` and the mirror's class name ending in `doesNotUnderstand:`
(`goldens/language/builtin-names/top_is_not_a_name.ns`,
`goldens/language/builtin-names/the_mirrors_class_is_not_a_name.ns`), and the
super-prefixed clause
(`goldens/language/class/nesting/a_super_prefixed_clause_starts_above_the_declaring_class.ns`).
`crates/ns-runtime/tests/object_class_accessors.rs` pins the recognizer and
the answer under both lowerings.

**Measured** against `909bf40` by the protocol with `-C metadata` layouts
([execution.md](execution.md), "What the value conditions cost"), start-up
included ([execution.md](execution.md), "What the class accessors cost"). The
four fixtures under `benchmarks/micro/` name no builtin in their loops and are
level, and start-up is level. The throwaway `bench_builtin.ns`, whose loop
names `Array` and `OrderedCollection` every iteration, is 1.5% slower, about
1.4 ns per accessor send against the constant load it replaces. `vm::run`
grew from 6,507 to 6,626 instructions and is still one function.

### `class` and the class accessors are protected

Decided with the user on 2026-10-13 and built the same day, before
sub-project 5
([plan](../superpowers/plans/2026-10-13-1158-protected-class.md)).
`Object>>class` and every class accessor `Object` declares ("The predefined
classes are accessors on `Object`" above) are `protected`, so that an
instance's authority is separated from its class's.

**Why.** Were they public, whoever holds an object would hold its class:
every public message of its class side, the factory included, mints new
instances that run in the same enclosing object with its capabilities (a
module that hands out one `File path:` would hand out every file); `class
Sub = (anObject class) ()` would subclass it and reach its protected
members in that environment; and public class-side state would be shared
with every holder. The specification says nothing on `class`, but §8.2
makes reflection a capability held through mirrors, and the
object-capability argument of `newspeak.pdf` and `newspeak-modules.pdf`
applies. Both Newspeak kernels make it protected:
`prior-art/newspeak/squeak/KernelForSqueak.ns`, line 1193, and Primordial
Soup's `KernelForPrimordialSoup.ns`, "to separate the authority of an
instance from the authority of its class", its class accessors on `Object`
too. SOMns's root classes declare no `class`.

**What a program sees.** `self class`, `super class` and a bare name such
as `Integer` are what they were: a self send, a super send and an
implicit-receiver send, all of which reach a protected member, from a
subclass, a block, a nested class and the class side alike (on the class
side `self class` answers the metaclass). `other class` and `other
Integer` reach `doesNotUnderstand:`, whose text is the one for any
selector nothing declares, "an object does not understand #class"
(decided with the user on 2026-10-13): §5.5's lookupPublic finds no
member there, so from outside there is none, and a text naming the
member protected would tell every sender, through `messageText`, which
protected members an object has -- the structure protected keeps from it.
No program reaches `Behavior`,
`Class` or `Metaclass`, a shipped class's metaclass, or the identity of
another object's class, and none can tell an `Integer` from a
`LargeInteger`: those were observable only through `class`. A program asks
another object's kind through the public protocol: `isKindOfInteger` and
its siblings, `printString`, the class a handler catches, and a class-side
method of its own answering `self class` for one level of metaclass.
`interpreter/kernel.rs`'s
`objects_class_accessors_are_exactly_the_nameable_builtins` holds every
accessor protected.

**The core library** sent `class` to another object in three places, each
rewritten without widening anything: `Value`'s `valueEquals:` compares
`self class` with `vmMirror objectClass: other ifFail: […]`, since the
kernel holds the mirror, and `CollectionsFactory.ns`'s two index checks ask
`index isKindOfInteger` instead of `index class == Integer`, Primordial
Soup's selector, which needs no mirror. `PlatformFactory.ns` sends neither.
`Proxy` overrides none of them: an outside send already reaches its
`doesNotUnderstand:`, and a bare `Integer` or `self class` inside a proxy
keeps the accessor's direct answer (`Kernel.ns`, `Proxy`'s comment;
`goldens/library/object/a_proxy_forwards_an_outside_class_send_to_its_target.ns`
pins the outside send).

**A project's module may be named like a class accessor.** The manifest
answers no protected member to a send from outside, so a module named
`Array` is reached through it, `manifest Array`, as any other module
(`crates/ns-cli/tests/run.rs`,
`a_module_named_like_a_class_accessor_is_reached_through_the_manifest`);
only a module named after a public member of every object, `yourself`
say, is refused ([image-format.md](image-format.md), "Packaging").

**The image.** No program reaches the private `NonBooleanReceiver` as a
value, so the shipped world has no clause for a class whose instances the
VM signals: a caught instance stays an ordinary record of that class,
since a record's class may be any shipped class, and the class itself is
refused as a value like any private class without a public accessor
([image-format.md](image-format.md), "Shipped names";
`crates/ns-runtime/tests/image.rs`,
`a_caught_non_boolean_receiver_round_trips_through_a_package` and
`non_boolean_receiver_as_a_value_is_refused`). The metaclasses and
`Behavior`, `Class` and `Metaclass` stay values: they carry no authority,
and a host can hand a program any of them -- through
`Mutator::builtin_class` or `Mutator::class_of`, as
`crates/ns-runtime/tests/representation.rs` reaches `Metaclass` -- which
a narrower rule would refuse to build.
`accessed_classes` counts public accessors only, and `Object`'s answer
public kernel classes, which are values anyway, so the rest of the rule
did not move.

**The tests kept what they check.** About 44 goldens and 33 Rust tests
sent `class` to another object. What a program can still observe, a golden
checks through the public protocol: `isKindOfInteger` and its siblings for
a result's kind, `printString` for a collection's, the class a handler
catches (`on: kernel ArgumentError do:`) or `description` for an error's,
and a class-side `metaclass` answering `self class` for one level of
metaclass. What only the representation shows -- the boundary between
`Integer` and `LargeInteger`, chains of metaclasses, that `Metaclass`
refuses instantiation, the identity of class objects -- left the goldens
and is checked by `crates/ns-runtime/tests/representation.rs`, which asks
the host for the class (`Mutator::class_of`), bound by no capability rule;
two goldens moved there whole, as
`a_class_object_cannot_be_made_by_instantiation` and
`a_subclass_of_large_integer_is_no_number`. Each changed golden's output is
the old one with exactly the moved lines gone or translated. New goldens
pin the rule: an outside `class` send and an outside class accessor send
are not understood
(`goldens/language/send/an_outside_class_send_is_not_understood.ns`,
`goldens/language/send/an_outside_class_accessor_send_is_not_understood.ns`),
a bare class name answers its class everywhere
(`goldens/language/send/a_bare_class_name_still_answers_its_class_everywhere.ns`),
`self class` on the class side is the metaclass
(`goldens/language/class/class-side/self_class_on_the_class_side_is_the_metaclass.ns`),
and a proxy forwards an outside `class` send to a target that does not
understand it either
(`goldens/library/object/a_proxy_forwards_an_outside_class_send_to_its_target.ns`).
The mirrors of road step 5a can check most of it from a golden again --
a result's class by `(ObjectMirror reflecting: x) getClass name`, a chain
of metaclasses by `isMeta` and `name`, the identity of class objects by a
class mirror's `=` -- but not that `Metaclass` refuses instantiation, since
a mirror reached by navigation does not hand `Metaclass` out; those
goldens are not written yet ([OPEN-WORK.md](../OPEN-WORK.md)).

### The pull future

`Kernel.ns`'s public `Proxy`, after Primordial Soup's, overrides every
public member of `Object` as protected, so a message from outside reaches
`doesNotUnderstand:`: a program's subclass that implements only
`doesNotUnderstand:` forwards every message, `Object`'s included, and a
bare `Proxy` understands none of them. The private `FutureImplementation`
is such a subclass: its `doesNotUnderstand:` forces the future by its own
private `resolve` and sends the message on to its value with
`Message>>sendTo:`; its state is the two slots `computation` and `state`,
which only a `resolve` writes, and the runtime forces a future by pushing
that method (`interpreter/future.rs`). `resolve` asks whether a value is a
future through the mirror row `objectIsFuture:ifFail:`, and takes over the
futures of a chain its computation answered through the rows
`futureState:ifFail:`, `futureComputation:ifFail:`,
`futureTakeOver:ifFail:`, `future:resolveTo:ifFail:` and
`futureAbandon:ifFail:` (`builtins/future.rs`). Like `Activation`, it is
private, `Object` declares no accessor for it, and an image names it,
`Kernel FutureImplementation`, only where it must (an unresolved future's
record). The
instructions `MakeFuture` and `ResolveFuture` make and resolve the futures
of a simultaneous slot clause, and the public `Future`, a class without
instances, makes one for a program with `Future computing:`, which sends
`FutureImplementation`'s factory `computing:` and stores the computation
untouched; `Future new` signals a
`CannotInstantiate` ([object-model.md](object-model.md), "Simultaneous
slots", "The future"). Once a program has made a future, the
callers of a leaf look for one among its operands on the leaf's failure
path (`future::after_a_failed_leaf`), and identity and the equality leaves
decline for one where they would answer "no"
(`Primitive::answers_false_to_a_stranger`). The design is
[object-model.md](object-model.md), "Simultaneous slots".

### Real bodies for control flow and `do:`

`Closure`'s `whileTrue:` and `whileFalse:` and `Array>>do:` are Newspeak
loops, not forwarders to leaves (decided with the user on 2026-10-27), as
`True`'s and `False`'s branches are Newspeak ("`True` and `False` are quick
methods" above). `whileTrue: body` is `^[self value] whileTrue: [body
value]`, `whileFalse:` its twin: the loop the compiler inlines over two
block literals ([compiler.md](compiler.md), "Control-flow inlining"), so this
method serves every send the compiler does not inline -- a receiver or a body
held in a variable -- and its own loop is that inlined form. `Array>>do:
action` reads its size through the `collectionSize:` leaf, `count = vmMirror
collectionSize: self ifFail: [^super do: action]`, and walks it with an
inlined `[index <= count] whileTrue: [action value: (self at: index). index::
index + 1]`, answering the receiver.

The reason is the JIT tiers. A leaf that runs a block is opaque: nothing can
inline its block, specialize its sends or compile the loop as a unit. Written
in Newspeak, every send of the loop -- `value`, `value:`, `at:` -- is an
ordinary virtual send with a call site of its own, which road step 8's
baseline JIT and the tiers after it see through ([object-model.md](object-model.md),
"Planned: the road to a high-performance VM"; [execution.md](execution.md),
"Planned: JIT tiers"). Before a JIT the loop is more sends than the leaf it
replaced, which is why it came as late as it could, right before the JIT
([execution.md](execution.md), "What Newspeak loop bodies moved"). The loop
is written out in `do:` rather than as `1 to: count do:`, which is a send to
`Number>>to:do:` and would put a second activation under every element; the
size is read through its leaf, as `ByteArray>>do:` reads its own, so that an
instance of a user subclass of `Array` that holds no elements goes on to
`Collection>>do:` rather than counting through `size`
(`goldens/library/builtin-subclass/a_payload_less_array_subclass_instance_falls_to_collection_do`).
Under the reference lowering, which inlines no other send, a literal loop is
a naive loop over real closures, since `Closure>>whileTrue:`'s own body would
otherwise send itself until the depth limit ([compiler.md](compiler.md),
"What it emits").

The bodies keep what the leaves they replaced did, but for one thing: a
loop over blocks held in variables is now an activation of
`Closure>>whileTrue:` or `whileFalse:`, counted under both lowerings, so a
recursion through such a loop takes three counted activations a level
where it took two and overflows at a lower level ([execution.md](execution.md),
"The depth limit"). `do:` reads the size once, when the loop starts, and each
element when the walk reaches it, so a store inside the block to a later index
is seen (`goldens/library/collection/do_sees_a_store_made_inside_its_block`);
an empty array sends nothing, whatever the argument
(`goldens/library/collection/do_on_an_empty_array_sends_nothing`). `do:`
sends `value:` to an argument that is no block, once per element, and
`whileTrue:` and `whileFalse:` send `value` to such a receiver or body, once
per iteration, as any send does
(`goldens/library/collection/do_sends_value_to_a_non_block`,
`goldens/language/control-flow/a_loop_sends_value_to_a_body_that_is_no_block`);
a future is seen through by that send. A block of the wrong arity signals the
`ArgumentError` that `value` or `value:` gives it, once, before any body runs
(`goldens/language/control-flow/a_loop_receiver_that_takes_a_parameter_signals_once`),
and a condition that answers no Boolean signals the `NonBooleanReceiver` of
the inlined loop's guard, for `whileFalse:` as for `whileTrue:`
(`goldens/language/control-flow/a_while_false_condition_that_goes_non_boolean_signals_non_boolean_receiver`).
A conditional sends `value` to such an argument too, and nothing answers
it ("`True` and `False` are quick methods" above).

## The kernel completed to Primordial Soup's core API

`Kernel.ns` and `CollectionsFactory.ns` carry the whole public core API
of Primordial Soup's kernel and collections
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`newspeak/Collections.ns`), completed before road step 5 (decided with the
user on 2026-10-06) in the six sub-projects below, the last built on
2026-10-16. Core means the classes a program computes with -- objects,
booleans, numbers, strings, closures, messages, exceptions, collections,
byte arrays, weak references -- and not the platform's side effects (files,
network, time, random numbers, processes), not reflection or tooling
(mirrors, `allInstances`, `become:`), and not actors. An inventory taken on
2026-10-06 counted 450 public selectors in that scope, 131 of them present,
11 in part and 308 missing.

**The API is adopted; the code is written here.** Selectors, arguments,
answers and error cases follow Primordial Soup's, so its behaviour may be
reproduced exactly, but every body is written from that description, not
adapted from Primordial Soup's source, and the shipped modules name no
other project in their comments (decided with the user on 2026-10-06).
`List`, `Map` and `Set` are written in the same way, so that
`CollectionsFactory.ns` holds no adapted code and has no licence header,
and `THIRD-PARTY-NOTICES.md` no entry for it.

**The on-demand rule no longer governs the core** ("The on-demand rule"
above): it stays for `PlatformFactory.ns`, whose members reach the host,
and `Kernel.ns` and `CollectionsFactory.ns` take the whole core API.

**Where Primordial Soup's API met a decision taken here**, each was settled
on its own (decided with the user on 2026-10-06):

- **A numeric tower, symmetric** -- built ("The numeric tower" above).
  `Fraction` exists; `Integer>>/` answers an exact quotient, an `Integer`
  when it divides; `=` compares `Integer`, `LargeInteger`, `Fraction` and
  `Float` by value in both directions, and `hash` agrees with it -- `3
  asFloat hash = 3 hash`, as Primordial Soup's `testFloatHash` asks --
  where Primordial Soup's `Fraction>>=` answers `false` for a `Float` and
  the other order signals. A NaN still equals a NaN.
- **Strings stay code points** ("Strings are code points in two widths"
  above). Primordial Soup's `String` selectors are adopted with code-point
  meaning -- `size`, `at:`, `do:`, `indexOf:`, `withAll:` count and take
  code points -- and bytes appear only where a conversion says so:
  `asUtf8` answers the UTF-8 bytes, `String fromUtf8:` decodes them
  strictly, and `StringBuilder` takes strings and code points
  ("`ByteArray`, `StringBuilder` and the views" above).
  §5.1.5 leaves the encoding to an API that "encapsulates this decision".
- **Primordial Soup's error classes, under `Error`.** `ArgumentError`,
  `CannotReturn`, `NonBooleanReceiver` and `CannotInstantiate` exist as
  subclasses of `Error`, so that `on: Error do:` still catches them and
  today's texts stay their `messageText`; Primordial Soup has no `Error` and
  its classes no text. The leaves, the VM and the kernel's and the
  collections' Newspeak signal them where it signals them, and
  `CollectionsFactory.ns` adds `NotFound`, which the collections' lookups
  and removals signal, and `ConcurrentModification` ("Exceptions"; "The
  collections module (plan 2)").
- **`isKindOf…` answers `false` for what a receiver does not declare**:
  `Object>>doesNotUnderstand:` answers `false` for a unary selector that
  begins with `isKindOf`, as Primordial Soup's and the canonical Newspeak's
  (`prior-art/newspeak/KernelForJS.ns`) do, and every core class answers
  its own with `true`: `isKindOfArray`, `isKindOfBehavior`,
  `isKindOfBoolean`, `isKindOfClosure`, `isKindOfCollection`,
  `isKindOfFloat`, `isKindOfFraction` (`Integer`'s too, an integer being
  a fraction whose denominator is 1), `isKindOfInteger`,
  `isKindOfNumber`, `isKindOfString`, `isKindOfByteArray`,
  `isKindOfStringBuilder`, `isKindOfTypedArray`, `isKindOfDataView`,
  `isKindOfList`, `isKindOfMap`, `isKindOfIdentityMap` and
  `isKindOfSet`; a class a later sub-project adds brings
  its own answer, and its accessor on `platform kernel`,
  `platform collections` or `platform typedArrays`, when it is built. A program can introduce a
  question of its own
  without touching `Object`, which no Newspeak program can. `Number`
  declares `isKindOfFloat`, `isKindOfInteger` and `isKindOfFraction` as
  `false` so that a subclass's `true` overrides a known answer rather than
  the rule; a keyword selector that begins with `isKindOf` is no such
  question and still ends in the exception.
- **`yourself` and `identityHash`.** `Object>>yourself` answers the
  receiver (a quick method), so a cascade can end with it. `identityHash`
  is the object's identity hash, the leaf `hash` uses; `Value` and
  `Behavior` answer their `hash` instead, since their identity is their
  equality.
- **A weak collection refuses a value as a key.** `WeakMap` and
  `Ephemeron` signal an `ArgumentError` for a key that is a value
  (§3.1.1): a value has no identity, an equal one can always be made
  again, so a weak entry under it would vanish or not as the collector
  happened to run. JavaScript's `WeakMap`, which the canonical Newspeak's
  is built on, refuses such keys too. `identityHash` of a value is its
  `hash`, since `==` on a value is `=`.

Kept as recorded deviations, not taken up: tuple literals stay mutable
`Array`s (§5.1.7), a non-integral literal stays a `Float` (§5.1.1, "Deviation:
a non-integral literal is a binary float" above), and there is no
`Character` (§5.1.4).

**Six sub-projects, in this order**, each with its own design and plan
(decided with the user on 2026-10-06):

1. **Aligning** -- built (2026-10-06, [its
   plan](../superpowers/plans/2026-10-06-0311-ns-next-kernel-aligning.md)).
2. **Foundations** -- built (2026-10-10, [its
   plan](../superpowers/plans/2026-10-06-2036-ns-next-kernel-foundations.md)):
   the error classes, the `isKindOf…` rule, `yourself`, `identityHash`, the
   kernel facet's accessors, `Proxy` and a public `Future`.
3. **Collections and strings** -- built (2026-10-10, [its
   plan](../superpowers/plans/2026-10-10-1800-ns-next-kernel-collections-and-strings.md)):
   an abstract `Collection` in the kernel with `Array` under it and
   `Object>>subclassResponsibility`; the whole protocol of `Array`, `List`,
   `Map` and `Set`, their removals signalling `NotFound` and a change
   during an iteration `ConcurrentModification`; `IdentityMap` under
   `Map`; `List`, `Map` and `Set` written here; `String`'s protocol in
   code points, with `stringAt:`; and every member of the shipped
   modules documented ("Collections and strings" under "Decisions"
   below).
4. **Numbers** -- built (2026-10-11, [its
   plan](../superpowers/plans/2026-10-11-1500-ns-next-kernel-numbers.md)):
   `Float`'s mathematics, rounding, questions and texts, Primordial Soup's
   shortest printing and `Float parse:`; `Integer`'s `gcd:`, shifts,
   digits, radix texts, parsing and modular arithmetic; `Number`'s share
   of the protocol; `Fraction`, a builtin made only in normal form; and
   the symmetric tower -- exact comparison, the numeric hash, exact
   arithmetic without a `Float`, `//`, `\\`, `quo:` and `rem:` over every
   pair ("The numeric tower" above; "The kernel's numbers" under
   "Decisions" below).
5. **`ByteArray` and `StringBuilder`** -- built (2026-10-13, [its
   plan](../superpowers/plans/2026-10-13-1614-byte-array-and-views.md);
   "`ByteArray`, `StringBuilder` and the views" above): `ByteArray` and
   the UTF-8 conversions, with
   `Object>>ByteArray` beside `Object>>Array`, the accessor §3.2 names
   ("the methods `Array` and `ByteArray` provide predefined factory
   objects for arrays"); and,
   beyond Primordial Soup's core, typed arrays as an API of their own
   (decided with the user on 2026-10-11): classes that wrap a
   `ByteArray` and answer its elements at one width -- `float32`,
   `float64`, the signed and unsigned integers -- indexed by element
   rather than by byte offset, as JavaScript's `Float32Array` and its
   siblings, so that a program keeps a large homogeneous run of numbers
   packed at the width it needs. Their elements lie contiguous and
   aligned to 32 bytes, so that a later leaf operating on a whole array -- `a add: b into: c`,
   `a dot: b` -- can use SIMD directly, paying a send once per array
   rather than once per element; such bulk operations, and a GPU API
   among the platform's capabilities that uploads from these arrays, are
   possible later APIs and not part of the sub-project (noted with the
   user on 2026-10-11). Designed with the user on 2026-10-12, with
   `DataView` beside the typed arrays.
6. **Weak references** -- built (2026-10-16, [its
   plan](../superpowers/plans/2026-10-16-1645-weak-references.md);
   "Weak references" above): `WeakArray`, `WeakMap` and `Ephemeron`, in
   the scavenger and the compaction, the image and the stress modes, a
   weak collection refusing a value as a key, and `platform kernel
   garbageCollect`.

## Decisions

- **The kernel's names and loops aligned with Primordial Soup's core API**
  (decided with the user on 2026-10-06, built the same day): `Closure`,
  `sendTo:`, `Array>>at:put:` answering the value, and loops that send
  `value`/`value:` to a non-block
  ([plan](../superpowers/plans/2026-10-06-0311-ns-next-kernel-aligning.md)).

- **The kernel's foundations** (decided with the user on 2026-10-06, built
  on 2026-10-10): Primordial Soup's error classes under `Error`, so that
  `on: Error do:` still catches them, each carrying its subject in a public
  slot -- `ArgumentError`'s is `argument`, not `value`, since an exception
  answering `value` would pass for a block; `CannotInstantiate` added as
  ns-next's own, public so that a program can catch and signal it;
  `NonBooleanReceiver` only for a loop's condition, since the
  specification has no inlining and an inlined conditional keeps its real
  send; the Rust site that finds an error naming its class
  (`ErrorClass`), built by the one `Mutator::signal_vm_error`; `Future` a
  public class only for its class side's `computing:`, over the private
  `FutureImplementation` under the public `Proxy`; and the `isKindOf…`
  rule, `false` for a unary question nobody declared ("Exceptions", "The
  pull future", "The kernel completed to Primordial Soup's core
  API" above;
  [plan](../superpowers/plans/2026-10-06-2036-ns-next-kernel-foundations.md)).

- **Collections and strings** (decided with the user on 2026-10-10, built
  the same day; "What exists", "The collections module (plan 2)" above;
  [plan](../superpowers/plans/2026-10-10-1800-ns-next-kernel-collections-and-strings.md)):
  the element of a `String` is a code point, an `Integer` -- `do:`, `first`
  and `last` answer one, `String withAll:` takes them, as `at:` and `String
  with:` already did -- and `stringAt:`, beyond Primordial Soup's API,
  answers the `String` of the one code point at an index, a name that says
  what it answers and suggests no `Character`. A `String` is no
  `Collection`, as in Primordial Soup, the canonical Newspeak and SOMns; it
  stays under `Value` with its own protocol. `Collection` is a kernel
  builtin under `Object`, because `Array` is a kernel builtin and needs it
  as its superclass; `List`, `Map` and `Set` reach it through
  `CollectionsFactory`'s slot `Collection`. `IdentityMap` is a subclass of
  `Map` in the collections module, not a separate kernel class: it compares
  keys with `==` and hashes them with `identityHash` and inherits
  everything else, insertion order and removal among it; its `at:put:`
  replaces a present key's value as `Map`'s does, and it keeps Primordial
  Soup's `at:putReplace:`, `at:ifAbsentPutVal:` and `atOrItself:` but not
  the public `size:` setter. Road step 5's atomic install is to be its
  first user: it maps old objects to new ones and must not run a program's
  `=`. `NotFound` stays in the collections module, under `Error`. No
  immutable variants yet: no `ReadOnlyTuple` (§5.1.7 stays a recorded
  deviation) and no value collections, and `Collection` being a class bars
  neither -- a `ReadOnlyTuple` is only shallowly immutable and fits under
  it, and a deeply immutable value collection would turn `Collection` into
  a mixin applied to `Object` and to `Value`
  ([OPEN-WORK.md](../OPEN-WORK.md)). Leaves stand where Primordial Soup has
  primitives -- `Array`'s `copyFrom:to:` and
  `replaceFrom:to:with:startingAt:`, `String`'s two searches and
  `withAll:`, and the kernel-private activation's `methodName` for
  `Object>>subclassResponsibility` -- and Newspeak everywhere else. Every
  body is written here from the protocol's description, not from the
  bodies of Primordial Soup's, SOMns's or Strongtalk's collections, and no
  test of another project is adapted. A structural change while an iteration
  runs signals `ConcurrentModification`, a collections-module `Error`
  beyond Primordial Soup's API, which, like the specification, leaves it
  undefined: every `add:`, `addFirst:`, `addLast:`, `addAll:`, removal and
  `removeAll`, and an `at:put:` of a key a `Map` did not hold, counts,
  while storing at an existing index or key and sorting do not; a `List`'s,
  `Map`'s or `Set`'s `do:`, `keysAndValuesDo:` and everything built on
  `do:` read the count before the first element and compare it after every
  block that does not end the iteration by returning, the last included,
  a `List`'s `sort:` compares it once its copy is sorted and then writes
  nothing back, and a removal selector's own changes while it
  scans (`removeAllSuchThat:`) are not reported. Every member of
  `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns`, public,
  protected and private, is documented to the standard of Javadoc, with
  doc tags (`CLAUDE.md`, "Conventions";
  `crates/ns-runtime/tests/core_library_documentation.rs`).

- **The kernel's numbers** (decided with the user on 2026-10-11, built the
  same day; "The numeric tower", "A Float's text" above;
  [plan](../superpowers/plans/2026-10-11-1500-ns-next-kernel-numbers.md)):
  ten decisions and two additions to Primordial Soup's API, each for the
  reason given.
  1. Mixed comparison is exact, since through `double` `=` is not
     transitive at 2^53 and no hash could serve it.
  2. The hash is Python's numeric hash modulo 2^61 - 1, since it hashes
     equal numbers alike across every representation and leaves a
     `SmallInteger`'s hash itself.
  3. `Fraction` is a builtin with a Rust representation, as
     `LargeInteger` is, since the tower's rules then live in one module, a
     fraction is normal by construction and a mixed operation needs no
     fallback send; chosen on its merits, not its cost.
  4. `Integer>>isKindOfFraction` answers `true`, with `numerator` itself
     and `denominator` 1, as in Primordial Soup, since an integer is a
     fraction whose denominator is 1.
  5. Arithmetic is exact without a `Float` and a `Float` with one, the
     rational side rounded once, since Primordial Soup's two roundings lose
     precision and answer NaN past `f64::MAX`.
  6. An exact division by zero signals an `ArgumentError` while a
     `Float`'s stays IEEE's, since a fraction with denominator 0, Primordial
     Soup's answer, is no number.
  7. `//`, `\\`, `quo:` and `rem:` take every pair of numbers, as
     Smalltalk-80's do, since one definition over the exact quotient
     serves all four representations.
  8. `raisedTo:` with an integer exponent is exact, a negative one
     answering the reciprocal, since repeated squaring over the tower
     loses nothing.
  9. No double-dispatch selectors, and an `ArgumentError` for a
     non-number, since the leaves compute every combination and the
     selectors were only Primordial Soup's mechanism.
  10. A `Float` prints as Primordial Soup's does, the shortest text that
      reads back, since a program written against that API expects its
      texts.
  The additions: `truncated` on every number, since all three classes
  answer `floor`, `ceiling` and `rounded`; and `isNaN`, `isFinite` and
  `isInfinite` on `Number`, so every number answers what `Float`
  overrides. `num-rational`'s `Ratio` was the model for the arithmetic,
  the rounding and the comparison, and no code of it was taken, so its
  licence is not taken on.

- **`ByteArray`, `StringBuilder` and the views** (decided with the user on
  2026-10-12, built on 2026-10-13; "`ByteArray`, `StringBuilder` and the
  views" above;
  [plan](../superpowers/plans/2026-10-13-1614-byte-array-and-views.md)):
  Primordial Soup's API is changed
  where it does not fit code-point strings or a 1-based language; UTF-8 is
  decoded strictly, by `String fromUtf8:`, and encoded by `asUtf8`; a
  `ByteArray`'s methods take a `String` as its UTF-8 bytes, a `String`'s
  refuse a `ByteArray`; `ByteArray` gains the sequence protocol, from a
  kernel mixin it shares with `Array` and `TypedArray`, and its `indexOf:`
  decides by the argument; `StringBuilder` counts code points and appends
  them with `addCodePoint:`; ten typed arrays, in the module
  `TypedArraysFactory.ns` behind `platform typedArrays`, view a whole
  `ByteArray` by element, little-endian, checking integer ranges and converting any number
  for a float element; `DataView` reads and writes at 1-based byte
  positions in the byte order its factory is given, through two private
  classes; and every `ByteArray`'s data lie at a 32-byte address, a reserve
  of three words that every mover realigns. Settled while building it, where
  the design was silent: `ByteArray>>startsWith:` and `endsWith:` refuse an
  argument that is no `Integer`, `ByteArray` or `String`, as the canonical
  implementation does, while `indexOf:` and `lastIndexOf:` look for it as
  an element; the leaf behind `addCodePoint:` reports its own selector,
  `at:putCodePoint:`, and `StringBuilder>>addCodePoint:` checks the code
  point itself, rather than the mirror machinery learning a row reported
  under another method's selector; a refusal written in Newspeak renders
  its argument with `printString`, a leaf's with `describe`; a view whose
  slot holds no `ByteArray` names the selector sent, and a builder whose
  slots an image crafted is refused by `checkState:`, an `ArgumentError`
  in both cases, since every error of the sub-project is one; and the
  decoder holds a byte-array body and the `ByteArray` header to each
  other both ways, since a record carries exactly its class's layout.
  Afterwards (2026-10-13) the typed arrays moved out of the kernel into the
  shipped module `TypedArraysFactory.ns`, behind `platform typedArrays`, and
  a view's and a typed array's size refusals became their own
  `ArgumentError`s under their own selector, decided before allocating by
  the mirror leaf `byteArrayClass:fits:ifFail:`, a Boolean through
  `Heap::byte_array_fits`: a row's reported selector is fixed, so a leaf
  that allocated could not have reported under `new:byteOrder:` ("Size
  refusals are the view's own" above).

- **`True` and `False` are Newspeak, over quick methods** (decided with the
  user on 2026-09-24, built the same day): the Boolean half of "Real
  bodies for control flow and `do:`", built ahead of the loops; the
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
- **`Kernel` is a value, and the mirror's class sits below `Value`**
  (decided with the user on 2026-09-23; written `= Value`, as every shipped
  module's root, since 2026-10-13), as in SOMns: §3.1.1's
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
  user on 2026-09-24, built the same day): one `Kernel.ns` method per
  nameable builtin, `^outer Kernel <Name>`, protected (below), answered by
  `MethodFastPath::BuiltinClass` without an activation, and a bare builtin
  name is the self send §5.7 makes it, so a class that declares or inherits
  the name reaches its own. `LoadBuiltinClass` and its wire tag 2 are
  retired — "The predefined classes are accessors on `Object`" above.
- **`class` and the class accessors are protected** (decided with the user
  on 2026-10-13, built the same day; "`class` and the class accessors are
  protected" above;
  [plan](../superpowers/plans/2026-10-13-1158-protected-class.md)):
  `Object>>class` and every class accessor of `Object` is `protected`,
  as in both Newspeak kernels, so that holding an instance does not hand
  out its class's authority -- its factory, its class side's state, a
  subclass in its environment; `self class`, `super class` and a bare name
  keep working, and an outside send meets the ordinary
  "does not understand" text, which names no protected member. The core
  library reads another object's class through the
  mirror (`valueEquals:`) or asks `isKindOfInteger`, widening nothing; the
  image's value clause for the classes the VM signals goes, since no
  program holds `NonBooleanReceiver` as a value; and what only the
  representation shows is checked from the host
  (`crates/ns-runtime/tests/representation.rs`), most of which road step
  5a's mirrors let goldens check again ("`class` and the class accessors
  are protected" above).
- **Integers are unbounded, through a `LargeInteger` written here**
  (decided with the user on 2026-09-29, built the same day): §5.1.1
  requires it, and a bignum crate would keep its digits outside the object
  space. Every integer has one representation, an immediate's class stays
  `Integer`, and `//` rounds toward negative infinity, as Primordial Soup's does. The
  algorithms are adapted from `num-bigint` and `num-integer` under MIT, with
  their attribution in `THIRD-PARTY-NOTICES.md` and a header per file,
  including the number-theoretic transform and Burnikel-Ziegler division
  (decided with the user the same day); divide-and-conquer radix conversion
  is not taken ("Large integers" above).
- **Strings store code points in one of two widths**, Latin-1 or 32
  bits (decided with the user on 2026-09-29, built on 2026-10-01, the
  byte- and 32-bit-indexed formats with road step 3a on 2026-10-03;
  "Strings are code points in two widths" above;
  [object-model.md](object-model.md), "The object layout"). `Symbol` is a
  subclass of `String` since road step 3a (decided with the user on
  2026-09-29).
- **`List` is used canonically and made fast; `Map` and `Set` have
  insertion-ordered hash tables** (decided with the user on 2026-10-01, built the
  same day; "The collections module (plan 2)" above;
  [plan](../superpowers/plans/2026-10-01-1855-ns-next-are-we-fast-yet-stage-3.md)):
  a queue is `add:` and `removeFirst`, as Are We Fast Yet's `Vector` is
  one of `append:` and `removeFirst`, and `List` keeps a first index so
  that `removeFirst` moves nothing, rather than a program being rewritten
  around a slow operation; `Map` and `Set` keep their keys in
  insertion order and rebuild their table from them, in a `lazy` slot; the
  representation of all three is private, but for `List`'s `size_` and
  the three hooks through which `IdentityMap` changes `Map`'s comparison.
- **A NaN equals a NaN** (decided with the user on 2026-10-01, built the
  same day; "A NaN equals a NaN" above).
- **Exceptions** (nine decisions taken with the user on 2026-09-25, built
  2026-09-25 to 2026-09-26; "Exceptions" above;
  [plan](../superpowers/plans/2026-09-25-2244-ns-next-exceptions.md)):
  Newspeak in `Kernel.ns` over activation leaves, as Primordial Soup, not
  native as SOMns; the scope — `Exception`, `Error`, `MessageNotUnderstood`,
  `Halt`, `Message` and the kernel-private `Activation`, ANSI's protocol
  with `on:do:`, `ensure:` and `ifCurtailed:`, and `ExceptionSet`,
  `Notification`, `outer`, `retryUsing:`, `signalerContext` and
  `Object>>error:` left out, and the finer `Error` subclasses left to
  the foundations (above);
  `doesNotUnderstand:` is Squeak's, re-sending with `sendTo:` when the
  default handler resumed it; every VM error becomes a signalled `Error`
  with today's text; the depth limit keeps a reserve of 1,000; one unwinder
  in `interpreter/`, running `ensure:` blocks as trampoline steps and aiming
  at a `FrameReference`; `Activation` is the frame object itself, reachable
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
2. **`Integer>>/` stayed absent through the move**, since adding it would
   have changed a recorded outcome (2026-09-21); the numbers sub-project
   added it, answering an exact quotient — "The numeric tower" above.
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
7. **Kernel method bodies forward; they do not reimplement** (2026-09-22),
   with one decided exception: `Closure`'s `whileTrue:` and `whileFalse:`
   and `Array>>do:` are Newspeak loops, so that the JIT tiers see through
   them (2026-10-27) — "Real bodies for control flow and `do:`" above.
8. **`True` and `False` are subclasses of `Boolean`** (2026-09-22), as both
   references declare them.
9. **A builtin subclass's own `doesNotUnderstand:` is passed by, per §5.10**
   (2026-09-22) — "The one observable difference" above.
