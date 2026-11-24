# Milestone 3 — Nested Classes, Mixins, and Source-Free Images: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status: Complete.** All 19 tasks (94 steps) implemented, each reviewed
> for spec compliance and quality, on `feature/m3-nesting-mixins`
> (`bbe5460..a925d68`), merged to `master` at `4f5ea64`. Verified on the
> merged result: 531 tests pass, `cargo clippy --workspace --all-targets`
> is warning-free, `cargo fmt --all` is clean, `parse-corpus` reports
> `parsed 210, failed 0, expected-fail 1, stale 0` and `run-corpus`
> reports `ran 6, failed 0, skipped 205`. Task 9 ran before Tasks 4-8
> because keying compiled code by mixin has to precede the first
> per-instance class object; one task not in the original plan (10b) fixed
> `outer` resolving along the receiver's dynamic chain instead of the
> lexical declaration chain. Findings deferred rather than fixed are
> recorded in the design spec's "Open questions".

**Goal:** Make `ns-next` run Newspeak's nested classes with the language
specification's semantics, and make the image format follow, which turns the
image source-free.

**Architecture:** The single `ClassData` of Milestone 1 splits into three
layers: a `Mixin` per class declaration (shared code and structure), a
`MixinApplication` per (mixin, superclass application) pair (layout and
lookup, globally cached), and a class object per enclosing instance (identity
and its enclosing object, lazily created and cached in a hidden slot of the
enclosing instance). Dispatch and inline caches key on the application; `outer`
and `super` walk the class-object chain. The image serializes a symbol table
and a mixin table; applications are rebuilt on load rather than stored.

**Tech Stack:** Rust 1.98.0, edition 2021, no new external dependencies.
Workspace crates `ns-syntax` (untouched by this milestone), `ns-runtime`,
`ns-cli`.

**Spec:** `docs/superpowers/specs/2026-09-06-1659-ns-next-nesting-mixins-source-free-images-design.md`

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency without a reason
  that survives review; the image format is hand-rolled on purpose.
- **Nothing under `crates/ns-syntax` changes.** The explicit mixin application
  operator `<:` stays unimplemented; this is runtime work.
- **Both engines must agree byte for byte** — exit status, stdout *and*
  stderr — for every program. `crates/ns-cli/tests/differential.rs` enforces
  this over every `tests-fixtures/*.ns` plus inline sources, with no
  exemptions. If a change makes them disagree, the tree-walker is the
  reference: fix the compiler or the VM.
- **`interpreter/` is shared orchestration.** Never copy dispatch, lookup, or
  error text into `vm.rs` or `compiler/`. Widen visibility to `pub(crate)`
  instead.
- **Compiling an already-resolved module never raises a `NewspeakError`.**
  Compiler-invariant violations are `panic!` / `debug_assert!`; program-level
  errors are deferred to execution so both engines report them identically.
- **An image is untrusted input.** No byte sequence may cause a panic or an
  abort: validate every id, tag, count and length before it indexes,
  allocates or slices, and bound any recursion whose depth the input controls.
- **`FORMAT_VERSION` is not bumped.** The project has no external users;
  images that predate this milestone are disposable.
- **Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an
  `EvalResult`.** The bootstrap `Object` class is its own `.class`, so `Debug`
  on a `Value::Obj` recurses forever, and `Signal` deliberately has no
  `Debug`. Tests unwrap through a local `expect_ok` helper — copy the one in a
  neighbouring test file.
- **Every task ends green:** `cargo test --workspace`, `cargo clippy
  --workspace --all-targets` warning-free, `cargo fmt --all`, and
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reporting
  `parsed 210, failed 0, expected-fail 1, stale 0`.
- **Conventional commits** (`feat:`, `fix:`, `perf:`, `test:`, `docs:`,
  `chore:`). All code, comments and commit messages in English.
- **Comments explain the decision, not the mechanics** — what a check
  prevents is the part a later reader cannot reconstruct.

## File Structure

**New files:**

- `crates/ns-runtime/src/mixin.rs` — the `Mixin` and `MixinApplication` types,
  the application cache, and layout computation. One responsibility: what a
  declaration *is* and what applying it to a superclass *produces*. Nothing
  here evaluates Newspeak code.
- `crates/ns-runtime/src/interpreter/nested.rs` — the implicit nested-class
  accessor: lazy creation, the per-receiver cache, and the superclass-clause
  evaluation it needs. Lives under `interpreter/` because it is shared
  orchestration that both engines call.
- `crates/ns-runtime/src/image/symbols.rs` — the image's symbol table:
  building it while encoding, and validating/interning it while decoding.
- `crates/ns-runtime/src/image/mixins.rs` — the image's mixin table.
- `corpus/runnable.toml` — the curated whitelist of corpus modules the
  `run-corpus` gate executes, with expected exit status and stdout.
- `tests-fixtures/nested_*.ns` / `.expected` — the nesting fixtures.

**Modified files:**

- `crates/ns-runtime/src/types.rs` — `ClassData` shrinks to
  `{application, enclosing, superclass, builtin}`; `Frame` gains the `pc`
  engine discriminator.
- `crates/ns-runtime/src/dispatch.rs` — lookup walks applications and reports
  the defining level; `lookup_slot` reads the application's layout.
- `crates/ns-runtime/src/interpreter/instantiate.rs` — `load_module` builds
  the mixin tree; instantiation runs one initializer activation per level with
  the superclass clause's own arguments.
- `crates/ns-runtime/src/bytecode.rs` — `class_key` becomes `mixin_key`.
- `crates/ns-runtime/src/compiler/mod.rs` — compiles the whole mixin tree.
- `crates/ns-runtime/src/image/{mod,encode,decode}.rs` — symbol and mixin
  tables, class objects in the graph, source-free load.
- `crates/ns-cli/src/main.rs` — factory-aware entry point, `run-corpus`,
  `--bench`.
- `CLAUDE.md`, `docs/PROGRESS.md` — invariants and milestone status.

---

## Phase 1 — The three-layer object model

### Task 1: `Mixin`, `MixinApplication`, and the application cache

**Files:**
- Create: `crates/ns-runtime/src/mixin.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (add `pub mod mixin;` and re-export),
  `crates/ns-runtime/src/interpreter/mod.rs` (the cache field)
- Test: `crates/ns-runtime/tests/mixin.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `Mixin`, `MixinApplication`, `mixin_key(&Rc<Mixin>) -> usize`,
  and `Interpreter::apply_mixin(&mut self, mixin: &Rc<Mixin>, superclass:
  Option<&Rc<MixinApplication>>) -> Rc<MixinApplication>`.

This task adds the types and the cache and nothing else — no existing code
path changes yet, so the whole suite must stay green on unchanged behaviour.

- [x] **Step 1: Write the failing test**

