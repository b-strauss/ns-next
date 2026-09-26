# The platform module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close §6.3.3's lazy-slot deviation, name every shipped object an image reaches by its path from the platform, and move `Platform`, `Transcript`, `Collections` and `KernelNamespace` out of `Kernel.ns` into a shipped `Platform.ns` booted with `usingVmMirror:kernel:`.

**Architecture:** Plan 1 of the two in `docs/design/core-library.md`, "Planned: the platform and collections as core-library modules". Lazy slots come first, because the platform's members become lazy slots and the image's platform paths rest on them: a lazy slot's getter is a compiled method that evaluates the initializer, stores it with the existing `InitializeSlot` and answers it, and a `MethodFastPath::LazySlot` answers a filled slot without an activation. Platform paths come second, built and tested against today's kernel-built platform, so that the third part — the boot of `Platform.ns` from a table of shipped modules — lands with an image format that already names what it reaches.

**Tech Stack:** Rust 1.98.0 (edition 2021), the `ns-syntax`, `ns-runtime`, `ns-cli` and `ns-lsp` crates, `indoc` and `insta` as dev-dependencies, Newspeak source under `crates/ns-runtime/core-library/`.

**Spec:** `docs/design/core-library.md`, "Planned: the platform and collections as core-library modules" — the sections "Lazy slots first (plan 1)", "The module table and the boot (plan 1)", "Platform paths in the image (plan 1)", "What changes in the invariants" and "What is measured". Read it before any task; the plan argues from it.

## Global Constraints

- All code, identifiers, comments, doc comments and commit messages are in English.
- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; `clippy.toml`'s `min_ident_chars`). This applies to every code sample below as written.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Never `{:?}` a `Value`; never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult` or a `HostResult`. Tests unwrap through a local `expect_ok` helper, copied from a neighbouring test file (`crates/ns-runtime/tests/platform.rs` has one).
- A Newspeak module written inline in a test is an `indoc! {"…"}` (or `formatdoc!`), indented with spaces, closing `"}` on a line of its own.
- Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`; add a missing accessor there.
- `vm.rs` holds no dispatch logic; a lazy slot's fast path lives in `interpreter/resolve.rs`'s `act_with_base`, next to the other `MethodFastPath` arms.
- Compiling a resolved module never raises a `NewspeakError`; a compiler-invariant violation is `panic!`/`debug_assert!`.
- An image is untrusted input: validate every id, tag, count and length before it indexes, allocates or slices; no byte sequence may panic.
- A wire tag is assigned once and never renumbered or reused. New tags take the next free number named in `crates/ns-runtime/src/image/opcodes.rs`; a removed one retires its number. `crates/ns-runtime/tests/opcode_tags.rs` pins each value.
- Program outcomes are pinned: a golden's recorded `status`, `stdout` and `stderr` change only where this plan names the change and says why.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`). Every commit message ends with a `Co-Authored-By:` trailer naming the model that actually wrote the code (for example `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`), a subagent's commits included.
- Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (warning-free), `cargo test --workspace --exclude ns-lsp`, `cargo run -p ns-cli -- parse-corpus corpus/modules` reporting `parsed 210, failed 0, expected-fail 1, stale 0`. A commit that touches `crates/ns-syntax` also runs `cargo test -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`.
- Design documents are living: each task updates the document for the aspect it changed, in place, describing the current state.

## Review Focus

1. **A lazy slot whose initializer answers `nil`** — §6.3.3 says the initializer runs whenever the slot "contains nil", so every read runs it again. Expect the initializer's side effect once per read. Pinned in Task 3 (`goldens/spec/spec_6_3_3_a_lazy_slot_that_answers_nil_runs_again.ns`).
2. **A lazy slot whose initializer does not complete** — it signals an error that a handler returns from, or it non-locally returns. The slot must stay `nil` and the next read must run the initializer again, rather than storing a half value. Pinned in Task 3 (`goldens/spec/spec_6_3_3_an_abandoned_initializer_leaves_the_slot_empty.ns`).
3. **An image that keeps `platform` itself and forces a platform member only after the load** — `platform collections` must then build in the loading process and answer the same object a path-stored `platform collections` answers. Pinned in Task 5 (`a_stored_platform_and_a_stored_member_agree_after_a_load`).
4. **A crafted platform path that names something a program may not reach or that is no shipped object** — a private member, a keyword or binary selector, a path longer than the bound, or a unary send that answers an ordinary object (`transcript printString`). Each must refuse the image, never run past it. Pinned in Task 5.
5. **A user class with a lazy slot saved in an image, filled and unfilled** — the decoded class must keep the slot lazy (an unfilled one still computes on first read, a filled one answers its value without running the initializer again). Pinned in Task 4.

---

## File Structure

- `crates/ns-syntax/src/ast.rs` — `SlotDeclaration` gains `lazy: bool`.
- `crates/ns-syntax/src/parser/declaration.rs` — the class body's lazy slot keeps its modifier.
- `crates/ns-runtime/src/resolver/mod.rs` — a lazy slot's initializer is resolved in the class body's scope, without the factory's parameters.
- `crates/ns-runtime/src/mixin.rs` — `MixinSlot.lazy`, `MethodFastPath::LazySlot`, `Mixin.shipped`, `slot_accessors` and `declared_slots_of` aware of lazy slots, `MixinMethod::lazy_slot_getter`.
- `crates/ns-runtime/src/compiler/mod.rs`, `lower.rs`, `reference.rs` — the lazy getter's body under both lowerings; the instance initializer skips lazy slots.
- `crates/ns-runtime/src/interpreter/instantiate.rs` — `build_methods` adds each lazy slot's getter; `mixin_of` records `shipped`.
- `crates/ns-runtime/src/interpreter/resolve.rs` — `act_with_base`'s `LazySlot` arm.
- `crates/ns-runtime/src/interpreter/mod.rs` — `push_step`'s method arm gives a lazy getter its slot base.
- `crates/ns-runtime/src/heap.rs` — `Heap::class_own_slot_base`, `Heap::class_is_shipped`.
- `crates/ns-runtime/src/image/opcodes.rs` — slot-kind tags, `VALUE_PLATFORM_PATH`, `CLASS_PLATFORM_PATH`, `HASHED_PLATFORM_PATH`, the retired builtin class tags.
- `crates/ns-runtime/src/image/mixins.rs` — lazy slots on the wire; the decoded getter's fast path.
- `crates/ns-runtime/src/image/platform_paths.rs` (new) — discovering the paths from the platform, and their section's encoding and decoding.
- `crates/ns-runtime/src/image/encode.rs`, `decode.rs`, `mod.rs`, `inspect.rs` — the paths in the graph, their resolution on load.
- `crates/ns-runtime/core-library/Platform.ns` (new) — `Platform`, `Transcript`, `Collections`, `KernelNamespace`.
- `crates/ns-runtime/src/interpreter/core_library.rs` (new) — the table of shipped modules and the platform's boot.
- `crates/ns-runtime/core-library/Kernel.ns`, `src/types.rs`, `src/interpreter/kernel.rs`, `src/platform.rs`, `src/compiler/primitive_method.rs` — the four classes leave the kernel.
- `goldens/spec/`, `goldens/kernel/` — the goldens named per task.
- `CLAUDE.md`, `docs/design/*.md`, `docs/PROGRESS.md` — per task, and the measurement in Task 7.

