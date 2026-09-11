# Retire the Tree-Walker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the tree-walking engine so that the bytecode VM is the only
engine, after first harvesting the tree-walker's verdicts into tests that
outlive it.

**Architecture:** Two harvesting tasks run while the tree-walker still exists:
`Interpreter::load_module` starts compiling and installing the module's code,
which moves every runtime test that silently ran under the tree-walker onto the
VM, and the inline cases of the engine differential become golden tests of exit
status, stdout and stderr. Then the removal goes outward-in: the image format
loses the frame record's pc-kind byte, the CLI stops offering the tree-walker,
the runtime deletes it, and the run-time `bindings` parameter, which only the
tree-walker read, disappears from every signature. Documents and a
before/after measurement close the step.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Three
crates: `ns-syntax` (untouched), `ns-runtime`, `ns-cli`.

**Spec:** [docs/design/execution.md](../../design/execution.md), "Planned: one
engine", step 1 ("Removal"). Context for what comes after it:
[docs/design/compiler.md](../../design/compiler.md), "Planned: the reference
lowering", and [docs/PROGRESS.md](../../PROGRESS.md), "Where the next session
starts" (steps A to C). Read all three before Task 1.

## Scope

This is step **A** of the three steps PROGRESS.md schedules before M4's 1b.
Deliberately **not** in this plan:

- **Cleanup (step B).** Renaming `interpreter/eval.rs` once it no longer
  evaluates anything, passing `Copy` handles by value, pre-interning selectors,
  splitting `ns-cli/src/main.rs`, attaching compiled code to `MixinMethod`
  instead of looking it up in a `HashMap`, rewriting history-narrating
  comments. Where this plan leaves code that B will reshape, it leaves it
  working and says so; it does not start B's work.
- **The reference lowering (step C).** Until C lands, the golden tests from
  Task 2, every fixture's `.expected`, `run-corpus` and the rest of the suite
  are the safety net. There is no differential between two ways of running a
  program during and after this plan.
- **The trampoline (1b)** and any recursion-depth limit.

**What the harvest already established** (measured while planning, by making
`load_module` install compiled code and running `cargo test -p ns-runtime`):
exactly four tests fail, and **none is a VM divergence**. Two image tests
deliberately build tree-walker closures, one `vm.rs` test asserts that the
tree-walker is the default, and `instantiate.rs`'s
`superclass_slot_initializer_runs_before_the_subclass_own_slots` loads two
modules into one interpreter, which fails unless the second load's code is
*added to* the first's rather than replacing it. Task 1 is written against
that result. If Task 1 finds a failure beyond these four, treat it as a real VM
defect: fix the VM with the tree-walker as the reference, which is still
possible until Task 5.

## Global Constraints

- Rust 1.98.0, edition 2021. **No new external dependency.**
- All code, identifiers, comments, doc comments and commit messages in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`,
  `refactor:`). **Every commit — including one made by a subagent — ends with
  exactly these two trailer lines:**
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv
  ```
- Never override git author or committer identity.
- `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets`
  must be warning-free; `cargo test --workspace` must pass at every commit.
- Both corpus gates hold at every commit:
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`, and
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- Until Task 4 removes it, `crates/ns-cli/tests/differential.rs` still runs and
  must pass. Until Task 5 removes the tree-walker, it is the reference when the
  engines disagree.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult`.
  Tests unwrap through a local `expect_ok` helper — copy the one in a
  neighbouring test file.
- Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`.
- Never copy dispatch logic, lookup logic or error text out of `interpreter/`
  into `vm.rs`.
- A wire tag is assigned once and never renumbered or reused
  (`crates/ns-runtime/src/image/opcodes.rs`, pinned by
  `crates/ns-runtime/tests/opcode_tags.rs`).
- Nothing under `crates/ns-syntax` changes.
- Never judge performance from a debug build.
- Design documents in `docs/design/` are living and describe the current state;
  this plan is an execution record — leave it as written once complete.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/ns-runtime/src/interpreter/mod.rs` | `Interpreter` holds one `CompiledModule` (`code`), which `load_module` and `image::load` add to through `install_code`. No `Engine`, no `set_engine`, no `EngineUnavailable`, no `decoded_from_image`. `invoke_method` runs `vm::run_code` directly. |
| `crates/ns-runtime/src/interpreter/eval.rs` | No expression evaluator. Keeps the send helpers (`dispatch_lexical_send`, `dispatch_super_send`), local and outer-local access, `home_of`, `store_instance_field`, `invoke_closure`, the deferred-error constructors and `parse_int_literal`. Its name is B's to change. |
| `crates/ns-runtime/src/interpreter/instantiate.rs`, `nested.rs` | Initializer, superclass-clause and superclass-message bodies always run compiled. |
| `crates/ns-runtime/src/types.rs` | `ClosureData.code: Rc<CompiledCode>` replaces `ClosureBody`; `PcKind` and `Frame.pc_kind` are gone; `ClosureData.bindings` is gone. |
| `crates/ns-runtime/src/heap.rs` | `ActivationHeader` and `StackEntry` carry no `pc_kind`. |
| `crates/ns-runtime/src/bytecode.rs` | `CompiledModule` derives `Default`, gains `extend`, and loses `bindings`. |
| `crates/ns-runtime/src/image/` | The frame record has no pc-kind byte; `PC_KIND_*` are retired; `LoadedImage` has no `empty_bindings`; `load` calls `install_code`. |
| `crates/ns-runtime/src/vm.rs`, `builtins/` | No `bindings` threading. |
| `crates/ns-cli/src/main.rs` | No `--engine`; `bench` times one engine; `run-corpus` runs each row from source and as a built binary. |
| `crates/ns-cli/tests/program_outcomes.rs` (new) | The former differential's inline cases as golden tests of exit status, stdout and stderr. |
| `crates/ns-cli/tests/differential.rs` | Deleted in Task 4. |
| `CLAUDE.md`, `docs/design/*.md`, `docs/PROGRESS.md` | Describe one engine; record the harvest and the measurement. |

---

### Task 1: `load_module` compiles and installs the module's code

The tree-walker's first harvest. Once `load_module` installs compiled code,
every runtime integration test that never called `set_engine` — about a hundred
across `builtins_*`, `factory_send`, `instantiate`, `mixin`, `nested_classes`
and `platform` — runs under the VM instead of the tree-walker. Some of those
tests exist to exercise the *natives* behind `ifTrue:`, `and:` and
`whileTrue:`; under the VM a literal block argument is inlined and never
reaches the native, so those tests gain a second source that passes the blocks
through variables, which the compiler cannot inline.

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (the `CompiledModule` struct, ~line 253)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`set_engine` doc, new `install_code`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`load_module`, ~line 54-84)
- Modify: `crates/ns-cli/src/main.rs` (`bootstrap_with_engine`, ~line 259-282)
- Test: `crates/ns-runtime/tests/vm.rs`, `crates/ns-runtime/tests/builtins_control_flow.rs`, `crates/ns-runtime/tests/builtins_boolean_numbers.rs`, `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `CompiledModule::extend(&mut self, other: CompiledModule)`;
  `Interpreter::install_code(&mut self, module: CompiledModule)` (`pub(crate)`);
  `load_module` now compiles and installs. Tasks 4 and 5 rely on all three.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/vm.rs`, replace the whole test
`the_tree_walk_engine_is_still_the_default` (it starts at `#[test]` above
`fn the_tree_walk_engine_is_still_the_default` and ends at its closing brace,
just before the `/// The probe that exposed the frame-shift defect` comment)
with:

```rust
/// Loading and compiling are one step: a class `load_module` returns is
/// already runnable as bytecode, so nothing can instantiate or send to a
/// loaded class whose code was never compiled.
#[test]
fn loading_a_module_installs_its_compiled_code() {
    let src =
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ 1 ] )\n) : ()";
    let (mut interp, class, bindings) = load(src);
    assert!(interp.compiled_module().is_some());
    let instance = new_instance(&mut interp, &class, &bindings);
    let block = expect_ok(
        send(&mut interp, instance, "makeBlock", vec![], &bindings),
        "makeBlock",
    );
    let Value::Obj(obj) = &block else {
        panic!("expected a closure object")
    };
    let closure = interp
        .heap_ref()
        .closure_data(obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Closure"));
    assert!(matches!(closure.body, ClosureBody::Compiled(_)));
}

/// A second `load_module` into the same interpreter adds its code to the
/// first module's rather than replacing it: every key is a mixin's or a
/// method's `Rc` identity, so two modules' bodies never collide, and a class
/// from the first load must keep running.
#[test]
fn loading_a_second_module_keeps_the_first_modules_code() {
    let first = "Newspeak3\n'T'\nclass A = Object new (\n) (\n  public answer = ( ^1 )\n) : ()";
    let second = "Newspeak3\n'T'\nclass B = Object new (\n) (\n  public answer = ( ^2 )\n) : ()";
    let (mut interp, a, bindings) = load(first);
    let module = ns_syntax::parse(second).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(second),
        SourceOrigin::File(PathBuf::from("b.ns")),
    )
    .expect("resolve");
    let b = interp.load_module(&resolved);
    let a_instance = new_instance(&mut interp, &a, &bindings);
    let b_instance = new_instance(&mut interp, &b, &resolved.bindings);
    let from_a = expect_ok(
        send(&mut interp, a_instance, "answer", vec![], &bindings),
        "A answer",
    );
    let from_b = expect_ok(
        send(&mut interp, b_instance, "answer", vec![], &resolved.bindings),
        "B answer",
    );
    assert_eq!(int_of(&from_a), 1);
    assert_eq!(int_of(&from_b), 2);
}
```

- [ ] **Step 2: Run them to verify the first fails**

Run: `cargo test -p ns-runtime --test vm loading_a -- --nocapture`
Expected: `loading_a_module_installs_its_compiled_code` FAILS at
`assert!(interp.compiled_module().is_some())`.
`loading_a_second_module_keeps_the_first_modules_code` passes today (it runs
under the tree-walker); it is the guard that Step 3 adds code rather than
replacing it.

- [ ] **Step 3: Implement**

(a) `crates/ns-runtime/src/bytecode.rs`: directly after the closing brace of
`pub struct CompiledModule { ... }`, add:

```rust
impl CompiledModule {
    /// Adds `other`'s bodies to this module's. Every key is a mixin's or a
    /// method's `Rc` identity, so the bodies of two separately loaded modules
    /// never collide, and nothing already installed is replaced.
    pub fn extend(&mut self, other: CompiledModule) {
        self.methods.extend(other.methods);
        self.initializers.extend(other.initializers);
        self.superclass_clauses.extend(other.superclass_clauses);
        self.superclass_messages.extend(other.superclass_messages);
    }
}
```

(b) `crates/ns-runtime/src/interpreter/mod.rs`: directly after
`mark_decoded_from_image`, add:

```rust
    /// Makes `module`'s compiled bodies runnable, adding them to whatever an
    /// earlier load installed. `load_module` is the caller: loading and
    /// compiling are one step.
    pub(crate) fn install_code(&mut self, module: CompiledModule) {
        match &mut self.engine {
            Engine::Bytecode(installed) => installed.extend(module),
            Engine::TreeWalk => self.engine = Engine::Bytecode(module),
        }
    }
```

