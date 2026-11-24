# Leftover Bitsets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Neither a push nor a return in road step 7a′'s overlapped frames fills scratch registers. A collector pre-pass nils what is dead before every trace, the decoder refuses image code that reads it, and stated construct limits bound the analysis that decides it.

**Architecture:** This plan supersedes `2026-10-25-1248-leftover-analysis.md` from its Task 1 on. That plan's undo-log analysis (`1f1e3033`, `8b1b6f2a`) is replaced in place, and its `2091ed43` (register effects in `bytecode.rs`) stays.

One forward pass per body computes the *tainted* registers: the scratch registers (`>= local_count`) that may hold a word the body did not write.
- **At entry:** every scratch register is tainted. The push no longer fills them.
- **At a send other than `SendMirror`:** the registers from the send's slot end up are tainted. A send writes none of its slots (`vm.rs`'s `argument_window`).
- **At a write:** the written register is clean.

Each open forward-jump target holds one bitset over the body's registers, and a jump ORs its state into it. Loops take the floor rule. The construct limits make this exact pass constant in memory and linear in work.

Three readers use it:
- the decoder, which refuses a read of a tainted register;
- a debug check over every compiled body, under both lowerings;
- the collector's pre-pass, which nils the tainted registers of every frame before every trace.

The top never drops below a live window end. In a debug build, dead registers hold a poison word that no value can be, and a register read that answers it panics.

**Tech Stack:** Rust 1.98.0, edition 2021. Crates `ns-runtime` (`#![forbid(unsafe_code)]`) and `ns-syntax` only for the parser's existing `MAX_DEPTH`.

**Spec:** `.superpowers/sdd/2026-10-24-2054-register-windows/design-b-memo.md`, option (iv), with the user's decisions at the end of that workspace's `progress.md`, and these from 2026-10-25, recorded in `.superpowers/sdd/2026-10-25-1248-leftover-analysis/progress.md`:
- **The analysis:** option A, an exact bitset dataflow with clear limits. The measurements behind it are in the same workspace: `declared-states-report.md`, `fill-profile-report.md` and `limits-report.md`.
- **Limits:**
  - **Kept:** the parser's `MAX_DEPTH` 128; registers per body 65,535.
  - **New:** instructions per body 2^20; parameters per method, block or factory, and arguments per send, 255; `<:` clauses per superclass clause 1,024.
  - **No limit:** literals, slots, cascades, chains, strings.
- **The push** stops filling scratch; only named locals are filled nil.
- **A debug poison net** checks the VM against the analysis dynamically.
- **The decoder's refusal** past a bound is acceptable only where no compiled body can reach it.
- **The measurement** keeps or reverts the performance part, as before.

## Global Constraints

- **Invariants:** `CLAUDE.md`'s all hold. Especially:
  - an image is untrusted input;
  - the amplification bound (`docs/design/memory.md`, "The amplification bound of a rejected load");
  - compiling a resolved module never raises (a limit past which a body cannot run is a deferred program-level error, as `Literal::BodyTooLarge` is);
  - the value-register invariant;
  - wire tags.
- **No wire change:** no new tag, no new record. New `DecodeError` variants are allowed.
- **Nil, never skip:** a dead register is written nil (the poison in a debug build) before every trace. It is never left untraced.
- **Named locals are always live:** registers below `local_count` are never tainted, and the push fills them nil.
- **Lowerings:** both lowerings' output passes the verifier (`image/encode.rs:358`). Every golden stays unchanged under both.
- **Names written out, conventional commits.** Gates per commit:
  - always `commit`;
  - `images` for `image/` or `bytecode/`;
  - `codegen` for the hot-path files `CLAUDE.md` names;
  - `release-checks` for a type a size test pins.

  `end-of-batch` runs once, in Task 7. Commit trailer as the dispatch gives it.
- **After this plan:** the register-windows plan's Task 8 Step 4 (docs, PROGRESS) follows, then the final review.

## Review Focus

