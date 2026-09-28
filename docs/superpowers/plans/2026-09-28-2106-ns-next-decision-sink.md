# A Send's Value Delivered Where It Is Decided — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A send's decision writes a value straight into the sending instruction's destination register and answers one byte, so no `Value` passes through the joined `Result<Step, Signal>` stack slot that costs a store-forwarding stall and a `Step` drop-glue call on every send.

**Architecture:** `interpreter/` gains `DecisionSink`; the functions on the send path (`act_with_base`, `decide_slot_accessor`, `cached`, `miss`, `send_public`, `send_lexical`, `send_super`, `decide_send`, `mirror::execute_method`) become generic over it, calling `sink.value`, `sink.step` or `sink.signal` where they answer today. `StepSink` rebuilds today's `Result<Step, Signal>` for every caller outside the run loop, behind the functions' current names; `vm.rs`'s `RegisterSink` writes the value into the destination register and keeps a rare `Step` or `Signal` in an out slot. Task 1 is an experiment that must show the stall gone before the rest is built.

**Tech Stack:** Rust 1.98.0 (edition 2021), crate `ns-runtime`, `cargo-nextest`, the measurement protocol in `docs/design/execution.md`, `xperf` (Windows Performance Toolkit) and `dumpbin` (Visual Studio Build Tools).

**Spec:** `docs/design/execution.md`, "Planned: a send's value delivered where it is decided", and the profile it argues from, "What the loop over locals moved" → "Where the 3% goes". Read both before any task.

## Global Constraints

- All code, identifiers, comments, doc comments and commit messages are in English; conversation with the user is in German.
- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; `clippy.toml`'s `min_ident_chars`).
- Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult` or a `HostResult`.
- The crate forbids `unsafe` code; nothing here needs any.
- One place decides: `act_with_base` stays one function (generic over the sink, never copied); `vm.rs` holds no dispatch logic — `RegisterSink` only carries out what was decided, as `deliver` does today; no decision, lookup or error text moves into `vm.rs`.
- A cache records what the lookup decided and never decides (`CLAUDE.md`, "A cache records what the lookup decided").
- Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`.
- Program outcomes are pinned: no golden's recorded `status`, `stdout` or `stderr` changes. A golden that changes is a defect to investigate.
- Performance is the project's top priority: measure only release builds with the pinned `RUSTFLAGS` (`CLAUDE.md`, "Working in this repository"); a comparison is several builds a side, pooled.
- Conventional commits; every commit message ends with a `Co-Authored-By:` trailer naming the model that actually wrote the code, then the `Claude-Session:` line the session's attribution reminder gives. Never override git identity.
- Line endings: every committed file is LF; check a blob with `git cat-file -p HEAD:<file> | tr -cd '\r' | wc -c`.
- **Before every commit:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  Task 3 also runs the three stress modes before committing (`CLAUDE.md`, "once at the end of a batch of work"). A commit that changes only Markdown runs none of them.
- The end-of-batch gates run in Task 4. Nothing under `editors/jetbrains` or `crates/ns-syntax` changes.
- Every command runs in the foreground. Work on branch `feature/decision-sink` (created from `master` before Task 1), directly, no worktree; Task 1's experiment lives on its own throwaway branch `spike/decision-sink`, never merged.

## Review Focus

1. **A value written to the destination register while the send's arguments are still needed** — the destination may be a register the send also reads (a receiver or argument register the lowering reuses). Every argument is gathered into the argument buffer before the decision runs, so the early write is safe only if nothing reads a register after `sink.value`. Pinned by Task 3's test `a_send_whose_destination_is_its_receiver_register_answers_right`.
2. **A decision that reaches the sink twice or not at all** — each generic function must call exactly one of `value`, `step`, `signal` on every path, including the fall-throughs of `MethodFastPath` arms that decline. Pinned by `StepSink::into_result`'s `debug_assert!` that exactly one decision arrived, over the whole debug suite (Task 2).
3. **A `Step` held by the sink across its push** — the out slot holds a `Step` (an `Rc` to a method, a closure handle) until `push_step`; nothing may reach a safepoint in between. Pinned by the three stress modes in Task 3.
4. **The callers outside the loop** — the host's send, the unwinder, `Message>>sentTo:`, the block leaves — must see exactly the `Result<Step, Signal>` they see today. Pinned by the unchanged suite in Task 2, which converts them to `StepSink` without touching their code.
5. **The profile claim** — that the stall and the drop-glue call are gone, not moved. Pinned by Task 1's gate and Task 4's before/after `xperf` table.

