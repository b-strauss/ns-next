# Precompiled shipped code Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The boot decodes the shipped modules' compiled mixins from an artifact embedded in the binary instead of parsing, resolving and compiling about 9,000 lines of Newspeak at every process start.

**Architecture:** `cargo xtask shipped-code` compiles the six shipped modules through today's source path and writes their mixins, encoded by a codec of their own (`crates/ns-runtime/src/shipped_code/`), to a gitignored file; `ns-runtime`'s `build.rs` embeds it with a fingerprint of everything that can change the compiled code. An optimized boot whose artifact matches decodes it; every other boot -- the reference lowering, `NS_NEXT_NO_PRIMITIVE_METHODS`, a missing or stale artifact -- compiles from source as today. From the root mixins on, both paths are one.

**Tech Stack:** Rust 1.98.0, edition 2021; `cargo-nextest`; no new external dependency.

**Spec:** `docs/superpowers/specs/2026-10-14-precompiled-shipped-code-design.md` -- read it before any task.

## Global Constraints

- No new external dependency; `build.rs` uses `std` only.
- `ns-runtime` forbids `unsafe`; nothing here needs it.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or two characters, no truncated word (`writer`, `reader`, `index`, never `w`, `r`, `idx`).
- The image decoder (`crates/ns-runtime/src/image/`) gains no arm, tag or mode for `SendMirror`, `MethodFastPath`, `class_accessor`, `unwind_marker` or `shipped`: lock (6) of the VM mirror stays structural.
- The artifact's decoder is `pub(crate)` and called only by the boot.
- Only the optimized lowering is in the artifact; the reference lowering and `NS_NEXT_NO_PRIMITIVE_METHODS` always compile the shipped modules from source.
- The artifact is `crates/ns-runtime/core-library/shipped-code.bin`, listed in `.gitignore`, never committed.
- A stale or missing artifact falls back to the source path silently: no warning, no `stderr` output.
- Bytes that do not decode are a defect of the binary and `panic!`; the decoder has no error type.
- Comments explain the decision, not the mechanics (`CLAUDE.md`, "Conventions").
- Commit messages are conventional (`feat:`, `fix:`, `test:`, `docs:`, `build:`, `refactor:`), and end with the two trailer lines the controller gives you.

**Gates, staggered** (`CLAUDE.md`, "The gates"): every commit that changes more than Markdown runs `cargo xtask gates commit` first and must be green. The `images`, `caches`, `benchmarks` and `stress` groups run once, in Task 8, at the end of the batch; a commit before Task 8 is work in progress, not finished work. Run every command in the foreground.

## Review Focus

1. **A compiler change with unchanged `.ns` sources** leaves a local artifact stale: the fingerprint covers `ns-runtime`'s and `ns-syntax`'s Rust sources, so the boot must fall back. Pinned by `an_artifact_with_another_fingerprint_is_not_used` (Task 6).
2. **A fresh clone without the artifact** must build and boot from source. Pinned by `an_empty_artifact_is_not_used` (Task 6) and by Task 4's `build.rs` writing an empty file.
3. **`NS_NEXT_STRESS_WIDE`** changes the in-memory encoding: the codec encodes the decoded instructions and re-emits them through `CodeBuilder::new()`, so a decoded body takes the wide form under the stress mode exactly as a compiled one does. Covered by Task 8's stress run; Task 3's round trip pins that the codec reads `instructions()`, never slots.
4. **`NS_NEXT_NO_PRIMITIVE_METHODS=1`** must compile from source. Pinned by `path_for`'s test (Task 6).
5. **A runtime reader of `declaration` on a shipped mixin** would see `None` after a decoded boot. Task 6 runs the whole default suite on the decoded path; any test that read a shipped declaration is moved to a source-path interpreter there.

---

### Task 1: The mixin carries what the kernel phase reads from the AST

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`Mixin`, `MixinApplicationClause`, `Mixin::empty`)
- Modify: `crates/ns-runtime/src/resolver/classes.rs:222-226` (`RootShape` public) and `crates/ns-runtime/src/resolver/mod.rs` (re-export)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs:117-133` (`clause_message_of`, `mixin_application_clause_of`), `mixin_of` (fill the new fields), `build_root_mixin` (root shape), `load_root_class:232` (read the shape off the mixin)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:197-408, 604-615` (`fill_builtin_classes`, `applied_kernel_class`, `superclass_kind`)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (decoded mixins set the new fields to `None`)
- Test: `crates/ns-runtime/src/interpreter/kernel.rs` (its `tests` module)

**Interfaces:**
- Produces: `Mixin::superclass_receiver: Option<Symbol>`, `MixinApplicationClause::receiver: Option<Symbol>`, `Mixin::root_shape: Option<crate::resolver::RootShape>`; `pub struct RootShape { pub over_value: bool, pub applies_value: bool }` (Copy, Eq). `fill_builtin_classes(&mut self, root_mixin: &Rc<Mixin>)` -- no `ResolvedModule`.

- [ ] **Step 1: Write the failing tests** in `kernel.rs`'s `tests` module:

```rust
#[test]
fn every_kernel_class_records_the_receiver_its_clauses_name() {
    let resolved = resolved_kernel();
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let root = mutator.build_root_mixin(
            &resolved.module.root_class,
            &resolved.bindings,
            crate::interpreter::instantiate::ModuleSource::Kernel,
        );
        for nested in &root.nested {
            let declaration = nested.declaration.as_ref().expect("built from source");
            let written = declaration
                .superclass
                .as_ref()
                .map(|clause| Symbol::intern(&clause.receiver.name));
            assert_eq!(nested.superclass_receiver, written, "{}", nested.name);
            let applied: Vec<Option<Symbol>> = declaration
                .mixin_applications
                .iter()
                .map(|clause| Some(Symbol::intern(&clause.receiver.name)))
                .collect();
            let recorded: Vec<Option<Symbol>> = nested
                .mixin_applications
                .iter()
                .map(|clause| clause.receiver)
                .collect();
            assert_eq!(recorded, applied, "{}", nested.name);
        }
    });
}

#[test]
fn the_kernel_root_records_its_shape() {
    let resolved = resolved_kernel();
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let root = mutator.build_root_mixin(
            &resolved.module.root_class,
            &resolved.bindings,
            crate::interpreter::instantiate::ModuleSource::Kernel,
        );
        assert_eq!(
            root.root_shape,
            crate::resolver::root_shape(&resolved.module.root_class)
        );
        assert!(root.root_shape.is_some());
    });
}
```

