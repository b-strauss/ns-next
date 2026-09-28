# Simultaneous Slots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** §6.3.2's simultaneous slot clauses `|| ... ||` initialize their slots through pull futures, so well-founded mutual recursion between them resolves, and after the clause every slot holds its value and no program can observe a future.

**Architecture:** The parser keeps the `simultaneous` flag it discards today. The resolver resolves a simultaneous initializer as if it were the body of a block, and both lowerings, from one shared helper, wrap it in that block, make a future of it (`MakeFuture`), store it — provisionally, past §3.1.1's check, for a class slot — and at the clause's end read each slot back, resolve it (`ResolveFuture`) and store the value. A future is an instance of a kernel-private `Future` class whose state `interpreter/future.rs` alone reads and writes; forcing one is a trampoline step (the closure's activation under a new `Continuation::Future`), never a call back into Newspeak. Where Rust meets a future — a leaf's failure path, an equality leaf's "false", `InitializeSlot`'s refusal of a non-value — a resolved one is replaced by its value, and an unresolved one is forced under `Reexecute`, which runs the instruction that met it again once it is resolved. The collector replaces resolved futures in object fields, and the image writes a resolved future as its value.

**Tech Stack:** Rust 1.98.0 (edition 2021), crates `ns-syntax`, `ns-runtime`, `ns-cli`, `ns-lsp`; Newspeak source in `crates/ns-runtime/core-library/Kernel.ns`; `cargo-nextest`; `indoc` (dev-dependency).

**Spec:** `docs/design/object-model.md`, "Planned: simultaneous slots", and its pointers in `compiler.md` ("Planned: the simultaneous slot clause's desugaring"), `execution.md` ("Planned: running an instruction again after forcing a future"), `core-library.md` ("Planned: the pull future"), `memory.md` (the "Planned: a resolved future is traced as its value" paragraph) and `image-format.md` ("Planned: futures in a graph"). Read them before any task. Where this plan departs from the recorded design it says so, and the task named writes the departure into the design:

- **Forcing is Rust over trampoline steps, not a Newspeak `resolve` method; the lowering uses instructions, not class-side `computing:` and `valueOf:`.** A future is made by `MakeFuture` and resolved by `ResolveFuture`; `Future`'s only Newspeak is its protected overrides and a `doesNotUnderstand:` that forwards through a mirror leaf. This keeps the class unreachable even from compiled code (no instruction names a kernel class), and gives the leaves, `InitializeSlot` and the clause's end one forcing path (`interpreter::future::force`). Written into the design by Task 2. *To be confirmed by the user before execution.*
- **A future a host passes straight to a leaf ends the run with an error** instead of the leaf declining: `Reexecute` needs an instruction to run again, and a host's send has none. Written into the design by Task 5. *To be confirmed by the user before execution.*
- **The class's name is `Future`**, §6.3.2's (the design lists `Deferred` as offered and undecided). *To be confirmed by the user before execution.*

## Global Constraints

- All code, identifiers, comments, doc comments and commit messages are in English; conversation with the user is in German.
- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; `clippy.toml`'s `min_ident_chars`). Every code sample below follows it.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult` or a `HostResult`. Tests unwrap through a local `expect_ok` helper copied from a neighbouring test file (`crates/ns-runtime/src/interpreter/mirror.rs`'s test module has one).
- A Newspeak module written inline in a test is an `indoc! {"…"}`, indented with spaces, the closing `"}` on a line of its own.
- Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`; add a missing accessor there.
- `vm.rs` holds no dispatch logic: its new arms read registers, call one function in `interpreter/` and carry out the `Flow` it implies. What a future is, whether it is resolved and how it is forced are decided in `interpreter/future.rs`.
- A leaf never calls back into Newspeak (`ns-next-builtins-must-stay-optimizable`): forcing is a `Step`.
- Compiling an already-resolved module never raises a `NewspeakError`; a compiler-invariant violation is `panic!` / `debug_assert!`.
- An image is untrusted input. A wire tag is assigned once and never renumbered or reused: the four new instructions take opcodes **36, 37, 38, 39** (`image/opcodes.rs`, highest today 35); `Future` takes builtin class tag **30** ("Next free number: 30"), and both "Next free number" comments move on.
- The VM mirror is a capability: nothing here gives a program the mirror, its class or a `MirrorSend` binding. `Future` is private in `Kernel.ns`, `Object` declares no accessor for it, and the encoder and decoder treat its class as they treat `Activation`'s.
- Program outcomes are pinned: a golden's recorded `status`, `stdout` and `stderr` change only where a task names the change and says why. Establish that a new golden's output is what §6.3.2 requires before recording it; never record what the code happens to print.
- Conventional commits. Every commit message ends with a `Co-Authored-By:` trailer naming the model that actually wrote the code, a subagent's commits included, then the `Claude-Session:` line the session's attribution reminder gives.
- **Before every commit** (`CLAUDE.md`, "Working in this repository"):
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  Task 1 touches `crates/ns-syntax`, so its commit also runs `cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`. A commit that changes only Markdown runs none of them.
- The end-of-batch gates (the three stress modes, `ns-lsp`, `run-corpus` and its stress variants) run once, in Task 9. The plugin's `verifyPlugin buildPlugin` does not run: nothing under `editors/jetbrains` changes.
- Design documents are living: a task updates the document for the aspect it changed, in place; Task 9 turns "Planned: simultaneous slots" into the present-tense description.
- Every command runs in the foreground; a subagent never hands back while its own background work runs.

## Review Focus

1. **A sibling read inside a block of a simultaneous temporary's initializer**, `|| a = [b + 1]. b = 2. || ^a value` — the block captured by `a`'s future block must see `b`'s *resolved* value, not the nil its register held when the closure was made. This fails silently (exit 0) if the capture analysis treats a simultaneous immutable local as read-only and copies it. Pinned in Task 4 (`goldens/spec/spec_6_3_2_a_captured_sibling_is_read_after_the_clause`).
2. **An initializer that raises, caught outside the instantiation**, then a later touch of a future that escaped — must raise the same error again (recompute), never "Divergent evaluation" left behind by a stuck `resolving` state. Pinned in Task 2 (`abandoning_a_forcing_resets_the_future`) and Task 4 (`goldens/spec/spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch`).
3. **A future as the argument of an equality leaf**, `3 = second`, `'a' = second`, `x == second` and `second == x` during the clause — must answer as the value would, both directions. Pinned in Task 5 (`goldens/spec/spec_6_3_2_a_future_is_equal_and_identical_to_its_value`).
4. **A value class whose simultaneous initializer builds another value around a sibling**, `|| p = Pair first: q second: 1. q = 3. ||` — the `Pair` must hold `3`, not a future, and must compare equal to `Pair first: 3 second: 1`. Pinned in Task 6.
5. **A snapshot of module state holding an array into which a future escaped** — the image must hold the value, and after `load` `==` between the array element and the slot must still be true. Pinned in Task 8 (`goldens/spec/spec_6_3_2_a_data_cycle_survives_an_image` via the golden round trip).

---

## File Structure

- `crates/ns-syntax/src/ast.rs`, `crates/ns-syntax/src/parser/declaration.rs` — `SlotDeclaration.simultaneous` (Task 1).
- `crates/ns-runtime/src/resolver/mod.rs`, `resolver/scope.rs` — a simultaneous initializer resolved inside a block scope (Task 4).
- `crates/ns-runtime/src/compiler/simultaneous.rs` (new, Task 4) — the one desugaring helper both lowerings and the capture analysis use.
- `crates/ns-runtime/src/compiler/mod.rs`, `lower.rs`, `reference.rs`, `capture.rs` — the lowering (Task 4).
- `crates/ns-runtime/src/bytecode.rs`, `image/opcodes.rs`, `image/encode.rs`, `image/decode.rs`, `image/inspect.rs` — the four instructions (Task 3).
- `crates/ns-runtime/src/interpreter/future.rs` (new, Task 2) — recognizing, reading, forcing and seeing through a future.
- `crates/ns-runtime/core-library/Kernel.ns` — `Future` (Task 2).
- `crates/ns-runtime/src/types.rs` — `BuiltinClass::Future`, `Continuation::Future` (Task 2).
- `crates/ns-runtime/src/interpreter/mod.rs`, `interpreter/kernel.rs`, `trace.rs`, `heap.rs` — `Mutator` fields and methods, bootstrap, tracing (Tasks 2, 7).
- `crates/ns-runtime/src/vm.rs` — `finish`'s `Future` arm, `Reexecute`, the four instruction arms, `InitializeSlot`'s refusal path (Tasks 2, 3, 6).
- `crates/ns-runtime/src/builtins/mod.rs`, `builtins/primitive.rs`, `builtins/future.rs` (new) — the `futureValue:ifFail:` leaf (Task 2) and the see-through wrapper (Task 5).
- `crates/ns-runtime/src/heap/world.rs` — replacing resolved futures before a collection (Task 7).
- Tests: `crates/ns-syntax` parser tests (Task 1); `crates/ns-runtime/tests/future.rs` (new, Tasks 2, 5, 6, 7), `tests/opcode_tags.rs`, `tests/image.rs`; unit tests in `compiler/capture.rs`, `interpreter/kernel.rs`.
- Goldens: `goldens/spec/spec_6_3_2_*` (Tasks 4–8), `goldens/spec/spec_5_1_8_a_code_body_declares_simultaneous_temporaries` (Task 4).
- Docs: `docs/design/object-model.md`, `frontend.md`, `compiler.md`, `execution.md`, `core-library.md`, `memory.md`, `image-format.md`, `dispatch.md`, `README.md`, `docs/PROGRESS.md`.

---

### Task 1: The parser keeps the `simultaneous` flag

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs:62-72` (`SlotDeclaration`)
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`parse_slot_declarations` ~line 218, `parse_slot_section` ~line 234, the `SlotDeclaration { .. }` literals at ~279 and ~928)
- Modify: every other `SlotDeclaration { .. }` literal: `crates/ns-runtime/src/resolver/scope.rs:125`, `crates/ns-runtime/tests/mixin.rs:20`, `crates/ns-runtime/tests/vm_initializer.rs:178`
- Test: `crates/ns-syntax/src/parser/declaration.rs` (test module, beside `a_slot_section_is_sequential_or_simultaneous_everywhere` ~line 2392)

**Interfaces:**
- Produces: `ns_syntax::ast::SlotDeclaration.simultaneous: bool` — `true` exactly for a declaration inside a `|| ... ||` clause (class header or code-body temporaries); `false` for `| ... |`, for a lazy slot, and for everything built outside the parser.

- [ ] **Step 1: Write the failing test**

Add to the parser's test module in `declaration.rs`:

```rust
/// §6.3.2: a declaration remembers which clause it stood in, since a
/// simultaneous clause initializes differently. A lone `||` is still the
/// empty sequential section, and a lazy slot belongs to no clause.
#[test]
fn a_slot_declaration_records_whether_its_clause_is_simultaneous() {
    let source = indoc::indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new (|| first = second. second = 2. ||) (
            lazy late = 3.
            public sequential = ( | one = 1. | ^one )
            public simultaneous = ( || one = two. two = 2. || ^one )
            public empty = ( || ^nil )
        ) : ()
    "};
    let module = crate::parse(source).expect("parse");
    let body = &module.class.instance_side;
    let header: Vec<bool> = body
        .slots
        .iter()
        .filter(|slot| !slot.lazy)
        .map(|slot| slot.simultaneous)
        .collect();
    assert_eq!(header, vec![true, true]);
    assert!(body.slots.iter().filter(|slot| slot.lazy).all(|slot| !slot.simultaneous));
    let temporaries = |selector: &str| -> Vec<bool> {
        body.methods
            .iter()
            .find(|method| crate::ast::selector_of(&method.pattern) == selector)
            .map(|method| method.body.temporaries.iter().map(|temporary| temporary.simultaneous).collect())
            .unwrap_or_default()
    };
    assert_eq!(temporaries("sequential"), vec![false]);
    assert_eq!(temporaries("simultaneous"), vec![true, true]);
    assert_eq!(temporaries("empty"), Vec::<bool>::new());
}
```

