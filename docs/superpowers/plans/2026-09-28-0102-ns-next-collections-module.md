# The collections module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the Core Library's collections as a Newspeak module, `CollectionsFactory.ns`, whose `List` replaces the kernel's `OrderedCollection` behind `platform collections`, and remove `OrderedCollection`, its `add:` leaf and `Heap::array_push` from the runtime.

**Architecture:** Plan 2 of the two in `docs/design/core-library.md`, "Planned: the platform and collections as core-library modules", in the **factory pattern** the design names ("The platform"; confirmed by the user on 2026-09-28): `class CollectionsFactory usingKernel: theKernel = Object <: Value` is a root that holds the kernel instance and is never handed out; its nested classes are private; its one public member, `newCollections`, answers an instance of its nested `Collections`, the facet `platform collections` answers. `PlatformFactory`'s root builds the one `CollectionsFactory` instance in a lazy slot, through a new mirror leaf `moduleNamed:`, so every platform shares one `List` class. The design's open question -- how the `Collections` class is named in an image once it is no longer `PlatformFactory`'s -- is settled the way `Platform` names its own class: the facet declares `public Collections = ( ^outer CollectionsFactory Collections )` and `public List = ( ^outer CollectionsFactory List )`, class accessors the compiler already recognizes in every shipped module but the kernel (`MixinMethod::class_accessor`), so the platform paths `collections Collections` and `collections List` need no wider recognition, and `Platform>>Collections` goes. Because no program reaches the root, it needs no check of its own and can hold the kernel itself. `Array new:` comes first, because `List` and the handler search both need a sized array, and the removal comes last, once nothing uses `OrderedCollection`.

**Tech Stack:** Rust 1.98.0 (edition 2021), the `ns-runtime` and `ns-cli` crates, `indoc` as a dev-dependency, Newspeak source under `crates/ns-runtime/core-library/`, `cargo-nextest`.

**Spec:** `docs/design/core-library.md` — "Planned: the platform and collections as core-library modules", its subsections "The module table and the boot (plan 1)" (the `moduleNamed:` bullet), "Platform paths in the image (plan 1)", "The collections module (plan 2)", "What changes in the invariants" and "What is measured"; and "The platform" for the factory pattern. Read them before any task. Where this plan departs from the recorded design, it says so at the task and the departure is written into the design document by that task:

- **The open question is settled by the facet's own class accessor** (above): the platform path `Collections` becomes `collections Collections`, and `platform Collections` is gone -- the corpus never sends it, only one golden and one image test did, and both move to `platform collections Collections`.
- **`moduleNamed:` keeps nothing in Rust.** The design says the leaf answers a module class "built on its first request and kept". Here the leaf builds the class on every request and the caller keeps it: `PlatformFactory`'s root holds it in a lazy slot, which Newspeak evaluates once. That needs no new root in `Mutator` and no tracing (`ns-next-prefer-newspeak-semantics-over-runtime-rules`). *Confirmed by the user on 2026-09-28.*
- **`List>>at:put:` answers the value**, as `add:` answers the element and as Smalltalk's and Primordial Soup's `at:put:` do; the kernel's `Array>>at:put:` leaf answers the receiver and is left as it is. *Confirmed by the user on 2026-09-28.*
- **`List>>do:` visits the elements the list held when it started** (Primordial Soup's `1 to: size_ do:` reads `size_` once): an `add:` inside the block does not extend the walk, as `OrderedCollection>>do:`'s copied payload did not; an `at:put:` inside it is seen. A non-block argument is sent `value:`, as any Newspeak loop written over `at:` would, where the `Array>>do:` leaf treats it as a no-op. *Confirmed by the user on 2026-09-28.*

## Global Constraints

- All code, identifiers, comments, doc comments and commit messages are in English; conversation with the user is in German.
- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; `clippy.toml`'s `min_ident_chars`). Every code sample below follows it; `size_` in `CollectionsFactory.ns` is Primordial Soup's Newspeak slot name, which the design names, not a Rust identifier.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult` or a `HostResult`. Tests unwrap through a local `expect_ok` helper copied from a neighbouring test file (`crates/ns-runtime/tests/platform.rs` has one).
- A Newspeak module written inline in a test is an `indoc! {"…"}`, indented with spaces, closing `"}` on a line of its own.
- Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`; add a missing accessor there.
- `vm.rs` holds no dispatch logic; nothing in this plan touches `vm.rs`.
- The VM mirror is a capability: a program never obtains the mirror, its class or a `MirrorSend` binding. `CollectionsFactory.ns` is shipped source, resolved with `resolve_kernel`; it declares no `vmMirror` and needs none. No program reaches its root instance or root class, exactly as for `PlatformFactory` (`docs/design/core-library.md`, "No program reaches a shipped module's root"): no code whose receiver is that root may send it anything it does not understand, since a `MessageNotUnderstood` hands its receiver to whoever catches it.
- A leaf never calls back into Newspeak (`ns-next-builtins-must-stay-optimizable`): `moduleNamed:` builds a class, which is pure structure (`Mutator::load_root_class`), and runs no Newspeak.
- An image is untrusted input; a wire tag is assigned once and never renumbered or reused. Builtin class tag 10 (`OrderedCollection`) is **retired**, not reused; "Next free number: 30" stays. A mirror row is no wire tag — no artifact carries one (`SendMirror` has no wire tag, and `image/mixins.rs` decodes every method with `fast_path: None`) — so removing the `add:` row may shift the rows after it.
- Program outcomes are pinned: a golden's recorded `status`, `stdout` and `stderr` change only where this plan names the change and says why. A golden that changes anywhere else is a defect to investigate, never a file to re-record.
- `PlatformFactory.ns` and `CollectionsFactory.ns` are widened only on demand: every `List` selector exists because the corpus or a golden asks for it (`corpus/runnable.toml`'s `Ducts.ns`, `NewspeakASTs.ns`, `V5BytecodeSimulation.ns` bind `platform collections List`; the goldens send `new`, `add:`, `at:`, `at:put:`, `do:` and `size`).
- Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `chore:`, `refactor:`). Every commit message ends with a `Co-Authored-By:` trailer naming the model that actually wrote the code, a subagent's commits included, followed by the `Claude-Session:` line the session's attribution reminder gives.
- **Before every commit** (from `CLAUDE.md`, "Working in this repository"; no task touches `crates/ns-syntax`, `crates/ns-lsp` or `editors/jetbrains`, so their per-commit gates do not apply):
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  A commit that changes only Markdown runs none of them.
- The end-of-batch gates (stress modes, `ns-lsp`, `run-corpus` and its stress variants) run once, in Task 7. The JetBrains plugin's `verifyPlugin buildPlugin` does not run: nothing under `editors/jetbrains` changes.
- Design documents are living: each task updates the document for the aspect it changed, in place, describing the current state, not the history.
- Every command runs in the foreground; a subagent never hands back while its own background work runs.

## Review Focus

1. **The handler search near the depth limit.** The search now fills an `Array new: count` instead of an `OrderedCollection`. If `Array class>>new:` were not recognized as a mirror forwarder, each search would push one activation more than before, and the depth-limit goldens (`goldens/exceptions/an_error_just_below_the_depth_limit_is_caught`, `a_message_not_understood_just_below_the_depth_limit_is_caught`, `a_handler_overrunning_the_reserve_ends_the_run`, `goldens/limits/`) would change outcome. They must not change. Pinned in Task 1 (`array_new_is_a_mirror_forwarder`) and Task 2 (the second walk is inlined into `invokeNextHandler`, adding no method activation).
2. **An `Array new:` size that cannot be allocated.** `Array new: 9223372036854775807` must be an ordinary, catchable `Error`, never a Rust allocation abort; a negative size and a size that is no `Integer` likewise. Pinned in Task 1 (`goldens/kernel/array_new_refuses_what_is_no_size`).
3. **A `List` index that is no `Integer`, zero, or past the end** — `at:` and `at:put:` must signal an `Error` with `Array>>at:`'s text, catchable by `on: Error do:`, never read the backing array's spare capacity (`List new at: 1` must fail although `data` has four slots). Pinned in Task 3 (`goldens/collections/a_list_index_error_is_caught_as_an_error`).
4. **An image holding a `List`, a second `Collections` facet, or the factory itself.** A `List` with elements in module state must survive snapshot and build/load as an ordinary record of the class `collections List` names; a facet a program made with `platform collections class new` is an ordinary record of the class `collections Collections` names; the `CollectionsFactory` instance and class, which only a host can reach, must be refused by the encoder with a clear message, never written field by field and never a panic. Pinned in Task 4.
5. **A user subclass of `List`, and a module that binds `platform collections List` before `main:args:` runs.** A subclass must inherit the whole protocol and reach `size_` by a self send (protected); the three whitelisted corpus modules must still run and build (Task 7's `run-corpus`). Pinned in Task 3 (`goldens/collections/a_user_subclass_of_list_inherits_its_protocol`) and Task 7.

---

## File Structure

- `crates/ns-runtime/core-library/Kernel.ns` — `Array`'s class side gains `new:` (Task 1); `invokeNextHandler` fills an `Array` (Task 2); `OrderedCollection` and its `Object` accessor leave (Task 6).
- `crates/ns-runtime/src/builtins/collections.rs` — the `new:` leaf (Task 1); the `add:` leaf leaves (Task 6).
- `crates/ns-runtime/src/builtins/primitive.rs` — `CollectionPrimitive::Allocate` and its mirror row (Task 1); `Primitive::Module(ModulePrimitive)` and the `moduleNamed:ifFail:` row (Task 3); `CollectionPrimitive::Add` and its row leave (Task 6).
- `crates/ns-runtime/src/builtins/module.rs` (new, Task 3) — the `moduleNamed:` leaf.
- `crates/ns-runtime/src/builtins/mod.rs` — the new module and its `execute` arm (Task 3).
- `crates/ns-runtime/core-library/CollectionsFactory.ns` (new, Task 3) — the factory root, the `Collections` facet and `List`.
- `crates/ns-runtime/core-library/PlatformFactory.ns` — the lazy `collectionsModule`, `Platform>>collections` answering its facet, no `Collections` class (Task 3).
- `crates/ns-runtime/src/interpreter/core_library.rs` — the table entry, `shipped_module_named` (Task 3).
- `crates/ns-runtime/src/platform.rs` — module doc (Task 3).
- `crates/ns-runtime/src/types.rs`, `src/interpreter/kernel.rs`, `src/interpreter/instantiate.rs`, `src/interpreter/nested.rs`, `src/heap.rs`, `src/image/encode.rs`, `src/image/decode.rs`, `src/image/opcodes.rs` — `BuiltinClass::OrderedCollection`, tag 10 and `Heap::array_push` leave (Task 6).
- Tests: `crates/ns-runtime/tests/platform.rs`, `image.rs`, `builtins_collections_block.rs`, `factory_send.rs`, `builtin_object_inheritance.rs`, `heap.rs`, `opcode_tags.rs`, `vm.rs`, `reference_lowering.rs`, `collector.rs`, `inline_caches.rs`, `interpreter.rs`, `resolver.rs`, and the unit tests in `src/compiler/lower.rs`, `src/interpreter/kernel.rs`, `src/interpreter/core_library.rs`, `src/heap.rs`.
- Goldens: `goldens/kernel/` (Tasks 1, 3, 6), `goldens/collections/` (new category, Task 3), the goldens that write `OrderedCollection` (Task 5).
- Docs: `CLAUDE.md`, `docs/design/core-library.md`, `image-format.md`, `object-model.md`, `dispatch.md`, `memory.md`, `execution.md`, `README.md`, `docs/PROGRESS.md`.

---

### Task 1: `Array new:`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (the `Array` class, around line 486)
- Modify: `crates/ns-runtime/src/builtins/collections.rs`
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (unit test)
- Create: `goldens/kernel/array_new_answers_an_array_of_nils.ns` / `.toml`
- Create: `goldens/kernel/array_new_refuses_what_is_no_size.ns` / `.toml`
- Modify: `docs/design/core-library.md`

**Interfaces:**
- Produces: `Array new: size` in Newspeak — a fresh `Array` of `size` `nil`s, reachable from any module as `Array new: n` and from inside `Kernel.ns` as `outer Kernel Array new: n`; `CollectionPrimitive::Allocate`; the mirror row `arrayClass:new:ifFail:` (reported selector `new:`).

- [ ] **Step 1: Write the two goldens**

`goldens/kernel/array_new_answers_an_array_of_nils.ns`:

```newspeak
Newspeak3
'Kernel'
(* `Array new: size` answers an array of `size` nils, which `at:put:`
   fills (docs/design/core-library.md, "The collections module"). `Array
   new` is `Array new: 0`. *)
class ArrayNewAnswersAnArrayOfNils usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript. |
) (
    public main: platform args: arguments = (
        | sized |
        sized:: Array new: 3.
        transcript show: sized size printString; newLine.
        transcript show: (sized at: 1) printString; show: ' '; show: (sized at: 3) printString; newLine.
        sized at: 2 put: 42.
        transcript show: (sized at: 2) printString; newLine.
        transcript show: (Array new: 0) size printString; show: ' '; show: Array new size printString; newLine
    )
) : ()
```

`goldens/kernel/array_new_answers_an_array_of_nils.toml`:

```toml
stdout = '''
3
nil nil
42
0 0
'''
```

`goldens/kernel/array_new_refuses_what_is_no_size.ns`:

```newspeak
Newspeak3
'Kernel'
(* `Array new:` refuses a negative size, a size that is no Integer and a
   size it cannot allocate, each as an ordinary Error a handler catches --
   never an abort of the process. *)
