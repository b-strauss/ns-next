# The kernel completed: collections and strings — design

Sub-project 3 of "the kernel completed to Primordial Soup's core API"
([core-library.md](../../design/core-library.md), "Planned: the kernel
completed to Primordial Soup's core API"). Decided with the user on
2026-10-10. This file is temporary: the batch's documentation task moves its
content into `docs/design/core-library.md` and deletes it.

## Goal

`Kernel.ns` and `CollectionsFactory.ns` carry Primordial Soup's whole public
collection and string API: `Collection`, the whole protocol of `Array`,
`List`, `Map` and `Set`, `IdentityMap`, and `String`'s protocol in code
points. `List`, `Map` and `Set` are rewritten so that no code adapted from
Primordial Soup remains; `CollectionsFactory.ns` loses its licence header
and `THIRD-PARTY-NOTICES.md` its entry for it. Every member of the three
shipped modules -- existing ones included, private ones included -- is
documented to the standard of Javadoc.

An inventory of 2026-10-10 counted 151 public selectors in scope, 40
present, 16 present in part and 95 missing.

## Decisions

1. **The element of a String is a code point, an Integer.** `do:`, `first`
   and `last` answer code points, `String withAll:` takes them, as `at:` and
   `String with:` already do. Canonical Newspeak (`KernelForJS.ns`) answers
   an Integer from `at:`, `first` and `last` too.
2. **`String>>stringAt: index`**, an extension beyond Primordial Soup's API:
   the String of the one code point at `index`. SOMns's `charAt:` is the
   precedent; the name says what it answers and suggests no `Character`.
3. **A String is no Collection**, as in Primordial Soup, canonical Newspeak
   and SOMns (Strongtalk alone makes it one, with a `Character` class). It
   stays under `Value` with its own protocol.
4. **`Collection` lives in the kernel**, under `Object`, because `Array`
   does; `Array` moves under it. `List`, `Map` and `Set` inherit from it
   through the kernel argument of `CollectionsFactory`.
5. **`IdentityMap` is a subclass of `Map`** in the collections module, as
   SOMns's `IdentityDictionary` is of `Dictionary`, not Primordial Soup's
   separate kernel class: it compares keys with `==` and hashes them with
   `identityHash` and inherits everything else, insertion order and removal
   among it. Its `at:put:` replaces an existing key, as `Map`'s does;
   Primordial Soup's refuses one by sending the undefined `duplicateKey`. It
   keeps Primordial Soup's further selectors `at:putReplace:`,
   `at:ifAbsentPutVal:` and `atOrItself:`. Road step 5's atomic install is
   its first user: it maps old objects to new ones and must not run a
   program's `=`.
6. **`NotFound` stays in the collections module**, under `Error`.
7. **No immutable variants** in this sub-project: no `ReadOnlyTuple`
   (§5.1.7 stays a recorded deviation), no value collections. `Collection`
   being a class bars neither: a `ReadOnlyTuple` is only shallowly
   immutable and fits under it; a deeply immutable value collection would
   turn `Collection` into a mixin applied to `Object` and to `Value`, as
   SOMns's `ArrayReadMixin`.
8. **Leaves where Primordial Soup has primitives, Newspeak elsewhere.**
9. **The bodies are written here, from this description.** Implementers do
   not open the bodies of Primordial Soup's `Collections.ns`, nor those of
   `Collection`, `Array`, `String` and `IdentityMap` in
   `KernelForPrimordialSoup.ns`, nor SOMns's or Strongtalk's collections.
   The goldens are derived from this description too; no test of another
   project is adapted.
10. **Structural modification during iteration signals
    `ConcurrentModification`**, a new `Error` subclass in the collections
    module -- an extension beyond Primordial Soup, as Dart's
    `ConcurrentModificationError` and Java's
    `ConcurrentModificationException`. Primordial Soup and the specification
    leave it undefined.
11. **Doc comments to the standard of Javadoc for every member** of
    `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns`: public,
    protected and private, existing and new. `CLAUDE.md`'s "one to four
    lines" rule is rewritten (see "Documentation" below).

## The hierarchy

```
Kernel.ns
Object
├── Collection            new, abstract
│   └── Array             was directly under Object
├── Value
│   └── String            unchanged place
│       └── Symbol
└── Exception
    └── Error
        └── ArgumentError ...

CollectionsFactory.ns
Collection (the kernel's)
├── List                  was directly under Object
├── Map                   was directly under Object
│   └── IdentityMap       new
└── Set                   was directly under Object
Error (the kernel's)
├── NotFound
└── ConcurrentModification  new
```

New accessors: `platform kernel Collection` (in `PlatformFactory.ns`'s
`KernelNamespace`), `platform collections Collection`,
`platform collections IdentityMap` and
`platform collections ConcurrentModification`. `CollectionsFactory` reaches
the kernel's `Collection` through a slot beside `kernelError`, for the same
reason (an inheritance clause is one name and at most one message, §6.1).
New `isKindOf…` answers: `isKindOfCollection` on `Collection`,
`isKindOfIdentityMap` on `IdentityMap`.

## The protocol

"New" marks a selector that does not exist today. Indices are 1-based.
Unless said otherwise a selector answers the receiver.

### Collection (kernel, abstract)

- `do: action` — `subclassResponsibility`. Every other selector here is
  written over it.
- `allSatisfy: predicate`, `anySatisfy: predicate`,
  `noneSatisfy: predicate` — answer a Boolean, stopping at the first
  element that decides. An empty collection answers `true`, `false`,
  `true`.
- `detect: predicate` — the first element for which `predicate` answers
  `true`; when none does, signals an `Error` with the text
  `'No matching element detected'`.
- `detect: predicate ifNone: onNone` — the same, answering `onNone value`
  when none does.
- `do: action separatedBy: between` — `action` on every element,
  `between value` between two consecutive ones.
- `includes: object` — whether some element is `=` to `object`.
- `inject: initial into: fold` — folds from the left:
  `fold value: accumulated value: element`, answering the last result, or
  `initial` for an empty collection.
- `isEmpty` — `size = 0`.
- `reduce: merge` — folds from the left starting with the first element;
  on an empty collection signals an `Error` with the text
  `'Cannot reduce an empty collection'`. A one-element collection answers
  that element without sending `merge`.
- `reduce: merge ifEmpty: onEmpty` — the same, answering `onEmpty value`
  for an empty collection.
- `collect: transform`, `select: predicate`, `reject: predicate` — a new
  collection obtained from `self newForCollectUsingAdd: self size` and
  filled with `add:`. The hook `newForCollectUsingAdd: capacity` is
  protected and `subclassResponsibility` here (the kernel does not know
  `List`): a program's own subclass defines `do:` and, for these three, the
  hook. `List` answers a `List`, `Set` a `Set`; `Array` and `Map` override
  the three.
- `size` — counts the elements through `do:`.
- `isKindOfCollection` — `true`.

### Array (kernel, under Collection)

Unchanged: `at:`, `at:put:` (answers the value), `do:` (leaf), `size`,
`isKindOfArray`, class-side `new:`.

New:

- `, collection` — a new Array of the receiver's elements followed by
  those of `collection`, which may be any object answering `size` and `at:`
  (Array, List, String).
- `asArray` — the receiver itself.
- `collect:`, `select:`, `reject:` — a new Array.
- `copyFrom: start to: stop` — a new Array of the elements from `start` to
  `stop`; empty when `stop = start - 1`. Signals an `ArgumentError` unless
  `1 <= start`, `stop <= size` and `start <= stop + 1` (leaf).
- `copyWith: element` — a new Array one longer, `element` last.
- `copyWithSize: newSize` — a new Array of `newSize` elements: the
  receiver's first ones, truncated, or followed by `nil`.
- `first`, `last` — `self at: 1`, `self at: self size`; on an empty Array
  the `ArgumentError` of `at:`.
- `indexOf: element` — the index of the first element `=` to `element`,
  or `0`.
- `isEmpty`.
- `keysAndValuesDo: action` — `action value: index value: element` for
  each index in order.
- `replaceFrom: start to: stop with: source startingAt: sourceStart` —
  stores `source`'s elements from `sourceStart` on into the receiver's
  positions `start` to `stop`; correct when `source` is the receiver and
  the ranges overlap. Signals an `ArgumentError` for a range outside either
  receiver or source. A leaf when `source` is an Array; any other source
  answering `at:` goes through a Newspeak loop.
- `sort: lessOrEqual` — sorts in place, stably, with the two-argument
  block answering whether its first argument may precede its second.
- `sortFrom: start to: stop by: lessOrEqual` — the same over a range.
- class-side `withAll: collection` — a new Array of `collection`'s
  elements in its `do:` order.

### String (kernel, under Value; Symbol inherits all of it)

Indices and lengths count code points. A selector that answers a copy or a
part answers a String, for a Symbol receiver too.

Unchanged: `size`, `at:`, `,`, `=`, `hash`, `asString`, `asSymbol`,
`copyFrom:to:`, `printString`, `isKindOfString`, class-side `with:`.

New:

- `do: action` — each code point, in order.
- `first`, `last` — the first or last code point; on an empty String the
  `ArgumentError` of `at:`.
- `isEmpty`.
- `stringAt: index` — the String of the one code point at `index`; the
  `ArgumentError` of `at:` for a bad index (over the `copyFrom:to:` leaf).
- `copyStringFrom: start to: stop` — `copyFrom:to:`.
- `indexOf: substring` — `indexOf: substring startingAt: 1`.
- `indexOf: substring startingAt: index` — the index of the first
  occurrence of `substring` that starts at or after `index`, or `0`.
  `index` must lie in `1..size + 1`; an empty `substring` is found at
  `index`. Signals an `ArgumentError` for an `index` outside that range or
  a `substring` that is no String (leaf).
- `lastIndexOf: substring` — `lastIndexOf: substring startingAt: size + 1`.
- `lastIndexOf: substring startingAt: index` — the index of the last
  occurrence that starts at or before `index`, or `0`; same bounds (leaf).
- `startsWith: prefix`, `endsWith: suffix` — Booleans; an argument that is
  no String signals an `ArgumentError`.
- class-side `withAll: codePoints` — a String of the code points of a
  collection answering `do:`, or a copy of a String. A code point that is
  no Integer in `0..1114111` or is a surrogate signals the `ArgumentError`
  `with:` signals (leaf for an Array or a String; any other collection is
  first gathered into an Array).

Deferred to sub-project 5 (`ByteArray`): `asByteArray`,
`copyByteArrayFrom:to:`, and a `ByteArray` as argument of the search
selectors and of `withAll:`. Not adopted: `out`, a side effect of the
platform, outside the core.

### List (collections, under Collection)

Unchanged in behaviour: `add:` (answers the element), `at:`, `at:put:`,
`do:`, `isEmpty`, `isKindOfList`, `remove:ifAbsent:`, `removeAll` (keeps
the capacity), `removeFirst`, `removeLast` (both answer the element; on an
empty List an `Error`, `'removeFirst: the List is empty'` and
`'removeLast: the List is empty'`), `size`, `sort:`, class-side `new`
(capacity 3) and primary factory `new: capacity`. `List new: 15` as a
superclass clause keeps working.

New:

- `, collection` — a new List of the receiver's elements followed by
  `collection`'s (any Collection).
- `addAll: collection` — `add:` of each element in `collection`'s `do:`
  order; answers `collection`.
- `addFirst: element`, `addLast: element` — answer the element.
- `asArray` — a new Array of the elements.
- `collect:`, `select:`, `reject:` — a new List (through the hook).
- `copyFrom: start to: stop` — a new List; bounds as `Array`'s.
- `first`, `last` — the `ArgumentError` of `at:` when empty.
- `indexOf: element` — as `Array`'s.
- `keysAndValuesDo: action` — as `Array`'s.
- `remove: element` — removes the first element `=` to `element` and
  answers it; signals `NotFound` with the element when there is none.
- `removeAllSuchThat: predicate` — removes every element for which
  `predicate` answers `true`, keeping the order of the rest.
- class-side `withAll: collection`.

### Map (collections, under Collection)

Keys are found by `hash` and compared with `=`. Iteration is in insertion
order; re-putting an existing key keeps its place. As a Collection, a
Map's elements are its values.

Unchanged: `at:` (`NotFound` with the key, `'key not found: '` and the
key's `printString`), `at:put:` (answers the value), `keys` (an Array, in
insertion order), `removeAll`, `isKindOfMap`, class-side `new` and primary
factory `new: capacity`, with today's capacity errors.

New:

- `addAll: aMap` — `at:put:` of each of `aMap`'s keys and values, in its
  order; answers `aMap`.
- `at: key ifAbsent: onAbsent` — the value, or `onAbsent value`.
- `at: key ifAbsentPut: generator` — the value; when absent, stores and
  answers `generator value`.
- `collect: transform` — a new Map of the same keys and transformed
  values, in the same order.
- `select: predicate`, `reject: predicate` — a new Map of the keys whose
  values pass, in the same order.
- `do: action` — each value. `includes:`, `inject:into:`, `detect:` and the
  rest of Collection's protocol follow from it.
- `includesKey: key`.
- `isEmpty`, `size`.
- `keysAndValuesDo: action` — `action value: key value: value`.
- `removeKey: key` — removes the key and answers its value; `NotFound` as
  `at:` when absent.
- `removeKey: key ifAbsent: onAbsent` — the same, answering
  `onAbsent value` when absent.
- `values` — an Array, in insertion order.
- class-side `withAll: aMap`.

### IdentityMap (collections, under Map)

Keys are found by `identityHash` and compared with `==`; nothing else
differs. A value as key (§3.1.1) behaves as in a `Map`, since a value's
`==` is `=` and its `identityHash` its `hash`. New beside the inherited
protocol:

- `at: key putReplace: value` — replaces the value of a key that must be
  present; signals `NotFound` as `at:` when absent. Answers the value.
- `at: key ifAbsentPutVal: value` — stores `value` when `key` is absent;
  answers `true` when it stored, `false` when the key was present (and its
  value is kept).
- `atOrItself: key` — the value, or `key` itself when absent.
- `isKindOfIdentityMap` — `true`.

Primordial Soup's public `size:` setter is not adopted: the size is the
map's, not a program's to set.

### Set (collections, under Collection)

Elements are found by `hash` and compared with `=`; iteration is in
insertion order; `add:` of an element equal to a present one replaces it
in place.

Unchanged: `add:` (answers the element), `do:`, `size`, `isKindOfSet`,
class-side `new` and `new: capacity`, with today's capacity errors.

New:

- `addAll: collection` — answers `collection`.
- `asArray`.
- `collect:` — a new Set (equal results merge), `select:`, `reject:` — a
  new Set.
- `includes: element` — by hash lookup.
- `isEmpty`.
- `remove: element` — removes and answers it; `NotFound` with the element
  when absent.
- `remove: element ifAbsent: onAbsent`.
- `removeAll`, `removeAll: collection` (each must be present, else
  `NotFound`), `removeAllSuchThat: predicate`.
- class-side `withAll: collection`.

### NotFound and ConcurrentModification (collections, under Error)

`NotFound` is unchanged: class-side `element:`, `element`, `description`.
`List>>remove:` and `Set>>remove:` set the text `'element not found: '`
and the element's `printString`.

`ConcurrentModification` is new: it carries the collection
(`collection`), its `messageText` is `'the collection was modified while
it was being iterated'`.

## Modification during iteration

`List`, `Map` and `Set` keep a modification count, a private slot that
every structural change increments: `add:`, `addFirst:`, `addLast:`,
`addAll:`, every removal, `removeAll`, and `at:put:` of a key a Map did not
hold. Storing at an existing index or key is no structural change and
leaves the count alone; neither does `sort:`, which reorders in place.

Every iteration that runs a program's block over the collection -- `do:`,
`keysAndValuesDo:` and everything Collection builds on `do:` -- reads the
count before the first element and compares it after every call of the
block, the last included; when it changed, it signals
`ConcurrentModification`. A modification that a removal selector makes
while it scans (`removeAllSuchThat:`) is its own and is not reported.

The golden `goldens/library/collection/do_visits_the_elements_a_list_held_when_it_started.ns`
pins today's semantics (elements added during `do:` are not visited). It
becomes a golden that pins the error, under a new name; this is a decided
behaviour change. No benchmark, no `runnable.toml` row and no core-library
code modifies a collection it is iterating (checked on 2026-10-10;
`corpus/modules/Ducts.ns`'s `detachAll` would, but never runs).

## Representations

The bodies are written here; only these shapes are fixed:

- **List** keeps `data`, `firstIndex` and `size_` (`removeFirst` in
  amortized constant time; the representation private, `size_` protected).
- **Map** keeps its data array of keys and values side by side in
  insertion order and its open-addressing table of indices into it, at
  most three quarters full. Removal leaves a deleted mark in the data
  array and a tombstone in the table; growth compacts both. `IdentityMap`
  overrides only the hash and the comparison, through two protected
  methods of `Map`.
- **Set** the same, with elements instead of key-value pairs.

Integer bit operations are written with what `Integer` has today
(`bitAnd:`, `bitOr:`, `<<`, `//`).

## Leaves

New leaves on the VM mirror, registered in `builtins/primitive.rs`:

| Selector | Mirror selector | File |
|---|---|---|
| `Array copyFrom:to:` | `collection:copyFrom:to:ifFail:` | `builtins/collections.rs` |
| `Array replaceFrom:to:with:startingAt:` | `collection:replaceFrom:to:with:startingAt:ifFail:` | `builtins/collections.rs` |
| `String indexOf:startingAt:` | `string:indexOf:startingAt:ifFail:` | `builtins/text.rs` |
| `String lastIndexOf:startingAt:` | `string:lastIndexOf:startingAt:ifFail:` | `builtins/text.rs` |
| `String withAll:` (class side) | `stringClass:withAll:ifFail:` | `builtins/text.rs` |

- Errors are `NewspeakError::argument_error`, the text naming the argument
  and the valid range, in the style of today's index errors.
- `replaceFrom:to:with:startingAt:` writes every word through `heap.rs`'s
  store accessors and their write barrier, and is correct for an
  overlapping range of one array.
- The search leaves read both string widths.
- No leaf gets a wire tag; the mirror's six locks are untouched; `vm.rs`
  and `compiler/` do not change.

## Documentation

`CLAUDE.md`'s rule "A public member of a shipped core-library module is
documented" becomes:

> **Every member of a shipped core-library module is documented** to the
> standard of Javadoc. Every method, slot and class of `Kernel.ns`,
> `PlatformFactory.ns` and `CollectionsFactory.ns` -- public, protected and
> private -- has a `(* … *)` comment directly above it that says what it
> does and what it answers, each argument and what it must be, every Error
> it signals and when, and what it guarantees about order, identity,
> copying and mutation, as long as that takes; a section that would say
> nothing is left out. It names no other implementation. A comment that
> explains a decision, as the rule above asks, goes after it.

Every existing member of the three modules is brought to it, and every new
one is written to it. `docs/design/core-library.md` ("Planned: …",
sub-project 3, and the sections describing the collections module and the
kernel's classes), `OPEN-WORK.md` ("Next" names sub-project 4, numbers),
`PROGRESS.md` and `THIRD-PARTY-NOTICES.md` follow the built state; the
licence header leaves `CollectionsFactory.ns`; this file is deleted.

## Testing

- Goldens under `goldens/library/collection/` and
  `goldens/library/string/`, one per protocol group of each class
  (accessing, iterating, copying, searching, removing) and one per error
  case above, the class and the `messageText` pinned;
  `ConcurrentModification` per class and per iterating selector, the
  last-step case included; String with both widths (ASCII and code points
  beyond U+FFFF) and with Symbol receivers.
- Rust tests for each leaf: bounds, both widths, an overlapping
  `replaceFrom:…`, a young element stored into an old array (also under
  `NS_NEXT_STRESS_SCAVENGE`).
- Every commit runs `CLAUDE.md`'s gates; the batch ends with
  `finishing-a-batch`.

## Measurement

The Are We Fast Yet benchmarks, release build, against `master`
(`measuring-performance`): after the rewrite of `List`, `Map` and `Set`
with the modification count, and again at the end. Success: not measurably
slower. A measurable loss goes back to the user with the numbers before
the batch goes on.

## Order of work

1. The documentation rule in `CLAUDE.md`; every existing member of
   `Kernel.ns` and `PlatformFactory.ns` documented to it.
2. `Collection` in the kernel, `Array` under it, the Array leaves.
3. `String`'s protocol, the search and `withAll:` leaves.
4. `CollectionsFactory.ns` rewritten: `List`, `Map` with removal,
   `IdentityMap`, `Set`, `NotFound`, `ConcurrentModification`; then the
   measurement.
5. Documentation, the licence removed, the end-of-batch gates.
