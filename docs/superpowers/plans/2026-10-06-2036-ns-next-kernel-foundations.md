# The kernel's foundations — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The second of the kernel completion's six sub-projects:
Primordial Soup's error classes (`ArgumentError`, `NotFound`,
`CannotReturn`, `NonBooleanReceiver`) and an own `CannotInstantiate`,
signalled where today an `Error` with a text is; the `isKindOf…` rule;
`yourself` and `identityHash`; a public `Proxy`; a public `Future` whose
class-side `computing:` makes the runtime's future; and the kernel facet
answering them.

**Architecture:** Rust names the concrete class at the site that finds an
error: `NewspeakError` carries an `ErrorClass` and a `subject` value, and
`Mutator::signal_vm_error`, the one place a Rust error becomes a Newspeak
object, instantiates that kernel class. Every new kernel class is a
builtin (the bootstrap requires one nested class of `Kernel.ns` per
`BuiltinClass`), with the next free wire tags. A loop's non-Boolean
condition signals through a new instruction, `SignalNonBoolean`, in
compiled code and through the native loop's continuation otherwise. The
runtime's future class is renamed `FutureImplementation` and moves under
`Proxy`; the public `Future` is a class whose class side's `computing:`
answers one through a new mirror row.

**Tech Stack:** Rust 1.98.0 (`crates/ns-runtime`), Newspeak
(`crates/ns-runtime/core-library/Kernel.ns`, `CollectionsFactory.ns`,
`PlatformFactory.ns`), goldens (`goldens/**/*.ns` + `.toml`, run by
`crates/ns-cli/tests/run_goldens.rs` under both lowerings).

**Spec:** `docs/superpowers/specs/2026-10-06-ns-next-kernel-foundations-design.md`.
Where this plan departs from it, it says so and why:

- The spec lists a `NotFound` kind for Rust. No Rust site raises
  `NotFound`, and `NotFound` lives in `CollectionsFactory.ns`, which Rust
  cannot build by name, so `ErrorClass` has no `NotFound` variant.
- The spec says the runtime's future "stays `BuiltinClass::Future`". The
  wire tag 30 stays; the Rust variant is renamed `FutureImplementation`
  after its `Kernel.ns` name, so that `BuiltinClass::Future` can name the
  public class.
- The spec says the kind would be boxed if `Signal` grew past its size
  test. Boxing still adds a word; instead `message` becomes a `Box<str>`
  (16 bytes, not 24), which frees the word `subject` needs.
- The spec lists four `CannotInstantiate` sites from Rust; every refusal
  whose text begins "cannot instantiate" is one, which adds the root
  clause's defect and a value class's mutable slot.
