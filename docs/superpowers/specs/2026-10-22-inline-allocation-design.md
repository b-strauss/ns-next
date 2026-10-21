# Inline allocation -- design

Road step 6. Decided with the user on 2026-10-22. Temporary: at the end of
the batch its content moves into the living design documents
(`memory.md`, `object-model.md`, `execution.md`) and this file is deleted.

## The problem

Every allocation goes through one path today: a typed allocator, then
`Heap::allocate_object` (`heap.rs`), which looks up the header's guard
(`header_dispatch_id`: the class's application field, the application
table, `dispatch_ids.instances`), then `Heap::allocate_block`, which decides
young or old, bumps, compares against the scavenge threshold, writes the
overflow word when needed, the header, the class word and a nil fill, and
remembers an old allocation. About 15-18 conditional branches and two calls
that are not inlined, for every object, whatever its kind.

Around that path the four allocating operations pay more than the bump:

- **`MakeClosure`** looks up the block's code id in a `HashMap` keyed by the
  `Rc` pointer (`Heap::register_code`) for every closure, and the first
  `MakeClosure` of an activation reifies its frame, which looks the
  activation's code id up the same way (`frame_code_id`).
- **`new`** is a send; `decide_factory` → `decide_instantiation` →
  `instantiate_allocating` clones the application's `Rc`, checks the
  refusals, allocates (reading the application a second time for the
  guard), copies the arguments (`to_vec`), and walks the whole superclass
  chain twice (`climb_levels`, `descend_levels`) to find which levels have
  a clause message or initializer code -- a fact of the application, the
  same for every instantiation.
- **`Array new:`** (`builtins/collections.rs`, `allocate`) builds a Rust
  `Vec` of `length` nils, allocates the array nil-filled, and copies the
  `Vec` over it. `MakeArray` and `MakeVector` go through a temporary `Vec`
  too.
- **A boxed `Float`** (`Mutator::new_float`, outside the `SmallFloat64`
  range only) goes the general path for one 64-bit field.

## What the prior art does

Every system's fast path is one compare of the new top against a limit,
with the header precomputed and the identity hash left 0 until first
requested.

- **Cog/Spur** (`prior-art/opensmalltalk-vm/src/spur64.cog/`). The
  interpreter's general allocator compares `freeStart + numBytes` against
  `scavengeThreshold` once and, past it, sets `needGCFlag`, forces an
  interrupt check and allocates old (`gcc3x-cointerp.c:51349-51358`); `new`
  and `new:` nil-fill. The `ee` allocators for closures, floats and
  contexts bump into the reserve past the threshold, schedule a scavenge,
  and leave the body to the caller's explicit stores (`:48869-48918`);
  `pushFullClosure` is inlined into the bytecode case (`:10770-10875`). The
  JIT's `genPrimitiveNew` compares against the threshold and fails to the C
  primitive (`cogitX64SysV.c:16456-16608`); `genCreateFullClosure` bumps,
  compares, and on overflow calls `ceScheduleScavengeTrampoline`
  (`:19024-19103`); `genInstantiateFloatValue` likewise (`:15181-15256`).
- **Strongtalk** (`prior-art/strongtalk/vm/`) bumps eden with one compare
  (`memory/space.hpp:143-152`) and, on failure, scavenges at the
  allocation site (`memory/universe_more.cpp:55-70`); its generated
  `primitiveNew0..9` and `allocateBlock` stubs store the mark and klass
  and unrolled nils (`prims/behavior_prims_gen.cpp:25-66`).
- **Primordial Soup** (`prior-art/primordialsoup/vm/heap.h:373-387`) checks
  the large-object size and the top, and scavenges at once on failure
  (`heap.cc:157-178`).
- No system fuses a Newspeak factory's initializers into the allocation:
  the factory is `basicNew` followed by an ordinary send
  (`prior-art/newspeak/Newspeak2SqueakCompilation.ns:1991-2020`, SOMns's
  `MixinBuilder.assemblePrimaryFactoryMethod`).

