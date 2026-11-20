# The Developer Tool, Stage 2: Printing Code — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ns-next run --print-bytecode --print-code --print-inlining [--print-filter=<pattern>]…` prints, on standard error, each body's bytecode when its mixin is made, each baseline body, twin and optimized copy's machine code when it is installed, and the inlining plan of every optimizing compile with its outcome; the DAP agent answers `nsNext/frameCode` for a stopped frame and the MCP server's `debug_code` asks it.

**Architecture:** The trace's queue carries listings beside events. `ns-runtime` captures a listing's facts where they exist -- the bytes, the base address, the position (scope, program counter) per machine offset, an annotation per leaf call, stub jump, exit and cell call, which the macro assembler records only when asked, the scope tree and the plan's arms and declines from `compile_planned` -- copies them into a pending listing at install, and names its bodies at the flush; the bytecode listings come from the index of new sides the trace's names already keep. `ns-cli` decodes the bytes (`iced-x86` on x86_64, `yaxpeax-arm` on AArch64) and formats all three listings; the agent reuses that formatter.

**Tech Stack:** Rust 1.100.0 (edition 2024), `cargo-nextest`; `iced-x86` 1.21 and `yaxpeax-arm` 0.5 / `yaxpeax-arch` 0.3 become normal dependencies of `ns-cli`, per target architecture.

**Spec:** `docs/design/developer-tool.md` -- decisions 3, 4, 5, 7, 10 to 15; "Planned: the stages", "Stage 2: printing code"; "Stage 1: tracing" for what this builds on.

## Global Constraints