---

## File Structure

- `crates/ns-runtime/src/interpreter/decision.rs` (new) — `DecisionSink`, `StepSink`.
- `crates/ns-runtime/src/interpreter/resolve.rs` — `act_with_base` generic (`act_into`), `act`/`act_with_base` as `StepSink` wrappers.
- `crates/ns-runtime/src/interpreter/slots.rs` — `decide_slot_accessor` generic.
- `crates/ns-runtime/src/interpreter/inline_cache.rs` — `cached`, `miss`, `send_public`, `send_lexical`, `send_super` generic, with wrappers under today's names.
- `crates/ns-runtime/src/interpreter/mod.rs` — `decide_send` generic, wrapper.
- `crates/ns-runtime/src/interpreter/mirror.rs` — `execute_method` delivering through the sink.
- `crates/ns-runtime/src/vm.rs` — `RegisterSink`, `send` and `SendMirror` through it.
- Tests: `crates/ns-runtime/src/interpreter/decision.rs` (unit), `crates/ns-runtime/tests/vm.rs`.
- Docs: `docs/design/execution.md`, `docs/design/dispatch.md`, `CLAUDE.md`, `docs/PROGRESS.md`.

---

### Task 1: The experiment — `send_lexical` through a sink, profiled (gate)

**Files (on branch `spike/decision-sink`, never merged):**
- Create: `crates/ns-runtime/src/interpreter/decision.rs`
- Modify: `interpreter/resolve.rs`, `interpreter/slots.rs`, `interpreter/inline_cache.rs`, `vm.rs` — only what `SendLexical` needs.

**Interfaces:**
- Produces for Tasks 2–3 (they re-create it on the feature branch from this text, not from the spike): the shapes below, and a gate verdict.

- [ ] **Step 1: The sink**

`interpreter/decision.rs`:

```rust
//! Where a send's decision goes (`docs/design/execution.md`, "A send's value
//! delivered where it is decided"): after Primordial Soup's primitives, which
//! push their result and answer only whether they succeeded, a decision
//! writes a value where it belongs and hands over whole only an activation
//! or a signal.

use crate::interpreter::{Mutator, Signal, Step};
use crate::types::Value;

pub(crate) trait DecisionSink<'gc> {
    fn value(&mut self, mutator: &mut Mutator<'gc>, value: Value<'gc>);
    fn step(&mut self, mutator: &mut Mutator<'gc>, step: Step<'gc>);
    fn signal(&mut self, mutator: &mut Mutator<'gc>, signal: Signal<'gc>);

    /// A decision some function still answers whole -- a native, a nested
    /// class, a factory -- handed on.
    #[inline(always)]
    fn result(&mut self, mutator: &mut Mutator<'gc>, result: Result<Step<'gc>, Signal<'gc>>) {
        match result {
            Ok(Step::Value(value)) => self.value(mutator, value),
            Ok(step) => self.step(mutator, step),
            Err(signal) => self.signal(mutator, signal),
        }
    }
}

/// Today's `Result<Step, Signal>`, rebuilt for every caller outside the run
/// loop.
#[derive(Default)]
pub(crate) struct StepSink<'gc> {
    decided: Option<Result<Step<'gc>, Signal<'gc>>>,
}

impl<'gc> StepSink<'gc> {
    pub(crate) fn into_result(self) -> Result<Step<'gc>, Signal<'gc>> {
        match self.decided {
            Some(result) => result,
            None => unreachable!("internal: a decision reached no sink"),
        }
    }

    fn decide(&mut self, result: Result<Step<'gc>, Signal<'gc>>) {
        debug_assert!(self.decided.is_none(), "internal: a decision reached its sink twice");
        self.decided = Some(result);
    }
}

impl<'gc> DecisionSink<'gc> for StepSink<'gc> {
    fn value(&mut self, _mutator: &mut Mutator<'gc>, value: Value<'gc>) {
        self.decide(Ok(Step::Value(value)));
    }
    fn step(&mut self, _mutator: &mut Mutator<'gc>, step: Step<'gc>) {
        self.decide(Ok(step));
    }
    fn signal(&mut self, _mutator: &mut Mutator<'gc>, signal: Signal<'gc>) {
        self.decide(Err(signal));
    }
}
```

