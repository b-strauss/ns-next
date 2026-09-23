# The Core Library over a VM Mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every builtin method out of Rust method tables into Newspeak
source, `Kernel.ns`, whose methods reach the Rust leaves only through one VM
mirror object, without changing a single recorded program outcome.

**Architecture:** A new builtin class, `VmMirror`, holds every leaf as an
enumerated primitive in its method table; its one instance is created at
bootstrap and handed to `Kernel vmMirror:`, and nothing else can obtain it.
The bootstrap compiles `Kernel.ns`, fills the existing builtin class objects
(the placeholders) from its nested classes, and instantiates it. Every kernel
method is a one-line forwarder, `^vmMirror <leaf>: self … ifFail: [^super …]`.
The compiler inlines the literal `ifFail:` block of such a send
(`Instruction::SendMirror`), and a send to a method of exactly that shape runs
its primitive without activating the method, activating it only when the
primitive fails — Cog's primitive methods, recognized from the body instead of
declared by a pragma.

**Tech Stack:** Rust 1.98, edition 2021, no new dependencies. Newspeak3 for
`Kernel.ns`.

**Spec:** `docs/design/core-library.md`, "Planned: the core library as Newspeak
source over a VM mirror" (all five subsections), and "Gap: an integer has no
`/`". Background: `docs/design/dispatch.md`, "Primitives" and "A primitive can
fail"; `docs/design/compiler.md`, "Control-flow inlining", "Arithmetic
inlining" and "How the limit counts"; `docs/design/object-model.md`, "Class
sides and metaclasses".

## Decisions this plan implements

Each was decided with the user on the date given. They are settled; do not
reopen them in passing.

1. **A mirror primitive's failure path is an explicit `ifFail:` block, inlined
   by the compiler** (2026-09-21). A kernel method is
   `^vmMirror stringSize: self ifFail: [^super size]`: a real send to the
   mirror, as §8.3.1 requires, whose failure path is visible Newspeak code, as
   Cog's method body is. SOMns already uses the same shape for its I/O
   primitives (`prior-art/somns/core-lib/Files.ns`,
   `vmMirror file: self readAt: pos ifFail: errf`).
2. **`Integer>>/` stays absent**, and the gap stays recorded as something that
   has to be solved eventually (2026-09-21). `core-library.md`'s "Gap: an
   integer has no `/`" is updated to say step 6 left it open deliberately.
3. **Every builtin family moves, one task per family** (2026-09-21), including
   `do:`, `whileTrue:` and `value`.
4. **The arithmetic fast path's `Jump` saving stays a separate step**
   (2026-09-21). Nothing here touches `IntegerBinary` or wire tag 33.
5. **Primitive methods** (2026-09-22): a send to a kernel method whose whole
   body is a mirror send forwarding `self` and the method's own parameters in
   order runs the primitive without an activation, and activates the method
   only when the primitive fails. Recognized **only while compiling
   `Kernel.ns`**. It applies **under both lowerings**, because it is dispatch
   (`act`), not lowering — and because the depth count's agreement between the
   lowerings depends on it (see "Why the fast path comes before the control-flow
   family" below).
6. **The fast path is built early, with a measurement-only switch**
   (2026-09-22): `NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without
   the marks, so the final measurement can run the same head twice — with and
   without the fast path — and report both what the move costs and what the
   fast path recovers.
7. **Kernel method bodies forward; they do not reimplement** (2026-09-22). The
   control-flow methods and `do:` are forwarders over today's natives, as
   mirror primitives. Rewriting `do:`, `whileTrue:`, `ifTrue:` and the others
   as real Newspeak loops and branches is **a separate step that must happen
   before the JIT tiers** — only a JIT gains from it, and before one it only
   costs. That step owns four semantic decisions, recorded in Task 16: a
   non-block argument taken as its own value (`true and: three`, pinned by
   `goldens/language/variable_and_block_coerces_its_value_to_a_boolean`),
   `do:` iterating over a copy (`Heap::array_elements` clones the payload),
   `do:` with a non-block argument being a no-op, and `whileTrue:` with a
   non-block body re-running the condition.
8. **`True` and `False` become subclasses of `Boolean`** (2026-09-22), as both
   references declare them. Today all three sit directly under `Object` and
   each carries the same primitives.
9. **A builtin subclass's own `doesNotUnderstand:` is passed by, per §5.10**
   (2026-09-22). A consequence of Decision 1: when a leaf declines on an
   instance of a user subclass of a builtin, the failure block's `^super …`
   reaches `Object`, and a selector `Object` does not have ends in "the
   `#doesNotUnderstand:` method defined for S" (§5.10) — `Object`'s — not in a
   handler the subclass declares. Today the pass-over continued the
   receiver's own lookup and reached that handler. No golden pinned the old
   behaviour, Newspeak has no send that continues a public lookup above a
   given level, and only the rejected implicit pass-over could keep it. It is
   the one observable difference this plan makes; Task 13 pins it and
   Task 16 records it.

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency.
- **All code, identifiers, comments, doc comments and commit messages in
  English.** Classic terminology: `whitelist`, `blacklist`, `sane`.
- **Names are written out.** No identifier of one or two characters and no
  truncated word: `mutator`, not `m`; `arguments`, not `args`;
  `instruction`, not `instr`; `destination`, not `dest`. Clippy's
  `min_ident_chars` (threshold 2, inclusive) enforces it.
- **Program outcomes are pinned.** Every golden under `goldens/<category>/*.ns`
  keeps its recorded `status`, `stdout` and `stderr` byte for byte, under both
  lowerings, and `run-corpus` keeps its report. **No task in this plan changes
  a golden's recorded output.** If one changes, the task is wrong; stop and
  find out why rather than re-recording it.
- **A Rust test that pins the *mechanism* may be rewritten; a test that pins
  an *outcome* may not.** Moving a family changes which method-table entry a
  send finds: `3 + 4` finds `Integer`'s kernel method rather than a
  `MethodEntry::Primitive` on `Integer`, and a cache records
  `CachedTargetKind::Method` where it recorded `CachedTargetKind::Primitive`.
  A test asserting that shape is rewritten to assert the new shape, with the
  same program and the same answer. A test asserting a value, an error text or
  an exit status is never edited to pass.
- **`vm.rs` holds no dispatch logic and no error text.** `SendMirror`'s arm
  reads registers, calls `interpreter::mirror`, and jumps or pushes what it was
  handed.
- **Compiling an already-resolved module can never raise a `NewspeakError`.**
  A kernel source defect the recognizers do not accept compiles as an ordinary
  send; a test (Task 2) checks every mirror send in `Kernel.ns` is recognized.
- **An image is untrusted input.** The decoder refuses the VM mirror's class
  tag everywhere a program could name a class (Task 1).
- **A wire tag is assigned once and never renumbered or reused.** The only new
  tag is builtin class tag **14** for `VmMirror`. `SendMirror` gets **no**
  opcode: it is emitted only into `Kernel.ns`, which no image carries.
- **`BuiltinClass::ALL` is appended to, never inserted into** — its order is
  also the wire-tag order.
- **The gates, for every task that changes Rust or Newspeak:**
  `cargo fmt --all`; `cargo clippy --workspace --all-targets` warning-free;
  `cargo test --workspace`; the same suite under `NS_NEXT_STRESS_MOVES=1`,
  `NS_NEXT_STRESS_CACHES=1` and `NS_NEXT_STRESS_COLLECT=1` (Git Bash syntax,
  debug builds); `cargo run -p ns-cli -- parse-corpus corpus/modules`
  reporting "parsed 210, failed 0, expected-fail 1, stale 0";
  `cargo run -p ns-cli -- run-corpus corpus/modules` reporting
  "ran 6, failed 0, skipped 205", also under each of the three stress modes.
  A task that changes only Markdown runs none of them.
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**
  (`formatdoc!` for a template), indented with the Rust around it, the closing
  `"}` on a line of its own.
- **Never `{:?}` a `Value`, and never `.unwrap()`/`.expect()` an
  `EvaluationResult` or a `HostResult`.** Tests unwrap through a local
  `expect_ok` helper copied from a neighbouring test file.
- **Commits are conventional** (`feat:`, `fix:`, `perf:`, `test:`, `docs:`,
  `chore:`) and end with the trailer naming the model that actually wrote the
  commit, followed by the session line:

  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01XZBjhUJaKHo3p6VsiUwHKx
  ```

  A subagent running on another model replaces the first line with its own
  model's name.
- **Subagents run every command in the foreground** and do not hand back while
  background work of their own is still running.

---

## How the pieces fit

### The mirror, its table, and the three ways a mirror send runs

`builtins/primitive.rs` gains one table, `MIRROR`: one row per leaf, each row
naming the **mirror selector** (`collection:at:put:ifFail:`), the **reported
selector** — the kernel selector that error texts name, `at:put:` — and the
existing `Primitive` it runs. `Primitive` gains a variant
`Primitive::Mirror(MirrorPrimitive)`, where `MirrorPrimitive(u8)` is a row index.
`u8` because 71 rows fit and because `Primitive` must stay two bytes:
`interpreter/resolve.rs`'s
`a_resolution_stays_the_size_the_hot_path_was_measured_at` pins the size of
the `Resolved` that carries it.

Every mirror selector ends in `ifFail:`. Its arguments are the **subject**
(the kernel method's `self`), the leaf's own arguments, and the failure block,
so its arity is the reported selector's plus two.

A mirror send runs one of three ways, and all three run the same row:

1. **Sent** — an ordinary `SendPublic`/`SendInlinable` to the mirror object,
   which the reference lowering emits and which also runs whenever the
   optimized lowering could not inline the block. The lookup finds
   `MethodEntry::Primitive(Primitive::Mirror(row))` on `VmMirror`;
   `builtins::execute` hands it to `interpreter::mirror::execute_sent`, which
   splits off the subject and the block, runs the row's leaf on the subject,
   and on the leaf's failure activates the block (as a control-flow block, so
   the depth count exempts it exactly as it exempts `ifTrue:`'s — see below).
   It never answers `None`, so `act_above` is never reached through it.
2. **Inlined** — `Instruction::SendMirror`, which the optimized lowering emits
   for a `Binding::MirrorSend` whose last argument is a literal block. The
   instruction carries the row, resolved at compile time, and a `failure`
   target where the block's body was lowered inline. Nothing is looked up at
   run time: the mirror's class and method table are built once, at bootstrap,
   from the same static table the compiler read, and nothing can add to or
   override them — no class can inherit from `VmMirror`, since no program can
   name it (Task 1), and there is no MirrorBuilder yet. When one exists, the
   mirror's class must be sealed against it; Task 16 records that.
3. **Direct** — the primitive-method fast path (Task 4). A kernel method whose
   whole body is a recognized mirror send forwarding `self` and its own
   parameters carries the row on its `MixinMethod`, and `act` runs the leaf on
   the send's own receiver and arguments before building an activation.

### Why `Binding::MirrorSend`, and why only in the kernel

Both recognizers need to know that a send's receiver is the VM mirror. The
compiler cannot know what a slot holds, so it is told: `resolver::resolve_kernel`
— used by the bootstrap and by crate-internal tests, never by the CLI —
records `Binding::MirrorSend` on the span of every explicit-receiver send whose
receiver is the bare identifier `vmMirror` resolving to a member of the
module's root class. Every other consumer treats a `MirrorSend` exactly like an
ordinary send with no binding. The bootstrap checks what the resolver cannot:
that the root class declares `vmMirror` as an immutable slot and that, after
instantiation, it holds the mirror object. A user module is resolved by
`resolve`, which never records the binding, so no user module can obtain a
primitive by copying the kernel's shape — it would get an ordinary send to
whatever its own `vmMirror` holds.

### Why the fast path comes before the control-flow family

`compiler.md`'s "How the limit counts" pins that both lowerings count
activations identically at every point of every program, which is what lets a
stack overflow be compared byte for byte. Today the reference lowering's
`SendInlinable` of `ifTrue:` reaches the Boolean native, which activates the
literal block; that activation is exempt because the optimized lowering never
performs it. With the kernel and no fast path, the same `SendInlinable` would
activate `Boolean>>ifTrue:` — counted — and the block inside it would fail the
exemption's condition (it was not created by the activation that sent), so the
reference lowering would count two where the optimized lowering counts none.
With the fast path, `Boolean>>ifTrue:` is never activated on success: the
primitive answers the block activation straight to the `SendInlinable`'s
sender, and the existing exemption applies unchanged. So the fast path (Task 4)
lands before any family moves (Tasks 5–12).

The measurement switch compiles the kernel without the marks, which breaks
this agreement again. It is for measuring only, and is documented as such.

### The bootstrap, step by step

`Interpreter::bootstrap_with` does what it does today — `World::new` builds
every builtin class object with a placeholder application — and then installs
the kernel (Task 2):

1. **Placeholders exist.** `Mutator::builtin_class` answers a class object for
   every `BuiltinClass` from the first instruction on, as it must, because a
   literal needs its class. `VmMirror`'s class and its one instance are built
   here too.
2. **`Kernel.ns` is compiled but not instantiated.** Its root mixin and its
   nested mixins are built with `Mutator::build_mixin`, compiled under the
   interpreter's lowering like any module.
3. **Each placeholder gets its kernel application**, in `BuiltinClass::ALL`
   order, so a superclass is always filled before its subclass: the nested
   mixin named like the class, applied to its superclass's application —
   `Object`'s to none, `True`'s and `False`'s to `Boolean`'s, every other to
   `Object`'s — installed with `Heap::set_class_application`, which restamps
   the few objects that exist this early. `VmMirror` keeps its own Rust mixin,
   re-applied to the kernel `Object`'s application. Until a family's task has
   moved it, the family's remaining Rust primitives are merged into the kernel
   mixin's table (kernel entries win); Task 13 removes the merge once nothing
   is left to merge.
4. **The immediate dispatch-id table is refreshed**, because an immediate's
   ids are read off its class's application, which just changed.
5. **The kernel class is built and instantiated** from the host, with the
   mirror as its factory argument: `Kernel vmMirror: theMirror`.
6. **The instance is wired in**: every placeholder's `enclosing` becomes the
   kernel instance and its `declaring_class` the kernel class, so a kernel
   method's lexical send of `vmMirror` reaches the slot; each hidden
   nested-class slot of the kernel instance is filled with its placeholder, so
   the nested-class accessor `Array` inside kernel code answers the builtin
   `Array` rather than building a second class object.

The resolved kernel is parsed and resolved once per thread and kept
(`thread_local!`): the test suite bootstraps an interpreter over a hundred
times, and the AST and bindings are immutable. Each interpreter still compiles
it, because compiled code belongs to one heap's inline caches.

---

## The mirror table

This is the complete `MIRROR` table, in this order (the row index is its
position). Task 1 transcribes it into `builtins/primitive.rs`. Each row is
`mirror selector — reported selector — primitive`.

| # | Mirror selector | Reported | Primitive |
|---|---|---|---|
| 0 | `objectClass:ifFail:` | `class` | `Object(Class)` |
| 1 | `object:identicalTo:ifFail:` | `==` | `Object(Identical)` |
| 2 | `object:equals:ifFail:` | `=` | `Object(Equal)` |
| 3 | `objectHash:ifFail:` | `hash` | `Object(Hash)` |
| 4 | `objectPrintString:ifFail:` | `printString` | `Object(PrintString)` |
| 5 | `object:doesNotUnderstand:ifFail:` | `doesNotUnderstand:` | `Object(DoesNotUnderstand)` |
| 6 | `boolean:ifTrue:ifFalse:ifFail:` | `ifTrue:ifFalse:` | `Boolean(IfTrueIfFalse)` |
| 7 | `boolean:ifFalse:ifTrue:ifFail:` | `ifFalse:ifTrue:` | `Boolean(IfFalseIfTrue)` |
| 8 | `boolean:ifTrue:ifFail:` | `ifTrue:` | `Boolean(IfTrue)` |
| 9 | `boolean:ifFalse:ifFail:` | `ifFalse:` | `Boolean(IfFalse)` |
| 10 | `booleanNot:ifFail:` | `not` | `Boolean(Not)` |
| 11 | `boolean:logicalAnd:ifFail:` | `&` | `Boolean(And)` |
| 12 | `boolean:and:ifFail:` | `and:` | `Boolean(And)` |
| 13 | `boolean:logicalOr:ifFail:` | `\|` | `Boolean(Or)` |
| 14 | `boolean:or:ifFail:` | `or:` | `Boolean(Or)` |
| 15 | `booleanPrintString:ifFail:` | `printString` | `Boolean(PrintString)` |
| 16 | `integer:add:ifFail:` | `+` | `Integer(Add)` |
| 17 | `integer:subtract:ifFail:` | `-` | `Integer(Subtract)` |
| 18 | `integer:multiply:ifFail:` | `*` | `Integer(Multiply)` |
| 19 | `integer:floorDivide:ifFail:` | `//` | `Integer(FloorDivide)` |
| 20 | `integer:lessThan:ifFail:` | `<` | `Integer(Less)` |
| 21 | `integer:greaterThan:ifFail:` | `>` | `Integer(Greater)` |
| 22 | `integer:lessOrEqual:ifFail:` | `<=` | `Integer(LessOrEqual)` |
| 23 | `integer:greaterOrEqual:ifFail:` | `>=` | `Integer(GreaterOrEqual)` |
| 24 | `integer:equals:ifFail:` | `=` | `Integer(Equal)` |
| 25 | `integerNegated:ifFail:` | `negated` | `Integer(Negated)` |
| 26 | `integerAbsolute:ifFail:` | `abs` | `Integer(Absolute)` |
| 27 | `integerPrintString:ifFail:` | `printString` | `Integer(PrintString)` |
| 28 | `integerAsString:ifFail:` | `asString` | `Integer(PrintString)` |
| 29 | `float:add:ifFail:` | `+` | `Float(Add)` |
| 30 | `float:subtract:ifFail:` | `-` | `Float(Subtract)` |
| 31 | `float:multiply:ifFail:` | `*` | `Float(Multiply)` |
| 32 | `float:divide:ifFail:` | `/` | `Float(Divide)` |
| 33 | `float:lessThan:ifFail:` | `<` | `Float(Less)` |
| 34 | `float:greaterThan:ifFail:` | `>` | `Float(Greater)` |
| 35 | `float:equals:ifFail:` | `=` | `Float(Equal)` |
| 36 | `floatPrintString:ifFail:` | `printString` | `Float(PrintString)` |
| 37 | `floatAsString:ifFail:` | `asString` | `Float(PrintString)` |
| 38 | `stringSize:ifFail:` | `size` | `String(Size)` |
| 39 | `string:concatenate:ifFail:` | `,` | `String(Concatenate)` |
| 40 | `string:equals:ifFail:` | `=` | `String(Equal)` |
| 41 | `stringAsSymbol:ifFail:` | `asSymbol` | `String(AsSymbol)` |
| 42 | `stringPrintString:ifFail:` | `printString` | `String(PrintString)` |
| 43 | `symbolAsString:ifFail:` | `asString` | `Symbol(AsString)` |
| 44 | `symbol:equals:ifFail:` | `=` | `Symbol(Equal)` |
| 45 | `symbolPrintString:ifFail:` | `printString` | `Symbol(PrintString)` |
| 46 | `characterAsInteger:ifFail:` | `asInteger` | `Character(AsInteger)` |
| 47 | `characterAsString:ifFail:` | `asString` | `Character(AsString)` |
| 48 | `character:equals:ifFail:` | `=` | `Character(Equal)` |
| 49 | `collection:do:ifFail:` | `do:` | `Collection(Enumerate)` |
| 50 | `collectionSize:ifFail:` | `size` | `Collection(Size)` |
| 51 | `collection:at:ifFail:` | `at:` | `Collection(ElementAt)` |
| 52 | `collection:at:put:ifFail:` | `at:put:` | `Collection(ElementAtPut)` |
| 53 | `collection:add:ifFail:` | `add:` | `Collection(Add)` |
| 54 | `blockValue:ifFail:` | `value` | `Block(Value)` |
| 55 | `block:value:ifFail:` | `value:` | `Block(Value)` |
| 56 | `block:value:value:ifFail:` | `value:value:` | `Block(Value)` |
| 57 | `block:value:value:value:ifFail:` | `value:value:value:` | `Block(Value)` |
| 58 | `blockNumArgs:ifFail:` | `numArgs` | `Block(ArgumentCount)` |
| 59 | `block:whileTrue:ifFail:` | `whileTrue:` | `Block(WhileTrue)` |
| 60 | `block:whileFalse:ifFail:` | `whileFalse:` | `Block(WhileFalse)` |
| 61 | `platformTranscript:ifFail:` | `transcript` | `Platform(Transcript)` |
| 62 | `platformObject:ifFail:` | `Object` | `Platform(ObjectClass)` |
| 63 | `platformArray:ifFail:` | `Array` | `Platform(ArrayClass)` |
| 64 | `platformOrderedCollection:ifFail:` | `OrderedCollection` | `Platform(OrderedCollectionClass)` |
| 65 | `platformString:ifFail:` | `String` | `Platform(StringClass)` |
| 66 | `platformSymbol:ifFail:` | `Symbol` | `Platform(SymbolClass)` |
| 67 | `platformCollections:ifFail:` | `collections` | `Platform(Collections)` |
| 68 | `platformList:ifFail:` | `List` | `Platform(List)` |
| 69 | `platform:show:ifFail:` | `show:` | `Platform(Show)` |
| 70 | `platformNewLine:ifFail:` | `newLine` | `Platform(NewLine)` |

`Array` and `OrderedCollection` share rows 49–53, as they share their
primitives today; each kernel class declares only the selectors its current
list in `builtins/primitive.rs` names (`Array`: `do:`, `at:put:`, `size`,
`at:`; `OrderedCollection`: `do:`, `add:`, `size`, `at:`).

## The shape of every kernel method

```newspeak
public <pattern> = ( ^vmMirror <mirror selector with: self, then the pattern's parameters in order> ifFail: [^super <the same pattern, the same parameters>] )
```

The failure block is `[^super …]` everywhere, and that one rule reproduces
today's "a failing primitive passes its level over": a receiver of the wrong
shape — an instance of `Text = String new`, which carries no string, or a
platform object sent another platform object's selector — reaches the
superclass's method, which is `Object`'s, and a selector `Object` does not
have ends in `Object`'s `doesNotUnderstand:` through §5.10's super failure
path, with the same message text. `goldens/errors/native_that_fails_on_its_receiver_falls_to_objects`
and `goldens/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class`
pin exactly this. `Object`'s own primitives never fail, so `Object`'s blocks
are unreachable; they keep the same shape so that one rule, and one
recognizer, covers every method.

Access follows today's lists: every method is `public` except `Object`'s
`doesNotUnderstand:`, which §5.5 makes `protected`.

---

## File Structure

- `crates/ns-runtime/kernel/Kernel.ns` — **create.** The core library's
  source: the `Kernel` module and one nested class per builtin class but
  `VmMirror`.
- `crates/ns-runtime/src/interpreter/kernel.rs` — **create.** The bootstrap's
  kernel phase: the per-thread resolved kernel, filling the placeholders,
  instantiating, wiring the instance in, and the checks on the kernel's shape.
- `crates/ns-runtime/src/interpreter/mirror.rs` — **create.** How a mirror
  send runs: sent, inlined and direct; the failure step; the error for a
  `SendMirror` whose receiver is not the mirror.
- `crates/ns-runtime/src/compiler/primitive_method.rs` — **create.** The
  primitive-method recognizer and the measurement switch.
- `crates/ns-runtime/src/builtins/primitive.rs` — the `MIRROR` table,
  `Primitive::Mirror`, `MirrorPrimitive`, the lookups by selector and row; the
  per-family lists shrink task by task and are gone by Task 13.
- `crates/ns-runtime/src/builtins/mod.rs` — `execute`'s `Mirror` arm.
- `crates/ns-runtime/src/types.rs` — `BuiltinClass::VmMirror`;
  `Binding::MirrorSend`.
- `crates/ns-runtime/src/resolver/mod.rs` — `resolve_kernel` and the one
  place it records `MirrorSend`.
- `crates/ns-runtime/src/interpreter/mod.rs` — `Mutator`'s `vm_mirror` and
  `kernel` fields, the mirror's construction, the immediate-id refresh as a
  function of its own, `bootstrap_with` calling the kernel phase.
- `crates/ns-runtime/src/heap/world.rs` — both root walks visit the two new
  fields.
- `crates/ns-runtime/src/heap.rs` — `set_class_lexical_context`.
- `crates/ns-runtime/src/interpreter/instantiate.rs` — `build_mixin` widened to
  `pub(crate)`; `decide_factory` refuses `VmMirror`.
- `crates/ns-runtime/src/interpreter/resolve.rs` — `act`'s fast path.
- `crates/ns-runtime/src/mixin.rs` — `MixinMethod.primitive`.
- `crates/ns-runtime/src/bytecode.rs` — `Instruction::SendMirror`, its
  `disassemble` arm, `outer_reach_of`.
- `crates/ns-runtime/src/compiler/control_flow.rs` — `recognize_mirror_send`.
- `crates/ns-runtime/src/compiler/lower.rs` — `lower_mirror_send`; the gate;
  `send_writes_register`.
- `crates/ns-runtime/src/compiler/capture.rs` — `Walker::send` agrees with the
  gate.
- `crates/ns-runtime/src/compiler/reference.rs` — a `MirrorSend` with a literal
  `ifFail:` block is a `SendInlinable`.
- `crates/ns-runtime/src/vm.rs` — the `SendMirror` arm; `send`'s delivery
  factored out so `send_mirror` shares it; `send_destination`.
- `crates/ns-runtime/src/image/encode.rs`, `decode.rs`, `inspect.rs`,
  `mixins.rs` — tag 14, its refusal, the exhaustive `Instruction` matches,
  `MixinMethod.primitive` on the way in.
- `crates/ns-cli/src/commands/build.rs` — `root_class_answers` stops at the
  kernel.
- Tests: `crates/ns-runtime/src/interpreter/mirror.rs` and `kernel.rs` (unit
  tests, crate-internal because the mirror is not reachable from outside the
  crate — deliberately, it is a capability), `compiler/control_flow.rs`,
  `compiler/lower.rs`, `compiler/primitive_method.rs`,
  `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`,
  `crates/ns-cli/tests/build_load.rs`, and new goldens under
  `goldens/kernel/`.
- Docs (Task 16): `core-library.md`, `dispatch.md`, `compiler.md`,
  `execution.md`, `object-model.md`, `image-format.md`, `memory.md`,
  `docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`.

---

### Task 1: The mirror class, its table, and the mirror object

Nothing in Newspeak can reach the mirror after this task; it is built, tested
from Rust inside the crate, and refused by the image format.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass`, `ALL`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs`
- Create: `crates/ns-runtime/src/interpreter/mirror.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (module declaration,
  `Mutator` field, construction in `bootstrap_builtin_classes`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::new`, both root walks)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`decide_factory`)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`, the
  graph walk's refusal)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`builtin_class_from_tag`,
  `read_program_builtin_class`, its three callers)
- Test: `crates/ns-runtime/src/interpreter/mirror.rs` (unit tests),
  `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`
  if it pins builtin class tags, otherwise `tests/image.rs`

**Interfaces:**
- Produces: `BuiltinClass::VmMirror` (last in `ALL`, which becomes
  `[BuiltinClass; 15]`); `builtins::primitive::{MirrorPrimitive, MirrorEntry,
  MIRROR, mirror_entry, mirror_primitive_named}`; `Primitive::Mirror`;
  `Mutator::vm_mirror(&self) -> Value<'gc>` and
  `Mutator::is_vm_mirror(&self, value: Value<'gc>) -> bool`;
  `interpreter::mirror::{execute_sent, run_leaf, reported_selector}`;
  `image::decode::read_program_builtin_class`.