and replace the first paragraph of `set_engine`'s doc comment (`/// Switches
engines. Call after ... keeps its /// behavior.`) with:

```rust
    /// Switches engines. `load_module` already installs the bytecode engine;
    /// the CLI switches to the tree-walker afterwards to run it as the
    /// differential oracle.
```

(c) `crates/ns-runtime/src/interpreter/instantiate.rs`, `load_module`: replace
its last statement

```rust
        self.new_class_object(application, None, None, superclass)
```

with

```rust
        let class = self.new_class_object(application, None, None, superclass);
        let code = crate::compiler::compile_module(&self.heap, &class, &resolved.bindings);
        self.install_code(code);
        class
```

and add this paragraph at the end of `load_module`'s doc comment:

```rust
    ///
    /// It also compiles the whole mixin tree and installs the code, adding it
    /// to any an earlier load installed: a class this returns is already
    /// runnable, so nothing can send to a loaded class whose code was never
    /// compiled.
```

(d) `crates/ns-cli/src/main.rs`, `bootstrap_with_engine`: replace its body
with

```rust
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    if engine == EngineArg::TreeWalk {
        interp
            .set_engine(ns_runtime::Engine::TreeWalk)
            // Only `Engine::TreeWalk` on a loaded image is ever refused, and
            // this interpreter was built from source a line ago.
            .expect("a source-built interpreter accepts the tree-walker");
    }
    (interp, class)
```

and its doc comment's first sentence with "Bootstraps an interpreter and loads
the module's root class, which compiles it and installs the bytecode engine;
`tree-walk` switches to the tree-walker afterwards -- *before* anything is
instantiated, so the root class's instance initializer runs under it too."

(e) `crates/ns-runtime/tests/image.rs`: the two tests that deliberately build a
tree-walker closure now have to ask for the tree-walker. In
`a_tree_walker_closure_is_a_snapshot_error_not_a_silent_omission` (~line 1369)
and `a_snapshot_of_a_tree_walker_closure_is_refused_rather_than_written`
(~line 2326), directly after the line that calls `load(...)`, add

```rust
    interp
        .set_engine(Engine::TreeWalk)
        .expect("a source-built interpreter accepts the tree-walker");
```

(the second test later installs `Engine::Bytecode` itself; leave that as it
is). Both tests are deleted in Task 5 with the tree-walker.

- [ ] **Step 4: Keep the natives covered**

`crates/ns-runtime/tests/builtins_control_flow.rs` exists to run the
`whileTrue:`/`whileFalse:`/`ifFalse:ifTrue:` natives. Replace its module doc
comment with

```rust
//! The three control-flow natives Milestone 2 adds (`Block>>whileTrue:`,
//! `Block>>whileFalse:`, `Boolean>>ifFalse:ifTrue:`). Every block reaches
//! the send through a variable: the compiler inlines only literal blocks, so
//! a variable is what makes these the natives running rather than the
//! inlined form, which the compiler's own tests and `program_outcomes.rs`
//! cover.
```

and replace the five tests' sources as follows (keep each test's name and
assertions):

- `while_true_loops_until_the_condition_is_false_and_answers_nil`: first source
  `module("| n c b | n := 0. c := [n < 3]. b := [n := n + 1]. c whileTrue: b. ^n")`,
  second source `module("| c | c := [false]. ^c whileTrue: [nil]")`.
- `while_false_loops_until_the_condition_is_true`:
  `module("| n c | n := 0. c := [n = 3]. c whileFalse: [n := n + 1]. ^n")`.
- `a_non_boolean_condition_sends_must_be_boolean`:
  `module("| c | c := [3]. ^c whileTrue: [nil]")`.
- `a_non_local_return_escapes_a_while_loop`:
  `module("| n c | n := 0. c := [true]. c whileTrue: [n := n + 1. n = 5 ifTrue: [^n]]")`.
- `if_false_if_true_picks_the_right_block`: first source
  `module("| f t | f := ['f']. t := ['t']. ^true ifFalse: f ifTrue: t")`,
  second source
  `module("| f t | f := ['f']. t := ['t']. ^false ifFalse: f ifTrue: t")`.

In `crates/ns-runtime/tests/builtins_boolean_numbers.rs`, four tests reach a
Boolean native through a literal block. Make each run both forms — the literal
one now covers the inlined path, the variable one the native. For
`boolean_if_true_if_false_selects_the_right_block_through_a_real_fixture`,
replace its body with:

```rust
    for body in [
        "^true ifTrue: [1] ifFalse: [2]",
        "| t f | t := [1]. f := [2]. ^true ifTrue: t ifFalse: f",
    ] {
        let src = format!(
            "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( {body} )\n) : ()"
        );
        let (mut interp, class, resolved) = load(&src);
        let receiver = expect_ok(
            interp.instantiate(class, vec![], &resolved.bindings),
            "instantiate",
        );
        let result = interp.send(receiver, Symbol::intern("m"), vec![], &resolved.bindings);
        match expect_ok(result, body) {
            Value::SmallInt(1) => {}
            other => panic!(
                "{body}: expected Value::SmallInt(1), got {}",
                interp.heap_ref().describe(&other)
            ),
        }
    }
```

Apply the same loop shape to the other three, keeping each one's own class
header and assertion:

| Test | Literal body (today's) | Variable body (new) |
|---|---|---|
| `if_true_block_body_resolves_an_enclosing_slot_reference` (class header `class Foo = Object new (\|x = 5.\|) (`) | `^true ifTrue: [ x ] ifFalse: [ 0 ]` | `\| t f \| t := [ x ]. f := [ 0 ]. ^true ifTrue: t ifFalse: f` |
| `and_block_arguments_non_local_return_actually_escapes_not_becomes_false` | `true and: [^'escaped']. ^'fell through'` | `\| b \| b := [^'escaped']. true and: b. ^'fell through'` |
| `and_or_short_circuit_never_invokes_the_untaken_block` | `false and: [^'wrongly invoked']. true or: [^'wrongly invoked']. ^'correct'` | `\| b \| b := [^'wrongly invoked']. false and: b. true or: b. ^'correct'` |

(The `\|` in the table is a Markdown escape; the Newspeak source has a plain
`|`.) The doc comment on `if_true_block_body_resolves_an_enclosing_slot_reference`
talks about `eval_ident` and the tree-walker's bindings map; add one sentence at
its end: "Both forms now run compiled; the variable form still reaches the
native `ifTrue:ifFalse:` with a real closure."

(`cmd_disasm` still calls `bootstrap_with_engine(&loaded, EngineArg::TreeWalk)`
and then compiles the module again itself. It compiles twice and runs nothing,
which is harmless; leave it — Task 4 rewrites it.)

- [ ] **Step 5: Run the whole suite**

Run: `cargo test --workspace`
Expected: PASS, including `loading_a_module_installs_its_compiled_code`,
`superclass_slot_initializer_runs_before_the_subclass_own_slots` and
`differential.rs`. If any other test fails, it is a VM divergence the
differential never covered: reproduce it under `ns-next run --engine
tree-walk` and `--engine bytecode`, treat the tree-walker's answer as correct,
fix the compiler or the VM test-first, and record the defect for Task 7's
PROGRESS entry.

- [ ] **Step 6: Gates and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates. Expected: clean, `parsed 210, failed 0, expected-fail 1, stale 0`,
`ran 6, failed 0, skipped 205`.

```bash
git add -A crates
git commit -m "feat: compile and install a module's code when it is loaded

Loading and compiling become one step, so a class load_module returns is
already runnable as bytecode. This moves every runtime test that never
installed an engine off the tree-walker and onto the VM; the tests whose
point is a control-flow native now pass their blocks through variables,
which the compiler does not inline. A second load adds its code to the
first's instead of replacing it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 2: Freeze the differential's verdicts as golden tests

The second harvest. `crates/ns-cli/tests/differential.rs` has thirteen inline
cases, each a program both engines agree on. Their agreed outcome — exit
status, stdout and stderr — becomes a golden test that survives the
tree-walker. The literals below were captured from both engines while planning;
they agreed on every one.

**Files:**
- Create: `crates/ns-cli/tests/program_outcomes.rs`
- Modify: `crates/ns-cli/tests/run_fixtures.rs`

**Interfaces:**
- Produces: `program_outcomes.rs`; Task 4 deletes `differential.rs` relying on
  it.

- [ ] **Step 1: Write the golden tests**

Create `crates/ns-cli/tests/program_outcomes.rs`:

```rust
//! The observable outcome -- exit status, stdout and stderr, byte for byte --
//! of programs that cannot be fixtures, because they exit non-zero or pin an
//! exact message. Every expectation here is a verdict the tree-walker and the
//! bytecode VM agreed on before the tree-walker was retired
//! (`docs/design/execution.md`), recorded so that a later change cannot move
//! it quietly. Until the reference lowering lands there is no second way to
//! run a program and compare: a change that alters one of these is a
//! behaviour change and has to be justified as one.

mod common;

use common::ScratchDir;

/// Runs `src` with `ns-next run` and asserts all three streams exactly.
fn assert_outcome(name: &str, src: &str, exit: i32, stdout: &str, stderr: &str) {
    let dir = ScratchDir::new(name);
    let path = dir.join(&format!("{name}.ns"));
    std::fs::write(&path, src).expect("write source");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run ns-next");
    let actual_stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let actual_stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{name}: exit status (stderr: {actual_stderr})"
    );
    assert_eq!(actual_stdout, stdout, "{name}: stdout");
    assert_eq!(actual_stderr, stderr, "{name}: stderr");
}

#[test]
fn a_does_not_understand_error() {
    assert_outcome(
        "dnu",
        "Newspeak3\n'T'\nclass Dnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 fooBar\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #fooBar\n",
    );
}

/// A block created while its class initializes, whose `^` runs after the
/// initializer has returned: its home is dead, so this is §7.2's
/// `cannotReturn`.
#[test]
fn a_return_from_a_block_whose_initializer_has_finished_is_cannot_return() {
    assert_outcome(
        "init_block_return",
        "Newspeak3\n'Test'\nclass InitBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk = [ ^2 ]. |\n) (\n\
         \tpublic main: p args: a = (\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: cannotReturn: the enclosing method activation has already returned\n",
    );
}

/// The method-home counterpart: `stash` stores a block holding `^` and
/// returns normally; a later send invokes it after its home's frame was
/// divorced.
#[test]
fn a_return_from_a_stored_block_whose_method_has_returned_is_cannot_return() {
    assert_outcome(
        "stored_block_return",
        "Newspeak3\n'Test'\nclass StoredBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk ::= nil. |\n) (\n\
         \tstash = (\n\
         \t\tblk := [^'too late'].\n\
         \t\t^'stashed'\n\
         \t)\n\
         \tpublic main: p args: a = (\n\
         \t\tp transcript show: self stash; newLine.\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
        1,
        "stashed\n",
        "Uncaught Newspeak error: cannotReturn: the enclosing method activation has already returned\n",
    );
}

