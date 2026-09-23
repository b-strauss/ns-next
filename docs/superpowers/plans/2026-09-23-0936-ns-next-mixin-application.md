# Mixin Application (§6.2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `newspeak-spec.pdf` §6.2's mixin application — `class C = S someMessage <: M otherMessage (…)` — completely, so a declaration can apply one or more mixins to its superclass, with each applied level's own initializer message and its own enclosing object.

**Architecture:** A mixin application adds *levels* between a declaration's own
mixin and its superclass. Each level is an ordinary class object: its mixin is
the applied class's mixin, its superclass is the level below, its enclosing
object and declaring class are the *applied class's*, per §6.2 ("The enclosing
object of *I* is the enclosing object of *C*"), and its clause message is the
one the applying declaration wrote after `<:`. Because the levels are ordinary
class objects, lookup, `super`, `outer` and the instantiation walk need no new
machinery — what is new is building them, compiling one message body per
applied level, and carrying them through the image format.

**Tech Stack:** Rust 1.98, edition 2021, no new dependencies. Newspeak3 for the
goldens.

**Spec:** `docs/references/newspeak-spec.md` §6.2 ("Mixin Application", the
whole section), the grammar at `classDeclaration` / `inheritanceListAndOrBody`
/ `mixinSpec` / `mixinAppSuffix` / `inheritanceClause` (lines ~2039–2051 and
~2128), and `mixinOperator → "<:"` (line ~980). Background: §6.1 (the
superclass clause), §6.3 (class bodies and the instance initializer), §3.3
(enclosing objects).

## Why now

The user asked for §3.1.1's conditions to be enforced strictly, and one of them
is that a value's enclosing objects are values. A module can only be a value if
it can be declared one, which §3.1.1's own examples and SOMns both write as a
mixin application (`class Kernel vmMirror: vmMirror = Object <: Value (`,
`prior-art/somns/core-lib/Kernel.ns`). So `<:` comes first, as a language
feature in its own right; the strict enforcement is a separate plan that builds
on it.