- [ ] **Step 1: Write the failing unit tests**

Create `crates/ns-runtime/src/interpreter/mirror.rs` with its module doc and a
test module first; declare it in `interpreter/mod.rs` as
`pub(crate) mod mirror;`.

```rust
//! How a send to the VM mirror runs (`docs/design/core-library.md`, "The VM
//! mirror is the whole Rust boundary"). One row of
//! `builtins::primitive::MIRROR` runs one leaf; this module is the one place
//! that turns a send's arguments into that leaf's receiver and arguments, and
//! a leaf's failure into the failure block's activation, whichever of the
//! three ways the send ran: sent to the mirror object (`execute_sent`), with
//! its failure block lowered inline (`send_inlined`, Task 3), or as the
//! primitive of a kernel method (`execute_method`, Task 4).

#[cfg(test)]
mod tests {
    use crate::builtins::primitive::{mirror_primitive_named, MIRROR};
    use crate::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    fn the_mirror(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        })
    }

    #[test]
    fn a_mirror_send_runs_its_leaf_on_the_subject() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let three = interpreter.stash(Value::SmallInteger(3));
        let four = interpreter.stash(Value::SmallInteger(4));
        let fallback = interpreter.stash(Value::SmallInteger(-1));
        let sum = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[three, four, fallback],
            ),
            "integer:add:ifFail:",
        );
        let answer = interpreter.enter(|mutator| mutator.fetch(&sum));
        assert!(matches!(answer, Value::SmallInteger(7)));
    }

    /// A leaf that does not recognize its subject answers the failure
    /// argument's value -- here not a block, so it stands for itself, the
    /// convention `ifTrue:` already follows for a non-block argument.
    #[test]
    fn a_leaf_that_fails_on_its_subject_answers_the_failure_argument() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let nothing = interpreter.stash(Value::Nil);
        let four = interpreter.stash(Value::SmallInteger(4));
        let fallback = interpreter.stash(Value::SmallInteger(-1));
        let answer = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[nothing, four, fallback],
            ),
            "integer:add:ifFail: on nil",
        );
        let answer = interpreter.enter(|mutator| mutator.fetch(&answer));
        assert!(matches!(answer, Value::SmallInteger(-1)));
    }

    /// The error a leaf raises names the kernel selector, not the mirror's:
    /// the text a program sees must not change when `+` moves behind the
    /// mirror.
    #[test]
    fn an_error_a_leaf_raises_names_the_reported_selector() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let three = interpreter.stash(Value::SmallInteger(3));
        let text = interpreter.enter(|mutator| {
            let string = mutator.new_string("x".to_string());
            mutator.stash(Value::Object(string))
        });
        let fallback = interpreter.stash(Value::Nil);
        match interpreter.send(
            &mirror,
            Symbol::intern("integer:add:ifFail:"),
            &[three, text, fallback],
        ) {
            Err(HostSignal::Error(error)) => {
                assert_eq!(error.selector, Symbol::intern("+"));
                assert!(
                    error.message.starts_with("+ requires a numeric argument"),
                    "{}",
                    error.message
                );
            }
            _ => panic!("3 + 'x' through the mirror must raise the mismatch error"),
        }
    }

    #[test]
    fn every_mirror_selector_is_unique_ends_in_if_fail_and_takes_two_more_arguments() {
        let mut seen = std::collections::HashSet::new();
        for (row, entry) in MIRROR.iter().enumerate() {
            assert!(seen.insert(entry.selector), "{} listed twice", entry.selector);
            assert!(entry.selector.ends_with("ifFail:"), "{}", entry.selector);
            let mirror_arity = entry.selector.matches(':').count();
            assert_eq!(
                mirror_arity,
                entry.leaf_arity() + 2,
                "{} must take the subject, the leaf's arguments and the failure block",
                entry.selector
            );
            assert!(
                !matches!(entry.primitive, crate::builtins::primitive::Primitive::Mirror(_)),
                "{} runs a mirror row",
                entry.selector
            );
            let named = mirror_primitive_named(entry.selector).map(|found| found.index());
            assert_eq!(named, Some(row), "{}", entry.selector);
        }
    }

    #[test]
    fn the_mirror_class_cannot_be_instantiated() {
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.enter(|mutator| {
            let class = mutator.builtin_class(crate::BuiltinClass::VmMirror);
            mutator.stash(Value::Object(class))
        });
        assert!(matches!(
            interpreter.send(&class, Symbol::NEW, &[]),
            Err(HostSignal::Error(_))
        ));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --lib interpreter::mirror`
Expected: FAIL to compile — `vm_mirror`, `MIRROR`, `mirror_primitive_named`
and `BuiltinClass::VmMirror` do not exist.

- [ ] **Step 3: Add `BuiltinClass::VmMirror`**

In `crates/ns-runtime/src/types.rs`, append `VmMirror` as the last variant of
`BuiltinClass`, and append `BuiltinClass::VmMirror` to `ALL`, whose type
becomes `[BuiltinClass; 15]`. Document the variant:

```rust
    /// The VM mirror's class (`docs/design/core-library.md`, "The VM mirror
    /// is the whole Rust boundary"): one instance, built at bootstrap and
    /// handed to the core library alone. No program can name it -- it is
    /// not a bare builtin name, and the image format refuses its tag
    /// wherever a program names a class.
    VmMirror,
```

`builtin_kind_from_name` (`interpreter/instantiate.rs`) does **not** gain a
`"VmMirror"` arm: that function is the list of names a program may write.

- [ ] **Step 4: Add the table to `builtins/primitive.rs`**

Add the variant to `Primitive`:

```rust
    /// A leaf reached as a send to the VM mirror: a row of `MIRROR`.
    Mirror(MirrorPrimitive),
```

and, after the `PlatformPrimitive` enum:

```rust
/// A row of `MIRROR`. A `u8`, because `Primitive` must stay two bytes: a
/// `Resolved::Primitive` carries one, and its size is pinned
/// (`interpreter::resolve`'s
/// `a_resolution_stays_the_size_the_hot_path_was_measured_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorPrimitive(u8);

