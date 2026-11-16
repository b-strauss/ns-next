# Inline allocation (road step 6) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 6. Every small young allocation takes its memory with one compare against a bump limit, through a cursor `ns-memory` lends memory by. Instances, closures, frames, boxed floats and arrays write a precomputed header. Around the bump, three costs go: the per-closure code-id map lookup, the per-`new` chain walk for classes with no initializer code, and the temporary `Vec` behind `Array new:`, `MakeArray` and `MakeVector`.

**Architecture:**
- `Reservation` holds one bump cursor `(top, limit)`, and its `bump` is one compare.
- Eden's top *is* that cursor's top. `Space` sets the limit to just below the scavenge threshold, so the fast path never requests anything.
- Every case the fast path refuses goes to today's `Heap::allocate_block`, made cold, which keeps every rule it has:
  - the reserve,
  - the scavenge request,
  - a spent eden goes old, born remembered,
  - a large object goes old,
  - the collection trigger.
- Headers come from templates:
  - an instance's from its `MixinApplication` row,
  - a closure's, a frame's and a float's from guards the heap keeps for those builtin classes.

**Tech Stack:** Rust 1.98.0, `cargo-nextest`.

**Spec:** `docs/superpowers/specs/2026-10-22-inline-allocation-design.md`. Read it and `docs/design/memory.md` ("Generations", "Allocation: eden, its reserve, and what goes old", "Where a collection can happen: only at a safepoint in the run loop", "The memory layer", "Planned: inline allocation") before Task 1.