(If `build_root_mixin` or `ModuleSource` is not reachable from the test module, widen it to `pub(crate)`; do not copy it.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime -E 'test(every_kernel_class_records_the_receiver_its_clauses_name) | test(the_kernel_root_records_its_shape)'`
Expected: compile error -- no field `superclass_receiver`, `receiver`, `root_shape`.

- [ ] **Step 3: Add the fields**

In `mixin.rs`, on `Mixin`, after `superclass`:

```rust
    /// The identifier the superclass clause's receiver names, after any
    /// prefix. The kernel phase reads a kernel class's superclass from it
    /// without running the clause, which it may not run before the kernel
    /// exists; recorded here so that a mixin decoded from the embedded
    /// shipped code, which has no declaration, answers it too. `None` with
    /// no clause, and for a mixin decoded from an image, whose mixins the
    /// kernel phase never reads.
    pub superclass_receiver: Option<Symbol>,
```

after `root_clause_defect`:

```rust
    /// How a module's root is built (`resolver::root_shape`), recorded on
    /// a root mixin built from source so that building its class needs no
    /// declaration. `None` for every other mixin, for a root with a defect,
    /// and for a mixin decoded from an image.
    pub root_shape: Option<crate::resolver::RootShape>,
```

On `MixinApplicationClause`, after `message`:

```rust
    /// The identifier the clause's receiver names, for the kernel phase,
    /// which reads the class a kernel class applies without running the
    /// clause. `None` for a clause decoded from an image.
    pub receiver: Option<Symbol>,
```

`Mixin::empty` sets `superclass_receiver: None` and `root_shape: None`. Make `RootShape` and its two fields `pub`, and re-export it as `crate::resolver::RootShape` beside `root_shape`.

- [ ] **Step 4: Fill them from source.** In `instantiate.rs`:

```rust
fn mixin_application_clause_of(clause: &ast::SuperclassClause) -> MixinApplicationClause {
    MixinApplicationClause {
        message: clause_message_of(clause),
        receiver: Some(Symbol::intern(&clause.receiver.name)),
        message_code: None,
    }
}
```

In `mixin_of`, where `superclass` is set from the declaration's clause, set `superclass_receiver: declaration.superclass.as_ref().map(|clause| Symbol::intern(&clause.receiver.name))` (a class side keeps `None`). In `build_root_mixin`, beside `root_clause_defect`:

```rust
    mixin.root_shape = crate::resolver::root_shape(declaration);
```

In `load_root_class`, replace `crate::resolver::root_shape(declaration)` with `mixin.root_shape` (the `debug_assert_eq!` against `root_clause_defect` stays). In `image/mixins.rs`, set `receiver: None` on every decoded `MixinApplicationClause`; `Mixin::empty` covers the rest.

- [ ] **Step 5: Read them in the kernel phase.** In `kernel.rs`:

`superclass_kind`:

```rust
/// The builtin a kernel class's clause names, read off its mixin and never
/// run; `None` for `Top`.
fn superclass_kind(nested: &Rc<Mixin>) -> Option<BuiltinClass> {
    let receiver = nested.superclass_receiver?;
    Some(kernel_kind_named(receiver.as_str()).unwrap_or_else(|| {
        panic!("internal: Kernel.ns's {} names no kernel class", nested.name)
    }))
}
```

In `fill_builtin_classes`, the applied levels read the mixin's clauses:

```rust
            let applied: Vec<(usize, Value<'gc>)> = mixin
                .mixin_applications
                .iter()
                .enumerate()
                .map(|(index, clause)| {
                    let receiver = clause.receiver.unwrap_or_else(|| {
                        panic!("internal: Kernel.ns's {kind:?} applies a clause with no receiver")
                    });
                    let named = self.applied_kernel_class(
                        &root_mixin,
                        receiver.as_str(),
                        &filled,
                        &mut applied_classes,
                    );
                    (index, Value::object(named))
                })
                .collect();
```

Track `let mut filled: Vec<BuiltinClass> = Vec::new();` before the loop and `filled.push(kind);` after `class_set_superclass` succeeds. In `applied_kernel_class`, take `filled: &[BuiltinClass]` and replace the `declaration.is_some()` assertion with:

```rust
        // A placeholder's mixin was built from no source: applied over it,
        // the class would inherit nothing of its superclass. `fill_order`
        // ranks builtins only, so this is where a kernel that applies a
        // class before its superclass is filled is caught.
        assert!(
            filled.contains(&superclass_kind),
            "internal: Kernel.ns's {name} is applied before its superclass {superclass_kind:?} is filled"
        );
```

Change `fill_builtin_classes` to take `root_mixin: &Rc<Mixin>` and read the shape off it:

```rust
        let shape = root_mixin
            .root_shape
            .unwrap_or_else(|| panic!("internal: Kernel.ns's root breaks the root rule"));
```

Move `build_root_mixin` and `check_kernel_shape(&root_mixin, &resolved)` out of `fill_builtin_classes` into `install_kernel`, which builds the root mixin inside its `enter` and passes it in. Behaviour is unchanged.

- [ ] **Step 6: Run the two tests and the crate**

Run: `cargo nextest run -p ns-runtime`
Expected: all PASS.

- [ ] **Step 7: Gates and commit**

Run: `cargo xtask gates commit` -- green.

```bash
git add crates/ns-runtime/src
git commit -m "refactor: read the kernel's clause receivers and the root shape off the mixin"
```

---

### Task 2: One source path for the shipped root mixins

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`install_shipped_modules`, `resolve_shipped_source`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`install_kernel`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`load_root_class` split)
- Test: `crates/ns-runtime/src/interpreter/core_library.rs` (its `tests` module)

**Interfaces:**
- Consumes: Task 1's `fill_builtin_classes(&Rc<Mixin>)` and `Mixin::root_shape`.
- Produces:
  - `impl<'gc> Mutator<'gc> { pub(crate) fn shipped_root_mixin_from_source(&mut self, name: &str) -> Rc<Mixin> }` -- `"Kernel"` or a `SHIPPED_MODULES` name: parse, resolve, `check_mirror_send_arguments`, compile with the interpreter's lowering, and for `"Kernel"` `check_kernel_shape`.
  - `impl<'gc> Mutator<'gc> { pub(crate) fn root_class_of_mixin(&mut self, mixin: &Rc<Mixin>) -> ClassReference<'gc> }` -- `load_root_class` minus the compilation; `load_root_class(resolved)` becomes `let mixin = self.compile_root_mixin(resolved); self.root_class_of_mixin(&mixin)`.
  - `impl Interpreter { pub(crate) fn shipped_root_mixins(&mut self) -> Vec<(&'static str, Rc<Mixin>)> }` -- after a boot, `"Kernel"` first, then `SHIPPED_MODULES` in order, each the mixin its module instance's class applies (read through `shipped_world.module_instance`, the instance's class and `Heap::class_application`).

- [ ] **Step 1: Write the failing test** in `core_library.rs`'s `tests` module:

```rust
#[test]
fn the_boot_keeps_every_shipped_root_mixin_in_table_order() {
    let mut interpreter = Interpreter::bootstrap();
    let names: Vec<&str> = interpreter
        .shipped_root_mixins()
        .iter()
        .map(|(name, mixin)| {
            assert_eq!(mixin.name.as_str(), *name);
            *name
        })
        .collect();
    let mut expected = vec!["Kernel"];
    expected.extend(SHIPPED_MODULES.iter().map(|shipped| shipped.name));
    assert_eq!(names, expected);
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-runtime -E 'test(the_boot_keeps_every_shipped_root_mixin_in_table_order)'`
Expected: compile error -- no method `shipped_root_mixins`.

- [ ] **Step 3: Implement.** `shipped_root_mixin_from_source` dispatches on the name: `"Kernel"` uses `resolved_kernel()`, `build_root_mixin(.., ModuleSource::Kernel)` and `check_kernel_shape`; any other name `resolved_shipped_module(name)` and `compile_root_mixin` (whose `ModuleSource::CoreLibrary` comes from `resolved.kernel`). `install_kernel` gets its root mixin from it; `install_shipped_modules` replaces `let factory_class = self.load_module(&resolved);` with:

```rust
            let factory_class = self.enter(|mutator| {
                let mixin = mutator.shipped_root_mixin_from_source(name);
                let class = mutator.root_class_of_mixin(&mixin);
                mutator.stash(Value::object(class))
            });
```

`shipped_root_mixins` reads, for each name, `mutator.shipped_world.module_instance(Symbol::intern(name))`, the instance's class (`Mutator::class_of`) and that class's application's `mixin`, cloning the `Rc`.

Update the two stale comments `kernel.rs:22-24` and `core_library.rs:95-97` ("compiled code is numbered against one heap's inline caches"): compiling needs no heap; each interpreter compiles its own copy because the boot builds its world from its own mixins.

- [ ] **Step 4: Run the test and the crate**

Run: `cargo nextest run -p ns-runtime`
Expected: all PASS.

- [ ] **Step 5: Gates and commit**

Run: `cargo xtask gates commit` -- green.

```bash
git add crates/ns-runtime/src
git commit -m "refactor: build every shipped root class from its root mixin"
```

---

### Task 3: The shipped-code codec

**Files:**
- Create: `crates/ns-runtime/src/shipped_code/mod.rs` (constants, `generate`, `decode_module`, `ShippedCodePath`)
- Create: `crates/ns-runtime/src/shipped_code/bytes.rs` (`ByteWriter`, `ByteReader`)
- Create: `crates/ns-runtime/src/shipped_code/encode.rs`
- Create: `crates/ns-runtime/src/shipped_code/decode.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (`pub mod shipped_code;`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs:732-738` (`MirrorPrimitive::from_index`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter::bootstrap_on`)
- Test: `crates/ns-runtime/src/shipped_code/mod.rs` (`tests` module)

**Interfaces:**
- Consumes: Task 2's `Interpreter::shipped_root_mixins`; `CompiledCode::from_builder` (`pub(crate)`), `CodeBuilder::new`, `CompiledCode::instructions`.
- Produces:
  - `pub(crate) const MAGIC: [u8; 4] = *b"NSSC";` `pub(crate) const FORMAT_VERSION: u16 = 1;`
  - `pub(crate) fn encode(modules: &[(&'static str, Rc<Mixin>)], fingerprint: u64) -> Vec<u8>`
  - `pub(crate) fn decode_module(bytes: &[u8], name: &str) -> Rc<Mixin>`
  - `pub(crate) enum ShippedCodePath { Embedded, Source }` (Copy, Eq, Debug)
  - `pub fn generate() -> Vec<u8>` (`#[doc(hidden)]`, for `xtask`): `Interpreter::bootstrap_on(Lowering::Optimized, ShippedCodePath::Source)`, then `encode(&interpreter.shipped_root_mixins(), crate::shipped_code::SHIPPED_CODE_FINGERPRINT)`. Until Task 4 the fingerprint is `0`; Task 4 replaces it.
  - `pub(crate) fn Interpreter::bootstrap_on(lowering: Lowering, path: ShippedCodePath) -> Interpreter` -- `bootstrap_with(lowering)` becomes `bootstrap_on(lowering, ShippedCodePath::Source)` here; Task 6 makes it choose.
  - `MirrorPrimitive::from_index(index: u8) -> MirrorPrimitive` (`pub(crate)`), panicking past `MIRROR.len()`.

- [ ] **Step 1: Write the failing tests** in `shipped_code/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Instruction;

    fn decoded_modules(bytes: &[u8]) -> Vec<(&'static str, Rc<Mixin>)> {
        module_names()
            .map(|name| (name, decode_module(bytes, name)))
            .collect()
    }

    #[test]
    fn generated_shipped_code_round_trips() {
        let generated = generate();
        let decoded = decoded_modules(&generated);
        assert_eq!(encode(&decoded, SHIPPED_CODE_FINGERPRINT), generated);
    }

    #[test]
    fn decoded_shipped_code_keeps_the_mirror_sends_and_fast_paths() {
        let generated = generate();
        let kernel = decode_module(&generated, "Kernel");
        let object = kernel
            .nested
            .iter()
            .find(|nested| nested.name.as_str() == "Object")
            .expect("Kernel.ns declares Object");
        let mut mirror_sends = 0;
        let mut fast_paths = 0;
        for method in object.compiled_methods() {
            fast_paths += usize::from(method.fast_path.is_some());
            mirror_sends += method
                .code
                .instructions()
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::SendMirror { .. }))
                .count();
        }
        assert!(mirror_sends > 0 && fast_paths > 0);
        assert!(kernel.shipped && kernel.declaration.is_none());
    }

    #[test]
    fn generated_shipped_code_is_reproducible() {
        assert_eq!(generate(), generate());
    }
}
```

`module_names()` is `pub(crate) fn module_names() -> impl Iterator<Item = &'static str>`: `"Kernel"` then every `SHIPPED_MODULES` name.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime -E 'test(shipped_code::)'`
Expected: compile error -- module `shipped_code` not found.

- [ ] **Step 3: `bytes.rs`.** Fixed-width little-endian, as the image:

```rust
//! The artifact's byte primitives: fixed-width little-endian numbers and
//! length-prefixed strings. The reader panics on bytes that do not decode:
//! they are the binary's own, so a short read is a defect of the binary.

pub(crate) struct ByteWriter {
    buffer: Vec<u8>,
}

impl ByteWriter {
    pub(crate) fn new() -> ByteWriter {
        ByteWriter { buffer: Vec::new() }
    }
    pub(crate) fn u8(&mut self, value: u8) {
        self.buffer.push(value);
    }
    pub(crate) fn boolean(&mut self, value: bool) {
        self.u8(u8::from(value));
    }
    pub(crate) fn u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn length(&mut self, value: usize) {
        self.u32(u32::try_from(value).expect("internal: a shipped length fits a u32"));
    }
    pub(crate) fn str(&mut self, text: &str) {
        self.length(text.len());
        self.buffer.extend_from_slice(text.as_bytes());
    }
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }
}

pub(crate) struct ByteReader<'bytes> {
    bytes: &'bytes [u8],
    position: usize,
}

impl<'bytes> ByteReader<'bytes> {
    pub(crate) fn new(bytes: &'bytes [u8]) -> ByteReader<'bytes> {
        ByteReader { bytes, position: 0 }
    }
    pub(crate) fn raw(&mut self, length: usize) -> &'bytes [u8] {
        let end = self.position + length;
        let slice = self
            .bytes
            .get(self.position..end)
            .expect("internal: the embedded shipped code ends early");
        self.position = end;
        slice
    }
    pub(crate) fn u8(&mut self) -> u8 {
        self.raw(1)[0]
    }
    pub(crate) fn boolean(&mut self) -> bool {
        match self.u8() {
            0 => false,
            1 => true,
            other => panic!("internal: the embedded shipped code has a boolean {other}"),
        }
    }
    pub(crate) fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.raw(2).try_into().expect("two bytes"))
    }
    pub(crate) fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.raw(4).try_into().expect("four bytes"))
    }
    pub(crate) fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.raw(8).try_into().expect("eight bytes"))
    }
    pub(crate) fn length(&mut self) -> usize {
        self.u32() as usize
    }
    pub(crate) fn str(&mut self) -> &'bytes str {
        let length = self.length();
        std::str::from_utf8(self.raw(length)).expect("internal: the embedded shipped code holds UTF-8")
    }
}
```

- [ ] **Step 4: The format, in `mod.rs`'s doc comment and `encode.rs`.**

Artifact: `MAGIC`, `FORMAT_VERSION` (`u16`), the fingerprint (`u64`), the module count (`u32`), then per module its name (`str`), its section's byte length (`u32`) and the section. A section: the symbol count (`u32`) and each symbol (`str`), in order of first use, then the root mixin. A symbol in a mixin or code is its `u32` index in its section's table.

`encode.rs` builds each section in two passes: write the mixin into a `ByteWriter` while a `SymbolTable` collects symbols, then write the table and append the mixin's bytes.

```rust
struct SymbolTable {
    indices: std::collections::HashMap<&'static str, u32>,
    names: Vec<&'static str>,
}