impl MirrorPrimitive {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One leaf, as the VM mirror answers it. The mirror selector's arguments
/// are the subject (the kernel method's `self`), the leaf's own arguments,
/// and the failure block, so it takes two more than `reported` does.
#[derive(Debug, Clone, Copy)]
pub struct MirrorEntry {
    /// What `Kernel.ns` sends to the mirror.
    pub selector: &'static str,
    /// The kernel selector the leaf's error texts name -- the one the
    /// program sent, so no text a program sees changes by moving behind the
    /// mirror.
    pub reported: &'static str,
    /// The leaf itself; never a `Primitive::Mirror`.
    pub primitive: Primitive,
}

impl MirrorEntry {
    /// How many arguments the reported selector takes: one for a binary
    /// selector, one per colon for a keyword selector, none for a unary one.
    pub fn leaf_arity(&self) -> usize {
        let binary = !self
            .reported
            .starts_with(|character: char| character.is_alphabetic());
        if binary {
            1
        } else {
            self.reported.matches(':').count()
        }
    }
}

const fn mirror(
    selector: &'static str,
    reported: &'static str,
    primitive: Primitive,
) -> MirrorEntry {
    MirrorEntry {
        selector,
        reported,
        primitive,
    }
}
```

Then `pub const MIRROR: &[MirrorEntry] = &[ … ];` with the 71 rows of "The
mirror table" above, in that order, one `mirror("…", "…", Primitive::…)` per
row. For example, rows 0, 16 and 52:

```rust
    mirror("objectClass:ifFail:", "class", Primitive::Object(ObjectPrimitive::Class)),
    mirror("integer:add:ifFail:", "+", Primitive::Integer(IntegerPrimitive::Add)),
    mirror(
        "collection:at:put:ifFail:",
        "at:put:",
        Primitive::Collection(CollectionPrimitive::ElementAtPut),
    ),
```

Then the two lookups:

```rust
/// The row `primitive` names.
pub fn mirror_entry(primitive: MirrorPrimitive) -> &'static MirrorEntry {
    &MIRROR[primitive.index()]
}

/// The row whose mirror selector is `selector`, for the compiler, which
/// binds a recognized mirror send to its row once, at compile time.
pub fn mirror_primitive_named(selector: &str) -> Option<MirrorPrimitive> {
    MIRROR
        .iter()
        .position(|entry| entry.selector == selector)
        .map(|index| MirrorPrimitive(index as u8))
}
```

A linear search is deliberate: it runs once per mirror send the compiler
recognizes, never at run time. Add a `const` assertion that the table fits a
`u8`:

```rust
const _: () = assert!(MIRROR.len() <= u8::MAX as usize + 1);
```

`primitives_of` gains `BuiltinClass::VmMirror => &[]`, and `method_table`
special-cases it, because the mirror's entries come from `MIRROR` rather than
from a `PrimitiveSelector` list:

```rust
pub fn method_table(kind: BuiltinClass) -> IdentityMap<Symbol, MethodEntry> {
    if kind == BuiltinClass::VmMirror {
        return MIRROR
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                (
                    Symbol::intern(entry.selector),
                    MethodEntry::Primitive(PrimitiveMethod {
                        primitive: Primitive::Mirror(MirrorPrimitive(index as u8)),
                        access: ast::AccessModifier::Public,
                    }),
                )
            })
            .collect();
    }
    // … the existing body, unchanged …
}
```

- [ ] **Step 5: Execute a sent mirror primitive**

In `builtins/mod.rs`, `execute` gains the arm

```rust
        Primitive::Mirror(row) => Some(crate::interpreter::mirror::execute_sent(
            mutator, row, arguments,
        )),
```

It is always `Some`: a mirror primitive never passes its level over, because
its failure path is the block the send carries. In `interpreter/mirror.rs`,
above the tests:

```rust
use crate::builtins::primitive::{mirror_entry, MirrorEntry, MirrorPrimitive, MIRROR};
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

thread_local! {
    /// Each row's reported selector, interned once per thread -- the
    /// interner is per thread (`symbol.rs`), so a process-wide table of
    /// `Symbol`s would be wrong on every thread but the first.
    static REPORTED: Vec<Symbol> =
        MIRROR.iter().map(|entry| Symbol::intern(entry.reported)).collect();
}

/// The kernel selector `row`'s error texts name.
pub(crate) fn reported_selector(row: MirrorPrimitive) -> Symbol {
    REPORTED.with(|reported| reported[row.index()])
}

/// A send to the mirror object: the reference lowering's, and the optimized
/// lowering's wherever it could not lower the failure block inline.
/// `arguments` are the subject, the leaf's arguments, then the failure block.
pub(crate) fn execute_sent<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let entry = mirror_entry(row);
    // The lookup matched the selector, whose arity fixes the argument count
    // for any send a compiler emitted; the check is for a decoded send that
    // names the selector with the wrong count, which must fail as an error,
    // never as a slice index.
    let [subject, rest @ .., block] = arguments else {
        return Err(arity_error(entry, arguments.len()));
    };
    if rest.len() != entry.leaf_arity() {
        return Err(arity_error(entry, arguments.len()));
    }
    match run_leaf(mutator, row, *subject, rest) {
        Some(result) => result.map(|step| owning_its_arguments(step, rest)),
        None => Ok(failure_step(mutator, *block)),
    }
}

/// The leaf of `row`, on `subject` with `leaf_arguments`; `None` when it
/// does not recognize the subject's shape.
pub(crate) fn run_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    subject: Value<'gc>,
    leaf_arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let entry = mirror_entry(row);
    crate::builtins::execute(
        mutator,
        entry.primitive,
        subject,
        reported_selector(row),
        leaf_arguments,
    )
}

/// A leaf that activates a block with "the send's own arguments" means the
/// kernel send's -- the leaf's arguments -- not the mirror send's, which
/// also carry the subject and the failure block.
fn owning_its_arguments<'gc>(step: Step<'gc>, leaf_arguments: &[Value<'gc>]) -> Step<'gc> {
    match step {
        Step::Block {
            closure,
            arguments: StepArguments::Send,
            then,
        } => Step::Block {
            closure,
            arguments: StepArguments::Owned(leaf_arguments.to_vec()),
            then,
        },
        other => other,
    }
}

/// The failure block, activated with no arguments; a failure argument that
/// is not a block is its own value. Marked `ControlFlow` so that the depth
/// count treats it as it treats `ifTrue:`'s block: the optimized lowering
/// lowers this block inline and never activates it, so under the reference
/// lowering its activation is exempt exactly when that rule's condition holds
/// (`docs/design/compiler.md`, "How the limit counts").
fn failure_step<'gc>(mutator: &Mutator<'gc>, block: Value<'gc>) -> Step<'gc> {
    if let Value::Object(object) = block {
        if mutator.heap().is_closure(object) {
            return Step::Block {
                closure: object,
                arguments: StepArguments::ControlFlow,
                then: Continuation::Deliver,
            };
        }
    }
    Step::Value(block)
}

fn arity_error<'gc>(entry: &MirrorEntry, given: usize) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(entry.selector),
        receiver: Value::Nil,
        message: format!(
            "the VM mirror's {} takes {} arguments, got {given}",
            entry.selector,
            entry.leaf_arity() + 2
        ),
    })
}
```

Check `Step`'s definition (`interpreter/mod.rs`) for any other variant that
can carry `StepArguments::Send` out of `builtins::execute`, and convert it in
`owning_its_arguments` the same way. Today only `Step::Block` does: no native
answers a `Step::Method`.

- [ ] **Step 6: Build the mirror object**

In `interpreter/mod.rs`, add to `Mutator`, beside `platform_singleton`:

```rust
    /// The VM mirror (`BuiltinClass::VmMirror`'s one instance), built with
    /// the builtin classes and handed to the core library at bootstrap. A
    /// capability: nothing hands it to a program.
    pub(crate) vm_mirror: Option<Value<'gc>>,
```

initialize it to `None` in `World::new` (`heap/world.rs`), and visit it in
**both** root walks there, exactly as `platform_singleton` is visited (the
loops at `world.rs` near lines 141 and 195). At the end of
`bootstrap_builtin_classes`, build it:

```rust
        let class = self.builtin_class(BuiltinClass::VmMirror);
        let mirror = self.new_instance_object(class, ObjectData::Instance(vec![]));
        self.vm_mirror = Some(Value::Object(mirror));
```

and add the two accessors:

```rust
    /// The VM mirror. Built by `bootstrap_builtin_classes`, before anything
    /// can ask.
    pub(crate) fn vm_mirror(&self) -> Value<'gc> {
        self.vm_mirror
            .expect("internal: the VM mirror is built with the builtin classes")
    }

    /// Whether `value` is the VM mirror.
    pub(crate) fn is_vm_mirror(&self, value: Value<'gc>) -> bool {
        match (value, self.vm_mirror) {
            (Value::Object(object), Some(Value::Object(mirror))) => object.same(mirror),
            _ => false,
        }
    }
```

`build_builtin_classes` needs no change: it already builds a class for every
`ALL` entry, applied to `Object`, with `builtin_mixin(kind)`, which now reads
`method_table(VmMirror)`.

- [ ] **Step 7: Refuse to instantiate the mirror's class**

In `interpreter/instantiate.rs`, `decide_factory` gains an arm before the
collections one:

```rust
            // The VM mirror is a capability: there is exactly one, and it is
            // the bootstrap's to hand out (`docs/design/core-library.md`, "The
            // VM mirror is the whole Rust boundary").
            Some(BuiltinClass::VmMirror) => Err(Signal::Error(NewspeakError {
                selector: Symbol::NEW,
                receiver: Value::Object(class),
                message: "the VM mirror's class cannot be instantiated".to_string(),
            })),
```

- [ ] **Step 8: Run the unit tests**

Run: `cargo test -p ns-runtime --lib interpreter::mirror`
Expected: PASS, all five.

- [ ] **Step 9: Give the class its wire tag, and refuse it where a program names a class**

In `image/encode.rs`, `builtin_class_tag` gains `BuiltinClass::VmMirror => 14`;
in `image/decode.rs`, `builtin_class_from_tag` gains `14 => BuiltinClass::VmMirror`.
The tag-named hash section (`encode_tag_named_hashes`, which loops over `ALL`)
now writes the mirror class's hash too, and the decoder reads it back through
`read_builtin_class` unchanged.

Add beside `read_builtin_class`:

```rust
/// A builtin class a *program* names: `LoadBuiltinClass`, a class reference
/// in the graph, a root class's superclass. The VM mirror's class is refused
/// here -- an artifact that could name it could instantiate or reach a second
/// mirror, and so claim every primitive, which no source module can. Only
/// the closing hash table names it, by tag, as it names every builtin.
pub fn read_program_builtin_class(reader: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    match read_builtin_class(reader)? {
        BuiltinClass::VmMirror => Err(DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: 14,
        }),
        kind => Ok(kind),
    }
}
```

and use it at the three program-level callers of `read_builtin_class` in
`decode.rs` — the `LoadBuiltinClass` decode (near line 1485), the
`CLASS_BUILTIN` class reference (near line 1825) and the root superclass (near
line 2119). The hash section (near line 2546) and `inspect.rs` keep
`read_builtin_class`: `inspect` describes bytes, it does not load them.

In the encoder's graph walk — where `encode_graph_section` discovers the
objects it will write, the place that returns `Err` for a shape it refuses —
refuse the mirror before anything is written:

```rust
        if heap.class_builtin(heap.class_of_object(object)) == Some(BuiltinClass::VmMirror) {
            return Err(
                "image encoding refuses the VM mirror: it is a capability the \
                 bootstrap hands to the core library alone, and an artifact that \
                 carried it would hand it to whatever loaded it"
                    .to_string(),
            );
        }
