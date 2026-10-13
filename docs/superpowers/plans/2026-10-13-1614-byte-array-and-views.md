# `ByteArray`, `StringBuilder` and the views — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sub-project 5 of the kernel completed to Primordial Soup's core API: a builtin `ByteArray` with 32-byte-aligned data, the sequence protocol as the kernel mixin `IndexedCollection`, UTF-8 on `String`, `StringBuilder`, the ten typed arrays and `DataView`.

**Architecture:** `ByteArray` is the only new builtin: a new heap format, `AlignedBytes`, whose data every mover realigns, and a new image record. Everything else is ordinary Newspeak in `Kernel.ns` over a `ByteArray` slot, reaching its bytes through mirror leaves that take the `ByteArray` and carry width, kind, byte order and indexing in the leaf's one-byte payload. The bootstrap learns two things it cannot do today: a kernel that declares classes that are not builtins, and a builtin whose superclass clause applies a kernel mixin (`Collection <: IndexedCollection`).

**Tech Stack:** Rust 1.98.0, Newspeak core library (`crates/ns-runtime/core-library/`), goldens under `goldens/`, `cargo-nextest`.

**Spec:** `docs/design/core-library.md`, "Planned: sub-project 5, `ByteArray`, `StringBuilder` and the views" (designed with the user on 2026-10-12), including its closing "What the plan must handle". Read it before Task 1. The decisions there are settled; this plan does not reopen them.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("Working in this repository") run before each commit. The end-of-batch gates (five stress modes, `run-corpus` plain and under stress, the benchmarks profile, `verifyPlugin`) run once, after Task 11, by the controller (`finishing-a-batch` skill); an implementer does not run them.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01BcChjJmpsEYtz6GZ8wwPBo`. Never change the git identity.
- Work on the branch `feature/byte-array-and-views` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. The controller creates it from `master` before Task 1.
- Names are written out (`CLAUDE.md`, "Names are written out"): `index`, `position`, `destination`, `big_endian`, never `idx`, `pos`, `dst`, `be`.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). An existing golden's `.toml` does not change in this plan; if one would, stop and report. A new golden's `.toml` is written from the specification and the design, before the code runs it, never copied from a run.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt <the files edited>`; never at the repository root.
- Every member of a shipped module, public, protected or private, has a `///` comment to the standard in `CLAUDE.md` ("Every member of a shipped core-library module is documented"): what it does and answers, each argument, every Error it signals with its text, order, identity, copying and mutation. No other implementation is named. `crates/ns-runtime/tests/core_library_documentation.rs` enforces the presence.
- Any change to `Kernel.ns`, `PlatformFactory.ns`, a `BuiltinClass` or the `MIRROR` table changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- **`Primitive` stays two bytes**: `resolve.rs`'s `a_resolution_stays_the_size_the_hot_path_was_measured_at` runs only in a release build, so every task that adds a `Primitive` variant runs `cargo nextest run -p ns-runtime --release -E 'test(a_resolution_stays_the_size)'`.
- **The `MIRROR` table holds at most 256 rows** (`builtins/primitive.rs`, the `const _` assert after the table); 164 are used today and this plan adds 69, so 233. New rows are appended at the end, never inserted.
- A leaf checks every operand before it writes anything, and declines (answers `None`) only when the receiver or the subject has the wrong shape, so the failure block runs; every other refusal is an `ArgumentError` the leaf raises itself through `NewspeakError::argument_error(selector, receiver, argument, message)` with the reported selector.
- No `unsafe` in `ns-runtime`: the bytes of an `AlignedBytes` object are read and written through its `u64` words by shifts, never by reinterpreting the slice.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- Goldens are `main: platform args:` modules printing through `platform transcript`, formatted, commented with `///`; each new golden directory's files follow their neighbours under `goldens/library/`. Every golden runs under both lowerings automatically.

## Review Focus

1. **A mover that skips the realignment leaves the data at the old offset within the block: wrong bytes at exit status 0.** All three movers -- the scavenger's `copy`, the compaction's slide and the move stress mode -- must be exercised with the object starting at each of the four word positions modulo 4 and landing at each; pinned by Task 1's `an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position` and Task 3's stress golden run.
2. **A user subclass of `Array` or `ByteArray` still falls through to `Object`'s and `Collection`'s methods** now that `super` reaches `IndexedCollection` first; pinned by the unchanged `goldens/library/builtin-subclass/` and Task 4's new `a_subclass_of_byte_array_falls_to_objects_methods`.
3. **A `StringBuilder` fed many multi-byte code points across several growths answers the right `size` and `asString`**; pinned by Task 7's golden `a_builder_grows_across_multi_byte_code_points`.
4. **The extremes of every element kind**: `Int64Array`'s and `Uint64Array`'s limits read back as `LargeInteger`s and are written from them, one past each limit refused, `Float32Array` rounding to nearest even, overflow to an infinity, NaN kept; pinned by Task 8's goldens.
5. **A crafted image gives a view a `bytes` slot that is no `ByteArray`, or a `StringBuilder` counts that disagree with its data**: every access is an `ArgumentError`, never a panic; pinned by Task 10's image tests.

## The leaves, in one table

Every leaf takes the `ByteArray` as its subject. "Row" is the mirror selector; "reported" is what an error names.

| Task | Rows | Reported | Family |
|---|---|---|---|
| 4 | `byteArrayClass:new:ifFail:`, `byteArraySize:ifFail:`, `byteArray:at:ifFail:`, `byteArray:at:put:ifFail:` | `new:`, `size`, `at:`, `at:put:` | `Primitive::Bytes(BytePrimitive)` |
| 5 | `byteArray:copyFrom:to:ifFail:`, `byteArray:replaceFrom:to:with:startingAt:ifFail:`, `byteArray:concatenate:ifFail:`, `byteArray:indexOf:startingAt:ifFail:`, `byteArray:lastIndexOf:startingAt:ifFail:`, `byteArray:startsWith:ifFail:`, `byteArray:endsWith:ifFail:` | `copyFrom:to:`, `replaceFrom:to:with:startingAt:`, `,`, `indexOf:startingAt:`, `lastIndexOf:startingAt:`, `startsWith:`, `endsWith:` | `Bytes` |
| 6 | `stringAsUtf8:ifFail:`, `stringClass:fromUtf8:from:to:ifFail:`, `stringUtf8Size:ifFail:`, `byteArray:at:putCodePoint:ifFail:` | `asUtf8`, `fromUtf8:from:to:`, `utf8Size`, `addCodePoint:` | `String(StringPrimitive)` for the first three, `Bytes` for the last |
| 8 | `byteArray:<kind>ElementAt:ifFail:` and `byteArray:<kind>ElementAt:put:ifFail:` for the nine kinds `int8`, `int16`, `uint16`, `int32`, `uint32`, `int64`, `uint64`, `float32`, `float64` (18 rows; `Uint8Array` uses `byteArray:at:ifFail:`) | `at:`, `at:put:` | `Primitive::Element(ElementAccess)` |
| 9 | `byteArray:int8At:ifFail:`, `byteArray:uint8At:ifFail:` and their `put:` rows (4); `byteArray:littleEndian<Width>At:ifFail:`, `byteArray:bigEndian<Width>At:ifFail:` and their `put:` rows for the eight widths `Int16`, `Uint16`, `Int32`, `Uint32`, `Int64`, `Uint64`, `Float32`, `Float64` (32) | `int8At:`, `int16At:put:`, … (the `DataView` selector) | `Element` |

## The error texts

Existing texts are reused verbatim where the design says "in `Array`'s texts": the index texts of `collections.rs`'s `out_of_range` and `not_an_integer`, the range texts of `checked_range`, the start texts of `text.rs`'s `search`, and `Array new:`'s three texts with "an Array of {n} elements" replaced by "a ByteArray of {n} bytes". The new ones, `{…}` filled by `Heap::describe_argument` unless it is a number:

| Where | Text |
|---|---|
| a byte value | `{selector} requires an Integer from 0 to 255, got {value}` |
| an element value of an integer kind | `{selector} requires an Integer from {min} to {max}, got {value}` |
| an element value of a float kind | `{selector} requires a Number, got {value}` |
| a `DataView` position | `{selector} position {position} out of range: valid positions are 1..{size + 1 - width} (size {size})` |
| a view's source | `{selector} requires a ByteArray, got {argument}` |
| `on:` of a typed array | `on: requires a ByteArray whose size is a multiple of {width}, got a ByteArray of {size}` |
| a byte order | `{selector} requires #littleEndian or #bigEndian, got {argument}` |
| invalid UTF-8 | `fromUtf8: invalid UTF-8 at byte {position}` (1-based, the first byte of the bad sequence) |
| `StringBuilder>>add:` | `add: requires a String, got {argument}` |
| `addCodePoint:` | `addCodePoint: requires a Unicode scalar value, got {argument}` |
| a view over a crafted slot | `{selector} found no ByteArray under the view, got {bytes}` |

---

### Task 1: The `AlignedBytes` format and the movers

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs` (`Format`, `from_code`, `unused_limit`, the module doc, the field test)
- Modify: `crates/ns-runtime/src/heap/object.rs` (the module doc, `pointer_ranges`, new layout helpers, tests)
- Modify: `crates/ns-runtime/src/heap/scavenge.rs` (`copy`, after the `copy_within`)
- Modify: `crates/ns-runtime/src/heap.rs` (allocation and accessors, `compact`'s slide, `move_everything`, `describe`, tests)

**Interfaces:**
- Produces, in `object.rs` (`pub(in crate::heap)`):

```rust
/// The words an `AlignedBytes` object holds ahead of its data, so that the
/// data can begin at any of the four word positions of a 32-byte line.
pub(in crate::heap) const ALIGNMENT_RESERVE: usize = 3;
/// The data's alignment in words: 32 bytes, AVX2's width.
pub(in crate::heap) const ALIGNMENT_WORDS: usize = 4;
/// The field count of a `ByteArray` of `size` bytes.
pub(in crate::heap) fn byte_array_field_count(size: usize) -> usize;
/// The absolute index of the first data word of the `AlignedBytes` object
/// whose header is at `header`: the first multiple of four at or after its
/// first field word. Computed, never stored.
pub(in crate::heap) fn aligned_data_start(header: usize) -> usize;
/// Moves the data of the `AlignedBytes` object just copied, header and all,
/// from header index `old_header` to `new_header`, from the old block's
/// alignment to the new block's, within the new block.
pub(in crate::heap) fn realign(words: &mut [u64], old_header: usize, new_header: usize);
/// Byte `index` of a run of data words, little-endian within each word.
pub(in crate::heap) fn data_byte(data: &[u64], index: usize) -> u8;
pub(in crate::heap) fn set_data_byte(data: &mut [u64], index: usize, byte: u8);
```

- Produces, on `Heap` (the public ones copy out; the `pub(crate)` lenders serve leaves that read or write in place):

```rust
pub fn allocate_byte_array(&mut self, class: ClassReference<'gc>, bytes: &[u8]) -> ObjectReference<'gc>;
pub(crate) fn allocate_zeroed_byte_array(&mut self, class: ClassReference<'gc>, size: usize) -> ObjectReference<'gc>;
pub fn is_byte_array(&self, object: ObjectReference<'gc>) -> bool;
pub fn byte_array_size(&self, object: ObjectReference<'gc>) -> Option<usize>;
pub fn byte_array_bytes(&self, object: ObjectReference<'gc>) -> Option<Vec<u8>>;
pub fn byte_array_at(&self, object: ObjectReference<'gc>, index: usize) -> Option<u8>;  // 0-based
pub fn byte_array_set(&mut self, object: ObjectReference<'gc>, index: usize, byte: u8) -> bool;
pub(crate) fn with_byte_array<T>(&self, object: ObjectReference<'gc>, read: impl FnOnce(ByteArrayData<'_>) -> T) -> Option<T>;
pub(crate) fn with_byte_array_mut<T>(&mut self, object: ObjectReference<'gc>, write: impl FnOnce(ByteArrayDataMut<'_>) -> T) -> Option<T>;
```

  `ByteArrayData` / `ByteArrayDataMut` (in `object.rs`, re-exported `pub(crate)` from `heap.rs` like the other seam types) wrap the data words and the size: `size()`, `byte(index)`, `bytes(range) -> Vec<u8>`, and on the mutable one `set_byte(index, byte)`, `copy_within(source: Range<usize>, destination: usize)` (memmove: right when the ranges overlap), `write(destination, &[u8])`. Task 8 adds the width reads and writes to them.

- [ ] **Step 1: Write the failing heap tests** in `heap.rs`'s `mod tests`, using the bare-heap helpers already there (`Heap::new`, `allocate_self_referential`, `collect_for_test`, `scavenge_bare`, `old_instance_class`, `move_bare_heap`, `rooted`):
  - `a_byte_array_reads_back_its_bytes_and_its_size`: sizes 0, 1, 7, 8, 9, 33; `byte_array_size`, `byte_array_bytes`, a header of format `AlignedBytes` with `unused` = `(8 - size % 8) % 8`, and `aligned_data_start(header) % 4 == 0` with the data inside the block.
  - `a_new_byte_array_is_zero_filled` (`allocate_zeroed_byte_array`, size 40).
  - `an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position`: for each `filler` in `0..4` and each mover, allocate an instance of `filler` fields first (a dead one, so the next object's header index moves by one word each time), then a `ByteArray` of 37 bytes `0..37`, root it, and run: `scavenge_bare(.., false, ..)` twice (survivor to survivor, so both survivor spaces are met), `scavenge_bare(.., true, ..)` (promotion), `collect_for_test` (the slide), and in a debug build `move_bare_heap` 62 times (every distance `2..=32` of the stress mode, both directions). After each step assert the bytes are `0..37` and `aligned_data_start` of the object's header index is a multiple of 4. Collect, over the whole test, the set of `(old residue, new residue)` pairs the scavenger and the slide met, and assert every one of the 16 occurs at least once for at least one mover, so the test cannot silently stop covering a position.
  - `a_byte_array_holds_no_pointer`: `pointer_ranges` on an `AlignedBytes` block names only the class word (extend `pointer_ranges_name_the_class_word_and_each_formats_pointer_fields` in `object.rs`).
  - `describe_names_a_byte_array_by_its_size`: "a ByteArray of 3".
  - In `header.rs`'s test, a header of format `AlignedBytes` with 7 unused reads back both.

Run: `cargo nextest run -p ns-runtime --lib -E 'test(byte_array) | test(aligned)'`
Expected: FAIL to compile (no `Format::AlignedBytes`, no allocation).

- [ ] **Step 2: The format.** In `header.rs` add `AlignedBytes = 24` with a doc comment ("24..=31: a `ByteArray`'s bytes, its data at a 32-byte address; the low three bits are the unused bytes of the last word"), the arm `24..=31 => Format::AlignedBytes` in `from_code`, and `Format::AlignedBytes => 7` in `unused_limit`; extend the module doc's format list. In `object.rs`, add `Format::AlignedBytes` to `pointer_ranges`'s arm without pointer fields, and the module doc's sentence on `AlignedBytes`. `units_per_word` keeps answering `None` for it, so no `ByteArray` is ever a string.

- [ ] **Step 3: The layout helpers** in `object.rs`:

```rust
pub(in crate::heap) fn byte_array_field_count(size: usize) -> usize {
    ALIGNMENT_RESERVE + size.div_ceil(BYTES_PER_WORD)
}

pub(in crate::heap) fn aligned_data_start(header: usize) -> usize {
    (header + FIELDS).next_multiple_of(ALIGNMENT_WORDS)
}

pub(in crate::heap) fn realign(words: &mut [u64], old_header: usize, new_header: usize) {
    // The block is the header and every word after it, the overflow word
    // read when the header's count says 255.
    let block = crate::heap::space::block_range(words, new_header);
    let data_words = block.len() - FIELDS - ALIGNMENT_RESERVE;
    let old_offset = aligned_data_start(old_header) - (old_header + FIELDS);
    let new_offset = aligned_data_start(new_header) - (new_header + FIELDS);
    if old_offset != new_offset {
        let source = new_header + FIELDS + old_offset;
        words.copy_within(source..source + data_words, new_header + FIELDS + new_offset);
    }
}
```

  Check `space::block_range`'s doc first: it must answer the header's index up to the last field, so that `len()` is 1 + the word count; adjust the subtraction if it answers another span. The reserve words' contents are never read, so `realign` does not clear them; the allocation and the decoder zero them so the space holds no stale data.

- [ ] **Step 4: Allocation and the accessors** in `heap.rs`, in "Object construction" beside `allocate_string` and in "Object accessors" after the string block, as the agent map of the heap shows: `allocate_zeroed_byte_array` is `allocate_object(class, Format::AlignedBytes, unused, false, byte_array_field_count(size))` followed by zeroing every field word (they come back nil); `allocate_byte_array` writes the bytes after it. A private `byte_array_words(&self, object) -> Option<(&[u64], usize)>` answers the data words and the size, or `None` unless the format is `AlignedBytes`; it `debug_assert!`s `aligned_data_start(header_index) % ALIGNMENT_WORDS == 0` (trivially) and that the data end inside the block. The public accessors and the two lenders are built on it. Add `Format::AlignedBytes => format!("a ByteArray of {}", size)` to `describe`. `object_shape` is left to Task 2: no program reaches an object of this format before Task 4.

- [ ] **Step 5: The movers.**
  - `scavenge.rs`'s `copy`: after `let moved = destination + header_within;`, if `Header(words[moved]).format() == Format::AlignedBytes`, call `object::realign(words, header, moved)` before the old place is overwritten with the forwarder. (The source block is not read again, so the order only matters for `moved`'s header.)
  - `heap.rs`'s `compact`, the slide loop: after `let moved = destination + (header - position);`, the same call with `header` and `moved`. The slide copies downward and the next object starts at or after `next`, so realigning inside `[moved, moved + length)` touches no word not yet moved.
  - `heap.rs`'s `move_everything` (debug only): after each region's `copy_within`, walk the region at its destination with `space::object_at` and realign each `AlignedBytes` object from `header - distance` to `header`, where `distance` is the region's shift in words (negative when it moves back to its base; compute with `isize` or both directions explicitly).
  - Comment each call site with what it prevents: the data left at the old block's offset, wrong bytes at exit status 0.

- [ ] **Step 6: Run the tests and the gates.**

Run: `cargo nextest run -p ns-runtime --lib`, then the per-commit gates.
Expected: PASS.

- [ ] **Step 7: Commit** — `feat: add the AlignedBytes format, realigned by every mover`

### Task 2: The `ByteArray` image record

**Files:**
- Modify: `crates/ns-runtime/src/heap/records.rs` (`ObjectShape::ByteArray`)
- Modify: `crates/ns-runtime/src/heap.rs` (`object_shape`, `record_size::byte_array_fields`, `fill_placeholder_byte_array`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_BYTE_ARRAY = 11`, "Next free number: 12.")
- Modify: `crates/ns-runtime/src/image/encode.rs` (the graph walk's no-reference arm, `encode_object`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`Reader::read_byte_slice`, `Sizing::record`, the second pass)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (`walk_graph`)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs` (the four lists), `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: Task 1's `allocate_byte_array`, `byte_array_bytes`, `byte_array_field_count`, `aligned_data_start`.
- Produces: `ObjectShape::ByteArray`; `record_size::byte_array_fields(length: usize) -> usize`; `Heap::fill_placeholder_byte_array(&mut self, object, class, identity_hash: u32, bytes: &[u8]) -> bool` (`false`, nothing written, when the placeholder is not `byte_array_field_count(bytes.len())` fields long); `Reader::read_byte_slice(&mut self) -> Result<&'a [u8], DecodeError>` (a `u32` length, then a borrowed slice, as `read_text` borrows).

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`. Until Task 4 there is no `ByteArray` class, so these tests make the object on the host with `mutator.heap_mut().allocate_byte_array(mutator.builtin_class(BuiltinClass::Object), &bytes)` -- the record's class is whatever the object's class is -- and Task 4 switches them to the real class:
  - `a_byte_array_round_trips_with_its_bytes_and_identity_hash` (pattern: `instances_and_arrays_round_trip_with_shared_identity_preserved`): sizes 0, 5 and 4,000, one of them referenced twice from an `Array`, so identity is kept; after decoding, `byte_array_bytes` equals the original and the decoded object's data are aligned (`heap.is_byte_array`, and in the unit tests the alignment already holds).
  - `a_truncated_byte_array_record_is_refused_before_allocating` (pattern: `the_first_pass_refuses_a_record_count_the_bytes_cannot_hold`): a record claiming 1,000,000 bytes of which 8 are written; `UnexpectedEndOfFile`, live objects unchanged.
  - `an_overlong_byte_array_length_is_refused` : a length of `u32::MAX`.
  - Add `OBJECT_BYTE_ARRAY` to `a_record_of_a_shipped_class_with_another_body_is_refused`'s list (a shipped non-builtin header with this body is `ShippedInstanceBody`).
  - `a_truncated_graph_section_is_a_decode_error_not_a_panic` and `every_truncation_of_an_image_is_reported_rather_than_panicking` gain a byte array in their graph.
  - In `opcode_tags.rs`: `assert_eq!(OBJECT_BYTE_ARRAY, 11);` and the tag in `retired_object_body_tags_are_refused`'s `live` list and in `no_two_graph_tags_in_a_family_share_a_number`'s object family.

Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags`
Expected: FAIL to compile.

- [ ] **Step 2: Implement.** `object_shape` gets `Format::AlignedBytes => ObjectShape::ByteArray` before its catch-all arms. The encoder writes `OBJECT_BYTE_ARRAY`, then `u32_length("byte array", bytes.len())?` and `writer.write_bytes(&bytes)` (a `u32` length and the bytes); the graph walk puts `ByteArray` with `String | Symbol | LargeInteger | Float`. The decoder's first pass: `OBJECT_BYTE_ARRAY => (record_size::byte_array_fields(reader.read_byte_slice()?.len()), false)` -- the slice read fails at the end of the input, so an overlong length is refused before anything is allocated, and the amplification check that follows counts these words like any record's. The second pass reads the slice again and calls `fill_placeholder_byte_array`, `UnexpectedEndOfFile` when it answers `false`. `fill_placeholder_byte_array` zeroes every field word, writes the bytes at `aligned_data_start` of the placeholder's own header index, and rewrites the header through `rewrite_placeholder_header` with `Format::AlignedBytes` and the unused bytes. `inspect` prints `byte array of {n} bytes` and the first 16 bytes in hex. Document the tag in `opcodes.rs` as `OBJECT_FRAME`'s comment does: the record, how the first pass sizes it from its own bytes, and that the reserve is not written.

- [ ] **Step 3: Check the amplification bound.** `docs/design/memory.md`, "The amplification bound of a rejected load", and `decode_value`'s doc comment derive the peak per record kind. A `ByteArray` record of `n` bytes is `14 + n` bytes on the wire and `8 × (5 + ⌈n/8⌉)` bytes in the heap (an overflow word more past 254 field words): at most 40 bytes for 14, under the instance record's ratio. Confirm that against the breakdown; if it raises the bound, stop and report rather than changing the bound.

- [ ] **Step 4: Run, regenerate nothing (no shipped world changed), the gates, commit** — `feat: carry a ByteArray in the image as the record OBJECT_BYTE_ARRAY`

### Task 3: A kernel with ordinary classes, and `Array` over the mixin `IndexedCollection`

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`check_kernel_shape`, `check_clause`, `fill_builtin_classes`, `wire_kernel_in`, the tests named below)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (the header comment, `IndexedCollection`, `Array`)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (tests), `crates/ns-runtime/src/image/shipped_names.rs` (tests)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Modify: `crates/ns-runtime/tests/image.rs` (a shipped level round trip)

**Interfaces:**
- Consumes: `nested::build_applied_levels`, `Mutator::apply_mixin`, `Mutator::new_class_object`, `give_existing_class_objects_metaclasses`, `hidden_slot_of`, `class_set_lexical_context`, `set_metaclass_lexical_context`.
- Produces: `IndexedCollection` in `Kernel.ns`, a private mixin class whose instance side holds the sequence protocol, written over `at:`, `at:put:`, `size` and the protected `newOfSize: size` (a new, empty-filled instance of the receiver's kind) that every class applying it implements; `Array` applies it. The bootstrap accepts (a) nested classes of `Kernel` that are no builtins, built by their accessors as any class is, and (b) a builtin superclass clause with `<:` clauses, each naming one such kernel class with no message.

- [ ] **Step 1: Write the failing tests** in `kernel.rs`'s tests and `shipped_world.rs`'s:
  - `the_hierarchy_follows_the_specification`: `Array`'s superclass is now an applied level whose `class_applied_origin` names `IndexedCollection`'s mixin and whose superclass is `Collection`. Change that row into an explicit walk; every other row unchanged.
  - `an_applied_level_of_a_builtin_has_the_kernels_lexical_context`: the level's enclosing object is the kernel instance and its declaring class the kernel class, as are its metaclass's; `IndexedCollection`'s class is the one the kernel's accessor answers (the hidden slot holds it, so `complete_shipped_world` builds no second one).
  - `a_shipped_level_is_named_and_resolves_to_the_boots_own_object` -- the test `docs/OPEN-WORK.md`, "No shipped class applies a mixin", asks for: `Kernel Array level 0` and its metaclass are in the shipped world with the frame-class-only positions, `shipped_names::resolve` resolves the name to the boot's own level object, `index_by_object` finds the level by its class object, and the level's identity hash round-trips. Model it on the existing tests that do the same for a shipped class (search `image/shipped_names.rs`, `interpreter/shipped_world.rs` and `tests/image.rs` for `HASHED_SHIPPED` and `index_by_object`), and turn `a_level_past_the_clauses_is_refused` so that `Kernel Array level 1` is the unknown one.
  - `only_top_activation_future_implementation_and_non_boolean_receiver_are_private`: count every nested class, no longer `ALL.len() - 1`, and admit `IndexedCollection` as private. Rename it to say what it checks (`only_the_kernels_private_classes_are_private`), the list of private names in one constant.
  - Extend `every_kernel_method_has_the_fast_path_it_should` so it walks every nested class of the kernel, not only the builtins': key `NOT_FORWARDERS` by the class's name instead of `BuiltinClass`, and move `Array`'s Newspeak methods' rows to `IndexedCollection`.

Run: `cargo nextest run -p ns-runtime --lib -E 'test(kernel) | test(shipped)'`
Expected: FAIL.

- [ ] **Step 2: `check_kernel_shape` takes ordinary classes.** The one-nested-class-per-builtin count becomes: every builtin but the mirror is declared exactly once, and every other nested class is no builtin's name. The builtin checks (slots, factory, clause) apply to the builtins only; an ordinary class is unchecked here -- its clause runs when its accessor builds it. `check_clause` additionally accepts `mixin_applications`, each with no message arguments and naming a nested class of the kernel that is no builtin; that class's own clause must name a builtin (`IndexedCollection = Collection`), so the bootstrap can build it from builtins alone. `fill_order` and `depth_below_top` keep ranking builtins only.

- [ ] **Step 3: `fill_builtin_classes` builds the levels.** For a builtin whose clause has `<:` clauses, after its superclass is filled: build each named kernel class once (memoized by name across builtins) as `apply_mixin(&its_mixin, Some(&collection_application))` and `new_class_object(application, None, None, Some(collection_class))`; then `build_applied_levels(self, &builtin_mixin, Some((superclass_class, superclass_application)), &[(index, Value::object(named_class)), …])` with the builtin's own mixin `Rc` -- the one `nested_named(&root_mixin, name)` answered, since `shipped_world::own_levels` finds a level by `Rc::ptr_eq` with it; apply the builtin's mixin over the top level's application and link the builtin's superclass to the top level. Pass every class object built here (the named kernel classes and the levels) to `give_existing_class_objects_metaclasses`. Keep the kernel root's own branch as it is.

- [ ] **Step 4: `wire_kernel_in` wires them.** Store each kernel class built in Step 3 in its hidden slot (`hidden_slot_of` keyed by its mixin) and give it, its metaclass, and every level of every builtin (walk each builtin's superclass chain while `class_is_applied_level`) the kernel's lexical context, as the existing loop does for the kernel's own levels. Generalise that loop into one helper used for both.

- [ ] **Step 5: `IndexedCollection` and `Array` in `Kernel.ns`.** Update the header comment: the bootstrap reads a builtin's clause and its `<:` clauses and runs neither; an ordinary class's clause runs when the boot builds it. Declare, before `Array`:

```newspeak
  /// The sequence protocol shared by Array, ByteArray and TypedArray: …
  private class IndexedCollection = Collection () (
    …
  )
```

  and `public class Array = Collection <: IndexedCollection () (`. Move into `IndexedCollection`, rewritten over `at:`, `at:put:`, `size` and `self newOfSize: n` in place of `outer Kernel Array new: n`: `isEmpty`, `first`, `last`, `indexOf:`, `keysAndValuesDo:`, `copyFrom:to:` (element by element), `replaceFrom:to:with:startingAt:` (the body of today's private `replaceFrom:to:withElementsOf:startingAt:`, which goes, overlap-safe as it is), `,`, `copyWith:`, `copyWithSize:`, `collect:` (always an `Array`: `outer Kernel Array new:`), `select:` and `reject:` (of the receiver's kind), `sort:`, `sortFrom:to:by:`, the private merge, and `asArray` (a new `Array` of the elements). `Array` keeps `isKindOfArray`, `do:`, `at:`, `at:put:`, `size`, `asArray` (the receiver; it overrides the mixin's), `copyFrom:to:` and `replaceFrom:to:with:startingAt:` (leaves whose failure block is now `^super …`, reaching the mixin), and gains `protected newOfSize: size = ( ^outer Kernel Array new: size. )`. The mixin declares none of `at:`, `at:put:`, `size`, `do:`, `=`, `hash`, `printString`, so a user subclass's fall-through stays as `goldens/library/builtin-subclass/` pins it. Every moved member keeps its comment, rewritten for the receiver's kind where it said `Array`.

- [ ] **Step 6: Format, regenerate the fixture, run the gates and the stress modes this task touches.**

Run: `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/Kernel.ns`, the fixture regeneration, the per-commit gates, and `NS_NEXT_STRESS_MOVES=1 cargo nextest run -p ns-cli --test run_goldens -E 'test(collection)'` (Windows: set the variable in PowerShell).
Expected: every existing golden unchanged and passing, the new tests pass.

- [ ] **Step 7: Commit** — `feat: share Array's sequence protocol through the kernel mixin IndexedCollection`

### Task 4: `ByteArray` as a builtin class

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::ByteArray`, appended to `ALL`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name`, `OBJECT_CLASS_ACCESSORS`, `QUICK_METHODS`, the hierarchy test, `every_value_classs_application_inherits_value`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`decide_factory`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::new_byte_array(&mut self, bytes: &[u8]) -> ObjectReference<'gc>`, beside `new_array`)
- Create: `crates/ns-runtime/src/builtins/bytes.rs`; Modify: `builtins/mod.rs`, `builtins/primitive.rs`
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`ByteArray`, `Object>>ByteArray`)
- Modify: `crates/ns-runtime/tests/image.rs` (Task 2's tests use the real class), `crates/ns-runtime/tests/fixtures/golden-image.bin`
- Create: `crates/ns-runtime/tests/byte_array_leaves.rs`
- Create: `goldens/library/byte-array/a_byte_array_holds_bytes.ns` / `.toml`, `goldens/library/byte-array/a_byte_array_refuses_what_is_no_byte.ns` / `.toml`, `goldens/library/builtin-subclass/a_subclass_of_byte_array_falls_to_objects_methods.ns` / `.toml`

**Interfaces:**
- Consumes: Task 1's heap accessors, Task 3's `IndexedCollection`.
- Produces: `BuiltinClass::ByteArray`; `Primitive::Bytes(BytePrimitive)` with `BytePrimitive::{Allocate, Size, ElementAt, ElementAtPut}` (Task 5 and 6 append variants); `Mutator::new_byte_array`; `ByteArray`'s `new`, `new:`, `size`, `at:`, `at:put:`, `do:`, `newOfSize:`, `isKindOfByteArray`, `asByteArray`.

- [ ] **Step 1: Write the failing goldens and tests.**
  - `a_byte_array_holds_bytes.ns`: `ByteArray new size`, `(ByteArray new: 3) at: 2` (0), `at:put:` answering the value, `do:` in order, `isKindOfByteArray`, `isKindOfCollection`, `printString` (Object's form, as `Array`'s is: "an Object"), `=` and `hash` identity (two byte arrays with equal bytes are not `=`), `first`, `last`, `isEmpty` from the mixin, `ByteArray new: 0`.
  - `a_byte_array_refuses_what_is_no_byte.ns`: `at:put: 256`, `-1`, `'a'` and `1.0` each caught as `ArgumentError`, printing its `messageText` and the byte unchanged after; index 0 and 4 of a 3-byte array (Array's texts); `new: -1`, `new: 'x'`, and `new:` of `SmallInteger maxVal`-sized -- write it as `(2 raisedTo: 60) - 1` or check the integer protocol for the spelling -- "new: cannot allocate a ByteArray of … bytes".
  - `a_subclass_of_byte_array_falls_to_objects_methods.ns`, as its `Array` neighbour `a_subclass_of_a_builtin_falls_to_objects_methods.ns`: `MyByteArray new` reports `true an Object`, and `MyByteArray new size` ends the run with status 1 and `Uncaught Newspeak error: Collection>>do: is abstract: MyByteArray must implement it`. The path: `size`'s leaf declines, `super size` reaches `Collection>>size`, which sends `do:`. `ByteArray>>do:` is a Newspeak loop over `size`, so it would recurse back into `size`; it therefore begins `| count | count:: vmMirror byteArraySize: self ifFail: [^super do: action].` and loops to `count`, reaching `Collection>>do:` for a receiver that holds no bytes. Its comment says so.
  - `byte_array_leaves.rs` (helpers copied from `array_leaves.rs`): the class of `ByteArray new: 2` is `BuiltinClass::ByteArray`; `Object>>ByteArray` is protected and answers the class; `ByteArray new: 40000` is allocated old (its offset is past the nursery: read through `Heap`'s statistics or a helper the heap tests use) and holds zeros.
  - In `kernel.rs`: `ByteArray` in `OBJECT_CLASS_ACCESSORS`, `(ByteArray, "isKindOfByteArray", Constant(True))` and `(ByteArray, "asByteArray", Receiver)` in `QUICK_METHODS`, a hierarchy row `ByteArray` → level of `IndexedCollection` → `Collection`, `(ByteArray, false)` in `every_value_classs_application_inherits_value`; `byte_array_new_is_a_mirror_forwarder` modelled on `array_new_is_a_mirror_forwarder`.

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(byte)' ; cargo nextest run -p ns-runtime -E 'test(byte_array)'`
Expected: FAIL.

- [ ] **Step 2: The builtin.** Append `ByteArray` to `BuiltinClass` and `ALL` with its doc comment (follow `ALL`'s checklist comment); `kernel_class_name`; `decide_factory`'s `Some(BuiltinClass::ByteArray) => allocate an empty one`; `Mutator::new_byte_array`. Append `ByteArray` to the hierarchy of `fill_order` by declaring it in `Kernel.ns` after `Array`: `public class ByteArray = Collection <: IndexedCollection () (`. `Object` gains `protected ByteArray = ( ^outer Kernel ByteArray. )` beside `Array`, documented like it.

- [ ] **Step 3: The leaves.** `BytePrimitive` in `primitive.rs` with its arm in `answers_false_to_a_stranger` (`false`, as `Collection`'s), the four rows appended to `MIRROR`, and `builtins/bytes.rs` with `execute`:
  - `Allocate` (class side): decline unless the receiver's `class_builtin` is `ByteArray`; `Array new:`'s three refusals with "a ByteArray of {n} bytes"; the size checked against what can be allocated before allocating (a size whose field count overflows, or past the object space's reservation, is the third text; reuse what `collections.rs`'s `allocate` does with `try_reserve_exact`, sized to the field words).
  - `Size`, `ElementAt`, `ElementAtPut`: decline unless `heap.is_byte_array(subject)`; index texts from `collections::out_of_range` / `not_an_integer` (make them `pub(crate)` if they are not); a value outside 0 to 255 or no integer is the byte-value text, carrying the value, the byte unchanged; `at:put:` answers the value.
  - `ByteArray` in `Kernel.ns`: `isKindOfByteArray`, `size`, `at:`, `at:put:` (each `^vmMirror … ifFail: [^super …]`), `do:` (see Step 1), `asByteArray` (`^self`), `protected newOfSize: size = ( ^outer Kernel ByteArray new: size. )`; class side `new: size = ( ^vmMirror byteArrayClass: self new: size ifFail: [nil]. )`.

- [ ] **Step 4: The image tests use the class.** Task 2's tests allocate through `mutator.new_byte_array` now, and `a_byte_array_round_trips_with_its_bytes_and_identity_hash` asserts the decoded class is `BuiltinClass::ByteArray`'s; add one built by a program (`ByteArray new: 3` in a slot) round-tripping through `round_trip_graph`.

- [ ] **Step 5: Format, regenerate the fixture, run the gates and the release size test.** Commit — `feat: add ByteArray, a builtin of aligned bytes`

### Task 5: `ByteArray`'s whole protocol

**Files:**
- Modify: `crates/ns-runtime/src/builtins/bytes.rs`, `builtins/primitive.rs` (seven rows, `BytePrimitive::{CopyRange, Replace, Concatenate, IndexOf, LastIndexOf, StartsWith, EndsWith}`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`ByteArray`), `tests/fixtures/golden-image.bin`
- Modify: `crates/ns-runtime/tests/byte_array_leaves.rs`
- Create: `goldens/library/byte-array/a_byte_array_copies_searches_and_sorts.ns` / `.toml`, `goldens/library/byte-array/a_string_argument_counts_as_its_utf8_bytes.ns` / `.toml`, `goldens/library/byte-array/a_byte_array_range_error_is_an_argument_error.ns` / `.toml`

**Interfaces:**
- Consumes: Task 4.
- Produces: the protocol of the design's "`ByteArray`'s protocol", whole. A shared Rust helper `bytes::argument_bytes(heap, value) -> Option<ArgumentBytes>` answering, for a `ByteArray` or a `String` argument, an iterator-capable view of its bytes -- a `String`'s as UTF-8, encoded while it is read (its code points are its bytes while every one is below 128) -- so comparisons allocate nothing; a replacement from a `String` encodes into a Rust buffer first (no Newspeak object is made), because the receiver is written while the string is read.

- [ ] **Step 1: Write the failing goldens and leaf tests.**
  - `a_byte_array_copies_searches_and_sorts.ns`, modelled on `goldens/library/collection/an_array_copies_searches_and_sorts.ns`: `copyFrom:to:` (a `ByteArray`), `replaceFrom:to:with:startingAt:` from a `ByteArray` and overlapping itself both ways (memmove), `,`, `copyWith:`, `copyWithSize:` (grown with zeros), `collect:` (an `Array`; `collect: [:each | each printString]` works), `select:` / `reject:` (`ByteArray`s), `sort:` and `sortFrom:to:by:`, `asArray`, `keysAndValuesDo:`, `withAll:` from an `Array` and from a `ByteArray`, `indexOf:` of an integer, `indexOf:startingAt:`, `lastIndexOf:`, `lastIndexOf:startingAt:` with an integer, a `ByteArray` and a one-byte subsequence, `startsWith:` / `endsWith:` with a `ByteArray`, the empty subsequence found at the start, a start of `size + 1` allowed.
  - `a_string_argument_counts_as_its_utf8_bytes.ns`: `(ByteArray withAll: 'GIF89a') startsWith: 'GIF'`, `withAll: 'é'` is two bytes 195 169, `indexOf: 'é'`, `,` with a string, `replaceFrom:to:with:startingAt:` from a string; a `String`'s methods still refuse a `ByteArray` (`'abc' indexOf: (ByteArray withAll: 'b')` is the existing "requires a String argument" text).
  - `a_byte_array_range_error_is_an_argument_error.ns`: `copyFrom: 0 to: 2`, a start of `size + 2`, `withAll: {1. 300}` (the byte-value text with "withAll:"), `withAll: {1. 'x'}`.

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(byte)'`
Expected: FAIL.

- [ ] **Step 2: Implement the leaves** with `collections::checked_range` and `source_start` for the ranges, `text.rs`'s search texts for the starts. `Replace` reads its source first and writes after, through `ByteArrayDataMut::copy_within` when the source is the receiver and `write` otherwise; `IndexOf`/`LastIndexOf` take an integer (an element: an integer outside 0 to 255 is simply not found) or a `ByteArray`/`String` (a subsequence); anything else declines, so the failure block reaches the mixin's element-wise version, which compares with `=`.

- [ ] **Step 3: The Newspeak side.** `ByteArray` gains the forwarders `copyFrom:to:`, `replaceFrom:to:with:startingAt:`, `,`, `indexOf:startingAt:`, `lastIndexOf:startingAt:`, `startsWith:`, `endsWith:`, and `indexOf:` / `lastIndexOf:` over them; class side `withAll: collection`: a `String` answers `collection asUtf8` (Task 6 adds it -- until then, write this branch in Task 6 and let `withAll:` take a `ByteArray` or any collection of integers here), a `ByteArray` its copy, anything else `new: collection size` then `at:put:` in `do:` order, signalling with the selector `withAll:` and the element (wrap the `at:put:` refusal by checking the element first in Newspeak, so the text names `withAll:`).

- [ ] **Step 4: Format, regenerate, gates, release size test, commit** — `feat: complete ByteArray's protocol`

### Task 6: UTF-8 on `String`

**Files:**
- Modify: `crates/ns-runtime/src/builtins/text.rs`, `builtins/bytes.rs`, `builtins/primitive.rs` (`StringPrimitive::{AsUtf8, FromUtf8, Utf8Size}`, `BytePrimitive::WriteCodePoint`)
- Modify: `crates/ns-runtime/src/code_points.rs` (`CodePoints::from_utf8_strict(bytes: &[u8]) -> Result<CodePoints, usize>`, the error the 0-based offset of the first byte of the bad sequence; `CodePoints::utf8_size(&self) -> usize`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`String`, `String class`, `ByteArray class>>withAll:`), `tests/fixtures/golden-image.bin`
- Modify: `crates/ns-runtime/tests/builtins_text.rs`
- Create: `goldens/library/string/a_string_converts_to_and_from_utf8.ns` / `.toml`, `goldens/library/string/invalid_utf8_is_refused_at_its_byte.ns` / `.toml`

**Interfaces:**
- Produces: `String>>asUtf8`, `String class>>fromUtf8:`, `String class>>fromUtf8:from:to:`, and the two members `StringBuilder` uses, which the design calls kernel-only leaves: `String>>utf8Size`, the string's UTF-8 length in bytes, and `ByteArray>>at: position putCodePoint: codePoint`, which writes one code point as UTF-8 at the 1-based `position` and answers the bytes written (1 to 4). `StringBuilder` is another class, so `protected` would not reach them: both are public and documented in full like every member. That a program can send them too is harmless, since neither breaks an invariant of its receiver.

- [ ] **Step 1: Write the failing tests.**
  - `a_string_converts_to_and_from_utf8.ns`: `'aé€😀' asUtf8` printed as its bytes (`asArray printString` of each, or `do:` printing), `String fromUtf8: that` equal to the original, `fromUtf8:from:to:` of a middle range, a byte-order mark stays U+FEFF (`(String fromUtf8: (ByteArray withAll: {239. 187. 191})) first` is 65279), `String withAll: (ByteArray withAll: {104. 233})` is Latin-1 `'hé'`, the empty array.
  - `invalid_utf8_is_refused_at_its_byte.ns`, each caught and its `messageText` printed: a truncated sequence at the end, a stray continuation byte, an overlong `{192. 128}`, a surrogate `{237. 160. 128}`, past U+10FFFF `{244. 144. 128. 128}`, a range ending inside a character (`fromUtf8: 'é' asUtf8 from: 1 to: 1`), a range out of bounds (`copyFrom:to:`'s text with the selector `fromUtf8:from:to:`), `fromUtf8: 'abc'` (no `ByteArray`).
  - In `builtins_text.rs`: `from_utf8_strict` agrees with `std::str::from_utf8` on every invalid kind and reports the position Rust's `Utf8Error::valid_up_to` gives, plus one.

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(utf8)' ; cargo nextest run -p ns-runtime --test builtins_text`
Expected: FAIL.

- [ ] **Step 2: Implement.** `from_utf8_strict` is `std::str::from_utf8` on the slice, `Err(error.valid_up_to())` on failure (strict exactly as the design asks, BOM kept). The leaves: `AsUtf8` (never fails), `FromUtf8` (class side; decline unless the receiver is the `String` class; a subject that is no `ByteArray` is the view's-source text with `fromUtf8:from:to:`; the range through `checked_range`; invalid UTF-8 the invalid-UTF-8 text, its position the 1-based index into the `ByteArray`, not into the range, so that `bytes at: position` is the bad byte; the comment says so), `Utf8Size`, `WriteCodePoint` (refuses what is no Unicode scalar value with the `addCodePoint:` text; refuses a position whose encoded bytes would pass the end with the index text). `String>>asUtf8`, `utf8Size`; `String class>>fromUtf8: bytes = ( ^self fromUtf8: bytes from: 1 to: bytes size )` -- but `bytes size` on a non-`ByteArray` answers anything: guard with `isKindOfByteArray` first and signal the source text; `ByteArray class>>withAll:`'s `String` branch is `^collection asUtf8`.

- [ ] **Step 3: Format, regenerate, gates, release size test, commit** — `feat: convert a String to and from UTF-8`

### Task 7: `StringBuilder`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`StringBuilder`), `crates/ns-runtime/core-library/PlatformFactory.ns` (a private slot and a `KernelNamespace` accessor), `crates/ns-runtime/src/interpreter/kernel.rs` (`the_platform_modules_members_have_the_fast_path_they_should`'s selector list), `tests/fixtures/golden-image.bin`
- Create: `goldens/library/string-builder/a_builder_appends_strings_and_code_points.ns` / `.toml`, `goldens/library/string-builder/a_builder_grows_across_multi_byte_code_points.ns` / `.toml`, `goldens/library/string-builder/a_builder_refuses_what_is_no_string.ns` / `.toml`

**Interfaces:**
- Consumes: Task 6's `utf8Size`, `at:putCodePoint:`, `fromUtf8:from:to:`; Task 5's `replaceFrom:to:with:startingAt:` with a `String` source and `copyFrom:to:`.
- Produces: `public class StringBuilder new: capacity = Object (` with the slots `private data ::= outer Kernel ByteArray new: capacity.` (a negative or non-integer capacity is `ByteArray new:`'s refusal), `private byteCount ::= 0.`, `private codePointCount ::= 0.`; class side `new = ( ^self new: 16. )`. Members: `add:`, `addCodePoint:`, `writeln:`, `size`, `isEmpty`, `isKindOfStringBuilder`, `asString`, `asUtf8`, and the private `ensureRoom: byteTotal`. `platform kernel StringBuilder`.

- [ ] **Step 1: Write the failing goldens.**
  - `a_builder_appends_strings_and_code_points.ns`: `add:` answers its argument, a `Symbol` is a `String`, `addCodePoint: 233` answers 233, `writeln:` answers the builder and ends in a newline, `size` counts code points, `isEmpty`, `asString` a new string each time (`builder asString == builder asString` is false), `asUtf8` a copy (changing it leaves the builder unchanged), `=`/`hash` identity, `StringBuilder new: 0` works.
  - `a_builder_grows_across_multi_byte_code_points.ns`: 1,000 appends of `'é😀'` to `StringBuilder new: 1`, then `size` (2,000), `asString size`, `asUtf8 size` (6,000), the first and last code points.
  - `a_builder_refuses_what_is_no_string.ns`: `add: 3`, `add: (ByteArray new: 2)`, `addCodePoint: 55296`, `addCodePoint: 1114112`, `addCodePoint: -1`, `addCodePoint: 'a'`, `StringBuilder new: -1`, each caught with its `messageText`, and the builder's `asString` unchanged after each.

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(builder)'`
Expected: FAIL.

- [ ] **Step 2: Implement** in `Kernel.ns` after `String`, every member documented. `add:` checks `isKindOfString` first, then `ensureRoom: byteCount + aString utf8Size`, `data replaceFrom: byteCount + 1 to: byteCount + n with: aString startingAt: 1`, adds `n` and `aString size`. `addCodePoint:` checks with `ensureRoom: byteCount + 4` and writes through `data at: byteCount + 1 putCodePoint: codePoint`, adding the answer -- the leaf refuses a non-scalar value before writing, so nothing is appended. `ensureRoom:` replaces `data` with `ByteArray new: (needed max: data size * 3 // 2)` and copies the used bytes. `asString` is `String fromUtf8: data from: 1 to: byteCount` (an empty builder answers `''`; check `from: 1 to: 0` is the empty range `checked_range` allows). In `PlatformFactory.ns`, `private StringBuilder = kernel StringBuilder.` and `KernelNamespace>>StringBuilder`, documented as its neighbours; add the selector to the kernel test's list.

- [ ] **Step 3: Format, regenerate, gates, commit** — `feat: add StringBuilder over a ByteArray of UTF-8`

### Task 8: The element access family and the typed arrays

**Files:**
- Create: `crates/ns-runtime/src/builtins/elements.rs`
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ElementKind`, `ElementAccess`, `Primitive::Element`, 18 rows), `builtins/mod.rs`, `src/heap/object.rs` / `src/heap.rs` (the width reads and writes on `ByteArrayData`/`ByteArrayDataMut`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`TypedArray` and its ten classes), `PlatformFactory.ns` (eleven slots and accessors), `interpreter/kernel.rs` (the selector list), `tests/fixtures/golden-image.bin`
- Create: `crates/ns-runtime/tests/element_access.rs`
- Create, under `goldens/library/typed-array/`: `each_integer_array_holds_its_range.ns`, `float32_rounds_to_nearest_even.ns`, `the_float_arrays_keep_infinities_and_nan.ns`, `views_over_one_byte_array_share_it.ns`, `a_typed_array_refuses_what_is_out_of_range.ns`, `a_typed_array_has_the_sequence_protocol.ns`, each with its `.toml`

**Interfaces:**
- Produces, in `primitive.rs`:

```rust
/// The ten element kinds of the typed arrays and `DataView`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ElementKind { Int8, Uint8, Int16, Uint16, Int32, Uint32, Int64, Uint64, Float32, Float64 }

/// One access to a `ByteArray` at a width: bits 0-3 the kind, bit 4 big-endian,
/// bit 5 a 1-based byte position (else a 1-based element index), bit 6 a write.
/// One byte, so that `Primitive` stays two (`resolve.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ElementAccess(u8);

impl ElementAccess {
    pub const fn new(kind: ElementKind, big_endian: bool, byte_position: bool, write: bool) -> ElementAccess;
    pub fn kind(self) -> ElementKind;
    pub fn big_endian(self) -> bool;
    pub fn byte_position(self) -> bool;
    pub fn write(self) -> bool;
}
```

  and on `ByteArrayData`: `read(offset: usize, kind: ElementKind, big_endian: bool) -> Option<Element>`; on `ByteArrayDataMut`: `write(offset, kind, big_endian, element: Element) -> bool`, where `Element` is `Integer(i128)` or `Float(f64)` and `offset` is 0-based in bytes; `None`/`false` when the bytes pass the end.
- Produces, in `Kernel.ns`: `public class TypedArray on: aByteArray = Collection <: IndexedCollection (` with `protected bytes = …` (validated: a `ByteArray` whose size is a multiple of `bytesPerElement`, else the `on:` texts), instance members `bytes`, `bytesPerElement` (subclass responsibility), `size`, `do:`, `isKindOfTypedArray`, `newOfSize:` (`^self class new: size`; `self class` is allowed), and the abstract `at:`, `at:put:`; class side helpers `bytesFor: size width: width` (`Array new:`'s first two texts for size, then `ByteArray new: size * width`) and `fill: anArray from: collection`. Ten subclasses `public class Int16Array on: aByteArray = TypedArray on: aByteArray (`, each with `bytesPerElement` (`^2`), `at:`, `at:put:`, and a class side `new`, `new:`, `withAll:`, `bytesPerElement`, each one line over `TypedArray`'s helpers -- a class side is not inherited (§3.2), so each declares its own.

- [ ] **Step 1: Write the failing tests.** `element_access.rs` checks `ElementAccess` round-trips every combination and the reads/writes of every kind in both orders at every offset modulo 8 against `i16::from_le_bytes` etc. The goldens: every integer kind's min and max written and read, one past each refused with the element unchanged; `Int64Array`'s and `Uint64Array`'s extremes read as `LargeInteger`s (`(a at: 1) = (2 raisedTo: 63) - 1`, without naming the class) and written from them; `Float32Array` with `0.1` reading back `0.10000000149011612`, a tie rounding to even, `1e39` becoming infinity, `-1e39` negative infinity, NaN staying NaN (`(a at: 1) = (a at: 1)` -- check the NaN rule in `core-library.md`, "A NaN equals a NaN", and print what that rule makes it), an integer and a fraction converted as `asFloat`, `at:put:` answering the value given; `views_over_one_byte_array_share_it`: an `Int32Array on: bytes` and a `Uint8Array on: bytes` over one `ByteArray` see each other's writes, `bytes` answers the very `ByteArray` (`==`); `a_typed_array_refuses_what_is_out_of_range`: `on:` of 6 bytes for an `Int32Array`, `on: 'abc'`, `new: -1`, `at: 0`; `a_typed_array_has_the_sequence_protocol`: `copyFrom:to:` and `select:` answer the receiver's class (`isKindOfTypedArray` and the values), `collect:` an `Array`, `indexOf: 3` finds `3.0` in a `Float64Array`, `sort:`, `withAll:`, `bytesPerElement` on both sides.

Run: `cargo nextest run -p ns-runtime --test element_access ; cargo nextest run -p ns-cli --test run_goldens -E 'test(typed)'`
Expected: FAIL.

- [ ] **Step 2: The reads and writes** on `ByteArrayData`/`ByteArrayDataMut`: gather `width` bytes with `data_byte` into a `[u8; 8]`, convert with `from_le_bytes`/`from_be_bytes` of the kind's Rust type (`f32` widened to `f64` exactly); a write range-checks nothing (the leaf has), converts (`f32` by `as f32`, which rounds to nearest even, overflows to an infinity and keeps NaN) and scatters the bytes.

- [ ] **Step 3: The leaves** in `elements.rs`: decline unless the subject is a `ByteArray`; the index or position through `collections`'s integer texts (element index: `out_of_range` against the element count; byte position: the position text); the value for an integer kind through `numbers::integer_operand` (make it `pub(crate)`), refused outside the kind's range; for a float kind the leaf takes only a `Float` (`heap.float_value`), since the Newspeak side converts; answer an integer through `mutator.new_integer(ArbitraryInteger::from_i128(…))` -- never `Value::small_integer`, which panics past 61 bits -- and a float through `mutator.new_float`; a write answers the value given. The 18 rows of this task: `byteArray:int8ElementAt:ifFail:` … `byteArray:float64ElementAt:put:ifFail:`, reported `at:` and `at:put:`.

- [ ] **Step 4: The Newspeak classes** after `ByteArray` in `Kernel.ns`, each documented. A float class's `at:put:` checks `value isKindOfNumber` (the float-value text otherwise), passes `value asFloat` to the leaf and answers `value`. `Uint8Array`'s `at:`/`at:put:` forward to `bytes at:`/`bytes at:put:` after `bytes isKindOfByteArray ifFalse: [^self noByteArray: #at:]`. Every leaf call's failure block is `^self noByteArray: #at:` (a private `TypedArray` method signalling the crafted-slot text), so a decoded view whose slot holds something else is an `ArgumentError` (Task 10 tests it). `PlatformFactory.ns`: `TypedArray` and the ten classes as private slots and `KernelNamespace` accessors.

- [ ] **Step 5: Format, regenerate, gates, release size test, commit** — `feat: add the typed arrays over a ByteArray`

### Task 9: `DataView`

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (36 rows), `builtins/elements.rs` (no new code beyond the rows, if Task 8's leaf already reads `byte_position` and `big_endian`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`DataView`, `LittleEndianDataView`, `BigEndianDataView`), `PlatformFactory.ns` (`DataView` only), `interpreter/kernel.rs` (the private-class list of Task 3, the selector list), `tests/fixtures/golden-image.bin`
- Create, under `goldens/library/data-view/`: `a_view_reads_both_byte_orders.ns`, `a_view_reads_at_unaligned_positions.ns`, `a_view_refuses_positions_past_its_bytes.ns`, `a_view_refuses_an_unknown_byte_order.ns`, each with its `.toml`

**Interfaces:**
- Consumes: Task 8's `ElementAccess` and leaf.
- Produces: `public class DataView over: aByteArray = Object (` (the factory a program does not use: its two-to-eight-byte accessors are abstract; document that `on:byteOrder:` and `new:byteOrder:` are the way to make one), `protected bytes`, `bytes`, `size`, `byteOrder` (abstract), `isKindOfDataView`, `int8At:`, `uint8At:` and their `put:`; class side `on: bytes byteOrder: order` and `new: size byteOrder: order` answering a `LittleEndianDataView` or a `BigEndianDataView` (`outer Kernel LittleEndianDataView over: bytes`), refusing another order or a source that is no `ByteArray`. The two private subclasses each declare `byteOrder` (`^#littleEndian`) and the sixteen accessors `int16At:` … `float64At:put:` over their order's rows.

- [ ] **Step 1: Write the failing goldens**: one `ByteArray` read through both orders (`uint16At: 1` of bytes 1 2 is 513 little-endian, 258 big-endian), every width, signed and unsigned, `float32At:`/`float64At:` of known patterns, `put:` answering the value; unaligned `uint32At: 3`; positions 0 and `size - 2` for `int32At:` refused with the position text; `#middleEndian` and `'littleEndian'` (a `String`, no `Symbol`: decide by `==` with the symbols) refused; `on: 'abc' byteOrder: #bigEndian` refused.

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(view)'`
Expected: FAIL.

- [ ] **Step 2: Implement** the 36 rows (reported as the `DataView` selector, e.g. `int32At:put:`), the classes and `PlatformFactory.ns`'s `DataView`. The two private classes join Task 3's private-name list.

- [ ] **Step 3: Format, regenerate, gates, release size test, commit** — `feat: add DataView in a byte order chosen at creation`

### Task 10: Views and builders in the image, and the boot's cost

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs` (`FIXTURE_NAMES`, new tests)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (a test)
- Modify: `docs/OPEN-WORK.md` ("The cost of building the shipped world at boot": the new measurement)

**Interfaces:**
- Consumes: Tasks 2–9.

- [ ] **Step 1: The shipped world names the new classes.** In `shipped_world.rs`'s `every_shipped_object_is_named_with_the_positions_the_rule_allows`: `Kernel ByteArray` is value, superclass and header; `Kernel StringBuilder`, `Kernel TypedArray`, `Kernel Int16Array`, `Kernel DataView` value, superclass and header; `Kernel LittleEndianDataView`, `Kernel BigEndianDataView` and `Kernel IndexedCollection` header only (private, no public accessor answers them); `Kernel TypedArray level 0` and `Kernel ByteArray level 0` frame-class-only.

- [ ] **Step 2: Round trips.** `a_view_and_a_builder_round_trip_through_a_package`: a program keeps an `Int16Array`, a `Float32Array` and a `Uint8Array` over one `ByteArray`, a big-endian `DataView` over another, and a `StringBuilder` with multi-byte content in slots; after `round_trip_graph` (or `build` and `load` as the package tests do), sending `at:`, `int32At:`, `asString` and `size` answers what it did before, and a write through one decoded view is seen through the other (shared storage survives).

- [ ] **Step 3: Crafted slots** (pattern: the `GraphFixture` tests; append the needed `FIXTURE_NAMES` entries and constants): an `Int16Array` instance record whose `bytes` field is the integer 7; a `BigEndianDataView` whose `bytes` is a `String`; a `StringBuilder` whose `byteCount` exceeds its data's size, and one whose `byteCount` is a `String`. Each decodes (the structure is valid), and the sends `at: 1`, `int32At: 1`, `asString`, `add: 'x'` answer a `HostError` of class `ArgumentError` (check through `expect_error` as `builtins_collections_block.rs` writes it), never a panic.

- [ ] **Step 4: Measure the boot.** With the `measuring-performance` skill: a release build's median start of a hello program on `master` and on this branch, the same way the open-work item's 12.3 ms → 16.0 ms were measured. Rewrite the item's numbers to the current ones with the date; the decision it leaves to the user stays open.

- [ ] **Step 5: Gates, commit** — `test: round-trip and refuse crafted views and builders in an image`

### Task 11: Documentation

**Files:**
- Modify: `docs/design/core-library.md` (the "Planned: sub-project 5" section becomes the current state in "What exists" and its own sections without "Planned:"; the leaf count "164 rows" → 233; the class table; a Decisions entry; "Planned: the kernel completed …" names sub-project 5 built and weak references next)
- Modify: `docs/design/memory.md` (the `AlignedBytes` format, its layout, every mover's realignment, in "Generations" and the object layout section)
- Modify: `docs/design/image-format.md` (the object table's list, "Next free number: 12", "Two passes", the record's paragraph, "Shipped names"'s note that the first shipped levels exist)
- Modify: `docs/design/object-model.md` (the format list, if it has one)
- Modify: `docs/OPEN-WORK.md` ("Next": sub-project 6, weak references, is next and has no design yet; remove "No shipped class applies a mixin"; keep the boot cost item as Task 10 left it)
- Modify: `docs/PROGRESS.md` (the milestone's entry)
- Modify: `CLAUDE.md`: the invariant "**Every mover realigns an `AlignedBytes` object's data.**" with the rule and the pointer to `memory.md`, as the design asks; the `ns-runtime` paragraph names `IndexedCollection`, `ByteArray` and the views only if it lists the kernel's classes elsewhere (it does not today; then leave it)

- [ ] **Step 1: Make every statement current** -- search `docs/` and `CLAUDE.md` for `164`, `Next free number: 11`, `no shipped class applies`, `Bytes = 16`, `24 to 31`, `sub-project 5`, `nested class per builtin`, and rewrite each one the batch made untrue. The plan file itself is left as written.
- [ ] **Step 2: Commit** (Markdown only, no gates) — `docs: describe ByteArray, StringBuilder and the views`
