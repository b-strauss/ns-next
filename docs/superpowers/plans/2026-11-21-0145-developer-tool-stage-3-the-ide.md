# The Developer Tool, Stage 3: The IDE — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ns-next run --trace-file=<path>` writes the trace and the listings as typed JSON lines, every body described once with its source positions and a checksum; the JetBrains plugin switches them on from a "Developer Tool" tab of the run configuration and a project-wide list of watched methods, reads the file live in Run and Debug, and shows it in a tool window (Methods, Events, Code, Inlining; a tab per run), as gutter state icons and inlay hints at the sends, in read-only code editor tabs, and, at a stop, as the frame's code.

**Architecture:** The runtime gives every body the trace names a number of its run and, when the host asks for descriptions, describes it once -- its module, its source range, a checksum of its source text and its statements' and sends' ranges, from the debugger's metadata -- through a new `TraceSink::body` before the first event or listing that names it; every name a host receives becomes a `BodyName { number, name }`. `ns-cli` adds a second sink, `TraceFileSink`, which writes header, body, event, bytecode, code (decoded by the existing decoders) and inlining records, converting byte offsets to the 0-based line and UTF-16 column the language server and the DAP agent use. The plugin reads the file with a tailing reader into a per-run model, from which a tool window and editor decorations are drawn; positions come from the file, never from a parser of its own, which the plugin does not have.