**Decided with the user on 2026-10-22** (not reopened; the spec's "Decisions"):

1. One compare against a limit below the scavenge threshold; everything else in the cold `allocate_block`.
2. The bump cursor and its lending live in `ns-memory`.
3. Templates: an instance's on its application row; `Closure`, `Float` and `Frame` from the builtin classes' guards.
4. The scope is the whole allocation path:
   - the code-id cache,
   - `new` without the walk for classes with no initializer code,
   - arrays without a `Vec`.
5. `vm.rs`'s arms are unchanged.

**Refined while planning, stated here for review:**

- *The fast limit sits 8 bytes below the threshold.* `allocate_block` asks for a scavenge once `top - start >= threshold`. A fast bump that ended exactly at the threshold would skip that request. So the limit is `start + threshold - 8`, and an allocation that reaches the threshold always takes the cold path.
- *No large-object test on the fast path.* The fast path takes only objects whose header holds their word count (at most 254 words after the header, 2,040 bytes). That is far below `LARGE_OBJECT_BYTES` (32 KiB).
- *`allocate_object` takes the fast bump too.* Every allocation that computes its guard per call benefits as well (strings, byte arrays and the rest), and nothing calls `allocate_block` directly any more except `allocate_object`'s cold arm and the decoder's placeholders.
- *The code-id cache's hit path still records a method.* `register_code` fills a row's `method` when a method's frame first registers it. The cached path does the same: one table read.
- *A copied `CompiledCode` starts with an empty cache.* A clone is a different body to `register_code`, which keys by the `Rc`'s address. So `CodeIdCache`'s `Clone` answers an empty cache.

## Global Constraints

- **Every commit is green.**
  - Before each commit, run the per-commit gates in `CLAUDE.md` ("The gates") through `cargo xtask gates <groups>`, the groups each task names. Every task runs `commit`.
  - The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, in Task 7, by the controller (`finishing-a-batch` skill). An implementer does not run them, and a task's commit is work in progress until they have run.
- **Commits.** They are conventional (`feat:`, `perf:`, `refactor:`, `test:`, `docs:`) and each ends with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- **Run every command in the foreground.** Pass `timeout: 600000` on builds and test runs, and never stop while a command of your own is still running.
- **Work on the checked-out branch `road-step-6`** in the main working directory, with no worktree.
- **Names are written out** (`CLAUDE.md`, "Conventions"): `words`, `limit`, `offset`, `header`, never `w`, `lim`, `off`, `hdr`. Clippy's `min_ident_chars` enforces the short ones.
- **`unsafe` only in `crates/ns-memory`**, each block with a `// SAFETY:` comment. `ns-runtime` forbids it.
- **`vm.rs` holds no allocation logic.** Its helpers `make_array_into` and `make_vector_into` become one call each into `Mutator`, and no arm changes.
- **Every store into an existing object goes through `heap.rs`'s accessors.** The fast path writes only into the object it has just bumped, before anything can read it, and so skips the barrier like every initializing store.
- **A shipped `.ns` is not touched**, and no golden's expected output changes. If one does, the task is wrong.
- **Never `{:?}` a `Value`, and never `.unwrap()` or `.expect()` an `EvaluationResult`.** Tests copy a neighbour's `expect_ok`.

## Review Focus

1. **The move stress mode shifts eden while the fast path is in use.** After a shift, a fast bump must still stop `REGION_SLACK` short of eden's end and below the shifted threshold. Task 2 pins it with `a_shifted_eden_keeps_its_fast_limit_below_the_threshold`.
2. **The image decoder turns `allocating_old` on and off around its graph pass.** While it is on, every allocation, the fast kinds included, must go old and not be remembered. Task 2 pins it with `while_allocating_old_no_fast_bump_succeeds`, and Task 4 pins it for closures and floats.
3. **Two interpreters in one thread share a `CompiledCode`** (the shipped modules, a test's mixin). Each must get its own code id, and a full collection that frees a row must never leave a stale id in the cache. Task 3 pins it with `two_heaps_holding_one_body_each_get_their_own_id` and `a_freed_row_is_never_answered_from_the_cache`.
4. **A class whose chain has a `<:` level, or whose superclass clause takes arguments**, must never take the instantiation fast path. Task 6 pins it: the debug build's cross-check over every golden under `cargo xtask gates commit`, plus `a_class_with_an_applied_level_takes_the_walk` and `a_superclass_clause_with_arguments_takes_the_walk`.
5. **`Array new:` with a length at the 254-word boundary and at the large-object boundary.** The object must be identical either side of the fast path's edge, and a large array must still go old. Task 5 pins it with `arrays_either_side_of_the_fast_edge_are_alike` and `a_large_array_goes_old`.

---

### Task 1: The bump cursor in `ns-memory`

**Files:**
- Modify: `crates/ns-memory/src/lib.rs` (the `Reservation` struct, `reserve`, `record`, new methods, tests)

**Interfaces:**
- Produces (used by Task 2):
  - `Reservation::bump_top(&self) -> usize`
  - `Reservation::bump_limit(&self) -> usize`
  - `Reservation::set_bump(&mut self, top: usize, limit: usize) -> Result<(), MemoryError>`
  - `Reservation::bump(&mut self, words: u8) -> Option<(usize, &mut [u64])>`
  - `Reservation::bump_array::<const COUNT: usize>(&mut self) -> Option<(usize, &mut [u64; COUNT])>`
  - `Reservation::bump_up_to(&mut self, words: usize, ceiling: usize) -> Option<usize>`

- [ ] **Step 1: Write the failing tests** in `lib.rs`'s `mod tests`:

```rust
    /// A reservation of four pages with the first two committed.
    fn two_committed_pages() -> (Reservation, usize) {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(4 * page).expect("reserve");
        reservation.commit(0..2 * page).expect("commit");
        (reservation, page)
    }

    #[test]
    fn a_bump_lends_exactly_its_words_and_moves_the_top() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(64, page).expect("inside the prefix");
        let (start, words) = reservation.bump(3).expect("room");
        assert_eq!(start, 64);
        assert_eq!(words.len(), 3);
        words.copy_from_slice(&[1, 2, 3]);
        assert_eq!(reservation.bump_top(), 64 + 3 * 8);
        let prefix = reservation.prefix(page).expect("committed");
        assert_eq!(&prefix[8..11], &[1, 2, 3]);
    }

    #[test]
    fn a_bump_past_the_limit_fails_and_leaves_the_top() {
        let (mut reservation, _) = two_committed_pages();
        reservation.set_bump(64, 64 + 2 * 8).expect("inside the prefix");
        assert!(reservation.bump(3).is_none());
        assert_eq!(reservation.bump_top(), 64);
        assert!(reservation.bump(2).is_some());
        assert!(reservation.bump(1).is_none());
    }

    #[test]
    fn a_fixed_bump_lends_an_array() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(0, page).expect("inside the prefix");
        let (start, words) = reservation.bump_array::<3>().expect("room");
        assert_eq!(start, 0);
        *words = [7, 8, 9];
        assert_eq!(reservation.bump_top(), 24);
    }

    #[test]
    fn set_bump_refuses_a_limit_past_the_committed_prefix_or_unaligned() {
        let (mut reservation, page) = two_committed_pages();
        assert_eq!(
            reservation.set_bump(0, 2 * page + 8),
            Err(MemoryError::InvalidRange)
        );
        assert_eq!(reservation.set_bump(4, page), Err(MemoryError::InvalidRange));
        assert_eq!(reservation.set_bump(0, page + 4), Err(MemoryError::InvalidRange));
        assert_eq!(
            reservation.set_bump(reservation.size() + 8, page),
            Err(MemoryError::InvalidRange)
        );
    }

    #[test]
    fn a_limit_below_the_top_refuses_every_bump() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(page, 64).expect("a limit below the top is allowed");
        assert!(reservation.bump(1).is_none());
        assert!(reservation.bump_array::<1>().is_none());
    }

    #[test]
    fn a_decommit_clamps_the_limit_to_the_new_prefix() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(page - 16, 2 * page).expect("inside the prefix");
        reservation.decommit(page..2 * page).expect("decommit");
        assert_eq!(reservation.bump_limit(), page);
        assert!(reservation.bump(2).is_some());
        assert!(reservation.bump(1).is_none());
    }

    #[test]
    fn the_cold_bump_takes_any_size_up_to_a_ceiling_inside_the_prefix() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(0, 0).expect("inside the prefix");
        assert_eq!(reservation.bump_up_to(300, 2 * page), Some(0));
        assert_eq!(reservation.bump_top(), 300 * 8);
        assert_eq!(reservation.bump_up_to(1, 2 * page + 8), None);
        assert_eq!(reservation.bump_up_to(usize::MAX, 2 * page), None);
        assert_eq!(reservation.bump_top(), 300 * 8);
    }
```

- [ ] **Step 2: Run them and see them fail to compile.**

Run: `cargo nextest run -p ns-memory`
Expected: errors for `set_bump`, `bump`, `bump_array`, `bump_top`, `bump_limit`, `bump_up_to`.

- [ ] **Step 3: Implement.**
  1. In `Reservation`, add a field `bump: BumpCursor`, set to `BumpCursor { top: 0, limit: 0 }` in `reserve`.
  2. Above `Reservation`, add the cursor type:

```rust
/// Where the next bump allocation begins and where none may end, in byte
/// offsets. The limit never lies past the committed read-write prefix
/// (`set_bump` checks it, and `record` clamps it when the prefix
/// shrinks), and the top is a multiple of 8 inside the reservation, so a
/// bump that stays within the limit lends committed, initialized words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BumpCursor {
    top: usize,
    limit: usize,
}
```

  3. In `record`, after `self.accessible_prefix = …`, add:

```rust
        // A shrunken prefix takes the cursor's limit with it, so no bump
        // lends memory that is no longer committed.
        self.bump.limit = self.bump.limit.min(self.accessible_prefix);
```

  4. Add the methods to `impl Reservation`:

```rust
    /// Where the next bump begins.
    #[inline(always)]
    pub fn bump_top(&self) -> usize {
        self.bump.top
    }

    /// Where no bump may end past.
    pub fn bump_limit(&self) -> usize {
        self.bump.limit
    }

    /// Places the bump cursor. Both offsets are multiples of 8; `top` lies
    /// inside the reservation and `limit` inside the committed read-write
    /// prefix. A `limit` below `top` is allowed, and refuses every bump
    /// until the cursor is placed again. `InvalidRange` otherwise, the
    /// cursor untouched.
    pub fn set_bump(&mut self, top: usize, limit: usize) -> Result<(), MemoryError> {
        let aligned = top.is_multiple_of(8) && limit.is_multiple_of(8);
        if !aligned || top > self.size || limit > self.accessible_prefix {
            return Err(MemoryError::InvalidRange);
        }
        self.bump = BumpCursor { top, limit };
        Ok(())
    }

    /// Bumps the cursor by `words` words when they end within its limit,
    /// and lends exactly those words, whatever they hold, with their byte
    /// offset; `None`, the cursor untouched, otherwise. One comparison:
    /// a `u8` count cannot carry the top past the end of the address
    /// space, and the subtraction saturates when the limit lies below the
    /// top.
    #[inline(always)]
    pub fn bump(&mut self, words: u8) -> Option<(usize, &mut [u64])> {
        let start = self.bump.top;
        let bytes = usize::from(words) * 8;
        if bytes > self.bump.limit.saturating_sub(start) {
            return None;
        }
        self.bump.top = start + bytes;
        // SAFETY: `start + bytes <= limit <= accessible_prefix`, so every
        // byte lent is committed read-write and initialized; `start` is a
        // multiple of 8 above a page-aligned base, so the words are
        // aligned; the cursor moved past them, so no later bump lends them
        // again, and `&mut self` keeps every other slice away while this
        // one lives.
        let words = unsafe {
            std::slice::from_raw_parts_mut(self.address(start).cast::<u64>(), usize::from(words))
        };
        Some((start, words))
    }

    /// `bump` for a fixed number of words, lent as an array, so that a
    /// caller's stores into it carry no index check.
    #[inline(always)]
    pub fn bump_array<const COUNT: usize>(&mut self) -> Option<(usize, &mut [u64; COUNT])> {
        const { assert!(COUNT <= u8::MAX as usize) };
        let start = self.bump.top;
        let bytes = COUNT * 8;
        if bytes > self.bump.limit.saturating_sub(start) {
            return None;
        }
        self.bump.top = start + bytes;
        // SAFETY: as for `bump`, for `COUNT` words; an array of `u64` has
        // their alignment.
        let words = unsafe { &mut *self.address(start).cast::<[u64; COUNT]>() };
        Some((start, words))
    }

    /// The cold bump, of any size, within `ceiling` rather than the limit:
    /// answers the byte offset of the first word, and lends nothing (the
    /// caller writes through `prefix_mut`). `None`, the cursor untouched,
    /// when the words would end past `ceiling` or `ceiling` lies past the
    /// committed read-write prefix.
    pub fn bump_up_to(&mut self, words: usize, ceiling: usize) -> Option<usize> {
        let start = self.bump.top;
        let end = words
            .checked_mul(8)
            .and_then(|bytes| start.checked_add(bytes))?;
        if end > ceiling || ceiling > self.accessible_prefix {
            return None;
        }
        self.bump.top = end;
        Some(start)
    }
```

  5. Extend the crate doc comment's last sentence. After "which is lent in O(1)." add: "The heap's young allocations bump a cursor the reservation holds (`Reservation::bump`), one comparison against a limit inside that prefix."

- [ ] **Step 4: Run the tests.**

Run: `cargo nextest run -p ns-memory`
Expected: every test passes.

- [ ] **Step 5: Gates and commit.**

Run: `cargo xtask gates commit memory` (timeout 600000)
Expected: all green.

```bash
git add crates/ns-memory/src/lib.rs
git commit -m "feat: a bump cursor in the reservation, one comparison per bump"
```

(with the two trailer lines from Global Constraints)

---

### Task 2: Eden's top is the cursor; the fast bump; `allocate_block` cold

**Files:**
- Modify: `crates/ns-runtime/src/heap/space.rs` (`Space`, `region`, `occupied`, `set_region_extent`, `flip_survivors`, `empty_nursery`, `allocate_young`, `Space::new`, new methods, tests)
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap::new`, `set_allocating_old`, `set_scavenge_threshold`, `allocate_object`, `allocate_block`, new `count_young_allocation`, tests)

**Interfaces:**
- Consumes: Task 1's `Reservation` methods.
- Produces (used by Tasks 4–6):
  - `Space::bump_object(&mut self, header: Header, class: u64) -> Option<(NonZeroU64, &mut [u64])>`: writes the header and class word, answers the header's offset and the fields, which are unwritten.
  - `Space::bump_object_array::<const FIELDS: usize>(&mut self, header: Header, class: u64) -> Option<(NonZeroU64, &mut [u64; FIELDS])>`: the same for a fixed field count.
  - `Heap::count_young_allocation(&mut self)`.
  - `Space::set_young_threshold(&mut self, threshold: Option<usize>)`.
  - `fn assert_no_poison(words: &[u64])` (heap.rs, `#[cfg(debug_assertions)]`).

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`, beside `an_allocation_that_finds_eden_spent_goes_old_born_remembered_and_asks_for_a_scavenge`:

```rust
    #[test]
    fn a_fast_bump_up_to_the_threshold_asks_for_nothing_and_the_next_asks_for_a_scavenge() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        // 32 words a block: the header, the class word and 30 fields.
        let fields = 30;
        let block_bytes = 8 * (fields + 2);
        let eden = heap.space.region(RegionName::Eden);
        while heap.space.region(RegionName::Eden).top + block_bytes
            < eden.start + heap.scavenge_threshold
        {
            heap.allocate_instance(class, fields);
            assert!(!heap.scavenge_requested);
        }
        heap.allocate_instance(class, fields);
        assert!(heap.scavenge_requested, "the allocation that reached the threshold asks");
    }

    #[test]
    fn while_allocating_old_no_fast_bump_succeeds() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.set_allocating_old(true);
        let decoded = heap.allocate_instance(class, 2);
        heap.set_allocating_old(false);
        assert!(!space::is_young(decoded.offset));
        assert!(!heap.header(decoded).remembered());
        assert!(space::is_young(heap.allocate_instance(class, 2).offset));
    }

    #[test]
    fn a_fast_and_a_cold_allocation_write_the_same_words() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let fast = heap.allocate_instance(class, 3);
        assert!(space::is_young(fast.offset));
        // The same object made by the cold path: eden refused.
        heap.space.set_young_threshold(Some(0));
        let cold = heap.allocate_instance(class, 3);
        heap.space.set_young_threshold(Some(heap.scavenge_threshold));
        assert!(space::is_young(cold.offset), "the cold path still bumps eden");
        assert_eq!(heap.block(fast), heap.block(cold));
    }

    #[test]
    fn an_object_of_254_words_takes_the_fast_path_and_one_of_255_takes_an_overflow_word() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let top = heap.space.region(RegionName::Eden).top;
        let largest = heap.allocate_instance(class, 253);
        assert_eq!(largest.offset.get() as usize, top, "no overflow word");
        let top = heap.space.region(RegionName::Eden).top;
        let overflowing = heap.allocate_instance(class, 254);
        assert_eq!(overflowing.offset.get() as usize, top + 8, "an overflow word first");
    }