---

### Task 1: The parser keeps `lazy`

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs:62-68`
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`parse_slot_declarations` around line 222, `parse_lazy_slot_declaration` at 748, the doc comment at 53, the tests at 1222-1260)
- Modify: `crates/ns-runtime/src/resolver/scope.rs:124-130`, `crates/ns-runtime/tests/mixin.rs:19`, `crates/ns-runtime/tests/vm_initializer.rs:178` (every other `SlotDeclaration { … }` literal; find them with `grep -rn "SlotDeclaration {" crates`)
- Modify: `crates/ns-syntax/tests/snapshots/parse_snapshot__hello_world_ast.snap`
- Modify: `docs/design/frontend.md:67-70`

**Interfaces:**
- Produces: `ns_syntax::ast::SlotDeclaration { access, name, mutability, initializer, lazy: bool, span }`. `lazy` is `true` only for a `lazySlotDecl` in a class body (grammar line 189); a header slot and a temporary are never lazy.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-syntax/src/parser/declaration.rs`, extend the two existing lazy tests and add one for a header slot:

```rust
    #[test]
    fn lazy_slot_in_g2() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( lazy foo = bar baz. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].name.name, "foo");
        assert_eq!(body.slots[0].mutability, SlotMutability::Immutable);
        assert!(body.slots[0].lazy, "a class body's `lazy` slot is lazy (§6.3.3)");
    }

    #[test]
    fn lazy_slot_modifier_first_in_g2() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( public lazy foo ::= bar. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].access, Some(AccessModifier::Public));
        assert_eq!(body.slots[0].mutability, SlotMutability::MutableInitialized);
        assert!(body.slots[0].lazy);
    }

    #[test]
    fn a_header_slot_is_never_lazy() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = ( | foo = 1. | ) ( ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert!(!body.slots[0].lazy);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-syntax lazy_slot a_header_slot_is_never_lazy`
Expected: compile error, `no field `lazy` on type `SlotDeclaration``.

- [ ] **Step 3: Add the field and set it**

In `crates/ns-syntax/src/ast.rs`:

```rust
pub struct SlotDeclaration {
    pub access: Option<AccessModifier>,
    pub name: Identifier,
    pub mutability: SlotMutability,
    pub initializer: Option<Expression>,
    /// §6.3.3: declared `lazy` in a class body -- initialized on first read
    /// of its getter, not by the instance initializer. Never set on a header
    /// slot or a temporary, which the grammar gives no `lazy` form.
    pub lazy: bool,
    pub span: Span,
}
```

In `parse_lazy_slot_declaration`, construct with `lazy: true`; in `parse_slot_declarations` (header slots and temporaries) with `lazy: false`. Update the doc comment at line 53 so it no longer says `lazy` is dropped "as it is for class slots": a temporary's `lazy` is still dropped, a class body's is kept. Add `lazy: false` to every other `SlotDeclaration { … }` literal the compiler reports.

- [ ] **Step 4: Update the AST snapshot and the grammar note**

Run: `cargo insta test -p ns-syntax --accept` if `cargo-insta` is installed, otherwise `INSTA_UPDATE=always cargo test -p ns-syntax` (Git Bash) and inspect the diff: the only change must be `lazy: false,` lines in `parse_snapshot__hello_world_ast.snap`.

In `docs/design/frontend.md`, replace the sentence at lines 67-70 ("A lazy slot declaration on the instance side … a deviation from §6.3.3's …") with: a lazy slot declaration on the instance side (`lazy m ::= e.`, §6.3.3) parses into a `SlotDeclaration` whose `lazy` is set; the runtime initializes it on first read (`docs/design/core-library.md`, "Lazy slots first").

- [ ] **Step 5: Run the tests and the gates**

Run: `cargo test -p ns-syntax` — Expected: PASS.
Then the per-commit gates (Global Constraints), plus `cargo test -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`, since this touches `crates/ns-syntax`. The runtime still treats every slot as eager, so every golden answers as before.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-syntax crates/ns-runtime/src/resolver/scope.rs crates/ns-runtime/tests/mixin.rs crates/ns-runtime/tests/vm_initializer.rs docs/design/frontend.md
git commit -m "feat: the parser keeps a class body slot's lazy modifier

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 2: A lazy slot's initializer is resolved without the factory's parameters

**Files:**
- Modify: `crates/ns-runtime/src/resolver/mod.rs:180-190` (the loop over `declaration.instance_side.slots`) and the method-resolution path it borrows from (`resolve_method`, around line 229)
- Test: `crates/ns-runtime/tests/resolver.rs`

**Interfaces:**
- Consumes: `SlotDeclaration.lazy` (Task 1).
- Produces: bindings for a lazy slot's initializer computed in the scope a unary method of the class has — `self` is the instance, no local but the initializer's own, no factory parameter. The compiler (Task 3) lowers that initializer as a zero-argument method body.

- [ ] **Step 1: Write the failing test**

```rust
/// §6: the primary factory's parameters "are in scope in the instance
/// initializer but not in the rest of the class body", and a lazy slot is
/// declared in the class body (§6.3.3), so `seed` in its initializer is the
/// implicit-receiver send `self seed`, not the factory's parameter.
#[test]
fn a_lazy_slot_initializer_does_not_see_the_factory_parameters() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo seed: seed = Object new (
            | eager = seed. |
        ) (
            lazy public later = seed.
        ) : ()
    "};
    let resolved = resolve_source(source);
    let eager = source.find("= seed.").expect("the eager initializer") + 2;
    let later = source.rfind("= seed.").expect("the lazy initializer") + 2;
    let binding_at = |start: usize| {
        resolved
            .bindings
            .iter()
            .find(|(span, _)| span.start as usize == start)
            .map(|(_, binding)| binding.clone())
    };
    assert!(matches!(binding_at(eager), Some(Binding::Local(_))));
    assert!(matches!(binding_at(later), Some(Binding::LexicalSend { depth: 0 })));
}
```