If `crate::ast::selector_of` does not exist, use whatever the neighbouring tests in this module use to find a method by selector (search the module for `pattern`); do not add a helper to the AST for the test's sake.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-syntax a_slot_declaration_records_whether_its_clause_is_simultaneous`
Expected: compile error, `no field simultaneous on type SlotDeclaration`.

- [ ] **Step 3: Add the field and set it**

In `ast.rs`, after `lazy`:

```rust
    /// §6.3.2: declared in a simultaneous slot clause, `|| ... ||`, whose
    /// initializers run as futures resolved at the clause's end
    /// (`docs/design/object-model.md`, "Simultaneous slots"). Never set on a
    /// lazy slot, which stands in no clause.
    pub simultaneous: bool,
```

In `declaration.rs`, give `parse_slot_section` the clause's kind and set it on every declaration it pushes: its signature becomes `fn parse_slot_section(&mut self, close: TokenKind) -> Vec<SlotDeclaration>` unchanged, and the literal at ~279 gains `simultaneous: close == TokenKind::DoubleBar,`. The empty-sequential fallback in `parse_slot_declarations` returns `Vec::new()` and needs nothing. The lazy slot literal at ~928 gains `simultaneous: false,`. Every literal outside the parser (`resolver/scope.rs`, `tests/mixin.rs`, `tests/vm_initializer.rs`) gains `simultaneous: false,`.

- [ ] **Step 4: Run the test and the syntax crate**

Run: `cargo nextest run -p ns-syntax`
Expected: PASS, including `a_slot_section_is_sequential_or_simultaneous_everywhere`.

- [ ] **Step 5: Run the per-commit gates, the language server's and the plugin's**

Run the "Before every commit" block, then `cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`.
Expected: all pass; nothing else observes the new field yet. `frontend.md`'s "Deviation" section still holds (the flag is kept but not yet acted on) — change its sentence "The parser computes the `simultaneous` flag only to choose the closing delimiter and then discards it" to "The parser records the flag on each declaration (`SlotDeclaration.simultaneous`), and nothing acts on it yet".

- [ ] **Step 6: Commit**

```bash
git add crates/ns-syntax crates/ns-runtime/src/resolver/scope.rs crates/ns-runtime/tests/mixin.rs crates/ns-runtime/tests/vm_initializer.rs docs/design/frontend.md
git commit -m "feat: keep a slot declaration's simultaneous flag in the AST"
```

---

### Task 2: The future object and forcing it

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (a new private class after `Activation`, ~line 357)
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass` ~line 227 and `ALL` ~line 313; `Continuation` ~line 474)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name` ~line 87 and the tests that list kernel-private classes ~lines 997, 1482–1560)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag` ~line 190, `unnameable_class` ~line 845, the prebuilt filter ~line 1786) and `image/decode.rs` (`builtin_class_of_tag` ~line 820, `referenced_builtin_class` ~line 878), `image/opcodes.rs` ("Next free number: 30" ~line 263)
- Create: `crates/ns-runtime/src/interpreter/future.rs`
- Create: `crates/ns-runtime/src/builtins/future.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs`, `builtins/primitive.rs` (a `Primitive::Future` family and its mirror row)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod future;`, `Mutator.futures_made`, `resume`/`abandon` arms)
- Modify: `crates/ns-runtime/src/vm.rs` (`finish`)
- Modify: `crates/ns-runtime/src/trace.rs` (the `Continuation` impl)
- Test: `crates/ns-runtime/tests/future.rs` (new)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `BuiltinClass::Future`, builtin class tag 30.
  - `interpreter::future::FutureState` — `enum FutureState<'gc> { Unresolved(ObjectReference<'gc> /* closure */), Resolving, Resolved(Value<'gc>) }`.
  - `Mutator::as_future(&self, value: Value<'gc>) -> Option<ObjectReference<'gc>>` — the future `value` is, if it is one.
  - `Mutator::future_state(&self, future: ObjectReference<'gc>) -> FutureState<'gc>`.
  - `Mutator::new_future(&mut self, closure: ObjectReference<'gc>) -> ObjectReference<'gc>` — sets `futures_made`.
  - `Mutator::futures_made: bool` (`pub(crate)`) — set once the first future is made in this heap, never cleared.
  - `interpreter::future::AfterForcing` — `enum AfterForcing { Deliver, Reexecute }`.
  - `interpreter::future::force(mutator: &mut Mutator<'gc>, future: ObjectReference<'gc>, after: AfterForcing) -> Result<Step<'gc>, Signal<'gc>>` — `Step::Value(value)` for a resolved future; `Step::Block` activating its closure under `Continuation::Future` for an unresolved one (and marks it resolving); `Err` for one already resolving.
  - `Continuation::Future(Box<FutureForcing<'gc>>)` with `pub struct FutureForcing<'gc> { pub(crate) future: ObjectReference<'gc>, pub(crate) after: AfterForcing }`.
  - `interpreter::future::FutureOutcome<'gc>` — `enum FutureOutcome<'gc> { Deliver(Value<'gc>), Reexecute, Force(Step<'gc>) }`, and `Mutator::future_computed(&mut self, forcing: FutureForcing<'gc>, value: Value<'gc>) -> Result<FutureOutcome<'gc>, Signal<'gc>>`.
  - `vm.rs`'s `finish` handles `Continuation::Future`; `fn reexecute(mutator, floor) -> Flow` switches to the caller at its stored `program_counter` without writing anything.
  - Mirror row `futureValue:ifFail:` (`Primitive::Future(FuturePrimitive::Value)`), which answers `force(subject, AfterForcing::Deliver)` for a future and declines for anything else.

- [ ] **Step 1: Write the failing tests**

Create `crates/ns-runtime/tests/future.rs`. It builds futures from Rust through `Mutator::new_future`, since no source makes one until Task 4. Copy `expect_ok` from `src/interpreter/mirror.rs`'s tests. A helper compiles a small module whose methods answer blocks:

```rust
//! The pull future of §6.3.2's simultaneous slots, driven from the host
//! before any source makes one (`docs/design/object-model.md`,
//! "Simultaneous slots").

use ns_runtime::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => panic!("{what}: unexpected Newspeak error: {}", error.message),
    }
}

/// A module whose `Source` answers blocks to make futures of, and counts
/// how often the counting block ran.
fn a_source() -> (Interpreter, Root) {
    let source = indoc::indoc! {"
        Newspeak3
        'Test'
        class Holder = Object new (
            | public runs ::= 0. |
        ) (
            public three = ( ^[runs: runs + 1. 3] )
            public failing = ( ^[runs: runs + 1. nil zork] )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        std::rc::Rc::from(source),
        ns_runtime::SourceOrigin::File(std::path::PathBuf::from("holder.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    (interpreter, holder)
}

/// A future of the block `holder` answers for `selector`.
fn a_future(interpreter: &mut Interpreter, holder: &Root, selector: &str) -> Root {
    let block = expect_ok(interpreter.send(holder, Symbol::intern(selector), &[]), selector);
    interpreter.enter(|mutator| {
        let Value::Object(closure) = mutator.fetch(&block) else {
            panic!("{selector} answers a block");
        };
        let future = mutator.new_future(closure);
        mutator.stash(Value::Object(future))
    })
}

#[test]
fn a_message_to_a_future_is_answered_by_its_value_computed_once() {
    let (mut interpreter, holder) = a_source();
    let future = a_future(&mut interpreter, &holder, "three");
    let four = interpreter.stash(Value::SmallInteger(4));
    let sum = expect_ok(interpreter.send(&future, Symbol::intern("+"), &[four.clone()]), "future + 4");
    let again = expect_ok(interpreter.send(&future, Symbol::intern("+"), &[four]), "future + 4 again");
    let runs = expect_ok(interpreter.send(&holder, Symbol::intern("runs"), &[]), "runs");
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&sum), Value::SmallInteger(7)));
        assert!(matches!(mutator.fetch(&again), Value::SmallInteger(7)));
        assert!(matches!(mutator.fetch(&runs), Value::SmallInteger(1)), "the block ran once");
    });
}

/// §5.5: every public member of `Object` is protected in `Future`, so even
/// `class`, `==` and `printString` reach `doesNotUnderstand:` and are
/// answered by the value.
#[test]
fn a_future_answers_objects_own_messages_as_its_value() {
    let (mut interpreter, holder) = a_source();
    let future = a_future(&mut interpreter, &holder, "three");
    let class = expect_ok(interpreter.send(&future, Symbol::intern("class"), &[]), "future class");
    let printed = expect_ok(interpreter.send(&future, Symbol::intern("printString"), &[]), "printString");
    let three = interpreter.stash(Value::SmallInteger(3));
    let identical = expect_ok(interpreter.send(&future, Symbol::intern("=="), &[three]), "future == 3");
    interpreter.enter(|mutator| {
        let Value::Object(class) = mutator.fetch(&class) else { panic!("a class") };
        assert!(class.same(mutator.builtin_class(ns_runtime::BuiltinClass::Integer)));
        assert_eq!(mutator.describe(mutator.fetch(&printed)), "the String \"3\"");
        assert!(matches!(mutator.fetch(&identical), Value::Boolean(true)));
    });
}

/// Every public selector `Object` declares is overridden by `Future` as
/// protected, so a member added to `Object` later without its override
/// fails here rather than letting a future tell itself from its value.
#[test]
fn future_overrides_every_public_member_of_object_as_protected() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let public_of_object = mutator.public_selectors_of(ns_runtime::BuiltinClass::Object);
        for selector in public_of_object {
            assert_eq!(
                mutator.access_of(ns_runtime::BuiltinClass::Future, selector),
                Some(ns_syntax::ast::AccessModifier::Protected),
                "Future must override {selector} as protected"
            );
        }
    });
}

#[test]
fn forcing_a_future_that_is_being_computed_is_an_error() {
    let source = indoc::indoc! {"
        Newspeak3
        'Test'
        class Holder = Object new (
            | public held |
        ) (
            public looping = ( ^[held + 1] )
        ) : ()
    "};
    // `held` is set to the future of `[held + 1]`, which forces itself.
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        std::rc::Rc::from(source),
        ns_runtime::SourceOrigin::File(std::path::PathBuf::from("holder.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let future = a_future(&mut interpreter, &holder, "looping");
    let _ = expect_ok(interpreter.send(&holder, Symbol::intern("held:"), &[future.clone()]), "held:");
    let one = interpreter.stash(Value::SmallInteger(1));
    match interpreter.send(&future, Symbol::intern("+"), &[one]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("divergent evaluation"),
            "{}",
            error.message
        ),
        _ => panic!("a future forcing itself must be an error"),
    }
}

/// An abandoned forcing -- its computation raised and something above
/// caught it -- leaves the future unresolved, not stuck resolving, so the
/// next touch computes again and raises the same error.
#[test]
fn abandoning_a_forcing_resets_the_future() {
    let (mut interpreter, holder) = a_source();
    let future = a_future(&mut interpreter, &holder, "failing");
    let one = interpreter.stash(Value::SmallInteger(1));
    for attempt in 0..2 {
        match interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&one)) {
            Err(HostSignal::Error(error)) => assert!(
                error.message.contains("zork"),
                "attempt {attempt}: {}",
                error.message
            ),
            _ => panic!("attempt {attempt}: the computation's error must reach the host"),
        }
    }
    let runs = expect_ok(interpreter.send(&holder, Symbol::intern("runs"), &[]), "runs");
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&runs), Value::SmallInteger(2)), "computed again");
    });
}
```

