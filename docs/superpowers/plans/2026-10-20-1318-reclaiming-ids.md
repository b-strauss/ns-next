# Reclaiming ids (road step 5b2c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5b2c: every per-heap table live editing grows is reclaimed during a full collection -- mixin applications with their guard ids and full-width indices, `successors`, superseded mixin versions, mixin ids no live thing names, compiled-code ids and the inline caches' feedback ranges -- and the nested-class cache slot's in-progress marker becomes one serial per creation.

**Architecture:** Marking gathers, in bit sets on the `Marker`, the ids that the marked class objects, closures and frames name. Between marking and compaction, beside `prune_canonical_symbols`, `Mutator::reclaim_ids` decides in a fixed order: first applications, then the caches, then mixin trees and superseded versions, then codes, then feedback families. Each table frees by setting a row to `None`. Its free indices go into a set, the smallest is minted first, and trailing free rows are truncated. A Rust holder keeps a mixin version and its tree alive through `Rc::strong_count`, so nothing has to list holders. Once a mixin is registered, its nested declarations are named by id in a table column: registration takes the `Rc`s out of the declared tree. A live module tree keeps all its ids, and every reader reaches the current version through `Heap::nested_of` and `Heap::mixin`.

**Tech Stack:** Rust 1.98.0, `cargo-nextest`, the Newspeak core library.

**Spec:** `docs/superpowers/specs/2026-10-20-road-step-5b2c-reclaiming-ids-design.md`. Read it and `docs/design/object-model.md` ("An application has two ids", "What a slot access checks", "The nested-class cache slot") and `docs/design/memory.md` ("Weak references", "Marking") before Task 1.

