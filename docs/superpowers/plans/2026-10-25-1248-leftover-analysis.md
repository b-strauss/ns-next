# Leftover Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A return in road step 7a′'s overlapped frames writes nothing. A collector pre-pass nils what a returned callee left in its sender's registers, and the decoder refuses image code that reads it.

**Architecture:** One forward analysis per body over its control flow marks the scratch registers (`>= local_count`) that may still hold a callee's leftovers. Every send but `SendMirror` taints the registers at and above its slot end, and a write cleans one. The analysis serves three readers:
- the decoder, which refuses a read of a tainted register;
- a debug build, which checks every compiled body under both lowerings;
- the collector, which computes it lazily per code and, before every trace, nils the tainted registers at each frame's program counter.

The top never drops below a live window end, so no word of a live window goes untraced.

**Tech Stack:** Rust 1.98.0, edition 2021. Crate `ns-runtime` only (`#![forbid(unsafe_code)]`).

**Spec:** `.superpowers/sdd/2026-10-24-2054-register-windows/design-b-memo.md`, option (iv), and the user's decisions at the end of `.superpowers/sdd/2026-10-24-2054-register-windows/progress.md`, quoted here:
- Q1, retaining less than today: yes.
- Q2, the decoder's bound: a refusal past a size or work bound is acceptable only where no compiled body can reach it. The bound is derived from what the compiler can emit, and a debug check holds compiled bodies within it. It never refuses valid code.
- Q3: divorce nils all scratch.
- Q4: the debug poison moves into the collector pre-pass, beside the static check.
- B is kept only if it measures faster than the current head, three builds a side; otherwise the fill stays.

## Global Constraints

- **Invariants:** `CLAUDE.md`'s all hold, especially that an image is untrusted input, the amplification bound (`docs/design/memory.md`, "The amplification bound of a rejected load"), the value-register invariant, and wire tags.
- **No wire change:** no new tag, no new record. A new `DecodeError` variant is allowed.
- **Nil, never skip:** a register the analysis calls dead is written nil, or `0x5EAD_5EAD` in a debug build. It is never left untraced.
- **Named locals are always live:** registers below `local_count` are never tainted.
- **Lowerings:** both lowerings' output must pass the verifier, since images can carry reference-lowered bodies (`image/encode.rs:358`). Every golden stays unchanged under both lowerings.
- **Names written out, conventional commits.** Gates per commit: `commit`, plus `images` for `image/` or `bytecode/`, plus `codegen` for the hot-path files `CLAUDE.md` names, plus `release-checks` for a type a size test pins. `end-of-batch` runs once, in Task 6.
- **After this plan:** the register-windows plan's Task 8 Step 4 (docs, PROGRESS) follows, then the final review.

## Review Focus

1. A collection while the top frame stands at a backward jump inside a loop whose body sends. The registers the loop's sends left must be nilled, and the registers the loop head reads must not be. Pinned in Task 4's tests.
2. A frame stopped at an instruction that runs again (a forced future, `MakeFuture`/`ResolveFuture`), or resumed by `resume:`/`retry`/`return:`. The set at its program counter must be the one before that instruction. Pinned in Task 4.
3. `ensure:`/`ifCurtailed:` blocks running during an unwind, so collections happen with frames at unusual program counters. The run under the stress modes in Task 5 must pass.
4. A crafted image whose code reads the register a shipped method's `SendMirror` kept the mirror in, after a send to that method. Refused at load. Pinned in Task 3.
5. A recursion at the 100,000-activation depth limit, with a scavenge per allocation under the scavenge stress mode. The pre-pass walks every frame, and its cost must stay proportional to what tracing already does. Measured in Task 6.

---

### Task 1: The analysis, and its bound proven against the compiler

**Files:**
- Create: `crates/ns-runtime/src/bytecode/leftovers.rs`.
- Modify:
  - `crates/ns-runtime/src/bytecode.rs`: the module, and the shared register effects.
  - `crates/ns-runtime/src/compiler/reference.rs`: move `operands` and `destination` out, into `bytecode`.
