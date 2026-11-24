# Generations Implementation Plan (Road Step 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the object space a young generation: eden and two survivor spaces at the low end of the reservation, below the old space; a Cheney scavenger with age-based tenuring; a remembered bit and set behind `heap.rs`'s store accessors; large objects allocated old; a full collection that marks and compacts both generations into the old space; and an image load that tenures once before the decoder allocates its graph old.

**Architecture:** Five stages, each landing green, then the measurement, the documentation and the gates. The space is split into its four regions while every allocation still goes to the old region, so the split is reviewed with no behaviour change (Task 1). The header's age and remembered bits, the write barrier, the remembered set and the old-allocation rules come next, behind a young-allocation switch that is off outside the heap's own tests (Task 2). The scavenger is built and tested on bare heaps (Task 3), then the full collection learns to compact the nursery's survivors into the old space (Task 4). Task 5 wires both into the run loop, the statistics and the image load, and switches young allocation on for good. Task 6 adds the fifth stress mode with the remembered-set verification and the poisoning of vacated nursery memory.

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime`; nothing changes in `ns-memory`. No new dependency. Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/memory.md`, "Planned: generations" (the whole section), "The object space", "Marking", "Compaction and forwarding", "The trigger and the growth policy", "Weak references", "Frames, the activation stack and the marriage", "The gates the collector needs", "The move stress mode"; `docs/design/image-format.md`, "Planned: the image under generations"; `docs/OPEN-WORK.md`, "Next". Background: `docs/design/memory.md`, "What the collector traces", "A resolved future is traced as its value", "The amplification bound of a rejected load", "Planned: inline allocation" (road step 6, which builds on this plan's eden reserve). Prior art: Spur's layout (`setHeapBase:memoryLimit:endOfMemory:`, `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:60020`), scavenger (`doScavenge:`, `:48477`; `scavengeReferentsOf:`, `:43278`), remembered set (`remember:`, `:42843`) and full collection (`fullGC`, `:50168`); Strongtalk's layout (`prior-art/strongtalk/vm/memory/universe.cpp:64-79`), scavenge (`universe_more.cpp:81-137`), age table (`ageTable.cpp:37-46`) and full collection, which compacts the old generation and then the new one into it (`markSweep.cpp:255`, `:264`); Primordial Soup's barrier (`prior-art/primordialsoup/vm/object.h:257-273`), large objects (`heap.h:111`), remembered-set overflow (`heap.h:112`, `heap.cc:279-283`) and snapshot allocation (`heap.cc:228`).

**Decided in this plan's session (2026-10-04, with the user):**

1. **A missed write barrier is found by verification and poisoning, not page protection.** In a debug build, under `NS_NEXT_STRESS_SCAVENGE`, every scavenge first walks the old space and asserts that every old object naming a young one carries the remembered bit, and that the remembered set and the bits agree, as Strongtalk's `VerifyBeforeScavenge`/`rSet::verify` and HotSpot's `VerifyBeforeGC` do. Every debug-build scavenge then fills the nursery memory it vacated with a poison word whose format is `Free`, and `Heap::block` refuses such a header in a debug build, as HotSpot's `ZapUnusedHeapArea` does. No page is protected: `Space::words` lends a safe slice over `[0, top)`, which covers the nursery, and a `&[u64]` over no-access pages would be undefined behaviour. The design's "alternates eden between two regions and protects the vacated one" is replaced (Task 8), and the decision of 2026-10-03 that nothing is protected stands.
2. **The full collection marks both generations and compacts them into the old space, as Strongtalk and HotSpot's Serial and Parallel collectors do.** One forwarding table covers both: the old space's survivors slide down to its bottom, the nursery's survivors follow them in address order, and every handle is rewritten in one pass. The nursery is empty afterwards and the remembered set with it. Rejected: Spur's mark, then scavenge-and-tenure, then compact the old space, which relocates twice per full collection; and tenure-everything-then-mark-compact, which keeps young garbage reachable only from dead remembered objects alive until the next full collection.
3. **The nursery's sizes follow HotSpot's ratio**: eden 8 MiB and two survivor spaces of 1 MiB each (`SurvivorRatio=8`), 10 MiB of nursery from the reservation's base. Objects of 32 KiB and more are allocated old (Primordial Soup's `kLargeAllocationSize`). A remembered set past 8,192 entries requests a scavenge that tenures everything (Soup's `kRememberedSetOverflowSize`). The tenuring threshold adapts by Strongtalk's age table, which HotSpot took over: start at 15, the widest the header's four age bits hold, so nothing is tenured by age at first; after each scavenge, the lowest age at which the survivors of that age and younger exceed half a survivor space (HotSpot's `TargetSurvivorRatio=50`, `MaxTenuringThreshold=15`).
4. **An image load tenures once, between the platform paths and the graph pass.** Spur, Soup, Strongtalk and HotSpot's CDS all load into a heap that holds nothing young; here the builtins, the module classes and what the platform paths answered are young when the graph pass starts, and one scavenge that tenures everything, between two entries, makes all three old. The decoder then allocates its graph old, as Soup's `AllocateSnapshot` does, and its stores are old-to-old. The design's two tenures (one right after the bootstrap, one after the paths) are replaced, because the bootstrap's objects and `build_module_classes` are made in the same entry, with no point between them where a collection may run (Task 8).

**Chosen by this plan, for the user to confirm at review** (each a constant one line changes, each stated where it is built): eden asks for a scavenge once it holds 7.5 MiB, keeping a 512 KiB reserve (`EDEN_RESERVE`) for what a stretch allocates before its safepoint; an allocation that finds eden spent goes old, born remembered, and asks for the scavenge, as road step 6's design already says; every nursery region keeps 264 bytes free past what it fills (`REGION_SLACK`), so the move stress mode's shift of up to 256 bytes never pushes an object out of its region; the old space's first commit is 256 KiB past the nursery, which is committed whole when the heap is made; a scavenge does not replace resolved futures, which stays the full collection's work ("A resolved future is traced as its value"); the remembered-set verification runs only under `NS_NEXT_STRESS_SCAVENGE`, since it walks the old space at every scavenge.

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic. The barrier lives in `heap.rs`'s store accessors and nowhere else; no instruction arm learns about generations.
  - An image is untrusted input: no byte sequence may panic. The decoder's allocations change only in where they land (old); every check, bound and pre-allocation stays. No wire tag is added, removed or renumbered.
  - A handle means nothing against a `Heap` that did not mint it, nor across a move. A scavenge moves objects, so it advances the debug build's move epoch exactly as a move or a compaction does, and rewrites every Rust-side handle it traces into the new epoch, old ones included.
  - The brand needs no cast; `Heap::load`/`Heap::store` stay `pub(crate)`.
  - A running activation is an `ActivationReference`; only a `FrameReference` is ever stored. A frame's `MARRIED` field keeps holding a stack index. `bytecode::outer_reach_of` and `compact::narrow_outer_reach` are not touched.
  - Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`. Tests use a local `expect_ok` helper.
  - The VM mirror's six locks stay as they are. The inline caches hold no handle and are untouched.
  - A collection of any kind -- a scavenge, a full collection, a move -- happens only at a safepoint of the run loop or between entries, where only roots hold values. An allocation sets a request at most.
- **Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`** and its submodules. A handle's offset stays private to the heap module.
- **The header layout is `heap/header.rs`'s**, unchanged: bits 27-30 are the age, bit 31 the remembered bit; this plan is the first to write them. Outside a scavenge no object is forwarded; outside a collection no object is marked; a young object never carries the remembered bit.
- **Young means an offset below `NURSERY_END`**, one comparison against a constant, in the barrier and wherever else a generation is asked.
- **A golden that changes is a behaviour change.** No task in this plan changes or adds golden output; if one changes, stop and report it.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's `min_ident_chars` refuses one- and two-character names. Comments explain the decision, not the mechanics. Classic terminology.
- A Newspeak module inline in a test is an `indoc! {"…"}` or `formatdoc!`, indented with the Rust around it, the closing `"}` on its own line.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01DhtAcwQi6qo4racmc1iK4i`.
  A subagent's commit names the subagent's own model.
- Work happens on the branch `feature/generations` in the main working directory (no worktree), created from `master` (`f7d8efdb`) before Task 1.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"), every task, however small:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace`
  - `cargo test --workspace --doc`
  - `cargo run -p ns-analyzer -- parse-corpus corpus/modules` → "parsed 211, failed 0"

  Tasks 1, 3 and 5 also run `cargo nextest run --workspace --release` (the size tests run only there). The stress modes, `run-corpus`, the benchmarks profile and `verifyPlugin` run once, in Task 9 (the `finishing-a-batch` skill). A commit that never saw them is work in progress; a subagent implementing a task is told so. Nothing under `crates/ns-syntax`, `crates/ns-memory` or `editors/jetbrains` changes.
- Run every command in the foreground, except Task 7's measurement, which the controller session runs itself.

## Review Focus