`Mutator::public_selectors_of` and `Mutator::access_of` are test support: if the crate has no such reflection, add both as `#[doc(hidden)] pub fn` on `Mutator` in `interpreter/kernel.rs` (reading the builtin class's application's mixin: its `methods` table's selectors whose access is public, and one selector's access), next to the existing test support there; `Mutator::describe` is `Heap::describe` if `Mutator` has no forwarder. Whether `new_future`, `as_future` and `future_state` are `pub` or `#[doc(hidden)] pub` follows how the crate exposes `builtin_class` to integration tests.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --test future`
Expected: compile errors — no `new_future`, no `BuiltinClass::Future`.

- [ ] **Step 3: Declare the class in `Kernel.ns`**

After `Activation`:

```newspeak
    (* §6.3.2's pull future: what a simultaneous slot clause stores in a slot
       until the clause's end resolves it (docs/design/object-model.md,
       "Simultaneous slots"). Private, as `Activation` is, and no program
       names it: every public member of `Object` is overridden as protected,
       so §5.5's lookupPublic stops here for each of them, and every message
       a program sends reaches `doesNotUnderstand:`, which forwards it to the
       forced value -- §5.5's proxy, indistinguishable from its target.
       Forcing is the runtime's (`interpreter/future.rs`), which alone reads
       and writes the two slots, by name: the computation, a block until it
       is resolved and then the value, and the state, 0 unresolved, 1
       resolving, 2 resolved. The runtime makes every instance; `new` is
       never sent. *)
    private class Future = Object new (
        | private computation. private state ::= 0. |
    ) (
        protected class = ( ^super class )
        protected == other = ( ^super == other )
        protected = other = ( ^super = other )
        protected hash = ( ^super hash )
        protected printString = ( ^super printString )
        protected Object = ( ^super Object )
        protected Value = ( ^super Value )
        protected Boolean = ( ^super Boolean )
        protected True = ( ^super True )
        protected False = ( ^super False )
        protected Number = ( ^super Number )
        protected Integer = ( ^super Integer )
        protected Float = ( ^super Float )
        protected String = ( ^super String )
        protected Symbol = ( ^super Symbol )
        protected UndefinedObject = ( ^super UndefinedObject )
        protected Array = ( ^super Array )
        protected Block = ( ^super Block )
        protected isNil = ( ^super isNil )
        protected notNil = ( ^super notNil )
        protected ifNil: block = ( ^super ifNil: block )
        protected ifNotNil: block = ( ^super ifNotNil: block )
        protected ifNil: nilBlock ifNotNil: notNilBlock = ( ^super ifNil: nilBlock ifNotNil: notNilBlock )
        protected ifNotNil: notNilBlock ifNil: nilBlock = ( ^super ifNotNil: notNilBlock ifNil: nilBlock )
        protected halt = ( ^super halt )
        protected doesNotUnderstand: message = (
            ^message sentTo: (vmMirror futureValue: self ifFail: [^nil])
        )
    )
```

The list must equal `Object`'s public members at the time of writing — compare with `Kernel.ns` lines 33–92 and add any member this list misses; the test `future_overrides_every_public_member_of_object_as_protected` is what keeps it equal. If the kernel compiler's primitive-method recognizer or the class-accessor recognizer (`compiler::primitive_method`, `MixinMethod::class_accessor`) turns one of the `^super X` bodies into something `act` answers without an activation, that is fine: a protected method is never found by `lookupPublic` either way.

- [ ] **Step 4: Register the builtin class**

`types.rs`: add `Future` to `BuiltinClass` after `Halt`, with a doc comment ("§6.3.2's pull future, kernel-private as `Activation` is; its instances are made by the runtime (`interpreter/future.rs`)"), and to `BuiltinClass::ALL`. `interpreter/kernel.rs`'s `kernel_class_name`: `BuiltinClass::Future => "Future"`. Extend the kernel tests that enumerate kernel-private classes (~lines 997, 1482–1560: the ones naming `Activation` as unreachable) to name `Future` too — `Object` declares no accessor for it, and `platform kernel Future` is not understood. Image code: `builtin_class_tag` gains `BuiltinClass::Future => 30`; the decoder's tag table gains `30 => BuiltinClass::Future`; `referenced_builtin_class` refuses 30 as it refuses 23 (the header exception comes in Task 8); `unnameable_class` names `Future{suffix}`; the prebuilt filter excludes `BuiltinClass::Future` beside `Activation`; `opcodes.rs`'s builtin-tag comment says "Next free number: 31". Add `30` to `tests/opcode_tags.rs`'s builtin class tag pin.

- [ ] **Step 5: Write `interpreter/future.rs`**

```rust
//! §6.3.2's pull future (`docs/design/object-model.md`, "Simultaneous
//! slots"): an instance of the kernel-private `Future`, whose two slots this
//! module alone reads and writes. Forcing one runs its block as a trampoline
//! step under `Continuation::Future`, never by calling back into Newspeak;
//! what happens once the block returns -- deliver the value to the send that
//! forced it, or run the instruction that met the future again -- is
//! `AfterForcing`.

use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::Mutator;

/// The `state` slot's three values, as `Kernel.ns` documents them.
const UNRESOLVED: i64 = 0;
const RESOLVING: i64 = 1;
const RESOLVED: i64 = 2;

pub(crate) enum FutureState<'gc> {
    /// Not yet forced: the closure that computes it.
    Unresolved(ObjectReference<'gc>),
    /// Its closure is running; forcing it again is §6.3.2's ill-founded
    /// recursion.
    Resolving,
    Resolved(Value<'gc>),
}

/// What the activation that forced a future does once its block returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterForcing {
    /// Answer the value to the send that forced it: `doesNotUnderstand:`'s
    /// `futureValue:ifFail:`.
    Deliver,
    /// Discard it and run the instruction that met the future again, which
    /// now finds it resolved (`docs/design/execution.md`, "Running an
    /// instruction again after forcing a future").
    Reexecute,
}

