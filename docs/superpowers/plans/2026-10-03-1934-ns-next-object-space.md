# The Object Space Implementation Plan (Road Step 3b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the heap's arena of boxed blocks with one space of variably sized objects in `ns-memory`'s reservation: handles become 64-bit byte offsets, allocation bumps a pointer, and the collector marks in the header and slides the survivors down; the layout of an object's words, which road step 3a built, does not change.

**Architecture:** Seven stages, each landing green, then the measurement, the documentation and the gates. The memory layer first lends its committed prefix in O(1) (Task 1). Then two changes that do not need the space are made on the arena, so that each is reviewed and measured alone: marking sets the header's mark bit through one enumeration of an object's pointer words by index range (Task 2), and handles become 64-bit byte offsets, still backed by arena slots (Task 3). The space is built and tested on its own (Task 4), then the objects move into it, with the sliding compaction and its live-word bitmap, the shifting move of the stress mode, and the trigger in bytes (Task 5). Finally every allocation writes its object in place and the per-object Rust allocations go (Task 6).

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime`; the `unsafe` stays in `ns-memory`. No new dependency (`ns-runtime` gains a path dependency on the workspace's own `ns-memory`). Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/memory.md`, "Planned: the object space" (the whole section), "The memory layer" (the heap's use of the reservation), "Weak references", "Frames, the activation stack and the marriage", "The trigger and the growth policy", "The move stress mode"; `docs/design/object-model.md`, "Planned with road step 3b: handles"; `docs/OPEN-WORK.md`, "Next" (what 3b's plan retires). Decided with the user on 2026-09-29 and 2026-10-03, and in this plan's session on 2026-10-03 (below). Background: `docs/design/memory.md`, "The seam", "A handle is arena-relative", "What the collector traces", "Marking", "Compaction and forwarding", "The amplification bound of a rejected load", "Payloads belong to the Rust allocator"; `docs/design/execution.md`, "The Are We Fast Yet benchmarks" and the measurement protocol. Prior art: Spur's overflow word and object walk (`numSlotsOfAny:`, `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:55584`; `objectAfter:`, `:55781`), its planning compactor (`planCompactSavingForwarders`, `:62399`; `compact`, `:61596`), Primordial Soup's bump allocation (`prior-art/primordialsoup/vm/heap.h:373-387`), Nova's `CompactionList` as the rejected binary search (`prior-art/nova/nova_vm/src/heap/heap_bits.rs:1345`, `:1457`).

**Decided in this plan's session (2026-10-03, with the user):**

