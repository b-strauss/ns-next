# Frames as Words in One Reservation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 7a: every running activation becomes a frame of tagged
words -- a fixed header, then its registers -- in one `ns-memory`
reservation per heap, replacing `ActivationStack`'s `values` and
`entries`, with no change to any program's outcome and no measurable
slowdown in release.

**Architecture:** Staged so that each stage removes one non-word field
from `StackEntry` while the two vectors still exist -- the code as a
`CodeId` (T1), the kind as a copyable view (T2), the continuation as a
side-table row (T3) -- then a self-contained frame stack over a
`Reservation` (T4) replaces both vectors (T5), with the byte bound as a
net (T6). The measurement after T1 is a checkpoint on the one hot-path
risk; the measurement at the end (T8) decides whether the step is taken.
Everything stays behind `heap.rs`'s accessors, the seam.

**Tech Stack:** Rust 1.98.0, `ns-memory`'s `Reservation`, cargo-nextest,
`cargo xtask gates`.

**Spec:** `docs/design/execution.md`, "Planned: frames as words in one
reservation" (decided with the user on 2026-10-24), read with "Activations"
(all of it, "Suspending and resuming" included), "The depth limit" and
`docs/design/memory.md`, "Frames, the activation stack and the marriage",
"What the collector traces", "The memory layer".

## Global Constraints

- `CLAUDE.md`'s invariants hold throughout; the ones this plan touches:
  handles are space-relative and never outlive their interpreter; every
  store into an existing object goes through `heap.rs`'s store accessors;
  a new Rust-side field holding a handle or a stored `Word` needs its
  `Trace` impl; `vm.rs` holds no dispatch logic; never `{:?}` a `Value`
  or `.unwrap()` an `EvaluationResult`.
- `ns-runtime` keeps `#![forbid(unsafe_code)]`; `ns-memory` is the only
  crate with `unsafe`, and this plan adds none to it unless a task says so.
- No new external dependency.
- Every golden, `run-corpus` and benchmark outcome stays exactly as
  recorded. A changed outcome is a defect of the change, never a reason
  to update a golden.
- The depth limit stays `MAX_ACTIVATION_DEPTH` (100,000) plus
  `DEPTH_RESERVE` (1,000), counted as today; its error text stays
  `stack overflow: recursion too deep`.
- The reservation: 16 GiB on a 64-bit target, 256 MiB otherwise,
  committed page by page, never decommitted while the heap lives.
- Names are written out (`CLAUDE.md`, "Conventions"), in code and tests.
- Object, frame and activation access goes through `heap.rs` and its
  submodules; the new frame stack lives in `crates/ns-runtime/src/heap/frames.rs`.
- Gates: every commit runs `cargo xtask gates commit`; a commit touching a
  type a size test pins (`RegisterBase`, `ActivationReference`) also runs
  `cargo xtask gates release-checks`. `end-of-batch` (`images`, `caches`,
  `benchmarks`, `release-checks`, `stress`) runs once, in T8: every
  commit before it is work in progress until then. Every command runs in
  the foreground.
- Never judge performance from a debug build; measurements follow the
  `measuring-performance` skill.

## Review Focus

- **Words above the top are never read.** A popped frame leaves stale
  words -- old offsets, old object words -- between the top and the
  committed end. The linear trace, a scavenge's root pass and a
  compaction's forwarding must stop exactly at the top; a stale word
  traced is a dangling object kept or a forwarded garbage word. T5 pins
  it with a collection right after a deep pop.
- **A body whose code row nothing else names, running across a full
  collection.** Today the entry's `Rc` keeps the body; with a `CodeId`
  the reclaim must count every running and suspended frame's code as
  named, or `free_unnamed_codes` frees the row under a running frame and
  its return reads another body. T1 pins it.
- **A frame of 65,535 registers at the committed end.** The push's growth
  path must commit across several pages at once, and the byte bound must
  refuse a frame the reservation cannot hold before writing any word. T4
  and T6 pin it.
- **A stale `ActivationReference` after a pop and a push at the same
  offset.** Offsets are reused as indexes are today; the debug generation
  must still catch a read through the old handle. T5 keeps the existing
  `reading_through_a_popped_activation_handle_panics` passing against
  offsets.