impl SymbolTable {
    fn index(&mut self, symbol: Symbol) -> u32 {
        let name = symbol.as_str();
        if let Some(index) = self.indices.get(name) {
            return *index;
        }
        let index = u32::try_from(self.names.len()).expect("internal: symbol count fits a u32");
        self.indices.insert(name, index);
        self.names.push(name);
        index
    }
}
```

**Per mixin** -- destructure without `..`, so a new field does not compile until its treatment is decided:

```rust
fn write_mixin(out: &mut Out, mixin: &Mixin) {
    let Mixin {
        name,
        access,
        methods,
        slots,
        factory,
        superclass,
        superclass_receiver,
        mixin_applications,
        has_initializer_expressions,
        // Read only by the compiler, which has run.
        initializer_expressions: _,
        nested,
        class_side,
        span,
        // Read only by the compiler and the source path's checks.
        declaration: _,
        initializer_code,
        superclass_clause_code,
        superclass_message_code,
        root_clause_defect,
        root_shape,
        shipped,
    } = mixin;
    assert!(
        root_clause_defect.is_none(),
        "internal: a shipped root passed the resolver, which refuses a defect"
    );
    out.symbol(*name);
    out.access(*access);
    // Sorted by selector name: a `HashMap`'s order is not the bytes'.
    let mut entries: Vec<(&Symbol, &MethodEntry)> = methods.iter().collect();
    entries.sort_by_key(|(selector, _)| selector.as_str());
    out.length(entries.len());
    for (selector, entry) in entries {
        out.symbol(*selector);
        out.method_entry(entry);
    }
    out.length(slots.len());
    for slot in slots {
        let MixinSlot { name, mutable, access, lazy, declaration: _ } = slot;
        out.symbol(*name);
        out.boolean(*mutable);
        out.access(*access);
        out.boolean(*lazy);
    }
    out.option(factory.as_ref(), |out, factory| {
        out.symbol(factory.selector);
        out.length(factory.argument_count);
    });
    out.option(superclass.as_ref(), |out, message| out.clause_message(message));
    out.option(superclass_receiver.as_ref(), |out, receiver| out.symbol(*receiver));
    out.length(mixin_applications.len());
    for clause in mixin_applications {
        let MixinApplicationClause { message, receiver, message_code } = clause;
        out.clause_message(message);
        out.option(receiver.as_ref(), |out, receiver| out.symbol(*receiver));
        out.option(message_code.as_ref(), |out, code| out.code(code));
    }
    out.boolean(*has_initializer_expressions);
    out.length(nested.len());
    for nested in nested {
        write_mixin(out, nested);
    }
    out.option(class_side.as_ref(), |out, class_side| write_mixin(out, class_side));
    out.span(*span);
    out.option(initializer_code.as_ref(), |out, code| out.code(code));
    out.option(superclass_clause_code.as_ref(), |out, code| out.code(code));
    out.option(superclass_message_code.as_ref(), |out, code| out.code(code));
    out.option(root_shape.as_ref(), |out, shape| {
        out.boolean(shape.over_value);
        out.boolean(shape.applies_value);
    });
    out.boolean(*shipped);
}
```

`Out` holds the section's `ByteWriter` and `SymbolTable`; `out.option(value, write)` writes a `0`/`1` byte then the value. `access` writes `Public` 0, `Protected` 1, `Private` 2. `clause_message` writes `selector` and `has_arguments`. `span` writes `start` and `end` as `u32`.

**Method entries:** tag `0` `Compiled`, `1` `SlotAccessor`, `2` `Factory`; `Primitive` panics (`"internal: only the mirror's class holds a primitive, and Rust builds it"`).

```rust
    fn method_entry(&mut self, entry: &MethodEntry) {
        match entry {
            MethodEntry::Compiled(method) => {
                let MixinMethod {
                    selector,
                    access,
                    argument_count,
                    // Read only by the compiler.
                    declaration: _,
                    code,
                    fast_path,
                    class_accessor,
                } = &**method;
                self.u8(0);
                self.symbol(*selector);
                self.access(*access);
                self.length(*argument_count);
                self.code(code);
                self.option(fast_path.as_ref(), |out, fast_path| out.fast_path(*fast_path));
                self.option(class_accessor.as_ref(), |out, accessor| {
                    out.u16(accessor.depth);
                    out.symbol(accessor.class_name);
                });
            }
            MethodEntry::SlotAccessor(SlotAccessor { selector, access, own_index, kind }) => {
                self.u8(1);
                self.symbol(*selector);
                self.access(*access);
                self.u32(*own_index);
                self.u8(match kind {
                    AccessorKind::Getter => 0,
                    AccessorKind::Setter => 1,
                });
            }
            MethodEntry::Factory(FactoryMethod { selector, argument_count, access }) => {
                self.u8(2);
                self.symbol(*selector);
                self.length(*argument_count);
                self.access(*access);
            }
            MethodEntry::Primitive(_) => {
                panic!("internal: only the mirror's class holds a primitive, and Rust builds it")
            }
        }
    }
