# The specification's semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The second batch of ns-next's specification: the language's
semantics (`docs/spec/language/semantics.md`) and the run-time semantics
as abstract operations (`docs/spec/runtime.md`), every rule named by the
tests that check it, the eight remaining deviations carried into the book
as decided, and the two gaps decided loud -- an asynchronous send and a
pattern literal that run -- built.

**Architecture:** One runtime change, then prose. The change adds a
deferred literal, as `UnresolvedIdentifier` and `BodyTooLarge` are, that
raises an error naming its gap when it is loaded: both lowerings emit it
for a `<-:` message, after its receiver and arguments are evaluated, and
for a pattern literal, in place of today's `nil`. The rest writes the two
chapters from the design documents and the code as they are, in the
book's fixed forms, adds `rules` lines and `// spec:` lines to the tests,
and moves every deviation out of `docs/design/README.md`'s table into the
book.

**Tech Stack:** Rust (`ns-runtime`'s compiler, bytecode, image and JIT;
`ns-cli`'s goldens), Markdown under `docs/spec/`, mdBook 0.5.4 through
`cargo xtask spec`.

**Spec:** `docs/design/specification.md`: decisions 1 to 7, "The order of
building it", item 2, and "The second batch's deviations", decisions 1
to 8, each decided with the user on 2026-11-23 and 2026-11-24.

## Global Constraints

- The book describes ns-next as it is at the batch's last commit. A rule
  is restated from Bracha where ns-next follows him, written from the
  design documents and the code where he is silent, and marked a gap
  where ns-next lacks what he requires; nothing undecided is invented.
  Where a design document and the code disagree, the code decides and
  the disagreement is a ruling in the ledger.
- The eight deviations, exactly as `docs/design/specification.md`, "The
  second batch's deviations", records them:
  1. `2.5` is a binary `Float` (ours, appendix); `Decimal`, its accessor
     on `Object` and the literal `2.50s` / `2.5s2` / `3s2` / `-0.1s` (no
     radix) are a gap.
  2. A string literal holds the code points its source holds (ours,
     appendix).
  3. `{e1. e2}` is an `ImmutableArray` (gap; today an `Array`); the
     read-only and value collections, their mixins, names and
     conversions are recorded in `docs/OPEN-WORK.md`, not in the book.
  4. `<-:`, promises, turns and E-Order are Bracha's rules and a gap; a
     `<-:` that runs signals an error naming the gap. How a turn waits is
     open.
  5. The remote representation and the copying of values are part of the
     actor gap.
  6. Pattern literals are Bracha's rule and a gap; one that is evaluated
     signals an error naming the gap. The grammar is unchanged.
  7. §8.2's three requirements are rules; the one-builder atomic install
     as it is; the rest gaps naming the complete install, the debugger's
     stage 6 and a new open topic.
  8. The root rule is ours (appendix).
- Rule identifiers: `semantics.` for `language/semantics.md`, `runtime.`
  for `runtime.md`; segments `[a-z][a-z0-9-]*`; a gap's identifier
  contains the segment `gap`. The identifiers each task declares are
  listed in it; a task may add identifiers beside them, never rename one
  another task links to.
- Abstract operations are written as ECMA-262 writes them: a heading with
  the operation's name and parameters, `Name(receiver, selector,
  arguments)`, then numbered steps, each a sentence; a step that can fail
  says what it signals and with which text.
- Every error text the book quotes is the text the code produces today,
  copied from the code or a golden, never paraphrased.
- A golden's `status`, `stdout` and `stderr` change only for the goldens
  Task 1 adds; every other `.toml` gains only its `rules` line.
- New wire tags take the next free numbers and are pinned by
  `crates/ns-runtime/tests/opcode_tags.rs`; the shipped code's literal
  tags (`shipped_code/encode.rs`, `decode.rs`) likewise.
- No change to `crates/ns-syntax`, the lexer or the plugin.
- Names written out (`CLAUDE.md`, "Conventions"), in code and in the
  examples of the book.
- Gates: the commit gate at every commit that changes more than Markdown;
  Task 1 changes the runtime and the image format, so the batch ends with
  `cargo xtask gates end-of-batch` (`images` among it).

## Review Focus

- A `<-:` inside a cascade (`self foo; <-: bar`) and a `<-:` whose
  receiver or argument has a side effect: the side effects happen, in
  order, before the error, and the error names the selector of the
  message that was asynchronous.
- A body holding a `<-:` or a pattern literal that is packaged with
  `build` and run with `load`: the image carries the new literal, and the
  loaded program fails with the same text.
- A `<-:` or a pattern literal under every JIT tier and stress mode: the
  load raises at the same point and the frame is the interpreter's; no
  register the leftover analysis calls written is left unwritten.
- A rule the book states that no test names: it must appear in the
  generated "Untested rules" chapter, not be dropped.
- A link from the new chapters to a rule of `grammar.md` or of each
  other: it must resolve, or `cargo xtask spec` fails.

---

## File Structure

- `crates/ns-runtime/src/bytecode.rs` -- `Literal::Unbuilt(Unbuilt)` and
  `enum Unbuilt`.
- `crates/ns-runtime/src/interpreter/literals.rs` -- `unbuilt_error`.
- `crates/ns-runtime/src/compiler/lower.rs`,
  `crates/ns-runtime/src/compiler/reference.rs` -- the two lowerings.
- `crates/ns-runtime/src/image/opcodes.rs`, `image/encode.rs`,
  `image/decode.rs`, `shipped_code/encode.rs`, `shipped_code/decode.rs`,
  `bytecode/compact.rs`, `jit/baseline.rs`, `jit/mod.rs`, `vm.rs` -- the
  arms every match over `Literal` needs.
- `goldens/language/send/`, `goldens/language/expression/` -- the new
  goldens.
- `docs/spec/language/semantics.md` -- the language's meaning.
- `docs/spec/runtime.md` -- the abstract operations.
- `docs/spec/appendix/differences-from-bracha.md` -- the adopted
  deviations and the gaps.
- `goldens/**/*.toml`, Rust tests under `crates/` -- `rules` and
  `// spec:` lines.
