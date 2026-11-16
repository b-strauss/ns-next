# The optimizing JIT's foundation (road step 9, stage 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A second, optimizing kind of machine code per body: an SSA graph built from a snapshot of the body, registers allocated by linear scan across the body, code emitted through the macro assembler and the baseline's own templates, installed at a safepoint beside the body's baseline code, triggered by a budget per body, with no speculation.

**Architecture:** A body that is hot -- its budget, which baseline code spends at every entry and back jump, runs out -- is snapshotted at the next safepoint, compiled synchronously from the snapshot alone and installed as the body's optimized code, which cells, pushes and machine entry rows then prefer; baseline code stays installed, since a frame that leaves optimized code continues in it. Optimized code keeps the values of bytecode registers in machine registers between interrupt points and writes every value it has not yet stored back into its frame word before each interrupt point -- every send, leaf call, exit, poll and closure -- so that at each one the frame is the interpreter's word for word and the existing exits, materialization, unwinder and collector see nothing new. Instructions that are not simple register arithmetic are emitted by the very template functions the baseline uses, after the write-back.

**Tech Stack:** Rust 1.98.0, `crates/ns-runtime/src/jit/`, `ns-assembler` (x64, AArch64), `cargo-nextest`, `cargo xtask gates`.

**Spec:** `docs/design/execution.md`, "Planned: the optimizing JIT" (and "Planned: the deoptimization contract", which it builds to). Read both before any task.

## Global Constraints