```

In `space.rs`'s `mod tests`:

```rust
    #[test]
    fn a_shifted_eden_keeps_its_fast_limit_below_the_threshold() {
        let mut space = Space::new(SMALL).expect("reserve");
        let threshold = EDEN_BYTES / 2;
        space.set_young_threshold(Some(threshold));
        space.set_region_extent(RegionName::Eden, EDEN_START + 64, EDEN_START + 64);
        let limit = space.reservation.bump_limit();
        assert_eq!(limit, EDEN_START + 64 + threshold - 8);
        space.set_young_threshold(Some(EDEN_BYTES));
        assert_eq!(space.reservation.bump_limit(), NURSERY_END - REGION_SLACK);
        space.set_young_threshold(None);
        assert!(space.reservation.bump_limit() <= space.region(RegionName::Eden).top);
    }

    #[test]
    fn a_flip_and_an_emptied_nursery_put_eden_back_at_its_base() {
        let mut space = Space::new(SMALL).expect("reserve");
        space.set_young_threshold(Some(EDEN_BYTES / 2));
        space.set_region_extent(RegionName::Eden, EDEN_START + 64, EDEN_START + 640);
        space.flip_survivors();
        let eden = space.region(RegionName::Eden);
        assert_eq!((eden.start, eden.top), (EDEN_START, EDEN_START));
        space.set_region_extent(RegionName::Eden, EDEN_START, EDEN_START + 640);
        space.empty_nursery();
        let eden = space.region(RegionName::Eden);
        assert_eq!((eden.start, eden.top), (EDEN_START, EDEN_START));
    }
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-runtime -E "test(fast) | test(shifted_eden) | test(emptied_nursery) | test(allocating_old)"`
Expected: compile errors for `set_young_threshold`.

- [ ] **Step 3: Implement `space.rs`.**
  1. Replace the field `eden: Region` with:

```rust
    /// Where eden's objects begin: `EDEN_START`, except after the move
    /// stress mode shifted them up. Eden's top is the reservation's bump
    /// cursor, never a second copy of it.
    eden_start: usize,
    /// Eden's occupied bytes at which an allocation asks for a scavenge,
    /// or `None` while the image decoder allocates old, which no fast bump
    /// may serve.
    young_threshold: Option<usize>,
```

  2. In `Space::new`, initialize `eden_start: EDEN_START, young_threshold: None`. After the struct is built, call `space.place_eden_top(EDEN_START)` and answer `Ok(space)`.
  3. In `region`, answer `RegionName::Eden => self.eden()` with:

```rust
    fn eden(&self) -> Region {
        Region {
            base: EDEN_START,
            start: self.eden_start,
            top: self.reservation.bump_top(),
            end: NURSERY_END,
        }
    }
```

  4. In `occupied`, use `self.eden_start..self.reservation.bump_top()`.
  5. In `set_region_extent`, handle eden first:

```rust
        if name == RegionName::Eden {
            debug_assert!(EDEN_START <= start && start <= top && top <= NURSERY_END);
            self.eden_start = start;
            self.place_eden_top(top);
            return;
        }
```

     Then drop the `RegionName::Eden` arm from the `match`, which now reaches only `PastSurvivor` and `Old`. Answer `unreachable!()` for `Eden`.
  6. In `flip_survivors` and `empty_nursery`, replace `self.eden = Region::empty(...)` with `self.eden_start = EDEN_START; self.place_eden_top(EDEN_START);`.
  7. Replace `allocate_young` (the cold path's bump into the reserve):

```rust
    #[inline]
    pub(in crate::heap) fn allocate_young(&mut self, words: usize) -> Option<usize> {
        self.reservation.bump_up_to(words, NURSERY_END - REGION_SLACK)
    }
```

  8. Add:

```rust
    /// Sets eden's scavenge threshold, or `None` while nothing may be
    /// allocated young by the fast bump, and places the limit to match.
    pub(in crate::heap) fn set_young_threshold(&mut self, threshold: Option<usize>) {
        self.young_threshold = threshold;
        self.place_eden_top(self.reservation.bump_top());
    }

    /// Puts eden's top at `top` and the fast bump's limit 8 bytes below
    /// the threshold, so that an allocation reaching the threshold always
    /// takes the cold path, which asks for the scavenge
    /// (`Heap::allocate_block`) -- never past `REGION_SLACK` short of
    /// eden's end, which the move stress mode needs. With no threshold,
    /// the limit is eden's start, which no bump fits under.
    fn place_eden_top(&mut self, top: usize) {
        let limit = match self.young_threshold {
            Some(threshold) => (self.eden_start + threshold)
                .saturating_sub(8)
                .min(NURSERY_END - REGION_SLACK),
            None => self.eden_start,
        };
        if self.reservation.set_bump(top, limit).is_err() {
            unreachable!("internal: eden lies inside the committed prefix");
        }
    }

    /// The offset a fast bump's object would begin at: eden's top, past
    /// every survivor space and so never 0.
    #[inline(always)]
    pub(in crate::heap) fn young_top(&self) -> NonZeroU64 {
        match NonZeroU64::new(self.reservation.bump_top() as u64) {
            Some(offset) => offset,
            None => unreachable!("internal: eden's top lies past the survivor spaces"),
        }
    }

    /// The fast bump of a young object that `header`'s count fits, at most
    /// 254 words after the header: writes `header` and the class word
    /// `class`, and answers the header's offset and the object's fields,
    /// which hold whatever eden held; the caller writes every one before
    /// anything reads the object. `None` past the fast limit.
    #[inline(always)]
    pub(in crate::heap) fn bump_object(
        &mut self,
        header: Header,
        class: u64,
    ) -> Option<(NonZeroU64, &mut [u64])> {
        let word_count = header.word_count();
        debug_assert!(word_count < WORD_COUNT_OVERFLOW);
        let (start, block) = self.reservation.bump(word_count as u8 + 1)?;
        block[HEADER] = header.0;
        block[CLASS] = class;
        Some((young_offset(start), &mut block[FIELDS..]))
    }

    /// `bump_object` for an object of exactly `BLOCK_WORDS` words, its
    /// header included, which `header`'s count must say, lent whole as an
    /// array: the caller writes its fields from `FIELDS` on.
    #[inline(always)]
    pub(in crate::heap) fn bump_object_array<const BLOCK_WORDS: usize>(
        &mut self,
        header: Header,
        class: u64,
    ) -> Option<(NonZeroU64, &mut [u64; BLOCK_WORDS])> {
        debug_assert_eq!(header.word_count() + 1, BLOCK_WORDS);
        let (start, block) = self.reservation.bump_array::<BLOCK_WORDS>()?;
        block[HEADER] = header.0;
        block[CLASS] = class;
        Some((young_offset(start), block))
    }
```

  and, outside `impl Space`:

```rust
/// The offset of a bumped object: eden lies past the survivor spaces, so
/// never 0.
#[inline(always)]
fn young_offset(start: usize) -> NonZeroU64 {
    match NonZeroU64::new(start as u64) {
        Some(offset) => offset,
        None => unreachable!("internal: eden lies past the survivor spaces"),
    }
}
```

  Import the layout constants with `use crate::heap::object::{CLASS, FIELDS, HEADER};`. `young_top` uses `young_offset(self.reservation.bump_top())` too.

- [ ] **Step 4: Implement `heap.rs`.**
  1. In `Heap::new`, bind the literal to `let mut heap = Heap { ... };`, then `heap.space.set_young_threshold(Some(heap.scavenge_threshold)); heap`.
  2. In `set_allocating_old`, after the assignment:

```rust
        let threshold = (!allocating_old).then_some(self.scavenge_threshold);
        self.space.set_young_threshold(threshold);