1. **A collection while the top frame stands at a backward jump inside a loop whose body sends.** The loop's leftovers are nilled, and what the loop head reads is kept. Task 5.
2. **A frame stopped at an instruction that runs again** (`MakeFuture`/`ResolveFuture`, a resumed `resume:`/`retry`/`return:`). It takes the set before that instruction. Task 5.
3. **A frame freshly pushed whose scratch holds words from frames popped before a collection moved their objects.** The pre-pass nils them before the trace, so no stale handle is traced. Task 6, stress modes.
4. **A crafted image reading the register a shipped method's `SendMirror` kept the mirror in.** Refused at load. Task 4.
5. **A recursion at the 100,000-activation depth limit, with a scavenge per allocation.** The pre-pass's cost stays proportional to tracing. Task 7 measures it.

---

### Task 1: The construct limits

**Files:**
- Modify:
  - `crates/ns-runtime/src/compiler/mod.rs` and `compiler/lower.rs`, `compiler/reference.rs`. Past either limit below, a body is `Literal::BodyTooLarge`, with a reason that its raise message names. The limits:
    - instructions per body over 2^20; this replaces `Lowerer::here`'s `u32` panic (`lower.rs:1924`) and the compact builder's (`bytecode/compact.rs:875`, `:899`);
    - parameters of a method, block or factory over 255, or arguments of a send over 255.
  - The resolver (`resolver/classes.rs:137`, `mixin.rs:567`): `CLAUSE_INDEX_LIMIT` becomes 1,024, one resolve error.
  - `crates/ns-runtime/src/image/decode.rs`: refuse each of the three, where the count is read and before anything is reserved for it. Use new `DecodeError` variants with messages.
- Docs:
  - `docs/design/compiler.md`: a section "Limits", one line per construct with its limit, its enforcement and the measured maximum. Values from `limits-report.md`'s table: the five above, plus "no limit" for literals, slots, cascades, chains and strings, with the reason.
  - `docs/design/image-format.md`, "Verification".
  - `docs/OPEN-WORK.md`. The compiler's chain recursion (overflow at about 20,000 links) sits below the 2^20 limit, and the fallback duplication of nested inlined conditionals can reach the limit from small source. Add each to its existing item, saying the limit is not yet the real one there.

**Interfaces:**
- Produces, in `bytecode`:
  - `pub(crate) const INSTRUCTION_LIMIT: u32 = 1 << 20;`
  - `pub(crate) const ARGUMENT_LIMIT: u16 = 255;`

  Both are named in compiler.md. The clause limit stays in the resolver as `CLAUSE_INDEX_LIMIT`.

- [ ] **Step 1: Write the failing tests**:
  - `a_method_of_256_parameters_raises_when_run`
  - `a_send_of_256_arguments_raises_when_run`
  - `a_method_of_255_parameters_runs`
  - `a_superclass_clause_of_1025_mixins_is_refused_at_resolve`
  - `an_image_with_a_body_past_the_instruction_limit_is_refused`
  - `an_image_with_a_method_of_256_parameters_is_refused`

  Each source is an `indoc!`, built by `formatdoc!` for the wide ones. A raise asserts the message names the limit. The instruction limit's compile side gets a unit test on the lowerer's counter, `a_body_past_the_instruction_limit_is_too_large`, since a 2^20-instruction source is too slow for the suite.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement** the three limits on both sides.