pub(crate) enum FutureOutcome<'gc> {
    Deliver(Value<'gc>),
    Reexecute,
    /// The block answered another future, still unresolved: force it next,
    /// in the popped activation's place.
    Force(Step<'gc>),
}

impl<'gc> Mutator<'gc> {
    /// The future `value` is, if it is one. The class check is one compare:
    /// no program can subclass the private `Future`.
    pub fn as_future(&self, value: Value<'gc>) -> Option<ObjectReference<'gc>> {
        let Value::Object(object) = value else {
            return None;
        };
        self.heap
            .class_of_object(object)
            .same(self.builtin_class(BuiltinClass::Future))
            .then_some(object)
    }

    pub fn future_state(&self, future: ObjectReference<'gc>) -> FutureState<'gc> {
        let computation = self.future_field(future, "computation");
        match self.future_field(future, "state") {
            Value::SmallInteger(RESOLVED) => FutureState::Resolved(computation),
            Value::SmallInteger(RESOLVING) => FutureState::Resolving,
            _ => match computation {
                Value::Object(closure) => FutureState::Unresolved(closure),
                _ => unreachable!("internal: an unresolved future holds its closure"),
            },
        }
    }

    /// A new unresolved future of `closure`. Marks the heap as having made
    /// one, which is what lets every see-through check stay off the paths of
    /// a program that never does (`builtins::execute`).
    pub fn new_future(&mut self, closure: ObjectReference<'gc>) -> ObjectReference<'gc> {
        self.futures_made = true;
        let class = self.builtin_class(BuiltinClass::Future);
        // Field order is the declaration order in `Kernel.ns`; `future_field`
        // reads by name, and `a_futures_slots_are_where_this_module_reads_them`
        // pins that the two agree.
        self.heap.allocate_object(
            class,
            ObjectData::Instance(vec![Value::Object(closure), Value::SmallInteger(UNRESOLVED)]),
        )
    }

    fn future_field(&self, future: ObjectReference<'gc>, name: &str) -> Value<'gc> {
        self.kernel_slot(future, BuiltinClass::Future, name)
            .expect("internal: a future has both of its slots")
    }

    fn set_future_fields(&mut self, future: ObjectReference<'gc>, computation: Value<'gc>, state: i64) {
        self.heap.set_instance_field(future, 0, computation);
        self.heap.set_instance_field(future, 1, Value::SmallInteger(state));
    }

    /// `FutureForcing`'s activation returned `value`: record it, unless it
    /// is itself a future, which is followed first -- §6.3.2's pipelined
    /// promise, as Primordial Soup's `resolve` loops.
    pub(crate) fn future_computed(
        &mut self,
        forcing: FutureForcing<'gc>,
        value: Value<'gc>,
    ) -> Result<FutureOutcome<'gc>, Signal<'gc>> {
        if let Some(inner) = self.as_future(value) {
            match self.future_state(inner) {
                FutureState::Resolved(resolved) => return self.future_computed(forcing, resolved),
                FutureState::Resolving => return Err(divergent(Value::Object(inner))),
                FutureState::Unresolved(_) => {
                    // Force the inner one, then come back to this one.
                    self.set_future_fields(forcing.future, value, RESOLVING);
                    let step = force(self, inner, AfterForcing::Deliver)?;
                    return Ok(FutureOutcome::Force(chain(step, forcing)));
                }
            }
        }
        self.set_future_fields(forcing.future, value, RESOLVED);
        Ok(match forcing.after {
            AfterForcing::Deliver => FutureOutcome::Deliver(value),
            AfterForcing::Reexecute => FutureOutcome::Reexecute,
        })
    }

    /// The forcing activation was popped without returning -- its block
    /// raised and an unwind passed it: the future is unresolved again, so
    /// the next touch computes it again rather than meeting a stale
    /// "resolving" (`docs/design/object-model.md`, "A clause abandoned by an
    /// exception").
    pub(crate) fn future_abandoned(&mut self, forcing: FutureForcing<'gc>) {
        if let FutureState::Resolving = self.future_state(forcing.future) {
            let computation = self.future_field(forcing.future, "computation");
            self.set_future_fields(forcing.future, computation, UNRESOLVED);
        }
    }
}

/// Forces `future`: its value when it is resolved, its block's activation
/// under `Continuation::Future` when it is not, and §6.3.2's error when it
/// is already being computed.
pub(crate) fn force<'gc>(
    mutator: &mut Mutator<'gc>,
    future: ObjectReference<'gc>,
    after: AfterForcing,
) -> Result<Step<'gc>, Signal<'gc>> {
    match mutator.future_state(future) {
        FutureState::Resolved(value) => Ok(Step::Value(value)),
        FutureState::Resolving => Err(divergent(Value::Object(future))),
        FutureState::Unresolved(closure) => {
            mutator.set_future_fields(future, Value::Object(closure), RESOLVING);
            Ok(Step::Block {
                closure,
                arguments: StepArguments::Owned(Vec::new()),
                then: Continuation::Future(Box::new(FutureForcing { future, after })),
            })
        }
    }
}
```

`chain(step, forcing)` wraps the inner future's `Step::Block` so that, when it returns, the outer one is completed: the simplest correct form is to give the inner forcing's continuation a link back — make `FutureForcing` carry `pub(crate) then: Option<Box<FutureForcing<'gc>>>`, set it to the outer forcing in `chain`, and have `future_computed` recurse into `then` after recording its own value (its `after` is the outer one's). Write it that way, and trace `then` in `trace.rs` too.

The error:

```rust
/// §6.3.2 resolves only "well founded" recursion: a future forced while its
/// own block runs is not, and says so as an ordinary, catchable error.
fn divergent<'gc>(future: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: crate::Symbol::intern("value"),
        receiver: future,
        message: "divergent evaluation of a simultaneous slot's initializer".to_string(),
    })
}
```

Add `mod future;` to `interpreter/mod.rs`, the field `pub(crate) futures_made: bool` to `Mutator` (initialized `false` wherever `Mutator` is built; a `Mutator` field holds no handle, so its `Trace` impl lists it as a number, as it lists the others), and to `Continuation` in `types.rs`:

```rust
    /// A future's block returned (`interpreter::future`): record its value,
    /// then deliver it or run the instruction that forced it again. Boxed,
    /// as every continuation that carries state is.
    Future(Box<crate::interpreter::future::FutureForcing<'gc>>),
```

`FutureForcing` lives in `interpreter/future.rs` (`#[derive(Debug, Clone)] pub struct FutureForcing<'gc> { pub(crate) future: ObjectReference<'gc>, pub(crate) after: AfterForcing, pub(crate) then: Option<Box<FutureForcing<'gc>>> }`). `resume`'s match in `interpreter/mod.rs` gains `Continuation::Future(_) => unreachable!("internal: vm::finish handles a future's continuation itself")`; `abandon` gains `if let Continuation::Future(forcing) = then { self.future_abandoned(*forcing); }` beside the `NestedClass` arm (and `future_abandoned` walks `then` too). `trace.rs`'s `Continuation` impl visits `forcing.future` and each linked `then`'s. Every other exhaustive match on `Continuation` the compiler names gains its arm.

- [ ] **Step 6: `finish` and `reexecute` in `vm.rs`**

In `finish`, before the generic `then => match mutator.resume(then, value)` arm:

```rust
        Continuation::Future(forcing) => match mutator.future_computed(*forcing, value) {
            Ok(FutureOutcome::Deliver(value)) => value,
            Ok(FutureOutcome::Reexecute) => return reexecute(mutator, floor),
            Ok(FutureOutcome::Force(step)) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => Flow::Switch {
                        activation: next,
                        code: mutator.heap.activation_code(next),
                        program_counter: 0,
                        feedback_base: mutator.heap.activation_feedback_base(next),
                    },
                    Err(signal) => unwind(mutator, signal, floor),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor),
        },
```

and beside `resume_caller`:

```rust
/// The instruction the popped activation's caller is stopped at, run again
/// rather than resumed after: it met an unresolved future, which is now
/// resolved (`interpreter::future`, `AfterForcing::Reexecute`). It wrote
/// nothing before it forced the future, so running it again is running it
/// once. A host's send has no instruction to run again; a future it passes
/// in straight to a leaf ends the run.
fn reexecute<'gc>(mutator: &mut Mutator<'gc>, floor: usize) -> Flow {
    if mutator.heap.activation_depth() == floor {
        let error = crate::interpreter::future::host_passed_a_future();
        return Flow::Exit(mutator.to_host(Err(error)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    Flow::Switch {
        activation: caller,
        code: mutator.heap.activation_code(caller),
        program_counter: mutator.heap.activation_program_counter(caller),
        feedback_base: mutator.heap.activation_feedback_base(caller),
    }
}
```

`host_passed_a_future()` in `interpreter/future.rs` answers `Signal::Error(NewspeakError { selector: Symbol::intern("value"), receiver: Value::Nil, message: "a future reached a native through a host's send, which has no instruction to run again".to_string() })` — the error text lives in `interpreter/`, not in `vm.rs`. Nothing produces `Reexecute` until Task 3.

- [ ] **Step 7: The `futureValue:ifFail:` leaf**

`builtins/primitive.rs`: a family `Primitive::Future(FuturePrimitive)` with one variant `Value`, and a `MIRROR` row `futureValue:ifFail:` reported as `value`, leaf arity 1 — copy the shape of an existing one-argument row. `builtins/future.rs`:

```rust
//! The one leaf of `Kernel.ns`'s `Future`: its `doesNotUnderstand:` forces
//! it through `futureValue:ifFail:` before forwarding the message.

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::future::{force, AfterForcing};
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FuturePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        FuturePrimitive::Value => {
            let future = mutator.as_future(receiver)?;
            Some(force(mutator, future, AfterForcing::Deliver))
        }
    }
}
```

and `builtins/mod.rs`'s `execute` gains `Primitive::Future(primitive) => future::execute(mutator, primitive, receiver),`. Wherever a primitive family is listed exhaustively (the kernel test that every row is reachable, `dispatch.md`'s table if the tests read it), add it.

- [ ] **Step 8: Pin the slot layout**

In `interpreter/future.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use crate::{BuiltinClass, Interpreter};

    /// `set_future_fields` writes by position, `future_field` reads by name:
    /// the two agree only while `Kernel.ns` declares `computation` first.
    #[test]
    fn a_futures_slots_are_where_this_module_reads_them() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(application) = mutator.heap().class_application(mutator.builtin_class(BuiltinClass::Future)) else {
                panic!("Future is a class");
            };
            let names: Vec<&str> = application.mixin.slots.iter().map(|slot| slot.name.as_str()).collect();
            assert_eq!(names, ["computation", "state"]);
            assert_eq!(application.own_slot_base, 0, "Object declares no slot");
        });
    }
}
```

- [ ] **Step 9: Run the tests**

Run: `cargo nextest run -p ns-runtime --test future` then `cargo nextest run -p ns-runtime`
Expected: PASS. If `a_future_answers_objects_own_messages_as_its_value` fails on `==`, check that `Future>>==` is protected and that `lookupPublic` stops at it (`docs/design/dispatch.md`, "lookupPublic").

- [ ] **Step 10: Write the departure into the design**

In `object-model.md`, "Planned: simultaneous slots": replace the sentences about the class side (`computing:` and `valueOf:`) and the private forcing method with: forcing is `interpreter/future.rs`'s, a trampoline step under `Continuation::Future`; `Future`'s only Newspeak is its protected overrides and `doesNotUnderstand:`, which forces through the mirror row `futureValue:ifFail:`; the state is `computation` and `state`. Under "The lowering", say the futures are made by `MakeFuture` and resolved by `ResolveFuture` (Task 3). Mirror both in `core-library.md`'s "Planned: the pull future".

- [ ] **Step 11: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime docs/design
git commit -m "feat: add the kernel-private future and forcing it as a trampoline step"
```

---

### Task 3: The four instructions

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`Instruction`, its disassembly ~line 703, `outer_reach_of` needs no arm — none reads another frame)
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `image/encode.rs` (~line 660), `image/decode.rs`, `image/inspect.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (four arms; a `force_and_reexecute` helper)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (`resolve_for_instruction`)
- Modify: `crates/ns-runtime/src/interpreter/locals.rs` (read an own slot, beside `store_instance_field`)
- Test: `crates/ns-runtime/tests/opcode_tags.rs`, `crates/ns-runtime/tests/image.rs` (or wherever instruction round trips are tested — search for `OPCODE_RETURN_SLOT_IF_FILLED` in `tests/`)

**Interfaces:**
- Consumes: `Mutator::new_future`, `Mutator::as_future`, `Mutator::future_state`, `force`, `AfterForcing::Reexecute`, `vm::reexecute` (Task 2).
- Produces:
  - `Instruction::MakeFuture { destination: u16, closure: u16 }` — opcode 36: a new unresolved future of the closure in `closure`.
  - `Instruction::ResolveFuture { destination: u16, source: u16 }` — opcode 37: `source`'s value when it is no future; a resolved future's value; an unresolved one is forced and the instruction runs again.
  - `Instruction::LoadOwnSlot { destination: u16, own_index: u32 }` — opcode 38: `self`'s field at the activation's slot base plus `own_index`, as `InitializeSlot` addresses it.
  - `Instruction::InitializeSlotProvisionally { source: u16, own_index: u32 }` — opcode 39: `InitializeSlot` without §3.1.1's check, for a simultaneous clause's future only.
  - `interpreter::future::Resolution<'gc>` — `enum Resolution<'gc> { Value(Value<'gc>), Force(Step<'gc>) }`, and `resolve_for_instruction(mutator: &mut Mutator<'gc>, value: Value<'gc>) -> Result<Resolution<'gc>, Signal<'gc>>`.

- [ ] **Step 1: Write the failing tests**

In `tests/opcode_tags.rs`, beside the other instruction pins:

```rust
#[test]
fn the_simultaneous_slot_instructions_keep_their_numbers() {
    assert_eq!(opcodes::OPCODE_MAKE_FUTURE, 36);
    assert_eq!(opcodes::OPCODE_RESOLVE_FUTURE, 37);
    assert_eq!(opcodes::OPCODE_LOAD_OWN_SLOT, 38);
    assert_eq!(opcodes::OPCODE_INITIALIZE_SLOT_PROVISIONALLY, 39);
}
```

(match the module path the file already uses for `opcodes`). In the test file that round-trips instructions through the encoder and decoder (find it by searching `tests/` for `ReturnSlotIfFilled`), add a round trip of a `CompiledCode` holding one of each new instruction, built the way the neighbouring test builds its code, asserting the decoded instructions equal the encoded ones.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --test opcode_tags`
Expected: compile error, no `OPCODE_MAKE_FUTURE`.

- [ ] **Step 3: Declare, encode, decode, list**

`bytecode.rs`, after `ReturnSlotIfFilled`:

```rust
    /// §6.3.2: a new unresolved future of the closure in `closure`, one per
    /// initializer of a simultaneous slot clause
    /// (`docs/design/object-model.md`, "Simultaneous slots").
    MakeFuture {
        destination: u16,
        closure: u16,
    },
    /// The value `source` holds, seen through a future: itself when it is
    /// none, a resolved future's value, and an unresolved one forced first --
    /// the instruction then runs again (`interpreter::future`,
    /// `AfterForcing::Reexecute`). A simultaneous clause's end.
    ResolveFuture {
        destination: u16,
        source: u16,
    },
    /// `self`'s field at the activation's slot base plus `own_index`, as
    /// `InitializeSlot` addresses it: a simultaneous clause's end reads each
    /// slot back without a getter send, which a subclass could override.
    LoadOwnSlot {
        destination: u16,
        own_index: u32,
    },
    /// `InitializeSlot` without §3.1.1's check of a value's slot: the future
    /// a simultaneous clause stores first, which its end replaces with the
    /// value through an ordinary, checked `InitializeSlot`.
    InitializeSlotProvisionally {
        source: u16,
        own_index: u32,
    },
```

Disassembly: `MakeFuture r{destination} <- future of r{closure}`, `ResolveFuture r{destination} <- r{source}`, `LoadOwnSlot r{destination} <- own {own_index}`, `InitSlotProvisionally own {own_index} <- r{source}`. `opcodes.rs`: the four constants with doc comments in the file's style, and the "Next free number" comment for opcodes moved to 40. `encode.rs`, `decode.rs` and `inspect.rs`: one arm each, field order and widths as `InitializeSlot`'s (`u16` registers, `u32` index); the decoder validates each register against the code's register count and each `own_index` the way it validates `InitializeSlot`'s — copy those checks, do not write new ones. Every exhaustive match on `Instruction` the compiler names gains its arm (the register-liveness and copy analyses in `compiler/lower.rs` treat `MakeFuture` and `ResolveFuture` as writing `destination` and reading their source, `LoadOwnSlot` as writing `destination`, and `InitializeSlotProvisionally` as reading `source`, as for `InitializeSlot`).

- [ ] **Step 4: `resolve_for_instruction` in `interpreter/future.rs`**

```rust
pub(crate) enum Resolution<'gc> {
    Value(Value<'gc>),
    /// Push this step; once it returns the instruction runs again.
    Force(Step<'gc>),
}

/// What an instruction that must not hold a future -- `ResolveFuture`, and
/// `InitializeSlot` into a value (Task 6) -- makes of `value`.
pub(crate) fn resolve_for_instruction<'gc>(
    mutator: &mut Mutator<'gc>,
    value: Value<'gc>,
) -> Result<Resolution<'gc>, Signal<'gc>> {
    let Some(future) = mutator.as_future(value) else {
        return Ok(Resolution::Value(value));
    };
    match force(mutator, future, AfterForcing::Reexecute)? {
        Step::Value(value) => Ok(Resolution::Value(value)),
        step => Ok(Resolution::Force(step)),
    }
}
```

- [ ] **Step 5: The four arms in `vm.rs`**

```rust
        Instruction::MakeFuture { destination, closure } => {
            let Value::Object(closure) = read_local(&mutator.heap, activation, *closure) else {
                unreachable!("internal: MakeFuture's operand is the closure MakeClosure just made");
            };
            let future = mutator.new_future(closure);
            write_local(&mut mutator.heap, activation, *destination, Value::Object(future));
            Flow::Next
        }
        Instruction::ResolveFuture { destination, source } => {
            let value = read_local(&mut mutator.heap, activation, *source);
            match crate::interpreter::future::resolve_for_instruction(mutator, value) {
                Ok(Resolution::Value(value)) => {
                    write_local(&mut mutator.heap, activation, *destination, value);
                    Flow::Next
                }
                Ok(Resolution::Force(step)) => force_and_reexecute(mutator, activation, program_counter, step, floor),
                Err(signal) => unwind(mutator, signal, floor),
            }
        }
        Instruction::LoadOwnSlot { destination, own_index } => {
            let value = crate::interpreter::locals::load_own_slot(&mutator.heap, activation, *own_index);
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            let value = read_local(&mutator.heap, activation, *source);
            let receiver = mutator.heap.activation_receiver(activation);
            let base = mutator.heap.activation_slot_base(activation);
            store_instance_field(&mut mutator.heap, &receiver, base + *own_index as usize, value);
            Flow::Next
        }
```

`MakeFuture`'s operand: a crafted image could put anything in that register, and an image must never panic — so instead of `unreachable!`, decline safely: if the register holds no closure (`!mutator.heap().is_closure(object)`), raise an ordinary error from `interpreter/future.rs` (`fn not_a_closure(value) -> Signal`, text "MakeFuture needs a block") and `unwind`. Write it that way; the `unreachable!` above shows only the shape. `locals::load_own_slot` reads the field the way `filled_own_slot` does (`vm.rs`'s `ReturnSlotIfFilled` arm uses it; reuse its address computation, answer `Value::Nil` where it answers `None`). The helper:

```rust
/// An instruction met an unresolved future: store where it stands, push the
/// forcing, and switch to it; `Reexecute` brings the loop back to this same
/// instruction once the future is resolved.
fn force_and_reexecute<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    step: Step<'gc>,
    floor: usize,
) -> Flow {
    mutator.heap.set_activation_program_counter(activation, program_counter);
    match mutator.push_step(step, &[], Some(activation), false) {
        Ok(callee) => Flow::Switch {
            activation: callee,
            code: mutator.heap.activation_code(callee),
            program_counter: 0,
            feedback_base: mutator.heap.activation_feedback_base(callee),
        },
        Err(signal) => unwind(mutator, signal, floor),
    }
}
```

Check against `vm.rs`'s comment at ~line 60–69 ("An activation's `program_counter` is stored only by the instructions that…") and add `ResolveFuture` (and, in Task 6, `InitializeSlot`'s refusal path) to its list.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 7: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime
git commit -m "feat: add the instructions that make, resolve and provisionally store futures"
```

---

### Task 4: Lowering a simultaneous clause

**Files:**
- Create: `crates/ns-runtime/src/compiler/simultaneous.rs`
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (`mod simultaneous;`, `compile_mixin_initializer` ~line 155, `compile_method` ~line 59)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (`lower_slot_initializer` ~466, `lower_local_initializers` ~507, the writable table ~355, `lower_block_literal` ~643)
- Modify: `crates/ns-runtime/src/compiler/reference.rs` (`initializer` ~51, `local_initializers` ~220)
- Modify: `crates/ns-runtime/src/compiler/capture.rs` (`declare` ~168, `local_initializers` ~331, `analyze` ~90)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (instance-side slots ~195, `resolve_local_initializers` ~318)
- Goldens: `goldens/spec/spec_6_3_2_simultaneous_slots_resolve_mutual_recursion.{ns,toml}`, `goldens/spec/spec_5_1_8_a_code_body_declares_simultaneous_temporaries.ns`, and new `goldens/spec/spec_6_3_2_*`
- Test: `crates/ns-runtime/src/compiler/capture.rs` (test module)

**Interfaces:**
- Consumes: `SlotDeclaration.simultaneous` (Task 1); the four instructions (Task 3).
- Produces: `compiler::simultaneous::future_block(declaration: &ast::SlotDeclaration) -> Option<ast::Expression>` — for a simultaneous declaration with an initializer, the block literal `[initializer]` whose span is the declaration's own (no block literal can share it, since a declaration's span always covers its name and `=`); `None` otherwise.

