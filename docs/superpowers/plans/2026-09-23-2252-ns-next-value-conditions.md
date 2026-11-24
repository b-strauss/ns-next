# Value Conditions ("Plan B") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce all four conditions of `newspeak-spec.pdf` §3.1.1 on value classes strictly, and close the root-clause gap: a module's root may inherit only `Object` or `Value`, or write `Object <: Value`.

**Architecture:** The kernel module and the VM mirror become values. Two new checks run where a nested class object is created (`interpreter/nested.rs`, `after_clause`): no identity method below `Value`, and every enclosing object along the class chain is a value, with an early stop at a class already checked. The mutable-slot check covers the whole chain. A defective root clause is recorded on the root mixin, refused when the root is instantiated, refused by the encoder, and refused by the decoder's root-section reader. The resolver stops binding a root clause's receivers, so that every root defect takes the one deferred path.

**Tech Stack:** Rust 1.98.0, edition 2021; crates `ns-syntax`, `ns-runtime`, `ns-cli`; goldens under `goldens/`.

**Spec:** `docs/design/object-model.md`, "Planned: all four conditions, enforced strictly ("Plan B")". Read it before any task; this plan argues from it.

## Global Constraints

- Read `CLAUDE.md` first. Its invariants all hold; the ones this plan touches most: `vm.rs` holds no dispatch logic and no error text (the checks and their texts live in `interpreter/`); compiling a resolved module can never raise; program outcomes are pinned, never re-derived; never `{:?}` a `Value`, never `.unwrap()`/`.expect()` an `EvaluationResult` or `HostResult` (use the file's `expect_ok`/`expect_error` helpers).
- **Names are written out**: no identifier of one or two characters and no truncated word, in code and in test code (`clippy.toml`'s `min_ident_chars`).
- `#![forbid(unsafe_code)]` stays.
- A Newspeak module written inline in a Rust test is an `indoc! {"…"}`, indented with spaces, the closing `"}` on its own line.
- Comments explain the decision, not the mechanics.
- Conventional commits. Every commit message ends with these two lines, naming the model that actually wrote the commit's code:
  ```
  Co-Authored-By: <model name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01NDmzDCTNbx2nb7bethExyr
  ```
- Work on branch `feature/value-conditions` (it exists; the design commit is on it).
- **Per-commit gate**, in this order, all four green before every commit:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo test --workspace
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 210, failed 0, expected-fail 1, stale 0"
  ```
- Run every command in the foreground.
- A golden's recorded outcome changes only where a task says so, and each such change is argued in the task. A golden whose outcome changes unexpectedly is a defect to investigate, not a file to re-record.
- Error texts are exact. The goldens pin them byte for byte, so copy them from this plan.

## Error texts introduced by this plan

The `{…}` parts are filled in at run time.

| Where | Text |
|---|---|
| root defect, superclass clause | `cannot instantiate {Class}: a module's root class may inherit only Object or Value, or write Object <: Value, and its superclass clause names {clause}` |
| root defect, `<:` clause | `cannot instantiate {Class}: a module's root class may inherit only Object or Value, or write Object <: Value, and its mixin application clause {n} names {clause}` |
| root defect, `Value <: Value` | `cannot instantiate {Class}: a module's root class may inherit only Object or Value, or write Object <: Value, and it applies Value to Value` |
| encoder, root defect | `image encoding cannot serialize {Class}: ` followed by the same tail as the three rows above, starting at `a module's root class …` |
| identity method | `cannot create {Class}: a value class may not define ==, and {Declaring} defines it` |
| own enclosing object | `cannot create {Class}: a value class's enclosing objects must be values, and its enclosing object, {describe}, is not one` |
| a level's enclosing object | `cannot create {Class}: a value class's enclosing objects must be values, and {Level}'s enclosing object, {describe}, is not one` |

`{clause}` is the clause as written, without its message: its prefix (`self `, `super `, `outer Name `) followed by the receiver name, so `self Object`, `Array`, `outer Missing Object`. `{n}` is the clause's 1-based position among the `<:` clauses. `{describe}` is `Heap::describe` of the object, so `an instance of Holder`.

The CLI prints a root instantiation failure as `Uncaught Newspeak error during instantiation: <text>` (`crates/ns-cli/src/entry.rs`, `report_signal`), and a failure inside `main:args:` as `Uncaught Newspeak error: <text>`, both with exit status 1.

---

### Task 1: The root rule, deferred

Every root defect becomes one deferred refusal. The resolver stops refusing root clause receivers; `build_mixin` records the defect on the root mixin; `apply_mixin` folds it and the existing mutable-slot refusal into one field; `decide_instantiation` refuses it; the encoder and the decoder refuse it.

**Files:**
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (`resolve_class`, the two `resolve_superclass_clause` loops near lines 109-122)
- Modify: `crates/ns-runtime/src/mixin.rs` (`Mixin`, `Mixin::empty`, `MixinApplication`, new `RootClauseDefect`, new `InstantiationRefusal`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`load_root_class`, `build_mixin`, delete `resolve_superclass_receiver`, `decide_instantiation`, new `root_clause_defect`, new refusal error)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin`)
- Modify: `crates/ns-runtime/src/dispatch.rs:447-460`, `crates/ns-runtime/src/heap.rs:3093-3105`, `crates/ns-runtime/src/types.rs:586-598` (test constructors of `MixinApplication`)
- Modify: `crates/ns-runtime/src/image/mod.rs` (`encode_describing`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`decode_root_section`)
- Modify: `crates/ns-runtime/tests/image.rs` (`root_section_bytes`, `a_root_level_naming_a_clause_out_of_place_is_refused`, new tests)
- Modify: `crates/ns-cli/tests/build_load.rs` (`every_golden_runs_identically_from_source_and_from_a_binary`)
- Create: `crates/ns-runtime/tests/value_conditions.rs`
- Create: ten goldens under `goldens/errors/`, each a `.ns` and a `.toml` (listed in Step 9)

**Interfaces:**
- Produces, in `crates/ns-runtime/src/mixin.rs`:
  ```rust
  /// Why a module's root class can never be instantiated (§6.1, §3.8): its
  /// clauses name something other than `Object`, `Value` or `Object <: Value`.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum RootClauseDefect {
      /// The superclass clause, as written without its message, when it is
      /// prefixed or names anything but `Object` or `Value`.
      Superclass { clause: String },
      /// A `<:` clause other than one unprefixed `Value`; `index` is 0-based.
      MixinApplication { index: usize, clause: String },
      /// `Value <: Value`: the one `<:` clause is right, the superclass is not.
      ValueOverValue,
  }
  impl RootClauseDefect {
      /// The tail both the instantiation refusal and the encoder print.
      pub fn describe(&self) -> String;
  }
  /// What `decide_instantiation` refuses before it allocates anything.
  #[derive(Debug, Clone)]
  pub enum InstantiationRefusal {
      RootClause(RootClauseDefect),
      MutableValueSlot(MutableValueSlot),
  }
  ```
  `Mixin` gains `pub root_clause_defect: Option<RootClauseDefect>` (`None` in `Mixin::empty`). `MixinApplication` loses `mutable_value_slot` and gains `pub instantiation_refusal: Option<InstantiationRefusal>`.
- Produces, in `crates/ns-runtime/src/interpreter/instantiate.rs`:
  `pub(crate) fn root_clause_defect(declaration: &ast::ClassDeclaration) -> Option<RootClauseDefect>`
  and `pub(crate) fn build_root_mixin(&mut self, declaration: &ast::ClassDeclaration, bindings: &Rc<HashMap<Span, Binding>>) -> Rc<Mixin>` on `Mutator`.

- [ ] **Step 1: Write the failing Rust tests**

Create `crates/ns-runtime/tests/value_conditions.rs`. Copy `load`, `expect_ok` and `expect_error` from `crates/ns-runtime/tests/mixin.rs:112-150` verbatim (the `expect_error` there answers the message `String`), with the imports they need (`indoc::indoc`, `ns_runtime::{HostResult, HostSignal, Interpreter, Root, Symbol, Value}`). Then add:

```rust
/// Every root defect is deferred: the module loads, its class exists and
/// answers its class side, and only an instance is refused -- which is what
/// lets a live system inspect and fix a module whose root is wrong.
#[test]
fn a_module_with_a_defective_root_loads_and_refuses_only_an_instance() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Defective = Array new () () : (
            public answer = ( ^42 )
        )
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let answer = expect_ok(
        interpreter.send(&class, Symbol::intern("answer"), &[]),
        "Defective answer",
    );
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&answer), Value::SmallInteger(42)));
    });
    let message = expect_error(interpreter.instantiate(&class, &[]), "Defective new");
    assert_eq!(
        message,
        "cannot instantiate Defective: a module's root class may inherit only Object or Value, \
         or write Object <: Value, and its superclass clause names Array"
    );
}

