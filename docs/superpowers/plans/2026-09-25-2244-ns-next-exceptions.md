# Exceptions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Exceptions as Newspeak in `Kernel.ns` over a few activation
leaves — `Exception`, `Error`, `MessageNotUnderstood`, `Halt`, `Message`,
`on:do:`, `ensure:`, `ifCurtailed:`, ANSI semantics with the handler on top
of the stack — with every VM error signalled as an `Error`, one unwinder in
`interpreter/` for every cause, and every recorded outcome unchanged.

**Architecture:** Activations become kernel-private `Activation` objects
over a `FrameReference`; `on:do:`, `ensure:` and `ifCurtailed:` are kernel
methods whose compiled code carries a marker. Handler search, `return:`,
`retry`, `resume:` and `pass` are Newspeak; unwinding — for non-local
return, for those, and for termination — is one Rust unwinder that runs
pending `ensure:` blocks as trampoline steps and aims at a `FrameReference`.
A `NewspeakError` a step raises becomes an `Error` the run loop signals;
an uncaught one ends the run with the same `HostError` as today.

**Tech Stack:** Rust 1.98.0 (edition 2021), `ns-runtime`, `Kernel.ns`, the
image format, the golden harness (`crates/ns-cli/tests/run_goldens.rs`).

**Spec:** `docs/design/core-library.md`, "Planned: exceptions" (designed
with the user on 2026-09-25; commits `3ee83c4`, `28dd82f`, `1f5674b`), and
`docs/design/execution.md`, "Planned: tail calls, skipped but kept" (the
`on:do:` constraint). Semantics authority: `docs/references/newspeak-spec.md`
§8.6, §5.5, §7 (`cannotReturn`), §3.6.

## Decisions this plan implements (user, 2026-09-25)

1. **Newspeak over activation leaves**, as Primordial Soup
   (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
   `Exception`, `Activation`), not native as SOMns.
2. **Scope:** `Exception` (resumable), `Error` (not), `MessageNotUnderstood`
   (an `Error`, resumable), `Halt` (resumable, `Object>>halt`), `Message`
   (`sentTo:` a public send), `Activation` (kernel-private). Protocol:
   `signal`, `signal:`, `messageText`, `description`, `printString`,
   `isResumable`, `defaultAction`, `return:`/`return`, `retry`,
   `resume:`/`resume`, `pass`; class side `signal`, `signal:`, `handles:`;
   `on:do:`, `ensure:`, `ifCurtailed:`. Left out: `ExceptionSet`,
   `Notification`, `outer`, `retryUsing:`, `signalerContext`,
   `Object>>error:`, finer `Error` subclasses.
3. **`doesNotUnderstand:` is Squeak's**: signal a `MessageNotUnderstood`;
   if the default handler resumed it, re-send with `message sentTo: self`.
4. **Every VM error becomes a signalled `Error`** whose `messageText` is
   today's text; an uncaught one prints today's line.
5. **The depth limit keeps a reserve of 1,000**; overrunning it ends the run
   unsignalled.
6. **One unwinder in `interpreter/`**; `ensure:` blocks run as trampoline
   steps; an unwind target is a `FrameReference`.
7. **`Activation`** is `ObjectData::Activation(FrameReference)`, private to
   the kernel, reachable only through mirror leaves.
8. **`halt` terminates** in the CLI ("Uncaught Newspeak error: Halt").
9. **Programs name the classes through `platform kernel`**, a value of the
   kernel class `KernelNamespace`; `Object` gains no accessor.

## Global Constraints

- Every name written out (`CLAUDE.md`, "Names are written out"); clippy's
  `min_ident_chars` refuses one- and two-character names — in every code
  sample below and everything added.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()` an
  `EvaluationResult` or `HostResult` — use the test file's `expect_ok`.
- Inline Newspeak in a Rust test is an `indoc! {"…"}` / `formatdoc! {"…"}`.
- A golden's recorded outcome changes only after establishing the new one
  is right. **This plan expects no existing golden's outcome to change**; a
  change is stopped at and reported, not updated.
- `vm.rs` and `compiler/` gain no dispatch, lookup, unwinding decision or
  error text (`CLAUDE.md`, "`vm.rs` holds no dispatch logic"); the
  unwinder lives in `interpreter/unwind.rs`.
- The VM mirror stays a capability: every new leaf is a `MIRROR` row,
  appended at the end so no row index moves, reached only from `Kernel.ns`.
- A kernel mirror send passes a literal block only as its final `ifFail:`
  argument (`check_mirror_send_arguments`).
- An image is untrusted input; new tags take the next free numbers —
  builtin class tags `Activation` 23, `Exception` 24, `Error` 25,
  `MessageNotUnderstood` 26, `Message` 27, `Halt` 28, `KernelNamespace` 29;
  object body `OBJECT_ACTIVATION` 9 — each pinned in
  `crates/ns-runtime/tests/opcode_tags.rs`.
- Code, comments, commit messages in English; conventional commits; every
  commit ends with
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01SHrss2ZMLzsrEWuiacrzLH
  ```
  naming the model that actually wrote the commit (a subagent names its own).
