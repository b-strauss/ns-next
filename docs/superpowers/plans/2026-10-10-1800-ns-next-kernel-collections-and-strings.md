# The kernel's collections and strings — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The third of the kernel completion's six sub-projects:
Primordial Soup's whole public collection and string API -- an abstract
`Collection` in the kernel with `Array` under it, the whole protocol of
`Array`, `List`, `Map` and `Set`, an `IdentityMap` under `Map`, and
`String`'s protocol in code points with `stringAt:` -- `List`, `Map` and
`Set` rewritten with no adapted code left, a `ConcurrentModification`
error for a collection changed while it is iterated, and every member of
the three shipped modules documented to the standard of Javadoc.

**Architecture:** `Collection` is a new builtin class (wire tag 38),
because the bootstrap reads every builtin's superclass clause and requires
it to name a builtin, and `Array`'s now names `Collection`. Its protocol is
Newspeak over `do:` and a protected hook `newForCollectUsingAdd:`. Five new
leaves on the VM mirror carry what must be fast in Rust: `Array`'s
`copyFrom:to:` and `replaceFrom:to:with:startingAt:`, `String`'s
`indexOf:startingAt:` and `lastIndexOf:startingAt:`, and `String
withAll:`. `CollectionsFactory.ns` is written anew: `List`, `Map` and
`Set` inherit the kernel's `Collection` through a slot of the module, keep
a private modification count that each iteration compares after every
block, `Map` and `Set` gain removal through a per-instance removed mark,
and `IdentityMap` overrides only `Map`'s two protected comparison methods.

**Tech Stack:** Rust 1.98.0 (`crates/ns-runtime`), Newspeak
(`crates/ns-runtime/core-library/Kernel.ns`, `CollectionsFactory.ns`,
`PlatformFactory.ns`), goldens (`goldens/**/*.ns` + `.toml`, run by
`crates/ns-cli/tests/run_goldens.rs` under both lowerings).

**Spec:** `docs/superpowers/specs/2026-10-10-kernel-collections-and-strings-design.md`.
Where this plan departs from it, it says so and why:

- The spec calls `Collection` a kernel class. It is also a **builtin**
  (`BuiltinClass::Collection`, wire tag **38**):
  `interpreter/kernel.rs`'s `superclass_kind` panics for a builtin whose
  superclass clause names a class that is no builtin, and `Array` is one.
- `Object>>subclassResponsibility` (spec, decision 12) reads the sending
  activation's method name through a new leaf, `activationMethodName:ifFail:`,
  which finds the method among the methods of the activation's
  `current_class`'s mixin by the identity of its compiled code:
  `CompiledCode` carries no selector.
- `Map`'s `collect:`, `select:` and `reject:` build their result through
  the same protected hook `newForCollectUsingAdd:` that `List` and `Set`
  answer, so that an `IdentityMap`'s answer an `IdentityMap`. A program's
  subclass of `List` whose primary factory is not `new:` (DeltaBlue's
  `Plan = List new: 15`) still gets a `List` from `select:`, because each
  hook names its class lexically, not `self class`.
- `List>>addAll:` of the receiver itself adds a snapshot of it, so that
  `list addAll: list` doubles the list rather than signal
  `ConcurrentModification`, as a reasonable person expects.
- `removeAllSuchThat:` takes a snapshot, runs the predicate over it, and
  signals `ConcurrentModification` if the predicate changed the
  collection; it then removes in one modification.
- `List` keeps its own `anySatisfy:` and `isEmpty` as faster overrides of
  `Collection`'s; the benchmarks send both on hot paths.

## Global Constraints

- Communicate with the user in German; all code, comments, identifiers,
  commit messages and documentation in English.
- Names are written out (CLAUDE.md, "Conventions"): no identifier of one or
  two characters, no truncated word, in Rust and in Newspeak; clippy's
  `min_ident_chars` enforces the short ones.
- **Never `{:?}` a `Value`, never `.unwrap()`/`.expect()`/`{:?}` an
  `EvaluationResult`**; tests use the file's local `expect_ok` /
  `expect_error` helpers -- copy a neighbour's.
- **`vm.rs` and `compiler/` do not change.** The new leaves live in
  `builtins/`; no dispatch or error text goes anywhere else.
- **Every store into an existing object goes through `heap.rs`'s store
  accessors**: `replaceFrom:to:with:startingAt:` writes through
  `Heap::array_set_element`, never around it.