/// The forms the rule allows load and instantiate as before.
#[test]
fn every_allowed_root_form_instantiates() {
    for header in [
        "class Allowed = ()",
        "class Allowed = Object new ()",
        "class Allowed = Value new ()",
        "class Allowed = Object <: Value ()",
        "class Allowed = Object new <: Value ()",
    ] {
        let source = format!("Newspeak3\n'Test'\n{header} ()\n");
        let (mut interpreter, class, _resolved) = load(&source);
        expect_ok(interpreter.instantiate(&class, &[]), header);
    }
}

/// A root clause the resolver refused until 2026-09-23 now resolves, and is
/// refused at instantiation like every other root defect.
#[test]
fn an_unknown_root_superclass_is_a_deferred_defect_not_a_resolve_error() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Unknown = Missing new () ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let message = expect_error(interpreter.instantiate(&class, &[]), "Unknown new");
    assert!(message.ends_with("its superclass clause names Missing"), "{message}");
}
```

Also add a unit test module at the bottom of `crates/ns-runtime/src/interpreter/instantiate.rs`, or extend its existing `#[cfg(test)] mod tests` if there is one, for the pure rule:

```rust
#[test]
fn root_clause_defect_classifies_every_root_form() {
    use crate::mixin::RootClauseDefect;
    let defect_of = |header: &str| {
        let source = format!("Newspeak3\n'T'\n{header} ()\n");
        let module = ns_syntax::parse(&source).expect("parse");
        super::root_clause_defect(&module.root_class)
    };
    assert_eq!(defect_of("class R = ()"), None);
    assert_eq!(defect_of("class R = Object new ()"), None);
    assert_eq!(defect_of("class R = Value new ()"), None);
    assert_eq!(defect_of("class R = Object <: Value ()"), None);
    let superclass = |clause: &str| Some(RootClauseDefect::Superclass { clause: clause.to_string() });
    let mixin = |index: usize, clause: &str| {
        Some(RootClauseDefect::MixinApplication { index, clause: clause.to_string() })
    };
    assert_eq!(defect_of("class R = self Object new ()"), superclass("self Object"));
    assert_eq!(defect_of("class R = super Object new ()"), superclass("super Object"));
    assert_eq!(defect_of("class R = outer Missing Object new ()"), superclass("outer Missing Object"));
    assert_eq!(defect_of("class R = Array new ()"), superclass("Array"));
    assert_eq!(defect_of("class R = Top new ()"), superclass("Top"));
    assert_eq!(defect_of("class R = Missing new ()"), superclass("Missing"));
    assert_eq!(defect_of("class R = Object <: self Value ()"), mixin(0, "self Value"));
    assert_eq!(defect_of("class R = Object <: Array ()"), mixin(0, "Array"));
    assert_eq!(defect_of("class R = Object <: Value <: Value ()"), mixin(1, "Value"));
    assert_eq!(defect_of("class R = Value <: Value ()"), Some(RootClauseDefect::ValueOverValue));
}
```

If the crate's unit tests cannot reach `ns_syntax::parse`, check `crates/ns-runtime/Cargo.toml`: `ns-syntax` is a normal dependency, so they can.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test value_conditions`
Expected: a compile error, or a panic at `load`'s `.expect("resolve")` for `Missing`, and a failed `instantiate` expectation for `Array new`, which instantiates today.

- [ ] **Step 3: Add the types to `mixin.rs`**

Add `RootClauseDefect` and `InstantiationRefusal` next to `MutableValueSlot`, exactly as in **Interfaces** above. `describe`:

```rust
impl RootClauseDefect {
    pub fn describe(&self) -> String {
        let rule = "a module's root class may inherit only Object or Value, or write Object <: Value";
        match self {
            RootClauseDefect::Superclass { clause } => {
                format!("{rule}, and its superclass clause names {clause}")
            }
            RootClauseDefect::MixinApplication { index, clause } => {
                format!("{rule}, and its mixin application clause {} names {clause}", index + 1)
            }
            RootClauseDefect::ValueOverValue => format!("{rule}, and it applies Value to Value"),
        }
    }
}
```

Add `pub root_clause_defect: Option<RootClauseDefect>` to `Mixin`, with a doc comment saying that only a module's root mixin built from source carries one, that a decoded mixin never does because the decoder refuses a defective root, and that `apply_mixin` turns it into `InstantiationRefusal::RootClause`. Set it to `None` in `Mixin::empty` and in `build_mixin`'s struct literal.

Replace `MixinApplication::mutable_value_slot` with:

```rust
    /// What `interpreter::instantiate` refuses before allocating an
    /// instance, computed once per application by `Mutator::apply_mixin`:
    /// a root clause defect (on this level or inherited), or -- when the
    /// chain inherits `Value` -- the first mutable slot below `Value`. One
    /// field, so the instantiation path tests one `Option` for both.
    pub instantiation_refusal: Option<InstantiationRefusal>,
```

In the three test constructors (`dispatch.rs:454`, `heap.rs:3097`, `types.rs:590`), replace `mutable_value_slot: None,` with `instantiation_refusal: None,`.

- [ ] **Step 4: Compute the refusal in `apply_mixin`** (`crates/ns-runtime/src/interpreter/mod.rs`, near line 1492)

Keep the current mutable-slot rule for now; Task 4 widens it. Replace the `mutable_value_slot` computation with:

```rust
        // A root clause defect refuses every instance of the root and of
        // anything built on it, so it is inherited like the mutable slot is.
        // It comes first: a root whose chain is wrong has no layout worth
        // checking.
        let instantiation_refusal = mixin
            .root_clause_defect
            .clone()
            .map(crate::mixin::InstantiationRefusal::RootClause)
            .or_else(|| superclass.and_then(|application| application.instantiation_refusal.clone()))
            .or_else(|| {
                if !inherits_value {
                    return None;
                }
                mixin.slots.iter().find(|slot| slot.mutable).map(|slot| {
                    crate::mixin::InstantiationRefusal::MutableValueSlot(
                        crate::mixin::MutableValueSlot {
                            class: mixin.name,
                            slot: slot.name,
                        },
                    )
                })
            });
```

and put `instantiation_refusal` into the `MixinApplication` literal in place of `mutable_value_slot`. Keep the existing comment block above it, reworded to say "refusal" rather than "mutable slot" where needed.

- [ ] **Step 5: Refuse in `decide_instantiation`** (`crates/ns-runtime/src/interpreter/instantiate.rs:398-414`)

Replace the `mutable_value_slot` block with:

```rust
        // Asked before anything is allocated: a class whose instances could
        // never be what §3.1.1 or the root rule demands has no instance to
        // make. The answer was computed once, when the application was built.
        if let Some(application) = application.as_ref() {
            if let Some(refusal) = &application.instantiation_refusal {
                return Err(Signal::Error(instantiation_refusal_error(
                    class,
                    application,
                    refusal,
                )));
            }
        }
```

Add, next to `mutable_value_slot_error` (which stays and is called from here):

```rust
/// The refusal `decide_instantiation` raises, named under the class's own
/// primary factory as both value refusals are.
#[cold]
fn instantiation_refusal_error<'gc>(
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
    refusal: &crate::mixin::InstantiationRefusal,
) -> NewspeakError<'gc> {
    match refusal {
        crate::mixin::InstantiationRefusal::MutableValueSlot(violation) => {
            mutable_value_slot_error(class, application, *violation)
        }
        crate::mixin::InstantiationRefusal::RootClause(defect) => NewspeakError {
            selector: factory_selector(application),
            receiver: Value::Object(class),
            message: format!(
                "cannot instantiate {}: {}",
                application.mixin.name,
                defect.describe()
            ),
        },
    }
}
```