```

  3. In `set_scavenge_threshold`, after the assignment:

```rust
        if !self.allocating_old {
            self.space.set_young_threshold(Some(bytes));
        }
```

  4. Add beside `allocate_block`:

```rust
    /// Counts a fast bump's object, as `allocate_block` counts its own.
    #[inline(always)]
    fn count_young_allocation(&mut self) {
        self.young_objects += 1;
        self.object_count += 1;
    }
```

  5. In `allocate_object`, try the fast bump first:

```rust
        let guard = self.header_dispatch_id(class);
        let class_word = self.store(Value::object(class));
        let word_count = field_count + 1;
        if word_count < WORD_COUNT_OVERFLOW {
            let header = Header::new(guard, format, unused, word_count).with_negative(negative);
            if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
                fields.fill(Word::NIL.bits());
                self.count_young_allocation();
                let first = offset.get() as usize / 8 + FIELDS;
                return (self.object_handle(offset), first..first + field_count);
            }
        }
        self.allocate_block(class_word, guard, format, unused, negative, field_count)
```

  6. Mark `allocate_block` `#[cold]` and `#[inline(never)]`. Add to its doc comment: "The cold path of every allocation: a fast bump (`Space::bump_object`) takes every young object whose header holds its count while eden is below its threshold, so this is reached for an object that crosses the threshold, one eden cannot hold, a large one, one with an overflow word, and everything the image decoder allocates."
  7. Add the debug helper below `allocate_block`:

```rust
/// Asserts that a fast bump's object holds no word a scavenge poisoned:
/// every field of an object filled explicitly, rather than with nil, must
/// have been written. Asked only of pointer fields, whose valid words never
/// carry the poison's tag.
#[cfg(debug_assertions)]
fn assert_no_poison(words: &[u64]) {
    assert!(
        !words.contains(&header::POISON),
        "internal: a fresh object kept a word the scavenge poisoned"
    );
}
```

     `header::POISON` is `pub(super)` in `heap/header.rs`, which `heap.rs` can see. Import it as `header::POISON` under `#[cfg(debug_assertions)]`.

- [ ] **Step 5: Run the heap and space tests, then the suite.**

Run: `cargo nextest run -p ns-runtime -E "test(heap::) | test(space::)"` (timeout 600000)
Expected: all pass, the new ones included.

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: all pass.

- [ ] **Step 6: Gates and commit.**

Run: `cargo xtask gates commit` (timeout 600000)

```bash
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/heap/space.rs
git commit -m "perf: a fast bump in eden, one comparison, before the cold allocate_block"
```

---

### Task 3: A closure's and a frame's code id without the map

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CompiledCode`: new field `code_id_cache`, and every place that builds one or destructures it, `CompiledCodeFields` included)
- Modify: `crates/ns-runtime/src/heap/tables.rs` (new `CodeIdCache`)
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap` field `code_table_epoch`, `register_code`, new `next_code_table_epoch`, tests)
- Modify: `crates/ns-runtime/src/heap/reclaim.rs` (`free_unnamed_codes` renews the epoch)

**Interfaces:**
- Produces: `Heap::code_id` and `Heap::code_id_of_method`, with signatures unchanged, now answering from the cache on a hit. `CodeIdCache` is `pub(crate)`, with `Default`, `Clone` (empty) and `Debug`.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`:

```rust
    #[test]
    fn a_body_answers_its_id_from_the_cache_after_the_first_lookup() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        let id = heap.code_id(&code);
        assert_eq!(code.code_id_cache.get(), Some((heap.code_table_epoch, id)));
        heap.tables.code_ids.clear();
        assert_eq!(heap.code_id(&code), id, "a hit never reaches the map");
    }

    #[test]
    fn two_heaps_holding_one_body_each_get_their_own_id() {
        let mut first = Heap::new();
        let mut second = Heap::new();
        let filler = shaped_code(0, 1);
        let code = shaped_code(0, 2);
        second.code_id(&filler);
        let in_first = first.code_id(&code);
        let in_second = second.code_id(&code);
        assert_ne!(in_first, in_second);
        assert_eq!(first.code_id(&code), in_first);
        assert_eq!(second.code_id(&code), in_second);
    }

    #[test]
    fn a_freed_row_is_never_answered_from_the_cache() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        let other = shaped_code(0, 2);
        let id = heap.code_id(&code);
        heap.free_unnamed_codes(&BitSet::default());
        let reused = heap.code_id(&other);
        assert_eq!(reused, id, "the smallest free row is minted first");
        let again = heap.code_id(&code);
        assert_ne!(again, reused, "the cache's id was freed and reused");
        assert!(Rc::ptr_eq(&heap.code(again).expect("a row"), &code));
    }

    #[test]
    fn a_cloned_body_starts_with_an_empty_cache() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        heap.code_id(&code);
        let copy = Rc::new((*code).clone());
        assert_eq!(copy.code_id_cache.get(), None);
    }

    #[test]
    fn a_cached_body_still_records_the_method_a_frame_registers_it_with() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(crate::BuiltinClass::Object);
            let application = mutator.heap().class_application(object).expect("a class");
            let method = application
                .mixin
                .compiled_methods()
                .next()
                .expect("Object has a method")
                .clone();
            // A body no frame has registered yet: a fresh copy, so the
            // bootstrap's own registration does not answer for it.
            let copy = std::rc::Rc::new(crate::mixin::MixinMethod {
                code: std::rc::Rc::new((*method.code).clone()),
                ..(*method).clone()
            });
            let id = mutator.heap_mut().code_id(&copy.code);
            assert!(mutator.heap().code_method(id).is_none());
            assert_eq!(mutator.heap_mut().code_id_of_method(&copy), id);
            assert!(std::rc::Rc::ptr_eq(
                &mutator.heap().code_method(id).expect("the method recorded"),
                &copy
            ));
        });
    }
```

The last test builds the copy by struct update. If `MixinMethod` is not `Clone`, or has private fields, build the copy the way `mixin.rs`'s own tests build a `MixinMethod`. `BitSet` is `crate::heap::reclaim::BitSet`, which has `Default`.

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-runtime -E "test(cache) | test(own_id) | test(freed_row)"`
Expected: compile errors for `code_id_cache` and `code_table_epoch`.

- [ ] **Step 3: Implement.**
  1. In `heap/tables.rs`:

```rust
/// A body's code id in the last heap that asked for it, with that heap's
/// code-table epoch (`Heap::code_table_epoch`): a one-entry cache in front of
/// `Heap::register_code`'s map, read by every `MakeClosure` and every
/// frame reified. The epoch is unique per heap of the process and renewed
/// whenever a full collection frees code rows, so an id from another heap,
/// or one since freed, is never answered. A copy of a body is another body
/// to the map, keyed by its address, so a copy starts empty.
#[derive(Default)]
pub(crate) struct CodeIdCache(std::cell::Cell<Option<(u64, CodeId)>>);

impl CodeIdCache {
    pub(crate) fn get(&self) -> Option<(u64, CodeId)> {
        self.0.get()
    }

    pub(crate) fn set(&self, epoch: u64, id: CodeId) {
        self.0.set(Some((epoch, id)));
    }
}

impl Clone for CodeIdCache {
    fn clone(&self) -> CodeIdCache {
        CodeIdCache::default()
    }
}

impl std::fmt::Debug for CodeIdCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CodeIdCache")
    }
}
```

     `CodeId` must be `Copy` (it is a row index; check). Make `CodeIdCache` reachable from `bytecode.rs` as `crate::heap::CodeIdCache`, re-exported from `heap.rs` the way `CodeId` is.
  2. In `bytecode.rs`, add to `CompiledCode`:

```rust
    /// This body's id in the heap that last asked (`Heap::code_id`), so a
    /// closure or frame made of it finds its id without the heap's map.
    /// Never on any wire; a copy starts empty.
    pub(crate) code_id_cache: crate::heap::CodeIdCache,
```

     Initialize it with `CodeIdCache::default()` wherever a `CompiledCode` is built (`from_builder` and any other constructor; let the compiler list them). Add it to `CompiledCodeFields` with `#[expect(dead_code, reason = "named so that a writer decides to leave it out")]`, as `family_size` is.
  3. In `heap.rs`, add a process-wide counter beside `next_heap_id`, in every build:

```rust
/// Hands every heap's code table a distinct epoch, and every full
/// collection that frees a code row a fresh one (`CodeIdCache`).
fn next_code_table_epoch() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
```

     Add a field `code_table_epoch: u64` to `Heap`, set to `next_code_table_epoch()` in `Heap::new`, and make it `pub(crate)` to the heap module like `tables`.
  4. In `register_code`, before the map lookup:

