# Weak references — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sub-project 6, the last of the kernel completed to Primordial Soup's core API: `WeakArray`, `Ephemeron` and `WeakMap` with Primordial Soup's protocol, weak slots and ephemerons in the full collection and the scavenge, two image records, and `platform kernel garbageCollect`.

**Architecture:** Two new heap formats, `WeakPointers` (code 10) and `Ephemeron` (code 11). `pointer_ranges` stays the enumeration that forwarding, the moves and the remembered set's verification read; a new `strong_pointer_ranges` is what marking and the scavenger trace. Both collectors list the weak objects and the ephemerons with an unknown key while they trace, run an ephemeron fixpoint after their drain, mourn the ephemerons left (every field `nil`), and clear the dead words of the weak objects, all before compaction or the scavenge's end. `WeakArray` and `Ephemeron` are builtins; `WeakMap` is Newspeak over an `Array` of `Ephemeron`s. The finalizer is held and never run.

**Tech Stack:** Rust 1.98.0, the Newspeak core library (`crates/ns-runtime/core-library/`), goldens under `goldens/`, `cargo-nextest`.

**Spec:** `docs/design/core-library.md`, "Planned: sub-project 6, weak references" (designed with the user on 2026-10-16). Read it before Task 1. Its decisions are settled; this plan does not reopen them. Prior art for the collector: `prior-art/primordialsoup/vm/heap.cc:308-339` (scavenge), `627-682` (mark-sweep), `848-1020` (the ephemeron and weak passes); for the protocol `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:852-861` (`Ephemeron`), `2553-2598` (`WeakArray`), `2599-2718` (`WeakMap`); for the tests `corpus/modules/KernelWeakTests.ns`. Read them; copy no code (`CLAUDE.md`, "Copying code out of `prior-art/` is a licensing decision").

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates", group `commit`, and `images` for Task 6) run before each commit, through `cargo xtask gates commit` (`cargo xtask gates commit images` for Task 6). The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 7, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `weak-references` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `weak_objects`, `ephemerons`, `position`, `progressed`, never `w`, `eph`, `pos`.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). An existing golden's `.toml` does not change in this plan; if one would, stop and report. A new golden's `.toml` is written from the design before the code runs it, never copied from a run.
- **A golden asserts only what holds whether or not an extra collection ran**: the stress modes collect at their own safepoints. A strongly held object stays; an object held only weakly reads `nil` after `platform kernel garbageCollect`. Never assert that a weakly held object is still there before `garbageCollect`. An object meant to be held only weakly is made in a method of its own that stores it and returns, so no register of the running frame holds it.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt <the files edited>`; never at the repository root.
- Every member of a shipped module, public, protected or private, has a `///` comment to the standard in `CLAUDE.md` ("Every member of a shipped core-library module is documented"). No other implementation is named. `crates/ns-runtime/tests/core_library_documentation.rs` enforces the presence.
- Any change to `Kernel.ns`, `PlatformFactory.ns`, `CollectionsFactory.ns`, a `BuiltinClass` or the `MIRROR` table changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- **`Primitive` stays two bytes**: every task that adds a `Primitive` variant runs `cargo nextest run -p ns-runtime --release -E 'test(a_resolution_stays_the_size)'`.
- **The `MIRROR` table holds at most 256 rows** (`builtins/primitive.rs`, the `const _` assert after the table); 236 are used today and this plan adds 6, so 242. New rows are appended at the end, never inserted.
- **Every store into an existing object goes through `heap.rs`'s store accessors** (`CLAUDE.md`). The collector's own clearing -- setting a word to `nil` in the passes of Tasks 1 and 2 -- is a collector rewrite and stores no young object, so it needs no barrier; nothing else writes a weak word but `set_pointer_word`.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- Goldens are `main: platform args:` modules printing through `platform transcript`, formatted, commented with `///`; each new file follows its neighbours under `goldens/library/`. The new directory is `goldens/library/weak/`. Every golden runs under both lowerings automatically.

