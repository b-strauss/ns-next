# Flat Metaclasses, the Primary Factory as a Method, and Three Cache Refinements — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a class side stop being inherited (§3.2, §6), make the primary
factory an ordinary entry of the metaclass mixin's method table (§6.3) so that
the three factory special cases can go with nothing in their place, refuse a
class side that declares the factory's selector, and land the three cache
refinements the user decided on 2026-09-17: the lexical global-cache key
includes N's application, call-site numbers are no longer serialized, and the
global cache becomes two-slot least-recently-used groups.

**Architecture:** The five changes are one step because the first two are one
question. §3.2 makes every metaclass a direct subclass of `Class`, never of the
superclass's metaclass, so a class side is not inherited; ns-next chains
metaclasses Smalltalk-style, and that is the only reason an inherited factory
is reachable and the reason the factory needs re-checks at all. With flat
metaclasses a factory entry sits on the receiver's own metaclass or nowhere,
which the class-side guard — that metaclass's dispatch id — determines, so the
entry can be a `MethodEntry` like any other and `act` needs no re-check. The
three cache refinements are independent of that and of each other; they are in
this step because they were decided together and are measured together.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** `docs/design/dispatch.md`, "Decisions" → "Decided by the user
(2026-09-17, extended 2026-09-18), to be built before 3c" (the authority for
this plan), with `docs/design/object-model.md` ("Class sides and metaclasses"),
`docs/design/image-format.md` ("The instruction encoding", "Verification"),
`docs/design/execution.md` (the measurement protocol in "What removing the
re-entries moved") and `docs/references/newspeak-spec.md` §3.2, §5.10, §6,
§6.1, §6.3 — the transcription is how to find the passages; the PDF is what a
citation names.

## Global Constraints

- Work on a branch `feature/flat-metaclasses` created from `master`, on the
  checked-out tree — no `git worktree` for the work itself (throwaway
  worktrees for the measurement's before-binaries are fine and are removed
  afterwards). This plan is the branch's first commit.
- Before Task 1, record `git rev-parse HEAD` of `master` as **START**
  (`0fa191f` when the plan was written; its code is 3b's head). Every
  measurement compares against START and against `d804eaf`.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or
  two characters, no truncated word, in code, tests and commit messages alike.
  Clippy's `min_ident_chars` refuses the short ones.
- No dispatch logic in `vm.rs` or `compiler/`. Everything this plan adds to a
  send's decision lives in `interpreter/`.
- **Outcomes are pinned, not re-derived.** Two recorded outcomes change in this
  step, both in Task 1, and both because today's answer is non-conforming:
  `tests-fixtures/class_side.ns` and
  `tests-fixtures/cache_super_send_reaches_the_factory_check.ns`. Every other
  `tests-fixtures/*.ns`, every golden in `crates/ns-cli/tests/program_outcomes.rs`
  and every `run-corpus` row keeps its recorded answer. A changed golden needs
  the specification passage that makes the new answer right, written into the
  commit message.
- **Never `{:?}` a `Value`**, never `.unwrap()`/`.expect()` an
  `EvaluationResult` or `HostResult`; tests use a local `expect_ok` as the
  neighbouring test files do.
- An image is untrusted input: no byte sequence may panic, in a debug build
  either. Task 5 changes the decoder; every count and number it reads is
  bounded before it indexes or allocates.
- A wire tag is assigned once and never reused. Task 5 retires the four send
  tags 21–24 and takes 25–28; the next free number becomes 29.
  `crates/ns-runtime/tests/opcode_tags.rs` pins every value.
- Code that implements a spec rule cites it (`newspeak-spec.pdf` §N.N in the
  doc comment), the house style of `dispatch.rs` and `interpreter/`.
- Comments explain the decision, not the mechanics.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, and `cargo test --workspace`. Before the last
  commit of every task also run, in Git Bash:
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about three minutes),
  `NS_NEXT_STRESS_CACHES=1 cargo test --workspace` (about one minute),
  `cargo run -p ns-cli -- parse-corpus corpus/modules` (must report
  `parsed 210, failed 0, expected-fail 1, stale 0`),
  `cargo run -p ns-cli -- run-corpus corpus/modules`,
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules` and
  `NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (all three must report `ran 6, failed 0, skipped 205`).
- Conventional commits (`test:`, `refactor:`, `feat:`, `fix:`, `perf:`,
  `docs:`). **Every commit message ends with two trailer lines**, the first
  naming the model that actually wrote the commit's code (a subagent names its
  own model, not the controller's):

  ```
  Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01G9fxg4LjiKUpza7ZBRtjfe
  ```

- Never override the git author or committer identity.
- Not in scope: 3c (the dispatch id in the object header); the rest of §3.2 —
  the reified instance-of relation and the `Class`, `Behavior` and `Metaclass`
  kernel classes — which is scheduled for step 6, the core library in Newspeak;
  closure conversion; the collector.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `crates/ns-runtime/src/interpreter/mod.rs` | `metaclass_of` stops chaining metaclasses (Task 1) |
| `crates/ns-runtime/src/interpreter/resolve.rs` | the factory check leaves `resolve_public` and `resolve_super`; `Resolved::Factory` gains the level it instantiates (Task 2) |
| `crates/ns-runtime/src/interpreter/instantiate.rs` | `try_factory_send` shrinks to `act`'s action; the builtin branch moves into it (Task 2) |
| `crates/ns-runtime/src/mixin.rs` | `MethodEntry::Factory`, built onto the metaclass mixin (Task 2) |
| `crates/ns-runtime/src/resolver/classes.rs` | the class side may not declare the factory's selector (Task 3) |
| `crates/ns-runtime/src/interpreter/inline_cache.rs` | the lexical key (Task 4), the two-slot LRU groups (Task 6) |
| `crates/ns-runtime/src/image/opcodes.rs` | the send tags retire (Task 5) |
| `crates/ns-runtime/src/image/encode.rs`, `decode.rs` | call-site numbers stop being written and are numbered on decode (Task 5) |
| `tests-fixtures/class_side.ns`, `cache_super_send_reaches_the_factory_check.ns` | re-cut to pin the conforming rule (Task 1) |
| `docs/design/dispatch.md`, `object-model.md`, `README.md`, `docs/PROGRESS.md` | the record of what was built (Task 7) |

---

### Task 1: Metaclasses are flat

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`metaclass_of`, and
  `empty_class_side_mixin`, which loses its only caller)