/// An inlined selector's guard fails on a non-Boolean receiver, and the
/// fallback is a real send: `3` does not understand `ifTrue:ifFalse:`.
#[test]
fn a_non_boolean_receiver_of_an_inlined_selector_does_not_understand() {
    assert_outcome(
        "if-dnu",
        "Newspeak3\n'T'\nclass IfDnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 ifTrue: ['a'] ifFalse: ['b']\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #ifTrue:ifFalse:\n",
    );
}

/// The condition body runs exactly once, then its non-Boolean value is sent
/// `mustBeBoolean`.
#[test]
fn a_non_boolean_while_condition_fails_with_must_be_boolean() {
    assert_outcome(
        "while-must-be-boolean",
        "Newspeak3\n'T'\nclass WhileMbb usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t[p transcript show: 'cond'; newLine. 3] whileTrue: [nil]\n\t)\n) : ()\n",
        1,
        "before\ncond\n",
        "Uncaught Newspeak error: an object does not understand #mustBeBoolean\n",
    );
}

#[test]
fn a_variable_block_receiver_takes_the_native_loop() {
    assert_outcome(
        "while-variable",
        "Newspeak3\n'T'\nclass WhileVar usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond |\n\t\tcond := [n < 3].\n\t\tcond whileTrue: [n := n + 1].\n\t\tp transcript show: n printString; newLine\n\t)\n) : ()\n",
        0,
        "3\n",
        "",
    );
}

/// Only the non-short-circuited arms run, and a non-Boolean block value
/// coerces to `false`.
#[test]
fn and_or_short_circuit_and_coerce() {
    assert_outcome(
        "and-or",
        "Newspeak3\n'T'\nclass AndOr usingPlatform: platform = Object new (\n\t| log ::= ''. |\n) (\n\tnote: s = ( log := log, s. ^true )\n\tpublic main: p args: a = (\n\t\tp transcript show: (true and: [self note: 'A']) printString; newLine.\n\t\tp transcript show: (false and: [self note: 'B']) printString; newLine.\n\t\tp transcript show: (true or: [self note: 'C']) printString; newLine.\n\t\tp transcript show: (false or: [self note: 'D']) printString; newLine.\n\t\tp transcript show: (true and: [3]) printString; newLine.\n\t\tp transcript show: log; newLine\n\t)\n) : ()\n",
        0,
        "true\nfalse\ntrue\ntrue\nfalse\nAD\n",
        "",
    );
}

/// A `^` inside an inlined loop body unwinds out of the whole loop, and a
/// closure built inside an inlined branch still reaches the enclosing frame.
#[test]
fn a_return_out_of_an_inlined_loop_body() {
    assert_outcome(
        "inlined-return",
        "Newspeak3\n'T'\nclass InlinedReturn usingPlatform: platform = Object new (\n) (\n\tcount: limit = (\n\t\t| i |\n\t\ti := 0.\n\t\t[true] whileTrue: [\n\t\t\ti := i + 1.\n\t\t\ti > limit ifTrue: [^i]].\n\t\t^0\n\t)\n\tcapture: x = (\n\t\t| c |\n\t\tx > 0 ifTrue: [c := [x * 2]] ifFalse: [c := [x]].\n\t\t^c value\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: (self count: 3) printString; newLine.\n\t\tp transcript show: (self capture: 5) printString; newLine\n\t)\n) : ()\n",
        0,
        "4\n10\n",
        "",
    );
}

/// A send's operands mutating each other's inputs: every operand keeps the
/// value it had when it was evaluated. One case per consumer -- keyword
/// arguments, a binary send's right operand mutating the left,
/// dynamic-array elements, and an inlined arm.
#[test]
fn operands_that_mutate_each_other_keep_their_evaluated_values() {
    assert_outcome(
        "operand-order",
        "Newspeak3\n'T'\nclass OperandOrder usingPlatform: platform = Object new (\n) (\n\tfirst: x second: y = ( ^x )\n\tkeywordArgs = ( | t | t := 1. ^self first: t second: (t := 9) )\n\tbinaryOperand = ( | t | t := 2. ^t + (t := 30) )\n\tarrayElements = ( | t | t := 1. ^{t. t := 5. t} at: 1 )\n\tinlinedArm = ( | t | t := 2. ^t + (true ifTrue: [t := 9. 1] ifFalse: [2]) )\n\tpublic main: p args: a = (\n\t\tp transcript show: self keywordArgs printString; newLine.\n\t\tp transcript show: self binaryOperand printString; newLine.\n\t\tp transcript show: self arrayElements printString; newLine.\n\t\tp transcript show: self inlinedArm printString; newLine\n\t)\n) : ()\n",
        0,
        "1\n32\n1\n3\n",
        "",
    );
}

/// The same invariant when the write happens inside a callee, through a
/// closure built in an earlier statement: a cascade receiver and an
/// assignment's value handed on through a setter send.
#[test]
fn an_operand_written_by_a_callee_keeps_its_evaluated_value() {
    assert_outcome(
        "operand-callee",
        "Newspeak3\n'T'\nclass CalleeClobber usingPlatform: platform = Object new (\n\t| c ::= nil. |\n) (\n\tfirst: x second: y = ( ^x )\n\thook: v = ( c value. ^v )\n\tsetterValue = ( | t | t := 1. c := [t := 9]. ^self first: (hook := t) second: 1 )\n\targThroughASend = ( | t | t := 1. c := [t := 9]. ^self first: t second: (self hook: 0) )\n\tcascadeReceiver = (\n\t\t| t orig |\n\t\torig := OrderedCollection new.\n\t\tt := orig.\n\t\tc := [t := OrderedCollection new].\n\t\tt add: 1; add: (self hook: 2).\n\t\t^orig size\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: self setterValue printString; newLine.\n\t\tp transcript show: self argThroughASend printString; newLine.\n\t\tp transcript show: self cascadeReceiver printString; newLine\n\t)\n) : ()\n",
        0,
        "1\n1\n2\n",
        "",
    );
}

/// A bare `super` parses and resolves but has no binding, so it is a
/// program-level error raised where it is evaluated.
#[test]
fn a_bare_super_is_a_program_error() {
    assert_outcome(
        "bare-super",
        "Newspeak3\n'T'\nclass BareSuper usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t^super\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: internal: unresolved identifier `super`\n",
    );
}

/// Raised where it is *evaluated*, not when the method is compiled:
/// everything before it still runs.
#[test]
fn a_bare_super_raises_only_when_it_is_reached() {
    assert_outcome(
        "bare-super-deferred",
        "Newspeak3\n'T'\nclass DeferredSuper usingPlatform: platform = Object new (\n) (\n\tunreached = ( ^super )\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t^self unreached\n\t)\n) : ()\n",
        1,
        "before\n",
        "Uncaught Newspeak error: internal: unresolved identifier `super`\n",
    );
}

/// §6.1: "It is a run time error if the message determined by the
/// inheritance clause is not the primary factory message of C."
#[test]
fn a_superclass_clause_naming_the_wrong_factory_fails() {
    assert_outcome(
        "wrong-superclass-factory",
        "Newspeak3\n'T'\nclass WrongFactory usingPlatform: platform = Object new (\n) (\n\tpublic class Base withBase: b = Object new (| v ::= b. |) (\n\t)\n\tpublic class Derived = Base new (\n\t) (\n\t)\n\tpublic main: p args: a = (\n\t\t^Derived new\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: the superclass clause of class Derived sends #new, but the primary factory of class Base is #withBase:\n",
    );
}
```

- [ ] **Step 2: Run them**

Run: `cargo test -p ns-cli --test program_outcomes`
Expected: 13 passed. These pin today's behaviour, so they pass immediately;
their job starts when the tree-walker is gone. If one fails, the literal here
was copied wrong: run the source under both `--engine bytecode` and `--engine
tree-walk`, confirm the two agree, and correct the literal to what they agree
on. Never correct it to something only one engine produces.

- [ ] **Step 3: Pin stderr for every fixture too**

In `crates/ns-cli/tests/run_fixtures.rs`, directly after the existing
`assert!(output.status.success(), ...)`, add:

```rust
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "",
            "{} wrote to stderr",
            path.display()
        );
```

and extend the file's single test's doc by adding above `#[test]`:

```rust
/// Every fixture succeeds, prints exactly its `.expected` stdout and writes
/// nothing to stderr -- the three streams the engine differential used to
/// compare, pinned against a recorded answer instead.
```

Run: `cargo test -p ns-cli --test run_fixtures`
Expected: PASS.

- [ ] **Step 4: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`,
`cargo test --workspace`.

```bash
git add crates/ns-cli/tests/program_outcomes.rs crates/ns-cli/tests/run_fixtures.rs
git commit -m "test: record the engine differential's verdicts as golden outcomes

Every inline case of differential.rs becomes a golden test of exit status,
stdout and stderr, with the literals both engines agree on today, so the
verdicts outlive the tree-walker. Fixtures now also pin an empty stderr.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 3: Drop the pc-kind byte from the image's frame record

A frame record says which unit its `pc` counts in, and only bytecode was ever
representable. The byte goes; its two tag values retire under the
assigned-once rule. `PcKind` itself stays in the runtime until Task 5, so the
encoder still refuses a tree-walker frame, but it no longer writes a byte for a
bytecode one.

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (`encode_frame`, ~line 978-1004)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`DecodeError::TreeWalkFrame` and its `Display` arm, `decode_frame` ~line 1608-1674, `MIN_FRAME_RECORD_BYTES` ~line 1699-1703)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (frame loop, ~line 279-324)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (the `PC_KIND_*` block, ~line 107-114)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Produces: a frame record is `receiver, args, temps, pc, sender, home,
  current_class, kind, uncontinuable` — no byte after `pc`.
  `DecodeError::TreeWalkFrame` no longer exists.

- [ ] **Step 1: Change the tests first**

In `crates/ns-runtime/tests/image.rs`:

(a) Replace `one_frame_prefix` and `one_frame_prefix_with_pc_kind` (~line
1876-1895) with one helper:

```rust
/// A one-frame graph section up to (but excluding) the frame-kind tag.
fn one_frame_prefix() -> encode::Writer {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w
}
```

(b) Delete the three lines `w.write_u8(1); // pc kind: an instruction index`
(~lines 1602, 2103, 2963 and 3025 — four occurrences; delete every one).

(c) Delete the tests `a_frame_says_which_engine_its_pc_belongs_to`,
`a_frame_claiming_a_tree_walk_pc_is_refused` and
`an_unknown_frame_pc_kind_is_an_error_not_a_panic` (~line 4928-4977), and
their doc comments.

(d) Add, next to `an_invalid_frame_kind_tag_is_a_decode_error_not_a_panic`:

```rust
/// The byte after `pc` is the frame's `sender` flag: a frame record no longer
/// says what its `pc` counts, because an image's code table holds only
/// bytecode. A record written in the old layout -- the retired pc-kind byte
/// after `pc` -- therefore has that byte read as "a sender follows", and the
/// next four bytes (the old sender, home and current-class flags and the kind
/// tag) as a sender id far outside the one-frame table, so it is refused
/// rather than read as a different frame.
#[test]
fn a_frame_record_in_the_old_layout_with_the_pc_kind_byte_is_refused() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_u8(1); // the retired pc-kind byte (bytecode)
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(2); // frame kind: TopLevel
    w.write_bool(false); // continuable
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    assert!(GraphFixture::new().decode(&bytes).is_err());
}
```

