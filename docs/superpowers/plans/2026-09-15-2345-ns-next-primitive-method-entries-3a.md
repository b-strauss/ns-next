# Primitives as Method-Table Entries (Step 3a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every native method becomes an entry of its builtin class's method
table carrying an enumerated `Primitive`, found by the ordinary lookups and
executed by a `match` on the enumeration, so that no send matches a selector
as a string, `dispatch::walk` loses its dispatch-leaf early return, and
`Object`'s `doesNotUnderstand:` is `protected` as §5.5 requires.

**Architecture:** A two-level `Primitive` enum names every native operation
(`Primitive::Integer(IntegerPrimitive::Add)`); a static list per `BuiltinClass`
maps each selector to its primitive and access; the bootstrap fills each
builtin mixin's method table from that list as `MethodEntry::Primitive`. A
lookup that reaches one answers `Lookup::Primitive(primitive, level)`. A
primitive *fails* when its receiver has the wrong shape; a failure passes its
level over and the same lookup continues from that level's superclass
(`Mutator::settle`), which reproduces exactly what the `None` a native answers
today leads to. Behaviour is preserved everywhere except one spec-required
change (Task 4), and the step ends measured.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** `docs/design/dispatch.md`, "Planned: inline caches" → "3a —
Primitives are method-table entries" (the authority for this plan), with
`docs/design/core-library.md` and `docs/references/newspeak-spec.pdf` §5.5.

## Global Constraints

- Work on a branch `feature/primitive-method-entries`, created from
  `feature/inline-caches-design` (which carries the design and this plan), on
  the checked-out tree — no `git worktree` for the work itself (throwaway
  worktrees for the *measurement's* before-binaries are fine and are removed
  afterwards).
- Before Task 1, record `git rev-parse HEAD` as **START**: the commit the step
  starts from, whose code is identical to `master`'s `7da8b92`. Task 5 measures
  against it and against `be3eb40`.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's
  `min_ident_chars` refuses one- and two-character names, which is why the
  collection primitives are `Enumerate` and `ElementAt`, not `Do` and `At`.
- No dispatch logic in `vm.rs` or `compiler/`. Primitive selection and the
  failure rule live in `dispatch.rs`, `interpreter/` and `builtins/`.
- Every native keeps its semantics and its error text word for word; only the
  way it is selected changes.
- A changed recorded outcome is a behaviour change: establish from the
  specification that the new answer is right before updating any golden. The
  only change this step may make is Task 4's.
- Code that implements a spec rule cites it (`newspeak-spec.pdf` §N.N in the
  doc comment), the house style of `dispatch.rs` and `interpreter/`.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, and `cargo test --workspace`. Before the last
  commit of every task also run, in Git Bash:
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about three minutes),
  `cargo run -p ns-cli -- parse-corpus corpus/modules` (must report
  `parsed 210, failed 0, expected-fail 1, stale 0`),
  `cargo run -p ns-cli -- run-corpus corpus/modules` and
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (both must report `ran 6, failed 0, skipped 205`).
- Conventional commits (`test:`, `refactor:`, `feat:`, `fix:`, `perf:`,
  `docs:`). **Every commit message ends with two trailer lines**, the first
  naming the model that actually wrote the commit's code (a subagent names its
  own model, not the controller's):

  ```
  Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01NRmF5jC1Zr2gQ4DdFDtrRS
  ```

- Never override the git author or committer identity.
- Not in scope: an arity check for primitives (a crafted binary can still send
  a native fewer arguments than it reads, as `image-format.md` records);
  giving the platform objects classes of their own; anything of step 3b.

## The primitive table

Every native today, by the `BuiltinClass` whose `dispatch_native` arm answers
it, with the primitive it becomes. Access is `public` for every entry except
`Object`'s `doesNotUnderstand:`, which Task 4 makes `protected`. The
`Boolean`, `True` and `False` classes all get the Boolean list, because
`dispatch_native` routes all three to `boolean::dispatch` today; `Nil` gets no
entries, because `dispatch_native` answers nothing for it.

| Class | Selector | Primitive |
|---|---|---|
| Object | `class` | `Object(ObjectPrimitive::Class)` |
| Object | `==` | `Object(ObjectPrimitive::Identical)` |
| Object | `=` | `Object(ObjectPrimitive::Equal)` |
| Object | `hash` | `Object(ObjectPrimitive::Hash)` |
| Object | `printString` | `Object(ObjectPrimitive::PrintString)` |
| Object | `doesNotUnderstand:` | `Object(ObjectPrimitive::DoesNotUnderstand)` |
| Boolean, True, False | `ifTrue:ifFalse:` | `Boolean(BooleanPrimitive::IfTrueIfFalse)` |
| Boolean, True, False | `ifFalse:ifTrue:` | `Boolean(BooleanPrimitive::IfFalseIfTrue)` |
| Boolean, True, False | `ifTrue:` | `Boolean(BooleanPrimitive::IfTrue)` |
| Boolean, True, False | `ifFalse:` | `Boolean(BooleanPrimitive::IfFalse)` |
| Boolean, True, False | `not` | `Boolean(BooleanPrimitive::Not)` |
| Boolean, True, False | `&`, `and:` | `Boolean(BooleanPrimitive::And)` |
| Boolean, True, False | `\|`, `or:` | `Boolean(BooleanPrimitive::Or)` |
| Boolean, True, False | `printString` | `Boolean(BooleanPrimitive::PrintString)` |
| Integer | `+` `-` `*` `//` | `Integer(IntegerPrimitive::Add / Subtract / Multiply / FloorDivide)` |
| Integer | `<` `>` `<=` `>=` `=` | `Integer(IntegerPrimitive::Less / Greater / LessOrEqual / GreaterOrEqual / Equal)` |
| Integer | `negated`, `abs` | `Integer(IntegerPrimitive::Negated / Absolute)` |
| Integer | `printString`, `asString` | `Integer(IntegerPrimitive::PrintString)` |
| Float | `+` `-` `*` `/` | `Float(FloatPrimitive::Add / Subtract / Multiply / Divide)` |
| Float | `<` `>` `=` | `Float(FloatPrimitive::Less / Greater / Equal)` |
| Float | `printString`, `asString` | `Float(FloatPrimitive::PrintString)` |
| String | `size` `,` `=` `asSymbol` `printString` | `String(StringPrimitive::Size / Concatenate / Equal / AsSymbol / PrintString)` |
| Symbol | `asString` `=` `printString` | `Symbol(SymbolPrimitive::AsString / Equal / PrintString)` |
| Character | `asInteger` `asString` `=` | `Character(CharacterPrimitive::AsInteger / AsString / Equal)` |
| Array | `do:` `at:put:` `size` `at:` | `Collection(CollectionPrimitive::Enumerate / ElementAtPut / Size / ElementAt)` |
| OrderedCollection | `do:` `add:` `size` `at:` | `Collection(CollectionPrimitive::Enumerate / Add / Size / ElementAt)` |
| Block | `value` `value:` `value:value:` `value:value:value:` | `Block(BlockPrimitive::Value)` |
| Block | `numArgs` `whileTrue:` `whileFalse:` | `Block(BlockPrimitive::ArgumentCount / WhileTrue / WhileFalse)` |
| Platform | `transcript` `Object` `Array` `OrderedCollection` `String` `Symbol` `collections` | `Platform(PlatformPrimitive::Transcript / ObjectClass / ArrayClass / OrderedCollectionClass / StringClass / SymbolClass / Collections)` |
| Platform | `List` | `Platform(PlatformPrimitive::List)` |
| Platform | `show:` `newLine` | `Platform(PlatformPrimitive::Show / NewLine)` |

