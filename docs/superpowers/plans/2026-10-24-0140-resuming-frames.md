# Resuming an activation from its frame (road step 7b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A running activation can be *suspended* into its frame object
and later *resumed* from it, lazily: only the top comes back onto the
activation stack, and each sender comes back when something returns or
unwinds into it. A sixth stress mode, `NS_NEXT_STRESS_SUSPEND`, suspends
every activation of the run at the safepoints its schedule picks and
runs on from the frames, so every golden goes through the path every
later deoptimization will take. The image decoder refuses a frame that
is not marked uncontinuable.

**Architecture:**
- A frame object is in one of three states, told by its `MARRIED` word:
  - **married**: a `SmallInteger` ≥ 0, the index of its stack entry, as
    today;
  - **suspended** (new): a `SmallInteger` < 0, `-(slot + 1)`, naming its
    row in the heap's side table of suspensions;
  - **returned**: nil, as today.
- A suspended frame holds all its registers (always copied, whatever
  `retains_registers` says) and its `SENDER` field names the frame of the
  activation it returns to, or nil when it returns to the host (it was
  the run's base). Its `uncontinuable` stays false.
- The side table holds, per suspended frame, what a frame object cannot:
  its `Continuation`, whether it is `counted`, its `entered_level` and
  `retains_registers`. Suspended counted activations still count against
  the depth limit.
- A resumed activation is pushed with its registers copied back from the
  frame, which is married to it again (its own registers nil once more,
  as every married frame's). When its frame names a sender, its
  continuation is wrapped in a new `Continuation::ReturnToSuspended`.
  When such an entry is popped -- by a return or by the unwinder -- the
  sender frame is resumed first, and everything after that proceeds as
  if the sender had been on the stack all along.
- Everything that walks from an activation to its sender -- the `sender`
  leaf the kernel's handler search uses, the handler leaves, the
  unwinder, the depth reserve's walks -- goes on into suspended frames.

**Tech Stack:** Rust 1.98.0, `cargo-nextest`.

**Spec:** the decisions are in `docs/design/execution.md`, "Planned: JIT
tiers", the paragraph on 7b; `docs/OPEN-WORK.md`, "Step 7"; and
`docs/design/image-format.md`, "Planned: suspend and resume", the last
paragraph. Read them before Task 1, together with:
- `docs/design/execution.md`: "Activations" (all of it), "The VM",
  "Unwinding", "The depth limit";
- `docs/design/memory.md`: "Where a collection can happen", "Frames, the
  activation stack and the marriage", "What the collector traces", "The
  move stress mode";
- `docs/design/object-model.md`: "What a slot access checks";
- Primordial Soup's `CreateBaseFrame` and `LocalBaseReturn`
  (`prior-art/primordialsoup/vm/interpreter.cc:786-906`), the model.

**Decided with the user on 2026-10-24** (not reopened):
1. Road step 7 is cut into 7b, 7a, 7a′, 7c; this plan is 7b.
2. Resuming is lazy: only the top comes back onto the stack, each sender
   when something returns into it (Primordial Soup's and Cog's model).
3. What a suspended frame cannot hold -- continuation, counting, entered
   level -- lives in a side table of the heap; a reified frame grows by
   nothing. A resumed activation starts with no recorded guard.
4. `retry` keeps popping to its `on:do:` activation; the unwinder resumes
   a target only when it is suspended.
5. The decoder refuses every frame not marked uncontinuable, and
   `verify_graph` gains the `home`-against-`kind` cross-check. The image
   carries nothing of the side table; suspend and resume will.

**Refined while planning, stated here for review:**
- **Suspended is a state of its own, not "unmarried and continuable".**
  The collector decides whether to trace a frame's registers by its
  `MARRIED` word (`heap/object.rs`, the `Format::Frame` arm of the
  pointer ranges: today any non-nil word skips them). A suspended frame's
  registers are the only copy of its values, so the arm must trace them:
  it skips the registers only for a `SmallInteger` ≥ 0. Every reader that
  asks "married?" (`Heap::married_entry`) already answers no for a
  negative one.
- **The stress mode resumes the top at once.** After suspending, the
  safepoint resumes the top frame, so the loop always returns to a stack
  entry; `Interpreter::safepoint` answers whether it rebuilt the stack,
  and `vm::run` then re-reads its `FrameRegisters` from the top entry.
  For that the stretch writes the running activation's program counter
  back into its entry when it leaves for a safepoint, one store on a
  cold path.
- **The feedback base is derived again on resume**, by the rule
  `Mutator::push_step` uses for the frame's kind (a closure's from the
  closure object, every other body's from `InlineCaches::base_for` of its
  code), so the side table holds no number a reclaim could orphan.
- **At the end of a run the side table is empty.** Every path out of a
  run pops through the `ReturnToSuspended` entries, resuming each sender
  in turn, so no suspended frame outlives the run; `run_from_host`
  debug-asserts it beside its depth assertion. This is also why the image
  encoder never meets a suspended frame.

## Global Constraints

- **Every commit is green.** Before each commit, run
  `cargo xtask gates <groups>` with the groups the task names; every task
  runs `commit`. `images` is named for a change to `crates/ns-runtime/src/image/`.
  The end-of-batch gates run once, in Task 6, by the controller
  (`finishing-a-batch` skill); implementers do not run them, and a task's
  commit is work in progress until they have run.
- **Commits** are conventional (`feat:`, `fix:`, `refactor:`, `test:`,
  `docs:`). Each ends with a `Co-Authored-By:` line naming the
  implementing model, then
  `Claude-Session: https://claude.ai/code/session_0186EPjmbyULEGSZ24aM5BMp`.
  Never change the git identity.
- **Run every command in the foreground.** Pass `timeout: 600000` on
  builds and test runs, and never stop while a command of your own is
  still running.
- **Work on the checked-out branch `execution-model`** in the main
  working directory, with no worktree.
- **Names are written out** (`CLAUDE.md`, "Conventions"): `suspension`,
  `sender`, `frame`, `activation`, never `susp`, `fr`, `act`.
- **`vm.rs` holds no dispatch or unwinding logic.** Each change there is
  a call into `interpreter/` or `heap.rs`.
- **Object, frame and activation access goes through `heap.rs`**; the
  frame's words are touched only by the heap module.
- **No golden's `.toml` changes**, and none is added: the stress mode is
  what runs the goldens through the new path. Every outcome under
  `NS_NEXT_STRESS_SUSPEND=1` is the outcome without it.
- **Never `{:?}` a `Value`, and never `.unwrap()` or `.expect()` an
  `EvaluationResult`.** Tests copy a neighbour's `expect_ok`.
- **Newspeak in a Rust test** is an `indoc! {"…"}` with the closing `"}`
  on its own line.
- **A new `Rust`-side field holding a handle gets its `Trace`**
  (`memory.md`, "What the collector traces"): the side table's
  continuations and frames are roots, forwarded on every move.

## Review Focus

1. **A suspension while a nested class is being created**
   (`Continuation::NestedClass`, its cache slot holding the in-progress
   marker). The marker stays while the clause's activation is suspended,
   and `after_clause` caches the class when it returns through the
   resumed sender. Task 2 pins it with
   `a_nested_class_created_across_a_suspension_is_cached_once`.
2. **A future forced across a suspension** (`Continuation::Future`, and
   the caller's instruction run again, `reexecute`). The caller is the
   resumed sender, which runs its stopped instruction again rather than
   resuming after it. Task 2 pins it with
   `a_future_forced_across_a_suspension_runs_its_instruction_again`.
3. **An unwind that passes suspended `ensure:` and `ifCurtailed:`
   activations**, and one whose target is suspended. Each block runs once,
   in order, and the target catches the return. Task 3 pins it with
   `an_unwind_runs_the_ensure_blocks_of_suspended_activations_in_order` and
   `a_return_into_a_suspended_home_is_caught_there`.
4. **The depth limit and its reserve with suspended activations below.**
   The counted depth includes them, so recursion ends at the same depth
   with the same error, and the reserve is still found held. The goldens
   under `goldens/runtime/depth-limit/` pin it under the stress mode;
   Task 3 adds `the_depth_limit_counts_suspended_activations`.
5. **A by-reference write into a suspended frame** (the reference
   lowering, a fallback closure: `StoreOuterLocal`) when the frame is old
   and the value young. The write goes through `frame_set_local` and its
   barrier, and the resume copies the value back. Task 1 pins it with
   `a_young_value_written_into_a_suspended_old_frame_is_remembered`.

---

### Task 1: Suspended frames in the heap

The state, the side table, suspending and resuming, tracing and the
reclaim. Nothing calls it outside the tests yet.

**Files:**
- Modify:
  - `crates/ns-runtime/src/heap.rs` (the side table on `Heap`,
    `suspend_activations`, `resume_activation`, the state predicates,
    `counted_depth`, the marriage readers)
  - `crates/ns-runtime/src/heap/object.rs` (the `Format::Frame` arm of the
    pointer ranges; the `frame_field::MARRIED` comment)
  - `crates/ns-runtime/src/heap/reclaim.rs` (entered levels of the side
    table)
  - `crates/ns-runtime/src/types.rs` (`Continuation::ReturnToSuspended`,
    `ReturnToSuspended`)
  - the `Trace` impls of `Continuation` and of the heap's roots
  - `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::resume` and
    `Mutator::abandon` arms)
- Test: `crates/ns-runtime/src/heap.rs`'s unit tests

**Interfaces:**
- Produces (used by Tasks 2 and 3):
  - `types::Continuation::ReturnToSuspended(Box<ReturnToSuspended<'gc>>)`,
    with `pub struct ReturnToSuspended<'gc> { pub(crate) sender: FrameReference<'gc>, pub(crate) then: Continuation<'gc> }`:
    the entry's own continuation, applied as before, after which control
    goes to `sender`, resumed.
  - `Heap::suspend_activations(&mut self, floor: usize) -> Option<FrameReference<'gc>>`:
    suspends every activation at a position ≥ `floor`, top first, and
    answers the top's frame (`None` when there is none). Per entry: reify
    it; copy every register into the frame; write its program counter;
    set `SENDER` (below); move `then`, `counted`, `entered_level` and
    `retains_registers` into a new side-table row; write `-(row + 1)` into
    `MARRIED`; truncate the entry and its registers; move its count from
    the stack's to the suspended count. `SENDER` is the `sender` of an
    entry whose `then` is `ReturnToSuspended` (its inner `then` is what
    the row keeps), otherwise the frame of the entry below when that entry
    is at a position ≥ `floor`, otherwise nil. The barrier's rule of
    `pop_activation` applies: an old frame that now holds a young value is
    remembered.
  - `Heap::resume_activation(&mut self, frame: FrameReference<'gc>, feedback_base: FeedbackBase) -> ActivationReference`:
    the inverse for one suspended frame. Pushes an entry from the frame's
    receiver, current class, kind, code, program counter and the row's
    fields, `recorded_guard` `DispatchId::NONE`; copies the registers
    back and sets the frame's own to nil; marries the frame to the entry
    and clears its `SENDER`; frees the row; moves the count back. Its
    `then` is `ReturnToSuspended { sender, then }` when the frame named a
    sender, the row's `then` otherwise. It never checks the depth limit:
    the activation was counted all along.
  - `Heap::frame_is_suspended(&self, frame: FrameReference<'gc>) -> bool`
    and `Heap::frame_is_running(&self, frame: FrameReference<'gc>) -> bool`
    (married or suspended).
  - `Heap::suspension_count(&self) -> usize`, for the assertion of Task 2.
  - `Heap::counted_depth` answers the stack's count plus the suspended
    one.
  - `Heap::frame_suspended_continuation(&self, frame: FrameReference<'gc>) -> Option<&Continuation<'gc>>`,
    the row's continuation, for the walks of Task 3.

- [ ] **Step 1: Write the failing unit tests** in `heap.rs`'s tests, on a
  `frame_heap()` with entries pushed by `push_activation` as the
  neighbouring tests do:
  - `suspending_and_resuming_an_activation_keeps_its_registers_and_program_counter`:
    two entries, the top with registers `[1, 2, nil]` and program counter
    7; after `suspend_activations(0)` the depth is 0,
    `frame_is_suspended(top_frame)`, `frame_local(top_frame, 1) == 2`,
    `frame_sender(top_frame) == Some(bottom_frame)`,
    `frame_sender(bottom_frame) == None`; after
    `resume_activation(top_frame, …)` the depth is 1, the entry's
    registers are `[1, 2, nil]`, its program counter 7,
    `frame_activation(top_frame)` is the new entry, and the frame's own
    register words are nil.
  - `a_resumed_activation_returns_to_its_suspended_sender`: the resumed
    top's `then` is `ReturnToSuspended` naming the bottom frame; the
    resumed bottom's `then` is the bottom entry's original continuation.
  - `suspended_activations_still_count_against_the_depth_limit`:
    `counted_depth()` is the same before suspending, after it and after
    resuming one.
  - `a_suspended_frame_survives_a_move_a_scavenge_and_a_collection_with_its_registers`:
    a young register value in a suspended frame is still read back after
    `move_everything`, a scavenge and a full collection, and after the
    resume.
  - `a_young_value_written_into_a_suspended_old_frame_is_remembered`
    (Review Focus 5): tenure, suspend, `frame_set_local` a young object,
    `verify_remembered_set` passes, scavenge, resume, the register holds
    the moved object.
  - `the_reclaim_keeps_the_entered_level_of_a_suspended_activation`: an
    entry of code that addresses its own slots, suspended; a full
    collection; its application row is not freed (copy the shape of the
    reclaim test that pins `entered_levels` for stack entries).
- [ ] **Step 2: Run them and see them fail**, with
  `cargo nextest run -p ns-runtime -E 'test(suspend) | test(suspended)'`.
  They fail to compile.
- [ ] **Step 3: Implement.**
  - The side table is a `Vec<Option<Suspension<'gc>>>` with a free list,
    smallest free row first, as the heap's other tables mint; `Suspension`
    is private to the heap module.
  - The pointer-range arm of `Format::Frame` skips the registers only for
    a `MARRIED` word that is a `SmallInteger` ≥ 0.
  - The side table is a root: marking, the scavenger and the move
    forward its frames and its continuations' handles, through the
    existing `Trace` of `Continuation`. `ReturnToSuspended` traces its
    `sender` and its inner continuation.
  - `GatheredIds` takes the side table's entered levels as it takes the
    stack entries' (`entered_levels`).
  - `Mutator::abandon` of `ReturnToSuspended` abandons its inner
    continuation (the sender is resumed by the unwinder, Task 3).
    `Mutator::resume` never sees one, since `vm::finish` resumes the
    sender first (Task 2): its arm is
    `unreachable!("internal: the run loop resumes a suspended sender before it applies the continuation")`.
- [ ] **Step 4: Run the tests and see them pass**, with the command of
  Step 2.
- [ ] **Step 5: Gates and commit.** `cargo xtask gates commit`, then
  `feat: suspend activations into their frames and resume them`.

---

### Task 2: The suspend stress mode and the return into a suspended sender

**Files:**
- Modify:
  - `crates/ns-runtime/src/interpreter/safepoint.rs` (`stress_suspend`,
    `set_stress_suspend`, `Interpreter::safepoint` answering `bool`)
  - `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::resume_frame`,
    the field `stress_suspend`, the assertion in `run_from_host`)
  - `crates/ns-runtime/src/vm.rs` (`run`, the stretch's write-back,
    `finish`, `reexecute`)
