# `Value`, `Top` and the Core Library's Shape Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the core library the shape the specification describes —
`Top`, `Value` and `Number` in the hierarchy, every class public but `Top` —
and give value objects the semantics §3.1.1 requires: no observable identity,
equality and hash derived from state.

**Architecture:** `Kernel.ns` moves to `crates/ns-runtime/core-library/` and
gains three classes: `Top` (private, empty, `Object`'s superclass), `Value`
(under `Object`) and `Number` (under `Value`, above `Integer` and `Float`).
Every value class — `Boolean`, `Number`, `String`, `Symbol`, `Character`,
`Nil` — hangs under `Value`. `Value` derives `=` from state, answers `==` as
`^self = other` and derives `hash` the same way, so a value has no observable
identity and a copy of it is equal to and hashes like its original. The
derivation runs in a Rust leaf, which declines whenever a slot's class brings
its own `=`, and the `ifFail:` block then runs the same comparison in
Newspeak, sending `=` per slot over two new reflective leaves. A class
inheriting `Value` may declare only immutable slots, and each of its slots
must hold a value object.

**Tech Stack:** Rust 1.98, edition 2021, no new dependencies. Newspeak3 for
the core library.

**Spec:** `docs/references/newspeak-spec.md` §3.1.1 ("Values: Deeply Immutable
Objects"), §3.2 (`Top`, and "Except for `Top` all classes are subclasses of
class `Object`"), §6.3.2 (slots and their accessors). Design:
`docs/design/core-library.md`, `docs/design/object-model.md`,
`docs/design/liveness-and-reflection.md` ("Deviation: no formal value
objects", which this plan narrows).

## Decisions this plan implements

All settled with the user on 2026-09-22, after reading the specification and
both reference implementations.

1. **The hierarchy follows §3.2 literally**: `Top` → `Object` → `Value` →
   the value classes. SOMns's shape, where `Value` sits beside `Object` under
   a `Thing`, was considered and rejected: §3.2 says every class but `Top` is
   a subclass of `Object`.
2. **`Value` declares `=`, `==` and `hash`, all derived from state.** A value
   object has **no observable identity**: §3.1.1 lets a value be copied
   across actor boundaries or shared as the runtime sees fit, so a copy must
   be equal to, and hash like, its original. Identity-based equality for a
   value is the mistake this avoids. `==` is `^self = other`, which satisfies
   §3.1.1's rule that an identity method on `Value` "must return the same
   results as `Value`'s equality method" by construction — one source of
   truth, no second copy of the rules.
3. **`Number` joins the hierarchy**, empty, between `Value` and
   `Integer`/`Float`: §3.1.1 names `Number` among the value classes, and it is
   where the numeric tower and `Integer>>/` will go.
4. **`Nil` is a value class.** `nil` is deeply immutable, has no slots and is
   unique, so it meets §3.1.1's second condition on its own; this also lets a
   value's slot hold `nil` without an exception in the checks.
5. **The derivation runs in a Rust leaf with a Newspeak fallback.** The leaf
   compares class and slots iteratively, with an identity memo so a cyclic
   value graph (`class Loop = Value (| public me = self. |)`) ends rather than
   hangs — an image is untrusted input. It **declines** when a slot's class
   declares its own `=`, because a leaf may not send; the `ifFail:` block then
   runs the comparison in Newspeak and sends that `=`.
6. **§3.1.1's conditions are enforced**: a class inheriting `Value` may
   declare only immutable slots (refused when the class object is created),
   and every slot of a value must hold a value object (checked as the slot is
   initialized).
7. **The builtin value classes carry their own `=` and `hash`**, by content,
   as both references do: `Integer>>hash` is `^self`, `Character>>hash` is
   `^self asInteger`, `Boolean>>hash` answers 1 or 0, and `Float`, `String`
   and `Symbol` each get a leaf. Primordial Soup states the contract this
   follows: "should be overridden if the method `=` is overridden … if `x = y`
   evaluates to true then `x hash = y hash` must evaluate to true".
8. **The library's directory is `crates/ns-runtime/core-library/`**, the file
   stays `Kernel.ns` and the module class stays `Kernel`. **Every nested class
   is `public`** — other builtin modules will need them — **except `Top`**,
   which stays private because §3.2 allows only `Object` to name it as a
   superclass. `Value` and `Number` become writable as bare names in user
   modules; `Top` does not.

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency.
- **All code, identifiers, comments, doc comments and commit messages in
  English.** Names are written out (clippy's `min_ident_chars`, threshold 2).
- **Program outcomes are pinned.** Every existing golden keeps its recorded
  `status`, `stdout` and `stderr`, under both lowerings, **except** where this
  plan names an outcome that changes as a consequence of Decision 2 or 7; each
  such change is recorded in a golden of its own, and the task that makes it
  says which existing golden may move and why. `run-corpus` keeps its report.
  A test pinning a *mechanism* may be rewritten; a test pinning an outcome may
  not be edited to pass.
- **A Rust leaf never calls back into Newspeak.** The derivation declines
  instead, and the `ifFail:` block does the sending.
- **An image is untrusted input.** No byte sequence may make the derivation
  loop forever, recurse without bound, or panic; the memo and an explicit work
  stack are what keep it iterative.
- **A wire tag is assigned once and never renumbered or reused.** The new
  builtin class tags are `Top` = 15, `Value` = 16, `Number` = 17, appended to
  `BuiltinClass::ALL` in that order. Because `ALL`'s order is the wire-tag
  order, it no longer lists a superclass before its subclasses — the
  bootstrap's fill order becomes topological instead (Task 2).
- **Compiling an already-resolved module can never raise a `NewspeakError`.**
  The new refusals happen when a class object is created and when a slot is
  initialized, both at run time, and are ordinary Newspeak errors. A defect in
  `Kernel.ns` itself is a panic at bootstrap, as it is today.
- **The gates, for every task that changes Rust or Newspeak:** `cargo fmt
  --all`; `cargo clippy --workspace --all-targets` warning-free; `cargo test
  --workspace`; the same under `NS_NEXT_STRESS_MOVES=1`,
  `NS_NEXT_STRESS_CACHES=1` and `NS_NEXT_STRESS_COLLECT=1`;
  `cargo run -p ns-cli -- parse-corpus corpus/modules` → "parsed 210, failed 0,
  expected-fail 1, stale 0"; `cargo run -p ns-cli -- run-corpus corpus/modules`
  → "ran 6, failed 0, skipped 205", also under each stress mode.
- **Commits are conventional and end with the trailer naming the model that
  wrote them**, then:

  ```
  Claude-Session: https://claude.ai/code/session_01XZBjhUJaKHo3p6VsiUwHKx
  ```

## How the pieces fit

### The hierarchy

```
Top  (private, empty)
└── Object
    ├── Value
    │   ├── Boolean ── True, False
    │   ├── Number ── Integer, Float
    │   ├── String
    │   ├── Symbol
    │   ├── Character
    │   └── Nil
    ├── Array
    ├── OrderedCollection
    ├── Block
    └── Platform
```

`Array`, `OrderedCollection`, `Block` and `Platform` stay under `Object`: they
are mutable or hold behaviour, and §3.1.1 does not name them.

### What `Value` declares, and why each form

```newspeak
public class Value = Object new () (
    public = other = ( ^vmMirror value: self sameAs: other ifFail: [^self valueEquals: other] )
    public == other = ( ^self = other )
    public hash = ( ^vmMirror valueHash: self ifFail: [^self valueHashOfSlots] )
    …private helpers…
)
```

- `=` is the derivation. Its leaf declines when it meets a slot whose class
  declares its own `=`; the fallback then compares in Newspeak.
- `==` is a plain send, not a leaf: agreement with `=` is then true by
  construction for every subclass, including `String`, `Integer`, `Float`,
  `Symbol` and `Character`, which each bring their own `=`. A second leaf with
  its own rules would be the second copy `CLAUDE.md` forbids.
- `hash` mirrors `=`: same derivation, same fallback.

### What the leaf does

`value:sameAs:ifFail:` answers, for a subject that is a value:

1. A different class than the argument's → `false`.
2. An immediate (`nil`, a Boolean, an Integer, a Float) → the comparison that
   class's own `=` primitive performs, by **calling the same Rust helper**,
   never a second implementation of the rule.
3. A `String`, `Symbol` or `Character` payload → likewise, through the helper
   `String>>=`, `Symbol>>=` and `Character>>=` already call.
4. An instance whose class's `=` resolves to `Value`'s own method → its slots,
   pairwise, iteratively over an explicit work stack, with a memo of the pairs
   already being compared, so a cycle answers `true` rather than looping.
5. Anything else — a slot whose class declares its own `=`, an `Array`, a
   `Block`, a class object — → **declines** (`None`), and the `ifFail:` block
   runs.

`valueHash:ifFail:` follows the same five cases, combining slot hashes.

### What the fallback does

Two new reflective leaves make the Newspeak fallback possible, and they are
the first two operations the later mirror work will need anyway:

- `valueSlotCount:ifFail:` — how many slots the receiver's class lays out.
- `value:slotAt:ifFail:` — the slot at a 1-based index.

`Value`'s private `valueEquals:` compares the classes, then the slot counts,
then each slot with `=`, so an overridden `=` is sent. `valueHashOfSlots`
sums the slots' hashes with the slot count; the sum cannot overflow, because a
hash is below 2³² and a class lays out at most 2¹⁶ slots.

### What the enforcement costs

Refusing a mutable slot happens once per class object, where the application
is built. Checking that a slot holds a value happens per `InitializeSlot` of a
value class's initializer; the check reads a flag computed once per
application, so a non-value class pays one predictable branch.

---

## File Structure

- `crates/ns-runtime/kernel/` → **rename to** `crates/ns-runtime/core-library/`
  (`git mv`, so the file's history follows); `Kernel.ns` keeps its name.
- `crates/ns-runtime/src/interpreter/kernel.rs` — the `include_str!` path, the
  topological fill order, the kernel-shape checks, `kernel_class_name` for the
  three new classes.
- `crates/ns-runtime/src/types.rs` — `BuiltinClass::{Top, Value, Number}`.
- `crates/ns-runtime/src/builtins/primitive.rs` — the new `MIRROR` rows and
  the `ValuePrimitive` enumeration.
- `crates/ns-runtime/src/builtins/value.rs` — **create**: the derivation, its
  memo and work stack, the reflective leaves.
- `crates/ns-runtime/src/builtins/{numbers,text}.rs` — the content hashes, and
  the comparison helpers the derivation calls.
- `crates/ns-runtime/src/interpreter/{mod,instantiate}.rs` — `apply_mixin` or
  `new_class_object` refusing a mutable slot under `Value`; the value flag on
  the application.
- `crates/ns-runtime/src/vm.rs` — `InitializeSlot`'s check, decided in
  `interpreter/` as every error is.
- `crates/ns-runtime/src/image/{encode,decode}.rs` — tags 15–17.
- `crates/ns-cli/src/commands/build.rs` — nothing, unless the walk names the
  kernel's classes.
- `goldens/kernel/` — the new recorded outcomes.
- Docs (Task 8): `core-library.md`, `object-model.md`,
  `liveness-and-reflection.md`, `dispatch.md`, `image-format.md`,
  `docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`.

---

### Task 1: The directory, and every class public but `Top`

**Files:**
- Rename: `crates/ns-runtime/kernel/` → `crates/ns-runtime/core-library/`
- Modify: `crates/ns-runtime/core-library/Kernel.ns`,
  `crates/ns-runtime/src/interpreter/kernel.rs`, `.gitattributes`
- Test: the suite; `interpreter/kernel.rs`'s own tests

**Interfaces:**
- Produces: the new path `KERNEL_SOURCE` reads, and public nested classes.

- [ ] **Step 1: Move the directory**

```bash
git mv crates/ns-runtime/kernel crates/ns-runtime/core-library
```

Update `include_str!` in `interpreter/kernel.rs` and the `-text` entry in
`.gitattributes`. Search the whole repository for the old path
(`grep -rn "ns-runtime/kernel" --include=* .`) and update every hit, including
documentation.

- [ ] **Step 2: Make every nested class public**

In `Kernel.ns`, every `class X = …` becomes `public class X = …`. There is no
`Top` yet; after Task 2 it is the one exception. §6.3.1 makes an undeclared
member protected, which is why they must say `public` to be reachable from
another module.

- [ ] **Step 3: Pin it**

In `interpreter/kernel.rs`'s tests, extend the test that walks the kernel's
classes (`every_kernel_method_is_a_primitive_method_named_after_its_row`'s
neighbour, or a new one) to require `mixin.access == AccessModifier::Public`
for every nested class of the kernel's root mixin. Match the spelling
`Mixin::access` actually has.

- [ ] **Step 4: Run the gates**

Expected: green, every golden unchanged — accessibility of a kernel class is
not observable yet, because nothing outside the kernel reaches its members.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "refactor: move the core library to core-library/ and publish its classes"
```

---

### Task 2: `Top`, `Value`, `Number`, and a topological bootstrap

No behaviour changes here: the classes are empty, and a deeper chain answers
every send exactly as before.

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Modify: `crates/ns-runtime/src/types.rs`,
  `crates/ns-runtime/src/interpreter/kernel.rs`,
  `crates/ns-runtime/src/interpreter/instantiate.rs` (`builtin_kind_from_name`),
  `crates/ns-runtime/src/image/encode.rs`, `decode.rs`
- Test: `interpreter/kernel.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Consumes: Task 1's paths.
- Produces: `BuiltinClass::{Top, Value, Number}` (tags 15, 16, 17); the
  topological fill; `Value` and `Number` as bare names.

- [ ] **Step 1: Write the failing tests**

In `interpreter/kernel.rs`'s tests:

```rust
    /// §3.2: `Top` is the empty class, and only `Object` may have it as its
    /// superclass. §3.1.1's value classes hang under `Value`.
    #[test]
    fn the_hierarchy_follows_the_specification() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let expected = [
                (BuiltinClass::Object, Some(BuiltinClass::Top)),
                (BuiltinClass::Value, Some(BuiltinClass::Object)),
                (BuiltinClass::Boolean, Some(BuiltinClass::Value)),
                (BuiltinClass::True, Some(BuiltinClass::Boolean)),
                (BuiltinClass::False, Some(BuiltinClass::Boolean)),
                (BuiltinClass::Number, Some(BuiltinClass::Value)),
                (BuiltinClass::Integer, Some(BuiltinClass::Number)),
                (BuiltinClass::Float, Some(BuiltinClass::Number)),
                (BuiltinClass::String, Some(BuiltinClass::Value)),
                (BuiltinClass::Symbol, Some(BuiltinClass::Value)),
                (BuiltinClass::Character, Some(BuiltinClass::Value)),
                (BuiltinClass::Nil, Some(BuiltinClass::Value)),
                (BuiltinClass::Array, Some(BuiltinClass::Object)),
                (BuiltinClass::OrderedCollection, Some(BuiltinClass::Object)),
                (BuiltinClass::Block, Some(BuiltinClass::Object)),
                (BuiltinClass::Platform, Some(BuiltinClass::Object)),
                (BuiltinClass::Top, None),
            ];
            for (kind, superclass) in expected {
                let actual = mutator
                    .heap()
                    .class_superclass(mutator.builtin_class(kind))
                    .map(|class| mutator.heap().class_builtin(class));
                match superclass {
                    Some(expected) => assert!(
                        matches!(actual, Some(Some(found)) if found == expected),
                        "{kind:?}'s superclass must be {expected:?}"
                    ),
                    None => assert!(actual.is_none(), "{kind:?} has no superclass"),
                }
            }
        });
    }

    /// `Top` is private (§3.2: only `Object` may name it); every other
    /// kernel class is public.
    #[test]
    fn only_top_is_private() { /* walk the root mixin's nested list */ }
```

and in the file that pins the builtin class tags, add `Top => 15`,
`Value => 16`, `Number => 17`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --lib interpreter::kernel`

- [ ] **Step 3: Declare the three classes**

In `Kernel.ns`:

```newspeak
    (* §3.2: "A class is either the empty class `Top` or the application of a
       mixin to another class". Only `Object` may name it, so it is private. *)
    private class Top = () (
    )
    public class Object = Top new () ( … unchanged … )
    (* §3.1.1: a value object's class inherits from `Value`. What `Value`
       declares comes in Task 5; the hierarchy comes first so that each step
       is a change of one kind. *)
    public class Value = Object new () (
    )
    public class Number = Value new () (
    )
```

and rehang `Boolean`, `String`, `Symbol`, `Character`, `Nil` onto `Value`,
`Integer` and `Float` onto `Number`. `True` and `False` keep `Boolean`.

- [ ] **Step 4: Append the three builtin classes**

`BuiltinClass` gains `Top`, `Value`, `Number` **at the end** of the enumeration
and of `ALL`; `builtin_class_tag`/`builtin_class_from_tag` map them to 15, 16,
17. `kernel_class_name` names them. `builtin_kind_from_name`
(`interpreter/instantiate.rs`) gains `"Value"` and `"Number"` — a user module
may name them — but **not** `"Top"`: §3.2 allows only `Object` to name it, and
nothing else may reach it.

- [ ] **Step 5: Fill topologically**

`ALL`'s order is the wire-tag order and must not change, so it no longer lists
a superclass before its subclasses. In `interpreter/kernel.rs`, replace the
"fill in `ALL` order" loop with one that fills in **superclass-first** order:
compute each kernel class's depth by following its declared superclass name
(the walk is bounded by the number of kernel classes, and a chain that does not
reach `Top` or that repeats a class is a panic naming the cycle), then fill in
order of increasing depth. `check_kernel_shape`'s "names a class that comes
earlier in `BuiltinClass::ALL`" check becomes "names a kernel class, and the
chain reaches `Top` without repeating".

- [ ] **Step 6: Run the gates**

Expected: green, every golden unchanged. Two extra levels appear on the
lookup path of a miss (`3 foo` now walks `Integer`, `Number`, `Value`,
`Object`), which changes no answer.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat: give the core library Top, Value and Number"
```

---

### Task 3: The builtin value classes' own `hash`

This task changes outcomes: `3 hash` answers 3 where it answered 0, and a
string's hash becomes content-based. No existing golden records those (only
`goldens/language/identity_hash`, whose class is an ordinary `Object`
subclass, which is unaffected) — confirm that before recording anything.

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (three new rows),
  `numbers.rs`, `text.rs`
- Create: `goldens/kernel/value_hashes_follow_value_equality.ns` and `.toml`

**Interfaces:**
- Consumes: Task 2's hierarchy.
- Produces: `hash` on `Integer`, `Float`, `String`, `Symbol`, `Character`,
  `Boolean`; the rows `floatHash:ifFail:`, `stringHash:ifFail:`,
  `symbolHash:ifFail:` (reported selector `hash` for each).

- [ ] **Step 1: Add the three leaves**

`FloatPrimitive::Hash`, `StringPrimitive::Hash`, `SymbolPrimitive::Hash`, each
answering a `SmallInteger`: a float by the bits of its value reduced into a
non-negative integer, a string by a content hash (use the crate's existing
hasher, `crate::hash`, so the repository keeps one hash function), a symbol by
its interned id — equal symbols are one object, so the id is stable and agrees
with `Symbol>>=`. Add their `MIRROR` rows with `reported: "hash"`.

- [ ] **Step 2: Declare the methods**

```newspeak
    (* Primordial Soup states the contract: "should be overridden if the
       method `=` is overridden … if `x = y` evaluates to true then
       `x hash = y hash` must evaluate to true." *)
    public hash = ( ^self )                                  (* Integer *)
    public hash = ( ^self asInteger )                        (* Character *)
    public hash = ( ^self ifTrue: [1] ifFalse: [0] )         (* Boolean *)
    public hash = ( ^vmMirror floatHash: self ifFail: [^super hash] )
    public hash = ( ^vmMirror stringHash: self ifFail: [^super hash] )
    public hash = ( ^vmMirror symbolHash: self ifFail: [^super hash] )
```

`Integer`'s and `Character`'s are ordinary methods, not primitive methods —
that is correct and costs an activation only where a program hashes.

- [ ] **Step 3: Record the golden**

A module that prints, for each value class, whether two equal values hash
alike and what a few hashes are — run it under both lowerings, confirm they
agree, and record `status`, `stdout` and `stderr`. This is a **new** outcome,
established from the contract in Step 2's comment, not copied from an earlier
commit.

- [ ] **Step 4: Run the gates**

Expected: green; no existing golden moves. If one does, stop: name it in the
report and do not re-record it without establishing the new output is right.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat: hash the builtin value classes by their value"
```

---

### Task 4: The derivation in Rust

**Files:**
- Create: `crates/ns-runtime/src/builtins/value.rs`
- Modify: `crates/ns-runtime/src/builtins/{mod,primitive}.rs`
- Test: `builtins/value.rs`'s own unit tests

**Interfaces:**
- Consumes: Tasks 2 and 3.
- Produces: `ValuePrimitive::{SameAs, Hash, SlotCount, SlotAt}` and the
  `MIRROR` rows `value:sameAs:ifFail:` (reported `=`), `valueHash:ifFail:`
  (reported `hash`), `valueSlotCount:ifFail:` (reported `slotCount`),
  `value:slotAt:ifFail:` (reported `slotAt:`).

- [ ] **Step 1: Write the failing tests**

Unit tests in `builtins/value.rs`, driven through `Interpreter::send` on the
mirror (copy the shape of `interpreter/mirror.rs`'s tests): two equal strings
compare equal and hash alike; an integer and a float of the same magnitude
compare unequal through the leaf (their classes differ) though `Float>>=`
widens; two instances of the same user value class with equal slots compare
equal; with unequal slots, unequal; an instance of a different class compares
unequal; a cyclic value (`| public me = self. |`) compares equal to itself and
terminates; a slot holding an `Array` makes the leaf decline; a slot whose
class declares its own `=` makes the leaf decline.

- [ ] **Step 2: Run them to verify they fail**

- [ ] **Step 3: Implement the derivation**

In `builtins/value.rs`, one iterative comparison over an explicit work stack
of pairs, with a memo (`HashSet` of the pairs in progress, keyed by the two
objects' indices) so a cycle answers `true`:

- classes differ → `Some(Ok(false))`;
- an immediate, a `String`, a `Symbol` or a `Character` → the **same helper**
  the class's own `=` primitive calls (extract one from `numbers.rs` /
  `text.rs` rather than writing the rule again);
- an instance whose class's `=` resolves to `Value`'s own method — compare
  `Rc::ptr_eq` against the method the bootstrap recorded — → push its slots;
- anything else → `None`, the decline.

`valueHash:` walks the same shapes, summing each slot's hash with the slot
count; a hash is below 2³² and a class lays out at most 2¹⁶ slots, so the sum
stays inside an `i64`. `SlotCount` and `SlotAt` answer the receiver's own slot
count and its slot at a 1-based index, refusing an out-of-range index as an
ordinary error, exactly as `Array>>at:` does.

`Mutator` gains the handle to the kernel's `Value>>=` method, recorded by the
bootstrap's kernel phase, since the derivation needs it to tell an inherited
`=` from an overridden one. It is an `Rc<MixinMethod>`, not a heap handle, so
it needs no root.

- [ ] **Step 4: Run the tests and the gates**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat: derive a value's equality and hash from its state"
```

---

### Task 5: `Value`'s methods, and the Newspeak fallback

This task changes outcomes, by Decision 2: `'a' == 'a'`, `$a == $a` and
`1.0 == 1` answer `true` where they answered `false`. Search the goldens and
the tests for `==` on a value before you start (`grep -rn "==" goldens --include=*.ns`)
and name every one that moves in the commit message.

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Create: `goldens/kernel/a_value_has_no_identity.ns` and `.toml`,
  `goldens/kernel/a_user_value_class_compares_by_its_slots.ns` and `.toml`
- Test: the suite

**Interfaces:**
- Consumes: Task 4's rows.
- Produces: `Value`'s `=`, `==`, `hash` and its private helpers.

- [ ] **Step 1: Declare the methods**

```newspeak
    public class Value = Object new () (
        (* §3.1.1: a value object is globally unique, in the sense that no
           other object is equal to it -- so equality is derived from state,
           and a value has no observable identity. The leaf declines when a
           slot's class brings its own `=`, because a leaf may not send;
           `valueEquals:` then sends it. *)
        public = other = ( ^vmMirror value: self sameAs: other ifFail: [^self valueEquals: other] )
        (* §3.1.1: where `Value` declares an identity method, "that method
           must return the same results as `Value`'s equality method". A send
           of `=` makes that true by construction, for every subclass. *)
        public == other = ( ^self = other )
        public hash = ( ^vmMirror valueHash: self ifFail: [^self valueHashOfSlots] )
        private valueEquals: other = (
            | index count |
            (self class == other class) ifFalse: [^false].
            count:: self valueSlotCount.
            (count = other valueSlotCount) ifFalse: [^false].
            index:: 1.
            [index <= count] whileTrue: [
                ((self valueSlotAt: index) = (other valueSlotAt: index)) ifFalse: [^false].
                index:: index + 1].
            ^true
        )
        private valueHashOfSlots = (
            | index count result |
            count:: self valueSlotCount.
            result:: count.
            index:: 1.
            [index <= count] whileTrue: [
                result:: result + (self valueSlotAt: index) hash.
                index:: index + 1].
            ^result
        )
        private valueSlotCount = ( ^vmMirror valueSlotCount: self ifFail: [^0] )
        private valueSlotAt: index = ( ^vmMirror value: self slotAt: index ifFail: [^nil] )
    )
```

`self class == other class` compares two class objects, which are ordinary
objects under `Object`, so `==` there is still identity — which is what a
class comparison must be.

- [ ] **Step 2: Record the two goldens**

`a_value_has_no_identity.ns` prints `'a' == 'a'`, `$a == $a`, `1.0 == 1`,
`1 == 1.0`, `3 == 3`, `#a == #a`, `nil == nil`, and the same expressions with
`=`, so the agreement is visible line by line. `1 == 1.0` stays `false`,
because `Integer>>=` takes only an Integer — the agreement rule is about `==`
answering what `=` answers, and it does.

`a_user_value_class_compares_by_its_slots.ns` declares
`class Point x: anX y: aY = Value (| public x = anX. public y = aY. |) ()`,
prints that two points with equal coordinates are `=` and `==` and hash alike,
that points with different coordinates are not, and that a point is not equal
to an instance of another value class with the same slots.

Run both under both lowerings, confirm they agree, and record what they print.

- [ ] **Step 3: Run the gates**

Expected: green except for goldens that record `==` on a value. For each, show
the old and the new output, state which line moved and why (Decision 2), and
re-record it in this commit — this is the one task in this plan allowed to.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat: give a value equality and hash derived from its state"
```

---

### Task 6: Enforce §3.1.1's conditions

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/{mod,instantiate}.rs`,
  `crates/ns-runtime/src/vm.rs` (the check's call, decided in `interpreter/`)
- Modify: `crates/ns-runtime/src/mixin.rs` (a flag on the application)
- Create: `goldens/kernel/a_value_class_refuses_a_mutable_slot.ns` and `.toml`,
  `goldens/kernel/a_values_slot_must_hold_a_value.ns` and `.toml`

**Interfaces:**
- Consumes: Tasks 2 and 5.
- Produces: `MixinApplication::is_value` (or the spelling that fits), the two
  refusals.

- [ ] **Step 1: Refuse a mutable slot**

Where an application is built (`Mutator::apply_mixin`), compute whether the
chain reaches `BuiltinClass::Value`; when it does and the mixin declares a
mutable slot, the class object's creation raises an ordinary Newspeak error
naming the class and the slot: a value's slots are immutable (§3.1.1). Since
`apply_mixin` has no error channel today, raise it where the class object is
created (`load_root_class`, `interpreter::nested`, `image::decode`'s rebuild)
— pick the one place all three reach, and say in the report which.

- [ ] **Step 2: Refuse a slot that is not a value**

`InitializeSlot` writes a slot during an initializer. When the receiver's
class is a value class, the value written must be a value object: an
immediate, a `String`, a `Symbol`, a `Character`, or an instance whose class
inherits `Value`. Otherwise the initializer raises an ordinary Newspeak error
naming the class and the slot. The decision and the text live in
`interpreter/`, as every error does; `vm.rs` only calls it.

- [ ] **Step 3: Record the two goldens**

One module whose value class declares `| public x ::= 1. |`, one whose value
class stores an `Array` in a slot. Each ends in its error; record `status`,
`stdout` and `stderr` under both lowerings.

- [ ] **Step 4: Run the gates**

Expected: green. `Kernel`'s own classes declare no slots, and the module
classes under `corpus/runnable.toml` inherit from `Object`, so nothing there
is a value class.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat: enforce that a value's slots are immutable and hold values"
```

---

### Task 7: Measure

**Files:**
- Modify: `docs/design/execution.md`, `docs/PROGRESS.md`

- [ ] **Step 1: Record the expectation**

Two extra levels appear on a failed lookup and one extra branch on
`InitializeSlot`; no send that succeeds walks further than before, and no
benchmark hashes or compares values in its loop. So: level on all eight
sources. Say so before measuring.

- [ ] **Step 2: Measure**

The protocol of `docs/design/execution.md`, "What removing the re-entries
moved": the eight sources, the RUSTFLAGS alignment pin, release builds, two
sittings, interleaved, pooled. Baseline: this branch's head before Task 1
(name the commit). `404ece6` need not be rebuilt — the section written for the
core library's move already carries it.

- [ ] **Step 3: Write it up**

Extend `execution.md`'s section on the core library's move with what this
shape change cost, in the same format, and update `PROGRESS.md`'s entry.

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: record what Top, Value and Number cost"
```

---

### Task 8: Documentation

Markdown and comments only; no gate run beyond `cargo fmt --all --check` and
clippy if a comment changed.

- [ ] **Step 1: `core-library.md`**

The hierarchy diagram and why it follows §3.2 rather than SOMns's shape; what
`Value` declares and why `==` is a send; the derivation, its decline rule and
its fallback; the two new reflective leaves and that the mirror work will
reuse them; the enforcement and what is still unchecked; the directory's new
name; that every class but `Top` is public and why.

- [ ] **Step 2: `object-model.md` and `liveness-and-reflection.md`**

`object-model.md`: `Top` as the empty class, the value classes' place, and
that `Number` is where the numeric tower will go. `liveness-and-reflection.md`:
narrow "Deviation: no formal value objects" to what is still deviating — the
actor boundary itself — since the class, the derived equality and the
conditions now exist.

- [ ] **Step 3: `dispatch.md`, `image-format.md`, `README.md`, `PROGRESS.md`, `CLAUDE.md`**

`dispatch.md`: the new rows, and that the derivation declines rather than
sends. `image-format.md`: tags 15–17, and that `Top` is not nameable from an
image any more than from source. `README.md`: the map row. `PROGRESS.md`: the
chronology entry and "Where the next session starts". `CLAUDE.md`: the
architecture paragraph's path, and — if the capability invariant's list
changes — that list.

- [ ] **Step 4: Fresh-session check and commit**

Read `CLAUDE.md`, `docs/design/README.md` and `PROGRESS.md` as a session that
knows nothing else would, fix what it could not act on, then:

```bash
git add -A
git commit -m "docs: describe Top, Value and the derived value semantics"
```

---

## Self-Review

**Spec coverage.** §3.2's `Top` and "every class but `Top` is a subclass of
`Object`" — Task 2. §3.1.1's value classes — Task 2 (the hierarchy), Task 3
(their hashes), Task 5 (the derivation), Task 6 (the conditions). §3.1.1's
identity rule for `Value` — Task 5's `==`. §6.3.2's immutable slots — Task 6.
The specification's `Number` — Task 2, empty, with the numeric tower and
`Integer>>/` still open and recorded in `core-library.md`'s gap section.

**Not covered here, deliberately:** the actor boundary itself, which is what
copying a value across actors needs and what
`liveness-and-reflection.md` keeps as the remaining deviation; `String`
canonicalization, which is not required for the derived equality; the numeric
tower.

**Placeholders.** Task 4 Step 3 and Task 6 Step 1 leave the exact call site to
the implementer, naming the candidates and requiring the report to say which
was chosen: three class-creation paths exist and the plan does not presume
which one is the single seam. Both are stated as contracts, with tests.

**Type consistency.** `ValuePrimitive::{SameAs, Hash, SlotCount, SlotAt}` is
declared in Task 4 and used by the `MIRROR` rows of the same task; `Value`'s
methods in Task 5 send exactly those four rows' selectors, with the parameter
order the rows declare. `BuiltinClass::{Top, Value, Number}` is declared in
Task 2 and used in Tasks 4 and 6.