In `crates/ns-runtime/tests/opcode_tags.rs`, leave the two `PC_KIND_*` pins
(~line 144-145, 180) exactly as they are: the numbers are retired, not freed,
and the pins are what keeps them from being reassigned.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — the hand-written frame records no longer carry the byte the
decoder still reads.

- [ ] **Step 3: Implement**

(a) `encode.rs`, `encode_frame`: replace the comment and `match f.pc_kind { ... }`
block (from `// \`pc\` alone is ambiguous` to the closing brace of the match)
with

```rust
    // An image's code table holds only compiled bodies, so a frame's `pc` is
    // always an instruction index and the record does not say so. A
    // tree-walker activation counts statements instead, which no image
    // carries.
    if f.pc_kind == PcKind::TreeWalk {
        return Err(
            "image encoding cannot serialize a tree-walker activation: its `pc` counts \
             statements, which an image's code table does not carry"
                .to_string(),
        );
    }
```

and remove `PC_KIND_BYTECODE` from the file's `use super::opcodes::...` list if
it is imported by name.

(b) `decode.rs`: delete the `TreeWalkFrame` variant of `DecodeError` and its
arm in `impl Display for DecodeError`. In `decode_frame`, delete the comment and
the `let pc_kind = match r.read_u8()? { ... };` statement, and in the `Frame { ... }`
literal it returns write `pc_kind: PcKind::Bytecode,`. Remove
`PC_KIND_TREE_WALK`/`PC_KIND_BYTECODE` from its opcodes import. Replace
`MIN_FRAME_RECORD_BYTES` and its doc with

```rust
/// The smallest frame record the wire admits: a `nil` receiver (1), empty
/// `args` and `temps` counts (4 + 4), a `pc` (4), absent `sender`, `home` and
/// `current_class` (1 + 1 + 1), a `TopLevel` kind tag (1), and
/// `uncontinuable` (1). Every other frame kind is larger.
const MIN_FRAME_RECORD_BYTES: usize = 18;
```

(c) `inspect.rs`, the frame loop: delete the `let pc_kind = match r.read_u8()? { ... };`
statement, and change the format string's `pc {pc} ({pc_kind}), \` to
`pc {pc}, \`.

(d) `opcodes.rs`: replace the doc comment above `PC_KIND_TREE_WALK` with

```rust
/// Retired. A frame record once said which unit its `pc` counted in -- a
/// statement for the tree-walker, an instruction for the VM -- and only
/// bytecode was ever representable, so the byte was removed from the record.
/// Both numbers stay assigned so that a pc-unit field, if one ever returns,
/// cannot give them a new meaning.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test image` then `cargo test --workspace`
Expected: PASS. A failing test elsewhere that hand-writes a frame record must
lose its pc-kind byte too — search for `write_u8(1)` directly after a
`write_u32(...)` commented as `pc` and delete the byte, nothing else.

- [ ] **Step 5: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates.

```bash
git add -A crates
git commit -m "feat!: drop the pc-kind byte from the image's frame record

Only bytecode was ever representable, so the byte said nothing a reader did
not already know. Its two tag values are retired, not freed, and stay pinned;
a frame record carrying the old byte is refused. The minimum frame record
shrinks from 19 bytes to 18.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 4: The CLI stops offering the tree-walker

After this task nothing outside `ns-runtime` selects the tree-walker, so Task 5
can delete it without touching the CLI's behaviour.

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Delete: `crates/ns-cli/tests/differential.rs`
- Modify: `crates/ns-cli/tests/run.rs` (~line 186-211), `crates/ns-cli/tests/cli.rs` (~line 249-267), `crates/ns-cli/tests/build_load.rs` (doc comment ~line 100-110)

**Interfaces:**
- Consumes: `load_module` installs the bytecode engine (Task 1);
  `program_outcomes.rs` (Task 2).
- Produces: `fn bootstrap(loaded: &LoadedSource) -> (Interpreter, ClassRef)`
  replaces `bootstrap_with_engine`. `ns-next run` has no `--engine` flag.

- [ ] **Step 1: Change the CLI tests first**

(a) Delete `crates/ns-cli/tests/differential.rs`.

(b) `crates/ns-cli/tests/cli.rs`: replace `bench_reports_both_engines` with

```rust
#[test]
fn bench_reports_the_entry_sends_median() {
    let out = ns_next()
        .arg("bench")
        .arg(repo_root().join("tests-fixtures/while_loop.ns"))
        .arg("--iterations")
        .arg("3")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("median"), "{stdout}");
    assert!(!stdout.contains("tree-walk"), "{stdout}");
}

#[test]
fn run_no_longer_accepts_an_engine() {
    let out = ns_next()
        .arg("run")
        .arg(repo_root().join("tests-fixtures/while_loop.ns"))
        .arg("--engine")
        .arg("tree-walk")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
}
```

(c) `crates/ns-cli/tests/run.rs`,
`a_unary_entry_selector_gets_no_arguments_and_reads_its_temporary_as_nil`:
replace the `for engine in ["bytecode", "tree-walk"] { ... }` loop with its
body run once, without the two `--engine` arguments and with the `{engine}: `
prefixes removed from both assertion messages.

(d) `crates/ns-cli/tests/build_load.rs`: in the doc comment that says the
differential harness pins bytecode against tree-walk (~line 104-107), replace
the sentence starting "Because the differential harness already pins" with
"`run_fixtures.rs` pins the same fixtures' output against their `.expected`
files, so equality here also holds against a recorded answer."

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-cli --test cli`
Expected: `bench_reports_the_entry_sends_median` and
`run_no_longer_accepts_an_engine` FAIL.

- [ ] **Step 3: Implement in `main.rs`**

(a) Delete `enum EngineArg` and its doc comment, and `engine_label`.

(b) `Cmd::Run`: delete the `engine` field and its `#[arg(...)]`; in `main`,
the arm becomes `Cmd::Run { file, send } => cmd_run(&file, &send),`.

(c) Replace `bootstrap_with_engine` (doc comment included) with

```rust
/// Bootstraps an interpreter and loads the module's root class, which
/// compiles the module and installs its code -- before anything is
/// instantiated, so the root class's instance initializer runs compiled too.
fn bootstrap(loaded: &LoadedSource) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    (interp, class)
}
```

and change every caller: `bootstrap_with_engine(&loaded, engine)` and
`bootstrap_with_engine(&loaded, EngineArg::Bytecode)` become
`bootstrap(&loaded)`; `bootstrap_with_engine(loaded, engine)` in `bench_engine`
becomes `bootstrap(loaded)`.

(d) `cmd_run` loses its `engine: EngineArg` parameter; its doc comment's
"install the engine," goes.

(e) `report_entry_send`'s doc comment: replace "these strings are compared
byte for byte across engines by the differential harness, so they are shared
and never duplicated" with "`program_outcomes.rs` pins these strings byte for
byte, so they live here once and are never duplicated".

(f) `cmd_snapshot`'s doc comment: replace "Always the bytecode engine -- a
tree-walker closure carries `ClosureBody::Ast`, which the format has no
representation for, and the image's own code table is what a later `load`
runs." with "The image's own code table is what a later `load` runs."

(g) `bench`: rename `bench_engine` to `bench_entry` and drop its `engine`
parameter. Its doc: "Bootstraps and loads the module (which compiles it),
discards one warm-up `timed_send`, times `iterations` further ones, and
reduces the samples to `BenchStats`. Bootstrapping, loading and compiling all
happen before the loop -- once-per-process costs, excluded from the per-send
timing for the same reason instantiation is." `BenchStats`'s doc: "The
timing summary." In `cmd_bench`, replace the `for engine in [...] { ... }`
loop with

```rust
    match bench_entry(&loaded, &bindings, send, iterations) {
        Ok(stats) => println!(
            "median {:?} (min {:?}, max {:?}, n={})",
            stats.median, stats.min, stats.max, stats.iterations
        ),
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    }
```

change the `timing:` line's "{iterations} timed run(s) per engine" to
"{iterations} timed run(s)", and delete the trailing comment block and the
`note:` `println!` about the cross-engine ratio. Replace the debug-build
warning's "not the engines" with "not the runtime". Update the `Bench`
variant's doc: "Time the entry send: bootstrap, load and compile once, then
discard a warm-up run and time `--iterations` further runs, reporting the
median (with min and max) time for the entry send alone." `signal_message`'s
doc: replace "`cmd_bench` folds it into that engine's `Result` instead" with
"`cmd_bench` folds it into its `Result` instead".

(h) `cmd_disasm`, source arm: replace

```rust
        let bindings = loaded.resolved.bindings.clone();
        let (interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
        let module = ns_runtime::compiler::compile_module(interp.heap_ref(), &class, &bindings);
```

with

```rust
        let (interp, class) = bootstrap(&loaded);
        let module = interp
            .compiled_module()
            .expect("load_module installed the module's code")
            .clone();
```

and in its doc comment replace "bootstrap and compile the *whole* mixin tree
(`compiler::compile_module`, exactly as the bytecode engine would)" with
"bootstrap and load it, which compiles the *whole* mixin tree", and "The
source path never instantiates and never installs the bytecode engine --
compile only;" with "The source path never instantiates;".