class ArrayNewRefusesWhatIsNoSize usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private Error = thePlatform kernel Error. |
) (
    public main: platform args: arguments = (
        transcript show: ([Array new: -1] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([Array new: 'three'] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([Array new: 9223372036854775807] on: Error do: [:e | e messageText]); newLine
    )
) : ()
```

`goldens/kernel/array_new_refuses_what_is_no_size.toml`:

```toml
stdout = '''
new: requires a size of at least 0, got -1
new: requires an Integer size, got the String "three"
new: cannot allocate an Array of 9223372036854775807 elements
'''
```

The third size is `i64::MAX`: 16 bytes per `Value` overflows `isize::MAX`, so `Vec::try_reserve_exact` fails with a capacity overflow on every platform, before asking the operating system for anything — the outcome does not depend on the machine's memory.

- [ ] **Step 2: Write the fast-path unit test**

In `crates/ns-runtime/src/interpreter/kernel.rs`'s `tests` module, beside `the_platform_modules_members_have_the_fast_path_they_should`:

```rust
    /// `Array new:` is a class-side mirror forwarder, so a send of it runs
    /// the leaf with no activation. The handler search depends on that: it
    /// sends `Array new:` on every handled exception, and one activation
    /// more per search would move where the depth limit's goldens overflow.
    #[test]
    fn array_new_is_a_mirror_forwarder() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let array = mutator.builtin_class(BuiltinClass::Array);
            let metaclass = mutator
                .heap()
                .class_metaclass(array)
                .expect("Array has a metaclass");
            let application = mutator
                .heap()
                .class_application(metaclass)
                .expect("a metaclass has an application");
            let method = application
                .mixin
                .compiled_method(crate::Symbol::intern("new:"))
                .expect("Array's class side declares new:")
                .clone();
            let row = crate::builtins::primitive::mirror_primitive_named("arrayClass:new:ifFail:")
                .expect("the row exists");
            assert_eq!(
                method.fast_path,
                Some(crate::mixin::MethodFastPath::Mirror(row))
            );
        });
    }
```

If the metaclass's application's mixin is not the class side's mixin in this runtime, find the class side's methods the way `interpreter/metaclass.rs` does and assert on the same `fast_path`; the assertion itself does not change.

- [ ] **Step 3: Run them and see them fail**

```bash
cargo nextest run -p ns-cli --test run_goldens array_new
cargo nextest run -p ns-runtime array_new_is_a_mirror_forwarder
```

Expected: the goldens fail with a `doesNotUnderstand` of `new:` on `Array class`; the unit test fails with "Array's class side declares new:".

- [ ] **Step 4: Add the leaf**

In `crates/ns-runtime/src/builtins/primitive.rs`, add `Allocate` to `CollectionPrimitive` (after `Add`, which Task 6 removes):

```rust
pub enum CollectionPrimitive {
    Enumerate,
    Size,
    ElementAt,
    ElementAtPut,
    Add,
    /// `Array new: size`, on `Array`'s class side.
    Allocate,
}
```

and append the row at the **end** of `MIRROR`, after `message:sendTo:ifFail:`:

```rust
    mirror(
        "arrayClass:new:ifFail:",
        "new:",
        Primitive::Collection(CollectionPrimitive::Allocate),
    ),
