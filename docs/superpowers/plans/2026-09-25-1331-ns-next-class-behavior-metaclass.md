# `Behavior`, `Class` and `Metaclass` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** §3.2's reified class hierarchy: `Behavior`, `Class` and `Metaclass`
are declared kernel classes, every class object's class is its own metaclass
— the application of its class side to `Class` — and every metaclass's class
is `Metaclass`; a class object is a value exactly when its enclosing object
is, with equality and hash derived from its state.

**Architecture:** A class object's state stays in `ClassData`; the three
kernel classes carry behaviour only. The work lands in five steps that each
leave the suite green: the kernel declares the classes (Task 1); every class
object gets an eager metaclass over `Class`, still reached through
`ClassData` while the header keeps saying `Object` (Task 2); the image format
carries metaclasses (Task 3); the header flips to the metaclass, which makes
dispatch uniform and removes the class-side special cases (Task 4); the
kernel protocol (Task 5) and the value semantics (Task 6) follow.
Documentation (Task 7) and the end-of-batch gates with the measurement
(Task 8) close it.

**Tech Stack:** Rust 1.98.0 (edition 2021), `ns-runtime`, `Kernel.ns`, the
image format, the golden harness (`crates/ns-cli/tests/run_goldens.rs`).

**Spec:** `docs/design/object-model.md`, "Planned: `Behavior`, `Class` and
`Metaclass`" (designed with the user on 2026-09-25; commits `9f72443`,
`8a51782`, `0fe417c`). Semantics authority: `docs/references/newspeak-spec.md`
§3.2 (metaclasses), §6 and §6.1 (the metaclass mixin applied to `Class`;
nested classes per enclosing instance), §6.4 (module definitions are values),
§3.1.1 (value objects), §2.3 (`Outer new Inner = Outer new Inner` is false for
an ordinary `Outer`).

## Decisions this plan implements (user, 2026-09-25)