A primitive fails — answers `None` from `builtins::execute` — exactly where its
native answers `None` today: a receiver of the wrong variant or payload. The
three platform objects share `BuiltinClass::Platform` and are told apart by
their instance field count (`platform.rs`): the seven singleton primitives
require a count of 0, `List` a count of 2, and `Show`/`NewLine` any other
count — the same partition `platform::dispatch` makes today.

---

### Task 1: Pin the outcomes 3a must keep

**Files:**
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:** Consumes nothing; produces a regression guard every later task
must keep green.

- [ ] **Step 1: Write the pinning test.** Append to `program_outcomes.rs`:

```rust
/// A user subclass of a builtin inherits the builtin's natives, but its
/// instances carry no payload of the builtin's kind, so every such native
/// fails on them and the send falls to `Object`'s: `=` answers identity and
/// `printString` the generic default, through an explicit receiver, a `self`
/// send and a `super` send alike, while `size`, which `Object` does not
/// have, ends in `doesNotUnderstand:`. The `transcript` stream shares the
/// `Platform` class with the platform singleton and has no `=` of its own,
/// so `=` is `Object`'s identity, false for two fresh streams. Step 3a keeps
/// every one of these outcomes (`docs/design/dispatch.md`, "A primitive can
/// fail").
#[test]
fn a_native_that_fails_on_its_receiver_falls_to_objects() {
    assert_outcome(
        "native-fallback",
        indoc! {"
            Newspeak3
            'T'
            class Sub usingPlatform: platform = Object new (
            ) (
                class Text = String new () (
                    public viaSelf = ( ^self printString )
                    public viaSuper = ( ^super printString )
                )
                public main: p args: a = (
                    | t |
                    t := Text new.
                    p transcript show: (t = t) printString; newLine.
                    p transcript show: t printString; newLine.
                    p transcript show: t viaSelf; newLine.
                    p transcript show: t viaSuper; newLine.
                    p transcript show: (p transcript = p transcript) printString; newLine.
                    p transcript show: t size printString; newLine
                )
            ) : ()
        "},
        1,
        "true\nan Object\nan Object\nan Object\nfalse\n",
        "Uncaught Newspeak error: an object does not understand #size\n",
    );
}
```

- [ ] **Step 2: Run it; it must pass on START.**

Run: `cargo test -p ns-cli --test program_outcomes a_native_that_fails_on_its_receiver_falls_to_objects`
Expected: PASS. This outcome was probed on START under both lowerings before
the plan was written; if it does not pass, stop and report — the plan's
failure rule rests on it.

- [ ] **Step 3: Gates and commit.** Run the gates of the Global Constraints,
then:

```bash
git add crates/ns-cli/tests/program_outcomes.rs
git commit -m "test: pin how a native that fails on its receiver falls to Object's" -m "<trailer lines>"
```

---

### Task 2: The `Primitive` enumeration and `builtins::execute`

Selection by enumeration replaces every `match selector.as_str()` in
`builtins/` and `platform.rs`; lookup is not touched yet. `dispatch_native`
stays as a temporary bridge that finds the primitive for `(kind, selector)` in
the static list, so the suite stays green and behaviour stays identical.

**Files:**
- Create: `crates/ns-runtime/src/builtins/primitive.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs`, `object_class.rs`,
  `boolean.rs`, `numbers.rs`, `text.rs`, `collections.rs`, `block.rs`
- Modify: `crates/ns-runtime/src/platform.rs`

**Interfaces:**
- Produces (in `builtins::primitive`, all `pub`):
  - `enum Primitive { Object(ObjectPrimitive), Boolean(BooleanPrimitive), Integer(IntegerPrimitive), Float(FloatPrimitive), String(StringPrimitive), Symbol(SymbolPrimitive), Character(CharacterPrimitive), Collection(CollectionPrimitive), Block(BlockPrimitive), Platform(PlatformPrimitive) }`
    and the ten inner enums with exactly the variants of "The primitive
    table", every one `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`.
  - `struct PrimitiveSelector { pub selector: &'static str, pub access: ast::AccessModifier, pub primitive: Primitive }`
  - `fn primitives_of(kind: BuiltinClass) -> &'static [PrimitiveSelector]`
- Produces (in `builtins`): `pub(crate) fn execute<'gc>(mutator: &mut Mutator<'gc>, primitive: Primitive, receiver: Value<'gc>, selector: Symbol, arguments: &[Value<'gc>]) -> Option<Result<Step<'gc>, Signal<'gc>>>` — `None` is a primitive failure.

