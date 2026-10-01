# Are We Fast Yet, Stage 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** DeltaBlue, Havlak, CD and Json, the last four Are We Fast Yet benchmarks, and Primordial Soup's own DeltaBlue as `DeltaBlueTon80`, join the eleven programs of stages 1 and 2 in `benchmarks/are-we-fast-yet/`. Each runs at the size Are We Fast Yet's `rebench.conf` pins, and the core library widens only as far as they need, in Primordial Soup's protocol. Along the way a String becomes its code points in two widths, so that `size` and `at:` are one access each.

**Architecture:**
- **Strings.** `ObjectData::String` holds a `CodePoints`: one byte per code point while every code point fits Latin-1, 32 bits otherwise, canonical, so `=` and `hash` read the representation directly. This is the representation decided on 2026-09-29 for road step 3, brought forward onto today's arena. `Symbol` stays as it is; `Symbol` under `String` stays with road step 3. Three leaves follow: `String>>at:`, `String>>copyFrom:to:` and `String class>>with:`.
- **Numbers.** `Float>>sin`, `cos` and `asInteger` become VM-mirror leaves, as `sqrt` is. `Integer>>timesRepeat:` and `Number>>max:` are Newspeak, Primordial Soup's bodies.
- **Collections.** `List` becomes Primordial Soup's `List`:
  - the primary factory is `new: capacity`, with a class-side `new`;
  - it gains `isEmpty`, `removeFirst`, `removeLast`, `remove:ifAbsent:`, `removeAll`, `anySatisfy:` and `sort:`;
  - it keeps the index of its first element, so a queue of `add:` and `removeFirst` runs in amortized constant time.

  `Map` and `Set` are adapted from Primordial Soup's `Collections.ns`, insertion-ordered hash tables, with PS's `transient` slot as ns-next's `lazy` slot. The representation of all three is private.
- **NaN.** `Float>>=` makes every NaN equal to every other, and `hash` agrees, so that a Float's equality is an equivalence, as a value's must be.
- **Documentation.** Every public member of the three shipped modules gets a short comment, and `CLAUDE.md` gains the rule.
- **The benchmarks.**
  - DeltaBlue, Havlak, CD and Json are ported from SOMns into Primordial Soup's module shape, as the eleven before them were.
  - `DeltaBlueTon80` is Primordial Soup's `newspeak/DeltaBlue.ns`, adapted as `RichardsTon80` was.
  - SOMns's second module of CD, `RedBlackTree.ns`, becomes nested classes of `CD`, so that CD is a module like every other (`class CD usingPlatform: platform`, reached as `manifest CD`).

**Tech Stack:** Rust 1.98.0, edition 2021; Newspeak in `crates/ns-runtime/core-library/` and `benchmarks/`; `cargo-nextest`; goldens under `goldens/`. No new dependency.

**Spec:** the design settled with the user on 2026-10-01, recorded in this plan. The background is in:
- `docs/OPEN-WORK.md`, "The road to a high-performance VM", stage 3 of "Before step 2's second plan, the Are We Fast Yet benchmarks";
- `docs/design/execution.md`, "The Are We Fast Yet benchmarks";
- `docs/design/core-library.md`, "What exists", "The on-demand rule", "Planned: strings of code points in two widths";
- `docs/design/object-model.md`, "Planned: the object layout", "Strings have two widths";
- the plans of stages 1 and 2, `docs/superpowers/plans/2026-09-30-1353-ns-next-are-we-fast-yet-stage-1.md` and `2026-09-30-2156-ns-next-are-we-fast-yet-stage-2.md`, whose module shape, header and harness this plan reuses.

The decisions, each the user's (2026-10-01):
1. **Protocol.** Stage 3 follows Primordial Soup's protocol, as stages 1 and 2 do: every selector a port sends to a kernel or collections object exists in Primordial Soup's `KernelForPrimordialSoup.ns` or `Collections.ns` with the same meaning. Stage 1's precedents stay: `notNil` (Towers) and `==` (Richards).
2. **DeltaBlueTon80.** Primordial Soup's own DeltaBlue joins as `DeltaBlueTon80`, beside the Are We Fast Yet `DeltaBlue`, as `RichardsTon80` sits beside `Richards`.
3. **List, used canonically and made fast.** SOMns's `Vector` is Primordial Soup's `List`, `append:` is `add:`, and the ports use a List exactly as the upstream benchmark uses its Vector: a queue is `add:` and `removeFirst`. No port rewrites an algorithm around a slow operation. Instead ns-next's `List` is made fast: `removeFirst` with Primordial Soup's meaning, in amortized constant time.
4. **Map and Set** are adapted from Primordial Soup's `Collections.ns` (Apache 2.0), optimized where worthwhile. Primordial Soup's `transient` slot is ns-next's `lazy` slot: the same getter (§6.3.3). That ns-next's image carries a lazy slot's value, where §6.3.3 intends it never be persisted, was recorded in `docs/OPEN-WORK.md` with the user.
5. **Strings are optimized now:** the two widths come forward onto today's arena.
6. **CD is a module like all the others.** `RedBlackTree.ns` becomes nested classes of `CD`.
7. **Sizes from Are We Fast Yet's `rebench.conf`:**
   - DeltaBlue 12000;
   - Havlak 1500, which answers 6102 loops in a graph of 5213 nodes;
   - CD 250, which answers 10830 collisions;
   - Json: its 100 is a repetition count, so `bench` parses once and checks what SOMns's `verifyResult:` checks;
   - `DeltaBlueTon80` keeps Primordial Soup's 100 and 100.
8. **Mandelbrot stays at 750,** and the documentation is corrected: 750 is SOMns's verified size (`setupVerifiedRun:`), not Are We Fast Yet's, whose `rebench.conf` runs 500.
9. **The licences go into the files.**
   - SOMns's `Havlak.ns` carries Google's Apache 2.0 text, and `CD.ns` and `RedBlackTree.ns` carry Purdue's and Apple's BSD-3 text, rather than SOMns's MIT.
   - Each port keeps its upstream header, as Mandelbrot does, and `THIRD-PARTY-NOTICES.md` gets no entry of its own for them.
   - Task 12 only corrects the notices' introduction, which says every benchmark from SOMns is MIT.
10. **The collections' representation is private.** `List`'s `data` and `firstIndex`, `Map`'s and `Set`'s `size_`, `data_` and `hashtable_`, and every helper method are `private`, where Primordial Soup's are protected. So the representation can change later, to a ring buffer say, or under a fuller List protocol, without a subclass noticing. Only `List`'s `size_` stays protected, as a subclass's view of the size (`goldens/collections/a_user_subclass_of_list_inherits_its_protocol` reads it).
11. **A NaN equals a NaN** (Task 6).
    - A Float is a value, so its `=` must be an equivalence and its `==` answers what `=` does (§3.1.1). IEEE 754's equality is not one.
    - So `Float>>=` is IEEE's except that every NaN equals every other, and every NaN hashes alike. This is JavaScript's SameValueZero, the equality its `Map` and `Set` use; `0.0 = -0.0` stays true.
    - The ordering and the arithmetic stay IEEE's.
    - This differs from Primordial Soup, whose `isNaN` is `^(self = self) not`; ns-next has no `isNaN` yet, and one added later is a leaf.
12. **The public API of the shipped modules is documented** (Task 11).
    - Every `public` member of `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns` gets a short comment that describes it as it stands: what it answers, what it takes, what it signals. It names no reference implementation; comments that give a decision's reasons stay as they are. A family of members may share one.
    - Not too much text and not too little: one to four lines.
    - `CLAUDE.md` gains the rule, so that every later public member comes with it.

**How the ports were checked when this plan was written.**
- Each was run on a release build of `master` (`8d75f82`) under both lowerings. Newspeak stand-ins replaced what is still missing: List, Map and Set, `timesRepeat:`, `sin`/`cos`/`asInteger` and String's `at:`/`copyFrom:to:`. Each stand-in copy differs from the file printed here only in the sends it replaces.
- Results and release times of one `bench`, with the stand-ins:

  | Benchmark | Result | Release time |
  |---|---|---|
  | DeltaBlue at 12000 | `true` | 1.1 s |
  | DeltaBlueTon80 | `true` | 10 ms |
  | Havlak | 6102 and 5213 | 7.1 s (stand-in Map and Set) |
  | CD | 10830 | 5.0 s |
  | Json | 156 operations | 20 ms |
- CD's count does not depend on the last bits of `sin` and `cos`. Correctly rounded values, values one ulp up, values one ulp down, the Windows C library's values and a Taylor series off by up to 557 ulps all gave 10830.
- The proposed `CollectionsFactory.ns` passed 93 checks under both lowerings. A full workspace run with it changed exactly the two goldens and the one Rust test Task 4 and Task 5 name.
- The two-width strings were prototyped in full:
  - every test passed;
  - `Object` stays 56 bytes in a release build;
  - 25,000 `size` and `at:` over a 25,000-character string went from 40 ms to 3 ms.

## Global Constraints

- **The invariants.** Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - **The VM mirror is a capability.** The new leaves are reached only from `Kernel.ns`, which is resolved with `resolve_kernel`. None of its six locks changes.
  - **`vm.rs` gets no dispatch logic.**
  - **No wire tag is added, renumbered or retired.** A String stays `OBJECT_STRING` (tag 1) followed by a `u32` length and UTF-8 bytes. A mirror row has no wire tag.
  - **An image is untrusted input.** The decoder still validates UTF-8 and bounds every length before it allocates. A decoded string allocates exactly what it needs, never more than 4 heap bytes per wire byte.
  - **Accessors copy out.** No `&CodePoints` leaves `heap.rs`: the new accessors answer a size, a code point, a bool, a hash or an owned `CodePoints`.
- **Primordial Soup's protocol.** Every method the kernel and the collections gain exists in Primordial Soup with the same selector and meaning:
  - String's `at:` (`KernelForPrimordialSoup.ns:2335`), `copyFrom:to:` (2343) and class-side `with:` (2421);
  - Float's `sin` (1140), `cos` (1079) and `asInteger` (2005);
  - `Integer>>timesRepeat:` (1738) and `Number>>max:` (2054);
  - List, Map and Set (`Collections.ns`).

  Where ns-next's meaning differs, a comment says so: `at:` answers a code point where Primordial Soup answers a UTF-8 byte (the same for ASCII), and `String with:` takes any code point where Primordial Soup takes a byte.
- **The on-demand rule** (`CLAUDE.md`, "Working in this repository"). Each new selector of `CollectionsFactory.ns` is sent by a benchmark, and its comment or its class's names which one. Map and Set carry only what DeltaBlue and Havlak send.
- **Names are written out, in Newspeak too** (`CLAUDE.md`, "Conventions"). The printed ports already are. Keyword parts such as `at:`, `of:`, `on:do:` are not names. `CD`, `Json` and `DeltaBlue` are the benchmarks' published names and stay.
- **Comments.**
  - A comment explains the decision, not the mechanics.
  - A golden's rationale is a `(* … *)` comment under its category string. The `.toml` holds only the recorded answer.
- **Writing files.**
  - Write every `.ns` file, and every Rust or TOML file holding a backslash, with the editor's Write/Edit tool, never through a shell heredoc, `echo` or `sed`: the shell swallows one backslash of `\\`. `Kernel.ns` has `\\`, `Json.ns` has `\` in comments, and the goldens' `\\` must survive.
  - The `.ns` files printed in this plan are copied byte for byte. That includes Json's 25,820-character input literal, which Task 10 checks against SOMns's file.
- **Goldens.** A golden whose recorded output changes is a behaviour change. This plan changes no recorded output of an existing golden except `goldens/awfy/all_benchmarks_verify.toml`, which gains a line per benchmark. It changes the source of two goldens because `List`'s primary factory becomes `new:`, keeping their output, and rewrites the comments of two others that describe `List`'s old growth (Task 4). If any other golden's output changes, stop and report it.
- **Commits.** Conventional commits. Every commit ends with these lines, and a subagent's commit names the subagent's own model:
  ```
  Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01XkuJBQierVYxAMXpiaz4ty
  ```