**Tech Stack:** Rust 1.100.0 (edition 2024), `cargo-nextest`, `serde_json` (already `ns-cli`'s); Kotlin 2.4.10, IntelliJ Platform 2026.2.3 (`sinceBuild` 261.26222), the platform's Gson, JUnit 4 with `BasePlatformTestCase`; no new dependency.

**Spec:** `docs/design/developer-tool.md` -- decisions 17 to 33, "Planned: the stages", "Stage 3: the developer tool in the IDE"; "Stage 1: tracing" and "Stage 2: printing code" for what this builds on; `docs/design/debugger.md`, "The JetBrains plugin", for the debug process this extends.

## Global Constraints

- **Nothing on the hot path.** Body numbers are minted in `BodyNames` at the flush; descriptions are made only when `TraceSettings::describe_bodies` is set (only `--trace-file` sets it). No site, template, stub or leaf changes. A commit touching `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs` runs `cargo xtask gates commit codegen`, the budgets unchanged.
- **No handle in anything a host receives**; descriptions are made at the flush from `Rc<CompiledCode>` and the mixin (`CLAUDE.md`, "A handle means nothing…"). Never flushed in a leaf.
- **A body number is unique for a run and never reused**, even when a body dies and its `Rc` address is reused (`BodyNames` keys by `Rc::as_ptr` with a `Weak`; a dead entry gets a new number).
- **The text trace is unchanged**: every line and listing `ns-next run --trace`/`--print-*` writes today stays byte for byte as it is; `trace.rs`'s and `listing.rs`'s unit tests and the CLI tests hold it.
- **`--trace-file` replaces standard error** (decision 20) and needs `--trace` or a print option; a path that cannot be created is refused with exit status 2 before the program runs, as a malformed filter is (`commands/run.rs`'s `refuse_trace`).
- **The record format is exactly:** one JSON object per line, `\n`-terminated, each written with one `write_all` and flushed at each `TraceSink` delivery batch end; field names in the text trace's kebab-case (`in-body`, `at`, `spent-by`, `walk-end`, `loop-head`, `inlined-scopes`, …); every body-valued field a body number. Records:
  - `{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"|"aarch64"|null}`
  - `{"kind":"body","body":N,"name":"Demo>>main:args:","module":{"id":"Demo","version":null},"source":{"file":"<absolute path>"}|{"library":"platform/TimeFactory.ns"}|null,"range":[startLine,startColumn,endLine,endColumn],"checksum":"<16 hex digits>","statements":[{"instructions":[from,to],"range":[…]}],"sends":[{"at":index,"site":n|null,"call":[…],"selector":[[…],…]}]}`
  - `{"kind":"event","category":"inlining","body":N|null,"event":"declined",…fields}`
  - `{"kind":"bytecode","body":N,"bodies":[{"body":N,"instructions":[{"index":0,"statement":0|null,"text":"…"}]}]}`
  - `{"kind":"code","body":N,"tier":"baseline"|"twin"|"optimized","key":"<application>"|null,"entered":"<application>"|null,"architecture":"x64","bytes":n,"instructions":[{"offset":0,"text":"…","scope":s|null,"at":pc|null,"annotation":"leaf miss"|null}],"data":[{"offset":0,"word":"budget word"}],"scopes":[{"scope":0,"body":N,"parent":[scope,at]|null}],"mark":offset|null}`
  - `{"kind":"inlining","body":N,"key":"<application>"|null,"entered":"<application>"|null,"scopes":[…],"arms":[{"scope":s,"at":pc,"receiver":"…","inlined":{"scope":s}|{"always":"identity"}}],"declined":[{"scope":s,"at":pc,"callee":N|null,"reason":"over-weight"}],"outcome":{"installed":bytes}|{"refused":"zone-full"}}`
  - Ranges are `[startLine, startColumn, endLine, endColumn]`, 0-based, columns in UTF-16 code units (`ns_tooling::lines::LineIndex::position`); the checksum is FNV-1a 64 (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`) over the UTF-8 bytes of the body's source range, lower-case hex, 16 digits.
- **The plugin parses no Newspeak for positions**: every range it decorates comes from a `body` record; a method's tool name and range for a watch entry come from the analyzer's new request `newspeak/methodNames` (Task 5).
- **Plugin threading**: no file I/O or JSON parsing on the EDT; the reader thread publishes batches at most every 200 ms through `ApplicationManager.getApplication().invokeLater`; models are read on the EDT only.
- **Kotlin names written out** as the Rust ones are (`configuration`, `document`, `offset`, never `cfg`, `doc`, `off`); bundle keys under `devtool.` in `NewspeakBundle.properties`; icons as `AllIcons` where one fits, new SVGs under `icons/devtool/` with `_dark` variants.
- **Tests**: runtime tests that pin machine code follow stage 2's (`#![cfg(ns_jit)]`, `set_jit_enabled(true)`, breakpoint/step/caches/zone/collect stress modes off); CLI tests use `plain_run`; plugin tests are `BasePlatformTestCase` under `test`, except one that starts `ns-next`, which gets its own Gradle task through `runsLikeUnitTests` and `debugsWithTheRuntime()` (`build.gradle.kts`).
- Names written out; never `{:?}` a `Value`; a Newspeak module inline in a test is an `indoc! {"…"}`; comments explain the decision.
- Every commit runs `cargo xtask gates commit` (+ `analyzer` for Task 5, + `codegen` as above). The batch ends with `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification` (Task 12); every commit before is work in progress. Subagents run every command in the foreground with `timeout: 600000` on every long one.

## Review Focus

1. **A run killed mid-write** (the user presses Stop): the file's last line is incomplete. The reader keeps it pending, never reports a parse error, and the tab keeps everything before it. Pinned in Task 6 (`a_partial_last_line_waits_for_its_end`).
2. **Two bodies with one name** (the blocks of one method, a class's three initializer bodies): each has its own number and range, and each gets its own decorations. Pinned in Task 1 (`blocks_of_one_method_get_numbers_of_their_own`) and Task 9 (`each_block_is_decorated_at_its_own_range`).
3. **Non-ASCII source** (umlauts, an emoji before a send): ranges in UTF-16 columns and the checksum over UTF-8 match what the plugin computes from the document. Pinned in Task 3 (`ranges_count_utf16_columns`) and Task 9 (`the_checksum_matches_a_document_with_an_emoji`).
4. **An unfiltered run with listings** (the whole core library's bytecode): the reader and the Events table stay responsive. Pinned in Task 6 (`a_large_file_is_read_in_batches`, 200,000 records, no batch over 5,000 records).
5. **A trace file path the runtime cannot create** (a missing directory): exit status 2 before the program runs, nothing on standard output. Pinned in Task 3 (`an_uncreatable_trace_file_is_refused`).

---

### Task 1: Body numbers and descriptions in the runtime

**Files:**
- Modify: `crates/ns-runtime/src/trace_events.rs` (`TraceEvent`, every body-valued field of `TraceEventKind`, `TraceSink`, `TraceSettings`, `flush`, `key_names`)
- Modify: `crates/ns-runtime/src/trace_events/names.rs` (`BodyNames`)
- Modify: `crates/ns-runtime/src/trace_events/listings.rs` (every `body: String` and `callee: Option<String>`)
- Create: `crates/ns-runtime/src/trace_events/descriptions.rs`
- Modify: `crates/ns-cli/src/trace.rs`, `crates/ns-cli/src/listing.rs`, `crates/ns-cli/src/debug/agent.rs` (read `.name` where they read the string)
- Test: `crates/ns-runtime/tests/trace.rs`, `crates/ns-runtime/src/trace_events/names.rs`'s unit tests

**Interfaces:**
- Produces:
  - `pub struct BodyName { pub number: u32, pub name: String }` (`trace_events`), `Clone`, `Debug`, `PartialEq`, `Eq`; it replaces `String` in `TraceEvent::body` (`Option<BodyName>`), in every `TraceEventKind` field that names a body (`spent_by`, `in_body`, `callee`), and in `BytecodeListing::body`, `CodeListing::body`, `ListedScope::body`, `InliningListing::body`, `ListedDecline::callee` (`Option<BodyName>`). What names an application -- `key`, `entered`, `receiver` -- stays a `String`.
  - `pub struct BodyDescription { pub body: BodyName, pub module: ModuleName, pub shipped_path: Option<String>, pub source: ns_syntax::Span, pub checksum: u64, pub statements: Vec<DescribedStatement>, pub sends: Vec<DescribedSend> }`; `pub struct DescribedStatement { pub instructions: std::ops::Range<u32>, pub span: Span }`; `pub struct DescribedSend { pub index: u32, pub site: Option<u32>, pub call: Span, pub selector: Vec<Span> }`. Spans are byte offsets into the module's text; `shipped_path` is the `ShippedSourceFile::path` of a shipped module (`"Kernel.ns"`, `"platform/TimeFactory.ns"`), `None` for a program's.
  - `TraceSink::body(&mut self, description: &BodyDescription) { let _ = description; }`, a provided method, called once per body number before the first event or listing naming it, only when `TraceSettings::describe_bodies` is set; the module's text is not passed -- `ns-cli` holds every text itself (Task 3).
  - `TraceSettings::describe_bodies: bool` (default `false` in every existing constructor).
  - `pub fn source_checksum(text: &[u8]) -> u64` (`trace_events::descriptions`), FNV-1a 64 as the constraints say.
- Consumes: `debugger::metadata::Cache::metadata_with_text` (cache.rs:63), `BodyOrigin`/`Member` (again.rs:30, 43), `names.rs`'s `name_side` facts (mixin, member, blocks via `created_blocks()`), `again::module_name` (again.rs:236).

- [ ] **Step 1: Write the failing tests** in `crates/ns-runtime/tests/trace.rs`:
  - `bodies_are_numbered_once_per_run`: run `HOT` with `compile` traced; collect `event.body`; the same name always carries the same number, two names never share one, numbers start at 1.
  - `blocks_of_one_method_get_numbers_of_their_own`: a method with two blocks, both hot (`[:each | sum := sum + each]` and `[:each | count := count + 1]` over `1 to: 1000`); two `compiled` events whose names are both `[] in Demo>>run` carry different numbers.
  - `a_description_precedes_the_first_mention`: with `describe_bodies: true`, a collector recording `body` and `event` calls in order: for every event naming body N, a `body` call with N came before; every number is described exactly once.
  - `a_description_carries_ranges_and_sends`: `Demo>>run`'s description has `module.id == "Demo"`, `shipped_path == None`, `source` covering `run = (` … `)` in the program text, at least one statement, and a send whose `selector` spans read `"to:"` and `"do:"` in the text, and `checksum == source_checksum(&text.as_bytes()[source])`.
  - `a_shipped_body_names_its_library_path`: a `compiled` event for a `Kernel` body (any `newspeak.core.Kernel/...` name) has a description with `shipped_path == Some("Kernel.ns")`.
  - `nothing_is_described_unless_asked`: `describe_bodies: false` → no `body` call, events still numbered.
  - In `names.rs`'s unit tests: `a_dead_body_never_lends_its_number`: name a body, drop every `Rc` of it, name a new body (which may reuse the address): its number differs.
- [ ] **Step 2: Run them to verify they fail**: `cargo nextest run -p ns-runtime --test trace bodies_are blocks_of a_description a_shipped nothing_is` — FAIL to compile (`BodyName` missing).
- [ ] **Step 3: Implement.** `BodyNames` keeps `number` beside each entry and a `next_number: u32` starting at 1; `name` answers `BodyName`. `descriptions.rs` holds `describe(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, body: &BodyName) -> Option<BodyDescription>`: it finds the body's `BodyOrigin` from the side `name_side` used, asks `Mutator.debug_metadata.metadata_with_text`, and maps `StatementEntry`/`SendEntry` (`call_site` to its site number) into the described types; a body whose metadata is not alike (`None`) is still described, with empty `statements` and `sends` and its `code.source`. `flush` calls `sink.body` for every number minted in this flush, in minting order, before delivering the first event or listing that names it. Update the CLI's text sink and formatter to print `.name` — their output does not change.
- [ ] **Step 4: Run the tests**: the command of Step 2, plus `cargo nextest run -p ns-cli trace:: listing::` (the text is unchanged) — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: the trace numbers its bodies and describes them on request`.

### Task 2: The bytecode listing, structured

**Files:**
- Modify: `crates/ns-runtime/src/trace_events/listings.rs` (`BytecodeListing`), `crates/ns-runtime/src/trace_events.rs` (where it is built), `crates/ns-runtime/src/bytecode.rs` (`instruction_lines` beside a structured form)
- Test: `crates/ns-runtime/tests/printing.rs`

**Interfaces:**
- Produces: `BytecodeListing { pub body: BodyName, pub text: String, pub bodies: Vec<ListedBody> }`, `pub struct ListedBody { pub body: BodyName, pub instructions: Vec<ListedInstruction> }`, `pub struct ListedInstruction { pub index: u32, pub statement: Option<u32>, pub text: String }` — the root first, then its blocks in `disassemble`'s order; `text` the instruction alone, as `instruction_lines` prints it after its index and statement columns. `bytecode::listed_instructions(code: &CompiledCode) -> Vec<(u32, Option<u32>, String)>`, which `instruction_lines` then formats, so that both share one walk.
- Consumes: Task 1's `BodyName`.

- [ ] **Step 1: Write the failing test** `a_bytecode_listing_lists_each_body_structured` in `printing.rs`: print `Demo>>run` (one block); `listing.bodies.len() == 2`; `bodies[0].body == listing.body`; `bodies[1].body.name == "[] in Demo>>run"` with a number of its own; every `instructions[i].index == i`; and joining each instruction as `instruction_lines` does reproduces the lines of `listing.text` for that body.
- [ ] **Step 2: Run it**: `cargo nextest run -p ns-runtime --test printing a_bytecode_listing_lists` — FAIL.
- [ ] **Step 3: Implement** as the Interfaces say; `text` stays exactly as built today.
- [ ] **Step 4: Run** Step 2's command and `cargo nextest run -p ns-cli listing::` — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: a bytecode listing lists each body's instructions`.

### Task 3: `--trace-file`, the JSON lines writer

**Files:**
- Create: `crates/ns-cli/src/trace_file.rs`
- Modify: `crates/ns-cli/src/main.rs` (`Command::Run`: `trace_file: Option<PathBuf>`, `requires` one of `trace`/the `printing` group), `crates/ns-cli/src/commands/run.rs` (`TraceOptions::file`, the sink choice, `describe_bodies: true` with a file), `crates/ns-cli/src/listing.rs` (`Decoded`, `decoded`, `annotation_text`, `word_text` become `pub(crate)`)
- Test: `crates/ns-cli/src/trace_file.rs`'s unit tests, `crates/ns-cli/tests/cli.rs`

**Interfaces:**
- Produces: `pub(crate) struct TraceFileSink { file: std::io::BufWriter<std::fs::File>, modules: ModuleTexts }` implementing `TraceSink`; `pub(crate) fn create(path: &Path, modules: ModuleTexts) -> Result<TraceFileSink, String>` writes the header; `pub(crate) struct ModuleTexts` maps a `ModuleName` to its text and its absolute path (from `program::debug_modules`) and a shipped path to `ns_runtime::shipped_source_files()`'s text, caching a `LineIndex` per module; `pub(crate) fn event_record(event: &TraceEvent) -> serde_json::Value`, `pub(crate) fn listing_record(listing: &Listing) -> serde_json::Value` (a `code` listing decoded through `listing::decoded`) and `pub(crate) fn body_record(description: &BodyDescription, texts: &mut ModuleTexts) -> serde_json::Value`, each an exhaustive `match` (no wildcard), so a new event kind cannot be forgotten.
- Consumes: Tasks 1 and 2; `trace.rs`'s field names (the record's keys are the text line's names); `listing::decoded`.

- [ ] **Step 1: Write the failing unit tests** in `trace_file.rs`:
  - `every_event_has_a_record`: for each event of `trace.rs`'s `every_event()`, `event_record` has `"kind":"event"`, `"category"`, `"event"` equal to `kind.name()`, and every field the text line writes, under the same name; body-valued fields are numbers.
  - `a_record_is_one_line`: no record's serialization contains `\n`.
  - `ranges_count_utf16_columns`: `body_record` of a description over the text `"Demo = (\n  run = ( 'ä😀' size )\n)"`: the send `size`'s selector range is `[1, 15, 1, 19]` (the emoji counts two UTF-16 units).
  - `a_shipped_body_points_at_the_library`: a description with `shipped_path: Some("platform/TimeFactory.ns")` → `"source":{"library":"platform/TimeFactory.ns"}`.
  - `machine_code_is_decoded_per_instruction` (`#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]`): stage 2's pinned leafless `CodeListing` gives `instructions` whose `text`s are, in order, the instruction texts of its pinned text listing, each `offset` that line's offset.
- [ ] **Step 2: Write the failing CLI tests** in `cli.rs`:
  - `trace_file_holds_header_bodies_and_events`: `plain_run` of `A_HOT_METHOD` with `--trace=compile --print-bytecode --print-filter=Demo... --trace-file=<scratch>/trace.jsonl`; exit 0; standard error holds no `[compile]` line and no `--- bytecode`; the file's first record is the header with `"version":1`; every record parses; every `body` number used is described earlier in the file; a `compiled` event names `Demo>>…`'s number.
  - `trace_file_needs_something_to_trace`: `--trace-file=x.jsonl` alone → exit 2 (clap's requirement).
  - `an_uncreatable_trace_file_is_refused`: `--trace=compile --trace-file=<scratch>/missing/trace.jsonl` → exit 2, standard output empty, standard error naming the path, the program not run.
- [ ] **Step 3: Run them**: `cargo nextest run -p ns-cli trace_file` — FAIL.
- [ ] **Step 4: Implement.** The file is created after the filters are checked and before `set_trace`; each record is serialized to one string and written with one `write_all` plus `\n`; the writer is flushed at the end of every delivery batch — implement `TraceSink::flushed(&mut self) {}` as a provided method in `trace_events.rs`, called by `flush` after its last delivery, so the file is current at every safepoint and every stop.
- [ ] **Step 5: Run** Step 3's command and `cargo nextest run -p ns-cli trace::` — PASS.
- [ ] **Step 6: Document** the format in `docs/design/developer-tool.md`, a new section "Stage 3: the developer tool in the IDE" › "The trace file", every record and field as the constraints list them; the section's status says **Partial** until Task 12.
- [ ] **Step 7: Commit** after `cargo xtask gates commit`: `feat: ns-next run --trace-file writes the trace as JSON lines`.

### Task 4: The run configuration's "Developer Tool" tab

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/devtool/DeveloperToolOptions.kt`, `devtool/DeveloperToolTab.kt`
- Modify: `run/NewspeakRunConfiguration.kt` (options, `getConfigurationEditor` as a `SettingsEditorGroup` of the existing editor and the new tab, `checkConfiguration`, `newspeakCommandLine`), `run/NewspeakRunConfigurationEditor.kt` (the field's label "Further runtime options"), `messages/NewspeakBundle.properties`
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfigurationTest.kt`

**Interfaces:**
- Produces: in `NewspeakRunConfigurationOptions` the persisted `traceCategories: MutableList<String>` (category names, `compile` … `caches`), `printBytecode`, `printCode`, `printInlining: Boolean`; `fun NewspeakRunConfiguration.tracesAnything(): Boolean`; `newspeakCommandLine(runtime, configuration, debugAddress, traceFile: Path? = null, filters: TraceFilters = TraceFilters.EVERYTHING)` adds, after `--debug-connect` and before the free options: `--trace=<names joined by ,>` (or `--trace=all` when all seven), `--trace-filter=<pattern>` per events pattern, `--print-bytecode`/`--print-code`/`--print-inlining`, `--print-filter=<pattern>` per listings pattern, `--trace-file=<path>`. `data class TraceFilters(val events: List<String>, val listings: List<String>, val noEvents: Boolean, val noListings: Boolean) { companion object { val EVERYTHING = TraceFilters(emptyList(), emptyList(), false, false) } }` lives in `DeveloperToolOptions.kt`; `noEvents` drops `--trace` and `noListings` the print options; Task 5's `WatchedMethods.filters()` makes it, until then the default.
- Each checkbox's tooltip is a bundle sentence `devtool.category.<name>.tooltip` / `devtool.listing.<kind>.tooltip` saying what it holds (e.g. `compile`: "Every baseline compile, its refusal and hold, abandonment and revival, with its reason."; `code-zone`: "Compactions of the code zone and the machine code they free."; listing code: "The machine code of every install -- baseline, twin and optimized -- with its annotations.").

- [ ] **Step 1: Write the failing tests** in `NewspeakRunConfigurationTest`:
  - `testCheckedCategoriesBecomeOneTraceOption`: `compile`, `inlining` checked → the command line holds `--trace=compile,inlining` once, after the subcommand, before the free options.
  - `testAllSevenAreAll`: all checked → `--trace=all`.
  - `testListingKindsBecomeTheirOptions`: `printCode` → `--print-code`, no `--trace`.
  - `testATraceFileIsPassedWithItsPath`: with a `traceFile` → `--trace-file=<path>` present; without → absent.
  - `testAnOptionWithAControlInTheFreeFieldIsAnError`: free field `--trace=compile` → `checkConfiguration` throws `RuntimeConfigurationError` naming `--trace`; likewise `--print-code`, `--trace-filter=…`, `--trace-file=…`; `--timed` passes.
  - `testBenchGreysTheTab`: the tab's checkboxes are disabled when the command is `bench`.
  - `testTheTabGivesBackWhatItWasShown`: reset → apply round trip keeps categories and kinds.
  - `testEveryCheckboxHasItsSentence`: every checkbox's `toolTipText` is non-empty and equals its bundle key.
- [ ] **Step 2: Run**: `editors/jetbrains/gradlew --no-daemon test --tests '*NewspeakRunConfigurationTest*'` — FAIL.
- [ ] **Step 3: Implement**; the tab in the Kotlin UI DSL, seven checkboxes and "All", three listing checkboxes, a link "Manage watched methods…" (opens Task 5's dialog; until then disabled).
- [ ] **Step 4: Run** Step 2's command — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): a Developer Tool tab in the run configuration`.

### Task 5: Watched methods

**Files:**
- Create: `devtool/WatchedMethods.kt` (project service, `PersistentStateComponent`, `@Storage(StoragePathMacros.WORKSPACE_FILE)`), `devtool/WatchedMethodsDialog.kt`, `devtool/WatchMethodAction.kt` (editor popup and gutter popup), `devtool/WatchMethodIntention.kt`
- Modify: `crates/ns-analyzer/src/` (a request `newspeak/methodName`, beside `breakable.rs`), `lsp/NewspeakLanguageServer.kt` (its `@JsonRequest`), `plugin.xml`, the bundle
- Test: `crates/ns-analyzer/tests/` (beside the breakable-lines tests), `editors/jetbrains/src/test/kotlin/.../devtool/WatchedMethodsTest.kt`

**Interfaces:**
- Produces:
  - Analyzer: `newspeak/methodNames` with params `{"textDocument":{"uri"}}` answering `[{"name":"<id>[/<Inner>(.<Inner>)*][ class]>><selector>","range":Range,"selectionRange":Range}]` (LSP ranges) for every method of the document, in source order; the id is the one the loader gives the module (`ns_runtime::modules::declared_identity`'s rule: the `:module:` id, else the class's name), the version left out (a name without a version covers every version). One request per document, cached by the plugin with the document's stamp as `DocumentSymbolCache` caches its symbols.
  - Plugin: `class WatchedMethods(project) { data class Entry(var pattern: String, var events: Boolean = true, var listings: Boolean = true); val entries: List<Entry>; fun watch(pattern: String); fun unwatch(pattern: String); fun isWatched(name: String): Boolean; fun filters(): TraceFilters }` (Task 4's type) — `filters()` answers `TraceFilters.EVERYTHING` for an empty list, and otherwise the patterns of the entries with each switch on; when the list is non-empty and no entry has `events` on, `noEvents` is true, so that no `--trace` is passed (an empty pattern list would mean everything); likewise `noListings`. `fun methodNames(file: VirtualFile): List<MethodName>` (`data class MethodName(val name: String, val range: TextRange, val selection: TextRange)`) wraps the request.
  - Patterns are the tools' syntax; the project's `newspeak.json` `as` of the file's module entry replaces the analyzer's id when present (read with Gson from the project file next to the run configuration's program).
- [ ] **Step 1: Write the failing analyzer test** `method_names_answer_the_tool_names`: a module `(* :module: com.acme.Demo *)` with an inner class `Account` and a class-side method answers, in order, `com.acme.Demo>>run`, `com.acme.Demo/Account>>deposit:`, `com.acme.Demo/Account class>>create`, each with its method's range and its pattern's selection range; a module without `:module:` named `Demo` answers `Demo>>run`.
- [ ] **Step 2: Write the failing plugin tests** in `WatchedMethodsTest`: `testWatchAddsAnEntryWithBothSwitches`, `testFiltersOfAnEmptyListAreEverything`, `testFiltersFollowTheSwitches`, `testTheListIsKeptInTheWorkspace` (`getState`/`loadState` round trip), `testAPrefixPatternWatchesItsMethods` (`isWatched("Demo>>square:")` for the entry `Demo>>sq...`).
- [ ] **Step 3: Run**: `cargo nextest run -p ns-analyzer method_names` and `gradlew --no-daemon test --tests '*WatchedMethodsTest*'` — FAIL.
- [ ] **Step 4: Implement**; the actions find the method whose `range` holds the caret (or the gutter's line) in `methodNames` and `watch`/`unwatch` its name; the dialog is a `TableView` of entries with the two switches and add/remove; Task 4's link and `newspeakCommandLine` now use `filters()`.
- [ ] **Step 5: Run** Step 3's commands — PASS.
- [ ] **Step 6: Commit** after `cargo xtask gates commit analyzer`: `feat: watched methods, named by the analyzer`.

### Task 6: The trace file reader and the run's model

**Files:**
- Create: `devtool/model/TraceRecords.kt` (data classes of the records), `devtool/model/TraceRun.kt` (one run's model), `devtool/model/TraceFileReader.kt`
- Test: `devtool/model/TraceFileReaderTest.kt`, `devtool/model/TraceRunTest.kt`

**Interfaces:**
- Produces:
  - `sealed interface TraceRecord` with `Header(version: Int, architecture: String?)`, `Body(number: Int, name: String, moduleId: String, moduleVersion: String?, file: String?, library: String?, range: SourceRange, checksum: String, statements: List<Statement>, sends: List<Send>)`, `Event(category: String, body: Int?, event: String, fields: JsonObject)`, `Bytecode(…)`, `Code(…)`, `Inlining(…)` mirroring the constraints' records; `data class SourceRange(val startLine: Int, val startColumn: Int, val endLine: Int, val endColumn: Int)`.
  - `class TraceFileReader(path: Path, onBatch: (List<TraceRecord>) -> Unit, onError: (String) -> Unit) : Disposable` — a pooled thread that reads appended bytes, splits at `\n`, keeps an incomplete tail pending, parses with Gson, refuses a header whose `version` is not 1 with `onError(NewspeakBundle.message("devtool.file.version", version))` and stops; publishes a batch at most every 200 ms or 5,000 records; `finish()` after the process ends reads to the end once more.
  - `class TraceRun(val name: String, val started: Instant)` — `bodies: Map<Int, Body>`, `events: List<Event>`, `bytecode`/`code`/`inlining` by body number, `fun state(body: Int): MethodState`, `fun sends(body: Int): Map<Int /*at*/, SendOutcome>`, `fun add(batch: List<TraceRecord>)`, listeners `onChanged`; `enum class MethodState { INTERPRETED, BASELINE, TWIN, OPTIMIZED, DEOPTIMIZED, ABANDONED }` decided from the body's events in order: `compiled` → BASELINE, `compiled-twin` → TWIN, `optimized` → OPTIMIZED (copies counted, `retired`/`invalidated`/`freed tier=optimized` uncounting), no copy left after one → DEOPTIMIZED, `abandoned` (until `revived`) → ABANDONED, none → INTERPRETED. `data class SendOutcome(val inlining: String?, val cache: String?)`: `inlining` is `"inlined"`, `"declined: <reason>"` from `inlined`/`declined` events whose `in-body` is the body and `at` the send's index; `cache` the `after` state's short form (`mono`, `poly <n>` from the cache transitions' count of receivers seen, `mega`) of the last `cache` transition whose `site` is the send's site.
- [ ] **Step 1: Write the failing tests**:
  - `testRecordsParseIntoTheirKinds` (one of each record kind from a fixture file `src/test/resources/devtool/trace.jsonl`, written to the constraints' examples).
  - `a_partial_last_line_waits_for_its_end` (as `testAPartialLastLineWaitsForItsEnd`): a file whose last line lacks `\n` → no error, the record arrives after the rest is appended.
  - `testAnUnknownVersionIsRefused`.
  - `a_large_file_is_read_in_batches` (`testALargeFileIsReadInBatches`): 200,000 event records → every batch ≤ 5,000, all arrive, order kept.
  - `TraceRunTest`: `testStateFollowsTheEvents` (compiled → BASELINE; optimized → OPTIMIZED; retired the only copy → DEOPTIMIZED; abandoned → ABANDONED; revived → BASELINE), `testASendCollectsItsInliningAndItsCache`.
- [ ] **Step 2: Run**: `gradlew --no-daemon test --tests '*devtool.model*'` — FAIL.
- [ ] **Step 3: Implement** as the Interfaces say.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): the trace file's reader and a run's model`.

### Task 7: Traced runs: the temporary file, the tool window and its tabs per run

**Files:**
- Create: `devtool/TracedRuns.kt` (project service), `devtool/ui/DeveloperToolWindowFactory.kt`, `devtool/ui/RunPanel.kt`
- Modify: `run/NewspeakRunConfiguration.kt` (`NewspeakRunState.startProcess`: when `tracesAnything()`, a temporary file under `PathManager.getTempPath()/newspeak-trace/`, passed to `newspeakCommandLine`, and `TracedRuns.started(configuration, path, processHandler)`), `debugger/NewspeakDebugRunner.kt` (the same for Debug), `plugin.xml` (`toolWindow id="Newspeak JIT" anchor="bottom" secondary="false"`, icon), the bundle
- Test: `devtool/TracedRunsTest.kt`

**Interfaces:**
- Produces: `class TracedRuns(project) { val runs: List<TraceRun>; var selected: TraceRun?; fun started(configuration: NewspeakRunConfiguration, file: Path, handler: ProcessHandler): TraceRun; fun close(run: TraceRun); fun addListener(listener: () -> Unit, parent: Disposable) }` — `started` creates the `TraceRun` named `"<configuration> <HH:mm:ss>"`, a `TraceFileReader` feeding it, a tool-window content tab per run (closing the tab calls `close`, which disposes the reader and deletes the file), selects it, and calls `finish()` on the reader when the process terminates. `RunPanel(run)` holds a `JBTabbedPane` with the four tabs of Tasks 8 and 10 (Code), each added by those tasks; this task adds the panel with placeholders.
- [ ] **Step 1: Write the failing tests**: `testAStartedRunGetsItsTabAndIsSelected`, `testClosingTheTabDeletesTheFile`, `testTheNewestRunIsSelected`, `testARunWithoutTracingStartsNothing` (`tracesAnything() == false` → no file, no tab).
- [ ] **Step 2: Run**: `gradlew --no-daemon test --tests '*TracedRunsTest*'` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): traced runs and the Newspeak JIT tool window`.

### Task 8: The Methods, Events and Inlining tabs

**Files:**
- Create: `devtool/ui/MethodsTab.kt`, `devtool/ui/EventsTab.kt`, `devtool/ui/InliningTab.kt`, `devtool/ui/Navigation.kt` (`fun navigate(project, run: TraceRun, body: Int, at: Int?)` opening the body's file — a program file by its absolute path, a library file under `NewspeakCoreLibrary.directory` by its relative path — at the send's call range, or the body's range)
- Modify: `devtool/ui/RunPanel.kt`, the bundle
- Test: `devtool/ui/TabsTest.kt`

**Interfaces:**
- Produces: `MethodsTab(run)`: a `TableView` of root bodies (names without `[] in`), columns State, Copies, Deopts, Abandons, Code bytes (largest copy), Declined callees, sortable, double click → `navigate`. `EventsTab(run)`: a `TableView` (Time order, Category, Body, Event, Fields as `name=value…` in the text trace's order) with a category filter (checkbox per category), a body filter (text field, a substring of the name), a "Group by method" toggle (a tree table) and a "Text" toggle that shows the same rows as the text trace's lines in a read-only editor; double click → `navigate(body, at)` where the event has `at`. `InliningTab(run)`: a `Tree` per `inlining` record — scopes as nodes under their parents, arms and declines as leaves (`declined <callee> <reason>`), the outcome as the root's suffix — click → `navigate(scope's body, at)`.
- [ ] **Step 1: Write the failing tests** over a `TraceRun` built from the Task 6 fixture: `testMethodsListsEachRootWithItsState`, `testMethodsSortByCopies`, `testEventsFilterByCategory`, `testEventsTextViewMatchesTheTextTrace` (the fixture's `declined` event shows as the text trace's line `[inlining] {Demo>>run} declined in-body={Demo>>run} callee={Demo>>helper} reason=over-weight at=7`), `testInliningTreeNestsScopesUnderTheirParents`, `testNavigationOpensALibraryFileUnderTheLibraryDirectory`.
- [ ] **Step 2: Run**: `gradlew --no-daemon test --tests '*devtool.ui.TabsTest*'` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): the Methods, Events and Inlining tabs`.

### Task 9: Gutter state icons and inlay hints

**Files:**
- Create: `devtool/editor/DecorationsService.kt` (project service, listening to `TracedRuns` and to `EditorFactory`'s editors), `devtool/editor/StateGutterRenderer.kt`, `devtool/editor/SendInlayRenderer.kt`, `devtool/editor/DeveloperToolDecorationsSettings.kt` (application settings: show state icons, show send hints — two checkboxes in the plugin's settings page), icons `icons/devtool/{interpreted,baseline,twin,optimized,deoptimized,abandoned,watched,stale}.svg` with `_dark`
- Modify: `settings/` (the two checkboxes), `plugin.xml`, the bundle
- Test: `devtool/editor/DecorationsTest.kt`

**Interfaces:**
- Produces: for the selected run, for each open editor whose file is a body's file or library file: one `RangeHighlighter` per root body at its range's start line with a `GutterIconRenderer` (icon by `MethodState`, a small watched badge when `WatchedMethods.isWatched(name)`, tooltip `"<state> · <n> copies · <deopts> deoptimizations"` plus the last reason, click → the popup of decision 28: Unwatch/Watch, Show Code, Show Inlining, Open in Tool Window); a watched method without a body in the selected run (or with no run selected) shows `watched.svg` (an eye) at its `selection` start from `WatchedMethods.methodNames`; and per send of each body with a `SendOutcome` an inline inlay after the selector's last part's end, text `inlined` / `declined: <reason>` and ` · mono` / ` · poly 3` / ` · mega`, its tooltip the event's fields. Every highlighter and inlay rides on a `RangeMarker`; on a document change, a body whose current text in its marker's range no longer has the record's checksum loses its inlays and its icon becomes `stale.svg` with the tooltip `devtool.stale` ("Changed since the run").
- Consumes: Task 6's model, Task 5's `WatchedMethods`, Task 7's `TracedRuns.selected`.
- [ ] **Step 1: Write the failing tests** with an editor on a fixture program and a `TraceRun` built from records whose ranges point into it: `testAnOptimizedMethodShowsItsIcon`, `testEachBlockIsDecoratedAtItsOwnRange` (two blocks, two inlay sets at their own ranges), `testADeclinedSendShowsItsReason`, `testACacheStateFollowsTheInliningHint`, `the_checksum_matches_a_document_with_an_emoji` (`testTheChecksumMatchesADocumentWithAnEmoji`: Kotlin's FNV-1a over the range's UTF-8 equals the record's), `testAnEditedMethodBecomesStale` (type into one method: its inlays go, its icon is `stale`; the other method keeps its decorations), `testSwitchingRunsRedecorates`, `testSettingsSwitchOffTheHints`, `testALibraryFileIsDecorated` (a `library` body decorated in a file under the library directory).
- [ ] **Step 2: Run**: `gradlew --no-daemon test --tests '*DecorationsTest*'` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): gutter state icons and inlay hints at the sends`.

### Task 10: The Code tab and the code editor tabs

**Files:**
- Create: `devtool/ui/CodeTab.kt`, `devtool/editor/CodeListingFile.kt` (a `LightVirtualFile` of a plain-text listing with a `CodeListingFileType`, read-only), `devtool/editor/LinkedHighlighting.kt`
- Modify: `devtool/ui/RunPanel.kt`, `plugin.xml` (the file type, `fileEditorProvider` not needed: a text editor), the bundle
- Test: `devtool/ui/CodeTabTest.kt`

**Interfaces:**
- Produces: `CodeTab(run)` follows the caret of the selected editor (`CaretListener` through `EditorFactory.eventMulticaster`), finds the innermost body whose range holds the caret, and shows a copy chooser (`ComboBox` of the body's `bytecode` and each `code` record: "bytecode", "baseline", "twin", "optimized key=<name>") over a read-only editor of the listing text in the CLI's format (`--- code …` heads included), built in Kotlin from the record; `fun openInEditor(listing)` opens a `CodeListingFile` named `"<body name> <tier>"` in a fixed editor tab. `LinkedHighlighting`: the caret's statement in the source (the narrowest `statements` range holding it) highlights the bytecode lines whose `index` lies in its `instructions` and the machine code lines whose `at` lies there (scope 0 or a scope whose body is the same body), and the reverse for a caret in the listing; one highlight color per statement, cycling through `EditorColors`-independent palette keys `devtool.linked.1..6` in a `ColorSettingsPage` group.
- [ ] **Step 1: Write the failing tests**: `testTheTabFollowsTheCaret`, `testTheChooserListsEveryCopy`, `testAStatementHighlightsItsBytecodeAndItsMachineCode`, `testACaretInTheListingHighlightsItsStatement`, `testOpenInEditorOpensAFixedReadOnlyTab`.
- [ ] **Step 2: Run**: `gradlew --no-daemon test --tests '*CodeTabTest*'` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): the Code tab and code editor tabs`.

### Task 11: Show Frame Code in the debugger

**Files:**
- Create: `debugger/ShowFrameCodeAction.kt`, `debugger/FrameCodeFollower.kt`
- Modify: `debugger/NewspeakDebugProcess.kt` (a `fun frameCode(frame: NewspeakStackFrame): CompletableFuture<FrameCode>` over `client.request("nsNext/frameCode", JsonObject().apply { addProperty("frameId", frame.id) })`, `data class FrameCode(val body: String, val bytecode: String, val machineCode: String?)`), `plugin.xml` (the action in `XDebugger.Frames.Tree.Popup` and `XDebugger.ToolWindow.TopToolbar`), the bundle
- Test: `debugger/NewspeakDebugProcessTest.kt` (the `debugProcessTest` task, which starts `ns-next`)

**Interfaces:**
- Produces: the action opens a read-only `CodeListingFile` (Task 10) named `"<body> (frame)"` holding `bytecode`, then `machineCode` or the bundle line `devtool.frame.interpreted` ("This frame goes on in the interpreter."); `FrameCodeFollower` listens to the session's `XDebugSessionListener.stackFrameChanged` and `sessionPaused` while that tab is open and replaces its text with the selected frame's code; it stops when the tab closes or the session ends.
- [ ] **Step 1: Write the failing test** `testShowFrameCodeOpensTheFramesCodeAndFollowsSteps` in `NewspeakDebugProcessTest`: stop at a line breakpoint in a hot method; run the action; the open listing holds `--- bytecode {Demo>>…}` and a line whose index is followed by `->`; select the caller frame → the text names the caller's body; step over → the mark moves.
- [ ] **Step 2: Run**: `gradlew --no-daemon debugProcessTest --tests '*testShowFrameCode*'` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat(plugin): Show Frame Code`.

### Task 12: End to end, the documents and the gates

**Files:**
- Create: `editors/jetbrains/src/test/kotlin/.../devtool/TracedRunEndToEndTest.kt`, and in `build.gradle.kts` its own task `traceFileTest` through `runsLikeUnitTests` and `debugsWithTheRuntime()`, excluded from `test`, part of `check`, `mustRunAfter` the other runtime-starting tasks
- Modify: `docs/design/developer-tool.md` (status, decision 9's sentence on the IDE now true of the CLI and MCP only, "Stage 3: the developer tool in the IDE" as built: the runtime's half, the file, the plugin's half, tests; "Planned: the stages" keeps stage 4), `docs/design/debugger.md` ("The JetBrains plugin": Show Frame Code), `docs/design/editor-support.md` ("The project file and running projects": the Developer Tool tab, the free field "Further runtime options", watched methods; the analyzer's `newspeak/methodName` beside `newspeak/breakableLines`), `docs/design/README.md` (the map's row), `CLAUDE.md` (`ns-cli`'s `--trace-file`; the analyzer's request; the plugin's paragraph: the developer tool), `docs/OPEN-WORK.md` ("Next"; the d8 item: stage 3 built; the view item closed), `docs/PROGRESS.md` (the milestone row and entry with the gate results)
- Test: the new end-to-end test

- [ ] **Step 1: Write the end-to-end test** `testATracedRunDecoratesItsMethod`: a run configuration on a scratch `Demo.ns` with `compile` and `printCode` checked and `Demo>>run` watched; run it through `ProgramRunnerUtil`; wait for termination; the selected `TraceRun` has `Demo>>run` with state BASELINE or OPTIMIZED, a `code` record for it, and an open editor on `Demo.ns` shows a gutter icon at `run`'s line; standard output of the console holds the program's output and no `[compile]` line.
- [ ] **Step 2: Run**: `gradlew --no-daemon traceFileTest` — PASS (all earlier tasks built).
- [ ] **Step 3: Write the documents** as listed; every statement in the present tense.
- [ ] **Step 4: Run the gates**: `cargo xtask gates commit`, then `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification`; record each group's summary in `PROGRESS.md`'s entry.
- [ ] **Step 5: Commit**: `docs: the developer tool's stage 3, the IDE`.
