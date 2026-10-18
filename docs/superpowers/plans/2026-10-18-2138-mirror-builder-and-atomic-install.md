# MirrorBuilder and atomic installation (road step 5b2a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5b2a: a program holding `platform mirrors` edits a class -- its header's slots, factory and initializer, its instance-side and class-side methods, its lazy slots -- through a `ClassDeclarationBuilder`, and `install` makes the edit visible at once to every application of the edited mixin, migrating the instances whose layout changed, or changes nothing.

**Architecture:** The builder, in `MirrorsFactory.ns`, holds only source text and hands it to two new VM-mirror leaves. On the Rust side an edit is a value (`interpreter::edit::Edit`); preparing it splices it into its module's current text, parses and resolves the whole module again, and compiles the edited declaration's subtree fresh, changing nothing (`Mutator::prepare_edit`). Installing it (`Mutator::install_edit`) prepares, then commits without a safepoint: every fresh mixin under its old id, one walk of the object space for the class objects of those ids and their instances, new applications superclasses first with a successor recorded for each level whose own slots stayed, the instances whose layout changed copied into the new shape by name, the rest restamped, one bulk forward, the caches invalidated. The slot check's cold path accepts an entered level's successor, so an initializer interrupted by an edit that kept its slots runs on.

**Tech Stack:** Rust 1.98.0, the Newspeak core library, `cargo-nextest`.

**Spec:** `docs/design/liveness-and-reflection.md`, "Planned: road step 5b2a, MirrorBuilder and atomic installation" (and "An edit replaces the mixin under its id", "What the id has to carry"); `docs/design/object-model.md`, "What a slot access checks" ("Planned: the entered level's successor"), "`become:` is an eager bulk forward", "Everything stays editable". Read them before Task 1. Prior art, for behaviour only: `prior-art/primordialsoup/newspeak/Mirrors.ns` (the builders `:566-824`, `:1310-1573`; `AtomicInstaller` `:264-563`, the migration plan `:369-386`, the layout comparison `:328-344`), `prior-art/primordialsoup/newspeak/MirrorBuilderTesting.ns`, `prior-art/newspeak/AtomicInstaller.ns`, `prior-art/newspeak/AtomicInstallTesting.ns`. Copy no code; the goldens are written here.