```rust
// crates/ns-runtime/tests/mixin.rs
use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::mixin::{Mixin, MixinApplication};
use ns_runtime::{Interpreter, Symbol};

fn bare_mixin(name: &str) -> Rc<Mixin> {
    Rc::new(Mixin {
        name: Symbol::intern(name),
        methods: HashMap::new(),
        instance_slots: Vec::new(),
        init_exprs: Vec::new(),
        factory: None,
        nested: Vec::new(),
        class_side: None,
        decl: None,
    })
}

#[test]
fn applying_the_same_mixin_to_the_same_superclass_is_cached() {
    let mut interp = Interpreter::bootstrap();
    let m = bare_mixin("Inner");
    let a = interp.apply_mixin(&m, None);
    let b = interp.apply_mixin(&m, None);
    assert!(
        Rc::ptr_eq(&a, &b),
        "the application cache must return one behaviour per (mixin, super)"
    );
}

#[test]
fn different_superclasses_produce_different_applications() {
    let mut interp = Interpreter::bootstrap();
    let sup_a = interp.apply_mixin(&bare_mixin("A"), None);
    let sup_b = interp.apply_mixin(&bare_mixin("B"), None);
    let m = bare_mixin("Inner");
    let x = interp.apply_mixin(&m, Some(&sup_a));
    let y = interp.apply_mixin(&m, Some(&sup_b));
    assert!(!Rc::ptr_eq(&x, &y));
}

#[test]
fn inherited_slots_are_counted_before_own_slots() {
    // A superclass application with two slots, a subclass mixin with none:
    // the subclass's layout must start past the inherited ones.
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    Rc::get_mut(&mut sup).unwrap().instance_slots = vec![slot_decl("a"), slot_decl("b")];
    let sup_app = interp.apply_mixin(&sup, None);
    let sub_app = interp.apply_mixin(&bare_mixin("Sub"), Some(&sup_app));
    assert_eq!(sup_app.num_slots, 2);
    assert_eq!(sub_app.num_slots, 2);
}
```

`slot_decl(name)` builds a minimal `Rc<ast::SlotDecl>`; copy the construction
a neighbouring test uses, or parse a one-slot class and take its slot.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test mixin`
Expected: FAIL — `ns_runtime::mixin` does not exist.

- [x] **Step 3: Write the implementation**

```rust
// crates/ns-runtime/src/mixin.rs
//! What a class declaration *is* (`Mixin`) and what applying it to a
//! superclass *produces* (`MixinApplication`). Nothing here evaluates
//! Newspeak code: applying a mixin is a total, deterministic operation.
//!
//! The split exists because `newspeak-spec.pdf` §6 requires a distinct class
//! object per enclosing instance, while §6.2 requires all applications of a
//! mixin to share its code and structure. Identity lives on the class object,
//! behaviour lives here, and that is what lets an inline cache stay
//! monomorphic across enclosing instances.

pub struct Mixin {
    pub name: Symbol,
    pub methods: HashMap<Symbol, Rc<ast::MethodDecl>>,
    pub instance_slots: Vec<Rc<ast::SlotDecl>>,
    pub init_exprs: Vec<ast::Stmt>,
    pub factory: Option<Rc<ast::MessagePattern>>,
    /// The nested class declarations, in declaration order. Each one gets an
    /// implicit accessor and a hidden cache slot (Task 4).
    pub nested: Vec<Rc<Mixin>>,
    pub class_side: Option<Rc<Mixin>>,
    /// Provenance for reflection and the image; `None` for builtin classes.
    pub decl: Option<Rc<ast::ClassDecl>>,
}

pub struct MixinApplication {
    pub mixin: Rc<Mixin>,
    pub superclass: Option<Rc<MixinApplication>>,
    /// Absolute index of every slot reachable on an instance, inherited ones
    /// included -- resolved through the *current* layout, per the liveness
    /// spec's object-model constraint.
    pub slot_index: HashMap<Symbol, usize>,
    pub num_slots: usize,
}

pub fn mixin_key(mixin: &Rc<Mixin>) -> usize {
    Rc::as_ptr(mixin) as *const () as usize
}

pub fn application_key(application: &Rc<MixinApplication>) -> usize {
    Rc::as_ptr(application) as *const () as usize
}
```

In `Interpreter`, add the cache and the operation:

```rust
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    applications: HashMap<(usize, usize), Rc<MixinApplication>>,

    pub fn apply_mixin(
        &mut self,
        mixin: &Rc<Mixin>,
        superclass: Option<&Rc<MixinApplication>>,
    ) -> Rc<MixinApplication> {
        let key = (
            mixin_key(mixin),
            superclass.map_or(0, application_key),
        );
        if let Some(existing) = self.applications.get(&key) {
            return existing.clone();
        }
        let inherited = superclass.map_or(0, |s| s.num_slots);
        let mut slot_index = superclass.map_or_else(HashMap::new, |s| s.slot_index.clone());
        for (i, slot) in mixin.instance_slots.iter().enumerate() {
            slot_index.insert(Symbol::intern(&slot.name), inherited + i);
        }
        let application = Rc::new(MixinApplication {
            mixin: mixin.clone(),
            superclass: superclass.cloned(),
            num_slots: inherited + mixin.instance_slots.len(),
            slot_index,
        });
        self.applications.insert(key, application.clone());
        application
    }
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test mixin` — Expected: PASS.
Run: `cargo test --workspace` — Expected: PASS, nothing else touched.

- [x] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/mixin.rs crates/ns-runtime/src/lib.rs \
        crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/tests/mixin.rs
git commit -m "feat: add Mixin, MixinApplication, and the application cache"
```

### Task 2: `ClassData` becomes identity, the application becomes behaviour

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`ClassData`),
  `crates/ns-runtime/src/dispatch.rs` (`lookup_public`, `lookup_protected`,
  `lookup_defined_for`, `lookup_slot`, `class_enclosing`),
  `crates/ns-runtime/src/interpreter/instantiate.rs` (construction sites),
  `crates/ns-runtime/src/builtins/mod.rs` (builtin class construction)
- Test: `crates/ns-runtime/tests/mixin.rs` (extend), plus the whole suite as
  the regression gate

**Interfaces:**
- Consumes: `Mixin`, `MixinApplication`, `Interpreter::apply_mixin` (Task 1).
- Produces: `ClassData { application: Rc<MixinApplication>, enclosing:
  Option<ObjRef>, superclass: Option<ClassRef>, builtin: Option<BuiltinClass> }`
  and `dispatch::Lookup::Method(Rc<ast::MethodDecl>, ClassRef)` unchanged in
  shape — the second field stays the *class object* that defines the method,
  because `Frame.current_class` needs it for `outer` and `super`.

This is a pure refactor: one class per module still, no nesting yet. Every
existing test must pass unchanged at the end of it. Do not add behaviour here.

- [x] **Step 1: Write the failing test**

```rust
// append to crates/ns-runtime/tests/mixin.rs
#[test]
fn a_class_object_carries_identity_and_its_application_carries_layout() {
    let src = "Newspeak3\n'Test'\nclass Point new = Object new (| x. y. |) ()\n";
    let (interp, class, _resolved) = load(src);
    let borrowed = class.borrow();
    let ns_runtime::ObjectData::Class(data) = &borrowed.data else {
        panic!("load_module must produce a class object");
    };
    assert_eq!(data.application.num_slots, 2);
    assert!(data.enclosing.is_none(), "a module's enclosing object is nil");
    assert_eq!(
        data.application.slot_index[&Symbol::intern("y")],
        1,
        "layout lives on the application, not on the class object"
    );
    let _ = interp;
}
```

