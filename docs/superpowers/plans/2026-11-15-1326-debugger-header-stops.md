# Debugger: Stops at a Class's Header, and the Shipped Sources in the IDE — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A step into an instantiation stops in the factory, at the class header's factory pattern, with the factory's parameters shown; a step into a superclass clause (or a `<:` clause's message) stops at that class's header with its parameters, level by level up to the last class below `Object`; and the shipped modules' sources, taken from the `ns-next` binary itself, are a library in the IDE that can be opened, browsed and given breakpoints that last.

**Architecture:** The VM's stepping (`debugger/stepping.rs`) stops passing over the factory's frame at its push and gains a stop at the entry of an initializer that a superclass clause or a `<:` clause's message entered; a frame standing at such a stop is placed at its declaration's header pattern (`debugger/stack.rs`, the metadata's `factory_pattern`). The agent reports those stops unchanged through DAP. `ns-next` gains a subcommand that writes its embedded shipped sources; the analyzer recognises a shipped text and treats it as the shipped module; the plugin extracts the sources of the binary it runs into a cache directory per fingerprint, registers them as a synthetic library and maps the agent's shipped sources onto those files.

**Tech Stack:** Rust 1.98.0 (edition 2021), `cargo-nextest`; Kotlin, the IntelliJ Platform's `AdditionalLibraryRootsProvider`/`SyntheticLibrary` and XDebugger API; no new external dependency.

**Spec:** the user's decisions of 2026-11-15 (below, "Decisions"); `docs/design/debugger.md`, decision 12 (which this plan replaces), decision 16 (step filters), decision 17 (step out), "Values the IDE holds"; `docs/design/compiler.md`, "The three initializer bodies"; `docs/design/object-model.md`, "Instantiation", "Mixin application".

## Decisions (the user's, 2026-11-15)

1. **The Java constructor pattern.** For `class Punkt x: ax y: ay = Basis new: ax (…)` and a sender's `Punkt x: 1 y: 2`:
   - a **step into** at the sender's statement stops in `Punkt`'s factory frame, placed at the header's factory pattern `x: ax y: ay`, its variables the factory's parameters (`ax = 1`, `ay = 2`);
   - a **step over** (or step into) at that stop goes to the first statement of `Punkt`'s own initializer -- the superclass clause `Basis new: ax` when it is written, otherwise the first `<:` clause's message or slot initializer -- not past the `new`;
   - a **step into** at the superclass clause stops at the entry of `Basis`'s initializer, placed at `Basis`'s header pattern `new: a`, its variables `Basis`'s factory parameters as the clause passed them (`a = 1`); from there the same holds one level up, and so on;
   - a **step over** at the clause runs the whole superclass chain and stops at `Punkt`'s next statement; a **step out** from a header stop goes back to the subclass's initializer, after its clause;
   - every level a `<:` clause adds has a header stop of its own, at its mixin's header, entered by a step into at that clause's message, in the order the levels run;
   - the return from an initializer into the factory still stops at the factory's return point in its caller, after the `new` (decision 12's rule for the way out stands).
2. **`Object` is left out.** `Object`'s initializer (and `Top`'s) never takes a header stop and is never stopped in, whatever the step filters say: the chain ends at the last class that inherits from `Object` directly.
3. **The shipped sources come from the binary.** `ns-next` writes the sources it embeds; the plugin takes them from the very binary it runs, keyed by their fingerprint, and shows them as the library "Newspeak Core Library": read-only, browsable, with line breakpoints that are kept with the project.

## Global Constraints