If `Span`'s start field or `Binding`'s `Clone` differ from what this assumes, adapt the lookup to them, not the assertion.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p ns-runtime --test resolver a_lazy_slot_initializer_does_not_see_the_factory_parameters`
Expected: FAIL — the lazy initializer's `seed` is `Binding::Local(0)`, because lazy slots are resolved with the header's slots today.

- [ ] **Step 3: Resolve lazy initializers in the class body's scope**

In the loop at `resolver/mod.rs:186`, skip `slot.lazy` slots. After the instance initializer's scope is popped, resolve each lazy slot's initializer the way `resolve_method` resolves a unary method's body: a fresh `ScopeStack` with one `LocalScope::new(&[], &[])` and the same `class_chain`. Comment the decision with the §6 sentence the test quotes.

- [ ] **Step 4: Run the test and the resolver suite**

Run: `cargo test -p ns-runtime --test resolver`
Expected: PASS. Then the per-commit gates. No golden declares a lazy slot that reads a factory parameter, so every golden answers as before.

- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/resolver/mod.rs crates/ns-runtime/tests/resolver.rs
git commit -m "fix: resolve a lazy slot's initializer without the factory's parameters

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 3: Lazy slots are initialized on first read

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinSlot` at 196, `MethodFastPath` at 113, `slot_accessors` at 336, `declared_slots_of` at 736, `MixinMethod` constructors near 160)
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (`compile_mixin_initializer`, a new `compile_lazy_slot_getter`), `crates/ns-runtime/src/compiler/lower.rs` (next to `lower_slot_initializer` at 466), `crates/ns-runtime/src/compiler/reference.rs` (`initializer` at 51, a new `lazy_slot_getter`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`build_methods` at 40)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`act_with_base`, the `match method.fast_path` at 359)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`push_step`'s `Step::Method` arm around 1087; `apply_mixin`'s `declared_slots_of` call at 1764)
- Modify: `crates/ns-runtime/src/heap.rs` (a new `class_own_slot_base`)
- Modify: `goldens/spec/spec_6_3_3_a_lazy_slot_is_initialized_on_first_use.toml` (delete the `skipped` line) and its `.ns` comment
- Create: the goldens listed in Step 1
- Test: `crates/ns-runtime/src/builtins/value.rs` (tests module)
- Modify: `docs/design/README.md` (the deviations table and the paragraphs around it), `docs/design/object-model.md` (slots), `docs/design/dispatch.md` (slot accessors / fast paths), `docs/design/core-library.md` ("Lazy slots first" becomes a description of what exists)

**Interfaces:**
- Consumes: `SlotDeclaration.lazy` (Task 1); lazy initializer bindings (Task 2).
- Produces:
  - `MixinSlot { name, mutable, access, lazy: bool, declaration }`.
  - `MethodFastPath::LazySlot { own_index: u32 }`.
  - `pub fn slot_accessors(slots: &[MixinSlot]) -> Vec<(Symbol, MethodEntry)>` — a lazy slot contributes its setter when mutable and **no** getter entry; its getter is a compiled method.
  - `pub fn declared_slots_of(superclass: Option<&Rc<MixinApplication>>, own_slot_base: usize, own_slots: &[MixinSlot]) -> Vec<usize>` — skips lazy slots.
  - `MixinMethod::lazy_slot_getter(slot: &MixinSlot, own_index: u32, code: Rc<CompiledCode>) -> MixinMethod` — selector the slot's name, the slot's access, `argument_count` 0, `declaration: None`, `fast_path: Some(MethodFastPath::LazySlot { own_index })`.
  - `pub(crate) fn compile_lazy_slot_getter(initializer: &ast::Expression, own_index: u32, span: Span, bindings: &Rc<HashMap<Span, Binding>>, lowering: Lowering) -> CompiledCode` in `compiler/mod.rs`, dispatching to `reference::compile_lazy_slot_getter` under `Lowering::Reference`.
  - `Heap::class_own_slot_base(&self, class: ClassReference<'gc>) -> Option<usize>`.

- [ ] **Step 1: Write the failing goldens**

Delete the `skipped = …` line of `goldens/spec/spec_6_3_3_a_lazy_slot_is_initialized_on_first_use.toml` and the "Skipped until …" sentences of its `.ns` comment; its recorded `stdout` is the expectation.

Create `goldens/spec/spec_6_3_3_a_lazy_slot_that_answers_nil_runs_again.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.3: "if the slot contains nil, e is evaluated". An
   initializer that answers nil leaves the slot containing nil, so every
   read evaluates it again: three reads, three marks. *)
class Spec633LazyNil usingPlatform: p = Object new (| t = p transcript. |) (
    class Holder = Object new (| public trace ::= ''. |) (
        lazy public computed = mark.
        mark = ( trace:: trace, '!'. ^nil )
    ) : ()
    public main: platform args: arguments = (
        | holder |
        holder:: Holder new.
        holder computed. holder computed. holder computed.
        t show: holder trace; newLine.
    )
) : ()
```

with `spec_6_3_3_a_lazy_slot_that_answers_nil_runs_again.toml`:

```toml
stdout = '''
!!!
'''
```

Create `goldens/spec/spec_6_3_3_an_abandoned_initializer_leaves_the_slot_empty.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6.3.3 sets the slot to the object e evaluated to. An
   initializer that signals, and whose exception a handler returns from,
   evaluated to nothing: the slot stays nil and the next read evaluates it
   again. The second read's attempt succeeds and is kept. *)
class Spec633LazyAbandoned usingPlatform: p = Object new (|
    t = p transcript.
    Error = p kernel Error.
|) (
    class Holder failing: errorClass = Object new (|
        public attempts ::= 0.
        ErrorClass = errorClass.
    |) (
        lazy public computed = attempt.
        attempt = (
            attempts:: attempts + 1.
            attempts = 1 ifTrue: [ErrorClass new signal: 'first attempt fails'].
            ^attempts
        )
    ) : ()
    public main: platform args: arguments = (
        | holder |
        holder:: Holder failing: Error.
        t show: ([holder computed] on: Error do: [:error | error return: 0]) printString; newLine.
        t show: holder computed printString; newLine.
        t show: holder computed printString; newLine.
        t show: holder attempts printString; newLine.
    )
) : ()
```

with its `.toml`:

```toml
stdout = '''
0
2
2
2
'''
```

Create `goldens/kernel/a_values_lazy_slot_is_not_part_of_its_state.ns`:

```newspeak
Newspeak3
'Test'
(* Decided with the user on 2026-09-26 (docs/design/core-library.md, "Lazy
   slots first"): a value's state-derived = and hash skip lazy slots, which
   are state derived from the rest -- Bracha's transient fields, purged on
   synchronization. Two values of one class, one of which has filled its
   lazy slot, are equal and hash alike. *)
class LazyValueState usingPlatform: p = Object <: Value () (
    class Point x: anX = Value new (| public x = anX. |) (
        lazy public doubled = x + x.
    ) : ()
    class Script on: aTranscript = Object new (| t = aTranscript. |) (
        public run = (
            | first second |
            first:: Point x: 3.
            second:: Point x: 3.
            first doubled.
            t show: (first = second) printString; newLine.
            t show: (first hash = second hash) printString; newLine.
            t show: (first = (Point x: 4)) printString; newLine.
        )
    ) : ()
    public main: platform args: arguments = (
        (Script on: platform transcript) run
    )
) : ()
```

with its `.toml`:

```toml
stdout = '''
true
true
false
'''
```

Create `goldens/spec/spec_6_a_lazy_slot_cannot_name_a_factory_parameter.ns`:

```newspeak
Newspeak3
'Test'
(* newspeak-spec.pdf 6: the primary factory's parameters "are in scope in
   the instance initializer but not in the rest of the class body", and a
   lazy slot is declared in the class body. So `seed` in its initializer is
   the implicit-receiver send `self seed`, which nothing understands. *)
class Spec6LazyFactory usingPlatform: p = Object new () (
    class Holder seed: seed = Object new () (
        lazy public later = seed.
    ) : ()
    public main: platform args: arguments = (
        (Holder seed: 7) later
    )
) : ()
```

Run it once the implementation is in and record what it prints as its `.toml` **only after** checking the text is the ordinary `doesNotUnderstand:` report for `#seed` on a `Holder` — the same text `stderr` shows for any unknown selector in the neighbouring `goldens/errors/*.toml`, with `status = 1`.

- [ ] **Step 2: Run the goldens to see them fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL on the four new or unskipped goldens (the first reads `before: !`).

- [ ] **Step 3: The mixin model**

In `mixin.rs`:

```rust
pub struct MixinSlot {
    pub name: Symbol,
    /// §6.3.2: only a mutable slot has a setter.
    pub mutable: bool,
    pub access: ast::AccessModifier,
    /// §6.3.3: initialized by its getter on first read rather than by the
    /// instance initializer. Its getter is a compiled method
    /// (`MixinMethod::lazy_slot_getter`), not an accessor entry, and a
    /// value's state skips it (`declared_slots_of`).
    pub lazy: bool,
    pub declaration: Option<Rc<ast::SlotDeclaration>>,
}
```

`MixinSlot::from_declaration` sets `lazy: declaration.lazy`.

`slot_accessors` pushes the getter only for `!slot.lazy`; the setter rule is unchanged.

Add the fast path, documented like its neighbours:

```rust
    /// A lazy slot's getter (§6.3.3): answer the slot's field -- the
    /// defining application's `own_slot_base` plus `own_index` -- without an
    /// activation when it holds anything but `nil`; activate the method,
    /// which evaluates the initializer and stores it, when it holds `nil`.
    LazySlot { own_index: u32 },
```

Add the constructor next to `MixinMethod::from_declaration`:

```rust
    /// The getter a lazy slot declares (§6.3.3): a method of the slot's name
    /// and accessibility whose `code` evaluates the initializer, stores it
    /// with `InitializeSlot` and answers it
    /// (`compiler::compile_lazy_slot_getter`).
    pub fn lazy_slot_getter(slot: &MixinSlot, own_index: u32, code: Rc<CompiledCode>) -> MixinMethod {
        MixinMethod {
            selector: slot.name,
            access: slot.access,
            argument_count: 0,
            declaration: None,
            code,
            fast_path: Some(MethodFastPath::LazySlot { own_index }),
        }
    }
```

Change `declared_slots_of` to take the level's slots and skip the lazy ones, extending the doc comment's "This is what a value's state is" paragraph with the 2026-09-26 decision:

```rust
pub fn declared_slots_of(
    superclass: Option<&Rc<MixinApplication>>,
    own_slot_base: usize,
    own_slots: &[MixinSlot],
) -> Vec<usize> {
    let mut declared =
        superclass.map_or_else(Vec::new, |superclass| superclass.declared_slots.clone());
    declared.extend(
        own_slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.lazy)
            .map(|(position, _)| own_slot_base + position),
    );
    declared
}
```

and pass `&mixin.slots` at the call in `interpreter/mod.rs:1765` (and at any other caller the compiler reports).

- [ ] **Step 4: The getter's body, under both lowerings**

In `compiler/lower.rs`, beside `lower_slot_initializer`:

```rust
    /// A lazy slot's getter (§6.3.3): the initializer, then `InitializeSlot`
    /// -- "the slot's value is set to o (but not by means of a setter
    /// method)" -- then `Return` of the same register, "and o is returned as
    /// the result of the send". The activation's `slot_base` is the defining
    /// application's, which `push_step` gives a method whose fast path is
    /// `LazySlot`.
    pub(crate) fn lower_lazy_slot_getter(&mut self, initializer: &Expression, own_index: u32) {
        self.next_scratch = self.base;
        let start = self.instructions.len();
        let source = self.lower_expression_for(initializer, ValueUse::Read);
        self.emit(Instruction::InitializeSlot { source, own_index });
        self.emit(Instruction::Return { register: source });
        let end = self.instructions.len();
        self.statement_program_counter
            .push((self.statement_index, start..end));
        self.statement_index += 1;
    }
```

In `compiler/mod.rs`, a `compile_lazy_slot_getter` shaped like `compile_superclass_clause`: `capture::analyze` over a `RootBody` with `argument_count: 0`, no temporaries, `slot_initializers: vec![initializer]` and no statements; a `Lowerer::new(CodeKind::Method, BodyKey::Root, 0, &[], bindings, captures, false, Enclosing::default(), 0)`; `lower_lazy_slot_getter`; `finish(span)`. Under `Lowering::Reference` it calls `reference::compile_lazy_slot_getter`, which mirrors `reference::initializer`'s per-slot step (`body.value(initializer)`, `InitializeSlot`, `record_statement`) and then emits `Instruction::Return { register: source }` before `body.finish(span)`.

In `compile_mixin_initializer` (both lowerings, `compiler/mod.rs:161` and `reference.rs:59`), skip slots whose `lazy` is set: filter the `slot_initializers` list the same way, so the capture analysis and the lowering see the same expressions.

In `instantiate.rs`'s `build_methods`, after the declared methods, add one getter per lazy slot:

```rust
    for (position, slot) in slots.iter().enumerate() {
        if !slot.lazy {
            continue;
        }
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        let Some(initializer) = &declaration.initializer else {
            unreachable!("internal: the grammar gives every lazy slot an initializer");
        };
        let own_index = u32::try_from(position).expect("a mixin's slot count fits a u32");
        let code = crate::compiler::compile_lazy_slot_getter(
            initializer,
            own_index,
            declaration.span,
            bindings,
            lowering,
        );
        let getter = MixinMethod::lazy_slot_getter(slot, own_index, Rc::new(code));
        let previous = methods.insert(getter.selector, MethodEntry::Compiled(Rc::new(getter)));
        debug_assert!(previous.is_none(), "the resolver refuses a duplicate member");
    }
```

- [ ] **Step 5: The fast path and the slot base**

In `heap.rs`, next to `class_application`:

```rust
    /// Where `class`'s own slots start in an instance's fields
    /// (`MixinApplication::own_slot_base`), copied out; `None` for an
    /// object that is not a class object.
    pub fn class_own_slot_base(&self, class: ClassReference<'gc>) -> Option<usize> { … }
```

(Read the application the way `class_application` does, without cloning the `Rc`.)

In `resolve.rs`'s `act_with_base`, add the arm beside `Receiver`:

```rust
                        // A filled lazy slot answers its field without an
                        // activation; an empty one -- `nil`, which §6.3.3
                        // reads as "not yet computed" -- activates the
                        // getter, which computes and stores it.
                        Some(MethodFastPath::LazySlot { own_index }) => {
                            if let (Value::Object(object), Some(base)) =
                                (receiver, self.heap().class_own_slot_base(defining_class))
                            {
                                if let Some(value) = self
                                    .heap()
                                    .instance_field(object, base + own_index as usize)
                                {
                                    if !matches!(value, Value::Nil) {
                                        return Ok(Step::Value(value));
                                    }
                                }
                            }
                        }
```

In `interpreter/mod.rs`'s `push_step`, `Step::Method` arm, replace `slot_base: 0, // a method body never contains InitializeSlot` with a slot base read only for a lazy getter:

```rust
                // Only a lazy slot's getter contains `InitializeSlot`, and it
                // writes its defining application's field.
                let slot_base = match method.fast_path {
                    Some(MethodFastPath::LazySlot { .. }) => self
                        .heap
                        .class_own_slot_base(defining_class)
                        .expect("internal: a method's defining class is a class object"),
                    _ => 0,
                };
```

Check `instantiate::non_value_slot_error`, which `InitializeSlot` calls when a value's slot is handed a non-value: it must name the slot from a method activation too (it may read the activation's `FrameKind::Initializer` today). If it does, take the level from the activation's `current_class` instead, so that a lazy slot of a value class filled with a non-value reports the slot by name.

- [ ] **Step 6: A unit test for the value state**

In `crates/ns-runtime/src/builtins/value.rs`'s tests, next to `a_nested_classs_cache_slot_is_not_part_of_a_values_state`, add `a_lazy_slot_is_not_part_of_a_values_state`: build a value class with one ordinary slot and one lazy slot, force the lazy slot on one instance only, and assert that `valueSlotCount:ifFail:` answers 1 for both and that the equality leaf answers `true`. Follow the neighbouring test's fixture helpers.

- [ ] **Step 7: Run everything**

Run: `cargo test --workspace --exclude ns-lsp`
Expected: PASS, including the goldens from Step 1 under both lowerings. Record the `.toml` of `spec_6_a_lazy_slot_cannot_name_a_factory_parameter` as Step 1 says. Then the per-commit gates.

- [ ] **Step 8: Update the documents**

- `docs/design/README.md`: remove the "Lazy slots are eager" row; the paragraph above the table then names three deviations that answer a correct program wrongly, not four; the paragraph below says one golden under `goldens/spec/` is `skipped`, not two.
- `docs/design/object-model.md` and `docs/design/dispatch.md`: where slots, their accessors and the method fast paths are described, add the lazy slot: its getter is a compiled method with `MethodFastPath::LazySlot`, its setter an ordinary accessor, its field outside a value's declared state.
- `docs/design/core-library.md`, "Lazy slots first (plan 1)": state that it is built.

- [ ] **Step 9: Commit**

```bash
git add crates/ns-runtime goldens docs
git commit -m "feat: initialize a lazy slot on first read (spec 6.3.3)

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 4: Lazy slots in the image

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (new slot-kind tags)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (`mutability_tag` at 196, its decoder at 203, the slot loop at 251, `RawSlot` at ~430, `check_member_names` at ~600, `build` at ~715)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`DecodeError`)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Test: `crates/ns-runtime/tests/image.rs`
- Modify: `docs/design/image-format.md` (the mixin table's slot record)

**Interfaces:**
- Consumes: `MixinSlot.lazy`, `MethodFastPath::LazySlot`, `MixinMethod::lazy_slot_getter` (Task 3).
- Produces: `opcodes::SLOT_IMMUTABLE = 0`, `SLOT_MUTABLE = 1`, `SLOT_LAZY_IMMUTABLE = 2`, `SLOT_LAZY_MUTABLE = 3`; `DecodeError::LazySlotWithoutGetter(String)`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/image.rs`, following the file's existing snapshot/load round-trip helpers:

- `a_lazy_slot_survives_an_image_filled_and_unfilled` — a module whose root holds two instances of a class with `lazy public computed = mark.` and a `trace` slot; the first instance has been read once before the snapshot, the second not. After the load, reading both through `computed` answers the value, and the first's trace still carries one mark while the second's gains its first. (Review Focus 5.)
- `a_lazy_slot_without_its_getter_is_refused` — encode such a module, then patch the bytes so the lazy slot's getter row is missing (the file has helpers for crafted images; use the one that rewrites the mixin table, or build the bytes with `Writer` as the neighbouring crafted-mixin tests do) and assert `image::load` answers an error naming `LazySlotWithoutGetter`, without a panic.

In `crates/ns-runtime/tests/opcode_tags.rs`, pin the four `SLOT_*` values.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test image lazy_slot` and `cargo test -p ns-runtime --test opcode_tags`
Expected: FAIL — the constants do not exist, and a decoded lazy slot comes back eager with no getter.

- [ ] **Step 3: The slot kind on the wire**

Declare in `opcodes.rs`, with a doc comment that says the four are a slot's accessor shape — the two eager ones are the old mutability tags, unchanged — and "Next free number: 4":

```rust
pub const SLOT_IMMUTABLE: u8 = 0;
pub const SLOT_MUTABLE: u8 = 1;
pub const SLOT_LAZY_IMMUTABLE: u8 = 2;
pub const SLOT_LAZY_MUTABLE: u8 = 3;
```

In `mixins.rs`, replace `mutability_tag`/`mutability_from_tag` with `slot_kind_tag(mutable: bool, lazy: bool) -> u8` and `slot_kind_from_tag(tag: u8) -> Result<(bool, bool), DecodeError>` over those constants (unknown tag: `DecodeError::InvalidTag { what: "slot kind", tag }`). `RawSlot` gains `lazy: bool`.

In `check_member_names`, a lazy slot contributes no getter name of its own — its getter is one of the row's methods — and a mutable one still contributes its setter.

In `build`, set `lazy` on each decoded `MixinSlot`. After the compiled methods are built, for each lazy slot at position `own_index`, find the row's method of the slot's name; refuse the row with `DecodeError::LazySlotWithoutGetter(slot name)` unless it exists, takes no argument and has the slot's access; and give that method `fast_path: Some(MethodFastPath::LazySlot { own_index })` in place of the `None` every other decoded method keeps. Update the comment above `fast_path: None` accordingly. Run this check where `build` can still answer an error — if `build` cannot fail today, do it in the verification pass `decode::verify_mixins` runs over the decoded rows.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime --test image` and `--test opcode_tags`
Expected: PASS. Then the per-commit gates.

- [ ] **Step 5: Document and commit**

In `docs/design/image-format.md`, where the mixin table's slot record is described, replace the mutability byte with the slot kind and its four values, and say that a lazy slot's getter is one of the row's methods whose fast path the decoder restores from the slot.