- Modify: `tests-fixtures/class_side.ns`, `tests-fixtures/class_side.expected`
- Modify: `tests-fixtures/cache_super_send_reaches_the_factory_check.ns` and
  its `.expected`
- Test: `crates/ns-runtime/tests/mixin.rs` (a new test beside the existing
  class-side tests)

**Interfaces:**
- Consumes: `Heap::class_metaclass`, `Heap::set_class_metaclass`,
  `Mutator::apply_mixin`, `Mutator::new_class_object` — all unchanged.
- Produces: `Mutator::metaclass_of(ClassReference<'gc>) -> Option<ClassReference<'gc>>`,
  same signature, answering a metaclass whose superclass is `Object`'s class
  rather than the superclass's metaclass. Task 2 relies on a metaclass chain
  that is exactly one application deep.

- [ ] **Step 1: Write the failing test**

In `crates/ns-runtime/tests/mixin.rs`:

```rust
#[test]
fn a_class_side_is_not_inherited() {
    // newspeak-spec.pdf §3.2: "All metaclasses are instances of class
    // Metaclass, and are direct subclasses of Class. Metaclasses may not be
    // subclassed." A subclass therefore answers a class-side message only if
    // its own class side declares it.
    let source = indoc! {"
        Newspeak3
        'Test'
        class FlatMetaclass usingPlatform: platform = Object new (|
            transcript = platform transcript.
        |) (
            class Base = Object new () (
            ) : (
                public describe = ( ^'base class side' )
            )
            class Derived = Base new () (
            )
            public main: ignored args: arguments = (
                transcript show: Derived describe printString; newLine.
            )
        ) : ()
    "};
    let outcome = run_module_source(source, "main:args:");
    assert!(
        outcome.stderr.contains("does not understand"),
        "a class side must not be inherited, stderr was: {}",
        outcome.stderr
    );
}
```

Use the module-running helper the neighbouring tests in this file use; if
`mixin.rs` has none, copy the one from `crates/ns-runtime/tests/nested_classes.rs`
verbatim rather than inventing a second shape.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test mixin a_class_side_is_not_inherited -- --nocapture`
Expected: FAIL — today `Derived describe` answers `'base class side'`, so the
assertion on stderr does not hold.

- [ ] **Step 3: Make the metaclass flat**

In `crates/ns-runtime/src/interpreter/mod.rs`, replace the body of
`metaclass_of` with a version that builds at most one metaclass, for the class
object's own class side, and never walks the superclass chain:

```rust
    /// The class object's metaclass, built on first use.
    ///
    /// `newspeak-spec.pdf` §3.2: "All metaclasses are instances of class
    /// Metaclass, and are direct subclasses of Class. Metaclasses may not be
    /// subclassed. [...] The metaclass hierarchy in Newspeak is deliberately
    /// kept very flat." §6 gives the construction: the class is an instance
    /// of "a metaclass that is the application of the metaclass mixin defined
    /// by the class' class side to the class Class".
    ///
    /// So a class side is *not* inherited: a class object with no class side
    /// of its own has no metaclass at all, and its class-side sends fall
    /// straight through to its own behaviour. ns-next reifies no `Class`, so
    /// the application has no superclass application and the fall-through is
    /// what a class-side chain already ends in
    /// (`docs/design/object-model.md`, "Class sides and metaclasses").
    ///
    /// No walk, no stack: the chain this used to climb was the reason the
    /// function was iterative.
    pub fn metaclass_of(&mut self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        if !self.heap.is_class(class) {
            return None;
        }
        if let Some(cached) = self.heap.class_metaclass(class) {
            return Some(cached);
        }
        let application = self.heap.class_application(class)?;
        let class_side_mixin = application.mixin.class_side.clone()?;
        let enclosing = self.heap.class_enclosing(class);
        let declaring_class = self.heap.class_declaring(class);
        let metaclass_application = self.apply_mixin(&class_side_mixin, None);
        // §6.3.1: a class side is written inside the same class declaration
        // its instance side is, so a class-side method's `outer` must reach
        // exactly the objects an instance-side method's does. The metaclass
        // borrows the class's own place in the declaration chain.
        let metaclass =
            self.new_class_object(metaclass_application, enclosing, declaring_class, None);
        if !self.heap.set_class_metaclass(class, metaclass) {
            unreachable!("is_class was checked above");
        }
        Some(metaclass)
    }