Strongtalk and Primordial Soup collect inside the allocator, which ns-next
does not allow (collection only at a safepoint, `memory.md`, "Where a
collection can happen"). Cog's shape -- an allocation only requests a
collection -- is the one ns-next already follows.

## Decisions

1. **The fast path is one compare against the scavenge threshold; every
   other case is today's `allocate_block`, made cold.** The space holds an
   absolute limit, `fast_limit = min(eden.start + scavenge_threshold,
   eden.end - REGION_SLACK)`. A fast allocation computes the new top,
   compares it against `fast_limit`, and writes the top back; past it, the
   typed allocator calls `allocate_block` (`#[cold] #[inline(never)]`),
   which keeps every rule it has today: bumping into the reserve and
   requesting a scavenge, eden spent goes old born remembered, a large
   object goes old, the collection trigger. Because the limit lies below the
   threshold, the fast path never requests anything. (Approach A, chosen
   over bumping into the reserve on the fast path, which needs a second
   compare and the request inline, and over inlining `allocate_block`
   itself, which keeps its branches and cannot precompute the header.)
2. **The bump cursor lives in `ns-memory`.** `Reservation` holds one cursor,
   `(top, limit)`, and the space's eden top is that cursor's top, not a
   second copy of it. `set_bump_limit` (the cold path) checks the limit lies
   within the committed prefix; `bump(words) -> Option<&mut [u64]>` is one
   compare and answers a slice of exactly `words` words built with
   `from_raw_parts_mut`, sound because the cursor never points past the
   committed prefix; `bump_array::<COUNT>() -> Option<&mut [u64; COUNT]>`
   does the same for a fixed size, so a fixed-size object's stores carry no
   index check. `decommit`, and anything else that shrinks the committed
   prefix, clamps the cursor, so no safe call leaves it pointing at
   uncommitted memory. The `unsafe` stays in `ns-memory`, beside
   `words_mut` and `prefix_mut`.
3. **`fast_limit` is recomputed in one place**, `Space::refresh_fast_limit`,
   called by everything that changes eden's start, its top's base, the
   threshold or `allocating_old`: a scavenge, a move (the move stress
   mode's shift), a compaction, `set_scavenge_threshold`,
   `set_allocating_old`. While the image decoder allocates old,
   `fast_limit` is `eden.start`, so every fast bump fails and the decoder
   allocates as today, old and not remembered.
4. **Header templates.** A template is the precomputed header word (guard,
   format, word count), and whether the object is small enough for the fast
   path when its size is fixed. An instance's template lives on its
   `MixinApplication` row, computed when the row is created: the row's
   `dispatch_ids` and `slot_count` never change after that (an edit makes a
   new row). The class word comes from the class object per allocation,
   since a class object is per enclosing instance. The `Closure`, `Float`
   and `Frame` templates live with the builtin classes on the `Mutator`,
   set where `refresh_immediate_dispatch_ids` runs (the boot, an image
   load). A size known only at run time (an array, a frame) is inserted
   into the template when it is written; an object that needs an overflow
   word (255 words or more) takes the cold path, so the fast path knows
   only one-word headers.
5. **What the fast path writes.** The header and the class word, always.
   Instances and arrays are nil-filled. A closure, a frame and a float store
   every field explicitly and are not filled first, as Cog's `ee`
   allocators. No safepoint and no read lies between the bump and the last
   field's store; a debug check after each explicit fill asserts no
   `header::POISON` word is left in the new object. The identity hash stays
   0. The `heap` module stays the one writer of a header: two entries, one
   template.
6. **`new`: one read of the application, and a precomputed fact.** The
   application row carries, besides its template,
   `instantiates_without_code`, computed with the row and never changed:
   no level of its application chain needs initializer code, every level's
   superclass clause names its superclass's primary factory selector with
   no arguments, and every level above the first takes no factory
   arguments. `decide_instantiation` reads the row once (copied out, no
   `Rc` clone, no second lookup for the guard). When the row's fact holds,
   the arguments match the class's factory arity, and no class object of
   the chain is a level a `<:` clause added, `new` is a bump, a nil fill
   and `Step::Value`, with no argument copy and no walk. The last condition
   is read off the class objects per instantiation, two field reads a
   level, because it is a fact of the class objects and not of the
   application: a class object can name another declaration's applied
   level as its superclass, and the application key does not record it.
   Every other instantiation walks the chain exactly as today. A debug
   build runs the full walk after every fast instantiation and asserts it
   answers the same instance as a value. The refusals
   (`instantiation_refusal`, and `class_has_value_refusal`, which belongs to
   the class object) still come first. Order is unchanged: a `<:` clause's
   message runs with the applying class's factory arguments just before the
   initializer of the level it adds.
7. **A closure's code id without the map.** `CompiledCode` carries a
   one-entry cache, `Cell<(table_epoch, CodeId)>`. `table_epoch` identifies
   the heap's code table: unique across every heap of the process (a
   process-wide counter) and renewed whenever a full collection frees code
   rows. A hit skips `register_code`'s map; a miss runs it and refills the
   cache. An id from another heap, or a freed one, is therefore never used
   -- the rule that a handle means nothing against a heap that did not mint
   it, applied to ids. The closure is bumped with its template and its code
   id, captured frame, feedback base and copied registers stored
   explicitly; the frame of an activation's first reify is bumped too,
   format `Frame`, its word count from `register_count`, its code id from
   the same cache.
8. **Arrays without a `Vec`.** `Array new:`'s leaf calls
   `Heap::allocate_array_nil(class, length)`: below the large-object
   threshold a bump and a nil fill, with no capacity probe, since eden is
   committed; at or above it the size is first asked of `Heap::fields_fit`,
   as `ByteArray new:` and `WeakArray new:` already are, and refused with
   the same `ArgumentError` as today, then allocated old. A length the
   probe passes today but the reservation cannot hold therefore becomes an
   `ArgumentError` instead of an abort. `MakeArray` writes its registers
   straight into the fresh array; `MakeVector` is a bump and a nil fill.
   The overcommit item in `OPEN-WORK.md` stays open: no probe can tell
   whether an overcommitting system will later grant the pages.
9. **A boxed `Float`** is `bump_array::<3>()` with the `Float` template:
   header, class, bits.
10. **`vm.rs` does not change.** Its arms call the same functions as today;
    every change is below them, so the run loop's register assignment and
    its size against the inlining threshold stay as measured
    (`execution.md`, "Where the 3% goes").

## Invariants the path touches

- **Born remembered**: only `allocate_block` allocates old, so every old
  allocation outside the decoder is still remembered; the fast path makes
  young objects only, whose initializing stores need no barrier.
- **Collection only at a safepoint**: the fast path requests nothing and
  never collects.
- **Poison**: the header and class word are written before any read; the
  debug check of decision 5 covers the explicit fills.
- **Origin guard and move epoch**: a handle is still minted only by
  `object_handle`.
- **The move stress mode**: `refresh_fast_limit` runs at every move of
  eden; `REGION_SLACK` stays free past the limit.
- **An image is untrusted input**: the decoder's allocation is unchanged
  (decision 3).
- **Readers of compiled code take the mixin**: the cache in decision 7 is
  an id, never a handle, and is checked against its heap's epoch before
  every use.

## Testing

- **Equivalence per kind**: an instance, a closure, a frame, a float,
  `Array new:`, `MakeArray` and `MakeVector` made by the fast path are word
  for word what `allocate_block` makes -- header, class word, fields.
- **The bump's edges**: exactly at the limit the fast path succeeds; one
  word past it the allocation takes the cold path and requests a scavenge;
  eden spent goes old and remembered; a large array goes old; under
  `allocating_old` old and not remembered; 255 words or more get an
  overflow word.
- **`ns-memory`**: `set_bump_limit` refuses a limit past the committed
  prefix; `decommit` clamps the cursor; no `bump` succeeds past the limit.
- **The code-id cache**: after a row is freed and its id reused, the cache
  answers the right new id; two heaps in one thread holding the same
  `CompiledCode` each get their own.
- **The instantiation plan**: a class with no initializer code answers
  `Step::Value` with no activation; the `<:` clauses and initializers keep
  their order (the existing goldens, `mixin_initializer_order.ns` among
  them, pin it); refusals come before any allocation.
- **`Array new:`** of a length just past the reservation's capacity is an
  `ArgumentError`, not an abort.
- **The gates**: every golden under both lowerings; the stress modes,
  `images`, `caches` and `benchmarks` at the end of the batch; the size
  tests in release.

## Measuring

`execution.md`'s protocol ("What removing the re-entries moved"): three
pinned release builds a side with the same explicit `RUSTFLAGS`, against
`master`; the eight micro sources, the four not in the tree written out
verbatim from "What spec-conformant lookup moved"; the sixteen Are We Fast
Yet programs. Each part -- the bump, the code-id cache, the instantiation
plan, the arrays -- is measured on its own, so a part that does not pay is
seen. After any change near the run loop, `dumpbin /disasm` confirms one
`vm::run` and no separate `run_stretch`.

## Documentation at the end of the batch

`memory.md`'s "Planned: inline allocation" becomes the present-tense section
on the built path, the cursor in `ns-memory` included; `object-model.md`
describes the instantiation plan; `execution.md` records the measurement;
`PROGRESS.md` gets the entry; `OPEN-WORK.md`'s "Next" moves to road step 7,
the "Factory entry carrying its bases" item stays (the plan carries no
feedback base, so the family-map probe per initializer push remains), and a
new item records clean blocks (a block that
needs neither `self`, outer registers, an implicit receiver nor `^` needs
no frame and could be one closure made once, as Cog's clean blocks are) as
a lowering change still to be weighed; `CLAUDE.md` names the bump cursor
among `ns-memory`'s contents.
