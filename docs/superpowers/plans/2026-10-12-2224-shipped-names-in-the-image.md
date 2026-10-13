# Shipped names in the image — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The bootstrap builds the whole shipped world, and an image names every object the binary owns -- a shipped module's instance, each class its root declares, their metaclasses and the levels their `<:` clauses add -- by module, declaration and kind, through one section resolved by table lookup before the graph, in place of the builtin class tags, `CLASS_METACLASS`, `MIXIN_BUILTIN`, the platform paths, `VALUE_PLATFORM`, `CLASS_MODULE`, `CLASS_MODULE_LEVEL` and every tag-named hash but `HASHED_SHIPPED`.

**Architecture:** A new `ShippedWorld` on the `Mutator` (traced like the kernel and the platform) is filled once at the end of the boot: the kernel, then `CollectionsFactory` and `PlatformFactory` instantiated through their own factories, every nested class of every shipped root built by running its accessor's creation from Rust, then every object recorded under its name with the positions it may stand in. The image gains a fingerprint of the shipped world in its header and a shipped-name section before the graph; graph references to shipped objects become an index into that section. The program's module roots become ordinary class records, so the module section shrinks to provenance.

**Tech Stack:** Rust 1.98.0 (`crates/ns-runtime`, `crates/ns-cli`, `crates/ns-analyzer` tests), Newspeak (`crates/ns-runtime/core-library/PlatformFactory.ns`).

**Spec:** `docs/design/image-format.md`, "Planned: shipped names in the image" (decided with the user on 2026-10-12). Read it in full before any task. Background: `docs/design/core-library.md`, "The VM mirror is the whole Rust boundary", "Platform paths in the image (plan 1)", "The module table and the boot"; `docs/design/image-format.md`, "What is *not* serialized", "Verification", "The image under generations".

## Global Constraints

- All code, comments, commit messages and documentation in English.
- Names are written out (`CLAUDE.md`, "Names are written out"): `mutator`, `index`, `interpreter`, `world_index`, never `m`, `idx`, `interp`. Clippy's `min_ident_chars` enforces it.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`; tests copy a neighbour's `expect_ok`.
- An image is untrusted input: every count is bounded by `decode::bounded_count` before it allocates; no byte sequence panics.
- A wire tag is assigned once: new tags take exactly these numbers -- `CLASS_SHIPPED` = 8 (class tag), `MIXIN_SHIPPED` = 2 (mixin reference), `VALUE_SHIPPED_MODULE` = 11 (value tag), `HASHED_SHIPPED` = 7 (hash tag); retired numbers go into the `RETIRED_*` arrays and are never reused.
- Every new Rust field holding a handle needs its `Trace` (`heap/world.rs`, `StepRoots`).
- No golden's `status`, `stdout` or `stderr` changes. In particular `goldens/library/object/identity_hash.ns` expects the first hashes after boot or load to be 1 and 2, so **the boot must draw no identity hash**.
- The VM mirror's six locks hold throughout (`CLAUDE.md`, "The VM mirror is a capability"); Tasks 4 and 6 rewrite locks (3) and (4) as the spec says, and never loosen anything else.
- `CollectionsFactory` keeps its factory `usingKernel:`; it is given no mirror.
- No new external dependency.
- Commits: conventional (`feat:`, `refactor:`, `test:`, `docs:`), ending with
  ```
  Co-Authored-By: <the implementing model's name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01BcChjJmpsEYtz6GZ8wwPBo
  ```