**Decided with the user on 2026-10-18** (the spec's decisions; not reopened):

1. Road step 5b2 is two plans: this one (5b2a), then edited programs in images, editing a loaded program and the reclaiming of ids (5b2b).
2. The install recompiles the edited declaration's whole subtree, from its module's text, every time.
3. An edit builds new applications even when it changes only methods; a level whose own slots stayed records its successor, which the slot check's cold path accepts.
4. The builder offers class-side methods besides the core; a class from source with no predecessor and nested-class edits are deferred.
5. The install is one Rust leaf; the forward is called on the Rust side, with no safepoint between rebuilding the classes and forwarding their instances.

**Refined while planning, stated here for review:**

- *Lazy slots.* A lazy slot is a member, not part of the header (`ast::Member::LazySlot`), so "slots through the header" leaves it unreachable; the `MixinBuilder` gains `lazySlots`, a group with the same `addFromSource:` and `removeMirrorNamed:` as `methods`, on the instance side only. The spec already says a new lazy slot is computed on its first read.
- *What the builder reads.* A leaf answers no failure argument to its `ifFail:` block (`interpreter/mirror.rs`'s `failure_step`), so a diagnostic travels in the answer: the check leaf answers `{true. text}` or `{false. diagnostic}`, the install leaf `nil` or a diagnostic. The staged state is read through `ClassDeclarationBuilder>>source`, the edited declaration's text as the check leaf answers it, rather than through mirror groups over staged members, which would need mirrors on members no mixin holds.
- *The current module text* is kept per module, by the root mixin's id, in a table of its own (`Tables::current_module_texts`): the root's row keeps the text its own version's spans point into, so a nested class's edit must not overwrite it.

Task 8 writes the refinements into the spec.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates", group `commit`; plus `images` where a task names it) run before each commit through `cargo xtask gates commit` (`cargo xtask gates commit images` where named, and `cargo xtask gates commit analyzer` never: nothing here touches `crates/ns-analyzer`). The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 8, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `road-step-5b2a` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `edit`, `replacement`, `successor`, `declaration_text`, `operation`, never `ed`, `repl`, `succ`, `op`.
- **Nothing under `crates/ns-syntax` changes.** The edit uses `ns_syntax::parse` on whole module texts.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). No existing golden's `.toml` changes in this plan; if one would, stop and report.
- **Compiling an already-resolved module never raises a `NewspeakError`** (`CLAUDE.md`). Every failure of an edit is decided in `prepare_edit` before compiling, and answered as a diagnostic `String`, never a panic and never a `NewspeakError` from the compiler.
- **The VM mirror is a capability** (`CLAUDE.md`, the six locks). The two new leaves are reached only through `MirrorsFactory.ns`; no program obtains the mirror. An edit of a shipped mixin is refused in `prepare_edit` as well as by `asBuilder`.
- **Every store into an existing object goes through `heap.rs`'s store accessors** (`CLAUDE.md`). The migration copies fields with `Heap::instance_set_field`; the restamp of a header guard and the forward are the heap's own rewrites, as `class_set_application`'s and `forward_identity`'s are.
- **A handle means nothing across a move.** Nothing collects inside a leaf, so the handles the install's walk collects stay valid through its commit; none is kept past the leaf.
- The `MIRROR` table gains two rows, appended after `elementsOf:forwardIdentityToElementsOf:ifFail:` (`builtins/primitive.rs:2350`), never inserted. That changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit (Task 5): `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- **The size tests run in release** after Task 5, which grows `ReflectionPrimitive`: `cargo nextest run -p ns-runtime --release -E 'test(stays_the_size) | test(one_word) | test(fits_in_the_niche)'`. If one grows, stop and report.
- Every new member of `MirrorsFactory.ns` has a Javadoc-standard `///` comment (`CLAUDE.md`, "Every member of a shipped core-library module is documented"; `tests/core_library_documentation.rs`). After editing a `.ns` file run `cargo run -p ns-analyzer -- fmt <the files>`.
- `vm.rs` holds no dispatch logic; the edit lives in `interpreter/edit.rs` and `interpreter/install.rs`, the leaves in `builtins/reflection.rs`.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- Comments explain the decision, not the mechanics.

## Review Focus

1. **Two edits in a row of one module.** A nested class `Inner` edited, then its enclosing class `Outer` edited: the second edit starts from the module's current text, so `Inner`'s edit survives it, and a method's `source` of each answers its current text. Pinned by Task 1's `a_second_edit_keeps_the_first` and Task 7's golden `an_enclosing_edit_keeps_a_nested_edit.ns`.
2. **An edit whose text holds non-ASCII.** A method text or a header with `ä` or an emoji before and after the spliced span: spans are byte offsets, so every splice cuts on character boundaries and `source` answers the text as written. Pinned by Task 1's `a_splice_around_multibyte_text_keeps_every_character`.
3. **An install that fails after its edits were checked one by one.** Two `addFromSource:` each fine alone but colliding together (a method `x` and a lazy slot `x`), and a header that drops a slot an added method's `x:` sends to: the install answers the resolver's diagnostic, and every table, application, guard and instance is as before. Pinned by Task 3's `a_failed_install_changes_nothing` and Task 6's golden `a_failed_install_changes_nothing.ns`.
4. **Instances held everywhere.** A migrated instance held by another instance's slot, an `Array`, a closure's copied value, a running activation's register and receiver, a `WeakArray` and an `Ephemeron`'s key: after the install each names the new-shape object. Pinned by Task 3's `a_migrated_instance_is_replaced_in_every_holder`, and run under every stress mode by the end-of-batch `stress` gate through the goldens of Task 7.
5. **Levels a program does not write itself.** A subclass of a shipped class (an `Error` subclass) whose slots change, a class whose superclass is a value class, a `<:` clause's level over an edited mixin, a metaclass of an edited class: each is re-applied, the shipped level's slots are copied by name, and `behavior:sharesApplicationWith:` still holds between two class objects of one mixin. Pinned by Task 3's `every_level_over_an_edited_mixin_is_reapplied` and Task 7's golden `an_error_subclass_migrates.ns`.

## The edit on the wire

The Newspeak builder hands both leaves the same `Array`, which `Edit::from_value` decodes, answering `None` -- the leaf declines, its `ifFail:` block runs -- for anything else:

```
{ header. instanceOperations. classOperations }
```

`header` is a `String` or `nil` (unchanged). Each operations entry is an `Array` of two-element `Array`s, applied in order: `{#addMethod. aString}`, `{#removeMethod. aSymbol}`, `{#addLazySlot. aString}`, `{#removeLazySlot. aSymbol}`. `classOperations` holds method operations only.

## The diagnostics

Each is a `String` the leaf answers and the builder signals as an `Error` (`MirrorsFactory`'s `Error`, as the mirrors' other refusals). `<name>` is the edited mixin's `reflected_name`.

| Where | Text |
|---|---|
| a shipped mixin (`prepare_edit`) | `<name> belongs to the shipped core library, which no MirrorBuilder edits` |
| a mixin with no declaration (`prepare_edit`) | `<name> was loaded from an image, and a loaded program cannot be edited yet` |
| a mixin with no module text (`prepare_edit`) | `<name> has no source text to edit` |
| a header that renames | `the header renames <name> to <new name>; an edit keeps a class's name` |
| a header that changes a superclass or `<:` clause | `the header changes the superclass of <name>; an edit keeps it` |
| a member text of the wrong kind | `addFromSource: of <name>'s methods takes one method declaration` (`lazy slots`: `one lazy slot declaration`) |
| a syntax error | `<name>'s edited source does not parse: <diagnostic message> (line <n>)` |
| a resolver error | `<name>'s edited source does not resolve: <resolve error message> (line <n>)` |
| an affected instance that is no ordinary object (`install_edit`) | `cannot migrate an instance of <class name>, which is not an ordinary object` |

`<n>` is the 1-based line of the diagnostic's span start in the edited module text.

---

### Task 1: Preparing an edit

**Files:**
- Create: `crates/ns-runtime/src/interpreter/edit.rs` (and `mod edit;` in `interpreter/mod.rs`)
- Modify: `crates/ns-runtime/src/heap/tables.rs` (`Tables`: `current_module_texts`)
- Modify: `crates/ns-runtime/src/heap.rs` (beside `mixin_module_text`, `:3366`: `current_module_text`, `set_current_module_text`)
- Modify: `crates/ns-runtime/src/interpreter/metaclass.rs` (`metaclass_mixin`, `:106-136`: the synthesized class side built by a helper both it and the edit call)
- Test: `interpreter/edit.rs`'s `mod tests` (load a module through `Interpreter::load_module` and instantiate it as `interpreter/mod.rs`'s tests around `replacement_inner`, `:2333`, do; copy their `expect_ok`)

