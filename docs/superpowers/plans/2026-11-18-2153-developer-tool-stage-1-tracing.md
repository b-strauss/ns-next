# The Developer Tool, Stage 1: Tracing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ns-next run --trace=<categories> [--trace-filter=<selection>]` writes one line per event on standard error, saying why a body is compiled, entered by on-stack replacement, abandoned, revived, optimized, inlined or not, retired, invalidated or freed, and how each inline cache moves between vacant, monomorphic, polymorphic and megamorphic; `debug_start` of the MCP server and the plugin's run configuration pass the same flags.

**Architecture:** `ns-runtime` gains a `trace` module: typed events whose reasons are enums, a `TraceSink` the host installs (`Interpreter::set_trace`), and a `Tracer` held by `JitState`. Event sites, all cold, test one category bit and push a *pending* event holding `Rc<CompiledCode>`s and plain values, never a handle; `trace::flush` names the bodies from the heap (cached), applies the filter and hands each `TraceEvent` to the sink, at every compile trigger's end, at the run loop's safepoint and when a host run returns. `ns-cli` formats each event as one line, `[category] <body> <event> name=value…`.

**Tech Stack:** Rust 1.100.0 (edition 2024), `cargo-nextest`; Kotlin for the plugin's run configuration; no new external dependency (the disassembler dependencies are stage 2's).

**Spec:** `docs/design/developer-tool.md` -- decisions 2, 3, 7, 8, 9; "Planned: the stages", "Stage 1: tracing"; `docs/OPEN-WORK.md`, "A developer tool like V8's `d8`".

## Global Constraints