- `docs/design/README.md`, `docs/design/*.md` the deviations point into,
  `docs/OPEN-WORK.md`, `docs/PROGRESS.md`, `CLAUDE.md`,
  `docs/design/specification.md` -- the moved deviations, the new open
  topics, the status.

---

### Task 1: A `<-:` and a pattern literal that run signal their gap

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (the enum, its doc comment,
  the text at `bytecode.rs:2092`)
- Modify: `crates/ns-runtime/src/interpreter/literals.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (`make_value`, beside
  `UnresolvedIdentifier` at `vm.rs:3641`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (the eventual
  message; `lower.rs:3130`), `crates/ns-runtime/src/compiler/reference.rs`
- Modify: every other match `rg "UnresolvedIdentifier" crates/ns-runtime/src`
  lists: `bytecode/compact.rs:664`, `image/encode.rs:771`,
  `image/decode.rs:2932`, `image/opcodes.rs`, `shipped_code/encode.rs:836`,
  `shipped_code/decode.rs:716`, `jit/baseline.rs:1217`, `jit/mod.rs:170`
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Create: `goldens/language/send/an_asynchronous_send_that_runs_signals_that_actors_are_not_built.{ns,toml}`,
  `goldens/language/send/an_asynchronous_send_evaluates_its_receiver_and_arguments_first.{ns,toml}`,
  `goldens/language/send/an_asynchronous_send_never_run_does_not_fail.{ns,toml}`,
  `goldens/language/send/an_asynchronous_message_of_a_cascade_signals_the_gap.{ns,toml}`,
  `goldens/language/expression/a_pattern_literal_that_runs_signals_that_pattern_matching_is_not_built.{ns,toml}`

**Interfaces:**
- Produces, in `bytecode.rs`:
  `Literal::Unbuilt(Unbuilt)` and
  `pub enum Unbuilt { AsynchronousSend(Symbol), PatternLiteral }`
  (`Debug, Clone, PartialEq`), documented as the third deferral: a
  construct the language has and ns-next does not build yet, loaded
  where its value would be made, raising instead.
- Produces, in `interpreter/literals.rs`:
  `pub(crate) fn unbuilt_error<'gc>(unbuilt: &Unbuilt, receiver: Value<'gc>) -> Signal<'gc>`,
  a `NewspeakError::error` whose selector is the asynchronous message's
  selector, or `Symbol::intern("pattern")` for a pattern, and whose text
  is exactly
  `` asynchronous send of `<selector>` needs actors, which ns-next does not build yet (§5.6) ``
  or
  `a pattern literal needs pattern matching, which ns-next does not build yet (§5.1.9)`.