```rust
        if let Some((epoch, id)) = code.code_id_cache.get() {
            if epoch == self.code_table_epoch {
                if let Some(method) = method {
                    if let Some(entry) = self.tables.codes[id.index()].as_mut() {
                        if entry.method.is_none() {
                            entry.method = Some(method.clone());
                        }
                    }
                }
                return id;
            }
        }
```

     At both places where it answers an id (found in the map, or newly minted), call `code.code_id_cache.set(self.code_table_epoch, id)` first.
  5. In `heap/reclaim.rs`'s `free_unnamed_codes`, track whether any row was freed (`let mut freed = false;`, set to `true` in the `if let Some(entry)` arm), and after `truncate_trailing` add:

```rust
        if freed {
            // Every body's cached id may name a freed row now.
            self.code_table_epoch = super::next_code_table_epoch();
        }
```

- [ ] **Step 4: Run the tests and the suite.**

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: all pass.

- [ ] **Step 5: Gates and commit.**

Run: `cargo xtask gates commit` (timeout 600000)

```bash
git add crates/ns-runtime/src/bytecode.rs crates/ns-runtime/src/heap.rs crates/ns-runtime/src/heap/tables.rs crates/ns-runtime/src/heap/reclaim.rs
git commit -m "perf: a body caches its code id per heap, so a closure skips the map"
```

---

### Task 4: Closures, frames and floats from templates

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs`:
  - a new `BuiltinGuards` and `Heap::set_builtin_guards`
  - `allocate_float`, `allocate_closure`, `allocate_closure_from_registers`, `allocate_frame`
  - tests
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`refresh_immediate_dispatch_ids` also sets the builtin guards)

**Interfaces:**
- Consumes: Task 2's `Space::bump_object`, `Space::bump_object_array`, `Heap::count_young_allocation` and `assert_no_poison`.
- Produces: `Heap::set_builtin_guards(&mut self, guards: BuiltinGuards)` and `pub(crate) struct BuiltinGuards { pub(crate) closure: DispatchId, pub(crate) frame: DispatchId }`. The float's guard is already the heap's `immediate_dispatch_ids[4]`; name that index `FLOAT_DISPATCH_INDEX` if it has no name yet.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`. Each compares a fast object with the same object made by the cold path, the way Task 2's `a_fast_and_a_cold_allocation_write_the_same_words` does, refusing the fast path with `heap.space.set_young_threshold(Some(0))`:
  1. **`a_fast_float_is_the_cold_float`:**
     - `allocate_float(class, 1e300)` both ways; the blocks are equal.
     - `heap.float_value(object)` (or the accessor the float leaves use; search `fn float_` in `heap.rs`) answers `1e300`.
  2. **`a_fast_closure_is_the_cold_closure`:**
     - Copy the setup of the existing closure test (search `allocate_closure_from_registers(` in `heap.rs`'s tests) and make the closure both ways.
     - Assert equal blocks, and that the closure's code id answers the same body.
  3. **`a_fast_frame_is_the_cold_frame`:**
     - Copy the setup of an existing `reify` test (search `heap.reify(` in the tests), reify two equal activations both ways, and assert equal blocks except the `MARRIED` field.
  4. **`a_closure_and_a_float_allocated_while_allocating_old_go_old`:**
     - Under `set_allocating_old(true)`, both are old and not remembered.

  Each test sets the guards first with `heap.set_builtin_guards(BuiltinGuards { closure: …, frame: … })`. Use the guards the class objects answer through `header_dispatch_id`; a bare heap's classes answer `DispatchId::NONE`, which is fine for equality.

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-runtime -E "test(fast_float) | test(fast_closure) | test(fast_frame) | test(allocated_while_allocating_old)"`
Expected: compile errors for `set_builtin_guards` and `BuiltinGuards`.

- [ ] **Step 3: Implement.**
  1. **The guards.** Add `builtin_guards: BuiltinGuards` to `Heap`, `BuiltinGuards::NONE` (both `DispatchId::NONE`) in `Heap::new`, and the setter:

```rust
    /// The guards a closure's and a frame's headers carry, read off the
    /// `Closure` and `Activation` classes, so a fast bump writes them
    /// without looking them up (`Mutator::refresh_immediate_dispatch_ids`,
    /// once when the builtin classes are built and again when the kernel
    /// gives them their applications).
    pub(crate) fn set_builtin_guards(&mut self, guards: BuiltinGuards) {
        self.builtin_guards = guards;
    }
```

  2. **Where the guards are set.** In `Mutator::refresh_immediate_dispatch_ids`, after `set_immediate_dispatch_ids`:

```rust
        let guards = crate::heap::BuiltinGuards {
            closure: instances(self, BuiltinClass::Closure),
            frame: instances(self, BuiltinClass::Activation),
        };
        self.heap.set_builtin_guards(guards);
```

  3. **`allocate_float`:**

```rust
        let class_word = self.store(Value::object(class));
        let header = Header::new(
            self.immediate_dispatch_ids[FLOAT_DISPATCH_INDEX],
            Format::SixtyFourBit,
            0,
            2,
        );
        if let Some((offset, block)) = self.space.bump_object_array::<3>(header, class_word.bits()) {
            block[FIELDS] = value.to_bits();
            self.count_young_allocation();
            return self.object_handle(offset);
        }
        let (object, fields) = self.allocate_object(class, Format::SixtyFourBit, 0, false, 1);
        self.space.words_mut()[fields].fill(value.to_bits());
        object
```

     Add `debug_assert_eq!(header.guard(), self.header_dispatch_id(class))` before the bump.
  4. **`allocate_closure_from_registers`.** After `fixed` and the debug `store` loop:

```rust
        let field_count = closure_field::COUNT + copied.len();
        let class_word = self.store(Value::object(class));
        if field_count + 1 < WORD_COUNT_OVERFLOW {
            let header = Header::new(self.builtin_guards.closure, Format::Closure, 0, field_count + 1);
            debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
            let Heap { space, stack, .. } = self;
            if let Some((offset, fields)) = space.bump_object(header, class_word.bits()) {
                let (fixed_slots, copied_slots) = fields.split_at_mut(closure_field::COUNT);
                write_words(fixed_slots, fixed);
                let base = register_base.index as usize;
                for (slot, register) in copied_slots.iter_mut().zip(copied) {
                    *slot = stack.values[base + usize::from(*register)].bits();
                }
                #[cfg(debug_assertions)]
                assert_no_poison(fields);
                self.count_young_allocation();
                return self.object_handle(offset);
            }
        }
```

     The existing `allocate_object` path stays below as the cold path. `fields` was split, so the poison check reborrows it after the two halves are done. If the borrow checker refuses, check `fixed_slots` and `copied_slots` separately.
  5. **`allocate_closure`.** The same shape, with `copied.iter().map(|value| value.word())` for the copied words.
  6. **`allocate_frame`.** After `fixed` and the debug loop:

```rust
        let field_count = frame_field::COUNT + register_count;
        let class_word = self.store(Value::object(class));
        if field_count + 1 < WORD_COUNT_OVERFLOW {
            let header = Header::new(self.builtin_guards.frame, Format::Frame, 0, field_count + 1);
            debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
            if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
                let (fixed_slots, register_slots) = fields.split_at_mut(frame_field::COUNT);
                write_words(fixed_slots, fixed);
                let (given, rest) = register_slots.split_at_mut(registers.len());
                write_words(given, registers.iter().map(|register| register.word()));
                rest.fill(Word::NIL.bits());
                #[cfg(debug_assertions)]
                assert_no_poison(fields);
                self.count_young_allocation();
                return self.frame_handle(offset);
            }
        }