- **Nothing on the hot path, nothing in a body's code.** Annotations are recorded by the macro assembler beside the code only when `JitSettings::annotate` is set; the bytes of every body are the same with and without it (Task 2 pins it). No template, stub or leaf changes what it emits. A commit touching `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs` runs `cargo xtask gates commit codegen`, the budgets unchanged.
- **Release builds print** (decision 3): no `cfg(debug_assertions)` around any of it. On a target without `cfg(ns_jit)` `--print-bytecode` works and `--print-code` and `--print-inlining` print nothing; `report_jit_unavailable` already says why.
- **No handle in a listing, pending or delivered** (`CLAUDE.md`, "A handle means nothing…"): a pending listing holds `Rc<CompiledCode>`, `CodeId`s turned into `Rc<CompiledCode>` at capture, `CustomizationKey`/`DispatchId`s, bytes and integers; names are made at the flush (`BodyNames::name`, `trace_events::key_names`).
- **Captured at install, delivered at a flush.** A machine-code listing copies its bytes and its body-relative facts at the install site, because a twin compiled in a leaf flushes only later, after `point_budget_words` and possibly a compaction. Delivery is the trace's: `trace_events::flush`, never in a leaf.
- **A listing differs between runs only in its head** (decision 14): offsets from the body's start; a target in the body `+0x…`; a data word named by its kind (`budget word`, `dispatching rows`, `dispatching tail`, `twin transfer @<pc>`), never its value; a call or jump out named (`leaf miss`, `stub polymorphic`, `exit send @12`, `cell site 3`). x64 in iced's Intel syntax (`IntelFormatter`), AArch64 in `yaxpeax-arm`'s `Display`.
- Names are the trace's (`{Demo/Account>>deposit:}`, decision 8); the filter is a second `TraceFilter`, `--print-filter` once per pattern, `...` ending a prefix, refused with exit status 2 as `--trace-filter` is (`trace::check_filter`), before anything runs.
- Every listing is one `write_all` on a locked standard error, a head line `--- <kind> {<body>} …` and an end line `--- end`, so a script can cut it out (decision 12).
- The DAP request is `nsNext/frameCode` (the agent's prefix, `debug/agent.rs:712-764`); the MCP tool `debug_code`. `debug_start` gains no print parameter: its answers keep 2,000 characters of output (`summary::OUTPUT_LIMIT`), and the code of a frame is `debug_code`'s.
- Tests that pin machine code follow `tests/jit.rs` and stage 1's `tests/trace.rs`: `#![cfg(ns_jit)]`, `set_jit_enabled(true)`, the breakpoint and step stress modes off; the CLI's strip every `NS_NEXT_STRESS_*` and `NS_NEXT_NO_JIT` (`plain_run`) and answer nothing without a JIT (`plain_jit_counts` → `None`). Assertions name lines that must occur, never a whole listing, but for Task 5's one pinned listing of a body that calls no leaf.
- Names written out; never `{:?}` a `Value`; a Newspeak module inline in a test is an `indoc! {"…"}`, its `"}` on its own line; comments explain the decision.
- Every commit runs `cargo xtask gates commit` (+ `codegen` as above). The batch ends with `cargo xtask gates end-of-batch` (Task 8); every commit before is work in progress. Subagents run every command in the foreground, with `timeout: 600000` on every long one.

## Review Focus

1. **A body compacted or freed between its install and the flush** (a twin compiled in a leaf, the zone compacted at the next safepoint): the listing shows the bytes and base it was installed with. Pinned in Task 2 (`a_listing_survives_a_compaction_before_its_flush`, under `Interpreter::compact_code_zone` or the zone stress mode's entry point between install and flush).
2. **Printing without `--trace`**: `EventTracer::install` today stores nothing when the categories are empty; with printing asked it must hold the sink. Pinned in Task 1 (`printing_needs_no_trace_category`) and Task 5 (`print_bytecode_alone_prints`).
3. **A program that writes to standard error itself**: a listing stays whole, never interleaved with the program's lines. Pinned in Task 5 (`a_listing_is_one_write_beside_the_programs_stderr`).
4. **An optimization refused after its plan** (the zone too small for the copy): `--print-inlining` prints the plan with `refused reason=zone-full`, and `--print-code` prints nothing for it. Pinned in Task 4 (`a_refused_optimization_prints_its_plan`), sizing the zone as stage 1's refused-optimization test does.
5. **An installed edit**: its new versions' bodies print at the next flush, the old ones never again. Pinned in Task 1 (`an_installed_edit_prints_its_new_bodies`).

---

### Task 1: Listings in the trace's queue, and the bytecode listing

**Files:**
- Create: `crates/ns-runtime/src/trace_events/listings.rs`
- Modify: `crates/ns-runtime/src/trace_events.rs` (`EventTracer`, `Pending`, `flush`, `TraceSink`), `crates/ns-runtime/src/trace_events/names.rs` (`index_new_sides` answers the sides it indexed), `crates/ns-runtime/src/interpreter/mod.rs` (`set_trace`), `crates/ns-runtime/src/bytecode.rs` (`disassemble_marked`), `crates/ns-cli/src/commands/run.rs` (the new `set_trace` call, nothing printed yet), every test that calls `set_trace`
- Test: `crates/ns-runtime/tests/printing.rs` (new), `bytecode.rs` unit tests

**Interfaces:**
- Produces (public, `ns_runtime::trace_events`):
  - `#[derive(Clone, Copy, Default, PartialEq, Eq)] pub struct Printing { pub bytecode: bool, pub code: bool, pub inlining: bool }` with `pub fn is_empty(self) -> bool`.
  - `pub struct TraceSettings { pub categories: TraceCategories, pub filter: TraceFilter, pub printing: Printing, pub print_filter: TraceFilter }`.
  - `Interpreter::set_trace(&mut self, settings: TraceSettings, sink: Box<dyn TraceSink>)` replaces stage 1's signature; it flushes before it returns, so that the bodies made before it print at once. With empty categories and empty printing it is no tracing.
  - `TraceSink` gains `fn listing(&mut self, listing: &Listing) {}` (a default, so stage 1's collectors stay as they are).
  - `pub enum Listing { Bytecode(BytecodeListing), Code(CodeListing), Inlining(InliningListing) }`; `pub struct BytecodeListing { pub body: String, pub text: String }`, `text` being `bytecode::disassemble`'s for the body, its blocks nested. `CodeListing` and `InliningListing` are declared here with the fields Tasks 2 to 4 fill (below), so the formatter of Task 5 matches all three.
  - `pub fn bytecode::disassemble_marked(code: &CompiledCode, name: &str, mark: Option<u32>) -> String`: `disassemble`'s text, the line of the instruction at `mark` (of the root body, not a block's) beginning `->` instead of its two spaces; `disassemble` is `disassemble_marked(code, name, None)`.
- Produces (crate): `PendingKind::Listing(PendingListing)` in the one queue, so a listing keeps its place among the events; `PendingListing::{Code(…), Inlining(…)}` hold `Rc<CompiledCode>` and keys; the bytecode listings are not pending -- `flush` makes them first, from `BodyNames::index_new_sides`, which now answers the sides it indexed in the order it indexed them, and lists each side's method, factory, initializer and superclass-clause bodies (`name_side`'s walk, blocks nested, not separate) that `print_filter` matches.
- [ ] **Step 1: Write the failing tests** in `tests/printing.rs`, a collecting sink (`Rc<RefCell<Vec<Listing>>>`, a neighbour of stage 1's in `tests/trace.rs`):
  - `the_programs_bodies_print_when_the_sink_is_installed`: load a module `Demo` with `sum:` and a block; `set_trace` with `printing.bytecode` and the filter `Demo...`; the collected `BytecodeListing`s include one with `body == "Demo>>sum:"` whose `text` contains `MakeClosure` and the nested `block`, and none for a `newspeak.core` body.
  - `printing_needs_no_trace_category`: the same with empty categories; listings arrive.
  - `a_shipped_module_built_on_demand_prints_at_the_next_flush`: filter `newspeak.core.TimeFactory...`, a program that sends `platform time` (or the first accessor that builds `TimeFactory`); after the run a listing of a `newspeak.core.TimeFactory/…` body was collected, none before the run.
  - `an_installed_edit_prints_its_new_bodies`: install an edit of `Demo>>sum:` through the MirrorBuilder (`tests/edit.rs`'s helpers); after the run exactly two listings named `Demo>>sum:`, the second's text the new body's.
  - `a_checked_edit_prints_nothing` and `the_debuggers_metadata_compile_prints_nothing` (attach a debugger, read a frame's variables, which compiles the metadata again).
  - `bytecode.rs`: `a_marked_instruction_begins_with_an_arrow` (mark 3 of the `sum:` of the unit test at `:2362`: that one line starts `->`, every other as before).
- [ ] **Step 2: Run them to see them fail** -- `cargo nextest run -p ns-runtime printing disassemble` -- they do not compile.
- [ ] **Step 3: Implement** the types and the flush's bytecode pass; `EventTracer::install` keeps the sink when `categories` or `printing` is non-empty; `is_on()` likewise.
- [ ] **Step 4: Run them to see them pass**; then `cargo xtask gates commit codegen`.
- [ ] **Step 5: Commit** -- `feat: listings in the trace's queue; bytecode printed when its mixin is made`

### Task 2: Annotations and the baseline listing

**Files:**
- Modify: `crates/ns-runtime/src/jit/macro_assembler.rs` (the annotation record), `jit/leaves.rs` (`Leaf::name`), `jit/stubs.rs` (`BlockWord::name`), `jit/sends.rs` (a cell call's annotation), `jit/baseline.rs` (`BaselineBody::annotations`, positions), `jit/mod.rs` (`JitSettings::annotate`, the capture in `compile_into_slot` and `compile_twin`), `trace_events/listings.rs`, `trace_events.rs`, `crates/ns-memory/src/code_zone.rs` only if `executable_address` needs widening
- Test: `crates/ns-runtime/tests/printing.rs`, `macro_assembler.rs` unit tests

**Interfaces:**
- Consumes: Task 1's `Listing`, `PendingKind::Listing`.
- Produces (public, `trace_events`):
  - `pub struct CodeListing { pub body: String, pub tier: CodeTier, pub key: Option<(String, Option<String>)>, pub architecture: Architecture, pub base_address: u64, pub bytes: Vec<u8>, pub code_end: u32, pub words: Vec<(u32, DataWord)>, pub positions: Vec<(u32, Position)>, pub annotations: Vec<(u32, Annotation)>, pub scopes: Vec<ListedScope>, pub mark: Option<u32> }` -- `code_end` the offset of the first data word (or the end), `positions` ascending, `mark` only Task 6's.
  - `pub enum CodeTier { Baseline, Twin, Optimized }`, `pub enum Architecture { X64, Aarch64 }`, each with `name()` (`baseline`, `twin`, `optimized`; `x64`, `aarch64`).
  - `pub enum Position { At { scope: u32, program_counter: u32 }, OutOfLine }`.
  - `pub struct ListedScope { pub number: u32, pub body: String, pub parent: Option<(u32, u32)>, pub instructions: Vec<String> }` -- a baseline listing has one, `s0`, its `instructions` the body's own, one line per program counter in `disassemble`'s instruction form (`bytecode::instruction_lines(code: &CompiledCode) -> Vec<String>`, new, public, `write_code`'s per-line text without the nesting).
  - `pub enum Annotation { Leaf(&'static str), Stub(&'static str), Exit { kind: &'static str, program_counter: u32 }, Cell { site: u32 } }`; `pub enum DataWord { Budget, DispatchingRows, DispatchingTail, TwinTransfer { program_counter: u32 }, Other }`.
- Produces (crate): `JitSettings::annotate: bool`, set by `set_trace` when `printing.code`; `MacroAssembler::annotate(&mut self, annotation: Annotation)` binds a label at the current offset and records it when annotating, nothing otherwise; `Assembled`'s label offsets turn the records into `(u32, Annotation)` in `baseline::compile` and `emit::emit`. `Leaf::name(self) -> &'static str` and `BlockWord::name(self) -> &'static str` in kebab case (`miss`, `materialize-frame`, …; `exit-stub`, `polymorphic-stub`, …), exhaustive matches. Annotated: every `call_leaf` (`Leaf`), every jump or call through a `BlockWord` stub (`Stub`), every `exit` and `trap` (`Exit` with its `ExitKind`'s kebab-case name and program counter), every `send_through_cell` (`Cell` with the site's number). The positions of a baseline body are its `ProgramCounterMap` starts (`Position::At { scope: 0, … }`) and `OutOfLine` at the first cold path `finish_with_sites` lays out and at the backward-branch and dispatching tails.
- [ ] **Step 1: Write the failing tests**:
  - `macro_assembler.rs`: `annotating_changes_no_byte` (assemble the same baseline body with `annotate` off and on: equal bytes).
  - `tests/printing.rs` (`#![cfg(ns_jit)]` module part): `a_hot_method_prints_its_baseline_code` (`Demo>>square:` entered twice, `printing.code`: one `CodeListing` with `tier == Baseline`, `positions` holding `At { scope: 0, program_counter: pc }` for every pc of the body at its `ProgramCounterMap` offset, and an `Annotation::Cell` for its send); `a_twin_prints_as_a_twin` (a body making a closure: a `Twin` listing after the closure is made); `a_data_word_is_named_not_shown` (a method's lean body: `words` holds `Budget` and `DispatchingRows`); `a_listing_survives_a_compaction_before_its_flush`; `the_filter_selects_code_listings_by_body`.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** The capture copies `Assembled.bytes` -- the bytes as assembled, the data words not yet pointed -- and the base `zone.writer.zone().executable_address(CodeOffset(start))` at install, into `PendingListing::Code` with the body's `Rc<CompiledCode>`; the flush names the body and builds `ListedScope` s0.
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: baseline machine code captured at install, annotated`

### Task 3: The optimized copy's listing

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/emit.rs` (positions), `jit/optimizing/mod.rs` (`PlanDecisions` carries the scope tree), `jit/optimizing/plan.rs` (`Declined::scope`), `jit/mod.rs` (`optimize`, `install_optimized`, `trace_inlining`), `trace_events.rs`, `trace_events/listings.rs`
- Test: `crates/ns-runtime/tests/printing.rs`, `emit.rs` unit tests

**Interfaces:**
- Consumes: Task 2's `CodeListing`, `Position`, `ListedScope`, `MacroAssembler::annotate`.
- Produces (crate): `PlanDecisions::scopes: Vec<PlannedScope>` with `pub(crate) struct PlannedScope { number: u32, code_id: CodeId, parent: Option<(u32, u32)> }`, the laid-out graph scopes in `ScopeId` order, filled in `compile_planned` beside `inlined`; `plan::Declined` gains `scope: ScopeId`; `OptimizedBody::positions: Vec<(u32, Position)>` recorded by the emitter: a label at every change of `(block.scope, program_counter)` in `Emitter::block` and `frame_node`, at each poll, trap and trap-out-of-line from its point's innermost scope (`metadata.points[point].scopes[0]`), and `OutOfLine` at the mixed paths, the trampolines and the cold paths. `install_optimized` and `optimize` capture a `CodeListing` with `tier == Optimized`, the key, the scopes (body names and each scope's `instructions` resolved from `heap.code(code_id)` at the flush).
- [ ] **Step 1: Write the failing tests**: `an_optimized_copy_lists_its_scopes` (stage 1's inlining test module, `Demo>>total` inlining `Demo/Account>>balance` under `NS_NEXT_STRESS_OPTIMIZE`'s entry point or the budget run-out stage 1's tests use: the `Optimized` listing's `scopes` hold `s0 Demo>>total` and `s1 Demo/Account>>balance` with `parent == Some((0, pc))`, and `positions` hold an `At { scope: 1, … }`); `the_scopes_of_a_listing_are_the_plans` (the same `scopes` as the `Inlined` events name); `emit.rs`: `positions_ascend_and_cover_the_code` (every offset below `code_end` lies under a position).
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: an optimized copy's listing, its scopes and their positions`

### Task 4: The inlining listing

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/mod.rs` (`PlanDecisions::arms`), `jit/mod.rs` (`trace_inlining` records the listing, the outcome after install or refusal), `trace_events.rs`, `trace_events/listings.rs`
- Test: `crates/ns-runtime/tests/printing.rs`

**Interfaces:**
- Consumes: Task 3's `PlanDecisions::scopes`, `Declined::scope`, `ListedScope`; stage 1's `InlineDecline`, `CompileRefusal`, `key_names`.
- Produces (public): `pub struct InliningListing { pub body: String, pub key: Option<(String, Option<String>)>, pub scopes: Vec<ListedScope>, pub arms: Vec<ListedArm>, pub declined: Vec<ListedDecline>, pub outcome: InliningOutcome }`; `pub struct ListedArm { pub scope: u32, pub program_counter: u32, pub receiver: String, pub inlined: ArmKind }`, `pub enum ArmKind { Scope(u32), Always(&'static str) }` (`AlwaysInlined`'s kind in kebab case: `getter`, `setter`, `lazy-getter`, `quick`, `identity`, `class`, `indexed`, `float`, …, exhaustive); `pub struct ListedDecline { pub scope: u32, pub program_counter: u32, pub callee: Option<String>, pub reason: InlineDecline }`; `pub enum InliningOutcome { Installed { bytes: usize }, Refused(CompileRefusal) }`. `ListedScope::instructions` is empty in an inlining listing.
- Produces (crate): `PlanDecisions::arms: Vec<(ScopeId, u32, DispatchId, ArmPlan-derived ArmKind)>` from `InliningPlan::sites`, in `(scope, program_counter)` order.
- [ ] **Step 1: Write the failing tests**: `an_installed_optimization_prints_its_plan` (`scopes`, an arm `Scope(1)` with `receiver == "Demo/Account"`, `outcome == Installed`); `a_refused_optimization_prints_its_plan` (Review Focus 4); `a_decline_names_its_scope` (stage 1's over-weight decline: `ListedDecline { scope: 0, reason: InlineDecline::OverWeight, … }`).
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: the inlining plan of every optimizing compile, with its outcome`

### Task 5: `ns-next run --print-*`, the decoders and the formats

**Files:**
- Create: `crates/ns-cli/src/listing.rs` (the formatter of all three), `crates/ns-cli/src/listing/x64.rs`, `crates/ns-cli/src/listing/aarch64.rs`
- Modify: `Cargo.toml` (workspace: `yaxpeax-arm`, `yaxpeax-arch` beside `iced-x86`), `crates/ns-assembler/Cargo.toml` (the workspace entries), `crates/ns-cli/Cargo.toml` (`[target.'cfg(target_arch = "x86_64")'.dependencies] iced-x86.workspace = true`, `[target.'cfg(target_arch = "aarch64")'.dependencies] yaxpeax-arm.workspace = true`, `yaxpeax-arch.workspace = true`), `crates/ns-cli/src/main.rs` (`Run`'s four options), `crates/ns-cli/src/commands/run.rs` (`TraceOptions` gains `printing` and `print_filter`; `check_filter` for both), `crates/ns-cli/src/trace.rs` (`StandardErrorSink::listing`)
- Test: `crates/ns-cli/src/listing.rs` (unit), `crates/ns-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1 to 4's `Listing` and its parts.
- Produces: `pub(crate) fn listing::text(listing: &Listing) -> String`, the whole listing with its head and end lines, used by the sink and by Task 7; `--print-bytecode`, `--print-code`, `--print-inlining` (flags) and `--print-filter=<PATTERN>` (repeatable, `requires` one of the three).
- The formats (decision 11, 14), exact:
  - bytecode: `--- bytecode {Demo>>sum:}`, the disassembly, `--- end`.
  - code: `--- code {Demo>>square:} baseline x64 base=0x… bytes=412` (an optimized copy adds ` key={Demo/Account}` and ` entered={…}` as the trace does), then for an optimized copy `scopes:` and the tree, two spaces per level, `s1 {Demo/Account>>balance} at s0@5`; then the code: at each position a line `  s<n> {<body>} @<pc>  <instruction>` (baseline: `  @<pc>  <instruction>`), `  out of line` for `OutOfLine`; each instruction `    +0x<offset:04x>  <decoded>` with an annotation as `  ; leaf miss` / `; stub polymorphic` / `; exit send @12` / `; cell site 3`, a branch target in the body rewritten to `+0x…`; then `data:` and `    +0x<offset:04x>  <word kind>`; `--- end`.
  - inlining: `--- inlining {Demo>>total} key={Demo/Account}`, the scope tree as above, then per arm `  s0@5 {Demo/Account} -> s1` or `-> always getter`, per decline `  s0@9 declined callee={…} reason=over-weight`, then `  installed bytes=…` or `  refused reason=zone-full`, `--- end`.
- [ ] **Step 1: Write the failing tests**: unit tests of `listing::text` over hand-built listings (one per kind, one per annotation kind; the x64 one over bytes of `ret` and a `call` and a relative `jmp` into the body, which prints `+0x…`); `cli.rs`: `print_bytecode_alone_prints` (`--print-bytecode --print-filter=Demo...` over `A_HOT_METHOD`: stdout `338350\n`, stderr holds `--- bytecode {Demo>>square:}`), `print_code_prints_the_hot_method` (with a JIT: a `--- code {Demo>>square:} baseline` head, an `--- end`, every line between them that is an instruction matching `^    \+0x[0-9a-f]{4}  `), `a_listing_is_one_write_beside_the_programs_stderr`, `a_print_filter_naming_no_module_is_refused` (exit 2, ``no module `Other` ``), `a_print_filter_needs_a_print_option` (clap's refusal, exit 2), `a_leafless_body_pins_its_listing` (a method `answer = (^ 42)` hot: the whole code listing between head and end pinned, the head's `base=` masked).
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: ns-next run prints bytecode, machine code and inlining`

### Task 6: The debugger answers a frame's code

**Files:**
- Modify: `crates/ns-runtime/src/debugger/remote.rs` (`Command::FrameCode`, `Event::FrameCode`, `Command::name`), `crates/ns-runtime/src/debugger/mod.rs` (`serve_stop`, `serve_queued_commands`, `configure`, `serve_between_runs`), `crates/ns-runtime/src/jit/mod.rs` (`frame_code_listing`), `crates/ns-runtime/src/trace_events/listings.rs`
- Test: `crates/ns-runtime/tests/debugger.rs` (or the debugger's test module that serves `Variables`)

**Interfaces:**
- Consumes: Task 1's `disassemble_marked`, Task 2's `CodeListing` and the annotation path.
- Produces: `Command::FrameCode { frame: usize }` (named `frame-code`), answered at a stop only, `NO_SUCH_FRAME` past the stack, `RUNNING` while running, "no run executes" between runs; `Event::FrameCode { body: String, bytecode: String, machine: Option<CodeListing> }`: `bytecode` is `disassemble_marked(code, body, Some(program_counter))`; `machine` is `Some` only when `Heap::activation_is_compiled` -- the frame goes on in baseline machine code, never optimized at a stop (`debugger.md`, "The stop") -- the lean body or, for a reified frame, its twin (`jit::installed`, `twin` of it), its bytes read from the zone (`CodeWriter::bytes`), the annotations from assembling the same body again with `annotate` on (`jit::assemble`, the debugger metadata's precedent of compiling again; a debug build asserts the code bytes equal), and `mark` the offset the frame returns to (`ProgramCounterMap`'s return of the program counter `mark_stopped_in_machine_code` stored). `pub(crate) fn jit::frame_code_listing(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, reified: bool, program_counter: u32) -> Option<CodeListing>`, `None` without `cfg(ns_jit)`.
- [ ] **Step 1: Write the failing tests**: `a_stopped_frames_code_is_its_bytecode_marked` (a breakpoint in `Demo>>sum:`: `bytecode`'s `->` line is the stopped instruction's, `machine == None` for the top frame); `a_caller_in_machine_code_lists_it` (`#[cfg(ns_jit)]`, JIT on: a hot caller entered in machine code calling a method with a breakpoint; frame 1's `machine` is `Some`, `tier == Baseline`, `mark` the return offset); `frame_code_past_the_stack_is_refused`.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: the debugger answers a stopped frame's code`

### Task 7: `nsNext/frameCode` and `debug_code`

**Files:**
- Modify: `crates/ns-cli/src/debug/agent.rs` (dispatch, `Conversion::FrameCode`, `answers`, `convert`), `crates/ns-cli/src/debug/protocol.rs` (`FrameCodeArguments { frame_id: i64 }`), `crates/ns-cli/src/listing.rs` (a `CodeListing`'s `mark` line begins `->`), `crates/ns-cli/src/mcp/tools.rs` (`debug_code`), `crates/ns-cli/src/mcp/session.rs` (`Session::code`)
- Test: `crates/ns-cli/tests/debug_adapter.rs`, `crates/ns-cli/tests/mcp.rs`

**Interfaces:**
- Consumes: Task 6's command and event, Task 5's `listing::text`.
- Produces: the request `nsNext/frameCode { frameId }` at a stop (`at_stop`, `frame_of_id`), answering `{ "body": "<name>", "bytecode": "<text>", "machineCode": "<text>" | null }`, `machineCode` formatted by `listing::text`; the MCP tool `debug_code { frame }` (the shared `frame()` schema, 0 the top), answering the bytecode, then the machine code or the line `the frame goes on in the interpreter`, through `with_output`.
- [ ] **Step 1: Write the failing tests**: `debug_adapter.rs`: `frame_code_marks_the_stopped_instruction` (`client.succeed("nsNext/frameCode", json!({"frameId": 1}))`: `bytecode` holds a line starting `->`; `frameId: 9` fails); `mcp.rs`: `TOOLS` grows to 17 with `("debug_code", &[], &["frame"])` in list order, and `debug_code_answers_the_bytecode` (stop at a breakpoint, `debug_code` with no arguments: the answer holds `--- bytecode {` and `->`).
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**; `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: nsNext/frameCode and the MCP tool debug_code`

### Task 8: Documents and the end of the batch

**Files:**
- Modify: `docs/design/developer-tool.md` (stage 2 **Built**, "Stage 2: printing code" moved out of "Planned" into a section of what exists, the formats and tests as stage 1's section has them, the head's **Partial** paragraph), `docs/design/debugger.md` (the agent's own requests list, `nsNext/frameCode`; "The MCP server"'s tools, `debug_code`), `docs/design/execution.md` (the macro assembler's annotations, one paragraph where the macro assembler is described), `docs/design/editor-support.md` and the plugin's `NewspeakBundle.properties` runtime-options comment only if they list `--trace` as the example of every runtime option (name `--print-code` beside it), `CLAUDE.md` (`ns-cli`'s `run` takes `--print-*`; the agent's `nsNext/frameCode`; `ns-assembler`'s "dev-dependencies only" and `ns-cli`'s decoders), `docs/OPEN-WORK.md` (stage 2 closed, stage 3 next for the tool), `docs/PROGRESS.md`
- [ ] **Step 1: Write the documents**, present tense, as stage 1's were.
- [ ] **Step 2: Run** `cargo xtask gates commit`, then `cargo xtask gates end-of-batch` (`timeout: 600000`, in the foreground; it is long, run it once); every failure is fixed and the group run again.
- [ ] **Step 3: Commit** -- `docs: the developer tool's stage 2, printing code`