- [ ] **Step 6: The rule and the root's construction** (`crates/ns-runtime/src/interpreter/instantiate.rs`)

Next to `builtin_kind_from_name` add:

```rust
/// §6.1 evaluates a module root's clauses "as if [each] was a top level
/// expression", where §3.8 leaves nothing to send to, so the only classes a
/// root can reach are the ones the rule names: `Object` or `Value` as its
/// superclass, and `Value` applied to `Object`. Decided with the user on
/// 2026-09-23 (`docs/design/object-model.md`, "The root rule"). Syntactic,
/// so it is decided here, once, and deferred to instantiation -- a module
/// whose root is wrong still loads and can be inspected and fixed.
pub(crate) fn root_clause_defect(
    declaration: &ast::ClassDeclaration,
) -> Option<crate::mixin::RootClauseDefect> {
    use crate::mixin::RootClauseDefect;
    let superclass_is_value = match &declaration.superclass {
        None => false,
        Some(clause) => {
            let named = clause.receiver.name.as_str();
            if clause.prefix.is_some() || !matches!(named, "Object" | "Value") {
                return Some(RootClauseDefect::Superclass {
                    clause: clause_as_written(clause),
                });
            }
            named == "Value"
        }
    };
    for (index, clause) in declaration.mixin_applications.iter().enumerate() {
        if index > 0 || clause.prefix.is_some() || clause.receiver.name != "Value" {
            return Some(RootClauseDefect::MixinApplication {
                index,
                clause: clause_as_written(clause),
            });
        }
        if superclass_is_value {
            return Some(RootClauseDefect::ValueOverValue);
        }
    }
    None
}

/// A clause's prefix and receiver, as the program wrote them -- what a
/// refusal names, since the message part is not at fault.
fn clause_as_written(clause: &ast::SuperclassClause) -> String {
    let prefix = match &clause.prefix {
        None => String::new(),
        Some(ast::SuperclassPrefix::SelfPrefix) => "self ".to_string(),
        Some(ast::SuperclassPrefix::SuperPrefix) => "super ".to_string(),
        Some(ast::SuperclassPrefix::Outer(name)) => format!("outer {} ", name.name),
    };
    format!("{prefix}{}", clause.receiver.name)
}
```

Split `build_mixin`: move its body into `fn mixin_of(&mut self, declaration, bindings) -> Mixin`, ending with `compile_mixin_bodies(&mut mixin, …); mixin`. `build_mixin` becomes `Rc::new(self.mixin_of(declaration, bindings))`. Add:

```rust
    /// A module's root mixin: `build_mixin`'s, with the root rule's verdict
    /// recorded on it for `apply_mixin` and the encoder to read.
    pub(crate) fn build_root_mixin(
        &mut self,
        declaration: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Rc<Mixin> {
        let mut mixin = self.mixin_of(declaration, bindings);
        mixin.root_clause_defect = root_clause_defect(declaration);
        Rc::new(mixin)
    }
```

Rewrite `load_root_class` (lines 154-191) so that it no longer resolves receivers by builtin name. Delete `resolve_superclass_receiver` (lines 360-379) and its comment about the gap.

```rust
    pub(crate) fn load_root_class(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let declaration = &resolved.module.root_class;
        let mixin = self.build_root_mixin(declaration, &resolved.bindings);
        // The rule leaves three shapes, and a defective root is built as the
        // first so that its class exists, answers its class side and can be
        // inspected -- only an instance is refused (`apply_mixin` carries the
        // defect into the application).
        let (superclass_kind, applies_value) = match &mixin.root_clause_defect {
            Some(_) => (BuiltinClass::Object, false),
            None => {
                let names_value = declaration
                    .superclass
                    .as_ref()
                    .is_some_and(|clause| clause.receiver.name == "Value");
                let superclass_kind = if names_value {
                    BuiltinClass::Value
                } else {
                    BuiltinClass::Object
                };
                (superclass_kind, !declaration.mixin_applications.is_empty())
            }
        };
        let superclass = Some(self.builtin_class(superclass_kind));
        let applied: Vec<(usize, Value<'gc>)> = if applies_value {
            vec![(0, Value::Object(self.builtin_class(BuiltinClass::Value)))]
        } else {
            Vec::new()
        };
        let superclass =
            match super::nested::build_applied_levels(self, &mixin, superclass, &applied) {
                Ok(top) => top,
                Err(_) => panic!("internal: every builtin class is a class object"),
            };
        let super_application = superclass
            .as_ref()
            .and_then(|superclass| self.heap.class_application(*superclass));
        let application = self.apply_mixin(&mixin, super_application.as_ref());
        // `enclosing`/`declaring_class` are both `None` -- the module's root
        // class is the outermost level, so no object encloses it and no
        // declaration contains it. Every other class object in the system
        // comes from `interpreter::nested`, which passes both.
        self.new_class_object(application, None, None, superclass)
    }
```

`build_applied_levels` answers `superclass` itself when `applied` is empty. Check its current behaviour: it returns `Ok(below)` with `below = superclass`, so that holds.

- [ ] **Step 7: The resolver stops binding root clause receivers** (`crates/ns-runtime/src/resolver/mod.rs:109-122`)

Guard both loops:

```rust
        // A module root's clauses are decided by the root rule, deferred to
        // instantiation (`interpreter::instantiate::root_clause_defect`), so
        // they bind nothing here: a receiver with no binding lowers to
        // `LoadBuiltinClass` or a deferred `UnresolvedIdentifier`, and a
        // root's clause receiver is never run. Refusing `Missing` here while
        // `Array` ran was one rule on two paths.
        if !enclosing.is_empty() {
            if let Some(clause) = &declaration.superclass {
                self.resolve_superclass_clause(clause, enclosing);
            }
            for clause in &declaration.mixin_applications {
                self.resolve_superclass_clause(clause, enclosing);
            }
        }
```

Keep the existing comments above the two loops, moved inside the guard. Resolving the clause *message* arguments further down is unchanged: they run in the root's initializer and see its factory parameters.

- [ ] **Step 8: Encoder and decoder**

In `crates/ns-runtime/src/image/mod.rs`, at the start of `encode_describing`:

```rust
    // A root that can never be instantiated could only fail as an artifact,
    // and refusing it here keeps the format from needing a way to say so.
    if let Some(application) = heap.class_application(root_class) {
        if let Some(defect) = &application.mixin.root_clause_defect {
            return Err(format!(
                "image encoding cannot serialize {}: {}",
                application.mixin.name,
                defect.describe()
            ));
        }
    }
```

In `crates/ns-runtime/src/image/decode.rs`, `decode_root_section`: after reading `superclass`, refuse a tag other than `Object` or `Value`; in the level loop, refuse a level whose builtin is not `Value`, a second level, and a level over a superclass other than `Some(BuiltinClass::Object)`:

```rust
    let superclass = if reader.read_boolean()? {
        let kind = read_program_builtin_class(reader)?;
        if !matches!(kind, BuiltinClass::Object | BuiltinClass::Value) {
            return Err(DecodeError::InvalidTag {
                what: "builtin class a module's root may inherit",
                tag: crate::image::encode::builtin_class_tag(kind),
            });
        }
        Some(kind)
    } else {
        None
    };
    // … existing clause_count / count / levels setup …
    for _ in 0..count {
        let index = reader.read_u32()?;
        if index < next_allowed
            || index as usize >= clause_count
            || !levels.is_empty()
            || superclass != Some(BuiltinClass::Object)
        {
            return Err(DecodeError::BadMixinApplicationClause { row: 0, index });
        }
        next_allowed = index + 1;
        let kind = read_program_builtin_class(reader)?;
        if kind != BuiltinClass::Value {
            return Err(DecodeError::InvalidTag {
                what: "builtin class a module's root may apply",
                tag: crate::image::encode::builtin_class_tag(kind),
            });
        }
        levels.push((index, kind));
    }
```