```

     Keep the rest of `allocate_frame` as the cold path. Check `write_words`'s signature (it takes an iterator of `Word` into a `&mut [u64]`) and adapt the calls to it.

- [ ] **Step 4: Run the tests and the suite.**

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: all pass. A `debug_assert_eq!` on a guard that fires means the template's guard and the class's application disagree. Find which refresh point is missing; do not weaken the assertion.

- [ ] **Step 5: Gates and commit.**

Run: `cargo xtask gates commit` (timeout 600000)

```bash
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/interpreter/mod.rs
git commit -m "perf: closures, frames and boxed floats bump eden with a precomputed header"
```

---

### Task 5: Arrays without a `Vec`

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (new `allocate_nil_array`, `allocate_array_from_registers`, `array_fits`; tests)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (new `Mutator::new_nil_array`, `Mutator::new_array_from_registers`)
- Modify: `crates/ns-runtime/src/builtins/collections.rs` (`allocate`)
- Modify: `crates/ns-runtime/src/vm.rs` (`make_array_into`, `make_vector_into`: one call each)

**Interfaces:**
- Consumes: Task 2's fast path, through `allocate_object`.
- Produces:
  - `Heap::allocate_nil_array(&mut self, class: ClassReference<'gc>, length: usize) -> ObjectReference<'gc>`
  - `Heap::allocate_array_from_registers(&mut self, class: ClassReference<'gc>, register_base: RegisterBase, registers: &[u16]) -> ObjectReference<'gc>`
  - `Heap::array_fits(&self, length: usize) -> bool`
  - `Mutator::new_nil_array(&mut self, length: usize) -> ObjectReference<'gc>`
  - `Mutator::new_array_from_registers(&mut self, register_base: RegisterBase, registers: &[u16]) -> ObjectReference<'gc>`

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests`:

```rust
    #[test]
    fn a_nil_array_is_the_array_of_nils() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let made = heap.allocate_nil_array(class, 5);
        let listed = heap.allocate_array(class, &[Value::nil(); 5]);
        assert_eq!(heap.block(made).to_vec(), heap.block(listed).to_vec());
    }

    #[test]
    fn arrays_either_side_of_the_fast_edge_are_alike() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        for length in [252, 253, 254, 255] {
            let array = heap.allocate_nil_array(class, length);
            assert!(space::is_young(array.offset));
            assert_eq!(heap.array_element_count(array), Some(length));
            assert!(heap
                .array_element(array, length - 1)
                .is_some_and(|value| value.is_nil()));
        }
    }

    #[test]
    fn a_large_array_goes_old() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let array = heap.allocate_nil_array(class, LARGE_OBJECT_BYTES / 8);
        assert!(!space::is_young(array.offset));
        assert!(heap.header(array).remembered());
    }

    #[test]
    fn a_length_past_the_old_space_does_not_fit() {
        let heap = Heap::new();
        assert!(heap.array_fits(10));
        assert!(!heap.array_fits(usize::MAX / 8));
    }
```

Add a Newspeak-level test beside the existing `Array new:` tests (search `cannot allocate an Array` in `crates/ns-runtime/tests/` and `builtins/collections.rs`). If no test pins it yet, add one: `Array new: 1 << 60` answers an `ArgumentError` whose message contains `cannot allocate an Array`, through the neighbouring tests' helper. Also check that `Array new: 300` answers an array of size 300, all nil.

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-runtime -E "test(array)"`
Expected: compile errors for `allocate_nil_array` and `array_fits`.

- [ ] **Step 3: Implement.**
  1. **`heap.rs`:**

```rust
    /// An `Array`-shaped object of `class` of `length` nils: `Array new:`'s
    /// and `MakeVector`'s, with no list of elements in between.
    pub(crate) fn allocate_nil_array(
        &mut self,
        class: ClassReference<'gc>,
        length: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::IndexablePointers, 0, false, length)
            .0
    }

    /// An `Array`-shaped object of `class` holding the registers
    /// `registers` names in the window at `register_base`, in order:
    /// `MakeArray`'s, with each value going from its register straight
    /// into its element.
    pub(crate) fn allocate_array_from_registers(
        &mut self,
        class: ClassReference<'gc>,
        register_base: RegisterBase,
        registers: &[u16],
    ) -> ObjectReference<'gc> {
        #[cfg(debug_assertions)]
        for register in registers {
            self.store(self.register(register_base, *register));
        }
        let (object, fields) =
            self.allocate_object(class, Format::IndexablePointers, 0, false, registers.len());
        let base = register_base.index as usize;
        let Heap { space, stack, .. } = self;
        for (slot, register) in space.words_mut()[fields].iter_mut().zip(registers) {
            *slot = stack.values[base + usize::from(*register)].bits();
        }
        object
    }

    /// Whether an `Array` of `length` elements can be allocated, asked
    /// before a length the program chose is. One below the large-object
    /// threshold is allocated young, where eden is committed, and needs no
    /// probe; a larger one is asked of `fields_fit`, as `byte_array_fits`
    /// asks for a `ByteArray`.
    pub(crate) fn array_fits(&self, length: usize) -> bool {
        let small = record_size::block_words(length)
            .checked_mul(8)
            .is_some_and(|bytes| bytes < LARGE_OBJECT_BYTES);
        small || self.fields_fit(length)
    }
```

     `allocate_array_from_registers` goes through `allocate_object`'s range, so an array of 254 words or more takes the cold path. That path writes nils first, and the copy then overwrites them, as `allocate_array` already does.
  2. **`interpreter/mod.rs`, beside `new_array`:**

```rust
    /// An `Array` of `length` nils.
    pub(crate) fn new_nil_array(&mut self, length: usize) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.heap.allocate_nil_array(class, length)
    }

    /// An `Array` of the registers `registers` names: `MakeArray`'s.
    pub(crate) fn new_array_from_registers(
        &mut self,
        register_base: crate::heap::RegisterBase,
        registers: &[u16],
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.heap
            .allocate_array_from_registers(class, register_base, registers)
    }
```

  3. **`vm.rs`.** `make_array_into`'s body becomes `let value = Value::object(mutator.new_array_from_registers(register_base, element_registers)); mutator.heap.set_register(register_base, destination, value);`. `make_vector_into`'s becomes the same with `mutator.new_nil_array(usize::from(size))`.
  4. **`collections.rs`'s `allocate`.** Replace the `Vec` and `try_reserve_exact` from `let mut elements` to the end:

```rust
    if !mutator.heap.array_fits(length) {
        return Some(Err(cannot_allocate()));
    }
    Some(Ok(Value::object(
        mutator.heap.allocate_nil_array(class, length),
    )))
```

     Rewrite the doc comment's last sentence: "A size that cannot be allocated is an error, found by `Heap::array_fits` before anything is allocated, never an abort: the size is the program's, and an abort is not catchable." Update the comment above `cannot_allocate` the same way: "like one `array_fits` refuses".
  5. **The probe's description.** In `Heap::byte_array_fits`'s doc comment, "probed as `Array new:` probes it" becomes "probed as `Array new:` probes a large array".

- [ ] **Step 4: Run the tests and the suite.**

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: all pass.

- [ ] **Step 5: Check `vm::run`.** Build release with debug info (`cargo build -p ns-cli --release`, with the `[profile.release]` debug setting the measuring skill names). Run `dumpbin /disasm` as the `measuring-performance` skill describes. Check that there is still one `vm::run`, with no `run_stretch` and no `vm::run::closure` of its own. If one appears, report it and stop; do not commit around it.

- [ ] **Step 6: Gates and commit.** `vm.rs` changed, but not the instruction encoding, so `images` does not run.

Run: `cargo xtask gates commit` (timeout 600000)

```bash
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/src/builtins/collections.rs crates/ns-runtime/src/vm.rs crates/ns-runtime/tests
git commit -m "perf: Array new:, MakeArray and MakeVector allocate with no Vec in between"
```

---

### Task 6: `new` without the walk for a class with no initializer code

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs`:
  - a new `InstanceHeader`
  - `register_application` sets `instance_header`
  - a new `class_instantiation_facts` and `chain_has_applied_level`
  - a new `allocate_instance_with`
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication`: `instance_header`, `instantiates_without_code`; a new `fn instantiates_without_code`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin` computes the fact)
- Modify: `crates/ns-runtime/src/types.rs:736` and `crates/ns-runtime/src/dispatch.rs:447,516` (the other `MixinApplication` literals: `instance_header: None, instantiates_without_code: false`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`decide_factory`, `decide_instantiation`, `instantiate_allocating`; tests)

**Interfaces:**
- Consumes: Task 2's `Space::bump_object`.
- Produces:
  - `pub struct InstanceHeader(u64)` (`Copy`, `Debug`, `PartialEq`), and `InstanceHeader::for_slots(guard: DispatchId, slot_count: usize) -> Option<InstanceHeader>` in the heap module.
  - `MixinApplication::instance_header: Option<InstanceHeader>` and `MixinApplication::instantiates_without_code: bool`.
  - `Heap::class_instantiation_facts(&self, class) -> Option<InstantiationFacts>`, with `pub(crate) struct InstantiationFacts { pub(crate) refused: bool, pub(crate) inherits_value: bool, pub(crate) slot_count: usize, pub(crate) header: Option<InstanceHeader>, pub(crate) without_code: bool, pub(crate) factory_arity: Option<usize> }`.
  - `Heap::chain_has_applied_level(&self, class) -> bool`.
  - `Heap::allocate_instance_with(&mut self, class, header: InstanceHeader, slot_count: usize) -> ObjectReference<'gc>`.