- [ ] **Step 1: Write the failing goldens**

Unskip `goldens/spec/spec_6_3_2_simultaneous_slots_resolve_mutual_recursion.toml` (delete its `skipped` line; `stdout` stays `3`) and rewrite the `.ns` comment's second paragraph: the clause now makes futures and resolves them at its end, so `second` is `2` when `first` is forced.

In `goldens/spec/spec_5_1_8_a_code_body_declares_simultaneous_temporaries.ns`, replace "(Simultaneous clauses still initialize in order here, a recorded deviation, so these initializers do not refer to each other.)" with "(`first` and `second` do not refer to each other; `spec_6_3_2_*` pins what a simultaneous clause does when they do.)". Its `.toml` stays `7` and `2`.

New goldens, each a `.ns` with a comment citing `newspeak-spec.pdf` 6.3.2 and a `.toml`. Work out each expected output from §6.3.2 by hand before running anything:

`goldens/spec/spec_6_3_2_simultaneous_temporaries_resolve_mutual_recursion.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.2: a code body's temporaries are slot declarations
   (5.1.8), so a simultaneous clause of them initializes through futures as
   a class's does: `first` reads `second`, declared after it, and after the
   clause both hold their values. *)
class Spec632SimultaneousTemporaries usingPlatform: p = Object new (| t = p transcript. |) (
    public main: platform args: arguments = (
        || first = second + 1. second = 2. ||
        t show: first printString; newLine.
        t show: second printString; newLine
    )
) : ()
```
`stdout`: `3`, `2`.

`goldens/spec/spec_6_3_2_a_captured_sibling_is_read_after_the_clause.ns` (Review Focus 1):

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.2: `first`'s initializer is a block that reads
   `second`, declared after it. The block runs after the clause, when every
   slot of it holds its value, so it must see 2 -- not the nil the register
   held when the block was made. *)
class Spec632ACapturedSibling usingPlatform: p = Object new (| t = p transcript. |) (
    public main: platform args: arguments = (
        || first = [second + 1]. second = 2. ||
        t show: first value printString; newLine
    )
) : ()
```
`stdout`: `3`.

`goldens/spec/spec_6_3_2_a_mutable_slot_a_sibling_assigns.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.2: after the clause no slot of it holds a future
   (docs/design/object-model.md, "Simultaneous slots", rule 1). `second`'s
   initializer assigns `first` with a setter send, which answers its
   argument (5.4.3); the clause's end forces `first`'s future first -- 1 --
   and then `second`'s, which writes 5 into `first`. Each slot ends with
   what was last written to it. *)
class Spec632AMutableSlotASiblingAssigns usingPlatform: p = Object new (| t = p transcript. |) (
    class Probe = Object new (|| public first ::= 1. public second = first:: 5. ||) () : ()
    public main: platform args: arguments = (
        | probe = Probe new. |
        t show: probe first printString; newLine.
        t show: probe second printString; newLine
    )
) : ()
```
Work it out before recording: the end resolves in declaration order. `LoadOwnSlot first` holds `first`'s future → forced → `1` → `InitializeSlot first ← 1`. `LoadOwnSlot second` holds `second`'s future → forced → runs `first:: 5` (`first` ← 5; a setter send answers its argument, §5.4.3, where the setter method `first:` would answer the receiver, §6.3.2) and answers `5` → `second ← 5`. `stdout`: `5`, `5`.

`goldens/spec/spec_6_3_2_ill_founded_recursion_is_an_error.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.2 resolves "any well founded mutual recursion";
   `first` and `second` each need the other's value, which is not, and
   forcing a future while its own block runs is an ordinary error, caught
   here like any other. *)
class Spec632IllFoundedRecursion usingPlatform: p = Object new (
    | t = p transcript. private Error = p kernel Error. |
) (
    class Probe = Object new (|| public first = second + 1. public second = first + 1. ||) () : ()
    public main: platform args: arguments = (
        t show: ([Probe new. 'no error'] on: Error do: [:error | error messageText]); newLine
    )
) : ()
```
`stdout`: `divergent evaluation of a simultaneous slot's initializer`.

`goldens/spec/spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch.ns` (Review Focus 2):