## Review Focus

1. **An old `WeakArray` in the remembered set naming a young object**: after a scavenge the word names the copy if the object survived, `nil` if it did not -- never the vacated nursery (a debug build poisons it) -- and the weak array leaves the remembered set once no word of it names a young object. Pinned by Task 2's `an_old_weak_array_forgets_a_young_object_and_follows_a_surviving_one`.
2. **An ephemeron whose key is reached only through its own value, or through a chain of other ephemerons' values**: collected whole, by both collectors, 512 deep. Pinned by Task 1's and Task 2's chain tests and Task 4's golden `a_chain_of_ephemerons_dies_together`.
3. **An ephemeron whose key is alive keeps its value alive although nothing else reaches the value**, and an ephemeron with an immediate or `nil` key holds its value strongly. Pinned by Task 1's `an_ephemeron_with_a_live_key_keeps_its_value` and Task 4's golden.
4. **`WeakMap` after `removeKey:` and re-insertion of a key further along the same probe chain** answers one entry, the latest value, and `removeKey:` then leaves none. Pinned by Task 5's golden `a_weak_map_reinserts_without_duplicating`.
5. **A crafted image**: `OBJECT_WEAK_ARRAY` on a class other than `WeakArray`, `OBJECT_EPHEMERON` on a class other than `Ephemeron` or with another field count, a count past the bytes that follow: each refused with an error, never a panic or an allocation the bytes do not bound. Pinned by Task 6's image tests.

## The leaves, in one table

| Task | Row | Reported | Family |
|---|---|---|---|
| 3 | `weakArrayClass:new:ifFail:` | `new:` | `Primitive::Weak(WeakPrimitive::Allocate)` |
| 3 | `weakArraySize:ifFail:` | `size` | `Weak(WeakPrimitive::Size)` |
| 3 | `weakArray:at:ifFail:` | `at:` | `Weak(WeakPrimitive::ElementAt)` |
| 3 | `weakArray:at:put:ifFail:` | `at:put:` | `Weak(WeakPrimitive::ElementAtPut)` |
| 3 | `garbageCollect:ifFail:` | `garbageCollect` | `Weak(WeakPrimitive::Collect)` |
| 4 | `objectIsValue:ifFail:` | `key:` | `Weak(WeakPrimitive::IsValue)` |

A leaf declines (answers `None`, so the failure block runs) only when its receiver has the wrong shape -- a user subclass of `WeakArray` carries no weak payload; every other refusal is an `ArgumentError` the leaf raises itself in `Array`'s texts (`builtins/collections.rs`'s `out_of_range` and `not_an_integer`, `Array new:`'s three texts with "an Array of {n} elements" replaced by "a WeakArray of {n} elements").

## The error texts

| Where | Text |
|---|---|
| `Ephemeron>>key:` with a value other than `nil` | `key: requires an object that is not a value, got {argument}` |
| `WeakMap>>at:put:` with a value, `nil` included | `at:put: requires a key that is not a value, got {argument}` |

`{argument}` is `Heap::describe_argument`'s text, as every `ArgumentError` of the kernel.

---