- [ ] **Step 4: Run** `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** `feat: instructions, arguments and mixin clauses have stated limits`.

### Task 2: The analysis over bitsets

**Files:**
- Modify:
  - `crates/ns-runtime/src/bytecode/leftovers.rs` and `bytecode/leftovers/tests.rs`: the undo-log machinery is replaced. Kept: the explicit-set fixpoint oracle, the crafted-body generators, `every_golden_body_stays_within_the_bound` with its skip accounting, and the flat N=2000 and 125-deep loop cases. Read them at `8b1b6f2a`.
  - `crates/ns-runtime/src/bytecode.rs`: `successors`/`falls_through` stay if used.
- Docs: `docs/design/compiler.md`, "What a callee leaves", rewritten: the rule, the entry state, the floor rule, and the bound.

**Interfaces:**
- Produces, in `bytecode::leftovers`:
  - `pub(crate) fn check(instructions: &[Instruction], local_count: u16, register_count: u16) -> Result<(), LeftoverRefusal>`, which keeps nothing.
  - `pub(crate) fn tainted_at(instructions: &[Instruction], local_count: u16, register_count: u16, program_counters: &[u32]) -> Vec<Vec<std::ops::Range<u16>>>`: the tainted set before each given program counter (sorted ascending), as sorted disjoint ranges, from one pass. It is called only on a body `check` accepted.
  - `pub(crate) enum LeftoverRefusal { Read { instruction: u32, register: u16 }, PastTheBound { instruction: u32 } }`.

**The pass.** It runs once, in instruction order, over a current bitset of width `register_count` rounded up to 64 bits.
- **Entry:** `[local_count, register_count)` is tainted.
- **A send** other than `SendMirror`: `[slot end, register_count)` is tainted, the slots unchanged.
- **A write:** the register is cleaned.
- **A read** of a tainted register: `Read`.
- **A forward jump** ORs the current state into its target's pending bitset, allocated on first use. More than 257 pending at once is `PastTheBound`; 2 × 127 + 3 is derived from `MAX_DEPTH`, and the derivation is in compiler.md.
- **Reaching a pending target:** the current state is ORed with the pending bitset, or replaced by it when the previous instruction does not fall through. The pending bitset is then freed.
- **A loop head** (the target of a backward jump):
  - Its state is the entry state ORed with `[floor, register_count)`, where `floor` is the least slot end of a non-mirror send between the head and its last back edge.
  - In a debug build, every back edge asserts its state is within the head's.
  - Loops must nest, and a jump into a loop other than at its head is `PastTheBound`. The lowering never emits either; Step 5 checks that.
- **A body over `INSTRUCTION_LIMIT`:** `PastTheBound` before anything is allocated.

**The bound,** proven in compiler.md:
- **Memory:** at most 258 bitsets of `ceil(register_count / 64)` words, at most 2.02 MiB per body, freed with it.
- **Work:** at most `3 × ceil(register_count / 64) + c` word operations per instruction.
- **Real code:** state the measured largest figures from Step 5.

- [ ] **Step 1: Write or adapt the failing unit tests:**
  - `scratch_is_tainted_at_entry`: with `local_count` 6 and `register_count` 10, a read of r7 before any write is `Read`.
  - `a_send_taints_from_its_slot_end_and_writes_none_of_its_slots`
  - `a_mirror_send_taints_nothing`
  - `named_locals_are_never_tainted`
  - `a_join_is_tainted_where_either_arm_is`
  - `crossing_forward_jumps_join_exactly`
  - `a_loop_head_takes_the_floor`
  - `a_jump_into_a_loop_past_its_head_is_past_the_bound`
  - `more_than_257_pending_targets_are_past_the_bound`
  - `tainted_at_answers_each_program_counter`

  Also the oracle comparison: exact on loop-free bodies, a superset with loops, over 4,000 crafted bodies and every compiled body up to 400 instructions.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime leftovers`. The new ones fail.
- [ ] **Step 3: Implement** the pass. Remove the undo log, `Leftovers`, `tainted_before` and `tainted_before_each`.
- [ ] **Step 4: Run** the tests. They pass.
- [ ] **Step 5: Run `every_golden_body_stays_within_the_bound`** under both lowerings, with the entry taint. No compiled body is refused, and no loop-nesting or jump-into-loop refusal fires.

  **Stop rule:** if a valid compiled body reads a register tainted at entry, stop and report BLOCKED with the body. A scratch register read before it is written would mean the push must keep filling it. Report the largest measured memory and work per instruction.
- [ ] **Step 6: Run** `cargo xtask gates commit images`.
- [ ] **Step 7: Commit** `refactor: the leftover analysis is one pass over bitsets`.

### Task 3: Compiled bodies are checked by the analysis

**Files:**
- Modify: `crates/ns-runtime/src/compiler/reference.rs` and `compiler/lower.rs`. `check_registers_above_sends_are_dead` is deleted. Every caller `debug_assert!`s `leftovers::check(..)` is `Ok`, with an `internal:` message naming the refusal.
- Docs: `docs/design/compiler.md`, "Sends". The dead-above invariant is checked over the control flow, back edges and the entry state included.