- Per-commit gates (every task's last step runs them, in the foreground, `timeout: 600000` on each long one):
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace
  cargo test --workspace --doc
  cargo run -p ns-analyzer -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library`. The end-of-batch gates (stress modes, `run-corpus` under stress, the benchmarks profile, `verifyPlugin`) are **not** run per task; the controller runs them once at the end with the `finishing-a-batch` skill. Work on the checked-out branch `feature/image-shipped-class-names` in the main working directory.

## Review Focus

1. **A boot that draws an identity hash.** Building every shipped class runs superclass clauses and initializers; if any draws a hash, `identity_hash.ns` prints other numbers. Task 2 pins "a fresh boot's next hash is 1".
2. **A name that resolves to a twin.** A decoded `Kernel Array` (or `CollectionsFactory List`) must be the very object the loading boot built, or identity-comparing Rust (`class_builtin`, the exception classes, `resolve.rs`) silently misbehaves. Task 4 pins `same` for a builtin, a metaclass, a level and a shipped non-builtin class after a round trip.
3. **A crafted name in a forbidden position.** `Kernel Top` as a value, `Kernel Activation` as a value or a non-frame header, a module root as a record's class, `VALUE_SHIPPED_MODULE` outside a frame, a non-kernel name in a started image -- each is a `DecodeError`, never a panic and never a load. Task 4 has one test per position.
4. **A crafted shipped-name section.** Unknown module, unknown declaration, unknown kind byte, a level index past the clauses, a name given twice, a count past the bytes left or past the maximum -- each a `DecodeError`. Task 4.
5. **The platform as a record.** A snapshot holding the platform with filled lazy slots (`transcript`, `collections`) loads, its copy `=` the loading platform, its facets usable; a record of a shipped non-builtin class with the wrong field count is refused. Task 4.

---

### Task 1: The shipped instances and `moduleInstanceNamed:`

Today `CollectionsFactory` is compiled and instantiated on the first `platform collections`, through the mirror leaf `moduleNamed:`, which builds a fresh root class on every call (`builtins/module.rs`). After this task every shipped module has exactly one instance, built at boot and kept in a table on the `Mutator`, and the leaf answers that instance.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/shipped_world.rs` (the table; Task 2 extends it)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (module declaration, `Mutator` field, `bootstrap_with`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::new`, `StepRoots`, `split`)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`ShippedModule` gains its factory; `install_platform` becomes `install_shipped_modules`)
- Modify: `crates/ns-runtime/src/builtins/module.rs`, `crates/ns-runtime/src/builtins/primitive.rs` (`ModulePrimitive::Named` → `ModulePrimitive::InstanceNamed`, row `moduleInstanceNamed:ifFail:`)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (the lazy `collectionsModule`)
- Test: `crates/ns-runtime/src/interpreter/core_library.rs` (unit tests), `crates/ns-runtime/tests/platform.rs`

**Interfaces:**
- Produces: `pub(crate) struct ShippedWorld<'gc>` with `pub(crate) fn module_instance(&self, name: Symbol) -> Option<ObjectReference<'gc>>`, `pub(crate) fn add_module_instance(&mut self, name: Symbol, instance: ObjectReference<'gc>)`, `pub(crate) fn module_instances(&self) -> &[(Symbol, ObjectReference<'gc>)]`; `Mutator.shipped_world: ShippedWorld<'gc>`; `ShippedModule { name, source, factory: ShippedFactory }`, `enum ShippedFactory { MirrorAndKernel, Kernel }`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/platform.rs`, beside `platform_collections_answers_the_collections_modules_list`:

```rust
/// Every shipped module has one instance, built at boot: the collections
/// facet's module is the same object before and after a second platform
/// is made, and is there before any program asked for `platform collections`.
#[test]
fn the_collections_module_is_built_once_at_boot() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let first = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let second_platform = expect_ok(
        interpreter.send(&platform, Symbol::intern("class"), &[]),
        "platform class",
    );
    let second_platform = expect_ok(
        interpreter.send(&second_platform, Symbol::intern("new"), &[]),
        "platform class new",
    );
    let second = expect_ok(
        interpreter.send(&second_platform, Symbol::intern("collections"), &[]),
        "second platform collections",
    );
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let module_of = |facet: &Root| {
            let facet = mutator.fetch(facet).as_object().expect("a facet is an object");
            heap.class_enclosing(heap.object_class(facet)).expect("a facet's class is nested")
        };
        assert!(module_of(&first).same(module_of(&second)));
    });
}
```

If `platform.rs` has no `expect_ok`, copy the neighbour helper from `tests/image.rs` (`expect_ok`, around line 2649). If `class_enclosing`/`object_class`/`same` are not reachable from an integration test, assert through Newspeak instead: run a module whose `main:` prints `(platform collections == platform class new collections) printString` and expect `true` -- whichever the existing tests in `platform.rs` already do for `second_list.same(list)` (around line 118-180) is the pattern to copy.

In `core_library.rs`'s `mod tests`, replace `module_named_answers_a_shipped_root_class_and_declines_anything_else` with:

```rust
/// The leaf answers the canonical instance the boot built, the same object
/// on every request, and declines a name the table does not list and a
/// subject that is no symbol, so the forwarder's failure block runs.
#[test]
fn module_instance_named_answers_the_boots_instance_and_declines_anything_else() {
    let mut interpreter = crate::Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let name = mutator.new_symbol(Symbol::intern("CollectionsFactory"));
        let Some(Ok(first)) = module_instance_named(mutator, name) else {
            panic!("moduleInstanceNamed: #CollectionsFactory answers no instance");
        };
        let Some(Ok(second)) = module_instance_named(mutator, name) else {
            panic!("moduleInstanceNamed: #CollectionsFactory answers no instance");
        };
        let (Some(first), Some(second)) = (first.as_object(), second.as_object()) else {
            panic!("moduleInstanceNamed: answers an object");
        };
        assert!(first.same(second));
        assert!(!mutator.heap().is_class(first));
        assert_eq!(
            mutator.heap().class_display_name(mutator.heap().object_class(first)),
            "CollectionsFactory"
        );
        let unknown = mutator.new_symbol(Symbol::intern("NoSuchModule"));
        assert!(module_instance_named(mutator, unknown).is_none());
        let string = Value::object(mutator.new_string("CollectionsFactory".to_string()));
        assert!(module_instance_named(mutator, string).is_none());
    });
}
```

with the helper renamed `module_instance_named` and calling `ModulePrimitive::InstanceNamed`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime module_instance_named the_collections_module_is_built_once_at_boot`
Expected: compile error (`ModulePrimitive::InstanceNamed` does not exist).

- [ ] **Step 3: The table**

`crates/ns-runtime/src/interpreter/shipped_world.rs`:

```rust
//! The shipped world: every object the binary owns and an image may name
//! (`docs/design/image-format.md`, "Planned: shipped names in the image").
//! Filled once, at the end of the boot, and never changed afterwards.

use crate::heap::value::ObjectReference;
use crate::trace::{Trace, Tracer};
use crate::Symbol;

/// The shipped modules' instances, in the order the boot built them: the
/// kernel first.
#[derive(Default)]
pub(crate) struct ShippedWorld<'gc> {
    module_instances: Vec<(Symbol, ObjectReference<'gc>)>,
}

impl<'gc> ShippedWorld<'gc> {
    /// Records `instance` as the one instance of the shipped module `name`.
    /// The boot names each module once; a second instance is a defect in
    /// the binary.
    pub(crate) fn add_module_instance(&mut self, name: Symbol, instance: ObjectReference<'gc>) {
        assert!(
            self.module_instance(name).is_none(),
            "internal: the shipped module {} is instantiated twice",
            name.as_str()
        );
        self.module_instances.push((name, instance));
    }

    /// The one instance of the shipped module `name`, or `None` when no
    /// shipped module has that name.
    pub(crate) fn module_instance(&self, name: Symbol) -> Option<ObjectReference<'gc>> {
        self.module_instances
            .iter()
            .find(|(module, _)| *module == name)
            .map(|(_, instance)| *instance)
    }

    pub(crate) fn module_instances(&self) -> &[(Symbol, ObjectReference<'gc>)] {
        &self.module_instances
    }
}

impl<'gc> Trace for ShippedWorld<'gc> {
    fn trace(&mut self, tracer: &mut dyn Tracer) {
        for (_, instance) in &mut self.module_instances {
            tracer.object(instance);
        }
    }
}
```

Adapt the `use` paths and the `Trace` signature to what `crate::trace` actually declares (read `src/trace.rs` and an existing `impl Trace`, e.g. `StepRoots`'s in `heap/world.rs`). Declare the module in `interpreter/mod.rs`, add `pub(crate) shipped_world: ShippedWorld<'gc>` to `Mutator` beside `kernel`, initialize it in `World::new` (`ShippedWorld::default()`), add `&mut` to it in `StepRoots`, destructure it in `split`, and trace it in `StepRoots`'s `Trace`.

- [ ] **Step 4: Boot every shipped module**

In `core_library.rs`:
- `ShippedModule` gains `pub(crate) factory: ShippedFactory`, with
  ```rust
  /// What a shipped module's factory takes: `PlatformFactory` the mirror
  /// and the kernel (`usingVmMirror:kernel:`), `CollectionsFactory` the
  /// kernel alone (`usingKernel:`), since it reaches no leaf and so is
  /// given no mirror.
  pub(crate) enum ShippedFactory {
      MirrorAndKernel,
      Kernel,
  }
  ```
  `PlatformFactory` → `MirrorAndKernel`, `CollectionsFactory` → `Kernel`.
- Rename `install_platform` to `install_shipped_modules` (update `bootstrap_with`). It first records the kernel: `mutator.shipped_world.add_module_instance(Symbol::intern("Kernel"), kernel_object)` from `mutator.kernel`. Then for every `SHIPPED_MODULES` entry in table order: `load_module(&resolved_shipped_module(name))`, `instantiate` with `[mirror, kernel]` or `[kernel]` per its factory (`booted(..., "<name> <factory selector>")`), for a `MirrorAndKernel` module `check_factory_holds_the_mirror`, then `add_module_instance`. Last, send `newPlatform` to the `PlatformFactory` instance (`mutator.shipped_world.module_instance(Symbol::intern("PlatformFactory"))`) and set `platform_singleton` as today.
- Rewrite the doc comments of `install_shipped_modules` and `RESOLVED_SHIPPED_MODULES`: every shipped module is resolved, compiled and instantiated at every boot now, because an image names its objects and the loader runs nothing the image chooses (`image-format.md`, "Planned: shipped names in the image"). Drop the paragraph about `image::load` resolving platform paths only in Task 4, where the paths go.

- [ ] **Step 5: The leaf**

`primitive.rs`: rename `ModulePrimitive::Named` to `ModulePrimitive::InstanceNamed`; the row becomes
```rust
mirror(
    "moduleInstanceNamed:ifFail:",
    // Unary: the module's name is the row's subject.
    "moduleInstanceNamed",
    Primitive::Module(ModulePrimitive::InstanceNamed),
),
```
`builtins/module.rs`:
```rust
ModulePrimitive::InstanceNamed => {
    let name = receiver.as_object()?;
    let name = mutator.heap().symbol_value(name)?;
    let instance = mutator.shipped_world.module_instance(name)?;
    Some(Ok(Value::object(instance)))
}
```
and rewrite its module doc: it answers the canonical instance the boot built, so every platform shares one collections module, and it builds nothing. `PlatformFactory.ns`:
```newspeak
  /// The one collections module, the instance the boot built.
  lazy private collectionsModule = vmMirror moduleInstanceNamed: #CollectionsFactory ifFail: [
    nil.
  ].
```
keeping the existing `///` documentation's content where it still holds (what it answers, that it is built once) and the comment rule of `CLAUDE.md` ("Every member of a shipped core-library module is documented"). Grep the workspace for `moduleNamed` (`rg moduleNamed --glob '!prior-art'`) and update every Rust and `.ns` hit; documentation hits wait for Task 6.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-runtime module_instance_named the_collections_module_is_built_once_at_boot platform`
Expected: PASS. Then `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library`.

- [ ] **Step 7: Gates and commit**

Run the per-commit gates. Commit: `feat: build every shipped module once at boot and answer it by moduleInstanceNamed:`.

---

### Task 2: The shipped world, built whole, with what may be named where

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs`
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (call the builder at the end of `install_shipped_modules`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (a host entry that builds one nested class)
- Test: `crates/ns-runtime/src/interpreter/shipped_world.rs` (`mod tests`), `crates/ns-runtime/tests/platform.rs`

**Interfaces:**
- Consumes: Task 1's `ShippedWorld`, `module_instances()`.
- Produces (Task 4 relies on exactly these):
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
  pub(crate) enum ShippedKind { ModuleInstance, Class, Metaclass, Level(u32), LevelMetaclass(u32) }
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
  pub(crate) struct ShippedName { pub(crate) module: Symbol, pub(crate) declaration: Symbol, pub(crate) kind: ShippedKind }
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub(crate) struct Positions { pub(crate) value: bool, pub(crate) superclass: bool, pub(crate) header: bool, pub(crate) frame_class: bool, pub(crate) frame_register: bool }
  pub(crate) struct ShippedEntry<'gc> { pub(crate) name: ShippedName, pub(crate) object: ObjectReference<'gc>, pub(crate) positions: Positions }
  impl ShippedWorld<'gc> {
      pub(crate) fn entries(&self) -> &[ShippedEntry<'gc>];
      pub(crate) fn index_of_name(&self, name: &ShippedName) -> Option<usize>;
      pub(crate) fn index_by_object(&self) -> HashMap<usize, usize>; // object key -> entry index, built per encoding
      pub(crate) fn index_by_mixin(&self) -> HashMap<usize, usize>;  // mixin_key of a Class entry's mixin -> entry index
  }
  impl ShippedName { pub(crate) fn describe(&self) -> String; }
  ```
  `describe` answers `Kernel Array`, `Kernel Array class`, `Kernel Kernel level 0`, `Kernel Kernel level 0 class`, `CollectionsFactory instance`. The object key is the same `usize` key `encode::GraphTable` uses for `object_ids` (read how `GraphTable::visit_value` derives it and reuse that function).

- [ ] **Step 1: Measure the boot before the change**

Build release once (`cargo build --release -p ns-cli`, `timeout: 600000`), write `C:\Users\Benjamin\AppData\Local\Temp\claude\D--Workspace\b29387c3-d3b9-40c0-be50-6c777317b431\scratchpad\hello.ns`:
```newspeak
class Hello = () (
  public main: platform args: arguments = (
    platform transcript show: 'hello'.
  )
)
```
and in PowerShell:
```powershell
$runs = 1..50 | ForEach-Object { (Measure-Command { & .\target\release\ns-next.exe run $env:TEMP\claude\D--Workspace\b29387c3-d3b9-40c0-be50-6c777317b431\scratchpad\hello.ns | Out-Null }).TotalMilliseconds }
($runs | Sort-Object)[24]
```
(use the scratchpad path literally). Record the median for Step 7. If `main:` needs another signature, copy one from `goldens/`.

- [ ] **Step 2: Write the failing tests**

In `shipped_world.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn name(module: &str, declaration: &str, kind: ShippedKind) -> ShippedName {
        ShippedName {
            module: Symbol::intern(module),
            declaration: Symbol::intern(declaration),
            kind,
        }
    }

    fn positions_of(world: &ShippedWorld<'_>, shipped: ShippedName) -> Positions {
        let index = world
            .index_of_name(&shipped)
            .unwrap_or_else(|| panic!("the shipped world has no {}", shipped.describe()));
        world.entries()[index].positions
    }

    /// The names the image format relies on all exist, and each may stand
    /// exactly where the rule of `image-format.md` allows.
    #[test]
    fn every_shipped_object_is_named_with_the_positions_the_rule_allows() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let array = positions_of(world, name("Kernel", "Array", ShippedKind::Class));
            assert!(array.value && array.superclass && array.header && array.frame_class);
            assert!(!array.frame_register);
            let array_class = positions_of(world, name("Kernel", "Array", ShippedKind::Metaclass));
            assert!(array_class.value && !array_class.header && array_class.frame_class);
            for private in ["Top", "Activation", "FutureImplementation", "NonBooleanReceiver"] {
                let positions = positions_of(world, name("Kernel", private, ShippedKind::Class));
                assert!(!positions.value && !positions.superclass, "{private} is no value");
                assert!(positions.frame_class);
            }
            for meta in ["Behavior", "Class", "Metaclass"] {
                let positions = positions_of(world, name("Kernel", meta, ShippedKind::Class));
                assert!(positions.value && !positions.superclass, "{meta} is no superclass");
            }
            let list = positions_of(world, name("CollectionsFactory", "List", ShippedKind::Class));
            assert!(list.value && list.superclass && list.header);
            let platform = positions_of(world, name("PlatformFactory", "Platform", ShippedKind::Class));
            assert!(platform.value && platform.header);
            for root in ["Kernel", "PlatformFactory", "CollectionsFactory"] {
                let positions = positions_of(world, name(root, root, ShippedKind::Class));
                assert!(!positions.value && !positions.header && positions.frame_class);
                let level = positions_of(world, name(root, root, ShippedKind::Level(0)));
                assert!(!level.value && !level.header && level.frame_class);
                positions_of(world, name(root, root, ShippedKind::LevelMetaclass(0)));
                let instance = positions_of(world, name(root, root, ShippedKind::ModuleInstance));
                assert_eq!(
                    instance,
                    Positions { frame_register: true, ..Positions::default() }
                );
            }
        });
    }

    /// Each name answers the object the boot built: the builtin `Array`,
    /// its metaclass, and the class the collections facet hands out.
    #[test]
    fn a_shipped_name_answers_the_boots_own_object() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let object_of = |shipped| world.entries()[world.index_of_name(&shipped).expect("named")].object;
            let array = mutator.builtin_class(crate::types::BuiltinClass::Array);
            assert!(object_of(name("Kernel", "Array", ShippedKind::Class)).same(array));
            let metaclass = mutator.heap().class_metaclass(array).expect("Array has a metaclass");
            assert!(object_of(name("Kernel", "Array", ShippedKind::Metaclass)).same(metaclass));
        });
    }

    /// A boot runs every shipped class's superclass clause and every shipped
    /// module's initializer, and draws no identity hash while it does: the
    /// first hash a program draws is 1, under `run` and `load` alike
    /// (`goldens/library/object/identity_hash.ns`).
    #[test]
    fn a_boot_draws_no_identity_hash() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.new_string(String::new());
            assert_eq!(mutator.heap_mut().identity_hash(object), 1);
        });
    }
}
```

Adapt `identity_hash`/`heap_mut`/`new_string` to the names `heap.rs` and `Mutator` really expose (the drawing accessor is the one `Object>>hash`'s leaf calls; `object_identity_hash` only reads). Add a positions test for `PlatformFactory Transcript`, `PlatformFactory Stopwatch` and `CollectionsFactory Map` (value true, because `Platform>>Transcript`, `Time>>Stopwatch` and `Collections>>Map` are public class accessors).

- [ ] **Step 3: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime shipped_world`
Expected: compile errors (`ShippedKind` and the rest do not exist).

- [ ] **Step 4: Build every nested class from Rust**

A private nested class cannot be built by a host send (access control refuses it, `dispatch.rs`, `Rule::verdict`), so add to `Interpreter` in `mod.rs`, beside `send`:

```rust
/// Builds the class `nested` declares in the class of `enclosing`, as
/// its accessor would, whatever the accessor's access: the boot builds
/// every class a shipped root declares this way
/// (`interpreter::shipped_world`). Answers the class object, or the
/// error its superclass clause raised.
pub(crate) fn build_nested_class(
    &mut self,
    enclosing: &Root,
    nested: &Rc<Mixin>,
) -> HostResult {
    let depth = self.world.activation_depth();
    let start = self.enter(|mutator| {
        let receiver = mutator.fetch(enclosing);
        let declaring_class = match receiver.as_object() {
            Some(object) => mutator.heap().object_class(object),
            None => unreachable!("internal: a shipped module instance is an object"),
        };
        let decided = nested::nested_class(mutator, receiver, nested, declaring_class);
        mutator.start(decided, &[])
    });
    self.run_from_host(start, depth)
}
```

The declaring class is the module root class itself: every shipped class is declared directly in its root, and an instance's class is its root class (the `Value` level lies below it). Check that against `nested::nested_class`'s doc on `declaring_class` before relying on it.

- [ ] **Step 5: Build and record the world**

In `shipped_world.rs` add the types of "Interfaces", an `entries: Vec<ShippedEntry<'gc>>` and `by_name: HashMap<ShippedName, usize>` beside `module_instances`, trace every entry's `object`, and:

```rust
impl crate::interpreter::Interpreter {
    /// The last phase of the boot: every class a shipped root declares is
    /// built, in declaration order, and every shipped object is recorded
    /// under its name with the positions it may stand in
    /// (`docs/design/image-format.md`, "Planned: shipped names in the image").
    pub(crate) fn complete_shipped_world(&mut self) { ... }
}
```

What it does, in this order:
1. For every `(module, instance)` in `module_instances()` (stash the instance as a `Root` first): take the root class's mixin (`class_application(object_class(instance)).mixin`), and for every `nested` in `mixin.nested`, `build_nested_class(&instance, nested)`, panicking through `booted`-style handling (`"internal: building {module} {name} failed at bootstrap: {message}"`). The kernel's classes are already built by `fill_builtin_classes`; the call answers the cached class at once.
2. In one `enter`, for every module instance, record:
   - `(module, module, ModuleInstance)` → the instance; positions: `frame_register` only.
   - `(module, module, Class)` → the root class; `(module, module, Metaclass)` → its metaclass; positions: `frame_class` only.
   - Each level the root's own `<:` clauses add, lowest first, by clause index `n` (`encode::root_levels`, or the same walk: down the superclass chain while `class_applied_origin(level).declaration` is the root's mixin): `Level(n)` and `LevelMetaclass(n)`; positions: `frame_class` only.
   - For every `nested` of the root's mixin: the cached class object (read the root instance's hidden slot, as `kernel::hidden_slot_of` and `platform_paths::class_accessor_target` do), recorded as `(module, nested.name, Class)` and its metaclass as `Metaclass`, and the levels its own `<:` clauses add as `Level(n)`/`LevelMetaclass(n)` (none exist today; sub-project 5 adds them). Positions of a nested class: `header` and `frame_class` always; `value` when `nested.access` is public or a public method of any class of the same module (the root's mixin and every nested mixin, instance and class side) has `class_accessor` naming it (`MixinMethod::class_accessor`, `ClassAccessor::class_name == nested.name`); `superclass` when `value` and `class_builtin` is not `Behavior`, `Class` or `Metaclass`. Its metaclass: `value` and `frame_class` when the class has `value`, else `frame_class` only; never `header`. A level and a level's metaclass: `frame_class` only.
3. Fill `by_name`; a name recorded twice panics (`"internal: the shipped name {} is recorded twice"`).

`Activation` and `FutureImplementation` keep `header`: the decoder still allows them only on a frame record and a future's (`decode_object_header`'s flags), unchanged.