- No new external dependency. `ns-runtime` formats nothing: an event is a Rust value; the line is `ns-cli`'s.
- **No handle in an event, pending or delivered.** A pending event holds `Rc<CompiledCode>`, `DispatchId`s, `Symbol`s and integers; a receiver is named as a `String` at the moment it is recorded (`Heap::class_display_name`), never kept as a `Value` (`CLAUDE.md`, "A handle means nothing against a `Heap` that did not mint it").
- **Nothing on the hot path.** Every event site is cold -- a compile, an optimization, a retirement, an invalidation, a free, a cache miss's `advance` -- and tests one bit (`Tracer::traces(category)`, `#[inline]`) before it builds anything. No template, stub or leaf of machine code changes. A commit touching `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs` or `heap/frames.rs` runs `cargo xtask gates commit codegen`, and the budgets hold unchanged.
- **Release builds trace** (decision 3): no `cfg(debug_assertions)` around any of it; on a target without a JIT (`cfg(not(ns_jit))`) only `caches` fires.
- **No golden, no `run-corpus` outcome, no image, no instruction encoding changes.** Without `--trace` the output of every program is byte for byte what it was.
- The line format (decision 8): `[<category>] <body> <event>` then ` name=value` pairs in a fixed order per event; `-` for an event that names no body; no timestamps. Category names: `compile`, `osr`, `optimize`, `inlining`, `deoptimize`, `code-zone`, `caches`. Every enum value is spelled by its `name()` in kebab case.
- A body is named as the debugger names a frame (`debugger/stack.rs`'s `frame_name`) with its owner qualified from its module down, as the MCP server's method breakpoints name a class: `Demo>>factorial:`, `Demo.Account>>deposit:`, `Demo.Account class>>named:`, `[] in Demo>>main:args:`, `Demo.Account initializer`, `Demo.Account superclass clause`, `Demo.Account class>>named:` for a factory. A body no mixin of the heap holds is `(unnamed)`.
- The filter (`--trace-filter`, decision 5's syntax): comma-separated patterns `Module.Class>>selector`, `Module.Class class>>selector`, a trailing `*` making a prefix; a block's event matches through its `[] in ` owner; an event without a body always passes; a malformed pattern is refused with exit status 2 before anything runs.
- Tests that pin events follow `crates/ns-runtime/tests/jit.rs`: `#![cfg(ns_jit)]`, `set_jit_enabled(true)`, the breakpoint and step stress modes off; the CLI's tests strip every `NS_NEXT_STRESS_*` and `NS_NEXT_NO_JIT` from the child, as `plain_jit_counts` in `crates/ns-cli/tests/cli.rs` does, and answer nothing on a target without a JIT. Assertions name events that must occur, never a complete sequence, so that a change of a threshold breaks nothing but the reason it changes.
- Names written out; never `{:?}` a `Value`; a Newspeak module inline in a test is an `indoc! {"…"}` with its closing `"}` on its own line; comments explain the decision.
- Every commit runs `cargo xtask gates commit` (+ `analyzer` when `crates/ns-analyzer` is touched, + `codegen` as above). The batch ends with `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification` (Task 9); every commit before is work in progress. Subagents run every command in the foreground, with `timeout: 600000` on every long one.

## Review Focus

1. **A body freed and its address reused before a flush** (a compaction between a `compiled` event and the flush): the pending event holds an `Rc<CompiledCode>`, so the name is the right body's, never another's; the names cache keys by pointer *and* holds a `Weak` it checks. Pinned in Task 1 (`a_name_cached_for_a_dropped_body_is_not_reused`).
2. **A superseded body** (a method an install replaced, still running): named after the version that holds it, through `Heap::superseded_mixins`. Pinned in Task 1 (`a_superseded_body_keeps_its_name`).
3. **A trace of the `caches` category on a long program** (DeltaBlue, many transitions): the pending queue is drained at every safepoint and host return, so it never grows with the run; Task 5 pins that a run of 100,000 sends leaves the queue empty after the run (`the_queue_is_empty_after_a_run`).
4. **A program that writes to standard error itself**: trace lines and the program's own stay whole lines, interleaved only between lines; the sink writes each line with one `write_all` of the line and its newline on a locked `stderr`. Pinned in Task 6 (`trace_lines_are_whole_lines_beside_the_programs_stderr`).
5. **An unknown category or a bad filter**: `--trace=compil` and `--trace-filter='>>'` refuse with exit status 2 and a message naming the bad word, before the program runs. Pinned in Task 6 (`an_unknown_category_is_refused`, `a_malformed_filter_is_refused`).

---

### Task 1: The `trace` module, its sink, names and filter

**Files:**
- Create: `crates/ns-runtime/src/trace.rs`, `crates/ns-runtime/src/trace/names.rs`, `crates/ns-runtime/src/trace/filter.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (re-exports), `crates/ns-runtime/src/jit/mod.rs` (`JitState::tracer`), `crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter::set_trace`, the flush after `run_from_host`), `crates/ns-runtime/src/interpreter/safepoint.rs` (the flush in `Mutator::safepoint`), `crates/ns-runtime/src/jit/mod.rs` (`after_compile_trigger` flushes)
- Test: `crates/ns-runtime/src/trace/filter.rs`, `trace/names.rs` (unit), `crates/ns-runtime/tests/trace.rs` (new)

**Interfaces:**
- Produces (public, re-exported from `lib.rs`):
  - `pub enum TraceCategory { Compile, Osr, Optimize, Inlining, Deoptimize, CodeZone, Caches }` with `pub const ALL: [TraceCategory; 7]`, `pub fn name(self) -> &'static str` and `pub fn from_name(name: &str) -> Option<TraceCategory>`.
  - `#[derive(Clone, Copy, Default, PartialEq, Eq)] pub struct TraceCategories(u8)` with `pub fn with(self, category: TraceCategory) -> TraceCategories`, `pub fn contains(self, category: TraceCategory) -> bool`, `pub fn is_empty(self) -> bool`.
  - `pub struct TraceFilter` with `pub fn everything() -> TraceFilter`, `pub fn parse(text: &str) -> Result<TraceFilter, String>` (the error names the bad pattern) and `pub fn matches(&self, body: &str) -> bool`.
  - `pub struct TraceEvent { pub category: TraceCategory, pub body: Option<String>, pub kind: TraceEventKind }` and `pub enum TraceEventKind` (its variants are added by Tasks 2 to 5; Task 1 declares it with none but `#[non_exhaustive]` is *not* used -- `ns-cli`'s formatter matches it exhaustively).
  - `pub trait TraceSink { fn event(&mut self, event: &TraceEvent); }`
  - `impl Interpreter { pub fn set_trace(&mut self, categories: TraceCategories, filter: TraceFilter, sink: Box<dyn TraceSink>) }` -- replaces any sink; with empty `categories` it is the same as no tracing.
- Produces (crate):
  - `pub(crate) struct Tracer` in `JitState` as `tracer`, with `#[inline] pub(crate) fn traces(&self, category: TraceCategory) -> bool` and `pub(crate) fn record(&mut self, pending: Pending)`.
  - `pub(crate) struct Pending { pub(crate) category: TraceCategory, pub(crate) body: Option<Rc<CompiledCode>>, pub(crate) kind: PendingKind }`; `PendingKind` mirrors `TraceEventKind` with `Rc<CompiledCode>` wherever the event names a second body and `CustomizationKey` wherever it names a key (Tasks 2 to 5 add its variants with theirs).
  - `pub(crate) fn flush(mutator: &mut Mutator<'_>)`: names each pending body and key, drops what the filter refuses, delivers the rest in order, empties the queue. A key is named `<receiver class>` or, when its entered application is another, `<receiver class>@<entered class>` (`Heap::application_of_dispatch_id`, then the application's mixin's `reflected_name`); an id no application holds any longer is `#<id>`.
  - `names.rs`: `pub(crate) struct BodyNames` with `pub(crate) fn name(&mut self, mutator: &Mutator<'_>, code: &Rc<CompiledCode>) -> String`, keyed by `Rc::as_ptr` with a `Weak<CompiledCode>` checked on every hit; a miss rebuilds the index over every mixin (`Heap::mixin_ids` then `Heap::mixin`, both sides, and `Heap::superseded_mixins`): its compiled methods, its factory's body, its initializer bodies, its superclass clause, and every block body reachable through `Instruction::MakeClosure { code, .. }` from those, recursively, named `[] in <its root's name>`. The owner is qualified from its module down through `Mutator::path_from_module_root` (`interpreter/edit.rs`).

- [ ] **Step 1: Write the failing unit tests**
  - `filter.rs`: `a_pattern_names_one_method` (`Demo.Account>>deposit:` matches `Demo.Account>>deposit:` and not `Demo.Account>>withdraw:`); `a_star_makes_a_prefix` (`Demo.Account>>*` matches both; `Demo*` matches `Demo>>main:args:` and `Demo.Account class>>named:`); `a_block_matches_through_its_owner` (`Demo>>main:args:` matches `[] in Demo>>main:args:`); `several_patterns_are_alternatives`; `a_malformed_pattern_is_refused` (`""`, `">>"`, `"Demo>>"`, `"*>>x"` each `Err` naming the pattern).
  - `names.rs`, over a module with a nested class, a class-side method, a slot initializer and a block: `a_method_is_named_from_its_module_down` (`Demo.Account>>deposit:`); `a_class_side_method_and_a_factory` (`Demo.Account class>>named:`); `a_block_is_named_after_its_root` (`[] in Demo>>main:args:`); `an_initializer_and_a_superclass_clause`; `a_superseded_body_keeps_its_name` (install an edit of `deposit:` through `interpreter/install.rs`'s test helpers, then name the old body); `a_name_cached_for_a_dropped_body_is_not_reused` (name a body, drop every `Rc` of it, compile another into the same allocation if the allocator reuses it -- or else construct the cache entry by hand with a dead `Weak` -- and the name is recomputed).
  - `tests/trace.rs`: `no_sink_no_events_and_no_cost` (a run with `set_trace` never called and one with empty categories: the sink, a `Rc<RefCell<Vec<TraceEvent>>>` collector, receives nothing).
- [ ] **Step 2: Run them to see them fail** -- `cargo nextest run -p ns-runtime trace` -- does not compile (`trace` missing).
- [ ] **Step 3: Implement** the types and functions above; the three flush points call `trace::flush` only when the queue is non-empty (`after_compile_trigger` holds the mutator already; `Mutator::safepoint` at its start; `Interpreter::run_from_host` after the run returns, through `self.enter`).
- [ ] **Step 4: Run them to see them pass**, then `cargo xtask gates commit codegen` (the safepoint and `interpreter/mod.rs` are touched).
- [ ] **Step 5: Commit** -- `feat: trace events, their sink, body names and the filter`

### Task 2: `compile` and `osr` events

**Files:**
- Modify: `crates/ns-runtime/src/trace.rs` (variants), `crates/ns-runtime/src/jit/mod.rs` (`note_entry`, `note_entry_by_call`, `compile_into_slot`, the twin compile, `abandon`, `refuse_for_its_twin`, `JitState::decay_exits`, `JitState::note_abandoned_entry`, `back_edge_ready`, `at_back_edge`)
- Test: `crates/ns-runtime/tests/trace.rs`

**Interfaces:**
- Produces:
  - `pub enum CompileTrigger { Entry, Call, BackJump }` and `pub enum CompileRefusal { JitOff, TooManyInstructions, Unassembled, ZoneFull, TooLarge, ExitsAtOnce }` (from `jit::Refusal` by `CompileRefusal::of(&Refusal)`; `Unassembled` drops the assembler's error), `pub enum Revival { Decay, Entries }`, each with `name()`.
  - `TraceEventKind::Compiled { trigger: CompileTrigger, bytes: usize }` (`compiled`, fields `trigger=`, `bytes=`), `CompiledTwin { bytes: usize }` (`compiled-twin`), `CompileRefused { trigger: CompileTrigger, reason: CompileRefusal }` (`refused`; a `ZoneFull` refusal is traced too: the body counts again), `Abandoned { exits: u32 }` (`abandoned`), `Revived { by: Revival }` (`revived`), `RefusedForItsTwin` (`refused-for-its-twin`), all category `compile`; `OsrCompiled { at: u32 }` (`osr-compiled`, `at=` the bytecode index of the loop's head) and `OsrEntered { at: u32 }` (`osr-entered`), category `osr`.
  - `compile_into_slot` takes `trigger: CompileTrigger`: `note_entry` passes `Entry`, `note_entry_by_call` `Call` (through a `trigger` parameter of `note_entry`, which every other caller passes as `Entry`), `back_edge_ready` `BackJump`. `bytes` is the installed body's `end - start`.
- [ ] **Step 1: Write the failing tests** in `tests/trace.rs`, each running a small module with a collecting sink and categories `compile`/`osr`:
  - `a_body_is_traced_compiled_at_its_second_entry` (a method sent twice: a `Compiled { trigger: Entry, .. }` for `Test>>twice`, named so).
  - `a_loop_is_traced_entering_by_osr` (a method with a 100,000-turn `whileTrue:` sent once: `OsrCompiled` and `OsrEntered` for it, `at` equal).
  - `a_body_entered_while_the_jit_is_off_is_traced_refused` (`set_jit_enabled(false)`, one send: `CompileRefused { reason: JitOff, .. }`).
  - `an_exiting_body_is_traced_abandoned` (reuse the program of `tests/jit.rs`'s abandonment test: `Abandoned` before the run ends).
  - `the_filter_keeps_one_body` (filter `Test>>a`, two hot methods `a` and `b`: events name `Test>>a` only).
- [ ] **Step 2: Run to fail** (`cargo nextest run -p ns-runtime --test trace`).
- [ ] **Step 3: Implement** the records at each site, each behind `state.tracer.traces(..)`.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: trace baseline compiles, abandonments and on-stack replacement`

### Task 3: `optimize` and `inlining` events

**Files:**
- Modify: `crates/ns-runtime/src/trace.rs`, `crates/ns-runtime/src/jit/mod.rs` (`budget_spent`'s request path, `find_recompilee`, `request`, `optimize`, `defer_optimization`), `crates/ns-runtime/src/jit/optimizing/plan.rs` (`InliningPlan::declined`, `Planner::take`, `classify`), `crates/ns-runtime/src/jit/optimizing/emit.rs` or `mod.rs` (carry the plan's decisions out on `OptimizedBody`)
- Test: `crates/ns-runtime/tests/trace.rs`, `crates/ns-runtime/src/jit/optimizing/plan.rs` (unit)

**Interfaces:**
- Produces:
  - `pub enum RecompileWalkEnd { BottomOfStack, NoFramesBetween, SearchLength, InlinedIntoACopyBelow, NotRecompilable, NotInlined }`; `find_recompilee` answers `(ActivationReference, RecompileWalkEnd)`, each `break` naming its value, the loop's natural end `BottomOfStack`.
  - `pub enum Deferral { Patched, ZoneFull }`.
  - `pub enum InlineDecline { NotAMethod, Observed, ReachesPastTheCompile, OverWeight, OverCompileLimit, OverRecursionLimit, OutOfRegisters }`; `InliningPlan` gains `pub(crate) declined: Vec<Declined>` with `pub(crate) struct Declined { pub(crate) scope_code: CodeId, pub(crate) program_counter: u32, pub(crate) callee: Option<CodeId>, pub(crate) reason: InlineDecline }`; every silent `return` of `Planner::take` pushes one (the arm-missing returns, which decide nothing, push none). `OptimizedBody` carries `declined` and, for each inlined scope, `(parent scope's CodeId, program counter, callee CodeId)` as `inlined: Vec<(CodeId, u32, CodeId)>`.
  - `TraceEventKind::OptimizationRequested { spent_by: String, walk_end: RecompileWalkEnd, key: Option<String> }` (`requested`; the event's body is the recompilee), `OptimizationDeferred { key: Option<String>, reason: Deferral }` (`deferred`), `Optimized { key: Option<String>, bytes: usize, inlined_scopes: u64, inlined_sites: u64, type_case_arms: u64, eliminated_blocks: u64 }` (`optimized`), `OptimizationRefused { key: Option<String>, reason: CompileRefusal }` (`refused`), category `optimize`; `Inlined { key: Option<String>, in_body: String, at: u32, callee: String }` (`inlined`) and `InlineDeclined { key: Option<String>, in_body: String, at: u32, callee: Option<String>, reason: InlineDecline }` (`declined`), category `inlining`, both recorded after `compile_tree` answered, installed or not. `CodeId`s are turned into `Rc<CompiledCode>` with `Heap::code` when recorded.
- [ ] **Step 1: Write the failing tests**
  - `plan.rs`: `an_overweight_callee_is_declined_with_its_reason` (lower `Limits::method` in a test plan so a callee is over: `declined` holds one `OverWeight` at the send's program counter); `a_recursion_past_the_limit_is_declined`.
  - `tests/trace.rs`, with the JIT's budget (`load_spending_with`'s setting): `a_hot_loop_is_traced_requested_and_optimized` (a 1,000,000-turn loop sending `step:`: `OptimizationRequested` and `Optimized` for the loop's method, `Inlined` naming `Test>>step:`); `a_patched_body_is_traced_deferred` (a breakpoint through the debugger's test helpers on the hot method: `OptimizationDeferred { reason: Patched, .. }`).
- [ ] **Step 2: Run to fail.**
- [ ] **Step 3: Implement.** `find_recompilee`'s callers take the pair; only the tracing reads the reason.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: trace optimization requests, installs and inlining decisions`

### Task 4: `deoptimize` and `code-zone` events

**Files:**
- Modify: `crates/ns-runtime/src/trace.rs`, `crates/ns-runtime/src/jit/mod.rs` (`count_trap`, `retire_optimized`, `abandon_optimized`, `invalidate_copy`, `invalidate_copies_of`, `invalidate_applications`, `invalidate_all_optimized`, the invalidate stress round, `forget_freed_copies`, `invalidate_copies_of_moved_bases`, `free_for_patch`), `crates/ns-runtime/src/jit/zone.rs` (`mark_free`, `compact_code_zone_freeing`, `free_dead_bodies`, `free_bodies_with_moved_bases`)
- Test: `crates/ns-runtime/tests/trace.rs`

**Interfaces:**
- Produces:
  - `pub enum RetireReason { Traps, Exits }`, `pub enum InvalidationCause { Edit, BulkForward, ReclaimedId, MovedFeedbackBase, Breakpoint, Stress }`, `pub enum CodeTier { Baseline, Optimized }` (not `jit::Tier`, which stays private), `pub enum FreeCause { Compaction, DeadBody, MovedFeedbackBase, Breakpoint, Abandoned }`, `pub enum CompactionFreeing { ToAQuarter, EveryInactiveBody }` (from `zone::Freeing`).
  - `invalidate_copy` and `invalidate_copies_of` take `cause: InvalidationCause`; their callers pass theirs.
  - `TraceEventKind::Retired { key: Option<String>, reason: RetireReason, version: Option<u8> }` (`retired`, `version=` the raised version or `none` when refused for good), `Invalidated { key: Option<String>, cause: InvalidationCause }` (`invalidated`), category `deoptimize`; `Compacted { freeing: CompactionFreeing, bodies_freed: usize }` (`compacted`, no body) and `Freed { tier: CodeTier, cause: FreeCause }` (`freed`), category `code-zone`.
- [ ] **Step 1: Write the failing tests**: `a_trapping_copy_is_traced_retired` (the program of `tests/jit.rs`'s or `jit/mod.rs`'s retirement test -- a loop speculating on `SmallInteger` then given `Float`s: `Retired { reason: Traps, .. }`); `an_edit_is_traced_invalidating_the_copy` (an optimized method, then a MirrorBuilder install of a method it inlined: `Invalidated { cause: Edit, .. }`); `a_compaction_is_traced` (the small zone of `tests/jit.rs`'s `a_zone_with_room_for_a_few_bodies_leaves_the_output_right`: `Compacted` and at least one `Freed { cause: Compaction, .. }`).
- [ ] **Step 2: Run to fail.**
- [ ] **Step 3: Implement.** `mark_free` gains the cause; `FreedBy` stays private.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: trace retirements, invalidations and freed code`

### Task 5: `caches` events

**Files:**
- Modify: `crates/ns-runtime/src/trace.rs`, `crates/ns-runtime/src/interpreter/inline_cache.rs` (`advance` answers the transition; `global_found`, `decided_miss`, `fill` record it; `InlineCaches::family_of_site`)
- Test: `crates/ns-runtime/tests/trace.rs`

**Interfaces:**
- Produces:
  - `pub enum SiteState { Vacant, Monomorphic, Polymorphic, Megamorphic }`.
  - `InlineCaches::advance` answers `Option<(SiteState, SiteState)>`, `Some` only when the site's state changed (a refill of the same state answers `None`).
  - `pub(crate) fn family_of_site(&self, index: usize) -> Option<(Rc<CompiledCode>, u32)>` -- the family whose range holds `index` (`families`), with the site's number within it.
  - `TraceEventKind::CacheTransition { from: SiteState, to: SiteState, selector: String, site: u32, receiver: String }` (`<from>-><to>`, fields `selector=#<selector>`, `site=`, `receiver=`), category `caches`; the body is the family's root; `receiver` the class of the send's receiver named when recorded (`Heap::class_display_name`).
- [ ] **Step 1: Write the failing tests**: `a_site_is_traced_through_its_states` (a send of `value` to a `SmallInteger`, then to 4 more classes, then to a sixth: `Vacant->Monomorphic`, `Monomorphic->Polymorphic`, `Polymorphic->Megamorphic` in that order for one site, named `Test>>send:`); `the_queue_is_empty_after_a_run` (100,000 polymorphic sends traced, then the tracer's queue length, through a `#[doc(hidden)] pub fn trace_queue_length(&mut self) -> usize` test hook, is 0).
- [ ] **Step 2: Run to fail.**
- [ ] **Step 3: Implement.** The interpreter-only tests at `inline_cache.rs`'s test module and `cells.rs` ignore the answer.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit caches` (the caches are touched).
- [ ] **Step 5: Commit** -- `feat: trace inline cache transitions`

### Task 6: `ns-next run --trace` and its line format

**Files:**
- Create: `crates/ns-cli/src/trace.rs` (the sink and the formatter)
- Modify: `crates/ns-cli/src/main.rs` (`Run`'s `trace` and `trace_filter`), `crates/ns-cli/src/commands/run.rs`
- Test: `crates/ns-cli/src/trace.rs` (unit: one line per variant), `crates/ns-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1 to 5's public types.
- Produces: `--trace=<categories>` (comma-separated `TraceCategory::name`s, or `all`) and `--trace-filter=<selection>` on `run` only; `pub(crate) fn line(event: &TraceEvent) -> String` and `pub(crate) struct StandardErrorSink` writing `line` and `\n` in one `write_all` on `std::io::stderr().lock()`.
- [ ] **Step 1: Write the failing tests**
  - unit: `every_event_has_its_line` -- one `TraceEvent` per `TraceEventKind` variant, each line asserted exactly, e.g. `[compile] Demo>>factorial: compiled trigger=entry bytes=412`, `[caches] Demo>>main:args: monomorphic->polymorphic selector=#deposit: site=3 receiver=Account`, `[code-zone] - compacted freeing=to-a-quarter bodies-freed=7`.
  - `cli.rs` (child stripped of stress modes, `None` without a JIT): `trace_compile_names_the_hot_method` (the `while_loop` golden with `--trace=compile`: stdout as without, a `[compile] ` line naming its method); `trace_lines_are_whole_lines_beside_the_programs_stderr` (the `while_loop` golden with `--trace=all --jit-statistics`, both writing stderr: every stderr line starts with `[` or with `jit: `, and exactly one with `jit: `); `an_unknown_category_is_refused` (`--trace=compil`: status 2, stderr names `compil`); `a_malformed_filter_is_refused`; `the_filter_narrows_the_lines` (`--trace=compile --trace-filter=<one method>`: every `[compile]` line names it).
- [ ] **Step 2: Run to fail** (`cargo nextest run -p ns-cli trace`).
- [ ] **Step 3: Implement.** Parse both before the program is read; install the sink after `load_program`.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: ns-next run --trace writes the trace events on stderr`

### Task 7: `debug_start` takes the trace

**Files:**
- Modify: `crates/ns-cli/src/mcp/tools.rs` (`StartArguments::trace`, `trace_filter`, the tool's schema), `crates/ns-cli/src/mcp/session.rs` (`StartOptions`, the child's command line)
- Test: `crates/ns-cli/tests/mcp.rs`

**Interfaces:**
- Produces: `debug_start`'s optional `trace` (string, `--trace`'s value) and `trace_filter` (string), passed to the child as `--trace=…` and `--trace-filter=…` before the program; their lines arrive with the program's output in every answer, as stderr does.
- [ ] **Step 1: Write the failing test**: `a_trace_arrives_with_the_output` (start the `while_loop` golden with `trace: "compile"`, continue to the end: the answer's output holds a `[compile] ` line).
- [ ] **Step 2: Run to fail.** **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: debug_start passes the trace to the program`

### Task 8: The plugin's runtime options

**Files:**
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfiguration.kt` (`runtimeOptions`), `NewspeakRunConfigurationEditor.kt` (a "Runtime options" field)
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfigurationTest.kt`

**Interfaces:**
- Produces: `var runtimeOptions: String?` on the options and the configuration, parsed with `ParametersListUtil.parse` and placed in `newspeakCommandLine` after the subcommand and `--debug-connect`, before `--script`: `ns-next run --trace=compile Demo.ns`.
- [ ] **Step 1: Write the failing test**: `testRuntimeOptionsComeBeforeTheProgram` (`runtimeOptions = "--trace=compile --trace-filter=Demo>>*"`: the command line is `[runtime, run, --trace=compile, --trace-filter=Demo>>*, <program>]`, and with a debug address `--debug-connect=…` stands before them).
- [ ] **Step 2: Run to fail** (`editors/jetbrains/gradlew --no-daemon test --tests '*NewspeakRunConfigurationTest*'`). **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat(plugin): runtime options in a Newspeak run configuration`

### Task 9: Documents and the end of the batch

**Files:**
- Modify: `docs/design/developer-tool.md` (stage 1 in the present tense, **Built**/**Partial** marker; "What exists"), `docs/design/README.md` (the map's row: status and next, stage 2), `docs/design/compiler.md` ("Reading the output"), `docs/design/execution.md` (a sentence where each traced decision is described, pointing at the category), `docs/design/debugger.md` ("The MCP server": `debug_start`'s new parameters), `docs/design/editor-support.md` or `debugger.md` ("The JetBrains plugin": runtime options), `docs/OPEN-WORK.md` (the d8 item: stage 1 done, next stage 2), `docs/PROGRESS.md` (the milestone), `CLAUDE.md` (the crate map: `trace` in `ns-runtime`, `--trace` of `run`, `debug_start`'s trace)
- [ ] **Step 1:** Write the documents; a fresh session must be able to start stage 2 from them alone.
- [ ] **Step 2:** `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification`; both pass.
- [ ] **Step 3: Commit** -- `docs: the developer tool's stage 1 is built`