### Task 1: The two formats and the full collection

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs` (`Format`, `from_code`, the module doc, the field test)
- Modify: `crates/ns-runtime/src/heap/object.rs` (`pointer_ranges`, new `strong_pointer_ranges`, the module doc)
- Modify: `crates/ns-runtime/src/heap/mark.rs` (`Marker`'s two lists)
- Modify: `crates/ns-runtime/src/heap.rs` (allocation and accessors, `drain_marking`, new `finish_marking`, `collect_for_test`, `object_shape`, `describe`, the instance-field accessors, tests)
- Modify: `crates/ns-runtime/src/heap/records.rs` (`ObjectShape::WeakArray`, `ObjectShape::Ephemeron`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`mark_everything_reachable` calls `finish_marking`)

**Interfaces:**
- Produces, in `header.rs`: `Format::WeakPointers = 10`, `Format::Ephemeron = 11`; `from_code` maps 10 and 11; `unused_limit` is 0 for both.
- Produces, in `object.rs` (`pub(in crate::heap)`): `fn strong_pointer_ranges(block: &[u64], key_survives: impl FnOnce(u64) -> bool) -> [Range<usize>; POINTER_RANGES]` -- the class word alone for `WeakPointers`; for `Ephemeron` with at least one field, the class word alone unless `key_survives(key_bits)`, every field otherwise; an `Ephemeron` with no field is traced as `FixedPointers`; `pointer_ranges(block)` for every other format. `pointer_ranges` gives both new formats every field, as `FixedPointers`.
- Produces, in `heap.rs`:
  - `pub(crate) fn allocate_weak_array(&mut self, class: ClassReference<'gc>, size: usize) -> ObjectReference<'gc>` -- every word nil;
  - `pub fn weak_array_size(&self, object) -> Option<usize>`, `pub fn weak_array_element(&self, object, index: usize) -> Option<Value<'gc>>`, `pub fn weak_array_set_element(&mut self, object, index: usize, value: Value<'gc>) -> bool` (through `set_pointer_word` with `Format::WeakPointers`, so the barrier runs), `pub fn weak_array_elements(&self, object) -> Option<Vec<Value<'gc>>>`;
  - `pub(crate) fn allocate_ephemeron(&mut self, class: ClassReference<'gc>, field_count: usize) -> ObjectReference<'gc>` -- every field nil;
  - `instance_field`, `set_instance_field`, `instance_field_count` and `instance_fields` accept `Format::Ephemeron` as they accept `FixedPointers`, the `FixedPointers` compare first so an ordinary object's slot pays nothing more;
  - `pub(crate) fn finish_marking(&mut self, marker: &mut mark::Marker)` -- the fixpoint, the mourning and the clearing below;
  - `object_shape` answers `ObjectShape::WeakArray` and `ObjectShape::Ephemeron`; `describe` answers "a WeakArray" and "an Ephemeron" as it answers other kernel objects.
- Produces, in `mark.rs`: `Marker { work, weak_objects: Vec<NonZeroU64>, ephemerons: Vec<NonZeroU64> }`.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`, next to `a_symbol_answers_its_symbol_across_collections_and_moves_and_a_pruned_one_leaves_no_entry`, with the helpers there (`named_class`, `collect_for_test`, `rooted`, `move_bare_heap`):
  - `a_weak_array_keeps_a_strongly_held_object_and_forgets_the_rest`: a weak array of 3 holding an object also rooted, an object nothing else holds and a `SmallInteger`; after `collect_for_test` with the weak array and the first object rooted, the elements are that object (forwarded: `same` as the rooted one), `nil` and the integer.
  - `an_ephemeron_with_a_live_key_keeps_its_value`: an ephemeron of 3 fields, key rooted, value an object nothing else holds; after the collection, the value is still there and its own field still reads what it was given.
  - `an_ephemeron_with_a_dead_key_is_mourned`: key held by nothing but the ephemeron, the ephemeron rooted; after the collection all three fields are `nil`.
  - `an_ephemeron_with_an_immediate_or_nil_key_holds_its_value`: keys `nil` and `SmallInteger 7`; the values survive.
  - `a_chain_of_ephemerons_whose_keys_only_their_values_reach_dies_whole`: 512 ephemerons, each rooted through a strong `Array`, the key of the first held by nothing, each value the key of the next; after the collection every field of every ephemeron is `nil`. A second variant roots the first key: every value survives.
  - `a_move_forwards_every_weak_word` (`#[cfg(debug_assertions)]`): a weak array and an ephemeron whose referents are rooted; after `move_bare_heap` the words name the moved objects.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(weak) | test(ephemeron)'` -- they do not compile (`allocate_weak_array` is missing).