- Branch: `feature/optimizing-jit-foundation`, created from `feature/optimizing-jit-design` (or from `master` once that is merged); every commit on it.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`); every commit message ends with `Co-Authored-By: <the committing model's own name> <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_01J94EWdTAqndqD5KeQGQLia`.
- Every command runs in the foreground; long commands pass `timeout: 600000`.
- Gates per commit: `cargo xtask gates commit`; plus `codegen` when a commit touches `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs`; `release-checks` when it touches a type a size test pins; `memory` when it touches `crates/ns-memory`. The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, in Task 11, not per commit: commits before it are work in progress.
- Names are written out (no `pc`, `reg`, `interp`, `num_`); clippy's `min_ident_chars` holds it.
- No file under `jit/` but `macro_assembler.rs` names an architecture's register or encoder outside a test module (`tests/jit_architecture_split.rs`); a new register need is a named role.
- `ns-runtime` forbids `unsafe`.
- Comments explain the decision, not the mechanics.
- A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1`, from the interpreter.
- No new external dependency.
- Leaves keep their rule (CLAUDE.md, "Machine code is a lowering…"): no new leaf compiles or installs anything; the optimizing compile runs at the run loop's safepoint.
- Optimized code is a lowering of the interpreter's instructions: every instruction it does not lower as a simple node it emits through the baseline's template function for it, never a second implementation of it.

## Decisions this plan makes inside the design

These follow from the design and the code as it is; each is stated once here and relied on below.

1. **Optimized code lives beside baseline code, never in its place.** A body gets optimized code only once it has baseline code; `JitSlot` names both. Pushes, compiled calls through cells and machine entry rows enter the optimized prologue; a frame returned to (`jit::resumed`), switched at a back edge (`jit::at_back_edge`) or resumed after a safepoint enters the *baseline* code at its program counter, since only baseline code has an entry at every instruction. Leaving optimized code is therefore always "continue in baseline code or the interpreter", which the frame's words, written back, make exact ("Between the tiers": a baseline frame becomes interpreted in place). OSR into optimized code is stage 6.
2. **Every interrupt point is frame-consistent.** Before every send, leaf call, exit, poll and `MakeClosure`, optimized code stores every value it holds for a bytecode register and has not stored yet into that register's frame word. No store is ever left out, so that what the collector traces is what the baseline would have left (CLAUDE.md, "A register a frame does not read before it writes it is dead…": a store left out keeps an object alive a `WeakArray` would show). The interrupt points' metadata records, per point, where each live register's value is; in stage 1 that is always its frame word, and the verifier checks it.
3. **`MakeClosure` transfers into the baseline twin**, after the write-back, exactly as a lean baseline body does (`closures::transfer_to_twin`): optimized code never reifies its frame.
4. **The snapshot is the compiler's only input and is `Send + 'static`.** It holds `SnapshotInstruction`s -- `Instruction` with `MakeClosure`'s `Rc` payloads replaced by the block's `CodeId` and its parameter count -- and `BodyFacts`, the plain numbers the templates read off a `CompiledCode`. The baseline's template functions take `BodyFacts` instead of `&CompiledCode` where they took it (`sends::prologue`, `sends::call_slots`).
5. **The raw area waits for stage 5.** Nothing is unboxed before stage 5, and a raw area costs the collector a lookup per frame; it is built with the first unboxed value. (`execution.md`'s stage 1 list is corrected in Task 11.)
6. **The allocatable registers are a named role**, `Roles::allocatable`, ordered so that those that survive a leaf call (callee-saved and no role's) come first: x64 `r13, rbp, r12, rsi, rdi, r8, r9, r10`; AArch64 `x22, x23, x24, x25, x26, x27, x28, x3, x4, x5, x6, x7, x8`. Every frame node clobbers all of them (a send's callee may be optimized code using them; the values were written back and are reloaded); a leaf call inside a simple node clobbers all but `Roles::survive_leaf_calls`.

## Review Focus

1. **A frame that leaves optimized code anywhere resumes exactly** -- at an exit inside a template, a refused push, a safepoint poll, a cut that answers the exit, a materialization from a callee three frames up: the interpreter or baseline code must see every register the interpreter would. Test: Task 9's goldens and `NS_NEXT_STRESS_DEOPT` over the suite.
2. **The debug poison and the leftover analysis stay exact**: optimized code must refuse to read a `DEAD_REGISTER` word as `load_frame_register` does, and must never store a register the baseline would not have stored. Test: Task 5's `an_optimized_body_refuses_a_dead_register` and the leftover verifier in Task 4.
3. **The depth limit stays exact with optimized frames**: the goldens pin where a recursion overflows. Test: Task 9's `deep_recursion_optimized.ns` beside the existing depth goldens under `NS_NEXT_STRESS_OPTIMIZE`.
4. **The code zone compacts, slides and frees optimized bodies as it does baseline ones**, and a stale cell or machine entry row never names freed optimized code. Test: Task 7's `a_compaction_slides_optimized_code` and the suite under `NS_NEXT_STRESS_ZONE` with `NS_NEXT_STRESS_OPTIMIZE`.
5. **AArch64 runs the same**: the allocator's roles and the new idioms on AAPCS64. Test: Task 5's cross-assembly tests and the `aarch64` gate in Task 11.

---

### Task 1: The snapshot and the body's facts

**Files:**
- Create: `crates/ns-runtime/src/jit/optimizing/mod.rs`, `crates/ns-runtime/src/jit/optimizing/snapshot.rs`
- Modify: `crates/ns-runtime/src/jit/mod.rs` (declare `mod optimizing;`), `crates/ns-runtime/src/jit/sends.rs:92` (`prologue`), `crates/ns-runtime/src/jit/sends.rs:397` (`call_slots`), `crates/ns-runtime/src/jit/baseline.rs:1048` (`compile` passes `BodyFacts`), every caller of the two
- Test: `crates/ns-runtime/src/jit/optimizing/snapshot.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub(crate) struct BodyFacts { pub(crate) argument_count: u16, pub(crate) local_count: u16, pub(crate) register_count: u16, pub(crate) kind: CodeKind, pub(crate) family_size: u32, pub(crate) copied_count: u16, pub(crate) addresses_own_slots: bool, pub(crate) declaration_code: bool, pub(crate) unwind_marker: bool, pub(crate) frame_register_count: usize }` with `impl BodyFacts { pub(crate) fn of(code: &CompiledCode) -> BodyFacts }` -- in `jit/optimizing/snapshot.rs`, re-exported for `sends.rs`.
  - `pub(crate) enum SnapshotInstruction` -- one variant per `Instruction` variant, same fields, but `MakeClosure { destination: u16, block: CodeId, parameter_count: u16, copied: Vec<u16> }`.
  - `pub(crate) struct Snapshot { pub(crate) code_id: CodeId, pub(crate) facts: BodyFacts, pub(crate) feedback_base: FeedbackBase, pub(crate) instructions: Vec<SnapshotInstruction> }` with `pub(crate) fn take(code: &CompiledCode, heap: &Heap, feedback_base: FeedbackBase) -> Snapshot`.
  - `fn sends::prologue(body: &mut BodyAssembler, facts: &BodyFacts)`, `fn sends::call_slots(facts: &BodyFacts, instruction: &SnapshotInstruction) -> …` (the same answer as today, from the snapshot form; the baseline converts its `Instruction` through `SnapshotInstruction::of`).
  - `impl SnapshotInstruction { pub(crate) fn of(instruction: &Instruction, heap: &Heap) -> SnapshotInstruction }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_snapshot_crosses_threads() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<Snapshot>();
}

#[test]
fn a_snapshot_holds_the_body_s_instructions_in_order() {
    // A body of LoadSelf, IntegerBinary Add, Return, built with CompiledCode::new
    // as baseline.rs's tests build theirs.
    let snapshot = Snapshot::take(&code, &heap, FeedbackBase::NOT_ASSIGNED);
    assert_eq!(snapshot.instructions.len(), code.instruction_count());
    assert!(matches!(snapshot.instructions[1], SnapshotInstruction::IntegerBinary { operation: IntegerOperation::Add, .. }));
    assert_eq!(snapshot.facts.local_count, code.local_count);
}

#[test]
fn a_snapshot_names_a_closure_s_block_by_its_code_id() {
    // A body whose second instruction is MakeClosure of a one-parameter block.
    let snapshot = Snapshot::take(&code, &heap, FeedbackBase::NOT_ASSIGNED);
    let SnapshotInstruction::MakeClosure { block, parameter_count, .. } = &snapshot.instructions[1] else { panic!() };
    assert_eq!(*block, heap.code_id(&block_code));
    assert_eq!(*parameter_count, 1);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime snapshot`
Expected: FAIL to compile (`Snapshot` not defined).

- [ ] **Step 3: Implement `BodyFacts`, `SnapshotInstruction`, `Snapshot::take`; move `prologue` and `call_slots` onto `BodyFacts` and `SnapshotInstruction`; the baseline builds its facts with `BodyFacts::of` and its instructions with `SnapshotInstruction::of` where those two functions need them**

The heap's code id table answers the block's `CodeId` (the call `heap.code_id` stands for whatever `heap/tables.rs` names it; use the existing accessor).

- [ ] **Step 4: Run the tests and the JIT's suite**

Run: `cargo nextest run -p ns-runtime snapshot` then `cargo nextest run -p ns-runtime --test jit`
Expected: PASS; no baseline test changes.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: the optimizing compiler's snapshot of a body"`.

---

### Task 2: The budget per body

**Files:**
- Create: `crates/ns-runtime/src/heap/optimization_budgets.rs` (modelled on `heap/machine_entries.rs`)
- Modify: `crates/ns-runtime/src/heap.rs` (hold and publish it), `crates/ns-runtime/src/heap/machine_words.rs` (two words), `crates/ns-runtime/src/jit/sends.rs` (`prologue` spends), `crates/ns-runtime/src/jit/baseline.rs` (`jump_to` and `emit_backward_branches` spend at a back jump), `crates/ns-runtime/src/jit/macro_assembler.rs` (the idiom), `crates/ns-runtime/src/jit/leaves.rs` (the budget leaf), `crates/ns-runtime/src/jit/mod.rs` (requests, statistics), `crates/ns-runtime/src/interpreter/inline_cache.rs` (a fill raises the budget)
- Test: `crates/ns-runtime/tests/jit.rs`, `crates/ns-runtime/src/heap/optimization_budgets.rs`

**Interfaces:**
- Consumes: `BodyFacts` (Task 1).
- Produces:
  - `pub(crate) struct OptimizationBudgets` with `new()`, `fn arm(&mut self, row: CodeId, budget: i64) -> bool`, `fn raise(&mut self, row: CodeId, at_least: i64)`, `fn park(&mut self, row: CodeId)`, `fn budget(&self, row: CodeId) -> i64`, `fn address(&self) -> u64`, `fn clear(&mut self, row: CodeId)`, `fn shrink(...)` as `MachineEntries` has; one `i64` word per `CodeId`, committed on demand.
  - `MachineWord::OptimizationBudgets` and `MachineWord::OptimizationBudgetRows`.
  - `MacroAssembler::spend_budget(&mut self, code_id: CodeId, amount: u32, program_counter: u32)`: subtract `amount` from the body's row; if the result is negative, a cold path calls `Leaf::Budget` with `[code_id, program_counter]` and continues (the leaf answers nothing to re-execute). Bodies whose `CodeId` row lies past `OptimizationBudgetRows` skip the spend.
  - `Leaf::Budget`: records `(code_id)` in `JitState::optimization_requests: Vec<Weak<CompiledCode>>` once, parks the row, requests a safepoint, counts `JitStatistics::optimization_requests`.
  - Constants in `jit/mod.rs`: `pub(crate) const OPTIMIZATION_BUDGET: i64 = 120_000;` (instructions executed before a body is optimized; measured in Task 10), `pub(crate) const BUDGET_AFTER_CACHE_CHANGE: i64 = 20_000;`.
  - `JitStatistics` gains `optimization_requests`, `optimized_bodies`, `optimized_exits`, `abandoned_optimized_bodies`, `optimize_nanoseconds` (and `tests/jit.rs`'s `since` lists them).

- [ ] **Step 1: Write the failing tests**

```rust
// tests/jit.rs
ns_runtime::for_each_lowering! {
    fn a_hot_loop_requests_its_body_s_optimization(lowering) {
        // `run` counts to 200_000 in a whileTrue: loop.
        let (_, statistics) = run_compiled(COUNTING_LOOP, lowering, true);
        assert_eq!(statistics.optimization_requests, 1);
    }
}

ns_runtime::for_each_lowering! {
    fn a_body_run_once_requests_nothing(lowering) {
        let (_, statistics) = run_compiled(RETURN_THREE, lowering, true);
        assert_eq!(statistics.optimization_requests, 0);
    }
}

// heap/optimization_budgets.rs
#[test]
fn a_raise_never_lowers_a_budget() {
    let mut budgets = OptimizationBudgets::new();
    budgets.arm(CodeId::from_index(3), 50_000);
    budgets.raise(CodeId::from_index(3), 20_000);
    assert_eq!(budgets.budget(CodeId::from_index(3)), 50_000);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime -E 'test(/optimization_requests|requests_nothing|never_lowers/)'`
Expected: FAIL (no field `optimization_requests`).

- [ ] **Step 3: Implement the table, the two machine words, `spend_budget` (prologue: the body's instruction count; back jump: the distance in instructions), the leaf, the arming when a body's baseline code is installed (`OPTIMIZATION_BUDGET`), the raise in the cache fill after a miss (`BUDGET_AFTER_CACHE_CHANGE`), the clearing wherever a `CodeId` is freed or the machine entry row is cleared**

- [ ] **Step 4: Run the tests, then look at the prologue's and the back edge's code**

Run: `cargo nextest run -p ns-runtime --test jit`, then disassemble one compiled body (the `Machine` harness's bytes through `iced-x86` in a test, as `macro_assembler.rs`'s tests do) and confirm the spend is a memory subtract and one branch on x64.
Expected: PASS; the spend is two instructions on the hot path.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit` (and `codegen` if `heap.rs` changed), then `git commit -m "feat: a budget per body that baseline code spends"`.

---

### Task 3: The SSA graph and its builder

**Files:**
- Create: `crates/ns-runtime/src/jit/optimizing/graph.rs`, `crates/ns-runtime/src/jit/optimizing/build.rs`, `crates/ns-runtime/src/jit/optimizing/verify.rs`
- Test: in-file `#[cfg(test)] mod tests` of `build.rs` and `verify.rs`

**Interfaces:**
- Consumes: `Snapshot`, `SnapshotInstruction` (Task 1).
- Produces:
  - `pub(crate) struct Graph { pub(crate) blocks: Vec<Block>, pub(crate) values: Vec<ValueDefinition> }`, `pub(crate) struct BlockId(u32)`, `pub(crate) struct ValueId(u32)`.
  - `pub(crate) struct Block { pub(crate) program_counter: u32, pub(crate) phis: Vec<Phi>, pub(crate) nodes: Vec<Node>, pub(crate) end: End, pub(crate) predecessors: Vec<BlockId>, pub(crate) loop_head: bool }`; `pub(crate) struct Phi { pub(crate) register: u16, pub(crate) value: ValueId, pub(crate) inputs: Vec<ValueId> }`.
  - `pub(crate) enum Node`:
    - `FrameRead { register: u16, value: ValueId }` -- the register's value as its frame word holds it (at entry and after a frame node).
    - `Constant { value: ValueId, word: u64 }` -- an immediate literal, `self` is not a constant.
    - `Receiver { value: ValueId }`.
    - `Copy { destination_register: u16, source: ValueId }` -- binds the register to the value; emits nothing.
    - `IntegerBinary { operation: IntegerOperation, destination_register: u16, value: ValueId, left: ValueId, right: ValueId, fallback: BlockId }` -- the `SmallInteger` fast path; `fallback` is the block holding the instruction's fallback send.
    - `Frame { program_counter: u32, instruction: SnapshotInstruction, reads: Vec<(u16, ValueId)>, written: Option<u16> }` -- every other instruction: its operands written back, the baseline's template, its result in its frame word.
  - `pub(crate) enum End { Jump(BlockId), Branch { condition: ValueId, if_true: BlockId, if_false: BlockId, not_boolean: BlockId }, Return { value: ValueId }, Frame /* the block ends in a frame node that does not fall through: a return, a non-local return, an exit */ }` -- a backward `Jump` or `Branch` edge is a poll.
  - `pub(crate) fn build(snapshot: &Snapshot) -> Graph` -- one pass over the instructions in order, blocks at every jump target and after every branch, each register mapped to the value that last defined it (`Vec<Option<ValueId>>` per block), phis at merges where predecessors disagree, loop heads given a phi for every register live in (filled when the back edge is seen).
  - `pub(crate) fn verify(graph: &Graph) -> Result<(), VerifyError>`: every value defined once, every use dominated by its definition, every phi has one input per predecessor, every block reachable from block 0 has an end. Called after `build` in debug builds (`debug_assert!` on `verify(&graph).is_ok()`).

- [ ] **Step 1: Write the failing tests** (`build.rs`)

```rust
#[test]
fn a_move_binds_a_register_to_its_source_s_value() {
    // LoadSelf r3; Move r4 <- r3; Return r4
    let graph = build(&snapshot);
    let End::Return { value } = graph.blocks[0].end else { panic!() };
    assert!(matches!(graph.definition(value), Node::Receiver { .. }));
}

#[test]
fn a_loop_head_takes_a_phi_for_the_register_its_body_writes() {
    // r3 := 0; loop: r3 := r3 + 1 (IntegerBinary); r4 := r3 < 10; BranchIfTrue r4 loop; Return r3
    let graph = build(&snapshot);
    let head = graph.loop_heads().next().unwrap();
    assert_eq!(graph.blocks[head].phis.len(), 1);
    assert_eq!(graph.blocks[head].phis[0].register, 3);
}

#[test]
fn a_send_is_a_frame_node_reading_its_argument_sources() {
    let graph = build(&snapshot);
    let Node::Frame { reads, written, .. } = &graph.blocks[0].nodes[2] else { panic!() };
    assert_eq!(reads.iter().map(|(register, _)| *register).collect::<Vec<_>>(), vec![3, 5]);
    assert_eq!(*written, Some(6));
}

#[test]
fn every_built_graph_verifies() {
    for snapshot in corpus_of_bodies() { // the bodies of goldens/runtime/jit/*.ns, compiled
        assert!(verify(&build(&snapshot)).is_ok());
    }
}
```

`corpus_of_bodies() -> Vec<Snapshot>` is a test helper of `jit/optimizing/mod.rs`, shared by Tasks 3-5: it compiles every `goldens/runtime/jit/*.ns` module with the optimized lowering and snapshots every method and block body it holds.

`verify.rs`: `a_use_before_its_definition_is_refused` (a hand-built graph whose return reads a value defined in a later block) asserting `Err(VerifyError::NotDominated { .. })`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime optimizing::build optimizing::verify`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `graph.rs`, `build.rs`, `verify.rs`**

What a frame node reads is `bytecode::registers_read` of the instruction; what it writes is `register_written`. Dominators by the iterative algorithm of Cooper, Harvey and Kennedy over the block order.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime optimizing`
Expected: PASS.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: the optimizing compiler's SSA graph and its builder"`.

---

### Task 4: Liveness, linear scan and the interrupt points' metadata

**Files:**
- Create: `crates/ns-runtime/src/jit/optimizing/liveness.rs`, `crates/ns-runtime/src/jit/optimizing/allocate.rs`, `crates/ns-runtime/src/jit/optimizing/metadata.rs`
- Modify: `crates/ns-runtime/src/jit/macro_assembler.rs` (`Roles::allocatable`, `Roles::survive_leaf_calls`)
- Test: in-file tests of each

**Interfaces:**
- Consumes: `Graph` (Task 3).
- Produces:
  - `Roles::allocatable: &'static [Register]`, `Roles::survive_leaf_calls: usize` (how many of `allocatable`'s first registers a leaf call preserves), with the lists of "Decisions" item 6.
  - `pub(crate) struct Intervals` from `pub(crate) fn intervals(graph: &Graph, order: &[BlockId]) -> Intervals`: per value its ranges over a linear numbering of nodes (two positions per node: use, then definition), and the positions of every frame node and every leaf-calling simple node.
  - `pub(crate) enum Location { Register(Register), FrameWord }`.
  - `pub(crate) struct Allocation { pub(crate) location_at: …, pub(crate) dirty_at: … }` from `pub(crate) fn allocate(graph: &Graph, intervals: &Intervals, roles: &Roles) -> Allocation`: linear scan in order of interval start; a frame node ends the register lifetime of every value (each continues in its frame word and is loaded again at its next use); a leaf-calling simple node evicts values from registers past `survive_leaf_calls`; when no register is free, the value whose next use is farthest goes to its frame word. A value is *dirty* from its definition until the write-back that stores it into its register's frame word; the allocation records, for each interrupt point, the dirty values to store and where each is.
  - `pub(crate) struct InterruptPoint { pub(crate) program_counter: u32, pub(crate) resumes: Resumes, pub(crate) locations: Vec<(u16, Location)> }`, `pub(crate) enum Resumes { Deliver, Reexecute }`, `pub(crate) struct Metadata { pub(crate) points: Vec<InterruptPoint> }` from `pub(crate) fn metadata(graph: &Graph, allocation: &Allocation) -> Metadata`.
  - `pub(crate) fn verify_allocation(graph: &Graph, allocation: &Allocation, metadata: &Metadata) -> Result<(), VerifyError>`: replays the allocation, checks every use finds its value where the definition or the last move left it, and that at each interrupt point every register the leftover analysis does not taint (`CompiledCode::tainted_before`, through the snapshot's taint sets) has `Location::FrameWord` after the write-back. Run after `allocate` in debug builds.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_value_used_across_a_send_is_in_its_frame_word_at_the_send() {
    let allocation = allocate(&graph, &intervals(&graph, &order), &Roles::of(Abi::Win64));
    let metadata = metadata(&graph, &allocation);
    let send = metadata.points.iter().find(|point| point.program_counter == 2).unwrap();
    assert!(send.locations.iter().all(|(_, location)| *location == Location::FrameWord));
}

#[test]
fn a_loop_counter_stays_in_a_register_across_the_loop() {
    // The counting loop of Task 3's test, with no send in it.
    let allocation = allocate(&graph, &intervals(&graph, &order), &Roles::of(Abi::Win64));
    assert!(matches!(allocation.location_of(counter_phi, loop_end_position), Location::Register(_)));
}

#[test]
fn more_live_values_than_registers_spill_the_farthest_used() {
    // Ten values live at once on Win64, which has eight allocatable registers.
    let allocation = allocate(&graph, &intervals(&graph, &order), &Roles::of(Abi::Win64));
    assert_eq!(allocation.in_frame_word_at(peak_position), 2);
    assert!(verify_allocation(&graph, &allocation, &metadata(&graph, &allocation)).is_ok());
}

#[test]
fn every_built_graph_allocates_and_verifies_on_both_abis() {
    for snapshot in corpus_of_bodies() {
        for abi in [Abi::Win64, Abi::SystemV, Abi::Aapcs64] {
            let graph = build(&snapshot);
            let allocation = allocate(&graph, &intervals(&graph, &block_order(&graph)), &Roles::of(abi));
            assert!(verify_allocation(&graph, &allocation, &metadata(&graph, &allocation)).is_ok());
        }
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime optimizing::allocate optimizing::liveness optimizing::metadata`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `Roles::allocatable`, liveness over SSA (backward over the block order, phis' inputs live out of their predecessors), the linear scan, the metadata and its verifier**

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime optimizing` and `cargo nextest run -p ns-runtime --test jit_architecture_split`
Expected: PASS.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: linear scan over the optimizing graph, with each interrupt point's metadata"`.

---

### Task 5: Emitting optimized code

**Files:**
- Create: `crates/ns-runtime/src/jit/optimizing/emit.rs`
- Modify: `crates/ns-runtime/src/jit/macro_assembler.rs` (idioms on allocated registers), `crates/ns-runtime/src/jit/baseline.rs` (make the template functions `BodyAssembler` offers reachable from `optimizing::emit`: `pub(super)` within `jit`), `crates/ns-runtime/src/jit/closures.rs` (`transfer_to_twin` serves an optimized body's transfer words)
- Test: in-file tests of `emit.rs` on `stubs::testing::Machine`, and cross-assembly tests for `Abi::Aapcs64`

**Interfaces:**
- Consumes: `Graph`, `Allocation`, `Metadata` (Tasks 3, 4); the baseline's `BodyAssembler` and template functions (`copy`, `integer_binary`, `sends::send_public`, `sends::send_lexical`, `sends::send_super`, `sends::return_from`, `sends::initialize`, `closures::outer_local`, `closures::transfer_to_twin`, `body.step()`, …).
- Produces:
  - `pub(crate) struct OptimizedBody { pub(crate) code: ns_assembler::Assembled, pub(crate) returns: Vec<(Label, u32)>, pub(crate) words: Option<Range<u32>>, pub(crate) twin_transfers: Vec<TwinTransfer>, pub(crate) metadata: Metadata }`.
  - `pub(crate) fn compile(snapshot: &Snapshot, macro_assembler: MacroAssembler, role: BodyRole) -> Result<OptimizedBody, Refusal>` in `jit/optimizing/mod.rs`: `build`, `verify`, `intervals`, `allocate`, `metadata`, `verify_allocation`, `emit`.
  - `pub(crate) fn emit(snapshot: &Snapshot, graph: &Graph, allocation: &Allocation, body: &mut BodyAssembler) -> Result<(), Refusal>`: the baseline's `sends::prologue`; then each block in order: phis' moves on each incoming edge (parallel moves resolved with `idiom_scratch` for cycles); `FrameRead` loads the frame word (`load_frame_register` into the allocated register, with its debug check); `Constant` a `move_immediate`; `Receiver` `load_receiver`; `IntegerBinary` on allocated registers -- tag test, untag, add/subtract/multiply with the overflow branch to `fallback`, compares into a Boolean -- the same instruction choice as `baseline::integer_binary`; before every frame node the write-back of the dirty values the allocation names, then the frame node by the template function its instruction's baseline arm calls; a backward edge writes back on its cold path and then polls (`poll(target)`); `MakeClosure` writes back and transfers into the twin.
  - New idioms in `macro_assembler.rs`: `move_register(&mut self, target: Register, source: Register)`, `store_frame_register_from(&mut self, register: u16, source: Register)` (as `store_frame_register`, any register), `load_frame_register_into(&mut self, target: Register, register: u16)` (as `load_frame_register`, any register, same debug poison check), `untag_small_integer`, `tag_small_integer`, `test_small_integer_tags(left: Register, right: Register, failure: Label)` -- whatever `integer_binary`'s template does inline today, moved into idioms both use.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_optimized_counting_loop_answers_the_count() {
    // r3 := 0; loop: r3 := r3 + 1; r4 := r3 < 1000; BranchIfTrue r4 loop; Return r3
    let mut machine = Machine::new();
    let optimized = compile(&snapshot, MacroAssembler::new(abi_of_this_machine()), BodyRole::Method).unwrap();
    let entry = machine.install(&optimized.code);
    assert_eq!(machine.run(entry, &mut frame), small_integer(1000));
}

#[test]
fn an_optimized_loop_writes_its_counter_back_before_its_poll_exits() {
    // The same loop, every poll exiting (set_exit_at_every_poll(true)).
    let exit = machine.run(entry, &mut frame);
    assert_eq!(exit.kind, ExitKind::Safepoint);
    assert_eq!(frame.register(3), small_integer(1)); // written back
}

#[test]
fn an_overflow_takes_the_fallback_send_with_the_operands_in_their_frame_words() {
    // r3 := SmallInteger max; r4 := r3 + r3 (fallback: SendPublic #+); the send exits (uncached).
    let exit = machine.run(entry, &mut frame);
    assert_eq!(exit.program_counter, fallback_send_program_counter);
    assert_eq!(frame.register(3), small_integer(SMALL_INTEGER_MAX));
}

#[test]
fn an_optimized_body_refuses_a_dead_register() {
    // Debug builds: a FrameRead of a word holding DEAD_REGISTER exits DeadRegister.
    assert_eq!(machine.run(entry, &mut poisoned_frame).kind, ExitKind::DeadRegister);
}

#[test]
fn an_optimized_body_assembles_for_aapcs64() {
    for snapshot in corpus_of_bodies() {
        assert!(compile(&snapshot, MacroAssembler::new(Abi::Aapcs64), BodyRole::Method).is_ok());
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime optimizing::emit`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the idioms (moving `integer_binary`'s inline sequences into them, the baseline calling them unchanged in effect), `emit` and `compile`**

- [ ] **Step 4: Run the tests and the baseline's**

Run: `cargo nextest run -p ns-runtime optimizing` and `cargo nextest run -p ns-runtime jit::` and `--test jit`
Expected: PASS; the baseline's machine code for every body unchanged byte for byte (hash every body compiled over `goldens/runtime/jit` before and after, as the AArch64 batch did).

- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: optimized code emitted through the macro assembler and the baseline's templates"`.

---

### Task 6: Installing optimized code at a safepoint

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`JitSlot::optimized`, `InstalledBody::tier`, the entry lookups, `optimize_requested`), `crates/ns-runtime/src/interpreter/safepoint.rs` (`Interpreter::safepoint` runs the requests), `crates/ns-runtime/src/interpreter/inline_cache.rs` (cells refreshed onto optimized code), `crates/ns-runtime/src/jit/materialize.rs` (frames of optimized code)
- Test: `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: `optimizing::compile` (Task 5), `JitState::optimization_requests` (Task 2).
- Produces:
  - `JitSlot` gains `optimized: Cell<u32>` (index in `bodies`, `NO_BODY = u32::MAX`), set only while the slot's baseline code is `COMPILED` in this zone.
  - `InstalledBody` gains `tier: Tier` (`pub(crate) enum Tier { Baseline, Optimized }`); an optimized body has no twin of its own, its `twin_transfers` name the baseline twin.
  - `fn optimized(&self, code: &CompiledCode) -> Option<&InstalledBody>` beside `installed`.
  - The lookups that **prefer optimized code**: `machine_entry_of`, `entry_of_target` (cells), `note_block_entry` (rows), `entered` at program counter 0 (a push). The lookups that **keep baseline code**: `interpreter_entry` at any program counter but 0, `entry_of_frame` for `resumed` and `at_back_edge`, `twin_entry`.
  - `pub(crate) fn optimize_requested(mutator: &mut Mutator<'_>)`: for each requested body still compiled in this zone, `Snapshot::take`, `optimizing::compile`, install through `install_body` (its returns into `JitState::returns`, its words, its transfer words), set `slot.optimized`, `refresh_cells_entering(baseline start)`, re-record the machine entry row, count `optimized_bodies` and `optimize_nanoseconds`. A refusal leaves the body baseline and parks its budget for good.
  - `Interpreter::safepoint` calls it before the stress modes' work, only when requests are pending.

- [ ] **Step 1: Write the failing tests**

```rust
ns_runtime::for_each_lowering! {
    fn a_hot_body_runs_optimized_and_answers_the_same(lowering) {
        let (answer, statistics) = run_compiled(SUM_OF_SQUARES_LOOP, lowering, true);
        assert_eq!(answer, expected_sum_of_squares());
        assert_eq!(statistics.optimized_bodies, 1);
    }
}

ns_runtime::for_each_lowering! {
    fn a_frame_returned_to_resumes_in_baseline_code(lowering) {
        // The optimized loop sends to a method that signals and resumes a
        // resumable exception, so that its frame is materialized and returned into.
        let (answer, statistics) = run_compiled(SIGNAL_IN_A_HOT_LOOP, lowering, true);
        assert_eq!(answer, 10_000);
        assert!(statistics.resumed_entries > 0);
    }
}

ns_runtime::for_each_lowering! {
    fn a_closure_made_in_optimized_code_transfers_into_the_twin(lowering) {
        // The hot loop makes a block capturing the counter and evaluates it.
        let (answer, statistics) = run_compiled(CLOSURE_IN_A_HOT_LOOP, lowering, true);
        assert_eq!(answer, expected());
        assert_eq!(statistics.optimized_bodies, 1);
        assert!(statistics.compiled_twins >= 1);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --test jit -E 'test(/optimized|baseline_code|into_the_twin/)'`
Expected: FAIL (`optimized_bodies` stays 0).

- [ ] **Step 3: Implement the slot field, the tier, the lookups, `optimize_requested`, its call at the safepoint; `materialize_frame` builds a `MachineFrame` from an optimized body's return site as from a baseline one (the frame's words are the interpreter's at a send)**

- [ ] **Step 4: Run the tests and the whole suite**

Run: `cargo nextest run --workspace` (timeout 600000)
Expected: PASS, every golden unchanged.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit` plus `codegen` (`interpreter/mod.rs` or `safepoint.rs` via `mod.rs` if touched), then `git commit -m "feat: optimized code installed at a safepoint beside baseline code"`.

---

### Task 7: Exits, abandonment and the code zone for optimized code

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`resumption_after_exit`, `abandon`), `crates/ns-runtime/src/jit/zone.rs` (`free_body`, `free_dead_bodies`, `free_bodies_with_moved_bases`, `slide`, `still_named`, `verify_code_zone_holders`)
- Test: `crates/ns-runtime/tests/jit.rs`, `crates/ns-runtime/src/jit/zone.rs` tests

**Interfaces:**
- Produces:
  - An exit from optimized code counts in `optimized_exits` and against `EXIT_BUDGET` on the optimized body's own count (`InstalledBody::exits: Cell<u32>`), never the baseline's; spent, `abandon_optimized(mutator, code)` frees only the optimized body, clears `slot.optimized`, refreshes the cells entering its start, re-records the row with the baseline entry, counts `abandoned_optimized_bodies`, and re-arms the budget at `OPTIMIZATION_BUDGET << abandonments` (Strongtalk's back-off, capped at `MAXIMUM_REVIVAL_DOUBLINGS`).
  - Freeing a body's baseline code (abandon, refuse, moved base, dead body, zone replaced, JIT switched off) frees its optimized code first.
  - A compaction slides optimized bodies with the rest (`JitSlot::relocated` carries `optimized`).

- [ ] **Step 1: Write the failing tests**

```rust
ns_runtime::for_each_lowering! {
    fn an_optimized_body_that_keeps_exiting_is_abandoned_and_baseline_stays(lowering) {
        // A hot loop whose every turn sends to an uncached site (an exit).
        let (answer, statistics) = run_compiled(EXITING_HOT_LOOP, lowering, true);
        assert_eq!(answer, expected());
        assert_eq!(statistics.abandoned_optimized_bodies, 1);
        assert_eq!(statistics.abandoned_bodies, 0);
    }
}

#[test]
fn a_compaction_slides_optimized_code() {
    // Interpreter with a tiny zone; optimize a body, free a body below it, compact.
    interpreter.compact_jit_zone();
    assert_eq!(run(&mut interpreter, &class), expected());
    assert!(interpreter.jit_statistics().moved_bytes > 0);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --test jit -E 'test(/abandoned_and_baseline|slides_optimized/)'`
Expected: FAIL.

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run the suite, and the suite under the zone stress mode**

Run: `cargo nextest run --workspace`, then `NS_NEXT_STRESS_ZONE=1 cargo nextest run -p ns-runtime --test jit`
Expected: PASS.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: optimized code abandoned, freed and slid apart from its baseline code"`.

---

### Task 8: The two stress modes

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`stress_optimize_from_env`, `stress_deopt_from_env`, `JitState::new`), `crates/ns-runtime/src/jit/optimizing/emit.rs` (the countdown at each interrupt point under DEOPT), `crates/ns-runtime/src/heap/machine_words.rs` (`MachineWord::DeoptCountdown`), `crates/ns-runtime/src/interpreter/safepoint.rs` (the rotation of n), `crates/ns-runtime/src/interpreter/mod.rs` (`set_optimize_stress`, `set_deopt_stress` hidden hooks), `xtask/src/main.rs` (`STRESS_MODES` to eleven, help text, `the_stress_group_runs_eleven_modes`), `crates/ns-runtime/tests/jit.rs` (`a_stress_mode_polls_out`'s list gains `DEOPT`), `crates/ns-cli/tests/cli.rs` (`ns_next_without_a_code_zone`'s and the other `env_remove` lists gain both, and the missing `ZONE`)
- Test: `crates/ns-runtime/tests/jit.rs`, `xtask/src/main.rs` tests

**Interfaces:**
- Produces:
  - `NS_NEXT_STRESS_OPTIMIZE` (debug builds, read once like `NS_NEXT_STRESS_JIT`): the JIT on, every body's budget armed at 0 so that its first spend requests it, and `optimize_requested` run at every safepoint.
  - `NS_NEXT_STRESS_DEOPT` (debug builds, implies OPTIMIZE): optimized code, at every interrupt point, decrements `MachineWord::DeoptCountdown`; at zero its cold path writes back and exits `Reexecute` at that point's program counter (a send point: before the send). `Interpreter::safepoint` resets the countdown to `n`, `n` rotating over 1, 2, 3, 5, 8, 13 at successive safepoints (`StressSchedule`'s pacing).
  - `Interpreter::set_optimize_stress(&mut self, on: bool)`, `Interpreter::set_deopt_stress(&mut self, on: bool)` (`#[doc(hidden)]`, every build, as `set_jit_stress`).

- [ ] **Step 1: Write the failing tests**

```rust
ns_runtime::for_each_lowering! {
    fn under_the_optimize_stress_a_body_run_twice_is_optimized(lowering) {
        let (mut interpreter, class) = load_with(RETURN_THREE_TWICE, lowering);
        interpreter.set_optimize_stress(true);
        assert_eq!(run(&mut interpreter, &class), 6);
        assert!(interpreter.jit_statistics().optimized_bodies >= 1);
    }
}

ns_runtime::for_each_lowering! {
    fn under_the_deopt_stress_every_point_leaves_and_the_answer_holds(lowering) {
        let (mut interpreter, class) = load_with(SUM_OF_SQUARES_LOOP, lowering);
        interpreter.set_deopt_stress(true);
        assert_eq!(run(&mut interpreter, &class), expected_sum_of_squares());
        assert!(interpreter.jit_statistics().optimized_exits > 100);
    }
}
```

`xtask`: rename `the_stress_group_runs_nine_modes` to `the_stress_group_runs_eleven_modes`, `assert_eq!(STRESS_MODES.len(), 11)`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --test jit -E 'test(/stress_a_body|every_point_leaves/)'` and `cargo nextest run -p xtask`
Expected: FAIL.

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run the suite under each new mode**

Run: `NS_NEXT_STRESS_OPTIMIZE=1 cargo nextest run --workspace`, then `NS_NEXT_STRESS_DEOPT=1 cargo nextest run --workspace` (each timeout 600000)
Expected: PASS, every golden's outcome unchanged. A failure here is a defect of Tasks 3-7: fix it at its cause, never by excluding a golden.

- [ ] **Step 5: Commit** -- `cargo xtask gates commit` plus `codegen`, then `git commit -m "feat: the optimize and deopt stress modes"`.

---

### Task 9: The optimizer's goldens

**Files:**
- Create: `goldens/runtime/optimizer/` -- `counting_loop.ns`, `sends_in_a_loop.ns`, `closure_in_a_loop.ns`, `non_local_return_from_a_loop.ns`, `overflow_in_a_loop.ns`, `allocation_in_a_loop.ns` (scavenges inside the optimized loop), `exception_through_an_optimized_frame.ns`, `deep_recursion_optimized.ns` (overflows the depth limit where the interpreter does), `mixin_applied_twice_optimized.ns` (one optimized body run on two applications), each with its `.toml` recorded under `NS_NEXT_NO_JIT=1`
- Modify: `crates/ns-cli/tests/run_goldens.rs` (every golden under `goldens/runtime/optimizer/` also runs as `golden::optimizer::<name>` with `NS_NEXT_STRESS_OPTIMIZE=1`, in debug builds), `docs/design/README.md` ("The goldens": that directory's rule)

**Interfaces:**
- Consumes: Tasks 6-8.

- [ ] **Step 1: Write the goldens and their `.toml`s** (`NS_NEXT_NO_JIT=1 cargo run -p ns-cli -- run <file>`; establish by reading each program that its output is right), formatted with `cargo run -p ns-analyzer -- fmt goldens/runtime/optimizer`, commented with `///`.

- [ ] **Step 2: Run them before the runner change, then after**

Run: `cargo nextest run -p ns-cli --test run_goldens -E 'test(/optimizer/)'`
Expected: PASS under both lowerings and under `golden::optimizer::`.

- [ ] **Step 3: Commit** -- `cargo xtask gates commit`, then `git commit -m "test: the optimizer's goldens, run under the optimize stress mode"`.

---

### Task 10: Measurement

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`OPTIMIZATION_BUDGET`, `BUDGET_AFTER_CACHE_CHANGE` once measured), `docs/design/execution.md` (a new subsection under "Measurements": "What the optimizing tier's foundation moved")

- [ ] **Step 1: Follow the `measuring-performance` skill's final measurement**: three pinned builds a side (`master`, this branch), the Are We Fast Yet programs and the micro sources, three rotating rounds; also `--jit-statistics` per program for `optimized_bodies`, `optimize_nanoseconds`, `optimized_exits`.
- [ ] **Step 2: Choose the two budget constants** from a sweep of `OPTIMIZATION_BUDGET` over 30_000, 60_000, 120_000, 240_000 on the interim protocol; record the sweep.
- [ ] **Step 3: Write the subsection**: per program the pooled change, the spread, the compile time's share, what register allocation alone moved against the register cache's forecast ("What a register cache would move"), and what the measurement does not cover. A measured slowdown is diagnosed and fixed in this task, not reported (the budget's spend in baseline code is the first suspect: look at its code before measuring again).
- [ ] **Step 4: Commit** -- `cargo xtask gates commit`, then `git commit -m "perf: the optimizing tier's foundation measured"`.

---

### Task 11: Documents and the end of the batch

**Files:**
- Modify: `docs/design/execution.md` (a "The optimizing tier's foundation" section in the present tense for what is built: the snapshot, the budget, the graph, the allocation, the write-back at interrupt points, the two tiers' entries, abandonment; "Planned: the optimizing JIT", "Stages": stage 1 built, the raw area moved to stage 5), `docs/design/README.md` (the map's row; "Gates": the two modes, "the twelve suites at once"), `docs/design/memory.md` (the stress modes' count and list, :2289-2304, :2956), `CLAUDE.md` (the `ns-runtime` paragraph names `jit/optimizing/`; the `stress` row "the eleven stress modes"; "Machine code is a lowering…": optimized code emits every instruction it does not lower as a simple node through the baseline's template), `README.md:128` (the count), `.claude/skills/finishing-a-batch/SKILL.md` (the modes), `docs/OPEN-WORK.md` ("Next": stage 2; step 9's entry: stage 1 built, what it left), `docs/PROGRESS.md` (the batch's entry with its commits)

- [ ] **Step 1: Write the documents.**
- [ ] **Step 2: Run the end-of-batch gates**: `cargo xtask gates end-of-batch` (timeout 600000 per group; the Docker groups last).
Expected: every group passes; the stress logs `.temp/stress-OPTIMIZE.log` and `.temp/stress-DEOPT.log` show their summaries passing.
- [ ] **Step 3: Commit** -- `git commit -m "docs: the optimizing tier's foundation"`.