- Docs: `docs/design/compiler.md`, a new section "What a callee leaves", with the proof.

**Interfaces:**
- Produces, in `bytecode`:
  - `pub(crate) fn registers_read(instruction: &Instruction) -> impl Iterator<Item = u16>`
  - `pub(crate) fn register_written(instruction: &Instruction) -> Option<u16>`
  - `pub(crate) fn successors(index: u32, instruction: &Instruction) -> impl Iterator<Item = u32>`

  Each is exhaustive over `Instruction`, as `operands`, `destination` and `outer_reach_of` are today.
- Produces, in `bytecode::leftovers`:
  - `pub(crate) struct Leftovers`
  - `pub(crate) fn analyze(instructions: &[Instruction], local_count: u16, register_count: u16) -> Result<Leftovers, LeftoverRefusal>`
  - `pub(crate) enum LeftoverRefusal { Read { instruction: u32, register: u16 }, PastTheBound { instruction: u32 } }`
  - `impl Leftovers { pub(crate) fn tainted_before(&self, program_counter: u32) -> impl Iterator<Item = u16> + '_ }`

**The representation.** The tainted set at a point is `[threshold, register_count) \ cleaned`, where `cleaned ⊆ [threshold, register_count)` is sorted. It is exact under the three operations:
- a send at slot end `end`: `threshold = min(threshold, end)`, and `cleaned` keeps only the members below `end`;
- a write of `register >= threshold`: insert it into `cleaned`;
- a join of `(t1, c1)` and `(t2, c2)` with `t1 <= t2`: `(t1, c1 ∩ (c2 ∪ [t1, t2)))`.

States are stored only at jump targets. Back edges iterate to a fixpoint; thresholds only fall and `cleaned` only shrinks, so the iteration terminates.

**The bound.** Count the work as the sum of stored and touched `cleaned` entries plus instructions visited. The bound is `K × instructions + C`.