`<:` has been a recorded deviation since Milestone 1
(`crates/ns-syntax/src/parser/declaration.rs`, the DEVIATION comment at the
superclass clause, and `docs/design/README.md`'s deviations). No corpus module
uses it: the `<:` occurrences in `corpus/modules/Collections.ns` and
`KernelForJS.ns` are inside type annotations and a string literal, checked
before this plan was written.

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency.
- **All code, identifiers, comments and commit messages in English.** Names are
  written out (clippy's `min_ident_chars`, threshold 2).
- **Program outcomes are pinned.** No existing golden's `status`, `stdout` or
  `stderr` may change, under either lowering, and `run-corpus` keeps its
  report. Nothing in this plan changes how an existing program runs: a
  declaration without `<:` builds exactly the levels it built before.
- **`parse-corpus` must keep reporting "parsed 210, failed 0, expected-fail 1,
  stale 0".** A corpus file that now parses where it did not is a change to
  `corpus/blacklist.toml`, and a blacklisted file that parses is a `stale`
  report — check both.
- **Nothing under `crates/ns-syntax` changes for runtime work** — but this
  plan *is* front-end work as well: Task 1 is the one place `ns-syntax`
  changes, and it changes nothing else.
- **Compiling an already-resolved module can never raise a `NewspeakError`.**
  A malformed mixin application is a parse error or a resolve error; an
  unresolvable receiver is deferred to evaluation exactly as a superclass
  clause receiver is today.
- **An image is untrusted input.** The applied levels a decoded mixin table
  describes are input: every row id, count and code id is validated before it
  indexes, and a cycle through applied levels must be refused, not looped on.
- **A wire tag is assigned once and never renumbered or reused.**
- **The gates are staggered** (`CLAUDE.md`, "Working in this repository"):
  before each commit `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, `cargo test --workspace`, and
  `cargo run -p ns-cli -- parse-corpus corpus/modules`; once at the end of a
  batch, the suite under `NS_NEXT_STRESS_MOVES=1`, `NS_NEXT_STRESS_CACHES=1`
  and `NS_NEXT_STRESS_COLLECT=1`, and `cargo run -p ns-cli -- run-corpus
  corpus/modules` plain and under each of the three.
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**, indented
  with the Rust around it, the closing `"}` on a line of its own.
- **Never `{:?}` a `Value`; never `.unwrap()`/`.expect()` an
  `EvaluationResult` or a `HostResult`.**
- **Commits are conventional** and end with the trailer naming the model that
  wrote them, then
  `Claude-Session: https://claude.ai/code/session_01XZBjhUJaKHo3p6VsiUwHKx`.

## How the pieces fit

### What the grammar says

```
classDeclaration          → class identifier messagePattern? "=" inheritanceListAndOrBody
inheritanceListAndOrBody  → classBody | (inheritanceClause mixinSpec)
mixinSpec                 → classBody | mixinAppSuffix
mixinAppSuffix            → (mixinOperator inheritanceClause)+ (dot | classBody)
inheritanceClause         → inheritancePrefix? unarySelector message?
mixinOperator             → "<:"
```

So after the superclass clause a declaration may carry one or more
`<: <inheritance clause>`, and may then end with a `.` instead of a body. Each
inheritance clause has the same shape as the superclass clause: an optional
prefix (`self`, `super`, `outer X`), an identifier, and an optional message.

### What §6.2 says the result is

For `class C = S ms <: M mm (…)`:

- The application produces a class *I* whose **mixin is M's** and whose
  **superclass is S**.
- *I*'s **enclosing object is M's class's enclosing object**, not the declaring
  one. A method of M's mixin running on an instance of C therefore resolves
  `outer` to where M was declared, which is the whole point of a mixin.
- *I*'s **primary factory selector is the one `mm` names**.
- The instance initializer of *I* runs **S's initializer with `ms`'s
  arguments** first, then **M's initializer with `mm`'s arguments**.

`C` itself then has superclass *I*. With several `<:` clauses the levels stack
in written order, each one's superclass being the level to its left.

### What that means in this runtime

The instantiation walk already climbs `Heap::class_superclass` level by level
and asks `clause_message_body(level, superclass)` for the message to run
(`interpreter/instantiate.rs`). So an applied level is an ordinary class
object, and the walk needs no change — **except** for where the message comes
from: today it is read off the level's own mixin
(`Mixin::superclass_message_code`), which cannot work for an applied level,
because one mixin can be applied in many places with different messages. The
compiled message body for an applied level therefore lives **on the level's
class object**, compiled from the applying declaration.

Lookup, `super`, the dispatch ids and the inline caches need nothing: they walk
applications and class objects, and an applied level is both.

### Where a class object is built

Three places build one, and all three must build the applied levels:
`instantiate::load_root_class` (a module's root class),
`interpreter/nested.rs` (the implicit accessor of a nested class), and
`image/decode.rs` (a decoded class record). The first two share
`Mutator::apply_mixin`; the decoder rebuilds applications itself.

---

## File Structure

- `crates/ns-syntax/src/lexer.rs` — the `<:` token.
- `crates/ns-syntax/src/ast.rs` — `ClassDeclaration::mixin_applications:
  Vec<SuperclassClause>`, and a body-less declaration.
- `crates/ns-syntax/src/parser/declaration.rs` — the suffix and the `.` form;
  the DEVIATION comment goes.
- `crates/ns-runtime/src/resolver/mod.rs` — each mixin clause's receiver
  resolved as a superclass clause receiver is.
- `crates/ns-runtime/src/mixin.rs` — `Mixin::mixin_applications`: one entry per
  applied level, each with its clause and its compiled message body.
- `crates/ns-runtime/src/compiler/mod.rs`, `reference.rs` — compile one message
  body per applied level, as `superclass_message_code` is compiled today.
- `crates/ns-runtime/src/interpreter/instantiate.rs` — build the applied levels
  when a class object is built; `clause_message_body` reads an applied level's
  message off the class object.
- `crates/ns-runtime/src/interpreter/nested.rs` — the same for a nested class.
- `crates/ns-runtime/src/types.rs` — `ClassData` carries an applied level's
  clause message and the mixin's own identity.
- `crates/ns-runtime/src/image/mixins.rs`, `encode.rs`, `decode.rs` — the
  applied levels on the wire.
- `goldens/mixin/` — a new category.
- Docs: `object-model.md`, `image-format.md`, `docs/design/README.md`
  (the deviation goes), `dispatch.md` if lookup's description names the chain,
  `PROGRESS.md`.

---

### Task 1: Parse `<:`

**Files:**
- Modify: `crates/ns-syntax/src/lexer.rs`, `src/ast.rs`,
  `src/parser/declaration.rs`
- Test: `crates/ns-syntax/src/parser/declaration.rs`'s tests, `src/lexer.rs`'s
  tests

**Interfaces:**
- Produces: `TokenKind::MixinOperator`; `ClassDeclaration.mixin_applications:
  Vec<SuperclassClause>` (empty for every declaration written today);
  a declaration whose body is absent because it ended with `.`.

- [ ] **Step 1: Write the failing parser tests**

In `parser/declaration.rs`'s tests:

```rust
    #[test]
    fn a_mixin_application_carries_its_clauses_in_order() {
        let module = parse_module(indoc! {"
            Newspeak3
            'T'
            class Applied using: platform = Object new <: Value <: Other with: 1 (
            ) (
            ) : ()
        "});
        let declaration = &module.root_class;
        let superclass = declaration.superclass.as_ref().expect("a superclass clause");
        assert_eq!(superclass.receiver.name, "Object");
        let applications = &declaration.mixin_applications;
        assert_eq!(applications.len(), 2);
        assert_eq!(applications[0].receiver.name, "Value");
        assert!(applications[0].messages.is_empty());
        assert_eq!(applications[1].receiver.name, "Other");
        assert_eq!(applications[1].messages.len(), 1);
    }

    #[test]
    fn a_mixin_application_may_end_with_a_dot_instead_of_a_body() {
        let module = parse_module(indoc! {"
            Newspeak3
            'T'
            class Applied = Object <: Value.
        "});
        let declaration = &module.root_class;
        assert_eq!(declaration.mixin_applications.len(), 1);
        assert!(declaration.instance_side.methods.is_empty());
        assert!(declaration.instance_side.slots.is_empty());
    }
```

and a lexer test that `<:` lexes as one token while `<` and `a < b` still lex
as they do (`x <: y` against `x < y`, and a type annotation `<Foo>` which the
parser skips).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-syntax`

- [ ] **Step 3: Lex `<:`**

Add the token. `<` is a `specialCharacter`, so a binary selector may start with
it: lex `<:` as `MixinOperator` only where `<` is immediately followed by `:`,
and make sure a binary selector `<=` and a type annotation keep lexing as
before. Say in a comment why the longest match is safe here (no binary selector
`<:` exists in the grammar's `specialCharacter*` that a program could mean
instead).

- [ ] **Step 4: Parse the suffix**

In `parse_class_declaration`, after the superclass clause, parse
`(MixinOperator inheritanceClause)*` into `mixin_applications`, reusing
`parse_superclass_clause` for each (it is the same production). Then accept
either a class body as today or a `.`, in which case the declaration has an
empty instance side, no class side and no class comment. Replace the DEVIATION
comment with a reference to §6.2 and to this plan's behaviour.

- [ ] **Step 5: Run the tests and the parse gate**

Run: `cargo test -p ns-syntax`, then
`cargo run -p ns-cli -- parse-corpus corpus/modules` — it must still report
"parsed 210, failed 0, expected-fail 1, stale 0". If a blacklisted file now
parses, that is a `stale` report and the blacklist entry goes, with the reason
in the commit message.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A crates/ns-syntax
git commit -m "feat: parse §6.2's mixin application"
```

---

### Task 2: Resolve a mixin clause

**Files:**
- Modify: `crates/ns-runtime/src/resolver/mod.rs`
- Test: `crates/ns-runtime/tests/resolver.rs`

**Interfaces:**
- Consumes: Task 1's AST.
- Produces: a `Binding` at each mixin clause receiver's span, by the same rules
  as a superclass clause receiver's.

- [ ] **Step 1: Write the failing test**

A module whose class applies a mixin named by a bare identifier, by `outer X`,
and by a name that resolves nowhere; assert the recorded bindings match what
the same receivers get in a superclass clause (`resolve_superclass_clause`'s
cases), including that an unresolvable name records none and is left to
evaluation.

- [ ] **Step 2: Run it to verify it fails**

- [ ] **Step 3: Resolve each clause**

In the resolver, wherever the superclass clause's receiver is resolved, resolve
every `mixin_applications` clause the same way, in written order. Say in a
comment that §6.2 gives the clause the same shape as §6.1's, which is why the
rules are the same and the code is shared rather than repeated.

- [ ] **Step 4: Run the tests and the gates, then commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: resolve a mixin application's clauses"
```

---

### Task 3: Build the applied levels

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (the applied levels on a `Mixin`),
  `crates/ns-runtime/src/types.rs` (`ClassData`), `interpreter/instantiate.rs`,
  `interpreter/nested.rs`, `compiler/mod.rs`, `compiler/reference.rs`
- Test: `crates/ns-runtime/tests/mixin.rs`, `tests/nested_classes.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: for a declaration with *n* mixin clauses, *n* class objects between
  the declaration's own class and its superclass, each with the applied mixin,
  the applied class's enclosing object and declaring class, and its own
  compiled clause message.

- [ ] **Step 1: Write the failing tests**

Three properties, each its own test, over a module like:

```newspeak
Newspeak3
'T'
class Host usingPlatform: platform = Object new (
    | private mark = 'host'. |
) (
    public class Marker = Object new () (
        public markOf: ignored = ( ^mark )
    )
    public class Applied = Object new <: Marker () (
    )
)
```

- the applied level sits between `Applied` and `Object`, carries `Marker`'s
  mixin, and `Applied new markOf: nil` answers `'host'` — the method came from
  the applied mixin;
- the applied level's enclosing object is `Marker`'s enclosing object, so a
  method of `Marker` that sends `outer Host` from an instance of `Applied`
  reaches the `Host` instance (write the method to make that observable);
- `Applied new class` is `Applied`, and `super` from `Applied`'s own method
  reaches the applied level, not `Object`.

- [ ] **Step 2: Run them to verify they fail**

- [ ] **Step 3: Carry the clauses on the mixin**

`Mixin` gains `mixin_applications: Vec<MixinApplicationClause>`, one per `<:`
clause in declaration order, each with the clause's selector (defaulted to
`new` as §6.1 defaults a superclass clause's), whether it has arguments, and —
filled by the compiler — the compiled message body. `Mixin::empty` and every
decoded mixin start with none. Compile each clause's arguments exactly as
`superclass_message_code` is compiled, in both lowerings.

- [ ] **Step 4: Build the levels where a class object is built**

Factor one function that, given the superclass class object and the declaring
context, walks the declaration's clauses in order and answers the top applied
level, then call it from `load_root_class` and from `interpreter/nested.rs`.
Per clause: resolve the named class (the resolver's binding decides how, as it
does for the superclass clause receiver), take its application's mixin, apply
it to the level below with `Mutator::apply_mixin`, and build a class object
whose `enclosing` and `declaring_class` are **the named class's**, whose
superclass is the level below, and which carries this clause's compiled message
body. `ClassData` gains that message; say in its doc comment why it cannot live
on the mixin (one mixin, many applications, different messages).

- [ ] **Step 5: Run the tests and the gates, then commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: build the class levels a mixin application adds"
```

---

### Task 4: Initialize through the applied levels

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
- Test: `crates/ns-runtime/tests/instantiate.rs`

**Interfaces:**
- Consumes: Task 3's levels.
- Produces: `clause_message_body` answering an applied level's own message;
  §6.2's order of initializers.

- [ ] **Step 1: Write the failing test**

A module whose superclass clause and whose `<:` clause both carry a message
with arguments, and whose three initializers append to a shared list, so the
recorded order shows §6.2's: the superclass's initializer with the superclass
clause's arguments, then the applied mixin's with the `<:` clause's, then the
declaring class's own. Assert the arity check fires when a clause's message
does not name the level's primary factory, as it does for a superclass clause
today.

- [ ] **Step 2: Run it to verify it fails**

- [ ] **Step 3: Read the message from the level**

`clause_message_body` asks the class object for its applied-level message
first, and falls back to the mixin's `superclass_message_code` for an ordinary
level. Nothing else in `climb_levels`/`descend_levels` changes; say so in a
comment, because that is the property that makes this feature cheap.

- [ ] **Step 4: Run the tests and the gates, then commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime
git commit -m "feat: run each applied level's initializer with its own message"
```

---

### Task 5: Goldens

**Files:**
- Create: `goldens/mixin/*.ns` and `.toml` (a new category)

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: recorded outcomes under both lowerings.

- [ ] **Step 1: Write the goldens**

At least these, each with a header comment citing §6.2:

- **method lookup and override**: a mixin's method answers where the declaring
  class has none, and the declaring class's own method overrides it, and
  `super` from that method reaches the mixin's;
- **`outer` inside an applied mixin** reaches the mixin's own enclosing object,
  not the applying class's — the property §6.2 states and the one a reader is
  most likely to get wrong;
- **two mixins applied in order**, showing which one wins a selector both
  declare;
- **the initializer order** of §6.2, printed;
- **the body-less form** `class Applied = Object <: Marker.` used and sent to;
- **an error path**: a `<:` clause naming something that is not a class, which
  must be an ordinary Newspeak error naming the clause.

- [ ] **Step 2: Establish each line, then record**

Run each under both lowerings
(`cargo run -p ns-cli -- run <file> --send main:args: --lowering optimized`,
then `reference`), confirm they agree, and check each line against §6.2 before
writing the `.toml`. These are new outcomes: derive them from the
specification, not from what the code prints.

- [ ] **Step 3: Run the gates, then commit**

```bash
cargo fmt --all
git add -A goldens/mixin
git commit -m "test: pin §6.2's mixin application end to end"
```

---

### Task 6: The applied levels on the wire

**Files:**
- Modify: `crates/ns-runtime/src/image/mixins.rs`, `encode.rs`, `decode.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-cli/tests/build_load.rs`

**Interfaces:**
- Consumes: Tasks 3 and 4.
- Produces: a mixin table row that carries its applied levels; a decoder that
  rebuilds them and refuses a malformed or cyclic description.

- [ ] **Step 1: Write the failing tests**

- a module using `<:` is built and loaded, and the loaded program answers what
  the source program answers (extend `crates/ns-cli/tests/build_load.rs`'s
  round-trip shape);
- a hand-built image whose applied level names a row id out of range is a
  `DecodeError`, not a panic;
- a hand-built image whose applied levels form a cycle (a row applying its own
  mixin, directly or through another row) is refused.

- [ ] **Step 2: Run them to verify they fail**

- [ ] **Step 3: Encode and decode the levels**

A row gains its applied levels: per level, how the named class is identified
(the same way a superclass clause's receiver is identified today — extend that
representation rather than inventing a second one), the clause's selector, and
the code id of its message body when it has one. `mixins::plan` collects those
bodies so `encode_code_table` assigns them ids. The decoder validates every id
and count before it indexes, and refuses a cycle; the structural verification
pass covers the new field, and `inspect`'s walk prints it.

- [ ] **Step 4: Run the tests and the gates, then commit**

```bash
cargo fmt --all
git add -A crates/ns-runtime crates/ns-cli
git commit -m "feat: carry a mixin application through the image format"
```

---

### Task 7: Check the hot paths did not move

**Files:**
- Modify: `docs/design/execution.md` (a short paragraph)

- [ ] **Step 1: Say what could have moved, and check it**

Nothing in this plan touches `vm.rs`, `act`, the caches or the compiled code of
an ordinary send: a declaration without `<:` builds the same levels it built
before, and `clause_message_body` gained one branch on a path that runs once
per level per instantiation. Confirm that by reading the diff, then measure
only if something contradicts it — and say which sources you ran if you did.
If nothing moved, record that in one paragraph rather than a table, and say
what was checked.

- [ ] **Step 2: Commit**

```bash
git add docs/design/execution.md
git commit -m "docs: record that mixin application leaves the hot paths alone"
```

---

### Task 8: Documentation

Markdown and comments only.

- [ ] **Step 1: `object-model.md`**

A section on mixin application: the grammar, §6.2's four properties (mixin,
superclass, enclosing object, factory selector), the initializer order, and how
the levels are built here — ordinary class objects, which is why lookup,
`super`, `outer`, the dispatch ids and the caches need nothing.

- [ ] **Step 2: `image-format.md`**

The row's new field, what the decoder validates, and that a cycle through
applied levels is refused like any other cycle.

- [ ] **Step 3: `docs/design/README.md` and `PROGRESS.md`**

Remove `<:` from the deviations — it is implemented — and add the chronology
entry in the style of the others, naming what it unlocks: §3.1.1's
enclosing-object condition, which the next plan enforces.

- [ ] **Step 4: Fresh-session check and commit**

Read `CLAUDE.md`, `docs/design/README.md` and `PROGRESS.md`'s "Where the next
session starts" as a session that knows nothing else would; fix what it could
not act on.

```bash
git add -A
git commit -m "docs: describe mixin application as built"
```

---

## Self-Review

**Spec coverage.** §6.2's grammar — Task 1 (both the suffix and the `.` form).
Its four properties — Task 3 (mixin, superclass, enclosing object) and Task 4
(factory selector and initializer order). The clause's shape being §6.1's —
Task 2. Everything §6.2 says is either built or pinned by a golden in Task 5.

**Not covered here, deliberately:** §3.1.1's strict enforcement, which is the
next plan and the reason this one exists; type parameters on a mixin clause
(`<: Collection[E]`), which the parser skips as it skips them elsewhere and
which no rule in §6.2 depends on — record it as a deviation if the parser does
not already.

**Placeholders.** Task 3 Step 4 and Task 6 Step 3 name the shape of the change
and the invariants it must keep rather than the exact function, because both
seams have three call sites and the plan does not presume which one is the
single place; each says what the report must state.

**Type consistency.** `ClassDeclaration.mixin_applications: Vec<SuperclassClause>`
(Task 1) is read by the resolver (Task 2), turned into
`Mixin::mixin_applications: Vec<MixinApplicationClause>` (Task 3), read by
`clause_message_body` through `ClassData` (Task 4), and written and read by the
image (Task 6).