Copy the `load` helper from `crates/ns-runtime/tests/instantiate.rs`
(parse → `resolve` → `Interpreter::bootstrap` → `load_module`).

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test mixin` — Expected: FAIL, `ClassData`
has no field `application`.

- [x] **Step 3: Write the implementation**

Shrink `ClassData` in `types.rs` to exactly:

```rust
pub struct ClassData {
    /// Behaviour: code, layout and the superclass chain. Shared with every
    /// other class object built from the same mixin and superclass.
    pub application: Rc<MixinApplication>,
    /// `newspeak-spec.pdf` §6: the enclosing object is the nested class's
    /// only state, which is why a class object cannot be shared between
    /// instances of its enclosing class.
    pub enclosing: Option<ObjRef>,
    /// The per-instance class-object chain, parallel to the application
    /// chain. `outer` and `super` need it; lookup does not.
    pub superclass: Option<ClassRef>,
    pub builtin: Option<BuiltinClass>,
}
```

Then move every reader over:

- `dispatch::lookup_*` walk `data.application` for methods
  (`application.mixin.methods`) and step to `application.superclass`, while
  reporting the *class object* of the defining level by walking
  `data.superclass` the same number of steps.
- `dispatch::lookup_slot` reads `application.slot_index`.
- `dispatch::class_enclosing` still reads `data.enclosing` — unchanged.
- `instantiate.rs` sizes a new instance from `application.num_slots`.
- Builtin classes get a one-mixin application each: build the `Mixin` from
  the builtin's native method set with empty slots, apply it to the builtin
  superclass's application, and store the result.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS, including `differential.rs`.
Run: `cargo clippy --workspace --all-targets` — Expected: no warnings.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: split class identity from mixin-application behaviour"
```

### Task 3: `load_module` builds the mixin tree

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
  (`load_module`, `load_class` → `build_mixin`)
- Test: `crates/ns-runtime/tests/mixin.rs` (extend)

**Interfaces:**
- Consumes: `apply_mixin` (Task 1), the shrunken `ClassData` (Task 2).
- Produces: `Interpreter::build_mixin(&mut self, decl: &ast::ClassDecl)
  -> Rc<Mixin>`, recursive over `decl.instance_side.nested_classes` and
  `decl.class_side`; `load_module` returns the root class object as before.

Milestone 1's `load_class` never recursed into `nested_classes` and dropped
`class_side` unconditionally (both recorded in its own doc comment). This task
removes both gaps *structurally* — the mixins exist and are reachable. Nothing
evaluates them yet; the accessor is Task 5.

- [x] **Step 1: Write the failing test**

```rust
// append to crates/ns-runtime/tests/mixin.rs
const NESTED_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Object new (| v. |) (
        public value = ( ^v )
    )
    public classSideMarker = ( ^1 )
) : (
    public marker = ( ^2 )
)
";

#[test]
fn the_mixin_tree_contains_nested_and_class_side_declarations() {
    let (_interp, class, _resolved) = load(NESTED_SRC);
    let borrowed = class.borrow();
    let ns_runtime::ObjectData::Class(data) = &borrowed.data else {
        panic!("expected a class object");
    };
    let mixin = &data.application.mixin;
    assert_eq!(mixin.nested.len(), 1, "Inner must be in the mixin tree");
    assert_eq!(mixin.nested[0].name, Symbol::intern("Inner"));
    assert!(
        mixin.class_side.is_some(),
        "a class side is a mixin now, not something to discard"
    );
}
```

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test mixin
the_mixin_tree_contains_nested_and_class_side_declarations`
Expected: FAIL — `nested` is empty and `class_side` is `None`.

- [x] **Step 3: Write the implementation**

Replace `load_class` with `build_mixin`, which is a pure structural walk:

```rust
    /// Builds the mixin for `decl`, recursively including its nested
    /// declarations and its class side. Pure structure -- no Newspeak code is
    /// evaluated here, which is what keeps module loading total.
    fn build_mixin(&mut self, decl: &ast::ClassDecl) -> Rc<Mixin> {
        let mut methods = HashMap::new();
        for m in &decl.instance_side.methods {
            methods.insert(Symbol::intern(&method_name(&m.pattern)), Rc::new(m.clone()));
        }
        let nested = decl
            .instance_side
            .nested_classes
            .iter()
            .map(|n| self.build_mixin(n))
            .collect();
        let class_side = decl.class_side.as_ref().map(|side| self.build_class_side(side));
        Rc::new(Mixin { /* name, methods, instance_slots, init_exprs, factory, nested, class_side, decl */ })
    }
```

`load_module` then becomes: build the root mixin, resolve the superclass
receiver as today, apply the mixin to that superclass's application, and wrap
the result in a class object with `enclosing: None`.

Keep `build_class_side` separate and small: a class side has methods but no
nested classes of its own to load in this milestone.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: build the whole mixin tree when loading a module"
```

## Phase 2 — Nesting semantics

### Task 4: Hidden cache slots for nested declarations

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication`, `apply_mixin`)
- Test: `crates/ns-runtime/tests/mixin.rs` (extend)

**Interfaces:**
- Consumes: `apply_mixin` (Task 1), the mixin tree (Task 3).
- Produces: `MixinApplication.nested_slot: HashMap<Symbol, usize>` — the
  absolute index of the hidden slot that caches each nested class, and
  `num_slots` grown to include them.

`newspeak-spec.pdf` §6 says the class "is effectively cached within r", and
that a distinct nested class exists per declaration "given in the declaration
of C *and its superclasses*". So the cache is a slot in the enclosing
instance, contributed at the level that declares it, exactly like an ordinary
inherited slot. Hidden slots are derived from `mixin.nested`, never declared,
so nothing stores them separately.

- [x] **Step 1: Write the failing test**

```rust
// append to crates/ns-runtime/tests/mixin.rs
#[test]
fn a_nested_declaration_reserves_a_hidden_cache_slot() {
    let (_interp, class, _resolved) = load(NESTED_SRC);
    let borrowed = class.borrow();
    let ns_runtime::ObjectData::Class(data) = &borrowed.data else {
        panic!("expected a class object");
    };
    let app = &data.application;
    assert_eq!(app.num_slots, 1, "Outer declares no slots but nests Inner");
    assert_eq!(app.nested_slot[&Symbol::intern("Inner")], 0);
}