```

using whatever accessor the walk already uses to read an object's class
(match its spelling; `class_of_object` stands for it here).

- [ ] **Step 10: Pin the tag and the refusals**

In the file that pins the builtin class tags (search for `builtin_class_tag`
under `crates/ns-runtime/tests/`), add `VmMirror => 14` beside the others. In
`crates/ns-runtime/tests/image.rs`, copy
`an_invalid_builtin_class_tag_is_a_decode_error_not_a_panic` as
`a_load_builtin_class_naming_the_mirror_is_refused`, writing `14` where it
writes `200`, and expect
`DecodeError::InvalidTag { what: "builtin class a program may name", tag: 14 }`.

In `interpreter/mirror.rs`'s tests, add the encoder's refusal:

```rust
    #[test]
    fn a_snapshot_that_reaches_the_mirror_is_refused() {
        let source = indoc::indoc! {"
            Newspeak3
            'T'
            class Holder = Object new (
                | public held |
            ) (
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
        let mirror = the_mirror(&mut interpreter);
        let _ = expect_ok(
            interpreter.send(&instance, Symbol::intern("held:"), &[mirror]),
            "held:",
        );
        let refused = interpreter.enter(|mutator| {
            let Value::Object(class) = mutator.fetch(&class) else {
                panic!("the root is a class object");
            };
            let platform = mutator.platform_value();
            let holder = mutator.fetch(&instance);
            crate::image::snapshot(
                mutator,
                None,
                &crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
                class,
                &platform,
                &[holder],
            )
        });
        match refused {
            Err(message) => assert!(message.contains("VM mirror"), "{message}"),
            Ok(_) => panic!("an image must never carry the VM mirror"),
        }
    }
```

Adjust the calls to the spelling the crate actually has (`load_module`,
`instantiate`, `image::snapshot`'s parameter order are as of this writing;
`platform_value` is `pub(crate)`).

- [ ] **Step 11: Run the gates**

Run all of Global Constraints' gates. Expected: every one green; the suite
gains the new tests and nothing else moves. If a test pins
`BuiltinClass::ALL.len()` or the number of builtin classes, update it to 15 —
that is a mechanism test.

- [ ] **Step 12: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: add the VM mirror's class, its leaf table and its one instance"
```

---

### Task 2: `Kernel.ns`, kernel resolution, and the bootstrap's kernel phase

After this task every builtin class object is filled from `Kernel.ns`, whose
classes declare no methods yet: every family still answers through its Rust
primitives, merged into the kernel mixin's table. The only structural change a
program could see is `True` and `False` inheriting from `Boolean`, and no
program can see it yet, because there is no reflection.

**Files:**
- Create: `crates/ns-runtime/kernel/Kernel.ns`
- Create: `crates/ns-runtime/src/interpreter/kernel.rs`
- Modify: `crates/ns-runtime/src/types.rs` (`Binding::MirrorSend`)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (`resolve_kernel`)
- Modify: every exhaustive `match` on `Binding` the compiler then reports
  (`compiler/lower.rs`, `compiler/reference.rs`, `compiler/capture.rs` if any)
- Modify: `crates/ns-runtime/src/heap.rs` (`set_class_lexical_context`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`kernel` field,
  `refresh_immediate_dispatch_ids`, `bootstrap_with`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (the `kernel` field in both
  root walks)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`build_mixin` to
  `pub(crate)`)
- Test: `crates/ns-runtime/src/interpreter/kernel.rs` (unit tests)

**Interfaces:**
- Consumes: Task 1's `vm_mirror`, `BuiltinClass::VmMirror`, `MIRROR`.
- Produces: `Binding::MirrorSend`; `resolver::resolve_kernel(module, source,
  origin) -> Result<ResolvedModule, Vec<ResolveError>>` (`pub(crate)`);
  `interpreter::kernel::{resolved_kernel, KERNEL_SOURCE}`;
  `Mutator.kernel: Option<Value<'gc>>`;
  `Heap::set_class_lexical_context(class, enclosing, declaring_class) -> bool`.

- [ ] **Step 1: Write the kernel skeleton**

Create `crates/ns-runtime/kernel/Kernel.ns`:

```newspeak
Newspeak3
'Kernel'
(* The core library (`docs/design/core-library.md`, "Planned: the core library
   as Newspeak source over a VM mirror"). One nested class per builtin class;
   the bootstrap fills the builtin class objects from them before it
   instantiates this module, so a nested class here *is* the class a literal
   has. Every method reaches Rust through `vmMirror`, the one object that
   holds the leaves, and nothing else here does.

   A superclass clause below names a sibling by its bare name and nothing
   else: the bootstrap reads the name and never runs the clause, because the
   classes it names must exist before any clause could run. `Object` declares
   none: it has no superclass. *)
class Kernel vmMirror: theMirror = Object new (
    | private vmMirror = theMirror. |
) (
    class Object = () (
    )
    class Boolean = Object new () (
    )
    class True = Boolean new () (
    )
    class False = Boolean new () (
    )
    class Integer = Object new () (
    )
    class Float = Object new () (
    )
    class String = Object new () (
    )
    class Symbol = Object new () (
    )
    class Character = Object new () (
    )
    class Array = Object new () (
    )
    class OrderedCollection = Object new () (
    )
    class Nil = Object new () (
    )
    class Block = Object new () (
    )
    class Platform = Object new () (
    )
) : ()
```

- [ ] **Step 2: Write the failing unit tests**

Create `crates/ns-runtime/src/interpreter/kernel.rs` with its module doc and
tests; declare it in `interpreter/mod.rs` as `pub(crate) mod kernel;`.

```rust
//! The bootstrap's kernel phase (`docs/design/core-library.md`, "The
//! bootstrap: placeholders first, kernel second"): `Kernel.ns`, compiled,
//! fills the builtin class objects `World::new` built as placeholders, and is
//! then instantiated with the VM mirror. See this plan's "The bootstrap, step
//! by step" -- the design documents carry it once Task 16 lands.

#[cfg(test)]
mod tests {
    use crate::{BuiltinClass, Interpreter, Value};

    /// Every builtin class but the mirror's is the kernel class of its name.
    #[test]
    fn every_builtin_class_is_filled_from_the_kernel() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for kind in BuiltinClass::ALL {
                if kind == BuiltinClass::VmMirror {
                    continue;
                }
                let class = mutator.builtin_class(kind);
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a builtin class has an application");
                let declaration = application
                    .mixin
                    .declaration
                    .as_ref()
                    .unwrap_or_else(|| panic!("{kind:?} is still a placeholder"));
                assert_eq!(declaration.name.name, super::kernel_class_name(kind));
            }
        });
    }

    #[test]
    fn true_and_false_inherit_from_boolean() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let boolean = mutator.builtin_class(BuiltinClass::Boolean);
            for kind in [BuiltinClass::True, BuiltinClass::False] {
                let superclass = mutator
                    .heap()
                    .class_superclass(mutator.builtin_class(kind))
                    .expect("True and False have a superclass");
                assert!(superclass.same(boolean), "{kind:?}");
            }
            let object = mutator.builtin_class(BuiltinClass::Object);
            let above_boolean = mutator.heap().class_superclass(boolean);
            assert!(above_boolean.is_some_and(|class| class.same(object)));
            assert!(mutator.heap().class_superclass(object).is_none());
        });
    }

    /// The nested-class accessor inside kernel code answers the builtin
    /// class itself, not a second class object of the same mixin.
    #[test]
    fn the_kernels_hidden_class_slots_hold_the_builtin_classes() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            for kind in BuiltinClass::ALL {
                if kind == BuiltinClass::VmMirror {
                    continue;
                }
                let class = mutator.builtin_class(kind);
                assert!(
                    mutator
                        .heap()
                        .class_enclosing(class)
                        .is_some_and(|enclosing| enclosing.same(kernel)),
                    "{kind:?}'s enclosing object is the kernel"
                );
                let field = super::hidden_slot_of(mutator, kernel, kind);
                assert!(
                    matches!(
                        mutator.heap().instance_field(kernel, field),
                        Some(Value::Object(held)) if held.same(class)
                    ),
                    "{kind:?}'s hidden slot holds the builtin class"
                );
            }
        });
    }

    #[test]
    fn the_kernel_holds_the_mirror() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let held = mutator
                .heap()
                .instance_field(kernel, super::mirror_slot_of(mutator, kernel));
            assert!(held.is_some_and(|value| mutator.is_vm_mirror(value)));
        });
    }

    /// `Kernel.ns` resolves in kernel mode, and every send to `vmMirror` in
    /// it carries the mark the recognizers read.
    #[test]
    fn every_send_to_vm_mirror_in_the_kernel_is_marked() {
        let resolved = super::resolved_kernel();
        super::for_each_send_to_vm_mirror(&resolved, |send| {
            assert!(
                matches!(
                    resolved.bindings.get(&send.span),
                    Some(crate::Binding::MirrorSend)
                ),
                "a send to vmMirror at {:?} is not marked",
                send.span
            );
        });
    }
}
```

Use the accessor spellings the heap actually has (`instance_field` stands for
the field read `set_instance_field` pairs with; `class_enclosing`,
`class_superclass` and `class_application` exist as named). The helpers
`kernel_class_name`, `hidden_slot_of`, `mirror_slot_of` and
`for_each_send_to_vm_mirror` are written in Step 6.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p ns-runtime --lib interpreter::kernel`
Expected: FAIL to compile.

- [ ] **Step 4: Resolve in kernel mode**

In `types.rs`, add to `Binding`:

```rust
    /// An explicit-receiver send whose receiver is the core library's
    /// `vmMirror` slot. Recorded only by `resolver::resolve_kernel`, so no
    /// user module ever carries it. Every consumer treats it as an ordinary
    /// send with no binding; the two recognizers that inline a mirror send
    /// (`control_flow::recognize_mirror_send`,
    /// `primitive_method::recognize`) read it, because only the kernel's
    /// `vmMirror` is known to hold the mirror (`docs/design/core-library.md`,
    /// "The VM mirror is the whole Rust boundary").
    MirrorSend,
```

Build, and give every exhaustive `match` on `Binding` the compiler reports an
arm that behaves as the ordinary-send case does (in `reference.rs`'s `send`,
the arm the `None` binding takes). `control_flow::is_ordinary_send` needs no
change: it rejects only `SuperSend` and `LexicalSend`.

In `resolver/mod.rs`, add a `kernel: bool` field to `Resolver`, set `false` by
`resolve`, and add:

```rust
/// `resolve`, for `Kernel.ns` alone: additionally marks each send to the
/// root class's `vmMirror` with `Binding::MirrorSend`. Only the bootstrap
/// (`interpreter::kernel`) and crate-internal tests call it.
pub(crate) fn resolve_kernel(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    // … `resolve`'s body with `kernel: true` -- factor the shared body into
    // a private function taking the flag, rather than copying it …
}
```

In `resolve_send`, in the final ordinary-send branch (the one that resolves
the receiver "on its own merits"), before resolving the receiver:

```rust
        if self.kernel && is_the_mirror_slot(&send.receiver, scope, chain) {
            self.bindings.insert(send.span, Binding::MirrorSend);
        }
```

with

```rust
/// The bare identifier `vmMirror`, not a local, naming a member of the
/// module's root class -- the outermost entry of `chain`. That it is an
/// immutable slot holding the mirror is the bootstrap's to check
/// (`interpreter::kernel`), since the resolver cannot know what a slot holds.
fn is_the_mirror_slot(
    receiver: &ast::Expression,
    scope: &mut ScopeStack,
    chain: &[EnclosingClass],
) -> bool {
    let ast::Expression::Identifier(identifier) = receiver else {
        return false;
    };
    let name = Symbol::intern(&identifier.name);
    identifier.name == "vmMirror"
        && scope.resolve(name).is_none()
        && sends::find_member_depth(chain, name) == Some((chain.len() - 1) as u16)
}
```

(`chain` is never empty inside a method body: it holds at least the class the
method is declared in.)

- [ ] **Step 5: Give class objects a lexical context after the fact**

In `heap.rs`, beside `set_class_superclass`:

```rust
    /// Sets `class`'s enclosing object and declaring class. The bootstrap's
    /// kernel phase is the one caller: a builtin class object exists before
    /// the kernel instance that encloses it can, because a literal needs its
    /// class from the first instruction on. Answers whether the write
    /// happened, like the other class setters.
    pub fn set_class_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.enclosing = enclosing;
            data.declaring_class = declaring_class;
            return true;
        }
        false
    }
```

Match the field names `ClassData` actually uses.

- [ ] **Step 6: The kernel phase**

In `interpreter/kernel.rs`:

```rust
use std::rc::Rc;

use crate::interpreter::{HostSignal, Interpreter, Mutator};
use crate::mixin::{mixin_key, Mixin};
use crate::types::*;
use crate::{Root, Symbol};

/// `Kernel.ns`, compiled into the binary: the library is source the runtime
/// ships (`docs/design/core-library.md`, "Where the library source lives").
pub(crate) const KERNEL_SOURCE: &str = include_str!("../../kernel/Kernel.ns");

thread_local! {
    /// The kernel, parsed and resolved once per thread: the AST and its
    /// bindings are immutable, and the test suite bootstraps well over a
    /// hundred interpreters. Each interpreter still compiles it, because
    /// compiled code is numbered against one heap's inline caches.
    static RESOLVED_KERNEL: Rc<ResolvedModule> = Rc::new(resolve_kernel_source());
}

pub(crate) fn resolved_kernel() -> Rc<ResolvedModule> {
    RESOLVED_KERNEL.with(Rc::clone)
}