- Test: `crates/ns-runtime/tests/stress.rs`

**Interfaces:**
- Consumes: Task 1's `suspend_activations`, `resume_activation`,
  `ReturnToSuspended`, `suspension_count`.
- Produces (used by Task 3):
  - `Mutator::resume_frame(&mut self, frame: FrameReference<'gc>) -> ActivationReference`:
    derives the feedback base by `push_step`'s rule for the frame's kind
    (minting a closure's range when it has none, as `push_step` does) and
    calls `Heap::resume_activation`.
  - `Interpreter::set_stress_suspend(&mut self, enabled: bool)` and
    `Interpreter::stress_suspensions(&self) -> usize` (how often the
    schedule fired), debug builds only, as `set_stress_collect` and
    `stress_collections` are; `NS_NEXT_STRESS_SUSPEND` read by
    `StressSchedule::from_env`.
  - `Interpreter::safepoint(&mut self, floor: usize) -> bool`: `true` when
    the suspend stress mode rebuilt the stack above `floor`, the run's
    floor `vm::run` already holds.

- [ ] **Step 1: Write the failing tests** in `tests/stress.rs`, each
  running a program twice -- plain, then with `set_stress_suspend(true)`
  -- and asserting the same output and status, and
  `stress_suspensions() > 0` on the second run:
  - `a_deep_recursion_runs_on_from_suspended_frames` (a recursive method
    to depth 2,000 that sums on the way back);
  - `blocks_loops_and_do_run_on_from_suspended_frames` (non-inlined
    blocks capturing and writing a variable, `whileTrue:` and `do:` with
    a block, `do:` with an object that is no block);
  - `a_nested_class_created_across_a_suspension_is_cached_once` (Review
    Focus 1: a superclass clause that loops long enough to pass several
    safepoints; the class is identical on a second access);
  - `a_future_forced_across_a_suspension_runs_its_instruction_again`
    (Review Focus 2: simultaneous slots whose initializer forces a
    future whose computation loops);
  - the same four under the reference lowering, as the neighbouring
    stress tests run both.
- [ ] **Step 2: Run them and see them fail**, with
  `cargo nextest run -p ns-runtime --test stress`. They fail to compile.
- [ ] **Step 3: Implement.**
  - The stress mode takes its turn in `Interpreter::safepoint` after the
    other four, on its own `StressSchedule`: when due,
    `suspend_activations(floor)` followed by `resume_frame` of the top
    frame, and the answer `true`.
  - When the stretch leaves for a safepoint it stores the running
    activation's program counter into its entry
    (`activation_set_program_counter`). When `safepoint` answers `true`,
    `run` re-reads its `FrameRegisters` from the top entry.
  - `finish`: an entry whose `then` is `ReturnToSuspended { sender, then }`
    has its sender resumed (`resume_frame`) right after the pop; `then` is
    then applied exactly as a popped `then` is today, and the caller found
    on top is the resumed sender. `Deliver` stays the first, inline arm.
  - `run_from_host` debug-asserts `suspension_count() == 0` where it
    asserts the depth.
  - Exceptions and non-local returns under the mode are Task 3's; until
    then the stress gate is not run.
- [ ] **Step 4: Run the tests and see them pass**, with the command of
  Step 1.
- [ ] **Step 5: Gates and commit.** `cargo xtask gates commit`, then
  `feat: a stress mode that suspends every activation and runs on from the frames`.

---

### Task 3: Unwinding and the sender walks through suspended frames

**Files:**
- Modify:
  - `crates/ns-runtime/src/interpreter/unwind.rs` (`unwind`,
    `unwind_signal`'s early exit, `overrun_of_the_reserve`)
  - `crates/ns-runtime/src/interpreter/mod.rs` (`raise_return`,
    `depth_reserve_held`, `pushes_the_reserves_does_not_understand`)
  - `crates/ns-runtime/src/builtins/activation.rs` (`activation_leaf`,
    `handler_registers`, `method_name`)
  - `crates/ns-runtime/src/heap.rs` (the frame-level readers the leaves
    and walks need)
- Test: `crates/ns-runtime/tests/stress.rs`, `crates/ns-runtime/tests/exceptions.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces:
  - `Heap::frame_sender_frame(&mut self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>>`:
    the frame of the activation `frame` returns to -- for a married
    frame, the `sender` of its entry's `ReturnToSuspended`, or the entry
    below (reified); for a suspended one, its `SENDER`; `None` for a
    returned one or the run's base.
  - `Heap::frame_unwind_marker(&self, frame: FrameReference<'gc>) -> Option<UnwindMarker>`,
    read from the frame's code, as `activation_unwind_marker` reads the
    entry's.
  - `Mutator::pop_for_unwind(&mut self, top: ActivationReference)`: pops
    `top`, abandons its continuation, and resumes the sender of a
    `ReturnToSuspended`, so the next top is the activation the unwind
    goes on with.

- [ ] **Step 1: Write the failing tests.** In `tests/stress.rs`, each run
  plain and under `set_stress_suspend(true)`, same output, both
  lowerings:
  - `an_exception_signalled_above_suspended_frames_finds_its_handler`;
  - `retry_restarts_a_suspended_on_do_activation`;
  - `an_unwind_runs_the_ensure_blocks_of_suspended_activations_in_order`
    (Review Focus 3: three nested `ensure:` and one `ifCurtailed:`, each
    printing, below a loop that passes safepoints, then a `^` out of a
    block past all of them);
  - `a_return_into_a_suspended_home_is_caught_there` (Review Focus 3);
  - `return_value_and_resume_on_a_suspended_activation` (`return:` and
    `resume:` of an exception whose handler activation is suspended);
  - `the_depth_limit_counts_suspended_activations` (Review Focus 4: a
    recursion to the limit; the error and the depth it reports are the
    same; then one that overruns the reserve).

  In `tests/exceptions.rs`, add
  `a_return_into_a_returned_home_is_still_cannot_return`, which pins that
  a returned frame (not suspended) still raises `cannotReturn`.
- [ ] **Step 2: Run them and see them fail**, with
  `cargo nextest run -p ns-runtime --test stress --test exceptions`.
- [ ] **Step 3: Implement.**
  - `unwind`: the destination is decided again on every iteration from
    `frame_activation(target)`, since a suspended target becomes a stack
    entry only once the pops reach it; every pop is `pop_for_unwind`. The
    early exit of `unwind_signal` for a `Signal::Error` pops the same way.
  - `raise_return`: a home that `frame_is_running` is the target, so a
    suspended home is unwound to; only a returned one is `cannotReturn`.
  - `activation_leaf` works on the frame: `frame_is_running` replaces
    `live.is_some()`; `Sender` answers `frame_sender_frame`'s frame object;
    the handler leaves read and write registers through `frame_local` and
    `frame_set_local` and the marker through `frame_unwind_marker`;
    `returnValue:` and `restart` signal for a running frame.
  - `overrun_of_the_reserve` and `depth_reserve_held` walk the live
    entries and then the suspended frames (`frame_sender_frame` and the
    side table's continuation through
    `frame_suspended_continuation`), never reifying a live entry the walk
    would not have reified before. Neither is on a hot path.
  - Review every other caller of `top_activation`, `activation_sender`
    and `activation_above` (`interpreter/unwind.rs`, `interpreter/mod.rs`)
    and say in the commit message, per call, why it cannot meet a
    suspended frame or how it now handles one.
- [ ] **Step 4: Run the tests and see them pass**, with the command of
  Step 2. Then run the whole suite under the stress mode once,
  `NS_NEXT_STRESS_SUSPEND=1 cargo nextest run --workspace`; every test
  passes.
- [ ] **Step 5: Gates and commit.** `cargo xtask gates commit`, then
  `feat: unwinding and the sender walks go on into suspended frames`.

---

### Task 4: The decoder refuses a continuable frame

**Files:**
- Modify:
  - `crates/ns-runtime/src/image/decode.rs` (the frame record,
    `verify_graph`, two `DecodeError` variants)
  - `crates/ns-runtime/src/image/encode.rs` (a debug assertion)
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `DecodeError::ContinuableFrame` and
  `DecodeError::FrameHomeMismatch`, each with a message in the style of
  their neighbours.

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`, on images
  crafted the way the neighbouring refusal tests craft theirs:
  - `a_frame_not_marked_uncontinuable_is_refused`: one frame record with
    `uncontinuable` false; `load` answers `ContinuableFrame`.
  - `a_method_frame_with_a_home_is_refused` and
    `a_closure_frame_without_a_home_is_refused`, if the format lets the
    second arise (a closure frame's home is derived from its closure; if
    the record cannot express a missing one, drop the test and say so in
    the commit): `FrameHomeMismatch`.
- [ ] **Step 2: Run them and see them fail**, with
  `cargo nextest run -p ns-runtime --test image -E 'test(refused)'`.
- [ ] **Step 3: Implement.** The refusal comes in the graph pass, where
  the record is read; the cross-check in `verify_graph`, before any
  decoded code runs. The encoder debug-asserts that no frame it writes is
  suspended (`frame_is_suspended`), beside its assertion that none is
  married.
- [ ] **Step 4: Run the tests and see them pass.**
- [ ] **Step 5: Gates and commit.** `cargo xtask gates commit images`,
  then `feat: the decoder refuses a frame that could be resumed`.

---

### Task 5: The gate and the documents

**Files:**
- Modify:
  - `xtask/src/main.rs` (`STRESS_MODES` gains `"SUSPEND"`; its tests)
  - `CLAUDE.md` (the gates table: "the six stress modes"; the
    `stress` row)
  - `docs/design/README.md` ("Gates": the sixth command and the counts)
  - `docs/design/execution.md` ("Activations": a section "Suspending and
    resuming" describing the three states, the side table, the lazy
    resume, `ReturnToSuspended` and the stress mode; the 7b paragraph of
    "Planned: JIT tiers" becomes "built" with a pointer to it)
  - `docs/design/memory.md` ("Frames, the activation stack and the
    marriage": a suspended frame's registers are traced, the side table
    is a root, the counted depth includes it; "The move stress mode" or
    its neighbour: the suspend stress mode)
  - `docs/design/image-format.md` ("Planned: suspend and resume": the
    refusal and the cross-check as built)
  - `docs/OPEN-WORK.md` (7b built; the rest of step 7 stays)

- [ ] **Step 1: Make the changes.** Documentation describes the current
  state; no "now", no history.
- [ ] **Step 2: Run `cargo nextest run -p xtask`** and see its tests pass.
- [ ] **Step 3: Gates and commit.** `cargo xtask gates commit`, then
  `docs: suspending and resuming activations, and the sixth stress mode`.

---

### Task 6: Measure and run the end of the batch (controller)

- [ ] **Step 1: Measure** the branch against `master` with the
  `measuring-performance` skill, the sources a change to `finish` and the
  safepoint can move: every `benchmarks/micro/` source and the Are We Fast
  Yet benchmarks. A regression beyond the protocol's noise stops the
  batch and goes to the user.
- [ ] **Step 2: Run `cargo xtask gates end-of-batch`**, which now includes
  the suspend stress mode.
- [ ] **Step 3: `PROGRESS.md`** gets the entry for 7b with the
  measurement, and the `finishing-a-batch` skill is followed.