```newspeak
Newspeak3
'Test'
(* An initializer that raises abandons its clause, and a future that had
   escaped stays unresolved (docs/design/object-model.md, "A clause
   abandoned by an exception"): the next message forces it, runs its
   initializer again, and raises the same error -- never "divergent
   evaluation". `escaped` receives `second`'s future before `first`'s
   initializer raises. *)
class Spec632AnAbandonedClause usingPlatform: p = Object new (
    | t = p transcript. private Error = p kernel Error. public escaped. |
) (
    class Probe keep: holder = Object new (
        || public first = {holder escaped: second. nil zork}. public second = 2 + 1. ||
    ) () : ()
    public main: platform args: arguments = (
        t show: ([Probe keep: self. 'no error'] on: Error do: [:error | error class name]); newLine.
        t show: (escaped + 1) printString; newLine
    )
) : ()
```
Work it out: the clause's end forces `first`'s future, whose array literal evaluates its elements in order: `holder escaped: second` stores `second`'s future (unresolved) in `escaped`, then `nil zork` raises `MessageNotUnderstood`, caught → prints `MessageNotUnderstood`. `second`'s future was never forced; `escaped + 1` forces it → `3` → `4`. `stdout`: `MessageNotUnderstood`, `4`. (This one pins that an escaped *unforced* future still computes; the reset of a stuck `resolving` is pinned by Task 2's `abandoning_a_forcing_resets_the_future`.)

Each `.toml` holds only `stdout` (status 0). Put them under `goldens/spec/`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_6_3_2`
Expected: FAIL — `second` is still nil (`nil + 1` is a `MessageNotUnderstood`), and the recursion golden prints `MessageNotUnderstood` or `no error`.

- [ ] **Step 3: The shared helper**

`compiler/simultaneous.rs`:

```rust
//! §6.3.2's simultaneous slot clause, the one desugaring both lowerings and
//! the capture analysis share (`docs/design/object-model.md`,
//! "Simultaneous slots", "The lowering"): each initializer becomes the body
//! of a block, made into a future and stored; the clause's end reads each
//! slot back and stores it resolved. The resolver scoped each initializer as
//! that block's body (`resolver::Resolver::resolve_simultaneous_initializer`),
//! so the bindings inside it already count the block as a scope.

use ns_syntax::ast;

/// `[initializer]` for a simultaneous declaration that has one. Its span is
/// the declaration's: the capture analysis and the lowering key a block by
/// its span (`capture::BodyKey::Block`), and no block literal can share it,
/// since a declaration's span also covers its name.
pub(crate) fn future_block(declaration: &ast::SlotDeclaration) -> Option<ast::Expression> {
    if !declaration.simultaneous {
        return None;
    }
    let initializer = declaration.initializer.as_ref()?;
    Some(ast::Expression::Block(ast::BlockExpression {
        parameters: Vec::new(),
        body: ast::CodeBody {
            temporaries: Vec::new(),
            statements: vec![ast::Statement::Expression(initializer.clone())],
            span: declaration.span,
        },
        span: declaration.span,
    }))
}
```

(If `Expression::Block` holds a `Box<BlockExpression>`, box it.)

- [ ] **Step 4: The resolver**

In `resolver/mod.rs`, add:

```rust
    /// §6.3.2: a simultaneous declaration's initializer runs inside a future's
    /// block (`compiler::simultaneous::future_block`), so it resolves in a
    /// scope of its own, one block deeper than its neighbours' -- a sibling
    /// or a factory parameter it reads is an outer local.
    fn resolve_simultaneous_initializer(
        &mut self,
        initializer: &Expression,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        scope.push(LocalScope::new(&[], &[]));
        self.resolve_expression(initializer, scope, chain);
        scope.pop();
    }
```

and call it instead of `resolve_expression` for a slot with `simultaneous` in the instance-side loop (~line 203) and in `resolve_local_initializers` (~line 325).

- [ ] **Step 5: The capture analysis**

In `capture.rs`:
- `declare`: a simultaneous temporary is mutable — `temporary.mutability != ast::SlotMutability::Immutable || temporary.simultaneous` — because the clause's end writes it again after its future's block may have captured a sibling.
- `local_initializers`: for a simultaneous temporary walk `future_block(temporary)` (the block, so the walker enters its scope as the lowering will) and record the write; after the loop, record a second write for every simultaneous temporary with an initializer, in declaration order — the clause's end — at a later position than every capture.
- `analyze`: `RootBody.slot_initializers` is built by `compile_mixin_initializer`; there, pass `future_block(declaration)` in place of the bare initializer for a simultaneous slot (the owned expressions must outlive the call: collect them into a `Vec<ast::Expression>` first and hand references).

Add a unit test beside `a_variable_written_after_capture_goes_into_the_vector`:

```rust
/// A simultaneous temporary is written twice -- its future, then its value
/// at the clause's end -- so a sibling's future block that reads it must
/// share it through the vector, not copy the nil its register held.
#[test]
fn a_simultaneous_temporary_a_sibling_reads_goes_into_the_vector() {
    let captures = analyze_method("simultaneous = ( || first = [second]. second = 2. || ^first )");
    assert!(captures.is_in_vector("second"), "second is read by first's future block and written at the clause's end");
}
```

Use whatever this module's tests use to ask where a named local lives (`analyze_method` exists at ~line 542; read how its neighbours assert placement and follow them exactly rather than inventing `is_in_vector`).

- [ ] **Step 6: The optimized lowering**

In `lower.rs`:
- The writable table (~355): `let writable = (temporary.mutability != ast::SlotMutability::Immutable || temporary.simultaneous) && layout.places[index as usize] == Place::Register;`
- `lower_local_initializers`: for a simultaneous temporary, lower `future_block(temporary)` with `lower_expression_for(&block, ValueUse::Read)` (a closure register), emit `MakeFuture { destination: scratch, closure }` into a fresh scratch register, and store it with the same `resolve_variable` + `emit_store` as today. After the loop, for each simultaneous temporary with an initializer, in order: read the local through `resolve_variable(0, index)` into a register (emit the load the `Access` needs — reuse the helper assignments and identifier reads use; search for the function that turns an `Access` into a value register), emit `ResolveFuture { destination: scratch, source }`, and `emit_store` it back. Record each emitted group as a statement in `statement_program_counter`, as the loop above does.
- `compile_mixin_initializer` (`compiler/mod.rs`): for a simultaneous slot call a new `lowerer.lower_simultaneous_slot_initializer(&block, own_index)` that lowers the block, emits `MakeFuture` then `InitializeSlotProvisionally { source, own_index }`; after the slot loop, for each simultaneous slot with an initializer, in declaration order, a new `lowerer.lower_simultaneous_slot_end(own_index)` emitting `LoadOwnSlot { destination, own_index }`, `ResolveFuture { destination, source: destination }` and `InitializeSlot { source: destination, own_index }`. Both record a statement each.

- [ ] **Step 7: The reference lowering**

In `reference.rs`, the same shape with its own helpers: `initializer` lowers `future_block(declaration)` through `self.value(&block)` for a simultaneous slot, then `MakeFuture` into `self.fresh()` and `InitializeSlotProvisionally`; after the loop the three-instruction end per simultaneous slot. `local_initializers`: `self.value(&block)`, `MakeFuture`, `StoreLocal`; then per simultaneous temporary `Move { destination: fresh, source: slot }`, `ResolveFuture { destination: fresh2, source: fresh }`, `StoreLocal { source: fresh2, slot }`. Every read is a fresh register, as the reference lowering always copies.

- [ ] **Step 8: Run the goldens and the suite**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_` then `cargo nextest run --workspace --exclude ns-lsp`
Expected: PASS under both lowerings (`golden::optimized::…` and `golden::reference::…`), and `golden_round_trips` for each new golden. A golden that answers differently under the two lowerings is a lowering defect: compare `ns-next disasm` of both before touching either.

- [ ] **Step 9: Docs**

`frontend.md`: remove the section "Deviation: simultaneous slot clauses initialize sequentially" and add one sentence to the slot-clause paragraph (~line 106) that a simultaneous clause's declarations carry `simultaneous`, which the resolver and compiler act on ([object-model.md](object-model.md), "Simultaneous slots"). `README.md`: remove the deviation row, "Four do worse" becomes "Three do worse" with the list without "the sequential simultaneous slot clauses", and the paragraph "One of these has a golden under `goldens/spec/`, recorded as `skipped`…" is rewritten for whatever `skipped` golden remains (search `goldens/` for `skipped =`; if none remains, say that no golden is skipped); the goldens section's `skipped` example (~line 154) takes the text of a remaining skipped golden, or an invented illustration marked as one. `compiler.md`: "Planned: the simultaneous slot clause's desugaring" becomes a present-tense section "The simultaneous slot clause" describing Steps 3–7.

- [ ] **Step 10: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime goldens docs/design
git commit -m "feat: initialize a simultaneous slot clause through futures resolved at its end"
```

---

### Task 5: A future is transparent to the leaves

**Files:**
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (`execute`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`Primitive::answers_false_to_a_stranger`)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (`see_through`)
- Goldens: new `goldens/spec/spec_6_3_2_*`
- Test: `crates/ns-runtime/tests/future.rs`

**Interfaces:**
- Consumes: `Mutator::futures_made`, `as_future`, `future_state`, `force`, `AfterForcing::Reexecute` (Task 2); `vm::reexecute` (Task 2).
- Produces: `Primitive::answers_false_to_a_stranger(self) -> bool`; `interpreter::future::SeeThrough<'gc>` — `enum SeeThrough<'gc> { NoFuture, Answer(Option<Result<Step<'gc>, Signal<'gc>>>) }`; `interpreter::future::see_through(mutator, primitive, receiver, selector, arguments) -> SeeThrough<'gc>`; `builtins::execute_leaf` (`pub(crate)`, today's `execute` body).

- [ ] **Step 1: Write the failing goldens**

`goldens/spec/spec_6_3_2_a_future_is_an_argument_like_its_value.ns`:

```newspeak
Newspeak3
'Test'
(* During a simultaneous clause a sibling is still its future, and a
   native that receives it as an argument must see its value
   (docs/design/object-model.md, "Where Rust sees a future"): `1 + second`
   reaches Integer's `+` leaf with the future, and `show:` the transcript's
   with it. *)
class Spec632AFutureIsAnArgument usingPlatform: p = Object new (| t = p transcript. |) (
    class Probe = Object new (|| public sum = 1 + second. public shown = t show: text. public text = 'shown'. public second = 2. ||) () : ()
    public main: platform args: arguments = (
        | probe = Probe new. |
        t newLine.
        t show: probe sum printString; newLine
    )
) : ()
```
Work it out: the clause's end forces `sum` → `1 + second` → the leaf fails on the future → seen through → `second` forced → `2` → `3`. Then `shown` → `t show: text` with `text`'s future → forced → `'shown'` printed (no newline). `stdout`: `shown` then (the `t newLine`) a line break, then `3`: the `.toml` is
```toml
stdout = '''
shown
3
'''
```

`goldens/spec/spec_6_3_2_a_future_is_equal_and_identical_to_its_value.ns` (Review Focus 3):

```newspeak
Newspeak3
'Test'
(* A future is indistinguishable from its value (5.5's proxy; docs/design/
   object-model.md, "Simultaneous slots", rule 3): compared during the
   clause, by equality or identity and from either side, it answers as the
   value would. `thing` is an ordinary object, so `==` is identity. *)
class Spec632AFutureIsEqualAndIdentical usingPlatform: p = Object new (| t = p transcript. |) (
    class Thing = Object new () () : ()
    class Probe = Object new (||
        public thing = Thing new.
        public checks = {3 = three. three = 3. 'a' = text. #a = symbol. thing == other. other == thing. thing = other}.
        public three = 3. public text = 'a'. public symbol = #a. public other = thing. ||) () : ()
    public main: platform args: arguments = (
        Probe new checks do: [:each | t show: each printString; newLine]
    )
) : ()
```
`stdout`: `true` seven times. Before recording, confirm each by the rule: every pair compares a value with a future of an equal (or, for `thing`/`other`, the identical) value.