- **Suspend from a floor, resume, and push again.** The floor and every
  comparison of two activations become offset comparisons; offsets grow
  with depth, as indexes do, and a resume rebuilds a frame at the top
  with a fresh offset. T5 runs the suspension tests unchanged and the
  suspend stress mode in T8.

---

### Task 1: The code as a `CodeId` in the entry

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`StackEntry::code`,
  `push_activation`, `activation_code`, `reify`, `suspend_activations`,
  `resume_activation`, `stack_roots`)
- Modify: `crates/ns-runtime/src/heap/reclaim.rs` (`StackRoots` gains the
  codes; the code reclaim keeps them)
- Modify: whatever caller of `free_unnamed_codes` builds the `BitSet` of
  named codes (`crates/ns-runtime/src/interpreter/reclaim.rs`)
- Test: `crates/ns-runtime/src/heap.rs` unit tests

**Interfaces:**
- Produces: `StackEntry::code: CodeId`; `ActivationHeader::code` stays
  `Rc<CompiledCode>`, registered on push through `register_code`'s
  one-entry cache (`CompiledCode::code_id_cache`); `Heap::activation_code(activation) -> Rc<CompiledCode>`
  keeps its signature and reads the code table. `StackRoots::codes: Vec<CodeId>`,
  running entries' and suspended rows' codes both.

- [ ] **Step 1: Write the failing test**
  `a_full_collection_keeps_the_code_row_of_a_running_activation`: push an
  activation of a body no object names (a fresh `CompiledCode`, as
  `trivial_code()` builds), run the full collection with its id reclaim,
  then assert `heap.activation_code(activation)` is `Rc::ptr_eq` to the
  pushed body and `heap.code(id)` still answers it. Before the change it
  passes trivially for `activation_code`, so assert on `heap.code(id)`
  with the id the push registered (`heap.code_id(&body)` before the
  collection): that row must survive.
- [ ] **Step 2: Run it to verify it fails**
  Run: `cargo nextest run -p ns-runtime a_full_collection_keeps_the_code_row_of_a_running_activation`
  Expected: FAIL -- the row is freed.
- [ ] **Step 3: Implement.** The entry holds the `CodeId`; the reclaim's
  named set includes `StackRoots::codes`. A suspended row's frame already
  names its code through `frame_field::CODE_ID`; check the reclaim reads
  it, and add it to `codes` if it does not.
- [ ] **Step 4: Run the suite**
  Run: `cargo xtask gates commit`
  Expected: passes, clippy warning-free.
- [ ] **Step 5: Commit** `perf: a stack entry names its code by id`.
- [ ] **Step 6: Checkpoint measurement.** In release, by the
  `measuring-performance` skill and execution.md's "What suspending and
  resuming moved" protocol cut down to one sitting: `master` against this
  commit, three builds a side, `bench_send.ns`, `bench_loop.ns`,
  `bench_block.ns`, `bench_megamorphic.ns` at `--iterations 30`, and
  Richards, DeltaBlue and Havlak. Record the table in the task report,
  not in the tree. If any program is slower with its builds not
  overlapping `master`'s, **stop and report to the user** before T2: the
  `CodeId` lookup is the change the step rests on.

### Task 2: The kind as a copyable view

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (new `ActivationKind<'gc>`)
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationHeader::kind`,
  `StackEntry::kind`, `activation_kind`, `reify`'s kind and code id,
  `kind_code`, the suspension's frame kind)
- Modify: the consumers of `activation_kind` and the push sites:
  `interpreter/mod.rs` (1151, 1176, 1276, 2347), `interpreter/unwind.rs`
  (545), `interpreter/locals.rs` (244), `interpreter/literals.rs` (36),
  `interpreter/instantiate.rs` (1840), and every `ActivationHeader { kind: … }`
- Modify: `crates/ns-runtime/src/trace.rs` (the new view's `Trace`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy)]
  pub enum ActivationKind<'gc> {
      Method(Symbol),                     // the selector
      Closure(ObjectReference<'gc>),
      TopLevel,
      Initializer(ClassReference<'gc>, InitializerBody),
      SuperclassClause(ClassReference<'gc>),
      Factory(ClassReference<'gc>),
  }
  ```
  `Heap::activation_kind(activation) -> ActivationKind<'gc>`.
  `FrameKind` stays the frame object's and the image's kind; `reify` and
  the suspension build it from the view and the code row's method
  (`Heap::code_method(id)` or the existing reader of `CodeEntry::method`).
  `InitializerBody` derives `Copy` if it does not.