```

**Fast paths:** `Mirror(primitive)` 0 + `u8` index; `BuiltinClass(kind)` 1 + `u8` position in `BuiltinClass::ALL`; `Constant` 2 + `u8` (`True` 0, `False` 1, `Nil` 2); `Receiver` 3; `Parameter(position)` 4 + `u8`; `ValueOfParameter` 5 + `u8`; `CullParameter` 6 + `u8`.

**Per `CompiledCode`** -- destructure the public fields, encode the decoded instructions (never the slots, so the in-memory form, wide or narrow, is the decoder's `CodeBuilder`'s to choose):

```rust
    fn code(&mut self, code: &CompiledCode) {
        let instructions = code.instructions();
        let CompiledCode {
            kind,
            argument_count,
            register_count,
            statement_program_counter,
            // Both derived from the instructions by `from_builder`, as the
            // compiler derives them, so `outer_reach_of` decides them here too.
            family_size: _,
            copied_count,
            outer_reach: _,
            unwind_marker,
            source,
            ..
        } = code;
        self.u8(match kind {
            CodeKind::Method => 0,
            CodeKind::Block => 1,
            CodeKind::Initializer => 2,
            CodeKind::SuperclassClause => 3,
            CodeKind::SuperclassMessage => 4,
        });
        self.u16(*argument_count);
        self.u16(*register_count);
        self.u16(*copied_count);
        self.length(statement_program_counter.len());
        for (statement, range) in statement_program_counter {
            self.length(*statement);
            self.length(range.start);
            self.length(range.end);
        }
        self.option(unwind_marker.as_ref(), |out, marker| out.unwind_marker(*marker));
        self.span(*source);
        self.length(instructions.len());
        for instruction in &instructions {
            self.instruction(instruction);
        }
    }