- [ ] **Step 3: The formats.** Add the two variants with doc comments in the style of their neighbours; `from_code`'s arms `10 => Format::WeakPointers`, `11 => Format::Ephemeron`; extend `pointer_ranges`' first arm to `Format::FixedPointers | Format::IndexablePointers | Format::WeakPointers | Format::Ephemeron`. Write `strong_pointer_ranges`; its doc says why marking and the scavenger read it and forwarding does not (the design's "Marking traces strong words; forwarding rewrites every word").
- [ ] **Step 4: Marking.** In `drain_marking`, read `strong_pointer_ranges` with `key_survives` answering whether the key word is no object or names a marked one; push a visited `WeakPointers` object's offset on `marker.weak_objects`, and an `Ephemeron` whose key did not survive on `marker.ephemerons`. `finish_marking`:

```rust
pub(crate) fn finish_marking(&mut self, marker: &mut mark::Marker) {
    loop {
        let pending = std::mem::take(&mut marker.ephemerons);
        let mut progressed = false;
        for ephemeron in pending {
            if self.ephemeron_key_marked(ephemeron) {
                self.push_ephemeron_fields(ephemeron, marker);
                progressed = true;
            } else {
                marker.ephemerons.push(ephemeron);
            }
        }
        if !progressed {
            break;
        }
        self.drain_marking(marker);
    }
    for ephemeron in std::mem::take(&mut marker.ephemerons) {
        self.mourn_ephemeron(ephemeron);
    }
    for weak_object in std::mem::take(&mut marker.weak_objects) {
        self.clear_unmarked_words(weak_object);
    }
}
```

  An object is visited once, when its mark bit is set, so no ephemeron is listed twice. `mourn_ephemeron` writes `Word::NIL` into every field; `clear_unmarked_words` writes `Word::NIL` into every field naming an unmarked object. Both are collector rewrites (Global Constraints). Call `finish_marking` from `World::mark_everything_reachable` after `drain_marking`, and from `collect_for_test` before `prune_canonical_symbols`.
- [ ] **Step 5: Allocation and accessors** as in Interfaces; `object_shape`, `describe`, and the instance-field accessors.
- [ ] **Step 6: Run the tests**: `cargo nextest run -p ns-runtime -E 'test(weak) | test(ephemeron)'` -- PASS; then `cargo nextest run -p ns-runtime` -- PASS.
- [ ] **Step 7: Commit** after `cargo xtask gates commit`: `feat: weak arrays and ephemerons in the full collection`.

### Task 2: The scavenge

**Files:**
- Modify: `crates/ns-runtime/src/heap/scavenge.rs` (`scan_object`, `drain`, the two lists, the passes)
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap::scavenge`, tests)

**Interfaces:**
- Consumes: `strong_pointer_ranges`, `Format::WeakPointers`, `Format::Ephemeron` (Task 1).
- Produces, in `scavenge.rs` (`pub(in crate::heap)`): `Scavenger::finish(&mut self)` -- the fixpoint, the mourning and the forwarding-or-clearing of weak words; `Heap::scavenge` calls it after `drain` and before `forward_weak_symbols`.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`, next to `a_symbol_held_only_by_young_garbage_is_pruned_and_a_kept_one_keeps_its_identity`, with `scavenge_bare` and `old_instance_class`:
  - `a_young_weak_array_forgets_a_young_object_and_follows_a_surviving_one`: young weak array rooted, two young objects, one rooted; after `scavenge_bare`, the word of the rooted one names its copy and the other is `nil`.
  - `an_old_weak_array_forgets_a_young_object_and_follows_a_surviving_one`: the weak array allocated old (`set_allocating_old(true)`), its words set through `weak_array_set_element` so the barrier remembers it; after the scavenge, as above, and after a second scavenge with the surviving one tenured (`tenure_all`), `heap.remembered` no longer holds the weak array.
  - `a_young_ephemeron_whose_key_survives_keeps_its_value` and `a_young_ephemeron_whose_key_dies_is_mourned`.
  - `an_old_ephemeron_with_an_old_key_keeps_a_young_value`: the key is old, so it survives a scavenge; the young value is copied.
  - `a_chain_of_young_ephemerons_dies_whole_in_a_scavenge`: as Task 1's chain, all young, 512 deep.
  - `the_remembered_set_verifies_after_a_scavenge_of_weak_objects` (`#[cfg(debug_assertions)]`): after each test scenario above, `heap.verify_remembered_set()` does not panic.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(weak) | test(ephemeron)'`.
- [ ] **Step 3: Implement.** `scan_object` reads `strong_pointer_ranges` with `key_survives` answering whether the key is no object, old, or a young object whose header is `Forwarded`; it pushes a `WeakPointers` object's header offset on `self.weak_objects` and an `Ephemeron` whose key did not survive on `self.ephemerons`, and answers `names_young` over the words it scanned. An old weak object from the remembered set keeps its remembered bit only if `finish` finds a young word in it afterwards: `Heap::scavenge` does not push it back itself (the loop over `old_remembered` lets `scan_object` answer `false` for it and `finish` re-remembers it). `finish`:
  1. Until a round scans none: each listed ephemeron whose key has since survived is scanned with every field (`scan_object` with the key known to survive), and `drain` runs again; an old ephemeron so scanned that names a young object is remembered.
  2. Each ephemeron left is mourned: every field `Word::NIL`.
  3. Each listed weak object: every word naming a young object is rewritten to the copy when the object's header is `Forwarded`, and to `Word::NIL` otherwise; an old weak object that still names a young object afterwards is remembered (its header bit and `self.remembered`), unless its bit is already set, so that no object is in the set twice; the same holds for an old ephemeron step 1 remembers.
- [ ] **Step 4: Run the tests**: `cargo nextest run -p ns-runtime -E 'test(weak) | test(ephemeron)'` -- PASS; then `cargo nextest run -p ns-runtime` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: weak arrays and ephemerons in the scavenge`.

### Task 3: `WeakArray` and `garbageCollect` in the kernel

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::WeakArray`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name`, the accessor lists)
- Create: `crates/ns-runtime/src/builtins/weak.rs` (`WeakPrimitive` and its leaves)
- Modify: `crates/ns-runtime/src/builtins/mod.rs`, `builtins/primitive.rs` (`Primitive::Weak`, the five rows of Task 3)
- Modify: `crates/ns-runtime/src/heap.rs` (`request_collection` widened to `pub(crate)` if it is not)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`WeakArray`, `Kernel>>garbageCollect` or its equivalent the facet calls)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (`KernelNamespace>>WeakArray`, `KernelNamespace>>garbageCollect`)
- Modify: `crates/ns-runtime/core-library/platform/CollectionsFactory.ns` (the `Collections` facet's `WeakArray`)
- Modify: `crates/ns-runtime/tests/platform.rs` (the facets answer `WeakArray`; `IdentitySet` stays not understood)
- Create: `goldens/library/weak/` goldens below
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)

**Interfaces:**
- Consumes: `allocate_weak_array`, `weak_array_*` (Task 1).
- Produces: `BuiltinClass::WeakArray`; `WeakPrimitive { Allocate, Size, ElementAt, ElementAtPut, Collect }` (Task 4 adds `IsValue`); `platform kernel WeakArray`, `platform collections WeakArray`, `platform kernel garbageCollect`.

- [ ] **Step 1: Write the goldens** (the `.toml` from the design, before the code runs them):
  - `a_weak_array_answers_its_size_and_elements.ns`: `new: 0` and `new: 3` sizes; `at:put:` answers the value; `at:` reads it back; a fresh one reads `nil`; `isEmpty`, `do:`, `keysAndValuesDo:`, `copyWith:`; `isKindOfArray` and `isKindOfWeakArray` answer `true`; `platform collections WeakArray == platform kernel WeakArray`.
  - `a_weak_array_refuses_a_bad_index_or_size.ns`: `at: 0`, `at: 4` on three elements, `at: nil`, `at: 1.5`, `new: -1`, `new: nil`, each caught with `on: ArgumentError do:` and its `messageText` printed; `WeakArray new` caught as `MessageNotUnderstood`.
  - `a_weak_array_forgets_an_object_after_a_collection.ns`: a strongly held object, an object stored by a helper method and dropped, a `SmallInteger`, a `String` made by a helper and dropped; after `platform kernel garbageCollect`, prints the strongly held one, `nil`, the integer, `nil`.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-cli -E 'test(weak)'`.
- [ ] **Step 3: The builtin and its leaves.** `WeakArray` in `Kernel.ns` beside `ByteArray`, superclass `Collection`, factory not `new` -- a class-side `new: size` over `weakArrayClass:new:ifFail:` and the instance factory named so that `WeakArray new` is not understood (as `DataView over:` declares its own) -- with `at:`, `at:put:`, `size` over the leaves, each failure block `[^super …]` (`core-library.md`, "The shape of a kernel method"), and `do:`, `keysAndValuesDo:`, `isEmpty`, `copyWith:`, `isKindOfArray`, `isKindOfWeakArray` in Newspeak; every member documented. The leaves in `builtins/weak.rs`, modelled on `builtins/bytes.rs`'s `Allocate`, `Size`, `ElementAt` and `ElementAtPut`, gating on `class_builtin(class) == Some(BuiltinClass::WeakArray)`; `decide_factory` gets no `WeakArray` arm, since `new` is not its factory. `Collect` calls `heap.request_collection()` and answers the receiver: the safepoint at the return from the kernel method serves it before the caller's next instruction (`interpreter/safepoint.rs`); the third golden pins that this holds. If the method compiles to a fast path without an activation, so that no safepoint follows, stop and report rather than adding a safepoint to `vm.rs` (`CLAUDE.md`, "`vm.rs` holds no dispatch logic").
- [ ] **Step 4: The facets.** `KernelNamespace>>WeakArray` and `>>garbageCollect`, the `Collections` facet's `WeakArray`, each documented; the on-demand rule exempts the kernel facet, and `platform collections WeakArray` is the corpus's (`Brazil.ns:24`), named in its comment. Update `tests/platform.rs`: the facets answer `WeakArray`, and `platform_collections_does_not_invent_an_identity_set` still holds.
- [ ] **Step 5: Regenerate the fixture** (Global Constraints) and run the size test (`Primitive` stays two bytes).
- [ ] **Step 6: Run**: `cargo nextest run --workspace` -- PASS, the three goldens under both lowerings among them.
- [ ] **Step 7: Commit** after `cargo xtask gates commit`: `feat: WeakArray and garbageCollect in the kernel`.

### Task 4: `Ephemeron`

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::Ephemeron`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs`, `interpreter/instantiate.rs` (`decide_factory`'s `Ephemeron` arm)
- Modify: `crates/ns-runtime/src/builtins/weak.rs`, `builtins/primitive.rs` (the `objectIsValue:ifFail:` row)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Ephemeron`), `PlatformFactory.ns` (`KernelNamespace>>Ephemeron`)
- Create: goldens under `goldens/library/weak/`
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: `allocate_ephemeron`, the instance-field accessors (Task 1); `WeakPrimitive` (Task 3); `interpreter::instantiate::is_a_value` (immediates answer `true`).
- Produces: `BuiltinClass::Ephemeron`; `platform kernel Ephemeron`.

- [ ] **Step 1: Write the goldens:**
  - `an_ephemeron_answers_its_key_value_and_finalizer.ns`: `Ephemeron new`'s three are `nil`; `key:`, `value:`, `finalizer:` answer the receiver and read back; `key: nil` is accepted; `key: 3`, `key: 'text'`, `key: 1.5` caught as `ArgumentError`, their texts printed; `isKindOfEphemeron`.
  - `an_ephemeron_keeps_its_value_while_its_key_lives.ns`: key held strongly, value only by the ephemeron; after `garbageCollect` the value is still there; then the key dropped (a helper made it, the strong holder's slot set to `nil`), `garbageCollect`, all three fields `nil`.
  - `a_chain_of_ephemerons_dies_together.ns`: `KernelWeakTests`' 512-deep chain, written here: each ephemeron held in a strong `Array`, each value the next one's key, the first key dropped; after `garbageCollect` every key reads `nil`.
  - `a_subclass_of_ephemeron_holds_strongly.ns`: a user subclass's key dropped, `garbageCollect`, the value still there.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** `Ephemeron` in `Kernel.ns`: three private slots, in this order, the key first (the collector reads field 0 as the key) -- `storedKey`, `storedValue`, `storedFinalizer` -- and public `key`, `key:`, `value`, `value:`, `finalizer`, `finalizer:`, `isKindOfEphemeron`; `key:` asks `objectIsValue:ifFail:` and signals the `ArgumentError` of "The error texts" for a value other than `nil`. `decide_factory` gets a `Some(BuiltinClass::Ephemeron)` arm that instantiates as `decide_instantiation` does but allocates with `allocate_ephemeron`, so that the instance has format `Ephemeron` -- the builtin is the class's own, so a user subclass takes the ordinary path and holds strongly. The leaf `IsValue` answers `true` for an immediate and for an object `is_a_value` accepts.
- [ ] **Step 4: Regenerate the fixture, run the size test, run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: Ephemeron in the kernel`.

### Task 5: `WeakMap`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`WeakMap`), `PlatformFactory.ns` (`KernelNamespace>>WeakMap`)
- Create: goldens under `goldens/library/weak/`
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: `Ephemeron`, `objectIsValue:ifFail:` (Task 4).
- Produces: `platform kernel WeakMap`.

- [ ] **Step 1: Write the goldens:**
  - `a_weak_map_answers_what_it_was_given.ns`: `at:` of an absent key answers `nil`; `at:put:` answers the value; `at:`, `includesKey:`, `removeKey:` (answers the value, then `nil`); 5000 keys held strongly in a `List`, each read back (growth past 4097, as `KernelWeakTests` asks); `isKindOfWeakMap`; `at: 3 put:`, `at: nil put:` and `at: 'text' put:` caught as `ArgumentError` with the text of "The error texts".
  - `a_weak_map_forgets_an_entry_whose_key_is_collected.ns`: two keys, one held strongly; after `garbageCollect`, `includesKey:` of the held one is `true`, and a third key added afterwards reuses the space (`at:` answers its value).
  - `a_weak_map_reinserts_without_duplicating.ns`: keys forced into one probe chain (a map of capacity 256 receives enough keys that some share a chain; the golden finds two such by `identityHash bitAnd: 255`), the first removed, the second re-put: `at:` answers the new value, `removeKey:` removes it, `at:` answers `nil`.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** `WeakMap` in `Kernel.ns`: a strong `Array` of 256 entries, power-of-two capacity, a `used` count, linear probing from `1 + (key identityHash bitAnd: mask)`, matching by `==`. `at:put:` first searches the whole chain for the key; only if it is absent does it reuse the first dead entry (an ephemeron whose key is `nil`) it passed, or else allocate an `Ephemeron`, count it, and grow past 75% used. Growing counts the live entries: past half the capacity it doubles, otherwise it rehashes at the same size, dropping dead entries; it counts during the rehash, since a collection can kill entries meanwhile. `removeKey:` sets the entry's key and value to `nil`. Every member documented; no enumeration, no `size`.
- [ ] **Step 4: Regenerate the fixture, run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: WeakMap in the kernel`.

### Task 6: The image

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_WEAK_ARRAY = 12`, `OBJECT_EPHEMERON = 13`, "Next free number: 14")
- Modify: `crates/ns-runtime/tests/opcode_tags.rs` (both pinned)
- Modify: `crates/ns-runtime/src/image/encode.rs` (the graph walk, the two records)
- Modify: `crates/ns-runtime/src/image/decode.rs` (first pass, second pass, the refusals), `heap/records.rs` (`PlaceholderFormat::WeakArray`, `PlaceholderFormat::Ephemeron`), `heap.rs` (`shape_placeholder`)
- Modify: `crates/ns-runtime/src/image/inspect.rs`
- Modify: `crates/ns-runtime/tests/image.rs`, `tests/image_truncations.rs`

**Interfaces:**
- Consumes: `ObjectShape::WeakArray`, `ObjectShape::Ephemeron`, `weak_array_elements`, `instance_fields` (Task 1); `BuiltinClass::WeakArray`, `BuiltinClass::Ephemeron` (Tasks 3, 4).

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`:
  - `a_weak_array_round_trips_its_strongly_reached_elements`: a program whose result holds a weak array with an object also held strongly, an object held only there and an integer; after encode and decode, the elements are the object, `nil` and the integer.
  - `an_ephemeron_round_trips_with_its_value_while_its_key_is_reached` and `an_ephemeron_whose_key_is_not_reached_is_written_mourned`.
  - `an_ephemeron_whose_key_is_reached_only_through_another_ephemeron_is_written_after_it`: the fixpoint in the encoder.
  - refusals: `a_weak_array_record_on_another_class_is_refused`, `an_ephemeron_record_on_another_class_is_refused`, `an_ephemeron_record_with_another_field_count_is_refused`, and both records cut at every byte in `tests/image_truncations.rs`'s style.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** The encoder's walk pushes the referents of an `Instance`'s and an `Array`'s fields as today; a `WeakArray`'s elements are not pushed, and an `Ephemeron`'s fields are pushed only once its key is reached (an immediate or `nil` key is reached). After the walk, repeat: each pending ephemeron whose key now has an id has its fields pushed and the walk resumes; until none does. Writing, a weak word names its object if the object has an id and `nil` otherwise, and an ephemeron left pending is written with every field `nil`. The decoder sizes both records from their own bytes, bounded by `bounded_count` like `OBJECT_ARRAY`, refuses each on the wrong class (`OBJECT_WEAK_ARRAY` only on the class whose builtin is `WeakArray`, `OBJECT_EPHEMERON` only on `Ephemeron`'s, with exactly its class's field count), and fills the placeholders allocated old. The inspector prints both.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS; the `images` profile: `cargo xtask gates commit images`.
- [ ] **Step 5: Commit**: `feat: weak arrays and ephemerons in the image`.

### Task 7: Documentation

**Files:**
- Modify: `docs/design/core-library.md` ("Planned: sub-project 6, weak references" becomes the present-tense section "Weak references", placed after "`ByteArray`, `StringBuilder` and the views"; item 6 of the list says built, with this plan's link; the table of the platform's facets names `WeakArray`, `Ephemeron`, `WeakMap` and `garbageCollect`)
- Modify: `docs/design/memory.md` ("Weak references": the two formats, `strong_pointer_ranges`, the passes in both collectors, replacing "Nothing else is weak"; "What the collector traces"; the summary list's "The canonical symbol table is the system's only weak structure")
- Modify: `docs/design/image-format.md` (the two records, the encoder's weak semantics)
- Modify: `docs/design/README.md` (the golden directory `library/weak/` in the table)
- Modify: `docs/OPEN-WORK.md` ("Next" moves on to road step 5; a new item: running the finalizer comes with the actors' event loop, the collector then firing an ephemeron -- keeping its key and value and queueing it -- rather than mourning it; the ephemeron fixpoint's rescan per round, if a measurement shows it matters)
- Modify: `docs/PROGRESS.md` (the milestone)
- Modify: `CLAUDE.md` (the `ns-runtime` paragraph: the heap's weak formats, one clause)

- [ ] **Step 1: Write the documents** in the present tense; read each section touched end to end and delete what is no longer true (`CLAUDE.md`, "Documentation other than a plan describes the current state").
- [ ] **Step 2: Commit** (Markdown only, no gates): `docs: describe the weak references`.