**Interfaces:**
- Produces, in `interpreter/edit.rs`:
  ```rust
  pub(crate) struct Edit {
      pub(crate) header: Option<String>,
      pub(crate) instance_side: Vec<MemberEdit>,
      pub(crate) class_side: Vec<MemberEdit>,
  }
  pub(crate) enum MemberEdit {
      AddMethod(String),
      RemoveMethod(Symbol),
      AddLazySlot(String),
      RemoveLazySlot(Symbol),
  }
  pub(crate) struct PreparedEdit {
      /// The module's root mixin's id, which keys its current text.
      pub(crate) module_root: MixinId,
      /// The whole edited module's text.
      pub(crate) module_text: ModuleText,
      /// The edited declaration's text, sliced from `module_text`.
      pub(crate) declaration_text: String,
      /// Every fresh mixin with the id it is installed under: the edited
      /// declaration's instance side first, then its class side, then
      /// each nested mixin's two sides, preorder. A fresh mixin whose old
      /// counterpart never got an id is absent: nothing names it.
      pub(crate) replacements: Vec<(MixinId, Rc<Mixin>)>,
      /// The id of the synthesized class side the old version had, if it
      /// was registered, with the fresh one replacing it when the new
      /// version declares none (the declared one replaces it otherwise,
      /// and stands in `replacements`).
      pub(crate) synthesized: Option<(MixinId, MixinId, Rc<Mixin>)>,
  }
  impl Edit {
      pub(crate) fn from_value<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Edit>;
  }
  impl<'gc> Mutator<'gc> {
      /// Changes nothing a program observes: no table, application or
      /// object. Registers no fresh mixin.
      pub(crate) fn prepare_edit(&mut self, id: MixinId, edit: &Edit) -> Result<PreparedEdit, String>;
  }
  ```
  `synthesized` is `(instance side id, synthesized class side id, fresh synthesized class side)`.
- Produces, on `Heap`: `pub(crate) fn current_module_text(&self, module_root: MixinId) -> Option<ModuleText>` -- the table's entry, else the root's row text (`mixin_module_text`); `pub(crate) fn set_current_module_text(&mut self, module_root: MixinId, text: ModuleText)`.
- Produces, in `interpreter/metaclass.rs`: `pub(crate) fn synthesized_class_side(mixin: &Rc<Mixin>) -> Rc<Mixin>` -- the mixin `metaclass_mixin` synthesizes today (its factory entry, `Symbol::CLASS_SIDE`, its `enclosing` set to `mixin`), uncached; `metaclass_mixin` calls it.

What `prepare_edit` does, in this order (each failure is the diagnostic of "The diagnostics", answered at once):

1. The current version `heap.mixin(id)`: refuse it shipped, without `declaration`, or a class side (only `asBuilder` on a declaration reaches here, which names the instance side; a class side is a defect of the caller: `debug_assert!` and refuse).
2. The ids from the module's root down to `id` (`heap.enclosing_mixin_id`, repeated); the root's current text (`current_module_text`), refused when absent.
3. `ns_syntax::parse` the current text; find the declaration by the names of the ids below the root (`ast::Member::Class` by `name`), the root's own when `id` is the root. It is found: the current text is what built the current versions.
4. Splice into the text, by byte spans, working from the end of the text to its start so earlier spans stay valid:
   - the header: from the declaration's `span.start` to the instance side's `members_delimiters.open.start`, replaced by `header` -- then the header's new text must end where a members `(` may follow, which step 5's parse checks;
   - `AddMethod` / `AddLazySlot`: parse the text alone inside a scratch module `class Scratch = () (\n<text>\n)` to learn its kind and selector or slot name (one member of the kind the operation names, else the wrong-kind diagnostic); an existing member of that kind and name is replaced, from its first attached comment's start (`comments`, else its `span.start`) to its `span.end`; otherwise it is appended, `"\n" + text + "\n"`, before the side's closing delimiter -- the instance side's `members_delimiters.close`, the class side's `delimiters.close`, or, with no class side declared, `" : (\n" + text + "\n)"` inserted after the instance side's `members_delimiters.close.end`;
   - `RemoveMethod` / `RemoveLazySlot`: the member's range as above cut out; nothing for a name the side does not declare.
   Operations apply in their order: compute each against the text the previous one produced (reparse between operations; module texts are small and an edit is rare, so the simple loop is right).