- [ ] **Step 1:** Change the types and every consumer; no behaviour
  changes, so the suite is the test. Every consumer of `FrameKind::Method(method)`
  on an activation reads `method.selector` today; it reads the
  `ActivationKind::Method(selector)` instead.
- [ ] **Step 2: Run the suite** -- `cargo xtask gates commit`, passes.
- [ ] **Step 3: Commit** `refactor: an activation's kind is a copyable view`.

### Task 3: Continuations other than `Deliver` in a side table

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`StackEntry::then` becomes
  `continuation: Option<ContinuationRow>`; a `Continuations` table beside
  `Suspensions`; `push_activation`, `pop_activation`,
  `activation_set_continuation`, `activation_own_continuation`,
  `activation_signals_a_vm_error`, `activation_signalled_vm_error`,
  `suspend_activations`, `resume_activation`, the `Trace` of the heap's roots)
- Test: `crates/ns-runtime/src/heap.rs` unit tests

**Interfaces:**
- Produces: `struct Continuations<'gc> { rows: Vec<Option<Continuation<'gc>>>, free: ids::FreeIndices }`,
  rows minted smallest free first, as `Suspensions`; traced as a root.
  `Deliver` is never stored (the entry's `None`). `Popped::then` keeps
  its type: the pop takes the row out and frees it. A suspension takes
  its activation's continuation out of the table into its row as today;
  a resume puts it back.

- [ ] **Step 1: Write the failing test**
  `a_popped_continuation_frees_its_row`: push three activations whose
  continuations are not `Deliver` (a `Continuation::Unwind` with a nil
  target is the cheapest), pop them, push one more, and assert the table
  holds one row, its first (`continuation_rows()`, a `#[cfg(test)]` reader).
- [ ] **Step 2: Run it to verify it fails** -- does not compile, then fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the suite** -- `cargo xtask gates commit`, passes.
- [ ] **Step 5: Commit** `refactor: a continuation other than Deliver lives in a side table`.

### Task 4: The frame stack over a reservation

**Files:**
- Create: `crates/ns-runtime/src/heap/frames.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (`mod frames;`)
- Test: unit tests in `frames.rs`

**Interfaces:**
- Produces, all `pub(super)`:
  ```rust
  pub(super) mod frame_header {       // word offsets from a frame's start
      pub const RECEIVER: usize;      pub const CURRENT_CLASS: usize;
      pub const OPERAND: usize;       pub const REIFIED: usize;
      pub const FLAGS: usize;         pub const CODE: usize;
      pub const PROGRAM_COUNTER: usize; pub const FEEDBACK_BASE: usize;
      pub const CALLER: usize;        pub const CONTINUATION: usize;
      pub const GUARD: usize;         pub const ENTERED_LEVEL: usize;
      #[cfg(debug_assertions)] pub const GENERATION: usize;
      pub const COUNT: usize;         // the registers start here
  }
  pub(super) struct FrameFlags { kind_tag: u8, argument_count: u16, counted: bool, retains_registers: bool }
  impl FrameFlags { pub fn word(self) -> Word; pub fn of(word: Word) -> FrameFlags; }
  pub(super) struct FrameStack { /* a Reservation, top, committed end, limit */ }
  pub(super) struct StackExhausted;
  impl FrameStack {
      pub fn new(reservation_bytes: usize) -> Result<FrameStack, MemoryError>;
      pub fn top(&self) -> usize;                       // in words
      pub fn push(&mut self, words: usize) -> Result<usize, StackExhausted>; // the new frame's word offset, words nil-filled
      pub fn truncate(&mut self, offset: usize);
      pub fn words(&self) -> &[u64];                    // [0, top)
      pub fn words_mut(&mut self) -> &mut [u64];        // [0, top)
  }
  ```
  The reservation constants: `STACK_RESERVATION`, 16 GiB on 64-bit and
  256 MiB otherwise, as `heap/space.rs`'s `DEFAULT_RESERVATION` is chosen.
  Every header word is a valid tagged `Word`: numbers as `SmallInteger`s,
  absent handles as nil. `push` checks `top + words` against the
  committed end in one comparison and commits on a `#[cold]` path,
  rounding up to whole pages, refusing with `StackExhausted` past the
  reservation before writing a word.

- [ ] **Step 1: Write the failing tests**
  - `flags_round_trip_through_a_small_integer_word`: every kind tag,
    argument counts 0, 1 and 65,535, both booleans; assert `FrameFlags::of_word(flags.word()) == flags`
    and `flags.word().is_small_integer()`.
  - `a_push_past_the_committed_end_commits_and_answers_nil_words`: a
    `FrameStack::new(1 << 20)`, push one frame of 65,535 + `COUNT` words
    (more than a page), assert every word of it is nil and `top()` moved
    by exactly that much.
  - `a_push_the_reservation_cannot_hold_is_refused_and_leaves_the_top`:
    a `FrameStack::new(64 << 10)`, push until `StackExhausted`, assert the
    top is unchanged by the refused push.
  - `truncate_then_push_reuses_the_offset`: push, truncate to the frame's
    offset, push again; same offset, words nil again.
- [ ] **Step 2: Run them to verify they fail** --
  `cargo nextest run -p ns-runtime heap::frames`, does not compile.
- [ ] **Step 3: Implement** `frames.rs`. Not yet used by `Heap`; mark the
  module `#[cfg_attr(not(test), allow(dead_code))]` until T5 removes it.
- [ ] **Step 4: Run the suite** -- `cargo xtask gates commit`, passes.
- [ ] **Step 5: Commit** `feat: a stack of frames as tagged words in one reservation`.

### Task 5: The activation stack is the frame stack

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationStack` holds a
  `FrameStack` and the live count; `StackEntry` is deleted; every
  `activation_*` accessor, `push_activation`, `pop_activation`, `reify`,
  `suspend_activations`, `resume_activation`, `activation_above`,
  `activation_position`, `top_activation`, `activation_depth`,
  `frame_activation`, `check_activation`, `RegisterBase`, the stack's `Trace`)
- Modify: `crates/ns-runtime/src/heap/reclaim.rs` (`stack_roots` walks frames)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::push`
  maps `StackExhausted` to the depth limit's error; `Heap::push_activation`
  answers `Result<ActivationReference, StackExhausted>`)