**Decided with the user on 2026-10-20** (not reopened; the spec's "Decisions"):

1. Caches are scrubbed selectively, only by a full collection that freed something.
2. The smallest free id is minted first, in every table, for guard ids and (first fit) for feedback ranges; trailing free rows are truncated.
3. When the guard ids run out, minting answers `NONE` and requests a full collection.
4. `CodeId`s are reclaimed.
5. `Mixin::nested` is taken over by the table on registration.
6. The in-progress marker is a serial `SmallInteger` per creation.
7. Feedback ranges are reclaimed.

**Refined while planning, stated here for review:**

- *A mixin id lives as long as its module tree.* The spec has a live id keep both its nested ids and its enclosing ids alive. Together those rules cover the whole tree: every declaration in it, plus each class side registered with an instance side as its enclosing mixin. So Task 5 decides liveness per tree. A tree is live when any of its ids is named by one of these: a live application, a gathered `APPLIED_CLAUSE`, a marked reified mixin object, or a Rust holder of its current version.
- *A Rust holder is counted against the structural holders.* A current version is held by its table row (1). A declared class side is also held by its instance side's `class_side` (1). Anything beyond that is a Rust holder. A superseded version is held by `superseded` (1), and a superseded instance side holds its class side, so dropping superseded versions repeats until nothing more drops.
- *`metaclass_mixins` is a memo, not a holder.* Its entries for the candidate-dead trees are removed before the strong counts are read. Otherwise the memo would keep every tree with a synthesized class side alive for ever.
- *Three more roots than the spec lists:*
  - An activation's `recorded_guard` keeps the application with that guard alive. A forwarded receiver could otherwise meet a reused guard that passes the own-slot check wrongly.
  - Every marked closure's `FEEDBACK_BASE` keeps its family alive.
  - So does every stack entry's `feedback_base`, since a closure names its family's range only by that number.
- *Eager registration of a tree.* Registration takes the children's `Rc`s, so `Heap::mixin_id` registers a mixin's whole declared subtree, preorder, whenever it registers the mixin. Class sides stay lazy: an instance side's `class_side` keeps holding its class side.
- *The pass lives on the `Mutator`.* `interpreter/reclaim.rs` holds `Mutator::reclaim_ids`, because the application cache, `metaclass_mixins`, the inline caches and the guard allocator are the mutator's. `heap/reclaim.rs` holds the heap's half: the gathered sets and the table freeing.

## Global Constraints

- **Every commit is green.**
  - Before each commit, run the per-commit gates in `CLAUDE.md` ("The gates") through `cargo xtask gates <groups>`, the groups each task names. Every task runs `commit`.
  - The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 7, by the controller (`finishing-a-batch` skill). An implementer does not run them, and a task's commit is work in progress until they have run.
- **Commits.** They are conventional (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`) and each ends with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- **Branch.** Work on `road-step-5b2c` in `D:\Workspace\ns-next`, checked out in the main working directory, with no worktree. The branch exists already.
- **Foreground.** Run every command in the foreground.
- **Names are written out** (`CLAUDE.md`): `free_indices`, `application_index`, `feedback_base`, `creation_serial`, never `idx`, `app`, `fb`, `serial_no`.
- **Nothing under `crates/ns-syntax` changes.** No wire tag, no image layout and no `opcodes.rs` entry changes either: the ids are per heap and not on the wire. If a task finds that an image names one of these ids, stop and report.
- **Program outcomes are pinned** (`CLAUDE.md`). No golden's `status`, `stdout` or `stderr` changes. If one would, stop and report.
- **Every store into an existing object goes through `heap.rs`'s store accessors.** The marker in Task 2 is written with `instance_set_field`, as today.
- **A handle means nothing across a move.** `reclaim_ids` runs between marking and compaction. It reads mark bits and ids and holds no handle past its return.
- **`vm.rs` holds no dispatch logic,** and the caches record what the lookup decided. Scrubbing only vacates entries; it decides nothing.
- **Testing rules.** Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` an `EvaluationResult`. Tests use their file's `expect_ok`. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- **Comments explain the decision, not the mechanics.** Documentation describes the current state.
- **Never judge performance from a debug build.** Task 4 adds a branch to the marking loop. If you want it measured, ask the controller.

## Review Focus

1. **A reused guard id never hits an old entry.** An application dies, and its guard is minted again for a different class. A send site, polymorphic block, global-cache entry or verified pair that cached the old guard must miss for the new class's receivers.
   - Pinned by Task 4's `a_reused_guard_misses_every_cache_entry_of_its_old_application`.
2. **An activation entered at a superseded level survives a collection.**
   - The program installs an edit while a method of the old version runs, collects, and lets the method go on writing its own slots.
   - `successors` and the application of `entered_level` must still be there, and the own-slot check must still pass.
   - Pinned by Task 4's `an_activation_at_a_superseded_level_runs_on_across_a_collection`.
3. **A reified mixin keeps its identity across collections while reachable, and a host-held mixin keeps its id.**
   - A mirror's reflectee is held in a slot across `collect_now`: it stays `==` to a fresh reification.
   - A host holding an `Rc<Mixin>` gets the same `known_mixin_id` after the collection.
   - Pinned by Task 5's `a_reachable_reified_mixin_keeps_its_identity` and `a_host_held_mixin_keeps_its_id`.
4. **A closure of a superseded version runs after its version was reclaimed from every table but its code.** The closure is held in a slot, the method is edited three times, and after `collect_now` the closure answers its old result.
   - Its `CodeId` must still name its body, and its feedback range must be its own.
   - Pinned by Task 5's `a_closure_of_a_superseded_version_runs_after_a_collection` and Task 6's `a_closure_keeps_its_feedback_range_across_a_collection`.
5. **Repeated editing stays bounded.**
   - The program edits, installs, calls and collects a hundred times. After that, every table is the size it was after the first round: `mixins`, `superseded`, `applications`, `successors`, `codes`, the application cache, `metaclass_mixins`, `sites` and the guard allocator's high-water mark.
   - Pinned by Task 6's `a_hundred_edits_leave_every_table_at_its_first_round_size`.

---

### Task 1: The allocators

**Files:**
- Create: `crates/ns-runtime/src/heap/ids.rs` (`FreeIndices`, `truncate_trailing`, `DispatchIdAllocator`)
- Modify: `crates/ns-runtime/src/heap.rs` (declare `mod ids;` beside the other submodules, `pub(crate) use ids::{DispatchIdAllocator, FreeIndices};`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`:
  - `next_dispatch_id: u32` becomes `dispatch_ids: crate::heap::DispatchIdAllocator`;
  - `mint_dispatch_ids` changes;
  - the two tests at `:2985-3017` change.
- Modify: `crates/ns-runtime/src/heap/world.rs:139` (the initializer)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`FreeRanges`; `fresh_range` takes from it)

**Interfaces:**
- Produces, in `crate::heap::ids`:

```rust
/// The free rows of one table: minted smallest first, so that a table
/// keeps its rows dense and can give its trailing ones back.
#[derive(Default)]
pub(crate) struct FreeIndices {
    free: std::collections::BTreeSet<u32>,
}

impl FreeIndices {
    /// The smallest free index, taken out of the set; `None` when none is
    /// free.
    pub(crate) fn take_smallest(&mut self) -> Option<u32>;
    /// Makes `index` free. A compiler-invariant violation panics: freeing
    /// an index twice.
    pub(crate) fn free(&mut self, index: u32);
    pub(crate) fn is_free(&self, index: u32) -> bool;
    pub(crate) fn len(&self) -> usize;
}

/// Pops `rows`' trailing `None`s and forgets their indices in `free`, so a
/// table whose last rows died gives their memory back.
pub(crate) fn truncate_trailing<T>(rows: &mut Vec<Option<T>>, free: &mut FreeIndices);

/// The guard ids: the smallest freed one first, else the next never
/// minted, below `DispatchId::LIMIT`.
pub(crate) struct DispatchIdAllocator {
    next: u32,
    free: FreeIndices,
}

impl DispatchIdAllocator {
    pub(crate) fn new() -> DispatchIdAllocator; // next = DispatchId::FIRST_MINTED
    /// `None` once every id below `LIMIT` is in use.
    pub(crate) fn mint(&mut self) -> Option<DispatchId>;
    /// Gives `id` back. `NONE`, `VACANT` and `FORWARDED` are never minted
    /// and never freed (`debug_assert!`).
    pub(crate) fn free(&mut self, id: DispatchId);
    /// One past the largest id ever minted, for the tests that bound
    /// growth.
    pub(crate) fn high_water(&self) -> u32;
    /// For the exhaustion tests: the next never-minted id is `next`.
    #[cfg(test)]
    pub(crate) fn set_next_for_test(&mut self, next: u32);
}
```

- Produces, in `inline_cache.rs` (private to the module, used by Task 6):

```rust
/// The free ranges of the site table, by start, never two adjacent.
#[derive(Default)]
struct FreeRanges {
    ranges: std::collections::BTreeMap<u32, u32>, // start -> size
}

impl FreeRanges {
    /// The first range of at least `size` sites, taken; its remainder stays
    /// free.
    fn take_first_fit(&mut self, size: u32) -> Option<u32>;
    /// Makes `start..start + size` free, merged with its neighbours.
    fn free(&mut self, start: u32, size: u32);
    /// Removes and answers the free range that ends at `end`, if one does:
    /// what `InlineCaches` truncates `sites` by.
    fn take_ending_at(&mut self, end: u32) -> Option<(u32, u32)>;
}
```

- [ ] **Step 1: Write the failing tests.** In `heap/ids.rs`, add a `#[cfg(test)] mod tests` with:

```rust
#[test]
fn the_smallest_free_index_is_taken_first() {
    let mut free = FreeIndices::default();
    free.free(7);
    free.free(2);
    free.free(5);
    assert_eq!(free.take_smallest(), Some(2));
    assert_eq!(free.take_smallest(), Some(5));
    assert_eq!(free.take_smallest(), Some(7));
    assert_eq!(free.take_smallest(), None);
}

#[test]
fn trailing_free_rows_are_truncated_and_forgotten() {
    let mut rows = vec![Some('a'), None, Some('c'), None, None];
    let mut free = FreeIndices::default();
    for index in [1, 3, 4] {
        free.free(index);
    }
    truncate_trailing(&mut rows, &mut free);
    assert_eq!(rows, vec![Some('a'), None, Some('c')]);
    assert!(free.is_free(1));
    assert!(!free.is_free(3) && !free.is_free(4));
    assert_eq!(free.len(), 1);
}

#[test]
fn a_freed_guard_is_minted_again_before_a_new_one() {
    let mut allocator = DispatchIdAllocator::new();
    let first = allocator.mint().expect("an id");
    let second = allocator.mint().expect("an id");
    allocator.free(first);
    assert_eq!(allocator.mint(), Some(first));
    assert_eq!(allocator.high_water(), second.bits() + 1);
}

#[test]
fn the_allocator_is_exhausted_below_twenty_two_bits() {
    let mut allocator = DispatchIdAllocator::new();
    allocator.set_next_for_test(DispatchId::LIMIT - 1);
    assert_eq!(allocator.mint(), Some(DispatchId::from_counter(DispatchId::LIMIT - 1)));
    assert_eq!(allocator.mint(), None);
    allocator.free(DispatchId::from_counter(DispatchId::LIMIT - 1));
    assert_eq!(allocator.mint(), Some(DispatchId::from_counter(DispatchId::LIMIT - 1)));
}
```

In `inline_cache.rs`'s tests module (create one at the end of the file if there is none, `#[cfg(test)] mod tests { use super::*; … }`), add:

```rust
#[test]
fn free_ranges_coalesce_and_fit_first() {
    let mut free = FreeRanges::default();
    free.free(10, 4);
    free.free(20, 3);
    free.free(14, 6); // joins 10..14 and 20..23 into 10..23
    assert_eq!(free.take_first_fit(5), Some(10));
    assert_eq!(free.take_first_fit(8), Some(15));
    assert_eq!(free.take_first_fit(1), None);
}

#[test]
fn the_free_range_ending_at_the_table_is_taken_for_truncation() {
    let mut free = FreeRanges::default();
    free.free(4, 2);
    free.free(30, 10);
    assert_eq!(free.take_ending_at(40), Some((30, 10)));
    assert_eq!(free.take_ending_at(40), None);
    assert_eq!(free.take_ending_at(6), Some((4, 2)));
}
```

Rewrite the two mint tests in `interpreter/mod.rs`:
- `the_first_mint_follows_the_reserved_guards` sets `mutator.dispatch_ids = DispatchIdAllocator::new()` instead of the counter.
- `minting_stops_below_twenty_two_bits_and_answers_none_once_exhausted` uses `mutator.dispatch_ids.set_next_for_test(DispatchId::LIMIT - 1)` and additionally asserts `mutator.heap.collection_requested()` after the first `NONE`.

- [ ] **Step 2: Run them to see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(/smallest_free|trailing_free|freed_guard|exhausted_below|free_ranges|ending_at_the_table|first_mint|minting_stops/)'`. Expected: they do not compile (`FreeIndices`, `DispatchIdAllocator`, `FreeRanges` and `dispatch_ids` are not defined).

- [ ] **Step 3: Implement.**
  - **`FreeIndices`** is a thin wrapper over `BTreeSet<u32>`:
    - `take_smallest` is `pop_first`;
    - `free` asserts `insert` answered `true`.
  - **`truncate_trailing`:**

```rust
pub(crate) fn truncate_trailing<T>(rows: &mut Vec<Option<T>>, free: &mut FreeIndices) {
    while matches!(rows.last(), Some(None)) {
        rows.pop();
        let index = u32::try_from(rows.len()).expect("a table row fits a u32");
        let was_free = free.free.remove(&index);
        debug_assert!(was_free, "internal: a trailing None row is a free index");
    }
}
```

  - **`DispatchIdAllocator::mint`:**

```rust
pub(crate) fn mint(&mut self) -> Option<DispatchId> {
    if let Some(index) = self.free.take_smallest() {
        return Some(DispatchId::from_counter(index));
    }
    if self.next >= DispatchId::LIMIT {
        return None;
    }
    let id = DispatchId::from_counter(self.next);
    self.next += 1;
    Some(id)
}
```

  - **`DispatchIdAllocator::free`** `debug_assert!`s `id.bits() >= DispatchId::FIRST_MINTED && id.bits() < self.next`, then `self.free.free(id.bits())`.
  - **`Mutator::mint_dispatch_ids`:**

```rust
pub(crate) fn mint_dispatch_ids(&mut self) -> crate::mixin::DispatchIds {
    match self.dispatch_ids.mint() {
        Some(instances) => crate::mixin::DispatchIds { instances },
        None => {
            // Freed ids come back only in a full collection, which nothing
            // but the old space's growth would otherwise start: a program
            // that makes and drops applications quickly would run out
            // with plenty of dead ones to reclaim.
            self.heap.request_collection();
            crate::mixin::DispatchIds::NONE
        }
    }
}
```

  - Rewrite the doc comment of `mint_dispatch_ids` and of the field `dispatch_ids` to say the above: the smallest freed id first, `NONE` and a requested collection once exhausted, freed by `reclaim_ids` (Task 4).
  - **`FreeRanges`:**
    - `take_first_fit` walks `ranges` in order and takes the first `(start, size)` with `size >= wanted`. It removes the range, re-inserts `(start + wanted, size - wanted)` when the size is larger, and answers `start`.
    - `free` merges with the range ending at `start` (the last range before `start`, if `range_start + range_size == start`) and with the range starting at `start + size`.
    - `take_ending_at(end)` looks at the last range before `end` and takes it if `start + size == end`.
  - Add a field `free_ranges: FreeRanges` to `InlineCaches` (initialized in `new`).
  - Make `fresh_range` first try `self.free_ranges.take_first_fit(size)`, answering `FeedbackBase(start)` on success. It falls back to growing `sites` as today.
  - Nothing frees a range until Task 6. Add `#[allow(dead_code)]` with a comment naming Task 6's use **only** if clippy complains about `free` or `take_ending_at`. Prefer having Task 6's code call them.

- [ ] **Step 4: Run the tests.** Same command as Step 2. Expected: PASS.

- [ ] **Step 5: Gates and commit.** Run `cargo xtask gates commit`. Expected: green, `parsed 211, failed 0`. Then commit:

```bash
git add crates/ns-runtime/src/heap/ids.rs crates/ns-runtime/src/heap.rs crates/ns-runtime/src/heap/world.rs crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/src/interpreter/inline_cache.rs
git commit -m "feat: mint the smallest free id first, and collect when the guards run out"
```

---

### Task 2: A serial marker per nested-class creation

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:492-507` (`NestedClassCreation` gains `serial`)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs`:
  - `nested_class` assigns the serial;
  - `creation_in_progress` goes;
  - `is_creation_in_progress`, `begin_creation` and `end_creation` change, as do their doc comments.
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`:
  - the field `next_creation_serial: u64` on `Mutator`, initialized to 1 in `heap/world.rs` beside `dispatch_ids`;
  - the call `nested::begin_creation(self, enclosing, index)` at `:1334` passes the serial.
- Test: `crates/ns-runtime/src/builtins/reflection.rs` (where the forward-during-clause tests of "The nested-class cache slot" live; find them with `grep -n "creation" crates/ns-runtime/src/builtins/reflection.rs`).

**Interfaces:**
- Produces: `NestedClassCreation { enclosing, index, nested, declaring_class, serial: u64 }`; `Mutator::next_creation_serial: u64`; `nested::begin_creation(mutator, enclosing, index, serial)`.

- [ ] **Step 1: Write the failing test.** Next to the existing forward-during-clause tests in `builtins/reflection.rs`, add a test named `a_cycle_after_a_forward_to_a_creation_underway_is_diagnosed`. Model it on the existing test whose clause forwards its enclosing object.

  The scenario is two creations at the same index, the inner one on a different enclosing object. Set it up like this:
  - `Outer` declares `Inner` with a superclass clause `outerClause`.
  - `outerClause` does three things: it starts the creation of `Inner` on a second instance `other`, whose own clause has not finished; inside that clause, `other` forwards `self` to `other`, both instances of `Outer`; and then the outer clause returns.
  - `other`'s clause then accesses `Inner` on `other` again: a cycle.

  With today's `false` marker, the outer creation's `end_creation` resets `other`'s marker. The cyclic access, made after that point, then answers a class instead of raising.

  The test asserts that the run ends in the error `cyclic definition of class Inner`; take the exact text from `cyclic_definition_error` in `nested.rs`.

  Write the Newspeak module with `indoc!`, under both lowerings, using the file's existing helpers for running a module under both lowerings. If you cannot build the ordering with a single forward, read the existing tests' comments and "The nested-class cache slot" in `object-model.md` again. The shape described there is: the replacement has a creation of its own underway at the same index, and the first of the two to finish resets the other's marker.

- [ ] **Step 2: Run it to see it fail.** Run `cargo nextest run -p ns-runtime -E 'test(a_cycle_after_a_forward_to_a_creation_underway_is_diagnosed)'`. Expected: FAIL. The access answers a class (or the run completes) instead of raising the cyclic-definition error.

- [ ] **Step 3: Implement.**

```rust
/// Whether a nested class's cache slot says a creation is underway: it
/// holds the serial of that creation (`begin_creation`), a `SmallInteger`,
/// which a class object never is. An image that stores any `SmallInteger`
/// in a hidden slot gets a cyclic-definition error on access, the same
/// wrong-but-safe answer as for any other non-class value it puts there.
fn is_creation_in_progress<'gc>(value: Value<'gc>) -> bool {
    value.as_small_integer().is_some()
}

pub(super) fn begin_creation<'gc>(
    mutator: &mut Mutator<'gc>,
    enclosing: ObjectReference<'gc>,
    index: usize,
    serial: u64,
) {
    let marker = i64::try_from(serial)
        .ok()
        .map(Value::small_integer)
        .expect("internal: a creation serial fits a SmallInteger");
    mutator.heap.instance_set_field(enclosing, index, marker);
    // … the debug count as today
}
```

  - **`Value::small_integer`.** It panics outside the `SmallInteger` range. Check its contract at `heap/value.rs:212`. If it panics past 61 bits, keep the `expect`: the counter cannot reach 2^60 in any run.
  - **`end_creation`.**
    - It keeps `still_the_slot`, the lookup through `current_cache`.
    - It replaces `is_some_and(is_creation_in_progress)` with `is_some_and(|value| value.as_small_integer() == i64::try_from(creation.serial).ok())`.
    - Rewrite its doc comment. The marker is reset only where it still is, and only if it is this creation's own serial. A replacement's creation underway at the same index holds another serial, and the program's own field at that index may hold any `SmallInteger`. The `still_the_slot` lookup is what keeps a program field that happens to hold the same number from being reset.
  - **`nested_class`** sets `serial` when it builds `NestedClassCreation`:

```rust
let serial = mutator.next_creation_serial;
mutator.next_creation_serial += 1;
```

    Assign it even for a declaration without a clause, which never marks. That keeps the code simple, and the number is never read there.
  - **The push in `interpreter/mod.rs`:** read `creation.serial` beside `enclosing` and `index` before the move, and pass it.
  - Delete the doc-comment sentences on `creation_in_progress` that said `false`. Move what still holds (why the slot rather than a list) onto `is_creation_in_progress`.

- [ ] **Step 4: Run the test and the cache-slot tests.** Run `cargo nextest run -p ns-runtime -E 'test(/creation|cyclic|nested_class/)'`. Expected: PASS.

- [ ] **Step 5: Gates and commit.** Run `cargo xtask gates commit`. Then:

```bash
git commit -am "fix: mark each nested-class creation with its own serial"
```

---

### Task 3: Nested declarations through ids

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs`:
  - `pub nested: Vec<Rc<Mixin>>` (`:634`) becomes private `declared_nested: RefCell<Vec<Rc<Mixin>>>` plus `adopted: Cell<bool>`, and the builder sets them;
  - `set_enclosing_of_children` (`:1047-1053`) reads the declared list;
  - the preorder at `:1443` changes.
- Modify: `crates/ns-runtime/src/heap/tables.rs` (a new column `nested: Vec<Vec<MixinId>>`)
- Modify: `crates/ns-runtime/src/heap.rs`:
  - `mixin_id` and `register_mixin` register the whole declared subtree and take the children;
  - new `nested_of`;
  - `assembled_text` reads through ids;
  - `replace_mixin` gives the replacement its row.
- Modify: every reader listed by `grep -rn "\.nested\b" crates/ --include=*.rs | grep -v "creation.nested\|nested_slot"`. As of this plan, these are:
  - in `ns-analyzer`: `commands/disasm.rs:41`;
  - in `ns-runtime/src`:
    - `builtins/activation.rs:356`, `builtins/reflection.rs:500`, `dispatch.rs:198`, `dispatch.rs:330`;
    - `image/inspect.rs:210`, `image/mixins.rs:123`, `:165`, `:199`, `:1044`, `image/shipped_names.rs:360`, `:429`;
    - `interpreter/core_library.rs:549`, `:800`, `interpreter/edit.rs:308`, `:351-353`, `interpreter/install.rs:169-172`, `:415`, `interpreter/instantiate.rs:1800`;
    - `interpreter/kernel.rs:547`, `:656`, `:799`, `:1031`, `:1820`, `:2366`, `:2531`, `:2544`, `:2550`;
    - `interpreter/mod.rs:1930`, `interpreter/nested.rs:348`, `:572`;
    - `interpreter/shipped_world.rs:227`, `:334`, `:428`, `:657`, `:925`, `:937`, `:1245`, `:1275`;
    - `shipped_code/decode.rs:159`, `:197`, `shipped_code/mod.rs:296`, `:348`;
  - the tests in `interpreter/edit.rs`, `interpreter/mod.rs`, `interpreter/shipped_world.rs` and `interpreter/install.rs`;
  - `crates/ns-runtime/tests/{compiler,image,mixin}.rs`.

  (`image/mixins.rs:335`, `:336`, `:682`, `:736`, `:822`, `:929` and `image/texts.rs:59` read the image *row*'s `nested`, a `Vec<u32>`, and do not change.)

**Interfaces:**
- Consumes: nothing from Tasks 1-2.
- Produces, on `Mixin` (all `pub`, since the integration tests and `ns-analyzer` read them):

```rust
impl Mixin {
    /// The nested declarations as they were built, before any heap
    /// registered the mixin: what the compiler, the shipped-code decoder,
    /// the image's mixin builder and an edit's pairing read. Empty once a
    /// heap has registered the mixin, which took them
    /// (`Heap::nested_of` answers them by id from then on).
    pub fn declared_nested(&self) -> std::cell::Ref<'_, [Rc<Mixin>]>;
    /// Whether a heap has taken this mixin's nested declarations.
    pub fn is_adopted(&self) -> bool;
    /// Hands the declared list to the heap registering the mixin and
    /// marks it adopted. Crate-private.
    pub(crate) fn take_declared_nested(&self) -> Vec<Rc<Mixin>>;
}
```

  Also, the builder method or field the shipped decoder and the tests use to set the list: `MixinBuilder`'s existing `nested` setter if there is one, else `Mixin::with_declared_nested(self, Vec<Rc<Mixin>>) -> Mixin`. Read `mixin.rs:1000-1060` and keep its idiom.
- Produces, on `Heap`:

```rust
/// The ids of the declarations nested directly in the mixin `id`, in
/// declaration order, the same for every version of it: what every
/// reader of a registered mixin's nested declarations reads, through
/// `mixin` for each one's current version. Empty for an id past the
/// table.
pub fn nested_of(&self, id: MixinId) -> &[MixinId];
```

- [ ] **Step 1: Write the failing tests.** In `heap.rs`'s tests module (or `interpreter/install.rs`'s, which already has `load`, `nested_id_of`, `install` and `add_method`), add the four tests below.

```rust
/// After `Inner` was edited, `Outer`'s current version names `Inner` by
/// id, so every reader reaches the edited version; the version `Outer`
/// was built with holds no `Inner` any more.
#[test]
fn an_enclosing_mixin_reaches_its_nested_edit_through_the_id() {
    let (mut interpreter, outer) = load(OUTER);
    let inner = nested_id_of(&mut interpreter, &outer, "Inner");
    install(&mut interpreter, inner, &add_method("public added = ( ^42 )"));
    interpreter.enter(|mutator| {
        let outer_mixin = mixin_of(mutator, &outer);
        let outer_id = mutator.heap.known_mixin_id(&outer_mixin).expect("registered");
        assert!(outer_mixin.declared_nested().is_empty());
        assert_eq!(mutator.heap.nested_of(outer_id), &[inner]);
        let current = mutator.heap.mixin(inner).expect("a row");
        assert!(current.compiled_method(Symbol::intern("added")).is_some());
    });
}
```

```rust
/// Registering a module's root registers every declaration nested in it,
/// so no tree keeps an unregistered child.
#[test]
fn registering_a_root_registers_its_whole_tree() {
    let (mut interpreter, outer) = load(indoc! {"
        Newspeak3
        'Test'
        class Outer = (
        ) (
          class Middle = (
          ) (
            class Inner = (
            ) (
            )
          )
        )
    "});
    interpreter.enter(|mutator| {
        let outer_id = mutator
            .heap
            .known_mixin_id(&mixin_of(mutator, &outer))
            .expect("registered");
        let mut parent = outer_id;
        for name in ["Middle", "Inner"] {
            let children = mutator.heap.nested_of(parent).to_vec();
            assert_eq!(children.len(), 1, "{name} is the only declaration there");
            let child = children[0];
            let mixin = mutator.heap.mixin(child).expect("a row");
            assert_eq!(mixin.name.as_str(), name);
            assert_eq!(mutator.heap.enclosing_mixin_id(child), Some(parent));
            parent = child;
        }
        assert!(mutator.heap.nested_of(parent).is_empty());
    });
}
```

```rust
/// A nested class added by an edit gets an id of its own, declared in the
/// edited mixin.
#[test]
fn a_nested_class_an_edit_adds_gets_a_fresh_id() { /* only if an edit can
    add a nested class: check `check_kept_header` and `MemberEdit`; if no
    edit can, delete this test and say so in the commit message */ }

/// A dispatch of the nested-class accessor after an edit of the nested
/// class reaches the edited version (`dispatch.rs` reads through the id).
#[test]
fn the_nested_class_accessor_reaches_the_edited_version() {
    /* edit `Inner` to add `public added = ( ^42 )`, then evaluate a fresh
       `Outer new Inner new added` (by sending a method of OUTER that
       does it, added with `add_method` on Outer's id) and assert 42 */
}
```

  Write the third and fourth bodies fully, following the neighbouring tests. For the fourth, add a method to `Outer` that answers `Inner new added`.

- [ ] **Step 2: Run them to see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(/through_the_id|whole_tree|fresh_id|reaches_the_edited_version/)'`. Expected: they do not compile (`declared_nested`, `nested_of`).

- [ ] **Step 3: Change `Mixin`.**
  - Replace the field with `declared_nested: RefCell<Vec<Rc<Mixin>>>` and `adopted: Cell<bool>`.
  - `declared_nested()` is `Ref::map(self.declared_nested.borrow(), Vec::as_slice)`.
  - `take_declared_nested` does `debug_assert!(!self.adopted.get(), "internal: a mixin is registered in at most one heap")`, sets `adopted`, and answers `std::mem::take(&mut *self.declared_nested.borrow_mut())`.
  - `set_enclosing_of_children` and the preorder at `:1443` read `declared_nested()`. They run while building, before registration.
  - **The invariant "a mixin is registered in at most one heap".** Confirm it before relying on it. Do this:
    - `grep -rn "thread_local!" crates/ns-runtime/src`, and check that no thread-local, static or shared cache holds an `Rc<Mixin>`. The 5b2b plan says `SHIPPED_MODULE_TEXTS` holds texts only.
    - `grep -rn "Rc<Mixin>\|Rc<crate::mixin::Mixin>" crates/ns-runtime/src/shipped_code` to confirm the decoded shipped code is decoded per boot.
    - If a mixin tree is shared between interpreters, **stop and report**: the take design does not hold then.

- [ ] **Step 4: Change the table and the registration.**
  - Add `pub(crate) nested: Vec<Vec<MixinId>>` to `Tables`, documented as the per-id list `nested_of` answers.
  - `register_mixin(mixin, enclosing)` becomes an explicit-stack walk that registers `mixin` and every declared descendant, preorder. For each mixin it pushes a row, takes its children, registers them with `Some(its id)`, and records their ids in its `nested` row:

```rust
fn register_mixin(&mut self, mixin: &Rc<Mixin>, enclosing: Option<MixinId>) -> MixinId {
    let root = self.push_mixin_row(mixin, enclosing);
    // Walked, not recursed: an image chooses how deep its declarations nest.
    let mut pending = vec![(root, Rc::clone(mixin))];
    while let Some((id, parent)) = pending.pop() {
        let children = parent.take_declared_nested();
        let mut ids = Vec::with_capacity(children.len());
        for child in children {
            let child_id = self.push_mixin_row(&child, Some(id));
            ids.push(child_id);
            pending.push((child_id, child));
        }
        self.tables.nested[id.index()] = ids;
    }
    root
}

/// One row for `mixin`, declared in `enclosing`'s, with no nested ids yet.
fn push_mixin_row(&mut self, mixin: &Rc<Mixin>, enclosing: Option<MixinId>) -> MixinId {
    let id = MixinId::from_index(self.tables.mixins.len())
        .expect("more than u32::MAX mixins in one heap");
    self.tables.mixins.push(Rc::clone(mixin));
    self.tables.enclosing.push(enclosing);
    self.tables.nested.push(Vec::new());
    self.tables.mixin_ids.insert(crate::mixin::mixin_key(mixin), id);
    id
}
```

    Task 5 turns these `Vec`s into `Vec<Option<…>>` and mints from the free set. Here they stay append-only.
  - In `mixin_id`, the walk up through `enclosing_mixin()` stays as it is. A child whose parent is registered is registered already now, so the loop finds the outermost unregistered ancestor and registers its whole tree. After the loop, answer `known_mixin_id(mixin)`, since the registration of an ancestor registered `mixin` too:

```rust
for each in unregistered.into_iter().rev() {
    if self.known_mixin_id(&each).is_none() {
        let enclosing = each
            .enclosing_mixin()
            .and_then(|parent| self.known_mixin_id(&parent));
        self.register_mixin(&each, enclosing);
    }
}
self.known_mixin_id(mixin)
    .expect("internal: registering its outermost unregistered ancestor registered it")
```

  - **`replace_mixin(id, replacement)`.** The replacement is a fresh, unregistered tree. Change it as follows:
    - It does **not** register the replacement's children. It records the replacement's address under `id` and takes its declared children into a pending list that `install` resolves. That makes `replace_mixin` answer `Vec<Rc<Mixin>>`, the replacement's declared children, in order.
    - The caller, `Mutator::replace_versions` (`install.rs:187`), runs after every pair has been replaced. For each replaced instance side, it gives `Heap::adopt_nested(id, children)` the children:

```rust
/// Gives the mixin `id`'s current version its nested row: each child
/// the install replaced keeps the id it was paired with, and a child
/// with no counterpart is registered fresh, with its subtree, declared
/// in `id`.
pub(crate) fn adopt_nested(&mut self, id: MixinId, children: Vec<Rc<Mixin>>) {
    let ids = children
        .iter()
        .map(|child| match self.known_mixin_id(child) {
            Some(child_id) => child_id,
            None => self.register_mixin(child, Some(id)),
        })
        .collect();
    self.tables.nested[id.index()] = ids;
}
```

    - In `replace_versions`, collect `(id, children)` from each `self.replace_mixin(...)` call of the `prepared.replacements` loop. After the loop, call `adopt_nested` for each in order. A class side's children are always empty.
    - `Mutator::replace_mixin` passes the children through.
  - **`edit.rs`'s `pair` (`:351-372`)** reads `fresh.declared_nested()` for the fresh tree and the heap for the old one:

```rust
for nested in fresh.declared_nested().iter() {
    let counterpart = self
        .heap
        .nested_of(id)
        .iter()
        .copied()
        .find(|old_id| {
            self.heap
                .mixin(*old_id)
                .is_some_and(|old_nested| old_nested.name == nested.name)
        });
    let Some(nested_id) = counterpart else { continue };
    let current = self.heap.mixin(nested_id).expect("internal: a nested id has a row");
    self.pair(nested_id, &current, nested, replacements, synthesized);
}
```

    `old` is now read only for its class side. Keep the parameter if `old_class_side_id` needs it.

- [ ] **Step 5: Port the readers.**
  - **Readers of a registered mixin.** Each one gets the mixin's id (`known_mixin_id`, or the id it already has), then `nested_of(id)`, and maps through `mixin(id)` where it needs the version:
    - **`interpreter/mod.rs:1930`** (`apply_mixin`): `for nested_id in self.heap.nested_of(id).to_vec() { nested_slot.insert(nested_id, next); next += 1; }`. `id` is the mixin's id, already computed at the top.
    - **`dispatch.rs:196-200` and `:328-333`** (lookup by name): from the application's `mixin`, take its id with `heap.known_mixin_id` (a dispatch runs on an applied mixin, which is registered). Iterate `nested_of`, compare `heap.mixin(nested_id)`'s `name`, and answer that **current** version in the `Lookup::NestedClass`.
      - Check whether these functions have a `&Heap` in scope. If not, widen their signatures; the callers in `interpreter/` have the heap.
      - The comment there that leans on `check_kept_header` goes, since the current version is read now.
    - **`nested.rs:348`, `:572`**:
      - `current_declaring_class` becomes `mutator.heap.nested_of(application_mixin_id).contains(&id)`, the application mixin's id taken with `known_mixin_id`.
      - For `:348`, read the context and use `nested_of` likewise.
    - **`builtins/reflection.rs:500`** (`mixin_nested`): map `nested_of(id)` through `mixin`, then reify each as today.
    - **`builtins/activation.rs:356`**: read the context. It is likely a test fixture; use `nested_of` likewise.
    - **`heap.rs` `assembled_text`**: it takes the mixin's id via `known_mixin_id`. An unregistered mixin keeps `declared_nested()`, its own current version. A registered one maps `nested_of(id)` through `mixin`. Delete the "found through the nested mixin's id" detour comment and say what is true now.
    - **`image/mixins.rs:111-129`, `:157-166`** (`current`, `current_preorder`, `plan`) and **`image/inspect.rs:210`**: through `nested_of` and `mixin`. The "current through the id" comments now describe the only way. `:199` and `:1044` build trees on the decoder side, before registration, so they use `declared_nested()` or the builder.
    - **`image/shipped_names.rs:360`, `:429`, `shipped_code/mod.rs:296`, `:348`**: read each.
      - One that reads a mixin a heap registered uses `nested_of`.
      - One that reads a freshly decoded tree uses `declared_nested()`.
    - **`shipped_code/decode.rs:159`** sets the declared list; `:197` reads it.
    - **`interpreter/kernel.rs`, `core_library.rs`, `shipped_world.rs`, `instantiate.rs:1800`, `install.rs:169-172` (`layout_may_change`), `install.rs:415`, `edit.rs:308`**: decide per site.
      - Has a heap registered this mixin at that point? A mixin reached through a class object, an application or `heap.mixin` has been. Such a site uses `nested_of`.
      - A mixin just built or just decoded has not. Such a site uses `declared_nested()`.
      - For `layout_may_change`, compare the old application's mixin's nested names (via `nested_of` of its id, mapped to names) with the fresh tree's `declared_nested()` names.
    - **`ns-analyzer`'s `commands/disasm.rs:41`**: the same decision.
  - **The tests** (`edit.rs`, `mod.rs`, `shipped_world.rs`, `install.rs`, `tests/compiler.rs`, `tests/image.rs`, `tests/mixin.rs`):
    - A test that builds a tree uses the builder or `declared_nested()`.
    - A test that reads a loaded tree uses `heap.nested_of(heap.known_mixin_id(&root)?)` and `heap.mixin`.
    - Tests that assign `mixin.nested = vec![...]` before applying (`tests/mixin.rs:265`, `:341`, `:347`, `tests/image.rs:775`) set the declared list through the builder or `with_declared_nested`.
    - Add a small helper `fn nested_named(heap: &Heap, mixin: &Rc<Mixin>, name: &str) -> Rc<Mixin>` to any file that needs it several times.
  - **Every site you did not move to `nested_of`.** A `declared_nested()` on an adopted mixin answers an empty list silently. Add `debug_assert!(!self.is_adopted(), "internal: read nested_of for a registered mixin")` inside `declared_nested()`. Then every debug-build test catches a site ported the wrong way. Keep `is_adopted` public for the readers that must branch, `assembled_text` among them.

- [ ] **Step 6: Run the tests.** First `cargo nextest run -p ns-runtime -E 'test(/through_the_id|whole_tree|fresh_id|reaches_the_edited_version/)'`, then `cargo nextest run --workspace`. Expected: PASS. Every golden passes unchanged, under both lowerings.

- [ ] **Step 7: Gates and commit.** Run `cargo xtask gates commit` and `cargo xtask gates images`: this touches `image/mixins.rs`'s reading of mixins. The `images` group is per-commit for changes under `image/`. Then:

```bash
git commit -am "refactor: name a registered mixin's nested declarations by id"
```

---

### Task 4: Reclaiming applications and guard ids

**Files:**
- Create: `crates/ns-runtime/src/heap/reclaim.rs`:
  - `GatheredIds`, the bit sets marking fills;
  - `Heap::free_application`;
  - `Heap::application_indices_named_by_the_stack`;
  - `Heap::prune_mixin_objects`.
- Create: `crates/ns-runtime/src/interpreter/reclaim.rs` (`Mutator::reclaim_ids`, the order of the decisions, and the tests)
- Modify: `crates/ns-runtime/src/heap/mark.rs` (`Marker` gains `gathered: GatheredIds`)
- Modify: `crates/ns-runtime/src/heap.rs`:
  - `drain_marking_recording` gathers;
  - `begin_marking` stops rooting `mixin_objects`;
  - `register_application` mints from a free set;
  - `Tables::applications` becomes `Vec<Option<Rc<MixinApplication>>>`, and every reader of it changes;
  - `collect_for_test` calls the prune.
- Modify: `crates/ns-runtime/src/heap/tables.rs` (`applications: Vec<Option<…>>`, `free_applications: FreeIndices`; the doc comments)
- Modify: `crates/ns-runtime/src/heap/world.rs`:
  - `mark_everything_reachable` answers the gathered ids;
  - `collect` calls `prune_mixin_objects` and `mutator.reclaim_ids`.
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`InlineCaches::forget_freed`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod reclaim;`)

**Interfaces:**
- Consumes: `FreeIndices`, `truncate_trailing` and `DispatchIdAllocator::free` (Task 1); `nested_of` (Task 3).
- Produces, in `crate::heap::reclaim`:

```rust
/// What a full marking found named: one bit per row of each table.
#[derive(Default)]
pub(crate) struct GatheredIds {
    /// `class_field::APPLICATION` of every marked class object.
    pub(crate) applications: BitSet,
    /// The mixin id `class_field::APPLIED_CLAUSE` packs, of every marked
    /// class object that has one (Task 5 reads it).
    pub(crate) applied_clauses: BitSet,
    /// `CODE_ID` of every marked closure and frame (Task 5).
    pub(crate) codes: BitSet,
    /// `FEEDBACK_BASE` of every marked closure, raw (Task 6).
    pub(crate) feedback_bases: std::collections::BTreeSet<u32>,
}

/// A growable set of `u32` rows, a word per 64.
#[derive(Default)]
pub(crate) struct BitSet { words: Vec<u64> }
impl BitSet {
    pub(crate) fn insert(&mut self, index: u32);
    pub(crate) fn contains(&self, index: u32) -> bool;
}

/// What a reclamation freed, for the caches' scrub.
#[derive(Default)]
pub(crate) struct FreedIds {
    pub(crate) guards: std::collections::HashSet<DispatchId>,
    pub(crate) applications: std::collections::HashSet<ApplicationIndex>,
}
impl FreedIds { pub(crate) fn is_empty(&self) -> bool; }
```

- Produces, on `Heap` (`pub(crate)`):
  - `fn prune_mixin_objects(&mut self)`;
  - `fn stack_roots(&self) -> StackRoots`, where `StackRoots { entered_levels: Vec<ApplicationIndex>, recorded_guards: Vec<DispatchId>, feedback_bases: Vec<FeedbackBase> }`;
  - `fn application_rows(&self) -> usize`;
  - `fn free_application(&mut self, index: ApplicationIndex) -> Rc<MixinApplication>`, which takes the row and frees the index;
  - `fn retain_successors(&mut self, live: impl Fn(ApplicationIndex) -> bool)`;
  - `fn truncate_application_rows(&mut self)`.
- Produces, on `Mutator`: `pub(crate) fn reclaim_ids(&mut self, gathered: GatheredIds)`.
- Produces, on `InlineCaches`: `pub(crate) fn forget_freed(&mut self, freed: &FreedIds)`.

- [ ] **Step 1: Write the failing tests.** In `interpreter/reclaim.rs`, write a `#[cfg(test)] mod tests` with the helpers `load`, `expect_ok`, `send`, `nested_id_of`, `install` and `add_method`, copied from `interpreter/install.rs`'s tests. Add three tests.

  **`dead_applications_are_reclaimed_and_their_guards_minted_again`:**
  - A module that builds a chain of a hundred applications and drops it. Each `Holder` instance caches its own `Local`, whose superclass is the previous round's `Local`. Its superclass application is therefore new each round, and so is its own application. Afterwards nothing holds the chain:

```newspeak
Newspeak3
'Test'
class Churn = (
) (
  class Holder base: superclass = (
    | public base = superclass. |
  ) (
    class Local = base (
    ) (
    )
  )
  public run = (
    | current |
    current := Object.
    1 to: 100 do: [:each | current := (Holder base: current) Local].
    ^current new
  )
)
```

  - If the resolver refuses `base` in a superclass clause, read `goldens/language/class/` for the form the corpus uses to name an enclosing slot there. Adapt the module, but keep the chain: each round's superclass is the previous round's class.
  - Run `run` once and call `interpreter.collect_now()`. Record `mutator.heap.application_rows()` and `mutator.dispatch_ids.high_water()`.
  - Run `run` again and call `collect_now()`. Both numbers must be **unchanged**: the second round reused the freed rows and guards.

  **`a_reused_guard_misses_every_cache_entry_of_its_old_application`:**
  - Class `A` answers `1` from `value`, and class `B` answers `2`. A site `probe: object = ( ^object value )` is driven monomorphic by `A` instances, and a polymorphic site by `A` and two others.
  - Drop every reference to `A`'s class and instances. Call `collect_now()`.
  - Then create applications until one gets `A`'s old guard. Record the guard first through `mutator.heap.class_dispatch_ids(class).instances`, and loop `apply_mixin` on fresh mixins until `mint` hands it back; Task 1's smallest-first rule makes it the next one.
  - Send `probe:` with an instance of the new class, whose `value` answers `3`. Assert `3`.
  - Run the test under both lowerings: the reference lowering has no caches and is the oracle.

  **`an_activation_at_a_superseded_level_runs_on_across_a_collection`:**
  - Start from `install.rs`'s test that installs while an old method runs (search for `entered_level` or `successor` in its tests).
  - In the middle, a host-side hook installs and then calls `collect_now()`. Use the existing callback mechanism of that test, or have the running method send a message the host answers.
  - Assert that the method finishes writing its own slot and answers the expected value.
  - If the existing tests do this through `Interpreter::enter` between steps, reproduce that.

- [ ] **Step 2: Run them to see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(/dead_applications|reused_guard|superseded_level_runs_on/)'`. Expected:
  - the first fails, because the counts grow;
  - the second fails to compile (no reclamation yet);
  - the third may pass, which is fine: it guards what Task 4 must not break.

- [ ] **Step 3: Gather while marking.**
  - In `drain_marking_recording`, after the mark bit is set and `format` read, add one arm. It runs for the main marking and the fixpoint alike:

```rust
match format {
    Format::Class => marker.gathered.class_words(
        words[block.start + object::class_field_position(class_field::APPLICATION)],
        words[block.start + object::class_field_position(class_field::APPLIED_CLAUSE)],
    ),
    Format::Closure => marker.gathered.closure_words(
        words[block.start + object::closure_field_position(closure_field::CODE_ID)],
        words[block.start + object::closure_field_position(closure_field::FEEDBACK_BASE)],
    ),
    Format::Frame => marker.gathered.frame_word(
        words[block.start + object::frame_field_position(frame_field::CODE_ID)],
    ),
    _ => {}
}
```

  - **Field positions.** Find how a field index maps to a word in the block: the header word and the class word come first. Use the existing helper that `Heap::load` uses. If there is none at `heap/object.rs`'s level, add `pub(in crate::heap) fn field_word(block_start, field) -> usize` beside `block_range`.
  - **`GatheredIds::class_words` and friends** decode each word as a `SmallInteger`, through the word-to-value decoding `Heap::load` uses (`Word` → `Value::as_small_integer`). They ignore a word that is not one: a crafted image, or a frame's nil code. They insert the row:
    - `APPLICATION` → `applications`;
    - `AppliedClause::unpack(packed)`'s mixin → `applied_clauses`;
    - `CODE_ID` → `codes`;
    - a closure's `FEEDBACK_BASE` (unless `FeedbackBase::NOT_ASSIGNED.raw()`) → `feedback_bases`.
  - `mark_everything_reachable` (`world.rs:241`) answers `std::mem::take(&mut marker.gathered)`. `collect` binds it.
- [ ] **Step 4: Make `mixin_objects` weak.**
  - Delete the loop at `begin_marking` (`heap.rs:6079-6081`).
  - Add `prune_mixin_objects`, modelled on `prune_canonical_symbols`: an entry whose object is unmarked becomes `None`.
  - Call it in `World::collect` right after `prune_canonical_symbols`, and in `collect_for_test`.
  - Rewrite the doc comment at `heap.rs:621-628`. The table is weak. An entry lives while its object is reachable, so `==` stays mixin identity for every comparison a program can make. A marked entry keeps its mixin id (Task 5).
  - The scavenger keeps tracing it strongly (`:5417`): a scavenge reclaims no id.
  - `record_mixin_object`'s assertion that the slot is `None` still holds: a pruned slot is `None` again.

- [ ] **Step 5: Free applications.**
  - `Tables::applications` becomes `Vec<Option<Rc<MixinApplication>>>`, with `free_applications: FreeIndices`.
  - `register_application` takes `free_applications.take_smallest()` before appending.
  - `Heap::application(index)` reads `.get(index)?.clone()`.
  - Fix every other reader the compiler names.
  - **`Mutator::reclaim_ids`, steps 1-2 of the spec's order:**

```rust
pub(crate) fn reclaim_ids(&mut self, gathered: GatheredIds) {
    let stack = self.heap.stack_roots();
    // Step 1: applications. Live: what a marked class object names, what an
    // activation was entered at, the application whose guard an activation
    // recorded -- a receiver forwarded under it must never meet the guard
    // reused -- and, transitively, superclass links and successors.
    let rows = self.heap.application_rows();
    let mut live = vec![false; rows];
    let mut work: Vec<ApplicationIndex> = Vec::new();
    for row in 0..rows {
        let index = ApplicationIndex::from_index(row).expect("a row fits a u32");
        if gathered.applications.contains(row as u32) {
            work.push(index);
        }
    }
    work.extend(stack.entered_levels.iter().copied()
        .filter(|level| *level != ApplicationIndex::UNREGISTERED));
    let recorded: HashSet<DispatchId> = stack.recorded_guards.iter().copied()
        .filter(|guard| *guard != DispatchId::NONE).collect();
    if !recorded.is_empty() {
        for row in 0..rows {
            let index = ApplicationIndex::from_index(row).expect("a row fits a u32");
            if self.heap.application(index)
                .is_some_and(|application| recorded.contains(&application.dispatch_ids.instances))
            {
                work.push(index);
            }
        }
    }
    while let Some(index) = work.pop() {
        let Some(flag) = live.get_mut(index.index()) else { continue };
        if std::mem::replace(flag, true) { continue; }
        if let Some(application) = self.heap.application(index) {
            if let Some(superclass) = &application.superclass {
                work.push(superclass.index());
            }
        }
        if let Some(successor) = self.heap.successor(index) {
            work.push(successor);
        }
    }
    self.heap.retain_successors(|old| live.get(old.index()).copied().unwrap_or(false));
    let mut freed = FreedIds::default();
    let mut dropped = Vec::new();
    for row in 0..rows {
        let index = ApplicationIndex::from_index(row).expect("a row fits a u32");
        if live[row] || self.heap.application(index).is_none() { continue; }
        let application = self.heap.free_application(index);
        if application.dispatch_ids.instances != DispatchId::NONE {
            self.dispatch_ids.free(application.dispatch_ids.instances);
            freed.guards.insert(application.dispatch_ids.instances);
        }
        freed.applications.insert(index);
        dropped.push(application);
    }
    // Step 2: the caches, only when something was freed.
    if !freed.is_empty() {
        self.applications.retain(|(_, superclass), application| {
            !freed.applications.contains(&application.index())
                && superclass.is_none_or(|superclass| !freed.applications.contains(&superclass))
        });
        self.caches.forget_freed(&freed);
    }
    // No other holder may outlive the scrub: an index freed while an `Rc`
    // still names it would alias the next application minted there.
    for application in &dropped {
        debug_assert_eq!(Rc::strong_count(application), 1,
            "internal: a reclaimed application is held outside the tables and caches");
    }
    drop(dropped);
    self.heap.truncate_application_rows();
    // Steps 3-5 come with Tasks 5 and 6.
}
```

  - **`dropped` and superclass links.** `dropped` holds `Rc`s whose `superclass` links can hold other dropped applications. So the strong-count check is right only for an application that no other dropped one names as its superclass. Check each one's count minus the number of dropped applications naming it as `superclass`. Alternatively, drop in an order where subclasses come first and check after each drop. Choose one, implement it, and state it in a comment.
  - **`Heap::stack_roots`** walks `self.stack.entries` and collects each entry's `entered_level`, `recorded_guard` and `feedback_base`.
  - **`Heap::retain_successors`** drops each entry whose key the closure calls dead.
  - **`World::collect`** calls `self.mutator.reclaim_ids(gathered)` after `prune_mixin_objects`, before `compact`.

- [ ] **Step 6: Scrub the caches.** `InlineCaches::forget_freed(&mut self, freed)` does this:
  - **`sites`:** a monomorphic entry whose `guard` is in `freed.guards` becomes vacant, as does a `Target::Slot` entry whose `level` is in `freed.applications` or whose `recorded_guard` is in `freed.guards`. A `Polymorphic` site's block entries are tested the same way: each matching slot becomes `CacheEntry::vacant()`, and the site stays polymorphic. A `Megamorphic` site holds no guard and stays.
  - **`global`:** every entry whose `guard`, whose `CacheRule::Lexical(guard)`, or whose target's `level` or `recorded_guard` was freed becomes vacant. Add `GlobalCache::retain(&mut self, keep: impl Fn(&GlobalEntry) -> bool)` beside `clear`.
  - **`verified_slot_level`:** `None` if its guard or its level was freed.
  - Doc comment: the one scrub a collection makes. It vacates exactly what names a freed id, so every other entry stays warm (`docs/design/dispatch.md`, "Invalidation", which Task 7 updates).

- [ ] **Step 7: Run the tests.** `cargo nextest run -p ns-runtime -E 'test(/dead_applications|reused_guard|superseded_level_runs_on/)'`, then `cargo nextest run --workspace`, then `NS_NEXT_STRESS_COLLECT=1 cargo nextest run -p ns-runtime -E 'test(/reclaim|install|mirror_builder/)'`. Expected: PASS.

- [ ] **Step 8: Gates and commit.** Run `cargo xtask gates commit caches`. The `caches` group is needed because this touches `inline_cache.rs`. Then:

```bash
git commit -am "feat: reclaim dead applications and their guard ids in a full collection"
```

---

### Task 5: Reclaiming mixin trees, superseded versions and codes

**Files:**
- Modify: `crates/ns-runtime/src/heap/tables.rs`:
  - `mixins: Vec<Option<Rc<Mixin>>>`, `enclosing: Vec<Option<Option<MixinId>>>` (or a row struct, see Step 3), `nested`, `free_mixins: FreeIndices`;
  - `superseded: Vec<Rc<Mixin>>` stays a `Vec`, compacted with `retain`;
  - `codes: Vec<Option<CodeEntry>>`, `free_codes: FreeIndices`.
- Modify: `crates/ns-runtime/src/heap.rs` (`push_mixin_row` and `register_code` mint from the free sets; `mixin`, `code` and `code_method` read `Option` rows; `Heap::free_mixin_tree`, `Heap::drop_superseded`, `Heap::free_unnamed_codes`)
- Modify: `crates/ns-runtime/src/heap/reclaim.rs`, `crates/ns-runtime/src/interpreter/reclaim.rs` (steps 3-4)
- Test: `crates/ns-runtime/src/interpreter/reclaim.rs`

**Interfaces:**
- Consumes: `GatheredIds.applied_clauses` and `.codes`, and the step 1 liveness (Task 4).
- Produces:
  - `Heap::mixin_rows() -> usize`, `Heap::superseded_count() -> usize`, `Heap::code_rows() -> usize` (`pub(crate)`, for tests and Task 6);
  - `Heap::module_trees() -> Vec<Vec<MixinId>>`, each registered root with its descendants and the class sides registered under them, every row once.

- [ ] **Step 1: Write the failing tests** in `interpreter/reclaim.rs`.

  **`a_dead_module_tree_is_reclaimed`:**
  - Record `mixin_rows()` after bootstrapping.
  - Load a module (`Outer` > `Inner`), instantiate it, and drop both `Root`s.
  - Call `collect_now()` and assert that `mixin_rows()` is back at the recorded value.
  - Keep no `Rc<Mixin>` of the module in the test. One would be a Rust holder and keep the tree alive; that case is the next test.

  **`a_host_held_mixin_keeps_its_id`:**
  - Load the module and take `let held = mixin_of(mutator, &outer)` out of `enter`.
  - Drop the class `Root` and collect.
  - Assert that `heap.known_mixin_id(&held)` is the same as before. Assert that `nested_of` of it still names `Inner` with the same id.

  **`a_reachable_reified_mixin_keeps_its_identity`:**
  - A Newspeak program keeps `(ClassMirror reflecting: Outer) mixin` in a slot, through `platform mirrors`, the way `goldens/library/mirror/` does.
  - Run `garbageCollect`, sent as `platform kernel garbageCollect`.
  - Compare the kept value with a fresh `(ClassMirror reflecting: Outer) mixin` using `==`. It answers `true`.
  - Write it as a golden under `goldens/library/mirror/` with its `.toml` (`status = 0`, `stdout = "true\n"`). Put a `///` comment on the module and run `ns-analyzer fmt` on it.

  **`superseded_versions_nothing_holds_are_dropped`:** load, edit a method of `Outer` three times, then collect. `superseded_count()` must be 0, since no activation, closure or class object holds an old version.

  **`a_closure_of_a_superseded_version_runs_after_a_collection`:**
  - `Outer` has `public make = ( ^[ 7 ] )` and a slot `kept`. The host sends `kept: (instance make)`.
  - Edit `make` three times and collect.
  - `kept value` must answer `7`. `superseded_count()` must be at most 1: the closure's code may hold the old method's body, but no old version holds it.

  **`codes_nothing_names_are_freed`:**
  - Create and drop a hundred closures of distinct bodies. The bodies are distinct because the program edits the method each round.
  - Collect. `code_rows()` must be at most what it was after the first round.

- [ ] **Step 2: Run them to see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(/module_tree|host_held|reified_mixin|superseded_versions|superseded_version_runs|codes_nothing/)'`. Expected: FAIL, because the counts do not drop.

- [ ] **Step 3: Rows that can be freed.**
  - `mixins` becomes `Vec<Option<Rc<Mixin>>>`. Keep `enclosing` and `nested` as parallel `Vec`s whose row is meaningful only while `mixins[row]` is `Some`, or fold the three into `Vec<Option<MixinRow>>` with `struct MixinRow { current: Rc<Mixin>, enclosing: Option<MixinId>, nested: Vec<MixinId> }`. The fold is preferred: one `Option` and one truncation.
  - `push_mixin_row` takes `free_mixins.take_smallest()` first.
  - `Heap::mixin`, `enclosing_mixin_id`, `nested_of` and `replace_mixin` read the row.
  - `codes` becomes `Vec<Option<CodeEntry>>` with `free_codes`, and `register_code` mints likewise.

- [ ] **Step 4: Step 3 of the order: mixin trees.** In `Mutator::reclaim_ids`, after the caches, do the following.
  1. **Named ids.** Collect into a set:
     - the id of every live application's mixin (`known_mixin_id(&application.mixin)`, which answers for superseded versions too);
     - every `gathered.applied_clauses` row;
     - the id of every `Some` entry left in `mixin_objects` after `prune_mixin_objects` (`Heap::reified_mixin_ids()`).
  2. **The memo.** For every tree with no named id, remove the `metaclass_mixins` entries keyed by its ids. It is a memo, not a holder.
  3. **Rust holders.** A tree with no named id is still live if any of its current versions has a Rust holder:

```rust
/// Whether something outside the tables holds the current version of
/// `id`: its row holds one reference, and a declared class side is held
/// by its instance side's `class_side` too.
fn held_outside_the_tables(&self, id: MixinId) -> bool {
    let Some(current) = self.mixin(id) else { return false };
    // `current` itself is one more.
    let mut structural = 2;
    if current.is_class_side() {
        if let Some(instance_side) = self.enclosing_mixin_id(id).and_then(|owner| self.mixin(owner)) {
            if instance_side.class_side.as_ref().is_some_and(|declared| Rc::ptr_eq(declared, &current)) {
                structural += 1;
            }
        }
    }
    Rc::strong_count(&current) > structural
}
```

     Superseded versions held by a Rust holder keep their own address registered (step 5 below), but not the tree.
  4. **Free.** Every row of a tree that is neither named nor held is freed:
     - its row becomes `None`;
     - its index goes to `free_mixins`;
     - the address entry of its current version in `mixin_ids` is removed.
     A superseded version of a freed id stays in `superseded` until step 5 drops it. Its `mixin_ids` entry is removed then.
  5. **Superseded versions.** Repeat until nothing changes: drop each `superseded` entry whose `Rc::strong_count == 1`, and remove its `mixin_ids` entry. `Vec::retain` with a flag for "changed" does it.
  6. Truncate the mixin rows.

  `Heap::module_trees` builds the trees from the rows. A root has `enclosing == None`. Descendants come through `nested`, and class sides through every row whose `enclosing` names a row of the tree and whose current version `is_class_side()`.
  - Compute the class sides by one pass over all rows, grouping by the root of their enclosing chain.
  - Walk with an explicit stack.

- [ ] **Step 5: Step 4 of the order: codes.**
  - Every `codes` row not in `gathered.codes` is freed: its row becomes `None` and its index goes to `free_codes`.
  - Its `code_ids` entry, keyed by the body's address, is removed.
  - Truncate the rows.
  - No stack entry names a `CodeId` (it holds the `Rc`), so the gathered set is the whole root set.

- [ ] **Step 6: Update the doc comments of `heap/tables.rs`.**
  - The module comment and `Tables`'s: the rows are freed by `reclaim_ids` in a full collection and minted smallest first.
  - The address maps name only versions the tables hold, so an address is never reused while a key names it.
  - Remove "road step 5b2c's reclamation" and "forever, in 3a".

- [ ] **Step 7: Run the tests.** `cargo nextest run -p ns-runtime -E 'test(/module_tree|host_held|reified_mixin|superseded_versions|superseded_version_runs|codes_nothing/)'`, then `cargo nextest run --workspace`, then `NS_NEXT_STRESS_COLLECT=1 cargo nextest run --workspace`. Expected: PASS. The stress run is long, so run it in the foreground with a generous timeout.

- [ ] **Step 8: Gates and commit.** Run `cargo xtask gates commit images`. Then:

```bash
git commit -am "feat: reclaim dead mixin trees, superseded versions and code ids"
```

---

### Task 6: Reclaiming feedback ranges

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs`:
  - `families: IdentityMap<usize, FeedbackBase>` and `family_codes: Vec<Rc<CompiledCode>>` become `families: IdentityMap<usize, Family>` with `struct Family { base: FeedbackBase, code: Rc<CompiledCode> }`;
  - `base_for`, `invalidate`'s doc, the reader at `:1292-1297`, and `forget_families`.
- Modify: `crates/ns-runtime/src/interpreter/reclaim.rs` (step 5)
- Test: `crates/ns-runtime/src/interpreter/reclaim.rs`

**Interfaces:**
- Consumes: `FreeRanges` (Task 1); `GatheredIds.feedback_bases` and `StackRoots.feedback_bases` (Task 4).
- Produces: `InlineCaches::forget_families(&mut self, live_bases: &BTreeSet<u32>)`, which drops every family whose code only the family holds and whose base no live closure or activation names, vacates its sites, frees its range and truncates `sites`.

- [ ] **Step 1: Write the failing tests.**

  **`a_closure_keeps_its_feedback_range_across_a_collection`:**
  - `Outer` has `public make = ( ^[:object | object size] )`. Held in a slot, its block's site goes monomorphic on `Array`s.
  - Edit `make` (so the version holding the body is superseded), collect, and run another family that takes the smallest free range.
  - The closure still answers `3` for `#(1 2 3)`, under both lowerings. With `NS_NEXT_STRESS_CACHES=1` it must also answer `3`.

  **`a_hundred_edits_leave_every_table_at_its_first_round_size`** (Review Focus 5):
  - Loop a hundred times: `install` an edit of `Outer>>value` that answers the round's number, `send` `value`, `collect_now()`.
  - After round 1, record `mixin_rows()`, `superseded_count()`, `application_rows()`, the number of `successors` (add `Heap::successor_count()`), `code_rows()`, `mutator.applications.len()`, `mutator.metaclass_mixins.len()`, `caches.table_len()` and `dispatch_ids.high_water()`.
  - After round 100, every one must be equal.

- [ ] **Step 2: Run them to see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(/feedback_range_across|hundred_edits/)'`. Expected: FAIL, because `table_len()` grows.

- [ ] **Step 3: Implement.**

```rust
pub(crate) fn forget_families(&mut self, live_bases: &BTreeSet<u32>) {
    let mut freed = Vec::new();
    self.families.retain(|_, family| {
        let held = Rc::strong_count(&family.code) > 1;
        if held || live_bases.contains(&family.base.raw()) {
            return true;
        }
        freed.push((family.base, family.code.family_size));
        false
    });
    for (base, size) in freed {
        for entry in &mut self.sites[base.raw() as usize..][..size as usize] {
            if let Target::Polymorphic { block, .. } = entry.target {
                self.pool[block as usize] = CacheEntry::vacant_block();
                self.free_blocks.push(block);
            }
            *entry = CacheEntry::vacant();
        }
        self.free_ranges.free(base.raw(), size);
    }
    let mut end = u32::try_from(self.sites.len()).expect("the site table fits a u32");
    while let Some((start, _)) = self.free_ranges.take_ending_at(end) {
        end = start;
    }
    self.sites.truncate(end as usize);
}
```

  - The borrow of `self.sites` and `self.pool` inside the loop needs splitting. Take the fields out as locals first.
  - A base of `0` with `family_size == 0` is never in `families` (`base_for` returns early), so no empty range is freed.
  - In `reclaim_ids`, step 5 is `self.caches.forget_families(&live_bases)`, where `live_bases` is `gathered.feedback_bases` plus `stack.feedback_bases`' raw values.
  - It runs whether or not step 1 freed anything: a code dropped in step 3 can end a family on its own.
  - Rewrite the doc comment of `invalidate`. `families` stays across an invalidation, and a collection's `forget_families` is what drops a family. `family_codes` is gone.

- [ ] **Step 4: Run the tests.** `cargo nextest run -p ns-runtime -E 'test(/feedback_range_across|hundred_edits/)'`, then `cargo nextest run --workspace`, then `NS_NEXT_STRESS_COLLECT=1 NS_NEXT_STRESS_CACHES=1 cargo nextest run -p ns-runtime -E 'test(/reclaim/)'`. Expected: PASS.

- [ ] **Step 5: Gates and commit.** Run `cargo xtask gates commit caches`. Then:

```bash
git commit -am "feat: reclaim the feedback ranges of families nothing runs"
```

---

### Task 7: The design documents

**Files:**
- Modify: `docs/design/object-model.md`:
  - "An application has two ids": the reuse rules are now enforced. Say how: gathering while marking; the order of `reclaim_ids`; the roots, including `recorded_guard`; smallest first; truncation; the requested collection on exhaustion.
  - "The nested-class cache slot": the serial marker. Delete the "What remains" sentences.
- Modify: `docs/design/memory.md`:
  - "Weak references": `mixin_objects` and the tables are weak.
  - The paragraph at `:1640-1663` on the application cache: it is pruned by `reclaim_ids` and is no longer strong.
  - "Marking": the gathering arm.
- Modify: `docs/design/liveness-and-reflection.md`:
  - "What the id has to carry": `Mixin::nested` is now read through ids.
  - "Planned: road step 5b2c": delete the whole section, and remove every reference to it (`grep -rn "5b2c" docs/ crates/`).
- Modify: `docs/design/dispatch.md`, "Invalidation" and "3b — The caches": the selective scrub of a collection, beside the install's full invalidation; feedback ranges are reclaimed.
- Modify: `docs/OPEN-WORK.md`:
  - "Next" names what the road says comes after 5b2c (read the road list in `OPEN-WORK.md`).
  - Remove the 5b2c items.
  - Add any item a task left open: for example, whether the marking arm measured as noise in release, if the controller measured it.
- Modify: `docs/PROGRESS.md`: an entry for road step 5b2c, dated, in the style of the 5b2b entry.
- Modify: `CLAUDE.md`, only if a statement in it is no longer true. `grep -n "append-only\|strong\|5b2c\|nested: Vec" CLAUDE.md`.

- [ ] **Step 1: Find every stale statement.** Run `grep -rn "5b2c\|append-only\|reclaims none\|never reclaimed\|stays strong\|family_codes\|Mixin::nested\|creation_in_progress\|holds .false." docs/ crates/ CLAUDE.md --include=*.md --include=*.rs`. Rewrite each hit to describe the current state.

- [ ] **Step 2: Write the sections** listed above, in the present tense. Name the code by path and function, as the neighbouring paragraphs do.

- [ ] **Step 3: Gates and commit.** This is a Markdown-only commit, unless Step 1 changed a Rust comment. If it did, run `cargo xtask gates commit`. Then:

```bash
git commit -am "docs: the ids are reclaimed in a full collection"
```

The controller then runs `cargo xtask gates end-of-batch` and the `finishing-a-batch` skill. That skill deletes the spec, `docs/superpowers/specs/2026-10-20-road-step-5b2c-reclaiming-ids-design.md`, and `.temp/`.