(`unreachable!` on `None` is a compiler-style invariant — no program can cause it; `Option::expect` would need no `Debug` either, choose whichever clippy accepts.)

- [ ] **Step 2: The generic path for `SendLexical`**

- `resolve.rs`: rename today's `act_with_base` body to `pub(crate) fn act_into<S: DecisionSink<'gc>>(&mut self, resolved, feedback_base, receiver, selector, arguments, sink: &mut S)` returning `()`; every `return Ok(Step::Value(v))` becomes `sink.value(self, v); return;`, every `return Ok(step)`/`Ok(Step::…)` becomes `sink.step(self, step); return;`, every `Err(e)` / `return Err(e)` becomes `sink.signal(self, e); return;`, and every call of a function that answers a whole `Result` (`act_handler_method`, `act_handler_slot`, `nested::nested_class`, `builtins::execute`, `decide_factory`, `mirror::execute_method`) becomes `sink.result(self, that_call)` — except `decide_slot_accessor`, which becomes generic too (`decide_slot_accessor_into(…, sink)` in `slots.rs`, same transformation). `act_with_base` stays as the wrapper: `let mut sink = StepSink::default(); self.act_into(…, &mut sink); sink.into_result()`.
- `inline_cache.rs`: `cached_into` and `miss_into` generic over the sink (`miss_into` stays `#[inline(never)]`, generic, so each sink gets its own out-of-line copy), `send_lexical_into` generic; `send_lexical` the wrapper.
- `vm.rs`: a `RegisterSink` (below) used by the `SendLexical` arm only.

```rust
/// Where a decision made for a send in the running frame goes: a value into
/// the destination register, an activation or a signal into `pending`.
/// It decides nothing (`CLAUDE.md`, "`vm.rs` holds no dispatch logic").
struct RegisterSink<'pending, 'gc> {
    register_base: RegisterBase,
    destination: u16,
    outcome: Outcome,
    pending: &'pending mut Pending<'gc>,
}

#[derive(Clone, Copy)]
enum Outcome {
    Delivered,
    Activate,
    Raised,
}

#[derive(Default)]
struct Pending<'gc> {
    step: Option<Step<'gc>>,
    signal: Option<Signal<'gc>>,
}

impl<'pending, 'gc> DecisionSink<'gc> for RegisterSink<'pending, 'gc> {
    #[inline(always)]
    fn value(&mut self, mutator: &mut Mutator<'gc>, value: Value<'gc>) {
        mutator.heap.set_register(self.register_base, self.destination, value);
        self.outcome = Outcome::Delivered;
    }
    fn step(&mut self, _mutator: &mut Mutator<'gc>, step: Step<'gc>) {
        self.pending.step = Some(step);
        self.outcome = Outcome::Activate;
    }
    fn signal(&mut self, _mutator: &mut Mutator<'gc>, signal: Signal<'gc>) {
        self.pending.signal = Some(signal);
        self.outcome = Outcome::Raised;
    }
}
```

After the decision, the arm answers `Transfer::Next` for `Delivered`, pushes `pending.step.take()` the way `deliver` does for `Activate`, and `unwind`s `pending.signal.take()` for `Raised`.