#[test]
fn inherited_nested_declarations_keep_their_declaring_levels_slot() {
    // A superclass that declares one slot and nests one class; a subclass
    // that declares one slot of its own. The inherited hidden slot must keep
    // the index its declaring level gave it, and the subclass's own slot must
    // land after it.
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    {
        let sup = Rc::get_mut(&mut sup).unwrap();
        sup.instance_slots = vec![slot_decl("a")];
        sup.nested = vec![bare_mixin("Inner")];
    }
    let sup_app = interp.apply_mixin(&sup, None);
    assert_eq!(sup_app.slot_index[&Symbol::intern("a")], 0);
    assert_eq!(sup_app.nested_slot[&Symbol::intern("Inner")], 1);
    assert_eq!(sup_app.num_slots, 2);

    let mut sub = bare_mixin("Sub");
    Rc::get_mut(&mut sub).unwrap().instance_slots = vec![slot_decl("own")];
    let sub_app = interp.apply_mixin(&sub, Some(&sup_app));
    assert_eq!(
        sub_app.nested_slot[&Symbol::intern("Inner")],
        1,
        "an inherited nested declaration keeps its declaring level's slot"
    );
    assert_eq!(sub_app.slot_index[&Symbol::intern("own")], 2);
    assert_eq!(sub_app.num_slots, 3);
}
```

Replace the second test's placeholder body with a two-class module once
Task 7 makes user-defined superclasses instantiable end to end; until then
assert the layout directly through `apply_mixin` with a hand-built mixin
chain, as Task 1's tests do.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test mixin
a_nested_declaration_reserves_a_hidden_cache_slot`
Expected: FAIL — `MixinApplication` has no field `nested_slot`.

- [x] **Step 3: Write the implementation**

In `apply_mixin`, after the declared slots are laid out:

```rust
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        let mut nested_slot = superclass.map_or_else(HashMap::new, |s| s.nested_slot.clone());
        let mut next = inherited + mixin.instance_slots.len();
        for nested in &mixin.nested {
            nested_slot.insert(nested.name, next);
            next += 1;
        }
```

and set `num_slots: next`.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: reserve a hidden cache slot per nested class declaration"
```

### Task 5: The implicit nested-class accessor

**Files:**
- Create: `crates/ns-runtime/src/interpreter/nested.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod nested;`, and the
  send path that consults it), `crates/ns-runtime/src/dispatch.rs`
  (`Lookup` gains a nested-class arm)
- Test: `crates/ns-runtime/tests/nested_classes.rs`

**Interfaces:**
- Consumes: `nested_slot` (Task 4), the mixin tree (Task 3), `apply_mixin`.
- Produces: `pub(crate) fn nested_class(interp: &mut Interpreter, receiver:
  &Value, nested: &Rc<Mixin>, bindings: &Rc<HashMap<Span, Binding>>)
  -> EvalResult`, and `dispatch::Lookup::NestedClass(Rc<Mixin>, ClassRef)`.

This is the heart of the milestone. It lives in `interpreter/` because it
evaluates Newspeak code (the superclass clause) and must therefore behave
identically under both engines: it runs that code through the same
body-execution hook `instantiate` already uses.

- [x] **Step 1: Write the failing test**

```rust
// crates/ns-runtime/tests/nested_classes.rs
use ns_runtime::{Interpreter, Signal, Symbol, Value};

// `load`, `expect_ok` copied from crates/ns-runtime/tests/instantiate.rs

const SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Object new (| v. |) (
        public value = ( ^42 )
    )
    public makeInner = ( ^Inner new )
    public innerClass = ( ^Inner )
)
";

#[test]
fn the_accessor_returns_the_same_class_object_every_time() {
    let (mut interp, class, resolved) = load(SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &resolved.bindings),
        "instantiate Outer",
    );
    let first = expect_ok(
        interp.send(outer.clone(), Symbol::intern("innerClass"), vec![], &resolved.bindings, None),
        "first Inner",
    );
    let second = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![], &resolved.bindings, None),
        "second Inner",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&first, &second) else {
        panic!("a nested class access must yield a class object");
    };
    assert!(std::rc::Rc::ptr_eq(a, b), "spec 6: cached within r");
}

#[test]
fn two_enclosing_instances_get_distinct_nested_classes() {
    let (mut interp, class, resolved) = load(SRC);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class.clone(), vec![platform.clone()], &resolved.bindings),
        "instantiate first Outer",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform], &resolved.bindings),
        "instantiate second Outer",
    );
    let a = expect_ok(
        interp.send(one, Symbol::intern("innerClass"), vec![], &resolved.bindings, None),
        "first",
    );
    let b = expect_ok(
        interp.send(two, Symbol::intern("innerClass"), vec![], &resolved.bindings, None),
        "second",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&a, &b) else { panic!("class objects") };
    assert!(!std::rc::Rc::ptr_eq(a, b), "spec 6: a distinct class per instance");
    let (da, db) = (a.borrow(), b.borrow());
    let (ns_runtime::ObjectData::Class(da), ns_runtime::ObjectData::Class(db)) =
        (&da.data, &db.data) else { panic!("class data") };
    assert!(
        std::rc::Rc::ptr_eq(&da.application, &db.application),
        "spec 6.2: applications of one mixin share code and structure -- this \
         is what keeps a call site monomorphic across enclosing instances"
    );
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test nested_classes`
Expected: FAIL — `innerClass` raises `doesNotUnderstand:`, because nothing
defines the accessor yet.

- [x] **Step 3: Write the implementation**

`dispatch::lookup_*` gain one arm: after the method dictionary of a level
misses, check that level's `mixin.nested` for a declaration whose name equals
the selector, honouring the declaration's access modifier exactly as a method
would. Return `Lookup::NestedClass(mixin, defining_class_object)`.

`interpreter::nested::nested_class` then performs the five steps:

```rust
    // 1. The cache slot for this declaration, in the receiver's own layout.
    let index = application_of(receiver).nested_slot[&nested.name];
    if let Some(cached) = read_slot(receiver, index) { return Ok(cached); }
    // 2. Evaluate the superclass clause *in the receiver's context*. This is
    //    Newspeak code, so it runs through the engine hook -- never inline,
    //    or the two engines would diverge.
    let superclass = eval_superclass_clause(interp, receiver, nested, bindings)?;
    // 3. The application cache decides whether this behaviour already exists.
    let application = interp.apply_mixin(nested, superclass_application(&superclass));
    // 4. The class object is per receiver: identity plus its enclosing object.
    let class = interp.new_class_object(application, Some(receiver_obj), superclass);
    // 5. Cache it in the slot before returning, so step 1 hits next time.
    write_slot(receiver, index, Value::Obj(class.clone()));
    Ok(Value::Obj(class))