(i) `run-corpus`: rename `EngineOutcome` to `RunOutcome` (doc: "What one
`ns-next run` or `load` of a whitelisted module produced."). `run_one` drops
its `engine` parameter and the two `--engine` arguments; its doc becomes "Runs
one module through a *child* `ns-next run`. A child process rather than an
in-process call on purpose: the module's stdout, which Newspeak code writes
straight to the process's stdout through `platform transcript`, is what gets
compared, and running it in this process would interleave it with the gate's
own output." In `cmd_run_corpus`, delete the `tree_walk` lane and the
`engines_disagree` check (and the `engines_disagree` function), rename the
`bytecode` variable to `source`, and in the binary lane's stderr check
replace "stderr differs from the bytecode engine's: binary {:?}, bytecode
{:?}" with "stderr differs from the source run's: binary {:?}, source {:?}".
Rewrite the function's doc comment's second paragraph as: "Every entry is run
from source and checked against the expectation; it is then run a second way
-- built into a stateless binary with `--entry <row.send>` and loaded with no
`--send`, so the recorded entry is what starts it -- and checked against the
same expectation, with stderr checked against the source run's. A row's `send`
doubling as the build's `--entry` is why `runnable.toml` needs no field of its
own for this lane. Each row still counts once in `ran`/`failed`: the first lane
that fails decides the row." In its third paragraph, delete "plus a check that
the two engines fail or succeed the same way,". Change the comment above
`let (mut ran, mut failed)` to "`ran` counts rows whose module actually
executed, ...". The `RunCorpus` variant's doc: "Run every whitelisted .ns file
under a directory, from source and as a built binary, and check each against
its recorded expected output."

- [ ] **Step 4: Verify**

Run: `cargo test --workspace`
Expected: PASS. Then `cargo run -p ns-cli -- run-corpus corpus/modules`
Expected: `ran 6, failed 0, skipped 205`.
Then `git grep -n -i "tree.walk\|EngineArg" crates/ns-cli`
Expected: no output.

- [ ] **Step 5: Gates and commit**

```bash
git add -A crates/ns-cli
git commit -m "feat!: stop offering the tree-walker from the CLI

run loses --engine, bench times the one engine, and run-corpus checks each
row from source and as a built binary. The engine differential is deleted;
its verdicts live on in program_outcomes.rs.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 5: Delete the tree-walker from the runtime

Everything that exists only because there were two engines goes. The
`bindings` parameters stay in this task — unused, but threaded — so the diff
stays about the engine; Task 6 removes them.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `eval.rs`, `instantiate.rs`, `nested.rs`
- Modify: `crates/ns-runtime/src/types.rs`, `heap.rs`, `vm.rs`, `bytecode.rs`, `lib.rs`, `mixin.rs`, `compiler/mod.rs`
- Modify: `crates/ns-runtime/src/image/mod.rs`, `encode.rs`, `decode.rs`
- Modify: `crates/ns-cli/src/main.rs` (only `compiled_module()` callers)
- Test: every file under `crates/ns-runtime/tests/` that names `Engine`, `set_engine`, `ClosureBody`, `PcKind` or `pc_kind`; `crates/ns-runtime/tests/interpreter.rs` is rewritten

**Interfaces:**
- Consumes: `install_code` (Task 1).
- Produces: `Interpreter::compiled_module(&self) -> &CompiledModule`;
  `ClosureData { params, code: Rc<CompiledCode>, captured_frame, home, bindings }`;
  `ActivationHeader { receiver, current_class, kind, slot_base }`;
  `Frame` without `pc_kind`; `CompiledModule: Default`. No `Engine`,
  `EngineUnavailable`, `ClosureBody`, `PcKind`.

- [ ] **Step 1: Rewrite the tests that depended on the tree-walker**

(a) `crates/ns-runtime/tests/interpreter.rs` builds classes by hand and runs
their bodies under the tree-walker, which Task 5 deletes. Replace the whole
file with module-based tests of the same behaviours:

```rust
//! Interpreter core behaviours through real modules: a method's implicit and
//! explicit return, a bare builtin class name, and non-local return from a
//! block to a live home and into a dead one.

use std::rc::Rc;

use ns_runtime::{Interpreter, NewspeakError, Signal, Symbol, Value};

/// Loads `src`, instantiates its root class and returns both.
fn instance_of(src: &str) -> (Interpreter, Value, Rc<std::collections::HashMap<ns_syntax::Span, ns_runtime::Binding>>) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let bindings = resolved.bindings.clone();
    let receiver = match interp.instantiate(class, vec![], &bindings) {
        Ok(v) => v,
        Err(Signal::Error(e)) => panic!("instantiate: unexpected Signal::Error: {}", e.message),
        Err(Signal::Return(..)) => panic!("instantiate: unexpected Signal::Return"),
    };
    (interp, receiver, bindings)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_cannot_return(result: Result<Value, Signal>, what: &str) {
    match result {
        Err(Signal::Error(NewspeakError { message, .. })) => assert!(
            message.contains("cannotReturn"),
            "{what}: expected cannotReturn, got: {message}"
        ),
        Ok(_) => panic!("{what}: expected cannotReturn, got a value"),
        Err(Signal::Return(..)) => panic!(
            "{what}: a return into a dead home escaped `send` as a raw Signal::Return"
        ),
    }
}

#[test]
fn method_with_no_explicit_return_returns_self() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public bar = ( 1. 2 )\n) : ()",
    );
    let result = expect_ok(
        interp.send(receiver, Symbol::intern("bar"), vec![], &bindings),
        "bar",
    );
    assert!(
        matches!((&result, &receiver), (Value::Obj(a), Value::Obj(b)) if a.same(b)),
        "a method with no explicit return must yield the receiver, not the last expression's value"
    );
}

#[test]
fn method_with_explicit_return_returns_the_returned_value() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public answer = ( ^42 )\n) : ()",
    );
    let result = expect_ok(
        interp.send(receiver, Symbol::intern("answer"), vec![], &bindings),
        "answer",
    );
    assert!(matches!(result, Value::SmallInt(42)));
}

/// A bare `OrderedCollection` (no `platform` prefix, outside any superclass
/// clause) names the builtin class.
#[test]
fn bare_builtin_class_name_reference_produces_a_working_instance() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public bar = ( ^OrderedCollection new )\n) : ()",
    );
    let result = expect_ok(
        interp.send(receiver, Symbol::intern("bar"), vec![], &bindings),
        "bar",
    );
    let Value::Obj(obj) = result else {
        panic!("expected an OrderedCollection instance")
    };
    assert!(matches!(
        interp.heap_ref().array_elements(&obj),
        Some(elements) if elements.is_empty()
    ));
}

/// `[ ^1 ] value`, invoked while its home method is still running, returns
/// from the home: `m` answers 1, and its second statement never runs.
#[test]
fn non_local_return_from_a_block_unwinds_to_the_home_methods_activation() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( [ ^1 ] value. ^2 )\n) : ()",
    );
    let result = expect_ok(
        interp.send(receiver, Symbol::intern("m"), vec![], &bindings),
        "m",
    );
    assert!(matches!(result, Value::SmallInt(1)));
    assert_eq!(interp.heap_ref().activation_depth(), 0);
}

/// The same block, stored and invoked again after its home returned, is
/// the spec's `cannotReturn` -- a `NewspeakError`, not a panic.
#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (|b.|) (\n  public m = ( b:: [ ^1 ]. ^b value )\n  public again = ( ^b value )\n) : ()",
    );
    let first = expect_ok(
        interp.send(receiver, Symbol::intern("m"), vec![], &bindings),
        "m",
    );
    assert!(matches!(first, Value::SmallInt(1)));
    expect_cannot_return(
        interp.send(receiver, Symbol::intern("again"), vec![], &bindings),
        "again",
    );
}