- **An old object that comes to name a young one, through every store path.** An instance field, an `Array` element, a class object's field, an object's class word (`object_set_class`), a dead frame's register (`frame_set_local`, what `StoreOuterLocal` reaches), and the registers a divorce copies into a frame that was tenured while married: after each store and a scavenge the young object is intact and the old one reads it back. Pinned in Task 2 (the remembered bit after each store) and Task 3 (the read-back after a scavenge, the divorce case included).
- **A young object reachable only through the remembered set, scavenge after scavenge, until it is tenured.** Its old holder stays remembered while the object is young and leaves the set once it is old; a promoted object that names a survivor is remembered by the scavenge that promoted it. Pinned in Task 3.
- **More survivors than a survivor space holds.** A scavenge whose live young data exceeds 1 MiB promotes what does not fit and loses nothing, and the next scavenge finds the promoted objects' young references. Pinned in Task 3.
- **A full collection whose nursery holds survivors.** Objects young and old, each reachable only through the other generation, come through a full collection in one old space, in address order, with every handle rewritten and an empty remembered set. Pinned in Task 4.
- **A load whose graph names a module class and what a platform path answered.** Both are young when the load begins; after the load's tenure the decoder's old graph names them with no remembered entry needed, and the program runs and comes through a scavenge and a full collection intact. Pinned in Task 5 (an image round trip in `tests/image.rs` with a platform path, followed by `scavenge_now` and `collect_now`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/src/heap/space.rs` | the four regions, their constants, young/old/survivor bump allocation, the occupied ranges every walk reads, the survivor flip and the empty nursery (Task 1); `allocation_range` (Task 3) |
| `crates/ns-runtime/src/heap/header.rs` | the age, the remembered bit and `with_format` (Task 2); the poison word (Task 6) |
| `crates/ns-runtime/src/heap/compaction.rs` | `ForwardingTable` with a base and two segments (Tasks 1, 4); `Shift` per region (Task 1) |
| `crates/ns-runtime/src/heap/scavenge.rs` (new, Task 3) | `Scavenger`: evacuation, forwarders, the Cheney scan, the age table and the threshold |
| `crates/ns-runtime/src/heap.rs` | the walks over the regions (Task 1); the barrier, the remembered set, the allocation rules, the young-allocation switch (Task 2); `Heap::scavenge` and its weak-symbol pass (Task 3); `compact` over both generations (Task 4); the triggers, statistics, the switch removed (Task 5); the verification and the poison check (Task 6) |
| `crates/ns-runtime/src/heap/world.rs` | `StepRoots`, one enumeration of the step state's roots for marking, forwarding and scavenging; `World::scavenge` (Task 5) |
| `crates/ns-runtime/src/interpreter/safepoint.rs` | the scavenge request served at the safepoint; `scavenge_now`, `set_scavenge_threshold` (Task 5); `StressSchedule` and the scavenge stress mode (Task 6) |
| `crates/ns-runtime/src/interpreter/mod.rs` | the `stress_scavenge` field (Task 6); `tenure_now` (Task 5) |
| `crates/ns-runtime/src/image/mod.rs` | the load's tenure and the graph pass allocating old (Task 5) |
| `crates/ns-runtime/src/lib.rs` | `HeapStatistics`/`CollectorStatistics` gain their fields, re-exported as today (Task 5) |
| `crates/ns-cli/src/entry.rs`, `crates/ns-cli/tests/cli.rs` | the statistics line names scavenges (Task 5) |
| `crates/ns-runtime/tests/collector.rs`, `tests/stress.rs`, `tests/image.rs` | Tasks 5 and 6 |
| `.claude/skills/finishing-a-batch/SKILL.md` | the stress loop gains `SCAVENGE` (Task 6) |
| `docs/**`, `CLAUDE.md` | Task 8 |

**The interfaces later tasks rely on** (exact signatures; `space.rs` unless named):

```rust
pub(in crate::heap) const SURVIVOR_BYTES: usize = 1 << 20;
pub(in crate::heap) const EDEN_BYTES: usize = 8 << 20;
pub(in crate::heap) const EDEN_START: usize = 2 * SURVIVOR_BYTES;
pub(in crate::heap) const NURSERY_END: usize = EDEN_START + EDEN_BYTES;
pub(in crate::heap) const REGION_SLACK: usize = 8 * 33;
pub(in crate::heap) fn is_young(offset: NonZeroU64) -> bool;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) enum RegionName { PastSurvivor, Eden, Old }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) struct Region { pub(in crate::heap) base: usize, pub(in crate::heap) start: usize, pub(in crate::heap) top: usize, pub(in crate::heap) end: usize }

impl Space {
    pub(in crate::heap) fn region(&self, name: RegionName) -> Region;
    pub(in crate::heap) fn future_survivor(&self) -> Region;
    pub(in crate::heap) fn occupied(&self) -> [Range<usize>; 3];          // byte ranges, address order: past survivor, eden, old
    pub(in crate::heap) fn occupied_bytes(&self) -> usize;
    pub(in crate::heap) fn allocate_young(&mut self, words: usize) -> Option<usize>;
    pub(in crate::heap) fn allocate_old(&mut self, words: usize) -> Option<usize>;
    pub(in crate::heap) fn allocate_survivor(&mut self, words: usize) -> Option<usize>;
    pub(in crate::heap) fn set_region_extent(&mut self, name: RegionName, start: usize, top: usize);
    pub(in crate::heap) fn flip_survivors(&mut self);                    // Task 3
    pub(in crate::heap) fn empty_nursery(&mut self);                     // Task 4
    pub(in crate::heap) fn ensure_committed(&mut self, end: usize) -> Option<()>;
    pub(in crate::heap) fn release_above(&mut self, keep: usize);
}
pub(in crate::heap) fn allocation_range(words: &[u64], header: usize) -> Range<usize>;  // Task 3

// header.rs, Task 2
pub(super) const MAX_AGE: u8 = 15;
impl Header {
    pub(super) fn age(self) -> u8;
    pub(super) fn with_age(self, age: u8) -> Header;
    pub(super) fn remembered(self) -> bool;
    pub(super) fn with_remembered(self, remembered: bool) -> Header;
    pub(super) fn with_format(self, format: Format) -> Header;            // keeps the unused units 0
}

// heap.rs, Task 2
const LARGE_OBJECT_BYTES: usize = 32 << 10;
const REMEMBERED_SET_LIMIT: usize = 8 << 10;
const EDEN_RESERVE: usize = 512 << 10;
impl Heap<'_> {
    fn write_barrier(&mut self, holder: NonZeroU64, word: Word);
    fn remember(&mut self, holder: NonZeroU64);
    pub(crate) fn set_allocating_old(&mut self, allocating_old: bool);
}

// heap/scavenge.rs, Task 3
pub(crate) struct Scavenger<'space> { .. }
impl crate::trace::Tracer for Scavenger<'_> { .. }
pub(in crate::heap) fn tenuring_threshold(ages: &[usize; MAX_AGE as usize + 1], desired_words: usize) -> u8;

// heap.rs, Task 3
impl Heap<'_> {
    pub(crate) fn scavenge(&mut self, tenure_all: bool, step_roots: impl FnOnce(&mut Scavenger<'_>));
}

// heap/world.rs, Task 5
pub(crate) struct StepRoots<'state, 'gc> { .. }
impl crate::trace::Trace for StepRoots<'_, '_> { .. }
impl World {
    pub(crate) fn scavenge(&mut self, tenure_all: bool);
}
```

---

### Task 1: The space is four regions, and every allocation still goes old

The split lands with no change in behaviour: every allocation goes to the old region, which now begins at `NURSERY_END`, and every walk of the space reads the occupied ranges of all three regions that can hold objects (the past survivor space, eden, the old space), all but the last empty for now.

**Files:**
- Modify: `crates/ns-runtime/src/heap/space.rs`
- Modify: `crates/ns-runtime/src/heap/compaction.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (every walk, `allocate_block`, `compact`, `move_everything`, `heap_statistics`, `Debug`, `reset_collection_limits`, the tests that name `FIRST_OBJECT`)

**Interfaces:**
- Produces: everything in `space.rs` above except `flip_survivors`, `empty_nursery` and `allocation_range`; `ForwardingTable::with_words(words, base_word)`, `ForwardingTable::summarize(first_chunk)`; `Shift` per region.

- [ ] **Step 1: Create the branch**

```bash
git switch -c feature/generations master
```

- [ ] **Step 2: Write the failing tests in `space.rs`'s test module**

Replace `a_new_space_is_empty_and_starts_at_the_first_object` and `allocation_bumps_the_top_by_whole_words`, and add the region tests:

```rust
#[test]
fn a_new_space_is_four_empty_regions_with_the_nursery_committed() {
    let space = Space::new(SMALL).expect("reserve");
    let past = space.region(RegionName::PastSurvivor);
    let future = space.future_survivor();
    let eden = space.region(RegionName::Eden);
    let old = space.region(RegionName::Old);
    assert_eq!((past.base, past.end), (FIRST_OBJECT, SURVIVOR_BYTES));
    assert_eq!((future.base, future.end), (SURVIVOR_BYTES, EDEN_START));
    assert_eq!((eden.base, eden.end), (EDEN_START, NURSERY_END));
    assert_eq!((old.base, old.start, old.top), (NURSERY_END, NURSERY_END, NURSERY_END));
    for region in [past, future, eden] {
        assert_eq!((region.start, region.top), (region.base, region.base));
    }
    assert!(space.committed() >= NURSERY_END + MINIMUM_COMMIT);
    assert_eq!(space.occupied_bytes(), 0);
    assert_eq!(space.words().len(), NURSERY_END / 8);
}

#[test]
fn each_region_bumps_its_own_top() {
    let mut space = Space::new(SMALL).expect("reserve");
    assert_eq!(space.allocate_old(2), Some(NURSERY_END));
    assert_eq!(space.allocate_old(5), Some(NURSERY_END + 16));
    assert_eq!(space.allocate_young(3), Some(EDEN_START));
    assert_eq!(space.allocate_survivor(4), Some(SURVIVOR_BYTES));
    assert_eq!(space.region(RegionName::Old).top, NURSERY_END + 56);
    assert_eq!(space.region(RegionName::Eden).top, EDEN_START + 24);
    assert_eq!(space.occupied_bytes(), 56 + 24, "the future survivor space is not occupied yet");
    let [past, eden, old] = space.occupied();
    assert_eq!(past, FIRST_OBJECT..FIRST_OBJECT);
    assert_eq!(eden, EDEN_START..EDEN_START + 24);
    assert_eq!(old, NURSERY_END..NURSERY_END + 56);
}

#[test]
fn eden_and_a_survivor_space_refuse_what_would_cross_their_slack() {
    let mut space = Space::new(SMALL).expect("reserve");
    let eden_words = (EDEN_BYTES - REGION_SLACK) / 8;
    assert_eq!(space.allocate_young(eden_words + 1), None);
    assert_eq!(space.allocate_young(eden_words), Some(EDEN_START));
    assert_eq!(space.allocate_young(1), None, "eden is spent");
    let survivor_words = (SURVIVOR_BYTES - REGION_SLACK) / 8;
    assert_eq!(space.allocate_survivor(survivor_words + 1), None);
    assert!(space.allocate_survivor(survivor_words).is_some());
}

#[test]
fn young_means_below_the_nursery_end() {
    let offset = |bytes: u64| NonZeroU64::new(bytes).expect("non-zero");
    assert!(is_young(offset(FIRST_OBJECT as u64)));
    assert!(is_young(offset(NURSERY_END as u64 - 8)));
    assert!(!is_young(offset(NURSERY_END as u64)));
}
```

Update the other `space.rs` tests to the old region: `one_allocation_larger_than_everything_committed_commits_enough_at_once`, `an_allocation_past_the_reservation_answers_none_and_leaves_the_top` and `the_walk_finds_every_object_an_overflow_word_included` call `allocate_old` and walk from `space.region(RegionName::Old).start`; `releasing_never_cuts_below_the_top_and_gives_back_what_lies_above` calls `set_region_extent(RegionName::Old, NURSERY_END, NURSERY_END + 64)` and asserts `space.committed() >= NURSERY_END + MINIMUM_COMMIT`. `SMALL` stays 16 MiB, which holds the 10 MiB nursery.

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --lib heap::space`
Expected: compile errors (`RegionName`, `allocate_young`, … not found).

- [ ] **Step 4: Write the regions**

In `space.rs`, below `FIRST_OBJECT`:

```rust
/// The two survivor spaces and eden, HotSpot's ratio (`SurvivorRatio=8`;
/// decided with the user on 2026-10-04): the survivor spaces first, then
/// eden, then the old space, as Spur and Strongtalk lay them out
/// (`setHeapBase:memoryLimit:endOfMemory:`,
/// `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:60020`;
/// `prior-art/strongtalk/vm/memory/universe.cpp:64-79`), so that young is
/// one comparison against `NURSERY_END`. The first survivor space starts at
/// `FIRST_OBJECT`, so it is 16 bytes smaller than the second.
pub(in crate::heap) const SURVIVOR_BYTES: usize = 1 << 20;
pub(in crate::heap) const EDEN_BYTES: usize = 8 << 20;
pub(in crate::heap) const EDEN_START: usize = 2 * SURVIVOR_BYTES;
/// Where the old space begins: a multiple of 64 words, so the forwarding
/// table's chunks never straddle the two generations.
pub(in crate::heap) const NURSERY_END: usize = EDEN_START + EDEN_BYTES;

/// What every nursery region keeps free past what it fills: the move
/// stress mode shifts a region's objects by up to `8 * (2 + 30)` bytes
/// within it, and the room for that must exist before the shift is asked.
pub(in crate::heap) const REGION_SLACK: usize = 8 * 33;

/// Whether the object at `offset` is young: one comparison, the one the
/// write barrier makes on every store.
#[inline(always)]
pub(in crate::heap) fn is_young(offset: NonZeroU64) -> bool {
    offset.get() < NURSERY_END as u64
}

/// The regions that hold objects, as a caller names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) enum RegionName {
    /// The survivor space holding the last scavenge's survivors.
    PastSurvivor,
    Eden,
    Old,
}

/// One region of the space, in byte offsets: its first possible object at
/// `base`, its objects from `start` to `top` (`start` is `base` except
/// after the move stress mode shifted them up), and its last byte before
/// `end` -- for the old space, the end of the reservation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) struct Region {
    pub(in crate::heap) base: usize,
    pub(in crate::heap) start: usize,
    pub(in crate::heap) top: usize,
    pub(in crate::heap) end: usize,
}

impl Region {
    fn empty(base: usize, end: usize) -> Region {
        Region { base, start: base, top: base, end }
    }

    /// Bumps the top by `words` and answers where they start; `None`, the
    /// top untouched, past `limit`.
    fn bump(&mut self, words: usize, limit: usize) -> Option<usize> {
        let end = words
            .checked_mul(8)
            .and_then(|bytes| self.top.checked_add(bytes))?;
        if end > limit {
            return None;
        }
        let start = self.top;
        self.top = end;
        Some(start)
    }
}
```

`Space` replaces `start` and `top` with the regions:

```rust
pub(in crate::heap) struct Space {
    reservation: Reservation,
    /// The two survivor spaces; `past` indexes the one holding the last
    /// scavenge's survivors, the other is empty outside a scavenge.
    survivors: [Region; 2],
    past: usize,
    eden: Region,
    old: Region,
    /// The end of the committed range, which starts at the base.
    committed: usize,
}
```

`Space::new` reserves as today, refuses a reservation smaller than `NURSERY_END + MINIMUM_COMMIT` (answering `MemoryError::InvalidRange`, which `Heap::new` already turns into the abort), commits `0..round_up_to_page(NURSERY_END + MINIMUM_COMMIT)`, and builds `survivors: [Region::empty(FIRST_OBJECT, SURVIVOR_BYTES), Region::empty(SURVIVOR_BYTES, EDEN_START)]`, `past: 0`, `eden: Region::empty(EDEN_START, NURSERY_END)`, `old: Region::empty(NURSERY_END, reservation.size())`. Then:

```rust
pub(in crate::heap) fn region(&self, name: RegionName) -> Region {
    match name {
        RegionName::PastSurvivor => self.survivors[self.past],
        RegionName::Eden => self.eden,
        RegionName::Old => self.old,
    }
}

/// The survivor space a scavenge copies into; empty outside one.
pub(in crate::heap) fn future_survivor(&self) -> Region {
    self.survivors[1 - self.past]
}

/// The byte ranges that hold objects, in address order: every walk of
/// the space reads these, so none can miss a region.
pub(in crate::heap) fn occupied(&self) -> [Range<usize>; 3] {
    let past = self.survivors[self.past];
    [past.start..past.top, self.eden.start..self.eden.top, self.old.start..self.old.top]
}

pub(in crate::heap) fn occupied_bytes(&self) -> usize {
    self.occupied().iter().map(ExactSizeIterator::len).sum()
}

#[inline]
pub(in crate::heap) fn allocate_young(&mut self, words: usize) -> Option<usize> {
    let limit = self.eden.end - REGION_SLACK;
    self.eden.bump(words, limit)
}

pub(in crate::heap) fn allocate_survivor(&mut self, words: usize) -> Option<usize> {
    let future = 1 - self.past;
    let limit = self.survivors[future].end - REGION_SLACK;
    self.survivors[future].bump(words, limit)
}

/// The old space's bump: grows the commit as `allocate` did.
#[inline]
pub(in crate::heap) fn allocate_old(&mut self, words: usize) -> Option<usize> {
    let end = words
        .checked_mul(8)
        .and_then(|bytes| self.old.top.checked_add(bytes))?;
    if end > self.committed {
        self.ensure_committed(end)?;
    }
    self.old.bump(words, self.old.end)
}

/// Where a region's objects lie after a compaction or a move. Both must
/// lie inside the region, and for the old space be committed.
pub(in crate::heap) fn set_region_extent(&mut self, name: RegionName, start: usize, top: usize) {
    let region = match name {
        RegionName::PastSurvivor => &mut self.survivors[self.past],
        RegionName::Eden => &mut self.eden,
        RegionName::Old => &mut self.old,
    };
    debug_assert!(region.base <= start && start <= top && top <= region.end);
    debug_assert!(name != RegionName::Old || top <= self.committed);
    region.start = start;
    region.top = top;
}
```

`words`/`words_mut` lend `prefix(self.old.top)` (the old top is never below `NURSERY_END`, so the slice covers the nursery). `release_above(keep)` keeps at least `NURSERY_END + MINIMUM_COMMIT` and the old top: `round_up_to_page(keep.max(self.old.top).max(NURSERY_END + MINIMUM_COMMIT))`. `start()`, `top()`, `allocate` and `set_extent` go; the compiler names every caller.

- [ ] **Step 5: Run the space tests**

Run: `cargo nextest run -p ns-runtime --lib heap::space`
Expected: PASS.

- [ ] **Step 6: Write the failing compaction and move tests**

In `compaction.rs`'s test module, the table takes a base word and summarizes the old segment first:

```rust
#[test]
fn the_old_segment_keeps_the_bottom_and_the_nursery_follows_it() {
    // Base 128 words, the old segment from chunk 2 on; one young object
    // at words 10..13 and two old ones at 130..132 and 200..204.
    let mut table = ForwardingTable::with_words(256, 128);
    table.set_live(10..13);
    table.set_live(130..132);
    table.set_live(200..204);
    assert_eq!(table.summarize(2), 3 + 2 + 4);
    assert_eq!(table.destination(130), 128);
    assert_eq!(table.destination(200), 128 + 2);
    assert_eq!(table.destination(10), 128 + 6, "after every old survivor");
    assert_eq!(table.destination(12), 128 + 8);
}
```

Rewrite the three existing tests to pass `FIRST_OBJECT / 8` as the base and `0` as the first chunk, so they keep their meaning.

In `heap.rs`'s tests, `objects_lie_end_to_end_from_the_first_offset` becomes `objects_lie_end_to_end_from_the_old_space_s_first_offset`, asserting `key(class) == space::NURSERY_END`; `an_object_word_is_a_byte_offset_past_the_first_sixteen_bytes` keeps asserting the offset is past 16. Every other heap test runs unchanged: it must, since behaviour does not change.

- [ ] **Step 7: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --lib`
Expected: compile errors in `compaction.rs` and `heap.rs`.

- [ ] **Step 8: Make the table and the shift region-aware**

`ForwardingTable` gains `base: usize` (a word index), set by `with_words(words, base_word)`; `destination` answers `self.base + (chunk.before + below.count_ones())`. `summarize(first_chunk)`:

```rust
/// Fills in each chunk's prefix: the chunks from `first_chunk` on -- the
/// old space -- first, then the ones below it -- the nursery -- so that
/// the old space's survivors keep its bottom and the nursery's follow
/// them, as Strongtalk's full collection compacts the new generation into
/// the old one after the old one's own survivors
/// (`prior-art/strongtalk/vm/memory/markSweep.cpp:255`). Answers the live
/// words in all.
pub(in crate::heap) fn summarize(&mut self, first_chunk: usize) -> usize {
    let first_chunk = first_chunk.min(self.chunks.len());
    let (nursery, old) = self.chunks.split_at_mut(first_chunk);
    let mut total = 0u64;
    for chunk in old.iter_mut().chain(nursery.iter_mut()) {
        chunk.before = total;
        total += u64::from(chunk.live.count_ones());
    }
    total as usize
}
```

`Shift` becomes per region, every region shifted by its own distance within itself:

```rust
/// The move stress mode's relocation: each region's objects shift within
/// that region by their own distance, so every offset changes, young stays
/// young, and a handle the tracer missed both carries an old epoch and
/// names a different place.
#[cfg(debug_assertions)]
pub(crate) struct Shift {
    pub(in crate::heap) regions: [ShiftedRegion; 3],
}

#[cfg(debug_assertions)]
#[derive(Clone)]
pub(crate) struct ShiftedRegion {
    pub(in crate::heap) source: Range<u64>,
    pub(in crate::heap) destination: u64,
}

#[cfg(debug_assertions)]
impl Relocation for Shift {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        for region in &self.regions {
            if region.source.contains(&offset.get()) {
                if let Some(moved) =
                    NonZeroU64::new(offset.get() - region.source.start + region.destination)
                {
                    return moved;
                }
            }
        }
        unreachable!("internal: a moved handle names an object in no occupied region")
    }
}
```

- [ ] **Step 9: Convert every walk and the collector in `heap.rs`**

Each loop `let mut position = self.space.start() / 8; while position < top { … }` becomes a loop over the occupied ranges, the body unchanged:

```rust
for range in self.space.occupied() {
    let mut position = range.start / 8;
    while position < range.end / 8 {
        let (header, next) = object_at(words, position);
        // … the existing body …
        position = next;
    }
}
```

The walks: `class_set_application`, `compact` (both of its walks), `forward_everything`, `replace_resolved_futures` (both passes), `begin_marking`'s debug check, `clear_marks_for_test`. `replace_resolved_futures`' first pass reads `self.space.words()` inside the loop to keep borrowing through `self.load`; take `let ranges = self.space.occupied();` before it.

`allocate_block` calls `self.space.allocate_old(allocation)` and compares `self.space.occupied_bytes()` -- not yet the old space alone, since everything is old -- against `next_collection`. `heap_statistics` and `Debug` report `self.space.occupied_bytes()`. `reset_collection_limits` uses `self.space.occupied_bytes()`.

`compact` builds `ForwardingTable::with_words(old.top / 8, NURSERY_END / 8)`, sets live bits across all occupied ranges (only the old one holds objects in this task), summarizes from `NURSERY_END / 8 / 64`, slides the old region exactly as today, sets `set_region_extent(RegionName::Old, NURSERY_END, NURSERY_END + 8 * live_words)`, and releases above `NURSERY_END + limit` when more than `NURSERY_END + 2 * limit` is committed. The nursery's survivors arrive in Task 4.

`move_everything` shifts each region within itself:

```rust
let mut regions = Vec::with_capacity(3);
for name in [RegionName::PastSurvivor, RegionName::Eden, RegionName::Old] {
    let region = self.space.region(name);
    let length = region.top - region.start;
    // Varied by the epoch, so that moves do not all shift alike.
    let destination = if region.start == region.base {
        region.base + 8 * (2 + to_epoch as usize % 31)
    } else {
        region.base
    };
    if name == RegionName::Old && self.space.ensure_committed(destination + length).is_none() {
        out_of_memory(destination + length)
    }
    // Both ranges lie below the region's top while the objects are copied.
    self.space
        .set_region_extent(name, region.start, region.start.max(destination) + length);
    self.space
        .words_mut()
        .copy_within(region.start / 8..(region.start + length) / 8, destination / 8);
    self.space.set_region_extent(name, destination, destination + length);
    regions.push(ShiftedRegion {
        source: region.start as u64..region.top as u64,
        destination: destination as u64,
    });
}
```

and builds `Shift { regions: [regions[0].clone(), regions[1].clone(), regions[2].clone()] }` (or `try_into` from the `Vec`, `unreachable!` on a wrong length). A nursery region's `top + 256 <= end` holds by `REGION_SLACK`.

- [ ] **Step 10: Run the whole suite, debug and release**

Run: `cargo nextest run --workspace` and `cargo nextest run --workspace --release`
Expected: PASS, every golden unchanged.

- [ ] **Step 11: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/heap/space.rs crates/ns-runtime/src/heap/compaction.rs crates/ns-runtime/src/heap.rs
git commit -m "refactor: the object space is two survivor spaces, eden and the old space"
```

---

### Task 2: The write barrier, the remembered set and where an allocation lands

Behind a switch that only the heap's own tests turn on, an allocation goes to eden; everything else here is live in every build: the header's age and remembered bits, the barrier in every store accessor, the remembered set, large objects old and born remembered, and the decoder's old allocation.

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs`
- Modify: `crates/ns-runtime/src/heap.rs`

**Interfaces:**
- Consumes: Task 1's `Space`, `is_young`, `NURSERY_END`.
- Produces: `Header::{age, with_age, remembered, with_remembered, with_format}`, `MAX_AGE`; `Heap::{write_barrier, remember, set_allocating_old}`; the fields `remembered: Vec<NonZeroU64>`, `young_allocation: bool` (removed in Task 5), `allocating_old: bool`, `scavenge_requested: bool`, `tenure_all_requested: bool`, `scavenge_threshold: usize`, `young_objects: usize`; `fn request_scavenge(&mut self, tenure_all: bool)`; `#[cfg(test)] fn enable_young_allocation_for_test(&mut self)`.

- [ ] **Step 1: Write the failing header tests**

In `header.rs`'s tests, `each_field_reads_back_what_was_written_and_disturbs_no_other` drops its "age and the remembered bit stay 0" assertion and gains:

```rust
#[test]
fn the_age_and_the_remembered_bit_are_bits_twenty_seven_to_thirty_one() {
    let header = Header::new(DispatchId::NONE, Format::FixedPointers, 0, 3).with_identity_hash(9);
    assert_eq!((header.age(), header.remembered()), (0, false));
    let aged = header.with_age(MAX_AGE).with_remembered(true);
    assert_eq!(aged.0, header.0 | 0b1111 << 27 | 1 << 31);
    assert_eq!((aged.age(), aged.remembered()), (MAX_AGE, true));
    assert_eq!(aged.with_age(0).with_remembered(false), header);
}

#[test]
fn a_forwarder_keeps_every_field_but_its_format() {
    let header = Header::new(DispatchId::from_counter(7), Format::Closure, 0, 300)
        .with_identity_hash(42)
        .with_age(3);
    let forwarded = header.with_format(Format::Forwarded);
    assert_eq!(forwarded.format(), Format::Forwarded);
    assert_eq!(forwarded.word_count(), 255);
    assert_eq!(forwarded.identity_hash(), 42);
    assert_eq!(forwarded.age(), 3);
}
```

- [ ] **Step 2: Write the failing barrier tests in `heap.rs`'s test module**

```rust
/// A bare heap whose small objects go to eden, as every heap's will from
/// Task 5 on.
fn young_heap<'gc>() -> Heap<'gc> {
    let mut heap = Heap::new();
    heap.enable_young_allocation_for_test();
    heap
}

/// An old instance of `class` with `field_count` nil fields: large objects
/// go old, so one past the threshold is old by construction.
fn old_instance<'gc>(heap: &mut Heap<'gc>, class: ClassReference<'gc>) -> ObjectReference<'gc> {
    heap.allocate_instance(class, LARGE_OBJECT_BYTES / 8)
}

#[test]
fn a_small_object_is_young_and_a_large_one_old_and_born_remembered() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    let small = heap.allocate_instance(class, 2);
    let large = old_instance(&mut heap, class);
    assert!(space::is_young(small.offset));
    assert!(!space::is_young(large.offset));
    assert!(heap.header(large).remembered(), "it may be filled with young values");
    assert_eq!(heap.remembered, vec![large.offset]);
}

#[test]
fn every_store_of_a_young_value_into_an_old_object_remembers_it_once() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    let young = heap.allocate_instance(class, 0);
    let holder = old_instance(&mut heap, class);
    heap.remembered.clear();
    heap.set_header(holder, heap.header(holder).with_remembered(false));

    assert!(heap.instance_set_field(holder, 0, Value::small_integer(1)));
    assert!(heap.remembered.is_empty(), "an immediate is never young");
    assert!(heap.instance_set_field(holder, 1, Value::object(young)));
    assert!(heap.instance_set_field(holder, 2, Value::object(young)));
    assert_eq!(heap.remembered, vec![holder.offset], "remembered once");
    assert!(heap.header(holder).remembered());
}

#[test]
fn a_store_into_a_young_object_or_of_an_old_value_remembers_nothing() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    let young = heap.allocate_instance(class, 1);
    let old = old_instance(&mut heap, class);
    heap.remembered.clear();
    heap.set_header(old, heap.header(old).with_remembered(false));
    assert!(heap.instance_set_field(young, 0, Value::object(old)));
    assert!(heap.instance_set_field(old, 0, Value::object(old)));
    assert!(heap.remembered.is_empty());
}
```

The remaining store paths, each holder old and taken out of the set first, since a large object is born remembered:

```rust
/// Takes `holder` out of the remembered set, as if nothing young had ever
/// been stored into it.
fn forget<'gc>(heap: &mut Heap<'gc>, holder: ObjectReference<'gc>) {
    heap.remembered.retain(|offset| *offset != holder.offset);
    heap.set_header(holder, heap.header(holder).with_remembered(false));
}

#[test]
fn an_array_element_a_class_word_and_a_class_field_each_remember_their_old_holder() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    let young = heap.allocate_instance(class, 0);

    let array = heap.allocate_array(class, &vec![Value::nil(); LARGE_OBJECT_BYTES / 8]);
    forget(&mut heap, array);
    assert!(heap.array_set_element(array, 0, Value::object(young)));
    assert!(heap.header(array).remembered());

    let young_class = named_class(&mut heap, "Young");
    assert!(space::is_young(young_class.offset));
    let instance = old_instance(&mut heap, class);
    forget(&mut heap, instance);
    assert!(heap.object_set_class(instance, young_class));
    assert!(heap.header(instance).remembered());

    heap.set_allocating_old(true);
    let old_class = named_class(&mut heap, "Old");
    heap.set_allocating_old(false);
    assert!(!heap.header(old_class).remembered());
    assert!(heap.class_set_superclass(old_class, young_class));
    assert!(heap.header(old_class).remembered());
}

#[test]
fn a_dead_old_frame_s_register_and_a_divorce_into_an_old_frame_each_remember_it() {
    let mut heap = frame_heap();
    heap.enable_young_allocation_for_test();
    let class = heap.allocate_self_referential(0);
    let young = heap.allocate_instance(class, 0);

    // A frame is old when its register file makes it large.
    let dead = top_level_frame(&mut heap, LARGE_OBJECT_BYTES / 8);
    let dead_object = heap.frame_object(dead);
    assert!(!space::is_young(dead_object.offset));
    forget(&mut heap, dead_object);
    heap.frame_set_local(dead, 0, Value::object(young));
    assert!(heap.header(dead_object).remembered());

    let activation = heap.push_activation(header(), &[], LARGE_OBJECT_BYTES / 8);
    let frame_object = heap.frame_object(heap.reify(activation));
    assert!(!space::is_young(frame_object.offset));
    forget(&mut heap, frame_object);
    heap.retain_registers(activation);
    heap.activation_set_local(activation, 0, Value::object(young));
    assert!(
        !heap.header(frame_object).remembered(),
        "a married frame's registers are the stack's, a root"
    );
    heap.pop_activation(activation);
    assert!(heap.header(frame_object).remembered(), "the divorce copied a young register in");
}
```

(`frame_heap` records the `Activation` class, `top_level_frame` and `header()` are the module's existing helpers; `named_class` builds a self-referential class object with a registered application, young unless the heap is allocating old.)

Then the switches:

```rust
#[test]
fn the_decoder_s_allocations_go_old_and_are_not_remembered() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    heap.set_allocating_old(true);
    let decoded = heap.allocate_placeholder(class, 2);
    heap.set_allocating_old(false);
    assert!(!space::is_young(decoded.offset));
    assert!(!heap.header(decoded).remembered(), "the load tenured everything it names");
    assert!(space::is_young(heap.allocate_instance(class, 0).offset));
}

#[test]
fn an_allocation_that_finds_eden_spent_goes_old_born_remembered_and_asks_for_a_scavenge() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    // One young object just under the large-object threshold at a time:
    // its header, its overflow word, its class word and `fill` fields.
    let fill = LARGE_OBJECT_BYTES / 8 - 4;
    let allocation = 8 * space::allocation_words(fill + 1);
    assert!(allocation < LARGE_OBJECT_BYTES);
    while heap.space.region(RegionName::Eden).top + allocation
        <= space::NURSERY_END - space::REGION_SLACK
    {
        assert!(space::is_young(heap.allocate_instance(class, fill).offset));
    }
    assert!(heap.scavenge_requested, "eden passed its reserve long before it was spent");
    let spill = heap.allocate_instance(class, fill);
    assert!(!space::is_young(spill.offset));
    assert!(heap.header(spill).remembered());
}

#[test]
fn a_remembered_set_past_its_limit_asks_for_a_scavenge_that_tenures_everything() {
    let mut heap = young_heap();
    let class = heap.allocate_self_referential(0);
    let young = heap.allocate_instance(class, 0);
    // Small old objects, as the decoder makes them, so none is born
    // remembered: each joins the set through the barrier alone.
    heap.set_allocating_old(true);
    let holders: Vec<_> = (0..=REMEMBERED_SET_LIMIT)
        .map(|_| heap.allocate_instance(class, 1))
        .collect();
    heap.set_allocating_old(false);
    assert!(!heap.scavenge_requested);
    for holder in holders {
        heap.instance_set_field(holder, 0, Value::object(young));
    }
    assert_eq!(heap.remembered.len(), REMEMBERED_SET_LIMIT + 1);
    assert!(heap.scavenge_requested && heap.tenure_all_requested);
}
```

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: compile errors (`age`, `remembered`, `enable_young_allocation_for_test`, …).

- [ ] **Step 4: The header's bits**

In `header.rs`:

```rust
const AGE_SHIFT: u32 = 27;
const AGE_MASK: u64 = 0b1111;
const REMEMBERED_BIT: u64 = 1 << 31;
/// The widest age the header's four bits hold: an object this old is
/// tenured at its next scavenge whatever the threshold says.
pub(super) const MAX_AGE: u8 = 15;
```

```rust
/// How many scavenges this object has survived in the nursery, at most
/// `MAX_AGE`; 0 for a new object and meaningless for an old one.
pub(super) fn age(self) -> u8 {
    ((self.0 >> AGE_SHIFT) & AGE_MASK) as u8
}

pub(super) fn with_age(self, age: u8) -> Header {
    debug_assert!(age <= MAX_AGE);
    Header((self.0 & !(AGE_MASK << AGE_SHIFT)) | ((u64::from(age) & AGE_MASK) << AGE_SHIFT))
}

/// Whether this old object is in the remembered set; never set on a
/// young one.
pub(super) fn remembered(self) -> bool {
    self.0 & REMEMBERED_BIT != 0
}

pub(super) fn with_remembered(self, remembered: bool) -> Header {
    if remembered {
        Header(self.0 | REMEMBERED_BIT)
    } else {
        Header(self.0 & !REMEMBERED_BIT)
    }
}

/// The header with `format`'s code and no unused units: what a scavenge
/// leaves at a copied object's old place, the forwarded format, keeping
/// the word count so the region stays walkable.
pub(super) fn with_format(self, format: Format) -> Header {
    debug_assert_eq!(format.unused_limit(), 0);
    Header((self.0 & !(FORMAT_MASK << FORMAT_SHIFT)) | ((format as u64) << FORMAT_SHIFT))
}
```

Update the module doc: the age and the remembered bit belong to the generational collector and are written by it; `Debug` prints both.

- [ ] **Step 5: The heap's fields and the allocation rules**

In `heap.rs`, beside `COLLECTION_FLOOR`:

```rust
/// From this size on an object is allocated old: copying it through the
/// survivor spaces would buy nothing (Primordial Soup's
/// `kLargeAllocationSize`, `prior-art/primordialsoup/vm/heap.h:111`).
const LARGE_OBJECT_BYTES: usize = 32 << 10;

/// Past this many entries the remembered set asks for a scavenge that
/// tenures everything, which empties it (Soup's
/// `kRememberedSetOverflowSize`, `heap.h:112`, `heap.cc:279-283`).
const REMEMBERED_SET_LIMIT: usize = 8 << 10;

/// What eden keeps free past the point where it asks for a scavenge: what
/// a stretch of the run loop allocates before its safepoint serves the
/// request. An allocation past it goes old instead
/// (`docs/design/memory.md`, "Planned: inline allocation", which road step
/// 6 builds on).
const EDEN_RESERVE: usize = 512 << 10;
```

New `Heap` fields, each with its doc comment: `remembered: Vec<NonZeroU64>` ("every old object that may name a young one, each once, its header's remembered bit set; the roots a scavenge adds to the activation stack's and the host's"); `young_allocation: bool` ("whether a small allocation goes to eden; off outside the heap's own tests until the run loop scavenges (Task 5 of the generations plan removes it)"); `allocating_old: bool`; `scavenge_requested: bool`; `tenure_all_requested: bool`; `scavenge_threshold: usize` (initially `EDEN_BYTES - EDEN_RESERVE`, eden's occupied bytes at which an allocation asks for a scavenge); `young_objects: usize` (beside `object_count`, which keeps counting every object).

`allocate_block`, whole:

```rust
fn allocate_block(
    &mut self,
    class: Word,
    guard: DispatchId,
    format: Format,
    unused: u8,
    negative: bool,
    field_count: usize,
) -> (ObjectReference<'gc>, Range<usize>) {
    let word_count = field_count.saturating_add(1);
    let allocation = allocation_words(word_count);
    let bytes = allocation.saturating_mul(8);
    let young = self.young_allocation && !self.allocating_old && bytes < LARGE_OBJECT_BYTES;
    let start = match young.then(|| self.space.allocate_young(allocation)).flatten() {
        Some(start) => {
            self.young_objects += 1;
            let eden = self.space.region(RegionName::Eden);
            if eden.top - eden.start >= self.scavenge_threshold {
                self.request_scavenge(false);
            }
            start
        }
        None => {
            // Eden spent: the scavenge is asked for and the object goes old
            // until it runs, as Strongtalk's `allocate_tenured` does when a
            // scavenge cannot run
            // (`prior-art/strongtalk/vm/memory/universe_more.cpp:55-64`).
            if young {
                self.request_scavenge(false);
            }
            let Some(start) = self.space.allocate_old(allocation) else {
                out_of_memory(bytes)
            };
            if self.old_occupied() >= self.next_collection {
                self.request_collection();
            }
            start
        }
    };
    let words = self.space.words_mut();
    let mut header = start / 8;
    if word_count >= WORD_COUNT_OVERFLOW {
        words[header] = overflow_word(word_count);
        header += 1;
    }
    words[header + HEADER] = Header::new(guard, format, unused, word_count)
        .with_negative(negative)
        .0;
    words[header + CLASS] = class.bits();
    let fields = header + FIELDS..header + FIELDS + field_count;
    words[fields.clone()].fill(Word::NIL.bits());
    self.object_count += 1;
    let Some(offset) = NonZeroU64::new(8 * header as u64) else {
        unreachable!("internal: an object lies past the first object")
    };
    // Its initializing stores took no barrier, so an old object that may be
    // filled with young values joins the remembered set at birth; the
    // decoder's never does, since the load tenured everything it names.
    if self.young_allocation && !space::is_young(offset) && !self.allocating_old {
        self.remember(offset);
    }
    (self.object_handle(offset), fields)
}
```

with `fn old_occupied(&self) -> usize { let old = self.space.region(RegionName::Old); old.top - old.start }`. `reset_collection_limits` uses `old_occupied()` from this task on, since the old space alone is what a full collection's limit measures.

```rust
/// Only `allocate_block` and the remembered set's overflow ask, and only
/// flags are set: the run loop's next safepoint scavenges.
fn request_scavenge(&mut self, tenure_all: bool) {
    self.scavenge_requested = true;
    self.tenure_all_requested |= tenure_all;
    self.safepoint_requested = true;
}

/// The image decoder's graph pass allocates old: everything it names was
/// tenured before it began (`image::load`).
pub(crate) fn set_allocating_old(&mut self, allocating_old: bool) {
    self.allocating_old = allocating_old;
}

#[cfg(test)]
fn enable_young_allocation_for_test(&mut self) {
    self.young_allocation = true;
}
```

- [ ] **Step 6: The barrier and its call sites**

```rust
/// The write barrier: `holder` now stores `word`. An old object that
/// comes to name a young one joins the remembered set, so the next
/// scavenge finds the reference without walking the old space -- per
/// object, as Spur's `storePointer:ofObject:withValue:` and Soup's
/// `HeapObject::Store` remember, rather than Strongtalk's cards. Every
/// store into an existing object comes through here, so the interpreter,
/// the reference lowering and a JIT share one rule
/// (`docs/design/memory.md`, "The write barrier").
#[inline(always)]
fn write_barrier(&mut self, holder: NonZeroU64, word: Word) {
    if !space::is_young(holder) && word.object_offset().is_some_and(space::is_young) {
        self.remember(holder);
    }
}

/// Adds `holder`, an old object, to the remembered set unless its bit says
/// it is in it already.
fn remember(&mut self, holder: NonZeroU64) {
    let index = header_index(holder);
    let words = self.space.words_mut();
    let header = Header(words[index]);
    if header.remembered() {
        return;
    }
    words[index] = header.with_remembered(true).0;
    self.remembered.push(holder);
    if self.remembered.len() > REMEMBERED_SET_LIMIT {
        self.request_scavenge(true);
    }
}
```

Call sites, each after its write:
- `set_pointer_word`: on `Some(slot)`, `self.write_barrier(object.offset, word)` after `*slot = word.bits()` (the borrow of `slot` ends first). This covers instance fields, array elements and class fields.
- `object_set_class`: `self.write_barrier(object.offset, class_word)`.
- `frame_set_local`'s dead-frame arm: `self.write_barrier(frame.offset, word)` when the slot existed.
- `pop_activation`'s divorce: when `entry.retains_registers` and `!space::is_young(frame.offset)`, after the copy, `if stack.values[base..].iter().any(|word| word.object_offset().is_some_and(space::is_young)) { self.remember(frame.offset) }` (after the destructuring borrow of `space` and `stack` ends).

The decoder's fills (`fill_placeholder_field`, `fill_placeholder_class`, `fill_placeholder_closure`, `fill_frame_fields`, `rewrite_placeholder_header`) take no barrier and gain a `debug_assert!` that no stored word is young while `self.allocating_old` is set ("the load tenures everything the graph names before the graph pass"). An initializing store in an allocation takes none: the object is born remembered when it is old.

- [ ] **Step 7: Run the heap tests**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: PASS.

- [ ] **Step 8: Run the whole suite**

Run: `cargo nextest run --workspace`
Expected: PASS, every golden unchanged (`young_allocation` is off outside the heap's tests).

- [ ] **Step 9: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/heap/header.rs crates/ns-runtime/src/heap.rs
git commit -m "feat: a write barrier and a remembered set behind the heap's store accessors"
```

---

### Task 3: The scavenger

A Cheney copy of the nursery's survivors into the future survivor space, or into the old space once their age reaches the threshold, the survivor space is full, or the scavenge tenures everything. Built and tested on bare heaps; nothing calls it outside tests until Task 5.

**Files:**
- Create: `crates/ns-runtime/src/heap/scavenge.rs`
- Modify: `crates/ns-runtime/src/heap/space.rs` (`allocation_range`, `flip_survivors`)
- Modify: `crates/ns-runtime/src/heap.rs` (`mod scavenge;`, `Heap::scavenge`, the fields `tenuring_threshold: u8` and `scavenges: usize`, `scavenge_for_test`)

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: `Scavenger`, `tenuring_threshold`, `Space::{allocation_range, flip_survivors}`, `Heap::scavenge(tenure_all, step_roots)`.

- [ ] **Step 1: Write the failing tests**

In `scavenge.rs`'s own test module:

```rust
#[test]
fn the_threshold_is_the_age_past_which_the_survivors_overflow_half_the_space() {
    let mut ages = [0; MAX_AGE as usize + 1];
    ages[1] = 300;
    ages[2] = 300;
    ages[3] = 300;
    assert_eq!(tenuring_threshold(&ages, 500), 2, "ages 1 and 2 already exceed 500 words");
    assert_eq!(tenuring_threshold(&ages, 1_000), MAX_AGE, "everything fits: tenure nothing by age");
    ages[1] = 600;
    assert_eq!(tenuring_threshold(&ages, 500), 1, "never below 1");
}
```

In `heap.rs`'s tests, a helper and the behaviour:

```rust
/// Scavenges a bare heap with `roots` as the step state's roots.
fn scavenge_bare<'gc>(heap: &mut Heap<'gc>, tenure_all: bool, roots: &mut [&mut Value<'gc>]) {
    use crate::trace::Tracer;
    heap.scavenge(tenure_all, |scavenger| {
        for root in roots.iter_mut() {
            scavenger.value(root);
        }
    });
}

#[test]
fn a_scavenge_copies_what_a_root_reaches_and_reclaims_eden() {
    let mut heap = young_heap();
    let class = old_instance_class(&mut heap);   // an old self-referential class, see below
    let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
    heap.allocate_string(class, crate::CodePoints::from("dead"));
    let mut root = Value::object(kept);
    let before = heap.heap_statistics();

    scavenge_bare(&mut heap, false, &mut [&mut root]);

    let Some(moved) = root.as_object() else { panic!("the root no longer holds an object") };
    assert!(space::is_young(moved.offset), "age 1 is below the starting threshold");
    assert_eq!(heap.str_value(moved), Some("kept".to_string()));
    assert_eq!(heap.header(moved).age(), 1);
    let after = heap.heap_statistics();
    assert_eq!(before.young_objects, 2);
    assert_eq!(after.young_objects, 1);
    assert_eq!(heap.space.region(RegionName::Eden).top, heap.space.region(RegionName::Eden).base);
}
```

with the class old, so that the young objects of most tests name an old class (the class word is a pointer word, and a young class is copied with its instances -- a case `a_young_class_is_copied_with_its_instances` covers on purpose):

```rust
/// A class object that is its own class, allocated old.
fn old_instance_class<'gc>(heap: &mut Heap<'gc>) -> ClassReference<'gc> {
    heap.set_allocating_old(true);
    let class = heap.allocate_self_referential(0);
    heap.set_allocating_old(false);
    class
}
```

The Review Focus cases, each its own test:
- `an_object_reachable_only_through_the_remembered_set_survives_until_it_is_tenured`: an old holder (`old_instance`) stores a young string in field 0; no root names the string; scavenge 16 times with `tenure_all = false`; after every scavenge the holder's field reads "kept" back; while the string is young the holder is remembered; once `!is_young`, after the next scavenge the holder is no longer remembered and `heap.remembered` is empty.
- `a_promoted_object_naming_a_survivor_is_remembered_by_the_scavenge_that_promoted_it`: a young array A, rooted, is scavenged once (age 1); a young string S, allocated after that scavenge (age 0), is stored into A's element 0; then `heap.tenuring_threshold = 1` and a second scavenge: A is promoted, S goes to the survivor space, and A is remembered; a third scavenge keeps S intact through A.
- `more_survivors_than_a_survivor_space_holds_are_promoted_and_nothing_is_lost`: 300 rooted young arrays of 512 elements each (4 KiB, 1.2 MiB in all, every one below the large-object threshold), each element 0 the array's index as a `SmallInteger`, held as elements of one old root array; scavenge; every element reads back its index; some arrays are old, some young.
- `a_young_class_is_copied_with_its_instances`: a young class object (a young self-referential `allocate_self_referential`) and a young instance of it rooted only through the instance; after the scavenge `heap.object_class(instance)` is the moved class and `same` as the copy of the root class.
- `a_married_frame_is_copied_and_its_entry_follows_it`: push an activation, reify it, scavenge with nothing else rooted, `frame_local`/`frame_set_local` reach the live registers through the moved frame, `pop_activation` divorces into the moved object.
- `the_divorce_into_a_tenured_frame_keeps_its_young_registers`: push an activation with one register, reify it, scavenge with `tenure_all = true` (the frame is now old), store a fresh young string in its register, `retain_registers`, pop; the frame is remembered; scavenge; `frame_local(frame, 0)` reads the string back.
- `a_symbol_held_only_by_young_garbage_is_pruned_and_a_kept_one_keeps_its_identity`: mirror `a_symbol_answers_its_symbol_across_collections_and_moves_and_a_pruned_one_leaves_no_entry` with `scavenge_bare` in place of `collect_for_test`, the symbols young.
- `a_scavenge_that_tenures_everything_empties_the_nursery_and_the_remembered_set`: young objects rooted and through a remembered old holder; `scavenge_bare(.., true, ..)`; `young_objects == 0`, the past survivor space empty, `heap.remembered` empty, every value reads back.
- `a_handle_kept_across_a_scavenge_is_stale` (`#[cfg(debug_assertions)]`, `#[should_panic(expected = "stale handle")]`): keep a young handle in a Rust local, scavenge without rooting it, read it.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: compile errors (`scavenge` not found).

- [ ] **Step 3: `space.rs`'s pieces**

```rust
/// The words of the allocation whose header is word `header`: from its
/// overflow word, when it has one, to its last field. What a scavenge
/// copies.
pub(in crate::heap) fn allocation_range(words: &[u64], header: usize) -> Range<usize> {
    let block = block_range(words, header);
    let first = if Header(words[header]).word_count() == WORD_COUNT_OVERFLOW {
        header - 1
    } else {
        header
    };
    first..block.end
}
```

```rust
/// After a scavenge: the survivor space just filled holds the survivors,
/// eden and the one they came from are empty.
pub(in crate::heap) fn flip_survivors(&mut self) {
    let past = self.past;
    self.survivors[past] = Region::empty(self.survivors[past].base, self.survivors[past].end);
    self.past = 1 - past;
    self.eden = Region::empty(self.eden.base, self.eden.end);
}
```

- [ ] **Step 4: The scavenger**

`crates/ns-runtime/src/heap/scavenge.rs`:

```rust
//! The scavenger (`docs/design/memory.md`, "Generations"): a Cheney copy
//! of the nursery's survivors, as Spur's `doScavenge:`
//! (`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:48477`) and
//! Strongtalk's `Universe::scavenge`
//! (`prior-art/strongtalk/vm/memory/universe_more.cpp:81-137`) are. A
//! survivor goes to the future survivor space, or to the old space once
//! its age reaches the threshold, the survivor space is full, or the
//! scavenge tenures everything. Its old place becomes a forwarder: the
//! forwarded format, its class word naming the copy.
//!
//! A child of `heap`, so it may name a handle's private offset. It holds
//! the space and nothing else of the heap, so that the roots it traces --
//! the activation stack, the host roots, the step state -- can be borrowed
//! beside it.

use std::num::NonZeroU64;

use crate::heap::header::{Format, Header, MAX_AGE};
use crate::heap::object::{pointer_ranges, CLASS};
use crate::heap::space::{allocation_range, block_range, is_young, Space};
use crate::heap::{header_index, FrameReference, ObjectReference, Word};
use crate::trace::Tracer;

pub(crate) struct Scavenger<'space> {
    pub(in crate::heap) space: &'space mut Space,
    tenure_all: bool,
    threshold: u8,
    /// The words copied into the survivor space, by the age they arrive
    /// with: Strongtalk's age table (`ageTable.cpp`).
    pub(in crate::heap) ages: [usize; MAX_AGE as usize + 1],
    /// Where the Cheney scan stands in the survivor space and in the old
    /// space; everything below is scanned, everything from here to the
    /// region's top was copied and not yet scanned.
    survivor_scan: usize,
    promoted_scan: usize,
    /// Promoted objects that still name a young one: the scavenge's
    /// additions to the remembered set.
    pub(in crate::heap) remembered: Vec<NonZeroU64>,
    /// How many objects were copied: the nursery's new count, and how many
    /// of them went old.
    pub(in crate::heap) survived: usize,
    pub(in crate::heap) promoted: usize,
    #[cfg(debug_assertions)]
    pub(in crate::heap) heap_id: u32,
    #[cfg(debug_assertions)]
    pub(in crate::heap) from_epoch: u32,
    #[cfg(debug_assertions)]
    pub(in crate::heap) to_epoch: u32,
}

impl<'space> Scavenger<'space> {
    pub(in crate::heap) fn new(
        space: &'space mut Space,
        tenure_all: bool,
        threshold: u8,
        #[cfg(debug_assertions)] heap_id: u32,
        #[cfg(debug_assertions)] from_epoch: u32,
        #[cfg(debug_assertions)] to_epoch: u32,
    ) -> Scavenger<'space> {
        let survivor_scan = space.future_survivor().top;
        let promoted_scan = space.region(crate::heap::space::RegionName::Old).top;
        Scavenger {
            space,
            tenure_all,
            threshold,
            ages: [0; MAX_AGE as usize + 1],
            survivor_scan,
            promoted_scan,
            remembered: Vec::new(),
            survived: 0,
            promoted: 0,
            #[cfg(debug_assertions)]
            heap_id,
            #[cfg(debug_assertions)]
            from_epoch,
            #[cfg(debug_assertions)]
            to_epoch,
        }
    }

    /// Where the object `offset` names lives once the scavenge is done:
    /// followed along every forwarder, whatever made each link, and copied
    /// if it is young and not already in the survivor space being filled,
    /// as Spur's `scavengeReferentsOf:` decides (`cointerp.c:43316-43322`).
    pub(in crate::heap) fn evacuate(&mut self, offset: NonZeroU64) -> NonZeroU64 {
        let mut offset = offset;
        loop {
            let words = self.space.words();
            let header = header_index(offset);
            if Header(words[header]).format() != Format::Forwarded {
                break;
            }
            offset = match Word::from_bits(words[header + CLASS]).object_offset() {
                Some(target) => target,
                None => unreachable!("internal: a forwarder names an object"),
            };
        }
        let future = self.space.future_survivor();
        if !is_young(offset) || (future.base..future.end).contains(&(offset.get() as usize)) {
            return offset;
        }
        self.copy(offset)
    }

    fn copy(&mut self, offset: NonZeroU64) -> NonZeroU64 {
        let words = self.space.words();
        let header = header_index(offset);
        let source = allocation_range(words, header);
        let header_within = header - source.start;
        let age = Header(words[header]).age();
        let length = source.len();
        let survivor = if self.tenure_all || age >= self.threshold {
            None
        } else {
            self.space.allocate_survivor(length)
        };
        let (start, promoted) = match survivor {
            Some(start) => (start, false),
            // The old space was committed for the worst case before the
            // scavenge began, so this never fails short of the reservation.
            None => match self.space.allocate_old(length) {
                Some(start) => (start, true),
                None => unreachable!("internal: a scavenge promotes into memory committed before it began"),
            },
        };
        let destination = start / 8;
        let words = self.space.words_mut();
        words.copy_within(source.clone(), destination);
        let moved = destination + header_within;
        let copied = Header(words[moved]);
        words[moved] = if promoted {
            copied.with_age(0)
        } else {
            let new_age = (age + 1).min(MAX_AGE);
            self.ages[usize::from(new_age)] += length;
            copied.with_age(new_age)
        }
        .0;
        let moved_offset = match NonZeroU64::new(8 * moved as u64) {
            Some(moved_offset) => moved_offset,
            None => unreachable!("internal: a copy lies past the first object"),
        };
        words[header] = Header(words[header]).with_format(Format::Forwarded).0;
        words[header + CLASS] = Word::with_object_offset(moved_offset).bits();
        self.survived += 1;
        self.promoted += usize::from(promoted);
        moved_offset
    }

    /// Rewrites every pointer word of the object at word `header`; answers
    /// whether it still names a young object afterwards.
    pub(in crate::heap) fn scan_object(&mut self, header: usize) -> bool {
        let block = block_range(self.space.words(), header);
        let ranges = pointer_ranges(&self.space.words()[block.clone()]);
        let mut names_young = false;
        for range in ranges {
            for position in range.start + block.start..range.end + block.start {
                let word = Word::from_bits(self.space.words()[position]);
                if let Some(target) = word.object_offset().filter(|target| is_young(*target)) {
                    let moved = self.evacuate(target);
                    self.space.words_mut()[position] = Word::with_object_offset(moved).bits();
                    names_young |= is_young(moved);
                }
            }
        }
        names_young
    }

    /// The Cheney loop: scans what was copied until nothing new was.
    pub(in crate::heap) fn drain(&mut self) {
        loop {
            let mut progressed = false;
            while self.survivor_scan < self.space.future_survivor().top {
                let (header, next) = crate::heap::space::object_at(self.space.words(), self.survivor_scan / 8);
                self.scan_object(header);
                self.survivor_scan = 8 * next;
                progressed = true;
            }
            while self.promoted_scan < self.space.region(crate::heap::space::RegionName::Old).top {
                let (header, next) = crate::heap::space::object_at(self.space.words(), self.promoted_scan / 8);
                if self.scan_object(header) {
                    let offset = match NonZeroU64::new(8 * header as u64) {
                        Some(offset) => offset,
                        None => unreachable!("internal: a promoted object lies past the first object"),
                    };
                    let words = self.space.words_mut();
                    words[header] = Header(words[header]).with_remembered(true).0;
                    self.remembered.push(offset);
                }
                self.promoted_scan = 8 * next;
                progressed = true;
            }
            if !progressed {
                return;
            }
        }
    }

    #[cfg(debug_assertions)]
    fn check_and_rebrand(&self, origin: u32, epoch: u32) -> u32 {
        assert_eq!(origin, self.heap_id, "a handle from another heap is stored in this one");
        assert_eq!(
            epoch, self.from_epoch,
            "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
            epoch, self.to_epoch
        );
        self.to_epoch
    }
}

impl Tracer for Scavenger<'_> {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        #[cfg(debug_assertions)]
        {
            handle.epoch = self.check_and_rebrand(handle.origin, handle.epoch);
        }
        if is_young(handle.offset) {
            handle.offset = self.evacuate(handle.offset);
        }
    }

    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        #[cfg(debug_assertions)]
        {
            handle.epoch = self.check_and_rebrand(handle.origin, handle.epoch);
        }
        if is_young(handle.offset) {
            handle.offset = self.evacuate(handle.offset);
        }
    }

    fn word(&mut self, word: &mut Word) {
        if let Some(target) = word.object_offset().filter(|target| is_young(*target)) {
            *word = Word::with_object_offset(self.evacuate(target));
        }
    }
}

/// The age at which the next scavenge tenures: the lowest at which the
/// survivors of that age and younger exceed `desired_words`, never below 1
/// and never above `MAX_AGE` -- Strongtalk's `ageTable::tenuring_threshold`
/// (`prior-art/strongtalk/vm/memory/ageTable.cpp:37-46`), which HotSpot
/// took over with `TargetSurvivorRatio=50` and `MaxTenuringThreshold=15`.
pub(in crate::heap) fn tenuring_threshold(ages: &[usize; MAX_AGE as usize + 1], desired_words: usize) -> u8 {
    let mut total = 0;
    for age in 1..=MAX_AGE {
        total += ages[usize::from(age)];
        if total > desired_words {
            return age.saturating_sub(1).max(1);
        }
    }
    MAX_AGE
}
```

(`object_at`, `block_range`, `header_index`, `CLASS` and `pointer_ranges` widen to `pub(in crate::heap)` where they are narrower; `ObjectReference`'s and `FrameReference`'s `origin`/`epoch`/`offset` fields are already visible to the heap's submodules, as `mark.rs` shows.)

- [ ] **Step 5: `Heap::scavenge`**

```rust
/// One scavenge (`scavenge.rs`): every young object the roots, the
/// remembered set and the copies themselves reach is copied, the rest of
/// the nursery is reclaimed, and every handle the heap holds is rewritten
/// into the next epoch. `step_roots` traces what the step state holds
/// beside the heap (`World::scavenge`). `tenure_all` sends every survivor
/// old, which empties the nursery and the remembered set.
///
/// Sound only where nothing outside the heap and the interpreter holds a
/// handle: at a safepoint of the run loop, or between entries.
pub(crate) fn scavenge(&mut self, tenure_all: bool, step_roots: impl FnOnce(&mut Scavenger<'_>)) {
    use crate::trace::{Trace, Tracer};
    // Committed for the worst case first, so that a promotion never meets a
    // refused commit halfway through (`docs/design/memory.md`).
    let [past, eden, _] = self.space.occupied();
    let worst = self.space.region(RegionName::Old).top + past.len() + eden.len();
    if self.space.ensure_committed(worst).is_none() {
        out_of_memory(worst)
    }
    #[cfg(debug_assertions)]
    let (heap_id, from_epoch, to_epoch) = (self.heap_id, self.epoch, self.epoch.wrapping_add(1));
    #[cfg(debug_assertions)]
    {
        self.epoch = to_epoch;
    }
    let threshold = self.tenuring_threshold;
    let old_remembered = std::mem::take(&mut self.remembered);
    let Heap { space, stack, activation_class, host_roots, canonical_symbols, symbol_of, .. } = self;
    let mut scavenger = Scavenger::new(
        space,
        tenure_all,
        threshold,
        #[cfg(debug_assertions)]
        heap_id,
        #[cfg(debug_assertions)]
        from_epoch,
        #[cfg(debug_assertions)]
        to_epoch,
    );
    if let Some(class) = activation_class {
        scavenger.object(class);
    }
    for entry in &mut stack.entries {
        entry.trace(&mut scavenger);
    }
    for word in &mut stack.values {
        scavenger.word(word);
    }
    let roots = host_roots.get_mut();
    roots.sweep();
    for slot in &roots.slots {
        let mut value = slot.get();
        scavenger.value(&mut value);
        slot.set(value);
    }
    step_roots(&mut scavenger);
    // The old remembered set: each entry leaves it, and joins the new one
    // again only if it still names a young object.
    for holder in old_remembered {
        let header = header_index(holder);
        let words = scavenger.space.words_mut();
        words[header] = Header(words[header]).with_remembered(false).0;
        if scavenger.scan_object(header) {
            let words = scavenger.space.words_mut();
            words[header] = Header(words[header]).with_remembered(true).0;
            scavenger.remembered.push(holder);
        }
    }
    scavenger.drain();
    // The weak symbol table: a young entry the scavenge copied names the
    // copy, one it did not is gone, an old one stays (`docs/design/memory.md`,
    // "Weak references").
    for entry in canonical_symbols.iter_mut() {
        let Some(object) = entry else { continue };
        if !space::is_young(object.offset) {
            #[cfg(debug_assertions)]
            {
                object.epoch = to_epoch;
            }
            continue;
        }
        let words = scavenger.space.words();
        let header = header_index(object.offset);
        let old_key = object.offset.get();
        if Header(words[header]).format() == Format::Forwarded {
            let Some(moved) = Word::from_bits(words[header + CLASS]).object_offset() else {
                unreachable!("internal: a forwarder names an object")
            };
            if let Some(symbol) = symbol_of.remove(&old_key) {
                symbol_of.insert(moved.get(), symbol);
            }
            object.offset = moved;
            #[cfg(debug_assertions)]
            {
                object.epoch = to_epoch;
            }
        } else {
            symbol_of.remove(&old_key);
            *entry = None;
        }
    }
    let remembered = std::mem::take(&mut scavenger.remembered);
    let (survived, promoted, ages) = (scavenger.survived, scavenger.promoted, scavenger.ages);
    drop(scavenger);
    // Task 6 poisons `past` and `eden`, the ranges vacated, here.
    self.remembered = remembered;
    self.object_count = self.object_count - self.young_objects + survived;
    self.young_objects = survived - promoted;
    self.space.flip_survivors();
    let survivor = self.space.future_survivor();
    let desired = (survivor.end - survivor.base - REGION_SLACK) / 8 / 2;
    self.tenuring_threshold = scavenge::tenuring_threshold(&ages, desired);
    self.scavenge_requested = false;
    self.tenure_all_requested = false;
    self.scavenges += 1;
    if self.old_occupied() >= self.next_collection {
        self.request_collection();
    }
}
```

The weak-symbol pass reads `symbol_of` as an `IdentityMap<u64, Symbol>`; the borrow of `scavenger.space` for the forwarder lookup ends before `symbol_of` is written. The `canonical_symbols` loop is one place for it: write it as a `fn forward_weak_symbols(...)` beside `prune_canonical_symbols` if the closure-free form reads more plainly. `tenuring_threshold` starts at `MAX_AGE` in `Heap::new`; `scavenges` at 0.

Add `#[cfg(test)] fn scavenge_for_test` only if the `scavenge_bare` helper above cannot reach `scavenge` (it can: the tests module is a child of `heap`).

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: PASS.

- [ ] **Step 7: Run the whole suite, debug and release**

Run: `cargo nextest run --workspace` and `cargo nextest run --workspace --release`
Expected: PASS, every golden unchanged; the size tests unchanged (no handle changed size).

- [ ] **Step 8: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/heap/scavenge.rs crates/ns-runtime/src/heap/space.rs crates/ns-runtime/src/heap.rs
git commit -m "feat: a copying scavenger with age-based tenuring"
```

---

### Task 4: The full collection compacts both generations into the old space

**Files:**
- Modify: `crates/ns-runtime/src/heap/space.rs` (`empty_nursery`)
- Modify: `crates/ns-runtime/src/heap.rs` (`compact`, `collect_for_test`)

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: `Space::empty_nursery`; `compact` leaves the nursery empty, `young_objects == 0`, the remembered set empty and no remembered bit set.

- [ ] **Step 1: Write the failing tests in `heap.rs`'s tests**

```rust
#[test]
fn a_full_collection_compacts_the_nursery_s_survivors_after_the_old_ones() {
    let mut heap = young_heap();
    let class = old_instance_class(&mut heap);
    let old_dead = old_instance(&mut heap, class);
    let old_kept = old_instance(&mut heap, class);
    let young_dead = heap.allocate_string(class, crate::CodePoints::from("dead"));
    let young_kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
    // Each generation reachable only through the other: the young string
    // through the old instance's field, the old instance through a young
    // array the root holds.
    heap.instance_set_field(old_kept, 0, Value::object(young_kept));
    let holder = heap.allocate_array(class, &[Value::object(old_kept)]);
    let _ = (old_dead, young_dead);
    let mut root = Value::object(holder);

    heap.collect_for_test(&mut [&mut root]);

    let statistics = heap.heap_statistics();
    assert_eq!(statistics.young_objects, 0);
    assert!(heap.remembered.is_empty());
    let Some(holder) = root.as_object() else { panic!("the root no longer holds an object") };
    assert!(!space::is_young(holder.offset));
    let Some(old_kept) = heap.array_element(holder, 0).and_then(|value| value.as_object()) else {
        panic!("the array lost its element")
    };
    assert!(!heap.header(old_kept).remembered(), "nothing is young any more");
    let Some(young_kept) = heap.instance_field(old_kept, 0).and_then(|value| value.as_object()) else {
        panic!("the instance lost its field")
    };
    assert_eq!(heap.str_value(young_kept), Some("kept".to_string()));
    let key = |object| heap.object_identity_key(object);
    assert!(key(class) < key(old_kept), "the old survivors keep their order");
    assert!(key(old_kept) < key(young_kept), "the nursery's follow them");
    assert!(key(young_kept) < key(holder), "in address order");
    // The class, the kept instance, the string and the array.
    assert_eq!(statistics.live_objects, 4);
}
```

Plus `a_full_collection_after_a_scavenge_finds_the_survivor_space`: allocate young, scavenge once (the survivors are in the past survivor space, not eden), allocate more young, collect: every rooted object reads back, `young_objects == 0`. And a stale-handle test: a young handle kept across `collect_for_test` panics with "stale handle" in a debug build.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: FAIL (the young survivors stay young; the nursery is not emptied).

- [ ] **Step 3: `empty_nursery` and the compaction**

```rust
/// After a full collection, which compacted the nursery's survivors into
/// the old space: eden and both survivor spaces empty.
pub(in crate::heap) fn empty_nursery(&mut self) {
    for survivor in &mut self.survivors {
        *survivor = Region::empty(survivor.base, survivor.end);
    }
    self.eden = Region::empty(self.eden.base, self.eden.end);
}
```

`compact` takes `let [past, eden, old_range] = self.space.occupied();` before anything else -- the live-bit walk reads the same three ranges -- and, after the live bits and `summarize(NURSERY_END / 8 / 64)`:

```rust
// The remembered bits go first, at the offsets the set holds now: once
// everything is old, nothing needs remembering.
for holder in std::mem::take(&mut self.remembered) {
    let index = header_index(holder);
    let words = self.space.words_mut();
    words[index] = Header(words[index]).with_remembered(false).0;
}
let new_top = NURSERY_END + 8 * live_words;
if self.space.ensure_committed(new_top).is_none() {
    out_of_memory(new_top)
}
let old = self.space.region(RegionName::Old);
// The words up to the new top must be lent while the nursery's survivors
// are copied above the old ones.
self.space.set_region_extent(RegionName::Old, old.start, old.top.max(new_top));
// The old space slides down first: a destination there is never above its
// source. The nursery's survivors then go above every old survivor, into
// words no slide still has to read.
for range in [old_range, past, eden] {
    let mut position = range.start / 8;
    while position < range.end / 8 {
        let words = self.space.words_mut();
        let (header, next) = object_at(words, position);
        if Header(words[header]).marked() {
            let destination = table.destination(position);
            words.copy_within(position..next, destination);
            let moved = destination + (header - position);
            words[moved] = Header(words[moved]).with_marked(false).with_age(0).0;
        }
        position = next;
    }
}
self.space.set_region_extent(RegionName::Old, NURSERY_END, new_top);
self.space.empty_nursery();
self.object_count = live_objects;
self.young_objects = 0;
self.scavenge_requested = false;
self.tenure_all_requested = false;
```

then the release and the forwarding exactly as Task 1 left them. `copy_within` for a nursery survivor reads below `NURSERY_END` and writes above it, so the ranges never overlap.

`collect_for_test` stays as it is; `prune_canonical_symbols` already runs between marking and compaction over young and old entries alike.

- [ ] **Step 4: Run the heap tests**

Run: `cargo nextest run -p ns-runtime --lib heap`
Expected: PASS.

- [ ] **Step 5: Run the whole suite**

Run: `cargo nextest run --workspace`
Expected: PASS, every golden unchanged.

- [ ] **Step 6: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/heap/space.rs crates/ns-runtime/src/heap.rs
git commit -m "feat: the full collection compacts both generations into the old space"
```

---

### Task 5: The run loop scavenges, the load tenures, and every heap allocates young

**Files:**
- Modify: `crates/ns-runtime/src/heap/world.rs` (`StepRoots`, `World::scavenge`, `World::collect`, `mark_everything_reachable`, `forward_step_state`)
- Modify: `crates/ns-runtime/src/heap.rs` (`young_allocation` goes; statistics; `record_scavenge`; `set_scavenge_threshold`)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs`, `crates/ns-runtime/src/interpreter/mod.rs`
- Modify: `crates/ns-runtime/src/image/mod.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (the statistics' re-exports, if they list fields)
- Modify: `crates/ns-cli/src/entry.rs`, `crates/ns-cli/tests/cli.rs`
- Modify: `crates/ns-runtime/tests/collector.rs`, `tests/image.rs`, `tests/stress.rs`

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: `StepRoots`, `World::scavenge(tenure_all)`, `Interpreter::{tenure_now (pub(crate)), scavenge_now, set_scavenge_threshold}` (the last two debug-only, for tests), `HeapStatistics::young_objects`, `CollectorStatistics::{scavenges, scavenge_pause, longest_scavenge_pause}`.

- [ ] **Step 1: Write the failing tests**

In `tests/collector.rs`, the run-loop trigger (beside `allocating_past_the_limit_collects_during_the_run`, which is rewritten below):

```rust
/// Eden reaching its threshold asks for a scavenge, and the run loop's next
/// safepoint serves it: the closures die young and the run's answer stands.
#[test]
fn filling_eden_scavenges_during_the_run() {
    let (mut interpreter, class) = load(CHURN);
    interpreter.set_scavenge_threshold(16 * 1024);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::small_integer(5000));
    let answer = expect_ok(interpreter.send(&instance, Symbol::intern("run:"), &[count]));
    let answer_value = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    let collector = interpreter.collector_statistics();
    assert_eq!(answer_value, 5000);
    assert!(collector.scavenges > 0, "eden passed its threshold without a scavenge");
    assert!(interpreter.heap_statistics().young_objects < 5000);
}
```

(`CHURN` is the `Churn` module of `allocating_past_the_limit_collects_during_the_run`, lifted into a `const` both tests share.)

`allocating_past_the_limit_collects_during_the_run` needs objects that reach the old space, since closures that die young no longer do. Its fixture keeps every closure it makes in an `Array` that lives through the run -- 5,000 closures, which with a 16 KiB scavenge threshold survive dozens of scavenges and are tenured once their age reaches the threshold, the `Array` itself (40 KB) old from birth -- and the test lowers both limits:

```rust
const KEEPER: &str = indoc! {"
    class Keeper = Object new () (
      public run: count = (
        | kept index |
        kept:: Array new: count.
        index:: 1.
        [index <= count] whileTrue: [
          | captured = index. |
          kept at: index put: [captured].
          index:: index + 1.
        ].
        ^(kept at: count) value
      )
    ) : ()
"};
```

with `interpreter.set_scavenge_threshold(16 * 1024)` and `interpreter.set_collection_floor(16 * 1024)` before the send, the answer asserted to be 5000, and `collector.scavenges > 0 && collector.collections > 0`: the scavenges promote what the survivor space cannot hold, the promotions reach the old space's limit, and the full collection runs at the next safepoint. If `Array` is not reachable from a module without a platform, use the `usingPlatform:` form and `platform collections Array`, as the goldens under `goldens/library/collection/` do.

Every other `collector.rs` test keeps its assertions: `collect_now` empties the nursery, so the counts they compare after it are the old space's.

In `tests/image.rs`, the Review Focus load case:

```rust
/// The root instance's class is the module class and its `collections` slot
/// what a platform path answered: both were young when the load began, and
/// the load's tenure made them old before the graph pass named them
/// (`a_stored_platform_and_a_stored_member_agree_after_a_load`, after both
/// collections).
#[test]
fn a_loaded_graph_naming_a_platform_path_and_a_module_class_survives_both_collections() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public keptPlatform = platform.
              public collections = platform collections. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.scavenge_now();
    loaded.interpreter.collect_now();
    loaded.interpreter.scavenge_now();
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let kept = send_to(&mut loaded, &instance, "keptPlatform");
    let through_the_platform = send_to(&mut loaded, &kept, "collections");
    let stored = send_to(&mut loaded, &instance, "collections");
    assert!(
        same_object(&mut loaded, &through_the_platform, &stored),
        "a stored platform and a stored member answer one collections module after both collections"
    );
}
```

(`scavenge_now` and `collect_now` are debug-only, as every test in `tests/image.rs` that calls `collect_now` already is: gate the test with `#[cfg(debug_assertions)]` if the file's neighbours do.)

In `crates/ns-cli/tests/cli.rs`, `benchmark_collector_statistics_reports_to_stderr` asserts `stderr.contains("scavenges")` and `!stderr.contains("collector: 0 scavenges")`, with the comment rewritten: `bench_block.ns` allocates a closure every iteration and scavenges even at `--iterations 1`; check the count by hand once against the built binary and write it into the comment.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime --test collector` and `cargo nextest run -p ns-cli --test cli`
Expected: compile errors (`set_scavenge_threshold`, `scavenges`, `young_objects`).

- [ ] **Step 3: One enumeration of the step state's roots**

In `world.rs`:

```rust
/// What the step state holds beside the heap that names objects: the
/// bootstrap's builtin classes, the `platform` singleton, the VM mirror,
/// the kernel and the depth limit's reserve. Marking, forwarding and
/// scavenging all trace it through this one `Trace` impl, so that a root
/// added later is named once and reached by all three.
pub(crate) struct StepRoots<'state, 'gc> {
    builtin_classes: &'state mut Vec<ClassReference<'gc>>,
    platform_singleton: &'state mut Option<Value<'gc>>,
    vm_mirror: &'state mut Option<Value<'gc>>,
    kernel: &'state mut Option<Value<'gc>>,
    depth_reserve: &'state mut Option<FrameReference<'gc>>,
}

impl crate::trace::Trace for StepRoots<'_, '_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let StepRoots { builtin_classes, platform_singleton, vm_mirror, kernel, depth_reserve } = self;
        for class in builtin_classes.iter_mut() {
            tracer.object(class);
        }
        if let Some(platform) = platform_singleton.as_mut() {
            tracer.value(platform);
        }
        if let Some(mirror) = vm_mirror.as_mut() {
            tracer.value(mirror);
        }
        if let Some(kernel) = kernel.as_mut() {
            tracer.value(kernel);
        }
        if let Some(frame) = depth_reserve.as_mut() {
            tracer.frame(frame);
        }
    }
}
```

The field types are `Mutator`'s (`interpreter/mod.rs`: `builtin_classes: Vec<ClassReference<'gc>>`, `platform_singleton`, `vm_mirror` and `kernel: Option<Value<'gc>>`, `depth_reserve: Option<FrameReference<'gc>>`). One function destructures the mutator, so the heap and the step roots are borrowed side by side:

```rust
fn split<'state, 'gc>(mutator: &'state mut Mutator<'gc>) -> (&'state mut Heap<'gc>, StepRoots<'state, 'gc>) {
    let Mutator { heap, builtin_classes, platform_singleton, vm_mirror, kernel, depth_reserve, .. } = mutator;
    (heap, StepRoots { builtin_classes, platform_singleton, vm_mirror, kernel, depth_reserve })
}
``` `mark_everything_reachable` and `forward_step_state` trace `StepRoots` instead of listing the fields.

```rust
/// One scavenge (`Heap::scavenge`), with the step state's roots traced
/// beside the heap's, timed like a collection.
pub(crate) fn scavenge(&mut self, tenure_all: bool) {
    let start = std::time::Instant::now();
    self.check_quiescent_state();
    let (heap, mut roots) = split(&mut self.mutator);
    heap.scavenge(tenure_all, |scavenger| {
        use crate::trace::Trace;
        roots.trace(scavenger);
    });
    self.mutator.heap.record_scavenge(start.elapsed());
}
```

`World::collect` is unchanged in shape: it marks both generations, prunes, compacts (Task 4) and forwards.

- [ ] **Step 4: The safepoint and the hooks**

`Interpreter::safepoint`, after the stress modes and before the collection branch:

```rust
if self.world.collection_requested() {
    self.world.collect();
    return;
}
if self.world.scavenge_requested() {
    let tenure_all = self.world.tenure_all_requested();
    self.world.scavenge(tenure_all);
    // As after a collection: the next poll finds nothing pending and
    // clears the safepoint request there.
    return;
}
```

with `World::{scavenge_requested, tenure_all_requested}` reading the heap's flags. A full collection takes precedence: it empties the nursery as well.

```rust
/// Tenures everything now, between entries, where only roots hold
/// values: `image::load` makes the builtins, the module classes and what
/// the platform paths answered old before the graph pass allocates old.
pub(crate) fn tenure_now(&mut self) {
    assert!(!self.running, "tenure_now is for between runs");
    self.world.scavenge(true);
}
```

Debug-only, beside `collect_now`: `scavenge_now(&mut self)` (`self.world.scavenge(false)`) and `set_scavenge_threshold(&mut self, bytes: usize)` (sets `Heap::scavenge_threshold`).

- [ ] **Step 5: Young allocation for good**

Remove `young_allocation`, `enable_young_allocation_for_test` and `young_heap` (the heap tests call `Heap::new()`); `allocate_block`'s `young` is `!self.allocating_old && bytes < LARGE_OBJECT_BYTES`, and its born-remembered test `!space::is_young(offset) && !self.allocating_old`. The heap tests that asserted an old offset for a small object (`objects_lie_end_to_end_from_the_old_space_s_first_offset`) now assert eden's: `key(class) == space::EDEN_START`.

- [ ] **Step 6: The load tenures once and decodes old**

In `image/mod.rs`'s `load`, after `resolve_platform_paths` and its identity-hash check:

```rust
// Every object the graph will name is old from here on: the builtins,
// the module classes and what the paths answered (decided with the user
// on 2026-10-04; `docs/design/image-format.md`, "The image under
// generations"). The graph pass then allocates old, and its stores are
// old-to-old.
interpreter.tenure_now();
```

and the third phase's closure body moves, unchanged, into a function taking what it captured today:

```rust
/// `load`'s third phase: the graph against what the paths answered, run
/// with the heap allocating old.
#[allow(clippy::too_many_arguments)]
fn decode_graph_phase<'gc>(
    mutator: &mut Mutator<'gc>,
    reader: &mut decode::Reader<'_>,
    header: &decode::Header,
    symbols: &symbols::Table,
    table: &decode::DecodedCodeTable,
    mixins: &mixins::Decoded,
    module_roots: &[Root],
    module_levels: &[Vec<Root>],
    resolved: &[Root],
) -> Result<Vec<Root>, String> {
    // the closure's body, from `let class_of = …` to the final `Ok(…)`
}
```

(match each parameter's type to what `load` holds -- `read_header`'s answer, `resolve_platform_paths`' -- and drop the `allow` if clippy does not ask for it), called as:

```rust
let roots = interpreter.enter(|mutator| -> Result<Vec<Root>, String> {
    mutator.heap.set_allocating_old(true);
    let decoded = decode_graph_phase(
        mutator,
        &mut reader,
        &header,
        &symbols,
        &table,
        &mixins,
        &module_roots,
        &module_levels,
        &resolved,
    );
    // Cleared on the refusal path too: the interpreter of a refused image
    // is dropped, but nothing should rest on that.
    mutator.heap.set_allocating_old(false);
    decoded
})?;
```

so the flag is cleared on the refusal path too. The decode's transient peak is unchanged: the same placeholders, in the old space instead of eden.

- [ ] **Step 7: Statistics and the CLI line**

`HeapStatistics` gains `young_objects: usize`; `occupied_bytes` stays both generations' occupancy; `committed_bytes` includes the nursery. `CollectorStatistics` gains `scavenges: usize`, `scavenge_pause: Duration`, `longest_scavenge_pause: Duration`, filled by `record_scavenge` as `record_collection` fills its three. `print_collector_statistics`:

```rust
eprintln!(
    "collector: {} scavenges, {:.3} ms total pause, {:.3} ms longest; \
     {} collections, {:.3} ms total pause, {:.3} ms longest; {} live objects",
    statistics.scavenges,
    statistics.scavenge_pause.as_secs_f64() * 1000.0,
    statistics.longest_scavenge_pause.as_secs_f64() * 1000.0,
    statistics.collections,
    statistics.total_pause.as_secs_f64() * 1000.0,
    statistics.longest_pause.as_secs_f64() * 1000.0,
    statistics.live_objects,
);
```

No golden records this line (it goes to stderr only under `--collector-statistics`, which no golden passes); confirm with `grep -rn "collector:" goldens` before committing.

- [ ] **Step 8: Run the whole suite, debug and release**

Run: `cargo nextest run --workspace` and `cargo nextest run --workspace --release`
Expected: PASS, every golden unchanged. A golden that changes is a behaviour change: stop and report it.

- [ ] **Step 9: Run the move and collect stress modes over `ns-runtime`**

Run: `NS_NEXT_STRESS_MOVES=1 cargo nextest run -p ns-runtime` and `NS_NEXT_STRESS_COLLECT=1 cargo nextest run -p ns-runtime`
Expected: PASS. These are the first runs with young objects under a move and under a full collection at every safepoint; the whole-workspace stress runs stay in Task 9.

- [ ] **Step 10: Per-commit gate, then commit**

```bash
git add crates/ns-runtime crates/ns-cli
git commit -m "feat: the run loop scavenges, an image load tenures, and every heap allocates young"
```

---

### Task 6: The scavenge stress mode, the remembered-set verification and the poison

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs` (`POISON`)
- Modify: `crates/ns-runtime/src/heap.rs` (`verify_remembered_set`, the poisoning in `scavenge`, the check in `block`/`block_mut`)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs`, `crates/ns-runtime/src/interpreter/mod.rs`
- Modify: `crates/ns-runtime/tests/stress.rs`, `tests/collector.rs`
- Modify: `.claude/skills/finishing-a-batch/SKILL.md`

**Interfaces:**
- Consumes: Task 5.
- Produces: `NS_NEXT_STRESS_SCAVENGE`; `Interpreter::{set_stress_scavenge, stress_scavenges}`; `StressSchedule` (the collect and the scavenge schedules); `Heap::verify_remembered_set` (debug); `header::POISON` (debug).

- [ ] **Step 1: Write the failing tests**

In `heap.rs`'s tests:

```rust
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "missed the write barrier")]
fn the_verification_finds_an_old_object_naming_a_young_one_unremembered() {
    let mut heap = Heap::new();
    let class = old_instance_class(&mut heap);
    let young = heap.allocate_instance(class, 0);
    let holder = old_instance(&mut heap, class);
    heap.instance_set_field(holder, 0, Value::object(young));
    // What a store that bypassed the barrier would leave behind.
    heap.remembered.clear();
    heap.set_header(holder, heap.header(holder).with_remembered(false));
    heap.verify_remembered_set();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "vacated nursery")]
fn a_word_left_naming_a_reclaimed_young_object_reads_the_poison() {
    let mut heap = Heap::new();
    let class = old_instance_class(&mut heap);
    let young = heap.allocate_instance(class, 0);
    let holder = old_instance(&mut heap, class);
    // The barrier is bypassed by writing the word as a missed store would.
    let word = heap.store(Value::object(young));
    heap.fields_mut(holder)[0] = word.bits();
    heap.remembered.clear();
    heap.set_header(holder, heap.header(holder).with_remembered(false));
    scavenge_bare(&mut heap, false, &mut []);
    let stale = heap.load(Word::from_bits(heap.fields(holder)[0]));
    let _ = stale.as_object().map(|object| heap.object_class(object));
}
```

(The second test reads through the heap after the scavenge: the word still names eden's first object, which is now poison. `heap.load` gives it the current epoch, so only the poison check catches it -- which is the point.)

In `tests/stress.rs`:

```rust
/// The scavenge stress mode verifies the remembered set and scavenges at
/// every safepoint; the program's answer must not change.
#[test]
fn every_continuation_survives_a_scavenge_at_every_safepoint() {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_scavenge(true);
    let sum = run_sum(&mut interpreter, &class);
    assert_eq!(sum, 31);
    assert!(interpreter.stress_scavenges() > 0, "the scavenge stress mode never scavenged");
}
```

and `run_every_continuation` adds `interpreter.set_stress_scavenge(false);` beside `set_stress_collect(false)`, with its doc comment naming the scavenge stress mode as a third thing that advances the move epoch. In `tests/collector.rs`, `every_kind_of_stored_word_survives_moves_and_collections` also turns on `set_stress_scavenge(true)` and asserts `stress_scavenges() > 0`.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo nextest run -p ns-runtime`
Expected: compile errors (`verify_remembered_set`, `set_stress_scavenge`).

- [ ] **Step 3: The verification**

```rust
/// Asserts that every old object naming a young one is remembered, and
/// that the remembered set and the bits agree -- what a store that missed
/// the write barrier breaks, found before the scavenge that would lose the
/// young object, as Strongtalk's `rSet::verify`
/// (`prior-art/strongtalk/vm/memory/rSet.cpp:275`) and HotSpot's
/// `VerifyBeforeGC` find it. A walk of the old space, so only the scavenge
/// stress mode runs it.
#[cfg(debug_assertions)]
pub(crate) fn verify_remembered_set(&self) {
    let words = self.space.words();
    let mut remembered_bits = 0;
    let old = self.space.region(RegionName::Old);
    let mut position = old.start / 8;
    while position < old.top / 8 {
        let (header, next) = object_at(words, position);
        let block = block_range(words, header);
        let names_young = pointer_ranges(&words[block.clone()]).into_iter().any(|range| {
            words[range.start + block.start..range.end + block.start]
                .iter()
                .any(|bits| Word::from_bits(*bits).object_offset().is_some_and(space::is_young))
        });
        let remembered = Header(words[header]).remembered();
        assert!(
            !names_young || remembered,
            "internal: an old object names a young one without being remembered: a store missed the write barrier"
        );
        remembered_bits += usize::from(remembered);
        position = next;
    }
    assert_eq!(
        remembered_bits,
        self.remembered.len(),
        "internal: the remembered set and the remembered bits disagree"
    );
}
```

- [ ] **Step 4: The poison**

In `header.rs`:

```rust
/// What a debug build writes over the nursery memory a scavenge vacated:
/// a header of the free format, which no object in the space has, so that
/// a word left naming a reclaimed young object -- a store that missed the
/// write barrier -- fails at its first read instead of answering whatever
/// is allocated there next. HotSpot's `ZapUnusedHeapArea`, without its
/// page protection, which a safe slice over the nursery rules out
/// (decided with the user on 2026-10-04).
#[cfg(debug_assertions)]
pub(super) const POISON: u64 = ((Format::Free as u64) << FORMAT_SHIFT) | 0xBAD;
```

with a header test asserting `Header(POISON).format() == Format::Free` and `Header(POISON).word_count() == 0`.

In `Heap::scavenge`, where Task 3 left the marker comment -- after the scavenger is dropped and before `flip_survivors` -- in a debug build, over the `past` and `eden` ranges `scavenge` took from `occupied()` at its start:

```rust
#[cfg(debug_assertions)]
{
    let words = self.space.words_mut();
    for range in [past, eden] {
        words[range.start / 8..range.end / 8].fill(header::POISON);
    }
}
```

(The comment Task 3 left there goes.)

Only the occupied part is filled, so a scavenge at every safepoint stays cheap.

`block` and `block_mut` check the header before computing the range, in a debug build:

```rust
#[cfg(debug_assertions)]
{
    let format = Header(words[header_index(object.offset)]).format();
    assert!(
        !matches!(format, Format::Free | Format::Forwarded),
        "a handle names vacated nursery memory: a store missed the write barrier"
    );
}
```

- [ ] **Step 5: The stress mode**

In `safepoint.rs`, `StressCollections` becomes `StressSchedule`, read from a named variable: `StressSchedule::from_env("NS_NEXT_STRESS_COLLECT")` and `StressSchedule::from_env("NS_NEXT_STRESS_SCAVENGE")`; its `collections` counter becomes `fired`. `Interpreter` gains `stress_scavenge: Option<safepoint::StressSchedule>` (debug-only), set from the environment in `bootstrap_with` and counted in `stress_enabled`; `set_stress_scavenge(enabled)` and `stress_scavenges()` mirror `set_stress_collect` and `stress_collections`, and every `set_stress_*` keeps the request set while any of the four is on. In `safepoint`:

```rust
if let Some(stress) = &mut self.stress_scavenge {
    if stress.due(self.world.traced_size()) {
        self.world.verify_remembered_set();
        self.world.scavenge(false);
    }
    stressed = true;
}
```

with `World::verify_remembered_set` forwarding to the heap (debug-only).

- [ ] **Step 6: Run the tests, then the new mode over `ns-runtime`**

Run: `cargo nextest run -p ns-runtime`, then `NS_NEXT_STRESS_SCAVENGE=1 cargo nextest run -p ns-runtime`
Expected: PASS both. A verification failure under the stress mode names a store that misses the barrier: fix the store accessor, never the check.

- [ ] **Step 7: The gate loop names the fifth mode**

In `.claude/skills/finishing-a-batch/SKILL.md`, the loop becomes `for mode in MOVES CACHES COLLECT SCAVENGE WIDE` and the `run-corpus` lines gain `NS_NEXT_STRESS_SCAVENGE=1 cargo run -p ns-cli -- run-corpus corpus/modules`; every mention of "the four stress modes" in the skill becomes five. `CLAUDE.md`'s mention changes in Task 8.

- [ ] **Step 8: Per-commit gate, then commit**

```bash
git add crates/ns-runtime .claude/skills/finishing-a-batch/SKILL.md
git commit -m "test: a scavenge stress mode that verifies the remembered set, and poisoned nursery memory"
```

---

### Task 7: Measured against the tree the branch started from

**Files:** none in the repository until Task 8 records the numbers.

- [ ] **Step 1:** Follow the `measuring-performance` skill and `execution.md`'s protocol ("What removing the re-entries moved", "The Are We Fast Yet benchmarks"): pinned release builds of `master` at the branch point (`f7d8efdb`) and of the branch head, the same explicit `RUSTFLAGS`, the sixteen benchmarks and the eight microbenchmarks (the four not in the tree written out from `execution.md`, "What spec-conformant lookup moved", never reconstructed), pooled over two sittings.
- [ ] **Step 2:** `--collector-statistics` on every benchmark, both trees: scavenges, full collections, their pauses. A benchmark whose full collections did not fall is one whose survivors are tenured too early or whose old space's limit is too low; report it before Task 8.
- [ ] **Step 3:** The barrier's cost on its own: profile the most store-heavy benchmarks (`DeltaBlue`, `Richards`, `Havlak` among them) and name the share of samples in `write_barrier` and `remember`. If the barrier's inline compare is not folded into the store accessor's path, report it.
- [ ] **Step 4:** Peak working set, as `memory.md`'s "The interim cost" measures it, on `bench_loop.ns`, `bench_send.ns` and `bench_block.ns`, both trees: the nursery commits 10 MiB at once, so a small program's peak grows by up to that; record it.
- [ ] **Step 5:** Record, per program, the change and its spread; profile the two largest regressions or the two largest gains and name where the time went. Anything worth fixing that this branch does not fix goes into `docs/OPEN-WORK.md` in Task 8, the sizes chosen in this plan's header among them if the measurement argues for others.

---

### Task 8: The documentation describes the built state

**Files:**
- Modify: `docs/design/memory.md`: "Planned: generations" becomes the built description in the present tense (its own section, "Generations", after "The object space"): the layout and its constants, the scavenger, the age table and the threshold, forwarders, the precommit, the roots, the write barrier and its call sites, the remembered set and its limit, large objects, born-remembered allocations and eden's reserve, the full collection over both generations (decision 2 of this plan), the weak symbol table at a scavenge, the scavenge stress mode with its verification and poison (decision 1, which replaces the alternating eden and its page protection); "The object space" (the space is four regions; walks read the occupied ranges; the move stress mode shifts each region within itself; the trigger is the old space's occupancy); "The shape: mark, compact, forward", "Compaction and forwarding" (two segments, the nursery after the old space), "The trigger and the growth policy" (the old space's limit, eden's threshold), "Weak references", "Frames, the activation stack and the marriage" (a frame tenured while married; the divorce's barrier), "The gates the collector needs" (five stress modes), "What the collector traces" (`StepRoots`), "The move stress mode", "The amplification bound of a rejected load" (the decoder's placeholders are old; confirm every figure stands against the code), "What changes in `CLAUDE.md`, and when" (step 4 done), "Decisions" (this plan's four).
- Modify: `docs/design/image-format.md`: "Planned: the image under generations" becomes built: the one tenure before the graph pass (decision 4) and the graph allocated old.
- Modify: `docs/design/execution.md`: Task 7's numbers as "What generations moved".
- Modify: `docs/design/README.md`: the map's status for memory.
- Modify: `CLAUDE.md`: the crate list's heap sentence (a nursery of eden and two survivor spaces below the old space, a Cheney scavenger beside the mark-and-slide collector, which compacts both generations; the barrier in `heap.rs`'s store accessors); "An image is untrusted input" (the decoder allocates only after a tenure between entries; the platform-path sends may collect at their safepoints like any run; the graph pass allocates old, after the last tenure, and never polls a safepoint -- so the peak argument stands); "the four stress modes" becomes five with `SCAVENGE`; a new invariant under "Invariants that are easy to break": **Every store into an existing object goes through `heap.rs`'s store accessors and their write barrier** -- a store that writes a word into an old object any other way leaves a young object unreachable to the next scavenge, silently; `NS_NEXT_STRESS_SCAVENGE`'s verification is what finds it (`memory.md`, "Generations").
- Modify: `docs/OPEN-WORK.md`: "Next" names road step 5, `become:` through forwarders; road step 4 becomes built in "The road"; add what Task 7 leaves open and anything the reviews defer.
- Modify: `docs/PROGRESS.md`: the milestone, and where the next session starts: road step 5.

- [ ] **Step 1:** Rewrite each section named above; every statement in the present tense, no "planned" for what is built, no "historical" label on anything. Each decision keeps its reason and its prior art.
- [ ] **Step 2:** Check `git diff --stat` is Markdown only, then commit (no gates for a Markdown-only commit):

```bash
git add docs CLAUDE.md
git commit -m "docs: generations are built, and road step 5 is next"
```

---

### Task 9: The end-of-batch gates

- [ ] **Step 1:** Run the `finishing-a-batch` skill: the five stress modes (moves, caches, collection, scavenge, every instruction wide), `run-corpus` plain and under each stress mode, the benchmarks profile, `verifyPlugin`, and the check that a fresh session can start from the documentation alone.
- [ ] **Step 2:** Fix what they find, each fix its own commit through the per-commit gate.
- [ ] **Step 3:** Request the final whole-branch review on Fable before merging into `master`.