1. **A class object is a value exactly when its enclosing object is**
   (SOMns's `SClass.isValue`). A module definition has none and is always
   one (§6.4). A metaclass is a value exactly when its class is.
2. **The state stays in `ClassData`**; the kernel classes carry behaviour
   only (SOMns, `KernelForJS.ns`), not Primordial Soup's VM-known slots.
3. **Eager metaclasses.** `new_class_object` builds the metaclass with the
   class; the header is right from the start. `ClassData.metaclass` goes;
   `ClassData.this_class` is a metaclass's link back to its class.
4. **Hierarchy `Object` → `Behavior` → {`Class`, `Metaclass`}**
   (Primordial Soup). None of the three inherits `Value`.
5. **Minimal protocol** (Primordial Soup): `Class>>name`, `Class>>printString`,
   `Metaclass>>name` (`thisClass name, ' class'`), `Metaclass>>printString`.
   No `superclass`, `selectors` or methods at the base level.
6. **No accessor on `Object`** for `Behavior`, `Class` or `Metaclass`.
   Instantiating any class that inherits `Behavior` is refused.
7. **Equality of value class objects is derived from state**: same mixin
   application, enclosing objects `=`, superclass class objects `=`;
   metaclasses by their `this_class`. It extends `builtins::value`'s walk
   with class objects as a node kind, and the walk's class comparison at
   every node becomes a pushed pair. `Behavior` declares `=`, `==` (same
   answer) and `hash`.
8. **The hash of a value class object is derived from its mixin's name
   alone** (a metaclass's from its class's); every other class object keeps
   its identity hash.
9. **Image format:** the metaclass of a class record is a record of its own
   (`OBJECT_METACLASS`), whose body is only the reference back to its class
   record; the metaclass of a prebuilt class (builtin, module root, root
   level) is named by tag (`CLASS_METACLASS` + that class's tag), its hash in
   the closing table (`HASHED_METACLASS`). The amplification bound does not
   move.

## Global Constraints

- Every name written out (`CLAUDE.md`, "Names are written out"); clippy's
  `min_ident_chars` refuses one- and two-character names. This holds for
  every code sample below and for everything an implementer adds.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()` an `EvaluationResult`
  or `HostResult` — use the test file's `expect_ok` helper.
- Inline Newspeak in a Rust test is an `indoc! {"…"}` / `formatdoc! {"…"}`.
- A golden's expected output changes only after establishing the new output
  is right; this plan names every golden it expects to change and why. A
  golden that changes and is not named here is stopped at and reported, not
  updated.
- `vm.rs` and `compiler/` gain no dispatch, lookup or error text
  (`CLAUDE.md`, "`vm.rs` holds no dispatch logic").
- An image is untrusted input: every new tag, id and pairing is validated
  before it indexes or allocates; no byte sequence may panic.
- A wire tag is assigned once: new tags take the next free number
  (builtin classes 20, 21, 22; `OBJECT_METACLASS` 8; `CLASS_METACLASS` 4;
  `HASHED_METACLASS` 3), and `crates/ns-runtime/tests/opcode_tags.rs` pins
  each.
- The VM mirror stays a capability (`CLAUDE.md`, "The VM mirror is a
  capability"): every new leaf is a `MIRROR` row reached only from
  `Kernel.ns`.
- Code, comments, commit messages in English; conventional commits; every
  commit ends with
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018GVCe67btK9tdsL6nWoa62
  ```
  naming the model that actually wrote the commit (a subagent names its own
  model in the `Co-Authored-By` line).
- Per-commit gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets`
  (warning-free), `cargo test --workspace`,
  `cargo run -p ns-cli -- parse-corpus corpus/modules` ("parsed 210, failed 0,
  expected-fail 1, stale 0"). The stress suites and `run-corpus` run once, in
  Task 8. A Markdown-only commit runs none of them.
- Every command runs in the foreground; nobody hands back while their own
  background work runs.
- Work on the branch `feature/class-behavior-metaclass`, checked out in the
  main working tree (no worktree).

## Review Focus

1. **A nested class's metaclass under many enclosing instances stays one
   dispatch id.** Every `X new` site that sees class objects of one
   declaration under different enclosing instances must stay monomorphic:
   their metaclasses share one application (the class side over `Class`),
   hence one id. A per-class-object application would silently send every
   such site megamorphic. Pinned by a Rust test in Task 4 and the existing
   `goldens/cache/cache_nested_classes_share_behaviour.ns`.
2. **A metaclass keeps its identity across an image round trip.** `Foo class`
   held in a slot and a class-side activation's `current_class` for the same
   class decode to one object, which is also the decoded class's header
   class. Pinned by a Rust test in Task 3 (extended in Task 4).
3. **A crafted image cannot forge the pairing.** A class record whose header
   names another class's metaclass record, two metaclass records naming one
   class, a metaclass record naming a non-class, a class record applying a
   class-side row, and an instance record whose class inherits `Behavior`
   are each refused with a `DecodeError`, never a panic. Pinned by Rust tests
   in Task 3.
4. **A class-side `outer` still reaches the declaration's enclosing objects**
   for a builtin's metaclass built before the kernel is wired and for every
   user metaclass built eagerly. Pinned by the existing
   `goldens/class-side/class_side_outer.ns` and a Rust test in Task 2 that
   every builtin's metaclass has its class's enclosing object and declaring
   class after bootstrap.
5. **Instances of equal-but-distinct nested classes compare equal, and
   instances of classes from unequal enclosing values do not.** `a Point new`
   and `b Point new` for `a = b` two separately built value modules must be
   `=` and hash alike; for `a ≠ b`, not `=`. Pinned by a golden in Task 6.

---

### Task 1: The kernel declares `Behavior`, `Class` and `Metaclass`

No behaviour changes: the three classes exist, are filled like every other
builtin, have wire tags, and no program can name them.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:228-294` (`BuiltinClass`, `ALL`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:68-92` (`kernel_class_name`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (three declarations, after `Object`)
- Modify: `crates/ns-runtime/src/image/encode.rs:185-205` (`builtin_class_tag`)
- Modify: `crates/ns-runtime/src/image/decode.rs:714-775` (`builtin_class_from_tag`, `read_program_builtin_class`, new `read_referenced_builtin_class`), `:1950` (`decode_class_tag`'s `CLASS_BUILTIN` arm)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs:297-316` (`builtin_class_tags_are_fixed`)
- Modify: `crates/ns-runtime/tests/image.rs` (a refusal test beside the one at `:1064`)
- Whatever else the compiler names as a non-exhaustive `match` on `BuiltinClass` (for example `image/inspect.rs`), each given the obvious arm.

**Interfaces:**
- Produces: `BuiltinClass::Behavior`, `BuiltinClass::Class`,
  `BuiltinClass::Metaclass` (appended to `ALL`, which becomes
  `[BuiltinClass; 22]`); `decode::read_referenced_builtin_class(reader) ->
  Result<BuiltinClass, DecodeError>`.

- [ ] **Step 1: Pin the new tags (failing test)**

In `tests/opcode_tags.rs`, `builtin_class_tags_are_fixed`, append:

```rust
    assert_eq!(builtin_class_tag(BuiltinClass::Behavior), 20);
    assert_eq!(builtin_class_tag(BuiltinClass::Class), 21);
    assert_eq!(builtin_class_tag(BuiltinClass::Metaclass), 22);
```

Run: `cargo test -p ns-runtime --test opcode_tags`
Expected: compile error, no variant `Behavior`.

- [ ] **Step 2: The variants**

In `types.rs`, after `Number`:

```rust
    /// What a class and a metaclass share (`newspeak-spec.pdf` §3.2 names
    /// neither this class nor `Metaclass`'s superclass; the hierarchy is
    /// Primordial Soup's). No program can name it, and no class that
    /// inherits it can be instantiated: class objects are made only by
    /// evaluating a declaration.
    Behavior,
    /// §3.2: every metaclass is a direct subclass of `Class`.
    Class,
    /// §3.2: every metaclass is an instance of `Metaclass`.
    Metaclass,
```

and append the three to `ALL` in that order (`[BuiltinClass; 22]`).
`kernel_class_name` gets `Behavior => "Behavior"`, `Class => "Class"`,
`Metaclass => "Metaclass"`. `builtin_class_tag` gets 20, 21, 22 and
`builtin_class_from_tag` the inverse.

- [ ] **Step 3: The declarations in `Kernel.ns`**

After `Object`'s declaration (before the comment that opens `Value`):

```newspeak
    (* §3.2: "Each class is an instance of a unique metaclass. All
       metaclasses are instances of class Metaclass, and are direct
       subclasses of Class." The specification names neither `Behavior`
       nor `Metaclass`'s superclass; this is Primordial Soup's hierarchy,
       which keeps what only a real class has on `Class` so that a
       metaclass does not inherit it. A class object's state lives in the
       runtime (`ClassData`), as SOMns keeps it in `SClass`; these three
       carry behaviour only. `Object` declares no accessor for any of them. *)
    public class Behavior = Object new () (
    )
    public class Class = Behavior new () (
    )
    public class Metaclass = Behavior new () (
    )
```

`check_kernel_shape` needs no change: each is slotless, factory-less, and
names a kernel class.

- [ ] **Step 4: Refuse them where a program names a class**

In `decode.rs`, split the two readers. `read_referenced_builtin_class` is
today's `read_program_builtin_class` body (refuses `VmMirror` and `Top`) and
is what `decode_class_tag`'s `CLASS_BUILTIN` arm calls — from Task 4 on a
value can hold `Metaclass`, since `3 class class class` reaches it.
`read_program_builtin_class` — called by the root's superclass (`:2304`),
the root levels (`:2327`) and `decode_builtin_level` (`:2944`) — refuses the
three new kinds too:

```rust
/// A builtin class a *program* names as a superclass or as the mixin of a
/// `<:` clause. `read_referenced_builtin_class`'s two refusals, and three
/// more: `Behavior`, `Class` and `Metaclass`, which `Object` declares no
/// accessor for, so no source module can name them either; a class over
/// one of them could never be instantiated (`InstantiationRefusal::ClassObject`).
pub fn read_program_builtin_class(reader: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    let kind = read_referenced_builtin_class(reader)?;
    match kind {
        BuiltinClass::Behavior | BuiltinClass::Class | BuiltinClass::Metaclass => {
            Err(DecodeError::InvalidTag {
                what: "builtin class a program may name",
                tag: crate::image::encode::builtin_class_tag(kind),
            })
        }
        kind => Ok(kind),
    }
}
```

Move the doc comment that today explains `VmMirror` and `Top` onto
`read_referenced_builtin_class`.

- [ ] **Step 5: A refusal test**

In `tests/image.rs`, beside the test around `:1064` that crafts a root
superclass tag, add `a_root_superclass_naming_metaclass_is_refused`: take a
valid stateless binary (copy how the neighbouring test builds one), patch
the root superclass tag byte to 22, and assert `image::load` answers `Err`
with a message naming "builtin class a program may name". Copy the
neighbouring test's way of locating the byte.

- [ ] **Step 6: Run the gate**

Run: `cargo test --workspace`
Expected: PASS, including `every_builtin_class_is_filled_from_the_kernel`
for the three new kinds.

- [ ] **Step 7: Commit**

```bash
git add -A crates
git commit -m "feat: declare Behavior, Class and Metaclass in the kernel"
```

---

### Task 2: Every class object gets an eager metaclass over `Class`

The header of a class object still names the builtin `Object`, and a
class-side send still goes through `resolve_class_side`; what changes is
that the metaclass exists from the start, is applied over `Class`, links
back to its class, and cannot be instantiated.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/metaclass.rs` (building a metaclass, the bootstrap pass, the invariant, unit tests)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod metaclass;`, the `metaclasses_ready` and `behavior_mixin` fields, `metaclass_of` `:1212-1266` moves into `metaclass.rs`, `new_class_object`/`new_applied_level` `:1390-1420`, `apply_mixin`'s refusal `:1476-1541`)
- Modify: `crates/ns-runtime/src/types.rs:152-162, 205-227` (`ClassData.this_class`)
- Modify: `crates/ns-runtime/src/trace.rs:86-107` (trace `this_class`)
- Modify: `crates/ns-runtime/src/heap.rs` (`class_this_class`, `set_class_this_class`, `class_display_name`)
- Modify: `crates/ns-runtime/src/mixin.rs:625-642` (`InstantiationRefusal::ClassObject`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs:1210-1236` (the refusal's text)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:136-285` (record `behavior_mixin`, run the bootstrap pass, wire the metaclasses' lexical context)
- Modify: `crates/ns-runtime/src/image/decode.rs:2381-2560` (`rebuild_applications` applies a metaclass row over `Class`)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs:246-257` (`resolve_super_does_not_understand`'s comment: S for a class-side method is `Class`)
- Test: `crates/ns-runtime/src/interpreter/metaclass.rs` (unit tests), `crates/ns-runtime/tests/metaclasses.rs` (new)

**Interfaces:**
- Consumes: `BuiltinClass::{Behavior, Class, Metaclass}` (Task 1).
- Produces:
  ```rust
  // types.rs, ClassData
  /// A metaclass's class: the class object whose class side this metaclass
  /// applies. `None` for every class object that is not a metaclass.
  pub this_class: Option<ClassReference<'gc>>,
  // heap.rs
  pub fn class_this_class(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>>;
  pub(crate) fn set_class_this_class(&mut self, metaclass: ClassReference<'gc>, class: ClassReference<'gc>) -> bool;
  /// `Foo` for a class, `Foo class` for its metaclass, the mixin's name for
  /// anything else a class object applies.
  pub fn class_display_name(&self, class: ClassReference<'gc>) -> String;
  // mixin.rs
  InstantiationRefusal::ClassObject
  // interpreter/metaclass.rs, on Mutator
  pub(crate) fn attach_metaclass(&mut self, class: ClassReference<'gc>) -> ClassReference<'gc>;
  pub(crate) fn metaclass_mixin(&mut self, mixin: &Rc<Mixin>) -> Rc<Mixin>; // moved, now pub(crate)
  pub fn metaclass_of(&mut self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>>; // moved
  pub(crate) fn give_existing_class_objects_metaclasses(&mut self);
  ```

- [ ] **Step 1: The failing test**

Create `crates/ns-runtime/tests/metaclasses.rs` with the `load`/`expect_ok`
helpers copied from `tests/nested_classes.rs:12-39`, and:

```rust
/// §3.2 and §6: a class's metaclass is its class side applied to `Class`,
/// so a class-side message it does not declare falls to `Class`, and a
/// metaclass links back to the class it belongs to.
#[test]
fn a_metaclass_is_the_class_side_applied_to_class() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Subject = Object new () (
        ) : (
          public make = ( ^1 )
        )
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let Value::Object(class_object) = mutator.fetch(&class) else {
            panic!("a loaded module is a class object");
        };
        let metaclass = mutator
            .metaclass_of(class_object)
            .expect("every class object has a metaclass");
        let heap = mutator.heap();
        assert!(heap.class_this_class(metaclass).is_some_and(|this| this.same(class_object)));
        let class_class = mutator.builtin_class(BuiltinClass::Class);
        assert!(heap.class_superclass(metaclass).is_some_and(|superclass| superclass.same(class_class)));
        let metaclass_application = heap.class_application(metaclass).expect("an application");
        let class_application = heap.class_application(class_class).expect("an application");
        assert!(metaclass_application
            .superclass
            .as_ref()
            .is_some_and(|superclass| Rc::ptr_eq(superclass, &class_application)));
    });
}
```

`BuiltinClass` and `Mutator::builtin_class` must be reachable from an
integration test; if either is not public, the test goes into
`interpreter/metaclass.rs`'s unit tests instead (with `Interpreter::bootstrap`
and the crate's own `load` helper — copy the one in `interpreter/resolve.rs`'s
tests).

Run: `cargo test -p ns-runtime --test metaclasses`
Expected: FAIL (`class_this_class` does not exist).

- [ ] **Step 2: `this_class` in `ClassData`, the heap and the tracer**

Add the field (doc comment above) to `ClassData`, `this_class: None` in
`ClassData::new`, the accessors in `heap.rs` beside `class_metaclass`
(`:1757`) in the same shape — `None`/`false` for a handle that is not a
class object — and trace it in `trace.rs` beside `metaclass`
(`trace_optional_object(this_class, tracer)`), adding it to the destructuring
pattern. `class_display_name`:

```rust
    /// The name a class object is shown by: its mixin's for a class, and
    /// its class's followed by ` class` for a metaclass, as
    /// `Metaclass>>name` answers it (`Kernel.ns`). For the refusal texts
    /// that name what could not be instantiated.
    pub fn class_display_name(&self, class: ClassReference<'gc>) -> String {
        match self.class_this_class(class) {
            Some(this_class) => format!("{} class", self.class_display_name(this_class)),
            None => self
                .class_application(class)
                .map_or_else(|| "a class".to_string(), |application| application.mixin.name.to_string()),
        }
    }
```

(The recursion is one level: a metaclass's `this_class` is never a
metaclass; `attach_metaclass` below never sets it to one, and Task 3's
decoder verifies the same for an image.)

- [ ] **Step 3: `InstantiationRefusal::ClassObject` and `behavior_mixin`**

In `mixin.rs`:

```rust
    /// §3.2: a class object is made only by evaluating a declaration --
    /// the one way a metaclass stays unique to its class. Every class
    /// whose chain reaches `Behavior` is refused: `Class`, `Metaclass`,
    /// every metaclass, and any subclass of them a program manages to
    /// declare.
    ClassObject,
```

In `Mutator` (`interpreter/mod.rs`), beside `value_mixin`:

```rust
    /// `Kernel.ns`'s `Behavior` mixin, recorded by the kernel phase before
    /// `Behavior`'s own application is built, so that `apply_mixin` can
    /// refuse to instantiate anything below it
    /// (`InstantiationRefusal::ClassObject`).
    pub(crate) behavior_mixin: Option<Rc<crate::mixin::Mixin>>,
```

In `apply_mixin`, the refusal chain gains `ClassObject` right after the
superclass's inherited refusal (so a subclass of `Class` inherits it
unchanged, as every refusal is inherited):

```rust
        let is_behavior_mixin = self
            .behavior_mixin
            .as_ref()
            .is_some_and(|behavior_mixin| Rc::ptr_eq(behavior_mixin, mixin));
        let instantiation_refusal = mixin
            .root_clause_defect
            .clone()
            .map(crate::mixin::InstantiationRefusal::RootClause)
            .or_else(|| {
                superclass.and_then(|application| application.instantiation_refusal.clone())
            })
            .or_else(|| is_behavior_mixin.then_some(crate::mixin::InstantiationRefusal::ClassObject))
            .or_else(/* the mutable slot, unchanged */)
            .or_else(/* the identity method, unchanged */);
```

In `kernel.rs`'s `fill_builtin_classes`, record it beside `value_mixin`:
`if kind == BuiltinClass::Behavior { self.behavior_mixin = Some(mixin.clone()); }`.

In `instantiate.rs`, `instantiation_refusal_error` has no heap, so
`decide_instantiation` computes the name first and passes it; the new arm:

```rust
        crate::mixin::InstantiationRefusal::ClassObject => NewspeakError {
            selector: factory_selector(application),
            receiver: Value::Object(class),
            message: format!(
                "cannot instantiate {display_name}: a class object is made only by evaluating its declaration"
            ),
        },
```

where `display_name: &str` is a new parameter, `self.heap.class_display_name(class)`
at the call site (`:431`).

- [ ] **Step 4: `interpreter/metaclass.rs`**

Move `metaclass_of` and `metaclass_mixin` (`mod.rs:1212-1266`) here and add
the builder and the bootstrap pass:

```rust
//! Metaclasses (`newspeak-spec.pdf` §3.2, §6): every class object's
//! metaclass is its class side -- together with its primary factory,
//! §6.3 -- applied to `Class`, built together with the class object
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

impl<'gc> Mutator<'gc> {
    /// `class`'s metaclass, which exists for every class object once the
    /// bootstrap has filled `Class` (`give_existing_class_objects_metaclasses`);
    /// `None` before that, and for a handle that is no class object.
    pub fn metaclass_of(&mut self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        if !self.heap.is_class(class) {
            return None;
        }
        if let Some(existing) = self.heap.class_metaclass(class) {
            return Some(existing);
        }
        // A class object decoded from an image has none until Task 3's
        // decoder gives it its metaclass record; built here meanwhile.
        self.metaclasses_ready.then(|| self.attach_metaclass(class))
    }

    /// Builds `class`'s metaclass: §6's "the application of the metaclass
    /// mixin defined by the class' class side to the class `Class`". Its
    /// class-object superclass is `Class`, so a class-side `super` send
    /// searches `Class` (§5.10), and it shares `class`'s place in the
    /// declaration chain, since §6.3 writes a class side inside the same
    /// declaration as its instance side.
    ///
    /// The application is the memoized one for (class side, `Class`), so
    /// every class object of one declaration -- one per enclosing instance
    /// -- gets a metaclass of one application, and so one dispatch id: a
    /// per-class-object application would send every `X new` site
    /// megamorphic.
    pub(crate) fn attach_metaclass(&mut self, class: ClassReference<'gc>) -> ClassReference<'gc> {
        let application = self
            .heap
            .class_application(class)
            .expect("internal: attach_metaclass is handed a class object");
        let class_side_mixin = self.metaclass_mixin(&application.mixin);
        let class_class = self.builtin_class(BuiltinClass::Class);
        let class_application = self.heap.class_application(class_class);
        let metaclass_application =
            self.apply_mixin(&class_side_mixin, class_application.as_ref());
        let mut data = ClassData::new(
            metaclass_application,
            self.heap.class_enclosing(class),
            self.heap.class_declaring(class),
            Some(class_class),
            None,
        );
        data.this_class = Some(class);
        let metaclass = self.allocate_class_object(data);
        if !self.heap.set_class_metaclass(class, metaclass) {
            unreachable!("internal: attach_metaclass is handed a class object");
        }
        metaclass
    }

    /// The end of the kernel phase: every class object built before
    /// `Class` was filled -- the builtins, the kernel's `Value` level and
    /// the kernel's own class -- gets its metaclass, and from here on
    /// `new_class_object` builds one with every class object.
    pub(crate) fn give_existing_class_objects_metaclasses(&mut self, others: &[ClassReference<'gc>]) {
        let builtins: Vec<ClassReference<'gc>> =
            BuiltinClass::ALL.into_iter().map(|kind| self.builtin_class(kind)).collect();
        for class in builtins.into_iter().chain(others.iter().copied()) {
            if self.heap.class_metaclass(class).is_none() {
                self.attach_metaclass(class);
            }
        }
        self.metaclasses_ready = true;
    }
}
```

`metaclasses_ready: bool` is a new `Mutator` field (default `false`, doc
comment: "whether `new_class_object` builds a metaclass with every class
object; false only during the kernel phase, before `Class` is filled").
`new_class_object` and `new_applied_level` end with
`if self.metaclasses_ready { self.attach_metaclass(class); }` before
answering `class`. Keep `metaclass_mixin`'s doc comment.

- [ ] **Step 5: Wire it into the kernel phase**

In `fill_builtin_classes`, after the kernel's class is built (the last
`new_class_object` call, `:215`), call
`self.give_existing_class_objects_metaclasses(&[level, kernel_class])` —
`level` is the `Value` level built just above, `kernel_class` the returned
class; the kernel's class is created before the flag is set, so the pass
covers it. The doc comment of `fill_builtin_classes` drops "no metaclass …
built before this phase".

In `wire_kernel_in`, replace the `debug_assert!` at `:264-267` with setting
the metaclass's context too, since it copied the placeholder's:

```rust
            // The metaclass was built at the end of the fill, when the
            // builtin's lexical context was still the placeholder's; §6.3
            // puts a class side in its class's declaration, so it takes the
            // same context.
            if let Some(metaclass) = self.heap.class_metaclass(class) {
                if !self
                    .heap
                    .set_class_lexical_context(metaclass, Some(kernel), Some(kernel_class))
                {
                    unreachable!("internal: a metaclass is a class object");
                }
            }
```

and do the same for each applied level's metaclass in the loop at `:271-283`.
The kernel's own class keeps `None`/`None` and so does its metaclass.

- [ ] **Step 6: The decoder applies a metaclass row over `Class`**

In `rebuild_applications` (`decode.rs:2449-2465`), `is_metaclass` no longer
means "no superclass application" but "over `Class`'s":

```rust
            let super_application = if is_metaclass {
                context
                    .mutator
                    .heap
                    .class_application(context.mutator.builtin_class(BuiltinClass::Class))
            } else { /* unchanged */ };
```

Update the doc comments at `:2360-2372` and `metaclass_rows` (`:2536-2543`):
a metaclass is applied over `Class` (§6), no longer flat. The encoder's
comment at `encode.rs:1176-1198` says the superclass written for a metaclass
is `Class`'s class object now. (Task 3 replaces this path entirely; this
step only keeps the suite green.)

- [ ] **Step 7: Unit tests in `metaclass.rs`**

```rust
    /// Every class object has a metaclass whose `this_class` is it, and
    /// every builtin's metaclass has its class's lexical context -- the
    /// one the kernel phase gave the builtin after the metaclass was built.
    #[test]
    fn every_builtin_has_a_metaclass_in_its_declaration_context() { /* for kind in ALL: metaclass_of is Some,
       class_this_class(metaclass) is the builtin, class_enclosing and class_declaring of the
       metaclass equal the builtin's (compare with `same`) */ }

    /// §3.2: nothing below `Behavior` can be instantiated.
    #[test]
    fn a_metaclass_and_the_three_kernel_classes_refuse_instantiation() { /* decide_instantiation of
       builtin Behavior, Class, Metaclass and of `metaclass_of(Object)` each answers
       `Err(Signal::Error(error))` whose message starts "cannot instantiate " and contains
       "a class object is made only by evaluating its declaration"; for the metaclass the
       name is "Object class" */ }

    /// Two class objects of one declaration under two enclosing instances
    /// have metaclasses of one application.
    #[test]
    fn metaclasses_of_one_declaration_share_an_application() { /* load a module with a nested
       class, instantiate two enclosing instances, send the nested class accessor to each
       (copy how tests/nested_classes.rs does it), and assert Rc::ptr_eq of the two
       metaclasses' applications */ }
```

Write each body out in full; the comments say exactly what each asserts.
Match `Signal::Error` by hand (no `unwrap` of an `EvaluationResult`).

- [ ] **Step 8: Goldens**

Run: `cargo test --workspace`
Expected: PASS with no golden output changed. A class-side miss now walks
the metaclass, `Class`, `Behavior`, `Object` and answers what `Object`
answered before (`Class` and `Behavior` declare nothing yet), and a
class-side `super printString` still answers `Object`'s (`an Object`). If a
golden changes, stop and report it.

- [ ] **Step 9: Commit**

```bash
git add -A crates
git commit -m "feat: build every class object's metaclass eagerly over Class"
```

---

### Task 3: The image format carries metaclasses

Still with the header of a class object naming `Object`: the encoder takes a
class record's metaclass from `ClassData.metaclass` and the decoder puts it
back there. Task 4 switches both to the header.

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_METACLASS = 8`, `CLASS_METACLASS = 4`, `HASHED_METACLASS = 3`, "next free" comments)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`is_class_tag` `:806-819`, `visit_value` `:872-924`, `visit_frame` `:926-978`, `encode_class_tag` `:1034-1063`, `encode_object` `:1276-1312`, `encode_tag_named_hashes` `:1426-1458`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`decode_class_tag` `:1942-1963`, `decode_graph_section` `:2664-2798`, `rebuild_applications` `:2381-2535`, `metaclass_rows`, `check_class_references` `:2599-2645`, `decode_tag_named_hashes` `:2800-2861`, `DecodeError`)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (`:207-250`, `:332`, `:389`)
- Modify: `crates/ns-runtime/src/interpreter/metaclass.rs` (drop the lazy fallback in `metaclass_of`)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs` (`graph_section_tags_are_fixed`, `no_two_graph_tags_in_a_family_share_a_number`)
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `ClassData.this_class`, `Heap::class_this_class`,
  `Mutator::metaclass_mixin`, `Mutator::metaclass_of` (Task 2).
- Produces: the wire forms; `DecodeError::MetaclassPairing`,
  `DecodeError::ClassRecordAppliesAClassSide`,
  `DecodeError::InstanceOfAClassClass`; `metaclass_of` no longer builds.

**The wire forms.**
- A class reference is one of `CLASS_BUILTIN`, `CLASS_USER_ROOT`,
  `CLASS_RECORD`, `CLASS_ROOT_LEVEL`, and now `CLASS_METACLASS` followed by a
  class reference that must be `CLASS_BUILTIN`, `CLASS_USER_ROOT` or
  `CLASS_ROOT_LEVEL` — a prebuilt class's metaclass. Its nesting depth is
  one: a nested `CLASS_METACLASS` or `CLASS_RECORD` is refused
  (`InvalidTag`), which bounds the recursion.
- The metaclass of a class *record* is an object record: its header class
  tag is `CLASS_BUILTIN` + `Metaclass`'s tag, then its identity hash, then
  body tag `OBJECT_METACLASS` and a single class reference, its class.
- A class record's header class tag names its metaclass record
  (`CLASS_RECORD` + id).
- The closing hash table gains `HASHED_METACLASS` followed by a prebuilt
  class's reference (the same three tags `CLASS_METACLASS` allows), for the
  metaclass of a builtin, of the root class and of a root level whose
  identity hash is nonzero.

- [ ] **Step 1: Pin the tags (failing test)**

In `opcode_tags.rs`: `assert_eq!(OBJECT_METACLASS, 8)`,
`assert_eq!(CLASS_METACLASS, 4)`, `assert_eq!(HASHED_METACLASS, 3)`, and add
each to its family in `no_two_graph_tags_in_a_family_share_a_number`.

Run: `cargo test -p ns-runtime --test opcode_tags` — Expected: compile error.

- [ ] **Step 2: The round-trip tests (failing)**

In `tests/image.rs`, following the neighbouring snapshot tests' helpers
(`snapshot` then `load` of a stateful image whose roots hold values):

```rust
/// A metaclass keeps its identity across a round trip: `Holder class`
/// held in a slot is the decoded class's metaclass, not a second one.
#[test]
fn a_metaclass_in_a_slot_is_the_decoded_class_metaclass() { /* module with a nested class
   `Holder` and a slot `kept` that the entry sets to `Holder class`... */ }
```

Today `Holder class` answers `Object` (the header flips in Task 4), so build
the state from Rust instead: after instantiating, fetch the nested class
object, take `mutator.metaclass_of(nested)`, store it into an instance slot
through `Heap::set_instance_field`, snapshot, load, and assert the loaded
slot's value `same`s `metaclass_of(loaded nested class)` and that
`class_this_class` of it `same`s the loaded nested class. Second test:
`a_builtin_metaclass_round_trips_by_tag` does the same with
`metaclass_of(builtin_class(Integer))` and asserts the loaded value is the
loading interpreter's `Integer` metaclass. Third:
`a_metaclass_identity_hash_survives` asks `ensure_identity_hash` on a
record-class metaclass and on a builtin's metaclass before the snapshot and
compares after.

Run: `cargo test -p ns-runtime --test image metaclass` — Expected: FAIL (the
record path refuses a class-side row once Step 5 lands; today it may decode
a second metaclass — the identity assertion fails).

- [ ] **Step 3: Encoder**

- `is_class_tag`: also `true` for a metaclass whose `this_class` is itself
  prebuilt (builtin, root, root level) — it is named by tag, never entered.
- `visit_value`, for an object whose data is a class: push its
  `ClassData.metaclass` (the header in Task 4) so a class record's metaclass
  record gets an id; for a metaclass (`this_class` is `Some`) push only its
  `this_class`, not `class_references` — a metaclass record writes nothing
  else.
- `encode_class_tag`: a metaclass whose `this_class` is prebuilt →
  `CLASS_METACLASS` then `encode_class_tag(this_class)`; any other metaclass
  → `CLASS_RECORD` + its id (it is a record).
- `encode_object`: a class record's header is
  `encode_class_tag(ClassData.metaclass)` (not `record.class`); a metaclass
  record is written as
  ```rust
          ObjectData::Class(data) if data.this_class.is_some() => {
              writer.write_u8(OBJECT_METACLASS);
              let this_class = data.this_class.expect("checked by the guard");
              encode_class_tag(writer, this_class, context);
          }
  ```
  with the header `CLASS_BUILTIN` + `Metaclass`'s tag.
- `encode_tag_named_hashes`: after the builtins, the root and the platform,
  a `HASHED_METACLASS` entry for each of: every builtin's metaclass, the root
  class's metaclass, each root level's metaclass — when its identity hash is
  nonzero — followed by `encode_class_tag` of its class.
- Remove the comment at `encode.rs:911-914` and rewrite the one at
  `:1176-1198`: a metaclass is never a `OBJECT_CLASS` record any more.

- [ ] **Step 4: Decoder, reading**

- `decode_class_tag`, new arm:
  ```rust
        CLASS_METACLASS => {
            let class = match reader.read_u8()? {
                CLASS_BUILTIN => context.mutator.builtin_class(read_referenced_builtin_class(reader)?),
                CLASS_USER_ROOT => *context.root_class,
                CLASS_ROOT_LEVEL => { /* as the arm above */ }
                tag => return Err(DecodeError::InvalidTag { what: "prebuilt class", tag }),
            };
            context
                .mutator
                .heap
                .class_metaclass(class)
                .ok_or(DecodeError::NotAClassReference)
        }
  ```
  `decode_class_tag` takes `&DecodeContext`, and `class_metaclass` only
  reads, so no `&mut` is needed.
- `decode_graph_section`, body tag `OBJECT_METACLASS`: read one class
  reference (`decode_class_tag`) and build
  ```rust
                let this_class = decode_class_tag(reader, context, &tables)?;
                let class_class = context.mutator.builtin_class(BuiltinClass::Class);
                let mut data = crate::types::ClassData::new(
                    placeholder_application.clone(),
                    None,
                    None,
                    Some(class_class),
                    None,
                );
                data.this_class = Some(this_class);
                pending_metaclasses.push(index);
                ObjectData::Class(Box::new(data))
  ```
- For an `OBJECT_CLASS` record, the decoded header is its metaclass: keep it
  in a `Vec<(usize, ClassReference)>` (`record_metaclasses`, record index →
  decoded header) and store the builtin `Object` in the object's header, as
  today (Task 4 stores the header itself).
- `decode_tag_named_hashes`: `HASHED_METACLASS` reads a prebuilt class
  reference exactly as `CLASS_METACLASS` does and sets that metaclass's
  hash, under the existing duplicate and `u32::MAX` rules.

- [ ] **Step 5: Decoder, rebuilding and verifying**

- `rebuild_applications` no longer applies any class record flat or over
  `Class` as a metaclass: an `OBJECT_CLASS` record whose mixin is a
  class-side row is refused (`DecodeError::ClassRecordAppliesAClassSide`,
  checked where `pending_classes` is built — `metaclass_rows` stays as the
  set to check against, its doc comment rewritten to say it is what a class
  record may *not* apply).
- After the class loop (and before the `installed_order` value pass), each
  pending metaclass record: its `this_class` must be an `OBJECT_CLASS`
  record — `position` holds it, and `class_this_class(this_class)` is `None`
  — else `MetaclassPairing`; the application is
  `apply_mixin(&metaclass_mixin(&pending[this].mixin), Some(&Class's application))`,
  installed with `install_class_application`; its enclosing object and
  declaring class are copied from `this_class`
  (`set_class_lexical_context`).
- The pairing, both ways: every `(record, header)` in `record_metaclasses`
  needs `class_this_class(header)` to `same` the record — else
  `MetaclassPairing`; then `set_class_metaclass(record, header)`. Every
  metaclass record must have been named by exactly one class record's
  header: count them, and a metaclass record named by none or by two is
  `MetaclassPairing`.
- `check_class_references`: a class record's own header is the builtin
  `Object` in this task (unchanged rule); a metaclass record's
  `this_class` is a class object (already implied by the pairing). An
  object record that is *not* a class and whose class's application carries
  `InstantiationRefusal::ClassObject` is `DecodeError::InstanceOfAClassClass`
  — no source can make an instance of `Class`, `Metaclass` or a metaclass.
- Each new `DecodeError` variant gets a message in the enum's `Display` in
  the style of its neighbours.

- [ ] **Step 6: `metaclass_of` stops building**

Every decoded class record now has its metaclass, so `metaclass_of` answers
`self.heap.class_metaclass(class)` and nothing else; delete the fallback
and its comment.

- [ ] **Step 7: `inspect`**

`read_class_reference` (`:207`) gains `CLASS_METACLASS` →
`format!("metaclass of {}", nested)`, reading the nested reference with the
same depth-one rule; the object walk (`:332`) gains `OBJECT_METACLASS` →
"metaclass of <reference>"; the hash table (`:389`) gains `HASHED_METACLASS`.

- [ ] **Step 8: Crafted-image tests**

In `tests/image.rs`, each builds a valid image with one nested class whose
metaclass is reachable (Step 2's fixture), then patches bytes the way the
neighbouring crafted-image tests do, and asserts `Err` with the named error:
- `a_class_record_naming_another_classs_metaclass_is_refused` (two nested
  classes, swap their headers) → `MetaclassPairing`;
- `a_metaclass_record_naming_a_non_class_is_refused` (point the back
  reference at an instance record) → `MetaclassPairing`;
- `a_nested_metaclass_reference_is_refused` (`CLASS_METACLASS` followed by
  `CLASS_METACLASS`) → `InvalidTag`;
- `an_instance_of_metaclass_is_refused` (an instance record whose header is
  `CLASS_BUILTIN` + 22) → `InstanceOfAClassClass`.

If patching a byte in place is impractical for one of them, build the
bytes with the `encode` module's `Writer` directly, as other tests there do.

- [ ] **Step 9: Run and commit**

Run: `cargo test --workspace` — Expected: PASS, Step 2's and Step 8's tests
included.

```bash
git add -A crates
git commit -m "feat: carry metaclasses through the image format"
```

---

### Task 4: A class object's class is its metaclass

The header flips. `Foo class` answers the metaclass, `Foo class class`
answers `Metaclass`, a class object's dispatch id is its metaclass's
instances id, and the class-side special cases in resolution, caching and
stamping go.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (remove `ClassData.metaclass`)
- Modify: `crates/ns-runtime/src/trace.rs` (drop `metaclass`)
- Modify: `crates/ns-runtime/src/heap.rs` (`class_metaclass` reads the header; remove `set_class_metaclass`; new `set_object_class`; `header_dispatch_id` `:1764-1780`; `class_dispatch_ids` `:1804-1816`; `install_class_application` `:1955-1970`; `allocate_self_referential_class` `:845-875`)
- Modify: `crates/ns-runtime/src/mixin.rs:40-87` (remove `DispatchId::class_side` and `DispatchIds.class_side`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mint_dispatch_ids` mints one id; its tests at `:2160-2197`)
- Modify: `crates/ns-runtime/src/interpreter/metaclass.rs` (`attach_metaclass` sets both headers; `metaclass_of` reads the header)
- Modify: `crates/ns-runtime/src/dispatch.rs:39, 339` (`Lookup::Factory` carries its level)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (remove `Chain::PublicClassSide`, `Chain::ProtectedClassSide`, `resolve_class_side`, `resolved_on_class_side`, the `is_class` branches in `resolve_public` and `resolve_lexical`; map `Lookup::Factory`; the chain test at `:730-800`)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`chain_start` `:535-555`; `Target::Factory`'s comment `:714-719`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`decide_factory`'s doc comment `:355-371`)
- Modify: `crates/ns-runtime/src/image/encode.rs`, `decode.rs` (the header instead of `ClassData.metaclass`)
- Test: `crates/ns-runtime/tests/metaclasses.rs`; goldens `goldens/class-side/` (new)

**Interfaces:**
- Consumes: everything from Tasks 2 and 3.
- Produces: `Heap::set_object_class(object, class) -> bool` (restamps the
  object's dispatch id); `Lookup::Factory(ClassReference<'gc>)`;
  `Heap::class_metaclass(class)` answers the header class when its
  `this_class` is `class`, else `None`.

- [ ] **Step 1: The failing tests**

In `tests/metaclasses.rs`:

```rust
/// §3.2: `Foo class` is Foo's metaclass, whose class is `Metaclass`, and
/// `Metaclass class class` is `Metaclass` again.
#[test]
fn the_class_of_a_class_is_its_metaclass() { /* load a module, send `class` to the class
   object through `interpreter.send` (copy how tests call a unary selector on a root),
   and assert the answer `same`s metaclass_of(class); send `class` to that and assert it
   `same`s builtin_class(Metaclass) */ }

/// Class objects of one declaration under two enclosing instances have one
/// dispatch id: their metaclasses share an application.
#[test]
fn class_objects_of_one_declaration_share_a_dispatch_id() { /* as Task 2's application
   test, asserting `heap.dispatch_id(&Value::Object(first)) == heap.dispatch_id(&Value::Object(second))` */ }
```

And two goldens:

`goldens/class-side/the_class_of_a_class_is_its_metaclass.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.2: "Each class is an instance of a unique metaclass.
   All metaclasses are instances of class Metaclass". So a class's class is
   its metaclass, every metaclass's class is `Metaclass`, and `Metaclass`'s
   own metaclass is an instance of `Metaclass` too. *)
class TheClassOfAClass usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Foo = Object new () ()
	class Bar = Object new () ()
	public main: platform args: args = (
		transcript show: (Foo class == Foo class) printString; newLine.
		transcript show: (Foo class == Bar class) printString; newLine.
		transcript show: (Foo class class == Bar class class) printString; newLine.
		transcript show: (Foo class class == Foo class class class class) printString; newLine.
		transcript show: (3 class class class == Foo class class) printString; newLine.
		transcript show: (Foo class == Foo) printString; newLine.
	)
) : ()
```

with `.toml` `stdout = '''\ntrue\nfalse\ntrue\ntrue\ntrue\nfalse\n'''`
(the exact TOML shape of the neighbouring goldens).

`goldens/class-side/a_class_object_cannot_be_made_by_instantiation.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.2 makes every class "an instance of a unique
   metaclass", which a second instance of a metaclass would break. A class
   object is made only by evaluating a declaration, so `Metaclass`, reached
   here as the class of a metaclass, refuses to be instantiated. A metaclass
   itself declares no factory: `Foo class new` is not understood. *)
class AClassObjectCannotBeMade usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Foo = Object new () ()
	public main: platform args: args = (
		transcript show: 'before'; newLine.
		3 class class class new.
		transcript show: 'not reached'; newLine.
	)
) : ()
```

with `status = 1`, `stdout = '''\nbefore\n'''`, `stderr` the uncaught
error line: `Uncaught Newspeak error: cannot instantiate Metaclass: a class
object is made only by evaluating its declaration`. Add a second golden
`a_metaclass_does_not_understand_new.ns` with `Foo class new` and the
`doesNotUnderstand:` error line that the existing error goldens use for a
missing selector (copy its exact wording from
`goldens/errors/` — do not invent it).

Run: `cargo test --workspace` — Expected: the four fail.

- [ ] **Step 2: Headers**

- `Heap::set_object_class(object, class) -> bool`: sets `objects[object].class`
  and restamps its `dispatch_id` with `header_dispatch_id`; `false` for a
  handle out of the heap (mirror `set_class_superclass`'s shape and doc).
- `attach_metaclass`: after allocating the metaclass,
  `set_object_class(metaclass, builtin Metaclass)` and
  `set_object_class(class, metaclass)`; no `set_class_metaclass`.
- `give_existing_class_objects_metaclasses` covers the builtin `Object`,
  whose self-referential header becomes its metaclass like every other's,
  and the builtin `Metaclass`, whose metaclass's header is `Metaclass`.
- `allocate_class_object` keeps the builtin `Object` as the provisional
  header: it is replaced before `new_class_object` returns, except during
  the kernel phase, where the bootstrap pass replaces it.
- Remove `ClassData.metaclass`, its tracing, and `set_class_metaclass`.
  `Heap::class_metaclass(class)`: the header class of `class`, when that
  class's `this_class` `same`s `class`; `None` otherwise (only a class object
  in the kernel phase before its pass).
- `metaclass_of` reads `class_metaclass`.

- [ ] **Step 3: Uniform dispatch ids**

- `header_dispatch_id`: drop the `ObjectData::Class` special case; every
  object's id is its class's `ClassData.dispatch_id`.
- `allocate_self_referential_class`: its `dispatch_id` is its own data's
  instances id when `data` is a class (it is its own class), `NONE`
  otherwise.
- `install_class_application`: the class object's own header id is no
  longer derived from its application; restamp it from its header class
  (`header_dispatch_id`), which for a decoded class record is its metaclass
  record — `stamp_dispatch_ids` afterwards restamps every decoded object
  anyway.
- `mixin.rs`: remove `DispatchId::class_side`, `DispatchIds.class_side` and
  `DispatchIds::NONE`'s second field; `mint_dispatch_ids` advances the
  counter by one and answers `DispatchIds { instances }`; update its doc
  comment and the two tests at `interpreter/mod.rs:2160-2197` (the pair
  assertions become single-id assertions). `class_dispatch_ids` answers
  `DispatchIds { instances: data.dispatch_id }`. Update `ClassData.dispatch_id`'s
  doc comment (no derived class-side id).
- `refresh_immediate_dispatch_ids` is unchanged.

- [ ] **Step 4: Resolution**

- `dispatch.rs`: `Lookup::Factory` becomes `Lookup::Factory(ClassReference<'gc>)`,
  the level whose method table held the entry (the walk has it at `:339`,
  as `Lookup::Primitive` carries its level).
- `resolve.rs`: delete `Chain::PublicClassSide`, `Chain::ProtectedClassSide`,
  `resolve_class_side` and `resolved_on_class_side`, and the class-object
  branches of `resolve_public` (`:112-119`) and `resolve_lexical`
  (`:210-218`): a class object's class is its metaclass now, so the
  ordinary lookup from `class_of(receiver)` searches the class side,
  `Class`, `Behavior` and `Object` in that order — which is what the two
  branches emulated. `resolve_public` and `resolve_lexical` pass the
  receiver (`o` for a lexical send) to a new mapping used instead of
  `resolved` wherever a public or protected lookup's answer is turned into a
  `Resolved`:
  ```rust
  /// §6.3's primary factory is an entry of a metaclass mixin, and it
  /// instantiates the class object the message was sent to -- so it is
  /// honoured only when it was found on that receiver's own class, its
  /// metaclass. Found anywhere else it could only have been put there by a
  /// crafted image (`image::mixins::decode` derives one for a class-side row
  /// alone), and it is refused rather than instantiating whatever received
  /// the send.
  fn resolved_for<'gc>(
      heap: &Heap<'gc>,
      lookup: Lookup<'gc>,
      receiver: Value<'gc>,
      chain: Chain,
  ) -> Resolved<'gc> {
      match (lookup, receiver) {
          (Lookup::Factory(level), Value::Object(class_object))
              if heap.is_class(class_object) && heap.class_of_object(class_object).same(level) =>
          {
              Resolved::Factory { level: class_object, chain }
          }
          (Lookup::Factory(_), _) => Resolved::Unhandled,
          (found, _) => resolved(found, chain),
      }
  }
  ```
  `resolved` keeps `Lookup::Factory(_) => Resolved::Unhandled` for the
  chains that never reach a metaclass's own table (`Private`, `Super`, the
  handler chains). `Resolved::Factory`'s doc comment: `chain` is `Public`
  or `Protected`.
- The resolution test at `resolve.rs:730-800`: `make` and `new` on the
  class object now resolve on `Chain::Public`.
- `inline_cache.rs`: `chain_start` loses the class-side arm; `Target::Factory`'s
  comment says the guard is the receiver's metaclass's instances id.
  `CachedTargetKind` and the polymorphic tables need no change beyond what
  the compiler reports.
- `instantiate.rs`, `decide_factory`'s doc comment: the entry was found on
  the receiver's own metaclass, whose instances id is the guard.
- `resolve_super_does_not_understand`'s doc comment: for a class-side method
  S is `Class`.

- [ ] **Step 5: The image reads the header**

Task 3's transitional paths go: the encoder writes `record.class` for a
class record (it is the metaclass) and pushes `record.class` like any
object's (already done by `visit_value`'s first push — remove the extra
`ClassData.metaclass` push); the decoder stores the decoded header in the
object itself (a class record's is its metaclass record, a metaclass
record's is `Metaclass`) and the pairing check reads `class_of_object`
instead of `record_metaclasses`. `check_class_references`: a class record's
header must be a metaclass record whose `this_class` is it (the pairing),
and a metaclass record's header must be the builtin `Metaclass`
(`DecodeError::MetaclassPairing` otherwise); rewrite the doc comment's
"the shared bootstrap `Object` placeholder" paragraph: the chain from a
class record to a class the graph did not create is at most two links
(class record → its metaclass record → `Metaclass`), which is what bounds
it. Extend Task 3's round-trip tests to assert the loaded class's header
`same`s the loaded slot's metaclass.

- [ ] **Step 6: Goldens**

Run: `cargo test --workspace` — Expected: PASS, including Step 1's tests.
No existing golden output changes (`Foo class printString` still answers
`an Object`: `Class` declares no `printString` until Task 5). If one does,
stop and report it.

- [ ] **Step 7: Commit**

```bash
git add -A crates goldens
git commit -m "feat: a class object's class is its metaclass"
```

---

### Task 5: `Class>>name`, `Metaclass>>name` and `printString`

**Files:**
- Create: `crates/ns-runtime/src/builtins/behavior.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (the module, `Primitive::Behavior` arm)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`Primitive::Behavior(BehaviorPrimitive)`, the enum, two rows appended to `MIRROR`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Class`, `Metaclass`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:667-692` (`NOT_FORWARDERS`)
- Modify: `goldens/class-side/class_side_super_reaches_object.*` → `class_side_super_reaches_class.*`
- Modify: `goldens/cache/cache_class_objects_and_instances_at_one_site.toml`
- Test: new goldens under `goldens/class-side/`

**Interfaces:**
- Produces:
  ```rust
  pub enum BehaviorPrimitive {
      /// A class's name, its mixin's, as a `String`; declines for a metaclass.
      Name,
      /// A metaclass's class; declines for anything else.
      ThisClass,
  }
  ```
  Mirror rows `className:ifFail:` (reported `name`) and
  `metaclassThisClass:ifFail:` (reported `thisClass`), appended at the end
  of `MIRROR` so no row index moves.

- [ ] **Step 1: The failing goldens**

Rename `class_side_super_reaches_object.{ns,toml}` to
`class_side_super_reaches_class.{ns,toml}` (`git mv`), rename the class in
the source to `ClassSideSuperReachesClass`, and change the expected stdout
from `an Object` to `Simple`: §5.10 makes S the superclass of the running
activation's current class, which §3.2 makes `Class` for a metaclass, and
`Class>>printString` answers the class's name. Add a comment at the top of
the source saying so.

`goldens/class-side/a_class_and_its_metaclass_print_their_names.ns`:

```newspeak
Newspeak3
'Test'
(* A class prints as its name and a metaclass as its class's name followed
   by ` class`, as Primordial Soup's `Class>>printString` and
   `Metaclass>>printString` do. `Metaclass` itself is a class. *)
class AClassAndItsMetaclassPrint usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Foo = Object new () ()
	public main: platform args: args = (
		transcript show: Foo printString; newLine.
		transcript show: Foo class printString; newLine.
		transcript show: Foo class class printString; newLine.
		transcript show: Foo class class class printString; newLine.
		transcript show: 3 class printString; newLine.
		transcript show: 3 class class printString; newLine.
		transcript show: Foo new printString; newLine.
	)
) : ()
```

stdout: `Foo`, `Foo class`, `Metaclass`, `Metaclass class`, `Integer`,
`Integer class`, `an Object`.

`goldens/cache/cache_class_objects_and_instances_at_one_site.toml`: the
seven lines after the six `describe` lines become `Plain`,
`the custom class`, `a custom instance`, `Maker`, `an Object`, `Plain`,
`the custom class` — `Plain`, `Maker` and `Plain` again are class objects
that declare no class-side `printString` and now answer `Class`'s; `Plain
new` still answers `Object`'s; `Custom`'s own class-side `printString` still
wins over `Class`'s, since the class side is searched first.

Run: `cargo test --workspace` — Expected: those three fail.

- [ ] **Step 2: The leaves**

`builtins/behavior.rs`:

```rust
//! The two leaves `Kernel.ns`'s `Class` and `Metaclass` forward to: a
//! class's name, and a metaclass's class. Nothing else about a class object
//! is answered at the base level; reflection is a mirror's
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

use crate::builtins::primitive::BehaviorPrimitive;
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BehaviorPrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = receiver else {
        return None;
    };
    if !mutator.heap().is_class(object) {
        return None;
    }
    match primitive {
        BehaviorPrimitive::Name => {
            if mutator.heap().class_this_class(object).is_some() {
                return None;
            }
            let name = mutator.heap().class_application(object)?.mixin.name.to_string();
            Some(Ok(Value::Object(mutator.new_string(name))))
        }
        BehaviorPrimitive::ThisClass => mutator
            .heap()
            .class_this_class(object)
            .map(|this_class| Ok(Value::Object(this_class))),
    }
}
```

Wire it into `builtins::execute` with `.map(wrap_error)` and append the two
rows to `MIRROR`.

- [ ] **Step 3: The kernel methods**

```newspeak
    public class Class = Behavior new () (
        public name = ( ^vmMirror className: self ifFail: [^nil] )
        public printString = ( ^name )
    )
    public class Metaclass = Behavior new () (
        public name = ( ^thisClass name, ' class' )
        public printString = ( ^name )
        (* Primordial Soup's `Metaclass` keeps its class in a slot of this
           name; here it is `ClassData`'s, read through a leaf. *)
        private thisClass = ( ^vmMirror metaclassThisClass: self ifFail: [^nil] )
    )
```

`NOT_FORWARDERS` gains `(BuiltinClass::Class, "printString")`,
`(BuiltinClass::Metaclass, "name")`, `(BuiltinClass::Metaclass, "printString")`,
with the comment that a send to `self` is no leaf.

- [ ] **Step 4: Run and justify every changed golden**

Run: `cargo test --workspace` — Expected: PASS. If a golden other than the
three above changes, it prints a class object: justify the new line against
`Class>>printString`/`Metaclass>>printString` in the commit message, or stop
and report if it is not that.

- [ ] **Step 5: Commit**

```bash
git add -A crates goldens
git commit -m "feat: Class and Metaclass answer their names"
```

---

### Task 6: A class object is a value when its enclosing object is

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs:1380-1390` (`is_a_value`)
- Modify: `crates/ns-runtime/src/builtins/value.rs` (`compare_node`, `hash_node`, `derived_equality`, `derived_hash`, module doc)
- Modify: `crates/ns-runtime/src/builtins/behavior.rs`, `primitive.rs` (three leaves for the Newspeak fallback)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Behavior`, `Metaclass`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`NOT_FORWARDERS`)
- Test: `builtins/value.rs` unit tests; new goldens under `goldens/kernel/`

**Interfaces:**
- Produces: `pub(crate) fn class_object_is_a_value(mutator: &Mutator<'gc>, class: ClassReference<'gc>) -> bool`
  in `instantiate.rs`; `BehaviorPrimitive::{SharesApplication, Enclosing, Superclass}`
  with rows `behavior:sharesApplicationWith:ifFail:` (reported
  `sharesApplicationWith:`), `behaviorEnclosing:ifFail:` (reported
  `enclosingObject`), `behaviorSuperclass:ifFail:` (reported `superclass`).

- [ ] **Step 1: The failing goldens**

`goldens/kernel/a_value_modules_nested_class_is_a_value.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.4: "Module definitions are value objects", and a
   class nested in a value is one too, since its only state is its
   enclosing object (6.1) and 3.1.1 asks exactly that its enclosing objects
   be values. So a value may hold one in a slot, and two equal values
   answer equal nested classes: 3.1.1 makes them the same value, and 6.1
   has the same receiver answer the same class. Their instances are equal
   in turn; two unequal values' nested classes are not. *)
class AValueModulesNestedClass usingPlatform: thePlatform = Object <: Value (| transcript = thePlatform transcript. |) (
	public class Box x: anX = Object <: Value (| public x = anX. |) (
		public class Point = Object <: Value () ()
	)
	public class Holder of: aClass = Object <: Value (| public held = aClass. |) ()
	public main: platform args: args = (
		| one another other |
		one:: Box x: 1.
		another:: Box x: 1.
		other:: Box x: 2.
		transcript show: (one = another) printString; newLine.
		transcript show: (one Point == another Point) printString; newLine.
		transcript show: (one Point = another Point) printString; newLine.
		transcript show: (one Point hash = another Point hash) printString; newLine.
		transcript show: (one Point = other Point) printString; newLine.
		transcript show: (one Point new = another Point new) printString; newLine.
		transcript show: (one Point new = other Point new) printString; newLine.
		transcript show: ((Holder of: one Point) = (Holder of: another Point)) printString; newLine.
		transcript show: (one Point class = another Point class) printString; newLine.
	)
) : ()
```

stdout: `true`, `true`, `true`, `true`, `false`, `true`, `false`, `true`,
`true`. (`==` is `=` for a value class object, as §3.1.1 asks of a value's
identity method.) Verify the module shape against
`goldens/kernel/a_value_over_a_slotless_class_in_a_value_module_is_accepted.ns`
first — nested value classes in a value module, factory arguments in an
immutable slot — and adjust the syntax to what that golden does if it
differs.

`goldens/kernel/a_values_slot_refuses_a_class_that_is_no_value.ns`: an
ordinary (non-value) module declaring a nested class `Plain` and a value
class `Holder of: aClass` (declared by a nested value module in the way the
neighbouring `a_values_slot_must_hold_a_value.ns` sets one up); `Holder of:
Plain` is refused with that golden's error wording, with `Plain` described
as `the class Plain`. Copy the wording from the neighbouring golden's
`.toml`; do not invent it.

Run: `cargo test --workspace` — Expected: both fail.

- [ ] **Step 2: `is_a_value`**

```rust
/// Whether `object` is a value object. An instance is one when its class
/// inherits `Value` (§3.1.1). A class object is one when its enclosing
/// object is (`docs/design/object-model.md`, "Class sides and metaclasses",
/// after SOMns's `SClass.isValue`): a module definition has none and is one
/// (§6.4), a nested class's only state is its enclosing object (§6.1). A
/// metaclass shares its class's enclosing object and so its verdict.
pub(crate) fn is_a_value<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    if mutator.heap().is_class(object) {
        return class_object_is_a_value(mutator, object);
    }
    mutator
        .heap()
        .class_inherits_value(mutator.heap().class_of_object(object))
}

pub(crate) fn class_object_is_a_value<'gc>(mutator: &Mutator<'gc>, class: ClassReference<'gc>) -> bool {
    match mutator.heap().class_enclosing(class) {
        None => true,
        // An enclosing object is always an instance, never a class object,
        // so this does not recurse.
        Some(enclosing) => {
            !mutator.heap().is_class(enclosing)
                && mutator
                    .heap()
                    .class_inherits_value(mutator.heap().class_of_object(enclosing))
        }
    }
}
```

Keep the doc comment's paragraph about `state_is_a_value`.

- [ ] **Step 3: The walk**

In `builtins/value.rs`:
- `derived_equality`: before `compare_node`, compare the two nodes'
  classes. Identical classes go on as today. Otherwise both classes must be
  value class objects (`class_object_is_a_value`), else the answer is
  `false`; when they are, the pair of classes is pushed onto the work stack
  (charged against `NODE_BUDGET` like any pushed pair, declining when the
  budget is out) and the node itself is still compared. `compare_node` loses
  its own class check.
- `compare_node`, `(Value::Object(left), Value::Object(right))`: before the
  symbol and string checks, when both are class objects, answer a new
  `Pair::Classes(left, right)`.
- `derived_equality`, `Pair::Classes(first, second)`: identical → go on;
  insert the pair into `assumed` as `State` does (a cycle through classes
  terminates the same way); both metaclasses (`class_this_class` is `Some`)
  → push the pair of their `this_class`es; either not a value class object →
  `return Some(false)`; applications not `Rc::ptr_eq` → `return Some(false)`;
  else push the pair of enclosing objects (`Value::Nil` for `None`) and the
  pair of superclass class objects (`Value::Nil` for `None`), charging two.
- `hash_node`, a class object: a value class object answers
  `Node::Leaf(text::string_hash(name))`, where `name` is its mixin's name, or
  for a metaclass its `this_class`'s mixin's name followed by `" class"`; a
  class object that is no value declines (`None`) — `Behavior>>hash` then
  answers `Object`'s identity hash.
- Rewrite the module doc: two values are equal when their classes are
  equal — identical, or value class objects equal by their state — and
  their states are; a class object is a node whose state is its
  application, enclosing object and superclass.
- Unit tests beside the existing ones:
  `two_value_class_objects_of_one_declaration_compare_equal`,
  `class_objects_of_one_declaration_under_unequal_values_compare_unequal`,
  `instances_of_equal_class_objects_compare_equal`,
  `a_class_object_that_is_no_value_compares_by_identity`,
  `equal_class_objects_hash_alike` — each built with the file's
  `a_value_fixture`-style helpers, each asserting through `answered_flag` /
  `answered_integer` as the neighbours do.

- [ ] **Step 4: `Behavior`'s methods and the fallback leaves**

`builtins/behavior.rs`, three more arms, each declining for a receiver that
is not a class object:
- `SharesApplication`: `Value::Boolean(Rc::ptr_eq(receiver's application,
  argument's application))`, declining when the argument is no class object;
- `Enclosing`: the enclosing object or `Value::Nil`;
- `Superclass`: the superclass class object or `Value::Nil`.

Rows appended to `MIRROR`. In `Kernel.ns`:

```newspeak
    public class Behavior = Object new () (
        (* A class object that is a value has no identity: two are equal
           when they apply one mixin application to equal enclosing objects
           over equal superclasses (docs/design/object-model.md, "Class
           sides and metaclasses"). The leaf answers every case it can
           decide without sending, identity included for a class object
           that is no value; it declines only where an enclosing object's
           class brings its own `=`. *)
        public = other = ( ^vmMirror value: self sameAs: other ifFail: [^self behaviorEquals: other] )
        public == other = ( ^vmMirror value: self sameAs: other ifFail: [^self behaviorEquals: other] )
        public hash = ( ^vmMirror valueHash: self ifFail: [^super hash] )
        private behaviorEquals: other = (
            (vmMirror behavior: self sharesApplicationWith: other ifFail: [^false]) ifFalse: [^false].
            ((self enclosingOf: self) = (self enclosingOf: other)) ifFalse: [^false].
            ^(self superclassOf: self) = (self superclassOf: other)
        )
        private enclosingOf: subject = ( ^vmMirror behaviorEnclosing: subject ifFail: [^nil] )
        private superclassOf: subject = ( ^vmMirror behaviorSuperclass: subject ifFail: [^nil] )
    )
```

and in `Metaclass`, whose equality is its class's:

```newspeak
        public = other = ( ^vmMirror value: self sameAs: other ifFail: [^(self thisClassOf: self) = (self thisClassOf: other)] )
        public == other = ( ^self = other )
        private thisClassOf: subject = ( ^vmMirror metaclassThisClass: subject ifFail: [^nil] )
```

(`thisClass` from Task 5 becomes `^self thisClassOf: self`, or stays the
forwarder — whichever `NOT_FORWARDERS` then lists correctly.) The value
check of Task 6 Step 2 is not bypassed: `Behavior` does not inherit `Value`,
so none of this touches `apply_mixin`'s identity-method refusal.
`NOT_FORWARDERS` gains `Behavior`'s `behaviorEquals:`, `enclosingOf:`,
`superclassOf:` and `Metaclass`'s `==` and `thisClassOf:`, with the same
comments `Value`'s neighbours carry.

`value:sameAs:`'s leaf now answers for class-object receivers: a class
object that is no value compares by identity (`Decided(same)`), which keeps
`self class == other class` in `Value>>valueEquals:` a leaf with no
activation.

- [ ] **Step 5: Run and commit**

Run: `cargo test --workspace` — Expected: PASS, both new goldens included.

```bash
git add -A crates goldens
git commit -m "feat: a class object is a value when its enclosing object is"
```

---

### Task 7: Documentation

Markdown only: no test run.

**Files:**
- Modify: `docs/design/object-model.md` ("Class sides and metaclasses", "Planned: …", "A value object's four conditions", "The builtin classes", "Decisions")
- Modify: `docs/design/README.md` (the aspect table's "Builtins and the platform" row; the deviations table)
- Modify: `docs/design/core-library.md` (header status, "What exists", "Step 6 also carries the rest of §3.2")
- Modify: `docs/design/dispatch.md` ("The dispatch id", `:1330-1345` "Step 6 makes the stamping uniform", `:1551-1590` flat metaclasses)
- Modify: `docs/design/image-format.md` (the graph section's tags, "What is *not* serialized", "Verification")
- Modify: `docs/design/liveness-and-reflection.md` (where it waits for the reified instance-of relation)
- Modify: `CLAUDE.md` (the crate description's "in §3.2's and §3.1.1's hierarchy" sentence; "The VM mirror is a capability" if it names the refused tags)
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: `object-model.md`**

The documents describe the current state. "Planned: `Behavior`, `Class` and
`Metaclass`" becomes the description of what exists (heading without
"Planned:", status line removed); the paragraphs above it that describe the
lazy metaclass, the flat application with no superclass application, the
metaclass whose superclass is `Object`'s behaviour, and "Deviation from §3.2"
are replaced by it rather than kept beside it. "A value object's four
conditions", Step 4: a class object that is a value is accepted in a value's
slot. "Decisions": one entry dated 2026-09-25 for the nine decisions at the
top of this plan, linking it.

- [ ] **Step 2: The rest**

- `README.md`: the §3.2 deviation row goes; the §3.1.1 row drops "and a
  module definition is not one"; the "Builtins and the platform" row drops
  "no `Class`, `Behavior` or `Metaclass`" and its "missing" column drops
  them.
- `core-library.md`: `Class`, `Behavior` and `Metaclass` are no longer
  missing; "What exists" names them and their protocol; "Step 6 also carries
  the rest of §3.2" says it is done.
- `dispatch.md`: a class object's dispatch id is its metaclass's instances
  id; there is no class-side id and no class-side chain; a factory is
  honoured only on the receiver's own metaclass (`resolved_for`).
- `image-format.md`: `OBJECT_METACLASS`, `CLASS_METACLASS`,
  `HASHED_METACLASS`, builtin tags 20–22 and where they are refused; the
  pairing verification; the amplification figures unchanged and why.
- `CLAUDE.md`: the hierarchy sentence names `Behavior`, `Class` and
  `Metaclass` under `Object`.
- Sweep:
  ```bash
  grep -rn "metaclass_of\|lazily created metaclass\|class_side()\|PublicClassSide\|ProtectedClassSide\|resolve_class_side\|flat.*no superclass application\|class object's own class is the builtin" docs/design docs/PROGRESS.md CLAUDE.md
  ```
  Expected: nothing that describes the old state (a mention of
  `metaclass_of` as it now is — reading the header — may stay).

- [ ] **Step 3: `PROGRESS.md`**

An entry "`Behavior`, `Class` and `Metaclass`" (2026-09-25) in the style of
its neighbours: what exists now, the nine decisions, every golden that
changed and why, the measurement from Task 8 (filled in after Task 8).
"Where the next session starts" names the next piece of work after step 6 —
read the order of work in `PROGRESS.md` for it (the reflection work that
needed the reified instance-of relation, or step 7, whichever the list puts
next) — and says step 6's third plan is done.

- [ ] **Step 4: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: record Behavior, Class and Metaclass as built"
```

---

### Task 8: Staggered gates and measurement

- [ ] **Step 1: The end-of-batch gates** (Git Bash syntax)

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
Expected: every suite passes; every `run-corpus` reports
"ran 6, failed 0, skipped 205". Run each command in the foreground.

- [ ] **Step 2: Measure against `master`**

Performance has the highest priority, so the change is measured before it
is handed on, by `docs/design/execution.md`'s protocol ("What removing the
re-entries moved"): a before-binary from `master` built in a throwaway
directory, the branch's binary, both with the pinned flags
(PowerShell: `$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
`cargo build --release -p ns-cli`, `Remove-Item Env:RUSTFLAGS`), several
builds of each tree pooled for `bench_block.ns`, and
`ns-next bench <source> --iterations 30` over every fixture under
`goldens/bench/` plus one throwaway source that exercises what changed —
nested class objects created per enclosing instance (an eager metaclass
each) and class-side sends — five rounds, alternating the binaries:

`bench_metaclass.ns` (throwaway, not committed):

```
Newspeak3
'Bench'
class BenchMetaclass usingPlatform: platform = Object new (
) (
    class Outer = Object new () (
        public class Inner = Object new () (
        ) : (
            public make = ( ^self new )
        )
    )
    public main: p args: a = (
        | count made |
        count:: 0.
        made:: 0.
        [count < 100000] whileTrue: [
            Outer new Inner make.
            made:: made + 1.
            count:: count + 1].
        p transcript show: made printString; newLine
    )
) : ()
```

Expected: the fixtures level within noise; `bench_metaclass.ns` pays for one
more class object per `Outer new Inner` — report its cost with the numbers.
The removed `is_class` test on every public send may show as a small gain on
`bench_send.ns`. A slowdown beyond noise on any fixture is reported to the
user with the numbers, not accepted silently. Check with `dumpbin /disasm`
on a release build with debug information that no `vm::run::closure` became
a function of its own (`CLAUDE.md`, "Working in this repository").

- [ ] **Step 3: Record the measurement**

Add the numbers to `PROGRESS.md`'s entry from Task 7 Step 3 and to
`execution.md` where this kind of result is recorded. Commit (Markdown only,
no gate):

```bash
git add docs
git commit -m "docs: record what reified metaclasses cost"
```