```

A superclass clause that does not evaluate to a class object is a *program*
error (`newspeak-spec.pdf` §6.1: "It is a run time error if ec does not
evaluate to a class object C"), so it raises a `NewspeakError` through the
normal path — identical text under both engines — and never panics.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test nested_classes` — Expected: PASS.
Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: implement the implicit nested-class accessor"
```

### Task 6: Nesting fixtures, the §2.3 override case, and `outer`

**Files:**
- Create: `tests-fixtures/nested_access.ns` + `.expected`,
  `tests-fixtures/nested_override.ns` + `.expected`,
  `tests-fixtures/nested_outer.ns` + `.expected`
- Test: the fixtures themselves; `crates/ns-cli/tests/differential.rs` and
  `run_fixtures.rs` pick them up with no change

**Interfaces:**
- Consumes: the accessor (Task 5), `dispatch::kth_enclosing_object`
  (unchanged since Milestone 1).
- Produces: the executable evidence that nesting works under *both* engines.

`kth_enclosing_object` already walks `class → enclosing`, and Task 5 is what
finally sets `enclosing`. This task proves it rather than changing it — if
`outer` fails here, the defect is in Task 5's class-object construction.

- [x] **Step 1: Write the failing fixtures**

`tests-fixtures/nested_outer.ns`:

```newspeak
Newspeak3
'Test'
class Outer usingPlatform: p = Object new (| t ::= p transcript. |) (
    public class Inner = Object new () (
        public shout = ( outer Outer say: 'from inner' )
    )
    say: s = ( t show: s; newLine )
    public main: platform args: args = ( ^Inner new shout )
)
```

`tests-fixtures/nested_outer.expected`: `from inner`

`tests-fixtures/nested_override.ns` encodes the specification's §2.3 example:
a nested declaration overridden by a slot in a subclass, and the already-cached
`Nested` keeping its superclass after that slot is reassigned.

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-cli --test run_fixtures` — Expected: FAIL until the
fixtures' expected output matches.
Run: `cargo test -p ns-cli --test differential` — Expected: both engines must
agree; a disagreement here means the accessor is not going through the shared
orchestration, and the tree-walker is the reference.

- [x] **Step 3: Fix whatever the fixtures expose**

No new production code is planned for this task. If a fixture fails, the
defect is in Task 4 or 5 — fix it there rather than special-casing here.

- [x] **Step 4: Run the full suite**

Run: `cargo test --workspace` — Expected: PASS, with the new fixtures counted
in both the fixture runner and the differential harness.

- [x] **Step 5: Commit**

```bash
git add tests-fixtures/
git commit -m "test: fixtures for nested access, the 2.3 override case, and outer"
```

### Task 7: Evaluate the superclass clause, one activation per level

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
  (`run_superclass_initializer`, `run_own_initializer`, `instantiate`)
- Test: `crates/ns-runtime/tests/instantiate.rs` (extend),
  `tests-fixtures/superclass_args.ns` + `.expected`

**Interfaces:**
- Consumes: the mixin chain on `MixinApplication` (Task 2).
- Produces: initializer activations built per level, each with the arguments
  its own superclass clause evaluates to.

This closes the Milestone 1 gap its own doc comment records: "the superclass
clause's message is never evaluated" and "every level runs in the subclass's
frame". Per §6.2, the instance initializer evaluates the superclass clause's
message, invokes the superclass's initializer on the same object with *those*
arguments, and only then runs its own mixin's initializer.

- [x] **Step 1: Write the failing test**

```rust
// crates/ns-runtime/tests/instantiate.rs
#[test]
fn a_superclass_factory_message_receives_its_own_arguments() {
    let src = "\
Newspeak3
'Test'
class Base withValue: v = Object new (| stored ::= v. |) (
    public stored = ( ^stored )
)
";
    // A subclass whose superclass clause passes a computed argument:
    //   class Derived new = Base withValue: 7 (| |) ()
    // `stored` must be 7 -- Milestone 1 silently produced nil, because the
    // clause's message was never evaluated and every level shared the
    // subclass's activation.
    let (mut interp, class, resolved) = load(src);
    let instance = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate Derived",
    );
    let stored = expect_ok(
        interp.send(instance, Symbol::intern("stored"), vec![], &resolved.bindings, None),
        "read stored",
    );
    assert!(matches!(stored, Value::SmallInt(7)));
}
```

Write the two-class module out in full in the fixture and in the test source
string; do not abbreviate it in a comment.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test instantiate
a_superclass_factory_message_receives_its_own_arguments`
Expected: FAIL — `stored` is `Nil`.

- [x] **Step 3: Write the implementation**

`instantiate` walks the application chain to the root, then initializes
downwards. At each level:

1. Evaluate that level's superclass clause message in the *current* level's
   activation, producing the argument list for the level above it.
2. Build a fresh initializer frame for the level above, with those arguments
   and with `current_class` set to that level's class object.
3. Run its slots and `initExprs` through the engine hook.

Keep the frame construction in `interpreter/`; `vm.rs` must not learn about
initializer frames.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS, differential included.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: evaluate the superclass clause and initialize per level"
```

### Task 8: Class sides become a lazily created metaclass

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the send path for a
  class-object receiver), `crates/ns-runtime/src/mixin.rs` (nothing new —
  `class_side` already exists from Task 3)
- Test: `crates/ns-runtime/tests/nested_classes.rs` (extend),
  `tests-fixtures/class_side.ns` + `.expected`

**Interfaces:**
- Consumes: `Mixin.class_side` (Task 3), `apply_mixin` (Task 1).
- Produces: `Interpreter::metaclass_of(&mut self, class: &ClassRef)
  -> Option<ClassRef>` — `None` when the mixin declares no class side.

A class side is the mixin for the class's metaclass (§6.3). The metaclass is
created on first use, so a class without a class side allocates nothing. The
full §3.2 model — every class an instance of a unique metaclass, metaclasses
as direct subclasses of a reified `Class` — is explicitly out of scope, and
the spec records that as a deviation rather than an oversight.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_class_side_method_dispatches_on_the_class_object() {
    let src = "\
Newspeak3
'Test'
class Widget new = Object new () (
) : (
    public describe = ( ^'a widget class' )
)
";
    let (mut interp, class, resolved) = load(src);
    let described = expect_ok(
        interp.send(
            Value::Obj(class),
            Symbol::intern("describe"),
            vec![],
            &resolved.bindings,
            None,
        ),
        "class-side send",
    );
    // Compare through the string builtin rather than Debug: Value has no
    // usable Debug (the bootstrap Object class is its own class).
    assert_eq!(string_contents(&described), "a widget class");
}
```

`string_contents` reads `ObjectData::Str`; copy the helper from
`crates/ns-runtime/tests/builtins_text.rs`.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test nested_classes
a_class_side_method_dispatches_on_the_class_object`
Expected: FAIL with `doesNotUnderstand: #describe`.

- [x] **Step 3: Write the implementation**