`goldens/spec/spec_6_3_2_a_data_cycle_resolves.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.2's pipelined promise: `first` holds an array
   whose element is `second`'s future, and `second` one whose element is
   `first`'s -- a cycle through data, which only futures can build. After
   the clause the elements are the arrays themselves, to any program. *)
class Spec632ADataCycle usingPlatform: p = Object new (| t = p transcript. |) (
    class Probe = Object new (|| public first = {second}. public second = {first}. ||) () : ()
    public main: platform args: arguments = (
        | probe = Probe new. |
        t show: ((probe first at: 1) == probe second) printString; newLine.
        t show: (probe second == (probe first at: 1)) printString; newLine.
        t show: (((probe second at: 1) at: 1) == probe second) printString; newLine
    )
) : ()
```
`stdout`: `true`, `true`, `true`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_6_3_2_a_`
Expected: FAIL — `1 + second` raises "+ requires a numeric argument", `3 = three` answers `false`, `(probe first at: 1) == probe second` answers `false` (the leaf compares the future with the array).

- [ ] **Step 3: Classify every primitive**

In `builtins/primitive.rs`:

```rust
impl Primitive {
    /// Whether this leaf answers `false` for an operand of the wrong kind
    /// instead of declining: identity and the equality leaves. On that
    /// answer `builtins::execute` looks for a future among the operands,
    /// since a future answers here for its value (`docs/design/
    /// object-model.md`, "Where Rust sees a future"). Every variant is
    /// named, so a new leaf does not compile until someone decides.
    pub(crate) fn answers_false_to_a_stranger(self) -> bool {
        match self {
            Primitive::Object(ObjectPrimitive::Identical | ObjectPrimitive::Equal) => true,
            Primitive::Object(ObjectPrimitive::Class | ObjectPrimitive::Hash | ObjectPrimitive::PrintString) => false,
            Primitive::Value(ValuePrimitive::SameAs) => true,
            Primitive::Value(ValuePrimitive::Hash | ValuePrimitive::SlotCount | ValuePrimitive::SlotAt) => false,
            // ... every family, every variant, spelled out: Integer::Equal,
            // Float::Equal, String::Equal, Symbol::Equal,
            // Behavior::SharesApplication and Activation::InheritsMixinOf are
            // `true`; every other variant of every family is `false`.
        }
    }
}
```

Write out every arm — no `_`, no `..` — for `Behavior`, `Integer`, `Float`, `String`, `Symbol`, `Collection`, `Block`, `Platform`, `Activation`, `Message`, `Module`, `Future` and `Mirror` (a `Mirror` row is `false`: it reaches its own primitive's row through `execute_sent`, which calls `execute` again). For each variant marked `false`, read its leaf and confirm it either declines or raises on an operand of the wrong kind — then the failure path covers it — or never needs to see through one (it stores, forwards or ignores the operand: `ElementAtPut`'s element, `Block`'s `Value` arguments, `Message`'s `SendTo` receiver, `ReturnValue`). A leaf that answers anything else for a stranger (a number, a class) is a defect this task must fix, by marking it and listing it in the commit message.

- [ ] **Step 4: The invariant and the wrapper**

Rename today's `builtins::execute` body to `fn execute_leaf(...)` (same signature) and make `execute`:

```rust
/// Runs `primitive` on `receiver` (`execute_leaf`), seeing through a future
/// among its operands where the leaf could not: when it declined, raised, or
/// -- for a leaf that answers `false` to a stranger -- answered `false`
/// (`interpreter::future::see_through`). A program that never made a future
/// pays one flag test, and only on those paths.
///
/// This rests on an invariant every leaf keeps: **a leaf that declines or
/// raises has changed nothing** -- it checks its operands before it writes
/// anything -- so running it again, or running the instruction that sent it
/// again, is running it once.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let answer = execute_leaf(mutator, primitive, receiver, selector, arguments);
    if !mutator.futures_made {
        return answer;
    }
    let unanswered = match &answer {
        None | Some(Err(Signal::Error(_))) => true,
        Some(Ok(Step::Value(Value::Boolean(false)))) => primitive.answers_false_to_a_stranger(),
        _ => false,
    };
    if !unanswered {
        return answer;
    }
    match crate::interpreter::future::see_through(mutator, primitive, receiver, selector, arguments) {
        SeeThrough::NoFuture => answer,
        SeeThrough::Answer(seen) => seen,
    }
}
```

and in `interpreter/future.rs`:

```rust
pub(crate) enum SeeThrough<'gc> {
    /// No operand is a future: the leaf's own answer stands.
    NoFuture,
    /// What stands in its place: the leaf run again on the resolved
    /// operands -- declining again (`None`) or raising again is then the
    /// leaf's own verdict on the values -- or a forcing under `Reexecute`.
    Answer(Option<Result<Step<'gc>, Signal<'gc>>>),
}

/// A leaf could not answer, and a future among its operands may be why.
/// Resolved ones are replaced by their values and the leaf runs again; the
/// first unresolved one is forced under `Reexecute`, after which the
/// instruction that sent the leaf runs again.
pub(crate) fn see_through<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> SeeThrough<'gc> {
    let operands = std::iter::once(receiver).chain(arguments.iter().copied());
    let mut resolved = Vec::with_capacity(arguments.len() + 1);
    let mut any_future = false;
    for operand in operands {
        match mutator.as_future(operand) {
            None => resolved.push(operand),
            Some(future) => {
                any_future = true;
                match mutator.future_state(future) {
                    FutureState::Resolved(value) => resolved.push(value),
                    FutureState::Resolving => return SeeThrough::Answer(Some(Err(divergent(operand)))),
                    FutureState::Unresolved(_) => {
                        return SeeThrough::Answer(Some(force(mutator, future, AfterForcing::Reexecute)));
                    }
                }
            }
        }
    }
    if !any_future {
        return SeeThrough::NoFuture;
    }
    let (receiver, arguments) = resolved
        .split_first()
        .expect("internal: the receiver is an operand");
    SeeThrough::Answer(crate::builtins::execute_leaf(mutator, primitive, *receiver, selector, arguments))
}
```

(`future_computed` follows a future-of-a-future, so a resolved future's value is never a future, and running the leaf once more on the resolved operands cannot come back here with another.)

Before writing any of this, go through `builtins/` and confirm the invariant for every leaf that can decline or raise; where one writes before it checks, reorder it, and name each in the commit message. Record the invariant in `docs/design/dispatch.md`'s "Primitive methods" (or wherever the leaves' contract is described — search for "declines").

- [ ] **Step 5: The host limitation, pinned**

In `tests/future.rs`:

```rust
/// A host's send has no instruction to run again, so an unresolved future
/// it passes straight to a leaf ends the run with an error rather than
/// answering wrongly (`docs/design/object-model.md`, "Where Rust sees a
/// future").
#[test]
fn a_host_passing_an_unresolved_future_to_a_leaf_is_an_error() {
    let (mut interpreter, holder) = a_source();
    let future = a_future(&mut interpreter, &holder, "three");
    let one = interpreter.stash(Value::SmallInteger(1));
    match interpreter.send(&one, Symbol::intern("+"), &[future]) {
        Err(HostSignal::Error(error)) => assert!(error.message.contains("host"), "{}", error.message),
        _ => panic!("1 + an unresolved future from the host must be an error"),
    }
}
```

and write the departure into `object-model.md`'s "Where Rust sees a future": the host bullet now says the run ends with that error.

- [ ] **Step 6: Run everything**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_6_3_2` then `cargo nextest run --workspace --exclude ns-lsp`
Expected: PASS under both lowerings.

- [ ] **Step 7: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime goldens docs/design
git commit -m "feat: let the leaves see through a future among their operands"
```

---

### Task 6: A value never holds a future

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs` (`InitializeSlot` arm ~line 435)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (`value_for_a_value_slot`)
- Goldens: new `goldens/spec/spec_6_3_2_*`

**Interfaces:**
- Consumes: `resolve_for_instruction`, `Resolution`, `force_and_reexecute` (Task 3); `check_value_slot` (`interpreter/instantiate.rs:1453`).
- Produces: nothing new for later tasks.

- [ ] **Step 1: Write the failing goldens**

`goldens/spec/spec_6_3_2_a_value_class_initializes_simultaneously.ns`:

```newspeak
Newspeak3
'Test'
(* A value class may declare a simultaneous clause: its slots hold futures
   only during the clause, provisionally, and the clause's end stores each
   value through 3.1.1's check (docs/design/object-model.md, "The
   lowering"). A slot whose value is no value is still refused. *)
class Spec632AValueClassInitializesSimultaneously usingPlatform: p = Object <: Value (
) (
    class Pair = Value new (|| public first = second - 1. public second = 3. ||) () : ()
    class Holder = Value new (|| public list = {1}. ||) () : ()
    public main: platform args: arguments = (
        | t = platform transcript. Error = platform kernel Error. |
        t show: Pair new first printString; newLine.
        t show: (Pair new = Pair new) printString; newLine.
        t show: ([Holder new. 'accepted'] on: Error do: [:error | 'refused']); newLine
    )
) : ()
```

The root is `Object <: Value` because a value class must sit in a value module (`object-model.md`, "A value class needs a value module"); a value's slot may not hold the transcript, and a factory parameter is not in scope in a method (§6), so `main:args:` reads it from its own `platform` argument. `stdout`: `2`, `true`, `refused`.

`goldens/spec/spec_6_3_2_a_future_passed_into_a_value_is_forced.ns` (Review Focus 4):

```newspeak
Newspeak3
'Test'
(* A future about to be stored in a value's slot is forced first, and its
   value is stored and checked (docs/design/object-model.md, "Simultaneous
   slots", rule 4): `pair`'s initializer builds a Pair around `third`'s
   future, and the Pair holds 3. *)
class Spec632AFuturePassedIntoAValue usingPlatform: p = Object <: Value (
) (
    class Pair first: firstValue second: secondValue = Value new (
        | public first = firstValue. public second = secondValue. |
    ) () : ()
    class Probe = Object new (|| public pair = Pair first: third second: 1. public third = 3. ||) () : ()
    public main: platform args: arguments = (
        | t = platform transcript. pair = Probe new pair. |
        t show: pair first printString; newLine.
        t show: (pair = (Pair first: 3 second: 1)) printString; newLine
    )
) : ()
```
`stdout`: `3`, `true`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_6_3_2_a_value spec_6_3_2_a_future_passed`
Expected: the first passes lines 1–2 already (the provisional store) and the second fails with "cannot instantiate Pair: … a value object is deeply immutable …" naming the future.

- [ ] **Step 3: `InitializeSlot`'s refusal path**

In `interpreter/future.rs`:

```rust
/// `InitializeSlot` refused `value` for a value's slot: if it is a future,
/// the value it stands for is what belongs there -- stored and checked
/// again, or forced first. `None` when it is no future, and the refusal
/// stands.
pub(crate) fn value_for_a_value_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    value: Value<'gc>,
) -> Option<Result<Resolution<'gc>, Signal<'gc>>> {
    mutator.as_future(value)?;
    Some(resolve_for_instruction(mutator, value))
}
```

In `vm.rs`'s `InitializeSlot` arm, the `Err(signal)` branch becomes:

```rust
                Err(signal) => match crate::interpreter::future::value_for_a_value_slot(mutator, value) {
                    None => unwind(mutator, signal, floor),
                    Some(Ok(Resolution::Value(resolved))) => {
                        match check_value_slot(mutator, activation, receiver, *own_index as usize, resolved) {
                            Ok(()) => {
                                store_instance_field(&mut mutator.heap, &receiver, base + *own_index as usize, resolved);
                                Flow::Next
                            }
                            Err(signal) => unwind(mutator, signal, floor),
                        }
                    }
                    Some(Ok(Resolution::Force(step))) => {
                        force_and_reexecute(mutator, activation, program_counter, step, floor)
                    }
                    Some(Err(signal)) => unwind(mutator, signal, floor),
                },
```

The accepted path is untouched: a value that passes `check_value_slot` never reaches this. Add `InitializeSlot` to `vm.rs`'s comment listing the instructions that store their `program_counter`.

- [ ] **Step 4: Run everything**

Run: `cargo nextest run -p ns-cli --test run_goldens spec_6_3_2` then `cargo nextest run --workspace --exclude ns-lsp`
Expected: PASS under both lowerings.

- [ ] **Step 5: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime goldens
git commit -m "feat: force a future stored in a value's slot and store its value"
```

---