Extend the function's doc comment with one sentence: the reader refuses what the root rule forbids, so a crafted artifact cannot bring back a root the encoder would refuse. Check `DecodeError::InvalidTag`'s field types and adapt the `tag` expression to them.

In `crates/ns-runtime/tests/image.rs`:
- `root_section_bytes` gets a `superclass: Option<BuiltinClass>` parameter and writes it as `encode_root_class` does: `write_boolean(true)` followed by `write_u8(encode::builtin_class_tag(kind))`, or `write_boolean(false)`. Update every caller.
- `a_root_level_naming_a_clause_out_of_place_is_refused`: its source `Object new <: Value <: Value` is now a root defect, so `snapshot_of` would fail. Use `class Root = Object new <: Value () ()`. Keep the cases the one-clause table can express: `[(0, value)]` accepted with `superclass: Some(BuiltinClass::Object)`; `[(1, value)]` refused with index 1; `[(0, value), (0, value)]` refused with index 0; the `u32::MAX` count case; the mirror-tag case. All level cases pass `Some(BuiltinClass::Object)` as the superclass.
- Add `a_root_section_the_root_rule_forbids_is_refused`, with the same source and helper:

```rust
#[test]
fn a_root_section_the_root_rule_forbids_is_refused() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let decoded = mixin_table_of(&snapshot_of(source));
    let read = |superclass: Option<BuiltinClass>, levels: &[(u32, u8)]| {
        decode::decode_root_section(
            &mut decode::Reader::new(&root_section_bytes(superclass, levels)),
            &decoded,
        )
    };
    let tag = encode::builtin_class_tag;
    assert_eq!(
        read(Some(BuiltinClass::Array), &[]),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class a module's root may inherit",
            tag: tag(BuiltinClass::Array),
        })
    );
    assert_eq!(
        read(Some(BuiltinClass::Object), &[(0, tag(BuiltinClass::Array))]),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class a module's root may apply",
            tag: tag(BuiltinClass::Array),
        })
    );
    assert_eq!(
        read(Some(BuiltinClass::Value), &[(0, tag(BuiltinClass::Value))]),
        Err(decode::DecodeError::BadMixinApplicationClause { row: 0, index: 0 })
    );
    assert_eq!(
        read(None, &[(0, tag(BuiltinClass::Value))]),
        Err(decode::DecodeError::BadMixinApplicationClause { row: 0, index: 0 })
    );
}
```

- Add `the_encoder_refuses_a_root_the_rule_forbids`:

```rust
#[test]
fn the_encoder_refuses_a_root_the_rule_forbids() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Array new () ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let (snapshot, build) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let origin = Origin::File(PathBuf::from("t.ns"));
        (
            image::snapshot(mutator, None, &origin, class, &platform, &[]),
            image::build(mutator.heap(), None, &origin, class, Symbol::intern("main:args:")),
        )
    });
    let expected = "image encoding cannot serialize Root: a module's root class may inherit only \
                    Object or Value, or write Object <: Value, and its superclass clause names Array";
    assert_eq!(snapshot.err().as_deref(), Some(expected));
    assert_eq!(build.err().as_deref(), Some(expected));
}
```

- [ ] **Step 9: The root goldens**

Each is under `goldens/errors/`. Each `.ns` opens with `Newspeak3`, `'Test'` and a comment citing §6.1 ("as if it was a top level expression … will almost certainly fail") and §3.8, stating the rule and saying that the refusal is deferred to instantiation. The body is always:

```newspeak
class <Name> usingPlatform: platformAtCreation = <CLAUSES> () (
    public main: platform args: arguments = (
        platform transcript show: 'unreachable'; newLine
    )
) : ()
```

Each `.toml` is:

```toml
status = 1
stderr = '''
Uncaught Newspeak error during instantiation: cannot instantiate <Name>: <tail>
'''
```

| File stem | `<Name>` | `<CLAUSES>` | `<tail>` |
|---|---|---|---|
| `root_superclass_prefixed_with_self_is_refused` | `RootPrefixedWithSelf` | `self Object new` | `a module's root class may inherit only Object or Value, or write Object <: Value, and its superclass clause names self Object` |
| `root_superclass_prefixed_with_outer_is_refused` | `RootPrefixedWithOuter` | `outer Missing Object new` | `… and its superclass clause names outer Missing Object` |
| `root_superclass_naming_an_unknown_class_is_refused` | `RootNamesAnUnknownClass` | `Missing new` | `… and its superclass clause names Missing` |
| `root_superclass_naming_another_builtin_is_refused` | `RootNamesAnotherBuiltin` | `Array new` | `… and its superclass clause names Array` |
| `root_superclass_naming_top_is_refused` | `RootNamesTop` | `Top new` | `… and its superclass clause names Top` |
| `root_mixin_prefixed_with_self_is_refused` | `RootMixinPrefixedWithSelf` | `Object <: self Value` | `… and its mixin application clause 1 names self Value` |
| `root_mixin_naming_another_builtin_is_refused` | `RootMixinNamesAnotherBuiltin` | `Object <: Array` | `… and its mixin application clause 1 names Array` |
| `root_mixin_naming_an_unknown_class_is_refused` | `RootMixinNamesAnUnknownClass` | `Object <: Missing` | `… and its mixin application clause 1 names Missing` |
| `root_applying_value_to_value_is_refused` | `RootAppliesValueToValue` | `Value <: Value` | `… and it applies Value to Value` |
| `root_applying_two_mixins_is_refused` | `RootAppliesTwoMixins` | `Object <: Value <: Value` | `… and its mixin application clause 2 names Value` |

In each `.toml`, `…` stands for the full rule text `a module's root class may inherit only Object or Value, or write Object <: Value,`. Write it out in full.

- [ ] **Step 10: The build sweep accepts a refused root** (`crates/ns-cli/tests/build_load.rs`, the `if !built` branch)

```rust
            if !built {
                // A golden whose root the root rule forbids cannot be built
                // (`image::encode_describing`); `run` must then fail on the
                // same defect, at instantiation.
                if build_error.contains("a module's root class may inherit only") {
                    let (run_ok, _, run_error) = ns_next(&[
                        "run".as_ref(),
                        path.as_os_str(),
                        "--lowering".as_ref(),
                        lowering.as_ref(),
                    ]);
                    let tail = build_error
                        .split("a module's root class")
                        .nth(1)
                        .expect("the build error names the rule");
                    assert!(
                        !run_ok && run_error.contains(tail.trim_end()),
                        "run of {} must fail on the defect the build refused: {run_error}",
                        path.display()
                    );
                    continue;
                }
                // … the existing `--entry` assertion, unchanged …
            }
```

`snapshot_load.rs` needs no change: `snapshot` instantiates first, so it fails exactly where `run` does.

- [ ] **Step 11: Run the tests**

Run: `cargo test -p ns-runtime --test value_conditions`, then `cargo test -p ns-runtime root_clause_defect`, then the whole per-commit gate.
Expected: all green, including every golden under both lowerings and the build and snapshot sweeps.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "feat: defer every module root clause the root rule forbids to instantiation"
```

---

### Task 2: Kernel and the VM mirror become values

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns:19`
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`superclass_of_kind`, `fill_builtin_classes`, `wire_kernel_in`, `check_kernel_shape`, tests)

**Interfaces:**
- Consumes: `build_applied_levels(mutator, &Rc<Mixin>, Option<ClassReference>, &[(usize, Value)]) -> Result<Option<ClassReference>, Signal>` (`interpreter/nested.rs:325`); `root_clause_defect` (Task 1).
- Produces: the kernel instance and the mirror answer `true` from `instantiate.rs`'s `is_a_value`. Task 6 relies on this.

- [ ] **Step 1: Write the failing tests** in `interpreter/kernel.rs`'s `mod tests`