```

Then delete `empty_class_side_mixin` and its call site: the empty stand-in
existed only to relay an ancestor's metaclass through a level that declares no
class side, and nothing is relayed any more. Clippy's dead-code lint will point
at anything else the walk left behind.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ns-runtime --test mixin a_class_side_is_not_inherited -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Re-cut `class_side.ns`**

`tests-fixtures/class_side.ns` pins the inheritance today: `SpecialWidget`
declares no class side and answers `Widget`'s `describe`. Re-cut it so that it
pins the conforming rule — that the subclass does *not* answer, and that an
overriding class side still does:

```newspeak
Newspeak3
'Test'
class ClassSide usingPlatform: platform = Object new (| transcript ::= platform transcript. |) (
	public class Widget new = Object new () (
	) : (
		public describe = ( ^'a widget class' )
	)
	public class SpecialWidget new = Widget new () (
	)
	public class OverridingWidget new = Widget new () (
	) : (
		public describe = ( ^'a special widget class' )
	)
	public main: platform args: args = (
		transcript show: self class describe; newLine.
		transcript show: (self class greet: 'world'); newLine.
		transcript show: Widget describe; newLine.
		transcript show: OverridingWidget describe; newLine.
	)
) : (
	public describe = ( ^'the class side of ClassSide' )
	public greet: name = ( ^'hello, ', name )
)
```

`SpecialWidget describe` is removed from `main:args:` and a second fixture
takes it, so that the successful lines and the failing send are not in one
program (a `doesNotUnderstand:` ends the run). Create
`tests-fixtures/class_side_not_inherited.ns`:

```newspeak
Newspeak3
'Test'
class ClassSideNotInherited usingPlatform: platform = Object new (| transcript ::= platform transcript. |) (
	public class Widget new = Object new () (
	) : (
		public describe = ( ^'a widget class' )
	)
	public class SpecialWidget new = Widget new () (
	)
	public main: platform args: args = (
		transcript show: SpecialWidget describe; newLine.
	)
) : ()
```

Write `tests-fixtures/class_side.expected` as the three lines the re-cut
program prints, and `tests-fixtures/class_side_not_inherited.expected` as the
empty stdout the failing run produces. Run the fixture runner to read the exact
stderr and exit status, and record them wherever the harness records them for a
fixture that fails — `crates/ns-cli/tests/program_outcomes.rs` is where the
golden exit status, stdout and stderr live:

Run: `cargo run -p ns-cli -- run tests-fixtures/class_side_not_inherited.ns`
(`--send` defaults to `main:args:`, which is what these fixtures declare.)

- [ ] **Step 6: Re-cut the factory-check fixture**

`tests-fixtures/cache_super_send_reaches_the_factory_check.ns` rests entirely
on a class-side `doesNotUnderstand:` declared by a superclass, which no longer
exists. It was written to pin that a `super` send in an inherited class-side
method reaches the factory check; with flat metaclasses there is no inherited
class-side method, and Task 2 removes the check. What is worth pinning in its
place is that a class-side `super` send finds `Object`'s behaviour and nothing
else. Replace the fixture with:

```newspeak
Newspeak3
'Test'
class CacheSuperSendReachesTheFactoryCheck usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Base = Object new () (
	) : (
	)
	class Simple = Base new () (
		public printString = ( ^'a simple' )
	) : (
		public build = ( ^super printString )
	)
	public main: platform args: args = (
		transcript show: Simple build; newLine.
	)
) : ()
```

`super printString` in `Simple`'s class-side `build` looks in `Object`'s
behaviour (§5.10 with a flat metaclass), so it answers the class object's own
`printString`. Record what it prints as the `.expected`, and update the golden
in `crates/ns-cli/tests/program_outcomes.rs` to match. Rename the fixture to
`class_side_super_reaches_object.ns` — the old name names a check that no
longer exists — and rename its golden with it.

- [ ] **Step 7: Run the whole suite and the corpus gates**

Run, in order:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```
Expected: clippy warning-free; every test passing; `parse-corpus` reporting
`parsed 210, failed 0, expected-fail 1, stale 0`; `run-corpus` reporting
`ran 6, failed 0, skipped 205`. A corpus module that now fails is a real
finding: read it before touching anything, because a corpus module relying on
an inherited class side would be evidence against the reading of §3.2, and
`prior-art/` decides that question, not the corpus.

- [ ] **Step 8: Commit**

```bash
git add crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/tests/mixin.rs \
        tests-fixtures/ crates/ns-cli/tests/program_outcomes.rs
git commit -m "fix: metaclasses are flat, so a class side is not inherited

newspeak-spec.pdf §3.2 makes every metaclass a direct subclass of Class,
never of the superclass's metaclass, and §6 gives the construction. All
three Newspeak implementations build it that way. metaclass_of chained
them Smalltalk-style, so a subclass answered class-side messages its own
class side never declared.

Two recorded outcomes pinned that and are re-cut."
```

---