When a send's receiver is a class object, consult its metaclass before
falling through to the existing class-object behaviour: `metaclass_of`
applies the mixin's `class_side` (to the superclass's metaclass application,
when the superclass has one) and caches the resulting class object on the
class it belongs to. Dispatch then proceeds exactly as for any other
receiver — no separate lookup algorithm, so the access-control rules and the
`doesNotUnderstand:` text stay identical.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: dispatch class-side methods through a lazy metaclass"
```

## Phase 3 — Engines

### Task 9: Key compiled code by mixin, not by class

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`class_key` → `mixin_key`
  usage in `CompiledModule`), `crates/ns-runtime/src/compiler/mod.rs`
  (compile the whole mixin tree), `crates/ns-runtime/src/interpreter/mod.rs`
  (the hooks' code lookup), `crates/ns-cli/src/main.rs` (`disasm`)
- Test: `crates/ns-runtime/tests/compiler.rs` (extend),
  `crates/ns-cli/tests/disasm_snapshots.rs`

**Interfaces:**
- Consumes: `mixin_key` (Task 1), the mixin tree (Task 3).
- Produces: `CompiledModule { methods: HashMap<usize, Rc<CompiledCode>>,
  initializers: HashMap<usize, Rc<CompiledCode>>, bindings }` where both maps
  are keyed by `mixin_key`/`method_key` — same types, different meaning.

`class_key` is `Rc::as_ptr(class)` today. With one class object per enclosing
instance that would compile the same method body once per instance, which
§6.2 rules out: applications of a mixin share code. This task is the reason
the milestone can stay fast.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_nested_class_is_compiled_once_for_all_enclosing_instances() {
    let (mut interp, class, resolved) = load(NESTED_SRC);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class.clone(), vec![platform.clone()], &resolved.bindings),
        "first Outer",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform], &resolved.bindings),
        "second Outer",
    );
    let a = expect_ok(interp.send(one, Symbol::intern("innerClass"), vec![], &resolved.bindings, None), "a");
    let b = expect_ok(interp.send(two, Symbol::intern("innerClass"), vec![], &resolved.bindings, None), "b");
    // Same compiled code object for the same method of the same mixin.
    assert!(std::rc::Rc::ptr_eq(&code_for(&interp, &a, "value"), &code_for(&interp, &b, "value")));
}
```

`code_for` looks the method up through the class object's application mixin
and indexes `interp.compiled_module()`'s `methods` by `method_key`.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test compiler
a_nested_class_is_compiled_once_for_all_enclosing_instances`
Expected: FAIL — the compiler never visited the nested declaration.

- [x] **Step 3: Write the implementation**

`compile_module` walks the mixin tree instead of the single root class,
emitting one `CompiledCode` per method of every mixin (nested and class-side
included) and one per initializer, keyed by `mixin_key`. `disasm` takes a
mixin path (`Outer.Inner`) so nested code is addressable.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS, including the disasm
snapshots (update them deliberately if the listing gained nested entries).

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: key compiled code by mixin so applications share it"
```

### Task 10: Dispatch reports the defining level

**Files:**
- Modify: `crates/ns-runtime/src/dispatch.rs` (`Lookup`),
  `crates/ns-runtime/src/interpreter/mod.rs` (`invoke_method`, frame
  construction), `crates/ns-runtime/src/interpreter/eval.rs`
  (`dispatch_super_send`)
- Test: `crates/ns-runtime/tests/nested_classes.rs` (extend),
  `tests-fixtures/nested_super.ns` + `.expected`

**Interfaces:**
- Consumes: the application chain (Task 2), the class-object chain (Task 2).
- Produces: `Lookup::Method(Rc<ast::MethodDecl>, ClassRef)` where the
  `ClassRef` is the class object of the *defining* level — reached by walking
  the receiver's class-object chain as many steps as the application chain
  walked.

`Frame.current_class` is what `outer` and `super` read, and it must be the
per-instance class object of the level that defines the running method, not
the receiver's own class. Milestone 4's inline caches record the same level
index `k` next to the application, which is why this is worth getting exactly
right now rather than at cache time.

- [x] **Step 1: Write the failing test**

`tests-fixtures/nested_super.ns` — a nested class inheriting from another
nested class of the same enclosing object, so that `super` must walk the
application chain while `outer` must walk the class-object chain of the level
that *defines* the running method:

```newspeak
Newspeak3
'Test'
class Outer usingPlatform: p = Object new (| t ::= p transcript. |) (
    label = ( ^'outer' )
    public class Base = Object new () (
        public describe = ( ^outer Outer label )
    )
    public class Derived = Base new () (
        public describe = ( ^(super describe), '/derived' )
    )
    public main: platform args: args = (
        t show: Derived new describe; newLine
    )
)
```

`tests-fixtures/nested_super.expected`:

```
outer/derived
```

If `current_class` were the receiver's own class rather than the defining
level's, `super describe` would either recurse into `Derived describe` or
`outer Outer label` would reach `nil` — both visible in this one fixture.

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test run_fixtures nested_super`
Expected: FAIL — `outer` reaches the wrong enclosing object, or `nil`.

- [x] **Step 3: Write the implementation**

Walk both chains in step in `lookup_public` / `lookup_protected` /
`lookup_defined_for`, returning the class object that sits at the level where
the method dictionary hit. Set `Frame.current_class` from it.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS, differential included.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: resolve current_class to the defining level's class object"
```

## Phase 4 — The image

### Task 11: A symbol table in the image

**Files:**
- Create: `crates/ns-runtime/src/image/symbols.rs`
- Modify: `crates/ns-runtime/src/image/{mod,encode,decode}.rs`
- Test: `crates/ns-runtime/tests/image.rs` (extend)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `symbols::Builder` with `intern(&mut self, s: &str) -> u32` used
  while encoding, and `symbols::Table` with `get(&self, id: u32)
  -> Result<Symbol, String>` used while decoding.

Every selector, every method-dictionary key and every lexical-send path
segment is written inline today, and the same selector recurs at very many
call sites because dispatch is by name. One table at the head, referenced by
index, deduplicates them and makes interning one pass instead of one
`Symbol::intern` per send instruction.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_repeated_selector_is_stored_once() {
    let bytes = snapshot_of(
        "Newspeak3\n'T'\nclass C new = Object new () (\
         a = ( ^self b ) b = ( ^self b ) )\n",
    );
    // The selector "b" occurs at three sites but must appear once in the
    // symbol table. Counting raw occurrences of the byte string is enough to
    // catch a regression to inline strings.
    assert_eq!(occurrences(&bytes, b"b"), 1);
}

#[test]
fn an_out_of_range_symbol_id_is_an_error_not_a_panic() {
    let mut bytes = snapshot_of(TRIVIAL_MODULE);
    corrupt_first_symbol_id(&mut bytes);
    assert!(ns_runtime::image::load(&bytes).is_err());
}
```

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test image` — Expected: FAIL, no symbol table.

- [x] **Step 3: Write the implementation**

Encode: collect into a `Vec<String>` with a `HashMap<String, u32>` for
dedup, write the count then the strings, and replace every `write_str` of a
selector or name with `write_u32(id)`.