- Per-commit gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets`
  (warning-free), `cargo test --workspace`,
  `cargo run -p ns-cli -- parse-corpus corpus/modules` ("parsed 210, failed 0,
  expected-fail 1, stale 0"). Stress suites and `run-corpus` once, in Task 8.
  A Markdown-only commit runs none of them.
- Every command runs in the foreground.
- Work on the branch `feature/exceptions`, in the main working tree.

## Review Focus

1. **A release build and a reused stack index.** An `ensure:` block running
   during an unwind pushes activations; the unwind must still land in its
   target, not in whatever activation reused the target's index. Pinned by a
   Rust test in Task 3 that the target is a `FrameReference` and a golden
   that returns through an `ensure:` whose block itself sends.
2. **The two lowerings count alike.** `on:do:`, `ensure:` and `ifCurtailed:`
   are ordinary kernel methods; a recursion through `on:do:` must reach the
   depth limit at the same point under both. Pinned by a golden in Task 6.
3. **An exception inside a handler searches further out**, and a handler
   for the same class below the running one does not catch it. Pinned by a
   golden in Task 4.
4. **`ensure:` runs exactly once** — on a normal end, on a `^`, on
   `return:`, on termination — never twice when a handler returns through
   it after it already ran. Pinned by goldens in Tasks 3 and 4.
5. **`sentTo:` never reaches a private or protected method.** Pinned by a
   golden in Task 5.

---

### Task 1: The classes, their tags and `platform kernel`

No behaviour beyond reaching the classes: `platform kernel Error` answers a
class; nothing signals yet.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass`: `Activation`, `Exception`, `Error`, `MessageNotUnderstood`, `Message`, `Halt`, `KernelNamespace`, appended to `ALL` in that order)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name`; `check_kernel_shape` lets `Exception`, `Error`, `MessageNotUnderstood`, `Message` and `Halt` declare slots and a class side — an ordinary Newspeak instance carries them — while `Activation` and `KernelNamespace` stay slotless)
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`platform_value` fills the new `kernel` slot)
- Modify: `crates/ns-runtime/src/image/encode.rs`, `decode.rs` (tags 23-29; `read_referenced_builtin_class` refuses `Activation`'s tag as it refuses `Top`'s)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Test: `goldens/exceptions/the_kernel_namespace_answers_the_exception_classes.ns` + `.toml`

**Interfaces:**
- Produces: the seven `BuiltinClass` variants; `platform kernel` with
  accessors `Exception`, `Error`, `MessageNotUnderstood`, `Halt`, `Message`.

- [ ] **Step 1: The failing golden**

```newspeak
Newspeak3
'Test'
(* Programs reach the exception classes through `platform kernel`, as the
   corpus does (`private Exception = platform kernel Exception.`) and as
   Primordial Soup's `Kernel wrapping:` facet provides them; `Object`
   declares no accessor for them. *)