```

In `crates/ns-runtime/src/builtins/collections.rs`, add the leaf and its arm in `execute` (`CollectionPrimitive::Allocate => leaf_step(allocate(mutator, receiver, selector, arguments)),`), and `use crate::types::BuiltinClass` if the glob import does not already bring it:

```rust
/// `Array new: size`: an array of `size` `nil`s. Its subject is the class
/// the forwarder runs in, which is only ever `Array` itself -- §3.2's
/// metaclasses are flat, so no subclass inherits `new:` -- and anything
/// else declines. A size that cannot be allocated is an error, found with
/// `try_reserve_exact` before anything is filled, never a Rust allocation
/// abort: the size is the program's, and an abort is not catchable.
fn allocate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(class) = receiver else {
        return None;
    };
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Array) {
        return None;
    }
    let refusal = |message: String| NewspeakError {
        selector,
        receiver,
        message,
    };
    let size = match arguments.first() {
        Some(Value::SmallInteger(size)) => *size,
        other => {
            return Some(Err(refusal(format!(
                "{selector} requires an Integer size, got {}",
                mutator.heap.describe_argument(other)
            ))))
        }
    };
    let Ok(length) = usize::try_from(size) else {
        return Some(Err(refusal(format!(
            "{selector} requires a size of at least 0, got {size}"
        ))));
    };
    let mut elements: Vec<Value<'gc>> = Vec::new();
    if elements.try_reserve_exact(length).is_err() {
        return Some(Err(refusal(format!(
            "{selector} cannot allocate an Array of {size} elements"
        ))));
    }
    elements.resize(length, Value::Nil);
    Some(Ok(Value::Object(mutator.new_array(elements))))
}
```

`Heap::class_builtin` answers `None` for an object that is no class object, so it needs no separate `is_class` check. `Mutator::new_array` tags the result with the builtin `Array` class, which is what `new:` answers.

Update the doc comment above `MIRROR` ("`Array` and `OrderedCollection` share rows 42-46") only as far as the new row needs; Task 6 rewrites it.

- [ ] **Step 5: Add the forwarder to `Kernel.ns`**

Give `Array` a class side:

```newspeak
    public class Array = Object new () (
        public do: action = ( ^vmMirror collection: self do: action ifFail: [^super do: action] )
        public at: index put: value = ( ^vmMirror collection: self at: index put: value ifFail: [^super at: index put: value] )
        public size = ( ^vmMirror collectionSize: self ifFail: [^super size] )
        public at: index = ( ^vmMirror collection: self at: index ifFail: [^super at: index] )
    ) : (
        (* A sized array of nils, which `List` grows into and the handler
           search fills. The leaf declines only a subject that is not `Array`
           itself, which a flat class side cannot be sent from. *)
        public new: size = ( ^vmMirror arrayClass: self new: size ifFail: [nil] )
    )
```

- [ ] **Step 6: Run the tests again**

```bash
cargo nextest run -p ns-cli --test run_goldens array_new
cargo nextest run -p ns-runtime array_new_is_a_mirror_forwarder
cargo nextest run -p ns-runtime mirror
```

Expected: all pass, the goldens under both lowerings. If a golden's text differs from the recording above, find out why before touching the `.toml`: the texts are the design, not a transcript of a run. `every_mirror_selector_is_unique_ends_in_if_fail_and_takes_two_more_arguments` checks the new row's arity.

- [ ] **Step 7: Update the design document**

In `docs/design/core-library.md`, "The collections module (plan 2)", mark the `Array new: size` bullet as built and say what it answers for a negative size, a non-integer and an unallocatable one; add the row to the table of leaves in "The core library as Newspeak source over a VM mirror" if that table lists rows.

- [ ] **Step 8: Run the per-commit gate and commit**

Run the per-commit gate from Global Constraints, then:

```bash
git add crates/ns-runtime/core-library/Kernel.ns crates/ns-runtime/src/builtins/collections.rs crates/ns-runtime/src/builtins/primitive.rs crates/ns-runtime/src/interpreter/kernel.rs goldens/kernel/array_new_* docs/design/core-library.md
git commit -m "feat: Array new: answers a sized array of nils"
```

---

### Task 2: The handler search keeps no `OrderedCollection`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Exception>>invokeNextHandler`, around line 234)
- Modify: `docs/design/core-library.md` ("Exceptions", and the plan-2 bullet)

**Interfaces:**
- Consumes: `outer Kernel Array new: count` (Task 1).
- Produces: `invokeHandlerAt:disabling:` receives an `Array` of the enabled `on:do:` activations the search passed, the found one last — as it received an `OrderedCollection` before.

This is a refactor with no change in outcome: the tests are the existing `goldens/exceptions/` and `goldens/limits/`, which must pass unchanged.

- [ ] **Step 1: Run the exception and limit goldens and note that they pass**

```bash
cargo nextest run -p ns-cli --test run_goldens exceptions limits
cargo nextest run -p ns-cli --test golden_round_trips exceptions limits
```

Expected: all pass. This is the baseline the rewrite must keep.

- [ ] **Step 2: Rewrite `invokeNextHandler`**

Replace the method and its comment with:

```newspeak
        (* The search walks the senders from where the last one stopped,
           counting every enabled `on:do:` it passes. When one handles this
           exception, it walks the same range again and collects the
           activations it counted, the handler last, into an array for
           `invokeHandlerAt:disabling:`. It cannot collect as it goes
           without a growable collection, and a walk after the disabling
           could not tell the handlers this search passed from ones
           disabled before it. The second walk is written out here rather
           than in a method of its own, so that a search pushes no
           activation it did not push before: the depth limit's reserve is
           counted in activations. *)
        private invokeNextHandler = (
            | activation count handlers index |
            count:: 0.
            activation:: vmMirror activationSender: handlerActivation ifFail: [nil].
            [activation isNil] whileFalse: [
                (vmMirror activationIsActiveHandler: activation ifFail: [false]) ifTrue: [
                    count:: count + 1.
                    (self isHandledAt: activation) ifTrue: [
                        handlers:: outer Kernel Array new: count.
                        index:: 0.
                        activation:: vmMirror activationSender: handlerActivation ifFail: [nil].
                        [index < count] whileTrue: [
                            (vmMirror activationIsActiveHandler: activation ifFail: [false]) ifTrue: [
                                index:: index + 1.
                                handlers at: index put: activation].
                            activation:: vmMirror activationSender: activation ifFail: [nil]].
                        ^self invokeHandlerAt: (handlers at: count) disabling: handlers]].
                activation:: vmMirror activationSender: activation ifFail: [nil]].
            reachedDefault:: true.
            handlerActivation:: nil.
            ^self defaultAction
        )
```

Every handler the first walk passed is enabled again when the second walk runs: `isHandledAt:` disables only the candidate it tests and enables it again in an `ensure:`, however `handles:` is left. So the second walk counts exactly the activations the first one counted, in the same order. `invokeHandlerAt:disabling:` is unchanged: it sends `do:` to its argument, which an `Array` answers.

- [ ] **Step 3: Run the goldens again**

```bash
cargo nextest run -p ns-cli --test run_goldens exceptions limits
cargo nextest run -p ns-cli --test golden_round_trips exceptions limits
```

Expected: all pass with their recorded outcomes. A depth-limit golden that changes means the search pushes an activation it did not push before — check `array_new_is_a_mirror_forwarder` (Task 1) and the NS_NEXT environment before anything else; never re-record it.

- [ ] **Step 4: Update the design document**

In `docs/design/core-library.md`, "Exceptions", describe the search's two walks where it describes `invokeNextHandler` collecting into a collection; mark the plan-2 bullet "The handler search keeps no `OrderedCollection`" as built.

- [ ] **Step 5: Run the per-commit gate and commit**

```bash
git add crates/ns-runtime/core-library/Kernel.ns docs/design/core-library.md
git commit -m "refactor: the handler search collects its handlers into an Array"
```

---

### Task 3: `CollectionsFactory.ns`, `moduleNamed:` and the platform's `collections`

**Files:**
- Create: `crates/ns-runtime/core-library/CollectionsFactory.ns`
- Create: `crates/ns-runtime/src/builtins/module.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs`, `crates/ns-runtime/src/builtins/primitive.rs`
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs`
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns`
- Modify: `crates/ns-runtime/src/platform.rs` (module doc only)
- Modify: `crates/ns-runtime/tests/platform.rs`, `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/src/interpreter/kernel.rs` (tests that name the removed `Platform>>Collections`)
- Modify: `goldens/kernel/the_platform_and_its_parts_are_values.ns`, `goldens/kernel/a_second_platform_writes_like_the_first.ns`
- Create: `goldens/collections/` with six goldens (below)
- Modify: `docs/design/core-library.md`, `docs/design/README.md` (the goldens table), `CLAUDE.md`

**Interfaces:**
- Consumes: `Array new:` (Task 1).
- Produces:
  - `core_library::shipped_module_named(name: &str) -> Option<Rc<ResolvedModule>>`.
  - `builtins::primitive::ModulePrimitive::Named`, `Primitive::Module(ModulePrimitive)`, the mirror row `moduleNamed:ifFail:` (reported `moduleNamed`), subject a `Symbol`, answering the module's root class or declining.
  - `platform collections` — a `Collections` facet of the one `CollectionsFactory` instance every platform shares; `platform collections List` — the factory's `List` class, protocol `new`, `add:` (answers the element), `at:`, `at:put:` (answers the value), `do:`, `size`; `platform collections Collections` — the facet's own class.
  - Platform paths `collections`, `collections List` and `collections Collections`; the path `Collections` no longer exists.

- [ ] **Step 1: Write the `List` goldens**

Create `goldens/collections/`, and in it:

`a_list_grows_past_its_first_array.ns`:

```newspeak
Newspeak3
'Collections'
(* A List starts over an Array of four and moves into one twice as large
   whenever it is full, so ten elements take two moves; `add:` answers the
   element and `at:put:` the value (docs/design/core-library.md, "The
   collections module"). *)
class AListGrowsPastItsFirstArray usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private List = thePlatform collections List. |
) (
    public main: platform args: arguments = (
        | list count total |
        list:: List new.
        transcript show: list size printString; newLine.
        count:: 0.
        [count < 10] whileTrue: [count:: count + 1. list add: count * count].
        transcript show: list size printString; newLine.
        transcript show: (list at: 1) printString; show: ' '; show: (list at: 5) printString; show: ' '; show: (list at: 10) printString; newLine.
        total:: 0.
        list do: [:each | total:: total + each].
        transcript show: total printString; newLine.
        transcript show: (list add: 'eleven'); newLine.
        list at: 11 put: 'replaced'.
        transcript show: (list at: 11); show: ' '; show: (list at: 11 put: 'again'); newLine
    )
) : ()
```

```toml
stdout = '''
0
10
1 25 100
385
eleven
replaced again
'''
```

`a_list_index_error_is_caught_as_an_error.ns`:

```newspeak
Newspeak3
'Collections'
(* An index outside 1..size is an Error with Array>>at:'s text, whatever
   capacity the List's array has spare: an empty List's array has four
   slots, and `at: 1` still fails. An index that is no Integer is out of
   range too. *)
class AListIndexErrorIsCaughtAsAnError usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private List = thePlatform collections List.
      private Error = thePlatform kernel Error. |
) (
    public main: platform args: arguments = (
        | list |
        list:: List new.
        list add: 1; add: 2.
        transcript show: ([list at: 3] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([list at: 0] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([list at: 3 put: 9] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([list at: 'one'] on: Error do: [:e | e messageText]); newLine.
        transcript show: ([list at: 3] on: Error do: [:e | e class name]); newLine.
        transcript show: ([List new at: 1] on: Error do: [:e | e messageText]); newLine
    )
) : ()
```

```toml
stdout = '''
index 3 out of range: valid indices are 1..2 (size 2)
index 0 out of range: valid indices are 1..2 (size 2)
index 3 out of range: valid indices are 1..2 (size 2)
index 'one' out of range: valid indices are 1..2 (size 2)
Error
index 1 out of range: valid indices are 1..0 (size 0)
'''
```