```bash
git add crates/ns-runtime docs/design/image-format.md
git commit -m "feat: carry lazy slots through the image

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 5: Platform paths in the image

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`Mixin.shipped`, `Mixin::empty`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`mixin_of`, `build_class_side`: set `shipped` from their `kernel` argument)
- Modify: `crates/ns-runtime/src/heap.rs` (`class_is_shipped`)
- Create: `crates/ns-runtime/src/image/platform_paths.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`, `mod.rs` (`load`, `snapshot`, `build`), `inspect.rs`
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-cli/tests/snapshot_load.rs`
- Modify: `docs/design/image-format.md`

**Interfaces:**
- Consumes: lazy slots (Task 3), so the walk reads a lazy slot's field like any other and skips it while it is `nil`.
- Produces:
  - `Mixin.shipped: bool` — compiled from shipped core-library source (`ResolvedModule.kernel`), set on the declaration's mixin, its nested mixins and its class side. A decoded mixin is never shipped.
  - `Heap::class_is_shipped(&self, class: ClassReference<'gc>) -> bool`.
  - `opcodes::VALUE_PLATFORM_PATH = 8`, `CLASS_PLATFORM_PATH = 5`, `HASHED_PLATFORM_PATH = 4`; each followed by a `u32` index into the image's platform path section.
  - `image::platform_paths::discover(heap: &Heap<'gc>, platform: ObjectReference<'gc>) -> PlatformPaths<'gc>`; `PlatformPaths::id_of(&self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<u32>`; `PlatformPaths::path(&self, id: u32) -> &[Symbol]`.
  - `image::platform_paths::MAX_PLATFORM_PATHS: usize = 4096`, `MAX_PLATFORM_PATH_LENGTH: usize = 32`.

**The rule** (`docs/design/core-library.md`, "Platform paths in the image"): a path is the list of public members sent from the platform, one unary send each. `discover` walks from the platform over, for every level of an object's class, each **public** slot's field — lazy slots included, skipped while `nil` — and each **public** nested class's cache slot (`MixinApplication::nested_slot`), skipped while empty. It records every object it reaches that is a class object or an instance of a shipped class, and descends into instances of shipped classes only, never into a class object. Breadth-first, visiting each object once, so an object's recorded path is its shortest, and among equals the first in declaration order, which keeps the encoding canonical.

Today that finds `transcript`, `collections` and `kernel` (the kernel-built platform's public slots), and after Task 6 also `Transcript`, `Collections` and `KernelNamespace` once their accessors have run.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/image.rs`:

- `a_platform_member_is_named_by_its_path` — snapshot a module whose root holds `platform transcript` and `platform collections` in two slots; assert that `ns-next inspect`'s structural walk (or the image module's own reader, whichever the neighbouring tests use) lists a platform path section with `transcript` and `collections` and no object record of either; load it and assert that each slot's value is identical (`same`) to what the loaded interpreter's own `platform transcript` and `platform collections` answer.
- `a_stored_platform_and_a_stored_member_agree_after_a_load` — the root keeps `platform` in one slot and `platform collections` in another. After the load, sending `collections` to the first slot's value answers the object in the second. (Review Focus 3.)
- `a_platform_path_the_loader_cannot_follow_is_refused` — craft images whose path section names, one per image: a member that does not exist (`nonexistent`), a keyword selector (`show:`), a path of `MAX_PLATFORM_PATH_LENGTH + 1` members, a count of `MAX_PLATFORM_PATHS + 1`, a member that is no symbol id, and `transcript printString`, whose unary send answers a `String`, which is no shipped object. Each load answers an error and none panics. (Review Focus 4.)
- `an_index_past_the_path_section_is_refused` — a `VALUE_PLATFORM_PATH` whose index is past the section's count.

In `crates/ns-cli/tests/snapshot_load.rs`, the existing tests that keep the transcript in a slot keep passing unchanged. They already pin the output a loaded image prints through it.