- **A wire tag is assigned once** (`image/opcodes.rs`,
  `image/encode.rs`'s `builtin_class_tag`, `tests/opcode_tags.rs`):
  `Collection` takes **38**. Nothing is renumbered. No leaf gets a tag.
- **The VM mirror is a capability**: the new rows are leaves on the
  existing mirror; no row hands out the mirror, its class or `Top`.
- **Program outcomes are pinned**: a golden whose output changes changes
  because the decided semantics changed; say so in its comment. One golden
  changes on purpose (Task 5). Every golden runs under both lowerings.
- **The documentation rule** (Task 1 writes it into `CLAUDE.md`; every task
  follows it from then on): every member of `Kernel.ns`,
  `PlatformFactory.ns` and `CollectionsFactory.ns` -- public, protected and
  private, class and slot included -- has a `(* … *)` comment directly
  above it, to the standard of Javadoc: what it does and answers, each
  argument and what it must be, every Error it signals and when, and what
  it guarantees about order, identity, copying and mutation, as long as
  that takes; a section that would say nothing is left out. A doc comment
  names no other implementation. A comment explaining a decision follows
  the doc comment as a second `(* … *)`. In `Kernel.ns` and
  `CollectionsFactory.ns` no comment names another project (no "Primordial
  Soup", "SOMns", "Dart", "Java"); benchmark files may be named.
- **The bodies are written here** (spec, decision 9): do not open the
  bodies of `prior-art/primordialsoup/newspeak/Collections.ns`, nor those
  of `Collection`, `Array`, `String` and `IdentityMap` in
  `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`, nor
  SOMns's or Strongtalk's collections. This plan and the spec are the
  description; the code below was written from them.
- **Comments explain the decision**, not the mechanics; a Newspeak module
  inline in a Rust test is an `indoc! {"…"}`, indented with spaces, the
  closing `"}` on its own line. `.ns` files outside `corpus/` are indented
  with four spaces (`crates/ns-cli/tests/source_formatting.rs`).
- **Documentation describes the current state**: when a task changes
  behaviour, the design-document sentences describing the old behaviour are
  rewritten in the same commit (no "previously", no "historical").
- Conventional commits; every commit message ends with
  ```
  Co-Authored-By: <the implementing model's name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Jkf2ZQ7tKiV22DYxa5NbfA
  ```
  where the first line names the implementer's own model. Never override
  the author or committer identity.
- Work on the checked-out branch `feature/kernel-collections-and-strings`
  in `D:\Workspace\ns-next`; no worktree.
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
  stressed, the benchmarks profile, the release size tests,
  `verifyPlugin`) are **not** run per task; the controller runs them once
  after Task 9 (the `finishing-a-batch` skill). Every commit before that is
  work in progress. No task touches `ns-syntax`, `ns-analyzer`, `schemas/`
  or the plugin, so the Gradle gate does not apply.
- Run one golden or a family with
  `cargo nextest run -p ns-cli --test run_goldens <part of its path>`.
- A golden's `.toml` holds `stdout = '''…'''` (and `status`/`stderr` for a
  failing run); copy a neighbour's. Establish each expected output by
  reasoning from this plan before running it; if the run disagrees, find
  out which is wrong -- never copy the run's output into the `.toml`
  because the code produced it.

## Review Focus

- **A collection given itself as argument** -- `list addAll: list`,
  `list , list`, `set addAll: set`, `map addAll: map` must answer what a
  person expects (the list doubled; the set and map unchanged), not
  `ConcurrentModification` (Task 5 and Task 6 and Task 7 goldens).
- **A map after many removals** -- removed entries leave marks; lookups must
  still find every live key past them, a re-added key goes last in the
  order, and growth compacts without losing or reordering entries (Task 6
  golden `a_map_removes_and_keeps_its_order`).
- **What does and does not count as a modification** -- `at:put:` on an
  existing index or key, `Set add:` of an equal element and `sort:` must
  not signal; an `add:` in the last step of a one-element `do:` must
  (Task 5, 6, 7 goldens).
- **Strings beyond Latin-1** -- searching, `stringAt:`, `do:` and
  `withAll:` on wide strings (code points above U+FFFF), a Symbol receiver,
  an empty substring at `size + 1` (Task 4 golden and Rust tests).
- **A program's own collection** -- a subclass of `Collection` that defines
  only `do:` gets the whole read protocol, and `collect:` without the hook
  signals the documented Error, not a `doesNotUnderstand:` (Task 2 golden);
  DeltaBlue's `Plan` (a `List` subclass) still runs (the benchmarks
  goldens under `goldens/tool/awfy/`).

---

### Task 1: The documentation rule, and the kernel and the platform documented to it

**Files:**
- Modify: `CLAUDE.md` ("Conventions", the rule "A public member of a
  shipped core-library module is documented")
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (comments only)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (comments only)
- Create: `crates/ns-runtime/tests/core_library_documentation.rs`
- Modify: `docs/design/core-library.md` (wherever it states the "one to
  four lines" rule)

**Interfaces:**
- Consumes: nothing.
- Produces: the test `every_member_of_a_documented_module_has_a_comment`
  with a list `DOCUMENTED_MODULES` of file names; Task 7 appends
  `"CollectionsFactory.ns"` to it.

- [ ] **Step 1: Write the failing test**

`crates/ns-runtime/tests/core_library_documentation.rs`. It parses each
listed module with `ns_syntax::parse`, collects the comment spans with
`Lexer::run_keeping_comments` (`crates/ns-syntax/src/lexer.rs:51`; check
the path it is exported under and the field of `Lexed` holding the comment
spans), and walks every class declaration recursively -- the module's
class, every nested class on both sides, the way
`crates/ns-analyzer/src/document_symbols.rs` walks them -- visiting each
`SlotDeclaration`, `MethodDeclaration` and nested `ClassDeclaration`
(`crates/ns-syntax/src/ast.rs`). A member is documented when a comment
span ends before the member's span starts and only whitespace lies between
them. The module's own class needs a comment too.

```rust
//! Every member of a shipped core-library module is documented
//! (CLAUDE.md, "Conventions"): a comment directly above every class, slot
//! and method, whatever its access.

use std::path::Path;

/// The modules the rule holds today.
const DOCUMENTED_MODULES: [&str; 2] = ["Kernel.ns", "PlatformFactory.ns"];

#[test]
fn every_member_of_a_documented_module_has_a_comment() {
    let mut undocumented = Vec::new();
    for name in DOCUMENTED_MODULES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("core-library")
            .join(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let module = match ns_syntax::parse(&source) {
            Ok(module) => module,
            Err(_) => panic!("{name} does not parse"),
        };
        let comments = comment_spans(&source);
        for (member, start) in member_starts(&module) {
            if !directly_after_a_comment(&source, &comments, start) {
                undocumented.push(format!("{name}: {member}"));
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "members without a comment directly above them:\n{}",
        undocumented.join("\n")
    );
}
```

Write the three helpers:

- `comment_spans(source) -> Vec<(usize, usize)>`: the lexer's comment
  spans as byte ranges.
- `member_starts(module) -> Vec<(String, usize)>`: each member's printable
  name -- `Kernel>>Array>>at:put:`, `Kernel>>Array class>>new:`,
  `Kernel>>Exception>>signalActivation (slot)`, `Kernel>>Array (class)` --
  and the byte offset its declaration starts at: the access modifier when
  there is one, `class` for a class, the selector's first token for a
  method, the slot's name for a slot without a modifier.
- `directly_after_a_comment(source, comments, start) -> bool`: some comment
  ends at or before `start` and `source[end..start]` is whitespace only.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-runtime --test core_library_documentation`
Expected: FAIL, listing the members of `Kernel.ns` and `PlatformFactory.ns`
without a comment directly above them (slots such as `Exception`'s
`signalActivation`, members sharing a family comment such as
`Closure>>valueOf:with:`).

- [ ] **Step 3: The rule in `CLAUDE.md`**

Replace the paragraph that begins "**A public member of a shipped
core-library module is documented.**" with:

```markdown
- **Every member of a shipped core-library module is documented** to the
  standard of Javadoc. Every method, slot and class of `Kernel.ns`,
  `PlatformFactory.ns` and `CollectionsFactory.ns` -- public, protected and
  private -- has a `(* … *)` comment directly above it that says what it
  does and what it answers, each argument and what it must be, every Error
  it signals and when, and what it guarantees about order, identity,
  copying and mutation, as long as that takes; a section that would say
  nothing is left out. It names no other implementation. A comment that
  explains a decision, as the rule above asks, goes after it.
  `crates/ns-runtime/tests/core_library_documentation.rs` holds every
  member to having one.
```

`grep -n "one to four" docs/design/*.md docs/*.md` and rewrite each
statement of the old rule to the new one (plans under
`docs/superpowers/plans/` stay as written).

- [ ] **Step 4: Document every member of `Kernel.ns` and `PlatformFactory.ns`**

For every class, slot and method, public, protected and private, read its
body -- and, for a leaf, the Rust it calls (`crates/ns-runtime/src/builtins/`;
the row in `builtins/primitive.rs` names the primitive) -- and write the
comment the rule asks for. It states what the code makes true:

- what it answers; for a mutator whether it answers the receiver or the
  argument;
- each argument: what it must be (an Integer index from 1 to `size`, a
  block of two arguments, anything understanding `value`), and what happens
  to it (sent `value`, stored, compared with `=`);
- **every Error it signals, by class** -- read the leaf: an index error is
  an `ArgumentError` (`NewspeakError::argument_error`), not "an Error" --
  with the `messageText` as the code builds it, and when;
- order, identity, copying and mutation where they apply (a new String,
  the receiver unchanged, elements visited from the first);
- for a private or protected member, what it is for and what its callers
  rely on.

A family sharing one comment today (`+ - *`, the comparisons, the class
accessors on `Object`, the nil protocol, `value` to
`value:value:value:`) gets a comment above each member; a short one that
refers to the first member's ("As `value`, with one argument.") is enough
when the first says everything. Keep every comment that explains a
decision; it follows the doc comment. Remove from `PlatformFactory.ns`'s
doc comments every mention of another implementation
(`grep -n -i "primordial\|somns" crates/ns-runtime/core-library/PlatformFactory.ns`);
a decision comment may keep one.

The standard, for `Array>>at:put:` as it is today:

```newspeak
        (* Stores `value` at `index` and answers `value`.

           index: an Integer from 1 to `size`.
           value: any object; the Array holds it from now on.

           Signals an ArgumentError carrying `index`: 'index … out of
           range: valid indices are 1..size (size …)' for an Integer outside
           1 to `size`, a LargeInteger included, and 'at:put: requires an
           Integer index, got …' for an index that is no Integer. *)
        public at: index put: value = (
```

and for a private member, `Closure>>valueOf:`:

```newspeak
        (* Sends `value` to `action` and answers what it answers. A native
           loop activates it for a loop body that is no block, so that the
           loop carries on from this activation's return whatever the send
           decides.

           action: any object; sent `value`. *)
        private valueOf: action = ( ^action value )
```

Only comments change: `git diff --stat` shows the two `.ns` files,
`CLAUDE.md`, the design documents and the new test, and every changed line
in the two `.ns` files is a comment line.

- [ ] **Step 5: Run the test and every golden**

Run: `cargo nextest run -p ns-runtime --test core_library_documentation`
Expected: PASS.
Run: `cargo nextest run -p ns-cli --test run_goldens --test source_formatting`
Expected: PASS (no behaviour changed).

- [ ] **Step 6: Gates and commit**

Commit: `docs: document every member of the kernel and the platform`.

---

### Task 2: `Collection` in the kernel, `Array` under it

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Collection` new,
  before `Array`; `Array`'s clause; `Object>>subclassResponsibility`)
- Modify: `crates/ns-runtime/src/builtins/activation.rs`,
  `crates/ns-runtime/src/builtins/primitive.rs`
  (`ActivationPrimitive::MethodName`, the row `activationMethodName:ifFail:`)
- Create: `goldens/library/object/subclass_responsibility_names_the_abstract_method.{ns,toml}`
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::Collection`,
  appended; `ALL` grows to 33)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`:
  38), `crates/ns-runtime/src/image/decode.rs`
  (`builtin_class_from_tag`: 38), `crates/ns-runtime/tests/opcode_tags.rs`
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs`
  (`kernel_class_name`, the hierarchy test, the class-accessor absent-list)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns`
  (`KernelNamespace>>Collection`)
- Modify: `goldens/library/exception/the_kernel_namespace_answers_the_exception_classes.{ns,toml}`
- Create: `goldens/library/collection/a_collection_is_built_on_do.{ns,toml}`
- Modify: `docs/design/core-library.md`, `docs/design/image-format.md`
  (the tag table), `docs/design/object-model.md` if it draws the kernel
  hierarchy

**Interfaces:**
- Consumes: the documentation rule (Task 1).
- Produces: `Object>>subclassResponsibility` (protected); the mirror row
  `activationMethodName:ifFail:`; `BuiltinClass::Collection` (tag 38); the kernel class
  `Collection` with the public protocol below and the protected hook
  `newForCollectUsingAdd: capacity`; `platform kernel Collection`; `Array`
  as a subclass of `Collection`.

- [ ] **Step 1: Write the failing golden**

`goldens/library/collection/a_collection_is_built_on_do.ns`:

```newspeak
(* A program's own Collection defines `do:` and inherits the rest of the
   read protocol; `collect:`, `select:` and `reject:` also need the hook
   `newForCollectUsingAdd:`, and without it signal an Error. An Array is a
   Collection and inherits what it does not define itself. *)
class ACollectionIsBuiltOnDo = Object new (
    | Collection ::= nil. |
) (
    class Digits = Collection new () (
        public do: action = (
            action value: 3.
            action value: 1.
            action value: 4
        )
    )
    class Nothing = Collection new () (
        public do: action = ( )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. digits nothing |
        Collection:: kernel Collection.
        digits:: Digits new.
        nothing:: Nothing new.
        transcript show: digits size printString; newLine.
        transcript show: digits isEmpty printString; newLine.
        transcript show: (digits includes: 4) printString; newLine.
        transcript show: (digits includes: 5) printString; newLine.
        transcript show: (digits inject: 0 into: [:sum :each | sum + each]) printString; newLine.
        transcript show: (digits reduce: [:left :right | left * 10 + right]) printString; newLine.
        transcript show: (digits detect: [:each | each < 3]) printString; newLine.
        transcript show: (digits detect: [:each | each > 9] ifNone: [0]) printString; newLine.
        transcript show: ([digits detect: [:each | each > 9]] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: (digits allSatisfy: [:each | each > 0]) printString; newLine.
        transcript show: (digits anySatisfy: [:each | each > 3]) printString; newLine.
        transcript show: (digits noneSatisfy: [:each | each > 3]) printString; newLine.
        digits do: [:each | transcript show: each printString] separatedBy: [transcript show: ', '].
        transcript newLine.
        transcript show: ([digits collect: [:each | each]] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: nothing isEmpty printString; newLine.
        transcript show: ([nothing reduce: [:left :right | left]] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: (nothing reduce: [:left :right | left] ifEmpty: [#none]) printString; newLine.
        transcript show: (nothing allSatisfy: [:each | false]) printString; newLine.
        transcript show: (nothing anySatisfy: [:each | true]) printString; newLine.
        transcript show: digits isKindOfCollection printString; newLine.
        transcript show: 3 isKindOfCollection printString; newLine.
        transcript show: {1. 2. 3} isKindOfCollection printString; newLine.
        transcript show: ({1. 2. 3} inject: 0 into: [:sum :each | sum + each]) printString; newLine.
        transcript show: ({1. 2. 3} includes: 2) printString; newLine.
        transcript show: kernel Collection printString; newLine.
    )
) : ()
```

`a_collection_is_built_on_do.toml`:

```toml
stdout = '''
3
false
true
false
8
314
1
0
No matching element detected
true
true
false
3, 1, 4
Collection>>newForCollectUsingAdd: is abstract: Digits must implement it
true
Cannot reduce an empty collection
#none
true
false
true
false
true
6
true
Collection
'''
```

`goldens/library/object/subclass_responsibility_names_the_abstract_method.ns`:

```newspeak
(* `self subclassResponsibility` signals an Error naming the abstract
   method -- the class that declares it and its selector -- and the class
   of the receiver that should have implemented it. It is protected: a
   send from outside is not understood. *)
class SubclassResponsibilityNamesTheAbstractMethod = Object new () (
    class Shape = Object new () (
        public area = ( ^self subclassResponsibility )
        public describe = ( ^'area ', self area printString )
    )
    class Square side: theSide = Shape new (
        | side = theSide. |
    ) (
        public area = ( ^side * side )
    )
    class Circle = Shape new () (
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. |
        transcript show: (Square side: 3) describe; newLine.
        transcript show: ([Circle new describe] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([Shape new area] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([Circle new subclassResponsibility] on: kernel MessageNotUnderstood do: [:error | #protected]) printString; newLine.
    )
) : ()
```

`subclass_responsibility_names_the_abstract_method.toml`:

```toml
stdout = '''
area 9
Shape>>area is abstract: Circle must implement it
Shape>>area is abstract: Shape must implement it
#protected
'''
```

In the kernel facet golden
(`goldens/library/exception/the_kernel_namespace_answers_the_exception_classes.ns`)
add after the `Proxy` line:

```newspeak
        transcript show: kernel Collection printString; newLine.
```

and in its `.toml` the line `Collection` after `Proxy`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_collection_is_built_on_do the_kernel_namespace subclass_responsibility`
Expected: FAIL -- `platform kernel` does not understand `Collection`.

- [ ] **Step 3: `Collection` and `Array`'s clause in `Kernel.ns`**

Insert before `Array`'s class comment:

```newspeak
    (* A group of elements that can be visited one by one: the abstract
       superclass of `Array` and of the collections module's `List`, `Map`
       and `Set`. A subclass defines `do:`, and inherits every other public
       member here, each written over `do:`; to `collect:`, `select:` or
       `reject:` it also defines `newForCollectUsingAdd:`. A subclass
       overrides any member it can answer faster. *)
    public class Collection = Object new () (
        (* Answers true: the receiver is a Collection. *)
        public isKindOfCollection = ( ^true )
        (* Evaluates `action` with each element in turn.

           action: a one-argument block, or anything understanding
               `value:`.

           Here, signals the Error of `subclassResponsibility`,
           'Collection>>do: is abstract: … must implement it': every
           subclass defines it. *)
        public do: action = ( ^self subclassResponsibility )
        (* Answers a new, empty collection that `collect:`, `select:` and
           `reject:` fill by sending it `add:` once per element they keep.

           capacity: an Integer of at least 0, the receiver's size: room
               enough for every element.

           Here, signals the Error of `subclassResponsibility`,
           'Collection>>newForCollectUsingAdd: is abstract: … must
           implement it'. *)
        (* The kernel cannot name the collections module's `List`, so a
           collection says itself what its `collect:` answers. *)
        protected newForCollectUsingAdd: capacity = ( ^self subclassResponsibility )
        (* Answers whether `predicate` answers true for every element; true
           for an empty collection. Stops at the first element for which it
           answers false.

           predicate: a one-argument block answering a Boolean. *)
        public allSatisfy: predicate = (
            self do: [:element | (predicate value: element) ifFalse: [^false]].
            ^true
        )
        (* Answers whether `predicate` answers true for some element; false
           for an empty collection. Stops at the first element for which it
           answers true.

           predicate: a one-argument block answering a Boolean. *)
        public anySatisfy: predicate = (
            self do: [:element | (predicate value: element) ifTrue: [^true]].
            ^false
        )
        (* Answers whether `predicate` answers false for every element; true
           for an empty collection. Stops at the first element for which it
           answers true.

           predicate: a one-argument block answering a Boolean. *)
        public noneSatisfy: predicate = (
            self do: [:element | (predicate value: element) ifTrue: [^false]].
            ^true
        )
        (* Answers the first element, in `do:` order, for which `predicate`
           answers true.

           predicate: a one-argument block answering a Boolean.

           Signals an Error, 'No matching element detected', when it answers
           true for none. *)
        public detect: predicate = (
            ^self detect: predicate ifNone: [(outer Kernel Error new) signal: 'No matching element detected']
        )
        (* Answers the first element, in `do:` order, for which `predicate`
           answers true, or the value of `onNone` when it answers true for
           none.

           predicate: a one-argument block answering a Boolean.
           onNone: anything understanding `value`; sent it only when no
               element matches. *)
        public detect: predicate ifNone: onNone = (
            self do: [:element | (predicate value: element) ifTrue: [^element]].
            ^onNone value
        )
        (* Evaluates `action` with each element in `do:` order, and
           `between` between every two of them: not before the first, not
           after the last. Answers the receiver.

           action: a one-argument block, or anything understanding `value:`.
           between: anything understanding `value`. *)
        public do: action separatedBy: between = (
            | first ::= true. |
            self do: [:element |
                first ifTrue: [first:: false] ifFalse: [between value].
                action value: element]
        )
        (* Answers whether some element is `=` to `object`; each element is
           sent `=` with `object` as its argument, in `do:` order, until one
           answers true. *)
        public includes: object = (
            self do: [:element | element = object ifTrue: [^true]].
            ^false
        )
        (* Folds the elements from the left: answers `initial` for an empty
           collection, otherwise the last answer of `fold value: accumulated
           value: element`, where `accumulated` starts as `initial` and
           becomes each answer in turn.

           fold: a two-argument block. *)
        public inject: initial into: fold = (
            | accumulated ::= initial. |
            self do: [:element | accumulated:: fold value: accumulated value: element].
            ^accumulated
        )
        (* Answers whether the collection holds no element. *)
        public isEmpty = ( ^self size = 0 )
        (* Folds the elements from the left, starting with the first: a
           one-element collection answers that element without sending
           `merge`; a longer one answers the last answer of `merge value:
           accumulated value: element` over the rest.

           merge: a two-argument block.

           Signals an Error, 'Cannot reduce an empty collection', for an
           empty collection. *)
        public reduce: merge = (
            ^self reduce: merge ifEmpty: [(outer Kernel Error new) signal: 'Cannot reduce an empty collection']
        )
        (* As `reduce:`, but answers the value of `onEmpty` for an empty
           collection.

           merge: a two-argument block.
           onEmpty: anything understanding `value`; sent it only when the
               collection is empty. *)
        public reduce: merge ifEmpty: onEmpty = (
            | first ::= true. accumulated |
            self do: [:element |
                first
                    ifTrue: [first:: false. accumulated:: element]
                    ifFalse: [accumulated:: merge value: accumulated value: element]].
            first ifTrue: [^onEmpty value].
            ^accumulated
        )
        (* Answers a new collection, from `newForCollectUsingAdd:`, holding
           `transform`'s answer for each element, added in `do:` order. The
           receiver is unchanged.

           transform: a one-argument block.

           Signals what `newForCollectUsingAdd:` signals when the subclass
           does not define it. *)
        public collect: transform = (
            | result |
            result:: self newForCollectUsingAdd: self size.
            self do: [:element | result add: (transform value: element)].
            ^result
        )
        (* Answers a new collection, from `newForCollectUsingAdd:`, holding
           the elements for which `predicate` answers true, in `do:` order.
           The receiver is unchanged.

           predicate: a one-argument block answering a Boolean.

           Signals what `newForCollectUsingAdd:` signals when the subclass
           does not define it. *)
        public select: predicate = (
            | result |
            result:: self newForCollectUsingAdd: self size.
            self do: [:element | (predicate value: element) ifTrue: [result add: element]].
            ^result
        )
        (* Answers a new collection, from `newForCollectUsingAdd:`, holding
           the elements for which `predicate` answers false, in `do:` order.
           The receiver is unchanged.

           predicate: a one-argument block answering a Boolean.

           Signals what `newForCollectUsingAdd:` signals when the subclass
           does not define it. *)
        public reject: predicate = (
            | result |
            result:: self newForCollectUsingAdd: self size.
            self do: [:element | (predicate value: element) ifFalse: [result add: element]].
            ^result
        )
        (* Answers the number of elements, counted through `do:`. *)
        public size = (
            | count ::= 0. |
            self do: [:element | count:: count + 1].
            ^count
        )
    )
```

`Array`'s clause becomes `public class Array = Collection new () (`.
Update `Array`'s class comment: "A fixed-size sequence of objects, indexed
from 1, and a Collection: …".

- [ ] **Step 4: `subclassResponsibility` and its leaf**

`Kernel.ns`, in `Object`, after `identityHash`:

```newspeak
        (* Signals an Error saying that the method which sent it is
           abstract: 'Collection>>do: is abstract: Nothing must implement
           it' -- the class declaring the sending method and its selector,
           then the class of the receiver -- or 'a method is abstract: …'
           when the sender is no method. What an abstract method's body
           sends, `^self subclassResponsibility`; answers what a handler
           that resumes the Error answers. *)
        (* The name is read off the sending activation, which only the
           kernel reaches: a program sees the Error, never the activation. *)
        protected subclassResponsibility = (
            | current method |
            current:: vmMirror currentActivation: self ifFail: [nil].
            method:: vmMirror activationMethodName: (vmMirror activationSender: current ifFail: [nil]) ifFail: [nil].
            ^(outer Kernel Error new)
                signal: (method isNil ifTrue: ['a method'] ifFalse: [method]), ' is abstract: ',
                    self class name, ' must implement it'
        )
```

`primitive.rs`: `ActivationPrimitive::MethodName`, `false` in the
"answers no" match beside the other activation leaves; the row beside
`activationSender:ifFail:`:

```rust
    mirror(
        "activationMethodName:ifFail:",
        "methodName",
        Primitive::Activation(ActivationPrimitive::MethodName),
    ),
```

`activation.rs`: `SubjectLeaf::MethodName`, run by `activation_leaf`:
for a live activation, take `heap.activation_code(activation)` and
`heap.activation_current_class(activation)`; find, among the methods of
that class's mixin (the class object's application's mixin; for a
metaclass the class side's methods), the selector whose compiled code is
`Rc::ptr_eq` the activation's code. Find the reader with
`grep -rn "fn methods\|method_table\|fn compiled_method" crates/ns-runtime/src`
(a reader of compiled code takes the mixin, CLAUDE.md). Answer the String
`"{class name}>>{selector}"`, the class name as `className:ifFail:`
answers it (`Class>>name`; a metaclass's `"X class"`). Answer `nil` for
an activation that has returned, one running a block, an initializer or
a clause (a `CodeKind` other than `Method`), or one whose code no method
of the mixin holds.

A Rust test beside the activation leaves' tests (find them with
`grep -rln "activationSender" crates/ns-runtime/tests crates/ns-runtime/src`):
a module whose `Shape>>area` is `^self subclassResponsibility`, run under
both lowerings as the neighbours run them, whose error's `messageText` for
`Circle new area` is `'Shape>>area is abstract: Circle must implement it'`.

The test `a_proxy_overrides_every_public_member_of_object_as_protected`
concerns public members only; `subclassResponsibility` is protected, so
`Proxy` needs no override. If a kernel table test (quick methods,
forwarders) names it, add what it reports.

- [ ] **Step 5: The builtin**

- `types.rs`: append `Collection` to `BuiltinClass` and `ALL` (33
  entries), doc comment "The abstract superclass of `Array` and of the
  collections module's `List`, `Map` and `Set`; a builtin because `Array`'s
  superclass clause names it." Follow the checklist in the doc comment of
  `BuiltinClass::ALL` (`types.rs:162-178`): `kernel_class_name`
  (`"Collection"`), the image's tag tables (`encode.rs`
  `builtin_class_tag` → 38, `decode.rs` `builtin_class_from_tag`,
  `tests/opcode_tags.rs`), and the class-accessor lists: `Object` declares
  no accessor for `Collection`, so it joins the absent-list of
  `objects_class_accessors_are_exactly_the_nameable_builtins`. A program
  may name it (through `platform kernel`), so it is not filtered or refused
  in `encode_tag_named_hashes`, `named_builtin` or
  `referenced_builtin_class`.
- `interpreter/kernel.rs`'s `the_hierarchy_follows_the_specification`:
  `(BuiltinClass::Collection, Some(BuiltinClass::Object))` and
  `(BuiltinClass::Array, Some(BuiltinClass::Collection))`.
- Whatever further table a kernel test reports (`check_kernel_shape`'s
  lists, `NOT_FORWARDERS`, quick-method tables): add `Collection`'s
  entries as the failing test names them. If a check refuses a builtin with
  no leaf at all, stop and report.

- [ ] **Step 6: The facet**

`PlatformFactory.ns`'s `KernelNamespace`, after `Proxy`:

```newspeak
        (* Answers the kernel's `Collection` class, the abstract superclass
           of `Array`, `List`, `Map` and `Set`, for a program to subclass. *)
        public Collection = ( ^kernelModule Collection )
```

- [ ] **Step 7: Run the goldens and the kernel and image tests**

Run: `cargo nextest run -p ns-cli --test run_goldens`
Run: `cargo nextest run -p ns-runtime`
Expected: PASS. (`Array`'s instances and leaves are unchanged; only its
superclass moved, so every Array golden still passes.)

- [ ] **Step 8: Documentation**

- `docs/design/core-library.md`: `subclassResponsibility` where `Object`'s
  protocol is described, and the activation leaf where the activation
  leaves are listed ("Exceptions"); the hierarchy drawing near the top (the
  tree with `Object`, `Value`, … at about line 53) gains `Collection` under
  `Object` with `Array` under it; the paragraph listing the builtins names
  `Collection` and why it is one; the "Planned: …" section's sub-project 3
  is not yet changed (Task 9 does it).
- `docs/design/image-format.md`: the builtin class tag table gains 38,
  `Collection`.

- [ ] **Step 9: Gates and commit**

Commit: `feat: an abstract Collection in the kernel, with Array under it, and subclassResponsibility`.

---

### Task 3: `Array`'s whole protocol, and its two leaves

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Array`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`
  (`CollectionPrimitive::CopyRange`, `CollectionPrimitive::Replace`, two
  rows, the exhaustive matches)
- Modify: `crates/ns-runtime/src/builtins/collections.rs` (the two leaves;
  the range check shared with `String`'s)
- Modify: `crates/ns-runtime/src/builtins/text.rs` (`copy_range` uses the
  shared range check)
- Create: `crates/ns-runtime/tests/array_leaves.rs` (or extend the test file
  that already exercises `collection:at:put:ifFail:` -- `grep -rln
  "at:put:" crates/ns-runtime/tests/` -- following its helpers)
- Create: `goldens/library/collection/an_array_copies_searches_and_sorts.{ns,toml}`
- Create: `goldens/library/collection/an_array_range_error_is_an_argument_error.{ns,toml}`

**Interfaces:**
- Consumes: `Collection` (Task 2).
- Produces: on `Array`: `, collection`, `asArray`, `collect:`, `select:`,
  `reject:`, `copyFrom:to:`, `copyWith:`, `copyWithSize:`, `first`,
  `last`, `indexOf:`, `isEmpty`, `keysAndValuesDo:`,
  `replaceFrom:to:with:startingAt:`, `sort:`, `sortFrom:to:by:`, class-side
  `withAll:`; the mirror rows `collection:copyFrom:to:ifFail:` and
  `collection:replaceFrom:to:with:startingAt:ifFail:`; in `collections.rs`
  a `pub(crate) fn checked_range(…)` that `text.rs` calls too.

- [ ] **Step 1: Write the failing goldens**

`an_array_copies_searches_and_sorts.ns`:

```newspeak
(* An Array's own protocol: concatenating, copying, searching, sorting --
   stably -- and replacing a range from any source answering `at:`,
   itself included with overlapping ranges. `collect:`, `select:` and
   `reject:` answer Arrays. *)
class AnArrayCopiesSearchesAndSorts = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. array show shifted pairs |
        show:: [:collection |
            collection do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
            transcript newLine].
        array:: {5. 3. 8. 1}.
        show value: array , {9. 7}.
        show value: (array copyFrom: 2 to: 3).
        show value: (array copyFrom: 3 to: 2).
        show value: (array copyWith: 0).
        show value: (array copyWithSize: 2).
        show value: (array copyWithSize: 6).
        show value: (array collect: [:each | each * 2]).
        show value: (array select: [:each | each > 3]).
        show value: (array reject: [:each | each > 3]).
        transcript show: (array select: [:each | true]) class printString; newLine.
        transcript show: array first printString; show: ' '; show: array last printString; newLine.
        transcript show: (array indexOf: 8) printString; show: ' '; show: (array indexOf: 4) printString; newLine.
        transcript show: array isEmpty printString; show: ' '; show: (Array new: 0) isEmpty printString; newLine.
        transcript show: (array asArray == array) printString; newLine.
        array keysAndValuesDo: [:index :each |
            transcript show: index printString; show: '='; show: each printString; show: ' '].
        transcript newLine.
        show value: ((Array withAll: array) sort: [:left :right | left <= right]).
        show value: array.
        pairs:: {{2. #a}. {1. #b}. {2. #c}. {1. #d}}.
        pairs sort: [:left :right | left first <= right first].
        show value: (pairs collect: [:pair | pair last]).
        show value: ({4. 3. 2. 1} sortFrom: 2 to: 3 by: [:left :right | left <= right]).
        shifted:: {1. 2. 3. 4. 5}.
        shifted replaceFrom: 2 to: 5 with: shifted startingAt: 1.
        show value: shifted.
        shifted:: {1. 2. 3. 4. 5}.
        shifted replaceFrom: 1 to: 4 with: shifted startingAt: 2.
        show value: shifted.
        show value: ((Array new: 3) replaceFrom: 1 to: 3 with: 'abc' startingAt: 1).
        show value: {1} , 'ab'.
    )
) : ()
```

`an_array_copies_searches_and_sorts.toml` (the empty range prints an empty
line; the `keysAndValuesDo:` line ends in a space):

```toml
stdout = '''
5 3 8 1 9 7
3 8

5 3 8 1 0
5 3
5 3 8 1 nil nil
10 6 16 2
5 8
3 1
Array
5 1
3 0
false true
true
1=5 2=3 3=8 4=1 
1 3 5 8
5 3 8 1
#b #d #a #c
4 2 3 1
1 1 2 3 4
2 3 4 5 5
97 98 99
1 97 98
'''
```

`an_array_range_error_is_an_argument_error.ns`:

```newspeak
(* A range an Array does not hold is an ArgumentError carrying the bound
   that is out of range, with copyFrom:to:'s text as a String's; a source
   too short for replaceFrom:to:with:startingAt: refuses its start. *)
class AnArrayRangeErrorIsAnArgumentError = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. array |
        array:: {5. 3. 8. 1}.
        transcript show: ([array copyFrom: 0 to: 2] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([array copyFrom: 2 to: 9] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([array copyFrom: 2 to: 9] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([array copyFrom: #a to: 2] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([array replaceFrom: 4 to: 5 with: {1. 2} startingAt: 1] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([(Array new: 0) first] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([(Array new: 0) last] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
    )
) : ()
```

`an_array_range_error_is_an_argument_error.toml`:

```toml
stdout = '''
0
9
copyFrom: 2 to: 9 out of range: valid ranges lie within 1..4, an empty range ends just before it starts (size 4)
copyFrom:to: requires an Integer index, got #a
1
startingAt: 1 out of range: the source holds 1 element, the range needs 2 from 1
5
1
0
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens an_array_`
Expected: FAIL -- `,` and the rest are not understood.

- [ ] **Step 3: The shared range check**

Move the bound checking of `text.rs`'s `copy_range` (lines ~134-192: the
two `bound` reads, `start >= 1 && stop >= start - 1`, the "refused" bound
and the text) into `collections.rs`:

```rust
/// The zero-based positions `start - 1 .. stop` of a range `copyFrom:to:`
/// or `replaceFrom:to:` names in a sequence of `size` elements, or the
/// `ArgumentError` it signals: one carrying the bound out of range -- the
/// start unless it is a position the sequence has, one past the end
/// included for the empty range, then the stop -- and the text
/// "<keyword> … to: … out of range: valid ranges lie within 1..size, an
/// empty range ends just before it starts (size …)", or the text of a
/// bound that is no Integer.
pub(crate) fn checked_range<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    keyword: &str,
    start: Option<Value<'gc>>,
    stop: Option<Value<'gc>>,
    size: usize,
) -> Result<std::ops::Range<usize>, NewspeakError<'gc>>
```

`keyword` is `"copyFrom:"` or `"replaceFrom:"`; the text reads
`"{keyword} {start} to: {stop} out of range: …"`, so `String>>copyFrom:to:`'s
text is unchanged. `copy_range` calls it and slices with the answer;
`goldens/library/string/` keeps passing unchanged.

- [ ] **Step 4: The two leaves**

`primitive.rs`: `CollectionPrimitive::CopyRange` and
`CollectionPrimitive::Replace`, both `false` in the "answers no" match at
~`:153`; rows beside `collection:at:put:ifFail:`:

```rust
    mirror(
        "collection:copyFrom:to:ifFail:",
        "copyFrom:to:",
        Primitive::Collection(CollectionPrimitive::CopyRange),
    ),
    mirror(
        "collection:replaceFrom:to:with:startingAt:ifFail:",
        "replaceFrom:to:with:startingAt:",
        Primitive::Collection(CollectionPrimitive::Replace),
    ),