The fourth line assumes `'one' printString` answers `'one'` with its quotes (`String>>printString`'s leaf); confirm that with a one-line `ns-next run` before recording it, and if the leaf prints it differently, record what the leaf prints — the line's point is that the text comes from `List`, not from a comparison inside it.

`an_uncaught_list_index_error_ends_the_run.ns`:

```newspeak
Newspeak3
'Collections'
(* An unhandled List index error ends the run like any other Error. *)
class AnUncaughtListIndexErrorEndsTheRun usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private List = thePlatform collections List. |
) (
    public main: platform args: arguments = (
        transcript show: 'before'; newLine.
        List new at: 1.
        transcript show: 'after'; newLine
    )
) : ()
```

```toml
status = 1
stdout = '''
before
'''
stderr = '''
Uncaught Newspeak error: index 1 out of range: valid indices are 1..0 (size 0)
'''
```

`a_user_subclass_of_list_inherits_its_protocol.ns`:

```newspeak
Newspeak3
'Collections'
(* A class of the program may inherit from List, which the collections
   facet hands out although it is a private class of its module: it keeps
   the whole protocol and reaches the protected `size_` by a self send. *)
class AUserSubclassOfListInheritsItsProtocol usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private List = thePlatform collections List. |
) (
    class Stack = List new () (
        public push: element = ( ^self add: element )
        public top = ( ^self at: self size )
        public depth = ( ^size_ )
    )
    public main: platform args: arguments = (
        | stack |
        stack:: Stack new.
        stack push: 1; push: 2; push: 3.
        transcript show: stack top printString; show: ' '; show: stack size printString; show: ' '; show: stack depth printString; newLine.
        transcript show: ([stack at: 4] on: platform kernel Error do: [:e | e messageText]); newLine
    )
) : ()
```

```toml
stdout = '''
3 3 3
index 4 out of range: valid indices are 1..3 (size 3)
'''
```

`do_visits_the_elements_a_list_held_when_it_started.ns`:

```newspeak
Newspeak3
'Collections'
(* `do:` reads the size once, as Primordial Soup's `1 to: size_ do:` does:
   an element added inside the block is not visited, as it was not when
   OrderedCollection>>do: copied its elements first. *)
class DoVisitsTheElementsAListHeldWhenItStarted usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private List = thePlatform collections List. |
) (
    public main: platform args: arguments = (
        | list |
        list:: List new.
        list add: 1; add: 2.
        list do: [:each | list add: each * 10. transcript show: each printString; show: ' '].
        transcript newLine.
        transcript show: list size printString; newLine
    )
) : ()
```

```toml
stdout = '''
1 2 
4
'''
```

(`1 2 ` ends in a space.)

`every_platform_shares_one_collections_module.ns`:

```newspeak
Newspeak3
'Collections'
(* Every platform's collections facet comes from one CollectionsFactory,
   built on first use, so a List made through a second platform is an
   instance of the same class. The facet is a value: any two are equal,
   one a program makes from the facet's class included. *)
class EveryPlatformSharesOneCollectionsModule usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript. |
) (
    public main: platform args: arguments = (
        | second |
        second:: platform class new.
        transcript show: (second collections == platform collections) printString; newLine.
        transcript show: (second collections List == platform collections List) printString; newLine.
        transcript show: (platform collections Collections new = platform collections) printString; newLine.
        transcript show: (platform collections Collections == platform collections class) printString; newLine
    )
) : ()
```

```toml
stdout = '''
true
true
true
true
'''
```

- [ ] **Step 2: Run them and see them fail**

```bash
cargo nextest run -p ns-cli --test run_goldens collections/
```

Expected: failures. `platform collections List` still answers `OrderedCollection`, which has no `at:put:`, answers `do:` over a copy and raises the leaf's own errors; `platform collections Collections` is not understood. A golden that happens to pass against `OrderedCollection` already is fine — it pins behaviour `List` must keep.

- [ ] **Step 3: Write `CollectionsFactory.ns`**

`crates/ns-runtime/core-library/CollectionsFactory.ns`:

```newspeak
Newspeak3
'CollectionsFactory'
(* The Core Library's collections module (§8.3: the platform "provides
   accessors to various modules"; docs/design/core-library.md, "The
   collections module"), in the factory pattern `PlatformFactory.ns` is
   written in.

   This root holds what is privileged -- the kernel instance -- and nothing
   hands its instance to a program: `PlatformFactory`'s root builds it once,
   through the mirror's `moduleNamed:`, sends it `newCollections`, its one
   public member, and keeps it in a private lazy slot. What a program gets
   is the `Collections` facet `newCollections` answers, which hands out this
   module's classes by class accessors, each exactly `^outer
   CollectionsFactory <Name>` -- a lexical send, which reaches the root's
   private classes (§5.9), and which an image names by its path from the
   platform (`docs/design/image-format.md`, "The platform path section").
   Nothing a program can name answers this root's instance or its class, so
   no program can build one over something that is not the kernel. Keep it
   so: no code whose receiver is this root's instance may send it anything
   it does not understand, since a `MessageNotUnderstood` hands its
   receiver to whoever catches it.

   A value (§3.1.1), so that its facet is one and the platform, a value,
   may hold the facet in a slot. *)
class CollectionsFactory usingKernel: theKernel = Object <: Value (
    | private kernelModule = theKernel. |
) (
    (* `platform collections`: the module's namespace. It names its own
       class as `Platform` does, so that the class has a path too. *)
    private class Collections = Value new () (
        public Collections = ( ^outer CollectionsFactory Collections )
        public List = ( ^outer CollectionsFactory List )
    )
    (* An ordered, growable sequence: Primordial Soup's `List`, with the
       protocol the corpus and the goldens ask for (the on-demand rule,
       docs/design/core-library.md). The elements live in `data`, an Array
       at least `size_` long, replaced by one twice as long when it is
       full; nothing grows in place. *)
    private class List = Object new (
        | protected size_ ::= 0.
          protected data ::= Array new: 4. |
    ) (
        public size = ( ^size_ )
        (* Answers the element, as Smalltalk's and Newspeak's `add:` do. *)
        public add: element = (
            size_ = data size ifTrue: [self grow].
            size_:: size_ + 1.
            data at: size_ put: element.
            ^element
        )
        public at: index = (
            (self includesIndex: index) ifFalse: [^self indexOutOfRange: index].
            ^data at: index
        )
        (* Answers the value, as `add:` answers the element. *)
        public at: index put: value = (
            (self includesIndex: index) ifFalse: [^self indexOutOfRange: index].
            data at: index put: value.
            ^value
        )
        (* Visits the elements the list holds when it starts: the size is
           read once, so an `add:` inside the block does not extend the
           walk. *)
        public do: action = (
            | end index |
            end:: size_.
            index:: 1.
            [index <= end] whileTrue: [
                action value: (data at: index).
                index:: index + 1]
        )
        (* An index that is no Integer is out of range too, rather than an
           error of the comparison inside this test. *)
        protected includesIndex: index = (
            ^(index class == Integer) and: [(1 <= index) and: [index <= size_]]
        )
        protected grow = (
            | larger index |
            larger:: Array new: data size * 2.
            index:: 1.
            [index <= size_] whileTrue: [
                larger at: index put: (data at: index).
                index:: index + 1].
            data:: larger
        )
        (* `Array>>at:`'s text, so a program sees one error from either. *)
        protected indexOutOfRange: index = (
            ^kernelModule Error signal: 'index ', index printString,
                ' out of range: valid indices are 1..', size_ printString,
                ' (size ', size_ printString, ')'
        )
    )
    (* The one public member: the facet `platform collections` answers. *)
    public newCollections = ( ^Collections new )
) : ()
```

`Array` and `Integer` inside `List` are implicit-receiver sends that no enclosing declaration answers, so they reach `Object`'s accessors (§5.7, §3.2); `kernelModule` is a lexical send to the root's private slot (§5.9).

- [ ] **Step 4: Add the table entry and `shipped_module_named`**

In `crates/ns-runtime/src/interpreter/core_library.rs`:

```rust
pub(crate) const COLLECTIONS_FACTORY_SOURCE: &str =
    include_str!("../../core-library/CollectionsFactory.ns");

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[
    ShippedModule {
        name: "PlatformFactory",
        source: PLATFORM_FACTORY_SOURCE,
    },
    ShippedModule {
        name: "CollectionsFactory",
        source: COLLECTIONS_FACTORY_SOURCE,
    },
];

/// The shipped module `name`, resolved, or `None` when the table lists no
/// such module: what the mirror's `moduleNamed:` answers from, since the
/// name there is a program value, even if only shipped code can send it.
pub(crate) fn shipped_module_named(name: &str) -> Option<Rc<ResolvedModule>> {
    let position = SHIPPED_MODULES
        .iter()
        .position(|shipped| shipped.name == name)?;
    Some(RESOLVED_SHIPPED_MODULES.with(|resolved| Rc::clone(&resolved[position])))
}
```

and let `resolved_shipped_module` call it, keeping its panic for a name the table does not list.

- [ ] **Step 5: Add the `moduleNamed:` leaf**

In `crates/ns-runtime/src/builtins/primitive.rs`:

```rust
    /// `moduleNamed:`: a shipped module's root class (`builtins::module`).
    Module(ModulePrimitive),
```

in `Primitive`, and

```rust
/// The module table's one leaf (`docs/design/core-library.md`, "The module
/// table and the boot").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulePrimitive {
    Named,
}
```

and append at the **end** of `MIRROR`:

```rust
    mirror(
        "moduleNamed:ifFail:",
        // Unary: the module's name is the row's subject.
        "moduleNamed",
        Primitive::Module(ModulePrimitive::Named),
    ),
```

`crates/ns-runtime/src/builtins/module.rs`:

```rust
//! `moduleNamed:`, the leaf through which shipped Newspeak reaches another
//! shipped module (`docs/design/core-library.md`, "The module table and the
//! boot"): `PlatformFactory.ns` builds the collections module with it.
//!
//! It answers the module's root class, built from the table's resolved
//! source by `Mutator::load_root_class`, which is pure structure and runs no
//! Newspeak. It keeps nothing: every request builds a new class object, and
//! the caller keeps the one it wants -- `PlatformFactory`'s root in a lazy
//! slot, which runs once -- so the runtime holds no module root of its own
//! and traces nothing more.

use crate::builtins::primitive::ModulePrimitive;
use crate::interpreter::core_library;
use crate::types::*;
use crate::Mutator;

/// Declines -- and the forwarder's failure block runs -- when the subject
/// is no symbol or names no shipped module.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ModulePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        ModulePrimitive::Named => {
            let Value::Object(name) = receiver else {
                return None;
            };
            let name = mutator.heap().symbol_value(name)?;
            let resolved = core_library::shipped_module_named(name.as_str())?;
            let class = mutator.load_root_class(&resolved);
            Some(Ok(Value::Object(class)))
        }
    }
}
```

In `crates/ns-runtime/src/builtins/mod.rs`, add `pub mod module;`, name it in the module doc's list, and add the arm:

```rust
        Primitive::Module(primitive) => {
            module::execute(mutator, primitive, receiver).map(wrap_error)
        }
```

Widen `core_library` or `load_root_class` to `pub(crate)` if either is not reachable from `builtins`. Fix every other exhaustive `match` on `Primitive` the compiler reports.

- [ ] **Step 6: Wire `PlatformFactory.ns`**

In the root's body — the second parenthesis, where lazy slots may be declared; a header slot cannot be lazy — add, before `Platform`:

```newspeak
    (* The collections module, built on first use and kept here, so that
       every platform's facet comes from one factory and hands out one
       `List` class. Its instance is as private as this one: only its
       `newCollections` facet leaves this module. *)
    lazy private collectionsModule = (vmMirror moduleNamed: #CollectionsFactory ifFail: [nil]) usingKernel: kernelModule.
```

In `Platform`, replace `lazy public collections = Collections new.` with `lazy public collections = collectionsModule newCollections.`, delete `public Collections = ( ^outer PlatformFactory Collections )`, and delete the nested `Collections` class with its comment. Update the root's comment where it lists what the platform hands out, and the comment above `Platform` (its class accessors are now `Platform`, `Transcript` and `KernelNamespace`; the collections module's classes are the facet's).

Update `install_platform`'s doc comment in `core_library.rs`: nothing past `Platform new` runs at boot; the first `platform collections` builds the collections module through `moduleNamed:`, and `image::load` may do so while it resolves a path. Update `crates/ns-runtime/src/platform.rs`'s module doc, which says `Collections>>List` answers its class without a leaf.

- [ ] **Step 7: Test the leaf**

In `core_library.rs`'s `#[cfg(test)] mod tests` (create it if there is none):

```rust
    fn module_named<'gc>(
        mutator: &mut Mutator<'gc>,
        subject: Value<'gc>,
    ) -> Option<Result<Value<'gc>, crate::types::NewspeakError<'gc>>> {
        crate::builtins::module::execute(
            mutator,
            crate::builtins::primitive::ModulePrimitive::Named,
            subject,
        )
    }

    /// The leaf answers a shipped module's root class, and declines a name
    /// the table does not list and a subject that is no symbol, so the
    /// forwarder's failure block runs.
    #[test]
    fn module_named_answers_a_shipped_root_class_and_declines_anything_else() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let name = mutator.new_symbol(crate::Symbol::intern("CollectionsFactory"));
            let Some(Ok(Value::Object(class))) = module_named(mutator, name) else {
                panic!("moduleNamed: #CollectionsFactory answers no class");
            };
            assert!(mutator.heap().is_class(class));
            assert!(mutator.heap().class_is_shipped(class));
            assert_eq!(mutator.heap().class_display_name(class), "CollectionsFactory");
            let unknown = mutator.new_symbol(crate::Symbol::intern("NoSuchModule"));
            assert!(module_named(mutator, unknown).is_none());
            let string = Value::Object(mutator.new_string("CollectionsFactory".to_string()));
            assert!(module_named(mutator, string).is_none());
        });
    }
```

`Mutator::new_symbol` (`interpreter/mod.rs`) makes the symbol value. `class_display_name`'s return type decides whether the comparison needs `.as_str()`.

- [ ] **Step 8: Update the tests that name the removed `Platform>>Collections`**

- `crates/ns-runtime/tests/platform.rs`:
  - Replace `platform_collections_answers_list_with_the_ordered_collection_class` with `platform_collections_answers_the_collections_modules_list`: `platform collections List` answers a class object with no builtin kind (`heap.class_builtin(list) == None`), compiled from shipped source (`heap.class_is_shipped(list)`), named `List`; `platform collections Collections` answers the same object as `platform collections class`; and a second platform (`platform class new`, sent through `interpreter.send`) answers the same `List` class object (`same`). Rewrite its doc comment: the three whitelisted corpus modules bind `platform collections List`, which is `CollectionsFactory.ns`'s `List`.
  - In `the_platform_factory_answers_only_new_platform` and `the_platforms_class_accessors_reach_the_private_classes`, drop `"Collections"` from the name lists.
  - Add, beside `the_platform_factory_answers_only_new_platform`, `the_collections_factory_answers_only_new_collections`: reach the factory instance as the host can — `platform collections`'s class, then `heap.class_enclosing` of it, stashed — and check that `Collections` and `List` sent to it are not understood while `newCollections` answers a facet whose `List` answers a class. A second line of defence behind the instance being unreachable, as for `PlatformFactory`.
- `crates/ns-runtime/src/interpreter/kernel.rs`:
  - In `the_platform_modules_members_have_the_fast_path_they_should`, the `collections` class is now reached as `platform collections` then `class` (two sends) instead of `platform Collections`; its `List` still has no fast path (a class accessor carries none). Rewrite the doc comment's sentence: `Collections>>List` is a class accessor of `CollectionsFactory.ns`, which only the image code reads.
  - In `the_platforms_class_accessors_are_recognized_without_a_fast_path`, drop `"Collections"` from the list, and add the facet's two, `Collections` and `List`, each `ClassAccessor { depth: 1, class_name }`, read off `platform collections class`'s application.
  - Keep `"Collections"` in the list of names `Object` declares no accessor for, and add `"CollectionsFactory"`.
- `crates/ns-runtime/tests/image.rs`:
  - `a_class_the_platform_hands_out_is_named_by_its_accessor`: replace `public collectionsClass = platform Collections.` with `public collectionsClass = platform collections Collections.` and add `public listClass = platform collections List.` after it; the expected paths become `["Transcript", "Platform", "collections Collections", "collections List", "KernelNamespace"]` (confirm the separator with `platform_paths::describe`, which `inspect` prints each path with); compare each stored class with what the loading platform answers — the two new ones through two sends, `collections` then the accessor.
  - The block in the path-refusal test that says `Collections>>List` "is no class accessor" (around line 10508): `collections List` is now a step, so replace it with the factory's own member, which is none of the facet's:
    ```rust
    // `newCollections` is the factory's member, not the facet's, so it is
    // no step: a path can never reach the collections factory.
    let collections = symbol_id(&report, "collections");
    let new_collections = symbol_id(&report, "newCollections");
    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![collections, new_collections]]),
    ));
    assert_eq!(
        message,
        "the image names platform path `collections newCollections`, whose member \
         `newCollections` is no public slot, nested class or class accessor of what the path \
         has reached"
    );
    ```
    If `newCollections` is not in that image's symbol table, snapshot a source that interns it (a selector named `newCollections`), as the test does for the other members.

- [ ] **Step 9: Update the two platform goldens (recorded outcomes unchanged)**

- `goldens/kernel/the_platform_and_its_parts_are_values.ns`: replace `t show: (c List == OrderedCollection) printString; newLine` with `t show: (c List == platform class new collections List) printString; newLine`. The `.toml` stays as it is: the line still prints `true`.
- `goldens/kernel/a_second_platform_writes_like_the_first.ns`: replace `platform transcript show: (platform Collections new = platform collections) printString; newLine.` with `platform transcript show: (platform collections Collections new = second collections) printString; newLine.`, and in its comment say that the collections facet hands out its module's classes as the platform hands out its own. The `.toml` stays as it is.

- [ ] **Step 10: Run the tests**

```bash
cargo nextest run -p ns-cli --test run_goldens collections/ kernel/
cargo nextest run -p ns-cli --test golden_round_trips collections/ kernel/
cargo nextest run -p ns-runtime platform image core_library kernel
```

Expected: all pass. The round trips check that a `List` class bound in module state is named by the path `collections List`.

- [ ] **Step 11: Update the documentation**

- `docs/design/README.md`: add a row for `collections/` to the goldens table: "the collections module behind `platform collections`: `List`'s protocol, its growth, its index errors, a user subclass, and one module shared by every platform".
- `docs/design/core-library.md`:
  - "The collections module (plan 2)": the `CollectionsFactory.ns` bullet becomes current state, and the "Open for plan 2" paragraph is resolved with its decision and date (2026-09-28, with the user): the facet names its own class with `^outer CollectionsFactory Collections`, as `Platform>>Platform` does, so the path is `collections Collections`, the recognition is not widened, and `Platform>>Collections` is gone. `List` is a private class of the factory, handed out by the facet's class accessor.
  - "The module table and the boot (plan 1)": the `moduleNamed:` bullets become current state — the leaf answers a root class built on every request, `PlatformFactory`'s lazy `collectionsModule` keeps it — and the table now has two entries.
  - "The platform": `Platform`'s members (`collections` is the collections factory's facet, not a `Collections new` of `PlatformFactory`'s).
  - "What changes in the invariants": `CollectionsFactory.ns` joins the modules resolved with `resolve_kernel` and whose root no program reaches.
  - `List`'s semantics as the plan's Spec section lists them: `add:` answers the element, `at:put:` the value, `do:` reads the size once and sends `value:` to any argument, index errors carry `Array>>at:`'s text.
- `CLAUDE.md`, "The VM mirror is a capability": "only the shipped core-library modules -- `Kernel.ns`, and `PlatformFactory.ns` from `interpreter/core_library.rs`'s table" becomes "`Kernel.ns`, and `PlatformFactory.ns` and `CollectionsFactory.ns` from `interpreter/core_library.rs`'s table"; the sentence on `PlatformFactory`'s factory pattern gains that `CollectionsFactory` is written the same way, its root held by `PlatformFactory`'s and its facet what `platform collections` answers; and the crate description at the top names `core-library/CollectionsFactory.ns` beside `PlatformFactory.ns` and the `moduleNamed:` leaf.

- [ ] **Step 12: Run the per-commit gate and commit**

```bash
git add crates/ns-runtime/core-library/ crates/ns-runtime/src/builtins/ crates/ns-runtime/src/interpreter/core_library.rs crates/ns-runtime/src/interpreter/kernel.rs crates/ns-runtime/src/platform.rs crates/ns-runtime/tests/platform.rs crates/ns-runtime/tests/image.rs goldens/collections goldens/kernel docs/design CLAUDE.md
git commit -m "feat: platform collections comes from a CollectionsFactory module with a Newspeak List"
```

---

### Task 4: The collections module in an image

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs`
- Modify: `docs/design/image-format.md`, `docs/design/core-library.md` ("Platform paths in the image")

**Interfaces:**
- Consumes: the paths `collections`, `collections List` and `collections Collections` (Task 3).
- Produces: tests only. No code change is expected; if one of these tests fails, the defect is in the encoder or the path discovery, and fixing it belongs to this task.

- [ ] **Step 1: Write a `List` round-trip test**

Beside `a_second_platform_is_a_record_of_its_path_named_class`:

```rust
/// A `List` is an ordinary instance of a shipped class: written as a record
/// of the class the path `collections List` names, with its `size_` and its
/// `data` array, and it answers as before after a load -- the class and its
/// code are the loading binary's.
#[test]
fn a_list_is_a_record_of_its_path_named_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public list = platform collections List new. |
            list add: 3.
            list add: 4.
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report.platform_paths.contains(&"collections List".to_string()),
        "the list's class is named by its path: {:?}",
        report.platform_paths
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let list = send_to(&mut loaded, &instance, "list");
    let size = send_to(&mut loaded, &list, "size");
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&size), Value::SmallInteger(2)));
    });
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_class = send_to(&mut loaded, &collections, "List");
    let stored_class = send_to(&mut loaded, &list, "class");
    assert!(
        same_object(&mut loaded, &stored_class, &fresh_class),
        "the loaded list is an instance of the loading platform's List"
    );
}
```

`send_to` takes a unary selector; for `at: 2`, add a send through `loaded.interpreter.send` with an integer root, as neighbouring tests do, and assert `4`.

- [ ] **Step 2: Write a second-facet round-trip test**

```rust
/// A facet a program makes itself from `platform collections class` is an
/// ordinary record of the class the path `collections Collections` names,
/// and after a load it is equal to the loading platform's facet: a value.
#[test]
fn a_second_collections_facet_is_a_record_of_its_path_named_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public facet = platform collections class new. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report.platform_paths.contains(&"collections Collections".to_string()),
        "the facet's class is named by its path: {:?}",
        report.platform_paths
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let facet = send_to(&mut loaded, &instance, "facet");
    let list = send_to(&mut loaded, &facet, "List");
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_list = send_to(&mut loaded, &collections, "List");
    assert!(
        same_object(&mut loaded, &list, &fresh_list),
        "the loaded facet hands out the loading platform's List"
    );
}
```

If `platform collections class new`'s class is reached before any accessor has filled the factory's nested-class slot, discovery skips it and the encoder refuses the image: then this test has found a real defect in the path discovery for a facet class reached by `class`, and fixing it (discovery reading the class of a path-named instance, or the facet's slot being filled whenever the facet exists) belongs to this task.

- [ ] **Step 3: Extend the refusal test with the factory**

In `a_shipped_object_no_platform_path_names_is_refused`, before the `interpreter.enter`, fetch through host sends `platform collections` and its `class`; inside, take `heap.class_enclosing` of that class — the `CollectionsFactory` instance — and its class, and add two rows to the loop:

```rust
            (
                collections_factory,
                "image encoding refuses a module instance of CollectionsFactory: it holds the \
                 core library's state, and no platform path names it",
            ),
            (
                collections_factory_class,
                "image encoding refuses the class object CollectionsFactory: it is compiled from \
                 shipped source, and no platform path names it",
            ),