Decode: read the count, bound it against the remaining input length before
allocating, read each string, intern once, and hand out `Symbol`s by index.
Every id is range-checked at the point of use — an out-of-range id is an
`Err`, never an index panic.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "perf: dedupe selectors through an image symbol table"
```

### Task 12: The mixin table

**Files:**
- Create: `crates/ns-runtime/src/image/mixins.rs`
- Modify: `crates/ns-runtime/src/image/{mod,encode,decode}.rs`
- Test: `crates/ns-runtime/tests/image.rs` (extend)

**Interfaces:**
- Consumes: the symbol table (Task 11), the mixin tree (Task 3).
- Produces: `mixins::encode(w: &mut Writer, root: &Rc<Mixin>, syms: &mut
  symbols::Builder, code: &CodeIds) -> HashMap<usize, u32>` (mixin_key →
  table id) and `mixins::decode(r: &mut Reader, syms: &symbols::Table)
  -> Result<Vec<Rc<Mixin>>, String>`.

One row per declaration: name, own slot names, primary factory selector and
arity, method table (selector → code id), nested declaration ids with their
accessor names and accessibility, class-side mixin id, initializer code id,
provenance span.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_nested_mixin_round_trips() {
    let bytes = snapshot_of(NESTED_SRC);
    let loaded = ns_runtime::image::load(&bytes).expect("load");
    let borrowed = loaded.root_class.borrow();
    let ns_runtime::ObjectData::Class(data) = &borrowed.data else { panic!() };
    assert_eq!(data.application.mixin.nested.len(), 1);
    assert_eq!(data.application.mixin.nested[0].name, Symbol::intern("Inner"));
}

#[test]
fn a_deeply_nested_mixin_tree_does_not_blow_the_stack() {
    // The nesting depth is input-controlled, so decoding must not recurse
    // proportionally to it. 100_000 declared levels must produce an Err or a
    // successful iterative decode -- never an abort.
    let bytes = synthetic_mixin_chain(100_000);
    let _ = ns_runtime::image::load(&bytes);
}

#[test]
fn a_mixin_id_pointing_past_the_table_is_an_error() {
    let mut bytes = snapshot_of(NESTED_SRC);
    corrupt_nested_mixin_id(&mut bytes);
    assert!(ns_runtime::image::load(&bytes).is_err());
}
```

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test image` — Expected: FAIL.

- [x] **Step 3: Write the implementation**

Flatten the tree while encoding: assign ids breadth-first and reference
children by id, so no structure is nested in the byte stream and decoding is
a loop over rows rather than a recursion. Resolve child ids into `Rc<Mixin>`
in a second pass, after every row exists.

Validate before use: the count against the remaining length, every child id
against the row count, every code id against the code table, and every
accessor name against the symbol table.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: serialize declarations as a flat mixin table"
```

### Task 13: Class objects in the object graph

**Files:**
- Modify: `crates/ns-runtime/src/image/{encode,decode}.rs`
  (`encode_class_tag` and its decode counterpart)
- Test: `crates/ns-runtime/tests/image.rs` (extend)

**Interfaces:**
- Consumes: the mixin table (Task 12), `apply_mixin` (Task 1).
- Produces: a class object encoded as `{mixin id, superclass class-object id
  or builtin tag, enclosing object id}`; applications rebuilt on decode.

This retires the binary `builtin`/`UserRoot` tag whose own comment records
that a second user-defined class would decode as the root — a wrong class,
not a decode error. Nesting is exactly what would have triggered it.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn sharing_survives_a_round_trip() {
    // Two enclosing instances, one shared application before the snapshot.
    // If the rebuild is wrong, Milestone 4's call sites go megamorphic after
    // every load -- which is why this is a property test, not a shape test.
    let bytes = snapshot_with_two_outers();
    let loaded = ns_runtime::image::load(&bytes).expect("load");
    let (a, b) = two_inner_classes(&loaded);
    assert!(!std::rc::Rc::ptr_eq(&a, &b), "distinct class objects");
    assert!(
        std::rc::Rc::ptr_eq(&application_of(&a), &application_of(&b)),
        "one shared application"
    );
}

#[test]
fn a_second_user_class_no_longer_decodes_as_the_root() {
    let bytes = snapshot_of(NESTED_SRC);
    let loaded = ns_runtime::image::load(&bytes).expect("load");
    assert_ne!(class_name(&loaded, "Inner"), class_name(&loaded, "Outer"));
}
```

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test image` — Expected: FAIL.

- [x] **Step 3: Write the implementation**

Encode class objects like any other object; decode them in two passes so a
class object can reference a superclass or enclosing object that appears
later in the table. Rebuild each application through `apply_mixin`, which
gives sharing back for free because the cache key is content-derived.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: encode class objects in the graph and rebuild applications"
```

### Task 14: A source-free load path

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`snapshot`, `load`,
  `LoadedImage`), `crates/ns-runtime/src/types.rs` (`Frame` pc
  discriminator), `crates/ns-cli/src/main.rs` (`snapshot --debug-source`)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-cli/tests/snapshot_load.rs`

**Interfaces:**
- Consumes: Tasks 11–13.
- Produces: `LoadedImage { interp, root_class, roots }` — `bindings` is gone;
  `snapshot(source: Option<&str>, ...)` where `None` writes no source blob.

`load` currently reads the stored source and re-runs parse → resolve →
bootstrap → `load_module` to rebuild the class graph and the bindings map.
With a mixin table there is nothing left for it to rebuild, and the front end
leaves the load path — along with the re-parsing of attacker-controlled text.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_source_free_image_still_runs() {
    let bytes = snapshot_of_without_source(NESTED_SRC);
    assert!(!contains(&bytes, b"class Outer"), "no source in the image");
    let loaded = ns_runtime::image::load(&bytes).expect("load");
    // and the entry send still works through the decoded code table
    assert!(matches!(run_entry(loaded), Value::SmallInt(42)));
}

#[test]
fn the_debug_variant_still_carries_source() {
    let bytes = snapshot_of_with_source(NESTED_SRC);
    assert!(contains(&bytes, b"class Outer"));
    assert!(ns_runtime::image::load(&bytes).is_ok());
}

#[test]
fn a_frame_says_which_engine_its_pc_belongs_to() {
    let loaded = ns_runtime::image::load(&snapshot_with_a_live_frame()).expect("load");
    assert_eq!(pc_kind_of_first_frame(&loaded), PcKind::Bytecode);
}
```

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test image` — Expected: FAIL.

- [x] **Step 3: Write the implementation**

Header gains a flag byte for the optional source blob. `load` decodes symbols,
mixins, code, then the graph, and installs `Engine::Bytecode` as it does
today. `Frame` gains a `pc_kind` field, written and validated with the frame
table, because `pc` means a statement index under the tree-walker and an
instruction index under the VM and frames are serialized.

Delete the re-parse path rather than leaving it unreachable.

`Interpreter::send` still takes a `&Rc<HashMap<Span, Binding>>`, and a
source-free image has no bindings map to give it. Pass an empty one, built
once in `load` and carried on `LoadedImage` as `empty_bindings`. That is
sound precisely because the bytecode engine never resolves a binding by span
at run time — the only `bindings.get(&span)` call sites are in
`compiler/lower.rs` (compile time) and `interpreter/eval.rs` (the
tree-walker, which cannot run from an image at all). Say that in the field's
doc comment, so nobody later "fixes" it by reconstructing a real map.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: load an image without its source"
```

### Task 15: Golden bytes, and the determinism test that proves nothing

**Files:**
- Create: `crates/ns-runtime/tests/fixtures/golden-image.bin`
- Modify: `crates/ns-runtime/tests/image.rs`
- Test: same

**Interfaces:**
- Consumes: the finished format (Tasks 11–14).
- Produces: a byte-exact fixture pinning the wire format.

Milestone 2 deferred this and tied it to "the first `FORMAT_VERSION` bump".
There will be no bump — the project has no external users — so it lands here,
with the format change it is meant to police.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn the_wire_format_matches_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SRC);
    let expected = include_bytes!("fixtures/golden-image.bin");
    assert_eq!(bytes, expected, "the image wire format changed -- if that is \
        intended, regenerate the fixture in the same commit and say why");
}

#[test]
fn the_code_table_encoding_is_deterministic() {
    // Compiling twice is the point: the previous version of this test shared
    // one HashMap instance between both encodes, so it would have passed
    // without the sort that actually makes encoding deterministic.
    let a = snapshot_of_without_source(GOLDEN_SRC);
    let b = snapshot_of_without_source(GOLDEN_SRC);
    assert_eq!(a, b);
}
```

