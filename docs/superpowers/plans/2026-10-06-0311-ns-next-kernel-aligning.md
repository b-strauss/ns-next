# The kernel aligned with Primordial Soup's core API — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The first of the kernel completion's six sub-projects: `Block`
becomes `Closure`, `Message>>sentTo:` becomes `sendTo:`, `Array>>at:put:`
answers the stored value, and the `do:`, `whileTrue:` and `whileFalse:`
leaves take Primordial Soup's semantics -- `do:` reads the elements as it
goes and sends `value:` to whatever it is given, a loop sends `value` to a
body that is no block.

**Architecture:** Three renames and one answer change are mechanical. The
loops need one new piece: a leaf that must send `value`/`value:` to an
object that is no closure and then carry the loop on. Today a step a
continuation answers is pushed without a continuation of its own, so a loop
cannot carry on after a send. This plan adds a boxed step,
`Step::MethodThen`, that pushes a kernel method's activation and hangs a
continuation on it, the way the unwinder already does by hand for
`valueOnUnwind:` (`interpreter/unwind.rs`, `start_unwind_block`); two
private kernel methods on `Closure`, `valueOf:` and `valueOf:with:`, are
what it pushes. The loops keep their leaves; moving them into Newspeak
stays at road step 7/8.

**Tech Stack:** Rust 1.98.0 (`crates/ns-runtime`), Newspeak
(`crates/ns-runtime/core-library/Kernel.ns`), goldens (`goldens/**/*.ns` +
`.toml`, run by `crates/ns-cli/tests/run_goldens.rs` under both lowerings).

**Spec:** `docs/design/core-library.md`, "Planned: the kernel completed to
Primordial Soup's core API" -- its decided bullets on `Closure`,
`sendTo:`, `Array>>at:put:` and the loops, and "Planned: real bodies for
control flow and `do:`, before the baseline JIT" for the three loop
questions this settles. Facts gathered for this plan (file:line lists of
every site) are summarized in each task.

## Global Constraints

- Communicate with the user in German; all code, comments, identifiers,
  commit messages and documentation in English.
- Names are written out (CLAUDE.md, "Conventions"): no identifier of one or
  two characters, no truncated word -- `action`, `element`, `index`,
  `mutator`, `arguments`; clippy's `min_ident_chars` enforces the short ones.
- **Never `{:?}` a `Value`, never `.unwrap()`/`.expect()`/`{:?}` an
  `EvaluationResult`**; tests use the file's local `expect_ok` /
  `expect_error` helpers.
- **`vm.rs` holds no dispatch logic** (CLAUDE.md): what a step pushes and
  which continuation it carries is decided in `interpreter/`; `vm.rs` only
  calls `Mutator::push_continuation`, unchanged.
- **A new Rust-side field holding a handle or a stored `Word` needs its
  `Trace` impl** (`crates/ns-runtime/src/trace.rs`): every new or changed
  field of `DoLoop`, `WhileLoop` and the new `MethodThen` is traced.
- **A wire tag is assigned once**: the builtin class tag of the closure
  class stays **12** (`image/encode.rs`, `image/decode.rs`,
  `tests/opcode_tags.rs`); only names change.
- **Program outcomes are pinned**: every golden whose output changes here
  changes because the decided semantics changed; state that in the golden's
  comment. Each task adds goldens pinning its new behaviour.
- **A public member of a shipped core-library module is documented**: each
  changed or new `public` member of `Kernel.ns` keeps a `(* … *)` comment
  directly above it describing what it does now, one to four lines, naming
  no other implementation (no "Primordial Soup" in `Kernel.ns` comments
  this plan writes or touches).
- **Comments explain the decision**, not the mechanics; match the comment
  density of the surrounding code. A Newspeak module inline in a Rust test
  is an `indoc! {"…"}`, indented with spaces, the closing `"}` on its own
  line. `.ns` files outside `corpus/` are indented with four spaces.
- **Documentation describes the current state** (no "previously", no
  "historical"): when a task changes behaviour, the design-document
  sentences describing the old behaviour are rewritten in the same commit.
- Conventional commits; every commit message ends with
  ```
  Co-Authored-By: <the implementing model's name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2
  ```
  where the first line names the subagent's own model. Never override the
  author or committer identity.
- Work on the checked-out branch `feature/kernel-aligning` in
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
  The end-of-batch gates (the five stress modes, `run-corpus` plain and
  stressed, the benchmarks profile, the release size tests) are **not** run
  per task; the controller runs them once after Task 6 (the
  `finishing-a-batch` skill). No task touches `ns-syntax`, `ns-analyzer`,
  `schemas/` or the plugin, so the Gradle gate does not apply.

## Review Focus

- **A non-block that does answer `value:`/`value`** -- a user object with a
  `value:` method handed to `do:`, or with `value` as a loop body: it must
  be sent once per element / per iteration, in order, its answer dropped,
  and the loop must carry on (Task 4's and Task 5's goldens pin it).
- **An error raised inside the sent `value:`** -- caught by an `on:do:`
  around the `do:` like an error in a block; the loop's state must not leak
  or resume (Task 4 adds a golden).
- **`at:put:` inside `do:` on an index already visited or not yet visited**
  -- only later indices are seen; and an empty array with a non-block
  answers the array without sending anything (Task 4 tests both).
- **The reference lowering against the optimized one** -- every new golden
  runs under both; a non-block body is never inlined, so both reach the
  same leaf and must count the same activations (the depth-limit goldens
  must stay green; Task 5 runs them).
- **A future as `do:`'s action or a loop's body** -- the existing decline
  stays, so `spec_6_3_2_a_future_is_a_block_argument_like_its_value` keeps
  its output (Tasks 4 and 5 re-run it).

---

### Task 1: `Block` becomes `Closure`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (line 68 the accessor
  `public Block = ( ^outer Kernel Block )`; line 489 Future's
  `protected Block = ( ^super Block )`; lines 925-927 the class comment and
  `public class Block = Object new () (`; the comment at line 189
  "caught by `Block>>on:do:`"; the shared accessor comment around lines
  50-55 if it lists `Block`)