1. **The forwarding side table is a live-word bitmap with a prefix count per 64-word chunk.** One bit per word of the space for every word of a surviving object, and for each chunk of 64 words the number of live words before it; an object's new offset is the first object's offset plus 8 times (the chunk's prefix plus the `popcount` of the live bits below its header in the chunk). O(1) per handle, one cache line per lookup, about 3% of the space while a collection runs (two `u64`s per 64 words). Rejected: a sorted list of (old, new) with a binary search per handle -- O(log n) dependent loads, the cost `memory.md` already holds against Nova's `CompactionList`; a dense table indexed by offset/16 -- one load, but half the space again in memory and a cache miss per handle on any heap larger than the cache.
2. **The debug build's no-access protection of vacated pages is deferred.** The accessors lend only `[0, top)`, so a stale offset at or above the top already finds no word; the protection pays when code dereferences addresses itself, which is road step 8's generated code. 3b protects nothing; `memory.md` and `OPEN-WORK.md` say so (Task 8).
3. **A refused commit or an exhausted reservation aborts the process, as a failed `Vec` push does today**, through `std::alloc::handle_alloc_error`. Allocation stays infallible, so no hot path carries a `Result`. A Newspeak-level out-of-memory error is an `OPEN-WORK.md` item (Task 8).

**Chosen by this plan, for the user to confirm at review** (each stated where it is built, each a constant one line changes): the default reservation is 64 GiB on a 64-bit target and 1 GiB otherwise (address space costs nothing until committed); the first commit is 256 KiB and a growth commits at least double what is committed; the collection floor is 512 KiB of occupied space (4,096 of today's slots at a 128-byte average); a handle's offset is a `NonZeroU64`, so `Option<ObjectReference>` stays one word; the move stress mode shifts every object up by `8 * (2 + epoch % 31)` bytes when the space starts at its first offset and back down to it otherwise.

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic; nothing in this plan touches `vm.rs` beyond what a changed accessor signature forces.
  - An image is untrusted input: no byte sequence may panic; every id, tag, count and length is validated before it indexes, allocates or slices; every pre-allocation is bounded by `bounded_count`. No wire tag is added, removed or renumbered.
  - A handle means nothing against a `Heap` that did not mint it, nor across a move: the debug build's origin and epoch stay on every Rust-side handle and value and are checked by every accessor, `Heap::store` and the `Forwarder`. `Heap::load`/`Heap::store` stay `pub(crate)`. The brand needs no cast.
  - A running activation is an `ActivationReference`; only a `FrameReference` is ever stored. `bytecode::outer_reach_of` and `compact::narrow_outer_reach` are not touched. The frame's `MARRIED` field keeps holding a stack index.
  - Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`. Tests use a local `expect_ok` helper.
  - The VM mirror's six locks stay as they are. The inline caches hold no handle and are untouched.
- **Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`** and its submodules. A handle's offset is private to the heap module; `object_identity_key` stays the one way out, as an opaque key.
- **The layout of an object's words is unchanged**: the header word of `heap/header.rs` (bits 0-21 the guard, 22-26 the format, 27-30 age, 31 remembered, 32 negative, 33 mark, 34-55 the identity hash, 56-63 the word count after the header, 255 meaning an overflow word), then the class word, then the fields. From Task 2 on the collector sets and clears bit 33 during a collection; outside one it is 0 in every object. From Task 5 on an object whose word count after the header exceeds 254 is preceded by an **overflow word**: top byte `0xFF`, the full count in the low 56 bits; the handle names the header, not the overflow word.
- **The space** (Task 4 on): one `ns_memory::Reservation` per `Heap`; byte offset 0 is the reservation's base; the first object's allocation starts at byte offset 16 (`FIRST_OBJECT`), so an all-zero word never names a live object; objects are 8-byte aligned and at least 16 bytes; the accessors read through the slice `[0, top)` of the committed prefix, so every access is bounds-checked and a stale offset reads a wrong object at worst, never undefined behaviour; every field, element and register accessor checks its index against the object's own word count.
- **A golden that changes is a behaviour change.** No task in this plan changes or adds golden output; if one changes, stop and report it.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's `min_ident_chars` refuses one- and two-character names. Comments explain the decision, not the mechanics. Classic terminology.
- A Newspeak module inline in a test is an `indoc! {"…"}` or `formatdoc!`, indented with the Rust around it, the closing `"}` on its own line.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_014iiGvJnqAeLJtzhwKA6Poa`.
  A subagent's commit names the subagent's own model.
- Work happens on the branch `feature/object-space` in the main working directory (no worktree), created from `master` before Task 1.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"), every task, however small:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules` → "parsed 211, failed 0"

  Task 1 also runs `cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu` and the same for `aarch64-apple-darwin`. Tasks 3, 5 and 6 also run `cargo nextest run --workspace --release` (the size tests run only there). The stress modes, `run-corpus`, the benchmarks profile and the `ns-lsp` suite run once, in Task 9 (the `finishing-a-batch` skill). A commit that never saw them is work in progress; a subagent implementing a task is told so. Nothing under `crates/ns-syntax` or `editors/jetbrains` changes.
- Run every command in the foreground, except Task 7's measurement, which the controller session runs itself.

## Review Focus

- **An object past 254 words.** An `Array` of 300 elements, a String of 2,500 Latin-1 characters and a frame of 250 registers each take an overflow word: every element and register reads back, the object after it is found by the walk, and all three come through a collection that slides them and a move that shifts them with their contents intact. Pinned in Task 4 (the walk over a hand-built overflow object) and Task 5 (heap tests across `collect_for_test` and `move_bare_heap`, and an image round trip of a 300-element `Array` in `tests/image.rs`).
- **The space growing inside one step.** One allocation larger than everything committed so far (an `Array` of 100,000 elements, 800 KB against a 256 KiB first commit) commits enough at once, its last element reads back, and no accessor ever sees a slice shorter than the top. Pinned in Task 4 (`Space` grows past its first commit in one allocation) and Task 5 (the heap test).
- **A stale offset after a collection.** A handle kept across a collection or a move panics at its first read in a debug build ("stale handle"), whether its old offset now lands inside a survivor, between two, or above the new top. Pinned in Task 5 (the existing stale-handle tests, plus one across `collect_for_test` whose stale offset lands inside a slid survivor).
- **A married frame across a slide and a shift.** A frame reified for a running activation reads and writes the live registers after its object slid down in a collection and after a move shifted it, and its divorce copies the registers into the moved object exactly when `retains_registers` was set. Pinned by the existing `a_married_frame_survives_a_collection_and_reads_its_live_registers`, `a_frame_divorced_after_a_collection_holds_its_registers_only_when_retained` and `a_married_frame_survives_a_move_and_is_divorced_into_its_moved_object`, which Task 5 keeps green.
- **The reservation running out.** An allocation past the reservation answers `None` from `Space::allocate` and leaves the top where it was, so the heap's abort is reached through one branch and never through a half-written object. Pinned in Task 4.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-memory/src/lib.rs`, `page_map.rs` | `Reservation::prefix`/`prefix_mut`, the O(1) lending of the committed read-write prefix (Task 1) |
| `crates/ns-runtime/Cargo.toml` | the path dependency on `ns-memory` (Task 4) |
| `crates/ns-runtime/src/heap/header.rs` | the mark bit's accessors (Task 2) |
| `crates/ns-runtime/src/heap/object.rs` | `pointer_ranges`, the one enumeration of an object's pointer words (Task 2); `Payload` goes, the packers write in place (Task 6) |
| `crates/ns-runtime/src/heap/mark.rs` | the marker becomes a work list of offsets; `MarkBits` goes (Tasks 2, 3) |
| `crates/ns-runtime/src/heap/space.rs` (new, Task 4) | `Space`: the reservation, the start, the top, the committed end, bump allocation, growth and release; the overflow word; the object walk |
| `crates/ns-runtime/src/heap/compaction.rs` (new, Task 5) | `ForwardingTable` (the live-word bitmap with its prefix counts), `Shift` (the stress move's relocation) |
| `crates/ns-runtime/src/heap/value.rs` | a value's object word is the offset itself; `Word::object_offset` (Task 3) |
| `crates/ns-runtime/src/heap.rs` | the handles' offset field (Task 3); the space behind every accessor, the walks, the trigger in bytes, `compact`, `move_everything`, the statistics (Task 5); the in-place allocators (Task 6) |
| `crates/ns-runtime/src/heap/world.rs` | `collect` without a marker to hand on (Task 2) |
| `crates/ns-runtime/src/lib.rs` | `HeapStatistics`' new fields re-exported as today (Task 5) |
| `crates/ns-runtime/src/image/decode.rs` | `record_size::block_words` counts the overflow word (Task 5); a refused class fill is a `DecodeError` (Task 6) |
| `crates/ns-runtime/tests/collector.rs`, `tests/image.rs` | the statistics' new fields, the floor in bytes (Task 5); the overflow round trip (Task 5) |
| `docs/**`, `CLAUDE.md` | Task 8 |

**The interfaces later tasks rely on** (exact signatures):

```rust
// ns-memory (Task 1)
impl Reservation {
    pub fn prefix(&self, end: usize) -> Option<&[u64]>;          // [0, end), O(1)
    pub fn prefix_mut(&mut self, end: usize) -> Option<&mut [u64]>;
}

// heap/header.rs (Task 2)
impl Header {
    pub(super) fn marked(self) -> bool;
    pub(super) fn with_marked(self, marked: bool) -> Header;
}

// heap/object.rs (Task 2): ranges of indices into a block (header at 0)
pub(in crate::heap) const POINTER_RANGES: usize = 6;
pub(in crate::heap) fn pointer_ranges(block: &[u64]) -> [std::ops::Range<usize>; POINTER_RANGES];

// heap/mark.rs (Task 2; offsets from Task 3)
pub(crate) struct Marker { /* work list */ }
impl Marker {
    pub(crate) fn new() -> Marker;
    pub(crate) fn push(&mut self, object: std::num::NonZeroU64);   // u32 slot index in Task 2
    pub(crate) fn next(&mut self) -> Option<std::num::NonZeroU64>;
}

// heap/value.rs (Task 3)
impl Word {
    pub(super) fn object_offset(self) -> Option<std::num::NonZeroU64>;
    pub(super) fn with_object_offset(offset: std::num::NonZeroU64) -> Word;
}
// heap.rs (Task 3)
pub(crate) trait Relocation { fn apply(&self, offset: NonZeroU64) -> NonZeroU64; }

// heap/space.rs (Task 4)
pub(in crate::heap) const FIRST_OBJECT: usize = 16;
pub(in crate::heap) const DEFAULT_RESERVATION: usize; // 64 GiB on 64-bit targets
pub(in crate::heap) struct Space { /* reservation, start, top, committed */ }
impl Space {
    pub(in crate::heap) fn new(reservation_bytes: usize) -> Result<Space, ns_memory::MemoryError>;
    pub(in crate::heap) fn start(&self) -> usize;      // bytes
    pub(in crate::heap) fn top(&self) -> usize;        // bytes
    pub(in crate::heap) fn committed(&self) -> usize;  // bytes
    pub(in crate::heap) fn words(&self) -> &[u64];     // [0, top)
    pub(in crate::heap) fn words_mut(&mut self) -> &mut [u64];
    pub(in crate::heap) fn allocate(&mut self, words: usize) -> Option<usize>; // byte offset of its first word
    pub(in crate::heap) fn ensure_committed(&mut self, end: usize) -> Option<()>;
    pub(in crate::heap) fn set_extent(&mut self, start: usize, top: usize);
    pub(in crate::heap) fn release_above(&mut self, keep: usize);
}
pub(in crate::heap) fn allocation_words(word_count: usize) -> usize;     // with the overflow word
pub(in crate::heap) fn overflow_word(word_count: usize) -> u64;
pub(in crate::heap) fn object_at(words: &[u64], position: usize) -> (usize, usize); // (header index, next position), word indices
pub(in crate::heap) fn block_range(words: &[u64], header: usize) -> std::ops::Range<usize>;

// heap/compaction.rs (Task 5)
pub(crate) struct ForwardingTable { /* chunks */ }
impl ForwardingTable {
    pub(in crate::heap) fn with_words(words: usize) -> ForwardingTable;
    pub(in crate::heap) fn set_live(&mut self, words: std::ops::Range<usize>);
    pub(in crate::heap) fn summarize(&mut self) -> usize;            // live words in all
    pub(in crate::heap) fn destination(&self, word: usize) -> usize; // new word index of a live word
}
impl Relocation for ForwardingTable { .. }
#[cfg(debug_assertions)] pub(crate) struct Shift { from: u64, to: u64 }
#[cfg(debug_assertions)] impl Relocation for Shift { .. }

// heap.rs (Task 5): the statistics
pub struct HeapStatistics { pub live_objects: usize, pub occupied_bytes: usize, pub committed_bytes: usize }

// heap.rs (Task 6): the one allocation every allocator goes through
fn allocate_block(&mut self, class: Word, guard: DispatchId, format: Format, unused: u8,
                  negative: bool, field_count: usize) -> (ObjectReference<'gc>, std::ops::Range<usize>);
```

---

### Task 1: The memory layer lends its committed prefix in O(1)

The heap will read every field through one slice of the space, re-borrowed per access, since storing a slice of its own reservation would make it self-referential (`memory.md`, "The memory layer"). `Reservation::words` walks the page map on every call; the prefix starting at the base is the one range the heap reads, so the reservation keeps its end as a field, refreshed whenever the page map changes.

**Files:**
- Modify: `crates/ns-memory/src/lib.rs` (a field `accessible_prefix: usize`; `prefix`, `prefix_mut`; refresh after every `pages.set`)
- Modify: `crates/ns-memory/src/page_map.rs` (`accessible_prefix`)
- Test: both files' `tests` modules

**Interfaces:**
- Produces: `Reservation::prefix(&self, end: usize) -> Option<&[u64]>`, `Reservation::prefix_mut(&mut self, end: usize) -> Option<&mut [u64]>`, `PageMap::accessible_prefix(&self) -> usize`.

- [ ] **Step 1: Write the failing tests.** In `page_map.rs`'s tests:

```rust
#[test]
fn the_accessible_prefix_is_the_read_write_range_at_the_base() {
    let mut map = PageMap::new();
    assert_eq!(map.accessible_prefix(), 0);
    map.set(4096..8192, Some(Protection::ReadWrite));
    assert_eq!(map.accessible_prefix(), 0, "a range not at the base is no prefix");
    map.set(0..4096, Some(Protection::ReadWrite));
    assert_eq!(map.accessible_prefix(), 8192, "adjacent ranges merge into one prefix");
    map.set(4096..8192, Some(Protection::NoAccess));
    assert_eq!(map.accessible_prefix(), 4096);
    map.set(0..4096, None);
    assert_eq!(map.accessible_prefix(), 0);
}
```

In `lib.rs`'s tests:

```rust
#[test]
fn the_prefix_is_lent_up_to_the_end_of_the_committed_read_write_range() {
    let page = Reservation::page_size();
    let mut reservation = Reservation::reserve(4 * page).expect("reserve");
    assert!(reservation.prefix(8).is_none(), "nothing committed yet");
    assert_eq!(reservation.prefix(0).map(<[u64]>::len), Some(0));
    reservation.commit(0..2 * page).expect("commit");
    assert_eq!(reservation.prefix(2 * page).map(<[u64]>::len), Some(2 * page / 8));
    assert!(reservation.prefix(2 * page + 8).is_none());
    assert!(reservation.prefix(12).is_none(), "an end that is no word boundary");
    reservation.commit(2 * page..3 * page).expect("commit");
    assert!(reservation.prefix(3 * page).is_some());
    reservation.protect(page..2 * page, Protection::NoAccess).expect("protect");
    assert!(reservation.prefix(page).is_some());
    assert!(reservation.prefix(page + 8).is_none());
    reservation.decommit(0..page).expect("decommit");
    assert!(reservation.prefix(8).is_none());
}

#[test]
fn a_write_through_the_mutable_prefix_reads_back_through_words() {
    let page = Reservation::page_size();
    let mut reservation = Reservation::reserve(page).expect("reserve");
    reservation.commit(0..page).expect("commit");
    let prefix = reservation.prefix_mut(16).expect("committed");
    prefix[1] = 0xABCD;
    assert_eq!(reservation.words(8..16), Some(&[0xABCD][..]));
}

#[test]
fn a_recorded_failure_ends_the_prefix() {
    let page = Reservation::page_size();
    let mut reservation = Reservation::reserve(2 * page).expect("reserve");
    reservation.commit(0..2 * page).expect("commit");
    reservation.record_failure(page..2 * page, None);
    assert!(reservation.prefix(page).is_some());
    assert!(reservation.prefix(page + 8).is_none());
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo nextest run -p ns-memory` -- expected: `prefix`, `prefix_mut` and `accessible_prefix` do not exist.
- [ ] **Step 3: Implement.** In `page_map.rs`:

```rust
/// The end of the read-write range that starts at byte 0, or 0 when byte 0
/// is not committed read-write. Adjacent ranges of one protection are
/// merged, so the first range is the whole prefix.
pub(crate) fn accessible_prefix(&self) -> usize {
    match self.ranges.first() {
        Some((range, Protection::ReadWrite)) if range.start == 0 => range.end,
        _ => 0,
    }
}
```

In `lib.rs`, add `accessible_prefix: usize` to `Reservation` (0 at `reserve`), and a private `fn record(&mut self, range: Range<usize>, state: Option<Protection>)` that calls `self.pages.set(range, state)` and then `self.accessible_prefix = self.pages.accessible_prefix()`; route every `self.pages.set` in `commit`, `decommit`, `protect` and `record_failure` through it. Then:

```rust
/// The words of `0..end` when every byte of it is committed read-write --
/// the heap's space, read on every access, so this is one comparison
/// against the end the page map last reported rather than a walk of it.
/// `end` must be a multiple of 8; 0 lends an empty slice.
pub fn prefix(&self, end: usize) -> Option<&[u64]> {
    self.check_prefix(end)?;
    // SAFETY: `check_prefix` found `0..end` inside the committed read-write
    // prefix, which starts at the page-aligned base, so it is 8-byte
    // aligned and initialized; `&self` keeps `protect` and `decommit` away
    // while the slice lives.
    Some(unsafe { std::slice::from_raw_parts(self.base.as_ptr().cast::<u64>(), end / 8) })
}

/// As `prefix`, mutably.
pub fn prefix_mut(&mut self, end: usize) -> Option<&mut [u64]> {
    self.check_prefix(end)?;
    // SAFETY: as for `prefix`, and `&mut self` makes the slice unique.
    Some(unsafe { std::slice::from_raw_parts_mut(self.base.as_ptr().cast::<u64>(), end / 8) })
}

fn check_prefix(&self, end: usize) -> Option<()> {
    (end.is_multiple_of(8) && end <= self.accessible_prefix && end <= isize::MAX as usize)
        .then_some(())
}
```

Update the crate's doc comment and `Reservation`'s to name the prefix as the heap's access path.
- [ ] **Step 4: Run the tests, the per-commit gate, and the two cross-target clippy runs** (`--target x86_64-unknown-linux-gnu`, `--target aarch64-apple-darwin`).
- [ ] **Step 5: Commit**

```bash
git add crates/ns-memory
git commit -m "feat: the memory layer lends its committed prefix in O(1)"
```

---

### Task 2: Marking sets the header's mark bit, through one enumeration of an object's pointer words

On the arena still. The mark bits move from the side table into bit 33 of the header (`header.rs`), where the space's collector reads them as it walks; the marker becomes a work list; and the pointer words of a block are named once, as index ranges, so that marking can read a block and test its targets' headers by index while forwarding rewrites words through `trace_block` -- one enumeration for both (`memory.md`, "What the collector traces").

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs` (`MARK_BIT`, `marked`, `with_marked`; its module doc and the test that pins bit 33 at 0)
- Modify: `crates/ns-runtime/src/heap/object.rs` (`pointer_ranges`; `trace_block` over it)
- Modify: `crates/ns-runtime/src/heap/mark.rs` (`MarkBits` goes; `Marker` is a work list)
- Modify: `crates/ns-runtime/src/heap.rs` (`begin_marking`, `drain_marking`, `prune_canonical_symbols(&mut self)`, `compact(&mut self)`, `compact_arena` with a liveness closure; the marking tests)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`mark_everything_reachable` answers nothing; `collect`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Header::{marked, with_marked}`; `object::{POINTER_RANGES, pointer_ranges}`; `Marker::{new, push, next}` over `u32` slot indices (offsets from Task 3); `Heap::prune_canonical_symbols(&mut self)`; `Heap::compact(&mut self) -> Forwarder<ForwardingTable>`; `World::mark_everything_reachable(&mut self)`.

- [ ] **Step 1: Write the failing tests.** In `header.rs`, replace the assertion `assert_eq!(header.0 >> 33 & 1, 0, "the mark bit stays 0");` with a new test beside it:

```rust
#[test]
fn the_mark_bit_is_bit_thirty_three_and_disturbs_no_other_field() {
    let header = Header::new(DispatchId::NONE, Format::Closure, 0, 7).with_identity_hash(5);
    assert!(!header.marked(), "a new header is unmarked");
    let marked = header.with_marked(true);
    assert!(marked.marked());
    assert_eq!(marked.0, header.0 | 1 << 33);
    assert_eq!(marked.with_marked(false), header);
}
```

In `object.rs`'s tests (add a `#[cfg(test)] mod tests` if none):

```rust
#[test]
fn pointer_ranges_name_the_class_word_and_each_formats_pointer_fields() {
    let block = |format: Format, fields: usize| {
        let mut words = vec![Word::NIL.bits(); FIELDS + fields];
        words[HEADER] = Header::new(DispatchId::NONE, format, 0, 1 + fields).0;
        words
    };
    let covered = |words: &[u64]| -> Vec<usize> {
        pointer_ranges(words).into_iter().flatten().collect()
    };
    assert_eq!(covered(&block(Format::FixedPointers, 2)), vec![CLASS, 2, 3]);
    assert_eq!(covered(&block(Format::SixtyFourBit, 2)), vec![CLASS]);
    assert_eq!(
        covered(&block(Format::Closure, closure_field::COUNT + 1)),
        vec![CLASS, FIELDS + closure_field::CAPTURED_FRAME, FIELDS + closure_field::COUNT]
    );
    let class = block(Format::Class, class_field::COUNT);
    let mut expected = vec![CLASS];
    expected.extend(class_field::POINTERS.iter().map(|field| FIELDS + field));
    assert_eq!(covered(&class), expected);
    // A frame's registers count only once it is no longer married.
    let mut frame = block(Format::Frame, frame_field::COUNT + 2);
    assert_eq!(covered(&frame).len(), 1 + frame_field::COUNT + 2);
    frame[FIELDS + frame_field::MARRIED] = 0b1001; // a SmallInteger
    assert_eq!(covered(&frame).len(), 1 + frame_field::COUNT);
}
```

(The test module needs `use super::*;` and `use crate::mixin::DispatchId;`; `Format`, `Header` and `Word` come through `super`.)

In `heap.rs`'s tests, add the helper and rewrite the five marking tests (`marking_reaches_what_a_root_reaches_and_nothing_else`, `marking_follows_an_edge_out_of_a_reachable_object`, `marking_reaches_a_married_frame_through_its_entry`, `marking_reaches_a_frame_through_an_activation_object`, `marking_terminates_on_a_cycle_between_two_ordinary_objects`) to assert through it instead of `marker.objects.is_set(x.index)`:

```rust
/// Marks from the heap's own roots, answers whether each of `objects` was
/// reached, and clears every mark again, so that the test can go on to
/// collect: a mark left set would make the next marking skip that object's
/// edges.
fn marked_by_marking<'gc>(heap: &mut Heap<'gc>, objects: &[ObjectReference<'gc>]) -> Vec<bool> {
    let mut marker = heap.begin_marking();
    heap.drain_marking(&mut marker);
    let marked = objects.iter().map(|object| heap.header(*object).marked()).collect();
    heap.clear_marks_for_test();
    marked
}

#[test]
fn a_collection_leaves_no_mark_bit_set() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    let kept = heap.allocate_object(class, Payload::string(&crate::CodePoints::from("kept")));
    heap.allocate_object(class, Payload::string(&crate::CodePoints::from("dead")));
    let mut root = Value::object(kept);
    heap.collect_for_test(&mut [&mut root]);
    let Some(kept) = root.as_object() else { panic!("the root keeps its object") };
    assert!(!heap.header(kept).marked());
    assert!(!heap.header(heap.object_class(kept)).marked());
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo nextest run -p ns-runtime heap::` -- expected: `marked`, `with_marked`, `pointer_ranges` and `clear_marks_for_test` do not exist.
- [ ] **Step 3: Implement.**
  - `header.rs`: `const MARK_BIT: u64 = 1 << 33;`, `marked` and `with_marked` beside `negative`/`with_negative`; the module doc says the collector sets bit 33 while a collection runs and clears it as it compacts, and that age and the remembered bit stay 0 until road step 4.
  - `object.rs`: `pointer_ranges` answers, relative to the block's start, `CLASS..CLASS + 1` first, then by format: `FixedPointers`/`IndexablePointers` `FIELDS..len`; `Frame` `FIELDS..FIELDS + traced` with `traced` as `trace_block` computes it today (`frame_field::COUNT` while `MARRIED` is not nil, every field otherwise); `Closure` the captured frame's one word and `FIELDS + closure_field::COUNT..len`; `Class` one range per `class_field::POINTERS` entry; every other format nothing. Unused entries are `0..0`; every range is clamped to `block.len()`, so a short block yields empty ranges rather than a panic. `trace_block` becomes: read the ranges, then for each index in each range hand the word to `tracer.word` and write it back. Its doc keeps the format list, which now lives on `pointer_ranges`.
  - `mark.rs`: delete `MarkBits`; `Marker { work: Vec<u32> }` with `new`, `push` and `next`; its `Tracer` impl pushes every handle's and every object word's slot index without testing anything -- the drain tests the mark bit when it pops. The module doc says why: the marker holds no `Heap`, and a root reached twice costs one extra pop.
  - `heap.rs`: `begin_marking` builds `Marker::new()` and, in a debug build, asserts that no object is marked (`debug_assert!(self.objects.iter().all(|block| !Header(block[HEADER]).marked()))`, with the message "internal: a mark bit survived the last collection"). `drain_marking`:

```rust
pub(crate) fn drain_marking(&mut self, marker: &mut mark::Marker) {
    while let Some(index) = marker.next() {
        let block = &mut self.objects[index as usize];
        let header = Header(block[HEADER]);
        if header.marked() {
            continue;
        }
        block[HEADER] = header.with_marked(true).0;
        let block = &self.objects[index as usize];
        for range in pointer_ranges(block) {
            for bits in &block[range] {
                if let Some(target) = Word::from_bits(*bits).object_index() {
                    if !Header(self.objects[target as usize][HEADER]).marked() {
                        marker.push(target);
                    }
                }
            }
        }
    }
}
```

    `prune_canonical_symbols(&mut self)` reads `Header(self.objects[object.index as usize][HEADER]).marked()` instead of the marker. `compact_arena<T>(slots, mut take_mark: impl FnMut(&mut T) -> bool, floor)` asks the closure whether a slot survives; `compact` passes one that reads the header's mark bit and clears it, so a survivor leaves the collection unmarked. The test `compaction_never_shrinks_an_arena_below_its_next_limit` passes `|_| false` where it passed an empty `MarkBits`. `mark_from_roots_for_test` goes; `collect_for_test` drops the marker it no longer hands on. Add `#[cfg(test)] fn clear_marks_for_test(&mut self)`, which clears bit 33 in every block.
  - `world.rs`: `mark_everything_reachable(&mut self)` drains and answers nothing; `collect` calls `prune_canonical_symbols()` and `compact()` without a marker.
- [ ] **Step 4: Run the tests, then the per-commit gate.**
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "refactor: marking sets the header's mark bit, through one enumeration of an object's pointer words"
```

---

### Task 3: A handle is a 64-bit byte offset

Still on the arena: every type-level consequence of the 64-bit handle lands here, so that its cost in the release build's pinned sizes is seen alone, before the space changes what stands behind a handle. Slot `index` is named by the offset `16 * (index + 1)` -- a multiple of 8, never 0 and at least 16, as the space's offsets will be -- through two private functions that Task 5 deletes.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`ObjectReference` and `FrameReference` hold `offset: NonZeroU64`; `slot_of`, `offset_of_slot`; every `.index` of a handle; `Relocation`, `Rotation`, `ForwardingTable`, `Forwarder` over offsets; `object_identity_key`; `symbol_of: IdentityMap<u64, Symbol>`; `replace_resolved_futures`' map; the `Debug` impls; `next_object_index` without its `u32` ceiling)
- Modify: `crates/ns-runtime/src/heap/value.rs` (the object word is the offset; `Word::object_offset`, `Word::with_object_offset`; `Value::object`, `reference`, `as_object`, `kind`; the module's and `Value`'s doc)
- Modify: `crates/ns-runtime/src/heap/mark.rs` (`Marker` over `NonZeroU64`)
- Test: `heap.rs`, `value.rs` unit tests; the release size tests

**Interfaces:**
- Consumes: Task 2's `Marker`, `pointer_ranges`.
- Produces: `Word::object_offset(self) -> Option<NonZeroU64>`, `Word::with_object_offset(NonZeroU64) -> Word`; `Relocation::apply(&self, offset: NonZeroU64) -> NonZeroU64`; `Marker::push(NonZeroU64)`, `Marker::next() -> Option<NonZeroU64>`; `object_identity_key` answers the offset as a `usize`.

- [ ] **Step 1: Write the failing tests.** In `heap.rs`'s tests:

```rust
/// A handle is a byte offset: a multiple of 8, so that the value word holding
/// it has the object tag, and at least 16, so that an all-zero word -- a
/// missed initialization -- never names a live object.
#[test]
fn an_object_word_is_a_byte_offset_past_the_first_sixteen_bytes() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    let object = heap.allocate_instance(class, 1);
    for handle in [class, object] {
        let word = heap.store(Value::object(handle)).bits();
        assert_eq!(word & 0b111, 0, "an object word carries the object tag");
        assert!(word >= 16);
        assert_eq!(heap.object_identity_key(handle) as u64, word);
    }
}
```

and, beside the other release size tests:

```rust
/// A handle is one word, and the offset's niche keeps an optional handle
/// one word too: `Option<ClassReference>` sits in every stack entry and in
/// `Step`, whose size the hot path was measured at.
#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
#[test]
fn a_handle_and_an_optional_handle_are_one_word() {
    assert_eq!(std::mem::size_of::<ObjectReference<'static>>(), 8);
    assert_eq!(std::mem::size_of::<Option<ObjectReference<'static>>>(), 8);
    assert_eq!(std::mem::size_of::<FrameReference<'static>>(), 8);
    assert_eq!(std::mem::size_of::<Option<FrameReference<'static>>>(), 8);
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo nextest run -p ns-runtime an_object_word_is_a_byte_offset` -- expected: the word is `index << 3`, so the slot 0 class answers 0 and fails `>= 16`.
- [ ] **Step 3: Implement.**
  - The handle structs: `offset: std::num::NonZeroU64` in place of `index: u32`; their docs say "the byte offset of the object's header" (and, until Task 5, "of the arena slot it names, `16 * (slot + 1)`"); the `Debug` impls print `ObjectReference(@{offset})`.
  - `heap.rs`: `fn offset_of_slot(slot: usize) -> NonZeroU64` (`16 * (slot + 1)`, through `NonZeroU64::new(..).expect("internal: a slot's offset is never zero")`) and `fn slot_of(offset: NonZeroU64) -> usize` (`offset / 16 - 1`), each with a comment that Task 5's space replaces them; `object_handle(offset: NonZeroU64)`, `frame_handle(offset: NonZeroU64)`; every `x.index as usize` becomes `slot_of(x.offset)`. `next_object_index` becomes `fn next_object_offset(&self) -> NonZeroU64 { offset_of_slot(self.objects.len()) }` -- the `u32` ceiling goes, since no handle is 32 bits any more.
  - `Relocation::apply(&self, offset: NonZeroU64) -> NonZeroU64`; `Rotation` and `ForwardingTable` convert through `slot_of`/`offset_of_slot`; `ForwardingTable`'s `DEAD` stays a `u32` slot sentinel; the `Forwarder` rewrites `handle.offset` and `Word::with_object_offset(apply(..))`.
  - `object_identity_key` answers `usize::try_from(object.offset.get())` (`expect`ed: a 64-bit target's offset fits; its doc says the key is the offset and nothing may do arithmetic on it); `symbol_of: IdentityMap<u64, Symbol>`, keyed by `offset.get()`; `replace_resolved_futures`' map `HashMap<u64, Word>`.
  - `value.rs`: `Value::object` stores `object.offset.get()` as the word; `reference(self)` builds the handle from `NonZeroU64::new(self.word)`, which answers `None` only for the word 0, never stored -- `kind` hits `unreachable!("internal: a stored object word is never zero")` there, `as_object` answers `None`; `Word::object_offset` is `(self.0 & TAG_MASK == OBJECT_TAG).then(|| NonZeroU64::new(self.0)).flatten()`; `with_object_offset(offset)` is `Word(offset.get())`. `Value`'s doc: "An object is its byte offset, a multiple of 8, so its low three bits are the object tag."
  - `mark.rs`: `work: Vec<NonZeroU64>`; `drain_marking` indexes `self.objects[slot_of(offset)]`.
- [ ] **Step 4: Run the tests, the per-commit gate, and `cargo nextest run --workspace --release`.** The release run checks the pins `a_decision_stays_the_size_the_hot_path_was_measured_at` (56), `a_signal_fits_in_the_niche_of_its_one_unboxed_error` (40/56/40), `a_lookup_stays_the_size_the_hot_path_was_measured_at` (24) and the new one. The niche is meant to keep them; if one changes anyway, do not stop: set the pin to the measured size, say in the test's comment which handle grew it (the 4-byte handle became 8), name the old and new size in the commit message and in your report -- Task 7 measures what it costs, and the debug twin `a_decision_stays_the_size_it_has_in_a_debug_build` moves with it if it moves.
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "refactor: a handle is a 64-bit byte offset"
```

---

### Task 4: The space

`heap/space.rs`: the reservation and the extent of the objects in it, bump allocation, growth by commit, release by decommit, the overflow word, and the walk -- built and tested alone. Nothing uses it until Task 5.

**Files:**
- Modify: `crates/ns-runtime/Cargo.toml` (`ns-memory = { path = "../ns-memory" }`)
- Create: `crates/ns-runtime/src/heap/space.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (`mod space;`)
- Modify: `crates/ns-runtime/src/heap/header.rs` (`pub(super) const WORD_COUNT_OVERFLOW`, so `space.rs` shares it)

**Interfaces:**
- Consumes: Task 1's `Reservation::prefix`/`prefix_mut`; `Header::word_count`.
- Produces: everything listed under "heap/space.rs (Task 4)" above.

- [ ] **Step 1: Write the failing tests** in `space.rs`'s `tests` module:

```rust
use super::*;
use crate::heap::header::{Format, Header};
use crate::mixin::DispatchId;

const SMALL: usize = 16 << 20;

/// Writes an object of `word_count` words after its header at the
/// allocation `start` (a byte offset), its overflow word first when it
/// needs one, and answers its header's word index.
fn write_object(space: &mut Space, start: usize, word_count: usize) -> usize {
    let words = space.words_mut();
    let mut header = start / 8;
    if word_count >= WORD_COUNT_OVERFLOW {
        words[header] = overflow_word(word_count);
        header += 1;
    }
    words[header] = Header::new(DispatchId::NONE, Format::FixedPointers, 0, word_count).0;
    header
}

#[test]
fn a_new_space_is_empty_and_starts_at_the_first_object() {
    let space = Space::new(SMALL).expect("reserve");
    assert_eq!((space.start(), space.top()), (FIRST_OBJECT, FIRST_OBJECT));
    assert_eq!(space.words().len(), FIRST_OBJECT / 8);
}

#[test]
fn allocation_bumps_the_top_by_whole_words() {
    let mut space = Space::new(SMALL).expect("reserve");
    assert_eq!(space.allocate(2), Some(FIRST_OBJECT));
    assert_eq!(space.allocate(5), Some(FIRST_OBJECT + 16));
    assert_eq!(space.top(), FIRST_OBJECT + 56);
    assert_eq!(space.words().len(), (FIRST_OBJECT + 56) / 8);
}

#[test]
fn one_allocation_larger_than_everything_committed_commits_enough_at_once() {
    let mut space = Space::new(SMALL).expect("reserve");
    let committed = space.committed();
    let words = committed / 8 * 3;
    let start = space.allocate(words).expect("inside the reservation");
    assert!(space.committed() >= start + words * 8);
    space.words_mut()[start / 8 + words - 1] = 7;
    assert_eq!(space.words()[start / 8 + words - 1], 7);
}

#[test]
fn an_allocation_past_the_reservation_answers_none_and_leaves_the_top() {
    let mut space = Space::new(SMALL).expect("reserve");
    let top = space.top();
    assert_eq!(space.allocate(usize::MAX / 8), None, "an overflowing size");
    assert_eq!(space.allocate(SMALL), None, "eight times the reservation");
    assert_eq!(space.top(), top);
}

#[test]
fn the_walk_finds_every_object_an_overflow_word_included() {
    let mut space = Space::new(SMALL).expect("reserve");
    let sizes = [1, 300, 3];
    let mut headers = Vec::new();
    for size in sizes {
        let start = space.allocate(allocation_words(size)).expect("room");
        headers.push(write_object(&mut space, start, size));
    }
    let words = space.words();
    let mut position = space.start() / 8;
    let mut found = Vec::new();
    while position < space.top() / 8 {
        let (header, next) = object_at(words, position);
        assert_eq!(block_range(words, header).end, next);
        found.push(header);
        position = next;
    }
    assert_eq!(found, headers);
    assert_eq!(headers[1], headers[0] + 2 + 1, "the large object's header follows its overflow word");
    assert_eq!(block_range(words, headers[1]).len(), 1 + 300);
}

#[test]
fn releasing_never_cuts_below_the_top_and_gives_back_what_lies_above() {
    let mut space = Space::new(SMALL).expect("reserve");
    space.allocate(SMALL / 8 / 4).expect("a quarter of the reservation");
    let grown = space.committed();
    space.set_extent(FIRST_OBJECT, FIRST_OBJECT + 64);
    space.release_above(0);
    assert!(space.committed() < grown);
    assert!(space.committed() >= space.top());
    assert_eq!(space.words().len(), (FIRST_OBJECT + 64) / 8);
    assert!(space.allocate(2).is_some(), "it grows again after a release");
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo nextest run -p ns-runtime heap::space` -- expected: the module does not exist.
- [ ] **Step 3: Implement** `space.rs`:

```rust
//! The object space: one reservation of `ns-memory`, in which every object
//! of a heap lies end to end from `FIRST_OBJECT` up to the top, each its
//! header word, its class word and its fields -- preceded by an overflow
//! word when its word count does not fit the header's byte
//! (`docs/design/memory.md`, "The object space"). A handle is the byte
//! offset of an object's header from the reservation's base. The heap
//! reads every object through `words`, the slice `[0, top)`, so every
//! access is bounds-checked against the top and a stale offset reads a
//! wrong object at worst, never memory outside the objects.
#![cfg_attr(not(test), expect(dead_code, reason = "the heap allocates in the space from the next task on"))]

use ns_memory::{MemoryError, Reservation};

use crate::heap::header::{Header, WORD_COUNT_OVERFLOW};

/// The byte offset of the first object's allocation: past two words that
/// no object uses, so that an all-zero word -- a missed initialization --
/// is never a live reference.
pub(in crate::heap) const FIRST_OBJECT: usize = 16;

/// How much address space a heap reserves. Reserved space costs nothing
/// until it is committed, so this is generous; the heap aborts as running
/// out of memory once it is used up.
#[cfg(target_pointer_width = "64")]
pub(in crate::heap) const DEFAULT_RESERVATION: usize = 64 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(in crate::heap) const DEFAULT_RESERVATION: usize = 1 << 30;

/// The first commit, and the least a growth commits.
const MINIMUM_COMMIT: usize = 256 << 10;

/// The top byte of an overflow word: the header's own word-count byte at
/// its overflow mark, so that a walk standing at an allocation's first word
/// tells an overflow word from a header -- a header there never carries
/// the mark, since only an object with an overflow word does
/// (Spur's `numSlotsOfAny:` and `objectAfter:`,
/// `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:55584`, `:55781`).
const OVERFLOW_MARK: u64 = (WORD_COUNT_OVERFLOW as u64) << 56;
const OVERFLOW_COUNT: u64 = (1 << 56) - 1;

pub(in crate::heap) struct Space {
    reservation: Reservation,
    /// Where the first object's allocation starts: `FIRST_OBJECT`, except
    /// after the move stress mode shifted every object up.
    start: usize,
    /// Where the next allocation starts; every byte below it from `start`
    /// is an object.
    top: usize,
    /// The end of the committed range, which starts at the base.
    committed: usize,
}

impl Space {
    pub(in crate::heap) fn new(reservation_bytes: usize) -> Result<Space, MemoryError> {
        let mut reservation = Reservation::reserve(reservation_bytes)?;
        let committed = round_up_to_page(MINIMUM_COMMIT)
            .unwrap_or(MINIMUM_COMMIT)
            .min(reservation.size());
        reservation.commit(0..committed)?;
        Ok(Space { reservation, start: FIRST_OBJECT, top: FIRST_OBJECT, committed })
    }

    pub(in crate::heap) fn start(&self) -> usize { self.start }
    pub(in crate::heap) fn top(&self) -> usize { self.top }
    pub(in crate::heap) fn committed(&self) -> usize { self.committed }

    /// Every word below the top: the one slice the heap reads objects
    /// through.
    #[inline(always)]
    pub(in crate::heap) fn words(&self) -> &[u64] {
        match self.reservation.prefix(self.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    #[inline(always)]
    pub(in crate::heap) fn words_mut(&mut self) -> &mut [u64] {
        match self.reservation.prefix_mut(self.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    /// Bumps the top by `words` and answers the byte offset of the first;
    /// `None`, the top untouched, when the reservation cannot hold them or
    /// the operating system refuses the commit. The words are whatever was
    /// there: the caller writes every one.
    #[inline]
    pub(in crate::heap) fn allocate(&mut self, words: usize) -> Option<usize> {
        let end = words.checked_mul(8).and_then(|bytes| self.top.checked_add(bytes))?;
        if end > self.committed {
            self.ensure_committed(end)?;
        }
        let start = self.top;
        self.top = end;
        Some(start)
    }

    /// Commits at least up to `end`: at least double what is committed and
    /// at least `MINIMUM_COMMIT`, so a run of allocations commits a
    /// logarithmic number of times; never past the reservation.
    pub(in crate::heap) fn ensure_committed(&mut self, end: usize) -> Option<()> {
        if end <= self.committed {
            return Some(());
        }
        let wanted = end.max(self.committed.saturating_mul(2)).max(MINIMUM_COMMIT);
        let target = round_up_to_page(wanted)?.min(self.reservation.size());
        if end > target {
            return None;
        }
        self.reservation.commit(self.committed..target).ok()?;
        self.committed = target;
        Some(())
    }

    /// Where the objects lie after a compaction or a move. Both must be
    /// committed.
    pub(in crate::heap) fn set_extent(&mut self, start: usize, top: usize) {
        debug_assert!(FIRST_OBJECT <= start && start <= top && top <= self.committed);
        self.start = start;
        self.top = top;
    }

    /// Gives back the committed memory above `keep`, rounded up to a page
    /// and never below the top. A refused decommit leaves the range
    /// recorded as not committed (`Reservation::decommit`), so the next
    /// growth commits it again either way.
    pub(in crate::heap) fn release_above(&mut self, keep: usize) {
        let Some(keep) = round_up_to_page(keep.max(self.top).max(MINIMUM_COMMIT)) else {
            return;
        };
        if keep < self.committed {
            let _ = self.reservation.decommit(keep..self.committed);
            self.committed = keep;
        }
    }
}

fn round_up_to_page(bytes: usize) -> Option<usize> {
    let page = Reservation::page_size();
    Some(bytes.checked_add(page - 1)? / page * page)
}

/// The words an object of `word_count` words after its header takes: its
/// header and those words, and an overflow word when the header's byte
/// cannot hold the count.
pub(in crate::heap) fn allocation_words(word_count: usize) -> usize {
    1 + word_count + usize::from(word_count >= WORD_COUNT_OVERFLOW)
}

/// The overflow word of an object of `word_count` words after its header.
pub(in crate::heap) fn overflow_word(word_count: usize) -> u64 {
    OVERFLOW_MARK | (word_count as u64 & OVERFLOW_COUNT)
}

/// The object whose allocation begins at word `position`: its header's
/// word index, and the word index at which the next allocation begins.
pub(in crate::heap) fn object_at(words: &[u64], position: usize) -> (usize, usize) {
    let first = words[position];
    if first & OVERFLOW_MARK == OVERFLOW_MARK {
        let header = position + 1;
        (header, header + 1 + (first & OVERFLOW_COUNT) as usize)
    } else {
        (position, position + 1 + Header(first).word_count())
    }
}

/// The words of the object whose header is word `header`: the header and
/// every word after it, the count read from the overflow word when the
/// header carries the mark.
#[inline(always)]
pub(in crate::heap) fn block_range(words: &[u64], header: usize) -> std::ops::Range<usize> {
    let count = match Header(words[header]).word_count() {
        WORD_COUNT_OVERFLOW => (words[header - 1] & OVERFLOW_COUNT) as usize,
        count => count,
    };
    header..header + 1 + count
}
```

  `header.rs`: `pub(super) const WORD_COUNT_OVERFLOW: usize = 255;` (it is private today). Check that `object_at` and `allocation_words` agree with `Header::new`, which stores any count of 255 or more as 255: a count of exactly 255 also needs the overflow word, hence `>=` in `allocation_words` and in `write_object`'s test helper.- [ ] **Step 4: Run the tests, then the per-commit gate.**
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "feat: the object space -- a reservation, a bump pointer, an overflow word and a walk"
```

---

### Task 5: Objects live in the space

The arena goes. Every object is allocated in the space (a block still built as today's `Payload` and copied in -- Task 6 removes the copy), read through `block_range` over `Space::words`, walked by `object_at`, collected by sliding compaction over the live-word bitmap, and moved by the stress mode with a shift. The trigger and the statistics count bytes.

**Files:**
- Create: `crates/ns-runtime/src/heap/compaction.rs` (`ForwardingTable`, `Shift`)
- Modify: `crates/ns-runtime/src/heap.rs` (`objects` → `space: Space`, `object_count: usize`; `next_object_collection` → `next_collection` in bytes; `COLLECTION_FLOOR` in bytes; `slot_of`/`offset_of_slot` go; `block`/`block_mut`, `fill_copied_values`, `pop_activation`, `class_set_application`, `replace_resolved_futures`, `begin_marking`, `drain_marking`, `prune_canonical_symbols`, `compact`, `move_everything`, `forward_everything`, `heap_statistics`, `collector_statistics`, `traced_size`, `reset_collection_limits`, `set_collection_floor`, `push_object` → `place_block`, `allocate_self_referential_guarded`, `out_of_memory`; `Rotation`, the old `ForwardingTable`, `DEAD`, `compact_arena`, `check_block` go; `record_size::block_words`; the `Debug` impl; the tests named below)
- Modify: `crates/ns-runtime/src/heap/space.rs` (the `expect(dead_code)` goes)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`set_collection_floor`'s doc: bytes)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (`set_collection_floor`'s doc: bytes)
- Modify: `crates/ns-runtime/tests/collector.rs` (`object_slots` → `committed_bytes`; the floor `256` → `16 * 1024`)
- Modify: `crates/ns-runtime/tests/image.rs` (the overflow round trip)

**Interfaces:**
- Consumes: Task 4's `Space` and walk; Task 3's offset handles and `Relocation`; Task 2's mark bit and `pointer_ranges`.
- Produces: `HeapStatistics { live_objects, occupied_bytes, committed_bytes }` (`occupied_bytes` is the space's `top - start`, the bytes its objects take); `Interpreter::set_collection_floor(bytes)`; `compaction::{ForwardingTable, Shift}` as listed above.

- [ ] **Step 1: Write the failing tests.** In `heap.rs`'s tests:

```rust
#[test]
fn objects_lie_end_to_end_from_the_first_offset() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    let first = heap.allocate_instance(class, 2);
    let second = heap.allocate_instance(class, 0);
    let key = |object| heap.object_identity_key(object);
    assert_eq!(key(class), space::FIRST_OBJECT);
    assert_eq!(key(first), key(class) + 8 * 2);
    assert_eq!(key(second), key(first) + 8 * 4);
    assert_eq!(heap.heap_statistics().occupied_bytes, 8 * (2 + 4 + 2));
}

#[test]
fn an_object_past_two_hundred_and_fifty_four_words_survives_a_collection_and_a_move() {
    let mut heap = frame_heap();
    let class = heap.allocate_self_referential(Payload::instance([]));
    heap.allocate_instance(class, 3); // dies, so the large ones slide
    let elements: Vec<Value> = (0..300).map(Value::small_integer).collect();
    let array = heap.allocate_array(class, &elements);
    let text: String = std::iter::repeat_n('x', 2_500).collect();
    let string = heap.allocate_string(class, crate::CodePoints::from(text.as_str()));
    let after = heap.allocate_instance(class, 1);
    let mut roots = [Value::object(array), Value::object(string), Value::object(after)];
    let check = |heap: &Heap<'_>, roots: &[Value<'_>; 3]| {
        let array = roots[0].as_object().expect("an array");
        assert_eq!(heap.array_element_count(array), Some(300));
        assert_eq!(heap.array_element(array, 299).and_then(|v| v.as_small_integer()), Some(299));
        assert_eq!(heap.string_size(roots[1].as_object().expect("a string")), Some(2_500));
        assert_eq!(heap.instance_field_count(roots[2].as_object().expect("an instance")), Some(1));
    };
    check(&heap, &roots);
    let [array_root, string_root, after_root] = &mut roots;
    heap.collect_for_test(&mut [array_root, string_root, after_root]);
    check(&heap, &roots);
    #[cfg(debug_assertions)]
    {
        let rooted: Vec<Root> = roots.iter().map(|root| heap.stash(*root)).collect();
        move_bare_heap(&mut heap);
        let moved = [heap.fetch(&rooted[0]), heap.fetch(&rooted[1]), heap.fetch(&rooted[2])];
        check(&heap, &moved);
    }
}

#[test]
fn a_frame_of_two_hundred_and_fifty_registers_takes_an_overflow_word() {
    let mut heap = frame_heap();
    let frame = top_level_frame(&mut heap, 250);
    heap.frame_set_local(frame, 249, Value::small_integer(7));
    assert_eq!(heap.frame_local(frame, 249).as_small_integer(), Some(7));
}

#[test]
fn an_allocation_larger_than_the_first_commit_reads_back_its_last_element() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    let elements = vec![Value::small_integer(1); 100_000];
    let array = heap.allocate_array(class, &elements);
    assert_eq!(heap.array_element(array, 99_999).and_then(|v| v.as_small_integer()), Some(1));
    assert!(heap.heap_statistics().committed_bytes >= 800_000);
}

/// A collection slides every survivor down to the first offset, in the
/// order they were allocated, with nothing between them.
#[test]
fn a_collection_slides_the_survivors_together_from_the_first_offset() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    heap.allocate_instance(class, 5);
    let kept = heap.allocate_instance(class, 2);
    heap.allocate_array(class, &vec![Value::nil(); 400]);
    let last = heap.allocate_instance(class, 0);
    let mut roots = [Value::object(kept), Value::object(last)];
    let [kept_root, last_root] = &mut roots;
    heap.collect_for_test(&mut [kept_root, last_root]);
    let kept = roots[0].as_object().expect("kept");
    let last = roots[1].as_object().expect("kept");
    assert_eq!(heap.object_identity_key(kept), space::FIRST_OBJECT + 8 * 2);
    assert_eq!(heap.object_identity_key(last), heap.object_identity_key(kept) + 8 * 4);
    assert_eq!(heap.heap_statistics().live_objects, 3);
    assert_eq!(heap.heap_statistics().occupied_bytes, 8 * (2 + 4 + 2));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "stale handle")]
fn reading_an_object_handle_kept_across_a_collection_panics() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    heap.allocate_instance(class, 1);
    let kept = heap.allocate_instance(class, 3);
    let mut root = Value::object(kept);
    heap.collect_for_test(&mut [&mut root]);
    // `kept`'s old offset now lies inside the slid survivor; the epoch
    // check refuses it before the read.
    let _ = heap.instance_field(kept, 0);
}

/// A move shifts every object by the same distance and the next move shifts
/// it back, so every offset changes at every move.
#[test]
#[cfg(debug_assertions)]
fn a_move_shifts_every_object_and_the_next_one_shifts_it_back() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    let object = heap.allocate_instance(class, 1);
    let roots = [heap.stash(Value::object(class)), heap.stash(Value::object(object))];
    let keys = |heap: &Heap<'_>| -> Vec<usize> {
        roots.iter().map(|root| heap.object_identity_key(heap.fetch(root).as_object().expect("rooted"))).collect()
    };
    let before = keys(&heap);
    move_bare_heap(&mut heap);
    let shifted = keys(&heap);
    assert!(shifted[0] > before[0]);
    assert_eq!(shifted[1] - shifted[0], before[1] - before[0]);
    move_bare_heap(&mut heap);
    assert_eq!(keys(&heap), before);
}

/// The growth policy in bytes (`docs/design/memory.md`, "The trigger and
/// the growth policy"): a live set under the floor leaves the floor, one
/// above it doubles.
#[test]
fn the_next_limit_is_twice_the_occupied_bytes_or_the_floor() {
    let mut heap = Heap::new();
    heap.collection_floor = 64;
    heap.reset_collection_limits();
    assert_eq!(heap.next_collection, 64);
    let class = heap.allocate_self_referential(Payload::instance([]));
    heap.allocate_instance(class, 10);
    heap.reset_collection_limits();
    assert_eq!(heap.next_collection, 2 * 8 * (2 + 12));
}

/// Compaction gives committed memory back only past twice its next limit,
/// and then no lower than the limit -- the arena's hysteresis, in bytes.
#[test]
fn compaction_gives_memory_back_only_past_twice_the_limit() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential(Payload::instance([]));
    heap.allocate_array(class, &vec![Value::nil(); 1 << 20]);
    let grown = heap.heap_statistics().committed_bytes;
    let mut root = Value::object(class);
    heap.collect_for_test(&mut [&mut root]);
    let after = heap.heap_statistics();
    assert!(after.committed_bytes < grown);
    assert!(after.committed_bytes >= space::FIRST_OBJECT + heap.next_collection);
}
```

  Rewrite in place, keeping what each pins: `a_block_is_its_header_its_class_word_and_its_fields` (read the block through `heap.block(object)`), `a_digit_shaped_like_an_object_word_is_never_forwarded` (the digit is `Word::with_object_offset(filler.offset).bits()`; compare offsets through `object_identity_key`; the range check reads `heap.space.top()`), `a_move_gives_every_slot_a_new_index_and_rewrites_what_names_it` (its name: `a_move_gives_every_object_a_new_offset_and_rewrites_what_names_it`), `collecting_reclaims_what_no_root_reaches` (assert `heap_statistics().live_objects` 4 then 2), `reset_collection_limits_reads_each_arenas_own_length` and `compaction_never_shrinks_an_arena_below_its_next_limit` (deleted: the two new tests above replace them), `an_object_slot_is_one_boxed_block_of_words` (deleted: Task 3's `a_handle_and_an_optional_handle_are_one_word` and Task 4's tests replace it). In `tests/image.rs`, beside `a_large_integer_value_round_trips` and built on the same helpers (`load`, `snapshot_one`, `class_reference`, `expect_ok`):

```rust
/// An `Array` past 254 elements is decoded into an object with an overflow
/// word, and its last element survives the round trip.
#[test]
fn an_array_past_two_hundred_and_fifty_four_elements_round_trips() {
    let source = indoc! {"
        class X = Object new (
          | big = self filled. |
        ) (
          filled = (
            | array = Array new: 300. |
            1 to: 300 do: [:index | array at: index put: index].
            ^array
          )
          public lastText = ( ^(big at: 300) printString )
          public sizeText = ( ^big size printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        snapshot_one(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    for (selector, expected) in [("lastText", "300"), ("sizeText", "300")] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            let Some(string) = mutator.fetch(&answer).as_object() else {
                panic!("{selector}: expected a string")
            };
            assert_eq!(
                mutator.heap().str_value(string).as_deref(),
                Some(expected),
                "{selector}"
            );
        });
    }
}
```

In `tests/collector.rs`: `object_slots` → `committed_bytes`; `set_collection_floor(256)` → `set_collection_floor(16 * 1024)`, its comment saying bytes.
- [ ] **Step 2: Run them to verify they fail.** `cargo nextest run -p ns-runtime` -- expected: `space`, `occupied_bytes`, `next_collection` do not exist on the heap.
- [ ] **Step 3: Implement.**
  - **`compaction.rs`:**

```rust
//! The relocations a collection and a move forward every handle through
//! (`Relocation`, `heap.rs`).

use std::num::NonZeroU64;
use std::ops::Range;

use crate::heap::space::FIRST_OBJECT;
use crate::heap::Relocation;

/// Where a collection slides each survivor: one bit per word of the space
/// for every word of a surviving object, its overflow word included, and
/// per chunk of 64 words the live words before the chunk. A survivor's new
/// position is the first object's plus the live words before it -- one
/// table entry, a mask and a `popcount` per handle, in about 3% of the
/// space for as long as the collection runs (decided with the user on
/// 2026-10-03, against a binary search of a sorted list and a dense table
/// half the size of the space; `docs/design/memory.md`, "Compaction and
/// forwarding").
pub(crate) struct ForwardingTable {
    chunks: Vec<Chunk>,
}

/// The bits and the prefix side by side, so one lookup reads one cache line.
#[derive(Clone, Copy, Default)]
struct Chunk {
    live: u64,
    before: u64,
}

impl ForwardingTable {
    pub(in crate::heap) fn with_words(words: usize) -> ForwardingTable {
        ForwardingTable { chunks: vec![Chunk::default(); words.div_ceil(64)] }
    }

    /// Marks the words of one surviving object's allocation as live.
    pub(in crate::heap) fn set_live(&mut self, words: Range<usize>) {
        let mut word = words.start;
        while word < words.end {
            let bit = word % 64;
            let span = (64 - bit).min(words.end - word);
            let mask = if span == 64 { u64::MAX } else { ((1u64 << span) - 1) << bit };
            self.chunks[word / 64].live |= mask;
            word += span;
        }
    }

    /// Fills in each chunk's prefix; answers the live words in all.
    pub(in crate::heap) fn summarize(&mut self) -> usize {
        let mut total = 0u64;
        for chunk in &mut self.chunks {
            chunk.before = total;
            total += u64::from(chunk.live.count_ones());
        }
        total as usize
    }

    fn is_live(&self, word: usize) -> bool {
        self.chunks.get(word / 64).is_some_and(|chunk| chunk.live >> (word % 64) & 1 == 1)
    }

    /// The new word index of live word `word`.
    pub(in crate::heap) fn destination(&self, word: usize) -> usize {
        let chunk = self.chunks[word / 64];
        let below = chunk.live & ((1u64 << (word % 64)) - 1);
        FIRST_OBJECT / 8 + (chunk.before + u64::from(below.count_ones())) as usize
    }
}

impl Relocation for ForwardingTable {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        let word = (offset.get() / 8) as usize;
        debug_assert!(
            self.is_live(word),
            "a handle to a collected object survived marking: a root the tracer does not know about"
        );
        match NonZeroU64::new(8 * self.destination(word) as u64) {
            Some(destination) => destination,
            None => unreachable!("internal: a destination lies past the first object"),
        }
    }
}

/// The move stress mode's relocation: every object shifts by one distance,
/// so every offset changes and a handle the tracer missed both carries an
/// old epoch and names a different place.
#[cfg(debug_assertions)]
pub(crate) struct Shift {
    pub(in crate::heap) from: u64,
    pub(in crate::heap) to: u64,
}

#[cfg(debug_assertions)]
impl Relocation for Shift {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        match NonZeroU64::new(offset.get() - self.from + self.to) {
            Some(moved) => moved,
            None => unreachable!("internal: a shifted object lies past the first object"),
        }
    }
}
```

    Add unit tests in `compaction.rs`: `set_live` over a range crossing a chunk boundary sets exactly those bits; `destination` of the first live word is `FIRST_OBJECT / 8`; of a word after a dead gap equals the live words before it plus `FIRST_OBJECT / 8`.
  - **`heap.rs`, the store:** `space: Space` (built by `Space::new(DEFAULT_RESERVATION)`, `unwrap_or_else(|_| out_of_memory(DEFAULT_RESERVATION))`), `object_count: usize`, `next_collection: usize`, `collection_floor: usize`; `const COLLECTION_FLOOR: usize = 512 << 10;` with its doc in bytes ("4,096 of the arena's slots at a 128-byte average; `bench_loop.ns` and `bench_send.ns` should never collect"). Delete `slot_of`/`offset_of_slot`, `Rotation`, `DEAD`, the old `ForwardingTable`, `compact_arena`, `check_block`.

```rust
/// Running out of memory: the reservation is used up or the operating
/// system refused a commit. Aborts, as a failed allocation of the Rust
/// allocator does, so that no allocation carries a `Result` (decided with
/// the user on 2026-10-03; a Newspeak-level error is `docs/OPEN-WORK.md`'s).
#[cold]
fn out_of_memory(bytes: usize) -> ! {
    std::alloc::handle_alloc_error(
        std::alloc::Layout::from_size_align(bytes, 8)
            .unwrap_or_else(|_| std::alloc::Layout::new::<u64>()),
    )
}
```

  - **Allocation:** `push_object(block)` becomes `place_block(&mut self, block: Box<[u64]>) -> ObjectReference<'gc>`: `let word_count = block.len() - 1;` allocate `allocation_words(word_count)` (or `out_of_memory`), write `overflow_word(word_count)` first when `word_count >= WORD_COUNT_OVERFLOW`, then the block's words; `object_count += 1`; `if self.space.top() - FIRST_OBJECT >= self.next_collection { self.request_collection(); }`; answer the handle of the header's byte offset. `allocate_self_referential_guarded` places its block, then overwrites the placed class word with `Word::with_object_offset` of the new handle (its own class). `record_size::block_words(field_count)` answers `space::allocation_words(field_count + 1)`, so the decoder's first pass counts the overflow word.
  - **Access:** `block`/`block_mut` read `let words = self.space.words(); let range = block_range(words, header_index); &words[range]` with `header_index = (object.offset.get() / 8) as usize`; `fill_copied_values` and `pop_activation` destructure `let Heap { space, stack, .. } = self;` and index `space.words_mut()` through `block_range`.
  - **Walks:** no walk collects the headers into a `Vec` first, which would allocate per collection; each walk loops `let mut position = self.space.start() / 8; while position < self.space.top() / 8 { let (header, next) = object_at(self.space.words(), position); …; position = next; }`. `class_set_application` compares the class word's `Word::object_offset()` with `class.offset` and restamps the guard; `replace_resolved_futures` keys its map by the header's byte offset; `forward_everything` traces each block (`trace_block(&mut words[block_range(words, header)], forwarder)`); the debug no-mark assertion of `begin_marking` walks the same way.
  - **Marking:** `drain_marking` as in Task 2 over the space: pop an offset, `header_index = offset / 8`, test and set bit 33 in `words[header_index]`, take `block_range`, and for each index in each `pointer_ranges` range (shifted by the block's start) push every target offset whose header is unmarked. `prune_canonical_symbols` reads the mark bit through `self.header(object)`.
  - **`compact(&mut self) -> Forwarder<ForwardingTable>`:** advance the epoch as today; then

```rust
let start = self.space.start() / 8;
let top = self.space.top() / 8;
let mut table = ForwardingTable::with_words(top);
let mut live_objects = 0;
let words = self.space.words();
let mut position = start;
while position < top {
    let (header, next) = object_at(words, position);
    if Header(words[header]).marked() {
        table.set_live(position..next);
        live_objects += 1;
    }
    position = next;
}
let live_words = table.summarize();
// Slide: a survivor's destination is never above where it lies, so
// copying it down never overwrites an allocation the walk has yet to read.
let words = self.space.words_mut();
let mut position = start;
while position < top {
    let (header, next) = object_at(words, position);
    if Header(words[header]).marked() {
        let destination = table.destination(position);
        words.copy_within(position..next, destination);
        let moved = destination + (header - position);
        words[moved] = Header(words[moved]).with_marked(false).0;
    }
    position = next;
}
self.space.set_extent(FIRST_OBJECT, FIRST_OBJECT + 8 * live_words);
self.object_count = live_objects;
let limit = collection_limit(self.collection_floor, 8 * live_words);
if self.space.committed() > FIRST_OBJECT + limit.saturating_mul(2) {
    self.space.release_above(FIRST_OBJECT + limit);
}
```

    then build the `Forwarder` over the table and `forward_everything`, which now walks the slid objects.
  - **`move_everything` (debug):** `let from = self.space.start(); let length = self.space.top() - from; let to = if from == FIRST_OBJECT { FIRST_OBJECT + 8 * (2 + to_epoch as usize % 31) } else { FIRST_OBJECT };` -- `ensure_committed(to + length)` (or `out_of_memory`), `set_extent(from, from.max(to) + length)` so `words_mut` lends both ranges, `copy_within(from / 8..(from + length) / 8, to / 8)`, `set_extent(to, to + length)`, then forward through `Shift { from: from as u64, to: to as u64 }`. Its doc: every object shifts by one distance, up from the first offset and back down to it, so every offset changes at every move (`docs/design/memory.md`, "The move stress mode").
  - **Statistics and limits:** `heap_statistics` answers `live_objects: self.object_count`, `occupied_bytes: self.space.top() - self.space.start()`, `committed_bytes: self.space.committed()`; `collector_statistics().live_objects` is `object_count`; `traced_size` counts `object_count`; `reset_collection_limits` sets `next_collection = collection_limit(self.collection_floor, self.space.top() - self.space.start())`; `set_collection_floor(floor)` sets both in bytes. `HeapStatistics`' doc describes the three fields.
  - **`space.rs`:** delete the `expect(dead_code)` attribute.
- [ ] **Step 4: Run the tests, the per-commit gate, and `cargo nextest run --workspace --release`.** Then, before committing, run the move and collect stress modes over the runtime crate alone as a smoke check (the full gates are Task 9's): `NS_NEXT_STRESS_MOVES=1 cargo nextest run -p ns-runtime` and `NS_NEXT_STRESS_COLLECT=1 cargo nextest run -p ns-runtime` (PowerShell: `$env:NS_NEXT_STRESS_MOVES='1'; cargo nextest run -p ns-runtime; Remove-Item Env:NS_NEXT_STRESS_MOVES`). A failure there is this task's to fix.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: objects live in the space, collected by sliding compaction over a live-word bitmap"
```

---

### Task 6: Every allocation writes its object in place

The per-object Rust allocation goes: `Payload`, `into_block` and `place_block` are deleted, and each allocator writes its header, class word and fields straight into the space -- a closure's copied values straight from the registers, a string's units straight from its `CodePoints`. `fill_placeholder_class` fills from an array on the stack and loses its fallback, the one place a block was rebuilt beside its placeholder; a class placeholder of the wrong size is now a `DecodeError`, which a valid first pass never produces (`OPEN-WORK.md`, "Next").

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`Payload` goes; `pack_code_points(text: &CodePoints, words: &mut [u64]) -> (Format, u8)` and `code_point_field_count(text: &CodePoints) -> usize` beside `pack_string`; the module doc)
- Modify: `crates/ns-runtime/src/heap.rs` (`allocate_block`; every `allocate_*` and the frame and class constructors write in place; `class_payload` → `class_fixed_words(&self, class: NewClass<'gc>) -> [Word; class_field::COUNT]`; `frame_payload` goes; `fill_placeholder_class` answers `bool`; the tests that compared a fill with a payload)
- Modify: `crates/ns-runtime/src/image/decode.rs` (a `false` from `fill_placeholder_class` is `DecodeError::UnexpectedEndOfFile`, as a refused string fill is; `decode_value`'s doc comment loses the 80-byte class-field line)

**Interfaces:**
- Consumes: Task 5's space and `place_block`'s bookkeeping (count, trigger, overflow word).
- Produces: `fn allocate_block(&mut self, class: Word, guard: DispatchId, format: Format, unused: u8, negative: bool, field_count: usize) -> (ObjectReference<'gc>, Range<usize>)` -- the header (with the overflow word when needed) and the class word written, every field `Word::NIL`, the range the fields' word indices in `self.space.words()`; `Heap::fill_placeholder_class(..) -> bool`.

- [ ] **Step 1: Write the failing tests.** In `heap.rs`'s tests:

```rust
/// A class placeholder the first pass sized wrongly is refused and left as
/// it was: the space cannot resize an object in place, and a valid first
/// pass never sizes a class record otherwise.
#[test]
fn a_class_placeholder_of_the_wrong_size_is_refused_and_left_alone() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "Point");
    let placeholder = heap.allocate_placeholder(class, 3);
    let application = heap.class_application(class).expect("a class");
    let filled = heap.fill_placeholder_class(
        placeholder,
        class,
        0,
        NewClass { application, enclosing: None, declaring_class: None, superclass: None, builtin: None },
    );
    assert!(!filled);
    assert!(!heap.is_class(placeholder));
    assert_eq!(heap.instance_field_count(placeholder), Some(3));
}
```

  and rewrite the four "filled in place is the block a payload builds" tests (`an_instance_or_array_filled_in_place_is_the_block_a_payload_builds`, `a_string_filled_in_place_is_the_block_a_payload_builds`, `a_closure_filled_in_place_is_the_block_a_payload_builds`, `a_frame_filled_in_place_is_the_frame_allocate_frame_builds`) to compare the placeholder's block with the block of the same object made by `allocate_instance`/`allocate_array`/`allocate_string`/`allocate_closure`/`allocate_returned_top_level_frame`, renaming each "…_is_the_block_its_allocation_writes". Every test that built an object through `Payload` (`allocate_self_referential(Payload::instance([]))`, `allocate_object(class, Payload::…)`) builds it through the typed allocations instead; `allocate_self_referential` (test-only) takes a field count.
- [ ] **Step 2: Run them to verify they fail.** Expected: `fill_placeholder_class` answers `()`.
- [ ] **Step 3: Implement.** `allocate_block` allocates `allocation_words(1 + field_count)` (or `out_of_memory`), writes the overflow word when needed, the header (`Header::new(guard, format, unused, 1 + field_count).with_negative(negative)`), the class word, and `Word::NIL` into every field; counts the object and checks the trigger as `place_block` did; answers the handle and the fields' range. Each allocator:
  - `allocate_instance`, `allocate_placeholder`: the block as allocated.
  - `allocate_array`: in a debug build `self.store` each element first (the check), then write `element.word().bits()` into the range -- the pattern `push_activation` uses.
  - `allocate_string`, `allocate_symbol`: `code_point_field_count(&text)` fields, then `pack_code_points` into them and the unused units into the header (`allocate_block` takes `unused` from a first call of a `fn string_shape(text: &CodePoints) -> (Format, u8, usize)` beside the packers, so the header is written once).
  - `allocate_large_integer`, `allocate_float`: `Format::SixtyFourBit`, the digits or `to_bits` into the range, the sign through `negative`.
  - `allocate_closure`, `allocate_closure_from_registers`: `closure_fixed_words` then the copied values; for the registers, `let Heap { space, stack, .. } = self;` and copy each register's word from `stack.values` into the range.
  - `allocate_class`, `allocate_self_referential_class`: `class_fixed_words(record)` into the range.
  - `allocate_frame`: `frame_fixed_words(fields)` then the registers; the nils after them are already written.
  - `fill_placeholder_class`: `let fields = self.class_fixed_words(record);` if `self.fields(object).len() != class_field::COUNT` answer `false` and change nothing; otherwise copy them in, `rewrite_placeholder_header`, answer `true`. Its doc drops the "laid out beside the placeholder" sentence.
  - Delete `Payload` (and its doc's list of builders) from `object.rs`, keeping its format paragraph on the module doc; delete `place_block`.
  - `decode.rs`: at the call, `if !heap.fill_placeholder_class(*object, class, identity_hash, record) { return Err(DecodeError::UnexpectedEndOfFile); }`, commented like the string fill's.
- [ ] **Step 4: Run the tests, the per-commit gate, and `cargo nextest run --workspace --release`.** Then the same two stress smoke runs as Task 5.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "perf: every allocation writes its object in place in the space"
```

---

### Task 7: Measured against the tree the branch started from

**Files:** none in the repository until Task 8 records the numbers.

- [ ] **Step 1:** Follow the `measuring-performance` skill and `execution.md`'s protocol ("What the object representation moved", "The Are We Fast Yet benchmarks"): three pinned release builds of `master` at the branch point (`d3e9f6fc`) and of the branch head, the same explicit `RUSTFLAGS`, the sixteen benchmarks and the microbenchmarks under `goldens/tool/bench/` (the four that are not in the tree written out from `execution.md`, "What spec-conformant lookup moved"), pooled. Also measure Task 3's head against `master` on the same sources if Task 3 changed a pinned size, so the handle's own cost is separated from the space's.
- [ ] **Step 2:** Peak working set, as `memory.md`'s "The interim cost" measures it (`PeakWorkingSetSize` through `GetProcessMemoryInfo` on the open handle): `run bench_loop.ns`, and `bench --iterations 10` of `bench_loop.ns`, `bench_send.ns` and `bench_block.ns`, both trees. `--collector-statistics` on `bench_loop.ns` and `bench_send.ns` must report 0 collections on the branch; if not, the 512 KiB floor is too low for them -- report it before Task 8.
- [ ] **Step 3:** Record, per program, the change and its spread; profile the two largest regressions or the two largest gains and name where the time went (the bump allocation, the block lookup through the prefix, the slide, the forwarding lookups). Anything worth fixing that this branch does not fix goes into `docs/OPEN-WORK.md` in Task 8.

---

### Task 8: The documentation describes the built state

**Files:**
- Modify: `docs/design/memory.md`: "Planned: the object space" becomes the built description in the present tense (its own section, "The object space", placed after "The memory layer"), with the forwarding table, the shift of the move stress mode, the abort on exhaustion and the deferred no-access protection, each with its reason; "The representation" (the `Heap` sketch: `space: Space`, a handle a `NonZeroU64` byte offset, 8 bytes in a release build), "A handle is arena-relative" (now space-relative: the offset; `object_identity_key` hands out the offset), "The seam" and "Known rough edges in the seam" (no copy-out claim that the space changed), "What the collector traces" (`pointer_ranges`), "What an object costs" (no slot: an object costs its words, 8 more with an overflow word past 254), "The amplification bound of a rejected load" (recompute from `decode_value`'s doc comment: a metaclass record of 9 bytes becomes a class object of 80 bytes, 8.9x; with the decoder's tables -- an 8-byte handle per record in a release build, 16 in a debug one, and the first pass's 8-byte size -- about 10.7x release and 11.6x debug; the root list stays 8x and 16x; so the peak is about 10.7x of the graph bytes in a release build and 16x in a debug one, plus at most 4x of the symbol section -- confirm each figure against the code before writing it), "The shape: mark, compact, forward", "Marking" (the header's bit), "Compaction and forwarding" (slide, the live-word bitmap, the hysteresis in bytes), "The trigger and the growth policy" (bytes, the 512 KiB floor), "Weak references" (`symbol_of` keyed by offset), "Payloads belong to the Rust allocator" (now history: rewrite as what owning the memory gave), "The memory layer" (the heap is its user; `prefix`), "Planned: generations" (unchanged in substance; its "Would change" names the built space), "Decisions" (the three of 2026-10-03 from this plan's header).
- Modify: `docs/design/object-model.md` ("Planned with road step 3b: handles" becomes built; "Arena allocation and handles"; "The object layout"'s mention of the overflow word's first use), `docs/design/image-format.md` (the amplification figures), `docs/design/execution.md` (Task 7's numbers as "What the object space moved"), `docs/design/README.md` (the map's status cells for memory)
- Modify: `CLAUDE.md`: the crate list's heap sentence (one space of variably sized objects in `ns-memory`'s reservation, byte-offset handles, a mark-and-slide collector); `ns-memory`'s "Nothing uses it yet" goes; "An image is untrusted input" gets the new figures; "A handle means nothing against a `Heap` that did not mint it" describes a 64-bit byte offset.
- Modify: `docs/OPEN-WORK.md`: "Next" names road step 4, generations; remove what this branch retired (the fallback, the index-keyed structures, the per-object allocations); add: a Newspeak-level out-of-memory error; the debug no-access protection of vacated pages, for when generated code dereferences addresses (road step 8); whatever Task 7 leaves open; update "Step 3, the object space" in the road to built.
- Modify: `docs/PROGRESS.md`: the milestone, and "Where the next session starts": road step 4.

- [ ] **Step 1:** Rewrite each section named above; every statement in the present tense, no "planned" for what is built, no "historical" label on anything.
- [ ] **Step 2:** Check `git diff --stat` is Markdown only, then commit (no gates for a Markdown-only commit):

```bash
git add docs CLAUDE.md
git commit -m "docs: the object space is built, and road step 4 is next"
```

---

### Task 9: The end-of-batch gates

- [ ] **Step 1:** Run the `finishing-a-batch` skill: the four stress modes (moves, caches, collection, every instruction wide), `run-corpus` plain and under stress, the benchmarks profile, `cargo nextest run -p ns-lsp`, `verifyPlugin`, `cargo clippy -p ns-memory` for both cross targets, and the check that a fresh session can start from the documentation alone.
- [ ] **Step 2:** Fix what they find, each fix its own commit through the per-commit gate.
- [ ] **Step 3:** Request the final whole-branch review (on Fable, per the user's standing instruction) before merging into `master`.