Call `complete_shipped_world` at the end of `install_shipped_modules`, before `newPlatform` is sent (so the platform's class is built and named), and add `index_of_name`, `index_by_object`, `index_by_mixin` and `describe`.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-runtime shipped_world platform`
Expected: PASS. If `a_boot_draws_no_identity_hash` fails, find which clause or initializer draws the hash (set a breakpoint-style `debug_assert!` in the drawing accessor temporarily) and report it -- do not change the golden.

- [ ] **Step 7: Measure after, and record**

Repeat Step 1's measurement on a fresh release build. Append both medians, the date and the machine (Windows 11, this workstation) to `docs/design/execution.md`, "What the collections module cost", rewriting that section to say that every shipped module is now built at boot, why (the image's shipped names), and the cost measured; if the difference exceeds 2 ms, stop and report to the controller before committing.

- [ ] **Step 8: Gates and commit**

Run the per-commit gates. Commit: `feat: build the whole shipped world at boot and record what an image may name`.

---

### Task 3: The shipped world's fingerprint in the header

**Files:**
- Create: `crates/ns-runtime/src/image/fingerprint.rs`
- Modify: `crates/ns-runtime/src/image/mod.rs` (`pub mod fingerprint;`), `encode.rs` (`write_header`), `decode.rs` (`Header`, `read_header`, a `DecodeError` variant), `inspect.rs` (print it)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)

