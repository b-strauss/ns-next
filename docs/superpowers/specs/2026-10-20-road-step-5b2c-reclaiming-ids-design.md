# Road step 5b2c: reclaiming ids — design

Decided with the user on 2026-10-20. Temporary: once the batch is
finished its content lives in the design documents named under
"Documentation" and this file is deleted.

## Goal

Every per-heap table that live editing grows is reclaimed: the mixin
applications, their guard ids and full-width indices, `successors`, the
mixin versions an install superseded and the mixin ids nothing names, the
compiled-code ids and the inline caches' feedback ranges. A program that
edits, installs and collects in a loop keeps its tables at a bounded
size. And the nested-class cache slot gets a marker per creation, closing
the cycle diagnosis a forward can lose.

Already decided before this design (2026-10-03,
`docs/design/liveness-and-reflection.md`, "What the id has to carry"):
reclamation happens during a full collection, by marking the ids that
live class objects name, as HotSpot unloads classes; the reuse rules of
the three kinds of id are `object-model.md`'s, "An application has two
ids".

## Prior art

- **Spur** (`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c`): the
  class table is weak from page 1 on (`markObjects`, 54733-54740); every
  marked object marks its class (`markAndTraceClassOf`, 53663), so no
  header names a freed index; `expungeDuplicateAndUnmarkedClasses:`
  (54844-54886) frees unmarked entries in a full collection only and
  lowers `classTableIndex` to the smallest freed slot to reuse it first;
  `preGCAction` flushes the method cache and unlinks every send before a
  full collection (18983-18989), and `freeUnmarkedMachineCode`
  (`cogitX64SysV.c:7473`) frees code that names dead objects, followed by
  `unlinkSendsToFree`.
- **Primordial Soup** (`prior-art/primordialsoup/vm/heap.cc`): class ids in
  headers, a weak class table in both collectors (1022-1070), freed ids on
  a free list, `AllocateClassId` (1279-1310) collecting with reason
  `kClassTable` before growing the table; the lookup cache is cleared after
  every collection (`interpreter.cc:1891-1908`). Its nested-class accessor
  has no in-progress marker: a cyclic definition recurses until the stack
  overflows (`Newspeak2PrimordialSoupCompilation.ns:1806-1851`).
- **Strongtalk, V8**: classes and maps are heap objects that caches name by
  pointer; there is no id to reclaim.

The specification (§6) says only that the nested-class accessor evaluates
the declaration on first invocation; ids and their reclamation are below
its level.

## Decisions

1. **Caches are scrubbed selectively.** Only a full collection that freed
   something walks the caches, and drops exactly the entries naming a freed
   guard id or a freed `ApplicationIndex`; every other entry stays warm. A
   later JIT does the same as Spur's `freeUnmarkedMachineCode` with
   `unlinkSendsToFree`.
2. **The smallest free id is minted first,** in every table, for guard ids
   and for feedback ranges (first fit); a table whose last rows are free is
   truncated.
3. **When the guard ids run out,** the minting answers `NONE`, as today, and
   requests a full collection at the next safepoint
   (`Heap::request_collection`), as Primordial Soup collects for its class
   table.
4. **`CodeId`s are reclaimed too.**
5. **`Mixin::nested` is taken over by the table on registration**
   (section "Nested declarations through ids").
6. **The in-progress marker is a serial `SmallInteger` per creation.**
7. **The inline caches' feedback ranges are reclaimed too.**

## When the pass runs and what it reads

`Heap::reclaim_ids` runs in `World::collect` between marking and
compaction, beside `prune_canonical_symbols`: it needs the mark bits.
Only a full collection reclaims; a scavenge frees no id.

**Ids are gathered while marking.** `drain_marking` already matches on the
format; for `Format::Class` it sets the rows of the `APPLICATION` and
`APPLIED_CLAUSE` fields in bit sets on the `Marker`, for `Format::Closure`
and `Format::Frame` the row of `CODE_ID`. No second walk of the heap.

**`Heap::mixin_objects` becomes weak**, as the canonical symbol table is:
an entry whose reified object is unmarked becomes `None`; a marked one
keeps its mixin id alive. Its strong root in `begin_marking` goes. A
reified mixin's identity holds: while anything reaches the object it is
marked, and once nothing does, nothing can compare it.

**Roots outside the heap.** Every activation on the stack: its
`entered_level` and, through `Rc`, its code and its nested-class
creation. Every other Rust holder -- `value_mixin`, a host's
`LoadedModule`, a creation underway -- counts through `Rc::strong_count`:
the table frees a version only when it alone holds it. That keeps a held
version and its id without listing the holders, and excludes the identity
split in which `mixin_id` would mint a fresh id for a version still held.

## The order of the decisions

Each step releases the `Rc`s the next one counts.

1. **Applications.** Live: the gathered `APPLICATION` indices, every
   `entered_level`, closed over `superclass` links and `successors` -- an
   entry of `successors` is kept only while its old index is live, and then
   keeps its new one live. Everything else is freed: its table row becomes
   `None`, its `ApplicationIndex` and its guard id are freed.