/// A kernel that does not parse or resolve is a defect in the binary, not
/// in a program, so it panics; `every_send_to_vm_mirror_in_the_kernel_is_marked`
/// and every other test that bootstraps catch it before a release could.
fn resolve_kernel_source() -> ResolvedModule {
    let module = match ns_syntax::parse(KERNEL_SOURCE) {
        Ok(module) => module,
        Err(diagnostics) => panic!("internal: Kernel.ns does not parse: {diagnostics:?}"),
    };
    match crate::resolver::resolve_kernel(
        module,
        Rc::from(KERNEL_SOURCE),
        SourceOrigin::File(std::path::PathBuf::from("Kernel.ns")),
    ) {
        Ok(resolved) => resolved,
        Err(errors) => panic!(
            "internal: Kernel.ns does not resolve: {}",
            errors
                .iter()
                .map(|error| error.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// The name of `kind`'s class in `Kernel.ns`. Not
/// `instantiate::builtin_kind_from_name`, which is the list of names a
/// *program* may write bare and which, for that reason, has no `Platform`.
pub(crate) fn kernel_class_name(kind: BuiltinClass) -> &'static str {
    match kind {
        BuiltinClass::Object => "Object",
        BuiltinClass::Boolean => "Boolean",
        BuiltinClass::True => "True",
        BuiltinClass::False => "False",
        BuiltinClass::Integer => "Integer",
        BuiltinClass::Float => "Float",
        BuiltinClass::String => "String",
        BuiltinClass::Symbol => "Symbol",
        BuiltinClass::Character => "Character",
        BuiltinClass::Array => "Array",
        BuiltinClass::OrderedCollection => "OrderedCollection",
        BuiltinClass::Nil => "Nil",
        BuiltinClass::Block => "Block",
        BuiltinClass::Platform => "Platform",
        BuiltinClass::VmMirror => unreachable!("internal: the mirror's class is not in Kernel.ns"),
    }
}

impl Interpreter {
    /// The kernel phase of the bootstrap: fill the placeholders, instantiate
    /// the kernel with the mirror, wire it in.
    pub(crate) fn install_kernel(&mut self) {
        let resolved = resolved_kernel();
        let (kernel_class, mirror) = self.enter(|mutator| {
            let kernel_class = mutator.fill_builtin_classes(&resolved);
            (
                mutator.stash(Value::Object(kernel_class)),
                mutator.stash(mutator.vm_mirror()),
            )
        });
        let kernel = match self.instantiate(&kernel_class, &[mirror]) {
            Ok(kernel) => kernel,
            Err(HostSignal::Error(error)) => {
                panic!("internal: Kernel.ns failed to instantiate: {}", error.message)
            }
            Err(HostSignal::Return) => {
                panic!("internal: a return escaped Kernel.ns's instantiation")
            }
        };
        self.enter(|mutator| mutator.wire_kernel_in(&kernel_class, &kernel));
    }
}

impl<'gc> Mutator<'gc> {
    /// Steps 2 to 4 of "The bootstrap, step by step": every placeholder
    /// gets the application of its kernel class, superclass first; then the
    /// kernel's own class is built over the filled `Object`.
    fn fill_builtin_classes(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let declaration = &resolved.module.root_class;
        let root_mixin = self.build_mixin(declaration, &resolved.bindings);
        check_kernel_shape(&root_mixin);
        for kind in BuiltinClass::ALL {
            let class = self.builtin_class(kind);
            let (mixin, superclass) = if kind == BuiltinClass::VmMirror {
                (self.heap.class_application(class).map(|application| application.mixin.clone())
                    .expect("internal: the mirror's class has its placeholder application"),
                 Some(BuiltinClass::Object))
            } else {
                let nested = nested_named(&root_mixin, kernel_class_name(kind));
                (merged_with_remaining_primitives(nested, kind), superclass_kind(nested))
            };
            let super_application = superclass.and_then(|superclass| {
                self.heap.class_application(self.builtin_class(superclass))
            });
            let application = self.apply_mixin(&mixin, super_application.as_ref());
            if !self.heap.set_class_application(class, application) {
                unreachable!("internal: a builtin class is a class object");
            }
            let superclass_object = superclass.map(|superclass| self.builtin_class(superclass));
            match superclass_object {
                Some(superclass_object) => {
                    self.heap.set_class_superclass(class, superclass_object);
                }
                None => self.heap.clear_class_superclass(class),
            }
        }
        self.refresh_immediate_dispatch_ids();
        let object = self.builtin_class(BuiltinClass::Object);
        let object_application = self.heap.class_application(object);
        let application = self.apply_mixin(&root_mixin, object_application.as_ref());
        self.new_class_object(application, None, None, Some(object))
    }

    /// Step 6: the kernel instance becomes every kernel class's enclosing
    /// object, and each of its hidden nested-class slots holds the builtin
    /// class it names.
    fn wire_kernel_in(&mut self, kernel_class: &Root, kernel: &Root) {
        let Value::Object(kernel_class) = self.fetch(kernel_class) else {
            unreachable!("internal: the kernel class is a class object");
        };
        let Value::Object(kernel) = self.fetch(kernel) else {
            unreachable!("internal: the kernel instance is an object");
        };
        let mirror_field = mirror_slot_of(self, kernel);
        let holds_the_mirror = self
            .heap
            .instance_field(kernel, mirror_field)
            .is_some_and(|value| self.is_vm_mirror(value));
        assert!(holds_the_mirror, "internal: Kernel's vmMirror slot must hold the VM mirror");
        for kind in BuiltinClass::ALL {
            if kind == BuiltinClass::VmMirror {
                continue;
            }
            let class = self.builtin_class(kind);
            let field = hidden_slot_of(self, kernel, kind);
            debug_assert!(
                matches!(self.heap.instance_field(kernel, field), Some(Value::Nil)),
                "internal: Kernel's initializer touched the nested class {kind:?} before the bootstrap could fill it"
            );
            self.heap.set_instance_field(kernel, field, Value::Object(class));
            self.heap
                .set_class_lexical_context(class, Some(kernel), Some(kernel_class));
            debug_assert!(
                self.heap.class_metaclass(class).is_none(),
                "internal: a metaclass built before the kernel was wired in borrowed the placeholder's context"
            );
        }
        self.kernel = Some(Value::Object(kernel));
    }
}
```

and the helpers the tests and the phase use:

```rust
/// The kernel's nested class called `name`. `check_kernel_shape` has made
/// sure there is exactly one.
fn nested_named<'mixin>(root: &'mixin Rc<Mixin>, name: &str) -> &'mixin Rc<Mixin> {
    root.nested
        .iter()
        .find(|nested| nested.name.as_str() == name)
        .unwrap_or_else(|| panic!("internal: Kernel.ns declares no class {name}"))
}

/// The builtin a kernel class's clause names, read off the declaration and
/// never run; `None` for `Object`.
fn superclass_kind(nested: &Rc<Mixin>) -> Option<BuiltinClass> {
    let declaration = nested.declaration.as_ref()?;
    let clause = declaration.superclass.as_ref()?;
    Some(
        BuiltinClass::ALL
            .into_iter()
            .find(|kind| {
                *kind != BuiltinClass::VmMirror && kernel_class_name(*kind) == clause.receiver.name
            })
            .unwrap_or_else(|| {
                panic!("internal: Kernel.ns's {} names no kernel class", nested.name)
            }),
    )
}

/// Until its family's task moves it, a family's Rust primitives are merged
/// into its kernel class's table, the kernel's own entries first. Task 13
/// deletes this once nothing is left to merge.
fn merged_with_remaining_primitives(nested: &Rc<Mixin>, kind: BuiltinClass) -> Rc<Mixin> {
    let remaining = crate::builtins::primitive::method_table(kind);
    if remaining.is_empty() {
        return nested.clone();
    }
    let mut merged = (**nested).clone();
    for (selector, entry) in remaining {
        merged.methods.entry(selector).or_insert(entry);
    }
    Rc::new(merged)
}

/// The shape the phase relies on and the resolver cannot check: one nested
/// class per builtin but the mirror's, named as `kernel_class_name` says,
/// each declaring no slots; a root class with exactly one slot, the
/// immutable `vmMirror`; a clause on every class but `Object`, naming a
/// class that comes earlier in `BuiltinClass::ALL`.
fn check_kernel_shape(root: &Rc<Mixin>) {
    // … assert each of those, with a message naming what is wrong …
}

/// The kernel instance's field holding `vmMirror`.
pub(super) fn mirror_slot_of(mutator: &Mutator<'_>, kernel: ObjectReference<'_>) -> usize {
    // The root application's own slot base plus `vmMirror`'s position among
    // the root mixin's own slots -- `InitializeSlot`'s rule
    // (`bytecode.rs`, `own_index`).
}

/// The kernel instance's hidden slot caching the nested class `kind`:
/// `MixinApplication::nested_slot`, keyed by the nested mixin's identity.
pub(super) fn hidden_slot_of(
    mutator: &Mutator<'_>,
    kernel: ObjectReference<'_>,
    kind: BuiltinClass,
) -> usize {
    // application of kernel's class -> nested_slot[&mixin_key(nested_named(root, name))]
}

/// Calls `visit` on every explicit-receiver send in `resolved` whose
/// receiver is the bare identifier `vmMirror`, for the test that checks the
/// resolver marked them all.
#[cfg(test)]
pub(super) fn for_each_send_to_vm_mirror(
    resolved: &ResolvedModule,
    visit: impl FnMut(&ns_syntax::ast::Send),
) {
    // a walk of every method body's statements and expressions
}
```

Write the bodies of `check_kernel_shape`, `mirror_slot_of`, `hidden_slot_of`
and `for_each_send_to_vm_mirror` against the accessors the crate has; each is
a few lines. `merged.methods.entry(…)` assumes `IdentityMap` is a `HashMap`
alias; use its insertion API if not. `Heap::clear_class_superclass` is new:
write it beside `set_class_superclass`, setting the field to `None` and
answering whether the write happened (`Object`'s placeholder has no
superclass already, so it is a no-op today; it makes the phase state what it
means rather than rely on that). `Heap::instance_field` stands for the field
read the heap already has. `build_mixin` becomes `pub(crate)`.

Move the immediate-id computation at the end of `bootstrap_builtin_classes`
into `Mutator::refresh_immediate_dispatch_ids(&mut self)`, called from there and
from `fill_builtin_classes`. Add `pub(crate) kernel: Option<Value<'gc>>` to
`Mutator` beside `vm_mirror`, `None` in `World::new`, visited in both root
walks. Call `interpreter.install_kernel()` at the end of `bootstrap_with`,
after the stress-mode setup: the kernel's own instantiation is a run like any
other, and the stress modes should see it.

- [ ] **Step 7: Run the unit tests**

Run: `cargo test -p ns-runtime --lib interpreter::kernel`
Expected: PASS. `every_send_to_vm_mirror_in_the_kernel_is_marked` passes
vacuously until Task 5 adds the first send; it is here so that the first one
is checked.

- [ ] **Step 8: Run the gates**

Run every gate. Expected: every golden and every outcome unchanged. Tests that
pin the *mechanism* of a builtin class may now fail — a builtin's mixin having
no declaration, an arena count after `Interpreter::bootstrap`, a builtin's
superclass being the self-referential `Object` class — and are rewritten to
the new shape per Global Constraints. **List every rewritten test in the
commit message**, with one line on what it pinned before and what it pins now.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: fill the builtin classes from Kernel.ns at bootstrap"
```

---

### Task 3: `SendMirror` — a mirror send with its failure block inline

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs`
- Modify: `crates/ns-runtime/src/compiler/control_flow.rs`
- Modify: `crates/ns-runtime/src/compiler/lower.rs`
- Modify: `crates/ns-runtime/src/compiler/capture.rs`
- Modify: `crates/ns-runtime/src/compiler/reference.rs`
- Modify: `crates/ns-runtime/src/interpreter/mirror.rs`
- Modify: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/src/image/encode.rs`, `decode.rs` (exhaustive
  `Instruction` matches only)
- Test: `compiler/control_flow.rs`, `compiler/lower.rs`,
  `compiler/reference.rs` unit tests; `interpreter/mirror.rs` unit tests

**Interfaces:**
- Consumes: `Binding::MirrorSend`, `resolve_kernel`, `mirror_primitive_named`,
  `run_leaf`, `Mutator::is_vm_mirror`.
- Produces: `Instruction::SendMirror { primitive: MirrorPrimitive,
  mirror_register: u16, subject_register: u16, argument_registers: Vec<u16>,
  destination: u16, failure: u32 }`;
  `control_flow::recognize_mirror_send(bindings, send) -> Option<(MirrorPrimitive, &ast::BlockExpression)>`;
  `interpreter::mirror::send_inlined(mutator, primitive, mirror, subject, leaf_arguments) -> Option<Result<Step, Signal>>`.

- [ ] **Step 1: Write the recognizer's failing tests**

In `compiler/control_flow.rs`'s tests, a helper that resolves in kernel mode
and the cases:

```rust
    /// `source` as the body of a method in a kernel-shaped module, resolved
    /// by `resolve_kernel`; answers the send it returns and the bindings.
    fn kernel_send(source: &str) -> (ast::Send, std::rc::Rc<HashMap<Span, Binding>>) {
        let full = formatdoc! {"
            Newspeak3
            'T'
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                foo: argument = ( ^{source} )
              )
            ) : ()
        "};
        let module = ns_syntax::parse(&full).expect("parse");
        let resolved = crate::resolver::resolve_kernel(
            module,
            std::rc::Rc::from(full.as_str()),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let nested = &resolved.module.root_class.instance_side.nested_classes[0];
        let send = match &nested.instance_side.methods[0].body.statements[0] {
            ast::Statement::Return(ast::Expression::Send(send)) => (**send).clone(),
            other => panic!("expected a return-of-send statement, got {other:?}"),
        };
        (send, resolved.bindings.clone())
    }

    #[test]
    fn recognizes_a_mirror_send_with_a_literal_failure_block() {
        let (send, bindings) = kernel_send("vmMirror stringSize: self ifFail: [^nil]");
        let recognized = recognize_mirror_send(&bindings, &send).map(|(row, _)| row.index());
        assert_eq!(recognized, crate::builtins::primitive::mirror_primitive_named("stringSize:ifFail:").map(|row| row.index()));
    }

    #[test]
    fn does_not_recognize_a_mirror_send_whose_failure_argument_is_not_a_literal_block() {
        let (send, bindings) = kernel_send("vmMirror stringSize: self ifFail: argument");
        assert!(recognize_mirror_send(&bindings, &send).is_none());
    }

    #[test]
    fn does_not_recognize_a_selector_the_mirror_does_not_have() {
        let (send, bindings) = kernel_send("vmMirror frobnicate: self ifFail: [^nil]");
        assert!(recognize_mirror_send(&bindings, &send).is_none());
    }

    /// The same text in a module `resolve` resolved is an ordinary send:
    /// only the kernel's `vmMirror` is known to hold the mirror.
    #[test]
    fn does_not_recognize_a_mirror_send_outside_the_kernel() {
        let send = parse_send("vmMirror stringSize: self ifFail: [^nil]");
        assert!(recognize_mirror_send(&HashMap::new(), &send).is_none());
    }
```

Adapt the AST field names (`instance_side.nested_classes`, `.methods[0]`) to
the ones `ns_syntax::ast` has; `parse_send` is the existing helper.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --lib compiler::control_flow`
Expected: FAIL to compile — `recognize_mirror_send` does not exist.

- [ ] **Step 3: Write the recognizer**

In `control_flow.rs`:

```rust
/// A send to the VM mirror whose failure block the optimized lowering lowers
/// inline (`lower.rs`'s `lower_mirror_send`): marked `Binding::MirrorSend` by
/// `resolve_kernel`, its selector a row of `MIRROR` -- which fixes its last
/// keyword as `ifFail:` -- and its last argument a literal block with no
/// parameters and no temporaries, the condition every inlined block meets.
///
/// Asked by the same three places `recognize` is (`lower_send`,
/// `send_writes_register`, `capture::Walker::send`), so the lowering and the
/// capture analysis agree about which block literals are closures.
pub(crate) fn recognize_mirror_send<'a>(
    bindings: &HashMap<Span, Binding>,
    send: &'a ast::Send,
) -> Option<(MirrorPrimitive, &'a ast::BlockExpression)> {
    if !matches!(bindings.get(&send.span), Some(Binding::MirrorSend)) {
        return None;
    }
    let ast::Selector::Keyword(selector) = &send.message.selector else {
        return None;
    };
    let row = crate::builtins::primitive::mirror_primitive_named(selector)?;
    let block = as_inlinable_block(send.message.arguments.last()?)?;
    Some((row, block))
}
```

`MirrorSend` is never on a `super` or lexical send, so a recognized send is
always ordinary; each caller still asks `is_ordinary_send` first, as it does
for `recognize`.

- [ ] **Step 4: The instruction**

In `bytecode.rs`, after `SendSuper`:

```rust
    /// A send to the VM mirror whose failure block was lowered inline, at
    /// `failure` (`docs/design/compiler.md`, "Mirror sends"). Emitted only
    /// by the optimized lowering, only for a `Binding::MirrorSend`, so only
    /// into `Kernel.ns`, which no image carries -- it has no wire tag.
    ///
    /// `primitive` is the row the selector names, bound at compile time:
    /// the mirror's class and method table are built once from the same
    /// table, and nothing can add to or override them. The run-time check
    /// that `mirror_register` holds the mirror is what stands for the
    /// lookup (`interpreter::mirror::send_inlined`). `subject_register` is
    /// the kernel method's `self`, `argument_registers` the leaf's own
    /// arguments; the failure block is not an argument, because it is code
    /// here, not a value.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// `outer_reach_of` does not name it.
    SendMirror {
        primitive: crate::builtins::primitive::MirrorPrimitive,
        mirror_register: u16,
        subject_register: u16,
        argument_registers: Vec<u16>,
        destination: u16,
        failure: u32,
    },
```

Add its `disassemble` arm in the style of `SendPublic`'s, naming the row's
mirror selector and the failure target, e.g.
`SendMirror r3 := r0 stringSize:ifFail: (r1) else 7`. Build, and give each
exhaustive `match` on `Instruction` the compiler then reports an arm:
`outer_reach_of` with the instructions that reach nothing;
`decode.rs`'s destination and verification matches as `SendPublic` is
treated (they are never reached for it — the decoder cannot produce one —
but a match must be total); `encode.rs`'s instruction encoder with

```rust
        Instruction::SendMirror { .. } => unreachable!(
            "internal: SendMirror is emitted only into Kernel.ns, and an image \
             carries only its own module's code"
        ),
```

The encoder's code table is collected from the module's own mixin tree
(`mixins::plan`), which never contains a kernel mixin, so this arm is
unreachable by construction; a panic here is a defect in this process, not a
property of any input.

- [ ] **Step 5: Run it inline, in `interpreter/mirror.rs`**

```rust
/// `Instruction::SendMirror`: the leaf of `primitive`, when `mirror` is the
/// VM mirror; `None` when the leaf does not recognize its subject, which
/// the VM answers by jumping to the inlined failure block.
///
/// `leaf_arguments` are the send's own arguments here -- the instruction
/// passes neither the subject nor the block as one -- so a leaf that
/// activates a block "with the send's arguments" is right as it stands.
pub(crate) fn send_inlined<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    mirror: Value<'gc>,
    subject: Value<'gc>,
    leaf_arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    if !mutator.is_vm_mirror(mirror) {
        return Some(Err(not_the_mirror(primitive, mirror)));
    }
    run_leaf(mutator, primitive, subject, leaf_arguments)
}

/// Only `Kernel.ns` contains a `SendMirror`, and the bootstrap checks that
/// its `vmMirror` holds the mirror, so this is unreachable while that holds.
/// It is an error rather than a panic so that it stays one if it ever does
/// not.
fn not_the_mirror<'gc>(primitive: MirrorPrimitive, receiver: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(mirror_entry(primitive).selector),
        receiver,
        message: "internal: a mirror send's receiver is not the VM mirror".to_string(),
    })
}
```

- [ ] **Step 6: The VM arm**

In `vm.rs`, factor the delivery out of `send` — everything after
`decide(mutator, &arguments)` produced its result — into:

```rust
/// What a send decided, run: a value written to `destination`, or an
/// activation pushed and switched to, its window filled from
/// `send_arguments`.
fn deliver<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    decided: Result<Step<'gc>, Signal<'gc>>,
    send_arguments: &[Value<'gc>],
    destination: u16,
    floor: usize,
    inlinable: bool,
) -> Flow {
    match decided {
        Ok(Step::Value(value)) => {
            write_local(&mut mutator.heap, activation, destination, value);
            Flow::Next
        }
        Ok(step) => match mutator.push_step(step, send_arguments, Some(activation), inlinable) {
            Ok(callee) => Flow::Switch {
                activation: callee,
                code: mutator.heap.activation_code(callee),
                program_counter: 0,
                feedback_base: mutator.heap.activation_feedback_base(callee),
            },
            Err(signal) => unwind(mutator, signal, floor),
        },
        Err(signal) => unwind(mutator, signal, floor),
    }
}
```

with `send` calling it, unchanged in behaviour. Add the arm:

```rust
        // A send to the VM mirror with its failure block inline: the leaf
        // runs, or declines and the block runs at `failure`. What the leaf
        // decided is delivered as any send's decision is.
        Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_registers,
            destination,
            failure,
        } => {
            let mirror = read_local(&mutator.heap, activation, *mirror_register);
            let subject = read_local(&mutator.heap, activation, *subject_register);
            let mut arguments = mutator.take_argument_buffer();
            arguments.extend(
                argument_registers
                    .iter()
                    .map(|register| read_local(&mutator.heap, activation, *register)),
            );
            mutator
                .heap
                .set_activation_program_counter(activation, program_counter);
            let flow = match crate::interpreter::mirror::send_inlined(
                mutator, *primitive, mirror, subject, &arguments,
            ) {
                None => Flow::Jump(*failure as usize),
                Some(decided) => deliver(
                    mutator, activation, decided, &arguments, *destination, floor, false,
                ),
            };
            mutator.return_argument_buffer(arguments);
            flow
        }
```

and add `Instruction::SendMirror { destination, .. }` to `send_destination`,
since a leaf that activates a block (`value:`, `do:`) resumes the caller here.

- [ ] **Step 7: Lower it**

In `lower.rs`, in `lower_send` after the `is_ordinary_send` gate and before
`control_flow::recognize`:

```rust
        if let Some((primitive, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
            return self.lower_mirror_send(send, primitive, block, use_);
        }
```

and the emission, shaped like `lower_integer_binary`, with the failure block
where that one has its fallback send:

```rust
    /// A mirror send with its failure block inline: `SendMirror`, then a jump
    /// over the block, then the block's body lowered into this body -- a `^`
    /// in it returns from the kernel method, as the failure path of a
    /// primitive method's body should.
    ///
    /// The receiver, the subject and the leaf's arguments are lowered in
    /// order, each protected against what is lowered after it, as
    /// `lower_arguments` protects a send's arguments. The block is not
    /// lowered as a value: it is code at `failure`, run only when the leaf
    /// declines.
    fn lower_mirror_send(
        &mut self,
        send: &ast::Send,
        primitive: MirrorPrimitive,
        block: &ast::BlockExpression,
        use_: ValueUse,
    ) -> u16 {
        let arguments = &send.message.arguments;
        let leaf = &arguments[..arguments.len() - 1];
        let mirror_register = self.lower_operand(&send.receiver, leaf);
        let [subject, rest @ ..] = leaf else {
            unreachable!("internal: every mirror selector takes a subject");
        };
        let subject_register = self.lower_operand(subject, rest);
        let argument_registers = self.lower_expressions_protected(rest);
        let destination = self.allocate_scratch();
        let failure_jump = self.emit(Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_registers,
            destination,
            failure: 0,
        });
        let after_send_jump = self.emit(Instruction::Jump { target: 0 });
        let failure_label = self.here();
        self.patch_target(failure_jump, failure_label);
        let value = self.lower_inline_block_body(block, use_);
        self.emit_move_into(destination, value);
        let end_label = self.here();
        self.patch_target(after_send_jump, end_label);
        destination
    }
```

`lower_expressions_protected` stands for what `lower_arguments` does to a
message's argument list — lowering each with `lower_operand` against the ones
after it; if `lower_arguments` takes an `ast::Message`, factor its body into a
function over a slice and call that from both. `emit_move_into` stands for
however `emit_guarded_branch` lands a branch's value in the shared
`destination` (read it and do the same; it may be a `Move` guarded by
`value != destination`, and it must handle a block whose body always returns,
as `[^super size]` does). `patch_target` gains a `SendMirror` arm patching
`failure`.

In `send_writes_register`, before `control_flow::recognize`:

```rust
            if let Some((_, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
                return self.writes_register(&send.receiver, register, inlined_depth)
                    || send.message.arguments[..send.message.arguments.len() - 1]
                        .iter()
                        .any(|argument| self.writes_register(argument, register, inlined_depth))
                    || self.inlined_block_writes_register(block, register, inlined_depth);
            }
```

using whatever `inlined_form_writes_register` uses to ask the question of one
inlined block, one resolver scope deeper.

In `capture.rs`, `Walker::send`, before `control_flow::recognize`:

```rust
            if let Some((_, block)) = control_flow::recognize_mirror_send(self.bindings, send) {
                self.expression(&send.receiver);
                for argument in &send.message.arguments[..send.message.arguments.len() - 1] {
                    self.expression(argument);
                }
                self.inlined_block(block, true, None);
                return;
            }
```

`conditional: true`, because the block runs at most once and only on one
path, exactly as an `ifTrue:` block.

In `reference.rs`, a send `control_flow::recognize_mirror_send` accepts is
emitted as `SendInlinable` rather than `SendPublic`, with its block a real
closure as every block there is: "SendInlinable is an ordinary public send
that only the reference lowering emits, for exactly the sends the optimized
lowering inlines", and the mirror's failure step is marked `ControlFlow`, so
the depth count exempts the block's activation under the same condition as
`ifTrue:`'s. Find where `reference.rs` asks `control_flow::recognize` to choose
`SendInlinable` and add the mirror form to that condition.

- [ ] **Step 8: Test the lowering**

In `lower.rs`'s tests, resolve a kernel-shaped module with `resolve_kernel`
(copy the helper from Step 1, or share one in a `#[cfg(test)]` module both use)
and lower `foo: argument = ( ^vmMirror string: self concatenate: argument ifFail: [^nil] )`.
Assert the instruction sequence: the receiver's lexical send of `vmMirror`,
`SendMirror` with `subject_register` holding `self` and one argument register
holding `argument`, a `Jump` to the end, then the block's lowering at the
`failure` target. Assert `family_size` counts no call site for the
`SendMirror` itself (it has none). In `reference.rs`'s tests, assert the same
send compiles to `MakeClosure` then `SendInlinable`.

In `interpreter/mirror.rs`'s tests, run it end to end: resolve a kernel-shaped
fixture with `resolve_kernel`, load it into a bootstrapped interpreter,
instantiate its root class with the VM mirror as the factory argument, and
send the nested class's method:

```rust
    #[test]
    fn an_inlined_mirror_send_runs_its_leaf_and_its_failure_block() {
        let source = indoc::indoc! {"
            Newspeak3
            'T'
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class Probe = Object new () (
                public sizeOf: text = ( ^vmMirror stringSize: text ifFail: [^-1] )
              )
              public probe = ( ^Probe new )
            ) : ()
        "};
        // resolve with crate::resolver::resolve_kernel, load_module,
        // instantiate with the_mirror(&mut interpreter), send #probe, then
        // #sizeOf: with 'abc' (expect 3) and with nil (expect -1), under
        // Interpreter::bootstrap() and under
        // Interpreter::bootstrap_with(Lowering::Reference) alike.
    }
```

Write it out in full, both lowerings in one loop.

- [ ] **Step 9: Run the gates, then commit**

Expected: green; no golden moves (no kernel method exists yet).

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: lower a mirror send's failure block inline"
```

---

### Task 4: Primitive methods, and the measurement switch

**Files:**
- Create: `crates/ns-runtime/src/compiler/primitive_method.rs`
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (module declaration)
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinMethod.primitive`,
  `from_declaration`)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (a decoded method's
  `primitive: None`)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`act_with_base`,
  `act_handler_method`)