- Modify: `crates/ns-runtime/src/types.rs:86` and `:160` (`BuiltinClass::Block`
  → `BuiltinClass::Closure`), and every use the compiler then reports:
  `interpreter/kernel.rs` (80, 782, 811-813, 1025, 1066, 1438, 1452, 1674,
  1715), `interpreter/dispatch.rs:617`, `interpreter/mod.rs:1497, 1512`,
  `interpreter/unwind.rs:487`, `interpreter/instantiate.rs:339`,
  `image/encode.rs:246`, `image/decode.rs:944`, `tests/image.rs` (2020,
  3518, 3933, 5288), `tests/opcode_tags.rs:386`
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:80`
  (`BuiltinClass::Closure => "Closure"` in `kernel_class_name`)
- Modify: `crates/ns-runtime/src/heap.rs:2585` (`Format::Closure => "a Closure"`)
- Modify (texts naming the class): `interpreter/kernel.rs` tests at 1456,
  1463, 1479, 1515-1516, 1522; `heap.rs:5936` (`named_class(&mut heap,
  "Closure")`); `interpreter/dispatch.rs:628`; `interpreter/unwind.rs:497`
  ("Kernel.ns's Closure declares valueOnUnwind:");
  `compiler/unwind_marker.rs:51, 74, 101`; doc comments naming the class at
  `bytecode.rs:436`, `compiler/unwind_marker.rs:2, 7, 15`,
  `interpreter/dispatch.rs:609-610`, `interpreter/resolve.rs:419`,
  `interpreter/unwind.rs:451, 453`, `interpreter/instantiate.rs:38, 285,
  334`, `interpreter/kernel.rs:1419, 1471`
- Modify: `goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods.ns`
  (line 8's comment "`MyArray` and `MyBlock`" and line 17
  `class MyBlock = Block new () ()` → `class MyClosure = Closure new () ()`;
  its `.toml` is unchanged)
- Create: `goldens/library/closure/a_closure_is_an_instance_of_closure.ns`
  and `.toml`
- Modify docs: `docs/design/core-library.md` (63, 123, 134, 145, 1393,
  1501, 1805, 1812, 1944, 2618, 2651, 2775), `docs/design/execution.md`
  (482, 568, 581, 3791, 4194, 5394), `docs/design/compiler.md:397`,
  `docs/design/object-model.md` (473, 1769 -- "a Block" as `describe`'s
  text) -- wherever the text names the kernel class `Block` (not where it
  means a block literal or "a block" as a concept)

**Not renamed** (they name the block *literal* or kernel-internal mirror
rows, not the class): `CodeKind::Block`, `Step::Block`,
`ast::Expression::Block`, `BodyKey::Block`, `Emitted::Block`,
`Primitive::Block`/`BlockPrimitive`, the module `builtins/block.rs`, the
mirror selectors `blockValue:ifFail:`, `block:value:ifFail:` and their
relatives, `blockNumArgs:ifFail:`, `block:whileTrue:ifFail:`,
`block:whileFalse:ifFail:`. `benchmarks/` and `corpus/` are not touched.

**Interfaces:**
- Produces: `BuiltinClass::Closure` (tag 12); the kernel class `Closure`
  and `Object>>Closure`; `Heap::describe` of a closure answers
  `"a Closure"`. Tasks 4 and 5 call
  `mutator.builtin_class(BuiltinClass::Closure)`.

- [ ] **Step 1: Write the failing golden**

`goldens/library/closure/a_closure_is_an_instance_of_closure.ns`:

```newspeak
(* §5.1.8: a closure literal denotes a newly created instance of class
   `Closure`, which `Object` names by its class accessor, and a closure
   that is no number is described as one in an error's text. *)
class AClosureIsAnInstanceOfClosure = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. |
        transcript show: [] class printString; newLine.
        transcript show: ([:each | each] class == Closure) printString; newLine.
        transcript show: ([3 + []] on: Error do: [:error | error messageText]); newLine
    )
) : ()
```

`goldens/library/closure/a_closure_is_an_instance_of_closure.toml`:

```toml
stdout = '''
Closure
true
+ requires a numeric argument, got a Closure
'''
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-cli --test run_goldens a_closure_is_an_instance_of_closure`
Expected: FAIL -- stdout starts `Block`, the second line errors with
"does not understand #Closure".

- [ ] **Step 3: Rename the kernel class and its accessor**

In `Kernel.ns` line 68: `public Closure = ( ^outer Kernel Closure )`;
line 489: `protected Closure = ( ^super Closure )`; lines 925-927:

```newspeak
    (* A closure, `[:argument | …]`: code that runs when it is sent
       `value`, in the scope it was written in (§5.1.8). *)
    public class Closure = Object new () (
```

and line 189's comment to "caught by `Closure>>on:do:`". Keep the accessor
list in `Object`'s shared comment in alphabetical order if it is one.

- [ ] **Step 4: Rename the builtin class and its name**

Rename the variant `BuiltinClass::Block` to `BuiltinClass::Closure` in
`types.rs` (declaration and the `ALL` list) and follow every compiler
error through the files listed above. In `interpreter/kernel.rs`'s
`kernel_class_name`:

```rust
BuiltinClass::Closure => "Closure",
```

In `heap.rs`'s `Heap::describe`:

```rust
Format::Closure => "a Closure".to_string(),
```

`image/encode.rs` keeps `BuiltinClass::Closure => 12`, `image/decode.rs`
keeps `12 => … BuiltinClass::Closure`, and `tests/opcode_tags.rs:386`
becomes `assert_eq!(builtin_class_tag(BuiltinClass::Closure), 12);` -- the
number never changes.

- [ ] **Step 5: Rename the texts and comments that name the class**

Update every test source, expectation text and comment listed under
**Files** to say `Closure`; in `interpreter/kernel.rs`'s tests the inline
Newspeak `public class Block = Object new () (` becomes `public class
Closure = Object new () (` and the lookups `nested.name.as_str() ==
"Block"` become `"Closure"`. Then update the golden
`a_subclass_of_a_builtin_falls_to_objects_methods.ns`: line 8 names
`MyArray` and `MyClosure`, line 17 is `class MyClosure = Closure new () ()`,
and every use of `MyBlock` in the file becomes `MyClosure`. Its `.toml`
must not change.

Run: `rg -n "\bBlock\b" crates/ns-runtime/src crates/ns-runtime/core-library crates/ns-runtime/tests crates/ns-cli goldens --glob '!*.toml'`
Expected: only hits that name a block literal or the kernel-internal
mirror rows listed under **Not renamed**; inspect each.

- [ ] **Step 6: Run the goldens and the runtime tests**

Run: `cargo nextest run -p ns-cli --test run_goldens a_closure_is_an_instance_of_closure a_subclass_of_a_builtin_falls_to_objects_methods`
Expected: PASS (both lowerings, and the two-entry-cache variant).
Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 7: Update the design documents**

In the doc lines listed under **Files**, rename the kernel class `Block`
to `Closure` (and `describe`'s text "a Block" to "a Closure") wherever
the sentence names the class, in present tense. In
`docs/design/core-library.md`'s "Planned: the kernel completed to
Primordial Soup's core API", delete the bullet "**`Block` becomes
`Closure`**", and make the class's description in "What exists" (lines
~63-145, where `Block` is listed) name `Closure` and cite §5.1.8.

- [ ] **Step 8: Run the per-commit gates and commit**

Run the gates from **Global Constraints**; all green.

```bash
git add -A crates goldens docs
git commit -m "feat: the closure class is Closure, as §5.1.8 names it" -m "Renames the kernel class Block, its accessor on Object and BuiltinClass::Block; describe answers 'a Closure'. The builtin class tag stays 12." -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```

---

### Task 2: `Message>>sentTo:` becomes `sendTo:`

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (line 450 the method,
  its comments at 432-436 and 443-448; the sends at 84 and 498)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs:891-895` (the row
  `mirror("message:sendTo:ifFail:", "sentTo:", …)` → reported selector
  `"sendTo:"`), and the comments at `builtins/primitive.rs:368`,
  `builtins/message.rs:1, 55`, `builtins/mod.rs:7`,
  `interpreter/resolve.rs:380, 574`
- Rename: `goldens/library/exception/sent_to_is_a_public_send.ns`/`.toml`
  → `goldens/library/exception/send_to_is_a_public_send.ns`/`.toml`
- Modify docs: `docs/design/core-library.md` (67, 1771, 1776, 1901, 1906,
  1936, 2062, 2760), `docs/design/dispatch.md` (364, 378, 380),
  `docs/design/object-model.md` (844, 941), `docs/design/execution.md:6462`

**Interfaces:**
- Produces: `Message>>sendTo: receiver` (public); `sentTo:` no longer
  exists. The error text is `'sendTo: needs a Symbol selector and an Array
  of arguments'`.

- [ ] **Step 1: Rename the golden and make it fail**

```bash
git mv goldens/library/exception/sent_to_is_a_public_send.ns goldens/library/exception/send_to_is_a_public_send.ns
git mv goldens/library/exception/sent_to_is_a_public_send.toml goldens/library/exception/send_to_is_a_public_send.toml
```

In the `.ns`: the class becomes `SendToIsAPublicSend`, every `sentTo:`
send becomes `sendTo:`, and the comment reads:

```newspeak
(* `Message>>sendTo:` sends the message as an ordinary public send (§5.5):
   a program can build a message, so a send through one reaches a public
   method and never a private or a protected one, which end in
   `doesNotUnderstand:`. A message built without `arguments:` is sent with
   none; one whose selector is no Symbol cannot be sent, and `sendTo:`
   raises an `Error` rather than answering nil. *)
```

In the `.toml`, the last stdout line becomes
`sendTo: needs a Symbol selector and an Array of arguments`; the other five
lines stay.

Run: `cargo nextest run -p ns-cli --test run_goldens send_to_is_a_public_send`
Expected: FAIL -- "does not understand #sendTo:".

- [ ] **Step 2: Rename the method, its row and its uses**

`Kernel.ns` line 450:

```newspeak
        public sendTo: receiver = ( ^vmMirror message: self sendTo: receiver ifFail: [^(outer Kernel Error new) signal: 'sendTo: needs a Symbol selector and an Array of arguments'] )
```

Its comment (432-436, 443-448) says `sendTo:` and quotes the new text;
lines 84 and 498 send `sendTo:`. In `builtins/primitive.rs` the row's
reported selector becomes `"sendTo:"` (the kernel's forwarder test names
each primitive method after its row's reported selector). Update the
comments listed under **Files**.

- [ ] **Step 3: Run the goldens that go through it**

Run: `cargo nextest run -p ns-cli --test run_goldens send_to_is_a_public_send a_message_not_understood_is_resumed reached_default_handler_stays_true_once_reached simultaneous`
Expected: PASS -- the last three families send `sendTo:` through
`Object>>doesNotUnderstand:` and `Future>>doesNotUnderstand:` with their
outputs unchanged.
Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 4: Update the design documents**

Rename `sentTo:` to `sendTo:` in the doc lines listed under **Files**. In
`core-library.md`'s "Planned: the kernel completed …" delete the bullet
"**`Message>>sentTo:` becomes `sendTo:`**".

- [ ] **Step 5: Run the per-commit gates and commit**

```bash
git add -A crates goldens docs
git commit -m "feat: Message>>sentTo: becomes sendTo:" -m "The name every other Newspeak kernel uses, with no alias; the error text names the new selector." -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```

---

### Task 3: `Array>>at:put:` answers the stored value

**Files:**
- Modify: `crates/ns-runtime/src/builtins/collections.rs:113-149`
  (`element_at_put`; line 148 answers `receiver`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns:885-889` (the comment)
- Modify: `crates/ns-runtime/tests/builtins_collections_block.rs:86-103`
  (`array_at_put_writes_and_returns_the_receiver`)
- Create: `goldens/library/collection/array_at_put_answers_the_value.ns`
  and `.toml`
- Modify docs: `docs/design/core-library.md` (the sentence around 2443-2445
  "where the kernel's `Array>>at:put:` leaf answers the receiver")

**Interfaces:**
- Produces: `Array>>at:put:` answers its second argument.

- [ ] **Step 1: Write the failing golden**

`goldens/library/collection/array_at_put_answers_the_value.ns`:

```newspeak
(* `Array>>at:put:` answers the value it stored, as `List>>at:put:` does,
   so a store can be the last expression of a method that answers it. *)
class ArrayAtPutAnswersTheValue = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. array |
        array:: Array new: 2.
        transcript show: (array at: 1 put: 5) printString; newLine.
        transcript show: (array at: 2 put: 'x'); newLine.
        transcript show: (array at: 1) printString; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
5
x
5
'''
```

Run: `cargo nextest run -p ns-cli --test run_goldens array_at_put_answers_the_value`
Expected: FAIL -- the first line is `an Object`.

- [ ] **Step 2: Answer the value**

In `builtins/collections.rs`'s `element_at_put`, the success arm answers
the stored value (already bound at line ~143 as the second argument)
instead of `receiver`:

```rust
    Some(Ok(value))
```

and its doc comment (line 113) says it answers the value. `Kernel.ns`
lines 885-889:

```newspeak
        (* Stores `value` at `index`, 1-based, and answers `value`.
           Signals an Error, 'index … out of range: valid indices are
           1..size (size …)', for an index outside 1 to `size`, and 'at:put:
           requires an Integer index, got …' for one that is no Integer. *)
```

- [ ] **Step 3: Update the unit test**

Rename `array_at_put_writes_and_returns_the_receiver` to
`array_at_put_writes_and_answers_the_value`, and make its Newspeak method
answer the store's answer as well as check the element: keep the existing
read of `a at: 2` and add an assertion, in the test's own style, that
`(a at: 2 put: 99)` answers `99`.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-cli --test run_goldens array_at_put_answers_the_value`
Expected: PASS.
Run: `cargo nextest run -p ns-runtime --test builtins_collections_block`
Expected: PASS.

- [ ] **Step 5: Update the design documents**

`core-library.md` around 2443-2445: "`add:` answers the element, and
`at:put:` the value, as `Array>>at:put:` does". In "Planned: the kernel
completed …" delete the bullet "**`Array>>at:put:` answers the value**".

- [ ] **Step 6: Run the per-commit gates and commit**

```bash
git add -A crates goldens docs
git commit -m "feat: Array>>at:put: answers the value it stored" -m "As List>>at:put: and every Smalltalk kernel do." -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```

---

### Task 4: A loop can send to a non-block; `Array>>do:` reads as it goes

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the `Step` enum at
  ~134; `push_step`; `resume` at ~1300-1315; `push_continuation`'s comment
  at ~1367-1370; the unit test `deciding_do_answers_its_block_for_the_first_element`
  at ~2096-2135)
- Modify: `crates/ns-runtime/src/types.rs:368-396` (`WhileLoop`, `DoLoop`)
  and add `LoopAction`
- Modify: `crates/ns-runtime/src/trace.rs:124-147` (tracing `WhileLoop`,
  `DoLoop`) and wherever `Step` values are traced, for `MethodThen`
- Modify: `crates/ns-runtime/src/builtins/collections.rs:244-314`
  (`do_step`, `after_element`, `element_step`)
- Modify: `crates/ns-runtime/src/symbol.rs` (add `Symbol::VALUE_OF` =
  `valueOf:` and `Symbol::VALUE_OF_WITH` = `valueOf:with:` beside
  `Symbol::VALUE` at ~46, in the file's own style)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (in `Closure`, beside
  `valueOnUnwind:` at ~977-986: the two private helpers; `Array>>do:`'s
  comment at 881-883)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (the quick-method
  table at ~1021-1028 if `valueOnUnwind:` is listed there, and any list of
  private kernel methods the bootstrap checks)
- Modify: `crates/ns-runtime/tests/builtins_collections_block.rs` (130-174
  `do_walks_a_snapshot_of_its_receiver`; 201-218
  `do_with_a_non_block_answers_its_receiver`)
- Create goldens under `goldens/library/collection/`:
  `do_sees_a_store_made_inside_its_block`, `do_sends_value_to_a_non_block`,
  `do_with_a_non_block_that_understands_nothing_fails`,
  `an_error_inside_do_with_a_non_block_is_caught`
- Modify docs: `docs/design/execution.md` (~5652-5675, "walks a
  snapshot", "a non-block stands for its own value"),
  `docs/design/core-library.md` (~2444-2451 and "Planned: real bodies for
  control flow and `do:`" ~2792-2808), `docs/design/memory.md:774`
  (`DoLoop`/`WhileLoop` roots)

**Interfaces:**
- Consumes: `BuiltinClass::Closure` (Task 1).
- Produces (Task 5 uses them):
  ```rust
  // types.rs
  /// What a native loop activates or sends to: a closure it activates,
  /// or any other object, which it sends `value`/`value:`.
  #[derive(Debug, Clone, Copy)]
  pub enum LoopAction<'gc> {
      Closure(ObjectReference<'gc>),
      Sent(Value<'gc>),
  }
  impl<'gc> LoopAction<'gc> { pub(crate) fn of(heap: &Heap, value: Value<'gc>) -> Self }

  // interpreter/mod.rs, in `enum Step`
  /// Activate a kernel method, as `Step::Method` with owned arguments
  /// does, and do `then` with its value.
  MethodThen(Box<MethodThen<'gc>>),
  pub(crate) struct MethodThen<'gc> {
      pub(crate) receiver: Value<'gc>,
      pub(crate) method: Rc<crate::mixin::MixinMethod>,
      pub(crate) defining_class: ClassReference<'gc>,
      pub(crate) arguments: Vec<Value<'gc>>,
      pub(crate) then: Continuation<'gc>,
  }
  // interpreter/mod.rs, on Mutator
  /// The step that sends `value` (no argument) or `value:` (one) to
  /// `action` through `Closure`'s private `valueOf:` / `valueOf:with:`,
  /// on `receiver`, and does `then` with the answer.
  pub(crate) fn send_value_then(
      &mut self,
      receiver: Value<'gc>,
      action: Value<'gc>,
      argument: Option<Value<'gc>>,
      then: Continuation<'gc>,
  ) -> Step<'gc>
  ```
  `after_element` takes `&mut Mutator` and answers `Step`.

**Why a kernel method rather than a send:** what a send of `value:`
decides may be a method, a block, a value or an instantiation of several
activations, and only one activation's return is a place to hang the
loop's continuation on -- the reason the unwinder already pushes
`valueOnUnwind:` (`interpreter/unwind.rs`, `start_unwind_block`). The step
is boxed so that `Step` keeps its size: an unboxed variant the size of
`Step::Method` measured 4-6% slower (the comment on
`Step::Initializer`).

- [ ] **Step 1: Write the failing goldens**

`goldens/library/collection/do_sees_a_store_made_inside_its_block.ns`:

```newspeak
(* `Array>>do:` reads each element when it reaches it, so a store the
   block makes to a later index is seen, and one to an index already
   visited is not; the size is read once, when the walk starts. *)
class DoSeesAStoreMadeInsideItsBlock = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. array |
        array:: {1. 2. 3}.
        array do: [:each |
            array at: 3 put: 99.
            array at: 1 put: 0.
            transcript show: each printString; show: ' '].
        transcript newLine.
        transcript show: (array at: 1) printString; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
1 2 99 
0
'''
```

`goldens/library/collection/do_sends_value_to_a_non_block.ns`:

```newspeak
(* `Array>>do:` sends `value:` to an argument that is no block, once per
   element and in order, as it would evaluate a block; an empty array
   sends nothing. *)
class DoSendsValueToANonBlock = Object new (
) (
    class Collector = Object new (
        | public seen ::= 0. public total ::= 0. |
    ) (
        public value: element = (
            seen:: seen + 1.
            total:: total * 10 + element.
            ^'ignored'
        )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. collector answer |
        collector:: Collector new.
        answer:: {1. 2. 3} do: collector.
        transcript show: collector seen printString; newLine.
        transcript show: collector total printString; newLine.
        transcript show: answer size printString; newLine.
        transcript show: (({} do: 3) size) printString; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
3
123
3
0
'''
```

`goldens/library/collection/do_with_a_non_block_that_understands_nothing_fails.ns`:

```newspeak
(* An argument of `Array>>do:` that is no block and has no `value:` ends
   in `doesNotUnderstand:` at the first element. *)
class DoWithANonBlockThatUnderstandsNothingFails = Object new (
) (
    public main: platform args: arguments = (
        | three |
        three:: 3.
        ^{1. 2} do: three
    )
) : ()
```

`.toml`:

```toml
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #value:
'''
```

`goldens/library/collection/an_error_inside_do_with_a_non_block_is_caught.ns`:

```newspeak
(* An error raised by the `value:` that `Array>>do:` sends to a non-block
   unwinds out of the loop like an error raised inside a block, and the
   loop does not carry on. *)
class AnErrorInsideDoWithANonBlockIsCaught = Object new (
) (
    class Failing = Object new (
        | public seen ::= 0. |
    ) (
        public value: element = (
            seen:: seen + 1.
            element = 2 ifTrue: [1 + nil].
            ^element
        )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. failing |
        failing:: Failing new.
        transcript show: ([{1. 2. 3} do: failing. 'finished'] on: Error do: [:error | 'caught']); newLine.
        transcript show: failing seen printString; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
caught
2
'''
```

Run: `cargo nextest run -p ns-cli --test run_goldens do_sees_a_store do_sends_value_to_a_non_block do_with_a_non_block_that_understands_nothing an_error_inside_do_with_a_non_block`
Expected: FAIL -- the first prints `1 2 3`, the second `0`/`0`, the third
exits 0, the fourth prints `finished` and `0`.

- [ ] **Step 2: Add the two private kernel helpers**

In `Kernel.ns`, inside `Closure`, next to `valueOnUnwind:`, in its style:

```newspeak
        (* What a native loop sends to an action that is no block, so that
           the loop carries on from this activation's return whatever the
           send decides: `do:`'s `value:`, a loop body's `value`. *)
        private valueOf: action = ( ^action value )
        private valueOf: action with: element = ( ^action value: element )
```

If the bootstrap or the kernel compiler treats `valueOnUnwind:` specially
(a quick-method row in `interpreter/kernel.rs` ~1021-1028, a list of
private kernel methods the shape check allows), give the two helpers the
same treatment; they must be pushed as real methods, as `valueOnUnwind:`
is. Add `Symbol::VALUE_OF` (`"valueOf:"`) and `Symbol::VALUE_OF_WITH`
(`"valueOf:with:"`) to `symbol.rs` beside `Symbol::VALUE`.

- [ ] **Step 3: Add `LoopAction`, `MethodThen` and `send_value_then`**

`types.rs`, before `WhileLoop`:

```rust
/// What a native loop runs for each step: a closure it activates itself,
/// or any other object, which it sends `value` or `value:` as a block
/// would be sent them. Decided once, when the loop starts: an object never
/// stops or starts being a closure.
#[derive(Debug, Clone, Copy)]
pub enum LoopAction<'gc> {
    Closure(ObjectReference<'gc>),
    Sent(Value<'gc>),
}

impl<'gc> LoopAction<'gc> {
    pub(crate) fn of(heap: &Heap, value: Value<'gc>) -> Self {
        match value.as_object() {
            Some(object) if heap.is_closure(object) => LoopAction::Closure(object),
            _ => LoopAction::Sent(value),
        }
    }
}
```

(Use the `Heap` path and visibility the neighbouring types use; if
`is_closure` is reached as `mutator.heap().is_closure`, take `&Heap`
accordingly.)

`interpreter/mod.rs`, a new variant at the end of `enum Step`:

```rust
    /// Activate `method`, as `Step::Method` with owned arguments does, and
    /// do `then` with its value: how a native loop sends `value` or
    /// `value:` to an action that is no block and carries on afterwards
    /// (`Mutator::send_value_then`). Boxed, so that `Step` keeps the size
    /// the hot path was measured at.
    MethodThen(Box<MethodThen<'gc>>),
```

with the struct beside `Step`:

```rust
pub(crate) struct MethodThen<'gc> {
    pub(crate) receiver: Value<'gc>,
    pub(crate) method: Rc<crate::mixin::MixinMethod>,
    pub(crate) defining_class: ClassReference<'gc>,
    pub(crate) arguments: Vec<Value<'gc>>,
    pub(crate) then: Continuation<'gc>,
}
```

In `push_step`, the arm pushes the method exactly as the `Step::Method`
arm does with `StepArguments::Owned(arguments)` and
`FeedbackBase::NOT_ASSIGNED`, counted, then sets the continuation on the
activation it pushed:

```rust
            Step::MethodThen(method_then) => {
                let MethodThen { receiver, method, defining_class, arguments, then } = *method_then;
                let activation = self.push_step(
                    Step::Method {
                        receiver,
                        method,
                        defining_class,
                        arguments: StepArguments::Owned(arguments),
                        feedback_base: inline_cache::FeedbackBase::NOT_ASSIGNED,
                    },
                    send_arguments,
                    sender,
                    counted,
                )?;
                self.heap.activation_set_continuation(activation, then);
                Ok(activation)
            }
```

(adapt the parameter names to `push_step`'s actual signature; the point is
that the method activation is pushed by the same code path as
`Step::Method` and only then given `then`). Every other `match` over
`Step` the compiler reports gets an arm: where a `Step` is traced, trace
`receiver`, `arguments` and `then` as the `Method` and `Block` arms trace
theirs (CLAUDE.md: a new Rust-side field holding a handle needs its
`Trace`).

`Mutator::send_value_then`, next to `start_unwind_block`'s lookup idiom:

```rust
    /// The step that sends `value` to `action`, or `value:` with
    /// `argument`, through `Closure`'s private `valueOf:` or
    /// `valueOf:with:` activated on `receiver`, and does `then` with the
    /// answer. A send's decision may be a method, a block, a value or an
    /// instantiation of several activations; the helper's own return is
    /// the one place to hang `then` on for all of them.
    pub(crate) fn send_value_then(
        &mut self,
        receiver: Value<'gc>,
        action: Value<'gc>,
        argument: Option<Value<'gc>>,
        then: Continuation<'gc>,
    ) -> Step<'gc> {
        let closure_class = self.builtin_class(BuiltinClass::Closure);
        let (selector, arguments) = match argument {
            None => (Symbol::VALUE_OF, vec![action]),
            Some(argument) => (Symbol::VALUE_OF_WITH, vec![action, argument]),
        };
        let method = self
            .heap
            .class_application(closure_class)
            .and_then(|application| application.mixin.compiled_method(selector).cloned())
            .expect("internal: Kernel.ns's Closure declares valueOf: and valueOf:with:");
        Step::MethodThen(Box::new(MethodThen {
            receiver,
            method,
            defining_class: closure_class,
            arguments,
            then,
        }))
    }
```

Update `push_continuation`'s comment: a continuation's step now also may
be a `MethodThen`, whose arguments are owned, so `&[]` stays right.

- [ ] **Step 4: Make `do:` read as it goes and send to a non-block**

`types.rs`'s `DoLoop` becomes:

```rust
/// A native `do:` loop in progress: the array it walks and answers when it
/// is done, what it runs for each element, the size it read when it began
/// -- the walk ends there, as a loop from 1 to the size does -- and the
/// index of the next element, which is read from the array only when the
/// walk reaches it, so a store the action makes to a later index is seen.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct DoLoop<'gc> {
    pub(crate) receiver: ObjectReference<'gc>,
    pub(crate) action: LoopAction<'gc>,
    pub(crate) size: usize,
    pub(crate) next: usize,
}
```

`trace.rs` traces `receiver` and the `action`'s value (either arm) in
place of `items`. `builtins/collections.rs`:

```rust
fn do_step<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let Some(action) = arguments.first().copied() else {
        return Some(Err(Signal::Error(NewspeakError {
            selector,
            receiver,
            message: "do: requires a block argument".to_string(),
        })));
    };
    // No element, nothing to run or send: the loop is over before it starts.
    if size == 0 {
        return Some(Ok(Step::Value(receiver)));
    }
    // A future stands for its value, which may well be a block: declining
    // hands it to the caller's failure path, which sees through it.
    if !action.as_object().is_some_and(|object| mutator.heap.is_closure(object))
        && mutator.as_future(action).is_some()
    {
        return None;
    }
    let action = LoopAction::of(&mutator.heap, action);
    Some(Ok(element_step(mutator, Box::new(DoLoop { receiver: object, action, size, next: 0 }))))
}

/// The action returned for one element. Its value is dropped, and the next
/// element's activation takes its place on the stack.
pub(crate) fn after_element<'gc>(mutator: &mut Mutator<'gc>, state: Box<DoLoop<'gc>>) -> Step<'gc> {
    element_step(mutator, state)
}

/// What runs for the element at `state.next`, read now, or the array once
/// the walk has reached the size it read at the start. A closure is
/// activated as `value:` activates it, counted; anything else is sent
/// `value:`.
fn element_step<'gc>(mutator: &mut Mutator<'gc>, mut state: Box<DoLoop<'gc>>) -> Step<'gc> {
    let receiver = Value::object(state.receiver);
    if state.next >= state.size {
        return Step::Value(receiver);
    }
    let Some(element) = mutator.heap.array_element(state.receiver, state.next) else {
        return Step::Value(receiver);
    };
    state.next += 1;
    match state.action {
        LoopAction::Closure(closure) => Step::Block {
            closure,
            arguments: StepArguments::Owned(vec![element]),
            then: Continuation::DoElement(state),
        },
        LoopAction::Sent(action) => {
            mutator.send_value_then(receiver, action, Some(element), Continuation::DoElement(state))
        }
    }
}
```

Check the actual names and signatures of `array_element_count` and
`array_element` in `heap.rs` (~2102, ~2124: whether the index is 0- or
1-based, and whether they answer `Option`); `execute`'s
`CollectionPrimitive::Enumerate` arm passes `mutator` as `&mut`. The
`array_element` `None` arm cannot happen while an array's size is fixed;
it ends the walk rather than panicking, since road step 5's `become:` will
be able to change what the receiver is. In `interpreter/mod.rs`'s
`resume`, the `DoElement` arm calls `after_element(self, state)`. Update
`Array>>do:`'s comment in `Kernel.ns` (881-883):

```newspeak
        (* Evaluates `action` with each element in turn, from the first,
           reading each element when it reaches it; an `action` that is no
           block is sent `value:`. Answers the receiver. *)
```

- [ ] **Step 5: Update the unit tests that pinned the snapshot**

`tests/builtins_collections_block.rs`:
- `do_walks_a_snapshot_of_its_receiver` becomes
  `do_reads_each_element_when_it_reaches_it`; its doc comment drops
  `Heap::array_elements`; the expected answer becomes `102099` (the block
  writes 99 to index 3 before index 3 is read: (1 + 2 + 99) * 1000 + 99).
  Re-derive it from the test's own source before changing the number.
- `do_with_a_non_block_answers_its_receiver` becomes
  `do_with_a_non_block_sends_it_value`, using the file's `expect_error`
  helper and asserting the error text "an object does not understand
  #value:".
- In `interpreter/mod.rs`, `deciding_do_answers_its_block_for_the_first_element`
  asserts `state.size == 3` (and `state.next == 1`) instead of
  `state.items.len()`.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-cli --test run_goldens do_sees_a_store do_sends_value_to_a_non_block do_with_a_non_block_that_understands_nothing an_error_inside_do_with_a_non_block spec_6_3_2_a_future_is_a_block_argument_like_its_value`
Expected: PASS, the last with its output unchanged (`123stepstep` / `3`).
Run: `cargo nextest run -p ns-runtime`
Expected: PASS -- in particular `tests/trampoline.rs`'s
`a_deep_recursion_through_do_runs_on_the_activation_stack`,
`tests/stress.rs`, `tests/reference_lowering.rs`, `tests/inline_caches.rs`.
Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: PASS.

- [ ] **Step 7: Update the design documents**

`execution.md` ~5652-5675: `do:` reads each element when it reaches it
and the size once; a non-block action is sent `value:` through
`Closure>>valueOf:with:`, pushed with the loop's continuation
(`Step::MethodThen`). `core-library.md` ~2444-2451 (List's `do:` compared
with the leaf: now alike) and "Planned: real bodies for control flow and
`do:`": the first two of its three bullets are now settled -- rewrite them
as the current behaviour of the leaf. `memory.md:774`: `DoLoop` keeps its
receiver and action, not a copy of the elements. In "Planned: the kernel
completed …", leave the loops' bullet until Task 5.

- [ ] **Step 8: Run the per-commit gates and commit**

```bash
git add -A crates goldens docs
git commit -m "feat: Array>>do: reads each element as it goes and sends value: to a non-block" -m "A native loop sends to an action that is no closure through Closure's private valueOf:with:, pushed as Step::MethodThen with the loop's continuation, as the unwinder pushes valueOnUnwind:." -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```

---

### Task 5: `whileTrue:` and `whileFalse:` send `value` to a non-block body

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`WhileLoop`: `body:
  Option<ObjectReference>` → `body: LoopAction<'gc>`) and `trace.rs`
  (124-134)
- Modify: `crates/ns-runtime/src/builtins/block.rs` (10-103: `execute`'s
  `body` closure, `body_step`, its doc comments at 18-22 and 92-93; the
  module comment line 1 says "`Closure` native methods")
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`resume`'s
  `WhileCondition` arm if `body_step` now needs `mutator`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns:945-950`
  (`whileTrue:`/`whileFalse:` comments)
- Modify: `crates/ns-runtime/tests/builtins_control_flow.rs` (add tests)
- Create goldens under `goldens/language/control-flow/`:
  `a_loop_sends_value_to_a_body_that_is_no_block`,
  `a_loop_whose_body_understands_nothing_fails`
- Modify docs: `docs/design/execution.md` (~5652-5675, "a non-block stands
  for its own value"), `docs/design/core-library.md` ("Planned: real bodies
  for control flow and `do:`", its third bullet; "Planned: the kernel
  completed …", the loops' bullet)

**Interfaces:**
- Consumes: `LoopAction`, `LoopAction::of`, `Mutator::send_value_then`,
  `Continuation::WhileBody` (Task 4).

- [ ] **Step 1: Write the failing goldens**

`goldens/language/control-flow/a_loop_sends_value_to_a_body_that_is_no_block.ns`:

```newspeak
(* `whileTrue:` and `whileFalse:` send `value` to a body that is no block,
   once per iteration, as they would evaluate a block; a body that is never
   reached is sent nothing. *)
class ALoopSendsValueToABodyThatIsNoBlock = Object new (
) (
    class Counter = Object new (
        | public count ::= 0. |
    ) (
        public value = ( count:: count + 1. ^'ignored' )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. counter body condition |
        counter:: Counter new.
        body:: counter.
        condition:: [counter count < 3].
        transcript show: (condition whileTrue: body) printString; newLine.
        transcript show: counter count printString; newLine.
        condition:: [counter count >= 5].
        condition whileFalse: body.
        transcript show: counter count printString; newLine.
        [false] whileTrue: 3.
        transcript show: 'never sent'; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
nil
3
5
never sent
'''
```

`goldens/language/control-flow/a_loop_whose_body_understands_nothing_fails.ns`:

```newspeak
(* A loop body that is no block and has no `value` ends in
   `doesNotUnderstand:` the first time the condition lets it run. *)
class ALoopWhoseBodyUnderstandsNothingFails = Object new (
) (
    public main: platform args: arguments = (
        | count body |
        count:: 0.
        body:: 7.
        ^[count:: count + 1. count < 3] whileTrue: body
    )
) : ()
```

`.toml`:

```toml
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #value
'''
```

Run: `cargo nextest run -p ns-cli --test run_goldens a_loop_sends_value_to_a_body_that_is_no_block a_loop_whose_body_understands_nothing_fails`
Expected: FAIL -- the first prints `0` for the count, the second exits 0.

- [ ] **Step 2: Make the body a `LoopAction`**

`types.rs`'s `WhileLoop`:

```rust
/// A native `whileTrue:`/`whileFalse:` loop in progress: its condition,
/// what it runs as the body -- a closure it activates, or any other object,
/// which it sends `value` -- and the condition value that keeps it going.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct WhileLoop<'gc> {
    /// The condition's closure object.
    pub(crate) condition: ObjectReference<'gc>,
    pub(crate) body: LoopAction<'gc>,
    pub(crate) expect: bool,
}
```

`trace.rs` traces the body's value in either arm. In `builtins/block.rs`,
`execute` builds `body: LoopAction::of(mutator.heap(), arguments[0])` in
place of the `body()` closure (the future decline before it stays), and
`body_step` takes the mutator:

```rust
/// The body: a closure activated as a control-flow step, or anything else
/// sent `value`, whose answer is dropped before the condition runs again.
fn body_step<'gc>(mutator: &mut Mutator<'gc>, state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    match state.body {
        LoopAction::Closure(body) => Step::Block {
            closure: body,
            arguments: StepArguments::ControlFlow,
            then: Continuation::WhileBody(state),
        },
        LoopAction::Sent(body) => {
            let receiver = Value::object(state.condition);
            mutator.send_value_then(receiver, body, None, Continuation::WhileBody(state))
        }
    }
}
```

`after_condition` calls `body_step(mutator, state)`. A closure body keeps
`StepArguments::ControlFlow` (the reference lowering's depth exemption is
unchanged); a sent body is counted under both lowerings, which is right
because a non-block body is never inlined by the optimized lowering
either. `Kernel.ns` 945-950:

```newspeak
        (* `whileTrue:` evaluates the receiver and, while it answers true,
           `body`; `whileFalse:` likewise while it answers false. A `body`
           that is no block is sent `value`. Both answer nil; a condition
           that answers no Boolean is sent `mustBeBoolean`. *)
```

(keep any sentence of the existing comment that is still true and is not
covered by this one.)

- [ ] **Step 3: Add the unit tests**

In `tests/builtins_control_flow.rs`, in the file's style (`run_method`,
`module`, `integer_of`), add
`a_while_loop_sends_value_to_a_body_that_is_no_block` (a counter object
with `value`, condition `[counter count < 4]`, assert the count is 4) and
`a_while_loop_body_that_understands_nothing_fails` (body `nil`, assert the
error text "an object does not understand #value").

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-cli --test run_goldens a_loop_sends_value a_loop_whose_body depth spec_6_3_2_a_future_is_a_block_argument_like_its_value variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean`
Expected: PASS, the depth-limit goldens included.
Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 5: Update the design documents**

`execution.md`: a while loop's non-block body is sent `value` through
`Closure>>valueOf:`, pushed with the loop's continuation.
`core-library.md`, "Planned: real bodies for control flow and `do:`": the
third bullet becomes current behaviour; the section keeps only what
moving the loops into Newspeak still has to do (no semantic question is
left to it). In "Planned: the kernel completed …" delete the loops'
bullet.

- [ ] **Step 6: Run the per-commit gates and commit**

```bash
git add -A crates goldens docs
git commit -m "feat: whileTrue: and whileFalse: send value to a body that is no block" -m "Through Closure's private valueOf:, as do: sends value: to a non-block action." -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```

---

### Task 6: The sub-project's record

**Files:**
- Modify: `docs/design/core-library.md` ("Planned: the kernel completed to
  Primordial Soup's core API": its sub-project list marks the first done;
  the status block at the top; "Decisions" gains one bullet)
- Modify: `docs/OPEN-WORK.md` ("Next": the aligning sub-project is done,
  the foundations sub-project is next)
- Modify: `docs/PROGRESS.md` (an entry for this sub-project, in the style
  of the entries before it, with its date, plan link and what changed; the
  milestone table row "The kernel completed to Primordial Soup's core API,
  in six sub-projects" becomes "In progress", with this plan linked)

- [ ] **Step 1: Check what is left of the decided bullets**

Run: `rg -n "Closure|sendTo:|at:put:|whileTrue:" docs/design/core-library.md`
and confirm that "Planned: the kernel completed …" no longer lists
`Block`→`Closure`, `sentTo:`→`sendTo:`, `Array>>at:put:` or the loops'
semantics (Tasks 1-5 deleted them) and that "What exists" and the sections
the tasks touched describe the new behaviour in present tense.

- [ ] **Step 2: Record the sub-project**

In "Planned: the kernel completed …", the numbered list's first item
reads "**Aligning** -- built (2026-10-06, [its
plan](../superpowers/plans/2026-10-06-0311-ns-next-kernel-aligning.md))".
Add to "Decisions" one bullet: "**The kernel's names and loops aligned
with Primordial Soup's core API** (decided with the user on 2026-10-06,
built the same day): `Closure`, `sendTo:`, `Array>>at:put:` answering the
value, and loops that send `value`/`value:` to a non-block through
`Closure>>valueOf:` and `valueOf:with:`". `OPEN-WORK.md` "Next": the
aligning sub-project is built; the foundations sub-project starts next and
has no plan yet. `PROGRESS.md`: the entry and the table row as above.

- [ ] **Step 3: Commit (Markdown only, no gates)**

Run: `git diff --stat` -- only `.md` files.

```bash
git add docs
git commit -m "docs: the kernel's aligning sub-project is built" -m "Co-Authored-By: <model> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Xp5itLVQcsQMvNvW3fWNu2"
```