- Modify: the scavenger's and compaction's root passes over the stack
  (`heap/scavenge.rs`, `heap/compaction.rs`), which now take one linear range
- Test: `crates/ns-runtime/src/heap.rs` unit tests

**Interfaces:**
- Consumes: T4's `FrameStack`, `frame_header`, `FrameFlags`; T1's code
  word; T2's `ActivationKind` encoded as the flags' kind tag plus the
  operand word; T3's continuation row in `frame_header::CONTINUATION`.
- Produces: `ActivationReference::index` is the frame's word offset
  (`u32`: 16 GiB is 2^31 words); `RegisterBase::index` the offset of its
  first register; `frame_field::MARRIED` of a married frame that offset.
  The activation above a frame is at `offset + frame_header::COUNT + register_count`,
  the register count read from the frame's code (`CompiledCode::register_count`)
  or from the window recorded at push -- whichever the push wrote; record
  the window length in the header if a body's `register_count` can differ
  from `arguments + temporaries` (check `Mutator::push_step`). Offsets grow
  with depth, so every comparison and every `floor` stays an ordering.
  The live-activation count, which `activation_depth` answers, is a field
  of `ActivationStack`.

- [ ] **Step 1: Write the failing tests**
  - `a_collection_after_a_deep_pop_reads_nothing_above_the_top`: push 100
    activations each holding a fresh young object in a register, pop 99,
    run a scavenge and a full collection, assert the objects of the
    popped frames are reclaimed (a weak array holding them answers nil)
    and the bottom frame's object survives.
  - `the_stack_is_traced_as_one_range_of_tagged_words`: after pushes of
    every kind, assert every word of `stack.frames.words()` decodes as a
    valid `Word` (a small integer, nil, a boolean or an object offset
    below the space's top).
- [ ] **Step 2: Run them to verify they fail** -- do not compile.
- [ ] **Step 3: Implement.** Delete `StackEntry` and both vectors; port
  every existing heap unit test of the activation stack unchanged in
  intent (rename only where a name says "entry"). The debug generation
  lives in the header's `GENERATION` word and is checked by
  `check_activation` as today.
- [ ] **Step 4: Run the suite** -- `cargo xtask gates commit` and
  `cargo xtask gates release-checks` (`RegisterBase`'s size test), pass.
- [ ] **Step 5: Run the suspend and move stress modes on the suite** --
  `NS_NEXT_STRESS_SUSPEND=1` and `NS_NEXT_STRESS_MOVES=1` (the names as
  `docs/design/README.md`, "Gates", gives them), `cargo nextest run --workspace`
  each, pass.
- [ ] **Step 6: Commit** `feat: activations are frames of tagged words in one reservation`.

### Task 6: The byte bound as a net

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::push`'s
  refusal path), `crates/ns-runtime/src/heap.rs` (a `#[cfg(test)]`
  constructor taking the stack's reservation size)
- Test: unit test in `crates/ns-runtime/src/interpreter/` beside the
  depth-limit tests

**Interfaces:**
- Consumes: T5's `Result<ActivationReference, StackExhausted>`.
- Produces: a push the reservation refuses is signalled exactly as a push
  past L is (`push_past_the_limit`'s refusal path), so `on: Error do:`
  catches it and the text is `stack overflow: recursion too deep`.

- [ ] **Step 1: Write the failing test**
  `a_recursion_the_reservation_cannot_hold_is_a_catchable_stack_overflow`:
  an interpreter whose stack reservation is 1 MiB, running
  ```newspeak
  class Deep = Object new (
  ) (
    recurse: depth = (
      | first second third fourth |
      ^self recurse: depth + 1.
    )

    public main: platform args: arguments = (
      platform transcript
          show: ([self recurse: 0] on: Error do: [:error | error messageText]);
          newLine.
    )
  )
  ```
  (a frame of about 20 words, so 1 MiB holds about 6,500 of them, far
  below L)
  as an `indoc!` module, run the way the neighbouring depth-limit unit
  tests run a module; assert the output is
  `stack overflow: recursion too deep` and the status 0, under both
  lowerings.
- [ ] **Step 2: Run it to verify it fails** -- the push panics or aborts.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the suite** -- `cargo xtask gates commit`, passes.
- [ ] **Step 5: Commit** `feat: the stack's reservation refuses a push as the depth limit does`.

### Task 7: The documentation describes frames as words

**Files:**
- Modify: `docs/design/execution.md` ("Representation", "Lifecycle",
  "What an activation carries", "Suspending and resuming", "The depth
  limit" -- its 104-byte entry figure becomes the header's size and the
  reservation's -- and "Planned: frames as words in one reservation",
  folded into "Representation" in the present tense and deleted as a
  planned section; "Planned: JIT tiers" says 7a is built)
- Modify: `docs/design/memory.md` ("Frames, the activation stack and the
  marriage", "What the collector traces", "The memory layer" -- the
  heap's second reservation)
- Modify: `CLAUDE.md` (the `ns-runtime` paragraph names `heap/frames.rs`)
- Modify: `docs/OPEN-WORK.md` (road step 7: 7a built; "Next" names 7a′)

- [ ] **Step 1:** Rewrite each section to the current state; no sentence
  labelled historical.
- [ ] **Step 2: Commit** `docs: activations as frames of tagged words`.

### Task 8: Measure, decide, and the end of the batch

- [ ] **Step 1: Measure** by execution.md's "What suspending and resuming
  moved" protocol in full: `master` (`d135623a` or the merge base)
  against the branch's head, three release builds a side with the pinned
  `RUSTFLAGS`, each in its own `--target-dir`, the seven micro sources at
  `--iterations 30`, five rounds a sitting, the sixteen Are We Fast Yet
  programs, three rounds a sitting, two sittings, the six binaries
  rotating.
- [ ] **Step 2: Decide.** Taken if no program is slower with its builds
  not overlapping `master`'s. Otherwise **stop and report to the user**:
  the step then moves to road step 8 and the branch is not merged as is.
- [ ] **Step 3: Record** the table as execution.md's "What frames as
  words moved", beside "What suspending and resuming moved", with what
  it covers and what not; a `docs/PROGRESS.md` entry for road step 7a.
- [ ] **Step 4: Run** `cargo xtask gates end-of-batch`; passes.
- [ ] **Step 5: Commit** `docs: road step 7a measured, and its progress entry`.