```rust
    /// §3.1.1 asks a value's enclosing objects to be values, and every
    /// builtin value class is nested in the kernel, so the kernel instance
    /// is one -- `Object <: Value`, as SOMns's `Kernel.ns` declares it --
    /// and so is the mirror its one slot holds, by the ordinary rule.
    #[test]
    fn the_kernel_and_the_mirror_are_values() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_class = mutator.heap().class_of_object(kernel);
            assert!(mutator.heap().class_inherits_value(kernel_class));
            let Value::Object(mirror) = mutator.vm_mirror() else {
                panic!("the mirror is an object");
            };
            let mirror_class = mutator.heap().class_of_object(mirror);
            assert!(mutator.heap().class_inherits_value(mirror_class));
        });
    }

    /// The kernel's `Value` level is an applied level like any other, and
    /// it too is enclosed by the kernel instance: a method of `Value` run on
    /// the kernel reaches `vmMirror` through it.
    #[test]
    fn the_kernels_value_level_is_enclosed_by_the_kernel() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_class = mutator.heap().class_of_object(kernel);
            let level = mutator
                .heap()
                .class_superclass(kernel_class)
                .expect("the kernel class has its Value level");
            assert!(mutator.heap().class_is_applied_level(level));
            assert!(mutator
                .heap()
                .class_enclosing(level)
                .is_some_and(|enclosing| enclosing.same(kernel)));
            assert!(mutator
                .heap()
                .class_declaring(level)
                .is_some_and(|declaring| declaring.same(kernel_class)));
        });
    }
```

Add `(BuiltinClass::VmMirror, true)` to `every_value_classs_application_inherits_value`'s table.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime kernel::tests`
Expected: FAIL. The kernel class does not inherit `Value`, and the mirror class answers `false`.

- [ ] **Step 3: `Kernel.ns`**

Change line 19 to `class Kernel vmMirror: theMirror = Object <: Value (`. Extend the file's header comment by one sentence: the kernel declares itself a value so that every builtin value class, nested here, meets §3.1.1's enclosing-object condition, as SOMns's kernel does.

- [ ] **Step 4: The mirror's class below `Value`** (`superclass_of_kind`, `interpreter/kernel.rs:309`)

Return `Some(BuiltinClass::Value)` for `VmMirror`, and update the doc comment: the mirror's class sits under `Value`, so the mirror is a value by the ordinary rule, with no slots and no `==`, as SOMns asserts (`vm/ObjectSystem.java`, `assert vmMirror.isValue()`). `fill_order` sorts by depth, so `Value` is still filled first.

- [ ] **Step 5: The kernel class gets its `Value` level** (`fill_builtin_classes`, the last four lines)

```rust
        // `Object <: Value`: the level is built as a user root's is, from
        // the filled `Value`, and its lexical context is set once the kernel
        // instance exists (`wire_kernel_in`), as every builtin class's is.
        assert!(
            super::instantiate::root_clause_defect(declaration).is_none()
                && declaration.mixin_applications.len() == 1,
            "internal: Kernel.ns's root must be `Object <: Value`"
        );
        let object = self.builtin_class(BuiltinClass::Object);
        let value = self.builtin_class(BuiltinClass::Value);
        let level = match super::nested::build_applied_levels(
            self,
            &root_mixin,
            Some(object),
            &[(0, Value::Object(value))],
        ) {
            Ok(Some(level)) => level,
            Ok(None) | Err(_) => panic!("internal: the kernel's Value level is a class object"),
        };
        let level_application = self.heap.class_application(level);
        let application = self.apply_mixin(&root_mixin, level_application.as_ref());
        self.new_class_object(application, None, None, Some(level))
```

`root_mixin` comes from `self.build_mixin(declaration, …)` at the top of the function, and `declaration` is `&resolved.module.root_class`. Both exist already.

- [ ] **Step 6: Wire the level in** (`wire_kernel_in`, after the builtin loop, before `self.kernel = …`)

```rust
        // The kernel's own `Value` level was built before the kernel
        // instance existed, over `Value`'s then-empty context.
        let mut level = self.heap.class_superclass(kernel_class);
        while let Some(class) = level {
            if !self.heap.class_is_applied_level(class) {
                break;
            }
            if !self
                .heap
                .set_class_lexical_context(class, Some(kernel), Some(kernel_class))
            {
                unreachable!("internal: an applied level is a class object");
            }
            level = self.heap.class_superclass(class);
        }
```

- [ ] **Step 7: Run the tests and the gate**