```

`CompiledCode`'s `body` is private, so the `..` above covers exactly `body`; add a comment saying so. `unwind_marker`: `Handler` 0 + three `u16`, `Ensure` 1 + two `u16`, `IfCurtailed` 2 + two `u16`.

**Instructions:** one `u8` tag per variant, in `Instruction`'s declaration order from 0 (`LoadLiteral` 0 … `IntegerBinary` 31), then every field in declaration order. Registers and depths `u16`, `own_index`, `target`, `failure` and `CallSite::number` `u32`, a register list as its length then each `u16`, `selector` a symbol, `primitive` its `u8` index, `operation` a `u8` in `IntegerOperation`'s order, `parameters` a length then symbols, `MakeClosure`'s `code` nested with `code()`. The match has no `_` arm. Literals: `Nil` 0, `Boolean` 1 + boolean, `SmallInteger` 2 + `u64` of the `i64`, `Float` 3 + `u64` of `to_bits()`, `String` 4 + `str`, `Symbol` 5 + symbol, `LargeInteger` 6 + sign boolean + digit count + each `u64` digit (`is_negative`, `magnitude`), `UnresolvedIdentifier` 7 + `str`, `BodyTooLarge` 8. These numbers are the artifact's own; nothing in `image/opcodes.rs` changes.

- [ ] **Step 5: `decode.rs`** mirrors `encode.rs` field for field, with `ByteReader`, a `Vec<Symbol>` built by `Symbol::intern` for the section's table, and `Mixin::empty(name)` as the base it fills. Every mixin field the encoder skipped stays at `Mixin::empty`'s value (`declaration: None`, `initializer_expressions: vec![]`); `root_clause_defect: None`. A method's `declaration: None`; a slot's `declaration: None`. A body:

```rust
    fn code(&mut self) -> Rc<CompiledCode> {
        let kind = self.code_kind();
        let argument_count = self.reader.u16();
        let register_count = self.reader.u16();
        let copied_count = self.reader.u16();
        let statement_program_counter = (0..self.reader.length())
            .map(|_| {
                let statement = self.reader.length();
                let start = self.reader.length();
                let end = self.reader.length();
                (statement, start..end)
            })
            .collect();
        let unwind_marker = self.option(|decoder| decoder.unwind_marker());
        let source = self.span();
        let mut builder = CodeBuilder::new();
        for _ in 0..self.reader.length() {
            let instruction = self.instruction();
            builder.emit(instruction);
        }
        let mut code = CompiledCode::from_builder(
            kind,
            builder,
            argument_count,
            register_count,
            copied_count,
            statement_program_counter,
            source,
        );
        code.unwind_marker = unwind_marker;
        Rc::new(code)
    }
```

An unknown tag panics naming what it was decoding (`"internal: the embedded shipped code has instruction tag {tag}"`). `decode_module(bytes, name)` reads the header (asserting the magic and version), walks the module entries, skipping each section by its length until the name matches, and decodes that section; a missing name panics.

Add `MirrorPrimitive::from_index` in `primitive.rs`:

```rust
    /// The row at `index` of `MIRROR`, for the embedded shipped code, whose
    /// bytes are the binary's own.
    pub(crate) fn from_index(index: u8) -> MirrorPrimitive {
        assert!(usize::from(index) < MIRROR.len(), "internal: no mirror row {index}");
        MirrorPrimitive(index)
    }
```

Add `ShippedCodePath`, `module_names`, `generate`, and `pub(crate) const SHIPPED_CODE_FINGERPRINT: u64 = 0;` (Task 4 replaces the constant) to `mod.rs`; add `Interpreter::bootstrap_on(lowering, path)` -- today it ignores `path` beyond being stored for Task 6 -- and route `bootstrap_with` through it with `ShippedCodePath::Source`.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-runtime -E 'test(shipped_code::)'`
Expected: 3 PASS.

- [ ] **Step 7: Gates and commit**

Run: `cargo xtask gates commit` -- green.

```bash
git add crates/ns-runtime/src
git commit -m "feat: encode and decode the shipped modules' compiled mixins"
```

---

### Task 4: `build.rs` -- the fingerprints and the embedded artifact