- Modify: `crates/ns-runtime/src/interpreter/mirror.rs` (`execute_method`)
- Test: `compiler/primitive_method.rs`, `interpreter/mirror.rs`

**Interfaces:**
- Consumes: `Binding::MirrorSend`, `mirror_primitive_named`, `run_leaf`.
- Produces: `MixinMethod.primitive: Option<MirrorPrimitive>`;
  `primitive_method::recognize(declaration, bindings) -> Option<MirrorPrimitive>`;
  `primitive_method::disabled() -> bool`;
  `interpreter::mirror::execute_method(mutator, primitive, receiver, arguments) -> Option<Result<Step, Signal>>`.

- [ ] **Step 1: Write the recognizer's failing tests**

In the new `compiler/primitive_method.rs`, tests over a kernel-shaped fixture
(resolved with `resolve_kernel`) whose nested class declares these methods,
each asserting what `recognize` answers:

| Method | Answer |
|---|---|
| `size = ( ^vmMirror stringSize: self ifFail: [^super size] )` | row of `stringSize:ifFail:` |
| `, other = ( ^vmMirror string: self concatenate: other ifFail: [^super , other] )` | row of `string:concatenate:ifFail:` |
| `at: index put: value = ( ^vmMirror collection: self at: index put: value ifFail: [^nil] )` | row of `collection:at:put:ifFail:` |
| `at: index put: value = ( ^vmMirror collection: self at: value put: index ifFail: [^nil] )` | `None` — parameters out of order |
| `size = ( ^vmMirror stringSize: 3 ifFail: [^nil] )` | `None` — subject is not `self` |
| `size = ( | temporary | ^vmMirror stringSize: self ifFail: [^nil] )` | `None` — a temporary |
| `size = ( self foo. ^vmMirror stringSize: self ifFail: [^nil] )` | `None` — more than one statement |
| `size = ( vmMirror stringSize: self ifFail: [^nil] )` | `None` — no `^` |
| `size = ( ^vmMirror stringSize: self ifFail: [:reason | ^nil] )` | `None` — the failure block takes a parameter |
| `, other = ( ^vmMirror stringSize: self ifFail: [^nil] )` | `None` — the row takes no argument, the method one |

And one over a module resolved with plain `resolve`, the first method's text,
answering `None`: outside the kernel there is no `MirrorSend`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --lib compiler::primitive_method`
Expected: FAIL to compile.

- [ ] **Step 3: Write the recognizer**

```rust
//! Primitive methods (`docs/design/dispatch.md`, "Primitive methods"): a
//! kernel method whose whole body forwards `self` and its own parameters to
//! one mirror row runs that row's leaf when it is sent, without an
//! activation, and is activated -- its body running the failure block --
//! only when the leaf declines. Cog's primitive methods, with the primitive
//! recognized from a body that means exactly what it says rather than
//! declared by a pragma that replaces what the body says.

use std::collections::HashMap;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::builtins::primitive::MirrorPrimitive;
use crate::compiler::control_flow;
use crate::types::Binding;

/// The row `declaration` forwards to, when its body is exactly
/// `^vmMirror <row>: self <its parameters, in order> ifFail: [<anything>]`
/// and the send is a recognized mirror send (`recognize_mirror_send`, which
/// only `resolve_kernel`'s bindings can satisfy). Anything else -- a
/// temporary, a second statement, a subject other than `self`, a parameter
/// out of place, an arity the row does not have -- is an ordinary method.
pub(crate) fn recognize(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
) -> Option<MirrorPrimitive> {
    if disabled() {
        return None;
    }
    let body = &declaration.body;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return None;
    }
    let ast::Statement::Return(ast::Expression::Send(send)) = &body.statements[0] else {
        return None;
    };
    let (row, _) = control_flow::recognize_mirror_send(bindings, send)?;
    let arguments = &send.message.arguments;
    let forwarded = &arguments[..arguments.len() - 1];
    let [subject, rest @ ..] = forwarded else {
        return None;
    };
    if !is_self(subject) {
        return None;
    }
    let parameters = parameter_names(&declaration.pattern);
    if rest.len() != parameters.len() {
        return None;
    }
    let in_order = rest.iter().zip(&parameters).enumerate().all(|(index, (argument, name))| {
        matches!(argument, ast::Expression::Identifier(identifier) if identifier.name == *name)
            && matches!(bindings.get(&argument_span(argument)), Some(Binding::Local(local)) if *local as usize == index)
    });
    in_order.then_some(row)
}
```

`is_self`, `parameter_names` and `argument_span` are a few lines each over
`ns_syntax::ast`; `rest.len() == parameters.len()` together with the row
being named by the selector already fixes the row's leaf arity to the
method's. The binding check makes sure a parameter's name was not shadowed by
something the resolver bound otherwise; a parameter's register is
`Binding::Local(position)` — confirm against `lower.rs`'s register layout
("`argument_count`, then …") and adjust if the index is offset.

The measurement switch:

```rust
/// `NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without primitive
/// methods, so a measurement can run the same binary with and without them
/// (`docs/design/execution.md`, "What the core library's move cost"). Read
/// once per process. **For measuring only**: without primitive methods the
/// two lowerings no longer count activations alike
/// (`docs/design/compiler.md`, "How the limit counts"), so the goldens'
/// overflow comparisons are not valid under it.
pub(crate) fn disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("NS_NEXT_NO_PRIMITIVE_METHODS").is_ok_and(|value| value == "1")
    })
}
```

Read in release builds too — it is what the measurement needs — and read only
when a kernel method is compiled, never on a send.

- [ ] **Step 4: Carry it on the method**

`MixinMethod` gains, after `code`:

```rust
    /// The mirror row this method forwards to, when it is a primitive
    /// method (`compiler::primitive_method`): `act` runs the row's leaf
    /// first and activates the method only when the leaf declines. `None`
    /// for every method outside the kernel, and for every method decoded
    /// from an image, which never carries a kernel method.
    pub primitive: Option<MirrorPrimitive>,
```

`from_declaration` sets it from `primitive_method::recognize(&declaration,
bindings)`; `image::mixins`' decoded methods set `None`; every other
construction site the compiler reports sets `None`.

- [ ] **Step 5: Run it in `act`**

In `interpreter/mirror.rs`:

```rust
/// A primitive method's leaf, on the send's own receiver and arguments --
/// the method forwards exactly those, which `primitive_method::recognize`
/// checked. `None` when the leaf declines, and the method is then activated
/// as any method is.
#[inline(always)]
pub(crate) fn execute_method<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    crate::builtins::execute(mutator, mirror_entry(primitive).primitive, receiver, selector, arguments)
}
```

`selector` is the send's, which is the method's own selector — and so the row's
reported selector — for every send but a `doesNotUnderstand:` handler's, which
passes `Symbol::DOES_NOT_UNDERSTAND` as today.

In `resolve.rs`, `act_with_base`'s `Resolved::Method` arm, the non-handler
case becomes:

```rust
                _ => {
                    if let Some(primitive) = method.primitive {
                        if let Some(result) = crate::interpreter::mirror::execute_method(
                            self, primitive, receiver, selector, arguments,
                        ) {
                            return result;
                        }
                    }
                    Ok(Step::Method {
                        receiver,
                        method,
                        defining_class,
                        arguments: StepArguments::Send,
                        feedback_base,
                    })
                }
```

and `act_handler_method` tries the same before building the handler's
activation, with the message as the one argument:

```rust
        let message = self.handler_message(selector, arguments);
        if let Some(primitive) = method.primitive {
            if let Some(result) = crate::interpreter::mirror::execute_method(
                self,
                primitive,
                receiver,
                Symbol::DOES_NOT_UNDERSTAND,
                std::slice::from_ref(&message),
            ) {
                return result;
            }
        }
```

A cache hit reaches this through `act` like a miss, so the caches need no
change: they record `Target::Method`, and the fast path is read off the method
on every send. This is one field test on every method send's path — measured
in Task 15, and the reason the field lives on `MixinMethod`, which the arm
already holds, rather than in a new `Resolved` variant a cache would have to
learn.

- [ ] **Step 6: Test it end to end**

In `interpreter/mirror.rs`'s tests, extend Task 3's kernel-shaped fixture's
`Probe` with two primitive methods — one whose leaf accepts any receiver, one
whose leaf never accepts a `Probe`:

```newspeak
                public same: other = ( ^vmMirror object: self identicalTo: other ifFail: [^-1] )
                public size = ( ^vmMirror stringSize: self ifFail: [^-1] )
```

and assert under both lowerings: both methods are marked
(`primitive.is_some()` on the compiled `MixinMethod`, read off `Probe`'s
application); `aProbe same: aProbe` answers `true` from the leaf; `aProbe size`
answers `-1`, the failure block's value, through the activated body. And a
test that the same methods in a module resolved with plain `resolve` carry
`None`. That a successful primitive method pushes no activation is pinned
where it matters, by the next step's golden.

- [ ] **Step 7: Pin the depth count across the move**

The families move from Task 5 on, and "Why the fast path comes before the
control-flow family" says what would break if the fast path did not hold:
the reference lowering would count activations the optimized lowering does
not, and an overflow would come at different depths. Pin it with a golden
that recurses through inlinable `ifTrue:ifFalse:` sends, a `printString`, a
`show:` and integer arithmetic — every family the recursion's frames touch —
and prints its progress, so that the output *before* the overflow shows the
depth it was reached at. Create
`goldens/kernel/recursion_through_the_core_library_counts_alike_under_both_lowerings.ns`:

```newspeak
Newspeak3
'Kernel'
(* Every level of this recursion sends ifTrue:ifFalse: with literal blocks
   -- inlined by the optimized lowering, a SendInlinable under the reference
   lowering -- and reaches printString, show: and + through the core library.
   The reference lowering is held to the same count of activations as the
   optimized one (`docs/design/compiler.md`, "How the limit counts"), so both
   print the same progress before the same overflow. That holds with the
   core library in Newspeak only because a primitive method runs its leaf
   without an activation of its own. *)
class DeepThroughTheKernel usingPlatform: platform = Object new (
    | transcript = platform transcript. |
) (
    down: depth = (
        depth = (depth // 20000 * 20000)
            ifTrue: [transcript show: depth printString; newLine]
            ifFalse: [nil].
        ^depth < 0 ifTrue: [0] ifFalse: [down: depth + 1]
    )
    public main: p args: a = ( down: 0 )
) : ()
```

**Record the output `e2bb31e` produces for this file**, under both lowerings,
before any kernel existed: check out `e2bb31e` in a scratch worktree
(`git worktree add <scratch> e2bb31e`), run
`cargo run -p ns-cli -- run <file> --send main:args:` there with
`--lowering optimized` and `--lowering reference`, confirm the two agree, and
write that `status`, `stdout` and `stderr` into the `.toml` beside the golden,
in the format the other goldens use. Then run it on this branch, under both
lowerings: it must match. Remove the scratch worktree afterwards.

- [ ] **Step 8: Run the gates, then commit**

Expected: green; no golden moves.

```bash
cargo fmt --all
git add -A crates/ns-runtime goldens/kernel
git commit -m "feat: run a primitive method's leaf before activating it"
```

---

## Tasks 5 to 12: moving a family

Each of the next eight tasks moves one family, and each follows the same five
steps, written out in full in every task so that a task can be read on its
own:

1. Add the family's forwarders to its class in `Kernel.ns`, exactly as the
   task gives them.
2. Empty the family's list in `builtins/primitive.rs` — `primitives_of` answers
   `&[]` for its kinds — so the bootstrap's merge (Task 2) has nothing left to
   add for them and every send reaches the kernel method.
3. Run the whole suite. **No golden, no `run-corpus` row and no outcome
   assertion may change.** Tests that pin the mechanism — a `MethodEntry::Primitive`
   or a `Lookup::Primitive` on the family's class, a `CachedTargetKind::Primitive`
   at a site that sends to it — are rewritten to pin the new mechanism: a
   `MethodEntry::Compiled` whose `MixinMethod.primitive` is the row, a
   `Lookup::Method`, a `CachedTargetKind::Method`. Keep the program and the
   answer each test checks.
4. Run every gate.
5. Commit, listing each rewritten test in the message.

If an outcome does change, the forwarder is wrong — a wrong row, parameters
out of order, a missing method — or the kernel method is not recognized as a
primitive method and is being activated where it should not be. Find which;
never re-record.

---

### Task 5: `Object`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Object`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`OBJECT`)
- Test: mechanism tests in `crates/ns-runtime/src/dispatch.rs`
  (`a_primitive_a_builtin_lacks_is_found_at_objects_level`,
  `objects_does_not_understand_is_protected`,
  `a_lookup_continued_above_a_level_passes_that_levels_entry_over`) and any
  other the suite reports

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: `Object`'s six methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

In `Kernel.ns`, `class Object = () ( … )` becomes:

```newspeak
    class Object = () (
        public class = ( ^vmMirror objectClass: self ifFail: [^super class] )
        public == other = ( ^vmMirror object: self identicalTo: other ifFail: [^super == other] )
        public = other = ( ^vmMirror object: self equals: other ifFail: [^super = other] )
        public hash = ( ^vmMirror objectHash: self ifFail: [^super hash] )
        public printString = ( ^vmMirror objectPrintString: self ifFail: [^super printString] )
        (* §5.5: protected, so an explicit-receiver send of it finds nothing
           through lookupPublic and falls to the access-blind "defined for"
           walk that invokes it for the failing send itself. *)
        protected doesNotUnderstand: message = ( ^vmMirror object: self doesNotUnderstand: message ifFail: [^super doesNotUnderstand: message] )
    )
```

`Object`'s leaves never decline, so these failure blocks never run; they keep
the one shape every kernel method has.

- [ ] **Step 2: Empty the list**

`const OBJECT: &[PrimitiveSelector] = &[];`, keeping the comment about §5.5
beside the kernel method now instead (it moved in Step 1).

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden and outcome unchanged; the three `dispatch.rs` tests
above fail on their `Lookup::Primitive` patterns. Rewrite each to expect
`Lookup::Method(method, level)` with `method.primitive` naming the row
(`mirror_primitive_named("object:identicalTo:ifFail:")` for `==`,
`object:doesNotUnderstand:ifFail:` for the handler, `object:equals:ifFail:`
for `=`), and the same level. `a_lookup_continued_above_a_level_passes_that_levels_entry_over`
still finds `String`'s `=` primitive until Task 9 moves `String`; only its
`Object` expectation changes here.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move Object's methods into Kernel.ns"
```

---

### Task 6: `Boolean`, `True` and `False`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Boolean`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`BOOLEAN`, and
  `primitives_of`'s arm for `Boolean | True | False`)