- Produces, in `image/opcodes.rs`: `LITERAL_UNBUILT_ASYNCHRONOUS_SEND: u8 = 14`
  (followed by the selector as the encoder writes a `Symbol` literal's)
  and `LITERAL_UNBUILT_PATTERN_LITERAL: u8 = 15`; in the shipped code,
  tags `11` and `12`.
- Declares, in `docs/spec/language/semantics.md` (each a paragraph saying
  what Bracha requires in one sentence and what ns-next does instead,
  quoting the error text; Task 3 moves both into its sections):
  `semantics.gap.actors`, `semantics.gap.pattern`, which these goldens
  name.

- [ ] **Step 1: Write the goldens, failing**

Each `.ns` is a module whose `main:args:` prints through
`platform transcript`:
- *…signals_that_actors_are_not_built*: `[3 <-: printString] on: Error do: [:error | transcript show: error messageText; newLine]`
  prints `` asynchronous send of `printString` needs actors, which ns-next does not build yet (§5.6) ``.
- *…evaluates_its_receiver_and_arguments_first*: a receiver and a
  keyword argument, each a method that prints `receiver` and `argument`
  before answering; the handler prints the text: three lines in that
  order.
- *…never_run_does_not_fail*: `false ifTrue: [3 <-: printString]`, then
  prints `ran`; exit status 0.
- *…of_a_cascade_signals_the_gap*: `3 printString; <-: yourself` caught,
  printing the text naming `yourself`.
- *…pattern_matching_is_not_built*: `[<_>] value` caught, printing the
  pattern text; a second statement shows `<x: 1>` the same.

Each `.toml` names its rule (`rules = ["semantics.gap.actors"]` or
`["semantics.gap.pattern"]`) and is recorded with `NS_NEXT_NO_JIT=1` from
the interpreter once the implementation exists (`CLAUDE.md`, "Program
outcomes are pinned"); until then write the expected `stdout` by hand
from the texts above.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-cli --test run_goldens asynchronous pattern_literal`
Expected: FAIL -- today the send answers its value and the pattern `nil`.

- [ ] **Step 3: Implement**

Add the variant and the error; `make_value` raises `unbuilt_error` for it
as it raises `unresolved_identifier_error`. In both lowerings, a message
whose `eventual` is set is lowered as the ordinary send would be up to
its last argument -- receiver first, then the arguments left to right --
and then, in place of the send instruction, a `LoadLiteral` of
`Unbuilt::AsynchronousSend(selector)` into the send's destination; a
pattern literal lowers to a `LoadLiteral` of `Unbuilt::PatternLiteral`
where it lowered to `Nil`. Every other match over `Literal` treats
`Unbuilt` as it treats `UnresolvedIdentifier` -- a load that raises
before it writes: the compact form, the leftover analysis, the baseline
JIT's exit, the optimizing tier's refusal, the image's and the shipped
code's encoder and decoder with the tags above, the listing text
`unbuilt asynchronous-send '<selector>'` and `unbuilt pattern-literal`.
Update the doc comment of `compiler/mod.rs:152` and `bytecode.rs`'s
`Literal` doc to name the third deferral.

- [ ] **Step 4: Run the goldens and the unit tests**

Run: `cargo nextest run -p ns-cli --test run_goldens asynchronous pattern_literal` then `cargo nextest run -p ns-runtime opcode_tags image compact leftovers`
Expected: PASS.

- [ ] **Step 5: The image round trip and the stress modes on the new goldens**

Run: `cargo xtask gates images`, then the five goldens under
`NS_NEXT_STRESS_JIT=1`, `NS_NEXT_STRESS_OPTIMIZE=1` and
`NS_NEXT_STRESS_WIDE=1` (`cargo nextest run -p ns-cli --test run_goldens asynchronous pattern_literal` with each set).
Expected: every lane passes.

- [ ] **Step 6: Commit**

Run: `cargo xtask gates commit`
Expected: every gate passed.

```bash
git add crates goldens docs/spec
git commit -m "feat: an asynchronous send and a pattern literal that run signal their gap"
```

### Task 2: Semantics -- objects, values, classes, mixins, enclosing objects

**Files:**
- Modify: `docs/spec/language/semantics.md` (replace the placeholder:
  the chapter's introduction, then these sections)
- Modify: the `.toml`s of `goldens/library/value/`,
  `goldens/language/class/{class-side,instantiation,mixin,nesting}/`,
  `goldens/language/module/`, `goldens/language/builtin-names/`
- Modify: the Rust tests that check these rules (`crates/ns-runtime/tests/value_conditions.rs`,
  the resolver's root-rule tests), a `// spec:` line each

**Interfaces:**
- Declares: `semantics.object`, `semantics.value`,
  `semantics.value.conditions`, `semantics.value.identity`,
  `semantics.class.per-enclosing-object`, `semantics.class.declaring`,
  `semantics.superclass.clause`, `semantics.mixin.application`,
  `semantics.mixin.clause-order`, `semantics.mixin.standalone`,
  `semantics.enclosing-object`, `semantics.outer.lexical`,
  `semantics.outer.alone`, `semantics.module.root-rule`,
  `semantics.builtin-names`.
- Sources: Bracha §3.1, §3.1.1, §3.2, §3.3, §6, §6.1, §6.2;
  `docs/design/object-model.md` ("Three layers, not one", "Mixin
  application", "The root rule", "A value object's four conditions",
  "Class sides and metaclasses"); `CLAUDE.md`'s invariants on `outer` and
  on a `<:` clause's message.

- [ ] **Step 1: Write the sections**

One section per topic, each rule its `r[...]` line and a paragraph; the
root rule names the seven forms and quotes the resolve error's text from
`object-model.md`, "The root rule". `semantics.builtin-names` states the
protected accessors on `Object` (Bracha §3.2) as `Kernel.ns` has them.

- [ ] **Step 2: Name the rules in the tests**

Every `.toml` under the directories above gains a `rules` line naming
the rules its program checks (one or more); a Rust test that checks one
gains its `// spec:` line.

- [ ] **Step 3: Build the book**

Run: `cargo xtask spec`, then `cargo xtask gates commit`
Expected: the book renders; every gate passed.

- [ ] **Step 4: Commit**

```bash
git add docs/spec goldens crates
git commit -m "docs: the specification's semantics of objects, classes and mixins"
```

### Task 3: Semantics -- messages, literals, asynchronous sends

**Files:**
- Modify: `docs/spec/language/semantics.md`
- Modify: the `.toml`s of `goldens/language/{send,evaluation-order,expression,control-flow}/`,
  `goldens/library/number/`, `goldens/library/string/`
- Modify: Task 1's goldens need no change; their identifiers are
  declared here

**Interfaces:**
- Declares: `semantics.send.ordinary`, `semantics.send.evaluation-order`,
  `semantics.send.implicit-receiver`, `semantics.send.self`,
  `semantics.send.super`, `semantics.send.outer`,
  `semantics.send.cascade`, `semantics.send.does-not-understand`,
  `semantics.literal.integer`, `semantics.literal.float`,
  `semantics.gap.decimal`, `semantics.literal.string`,
  `semantics.literal.symbol`, `semantics.literal.boolean-and-nil`,
  `semantics.gap.immutable-array` (with what a tuple is today: a fresh
  `Array` each evaluation), `semantics.literal.block`,
  `semantics.gap.pattern`, `semantics.actor`, `semantics.actor.promise`,
  `semantics.actor.turn`, `semantics.actor.e-order`,
  `semantics.actor.remote-representation`, `semantics.gap.actors`.
- Sources: Bracha §3.4, §3.7, §5.1 to §5.10; `docs/design/dispatch.md`
  (the lookup, the implicit receiver, access control);
  `docs/design/core-library.md` ("Deviation: a non-integral literal is a
  binary float", "The numeric tower"); `docs/design/frontend.md` (the
  string literal); `docs/design/specification.md`, "The second batch's
  deviations", decisions 1 to 6.

- [ ] **Step 1: Write the sections**

The actor section states Bracha's rules and closes with
`semantics.gap.actors`: what ns-next does instead (Task 1's error, its
text), and that how a turn waits for a promise is open. The decimal gap
gives the literal's form as decided and says the class and the literal
are not built.

- [ ] **Step 2: Name the rules in the tests**

As Task 2, for the directories above; Task 1's goldens now resolve.

- [ ] **Step 3: Build the book**

Run: `cargo xtask spec`, then `cargo xtask gates commit`
Expected: the book renders; every gate passed.

- [ ] **Step 4: Commit**

```bash
git add docs/spec goldens crates
git commit -m "docs: the specification's semantics of messages and literals"
```

### Task 4: Semantics -- methods, blocks, slots, access, instantiation

**Files:**
- Modify: `docs/spec/language/semantics.md`
- Modify: the `.toml`s of `goldens/language/{method,block}/`,
  `goldens/language/slot/{accessor,local,simultaneous}/`,
  `goldens/library/closure/`, `goldens/library/object/`

**Interfaces:**
- Declares: `semantics.method.return`, `semantics.block.closure`,
  `semantics.block.non-local-return`, `semantics.block.cannot-return`,
  `semantics.slot.immutable`, `semantics.slot.mutable`,
  `semantics.slot.lazy`, `semantics.slot.simultaneous`,
  `semantics.slot.accessors`, `semantics.access.public`,
  `semantics.access.protected`, `semantics.access.private`,
  `semantics.instantiation.factory`,
  `semantics.instantiation.initializer-order`,
  `semantics.class-side`.
- Sources: Bracha §3.5, §3.6, §5.1.8, §6.3; `docs/design/execution.md`
  ("Unwinding"); `docs/design/compiler.md` ("The three initializer
  bodies"); `docs/design/dispatch.md` (access control).

- [ ] **Step 1: Write the sections**
- [ ] **Step 2: Name the rules in the tests** (as Task 2)
- [ ] **Step 3: Build the book and run the commit gate** (`cargo xtask spec`, then `cargo xtask gates commit`; every gate passed)
- [ ] **Step 4: Commit**

```bash
git add docs/spec goldens crates
git commit -m "docs: the specification's semantics of methods, blocks, slots and instantiation"
```

### Task 5: Semantics -- exceptions, modules and programs, reflection

**Files:**
- Modify: `docs/spec/language/semantics.md`
- Modify: the `.toml`s of `goldens/library/exception/`,
  `goldens/tool/manifest/`, `goldens/library/platform/`,
  `goldens/library/mirror/`, `goldens/library/mirror-builder/`,
  `goldens/runtime/depth-limit/`

**Interfaces:**
- Declares: `semantics.exception.signal`, `semantics.exception.handler`,
  `semantics.exception.ensure`, `semantics.exception.unhandled`,
  `semantics.exception.resumable`, `semantics.module.identity`,
  `semantics.module.manifest`, `semantics.program.main`,
  `semantics.program.platform`, `semantics.reflection.mirrors`,
  `semantics.reflection.atomic-install`,
  `semantics.gap.reflection.several-builders` (and the edits the
  complete install adds), `semantics.gap.reflection.object-class`,
  `semantics.gap.reflection.activation`, `semantics.depth-limit`.
- Sources: Bracha §7, §8; `docs/design/core-library.md` (exceptions);
  `docs/design/modules.md`; `docs/design/liveness-and-reflection.md`
  ("Mirrors that read", "A MirrorBuilder and an atomic install",
  "Planned: the complete install"); `docs/design/execution.md` ("The
  depth limit").

- [ ] **Step 1: Write the sections**

The reflection gaps each name the work that closes it: the complete
install, the debugger's stage 6, and the open topic Task 7 adds.

- [ ] **Step 2: Name the rules in the tests** (as Task 2)
- [ ] **Step 3: Build the book and run the commit gate** (`cargo xtask spec`, then `cargo xtask gates commit`; every gate passed)
- [ ] **Step 4: Commit**

```bash
git add docs/spec goldens crates
git commit -m "docs: the specification's semantics of exceptions, modules and reflection"
```

### Task 6: The run-time semantics as abstract operations

**Files:**
- Modify: `docs/spec/runtime.md` (replace the placeholder)
- Modify: the Rust tests of the operations (`crates/ns-runtime/tests/`,
  `interpreter/` unit tests), a `// spec:` line each where a test checks
  one; `goldens/library/weak/`, `goldens/library/mirror-builder/`,
  `goldens/runtime/`

**Interfaces:**
- Declares one identifier per operation, and per failing step where it
  signals: `runtime.lookup` (`Lookup(class, selector, access)`),
  `runtime.implicit-receiver` (`ImplicitReceiverLookup(activation, selector)`),
  `runtime.send` (`Send(receiver, selector, arguments)`),
  `runtime.does-not-understand`, `runtime.instantiate`
  (`Instantiate(class, arguments)`), `runtime.apply-mixin`
  (`ApplyMixin(mixin, superclass, enclosing)`), `runtime.slot.read`,
  `runtime.slot.write`, `runtime.slot.guard`, `runtime.outer`
  (`EnclosingObject(activation, depth)`), `runtime.unwind`
  (`Unwind(target, value)`), `runtime.handler-search`,
  `runtime.forward-identity` (`become:`'s bulk forward and what it
  refuses), `runtime.install` (`Install(edits)` as it is),
  `runtime.vm-mirror` (the capability and its six locks, as a program
  sees them), `runtime.depth-limit`, `runtime.memory.identity-hash`,
  `runtime.memory.weak`, `runtime.memory.ephemeron`.
- Consumes: the `semantics.` identifiers of Tasks 2 to 5, which each
  operation links to.
- Sources: `docs/design/dispatch.md`, `docs/design/object-model.md`
  ("What a slot access checks"), `docs/design/execution.md`
  ("Unwinding", "The depth limit"), `docs/design/liveness-and-reflection.md`,
  `docs/design/memory.md` ("What the collector traces", weak arrays and
  ephemerons), `docs/design/core-library.md` ("The VM mirror is the whole
  Rust boundary"), `CLAUDE.md`'s invariants; the code each names.

- [ ] **Step 1: Write the operations**

Each in ECMA-262's form (Global Constraints), stating only what a
program observes -- the order of evaluation, what is signalled with
which text -- never the mechanism (caches, the JIT, handles), which the
VM invariants part takes later.

- [ ] **Step 2: Name the rules in the tests**
- [ ] **Step 3: Build the book**

Run: `cargo xtask spec`, then `cargo xtask gates commit`
Expected: the book renders; every gate passed.

- [ ] **Step 4: Commit**

```bash
git add docs/spec goldens crates
git commit -m "docs: the specification's run-time semantics"
```

### Task 7: The deviations leave the design documents

**Files:**
- Modify: `docs/spec/appendix/differences-from-bracha.md` (sections for
  decisions 1, 2 and 8: what Bracha says, what the book says, the rule's
  identifier, why; the gaps list gains decimal, immutable array, actors,
  pattern literals and the reflection gaps)
- Modify: `docs/design/README.md` ("Deviations from the language
  specification": the table goes; the section says every difference is
  the book's appendix and every gap its rules)
- Modify: `docs/design/core-library.md`, `docs/design/frontend.md`,
  `docs/design/object-model.md`, `docs/design/liveness-and-reflection.md`
  (each "recorded deviation" points at the book's rule instead)
- Modify: `docs/OPEN-WORK.md`: the "Deviations from the specification"
  section and the deviations in "Defects that change a correct program's
  output" go, pointing at the book's gaps; new items, each with a pointer
  to `docs/design/specification.md`, "The second batch's deviations":
  `Decimal` and its literal (decision 1, with the open meaning of a scale
  below the digits written); the read-only and value collections and
  `ImmutableArray` as the tuple literal's class (decision 3, its whole
  hierarchy); how a turn waits for a promise, under the existing actors
  item (decision 4); pattern matching's design (decision 6); changing an
  individual object's class (decision 7)
- Modify: `CLAUDE.md` ("for every part the book already writes -- today
  the lexical structure and the grammar" names the semantics and the
  run-time semantics; `docs/spec/`'s description likewise)
- Modify: `docs/design/specification.md` (the status paragraph: the
  second batch built; "The second batch's deviations" says they are
  carried into the book)
- Modify: `docs/PROGRESS.md` (the batch's entry and its milestone row),
  `docs/OPEN-WORK.md`'s "Next" (the specification's third batch, the
  image format)

- [ ] **Step 1: Write the appendix and move the deviations**
- [ ] **Step 2: Build the book and run the suite**

Run: `cargo xtask spec`, then `cargo xtask gates commit`
Expected: every gate passed.

- [ ] **Step 3: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: the deviations live in the specification"
```

- [ ] **Step 4: The end of the batch**

Run: `cargo xtask gates end-of-batch`
Expected: every gate passed; `run-corpus` "ran 6, failed 0, skipped 205".