**Files:**
- Create: `crates/ns-runtime/build.rs`
- Modify: `crates/ns-runtime/Cargo.toml` (`build = "build.rs"` only if Cargo does not pick it up by default; it does at the package root)
- Modify: `crates/ns-runtime/src/shipped_code/mod.rs` (constants from `OUT_DIR`, `embedded()`)
- Modify: `crates/ns-runtime/src/image/fingerprint.rs` (hash the core library's constant instead of the sources)
- Modify: `.gitignore`
- Test: `crates/ns-runtime/src/shipped_code/mod.rs` (`tests`)

**Interfaces:**
- Produces: `pub(crate) const SHIPPED_CODE_FINGERPRINT: u64`, `pub(crate) const CORE_LIBRARY_FINGERPRINT: u64` (both `include!`d from `OUT_DIR/fingerprints.rs`), `pub(crate) fn embedded() -> &'static [u8]` (`include_bytes!(concat!(env!("OUT_DIR"), "/shipped-code.bin"))`).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn the_two_fingerprints_differ() {
        // The code's covers the Rust sources too; equal, it would cover the
        // core library alone and a compiler change would leave a stale
        // artifact in use.
        assert_ne!(SHIPPED_CODE_FINGERPRINT, CORE_LIBRARY_FINGERPRINT);
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-runtime -E 'test(the_two_fingerprints_differ)'`
Expected: compile error -- no `CORE_LIBRARY_FINGERPRINT`.

- [ ] **Step 3: Write `build.rs`**

```rust
//! The shipped code's fingerprints and the embedded artifact
//! (`docs/design/core-library.md`, "The embedded shipped code").
//!
//! The code's fingerprint covers everything that can change what the
//! shipped modules compile to -- the core library's sources and the Rust
//! sources of `ns-syntax` and `ns-runtime` -- so an artifact a compiler
//! change made stale is never decoded: it invalidates more often than
//! needed, and the boot then compiles from source until the artifact is
//! regenerated. The core library's alone goes into an image's fingerprint,
//! which must not change with every Rust edit.

use std::fs;
use std::path::{Path, PathBuf};

const ARTIFACT: &str = "core-library/shipped-code.bin";

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let workspace = manifest.join("../..");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by Cargo"));

    let library = files_under(&manifest.join("core-library"), "ns");
    let mut code = library.clone();
    code.extend(files_under(&manifest.join("src"), "rs"));
    code.extend(files_under(&workspace.join("crates/ns-syntax/src"), "rs"));
    code.push(manifest.join("Cargo.toml"));
    code.push(workspace.join("crates/ns-syntax/Cargo.toml"));

    fs::write(
        out.join("fingerprints.rs"),
        format!(
            "pub(crate) const CORE_LIBRARY_FINGERPRINT: u64 = {:#x};\n\
             pub(crate) const SHIPPED_CODE_FINGERPRINT: u64 = {:#x};\n",
            fingerprint(&workspace, &library),
            fingerprint(&workspace, &code),
        ),
    )
    .expect("OUT_DIR is writable");

    // A tree without the artifact -- a fresh clone -- embeds an empty one
    // and boots from source.
    let artifact = manifest.join(ARTIFACT);
    let bytes = fs::read(&artifact).unwrap_or_default();
    fs::write(out.join("shipped-code.bin"), bytes).expect("OUT_DIR is writable");

    // Directories, not the artifact itself: Cargo reruns a script whose
    // named file is missing on every build, and a directory's scan sees the
    // artifact appear.
    println!("cargo:rerun-if-changed=core-library");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../ns-syntax/src");
    println!("cargo:rerun-if-changed=../ns-syntax/Cargo.toml");
}

/// Every file under `directory` with `extension`, sorted, so that the
/// fingerprint does not depend on the file system's order.
fn files_under(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|found| found == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// FNV-1a over each file's path relative to the workspace, with `/`
/// separators, and its bytes, each preceded by its length.
fn fingerprint(workspace: &Path, files: &[PathBuf]) -> u64 {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    let mut part = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let workspace = workspace.canonicalize().expect("the workspace exists");
    for file in files {
        let file = file.canonicalize().expect("a listed file exists");
        let relative = file.strip_prefix(&workspace).expect("inside the workspace");
        let name = relative.to_string_lossy().replace('\\', "/");
        part(name.as_bytes());
        part(&fs::read(&file).expect("a listed file is readable"));
    }
    state
}
```

Check `clippy` accepts the closure names and the workspace lints; `build.rs` is linted with the package.

- [ ] **Step 4: Use it.** In `shipped_code/mod.rs` replace the placeholder constant:

```rust
include!(concat!(env!("OUT_DIR"), "/fingerprints.rs"));

/// The artifact `cargo xtask shipped-code` wrote, embedded when the crate
/// was built; empty when the tree had none.
pub(crate) fn embedded() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/shipped-code.bin"))
}
```

In `image/fingerprint.rs`, replace the loop over `KERNEL_SOURCE` and `SHIPPED_MODULES` with `hasher.part(&crate::shipped_code::CORE_LIBRARY_FINGERPRINT.to_le_bytes());` and update its doc comment ("FNV-1a over the core library's fingerprint, which `build.rs` takes over its sources, …"). Drop the now-unused imports.

Add to `.gitignore`:

```
/crates/ns-runtime/core-library/shipped-code.bin
```

- [ ] **Step 5: Run the test and the images' tests**

Run: `cargo nextest run -p ns-runtime -E 'test(the_two_fingerprints_differ) | test(fingerprint)'`
Expected: PASS. Then `cargo nextest run --workspace` -- green (no artifact exists yet; every boot is still on the source path).

- [ ] **Step 6: Gates and commit**

Run: `cargo xtask gates commit` -- green.

```bash
git add crates/ns-runtime/build.rs crates/ns-runtime/src .gitignore crates/ns-runtime/Cargo.toml
git commit -m "build: fingerprint the shipped code and embed its artifact when there is one"
```

---

### Task 5: `cargo xtask shipped-code`, and the gates run it first

**Files:**
- Modify: `xtask/Cargo.toml` (`ns-runtime = { path = "../crates/ns-runtime" }`)
- Modify: `xtask/src/main.rs`
- Modify: `CLAUDE.md` ("The gates": a sentence above the table; the `xtask/` paragraph)

**Interfaces:**
- Consumes: `ns_runtime::shipped_code::generate()` (Task 3, `pub`, `#[doc(hidden)]`).
- Produces: `cargo xtask shipped-code` writes `crates/ns-runtime/core-library/shipped-code.bin`; `cargo xtask shipped-code --check` writes nothing and exits 1 when the file is missing or differs from `generate()`'s bytes, 0 otherwise; `cargo xtask gates <groups…>` runs `shipped-code` once before its first group.

- [ ] **Step 1: Implement.** In `main`, before the gate parsing, a `shipped-code` arm:

```rust
const SHIPPED_CODE: &str = "crates/ns-runtime/core-library/shipped-code.bin";

/// Writes the shipped modules' compiled mixins where `ns-runtime`'s
/// `build.rs` embeds them; with `check`, only says whether the file there
/// is current.
fn shipped_code(root: &Path, check: bool) -> ExitCode {
    let generated = ns_runtime::shipped_code::generate();
    let path = root.join(SHIPPED_CODE);
    let current = std::fs::read(&path).is_ok_and(|existing| existing == generated);
    if check {
        if current {
            println!("{SHIPPED_CODE} is current");
            return ExitCode::SUCCESS;
        }
        println!("{SHIPPED_CODE} is missing or stale: run `cargo xtask shipped-code`");
        return ExitCode::FAILURE;
    }
    if !current {
        std::fs::write(&path, &generated).expect("the core library directory is writable");
    }
    println!("{SHIPPED_CODE}: {} bytes", generated.len());
    ExitCode::SUCCESS
}
```

Writing only when the bytes differ keeps `build.rs` from rebuilding `ns-runtime` after an unchanged run. In the `gates` path, call `shipped_code(&root, false)` once before the first group and stop on failure; record its time as the other steps' are. Use the names `root` and the time bookkeeping `main.rs` already has.

- [ ] **Step 2: Run it**

Run: `cargo xtask shipped-code` -- prints the size; `cargo xtask shipped-code --check` -- "is current", exit 0. Touch `crates/ns-runtime/src/lib.rs` (add and remove a blank line, or `touch`), then `cargo xtask shipped-code --check` -- still "is current" (the bytes are the same; only a change to the compiled code makes them differ).

- [ ] **Step 3: Document.** In `CLAUDE.md`, "The gates", after "Each row is a group of `cargo xtask gates <group>` …": "Every `cargo xtask gates` call first runs `cargo xtask shipped-code`, which writes the embedded shipped code (`crates/ns-runtime/core-library/shipped-code.bin`, gitignored; `docs/design/core-library.md`, "The embedded shipped code")." In the `xtask/` paragraph: "and `shipped-code`, which writes that artifact (`--check` only says whether it is current)."

- [ ] **Step 4: Gates and commit**

Run: `cargo xtask gates commit` -- green; it runs `shipped-code` first.

```bash
git add xtask CLAUDE.md
git commit -m "build: generate the shipped code with cargo xtask, first in every gate run"
```

---

### Task 6: The boot decodes the embedded shipped code

**Files:**
- Modify: `crates/ns-runtime/src/shipped_code/mod.rs` (`usable`, `path_for`, tests)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::shipped_code_path`, `bootstrap_with`, `bootstrap_on`, `with_kernel`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (the field's initial value, where `lowering` is set)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`shipped_root_mixin`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`install_kernel`; the test at about line 983)
- Modify: any test the decoded boot breaks (Step 6)

**Interfaces:**
- Consumes: Task 3's `decode_module`, `ShippedCodePath`; Task 4's `embedded()`, `SHIPPED_CODE_FINGERPRINT`; Task 2's `shipped_root_mixin_from_source`.
- Produces:
  - `pub(crate) fn usable(bytes: &[u8]) -> bool` -- at least the header, magic, `FORMAT_VERSION` and `SHIPPED_CODE_FINGERPRINT` match.
  - `pub(crate) fn path_for(lowering: Lowering, primitive_methods_disabled: bool, bytes: &[u8]) -> ShippedCodePath`.
  - `Mutator::shipped_code_path: ShippedCodePath` (a field beside `lowering`).
  - `impl<'gc> Mutator<'gc> { pub(crate) fn shipped_root_mixin(&mut self, name: &str) -> Rc<Mixin> }` -- `Embedded`: `decode_module(embedded(), name)`; `Source`: `shipped_root_mixin_from_source(name)`.

- [ ] **Step 1: Write the failing tests** in `shipped_code/mod.rs`'s `tests`:

```rust
    fn header(fingerprint: u64) -> Vec<u8> {
        let mut writer = bytes::ByteWriter::new();
        writer.raw(&MAGIC);
        writer.u16(FORMAT_VERSION);
        writer.u64(fingerprint);
        writer.u32(0);
        writer.into_bytes()
    }

    #[test]
    fn an_empty_artifact_is_not_used() {
        assert_eq!(path_for(Lowering::Optimized, false, &[]), ShippedCodePath::Source);
    }

    #[test]
    fn an_artifact_with_another_fingerprint_is_not_used() {
        let stale = header(SHIPPED_CODE_FINGERPRINT ^ 1);
        assert_eq!(path_for(Lowering::Optimized, false, &stale), ShippedCodePath::Source);
    }

    #[test]
    fn only_the_optimized_lowering_with_primitive_methods_uses_the_artifact() {
        let current = header(SHIPPED_CODE_FINGERPRINT);
        assert_eq!(path_for(Lowering::Optimized, false, &current), ShippedCodePath::Embedded);
        assert_eq!(path_for(Lowering::Reference, false, &current), ShippedCodePath::Source);
        assert_eq!(path_for(Lowering::Optimized, true, &current), ShippedCodePath::Source);
    }

    #[test]
    fn the_optimized_boot_decodes_the_embedded_code() {
        assert!(
            usable(embedded()),
            "the embedded shipped code is missing or stale: run `cargo xtask shipped-code`"
        );
        let mut interpreter = Interpreter::bootstrap();
        let path = interpreter.enter(|mutator| mutator.shipped_code_path);
        assert_eq!(path, ShippedCodePath::Embedded);
    }

    #[test]
    fn the_reference_boot_compiles_from_source() {
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Reference);
        let path = interpreter.enter(|mutator| mutator.shipped_code_path);
        assert_eq!(path, ShippedCodePath::Source);
    }

    #[test]
    fn the_embedded_shipped_code_round_trips() {
        let bytes = embedded();
        assert!(usable(bytes), "run `cargo xtask shipped-code`");
        let decoded = decoded_modules(bytes);
        assert_eq!(encode(&decoded, SHIPPED_CODE_FINGERPRINT), bytes);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo xtask shipped-code`, then `cargo nextest run -p ns-runtime -E 'test(shipped_code::)'`
Expected: compile error -- no `path_for`, `usable`, `shipped_code_path`.

- [ ] **Step 3: Implement the choice**

```rust
/// Whether `bytes` is an artifact this binary may decode: one written for
/// exactly the sources it was built from.
pub(crate) fn usable(bytes: &[u8]) -> bool {
    let header_length = MAGIC.len() + 2 + 8;
    if bytes.len() < header_length {
        return false;
    }
    let mut reader = bytes::ByteReader::new(bytes);
    reader.raw(MAGIC.len()) == MAGIC
        && reader.u16() == FORMAT_VERSION
        && reader.u64() == SHIPPED_CODE_FINGERPRINT
}

/// Where a boot takes the shipped modules' mixins from. Only the optimized
/// lowering's code is embedded, and primitive methods are decided when a
/// body is compiled, so the reference lowering and a boot without them
/// compile from source -- the reference lowering stays an oracle that
/// compiles everything afresh. A missing or stale artifact falls back
/// silently: a warning would change every golden's `stderr`, and the gates
/// regenerate the artifact before they test.
pub(crate) fn path_for(
    lowering: Lowering,
    primitive_methods_disabled: bool,
    bytes: &[u8],
) -> ShippedCodePath {
    if lowering == Lowering::Optimized && !primitive_methods_disabled && usable(bytes) {
        ShippedCodePath::Embedded
    } else {
        ShippedCodePath::Source
    }
}
```

`bootstrap_with(lowering)` becomes `bootstrap_on(lowering, path_for(lowering, crate::compiler::primitive_method::disabled(), embedded()))`; `bootstrap_on` hands the path to `with_kernel`, which stores it on the `Mutator` (through `World::new` or right after it, before `install_kernel`). `install_kernel` and `install_shipped_modules` call `shipped_root_mixin(name)` instead of `shipped_root_mixin_from_source(name)`. Decoding happens once per boot; nothing caches decoded mixins across interpreters.

- [ ] **Step 4: Run the shipped-code tests**

Run: `cargo nextest run -p ns-runtime -E 'test(shipped_code::)'`
Expected: all PASS.

- [ ] **Step 5: Run the whole suite on the decoded boot**

Run: `cargo nextest run --workspace`
Expected: green, or failures only in tests that read a shipped mixin's `declaration`, a method's `declaration` or a slot's `declaration`.

- [ ] **Step 6: Move every such test to the source path.** Find them with `grep -rn "\.declaration" crates/ns-runtime/src crates/ns-runtime/tests crates/ns-cli/tests crates/ns-analyzer`. Known: `kernel.rs`, about line 983, asserts every builtin's application mixin has a declaration. Such a test asks about the source path, so it bootstraps with `Interpreter::bootstrap_on(Lowering::Optimized, ShippedCodePath::Source)` (widen to `pub` with `#[doc(hidden)]` only if an integration test needs it) and says so in a one-line comment. Do not weaken an assertion; if a non-test reader of a shipped `declaration` turns up, stop and report it -- the spec says none exists.

- [ ] **Step 7: Run the suite again**

Run: `cargo nextest run --workspace` -- green. `cargo test --workspace --doc` -- green.

- [ ] **Step 8: Gates and commit**

Run: `cargo xtask gates commit` -- green.

```bash
git add crates
git commit -m "feat: boot the shipped modules from their embedded compiled code"
```

---

### Task 7: Measure, and decide the second stage

**Files:**
- Modify: `docs/design/execution.md` (new section "What the precompiled shipped code moved", after "What the collections module cost")
- Temporary, never committed: timing marks in `interpreter/kernel.rs`, `interpreter/core_library.rs`, `interpreter/mod.rs` and a `boot_timing.rs`

**Interfaces:** none.

- [ ] **Step 1: Read the `measuring-performance` skill** and follow its protocol for builds (the same explicit `RUSTFLAGS` for both binaries).

- [ ] **Step 2: Build both binaries.** `master`'s head (source path) into one directory, this branch's head into another, both release; on the branch run `cargo xtask shipped-code` before building so the artifact is embedded.

- [ ] **Step 3: Instrument both, temporarily.** A `boot_timing::mark(label)` that, when `NS_NEXT_BOOT_TIMES` is set, prints `BOOT\t<label>\t<microseconds since the previous mark>` to `stderr`; marks at the start of `bootstrap_with`, and around each module's root mixin (source: parse, resolve, compile separately; branch: the decode), its class, its instantiation, `complete_shipped_world` and the platform's `build`. The program (`hello.ns`, in the session's scratchpad):

```newspeak
class Hello = () (
  public main: platform args: args = (
    platform transcript show: 'hello'.
  )
)
```

- [ ] **Step 4: Measure.** Per binary: 30 runs with `NS_NEXT_BOOT_TIMES=1`, the median per label; then 40 runs of the whole process without it, timed from outside (`time.perf_counter()` around `subprocess.run`), the median.

- [ ] **Step 5: Remove the instrumentation** (`git checkout -- crates/ns-runtime/src`, delete `boot_timing.rs`); `git status` shows no change under `crates/`.

- [ ] **Step 6: Record.** In `execution.md`, the new section: the date, the two commits, the machine (Windows 11, the measuring workstation), the table of phases per module for both binaries, the boot's sum and the whole process for both, and the 2026-10-14 measurement from the spec's table as the baseline it repeats. Say what decoding costs per module.

- [ ] **Step 7: Decide.** Sum the branch's decode and instantiation of `CollectionsFactory` and `TypedArraysFactory`. **If it exceeds 0.5 ms, stop and report the numbers to the controller**: the second stage (loading a module on first demand) is then designed with the user before any code. Otherwise record in `execution.md` that it does not pay at this size, and carry the item to Task 9's `OPEN-WORK.md` change with the numbers.

- [ ] **Step 8: Commit**

```bash
git add docs/design/execution.md
git commit -m "docs: measure what the precompiled shipped code moved"
```

---

### Task 8: The end of the batch's gates

**Files:** none, unless a gate fails.

- [ ] **Step 1: Run them**

Run: `cargo xtask gates end-of-batch` -- `images`, `caches`, `benchmarks`, `stress`, each after `shipped-code`.
Expected: green. The stress logs land in `.temp/stress-<MODE>.log`.

- [ ] **Step 2: Release size tests**

Run: `cargo nextest run --workspace --release` -- green (after `cargo xtask shipped-code`, which the gates already ran).

- [ ] **Step 3: On a failure**, use superpowers:systematic-debugging; a fix is its own commit, its gates run again, and so does this task.

---

### Task 9: The living documents

**Files:**
- Modify: `docs/design/core-library.md` (a section "The embedded shipped code", near the bootstrap's; and wherever it says the boot compiles the shipped source)
- Modify: `docs/design/image-format.md` ("Shipped names": the paragraph on the cost of building the whole world; "Why one mechanism" or the lock list: why the embedded code leaves lock (6) untouched)
- Modify: `docs/design/README.md` ("The map", if it lists the parts of the bootstrap)
- Modify: `docs/OPEN-WORK.md`
- Modify: `CLAUDE.md` (`ns-runtime`'s description)
- Modify: `.claude/skills/measuring-performance/SKILL.md`

**Interfaces:** none.

- [ ] **Step 1: `core-library.md`, "The embedded shipped code"**, in the present tense: what the artifact holds and what it leaves out; the two paths and when each is taken; the fingerprint and what it covers; `cargo xtask shipped-code` and the gates; the codec of its own and why (lock (6) structural, no verification of the binary's own bytes); why only the optimized lowering; the decisions the spec rejected, each with its reason. Cite `shipped_code/`, `build.rs`, `xtask/src/main.rs`.

- [ ] **Step 2: `image-format.md`.** "Shipped names": the cost paragraph now says the boot decodes the shipped code and builds the world from it, with the numbers Task 7 measured and a pointer to `execution.md`; the "two ways back … are open" sentence goes. Where the locks are explained: the embedded code is decoded by a codec the image never reaches, so no image byte sequence decodes into `SendMirror`.

- [ ] **Step 3: `OPEN-WORK.md`.** Remove "The cost of building the shipped world at boot". Add, each with its date and pointer: the parser's 3.4 ms for `Kernel.ns`, which `run` from source, the reference lowering and a stale artifact still pay; a release binary without the artifact boots slowly, so a later packaging (`cargo install` and the like) needs a way to require it; and, if Task 7 decided against it, loading a module on first demand, with the numbers. "Next" goes back to weak references as it stood before this batch.

- [ ] **Step 4: `CLAUDE.md`.** In `ns-runtime`'s description, after the core library's paragraph: "`shipped_code/` encodes and decodes the shipped modules' compiled mixins; `build.rs` embeds the artifact `cargo xtask shipped-code` writes (`core-library/shipped-code.bin`, gitignored) with a fingerprint of the sources that compile to it, and an optimized boot decodes it -- every other boot, and a stale or missing artifact, compiles the shipped source (`core-library.md`, "The embedded shipped code")."

- [ ] **Step 5: The `measuring-performance` skill.** Before any build that is measured: `cargo xtask shipped-code`, or the binary boots from source and measures that.

- [ ] **Step 6: Commit**

```bash
git add docs CLAUDE.md .claude/skills/measuring-performance/SKILL.md
git commit -m "docs: describe the embedded shipped code"
```

The spec is deleted and `PROGRESS.md` gets its entry by the `finishing-a-batch` skill, at the end of the batch.