- [ ] **Step 1: Write the failing tests** in `interpreter/instantiate.rs`'s `mod tests`. Use that module's existing loading and instantiation helpers (`load`, `expect_ok`, `interpreter.instantiate`); copy a neighbouring test's setup.
  1. **`a_class_with_no_initializer_code_answers_its_instance_without_an_activation`:**
     - Module `class Root = () (public class Empty = () ())`.
     - Instantiate `Empty` through a send from a method (`public make = ( ^Empty new )`).
     - Assert the class's `application.instantiates_without_code` is `true`, and that `Empty new` answers an instance of `Empty`.
  2. **`a_class_with_a_slot_takes_the_walk`:**
     - `class Holder = (| x = 3. |) ( public x = (^x) )`.
     - Assert `instantiates_without_code` is `false`, and `Holder new x` answers `3`.
  3. **`a_superclass_clause_with_arguments_takes_the_walk`:**
     - `class Base value: v = (| public value = v. |) ()` and `class Derived = Base value: 4 ()`.
     - Assert `Derived`'s fact is `false`, and `Derived new value` answers `4`.
  4. **`a_class_with_an_applied_level_takes_the_walk`:**
     - Reuse the module of `goldens/language/class/mixin/mixin_body_less_application.ns` (copy its source into an `indoc!`).
     - Assert the instantiation answers what the golden's `.toml` records.
     - Assert `heap.chain_has_applied_level` is `true` for the applying class.
  5. **`a_wrong_argument_count_still_raises_the_arity_error`:**
     - `class Empty = () ()` sent `new` through `perform:withArguments:` with one argument (copy how the existing arity tests send it; search `wrong number of factory arguments`).
     - Assert the same error text as today.

In `heap.rs`'s tests:
  6. **`an_instance_with_a_template_is_the_instance_without`:**
     - `allocate_instance_with(class, InstanceHeader::for_slots(guard, 3).expect("small"), 3)` against `allocate_instance(class, 3)`.
     - The blocks are equal.
  7. **`no_template_past_254_words`:**
     - `InstanceHeader::for_slots(DispatchId::NONE, 253)` is `Some`, and `for_slots(DispatchId::NONE, 254)` is `None`.

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-runtime -E "test(instantiate::) | test(template)"`
Expected: compile errors for `instantiates_without_code`, `InstanceHeader` and `allocate_instance_with`.

- [ ] **Step 3: Implement the heap's half.**
  1. **`InstanceHeader`:**

```rust
/// The header an ordinary instance of an application is born with --
/// its guard, `FixedPointers`, its word count -- computed once when the
/// application is registered, so that `new` writes it without looking
/// anything up. Only for an instance whose header holds its count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceHeader(u64);

impl InstanceHeader {
    pub(crate) fn for_slots(guard: DispatchId, slot_count: usize) -> Option<InstanceHeader> {
        let word_count = slot_count + 1;
        (word_count < WORD_COUNT_OVERFLOW)
            .then(|| InstanceHeader(Header::new(guard, Format::FixedPointers, 0, word_count).0))
    }
}
```

  2. **`register_application`.** Before the row is stored, set `application.instance_header = InstanceHeader::for_slots(application.dispatch_ids.instances, application.slot_count);`.
  3. **The facts:**

```rust
/// What `new` reads of a class's application, copied out in one lookup.
#[derive(Clone, Copy)]
pub(crate) struct InstantiationFacts {
    pub(crate) refused: bool,
    pub(crate) inherits_value: bool,
    pub(crate) slot_count: usize,
    pub(crate) header: Option<InstanceHeader>,
    pub(crate) without_code: bool,
    pub(crate) factory_arity: Option<usize>,
}

    /// What `new` needs of `class`'s application, read once and copied
    /// out; `None` for a class object with no application.
    pub(crate) fn class_instantiation_facts(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<InstantiationFacts> {
        let application = self.class_application_entry(class)?;
        Some(InstantiationFacts {
            refused: application.instantiation_refusal.is_some(),
            inherits_value: application.inherits_value,
            slot_count: application.slot_count,
            header: application.instance_header,
            without_code: application.instantiates_without_code,
            factory_arity: application
                .mixin
                .factory
                .map(|factory| usize::from(factory.argument_count)),
        })
    }

    /// Whether any class object of `class`'s superclass chain, `class`
    /// included, is a level a `<:` clause added: a fact of the class
    /// objects, not of their applications, since a class object may name
    /// another declaration's applied level as its superclass.
    pub(crate) fn chain_has_applied_level(&self, class: ClassReference<'gc>) -> bool {
        let mut level = Some(class);
        while let Some(current) = level {
            if self.class_is_applied_level(current) {
                return true;
            }
            level = self.class_superclass(current);
        }
        false
    }

    /// An instance of `class` with `slot_count` nil fields under `header`,
    /// which `class`'s application computed.
    pub(crate) fn allocate_instance_with(
        &mut self,
        class: ClassReference<'gc>,
        header: InstanceHeader,
        slot_count: usize,
    ) -> ObjectReference<'gc> {
        let header = Header(header.0);
        debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
        debug_assert_eq!(header.word_count(), slot_count + 1);
        let class_word = self.store(Value::object(class));
        if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
            fields.fill(Word::NIL.bits());
            self.count_young_allocation();
            return self.object_handle(offset);
        }
        self.allocate_instance(class, slot_count)
    }