Run: `cargo test -p ns-runtime kernel::tests`, then the per-commit gate.
Expected: all green, and no golden changes. The kernel's instantiation now stores the mirror into a slot of a value, which `check_value_slot` accepts because the mirror's class inherits `Value`.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: make the kernel module and the VM mirror values"
```

---

### Task 3: Every module declaring a value class becomes a value

Legal today, and it changes no outcome. It is what Task 6's enclosing-object check needs.

**Files:**
- Modify: `goldens/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class.ns`
- Modify: `goldens/errors/native_that_fails_on_its_receiver_falls_to_objects.ns`
- Modify: `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns`
- Modify: `goldens/kernel/a_user_value_class_compares_by_its_slots.ns`
- Modify: `goldens/kernel/a_value_class_refuses_a_mutable_slot.ns`
- Modify: `goldens/kernel/a_value_has_no_identity.ns`
- Modify: `goldens/kernel/a_values_nested_class_is_not_part_of_its_state.ns`
- Modify: `goldens/kernel/a_values_slot_must_hold_a_value.ns`
- Modify: `crates/ns-runtime/src/builtins/value.rs:605` (`a_value_fixture`)
- Modify: `crates/ns-runtime/tests/instantiate.rs:604` (`a_refused_slot_names_the_factory_the_program_sent`)
- Modify: `crates/ns-runtime/tests/image.rs:8864` (`a_nested_level_applying_a_builtin_mixin_round_trips_as_a_record`)

**No `.toml` changes.** Every recorded outcome stays byte for byte. If one moves, stop and investigate.

- [ ] **Step 1: The two `Text` goldens and the `errors/` one**

`errors/native_that_fails_on_its_receiver_falls_to_objects.ns` has no slots. Change its header to `class Sub usingPlatform: platform = Object <: Value (`.

`cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class.ns` holds the transcript in a slot. Change its header to `class CacheFailingPrimitive usingPlatform: thePlatform = Object <: Value () (`, removing the slot, and read the transcript into a temporary at the top of `main:args:`:

```newspeak
    public main: platform args: args = (
        | text transcript |
        transcript:: platform transcript.
        text:: Text new.
```

The rest of the method stays as it is. Its sends of `transcript show:` are the same call sites, so the cache behaviour the golden pins is unchanged.

In both files, add a paragraph to the comment: the module is declared a value (`Object <: Value`, §3.1.1) because it declares a value class, `Text` below `String`, and a value class's enclosing object must be a value; the transcript is therefore read where it is used, never held in a slot.

- [ ] **Step 2: The kernel goldens with helper methods: a nested `Script`**

`kernel/a_user_value_class_compares_by_its_slots`, `kernel/a_value_has_no_identity`, `kernel/a_values_nested_class_is_not_part_of_its_state` and `kernel/a_subclass_of_a_builtin_falls_to_objects_methods` hold `| transcript = platform transcript. |` in a slot and have helper methods that use it. Apply this transformation:

1. Header: `class X usingPlatform: platform = Object new (` followed by `| transcript = platform transcript. |` and `) (` becomes `class X usingPlatform: platformAtCreation = Object <: Value () (`.
2. The value classes and other nested classes stay where they are, at module level.
3. Every former module method except `main:args:` moves, unchanged, into a new nested class declared after the other nested classes:
   ```newspeak
       class Script on: aTranscript = Object new (
           | transcript = aTranscript. |
       ) (
           …the former module methods, unchanged…
       )
   ```
   `Script` is no value, so it may hold the transcript. Its methods reach the module's nested classes (`Point`, `Text`, `MyInteger` …) through implicit-receiver sends, lexically, exactly as before.
4. `main:args:` becomes a one-line delegation. Its former body moves into `Script` as a method `run`, with `ignoredPlatform`/`p` no longer needed:
   ```newspeak
       public main: platform args: arguments = (
           (Script on: platform transcript) run
       )
   ```
5. Add to the golden's comment: the module is a value (`Object <: Value`) because §3.1.1 asks a value class's enclosing object to be one; the transcript lives in `Script`, which is no value.

Worked example, `kernel/a_value_has_no_identity.ns` after the change (the comment above `class` stays, extended by point 5):

```newspeak
class ValueIdentity usingPlatform: platformAtCreation = Object <: Value () (
    public class Text = String new () ()
    class Script on: aTranscript = Object new (
        | transcript = aTranscript. |
    ) (
        report: label for: first and: second = (
            transcript show: label; show: ' '; show: (first = second) printString;
                show: ' '; show: (first == second) printString; newLine
        )
        public run = (
            report: 'String' for: 'a' and: 'a'.
            report: 'Character' for: "a" and: "a".
            report: 'Symbol' for: #a and: #a.
            report: 'Integer' for: 3 and: 3.
            report: 'Nil' for: nil and: nil.
            report: 'Float/Integer' for: 1.0 and: 1.
            report: 'Integer/Float' for: 1 and: 1.0.
            report: 'Text' for: Text new and: Text new.
        )
    )
    public main: platform args: arguments = (
        (Script on: platform transcript) run
    )
) : ()
```

`a_subclass_of_a_builtin_falls_to_objects_methods` keeps its `MyArray`, `MyOrderedCollection` and `MyBlock`, which are no values. They may sit in a value module: only a value class is asked about its enclosing object.

- [ ] **Step 3: The kernel goldens without helper methods**

`kernel/a_value_class_refuses_a_mutable_slot` and `kernel/a_values_slot_must_hold_a_value` use the transcript only in `main:args:`. Change the header to `class X usingPlatform: platformAtCreation = Object <: Value () (` and rename `main:`'s first parameter from `ignoredPlatform` to `platform`. Its body reads `platform transcript` where it read `transcript`, so `transcript show: 'before'; newLine.` becomes `platform transcript show: 'before'; newLine.`. Extend the comment as in Step 2, point 5.

- [ ] **Step 4: The Rust inline modules**

- `builtins/value.rs`, `a_value_fixture`: `class ValueFixture = Object new () (` becomes `class ValueFixture = Object <: Value () (`.
- `tests/instantiate.rs`, `a_refused_slot_names_the_factory_the_program_sent`: `class Holders = Object new () (` becomes `class Holders = Object <: Value () (`.
- `tests/image.rs`, `a_nested_level_applying_a_builtin_mixin_round_trips_as_a_record`: `class Host usingPlatform: p = Object new () (` becomes `class Host usingPlatform: p = Object <: Value () (`. The test's `inspect` assertion looks for `class mixin builtin Value`, which the root's level now also produces. It still holds, and it still names the nested level's record. Check that it passes. If `render_full` now lists the root level first and an assertion depends on order, keep the assertion about `Point`'s level and make it name that level.

Then search for any other inline module with a value class nested in an ordinary module:
`rg -n "= *(Value|String|Symbol|Character|Boolean|True|False|Number|Integer|Float|Nil) new|<: *Value" crates --glob '*.rs'`. For every hit that is a nested declaration (indented inside another class) whose module header is not `Object <: Value`, apply the same header change. Hits at the root of their module need nothing.

- [ ] **Step 5: Run the gate**

Run: the per-commit gate.
Expected: all green; `git diff --stat goldens` lists only `.ns` files.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test: declare every module that declares a value class a value itself"
```

---

### Task 4: The mutable-slot check covers the whole chain

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication`: new `first_mutable_slot`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin`)
- Modify: the three test constructors (`dispatch.rs`, `heap.rs`, `types.rs`)
- Create: `goldens/kernel/a_mutable_slot_above_the_value_level_is_refused.ns` and `.toml`
- Test: `crates/ns-runtime/tests/value_conditions.rs`

**Interfaces:**
- Produces: `MixinApplication::first_mutable_slot: Option<MutableValueSlot>`, the first mutable slot anywhere in the chain, superclass levels first, recorded whether or not the chain inherits `Value`.

- [ ] **Step 1: Write the failing golden**

`goldens/kernel/a_mutable_slot_above_the_value_level_is_refused.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1: a value object's slots "are immutable", all of
   them -- not only the ones declared below `Value`. 6.2 lets `<: Value` put
   `Value`'s level below an arbitrary class, so a class above that level can
   bring a mutable slot into a value's instances: `Counted` inherits `Value`
   through its `<:` clause, and `Counting`'s `count` is one of its slots.

   `Counting` alone is no value and keeps its mutable slot legally; only an
   instance of `Counted` is refused, where it would come into being. *)
class MutableSlotAboveValue usingPlatform: platformAtCreation = Object <: Value () (
    public class Counting = Object new (
        | public count ::= 0. |
    ) ()
    public class Counted = Counting new <: Value () ()
    public main: platform args: arguments = (
        platform transcript show: Counting new count printString; newLine.
        platform transcript show: Counted new count printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
0
'''
stderr = '''
Uncaught Newspeak error: cannot instantiate Counted: a value object is deeply immutable, and Counting declares the mutable slot count
'''
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL for the new golden. Today `Counted new count` prints `0`.

- [ ] **Step 3: Widen the computation**

Add to `MixinApplication`:

```rust
    /// The first mutable slot anywhere in this application's chain, whether
    /// or not the chain inherits `Value`: a `<:` clause can put `Value`'s
    /// level below a class that declares one, and the instances of every
    /// class built on that level carry the slot. `instantiation_refusal`
    /// turns it into a refusal only where the chain inherits `Value`, so a
    /// class with no value in its chain keeps its mutable slots legally.
    pub first_mutable_slot: Option<MutableValueSlot>,
```

In `apply_mixin`:

```rust
        let first_mutable_slot = superclass
            .and_then(|application| application.first_mutable_slot)
            .or_else(|| {
                mixin.slots.iter().find(|slot| slot.mutable).map(|slot| {
                    crate::mixin::MutableValueSlot {
                        class: mixin.name,
                        slot: slot.name,
                    }
                })
            });
```

The last `or_else` of `instantiation_refusal` becomes `.or_else(|| first_mutable_slot.filter(|_| inherits_value).map(crate::mixin::InstantiationRefusal::MutableValueSlot))`. Add `first_mutable_slot` to the struct literal, and `first_mutable_slot: None` to the three test constructors. Update the comment block above the computation: why the whole chain, and why the refusal still depends on `inherits_value`.

- [ ] **Step 4: Run the gate**

Expected: all green. The existing `kernel/a_value_class_refuses_a_mutable_slot` keeps its text, because its slot is still the first mutable one.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: refuse a value whose chain has a mutable slot above the Value level"
```

---

### Task 5: A value class may not define `==`

**Files:**
- Modify: `crates/ns-runtime/src/symbol.rs` (well-known symbol `IDENTICAL`)
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication::identity_method`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin`)
- Modify: the three test constructors
- Create: `crates/ns-runtime/src/interpreter/value_class.rs` (the class-creation checks; Task 6 extends it)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`pub(crate) mod value_class;`)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`after_clause`)
- Create: `goldens/kernel/a_value_class_defining_identity_is_refused.ns`/`.toml`, `goldens/kernel/an_identity_method_from_an_applied_mixin_is_refused.ns`/`.toml`

**Interfaces:**
- Produces: `Symbol::IDENTICAL` (`"=="`); `MixinApplication::identity_method: Option<Symbol>`, the name of the first level below `Value`, `Value`'s own excluded, that declares an instance-side `==`; and in `interpreter/value_class.rs`:
  ```rust
  pub(crate) fn check_value_class<'gc>(
      mutator: &Mutator<'gc>,
      name: Symbol,
      application: &MixinApplication,
      enclosing: ObjectReference<'gc>,
      superclass: ClassReference<'gc>,
  ) -> Result<(), Signal<'gc>>
  ```
  Task 5 checks only the identity method in it; Task 6 adds the enclosing objects.

- [ ] **Step 1: Write the failing goldens**

`goldens/kernel/a_value_class_defining_identity_is_refused.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1: a value's class "inherits from class Value and
   does not define an identity method (==)". `Value`'s own `==` answers what
   its `=` does, which is what makes a value globally unique; a class that
   redefines it would break that silently. The refusal is raised where the
   class object is created, as SOMns checks a value class's conditions
   (InstantiationNode), so the line before it still prints. *)
class ValueDefiningIdentity usingPlatform: platformAtCreation = Object <: Value () (
    public class Odd = Value new () (
        public == other = ( ^false )
    )
    public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        platform transcript show: Odd new printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
before
'''
stderr = '''
Uncaught Newspeak error: cannot create Odd: a value class may not define ==, and Odd defines it
'''
```

`goldens/kernel/an_identity_method_from_an_applied_mixin_is_refused.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1 and 6.2: a level a `<:` clause adds below `Value`
   is part of the value class, so an `==` its mixin declares is one the class
   defines. `Identity` alone is no value and may declare `==`; `Mixed`
   applies it over `Value` and is refused where it is created. *)
class IdentityFromAMixin usingPlatform: platformAtCreation = Object <: Value () (
    public class Identity = Object new () (
        public == other = ( ^true )
    )
    public class Mixed = Value new <: Identity () ()
    public main: platform args: arguments = (
        platform transcript show: (Identity new == Identity new) printString; newLine.
        platform transcript show: Mixed new printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
true
'''
stderr = '''
Uncaught Newspeak error: cannot create Mixed: a value class may not define ==, and Identity defines it
'''
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL for both new goldens.

- [ ] **Step 3: The symbol and the per-application answer**

In `symbol.rs`'s `well_known_symbols!`, append `IDENTICAL => "==",` as the last entry.

Add to `MixinApplication`:

```rust
    /// §3.1.1: a value's class "does not define an identity method (==)".
    /// The class, if any, that declares one below `Value` -- `Value`'s own
    /// excluded, which the specification's next sentence allows, and every
    /// level above `Value` too, whose `==` `Value` overrides. Refused where a
    /// class object is created (`interpreter::value_class`).
    pub identity_method: Option<Symbol>,
```

In `apply_mixin`, name the `Value`-mixin test once and reuse it:

```rust
        let is_value_mixin = self
            .value_mixin
            .as_ref()
            .is_some_and(|value_mixin| Rc::ptr_eq(value_mixin, mixin));
        let inherits_value = is_value_mixin
            || superclass.is_some_and(|application| application.inherits_value);
        let identity_method = superclass
            .and_then(|application| application.identity_method)
            .or_else(|| {
                (inherits_value
                    && !is_value_mixin
                    && mixin.compiled_method(Symbol::IDENTICAL).is_some())
                .then_some(mixin.name)
            });
```

Add `identity_method` to the literal and `identity_method: None` to the three test constructors.

- [ ] **Step 4: The check at class creation**

Create `crates/ns-runtime/src/interpreter/value_class.rs`:

```rust
//! §3.1.1's conditions on a value *class*, decided where a nested class
//! object is created (`nested::after_clause`), as SOMns decides them
//! (`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`).
//! Cold: `after_clause` calls in only for a class that inherits `Value`.
//! Every text lives here, so both lowerings raise the same errors.

use crate::heap::{ClassReference, ObjectReference};
use crate::interpreter::Mutator;
use crate::mixin::MixinApplication;
use crate::{NewspeakError, Signal, Symbol, Value};

#[cold]
#[inline(never)]
pub(crate) fn check_value_class<'gc>(
    _mutator: &Mutator<'gc>,
    name: Symbol,
    application: &MixinApplication,
    enclosing: ObjectReference<'gc>,
    _superclass: ClassReference<'gc>,
) -> Result<(), Signal<'gc>> {
    if let Some(declaring) = application.identity_method {
        return Err(Signal::Error(NewspeakError {
            selector: name,
            receiver: Value::Object(enclosing),
            message: format!(
                "cannot create {name}: a value class may not define ==, and {declaring} defines it"
            ),
        }));
    }
    Ok(())
}
```

Adjust the `use` paths to where `NewspeakError`, `Signal`, `ObjectReference` and `ClassReference` actually live; copy them from `interpreter/nested.rs`'s imports. The two underscore parameters are Task 6's. If clippy objects to the unused-parameter names, name them `mutator` and `superclass` with `let _ = (mutator, superclass);` until Task 6 uses them.

Register `pub(crate) mod value_class;` in `interpreter/mod.rs` beside the other modules.

In `after_clause` (`interpreter/nested.rs`), directly after `let application = mutator.apply_mixin(&nested, Some(&super_application));`:

```rust
    // §3.1.1's conditions on a value class, asked once, where its class
    // object comes into being; a class that is no value pays this one test.
    if application.inherits_value {
        crate::interpreter::value_class::check_value_class(
            mutator,
            nested.name,
            &application,
            enclosing,
            superclass,
        )?;
    }
```

- [ ] **Step 5: Run the gate**

Expected: all green, both new goldens included. No kernel class below `Value` declares `==`, so the bootstrap is unaffected.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: refuse a value class that defines an identity method"
```

---

### Task 6: A value class's enclosing objects are values

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/value_class.rs`
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`is_a_value` becomes `pub(crate)`)
- Create: `goldens/kernel/a_value_class_in_an_ordinary_module_is_refused.ns`/`.toml`, `goldens/kernel/a_value_over_a_class_with_a_mutable_enclosing_object_is_refused.ns`/`.toml`, `goldens/kernel/a_value_over_a_slotless_class_in_a_value_module_is_accepted.ns`/`.toml`
- Test: `crates/ns-runtime/tests/value_conditions.rs`

**Interfaces:**
- Consumes: `check_value_class` (Task 5), `is_a_value(mutator, ObjectReference) -> bool` (`instantiate.rs`), the kernel and mirror as values (Task 2), the restructured modules (Task 3).

- [ ] **Step 1: Write the failing goldens and test**

`goldens/kernel/a_value_class_in_an_ordinary_module_is_refused.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1: a value object's "enclosing objects (3.3) are
   all value objects". A class declared in a module lives inside the
   module's instance, so a value class in an ordinary module -- one declared
   `= Object new`, not `= Object <: Value` -- could never have a value for an
   instance. Refused where the class object is created, as SOMns does. *)
class ValueInAnOrdinaryModule usingPlatform: platformAtCreation = Object new () (
    public class Point x: anX y: aY = Value new (
        | public x = anX. public y = aY. |
    ) ()
    public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        platform transcript show: (Point x: 1 y: 2) x printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
before
'''
stderr = '''
Uncaught Newspeak error: cannot create Point: a value class's enclosing objects must be values, and its enclosing object, an instance of ValueInAnOrdinaryModule, is not one
'''
```

`goldens/kernel/a_value_over_a_class_with_a_mutable_enclosing_object_is_refused.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1 and 6.2: an inherited method reaches its own
   level's enclosing objects through `outer`, so they are part of what a
   value can reach. `Stamped` is declared in a value module and inherits
   `Value` through `<:`, but its superclass `Counter` is enclosed by a
   `Holder`, whose `count` is mutable -- `bump` would change it through any
   `Stamped`. Every level's enclosing object is asked, not only the class's
   own. *)
class ValueOverAMutableLevel usingPlatform: platformAtCreation = Object <: Value () (
    public class Holder = Object new (
        | public count ::= 0. |
    ) (
        public class Counter = Object new () (
            public bump = ( count:: count + 1. ^count )
        )
    )
    public MutableCounter = ( ^Holder new Counter )
    public class Stamped = MutableCounter new <: Value () ()
    public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        platform transcript show: Stamped new bump printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stdout = '''
before
'''
stderr = '''
Uncaught Newspeak error: cannot create Stamped: a value class's enclosing objects must be values, and Counter's enclosing object, an instance of Holder, is not one
'''
```

`goldens/kernel/a_value_over_a_slotless_class_in_a_value_module_is_accepted.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 3.1.1 and 6.2: the conditions are on the value, not on
   every class above it. `Describer` is no value -- it does not inherit
   `Value` -- but it has no slots and its enclosing object is a value, so
   `Describer new <: Value` is a value class, and making a value variant of
   an ordinary class is what `<: Value` is for. Requiring `Describer` itself
   to inherit `Value` would refuse a program 3.1.1 allows. *)
class ValueOverASlotlessClass usingPlatform: platformAtCreation = Object <: Value () (
    public class Describer = Object new () (
        public describe = ( ^'described' )
    )
    public class Described = Describer new <: Value () ()
    public main: platform args: arguments = (
        platform transcript show: Described new describe; newLine.
        platform transcript show: (Described new = Described new) printString; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
described
true
'''
```

Add to `tests/value_conditions.rs`:

```rust
/// The walk stops at the first class that was itself created as a value
/// class, so a subclass of a checked value class is not refused for a level
/// above it that was already vouched for -- and a subclass declared in an
/// ordinary module is still refused for its own enclosing object.
#[test]
fn a_value_subclass_is_checked_for_its_own_enclosing_object() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Outer = Object <: Value () (
            public class Base = Value new () ()
            public class Inner = Object new () (
                public class Derived = Base new () ()
            )
            public class Sibling = Base new () ()
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let outer = expect_ok(interpreter.instantiate(&class, &[]), "Outer new");
    expect_ok(interpreter.send(&outer, Symbol::intern("Sibling"), &[]), "Sibling");
    let inner_class = expect_ok(interpreter.send(&outer, Symbol::intern("Inner"), &[]), "Inner");
    let inner = expect_ok(interpreter.instantiate(&inner_class, &[]), "Inner new");
    let message = expect_error(
        interpreter.send(&inner, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    assert_eq!(
        message,
        "cannot create Derived: a value class's enclosing objects must be values, and its \
         enclosing object, an instance of Inner, is not one"
    );
}
```

`Derived` names `Base` through an implicit-receiver send that reaches `Outer`. Check that the resolver binds it that way. If it does not, write `outer Outer Base new` as the clause.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-cli --test run_goldens` and `cargo test -p ns-runtime --test value_conditions`
Expected: the two refusing goldens and the new test FAIL, because both classes are created today. The accepting golden passes already.

- [ ] **Step 3: The walk**

Make `instantiate.rs`'s `is_a_value` `pub(crate)`. Extend `check_value_class`, now using both parameters under their real names:

```rust
    if !super::instantiate::is_a_value(mutator, enclosing) {
        return Err(non_value_enclosing_error(
            mutator, name, enclosing, None, enclosing,
        ));
    }
    // Every level's enclosing object, not only the class's own: an
    // inherited method reaches its own level's through `outer`. The walk
    // stops at a class that inherits `Value` and was created as a class of
    // its own -- by `after_clause`, as a builtin or as a module root, never
    // a level a `<:` clause added -- because that class was checked the
    // same way and everything above it holds by induction.
    let heap = mutator.heap();
    let mut level = Some(superclass);
    while let Some(class) = level {
        if heap.class_inherits_value(class) && !heap.class_is_applied_level(class) {
            return Ok(());
        }
        if let Some(level_enclosing) = heap.class_enclosing(class) {
            if !super::instantiate::is_a_value(mutator, level_enclosing) {
                let level_name = heap
                    .class_application(class)
                    .map(|application| application.mixin.name);
                return Err(non_value_enclosing_error(
                    mutator,
                    name,
                    enclosing,
                    level_name,
                    level_enclosing,
                ));
            }
        }
        level = heap.class_superclass(class);
    }
    Ok(())
```

and the error:

```rust
/// The refusal, naming the class being created and -- for an inherited
/// level -- the level whose enclosing object is at fault.
#[cold]
fn non_value_enclosing_error<'gc>(
    mutator: &Mutator<'gc>,
    name: Symbol,
    enclosing: ObjectReference<'gc>,
    level: Option<Symbol>,
    offending: ObjectReference<'gc>,
) -> Signal<'gc> {
    let described = mutator.heap().describe(&Value::Object(offending));
    let whose = match level {
        None => "its enclosing object".to_string(),
        Some(level) => format!("{level}'s enclosing object"),
    };
    Signal::Error(NewspeakError {
        selector: name,
        receiver: Value::Object(enclosing),
        message: format!(
            "cannot create {name}: a value class's enclosing objects must be values, and \
             {whose}, {described}, is not one"
        ),
    })
}
```

The walk is iterative and each step is an accessor read, so a long chain from an image costs time proportional to it and never recurses.

Update the module's doc comment to name both conditions.

- [ ] **Step 4: Run the gate**

Expected: all green. If a golden or test from before this plan now fails with "cannot create", Task 3 missed a module. Restructure it the same way; do not re-record it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: refuse a value class whose enclosing objects are not values"
```

---

### Task 7: Documents

Markdown only, so this commit needs no test run.

**Files:**
- Modify: `docs/design/object-model.md`
- Modify: `docs/design/core-library.md`
- Modify: `docs/design/image-format.md`
- Modify: `docs/design/README.md`
- Modify: `docs/PROGRESS.md`
- Modify: `CLAUDE.md` (only where a statement became false)

- [ ] **Step 1: `object-model.md`**

- "A value object's two conditions" becomes "A value object's four conditions", describing all four as built: two at instantiation (the mutable slot over the whole chain, the slot holding a value), two at class creation (identity method, enclosing objects with the early stop), and the root rule as a fifth refusal at instantiation. Keep the existing text about the nested-class cache slot.
- "Planned: all four conditions, enforced strictly" is folded into that section and removed as a planned section. Keep its reasoning, which is the part a later reader needs: why class creation, why every level, why not "`Foo` must be a value", the mutable slot above `Value`, the root rule and why it is deferred.
- At the end of "Mixin application", the "Gap:" paragraph goes. The root rule is stated where `<: Value` at the root is described.

- [ ] **Step 2: `core-library.md`**

- "What is still unchecked": remove the enclosing-object and `==` bullets; keep "A ready-made instance decoded from an image is not re-checked" and extend it to class objects; keep the `=`/`hash` bullet. The "value class nothing instantiates" bullet now reads: a value class nothing creates is never checked, and one nothing instantiates is not checked for its slots.
- The bootstrap section: the kernel is `Object <: Value`, the mirror's class sits under `Value`, and why.
- The class tree at the top gets the mirror under `Value`, if the tree shows it.

- [ ] **Step 3: `image-format.md`**

In "Verification", state that the root-section reader refuses what the root rule forbids, and that the encoder refuses a module with a root defect. Name the error texts' shape.

- [ ] **Step 4: `README.md`**

Remove the root-clause deviation row from "Deviations from the language specification". Update the `kernel/` and `errors/` rows of the goldens table: `kernel/` holds all of §3.1.1's value conditions, those refused at class creation included; `errors/` holds the root rule's refusals.

- [ ] **Step 5: `PROGRESS.md`**

Add a milestone entry "Plan B: the value conditions" in the style of the neighbouring entries: what was built, the decisions with their dates, and what the measurement in Task 8 showed. Fill in that last part after Task 8. Rewrite "Where the next session starts" so it names step 6's third plan, `Class`, `Behavior` and `Metaclass` in `Kernel.ns`, as the next piece of work, and drops Plan B's open decisions.

- [ ] **Step 6: `CLAUDE.md`**

Search it for statements the plan made false. The candidates: the mirror's class being under `Object`, and anything that says a root clause resolves by builtin name. Change only those sentences.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "docs: describe the value conditions and the root rule as built"
```

---

### Task 8: Final gates and measurement

- [ ] **Step 1: The end-of-batch gates**, in Git Bash, each green:

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
cargo run -p ns-cli -- run-corpus corpus/modules                           # "ran 6, failed 0, skipped 205"
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo build --workspace
```

- [ ] **Step 2: Measurement**

Follow `docs/design/execution.md`'s measurement protocol ("What removing the re-entries moved"). Build `4fa3036` (baseline) and this branch's head as release builds with the pinned flags, in PowerShell, several builds of each tree, pooled:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Time every source under `goldens/bench/` with `ns-next bench`, and `ns-next run` of a module that answers at once for start-up. The kernel now builds one applied level at bootstrap. The expectation is "level": the only new hot-path test is one flag test in `after_clause`, once per class created. Record the numbers and the verdict in `docs/design/execution.md` under a new heading "What the value conditions cost", and summarize them in Task 7's `PROGRESS.md` entry.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: record what the value conditions cost"
```