- No new external dependency (`CLAUDE.md`). `ns-memory` stays the only crate with `unsafe`.
- **Nothing on the hot path.** Every new decision lives where the stepping decisions already live (the held step's safepoint, the one-shots, the cold arms); no instruction, no golden, no `run-corpus` outcome and no image changes; a program run without `--debug` takes no new branch.
- Names are written out (`CLAUDE.md`, Conventions); a Newspeak module inline in a test is an `indoc! {"…"}`, the closing `"}` on its own line; never `{:?}` a `Value` nor `.unwrap()` an `EvaluationResult` (copy a neighbour's `expect_ok`).
- Behaviour tests in `crates/ns-runtime/tests/debugger.rs` run under both lowerings where their neighbours do; agent-level tests in `crates/ns-cli/tests/debug_adapter.rs`.
- Every commit that changes more than Markdown runs `cargo xtask gates commit` (plus `analyzer` for one touching `crates/ns-analyzer`, `codegen` for one touching its files). The controller alone runs `end-of-batch` and `plugin-verification`, after the branch review; every commit before them is work in progress. Subagents run every command in the foreground with `timeout: 600000` on long ones.
- Comments explain the decision, not the mechanics. Every document statement a task makes untrue is rewritten in the same commit, in the present tense.

## Review Focus

1. **The step over at the factory stop** must reach `Punkt`'s own initializer's first statement under both lowerings, including a class with no superclass clause written, a body-less declaration (`class C = S <: M.`), and a class whose initializer has no statement at all (then: after the `new`).
2. **A step into at a clause whose arguments send** (`= Basis new: self helper`) enters `helper` first, as today, and only the clause's `InitializeSuperclass` takes the header stop.
3. **`Object` never stops**, with the shipped filter off too, and an explicit `= Object new` clause steps over to the next statement.
4. **A shipped superclass** (a class inheriting from a shipped class other than `Object`) is filtered as any shipped code: its header stop comes only with the shipped filter off.
5. **A stripped image** (`build --strip-source`): the factory and header stops happen as frames without a line, never a panic and never a step that runs to the end.

---

### Task 1: The factory's frame is stopped in at its push

**Files:**
- Modify: `crates/ns-runtime/src/debugger/stepping.rs` (`passes_over`, `held_step`, `sent_by_step`, `went_on_past_filtered` and the doc comments naming decision 12)
- Modify: `crates/ns-runtime/src/debugger/stack.rs` (the factory frame's location at its stop)
- Test: `crates/ns-runtime/tests/debugger.rs`

- [ ] RED: a step into at `pt:: Punkt x: 1 y: 2.` stops in a frame of kind factory, placed at the span of `x: ax y: ay`, whose variables are `ax = 1`, `ay = 2` (both lowerings).
- [ ] RED: a step over at that stop stops at the first statement of `Punkt`'s initializer: the superclass clause when written; the first slot initializer when none is; after the `new` for a class whose initializer has no statement.
- [ ] RED: a step into at the factory stop behaves as the step over (the factory sends nothing but the initializer).
- [ ] RED: a step out at the factory stop stops after the `new` in the caller.
- [ ] RED: a body-less declaration's factory (`class C = S <: M.`), and a factory a host sends (the root's instantiation run, decision 17's carried steps): the stop happens and the next step reaches the first initializer statement.
- [ ] Implement: the factory's frame is no longer passed over at its push; the push of a factory stops at its frame as at any body's first statement start, even though it has none (the stop at the frame's entry). The way out stays as decision 12 says: an initializer's return or a step out into the factory goes on to the factory's return point in its caller. A step over or into from the factory stop goes on as a step into to the initializer the factory pushes.
- [ ] A stripped image's factory (no metadata): an agent-level test through `build --strip-source` + `load --debug-connect`, as the existing stripped-image tests: the stop happens, without a line, and the next step goes on.
- [ ] Rename or rewrite the tests that pin the pass-over (`a_step_into_passes_over_a_stripped_image_s_factory`, `a_step_over_goes_through_an_initializer_and_past_the_factory`, …) to the new behaviour; keep the ones about the way out.
- [ ] `cargo xtask gates commit`; commit `feat: a step into an instantiation stops in the factory`.

### Task 2: A header stop at the entry of a superclass's or applied level's initializer

**Files:**
- Modify: `crates/ns-runtime/src/debugger/stepping.rs`
- Modify: `crates/ns-runtime/src/debugger/stack.rs` (an initializer frame at its entry stop placed at its declaration's header pattern)
- Modify: `crates/ns-runtime/src/debugger/metadata/` (the header pattern's span for an initializer's metadata, if it does not carry it yet)
- Test: `crates/ns-runtime/tests/debugger.rs`

- [ ] RED: a step into at `Punkt`'s clause `Basis new: ax` stops in `Basis`'s initializer at its entry, before its first statement, placed at `Basis`'s header pattern `new: a`, variables `a = 1` (both lowerings).
- [ ] RED: three levels (`C = B …`, `B = A …`, `A` from `Object`): step into at each clause reaches the next header; at `A`'s header a step into or over goes to `A`'s first statement, never into `Object`, with the shipped filter on and off.
- [ ] RED: a step over at a clause stops at the subclass's next statement; a step out from a header stop stops in the subclass's initializer after its clause.
- [ ] RED: `<:` clauses: each applied level's own part, entered by a step into at its clause's message, stops at its mixin's header, in the order the levels run (`a_step_into_goes_through_a_mixin_application_s_initializers_in_order`'s program, extended).
- [ ] RED: a clause whose argument sends (`= Basis new: self helper`): step into enters `helper` first; the next step into at the clause's `InitializeSuperclass` takes the header stop.
- [ ] RED: an explicit `= Object new` clause: a step into at it goes to the next statement of the subclass, no stop in `Object`; a shipped superclass other than `Object` takes its header stop only with the shipped filter off.
- [ ] RED: a step over or out at an initializer's last statement goes on as today (next level's first statement, after the `new`): no header stop on the way down.
- [ ] Implement: a held step into whose push is an initializer entered by `InitializeSuperclass` or `InitializeAppliedLevel` stops at its entry (its program counter before its first statement start), the stop's place its declaration's factory pattern; `Object`'s and `Top`'s initializers are passed over always (decided by the declaration, not by its being shipped). An initializer entered from its own factory never takes a header stop: the factory's stop is that level's.
- [ ] `cargo xtask gates commit`; commit `feat: a step into a superclass clause stops at the class header`.

### Task 3: The header stops through DAP

**Files:**
- Modify (if needed): `crates/ns-cli/src/debug/stopped.rs`, `stepping.rs`
- Test: `crates/ns-cli/tests/debug_adapter.rs`

- [ ] An agent-level test: `stepIn` at the instantiation answers a `stopped` (reason `step`) whose top frame is the factory, named as the stack trace names a factory, at the header's line and column; `scopes`/`variables` show the parameters; `next` reaches the clause; `stepIn` at the clause reaches the superclass's header; `stepOut` returns after the clause.
- [ ] Anything the agent assumed about a stop having a statement (a frame's `endLine`/`endColumn`, the highlighted range) holds for the header pattern's span.
- [ ] `cargo xtask gates commit`; commit `test: the header stops through the debug adapter` (or `fix:` if the agent needed changes).

### Task 4: `ns-next shipped-sources`

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (the subcommand)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (a public list of the shipped modules' names, with `shipped_module_source`, and the fingerprint of the shipped sources if it is not public yet)
- Test: `crates/ns-cli/tests/` (a test of its own file, as the other subcommands' are)

- [ ] `ns-next shipped-sources --fingerprint` prints the fingerprint of the embedded shipped sources (the one `build.rs` computes, `shipped_code/fingerprint.rs`) as one line.
- [ ] `ns-next shipped-sources <directory>` writes every shipped module as `<Module>.ns`, byte for byte the binary's text (LF), and a file `fingerprint` holding the same line; it creates the directory, refuses a directory that holds other files than these, and overwrites its own.
- [ ] `--json` prints `{ "fingerprint": …, "modules": [ { "name": …, "text": … } ] }` instead of writing, so an agent with a shell can read them.
- [ ] Tests: every module `shipped_module_source` knows is written and equal to it; the fingerprint agrees between the two forms; a foreign file in the directory is refused.
- [ ] `CLAUDE.md`'s `ns-cli` paragraph names the subcommand.
- [ ] `cargo xtask gates commit`; commit `feat: ns-next shipped-sources writes the embedded core library`.

### Task 5: The analyzer recognises a shipped text

**Files:**
- Modify: `crates/ns-analyzer/src/` (the server's document handling and `breakable.rs`)
- Test: `crates/ns-analyzer/tests/`

- [ ] A document whose text is, byte for byte, a shipped module's (`shipped_module_source` of its root's name) is analysed as that shipped module: no diagnostic the program-side resolution would raise for its `MirrorSend` bindings, and `newspeak/breakableLines` answering exactly the statements the VM's metadata for the shipped module has (check against `Interpreter::breakable_statements` on the kernel-resolved module, and that a line breakpoint set there through the agent lands where the answer says).
- [ ] A document that differs from the shipped text by one byte is an ordinary module.
- [ ] `cargo xtask gates commit analyzer`; commit `feat: the analyzer treats a shipped module's text as shipped`.

### Task 6: The plugin's "Newspeak Core Library"

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/library/` (the extraction and the `AdditionalLibraryRootsProvider`)
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/debugger/NewspeakSources.kt`, `NewspeakBreakpointHandlers.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`
- Test: `editors/jetbrains/src/test/kotlin/…` (in the test task whose inputs fit: one that starts `ns-next` is in the debugging tasks' set)

- [ ] The plugin finds the `ns-next` binary it runs (as the run configuration does), asks `shipped-sources --fingerprint`, and, when `<the IDE's system directory>/newspeak/core-library/<fingerprint>/` does not hold it yet, writes it there with `shipped-sources <directory>`; on a background thread, never on the EDT; a failure is logged and leaves the library out.
- [ ] A `SyntheticLibrary` "Newspeak Core Library" over that directory, its files read-only, shown under External Libraries.
- [ ] A stop in a shipped module (the agent's `source` with `sourceReference` and name `<Module>.ns`) opens the library's file of that name when its text equals the agent's (read through the reference once per session), and the reference's `LightVirtualFile` otherwise.
- [ ] A line breakpoint in a library file is sent as the shipped module's source (by name, as today's handler sends a referenced module), verified, and kept with the project across sessions.
- [ ] Tests: the extraction (fingerprint directory written once, reused); the mapping of a shipped frame onto the library file; a breakpoint in the library's `Kernel.ns` (e.g. in a collection method a program calls) stops the program in a debug session (`debugProcessTest`'s or `breakpointTypesTest`'s harness).
- [ ] `docs/design/editor-support.md` and `debugger.md`'s plugin part describe it; `CLAUDE.md`'s plugin paragraph names it in a few words.
- [ ] `cargo xtask gates commit`; commit `feat: the shipped modules as a library in the IDE`.

### Task 7: The documents

- [ ] `docs/design/debugger.md`: decision 12 rewritten to the decisions above (the factory stop, the header stops, `Object` left out, the way out unchanged), its tests named; every other statement that says a step passes over the factory (decision 16/17's text, "Step over in recursion and blocks", the agent's test list, l.42's summary) rewritten; the reasons kept that still hold (no hiding the frame, §3.6).
- [ ] `CLAUDE.md`'s debugger paragraph if a statement there became untrue.
- [ ] `docs/OPEN-WORK.md`: items any review left open; `docs/PROGRESS.md`: an entry for this batch (2026-11-15).
- [ ] Commit `docs: the header stops and the core library in the IDE`.
- [ ] The controller: branch review (Fable), its fix wave, then `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification`.