- [x] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test image` — Expected: FAIL, no fixture yet.

- [x] **Step 3: Generate the fixture and fix the determinism test**

Generate the fixture with an ignored test that writes it, run once, then
commit the bytes:

```rust
#[test]
#[ignore = "regenerates tests/fixtures/golden-image.bin; run deliberately"]
fn regenerate_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SRC);
    std::fs::write("tests/fixtures/golden-image.bin", bytes).expect("write fixture");
}
```

Run it with `cargo test -p ns-runtime --test image regenerate_the_golden_fixture
-- --ignored`. Then make the determinism test call
`snapshot_of_without_source` twice, as written above, so it compiles the
module twice instead of reusing one `CompiledModule`.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "test: pin the image wire format with golden bytes"
```

## Phase 5 — Running real modules

### Task 16: A factory-aware entry point

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (`cmd_run`, `cmd_snapshot`)
- Test: `crates/ns-cli/tests/run.rs` (extend)

**Interfaces:**
- Consumes: `Mixin.factory` (Task 3).
- Produces: an entry point that sends the root class's declared primary
  factory message.

`cmd_run` sends `usingPlatform: platform` to every module, so `ADummyPush`,
whose primary factory is `new`, fails with "wrong number of factory
arguments: factory takes 0, got 1" — a CLI defect, not a language gap.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_zero_argument_factory_is_sent_without_the_platform() {
    let out = run_cli(&["run", "corpus/modules/ADummyPush.ns", "--send", "foo"]);
    assert_eq!(out.status.code(), Some(0), "stderr was: {}", out.stderr);
    assert_eq!(out.stdout.trim(), "99");
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test run
a_zero_argument_factory_is_sent_without_the_platform`
Expected: FAIL with the factory-arity error.

- [x] **Step 3: Write the implementation**

Read the root mixin's `factory` pattern. A unary factory takes no arguments;
a keyword factory whose single parameter names the platform takes
`[platform]`. Any other arity is a clear error naming the declared selector,
raised identically under both engines.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: send the root class's declared factory message"
```

### Task 17: The `run-corpus` gate

**Files:**
- Create: `corpus/runnable.toml`
- Modify: `crates/ns-cli/src/main.rs` (the `run-corpus` subcommand),
  `CLAUDE.md` (the gate's counter line)
- Test: `crates/ns-cli/tests/cli.rs` (extend)

**Interfaces:**
- Consumes: Task 16's entry point, both engines.
- Produces: `ns-next run-corpus corpus/modules` printing
  `ran N, failed 0, skipped M` and exiting non-zero on any failure.

Built like `parse-corpus`, with a whitelist rather than a blacklist: a module
is expected to run only once someone has put it on the list with its expected
output. That makes growth deliberate and a regression loud.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn the_run_corpus_gate_reports_its_counts() {
    let out = run_cli(&["run-corpus", "corpus/modules"]);
    assert_eq!(out.status.code(), Some(0), "stderr was: {}", out.stderr);
    assert!(out.stdout.contains("ran "), "stdout was: {}", out.stdout);
    assert!(out.stdout.contains("failed 0"));
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test cli the_run_corpus_gate_reports_its_counts`
Expected: FAIL — unknown subcommand.

- [x] **Step 3: Write the implementation**

`corpus/runnable.toml` starts with the modules that pass once Tasks 1–16 are
in — begin with `ADummyPush.ns`, add every further module deliberately, each
with `expected_stdout` and `expected_status`. The subcommand runs each entry
under both `--engine=bytecode` and `--engine=tree-walk` and fails if the two
disagree, which makes the gate a differential harness over real modules as
well as a non-regression counter.

**Widening the platform is part of this step, and only this step.** When a
module you want on the whitelist fails because `platform.rs` does not answer
some selector, add exactly that capability and note in the commit which
whitelisted module demanded it. Do not add capabilities no listed module
needs — a speculative platform library is what this milestone's non-goals
exclude, and an unused capability has no test that would catch it rotting.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.
Run: `cargo run -p ns-cli -- run-corpus corpus/modules` — Expected:
`failed 0`.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add a run-corpus gate over a curated whitelist"
```

### Task 18: `--bench`

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (the `bench` subcommand)
- Test: `crates/ns-cli/tests/cli.rs` (extend)

**Interfaces:**
- Consumes: both engines.
- Produces: `ns-next bench <file> [--send selector] [--iterations N]`
  reporting per-engine timings after a warm-up.

Milestone 4 is the inline-cache milestone. Without a before-number no claimed
improvement is verifiable, and the project treats performance as its highest
priority — so the harness ships before the optimization, not after.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn bench_reports_both_engines() {
    let out = run_cli(&["bench", "tests-fixtures/while_loop.ns", "--iterations", "3"]);
    assert_eq!(out.status.code(), Some(0), "stderr was: {}", out.stderr);
    assert!(out.stdout.contains("bytecode"));
    assert!(out.stdout.contains("tree-walk"));
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-cli --test cli bench_reports_both_engines`
Expected: FAIL — unknown subcommand.

- [x] **Step 3: Write the implementation**

Discard a warm-up run, then time `--iterations` runs per engine and print
each engine's median. Never judge performance from a debug build: the
subcommand's help text says to measure with `--release`.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace` — Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add a bench mode with per-engine timings"
```

### Task 19: Documentation

**Files:**
- Modify: `CLAUDE.md` (invariants and gates), `docs/PROGRESS.md` (the M3 row
  and its section)

**Interfaces:**
- Consumes: everything above.
- Produces: repository documentation describing the milestone's result.

- [x] **Step 1: Update `CLAUDE.md`**

Add the three-layer model to "Invariants that are easy to break": a class
object is per enclosing instance, behaviour is the application, and inline
caches key on the application. Add the `run-corpus` counter line beside the
`parse-corpus` one.

- [x] **Step 2: Update `docs/PROGRESS.md`**

Set the M3 row to Complete with the date, link the plan beside the spec, and
add the milestone section describing what shipped — the current state, not
the sequence of changes that produced it.

- [x] **Step 3: Run the gates one last time**

Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo fmt --all`, `cargo run -p ns-cli -- parse-corpus corpus/modules`,
`cargo run -p ns-cli -- run-corpus corpus/modules`.

- [x] **Step 4: Commit**

```bash
git add CLAUDE.md docs/PROGRESS.md
git commit -m "docs: record Milestone 3 in the invariants and the tracker"
```