- Division by zero is an `ArgumentError` whose argument is the zero, as
  Primordial Soup's `//`, `quo:` and `rem:` signal
  (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1959-1962`,
  `:2067-2070`, `:2089-2092`).
- `NonBooleanReceiver`'s error carries the selector `value`, the send whose
  answer was no Boolean, in both lowerings alike.
- The divergent-future text names a future, no longer a simultaneous slot,
  since a program's own future can diverge too.

## Global Constraints

- Communicate with the user in German; all code, comments, identifiers,
  commit messages and documentation in English.
- Names are written out (CLAUDE.md, "Conventions"): no identifier of one or
  two characters, no truncated word; clippy's `min_ident_chars` enforces
  the short ones.
- **Never `{:?}` a `Value`, never `.unwrap()`/`.expect()`/`{:?}` an
  `EvaluationResult`**; tests use the file's local `expect_ok` /
  `expect_error` helpers.
- **`vm.rs` holds no dispatch logic** (CLAUDE.md): the `SignalNonBoolean`
  arm calls one function in `interpreter/` that builds the error, and
  nothing else.
- **A new Rust-side field holding a handle needs its `Trace` impl**:
  `NewspeakError::subject` is traced wherever `trace.rs` traces
  `NewspeakError::receiver` (`trace.rs:73`, `:110`).
- **A wire tag is assigned once** (`image/opcodes.rs`,
  `tests/opcode_tags.rs`): the new builtin classes take **32**
  `ArgumentError`, **33** `CannotReturn`, **34** `NonBooleanReceiver`,
  **35** `CannotInstantiate`, **36** `Proxy`, **37** `Future`; the
  runtime's future keeps **30**; the new instruction takes opcode **40**.
  Nothing is renumbered.
- **Every instruction answers its depth in `bytecode::outer_reach_of` and
  `bytecode::compact::narrow_outer_reach`**: `SignalNonBoolean` reads only
  its own frame's register and answers 0 in both.
- **Program outcomes are pinned**: a golden whose output changes changes
  because the decided semantics changed; say so in its comment. Each task
  adds goldens pinning its new behaviour; every golden runs under both
  lowerings.
- **The VM mirror is a capability**: `CannotInstantiate` for the mirror's
  class carries `nil` as its `behavior`; no new row hands out the mirror,
  its class or `Top`.
- **A public member of a shipped core-library module is documented**: each
  new or changed `public` member of `Kernel.ns`, `CollectionsFactory.ns` and
  `PlatformFactory.ns` has a `(* … *)` comment directly above it, or shares
  one with its family, one to four lines, naming no other implementation
  (no "Primordial Soup" in a comment this plan writes in those files).
- **Comments explain the decision**, not the mechanics; match the comment
  density of the surrounding code. A Newspeak module inline in a Rust test
  is an `indoc! {"…"}`, indented with spaces, the closing `"}` on its own
  line. `.ns` files outside `corpus/` are indented with four spaces.
- **Documentation describes the current state**: when a task changes
  behaviour, the design-document sentences describing the old behaviour
  are rewritten in the same commit (no "previously", no "historical").
- Conventional commits; every commit message ends with
  ```
  Co-Authored-By: <the implementing model's name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2
  ```
  where the first line names the implementer's own model. Never override
  the author or committer identity.
- Work on the checked-out branch `feature/kernel-foundations` in
  `D:\Workspace\ns-next`; no worktree.
- Run every command in the foreground, never in the background; pass a
  600000 ms timeout to builds and test suites.
- **The per-commit gates** (CLAUDE.md, "Working in this repository"), run
  from the repository root before every commit, all green:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace
  cargo test --workspace --doc
  cargo run -p ns-analyzer -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  Task 1 and Task 3 also run the release size tests:
  `cargo nextest run -p ns-runtime --release a_signal_fits_in_the_niche_of_its_one_unboxed_error`.
  The end-of-batch gates (the five stress modes, `run-corpus` plain and
  stressed, the benchmarks profile, the release size tests) are **not** run
  per task; the controller runs them once after Task 7 (the
  `finishing-a-batch` skill). Every commit before that is work in progress.
  No task touches `ns-syntax`, `ns-analyzer`, `schemas/` or the plugin, so
  the Gradle gate does not apply.
- Run one golden or a family with
  `cargo nextest run -p ns-cli --test run_goldens <part of its path>`.

## Review Focus

- **An error class caught by `on: Error do:`** -- every existing program
  that catches today's texts through `Error` must catch the new classes
  unchanged and read the same `messageText` (every task re-runs the whole
  golden suite; Task 2 adds a golden that catches an `ArgumentError` from
  a leaf both ways).
- **The optimized and the reference lowering reaching the same error** --
  a literal loop is inlined only under the optimized lowering, so its
  `NonBooleanReceiver` comes from the new instruction there and from the
  native loop under the reference lowering; both must build the same
  class, text, selector and receiver (Task 3's goldens run under both).
- **A future as an argument of a new row or forwarder** -- a future handed
  to `Future computing:`, or a resolved one asked `identityHash`,
  `yourself` or `isKindOf…`, must behave as its value (Task 6 adds a
  golden for each).
- **`doesNotUnderstand:` near the depth limit** -- the `isKindOf…` test
  adds sends to every `doesNotUnderstand:`; the depth-limit goldens and
  `exceptions.rs` must stay green (Task 5 re-runs them by name).
- **An image holding the new objects** -- a caught `ArgumentError`, a
  `Proxy` subclass instance and a program's `Future` must survive an
  image round trip (Task 6 adds the image test).

---

### Task 1: The error classes and the error's class in Rust

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass`, `ALL`,
  `NewspeakError`, a new `ErrorClass`)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`),
  `crates/ns-runtime/src/image/decode.rs` (the tag table at ~957)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs`
  (`kernel_class_name`, the slot assertion at ~498-510, the method tables)
- Modify: `crates/ns-runtime/src/interpreter/unwind.rs` (`signal_vm_error`)
- Modify: `crates/ns-runtime/src/trace.rs` (`:73`, `:110`)
- Modify: every file that builds a `NewspeakError` (17 files, 53 sites:
  `grep -rn "NewspeakError {" crates/ns-runtime/src`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (four classes after
  `Halt`), `crates/ns-runtime/core-library/PlatformFactory.ns`
  (`KernelNamespace`)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Create: `goldens/library/exception/the_error_classes_carry_what_they_were_signalled_for.{ns,toml}`
- Create: `goldens/library/exception/an_uncaught_argument_error_describes_its_argument.{ns,toml}`
- Modify: `goldens/library/exception/the_kernel_namespace_answers_the_exception_classes.{ns,toml}`

**Interfaces:**
- Produces, in `types.rs`:
  ```rust
  /// The kernel class `Mutator::signal_vm_error` signals for an error: an
  /// `Error`, or one of the classes that carry what they were signalled
  /// for in `NewspeakError::subject`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ErrorClass {
      Error,
      ArgumentError,
      CannotReturn,
      NonBooleanReceiver,
      CannotInstantiate,
  }

  pub struct NewspeakError<'gc> {
      pub selector: Symbol,
      pub receiver: Value<'gc>,
      /// What the class carries: `ArgumentError`'s `argument`,
      /// `CannotReturn`'s `result`, `NonBooleanReceiver`'s `receiver`,
      /// `CannotInstantiate`'s `behavior`; nil for an `Error`.
      pub subject: Value<'gc>,
      pub class: ErrorClass,
      pub message: Box<str>,
  }

  impl<'gc> NewspeakError<'gc> {
      pub fn error(selector: Symbol, receiver: Value<'gc>, message: impl Into<Box<str>>) -> Self;
      pub fn argument_error(selector: Symbol, receiver: Value<'gc>, argument: Value<'gc>, message: impl Into<Box<str>>) -> Self;
      pub fn cannot_return(selector: Symbol, receiver: Value<'gc>, result: Value<'gc>, message: impl Into<Box<str>>) -> Self;
      pub fn non_boolean_receiver(selector: Symbol, value: Value<'gc>, message: impl Into<Box<str>>) -> Self;
      pub fn cannot_instantiate(selector: Symbol, receiver: Value<'gc>, behavior: Value<'gc>, message: impl Into<Box<str>>) -> Self;
  }
  ```
  `non_boolean_receiver` sets both `receiver` and `subject` to `value`.
- Produces `BuiltinClass::{ArgumentError, CannotReturn, NonBooleanReceiver,
  CannotInstantiate}` and the `Kernel.ns` classes of those names, and
  `platform kernel ArgumentError`, `CannotReturn`, `CannotInstantiate`.
  Later tasks use the constructors; this task converts every site with
  `NewspeakError::error` and changes no behaviour.

- [ ] **Step 1: Write the failing goldens**

`goldens/library/exception/the_error_classes_carry_what_they_were_signalled_for.ns`:

```newspeak
(* `ArgumentError`, `CannotReturn` and `CannotInstantiate` are Errors, so
   `on: Error do:` catches them, and each carries what it was signalled
   for in a public slot; with no text, its description is its class's name
   and that slot's `printString`. *)
class TheErrorClassesCarryWhatTheyWereSignalledFor = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. |
        transcript show: ([(kernel ArgumentError value: 3) signal] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([(kernel CannotReturn result: 4) signal] on: kernel Error do: [:error | error result]) printString; newLine.
        transcript show: ([(kernel CannotInstantiate behavior: kernel Error) signal] on: kernel Error do: [:error | error behavior]) printString; newLine.
        transcript show: ([(kernel ArgumentError value: 3) signal] on: kernel Error do: [:error | error description]); newLine.
        transcript show: ([(kernel CannotReturn result: 4) signal] on: kernel Error do: [:error | error description]); newLine.
        transcript show: ([(kernel CannotInstantiate behavior: kernel Error) signal] on: kernel Error do: [:error | error description]); newLine.
        transcript show: ([kernel ArgumentError new signal: 'bad'] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([kernel ArgumentError new signal] on: kernel Error do: [:error | error argument]) printString; newLine.
        transcript show: (kernel ArgumentError value: 3) isResumable printString; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
3
4
Error
ArgumentError: 3
CannotReturn: 4
CannotInstantiate: Error
bad
nil
false
'''
```

`goldens/library/exception/an_uncaught_argument_error_describes_its_argument.ns`:

```newspeak
(* An uncaught `ArgumentError` signalled without a text ends the run with
   its description, its class's name and its argument. *)
class AnUncaughtArgumentErrorDescribesItsArgument = Object new () (
    public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        (platform kernel ArgumentError value: 7) signal.
        platform transcript show: 'not reached'; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
before
'''
stderr = '''
Uncaught Newspeak error: ArgumentError: 7
'''
```

In `the_kernel_namespace_answers_the_exception_classes.ns`, add after the
`Message` line:

```newspeak
        transcript show: kernel ArgumentError printString; newLine.
        transcript show: kernel CannotReturn printString; newLine.
        transcript show: kernel CannotInstantiate printString; newLine.
```

and in its `.toml` the three lines `ArgumentError`, `CannotReturn`,
`CannotInstantiate` after `Message`. Its comment: "the exception classes,
`Message` and the classes a program may name".

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens error_classes_carry uncaught_argument_error the_kernel_namespace`
Expected: FAIL -- `kernel ArgumentError` is a `MessageNotUnderstood`.

- [ ] **Step 3: Add the four classes to `Kernel.ns`**

After `Halt` (`Kernel.ns:424-431`), in this order:

```newspeak
    (* An argument a method cannot take: an index out of range, a size, an
       object of the wrong kind, a divisor of zero, a block given the wrong
       number of arguments. Carries the argument. *)
    public class ArgumentError = Error new (
        (* The argument that was refused. *)
        | public argument ::= nil. |
    ) (
        (* `messageText`, or the class's name and the argument's
           `printString` when it is nil. *)
        public description = ( ^messageText isNil ifTrue: ['ArgumentError: ', argument printString] ifFalse: [messageText] )
    ) : (
        (* A new ArgumentError carrying `offending`. *)
        public value: offending = ( ^self new argument: offending )
        (* `signal` and `signal: text` signal a new instance. *)
        public signal = ( ^self new signal )
        public signal: text = ( ^self new signal: text )
    )
    (* A return to an activation that has already returned: a `^` from a
       block whose home has returned, or a `return:`, `retry` or `resume:`
       aimed at one. Carries the value that was being returned. *)
    public class CannotReturn = Error new (
        (* The value that could not be returned. *)
        | public result ::= nil. |
    ) (
        (* `messageText`, or the class's name and the result's
           `printString` when it is nil. *)
        public description = ( ^messageText isNil ifTrue: ['CannotReturn: ', result printString] ifFalse: [messageText] )
    ) : (
        (* A new CannotReturn carrying `value`. *)
        public result: value = ( ^self new result: value )
        (* `signal` and `signal: text` signal a new instance. *)
        public signal = ( ^self new signal )
        public signal: text = ( ^self new signal: text )
    )
    (* What a `whileTrue:` or `whileFalse:` signals when its condition
       answers an object that is no Boolean. Carries that object. *)
    (* Private, so a program catches it as an `Error`: no program needs to
       name the one mistake it reports. *)
    private class NonBooleanReceiver = Error new (
        (* The condition's answer. *)
        | public receiver ::= nil. |
    ) (
        (* `messageText`, or the class's name and the receiver's
           `printString` when it is nil. *)
        public description = ( ^messageText isNil ifTrue: ['NonBooleanReceiver: ', receiver printString] ifFalse: [messageText] )
    ) : (
        (* A new NonBooleanReceiver carrying `value`. *)
        public receiver: value = ( ^self new receiver: value )
        (* `signal` and `signal: text` signal a new instance. *)
        public signal = ( ^self new signal )
        public signal: text = ( ^self new signal: text )
    )
    (* A class that refused to make an instance through the factory it was
       sent: one made only by a literal, by its declaration, or by another
       factory. Carries the class, or nil when it may not be named. *)
    public class CannotInstantiate = Error new (
        (* The class that refused. *)
        | public behavior ::= nil. |
    ) (
        (* `messageText`, or the class's name and the refusing class's
           `printString` when it is nil. *)
        public description = ( ^messageText isNil ifTrue: ['CannotInstantiate: ', behavior printString] ifFalse: [messageText] )
    ) : (
        (* A new CannotInstantiate carrying `refusing`. *)
        public behavior: refusing = ( ^self new behavior: refusing )
        (* `signal` and `signal: text` signal a new instance. *)
        public signal = ( ^self new signal )
        public signal: text = ( ^self new signal: text )
    )
```

(The slot is `argument`, not `value`: an exception answering `value` would
pass for a block wherever the kernel sends `value` to an argument. The
slot of `CannotInstantiate` is `behavior`, not `class`, which would
override `Object>>class`. Keep these two reasons as comments beside the
slots.)

- [ ] **Step 4: Make them builtins**

- `types.rs`: append `ArgumentError, CannotReturn, NonBooleanReceiver,
  CannotInstantiate` to `BuiltinClass` and to `ALL` (now 30 entries), each
  with a one-line doc comment as `Halt`'s neighbours have.
- `image/encode.rs` `builtin_class_tag`: 32, 33, 34, 35. `image/decode.rs`:
  the same four in its tag table. `tests/opcode_tags.rs`: one
  `assert_eq!` each.
- `interpreter/kernel.rs` `kernel_class_name`: the four names; the slot
  assertion (`kernel.rs:~500`) admits the four with the other exception
  classes; the method tables: the class-side `signal`, `signal:` and the
  secondary factories, and the instance-side `description`, go into
  `NOT_FORWARDERS` beside `Halt`'s (`(BuiltinClass::ArgumentError,
  "value:")`, `(BuiltinClass::CannotReturn, "result:")`,
  `(BuiltinClass::NonBooleanReceiver, "receiver:")`,
  `(BuiltinClass::CannotInstantiate, "behavior:")`, and `"description"`,
  `"signal"`, `"signal:"` for each). Run
  `cargo nextest run -p ns-runtime kernel` and add exactly what the table
  tests name.

- [ ] **Step 5: Give `NewspeakError` its class and subject**

Replace the struct in `types.rs` with the one under **Interfaces**, add
`ErrorClass` and the five constructors, each a struct literal; `error`
sets `subject: Value::nil()` and `class: ErrorClass::Error`. Convert every
`NewspeakError { selector, receiver, message }` literal in
`crates/ns-runtime/src` to `NewspeakError::error(selector, receiver,
message)` -- including the test literals in `unwind.rs` (`:651`, `:681`)
-- and every reader of `.message` that needs a `String` to
`.message.to_string()` or `String::from(error.message)` (`host_error`,
`interpreter/mod.rs:890`, keeps `HostError::message: String`). `trace.rs`
traces `error.subject` after `error.receiver` at both sites.

`signal_vm_error` (`interpreter/unwind.rs:326`):

```rust
    pub(crate) fn signal_vm_error(
        &mut self,
        error: NewspeakError<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let text = Value::object(self.new_string(String::from(error.message)));
        let selector = self.new_symbol(error.selector);
        let receiver = error.receiver;
        let subject = error.subject;
        // The slot each class adds for what it carries; an `Error` adds none.
        let (kind, subject_slot) = match error.class {
            ErrorClass::Error => (BuiltinClass::Error, None),
            ErrorClass::ArgumentError => (BuiltinClass::ArgumentError, Some("argument")),
            ErrorClass::CannotReturn => (BuiltinClass::CannotReturn, Some("result")),
            ErrorClass::NonBooleanReceiver => (BuiltinClass::NonBooleanReceiver, Some("receiver")),
            ErrorClass::CannotInstantiate => (BuiltinClass::CannotInstantiate, Some("behavior")),
        };
        let exception = self.new_kernel_instance(kind, |name| match name {
            "messageText" => text,
            "originSelector" => selector,
            "originReceiver" => receiver,
            "reachedDefault" | "searching" => Value::boolean(false),
            "signalActivation" | "handlerActivation" => Value::nil(),
            other if subject_slot == Some(other) => subject,
            other => panic!("internal: Kernel.ns's {other} slot is one the VM does not fill"),
        });
        self.decide_send(Value::object(exception), Symbol::SIGNAL, &[])
    }
```

Update its doc comment: the error becomes an instance of the class its
`class` names, the subject in that class's slot.

No site uses a class other than `Error` yet, so this step changes no
behaviour; Task 2's goldens test each class `signal_vm_error` builds.

- [ ] **Step 6: The facet**

`PlatformFactory.ns`'s `KernelNamespace` gains, after `Message`:

```newspeak
        public ArgumentError = ( ^kernelModule ArgumentError )
        public CannotReturn = ( ^kernelModule CannotReturn )
        public CannotInstantiate = ( ^kernelModule CannotInstantiate )
```

Its class comment: "the kernel classes a program may name".
`platform kernel`'s slot comment (`PlatformFactory.ns:66`): "the namespace
of the kernel classes a program may name".

- [ ] **Step 7: Run the goldens and the size test**

Run: `cargo nextest run -p ns-cli --test run_goldens error_classes_carry uncaught_argument_error the_kernel_namespace`
Expected: PASS.
Run: `cargo nextest run -p ns-runtime --release a_signal_fits_in_the_niche_of_its_one_unboxed_error`
Expected: PASS (`Signal` 40, `Result<Step, Signal>` 56). If it fails, stop
and report the sizes; do not change the test.

- [ ] **Step 8: Documentation**

`docs/design/core-library.md`, "Exceptions": the hierarchy now has
`ArgumentError`, `CannotReturn`, `NonBooleanReceiver` and
`CannotInstantiate` under `Error`, each carrying its subject, and a VM
error becomes the class its Rust site names (`ErrorClass`,
`signal_vm_error`). Rewrite the sentence at ~1982-1983 ("Finer classes
(`ArgumentError`, `CannotReturn`, …) come when something wants to catch
one alone") to say which exist. `docs/design/image-format.md`'s builtin
class tag table, if it lists the tags: 32 to 35.

- [ ] **Step 9: Gates and commit**

Run the per-commit gates. Commit: `feat: the error classes, and the class a VM error is signalled as`.

---

### Task 2: The Rust sites signal `ArgumentError`, `CannotReturn` and `CannotInstantiate`

**Files:**
- Modify: `crates/ns-runtime/src/builtins/collections.rs` (`out_of_range`,
  `not_an_integer`, the `refusal` at ~167, `do:` at ~259)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (`:70`, division by
  zero at `:210`, the `refusal` at ~229, `:840`, `:859`, `:865`)
- Modify: `crates/ns-runtime/src/builtins/text.rs` (`:109`, `:168`, `:237`),
  `crates/ns-runtime/src/builtins/value.rs` (`:138`),
  `crates/ns-runtime/src/platform.rs` (`:62`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`raise_return`
  ~1488, the block arity error ~1985)
- Modify: `crates/ns-runtime/src/interpreter/unwind.rs`
  (`dead_activation_error`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`:483`,
  `:504`, `:1304`, `:1316`, `:1348`, `:1374`, and `:1415` if its text says
  "cannot instantiate")
- Create: `goldens/library/exception/a_leafs_argument_error_is_caught_by_its_class.{ns,toml}`
- Create: `goldens/library/exception/a_cannot_return_carries_the_returned_value.{ns,toml}`
- Create: `goldens/library/exception/a_refused_instantiation_is_a_cannot_instantiate.{ns,toml}`

**Interfaces:**
- Consumes: Task 1's constructors and `platform kernel` accessors.
- Produces: nothing later tasks call.

The rule for each site: the class Primordial Soup signals in the same
situation, the subject what Primordial Soup's slot holds.

| Site | Class | Subject |
|---|---|---|
| `out_of_range` (index), `text.rs:168` (`copyFrom:to:` range) | `ArgumentError` | the index; for `copyFrom:to:`, the bound out of range (the start when both are) |
| `not_an_integer`, `value.rs:138`, `text.rs:109` (code point), `text.rs:237` (`,`), `platform.rs:62` (`show:`), `numbers.rs:70` and the `refusal`s in `numbers.rs` and `collections.rs` ("requires … got …", sizes, capacities) | `ArgumentError` | the argument that was refused |
| `numbers.rs:210` division by zero | `ArgumentError` | the divisor, `0` |
| `numbers.rs:840` `asInteger` of a non-finite Float | `ArgumentError` | the receiver |
| `numbers.rs:859`, `:865` `asStringFixed:` | `ArgumentError` | the count |
| `collections.rs:259` `do:` | `ArgumentError` | the action |
| `interpreter/mod.rs:1985` wrong number of block arguments | `ArgumentError` | the block |
| `raise_return` (`interpreter/mod.rs:1488`) | `CannotReturn` | the value returned |
| `dead_activation_error` | `CannotReturn` | nil (the value is not at hand; `returnValue:`'s callers may pass it: give `dead_activation_error` a `result: Value<'gc>` parameter and pass the value at `builtins/activation.rs:168`, nil for `restart`) |
| every `instantiate.rs` refusal whose text begins "cannot instantiate", and the mirror's class at `:483` | `CannotInstantiate` | the class; **nil for the mirror's class** |

Everything else stays `NewspeakError::error`: the depth limit, the
divergent and malformed futures, superclass and mixin clauses that are no
class, a wrong number of factory arguments or of a method's arguments,
the value-slot store refusal (`instantiate.rs:1562`), and every
`internal:` error. A helper that takes an `index: impl Display` takes the
`Value` too; keep the texts byte for byte.

- [ ] **Step 1: Write the failing goldens**

`a_leafs_argument_error_is_caught_by_its_class.ns`:

```newspeak
(* A leaf that refuses an argument signals an `ArgumentError` carrying it,
   with the text an uncaught one ends the run with; `on: Error do:` still
   catches it. Division by zero refuses its divisor. *)
class ALeafsArgumentErrorIsCaughtByItsClass = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. array = Array new: 3. |
        transcript show: ([array at: 7] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([array at: 7] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([3 // 0] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: (['abc' , 4] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([[:each | each] value] on: kernel ArgumentError do: [:error | error class name]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
7
index 7 out of range: valid indices are 1..3 (size 3)
0
4
ArgumentError
'''
```

`a_cannot_return_carries_the_returned_value.ns`:

```newspeak
(* A `^` from a block whose home has returned signals a `CannotReturn`
   carrying the value it tried to return. *)
class ACannotReturnCarriesTheReturnedValue = Object new (
    | stored ::= nil. |
) (
    stash = (
        stored:: [^'too late'].
        ^'stashed'
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. |
        self stash.
        transcript show: ([stored value] on: kernel CannotReturn do: [:error | error result]); newLine.
        transcript show: ([stored value] on: kernel Error do: [:error | error class name]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
too late
CannotReturn
'''
```

`a_refused_instantiation_is_a_cannot_instantiate.ns`:

```newspeak
(* `Symbol new` and a value class that defines `==` refuse to instantiate:
   each signals a `CannotInstantiate` carrying the class. *)
class ARefusedInstantiationIsACannotInstantiate = Object new () (
    class Odd = Value new () (
        public == other = ( ^true )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. |
        transcript show: ([Symbol new] on: kernel CannotInstantiate do: [:error | error behavior name]); newLine.
        transcript show: ([Odd new] on: kernel CannotInstantiate do: [:error | error behavior name]); newLine.
        transcript show: ([Symbol new] on: kernel Error do: [:error | error messageText]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
Symbol
Odd
cannot instantiate Symbol: a Symbol is made only by a literal or by asSymbol
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens argument_error_is_caught cannot_return_carries refused_instantiation`
Expected: FAIL -- each `on: kernel ArgumentError do:` (and the others)
misses: the errors are still plain `Error`s.

- [ ] **Step 3: Convert the sites**

Apply the table above. Where a test in `crates/ns-runtime/src` or
`crates/ns-runtime/tests` matches an error by text, it keeps passing; add,
in `interpreter/unwind.rs`'s test module, an assertion that
`dead_activation_error(Symbol::intern("restart"), Value::nil()).class ==
ErrorClass::CannotReturn`, since no golden reaches it.

- [ ] **Step 4: Run the goldens**

Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: PASS, the three new goldens and every existing one.

- [ ] **Step 5: Documentation**

`docs/design/core-library.md`, "Exceptions": which errors are which class
(the rule and the table's classes, in prose). Close `OPEN-WORK.md`'s
"Failure blocks and Primordial Soup's error classes": the leaves signal
the classes themselves, so the failure blocks stay `^super …` for a
receiver of the wrong shape. `docs/design/execution.md`'s sentences on
§7.2's `cannotReturn` (~524, ~550, ~598) name `CannotReturn`.

- [ ] **Step 6: Gates and commit**

Commit: `feat: leaves and the VM signal ArgumentError, CannotReturn and CannotInstantiate`.

---

### Task 3: A loop's non-Boolean condition signals `NonBooleanReceiver`

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`Instruction`,
  `outer_reach_of`, the disassembly at ~1017, the list at ~807)
- Modify: `crates/ns-runtime/src/bytecode/compact.rs` (the compact form,
  the conversions both ways, `narrow_outer_reach`, the lists at ~655,
  ~1455, ~1523, the round-trip test at ~2373)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OPCODE_SIGNAL_NON_BOOLEAN = 40`),
  `crates/ns-runtime/src/image/encode.rs`, `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (the compact arm ~783 and the wide
  arm ~1270)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (`emit_must_be_boolean`,
  its test at ~4082), `crates/ns-runtime/src/compiler/reference.rs`
  (~807, ~839)
- Modify: `crates/ns-runtime/src/builtins/block.rs` (`after_condition`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (~1350, ~1422 comments)
- Modify: `crates/ns-runtime/src/symbol.rs` (remove `MUST_BE_BOOLEAN`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Closure`'s loop
  comment, ~944-948)
- Modify: `crates/ns-runtime/tests/builtins_control_flow.rs` (~106),
  `crates/ns-runtime/tests/trampoline.rs` (~618), `tests/opcode_tags.rs`,
  `tests/compact_encoding.rs` and `tests/image.rs` where they enumerate
  every instruction
- Rename + rewrite: `goldens/language/control-flow/non_boolean_while_condition_fails_with_must_be_boolean.{ns,toml}`
  → `a_non_boolean_loop_condition_signals_non_boolean_receiver.{ns,toml}`;
  `variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean.{ns,toml}`
  → `a_variable_loop_condition_that_goes_non_boolean_signals_non_boolean_receiver.{ns,toml}`
- Create: `goldens/language/control-flow/a_non_boolean_receiver_is_caught_as_an_error.{ns,toml}`

**Interfaces:**
- Consumes: `NewspeakError::non_boolean_receiver` (Task 1).
- Produces:
  ```rust
  /// The guard-failure path of an inlined `whileTrue:`/`whileFalse:`:
  /// signals `NonBooleanReceiver` for the value in `register`, which the
  /// condition answered. Ends its path: nothing after it runs.
  Instruction::SignalNonBoolean { register: u16 }
  ```
  and, in `interpreter/unwind.rs`:
  ```rust
  /// The `NonBooleanReceiver` a loop signals when its condition answers
  /// `value`, which is no Boolean; one error for the inlined loop and the
  /// native one alike.
  pub(crate) fn non_boolean_condition<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> NewspeakError<'gc>
  ```
  with selector `Symbol::VALUE` (or `Symbol::intern("value")` if there is
  no constant) and the text
  `format!("a loop's condition answered {}, which is no Boolean", heap.describe(value))`.

- [ ] **Step 1: Rewrite and add the goldens**

`git mv` the two goldens to their new names. The first:

```newspeak
(* An inlined loop whose condition answers no Boolean signals a
   `NonBooleanReceiver` once the condition has run, and nothing after it
   runs. A loop's behaviour is the library's to define: there is no
   `mustBeBoolean` for a program to answer. *)
class ANonBooleanLoopConditionSignalsNonBooleanReceiver = Object new (
) (
    public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        [platform transcript show: 'cond'; newLine. 3] whileTrue: [nil]
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
before
cond
'''
stderr = '''
Uncaught Newspeak error: a loop's condition answered 3, which is no Boolean
'''
```

The second keeps its program (renamed class
`AVariableLoopConditionThatGoesNonBooleanSignalsNonBooleanReceiver`, its
comment: "A native loop whose condition goes non-Boolean after two rounds
signals a `NonBooleanReceiver`, after exactly two bodies."), and its
`.toml` keeps `1`, `2` on stdout with stderr
`Uncaught Newspeak error: a loop's condition answered 2, which is no Boolean`.

`a_non_boolean_receiver_is_caught_as_an_error.ns`:

```newspeak
(* A `NonBooleanReceiver` is an Error carrying the condition's answer;
   `whileFalse:` signals it as `whileTrue:` does, inlined and native. *)
class ANonBooleanReceiverIsCaughtAsAnError = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. condition |
        transcript show: ([[4] whileFalse: [nil]] on: Error do: [:error | error class name]); newLine.
        transcript show: ([[4] whileFalse: [nil]] on: Error do: [:error | error receiver]) printString; newLine.
        condition:: [5].
        transcript show: ([condition whileTrue: [nil]] on: Error do: [:error | error receiver]) printString; newLine.
        transcript show: ([condition whileFalse] on: Error do: [:error | error messageText]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
NonBooleanReceiver
4
5
a loop's condition answered 5, which is no Boolean
'''
```

(If `whileFalse` without an argument does not exist in `Kernel.ns`, use
`condition whileFalse: [nil]` on that line.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens non_boolean`
Expected: FAIL -- the errors are a `MessageNotUnderstood` of
`#mustBeBoolean`.

- [ ] **Step 3: The instruction**

Follow `Instruction::ReturnSlotIfFilled`'s footprint
(`grep -rn ReturnSlotIfFilled crates/ns-runtime/src crates/ns-runtime/tests`)
for every place an instruction is enumerated: `bytecode.rs` (the variant,
`outer_reach_of` answering 0 through its catch-all or an explicit arm,
the disassembly `SignalNonBoolean r{register}`), `bytecode/compact.rs`
(a compact variant holding the register, both conversions,
`narrow_outer_reach` 0, the lists that ask whether an instruction ends a
path -- it does, like a return), `image/opcodes.rs` (opcode 40, its doc
comment), `image/encode.rs` and `image/decode.rs` (the register is
validated against the body's register count like every other register
operand), `compiler/reference.rs`'s lists, and `vm.rs`'s two arms:

```rust
CompactInstruction::SignalNonBoolean { register } => {
    let value = /* read the register as the neighbouring arms do */;
    break Err(Signal::Error(unwind::non_boolean_condition(&mutator.heap, value)));
}
```

(match the neighbouring arms' way of raising -- the `Literal::UnresolvedIdentifier`
path at `vm.rs:~2169` raises from the loop). `tests/opcode_tags.rs` pins
`OPCODE_SIGNAL_NON_BOOLEAN == 40`; the round-trip tests in
`compact.rs` and `tests/compact_encoding.rs`/`tests/image.rs` that list
every instruction gain one.

- [ ] **Step 4: Emit it and raise it natively**

`compiler/lower.rs`: `emit_must_be_boolean` becomes

```rust
    /// The guard-failure path of an inlined loop: signals
    /// `NonBooleanReceiver`, as the native `whileTrue:`/`whileFalse:` does
    /// with a non-Boolean condition value. The path ends here.
    fn emit_signal_non_boolean(&mut self, register: u16) {
        self.emit(Instruction::SignalNonBoolean { register });
    }
```

and its caller (~1971) emits it and no `Move` after it; the end label and
the exit jump's patch stay. Rename the test at ~4082 to
`while_true_compiles_to_a_back_edge_jump_and_a_non_boolean_signal` and
match `Instruction::SignalNonBoolean { .. }`. Update the doc comments at
~1086, ~1939 and ~2436.

`builtins/block.rs` `after_condition`:

```rust
        None => Err(Signal::Error(crate::interpreter::unwind::non_boolean_condition(
            &mutator.heap,
            value,
        ))),
```

Remove `MUST_BE_BOOLEAN` from `symbol.rs`; rewrite the comments at
`interpreter/mod.rs` ~1350 and ~1422 (a continuation's step now never
sends: say what remains true). `Kernel.ns`'s loop comment: "a condition
that answers no Boolean signals a `NonBooleanReceiver`".
`tests/builtins_control_flow.rs` and `tests/trampoline.rs` assert the new
text (`"which is no Boolean"`).

- [ ] **Step 5: Run the goldens, both lowerings, and the size test**

Run: `cargo nextest run -p ns-cli --test run_goldens control-flow`
Expected: PASS.
Run: `cargo nextest run -p ns-runtime --release a_signal_fits_in_the_niche_of_its_one_unboxed_error`
Expected: PASS.

- [ ] **Step 6: Documentation**

Every mention of `mustBeBoolean` in `docs/design/` describes the current
behaviour instead: `compiler.md` (~403-404, ~444, ~1004, ~1042, ~1227),
`dispatch.md` (~439, ~1269), `execution.md` (~381, ~5998, ~6025). The
instruction gets its entry wherever `execution.md` or `compiler.md` lists
the instruction set (grep `ReturnSlotIfFilled` in `docs/design/`).
`image-format.md`'s opcode table, if it lists opcodes: 40.

- [ ] **Step 7: Gates and commit**

Commit: `feat: a loop's non-Boolean condition signals NonBooleanReceiver`.

---

### Task 4: The collections signal `NotFound` and `ArgumentError`

**Files:**
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (header
  comment ~37-39, `Collections` facet, a `NotFound` class, `:350`, `:382`,
  `:417`, `:526`)
- Modify: `goldens/library/collection/a_map_and_a_set_keep_insertion_order.ns`
  only if it catches by class (it pins the text, which stays)
- Create: `goldens/library/collection/a_missing_key_signals_not_found.{ns,toml}`

**Interfaces:**
- Consumes: `kernelModule ArgumentError` (Task 1).
- Produces: `platform collections NotFound`, a class with the class-side
  factory `element:` and the public slot `element`.

- [ ] **Step 1: Write the failing golden**

`a_missing_key_signals_not_found.ns`:

```newspeak
(* `Map>>at:` of an absent key signals a `NotFound` carrying the key, an
   Error with the text an uncaught one ends the run with; a List index out
   of range and a negative capacity signal an `ArgumentError`, as an
   Array's do. *)
class AMissingKeySignalsNotFound = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. kernel = platform kernel. map list |
        map:: collections Map new.
        map at: #present put: 1.
        transcript show: ([map at: #missing] on: collections NotFound do: [:error | error element]) printString; newLine.
        transcript show: ([map at: #missing] on: kernel Error do: [:error | error messageText]); newLine.
        list:: collections List new.
        transcript show: ([list at: 2] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([collections Map new: -1] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([(collections NotFound element: 3) signal] on: kernel Error do: [:error | error description]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
#missing
key not found: #missing
2
-1
NotFound: 3
'''
```

(`createTable:`'s `count` at `:417` is the capacity the program passed.)

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_missing_key_signals_not_found`
Expected: FAIL -- `collections NotFound` is a `MessageNotUnderstood`.

- [ ] **Step 3: `NotFound` and the sites**

In `CollectionsFactory.ns`, a class beside `List`, `Map` and `Set`:

```newspeak
    (* An element or key a collection was asked for and does not hold.
       Carries it. *)
    private class NotFound = kernelModule Error new (
        (* The element or key that was not found. *)
        | public element ::= nil. |
    ) (
        (* `messageText`, or the class's name and the element's
           `printString` when it is nil. *)
        public description = ( ^messageText isNil ifTrue: ['NotFound: ', element printString] ifFalse: [messageText] )
    ) : (
        (* A new NotFound carrying `missing`. *)
        public element: missing = ( ^self new element: missing )
    )
```

(If the resolver refuses `kernelModule Error new` as a superclass clause
here, declare `private Error = kernelModule Error` style access as the
module's other references to the kernel do and report what was needed.)
The `Collections` facet gains `public NotFound = ( ^outer CollectionsFactory NotFound )`
with a comment. The sites:

- `:382` `^(NotFound element: key) signal: 'key not found: ', key printString`
- `:350` `indexOutOfRange:` signals `(kernelModule ArgumentError value: index) signal: …` with the same text
- `:417`, `:526` `(kernelModule ArgumentError value: count) signal: …`, same texts
- `:185`, `:201` (`removeFirst`/`removeLast` of an empty `List`) stay `Error`s.

The header comment (~37-39): errors are the kernel's `ArgumentError` and
this module's `NotFound` where those apply, and `Error`s with texts of
their own otherwise. The public members' comments that say "Signals an
Error, 'key not found: …'" name `NotFound`; those for an index or a
capacity name `ArgumentError`.

- [ ] **Step 4: Run the goldens**

Run: `cargo nextest run -p ns-cli --test run_goldens collection`
Expected: PASS.

- [ ] **Step 5: Documentation**

`docs/design/core-library.md`, "The collections module (plan 2)": which
errors the collections signal.

- [ ] **Step 6: Gates and commit**

Commit: `feat: the collections signal NotFound and ArgumentError`.

---

### Task 5: `yourself`, `identityHash` and the `isKindOf…` rule

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Object`, `Behavior`,
  `Value`, `Number`, `Boolean`, `Integer`, `Float`, `String`, `Array`,
  `Closure`)
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (`List`,
  `Map`, `Set`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`QUICK_METHODS`,
  `NOT_FORWARDERS`, `REPORTED_UNDER_ANOTHER_SELECTOR`)
- Create: `goldens/library/object/yourself_answers_the_receiver_of_a_cascade.{ns,toml}`
- Create: `goldens/library/object/identity_hash_is_a_values_hash.{ns,toml}`
- Create: `goldens/library/object/is_kind_of_answers_what_a_receiver_declares.{ns,toml}`

**Interfaces:**
- Produces: `Object>>yourself`, `Object>>identityHash` (Task 6's `Proxy`
  overrides both).

- [ ] **Step 1: Write the failing goldens**

`yourself_answers_the_receiver_of_a_cascade.ns`:

```newspeak
(* `yourself` answers the receiver, so a cascade that ends with it answers
   the object its messages went to rather than the last one's answer. *)
class YourselfAnswersTheReceiverOfACascade = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. array = (Array new: 2) at: 1 put: 'a'; at: 2 put: 'b'; yourself. |
        transcript show: array size printString; newLine.
        transcript show: (array at: 2); newLine.
        transcript show: (3 yourself + 1) printString; newLine.
        transcript show: (nil yourself) printString; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
2
b
4
nil
'''
```

`identity_hash_is_a_values_hash.ns`:

```newspeak
(* `identityHash` is an object's identity hash, which `hash` answers for
   an object without an `=` of its own; for a value, whose identity is its
   equality, it is its `hash`. *)
class IdentityHashIsAValuesHash = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. object = Object new. |
        transcript show: (object identityHash = object hash) printString; newLine.
        transcript show: (object identityHash = object identityHash) printString; newLine.
        transcript show: (3 identityHash = 3 hash) printString; newLine.
        transcript show: ('abc' identityHash = 'abc' hash) printString; newLine.
        transcript show: (#abc identityHash = #abc hash) printString; newLine.
        transcript show: (1.5 identityHash = 1.5 hash) printString; newLine.
        transcript show: (true identityHash = true hash) printString; newLine.
        transcript show: (Object identityHash = Object hash) printString; newLine.
    )
) : ()
```

`.toml`: eight lines `true`.

`is_kind_of_answers_what_a_receiver_declares.ns`:

```newspeak
(* A unary `isKindOf…` question answers true where the receiver declares
   it and false everywhere else, so a program can ask a question of its own
   of any object without touching `Object`. A keyword selector that begins
   with `isKindOf` is not such a question. *)
class IsKindOfAnswersWhatAReceiverDeclares = Object new () (
    class Shape = Object new () (
        public isKindOfShape = ( ^true )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. Error = platform kernel Error. |
        transcript show: (Array new: 2) isKindOfArray printString; newLine.
        transcript show: 3 isKindOfInteger printString; newLine.
        transcript show: 100000000000000000000000 isKindOfInteger printString; newLine.
        transcript show: 3 isKindOfNumber printString; newLine.
        transcript show: 3 isKindOfFloat printString; newLine.
        transcript show: 3 isKindOfFraction printString; newLine.
        transcript show: 1.5 isKindOfFloat printString; newLine.
        transcript show: 1.5 isKindOfInteger printString; newLine.
        transcript show: 'abc' isKindOfString printString; newLine.
        transcript show: #abc isKindOfString printString; newLine.
        transcript show: true isKindOfBoolean printString; newLine.
        transcript show: [] isKindOfClosure printString; newLine.
        transcript show: Object isKindOfBehavior printString; newLine.
        transcript show: collections List new isKindOfList printString; newLine.
        transcript show: collections Map new isKindOfMap printString; newLine.
        transcript show: collections Set new isKindOfSet printString; newLine.
        transcript show: Object new isKindOfArray printString; newLine.
        transcript show: nil isKindOfString printString; newLine.
        transcript show: Shape new isKindOfShape printString; newLine.
        transcript show: 3 isKindOfShape printString; newLine.
        transcript show: 3 isKindOf printString; newLine.
        transcript show: ([3 isKindOf: 4] on: Error do: [:error | error class name]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
true
true
true
true
false
false
true
false
true
true
true
true
true
true
true
true
false
false
true
false
false
MessageNotUnderstood
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens yourself identity_hash is_kind_of`
Expected: FAIL -- each new selector is a `MessageNotUnderstood`.

- [ ] **Step 3: `Object`**

After `printString` in `Object`:

```newspeak
        (* The receiver. *)
        (* For a cascade: `(Array new: 2) at: 1 put: x; yourself` answers
           the array, not the last message's answer. *)
        public yourself = ( ^self )
        (* The receiver's identity hash, an Integer that stays the same for
           its lifetime; a value's is its `hash`. *)
        public identityHash = ( ^vmMirror objectHash: self ifFail: [^super identityHash] )
```

`Object>>doesNotUnderstand:` begins with the `isKindOf…` rule:

```newspeak
        protected doesNotUnderstand: message = (
            | exception value selector |
            (* An `isKindOf…` question nobody declared answers false, so a
               program can ask one of any object; a keyword selector is no
               such question. *)
            selector:: message selector asString.
            (message arguments isNil or: [message arguments size = 0]) ifTrue: [
                (selector size >= 8 and: [(selector copyFrom: 1 to: 8) = 'isKindOf']) ifTrue: [^false]].
            exception:: outer Kernel MessageNotUnderstood new.
            …unchanged…
        )
```

and its comment gains the sentence "A unary selector that begins with
`isKindOf` answers false instead." (the `doesNotUnderstand:` comment is
the protocol's; keep the §5.5 text).

`Value` and `Behavior` answer `identityHash` with `^self hash`, each with
the comment "Its `hash`: a value's identity is its equality."

The private `Future` (`Kernel.ns:468`) overrides every public member of
`Object` as protected, which `future_overrides_every_public_member_of_object_as_protected`
checks; add the two new ones beside its `hash`:

```newspeak
        protected yourself = ( ^super yourself )
        protected identityHash = ( ^super identityHash )
```

- [ ] **Step 4: The answers**

Each with `(* True: the receiver is a <Name>. *)` or one shared comment
per class where two sit together:

- `Array`: `public isKindOfArray = ( ^true )`
- `Behavior`: `public isKindOfBehavior = ( ^true )`
- `Boolean`: `public isKindOfBoolean = ( ^true )`
- `Closure`: `public isKindOfClosure = ( ^true )`
- `Number`: `public isKindOfNumber = ( ^true )`, and
  `isKindOfFloat`, `isKindOfInteger`, `isKindOfFraction` each `( ^false )`
  under one comment "False: a Number is none of these unless its class
  says so." -- declared so that a subclass's answer overrides a known
  `false` rather than the `doesNotUnderstand:` rule.
- `Integer`: `public isKindOfInteger = ( ^true )`
- `Float`: `public isKindOfFloat = ( ^true )`
- `String`: `public isKindOfString = ( ^true )` (`Symbol` inherits it)
- `CollectionsFactory.ns`'s `List`, `Map`, `Set`: `isKindOfList`,
  `isKindOfMap`, `isKindOfSet`.

`Integer>>isKindOfFraction` is left to the numbers sub-project, which
brings `Fraction`.

- [ ] **Step 5: The kernel's tables**

`interpreter/kernel.rs`: every new `^true`/`^false` body goes into
`QUICK_METHODS` as `MethodFastPath::Constant(QuickConstant::True)` /
`False`, `yourself` as `MethodFastPath::Receiver`;
`(BuiltinClass::Object, "identityHash", "hash")` into
`REPORTED_UNDER_ANOTHER_SELECTOR` (the row `objectHash:ifFail:` reports
`hash`); `Value>>identityHash` and `Behavior>>identityHash` into
`NOT_FORWARDERS` ("a send of `hash`, which no leaf could answer for a
value"). Run `cargo nextest run -p ns-runtime kernel` and follow what the
table tests report.

- [ ] **Step 6: Run the goldens, the depth-limit ones by name**

Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: PASS -- in particular every golden with `depth` or `recursion`
in its name, and `cargo nextest run -p ns-runtime --test exceptions`.

- [ ] **Step 7: Documentation**

`docs/design/core-library.md`: `yourself`, `identityHash` and the
`isKindOf…` rule where `Object`'s protocol is described.

- [ ] **Step 8: Gates and commit**

Commit: `feat: yourself, identityHash and the isKindOf rule`.

---

### Task 6: `Proxy`, `FutureImplementation` and a public `Future`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Proxy` new, the
  private future renamed and moved under it, `Future` new)
- Modify: `crates/ns-runtime/src/types.rs` (rename
  `BuiltinClass::Future` → `FutureImplementation`; append `Proxy`,
  `Future`), `image/encode.rs`, `image/decode.rs` (tags 36, 37; 30 stays
  the implementation's), `interpreter/kernel.rs`
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (a
  `FuturePrimitive::Computing` row), `crates/ns-runtime/src/builtins/future.rs`
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (the divergent
  text; doc comments naming `Future`)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (`Proxy`,
  `Future`, `Closure` in the facet)
- Modify: `crates/ns-runtime/tests/future.rs` (the override test moves to
  `Proxy`), `tests/opcode_tags.rs`, `tests/image.rs`
- Modify: `goldens/language/slot/simultaneous/spec_6_3_2_ill_founded_recursion_is_an_error.toml`
- Create: `goldens/library/object/a_proxy_forwards_every_message.{ns,toml}`
- Create: `goldens/library/object/future_computing_defers_its_block.{ns,toml}`
- Create: `goldens/library/object/a_future_is_made_only_by_computing.{ns,toml}`

**Interfaces:**
- Consumes: `Object>>yourself`, `identityHash` (Task 5),
  `CannotInstantiate` (Task 1).
- Produces: `BuiltinClass::FutureImplementation` (tag 30),
  `BuiltinClass::Proxy` (36), `BuiltinClass::Future` (37); the mirror row
  `futureComputing:ifFail:` (`Primitive::Future(FuturePrimitive::Computing)`),
  which answers `Mutator::new_future(closure)` for a closure and declines
  for anything else.

- [ ] **Step 1: Write the failing goldens**

`a_proxy_forwards_every_message.ns`:

```newspeak
(* A `Proxy` overrides every public member of `Object` as protected, so a
   subclass that implements only `doesNotUnderstand:` forwards every
   message a program sends it, `Object`'s included. A bare `Proxy`
   understands none of them. *)
class AProxyForwardsEveryMessage = Object new (
    | Proxy ::= nil. |
) (
    class Forwarder on: theTarget = Proxy new (
        | private target = theTarget. |
    ) (
        protected doesNotUnderstand: message = ( ^message sendTo: target )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. forwarder |
        Proxy:: platform kernel Proxy.
        forwarder:: Forwarder on: 3.
        transcript show: (forwarder + 4) printString; newLine.
        transcript show: forwarder printString; newLine.
        transcript show: (forwarder = 3) printString; newLine.
        transcript show: (forwarder hash = 3 hash) printString; newLine.
        transcript show: (forwarder yourself == 3) printString; newLine.
        transcript show: forwarder class printString; newLine.
        transcript show: (Forwarder on: nil) isNil printString; newLine.
        transcript show: ([Proxy new printString] on: Error do: [:error | error class name]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
7
3
true
true
true
Integer
true
MessageNotUnderstood
'''
```

`future_computing_defers_its_block.ns`:

```newspeak
(* `Future computing:` answers a future of its block: the block runs on the
   future's first message, once, and the future then behaves as its value,
   `==` both ways included. A computation that is no block is sent `value`.
   A future forced while it is being computed is an error. *)
class FutureComputingDefersItsBlock = Object new (
    | count ::= 0. looping ::= nil. |
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. Future = platform kernel Future. Error = platform kernel Error. future |
        future:: Future computing: [count:: count + 1. 6 * 7].
        transcript show: count printString; newLine.
        transcript show: (future + 0) printString; newLine.
        transcript show: count printString; newLine.
        transcript show: (future == 42) printString; newLine.
        transcript show: (42 == future) printString; newLine.
        transcript show: (future identityHash = 42 identityHash) printString; newLine.
        transcript show: future isKindOfInteger printString; newLine.
        transcript show: count printString; newLine.
        transcript show: ((Future computing: [3]) + (Future computing: [4])) printString; newLine.
        transcript show: ([(Future computing: 5) + 1] on: Error do: [:error | error messageText]); newLine.
        looping:: Future computing: [looping + 1].
        transcript show: ([looping + 1] on: Error do: [:error | error messageText]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
0
42
1
true
true
true
true
1
7
an object does not understand #value
divergent evaluation: a future was forced while it was being computed
'''
```

`a_future_is_made_only_by_computing.ns`:

```newspeak
(* `Future` is a class only for its `computing:`: `Future new`, and an
   instance of a subclass, signal a `CannotInstantiate` carrying the class
   that was instantiated. A subclass does not inherit `computing:`, since a
   class side is not inherited. *)
class AFutureIsMadeOnlyByComputing = Object new (
    | Future ::= nil. |
) (
    class Later = Future new () ()
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. |
        Future:: kernel Future.
        transcript show: Future printString; newLine.
        transcript show: ([Future new] on: kernel CannotInstantiate do: [:error | error behavior name]); newLine.
        transcript show: ([Future new] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([Later new] on: kernel CannotInstantiate do: [:error | error behavior name]); newLine.
        transcript show: ([Later computing: [1]] on: kernel Error do: [:error | error class name]); newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
Future
Future
a Future is made by Future computing:
Later
MessageNotUnderstood
'''
```

`spec_6_3_2_ill_founded_recursion_is_an_error.toml`: the text becomes
`divergent evaluation: a future was forced while it was being computed`
(the golden's comment: a program's own future diverges with the same
error, so the text names the future).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens proxy future_computing made_only_by_computing ill_founded`
Expected: FAIL -- `platform kernel Proxy` and `Future` are
`MessageNotUnderstood`s; the old divergent text.

- [ ] **Step 3: `Proxy` and `FutureImplementation`**

`Kernel.ns`: a public `Proxy` after `Activation`, holding every protected
override the private future has today (`Kernel.ns:476-500`: `class`, `==`,
`=`, `hash`, `printString`, every class accessor, the nil protocol,
`halt`) plus `yourself` and `identityHash` (each `^super …`), and no
`doesNotUnderstand:`:

```newspeak
    (* An object that answers none of `Object`'s public messages itself:
       each is overridden as protected, so a message from outside reaches
       `doesNotUnderstand:`. A subclass that implements only
       `doesNotUnderstand:` forwards every message it is sent. *)
    (* §5.5's proxy, "Otherwise it would be possible to distinguish
       between an object and a proxy": lookupPublic stops at a protected
       method, and the failing send falls to `doesNotUnderstand:`. Inside
       the proxy, a self send still reaches each of them. A member added to
       `Object` without its override here fails
       `a_proxy_overrides_every_public_member_of_object_as_protected`. *)
    public class Proxy = Object new () (
        protected class = ( ^super class )
        …
    )
```

The private future becomes `private class FutureImplementation = Proxy new (
| private computation. private state ::= 0. | ) ( protected
doesNotUnderstand: message = ( … unchanged … ) )`, its overrides gone
(inherited), its comment as today's with "Private" and the slot
description, plus: "`Future computing:` and a simultaneous slot clause
make every instance; `new` is never sent."

The public `Future`, after it:

```newspeak
    (* The class whose `computing:` answers a future of a block: an object
       that runs the block on its first message and behaves as the block's
       value from then on. `Future new` signals a `CannotInstantiate`. *)
    (* A class only for its class side: the future it answers is a
       `FutureImplementation`, the one class the runtime forces and sees
       through, which no program can name or subclass, so every future
       keeps the rules of docs/design/object-model.md, "Simultaneous
       slots". Its initializer refuses, so a subclass's instances are
       refused too; a subclass does not inherit `computing:` (§3.2). *)
    public class Future = Object new (
        (outer Kernel CannotInstantiate behavior: self class) signal: 'a Future is made by Future computing:'
    ) (
    ) : (
        (* A new future of `computation`'s value: a block, or any object,
           which the future sends `value` when it is first forced. *)
        public computing: computation = (
            ^vmMirror futureComputing: computation ifFail: [^self computing: [computation value]]
        )
    )
```

(If the kernel shape check refuses an initializer statement on a builtin,
or `vmMirror` is not reachable from a class side, stop and report; do not
loosen `check_kernel_shape`'s other rules.)

- [ ] **Step 4: The builtins and the row**

- `types.rs`: rename `BuiltinClass::Future` → `FutureImplementation`
  everywhere (`grep -rn "BuiltinClass::Future" crates/`), keep tag 30;
  append `Proxy` and `Future` to the enum and `ALL` (32 entries); doc
  comments: `FutureImplementation` "§6.3.2's pull future, kernel-private;
  its instances are made by the runtime (`interpreter/future.rs`)",
  `Proxy` "a forwarding object's superclass", `Future` "the public class
  whose class side makes a `FutureImplementation`; it has no instances".
- `encode.rs`/`decode.rs`: 36 and 37; every arm that names the future
  (`decode.rs:1014`, `:2316`, `:3453`, `encode.rs:390`, `:954`, `:1147`,
  `:1698`, `:2013`) names `FutureImplementation`; the encoder's printed
  name at `:954` follows the Kernel name. `tests/opcode_tags.rs`: 36, 37.
- `kernel.rs`: names `"FutureImplementation"`, `"Proxy"`, `"Future"`; the
  slot assertion admits `FutureImplementation`; the tables gain what
  their tests report (`Future`'s class-side `computing:` is a forwarder of
  its argument, not of `self`: `NOT_FORWARDERS`).
- `builtins/primitive.rs`: `FuturePrimitive::Computing`, a row
  `mirror("futureComputing:ifFail:", "computing:", Primitive::Future(FuturePrimitive::Computing))`,
  and its arm at `:185` (`false`: it never answers "no").
- `builtins/future.rs`:

  ```rust
          FuturePrimitive::Computing => {
              let closure = receiver.as_object().filter(|object| mutator.heap.is_closure(*object))?;
              Some(Ok(Step::Value(Value::object(mutator.new_future(closure)))))
          }
  ```

  (the row's subject is its first argument, the computation, as
  `futureValue:ifFail:`'s is the future); the module comment: the two
  leaves of the futures.
- `interpreter/future.rs:434`: the text
  `"divergent evaluation: a future was forced while it was being computed"`;
  `tests/future.rs:209` checks `contains("divergent evaluation")` and keeps
  passing.
- `tests/future.rs:174`: rename to
  `a_proxy_overrides_every_public_member_of_object_as_protected`, asking
  `BuiltinClass::Proxy`.

- [ ] **Step 5: The facet**

`KernelNamespace` gains `Closure`, `Future` and `Proxy`:

```newspeak
        public Closure = ( ^kernelModule Closure )
        public Future = ( ^kernelModule Future )
        public Proxy = ( ^kernelModule Proxy )
```

and `the_kernel_namespace_answers_the_exception_classes.{ns,toml}` the
three lines `Closure`, `Future`, `Proxy`.

- [ ] **Step 6: The image test**

In `crates/ns-runtime/tests/image.rs`, beside the existing round trips of
a program's objects, one that saves a program holding a caught
`ArgumentError`, a `Proxy` subclass instance (forwarding to `3`) and an
unresolved `Future computing: [6 * 7]`, loads it, and checks that the
error's `argument` is `7`, the proxy answers `+ 4` with `7`, and the
future answers `+ 0` with `42`. Copy the neighbouring test's way of
building, saving and loading; its `expect_ok` helper.

- [ ] **Step 7: Run everything touching futures**

Run: `cargo nextest run -p ns-cli --test run_goldens`
Run: `cargo nextest run -p ns-runtime --test future --test image --test collector`
Expected: PASS.

- [ ] **Step 8: Documentation**

`docs/design/object-model.md`, "Simultaneous slots": "The future"
paragraph describes `FutureImplementation` under `Proxy`, `Future
computing:` as the program's way to make one (a non-block computation is
sent `value`), the divergent text, and why `Future` is a class without
instances (a subclass would be a future the runtime does not know);
"Where Rust sees a future" names `FutureImplementation`. A sentence on
`Proxy` where `object-model.md` or `core-library.md` describes the
kernel's classes. `image-format.md`'s tag table: 36, 37.

- [ ] **Step 9: Gates and commit**

Commit: `feat: Proxy, and a public Future whose computing: makes the runtime's future`.

---

### Task 7: The living documents

**Files:**
- Modify: `docs/design/core-library.md` ("Planned: the kernel completed to
  Primordial Soup's core API", "Decisions")
- Modify: `docs/OPEN-WORK.md` ("Next")
- Modify: `docs/PROGRESS.md`
- Delete: `docs/superpowers/specs/2026-10-06-ns-next-kernel-foundations-design.md`

- [ ] **Step 1: Move the spec's decisions into `core-library.md`**

In "Planned: the kernel completed …", sub-project 2 is built (2026-10-06,
a link to this plan), as sub-project 1's entry is. Its decisions go under
"Decisions" as one bullet in the form of the aligning bullet: the error
classes under `Error` with their public slots (`argument`, not `value`,
and why), `CannotInstantiate` as an addition, `NonBooleanReceiver` only
for loops (the specification has no inlining, a conditional keeps its
real send), Rust naming the class at the error's site
(`ErrorClass`/`signal_vm_error`), `Future` as a class only for its class
side over the private `FutureImplementation` under `Proxy`, and the
`isKindOf…` rule. Every part of the spec not already written into a
design document by Tasks 1-6 goes into the section of the design
document for its aspect; check the spec section by section.

- [ ] **Step 2: `OPEN-WORK.md` and `PROGRESS.md`**

"Next": the foundations sub-project is built; the collections and
strings sub-project is next and has no plan yet. `PROGRESS.md`: an entry
for the foundations, in the form of the aligning entry.

- [ ] **Step 3: Delete the spec**

`git rm docs/superpowers/specs/2026-10-06-ns-next-kernel-foundations-design.md`
(the user's rule: a brainstormed spec is temporary, its content moves into
the living documents at the batch's end). Grep `docs/` for the spec's
path; nothing may link to it.

- [ ] **Step 4: Commit**

Only Markdown changed (check `git diff --stat`), so no gates. Commit:
`docs: the kernel's foundations sub-project is built`.

After this task the controller runs the end-of-batch gates
(`finishing-a-batch`).