```

and extend the doc comment: the collections factory is the second such module, reached here only because a host can walk a class's enclosing object, which no program can.

- [ ] **Step 4: Retarget the hash-table test that relied on a builtin `List`**

`an_object_reached_twice_is_listed_once_in_the_hash_table` rested on `collections List` answering the builtin `OrderedCollection`. No path answers a builtin now, but `encode_tag_named_hashes` must still list an object once however it is reached. Hand it the builtin `Array` class as the path-named object directly (the function takes the list of path-named objects; it does not discover them), replace every `OrderedCollection` in the test with `Array`, and rewrite the doc comment accordingly: "No platform path answers a builtin class today; the table must still list one once if a path ever does, so this hands the encoder `Array` as a path-named object …". Drop the `platform collections` / `List` sends, which the test no longer needs.

- [ ] **Step 5: Run the tests**

```bash
cargo nextest run -p ns-runtime --test image
```

Expected: all pass.

- [ ] **Step 6: Update the documentation**

- `docs/design/image-format.md`: the platform path section's examples (`collections`, `collections List`, `collections Collections`), and the sentence at line 895 that cites `^OrderedCollection` as no class accessor — replace it with what is true now (`collections List` is a class accessor step of the facet; the factory's `newCollections` is no step).
- `docs/design/core-library.md`, "Platform paths in the image": the paths the collections facet offers, and that the factory instance and class have none and are refused.

- [ ] **Step 7: Run the per-commit gate and commit**

```bash
git add crates/ns-runtime/tests/image.rs docs/design/image-format.md docs/design/core-library.md
git commit -m "test: a List and a second collections facet round-trip through an image"
```

---

### Task 5: Every program writes `List`, not `OrderedCollection`

**Files:**
- Modify: the eleven goldens below
- Modify: `crates/ns-runtime/tests/vm.rs`, `reference_lowering.rs`, `collector.rs`, `inline_caches.rs`, `interpreter.rs`, `resolver.rs`
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (unit tests)

**Interfaces:**
- Consumes: `platform collections List` (Task 3), `Array new:` (Task 1).
- Produces: nothing outside `Kernel.ns` and the tests Task 6 deletes names `OrderedCollection`.

No recorded outcome changes in this task. Every golden keeps its `.toml` byte for byte; one that changes is a defect in `List` to find, not a file to re-record.

- [ ] **Step 1: Move the goldens to `List`**

In each of these, add a private slot binding the list class to the module's header slots, using the module's own name for its factory parameter, and replace every `OrderedCollection new` with `List new`:

| Golden | Its platform parameter | Slot to add |
|---|---|---|
| `goldens/capture/a_blocks_own_registers_and_vector.ns` | `platform` | `private List = platform collections List.` |
| `goldens/capture/capture_in_inlined_loops.ns` | `platform` | same |
| `goldens/capture/closures_that_outlive_their_method.ns` | `platform` | same |
| `goldens/capture/nested_blocks_carry_variables.ns` | `platform` | same |
| `goldens/language/blocks_and_return.ns` | `platform` | same |
| `goldens/language/closure_variables.ns` | `platform` | same |
| `goldens/language/nonlocal_return_depth.ns` | `platform` | same |
| `goldens/language/operand_written_by_a_callee_keeps_its_evaluated_value.ns` | `platform` | same |
| `goldens/nesting/nested_override.ns` | `p` | `private List = p collections List.` |

A module whose header slot list is `()` gets one: `class X usingPlatform: platform = Object new (| private List = platform collections List. |) (`. In `nested_override.ns`, the nested slot becomes `| public Inner ::= List. |` — an implicit-receiver send the module's slot answers — and its comment says the slot starts at the collections module's `List`, which is no value class, rather than at `OrderedCollection`; its output stays `an Object` / `true`, since `Nested`'s instance inherits `Object>>printString`.

Read each golden's comment and adjust any sentence that names `OrderedCollection`. `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns` and `the_platform_and_its_parts_are_values.ns` are not in this task (Tasks 6 and 3).

- [ ] **Step 2: Run the moved goldens**

```bash
cargo nextest run -p ns-cli --test run_goldens capture/ language/ nesting/
cargo nextest run -p ns-cli --test golden_round_trips capture/ language/ nesting/
```

Expected: all pass with their recorded outcomes, under both lowerings and both round trips.

- [ ] **Step 3: Move the Rust tests**

- `tests/vm.rs`:
  - `cascades_and_builtin_instantiation_work` → rename `cascades_and_list_instantiation_work`; the module becomes `class X usingPlatform: platform = Object new ( | List = platform collections List. | ) ( public foo = ( | c | c:: List new. c add: 1; add: 2. ^c size ) ) : ()`, instantiated with the platform (`interpreter.instantiate(&class, &[interpreter.platform()])` through `expect_ok`) instead of `new_instance`.
  - `a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame`: the point is a block that a *leaf's* `do:` invokes, so use an array literal, whose `do:` is still the `Array` leaf: `public main = ( ^(self find: 2 in: {1. 2. 3}), (self find: 9 in: {1. 2. 3}) )`.
- `tests/reference_lowering.rs`, `PROGRAMS`:
  - `early` → `public early = ( | c | c:: {1. 2. 3}. c do: [:e | e = 2 ifTrue: [^e * 100]]. ^0 )`.
  - `cascade` keeps a cascade: `public cascade = ( | c n | c:: Array new: 3. c at: 1 put: 1; at: 2 put: 2; at: 3 put: 3. n:: 0. c do: [:e | n:: n + e]. ^n )`. The expected answers (200 and 6) do not change.
- `tests/collector.rs`, `kept_closures_keep_their_frames`: `made:: Array new: count.` and `made at: count - remaining + 1 put: [captured + 1].` in place of `made add: [captured + 1].`, so that `blockAt: index` answers the same closure it answered before.
- `tests/inline_caches.rs`, `a_failing_primitive_keeps_its_entry`: `| answers index |`, `answers:: Array new: 2. index:: 0.`, and `{Pair of: 1. Pair of: Loud new} do: [:each | index:: index + 1. answers at: index put: each = each].` The asserted call-site states for `=` stay as they are.
- `tests/interpreter.rs` (around line 86): the bare name becomes `Array` (`public bar = ( ^Array new )`), the assertion checks for an `Array` instance, and the doc comment says `self Array`.
- `tests/resolver.rs` (around line 81): the identifier becomes `Array` (`bar = ( | c | c:: Array new. ^c )`) in the source, the panics' texts and the final `assert_eq!`.
- `src/compiler/lower.rs` tests: in `a_bare_builtin_class_name_lowers_to_a_self_send`, `OrderedCollection` becomes `Array` (source, doc comment and the selector assertion). In `a_cascade_reuses_one_receiver_register_across_every_message`, `a_cascade_receiver_a_branch_assigns_is_copied_out_of_its_local` and `a_cascade_receiver_a_later_messages_argument_assigns_is_copied_out_of_its_local`, `OrderedCollection` becomes `List` — a name no declaration declares, so still a depth-0 self send, and these tests only lower — and the comments say `self List`; the family-size count (4) does not change.

- [ ] **Step 4: Check that nothing else writes it**

```bash
git grep -n "OrderedCollection" -- goldens crates/ns-runtime/tests crates/ns-runtime/src/compiler crates/ns-cli
```

Expected: only `crates/ns-runtime/tests/builtins_collections_block.rs`, `factory_send.rs`, `builtin_object_inheritance.rs`, `heap.rs`, `image.rs` (the builtin tag list), `opcode_tags.rs`, `platform.rs` (the accessor list) and `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.*` — each Task 6's.

- [ ] **Step 5: Run the per-commit gate and commit**

```bash
git add goldens crates/ns-runtime/tests crates/ns-runtime/src/compiler/lower.rs
git commit -m "test: programs build growable sequences with platform collections List"
```

---

### Task 6: Remove `OrderedCollection`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Modify: `crates/ns-runtime/src/types.rs`, `src/interpreter/kernel.rs`, `src/interpreter/instantiate.rs`, `src/interpreter/nested.rs`, `src/builtins/collections.rs`, `src/builtins/primitive.rs`, `src/heap.rs`, `src/image/encode.rs`, `src/image/decode.rs`, `src/image/opcodes.rs`
- Modify: `crates/ns-runtime/tests/builtins_collections_block.rs`, `factory_send.rs`, `builtin_object_inheritance.rs`, `heap.rs`, `image.rs`, `opcode_tags.rs`, `platform.rs`
- Modify: `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns` / `.toml`
- Modify: `docs/design/core-library.md`, `object-model.md`, `dispatch.md`, `memory.md`, `execution.md`, `image-format.md`

**Interfaces:**
- Consumes: nothing uses `OrderedCollection` (Task 5).
- Produces: `BuiltinClass` without `OrderedCollection` (`ALL` has 24 entries); `RETIRED_BUILTIN_CLASS_TAGS = [8, 10, 13, 18, 19, 29]`; no `CollectionPrimitive::Add`, no `collection:add:ifFail:` row, no `Heap::array_push`. No payload grows in place any more.

- [ ] **Step 1: Change the tests and the golden to what the removal makes true**

- `tests/opcode_tags.rs`, `builtin_class_tags_are_fixed`: delete the `OrderedCollection` line; the retired assertion becomes `assert_eq!(RETIRED_BUILTIN_CLASS_TAGS, [8, 10, 13, 18, 19, 29]);`, with a comment line: "10 was `OrderedCollection`, retired 2026-09-28 when `platform collections List`, Newspeak in the shipped `CollectionsFactory.ns`, replaced it."
- `tests/image.rs`, `every_builtin_class_tag_round_trips`: delete `BuiltinClass::OrderedCollection` from the list.
- `tests/platform.rs`, `platform_class_accessors_return_the_matching_builtin_class`: delete the `("OrderedCollection", …)` row.
- `tests/builtins_collections_block.rs`: delete `new_ordered_collection` and the three `ordered_collection_*` tests; the module doc speaks of `Array`'s leaves only.
- `tests/factory_send.rs`: delete `ordered_collection_new_via_ordinary_send_is_array_backed_and_supports_add_and_size`; in `a_class_object_receiving_a_non_factory_selector_falls_through_to_ordinary_dispatch` and `a_builtin_class_has_a_factory_entry_under_new`, use `BuiltinClass::Array`; in `array_new_via_ordinary_send_does_not_understand_add`'s doc comment, drop "(not `OrderedCollection`)".
- `tests/builtin_object_inheritance.rs`: `builtins_without_their_own_print_string_inherit_objects_default` sends `new` to the builtin `Array` class instead, its texts and doc comment naming `Array` (it declares no `printString`, so it answers `an Object`).
- `tests/heap.rs`: delete the `array_push` assertions in both tests; keep the rest.
- `src/heap.rs`'s `a_stale_handle_stored_in_the_heap_is_caught_by_the_next_move`: allocate `ObjectData::Array(vec![Value::Nil])` and plant the stale handle with `heap.array_set(array, 0, Value::Object(stale));`.
- `src/interpreter/kernel.rs` tests: delete the `OrderedCollection` entries from the builtin list around line 1014, the superclass table and the value-class table; add `"OrderedCollection"` and `"List"` to the list of names `Object` declares no accessor for.
- `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns`: delete `class MyOrderedCollection = OrderedCollection new () ()` and `report: MyOrderedCollection new.`, and `MyOrderedCollection` from the comment. **Its `.toml` loses one `true an Object` line** (six instead of seven): this is the one recorded outcome the removal changes, because there is no builtin `OrderedCollection` to subclass; `List` is no builtin and has a golden of its own (Task 3). The design recorded this change beforehand (`docs/design/core-library.md`, "The collections module (plan 2)", "Goldens").

- [ ] **Step 2: Run them and see the expected failures**

```bash
cargo nextest run -p ns-runtime --test opcode_tags
cargo nextest run -p ns-cli --test run_goldens a_subclass_of_a_builtin
```

Expected: `builtin_class_tags_are_fixed` fails on the retired list, since tag 10 still decodes. The golden already passes with its six lines — the program no longer names `OrderedCollection` — and the removal below must keep it passing.

- [ ] **Step 3: Remove the class and its leaf**

- `Kernel.ns`: delete `public OrderedCollection = ( ^outer Kernel OrderedCollection )` in `Object` and the `OrderedCollection` class.
- `src/types.rs`: delete `BuiltinClass::OrderedCollection`, its entry in `ALL` (now `[BuiltinClass; 24]`), and the doc comment's mention in `ClassData.builtin` (`decide_factory` instantiates `Array` with its own payload).
- `src/interpreter/kernel.rs`: delete its name mapping.
- `src/interpreter/instantiate.rs`, `decide_factory`: the arm is `Some(BuiltinClass::Array) => …`; rewrite the comment above it for `Array` alone.
- `src/interpreter/nested.rs`: the two doc comments that name `OrderedCollection` as a builtin with its own payload name `Array` alone.
- `src/builtins/collections.rs`: delete `add`, its arm and every mention of `OrderedCollection`, including the module's design note, which becomes: "`Array`'s leaves. An `Array` is fixed-size once made -- `new:` sizes it, a literal fills it -- and nothing grows a payload in place; the growable sequence is `List`, Newspeak in `CollectionsFactory.ns`, over arrays it replaces when full."
- `src/builtins/primitive.rs`: delete `CollectionPrimitive::Add` and the `collection:add:ifFail:` row; rewrite the doc comments on `CollectionPrimitive` and `MIRROR`: the rows are `Array`'s; and replace "Appended, so no row above moves ("A wire tag is assigned once", `CLAUDE.md`)" with the reason that holds: "A row's index never reaches an artifact -- `SendMirror` has no wire tag and a decoded method carries no fast path -- so a row may be removed; new rows are appended all the same, so a diff shows what was added."
- `src/heap.rs`: delete `array_push`.
- `src/image/encode.rs`: delete the `OrderedCollection => 10` arm. `src/image/decode.rs`: delete the `10 =>` arm. `src/image/opcodes.rs`: `RETIRED_BUILTIN_CLASS_TAGS: [u8; 6] = [8, 10, 13, 18, 19, 29]`, and its doc comment names 10 as `OrderedCollection`, retired 2026-09-28.
- Fix whatever else the compiler reports.

- [ ] **Step 4: Run the tests**

```bash
cargo nextest run -p ns-runtime
cargo nextest run -p ns-cli --test run_goldens
cargo nextest run -p ns-cli --test golden_round_trips
git grep -n "OrderedCollection\|array_push" -- crates goldens
```

Expected: all pass; the grep finds only the retired-tag comments.

- [ ] **Step 5: Update the documentation**

- `docs/design/core-library.md`: the plan-2 "Removed" and "Goldens" bullets become current state; "Planned: the platform and collections as core-library modules" gets its status line changed to "Plan 2 is built (2026-09-28)"; every other mention of `OrderedCollection` as something that exists is rewritten to `List` or removed — the historical accounts under "Decisions" stay as they are.
- `docs/design/object-model.md` (lines around 322, 354, 592), `dispatch.md` (around 1141, 1585): `Array` alone builds its own payload; the `nested_override` golden starts its slot at `List`.
- `docs/design/memory.md` (around 1343-1350 and 1498): `array_push` is gone, so no payload grows in place — the question step 7 has to settle first is answered: every `ObjectData::Array` is fixed-size after allocation. Say so where step 7's open question is stated.
- `docs/design/execution.md`: the five mentions — keep the ones that describe measurements taken when `OrderedCollection` existed, and rewrite any that describe current behaviour.
- `docs/design/image-format.md`: builtin class tag 10 in its list of retired tags.

- [ ] **Step 6: Run the per-commit gate and commit**

```bash
git add -A crates/ns-runtime goldens/kernel docs/design
git commit -m "feat: remove OrderedCollection, its add: leaf and Heap::array_push"
```

---

### Task 7: Measurement, the end-of-batch gates and the progress tracker

**Files:**
- Modify: `docs/design/execution.md` (new section "What the collections module cost")
- Modify: `docs/design/core-library.md` ("What is measured")
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: Measure**

Follow `docs/design/execution.md`'s measurement protocol ("What removing the re-entries moved") exactly: release builds with the pinned flags, in PowerShell

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

for the base commit `da103f5` (the commit before this plan's first) and for the branch head, three builds of each tree, pooled. Measure:

1. the start-up of a module that answers at once (the same source plan 1 measured, named in "What the platform module cost");
2. the start-up of a module that sends `platform collections List new` once and answers — the cost of building the collections module on first use, which the base commit does not pay;
3. the four `goldens/bench/` fixtures with `ns-next bench`.

None of the four fixtures sends a collection message in its timed loop, so each must be level with the base; a difference beyond the protocol's spread is a finding to explain before this task is done.

- [ ] **Step 2: Write the numbers down**

Add "What the collections module cost" to `docs/design/execution.md`, beside "What the platform module cost", with the tables and the spread; add a plan-2 paragraph to `docs/design/core-library.md`'s "What is measured" that summarizes it and links there.

- [ ] **Step 3: Run the end-of-batch gates**

In Git Bash:

```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules          # "ran 6, failed 0, skipped 205"
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo nextest run --workspace --release                   # the size tests
```

Expected: every stress mode's summary shows no failure; `run-corpus` reports `ran 6, failed 0, skipped 205` under all four runs. A stress failure is most likely a handle the new leaf holds across an allocation in `load_root_class` — investigate with `superpowers:systematic-debugging` before anything else.

- [ ] **Step 4: Update the progress tracker**

In `docs/PROGRESS.md`:

- Add an entry "The collections module is done (2026-09-28)" beside the platform module's, in the same style: the factory pattern and the facet's own class accessor, `moduleNamed:`, `Array new:`, `List`, the handler search, the removal with tag 10 retired, the one golden whose output changed and why, and the measurement's result, linking this plan and the design sections.
- In "Where the next session starts", replace the paragraph that says the next piece of work is the collections module; step 7, the payload space, is now unblocked (no payload grows in place), and the "Noted for later" items stay as they are.

- [ ] **Step 5: Commit**

```bash
git add docs/design/execution.md docs/design/core-library.md docs/PROGRESS.md
git commit -m "docs: record what the collections module cost and where the next session starts"
```

Before handing the branch on, check that the documentation lets a fresh session start from it (`docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`), as the user gates merges on it.