- Test: mechanism tests the suite reports (`tests/builtins_boolean_numbers.rs`,
  `tests/inline_caches.rs`, `tests/trampoline.rs` are the likely ones)

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: `Boolean`'s ten methods as kernel primitive methods, inherited by
  `True` and `False`, which declare none.

- [ ] **Step 1: Add the forwarders**

`class Boolean = Object new () ( … )` becomes:

```newspeak
    class Boolean = Object new () (
        public ifTrue: trueBlock ifFalse: falseBlock = ( ^vmMirror boolean: self ifTrue: trueBlock ifFalse: falseBlock ifFail: [^super ifTrue: trueBlock ifFalse: falseBlock] )
        public ifFalse: falseBlock ifTrue: trueBlock = ( ^vmMirror boolean: self ifFalse: falseBlock ifTrue: trueBlock ifFail: [^super ifFalse: falseBlock ifTrue: trueBlock] )
        public ifTrue: trueBlock = ( ^vmMirror boolean: self ifTrue: trueBlock ifFail: [^super ifTrue: trueBlock] )
        public ifFalse: falseBlock = ( ^vmMirror boolean: self ifFalse: falseBlock ifFail: [^super ifFalse: falseBlock] )
        public not = ( ^vmMirror booleanNot: self ifFail: [^super not] )
        public & other = ( ^vmMirror boolean: self logicalAnd: other ifFail: [^super & other] )
        public and: alternative = ( ^vmMirror boolean: self and: alternative ifFail: [^super and: alternative] )
        public | other = ( ^vmMirror boolean: self logicalOr: other ifFail: [^super | other] )
        public or: alternative = ( ^vmMirror boolean: self or: alternative ifFail: [^super or: alternative] )
        public printString = ( ^vmMirror booleanPrintString: self ifFail: [^super printString] )
    )
```

`True` and `False` keep empty bodies: their instances are `Value::Boolean`s,
which the Boolean leaves recognize whichever of the two they are. The
arguments keep the order the send gave them — `ifFalse:ifTrue:`'s leaf reads
its false block first, as `BooleanPrimitive::IfFalseIfTrue` does today.

- [ ] **Step 2: Empty the list**

`const BOOLEAN: &[PrimitiveSelector] = &[];`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden and outcome unchanged — in particular every
`goldens/capture/*fallback*` golden, the `control_flow_fallback` and
`variable_and_block_coerces_its_value_to_a_boolean` goldens, and Task 4's
depth-count golden, under both lowerings. Rewrite the mechanism tests the
suite reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move Boolean's methods into Kernel.ns"
```

---

### Task 7: `Integer`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Integer`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`INTEGER`)
- Test: mechanism tests the suite reports, including `dispatch.rs`'s
  `a_builtins_own_primitive_is_found_at_its_own_level`

**Interfaces:**
- Consumes: Tasks 1–6.
- Produces: `Integer`'s thirteen methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    class Integer = Object new () (
        public + other = ( ^vmMirror integer: self add: other ifFail: [^super + other] )
        public - other = ( ^vmMirror integer: self subtract: other ifFail: [^super - other] )
        public * other = ( ^vmMirror integer: self multiply: other ifFail: [^super * other] )
        public // other = ( ^vmMirror integer: self floorDivide: other ifFail: [^super // other] )
        public < other = ( ^vmMirror integer: self lessThan: other ifFail: [^super < other] )
        public > other = ( ^vmMirror integer: self greaterThan: other ifFail: [^super > other] )
        public <= other = ( ^vmMirror integer: self lessOrEqual: other ifFail: [^super <= other] )
        public >= other = ( ^vmMirror integer: self greaterOrEqual: other ifFail: [^super >= other] )
        public = other = ( ^vmMirror integer: self equals: other ifFail: [^super = other] )
        public negated = ( ^vmMirror integerNegated: self ifFail: [^super negated] )
        public abs = ( ^vmMirror integerAbsolute: self ifFail: [^super abs] )
        public printString = ( ^vmMirror integerPrintString: self ifFail: [^super printString] )
        public asString = ( ^vmMirror integerAsString: self ifFail: [^super asString] )
    )
```

No `/` (Decision 2). Inside these bodies `+` is a mirror send, not an
arithmetic send, so `IntegerBinary` never applies to them; everywhere else it
still guards `+` and falls back to the send that now reaches this method.

- [ ] **Step 2: Empty the list**

`const INTEGER: &[PrimitiveSelector] = &[];`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged — the `arithmetic` category, and the two
`errors/inlined_integer_*` goldens whose error texts name `+` and the zero
divisor, in particular. Rewrite `a_builtins_own_primitive_is_found_at_its_own_level`
to expect `Lookup::Method` on `Integer`'s own level whose method carries the
`integer:add:ifFail:` row, and any other mechanism test the suite reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move Integer's methods into Kernel.ns"
```

---

### Task 8: `Float`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Float`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`FLOAT`)
- Test: mechanism tests the suite reports

**Interfaces:**
- Consumes: Tasks 1–7.
- Produces: `Float`'s nine methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    class Float = Object new () (
        public + other = ( ^vmMirror float: self add: other ifFail: [^super + other] )
        public - other = ( ^vmMirror float: self subtract: other ifFail: [^super - other] )
        public * other = ( ^vmMirror float: self multiply: other ifFail: [^super * other] )
        public / other = ( ^vmMirror float: self divide: other ifFail: [^super / other] )
        public < other = ( ^vmMirror float: self lessThan: other ifFail: [^super < other] )
        public > other = ( ^vmMirror float: self greaterThan: other ifFail: [^super > other] )
        public = other = ( ^vmMirror float: self equals: other ifFail: [^super = other] )
        public printString = ( ^vmMirror floatPrintString: self ifFail: [^super printString] )
        public asString = ( ^vmMirror floatAsString: self ifFail: [^super asString] )
    )
```

- [ ] **Step 2: Empty the list**

`const FLOAT: &[PrimitiveSelector] = &[];`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged; rewrite the mechanism tests the suite
reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move Float's methods into Kernel.ns"
```

---

### Task 9: `String`, `Symbol` and `Character`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (classes `String`, `Symbol`,
  `Character`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`STRING`, `SYMBOL`,
  `CHARACTER`)
- Test: mechanism tests the suite reports (`tests/builtins_text.rs`, and
  `dispatch.rs`'s `a_lookup_continued_above_a_level_passes_that_levels_entry_over`,
  whose `String` half now finds a kernel method)

**Interfaces:**
- Consumes: Tasks 1–8.
- Produces: the three text families' eleven methods as kernel primitive
  methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    class String = Object new () (
        public size = ( ^vmMirror stringSize: self ifFail: [^super size] )
        public , other = ( ^vmMirror string: self concatenate: other ifFail: [^super , other] )
        public = other = ( ^vmMirror string: self equals: other ifFail: [^super = other] )
        public asSymbol = ( ^vmMirror stringAsSymbol: self ifFail: [^super asSymbol] )
        public printString = ( ^vmMirror stringPrintString: self ifFail: [^super printString] )
    )
    class Symbol = Object new () (
        public asString = ( ^vmMirror symbolAsString: self ifFail: [^super asString] )
        public = other = ( ^vmMirror symbol: self equals: other ifFail: [^super = other] )
        public printString = ( ^vmMirror symbolPrintString: self ifFail: [^super printString] )
    )
    class Character = Object new () (
        public asInteger = ( ^vmMirror characterAsInteger: self ifFail: [^super asInteger] )
        public asString = ( ^vmMirror characterAsString: self ifFail: [^super asString] )
        public = other = ( ^vmMirror character: self equals: other ifFail: [^super = other] )
    )
```

`String`'s failure blocks are the ones the two failing-primitive goldens run:
an instance of `Text = String new` carries no string, so `stringSize:` and
`string:equals:` and `stringPrintString:` decline, and `super` reaches
`Object`'s `=` and `printString` or, for `size`, `Object`'s
`doesNotUnderstand:`.

- [ ] **Step 2: Empty the lists**

`STRING`, `SYMBOL` and `CHARACTER` become `&[]`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged —
`errors/native_that_fails_on_its_receiver_falls_to_objects` and
`cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class` above all,
under both lowerings. Rewrite the mechanism tests the suite reports; in
`a_lookup_continued_above_a_level_passes_that_levels_entry_over`, the premise
("a failed primitive passes its level over") no longer describes a builtin
class, since `String`'s `=` is a method whose failure path is its own body:
rewrite it to pin what now stands in its place — `String`'s `=` is a primitive
method carrying `string:equals:ifFail:` — and leave the pass-over itself to
Task 13, which removes it.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move the text classes' methods into Kernel.ns"
```

---

### Task 10: `Array` and `OrderedCollection`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (classes `Array`,
  `OrderedCollection`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ARRAY`,
  `ORDERED_COLLECTION`)
- Test: mechanism tests the suite reports (`tests/builtins_collections_block.rs`,
  `tests/factory_send.rs`'s entry-kind check near line 77)

**Interfaces:**
- Consumes: Tasks 1–9.
- Produces: the collection methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    class Array = Object new () (
        public do: action = ( ^vmMirror collection: self do: action ifFail: [^super do: action] )
        public at: index put: value = ( ^vmMirror collection: self at: index put: value ifFail: [^super at: index put: value] )
        public size = ( ^vmMirror collectionSize: self ifFail: [^super size] )
        public at: index = ( ^vmMirror collection: self at: index ifFail: [^super at: index] )
    )
    class OrderedCollection = Object new () (
        public do: action = ( ^vmMirror collection: self do: action ifFail: [^super do: action] )
        public add: element = ( ^vmMirror collection: self add: element ifFail: [^super add: element] )
        public size = ( ^vmMirror collectionSize: self ifFail: [^super size] )
        public at: index = ( ^vmMirror collection: self at: index ifFail: [^super at: index] )
    )
```

`do:` stays today's trampoline loop, reached as a leaf (Decision 7): the
primitive method answers the first element's block activation straight to the
sender, and the loop's continuation runs the rest, so a `do:` costs what it
cost before this plan. Its factory is unchanged: `decide_factory` still reads
`ClassData.builtin` off the class object to build an `Array` payload.

- [ ] **Step 2: Empty the lists**

`ARRAY` and `ORDERED_COLLECTION` become `&[]`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged —
`limits/recursion_through_do_is_bounded_by_the_activation_limit` among them —
and every `run-corpus` row. Rewrite the mechanism tests the suite reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move the collection classes' methods into Kernel.ns"
```

---

### Task 11: `Block`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Block`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`BLOCK`)
- Test: mechanism tests the suite reports (`tests/trampoline.rs`,
  `tests/builtins_collections_block.rs`)

**Interfaces:**
- Consumes: Tasks 1–10.
- Produces: the block methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    class Block = Object new () (
        public value = ( ^vmMirror blockValue: self ifFail: [^super value] )
        public value: first = ( ^vmMirror block: self value: first ifFail: [^super value: first] )
        public value: first value: second = ( ^vmMirror block: self value: first value: second ifFail: [^super value: first value: second] )
        public value: first value: second value: third = ( ^vmMirror block: self value: first value: second value: third ifFail: [^super value: first value: second value: third] )
        public numArgs = ( ^vmMirror blockNumArgs: self ifFail: [^super numArgs] )
        public whileTrue: body = ( ^vmMirror block: self whileTrue: body ifFail: [^super whileTrue: body] )
        public whileFalse: body = ( ^vmMirror block: self whileFalse: body ifFail: [^super whileFalse: body] )
    )
```

A block's activation is the leaf's answer, `Step::Block` with the send's own
arguments, which the primitive method hands on as they are: the method's
arguments *are* the block's.

- [ ] **Step 2: Empty the list**

`const BLOCK: &[PrimitiveSelector] = &[];`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged —
`errors/if_block_that_takes_an_argument_is_a_wrong_arity_error`,
`errors/non_boolean_while_condition_fails_with_must_be_boolean` and
`errors/variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean`
among them. Rewrite the mechanism tests the suite reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move Block's methods into Kernel.ns"
```

---

### Task 12: `Platform`

**Files:**
- Modify: `crates/ns-runtime/kernel/Kernel.ns` (class `Platform`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`PLATFORM`)
- Modify: `crates/ns-runtime/src/platform.rs` (module doc: how its leaves are
  reached now)
- Test: mechanism tests the suite reports

**Interfaces:**
- Consumes: Tasks 1–11.
- Produces: the platform methods as kernel primitive methods.

- [ ] **Step 1: Add the forwarders**

```newspeak
    (* The platform object, its transcript and its collections namespace share
       this class and are told apart by payload shape (`platform.rs`), so a
       leaf declines a platform object that is not the one it serves, and the
       failure block's `super` send ends in Object's doesNotUnderstand:. *)
    class Platform = Object new () (
        public transcript = ( ^vmMirror platformTranscript: self ifFail: [^super transcript] )
        public Object = ( ^vmMirror platformObject: self ifFail: [^super Object] )
        public Array = ( ^vmMirror platformArray: self ifFail: [^super Array] )
        public OrderedCollection = ( ^vmMirror platformOrderedCollection: self ifFail: [^super OrderedCollection] )
        public String = ( ^vmMirror platformString: self ifFail: [^super String] )
        public Symbol = ( ^vmMirror platformSymbol: self ifFail: [^super Symbol] )
        public collections = ( ^vmMirror platformCollections: self ifFail: [^super collections] )
        public List = ( ^vmMirror platformList: self ifFail: [^super List] )
        public show: text = ( ^vmMirror platform: self show: text ifFail: [^super show: text] )
        public newLine = ( ^vmMirror platformNewLine: self ifFail: [^super newLine] )
    )
```

These methods are members of `Platform`, so inside `Platform`'s own body the
bare names `Object`, `Array` and the rest would resolve to them rather than to
the kernel's classes; nothing in `Platform` uses those names bare, and nothing
may.