/// A method's activation is dead once it returns *normally*, not only after
/// an explicit `^`: `capture` stores `[^1]` and falls off the end.
#[test]
fn a_block_escaping_a_normally_returned_method_is_a_clean_cannot_return_error() {
    let (mut interp, receiver, bindings) = instance_of(
        "Newspeak3\n'T'\nclass Foo = Object new (|b.|) (\n  public capture = ( b:: [^1] )\n  public runIt = ( ^b value )\n) : ()",
    );
    expect_ok(
        interp.send(receiver, Symbol::intern("capture"), vec![], &bindings),
        "capture",
    );
    expect_cannot_return(
        interp.send(receiver, Symbol::intern("runIt"), vec![], &bindings),
        "runIt",
    );
}
```

(b) `crates/ns-runtime/tests/image.rs`:
- Delete these tests and their doc comments:
  `a_tree_walker_closure_is_a_snapshot_error_not_a_silent_omission`,
  `a_snapshot_of_a_tree_walker_closure_is_refused_rather_than_written`,
  `the_tree_walking_engine_is_refused_after_a_load`,
  `a_source_built_interpreter_still_accepts_the_tree_walking_engine`,
  `a_world_holding_a_tree_walker_activation_is_refused_when_written`.
- Every `let module = compiler::compile_module(...)` followed by
  `interp.set_engine(Engine::Bytecode(module)).expect(...)` directly after a
  `load(...)`: delete the `set_engine` statement; where `module` is used later
  (for example passed to `image::snapshot`), replace the `compile_module` line
  with `let module = interp.compiled_module().clone();`, otherwise delete it
  too. The same applies to the helper that `snapshot_module` and
  `closure_world` use.
- `let ClosureBody::Compiled(code) = &c.body else { ... };` becomes
  `let code = &c.code;`; `matches!(&outer.body, ClosureBody::Compiled(_))` is
  deleted with its `assert!`; a hand-built `ClosureData { body:
  ClosureBody::Compiled(x), ... }` becomes `ClosureData { code: x, ... }`.
- Delete every `pc_kind: ...,` line in a hand-built `Frame` or
  `ActivationHeader`.
- Fix the `use ns_runtime::{...}` list: drop `ClosureBody`, `Engine`, and
  `compiler` if it is no longer used.

(c) `crates/ns-runtime/tests/vm.rs`:
- `setup` becomes `load` (delete `setup`; replace every call to `setup(` with
  `load(`); drop the `compiler` and `Engine` imports.
- `loading_a_module_installs_its_compiled_code`: delete the
  `assert!(interp.compiled_module().is_some());` line and replace
  `assert!(matches!(closure.body, ClosureBody::Compiled(_)));` with
  `assert_eq!(closure.code.num_args, 0);` — the closure carries compiled code
  by type now; the test still pins that loading makes the class runnable.
- `a_correctly_sent_method_reads_its_temporary_as_nil_under_both_engines`
  becomes `a_correctly_sent_method_reads_its_temporary_as_nil`, with the
  `for build in [load, setup]` loop reduced to one `load` call.
- `a_wrong_arity_send_is_refused_identically_by_both_engines` becomes
  `a_wrong_arity_send_is_refused`: one `load`, one `expect_error`, and
  `assert_eq!(error.message, "wrong number of arguments: \`probe\` takes 0, got 2");`
  — no `messages` vector.
- Update the file's module doc to "Bytecode execution through the shared
  orchestration, every send driven via `Interpreter::send`."

(d) `crates/ns-runtime/tests/vm_initializer.rs` and
`crates/ns-runtime/tests/nested_classes.rs`: their `load` helpers take a
`bytecode: bool` (or install the engine by hand). Delete the parameter and the
install; every caller drops the argument; a test that loops over both engines
runs once. Rename any test whose name ends in `_under_both_engines` by dropping
that suffix. `nested_classes.rs`'s module doc's second paragraph ("Every
behavioural test runs under both engines ...") is deleted.

(e) Any other test file that still mentions `Engine`, `set_engine`,
`EngineUnavailable`, `ClosureBody`, `PcKind` or `pc_kind`:
`git grep -n "Engine\b\|set_engine\|EngineUnavailable\|ClosureBody\|PcKind\|pc_kind" crates/ns-runtime/tests`
lists them; apply the same rules.

- [ ] **Step 2: Delete from `types.rs`, `heap.rs`, `bytecode.rs`, `lib.rs`**

(a) `types.rs`: delete `pub enum ClosureBody` and its doc comment. In
`ClosureData`, replace `pub body: ClosureBody,` with

```rust
    pub code: Rc<crate::bytecode::CompiledCode>,
```

and replace the `bindings` field's doc comment with "Carried until the
run-time `bindings` parameter goes; nothing reads it." Delete `pub enum PcKind`
and its doc comment, and `Frame`'s `pub pc_kind: PcKind,` field. `Frame.pc`'s
doc becomes "How far this activation has progressed: an instruction index into
the `CompiledCode` it runs." In the `#[cfg(test)]` module, delete the
`pc_kind: PcKind::TreeWalk,` line.

(b) `heap.rs`: delete `pub pc_kind: PcKind,` from `ActivationHeader`,
`pc_kind: PcKind,` from `StackEntry`, `pc_kind: header.pc_kind,` in
`push_activation`, `pc_kind: entry.pc_kind,` in `reify`, and every
`pc_kind: PcKind::...,` line in the test module (four occurrences).

(c) `bytecode.rs`: add `Default` to `CompiledModule`'s `#[derive(...)]`.
Replace its doc comment's first sentence ("The bytecode engine's per-module
cache, installed on the `Interpreter` as `Engine::Bytecode(module)`.") with
"Every compiled body a module loaded into an `Interpreter` runs, installed by
`Interpreter::install_code`." In the `bindings` field's doc, replace "the map
is read only by `compiler::lower` (compile time) and by the tree-walker, and
neither runs on a loaded world" with "the map is read only by
`compiler::lower`, at compile time". `CompiledCode.num_registers`'s doc: "The
engine hooks size `Frame.temps`" becomes "An activation is pushed with".

(d) `lib.rs`: `pub use interpreter::{Engine, EngineUnavailable, Interpreter, Signal};`
becomes `pub use interpreter::{Interpreter, Signal};`, and the crate doc's
"tree-walking interpreter, and bytecode compiler/VM" becomes "bytecode
compiler and VM".

- [ ] **Step 3: Delete from `interpreter/mod.rs`**

- Delete `pub enum Engine`, `pub enum EngineUnavailable` and its `Display`
  impl, the `engine` and `decoded_from_image` fields, `set_engine`,
  `mark_decoded_from_image` and `run_statements`.
- Add the field `code: CompiledModule,` (doc: "Every compiled body every
  loaded module runs. `load_module` and `image::load` add to it through
  `install_code`; nothing runs a body that is not here.") and initialize it
  with `code: CompiledModule::default(),` in both `new` and `bootstrap`.
- `install_code` becomes

```rust
    pub(crate) fn install_code(&mut self, module: CompiledModule) {
        self.code.extend(module);
    }
```

- `compiled_module` becomes

```rust
    pub fn compiled_module(&self) -> &CompiledModule {
        &self.code
    }
```

- `invoke_method`: replace everything from the `// Hook 1` comment to the
  statement before `self.heap.pop_activation(&act);` with

```rust
        let code = self
            .code
            .methods
            .get(&method_key(&method))
            .cloned()
            .unwrap_or_else(|| {
                panic!("internal: dispatched method has no compiled form (compile_module coverage gap)")
            });
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver,
                current_class: Some(defining_class),
                kind: FrameKind::Method(method),
                slot_base: 0, // a method body never contains InitSlot
            },
            &args,
            (code.num_registers - code.num_args) as usize,
        );
        let result = crate::vm::run_code(self, &act, &code, &bindings);
```

  (`method` is moved into `FrameKind::Method`; the arity check above it reads
  `method.num_args` and `method.selector` before that, so no clone is
  needed.) Its doc: "Pushes an activation, runs the method's compiled body,
  and unwinds a matching `Signal::Return` back into an ordinary value." The
  comment inside the arity check that mentions "under both engines
  identically, which is why the differential harness cannot see it" becomes
  "silently, which no golden output necessarily shows".
- The module doc becomes "Sends, method invocation, and the `Signal`
  non-local-return/error channel, shared by everything that runs Newspeak
  code. Send helpers and local-variable access are in `eval.rs`; the bootstrap
  and class instantiation in `instantiate.rs`."

- [ ] **Step 4: Delete from `interpreter/eval.rs`**

- Delete `eval_expr`, `eval_ident`, `eval_literal`, `eval_send`, `eval_args`,
  `selector_of`, `eval_cascade`, `eval_assign`, `eval_block_literal` and
  `run_block_body`, with their doc comments.
- `invoke_closure` becomes

```rust
/// Invokes a closure with `args`. Called by `builtins::block` for `value`,
/// `value:`, `value:value:` and so on, and by every native that runs a block
/// argument, through `builtins::call_block`.
///
/// The invoking activation is not passed: a live activation's sender is the
/// one below it on the activation stack.
pub fn invoke_closure(
    interp: &mut Interpreter,
    closure: &Rc<ClosureData>,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    // A wrong argument count is refused before it can reach a register: the
    // window is laid out from the arguments actually given, so too few would
    // address registers past its end -- a panic -- and too many would
    // silently shift every temporary. Never panic on this
    // Newspeak-program-input path.
    if args.len() != closure.params.len() {
        let receiver = interp.heap.frame_receiver(&closure.captured_frame);
        return Err(Signal::Error(NewspeakError {
            selector: Symbol::intern("value"),
            receiver,
            message: format!(
                "wrong number of block arguments: block takes {}, got {}",
                closure.params.len(),
                args.len()
            ),
        }));
    }
    let receiver = interp.heap.frame_receiver(&closure.captured_frame);
    let current_class = interp.heap.frame_current_class(&closure.captured_frame);
    let code = &closure.code;
    let act = interp.heap.push_activation(
        ActivationHeader {
            receiver,
            current_class,
            kind: FrameKind::Closure(closure.clone()),
            slot_base: 0, // a closure body never contains InitSlot
        },
        &args,
        (code.num_registers - code.num_args) as usize,
    );
    let result = crate::vm::run_code(interp, &act, code, bindings);
    // Popped on every path out. A closure activation catches no return: its
    // `^` targets its home.
    interp.heap.pop_activation(&act);
    result
}
```

- Doc comments that name the tree-walker: `unresolved_identifier_error` ("in
  Milestone 1 and 2 reachable only for a bare `super`" stays; delete "Shared
  with the bytecode engine (...) so the two engines cannot drift apart in
  wording: the tree-walker is the differential oracle down to the byte." and
  write "Raised by `vm::make_value` for `bytecode::Literal::UnresolvedIdent`."),
  `malformed_int_error` (same treatment: "Raised by `vm::make_value` for
  `bytecode::Literal::MalformedInt`."), `read_outer_local`/`write_outer_local`
  ("Shared by the tree-walker's ... and the VM's ..." becomes "The VM's
  `LoadOuterLocal`." / "The VM's `StoreOuterLocal`."), `home_of` ("Shared by
  the tree-walker's `eval_block_literal` and the VM's `MakeClosure`." becomes
  "The VM's `MakeClosure` calls it."), `store_instance_field` ("Shared by
  `instantiate::run_own_slots` ... and `vm::run_code`'s `Instr::InitSlot`
  arm ..." becomes "`vm::run_code`'s `Instr::InitSlot` arm writes a slot
  initializer's value through it."), `frame_at_depth` unchanged.
- The module doc becomes "Send helpers (`LexicalSend`, `SuperSend`), local and
  outer-local access, closure invocation, and the errors the VM raises for a
  deferred literal."
- Remove imports this leaves unused (`ast::{self, Expr}` and anything else
  `cargo build` reports).

- [ ] **Step 5: Delete from `instantiate.rs` and `nested.rs`**

`instantiate.rs`:
- `run_initializer_level`: delete the `pc_kind: ...` field (and its comment)
  from the `ActivationHeader`; the call becomes
  `self.run_own_initializer(class, &act, bindings)`.
- `eval_superclass_message`: replace the `let compiled = ...;` and
  `let args_expr = ...;` statements and the `run_clause_body` call with

```rust
        let code = application
            .as_ref()
            .and_then(|application| {
                self.code
                    .superclass_messages
                    .get(&crate::mixin::mixin_key(&application.mixin))
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("internal: mixin whose superclass clause message takes arguments has no compiled form for it (compile_module coverage gap)")
            });
        // The message is evaluated on the instance being initialized and
        // with *this* level's factory arguments -- §6.2's "in the context of
        // the instance initializer" -- through the same activation the
        // nested-class accessor runs a clause's receiver in.
        let value = super::nested::run_clause_body(
            self,
            instance,
            factory_args.to_vec(),
            class,
            FrameKind::SuperclassMessage(*class),
            code,
            bindings,
        )?;
```

- `run_own_initializer` loses its `instance` parameter and becomes

```rust
    /// Class Instantiation steps 4-5 for one level of the chain: this class's
    /// compiled instance initializer -- its own slot initializers, then its
    /// own `initExprs` -- on that level's own activation, after growing the
    /// activation's register file to fit the body.
    ///
    /// `InitSlot`'s `own_index` is relative to the mixin's own slots, not an
    /// absolute field index, because the compiled body is shared by every
    /// application of the mixin. The activation's `slot_base` turns it into
    /// an absolute index at run time; `run_initializer_level` sets it once,
    /// from this level's own `MixinApplication.own_slot_base`, which the
    /// assertion below states.
    fn run_own_initializer(
        &mut self,
        class: &ClassRef,
        act: &ActivationRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        // A builtin, or any class with nothing to run: `compile_module` emits
        // no initializer for such a mixin, so there is nothing to look up.
        if !class_has_initializer_code(&self.heap, class) {
            return Ok(());
        }
        let application = self
            .heap
            .class_application(class)
            .expect("internal: class_has_initializer_code confirmed this is a class object");
        let code = self
            .code
            .initializers
            .get(&crate::mixin::mixin_key(&application.mixin))
            .cloned()
            .unwrap_or_else(|| {
                panic!("internal: class has no compiled instance initializer (compile_module coverage gap)")
            });
        debug_assert_eq!(
            self.heap.activation_slot_base(act),
            application.own_slot_base,
            "an initializer activation must be built for the level it runs"
        );
        self.heap
            .grow_activation_temps(act, (code.num_registers - code.num_args) as usize);
        crate::vm::run_code(self, act, &code, bindings).map(|_| ())
    }
```

- Delete `run_own_slots` and `run_own_init_exprs` with their doc comments.
- Delete `use super::{Engine, ...}`'s `Engine`. The comment in `build_mixin`
  that says "evaluated for real by `run_own_init_exprs` below (Step 5 of the
  spec's six numbered steps)" becomes "lowered into the instance initializer by
  `compiler::compile_mixin_initializer` (Step 5 of the spec's six numbered
  steps)". The comment on `factory:` that says "under both engines" drops
  those three words.

`nested.rs`:
- `eval_superclass_clause`: replace the `let compiled = ...;` and
  `let expr = ...;` statements and the call with

```rust
    let code = interp
        .code
        .superclass_clauses
        .get(&mixin_key(nested))
        .cloned()
        .unwrap_or_else(|| {
            panic!("internal: mixin with a superclass clause has no compiled form for it (compile_module coverage gap)")
        });
    run_clause_body(
        interp,
        receiver,
        vec![],
        declaring_class,
        FrameKind::SuperclassClause(*declaring_class),
        code,
        bindings,
    )
```

  Its doc's first sentence becomes "The superclass clause is Newspeak code: the
  body `compiler::compile_superclass_clause` emitted for this mixin runs in a
  fresh activation." In the "No clause at all" comment, delete "Neither engine
  runs any code for this, so neither can diverge."
- `run_clause_body`: delete `#[allow(clippy::too_many_arguments)]`, replace
  the parameters `compiled: Option<Rc<crate::bytecode::CompiledCode>>, expr:
  Option<&ast::Expr>,` with `code: Rc<crate::bytecode::CompiledCode>,`, the
  `temp_count` computation with `(code.num_registers - code.num_args) as usize`
  passed straight to `push_activation`, delete the `pc_kind` field, and
  replace the `let result = match (&compiled, expr) { ... };` with
  `let result = crate::vm::run_code(interp, &act, &code, bindings);`. Its doc:
  "One per-mixin clause body, compiled, in a fresh activation. Everything
  around the body -- the activation, the receiver, the `Signal::Return` catch
  -- is the same shape as `Interpreter::invoke_method`'s." The rest of the
  doc (what it shares with `instantiate`, and why both kinds are return
  targets) stays; drop "through the engine hook" wherever it appears.
- Remove imports this leaves unused.

- [ ] **Step 6: Delete from `vm.rs`, `mixin.rs`, `compiler/mod.rs`, `builtins/`**

- `vm.rs`, `MakeClosure`: `body: ClosureBody::Compiled(block.clone()),` becomes
  `code: block.clone(),`. Module doc: replace "which re-enters `run_code`
  through the engine hooks for any user-defined target" with "which re-enters
  `run_code` for any user-defined target", and "that is what makes the
  tree-walker a byte-for-byte oracle" with "dispatch lives in one place, which
  the reference lowering and a JIT will share". `run_code`'s doc: "pushed by
  the shared orchestration (`invoke_method`, `invoke_closure`, `instantiate`
  or `nested::run_clause_body`)" stays.
- `mixin.rs`: in the module doc, "are read by the compiler, which runs before
  an image exists, and by the tree-walking engine, which cannot run from an
  image at all" becomes "are read by the compiler, which runs before an image
  exists". `MixinMethod::decl`'s doc: "the compiler lowers its body, and the
  tree-walking engine evaluates it" becomes "the compiler lowers its body".
  `MixinSlot::decl`: "which only the compiler and the tree-walker read" becomes
  "which only the compiler reads". `Mixin::init_exprs`: "evaluated directly by
  the tree-walker" becomes "lowered by the compiler into the instance
  initializer". `Mixin::decl`: "and the tree-walker evaluates that clause's" —
  rewrite the sentence to say only that the compiler lowers its superclass
  clause.
- `compiler/mod.rs`: fix any doc comment that names the tree-walker
  (`git grep -n -i "tree.walk" crates/ns-runtime/src/compiler`).
- `builtins/mod.rs`: `call_block`'s doc mentions `invoke_closure` only; leave
  it. `builtins/block.rs`: unchanged.

- [ ] **Step 7: Delete from `image/`**

- `image/mod.rs`, `load`: replace
  `interp.set_engine(Engine::Bytecode(module)).map_err(|e| e.to_string())?;`
  and the `mark_decoded_from_image` call (with its comment) with
  `interp.install_code(module);`. Delete `Engine` from the imports.
  `LoadedImage.interp`'s doc becomes "Bootstrapped, with the image's own code
  table installed." In `empty_bindings`' doc, the bullet
  "`interpreter/eval.rs` -- the tree-walker, which cannot run from an image at
  all: ..." is deleted and "at exactly two kinds of call site, and neither is
  reachable here" becomes "at one kind of call site, which is not reachable
  here". `load`'s doc: "Installs `Engine::Bytecode`" becomes "Installs the
  code table".
- `encode.rs`: `encode_closure_record` loses its `let ClosureBody::Compiled(code)
  = &closure.body else { ... };` and uses `&closure.code` where it wrote
  `code`; its doc keeps only "`params`, `code_id`, `captured_frame` id, `home`
  id. `bindings` is not serialized: nothing reads one." `encode_frame` loses
  the `if f.pc_kind == PcKind::TreeWalk { ... }` block Task 3 left. Remove
  `ClosureBody` and `PcKind` from the imports; if `encode_closure_record`
  and `encode_frame` can no longer fail for any reason, leave their `Result`
  return types alone — B decides that.
- `decode.rs`: `decode_closure_record`'s `body: ClosureBody::Compiled(code),`
  becomes `code,` (delete the comment above it); delete `pc_kind:
  PcKind::Bytecode,` from `decode_frame`'s and `placeholder_frame`'s `Frame`
  literals. In `frame_body` (~line 2171), the arm
  `FrameKind::Closure(closure) => match &closure.body { ... },` becomes
  `FrameKind::Closure(closure) => Some(closure.code.clone()),`. In
  `verify_closure` (~line 2258), replace

  ```rust
      let ClosureBody::Compiled(code) = &closure.body else {
          // As in `frame_body`: the format cannot express an AST body.
          return Err(DecodeError::WrongCodeKind { expected: "block" });
      };
  ```

  with `let code = &closure.code;`. In `verify_graph`'s doc, item 0's
  parenthesis "(`PcKind::Bytecode` is the only value the format admits)"
  becomes "(an image carries only bytecode)", and "The frame record says its
  `pc` counts instructions" becomes "A frame's `pc` counts instructions".
  Remove `ClosureBody` and `PcKind` from the imports.
- `inspect.rs`: nothing left to change after Task 3.

- [ ] **Step 8: The CLI's `compiled_module()` callers**

`compiled_module()` now returns `&CompiledModule`, not an `Option`. In
`crates/ns-cli/src/main.rs`, each
`interp.compiled_module().expect("...")` becomes `interp.compiled_module()`
(in `cmd_snapshot`, `cmd_build` and both arms of `cmd_disasm`; in
`cmd_disasm` keep the `.clone()`).

- [ ] **Step 9: Build and test**

Run: `cargo build --workspace` and fix every error by the rules above, then
`cargo test --workspace`.
Expected: PASS.
Then:
`git grep -n -i "tree.walk\|treewalk\|ClosureBody\|PcKind\|pc_kind\|set_engine\|EngineUnavailable\|decoded_from_image\|Engine::" crates`
Expected: only `opcodes.rs`'s retired `PC_KIND_*` constants and their pins in
`opcode_tags.rs`, and nothing else. Any other hit is a leftover.

- [ ] **Step 10: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates.

```bash
git add -A crates
git commit -m "feat!: delete the tree-walking engine

The bytecode VM is the only engine. Engine, set_engine, EngineUnavailable,
the tree-walk halves of the three hooks, the AST evaluator, ClosureBody and
PcKind are gone; a closure carries its compiled code directly, and the
interpreter holds one code table that every load adds to. The tests that
built classes by hand to run them under the tree-walker now load real
modules.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 6: Remove the run-time `bindings` parameter

The resolver's `Span -> Binding` map is compile-time input. At run time only
the tree-walker read it; every run-time signature has carried it unread since
Task 5. It goes from the runtime's run-time API and stays only where the
compiler takes it.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `eval.rs`, `instantiate.rs`, `nested.rs`, `vm.rs`, `builtins/mod.rs`, `builtins/block.rs`, `types.rs`, `bytecode.rs`, `image/mod.rs`, `image/decode.rs`, `image/encode.rs`
- Modify: `crates/ns-cli/src/main.rs`
- Test: every test under `crates/ns-runtime/tests/` and `crates/ns-cli/` that passes bindings to `send` or `instantiate`

**Interfaces:**
- Produces:
  - `Interpreter::send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>) -> EvalResult`
  - `Interpreter::instantiate(&mut self, class: ClassRef, factory_args: Vec<Value>) -> EvalResult`
  - `Interpreter::invoke_method(&mut self, receiver: Value, method: Rc<MixinMethod>, defining_class: ClassRef, args: Vec<Value>) -> EvalResult`
  - `eval::invoke_closure(interp: &mut Interpreter, closure: &Rc<ClosureData>, args: Vec<Value>) -> EvalResult`
  - `vm::run_code(interp: &mut Interpreter, act: &ActivationRef, code: &Rc<CompiledCode>) -> EvalResult`
  - `ClosureData { params, code, captured_frame, home }`
  - `CompiledModule` without `bindings`; `LoadedImage` without `empty_bindings`.
  - Unchanged: `compiler::compile_module(heap, root_class, bindings)`,
    `compile_initializer`, and everything else under `compiler/`, and
    `ResolvedModule.bindings`.

- [ ] **Step 1: Change the signatures**

Remove the `bindings: &Rc<HashMap<Span, Binding>>` (or by-value
`Rc<HashMap<Span, Binding>>`) parameter, and the argument at every call site,
from: `Interpreter::send`, `dispatch_send`, `dispatch_native_or_dnu`,
`does_not_understand`, `invoke_method`, `try_factory_send`, `instantiate`,
`instantiate_unchecked`, `run_initializer_level`, `eval_superclass_message`,
`run_own_initializer`, `nested::nested_class`, `eval_superclass_clause`,
`run_clause_body`, `eval::dispatch_lexical_send`, `dispatch_super_send`,
`invoke_closure`, `vm::run_code`, `block::run_while`. In `builtins/mod.rs`,
`call_block` stops reading `closure.bindings`:

```rust
                Some(closure_data) => {
                    let closure = std::rc::Rc::new(closure_data);
                    crate::interpreter::eval::invoke_closure(interp, &closure, args)
                }
```

and its doc drops the paragraph beginning "It invokes the closure under
`closure.bindings.clone()`". In `builtins/block.rs`, the two `&closure.bindings`
arguments go, and `run_while`'s `interp.send(other, ..., vec![], bindings)`
becomes `interp.send(other, Symbol::intern("mustBeBoolean"), vec![])`.

- [ ] **Step 2: Remove the fields**

- `types.rs`: delete `ClosureData.bindings` and its doc.
- `vm.rs`, `MakeClosure`: delete `bindings: bindings.clone(),`.
- `bytecode.rs`: delete `CompiledModule.bindings` and its doc;
  `compiler::compile_module` stops setting it.
- `image/decode.rs`: delete `DecodeContext.bindings` and the
  `bindings: ctx.bindings.clone(),` line in `decode_closure_record` (with its
  comment); `build_module` loses its bindings parameter.
- `image/encode.rs`: `encode_closure_record`'s doc drops the `bindings`
  sentence.
- `image/mod.rs`: delete `LoadedImage.empty_bindings` and its long doc comment,
  the `let empty_bindings = ...;` in `load`, and every use.
- Remove imports this leaves unused (`HashMap`, `Span`, `Binding` in most of
  these files).

- [ ] **Step 3: The CLI and the tests**

In `crates/ns-cli/src/main.rs`, every `let bindings = loaded.resolved.bindings.clone();`
goes, with the argument at each `send`/`instantiate`/`timed_send`/`bench_entry`
call; `timed_send` and `bench_entry` lose their `bindings` parameter. In the
tests, the same mechanical change: every `interp.send(r, s, a, &bindings)`
becomes `interp.send(r, s, a)`, every `instantiate(c, args, &bindings)`
becomes `instantiate(c, args)`, and helpers that returned bindings only to pass
them on stop returning them. A test helper that still needs bindings for
`compiler::compile_module` (for example in `compiler.rs`) keeps them.

- [ ] **Step 4: Build and test**

Run: `cargo build --workspace`, then `cargo test --workspace`.
Expected: PASS.
Then: `git grep -n "bindings" crates/ns-runtime/src -- ':!crates/ns-runtime/src/compiler' ':!crates/ns-runtime/src/resolver'`
Expected: only `types.rs`'s `ResolvedModule.bindings`, `interpreter/instantiate.rs`'s
`load_module` passing `&resolved.bindings` to `compile_module`, and doc
comments that describe the resolver's map. No run-time signature.

- [ ] **Step 5: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates.

```bash
git add -A crates
git commit -m "refactor!: take the resolver's bindings out of every run-time signature

The Span -> Binding map is compile-time input; at run time only the
tree-walker read it. send, instantiate, invoke_method, invoke_closure,
run_code and the natives no longer thread it, closures no longer carry it,
and a loaded image no longer needs an empty one to pass along.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 7: Documents, measurement, and PROGRESS

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/design/execution.md`, `compiler.md`, `memory.md`, `image-format.md`, `dispatch.md`, `object-model.md`, `frontend.md`, `README.md`
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: Measure in one sitting**

The "before" is `master` (the merge-base), built in a throwaway worktree:

```bash
git worktree add ../ns-next-before master
cargo build --release --manifest-path ../ns-next-before/Cargo.toml
cargo build --release
```

Three rounds, alternating the two binaries within each round, over
`tests-fixtures/bench_loop.ns`, `bench_send.ns` and `bench_block.ns`:

```bash
../ns-next-before/target/release/ns-next bench tests-fixtures/bench_loop.ns
target/release/ns-next bench tests-fixtures/bench_loop.ns
```

(and the same pair for `bench_send.ns` and `bench_block.ns`, three rounds.)
Record the **bytecode** median of each run; the before-binary also prints a
tree-walk line, which is not compared. Expected: no movement beyond noise —
`invoke_method` lost a `match` on the engine and an `Option`, nothing on the
hot path grew. If a median moves by more than the spread between rounds,
investigate before going on. Then `git worktree remove ../ns-next-before`.

Also measure the sizes the image-decoding amplification table depends on. Add
this temporary test at the end of `heap.rs`'s `mod tests`:

```rust
    #[test]
    fn print_sizes() {
        eprintln!(
            "Object {} Frame {} FrameSlot {}",
            std::mem::size_of::<Object>(),
            std::mem::size_of::<Frame>(),
            std::mem::size_of::<FrameSlot>()
        );
    }
```

run `cargo test -p ns-runtime --lib print_sizes -- --nocapture` and
`cargo test -p ns-runtime --lib --release print_sizes -- --nocapture`, write
the four figures down, and delete the test. Do not commit it.

- [ ] **Step 2: `CLAUDE.md`**

- Intro: "lexer, parser, resolver, tree-walking interpreter, bytecode compiler
  and register VM, and a versioned image format" becomes "lexer, parser,
  resolver, bytecode compiler and register VM, and a versioned image format".
  `crates/ns-runtime`'s line: "both execution engines (`interpreter/` and
  `compiler/` + `vm.rs`)" becomes "the compiler (`compiler/`), the VM
  (`vm.rs`) and the send machinery it calls (`interpreter/`)".
- Replace the invariant **Two engines must agree byte for byte.** with:

  **Program outcomes are pinned, not re-derived.** Until the reference
  lowering lands (`docs/design/compiler.md`, "Planned: the reference
  lowering") there is no second way to run a program and compare against.
  What protects behaviour is recorded answers: every `tests-fixtures/*.ns`
  against its `.expected` stdout and an empty stderr, the golden exit status,
  stdout and stderr in `crates/ns-cli/tests/program_outcomes.rs`, and
  `run-corpus`. A change that alters one is a behaviour change: establish that
  the new output is right before updating the golden, never update it because
  the code now produces it.

- Replace **`interpreter/` is shared orchestration, not the tree-walker's
  private code.** with:

  **`vm.rs` holds no dispatch logic.** Send, `doesNotUnderstand:`, lexical,
  super and native dispatch, frame construction and `instantiate` live in
  `interpreter/`; the VM's instruction loop calls them. Never copy dispatch
  logic, lookup logic or error text into `vm.rs` or `compiler/`: the
  reference lowering and, later, a JIT must reach the same dispatch and raise
  the same errors, and a second copy is how they would drift. If you need
  something private there, widen it to `pub(crate)`.

- **Compiling an already-resolved module can never raise a `NewspeakError`.**:
  replace its last sentence ("A *program-level* error that Milestone 1 raises
  at evaluation time is deferred to execution instead (...), so both engines
  report it identically.") with "A *program-level* error the language raises
  at evaluation time is deferred to execution instead (see `Literal::MalformedInt`
  and `Literal::UnresolvedIdent`), so a method that is never run never fails."
- **Compiled code is keyed by mixin ...**: replace the last sentence ("The
  tree-walker must resolve the same slot by name ... not as a single wrong
  answer.") with "Getting it wrong is silent wrong output under one
  application and correct output under another; `vm_initializer.rs` pins
  both."
- Delete the invariant **The tree-walking engine cannot run from a loaded
  image.**
- The image invariant's amplification figures: recompute from Step 1's sizes.
  The object factor is `Object` size / 6, the frame factor is `FrameSlot`
  size / 18, and the combined figure is their sum; replace "about **17.8x in a
  release build** (an `Object` is 64 bytes against a 6-byte minimum record, a
  frame-arena entry 136 against 19) and about 22.2x in a debug build" with the
  new release and debug figures in the same sentence shape.
- "Working in this repository": `# the full suite, including the differential
  harness that compares the two engines` becomes `# the full suite, including
  the golden program outcomes`. The `run-corpus` sentence "`run-corpus` runs
  every listed module under both engines and fails if they disagree" becomes
  "`run-corpus` runs every listed module from source and as a built binary and
  fails if either differs from the recorded output".
- `git grep -n -i "tree.walk\|both engines\|two engines" CLAUDE.md` must come
  back empty.

- [ ] **Step 3: The design documents**

Each describes the current state after this plan; history goes to git.

- `execution.md`:
  - Status block: drop "the three engine hooks" wording; code list names
    `interpreter/` as "the send machinery and activation construction".
  - Replace "## Two engines, permanently" and "## Planned: one engine" with a
    "## One engine" section: the bytecode VM is the only engine; the
    tree-walker was retired on 2026-09-11 because a recursive AST evaluator
    keeps its computation on the Rust stack and so cannot take part in
    suspension, the debugger or edit-and-continue, which the trampoline builds
    on, and because it was blind to defects in the orchestration both engines
    shared. Verification until the reference lowering lands is the golden
    outcomes (name the three sources the CLAUDE.md invariant names); the
    reference lowering ([compiler.md](compiler.md)) is the planned
    differential oracle.
  - "## The VM and the three hooks" becomes "## The VM": `invoke_method`,
    `invoke_closure`, `run_initializer_level` and `nested::run_clause_body`
    push an activation and call `run_code`; the paragraphs about hooks, about
    "a closure always runs with the engine that created it" and the Decision
    bullet with the same content go.
  - Every other sentence naming the tree-walker: the Representation bullet on
    what gets reified (keep the VM half: an activation nothing reifies leaves
    nothing behind); "Lifecycle" (reify's callers are the VM's `MakeClosure`,
    directly and through `home_of`); "What an activation carries" (the `pc` /
    `pc_kind` paragraph becomes: `pc` is an instruction index into the
    activation's compiled body); "Where `pc` is stored" (drop the tree-walker's
    statement index); "Non-local return" (drop the tree-walker's method-level
    `^`); "Planned: the trampoline" (the "What stays recursive" paragraph's
    "the same way the tree-walker's are" goes).
  - Measurements: keep every dated table's **bytecode** columns and the prose
    about them; delete the tree-walk columns and the paragraphs that only
    discuss the tree-walker (its `bench_loop.ns` gain, its maximum, its
    decomposition). Add a subsection "### What retiring the tree-walker moved"
    with Step 1's table (before and after bytecode medians per round for the
    three sources) and one sentence stating whether anything moved.
  - Decisions: replace "**The tree-walker is permanent**, and is the
    reference when the engines disagree." with "**One engine.** The
    tree-walker was retired because it could not represent a suspended
    computation; golden outcomes protect behaviour until the reference
    lowering becomes the differential oracle." Delete "**A closure runs with
    the engine that created it** ...". In "**`vm.rs` holds no dispatch
    logic.**", "which is what makes the differential harness meaningful rather
    than approximate" becomes "so the reference lowering and a JIT share it".
- `compiler.md`: "Planned: the reference lowering" loses "It is the
  differential oracle that replaces the tree-walker" in favour of "It is the
  differential oracle the retired tree-walker used to be"; every other mention
  of the tree-walker or of both engines (`git grep -n -i "tree.walk\|both
  engines" docs/design/compiler.md`) is rewritten to describe the VM alone —
  in "Testing the fallbacks", the coverage is now `program_outcomes.rs` and the
  compiler's own tests.
- `memory.md`: the tree-walker's retention ("Open: what the collector owns",
  the 136.8 MB against 4.6 MB comparison) and its handle discipline under a
  moving collector are gone with it; keep what those sections say about the
  VM. The amplification table (~line 416-423): the frame record minimum is 18
  B, and the `Object`, `Frame` and `FrameSlot` sizes and factors are Step 1's.
- `image-format.md`: the frame record has no pc-kind byte; the retired
  `PC_KIND_*` numbers are mentioned where the other tags are; no mention of a
  tree-walker frame or closure being refused.
- `dispatch.md`, `object-model.md`, `frontend.md`: one mention each; rewrite
  it for one engine.
- `README.md`: intro drops "tree-walking interpreter"; the map row
  "Activations, both engines, the VM loop" becomes "Activations, the VM loop"
  with "Planned next" `the trampoline, closure conversion, JIT tiers`; the
  compiler row keeps `the reference lowering, inline-cache slots wired up`;
  the architecture block's `ns-runtime/` line drops "both engines"; the gates
  comment `# includes the two-engine differential harness` becomes
  `# includes the golden program outcomes`.
- `git grep -n -i "tree.walk\|both engines\|two engines" docs/design` must
  come back empty except for the one sentence per document that says the
  tree-walker was retired, where one is needed.

- [ ] **Step 4: `docs/PROGRESS.md`**

- The M4 row's Plan column gains this plan's link,
  `[retire-tree-walker](superpowers/plans/2026-09-11-1848-ns-next-retire-tree-walker.md)`.
- "Where the next session starts": step A is done; **the next piece is B**,
  whose detailed plan is written first. Keep C and 1b as they are.
- Add a section after "M4 — The activation stack":
  `## M4 — Retiring the tree-walker (YYYY-MM-DD)`, with the date the step completes, `Plan: [...]`, stating: what
  was harvested (Task 1's result — the four expected failures and any real
  divergence found; Task 2's thirteen golden outcomes); what was deleted
  (the list from Task 5 and Task 6, with the image's frame record losing a
  byte in Task 3); what replaced the differential for now; the measurement
  table from Step 1; test count and both corpus gates.

- [ ] **Step 5: Verify and commit**

Run `cargo test --workspace` (nothing in `docs/` is compiled, but the
numbers recorded must be the ones measured), then

```bash
git add CLAUDE.md docs
git commit -m "docs: describe one engine, record the harvest and the measurement

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 6: A fresh session can start here**

Read `CLAUDE.md`, `docs/design/README.md` and PROGRESS's "Where the next
session starts" as a newcomer would. They must name B as the next step, say
that its plan is written first, and point at the design text for B (PROGRESS's
step list) and C (compiler.md). Fix anything that points at the tree-walker,
`--engine` or `differential.rs`.