**Interfaces:**
- Produces: `pub fn shipped_world_fingerprint() -> u64`; `Header { has_source, start, fingerprint: u64 }`; `DecodeError::ShippedWorldMismatch { found: u64, expected: u64 }`; the header is `MAGIC, FORMAT_VERSION (u32), flags (u8), fingerprint (u64)`.

- [ ] **Step 1: Write the failing tests**

In `tests/image.rs`, beside the version-mismatch test (around line 4678):

```rust
/// An image another binary wrote is refused before anything past the
/// header is read: its shipped names would resolve against another world.
#[test]
fn an_image_of_another_shipped_world_is_refused() {
    let mut bytes = snapshot_of(EMPTY);
    let fingerprint_at = encode::MAGIC.len() + 4 + 1;
    bytes[fingerprint_at] ^= 1;
    let error = load_error(&bytes);
    assert!(
        error.contains("another shipped world"),
        "the refusal names the mismatch: {error}"
    );
}

/// The fingerprint is a function of the binary alone.
#[test]
fn the_shipped_world_fingerprint_is_stable_within_a_binary() {
    assert_eq!(
        image::fingerprint::shipped_world_fingerprint(),
        image::fingerprint::shipped_world_fingerprint()
    );
    let bytes = snapshot_of(EMPTY);
    let fingerprint_at = encode::MAGIC.len() + 4 + 1;
    let written = u64::from_le_bytes(bytes[fingerprint_at..fingerprint_at + 8].try_into().expect("eight bytes"));
    assert_eq!(written, image::fingerprint::shipped_world_fingerprint());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --test image shipped_world`
Expected: compile error (`image::fingerprint` does not exist).

- [ ] **Step 3: Implement**

`fingerprint.rs`:

```rust
//! The shipped world's fingerprint, which an image's header carries
//! (`docs/design/image-format.md`, "Planned: shipped names in the image"):
//! an image names the shipped world by declaration, so it is loaded only
//! by a binary whose shipped world is the one that wrote it.

use std::sync::OnceLock;

use crate::builtins::primitive::MIRROR;
use crate::interpreter::core_library::SHIPPED_MODULES;
use crate::interpreter::kernel::KERNEL_SOURCE;
use crate::types::BuiltinClass;

/// FNV-1a over the shipped modules' sources, the builtin classes' names
/// and the mirror's selectors, each preceded by its length so that no two
/// sequences of parts hash alike by running together. The lowering is not
/// in it: `load` always compiles the shipped code with the optimized
/// lowering, and an application carries its own code.
pub fn shipped_world_fingerprint() -> u64 {
    static FINGERPRINT: OnceLock<u64> = OnceLock::new();
    *FINGERPRINT.get_or_init(|| {
        let mut hasher = Fnv::new();
        hasher.part(KERNEL_SOURCE.as_bytes());
        for shipped in SHIPPED_MODULES {
            hasher.part(shipped.name.as_bytes());
            hasher.part(shipped.source.as_bytes());
        }
        for kind in BuiltinClass::ALL {
            hasher.part(format!("{kind:?}").as_bytes());
        }
        for entry in MIRROR {
            hasher.part(entry.selector.as_bytes());
            hasher.part(entry.reported.as_bytes());
        }
        hasher.finish()
    })
}

struct Fnv {
    state: u64,
}

impl Fnv {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Fnv { state: Self::OFFSET_BASIS }
    }

    fn part(&mut self, bytes: &[u8]) {
        self.bytes(&(bytes.len() as u64).to_le_bytes());
        self.bytes(bytes);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.state
    }
}
```