5. `ns_syntax::parse` the spliced text -- a `Diagnostic` is the syntax diagnostic -- and find the declaration again by the same path: its `name` must still be the old one (else the rename diagnostic), and its `superclass` and `mixin_applications` spans' texts must equal the old declaration's (else the superclass diagnostic).
6. `resolver::resolve(module, text, SourceOrigin::Embedded("<edit>".into()))` -- the first `ResolveError` is the resolver diagnostic. Never `resolve_kernel`: only shipped source records `Binding::MirrorSend`.
7. Compile the declaration: `build_root_mixin` when it is the root, else `build_mixin(declaration, root, &bindings, ModuleSource::User, &text)`. Neither registers the result (`Heap::mixin_id` is not called on it; `debug_assert!` that `heap.known_mixin_id` of each fresh mixin is `None`).
8. Pair fresh with old, recursively: the edited one with `id`; its class side with the old declared class side's `known_mixin_id`, or with the old synthesized one's (`Mutator::metaclass_mixins.get(&id)` then `known_mixin_id`); each fresh nested mixin with the old nested mixin of the same name, and so on down. A pair whose old side has no id is left out.

- [ ] **Step 1: Write the failing tests** in `interpreter/edit.rs`'s `mod tests`, each over a module loaded from source:
  - `adding_a_method_answers_the_declaration_with_it`: `Outer` with `class Inner = () ( public a = (^1) )`; `prepare_edit(inner id, AddMethod("public b = (^2)"))` answers a `declaration_text` holding both methods, a `module_text` whose rest is unchanged byte for byte, and `replacements[0]` keyed by `Inner`'s id whose `methods` holds `#b`.
  - `replacing_and_removing_find_the_member_by_name`: `AddMethod("public a = (^3)")` replaces `a` in place (one `a` in the text), `RemoveMethod(#a)` cuts it with its `///` comment, `RemoveMethod(#missing)` changes nothing.
  - `a_header_edit_changes_the_slots`: header `class Inner = (| public x = 1. public y = 2. |)` gives a fresh mixin with slots `x`, `y`.
  - `an_added_method_rebinds_a_nested_class's_implicit_send`: `Outer` holds `class Inner = () ( public probe = (^target) )` and no `target`; adding `public target = (^#outer)` to `Outer` gives a fresh `Inner` whose `probe` code holds `SendLexical { depth: 1, .. }` for `target` (read the compiled instructions as `compiler/lower.rs`'s tests do).
  - `a_class_side_method_is_added_to_a_class_that_declares_none`: the fresh class side is declared, holds the method and the factory, and pairs with the old synthesized class side's id when it was registered (instantiate the class first so it is).
  - `a_second_edit_keeps_the_first`: edit `Inner`, record the prepared text with `set_current_module_text`, then prepare an edit of `Outer`: its `module_text` still holds `Inner`'s edit.
  - `a_splice_around_multibyte_text_keeps_every_character`: a module whose comments and strings hold `ä` and `🙂` around the edited method; the answered text equals the expected `String` exactly.
  - `every_refusal_answers_its_diagnostic`: a shipped mixin (`List`), a rename, a superclass change, a wrong-kind member (`AddMethod("lazy x = 1.")`), a syntax error, a duplicate member (`AddLazySlot("lazy a = 1.")` beside method `a`) -- each answers its text of "The diagnostics", the line number included.
  - `a_failed_preparation_changes_nothing`: after each refusal above, `heap.mixin(id)` is the same `Rc` and the current module text the same.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(/interpreter::edit::/)'` -- compile errors.
- [ ] **Step 3: Implement** `Edit`, `MemberEdit`, `PreparedEdit`, `prepare_edit`, the two heap accessors, and `synthesized_class_side`. `Edit::from_value` reads through `heap.array_elements`, `heap.string_code_points(..)?.to_string()` (`CodePoints`' `Display`, `code_points.rs`) and `heap.symbol_value`; it is exercised by Task 5's leaf tests, but write its unit test here (`an_edit_decodes_from_its_array`, building the `Array` with `Mutator::new_string` and the symbol table).
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: prepare an edit of a class from its module's current text`.

### Task 2: Applications of one id match, and a level has a successor

**Files:**
- Modify: `crates/ns-runtime/src/heap/tables.rs` (`Tables`: `successors`)
- Modify: `crates/ns-runtime/src/heap.rs` (`with_level_applying_mixin_of`, `:3540-3556`; `receiver_has_level`, `:4780-4796`; new `record_successor`, `successor`)
- Modify: `crates/ns-runtime/src/builtins/activation.rs` (`inherits_mixin_of`, `:252-272`)
- Test: `crates/ns-runtime/src/heap.rs`'s `mod tests` and `builtins/activation.rs`'s

**Interfaces:**
- Produces, on `Heap`: `pub(crate) fn record_successor(&mut self, old: ApplicationIndex, new: ApplicationIndex)` -- records `new` as `old`'s successor and repoints every application whose successor was `old` at `new`, so a chain of edits answers its newest level in one lookup; `pub(crate) fn successor(&self, level: ApplicationIndex) -> Option<ApplicationIndex>`.
- Produces: `receiver_has_level(receiver, level)` answers true when the receiver's chain holds `level` or `successor(level)`; its doc says why (an activation entered before an edit that kept its level's slots).
- Produces: `with_level_applying_mixin_of` and `inherits_mixin_of` compare `known_mixin_id` of the two mixins (`Rc::ptr_eq` first, as the fast case), so an old and a new application of one id match.

- [ ] **Step 1: Write the failing tests:**
  - `a_receiver_has_its_entered_level's_successor`: two applications of one mixin over one superclass, built by hand as `tests/image.rs`'s `install_mixin` builds one (`:735`); an instance of a class object now naming the second; `receiver_has_level(instance, first)` is false, then true after `record_successor(first, second)`, and after a third application and `record_successor(second, third)` with the class object naming the third, `successor(first)` is `Some(third)`.
  - `an_old_and_a_new_application_of_one_id_match`: `with_level_applying_mixin_of(class, mixin_of)` where `mixin_of` names an application of the replacement and `class` one of the old version (use `Mutator::replace_mixin` as `interpreter/mod.rs`'s `replace_inner`, `:2371`, does) finds the level; `inherits_mixin_of` likewise answers true.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** `Tables::successors: IdentityMap<ApplicationIndex, ApplicationIndex>`, documented with its reuse rule left to road step 5b2b (no entry is dropped). In `receiver_has_level`, look the successor up once before the walk; the walk's compare becomes two compares only when there is one.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: let a level's successor pass the slot check and match applications of one id`.

### Task 3: The atomic install

**Files:**
- Create: `crates/ns-runtime/src/interpreter/install.rs` (and `mod install;`)
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication`: `field_names`)
- Modify: `crates/ns-runtime/src/heap.rs` (a read-only walk, `classes_and_instances_over`; `restamp`; `mixin_is_edited` with a per-id flag set by `replace_mixin`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::replace_mixin`, `:2055-2071`: its `expect(dead_code)` goes; the doc's "road step 5b2's install rewrites those" now names `install_edit`)
- Test: `interpreter/install.rs`'s `mod tests`

**Interfaces:**
- Consumes: `Edit`, `PreparedEdit`, `Mutator::prepare_edit`, `synthesized_class_side`, `Heap::set_current_module_text` (Task 1); `Heap::record_successor` (Task 2); `world::forward_identity` (`heap/world.rs:96`); `Mutator::apply_mixin` (`interpreter/mod.rs:1893`); `Heap::class_install_application` (`heap.rs:4009`).
- Produces:
  ```rust
  impl<'gc> Mutator<'gc> {
      /// Answers the edited declaration's text, or a diagnostic with
      /// nothing changed.
      pub(crate) fn install_edit(&mut self, id: MixinId, edit: &Edit) -> Result<String, String>;
  }
  pub(crate) enum FieldName { Slot(Symbol), Nested(MixinId) }
  impl MixinApplication {
      /// Every field of an instance, by index: each level's own slots from
      /// its `own_slot_base`, then its hidden nested-class slots, read off
      /// `nested_slot` -- superclass first.
      pub(crate) fn field_names(&self) -> Vec<FieldName>;
  }
  impl<'gc> Heap<'gc> {
      /// Every class object whose chain -- the class and its superclass
      /// links -- holds an application `replaced` answers true for (by the
      /// application's mixin's id), and every object whose class word
      /// names one of them, in one walk of the space; changes nothing.
      pub(crate) fn classes_and_instances_over(&self, replaced: impl Fn(MixinId) -> bool)
          -> (Vec<ClassReference<'gc>>, Vec<ObjectReference<'gc>>);
      /// Stamps each object's header with its class's guard, as
      /// `class_set_application` stamps every instance of one class.
      pub(crate) fn restamp(&mut self, objects: &[ObjectReference<'gc>]);
      pub(crate) fn mixin_is_edited(&self, id: MixinId) -> bool;
  }
  ```

What `install_edit` does:

1. *Prepare*: `prepare_edit`. The walk `classes_and_instances_over(|id| replaced ids contain it)` runs here too, read-only, and the instances whose class's layout will change are checked to be ordinary objects (`instance_field_count` is `Some`, no reified mixin) -- else the migration diagnostic. Whether a class's layout changes is known only after re-applying, so compare instead the old and the fresh mixins of the chain: the layout changes when any replaced level of the class's chain has another slot list (names in order) or another nested list. Nothing has changed yet.
2. *Replace*: each pair of `replacements` through `Mutator::replace_mixin(id, fresh, Some(module_text))`; `synthesized`'s fresh class side through `Heap::replace_mixin` and into `metaclass_mixins` under the instance side's id; `set_current_module_text(module_root, module_text)`.
3. *Re-apply*: sort the collected class objects by the length of their superclass chain, shortest first. For each: its superclass's application now (`class_application` of `class_superclass`, re-applied already when it was collected), `apply_mixin(&heap.mixin(id of its old application's mixin), superclass application)`, `class_install_application(class, new)`. Then, when the old and the new application have the same `own_slot_base` and the same own slot names in order (`mixin.slots`' names), `record_successor(old index, new index)`.
4. *Migrate*: for each collected instance whose class's old and new `field_names` differ: allocate `allocate_instance(class, new slot_count)`; for each new field index, the old field of the last old index with an equal `FieldName`, copied with `instance_set_field`; the rest stay `nil`. No initializer runs and nothing is sent.
5. *Restamp*: `restamp` the collected instances that did not migrate, and the class objects whose header class (their metaclass) was re-applied -- `class_install_application` restamps only the class object itself.
6. *Forward*: `world::forward_identity(mutator, &old, &new)` over the migrated pairs, `debug_assert!` that each side passes `builtins::reflection`'s `may_be_forwarded` (make it `pub(crate)`); a `ForwardRefusal` here is a defect: `unreachable!`. With nothing migrated, `self.caches.invalidate()` directly: the applications changed under unchanged class objects either way.
7. Answer `declaration_text`.

- [ ] **Step 1: Write the failing tests** in `interpreter/install.rs`'s `mod tests`, each loading a module, instantiating it from the host, editing, then sending:
  - `an_added_method_answers_on_an_existing_instance` and `a_replaced_method_answers_the_new_value` and `a_removed_method_is_not_understood`.
  - `an_added_slot_is_nil_on_an_old_instance_and_initialized_on_a_new_one`; `a_removed_slot's_neighbours_keep_their_values`; `the_identity_hash_survives_a_migration`.
  - `two_class_objects_of_one_nested_mixin_both_see_an_edit` (two `Outer` instances, each with its `Inner` class object created before the edit; §6.2).
  - `every_level_over_an_edited_mixin_is_reapplied`: a subclass in a second module, a `<:` level, the metaclass (a class-side method added answers through the class object), an `Error` subclass whose slots change, and `behavior:sharesApplicationWith:` true between two class objects of one mixin after the edit.
  - `a_migrated_instance_is_replaced_in_every_holder`: holders as Review Focus 4 lists, each read after the install.
  - `an_old_closure_keeps_its_code`: a closure made before the install, run after it, answers the old method's value; a closure made after answers the new.
  - `a_failed_install_changes_nothing`: the two Review Focus 3 cases; afterwards every replaced id's `heap.mixin` is the old `Rc`, every class object's `class_application_index` is as before, every instance's header guard is as before.
  - `a_synthesized_class_side_keeps_its_id`: `known_mixin_id` of the class side before and after an edit that changes the factory's selector through the header is the same id, and the factory answers by the new selector.
  - `a_class_object's_current_application_may_differ_from_an_entered_level`: the test 5b1 did not write -- an initializer activation entered at the old application reads its slot base from it while the class object names the new one (`Heap::activation_slot_base`).
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** `field_names`, the heap's three functions and `install_edit`.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: install an edit atomically, migrating the instances whose layout changed`.