```

     Adapt `factory.argument_count`'s conversion to its type (`u16` or `usize`).

- [ ] **Step 4: Implement the fact.** In `mixin.rs`:

```rust
/// Whether instantiating an application of `mixin` over `superclass` runs
/// no code: no level of the chain needs initializer code, every level's
/// superclass clause names its superclass's primary factory selector and
/// passes it no arguments, and every level above the first takes no
/// factory arguments. A fact of the application chain alone; whether a
/// class object's chain has a `<:` level is asked of the class objects
/// (`Heap::chain_has_applied_level`).
pub(crate) fn instantiates_without_code(
    mixin: &Mixin,
    superclass: Option<&Rc<MixinApplication>>,
) -> bool {
    if crate::interpreter::instantiate::mixin_needs_initializer_code(mixin) {
        return false;
    }
    let Some(superclass) = superclass else {
        return true;
    };
    let selector = mixin.superclass.map_or(Symbol::NEW, |clause| clause.selector);
    let expected = superclass
        .mixin
        .factory
        .map_or(Symbol::NEW, |factory| factory.selector);
    let takes_no_arguments = superclass
        .mixin
        .factory
        .is_none_or(|factory| factory.argument_count == 0);
    let passes_no_arguments = !mixin.superclass.is_some_and(|clause| clause.has_arguments);
    selector == expected
        && passes_no_arguments
        && takes_no_arguments
        && superclass.instantiates_without_code
}
```

  Check the exact module path of `mixin_needs_initializer_code` (`interpreter/instantiate.rs`, `pub(crate)`) and of `Symbol`.

  Add the two fields to `MixinApplication`, each with a doc comment:
  - `pub instance_header: Option<crate::heap::InstanceHeader>`: "set by `Heap::register_application`".
  - `pub instantiates_without_code: bool`: "computed by `apply_mixin`; see `instantiates_without_code`".

  In `apply_mixin`, compute `let instantiates_without_code = crate::mixin::instantiates_without_code(mixin, superclass);` and set both fields in the literal (`instance_header: None`; registration fills it).

- [ ] **Step 5: Implement the fast path** in `instantiate.rs`.
  1. **`decide_factory`.** The default arm becomes `_ => self.decide_instantiation_from(class, arguments)`, taking `&[Value<'gc>]`. Keep `decide_instantiation(class, Vec)` for its other callers; it becomes `self.decide_instantiation_from(class, &factory_arguments)`.
  2. **`decide_instantiation_from`:**

```rust
    /// `decide_instantiation` for arguments the caller lends. A class
    /// whose application runs no code, called with its factory's arity
    /// and with no `<:` level in its chain, is allocated and answered at
    /// once; every other walks the chain (`instantiate_allocating`).
    fn decide_instantiation_from(
        &mut self,
        class: ClassReference<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(facts) = self.heap.class_instantiation_facts(class) {
            let arity_matches = facts.factory_arity.is_none_or(|arity| arity == arguments.len());
            let value_refused =
                facts.inherits_value && self.heap.class_has_value_refusal(class);
            if facts.without_code
                && !facts.refused
                && !value_refused
                && arity_matches
                && !self.heap.chain_has_applied_level(class)
            {
                let instance = match facts.header {
                    Some(header) => self.heap.allocate_instance_with(class, header, facts.slot_count),
                    None => self.heap.allocate_instance(class, facts.slot_count),
                };
                #[cfg(debug_assertions)]
                self.assert_the_walk_agrees(class, instance, arguments);
                return Ok(Step::Value(Value::object(instance)));
            }
        }
        self.instantiate_allocating(class, arguments.to_vec(), Heap::allocate_instance)
    }

    /// The fast path's oracle in a debug build: the full walk over the
    /// instance the fast path made must answer that same instance, with no
    /// code to run.
    #[cfg(debug_assertions)]
    fn assert_the_walk_agrees(
        &mut self,
        class: ClassReference<'gc>,
        instance: ObjectReference<'gc>,
        arguments: &[Value<'gc>],
    ) {
        let state = self.walk_state(Value::object(instance));
        match self.climb_levels(state, class, arguments.to_vec()) {
            Ok(Step::Value(value)) => assert!(
                value.as_object().is_some_and(|object| object.same(instance)),
                "internal: the walk answered another instance than the fast path"
            ),
            _ => panic!("internal: the fast instantiation skipped code the walk runs"),
        }
    }
```

     Use the identity comparison `ObjectReference` has (`same`, per `object-model.md`). If `Step` has no wildcard-friendly variants, add the arms explicitly.
  3. **The refusal paths.** A refused class, a value-refused class, or one with a wrong arity falls through to `instantiate_allocating`, which raises exactly as today. The existing refusal and arity tests pin it.

- [ ] **Step 5b: The walk reads per-level facts instead of cloning applications.** Every instantiation that still walks benefits, classes with initializers and `<:` levels included. Today `check_factory_arity`, `clause_message_body` (twice: the level's application and `primary_factory_selector`'s) and `class_has_initializer_code` (once per level per `initializing_level_below` call) each clone the level's application `Rc`. The facts they read belong to the level's own mixin alone, so they are copied out once per level.
  1. In `mixin.rs`:

```rust
/// What the instantiation walk asks of one level, read off the level's own
/// mixin when its application is registered, so the walk copies it out
/// instead of cloning the application: whether the level needs initializer
/// code, its superclass clause's selector and whether that clause passes
/// arguments, and its primary factory's selector and arity. Nothing here
/// depends on the superclass: the walk compares a level's clause selector
/// with its superclass's factory selector itself, from both levels' facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelFacts {
    pub needs_initializer_code: bool,
    pub clause_selector: Symbol,
    pub clause_has_arguments: bool,
    pub factory_selector: Symbol,
    pub factory_arity: Option<usize>,
}

impl LevelFacts {
    pub(crate) fn of(mixin: &Mixin) -> LevelFacts {
        LevelFacts {
            needs_initializer_code:
                crate::interpreter::instantiate::mixin_needs_initializer_code(mixin),
            clause_selector: mixin.superclass.map_or(Symbol::NEW, |clause| clause.selector),
            clause_has_arguments: mixin.superclass.is_some_and(|clause| clause.has_arguments),
            factory_selector: mixin.factory.map_or(Symbol::NEW, |factory| factory.selector),
            factory_arity: mixin.factory.map(|factory| usize::from(factory.argument_count)),
        }
    }
}
```

     Add `pub level_facts: LevelFacts` to `MixinApplication`, documented "set by `Heap::register_application` from `mixin`". `register_application` sets it with `LevelFacts::of_mixin(&application.mixin)`. The other literals (`types.rs`, `dispatch.rs`) set `LevelFacts::of_mixin(&mixin)` from the mixin they hold. `InstantiationFacts::factory_arity` above reads `application.level_facts.factory_arity`.
  2. In `heap.rs`:

```rust
    /// The instantiation walk's facts of `class`'s level, copied out of its
    /// application without cloning it; `None` for a class object with no
    /// application.
    pub(crate) fn class_level_facts(&self, class: ClassReference<'gc>) -> Option<LevelFacts> {
        self.class_application_entry(class)
            .map(|application| application.level_facts)
    }
```

  3. In `instantiate.rs`, the three readers ask `class_level_facts` and touch the application only where they must hand out code or build an error, which are cold:
     - **`check_factory_arity`:** `let declared_arity = self.heap.class_level_facts(class).and_then(|facts| facts.factory_arity);`, then the rest unchanged.
     - **`clause_message_body`:**
       - `let facts = self.heap.class_level_facts(applying);`
       - `let selector = facts.map_or(Symbol::NEW, |facts| facts.clause_selector);`
       - `let expected = self.heap.class_level_facts(superclass).map(|facts| facts.factory_selector);`
       - The error branch is unchanged. Then `if !facts.is_some_and(|facts| facts.clause_has_arguments) { return Ok(None); }`. Only past that line is the application cloned to take `superclass_message_code`.
     - **`primary_factory_selector`** (also used by `applied_clause_message_body` and the error texts): reads `class_level_facts(class).map(|facts| facts.factory_selector)`.
     - **`class_has_initializer_code`:** reads `class_level_facts(class).is_some_and(|facts| facts.needs_initializer_code)`.

     The walk's shape, its order and every error text stay as they are. Only where the facts come from changes.
  4. Test, in `instantiate.rs`'s tests, `level_facts_agree_with_the_mixin`: for every application of a loaded module's chain (walk `superclass` from a class's application up), assert `application.level_facts == LevelFacts::of_mixin(&application.mixin)`. The goldens, under both lowerings, pin the behaviour.

- [ ] **Step 6: Run the tests and the suite in debug** (the cross-check runs on every fast instantiation of every golden):

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: all pass, with no "internal: the fast instantiation…" panic.

- [ ] **Step 7: Gates and commit.**

Run: `cargo xtask gates commit` (timeout 600000)

```bash
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/mixin.rs crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/src/interpreter/instantiate.rs crates/ns-runtime/src/types.rs crates/ns-runtime/src/dispatch.rs
git commit -m "perf: new of a class with no initializer code is a bump and a nil fill"
```

---

### Task 7: Measure, document, run the end of the batch (controller)

**Files:**
- Modify:
  - `docs/design/memory.md`: "Planned: inline allocation" becomes a present-tense section; "The memory layer" names the cursor; the code list names `Reservation::bump`.
  - `docs/design/object-model.md`: "Instantiation" names the fast path and its conditions.
  - `docs/design/execution.md`: a "What inline allocation moved" section with the measurement.
  - `docs/PROGRESS.md`: the entry for road step 6.
  - `docs/OPEN-WORK.md`:
    - "Next" becomes **`new` as compiled code** (decided with the user on 2026-10-22, before road step 7). The primary factory runs as code the compiler emits, as both references do, instead of the walk in Rust:
      - Squeak-Newspeak compiles it as `basicNew` followed by an ordinary send to the initializer (`prior-art/newspeak/Newspeak2SqueakCompilation.ns:1991-2020`); SOMns does the same in `MixinBuilder.assemblePrimaryFactoryMethod`.
      - Then `new` takes the inline caches, and road steps 8 and 9 can inline it.
      - The entry records why the walk exists. Milestone 1 built it in Rust, and Milestone 4's step 2a made it a state machine with its semantics unchanged. 3b took the factory's place in the method table from the references but kept it bodiless, and the `<:` clauses were built into the walk. No document weighed compiled code against it.
      - It names what the compiled form must keep: §6.1's superclass-selector check at run time, the per-level arity check an untrusted image relies on, and the deferred `<:` messages just before their level's initializer.
      - Road step 7 follows it.
    - The road's step 6 line is marked built.
    - A new "clean blocks" item records a block that needs neither `self`, outer registers, an implicit receiver nor `^`, which needs no frame and could be one closure made once, as Cog's clean blocks are: a lowering change still to be weighed.
    - The "Factory entry carrying its bases" item stays.
    - The overcommit item names `Heap::array_fits` instead of `try_reserve_exact` for `Array new:`.
  - `CLAUDE.md`: in `crates/ns-memory`'s entry, after "(page-range reservation on Windows and POSIX)", add "and the bump cursor eden's young allocations take their memory from".
- Delete: `docs/superpowers/specs/2026-10-22-inline-allocation-design.md`.

- [ ] **Step 1: Measure.** Follow the `measuring-performance` skill and `execution.md`'s "What removing the re-entries moved".
  - **Trees:** `master` and the commits of Tasks 2, 3, 4, 5 and 6, so each part's share is seen.
  - **Builds:** three pinned release builds of each (`-C metadata=alpha|beta|gamma`), `cargo xtask shipped-code` before each.
  - **Sources:** the eight micro sources (write `bench_new.ns`, `bench_new2.ns`, `bench_newarg.ns` and `bench_do.ns` out verbatim from `execution.md`, "What spec-conformant lookup moved", into `.temp/`, never reconstructed), and the sixteen Are We Fast Yet programs at the skill's iteration counts.
  - **Report:** per-build numbers, pooled medians and spreads.
  - **Stop and report to the user before going on** if any part is slower than `master` beyond its spread, or does not pay.
- [ ] **Step 2: Document.** Write the documents above in the present tense. The measurement goes into `execution.md` with its protocol line, and `memory.md`'s section states the rules the spec's "Invariants the path touches" lists. Delete the spec.
- [ ] **Step 3: End of the batch.** Run `cargo xtask gates end-of-batch` (timeout 600000 per call; split it into `images`, `caches`, `benchmarks` and `stress` if one call outlives the timeout). Then follow the `finishing-a-batch` skill.
- [ ] **Step 4: Commit the documentation.**

```bash
git add docs CLAUDE.md
git commit -m "docs: inline allocation is built (road step 6)"
```