- [ ] **Step 1: Write the failing unit tests** in `leftovers.rs`. Each asserts `analyze` and `tainted_before` on hand-built instruction lists:
  - `a_send_taints_the_registers_from_its_slot_end`: after a `SendPublic` with slots `[5, 7)` and `register_count` 10, `tainted_before(1)` is `7, 8, 9`.
  - `a_write_cleans_a_register`.
  - `a_mirror_send_taints_nothing`.
  - `the_slots_a_send_writes_are_clean`.
  - `named_locals_are_never_tainted`: with `local_count` 6, a send with slot end 3 taints `6..`.
  - `a_join_is_tainted_where_either_arm_is`: an `ifTrue:ifFalse:` shape, where one arm sends and the other writes.
  - `a_loop_body_taints_its_head`: a backward jump after a send makes the head's set contain the send's taint.
  - `a_read_of_a_tainted_register_is_refused`: answers `Read { instruction, register }`.
  - `a_read_after_rewriting_is_accepted`.
  - `a_crafted_body_past_the_bound_is_refused`: a body built to make `cleaned` large at many jump targets answers `PastTheBound`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime leftovers`. They fail to compile.
- [ ] **Step 3: Move the register effects** from `reference.rs` into `bytecode.rs`. Then make the moved `register_written` match the decoder's `written_register` (`image/decode.rs:2019`): a `LoadLiteral` of `BodyTooLarge` writes nothing. Have both callers use it.
- [ ] **Step 4: Implement `analyze`** with the representation above.
- [ ] **Step 5: Prove the bound against both lowerings.** Write the argument in compiler.md's "What a callee leaves": which `K` and `C` every body the optimized and the reference lowering can emit stays within, from the scratch allocator's discipline (`compiler/lower.rs`), the reference lowering's no-reuse rule, and the slot placement. Then add two tests:
  - `every_golden_body_stays_within_the_bound`, in `crates/ns-runtime/tests/`. It compiles every golden, the core library and `benchmarks/` under both lowerings, asserts `analyze` is `Ok`, and prints the largest work/instructions ratio.
  - `the_deepest_nesting_stays_within_the_bound`. It compiles the program the proof names as the worst case, for example a 2,000-deep right-nested `+` around 2,000 inlined conditionals, under both lowerings.

  **Stop rule:** if no linear bound holds for compiled code (the proof fails, or the worst-case program exceeds it), stop. Commit nothing beyond Step 3 and report to the controller, which reports to the user. The fill then stays.
- [ ] **Step 6: Run** `cargo nextest run -p ns-runtime leftovers every_golden_body the_deepest_nesting`. They pass.
- [ ] **Step 7: Commit** `feat: the leftovers a callee may leave are derived from the code`. Gates: `commit`, `images`.

### Task 2: Compiled bodies are checked by the analysis

**Files:**
- Modify: `crates/ns-runtime/src/compiler/reference.rs` and `compiler/lower.rs`, the calls to `check_registers_above_sends_are_dead`. The function is deleted; every caller calls `leftovers::analyze` and `debug_assert!`s `Ok`.
- Docs: `docs/design/compiler.md`, "Sends". The dead-above invariant is now checked over the control flow, back edges included.

**Interfaces:** consumes Task 1's `analyze`.

- [ ] **Step 1: Write the failing test** `a_loop_reading_what_its_body_sent_over_panics_in_debug`, in `compiler/` tests. A hand-built body whose loop head reads a register a send in the loop's body tainted makes the debug check panic with `internal:`. Today's in-order check misses it.
- [ ] **Step 2: Run it.** It fails, because no panic occurs.
- [ ] **Step 3: Replace the check** in both lowerings.
- [ ] **Step 4: Run** `cargo xtask gates commit`. Every golden passes under both lowerings.
- [ ] **Step 5: Commit** `refactor: both lowerings check what a callee leaves over the control flow`.

### Task 3: The decoder refuses a read of a callee's leftovers

**Files:**
- Modify: `crates/ns-runtime/src/image/decode.rs`: `verify_body`, and a new `DecodeError::LeftoverRead { .. }` and `DecodeError::VerificationPastTheBound { .. }`, each with its message.
- Docs: `docs/design/image-format.md`, "Verification"; `docs/design/memory.md`, the amplification bound, with the analysis's peak.
- Test: `crates/ns-runtime/tests/`, where the decoder's other crafted-image refusals live.

**Interfaces:** consumes Task 1's `analyze`.

- [ ] **Step 1: Write the failing tests.**
  - `an_image_reading_the_mirror_a_kernel_method_left_is_refused`: an image whose method sends to a shipped method that runs `SendMirror`, then `Return`s the register that held the mirror. Loading it answers `LeftoverRead`.
  - `an_image_past_the_verification_bound_is_refused`: Task 1's crafted body inside an image answers `VerificationPastTheBound`, and its peak allocation stays within the bound memory.md states.
- [ ] **Step 2: Run them.** Both fail, because the image loads.
- [ ] **Step 3: Call `analyze`** from `verify_body` and map its refusals. The analysis is computed per body and dropped before the next one.
- [ ] **Step 4: Run** `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** `feat: the decoder refuses code that reads what a callee left`.

### Task 4: The collector's pre-pass

**Files:**
- Modify:
  - `crates/ns-runtime/src/bytecode.rs`: `CompiledCode` gets a lazily filled `std::cell::OnceCell<Leftovers>` and `pub(crate) fn leftovers(&self) -> &Leftovers`. For shipped code, which is not verified, it computes and `expect`s.
  - `crates/ns-runtime/src/heap.rs`: a new `fn nil_leftovers(&mut self)`. It is called first in every trace of the stack: the scavenge (`~7838`), marking (`begin_marking`, `~8526`), forwarding (`forward_everything`, `~8313`) and the between-entries collections.
- Docs: `docs/design/memory.md`, "What the collector traces".

**Interfaces:** consumes `CompiledCode::leftovers` and `Leftovers::tainted_before`.

**What the pre-pass nils.** It walks the frames from the top by the caller word.
- The top frame: `tainted_before(program counter)`.
- Every other frame F, in flight at the send whose slots end at `end`: `tainted_before(the send's index)` below `end`. On top of that, `[max(end, callee window end), F's window end)` when the callee lay on F's slots.