### Task 7: The collector replaces resolved futures

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap::replace_resolved_futures`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`collect` ~line 180)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (the call, with the class and the field positions)
- Test: `crates/ns-runtime/tests/future.rs`

**Interfaces:**
- Consumes: `Mutator::futures_made`, `BuiltinClass::Future`, the slot positions pinned in Task 2.
- Produces: `Heap::replace_resolved_futures(&mut self, future_class: ClassReference<'gc>, computation_index: usize, state_index: usize, resolved: i64) -> usize` — answers how many references it replaced.

- [ ] **Step 1: Write the failing test**

```rust
/// A collection replaces a resolved future with its value in every object
/// field (`docs/design/memory.md`), so an array a future escaped into holds
/// the value itself afterwards -- and the future, reachable from nothing
/// else, is collected.
#[test]
fn a_collection_replaces_a_resolved_future_in_an_array() {
    let (mut interpreter, holder) = a_source();
    let future = a_future(&mut interpreter, &holder, "three");
    let four = interpreter.stash(Value::SmallInteger(4));
    let _ = expect_ok(interpreter.send(&future, Symbol::intern("+"), &[four]), "force it");
    let array = interpreter.enter(|mutator| {
        let future = mutator.fetch(&future);
        let array = mutator.new_array(vec![future]);
        mutator.stash(Value::Object(array))
    });
    drop(future);
    interpreter.collect_garbage();
    interpreter.enter(|mutator| {
        let Value::Object(array) = mutator.fetch(&array) else { panic!("an array") };
        assert!(matches!(mutator.heap().array_element(array, 0), Some(Value::SmallInteger(3))));
    });
}
```

Use the names the crate has for making an array from Rust, reading an element and forcing a collection from a test (search `tests/collector.rs`); do not add new ones if these exist under other names.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-runtime --test future a_collection_replaces`
Expected: FAIL — the element is still the future.

- [ ] **Step 3: Replace before marking**

`heap.rs`:

```rust
    /// Rewrites every object field that holds a resolved future to hold its
    /// value instead (`docs/design/memory.md`, "A resolved future is traced
    /// as its value"): instance and array fields and a closure's copied
    /// values. Never a register or a host root -- a forcing activation that
    /// is returning has the future as `self` and reads its own slots -- so
    /// a future still held there survives this collection and is replaced
    /// by the next. Runs before marking, so a future nothing else reaches is
    /// garbage by the time the marker looks. Answers the count.
    pub(crate) fn replace_resolved_futures(
        &mut self,
        future_class: ClassReference<'gc>,
        computation_index: usize,
        state_index: usize,
        resolved: i64,
    ) -> usize {
        // Two passes: find each resolved future's value, then rewrite.
        // ... iterate the arena; for `ObjectData::Instance` of `future_class`
        // whose field `state_index` is `SmallInteger(resolved)`, record
        // (its index, field `computation_index`); then, for every live
        // record's `Instance`/`Array` fields and `Closure.copied`, replace a
        // `Value::Object(handle)` whose index is recorded.
    }
```

Write the body against the arena's real layout (read `Heap::compact` for how it iterates live records and matches `ObjectData`); every match names every variant, as `trace.rs`'s do. A resolved future's value is never a future (`future_computed` follows chains), so one pass of replacement suffices. In `world.rs`'s `collect`, before `mark_everything_reachable`:

```rust
        if self.mutator.futures_made {
            crate::interpreter::future::replace_resolved_futures(&mut self.mutator);
        }
```

with `replace_resolved_futures` in `interpreter/future.rs` passing the class, the two positions (0 and 1, pinned by `a_futures_slots_are_where_this_module_reads_them`) and `RESOLVED`. `move_everything` (the move stress mode) does not replace: moving is not collecting.

- [ ] **Step 4: Run it, and the suite under `COLLECT`**

Run: `cargo nextest run -p ns-runtime --test future` then, in Git Bash, `NS_NEXT_STRESS_COLLECT=1 cargo nextest run -p ns-cli --test run_goldens spec_6_3_2`
Expected: PASS — under `COLLECT` every safepoint collects, so every golden above runs with futures replaced mid-run.

- [ ] **Step 5: Docs**

`memory.md`: turn the "Planned: a resolved future is traced as its value" paragraph into present tense under "What the collector traces", naming `Heap::replace_resolved_futures` and why registers are exempt.

- [ ] **Step 6: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime docs/design/memory.md
git commit -m "feat: replace resolved futures with their values when collecting"
```

---

### Task 8: Futures in an image

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (the graph walk ~line 1040, `encode` of a value reference, `unnameable_class` ~line 845)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`decode_object_header` ~line 2089)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (a read-only `resolved_value(heap-side)` the encoder can call, or a `Heap` accessor)
- Goldens: new `goldens/spec/spec_6_3_2_a_data_cycle_survives_an_image`
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `BuiltinClass::Future`, tag 30 (Task 2); the state encoding (Task 2).
- Produces: nothing for later tasks.

- [ ] **Step 1: Write the failing golden and tests**

`goldens/spec/spec_6_3_2_a_data_cycle_survives_an_image.ns` (Review Focus 5):

```newspeak
Newspeak3
'Test'
(* The data cycle of spec_6_3_2_a_data_cycle_resolves, kept in module state
   so the golden round trip snapshots it: an image never holds a resolved
   future, only its value (docs/design/image-format.md), so after a load the
   element and the slot are still one object. *)
class Spec632ADataCycleSurvivesAnImage usingPlatform: p = Object new (
    | t = p transcript. probe = Probe new. |
) (
    class Probe = Object new (|| public first = {second}. public second = {first}. ||) () : ()
    public main: platform args: arguments = (
        t show: ((probe first at: 1) == probe second) printString; newLine.
        t show: (((probe second at: 1) at: 1) == probe second) printString; newLine
    )
) : ()
```
`stdout`: `true`, `true`. Check how `golden_round_trips.rs` snapshots (module state at which point) and confirm the cycle is in the state it writes; if the round trip builds a stateless binary instead, this golden still pins the source run, and the image test below pins the stateful case.

In `tests/image.rs`, following the neighbouring snapshot tests:
- `a_resolved_future_is_written_as_its_value` — a module whose slot holds an array containing a resolved future (make it through a simultaneous clause in the module's source); snapshot, load, and assert the element is the array/value itself (`==` true through a send) and that the loaded heap holds no instance of `Future` (count instances of `BuiltinClass::Future` with whatever heap statistics or walk the tests use).
- `an_unresolved_future_round_trips_and_computes_after_load` — an abandoned clause (as in `spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch`) leaves `escaped` holding an unresolved future; snapshot, load, send `escaped + 1` in the loaded world, expect `4`.
- `a_graph_naming_future_as_a_class_is_refused` — take a valid image and patch a `CLASS_BUILTIN` reference that is not an object header to tag 30 (follow how the existing test for tag 23 builds its bytes — search for `tag: 23` in `tests/`), and expect `DecodeError::InvalidTag { tag: 30, .. }`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --test image future` and `cargo nextest run -p ns-cli --test golden_round_trips spec_6_3_2`
Expected: FAIL — the encoder refuses `Future` (`unnameable_class`) or writes the future itself.

- [ ] **Step 3: The encoder**

Wherever the graph walk and the value encoder meet a `Value::Object`, first map a resolved future to its value (a function `fn seen_through(heap: &Heap<'gc>, future_class: ClassReference<'gc>, value: Value<'gc>) -> Value<'gc>` in `interpreter/future.rs`, reading by the positions pinned in Task 2): the walk never visits a resolved future, and every reference to one is written as a reference to its value. An unresolved or resolving future is an ordinary instance record whose header is `CLASS_BUILTIN` + 30, like an activation record's `CLASS_BUILTIN` + 23: its class is not walked (the `if matches!(record.data, ObjectData::Activation(_))` branch ~line 1045 gains the future case, keyed on the class being `BuiltinClass::Future`), and `unnameable_class` keeps refusing `Future` everywhere else. A *resolving* future can only be met mid-computation (a snapshot taken from inside its block); write it as unresolved — state 0 with its closure — which is what `future_abandoned` would make of it. The closure field is an ordinary closure the encoder already handles.

- [ ] **Step 4: The decoder**

`decode_object_header`'s builtin arm gains `BuiltinClass::Future => (context.mutator.builtin_class(BuiltinClass::Future), false)`, accepted only here, as `Activation` is; the record's body must be an instance body with exactly two fields — validate the count against the class's layout the way an instance record's field count is already validated. After decoding, set `mutator.futures_made = true` if any future record was decoded, so the see-through checks run in the loaded world.

- [ ] **Step 5: Run everything**

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: PASS, the golden round trips included.

- [ ] **Step 6: Docs**

`image-format.md`: "Planned: futures in a graph" becomes present tense under the section that lists builtin class tags and the header exception for `Activation`; add tag 30 to the tag list.

- [ ] **Step 7: Per-commit gates, then commit**

```bash
git add -A crates/ns-runtime goldens docs/design/image-format.md
git commit -m "feat: write a resolved future as its value and an unresolved one as a record"
```

---

### Task 9: The design in present tense, measurement, the end-of-batch gates and the progress tracker

**Files:**
- Modify: `docs/design/object-model.md`, `execution.md`, `core-library.md`, `dispatch.md`, `README.md`
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: The design documents**

`object-model.md`: "Planned: simultaneous slots" becomes "Simultaneous slots" in present tense, keeping every decision and its reasons, naming the code (`compiler/simultaneous.rs`, `interpreter/future.rs`, the four instructions) and the goldens; add `interpreter/future.rs` to the document's status block; add a bullet to "Decisions" (simultaneous slots through pull futures, decided with the user on 2026-09-28, with this plan's link). `execution.md`: "Planned: running an instruction again after forcing a future" becomes present tense (`Continuation::Future`, `AfterForcing`, `vm::reexecute`, `force_and_reexecute`, the host case). `core-library.md`: "Planned: the pull future" becomes present tense. `README.md`: the map row's "Planned next" for object-model loses "Simultaneous slots…"; its "Built" text gains "§6.3.2's simultaneous slots, through pull futures". Read each changed section once more against the code.

- [ ] **Step 2: Measure**

Follow `docs/design/execution.md`'s measurement protocol: three release builds of `HEAD` and three of the commit before Task 1, each with the pinned `RUSTFLAGS` (PowerShell: `$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"` before, `Remove-Item Env:RUSTFLAGS` after), `ns-next bench` over the four `goldens/bench/` fixtures, pooled. Expected: level (every comparison overlapping). If one is not, check with `dumpbin /disasm` that no `vm::run::closure` became a function of its own (`CLAUDE.md`), and that `builtins::execute`'s flag test sits after the leaf's answer. Record the numbers in `execution.md` as a short "What simultaneous slots cost" section in the style of "What the collections module cost".

- [ ] **Step 3: The end-of-batch gates**

Run, in Git Bash:
```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules     # "ran 6, failed 0, skipped 205"
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo nextest run --workspace --release   # the size tests: Object stays 56 bytes, Signal fits its niche
```
Expected: every run passes; the release run's size tests confirm `Continuation::Future` did not grow `Object` or `Signal`.

- [ ] **Step 4: The progress tracker**

`docs/PROGRESS.md`, "Where the next session starts": a new paragraph at the top, "**Simultaneous slots are done**" (date, this plan's link, `object-model.md`'s section), summarizing the rules, the mechanism, the goldens and the measurement; in "Noted for later", drop "sequential simultaneous slots" from the semantic audit's first candidates, leaving the synchronous `<-:` (set aside by the user until there are actors).

- [ ] **Step 5: Commit**

```bash
git add docs
git commit -m "docs: describe simultaneous slots as built and record what they cost"
```