- **Where the work happens:** the branch `are-we-fast-yet-stage-3`, made from `master` in the main working directory (no worktree), before Task 1.
- **Per-commit gate** (`CLAUDE.md`, "Working in this repository"):
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules`, which must report "parsed 211, failed 0"

  Task 1 also runs `cargo nextest run --workspace --release`, because it touches `Object`'s payload and only a release build runs the size tests.

  The end-of-batch gates run once, in Task 13: the three stress modes, `run-corpus` plain and under stress, the benchmarks profile and the `ns-lsp` suite. That staggering is deliberate: a commit before Task 13 is work in progress.

  The JetBrains plugin and `ns-syntax` are untouched.
- **The benchmarks profile gets much slower, on purpose.** Each benchmark task adds its program to `goldens/awfy/all_benchmarks_verify`, which runs in five variants, and to `a_timed_run_prints_a_score_for_every_benchmark`. Neither runs in the per-commit gate (`.config/nextest.toml`).
  - Each benchmark task runs its own program through the harness instead, in a release build (its last step).
  - Task 13 runs the profile. In a debug build Havlak and CD take tens of seconds each per variant, and much more under the reference lowering.
  - Give that run `timeout: 600000`. If it does not finish in time, run its six tests one at a time with `-E 'test(=<name>)'`.

## Review Focus

These inputs are the ones most likely to break in use without any upstream benchmark exercising them. Each is pinned by a golden in the task named:

1. **A String with code points beyond Latin-1** through every String leaf, held in a slot so that `build`/`load` carries it. Expected: `size` counts code points, `=` and `hash` agree across widths, and a copy cut from a wide string equals the same text as a literal. Tasks 1 and 2.
2. **`copyFrom:to:` and `at:` at their edges:**
   - the empty range at either end;
   - start 0;
   - stop past the end;
   - stop two before start;
   - the largest and smallest SmallInteger.

   Expected: an Error with a clear text, never a Rust overflow panic. Task 2.
3. **A List used as a queue past many growths and in-place moves.** Expected: order, `at:`, `at:put:`, `do:` and `size` stay right after `removeFirst` has moved the first index, and an emptied list starts over. Task 4.
4. **Map keys of every kind:**
   - an equal but not identical String;
   - a negative and a LargeInteger key;
   - `nil`;
   - two plain objects;
   - a thousand keys across many rehashes;
   - `removeAll` and reuse.

   Expected: lookup by `=`/`hash`, insertion order. Task 5.
5. **A NaN as a key, and two different NaNs.** Expected: `nan = nan`, `nan == nan`, two NaNs from different operations equal and hashing alike, a Map finding a NaN key again; every ordering with a NaN false, and `0.0 = -0.0` still true. Task 6.
6. **`Float>>asInteger` beyond 64 bits and at the edges:** `-0.0`, a negative fraction, 2^63, -2^63, 10^20, NaN and an infinity. Expected: truncation toward zero, exact, a LargeInteger where needed, and an Error for a value that is not finite. Task 3.

## File Structure

- **Create:**
  - `crates/ns-runtime/src/code_points.rs`: the `CodePoints` payload type.
  - `benchmarks/are-we-fast-yet/DeltaBlue.ns`, `DeltaBlueTon80.ns`, `Havlak.ns`, `CD.ns`, `Json.ns`.
  - Goldens:
    - `goldens/kernel/a_string_is_its_code_points`
    - `goldens/kernel/a_string_answers_its_code_points`
    - `goldens/exceptions/a_string_index_error_is_caught`
    - `goldens/arithmetic/float_sine_cosine_and_truncation`
    - `goldens/kernel/times_repeat_and_max`
    - `goldens/collections/a_list_is_a_queue_a_stack_and_sorts`
    - `goldens/collections/a_map_and_a_set_keep_insertion_order`
    - `goldens/arithmetic/not_a_number_equals_itself`

    Each is a `.ns` and a `.toml`.
- **Modify (Rust):**
  - `crates/ns-runtime/src/` files: `lib.rs`, `types.rs`, `hash.rs`, `heap.rs`, `interpreter/mod.rs`, `image/encode.rs`, `image/decode.rs`, `builtins/text.rs`, `builtins/value.rs`, `builtins/collections.rs`, `builtins/primitive.rs`, `builtins/numbers.rs`, `arbitrary_integer/mod.rs`.
  - `crates/ns-runtime/tests/platform.rs`.
  - `crates/ns-cli/tests/run.rs`.
- **Modify (Newspeak):**
  - `crates/ns-runtime/core-library/Kernel.ns`, `CollectionsFactory.ns`, and `PlatformFactory.ns` (its comments only, Task 11).
  - `benchmarks/are-we-fast-yet/AreWeFastYet.ns`, `are-we-fast-yet.json`.
- **Modify (goldens):**
  - `goldens/awfy/*.json` and `goldens/awfy/all_benchmarks_verify.toml`.
  - `goldens/collections/a_user_subclass_of_list_inherits_its_protocol.ns`, `a_list_grows_past_its_first_array.ns`, `a_list_index_error_is_caught_as_an_error.ns`.
  - `goldens/nesting/nested_override.ns`.
- **Modify (documentation, Task 12):**
  - `docs/design/core-library.md`, `object-model.md`, `memory.md`, `image-format.md`, `execution.md`, `README.md`.
  - `docs/OPEN-WORK.md`, `docs/PROGRESS.md`.
  - `THIRD-PARTY-NOTICES.md`, `prior-art/PROVENANCE.md`, `CLAUDE.md`.

---

### Task 0: the branch

- [ ] **Step 1:** From a clean `master`, create the branch the whole plan runs on.

```bash
git switch -c are-we-fast-yet-stage-3
```

---

### Task 1: a String is its code points in two widths

**What changes, and what does not.** `ObjectData::String` holds a `CodePoints` instead of a Rust `String`. No output of any golden or `runnable.toml` module changes:
- `size` already counted code points;
- `hash` keeps its value bit for bit: it is still the hash of the UTF-8 text;
- `describe`, `printString` and the transcript write the same text;
- the image's wire format is unchanged.

The leaves stop cloning the payload. Today every String leaf clones the whole text first, and `size` then walks it.

**Files:**
- Create: `crates/ns-runtime/src/code_points.rs`
- Modify:
  - `crates/ns-runtime/src/lib.rs`
  - `crates/ns-runtime/src/types.rs`
  - `crates/ns-runtime/src/hash.rs`
  - `crates/ns-runtime/src/heap.rs`
  - `crates/ns-runtime/src/interpreter/mod.rs`
  - `crates/ns-runtime/src/image/encode.rs`
  - `crates/ns-runtime/src/image/decode.rs`
  - `crates/ns-runtime/src/builtins/text.rs`
  - `crates/ns-runtime/src/builtins/value.rs`
- Create: `goldens/kernel/a_string_is_its_code_points.ns`, `.toml`

**Interfaces:**
- Produces:
  - **`crate::CodePoints`** (`pub`, re-exported from `lib.rs`), with:
    - `from_code_point(u32) -> Option<CodePoints>`
    - `size(&self) -> usize`
    - `code_point_at(&self, usize) -> Option<u32>` (0-based)
    - `slice(&self, start: usize, end: usize) -> Option<CodePoints>` (0-based, half-open)
    - `concatenate(&self, &CodePoints) -> CodePoints`
    - `elements()`
    - `characters()`
    - `content_hash(&self) -> i64`
    - `From<&str>`, `From<String>`, `Display`, `Debug`
  - **`Heap` accessors:**
    - `is_string(object) -> bool`
    - `string_size(object) -> Option<usize>`
    - `string_code_point_at(object, usize) -> Option<u32>`
    - `string_slice(object, usize, usize) -> Option<CodePoints>`
    - `string_concatenation(first, second) -> Option<CodePoints>`
    - `strings_equal(first, second) -> Option<bool>`
    - `string_hash(object) -> Option<i64>`

    `str_value` keeps its signature and becomes the UTF-8 boundary.
  - **`Mutator::new_string_from_code_points(CodePoints) -> ObjectReference`.**
  - **`text::text_equals(heap, receiver, argument)`,** which loses its `text: &str` parameter.

- [ ] **Step 1: Write `code_points.rs` with its unit tests.**

`crates/ns-runtime/src/code_points.rs`:
```rust
//! A `String`'s payload: its code points, in one of two widths
//! (`docs/design/core-library.md`, "Strings are code points in two widths").

/// A string's elements (§5.1.5), each a Unicode scalar value, stored one
/// byte each while every one fits Latin-1 and 32 bits each otherwise, so
/// that `size` and `at:` are one access.
///
/// Canonical: a string whose code points all fit Latin-1 is always narrow.
/// Equal strings therefore have equal representations, so `=` compares
/// them directly and `hash` agrees with it. Every constructor keeps that,
/// which is why the representation is private.
///
/// Every element is a scalar value -- at most U+10FFFF and never a
/// surrogate -- so the conversion to UTF-8 at every boundary (the
/// transcript, the image, an error text) is total.
#[derive(Clone, PartialEq, Eq)]
pub struct CodePoints(Representation);

#[derive(Clone, PartialEq, Eq)]
enum Representation {
    Narrow(Box<[u8]>),
    Wide(Box<[u32]>),
}

impl CodePoints {
    /// One code point; `None` for a surrogate or a value past U+10FFFF.
    pub fn from_code_point(code_point: u32) -> Option<CodePoints> {
        let character = char::from_u32(code_point)?;
        Some(match u8::try_from(u32::from(character)) {
            Ok(byte) => CodePoints(Representation::Narrow(Box::new([byte]))),
            Err(_) => CodePoints(Representation::Wide(Box::new([code_point]))),
        })
    }

    pub fn size(&self) -> usize {
        match &self.0 {
            Representation::Narrow(bytes) => bytes.len(),
            Representation::Wide(words) => words.len(),
        }
    }

    /// The code point at the 0-based `index`; `None` past the end.
    pub fn code_point_at(&self, index: usize) -> Option<u32> {
        match &self.0 {
            Representation::Narrow(bytes) => bytes.get(index).map(|byte| u32::from(*byte)),
            Representation::Wide(words) => words.get(index).copied(),
        }
    }

    /// The elements in the 0-based, half-open `start..end`; `None` when the
    /// range is not inside the string. A slice of a wide string whose
    /// elements all fit Latin-1 is narrowed, which keeps it canonical.
    pub fn slice(&self, start: usize, end: usize) -> Option<CodePoints> {
        match &self.0 {
            Representation::Narrow(bytes) => Some(CodePoints(Representation::Narrow(
                bytes.get(start..end)?.into(),
            ))),
            Representation::Wide(words) => Some(CodePoints::from_words(words.get(start..end)?)),
        }
    }

    /// Canonical without a scan: a wide operand holds a code point past
    /// U+00FF, and so does the result.
    pub fn concatenate(&self, other: &CodePoints) -> CodePoints {
        match (&self.0, &other.0) {
            (Representation::Narrow(first), Representation::Narrow(second)) => {
                let mut bytes = Vec::with_capacity(first.len() + second.len());
                bytes.extend_from_slice(first);
                bytes.extend_from_slice(second);
                CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
            }
            _ => {
                let mut words = Vec::with_capacity(self.size() + other.size());
                words.extend(self.elements());
                words.extend(other.elements());
                CodePoints(Representation::Wide(words.into_boxed_slice()))
            }
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = u32> + '_ {
        let (narrow, wide): (&[u8], &[u32]) = match &self.0 {
            Representation::Narrow(bytes) => (bytes, &[]),
            Representation::Wide(words) => (&[], words),
        };
        narrow
            .iter()
            .map(|byte| u32::from(*byte))
            .chain(wide.iter().copied())
    }

    /// Every element is a scalar value by construction, so the replacement
    /// character is never produced; it stands in for a panic.
    pub fn characters(&self) -> impl Iterator<Item = char> + '_ {
        self.elements()
            .map(|code_point| char::from_u32(code_point).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// The hash of the UTF-8 text, `hash::content_hash` of a `&str`, so that
    /// a String hashes as it did when it was one, and as the class names
    /// `builtins::value` hashes through `text::string_hash` do.
    pub fn content_hash(&self) -> i64 {
        match &self.0 {
            // ASCII is its own UTF-8.
            Representation::Narrow(bytes) if bytes.is_ascii() => {
                crate::hash::content_hash(std::str::from_utf8(bytes).unwrap_or_default())
            }
            _ => crate::hash::content_hash_of_characters(self.characters()),
        }
    }

    fn from_words(words: &[u32]) -> CodePoints {
        if words.iter().all(|word| *word <= 0xFF) {
            let bytes: Vec<u8> = words
                .iter()
                .filter_map(|word| u8::try_from(*word).ok())
                .collect();
            CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
        } else {
            CodePoints(Representation::Wide(words.into()))
        }
    }
}

/// Counts the code points first and allocates exactly, so that a decoded
/// string never holds more than 4 heap bytes per wire byte
/// (`docs/design/memory.md`, the decode amplification).
impl From<&str> for CodePoints {
    fn from(text: &str) -> CodePoints {
        if text.is_ascii() {
            return CodePoints(Representation::Narrow(text.as_bytes().into()));
        }
        let count = text.chars().count();
        if text.chars().all(|character| u32::from(character) <= 0xFF) {
            let mut bytes = Vec::with_capacity(count);
            bytes.extend(
                text.chars()
                    .filter_map(|character| u8::try_from(u32::from(character)).ok()),
            );
            CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
        } else {
            let mut words = Vec::with_capacity(count);
            words.extend(text.chars().map(u32::from));
            CodePoints(Representation::Wide(words.into_boxed_slice()))
        }
    }
}

/// An ASCII `String` hands its buffer over.
impl From<String> for CodePoints {
    fn from(text: String) -> CodePoints {
        if text.is_ascii() {
            return CodePoints(Representation::Narrow(text.into_bytes().into_boxed_slice()));
        }
        CodePoints::from(text.as_str())
    }
}

impl std::fmt::Display for CodePoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        for character in self.characters() {
            formatter.write_char(character)?;
        }
        Ok(())
    }
}

/// The text `{:?}` of a Rust `String` gives, so `Heap::describe` reads as
/// it did.
impl std::fmt::Debug for CodePoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.to_string(), formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::CodePoints;

    const TEXTS: [&str; 10] = [
        "",
        "a",
        "abcdefgh",
        "abcdefghi",
        "café",
        "naïve façade",
        "ÿ",
        "€uro",
        "😀 x",
        "Ã©",
    ];

    #[test]
    fn the_hash_is_the_hash_of_the_utf8_text() {
        for text in TEXTS {
            assert_eq!(
                CodePoints::from(text).content_hash(),
                crate::hash::content_hash(text),
                "{text}"
            );
        }
    }

    #[test]
    fn the_text_survives_the_round_trip() {
        for text in TEXTS {
            assert_eq!(CodePoints::from(text).to_string(), text);
            assert_eq!(CodePoints::from(text.to_string()).to_string(), text);
            assert_eq!(
                CodePoints::from(text).size(),
                text.chars().count(),
                "{text}"
            );
        }
    }

    #[test]
    fn latin1_text_is_narrow_whatever_built_it() {
        let wide = CodePoints::from("€abc");
        assert_eq!(wide.slice(1, 4), Some(CodePoints::from("abc")));
        assert_eq!(
            CodePoints::from("caf").concatenate(&CodePoints::from("é")),
            CodePoints::from("café")
        );
        assert_eq!(
            CodePoints::from_code_point(0xE9),
            Some(CodePoints::from("é"))
        );
        assert_ne!(CodePoints::from("€"), CodePoints::from("e"));
    }

    #[test]
    fn a_concatenation_with_a_wide_operand_is_wide_and_equal_to_its_literal() {
        let joined = CodePoints::from("café").concatenate(&CodePoints::from("€"));
        assert_eq!(joined, CodePoints::from("café€"));
        assert_eq!(
            joined.content_hash(),
            CodePoints::from("café€").content_hash()
        );
        assert_eq!(joined.code_point_at(4), Some(0x20AC));
    }

    #[test]
    fn only_scalar_values_are_code_points() {
        assert_eq!(CodePoints::from_code_point(0xD800), None);
        assert_eq!(CodePoints::from_code_point(0xDFFF), None);
        assert_eq!(CodePoints::from_code_point(0x11_0000), None);
        assert_eq!(
            CodePoints::from_code_point(0x10_FFFF).map(|code_points| code_points.size()),
            Some(1)
        );
        assert_eq!(
            CodePoints::from_code_point(0).map(|code_points| code_points.size()),
            Some(1)
        );
    }

    #[test]
    fn a_slice_outside_the_string_is_none() {
        let text = CodePoints::from("abc");
        assert_eq!(text.slice(1, 4), None);
        assert_eq!(text.slice(2, 1), None);
        assert_eq!(text.slice(3, 3), Some(CodePoints::from("")));
        assert_eq!(text.code_point_at(3), None);
    }
}
```

In `crates/ns-runtime/src/lib.rs`, declare the module after `pub mod bytecode;`:
```rust
pub mod code_points;
```
and re-export the type just before `pub use heap::{`:
```rust
pub use code_points::CodePoints;
```

In `crates/ns-runtime/src/hash.rs`, after `content_hash`, add the streamed form `CodePoints::content_hash` uses:
```rust
/// `content_hash` of the text `characters` spell, without building it:
/// `str`'s `Hash` writes the UTF-8 bytes in one `write`, which
/// `IdentityHasher` takes eight at a time, and then `0xff`.
pub(crate) fn content_hash_of_characters(characters: impl Iterator<Item = char>) -> i64 {
    let mut hasher = IdentityHasher::default();
    let mut word = [0u8; 8];
    let mut filled = 0;
    let mut encoded = [0u8; 4];
    for character in characters {
        for byte in character.encode_utf8(&mut encoded).bytes() {
            word[filled] = byte;
            filled += 1;
            if filled == 8 {
                hasher.add(u64::from_le_bytes(word));
                word = [0u8; 8];
                filled = 0;
            }
        }
    }
    if filled > 0 {
        hasher.add(u64::from_le_bytes(word));
    }
    hasher.write_u8(0xff);
    i64::from(hasher.finish() as u32)
}
```

- [ ] **Step 2: Run the unit tests.**

Run: `cargo nextest run -p ns-runtime --lib code_points`
Expected: the six tests PASS. `the_hash_is_the_hash_of_the_utf8_text` is the one that matters most: it pins that a String's hash does not change.

- [ ] **Step 3: Write the golden, which pins today's behaviour.**

`goldens/kernel/a_string_is_its_code_points.ns`:
```newspeak
Newspeak3
'Kernel'
(* A String is its code points (§5.1.5), stored one byte each while every
   one fits Latin-1 and 32 bits each otherwise
   (docs/design/core-library.md, "Strings are code points in two widths").
   The width is invisible: `size` counts code points whatever the width,
   `=` and `hash` agree across a concatenation that crosses widths, and
   the transcript and `printString` write UTF-8. The strings sit in slots,
   so the image round trip carries both widths. *)
class AStringIsItsCodePoints = Object new (
    | ascii = 'plain'.
      latin = 'café'.
      wide = '€uro 😀'. |
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        transcript show: ascii; newLine.
        transcript show: latin; newLine.
        transcript show: wide; newLine.
        transcript print: ascii size; newLine.
        transcript print: latin size; newLine.
        transcript print: wide size; newLine.
        transcript print: (latin, wide) size; newLine.
        transcript print: (latin, wide) = 'café€uro 😀'; newLine.
        transcript print: (latin, wide) hash = 'café€uro 😀' hash; newLine.
        transcript print: ('caf', 'é') = latin; newLine.
        transcript print: latin = 'cafe'; newLine.
        transcript show: wide printString; newLine.
        transcript show: wide asSymbol printString; newLine
    )
) : ()
```
`goldens/kernel/a_string_is_its_code_points.toml` (UTF-8):
```toml
stdout = '''
plain
café
€uro 😀
5
4
6
10
true
true
true
false
'€uro 😀'
#€uro 😀
'''
```

Run: `cargo nextest run -p ns-cli --test run_goldens a_string_is_its_code_points`
Expected: PASS already. This golden records what `size`, `,`, `=`, `hash`, `printString` and the transcript answer today, across both widths. The change below must keep it, and `golden_round_trips.rs` runs it through `build`/`load` too.

- [ ] **Step 4: Change the payload.**

`crates/ns-runtime/src/types.rs`, in `ObjectData`:
```rust
    String(crate::CodePoints),
```

`crates/ns-runtime/src/heap.rs`: replace `str_value` with this block, which keeps `str_value`'s signature:
```rust
    /// A String's text as UTF-8, for the boundaries that need a Rust
    /// `String`: an error text, the transcript, `asSymbol`, `printString`.
    /// The leaves that only read the elements use the accessors below,
    /// which copy out a size, a code point or a hash, not the whole text.
    pub fn str_value(&self, object: ObjectReference<'gc>) -> Option<String> {
        self.code_points(object)
            .map(|code_points| code_points.to_string())
    }

    /// The one lender of a String's payload, private to the heap, so that
    /// every public accessor copies out (`docs/design/memory.md`, "The
    /// seam").
    fn code_points(&self, object: ObjectReference<'gc>) -> Option<&crate::CodePoints> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::String(code_points) => Some(code_points),
            _ => None,
        }
    }

    pub fn is_string(&self, object: ObjectReference<'gc>) -> bool {
        self.code_points(object).is_some()
    }

    /// A String's number of code points; `None` for any other object.
    pub fn string_size(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.code_points(object).map(crate::CodePoints::size)
    }

    /// The code point at the 0-based `index`; `None` for any other object
    /// or past the end.
    pub fn string_code_point_at(&self, object: ObjectReference<'gc>, index: usize) -> Option<u32> {
        self.code_points(object)?.code_point_at(index)
    }

    /// The code points in the 0-based, half-open `start..end`; `None` for
    /// any other object or a range outside the string.
    pub fn string_slice(
        &self,
        object: ObjectReference<'gc>,
        start: usize,
        end: usize,
    ) -> Option<crate::CodePoints> {
        self.code_points(object)?.slice(start, end)
    }

    /// `first`'s code points followed by `second`'s; `None` unless both are
    /// Strings.
    pub fn string_concatenation(
        &self,
        first: ObjectReference<'gc>,
        second: ObjectReference<'gc>,
    ) -> Option<crate::CodePoints> {
        Some(
            self.code_points(first)?
                .concatenate(self.code_points(second)?),
        )
    }

    /// Whether two Strings hold the same code points; `None` unless both
    /// are Strings. The width is canonical, so this compares the payloads.
    pub fn strings_equal(
        &self,
        first: ObjectReference<'gc>,
        second: ObjectReference<'gc>,
    ) -> Option<bool> {
        Some(self.code_points(first)? == self.code_points(second)?)
    }

    /// A String's content hash; `None` for any other object.
    pub fn string_hash(&self, object: ObjectReference<'gc>) -> Option<i64> {
        self.code_points(object)
            .map(crate::CodePoints::content_hash)
    }
```
In `heap.rs`'s own tests, every `ObjectData::String("<text>".to_string())` becomes `ObjectData::String(crate::CodePoints::from("<text>"))`. There are 23 sites (`rg -n 'ObjectData::String\("' crates/ns-runtime/src/heap.rs`). No other file constructs `ObjectData::String` (`rg -n 'ObjectData::String\(' crates` shows the rest are match arms, which compile unchanged).

`crates/ns-runtime/src/interpreter/mod.rs`: `new_string` keeps its signature and gains a sibling that takes the payload:
```rust
    pub fn new_string(&mut self, string: String) -> ObjectReference<'gc> {
        self.new_string_from_code_points(crate::CodePoints::from(string))
    }

    pub fn new_string_from_code_points(
        &mut self,
        code_points: crate::CodePoints,
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::String);
        self.new_instance_object(class, ObjectData::String(code_points))
    }
```

`crates/ns-runtime/src/image/encode.rs`, in `encode_object`'s `ObjectData::String` arm:
```rust
        ObjectData::String(string) => {
            writer.write_u8(OBJECT_STRING);
            // The wire keeps UTF-8 (`docs/design/image-format.md`); the
            // width is the heap's, chosen afresh when the record is decoded.
            writer.write_str(&string.to_string());
        }
```

`crates/ns-runtime/src/image/decode.rs`: after `Reader::read_str`, add
```rust
    /// A String record's text, validated as UTF-8 where it lies and
    /// converted straight from the buffer: no intermediate `Vec` or
    /// `String`, so a record costs at most the 4 heap bytes per wire byte
    /// of a wide string (`docs/design/memory.md`, the decode
    /// amplification).
    pub fn read_code_points(&mut self) -> Result<crate::CodePoints, DecodeError> {
        let length = self.read_u32()? as usize;
        let bytes = self.read_raw(length)?;
        let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
        Ok(crate::CodePoints::from(text))
    }
```
and in `decode_graph_section`:
```rust
            OBJECT_STRING => ObjectData::String(reader.read_code_points()?),
```
`LITERAL_STRING` (a literal in compiled code) keeps `read_str`: `Literal::String` stays a Rust `String`.

`crates/ns-runtime/src/builtins/value.rs`, in `compare_node`:
```rust
            if mutator.heap().is_string(left) {
                return Some(Pair::Decided(text::text_equals(
                    mutator.heap(),
                    left,
                    Some(&second),
                )));
            }
```
and in `hash_node`:
```rust
            if let Some(hash) = mutator.heap().string_hash(object) {
                return Some(Node::Leaf(hash));
            }
```
`text::string_hash(&str)` stays: `hash_node` still hashes a value class object's name with it, and `CodePoints::content_hash` answers the same number for the same text.

`crates/ns-runtime/src/builtins/text.rs`:
- Delete `argument_string`. Concatenation reads both payloads through `string_concatenation`.
- `text_equals` loses its text:
  ```rust
  /// `String>>=`'s rule: an argument of the receiver's own class carrying
  /// the same code points. Named rather than written twice, so that
  /// `builtins::value`'s derived equality decides one exactly as the class's
  /// own `=` does instead of growing a second copy of the rule.
  pub(crate) fn text_equals<'gc>(
      heap: &Heap<'gc>,
      receiver: ObjectReference<'gc>,
      argument: Option<&Value<'gc>>,
  ) -> bool {
      match of_the_same_class(heap, receiver, argument) {
          Some(other) => heap.strings_equal(receiver, other) == Some(true),
          None => false,
      }
  }
  ```
- `execute_string` reads the size, not a clone:
  ```rust
      let Value::Object(object) = receiver else {
          return None;
      };
      // Declines for anything that is no String, before any leaf reads it.
      let size = mutator.heap.string_size(object)?;
      match primitive {
          StringPrimitive::Size => Some(Ok(Value::SmallInteger(size as i64))),
          StringPrimitive::Concatenate => {
              let joined = match arguments.first() {
                  Some(Value::Object(other)) => mutator.heap.string_concatenation(object, *other),
                  _ => None,
              };
              Some(match joined {
                  Some(code_points) => Ok(Value::Object(
                      mutator.new_string_from_code_points(code_points),
                  )),
                  None => Err(NewspeakError {
                      // … the existing error, unchanged …
                  }),
              })
          }
          StringPrimitive::Equal => {
              let equal = text_equals(&mutator.heap, object, arguments.first());
              // … the existing `declines_for_a_future` call, unchanged …
          }
          StringPrimitive::Hash => mutator
              .heap
              .string_hash(object)
              .map(|hash| Ok(Value::SmallInteger(hash))),
          StringPrimitive::AsSymbol => {
              let text = mutator.heap.str_value(object)?;
              Some(Ok(mutator.new_symbol(Symbol::intern(&text))))
          }
          StringPrimitive::PrintString => {
              let text = mutator.heap.str_value(object)?;
              Some(Ok(Value::Object(mutator.new_string(quote_string(&text)))))
          }
      }
  ```
  The two "unchanged" comments mark code that stays as it is today. Keep the existing error expression and the `declines_for_a_future` call verbatim.

- [ ] **Step 5: Run the per-commit gate, and the release suite for the size tests.**

Run the per-commit gate (Global Constraints) and `cargo nextest run --workspace --release`.
Expected:
- everything passes, clippy has no warning, and the corpus reports "parsed 211, failed 0";
- in the release run, `an_object_is_the_size_the_amplification_figure_assumes` passes unchanged: `CodePoints` is 24 bytes, as `String` is, so `Object` stays 56.

- [ ] **Step 6: Commit.**

```bash
git add crates/ns-runtime goldens/kernel/a_string_is_its_code_points.ns goldens/kernel/a_string_is_its_code_points.toml
git commit -m "perf: a String is its code points, one byte or four each"
```

---

### Task 2: `String>>at:`, `copyFrom:to:` and `String with:`

Json sends all three:
- **`at:`** answers a code point, 1-based. Primordial Soup's answers a UTF-8 byte (`KernelForPrimordialSoup.ns:2335`, primitive 117); the two agree for ASCII, and Json's input is ASCII.
- **`copyFrom:to:`** is Primordial Soup's `Bytes_copyStringFromTo` (`vm/primitives.cc:2698`): 1-based and inclusive. The range lies within the string, and the empty range is the one that stops just before it starts, at either end.
- **`String with:`** answers a one-character String. Primordial Soup's takes a byte (`String_class_with`, `primitives.cc:2725`); ours takes any code point.

`at:` and a non-Integer argument share `Array>>at:`'s error texts, so a program sees one text from either.

**Files:**
- Modify:
  - `crates/ns-runtime/src/builtins/primitive.rs`
  - `crates/ns-runtime/src/builtins/text.rs`
  - `crates/ns-runtime/src/builtins/collections.rs`
  - `crates/ns-runtime/core-library/Kernel.ns`
- Create:
  - `goldens/kernel/a_string_answers_its_code_points.ns`, `.toml`
  - `goldens/exceptions/a_string_index_error_is_caught.ns`, `.toml`

**Interfaces:**
- Consumes (from Task 1): `Heap::string_size`, `string_code_point_at`, `string_slice`, `Mutator::new_string_from_code_points`, `CodePoints::from_code_point`.
- Produces:
  - `StringPrimitive::ElementAt`, `CopyRange`, `FromCodePoint`;
  - the mirror selectors `string:at:ifFail:`, `string:copyFrom:to:ifFail:`, `stringClass:with:ifFail:`;
  - `String>>at:`, `String>>copyFrom:to:` and `String class>>with:`;
  - `collections::out_of_range` and `collections::not_an_integer` as `pub(crate)`.

- [ ] **Step 1: Write the two goldens.**

`goldens/kernel/a_string_answers_its_code_points.ns`:
```newspeak
Newspeak3
'Kernel'
(* Primordial Soup's `at:`, `copyFrom:to:` and `String with:`, over
   ns-next's code points: `at:` answers the Integer code point, 1-based;
   `copyFrom:to:` is inclusive and takes the empty range that stops just
   before it starts; `String with:` takes any code point, where Primordial
   Soup's takes a byte, since an ns-next String's elements are code points
   (docs/design/core-library.md, "Strings are code points in two widths").
   A copy whose code points all fit Latin-1 is equal to, and hashes like,
   the same text written as a literal, whatever width it was cut from. *)
class AStringAnswersItsCodePoints = Object new (
    | wide = '€uro 😀'. |
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        transcript print: ('abc' at: 1); show: ' '; print: ('café' at: 4); show: ' '; print: (wide at: 1); show: ' '; print: (wide at: 6); newLine.
        transcript show: ('hello' copyFrom: 2 to: 4); newLine.
        transcript print: ('hello' copyFrom: 6 to: 5) size; show: ' '; print: ('hello' copyFrom: 1 to: 0) size; show: ' '; print: ('' copyFrom: 1 to: 0) size; newLine.
        transcript show: (wide copyFrom: 2 to: 4); show: ' '; print: (wide copyFrom: 2 to: 4) = 'uro'; show: ' '; print: (wide copyFrom: 2 to: 4) hash = 'uro' hash; newLine.
        transcript show: (wide copyFrom: 5 to: 6); show: ' '; print: (wide copyFrom: 5 to: 6) size; newLine.
        transcript show: (String with: 97); show: (String with: 8364); show: (String with: 128512); newLine.
        transcript print: (String with: 233) = 'é'; show: ' '; print: (String with: 0) size; newLine
    )
) : ()
```
`goldens/kernel/a_string_answers_its_code_points.toml` (UTF-8; the fifth line starts with a space):
```toml
stdout = '''
97 233 8364 128512
ell
0 0 0
uro true true
 😀 2
a€😀
true 1
'''
```
`goldens/exceptions/a_string_index_error_is_caught.ns`:
```newspeak
Newspeak3
'Test'
(* A String's `at:` shares `Array>>at:`'s errors, so a program sees one
   text from either; `copyFrom:to:` refuses a range that leaves the string
   or stops more than one before it starts, without overflowing on the
   largest and smallest SmallInteger; `String with:` refuses what is no
   code point -- a negative Integer, a surrogate, anything past U+10FFFF,
   and what is no Integer. Each is a signalled Error, caught here. *)
class AStringIndexErrorIsCaught = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. largest smallest |
        largest:: 9223372036854775807.
        smallest:: 0 - largest - 1.
        transcript show: (['abc' at: 4] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' at: 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' at: smallest] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' at: nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: 0 to: 2] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: 2 to: 4] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: 3 to: 1] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: smallest to: largest] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: 'a' to: 2] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([String with: -1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([String with: 55296] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([String with: 1114112] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([String with: nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' at: 4] on: Error do: [:error | error class name]); newLine
    )
) : ()
```
`goldens/exceptions/a_string_index_error_is_caught.toml`:
```toml
stdout = '''
index 4 out of range: valid indices are 1..3 (size 3)
index 0 out of range: valid indices are 1..3 (size 3)
index -9223372036854775808 out of range: valid indices are 1..3 (size 3)
at: requires an Integer index, got nil
copyFrom: 0 to: 2 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
copyFrom: 2 to: 4 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
copyFrom: 3 to: 1 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
copyFrom: -9223372036854775808 to: 9223372036854775807 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
copyFrom:to: requires an Integer index, got the String "a"
with: requires a Unicode code point, an Integer from 0 to 1114111 outside 55296 to 57343, got -1
with: requires a Unicode code point, an Integer from 0 to 1114111 outside 55296 to 57343, got 55296
with: requires a Unicode code point, an Integer from 0 to 1114111 outside 55296 to 57343, got 1114112
with: requires a Unicode code point, an Integer from 0 to 1114111 outside 55296 to 57343, got nil
Error
'''
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens a_string_answers_its_code_points a_string_index_error_is_caught`
Expected: FAIL, with `an object does not understand #at:` (or `#with:`).

- [ ] **Step 3: Add the leaves.**

`crates/ns-runtime/src/builtins/collections.rs`: make `out_of_range` and `not_an_integer` `pub(crate)`.

`crates/ns-runtime/src/builtins/primitive.rs`:
- **The enum.** In `StringPrimitive`, after `AsSymbol`:
  ```rust
      ElementAt,
      CopyRange,
      /// `String with: code`, on `String`'s class side: its subject is the
      /// class.
      FromCodePoint,
  ```
- **`answers_false_to_a_stranger`.** Add all three to the String arm's `=> false` group: each raises on a wrong argument rather than answering.
- **The mirror rows.** Append three rows to `MIRROR`, after `floatSquareRoot:ifFail:`:
  ```rust
      mirror(
          "string:at:ifFail:",
          "at:",
          Primitive::String(StringPrimitive::ElementAt),
      ),
      mirror(
          "string:copyFrom:to:ifFail:",
          "copyFrom:to:",
          Primitive::String(StringPrimitive::CopyRange),
      ),
      mirror(
          "stringClass:with:ifFail:",
          "with:",
          Primitive::String(StringPrimitive::FromCodePoint),
      ),
  ```

`crates/ns-runtime/src/builtins/text.rs`:
- **The import.** Add `use crate::builtins::collections;`.
- **The two functions.** Add these before `execute_string`:
  ```rust
  /// `String with: code`: a one-element String. Primordial Soup's takes a
  /// byte (`String_class_with`); this takes any code point, since an ns-next
  /// String's elements are code points. Declines for a subject other than
  /// `String` itself, as `Array new:` does.
  fn from_code_point<'gc>(
      mutator: &mut Mutator<'gc>,
      receiver: Value<'gc>,
      selector: Symbol,
      arguments: &[Value<'gc>],
  ) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
      let Value::Object(class) = receiver else {
          return None;
      };
      if mutator.heap.class_builtin(class) != Some(BuiltinClass::String) {
          return None;
      }
      let code_points = match arguments.first() {
          Some(Value::SmallInteger(code)) => u32::try_from(*code)
              .ok()
              .and_then(crate::CodePoints::from_code_point),
          _ => None,
      };
      Some(match code_points {
          Some(code_points) => Ok(Value::Object(
              mutator.new_string_from_code_points(code_points),
          )),
          None => Err(NewspeakError {
              selector,
              receiver,
              message: format!(
                  "{selector} requires a Unicode code point, an Integer from 0 to 1114111 \
                   outside 55296 to 57343, got {}",
                  mutator.heap.describe_argument(arguments.first())
              ),
          }),
      })
  }

  /// `copyFrom: start to: stop`, 1-based and inclusive, as Primordial Soup's
  /// `Bytes_copyStringFromTo`: the range lies within the string, and the
  /// empty range is the one that stops just before it starts, at either end.
  fn copy_range<'gc>(
      mutator: &mut Mutator<'gc>,
      object: ObjectReference<'gc>,
      size: usize,
      receiver: Value<'gc>,
      selector: Symbol,
      arguments: &[Value<'gc>],
  ) -> Result<Value<'gc>, NewspeakError<'gc>> {
      let bound = |argument: Option<&Value<'gc>>| match argument {
          Some(Value::SmallInteger(bound)) => Ok(*bound),
          other => Err(collections::not_an_integer(
              &mutator.heap,
              &receiver,
              selector,
              other,
          )),
      };
      let start = bound(arguments.first())?;
      let stop = bound(arguments.get(1))?;
      // `start` is tested first, so that `start - 1` cannot overflow; the
      // slice refuses a `stop` past the end.
      let slice = if start >= 1 && stop >= start - 1 {
          usize::try_from(stop).ok().and_then(|stop| {
              mutator
                  .heap
                  .string_slice(object, (start - 1) as usize, stop)
          })
      } else {
          None
      };
      match slice {
          Some(code_points) => Ok(Value::Object(
              mutator.new_string_from_code_points(code_points),
          )),
          None => Err(NewspeakError {
              selector,
              receiver,
              message: format!(
                  "copyFrom: {start} to: {stop} out of range: valid ranges lie within \
                   1..{size}, an empty range ends just before it starts (size {size})"
              ),
          }),
      }
  }
  ```
- **`execute_string`.** Its head becomes the following; `FromCodePoint`'s subject is a class, so it is answered before the String check:
  ```rust
      if primitive == StringPrimitive::FromCodePoint {
          return from_code_point(mutator, receiver, selector, arguments);
      }
      let Value::Object(object) = receiver else {
          return None;
      };
      // Declines for anything that is no String, before any leaf reads it.
      let size = mutator.heap.string_size(object)?;
      match primitive {
          StringPrimitive::FromCodePoint => None,
          // Array's errors, so that a program sees one text from either.
          StringPrimitive::ElementAt => Some(match arguments.first() {
              Some(Value::SmallInteger(index)) => {
                  match usize::try_from(*index)
                      .ok()
                      .and_then(|index| index.checked_sub(1))
                      .and_then(|position| mutator.heap.string_code_point_at(object, position))
                  {
                      Some(code_point) => Ok(Value::SmallInteger(i64::from(code_point))),
                      None => Err(collections::out_of_range(&receiver, selector, *index, size)),
                  }
              }
              other => Err(collections::not_an_integer(
                  &mutator.heap,
                  &receiver,
                  selector,
                  other,
              )),
          }),
          StringPrimitive::CopyRange => Some(copy_range(
              mutator, object, size, receiver, selector, arguments,
          )),
          StringPrimitive::Size => Some(Ok(Value::SmallInteger(size as i64))),
          // … Concatenate, Equal, Hash, AsSymbol and PrintString as Task 1 left them …
  ```

`crates/ns-runtime/core-library/Kernel.ns`:
- **The instance side.** In `class String`, after `size`:
  ```newspeak
          (* Primordial Soup's `at:` and `copyFrom:to:`, over code points:
             `at:` answers the code point, where Primordial Soup's answers a
             UTF-8 byte -- the same for ASCII. Json sends both. *)
          public at: index = ( ^vmMirror string: self at: index ifFail: [^super at: index] )
          public copyFrom: start to: stop = ( ^vmMirror string: self copyFrom: start to: stop ifFail: [^super copyFrom: start to: stop] )
  ```
- **The class side.** `String`'s instance side ends with `public asString = ( ^self )`. Close it and add a class side:
  ```newspeak
          public asString = ( ^self )
      ) : (
          (* A one-character String: Primordial Soup's `String with:`, which
             takes a byte, over code points. The leaf declines only a subject
             that is not `String` itself, which a flat class side cannot be
             sent from. Json's escapes send it. *)
          public with: code = ( ^vmMirror stringClass: self with: code ifFail: [nil] )
      )
  ```

- [ ] **Step 4: Run the goldens and the per-commit gate.**

Expected: PASS. `every_kernel_method_has_the_fast_path_it_should` (`interpreter/kernel.rs`) passes unchanged: each new method is a forwarder named after its row's reported selector.

- [ ] **Step 5: Commit.**

```bash
git add crates/ns-runtime goldens/kernel/a_string_answers_its_code_points.* goldens/exceptions/a_string_index_error_is_caught.*
git commit -m "feat: a String answers at:, copyFrom:to: and String with:"
```

---

### Task 3: `Float>>sin`, `cos` and `asInteger`; `timesRepeat:` and `max:`

The selectors and their senders:
- **`sin` and `cos`** (CD) are Primordial Soup's `FLOAT_FUNCTION_1(sin)` and `(cos)` (`primitives.cc:1283`), the C library's functions, which Rust's `f64::sin` and `cos` call too.
- **`asInteger`** (CD) is Primordial Soup's `Number_asInteger` for a Float (`primitives.cc:944`, `LargeInteger::FromDouble`): exact truncation toward zero at any magnitude, failing for a NaN or an infinity.
- **`timesRepeat:`** (DeltaBlue, Havlak) is Primordial Soup's Newspeak body (`KernelForPrimordialSoup.ns:1738`).
- **`max:`** (Havlak) is also Primordial Soup's Newspeak body (2054).

`Number>>sin`, `cos`, `asInteger` for an integer receiver and `min:` are not added: no benchmark sends them.

**Files:**
- Modify:
  - `crates/ns-runtime/src/arbitrary_integer/mod.rs`
  - `crates/ns-runtime/src/builtins/numbers.rs`
  - `crates/ns-runtime/src/builtins/primitive.rs`
  - `crates/ns-runtime/src/interpreter/kernel.rs` (its test's `NOT_FORWARDERS`)
  - `crates/ns-runtime/core-library/Kernel.ns`
- Create:
  - `goldens/arithmetic/float_sine_cosine_and_truncation.ns`, `.toml`
  - `goldens/kernel/times_repeat_and_max.ns`, `.toml`

**Interfaces:**
- Produces:
  - `ArbitraryInteger::from_f64_truncated(f64) -> Option<ArbitraryInteger>`;
  - `FloatPrimitive::Sine`, `Cosine`, `AsInteger`;
  - the mirror selectors `floatSine:ifFail:`, `floatCosine:ifFail:`, `floatAsInteger:ifFail:`;
  - `Float>>sin`, `cos`, `asInteger`;
  - `Integer>>timesRepeat:`;
  - `Number>>max:`.

- [ ] **Step 1: Write the two goldens.**

`goldens/arithmetic/float_sine_cosine_and_truncation.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `sin` and `cos` are Primordial Soup's `Float>>sin` and `cos`, the C
   library's functions of radians; only values exact on every platform are
   printed, the others are bounded. `asInteger` is Primordial Soup's
   `Float>>asInteger`: it truncates toward zero, exactly at any magnitude,
   into a LargeInteger beyond 64 bits, and refuses a NaN and an infinity.
   CD sends all three. *)
class FloatSineCosineAndTruncation = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. sine cosine |
        transcript print: 0.0 sin; show: ' '; print: -0.0 sin; show: ' '; print: 0.0 cos; newLine.
        sine:: 0.5 sin.
        cosine:: 0.5 cos.
        transcript print: (sine > 0.4794 and: [sine < 0.4795]); show: ' '; print: (cosine > 0.8775 and: [cosine < 0.8776]); newLine.
        transcript print: (0.0 / 0.0) sin; show: ' '; print: (1.0 / 0.0) cos; newLine.
        transcript print: 3.7 asInteger; show: ' '; print: -3.7 asInteger; show: ' '; print: 0.5 asInteger; show: ' '; print: -0.0 asInteger; newLine.
        transcript print: 100000000000000000000 asFloat asInteger; show: ' '; print: 100000000000000000000 asFloat asInteger class; newLine.
        transcript print: 9223372036854775808 asFloat asInteger; show: ' '; print: (0 - 9223372036854775808) asFloat asInteger; show: ' '; print: (0 - 9223372036854775808) asFloat asInteger class; newLine.
        transcript show: ([(0.0 / 0.0) asInteger] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([(-1.0 / 0.0) asInteger] on: Error do: [:error | error messageText]); newLine
    )
) : ()
```
`goldens/arithmetic/float_sine_cosine_and_truncation.toml`:
```toml
stdout = '''
0 -0 1
true true
NaN NaN
3 -3 0 0
100000000000000000000 LargeInteger
9223372036854775808 -9223372036854775808 Integer
asInteger requires a finite Float, got NaN
asInteger requires a finite Float, got -inf
'''
```
`goldens/kernel/times_repeat_and_max.ns`:
```newspeak
Newspeak3
'Kernel'
(* `timesRepeat:` is Primordial Soup's `Integer>>timesRepeat:`, which
   evaluates its block the receiver's number of times and answers nil;
   `max:` is its `Number>>max:`, for any two numbers the ordering
   compares. DeltaBlue and Havlak send them. *)
class TimesRepeatAndMax = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. count |
        count:: 0.
        5 timesRepeat: [count:: count + 1].
        0 timesRepeat: [count:: count + 100].
        -3 timesRepeat: [count:: count + 100].
        transcript print: count; show: ' '; print: (2 timesRepeat: []); newLine.
        transcript print: (3 max: 7); show: ' '; print: (7 max: 3); show: ' '; print: (-1 max: -2); newLine.
        transcript print: (2.5 max: 1); show: ' '; print: (1 max: 2.5); show: ' '; print: (18446744073709551616 max: 1); newLine
    )
) : ()
```
`goldens/kernel/times_repeat_and_max.toml`:
```toml
stdout = '''
5 nil
7 7 -1
2.5 2.5 18446744073709551616
'''
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens float_sine_cosine_and_truncation times_repeat_and_max`
Expected: FAIL with `does not understand #sin` and `#timesRepeat:`.

- [ ] **Step 3: Add the conversion, the leaves and the methods.**

`crates/ns-runtime/src/arbitrary_integer/mod.rs`, before `is_negative`:
```rust
    /// The integer `value` truncates to, toward zero, exactly; `None` for a
    /// NaN or an infinity. Primordial Soup's `Float>>asInteger`
    /// (`LargeInteger::FromDouble`).
    pub fn from_f64_truncated(value: f64) -> Option<ArbitraryInteger> {
        if !value.is_finite() {
            return None;
        }
        let truncated = value.trunc();
        // Below 2^63 in magnitude, `as` converts a whole `f64` exactly.
        if truncated.abs() < 9_223_372_036_854_775_808.0 {
            return Some(ArbitraryInteger::from_i64(truncated as i64));
        }
        // Beyond, the value is its 53-bit significand times a power of two
        // of at least 2^11, which `shift_left` multiplies out.
        let bits = truncated.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) - 1075;
        let significand = (bits & ((1 << 52) - 1)) | (1 << 52);
        let magnitude =
            ArbitraryInteger::from_parts(false, vec![significand]).shift_left(exponent)?;
        Some(if truncated < 0.0 {
            magnitude.negated()
        } else {
            magnitude
        })
    }
```

`crates/ns-runtime/src/builtins/primitive.rs`:
- **The enum.** In `FloatPrimitive`, after `SquareRoot`, add `Sine`, `Cosine` and `AsInteger`.
- **`answers_false_to_a_stranger`.** Add the three to the Float arm's `=> false` group.
- **The mirror rows.** Append to `MIRROR`, after Task 2's rows:
  ```rust
      mirror(
          "floatSine:ifFail:",
          "sin",
          Primitive::Float(FloatPrimitive::Sine),
      ),
      mirror(
          "floatCosine:ifFail:",
          "cos",
          Primitive::Float(FloatPrimitive::Cosine),
      ),
      mirror(
          "floatAsInteger:ifFail:",
          "asInteger",
          Primitive::Float(FloatPrimitive::AsInteger),
      ),
  ```

`crates/ns-runtime/src/builtins/numbers.rs`, in `execute_float`, after `SquareRoot`:
```rust
        // Primordial Soup's are the C library's `sin` and `cos`; Rust's
        // call the same functions. No benchmark result depends on their
        // last bit (CD was checked against correctly rounded values).
        FloatPrimitive::Sine => Some(Ok(Value::Float(receiver_float.sin()))),
        FloatPrimitive::Cosine => Some(Ok(Value::Float(receiver_float.cos()))),
        FloatPrimitive::AsInteger => Some(
            match crate::ArbitraryInteger::from_f64_truncated(receiver_float) {
                Some(integer) => Ok(mutator.new_integer(integer)),
                None => Err(NewspeakError {
                    selector: Symbol::intern("asInteger"),
                    receiver,
                    message: format!("asInteger requires a finite Float, got {receiver_float}"),
                }),
            },
        ),
```

`crates/ns-runtime/core-library/Kernel.ns`:
- **`Number`.** After `to:by:do:`:
  ```newspeak
          (* Primordial Soup's `Number>>max:`, for any two numbers the
             ordering compares. Havlak sends it. *)
          public max: other = ( ^self > other ifTrue: [self] ifFalse: [other] )
  ```
- **`Integer`.** After `bitInvert`:
  ```newspeak
          (* Primordial Soup's `Integer>>timesRepeat:`, which its compiler
             inlines and ours sends. DeltaBlue and Havlak send it. *)
          public timesRepeat: action = (
              1 to: self do: [:ignored | action value].
              ^nil
          )
  ```
- **`Float`.** After `sqrt`:
  ```newspeak
          (* Primordial Soup's `sin`, `cos` and `asInteger`, which truncates
             toward zero. CD sends all three. *)
          public sin = ( ^vmMirror floatSine: self ifFail: [^super sin] )
          public cos = ( ^vmMirror floatCosine: self ifFail: [^super cos] )
          public asInteger = ( ^vmMirror floatAsInteger: self ifFail: [^super asInteger] )
  ```

`crates/ns-runtime/src/interpreter/kernel.rs`: `every_kernel_method_has_the_fast_path_it_should` fails for the two Newspeak bodies until they are named in `NOT_FORWARDERS`. Add them after `(BuiltinClass::Integer, "bitInvert"),`:
```rust
        // Primordial Soup's Newspeak bodies: `timesRepeat:` sends `to:do:`
        // and `value`, and `max:` sends `>` to `self`, which no leaf could
        // do.
        (BuiltinClass::Integer, "timesRepeat:"),
        (BuiltinClass::Number, "max:"),
```

- [ ] **Step 4: Run the goldens and the per-commit gate.**

Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/ns-runtime goldens/arithmetic/float_sine_cosine_and_truncation.* goldens/kernel/times_repeat_and_max.*
git commit -m "feat: Float answers sin, cos and asInteger; Integer timesRepeat: and Number max:"
```

---

### Task 4: `List` is Primordial Soup's `List`, with a first index

This task writes `CollectionsFactory.ns` with the new `List` (printed whole in Step 3). Task 5 then adds `Map`, `Set`, their two facet accessors and their test, and prints the file again whole.

**What `List` becomes,** with Primordial Soup's meaning for every selector (`Collections.ns`, `List`):
- **The factory.** The primary factory is `new: capacity`, with a class-side `new` (`^self new: 3`). So a program may write `List new: 15` in a superclass clause, as DeltaBlue's `Plan` does. A clause `List new ()` no longer names the primary factory.
- **The selectors the benchmarks send:**
  - `isEmpty`, `removeFirst` and `removeAll`;
  - `removeLast` (DeltaBlueTon80);
  - `remove:ifAbsent:`, which removes the first element `=` to its argument;
  - `anySatisfy:`;
  - `sort:`, which takes a block answering whether its first argument may precede its second, and is a stable merge sort, `Array>>sortFrom:to:by:`'s, adapted.
- **The first index.** The elements live from `firstIndex` on, so `removeFirst` moves nothing and a queue of `add:` and `removeFirst` runs in amortized constant time, as SOMns's `Vector` does. Primordial Soup's `removeFirst` shifts every element: about 52 s for DeltaBlue at 12000, against 0.9 s.
- **A private representation.** `data`, `firstIndex` and every helper are `private`, where Primordial Soup's are protected, so the representation can change without a subclass noticing (decision 10). `size_` alone stays protected.
- **Growth.** When `add:` finds the end full:
  - the elements move to the front of the same array if at least as many slots lie empty before them as they fill;
  - otherwise they move into an array half as large again, Primordial Soup's growth: 3 → 7 → 15 → 22 → ….

**Files:**
- Modify:
  - `crates/ns-runtime/core-library/CollectionsFactory.ns`
  - `goldens/collections/a_user_subclass_of_list_inherits_its_protocol.ns`
  - `goldens/collections/a_list_grows_past_its_first_array.ns` (comment)
  - `goldens/collections/a_list_index_error_is_caught_as_an_error.ns` (comment)
  - `goldens/nesting/nested_override.ns`
- Create: `goldens/collections/a_list_is_a_queue_a_stack_and_sorts.ns`, `.toml`

**Interfaces:**
- Produces: `List new: capacity`, class-side `new`, and `isEmpty`, `removeFirst`, `removeLast`, `remove:ifAbsent:`, `removeAll`, `anySatisfy:`, `sort:`. Error texts: `'removeFirst: the List is empty'`, `'removeLast: the List is empty'`.

- [ ] **Step 1: Write the golden.**

`goldens/collections/a_list_is_a_queue_a_stack_and_sorts.ns`:
```newspeak
Newspeak3
'Collections'
(* Primordial Soup's List protocol, as the Are We Fast Yet benchmarks of
   stage 3 send it (docs/design/core-library.md, "The collections
   module"): a queue of `add:` and `removeFirst` that runs past many
   growths and moves keeps its order, `removeLast` makes it a stack,
   `remove:ifAbsent:` removes the first element `=` to its argument,
   `sort:` takes a block answering whether its first argument may precede
   its second and keeps equal elements in order -- so a strict comparison
   puts them in reverse order, as in Primordial Soup -- and `new: capacity` is
   the primary factory, so a class of the program may name `List new: 15`
   in its superclass clause. *)
class AListIsAQueueAStackAndSorts = Object new (
    | private List ::= nil. |
) (
    class Plan = List new: 15 () (
        public total = (
            | sum |
            sum:: 0.
            self do: [:each | sum:: sum + each].
            ^sum
        )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript.
          Error = platform kernel Error.
          queue last stack list pairs line plan sum |
        List:: platform collections List.
        queue:: List new.
        transcript print: queue isEmpty; newLine.
        1 to: 1000 do: [:index |
            queue add: index.
            index \\ 3 = 0 ifTrue: [queue removeFirst]].
        transcript print: queue size; show: ' '; print: (queue at: 1); show: ' '; print: (queue at: 667); newLine.
        transcript print: queue removeFirst; show: ' '; print: (queue at: 1); newLine.
        sum:: 0.
        queue do: [:each | sum:: sum + each].
        transcript print: sum; newLine.
        queue at: 1 put: 0.
        transcript print: (queue at: 1); show: ' '; print: (queue anySatisfy: [:each | each = 0]); show: ' '; print: (queue anySatisfy: [:each | each > 1000]); newLine.
        [queue isEmpty] whileFalse: [last:: queue removeFirst].
        transcript print: last; show: ' '; print: queue size; newLine.
        queue add: 7.
        transcript print: (queue at: 1); show: ' '; print: queue size; newLine.

        stack:: List new: 0.
        1 to: 20 do: [:index | stack add: index].
        transcript print: stack removeLast; show: ' '; print: stack removeLast; show: ' '; print: stack size; show: ' '; print: (stack at: 18); newLine.

        list:: List new.
        list add: 10; add: 20; add: 30; add: 20; add: 40.
        transcript print: (list remove: 20 ifAbsent: [nil]); show: ' '; show: (list remove: 99 ifAbsent: ['absent']); newLine.
        list remove: 10 ifAbsent: [nil].
        list remove: 40 ifAbsent: [nil].
        line:: ''.
        list do: [:each | line:: line, each printString, ' '].
        transcript show: line; print: list size; newLine.
        list add: 'ab'.
        transcript show: (list remove: 'a', 'b' ifAbsent: ['absent']); show: ' '; print: list size; newLine.
        list removeAll.
        transcript print: list size; show: ' '; print: list isEmpty; newLine.
        list add: 5.
        transcript print: (list at: 1); newLine.

        pairs:: List new.
        pairs add: {3. 'a'}; add: {1. 'b'}; add: {3. 'c'}; add: {2. 'd'}; add: {1. 'e'}.
        pairs sort: [:first :second | (first at: 1) <= (second at: 1)].
        line:: ''.
        pairs do: [:each | line:: line, (each at: 2)].
        transcript show: line; newLine.
        pairs sort: [:first :second | (first at: 1) > (second at: 1)].
        line:: ''.
        pairs do: [:each | line:: line, (each at: 2)].
        transcript show: line; newLine.

        plan:: Plan new.
        plan add: 4; add: 5.
        transcript print: plan total; show: ' '; print: plan size; newLine.

        transcript show: ([List new removeFirst] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([List new removeLast] on: Error do: [:error | error messageText]); newLine
    )
) : ()
```
`goldens/collections/a_list_is_a_queue_a_stack_and_sorts.toml`:
```toml
stdout = '''
true
667 334 1000
334 335
444555
0 true false
1000 0
7 1
20 19 18 18
20 absent
30 20 2
ab 2
0 true
5
bedac
cadeb
9 2
removeFirst: the List is empty
removeLast: the List is empty
'''
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens a_list_is_a_queue_a_stack_and_sorts`
Expected: FAIL (`class Plan = List new: 15` sends `new:`, which `List`'s primary factory is not yet).

- [ ] **Step 3: Write `CollectionsFactory.ns`.** The whole file:
```newspeak
Newspeak3
'CollectionsFactory'
(* The Core Library's collections module (§8.3: the platform "provides
   accessors to various modules"; docs/design/core-library.md, "The
   collections module"), in the factory pattern `PlatformFactory.ns` is
   written in.

   This root holds what is privileged -- the kernel instance -- and nothing
   hands its instance to a program: `PlatformFactory`'s root builds it once,
   through the mirror's `moduleNamed:`, sends it `newCollections`, its one
   public member, and keeps it in a private lazy slot. What a program gets
   is the `Collections` facet `newCollections` answers, which hands out this
   module's classes by class accessors, each exactly `^outer
   CollectionsFactory <Name>` -- a lexical send, which reaches the root's
   private classes (§5.9), and which an image names by its path from the
   platform (`docs/design/image-format.md`, "The platform path section").
   Nothing a program can name answers this root's instance or its class, so
   no program can build one over something that is not the kernel. Keep it
   so: no code whose receiver is this root's instance may send it anything
   it does not understand, since a `MessageNotUnderstood` hands its
   receiver to whoever catches it.

   A value (§3.1.1), so that its facet is one and the platform, a value,
   may hold the facet in a slot.

   `List` is adapted from Primordial Soup's Collections.ns
   (https://github.com/rmacnak/primordialsoup, commit
   338f0bb342a618f5608ab72ab1b1fae9b226e472, newspeak/Collections.ns), and
   `List>>sort:`'s merge sort from `Array>>sortFrom:to:by:` in its
   newspeak/KernelForPrimordialSoup.ns. Changed for ns-next: only the
   selectors the corpus, the goldens and the Are We Fast Yet benchmarks
   send (the on-demand rule); `List` keeps the index of its first element,
   so that `removeFirst` runs in amortized constant time, as SOMns's
   `Vector` does, where Primordial Soup's shifts every element; a
   `removeAll` keeps the capacity, as the Are We Fast Yet `Vector` does;
   errors are `kernelModule Error`s with texts of their own, where
   Primordial Soup sends the undefined `errorEmpty`; `>>` and `|` on
   integers are written with `//` and `bitOr:`; names are written out.

   Copyright 2008, the Newspeak project authors.

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root. *)
class CollectionsFactory usingKernel: theKernel = Object <: Value (
    | private kernelModule = theKernel. |
) (
    (* `platform collections`: the module's namespace. It names its own
       class as `Platform` does, so that the class has a path too. `List`
       is here because `Ducts.ns`, `NewspeakASTs.ns` and
       `V5BytecodeSimulation.ns` on `corpus/runnable.toml` bind `platform
       collections List` (docs/design/core-library.md, "The on-demand
       rule"). *)
    private class Collections = Value new () (
        public Collections = ( ^outer CollectionsFactory Collections )
        public List = ( ^outer CollectionsFactory List )
    )
    (* An ordered, growable sequence: Primordial Soup's `List`, with the
       protocol the corpus, the goldens and the Are We Fast Yet benchmarks
       ask for (the on-demand rule, docs/design/core-library.md). The
       elements live in `data` from `firstIndex` on, `size_` of them, so
       that `removeFirst` moves nothing and a queue of `add:` and
       `removeFirst` runs in amortized constant time, as SOMns's `Vector`
       keeps its `firstIdx`. When `add:` finds the array full at its end, the
       elements move to its start if at least half of it lies empty before
       them, and otherwise into an Array half as large again; nothing grows
       in place. The primary factory takes the capacity, as Primordial
       Soup's does, so a program may write `List new: 15` as a superclass
       clause, as DeltaBlue's `Plan` does. The representation is private,
       where Primordial Soup's is protected, so that it can change -- to a
       ring buffer, say -- without a subclass noticing; `size_` alone stays
       protected, as a subclass's view of the size. *)
    private class List new: capacity = Object new (
        | protected size_ ::= 0.
          private firstIndex ::= 1.
          private data ::= Array new: capacity. |
    ) (
        public size = ( ^size_ )
        (* `benchmarks/are-we-fast-yet/DeltaBlue.ns`, `DeltaBlueTon80.ns`
           and `Havlak.ns` ask whether a work list is empty. *)
        public isEmpty = ( ^0 = size_ )
        (* Answers the element, as Smalltalk's and Newspeak's `add:` do. *)
        public add: element = (
            | end |
            end:: firstIndex + size_.
            end > data size ifTrue: [end:: makeRoomAtEnd].
            data at: end put: element.
            size_:: size_ + 1.
            ^element
        )
        (* The index check is written out here rather than sent, since
           every `at:` pays for it. An index that is no Integer is out of
           range too, rather than an error of the comparison. *)
        public at: index = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            ^data at: firstIndex + index - 1
        )
        (* Answers the value, as `add:` answers the element. *)
        public at: index put: value = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            data at: firstIndex + index - 1 put: value.
            ^value
        )
        (* Visits the elements the list holds when it starts: the size is
           read once, so an `add:` inside the block does not extend the
           walk, while `data` and `firstIndex` are read for every element,
           so an `at:put:` inside it is seen, as in Primordial Soup's `1 to:
           size_ do:`. *)
        public do: action = (
            | end index |
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                action value: (data at: firstIndex + index).
                index:: index + 1]
        )
        (* Primordial Soup's `Collection>>anySatisfy:`, a walk like `do:`'s
           written out, which needs no non-local return.
           `benchmarks/are-we-fast-yet/Havlak.ns` sends it. *)
        public anySatisfy: predicate = (
            | end index |
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                (predicate value: (data at: firstIndex + index)) ifTrue: [^true].
                index:: index + 1].
            ^false
        )
        (* Moves nothing: the vacated slot is cleared, so that the list
           keeps no element alive, and an emptied list starts its next
           element at the front again. `benchmarks/are-we-fast-yet/DeltaBlue.ns`
           and `Havlak.ns` use a list as a queue. *)
        public removeFirst = (
            | element |
            0 = size_ ifTrue: [^kernelModule Error signal: 'removeFirst: the List is empty'].
            element:: data at: firstIndex.
            data at: firstIndex put: nil.
            size_:: size_ - 1.
            0 = size_
                ifTrue: [firstIndex:: 1]
                ifFalse: [firstIndex:: firstIndex + 1].
            ^element
        )
        (* `benchmarks/are-we-fast-yet/DeltaBlueTon80.ns` uses a list as a
           stack. *)
        public removeLast = (
            | last element |
            0 = size_ ifTrue: [^kernelModule Error signal: 'removeLast: the List is empty'].
            last:: firstIndex + size_ - 1.
            element:: data at: last.
            data at: last put: nil.
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1].
            ^element
        )
        (* Removes the first element `=` to `element` and answers
           `element`, or answers what `onAbsent` answers.
           `benchmarks/are-we-fast-yet/DeltaBlue.ns` and `DeltaBlueTon80.ns`
           remove a constraint from a variable's list. *)
        public remove: element ifAbsent: onAbsent = (
            | position end |
            position:: firstIndex.
            end:: firstIndex + size_.
            [position < end] whileTrue: [
                (data at: position) = element ifTrue: [
                    removeAtDataIndex: position.
                    ^element].
                position:: position + 1].
            ^onAbsent value
        )
        (* Keeps the capacity, as the Are We Fast Yet `Vector>>removeAll`
           does, since a list emptied is usually filled again to about its
           old size: `benchmarks/are-we-fast-yet/Havlak.ns` empties and
           refills its predecessor lists for every loop search. *)
        public removeAll = (
            data:: Array new: data size.
            firstIndex:: 1.
            size_:: 0
        )
        (* Primordial Soup's meaning: `lessOrEqual` answers whether its
           first argument may precede its second, and the sort is a stable
           merge sort, `Array>>sortFrom:to:by:`'s, run over the list's range
           of `data` with a copy of that range as the other buffer.
           `benchmarks/are-we-fast-yet/DeltaBlue.ns` sorts its unsatisfied
           constraints by strength. *)
        public sort: lessOrEqual = (
            | last buffer index |
            size_ < 2 ifTrue: [^self].
            last:: firstIndex + size_ - 1.
            buffer:: Array new: last.
            index:: firstIndex.
            [index <= last] whileTrue: [
                buffer at: index put: (data at: index).
                index:: index + 1].
            sort: buffer into: data from: firstIndex to: last by: lessOrEqual
        )
        (* `source` and `destination` hold the same elements from `start`
           to `stop`; this leaves them sorted in `destination`, sorting each
           half into `source` first. *)
        private sort: source into: destination from: start to: stop by: lessOrEqual = (
            | middle |
            start >= stop ifTrue: [^self].
            middle:: (start + stop) // 2.
            sort: destination into: source from: start to: middle by: lessOrEqual.
            sort: destination into: source from: middle + 1 to: stop by: lessOrEqual.
            merge: source into: destination start: start middle: middle stop: stop by: lessOrEqual
        )
        (* Takes from the left run while its element may precede the right
           run's, which is what makes the sort stable. *)
        private merge: source into: destination start: start middle: middle stop: stop by: lessOrEqual = (
            | leftCursor rightCursor destinationCursor |
            leftCursor:: start.
            rightCursor:: middle + 1.
            destinationCursor:: start.
            [(leftCursor <= middle) and: [rightCursor <= stop]] whileTrue: [
                (lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor))
                    ifTrue: [
                        destination at: destinationCursor put: (source at: leftCursor).
                        leftCursor:: leftCursor + 1]
                    ifFalse: [
                        destination at: destinationCursor put: (source at: rightCursor).
                        rightCursor:: rightCursor + 1].
                destinationCursor:: destinationCursor + 1].
            [leftCursor <= middle] whileTrue: [
                destination at: destinationCursor put: (source at: leftCursor).
                leftCursor:: leftCursor + 1.
                destinationCursor:: destinationCursor + 1].
            [rightCursor <= stop] whileTrue: [
                destination at: destinationCursor put: (source at: rightCursor).
                rightCursor:: rightCursor + 1.
                destinationCursor:: destinationCursor + 1]
        )
        (* Closes the gap from the shorter side, so that removing the first
           or the last element moves nothing. *)
        private removeAtDataIndex: position = (
            | last index |
            last:: firstIndex + size_ - 1.
            position - firstIndex < (last - position)
                ifTrue: [
                    index:: position.
                    [index > firstIndex] whileTrue: [
                        data at: index put: (data at: index - 1).
                        index:: index - 1].
                    data at: firstIndex put: nil.
                    firstIndex:: firstIndex + 1]
                ifFalse: [
                    index:: position.
                    [index < last] whileTrue: [
                        data at: index put: (data at: index + 1).
                        index:: index + 1].
                    data at: last put: nil].
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1]
        )
        (* Moves the elements to the start of `data`, or of a larger Array,
           and answers the index after the last. Moving within `data` only
           when at least as many slots lie empty before the elements as
           they fill pays for the move with the `removeFirst`s that emptied
           those slots; growing by half pays for the copy with the `add:`s
           that fill the new room. Primordial Soup's growth, `capacity >> 1
           + capacity | 7`, also gives an Array of size 0 room. *)
        private makeRoomAtEnd = (
            | target index |
            ((firstIndex > 1) and: [firstIndex - 1 >= size_])
                ifTrue: [target:: data]
                ifFalse: [target:: Array new: (data size // 2 + data size bitOr: 7)].
            index:: 0.
            [index < size_] whileTrue: [
                target at: index + 1 put: (data at: firstIndex + index).
                index:: index + 1].
            target == data ifTrue: [
                index:: firstIndex.
                [index < (firstIndex + size_)] whileTrue: [
                    data at: index put: nil.
                    index:: index + 1]].
            data:: target.
            firstIndex:: 1.
            ^size_ + 1
        )
        (* `Array>>at:`'s text, so a program sees one error from either. *)
        private indexOutOfRange: index = (
            ^kernelModule Error signal: 'index ', index printString,
                ' out of range: valid indices are 1..', size_ printString,
                ' (size ', size_ printString, ')'
        )
    ) : (
        public new = ( ^self new: 3 )
    )
    (* The one public member: the facet `platform collections` answers. *)
    public newCollections = ( ^Collections new )
) : ()
```

- [ ] **Step 4: Adapt the goldens that relied on the old `List`.**

Their recorded output stays the same:
- `goldens/collections/a_user_subclass_of_list_inherits_its_protocol.ns`:
  - `class Stack = List new () (` becomes `class Stack = List new: 3 () (`;
  - the comment's last sentence gains: "Its superclass clause sends `new:`, List's primary factory, as Primordial Soup's is."
- `goldens/nesting/nested_override.ns`: its `Inner` slot started at `List`, which `class Nested = Inner () ()` instantiates through `new`. It now starts at the platform's `Stopwatch`, which is no value class either and whose primary factory is `new`. The whole file becomes:
  ```newspeak
Newspeak3
'Test'
(* A nested class's superclass clause (newspeak-spec.pdf 6.1) is evaluated
   once per enclosing instance, and `GrandChildOfOuter` overrides what its
   `Inner` clause reads with a slot rather than a nested-class declaration.
   `Nested`'s cached class is per receiver (6): once `g Nested` has resolved
   `Inner` for `g`, changing `g`'s `Inner` slot afterwards must not change
   the class `g Nested` already answers.

   The slot starts at `Stopwatch`, an implicit-receiver send the module's
   own slot answers -- the platform's `Stopwatch` -- not `Array`:
   `setInner:` supplies `Array` later, to show the cached class does not
   change, so the starting value has to differ from it. `Stopwatch` is no
   value class -- this golden instantiates `Nested` (`n1 new` below), and a
   value class whose enclosing object is mutable, as `g` is here, is
   refused where an instance is made (newspeak-spec.pdf 3.1.1), which would
   be `n1 new` itself rather than the line above it -- and its primary
   factory is `new`, which `Nested`'s superclass clause sends; `List`'s is
   `new:`. *)
class NestedOverride = Object new (| private Stopwatch ::= nil. |) (
    class Outer = Object new () (
        class Inner = Object new () (
            public printString = ( ^'Inner instance' )
        )
    )
    class ChildOfOuter = Outer () (
        public class Nested = Inner () ()
    )
    class GrandChildOfOuter = ChildOfOuter (| public Inner ::= Stopwatch. |) (
        public setInner: v = ( Inner:: v )
    )
    public main: platform args: args = (
        | t ::= platform transcript. g n1 n2 |
        Stopwatch:: platform time Stopwatch.
        g:: GrandChildOfOuter new.
        n1:: g Nested.
        t show: n1 new printString; newLine.
        g setInner: Array.
        n2:: g Nested.
        t show: (n1 == n2) printString; newLine.
    )
)
  ```
- `goldens/collections/a_list_grows_past_its_first_array.ns`, its comment's first sentence becomes: "A List starts over an Array of three and moves into one half as large again whenever it is full -- Primordial Soup's growth, to seven and then fifteen -- so ten elements take two moves; `add:` answers the element and `at:put:` the value (docs/design/core-library.md, "The collections module")."
- `goldens/collections/a_list_index_error_is_caught_as_an_error.ns`, its comment: "an empty List's array has four slots" becomes "has three slots".

- [ ] **Step 5: Run the per-commit gate.**

Expected: PASS, the new golden included, and no recorded output changed.

- [ ] **Step 6: Commit.**

```bash
git add crates/ns-runtime/core-library/CollectionsFactory.ns goldens/collections goldens/nesting/nested_override.ns
git commit -m "feat: List is Primordial Soup's, with a first index so removeFirst moves nothing"
```

---

### Task 5: `Map` and `Set`

Both are adapted from Primordial Soup's `Collections.ns`:
- an insertion-ordered data array (keys and values side by side for `Map`);
- an open-addressing table of indices into it, kept at most three quarters full;
- keys found by `hash` and compared with `=`;
- `at:` signals when the key is absent.

**Where they differ from Primordial Soup:**
- **No single removal.** They carry only what DeltaBlue and Havlak send, so there are no deleted marks.
- **A lookup answers the element's index.** So no operation reads the lazy table twice: every read of a lazy slot runs its getter.
- **`removeAll` keeps the capacity.** Havlak empties and refills its map on every loop search.
- **The integer selectors.** `>>` and `|` are written `//` and `bitOr:`.
- **The errors.** `NotFound` and `ArgumentError` are `kernelModule Error`s.

Primordial Soup's `transient hashtable_` is `lazy hashtable_` (§6.3.3, the user's decision). That ns-next's image carries its value is `docs/OPEN-WORK.md`'s "Whether an image carries a lazy slot's value".

**Files:**
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns`, `crates/ns-runtime/tests/platform.rs`
- Create: `goldens/collections/a_map_and_a_set_keep_insertion_order.ns`, `.toml`

**Interfaces:**
- Consumes: Task 4's `CollectionsFactory.ns`.
- Produces:
  - `platform collections Map`, with `new`, `new:`, `at:`, `at:put:`, `keys`, `removeAll`;
  - `platform collections Set`, with `new`, `new:`, `add:`, `do:`, `size`;
  - the error text `'key not found: ', key printString`.

- [ ] **Step 1: Write the golden.**

`goldens/collections/a_map_and_a_set_keep_insertion_order.ns`:
```newspeak
Newspeak3
'Collections'
(* Primordial Soup's Map and Set, as the Are We Fast Yet benchmarks of
   stage 3 send them (docs/design/core-library.md, "The collections
   module"): keys are found by `hash` and compared with `=`, so an equal
   String is the same key and two plain objects are two keys; `keys` and
   `do:` answer in insertion order, which overwriting a key does not
   change; growth past many rehashes and `removeAll` keep every key
   findable; `at:` signals when the key is absent. *)
class AMapAndASetKeepInsertionOrder = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          Error = platform kernel Error.
          Map = platform collections Map.
          Set = platform collections Set.
          map first second big set line |
        map:: Map new.
        map at: #strong put: 1; at: #weak put: 2.
        map at: #strong put: 10.
        transcript print: (map at: #strong); show: ' '; print: (map at: #weak); show: ' '; print: (map keys at: 1); show: ' '; print: (map keys at: 2); newLine.
        map at: 'ab' put: 3.
        transcript print: (map at: 'a', 'b'); newLine.
        map at: -5 put: 'minus'; at: 18446744073709551616 put: 'large'; at: nil put: 'nothing'.
        transcript show: (map at: -5); show: ' '; show: (map at: 18446744073709551616); show: ' '; show: (map at: nil); newLine.
        first:: Object new.
        second:: Object new.
        map at: first put: 'first'.
        transcript show: (map at: first); show: ' '; show: ([map at: second] on: Error do: [:error | 'absent']); newLine.
        transcript show: ([map at: #missing] on: Error do: [:error | error messageText]); newLine.
        transcript print: map keys size; newLine.

        big:: Map new.
        1 to: 1000 do: [:index | big at: index put: index * index].
        transcript print: (big at: 777); show: ' '; print: big keys size; show: ' '; print: (big keys at: 1); show: ' '; print: (big keys at: 1000); newLine.
        big removeAll.
        transcript print: big keys size; newLine.
        big at: 5 put: 25.
        transcript print: (big at: 5); show: ' '; print: big keys size; newLine.

        set:: Set new.
        set add: 3; add: 1; add: 3; add: 2.
        line:: ''.
        set do: [:each | line:: line, each printString, ' '].
        transcript show: line; print: set size; newLine.
        set add: 'x'; add: 'x', ''; add: first; add: second; add: first.
        transcript print: set size; newLine.
        1 to: 1000 do: [:index | set add: index].
        transcript print: set size; newLine
    )
) : ()
```
`goldens/collections/a_map_and_a_set_keep_insertion_order.toml`:
```toml
stdout = '''
10 2 #strong #weak
3
minus large nothing
first absent
key not found: #missing
7
603729 1000 1 1000
0
25 1
3 1 2 3
6
1003
'''
```

- [ ] **Step 2: Run it and see it fail.**

Expected: FAIL with `does not understand #Map`.

- [ ] **Step 3: Write `CollectionsFactory.ns`.** The whole file:
```newspeak
Newspeak3
'CollectionsFactory'
(* The Core Library's collections module (§8.3: the platform "provides
   accessors to various modules"; docs/design/core-library.md, "The
   collections module"), in the factory pattern `PlatformFactory.ns` is
   written in.

   This root holds what is privileged -- the kernel instance -- and nothing
   hands its instance to a program: `PlatformFactory`'s root builds it once,
   through the mirror's `moduleNamed:`, sends it `newCollections`, its one
   public member, and keeps it in a private lazy slot. What a program gets
   is the `Collections` facet `newCollections` answers, which hands out this
   module's classes by class accessors, each exactly `^outer
   CollectionsFactory <Name>` -- a lexical send, which reaches the root's
   private classes (§5.9), and which an image names by its path from the
   platform (`docs/design/image-format.md`, "The platform path section").
   Nothing a program can name answers this root's instance or its class, so
   no program can build one over something that is not the kernel. Keep it
   so: no code whose receiver is this root's instance may send it anything
   it does not understand, since a `MessageNotUnderstood` hands its
   receiver to whoever catches it.

   A value (§3.1.1), so that its facet is one and the platform, a value,
   may hold the facet in a slot.

   `List`, `Map` and `Set` are adapted from Primordial Soup's Collections.ns
   (https://github.com/rmacnak/primordialsoup, commit
   338f0bb342a618f5608ab72ab1b1fae9b226e472, newspeak/Collections.ns), and
   `List>>sort:`'s merge sort from `Array>>sortFrom:to:by:` in its
   newspeak/KernelForPrimordialSoup.ns. Changed for ns-next: only the
   selectors the corpus, the goldens and the Are We Fast Yet benchmarks
   send (the on-demand rule); `List` keeps the index of its first element,
   so that `removeFirst` runs in amortized constant time, as SOMns's
   `Vector` does, where Primordial Soup's shifts every element; a
   `removeAll` keeps the capacity, as the Are We Fast Yet `Vector` and
   `Dictionary` do; `Map` and `Set` have no removal of single elements and
   so no deleted marks; a hash table lookup answers the element's index in
   the data array, so that no operation reads the hash table twice; errors
   are `kernelModule Error`s with texts of their own, where Primordial Soup
   signals `NotFound` or `ArgumentError`, or sends the undefined
   `errorEmpty`; `>>` and `|` on integers are written with `//` and
   `bitOr:`; names are written out.

   Copyright 2008, the Newspeak project authors.

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root. *)
class CollectionsFactory usingKernel: theKernel = Object <: Value (
    | private kernelModule = theKernel. |
) (
    (* `platform collections`: the module's namespace. It names its own
       class as `Platform` does, so that the class has a path too. `List`
       is here because `Ducts.ns`, `NewspeakASTs.ns` and
       `V5BytecodeSimulation.ns` on `corpus/runnable.toml` bind `platform
       collections List` (docs/design/core-library.md, "The on-demand
       rule"). *)
    private class Collections = Value new () (
        public Collections = ( ^outer CollectionsFactory Collections )
        public List = ( ^outer CollectionsFactory List )
        (* `benchmarks/are-we-fast-yet/DeltaBlue.ns` and `Havlak.ns` need
           it, for SOMns's `Dictionary` and `IdentityDictionary`. *)
        public Map = ( ^outer CollectionsFactory Map )
        (* `benchmarks/are-we-fast-yet/Havlak.ns` needs it, for SOMns's
           `Set` and `IdentitySet`. *)
        public Set = ( ^outer CollectionsFactory Set )
    )
    (* An ordered, growable sequence: Primordial Soup's `List`, with the
       protocol the corpus, the goldens and the Are We Fast Yet benchmarks
       ask for (the on-demand rule, docs/design/core-library.md). The
       elements live in `data` from `firstIndex` on, `size_` of them, so
       that `removeFirst` moves nothing and a queue of `add:` and
       `removeFirst` runs in amortized constant time, as SOMns's `Vector`
       keeps its `firstIdx`. When `add:` finds the array full at its end, the
       elements move to its start if at least half of it lies empty before
       them, and otherwise into an Array half as large again; nothing grows
       in place. The primary factory takes the capacity, as Primordial
       Soup's does, so a program may write `List new: 15` as a superclass
       clause, as DeltaBlue's `Plan` does. The representation is private,
       where Primordial Soup's is protected, so that it can change -- to a
       ring buffer, say -- without a subclass noticing; `size_` alone stays
       protected, as a subclass's view of the size. *)
    private class List new: capacity = Object new (
        | protected size_ ::= 0.
          private firstIndex ::= 1.
          private data ::= Array new: capacity. |
    ) (
        public size = ( ^size_ )
        (* `benchmarks/are-we-fast-yet/DeltaBlue.ns`, `DeltaBlueTon80.ns`
           and `Havlak.ns` ask whether a work list is empty. *)
        public isEmpty = ( ^0 = size_ )
        (* Answers the element, as Smalltalk's and Newspeak's `add:` do. *)
        public add: element = (
            | end |
            end:: firstIndex + size_.
            end > data size ifTrue: [end:: makeRoomAtEnd].
            data at: end put: element.
            size_:: size_ + 1.
            ^element
        )
        (* The index check is written out here rather than sent, since
           every `at:` pays for it. An index that is no Integer is out of
           range too, rather than an error of the comparison. *)
        public at: index = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            ^data at: firstIndex + index - 1
        )
        (* Answers the value, as `add:` answers the element. *)
        public at: index put: value = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            data at: firstIndex + index - 1 put: value.
            ^value
        )
        (* Visits the elements the list holds when it starts: the size is
           read once, so an `add:` inside the block does not extend the
           walk, while `data` and `firstIndex` are read for every element,
           so an `at:put:` inside it is seen, as in Primordial Soup's `1 to:
           size_ do:`. *)
        public do: action = (
            | end index |
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                action value: (data at: firstIndex + index).
                index:: index + 1]
        )
        (* Primordial Soup's `Collection>>anySatisfy:`, a walk like `do:`'s
           written out, which needs no non-local return.
           `benchmarks/are-we-fast-yet/Havlak.ns` sends it. *)
        public anySatisfy: predicate = (
            | end index |
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                (predicate value: (data at: firstIndex + index)) ifTrue: [^true].
                index:: index + 1].
            ^false
        )
        (* Moves nothing: the vacated slot is cleared, so that the list
           keeps no element alive, and an emptied list starts its next
           element at the front again. `benchmarks/are-we-fast-yet/DeltaBlue.ns`
           and `Havlak.ns` use a list as a queue. *)
        public removeFirst = (
            | element |
            0 = size_ ifTrue: [^kernelModule Error signal: 'removeFirst: the List is empty'].
            element:: data at: firstIndex.
            data at: firstIndex put: nil.
            size_:: size_ - 1.
            0 = size_
                ifTrue: [firstIndex:: 1]
                ifFalse: [firstIndex:: firstIndex + 1].
            ^element
        )
        (* `benchmarks/are-we-fast-yet/DeltaBlueTon80.ns` uses a list as a
           stack. *)
        public removeLast = (
            | last element |
            0 = size_ ifTrue: [^kernelModule Error signal: 'removeLast: the List is empty'].
            last:: firstIndex + size_ - 1.
            element:: data at: last.
            data at: last put: nil.
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1].
            ^element
        )
        (* Removes the first element `=` to `element` and answers
           `element`, or answers what `onAbsent` answers.
           `benchmarks/are-we-fast-yet/DeltaBlue.ns` and `DeltaBlueTon80.ns`
           remove a constraint from a variable's list. *)
        public remove: element ifAbsent: onAbsent = (
            | position end |
            position:: firstIndex.
            end:: firstIndex + size_.
            [position < end] whileTrue: [
                (data at: position) = element ifTrue: [
                    removeAtDataIndex: position.
                    ^element].
                position:: position + 1].
            ^onAbsent value
        )
        (* Keeps the capacity, as the Are We Fast Yet `Vector>>removeAll`
           does, since a list emptied is usually filled again to about its
           old size: `benchmarks/are-we-fast-yet/Havlak.ns` empties and
           refills its predecessor lists for every loop search. *)
        public removeAll = (
            data:: Array new: data size.
            firstIndex:: 1.
            size_:: 0
        )
        (* Primordial Soup's meaning: `lessOrEqual` answers whether its
           first argument may precede its second, and the sort is a stable
           merge sort, `Array>>sortFrom:to:by:`'s, run over the list's range
           of `data` with a copy of that range as the other buffer.
           `benchmarks/are-we-fast-yet/DeltaBlue.ns` sorts its unsatisfied
           constraints by strength. *)
        public sort: lessOrEqual = (
            | last buffer index |
            size_ < 2 ifTrue: [^self].
            last:: firstIndex + size_ - 1.
            buffer:: Array new: last.
            index:: firstIndex.
            [index <= last] whileTrue: [
                buffer at: index put: (data at: index).
                index:: index + 1].
            sort: buffer into: data from: firstIndex to: last by: lessOrEqual
        )
        (* `source` and `destination` hold the same elements from `start`
           to `stop`; this leaves them sorted in `destination`, sorting each
           half into `source` first. *)
        private sort: source into: destination from: start to: stop by: lessOrEqual = (
            | middle |
            start >= stop ifTrue: [^self].
            middle:: (start + stop) // 2.
            sort: destination into: source from: start to: middle by: lessOrEqual.
            sort: destination into: source from: middle + 1 to: stop by: lessOrEqual.
            merge: source into: destination start: start middle: middle stop: stop by: lessOrEqual
        )
        (* Takes from the left run while its element may precede the right
           run's, which is what makes the sort stable. *)
        private merge: source into: destination start: start middle: middle stop: stop by: lessOrEqual = (
            | leftCursor rightCursor destinationCursor |
            leftCursor:: start.
            rightCursor:: middle + 1.
            destinationCursor:: start.
            [(leftCursor <= middle) and: [rightCursor <= stop]] whileTrue: [
                (lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor))
                    ifTrue: [
                        destination at: destinationCursor put: (source at: leftCursor).
                        leftCursor:: leftCursor + 1]
                    ifFalse: [
                        destination at: destinationCursor put: (source at: rightCursor).
                        rightCursor:: rightCursor + 1].
                destinationCursor:: destinationCursor + 1].
            [leftCursor <= middle] whileTrue: [
                destination at: destinationCursor put: (source at: leftCursor).
                leftCursor:: leftCursor + 1.
                destinationCursor:: destinationCursor + 1].
            [rightCursor <= stop] whileTrue: [
                destination at: destinationCursor put: (source at: rightCursor).
                rightCursor:: rightCursor + 1.
                destinationCursor:: destinationCursor + 1]
        )
        (* Closes the gap from the shorter side, so that removing the first
           or the last element moves nothing. *)
        private removeAtDataIndex: position = (
            | last index |
            last:: firstIndex + size_ - 1.
            position - firstIndex < (last - position)
                ifTrue: [
                    index:: position.
                    [index > firstIndex] whileTrue: [
                        data at: index put: (data at: index - 1).
                        index:: index - 1].
                    data at: firstIndex put: nil.
                    firstIndex:: firstIndex + 1]
                ifFalse: [
                    index:: position.
                    [index < last] whileTrue: [
                        data at: index put: (data at: index + 1).
                        index:: index + 1].
                    data at: last put: nil].
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1]
        )
        (* Moves the elements to the start of `data`, or of a larger Array,
           and answers the index after the last. Moving within `data` only
           when at least as many slots lie empty before the elements as
           they fill pays for the move with the `removeFirst`s that emptied
           those slots; growing by half pays for the copy with the `add:`s
           that fill the new room. Primordial Soup's growth, `capacity >> 1
           + capacity | 7`, also gives an Array of size 0 room. *)
        private makeRoomAtEnd = (
            | target index |
            ((firstIndex > 1) and: [firstIndex - 1 >= size_])
                ifTrue: [target:: data]
                ifFalse: [target:: Array new: (data size // 2 + data size bitOr: 7)].
            index:: 0.
            [index < size_] whileTrue: [
                target at: index + 1 put: (data at: firstIndex + index).
                index:: index + 1].
            target == data ifTrue: [
                index:: firstIndex.
                [index < (firstIndex + size_)] whileTrue: [
                    data at: index put: nil.
                    index:: index + 1]].
            data:: target.
            firstIndex:: 1.
            ^size_ + 1
        )
        (* `Array>>at:`'s text, so a program sees one error from either. *)
        private indexOutOfRange: index = (
            ^kernelModule Error signal: 'index ', index printString,
                ' out of range: valid indices are 1..', size_ printString,
                ' (size ', size_ printString, ')'
        )
    ) : (
        public new = ( ^self new: 3 )
    )
    (* Primordial Soup's `Map`: keys and values side by side in `data_`, in
       insertion order, and `hashtable_`, an open-addressing table of the
       keys' indices in `data_`, 0 marking an empty bucket. A key is found
       by its `hash` and compared with `=`; `at:` signals when the key is
       absent. The table is kept at most three quarters full, so a probe
       always meets an empty bucket. Only what
       `benchmarks/are-we-fast-yet/DeltaBlue.ns` and `Havlak.ns` send is
       here: `new`, `at:`, `at:put:`, `keys` and `removeAll`. *)
    private class Map new: capacity = Object new (
        | private size_ ::= 0.
          private data_ ::= nil. |
        createTable: capacity
    ) (
        (* Primordial Soup's transient slot, rebuilt from `data_` whenever
           it is nil. Every read runs its getter, so each operation reads
           it once and hands the table on. *)
        private lazy hashtable_ ::= rehash.
        public at: key = (
            | keyIndex |
            keyIndex:: scanFor: key in: hashtable_.
            keyIndex < 0 ifTrue: [^kernelModule Error signal: 'key not found: ', key printString].
            ^data_ at: keyIndex + 1
        )
        (* Answers the value. A key already present keeps its place in the
           order. *)
        public at: key put: value = (
            | table keyIndex |
            table:: hashtable_.
            keyIndex:: scanFor: key in: table.
            keyIndex < 0 ifTrue: [^insert: 0 - keyIndex in: table key: key value: value].
            data_ at: keyIndex + 1 put: value.
            ^value
        )
        (* An Array of the keys in insertion order. *)
        public keys = (
            | result data index |
            result:: Array new: size_.
            data:: data_.
            index:: 1.
            [index <= size_] whileTrue: [
                result at: index put: (data at: index * 2 - 1).
                index:: index + 1].
            ^result
        )
        (* Keeps the capacity, as the Are We Fast Yet `Dictionary>>removeAll`
           does: `benchmarks/are-we-fast-yet/Havlak.ns` empties its map of
           depth-first numbers for every loop search and fills it again with
           as many nodes. *)
        public removeAll = (
            createTable: data_ size // 2 - 1
        )
        (* Answers the new hash table. *)
        private createTable: count = (
            | table index |
            count < 0 ifTrue: [^kernelModule Error signal: 'new: requires a capacity of at least 0, got ', count printString].
            size_:: 0.
            data_:: Array new: count + 1 * 2.
            table:: Array new: count + 1 * 4 // 3.
            index:: table size.
            [index > 0] whileTrue: [
                table at: index put: 0.
                index:: index - 1].
            hashtable_:: table.
            ^table
        )
        private insert: bucketIndex in: table key: key value: value = (
            | keyIndex |
            keyIndex:: size_ * 2 + 1.
            size_:: size_ + 1.
            data_ at: keyIndex put: key.
            data_ at: keyIndex + 1 put: value.
            table at: bucketIndex put: keyIndex.
            keyIndex + 2 > data_ size ifTrue: [rehash].
            ^value
        )
        (* With no removal, every entry keeps its index in `data_`. *)
        private rehash = (
            | oldData end table newData keyIndex |
            oldData:: data_.
            end:: size_ * 2.
            table:: createTable: size_ * 2.
            newData:: data_.
            keyIndex:: 1.
            [keyIndex < end] whileTrue: [
                table at: (emptyBucketFor: (oldData at: keyIndex) in: table) put: keyIndex.
                newData at: keyIndex put: (oldData at: keyIndex).
                newData at: keyIndex + 1 put: (oldData at: keyIndex + 1).
                keyIndex:: keyIndex + 2].
            size_:: end // 2.
            ^table
        )
        (* Answers the key's index in `data_`, or the negated index of the
           empty bucket where it belongs. *)
        private scanFor: key in: table = (
            | data tableSize bucketIndex keyIndex |
            data:: data_.
            tableSize:: table size.
            bucketIndex:: key hash \\ tableSize + 1.
            [true] whileTrue: [
                keyIndex:: table at: bucketIndex.
                0 = keyIndex ifTrue: [^0 - bucketIndex].
                key = (data at: keyIndex) ifTrue: [^keyIndex].
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]]
        )
        private emptyBucketFor: key in: table = (
            | tableSize bucketIndex |
            tableSize:: table size.
            bucketIndex:: key hash \\ tableSize + 1.
            [0 = (table at: bucketIndex)] whileFalse: [
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]].
            ^bucketIndex
        )
    ) : (
        public new = ( ^self new: 3 )
    )
    (* Primordial Soup's `Set`: the elements in `data_`, in insertion
       order, and `hashtable_`, an open-addressing table of their indices,
       as `Map` keeps its keys; elements are compared with `=` and found by
       their `hash`. Only what `benchmarks/are-we-fast-yet/Havlak.ns` sends
       is here: `new`, `add:`, `do:` and `size`. *)
    private class Set new: capacity = Object new (
        | private size_ ::= 0.
          private data_ ::= nil. |
        createTable: capacity
    ) (
        (* As `Map`'s. *)
        private lazy hashtable_ ::= rehash.
        public size = ( ^size_ )
        (* Answers the element. An element already present keeps its place
           in the order and is replaced by the argument, as in Primordial
           Soup. *)
        public add: element = (
            | table keyIndex |
            table:: hashtable_.
            keyIndex:: scanFor: element in: table.
            keyIndex < 0 ifTrue: [^insert: 0 - keyIndex in: table element: element].
            data_ at: keyIndex put: element.
            ^element
        )
        (* Visits the elements in insertion order; the size and the data
           array are read once, as in Primordial Soup. *)
        public do: action = (
            | data end index |
            data:: data_.
            end:: size_.
            index:: 1.
            [index <= end] whileTrue: [
                action value: (data at: index).
                index:: index + 1]
        )
        private createTable: count = (
            | table index |
            count < 0 ifTrue: [^kernelModule Error signal: 'new: requires a capacity of at least 0, got ', count printString].
            size_:: 0.
            data_:: Array new: count + 1.
            table:: Array new: count + 1 * 4 // 3.
            index:: table size.
            [index > 0] whileTrue: [
                table at: index put: 0.
                index:: index - 1].
            hashtable_:: table.
            ^table
        )
        private insert: bucketIndex in: table element: element = (
            | keyIndex |
            keyIndex:: size_ + 1.
            size_:: keyIndex.
            data_ at: keyIndex put: element.
            table at: bucketIndex put: keyIndex.
            keyIndex + 1 > data_ size ifTrue: [rehash].
            ^element
        )
        private rehash = (
            | oldData end table newData keyIndex |
            oldData:: data_.
            end:: size_.
            table:: createTable: size_ * 2.
            newData:: data_.
            keyIndex:: 1.
            [keyIndex <= end] whileTrue: [
                table at: (emptyBucketFor: (oldData at: keyIndex) in: table) put: keyIndex.
                newData at: keyIndex put: (oldData at: keyIndex).
                keyIndex:: keyIndex + 1].
            size_:: end.
            ^table
        )
        private scanFor: element in: table = (
            | data tableSize bucketIndex keyIndex |
            data:: data_.
            tableSize:: table size.
            bucketIndex:: element hash \\ tableSize + 1.
            [true] whileTrue: [
                keyIndex:: table at: bucketIndex.
                0 = keyIndex ifTrue: [^0 - bucketIndex].
                element = (data at: keyIndex) ifTrue: [^keyIndex].
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]]
        )
        private emptyBucketFor: element in: table = (
            | tableSize bucketIndex |
            tableSize:: table size.
            bucketIndex:: element hash \\ tableSize + 1.
            [0 = (table at: bucketIndex)] whileFalse: [
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]].
            ^bucketIndex
        )
    ) : (
        public new = ( ^self new: 3 )
    )
    (* The one public member: the facet `platform collections` answers. *)
    public newCollections = ( ^Collections new )
) : ()
```

- [ ] **Step 4: Name another absent class in `platform.rs`.**

`platform_collections_does_not_invent_a_map` asks for `Map`, which now exists:
- rename it `platform_collections_does_not_invent_an_identity_map`;
- send `IdentityMap`, which the corpus asks for and the runtime does not have;
- rewrite its doc comment as: "The namespace answers exactly what a whitelisted module or a benchmark asks for, and nothing more: `IdentityMap` (and the rest of the corpus's collections protocol) names a class this runtime does not have, and must fail loudly rather than quietly alias onto some other collection."
- the panic text names `collections IdentityMap`.

- [ ] **Step 5: Run the per-commit gate.**

Expected: PASS. The proposed file was checked on 2026-10-01 against the whole workspace: 93 Newspeak checks of every method under both lowerings, and no recorded output changed beyond Task 4's source edits.

- [ ] **Step 6: Commit.**

```bash
git add crates/ns-runtime/core-library/CollectionsFactory.ns crates/ns-runtime/tests/platform.rs goldens/collections/a_map_and_a_set_keep_insertion_order.*
git commit -m "feat: Map and Set, adapted from Primordial Soup's Collections"
```

---

### Task 6: a NaN equals a NaN

`Float>>=` is IEEE's except that every NaN equals every other, and `Float>>hash` answers one hash for every NaN (decision 11). Because a value's `==` answers what `=` does, a NaN is then identical to itself, and a Map or a Set finds a NaN key again.
- The ordering (`< > <= >=`) and the arithmetic stay IEEE's.
- `0.0 = -0.0` stays true, and `-0.0` already hashes as `0.0`.
- The derived equality and hash of a value object with a Float slot (`builtins/value.rs`) call the same two functions, so they change with them.

No existing golden or test depends on a NaN's equality: the whole suite passed with this change on 2026-10-01.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`
- Create: `goldens/arithmetic/not_a_number_equals_itself.ns`, `.toml`

**Interfaces:**
- Consumes: `Map` and `Set` (Task 5), which the golden uses.
- Produces: `numbers::float_equals` and `numbers::float_hash` with the new meaning.

- [ ] **Step 1: Write the golden.**

`goldens/arithmetic/not_a_number_equals_itself.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* A Float is a value, so its `=` must be an equivalence and its `==`
   answers what `=` does (§3.1.1). IEEE 754's equality is not one: a NaN
   equals nothing, itself included. So `=` is IEEE's except that every NaN
   equals every other, and every NaN hashes alike -- JavaScript's
   SameValueZero, the equality its Map and Set use: `0.0 = -0.0` stays
   true. The ordering stays IEEE's, false whenever a NaN takes part, and
   so does arithmetic. A NaN is thus itself, and a key a Map finds again
   (docs/design/core-library.md, "A NaN equals a NaN"). *)
class NotANumberEqualsItself = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          Map = platform collections Map.
          Set = platform collections Set.
          notANumber other map set |
        notANumber:: 0.0 / 0.0.
        other:: (1.0 / 0.0) - (1.0 / 0.0).
        transcript print: notANumber = notANumber; show: ' '; print: notANumber == notANumber; show: ' '; print: notANumber = other; show: ' '; print: notANumber hash = other hash; newLine.
        transcript print: notANumber < notANumber; show: ' '; print: notANumber <= notANumber; show: ' '; print: notANumber > 1.0; show: ' '; print: notANumber = 1.0; show: ' '; print: 1.0 = notANumber; newLine.
        transcript print: 0.0 = -0.0; show: ' '; print: 0.0 hash = -0.0 hash; show: ' '; print: (notANumber + 1.0) = notANumber; newLine.
        map:: Map new.
        map at: notANumber put: 'found'.
        transcript show: (map at: other); newLine.
        set:: Set new.
        set add: notANumber; add: other; add: 1.0.
        transcript print: set size; newLine
    )
) : ()
```
`goldens/arithmetic/not_a_number_equals_itself.toml`:
```toml
stdout = '''
true true true true
false false false false false
true true true
found
2
'''
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens not_a_number_equals_itself`
Expected: FAIL. The first line starts `false false false`, and the map lookup signals `key not found: NaN`.

- [ ] **Step 3: Change the two functions.**

`crates/ns-runtime/src/builtins/numbers.rs`:
```rust
/// `Float>>=`: IEEE 754's equality, except that every NaN equals every
/// other. A Float is a value, so its `=` must be an equivalence and its
/// `==` answers the same (§3.1.1); IEEE's is not reflexive. This is
/// JavaScript's SameValueZero, the equality of its `Map` and `Set`: `0.0`
/// and `-0.0` stay equal. The ordering stays IEEE's
/// (`docs/design/core-library.md`, "A NaN equals a NaN").
pub(crate) fn float_equals<'gc>(receiver: f64, argument: Option<&Value<'gc>>) -> bool {
    matches!(argument, Some(Value::Float(other))
        if *other == receiver || (other.is_nan() && receiver.is_nan()))
}

/// A float's content hash: its bits, mixed and folded to 32 bits. `-0.0` is
/// normalized to `0.0` and every NaN to one NaN first, because `=` makes
/// each of them equal to the other, and a `hash` that told them apart would
/// break the one rule a `hash` has ("if `x = y` ... then `x hash = y
/// hash`").
///
/// Shared with `builtins::value`, whose derived hash must answer the same
/// number for a float slot as `Float>>hash` does for the float itself.
pub(crate) fn float_hash(value: f64) -> i64 {
    let normalized = if value == 0.0 {
        0.0
    } else if value.is_nan() {
        f64::NAN
    } else {
        value
    };
    crate::hash::content_hash(normalized.to_bits())
}
```
(This replaces both functions and their doc comments: the old hash comment's "A `NaN` needs no such care" is no longer true.)

- [ ] **Step 4: Run the golden and the per-commit gate.** Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/ns-runtime/src/builtins/numbers.rs goldens/arithmetic/not_a_number_equals_itself.*
git commit -m "feat: a NaN equals a NaN, so a Float's equality is an equivalence"
```

---

### Registering a benchmark (used by Tasks 7 to 10)

A benchmark joins the harness in these places, kept in alphabetical order. Each task prints them as they stand after it:
1. `benchmarks/are-we-fast-yet/AreWeFastYet.ns`: the `benchmarks` tuple in the module's slots;
2. `benchmarks/are-we-fast-yet/are-we-fast-yet.json`: `modules`;
3. the four `goldens/awfy/*.json`: `modules`, each path `../../benchmarks/are-we-fast-yet/<Name>.ns`;
4. `goldens/awfy/all_benchmarks_verify.toml`: one `<Name>: ok` line per benchmark;
5. `crates/ns-cli/tests/run.rs`: `a_timed_run_prints_a_score_for_every_benchmark`'s list of names.

The fourth golden file, `a_named_benchmark_runs_alone`, runs Sieve alone, and the other two run no benchmark. All four list every module, because the harness names every benchmark in its tuple.

---

### Task 7: DeltaBlue and DeltaBlueTon80

**The ports.**
- **DeltaBlue** is SOMns's `DeltaBlue.ns`. `bench` runs `Planner chainTest: 12000` and `Planner projectionTest: 12000`, Are We Fast Yet's size, and answers true, as SOMns's `innerBenchmarkLoop:` does. The checks are the benchmark's own, each an `Error signal:`.
- **DeltaBlueTon80** is Primordial Soup's `DeltaBlue.ns`, adapted as `RichardsTon80.ns` was. `bench` runs `chainTest: 100` and `projectionTest: 100`.
- **What each header records.** Every change of substance is in its header:
  - `remove:` becomes `remove:ifAbsent: []`, because `destroyConstraint:` removes a constraint twice;
  - the `sort:` block becomes a lessOrEqual block;
  - SOMns's truncating `/` is `//`; every division the workload makes is exact and non-negative.

**Files:**
- Create: `benchmarks/are-we-fast-yet/DeltaBlue.ns`, `DeltaBlueTon80.ns`
- Modify: the registration places above

**Interfaces:**
- Consumes:
  - `List new:`, `isEmpty`, `removeFirst`, `removeLast`, `remove:ifAbsent:`, `sort:` (Task 4);
  - `Map new`, `at:put:`, `at:`, `keys` (Task 5);
  - `timesRepeat:` (Task 3).
- Produces: the modules `DeltaBlue` and `DeltaBlueTon80`, each `usingPlatform:` with a public `bench`.

- [ ] **Step 1: Write `DeltaBlue.ns`.**

`benchmarks/are-we-fast-yet/DeltaBlue.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/DeltaBlue.ns), one of the Are We Fast Yet benchmarks.
   It is not Primordial Soup's DeltaBlue, which is DeltaBlueTon80.ns beside
   it. Changed for ns-next: a module in Primordial Soup's shape, whose bench
   runs once and answers true, as SOMns's innerBenchmarkLoop does, the
   checks being the benchmark's own; only Primordial Soup's protocol is
   sent: a Vector is a List and a Dictionary a Map, Array new:withAll:,
   which evaluates its block once per element, is a loop, and remove: is
   remove:ifAbsent: with an empty block, because destroyConstraint: removes
   a satisfied constraint from its variables twice and SOMns's Vector
   answers false for an absent element where Primordial Soup's List
   signals; the sort block answers whether the first strength is at least
   as strong as the second, since Primordial Soup's sort: takes a
   lessOrEqual block; / is //, the same here because every division the
   benchmark makes is exact; the tab escape in the cycle error's text,
   which ns-next's strings do not have, is a space; names are written
   out.

   Copyright (c) 2001-2015 see AUTHORS file

   Permission is hereby granted, free of charge, to any person obtaining a copy
   of this software and associated documentation files (the 'Software'), to deal
   in the Software without restriction, including without limitation the rights
   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
   copies of the Software, and to permit persons to whom the Software is
   furnished to do so, subject to the following conditions:

   The above copyright notice and this permission notice shall be included in
   all copies or substantial portions of the Software.

   THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
   THE SOFTWARE. *)
class DeltaBlue usingPlatform: platform = (
    | private Error = platform kernel Error.
      private List = platform collections List.
      private Map = platform collections Map.
      private strengthTable = Strength createStrengthTable.
      private strengthConstants = Strength createStrengthConstants.

      private AbsoluteStrongest = Strength of: #absoluteStrongest.
      private AbsoluteWeakest = Strength of: #absoluteWeakest.
      private Required = Strength of: #required. |
) (
    (* A Plan is an ordered list of constraints to be executed in sequence to
       resatisfy all currently satisfiable constraints in the face of one or
       more changing inputs. *)
    class Plan = List new: 15 () (
        (* Execute my constraints in order. *)
        public execute = (
            self do: [:constraint | constraint execute]
        )
    )
    (* This benchmark is an implementation of the DeltaBlue Constraint Solver
       described in `The DeltaBlue Algorithm: An Incremental Constraint
       Hierarchy Solver'', by Bjorn N. Freeman-Benson and John Maloney,
       Communications of the ACM, January 1990 (also as University of
       Washington TR 89-08-06). *)
    class Planner = (
        | currentMark ::= 1. |
    ) (
        (* Attempt to satisfy the given constraint and, if successful,
           incrementally update the dataflow graph.

           Details: If satifying the constraint is successful, it may override a
           weaker constraint on its output. The algorithm attempts to resatisfy
           that constraint using some other method. This process is repeated
           until either a) it reaches a variable that was not previously
           determined by any constraint or b) it reaches a constraint that
           is too weak to be satisfied using any of its methods. The variables
           of constraints that have been processed are marked with a unique mark
           value so that we know where we've been. This allows the algorithm to
           avoid getting into an infinite loop even if the constraint graph has
           an inadvertent cycle. *)
        public incrementalAdd: constraint = (
            | mark overridden |
            mark:: self newMark.
            overridden:: constraint satisfy: mark propagate: self.
            [overridden == nil] whileFalse:
                [overridden:: overridden satisfy: mark propagate: self]
        )
        (* Entry point for retracting a constraint. Remove the given constraint,
           which should be satisfied, and incrementally update the dataflow
           graph.

           Details: Retracting the given constraint may allow some currently
           unsatisfiable downstream constraint be satisfied. We thus collect a
           list of unsatisfied downstream constraints and attempt to satisfy
           each one in turn. This list is sorted by constraint strength,
           strongest first, as a heuristic for avoiding unnecessarily adding
           and then overriding weak constraints. *)
        public incrementalRemove: constraint = (
            | outputVariable unsatisfied |
            outputVariable:: constraint output.
            constraint markUnsatisfied.
            constraint removeFromGraph.
            unsatisfied:: self removePropagateFrom: outputVariable.
            unsatisfied do: [:each | self incrementalAdd: each]
        )
        (* Extract a plan for resatisfaction starting from the outputs of the
           given constraints, usually a set of input constraints. *)
        public extractPlanFromConstraints: constraints = (
            | sources |
            sources:: List new.
            constraints do: [:constraint |
                (constraint isInput and: [constraint isSatisfied]) ifTrue: [sources add: constraint]].
            ^self makePlan: sources
        )
        (* Extract a plan from the dataflow graph having the given variables. It
           is assumed that the given set of variables is complete, or at least
           that it contains all the input variables. *)
        public extractPlanFromVariables: variables = (
            | sources |
            sources:: List new.
            variables do: [:variable |
                variable constraints do: [:constraint |
                    (constraint isInput and: [constraint isSatisfied]) ifTrue: [sources add: constraint]]].
            ^self makePlan: sources
        )
        (* Extract a plan for resatisfaction starting from the given satisfied
           source constraints, usually a set of input constraints. This method
           assumes that stay optimization is desired; the plan will contain only
           constraints whose output variables are not stay. Constraints that do
           no computation, such as stay and edit constraints, are not included
           in the plan.

           Details: The outputs of a constraint are marked when it is added to
           the plan under construction. A constraint may be appended to the plan
           when all its input variables are known. A variable is known if either
           a) the variable is marked (indicating that has been computed by a
           constraint appearing earlier in the plan), b) the variable is 'stay'
           (i.e. it is a constant at plan execution time), or c) the variable
           is not determined by any constraint. The last provision is for past
           states of history variables, which are not stay but which are also
           not computed by any constraint. *)
        makePlan: sources = (
            | mark plan todo constraint |
            mark:: self newMark.
            plan:: Plan new.
            todo:: sources.
            [todo isEmpty] whileFalse: [
                constraint:: todo removeFirst.
                ((constraint output mark = mark) not and: [ (* not in plan already and... *)
                    constraint inputsKnown: mark]) ifTrue: [ (* eligible for inclusion *)
                        plan add: constraint.
                        constraint output mark: mark.
                        self addConstraintsConsuming: constraint output to: todo]].
            ^plan
        )
        (* The given variable has changed. Propagate new values downstream. *)
        propagateFrom: variable = (
            | todo constraint |
            todo:: List new.
            self addConstraintsConsuming: variable to: todo.
            [todo isEmpty] whileFalse: [
                constraint:: todo removeFirst.
                constraint execute.
                self addConstraintsConsuming: constraint output to: todo]
        )
        addConstraintsConsuming: variable to: collection = (
            | determining |
            determining:: variable determinedBy.
            variable constraints do: [:constraint |
                (constraint == determining or: [constraint isSatisfied not]) ifFalse:
                    [collection add: constraint]]
        )
        (* Recompute the walkabout strengths and stay flags of all variables
           downstream of the given constraint and recompute the actual values
           of all variables whose stay flag is true. If a cycle is detected,
           remove the given constraint and answer false. Otherwise, answer true.

           Details: Cycles are detected when a marked variable is encountered
           downstream of the given constraint. The sender is assumed to have
           marked the inputs of the given constraint with the given mark. Thus,
           encountering a marked node downstream of the output constraint means
           that there is a path from the constraint's output to one of its
           inputs. *)
        public addPropagate: constraint mark: mark = (
            | todo current |
            todo:: List new.
            todo add: constraint.
            [todo isEmpty] whileFalse: [
                current:: todo removeFirst.
                current output mark = mark ifTrue: [
                    self incrementalRemove: constraint.
                    ^false].
                current recalculate.
                self addConstraintsConsuming: current output to: todo].
            ^true
        )
        public changeVariable: variable newValue: newValue = (
            | editConstraint plan |
            editConstraint:: EditConstraint variable: variable strength: #preferred addTo: self.
            plan:: self extractPlanFromConstraints: {editConstraint}.
            10 timesRepeat: [
                variable value: newValue.
                plan execute].
            editConstraint destroyConstraint: self
        )
        constraintsConsuming: variable do: action = (
            | determining |
            determining:: variable determinedBy.
            variable constraints do: [:constraint |
                (constraint == determining or: [constraint isSatisfied not]) ifFalse:
                    [action value: constraint]]
        )
        (* Select a previously unused mark value.

           Details: We just keep incrementing. If necessary, the counter will
           turn into a LargePositiveInteger. In that case, it will be a bit
           slower to compute the next mark but the algorithms will all behave
           correctly. We reserve the value '0' to mean 'unmarked'. Thus, this
           generator starts at '1' and will never produce '0' as a mark value. *)
        newMark = (
            ^currentMark:: currentMark + 1
        )
        (* Update the walkabout strengths and stay flags of all variables
           downstream of the given constraint. Answer a collection of unsatisfied
           constraints sorted in order of decreasing strength. *)
        removePropagateFrom: outputVariable = (
            | unsatisfied todo variable |
            unsatisfied:: List new.

            outputVariable determinedBy: nil.
            outputVariable walkStrength: Strength absoluteWeakest.
            outputVariable stay: true.
            todo:: List new.
            todo add: outputVariable.
            [todo isEmpty] whileFalse: [
                variable:: todo removeFirst.
                variable constraints do: [:constraint |
                    constraint isSatisfied ifFalse: [unsatisfied add: constraint]].
                self constraintsConsuming: variable do: [:constraint |
                    constraint recalculate.
                    todo add: constraint output]].

            unsatisfied sort: [:first :second | (second strength stronger: first strength) not].
            ^unsatisfied
        )
    ) : (
        (* Do chain-of-equality-constraints performance tests. *)
        public chainTest: count = (
            | variables editConstraint plan planner |
            planner:: Planner new.
            variables:: Array new: count + 1.
            1 to: count + 1 do: [:index | variables at: index put: Variable new].

            (* thread a chain of equality constraints through the variables *)
            1 to: count do: [:index |
                | first second |
                first:: variables at: index.
                second:: variables at: index + 1.
                EqualityConstraint variable: first variable: second strength: #required addTo: planner].

            StayConstraint variable: (variables at: variables size) strength: #strongDefault addTo: planner.
            editConstraint:: EditConstraint variable: (variables at: 1) strength: #preferred addTo: planner.
            plan:: planner extractPlanFromConstraints: {editConstraint}.

            1 to: 100 do: [:value |
                (variables at: 1) value: value.
                plan execute.
                ((variables at: variables size) value = value) not ifTrue: [Error signal: 'Chain test failed!!']].

            editConstraint destroyConstraint: planner
        )
        (* This test constructs a two sets of variables related to each other by
           a simple linear transformation (scale and offset). *)
        public projectionTest: count = (
            | scale offset source destination planner destinations |
            planner:: Planner new.
            destinations:: List new.
            scale:: Variable value: 10.
            offset:: Variable value: 1000.

            1 to: count do: [:index |
                source:: Variable value: index.
                destination:: Variable value: index.
                destinations add: destination.
                StayConstraint variable: source strength: #default addTo: planner.
                ScaleConstraint source: source scale: scale offset: offset destination: destination strength: #required addTo: planner].

            planner changeVariable: source newValue: 17.
            (destination value = 1170) not ifTrue: [Error signal: 'Projection test 1 failed!!'].

            planner changeVariable: destination newValue: 1050.
            (source value = 5) not ifTrue: [Error signal: 'Projection test 2 failed!!'].

            planner changeVariable: scale newValue: 5.
            1 to: count - 1 do: [:index |
                ((destinations at: index) value = (index * 5 + 1000)) not
                    ifTrue: [Error signal: 'Projection test 3 failed!!']].

            planner changeVariable: offset newValue: 2000.
            1 to: count - 1 do: [:index |
                ((destinations at: index) value = (index * 5 + 2000)) not
                    ifTrue: [Error signal: 'Projection test 4 failed!!']]
        )
    )
    (* Strengths are used to measure the relative importance of constraints. The
       hierarchy of available strengths is determined by the class variable
       StrengthTable (see my class initialization method). Because Strengths are
       invariant, references to Strength instances are shared (i.e. all references
       to `Strength of: #required` point to a single, shared instance). New
       strengths may be inserted in the strength hierarchy without disrupting
       current constraints.

       Instance variables:
           symbolicValue      symbolic strength name (e.g. #required) <Symbol>
           arithmeticValue    index of the constraint in the hierarchy, used for comparisons <Number> *)
    class Strength new: symbol = (
        | private symbolicValue = symbol.
          public arithmeticValue = strengthTable at: symbol. |
    ) (
        (* Answer true if I am the same strength as the given Strength. *)
        public sameAs: strength = (
            ^arithmeticValue = strength arithmeticValue
        )
        (* Answer true if I am stronger than the given Strength. *)
        public stronger: strength = (
            ^arithmeticValue < strength arithmeticValue
        )
        (* Answer true if I am weaker than the given Strength. *)
        public weaker: strength = (
            ^arithmeticValue > strength arithmeticValue
        )
        (* Answer the stronger of myself and the given Strength. *)
        public strongest: strength = (
            (strength stronger: self)
                ifTrue: [^strength]
                ifFalse: [^self]
        )
        (* Answer the weaker of myself and the given Strength. *)
        public weakest: strength = (
            (strength weaker: self)
                ifTrue: [^strength]
                ifFalse: [^self]
        )
    ) : (
        (* Initialize the symbolic strength table. *)
        public createStrengthTable = (
            | table |
            table:: Map new.
            table at: #absoluteStrongest put: -10000.
            table at: #required put: -800.
            table at: #strongPreferred put: -600.
            table at: #preferred put: -400.
            table at: #strongDefault put: -200.
            table at: #default put: 0.
            table at: #weakDefault put: 500.
            table at: #absoluteWeakest put: 10000.
            ^table
        )
        public createStrengthConstants = (
            | constants |
            constants:: Map new.
            strengthTable keys do: [:strengthSymbol |
                constants
                    at: strengthSymbol
                    put: (self new: strengthSymbol)].
            ^constants
        )
        (* Answer an instance with the specified strength. *)
        public of: symbol = (
            ^strengthConstants at: symbol
        )
        public absoluteStrongest = (
            ^AbsoluteStrongest
        )
        public absoluteWeakest = (
            ^AbsoluteWeakest
        )
        public required = (
            ^Required
        )
    )
    (* I am an abstract class representing a system-maintainable relationship
       (or 'constraint') between a set of variables. I supply a strength instance
       variable; concrete subclasses provide a means of storing the constrained
       variables and other information required to represent a constraint.

       Instance variables:
           strength            the strength of this constraint <Strength> *)
    class AbstractConstraint new: strengthSymbol = (
        | public strength = Strength of: strengthSymbol. |
    ) (
        (* Normal constraints are not input constraints. An input constraint is
           one that depends on external state, such as the mouse, the keyboard,
           a clock, or some arbitrary piece of imperative code. *)
        public isInput = (
            ^false
        )
        (* Answer true if this constraint is satisfied in the current solution. *)
        public isSatisfied = (
            self subclassResponsibility
        )
        (* Activate this constraint and attempt to satisfy it. *)
        addConstraint: planner = (
            self addToGraph.
            planner incrementalAdd: self
        )
        (* Add myself to the constraint graph. *)
        addToGraph = (
            self subclassResponsibility
        )
        (* Deactivate this constraint, remove it from the constraint graph,
           possibly causing other constraints to be satisfied, and destroy it. *)
        public destroyConstraint: planner = (
            self isSatisfied ifTrue: [planner incrementalRemove: self].
            self removeFromGraph
        )
        (* Remove myself from the constraint graph. *)
        public removeFromGraph = (
            self subclassResponsibility
        )
        (* Decide if I can be satisfied and record that decision. The output of
           the choosen method must not have the given mark and must have a
           walkabout strength less than that of this constraint. *)
        public chooseMethod: mark = (
            self subclassResponsibility
        )
        (* Enforce this constraint. Assume that it is satisfied. *)
        public execute = (
            self subclassResponsibility
        )
        (* Assume that I am satisfied. Evaluate the given block on all my current
           input variables. *)
        public inputsDo: action = (
            self subclassResponsibility
        )
        (* Assume that I am satisfied. Answer true if all my current inputs are
           known. A variable is known if either a) it is 'stay' (i.e. it is a
           constant at plan execution time), b) it has the given mark (indicating
           that it has been computed by a constraint appearing earlier in the
           plan), or c) it is not determined by any constraint. *)
        public inputsKnown: mark = (
            self inputsDo: [:variable |
                (variable mark = mark or: [variable stay or: [variable determinedBy == nil]])
                    ifFalse: [^false]].
            ^true
        )
        (* Record the fact that I am unsatisfied. *)
        public markUnsatisfied = (
            self subclassResponsibility
        )
        (* Answer my current output variable. Raise an error if I am not
           currently satisfied. *)
        public output = (
            self subclassResponsibility
        )
        (* Calculate the walkabout strength, the stay flag, and, if it is 'stay',
           the value for the current output of this constraint. Assume this
           constraint is satisfied. *)
        public recalculate = (
            self subclassResponsibility
        )
        (* Attempt to find a way to enforce this (still unsatisfied) constraint.
           If successful, record the solution, perhaps modifying the current
           dataflow graph.  Answer the constraint that this constraint overrides,
           if there is one, or nil, if there isn't. *)
        public satisfy: mark propagate: planner = (
            | overridden |
            self chooseMethod: mark.
            self isSatisfied
                ifTrue: [
                    | outputVariable |
                    (* constraint can be satisfied
                       mark inputs to allow cycle detection in addPropagate *)
                    self inputsDo: [:input | input mark: mark].
                    outputVariable:: self output.
                    overridden:: outputVariable determinedBy.
                    overridden == nil ifFalse: [overridden markUnsatisfied].
                    outputVariable determinedBy: self.
                    (planner addPropagate: self mark: mark) ifFalse: [
                        Error signal: 'Cycle encountered adding: Constraint removed.'.
                        ^nil].
                    outputVariable mark: mark]
                ifFalse: [ (* constraint cannot be satisfied *)
                    overridden:: nil.
                    (strength sameAs: Strength required) ifTrue: [
                        Error signal: 'Failed to satisfy a required constraint']].
            ^overridden
        )
    )
    (* I am an abstract superclass for constraints having two possible output
       variables.

       Instance variables:
           first, second   possible output variables <Variable>
           direction       one of:
                           #forward (second is output)
                           #backward (first is output)
                           nil (not satisfied) *)
    class BinaryConstraint variable: firstVariable variable: secondVariable strength: strengthSymbol addTo: planner = AbstractConstraint new: strengthSymbol (
        | first = firstVariable.
          second = secondVariable.
          direction ::= nil. |
    ) (
        (* Answer true if this constraint is satisfied in the current solution. *)
        public isSatisfied = (
            ^direction notNil
        )
        (* Add myself to the constraint graph. *)
        addToGraph = (
            first addConstraint: self.
            second addConstraint: self.
            direction:: nil
        )
        (* Remove myself from the constraint graph. *)
        public removeFromGraph = (
            first == nil ifFalse: [first removeConstraint: self].
            second == nil ifFalse: [second removeConstraint: self].
            direction:: nil
        )
        (* Decide if I can be satisfied and which way I should flow based on
           the relative strength of the variables I relate, and record that
           decision. *)
        chooseMethod: mark = (
            first mark = mark ifTrue: [ (* forward or nothing *)
                (((second mark = mark) not) and: [strength stronger: second walkStrength])
                    ifTrue: [^direction:: #forward]
                    ifFalse: [^direction:: nil]].

            second mark = mark ifTrue: [ (* backward or nothing *)
                (((first mark = mark) not) and: [strength stronger: first walkStrength])
                    ifTrue: [^direction:: #backward]
                    ifFalse: [^direction:: nil]].

            (* if we get here, neither variable is marked, so we have choice *)
            (first walkStrength weaker: second walkStrength)
                ifTrue:
                    [(strength stronger: first walkStrength)
                        ifTrue: [^direction:: #backward]
                        ifFalse: [^direction:: nil]]
                ifFalse:
                    [(strength stronger: second walkStrength)
                        ifTrue: [^direction:: #forward]
                        ifFalse: [^direction:: nil]]
        )
        (* Enforce this constraint. Assume that it is satisfied. *)
        public execute = (
            self subclassResponsibility
        )
        (* Evaluate the given block on my current input variable. *)
        inputsDo: action = (
            direction = #forward
                ifTrue: [action value: first]
                ifFalse: [action value: second]
        )
        (* Record the fact that I am unsatisfied. *)
        public markUnsatisfied = (
            direction:: nil
        )
        (* Answer my current output variable. *)
        public output = (
            direction = #forward
                ifTrue: [^second]
                ifFalse: [^first]
        )
        (* Calculate the walkabout strength, the stay flag, and, if it is 'stay',
           the value for the current output of this constraint. Assume this
           constraint is satisfied. *)
        public recalculate = (
            | inputVariable outputVariable |
            direction = #forward
                ifTrue: [inputVariable:: first. outputVariable:: second]
                ifFalse: [inputVariable:: second. outputVariable:: first].
            outputVariable walkStrength: (strength weakest: inputVariable walkStrength).
            outputVariable stay: inputVariable stay.
            outputVariable stay ifTrue: [self execute] (* stay optimization *)
        )
    )
    (* I am a unary input constraint used to mark a variable that the client
       wishes to change. *)
    class EditConstraint variable: variable strength: strengthSymbol addTo: planner = UnaryConstraint variable: variable strength: strengthSymbol addTo: planner () (
        (* I indicate that a variable is to be changed by imperative code. *)
        public isInput = (
            ^true
        )
        (* Edit constraints do nothing. *)
        public execute = (
        )
    )
    (* I constrain two variables to have the same value: `first = second`. *)
    class EqualityConstraint variable: firstVariable variable: secondVariable strength: strengthSymbol addTo: planner = BinaryConstraint variable: firstVariable variable: secondVariable strength: strengthSymbol addTo: planner (
        self addConstraint: planner.
    ) (
        (* Enforce this constraint. Assume that it is satisfied. *)
        public execute = (
            direction = #forward
                ifTrue: [second value: first value]
                ifFalse: [first value: second value]
        )
    )
    (* I relate two variables by the linear scaling relationship:
       `second = (first * scale) + offset`. Either first or second may be
       changed to maintain this relationship but the scale factor and offset
       are considered read-only.

       Instance variables:
           scale       scale factor input variable <Variable>
           offset      offset input variable <Variable> *)
    class ScaleConstraint source: sourceVariable scale: scaleVariable offset: offsetVariable destination: destinationVariable strength: strengthSymbol addTo: planner = BinaryConstraint variable: sourceVariable variable: destinationVariable strength: strengthSymbol addTo: planner (
        | scale = scaleVariable.
          offset = offsetVariable. |
        self addConstraint: planner.
    ) (
        (* Add myself to the constraint graph. *)
        addToGraph = (
            first addConstraint: self.
            second addConstraint: self.
            scale addConstraint: self.
            offset addConstraint: self.
            direction:: nil
        )
        (* Remove myself from the constraint graph. *)
        public removeFromGraph = (
            first == nil ifFalse: [first removeConstraint: self].
            second == nil ifFalse: [second removeConstraint: self].
            scale == nil ifFalse: [scale removeConstraint: self].
            offset == nil ifFalse: [offset removeConstraint: self].
            direction:: nil
        )
        (* Enforce this constraint. Assume that it is satisfied. *)
        public execute = (
            direction = #forward
                ifTrue: [second value: (first value * scale value) + offset value]
                ifFalse: [first value: (second value - offset value) // scale value]
        )
        (* Evaluate the given block on my current input variable. *)
        inputsDo: action = (
            direction = #forward
                ifTrue: [action value: first.
                         action value: scale.
                         action value: offset]
                ifFalse: [action value: second.
                          action value: scale.
                          action value: offset]
        )
        (* Calculate the walkabout strength, the stay flag, and, if it is 'stay',
           the value for the current output of this constraint. Assume this
           constraint is satisfied. *)
        public recalculate = (
            | inputVariable outputVariable |
            direction = #forward
                ifTrue: [inputVariable:: first. outputVariable:: second]
                ifFalse: [outputVariable:: first. inputVariable:: second].
            outputVariable walkStrength: (strength weakest: inputVariable walkStrength).
            outputVariable stay: (inputVariable stay and: [scale stay and: [offset stay]]).
            outputVariable stay ifTrue: [self execute] (* stay optimization *)
        )
    )
    (* I am an abstract superclass for constraints having a single possible
       output variable.

       Instance variables:
           output      possible output variable <Variable>
           satisfied   true if I am currently satisfied <Boolean> *)
    class UnaryConstraint variable: variable strength: strengthSymbol addTo: planner = AbstractConstraint new: strengthSymbol (
        | public output = variable.
          satisfied ::= false. |
        self addConstraint: planner.
    ) (
        (* Answer true if this constraint is satisfied in the current solution. *)
        public isSatisfied = (
            ^satisfied
        )
        (* Add myself to the constraint graph. *)
        addToGraph = (
            output addConstraint: self.
            satisfied:: false
        )
        (* Remove myself from the constraint graph. *)
        public removeFromGraph = (
            output == nil ifFalse: [output removeConstraint: self].
            satisfied:: false
        )
        (* Decide if I can be satisfied and record that decision. *)
        chooseMethod: mark = (
            satisfied::
                (output mark = mark) not and: [
                    strength stronger: output walkStrength]
        )
        (* Enforce this constraint. Assume that it is satisfied. *)
        public execute = (
            self subclassResponsibility
        )
        (* I have no input variables. *)
        inputsDo: action = (
        )
        (* Record the fact that I am unsatisfied. *)
        public markUnsatisfied = (
            satisfied:: false
        )
        (* Calculate the walkabout strength, the stay flag, and, if it is 'stay',
           the value for the current output of this constraint. Assume this
           constraint is satisfied. *)
        public recalculate = (
            output walkStrength: strength.
            output stay: self isInput not.
            output stay ifTrue: [self execute] (* stay optimization *)
        )
    )
    (* I mark variables that should, with some level of preference, stay the
       same. I have one method with zero inputs and one output, which does
       nothing. Planners may exploit the fact that, if I am satisfied, my output
       will not change during plan execution.
       This is called 'stay optimization.' *)
    class StayConstraint variable: variable strength: strengthSymbol addTo: planner = UnaryConstraint variable: variable strength: strengthSymbol addTo: planner () (
        (* Stay constraints do nothing. *)
        public execute = (
        )
    )
    (* I represent a constrained variable. In addition to my value, I maintain the
       structure of the constraint graph, the current dataflow graph, and various
       parameters of interest to the DeltaBlue incremental constraint solver.

       Instance variables:
           value           my value; changed by constraints, read by client <Object>
           constraints     normal constraints that reference me <Array of Constraint>
           determinedBy    the constraint that currently determines
                           my value (or nil if there isn't one) <Constraint>
           walkStrength    my walkabout strength <Strength>
           stay            true if I am a planning-time constant <Boolean>
           mark            used by the planner to mark constraints <Number> *)
    class Variable = (
        | public value ::= 0.
          public constraints = List new: 2.
          public determinedBy ::= nil.
          public walkStrength ::= Strength absoluteWeakest.
          public stay ::= true.
          public mark ::= 0. |
    ) (
        (* Add the given constraint to the set of all constraints that refer to me. *)
        public addConstraint: constraint = (
            constraints add: constraint
        )
        (* Remove all traces of the given constraint from this variable. *)
        public removeConstraint: constraint = (
            constraints remove: constraint ifAbsent: [].
            determinedBy == constraint ifTrue: [determinedBy:: nil]
        )
    ) : (
        public value: initialValue = (
            | variable |
            variable:: self new.
            variable value: initialValue.
            ^variable
        )
    )
    public bench = (
        Planner chainTest: 12000.
        Planner projectionTest: 12000.
        ^true
    )
)
```

- [ ] **Step 2: Write `DeltaBlueTon80.ns`.**

`benchmarks/are-we-fast-yet/DeltaBlueTon80.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from Primordial Soup's DeltaBlue.ns
   (https://github.com/rmacnak/primordialsoup, commit
   338f0bb342a618f5608ab72ab1b1fae9b226e472, newspeak/DeltaBlue.ns), which
   says of itself: "One-way constraint solver, originally written in
   Smalltalk by John Maloney and Mario Wolczko." It is not the Are We Fast
   Yet DeltaBlue, which is DeltaBlue.ns beside it. Changed for ns-next: the
   module is named DeltaBlueTon80 so that both sit in one harness; bench
   signals an Error rather than an Exception and answers true once its
   own checks pass, where Primordial Soup's answers the module; names are
   written out.

   Copyright 2008, the Newspeak project authors.

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root. *)
class DeltaBlueTon80 usingPlatform: platform = (
    | private Error = platform kernel Error.
      private List = platform collections List.

      private required = Strength name: 'required' value: 0.
      private strongPreferred = Strength name: 'strongPreferred' value: 1.
      private preferred = Strength name: 'preferred' value: 2.
      private strongDefault = Strength name: 'strongDefault' value: 3.
      private normal = Strength name: 'normal' value: 4.
      private weakDefault = Strength name: 'weakDefault' value: 5.
      private weakest = Strength name: 'weakest' value: 6.

      private descendingStrengths = {required. strongPreferred. preferred. strongDefault. normal. weakDefault. weakest}.

      private forward = Direction named: 'forward'.
      private backward = Direction named: 'backward'.

      private planner = Planner new. |
) (
    class Direction named: directionName = (
        | name <String> = directionName. |
    ) (
    )
    (* A Plan is an ordered list of constraints to be executed in sequence to resatisfy all currently satisfiable constraints in the face of one or more changing inputs. *)
    class Plan = (
        | private constraints = List new. |
    ) (
        public addConstraint: constraint <Constraint> = (
            constraints add: constraint.
        )
        (* Execute my constraints in order. *)
        public execute = (
            constraints do: [:constraint | constraint execute].
        )
    )
    (* I embody the DeltaBlue algorithm described in: ''The DeltaBlue Algorithm: An Incremental Constraint Hierarchy Solver'' by Bjorn N. Freeman-Benson and John Maloney. See January 1990 Communications of the ACM or University of Washington TR 89-08-06 for further details. *)
    class Planner = (
        | currentMark <Integer> ::= 0. |
    ) (
        (* I am an abstract superclass for constraints having two possible output variables. *)
        class BinaryConstraint variable: firstVariable variable: secondVariable strength: initialStrength = Constraint strength: initialStrength (
            | first <Variable> = firstVariable.
              second <Variable> = secondVariable.
              direction <Direction> |
        ) (
            (* Add myself to the constraint graph. *)
            public addToGraph = (
                first addConstraint: self.
                second addConstraint: self.
                direction:: nil.
            )
            (* Decide if I can be satisfied and which way I should flow based on the relative strength of the variables I relate, and record that decision. *)
            protected chooseMethod: mark <Integer> = (
                first mark = mark ifTrue: [
                    ((second mark = mark) not and: [strength strongerThan: second walkStrength])
                        ifTrue: [direction:: forward]
                        ifFalse: [direction:: nil].
                    ^self].
                second mark = mark ifTrue: [
                    ((first mark = mark) not and: [strength strongerThan: first walkStrength])
                        ifTrue: [direction:: backward]
                        ifFalse: [direction:: nil].
                    ^self].

                (* If we get here, neither variable is marked, so we have a choice. *)
                (first walkStrength weakerThan: second walkStrength)
                    ifTrue: [
                        direction:: (strength strongerThan: first walkStrength)
                            ifTrue: [backward]
                            ifFalse: [nil]]
                    ifFalse: [
                        direction:: (strength strongerThan: second walkStrength)
                            ifTrue: [forward]
                            ifFalse: [(*nil*) backward]].
            )
            (* Answer my current input variable *)
            public input ^<Constraint> = (
                ^direction = forward ifTrue: [first] ifFalse: [second]
            )
            public inputsKnown: mark <Integer> ^<Boolean> = (
                | inputVariable <Variable> = input. |
                ^inputVariable mark = mark or: [inputVariable stay or: [inputVariable determinedBy isNil]]
            )
            (* Answer true if this constraint is satisfied in the current solution. *)
            public isSatisfied ^<Boolean> = (
                ^(nil = direction) not
            )
            (* Mark the input variable with the given mark. *)
            public markInputs: mark <Integer> = (
                input mark: mark
            )
            (* Record the fact that I am unsatisfied. *)
            public markUnsatisfied = (
                direction:: nil.
            )
            (* Answer my current output variable. *)
            public output ^<Constraint> = (
                ^direction = forward ifTrue: [second] ifFalse: [first]
            )
            (* Calculate the walkabout strength, the stay flag, and, if it is 'stay', the value for the current output of this constraint. Assume this constraint is satisfied. *)
            public recalculate = (
                | inputVariable <Variable> = input. outputVariable <Variable> = output. |
                outputVariable walkStrength: (strength weakest: inputVariable walkStrength).
                outputVariable stay: inputVariable stay.
                outputVariable stay ifTrue: [execute].
            )
            (* Remove myself from the constraint graph. *)
            public removeFromGraph = (
                first isNil ifFalse: [first removeConstraint: self].
                second isNil ifFalse: [second removeConstraint: self].
                direction:: nil.
            )
        )
        (* I am an abstract class representing a system-maintainable relationship (or "constraint") between a set of variables. I supply a strength instance variable; concrete subclasses provide a means of storing the constrained variables and other information required to represent a constraint. *)
        class Constraint strength: initialStrength = (
            | public strength <Strength> = initialStrength. |
        ) (
            (* Activate this constraint and attempt to satisfy it. *)
            public addConstraint = (
                addToGraph.
                incrementalAdd: self.
            )
            (* Add myself to the constraint graph. *)
            public addToGraph = (
                subclassResponsibility
            )
            (* Decide if I can be satisfied and record that decision. The output of the chosen method must not have the given mark and must have a walkabout strength less than that of this constraint. *)
            public chooseMethod: mark <Integer> = (
                subclassResponsibility
            )
            (* Deactivate this constraint, remove it from the constraint graph, possibly causing other constraints to be satisfied, and destroy it. *)
            public destroyConstraint = (
                isSatisfied ifTrue: [incrementalRemove: self].
                removeFromGraph.
            )
            (* Enforce this constraint. Assume that it is satisfied. *)
            public execute = (
                subclassResponsibility
            )
            (* Assume that I am satisfied. Answer true if all my current inputs are known. A variable is known if either a) it is 'stay' (i.e. it is a constant at plan execution time), b) it has the given mark (indicating that it has been computed by a constraint appearing earlier in the plan), or c) it is not determined by any constraint. *)
            public inputsKnown: mark <Integer> ^<Boolean> = (
                subclassResponsibility
            )
            (* Normal constraints are not input constraints. An input constraint is one that depends on external state, such as the mouse, the keyboard, a clock, or some arbitrary piece of imperative code. *)
            public isInput ^<Boolean> = (
                ^false
            )
            (* Answer true if this constraint is satisfied in the current solution. *)
            public isSatisfied ^<Boolean> = (
                subclassResponsibility
            )
            (* Set the mark of all input from the given mark. *)
            public markInputs: mark <Integer> = (
                subclassResponsibility
            )
            (* Record the fact that I am unsatisfied. *)
            public markUnsatisfied = (
                subclassResponsibility
            )
            (* Answer my current output variable. Raise an error if I am not currently satisfied. *)
            public output ^<Variable> = (
                subclassResponsibility
            )
            (* Calculate the walkabout strength, the stay flag, and, if it is 'stay', the value for the current output of this constraint. Assume this constraint is satisfied. *)
            public recalculate = (
                subclassResponsibility
            )
            (* Remove myself from the constraint graph. *)
            public removeFromGraph = (
                subclassResponsibility
            )
            (* Attempt to find a way to enforce this constraint. If successful, record the solution, perhaps modifying the current dataflow graph. Answer the constraint that this constraint overrides, if there is one, or nil, if there isn't. Assume: I am not already satisfied. *)
            public satisfy: mark <Integer> ^<Constraint> = (
                | outputVariable <Variable> overridden <Constraint> |
                chooseMethod: mark.
                isSatisfied ifFalse: [
                    strength = required ifTrue: [
                        Error signal: 'Could not satisfy a required constraint'].
                    ^nil].
                (* constraint can be satisfied *)
                (* mark inputs to allow cycle detection in addPropagate *)
                markInputs: mark.
                outputVariable:: output.
                overridden:: outputVariable determinedBy.
                overridden isNil ifFalse: [overridden markUnsatisfied].
                outputVariable determinedBy: self.
                (addPropagate: self mark: mark) ifFalse: [Error signal: 'Cycle encountered'].
                outputVariable mark: mark.
                ^overridden
            )
        )
        (* I am a unary input constraint used to mark a variable that the client wishes to change. *)
        class EditConstraint variable: variable strength: initialStrength = UnaryConstraint variable: variable strength: initialStrength (
            addConstraint.
        ) (
            (* Edit constraints do nothing. *)
            public execute = (
            )
            (* I indicate that a variable is to be changed by imperative code. *)
            public isInput ^<Boolean> = (
                ^true
            )
        )
        (* I constrain two variables to have the same value: "first = second". *)
        class EqualityConstraint variable: firstVariable variable: secondVariable strength: initialStrength = BinaryConstraint variable: firstVariable variable: secondVariable strength: initialStrength (
            addConstraint.
        ) (
            (* Enforce this constraint. Assume that it is satisfied. *)
            public execute = (
                output value: input value.
            )
        )
        (* I relate two variables by the linear scaling relationship: "second = (first * scale) + offset". Either first or second may be changed to maintain this relationship but the scale factor and offset are considered read-only. *)
        class ScaleConstraint source: source scale: scaleVariable offset: offsetVariable destination: destination strength: initialStrength = BinaryConstraint variable: source variable: destination strength: initialStrength (
            | scale <Variable> = scaleVariable. (* scale factor input variable *)
              offset <Variable> = offsetVariable. (* offset input variable *) |
            addConstraint
        ) (
            (* Add myself to the constraint graph. *)
            public addToGraph = (
                super addToGraph.
                scale addConstraint: self.
                offset addConstraint: self.
            )
            (* Enforce this constraint. Assume that it is satisfied. *)
            public execute = (
                direction = forward
                    ifTrue: [second value: first value * scale value + offset value]
                    ifFalse: [first value: (second value - offset value) // scale value].
            )
            (* Mark the inputs from the given mark. *)
            public markInputs: mark <Integer> = (
                super markInputs: mark.
                scale mark: mark.
                offset mark: mark.
            )
            (* Calculate the walkabout strength, the stay flag, and, if it is 'stay', the value for the current output of this constraint. Assume this constraint is satisfied. *)
            public recalculate = (
                | inputVariable <Variable> = input. outputVariable <Variable> = output. |
                outputVariable walkStrength: (strength weakest: inputVariable walkStrength).
                outputVariable stay: (inputVariable stay and: [scale stay and: [offset stay]]).
                outputVariable stay ifTrue: [execute]. (* stay optimization *)
            )
            (* Remove myself from the constraint graph. *)
            public removeFromGraph = (
                super removeFromGraph.
                scale isNil ifFalse: [scale removeConstraint: self].
                offset isNil ifFalse: [offset removeConstraint: self].
            )
        )
        (* I mark variables that should, with some level of preference, stay the same. I have one method with zero inputs and one output, which does nothing. Planners may exploit the fact that, if I am satisfied, my output will not change during plan execution. This is called "stay optimization". *)
        class StayConstraint variable: variable strength: initialStrength = UnaryConstraint variable: variable strength: initialStrength (
            addConstraint.
        ) (
            (* Stay constraints do nothing. *)
            public execute = (
            )
        )
        (* I am an abstract superclass for constraints having a single possible output variable. *)
        class UnaryConstraint variable: variable strength: initialStrength = Constraint strength: initialStrength (
            | public output <Variable> = variable.
              protected satisfied <Boolean> ::= false. |
        ) (
            (* Add myself to the constraint graph. *)
            public addToGraph = (
                output addConstraint: self.
                satisfied: false.
            )
            (* Decide if I can be satisfied and record that decision. *)
            public chooseMethod: mark <Integer> = (
                satisfied:: ((output mark = mark) not and: [strength strongerThan: output walkStrength])
            )
            public inputsKnown: mark <Integer> ^<Boolean> = (
                ^true
            )
            (* Answer true if this constraint is satisfied in the current solution. *)
            public isSatisfied ^<Boolean> = (
                ^satisfied
            )
            (* I have no inputs. *)
            public markInputs: mark <Integer> = (
            )
            (* Record the fact that I am unsatisfied. *)
            public markUnsatisfied = (
                satisfied: false
            )
            (* Calculate the walkabout strength, the stay flag, and, if it is 'stay', the value for the current output of this constraint. Assume this constraint is satisfied. *)
            public recalculate = (
                output walkStrength: strength.
                output stay: isInput not.
                output stay ifTrue: [execute]. (* Stay optimization *)
            )
            (* Remove myself from the constraint graph. *)
            public removeFromGraph = (
                output isNil ifFalse: [output removeConstraint: self].
                satisfied: false.
            )
        )
        (* I represent a constrained variable. In addition to my value, I maintain the structure of the constraint graph, the current dataflow graph, and various parameters of interest to the DeltaBlue incremental constraint solver. *)
        class Variable name: variableName initialValue: initialValue = (
            | public value <Integer> ::= initialValue.
              public constraints <List[Constraint]> = List new: 2.
              public determinedBy <Constraint>
              public mark <Integer> ::= 0.
              public walkStrength <Strength> ::= weakest.
              public stay <Boolean> ::= true.
              public name <String> = variableName. |
        ) (
            (* Add the given constraint to the set of all constraints that refer to me. *)
            public addConstraint: constraint <Constraint> = (
                constraints add: constraint.
            )
            (* Remove all traces of the given constraint from this variable. *)
            public removeConstraint: constraint <Constraint> = (
                constraints remove: constraint ifAbsent: [].
                determinedBy = constraint ifTrue: [determinedBy: nil].
            )
        )
        public addConstraintsConsuming: variable <Variable> to: list <List[Constraints]> = (
            | determining <Constraint> = variable determinedBy. |
            variable constraints do: [:constraint | ((constraint = determining) not and: [constraint isSatisfied]) ifTrue: [list add: constraint]].
        )
        (* Recompute the walkabout strengths and stay flags of all variables downstream of the given constraint and recompute the actual values of all variables whose stay flag is true. If a cycle is detected, remove the given constraint and answer false. Otherwise, answer true. *)
        public addPropagate: constraint <Constraint> mark: mark <Integer> ^<Boolean> = (
            (* Details: Cycles are detected when a marked variable is encountered downstream of the given constraint. The sender is assumed to have marked the inputs of the given constraint with the given mark. Thus, encountering a marked node downstream of the output constraint means that there is a path from the constraint's output to one of its inputs. *)
            | todo <List[Constraint]> = List new. |
            todo add: constraint.
            [todo isEmpty] whileFalse: [
                | current <Constraint> = todo removeLast. |
                current output mark = mark ifTrue: [incrementalRemove: constraint. ^false].
                current recalculate.
                addConstraintsConsuming: current output to: todo.
            ].
            ^true
        )
        (* This is the standard DeltaBlue benchmark. A long chain of equality constraints is constructed with a stay constraint on one end. An edit constraint is then added to the opposite end and the time is measured for adding and removing this constraint, and extracting and executing a constraint satisfaction plan. There are two cases. In case 1, the added constraint is stronger than the stay constraint and values must propagate down the entire length of the chain. In case 2, the added constraint is weaker than the stay constraint so it cannot be accommodated. The cost in this case is, of course, very low. Typical situations lie somewhere between these two extremes. *)
        public chainTest: count = (
            | previous <Variable> first <Variable> last <Variable>
              editConstraint <Constraint> editConstraints <List[Constraint]> plan <Plan> |
            1 to: count do: [:index |
                | name variable |
                name:: 'v', index printString.
                variable:: Variable name: name initialValue: 0.
                previous isNil ifFalse: [EqualityConstraint variable: previous variable: variable strength: required].
                index = 1 ifTrue: [first:: variable].
                index = count ifTrue: [last:: variable].
                previous:: variable.
            ].

            StayConstraint variable: last strength: strongDefault.
            editConstraint:: EditConstraint variable: first strength: preferred.
            editConstraints:: List new.
            editConstraints add: editConstraint.
            plan:: extractPlanFromConstraints: editConstraints.
            1 to: count do: [:index |
                first value: index.
                plan execute.
                last value = index ifFalse: [Error signal: 'Chain test failed!'].
            ].
            editConstraint destroyConstraint.
        )
        (* Extract a plan for resatisfaction starting from the outputs of the given constraints, usually a set of input constraints. *)
        public extractPlanFromConstraints: constraints <List[Constraint]> ^<Plan> = (
            | sources <List[Constraint]> = List new. |
            constraints do: [:constraint | (constraint isInput and: [constraint isSatisfied]) ifTrue: [sources add: constraint]].
            ^makePlan: sources
        )
        (* Attempt to satisfy the given constraint and, if successful, incrementally update the dataflow graph.  Details: If satisfying the constraint is successful, it may override a weaker constraint on its output. The algorithm attempts to resatisfy that constraint using some other method. This process is repeated until either a) it reaches a variable that was not previously determined by any constraint or b) it reaches a constraint that is too weak to be satisfied using any of its methods. The variables of constraints that have been processed are marked with a unique mark value so that we know where we've been. This allows the algorithm to avoid getting into an infinite loop even if the constraint graph has an inadvertent cycle. *)
        public incrementalAdd: constraint <Constraint> = (
            | mark <Integer> = newMark.
              overridden <Constraint> ::= constraint satisfy: mark. |
            [overridden isNil] whileFalse: [overridden:: overridden satisfy: mark].
        )
        (* Entry point for retracting a constraint. Remove the given constraint and incrementally update the dataflow graph. *)
        public incrementalRemove: constraint <Constraint> = (
            (* Details: Retracting the given constraint may allow some currently unsatisfiable downstream constraint to be satisfied. We therefore collect a list of unsatisfied downstream constraints and attempt to satisfy each one in turn. This list is traversed by constraint strength, strongest first, as a heuristic for avoiding unnecessarily adding and then overriding weak constraints. Assume: the given constraint is satisfied. *)
            | outputVariable <Variable> unsatisfied <List[Constraint]> |
            outputVariable:: constraint output.
            constraint markUnsatisfied.
            constraint removeFromGraph.
            unsatisfied:: removePropagateFrom: outputVariable.
            descendingStrengths do: [:strength |
                unsatisfied do: [:each |
                    each strength = strength
                        ifTrue: [incrementalAdd: each]]].
        )
        (* Extract a plan for resatisfaction starting from the given source constraints, usually a set of input constraints. This method assumes that stay optimization is desired; the plan will contain only constraints whose output variables are not stay. Constraints that do no computation, such as stay and edit constraints, are not included in the plan. *)
        public makePlan: sources <List[Constraint]> ^<Plan> = (
            (*Details: The outputs of a constraint are marked when it is added to the plan under construction. A constraint may be appended to the plan when all its input variables are known. A variable is known if either a) the variable is marked (indicating that has been computed by a constraint appearing earlier in the plan), b) the variable is 'stay' (i.e. it is a constant at plan execution time), or c) the variable is not determined by any constraint. The last provision is for past states of history variables, which are not stay but which are also not computed by any constraint. Assume: sources are all satisfied. *)
            | mark <Integer> = newMark.
              plan <Plan> = Plan new.
              todo <List[Constraint]> = sources. |
            [todo isEmpty] whileFalse: [
                | constraint <Constraint> = todo removeLast. |
                ((constraint output mark = mark) not and: [constraint inputsKnown: mark]) ifTrue: [
                    (* not in plan already and eligible for inclusion *)
                    plan addConstraint: constraint.
                    constraint output mark: mark.
                    addConstraintsConsuming: constraint output to: todo.
                ]
            ].
            ^plan
        )
        (* Select a previously unused mark value. *)
        public newMark ^<Integer> = (
            currentMark:: currentMark + 1.
            ^currentMark
        )
        (* This test constructs a two sets of variables related to each other by a simple linear transformation (scale and offset). The time is measured to change a variable on either side of the mapping and to change the scale and offset factors. *)
        public projectionTest: count <Integer> = (
            | scale <Variable> offset <Variable> source <Variable> destination <Variable>
              destinations <List[Variable]> |
            scale:: Variable name: 'scale' initialValue: 10.
            offset:: Variable name: 'offset' initialValue: 1000.
            destinations:: List new.
            0 to: count - 1 do: [:index |
                source:: Variable name: 'src', index printString initialValue: index.
                destination:: Variable name: 'dst', index printString initialValue: index.
                destinations add: destination.
                StayConstraint variable: source strength: normal.
                ScaleConstraint source: source scale: scale offset: offset destination: destination strength: required.
            ].

            setValueOf: source to: 17.
            destination value = 1170 ifFalse: [Error signal: 'Projection test 1 failed!'].

            setValueOf: destination to: 1050.
            source value = 5 ifFalse: [Error signal: 'Projection test 2 failed!'].

            setValueOf: scale to: 5.
            0 to: count - 2 do: [:index |
                (destinations at: index + 1) value = (index * 5 + 1000)
                    ifFalse: [Error signal: 'Projection test 3 failed!']].

            setValueOf: offset to: 2000.
            0 to: count - 2 do: [:index |
                (destinations at: index + 1) value = (index * 5 + 2000)
                    ifFalse: [Error signal: 'Projection test 4 failed!']].
        )
        (* The given variable has changed. Propagate new values downstream. *)
        public propagateFrom: variable <Variable> = (
            | todo <List[Constraint]> = List new. |
            addConstraintsConsuming: variable to: todo.
            [todo isEmpty] whileFalse: [
                | constraint <Constraint> = todo removeLast. |
                constraint execute.
                addConstraintsConsuming: constraint output to: todo.
            ].
        )
        (* Update the walkabout strengths and stay flags of all variables downstream of the given constraint. Answer a collection of unsatisfied constraints sorted in order of decreasing strength. *)
        public removePropagateFrom: outputVariable <Variable> ^<List[Constraint]> = (
            | unsatisfied <List[Constraint]> todo <List[Variable]> |
            outputVariable determinedBy: nil.
            outputVariable walkStrength: weakest.
            outputVariable stay: true.
            unsatisfied:: List new.
            todo:: List new.
            todo add: outputVariable.
            [todo isEmpty] whileFalse: [
                | variable <Variable> determining <Constraint> |
                variable:: todo removeLast.
                variable constraints do: [:constraint | constraint isSatisfied ifFalse: [unsatisfied add: constraint]].
                determining:: variable determinedBy.
                variable constraints do: [:next |
                    ((next = determining) not and: [next isSatisfied])
                        ifTrue: [next recalculate. todo add: next output]].
            ].
            ^unsatisfied
        )
        public setValueOf: variable <Variable> to: newValue = (
            | editConstraint <Constraint> editConstraints <List[Constraint]> plan <Plan> |
            editConstraint:: EditConstraint variable: variable strength: preferred.
            editConstraints:: List new.
            editConstraints add: editConstraint.
            plan:: extractPlanFromConstraints: editConstraints.
            10 timesRepeat: [variable value: newValue. plan execute].
            editConstraint destroyConstraint.
        )
    )
    (* Strengths are used to measure the relative importance of constraints. New strengths may be inserted in the strength hierarchy without disrupting current constraints. Strengths cannot be created outside this class, so pointer comparison can be used for value comparison. *)
    class Strength name: strengthName value: strengthValue = (
        | public name <String> = strengthName.
          public value <Integer> = strengthValue. |
    ) (
        public strongerThan: other <Strength> ^<Boolean> = (
            ^self value < other value
        )
        public strongest: other <Strength> ^<Strength> = (
            ^(self strongerThan: other) ifTrue: [self] ifFalse: [other]
        )
        public weakerThan: other <Strength> ^<Boolean> = (
            ^self value > other value
        )
        public weakest: other <Strength> ^<Strength> = (
            ^(self weakerThan: other) ifTrue: [self] ifFalse: [other]
        )
    )
    public bench = (
        planner chainTest: 100.
        planner projectionTest: 100.
        ^true
    )
)
```

- [ ] **Step 3: Register both.**

1. `AreWeFastYet.ns`, the `benchmarks` slot:
   ```newspeak
    | private benchmarks = {
        manifest Bounce. manifest DeltaBlue. manifest DeltaBlueTon80.
        manifest List. manifest Mandelbrot. manifest NBody.
        manifest Permute. manifest Queens. manifest Richards.
        manifest RichardsTon80. manifest Sieve. manifest Storage.
        manifest Towers }. |
   ```
2. `are-we-fast-yet.json`:
   ```json
{
  "root": "AreWeFastYet.ns",
  "modules": ["Bounce.ns", "DeltaBlue.ns", "DeltaBlueTon80.ns", "List.ns", "Mandelbrot.ns", "NBody.ns", "Permute.ns", "Queens.ns", "Richards.ns", "RichardsTon80.ns", "Sieve.ns", "Storage.ns", "Towers.ns"]
}
   ```
3. Each of `goldens/awfy/a_named_benchmark_runs_alone.json`, `all_benchmarks_verify.json`, `an_unknown_benchmark_is_refused.json` and `extra_arguments_are_a_usage_error.json`:
   ```json
{
  "root": "../../benchmarks/are-we-fast-yet/AreWeFastYet.ns",
  "modules": [
    "../../benchmarks/are-we-fast-yet/Bounce.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlue.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlueTon80.ns",
    "../../benchmarks/are-we-fast-yet/List.ns",
    "../../benchmarks/are-we-fast-yet/Mandelbrot.ns",
    "../../benchmarks/are-we-fast-yet/NBody.ns",
    "../../benchmarks/are-we-fast-yet/Permute.ns",
    "../../benchmarks/are-we-fast-yet/Queens.ns",
    "../../benchmarks/are-we-fast-yet/Richards.ns",
    "../../benchmarks/are-we-fast-yet/RichardsTon80.ns",
    "../../benchmarks/are-we-fast-yet/Sieve.ns",
    "../../benchmarks/are-we-fast-yet/Storage.ns",
    "../../benchmarks/are-we-fast-yet/Towers.ns"
  ]
}
   ```
4. `goldens/awfy/all_benchmarks_verify.toml`:
   ```toml
stdout = '''
Bounce: ok
DeltaBlue: ok
DeltaBlueTon80: ok
List: ok
Mandelbrot: ok
NBody: ok
Permute: ok
Queens: ok
Richards: ok
RichardsTon80: ok
Sieve: ok
Storage: ok
Towers: ok
'''
   ```
5. `crates/ns-cli/tests/run.rs`, `a_timed_run_prints_a_score_for_every_benchmark`'s expected names:
   ```rust
        [
            "Bounce",
            "DeltaBlue",
            "DeltaBlueTon80",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Richards",
            "RichardsTon80",
            "Sieve",
            "Storage",
            "Towers"
        ]
   ```

- [ ] **Step 4: Run both through the harness, in a release build.**

```bash
cargo build --release -p ns-cli
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- DeltaBlue
./target/release/ns-next run --lowering reference benchmarks/are-we-fast-yet/are-we-fast-yet.json -- DeltaBlue
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- DeltaBlueTon80
./target/release/ns-next run --lowering reference benchmarks/are-we-fast-yet/are-we-fast-yet.json -- DeltaBlueTon80
```
Expected: `DeltaBlue: ok` and `DeltaBlueTon80: ok`, exit status 0, each time. DeltaBlue takes about 0.9 s under the optimized lowering, about 4 s under the reference one.

- [ ] **Step 5: Run the per-commit gate.**

Expected: PASS. The `awfy` goldens in it load every module, so a port that does not compile fails here.

- [ ] **Step 6: Commit.**

```bash
git add benchmarks/are-we-fast-yet goldens/awfy crates/ns-cli/tests/run.rs
git commit -m "feat: DeltaBlue and Primordial Soup's DeltaBlue join the Are We Fast Yet harness"
```

---

### Task 8: Havlak

**The port** is SOMns's `Havlak.ns`. It runs Are We Fast Yet's workload at its size 1500:
- 1500 dummy loops, 50 loop-finding iterations, and 10, 10 and 5 nested loops;
- the result is 6102 loops in a graph of 5213 nodes;
- SOMns's own harness derives all five numbers from one count (50 n, 5 n, n, n, 5).

The workload was cross-checked against SOMns's own verified results:
- SOMns's parameters at n = 10 give 3102 and 5213, and at n = 1 give 168 and 56, both its `verifyResult:` values;
- the loop count is 1602 plus three per dummy loop.

**The collections.** `IdentityDictionary` is `Map`, and `IdentitySet` and `Set` are `Set`. Basic blocks and loops are keys and elements by identity because they define no `=`. The result does not depend on a Set's order; reversing `do:` was tried.

**Files:**
- Create: `benchmarks/are-we-fast-yet/Havlak.ns`
- Modify: the registration places

**Interfaces:**
- Consumes:
  - `List new:`, `isEmpty`, `removeFirst`, `removeAll`, `anySatisfy:` (Task 4);
  - `Map`, `Set` (Task 5);
  - `timesRepeat:`, `max:` (Task 3).
- Produces: the module `Havlak`.

- [ ] **Step 1: Write `Havlak.ns`.**

`benchmarks/are-we-fast-yet/Havlak.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/Havlak.ns), one of the Are We Fast Yet benchmarks.
   Changed for ns-next: a module in Primordial Soup's shape, whose bench
   runs once and checks its own result; it runs Are We Fast Yet's workload
   at its size 1500, 1500 dummy loops, 50 loop-finding iterations and 10,
   10 and 5 nested loops, where SOMns's harness derives all five from one
   iteration count (50 n, 5 n, n, n and 5); Vector is Primordial Soup's
   List, IdentityDictionary is Map and IdentitySet and Set are Set, the
   basic blocks and loops being keys and elements by identity because they
   define no =; BasicBlock's customHash, which only SOMns's collections
   ask for, is left out; only Primordial Soup's protocol is sent; names are
   written out, the basic block kinds included.

   Copyright 2011 Google Inc.

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root. *)
class Havlak usingPlatform: platform = (
    | private Error = platform kernel Error.
      private List = platform collections List.
      private Map = platform collections Map.
      private Set = platform collections Set. |
) (
    public bench = (
        | result |
        result:: LoopTesterApp new
            main: 1500
            iterations: 50
            parallelLoops: 10
            nestedLoops: 10
            baseLoops: 5.
        ((result at: 1) = 6102 and: [(result at: 2) = 5213]) ifFalse: [
            Error signal: 'Havlak: expected 6102 loops and 5213 nodes, got ',
                (result at: 1) printString, ' and ', (result at: 2) printString].
        ^result
    )
    class BasicBlock new: name = (
        | public inEdges = List new: 2.
          public outEdges = List new: 2.
          private name = name. |
    ) (
        public predecessorCount = ( ^inEdges size )
        public addOutEdge: to = ( outEdges add: to )
        public addInEdge: from = ( inEdges add: from )
    )
    class BasicBlockEdge for: controlFlowGraph from: fromName to: toName = (
        | private from = controlFlowGraph createNode: fromName.
          private to = controlFlowGraph createNode: toName. |
        from addOutEdge: to.
        to addInEdge: from.
        controlFlowGraph addEdge: self
    ) ()
    class ControlFlowGraph = (
        | private basicBlockMap = List new.
          private startNode ::= nil.
          private edgeList = List new. |
    ) (
        public createNode: name = (
            | node |
            basicBlockMap size >= name
                ifTrue: [node:: basicBlockMap at: name]
                ifFalse: [
                    node:: BasicBlock new: name.
                    basicBlockMap add: node].
            nodeCount = 1 ifTrue: [startNode:: node].
            ^node
        )
        public addEdge: edge = ( edgeList add: edge )
        public nodeCount = ( ^basicBlockMap size )
        public startBasicBlock = ( ^startNode )
        public basicBlocks = ( ^basicBlockMap )
    )
    class LoopStructureGraph = (
        | private root = SimpleLoop basicBlock: nil reducible: false.
          private loops = List new.
          private loopCounter ::= 0. |
        root nestingLevel: 0.
        root counter: loopCounter.
        loopCounter:: loopCounter + 1.
        loops add: root
    ) (
        public createNewLoop: basicBlock reducible: isReducible = (
            | loop |
            loop:: SimpleLoop basicBlock: basicBlock reducible: isReducible.
            loop counter: loopCounter.
            loopCounter:: loopCounter + 1.
            loops add: loop.
            ^loop
        )
        public calculateNestingLevel = (
            loops do: [:loop |
                loop isRoot ifFalse: [
                    loop parent isNil ifTrue: [
                        loop parent: root]]].
            calculateNestingLevelOf: root depth: 0
        )
        private calculateNestingLevelOf: loop depth: depth = (
            loop depthLevel: depth.
            loop children do: [:child |
                calculateNestingLevelOf: child depth: depth + 1.
                loop nestingLevel: (loop nestingLevel max: 1 + child nestingLevel)]
        )
        public loopCount = ( ^loops size )
    )
    class SimpleLoop basicBlock: basicBlock reducible: isReducible = (
        | public counter ::= 0.
          public depthLevel ::= 0.
          private parent_ ::= nil.
          private isRoot_ ::= false.
          private nestingLevel_ ::= 0.
          private header = basicBlock.
          private isReducible = isReducible.
          private basicBlocks = Set new.
          public children = Set new. |
        basicBlock notNil ifTrue: [basicBlocks add: basicBlock]
    ) (
        public addNode: basicBlock = ( basicBlocks add: basicBlock )
        public addChildLoop: loop = ( children add: loop )
        public parent = ( ^parent_ )
        public parent: loop = (
            parent_:: loop.
            parent_ addChildLoop: self
        )
        public isRoot = ( ^isRoot_ )
        public setIsRoot = ( isRoot_:: true )
        public nestingLevel = ( ^nestingLevel_ )
        public nestingLevel: level = (
            nestingLevel_:: level.
            level = 0 ifTrue: [setIsRoot]
        )
    )
    class UnionFindNode = (
        | private parent_ ::= nil.
          private basicBlock_ ::= nil.
          private depthFirstNumber_ ::= 0.
          public loop ::= nil. |
    ) (
        public initializeNode: basicBlock depthFirstNumber: depthFirstNumber = (
            parent_:: self.
            basicBlock_:: basicBlock.
            depthFirstNumber_:: depthFirstNumber.
            loop:: nil
        )
        public findSet = (
            | nodeList node |
            nodeList:: List new.
            node:: self.
            [(node == node parent) not] whileTrue: [
                (node parent == node parent parent) not ifTrue: [
                    nodeList add: node].
                node:: node parent].
            nodeList do: [:each | each union: parent_].
            ^node
        )
        public union: node = ( parent_:: node )
        public parent = ( ^parent_ )
        public basicBlock = ( ^basicBlock_ )
        public depthFirstNumber = ( ^depthFirstNumber_ )
    )
    class LoopTesterApp = (
        | private controlFlowGraph = ControlFlowGraph new.
          private loopStructureGraph = LoopStructureGraph new. |
        controlFlowGraph createNode: 1
    ) (
        private buildDiamond: start = (
            BasicBlockEdge for: controlFlowGraph from: start to: start + 1.
            BasicBlockEdge for: controlFlowGraph from: start to: start + 2.
            BasicBlockEdge for: controlFlowGraph from: start + 1 to: start + 3.
            BasicBlockEdge for: controlFlowGraph from: start + 2 to: start + 3.
            ^start + 3
        )
        private buildConnect: start end: end = (
            BasicBlockEdge for: controlFlowGraph from: start to: end
        )
        private buildStraight: start length: length = (
            0 to: length - 1 do: [:index |
                buildConnect: start + index end: start + index + 1].
            ^start + length
        )
        private buildBaseLoop: from = (
            | header firstDiamond between secondDiamond footer |
            header:: buildStraight: from length: 1.
            firstDiamond:: buildDiamond: header.
            between:: buildStraight: firstDiamond length: 1.
            secondDiamond:: buildDiamond: between.
            footer:: buildStraight: secondDiamond length: 1.
            buildConnect: secondDiamond end: between.
            buildConnect: firstDiamond end: header.
            buildConnect: footer end: from.
            footer:: buildStraight: footer length: 1.
            ^footer
        )
        public main: dummyLoopCount iterations: findLoopIterations parallelLoops: parallelLoops nestedLoops: nestedLoops baseLoops: baseLoops = (
            constructSimpleControlFlowGraph.
            addDummyLoops: dummyLoopCount.
            constructControlFlowGraph: parallelLoops nestedLoops: nestedLoops baseLoops: baseLoops.
            findLoops: loopStructureGraph.
            findLoopIterations timesRepeat: [
                findLoops: LoopStructureGraph new].
            loopStructureGraph calculateNestingLevel.
            ^{loopStructureGraph loopCount. controlFlowGraph nodeCount}
        )
        private constructControlFlowGraph: parallelLoops nestedLoops: nestedLoops baseLoops: baseLoops = (
            | current |
            current:: 3.
            parallelLoops timesRepeat: [
                controlFlowGraph createNode: current + 1.
                buildConnect: 2 end: current + 1.
                current:: current + 1.
                nestedLoops timesRepeat: [
                    | top bottom |
                    top:: current.
                    current:: buildStraight: current length: 1.
                    baseLoops timesRepeat: [current:: buildBaseLoop: current].
                    bottom:: buildStraight: current length: 1.
                    buildConnect: current end: top.
                    current:: bottom].
                buildConnect: current end: 1]
        )
        private addDummyLoops: dummyLoopCount = (
            dummyLoopCount timesRepeat: [
                findLoops: loopStructureGraph]
        )
        private findLoops: loopStructure = (
            | finder |
            finder:: HavlakLoopFinder new: controlFlowGraph loopStructureGraph: loopStructure.
            finder findLoops
        )
        private constructSimpleControlFlowGraph = (
            controlFlowGraph createNode: 1.
            buildBaseLoop: 1.
            controlFlowGraph createNode: 2.
            BasicBlockEdge for: controlFlowGraph from: 1 to: 3
        )
    )
    class HavlakLoopFinder new: controlFlowGraph loopStructureGraph: loopStructureGraph = (
        | private controlFlowGraph = controlFlowGraph.
          private loopStructureGraph = loopStructureGraph.
          private Unvisited = 2147483647.
          private MaxNonBackPredecessors = 32 * 1024.
          private nonBackPredecessors = List new.
          private backPredecessors = List new.
          private number = Map new.
          private maxSize ::= 0.
          private header ::= nil.
          private type ::= nil.
          private last ::= nil.
          private nodes ::= nil. |
    ) (
        (* The kinds of basic block: #nonHeader #reducible #selfLoop
           #irreducible #dead. *)
        private isAncestor: ancestor of: descendant = (
            ^ancestor <= descendant and: [descendant <= (last at: ancestor)]
        )
        private depthFirstSearch: currentNode current: current = (
            | lastId outerBlocks |
            (nodes at: current) initializeNode: currentNode depthFirstNumber: current.
            number at: currentNode put: current.
            lastId:: current.
            outerBlocks:: currentNode outEdges.
            1 to: outerBlocks size do: [:index |
                | target |
                target:: outerBlocks at: index.
                (number at: target) = Unvisited ifTrue: [
                    lastId:: depthFirstSearch: target current: lastId + 1]].
            last at: current put: lastId.
            ^lastId
        )
        private initializeAllNodes = (
            controlFlowGraph basicBlocks do: [:basicBlock |
                number at: basicBlock put: Unvisited].
            depthFirstSearch: controlFlowGraph startBasicBlock current: 1
        )
        private identifyEdges: size = (
            1 to: size do: [:candidate |
                | candidateBlock |
                header at: candidate put: 1.
                type at: candidate put: #nonHeader.
                candidateBlock:: (nodes at: candidate) basicBlock.
                candidateBlock isNil
                    ifTrue: [type at: candidate put: #dead]
                    ifFalse: [processEdges: candidateBlock candidate: candidate]]
        )
        private processEdges: candidateBlock candidate: candidate = (
            candidateBlock predecessorCount > 0 ifTrue: [
                candidateBlock inEdges do: [:predecessorBlock |
                    | predecessor |
                    predecessor:: number at: predecessorBlock.
                    (predecessor = Unvisited) not ifTrue: [
                        (isAncestor: candidate of: predecessor)
                            ifTrue: [(backPredecessors at: candidate) add: predecessor]
                            ifFalse: [(nonBackPredecessors at: candidate) add: predecessor]]]]
        )
        public findLoops = (
            | size |
            controlFlowGraph startBasicBlock isNil ifTrue: [^self].
            size:: controlFlowGraph nodeCount.
            nonBackPredecessors removeAll.
            backPredecessors removeAll.
            number removeAll.
            size > maxSize ifTrue: [
                header:: Array new: size.
                type:: Array new: size.
                last:: Array new: size.
                nodes:: Array new: size.
                maxSize:: size].
            1 to: size do: [:index |
                nonBackPredecessors add: Set new.
                backPredecessors add: List new.
                nodes at: index put: UnionFindNode new].
            initializeAllNodes.
            identifyEdges: size.
            header at: 1 put: 1.
            size to: 1 by: -1 do: [:candidate |
                | nodePool candidateBlock |
                nodePool:: List new.
                candidateBlock:: (nodes at: candidate) basicBlock.
                candidateBlock notNil ifTrue: [
                    | workList |
                    collectBackPredecessors: candidate nodePool: nodePool.
                    workList:: List new.
                    nodePool do: [:each | workList add: each].
                    (nodePool size = 0) not ifTrue: [
                        type at: candidate put: #reducible].
                    [workList isEmpty] whileFalse: [
                        | workNode nonBackSize |
                        workNode:: workList removeFirst.
                        nonBackSize:: (nonBackPredecessors at: workNode depthFirstNumber) size.
                        nonBackSize > MaxNonBackPredecessors ifTrue: [^self].
                        processNonBackPredecessors: candidate nodePool: nodePool workList: workList node: workNode].
                    (nodePool size > 0 or: [(type at: candidate) = #selfLoop]) ifTrue: [
                        | loop |
                        loop:: loopStructureGraph
                            createNewLoop: candidateBlock
                            reducible: ((type at: candidate) = #irreducible) not.
                        setLoopAttribute: candidate nodePool: nodePool loop: loop]]]
        )
        (* Step e of Havlak's algorithm. *)
        private processNonBackPredecessors: candidate nodePool: nodePool workList: workList node: workNode = (
            (nonBackPredecessors at: workNode depthFirstNumber) do: [:predecessor |
                | representative |
                representative:: (nodes at: predecessor) findSet.
                (isAncestor: candidate of: representative depthFirstNumber) not
                    ifTrue: [
                        type at: candidate put: #irreducible.
                        (nonBackPredecessors at: candidate) add: representative depthFirstNumber]
                    ifFalse: [
                        (representative depthFirstNumber = candidate) not ifTrue: [
                            (nodePool anySatisfy: [:each | each == representative]) ifFalse: [
                                workList add: representative.
                                nodePool add: representative]]]]
        )
        private setLoopAttribute: candidate nodePool: nodePool loop: loop = (
            (nodes at: candidate) loop: loop.
            nodePool do: [:node |
                header at: node depthFirstNumber put: candidate.
                node union: (nodes at: candidate).
                node loop notNil
                    ifTrue: [node loop parent: loop]
                    ifFalse: [loop addNode: node basicBlock]]
        )
        (* Step d of Havlak's algorithm. *)
        private collectBackPredecessors: candidate nodePool: nodePool = (
            (backPredecessors at: candidate) do: [:predecessor |
                (predecessor = candidate) not
                    ifTrue: [nodePool add: (nodes at: predecessor) findSet]
                    ifFalse: [type at: candidate put: #selfLoop]]
        )
    )
)
```

- [ ] **Step 2: Register it.**

1. `AreWeFastYet.ns`, the `benchmarks` slot:
   ```newspeak
    | private benchmarks = {
        manifest Bounce. manifest DeltaBlue. manifest DeltaBlueTon80.
        manifest Havlak. manifest List. manifest Mandelbrot.
        manifest NBody. manifest Permute. manifest Queens.
        manifest Richards. manifest RichardsTon80. manifest Sieve.
        manifest Storage. manifest Towers }. |
   ```
2. `are-we-fast-yet.json`:
   ```json
{
  "root": "AreWeFastYet.ns",
  "modules": ["Bounce.ns", "DeltaBlue.ns", "DeltaBlueTon80.ns", "Havlak.ns", "List.ns", "Mandelbrot.ns", "NBody.ns", "Permute.ns", "Queens.ns", "Richards.ns", "RichardsTon80.ns", "Sieve.ns", "Storage.ns", "Towers.ns"]
}
   ```
3. Each of `goldens/awfy/a_named_benchmark_runs_alone.json`, `all_benchmarks_verify.json`, `an_unknown_benchmark_is_refused.json` and `extra_arguments_are_a_usage_error.json`:
   ```json
{
  "root": "../../benchmarks/are-we-fast-yet/AreWeFastYet.ns",
  "modules": [
    "../../benchmarks/are-we-fast-yet/Bounce.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlue.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlueTon80.ns",
    "../../benchmarks/are-we-fast-yet/Havlak.ns",
    "../../benchmarks/are-we-fast-yet/List.ns",
    "../../benchmarks/are-we-fast-yet/Mandelbrot.ns",
    "../../benchmarks/are-we-fast-yet/NBody.ns",
    "../../benchmarks/are-we-fast-yet/Permute.ns",
    "../../benchmarks/are-we-fast-yet/Queens.ns",
    "../../benchmarks/are-we-fast-yet/Richards.ns",
    "../../benchmarks/are-we-fast-yet/RichardsTon80.ns",
    "../../benchmarks/are-we-fast-yet/Sieve.ns",
    "../../benchmarks/are-we-fast-yet/Storage.ns",
    "../../benchmarks/are-we-fast-yet/Towers.ns"
  ]
}
   ```
4. `goldens/awfy/all_benchmarks_verify.toml`:
   ```toml
stdout = '''
Bounce: ok
DeltaBlue: ok
DeltaBlueTon80: ok
Havlak: ok
List: ok
Mandelbrot: ok
NBody: ok
Permute: ok
Queens: ok
Richards: ok
RichardsTon80: ok
Sieve: ok
Storage: ok
Towers: ok
'''
   ```
5. `crates/ns-cli/tests/run.rs`, `a_timed_run_prints_a_score_for_every_benchmark`'s expected names:
   ```rust
        [
            "Bounce",
            "DeltaBlue",
            "DeltaBlueTon80",
            "Havlak",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Richards",
            "RichardsTon80",
            "Sieve",
            "Storage",
            "Towers"
        ]
   ```

- [ ] **Step 3: Run it through the harness, in a release build.**

```bash
cargo build --release -p ns-cli
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Havlak
./target/release/ns-next run --lowering reference benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Havlak
```
Expected: `Havlak: ok`, exit status 0, both times. It takes about 5 s under the optimized lowering, about 20 s under the reference one.

- [ ] **Step 4: Run the per-commit gate.** Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add benchmarks/are-we-fast-yet goldens/awfy crates/ns-cli/tests/run.rs
git commit -m "feat: Havlak joins the Are We Fast Yet harness"
```

---

### Task 9: CD

**The port** is SOMns's `CD.ns`, with SOMns's second module `RedBlackTree.ns` brought in as members of `CD`:
- the class `RedBlackTree`;
- its helper classes `Node`, `Entry` and `InsertResult`;
- `treeMinimum:`.

Its header names both upstream files. The members sit beside `RedBlackTree` rather than inside it, as they sat in their module. Inside it, every tree would mint its own `Node` class, and `draw:` makes a tree per motion.

`bench` runs 250 aircraft, Are We Fast Yet's size, and checks for 10830 collisions.

**The translations.** The header records each:
- **Float division.** SOMns's `//` on floats is float division, so it is `/`, and an integer receiver's `i // 10.0` is `frame asFloat / 10.0`, since ns-next has no `Integer>>/`.
- **`asInteger`** truncates toward zero, as SOMns's and Primordial Soup's do; the values can be negative.

**Files:**
- Create: `benchmarks/are-we-fast-yet/CD.ns`
- Modify: the registration places

**Interfaces:**
- Consumes: `sin`, `cos`, `asInteger` (Task 3); `List` (Task 4).
- Produces: the module `CD`.

- [ ] **Step 1: Write `CD.ns`.**

`benchmarks/are-we-fast-yet/CD.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/CD.ns and core-lib/Benchmarks/RedBlackTree.ns), one
   of the Are We Fast Yet benchmarks.
   Changed for ns-next: a module in Primordial Soup's shape, whose bench
   runs once at 250 aircraft and checks its own result; the module
   RedBlackTree.ns, which SOMns loads beside CD.ns, became the nested class
   RedBlackTree, its helper classes and treeMinimum: members of CD beside
   it; only Primordial Soup's protocol is sent, so SOMns's Vector is List,
   its float division // is /, ~= and <> are (a == b) not and (a = b) not;
   names are written out.

   SOMns's own header of both files:

     Ported from the adapted JavaScript and Java versions.

   Copyright (c) 2001-2010, Purdue University. All rights reserved.
   Copyright (C) 2015 Apple Inc. All rights reserved.

   Redistribution and use in source and binary forms, with or without
   modification, are permitted provided that the following conditions are met:
    * Redistributions of source code must retain the above copyright
      notice, this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright
      notice, this list of conditions and the following disclaimer in the
      documentation and/or other materials provided with the distribution.
    * Neither the name of the Purdue University nor the
      names of its contributors may be used to endorse or promote products
      derived from this software without specific prior written permission.

   THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
   ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
   WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
   DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER BE LIABLE FOR ANY
   DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
   (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
   LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
   ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
   (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
   SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE. *)
class CD usingPlatform: platform = (
    | private Error = platform kernel Error.
      private List = platform collections List.

      private MinX = 0.0.
      private MinY = 0.0.
      private MaxX = 1000.0.
      private MaxY = 1000.0.
      private MinZ = 0.0.
      private MaxZ = 10.0.
      private ProximityRadius = 1.0.
      private GoodVoxelSize = ProximityRadius * 2.0.

      private horizontal = Vector2D coordinateX: GoodVoxelSize coordinateY: 0.0.
      private vertical = Vector2D coordinateX: 0.0 coordinateY: GoodVoxelSize. |
) (
    public bench = (
        | collisions |
        collisions:: benchmark: 250.
        collisions = 10830 ifFalse: [
            Error signal: 'CD: expected 10830, got ', collisions printString].
        ^collisions
    )
    benchmark: aircraftCount = (
        | frameCount simulator detector actualCollisions |
        frameCount:: 200.

        simulator:: Simulator new: aircraftCount.
        detector:: CollisionDetector new.

        actualCollisions:: 0.

        0 to: frameCount - 1 do: [:frame |
            | time collisions |
            time:: frame asFloat / 10.0.
            collisions:: detector handleNewFrame: (simulator simulate: time).
            actualCollisions:: actualCollisions + collisions size].

        ^actualCollisions
    )

    private class Vector2D coordinateX: coordinateX coordinateY: coordinateY = (
        | public coordinateX = coordinateX.
          public coordinateY = coordinateY. |
    ) (
        public plus: other = (
            ^Vector2D
                coordinateX: coordinateX + other coordinateX
                coordinateY: coordinateY + other coordinateY
        )

        public minus: other = (
            ^Vector2D
                coordinateX: coordinateX - other coordinateX
                coordinateY: coordinateY - other coordinateY
        )

        public compareTo: other = (
            | result |
            result:: compare: coordinateX and: other coordinateX.
            (result = 0) not ifTrue: [^result].
            ^compare: coordinateY and: other coordinateY
        )

        private compare: left and: right = (
            left = right ifTrue: [^0].
            left < right ifTrue: [^-1].
            left > right ifTrue: [^1].

            (* We say that NaN is smaller than non-NaN. *)
            left = left ifTrue: [^1].
            ^-1
        )
    )

    private class Vector3D coordinateX: coordinateX coordinateY: coordinateY coordinateZ: coordinateZ = (
        | public coordinateX = coordinateX.
          public coordinateY = coordinateY.
          public coordinateZ = coordinateZ. |
    ) (
        public plus: other = (
            ^Vector3D
                coordinateX: coordinateX + other coordinateX
                coordinateY: coordinateY + other coordinateY
                coordinateZ: coordinateZ + other coordinateZ
        )

        public minus: other = (
            ^Vector3D
                coordinateX: coordinateX - other coordinateX
                coordinateY: coordinateY - other coordinateY
                coordinateZ: coordinateZ - other coordinateZ
        )

        public dot: other = (
            ^(coordinateX * other coordinateX)
                + (coordinateY * other coordinateY)
                + (coordinateZ * other coordinateZ)
        )

        public squaredMagnitude = (
            ^self dot: self
        )

        public magnitude = (
            ^squaredMagnitude sqrt
        )

        public times: amount = (
            ^Vector3D
                coordinateX: coordinateX * amount
                coordinateY: coordinateY * amount
                coordinateZ: coordinateZ * amount
        )
    )

    private class CallSign new: number = (
        | public value = number. |
    ) (
        public compareTo: other = (
            ^value = other value
                ifTrue: [0]
                ifFalse: [value < other value ifTrue: [-1] ifFalse: [1]]
        )
    )

    private class Collision aircraftA: aircraftA aircraftB: aircraftB position: position = (
        | public aircraftA = aircraftA.
          public aircraftB = aircraftB.
          public position = position. |
    ) ()

    private class CollisionDetector = (
        | private state = RedBlackTree new. |
    ) (
        public handleNewFrame: frame = (
            | motions seen toRemove allReduced collisions |
            motions:: List new.
            seen:: RedBlackTree new.

            frame do: [:aircraft |
                | oldPosition newPosition |
                oldPosition:: state at: aircraft callsign put: aircraft position.
                newPosition:: aircraft position.
                seen at: aircraft callsign put: true.

                oldPosition isNil ifTrue: [
                    (* Treat newly introduced aircraft as if they were stationary *)
                    oldPosition:: newPosition].

                motions add: (Motion new: aircraft callsign old: oldPosition new: newPosition)].

            (* Remove aircraft that are no longer present *)
            toRemove:: List new.
            state do: [:entry |
                (seen at: entry key) ifFalse: [toRemove add: entry key]].

            toRemove do: [:callsign | state remove: callsign].

            allReduced:: reduceCollisionSet: motions.
            collisions:: List new.
            allReduced do: [:reduced |
                1 to: reduced size do: [:first |
                    | motionOne |
                    motionOne:: reduced at: first.
                    first + 1 to: reduced size do: [:second |
                        | motionTwo collision |
                        motionTwo:: reduced at: second.
                        collision:: motionOne findIntersection: motionTwo.
                        collision notNil ifTrue: [
                            collisions add: (Collision
                                aircraftA: motionOne callsign
                                aircraftB: motionTwo callsign
                                position: collision)]]]].

            ^collisions
        )

        private isInVoxel: voxel motion: motion = (
            | start finish voxelSize radius voxelX startX deltaX voxelY startY deltaY lowX highX lowY highY |
            (voxel coordinateX > MaxX or: [
             voxel coordinateX < MinX or: [
             voxel coordinateY > MaxY or: [
             voxel coordinateY < MinY]]]) ifTrue: [^false].

            start:: motion positionOne.
            finish:: motion positionTwo.

            voxelSize:: GoodVoxelSize.
            radius:: ProximityRadius / 2.0.

            voxelX:: voxel coordinateX.
            startX:: start coordinateX.
            deltaX:: finish coordinateX - start coordinateX.

            voxelY:: voxel coordinateY.
            startY:: start coordinateY.
            deltaY:: finish coordinateY - start coordinateY.

            lowX:: (voxelX - radius - startX) / deltaX.
            highX:: (voxelX + voxelSize + radius - startX) / deltaX.

            deltaX < 0.0 ifTrue: [
                | temp |
                temp:: lowX.
                lowX:: highX.
                highX:: temp].

            lowY:: (voxelY - radius - startY) / deltaY.
            highY:: (voxelY + voxelSize + radius - startY) / deltaY.

            deltaY < 0.0 ifTrue: [
                | temp |
                temp:: lowY.
                lowY:: highY.
                highY:: temp].

            ^(((deltaX = 0.0 and: [voxelX <= (startX + radius) and: [(startX - radius) <= (voxelX + voxelSize)]]) or: [ (* no motion in x *)
                (lowX <= 1.0 and: [1.0 <= highX]) or: [
                (lowX <= 0.0 and: [0.0 <= highX]) or: [
                (0.0 <= lowX and: [highX <= 1.0])]]]) and: [

                (deltaY = 0.0 and: [voxelY <= (startY + radius) and: [(startY - radius) <= (voxelY + voxelSize)]]) or: [ (* no motion in y *)
                  (lowY <= 1.0 and: [1.0 <= highY]) or: [
                  (lowY <= 0.0 and: [0.0 <= highY]) or: [
                  (0.0 <= lowY and: [highY <= 1.0])]]]]) and: [

                 deltaX = 0.0 or: [
                 deltaY = 0.0 or: [ (* no motion in x or y or both *)
                 (lowY <= highX and: [highX <= highY]) or: [
                 (lowY <= lowX and: [lowX <= highY]) or: [
                 (lowX <= lowY and: [highY <= highX])]]]]]
        )

        private put: motion and: voxel into: voxelMap = (
            | array |
            array:: voxelMap at: voxel.
            array isNil ifTrue: [
                array:: List new.
                voxelMap at: voxel put: array].
            array add: motion
        )

        private recurse: voxelMap seen: seen voxel: nextVoxel motion: motion = (
            (isInVoxel: nextVoxel motion: motion) ifFalse: [^self].
            (seen at: nextVoxel put: true) = true ifTrue: [^self].

            put: motion and: nextVoxel into: voxelMap.

            recurse: voxelMap seen: seen voxel: (nextVoxel minus: horizontal) motion: motion.
            recurse: voxelMap seen: seen voxel: (nextVoxel plus: horizontal) motion: motion.
            recurse: voxelMap seen: seen voxel: (nextVoxel minus: vertical) motion: motion.
            recurse: voxelMap seen: seen voxel: (nextVoxel plus: vertical) motion: motion.
            recurse: voxelMap seen: seen voxel: ((nextVoxel minus: horizontal) minus: vertical) motion: motion.
            recurse: voxelMap seen: seen voxel: ((nextVoxel minus: horizontal) plus: vertical) motion: motion.
            recurse: voxelMap seen: seen voxel: ((nextVoxel plus: horizontal) minus: vertical) motion: motion.
            recurse: voxelMap seen: seen voxel: ((nextVoxel plus: horizontal) plus: vertical) motion: motion
        )

        private reduceCollisionSet: motions = (
            | voxelMap result |
            voxelMap:: RedBlackTree new.
            motions do: [:motion | draw: motion on: voxelMap].

            result:: List new.
            voxelMap do: [:entry |
                entry value size > 1 ifTrue: [result add: entry value]].
            ^result
        )

        private voxelHash: position = (
            | quotientX quotientY voxelX voxelY |
            quotientX:: (position coordinateX / GoodVoxelSize) asInteger.
            quotientY:: (position coordinateY / GoodVoxelSize) asInteger.

            voxelX:: GoodVoxelSize * quotientX.
            voxelY:: GoodVoxelSize * quotientY.

            position coordinateX < 0 ifTrue: [voxelX:: voxelX - GoodVoxelSize].
            position coordinateY < 0 ifTrue: [voxelY:: voxelY - GoodVoxelSize].

            ^Vector2D coordinateX: voxelX coordinateY: voxelY
        )

        private draw: motion on: voxelMap = (
            | seen |
            seen:: RedBlackTree new.
            recurse: voxelMap seen: seen voxel: (voxelHash: motion positionOne) motion: motion
        )
    )

    private class Motion new: callsign old: positionOne new: positionTwo = (
        | public callsign = callsign.
          public positionOne = positionOne.
          public positionTwo = positionTwo. |
    ) (
        public delta = (
            ^positionTwo minus: positionOne
        )

        public findIntersection: other = (
            | initialOne initialTwo vectorOne vectorTwo radius squaredSpeed distance |
            initialOne:: positionOne.
            initialTwo:: other positionOne.
            vectorOne:: delta.
            vectorTwo:: other delta.
            radius:: ProximityRadius.

            (* this test is not geometrical 3-d intersection test, it takes the fact that the aircraft move
               into account ; so it is more like a 4d test
               (it assumes that both of the aircraft have a constant speed over the tested interval)

               we thus have two points, each of them moving on its line segment at constant speed ; we are looking
               for times when the distance between these two points is smaller than r

               vectorOne is vector of aircraft 1
               vectorTwo is vector of aircraft 2

               a = (V2 - V1)^T * (V2 - V1) *)
            squaredSpeed:: (vectorTwo minus: vectorOne) squaredMagnitude.

            (squaredSpeed = 0.0) not ifTrue: [
                | linear constant discriminant timeOne timeTwo |
                (* we are first looking for instances of time when the planes are exactly r from each other
                   at least one plane is moving ; if the planes are moving in parallel, they do not have constant speed

                   if the planes are moving in parallel, then
                     if the faster starts behind the slower, we can have 2, 1, or 0 solutions
                     if the faster plane starts in front of the slower, we can have 0 or 1 solutions

                   if the planes are not moving in parallel, then

                   point P1 = I1 + vV1
                   point P2 = I2 + vV2
                     - looking for v, such that dist(P1,P2) = || P1 - P2 || = r

                   it follows that || P1 - P2 || = sqrt( < P1-P2, P1-P2 > )
                     0 = -r^2 + < P1 - P2, P1 - P2 >
                    from properties of dot product
                     0 = -r^2 + <I1-I2,I1-I2> + v * 2<I1-I2, V1-V2> + v^2 *<V1-V2,V1-V2>
                     so we calculate a, b, c - and solve the quadratic equation
                     0 = c + bv + av^2

                 b = 2 * <I1-I2, V1-V2> *)
                linear:: 2.0 * ((initialOne minus: initialTwo) dot: (vectorOne minus: vectorTwo)).

                (* c = -r^2 + (I2 - I1)^T * (I2 - I1) *)
                constant:: ((0.0 - radius) * radius) + ((initialTwo minus: initialOne) squaredMagnitude).

                discriminant:: (linear * linear) - (4.0 * squaredSpeed * constant).
                discriminant < 0.0 ifTrue: [^nil].

                timeOne:: ((0.0 - linear) - discriminant sqrt) / (2.0 * squaredSpeed).
                timeTwo:: ((0.0 - linear) + discriminant sqrt) / (2.0 * squaredSpeed).

                (timeOne <= timeTwo and: [((timeOne <= 1.0 and: [1.0 <= timeTwo]) or: [
                                       (timeOne <= 0.0 and: [0.0 <= timeTwo]) or: [
                                       (0.0 <= timeOne and: [timeTwo <= 1.0])]])]) ifTrue: [
                    (* Pick a good "time" at which to report the collision *)
                    | time resultOne resultTwo result |
                    timeOne <= 0.0
                        ifTrue: [
                            (* The collision started before this frame. Report it at the start of the frame *)
                            time:: 0.0]
                        ifFalse: [
                            (* The collision started during this frame. Report it at that moment *)
                            time:: timeOne].

                    resultOne:: initialOne plus: (vectorOne times: time).
                    resultTwo:: initialTwo plus: (vectorTwo times: time).

                    result:: (resultOne plus: resultTwo) times: 0.5.

                    (result coordinateX >= MinX and: [
                     result coordinateX <= MaxX and: [
                     result coordinateY >= MinY and: [
                     result coordinateY <= MaxY and: [
                     result coordinateZ >= MinZ and: [
                     result coordinateZ <= MaxZ]]]]]) ifTrue: [^result]].

                ^nil].

            (* the planes have the same speeds and are moving in parallel (or they are not moving at all)
               they  thus have the same distance all the time ; we calculate it from the initial point

               dist = || i2 - i1 || = sqrt(  ( i2 - i1 )^T * ( i2 - i1 ) ) *)
            distance:: (initialTwo minus: initialOne) magnitude.
            distance <= radius ifTrue: [
                ^(initialOne plus: initialTwo) times: 0.5].

            ^nil
        )
    )

    private class Aircraft new: callsign position: position = (
        | public callsign = callsign.
          public position = position. |
    ) ()

    private class Simulator new: aircraftCount = (
        | private aircrafts = List new. |
        0 to: aircraftCount - 1 do: [:index |
            aircrafts add: (CallSign new: index)]
    ) (
        public simulate: time = (
            | frame |
            frame:: List new.
            0 to: aircrafts size - 2 by: 2 do: [:index |
                frame add: (Aircraft
                    new: (aircrafts at: index + 1)
                    position: (Vector3D
                        coordinateX: time
                        coordinateY: (time cos * 2.0) + (index * 3.0)
                        coordinateZ: 10.0)).
                frame add: (Aircraft
                    new: (aircrafts at: index + 2)
                    position: (Vector3D
                        coordinateX: time
                        coordinateY: (time sin * 2.0) + (index * 3.0)
                        coordinateZ: 10.0))].
            ^frame
        )
    )

    (* SOMns's RedBlackTree.ns: a red-black tree keyed by objects that
       answer compareTo:. *)
    private class Node key: key value: value = (
        | public key = key.
          public value ::= value.
          public left ::= nil.
          public right ::= nil.
          public parent ::= nil.
          public color ::= #red. |
    ) (
        public successor = (
            | node ancestor |
            node:: self.
            node right notNil ifTrue: [
                ^treeMinimum: node right].

            ancestor:: node parent.
            [ancestor notNil and: [node == ancestor right]] whileTrue: [
                node:: ancestor.
                ancestor:: ancestor parent].
            ^ancestor
        )
    )

    private class Entry key: key value: value = (
        | public key = key.
          public value = value. |
    ) ()

    private class InsertResult new: isNewEntry node: newNode value: oldValue = (
        | public isNewEntry = isNewEntry.
          public newNode = newNode.
          public oldValue = oldValue. |
    ) ()

    private class RedBlackTree = (
        | private root ::= nil. |
    ) (
        (* Answers the value the key held before, or nil when it is new;
           CollisionDetector relies on it. *)
        public at: key put: value = (
            | insertionResult node |
            insertionResult:: treeAt: key insert: value.
            insertionResult isNewEntry ifFalse: [
                ^insertionResult oldValue].

            node:: insertionResult newNode.

            [(node == root) not and: [node parent color = #red]] whileTrue: [
                node parent == node parent parent left
                    ifTrue: [
                        | uncle |
                        uncle:: node parent parent right.
                        (uncle notNil and: [uncle color = #red])
                            ifTrue: [
                                (* Case 1 *)
                                node parent color: #black.
                                uncle color: #black.
                                node parent parent color: #red.
                                node:: node parent parent]
                            ifFalse: [
                                node == node parent right ifTrue: [
                                    (* Case 2 *)
                                    node:: node parent.
                                    leftRotate: node].

                                (* Case 3 *)
                                node parent color: #black.
                                node parent parent color: #red.
                                rightRotate: node parent parent]]
                    ifFalse: [
                        (* Same as "then" clause with "right" and "left" exchanged *)
                        | uncle |
                        uncle:: node parent parent left.
                        (uncle notNil and: [uncle color = #red])
                            ifTrue: [
                                (* Case 1 *)
                                node parent color: #black.
                                uncle color: #black.
                                node parent parent color: #red.
                                node:: node parent parent]
                            ifFalse: [
                                node == node parent left ifTrue: [
                                    (* Case 2 *)
                                    node:: node parent.
                                    rightRotate: node].

                                (* Case 3 *)
                                node parent color: #black.
                                node parent parent color: #red.
                                leftRotate: node parent parent]]].

            root color: #black.
            ^nil
        )

        public remove: key = (
            | child spliced target childParent |
            target:: findNode: key.
            target isNil ifTrue: [^nil].

            (* Spliced is the node to be unlinked from the tree. *)
            (target left isNil or: [target right isNil])
                ifTrue: [spliced:: target]
                ifFalse: [spliced:: target successor].

            (* Spliced is guaranteed to be non-null at this point. *)
            spliced left notNil
                ifTrue: [child:: spliced left]
                ifFalse: [child:: spliced right].

            (* Child is the child of spliced which might potentially replace
               spliced in the tree. Child might be null at this point. *)
            child notNil
                ifTrue: [
                    child parent: spliced parent.
                    childParent:: child parent]
                ifFalse: [
                    childParent:: spliced parent].

            spliced parent isNil
                ifTrue: [root:: child]
                ifFalse: [
                    spliced == spliced parent left
                        ifTrue: [spliced parent left: child]
                        ifFalse: [spliced parent right: child]].

            (spliced == target) not
                ifTrue: [
                    spliced color = #black ifTrue: [
                        remove: child andFixup: childParent].

                    spliced parent: target parent.
                    spliced color: target color.
                    spliced left: target left.
                    spliced right: target right.

                    target left notNil ifTrue: [
                        target left parent: spliced].
                    target right notNil ifTrue: [
                        target right parent: spliced].

                    target parent notNil
                        ifTrue: [
                            target parent left == target
                                ifTrue: [target parent left: spliced]
                                ifFalse: [target parent right: spliced]]
                        ifFalse: [root:: spliced]]
                ifFalse: [
                    spliced color = #black ifTrue: [
                        remove: child andFixup: childParent]].

            ^target value
        )

        public at: key = (
            | node |
            node:: findNode: key.
            node isNil ifTrue: [^nil].
            ^node value
        )

        public contains: key = (
            ^(findNode: key) notNil
        )

        public do: block = (
            | current |
            root isNil ifTrue: [^self].
            current:: treeMinimum: root.
            [current notNil] whileTrue: [
                block value: (Entry key: current key value: current value).
                current:: current successor]
        )

        private findNode: key = (
            | current |
            current:: root.
            [current notNil] whileTrue: [
                | comparisonResult |
                comparisonResult:: key compareTo: current key.
                comparisonResult = 0 ifTrue: [^current].
                comparisonResult < 0
                    ifTrue: [current:: current left]
                    ifFalse: [current:: current right]].
            ^nil
        )

        private treeAt: key insert: value = (
            | current parent newNode |
            parent:: nil.
            current:: root.

            [current notNil] whileTrue: [
                | comparisonResult |
                parent:: current.
                comparisonResult:: key compareTo: current key.
                comparisonResult < 0
                    ifTrue: [current:: current left]
                    ifFalse: [
                        comparisonResult > 0
                            ifTrue: [current:: current right]
                            ifFalse: [
                                | oldValue |
                                oldValue:: current value.
                                current value: value.
                                ^InsertResult new: false node: nil value: oldValue]]].

            newNode:: Node key: key value: value.
            newNode parent: parent.
            parent isNil
                ifTrue: [root:: newNode]
                ifFalse: [
                    (key compareTo: parent key) < 0
                        ifTrue: [parent left: newNode]
                        ifFalse: [parent right: newNode]].

            ^InsertResult new: true node: newNode value: nil
        )

        private leftRotate: node = (
            | pivot |
            pivot:: node right.

            (* Turn pivot's left subtree into node's right subtree *)
            node right: pivot left.
            pivot left notNil ifTrue: [
                pivot left parent: node].

            (* Link node's parent to pivot *)
            pivot parent: node parent.
            node parent isNil
                ifTrue: [root:: pivot]
                ifFalse: [
                    node == node parent left
                        ifTrue: [node parent left: pivot]
                        ifFalse: [node parent right: pivot]].

            (* Put node on pivot's left *)
            pivot left: node.
            node parent: pivot.
            ^pivot
        )

        private rightRotate: node = (
            | pivot |
            pivot:: node left.

            (* Turn pivot's right subtree into node's left subtree *)
            node left: pivot right.
            pivot right notNil ifTrue: [pivot right parent: node].

            (* Link node's parent to pivot *)
            pivot parent: node parent.
            node parent isNil
                ifTrue: [root:: pivot]
                ifFalse: [
                    node == node parent left
                        ifTrue: [node parent left: pivot]
                        ifFalse: [node parent right: pivot]].

            pivot right: node.
            node parent: pivot.
            ^pivot
        )

        private remove: startChild andFixup: startParent = (
            | child childParent |
            child:: startChild.
            childParent:: startParent.

            [(child == root) not and: [child isNil or: [child color = #black]]] whileTrue: [
                child == childParent left
                    ifTrue: [
                        | sibling |
                        (* Note: the text points out that sibling cannot be null. The reason is not obvious from
                           simply looking at the code; it comes about from the properties of the red-black
                           tree. *)
                        sibling:: childParent right.
                        sibling color = #red ifTrue: [
                            (* Case 1 *)
                            sibling color: #black.
                            childParent color: #red.
                            leftRotate: childParent.
                            sibling:: childParent right].

                        ((sibling left isNil or: [sibling left color = #black]) and: [
                          sibling right isNil or: [sibling right color = #black]])
                            ifTrue: [
                                (* Case 2 *)
                                sibling color: #red.
                                child:: childParent.
                                childParent:: child parent]
                            ifFalse: [
                                (sibling right isNil or: [sibling right color = #black]) ifTrue: [
                                    (* Case 3 *)
                                    sibling left color: #black.
                                    sibling color: #red.
                                    rightRotate: sibling.
                                    sibling:: childParent right].

                                (* Case 4 *)
                                sibling color: childParent color.
                                childParent color: #black.
                                sibling right notNil ifTrue: [
                                    sibling right color: #black].

                                leftRotate: childParent.
                                child:: root.
                                childParent:: child parent]]
                    ifFalse: [
                        | sibling |
                        (* Same as "then" clause with "right" and "left" exchanged *)
                        sibling:: childParent left.
                        sibling color = #red ifTrue: [
                            (* Case 1 *)
                            sibling color: #black.
                            childParent color: #red.
                            rightRotate: childParent.
                            sibling:: childParent left].

                        ((sibling right isNil or: [sibling right color = #black]) and: [
                         (sibling left isNil or: [sibling left color = #black])])
                            ifTrue: [
                                (* Case 2 *)
                                sibling color: #red.
                                child:: childParent.
                                childParent:: child parent]
                            ifFalse: [
                                (sibling left isNil or: [sibling left color = #black]) ifTrue: [
                                    (* Case 3 *)
                                    sibling right color: #black.
                                    sibling color: #red.
                                    leftRotate: sibling.
                                    sibling:: childParent left].

                                (* Case 4 *)
                                sibling color: childParent color.
                                childParent color: #black.
                                sibling left notNil ifTrue: [
                                    sibling left color: #black].

                                rightRotate: childParent.
                                child:: root.
                                childParent:: child parent]]].

            child notNil ifTrue: [child color: #black]
        )
    )

    private treeMinimum: node = (
        | current |
        current:: node.
        [current left notNil] whileTrue: [
            current:: current left].
        ^current
    )
)
```

- [ ] **Step 2: Register it.**

1. `AreWeFastYet.ns`, the `benchmarks` slot:
   ```newspeak
    | private benchmarks = {
        manifest Bounce. manifest CD. manifest DeltaBlue.
        manifest DeltaBlueTon80. manifest Havlak. manifest List.
        manifest Mandelbrot. manifest NBody. manifest Permute.
        manifest Queens. manifest Richards. manifest RichardsTon80.
        manifest Sieve. manifest Storage. manifest Towers }. |
   ```
2. `are-we-fast-yet.json`:
   ```json
{
  "root": "AreWeFastYet.ns",
  "modules": ["Bounce.ns", "CD.ns", "DeltaBlue.ns", "DeltaBlueTon80.ns", "Havlak.ns", "List.ns", "Mandelbrot.ns", "NBody.ns", "Permute.ns", "Queens.ns", "Richards.ns", "RichardsTon80.ns", "Sieve.ns", "Storage.ns", "Towers.ns"]
}
   ```
3. Each of `goldens/awfy/a_named_benchmark_runs_alone.json`, `all_benchmarks_verify.json`, `an_unknown_benchmark_is_refused.json` and `extra_arguments_are_a_usage_error.json`:
   ```json
{
  "root": "../../benchmarks/are-we-fast-yet/AreWeFastYet.ns",
  "modules": [
    "../../benchmarks/are-we-fast-yet/Bounce.ns",
    "../../benchmarks/are-we-fast-yet/CD.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlue.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlueTon80.ns",
    "../../benchmarks/are-we-fast-yet/Havlak.ns",
    "../../benchmarks/are-we-fast-yet/List.ns",
    "../../benchmarks/are-we-fast-yet/Mandelbrot.ns",
    "../../benchmarks/are-we-fast-yet/NBody.ns",
    "../../benchmarks/are-we-fast-yet/Permute.ns",
    "../../benchmarks/are-we-fast-yet/Queens.ns",
    "../../benchmarks/are-we-fast-yet/Richards.ns",
    "../../benchmarks/are-we-fast-yet/RichardsTon80.ns",
    "../../benchmarks/are-we-fast-yet/Sieve.ns",
    "../../benchmarks/are-we-fast-yet/Storage.ns",
    "../../benchmarks/are-we-fast-yet/Towers.ns"
  ]
}
   ```
4. `goldens/awfy/all_benchmarks_verify.toml`:
   ```toml
stdout = '''
Bounce: ok
CD: ok
DeltaBlue: ok
DeltaBlueTon80: ok
Havlak: ok
List: ok
Mandelbrot: ok
NBody: ok
Permute: ok
Queens: ok
Richards: ok
RichardsTon80: ok
Sieve: ok
Storage: ok
Towers: ok
'''
   ```
5. `crates/ns-cli/tests/run.rs`, `a_timed_run_prints_a_score_for_every_benchmark`'s expected names:
   ```rust
        [
            "Bounce",
            "CD",
            "DeltaBlue",
            "DeltaBlueTon80",
            "Havlak",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Richards",
            "RichardsTon80",
            "Sieve",
            "Storage",
            "Towers"
        ]
   ```

- [ ] **Step 3: Run it through the harness, in a release build.**

```bash
cargo build --release -p ns-cli
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- CD
./target/release/ns-next run --lowering reference benchmarks/are-we-fast-yet/are-we-fast-yet.json -- CD
```
Expected: `CD: ok`, exit status 0, both times. It takes about 5 s under the optimized lowering, about 13 s under the reference one.

- [ ] **Step 4: Run the per-commit gate.** Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add benchmarks/are-we-fast-yet goldens/awfy crates/ns-cli/tests/run.rs
git commit -m "feat: CD, with its red-black tree, joins the Are We Fast Yet harness"
```

---

### Task 10: Json

**The port** is SOMns's `Json.ns`. `bench` parses its input once, Are We Fast Yet's 100 being a repetition count. It checks what SOMns's `verifyResult:` checks:
- the result is an object;
- `head` is an object;
- `operations` is an array of 156.

Primordial Soup's own `JSON.ns` is the model for the character handling:
- `at:` answers a character's code, so every character is compared as a radix literal with a comment naming it (`16r7B`);
- an escape appends `String with:` the code it answers.

The header records what is left out: code the benchmark never runs and that cannot run as upstream wrote it, and the number API, which would need `endsWith:` and `Integer`/`Float parse:`.

**The input literal** is 25,820 characters on one line, pure ASCII, with no backslash and no apostrophe. Copy it byte for byte. The comments of `readEscapeCharacter` hold backslashes: write the file with the editor's Write tool, never through the shell.

**Files:**
- Create: `benchmarks/are-we-fast-yet/Json.ns`
- Modify: the registration places

**Interfaces:**
- Consumes: String `at:`, `copyFrom:to:`, `String with:` (Task 2); `List` (Task 4).
- Produces: the module `Json`.

- [ ] **Step 1: Write `Json.ns`.**

`benchmarks/are-we-fast-yet/Json.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/Json.ns), one of the Are We Fast Yet benchmarks,
   which says of itself: "This is a rough port of minijson from Java."
   Changed for ns-next: a module in Primordial Soup's shape, whose bench
   parses the input once and checks what upstream's verifyResult: checks;
   only Primordial Soup's protocol is sent, so String at: answers a
   character's code, every character is compared as its code, and an
   escape appends String with: the code it answers; a parse error signals
   an Error naming its line and column instead of printing and answering a
   ParseException; the unsupported-operation errors describe the receiver
   with printString, since upstream's JsonValue>>asString sends itself
   without end; code the benchmark never runs and that cannot run as
   upstream wrote it is left out: JsonValue's class-side string: and
   boolean:, which send JsonLiteral NULL, TRUE and FALSE, and every method
   sending them or the absent JsonValue number:; JsonValue>>writeTo:,
   which sends the absent writeTo:with:, and writeOn:, whose
   subclassResponsibility Primordial Soup does not define;
   JsonObject>>updateHashIndex, which sends the absent add:put:;
   JsonObject>>=, which reads another object's private values; JsonObject>>remove: and
   HashIndexTable>>remove:, which pass a name for an index and add 1 to a
   Boolean; JsonArray>>removeAt:, which Primordial Soup's List lacks;
   the unused number API, which needs endsWith: or Integer and Float
   parse:, is left out too (JsonValue's class-side integer:, double: and
   cutOffPointZero:, asInteger and asDouble, and the JsonArray and
   JsonObject methods sending them); names are written out.

   Copyright (c) 2001-2015 see AUTHORS file

   Permission is hereby granted, free of charge, to any person obtaining a copy
   of this software and associated documentation files (the 'Software'), to deal
   in the Software without restriction, including without limitation the rights
   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
   copies of the Software, and to permit persons to whom the Software is
   furnished to do so, subject to the following conditions:

   The above copyright notice and this permission notice shall be included in
   all copies or substantial portions of the Software.

   THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
   THE SOFTWARE. *)
class Json usingPlatform: platform = (
    | private Error = platform kernel Error.
      private List = platform collections List.

      private NULL = JsonLiteral new: 'null'.
      private TRUE = JsonLiteral new: 'true'.
      private FALSE = JsonLiteral new: 'false'. |
) (
    public bench = (
        | result head operations |
        result:: (JsonParser with: RapBenchmarkMinified) parse.
        result isObject ifFalse: [
            Error signal: 'Json: expected an object, got ', result printString].
        head:: result asObject at: 'head'.
        head isObject ifFalse: [
            Error signal: 'Json: expected head to be an object, got ', head printString].
        operations:: result asObject at: 'operations'.
        operations isArray ifFalse: [
            Error signal: 'Json: expected operations to be an array, got ', operations printString].
        operations asArray size = 156 ifFalse: [
            Error signal: 'Json: expected 156 operations, got ', operations asArray size printString].
        ^result
    )
    RapBenchmarkMinified = (
        ^'{"head":{"requestCounter":4},"operations":[["destroy","w54"],["set","w2",{"activeControl":"w99"}],["set","w21",{"customVariant":"variant_navigation"}],["set","w28",{"customVariant":"variant_selected"}],["set","w53",{"children":["w95"]}],["create","w95","rwt.widgets.Composite",{"parent":"w53","style":["NONE"],"bounds":[0,0,1008,586],"children":["w96","w97"],"tabIndex":-1,"clientArea":[0,0,1008,586]}],["create","w96","rwt.widgets.Label",{"parent":"w95","style":["NONE"],"bounds":[10,30,112,26],"tabIndex":-1,"customVariant":"variant_pageHeadline","text":"TableViewer"}],["create","w97","rwt.widgets.Composite",{"parent":"w95","style":["NONE"],"bounds":[0,61,1008,525],"children":["w98","w99","w226","w228"],"tabIndex":-1,"clientArea":[0,0,1008,525]}],["create","w98","rwt.widgets.Text",{"parent":"w97","style":["LEFT","SINGLE","BORDER"],"bounds":[10,10,988,32],"tabIndex":22,"activeKeys":["#13","#27","#40"]}],["listen","w98",{"KeyDown":true,"Modify":true}],["create","w99","rwt.widgets.Grid",{"parent":"w97","style":["SINGLE","BORDER"],"appearance":"table","indentionWidth":0,"treeColumn":-1,"markupEnabled":false}],["create","w100","rwt.widgets.ScrollBar",{"parent":"w99","style":["HORIZONTAL"]}],["create","w101","rwt.widgets.ScrollBar",{"parent":"w99","style":["VERTICAL"]}],["set","w99",{"bounds":[10,52,988,402],"children":[],"tabIndex":23,"activeKeys":["CTRL+#70","CTRL+#78","CTRL+#82","CTRL+#89","CTRL+#83","CTRL+#71","CTRL+#69"],"cancelKeys":["CTRL+#70","CTRL+#78","CTRL+#82","CTRL+#89","CTRL+#83","CTRL+#71","CTRL+#69"]}],["listen","w99",{"MouseDown":true,"MouseUp":true,"MouseDoubleClick":true,"KeyDown":true}],["set","w99",{"itemCount":118,"itemHeight":28,"itemMetrics":[[0,0,50,3,0,3,44],[1,50,50,53,0,53,44],[2,100,140,103,0,103,134],[3,240,180,243,0,243,174],[4,420,50,423,0,423,44],[5,470,50,473,0,473,44]],"columnCount":6,"headerHeight":35,"headerVisible":true,"linesVisible":true,"focusItem":"w108","selection":["w108"]}],["listen","w99",{"Selection":true,"DefaultSelection":true}],["set","w99",{"enableCellToolTip":true}],["listen","w100",{"Selection":true}],["set","w101",{"visibility":true}],["listen","w101",{"Selection":true}],["create","w102","rwt.widgets.GridColumn",{"parent":"w99","text":"Nr.","width":50,"moveable":true}],["listen","w102",{"Selection":true}],["create","w103","rwt.widgets.GridColumn",{"parent":"w99","text":"Sym.","index":1,"left":50,"width":50,"moveable":true}],["listen","w103",{"Selection":true}],["create","w104","rwt.widgets.GridColumn",{"parent":"w99","text":"Name","index":2,"left":100,"width":140,"moveable":true}],["listen","w104",{"Selection":true}],["create","w105","rwt.widgets.GridColumn",{"parent":"w99","text":"Series","index":3,"left":240,"width":180,"moveable":true}],["listen","w105",{"Selection":true}],["create","w106","rwt.widgets.GridColumn",{"parent":"w99","text":"Group","index":4,"left":420,"width":50,"moveable":true}],["listen","w106",{"Selection":true}],["create","w107","rwt.widgets.GridColumn",{"parent":"w99","text":"Period","index":5,"left":470,"width":50,"moveable":true}],["listen","w107",{"Selection":true}],["create","w108","rwt.widgets.GridItem",{"parent":"w99","index":0,"texts":["1","H","Hydrogen","Nonmetal","1","1"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w109","rwt.widgets.GridItem",{"parent":"w99","index":1,"texts":["2","He","Helium","Noble gas","18","1"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w110","rwt.widgets.GridItem",{"parent":"w99","index":2,"texts":["3","Li","Lithium","Alkali metal","1","2"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w111","rwt.widgets.GridItem",{"parent":"w99","index":3,"texts":["4","Be","Beryllium","Alkaline earth metal","2","2"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w112","rwt.widgets.GridItem",{"parent":"w99","index":4,"texts":["5","B","Boron","Metalloid","13","2"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w113","rwt.widgets.GridItem",{"parent":"w99","index":5,"texts":["6","C","Carbon","Nonmetal","14","2"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w114","rwt.widgets.GridItem",{"parent":"w99","index":6,"texts":["7","N","Nitrogen","Nonmetal","15","2"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w115","rwt.widgets.GridItem",{"parent":"w99","index":7,"texts":["8","O","Oxygen","Nonmetal","16","2"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w116","rwt.widgets.GridItem",{"parent":"w99","index":8,"texts":["9","F","Fluorine","Halogen","17","2"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w117","rwt.widgets.GridItem",{"parent":"w99","index":9,"texts":["10","Ne","Neon","Noble gas","18","2"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w118","rwt.widgets.GridItem",{"parent":"w99","index":10,"texts":["11","Na","Sodium","Alkali metal","1","3"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w119","rwt.widgets.GridItem",{"parent":"w99","index":11,"texts":["12","Mg","Magnesium","Alkaline earth metal","2","3"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w120","rwt.widgets.GridItem",{"parent":"w99","index":12,"texts":["13","Al","Aluminium","Poor metal","13","3"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w121","rwt.widgets.GridItem",{"parent":"w99","index":13,"texts":["14","Si","Silicon","Metalloid","14","3"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w122","rwt.widgets.GridItem",{"parent":"w99","index":14,"texts":["15","P","Phosphorus","Nonmetal","15","3"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w123","rwt.widgets.GridItem",{"parent":"w99","index":15,"texts":["16","S","Sulfur","Nonmetal","16","3"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w124","rwt.widgets.GridItem",{"parent":"w99","index":16,"texts":["17","Cl","Chlorine","Halogen","17","3"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w125","rwt.widgets.GridItem",{"parent":"w99","index":17,"texts":["18","Ar","Argon","Noble gas","18","3"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w126","rwt.widgets.GridItem",{"parent":"w99","index":18,"texts":["19","K","Potassium","Alkali metal","1","4"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w127","rwt.widgets.GridItem",{"parent":"w99","index":19,"texts":["20","Ca","Calcium","Alkaline earth metal","2","4"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w128","rwt.widgets.GridItem",{"parent":"w99","index":20,"texts":["21","Sc","Scandium","Transition metal","3","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w129","rwt.widgets.GridItem",{"parent":"w99","index":21,"texts":["22","Ti","Titanium","Transition metal","4","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w130","rwt.widgets.GridItem",{"parent":"w99","index":22,"texts":["23","V","Vanadium","Transition metal","5","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w131","rwt.widgets.GridItem",{"parent":"w99","index":23,"texts":["24","Cr","Chromium","Transition metal","6","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w132","rwt.widgets.GridItem",{"parent":"w99","index":24,"texts":["25","Mn","Manganese","Transition metal","7","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w133","rwt.widgets.GridItem",{"parent":"w99","index":25,"texts":["26","Fe","Iron","Transition metal","8","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w134","rwt.widgets.GridItem",{"parent":"w99","index":26,"texts":["27","Co","Cobalt","Transition metal","9","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w135","rwt.widgets.GridItem",{"parent":"w99","index":27,"texts":["28","Ni","Nickel","Transition metal","10","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w136","rwt.widgets.GridItem",{"parent":"w99","index":28,"texts":["29","Cu","Copper","Transition metal","11","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w137","rwt.widgets.GridItem",{"parent":"w99","index":29,"texts":["30","Zn","Zinc","Transition metal","12","4"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w138","rwt.widgets.GridItem",{"parent":"w99","index":30,"texts":["31","Ga","Gallium","Poor metal","13","4"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w139","rwt.widgets.GridItem",{"parent":"w99","index":31,"texts":["32","Ge","Germanium","Metalloid","14","4"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w140","rwt.widgets.GridItem",{"parent":"w99","index":32,"texts":["33","As","Arsenic","Metalloid","15","4"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w141","rwt.widgets.GridItem",{"parent":"w99","index":33,"texts":["34","Se","Selenium","Nonmetal","16","4"],"cellBackgrounds":[null,null,null,[138,226,52,255],null,null]}],["create","w142","rwt.widgets.GridItem",{"parent":"w99","index":34,"texts":["35","Br","Bromine","Halogen","17","4"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w143","rwt.widgets.GridItem",{"parent":"w99","index":35,"texts":["36","Kr","Krypton","Noble gas","18","4"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w144","rwt.widgets.GridItem",{"parent":"w99","index":36,"texts":["37","Rb","Rubidium","Alkali metal","1","5"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w145","rwt.widgets.GridItem",{"parent":"w99","index":37,"texts":["38","Sr","Strontium","Alkaline earth metal","2","5"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w146","rwt.widgets.GridItem",{"parent":"w99","index":38,"texts":["39","Y","Yttrium","Transition metal","3","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w147","rwt.widgets.GridItem",{"parent":"w99","index":39,"texts":["40","Zr","Zirconium","Transition metal","4","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w148","rwt.widgets.GridItem",{"parent":"w99","index":40,"texts":["41","Nb","Niobium","Transition metal","5","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w149","rwt.widgets.GridItem",{"parent":"w99","index":41,"texts":["42","Mo","Molybdenum","Transition metal","6","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w150","rwt.widgets.GridItem",{"parent":"w99","index":42,"texts":["43","Tc","Technetium","Transition metal","7","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w151","rwt.widgets.GridItem",{"parent":"w99","index":43,"texts":["44","Ru","Ruthenium","Transition metal","8","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w152","rwt.widgets.GridItem",{"parent":"w99","index":44,"texts":["45","Rh","Rhodium","Transition metal","9","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w153","rwt.widgets.GridItem",{"parent":"w99","index":45,"texts":["46","Pd","Palladium","Transition metal","10","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w154","rwt.widgets.GridItem",{"parent":"w99","index":46,"texts":["47","Ag","Silver","Transition metal","11","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w155","rwt.widgets.GridItem",{"parent":"w99","index":47,"texts":["48","Cd","Cadmium","Transition metal","12","5"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w156","rwt.widgets.GridItem",{"parent":"w99","index":48,"texts":["49","In","Indium","Poor metal","13","5"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w157","rwt.widgets.GridItem",{"parent":"w99","index":49,"texts":["50","Sn","Tin","Poor metal","14","5"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w158","rwt.widgets.GridItem",{"parent":"w99","index":50,"texts":["51","Sb","Antimony","Metalloid","15","5"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w159","rwt.widgets.GridItem",{"parent":"w99","index":51,"texts":["52","Te","Tellurium","Metalloid","16","5"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w160","rwt.widgets.GridItem",{"parent":"w99","index":52,"texts":["53","I","Iodine","Halogen","17","5"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w161","rwt.widgets.GridItem",{"parent":"w99","index":53,"texts":["54","Xe","Xenon","Noble gas","18","5"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w162","rwt.widgets.GridItem",{"parent":"w99","index":54,"texts":["55","Cs","Caesium","Alkali metal","1","6"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w163","rwt.widgets.GridItem",{"parent":"w99","index":55,"texts":["56","Ba","Barium","Alkaline earth metal","2","6"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w164","rwt.widgets.GridItem",{"parent":"w99","index":56,"texts":["57","La","Lanthanum","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w165","rwt.widgets.GridItem",{"parent":"w99","index":57,"texts":["58","Ce","Cerium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w166","rwt.widgets.GridItem",{"parent":"w99","index":58,"texts":["59","Pr","Praseodymium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w167","rwt.widgets.GridItem",{"parent":"w99","index":59,"texts":["60","Nd","Neodymium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w168","rwt.widgets.GridItem",{"parent":"w99","index":60,"texts":["61","Pm","Promethium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w169","rwt.widgets.GridItem",{"parent":"w99","index":61,"texts":["62","Sm","Samarium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w170","rwt.widgets.GridItem",{"parent":"w99","index":62,"texts":["63","Eu","Europium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w171","rwt.widgets.GridItem",{"parent":"w99","index":63,"texts":["64","Gd","Gadolinium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w172","rwt.widgets.GridItem",{"parent":"w99","index":64,"texts":["65","Tb","Terbium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w173","rwt.widgets.GridItem",{"parent":"w99","index":65,"texts":["66","Dy","Dysprosium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w174","rwt.widgets.GridItem",{"parent":"w99","index":66,"texts":["67","Ho","Holmium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w175","rwt.widgets.GridItem",{"parent":"w99","index":67,"texts":["68","Er","Erbium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w176","rwt.widgets.GridItem",{"parent":"w99","index":68,"texts":["69","Tm","Thulium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w177","rwt.widgets.GridItem",{"parent":"w99","index":69,"texts":["70","Yb","Ytterbium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w178","rwt.widgets.GridItem",{"parent":"w99","index":70,"texts":["71","Lu","Lutetium","Lanthanide","3","6"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w179","rwt.widgets.GridItem",{"parent":"w99","index":71,"texts":["72","Hf","Hafnium","Transition metal","4","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w180","rwt.widgets.GridItem",{"parent":"w99","index":72,"texts":["73","Ta","Tantalum","Transition metal","5","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w181","rwt.widgets.GridItem",{"parent":"w99","index":73,"texts":["74","W","Tungsten","Transition metal","6","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w182","rwt.widgets.GridItem",{"parent":"w99","index":74,"texts":["75","Re","Rhenium","Transition metal","7","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w183","rwt.widgets.GridItem",{"parent":"w99","index":75,"texts":["76","Os","Osmium","Transition metal","8","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w184","rwt.widgets.GridItem",{"parent":"w99","index":76,"texts":["77","Ir","Iridium","Transition metal","9","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w185","rwt.widgets.GridItem",{"parent":"w99","index":77,"texts":["78","Pt","Platinum","Transition metal","10","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w186","rwt.widgets.GridItem",{"parent":"w99","index":78,"texts":["79","Au","Gold","Transition metal","11","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w187","rwt.widgets.GridItem",{"parent":"w99","index":79,"texts":["80","Hg","Mercury","Transition metal","12","6"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w188","rwt.widgets.GridItem",{"parent":"w99","index":80,"texts":["81","Tl","Thallium","Poor metal","13","6"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w189","rwt.widgets.GridItem",{"parent":"w99","index":81,"texts":["82","Pb","Lead","Poor metal","14","6"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w190","rwt.widgets.GridItem",{"parent":"w99","index":82,"texts":["83","Bi","Bismuth","Poor metal","15","6"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w191","rwt.widgets.GridItem",{"parent":"w99","index":83,"texts":["84","Po","Polonium","Metalloid","16","6"],"cellBackgrounds":[null,null,null,[156,159,153,255],null,null]}],["create","w192","rwt.widgets.GridItem",{"parent":"w99","index":84,"texts":["85","At","Astatine","Halogen","17","6"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w193","rwt.widgets.GridItem",{"parent":"w99","index":85,"texts":["86","Rn","Radon","Noble gas","18","6"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w194","rwt.widgets.GridItem",{"parent":"w99","index":86,"texts":["87","Fr","Francium","Alkali metal","1","7"],"cellBackgrounds":[null,null,null,[239,41,41,255],null,null]}],["create","w195","rwt.widgets.GridItem",{"parent":"w99","index":87,"texts":["88","Ra","Radium","Alkaline earth metal","2","7"],"cellBackgrounds":[null,null,null,[233,185,110,255],null,null]}],["create","w196","rwt.widgets.GridItem",{"parent":"w99","index":88,"texts":["89","Ac","Actinium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w197","rwt.widgets.GridItem",{"parent":"w99","index":89,"texts":["90","Th","Thorium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w198","rwt.widgets.GridItem",{"parent":"w99","index":90,"texts":["91","Pa","Protactinium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w199","rwt.widgets.GridItem",{"parent":"w99","index":91,"texts":["92","U","Uranium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w200","rwt.widgets.GridItem",{"parent":"w99","index":92,"texts":["93","Np","Neptunium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w201","rwt.widgets.GridItem",{"parent":"w99","index":93,"texts":["94","Pu","Plutonium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w202","rwt.widgets.GridItem",{"parent":"w99","index":94,"texts":["95","Am","Americium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w203","rwt.widgets.GridItem",{"parent":"w99","index":95,"texts":["96","Cm","Curium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w204","rwt.widgets.GridItem",{"parent":"w99","index":96,"texts":["97","Bk","Berkelium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w205","rwt.widgets.GridItem",{"parent":"w99","index":97,"texts":["98","Cf","Californium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w206","rwt.widgets.GridItem",{"parent":"w99","index":98,"texts":["99","Es","Einsteinium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w207","rwt.widgets.GridItem",{"parent":"w99","index":99,"texts":["100","Fm","Fermium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w208","rwt.widgets.GridItem",{"parent":"w99","index":100,"texts":["101","Md","Mendelevium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w209","rwt.widgets.GridItem",{"parent":"w99","index":101,"texts":["102","No","Nobelium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w210","rwt.widgets.GridItem",{"parent":"w99","index":102,"texts":["103","Lr","Lawrencium","Actinide","3","7"],"cellBackgrounds":[null,null,null,[173,127,168,255],null,null]}],["create","w211","rwt.widgets.GridItem",{"parent":"w99","index":103,"texts":["104","Rf","Rutherfordium","Transition metal","4","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w212","rwt.widgets.GridItem",{"parent":"w99","index":104,"texts":["105","Db","Dubnium","Transition metal","5","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w213","rwt.widgets.GridItem",{"parent":"w99","index":105,"texts":["106","Sg","Seaborgium","Transition metal","6","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w214","rwt.widgets.GridItem",{"parent":"w99","index":106,"texts":["107","Bh","Bohrium","Transition metal","7","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w215","rwt.widgets.GridItem",{"parent":"w99","index":107,"texts":["108","Hs","Hassium","Transition metal","8","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w216","rwt.widgets.GridItem",{"parent":"w99","index":108,"texts":["109","Mt","Meitnerium","Transition metal","9","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w217","rwt.widgets.GridItem",{"parent":"w99","index":109,"texts":["110","Ds","Darmstadtium","Transition metal","10","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w218","rwt.widgets.GridItem",{"parent":"w99","index":110,"texts":["111","Rg","Roentgenium","Transition metal","11","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w219","rwt.widgets.GridItem",{"parent":"w99","index":111,"texts":["112","Uub","Ununbium","Transition metal","12","7"],"cellBackgrounds":[null,null,null,[252,175,62,255],null,null]}],["create","w220","rwt.widgets.GridItem",{"parent":"w99","index":112,"texts":["113","Uut","Ununtrium","Poor metal","13","7"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w221","rwt.widgets.GridItem",{"parent":"w99","index":113,"texts":["114","Uuq","Ununquadium","Poor metal","14","7"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w222","rwt.widgets.GridItem",{"parent":"w99","index":114,"texts":["115","Uup","Ununpentium","Poor metal","15","7"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w223","rwt.widgets.GridItem",{"parent":"w99","index":115,"texts":["116","Uuh","Ununhexium","Poor metal","16","7"],"cellBackgrounds":[null,null,null,[238,238,236,255],null,null]}],["create","w224","rwt.widgets.GridItem",{"parent":"w99","index":116,"texts":["117","Uus","Ununseptium","Halogen","17","7"],"cellBackgrounds":[null,null,null,[252,233,79,255],null,null]}],["create","w225","rwt.widgets.GridItem",{"parent":"w99","index":117,"texts":["118","Uuo","Ununoctium","Noble gas","18","7"],"cellBackgrounds":[null,null,null,[114,159,207,255],null,null]}],["create","w226","rwt.widgets.Composite",{"parent":"w97","style":["BORDER"],"bounds":[10,464,988,25],"children":["w227"],"tabIndex":-1,"clientArea":[0,0,986,23]}],["create","w227","rwt.widgets.Label",{"parent":"w226","style":["NONE"],"bounds":[10,10,966,3],"tabIndex":-1,"text":"Hydrogen (H)"}],["create","w228","rwt.widgets.Label",{"parent":"w97","style":["WRAP"],"bounds":[10,499,988,16],"tabIndex":-1,"foreground":[150,150,150,255],"font":[["Verdana","Lucida Sans","Arial","Helvetica","sans-serif"],10,false,false],"text":"Shortcuts: [CTRL+F] - Filter | Sort by: [CTRL+R] - Number, [CTRL+Y] - Symbol, [CTRL+N] - Name, [CTRL+S] - Series, [CTRL+G] - Group, [CTRL+E] - Period"}],["set","w1",{"focusControl":"w99"}],["call","rwt.client.BrowserNavigation","addToHistory",{"entries":[["tableviewer","TableViewer"]]}]]}'
    )
    class JsonParser with: string = (
        | input = string.
          index ::= 0.
          line ::= 1.
          column ::= 0.
          current ::= nil.
          captureBuffer ::= ''.
          captureStart |
    ) (
        public parse = (
            | result |
            read.
            skipWhiteSpace.
            result:: readValue.
            skipWhiteSpace.
            isEndOfText ifFalse: [error: 'Unexpected character'].
            ^result
        )
        readValue = (
            current = 16r6E ifTrue: [^readNull]. (* n *)
            current = 16r74 ifTrue: [^readTrue]. (* t *)
            current = 16r66 ifTrue: [^readFalse]. (* f *)
            current = 16r22 ifTrue: [^readString]. (* " *)
            current = 16r5B ifTrue: [^readArray]. (* [ *)
            current = 16r7B ifTrue: [^readObject]. (* { *)

            (* Each digit is a comparison of its own, as upstream has it: the
               workload is the benchmark's, not a range check's. *)
            current = 16r2D ifTrue: [^readNumber]. (* - *)
            current = 16r30 ifTrue: [^readNumber]. (* 0 *)
            current = 16r31 ifTrue: [^readNumber]. (* 1 *)
            current = 16r32 ifTrue: [^readNumber]. (* 2 *)
            current = 16r33 ifTrue: [^readNumber]. (* 3 *)
            current = 16r34 ifTrue: [^readNumber]. (* 4 *)
            current = 16r35 ifTrue: [^readNumber]. (* 5 *)
            current = 16r36 ifTrue: [^readNumber]. (* 6 *)
            current = 16r37 ifTrue: [^readNumber]. (* 7 *)
            current = 16r38 ifTrue: [^readNumber]. (* 8 *)
            current = 16r39 ifTrue: [^readNumber]. (* 9 *)

            expected: 'value'
        )
        readArrayElement: array = (
            skipWhiteSpace.
            array add: readValue.
            skipWhiteSpace.
        )
        readArray = (
            | array |
            read.
            array:: JsonArray new.

            (* Array might be empty *)
            skipWhiteSpace.
            (readCharacter: 16r5D) ifTrue: [^array]. (* ] *)

            readArrayElement: array.
            [readCharacter: 16r2C] whileTrue: [readArrayElement: array]. (* , *)

            (readCharacter: 16r5D) ifFalse: [expected: '"," or "]"']. (* ] *)
            ^array
        )
        readObjectKeyValuePair: object = (
            | name |
            skipWhiteSpace.
            name:: readName.
            skipWhiteSpace.

            (readCharacter: 16r3A) ifFalse: [expected: ':']. (* : *)

            skipWhiteSpace.

            object add: name with: readValue.

            skipWhiteSpace.
        )
        readObject = (
            | object |
            read.
            object:: JsonObject new.
            skipWhiteSpace.

            (readCharacter: 16r7D) ifTrue: [^object]. (* } *)

            readObjectKeyValuePair: object.
            [readCharacter: 16r2C] whileTrue: [readObjectKeyValuePair: object]. (* , *)

            (readCharacter: 16r7D) ifFalse: [expected: '"," or "}"']. (* } *)

            ^object
        )
        readName = (
            current = 16r22 ifFalse: [expected: 'name']. (* " *)
            ^readStringInternal
        )
        readNull = (
            read.
            readRequiredCharacter: 16r75. (* u *)
            readRequiredCharacter: 16r6C. (* l *)
            readRequiredCharacter: 16r6C. (* l *)
            ^NULL
        )
        readTrue = (
            read.
            readRequiredCharacter: 16r72. (* r *)
            readRequiredCharacter: 16r75. (* u *)
            readRequiredCharacter: 16r65. (* e *)
            ^TRUE
        )
        readFalse = (
            read.
            readRequiredCharacter: 16r61. (* a *)
            readRequiredCharacter: 16r6C. (* l *)
            readRequiredCharacter: 16r73. (* s *)
            readRequiredCharacter: 16r65. (* e *)
            ^FALSE
        )
        readRequiredCharacter: character = (
            (readCharacter: character) ifFalse: [
                expected: 'character: ', (String with: character)]
        )
        readString = (
            ^JsonString new: readStringInternal
        )
        readStringInternal = (
            | string |
            read.
            startCapture.

            [current = 16r22] whileFalse: [ (* " *)
                current = 16r5C (* \ *)
                    ifTrue: [
                        pauseCapture.
                        readEscape.
                        startCapture]
                    ifFalse: [
                        (* The Java original rejects a control character
                           here; upstream does not, and neither does this
                           port, so the workload stays upstream's. *)
                        read]].
            string:: endCapture.
            read.
            ^string
        )
        readEscapeCharacter = (
            current = 16r22 ifTrue: [^16r22]. (* \" *)
            current = 16r2F ifTrue: [^16r2F]. (* \/ *)
            current = 16r5C ifTrue: [^16r5C]. (* \\ *)

            current = 16r62 ifTrue: [^16r08]. (* \b *)
            current = 16r66 ifTrue: [^16r0C]. (* \f *)
            current = 16r6E ifTrue: [^16r0A]. (* \n *)
            current = 16r72 ifTrue: [^16r0D]. (* \r *)
            current = 16r74 ifTrue: [^16r09]. (* \t *)

            (* \u is not supported, as upstream does not support it. *)
            expected: 'valid escape sequence. note, some are not supported'
        )
        readEscape = (
            read.
            captureBuffer:: captureBuffer, (String with: readEscapeCharacter).
            read
        )
        readNumber = (
            | firstDigit |
            startCapture.
            readCharacter: 16r2D. (* - *)
            firstDigit:: current.

            readDigit ifFalse: [expected: 'digit'].
            (firstDigit = 16r30) not ifTrue: [[readDigit] whileTrue: []]. (* 0 *)

            readFraction.
            readExponent.
            ^JsonNumber new: endCapture
        )
        readFraction = (
            (readCharacter: 16r2E) ifFalse: [^false]. (* . *)

            readDigit ifFalse: [expected: 'digit'].

            [readDigit] whileTrue: [].

            ^true
        )
        readExponent = (
            ((readCharacter: 16r65) not and: [ (* e *)
                (readCharacter: 16r45) not]) ifTrue: [^false]. (* E *)

            (readCharacter: 16r2B) ifFalse: [readCharacter: 16r2D]. (* + - *)

            readDigit ifFalse: [expected: 'digit'].

            [readDigit] whileTrue: [].

            ^true
        )
        readCharacter: character = (
            current = character ifFalse: [^false].
            read.
            ^true
        )
        readDigit = (
            isDigit ifFalse: [^false].
            read.
            ^true
        )
        skipWhiteSpace = (
            [isWhiteSpace] whileTrue: [read]
        )
        read = (
            current = 16r0A ifTrue: [ (* LF *)
                line:: line + 1.
                column:: 0].

            index:: index + 1.
            column:: column + 1.

            input isNil ifTrue: [error: 'input nil'].
            index <= input size
                ifTrue: [current:: input at: index]
                ifFalse: [current:: nil]
        )
        startCapture = (
            captureStart:: index
        )
        pauseCapture = (
            captureBuffer:: captureBuffer, (input copyFrom: captureStart to: index - 1).
            captureStart:: -1
        )
        endCapture = (
            | captured |
            '' = captureBuffer
                ifTrue: [captured:: input copyFrom: captureStart to: index - 1]
                ifFalse: [
                    pauseCapture.
                    captured:: captureBuffer.
                    captureBuffer:: ''].
            captureStart:: -1.

            ^captured
        )
        expected: expected = (
            isEndOfText ifTrue: [
                error: 'Unexpected end of input, expected ', expected].
            error: 'Expected ', expected
        )
        error: message = (
            Error signal: 'Json: ', message, ' at line ', line printString,
                ', column ', column printString
        )
        isWhiteSpace = (
            current = 16r20 ifTrue: [^true]. (* space *)
            current = 16r09 ifTrue: [^true]. (* tab *)
            current = 16r0A ifTrue: [^true]. (* LF *)
            current = 16r0D ifTrue: [^true]. (* CR *)
            ^false
        )
        isDigit = (
            current = 16r30 ifTrue: [^true]. (* 0 *)
            current = 16r31 ifTrue: [^true]. (* 1 *)
            current = 16r32 ifTrue: [^true]. (* 2 *)
            current = 16r33 ifTrue: [^true]. (* 3 *)
            current = 16r34 ifTrue: [^true]. (* 4 *)
            current = 16r35 ifTrue: [^true]. (* 5 *)
            current = 16r36 ifTrue: [^true]. (* 6 *)
            current = 16r37 ifTrue: [^true]. (* 7 *)
            current = 16r38 ifTrue: [^true]. (* 8 *)
            current = 16r39 ifTrue: [^true]. (* 9 *)
            ^false
        )
        isEndOfText = (
            ^current isNil
        )
    )
    class HashIndexTable = (
        | private hashTable = Array new: 32. |
        (* SOMns's `Array new: 32 withAll: 0`; Primordial Soup's `Array` has
           no `new:withAll:`. *)
        1 to: 32 do: [:slot | hashTable at: slot put: 0].
    ) (
        public at: name put: index = (
            | slot |
            slot:: hashSlotFor: name.

            index < 255
                ifTrue: [hashTable at: slot put: index + 1]
                ifFalse: [hashTable at: slot put: 0]
        )
        public at: name = (
            | slot |
            slot:: hashSlotFor: name.

            (* subtract 1, 0 stands for empty *)
            ^((hashTable at: slot) bitAnd: 255) - 1
        )
        hashSlotFor: element = (
            ^(element hash bitAnd: 31) + 1
        )
    )
    class JsonValue = () (
        public isObject = ( ^false )
        public isArray = ( ^false )
        public isNumber = ( ^false )
        public isString = ( ^false )
        public isBoolean = ( ^false )

        public isTrue = ( ^false )
        public isFalse = ( ^false )
        public isNull = ( ^false )

        public asObject = (
            Error signal: 'Unsupported operation, not an object: ', self printString
        )
        public asArray = (
            Error signal: 'Unsupported operation, not an array: ', self printString
        )
        public asString = (
            Error signal: 'Unsupported operation, not a string: ', self printString
        )
        public asBoolean = (
            Error signal: 'Unsupported operation, not a boolean: ', self printString
        )
    ) : (
        public readFrom: text = (
            ^(JsonParser with: text) parse
        )
    )
    class JsonArray = JsonValue (
        | public values = List new. |
    ) (
        public add: value = (
            value isNil ifTrue: [Error signal: 'value is null'].
            values add: value
        )
        public at: index put: value = (
            value isNil ifTrue: [Error signal: 'value is null'].
            values at: index put: value
        )
        public size = (
            ^values size
        )
        public isEmpty = (
            ^values isEmpty
        )
        public at: index = (
            ^values at: index
        )
        public isArray = (
            ^true
        )
        public asArray = (
            ^self
        )
        public hash = (
            ^values hash
        )
        public = other = (
            self == other ifTrue: [^true].
            other == nil ifTrue: [^false].
            self class == other class ifFalse: [^false].

            ^values = other values
        )
    ) : (
        public readFrom: text = (
            ^(JsonValue readFrom: text) asArray
        )
    )
    class JsonLiteral new: literalValue = JsonValue (
        | value = literalValue.
          public isNull = 'null' = literalValue.
          public isTrue = 'true' = literalValue.
          public isFalse = 'false' = literalValue. |
    ) (
        public asString = ( ^value )
        public hash = ( ^value hash )
        public isBoolean = ( ^isTrue or: [isFalse] )

        public asBoolean = (
            isNull ifTrue: [^super asBoolean] ifFalse: [^isTrue]
        )

        public = other = (
            self == other ifTrue: [^true].
            other == nil ifTrue: [^false].
            self class == other class ifFalse: [^false].

            ^value = other asString
        )
    )
    class JsonNumber new: text = JsonValue (
        | string = text. |
        text isNil ifTrue: [Error signal: 'string is null'].
    ) (
        public asString = ( ^string )
        public isNumber = ( ^true )

        public hash = (
            ^string hash
        )
        public = other = (
            self == other ifTrue: [^true].
            other == nil ifTrue: [^false].

            self class == other class ifFalse: [^false].
            ^string = other asString
        )
    )
    class JsonObject = JsonValue (
        | public names = List new.
          private values = List new.
          private table = HashIndexTable new. |
    ) (
        public add: name with: jsonValue = (
            name isNil ifTrue: [Error signal: 'name is null'].
            jsonValue isNil ifTrue: [Error signal: 'jsonValue is null'].

            names add: name.
            values add: jsonValue.
            table at: name put: names size.
        )
        public at: name put: jsonValue = (
            | position |
            name isNil ifTrue: [Error signal: 'name is null'].
            jsonValue isNil ifTrue: [Error signal: 'jsonValue is null'].

            position:: indexOf: name.
            (position = -1) not
                ifTrue: [values at: position put: jsonValue]
                ifFalse: [
                    names add: name.
                    values add: jsonValue.
                    table at: name put: names size].
        )
        public at: name = (
            | position |
            name isNil ifTrue: [Error signal: 'name is null'].
            position:: indexOf: name.
            position = -1
                ifTrue: [^nil]
                ifFalse: [^values at: position]
        )
        public at: name asBooleanWith: default = (
            | value |
            value:: at: name.
            value isNil ifTrue: [^default].
            ^value asBoolean
        )
        public at: name asStringWith: default = (
            | value |
            value:: at: name.
            value isNil ifTrue: [^default].
            ^value asString
        )
        public size = (
            ^names size
        )
        public isEmpty = (
            ^names isEmpty
        )

        public isObject = ( ^true )
        public asObject = ( ^self )

        public hash = (
            | result |
            result:: 1.
            result:: 31 * result + names hash.
            result:: 31 * result + values hash.
            ^result
        )
        indexOf: name = (
            | position |
            position:: table at: name.
            ((position = -1) not and: [name = (names at: position)]) ifTrue: [^position].
            ^lastIndexOf: name
        )
        (* SOMns's Vector>>lastIndexOf:; Primordial Soup's List has
           indexOf: but no lastIndexOf:. *)
        lastIndexOf: name = (
            names size to: 1 by: -1 do: [:position |
                (names at: position) = name ifTrue: [^position]].
            ^-1
        )
    ) : (
        public readFrom: text = (
            ^(JsonValue readFrom: text) asObject
        )
    )
    class JsonString new: text = JsonValue (
        | string = text. |
    ) (
        public isString = ( ^true )
        public asString = ( ^string )

        public hash = (
            ^string hash
        )
        public = other = (
            self == other ifTrue: [^true].
            other == nil ifTrue: [^false].

            self class == other class ifFalse: [^false].

            ^string = other asString
        )
    )
)
```

- [ ] **Step 2: Check the input literal against SOMns's.**

```bash
python - <<'EOF'
import re
ours = open("benchmarks/are-we-fast-yet/Json.ns", encoding="utf-8").read()
theirs = open("prior-art/somns/core-lib/Benchmarks/Json.ns", encoding="utf-8").read()

# SOMns's longest literal is the input; it must stand in ours unchanged.
literal = max(re.findall(r"'([^']*)'", theirs), key=len)
assert ("'" + literal + "'") in ours, "the input literal differs"
print(len(literal), "characters, identical")
EOF
```
Expected: `25820 characters, identical`.

- [ ] **Step 3: Register it.**

1. `AreWeFastYet.ns`, the `benchmarks` slot:
   ```newspeak
    | private benchmarks = {
        manifest Bounce. manifest CD. manifest DeltaBlue.
        manifest DeltaBlueTon80. manifest Havlak. manifest Json.
        manifest List. manifest Mandelbrot. manifest NBody.
        manifest Permute. manifest Queens. manifest Richards.
        manifest RichardsTon80. manifest Sieve. manifest Storage.
        manifest Towers }. |
   ```
2. `are-we-fast-yet.json`:
   ```json
{
  "root": "AreWeFastYet.ns",
  "modules": ["Bounce.ns", "CD.ns", "DeltaBlue.ns", "DeltaBlueTon80.ns", "Havlak.ns", "Json.ns", "List.ns", "Mandelbrot.ns", "NBody.ns", "Permute.ns", "Queens.ns", "Richards.ns", "RichardsTon80.ns", "Sieve.ns", "Storage.ns", "Towers.ns"]
}
   ```
3. Each of `goldens/awfy/a_named_benchmark_runs_alone.json`, `all_benchmarks_verify.json`, `an_unknown_benchmark_is_refused.json` and `extra_arguments_are_a_usage_error.json`:
   ```json
{
  "root": "../../benchmarks/are-we-fast-yet/AreWeFastYet.ns",
  "modules": [
    "../../benchmarks/are-we-fast-yet/Bounce.ns",
    "../../benchmarks/are-we-fast-yet/CD.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlue.ns",
    "../../benchmarks/are-we-fast-yet/DeltaBlueTon80.ns",
    "../../benchmarks/are-we-fast-yet/Havlak.ns",
    "../../benchmarks/are-we-fast-yet/Json.ns",
    "../../benchmarks/are-we-fast-yet/List.ns",
    "../../benchmarks/are-we-fast-yet/Mandelbrot.ns",
    "../../benchmarks/are-we-fast-yet/NBody.ns",
    "../../benchmarks/are-we-fast-yet/Permute.ns",
    "../../benchmarks/are-we-fast-yet/Queens.ns",
    "../../benchmarks/are-we-fast-yet/Richards.ns",
    "../../benchmarks/are-we-fast-yet/RichardsTon80.ns",
    "../../benchmarks/are-we-fast-yet/Sieve.ns",
    "../../benchmarks/are-we-fast-yet/Storage.ns",
    "../../benchmarks/are-we-fast-yet/Towers.ns"
  ]
}
   ```
4. `goldens/awfy/all_benchmarks_verify.toml`:
   ```toml
stdout = '''
Bounce: ok
CD: ok
DeltaBlue: ok
DeltaBlueTon80: ok
Havlak: ok
Json: ok
List: ok
Mandelbrot: ok
NBody: ok
Permute: ok
Queens: ok
Richards: ok
RichardsTon80: ok
Sieve: ok
Storage: ok
Towers: ok
'''
   ```
5. `crates/ns-cli/tests/run.rs`, `a_timed_run_prints_a_score_for_every_benchmark`'s expected names:
   ```rust
        [
            "Bounce",
            "CD",
            "DeltaBlue",
            "DeltaBlueTon80",
            "Havlak",
            "Json",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Richards",
            "RichardsTon80",
            "Sieve",
            "Storage",
            "Towers"
        ]
   ```

- [ ] **Step 4: Run it through the harness, and the whole harness once, in a release build.**

```bash
cargo build --release -p ns-cli
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Json
./target/release/ns-next run --lowering reference benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Json
./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json
./target/release/ns-next build benchmarks/are-we-fast-yet/are-we-fast-yet.json target/awfy.nsimg
./target/release/ns-next load target/awfy.nsimg -- Json
```
Expected:
- `Json: ok` (about 15 ms) each time it runs alone;
- the full run prints the sixteen `<Name>: ok` lines of `all_benchmarks_verify.toml` in that order;
- the image built from the project runs Json the same way.

- [ ] **Step 5: Run the per-commit gate.** Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add benchmarks/are-we-fast-yet goldens/awfy crates/ns-cli/tests/run.rs
git commit -m "feat: Json joins the Are We Fast Yet harness"
```

---

### Task 11: the public API of the shipped modules is documented

Every `public` member of `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns` gets a comment that a programmer using it needs (decision 12). Today there are about 200 in `Kernel.ns`, 17 in `PlatformFactory.ns` and 28 in `CollectionsFactory.ns`. Count them with `rg -c '^\s*public ' crates/ns-runtime/core-library/*.ns`. Nothing but comments changes, so no golden and no test may change.

**The rule** (it goes into `CLAUDE.md` in Step 3, in these words):

> **A public member of a shipped core-library module is documented.** Every `public` method, slot and class of `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns` has a `(* … *)` comment directly above it, or shares one with the family it belongs to (`+ - *`, the comparisons, the class accessors on `Object`). It describes what the member does as it stands: what it answers, what it takes, and which Error it signals and when -- one to four lines, no more. It names no other implementation. A comment that explains a decision, as the rule above asks, goes after it.

**How to write them:**
- **Describe the current behaviour, nothing else.** No reference implementation, no comparison with one, no history.
- **Keep the reasons apart.** Most members already sit under a comment that explains a decision, and those comments may name where a decision comes from. Put the description first, as its own sentence or as its own comment above, and never delete a reason that is still true.
- **Families share one comment** when they share one contract:
  - `Integer`'s and `LargeInteger`'s arithmetic, comparisons and bit operations;
  - `Float`'s arithmetic and comparisons;
  - `Object`'s class accessors;
  - the nil protocol;
  - `True`'s and `False`'s conditionals.

  A member whose behaviour differs from its family's (`//`, `\\`, `<<`, `bitInvert`) gets its own sentence.
- **Error texts are quoted as the leaf writes them,** with the variable part named: "signals an Error, 'index … out of range: valid indices are 1..size (size …)', for an index outside 1 to `size`". Find each text in the leaf (`builtins/*.rs`); do not paraphrase it.
- **No mechanics.** "Forwards to `vmMirror`" says nothing a user needs.

**Examples, in the house style:**
```newspeak
        (* The element at `index`, 1-based. Signals an Error, 'index … out of
           range: valid indices are 1..size (size …)', for an index outside 1
           to `size`, and 'at: requires an Integer index, got …' for one that
           is no Integer. *)
        public at: index = ( ^vmMirror collection: self at: index ifFail: [^super at: index] )
```
```newspeak
        (* Removes the first element and answers it, in constant time.
           Signals an Error, 'removeFirst: the List is empty', for an empty
           List. *)
```
```newspeak
    (* Arithmetic and ordering: an Integer or a LargeInteger argument
       answers an integer, a LargeInteger only when it does not fit 64 bits;
       a Float argument converts the receiver and answers a Float, or a
       Boolean for the ordering. Any other argument signals an Error naming
       what the operator takes. *)
```
```newspeak
        (* The code point at `index`, 1-based, as an Integer. Signals an
           Error, as `Array>>at:` does, for an index outside 1 to `size` or
           one that is no Integer. *)
```
```newspeak
        (* `platform transcript`: the program's standard output, written as
           UTF-8. Built on first use. *)
        lazy public transcript = Transcript new.
```

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns`, `PlatformFactory.ns`, `CollectionsFactory.ns` (comments only), `CLAUDE.md`

- [ ] **Step 1: Document `Kernel.ns`,** class by class, in source order. Then check the coverage: every line matching `^\s*public ` has a comment ending on the line above it, or belongs to a family whose shared comment ends above its first member. List the members you treated as a family in the commit message's body.

- [ ] **Step 2: Document `PlatformFactory.ns` and `CollectionsFactory.ns`** the same way. `CollectionsFactory.ns`'s comments from Tasks 4 and 5 mostly give reasons: add the description of each member before them.

- [ ] **Step 3: Add the rule to `CLAUDE.md`'s "Conventions"**, after "Comments explain the decision …", in the words quoted above.

- [ ] **Step 4: Run the per-commit gate.**

Expected: PASS, with no golden and no test changed. `git diff --stat` shows only the three `.ns` files and `CLAUDE.md`.

- [ ] **Step 5: Commit.**

```bash
git add crates/ns-runtime/core-library CLAUDE.md
git commit -m "docs: every public member of the shipped modules says what it does"
```

---

### Task 12: the documentation describes the built state

**Files:**
- Modify:
  - `docs/design/core-library.md`, `object-model.md`, `memory.md`, `image-format.md`, `execution.md`, `README.md`;
  - `docs/OPEN-WORK.md`, `docs/PROGRESS.md`;
  - `THIRD-PARTY-NOTICES.md`, `prior-art/PROVENANCE.md`, `CLAUDE.md`.

Every statement below that names a count was checked against the tree when this plan was written. Count again; it stays true only if no other change landed.

- [ ] **Step 1: `core-library.md`.**
  - **Status block** (top): "with a Newspeak `List`" becomes "with Newspeak `List`, `Map` and `Set`".
  - **"What exists", the forwarder list:**
    - `Float`'s gains `sin`, `cos` and `asInteger`;
    - `String`'s gains `at:` and `copyFrom:to:`, and `String`'s class side `with:`;
    - add that `Integer>>timesRepeat:` and `Number>>max:` are ordinary Newspeak, Primordial Soup's bodies, and reach no leaf.
  - **"Planned: strings of code points in two widths".** Rewrite it as current, under the heading "Strings are code points in two widths":
    - a `String`'s payload is `CodePoints`, one byte per code point while every one fits Latin-1 and 32 bits otherwise, so `size` and `at:` are one access;
    - it is canonical, so `=` compares the payloads and `hash` agrees;
    - every element is a scalar value;
    - UTF-8 stays the encoding at every boundary (source, the transcript, the image);
    - `hash` is the hash of the UTF-8 text, the value it had before;
    - built 2026-10-01 for Json, ahead of road step 3, on today's arena. `crates/ns-runtime/src/code_points.rs`.

    Keep as Planned, with road step 3, only: `Symbol` becomes a subclass of `String` sharing both formats, `=` stays class-tight both ways (`'foo' = #foo` and `#foo = 'foo'` answer `false`), and a golden pins it.
  - **String's three leaves**, in that section or beside the `Array new:` bullet of "The collections module (plan 2)":
    - `at:` answers a code point where Primordial Soup's answers a UTF-8 byte;
    - `copyFrom:to:` is Primordial Soup's, 1-based and inclusive, with the empty range that stops just before it starts;
    - `String with:` takes any code point where Primordial Soup's takes a byte;
    - the error texts: `at:` shares `Array>>at:`'s;
    - `goldens/kernel/a_string_answers_its_code_points`, `goldens/exceptions/a_string_index_error_is_caught`.
  - **"The leaves are the 105 rows"** becomes 111. Count the `mirror(` rows in `MIRROR`.
  - **"The on-demand rule".** The worked example's paragraph becomes:
    - the `collections` namespace hands out `List`, `Map` and `Set`;
    - `List` because `Ducts.ns`, `NewspeakASTs.ns` and `V5BytecodeSimulation.ns` bind it, and the Are We Fast Yet benchmarks of stage 3 widen it;
    - `Map` and `Set` because DeltaBlue and Havlak need them;
    - each carries only the selectors those programs send.

    The corpus still asks for `IdentityMap`, `IdentitySet`, `WeakArray` and `Collection`, which the runtime does not have. Keep the sentence on aliasing.
  - **"The collections module (plan 2)", the `List` bullet.**
    - Its growth: an Array of three to start, and when full either a move to the front of the same array or an Array half as large again (Primordial Soup's growth).
    - Its first index, and why: `removeFirst` moves nothing, where Primordial Soup's shifts every element; DeltaBlue at 12000 took 52 s that way against 0.9 s.
    - Its primary factory `new: capacity`, with a class-side `new`.
    - Its protocol: the six selectors plus `isEmpty`, `removeFirst`, `removeLast`, `remove:ifAbsent:`, `removeAll`, `anySatisfy:` and `sort:` (stable, lessOrEqual), with the benchmark that sends each.
  - **A new bullet, `Map` and `Set`, built:** adapted from Primordial Soup's `Collections.ns` (Apache 2.0), as Task 5 describes them. Name:
    - the lazy table and `docs/OPEN-WORK.md`'s "Whether an image carries a lazy slot's value";
    - `goldens/collections/a_map_and_a_set_keep_insertion_order`.
  - **A new section, "A NaN equals a NaN"**, beside "Deviation: a non-integral literal is a binary float". It says:
    - `Float>>=` is IEEE's except that every NaN equals every other, and every NaN hashes alike, so that a Float's equality is an equivalence, as §3.1.1 asks of a value, whose `==` answers what `=` does;
    - this is JavaScript's SameValueZero, the equality of its `Map` and `Set`, and Julia's `isequal`; Java's `Double.equals` makes NaN equal too but tells `0.0` from `-0.0`;
    - the ordering and the arithmetic stay IEEE's, so for a NaN `a = b` is true while `a <= b` is false;
    - it differs from Primordial Soup and Squeak, where a NaN equals nothing; Primordial Soup's `isNaN` is `^(self = self) not`, so ns-next's, when a program asks for it, is a leaf;
    - decided with the user on 2026-10-01; `goldens/arithmetic/not_a_number_equals_itself`.
  - **The collections bullets** say that the representation of `List`, `Map` and `Set` is private, where Primordial Soup's is protected, and why (decision 10). `List`'s `size_` stays protected.
  - **"Decisions".** The strings bullet becomes built, on today's arena, with the stage-3 date. Add one bullet for this plan's decisions 3 and 4 (List used canonically and made fast; Map and Set adapted from Primordial Soup, `transient` as `lazy`), dated 2026-10-01.
- [ ] **Step 2: `object-model.md`.**
  - **The `ObjectData` sketch** (near the top): `String(String)` becomes `String(CodePoints)`. Add a sentence after the `LargeInteger` paragraph: a String's payload holds its code points in one of two widths and no handle ([core-library.md](core-library.md), "Strings are code points in two widths").
  - **"Planned: the object layout".**
    - Its opening paragraph: strings in two widths are built already, on the arena. What road step 3 changes for them is that they become byte- and word-format objects in the space.
    - "**Strings have two widths.**": move the sentence about what is built into the present tense, keeping the reasons (Spur, JEP 254, V8, PEP 393, Primordial Soup's bytes, grapheme clusters).
    - Keep as planned: `Symbol` under `String`, `Mutator::new_symbol` as the only way to make one, `String new` answering an empty one-byte string, and the class-tight `=`.
  - **"Decisions".** "Strings will store code points in one of two widths … (planned …)" becomes "Strings store code points in one of two widths … (decided with the user on 2026-09-29, built on today's arena on 2026-10-01)".
- [ ] **Step 3: `memory.md`.**
  - **"Copy-out costs".** The string dispatch no longer clones. Its accessors (`string_size`, `string_code_point_at`, `string_slice`, `string_concatenation`, `strings_equal`, `string_hash`) copy out a size, a code point, a slice, a bool or a hash. `str_value` stays as the UTF-8 boundary. The measured effect: 25,000 `size` sends over a 25,000-character string went from 22.3 ms to 2.4 ms, and `size` with `at:` from 40 ms to 3 ms (release, two pinned builds per side, 2026-10-01). `object_snapshot` still clones, and `encode` converts each String to UTF-8 once.
  - **The decode amplification.** Name the String term:
    - a decoded String costs at most 4 heap bytes per wire byte (one wide code point makes every element four bytes);
    - it is allocated exactly, from the buffer in place (`Reader::read_code_points`);
    - a byte cannot be both a String's and a `LargeInteger`'s, so the term stays below `LargeInteger`'s 4.6x, and the ~16.4x and ~21.0x peaks stand.
- [ ] **Step 4: `image-format.md`.**
  - **"Canonical encodings".** Add: the decoder picks a String's width from the code points it reads.
  - **"Planned: the image under the new representation".** The bullet "**Strings and symbols** stay UTF-8 on the wire; the decoder picks the one-byte or the four-byte form…" is built for strings. Reduce it to symbols and to the object format of road step 3.
- [ ] **Step 5: `execution.md`, "The Are We Fast Yet benchmarks".**
  - **The benchmarks:**
    - "ten of the Are We Fast Yet benchmarks -- Bounce, …, Towers --" becomes all fourteen (Bounce, CD, DeltaBlue, Havlak, Json, List, Mandelbrot, NBody, Permute, Queens, Richards, Sieve, Storage, Towers);
    - Primordial Soup's own Richards and DeltaBlue, `RichardsTon80` and `DeltaBlueTon80`, sit beside them;
    - `DeltaBlueTon80.ns` comes from Primordial Soup's `DeltaBlue.ns`, Apache 2.0.
  - **CD's tree:** CD's `RedBlackTree.ns` is folded into `CD.ns`, so every benchmark is one module the harness reaches as `manifest <Name>`.
  - **The licences:** Havlak's header carries Google's Apache 2.0 text, and CD's carries Purdue's and Apple's BSD-3 text, as SOMns's files do.
  - **The sizes paragraph:**
    - Mandelbrot runs at 750, which answers 50: SOMns's verified size (`setupVerifiedRun:`). Are We Fast Yet's own `rebench.conf` runs 500. The user kept 750 on 2026-10-01.
    - NBody runs at Are We Fast Yet's 250000 steps.
    - Add: DeltaBlue 12000, Havlak 1500 (6102 loops, 5213 nodes) and CD 250 (10830 collisions) are `rebench.conf`'s sizes. Json parses once, its 100 there being a repetition count. Release times of one run: about 0.9 s, 5 s, 5 s and 15 ms.
  - **The cost of the benchmarks profile:** it grows by Havlak and CD in every variant. Leave the "37 minutes under the stress modes" figure as it is: those tests do not run under a stress mode.
  - **The run count:** "all eleven in turn" becomes "all sixteen in turn".
- [ ] **Step 6: `README.md`.**
  - **The paragraph under the table:** "ten of the Are We Fast Yet benchmarks, Primordial Soup's Ton80 Richards" becomes "the fourteen Are We Fast Yet benchmarks, Primordial Soup's Ton80 Richards and DeltaBlue".
  - **The goldens table:**
    - `arithmetic/` gains `sin`, `cos` and `asInteger`;
    - `collections/` becomes "the collections module behind `platform collections`: `List`'s protocol, its queue, its growth, its index errors, a user subclass, `Map` and `Set`, and one module shared by every platform";
    - `kernel/` gains "a String's code points in either width, `timesRepeat:` and `max:`";
    - `exceptions/` gains "a String's index errors".
  - **The core-library row:** "its `List` is Newspeak" becomes "its `List`, `Map` and `Set` are Newspeak".
- [ ] **Step 7: `OPEN-WORK.md`.**
  - **"Next"**: the baseline over all sixteen programs, measured and recorded in `execution.md` as the measurement protocol asks, before road step 2's second plan.
  - **"The road to a high-performance VM", the Are We Fast Yet item:**
    - stage 3 is built: DeltaBlue, Havlak, CD and Json, and Primordial Soup's DeltaBlue as `DeltaBlueTon80`, in Primordial Soup's protocol (decided with the user on 2026-10-01); remove the numbered line 3;
    - the baseline sentence becomes "over all sixteen -- the fourteen Are We Fast Yet benchmarks and Primordial Soup's Richards and DeltaBlue";
    - "one informal pinned build of stage 1 scored …" stays.
  - **Road step 3:** drop "strings in two widths;" and write instead that strings become byte- and word-format objects in the space (built on the arena, `core-library.md`).
  - **"Known defects and gaps"**, add: **`ns-next run` panics when its stdout pipe closes early** (`… | head`): the transcript's `print!` reports "failed printing to stdout" (os error 232 on Windows) instead of ending quietly. Found 2026-10-01 while checking the Json port.
  - **Keep** "Whether an image carries a lazy slot's value", recorded with the user on 2026-10-01. `Map` and `Set` now exist.
- [ ] **Step 8: `PROGRESS.md`.**
  - **"Where the next session starts"** names the baseline over all sixteen, and keeps the eight-byte value after it.
  - **The stage 2 section, decision 3:** "run at Are We Fast Yet's pinned sizes" becomes "Mandelbrot at SOMns's verified 750, which answers 50 (Are We Fast Yet's `rebench.conf` runs 500), and NBody at Are We Fast Yet's 250000 steps". That statement was never true, so it is rewritten (`CLAUDE.md`, "Conventions").
  - **A new milestone section**, "Are We Fast Yet, stage 3 (2026-10-01)", in the shape of stage 2's:
    - the plan's link;
    - the commits, one line each;
    - the twelve decisions of this plan's header.
- [ ] **Step 9: `THIRD-PARTY-NOTICES.md` and `prior-art/PROVENANCE.md`.**
  - **The notices' introduction.** The Newspeak bullet becomes: the benchmarks from SOMns, licensed MIT, except that `Mandelbrot.ns`, `Havlak.ns` and `CD.ns` keep their upstream files' own licence texts in their headers (BSD-3, Apache 2.0, BSD-3); the harness, `RichardsTon80.ns` and `DeltaBlueTon80.ns` from Primordial Soup, Apache 2.0. Add a bullet: Newspeak code in `crates/ns-runtime/core-library/CollectionsFactory.ns`, its `List`, `Map` and `Set`, adapted from Primordial Soup's `newspeak/Collections.ns` and `KernelForPrimordialSoup.ns`, Apache 2.0. "four third-party projects" stays four.
  - **`PROVENANCE.md`, the primordialsoup entry:** add `newspeak/DeltaBlue.ns` (to `DeltaBlueTon80.ns`) and `newspeak/Collections.ns` (to `CollectionsFactory.ns`).
  - **`PROVENANCE.md`, the somns entry:** it already covers the benchmarks.
- [ ] **Step 10: `CLAUDE.md`.** "`benchmarks/are-we-fast-yet/` — ten Are We Fast Yet benchmarks, Primordial Soup's Ton80 Richards and their harness" becomes "— the fourteen Are We Fast Yet benchmarks, Primordial Soup's Ton80 Richards and DeltaBlue, and their harness".
- [ ] **Step 11: Check the diff is Markdown only** (`git diff --stat`), then commit. A Markdown-only commit runs no gates.

```bash
git commit -am "docs: Are We Fast Yet stage 3, strings in two widths, List, Map and Set"
```

---

### Task 13: the end-of-batch gates

- [ ] **Step 1: Run the `finishing-a-batch` skill** and every gate it names:
  - **the three stress modes;**
  - **`run-corpus`** plain and under stress;
  - **the benchmarks profile:** `cargo nextest run -p ns-cli --profile benchmarks`, plain and never under a stress mode. It now runs Havlak and CD at full size in five variants in a debug build. Give it `timeout: 600000`; if it does not finish, run its six tests one at a time with `-E 'test(=<name>)'`;
  - **`cargo nextest run -p ns-lsp`;**
  - **`cargo nextest run --workspace --release`** for the size tests;
  - **its check** that a fresh session can start from the documentation alone.

  `verifyPlugin` does not apply: nothing under `editors/jetbrains`, `ns-lsp` or `ns-syntax` changed. Confirm with `git diff --stat master`.
- [ ] **Step 2: Fix what a gate finds** in a commit of its own, naming the gate, and rerun that gate.