- [ ] **Step 3: Correctness**

Run: `cargo nextest run --workspace --exclude ns-lsp` and `NS_NEXT_STRESS_MOVES=1 cargo nextest run -p ns-cli --test run_goldens`.
Expected: PASS, no golden changed.

- [ ] **Step 4: Profile and measure**

Build `master` and the spike, release, pinned flags, `CARGO_PROFILE_RELEASE_DEBUG=limited` (scripts under `.temp/loop/` from the probing: `build.ps1`, `bench.ps1`, and the attribution in `docs/design/execution.md`, "Where the 3% goes"). `xperf -SetProfInt 1221; xperf -on PROC_THREAD+LOADER+PROFILE`, 400 runs of `bench_loop.ns` on each binary, `xperf -d`, `xperf -i … -a dumper`, attribute samples per function and per instruction in `vm::run` (disassemble with `dumpbin /disasm` and decode the dispatch jump table as the probing did). Then three builds a side, one sitting, all four fixtures, by `bench.ps1`.

- [ ] **Step 5: The gate**

Report, in `.superpowers/sdd/…/task-1-report.md` (the controller's workspace) and in the reply:
- the per-function sample table (`vm::run`, `decide_slot_accessor`, `drop_glue<Step>`) for both;
- the top ten instructions in `vm::run` for both, with context around any store after a 16-byte load;
- the four fixtures' pooled means.

**Pass:** `drop_glue<Step>` no longer on `bench_loop.ns`'s send path (under 1% of samples) and no 16-byte-load-then-store site above 10% of `vm::run`'s samples, and `bench_loop.ns` faster. **Fail:** otherwise — stop; the controller asks the user before Task 2. Nothing from this branch is merged either way; delete `spike/decision-sink` after reporting.

---

### Task 2: The sink in `interpreter/`, every caller unchanged

**Files:**
- Create: `crates/ns-runtime/src/interpreter/decision.rs` (Task 1 Step 1's text, plus the unit test below)
- Modify: `interpreter/resolve.rs`, `interpreter/slots.rs`, `interpreter/inline_cache.rs`, `interpreter/mod.rs` (`decide_send`), `interpreter/mirror.rs` (`execute_method`)

**Interfaces:**
- Produces:
  - `pub(crate) trait DecisionSink<'gc>` with `value`, `step`, `signal`, `result` (Task 1 Step 1).
  - `pub(crate) struct StepSink<'gc>` with `into_result`.
  - `Mutator::act_into<S: DecisionSink<'gc>>(&mut self, resolved: Resolved<'gc>, feedback_base: FeedbackBase, receiver: Value<'gc>, selector: Symbol, arguments: &[Value<'gc>], sink: &mut S)`.
  - `Mutator::decide_slot_accessor_into<S>(…, sink: &mut S)`.
  - `inline_cache::send_public_into`, `send_lexical_into`, `send_super_into` — today's parameters plus `sink: &mut S`, returning `()`.
  - `Mutator::decide_send_into<S>(&mut self, receiver, selector, arguments, sink: &mut S)`.
  - `mirror::execute_method_into<S>(…, sink: &mut S) -> bool` — `true` when the leaf decided (delivered to the sink), `false` when it declined.
  - Every function under its current name (`act_with_base`, `act`, `decide_slot_accessor`, `send_public`, `send_lexical`, `send_super`, `decide_send`, `execute_method`) keeps its signature as a `StepSink` wrapper.

- [ ] **Step 1: The unit test**

In `decision.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interpreter;

    /// `StepSink` rebuilds exactly the `Result<Step, Signal>` a decision
    /// used to answer, so every caller outside the run loop sees what it
    /// saw before the sink.
    #[test]
    fn a_step_sink_answers_what_it_was_given() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let mut sink = StepSink::default();
            sink.value(mutator, Value::SmallInteger(7));
            assert!(matches!(sink.into_result(), Ok(Step::Value(Value::SmallInteger(7)))));
        });
    }

    /// A decision reaches its sink once: a second is a compiler invariant
    /// broken, caught in a debug build.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "reached its sink twice")]
    fn a_decision_reaching_its_sink_twice_panics() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let mut sink = StepSink::default();
            sink.value(mutator, Value::Nil);
            sink.value(mutator, Value::Nil);
        });
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-runtime --lib a_step_sink`
Expected: compile error, no module `decision`.

- [ ] **Step 3: Convert the path**

Apply Task 1 Step 2's transformation to all of: `act_with_base` → `act_into`; `decide_slot_accessor` → `decide_slot_accessor_into`; `cached`/`miss` → generic; `send_public`, `send_lexical`, `send_super` → `*_into` (their uncached fall-backs, `decide_lexical_send` and `decide_super_send`, are handed on with `sink.result(…)`); `decide_send` → `decide_send_into`; `mirror::execute_method` → `execute_method_into` (a leaf's `Some(result)` becomes `sink.result(mutator, result); true`, `None` stays `false` and `act_into` then activates the method through `sink.step`). Keep every decision-explaining comment where its code went. Each current name becomes its `StepSink` wrapper; no caller outside these files changes in this task.

- [ ] **Step 4: Run everything**

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: PASS, nothing changed — `StepSink`'s `debug_assert!` runs on every decision of the debug suite.

- [ ] **Step 5: Per-commit gates, then commit**

```bash
git add crates/ns-runtime/src/interpreter
git commit -m "refactor: deliver a send's decision through a sink, the result rebuilt for every caller"
```

---

### Task 3: The run loop delivers through `RegisterSink`

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs` (`send`, `deliver`, the `SendPublic`, `SendInlinable`, `SendLexical`, `SendSuper` and `SendMirror` arms)
- Test: `crates/ns-runtime/tests/vm.rs`

**Interfaces:**
- Consumes: `DecisionSink`, `send_public_into`, `send_lexical_into`, `send_super_into`, `decide_send_into`, `execute_method_into` (Task 2); `mirror::send_inlined` (unchanged, or an `_into` form if Task 2 found it on the path).
- Produces: `RegisterSink`, `Outcome`, `Pending` (Task 1 Step 2's text) in `vm.rs`.

- [ ] **Step 1: The failing test**

In `tests/vm.rs`, with its `load`, `new_instance`, `send`, `expect_ok`, `integer_of`:

```rust
/// A send's value is written into its destination the moment it is
/// decided; a lowering that reuses the receiver's register as the
/// destination must still see the right answer, since every argument is
/// gathered before the decision runs.
#[test]
fn a_send_whose_destination_is_its_receiver_register_answers_right() {
    let (mut interpreter, class) = load(indoc::indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new (| public count ::= 3. |) (
            public chain = ( ^((count + 1) + 1) + 1 )
        ) : ()
    "});
    let probe = new_instance(&mut interpreter, &class);
    let answer = expect_ok(send(&mut interpreter, &probe, "chain", &[]), "chain");
    interpreter.enter(|mutator| assert_eq!(integer_of(&mutator.fetch(&answer)), 6));
}
```

Before relying on it, check with `ns-next disasm` on the module that some send in `chain` really has its destination equal to its receiver register under the optimized lowering; if none does, change the expression until one does (a chain of unary sends on a temporary usually reuses it), and say which in the report.

- [ ] **Step 2: Run it against today's loop**

Run: `cargo nextest run -p ns-runtime --test vm a_send_whose_destination`
Expected: PASS today (it pins behaviour the change must keep).

- [ ] **Step 3: `RegisterSink` in the loop**

Add `RegisterSink`, `Outcome`, `Pending` to `vm.rs`. `send` builds a `RegisterSink { register_base, destination, outcome: Outcome::Delivered, pending: &mut pending }` around the gathered arguments, calls the arm's `*_into` decision with it, returns the argument buffer, and then:
- `Outcome::Delivered` → `Transfer::Next`;
- `Outcome::Activate` → push `pending.step.take()` exactly as `deliver` pushes a non-value step today (`push_step(step, &arguments, Some(activation), inlinable)` — the arguments buffer must still be alive for `StepArguments::Send`, so return it only after the push, as today);
- `Outcome::Raised` → `unwind(pending.signal.take(), …)`.
`SendMirror` does the same with `execute_method_into`-style delivery if Task 2 gave `send_inlined` a sink form, otherwise it keeps `deliver`. `deliver` stays for the paths that still hold a whole `Result` (continuations, `SendMirror` if unchanged). `Pending` holds branded values only within the entry and is empty between instructions (debug-assert it before every `Leave::Safepoint`).

- [ ] **Step 4: Run everything, the stress modes included**

Run: `cargo nextest run --workspace --exclude ns-lsp`, then the three stress modes (Git Bash, as in `CLAUDE.md`).
Expected: every run passes, no golden changed.

- [ ] **Step 5: Per-commit gates, then commit**

```bash
git add crates/ns-runtime/src/vm.rs crates/ns-runtime/tests/vm.rs
git commit -m "perf: write a send's value into its destination register where it is decided"
```

---

### Task 4: Profile, measurement, robustness, docs, end-of-batch gates

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/dispatch.md`, `CLAUDE.md`, `docs/PROGRESS.md`

- [ ] **Step 1: Profile before and after**

`xperf` on `bench_loop.ns` and `bench_send.ns`, `master` (the branch's base) against `HEAD`, as Task 1 Step 4. Record the per-function table and `vm::run`'s top instructions for both.

- [ ] **Step 2: Measure against the base**

The protocol ("What removing the re-entries moved"): three release builds of the base (exported with `git archive` under `.temp/`) and of `HEAD`, pinned flags, `--iterations 30`, five rounds a sitting rotating the six binaries, two sittings, all four `goldens/bench/` fixtures. Expected: every fixture at least level; `bench_loop.ns` and `bench_send.ns` faster.

- [ ] **Step 3: The robustness probe, repeated**

A scratch copy of `HEAD`, not committed: take `LoadOwnSlot` out of the shared out-of-line arm into an arm of its own that calls `load_own_slot_into` (`#[inline(never)]`) — one more calling arm, semantics unchanged. Three builds, measured beside `HEAD`'s three in one sitting. Expected: every fixture overlapping. Report the numbers either way.

- [ ] **Step 4: `dumpbin`**

One `ns_runtime::vm::run`, no `vm::run::closure`, no `run_stretch` of its own; record its instruction count and stack frame size against the base's 7,143 and 0x3E8.

- [ ] **Step 5: Docs**

- `execution.md`: "Planned: a send's value delivered where it is decided" becomes present tense ("A send's value delivered where it is decided"), naming `DecisionSink`, `StepSink`, `RegisterSink`; a "What delivering where it is decided moved" section with Steps 1–4's tables; "Where the 3% goes" closed with a pointer ("until it lands, an instruction arm is not free" replaced by what Step 3 found).
- `dispatch.md`: wherever it says a send's decision is a `Result<Step, Signal>` / `Step` answered to the loop, say that it is delivered to a sink, `StepSink` rebuilding the result for the other callers.
- `CLAUDE.md`: the sentence "An instruction arm is not free …" updated to Step 3's result; the "`vm.rs` holds no dispatch logic" paragraph names `RegisterSink` beside `deliver` as carrying out a decision.
- `PROGRESS.md`, "Where the next session starts": "**A send's value is delivered where it is decided**" (date, this plan, the verdicts), next the compact instruction encoding; the eight-byte value stays under "Noted for later".

- [ ] **Step 6: The end-of-batch gates**

```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules     # "ran 6, failed 0, skipped 205"
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo nextest run --workspace --release   # the size tests
```
Expected: every run passes.

- [ ] **Step 7: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: describe a send's value delivered where it is decided and record what it moved"
```