**Interfaces:** consumes Task 2's `check`.

- [ ] **Step 1: Write the failing test** `a_loop_reading_what_its_body_sent_over_panics_in_debug`, in the compiler's tests. A hand-built body whose loop head reads a register a send in the loop tainted panics with `internal:`. Today's in-order check misses it.
- [ ] **Step 2: Run it.** It fails.
- [ ] **Step 3: Replace the check** in both lowerings.
- [ ] **Step 4: Run** `cargo xtask gates commit`.
- [ ] **Step 5: Commit** `refactor: both lowerings check what a callee leaves over the control flow`.

### Task 4: The decoder refuses a read of what a body did not write

**Files:**
- Modify: `crates/ns-runtime/src/image/decode.rs`. `verify_body` calls `leftovers::check` and maps its refusals to new `DecodeError::LeftoverRead { .. }` and `DecodeError::VerificationPastTheBound { .. }`, each with its message.
- Docs:
  - `docs/design/image-format.md`, "Verification".
  - `docs/design/memory.md`, "The amplification bound of a rejected load": the analysis adds a fixed term of at most 2.02 MiB, held for one body at a time and before the graph pass (`image/mod.rs:551` against `:603`). Update `CLAUDE.md`'s amplification paragraph to name the term.
- Test: `crates/ns-runtime/tests/`, beside the decoder's other crafted-image refusals.

**Interfaces:** consumes Task 2's `check`.

- [ ] **Step 1: Write the failing tests:**
  - `an_image_reading_the_mirror_a_kernel_method_left_is_refused`: the image's method sends to a shipped method that runs `SendMirror`, then `Return`s the register that held the mirror. Loading answers `LeftoverRead`.
  - `an_image_reading_unwritten_scratch_is_refused`
  - `an_image_past_the_verification_bound_is_refused`: 258 pending targets.
- [ ] **Step 2: Run them.** They fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** `feat: the decoder refuses code that reads what it did not write`.

### Task 5: The collector's pre-pass

**Files:**
- Modify:
  - `crates/ns-runtime/src/bytecode.rs`: `CompiledCode` gets a per-code cache of tainted sets, keyed by program counter and filled lazily from `tainted_at`. Shipped code is unverified, so it is `check`ed once and `expect`ed.
    - If `CompiledCode` must stay `Sync`, or a cell does not fit it, keep the cache in the heap's compiled-code table (`heap/tables.rs`) instead, keyed by the code's id.
    - Per-target bitsets are never kept.
  - `crates/ns-runtime/src/heap.rs`: a new `fn nil_leftovers(&mut self)`. It runs first in every trace of the stack: the scavenge, marking (`begin_marking`), forwarding (`forward_everything`) and the between-entries collections. It walks the frames from the top by the caller word:
    - **The top frame:** the set before its program counter.
    - **Every other frame F**, in flight at the send whose slots end at `end`: the set before that send, below `end`. Plus `[max(end, callee window end), F's window end)` when the callee lay on F's slots.

    It writes nil, or the poison word in a debug build. Which instruction a stored program counter names is read from `vm.rs`'s write-back and stated in the doc comment. A frame at an instruction that runs again takes the set before it.
- Docs: `docs/design/memory.md`, "What the collector traces".

**Interfaces:** consumes Task 2's `tainted_at`.

- [ ] **Step 1: Write the failing heap tests:**
  - `a_collection_nils_what_a_returned_callee_left`: a weak array names an object held only in a register above a finished send's slots. After a scavenge its slot is nil.
  - `a_collection_keeps_a_register_written_after_the_send`
  - `a_collection_keeps_the_named_locals`
  - `a_frame_in_flight_keeps_its_registers_below_its_slots`
  - `a_loop_head_keeps_what_it_reads` (Review Focus 1)
  - `a_rerun_instruction_takes_the_set_before_it` (Review Focus 2)
  - `a_collection_nils_unwritten_scratch`

  Each builds its frames through the heap's push API and its code as a hand-built `CompiledCode`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime a_collection nil_leftovers`. They fail.