```

`collections.rs`, in `execute`:

```rust
        CollectionPrimitive::CopyRange => {
            leaf_step(copy_range(mutator, receiver, selector, arguments))
        }
        CollectionPrimitive::Replace => leaf_step(replace(mutator, receiver, selector, arguments)),
```

```rust
/// `copyFrom:to:` on an `Array`: a new Array of the elements in the range.
/// `None` when the receiver is no Array.
fn copy_range<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "copyFrom:",
        arguments.first().copied(),
        arguments.get(1).copied(),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    // Read before allocating: the new array's allocation may collect.
    let elements: Vec<Value<'gc>> = range
        .filter_map(|position| mutator.heap.array_element(object, position))
        .collect();
    Some(Ok(Value::object(mutator.new_array(elements))))
}
```

(If `new_array` can move or collect, check how `Mutator::new_array` is
used by the neighbours; a `Value` read out of the heap is held across an
allocation the same way `allocate` and the handler search do.)

```rust
/// `replaceFrom:to:with:startingAt:` on an `Array` from an `Array`:
/// stores the source's elements from `startingAt` on into the range,
/// through the write barrier. Reads every source element before storing
/// any, so overlapping ranges of one array copy as if through a buffer.
/// `None` -- the Newspeak fallback runs -- when the receiver or the source
/// is no Array.
fn replace<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let source = arguments.get(2).copied()?.as_object()?;
    let source_size = mutator.heap.array_element_count(source)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "replaceFrom:",
        arguments.first().copied(),
        arguments.get(1).copied(),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let source_start = match source_start(
        &mutator.heap,
        receiver,
        selector,
        arguments.get(3).copied(),
        range.len(),
        source_size,
    ) {
        Ok(start) => start,
        Err(error) => return Some(Err(error)),
    };
    let elements: Vec<Value<'gc>> = (source_start..source_start + range.len())
        .filter_map(|position| mutator.heap.array_element(source, position))
        .collect();
    for (position, element) in range.zip(elements) {
        mutator.heap.array_set_element(object, position, element);
    }
    Some(Ok(receiver))
}
```

`source_start` answers the zero-based start, or an `ArgumentError`
carrying the `startingAt:` argument: for a non-Integer, the
`not_an_integer` text under the selector; when `start < 1` or `start - 1 +
count > source_size`, the text
`"startingAt: {start} out of range: the source holds {source_size} element(s), the range needs {count} from {start}"`
-- "element" when `source_size` is 1, "elements" otherwise. When `count` is
0 any Integer `start` from 1 to `source_size + 1` is accepted.

- [ ] **Step 5: `Array`'s protocol in `Kernel.ns`**

Add to `Array` (instance side), each with the doc comment shown:

```newspeak
        (* Answers the receiver itself: an Array is its own Array. *)
        public asArray = ( ^self )
        (* Answers whether the Array has no element. *)
        public isEmpty = ( ^self size = 0 )
        (* Answers the first element.

           Signals the ArgumentError of `at:`, carrying 1, for an empty
           Array. *)
        public first = ( ^self at: 1 )
        (* Answers the last element.

           Signals the ArgumentError of `at:`, carrying 0, for an empty
           Array. *)
        public last = ( ^self at: self size )
        (* Answers the index of the first element `=` to `element`, or 0
           when there is none. Each element is sent `=` with `element`, from
           the first, until one answers true. *)
        public indexOf: element = (
            1 to: self size do: [:index | (self at: index) = element ifTrue: [^index]].
            ^0
        )
        (* Evaluates `action` with each index and the element there, from
           the first, reading each element when it reaches it. Answers the
           receiver.

           action: a two-argument block, given the index and the element. *)
        public keysAndValuesDo: action = (
            1 to: self size do: [:index | action value: index value: (self at: index)]
        )
        (* Answers a new Array of the elements from `start` to `stop`; empty
           when `stop` is `start - 1`. The receiver is unchanged.

           start, stop: Integers with 1 <= start <= stop + 1 and
               stop <= size.

           Signals an ArgumentError carrying the bound out of range,
           'copyFrom: … to: … out of range: valid ranges lie within
           1..size, an empty range ends just before it starts (size …)', or
           'copyFrom:to: requires an Integer index, got …' for a bound that
           is no Integer. *)
        public copyFrom: start to: stop = (
            ^vmMirror collection: self copyFrom: start to: stop
                ifFail: [^super copyFrom: start to: stop]
        )
        (* Stores the elements of `source` from `sourceStart` on into the
           receiver's positions `start` to `stop`, and answers the
           receiver. Correct when `source` is the receiver and the ranges
           overlap: every element is read before any is stored.

           start, stop: a range of the receiver, as for `copyFrom:to:`.
           source: an Array, or anything answering `size` and `at:` -- a
               List, a String, whose elements are its code points.
           sourceStart: an Integer with 1 <= sourceStart and
               sourceStart + (stop - start) <= source size.

           Signals the ArgumentError `copyFrom:to:` signals, its text
           beginning 'replaceFrom: … to: …', for a range the receiver does
           not hold, and an ArgumentError carrying `sourceStart`,
           'startingAt: … out of range: the source holds … elements, the
           range needs … from …', for a source too short. *)
        public replaceFrom: start to: stop with: source startingAt: sourceStart = (
            ^vmMirror collection: self replaceFrom: start to: stop with: source startingAt: sourceStart
                ifFail: [self replaceFrom: start to: stop withElementsOf: source startingAt: sourceStart]
        )
        (* `replaceFrom:to:with:startingAt:` for a source that is no Array:
           copies the whole source into a new Array of its size and hands
           that to the leaf with `sourceStart` unchanged, so the leaf checks
           the range and the start against a source of the same size, and
           every error text is the leaf's.

           source: anything answering `size` and `at:`; signals the
               MessageNotUnderstood of the first it lacks.

           Signals what `replaceFrom:to:with:startingAt:` documents. *)
        private replaceFrom: start to: stop withElementsOf: source startingAt: sourceStart = (
            | elements |
            elements:: outer Kernel Array new: source size.
            1 to: source size do: [:index | elements at: index put: (source at: index)].
            ^vmMirror collection: self replaceFrom: start to: stop with: elements startingAt: sourceStart
                ifFail: [^super replaceFrom: start to: stop with: source startingAt: sourceStart]
        )
        (* Answers a new Array of the receiver's elements followed by those
           of `collection`. The receiver is unchanged.

           collection: an Array, or anything answering `size` and `at:` --
               a List, a String, whose elements are its code points. *)
        public , collection = (
            | result |
            result:: outer Kernel Array new: self size + collection size.
            result replaceFrom: 1 to: self size with: self startingAt: 1.
            result replaceFrom: self size + 1 to: result size with: collection startingAt: 1.
            ^result
        )
        (* Answers a new Array one longer than the receiver, `element` last.
           The receiver is unchanged. *)
        public copyWith: element = (
            | result |
            result:: self copyWithSize: self size + 1.
            result at: result size put: element.
            ^result
        )
        (* Answers a new Array of `newSize` elements: the receiver's first
           ones, cut off when `newSize` is smaller than `size`, followed by
           nils when it is larger. The receiver is unchanged.

           newSize: an Integer of at least 0.

           Signals the ArgumentError of `Array new:` for a `newSize` that
           is negative or no Integer. *)
        public copyWithSize: newSize = (
            | result kept |
            result:: outer Kernel Array new: newSize.
            kept:: newSize < self size ifTrue: [newSize] ifFalse: [self size].
            result replaceFrom: 1 to: kept with: self startingAt: 1.
            ^result
        )
        (* Answers a new Array of `transform`'s answer for each element, in
           order. The receiver is unchanged.

           transform: a one-argument block. *)
        public collect: transform = (
            | result |
            result:: outer Kernel Array new: self size.
            1 to: self size do: [:index | result at: index put: (transform value: (self at: index))].
            ^result
        )
        (* Answers a new Array of the elements for which `predicate`
           answers true, in order. The receiver is unchanged.

           predicate: a one-argument block answering a Boolean. *)
        public select: predicate = (
            | result count |
            result:: outer Kernel Array new: self size.
            count:: 0.
            self do: [:element |
                (predicate value: element) ifTrue: [
                    count:: count + 1.
                    result at: count put: element]].
            ^result copyFrom: 1 to: count
        )
        (* Answers a new Array of the elements for which `predicate`
           answers false, in order. The receiver is unchanged.

           predicate: a one-argument block answering a Boolean. *)
        public reject: predicate = ( ^self select: [:element | (predicate value: element) not] )
        (* Sorts the elements in place, stably -- elements that may precede
           each other both ways keep their order -- and answers the
           receiver.

           lessOrEqual: a two-argument block answering whether its first
               argument may precede its second. *)
        public sort: lessOrEqual = ( ^self sortFrom: 1 to: self size by: lessOrEqual )
        (* Sorts the elements from `start` to `stop` in place, stably, as
           `sort:` does, and answers the receiver; the rest stay where they
           are.

           start, stop: a range of the receiver, as for `copyFrom:to:`.
           lessOrEqual: a two-argument block answering whether its first
               argument may precede its second.

           Signals the ArgumentError of `copyFrom:to:` for a range the
           receiver does not hold. *)
        (* A bottom-up merge sort over a copy of the range and a buffer of
           the same size, trading the two each pass: no recursion, so its
           depth does not grow with the range, and at most `size` extra
           slots. Taking from the left run while its element may precede
           the right run's keeps it stable. *)
        public sortFrom: start to: stop by: lessOrEqual = (
            | source target count width left swap |
            source:: self copyFrom: start to: stop.
            count:: source size.
            count < 2 ifTrue: [^self].
            target:: outer Kernel Array new: count.
            width:: 1.
            [width < count] whileTrue: [
                left:: 1.
                [left <= count] whileTrue: [
                    self merge: source into: target from: left width: width count: count by: lessOrEqual.
                    left:: left + width + width].
                swap:: source.
                source:: target.
                target:: swap.
                width:: width + width].
            self replaceFrom: start to: stop with: source startingAt: 1
        )
        (* Merges the sorted runs of `source` that start at `left` -- the
           first `width` elements long, the second the next `width`, each
           cut off at `count` -- into the same positions of `target`,
           taking from the first run while its element may precede the
           second's. *)
        private merge: source into: target from: left width: width count: count by: lessOrEqual = (
            | middle right leftCursor rightCursor targetCursor |
            middle:: left + width - 1.
            middle > count ifTrue: [middle:: count].
            right:: middle + width.
            right > count ifTrue: [right:: count].
            leftCursor:: left.
            rightCursor:: middle + 1.
            targetCursor:: left.
            [targetCursor <= right] whileTrue: [
                ((rightCursor > right) or: [(leftCursor <= middle)
                        and: [lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor)]])
                    ifTrue: [
                        target at: targetCursor put: (source at: leftCursor).
                        leftCursor:: leftCursor + 1]
                    ifFalse: [
                        target at: targetCursor put: (source at: rightCursor).
                        rightCursor:: rightCursor + 1].
                targetCursor:: targetCursor + 1]
        )
```

and on the class side:

```newspeak
        (* Answers a new Array of `collection`'s elements, in its `do:`
           order.

           collection: a Collection, or anything answering `size` and
               `do:` -- a String, whose elements are its code points. *)
        public withAll: collection = (
            | result index |
            result:: self new: collection size.
            index:: 0.
            collection do: [:element |
                index:: index + 1.
                result at: index put: element].
            ^result
        )
```

Check `merge:…`'s condition by hand on the stability line of the golden:
pairs (2 a)(1 b)(2 c)(1 d), width 1: (2a,1b) → 1b 2a; (2c,1d) → 1d 2c;
width 2: left run 1b 2a, right run 1d 2c: 1b<=1d take 1b; 2a<=1d no, take
1d; 2a<=2c take 2a; left run exhausted, take 2c → b d a c.

- [ ] **Step 6: Rust tests for the leaves**

In the test file chosen above, beside its neighbours and with its
helpers, one test each:

- `copy_from_to_answers_a_new_array_of_the_range`: `{1. 2. 3} copyFrom: 2
  to: 3` answers a new Array (`==` the receiver is false) holding 2 and 3;
  `copyFrom: 4 to: 3` an empty one.
- `replace_from_an_overlapping_range_of_the_same_array_reads_before_it_stores`:
  both directions, as in the golden.
- `replace_stores_a_young_element_into_an_old_array_through_the_barrier`:
  allocate an Array, tenure it (copy how
  `crates/ns-runtime/tests/collector.rs` or the generations tests make an
  object old -- a full collection between entries), then in a later entry
  `old replaceFrom: 1 to: 1 with: {Object new} startingAt: 1`, scavenge,
  and check that `(old at: 1) printString` still answers `'an Object'`.
  Run this test also with `NS_NEXT_STRESS_SCAVENGE=1` once by hand.

- [ ] **Step 7: Run everything touching arrays and strings**

Run: `cargo nextest run -p ns-cli --test run_goldens collection string`
Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 8: Documentation**

`docs/design/core-library.md`: the paragraph listing `Array`'s leaves
(about line 145, "`Array`'s `do:`, `at:put:`, `size` …") names the two
new ones; the description of `Array` names its protocol in one sentence
and the stable bottom-up merge sort.

- [ ] **Step 9: Gates and commit**

Commit: `feat: Array's whole protocol, with copyFrom:to: and replaceFrom:to:with:startingAt: as leaves`.

---

### Task 4: `String`'s protocol in code points, `stringAt:`, and three leaves

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`String`, both
  sides)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`
  (`StringPrimitive::IndexOf`, `LastIndexOf`, `FromCodePoints`, three
  rows, the exhaustive matches)
- Modify: `crates/ns-runtime/src/builtins/text.rs` (the three leaves)
- Modify: the Rust test file that exercises `string:copyFrom:to:ifFail:`
  (`grep -rln "copyFrom" crates/ns-runtime/tests/`) or create
  `crates/ns-runtime/tests/string_leaves.rs`
- Create: `goldens/library/string/a_string_searches_and_iterates_its_code_points.{ns,toml}`
- Create: `goldens/library/string/a_string_search_error_is_an_argument_error.{ns,toml}`

**Interfaces:**
- Consumes: `Array withAll:` (Task 3).
- Produces: on `String` (and so `Symbol`): `do:`, `first`, `last`,
  `isEmpty`, `stringAt:`, `copyStringFrom:to:`, `indexOf:`,
  `indexOf:startingAt:`, `lastIndexOf:`, `lastIndexOf:startingAt:`,
  `startsWith:`, `endsWith:`; class-side `withAll:`; the mirror rows
  `string:indexOf:startingAt:ifFail:`,
  `string:lastIndexOf:startingAt:ifFail:` and
  `stringClass:withAll:ifFail:`.

- [ ] **Step 1: Write the failing goldens**

`a_string_searches_and_iterates_its_code_points.ns` (the source file is
UTF-8; `wide` holds U+00E9 and U+1F600):

```newspeak
(* A String's elements are its code points, Integers, in both widths and
   past U+FFFF: `do:`, `first`, `last` and `withAll:` deal in them, and
   `stringAt:` answers the String of one. Searching answers a code-point
   index, or 0; a Symbol answers what its text does, as Strings. *)
class AStringSearchesAndIteratesItsCodePoints = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. text wide |
        text:: 'hello world'.
        wide:: 'aé😀b'.
        'abc' do: [:codePoint | transcript show: codePoint printString; show: ' '].
        transcript newLine.
        wide do: [:codePoint | transcript show: codePoint printString; show: ' '].
        transcript newLine.
        transcript show: text first printString; show: ' '; show: text last printString; newLine.
        transcript show: (wide stringAt: 3); show: ' '; show: (wide stringAt: 3) size printString; newLine.
        transcript show: '' isEmpty printString; show: ' '; show: text isEmpty printString; newLine.
        transcript show: (text indexOf: 'o') printString; newLine.
        transcript show: (text indexOf: 'o' startingAt: 6) printString; newLine.
        transcript show: (text indexOf: 'xyz') printString; newLine.
        transcript show: (text indexOf: '' startingAt: 12) printString; newLine.
        transcript show: (text lastIndexOf: 'o') printString; newLine.
        transcript show: (text lastIndexOf: 'o' startingAt: 7) printString; newLine.
        transcript show: (text lastIndexOf: 'l') printString; newLine.
        transcript show: (wide indexOf: '😀b') printString; newLine.
        transcript show: (text startsWith: 'hello') printString; show: ' ';
            show: (text startsWith: 'world') printString; show: ' ';
            show: (text endsWith: 'world') printString; show: ' ';
            show: (text endsWith: 'hello') printString; show: ' ';
            show: (text endsWith: '') printString; show: ' ';
            show: ('' startsWith: 'a') printString; newLine.
        transcript show: (#hello: startsWith: 'hel') printString; newLine.
        transcript show: (#abc stringAt: 2) printString; show: ' '; show: (#abc stringAt: 2) class printString; newLine.
        transcript show: (#abc copyStringFrom: 2 to: 3) printString; newLine.
        transcript show: (String withAll: {104. 105}); newLine.
        transcript show: (String withAll: 'é'); newLine.
        transcript show: ((String withAll: (Array withAll: wide)) = wide) printString; newLine.
        transcript show: (String withAll: {128512}) size printString; newLine.
        (Array withAll: 'hi') do: [:codePoint | transcript show: codePoint printString; show: ' '].
        transcript newLine.
    )
) : ()
```

`a_string_searches_and_iterates_its_code_points.toml` (the two `do:` lines
and the last end in a space):

```toml
stdout = '''
97 98 99 
97 233 128512 98 
104 100
😀 1
true false
5
8
0
12
8
5
10
3
true false true false true false
true
'b' String
'bc'
hi
é
true
1
104 105 
'''
```

`a_string_search_error_is_an_argument_error.ns`:

```newspeak
(* A start a search cannot begin at, an argument that is no String and a
   code point that is none are ArgumentErrors carrying what was refused. *)
class AStringSearchErrorIsAnArgumentError = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. text |
        text:: 'hello world'.
        transcript show: ([text indexOf: 'o' startingAt: 13] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([text indexOf: 'o' startingAt: 0] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([text lastIndexOf: 'o' startingAt: 13] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([text indexOf: 3] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([text startsWith: 3] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([text endsWith: 3] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([String withAll: {104. 55296}] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([String withAll: {104. 55296}] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: ([text stringAt: 12] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
    )
) : ()
```

`a_string_search_error_is_an_argument_error.toml`:

```toml
stdout = '''
13
indexOf:startingAt: 0 out of range: valid starts are 1..12 (size 11)
13
indexOf:startingAt: requires a String argument, got 3
3
endsWith: requires a String argument, got 3
55296
withAll: requires Unicode code points, Integers from 0 to 1114111 outside 55296 to 57343, got 55296
12
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_string_search`
Expected: FAIL -- `do:` and the rest are not understood.

- [ ] **Step 3: The three leaves**

`primitive.rs`: `StringPrimitive::IndexOf`, `StringPrimitive::LastIndexOf`,
`StringPrimitive::FromCodePoints` (doc: "`String withAll: codePoints`, on
`String`'s class side: its subject is the class."), each `false` in the
"answers no" match; rows beside `string:copyFrom:to:ifFail:`:

```rust
    mirror(
        "string:indexOf:startingAt:ifFail:",
        "indexOf:startingAt:",
        Primitive::String(StringPrimitive::IndexOf),
    ),
    mirror(
        "string:lastIndexOf:startingAt:ifFail:",
        "lastIndexOf:startingAt:",
        Primitive::String(StringPrimitive::LastIndexOf),
    ),
    mirror(
        "stringClass:withAll:ifFail:",
        "withAll:",
        Primitive::String(StringPrimitive::FromCodePoints),
    ),
```

`text.rs`, `execute_string`: `FromCodePoints` returns early like
`FromCodePoint` (its subject is the class); `IndexOf` and `LastIndexOf`
call one function:

```rust
/// `indexOf:startingAt:` and `lastIndexOf:startingAt:` over code points:
/// the 1-based index of the first occurrence of the argument that starts
/// at or after the start, or of the last that starts at or before it, or
/// 0. The start must lie in 1..size + 1; an empty argument is found at
/// the start itself.
fn search<'gc>(
    mutator: &Mutator<'gc>,
    object: ObjectReference<'gc>,
    size: usize,
    backwards: bool,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>>
```

- The argument must be a String or a Symbol (`heap.string_size` answers
  `Some`); otherwise an `ArgumentError` carrying it,
  `"{selector} requires a String argument, got {describe_argument}"`.
- The start through `collections::integer_argument`: a non-Integer gives
  `collections::not_an_integer`; an Integer outside `1..=size + 1` (a
  `LargeInteger` included) an `ArgumentError` carrying it,
  `"{selector} {start} out of range: valid starts are 1..{size + 1} (size {size})"`.
- Read both strings' code points (`heap.string_code_points`, whatever it
  answers -- collect into `Vec<u32>`), then scan: forwards from
  `start - 1` to `size - needle.len()`, backwards from
  `min(start - 1, size - needle.len())` down to 0; answer the position
  plus 1, or 0. A needle longer than the receiver answers 0.

`withAll:`:

```rust
/// `String withAll: codePoints` for an Array of code points or a String:
/// a new String of them. `None` -- the class side gathers any other
/// collection into an Array and asks again -- for any other argument, or
/// a subject that is not `String` itself.
fn from_code_points<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>>
```

For a String argument answer a new String of its code points. For an
Array, check every element as `from_code_point` checks its one argument
(a SmallInteger in 0..=1114111 outside 55296..=57343); the first that
fails gives an `ArgumentError` carrying that element,
`"withAll: requires Unicode code points, Integers from 0 to 1114111 outside 55296 to 57343, got {describe_argument}"`.
Then `mutator.new_string_from_code_points(…)`.

- [ ] **Step 4: `String`'s protocol in `Kernel.ns`**

Instance side, each with its doc comment:

```newspeak
        (* Evaluates `action` with each code point, an Integer, from the
           first. Answers the receiver.

           action: a one-argument block, or anything understanding
               `value:`. *)
        public do: action = (
            1 to: self size do: [:index | action value: (self at: index)]
        )
        (* Answers whether the String has no code point. *)
        public isEmpty = ( ^self size = 0 )
        (* Answers the first code point, an Integer.

           Signals the ArgumentError of `at:`, carrying 1, for an empty
           String. *)
        public first = ( ^self at: 1 )
        (* Answers the last code point, an Integer.

           Signals the ArgumentError of `at:`, carrying 0, for an empty
           String. *)
        public last = ( ^self at: self size )
        (* Answers a new String of the one code point at `index`; for a
           Symbol too.

           index: an Integer from 1 to `size`.

           Signals the ArgumentError of `at:` for any other index. *)
        public stringAt: index = (
            self at: index.
            ^self copyFrom: index to: index
        )
        (* Answers what `copyFrom: start to: stop` answers: a new String of
           the code points from `start` to `stop`.

           Signals what `copyFrom:to:` signals. *)
        public copyStringFrom: start to: stop = ( ^self copyFrom: start to: stop )
        (* Answers the index of the first occurrence of `substring`, or 0;
           `indexOf: substring startingAt: 1`. *)
        public indexOf: substring = ( ^self indexOf: substring startingAt: 1 )
        (* Answers the code-point index at which the first occurrence of
           `substring` that starts at or after `start` begins, or 0 when
           there is none. An empty `substring` is found at `start`.

           substring: a String or a Symbol.
           start: an Integer from 1 to `size + 1`.

           Signals an ArgumentError carrying `substring`,
           'indexOf:startingAt: requires a String argument, got …', for a
           `substring` that is no String, and one carrying `start`,
           'indexOf:startingAt: … out of range: valid starts are 1..…
           (size …)', for a `start` outside 1 to `size + 1`, or
           'indexOf:startingAt: requires an Integer index, got …'. *)
        public indexOf: substring startingAt: start = (
            ^vmMirror string: self indexOf: substring startingAt: start
                ifFail: [^super indexOf: substring startingAt: start]
        )
        (* Answers the index of the last occurrence of `substring`, or 0;
           `lastIndexOf: substring startingAt: size + 1`. *)
        public lastIndexOf: substring = ( ^self lastIndexOf: substring startingAt: self size + 1 )
        (* Answers the code-point index at which the last occurrence of
           `substring` that starts at or before `start` begins, or 0 when
           there is none. An empty `substring` is found at `start`.

           substring: a String or a Symbol.
           start: an Integer from 1 to `size + 1`.

           Signals the ArgumentErrors `indexOf:startingAt:` signals, their
           texts naming 'lastIndexOf:startingAt:'. *)
        public lastIndexOf: substring startingAt: start = (
            ^vmMirror string: self lastIndexOf: substring startingAt: start
                ifFail: [^super lastIndexOf: substring startingAt: start]
        )
        (* Answers whether the receiver begins with `prefix`; true for an
           empty `prefix`.

           prefix: a String or a Symbol.

           Signals an ArgumentError carrying `prefix`, 'startsWith:
           requires a String argument, got …' and its `printString`, for a
           `prefix` that is no String. *)
        (* Looks for an occurrence starting at 1 at the latest, so only the
           first position is compared, whatever the receiver's length. *)
        public startsWith: prefix = (
            prefix isKindOfString ifFalse: [
                ^(outer Kernel ArgumentError value: prefix)
                    signal: 'startsWith: requires a String argument, got ', prefix printString].
            ^(self lastIndexOf: prefix startingAt: 1) = 1
        )
        (* Answers whether the receiver ends with `suffix`; true for an
           empty `suffix`.

           suffix: a String or a Symbol.

           Signals an ArgumentError carrying `suffix`, 'endsWith: requires
           a String argument, got …' and its `printString`, for a `suffix`
           that is no String. *)
        (* The last occurrence is the one at the end when there is one
           there. *)
        public endsWith: suffix = (
            | position |
            suffix isKindOfString ifFalse: [
                ^(outer Kernel ArgumentError value: suffix)
                    signal: 'endsWith: requires a String argument, got ', suffix printString].
            position:: self lastIndexOf: suffix startingAt: self size + 1.
            ^(position > 0) and: [position = (self size - suffix size + 1)]
        )
```

(`'' endsWith: ''`: `lastIndexOf: '' startingAt: 1` answers 1, and
`0 - 0 + 1` is 1, so true. An empty receiver and a non-empty suffix: 0,
false.)

Class side, after `with:`:

```newspeak
        (* Answers a new String of the code points of `codePoints`, in its
           `do:` order, or a new String of the same code points when
           `codePoints` is a String.

           codePoints: a String, or a Collection of Integers, each a
               Unicode code point: from 0 to 1114111 and outside 55296 to
               57343. Anything else answering `size` and `do:` is gathered
               into an Array first.

           Signals an ArgumentError carrying the first element that is no
           code point, 'withAll: requires Unicode code points, Integers
           from 0 to 1114111 outside 55296 to 57343, got …', and the
           MessageNotUnderstood of `size` or `do:` for an argument that
           lacks it. Sent to a subclass, answers nil, as `with:` does. *)
        public withAll: codePoints = (
            ^vmMirror stringClass: self withAll: codePoints ifFail: [
                (codePoints isKindOfArray or: [codePoints isKindOfString])
                    ifTrue: [nil]
                    ifFalse: [self withAll: (outer Kernel Array withAll: codePoints)]]
        )
```

- [ ] **Step 5: Rust tests for the leaves**

Beside the neighbours, with their helpers:

- `index_of_finds_a_wide_needle_in_a_wide_string`: `'xé😀é😀' indexOf: 'é😀'
  startingAt: 3` answers 4; `lastIndexOf: 'é😀' startingAt: 3` answers 2.
- `an_empty_needle_is_found_at_the_start_itself`: `'abc' indexOf: ''
  startingAt: 4` answers 4, `lastIndexOf: '' startingAt: 1` answers 1.
- `a_needle_longer_than_the_string_is_not_found`: `'ab' indexOf: 'abc'`
  and `lastIndexOf: 'abc'` answer 0.
- `with_all_of_a_latin_one_array_makes_a_narrow_string`: `String withAll:
  {104. 233}` answers a String that `Heap` reports as the byte-indexed
  format (find the predicate `string_fields` or the format check the
  existing width tests use); `{104. 128512}` the 32-bit format.

- [ ] **Step 6: Run everything touching strings**

Run: `cargo nextest run -p ns-cli --test run_goldens string`
Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 7: Documentation**

`docs/design/core-library.md`, "Strings are code points in two widths":
a String's elements are code points; `stringAt:` and why it exists (the
String of one code point, as no `Character` exists); the search and
`withAll:` leaves; `asByteArray` and the ByteArray forms wait for
sub-project 5; `out` is not adopted. The leaf list (about line 144) names
the three new leaves.

- [ ] **Step 8: Gates and commit**

Commit: `feat: String's protocol in code points, with stringAt: and the search leaves`.

---

### Task 5: `List` rewritten under `Collection`, and `ConcurrentModification`

**Files:**
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (the
  module's slots, `Collections`, `List` written anew, `NotFound`'s
  comments, `ConcurrentModification` new, `signalModificationOf:` new)
- Rename and rewrite:
  `goldens/library/collection/do_visits_the_elements_a_list_held_when_it_started.{ns,toml}`
  → `goldens/library/collection/a_list_changed_while_iterated_signals.{ns,toml}`
  (`git mv`, then the content below)
- Create: `goldens/library/collection/a_list_has_the_whole_protocol.{ns,toml}`
- Modify: `goldens/library/collection/every_platform_shares_one_collections_module.{ns,toml}`
  if it lists the facet's classes (read it first)

**Interfaces:**
- Consumes: `Collection` (Task 2), `Array`'s `copyFrom:to:`,
  `replaceFrom:to:with:startingAt:`, `withAll:` (Task 3).
- Produces: `platform collections Collection`, `platform collections
  ConcurrentModification` (class side `collection: modified`, instance
  slot `collection`); the module's private `signalModificationOf:
  collection` and slot `kernelCollection`, which Tasks 6 and 7 use; `List`
  with the protocol below and a private slot `modifications`.

- [ ] **Step 1: Write the failing goldens**

`git mv` the `do_visits_…` pair to `a_list_changed_while_iterated_signals`
and replace the content. `.ns`:

```newspeak
(* A List changed while it is iterated -- an element added or removed
   inside the block of `do:`, `keysAndValuesDo:`, `anySatisfy:`,
   `removeAllSuchThat:` or anything built on `do:` -- signals a
   ConcurrentModification carrying the list, checked after every block,
   the last one included. Storing at an existing index or sorting is no
   such change. This replaces the earlier rule that `do:` visited the
   elements the list held when it started (decided with the user on
   2026-10-10). *)
class AListChangedWhileIteratedSignals = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. List = collections List. Modified = collections ConcurrentModification. list |
        list:: List withAll: {1. 2}.
        transcript show: ([list do: [:each | list add: each]] on: Modified do: [:error | error messageText]); newLine.
        transcript show: list size printString; newLine.
        transcript show: ([list do: [:each | list add: each]] on: Modified do: [:error | error collection == list]) printString; newLine.
        list:: List withAll: {1}.
        transcript show: ([list do: [:each | list removeFirst]. #finished] on: Modified do: [:error | #signalled]) printString; newLine.
        list:: List withAll: {3. 1. 2}.
        list do: [:each | list at: 1 put: each].
        transcript show: (list at: 1) printString; newLine.
        list do: [:each | list sort: [:left :right | left <= right]].
        list do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
        transcript newLine.
        transcript show: ([list keysAndValuesDo: [:index :each | list removeLast]] on: platform kernel Error do: [:error | error class printString]); newLine.
        list:: List withAll: {1. 2. 3}.
        transcript show: ([list collect: [:each | list add: 0]] on: Modified do: [:error | #collect]) printString; newLine.
        transcript show: ([list anySatisfy: [:each | list add: 0. false]] on: Modified do: [:error | #anySatisfy]) printString; newLine.
        transcript show: ([list removeAllSuchThat: [:each | list add: 0. false]] on: Modified do: [:error | #removeAllSuchThat]) printString; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
the collection was modified while it was being iterated
3
true
#signalled
2
1 2 2
ConcurrentModification
#collect
#anySatisfy
#removeAllSuchThat
'''
```

(Line 5: `do:` re-reads each element, so it stores 3, then 1, then 2 at
index 1. Line 6: `{2. 1. 2}` sorted.)

`a_list_has_the_whole_protocol.ns`:

```newspeak
(* A List adds at both ends and in bulk, removes by element, by predicate
   and from both ends, searches, copies and converts; `collect:`,
   `select:`, `reject:`, `,` and `copyFrom:to:` answer Lists, and the
   rest of Collection's protocol is inherited. A list given itself as
   argument is read before it changes. *)
class AListHasTheWholeProtocol = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. collections = platform collections. List = collections List. list show |
        show:: [:collection |
            collection do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
            transcript newLine].
        list:: List withAll: {3. 1. 2}.
        show value: list.
        list addFirst: 0; addLast: 9.
        show value: list.
        list addAll: {7. 7}.
        show value: list.
        transcript show: list first printString; show: ' '; show: list last printString; newLine.
        transcript show: (list indexOf: 2) printString; show: ' '; show: (list indexOf: 5) printString; newLine.
        transcript show: (list remove: 7) printString; newLine.
        show value: list.
        transcript show: ([list remove: 5] on: collections NotFound do: [:error | error element]) printString; newLine.
        transcript show: ([list remove: 5] on: kernel Error do: [:error | error messageText]); newLine.
        list removeAllSuchThat: [:each | each > 2].
        show value: list.
        show value: (list collect: [:each | each * 10]).
        transcript show: (list collect: [:each | each]) class printString; newLine.
        show value: (list select: [:each | each > 0]).
        show value: (list reject: [:each | each > 0]).
        show value: list , {5. 6}.
        show value: (list copyFrom: 2 to: 3).
        transcript show: (list copyFrom: 2 to: 3) class printString; newLine.
        show value: list asArray.
        transcript show: list asArray class printString; newLine.
        list keysAndValuesDo: [:index :each |
            transcript show: index printString; show: '='; show: each printString; show: ' '].
        transcript newLine.
        transcript show: (list inject: 0 into: [:sum :each | sum + each]) printString; newLine.
        transcript show: (list includes: 2) printString; newLine.
        transcript show: (list detect: [:each | each > 0]) printString; newLine.
        list addAll: list.
        show value: list.
        show value: (kernel Array withAll: list).
        show value: {9} , list.
        show value: list , list.
        transcript show: ([List new first] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: List new isEmpty printString; newLine.
        list removeAll.
        transcript show: list size printString; newLine.
        list:: List new: 0.
        list addFirst: 2; addFirst: 1; add: 3.
        show value: list.
    )
) : ()
```

`a_list_has_the_whole_protocol.toml` (the `keysAndValuesDo:` line ends in
a space):

```toml
stdout = '''
3 1 2
0 3 1 2 9
0 3 1 2 9 7 7
0 7
4 0
7
0 3 1 2 9 7
5
element not found: 5
0 1 2
0 10 20
List
1 2
0
0 1 2 5 6
1 2
List
0 1 2
Array
1=0 2=1 3=2 
3
true
1
0 1 2 0 1 2
0 1 2 0 1 2
9 0 1 2 0 1 2
0 1 2 0 1 2 0 1 2 0 1 2
1
true
0
1 2 3
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_list_`
Expected: FAIL -- `withAll:`, `ConcurrentModification` and the rest are
not understood.

- [ ] **Step 3: The module's slots, the facet, the error classes**

The module's slot list gains the kernel's `Collection`, for the same
reason as `kernelError`:

```newspeak
    | (* The kernel module, which the classes here reach `Array`'s errors
         and `ArgumentError` through. *)
      private kernelModule = theKernel.
      (* The kernel's `Error`, the superclass of `NotFound` and
         `ConcurrentModification`. *)
      (* An inheritance clause is one name, optionally after `outer`,
         `self` or `super`, and at most one message (§6.1), so a clause
         cannot reach the kernel's classes through `kernelModule` and
         names them through these slots. *)
      private kernelError = theKernel Error.
      (* The kernel's `Collection`, the superclass of `List`, `Map` and
         `Set`. *)
      private kernelCollection = theKernel Collection. |
```

`Collections` gains, each documented as its neighbours are:

```newspeak
        (* Answers the kernel's `Collection` class, the abstract superclass
           of `List`, `Map` and `Set`, for a program to subclass. *)
        public Collection = ( ^kernelCollection )
        (* Answers the `ConcurrentModification` class: the Error a `List`,
           `Map` or `Set` signals when it is changed while it is iterated.
           It carries the collection in `collection`. *)
        public ConcurrentModification = ( ^outer CollectionsFactory ConcurrentModification )
```

After `NotFound` (whose comments Task 1's rule now governs too -- bring
them to it):

```newspeak
    (* The Error a List, Map or Set signals when an iteration over it finds
       it changed -- an element added or removed -- by the block it ran.
       Carries the collection.

       Iteration means `do:`, `keysAndValuesDo:` and everything built on
       them; storing at an existing index or key, and sorting, are no
       change. *)
    (* Without it a change inside `do:` would visit an element twice, skip
       one or end early, silently. *)
    private class ConcurrentModification = kernelError new (
        (* The collection that was changed while it was iterated. *)
        | public collection ::= nil. |
    ) (
    ) : (
        (* Answers a new ConcurrentModification carrying `modified`, not
           yet signalled. *)
        public collection: modified = ( ^self new collection: modified )
    )
    (* Signals a ConcurrentModification carrying `collection`, its
       messageText 'the collection was modified while it was being
       iterated', and answers what a handler that resumes it answers. The
       one place the error is raised, so every collection's text is one. *)
    private signalModificationOf: collection = (
        ^(ConcurrentModification collection: collection)
            signal: 'the collection was modified while it was being iterated'
    )
```

- [ ] **Step 4: `List`, written anew**

Replace the whole `List` class -- its comments, slots, methods and class
side -- with:

```newspeak
    (* An ordered, growable sequence, indexed from 1, that adds and removes
       at both ends in amortized constant time: `List new`, `List new: 20`,
       `List withAll: {1. 2}`. A Collection: `collect:`, `select:` and
       `reject:` answer Lists.

       Changing its elements' number -- adding, removing -- while `do:`,
       `keysAndValuesDo:`, `anySatisfy:`, `removeAllSuchThat:` or anything
       built on `do:` runs over it signals a ConcurrentModification after
       the block that did it. Storing at an existing index does not.

       The primary factory takes the capacity, so a program may write
       `List new: 15` as a superclass clause. *)
    (* The elements lie in `data` from `firstIndex` on, so that removing at
       either end moves nothing; the representation is private, so that it
       can change without a subclass noticing, and `size_` alone is
       protected, as a subclass's view of the size. *)
    private class List new: capacity = kernelCollection new (
        (* The number of elements. *)
        | protected size_ ::= 0.
          (* The index in `data` of the first element; 1 whenever the list
             is empty. *)
          private firstIndex ::= 1.
          (* The elements, `size_` of them from `firstIndex` on; every
             other slot holds nil, so the list keeps nothing alive it no
             longer holds. *)
          private data ::= Array new: capacity.
          (* The number of changes to the number of elements so far; an
             iteration compares it after every block. *)
          private modifications ::= 0. |
    ) (
        (* Answers true: the receiver is a List. *)
        public isKindOfList = ( ^true )
        (* Answers the number of elements. *)
        public size = ( ^size_ )
        (* Answers whether the list holds no element. *)
        (* The size is at hand; `Collection`'s would send `size`. *)
        public isEmpty = ( ^0 = size_ )
        (* Answers the element at `index`.

           index: an Integer from 1 to `size`.

           Signals an ArgumentError carrying `index`, 'index … out of
           range: valid indices are 1..size (size …)', for any other
           index, an index that is no Integer included. *)
        (* The check is written out, since every `at:` pays for it; an
           index that is no Integer is out of range too. *)
        public at: index = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            ^data at: firstIndex + index - 1
        )
        (* Replaces the element at `index` with `value` and answers `value`.
           Not a change of the number of elements: an iteration running
           meanwhile carries on, and sees `value` if it has not passed
           `index` yet.

           index: an Integer from 1 to `size`.

           Signals the ArgumentError `at:` signals for any other index. *)
        public at: index put: value = (
            ((index class == Integer) and: [(1 <= index) and: [index <= size_]])
                ifFalse: [^self indexOutOfRange: index].
            data at: firstIndex + index - 1 put: value.
            ^value
        )
        (* Answers the first element.

           Signals the ArgumentError of `at:`, carrying 1, for an empty
           list. *)
        public first = ( ^self at: 1 )
        (* Answers the last element.

           Signals the ArgumentError of `at:`, carrying 0, for an empty
           list. *)
        public last = ( ^self at: size_ )
        (* Appends `element` at the end, in amortized constant time, and
           answers it. *)
        public add: element = (
            | end |
            end:: firstIndex + size_.
            end > data size ifTrue: [end:: makeRoomAtEnd].
            data at: end put: element.
            size_:: size_ + 1.
            modifications:: modifications + 1.
            ^element
        )
        (* Appends `element` at the end and answers it, as `add:` does. *)
        public addLast: element = ( ^self add: element )
        (* Inserts `element` before the first, in amortized constant time,
           and answers it. *)
        public addFirst: element = (
            firstIndex = 1 ifTrue: [makeRoomAtFront].
            firstIndex:: firstIndex - 1.
            data at: firstIndex put: element.
            size_:: size_ + 1.
            modifications:: modifications + 1.
            ^element
        )
        (* Appends each element of `collection`, in its `do:` order, and
           answers `collection`. The list itself as `collection` appends a
           copy of its elements: `list addAll: list` doubles it.

           collection: a Collection, or anything understanding `do:`. *)
        public addAll: collection = (
            (collection == self ifTrue: [self asArray] ifFalse: [collection])
                do: [:element | self add: element].
            ^collection
        )
        (* Evaluates `action` with each element in turn, from the first, and
           answers the receiver. Each element is read when the walk reaches
           it, so one stored meanwhile by `at:put:` is seen.

           action: a one-argument block, or anything understanding
               `value:`.

           Signals a ConcurrentModification after the evaluation of
           `action` that added or removed an element. *)
        public do: action = (
            | expected end index |
            expected:: modifications.
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                action value: (data at: firstIndex + index).
                expected = modifications ifFalse: [^signalModificationOf: self].
                index:: index + 1]
        )
        (* Evaluates `action` with each index and the element there, from
           the first, and answers the receiver.

           action: a two-argument block, given the index and the element.

           Signals a ConcurrentModification as `do:` does. *)
        public keysAndValuesDo: action = (
            | expected end index |
            expected:: modifications.
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                action value: index + 1 value: (data at: firstIndex + index).
                expected = modifications ifFalse: [^signalModificationOf: self].
                index:: index + 1]
        )
        (* Answers whether `predicate` answers true for some element, tried
           in turn from the first until one does; false for an empty list.

           predicate: a one-argument block answering a Boolean.

           Signals a ConcurrentModification as `do:` does. *)
        (* Written out rather than inherited: `Collection`'s returns from a
           block, and the benchmarks ask it of their work lists. *)
        public anySatisfy: predicate = (
            | expected end index |
            expected:: modifications.
            end:: size_.
            index:: 0.
            [index < end] whileTrue: [
                (predicate value: (data at: firstIndex + index)) ifTrue: [^true].
                expected = modifications ifFalse: [^signalModificationOf: self].
                index:: index + 1].
            ^false
        )
        (* Answers the index of the first element `=` to `element`, or 0
           when there is none. Each element is sent `=` with `element`,
           from the first, until one answers true. *)
        public indexOf: element = (
            | index |
            index:: 0.
            [index < size_] whileTrue: [
                (data at: firstIndex + index) = element ifTrue: [^index + 1].
                index:: index + 1].
            ^0
        )
        (* Removes the first element `=` to `element` and answers it.

           Signals a NotFound carrying `element`, 'element not found: …'
           and its `printString`, when there is none. *)
        public remove: element = (
            ^self remove: element ifAbsent: [
                (NotFound element: element) signal: 'element not found: ', element printString]
        )
        (* Removes the first element `=` to `element` and answers
           `element`; when there is none, answers the value of `onAbsent`
           and leaves the list unchanged.

           onAbsent: anything understanding `value`. *)
        public remove: element ifAbsent: onAbsent = (
            | position end |
            position:: firstIndex.
            end:: firstIndex + size_.
            [position < end] whileTrue: [
                (data at: position) = element ifTrue: [
                    removeAtDataIndex: position.
                    ^element].
                position:: position + 1].
            ^onAbsent value
        )
        (* Removes the first element and answers it, in constant time.

           Signals an Error, 'removeFirst: the List is empty', for an empty
           list. *)
        public removeFirst = (
            | element |
            0 = size_ ifTrue: [^(kernelModule Error new) signal: 'removeFirst: the List is empty'].
            element:: data at: firstIndex.
            data at: firstIndex put: nil.
            size_:: size_ - 1.
            0 = size_
                ifTrue: [firstIndex:: 1]
                ifFalse: [firstIndex:: firstIndex + 1].
            modifications:: modifications + 1.
            ^element
        )
        (* Removes the last element and answers it, in constant time.

           Signals an Error, 'removeLast: the List is empty', for an empty
           list. *)
        public removeLast = (
            | last element |
            0 = size_ ifTrue: [^(kernelModule Error new) signal: 'removeLast: the List is empty'].
            last:: firstIndex + size_ - 1.
            element:: data at: last.
            data at: last put: nil.
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1].
            modifications:: modifications + 1.
            ^element
        )
        (* Removes every element and answers the receiver; the capacity
           stays. *)
        (* A list emptied is usually filled again to about its old size:
           `benchmarks/are-we-fast-yet/Havlak.ns` empties and refills its
           lists for every loop search. *)
        public removeAll = (
            data:: Array new: data size.
            firstIndex:: 1.
            size_:: 0.
            modifications:: modifications + 1
        )
        (* Removes every element for which `predicate` answers true, keeping
           the order of the rest, and answers the receiver. `predicate` sees
           every element once, in order, before anything is removed.

           predicate: a one-argument block answering a Boolean.

           Signals a ConcurrentModification after the evaluation of
           `predicate` that added or removed an element. *)
        public removeAllSuchThat: predicate = (
            | elements expected kept count |
            elements:: self asArray.
            expected:: modifications.
            kept:: Array new: elements size.
            count:: 0.
            elements do: [:element |
                (predicate value: element) ifFalse: [
                    count:: count + 1.
                    kept at: count put: element].
                expected = modifications ifFalse: [^signalModificationOf: self]].
            count = size_ ifTrue: [^self].
            data:: Array new: data size.
            data replaceFrom: 1 to: count with: kept startingAt: 1.
            firstIndex:: 1.
            size_:: count.
            modifications:: modifications + 1
        )
        (* Sorts the elements in place, stably, and answers the receiver.
           Not a change of the number of elements: an iteration running
           meanwhile carries on.

           lessOrEqual: a two-argument block answering whether its first
               argument may precede its second. *)
        public sort: lessOrEqual = (
            | sorted |
            size_ < 2 ifTrue: [^self].
            sorted:: self asArray.
            sorted sort: lessOrEqual.
            data replaceFrom: firstIndex to: firstIndex + size_ - 1 with: sorted startingAt: 1
        )
        (* Answers a new Array of the elements, in order. *)
        public asArray = ( ^data copyFrom: firstIndex to: firstIndex + size_ - 1 )
        (* Answers a new List of the receiver's elements followed by those of
           `collection`. The receiver is unchanged.

           collection: a Collection, or anything answering `size` and
               `do:`. *)
        public , collection = (
            | result |
            result:: List new: size_ + collection size.
            result addAll: self; addAll: collection.
            ^result
        )
        (* Answers a new List of the elements from `start` to `stop`; empty
           when `stop` is `start - 1`. The receiver is unchanged.

           Signals the ArgumentError of `Array>>copyFrom:to:` for a range
           the list does not hold. *)
        public copyFrom: start to: stop = ( ^List withAll: (self asArray copyFrom: start to: stop) )
        (* Answers a new, empty List with room for `capacity` elements:
           what `collect:`, `select:` and `reject:` fill. *)
        (* `List`, not `self class`: a subclass's primary factory may take
           no capacity, as DeltaBlue's `Plan` takes none. *)
        protected newForCollectUsingAdd: capacity = ( ^List new: capacity )
        (* Signals the ArgumentError `Array>>at:` signals for `index`, with
           the list's size, so a program sees one text from either. *)
        private indexOutOfRange: index = (
            ^(kernelModule ArgumentError value: index) signal: 'index ', index printString,
                ' out of range: valid indices are 1..', size_ printString,
                ' (size ', size_ printString, ')'
        )
        (* Removes the element at `position` in `data`, closing the gap from
           the shorter side so that removing near either end moves little,
           and counts the change. *)
        private removeAtDataIndex: position = (
            | last |
            last:: firstIndex + size_ - 1.
            position - firstIndex < (last - position)
                ifTrue: [
                    data replaceFrom: firstIndex + 1 to: position with: data startingAt: firstIndex.
                    data at: firstIndex put: nil.
                    firstIndex:: firstIndex + 1]
                ifFalse: [
                    data replaceFrom: position to: last - 1 with: data startingAt: position + 1.
                    data at: last put: nil].
            size_:: size_ - 1.
            0 = size_ ifTrue: [firstIndex:: 1].
            modifications:: modifications + 1
        )
        (* Makes room after the last element and answers the index in
           `data` where the next one goes: slides the elements to the start
           when at least as many slots lie empty before them as they fill
           -- the `removeFirst`s that emptied those slots pay for the move --
           and otherwise copies them into an Array half as large again, plus
           four, so that an Array of size 0 grows too. *)
        private makeRoomAtEnd = (
            | grown |
            ((firstIndex > 1) and: [firstIndex - 1 >= size_]) ifTrue: [
                data replaceFrom: 1 to: size_ with: data startingAt: firstIndex.
                size_ + 1 to: firstIndex + size_ - 1 do: [:index | data at: index put: nil].
                firstIndex:: 1.
                ^size_ + 1].
            grown:: Array new: data size + (data size // 2) + 4.
            grown replaceFrom: 1 to: size_ with: data startingAt: firstIndex.
            data:: grown.
            firstIndex:: 1.
            ^size_ + 1
        )
        (* Makes room before the first element, which lies at index 1 of
           `data`: slides the elements right by half the empty slots after
           them, rounded up, or, when there are none, copies them into an
           Array half as large again, plus four, leaving that room in
           front. *)
        private makeRoomAtFront = (
            | free shift target |
            free:: data size - size_.
            free > 0
                ifTrue: [
                    shift:: free + 1 // 2.
                    target:: data]
                ifFalse: [
                    shift:: data size // 2 + 4.
                    target:: Array new: data size + shift].
            target replaceFrom: firstIndex + shift to: firstIndex + shift + size_ - 1 with: data startingAt: firstIndex.
            target == data ifTrue: [
                firstIndex to: firstIndex + shift - 1 do: [:index | data at: index put: nil]].
            data:: target.
            firstIndex:: firstIndex + shift
        )
    ) : (
        (* Answers an empty List with room for 3 elements, `List new: 3`. *)
        public new = ( ^self new: 3 )
        (* Answers a new List of `collection`'s elements, in its `do:`
           order.

           collection: a Collection, or anything answering `size` and
               `do:`. *)
        public withAll: collection = ( ^(self new: collection size) addAll: collection; yourself )
    )
```

Check `makeRoomAtFront`'s clearing: after sliding `size_` elements from 1
right by `shift`, positions 1 to `shift` are either vacated or were never
used; clearing them all is right because the elements now start at
`shift + 1`. Check `removeAtDataIndex:`: removing at `position` near the
front copies `firstIndex .. position - 1` one to the right (the leaf reads
before it stores, so the overlap is safe); near the back copies
`position + 1 .. last` one to the left.

Check `List new: 0` then `addFirst: 2`: `free` 0, `shift` 4, target size
4, the empty range copied, `firstIndex` 5, then 4 holds 2; `addFirst: 1`:
`firstIndex` 4 is not 1, so 3 holds 1; `add: 3`: end = 3 + 2 = 5 > 4,
`makeRoomAtEnd`: `firstIndex - 1` = 2 >= 2, slide to 1..2, clear 3..4,
answer 3 → `1 2 3`.

- [ ] **Step 5: Run the goldens and the benchmarks' goldens**

Run: `cargo nextest run -p ns-cli --test run_goldens collection awfy`
Expected: PASS -- the Are We Fast Yet goldens under `goldens/tool/awfy/`
run DeltaBlue's `Plan` (a `List` subclass), the work lists and the sorts
on the new `List`.

- [ ] **Step 6: Documentation**

`docs/design/core-library.md`, "The collections module" (the paragraph
about line 2536 and the drawing near line 540): `List` is a `Collection`
written anew, its protocol in one sentence, `addFirst:`'s room at the
front, `ConcurrentModification` and the modification count (what counts,
when it is checked); the sentence saying `do:` visits the elements the
list held when it started is removed.

- [ ] **Step 7: Gates and commit**

Commit: `feat: List under Collection with its whole protocol, and ConcurrentModification`.

---

### Task 6: `Map` rewritten under `Collection` with removal, and `IdentityMap`

**Files:**
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (`Map`
  written anew, `IdentityMap` new, `Collections>>IdentityMap`)
- Create: `goldens/library/collection/a_map_has_the_whole_protocol.{ns,toml}`
- Create: `goldens/library/collection/a_map_removes_and_keeps_its_order.{ns,toml}`
- Create: `goldens/library/collection/an_identity_map_compares_keys_by_identity.{ns,toml}`
- Create: `goldens/library/collection/a_map_changed_while_iterated_signals.{ns,toml}`

**Interfaces:**
- Consumes: `kernelCollection`, `signalModificationOf:`, `NotFound`
  (Task 5).
- Produces: `Map` with the protocol below; its protected `hashOf: key`,
  `is: key sameAs: stored` and `newForCollectUsingAdd: capacity`;
  `IdentityMap` (primary factory `new: capacity`, class-side `new`
  inherited); `platform collections IdentityMap`.

- [ ] **Step 1: Write the failing goldens**

`a_map_has_the_whole_protocol.ns`:

```newspeak
(* A Map's elements are its values: `do:`, `includes:` and `inject:into:`
   visit them, and `collect:`, `select:` and `reject:` answer Maps of the
   same keys. Keys keep the order they were first stored in; a removed key
   leaves the order, and storing an existing key keeps its place. A map
   given itself to `addAll:` stays as it is. *)
class AMapHasTheWholeProtocol = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. collections = platform collections. Map = collections Map. map other show showMap |
        show:: [:collection |
            collection do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
            transcript newLine].
        showMap:: [:aMap |
            aMap keysAndValuesDo: [:key :value |
                transcript show: key printString; show: '->'; show: value printString; show: ' '].
            transcript newLine].
        map:: Map new.
        map at: #a put: 1; at: #b put: 2; at: #c put: 3.
        showMap value: map.
        transcript show: map size printString; show: ' '; show: map isEmpty printString; newLine.
        transcript show: (map at: #z ifAbsent: [0]) printString; newLine.
        transcript show: (map at: #d ifAbsentPut: [4]) printString; show: ' '; show: (map at: #d ifAbsentPut: [5]) printString; newLine.
        transcript show: (map includesKey: #d) printString; show: ' '; show: (map includesKey: #e) printString; newLine.
        transcript show: (map removeKey: #b) printString; newLine.
        transcript show: (map removeKey: #b ifAbsent: [#gone]) printString; newLine.
        transcript show: ([map removeKey: #b] on: collections NotFound do: [:error | error element]) printString; newLine.
        transcript show: ([map removeKey: #b] on: kernel Error do: [:error | error messageText]); newLine.
        showMap value: map.
        map do: [:value | transcript show: value printString; show: ' '].
        transcript newLine.
        transcript show: (map includes: 3) printString; show: ' '; show: (map includes: #c) printString; newLine.
        transcript show: (map inject: 0 into: [:sum :value | sum + value]) printString; newLine.
        showMap value: (map collect: [:value | value * 10]).
        showMap value: (map select: [:value | value > 1]).
        showMap value: (map reject: [:value | value > 1]).
        transcript show: (map select: [:value | true]) class printString; newLine.
        show value: map keys.
        show value: map values.
        other:: Map withAll: map.
        other at: #e put: 5.
        transcript show: map size printString; show: ' '; show: other size printString; newLine.
        map addAll: other.
        showMap value: map.
        map addAll: map.
        transcript show: map size printString; newLine.
        map at: #a put: 100.
        showMap value: map.
        map removeAll.
        transcript show: map size printString; show: ' '; show: map isEmpty printString; newLine.
    )
) : ()
```

`a_map_has_the_whole_protocol.toml` (each `showMap` line and the `do:`
line end in a space):

```toml
stdout = '''
#a->1 #b->2 #c->3 
3 false
0
4 4
true false
2
#gone
#b
key not found: #b
#a->1 #c->3 #d->4 
1 3 4 
true false
8
#a->10 #c->30 #d->40 
#c->3 #d->4 
#a->1 
Map
#a #c #d
1 3 4
3 4
#a->1 #c->3 #d->4 #e->5 
4
#a->100 #c->3 #d->4 #e->5 
0 true
'''
```

`a_map_removes_and_keeps_its_order.ns`:

```newspeak
(* Removing half a map's keys, storing a removed key again and growing past
   several tables keeps every live key reachable and the order of first
   storing: a key stored again after its removal comes last. *)
class AMapRemovesAndKeepsItsOrder = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Map = platform collections Map. map show |
        show:: [:collection |
            collection do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
            transcript newLine].
        map:: Map new.
        1 to: 20 do: [:index | map at: index put: index * index].
        1 to: 20 by: 2 do: [:index | map removeKey: index].
        map at: 1 put: #one.
        21 to: 30 do: [:index | map at: index put: index].
        transcript show: map size printString; newLine.
        show value: map keys.
        transcript show: (map at: 20) printString; show: ' '; show: (map at: 1) printString; show: ' ';
            show: (map includesKey: 3) printString; newLine.
        transcript show: (map keys allSatisfy: [:key | (map at: key ifAbsent: [nil]) notNil]) printString; newLine.
        map keys do: [:key | map removeKey: key].
        transcript show: map size printString; show: ' '; show: map isEmpty printString; newLine.
        map at: #x put: 1.
        show value: map keys.
    )
) : ()
```

`a_map_removes_and_keeps_its_order.toml`:

```toml
stdout = '''
21
2 4 6 8 10 12 14 16 18 20 1 21 22 23 24 25 26 27 28 29 30
400 #one false
true
0 true
#x
'''
```

`an_identity_map_compares_keys_by_identity.ns`:

```newspeak
(* An IdentityMap compares keys with `==` and hashes them with
   `identityHash`: two equal keys that are distinct objects are two keys,
   where a Map holds one. A value as key is found by an equal value, since
   a value's `==` is `=`. It answers Map's protocol and its own
   `at:putReplace:`, `at:ifAbsentPutVal:` and `atOrItself:`. *)
class AnIdentityMapComparesKeysByIdentity = Object new () (
    class Key named: theName = Object new (
        | public name = theName. |
    ) (
        public = other = ( ^name = other name )
        public hash = ( ^name hash )
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. first second equal identity |
        first:: Key named: #k.
        second:: Key named: #k.
        equal:: collections Map new.
        equal at: first put: 1.
        equal at: second put: 2.
        transcript show: equal size printString; show: ' '; show: (equal at: first) printString; newLine.
        identity:: collections IdentityMap new.
        identity at: first put: 1.
        identity at: second put: 2.
        transcript show: identity size printString; show: ' '; show: (identity at: first) printString; show: ' ';
            show: (identity at: second) printString; newLine.
        transcript show: (identity at: 'abc' put: 3; at: 'ab' , 'c') printString; newLine.
        transcript show: (identity at: first putReplace: 10) printString; show: ' '; show: (identity at: first) printString; newLine.
        transcript show: ([identity at: #missing putReplace: 1] on: collections NotFound do: [:error | error element]) printString; newLine.
        transcript show: (identity at: first ifAbsentPutVal: 99) printString; show: ' ';
            show: (identity at: (Key named: #new) ifAbsentPutVal: 7) printString; newLine.
        transcript show: (identity atOrItself: #absent) printString; show: ' '; show: (identity atOrItself: first) printString; newLine.
        transcript show: (identity collect: [:value | value]) class printString; newLine.
        transcript show: identity isKindOfIdentityMap printString; show: ' '; show: identity isKindOfMap printString; show: ' ';
            show: equal isKindOfIdentityMap printString; newLine.
        transcript show: (identity removeKey: second) printString; show: ' '; show: identity size printString; newLine.
    )
) : ()
```

`an_identity_map_compares_keys_by_identity.toml`:

```toml
stdout = '''
1 2
2 1 2
3
10 10
#missing
false true
#absent 10
IdentityMap
true true false
2 3
'''
```

`a_map_changed_while_iterated_signals.ns`:

```newspeak
(* A Map changed while it is iterated -- a key added or removed inside the
   block -- signals a ConcurrentModification; storing a new value under an
   existing key does not. *)
class AMapChangedWhileIteratedSignals = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. Modified = collections ConcurrentModification. map |
        map:: collections Map new.
        map at: 1 put: 1; at: 2 put: 2.
        transcript show: ([map do: [:value | map at: 3 put: 3]] on: Modified do: [:error | #added]) printString; newLine.
        transcript show: ([map keysAndValuesDo: [:key :value | map removeKey: key]] on: Modified do: [:error | #removed]) printString; newLine.
        map:: collections Map new.
        map at: 1 put: 1; at: 2 put: 2.
        map keysAndValuesDo: [:key :value | map at: key put: value * 10].
        map do: [:value | transcript show: value printString; show: ' '].
        transcript newLine.
        transcript show: ([map collect: [:value | map removeAll]] on: Modified do: [:error | #removeAll]) printString; newLine.
    )
) : ()
```

`a_map_changed_while_iterated_signals.toml`:

```toml
stdout = '''
#added
#removed
10 20 
#removeAll
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_map_ an_identity_map`
Expected: FAIL.

- [ ] **Step 3: `Map`, written anew**

Replace the whole `Map` class with:

```newspeak
    (* A table from keys to values: `Map new`, `Map new: 20`,
       `Map withAll: aMap`. A key is found by its `hash` and compared with
       `=`; every key's `hash` must agree with its `=`. Keys keep the order
       they were first stored in: storing an existing key keeps its place,
       a removed key leaves the order, and one stored again after its
       removal comes last.

       A Collection whose elements are the values: `do:`, `includes:` and
       the rest of Collection's protocol visit them, and `collect:`,
       `select:` and `reject:` answer Maps of the same keys.

       Adding or removing a key while `do:`, `keysAndValuesDo:` or anything
       built on them runs over the map signals a ConcurrentModification
       after the block that did it; storing a value under an existing key
       does not. *)
    (* The keys and values lie side by side in `data_`, in the order they
       were stored, and `hashtable_` is an open-addressing table of their
       indices in `data_`, 0 marking an empty bucket. A removal overwrites
       the key with `removed`, which no lookup can match, so its bucket
       stays in the probe chain as a tombstone; growing copies only the
       live entries, which compacts both. *)
    private class Map new: capacity = kernelCollection new (
        (* The number of keys the map holds. *)
        | private size_ ::= 0.
          (* The number of entries `data_` has been given since the table
             was last built, removed ones included: the next entry goes at
             `used_ * 2 + 1`. *)
          private used_ ::= 0.
          (* The keys and values, side by side in storing order; a removed
             entry's key is `removed` and its value nil. *)
          private data_ ::= nil.
          (* The number of keys added or removed so far; an iteration
             compares it after every block. *)
          private modifications ::= 0.
          (* The removed mark: an object no program can reach, so no key is
             ever `==` to it. *)
          private removed = Array new: 0. |
        createTable: capacity
    ) (
        (* Answers true: the receiver is a Map. *)
        public isKindOfMap = ( ^true )
        (* The table of the indices in `data_`, rebuilt from `data_` by
           `rehash` whenever it is nil. *)
        private lazy hashtable_ ::= rehash.
        (* Answers the number of keys. *)
        public size = ( ^size_ )
        (* Answers whether the map holds no key. *)
        public isEmpty = ( ^0 = size_ )
        (* Answers the value stored under `key`.

           Signals a NotFound carrying `key`, 'key not found: …' and the
           key's `printString`, when the map holds no such key. *)
        public at: key = (
            | keyIndex |
            keyIndex:: scanFor: key in: hashtable_.
            keyIndex < 0 ifTrue: [
                ^(NotFound element: key) signal: 'key not found: ', key printString].
            ^data_ at: keyIndex + 1
        )
        (* Answers the value stored under `key`, or the value of `onAbsent`
           when the map holds no such key.

           onAbsent: anything understanding `value`; sent it only when the
               key is absent. *)
        public at: key ifAbsent: onAbsent = (
            | keyIndex |
            keyIndex:: scanFor: key in: hashtable_.
            keyIndex < 0 ifTrue: [^onAbsent value].
            ^data_ at: keyIndex + 1
        )
        (* Answers the value stored under `key`; when the map holds no such
           key, stores the value of `generator` under it and answers that.

           generator: anything understanding `value`; sent it only when the
               key is absent. *)
        public at: key ifAbsentPut: generator = (
            ^self at: key ifAbsent: [self at: key put: generator value]
        )
        (* Stores `value` under `key` and answers `value`. A new key comes
           last in the order and counts as a change for an iteration
           running meanwhile; an existing key keeps its place and does not. *)
        public at: key put: value = (
            | table keyIndex |
            table:: hashtable_.
            keyIndex:: scanFor: key in: table.
            keyIndex < 0 ifTrue: [^insert: key value: value at: 0 - keyIndex in: table].
            data_ at: keyIndex + 1 put: value.
            ^value
        )
        (* Answers whether the map holds `key`. *)
        public includesKey: key = ( ^(scanFor: key in: hashtable_) > 0 )
        (* Removes `key` and answers the value it had.

           Signals a NotFound carrying `key`, 'key not found: …' and the
           key's `printString`, when the map holds no such key. *)
        public removeKey: key = (
            ^self removeKey: key ifAbsent: [
                (NotFound element: key) signal: 'key not found: ', key printString]
        )
        (* Removes `key` and answers the value it had; when the map holds no
           such key, answers the value of `onAbsent` and leaves the map
           unchanged.

           onAbsent: anything understanding `value`. *)
        public removeKey: key ifAbsent: onAbsent = (
            | keyIndex value |
            keyIndex:: scanFor: key in: hashtable_.
            keyIndex < 0 ifTrue: [^onAbsent value].
            value:: data_ at: keyIndex + 1.
            data_ at: keyIndex put: removed.
            data_ at: keyIndex + 1 put: nil.
            size_:: size_ - 1.
            modifications:: modifications + 1.
            ^value
        )
        (* Removes every key and answers the receiver; the capacity stays. *)
        (* `benchmarks/are-we-fast-yet/Havlak.ns` empties its map for every
           loop search and fills it again with as many nodes. *)
        public removeAll = (
            createTable: data_ size // 2 - 1.
            modifications:: modifications + 1
        )
        (* Stores every key and value of `aMap` in the receiver, in
           `aMap`'s order, and answers `aMap`. The receiver itself as
           `aMap` changes nothing.

           aMap: a Map, or anything understanding `keysAndValuesDo:`. *)
        public addAll: aMap = (
            aMap keysAndValuesDo: [:key :value | self at: key put: value].
            ^aMap
        )
        (* Evaluates `action` with each value, in the order of the keys, and
           answers the receiver.

           action: a one-argument block, or anything understanding
               `value:`.

           Signals a ConcurrentModification after the evaluation of
           `action` that added or removed a key. *)
        public do: action = (
            | data end expected keyIndex |
            data:: data_.
            end:: used_ * 2.
            expected:: modifications.
            keyIndex:: 1.
            [keyIndex < end] whileTrue: [
                (data at: keyIndex) == removed ifFalse: [
                    action value: (data at: keyIndex + 1).
                    expected = modifications ifFalse: [^signalModificationOf: self]].
                keyIndex:: keyIndex + 2]
        )
        (* Evaluates `action` with each key and its value, in the order of
           the keys, and answers the receiver.

           action: a two-argument block, given the key and the value.

           Signals a ConcurrentModification as `do:` does. *)
        public keysAndValuesDo: action = (
            | data end expected keyIndex |
            data:: data_.
            end:: used_ * 2.
            expected:: modifications.
            keyIndex:: 1.
            [keyIndex < end] whileTrue: [
                (data at: keyIndex) == removed ifFalse: [
                    action value: (data at: keyIndex) value: (data at: keyIndex + 1).
                    expected = modifications ifFalse: [^signalModificationOf: self]].
                keyIndex:: keyIndex + 2]
        )
        (* Answers a new Array of the keys, in their order. *)
        public keys = ( ^self entriesAt: 0 )
        (* Answers a new Array of the values, in the order of the keys. *)
        public values = ( ^self entriesAt: 1 )
        (* Answers a new map, from `newForCollectUsingAdd:` -- a Map, an
           IdentityMap for an IdentityMap -- holding each key with
           `transform`'s answer for its value, in the same order.

           transform: a one-argument block, given a value.

           Signals a ConcurrentModification as `keysAndValuesDo:` does. *)
        public collect: transform = (
            | result |
            result:: self newForCollectUsingAdd: size_.
            self keysAndValuesDo: [:key :value | result at: key put: (transform value: value)].
            ^result
        )
        (* Answers a new map, from `newForCollectUsingAdd:`, holding the
           keys whose values `predicate` answers true for, with their
           values, in the same order.

           predicate: a one-argument block answering a Boolean, given a
               value.

           Signals a ConcurrentModification as `keysAndValuesDo:` does. *)
        public select: predicate = (
            | result |
            result:: self newForCollectUsingAdd: size_.
            self keysAndValuesDo: [:key :value |
                (predicate value: value) ifTrue: [result at: key put: value]].
            ^result
        )
        (* Answers a new map, from `newForCollectUsingAdd:`, holding the
           keys whose values `predicate` answers false for, with their
           values, in the same order.

           predicate: a one-argument block answering a Boolean, given a
               value.

           Signals a ConcurrentModification as `keysAndValuesDo:` does. *)
        public reject: predicate = ( ^self select: [:value | (predicate value: value) not] )
        (* Answers a new, empty Map with room for `capacity` keys: what
           `collect:`, `select:` and `reject:` fill. *)
        protected newForCollectUsingAdd: capacity = ( ^Map new: capacity )
        (* Answers the hash a key is found by: its `hash`. A subclass that
           overrides it overrides `is:sameAs:` to agree. *)
        protected hashOf: key = ( ^key hash )
        (* Answers whether `key` is the key `stored`: `key = stored`. *)
        protected is: key sameAs: stored = ( ^key = stored )
        (* Answers a new Array of the live entries' keys (`offset` 0) or
           values (`offset` 1), in storing order. *)
        private entriesAt: offset = (
            | result data end count keyIndex |
            result:: Array new: size_.
            data:: data_.
            end:: used_ * 2.
            count:: 0.
            keyIndex:: 1.
            [keyIndex < end] whileTrue: [
                (data at: keyIndex) == removed ifFalse: [
                    count:: count + 1.
                    result at: count put: (data at: keyIndex + offset)].
                keyIndex:: keyIndex + 2].
            ^result
        )
        (* Empties the map with room for `count + 1` entries -- `data_` of
           twice that, the table a third larger so it stays at most three
           quarters full -- and answers the new table.

           count: an Integer of at least 0.

           Signals an ArgumentError carrying `count`, 'new: requires a
           capacity of at least 0, got …', for a negative `count`, and what
           `Array new:` signals for a size that is no Integer or too large. *)
        private createTable: count = (
            | table |
            count < 0 ifTrue: [
                ^(kernelModule ArgumentError value: count)
                    signal: 'new: requires a capacity of at least 0, got ', count printString].
            size_:: 0.
            used_:: 0.
            data_:: Array new: count + 1 * 2.
            table:: Array new: count + 1 * 4 // 3.
            1 to: table size do: [:index | table at: index put: 0].
            hashtable_:: table.
            ^table
        )
        (* Stores a new entry at the end of `data_`, its index in the empty
           bucket `bucketIndex` of `table`, counts it as a change, grows the
           map when `data_` is full, and answers `value`. *)
        private insert: key value: value at: bucketIndex in: table = (
            | keyIndex |
            keyIndex:: used_ * 2 + 1.
            data_ at: keyIndex put: key.
            data_ at: keyIndex + 1 put: value.
            table at: bucketIndex put: keyIndex.
            used_:: used_ + 1.
            size_:: size_ + 1.
            modifications:: modifications + 1.
            keyIndex + 2 > data_ size ifTrue: [rehash].
            ^value
        )
        (* Builds a new table, with room for twice the live entries, from
           the live entries of `data_` in their order, dropping the removed
           ones, and answers it. Runs when `data_` is full, and when the
           table slot is nil. *)
        private rehash = (
            | oldData oldEnd table keyIndex key newIndex |
            oldData:: data_.
            oldEnd:: used_ * 2.
            table:: createTable: size_ * 2.
            keyIndex:: 1.
            [keyIndex < oldEnd] whileTrue: [
                key:: oldData at: keyIndex.
                key == removed ifFalse: [
                    newIndex:: used_ * 2 + 1.
                    data_ at: newIndex put: key.
                    data_ at: newIndex + 1 put: (oldData at: keyIndex + 1).
                    table at: (emptyBucketFor: key in: table) put: newIndex.
                    used_:: used_ + 1.
                    size_:: size_ + 1].
                keyIndex:: keyIndex + 2].
            ^table
        )
        (* Answers the index in `data_` of `key`, or the negated index of the
           empty bucket of `table` where it belongs. Probes from the key's
           `hashOf:` bucket onwards, passing removed entries. *)
        private scanFor: key in: table = (
            | data tableSize bucketIndex keyIndex stored |
            data:: data_.
            tableSize:: table size.
            bucketIndex:: (self hashOf: key) \\ tableSize + 1.
            [true] whileTrue: [
                keyIndex:: table at: bucketIndex.
                0 = keyIndex ifTrue: [^0 - bucketIndex].
                stored:: data at: keyIndex.
                (stored == removed) ifFalse: [
                    (self is: key sameAs: stored) ifTrue: [^keyIndex]].
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]]
        )
        (* Answers the index of the first empty bucket of `table` from
           `key`'s `hashOf:` bucket onwards: where a key not yet in it
           goes. *)
        private emptyBucketFor: key in: table = (
            | tableSize bucketIndex |
            tableSize:: table size.
            bucketIndex:: (self hashOf: key) \\ tableSize + 1.
            [0 = (table at: bucketIndex)] whileFalse: [
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]].
            ^bucketIndex
        )
    ) : (
        (* Answers an empty map with room for 3 keys, `new: 3`. *)
        public new = ( ^self new: 3 )
        (* Answers a new map of `aMap`'s keys and values, in its order.

           aMap: a Map, or anything answering `size` and understanding
               `keysAndValuesDo:`. *)
        public withAll: aMap = ( ^(self new: aMap size) addAll: aMap; yourself )
    )
    (* A Map that compares keys with `==` and finds them by their
       `identityHash`: two distinct objects are two keys however `=` they
       are, and no key's `=` or `hash` is ever sent. A value as key behaves
       as in a Map, since a value's `==` is its `=` and its `identityHash`
       its `hash`. Everything else is Map's: the order, removal,
       ConcurrentModification. *)
    private class IdentityMap new: capacity = Map new: capacity (
    ) (
        (* Answers true: the receiver is an IdentityMap. *)
        public isKindOfIdentityMap = ( ^true )
        (* Answers the hash a key is found by: its `identityHash`. *)
        protected hashOf: key = ( ^key identityHash )
        (* Answers whether `key` is the key `stored`: `key == stored`. *)
        protected is: key sameAs: stored = ( ^key == stored )
        (* Answers a new, empty IdentityMap with room for `capacity` keys:
           what `collect:`, `select:` and `reject:` fill. *)
        protected newForCollectUsingAdd: capacity = ( ^IdentityMap new: capacity )
        (* Stores `value` under `key`, which the map must hold already, and
           answers `value`.

           Signals a NotFound carrying `key`, 'key not found: …' and the
           key's `printString`, when the map holds no such key, and stores
           nothing. *)
        public at: key putReplace: value = (
            (self includesKey: key) ifFalse: [
                ^(NotFound element: key) signal: 'key not found: ', key printString].
            ^self at: key put: value
        )
        (* Stores `value` under `key` when the map holds no such key, and
           answers true; answers false, storing nothing, when it does. *)
        public at: key ifAbsentPutVal: value = (
            (self includesKey: key) ifTrue: [^false].
            self at: key put: value.
            ^true
        )
        (* Answers the value stored under `key`, or `key` itself when the
           map holds no such key. *)
        public atOrItself: key = ( ^self at: key ifAbsent: [key] )
    )
```

`Collections` gains:

```newspeak
        (* Answers the `IdentityMap` class: a Map comparing keys with `==`.
           `IdentityMap new`, or `IdentityMap new: capacity`, which
           signals what `Map new:` signals. *)
        public IdentityMap = ( ^outer CollectionsFactory IdentityMap )
```

and the `Map` accessor's comment is brought to the documentation rule
(its capacity errors stay as they are documented today).

Check the order golden by hand against `rehash`: removals leave marks;
`at: 1 put: #one` probes past 1's tombstone to an empty bucket and appends;
every growth copies the live entries in `data_` order; so the keys answer
2, 4, …, 20, 1, 21, …, 30.

- [ ] **Step 4: Run the goldens and the benchmarks' goldens**

Run: `cargo nextest run -p ns-cli --test run_goldens collection awfy`
Expected: PASS (DeltaBlue and Havlak run on the new `Map`).

- [ ] **Step 5: Documentation**

`docs/design/core-library.md`, the collections module: `Map` written anew
under `Collection`, its values as elements, removal through the removed
mark and the tombstone it leaves, compaction on growth; `IdentityMap` as a
subclass of `Map` and why it is not a separate kernel class (the decision
from the spec); the sentence "no removal of single elements and no deleted
marks" goes.

- [ ] **Step 6: Gates and commit**

Commit: `feat: Map under Collection with removal, and IdentityMap`.

---

### Task 7: `Set` rewritten under `Collection`, and the module without adapted code

**Files:**
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (`Set`
  written anew; the module comment without the licence header; every
  remaining member brought to the documentation rule)
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs`
  (`DOCUMENTED_MODULES` gains `"CollectionsFactory.ns"`)
- Modify: `THIRD-PARTY-NOTICES.md` (the entry for `CollectionsFactory.ns`
  goes)
- Create: `goldens/library/collection/a_set_has_the_whole_protocol.{ns,toml}`
- Create: `goldens/library/collection/a_set_changed_while_iterated_signals.{ns,toml}`

**Interfaces:**
- Consumes: `kernelCollection`, `signalModificationOf:`, `NotFound`
  (Task 5); `Map`'s representation as the pattern (Task 6).
- Produces: `Set` with the protocol below; a `CollectionsFactory.ns`
  holding no adapted code and no licence header.

- [ ] **Step 1: Write the failing goldens**

`a_set_has_the_whole_protocol.ns`:

```newspeak
(* A Set holds each element once, `=` and `hash` deciding, in the order
   elements were first added; adding an equal element replaces it in
   place. It removes by element, in bulk and by predicate, and `collect:`,
   `select:` and `reject:` answer Sets, so equal results merge. *)
class ASetHasTheWholeProtocol = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. kernel = platform kernel. collections = platform collections. Set = collections Set. set show |
        show:: [:collection |
            collection do: [:each | transcript show: each printString] separatedBy: [transcript show: ' '].
            transcript newLine].
        set:: Set withAll: {3. 1. 3. 2}.
        show value: set.
        transcript show: set size printString; newLine.
        transcript show: (set includes: 2) printString; show: ' '; show: (set includes: 5) printString; newLine.
        transcript show: (set remove: 1) printString; newLine.
        show value: set.
        transcript show: ([set remove: 1] on: collections NotFound do: [:error | error element]) printString; newLine.
        transcript show: ([set remove: 1] on: kernel Error do: [:error | error messageText]); newLine.
        transcript show: (set remove: 1 ifAbsent: [#absent]) printString; newLine.
        set add: 1.
        show value: set.
        set addAll: {4. 2}.
        show value: set.
        show value: (set collect: [:each | each \\ 2]).
        transcript show: (set collect: [:each | each]) class printString; newLine.
        show value: (set select: [:each | each > 2]).
        show value: (set reject: [:each | each > 2]).
        show value: set asArray.
        transcript show: set asArray class printString; newLine.
        set removeAll: {2. 4}.
        show value: set.
        transcript show: ([set removeAll: {9}] on: collections NotFound do: [:error | error element]) printString; newLine.
        set removeAllSuchThat: [:each | each > 2].
        show value: set.
        set addAll: set.
        transcript show: set size printString; newLine.
        set removeAll.
        transcript show: set size printString; show: ' '; show: set isEmpty printString; newLine.
        set:: Set new.
        1 to: 30 do: [:index | set add: index].
        1 to: 30 by: 3 do: [:index | set remove: index].
        transcript show: set size printString; show: ' '; show: (set includes: 4) printString; show: ' ';
            show: (set includes: 5) printString; newLine.
        transcript show: (set inject: 0 into: [:sum :each | sum + each]) printString; newLine.
    )
) : ()
```

`a_set_has_the_whole_protocol.toml`:

```toml
stdout = '''
3 1 2
3
true false
1
3 2
1
element not found: 1
#absent
3 2 1
3 2 1 4
1 0
Set
3 4
2 1
3 2 1 4
Array
3 1
9
1
1
0 true
20 false true
320
'''
```

(The last line: 1 + … + 30 is 465; the removed 1, 4, …, 28 sum to 145.)

`a_set_changed_while_iterated_signals.ns`:

```newspeak
(* A Set changed while it is iterated -- an element added or removed
   inside the block -- signals a ConcurrentModification; adding an element
   equal to one it holds replaces it and does not. *)
class ASetChangedWhileIteratedSignals = Object new () (
    public main: platform args: arguments = (
        | transcript = platform transcript. collections = platform collections. Modified = collections ConcurrentModification. set |
        set:: collections Set withAll: {1. 2}.
        transcript show: ([set do: [:each | set add: each + 10]] on: Modified do: [:error | #added]) printString; newLine.
        transcript show: ([set do: [:each | set remove: each]] on: Modified do: [:error | #removed]) printString; newLine.
        set:: collections Set withAll: {1. 2}.
        set do: [:each | set add: each].
        set do: [:each | transcript show: each printString; show: ' '].
        transcript newLine.
    )
) : ()
```

`a_set_changed_while_iterated_signals.toml`:

```toml
stdout = '''
#added
#removed
1 2 
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_set_`
Expected: FAIL.

- [ ] **Step 3: `Set`, written anew**

Replace the whole `Set` class with:

```newspeak
    (* A collection of distinct elements: `Set new`, `Set new: 20`,
       `Set withAll: {1. 2}`. An element is found by its `hash` and
       compared with `=`; every element's `hash` must agree with its `=`.
       Elements keep the order they were first added in; adding an element
       `=` to one the set holds replaces it in place.

       A Collection: `collect:`, `select:` and `reject:` answer Sets, so
       equal results of `collect:` merge.

       Adding or removing an element while `do:` or anything built on it
       runs over the set signals a ConcurrentModification after the block
       that did it; replacing an equal element does not. *)
    (* Map's representation with elements instead of key-value pairs: the
       elements in `data_` in adding order, `hashtable_` of their indices,
       the removed mark overwriting a removed element. *)
    private class Set new: capacity = kernelCollection new (
        (* The number of elements the set holds. *)
        | private size_ ::= 0.
          (* The number of entries `data_` has been given since the table
             was last built, removed ones included: the next goes at
             `used_ + 1`. *)
          private used_ ::= 0.
          (* The elements in adding order; a removed one is `removed`. *)
          private data_ ::= nil.
          (* The number of elements added or removed so far; an iteration
             compares it after every block. *)
          private modifications ::= 0.
          (* The removed mark: an object no program can reach, so no
             element is ever `==` to it. *)
          private removed = Array new: 0. |
        createTable: capacity
    ) (
        (* Answers true: the receiver is a Set. *)
        public isKindOfSet = ( ^true )
        (* The table of the indices in `data_`, rebuilt from `data_` by
           `rehash` whenever it is nil. *)
        private lazy hashtable_ ::= rehash.
        (* Answers the number of elements. *)
        public size = ( ^size_ )
        (* Answers whether the set holds no element. *)
        public isEmpty = ( ^0 = size_ )
        (* Adds `element` last in the order, or replaces the element `=` to
           it that the set holds, which keeps its place; answers `element`.
           Only adding counts as a change for an iteration running
           meanwhile. *)
        public add: element = (
            | table index |
            table:: hashtable_.
            index:: scanFor: element in: table.
            index < 0 ifTrue: [^insert: element at: 0 - index in: table].
            data_ at: index put: element.
            ^element
        )
        (* Adds each element of `collection`, in its `do:` order, and
           answers `collection`. The set itself as `collection` changes
           nothing.

           collection: a Collection, or anything understanding `do:`. *)
        public addAll: collection = (
            (collection == self ifTrue: [self asArray] ifFalse: [collection])
                do: [:element | self add: element].
            ^collection
        )
        (* Answers whether the set holds an element `=` to `element`, found
           by its `hash`. *)
        public includes: element = ( ^(scanFor: element in: hashtable_) > 0 )
        (* Removes the element `=` to `element` and answers `element`.

           Signals a NotFound carrying `element`, 'element not found: …'
           and its `printString`, when the set holds none. *)
        public remove: element = (
            ^self remove: element ifAbsent: [
                (NotFound element: element) signal: 'element not found: ', element printString]
        )
        (* Removes the element `=` to `element` and answers `element`; when
           the set holds none, answers the value of `onAbsent` and leaves
           the set unchanged.

           onAbsent: anything understanding `value`. *)
        public remove: element ifAbsent: onAbsent = (
            | index |
            index:: scanFor: element in: hashtable_.
            index < 0 ifTrue: [^onAbsent value].
            data_ at: index put: removed.
            size_:: size_ - 1.
            modifications:: modifications + 1.
            ^element
        )
        (* Removes each element of `collection`, in its `do:` order, and
           answers `collection`. The set itself as `collection` empties it.

           Signals a NotFound carrying the first element of `collection`
           the set does not hold, as `remove:` does; the elements before it
           are removed. *)
        public removeAll: collection = (
            (collection == self ifTrue: [self asArray] ifFalse: [collection])
                do: [:element | self remove: element].
            ^collection
        )
        (* Removes every element for which `predicate` answers true and
           answers the receiver. `predicate` sees every element once, in
           order, before anything is removed.

           predicate: a one-argument block answering a Boolean.

           Signals a ConcurrentModification after the evaluation of
           `predicate` that added or removed an element. *)
        public removeAllSuchThat: predicate = (
            | elements expected doomed count |
            elements:: self asArray.
            expected:: modifications.
            doomed:: Array new: elements size.
            count:: 0.
            elements do: [:element |
                (predicate value: element) ifTrue: [
                    count:: count + 1.
                    doomed at: count put: element].
                expected = modifications ifFalse: [^signalModificationOf: self]].
            1 to: count do: [:index | self remove: (doomed at: index)]
        )
        (* Removes every element and answers the receiver; the capacity
           stays. *)
        public removeAll = (
            createTable: data_ size - 1.
            modifications:: modifications + 1
        )
        (* Evaluates `action` with each element, in adding order, and
           answers the receiver.

           action: a one-argument block, or anything understanding
               `value:`.

           Signals a ConcurrentModification after the evaluation of
           `action` that added or removed an element. *)
        public do: action = (
            | data end expected index element |
            data:: data_.
            end:: used_.
            expected:: modifications.
            index:: 1.
            [index <= end] whileTrue: [
                element:: data at: index.
                element == removed ifFalse: [
                    action value: element.
                    expected = modifications ifFalse: [^signalModificationOf: self]].
                index:: index + 1]
        )
        (* Answers a new Array of the elements, in adding order. *)
        public asArray = (
            | result data count |
            result:: Array new: size_.
            data:: data_.
            count:: 0.
            1 to: used_ do: [:index |
                (data at: index) == removed ifFalse: [
                    count:: count + 1.
                    result at: count put: (data at: index)]].
            ^result
        )
        (* Answers a new, empty Set with room for `capacity` elements: what
           `collect:`, `select:` and `reject:` fill. *)
        protected newForCollectUsingAdd: capacity = ( ^Set new: capacity )
        (* Empties the set with room for `count + 1` elements, the table a
           third larger so it stays at most three quarters full, and
           answers the new table.

           count: an Integer of at least 0.

           Signals an ArgumentError carrying `count`, 'new: requires a
           capacity of at least 0, got …', for a negative `count`, and what
           `Array new:` signals for a size that is no Integer or too large. *)
        private createTable: count = (
            | table |
            count < 0 ifTrue: [
                ^(kernelModule ArgumentError value: count)
                    signal: 'new: requires a capacity of at least 0, got ', count printString].
            size_:: 0.
            used_:: 0.
            data_:: Array new: count + 1.
            table:: Array new: count + 1 * 4 // 3.
            1 to: table size do: [:index | table at: index put: 0].
            hashtable_:: table.
            ^table
        )
        (* Stores `element` at the end of `data_`, its index in the empty
           bucket `bucketIndex` of `table`, counts it as a change, grows the
           set when `data_` is full, and answers `element`. *)
        private insert: element at: bucketIndex in: table = (
            | index |
            index:: used_ + 1.
            data_ at: index put: element.
            table at: bucketIndex put: index.
            used_:: index.
            size_:: size_ + 1.
            modifications:: modifications + 1.
            index + 1 > data_ size ifTrue: [rehash].
            ^element
        )
        (* Builds a new table, with room for twice the live elements, from
           the live elements of `data_` in their order, dropping the
           removed ones, and answers it. Runs when `data_` is full, and when
           the table slot is nil. *)
        private rehash = (
            | oldData oldEnd table element |
            oldData:: data_.
            oldEnd:: used_.
            table:: createTable: size_ * 2.
            1 to: oldEnd do: [:index |
                element:: oldData at: index.
                element == removed ifFalse: [
                    used_:: used_ + 1.
                    size_:: size_ + 1.
                    data_ at: used_ put: element.
                    table at: (emptyBucketFor: element in: table) put: used_]].
            ^table
        )
        (* Answers the index in `data_` of the element `=` to `element`, or
           the negated index of the empty bucket of `table` where it
           belongs. Probes from its `hash` bucket onwards, passing removed
           elements. *)
        private scanFor: element in: table = (
            | data tableSize bucketIndex index stored |
            data:: data_.
            tableSize:: table size.
            bucketIndex:: element hash \\ tableSize + 1.
            [true] whileTrue: [
                index:: table at: bucketIndex.
                0 = index ifTrue: [^0 - bucketIndex].
                stored:: data at: index.
                (stored == removed) ifFalse: [element = stored ifTrue: [^index]].
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]]
        )
        (* Answers the index of the first empty bucket of `table` from
           `element`'s `hash` bucket onwards: where an element not yet in it
           goes. *)
        private emptyBucketFor: element in: table = (
            | tableSize bucketIndex |
            tableSize:: table size.
            bucketIndex:: element hash \\ tableSize + 1.
            [0 = (table at: bucketIndex)] whileFalse: [
                bucketIndex = tableSize
                    ifTrue: [bucketIndex:: 1]
                    ifFalse: [bucketIndex:: bucketIndex + 1]].
            ^bucketIndex
        )
    ) : (
        (* Answers an empty Set with room for 3 elements, `Set new: 3`. *)
        public new = ( ^self new: 3 )
        (* Answers a new Set of `collection`'s elements, in its `do:` order,
           equal ones merged.

           collection: a Collection, or anything answering `size` and
               `do:`. *)
        public withAll: collection = ( ^(self new: collection size) addAll: collection; yourself )
    )
```

(A `rehash` triggered by `insert:at:in:` while the table slot is being
read cannot happen: `insert:at:in:` runs only after `scanFor:in:` has
answered.)

- [ ] **Step 4: The module without adapted code**

Replace the module comment above `class CollectionsFactory usingKernel:`
-- the licence header and the list of what was changed from another
project -- with a doc comment for the module class, following the
documentation rule:

```newspeak
(* The collections module: the growable Collections a program computes
   with -- `List`, `Map`, `IdentityMap` and `Set` -- and the Errors they
   signal, `NotFound` and `ConcurrentModification`, reached through
   `platform collections`. Built once per kernel by the platform
   (`PlatformFactory`'s `collectionsModule`), with the kernel it was built
   on, from which it takes `Collection`, `Error`, `ArgumentError` and
   `Array`.

   theKernel: the kernel module instance; its `Collection` and `Error` are
       the superclasses of the classes here. *)
(* A value, so that every platform shares the one module and its classes:
   `platform collections List` is one class however many platforms a
   program makes. Its classes are private and reached through the
   `Collections` namespace, so that a program can name a class but not the
   module's slots. *)
```

(Keep whatever the existing decision comments say about why the module is
a value and how it is reached, rewritten to the current state.)
`grep -n -i "primordial\|somns\|squeak\|copyright\|licen" crates/ns-runtime/core-library/CollectionsFactory.ns`
answers nothing.

`THIRD-PARTY-NOTICES.md`: remove the entry for
`crates/ns-runtime/core-library/CollectionsFactory.ns`; keep every other.

Bring every remaining member of the module -- `Collections` and its
accessors, `NotFound` and its members, `newCollections` -- to the
documentation rule, and add `"CollectionsFactory.ns"` to
`DOCUMENTED_MODULES` (the array's length becomes 3).

- [ ] **Step 5: Run the documentation test, the goldens and the benchmarks' goldens**

Run: `cargo nextest run -p ns-runtime --test core_library_documentation`
Run: `cargo nextest run -p ns-cli --test run_goldens collection awfy`
Expected: PASS.

- [ ] **Step 6: Documentation**

`docs/design/core-library.md`, the collections module: `Set` written anew
under `Collection`, with removal; the module holds no adapted code; the
paragraph naming its origin and its licence (about line 3117,
"`Collections.ns` (Apache 2.0) …") is rewritten to say the module is
written here. `docs/references/PROVENANCE.md` and `prior-art/PROVENANCE.md`
if either says `CollectionsFactory.ns` is adapted: rewrite it.
`CLAUDE.md`'s sentence "and Primordial Soup again for
`CollectionsFactory.ns`'s `List`, `Map` and `Set`" in the paragraph on
licensing decisions goes.

- [ ] **Step 7: Gates and commit**

Commit: `feat: Set under Collection with removal; the collections module is written here`.

---

### Task 8: The measurement against `master`

**Files:**
- Create: `.temp/measurements/2026-10-10-collections/` (raw output only;
  `.temp/` is not committed)
- Modify: `docs/design/core-library.md` (one sentence with the result, in
  the collections module's section)

**Interfaces:**
- Consumes: Tasks 5 to 7 (the rewritten `List`, `Map`, `Set`).
- Produces: the numbers the controller reports to the user.

- [ ] **Step 1: Follow the `measuring-performance` skill**

Invoke the skill and follow it exactly: release builds of `master` and of
this branch, the Are We Fast Yet benchmarks (the fourteen and Ton80's
Richards and DeltaBlue, `ns-next bench` as the skill says), the number of
runs and the comparison it prescribes. Never a debug build.

- [ ] **Step 2: Judge and report**

The criterion (spec, "Measurement"): not measurably slower. Report per
benchmark the two medians and the ratio, in the format the skill gives.

- If no benchmark is measurably slower: add one sentence to
  `docs/design/core-library.md`'s collections section ("Measured on
  2026-10-…: the rewritten collections, with the modification count, run
  the Are We Fast Yet benchmarks as fast as before", with the largest
  ratio), commit
  `docs: the rewritten collections measured against master`, and go on.
- If any is measurably slower: **stop**. Do not change code to recover it.
  Report the numbers and the benchmarks that use the collection concerned
  (DeltaBlue: `List` `do:`, `removeFirst`, `sort:`; Havlak: `Map`
  `at:`/`at:put:`, `Set` `add:`/`do:`, `List` `anySatisfy:`) to the
  controller, who takes them to the user. A known candidate, for the user
  to decide on: `Map`'s `scanFor:in:` sends `hashOf:` and `is:sameAs:` on
  every lookup; overriding `scanFor:in:` and `emptyBucketFor:in:`
  themselves in `IdentityMap` (made protected) would let `Map`'s inline
  `hash` and `=` again.

---

### Task 9: The living documents

**Files:**
- Modify: `docs/design/core-library.md`, `docs/OPEN-WORK.md`,
  `docs/PROGRESS.md`, `docs/design/README.md` (if its map or its "how much
  exists" line names the collections)
- Delete: `docs/superpowers/specs/2026-10-10-kernel-collections-and-strings-design.md`

**Interfaces:**
- Consumes: everything above.
- Produces: documentation from which a fresh session can start.

- [ ] **Step 1: `core-library.md`**

- The section "Planned: the kernel completed to Primordial Soup's core
  API": sub-project 3 is built; say so in the list ("3. **Collections and
  strings**, built: …") and move what the spec decided into the section
  "Decisions" as one entry, dated 2026-10-10: the code-point element and
  `stringAt:`; String no Collection; `Collection` a kernel builtin;
  `IdentityMap` under `Map` and why; `NotFound` stays; no immutable
  variants yet and why `Collection` as a class bars neither; leaves where
  Primordial Soup has primitives; the bodies written here;
  `ConcurrentModification`, what counts and when it is checked; the
  documentation rule.
- Everything the earlier tasks described per class is consistent: read
  the whole collections and strings text once and remove any sentence
  that still describes the old state ("only what the benchmarks send",
  "no removal", "adapted from").

- [ ] **Step 2: `OPEN-WORK.md` and `PROGRESS.md`**

- `OPEN-WORK.md`, "Next": sub-project 3 is built; the next piece of work
  is sub-project 4, numbers (`Float`'s mathematics, `Integer`'s and
  `Number`'s protocol, then `Fraction` and the tower), which has no design
  or plan yet. Add an item for what this sub-project left open: the
  ByteArray forms of `String` (`asByteArray`, `copyByteArrayFrom:to:`, a
  ByteArray argument to the searches and `withAll:`) wait for sub-project
  5; `ReadOnlyTuple` (§5.1.7) and value collections are not built
  (decided with the user on 2026-10-10), with why `Collection` would
  become a mixin for the latter. Update the item "Whether an image carries
  a lazy slot's value" where it describes `Map` and `Set` (they now
  remove, and their table is still the lazy `hashtable_`).
- `PROGRESS.md`: an entry for the sub-project in the style of its
  neighbours: what was built, the decisions, the measurement's result.

- [ ] **Step 3: Delete the spec**

`git rm docs/superpowers/specs/2026-10-10-kernel-collections-and-strings-design.md`;
`grep -rn "2026-10-10-kernel-collections-and-strings-design" docs CLAUDE.md`
answers only this plan.

- [ ] **Step 4: Commit**

Only Markdown changed (`git diff --stat`), so no gates run.
Commit: `docs: the kernel's collections and strings in the living documents`.

After this task the controller runs the end-of-batch gates (the
`finishing-a-batch` skill).