2. **Caches.** The application cache loses every entry whose value or
   superclass key is dead; `metaclass_mixins` the entries of dead mixin ids;
   the inline caches, the global cache and the verified pair every entry
   naming a freed guard id or `ApplicationIndex`. Only when step 1 freed
   something.
3. **Mixins.** An id is live when a live application, a gathered
   `APPLIED_CLAUSE`, a marked reified object or a Rust holder
   (`strong_count > 1`) names it; a live id keeps its nested declarations'
   ids and its enclosing declarations' ids alive, through the table. A
   superseded version in `superseded` is dropped once only the table holds
   it.
4. **Codes.** A `CodeId` no marked closure and no marked frame names is
   freed with its `code_ids` entry; the body itself lives on with whoever
   else holds it.
5. **Feedback families.** A family whose root body only `family_codes`
   holds is dropped and its range freed.

## Freeing and reuse

**Tables.** `mixins`, `enclosing`, `nested`, `applications` and `codes`
become `Vec<Option<…>>`, each with a set of free indices (`BTreeSet<u32>`).
Minting takes the smallest free index, else appends. At the end of the
pass every table is truncated by its trailing `None` rows, which leave the
set. `mixin_ids` and `code_ids` lose the entry of every version and body
that is dropped.

**Guard ids.** The counter `next_dispatch_id` becomes an allocator with the
same rule: the smallest free id, else the next never minted; past `LIMIT`
it answers `NONE` and requests a collection.

**Feedback ranges.** Free ranges are kept as `(start, size)` ordered by
start and coalesced with their neighbours; `fresh_range` takes the first
that fits and returns the remainder. A freed range is vacated, so a family
given it again starts with empty sites; a free range at the end truncates
`sites`.

**Images.** The ids are per heap and not on the wire: the encoder numbers
its own rows and codes. No tag and no format change; the plan verifies
this.

## Nested declarations through ids

Until registration, `nested` is the declared tree that the compiler,
`shipped_code`, the image decoder and an edit's pairing read.
`Heap::register_mixin` records the children's ids in declaration order in
a new column `Tables::nested` and **takes the `Rc`s out of the mixin**, so
`nested` becomes a cell they can be taken from. This rests on a mixin
being registered in at most one heap; the plan establishes it and a
`debug_assert!` holds it.

After registration every reader reads `Heap::nested_of(id)` and the
current version through `mixins[id]`: the lookup by name in `dispatch.rs`,
`nested.rs`, the install's `layout_may_change`, the reflection leaves,
`kernel.rs`, `shipped_world.rs`, the image encoder and `assembled_text`.
`known_mixin_id` on a superseded version and the two detours "to the
current version through the id" in the encoder and `assembled_text` go.
An install replacing a mixin under its old id gives the fresh version its
own `nested` row: paired children keep their ids, a child without a
counterpart gets a fresh one.

## The marker per creation

`Mutator` gets a counter `next_creation_serial: u64`. `begin_creation`
takes the next serial, writes it into the slot as a `SmallInteger` and
keeps it in `NestedClassCreation`. `is_creation_in_progress` asks whether
the slot holds a `SmallInteger`. `end_creation` still looks the slot up in
the current layout (`current_cache`), since an install's migration can
carry the marker to another index, and resets it to `nil` only while it
holds its own serial -- a replacement's creation underway at the same
index holds another.
`after_clause` and `cache_slot` take any other serial for another creation
underway and raise the cyclic-definition error as today. An image that
writes such a value into a hidden slot still raises it on access.

## Tests

Newspeak programs run under both lowerings; helpers such as `expect_ok`
are copied from neighbours.

- **Allocators.** Unit tests of the free sets, the range list (coalescing,
  remainder, truncation) and the guard allocator, its exhaustion included:
  `NONE` and a requested collection.
- **Bounded growth.** A loop that edits a method a hundred times through a
  MirrorBuilder, installs, calls it and requests a full collection: the
  tables, `superseded`, `successors`, `sites` and the application cache are
  back at their size after the first round, read through test-only
  accessors.
- **Dynamic superclasses.** A program that creates many applications and
  lets them die: guard ids are reused, and a site that cached an old guard
  misses rather than hitting the new application.
- **Roots that must survive.** An activation entered at the old level
  before an install runs on across a collection, `successors` and
  `entered_level` intact; a reified mixin is `==` to itself after a
  collection; a host's `LoadedModule` keeps its id; a closure of a
  superseded version runs after the collection.
- **The marker.** A superclass clause forwards its enclosing object to one
  with a creation of its own underway at the same index; a cyclic
  definition after that point is now diagnosed.
- **Stress.** `NS_NEXT_STRESS_COLLECT` runs the pass at every safepoint;
  the batch ends with the `end-of-batch` gates.

## Documentation

`object-model.md`, "An application has two ids" and "The nested-class
cache slot" (its "What remains" goes); `memory.md`, "Weak references" and
the paragraph on the application cache; `liveness-and-reflection.md`,
"What the id has to carry" (and "Planned: road step 5b2c" goes);
`heap/tables.rs`'s module comment; `OPEN-WORK.md`'s "Next" and the items
this closes; `PROGRESS.md`'s entry.