- [ ] **Step 3: Implement** the cache and `nil_leftovers`, called before every trace.
- [ ] **Step 4: Run** `cargo xtask gates commit codegen`. Both fills still exist, so the goldens are unchanged.
- [ ] **Step 5: Commit** `feat: a collection nils what no frame wrote or still needs`.

### Task 6: Neither push nor return fills scratch

**Files:**
- Modify:
  - `crates/ns-runtime/src/heap/frames.rs`: `push_from` never lowers the top below the current top. `set_top_over_dead_registers` becomes a plain top restore in a release build. A debug build keeps the fill, with the poison word.
  - `crates/ns-runtime/src/heap.rs`:
    - `push_overlapped_activation` and `push_activation_in` fill only the named locals, `[argument count, local_count)` of the temporaries, as the layout decides. A debug build fills the scratch with the poison word.
    - `take_off_words` and `top_without` restore the top the caller word records, asserting `>=` the caller's window end.
    - `suspend_activations` nils the copy's tainted registers. `divorce` nils every scratch register of the copy.
    - The poison word becomes one no `Value` can be: a reserved tag pattern from `heap/value.rs`. Explain the choice in `dead_register`'s doc. `0x5EAD_5EAD` is a valid small integer, so it cannot be told from a program's value.
    - In a debug build, `Heap::register` and every other register read the run loop and the interpreter make `debug_assert!` that the word is not the poison, with an `internal:` message naming the frame and the register. This is the dynamic net that catches a VM that reads where the analysis says nothing reads.
- Docs:
  - `docs/design/execution.md`, the overlap's sections: neither push nor return fills scratch, the top stays at a live window end, and the poison net exists.
  - `CLAUDE.md`, "Invariants that are easy to break": a paragraph on the dead-above rule. It covers:
    - what reads it: the decoder, the debug check, the poison net;
    - what nils it: the pre-pass before every trace;
    - what a new instruction must do: declare its register effects in `bytecode.rs`'s exhaustive matches;
    - what a miss is: a stale handle traced, or retention.

**Interfaces:** consumes Task 5's `nil_leftovers`.

- [ ] **Step 1: Change the heap unit tests that pin the fills.**
  - After an overlapped pop in a release build, the words above the callee are untouched and the top equals the caller's window end.
  - A push leaves scratch unwritten in a release build.
  - A debug read of the poison panics.

  Run them; they fail.
- [ ] **Step 2: Implement** the push, pop, suspend, divorce and poison changes.
- [ ] **Step 3: Run** `cargo xtask gates commit images caches release-checks codegen stress`. Every golden is unchanged under both lowerings, the 12 under `goldens/library/weak/` included, and every stress mode passes. Check the codegen budgets of `push_sent`, `push_step`, `push_initializer_placed` and `pop_activation`, and lower them in `xtask/src/codegen.rs` where the change shrank them.
- [ ] **Step 4: Commit** `perf: neither a push nor a return fills scratch registers`.

### Task 7: Measured, kept or reverted

**Files:** `docs/design/execution.md`, the measured result; `docs/OPEN-WORK.md`.

- [ ] **Step 1: Run** `cargo xtask gates end-of-batch` at Task 6's head.
- [ ] **Step 2: Measure** Task 6's head against `78a17506`:
  - three pinned builds a side (a, b, c, flags as in `.superpowers/sdd/2026-10-24-2054-register-windows/task-3-measure-report.md`);
  - all 24 programs, in two sittings, pooled, by the protocol of `task-8-report.md`;
  - for information, a third column with stage 1 plus fix 5 (`.temp/targets/head2-forget-*`);
  - for Review Focus 5, a deep recursion under `NS_NEXT_STRESS_SCAVENGE`, the pre-pass's share of the run.
- [ ] **Step 3: Decide.**
  - **Kept** if no program is slower with its builds separated **and** the geometric mean is faster.
  - **Otherwise:** `git revert` Tasks 2-6, keeping Task 1's limits, and record the measured cost of the fills in execution.md as the price of their guarantees.
- [ ] **Step 4: Commit** `docs: what the leftover analysis measured`.