In `opcode_tags.rs`, pin the three new tags.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test image platform_path` and `--test opcode_tags`
Expected: FAIL — the section and tags do not exist; today the transcript is an object record of builtin class 18.

- [ ] **Step 3: Provenance**

`Mixin.shipped` (documented as above), `false` in `Mixin::empty`, set in `instantiate.rs`'s `mixin_of` and `build_class_side` from their `kernel` argument, which `ResolvedModule.kernel` feeds. `Heap::class_is_shipped` answers the flag of the class's application's mixin, and `false` for anything else.

- [ ] **Step 4: Discovery and the section**

In `image/platform_paths.rs`, `PlatformPaths<'gc> { paths: Vec<Vec<Symbol>>, objects: Vec<ObjectReference<'gc>>, ids: HashMap<usize, u32> }`, keyed by `heap.object_identity_key`, and `discover` as the rule says. Also:

- `pub(crate) fn encode_section(writer: &mut Writer, used: &[Vec<Symbol>], symbols: &mut symbols::Builder)` — a `u32` count, then per path a `u32` member count and that many symbol ids.
- `pub(crate) fn decode_section(reader: &mut Reader, symbols: &DecodedSymbols) -> Result<Vec<Vec<Symbol>>, DecodeError>` — refuses a count above `MAX_PLATFORM_PATHS`, a member count of 0 or above `MAX_PLATFORM_PATH_LENGTH`, an unknown symbol id, and a member that is not a unary selector (no `:`, and starting with a letter). Use the exact reader and symbol-table types `decode.rs` uses; add the `DecodeError` variants this needs, each naming what it refuses.

Only the paths the graph **uses** are written, in the order the graph table first reaches them, and renumbered densely.

- [ ] **Step 5: The encoder**

- `GraphTable::build` takes the `PlatformPaths`. `visit_value` returns early for an object `id_of` finds, recording it as used, as it does for the platform itself.
- After that check, it refuses — through a field like `unnameable`, which `encode_graph_section` turns into its error — a class object whose class is shipped and which no builtin tag names, and an instance of a shipped class whose class object has no enclosing object, which makes it a module instance. The message says what was reached and that no platform path names it.
- `encode_value` writes `VALUE_PLATFORM_PATH` and the used index for a path-named object, before its class-tag branch. `encode_class_tag` writes `CLASS_PLATFORM_PATH` and the index for a path-named class, before `CLASS_BUILTIN`. `CLASS_METACLASS` may be followed by a `CLASS_PLATFORM_PATH` reference: a path-named class is prebuilt as far as the image is concerned, so `is_prebuilt_class` answers `true` for it. `encode_tag_named_hashes` writes a `HASHED_PLATFORM_PATH` entry for each used path whose object has an assigned hash, after the platform's, in used order, and the metaclass entries cover path-named classes too.
- The section is written only in a stateful image, immediately before the graph section.

- [ ] **Step 6: The loader**

In `image/mod.rs`'s `load`, split the single `interpreter.enter` into three phases:

1. Decode everything up to and including the entry and the platform path section, and stash `root_class` and each root level as `Root`s. Mixins and code hold no handles.
2. Outside any entry, resolve each path by `interpreter.send` from `interpreter.platform()`, one member at a time. A failed send refuses the image with a message naming the path. So does a final answer that is not a `Value::Object` whose class is shipped (`class_is_shipped`), or that is not a class object whose own mixin is shipped.
3. Enter again, fetch the roots and the resolved objects, and decode the graph with them in a new `DecodeContext` field `platform_paths: &[Value<'gc>]`.

`VALUE_PLATFORM_PATH` and `CLASS_PLATFORM_PATH` look their index up there, refusing an index past the end and a `CLASS_PLATFORM_PATH` whose object is not a class. `HASHED_PLATFORM_PATH` restores the hash like `HASHED_PLATFORM` does.

An `OBJECT_INSTANCE` record whose class reference is a `CLASS_PLATFORM_PATH` must carry exactly the class's `slot_count` fields, or the image is refused (`docs/design/core-library.md`: the layout comes from the loading binary).

- [ ] **Step 7: `inspect`**

`inspect.rs`'s structural walk reads the new section and prints the three new references as `<platform path N: member member …>`.

- [ ] **Step 8: Run everything**

Run: `cargo test --workspace --exclude ns-lsp`, then `cargo run -p ns-cli -- run-corpus corpus/modules` (its built binaries carry no graph, so it must still report `ran 6, failed 0, skipped 205`).
Expected: PASS. Then the per-commit gates.

- [ ] **Step 9: Document and commit**

In `docs/design/image-format.md`:
- Add the platform path section to "Sections".
- Add the three tags where the value, class and hash tags are listed.
- Replace the "Planned" paragraph added on 2026-09-26 under "The `platform` singleton" with the description of what now exists.
- In "Verification", add the load-time resolution and the field-count check.

```bash
git add crates/ns-runtime crates/ns-cli docs/design/image-format.md
git commit -m "feat: name the objects an image reaches through the platform by their paths

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 6: `Platform.ns`, booted from the table of shipped modules

**Files:**
- Create: `crates/ns-runtime/core-library/Platform.ns`
- Create: `crates/ns-runtime/src/interpreter/core_library.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`bootstrap_with`, `platform_value` at 1460, `mod core_library;`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (remove `Platform`, `Transcript`, `Collections`, `KernelNamespace`; the `Object` accessor comment)
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass` and `BuiltinClass::ALL`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name`, `check_kernel_shape`'s slot allowance at 453, the tests at 990-1030, 1140-1160, 1375-1425)
- Modify: `crates/ns-runtime/src/compiler/primitive_method.rs:176-183`
- Modify: `crates/ns-runtime/src/platform.rs`
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`), `decode.rs` (`builtin_class_from_tag`), `opcodes.rs` (the retired numbers), `crates/ns-runtime/tests/opcode_tags.rs`
- Modify: every test the compiler or the suite reports: `grep -rn "BuiltinClass::\(Platform\|Transcript\|Collections\|KernelNamespace\)" crates` (among them `tests/platform.rs`, `tests/image.rs:1210`, `interpreter/mirror.rs`'s tests)
- Modify: `goldens/kernel/the_platform_and_its_parts_are_values.ns`
- Modify: `CLAUDE.md`, `docs/design/core-library.md` ("The platform", "The on-demand rule", "Where the library source lives, and how modules are found", the "Planned" section), `docs/design/README.md` (the "Builtins and the platform" row)

**Interfaces:**
- Consumes: lazy slots (Task 3), platform paths (Task 5).
- Produces:
  - `core_library::SHIPPED_MODULES: &[ShippedModule]`, `ShippedModule { name: &'static str, source: &'static str }`. Only shipped core-library source is in it today; user modules join it, flagged as such, with the step that loads them (`docs/design/core-library.md`).
  - `core_library::resolved_shipped_module(name: &str) -> Rc<ResolvedModule>`, parsed and resolved once per thread with `resolver::resolve_kernel`, panicking for a module that does not parse or resolve, as `interpreter::kernel::resolve_kernel_source` does.
  - `Interpreter::install_platform(&mut self)`, run by `bootstrap_with` right after `install_kernel`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/platform.rs`, replace every assertion that names `BuiltinClass::Platform`, `Transcript`, `Collections` or `KernelNamespace` with the property it was checking, stated through sends. Add:

- `the_platform_is_an_instance_of_the_shipped_platform_module` — `platform class name` answers `'Platform'`, and `platform transcript class name` answers `'Transcript'`.
- `the_platform_builds_its_members_on_first_use` — right after bootstrap, before any send to it, the platform's `transcript` field is `nil`. Read it through the heap accessors: the field of the slot named `transcript`, found through the platform class's application's mixin slots. After `platform transcript` it is not `nil`.
- `a_subclass_of_the_transcript_class_writes_too` — a module declaring `class Loud = (platform Transcript) new () ()`. If §6.1 needs the superclass clause to name the class differently, bind it through a slot first. Its instance answers `show:` without `doesNotUnderstand:`. This is the observable change the design names.

In `opcode_tags.rs`, assert that builtin class tags 13, 18, 19 and 29 are refused by `builtin_class_from_tag`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test platform --test opcode_tags`
Expected: FAIL.

- [ ] **Step 3: `Platform.ns`**

```newspeak
Newspeak3
'Platform'
(* The platform a module is handed (§8.3): a shipped core-library module,
   booted by the host with the VM mirror and the kernel instance
   (`docs/design/core-library.md`, "The module table and the boot").
   §8.3 describes the platform as an object that "provides accessors to
   various modules"; each is a lazy slot (§6.3.3), built on first use, so a
   program pays only for what it asks for, and an image names each by its
   path from the platform rather than carrying it.

   The factory's parameters are copied into slots because neither a lazy
   slot nor a method may name them (§6). A send to `vmMirror` here is a
   mirror send, as in `Kernel.ns`: this module is resolved with
   `resolve_kernel`, which only shipped source is.

   The platform and its members are values (§3.1.1), as SOMns's `Platform`
   is: stateless but for what the host hands them. *)
class Platform usingVmMirror: theMirror kernel: theKernel = Object <: Value (
    | private vmMirror = theMirror.
      private kernelModule = theKernel. |
) (
    public class Transcript = Value new () (
        public show: text = ( ^vmMirror transcript: self show: text ifFail: [^super show: text] )
        public newLine = ( ^vmMirror transcriptNewLine: self ifFail: [^super newLine] )
    )
    (* The Core Library's `collections` namespace, whose one member is the
       one a whitelisted corpus module asks for (`docs/design/core-library.md`,
       "The on-demand rule"). *)
    public class Collections = Value new () (
        public List = ( ^OrderedCollection )
    )
    (* `platform kernel`: the kernel classes a program may name, as
       Primordial Soup's `Kernel wrapping:` facet answers them. *)
    public class KernelNamespace = Value new () (
        public Exception = ( ^kernelModule Exception )
        public Error = ( ^kernelModule Error )
        public MessageNotUnderstood = ( ^kernelModule MessageNotUnderstood )
        public Halt = ( ^kernelModule Halt )
        public Message = ( ^kernelModule Message )
    )
    lazy public transcript = Transcript new.
    lazy public collections = Collections new.
    lazy public kernel = KernelNamespace new.
) : ()
```

- [ ] **Step 4: The table and the boot**

`interpreter/core_library.rs`:

```rust
//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped, and booted at every start.

pub(crate) const PLATFORM_SOURCE: &str = include_str!("../../core-library/Platform.ns");

/// One shipped module. Every entry here is shipped source; a user module
/// joins the table, marked as such by the host, with the step that loads
/// one.
pub(crate) struct ShippedModule {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
}

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[ShippedModule {
    name: "Platform",
    source: PLATFORM_SOURCE,
}];
```

Resolve each with a thread-local cache, as `interpreter::kernel`'s `RESOLVED_KERNEL` does: `resolved_shipped_module(name)`.

`Interpreter::install_platform`:
- `load_module` the resolved `Platform`;
- stash the mirror (`mutator.vm_mirror()`) and the kernel instance (`mutator.kernel`, set by `wire_kernel_in`);
- `instantiate` the class with both, and panic with the error's message if that fails, as `install_kernel` does;
- store the answer in `mutator.platform_singleton`.

`platform_value` then only answers `platform_singleton`, panicking with "internal: the platform is booted at bootstrap" when it is unset. Its doc comment describes the new origin.

- [ ] **Step 5: The kernel lets go**

- **`Kernel.ns`**: delete the four classes and their comment. In `Object`'s accessor comment, "`Top`, the VM mirror's class and `Platform` have no accessor" becomes "`Top` and the VM mirror's class have no accessor".
- **`types.rs`**: remove the four `BuiltinClass` variants and their `ALL` entries.
- **Image tags**: `encode.rs`'s `builtin_class_tag` loses them; `decode.rs`'s `builtin_class_from_tag` answers `None` for 13, 18, 19 and 29. Add a line to `opcodes.rs`'s builtin-class tag comment recording the four retired numbers.
- **`kernel.rs`**: `kernel_class_name` loses them. `check_kernel_shape`'s slot allowance loses `Platform`, and its comment is updated. The fast-path table's `Collections` and `KernelNamespace` rows move to a new test over `Platform.ns`, which asserts that `Collections>>List` has `MethodFastPath::BuiltinClass(OrderedCollection)` if the recognizer gives it one, and otherwise that it is an ordinary method. Record which in the test's comment. `the_platform_declares_no_class_accessor_of_its_own` reads the platform's class from `platform_value`.
- **`primitive_method.rs`**: its refusal list loses the three names.
- **`platform.rs`**: `execute` no longer reads the receiver's class. The mirror is the capability, and the leaf reads only the string argument. Its module comment says so, with the reference to `docs/design/core-library.md`.

- [ ] **Step 6: The golden**

In `goldens/kernel/the_platform_and_its_parts_are_values.ns`, `platform = platform class new` and `platform hash = platform class new hash` become `c = c class new` and `c hash = c class new hash`. `Platform`'s factory is now `usingVmMirror:kernel:`, which a program has nothing to pass to, and the collections namespace is a value whose class answers `new`. The recorded `stdout` (`true` on both lines) stays exactly as it is. Update the module comment to say why the platform is no longer compared with a fresh one.

- [ ] **Step 7: Run everything**

Run: `cargo test --workspace --exclude ns-lsp`, then `cargo run -p ns-cli -- run-corpus corpus/modules` (`ran 6, failed 0, skipped 205`).
Expected: PASS, every golden's recorded outcome unchanged. Then the per-commit gates.

- [ ] **Step 8: The invariants and the documents**

- **`CLAUDE.md`, "The VM mirror is a capability":**
  - "only `Kernel.ns` is resolved with it" becomes: only the shipped core-library modules — `Kernel.ns`, and `Platform.ns` from `interpreter/core_library.rs`'s table — are resolved with it.
  - Add that the platform holds the mirror in a private slot, is handed to every program, and is never written to an image field by field: the encoder names it, and every shipped module instance or class it reaches, by its platform path.
  - In `CLAUDE.md`'s crate map, name `Platform.ns` beside `Kernel.ns`.
- **`CLAUDE.md`, "Working in this repository":** the on-demand rule's "`platform.rs` is widened only on demand" becomes `Platform.ns`.
- **`docs/design/core-library.md`:**
  - Rewrite "The platform" for what now exists: the lazy members, the boot, the transcript leaf.
  - Move the on-demand rule to `Platform.ns`.
  - In "Where the library source lives, and how modules are found", describe the table.
  - Mark plan 1's parts of the "Planned" section as built.
- **`docs/design/README.md`:** update the "Builtins and the platform" row.

- [ ] **Step 9: Commit**

```bash
git add CLAUDE.md crates goldens docs
git commit -m "feat: boot the platform from a shipped Platform.ns module

Co-Authored-By: <model> <noreply@anthropic.com>"
```

---

### Task 7: Measure, run the end-of-batch gates, record the milestone

**Files:**
- Modify: `docs/design/execution.md` (a measurement section after "What the core library's move cost")
- Modify: `docs/design/core-library.md` ("What is measured")
- Modify: `docs/PROGRESS.md` (a milestone entry; "Where the next session starts" names plan 2)

- [ ] **Step 1: Measure start-up and the bench sources**

Follow `docs/design/execution.md`'s protocol ("What removing the re-entries moved"):
- Build release binaries of `master` before Task 1 and of this branch's head. Set `$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"` in PowerShell before each build and `Remove-Item Env:RUSTFLAGS` after it.
- Build several binaries of each tree and pool them: one build per tree measures the build, not the commit.
- Time `ns-next run` of a module that answers at once, against the 4.07 ms recorded in `core-library.md`.
- Time `bench` over the four `goldens/bench/bench_*.ns`.

Record the numbers, the machine and the method in `execution.md`, and the start-up figure in `core-library.md`'s "What is measured". A regression is reported as measured, with its cause if the profile shows one — not rounded away.

- [ ] **Step 2: The end-of-batch gates**

Run each, in Git Bash, and record the results:

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace --exclude ns-lsp
NS_NEXT_STRESS_CACHES=1 cargo test --workspace --exclude ns-lsp
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace --exclude ns-lsp
cargo test -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin
cargo test --workspace --release
cargo build --workspace
```

Expected: every suite passes, and every `run-corpus` reports `ran 6, failed 0, skipped 205`.

- [ ] **Step 3: Record the milestone and commit**

- **`docs/PROGRESS.md`:** add an entry for the platform module (what was built, the goldens that changed and why, the measurement).
- **"Where the next session starts":** plan 2 — the collections module — is now the next piece of work.
- **`docs/design/core-library.md`, "Planned: the platform and collections as core-library modules":** only plan 2 is still planned.

```bash
git add docs
git commit -m "docs: record the platform module and its measurement

Co-Authored-By: <model> <noreply@anthropic.com>"
```