Which instruction a stored program counter names (the send itself, or the one after it) is read from `vm.rs`'s write-back (`activation_set_program_counter`, `~1265`) and the switch. It is stated in the function's doc comment. A frame at an instruction that runs again takes the set before that instruction.

In a debug build it writes `0x5EAD_5EAD` (`dead_register`), in release nil.

- [ ] **Step 1: Write the failing heap tests.**
  - `a_collection_nils_what_a_returned_callee_left`: a frame whose register above a finished send's slots holds a fresh object, with a weak array naming it. After a scavenge the weak slot is nil.
  - `a_collection_keeps_a_register_written_after_the_send`.
  - `a_collection_keeps_the_named_locals`.
  - `a_frame_in_flight_keeps_its_registers_below_its_slots`.
  - `a_loop_head_keeps_what_it_reads` (Review Focus 1).
  - `a_rerun_instruction_takes_the_set_before_it` (Review Focus 2).

  Each builds its frames through the heap's push API and the code through a hand-built `CompiledCode`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime nil_leftovers a_collection`. They fail.
- [ ] **Step 3: Implement** the cache and `nil_leftovers`, with a call before every trace.
- [ ] **Step 4: Run** `cargo xtask gates commit codegen`. The pop's fill still exists, so the goldens are unchanged.
- [ ] **Step 5: Commit** `feat: a collection nils what returned callees left in their senders`.

### Task 5: The return writes nothing

**Files:**
- Modify:
  - `crates/ns-runtime/src/heap/frames.rs`: `push_from` never lowers the top below the current top. `set_top_over_dead_registers` is replaced by a plain top restore.
  - `crates/ns-runtime/src/heap.rs`: `take_off_words` and `top_without` restore the top the caller word records. Their debug assertion becomes `>=` the caller's window end. `suspend_activations` applies `tainted_before` to the copy, and `divorce` nils every scratch register (`>= local_count`) in the copy (Q3).
- Docs:
  - `docs/design/execution.md`, the overlap's sections: the pop writes nothing, and the top is kept at a live window end.
  - `CLAUDE.md`: the dead-above invariant gets a paragraph in "Invariants that are easy to break". It covers what reads it (the decoder, the debug check), what nils it (the pre-pass, before every trace), and the defect class a missed trace would be: silent retention.

**Interfaces:** consumes Task 4's `nil_leftovers`.

- [ ] **Step 1: Change the heap unit tests that pin the fill**. `heap.rs`'s overlapped-pop tests assert that the words above the callee are untouched after the pop and that the top equals the caller's window end. Run them; they fail.
- [ ] **Step 2: Implement** the push, the pop, suspend and divorce changes.
- [ ] **Step 3: Run** `cargo xtask gates commit images caches release-checks codegen stress`. Every golden is unchanged under both lowerings, including the 12 under `goldens/library/weak/`, and every stress mode passes.
- [ ] **Step 4: Commit** `perf: a return writes nothing into its sender's registers`.

### Task 6: Measured, kept or reverted

**Files:** `docs/design/execution.md`, the measured result; `docs/OPEN-WORK.md`.

- [ ] **Step 1: Run `cargo xtask gates end-of-batch`** at Task 5's head.
- [ ] **Step 2: Measure** Task 5's head against the head before Task 1. Use three pinned builds a side (a, b, c, flags as in `.superpowers/sdd/2026-10-24-2054-register-windows/task-3-measure-report.md`), all 24 programs, two sittings, pooled, the protocol of `task-8-report.md`. Measure stage 1 plus fix 5 (`.temp/targets/head2-forget-*`) as a third column, for information.
- [ ] **Step 3: Decide.** B is kept if no program is slower with its builds separated **and** the geometric mean over the 24 programs is faster. Otherwise revert Tasks 1-5 with `git revert` and record the measured cost of the fill in execution.md as the price of its three guarantees: no stale word, no retention, and the capability barrier.
- [ ] **Step 4: Commit** `docs: what the leftover analysis measured`.