`platform.rs` keeps its role: §8.3 separates the platform, which is the
environment, from the VM mirror, which is the virtual machine. Its leaves are
reached through the mirror now; say so in its module doc, and keep the
whitelist rule ("an accessor exists because a module on `corpus/runnable.toml`
needs it") as it is.

- [ ] **Step 2: Empty the list**

`const PLATFORM: &[PrimitiveSelector] = &[];`.

- [ ] **Step 3: Run the suite**

Run: `cargo test --workspace`
Expected: every golden unchanged — every golden prints through
`platform transcript show:`, so this is the family every golden exercises.
Rewrite the mechanism tests the suite reports.

- [ ] **Step 4: Run every gate**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: move the platform's methods into Kernel.ns"
```

---

### Task 13: Remove what the move left behind, and pin the fall-through

After Task 12 no builtin class but `VmMirror` has a primitive entry, and the
only primitive entries are the mirror's, which never pass their level over.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (the per-family lists,
  `PrimitiveSelector`, `primitives_of`; `method_table` becomes the mirror's
  alone)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (the merge)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`builtin_mixin`)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`act`'s pass-over,
  `act_above`, `act_handler_primitive`)
- Create: `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns`
  and its `.toml`
- Test: the suite

**Interfaces:**
- Consumes: Tasks 1–12.
- Produces: `builtins::primitive::mirror_method_table() -> IdentityMap<Symbol, MethodEntry>`
  in place of `method_table(kind)`.

- [ ] **Step 1: Delete the empty lists and the merge**

Delete `OBJECT` … `PLATFORM`, `PrimitiveSelector`, `public`, `protected`,
`primitives_of` and the `no_builtin_lists_a_selector_twice` test (its
successor is Task 1's `every_mirror_selector_is_unique_…`). Rename
`method_table` to `mirror_method_table()`, keeping only the `VmMirror` body.
`builtin_mixin(kind)` builds `VmMirror`'s mixin from `mirror_method_table()`
and every other kind's placeholder mixin with an empty table, documented as a
placeholder the kernel phase replaces before anything runs. Delete
`merged_with_remaining_primitives` and use the nested mixin itself.

- [ ] **Step 2: Delete the pass-over**

`act`'s `Resolved::Primitive` arm runs only mirror rows now, whose
`builtins::execute` answer is always `Some`. Replace the `None =>
self.act_above(…)` arm with an `Err(unhandled_error(receiver, selector))`
arm, documented: a primitive entry is a mirror row and a mirror row's failure
path is its block, so no primitive passes its level over any more; the arm is
what keeps that an error rather than a panic if it ever stopped holding.
Delete `act_above`. In `act_handler_primitive`, a `doesNotUnderstand:` handler
cannot be a primitive — `Object`'s is a kernel method, and the mirror answers
no `doesNotUnderstand:` of its own — so reduce it to the same unhandled error,
or delete it and route its caller's `Chain::DoesNotUnderstand` primitive arm
to that error; choose whichever leaves `act` smaller.

Check the comments that described the pass-over — `resolve.rs`, `dispatch.rs`
(`lookup_above`'s doc; delete `lookup_above` if nothing calls it any more),
`inline_cache.rs`, `builtins/mod.rs`'s `execute` doc ("`None` is a primitive
failure") — and bring each up to date: a leaf that declines answers `None` to
the mirror code, which runs the failure block.

- [ ] **Step 3: Pin the fall-through, family by family**

The two existing failing-primitive goldens cover `String` and `Platform`.
Create `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns`
covering the rest:

```newspeak
Newspeak3
'Kernel'
(* A user subclass of a builtin inherits its kernel methods but none of its
   payload, so each leaf those methods forward to declines, and the failure
   block's `super` send reaches Object's method: `=` is identity and
   `printString` the generic default, family by family. *)
class FallThrough usingPlatform: platform = Object new (
    | transcript = platform transcript. |
) (
    class MyBoolean = Boolean new () ()
    class MyInteger = Integer new () ()
    class MyFloat = Float new () ()
    class MySymbol = Symbol new () ()
    class MyCharacter = Character new () ()
    class MyArray = Array new () ()
    class MyOrderedCollection = OrderedCollection new () ()
    class MyBlock = Block new () ()
    report: instance = (
        transcript show: (instance = instance) printString; show: ' '; show: instance printString; newLine
    )
    public main: p args: a = (
        report: MyBoolean new.
        report: MyInteger new.
        report: MyFloat new.
        report: MySymbol new.
        report: MyCharacter new.
        report: MyArray new.
        report: MyOrderedCollection new.
        report: MyBlock new.
        transcript show: MyArray new size printString; newLine
    )
) : ()
```

**Record the output `e2bb31e` produces for this file** under both lowerings,
in a scratch worktree as in Task 4 Step 7, and check this branch matches it.
The last line ends the run in `doesNotUnderstand: #size`, which is what
`e2bb31e` does too.

Then pin Decision 9, the one observable difference, in a golden of its own —
**not** recorded from `e2bb31e`, which answered differently, but established
from §5.10 and checked to agree under both lowerings. Create
`goldens/kernel/a_builtin_subclass_handler_is_passed_by_as_section_5_10_says.ns`:

```newspeak
Newspeak3
'Kernel'
(* A user subclass of Array that answers doesNotUnderstand: itself. Its
   instances carry no array, so Array>>size's leaf declines, and the failure
   block's `super size` finds no size in Object. §5.10: "the
   #doesNotUnderstand: method defined for S is invoked" -- S being Object,
   the superclass of the class declaring the super send -- so Object's
   handler runs, not the one declared here, and the run ends in Object's
   error. Before the core library moved into Kernel.ns a declining primitive
   continued the receiver's own lookup and reached this handler instead
   (`docs/design/core-library.md`, "The one observable difference"). *)
class HandlerPassedBy usingPlatform: platform = Object new (
    | transcript = platform transcript. |
) (
    class Handled = Array new () (
        protected doesNotUnderstand: message = ( ^'handled here' )
    )
    public main: p args: a = (
        transcript show: 'before'; newLine.
        transcript show: Handled new size printString; newLine
    )
) : ()
```

Its record: `status = 1`, `stdout` `before` and a newline, and `stderr`
`Uncaught Newspeak error: an object does not understand #size` and a newline,
in the format the other goldens use — run it under both lowerings and confirm
exactly that before writing the `.toml`. `e2bb31e` printed `before` and
`handled here` and exited 0; say so in the commit message.

- [ ] **Step 4: Run every gate**

Expected: green. `vm.rs`, `interpreter/` and `builtins/` compile without
dead-code warnings.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime goldens/kernel
git commit -m "refactor: remove the per-class primitive lists and the pass-over they needed"
```

---

### Task 14: `build`'s entry check stops at the kernel

`crates/ns-cli/src/commands/build.rs`'s `root_class_answers` walks the root
class's superclass applications and accepts any non-primitive entry. Its own
comment names the hazard this plan triggers: once `Object` is written in
Newspeak, `root_class_answers(…, "printString")` finds `Object`'s kernel
method and accepts an entry selector the image's symbol table never interns —
the module's own tree is all `mixins::encode` walks — instead of refusing the
build.

**Files:**
- Modify: `crates/ns-cli/src/commands/build.rs`
- Test: `crates/ns-cli/tests/build_load.rs`

- [ ] **Step 1: Write the failing test**

In `build_load.rs`, beside the existing refusal tests, a fixture whose root
class inherits from `Object` and declares neither `main:args:` nor
`printString`; run `ns-next build <fixture> --entry printString --out <file>`
and assert it exits with failure and its stderr says the root class "does not
answer `printString`". Copy the neighbouring tests' way of writing a fixture
and running the binary.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test build_load`
Expected: the new test FAILS — the build succeeds.

- [ ] **Step 3: Stop the walk at the first builtin level**

Walk class objects rather than applications, so a level can be asked whether
it is a builtin: start at the root class, and stop at the first class whose
`class_builtin` is `Some` — everything from there up is the kernel's, which no
image carries. At each user level, accept a declared entry (any entry but a
primitive, as today) or a nested class of that name. Replace the comment's
"No test catches that today" with the test's name.

- [ ] **Step 4: Run it, then every gate**

Run: `cargo test -p ns-cli --test build_load`, then every gate.
Expected: PASS; `run-corpus` still "ran 6, failed 0, skipped 205" — its built
lanes build every whitelisted module.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/ns-cli
git commit -m "fix: refuse a build entry only the core library answers"
```

---

### Task 15: Measure, and record the result

**Files:**
- Modify: `docs/design/execution.md` — a new "What the core library's move
  cost" section
- Modify: `docs/PROGRESS.md` — the step's entry

**Interfaces:**
- Consumes: the finished implementation.
- Produces: the figures the next plans argue from.

- [ ] **Step 1: Record the expectation before measuring**

Write it down before running anything, in the section's draft:

- **Head, primitive methods on**, against `e2bb31e`: level on every source.
  A leaf send that `IntegerBinary` does not absorb costs one cached send plus
  one field test on the method, where it cost one cached send; the rest of the
  path is the same primitive. The one new cost on *every* method send is that
  field test; if a source that sends no leaf moves, that is where to look.
- **Head, `NS_NEXT_NO_PRIMITIVE_METHODS=1`**, against `e2bb31e`: slower on
  every source that sends a leaf the fast path does not absorb — each such
  send becomes an activation of the kernel method, a lexical send of
  `vmMirror`, and a `SendMirror`. Name in advance which sources those are,
  from their text: `bench_block.ns` (`value:`), and every source that sends
  `printString`, `at:`, `size` or `show:` in its loop.
- **Start-up**, against `e2bb31e`: slower by the kernel's parse, resolution,
  compilation and instantiation, which did not exist.

- [ ] **Step 2: Build every binary with the protocol's flags**

For the head, `e2bb31e` and `be3eb40`, in PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Never measure a debug build.

- [ ] **Step 3: Check `vm::run` is still one inlined function**

`dumpbin /disasm` (Visual Studio Build Tools; not on `PATH`) over a release
build with debug information: no `vm::run::closure` may have become a
function of its own. `SendMirror` adds an arm near LLVM's inlining threshold.
**Do this before any timing.** If the step split, the comparison is invalid;
move `SendMirror`'s arm body out of line behind `#[inline(never)]` (it is cold
whenever primitive methods are on) and check again.

- [ ] **Step 4: Measure all eight sources, two sittings**

The seven sources of `execution.md`'s "What removing the re-entries moved",
plus `goldens/bench/bench_megamorphic.ns`. Four are goldens; the other four
are not in the tree and must be written out from the text in `execution.md`,
"What spec-conformant lookup moved" — never reconstructed from their prose
descriptions. Four binaries each sitting: `e2bb31e`, `be3eb40`, the head, and
the head under `NS_NEXT_NO_PRIMITIVE_METHODS=1` (the same binary; set the
variable for its runs only). Follow the protocol those sections follow:
interleaved rounds, the same `bench --iterations` count, pooled groups.

- [ ] **Step 5: Measure start-up**

The time from process start to the first instruction of a trivial module:
`ns-next run` on a module whose `main:args:` returns immediately, 50 runs per
binary, `e2bb31e` against the head, interleaved. Report the medians and the
spread. If the head adds more than a few milliseconds, say so plainly and name
the next step the design already names for it — embedding the compiled kernel
as an image generated at build time, never checked in
(`core-library.md`, "Where the library source lives") — without building it.

- [ ] **Step 6: Write the measurement up**

Add "What the core library's move cost" to `execution.md`, in the shape of
the sections around it: the expectation recorded in Step 1, the table (each
source against both baselines, for both head configurations), which sources
moved and which did not, what the primitive methods recovered, the start-up
figure, and what is left unexplained stated as unexplained rather than guessed
at.

- [ ] **Step 7: Update the progress tracker**

In `docs/PROGRESS.md`, record plan 1 of step 6 as done — the first of the three
plans "What step 6 still needs" names — with the measured range for both head
configurations and the start-up figure.

- [ ] **Step 8: Commit**

```bash
git add docs/design/execution.md docs/PROGRESS.md
git commit -m "docs: record what moving the core library into Kernel.ns cost"
```

---

### Task 16: Bring the documentation up to the code

Markdown only: no gate runs for this task.

**Files:**
- Modify: `docs/design/core-library.md`, `dispatch.md`, `compiler.md`,
  `object-model.md`, `image-format.md`, `memory.md`, `docs/design/README.md`,
  `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: `core-library.md`**

- "What exists": the core library is `crates/ns-runtime/kernel/Kernel.ns` over
  the VM mirror; the leaves are the `MIRROR` table.
- "Planned: the core library as Newspeak source over a VM mirror" becomes the
  description of what is built, keeping its reasoning: the mirror as the whole
  Rust boundary, the bootstrap as built (the six steps of this plan's "The
  bootstrap, step by step"), where the source lives, what the step did not
  break, and the ordering.
- A new subsection, **"Planned: real bodies for control flow and `do:`, before
  the JIT tiers"** (Decision 7): the kernel's control-flow methods and `do:`
  forward to today's natives, so `do:`'s loop is still opaque; rewriting them
  as Newspeak loops and branches must happen before the JIT tiers and pays
  off only with one; and it owns four decisions, each with its golden or its
  code named: a non-block argument taken as its own value
  (`variable_and_block_coerces_its_value_to_a_boolean`), `do:` iterating over
  a copy (`Heap::array_elements` clones), `do:` with a non-block argument
  being a no-op, and `whileTrue:` with a non-block body re-running the
  condition. Say that an `Object>>value` answering `self` would be a new,
  observable selector.
- "Gap: an integer has no `/`": step 6 left it open deliberately (Decision 2);
  it still has to be settled, with the three choices as they stand.
- "Decisions": the eight decisions of this plan, dated.
- A new subsection, **"The one observable difference"** (Decision 9): what
  changed for a builtin subclass that declares `doesNotUnderstand:`, why
  (§5.10, and no Newspeak send continues a public lookup above a level), that
  it was decided with the user on 2026-09-22, and the golden that pins it.

- [ ] **Step 2: `dispatch.md`**

- "Primitives": a primitive entry now lives only on `VmMirror`; the builtin
  classes' behaviour is `Kernel.ns`.
- "A primitive can fail" is rewritten: a leaf that declines answers `None` to
  the mirror code, which runs the send's failure block — inline for
  `SendMirror`, activated for a sent mirror send, the method's own body for a
  primitive method — and the pass-over is gone.
- A new section, **"Primitive methods"**: the recognition (kernel only, the
  exact shape, what it rejects), `act`'s fast path under both lowerings, why
  it applies under the reference lowering too (the depth count), the
  measurement switch and why it is for measuring only, and that a cache needs
  no change because the mark is read off the method.
- The VM mirror's class and table are fixed at bootstrap, which is what lets
  `SendMirror` bind its row at compile time; a MirrorBuilder must seal the
  mirror's class when it arrives.

- [ ] **Step 3: `compiler.md`**

- The instruction set: `SendMirror`, with no wire tag and why.
- A new section, **"Mirror sends"**: `recognize_mirror_send`, the three places
  that ask it, the lowering, and the reference lowering's `SendInlinable`.
- "How the limit counts": the exempt activations now include a mirror send's
  failure block, under the same condition; and the agreement between the
  lowerings depends on primitive methods, which is why they apply under both.

- [ ] **Step 4: The other design documents**

- `object-model.md`: the builtin classes are filled from `Kernel.ns`; `True`
  and `False` inherit from `Boolean`; a builtin class's enclosing object is the
  kernel instance.
- `image-format.md`, "Verification": builtin class tag 14 is `VmMirror`, and
  the decoder refuses it wherever a program names a class; the encoder refuses
  an image that reaches the mirror.
- `memory.md`: `Mutator`'s two new roots, `vm_mirror` and `kernel`.
- `docs/design/README.md`: the map's status line for the core library; the
  `goldens/kernel` category in "The goldens".

- [ ] **Step 5: `PROGRESS.md`'s "Where the next session starts"**

Say what plan 1 of step 6 built, in a paragraph in the style of the others,
and what comes next: plan 2, `Class`, `Behavior` and `Metaclass`; then plan 3,
the payload space. Name the separate step Decision 7 created and that it sits
before the JIT tiers; name the separate `Jump` step (Decision 4) and the `/`
gap (Decision 2) as still open.

- [ ] **Step 6: `CLAUDE.md`**

- The architecture paragraph: `ns-runtime` holds `kernel/Kernel.ns`, the core
  library, which the bootstrap compiles into the builtin classes.
- "`vm.rs` holds no dispatch logic": `SendMirror`'s arm calls
  `interpreter::mirror` and does nothing else with what it is handed.
- A new invariant, **"The VM mirror is a capability"**: only
  `resolver::resolve_kernel` records `Binding::MirrorSend`, only `Kernel.ns`
  is resolved with it, the bootstrap checks `vmMirror` holds the mirror, the
  decoder refuses tag 14 wherever a program names a class, the encoder refuses
  an image that reaches the mirror, and `decide_factory` refuses to instantiate
  its class. Say what breaks if any of these is loosened: a program could
  claim every primitive.
- "A cache records what the lookup decided": drop `act_above` from the list
  of re-reads; the primitive-method fast path is read off the method on every
  send, never from the entry.

- [ ] **Step 7: Check a fresh session could start from it**

Read `CLAUDE.md`, `docs/design/README.md` and `PROGRESS.md`'s "Where the next
session starts" as a session that knows nothing else would, and fix whatever
it could not act on.

- [ ] **Step 8: Commit**

```bash
git add CLAUDE.md docs
git commit -m "docs: describe the core library over the VM mirror as built"
```

---

## Self-Review

**Spec coverage.** `core-library.md`'s five subsections: "The VM mirror is the
whole Rust boundary" — Task 1 (the class, the table, one instance, the
capability's refusals) and Tasks 5–12 (every leaf reached only through it).
"The bootstrap: placeholders first, kernel second" — Task 2, whose three
numbered steps are the design's, extended by the refresh of immediate ids and
the wiring-in the design did not spell out. "`Kernel` is declared without a
`Value` mixin application" — the skeleton in Task 2 Step 1. "Where the library
source lives, and how modules are found" — the source is shipped and compiled
at every start (Task 2), and the start-up cost is measured, not assumed
(Task 15 Step 5); **the module namespace on the mirror is not built**: nothing
in this plan loads a second module, and the design says loading several user
modules "is a separate, later step and does not block this one". The
namespace primitive belongs with that step; Task 16 should keep it in the
design as planned. "What this step must not break" — `root_class_answers` is
Task 14; the goldens and the gates are every task's Step 3 and Step 4. "The
ordering, and what it costs" — Task 15. The rest of §3.2 is plan 2, and the
payload space plan 3, by the breakdown `PROGRESS.md` records.

**Placeholders.** Several steps tell the implementer to match a spelling the
code has rather than one this plan guesses: the heap's field-read accessor,
the graph walk's class read (Task 1 Step 9), `lower_arguments`'s slice form
and how `emit_guarded_branch` lands a value (Task 3 Step 7), the parameter
binding index (Task 4 Step 3). Each names the surrounding code to copy. Task 2
Step 6 leaves four helper bodies of a few lines each to be written against the
accessors the crate has; their contracts are stated, and the unit tests in
Task 2 Step 2 exercise every one.

**Type consistency.** `MirrorPrimitive(u8)` with `index()` (Task 1) is what
`Primitive::Mirror`, `Instruction::SendMirror.primitive` (Task 3) and
`MixinMethod.primitive` (Task 4) carry. `mirror_primitive_named(&str)` is
called by `recognize_mirror_send` and the Task 1 tests with a `&str`.
`run_leaf(mutator, row, subject, leaf_arguments)` is called by `execute_sent`
(Task 1) and `send_inlined` (Task 3); `execute_method` (Task 4) calls
`builtins::execute` directly because it passes the send's own selector.
`Binding::MirrorSend` is produced by `resolve_kernel` (Task 2) and read by
`recognize_mirror_send` (Task 3) and `primitive_method::recognize` (Task 4).
`Mutator::vm_mirror()`, `is_vm_mirror()` and the `kernel` field are defined in
Tasks 1 and 2 and used from Task 2 on.