class TheKernelNamespace usingPlatform: thePlatform = Object new (
    | transcript = thePlatform transcript.
      private kernel = thePlatform kernel. |
) (
    public main: platform args: arguments = (
        transcript show: kernel Exception printString; newLine.
        transcript show: kernel Error printString; newLine.
        transcript show: kernel MessageNotUnderstood printString; newLine.
        transcript show: kernel Halt printString; newLine.
        transcript show: kernel Message printString; newLine.
        transcript show: (kernel = thePlatform kernel) printString; newLine.
    )
) : ()
```

`.toml`: `stdout = '''\nException\nError\nMessageNotUnderstood\nHalt\nMessage\ntrue\n'''`
(the TOML shape of the neighbouring goldens). Pin the tags in
`opcode_tags.rs`: `builtin_class_tag(BuiltinClass::Activation) == 23` …
`KernelNamespace == 29`. Run `cargo test --workspace`: both fail.

- [ ] **Step 2: The declarations**

In `Kernel.ns`, after `Metaclass` (the order in the file is free; the fill
order is computed):

```newspeak
    (* §8.6: exception handling is a library issue, built here over the
       activation (docs/design/core-library.md, "Exceptions"). The slots are
       the exception's own state; `originSelector` and `originReceiver` are
       set by the VM for an error it raises, so a host still receives the
       selector and receiver it received before exceptions existed. *)
    public class Exception = Object new (
        | private signalActivation ::= nil.
          private handlerActivation ::= nil.
          private reachedDefault ::= false.
          private originSelector ::= nil.
          private originReceiver ::= nil.
          public messageText ::= nil. |
    ) (
    ) : (
    )
    public class Error = Exception new () (
    )
    public class MessageNotUnderstood = Error new (
        | public message ::= nil.
          public receiver ::= nil. |
    ) (
    )
    public class Halt = Exception new () (
    )
    public class Message = Object new (
        | public selector ::= nil.
          public arguments ::= nil. |
    ) (
    )
    (* A running activation as an object: the frame it wraps, and nothing a
       program can reach -- it reads other frames and unwinds stacks, a
       capability Newspeak hands out only through mirrors. *)
    private class Activation = Object new () (
    )
    (* `platform kernel`: the kernel classes a program may name, as
       Primordial Soup's `Kernel wrapping:` facet answers them. *)
    public class KernelNamespace = Value new () (
        public Exception = ( ^outer Kernel Exception )
        public Error = ( ^outer Kernel Error )
        public MessageNotUnderstood = ( ^outer Kernel MessageNotUnderstood )
        public Halt = ( ^outer Kernel Halt )
        public Message = ( ^outer Kernel Message )
    )
```

and `Platform` gains `public kernel = KernelNamespace new.` beside
`transcript` and `collections`. `platform_value` fills it with a
`KernelNamespace` instance the way it fills `collections`. The accessors of
`KernelNamespace` are recognized as `MethodFastPath::BuiltinClass` like
`Object`'s, if `compiler::primitive_method`'s recognizer takes any class's
`^outer Kernel X` — check, and extend it to `KernelNamespace` if it is
restricted to `Object`.

- [ ] **Step 3: Tags and refusal**

`builtin_class_tag`/`builtin_class_from_tag` get 23-29.
`read_referenced_builtin_class` refuses `Activation` (tag 23) with the
message it uses for `Top`. `read_program_builtin_class` (which already adds
refusals) needs nothing more: its refusals include the referenced ones.

- [ ] **Step 4: Run and commit**

Run: `cargo test --workspace` — Expected: PASS, no other golden changed.

```bash
git add -A crates goldens
git commit -m "feat: declare the exception classes and platform kernel"
```

---

### Task 2: Activation objects

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`ObjectData::Activation(FrameReference<'gc>)`)
- Modify: `crates/ns-runtime/src/trace.rs` (trace the frame reference)
- Modify: `crates/ns-runtime/src/heap.rs` (`describe` answers "an Activation"; accessor `activation_object_frame(object) -> Option<FrameReference>`; `Mutator::new_activation_object(frame)` in `interpreter/mod.rs`, allocating with class `BuiltinClass::Activation`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`, `inspect.rs` (`OBJECT_ACTIVATION = 9`: body is a frame id)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/src/heap.rs` unit test

**Interfaces:**
- Produces: `ObjectData::Activation(FrameReference<'gc>)`;
  `Heap::activation_object_frame(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>>`;
  `Mutator::new_activation_object(&mut self, frame: FrameReference<'gc>) -> ObjectReference<'gc>`.

- [ ] **Step 1: Failing tests**

`opcode_tags.rs`: `assert_eq!(OBJECT_ACTIVATION, 9)` and add it to its
family's uniqueness check. `tests/image.rs`:
`an_activation_object_round_trips` — reify a frame through a host send into
a module whose method stores something the test can reach (copy how the
neighbouring frame tests build a reified frame), make an activation object
over it with `new_activation_object`, store it in an instance slot through
`Heap::set_instance_field`, snapshot, load, and assert the loaded slot is an
activation object whose frame is the loaded counterpart of the original
(compare the loaded frame's receiver). `an_activation_record_naming_no_frame_is_refused`
— an `OBJECT_ACTIVATION` body with an out-of-range frame id, built with
`encode::Writer` as the neighbouring crafted tests do → `Err`, no panic.

- [ ] **Step 2: Implement**

The variant, its tracing (`trace_frame(frame, tracer)` as a closure's
`captured_frame` is traced), `describe`, the accessor, the constructor; the
encoder writes `OBJECT_ACTIVATION` then the frame id
(`encode_frame_reference`), the graph walk pushes `Pending::Frame`; the
decoder reads it through `decode_frame_id`; `inspect` prints
"activation of frame N". Every `match` on `ObjectData` the compiler names
gets its arm (a collector, `Heap::object_snapshot`, `describe`…).

- [ ] **Step 3: Run and commit**

`cargo test --workspace` PASS; also `NS_NEXT_STRESS_MOVES=1 cargo test -p ns-runtime --test image` (Git Bash).

```bash
git add -A crates
git commit -m "feat: activations as kernel-private objects"
```

---

### Task 3: Markers and the one unwinder

`ensure:` and `ifCurtailed:` work for a `^` from a block; `on:do:` exists
and is `^self value`. No exception signals yet.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/unwind.rs`
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CompiledCode.unwind_marker: Option<UnwindMarker>`)
- Modify: `crates/ns-runtime/src/compiler/primitive_method.rs` or a sibling `compiler/unwind_marker.rs` (recognition, kernel only)
- Modify: `crates/ns-runtime/src/vm.rs` (`unwind` becomes a call into `interpreter::unwind`; the loop only acts on the `Flow` it answers)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Signal::Return(FrameReference<'gc>, Value<'gc>)`; `raise_return` reifies the target; `Continuation::Unwind`)
- Modify: `crates/ns-runtime/src/types.rs` (`Continuation::Unwind(Box<PendingUnwind<'gc>>)`, its `Trace`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Block>>on:do:`, `ensure:`, `ifCurtailed:`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`NOT_FORWARDERS`: the three)
- Test: goldens under `goldens/exceptions/`; `crates/ns-runtime/src/interpreter/unwind.rs` unit tests

**Interfaces:**
- Produces:
  ```rust
  // bytecode.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum UnwindMarker {
      /// `on:do:`: the registers holding the exception class, the handler
      /// block and the `handlerActive` local.
      Handler { class_register: u16, block_register: u16, active_register: u16 },
      /// `ensure:`: the block to run and the `complete` local that says it ran.
      Ensure { block_register: u16, complete_register: u16 },
      /// `ifCurtailed:`: the block, run only on an unwind, and its `complete` local.
      IfCurtailed { block_register: u16, complete_register: u16 },
  }
  // interpreter/unwind.rs
  pub(crate) enum UnwindAction<'gc> { Return(Value<'gc>), Restart, Terminate(NewspeakError<'gc>) }
  pub(crate) struct PendingUnwind<'gc> { pub target: Option<FrameReference<'gc>>, pub action: UnwindAction<'gc> }
  pub(crate) fn unwind<'gc>(mutator: &mut Mutator<'gc>, pending: PendingUnwind<'gc>, floor: usize) -> UnwindOutcome<'gc>;
  pub(crate) enum UnwindOutcome<'gc> {
      /// Popped to the target; it returns this value (`Return`) or restarts.
      Arrived { target: ActivationReference, action: UnwindAction<'gc> },
      /// An `ensure:`/`ifCurtailed:` block to run first; the unwind resumes
      /// when it returns (`Continuation::Unwind`).
      RunBlock { block: ObjectReference<'gc>, then: Box<PendingUnwind<'gc>> },
      /// Popped to the floor: the run ends with this.
      Exit(EvaluationResult<'gc>),
  }
  ```
  (`target: None` means the floor — termination.) Names may be adjusted to
  the code as long as the three outcomes stay distinct.

- [ ] **Step 1: The kernel methods**

In `Block`:

```newspeak
        (* ANSI's handler: the marker on this method's code tells `signal`
           this activation handles `exceptionClass` with `handler`, while
           `handlerActive` is true (docs/design/core-library.md,
           "Exceptions"). The body is a tail send that tail-call elimination
           must never skip (docs/design/execution.md, "Planned: tail calls"). *)
        public on: exceptionClass do: handler = (
            | handlerActive ::= true. |
            ^self value
        )
        (* On a normal end the block runs here; on an unwind the unwinder
           runs it, having set `complete` first, so it runs exactly once. *)
        public ensure: onUnwind = (
            | complete ::= nil. result |
            result:: self value.
            complete isNil ifTrue: [complete:: true. onUnwind value].
            ^result
        )
        (* Runs `onUnwind` only when an unwind abandons the block. *)
        public ifCurtailed: onUnwind = (
            | complete ::= nil. |
            ^self value
        )
```

- [ ] **Step 2: The failing goldens**

`goldens/exceptions/ensure_runs_on_a_normal_end_and_on_a_return.ns`: a
method `probe` whose body is `[^1] ensure: [transcript show: 'ensured'; newLine]`,
another whose body is `^[2] ensure: [...]`, a third where the protected
block sends a message that itself returns non-locally through a second
`ensure:` (Review Focus 1: the ensure block of the inner one sends
`printString` and `show:`, pushing activations while the unwind is in
progress). Print each result after it. Expected: each `ensured` line once,
before the result. `goldens/exceptions/if_curtailed_runs_only_on_an_unwind.ns`:
`[3] ifCurtailed: [...]` prints nothing extra; `[^4] ifCurtailed: [...]`
prints the curtailed line once. Write both files and their `.toml`s with the
exact expected output. Run: FAIL (the blocks never run on the `^`).

- [ ] **Step 3: Markers**

Recognize, only when compiling the kernel (`kernel == true`, where
`recognize` is called), the three methods by selector on `Block` —
`on:do:`, `ensure:`, `ifCurtailed:` — and record the registers of their
parameters and named locals from the code the compiler just produced (the
layout is `argument_count`, then copied, then named temporaries in
declaration order: `docs/design/compiler.md`, "Registers"). Store the
marker on `CompiledCode`. Add a test beside `QUICK_METHODS` in
`interpreter/kernel.rs` asserting the three markers and nothing else in the
kernel carries one.

- [ ] **Step 4: The unwinder**

Move `vm::unwind`'s logic into `interpreter/unwind.rs::unwind`, keeping its
doc comment's substance. `Signal::Return` carries the home's
`FrameReference` (`raise_return`: a block's home frame as it already is; any
other activation is reified with `Heap::reify` first). The unwinder, for a
`Return` or `Restart` whose target frame is married (`Heap::frame_activation`)
to a live activation at or above the floor that catches a return, and for a
`Terminate` down to the floor, pops from the top; before popping an
activation whose code carries `Ensure` or `IfCurtailed` with `complete`
still `nil` (read its register through the activation), it sets `complete`
to `true` and answers `RunBlock` with that activation's block — the popping
stops there, the marked activation stays on the stack under its block. The
loop pushes the block as a `Step::Block` whose `then` is
`Continuation::Unwind(pending)`; `Mutator::resume` for that continuation
calls `unwind` again with the same pending unwind. Every popped
activation's continuation goes to `abandon`; `abandon` of a
`Continuation::Unwind` drops it (a newer unwind replaced it). A target
that is not live (the known initializer gap) behaves as today. `vm.rs`
keeps only the translation of the outcome into its `Flow`.

- [ ] **Step 5: Unit test for the target**

In `unwind.rs`: `a_target_survives_its_index_being_reused` — build a stack
where the target frame is reified, pop it, push another activation at the
same depth, and assert `unwind` does not answer `Arrived` for the stale
frame (it is not married, so the unwind goes on to the floor).

- [ ] **Step 6: Run and commit**

`cargo test --workspace` PASS (both new goldens, both lowerings; no other
golden changes). `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` once.

```bash
git add -A crates goldens
git commit -m "feat: one unwinder that runs ensure: and ifCurtailed: blocks"
```

---

### Task 4: Signalling and handling

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (the `ActivationPrimitive` rows, appended)
- Create: `crates/ns-runtime/src/builtins/activation.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Signal::Restart(FrameReference<'gc>)`, `Signal::Terminate(NewspeakError<'gc>)`, both routed to the unwinder)
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`NOT_FORWARDERS`)
- Test: goldens under `goldens/exceptions/`

**Interfaces:**
- Produces the mirror rows (subject first, failure block last):

| mirror selector | reported | answers |
|---|---|---|
| `currentActivation:ifFail:` | `currentActivation` | the `Activation` of the top activation — the kernel method making the mirror send; never call it from a forwarder |
| `activationSender:ifFail:` | `sender` | the sender's `Activation`, or `nil` |
| `activationIsActiveHandler:ifFail:` | `isActiveHandler` | `true` for an `on:do:` activation whose `handlerActive` is `true` |
| `activationHandlerClass:ifFail:` | `handlerClass` | its exception class argument |
| `activationHandlerBlock:ifFail:` | `handlerBlock` | its handler block argument |
| `activation:handlerActive:ifFail:` | `handlerActive:` | sets the local, answers the activation |
| `activation:returnValue:ifFail:` | `returnValue:` | `Err(Signal::Return(frame, value))` |
| `activationRestart:ifFail:` | `restart` | `Err(Signal::Restart(frame))` |
| `object:inheritsMixinOf:ifFail:` | `inheritsMixinOf:` | whether the object's class chain applies the argument class's mixin (Primordial Soup's `is:interestedIn:` compares mixins, so a nested exception class matches under any enclosing instance) |
| `terminate:selector:receiver:description:ifFail:` | `terminate` | `Err(Signal::Terminate(NewspeakError { selector, receiver, message: description }))` |

  Each declines (`None`) for a subject of the wrong shape; a dead
  activation's `returnValue:`/`restart` answers `Err(Signal::Error)` with
  "cannotReturn: the activation has already returned".

- [ ] **Step 1: The protocol in `Kernel.ns`**

`Exception`'s instance side (class side follows):

```newspeak
        public description = ( ^messageText isNil ifTrue: [self class name] ifFalse: [messageText] )
        public printString = ( ^self description )
        public isResumable = ( ^true )
        public reachedDefaultHandler = ( ^reachedDefault )
        public signal = (
            signalActivation:: vmMirror currentActivation: self ifFail: [nil].
            handlerActivation:: signalActivation.
            ^self invokeNextHandler
        )
        public signal: text = (
            messageText:: text.
            ^self signal
        )
        public pass = (
            handlerActivation isNil ifTrue: [^self notInAHandler: #pass].
            ^self invokeNextHandler
        )
        public return: value = (
            | target |
            target:: handlerActivation.
            (target isNil or: [target == signalActivation]) ifTrue: [^self notInAHandler: #return:].
            signalActivation:: nil.
            handlerActivation:: nil.
            ^vmMirror activation: target returnValue: value ifFail: [nil]
        )
        public return = ( ^self return: nil )
        public retry = (
            | target |
            target:: handlerActivation.
            (target isNil or: [target == signalActivation]) ifTrue: [^self notInAHandler: #retry].
            signalActivation:: nil.
            handlerActivation:: nil.
            ^vmMirror activationRestart: target ifFail: [nil]
        )
        public resume: value = (
            | target |
            self isResumable ifFalse: [^(outer Kernel Error new) signal: 'cannot resume a non-resumable exception'].
            target:: signalActivation.
            target isNil ifTrue: [^self notInAHandler: #resume:].
            signalActivation:: nil.
            handlerActivation:: nil.
            ^vmMirror activation: target returnValue: value ifFail: [nil]
        )
        public resume = ( ^self resume: nil )
        public defaultAction = (
            | selector receiver |
            selector:: self hostSelector.
            receiver:: self hostReceiver.
            ^vmMirror terminate: self selector: selector receiver: receiver description: self description ifFail: [nil]
        )
        protected hostSelector = ( ^originSelector isNil ifTrue: [#signal] ifFalse: [originSelector] )
        protected hostReceiver = ( ^originReceiver isNil ifTrue: [self] ifFalse: [originReceiver] )
        private invokeNextHandler = (
            | activation |
            activation:: vmMirror activationSender: handlerActivation ifFail: [nil].
            [activation isNil] whileFalse: [
                (self isHandledAt: activation) ifTrue: [^self invokeHandlerAt: activation].
                activation:: vmMirror activationSender: activation ifFail: [nil]].
            reachedDefault:: true.
            handlerActivation:: nil.
            ^self defaultAction
        )
        private isHandledAt: activation = (
            | exceptionClass |
            (vmMirror activationIsActiveHandler: activation ifFail: [false]) ifFalse: [^false].
            exceptionClass:: vmMirror activationHandlerClass: activation ifFail: [nil].
            ^exceptionClass handles: self
        )
        private invokeHandlerAt: activation = (
            | handler result |
            handlerActivation:: activation.
            handler:: vmMirror activationHandlerBlock: activation ifFail: [nil].
            vmMirror activation: activation handlerActive: false ifFail: [nil].
            result:: handler cull: self.
            vmMirror activation: activation handlerActive: true ifFail: [nil].
            ^self return: result
        )
        private notInAHandler: selector = (
            ^(outer Kernel Error new) signal: selector asString, ' sent to an exception that is not being handled'
        )
```

(If the kernel has no `or:` fast path for a non-literal receiver, write the
two checks as nested `ifTrue:`; the recognizer and `Kernel.ns`'s shape
check decide which forms are allowed — adapt, don't widen them.) The class
side:

```newspeak
    ) : (
        public signal = ( ^self new signal )
        public signal: text = ( ^self new signal: text )
        public handles: exception = ( ^vmMirror object: exception inheritsMixinOf: self ifFail: [false] )
    )
```

`Error`: `public isResumable = ( ^false )`. `Halt` and `Object>>halt =
( ^outer Kernel Halt new signal )` — `halt` beside the nil protocol, with a
comment that it is the hook the debugger will replace through
`defaultAction`. Every method that is not a forwarder goes into
`NOT_FORWARDERS` with a one-line reason.

- [ ] **Step 2: The failing goldens** (each a module with
`private Error = platform kernel Error.` etc. in its slots, written with the
exact expected `.toml`):

- `on_do_answers_the_handlers_value` — `[Error signal: 'x'] on: Error do: [:e | e messageText]` prints `x`.
- `return_retry_resume_pass` — `return: 5` answers 5; `retry` with a counter slot runs the block twice then succeeds; `(kernel Exception signal) ` resumed with `e resume: 7` answers 7 from `signal`; `e resume: 7` on an `Error` ends in "Uncaught Newspeak error: cannot resume a non-resumable exception" — put that last, since it ends the run (status 1).
- `pass_reaches_the_outer_handler` — inner `on: Error do: [:e | e pass]` inside outer `on: Error do: [:e | 'outer']` answers `outer`.
- `a_subclass_overrides_handles` — a user `class Picky = Error new ()() : ( public handles: exception = ( ^exception messageText = 'mine' ) )`; `on: Picky do:` catches `Error signal: 'mine'` but not `'other'` (the second one uncaught, last, status 1).
- `an_exception_in_a_handler_searches_further_out` (Review Focus 3) — inner handler signals a second `Error`; the same inner `on: Error do:` does not catch it, the outer one does.
- `ensure_runs_once_through_a_handler_return` (Review Focus 4) — `[[Error signal] ensure: [show 'ensured']] on: Error do: [:e | e return: 1]` prints `ensured` once, then 1.
- `a_return_from_a_handler_block` — a method whose handler is `[:e | ^'returned']` answers `returned`, running an `ensure:` on the way.
- `an_uncaught_error_runs_pending_ensures` — `[Error signal: 'boom'] ensure: [show 'cleanup']`: stderr `Uncaught Newspeak error: boom`, stdout `cleanup`, status 1.
- `an_exception_in_an_ensure_block_during_an_unwind` — an unwind (`^` from a block) runs an `ensure:` block that signals an `Error` caught inside that block; the unwind then completes.
- `halt_resumes_when_handled_and_ends_the_run_when_not` — `[self halt. 3] on: Halt do: [:e | e resume: 7]` answers 3 (execution continues after `halt`); then an unhandled `halt`: stderr `Uncaught Newspeak error: Halt`, status 1.

Run: FAIL.

- [ ] **Step 3: The leaves and signals**

`builtins/activation.rs` with `ActivationPrimitive` per row above;
`currentActivation` reifies `mutator.heap.top_activation()` and wraps it
with `new_activation_object`. `Signal::Restart` and `Signal::Terminate` go
to the unwinder as `UnwindAction::Restart` and `UnwindAction::Terminate`;
`Arrived` with `Restart` sets the target's program counter to 0 and
switches to it (its registers stay; `on:do:`'s `handlerActive` initializer
runs again, resetting it). `Terminate` unwinds to the floor and exits with
`Err(Signal::Error(error))` so `to_host` builds the same `HostError`.
`to_host` and every `match` on `Signal` get the two variants.

- [ ] **Step 4: Run and commit**

`cargo test --workspace` PASS; no existing golden changes.

```bash
git add -A crates goldens
git commit -m "feat: signal, on:do:, return:, retry, resume: and pass in the kernel"
```

---

### Task 5: VM errors, `doesNotUnderstand:` and `Message`

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/unwind.rs` or a sibling `interpreter/signal.rs` (turning `Signal::Error` into a signalled `Error`)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs:580` (`handler_message` builds a `Message`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (row `message:sendTo:ifFail:`, reported `sentTo:`; remove the row `object:doesNotUnderstand:ifFail:` and `ObjectPrimitive::DoesNotUnderstand` with `object_class.rs`'s arm for it — a row index is a property of one build, never serialized, since `SendMirror` has no wire tag, so removing a row renumbers nothing that outlives the binary)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Object>>doesNotUnderstand:`, `Message>>sentTo:`, `MessageNotUnderstood`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (a test hook, below)
- Test: goldens; `crates/ns-runtime/tests/exceptions.rs` (new)

**Interfaces:**
- Consumes: Task 4's protocol.
- Produces: `Mutator::signal_vm_error(&mut self, error: NewspeakError<'gc>) -> Result<Step<'gc>, Signal<'gc>>`;
  `Interpreter::resume_unhandled_exceptions(&mut self, count: usize)` (a
  documented test hook: the next `count` unhandled exceptions are resumed
  with `nil` by `terminate` instead of ending the run — what the debugger
  will do; `#[doc(hidden)]`, like `set_global_cache_size`).

- [ ] **Step 1: The failing goldens and test**

- `an_array_index_error_is_caught` — `[#(1 2) at: 5] on: Error do: [:e | e messageText]` prints today's text (copy it from the existing error golden that pins `at:` out of range).
- `a_message_not_understood_is_resumed` — `[3 zork] on: MessageNotUnderstood do: [:e | e resume: 42]` answers 42; `e message selector` prints `zork`; `e receiver` prints 3.
- `a_does_not_understand_is_caught_as_an_error` — `on: Error do:` catches `nil zork`.
- `a_cannot_return_is_caught` — a block whose home returned, `^`-ing inside `on: Error do:`, prints the `cannotReturn` text.
- `sent_to_is_a_public_send` (Review Focus 5) — `Message new selector: #secret; arguments: {}` `sentTo:` an object with a private `secret` ends in `doesNotUnderstand:` (caught, prints the text); a public one answers.

`tests/exceptions.rs`: `an_unhandled_message_not_understood_is_resent_when_resumed` —
a module with a class whose `probe` sends `self absent`; call
`interpreter.resume_unhandled_exceptions(1)`; the first `doesNotUnderstand:`
reaches the default handler, which resumes, so `doesNotUnderstand:`
re-sends `absent`, which fails again and, with the hook spent, ends the run
with "an object does not understand #absent" — assert the host error and
that the hook's count reached zero (so the re-send happened).

Run: FAIL.

- [ ] **Step 2: The kernel side**

```newspeak
        (* §5.5: Object's default `doesNotUnderstand:` throws a
           `MessageNotUnderstood`. As in Squeak, if the default handler
           resumed it -- the debugger, once the missing method has been
           created -- the message is sent again, into the new method. *)
        protected doesNotUnderstand: message = (
            | exception value |
            exception:: outer Kernel MessageNotUnderstood new.
            exception message: message.
            exception receiver: self.
            exception messageText: 'an object does not understand #', message selector asString.
            value:: exception signal.
            exception reachedDefaultHandler ifTrue: [^message sentTo: self].
            ^value
        )
```

`MessageNotUnderstood`: `public isResumable = ( ^true )`,
`protected hostSelector = ( ^message selector )`,
`protected hostReceiver = ( ^receiver )`. `Message`:
`public sentTo: receiver = ( ^vmMirror message: self sendTo: receiver ifFail: [nil] )`.
The leaf decides the send with `decide_send` (a public send, §5.5) and
answers its `Step`.

- [ ] **Step 3: The VM side**

Where the run loop receives `Err(Signal::Error(error))` from a step — the
same place `unwind` is entered today — and the kernel is installed,
`signal_vm_error` builds an `Error` (fields filled by name through its
application, as `platform_value` does: `messageText` a new string of
`error.message`, `originSelector` the symbol, `originReceiver` the
receiver), and answers the `Step::Method` of its `signal` (looked up with
`resolve_public`), pushed above the activation where the error arose with a
`Continuation::VmErrorSignalled(error)` whose `resume` answers
`Err(Signal::Terminate(error))` — an `Error` is not resumable, so `signal`
returning there means something went wrong, and the run ends as it would
have. `handler_message` builds a `Message` with `selector` and `arguments`.
`raise_return`'s dead-home case keeps its text and becomes signalled through
the same path. Until the kernel is installed, errors stay host errors.

- [ ] **Step 4: Run and commit**

`cargo test --workspace` PASS; **no existing golden changes** — every error
golden prints the same line, since `defaultAction` reports the same
selector, receiver and text.

```bash
git add -A crates goldens
git commit -m "feat: VM errors and doesNotUnderstand: signal exceptions"
```

---

### Task 6: The depth limit's reserve

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`push`, `recursion_too_deep`, a `depth_reserve_in_use` field)
- Test: goldens under `goldens/exceptions/` (and the existing `goldens/limits/` unchanged)

- [ ] **Step 1: The failing goldens**

- `the_depth_limit_is_caught` — `[self recurse] on: Error do: [:e | e messageText]` prints `stack overflow: recursion too deep`; then a second, independent recursion caught the same way (the reserve was restored).
- `a_handler_overrunning_the_reserve_ends_the_run` — a handler that recurses itself: stderr `Uncaught Newspeak error: stack overflow: recursion too deep`, status 1.
- `recursion_through_on_do_counts_alike` (Review Focus 2) — a recursion whose every level goes through `on:do:` prints the same progress under both lowerings (the harness checks both).

Run: FAIL.

- [ ] **Step 2: Implement**

`push` refuses at `MAX_ACTIVATION_DEPTH` while the reserve is unused, and
at `MAX_ACTIVATION_DEPTH + DEPTH_RESERVE` (a new constant, 1,000, with a
doc comment citing Python's `RecursionError` and the JVM's yellow zone)
while it is in use. The first refusal sets `depth_reserve_in_use` and
answers the signalled `Error` as every VM error; a refusal while it is in
use answers `Err(Signal::Terminate(...))` with the same text. `push`
clears the flag whenever the counted depth is below `MAX_ACTIVATION_DEPTH`.
Both lowerings count as before.

- [ ] **Step 3: Run and commit**

`cargo test --workspace` PASS; `goldens/limits/` unchanged.

```bash
git add -A crates goldens
git commit -m "feat: a reserve above the depth limit for signalling it"
```

---

### Task 7: Documentation

Markdown and `CLAUDE.md` only; no test run.

- `docs/design/core-library.md`: "Planned: exceptions" becomes "Exceptions",
  describing what exists (heading and status line changed, content the
  current state); "Decisions" gains one entry for the nine decisions,
  linking this plan.
- `docs/design/execution.md`: "Non-local return" becomes "Unwinding" — the
  one unwinder, `FrameReference` targets, `ensure:` as trampoline steps,
  the `Continuation::Unwind`; the depth limit's reserve; remove the hazard
  paragraph's future tense.
- `docs/design/image-format.md`: tags 23-29, `OBJECT_ACTIVATION`; the
  unwind continuation on "Planned: suspend and resume"'s list.
- `docs/design/dispatch.md`: `doesNotUnderstand:` signals and re-sends.
- `docs/design/README.md`: the aspect table's builtins row.
- `CLAUDE.md`: the crate description names the exception classes; a new
  invariant paragraph — "An unwind target is a `FrameReference`" — with the
  reason (a release build does not check an activation's generation).
- `docs/PROGRESS.md`: an entry "Exceptions" (2026-09-25) in the style of its
  neighbours, with a marked placeholder for Task 8's numbers; "Where the
  next session starts" names the platform and collections modules next.
- Sweep: `grep -rn "object:doesNotUnderstand:\|Signal::Return(ActivationReference\|vm::unwind" docs CLAUDE.md` — nothing describing the old state as current.

```bash
git add docs CLAUDE.md
git commit -m "docs: record exceptions as built"
```

---

### Task 8: Staggered gates and measurement

- [ ] **Step 1: The end-of-batch gates** (Git Bash, foreground each)

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo build --workspace
```
Expected: all pass; each `run-corpus` "ran 6, failed 0, skipped 205".

- [ ] **Step 2: Measure against `master`** by `docs/design/execution.md`'s
protocol (pinned `RUSTFLAGS`, several builds per tree pooled, the `master`
binaries built in a throwaway worktree under the scratchpad, `bench <source>
--iterations 30`, rotating): every fixture under `goldens/bench/`, expected
level; and two throwaway sources in the scratchpad:

`bench_ensure.ns` — a loop of 100,000 iterations, each `[count:: count + 1] ensure: [total:: total + 1]` and `[count:: count + 1] on: Error do: [:e | 0]`.
`bench_signal.ns` — a loop of 10,000 iterations, each signalling an `Error` twenty activations down and catching it at the top.

Report every slowdown beyond noise with numbers, not optimized away here.
Check with `dumpbin /disasm` that no `vm::run::closure` became a function of
its own.

- [ ] **Step 3: Record** the numbers in `PROGRESS.md`'s entry and
`execution.md`, replacing the placeholder:

```bash
git add docs
git commit -m "docs: record what exceptions cost"
```