Widen `KERNEL_SOURCE`, `SHIPPED_MODULES` and `MIRROR` visibility to `pub(crate)` where needed. `write_header` writes `writer.write_u64(shipped_world_fingerprint())` after the flags (add `write_u64`/`read_u64` to `Writer`/`Reader` if absent, little-endian). `read_header` reads it after the flags and refuses a mismatch with `ShippedWorldMismatch`, displayed as `"the image was written by another shipped world (fingerprint {found:016x}, this binary's {expected:016x})"`. `inspect`'s `Report` gains `shipped_world: u64`, rendered after the format version as `shipped world {:016x}`. Update `image/mod.rs`'s module doc and the `Layout` comment in `encode.rs`.

- [ ] **Step 4: Repair what the header move broke**

Every test that patches bytes at a fixed header offset (`bytes[MAGIC.len()+4] |= FLAG_START` around `tests/image.rs:13040`, the version tests around :4678, `crates/ns-cli/tests/build_load.rs`'s corruption sweep, which starts at offset 8 -- make it start after the fingerprint so it still sweeps sections rather than refusing everything at the header) is adjusted. Regenerate the wire fixture: `cargo nextest run -p ns-runtime --test image regenerate_the_golden_fixture --run-ignored only`, then confirm `the_wire_format_matches_the_golden_fixture` passes.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p ns-runtime --test image` and `cargo nextest run -p ns-cli --test build_load`
Expected: PASS.

- [ ] **Step 6: Gates and commit**

Run the per-commit gates. Commit: `feat: refuse an image another shipped world wrote`.

---

### Task 4: The shipped-name section

The heart of the batch: every reference to a shipped object becomes an index into a section of names, the platform becomes a record, and the platform paths go. Module roots keep `CLASS_MODULE`, `CLASS_MODULE_LEVEL`, their hash tags and the root section until Task 5 (so `encode::builtin_class_tag` survives for the root section until then).

**Files:**
- Create: `crates/ns-runtime/src/image/shipped_names.rs` (the section's encoding and decoding)
- Delete: `crates/ns-runtime/src/image/platform_paths.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`, `inspect.rs`, `mod.rs`
- Modify: `crates/ns-runtime/src/interpreter/mirror.rs` (its test's `snapshot` call), `crates/ns-runtime/tests/platform.rs` (the `is_path_step` test goes)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`, `crates/ns-cli/tests/build_load.rs`, `crates/ns-analyzer/tests/inspect.rs`, `tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: Task 2's `ShippedWorld`, `ShippedName`, `ShippedKind`, `Positions`, `index_of_name`, `index_by_object`, `index_by_mixin`, `describe`; Task 3's header.
- Produces:
  - Section layout, between the module section and the graph: `u32 count`, then per entry `u32 module symbol, u32 declaration symbol, u8 kind` (0 module instance, 1 class, 2 metaclass, 3 level, 4 level's metaclass) and, for kinds 3 and 4, `u32 clause index`. `MAX_SHIPPED_NAMES: usize = 65_536`; `MIN_SHIPPED_NAME_BYTES = 9`.
  - `pub(crate) fn encode_section(writer: &mut Writer, world: &ShippedWorld<'_>, used: &[usize], symbols: &mut symbols::Builder)`.
  - `pub(crate) fn decode_section(reader: &mut Reader, symbols: &symbols::Table) -> Result<Vec<ShippedName>, DecodeError>` (structure only, no world: `inspect` uses it too) and `pub(crate) fn resolve(names: &[ShippedName], world: &ShippedWorld<'gc>, started: bool) -> Result<Vec<ResolvedName<'gc>>, DecodeError>` with `pub struct ResolvedName<'gc> { pub object: ObjectReference<'gc>, pub positions: Positions, pub name: ShippedName }`.
  - `DecodeContext` loses `platform` and `platform_paths` and gains `shipped: &'a [ResolvedName<'gc>]`.
  - New `DecodeError`s: `TooManyShippedNames { count }`, `UnknownShippedName(String)`, `DuplicateShippedName(String)`, `BadShippedName(u32)` (an index past the section), `ShippedNameNotAllowed { name: String, position: &'static str }`, `ShippedNameInStartedImage(String)`, `ShippedInstanceFieldCount { expected, found }` (replacing `PlatformInstanceFieldCount`); the platform path variants go.
  - `image::snapshot(mutator, modules, roots)` loses its platform argument; `image::package` is unchanged in signature.

- [ ] **Step 1: Tags**

`opcodes.rs`: add `CLASS_SHIPPED: u8 = 8`, `MIXIN_SHIPPED: u8 = 2`, `VALUE_SHIPPED_MODULE: u8 = 11`, `HASHED_SHIPPED: u8 = 7`, each documented (what follows it: a `u32` index into the shipped-name section; which kinds and positions it allows -- the table of the spec). Remove `CLASS_BUILTIN` (0), `CLASS_METACLASS` (4), `CLASS_PLATFORM_PATH` (5), `MIXIN_BUILTIN` (1), `VALUE_PLATFORM` (7), `VALUE_PLATFORM_PATH` (8), `HASHED_BUILTIN_CLASS` (0), `HASHED_PLATFORM` (2), `HASHED_METACLASS` (3), `HASHED_PLATFORM_PATH` (4), and retire them: `RETIRED_CLASS_TAGS = [0, 1, 3, 4, 5]`, `RETIRED_MIXIN_TAGS = [1]` (new), `RETIRED_VALUE_TAGS = [7, 8]` (new), `RETIRED_HASHED_TAGS = [0, 1, 2, 3, 4]`, each with a doc comment saying what the number was and that a shipped name replaced it on this date. Update every "Next free number". In `tests/opcode_tags.rs`: `graph_section_tags_are_fixed` pins the new and live tags and the retired arrays; `LIVE_CLASS_TAGS = [CLASS_RECORD, CLASS_MODULE, CLASS_MODULE_LEVEL, CLASS_SHIPPED]`, `LIVE_HASHED_TAGS = [HASHED_MODULE_CLASS, HASHED_MODULE_LEVEL, HASHED_SHIPPED]`; add live-vs-retired disjointness checks for the two new retired arrays to `retired_class_and_hashed_tags_are_never_reused`; `no_two_graph_tags_in_a_family_share_a_number` takes the new value and mixin tags. `builtin_class_tags_are_fixed` stays until Task 5.

- [ ] **Step 2: The section**

`shipped_names.rs` with `encode_section`, `decode_section`, `resolve` as in "Interfaces". `decode_section`: count through `bounded_count(count, MIN_SHIPPED_NAME_BYTES, reader)` and `> MAX_SHIPPED_NAMES` → `TooManyShippedNames`; a symbol id through the symbol table's checked lookup; an unknown kind byte → `InvalidTag { what: "shipped name kind", tag }`; a clause index `>= CLAUSE_INDEX_LIMIT` → `ClauseIndexTooLarge`-style refusal (reuse the existing variant if it fits, otherwise `UnknownShippedName`). `resolve`: each name through `world.index_of_name` (`UnknownShippedName(name.describe())`), a repeat → `DuplicateShippedName`, and when `started` a module other than `Kernel` → `ShippedNameInStartedImage(name.describe())`. Unit tests in the file: a round trip of three names; each refusal above on hand-written bytes.

- [ ] **Step 3: The encoder**

In `encode.rs`, with the spec's table in front of you:
- `GraphTable::build` loses `platform` and `platform_paths` and takes `world: &ShippedWorld<'gc>` and `shipped_by_object: &HashMap<usize, usize>` (from `index_by_object`). `visit_value`'s order becomes: `seen_through`, skip non-objects, skip `LargeInteger`/float/fraction; **the mirror object** (`is_vm_mirror`, or its class is builtin `VmMirror`) → refusal "image encoding refuses the VM mirror: …" (keep today's message); **a shipped name** (`shipped_by_object` has it) → record its use (first-use order, `used_names: Vec<usize>` and `used_index: HashMap<usize, u32>`) and stop; **a shipped class object no name covers** (`heap.is_class(object) && heap.class_is_shipped(object)` and not a program class record -- i.e. a class whose mixin is shipped and whose enclosing object is a shipped module instance) → refusal "image encoding refuses the class object {name}: it is compiled from shipped source, and no shipped name names it"; otherwise assign an id and push children as today. A program class applying a shipped mixin (a level of `Object <: Value`) is a program class record, not a shipped name: it is recognised by not being in `shipped_by_object`.
- Delete `is_class_tag`, `unnameable_class`, `is_prebuilt_class`, `shipped_without_path`, `PrebuiltClass`, `prebuilt_class`, `write_prebuilt_class` for the builtin and path cases (module roots keep a `CLASS_MODULE`/`CLASS_MODULE_LEVEL` branch until Task 5), `UsedPlatformPaths`, the `MAX_PLATFORM_PATHS` check.
- A class reference: a shipped name → `CLASS_SHIPPED` + `u32` used index; otherwise module root/level (until Task 5) or `CLASS_RECORD` + id. Every class reference states its position, so `encode_class_tag` gains `position: NamePosition` (`enum NamePosition { Value, Superclass, Header, FrameClass, FrameRegister }` in `shipped_names.rs`, with `fn allows(self, positions: Positions) -> bool` and `fn describe(self) -> &'static str`: `"a value"`, `"a superclass or an applied mixin"`, `"a record's class"`, `"a frame's class"`, `"a frame's register"`). A name the position does not allow is refused: `"image encoding refuses {name} as {position}: no program may name it there, and the loader refuses an image that does"`. Positions: a value's `VALUE_CLASS_TAG` → `Value`; a class record's superclass → `Superclass`; a class record's declaring class and a frame's `current_class` and a frame kind's class (`INITIALIZER`, `SUPERCLASS_CLAUSE`, `SUPERCLASS_MESSAGE`, `MIXIN_APPLICATION_MESSAGE`) → `FrameClass`; an object's header → `Header`. `OBJECT_METACLASS`'s class reference names the metaclass's own class, which is always a program class record; a shipped name there is refused, checked as `Header` (a shipped metaclass is never a record).
- A value naming a module instance → `VALUE_SHIPPED_MODULE` + index, allowed only when `encode_value` is called for a frame's receiver, arguments or temporaries (pass `NamePosition::FrameRegister` there, `NamePosition::Value` everywhere else); elsewhere refused: `"image encoding refuses the module instance {name} outside a frame: it holds the core library's state"`.
- `VALUE_PLATFORM` goes: the platform is an ordinary instance record (its header `CLASS_SHIPPED` naming `PlatformFactory Platform`, its fields its slots as they stand).
- A class record applying a shipped mixin (today's `MIXIN_BUILTIN`, plus any shipped mixin): `MIXIN_SHIPPED` + the used index of the `Class` entry whose mixin it is (`index_by_mixin`), which must allow `Superclass` (refused otherwise, as above), then optional superclass, then the applied clause (row, index) -- the layout `MIXIN_BUILTIN` had, the enclosing object and declaring class still the named class's. A shipped mixin no `Class` entry carries → refusal `"…applying the shipped mixin {name}, which no shipped name names"`.
- Hashes: `encode_tag_named_hashes(mutator, writer, modules, world, used)` writes, after the module-root entries, one `HASHED_SHIPPED` + index entry for every world entry whose `object_identity_hash` is non-zero, adding it to the used names if the graph did not use it (so the section lists it). The platform's hash travels in its record's header like any record's. The started-image rule then covers hashes too.
- `image::package` refuses a graph using a non-kernel name: after the graph is encoded, `if started && used names any module but Kernel` → `"image encoding refuses {name} in a started application: nothing before main:args: reaches it"`.
- `write_image` writes `header | symbols | code | mixins | modules | shipped names | graph | tag-named hashes`: the section is encoded after the graph (which decides the used names) and written before it.
- `snapshot` loses its `platform` parameter; update `mirror.rs`'s test, `crates/ns-cli/tests/build_load.rs:484` and every `tests/image.rs` call.

- [ ] **Step 4: The decoder and `load`**

- `load`: after `decode_modules`/`build_module_classes`, `shipped_names::decode_section`, then `resolve(&names, &mutator.shipped_world, header.start)`; `Described` keeps the names, not handles, so delete `resolve_platform_paths`, the second phase, `hash_before_paths` and its `debug_assert`; the `tenure_now()` stays where it is, between the description and the graph, and `decode_graph_phase` resolves the names again inside its entry (a lookup, no allocation of objects) and builds `DecodeContext { shipped: &resolved, … }`. Rewrite `load`'s doc comment: two phases, nothing runs between them.
- `read_class_tag_outside_the_graph`: `CLASS_SHIPPED` → `read_shipped(reader, context, position)` which reads the `u32`, refuses an index past the section (`BadShippedName`), refuses a module instance (`NotAClassReference`), refuses a position the entry does not allow (`ShippedNameNotAllowed`), and answers the object. Delete `CLASS_BUILTIN`, `CLASS_METACLASS`, `CLASS_PLATFORM_PATH` arms, `read_prebuilt_class`, `read_platform_path(_class)`, `read_referenced_builtin_class`, `referenced_builtin_class`, `read_program_builtin_class` where only those arms use them (the root section still reads builtin tags until Task 5; keep `read_builtin_class` and `builtin_class_from_tag` for it). Thread the position through every caller exactly as the encoder assigns it (Step 3).
- `decode_object_header`: a `CLASS_SHIPPED` header checks `Header`; the `activation`/`future` flags come from `class_builtin(object) == Some(Activation | FutureImplementation)`; `platform_path` becomes `shipped_non_builtin: class_builtin(class).is_none()`, and the slot-count check (today only for path-named classes, `decode.rs` around 3911-3933) applies to every such header (`ShippedInstanceFieldCount`).
- `decode_value`: `VALUE_SHIPPED_MODULE` → index, must be a `ModuleInstance` entry and the position `FrameRegister` (`decode_frame` passes it for receiver, arguments and temporaries); `VALUE_PLATFORM`/`VALUE_PLATFORM_PATH` arms go (their numbers now meet `InvalidTag`). The sizing pass (`Sizing::value`, `class_tag_after`, `header`, `class_record`) takes the same tags and checks.
- `decode_class_record`: `MIXIN_SHIPPED` → `decode_shipped_level` (today's `decode_builtin_level` with the class taken from the name, which must be a `Class` entry allowing `Superclass`); `decode_applied_clause`'s `named_builtin` is `class_builtin(named)` (`None` for a non-builtin shipped class).
- `decode_tag_named_hashes`: `HASHED_SHIPPED` → index → object; the builtin, platform, metaclass and path arms go.
- Search `decode.rs` for every remaining `platform` and `builtin_class_tag` use and justify or remove each.

- [ ] **Step 5: `inspect`**

`inspect` reads the section with `decode_section` (no world) and prints it as section `shipped names` with detail `{n} names`; a `CLASS_SHIPPED` reference prints the entry's `describe()` (`Kernel Array`, `Kernel Array class`, `Kernel Kernel level 0`), an index past the section `<shipped name {index}: out of range>`; `VALUE_SHIPPED_MODULE` prints `CollectionsFactory instance`; `MIXIN_SHIPPED` prints `class mixin shipped {describe}, superclass {…}, applied by clause I of row R`; a hashed entry prints `{describe} hash {n}`. `Report.platform_paths` becomes `shipped_names: Vec<String>`; `render_full` lists them under `shipped names (n)`.

- [ ] **Step 6: Rewrite the tests**

In `tests/image.rs` (the map below is from the investigation; line numbers drift):
- Graph fixtures (`GraphFixture`, `round_trip_graph`, `closure_world`, `decode_tag_named`, `decode_tag_named_with_the_transcript`): build `DecodeContext { shipped, … }` from a fixed list resolved against the fixture's own interpreter:
  ```rust
  /// The shipped names every hand-written graph in this file may use, by
  /// their index in the section a fixture decodes against.
  const FIXTURE_NAMES: &[(&str, &str, u8)] = &[
      ("Kernel", "Object", 1),               // OBJECT_NAME = 0
      ("Kernel", "Array", 1),                // ARRAY_NAME = 1
      ("Kernel", "Activation", 1),           // ACTIVATION_NAME = 2
      ("Kernel", "Metaclass", 1),            // METACLASS_NAME = 3
      ("Kernel", "String", 1),               // STRING_NAME = 4
      ("Kernel", "Symbol", 1),               // SYMBOL_NAME = 5
      ("Kernel", "Integer", 1),              // INTEGER_NAME = 6
      ("Kernel", "Fraction", 1),             // FRACTION_NAME = 7
      ("Kernel", "FutureImplementation", 1), // FUTURE_IMPLEMENTATION_NAME = 8
      ("Kernel", "Value", 1),                // VALUE_NAME = 9
      ("Kernel", "Top", 1),                  // TOP_NAME = 10
      ("Kernel", "Integer", 2),              // INTEGER_CLASS_NAME = 11
      ("Kernel", "Kernel", 0),               // KERNEL_INSTANCE_NAME = 12
  );
  ```
  with a `const` per index, a helper `fn shipped(writer: &mut Writer, index: u32) { writer.write_u8(CLASS_SHIPPED); writer.write_u32(index); }`, and every `CLASS_BUILTIN` + `builtin_class_tag(X)` pair in the helpers (`frame_header`, `one_array_object`, `metaclass_record`, `closure_object_with_copied_values`, `closure_over_frame`, `instance_chain_graph`, `activation_header`, `future_record`, `fraction_record`, `builtin_mixin_record` → `MIXIN_SHIPPED`) rewritten to it. Extend the list rather than adding a second one.
- Splicing helpers: `with_platform_path_section` becomes `with_shipped_name_section` (locates the `shipped names` section through `inspect`); `image_with_graph_section`'s assertion about the last 12 bytes still holds if the section precedes the graph -- check it.
- Delete the platform path tests (the section around :12156-12952) and replace them with shipped-name tests, at least:
  - `a_platform_member_round_trips_as_a_record` -- a snapshot holding `platform transcript`, `platform collections`, `platform time` and `platform` itself loads; the decoded platform `=` the loading platform (send `=` and expect `true`); the decoded facets answer (`collections List new` works).
  - `a_class_the_platform_hands_out_is_named_by_its_declaration` -- `report.shipped_names` lists `PlatformFactory Transcript`, `CollectionsFactory List`, `PlatformFactory Stopwatch`; after load each decoded class is `same` as the loading boot's (fetch through `send_to` and `same_object`).
  - Review Focus 2: a snapshot holding `Array`, `Integer class` and `platform collections List` in slots loads, and each decoded class is `same` as the loading boot's own (`same_object`). A level is never a value, so `shipped_names.rs`'s unit tests check instead that `resolve` answers, for `Kernel Kernel level 0`, the very object `ShippedWorld` recorded.
  - Review Focus 3, one hand-written graph per position against `GraphFixture`: `TOP_NAME` as a value; `ACTIVATION_NAME` as a value; a module root as a header (add `("Kernel", "Kernel", 1)` to the list); `KERNEL_INSTANCE_NAME` as a slot value; each → the `DecodeError` named in "Interfaces", asserted by variant.
  - Review Focus 3, started image: `package` of an application whose graph uses `PlatformFactory Platform` is refused by the encoder (craft it by patching a packaged image's section entry from `Kernel` to `PlatformFactory` with `with_shipped_name_section`, then `load` → `ShippedNameInStartedImage`).
  - Review Focus 4: each malformed section from Step 2 through `with_shipped_name_section` and `load_error`, plus `every_truncation_of_an_image_is_reported_rather_than_panicking` staying green.
  - Review Focus 5: a record of `PlatformFactory Stopwatch` with one field too many → `ShippedInstanceFieldCount`.
- `a_shipped_object_no_platform_path_names_is_refused` becomes `a_module_instance_outside_a_frame_is_refused`: a snapshot of a graph holding the kernel instance in a slot (reach it as the existing test does) is refused with the new message; same for the `CollectionsFactory` instance.
- Tag-named hash tests: `the_hashes_of_objects_named_by_tag_survive_a_snapshot` expects `["<module 0> hash N", "Kernel Integer hash N"]` in the encoder's order (module entries first, then shipped names in world order); the platform's hash now travels in its record; the retired hash tags are refused (`an_unknown_tag_named_hash_tag_is_refused` loops over `HASHED_SHIPPED + 1` and `RETIRED_HASHED_TAGS`).
- `an_invalid_class_tag_is_a_decode_error_not_a_panic` loops over the new `RETIRED_CLASS_TAGS`; add the same for `RETIRED_VALUE_TAGS` and `RETIRED_MIXIN_TAGS`.
- Exact-message tests (`a_graph_reaching_an_unnameable_class_is_refused`, `a_graph_reaching_the_vm_mirror_is_refused_as_the_mirror`) take the new messages.
- `crates/ns-runtime/tests/platform.rs`: `the_platforms_class_accessors_reach_the_private_classes` loses its `is_path_step` assertion and asserts instead that `PlatformFactory Transcript` etc. allow `value` (through `ShippedWorld` if the test can reach it, else drop the half).
- `crates/ns-analyzer/tests/inspect.rs`: the section list `platform paths` → `shipped names`.
- Regenerate `tests/fixtures/golden-image.bin` (`--run-ignored only regenerate_the_golden_fixture`).

- [ ] **Step 7: Delete `platform_paths.rs`**

Remove the module and `pub mod platform_paths;`; `rg platform_path --type rust` must answer nothing outside comments that Task 6 rewrites; fix the comments in code now (`core_library.rs`, `encode.rs`, `decode.rs`, `opcodes.rs`, `modules.rs`).

- [ ] **Step 8: Run the tests**

Run: `cargo nextest run --workspace` (`timeout: 600000`)
Expected: PASS, every golden under both lowerings and the `build_load` round trips unchanged.

- [ ] **Step 9: Gates and commit**

Run the per-commit gates. Commit: `feat: name the shipped world in an image by declaration`.

---

### Task 5: The program's module roots as records

**Files:**
- Modify: `crates/ns-runtime/src/image/modules.rs`, `encode.rs`, `decode.rs`, `inspect.rs`, `mod.rs`, `opcodes.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`, `tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: Task 4's `CLASS_SHIPPED`, `MIXIN_SHIPPED`, `DecodeContext.shipped`.
- Produces: the module section per module is `origin, source?, u32 object id of the root class record`; `LoadedModule.class` is that record, rooted; `ModuleClasses`, `ModuleClass`, `build_module_classes`, `decode_root_section`, `build_root_class`, `encode_root_class`, `RootSection`, `encode::builtin_class_tag`, `decode::builtin_class_from_tag`, `read_builtin_class`, `CLASS_MODULE` (6), `CLASS_MODULE_LEVEL` (7), `HASHED_MODULE_CLASS` (5), `HASHED_MODULE_LEVEL` (6) and `RETIRED_BUILTIN_CLASS_TAGS` are gone.

- [ ] **Step 1: Write the failing test**

In `tests/image.rs`, in the module section's group:

```rust
/// A module's root class is a record of the graph like any class: the
/// module section names it by object id, and a load answers that record.
#[test]
fn a_modules_root_class_is_a_record_the_module_section_names() {
    let (mut interpreter, class) = load(EMPTY);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, Some(EMPTY), &origin(), class, &[]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report.graph_objects.iter().any(|object| object.contains("class mixin row 0")),
        "the root class is a class record: {:?}",
        report.graph_objects
    );
    let loaded = image::load(&bytes).expect("load");
    assert_eq!(loaded.modules.len(), 1);
}
```

(adapt `origin()`/`snapshot_one` to the helpers' real names and Task 4's signature).

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-runtime --test image a_modules_root_class_is_a_record`
Expected: FAIL (no class record for the root today).

- [ ] **Step 3: Implement**

- Encoder: `GraphTable::build` takes the module root classes as additional traversal roots (after the image's roots, so ids of existing graphs shift only by the roots' own records), and module roots and their levels are ordinary class records: a root's record applies `MIXIN_ROW` (its tree's row 0), its superclass `CLASS_SHIPPED(Kernel Object)` or its lowest level; a level applying `Value` is `MIXIN_SHIPPED(Kernel Value)` with the applied clause `(row 0 of the tree, clause index)`. `encode_modules` writes the root's object id instead of `encode_root_class`. The module-root branch of `encode_class_tag` and the `HASHED_MODULE_*` entries go (a root's hash is in its record).
- Decoder: `decode_modules` reads `u32` ids; after `decode_graph_section` each id must name a class record whose mixin is `mixins.module_root(index)` (`Rc::ptr_eq`) and which is no level, else `DecodeError::ModuleRootNotItsRecord { module }`; its superclass chain must satisfy the root rule the encoder enforces (`root_clause_defect`): superclass `Kernel Object`, or one level applying `Kernel Value` over `Kernel Object` -- else `RootRule { module }`. `method_declaration`'s fallback for a frame with no current class uses module 0's root record. `LoadedModule.class` stashes the record.
- `inspect`: module lines print `root object {id}`; the `<module …>` printers go.
- `opcodes.rs`: retire `CLASS_MODULE`/`CLASS_MODULE_LEVEL` into `RETIRED_CLASS_TAGS = [0, 1, 3, 4, 5, 6, 7]` and `HASHED_MODULE_CLASS`/`HASHED_MODULE_LEVEL` into `RETIRED_HASHED_TAGS = [0, 1, 2, 3, 4, 5, 6]`; delete `RETIRED_BUILTIN_CLASS_TAGS` and the builtin class tag numbering with a doc comment: the builtin class tags are retired as a whole, since no part of an image names a builtin by position any more. `opcode_tags.rs`: delete `builtin_class_tags_are_fixed`, update the live arrays (`LIVE_CLASS_TAGS = [CLASS_RECORD, CLASS_SHIPPED]`, `LIVE_HASHED_TAGS = [HASHED_SHIPPED]`).
- `crates/ns-runtime/src/types.rs:168` and any other comment naming `builtin_class_tag` is rewritten.

- [ ] **Step 4: Rewrite the tests**

The module-section tests (around `tests/image.rs:7184-7663`), `root_section_bytes` and its users (:2163, :2192, :7480, :11679, :11727), the `CLASS_MODULE`/`CLASS_MODULE_LEVEL` crafted tests (:7356, :11550, :11574, :11591), the module-level hash tests (:6192, :6256, :7618, :9913) and the inspect texts `<module 0>`, `<module 0 level 0>` become their record equivalents: a crafted module id past the graph → refused; an id naming an array → `ModuleRootNotItsRecord`; a root record with superclass `Kernel Array` → `RootRule`; a module root's and its level's hash survive (they are record headers now). Regenerate the wire fixture.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run --workspace` (`timeout: 600000`)
Expected: PASS.

- [ ] **Step 6: Gates and commit**

Run the per-commit gates. Commit: `refactor: write the program's module roots as class records`.

---

### Task 6: The documentation

**Files:**
- Modify: `docs/design/image-format.md`, `docs/design/core-library.md`, `docs/design/execution.md`, `docs/design/memory.md`, `docs/design/object-model.md`, `docs/design/README.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: `image-format.md`**

"Planned: shipped names in the image" becomes the built description, moved into the body: "What is *not* serialized" (shipped objects by name, the platform as a record), "Layout" and "Sections" (the header's fingerprint, the shipped-name section instead of the platform paths), the section "Platform paths" replaced by "Shipped names", "Verification" (the name checks, the started-image rule), "Packaging", "The image under generations" (one tenure, no sends before the graph), the Status block and the Code list (`shipped_names.rs`, `fingerprint.rs`, no `platform_paths.rs`). The Decisions entry loses "not yet built". Every statement about builtin class tags, `CLASS_METACLASS`, `CLASS_MODULE`, `VALUE_PLATFORM` or platform paths is rewritten or deleted (`rg -n 'platform path|CLASS_BUILTIN|CLASS_MODULE|VALUE_PLATFORM|builtin class tag|HASHED_' docs`).

- [ ] **Step 2: `core-library.md`**

"Platform paths in the image (plan 1)" becomes "Shipped names in the image", stating the nameability rule for an author of a new shipped module; "The module table and the boot" says every module is built at boot and `moduleInstanceNamed:` answers the canonical instance; lock (3) and (4) in "The VM mirror is the whole Rust boundary" as the spec rewrites them; sub-project 5's "What the plan must handle" says the naming is built.

- [ ] **Step 3: `CLAUDE.md`**

- "An image is untrusted input": replace "the platform-path sends before it may collect at their safepoints like any run" and "an image that names platform paths first follows them through shipped code only" with what holds now (the loader runs no code before the graph; a name is a table lookup), keeping every other sentence.
- "The VM mirror is a capability": lock (3) "the encoder names a shipped object by its declaration and refuses one the rule of what may be named where does not allow (`image/shipped_names.rs`)"; lock (4) "the VM mirror's class has no declaration and so no name, and the decoder refuses `Top`, `Activation` and every other shipped name in a position the rule does not allow".
- The `image/` line of the `ns-runtime` crate description if it names `platform_paths.rs`.

- [ ] **Step 4: The rest**

`execution.md` (Task 2's measurement already there -- check), `memory.md` ("The amplification bound of a rejected load": the shipped-name section's resolved entries are a per-entry allocation bounded by its bytes; recompute or state why it stays under the bound), `object-model.md` ("The image format" if it names builtin tags), `README.md`'s map row, `OPEN-WORK.md` (remove the "Next" paragraph on the naming, name sub-project 5's plan as next; the OPEN-WORK item about "`image::package`'s refusal of a shipped class or instance other than the platform" and "the CLI prints nothing when an image's platform path is refused" are rewritten or removed), `PROGRESS.md` (a milestone entry for the naming).

- [ ] **Step 5: Commit**

`git diff --stat` must show only Markdown; no gates. Commit: `docs: describe shipped names in the image as built`.

---

## After the last task (the controller)

Run the `finishing-a-batch` skill's end-of-batch gates (the five stress modes, `run-corpus` plain and under stress, the benchmarks profile, `verifyPlugin` if the analyzer's tests changed), then the whole-branch review on Fable, then write sub-project 5's plan.