### Task 2: The primary factory is an entry of the metaclass mixin's method table

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MethodEntry`, a `FactoryMethod`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
  (`build_class_side`, `try_factory_send`, `is_factory_send`)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`resolve_public`,
  `resolve_super`, `resolved`, `act`, `act_factory`, `handler_message`)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`Target::Factory`)
- Modify: `crates/ns-runtime/src/image/mixins.rs` and `image/decode.rs` if a
  decoded metaclass mixin must carry the entry (read `verify_mixins` first)
- Test: `crates/ns-runtime/tests/factory_send.rs`

**Interfaces:**
- Consumes: Task 1's flat `metaclass_of`.
- Produces: `MethodEntry::Factory(FactoryMethod)` with
  `struct FactoryMethod { pub selector: Symbol, pub argument_count: usize, pub access: ast::AccessModifier }`,
  found by `dispatch::lookup_public` like any entry;
  `Resolved::Factory { level: ClassReference<'gc>, chain: Chain }`, where
  `level` is the class object to instantiate. Task 3 relies on the entry being
  in the metaclass mixin's `methods` map under the factory's selector.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/factory_send.rs`:

```rust
#[test]
fn a_factory_is_found_by_the_ordinary_lookup() {
    // newspeak-spec.pdf §6.3: "The class side, together with the primary
    // factory method, defines the mixin for the metaclass." The factory is an
    // entry of that mixin's method table, not a case ahead of the lookup.
    let interpreter = /* the harness this file already uses */;
    let class = load_class(&interpreter, "class Boxed with: contents = Object new () ()");
    let metaclass = metaclass_of(&interpreter, class);
    let entry = method_entry(&interpreter, metaclass, Symbol::intern("with:"));
    assert!(
        matches!(entry, Some(MethodEntry::Factory(_))),
        "the metaclass mixin must hold the factory under its selector"
    );
}

#[test]
fn a_factory_selector_of_a_superclass_is_not_understood() {
    // With a flat metaclass (§3.2) a factory entry is reachable only on the
    // receiver's own metaclass: `Keyed new` is not an instantiation, because
    // `new` is Simple's factory selector, not Keyed's.
    let source = indoc! {"
        Newspeak3
        'Test'
        class FactoryNotInherited usingPlatform: platform = Object new (|
            transcript = platform transcript.
        |) (
            class Simple = Object new () (
            )
            class Keyed with: seed = Simple new () (
            )
            public main: ignored args: arguments = (
                transcript show: Keyed new printString; newLine.
            )
        ) : ()
    "};
    let outcome = run_module_source(source, "main:args:");
    assert!(
        outcome.stderr.contains("does not understand"),
        "stderr was: {}",
        outcome.stderr
    );
}
```

Write the first test against the helpers `factory_send.rs` already has; if it
has no way to read a method entry, add one next to the test rather than
widening a runtime API for a test's sake.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test factory_send -- --nocapture`
Expected: the first FAILs (no `MethodEntry::Factory` exists), the second FAILs
(today `Keyed new` is answered by the factory check ahead of the lookup only
when `new` is Keyed's own selector — read the failure text before assuming
which way it fails).

- [ ] **Step 3: Add the entry kind**

In `crates/ns-runtime/src/mixin.rs`:

```rust
/// The primary factory as a member of the metaclass mixin (§6.3: "The class
/// side, together with the primary factory method, defines the mixin for the
/// metaclass"). It carries no body: what it does is instantiate the class
/// object it was found on, which `interpreter::resolve::act` runs.
#[derive(Debug, Clone)]
pub struct FactoryMethod {
    pub selector: Symbol,
    pub argument_count: usize,
    pub access: ast::AccessModifier,
}

pub enum MethodEntry {
    Compiled(Rc<MixinMethod>),
    SlotAccessor(SlotAccessor),
    Primitive(PrimitiveMethod),
    Factory(FactoryMethod),
}
```

Extend `MethodEntry::access` with the new arm (`MethodEntry::Factory(factory)
=> factory.access`) and leave `compiled` answering `None` for it. A factory is
`public` (§6.3.1's default is protected, but a factory that could not be sent
from outside would make no class instantiable; the specification's own examples
send it across module boundaries).

- [ ] **Step 4: Build the entry onto the metaclass mixin**

In `crates/ns-runtime/src/interpreter/instantiate.rs`, `build_class_side` is
where a class side becomes a `Mixin`. Insert the factory entry there, under the
declaration's factory selector, so that every metaclass mixin — including the
synthesized one for a class with no class-side declarations — holds exactly one
factory entry:

```rust
    // §6.3: the metaclass mixin is the class side *together with the primary
    // factory method*, so the factory is an entry of that method table and is
    // found by the ordinary lookups. Before this it was a case ahead of the
    // lookup, which cost every class-object send a factory-selector compare
    // and needed a re-check wherever a cache could record the decision.
    let factory_selector = mixin.factory.map_or(Symbol::NEW, |factory| factory.selector);
    let argument_count = mixin.factory.map_or(0, |factory| factory.argument_count);
    methods.insert(
        factory_selector,
        MethodEntry::Factory(FactoryMethod {
            selector: factory_selector,
            argument_count,
            access: ast::AccessModifier::Public,
        }),
    );
```

A class with no class side now needs a metaclass mixin all the same, because
the factory lives on it: `metaclass_of` must synthesize an otherwise empty one
rather than answering `None`. Adjust Task 1's `metaclass_of` accordingly — the
`class_side.clone()?` becomes "the declared class side, or an empty mixin" —
and keep the laziness: the metaclass is still built on first class-side send.

- [ ] **Step 5: Resolve and act on it**

In `crates/ns-runtime/src/interpreter/resolve.rs`:

- delete the `is_factory_send` branch at the top of `resolve_public` and the
  one in `resolve_super`, and the factory re-check in `handler_message`;
- map the new entry in `resolved(...)` to
  `Resolved::Factory { level, chain }`, where `level` is the class object the
  lookup walked to — for a flat metaclass that is the receiver itself;
- change `act`'s factory arm to instantiate `level` directly, with no re-check:

```rust
            Resolved::Factory { level, .. } => self.decide_factory(level, arguments),
```

and in `instantiate.rs` replace `try_factory_send` with:

```rust
    /// §6.3's instantiation, the action of a factory entry: a fresh instance
    /// of `class`, then its instance initializer. No re-check: the entry was
    /// found on the receiver's own metaclass, whose dispatch id is the
    /// class-side guard, so a cache hit and a miss reach the same class.
    pub(super) fn decide_factory(
        &mut self,
        class: ClassReference<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match self.heap.class_builtin(class) {
            // `Array`/`OrderedCollection`'s representation is
            // `ObjectData::Array`, not `decide_instantiation`'s
            // `ObjectData::Instance`: `builtins::collections`' primitives
            // pattern-match on it, so a generic instance payload would leave
            // every native method on the result unable to find its data.
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Step::Value(
                Value::Object(self.new_instance_object(class, ObjectData::Array(vec![]))),
            )),
            _ => self.decide_instantiation(class, arguments.to_vec()),
        }
    }
```

`is_factory_send` and `primary_factory_selector` stay only where §6.1 needs
them, in `clause_message_body`'s check of the superclass clause, which does not
change. Let the compiler find every other caller.

- [ ] **Step 6: Follow the entry into the cache**

`Target::Factory` in `interpreter/inline_cache.rs` carried a chain and nothing
else, because `act_factory` re-checked. It now records what the lookup found
like any other target: the chain and the level, rebuilt on a hit by the same
`chain_start` + `level` walk the other targets use. Check the release size of
`Target` and `CacheEntry` afterwards — the sizes are pinned by a test:

Run: `cargo test -p ns-runtime --release --test inline_caches sizes -- --nocapture`

If `Target` has grown past 16 bytes, say so in the task's report rather than
widening the pinned size quietly: the entry's size is the user's decision of
2026-09-16.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test factory_send -- --nocapture`
Expected: PASS. Then `cargo test --workspace`.

- [ ] **Step 8: Run the gates and commit**

Run the full gate list from the Global Constraints, then:

```bash
git add crates/ns-runtime/src crates/ns-runtime/tests
git commit -m "feat: the primary factory is an entry of the metaclass mixin

newspeak-spec.pdf §6.3: the class side, together with the primary factory
method, defines the mixin for the metaclass. Both references build it that
way. The factory is now found by the ordinary lookups, and the three
special cases go with nothing in their place: the check ahead of lookup,
the Factory target's re-check and the re-check ahead of a super send's
doesNotUnderstand: target. With a flat metaclass the entry is reachable
only on the receiver's own metaclass, which the class-side guard
determines."
```

---

### Task 3: A class side may not declare the primary factory's selector

**Files:**
- Modify: `crates/ns-runtime/src/resolver/classes.rs` (`duplicate_member_names`)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (the error's wording, if the
  existing one does not fit)
- Test: `crates/ns-runtime/tests/resolver.rs`

**Interfaces:**
- Consumes: Task 2's entry — the reason the conflict matters is that a method
  table holds one entry per selector.
- Produces: no new API; one more `ResolveError` from the existing check.

- [ ] **Step 1: Write the failing test**

In `crates/ns-runtime/tests/resolver.rs`, beside the existing duplicate-name
tests:

```rust
#[test]
fn a_class_side_may_not_declare_the_factory_selector() {
    // §6.3 makes the factory a member of the metaclass mixin, and a method
    // table holds one entry per selector. Primordial Soup refuses it too
    // (Intermediates.ns, checkNameConflictsForFactory:).
    let source = indoc! {"
        Newspeak3
        'Test'
        class Conflict usingPlatform: platform = Object new () (
            class Widget new = Object new () (
            ) : (
                public new = ( ^'not a widget' )
            )
        ) : ()
    "};
    let errors = resolve_errors(source);
    assert!(
        errors.iter().any(|error| error.message.contains("new")),
        "the resolver must refuse a class side that declares the factory's \
         selector, errors were: {errors:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test resolver a_class_side_may_not_declare -- --nocapture`
Expected: FAIL — no error is reported today.

- [ ] **Step 3: Extend the check**

In `crates/ns-runtime/src/resolver/classes.rs`, `duplicate_member_names` walks
each side's declared members. The factory is a member of the class side, so
feed it in:

```rust
    // §6.3: the class side and the primary factory method together define the
    // metaclass mixin, so a class-side method under the factory's selector is
    // a second entry for one name. The canonical compiler refuses it
    // (`prior-art/newspeak/...`, `checkNameConflictsForFactory:`).
    if let Some(class_side) = declaration.class_side.as_ref() {
        let factory = factory_selector_of(declaration);
        let mut seen = HashSet::new();
        seen.insert(factory);
        for member in declared_members(class_side) {
            if !seen.insert(member) && !duplicates.contains(&member) {
                duplicates.push(member);
            }
        }
    }
```

with `factory_selector_of` reading the declaration's message pattern through
`instantiate::method_name` / `pattern_parameter_count`, the two functions that
already turn a pattern into a selector, and defaulting to `new` when the
declaration writes no factory signature (§6).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ns-runtime --test resolver a_class_side_may_not_declare -- --nocapture`
Expected: PASS. Then `cargo test --workspace` — a fixture or corpus module that
declares such a class side is a finding, not a licence to weaken the check.

- [ ] **Step 5: Run the gates and commit**

```bash
git add crates/ns-runtime/src/resolver crates/ns-runtime/tests/resolver.rs
git commit -m "feat: refuse a class side that declares the factory's selector

§6.3 makes the primary factory a member of the same metaclass mixin, and a
method table holds one entry per selector. Primordial Soup refuses it;
SOMns lets the declared method win and files the factory under an
unreachable name, after which the class no longer instantiates and nothing
says so."
```

---

### Task 4: The global cache's lexical key includes N's application

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs`
  (`CacheRule`, `SendKey`, `GlobalCache::probes`/`get`/`insert`,
  `send_lexical`, `send_lexical_first`)
- Test: `crates/ns-runtime/tests/inline_caches.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces: a global-cache key of `(o's dispatch id, selector, rule, N's
  application id)`; `CacheRule::Lexical` entries are valid for any site with
  the same N, so a vacant lexical site may use the global cache.

- [ ] **Step 1: Write the failing test**

In `crates/ns-runtime/tests/inline_caches.rs`:

```rust
#[test]
fn a_vacant_lexical_site_uses_the_global_cache() {
    // 3b let a vacant lexical site skip the global cache, because a Lexical
    // entry did not say which N decided it. With N's application in the key
    // every entry under it is right for any site.
    let statistics = run_and_read_statistics(indoc! {"
        ... a module whose two distinct lexical sites send the same selector
        to the same enclosing object ...
    "});
    assert_eq!(
        statistics.filled_global_entries, 1,
        "both sites must share one global entry"
    );
}
```

Write the module so that the two sites are in different methods of the same
class and send the same implicit-receiver selector; read
`CacheStatistics::filled_global_entries` through the accessor
`inline_caches.rs` already exposes for the existing statistics tests.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test inline_caches a_vacant_lexical_site -- --nocapture`
Expected: FAIL — today the second site fills a second entry (or none).

- [ ] **Step 3: Widen the key**

Give `GlobalCache::probes`, `get` and `insert` a fourth component, the lexical
class's `instances` dispatch id, `DispatchId::NONE` for the rules that do not
use it:

```rust
    fn probes(
        &self,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
        lexical: DispatchId,
    ) -> [usize; 2] {
        let hash = crate::hash::IdentityBuildHasher::default()
            .hash_one((guard, selector, rule, lexical));
        [hash as usize & self.mask, (hash >> 32) as usize & self.mask]
    }
```

`GlobalEntry::holds` compares the fourth component too. In `send_lexical`,
drop the rule that a vacant site skips the global cache, and pass the lexical
class's id for `CacheRule::Lexical`; `CacheRule::LexicalPrivate` keeps its
present key, which is already N's application alone.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p ns-runtime --test inline_caches -- --nocapture`
Expected: PASS, including every existing cache test.

- [ ] **Step 5: Run the gates and commit**

```bash
git add crates/ns-runtime/src/interpreter/inline_cache.rs crates/ns-runtime/tests/inline_caches.rs
git commit -m "perf: key a lexical global-cache entry by N's application

Whether §5.9's private rule applies depends only on N and the selector, so
an entry under (o's id, selector, lexical, N's application) is right for
any site, and a vacant lexical site may use it. Primordial Soup keys its
implicit-receiver cache by the calling method for the same reason."
```

---

### Task 5: Call-site numbers are not serialized

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (retire 21–24, take 25–28)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`encode_instruction` stops
  writing the number; `encode_code` writes the uncached flag)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`decode_instruction` reads
  no number; a family is numbered once its root is read; the density check and
  `DecodeError::CallSitesNotDense` go; `CallSiteListNotEmpty` may go with the
  list)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `bytecode::family_size_of`, which both sides already use.
- Produces: a decoded `CompiledCode` whose call-site numbers equal the
  compiler's, and a code record that says whether its family was compiled
  uncached.

- [ ] **Step 1: Write the failing test**

In `crates/ns-runtime/tests/image.rs`:

```rust
#[test]
fn a_decoded_family_is_numbered_as_the_compiler_numbered_it() {
    let compiled = compile_module_source(SOURCE_WITH_BLOCKS);
    let bytes = encode_image(&compiled);
    let decoded = decode_image(&bytes);
    assert_eq!(
        call_site_numbers(&decoded),
        call_site_numbers(&compiled),
        "the decoder must number a family in the compiler's order"
    );
}
```

`call_site_numbers` walks a code family in the compiler's order — a body's
sends in order, a block's at its `MakeClosure` — and collects
`Instruction::call_site`. Use a source with at least two nested blocks and a
send in each, so that the order is not accidentally the same as any other.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test image a_decoded_family_is_numbered -- --nocapture`
Expected: FAIL to compile or to match, depending on how much of the encoder is
already gone.

- [ ] **Step 3: Stop writing the number**

In `opcodes.rs`, retire the four send tags and take the next four:

```rust
// 21-24 retired: the sends carried a call-site number until the number
// became a property of the code rather than of the artifact. Retiring the
// numbers is what makes a stale artifact fail to decode instead of decoding
// into a different, perfectly valid program.
pub const OPCODE_SEND_LEXICAL: u8 = 25;
pub const OPCODE_SEND_PUBLIC: u8 = 26;
pub const OPCODE_SEND_INLINABLE: u8 = 27;
pub const OPCODE_SEND_SUPER: u8 = 28;
```

Update `crates/ns-runtime/tests/opcode_tags.rs` to pin the new values and to
state the retired ones. Delete `encode_call_site` and its four call sites, and
`decode_call_site` with them.

- [ ] **Step 4: Number on decode**

A block precedes its parent in the code table, but its numbers depend on its
place in the parent, so the numbering runs once the root is read. In
`decode.rs`, after a code record's instructions are decoded, walk the family in
the compiler's order and assign:

```rust
/// Numbers one code family as `compiler::lower` numbers it: a body's sends in
/// emission order, a block's at its `MakeClosure`. The number is a property of
/// the code, never read from the artifact, so a stale body cannot bring a
/// number of its own (`docs/design/dispatch.md`, "Decisions").
fn number_call_sites(code: &mut CompiledCode, next: &mut u32) {
    for instruction in &mut code.instructions {
        match instruction {
            Instruction::MakeClosure { code: block, .. } => {
                number_call_sites(Rc::make_mut(block), next)
            }
            other => {
                if let Some(site) = call_site_mut(other) {
                    *site = CallSite { number: *next };
                    *next += 1;
                }
            }
        }
    }
}
```

Recursion depth here is the *block nesting* depth, which the decoder already
bounds; if it does not, bound it with the counted limit the rest of the decoder
uses rather than leaving a crafted artifact to recurse.

A family compiled by the reference lowering runs uncached, so the code record
gains a flag: write it in `encode_code`, read it in `decode_code`, and give
every send of that family `CallSite::UNCACHED` instead of a number. Recompute
`family_size` from the numbered instructions with `family_size_of`, as the
decoder already does.

- [ ] **Step 5: Delete the density check**

`verify_call_sites` and `DecodeError::CallSitesNotDense` exist only to refuse a
body whose serialized numbers were not dense. Nothing serializes numbers any
more, so both go. Keep a test that a stale artifact — one with a retired send
tag — is refused, and point it at `DecodeError::UnknownOpcode` (or whatever the
decoder's unknown-tag error is called).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test image -- --nocapture`,
then `cargo test -p ns-runtime --test opcode_tags`,
then `cargo test --workspace`. The golden image round trip
(`crates/ns-runtime/tests/fixtures/golden-image.bin`) is part of the suite: it
must be rebuilt, and the commit says why — the send tags moved.

- [ ] **Step 7: Run the gates and commit**

```bash
git add crates/ns-runtime/src/image crates/ns-runtime/tests
git commit -m "refactor: number call sites on decode instead of serializing them

A number is a property of the code, so the decoder assigns it in the
compiler's order rather than reading it from the artifact. The send tags
21-24 retire for 25-28, which is what makes a stale artifact fail to decode
rather than decode into a different, perfectly valid program; the density
check and CallSitesNotDense go with the numbers, and a code record carries
a flag for a family compiled uncached."
```

---

### Task 6: The global cache becomes two-slot least-recently-used groups

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`GlobalCache`)
- Create: `tests-fixtures/bench_megamorphic.ns`, beside `bench_block.ns`,
  `bench_loop.ns` and `bench_send.ns` (the protocol's other four sources are
  written out in the 2c plan and the spec-conformant-lookup section of
  `PROGRESS.md`, not checked in)
- Test: `crates/ns-runtime/tests/inline_caches.rs`

**Interfaces:**
- Consumes: Task 4's four-component key.
- Produces: `GlobalCache` with 512 groups of two adjacent slots; the same
  `get`/`insert` signatures.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_group_keeps_the_more_recently_used_of_two_keys() {
    // Strongtalk's lookupCache: a miss moves the first slot's entry to the
    // second and writes the first; a hit in the second swaps them. 3b's
    // insert evicted the first probe whatever its use.
    let mut cache = GlobalCache::with_groups(1);
    cache.insert(first_key(), first_target());
    cache.insert(second_key(), second_target());
    assert!(cache.get(first_key()).is_some(), "the older key stays in the group");
    cache.insert(third_key(), third_target());
    assert!(cache.get(first_key()).is_none(), "the least recently used goes");
    assert!(cache.get(second_key()).is_some());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test inline_caches a_group_keeps_the_more -- --nocapture`
Expected: FAIL — `with_groups` does not exist and the eviction rule is the old
one.

- [ ] **Step 3: Implement the groups**

512 groups of two adjacent slots, so that the table keeps its 1024 entries and
a group is one cache line's worth of probing. One hash picks the group; a hit
in the second slot swaps the two; a miss moves the first to the second and
writes the first. `set_global_cache_size` keeps its meaning in entries and
panics on a count that is not twice a power of two, as it panics today on one
that is not a power of two.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p ns-runtime --test inline_caches -- --nocapture`

- [ ] **Step 5: Write the megamorphic benchmark**

None of the seven measured sources reaches a megamorphic site, so the change
would be measured on programs that never exercise it. Write
`bench_megamorphic.ns`: one send site that sees more than four receiver classes
in a loop, so the site goes megamorphic and every send probes the global cache.
Keep its shape and its iteration count in line with the existing benchmark
sources, so that one sitting can run them all.

- [ ] **Step 6: Run the gates and commit**

```bash
git add crates/ns-runtime/src/interpreter/inline_cache.rs crates/ns-runtime/tests/inline_caches.rs bench_megamorphic.ns
git commit -m "perf: two-slot least-recently-used groups in the global cache

Strongtalk's lookupCache. 3b's insert evicted the first probe whatever its
use; a group of two adjacent slots keeps the more recently used of two keys
that collide. Measured with a benchmark whose site is megamorphic, which
none of the existing sources reaches."
```

---

### Task 7: Measure the step whole, and record what was built

**Files:**
- Modify: `docs/design/dispatch.md` (the decisions become built, with the
  numbers), `docs/design/object-model.md` (the deviation is gone),
  `docs/design/README.md` (two rows leave the table), `docs/PROGRESS.md` (the
  step's own section, and "Where the next session starts" moves to 3c)

**Interfaces:**
- Consumes: Tasks 1–6.
- Produces: the documents a fresh session starts from.

- [ ] **Step 1: Build the binaries the protocol compares**

Follow `docs/design/execution.md`, "What removing the re-entries moved": every
binary — the current tree included — built with the same explicit `RUSTFLAGS`,
which pin every function start to a 64-byte boundary. In PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Build START (`0fa191f`) and `d804eaf` the same way, in throwaway worktrees
removed afterwards.

- [ ] **Step 2: Measure**

Run the protocol's seven sources plus `tests-fixtures/bench_megamorphic.ns`,
two sittings, against START and against `d804eaf`. Record every number, not a summary: the
report says which source moved how far in each sitting.

- [ ] **Step 3: Check that no send's hit path became a call**

`vm::run`'s per-instruction step sits near LLVM's inlining threshold. On a
release build with debug information, run `dumpbin /disasm` (from the Visual
Studio Build Tools, not on `PATH`) and confirm that no `vm::run::closure`
became a function of its own, as `docs/design/execution.md`, "What the inline
caches moved" requires.

- [ ] **Step 4: Write the documents**

- `dispatch.md`: the five decisions move from "to be built before 3c" to built,
  each with what it measured; the factory's re-checks leave "Every target is a
  function of its key and its site", and `ClassData.builtin` leaves the list of
  reads a key does not cover.
- `object-model.md`: "Class sides and metaclasses" drops the *Today:* paragraph;
  what remains of §3.2 keeps its step-6 schedule.
- `README.md`: the class-side-inheritance row and the primary-factory row leave
  the deviations table; the §3.2 row keeps its schedule.
- `PROGRESS.md`: a section for this step in the chronology, and "Where the next
  session starts" now names 3c and its plan.

- [ ] **Step 5: Commit**

```bash
git add docs
git commit -m "docs: record the step before 3c as built, with its numbers"
```

---

## Self-Review

**Spec coverage.** Each of the five decisions in `dispatch.md`'s "Decided by
the user (2026-09-17, extended 2026-09-18)" has a task: flat metaclasses
(Task 1), the factory as a metaclass entry (Task 2), the class-side name
conflict (Task 3), the lexical key (Task 4), call-site numbers (Task 5), the
LRU groups (Task 6). `object-model.md`'s correction is Task 1's, and its
step-6 schedule is explicitly out of scope. The measurement the decisions ask
for is Task 7.

**Placeholders.** Two places in this plan name a helper the test file must
supply rather than showing it: `run_module_source` and `resolve_errors` in
Tasks 1–3, and `run_and_read_statistics` in Task 4. Each says which existing
test file to copy it from; copying the neighbour's helper is deliberate, since
a second shape for the same job is how test files drift. The benchmark source
in Task 6 is described by its shape rather than written out, because its
iteration count has to match the existing sources, which the implementer reads.

**Type consistency.** `FactoryMethod` (Task 2) is used by name in Tasks 2 and
3. `Resolved::Factory { level, chain }` is introduced in Task 2 and read in
Task 2 only. `GlobalCache::with_groups` (Task 6) replaces `with_entries`
(Task 4 still calls the old name) — Task 6 renames it, and Task 4's call site
moves with it; that is the one signature this plan changes twice, and it is
called out here so the implementer of Task 6 does not leave Task 4's call
behind.

## Execution Handoff

Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, review
   between tasks, fast iteration.
2. **Inline Execution** — the tasks run in this session with checkpoints.