- [ ] **Step 1: Write the failing tests** at the end of the new
  `builtins/primitive.rs` (the file holds only the test module at this point,
  so the build fails on the missing items):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A method table holds one entry per selector, so a list that names a
    /// selector twice would silently keep only one of its primitives.
    #[test]
    fn no_builtin_lists_a_selector_twice() {
        for kind in BuiltinClass::ALL {
            let mut seen = HashSet::new();
            for entry in primitives_of(kind) {
                assert!(
                    seen.insert(entry.selector),
                    "{kind:?} lists `{}` twice",
                    entry.selector
                );
            }
        }
    }

    /// `Nil` has no natives of its own today; every other builtin does.
    #[test]
    fn every_builtin_but_nil_has_primitives() {
        for kind in BuiltinClass::ALL {
            assert_eq!(
                primitives_of(kind).is_empty(),
                kind == BuiltinClass::Nil,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn integer_addition_is_its_own_primitive() {
        let entry = primitives_of(BuiltinClass::Integer)
            .iter()
            .find(|entry| entry.selector == "+")
            .expect("Integer lists `+`");
        assert_eq!(entry.primitive, Primitive::Integer(IntegerPrimitive::Add));
        assert_eq!(entry.access, ast::AccessModifier::Public);
    }
}
```

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --lib builtins::primitive`
Expected: FAIL to compile — `primitives_of`, `Primitive` and
`IntegerPrimitive` are not defined (and the module is not declared yet; add
`pub mod primitive;` to `builtins/mod.rs` first).

- [ ] **Step 3: Write `builtins/primitive.rs`.** Above the test module:

```rust
//! The enumerated primitives: every native operation of the builtin classes,
//! named rather than selected by its selector's text, so that a lookup can
//! bind one and a later JIT tier can recognize it and emit it inline
//! (`docs/design/dispatch.md`, "3a — Primitives are method-table entries").

use ns_syntax::ast;

use crate::types::BuiltinClass;

/// One native operation. Grouped by the builtin family whose module executes
/// it, so each module matches its own group exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Object(ObjectPrimitive),
    Boolean(BooleanPrimitive),
    Integer(IntegerPrimitive),
    Float(FloatPrimitive),
    String(StringPrimitive),
    Symbol(SymbolPrimitive),
    Character(CharacterPrimitive),
    Collection(CollectionPrimitive),
    Block(BlockPrimitive),
    Platform(PlatformPrimitive),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectPrimitive { Class, Identical, Equal, Hash, PrintString, DoesNotUnderstand }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanPrimitive { IfTrueIfFalse, IfFalseIfTrue, IfTrue, IfFalse, Not, And, Or, PrintString }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerPrimitive {
    Add, Subtract, Multiply, FloorDivide, Less, Greater, LessOrEqual, GreaterOrEqual,
    Equal, Negated, Absolute, PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPrimitive { Add, Subtract, Multiply, Divide, Less, Greater, Equal, PrintString }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringPrimitive { Size, Concatenate, Equal, AsSymbol, PrintString }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolPrimitive { AsString, Equal, PrintString }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterPrimitive { AsInteger, AsString, Equal }

/// `Array` and `OrderedCollection` share their payload, so they share their
/// primitives; each class's list names only the ones it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionPrimitive { Enumerate, Size, ElementAt, ElementAtPut, Add }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockPrimitive { Value, ArgumentCount, WhileTrue, WhileFalse }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformPrimitive {
    Transcript, ObjectClass, ArrayClass, OrderedCollectionClass, StringClass,
    SymbolClass, Collections, List, Show, NewLine,
}

/// One selector a builtin class answers natively.
#[derive(Debug, Clone, Copy)]
pub struct PrimitiveSelector {
    pub selector: &'static str,
    pub access: ast::AccessModifier,
    pub primitive: Primitive,
}

const fn public(selector: &'static str, primitive: Primitive) -> PrimitiveSelector {
    PrimitiveSelector { selector, access: ast::AccessModifier::Public, primitive }
}
```

  Then one `const` slice per family, written out from "The primitive table"
  (every row, every selector — `&` and `and:` are two entries with the same
  primitive, and so are `printString` and `asString`, and the four `value`
  arities), for example:

```rust
const OBJECT: &[PrimitiveSelector] = &[
    public("class", Primitive::Object(ObjectPrimitive::Class)),
    public("==", Primitive::Object(ObjectPrimitive::Identical)),
    public("=", Primitive::Object(ObjectPrimitive::Equal)),
    public("hash", Primitive::Object(ObjectPrimitive::Hash)),
    public("printString", Primitive::Object(ObjectPrimitive::PrintString)),
    public("doesNotUnderstand:", Primitive::Object(ObjectPrimitive::DoesNotUnderstand)),
];
```

  and `BOOLEAN`, `INTEGER`, `FLOAT`, `STRING`, `SYMBOL`, `CHARACTER`,
  `ARRAY`, `ORDERED_COLLECTION`, `BLOCK`, `PLATFORM` in the same form, then:

```rust
/// The selectors `kind` answers natively, in the order its module lists them.
pub fn primitives_of(kind: BuiltinClass) -> &'static [PrimitiveSelector] {
    match kind {
        BuiltinClass::Object => OBJECT,
        BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => BOOLEAN,
        BuiltinClass::Integer => INTEGER,
        BuiltinClass::Float => FLOAT,
        BuiltinClass::String => STRING,
        BuiltinClass::Symbol => SYMBOL,
        BuiltinClass::Character => CHARACTER,
        BuiltinClass::Array => ARRAY,
        BuiltinClass::OrderedCollection => ORDERED_COLLECTION,
        BuiltinClass::Block => BLOCK,
        BuiltinClass::Platform => PLATFORM,
        BuiltinClass::Nil => &[],
    }
}
```

  Let `cargo fmt` lay the enums out one variant per line.

- [ ] **Step 4: Run the tests.**

Run: `cargo test -p ns-runtime --lib builtins::primitive`
Expected: PASS (3 tests).

- [ ] **Step 5: Convert each family module from selector to primitive.** In
  each, the dispatch function becomes an `execute` function whose `match`
  is over the family's enum instead of `selector.as_str()`; **every arm body
  moves unchanged**, including every `Symbol::intern(operator)` and every
  error message. Receiver checks (`let Value::SmallInteger(..) = receiver
  else { return None }` and the like) stay where they are: a `None` is a
  primitive failure.

  - `object_class.rs`: `pub(crate) fn execute<'gc>(mutator: &mut Mutator<'gc>, primitive: ObjectPrimitive, receiver: Value<'gc>, arguments: &[Value<'gc>]) -> Option<Result<Value<'gc>, NewspeakError<'gc>>>`; arms `Class`, `Identical`, `Equal`, `Hash`, `PrintString`, `DoesNotUnderstand` for `"class"`, `"=="`, `"="`, `"hash"`, `"printString"`, `"doesNotUnderstand:"`. The match is exhaustive, so the `_ => None` arm goes.
  - `boolean.rs`: `execute(mutator, primitive: BooleanPrimitive, receiver, arguments) -> Option<Result<Step, Signal>>`; `And` takes the body of `"&" | "and:"`, `Or` the body of `"|" | "or:"`.
  - `numbers.rs`: `execute_integer(mutator, primitive: IntegerPrimitive, receiver, arguments)` and `execute_float(mutator, primitive: FloatPrimitive, receiver, arguments)`, both answering `Option<Result<Value, NewspeakError>>`; `PrintString` takes the body of `"printString" | "asString"`.
  - `text.rs`: `execute_string(mutator, primitive: StringPrimitive, receiver, selector, arguments)` (the `,` error names `selector`), `execute_symbol(mutator, primitive: SymbolPrimitive, receiver, arguments)`, `execute_character(mutator, primitive: CharacterPrimitive, receiver, arguments)`.
  - `collections.rs`: one `pub(crate) fn execute<'gc>(mutator: &mut Mutator<'gc>, primitive: CollectionPrimitive, receiver: Value<'gc>, selector: Symbol, arguments: &[Value<'gc>]) -> Option<Result<Step<'gc>, Signal<'gc>>>`: `Enumerate` → `do_step(mutator, receiver, selector, arguments)`; `Size`, `ElementAt`, `ElementAtPut`, `Add` → the bodies of `"size"`, `"at:"` (`dispatch_common`), `"at:put:"` (`array_leaf`) and `"add:"` (`ordered_collection_leaf`), each lifted by `leaf_step`. `dispatch_common`, `array_leaf`, `ordered_collection_leaf`, `dispatch_array` and `dispatch_ordered_collection` fold into that one function and small per-primitive helpers; keep `out_of_range`, `not_an_integer`, `do_step`, `after_element` and `element_step` as they are. Which class answers `at:put:` and which `add:` is now decided by the lists, not by the function.
  - `block.rs`: `execute(mutator, primitive: BlockPrimitive, receiver, arguments) -> Option<Result<Step, Signal>>`; `Value` takes the body of the four `value` arities, `ArgumentCount` of `"numArgs"`.
  - `platform.rs`: `pub fn execute<'gc>(mutator: &mut Mutator<'gc>, primitive: PlatformPrimitive, receiver: Value<'gc>, arguments: &[Value<'gc>]) -> Option<Result<Value<'gc>, NewspeakError<'gc>>>`. Read the field count first as today, then:

```rust
    let tag = mutator.heap().instance_field_count(*object)?;
    match primitive {
        PlatformPrimitive::List => (tag == 2).then(|| collections_list(mutator)),
        PlatformPrimitive::Show | PlatformPrimitive::NewLine => {
            (tag != 0 && tag != 2).then(|| stream(mutator, primitive, receiver, arguments))
        }
        _ if tag != 0 => None,
        PlatformPrimitive::Transcript => Some(Ok(Value::Object(mutator.new_stream_object()))),
        // ... ObjectClass, ArrayClass, OrderedCollectionClass, StringClass,
        // SymbolClass and Collections with today's bodies
    }
```

    where `collections_list` is today's `"List"` body and `stream` today's
    `"show:"`/`"newLine"` bodies matched on the primitive. Keep the module
    comment's explanation of the field-count tag.

- [ ] **Step 6: `builtins::execute` and the temporary bridge.** In
  `builtins/mod.rs`:

```rust
/// Runs `primitive` on `receiver`. `None` is a primitive failure: the
/// receiver is not of the shape the primitive works on (`docs/design/dispatch.md`,
/// "A primitive can fail"). `selector` is the send's, for the error texts
/// that name it. No primitive calls a block: the control-flow ones, `value`
/// and `do:` answer a `Step` (`docs/design/execution.md`, "The trampoline").
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        Primitive::Object(primitive) => {
            object_class::execute(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Boolean(primitive) => boolean::execute(mutator, primitive, receiver, arguments),
        Primitive::Integer(primitive) => {
            numbers::execute_integer(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Float(primitive) => {
            numbers::execute_float(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::String(primitive) => {
            text::execute_string(mutator, primitive, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Symbol(primitive) => {
            text::execute_symbol(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Character(primitive) => {
            text::execute_character(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Collection(primitive) => {
            collections::execute(mutator, primitive, receiver, selector, arguments)
        }
        Primitive::Block(primitive) => block::execute(mutator, primitive, receiver, arguments),
        Primitive::Platform(primitive) => {
            platform::execute(mutator, primitive, receiver, arguments).map(wrap_error)
        }
    }
}
```

  and rewrite `dispatch_native` as the bridge Task 3 removes:

```rust
/// Temporary, until lookup finds primitives in the method tables (step 3a,
/// Task 3): the primitive `kind` lists for `selector`, run.
pub(crate) fn dispatch_native<'gc>(
    mutator: &mut Mutator<'gc>,
    kind: BuiltinClass,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let primitive = primitive::primitives_of(kind)
        .iter()
        .find(|entry| entry.selector == selector.as_str())?
        .primitive;
    execute(mutator, primitive, receiver, selector, arguments)
}
```

- [ ] **Step 7: Run the whole suite.**

Run: `cargo test --workspace`
Expected: PASS, every existing test unchanged — in particular
`builtins_boolean_numbers.rs`, `builtins_text.rs`,
`builtins_collections_block.rs`, `builtin_object_inheritance.rs`,
`platform.rs` and Task 1's pin.

- [ ] **Step 8: Gates and commit.**

```bash
git add crates/ns-runtime/src/builtins crates/ns-runtime/src/platform.rs
git commit -m "refactor: select every native by an enumerated primitive" -m "<trailer lines>"
```

---

### Task 3: Primitives in the method tables, found by the ordinary lookups

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MethodEntry::Primitive`, `PrimitiveMethod`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`method_table`)
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (the bridge goes)
- Modify: `crates/ns-runtime/src/dispatch.rs` (`Rule`, `lookup`, `lookup_above`, `Lookup::Primitive`, `walk`)
- Modify: `crates/ns-runtime/src/heap.rs` (`class_lookup_step`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Settled`, `settle`, `decide_send`, the `doesNotUnderstand:` path, `builtin_mixin`)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs`
- Modify: `crates/ns-runtime/src/types.rs` (the `ClassData::builtin` doc comment)
- Modify: `crates/ns-cli/src/commands/build.rs`
- Modify: `crates/ns-runtime/tests/builtin_object_inheritance.rs` (module doc only)
- Test: `crates/ns-runtime/src/dispatch.rs` (unit tests), `crates/ns-cli/tests/build_load.rs`

**Interfaces:**
- Consumes: `Primitive`, `primitives_of`, `builtins::execute` from Task 2.
- Produces:
  - `mixin::PrimitiveMethod { pub primitive: Primitive, pub access: ast::AccessModifier }` and `MethodEntry::Primitive(PrimitiveMethod)`.
  - `builtins::primitive::method_table(kind: BuiltinClass) -> IdentityMap<Symbol, MethodEntry>`.
  - `dispatch::Rule { Public, Protected, DefinedFor }`, `dispatch::lookup(heap, rule, class, selector) -> Lookup`, `dispatch::lookup_above(heap, rule, level, selector) -> Lookup`, `Lookup::Primitive(Primitive, ClassReference<'gc>)` replacing `Lookup::Native(BuiltinClass)`.
  - `interpreter::Settled<'gc> { Answered(Result<Step<'gc>, Signal<'gc>>), Found(Lookup<'gc>) }` and `Mutator::settle(&mut self, rule, lookup, receiver, selector, arguments) -> Settled<'gc>`.
  - `Heap::class_lookup_step(class) -> Option<Option<ClassReference<'gc>>>` (the superclass, or `None` for a non-class).

- [ ] **Step 1: Write the failing lookup tests** in `dispatch.rs`'s test
  module:

```rust
    use crate::builtins::primitive::{IntegerPrimitive, ObjectPrimitive, Primitive};

    /// A builtin's own native is an entry of its own method table.
    #[test]
    fn a_builtins_own_primitive_is_found_at_its_own_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            match lookup_public(&mutator.heap, integer, Symbol::intern("+")) {
                Lookup::Primitive(Primitive::Integer(IntegerPrimitive::Add), level) => {
                    assert!(level.same(integer), "found above Integer's own level")
                }
                _ => panic!("`3 + 4` must find Integer's own `+`"),
            }
        });
    }

    /// `Integer` has no `==`; the ordinary walk reaches `Object`'s, since
    /// `Integer`'s superclass is `Object` -- no hand-restored inheritance.
    #[test]
    fn a_primitive_a_builtin_lacks_is_found_at_objects_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let object = mutator.builtin_class(BuiltinClass::Object);
            match lookup_public(&mutator.heap, integer, Symbol::intern("==")) {
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::Identical), level) => {
                    assert!(level.same(object))
                }
                _ => panic!("`3 == 3` must find Object's `==`"),
            }
        });
    }

    /// What a failed primitive passes to: the same lookup, from the level
    /// above the one that declared it.
    #[test]
    fn a_lookup_continued_above_a_level_passes_that_levels_entry_over() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let string = mutator.builtin_class(BuiltinClass::String);
            match lookup_above(&mutator.heap, Rule::Public, string, Symbol::intern("=")) {
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::Equal), _) => {}
                _ => panic!("above String, `=` is Object's"),
            }
            let object = mutator.builtin_class(BuiltinClass::Object);
            assert!(matches!(
                lookup_above(&mutator.heap, Rule::Public, object, Symbol::intern("=")),
                Lookup::NotFound
            ));
        });
    }
```

- [ ] **Step 2: Write the failing CLI test** in `build_load.rs`:

```rust
/// `build` accepts an entry only where `mixins::encode` has interned its
/// selector (`commands/build.rs`, `root_class_answers`): a method or nested
/// class of the module's own. `printString` is answered only by a builtin's
/// primitive, which no table of the module names, so it stays refused now
/// that primitives are method-table entries too.
#[test]
fn build_refuses_an_entry_only_a_builtin_answers() {
    let directory = ScratchDirectory::new("builtin-entry");
    let out = directory.join("app.nsb");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "printString".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("does not answer `printString`"), "{error}");
    assert!(!out.exists(), "a refused build must write no file");
}
```

- [ ] **Step 3: Run them.**

Run: `cargo test -p ns-runtime --lib dispatch::tests` and `cargo test -p ns-cli --test build_load build_refuses_an_entry_only_a_builtin_answers`
Expected: the three `dispatch` tests FAIL to compile (`Lookup::Primitive`,
`lookup_above`, `Rule` undefined); the CLI test PASSES on the current code —
it guards the change in Step 11, which would otherwise break it.

- [ ] **Step 4: `MethodEntry::Primitive`.** In `mixin.rs`, beside
  `SlotAccessor`:

```rust
/// A native method of a builtin class (`docs/design/dispatch.md`, "3a --
/// Primitives are method-table entries"): the primitive it runs and its §5.2
/// accessibility. Found by the same lookups as a compiled method; it runs no
/// code and pushes no activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimitiveMethod {
    pub primitive: crate::builtins::primitive::Primitive,
    pub access: ast::AccessModifier,
}
```

  Add `Primitive(PrimitiveMethod)` to `MethodEntry`, update its doc comment
  ("a compiled method, one of the accessors a slot declaration defines, or a
  builtin's primitive"), and add the arms: `access()` answers
  `method.access`; `compiled()` answers `None`.

- [ ] **Step 5: `method_table`.** In `builtins/primitive.rs`:

```rust
/// The method table the bootstrap gives `kind`'s mixin: one
/// `MethodEntry::Primitive` per selector `kind` answers natively.
pub fn method_table(kind: BuiltinClass) -> IdentityMap<Symbol, MethodEntry> {
    primitives_of(kind)
        .iter()
        .map(|entry| {
            (
                Symbol::intern(entry.selector),
                MethodEntry::Primitive(PrimitiveMethod {
                    primitive: entry.primitive,
                    access: entry.access,
                }),
            )
        })
        .collect()
}
```

  (imports: `crate::hash::IdentityMap`, `crate::mixin::{MethodEntry,
  PrimitiveMethod}`, `crate::Symbol`). In `interpreter/mod.rs`,
  `builtin_mixin` fills it:

```rust
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin {
        methods: crate::builtins::primitive::method_table(kind),
        ..crate::mixin::Mixin::empty(Symbol::intern(&format!("{kind:?}")))
    })
}
```

  and the comment in `build_builtin_classes` that says a builtin's mixin
  "declares nothing" now says its method table holds its primitives.

- [ ] **Step 6: `dispatch.rs`.** Replace the verdict closures with a rule:

```rust
/// Which of the three lookups a walk is (`newspeak-spec.pdf` §5.5, §5.9,
/// §3.5) -- what it does with a declaration it finds at one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// §5.5's lookupPublic: public accepted, protected stops, private skipped.
    Public,
    /// §5.9's lookupProtected: public and protected accepted, private skipped.
    Protected,
    /// §3.5's "defined for", `doesNotUnderstand:`'s: every declaration accepted.
    DefinedFor,
}

impl Rule {
    fn verdict(self, access: ast::AccessModifier) -> Verdict {
        match (self, access) {
            (Rule::DefinedFor, _)
            | (Rule::Public, ast::AccessModifier::Public)
            | (Rule::Protected, ast::AccessModifier::Public | ast::AccessModifier::Protected) => {
                Verdict::Accept
            }
            (Rule::Public, ast::AccessModifier::Protected) => Verdict::Stop,
            (Rule::Public | Rule::Protected, ast::AccessModifier::Private) => Verdict::Skip,
        }
    }
}

pub fn lookup<'gc>(heap: &Heap<'gc>, rule: Rule, class: ClassReference<'gc>, selector: Symbol) -> Lookup<'gc> {
    walk(heap, class, selector, rule)
}

/// The same lookup, continued from the level above `level`: where a
/// primitive found at `level` that failed passes the send on
/// (`Mutator::settle`). `NotFound` above the root.
pub fn lookup_above<'gc>(heap: &Heap<'gc>, rule: Rule, level: ClassReference<'gc>, selector: Symbol) -> Lookup<'gc> {
    match heap.class_superclass(level) {
        Some(superclass) => lookup(heap, rule, superclass, selector),
        None => Lookup::NotFound,
    }
}
```

  `lookup_public`, `lookup_protected` and `lookup_defined_for` keep their
  doc comments and become `lookup(heap, Rule::…, class, selector)`. `walk`
  takes `rule: Rule` and calls `rule.verdict(...)` where it called
  `verdict(...)`; its level step becomes

```rust
        let Some(superclass) = heap.class_lookup_step(level_class) else {
            return Lookup::NotFound;
        };
```

  and the `if let Some(kind) = builtin { return Lookup::Native(kind); }`
  block is deleted. `Lookup::Native(BuiltinClass)` becomes

```rust
    /// A builtin's primitive, with the class object of the level whose
    /// method table holds it: a primitive that fails passes that level over,
    /// and the lookup continues from its superclass (`lookup_above`).
    Primitive(Primitive, ClassReference<'gc>),
```

  and `found_entry` gains
  `MethodEntry::Primitive(method) => Lookup::Primitive(method.primitive, level_class)`.
  In `heap.rs`, `class_lookup_step` answers `Option<Option<ClassReference<'gc>>>`
  — `Some(data.superclass)` for a class object, `None` otherwise — and its doc
  comment drops "builtin kind".

- [ ] **Step 7: `Mutator::settle`.** In `interpreter/mod.rs`:

```rust
/// What a lookup answered once any primitive it found has run.
pub(crate) enum Settled<'gc> {
    /// A primitive ran and answered.
    Answered(Result<Step<'gc>, Signal<'gc>>),
    /// The lookup's answer; never `Lookup::Primitive`.
    Found(Lookup<'gc>),
}
```

  and on `Mutator`:

```rust
    /// Runs the primitive `lookup` found, if it found one. A primitive fails
    /// when its receiver has the wrong shape -- an instance of a user subclass
    /// of `String`, which carries no string -- and a failure passes its level
    /// over: the lookup continues under the same `rule` from that level's
    /// superclass, `Object` for every builtin, and what it finds there is
    /// settled the same way. That is the outcome a native's `None` had before
    /// primitives were method-table entries: `Object`'s natives, then
    /// `doesNotUnderstand:` (`docs/design/dispatch.md`, "A primitive can
    /// fail"). The loop ends: each turn moves one level up a superclass chain
    /// that `image::decode` refuses to let form a cycle.
    pub(crate) fn settle(
        &mut self,
        rule: dispatch::Rule,
        mut lookup: Lookup<'gc>,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Settled<'gc> {
        loop {
            match lookup {
                Lookup::Primitive(primitive, level) => {
                    if let Some(result) =
                        crate::builtins::execute(self, primitive, receiver, selector, arguments)
                    {
                        return Settled::Answered(result);
                    }
                    lookup = dispatch::lookup_above(&self.heap, rule, level, selector);
                }
                found => return Settled::Found(found),
            }
        }
    }
```

- [ ] **Step 8: The callers.** Each runs its lookup through `settle` and
  keeps its own "not found". A `Settled::Found(Lookup::Primitive(..))` arm is
  `unreachable!("internal: `settle` runs every primitive it is handed")`.
  - `decide_send`, main path: `lookup_public(class)` settled with
    `Rule::Public`; `Answered(result)` → `result`; `Method`, `SlotAccessor`,
    `NestedClass` arms as today; `NotFound` →
    `self.decide_does_not_understand(receiver, selector, arguments)`.
  - `decide_send`, metaclass branch: `lookup_public(metaclass)` settled with
    `Rule::Public`; `Answered` returns it; the three member arms return as
    today; `NotFound` falls through to the class object's own behaviour, as
    today. A comment notes that no metaclass chain holds a primitive — a
    metaclass's mixins are class sides or empty stand-ins, never a builtin's
    instance mixin — so `Answered` is reached only in principle.
  - `sends::decide_lexical_send`: the combined `lookup` (private rule, then the
    metaclass-first `lookup_protected`, whose filter keeps `Method`,
    `SlotAccessor` and `NestedClass` and so passes a primitive over exactly as
    it passed `Native` over) settled with `Rule::Protected` against
    `enclosing`; `NotFound` → `mutator.decide_send(enclosing, selector,
    arguments)`, as today.
  - `sends::decide_super_send`: `lookup_protected(superclass)` settled with
    `Rule::Protected` against `self_value`; `NotFound` →
    `mutator.decide_super_does_not_understand(self_value, superclass,
    selector, arguments)`.
  - `decide_does_not_understand_in`: build the message argument first — the
    three allocations every arm made — then settle
    `lookup_defined_for(class, Symbol::DOES_NOT_UNDERSTAND)` with
    `Rule::DefinedFor`, `Symbol::DOES_NOT_UNDERSTAND` and
    `&[message_argument]`:

```rust
        let selector_value = self.new_symbol(selector);
        let arguments_value = Value::Object(self.new_array(arguments.to_vec()));
        let message_argument = Value::Object(self.new_array(vec![selector_value, arguments_value]));
        let lookup = dispatch::lookup_defined_for(&self.heap, class, Symbol::DOES_NOT_UNDERSTAND);
        match self.settle(
            dispatch::Rule::DefinedFor,
            lookup,
            receiver,
            Symbol::DOES_NOT_UNDERSTAND,
            &[message_argument],
        ) {
            Settled::Answered(result) => result,
            Settled::Found(Lookup::Method(method, defining_class)) => Ok(Step::Method {
                receiver,
                method,
                defining_class,
                arguments: StepArguments::Owned(vec![message_argument]),
            }),
            Settled::Found(Lookup::SlotAccessor(accessor, field)) => {
                self.decide_slot_accessor(receiver, accessor, field, &[message_argument])
            }
            Settled::Found(Lookup::NotFound | Lookup::NestedClass(..)) => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
            })),
            Settled::Found(Lookup::Primitive(..)) => {
                unreachable!("internal: `settle` runs every primitive it is handed")
            }
        }
```

    Allocation order is unobservable (identity hashes are drawn lazily), so
    allocating the message before the lookup changes nothing a program sees.
    The "native class … has no doesNotUnderstand: implementation" error goes:
    `Object`'s primitive never fails.

- [ ] **Step 9: Delete what 3a replaces.** `builtins::dispatch_native` (the
  bridge), `Mutator::dispatch_native_inheriting_object` and
  `Mutator::decide_native_or_does_not_understand`, with their doc comments.
  Update `types.rs`'s `ClassData::builtin` doc comment: it marks a core
  library class for `class_of`, the image's class tags and `LoadBuiltinClass`;
  lookup no longer reads it. Update the module doc of
  `crates/ns-runtime/tests/builtin_object_inheritance.rs`: every builtin's
  superclass is `Object`, and the ordinary walk reaches `Object`'s primitives
  from any builtin receiver; the tests are unchanged.

- [ ] **Step 10: Run the lookup tests.**

Run: `cargo test -p ns-runtime --lib dispatch::tests`
Expected: PASS, the three new tests with the existing ones.

- [ ] **Step 11: `build`'s entry check skips primitives.** In
  `crates/ns-cli/src/commands/build.rs`, `root_class_answers`:

```rust
        let declared = application
            .mixin
            .methods
            .get(&symbol)
            .is_some_and(|entry| !matches!(entry, MethodEntry::Primitive(_)));
        if declared {
            return true;
        }
```

  (`use ns_runtime::mixin::MethodEntry;`). Rewrite the doc comment's
  paragraph on walking past the root level: a builtin mixin now holds its
  primitives, which `mixins::encode` never writes, so the check counts only
  what is not a primitive; the cross-module caveat stays as it is.

  Nothing reaches an artifact: `image::mixins::plan` lists only the module's
  own mixins and, of their tables, only `compiled_methods()`, which answers
  `None` for a primitive, and the builtin mixins are rebuilt by the bootstrap
  on every load. `a_build_and_a_snapshot_agree_on_the_describing_sections`
  and the image tests keep holding unchanged; if one moves, stop and report.

- [ ] **Step 12: Run the whole suite.**

Run: `cargo test --workspace`
Expected: PASS — every existing test, Task 1's pin, and
`build_refuses_an_entry_only_a_builtin_answers`.

- [ ] **Step 13: Gates and commit.**

```bash
git add crates
git commit -m "refactor!: make every native a method-table entry found by lookup" -m "<trailer lines>"
```

---

### Task 4: `Object`'s `doesNotUnderstand:` is protected

§5.5: `Object`'s `doesNotUnderstand:` "must be protected", and
`lookupPublic` is "Undefined, if the mixin of C defines a method m with
selector n and protected access". An explicit-receiver
`3 doesNotUnderstand: 5` therefore finds nothing, and `doesNotUnderstand:` is
invoked for *that* send: its message names `doesNotUnderstand:`. Today the
native answers the send directly and reads `5` as the message, reporting
`#unknown`.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (the `OBJECT` list)
- Test: `crates/ns-cli/tests/program_outcomes.rs`, `crates/ns-runtime/src/dispatch.rs`

- [ ] **Step 1: Write the failing tests.** In `program_outcomes.rs`:

```rust
/// `newspeak-spec.pdf` §5.5: `Object`'s `doesNotUnderstand:` must be
/// protected, and lookupPublic is "Undefined, if the mixin of C defines a
/// method m with selector n and protected access". So an explicit-receiver
/// send of `doesNotUnderstand:` finds nothing, and `Object`'s
/// `doesNotUnderstand:` -- found by the access-blind "defined for" walk --
/// is invoked for that send, whose selector is `doesNotUnderstand:`.
#[test]
fn an_explicit_does_not_understand_send_is_refused_as_protected() {
    assert_outcome(
        "explicit-dnu",
        indoc! {"
            Newspeak3
            'T'
            class Dnu usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    3 doesNotUnderstand: 5
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #doesNotUnderstand:\n",
    );
}
```

  In `dispatch.rs`'s test module:

```rust
    /// §5.5: `Object`'s `doesNotUnderstand:` is protected -- lookupPublic
    /// stops at it, lookupProtected and the "defined for" walk accept it.
    #[test]
    fn objects_does_not_understand_is_protected() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let selector = Symbol::DOES_NOT_UNDERSTAND;
            assert!(matches!(lookup_public(&mutator.heap, integer, selector), Lookup::NotFound));
            assert!(matches!(
                lookup_protected(&mutator.heap, integer, selector),
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::DoesNotUnderstand), _)
            ));
            assert!(matches!(
                lookup_defined_for(&mutator.heap, integer, selector),
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::DoesNotUnderstand), _)
            ));
        });
    }
```

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-cli --test program_outcomes an_explicit_does_not_understand_send_is_refused_as_protected` and `cargo test -p ns-runtime --lib dispatch::tests::objects_does_not_understand_is_protected`
Expected: both FAIL — the outcome's stderr reads `#unknown`, and
`lookup_public` finds the primitive.

- [ ] **Step 3: Make it protected.** In `builtins/primitive.rs` add

```rust
const fn protected(selector: &'static str, primitive: Primitive) -> PrimitiveSelector {
    PrimitiveSelector { selector, access: ast::AccessModifier::Protected, primitive }
}
```

  and write `Object`'s entry as
  `protected("doesNotUnderstand:", Primitive::Object(ObjectPrimitive::DoesNotUnderstand))`,
  with a comment citing §5.5.

- [ ] **Step 4: Run them, and the suite.**

Run: `cargo test --workspace`
Expected: PASS, both new tests included, every recorded outcome unchanged. If
any other recorded outcome moved, stop: establish from §5.5 that the new
answer is right before touching its golden, and report it.

- [ ] **Step 5: Gates and commit.**

```bash
git add crates
git commit -m "fix: Object's doesNotUnderstand: is protected, per spec 5.5" -m "<trailer lines>"
```

---

### Task 5: Measure the step

**Files:** none committed. The throwaway sources live under
`target\bench-lookup\` (ignored by git).

- [ ] **Step 1: The seven sources.** The three fixtures
  `tests-fixtures\bench_loop.ns`, `bench_send.ns`, `bench_block.ns`, and four
  throwaway sources under `target\bench-lookup\`; write any that is missing,
  tab-indented like the fixtures.

  `bench_do.ns` (prints `275000`):

```
Newspeak3
'Bench'
class BenchDo usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	public main: p args: a = (
		| n |
		n := 0.
		[n < 5000] whileTrue: [#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum := sum + e]. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

  `bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` (each prints `20000`)
  are written out in `docs/design/execution.md`, "What spec-conformant lookup
  moved" — the versions with the renamed slots `held`, `heldU`, `heldV`; copy
  them from there verbatim. Check each source prints what it should
  (`cargo run -p ns-cli -- run target\bench-lookup\bench_do.ns`).

- [ ] **Step 2: Build the three binaries, every one with the protocol's
  flags.** In PowerShell:

```powershell
git worktree add ..\ns-next-base be3eb40
git worktree add ..\ns-next-start <START>
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --release -p ns-cli --manifest-path ..\ns-next-base\Cargo.toml
cargo build --release -p ns-cli --manifest-path ..\ns-next-start\Cargo.toml
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS
```

  Check that no binary imports `VCRUNTIME140.dll` (`dumpbin /dependents`) and
  that each prints the expected output for every source.

- [ ] **Step 3: Time them, two sittings.** Each sitting, five rounds, the
  order of the three binaries rotating from round to round:

```powershell
$bins = @(
  @{ name = "base";  path = "..\ns-next-base\target\release\ns-next.exe" },
  @{ name = "start"; path = "..\ns-next-start\target\release\ns-next.exe" },
  @{ name = "after"; path = ".\target\release\ns-next.exe" }
)
$sources = "tests-fixtures\bench_loop.ns", "tests-fixtures\bench_send.ns",
           "tests-fixtures\bench_block.ns", "target\bench-lookup\bench_do.ns",
           "target\bench-lookup\bench_new.ns", "target\bench-lookup\bench_new2.ns",
           "target\bench-lookup\bench_newarg.ns"
foreach ($round in 1..5) {
  $order = $bins[(($round - 1) % 3)..2] + $bins[0..(($round - 1) % 3)] | Select-Object -First 3
  foreach ($source in $sources) {
    foreach ($bin in $order) {
      "$round $($bin.name) $source"
      & $bin.path bench $source --iterations 30   # prints median, min, max
    }
  }
}
```

  After both sittings: `git worktree remove ..\ns-next-base`,
  `git worktree remove ..\ns-next-start`, and
  `cargo test --release -p ns-runtime --lib a_decision_stays_the_size_the_hot_path_was_measured_at`.

- [ ] **Step 4: Evaluate.** Record the five medians per source and binary and
  each group's mean, per sitting. The comparison with START is the gate: the
  step is "no slower" when, for every source, the after-group does not lie
  entirely above the start-group, "faster" where it lies entirely below. The
  comparison with `be3eb40` is reported. If any source is slower than START
  beyond noise, stop and report with both tables before anything else —
  PROGRESS.md's open finding says the send path is codegen-sensitive, so a
  slowdown is investigated (a variant measured, not assumed), not accepted.
  Put both sittings' tables and the change of each mean in the task's report;
  Task 6 carries them into the documents.

---

### Task 6: Documents

**Files:**
- Modify: `docs/design/dispatch.md`, `docs/design/core-library.md`,
  `docs/design/execution.md`, `docs/design/README.md`, `docs/PROGRESS.md`

- [ ] **Step 1: `dispatch.md`.** Add a section `## Primitives` after
  "Slot accessors", in the present tense: the builtin classes' natives are
  entries of their mixins' method tables (`MethodEntry::Primitive`), with
  `public` access except `Object`'s `doesNotUnderstand:` (§5.5); a lookup
  that reaches one answers `Lookup::Primitive(primitive, level)`; a primitive
  fails on a receiver of the wrong shape and passes its level over
  (`Mutator::settle`, `dispatch::lookup_above`); the prior art (Cog's and
  Primordial Soup's primitive methods, SOMns's dispatchables). Move the
  substance of "3a — Primitives are method-table entries" there and leave in
  its place under "Planned: inline caches" one line: 3a is built, see
  "Primitives" above. Name `builtins/primitive.rs` in the status block's code
  list, and turn the 2026-09-15 decision bullet for 3a from "Planned" to a
  decision.
- [ ] **Step 2: `core-library.md`.** The paragraph on `match selector.as_str()`
  and the platform paragraph become present tense; the "Decisions" bullet
  says natives are found as method-table entries.
- [ ] **Step 3: `execution.md`.** A section "What primitives as method-table
  entries moved" after "What spec-conformant lookup moved": the protocol
  (every binary with the pinned `RUSTFLAGS`, seven sources, five rounds,
  rotating order, two sittings), both sittings' tables against START and
  `be3eb40`, and what they show.
- [ ] **Step 4: `README.md`.** The dispatch row's "Planned next" becomes the
  inline caches, 3b then 3c; the core library's becomes the core library as
  Newspeak source.
- [ ] **Step 5: `PROGRESS.md`.** A section "M4 — Primitives as method-table
  entries" (what changed, the one behaviour change and its §5.5 citation, the
  measurement), the plan added to M4's plan list in the milestone table, the
  order-of-work item 3 updated, and "Where the next session starts" naming
  3b's plan as the next piece — written against `dispatch.md`, "3b — The
  caches", opening with the profile that sets the ceiling.
- [ ] **Step 6: Check, then commit.** Every document still lets a fresh
  session start from `docs/design/README.md` and PROGRESS.md's "Where the
  next session starts" without this conversation; nothing describes the
  history of how the code got here.

```bash
git add docs
git commit -m "docs: record primitives as method-table entries and what they moved" -m "<trailer lines>"
```