### Task 4: The encoder refuses an edited program

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (before the mixin rows are planned)
- Test: `crates/ns-runtime/tests/image.rs`

- [ ] **Step 1: Write the failing test** `an_edited_program_is_refused_with_an_error`: a program edited by `install_edit` (through the crate-internal path the tests of `tests/image.rs` already use for `install_mixin`, `:735`, or a `#[cfg(test)]` re-export if `install_edit` is not reachable) is encoded: the result is an `Err` whose text is `image encoding cannot yet serialize mixin `<name>`, which a MirrorBuilder edited`, and nothing panics.
- [ ] **Step 2: Run it to see it fail** (today the class object's lookup fails with "not part of any module the image carries", or worse).
- [ ] **Step 3: Implement**: walk the module roots' trees (`Mixin::preorder`, `mixin.rs:1100`) and refuse the first whose `known_mixin_id` is edited (`Heap::mixin_is_edited`), before any row is planned.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit images`: `feat: refuse to encode an edited program until images carry edits`.

### Task 5: The two leaves

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ReflectionPrimitive`, `:827-888`; the exhaustive match `:313-334`; two `MIRROR` rows appended after `:2350`)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (`execute`, `:36`)
- Modify: `crates/ns-runtime/src/interpreter/future.rs` (`:519`, if its match over `Primitive::Reflection` needs the new variants)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Test: `builtins/reflection.rs`'s `mod tests`

**Interfaces:**
- Produces two rows, reported selectors as the forward's row explains (`:2343-2345`):
  - `reflectionMixin:checkEdit:ifFail:`, reported `checkEdit:`, `ReflectionPrimitive::CheckEdit`: the subject a reified mixin, the argument "The edit on the wire"'s `Array`; answers `{true. declarationText}` when `prepare_edit` succeeds, `{false. diagnostic}` when it fails; declines for any other subject or a malformed edit.
  - `reflectionMixin:installEdit:ifFail:`, reported `installEdit:`, `ReflectionPrimitive::InstallEdit`: the same arguments; answers `{true. declarationText}` after `install_edit` succeeded and `{false. diagnostic}` when it changed nothing -- the check leaf's shape, so the builder reads both alike.
- The subject's id is `heap.mixin_object_id`; the instance side's when the subject is a class side (`reflection.rs`'s `instance_side_id`).

- [ ] **Step 1: Write the failing tests** with the `leaf` helper: `the_check_leaf_answers_the_staged_declaration`, `the_check_leaf_answers_a_diagnostic_and_changes_nothing`, `the_install_leaf_installs`, `a_malformed_edit_is_declined` (a `String` instead of the `Array`, an unknown operation symbol, a `nil` subject). And the code running across an install, which needs a program to send the leaf itself -- hand the VM mirror to a test program from the host, as the `leaf` helper (`builtins/reflection.rs:511`) and 5b1's forward tests do:
  - `an_interrupted_initializer_runs_on_when_its_slots_stayed`: a class whose initializer's first slot expression installs a methods-only edit of its own class through the mirror, then initializes a later slot: the instance has both slots, and the successor let the own-slot check pass.
  - `an_interrupted_initializer_raises_when_its_slots_changed`: the edit adds a slot; the initializer's next own-slot access raises `... no longer has the slots of <class>, which the running code was compiled for`.
  - `a_method_that_installs_finishes_with_its_old_code`: a method that installs a replacement of itself and then sends a helper the edit also replaced answers its own old remainder and the helper's new value.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement**; regenerate the golden fixture as "Global Constraints" says and check its diff; run the release size tests.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit images`: `feat: the VM-mirror leaves that check and install an edit`.

### Task 6: The builder in `MirrorsFactory.ns`, and its method goldens

**Files:**
- Modify: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns` (`ClassDeclarationMirror`, `:775-850`: `asBuilder`; new private classes `ClassDeclarationBuilder`, `ClassHeaderBuilder`, `MixinBuilder`, `MutableMemberGroup`)
- Create: `goldens/library/mirror-builder/*.ns` and their `.toml`s (listed below), a new category: `goldens/library/mirror/` holds files, so it may hold no further directory (`crates/ns-cli/tests/common/mod.rs`'s `walk_golden_directory`)
- Modify: `crates/ns-cli/tests/common/mod.rs` (`Expectation`, `:307-321`, and `expectation_of`, `:348-371`: a field `skipped_in_images: Option<String>`, refused blank as `skipped` is)
- Modify: `crates/ns-cli/tests/golden_round_trips.rs` (`:27-39`: a golden with `skipped_in_images` is marked ignored there, as a `skipped` one is)

Every golden here records `skipped_in_images = "a program loaded from an image cannot be edited until road step 5b2b"`: the `images` profile runs each golden through `build` then `load`, and a loaded program's mixins have no declaration, which 5b2a refuses (spec, "The protocol"). The key is the profile's alone; the program still runs under `run`, both lowerings, every stress mode and the caches profile. Road step 5b2b deletes the key with the refusal.

**Interfaces** (Newspeak, each member documented):
- `ClassDeclarationMirror>>asBuilder` -- `ClassDeclarationBuilder reflecting: subject`, which checks the empty edit at once (`vmMirror reflectionMixin: subject checkEdit: {nil. {}. {}} ifFail: [...]`) and signals `Error` with the diagnostic when it answers false.
- `ClassDeclarationBuilder`: `header` (a `ClassHeaderBuilder`), `instanceSide`, `classSide` (each a `MixinBuilder`), `source` (the staged declaration's text, from the last check), `install` (sends `installEdit:`, signals `Error` with the diagnostic or, on success, starts a new staged state from the installed text and answers the receiver), and the private staged state: `headerText`, `instanceOperations`, `classOperations` (growable collections; use the kernel's `List` as the module already does for groups, `:518`), and `check: newState`, which sends `checkEdit:` with the candidate state and adopts it only when it answers true -- so a failed edit leaves the builder as it was.
- `ClassHeaderBuilder`: `source` (the current header's text, sliced from the builder's `source` up to the members' `(` -- or kept from the last `source:`), `source: aString`.
- `MixinBuilder`: `methods` and, on the instance side only, `lazySlots`, each a `MutableMemberGroup` with `addFromSource: aString` and `removeMirrorNamed: aSymbol`; `slots` answers the instance side's slot mirrors of the *live* mixin (reading, as `MixinMirror>>slots` does).

Goldens (each a program that edits a class of its own and prints what the prior-art suites assert; written fresh, `///` comments, formatted with `ns-analyzer fmt`):
`an_added_method_is_invisible_until_install.ns`, `a_replaced_method_answers_the_new_value.ns`, `a_removed_method_is_not_understood.ns`, `removing_a_missing_method_changes_nothing.ns`, `a_class_side_method_is_added_and_removed.ns`, `a_failed_edit_leaves_the_builder_as_it_was.ns` (a syntax error, then a good edit and `install`: only the good edit lands), `a_failed_install_changes_nothing.ns`, `a_shipped_class_is_refused.ns` (`List`, `Object` and the platform's class each refused by `asBuilder` with the diagnostic), `a_header_that_renames_or_changes_the_superclass_is_refused.ns`, `the_builder_source_answers_the_staged_declaration.ns`.

- [ ] **Step 1: Write the goldens** with the outputs the prior-art tests assert (state each expected line from the tests named in the spec's catalogue, not from running the code).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-cli -E 'test(/mirror-builder/)'` (the goldens' runner, `crates/ns-cli/tests/`; find the filter its other goldens use).
- [ ] **Step 3: Implement the builder**; `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/platform/MirrorsFactory.ns goldens/reflection/builder`.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS, the documentation test included.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: a MirrorBuilder for a class's header, methods and lazy slots`.

### Task 7: The goldens of slots, migration and running code

**Files:**
- Create: `goldens/library/mirror-builder/*.ns` and `.toml`s

Goldens: `an_added_slot_is_nil_on_an_old_instance.ns` (and initialized on a new one), `a_changed_slot_initializer_reaches_only_new_instances.ns`, `a_removed_slot_keeps_the_others.ns`, `a_replaced_slot_is_nil_and_the_others_kept.ns`, `a_superclass_slot_is_seen_by_a_subclass_instance.ns`, `a_migration_keeps_the_identity_hash.ns`, `a_migration_sends_nothing_to_a_hostile_object.ns` (a class whose `=` and `hash` print and fail), `a_subclass_in_another_module_migrates` (a project golden: the directory `a_subclass_in_another_module_migrates/` holding a `newspeak.json` and two modules, its `.toml` beside it; `common/mod.rs`'s `golden_projects`), `an_error_subclass_migrates.ns`, `two_nested_class_objects_see_one_edit.ns`, `an_implicit_send_is_rebound_by_an_enclosing_edit.ns` (adding then removing a method in the enclosing class flips the nested class's answer), `an_enclosing_edit_keeps_a_nested_edit.ns`, `an_added_lazy_slot_is_computed_on_first_read.ns`, `a_method_that_installs_finishes_with_its_old_code.ns`, `an_old_closure_keeps_its_code.ns`, `an_interrupted_initializer_runs_on_when_its_slots_stayed.ns`, `an_interrupted_initializer_raises_when_its_slots_changed.ns` (caught with `on: Error do:`, printing the message).

- [ ] **Step 1: Write the goldens**, each expected output reasoned from the spec and the prior-art tests.
- [ ] **Step 2: Run them**: every one passes with Tasks 1-6 in place; one that does not is a defect to fix in the task's own commit, with a Rust test beside the fix when the golden alone does not say where it lies.
- [ ] **Step 3: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 4: Commit** after `cargo xtask gates commit`, every new `.toml` carrying Task 6's `skipped_in_images` line: `test: goldens of slot edits, migration and code running across an install`.

### Task 8: The documentation describes the built step

**Files:**
- Modify: `docs/design/liveness-and-reflection.md` (status block, `:3-17`; "Planned: road step 5b2a" becomes the built section "A MirrorBuilder and an atomic install", in the present tense, with the refinements of this plan's "Refined while planning" and what pins it; "What the id has to carry"; "An edit replaces the mixin under its id"'s "nothing but tests edits a live class yet")
- Modify: `docs/design/object-model.md` ("Planned: the entered level's successor" becomes present tense; "`become:` is an eager bulk forward"'s caller sentence; "Everything stays editable")
- Modify: `docs/design/image-format.md` ("Until then the encoder refuses...", now built)
- Modify: `docs/design/README.md` (the map's row for liveness and the specification-deviation row, `:89`, `:441`)
- Modify: `docs/design/core-library.md` (the mirrors module's leaves: the two new ones)
- Modify: `docs/OPEN-WORK.md` ("Next" becomes road step 5b2b, which also deletes `skipped_in_images` from every golden and the key from `common/mod.rs`; the road list; every item this step closed removed)
- Modify: `docs/PROGRESS.md` (the road step's row completed with this plan's link; its entry, with the commits)
- Modify: `CLAUDE.md` only where a statement became false (`MirrorsFactory.ns`'s description is unchanged by this step; check `rg "5b2" CLAUDE.md`)

- [ ] **Step 1: Rewrite** each passage named above to the current state, as `CLAUDE.md`'s conventions require ("Documentation other than a plan describes the current state").
- [ ] **Step 2: Check** every link and section name the passages cite still exists: `rg -n "Planned: road step 5b2a" docs` answers only the plan.
- [ ] **Step 3: Commit** (Markdown only, no gates): `docs: describe the MirrorBuilder and the atomic install`.
