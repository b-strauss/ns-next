# The Loop over Locals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vm::run` runs many instructions per entry with the running frame's state in local variables, each arm advancing the program counter itself, and reads a register with one index — so no per-instruction `Flow` crosses the stack and no new instruction arm can re-lay out the send path.

**Architecture:** Cog's StackInterpreter keeps `localIP`/`localFP` as locals of `interpret()`; this plan does the same in safe Rust. `run` enters once per stretch between safepoints; `run_stretch` holds activation, code, program counter, feedback base and a new `RegisterBase` as locals, dispatches `code.instructions[program_counter]`, lets straight-line arms `continue`, and has sends, returns, unwinds and forcings answer a one-byte `Transfer` with the next frame written into a `Handoff`. Decisions stay in `interpreter/`, untouched.

**Tech Stack:** Rust 1.98.0 (edition 2021), crate `ns-runtime`, `cargo-nextest`, the measurement protocol in `docs/design/execution.md`, `dumpbin` from the Visual Studio Build Tools.

**Spec:** `docs/design/execution.md`, "Planned: the loop over locals" (and `memory.md`'s pointer to it in "The brand, checked by the compiler rather than by convention"). Read it before any task. The plan departs from it in one detail, written into the design by Task 2: the design says helpers "answer a one-byte `Continue { Stay, Leave }`"; they answer `Transfer { Next, Switched, Exited }`, because the arm must also know whether to advance the program counter or adopt a new frame — still one byte, still in a register.

## Global Constraints

- All code, identifiers, comments, doc comments and commit messages are in English; conversation with the user is in German.
- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; `clippy.toml`'s `min_ident_chars`).
- Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult` or a `HostResult` (`Option::expect` on an `Option<HostResult>` needs no `Debug` and is allowed).
- The crate forbids `unsafe` code; nothing here needs any.
- `vm.rs` holds no dispatch logic: its arms read registers, call one function in `interpreter/` or one of `vm.rs`'s own control helpers, and carry out what it answers. No decision, lookup or error text moves into `vm.rs`.
- Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`; `RegisterBase` is defined there and its inner index is private to it.
- A move or collection happens only between two entries (`Interpreter::enter`); nothing branded may be held across the outer loop's `interpreter.safepoint()`. `FrameRegisters` and `Handoff` hold only unbranded values (`ActivationReference`, `Rc<CompiledCode>`, `usize`, `FeedbackBase`, `RegisterBase`, `HostResult`).
- The safepoint is tested where it is today: after every switch to another activation and after every backward jump — never elsewhere, never skipped.
- Program outcomes are pinned: no golden's recorded `status`, `stdout` or `stderr` changes in this plan. A golden that changes is a defect to investigate.
- Performance is the project's top priority: measure only release builds with the pinned `RUSTFLAGS` (`CLAUDE.md`, "Working in this repository"); a comparison is several builds a side, pooled.
- Conventional commits; every commit message ends with a `Co-Authored-By:` trailer naming the model that actually wrote the code, then the `Claude-Session:` line the session's attribution reminder gives. Never override git identity.
- Line endings: every committed file is LF (`crates/ns-runtime/core-library/**` is `-text`); check a blob with `git cat-file -p HEAD:<file> | tr -cd '\r' | wc -c`.
- **Before every commit:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  Because this plan rewrites the run loop, Tasks 2 and 3 also run the three stress modes before committing (the block under "once at the end of a batch of work" in `CLAUDE.md`). A commit that changes only Markdown runs none of them.
- The end-of-batch gates (stress modes, `ns-lsp`, `run-corpus` and its stress variants, the release suite) run in Task 4. Nothing under `editors/jetbrains` or `crates/ns-syntax` changes.
- Design documents are living; Task 4 turns "Planned: the loop over locals" into present tense.
- Every command runs in the foreground; a subagent never hands back while its own background work runs. Work on branch `feature/loop-over-locals` (created from `master` before Task 1), directly, no worktree.

## Review Focus

1. **A safepoint requested while a send's callee has just been pushed** — the stretch must write the callee's frame back to `FrameRegisters` before leaving, and the outer loop must resume exactly there after the collection moved everything. Pinned by the stress modes in Tasks 2 and 3 (`NS_NEXT_STRESS_MOVES`/`COLLECT` request one at every safepoint) and by Task 2's test `a_stretch_leaves_for_a_requested_safepoint_and_resumes_where_it_stood`.
2. **A register base kept after the running activation changed** — after a return, an unwind, a `retry` restart or a reexecution, the base must be recomputed for the new activation; a stale one silently reads the wrong window in a release build. Pinned by Task 1's `reading_through_a_stale_register_base_panics` and by every golden under the debug suite in Task 3, where each access checks the base's activation and generation.
3. **The run's floor** — a stretch must end with `Leave::Exit` exactly when `finish` or `unwind` reaches the floor, including the host-passed-future case of `reexecute`; `Interpreter::run_from_host`'s balanced-stack debug assertion catches an extra or missing pop. Pinned by the existing `a_host_passing_an_unresolved_future_to_a_leaf_is_an_error` and the exception goldens.
4. **A backward jump in a loop that sends nothing** — it must still reach a safepoint when one is requested, or a collection requested during `whileTrue:`-style loops never happens. Pinned by Task 2's `a_loop_that_sends_nothing_still_reaches_a_safepoint`.
5. **A new instruction arm moving no benchmark** — the reason for this plan. Pinned by Task 4's robustness build, measured beside the head and not committed.

---

## File Structure

- `crates/ns-runtime/src/heap.rs` — `RegisterBase`, `Heap::register_base`, `Heap::register`, `Heap::set_register`, and their unit tests (Task 1).
- `crates/ns-runtime/src/vm.rs` — `run`, `run_stretch`, `FrameRegisters`, `Handoff`, `Transfer`, `Leave`; every arm and control helper (`send`, `deliver`, `finish`, `resume_caller`, `reexecute`, `unwind`, `force_and_reexecute`, `initialize_slot_refused`, `simultaneous_clause_instruction` and its four helpers); `Flow` removed (Task 2); register access through the base (Task 3).
- `crates/ns-runtime/src/lib.rs` or `heap.rs`'s re-exports — `RegisterBase` exported only as far as tests need.
- Tests: `crates/ns-runtime/src/heap.rs` (unit), `crates/ns-runtime/tests/vm.rs` (safepoint behaviour, Task 2).
- Docs: `docs/design/execution.md`, `docs/design/memory.md`, `docs/design/README.md`, `CLAUDE.md`, `docs/PROGRESS.md` (Tasks 2 and 4).

---

### Task 1: `RegisterBase` in `heap.rs`

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (beside `ActivationReference` ~line 216; accessors beside `activation_local` ~line 2427; tests beside `reading_through_a_popped_activation_handle_panics` ~line 3654)

**Interfaces:**
- Produces:
  - `pub struct RegisterBase` — `Clone + Copy`, fields private: `index: u32` (the window's start in `stack.values`) and, under `#[cfg(debug_assertions)]`, `activation: ActivationReference`.
  - `pub fn register_base(&self, activation: ActivationReference) -> RegisterBase`
  - `pub fn register(&self, base: RegisterBase, index: u16) -> Value<'gc>`
  - `pub fn set_register(&mut self, base: RegisterBase, index: u16, value: Value<'gc>)`

- [ ] **Step 1: Write the failing tests**

In `heap.rs`'s test module, beside the stale-activation test, using the same `header()` helper:

```rust
    /// A register base names a window by index, as an activation handle
    /// does: reading through it reads the same register the activation
    /// handle does.
    #[test]
    fn a_register_base_reads_and_writes_its_activations_registers() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[Value::SmallInteger(7)], 2);
        let base = heap.register_base(activation);
        assert!(matches!(heap.register(base, 0), Value::SmallInteger(7)));
        heap.set_register(base, 2, Value::SmallInteger(9));
        assert!(matches!(heap.activation_local(activation, 2), Value::SmallInteger(9)));
        heap.pop_activation(activation);
    }

    /// A base kept past its activation's pop names whatever activation
    /// reuses the window next -- a different, valid window, so a release
    /// build would read the wrong register. Debug builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale activation handle")]
    fn reading_through_a_stale_register_base_panics() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 1);
        let base = heap.register_base(first);
        heap.pop_activation(first);
        let _second = heap.push_activation(header(), &[], 1);
        let _ = heap.register(base, 0);
    }

    /// The window of a deeper activation starts after its caller's: a base
    /// survives the register file growing, since it is an index.
    #[test]
    fn a_register_base_survives_a_deeper_push() {
        let mut heap = Heap::new();
        let caller = heap.push_activation(header(), &[Value::SmallInteger(1)], 1);
        let base = heap.register_base(caller);
        let callee = heap.push_activation(header(), &[], 4096);
        assert!(matches!(heap.register(base, 0), Value::SmallInteger(1)));
        heap.pop_activation(callee);
        heap.pop_activation(caller);
    }

    /// In a release build a base is exactly its index.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_register_base_is_four_bytes_in_a_release_build() {
        assert_eq!(std::mem::size_of::<RegisterBase>(), 4);
    }
```

Adapt `push_activation`'s argument list to its real signature (read the neighbouring tests); keep each test's meaning.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --lib register_base`
Expected: compile error, no `RegisterBase` / `register_base`.

- [ ] **Step 3: Implement**

Beside `ActivationReference`:

```rust
/// Where a running activation's registers start in the activation stack's
/// register file: Cog's `localFP`, held by the run loop so that a register
/// access is one index (`docs/design/execution.md`, "The loop over locals").
/// It is an index, not a pointer, so it survives the register file growing
/// and a move, which touches no register window; it carries the danger an
/// `ActivationReference` does -- once its activation is popped, the next one
/// at that depth reuses the window -- and a debug build checks every access
/// against its activation's generation, as `check_activation` does.
#[derive(Clone, Copy)]
pub struct RegisterBase {
    index: u32,
    #[cfg(debug_assertions)]
    activation: ActivationReference,
}
```

Beside `activation_local`:

```rust
    /// The register window of `activation`, computed once per switch to it.
    pub fn register_base(&self, activation: ActivationReference) -> RegisterBase {
        self.check_activation(activation);
        RegisterBase {
            index: self.stack.entries[activation.index as usize].base,
            #[cfg(debug_assertions)]
            activation,
        }
    }

    /// Register `index` of the window `base` names: one index into the
    /// register file. A debug build checks the base is still its
    /// activation's and the register inside its window.
    #[inline(always)]
    pub fn register(&self, base: RegisterBase, index: u16) -> Value<'gc> {
        #[cfg(debug_assertions)]
        self.check_register(base, index);
        self.stack.values[base.index as usize + index as usize]
    }

    #[inline(always)]
    pub fn set_register(&mut self, base: RegisterBase, index: u16, value: Value<'gc>) {
        #[cfg(debug_assertions)]
        self.check_register(base, index);
        self.stack.values[base.index as usize + index as usize] = value;
    }

    #[cfg(debug_assertions)]
    fn check_register(&self, base: RegisterBase, index: u16) {
        self.check_activation(base.activation);
        let _ = self.register_slot(base.activation.index, index);
    }
```

(`register_slot`'s own `debug_assert!` checks the window bound; if `entries[..].base` is not `u32`, convert as `register_slot` does.) Export `RegisterBase` from `heap.rs` the way `ActivationReference` is exported.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime --lib register_base` and `cargo nextest run -p ns-runtime --release --lib a_register_base_is_four_bytes`
Expected: PASS.

- [ ] **Step 5: Per-commit gates, then commit**

```bash
git add crates/ns-runtime/src/heap.rs
git commit -m "feat: add a register base naming a running activation's register window"
```

---

### Task 2: The loop over locals

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs` (whole file: `Flow` at ~37, `run` ~77, `poll` ~154, `execute` ~162, every helper ~602–1030)
- Test: `crates/ns-runtime/tests/vm.rs`
- Modify: `docs/design/execution.md` ("Planned: the loop over locals" — the `Transfer` departure)

**Interfaces:**
- Consumes: `Heap::register_base` (Task 1) — used here only to fill `FrameRegisters.register_base`; registers are still read through `read_local`/`write_local` in this task.
- Produces:
  - `struct FrameRegisters { activation: ActivationReference, code: Rc<CompiledCode>, program_counter: usize, feedback_base: FeedbackBase, register_base: RegisterBase }`
  - `enum Leave { Safepoint, Exit(HostResult) }`
  - `#[derive(Clone, Copy, PartialEq, Eq)] enum Transfer { Next, Switched, Exited }`
  - `struct Handoff { activation: ActivationReference, code: Option<Rc<CompiledCode>>, program_counter: usize, feedback_base: FeedbackBase, exit: Option<HostResult> }`
  - `fn run_stretch<'gc>(mutator: &mut Mutator<'gc>, frame: &mut FrameRegisters, floor: usize) -> Leave`
  - Every control helper's signature gains `handoff: &mut Handoff` and answers `Transfer` instead of `Flow`: `send`, `deliver`, `finish`, `resume_caller`, `reexecute`, `unwind`, `force_and_reexecute`, `initialize_slot_refused`, `simultaneous_clause_instruction` and its four helpers.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/vm.rs`, which already has `load`, `new_instance`, `send`, `expect_ok` and `integer_of`. The stress mode requests a safepoint at every safepoint and counts the ones taken (`Interpreter::set_stress_moves`, `Interpreter::stress_safepoints`, `interpreter/safepoint.rs`), so the tests count exactly the places the loop tests the flag:

```rust
/// Every switch to another activation is a safepoint: with one requested at
/// each, a send and its return reach at least two, and the run carries on
/// where it stood after each -- the callee after its push, the caller after
/// the return (`docs/design/execution.md`, "The loop over locals").
#[test]
#[cfg(debug_assertions)]
fn a_stretch_leaves_for_a_requested_safepoint_and_resumes_where_it_stood() {
    let (mut interpreter, class) = load(indoc::indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public outer = ( ^self inner + 1 )
            public inner = ( ^41 )
        ) : ()
    "});
    let probe = new_instance(&mut interpreter, &class);
    interpreter.set_stress_moves(true);
    let answer = expect_ok(send(&mut interpreter, &probe, "outer", &[]), "outer");
    let safepoints = interpreter.stress_safepoints();
    interpreter.set_stress_moves(false);
    interpreter.enter(|mutator| assert_eq!(integer_of(&mutator.fetch(&answer)), 42));
    assert!(safepoints >= 2, "the push of inner and the return to outer: {safepoints}");
}

/// A loop that sends nothing -- the optimized lowering inlines
/// `whileTrue:`, so its body is `IntegerBinary`s and jumps -- still reaches
/// a safepoint at every backward jump.
#[test]
#[cfg(debug_assertions)]
fn a_loop_that_sends_nothing_still_reaches_a_safepoint() {
    let (mut interpreter, class) = load(indoc::indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public count = (
                | index ::= 0. |
                [index < 1000] whileTrue: [index:: index + 1].
                ^index
            )
        ) : ()
    "});
    let probe = new_instance(&mut interpreter, &class);
    interpreter.set_stress_moves(true);
    let answer = expect_ok(send(&mut interpreter, &probe, "count", &[]), "count");
    let safepoints = interpreter.stress_safepoints();
    interpreter.set_stress_moves(false);
    interpreter.enter(|mutator| assert_eq!(integer_of(&mutator.fetch(&answer)), 1000));
    assert!(safepoints >= 1000, "one per backward jump: {safepoints}");
}
```

If `load` is not the file's helper for a source with a nested class, follow the neighbouring test that instantiates a nested class (search the file for `class Probe` or `nested`). Both tests pass on today's loop — they pin behaviour the rewrite must keep.

- [ ] **Step 2: Run them against today's loop**

Run: `cargo nextest run -p ns-runtime --test vm safepoint`
Expected: PASS on the unchanged loop. Then temporarily delete the `poll(interpreter)` in `Flow::Jump`'s backward branch, confirm `a_loop_that_sends_nothing_still_reaches_a_safepoint` FAILS (its count drops below 1000), and restore it; likewise delete the `poll` in `Flow::Switch` and confirm the first test fails. Record all runs in the report.

- [ ] **Step 3: The types**

Replace `enum Flow` with:

```rust
/// The running frame's state, which the loop holds in locals while it runs
/// and in this struct between two stretches: nothing here is branded, so it
/// survives the safepoint between them (`docs/design/execution.md`, "The
/// loop over locals").
struct FrameRegisters {
    activation: ActivationReference,
    code: Rc<CompiledCode>,
    program_counter: usize,
    feedback_base: FeedbackBase,
    register_base: RegisterBase,
}

/// Why a stretch ended.
enum Leave {
    /// A safepoint is requested; the frame is written back.
    Safepoint,
    /// The run's base activation is done.
    Exit(HostResult),
}

/// Where control goes after a send, a return, an unwind or a forcing: one
/// byte, answered in a register. The frame a `Switched` names and the
/// result an `Exited` carries are in the `Handoff`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    /// Resume after the instruction: its value, if any, is written.
    Next,
    /// Run the frame the handoff names, from its program counter.
    Switched,
    /// The run is over; the handoff holds its result.
    Exited,
}

/// What a `Transfer::Switched` or `Transfer::Exited` carries.
struct Handoff {
    activation: ActivationReference,
    code: Option<Rc<CompiledCode>>,
    program_counter: usize,
    feedback_base: FeedbackBase,
    exit: Option<HostResult>,
}

impl Handoff {
    /// The frame of `activation`, from `program_counter`.
    fn switch_to(&mut self, mutator: &Mutator<'_>, activation: ActivationReference, program_counter: usize) -> Transfer {
        self.activation = activation;
        self.code = Some(mutator.heap.activation_code(activation));
        self.program_counter = program_counter;
        self.feedback_base = mutator.heap.activation_feedback_base(activation);
        Transfer::Switched
    }

    fn exit(&mut self, result: HostResult) -> Transfer {
        self.exit = Some(result);
        Transfer::Exited
    }
}
```

- [ ] **Step 4: `run` and `run_stretch`**

```rust
pub(crate) fn run(interpreter: &mut Interpreter, base: ActivationReference) -> HostResult {
    let (floor, mut frame) = interpreter.enter(|mutator| {
        (
            mutator.heap.activation_position(base),
            FrameRegisters {
                activation: base,
                code: mutator.heap.activation_code(base),
                program_counter: 0,
                feedback_base: mutator.heap.activation_feedback_base(base),
                register_base: mutator.heap.register_base(base),
            },
        )
    });
    loop {
        match interpreter.enter(
            #[inline(always)]
            |mutator| run_stretch(mutator, &mut frame, floor),
        ) {
            Leave::Safepoint => interpreter.safepoint(),
            Leave::Exit(result) => return result,
        }
    }
}

/// Instructions of the running frame and every frame it switches to, inside
/// one entry, until a safepoint is requested at a switch or a backward jump,
/// or the run's base activation is done. The frame's state lives in locals
/// here -- Cog's `localIP`/`localFP` -- and is written back to `frame` only
/// when the stretch leaves for a safepoint.
#[inline(always)]
fn run_stretch<'gc>(mutator: &mut Mutator<'gc>, frame: &mut FrameRegisters, floor: usize) -> Leave {
    let mut activation = frame.activation;
    let mut code = Rc::clone(&frame.code);
    let mut program_counter = frame.program_counter;
    let mut feedback_base = frame.feedback_base;
    let mut register_base = frame.register_base;
    let mut handoff = Handoff { activation, code: None, program_counter: 0, feedback_base, exit: None };
    loop {
        let transfer = match &code.instructions[program_counter] {
            // ... every arm, below ...
        };
        match transfer {
            Transfer::Next => program_counter += 1,
            Transfer::Switched => {
                activation = handoff.activation;
                code = handoff.code.take().expect("internal: a switch names its code");
                program_counter = handoff.program_counter;
                feedback_base = handoff.feedback_base;
                register_base = mutator.heap.register_base(activation);
                if mutator.heap.safepoint_requested() {
                    *frame = FrameRegisters { activation, code, program_counter, feedback_base, register_base };
                    return Leave::Safepoint;
                }
            }
            Transfer::Exited => {
                return Leave::Exit(handoff.exit.take().expect("internal: an exit carries its result"));
            }
        }
    }
}
```

`interpreter.safepoint()` is today's slow path of `poll`; `poll` itself goes (its doc comment moves onto `run`). If `Heap::safepoint_requested` is not reachable from `Mutator` under that name, use whatever `World::safepoint_requested` reads (`heap/world.rs` ~76).

- [ ] **Step 5: The arms**

Convert every arm of today's `execute` into an arm of `run_stretch`'s `match`, with these rules — read today's arm, keep its body, change only its ending:

- An arm that answered `Flow::Next` ends with `program_counter += 1; continue;`. Example:
  ```rust
            Instruction::LoadSelf { destination } => {
                let value = mutator.heap.activation_receiver(activation);
                write_local(&mut mutator.heap, activation, *destination, value);
                program_counter += 1;
                continue;
            }
  ```
- An arm that answered `Flow::Jump(target)` sets `program_counter = target` and, **only when `target <= program_counter`** (a backward jump), tests the safepoint first:
  ```rust
            Instruction::Jump { target } => {
                let target = *target as usize;
                if target <= program_counter && mutator.heap.safepoint_requested() {
                    *frame = FrameRegisters { activation, code: Rc::clone(&code), program_counter: target, feedback_base, register_base };
                    return Leave::Safepoint;
                }
                program_counter = target;
                continue;
            }
  ```
  Put the backward-jump test in one `#[inline(always)]` local helper or macro used by `Jump`, `JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue`, `IntegerBinary`'s fallback jump and `SendMirror`'s failure jump, so it is written once. (Today every `Flow::Jump` goes through the same backward test in `run`; keep exactly that set.)
- An arm that answered what a helper answered (`send`, `deliver`, `finish`, `unwind`, `initialize_slot_refused`, `simultaneous_clause_instruction`) evaluates to that helper's `Transfer`, passing `&mut handoff`; the `match transfer` after the dispatch applies it. `SendMirror`'s `None` (leaf declined) is a jump to `failure`, as today.
- `MakeClosure`, `ReturnSlotIfFilled`, `NonLocalReturn` and the rest follow the same three rules.

- [ ] **Step 6: The helpers**

Each helper takes `handoff: &mut Handoff` and answers `Transfer`:
- `deliver`: `Step::Value` → write, `Transfer::Next`; a pushed callee → `handoff.switch_to(mutator, callee, 0)`; an error → `unwind(...)`.
- `finish`: every `Flow::Switch { activation: next, .. program_counter: 0 ..}` → `handoff.switch_to(mutator, next, 0)`; `Flow::Exit(result)` → `handoff.exit(result)`; `resume_caller(...)` → answers `handoff.switch_to(mutator, caller, program_counter + 1)` after writing the value; `reexecute(...)` → `handoff.switch_to(mutator, caller, stored_program_counter)` or `handoff.exit(...)` at the floor.
- `unwind`: `Restarted { target }` and a pushed block → `handoff.switch_to(mutator, target_or_next, 0)`; `Returned` → `finish(...)`; `Exit(result)` → `handoff.exit(mutator.to_host(result))`.
- `force_and_reexecute`: store the program counter, push, `handoff.switch_to(mutator, callee, 0)`.
- `initialize_slot_refused`, `simultaneous_clause_instruction` and its four helpers: `Flow::Next` → `Transfer::Next`, the rest as above. They keep reading their instruction back from `mutator.heap.activation_code(activation)` as today.
- Keep every comment that explains a decision (the `mem::forget` of `Continuation::Deliver`, the `#[inline(never)]` on the simultaneous-clause helpers); reword only what names `Flow`.

- [ ] **Step 7: Run everything, the stress modes included**

Run: `cargo nextest run --workspace --exclude ns-lsp`, then the three stress modes as in `CLAUDE.md` (Git Bash):
```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
```
Expected: every run passes, no golden changed.

- [ ] **Step 8: Docs**

In `execution.md`'s "Planned: the loop over locals", replace "answer a one-byte `Continue { Stay, Leave }`, leaving a run's result in an out-parameter on the rare exit" with "answer a one-byte `Transfer { Next, Switched, Exited }`, the next frame or a run's result written into a `Handoff`".

- [ ] **Step 9: Per-commit gates, then commit**

```bash
git add crates/ns-runtime/src/vm.rs crates/ns-runtime/tests/vm.rs docs/design/execution.md
git commit -m "perf: run instructions in a loop over the frame's locals, without a per-instruction Flow"
```

---

### Task 3: Registers through the base

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs` (every `read_local` / `write_local` on the running activation inside `run_stretch` and the helpers that run on it)

**Interfaces:**
- Consumes: `Heap::register`, `Heap::set_register`, `RegisterBase` (Task 1); `run_stretch`'s `register_base` local and `Handoff`/`Transfer` (Task 2).
- Produces: nothing new for later tasks.

- [ ] **Step 1: Replace the running frame's register accesses**

In every arm of `run_stretch`, `read_local(&mutator.heap, activation, register)` becomes `mutator.heap.register(register_base, register)` and `write_local(&mut mutator.heap, activation, register, value)` becomes `mutator.heap.set_register(register_base, register, value)`. Pass `register_base` to `send` and `deliver` (a new parameter beside `activation`) for their argument gathering and `Step::Value` write. Leave unchanged:
- `resume_caller`'s write into the caller (it names another activation; use `register_base(caller)` there only if it is computed anyway — do not add a second base computation);
- `read_outer_local`, `write_outer_local`, `load_vector`, `store_vector` (they reach other frames or vectors in `interpreter/locals.rs`);
- the out-of-line simultaneous-clause helpers and `initialize_slot_refused` (cold; they may keep `read_local`).

- [ ] **Step 2: Run everything, the stress modes included**

Run the full suite and the three stress modes as in Task 2 Step 7.
Expected: every run passes. A debug-build panic "stale activation handle" names a place where `register_base` was not recomputed after a switch — fix the switch path, not the check.

- [ ] **Step 3: Per-commit gates, then commit**

```bash
git add crates/ns-runtime/src/vm.rs
git commit -m "perf: read and write the running frame's registers through its register base"
```

---

### Task 4: Measurement, robustness, the design in present tense, the end-of-batch gates

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/memory.md`, `docs/design/README.md`, `CLAUDE.md`, `docs/PROGRESS.md`

- [ ] **Step 1: Measure against `9900212`**

By the protocol of `execution.md`, "What removing the re-entries moved" (as "What simultaneous slots cost" applied it): three release builds of `9900212` (exported with `git archive` under `.temp/`) and three of `HEAD`, each with the pinned `RUSTFLAGS` (PowerShell: `$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`, the second and third adding `-C metadata=buildb` / `buildc`, each its own `--target-dir`; `Remove-Item Env:RUSTFLAGS` after), `ns-next bench <fixture> --iterations 30` over the four `goldens/bench/` fixtures, five rounds a sitting rotating the six binaries, two sittings. Expected: all four at least level (overlapping), `bench_megamorphic.ns` recovering the 4%. If a fixture is slower, bisect between Task 2 and Task 3 and report with numbers.

- [ ] **Step 2: The robustness build**

In a scratch copy of `HEAD` (not committed): add one instruction variant to `bytecode::Instruction` that no lowering emits and give it an arm in `run_stretch` that calls an `#[inline(never)]` function and answers its result (the shape that cost 4% before); add whatever exhaustive matches the compiler demands with `unreachable!`. Build it three times by the same protocol and measure it beside `HEAD`'s three in one sitting. Expected: every fixture overlapping `HEAD`. If one moves, report it with numbers: that is a finding against the design's claim.

- [ ] **Step 3: `dumpbin`**

On a release build with `CARGO_PROFILE_RELEASE_DEBUG=limited` and the pinned flags, `dumpbin /disasm` (search the Visual Studio Build Tools install, e.g. `C:\Program Files*\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe`, before concluding it is absent): one `ns_runtime::vm::run` label, no `vm::run::closure` and no separate `run_stretch`; record `vm::run`'s instruction count against `9900212`'s 7,083.

- [ ] **Step 4: The design in present tense**

- `execution.md`: "Planned: the loop over locals" becomes "The loop over locals" in present tense, naming `FrameRegisters`, `run_stretch`, `Transfer`, `Handoff`, `RegisterBase`; update "The VM" (and any text that says each instruction answers a `Flow`); add "What the loop over locals moved" with the tables of Steps 1–3 in the style of "What simultaneous slots cost"; close that section's open finding ("is **open**, and the fixture stays about 4% slower until it is found") with a pointer to the new one.
- `memory.md`, "The brand": "Each instruction the loop executes is one entry" becomes "one entry per stretch between two safepoints", and the added "planned" sentence goes.
- `README.md`: the map row's "Planned next" loses "The loop over locals"; its "Built" text gains "the loop runs over the frame's state in locals".
- `CLAUDE.md`, "Working in this repository": the sentence "`vm::run`'s per-instruction step sits near LLVM's inlining threshold and is forced inline; before timing a change to a send's path, check with `dumpbin /disasm` … that no `vm::run::closure` became a function of its own" is updated to the stretch (`run_stretch` is forced inline into `run`; the check is that no `vm::run::closure` and no `run_stretch` became a function of its own).
- `docs/PROGRESS.md`, "Where the next session starts": a paragraph "**The loop over locals is done**" (date, this plan, the measurement verdict), and the compact instruction encoding named as the next plan; remove the open-finding note about the 4%.

- [ ] **Step 5: The end-of-batch gates**

```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules     # "ran 6, failed 0, skipped 205"
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo nextest run --workspace --release   # the size tests: Object 56 bytes, Signal's niche, RegisterBase 4 bytes
```
Expected: every run passes.

- [ ] **Step 6: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: describe the loop over locals as built and record what it moved"
```
