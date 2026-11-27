# The complete install (hot swap, batch A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Everything about a class can change through the MirrorBuilder, atomically: a superclass and its `<:` clauses, a nested class added, replaced, renamed or removed, a nested class's access, a method, the factory and the initializer, a module's identity. A new module can be declared from source. Several builders install as one. The invalidation after an install vacates only what the install made stale.

**Architecture:** An install becomes a list of edits, prepared module by module on one intermediate text per module and committed in one walk, one commit and one invalidation (`Mutator::prepare_install`, `Mutator::commit_install`). The commit still runs no code and polls no safepoint. A superclass change splits the install into three mirror leaves: *prepare* answers the class objects whose clause changed, Newspeak *evaluates* each one's fresh clause body in its enclosing instance as program code, and *commit* re-applies them over the answers. An install epoch tells the commit whether anything was installed in between. A removed class stays obsolete: it is out of its module's tree and runs on until a full collection frees it, and an image carries it as a row of its own. The caches and the optimized copies are invalidated per re-applied application, through a dependency index the JIT keeps. A new stress mode, `INSTALL`, checks every remaining entry against a fresh lookup.

**Tech Stack:** Rust 1.100.0 (edition 2024), the Newspeak core library (`MirrorsFactory.ns`), `cargo-nextest`, mdBook 0.5.4 for the book.

**Spec:** `docs/design/liveness-and-reflection.md`:
- "Planned: the complete install", decisions 1–9; this plan implements them all;
- "A MirrorBuilder and an atomic install", what is built and what this plan extends;
- "An edit replaces the mixin under its id";
- "Edited programs in images".

Also read:
- `docs/design/modules.md`, decisions 1, 2, 5, 9 and 10;
- `docs/design/image-format.md`, "Edited programs in the image" and "Planned: suspend and resume";
- `docs/design/object-model.md`, "Mixin application" and "What a slot access checks";
- `docs/design/dispatch.md`, "3b — The caches".

The book's chapters this changes:
- `docs/spec/language/semantics.md`: `r[semantics.reflection.atomic-install]` and the gap `r[semantics.gap.reflection.several-builders]`;
- `docs/spec/runtime.md`: `r[runtime.install]`;
- `docs/spec/vm-invariants.md`: `r[vm.heap.install-commit-runs-nothing]`, `r[vm.heap.bulk-forward-atomic]`, `r[vm.dispatch.invalidation-after-change]`, `r[vm.machine.stale-copies-freed]` and the table of detectors;
- `docs/spec/image-format.md`: `r[image.mixins]`, `r[image.modules]`, `r[image.verify.mixin-tree]`, `r[image.verify.module-identity]`, `r[image.write]`, `r[image.write.refusals]` and the tables of tags and structures.

Prior art, for behaviour only; copy no code:
- `prior-art/primordialsoup/newspeak/Mirrors.ns`: the builders at `:566-824` and `:1310-1573`, `name:` at `:764-798`, the `nestedClasses` group at `:1475-1501`, and `AtomicInstaller` at `:264-563`;
- `prior-art/newspeak/AtomicInstaller.ns`;
- `prior-art/newspeak/AtomicInstallTesting.ns`;
- `prior-art/primordialsoup/newspeak/MirrorBuilderTesting.ns`.

**Decided with the user** on 2026-11-22 and 2026-11-27. These are decisions 1–9 of the spec; this plan does not reopen them.

**Refined while planning, stated here for review.** None of these is decided by the user. Task 9 writes each one into the design document as built.

1. **The three-leaf protocol.** A leaf runs no Newspeak code, so a superclass clause is evaluated between two leaves:
   - `reflectionPrepareInstall:ifFail:` answers the epoch and the clauses to evaluate.
   - `reflectionInstall:evaluateClause:epoch:ifFail:` answers a step that activates one fresh clause body on its enclosing instance, as the interpreter's own clause activation does.
   - `reflectionInstall:commitEpoch:superclasses:ifFail:` prepares again, or takes the prepared install it cached for the same epoch and edits, and commits.

   No prepared state outlives a run except a one-entry cache that holds no handle. The class objects travel through Newspeak in an `Array`. The commit refuses:
   - when the epoch moved;
   - when a class object of a changed declaration exists that was not evaluated, because one was made during the evaluation.
2. **`installEdit:` is replaced.** The rows `reflectionPrepareInstall:ifFail:`, `reflectionInstall:evaluateClause:epoch:ifFail:` and `reflectionInstall:commitEpoch:superclasses:ifFail:` are appended to `MIRROR`. The row `reflectionMixin:installEdit:ifFail:` is removed, since every install, a single builder's too, goes through the new rows. `checkEdit:` stays.
3. **The install epoch** is a `u64` on `Mutator`. It is raised by every commit and by every module that joins the heap (`load_root_class`, and a module installed from source). It is distinct from the debugger's `text_revision`, which also moves without an install.
4. **A root's header begins at its first metadata comment.** The root's `ClassHeaderBuilder source` answers the `(* :module: … *)` above the class too, and `source:` may change it. That way an install can change a module's id and version (decision 9), as Primordial Soup's header source holds its metadata (`Mirrors.ns`, `metadataFrom: source`). A nested class's header is unchanged.
5. **A renamed or re-accessed nested class widens the edit to its enclosing declaration.** The preparation recompiles the enclosing declaration's subtree, because its bindings and its method table name the class (decision 5). The renamed child is paired with its old id by the name it had.
6. **A module installed from source has no path.** `ModuleIdentity::path` becomes `Option<String>`. A module without `:module:` gets `ModuleIdentity::undeclared`'s id, its class's name, and no path. The image's module section writes such a module with a new origin tag, `ORIGIN_NONE = 4`.
7. **Obsolete rows in the image** follow the module trees in the mixin table. A `u32` count after the rows says how many of the last trees are obsolete. Each obsolete tree is one removed declaration with its nested ones. It has no module record, and it loads with no enclosing mixin.
8. **An obsolete row is reclaimed on its own.** A full collection frees an obsolete tree once nothing applies, reifies or holds any of its ids, whether or not its former module lives.
9. **The new stress mode is `INSTALL`** (`NS_NEXT_STRESS_INSTALL=1`). After every commit it compares each cache entry, polymorphic slot, global entry and the verified pair against a fresh lookup, and panics on a difference. It is the sixteenth mode.

## Global Constraints

- **Every commit is green.** Before each commit, run `cargo xtask gates commit`, adding the groups a task names:
  - `images` for a commit touching `crates/ns-runtime/src/image/`;
  - `caches` for one touching `interpreter/inline_cache.rs`;
  - `codegen` for one touching `heap.rs`, `interpreter/mod.rs` or `interpreter/sends.rs`;
  - `memory` never;
  - `analyzer` only where a task changes `crates/ns-analyzer`.

  The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 9, by the controller (`finishing-a-batch` skill). An implementer does not run them, and a task's commit is work in progress until they have run. The Docker groups `linux` and `aarch64` are on demand; Task 2 touches the JIT, so the controller runs them before the merge.
- **Commits are conventional** (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`). Each ends with `Co-Authored-By: <the implementing model> <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_01J94EWbk33Lp66SiqCyyDky`. Never change the git identity.
- **Work on the branch `feature/complete-install`** in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already. Never push.
- Run every command in the foreground.
- **Names are written out** (`CLAUDE.md`): `enclosing`, `superclass`, `clause`, `epoch`, `obsolete`, `prepared`, never `enc`, `sup`, `cl`, `prep`.
- **Nothing under `crates/ns-syntax` changes.** The preparation parses whole module texts with `ns_syntax::parse` and reads metadata with `ns_syntax`'s `metadata_of`, as `modules::declared_identity` does.
- **Program outcomes are pinned** (`CLAUDE.md`). Exactly two existing goldens change:
  - `goldens/library/mirror-builder/a_header_that_renames_or_changes_the_superclass_is_refused`, which Task 4 and Task 7 turn into goldens of what now succeeds;
  - `a_header_that_changes_the_access_is_refused`, which Task 4 turns likewise.

  Each change is justified in its commit message. If any other golden would change, stop and report. A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1` and names its rules (`rules = [...]`).
- **Compiling an already-resolved module never raises a `NewspeakError`.** Every refusal is decided before compiling, or between preparation and commit, and is answered as a diagnostic `String`.
- **The commit runs no Newspeak code, sends nothing to a user object and polls no safepoint** (`r[vm.heap.install-commit-runs-nothing]`). Everything that can fail fails before `replace_versions`. Allocation inside the commit (migration copies, new applied-level class objects) is allowed, as today: nothing collects inside a leaf.
- **The VM mirror is a capability** (`CLAUDE.md`, the locks):
  - The new rows are reached only through `MirrorsFactory.ns`.
  - `prepare_edit` still refuses a shipped mixin.
  - A module from source is resolved with `resolver::resolve`, never `resolve_kernel`.
  - An id under `newspeak.` is refused.
- **A handle means nothing across a safepoint.**
  - The class objects and enclosing instances the prepare leaf answers go back to Newspeak inside an `Array`, never into a Rust table.
  - The prepared-install cache holds only `Rc<Mixin>`, ids and texts.
- **Every store into an existing object goes through `heap.rs`'s store accessors.** A class object's new superclass is written with `Heap::class_set_superclass`. An obsolete level's hidden slot is migrated away, as any field is.
- **`MIRROR` rows are appended**, after `block:valueWithArguments:ifFail:`. The one row removed is `installEdit:`; first confirm that nothing indexes rows by position (`mirror_primitive_named` finds them by selector). Changing the rows changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit:
  - run `cargo test -p ns-runtime --test image regenerate_the_golden_fixture -- --ignored` from `crates/ns-runtime`;
  - check with `cmp -l` against `git show HEAD:…` which bytes moved, and name them in the commit message.
- **Every new member of `MirrorsFactory.ns` has a Javadoc-standard `///` comment**, held by `tests/core_library_documentation.rs`. After editing a `.ns` file, run `cargo run -p ns-analyzer -- fmt <the files>`. `MirrorsFactory.ns` is LF; edit it with the Edit tool, never with a Python text-mode write.
- **Every behaviour, format or invariant a task changes changes its rule in `docs/spec/` in the same commit.** A new test names its rules: `// spec: <rule>` above a Rust `#[test]`, `rules = [...]` in a golden's `.toml`. `cargo xtask spec` is part of the commit gate.
- **`vm.rs` holds no dispatch logic.**
  - The install lives in `interpreter/edit.rs`, `interpreter/install.rs` and a new `interpreter/install/` directory where a file grows past what one responsibility needs.
  - The leaves live in `builtins/reflection.rs`.
  - The cache verification lives in `interpreter/inline_cache/verify.rs`.
- **Error handling in tests:** never `{:?}` a `Value`, never `.unwrap()` an `EvaluationResult`; copy a neighbour's `expect_ok`. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- **Comments explain the decision**, not the mechanics.

## Review Focus

1. **The world moves during a clause's evaluation.** A clause that itself installs a builder, or that sends the edited class's accessor in a new enclosing instance, so that a new class object appears. The commit refuses with the epoch or the unevaluated-class diagnostic, and every version, application, guard and instance is as before. Pinned by Task 7's `a_clause_that_installs_fails_the_commit` and `a_class_object_made_during_the_evaluation_fails_the_commit`.
2. **A removed class keeps running.** Its instance answers its methods, its subclass in another module still inherits, and `outer` inside it reaches its enclosing instance. This holds after a scavenge, after a full collection while it is reachable, and after `build` then `load`. A full collection frees it once nothing reaches it, without freeing its former module. Pinned by Task 5's `a_removed_class_runs_on_obsolete` and `an_unreachable_obsolete_class_is_reclaimed`, and Task 6's `an_obsolete_class_loads_obsolete`.
3. **A rename is seen everywhere a name is bound.** An implicit send of the new name in the enclosing class and in a sibling, the old name not understood, a class nested in the renamed one keeping its instances, and the root's `:module:` following its name. Pinned by Task 4's goldens `a_renamed_nested_class_answers_its_new_name` and `a_renamed_root_rewrites_its_module_id`.
4. **The scoped invalidation misses nothing and keeps the rest warm.**
   - An entry of a subclass in another module, a `<:` level over the edited mixin, a global-cache entry with a `Lexical` rule naming a re-applied application, and a baseline cell calling into machine code are each vacated.
   - A cache entry of an unrelated class stays filled.

   Pinned by Task 2's `an_install_vacates_only_entries_naming_a_reapplied_application`, and checked in every suite run under Task 3's `INSTALL` mode.
5. **Texts a reader would not expect.** A `fromSource:` or nested `addFromSource:` text:
   - with a leading `///` comment or `(* :module: … *)` and trailing whitespace is accepted;
   - with two class declarations, or one followed by a member, is refused with `takes one class declaration`;
   - with non-ASCII before and after the spliced span keeps every character.

   Pinned by Task 5's `a_nested_class_text_of_two_declarations_is_refused` and `a_nested_class_splice_keeps_multibyte_text`, and Task 8's `a_module_text_of_two_declarations_is_refused`.

## The install on the wire

The Newspeak side hands the install leaves an `Array` of entries, one per builder, in the order the builders were given:

```
{ {subject. edit}. {subject. edit}. … }
```

- `subject` is a reified mixin, either side of the edited declaration, or `nil` for a module declared from source.
- `edit`, for a reified mixin, is `Edit::from_value`'s array, now of four elements: `{header. instanceOperations. classOperations. newName}`.
  - `newName` is a `Symbol` or `nil`.
  - `instanceOperations` gains `{#addClass. aString}` and `{#removeClass. aSymbol}`.
- `edit` for a `nil` subject is the module's text, a `String`.

The leaves' answers:
- `reflectionPrepareInstall:ifFail:` answers `{true. epoch. clauses}` or `{false. diagnostic}`. `clauses` is an `Array` of `{classObject. enclosingObject}`, one per existing class object whose declaration's superclass or `<:` clauses changed. A root has no clause to evaluate: its superclass follows its fresh `root_shape`.
- `reflectionInstall:evaluateClause:epoch:ifFail:` takes a 1-based index into `clauses` and the epoch. It answers a step activating the fresh clause body:
  - the receiver is the enclosing object;
  - the current class is `Heap::class_declaring(classObject)`;
  - the kind is `ActivationKind::SuperclassClause`;
  - the continuation is `Continuation::Deliver`, so nothing is cached into a hidden slot.

  It declines, and its `ifFail:` block runs, for a stale epoch or an index out of range.
- `reflectionInstall:commitEpoch:superclasses:ifFail:` takes the epoch and an `Array` of the clauses' answers by index. It answers one of:
  - `{true. results}`, where `results` holds, per entry, `{declarationText. headerText}` for an edited declaration, or `{reifiedMixin. declarationText. headerText}` for a module from source;
  - `{false. diagnostic}`.

Each of the three declines, and the builder signals `a MirrorBuilder takes a member or a header as a String and a name as a Symbol`, for an entry it cannot read.

## The diagnostics

These are new or changed. The existing ones are in `liveness-and-reflection.md`'s table, which Task 9 extends with these. `<name>` is a `reflected_name`. `<n>` is a 1-based line.

| Where | Text |
|---|---|
| two entries naming one declaration | `two builders edit <name>` |
| an entry whose declaration another entry removes or replaces | `the builder of <name> edits a class the builder of <other> removes` (`replaces`) |
| a rename to a member name the enclosing class already has | `<enclosing> already has a member named <new name>` |
| a nested-class text that is not one class declaration | `addFromSource: of <name>'s nested classes takes one class declaration` |
| a module text that is not one class declaration | `fromSource: takes one class declaration` |
| a module text that does not parse or resolve | `the module's source does not parse: <message> (line <n>)`; `… does not resolve: …` |
| a new identity the loader's rules refuse | the loader's own text: `modules::declared_identity`, `check_unreserved`, `check_distinct` (`ID_DOES_NOT_END_IN_CLASS`, `IN_RESERVED_NAMESPACE`, `TWO_OF_ONE_VERSION`, `TWO_ONE_WITHOUT_VERSION`, the version's `` `<text>` is no version: <reason> ``), a pathless module named by its `display_name()` |
| an epoch that moved before the commit | `an install ran between the preparation and the commit of <name>'s install; nothing was installed` |
| a class object not evaluated | `a class object of <name> was made while its superclass clauses were evaluated; nothing was installed` |
| a clause answer that is no class | `the superclass clause of <name> answered <description>, which is no class` (`Heap::describe`) |
| a cycle | `the superclass clause of <name> answers a class that inherits from <name>` |
| a value class whose new chain breaks the value rule | the text `nested::after_clause`'s value-class check signals today, as a diagnostic |

The rename, access and superclass refusals of `check_kept_header` are removed. `the header changes the superclass of <name>; an edit keeps it`, `the header renames …` and `the header changes the access …` no longer exist.

---

### Task 1: One install of several builders, and the three leaves

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`Edit` gains `new_name: Option<Symbol>`; `from_value` reads the fourth element; `prepare_edit` becomes per entry over a module text it is handed)
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (`install_edit` gives way to `commit_install`, which runs its commit over every entry)
- Create: `crates/ns-runtime/src/interpreter/install/prepared.rs` beside `install.rs`, as `inline_cache/` stands beside `inline_cache.rs` (`PreparedInstall`, the one-entry cache, the epoch checks)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::install_epoch: u64`, raised in `load_root_class`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (three rows appended, `installEdit:` removed; `ReflectionPrimitive::{PrepareInstall, EvaluateClause, CommitInstall}`; the "declines on a future" match)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (the three leaves; until Task 7, `EvaluateClause` declines for every index, since no clause can change yet)
- Modify: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns` (a private `Installer` class; `ClassDeclarationBuilder>>install` through it; `Mirrors>>installAtomically:`)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Modify: `docs/spec/language/semantics.md` (`r[semantics.reflection.atomic-install]`: several builders as one; the gap rule loses "several builders' edits are not one atomic change"), `docs/spec/runtime.md` (`r[runtime.install]`: the list of edits, prepared per module, one commit)
- Test: `interpreter/install.rs`'s tests; `builtins/reflection.rs`'s tests (the three leaves under both lowerings, after the `CHECK`/`INSTALL` constants there); goldens `goldens/library/mirror-builder/two_builders_install_as_one.ns`, `two_builders_of_one_class_are_refused.ns`, `a_failed_atomic_install_changes_nothing.ns`

**Interfaces:**
- Produces, in `interpreter/edit.rs`:
  ```rust
  pub(crate) enum InstallEntry {
      /// An edit of the declaration whose mixin id this is.
      Edit(MixinId, Edit),
      /// A module declared from source (Task 8 fills it in; until then
      /// `from_value` answers None for a nil subject).
      Module(String),
  }
  pub(crate) fn entries_from_value<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> Option<Vec<InstallEntry>>;
  ```
- Produces, in `interpreter/install/prepared.rs`:
  ```rust
  pub(crate) struct PreparedInstall {
      pub(crate) epoch: u64,
      /// Per entry, in order: the texts the builder reads afterwards.
      pub(crate) texts: Vec<EditedTexts>,
      pub(crate) replacements: Vec<(MixinId, Rc<Mixin>)>,
      pub(crate) synthesized: Vec<(MixinId, MixinId, Rc<Mixin>)>,
      // Task 5: added and removed nested ids; Task 7: the changed clauses;
      // Task 8: the modules to register.
  }
  impl<'gc> Mutator<'gc> {
      pub(crate) fn prepare_install(&mut self, entries: &[InstallEntry]) -> Result<Rc<PreparedInstall>, String>;
      pub(crate) fn commit_install(&mut self, entries: &[InstallEntry], epoch: u64, superclasses: &[Value<'gc>]) -> Result<Vec<EditedTexts>, String>;
  }
  ```
  `prepare_install` groups the entries by module root (`path_from_module_root`), in their order. For each module it applies that module's entries one after another to one intermediate text, starting from `Heap::module_source`. It answers the cached `Rc<PreparedInstall>` when the cache's epoch and entries equal the request's. `commit_install` refuses a stale epoch, then does what `install_edit` did, once for the union of every module's replacements, and raises the epoch.
- Produces, in Newspeak (`MirrorsFactory.ns`; documented):
  - `private class Installer builders: anArray`;
  - `public install`: prepare, evaluate each clause through `vmMirror reflectionInstall: entries evaluateClause: index epoch: epoch ifFail: […]`, commit, then hand each builder its texts through `ClassDeclarationBuilder>>installed: texts`;
  - `Mirrors>>installAtomically: aCollection`, answering `nil` and signalling the first diagnostic;
  - `ClassDeclarationBuilder>>install` becomes `(Installer builders: {self}) install`;
  - `ClassDeclarationBuilder>>entry` answers `{subject. self edit}`;
  - the edit grows its fourth element, `nil` until Task 4.

- [ ] **Step 1: Write the failing tests.**
  - `install.rs`:
    - `two_edits_of_one_module_compose_on_one_text`: two `InstallEntry::Edit`s, of `Outer` and of its nested `Inner`, each adding a method. Both answer after the install, and `Heap::module_source` holds both.
    - `two_edits_of_one_declaration_are_refused`: diagnostic `two builders edit Inner`, and the `snapshot` helper's state unchanged.
    - `a_failed_entry_fails_the_whole_install`: the second entry's syntax error leaves the first's method uninstalled.
    - `a_commit_after_another_install_is_refused`: prepare, install something else, commit with the first epoch. Diagnostic `an install ran between the preparation and the commit of <name>'s install; nothing was installed`, nothing changed.
    - `the_epoch_rises_at_a_commit_and_a_module_load`.
  - `reflection.rs`, `the_install_leaves_answer_under_both_lowerings`, inside `for_each_lowering!`: prepare answers `{true. epoch. {}}`, and commit answers `{true. {{declaration. header}}}`.
  - The three goldens:
    - `two_builders_install_as_one`: two classes of two modules edited, both visible after `installAtomically:`, neither before.
    - `two_builders_of_one_class_are_refused`.
    - `a_failed_atomic_install_changes_nothing`.
- [ ] **Step 2: Run them and see them fail.** Run `cargo nextest run -p ns-runtime -E 'test(two_edits) | test(a_failed_entry) | test(a_commit_after) | test(the_epoch_rises) | test(the_install_leaves)'`, and the goldens through `cargo nextest run -p ns-cli --test goldens -E 'test(atomic) | test(two_builders)'`. Expected: they do not compile, or they fail.
- [ ] **Step 3: Implement the interfaces above.** Notes on the approach:
  - The conflict check runs before any text is touched. Two entries of one `MixinId` are refused; Task 5 adds removal and replacement.
  - `install_edit` goes; its callers, the tests' `install` helper among them, prepare and commit one `InstallEntry::Edit(id, edit)`. Keep the old tests' names and assertions.
  - The leaves take the entries `Array` as their subject.
  - The cache is `Mutator::prepared_install: Option<(u64, Vec<InstallEntry>, Rc<PreparedInstall>)>`, cleared at every commit.
- [ ] **Step 4: Run the tests and see them pass**, then the whole commit gate: `cargo xtask gates commit`.
- [ ] **Step 5: Regenerate the fixture**, as the Global Constraints say.
- [ ] **Step 6: Commit** with the message `feat: several builders install as one, through three install leaves`.

### Task 2: The invalidation is scoped to the re-applied applications

**Files:**
- Modify: `crates/ns-runtime/src/heap/reclaim.rs` (`FreedIds` generalized, or a sibling `StaleIds { guards, applications }` sharing `forget_freed`'s criterion; name it `StaleIds` and have `FreedIds` be its alias or wrapper, whichever keeps `forget_freed`'s callers unchanged)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`InlineCaches::forget_stale(&mut self, stale: &StaleIds)` over sites, the pool, the global cache, the cells and the verified pair, by `forget_freed`'s criterion; `forget_freed` calls it)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`forward_identity` takes `Invalidation::{Everything(VacateCause), Stale(&StaleIds)}`; the bulk-forward leaf passes `Everything`)
- Modify: `crates/ns-runtime/src/jit/mod.rs` (a per-application index of optimized copies, `JitState::dependents: HashMap<DispatchId, Vec<CopyHandle>>`, written where `CopyDependencies::of_relied_on` records a copy, including the copy's own `CustomizationKey`'s two guards, and pruned where a copy is freed; `copies_depending` keeps its scan only for code ids and feedback bases; `invalidate_applications` reads the index)
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (the commit builds `StaleIds` from the old applications it replaced: their `dispatch_ids.instances` guards and their `ApplicationIndex`es)
- Modify: `docs/spec/vm-invariants.md`:
  - `r[vm.dispatch.invalidation-after-change]`: an install vacates exactly the entries naming a re-applied application; the bulk forward's leaf, a breakpoint's fast-path switch and the stress modes still vacate every cache;
  - `r[vm.heap.bulk-forward-atomic]`: the install's forward is scoped;
  - `r[vm.machine.stale-copies-freed]`: found through the index.
- Test: `interpreter/inline_cache.rs`'s tests (beside the `forget_freed` tests at `:4162`, `:4288`, `:4353`); `jit/mod.rs`'s tests or `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Produces: `pub(crate) struct StaleIds { pub(crate) guards: HashSet<DispatchId>, pub(crate) applications: HashSet<ApplicationIndex> }` and `InlineCaches::forget_stale(&mut self, stale: &StaleIds)`.
- Produces: `pub(crate) enum Invalidation<'a> { Everything(VacateCause), Stale(&'a StaleIds) }` as `forward_identity`'s last parameter.
- Produces: `JitState::copies_depending_on(&self, guards: &HashSet<DispatchId>) -> Vec<(Rc<CompiledCode>, Option<CustomizationKey>)>`, read from the index.
- Consumes: Task 1's `commit_install`.

- [ ] **Step 1: Write the failing tests.**
  - `an_install_vacates_only_entries_naming_a_reapplied_application`: warm the caches of two unrelated classes `Edited` and `Untouched` (send each a method twice), install a method on `Edited`, then assert:
    - `Untouched`'s site entry is unchanged (`InlineCaches::entries_of`);
    - `Edited`'s site entry is vacant;
    - a global-cache entry with a `CacheRule::Lexical` rule naming `Edited`'s application is gone;
    - the verified pair naming `Edited`'s level is cleared.
  - `an_install_frees_only_the_copies_depending_on_it`: two optimized copies, one inlining a method of `Edited`, one of `Untouched` only. After the install the first is freed and the second still installed. Force optimization as `tests/jit.rs`'s neighbours do; under `cfg(ns_jit)`.
  - `the_dependency_index_forgets_a_freed_copy`.
- [ ] **Step 2: Run them and see them fail** (`cargo nextest run -p ns-runtime -E 'test(vacates_only) | test(frees_only_the_copies) | test(dependency_index)'`).
- [ ] **Step 3: Implement.** The baseline cells of the vacated entries are vacated with them, as `forget_freed` vacates them. The cells that entered a freed copy are refreshed by `refresh_cells_entering`, as today.
- [ ] **Step 4: Run the tests and the gates** `cargo xtask gates commit caches codegen`.
- [ ] **Step 5: Commit** with the message `perf: an install vacates only what names a re-applied application`.

### Task 3: The `INSTALL` stress mode checks every entry against a fresh lookup

**Files:**
- Create: `crates/ns-runtime/src/interpreter/inline_cache/verify.rs` (`Mutator::verify_caches_against_lookups(&mut self) -> Result<(), String>`)
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (after the commit, under the mode, call it and panic `internal: after an install, <site description> caches <target> where the lookup decides <target>`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the flag `stress_install`, read from `NS_NEXT_STRESS_INSTALL` in debug builds beside `stress_caches_from_env`, with a test setter `set_stress_install`)
- Modify, every list of the modes:
  - `xtask/src/main.rs`: `STRESS_MODES` grows to 16; a `STRESS_SUITE_SECONDS` row (measure: run the suite under the mode once, `cargo xtask gates stress --only "stress INSTALL"`); the help text; the test `the_stress_group_runs_fifteen_modes`, renamed `…_sixteen_modes`;
  - `.github/workflows/stress.yml`'s matrix;
  - `crates/ns-cli/tests/cli.rs` (`:243`, `:488-505`, `:720`);
  - `crates/ns-cli/tests/mcp.rs` (`STRESS_MODES`);
  - `crates/ns-runtime/tests/jit.rs` (`MODES_THAT_CHANGE_THE_COPY`, only if the mode changes a copy; it does not, so leave it);
  - the ad-hoc lists in `jit/inlining_tests.rs:1686`, `builtins/reflection.rs:1895` and `jit/mod.rs:11160` (add it where the list strips modes that change what the test observes);
  - `docs/design/README.md` "Gates";
  - `README.md:173`;
  - `CLAUDE.md`'s table ("fifteen stress modes" becomes "sixteen").
- Modify: `docs/spec/vm-invariants.md`, the table of detectors: a row `INSTALL` | stress mode | every cache entry, polymorphic slot, global entry and verified pair after an install equals a fresh lookup | `vm.dispatch.cache-records-lookup`, `vm.dispatch.invalidation-after-change`
- Test: `interpreter/inline_cache/verify.rs`'s tests

**Interfaces:**
- Consumes: `resolve_public`, `resolve_lexical` and `resolve_super` (`interpreter/resolve.rs:118`, `:163`, `:190`).
- For each filled entry the check needs:
  - the selector, from the site's instruction (`Instruction::sent_message` of the body the site belongs to);
  - the receiver's application, from the entry's guard through the heap's dispatch-id table.

  A lookup that needs a receiver value takes any class object or instance whose application has that guard, which the check finds in one walk of the space (`classes_and_instances_over` with an always-true filter is the precedent). An entry whose guard no live object has is skipped: it can never hit.

- [ ] **Step 1: Write the failing tests.**
  - `a_stale_entry_is_reported`: write a wrong target into a site through the test-only `InlineCaches` setter its neighbours use. The check answers `Err` naming the site.
  - `a_warm_cache_after_an_install_passes`: install, run, check. The check answers `Ok`.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement the check and the mode.**
- [ ] **Step 4: Run the suite once under the mode:** `NS_NEXT_STRESS_INSTALL=1 cargo nextest run --workspace -E 'test(install) | test(mirror_builder)'`. Expected: PASS. A failure is a Task 2 defect: fix it in Task 2's code, in this commit.
- [ ] **Step 5: Run the gates** `cargo xtask gates commit caches` (the spec step checks the table of detectors against `STRESS_MODES`).
- [ ] **Step 6: Commit** with the message `test: the INSTALL stress mode checks every cache entry after an install`.

### Task 4: A rename, an access change and a module's identity

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs`:
  - `check_kept_header` keeps only "the header must declare the class and end where its members begin";
  - `prepare_edit` widens to the enclosing declaration when the name or the access changes;
  - `pair` pairs a renamed child by its old name;
  - the root's header range starts at its first metadata comment;
  - `new_name` rewrites the header's name, and for an annotated root the `:module:` id's last part.
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap::module_identities(&self) -> Vec<Rc<ModuleIdentity>>`, the identities of every registered root, shipped ones included, for `check_distinct`)
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (the identity check before the commit; the fresh root's `module_identity` from `modules::declared_identity` over the fresh text, keeping the old path)
- Modify: `MirrorsFactory.ns` (`ClassHeaderBuilder>>name: aSymbol`, documented with its `@signals`)
- Modify: `docs/spec/runtime.md` (`r[runtime.install]`), `docs/spec/language/semantics.md` (the gap rule's rename part gone)
- Rewrite: `goldens/library/mirror-builder/a_header_that_renames_or_changes_the_superclass_is_refused.{ns,toml}` as `a_header_that_changes_the_superclass_is_refused`, its rename part dropped (the superclass refusal stays true until Task 7), and `a_header_that_changes_the_access_is_refused` as `an_access_change_is_installed`.
- Test: `edit.rs`'s and `install.rs`'s tests; goldens `a_renamed_nested_class_answers_its_new_name.ns`, `a_rename_to_an_existing_member_is_refused.ns`, `a_renamed_root_rewrites_its_module_id/` (a project directory with `newspeak.json`, as `a_subclass_in_another_module_migrates/` is), `a_version_change_is_installed_in_place/`, `a_colliding_identity_is_refused/` (two modules, `Util@1.0.0` and `Util@2.0.0`, the first's header set to `@2.0.0`)

**Interfaces:**
- Consumes: Task 1's `prepare_install` and `commit_install`.
- Consumes: `modules::declared_identity`, `check_unreserved` and `check_distinct` (`crates/ns-runtime/src/modules.rs:379`, `:424`, `:436`).
- Produces: `Heap::module_identities`, which Task 8 reuses.

- [ ] **Step 1: Write the failing tests.**
  - `edit.rs`:
    - `a_rename_widens_the_edit_to_the_enclosing_declaration`: the prepared replacements hold the enclosing mixin's id.
    - `a_renamed_child_keeps_its_id`.
    - `a_root_header_holds_its_metadata`: the header text starts at `(* :module:`.
  - `install.rs`:
    - `a_root_rename_changes_the_identity_in_place`: the same `MixinId`, path kept, id `com.acme.JsonCodec`, same class object.
    - `a_colliding_identity_changes_nothing`.
  - The goldens above. In `a_renamed_nested_class_answers_its_new_name`:
    - the outer class's method `make` sends the new name;
    - an old instance answers its methods;
    - `Inner2 name` answers `'Inner2'`;
    - the old selector is not understood.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** The breakpoints of the module stand on bodies the install replaced. The existing `module_built` call sets them again, under the module's new name when it changed; a breakpoint naming the old id or version stays pending, which batch B handles.
- [ ] **Step 4: Run the tests and the gates** `cargo xtask gates commit`.
- [ ] **Step 5: Commit** with the message `feat: an install renames a class, changes its access and a module's identity in place`. The message says why the two goldens changed: the refusals they pinned are decisions 4, 5 and 9's installs now.

### Task 5: Nested classes added, replaced and removed; obsolete classes

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`MemberEdit::{AddClass(String), RemoveClass(Symbol)}`; `read_member_text` accepts a class declaration; the splice adds or replaces a nested class by name, attached comments included, and cuts a removed one)
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` `pair` (a fresh nested mixin with no counterpart becomes `PreparedInstall::added: Vec<(MixinId /*enclosing*/, Rc<Mixin>)>`, registered under its enclosing id at the commit; an old one with no fresh counterpart becomes `PreparedInstall::removed: Vec<MixinId>`)
- Modify: `crates/ns-runtime/src/heap/tables.rs` (`MixinRow::obsolete: bool`) and `crates/ns-runtime/src/heap.rs`:
  - `Heap::make_obsolete(&mut self, id: MixinId)` drops the id from its parent's `nested` list and marks it and its subtree obsolete, keeping `enclosing` for `path_from_module_root` and `reflected_name`;
  - `Heap::is_obsolete(&self, id) -> bool`.
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (the enclosing classes' instances migrate: their nested list changed, which `layout_may_change` already sees, and `FieldName`'s hidden-slot names drop the removed one)
- Modify: `crates/ns-runtime/src/heap/reclaim.rs`, `crates/ns-runtime/src/interpreter/reclaim.rs` (`module_trees` treats each obsolete subtree as a tree of its own, so `reclaim_mixins` frees it on its own)
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (an edit of an obsolete mixin is refused: `<name> was removed by an install, and an obsolete class is not edited`; add the row to the diagnostics)
- Modify: `MirrorsFactory.ns`: `MixinBuilder>>nestedClasses`, a `MutableMemberGroup adding: #addClass removing: #removeClass …` on the instance side; the class side's `nestedClasses` signals `'nestedClasses: a class side declares no nested classes'`, as `lazySlots` does.
- Modify: `crates/ns-runtime/src/interpreter/install/prepared.rs` (the conflict check gains "removes" and "replaces" between entries)
- Modify: `docs/spec/runtime.md` (`r[runtime.install]`: nested classes paired by name, an obsolete class), `docs/spec/language/semantics.md` (the gap rule's nested part gone), `docs/spec/vm-invariants.md` (a rule `r[vm.heap.obsolete-mixin-out-of-tree]`: an obsolete mixin is in no module's tree and keeps its row until a full collection frees it; detectors: the tests below)
- Test: `install.rs`, `heap/reclaim.rs`'s tests; goldens `a_nested_class_is_added.ns`, `a_nested_class_is_replaced_and_its_instances_migrate.ns`, `a_removed_class_runs_on_obsolete.ns`, `an_added_nested_class_with_a_clause.ns`, `a_builder_of_a_removed_class_is_refused.ns`

**Interfaces:**
- Consumes: Task 1's entries and conflict check.
- Produces: `Heap::is_obsolete` and `Heap::make_obsolete`, which Task 6 reads.

- [ ] **Step 1: Write the failing tests.**
  - `install.rs`:
    - `a_removed_class_runs_on_obsolete`: an instance of the removed `Inner` answers its method; the enclosing class's accessor `Inner` is not understood; `Heap::is_obsolete(inner_id)`; `module_source` no longer holds it.
    - `an_added_class_gets_an_id_under_its_enclosing_one`.
    - `a_replaced_nested_class_keeps_its_id_and_class_objects` (decision 2).
    - `a_nested_class_text_of_two_declarations_is_refused`: text `class A = ()() class B = ()()`.
    - `a_nested_class_splice_keeps_multibyte_text`: an `ä` and an emoji in a comment before and after.
  - `reclaim.rs`:
    - `an_unreachable_obsolete_class_is_reclaimed`: remove `Inner`, drop every instance, run a full collection. The row is freed and the module's other rows stay.
    - `a_reachable_obsolete_class_survives_a_collection`.
  - The goldens above.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** Until Task 6, the image encoder refuses a graph reaching an obsolete class with its existing refusal (`encode.rs:1691-1696`). That keeps the `images` gate green, because the obsolete goldens' outcomes do not depend on `build`/`load`. Check that the image sweep skips nothing; if a new golden fails the sweep, mark it as the sweep's existing mechanism allows, and remove the mark in Task 6.
- [ ] **Step 4: Run the tests and the gates** `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** with the message `feat: an install adds, replaces and removes nested classes; a removed one runs on obsolete`.

### Task 6: An image carries the obsolete classes

**Files:**
- Modify: `crates/ns-runtime/src/image/mixins.rs`:
  - `plan` appends, after the module trees, one tree per obsolete mixin reachable from the graph (a class's level, an applied clause, a reified mixin, or a body a frame or closure runs);
  - `encode` writes the `u32` count of obsolete trees after the rows;
  - `read` and `check_preorder` count the trees as module records plus obsolete trees;
  - `build` marks them obsolete and gives their roots no enclosing.
- Modify: `crates/ns-runtime/src/image/mod.rs` (`plan_describing`, `extra_bodies`: an obsolete id's bodies are admitted), `crates/ns-runtime/src/image/texts.rs` (`texts.rs:61-71`'s "the last row that is no row's child" counts only module trees)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`ModuleCountMismatch` now compares records with `trees - obsolete`)
- Modify: `crates/ns-runtime/src/heap.rs` (the loader marks the decoded obsolete rows with `make_obsolete`'s state, without a parent)
- Modify: `docs/spec/image-format.md`:
  - `r[image.mixins]`: the forest is the module trees, then the obsolete trees;
  - `r[image.verify.mixin-tree]`;
  - the structure table's `mixin-table` row;
  - `r[image.write]` step 3;
  - `r[image.write.refusals]` loses the obsolete case;
  - `### LoadImage` step 4;
  - a new `r[image.load.obsolete]`: an obsolete row loads obsolete, out of every module's tree, and its class objects and instances run its last version.
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated: the new `u32`)
- Test: `crates/ns-runtime/tests/image.rs` (`an_obsolete_class_loads_obsolete`, `an_unreachable_obsolete_class_is_not_written`, `an_obsolete_count_beyond_the_trees_is_refused`, the last in the truncation and crafted-input style of `tests/image_truncations.rs`); every golden of Task 5 through the `images` sweep

**Interfaces:**
- Consumes: Task 5's `Heap::is_obsolete`.

- [ ] **Step 1: Write the failing tests.**
  - `an_obsolete_class_loads_obsolete`: build an image holding an instance of a removed class, load it, send its method, check `is_obsolete`, and check that `module_source` does not hold it.
  - `an_unreachable_obsolete_class_is_not_written`.
  - `an_obsolete_count_beyond_the_trees_is_refused`: the decoder answers its `DecodeError`, and nothing panics.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** The count is validated against the trees before anything is indexed (`r[image.untrusted]`), and each obsolete tree's depth is held to `MAX_MIXIN_DEPTH` like a module's.
- [ ] **Step 4: Regenerate the fixture.**
- [ ] **Step 5: Run the gates** `cargo xtask gates commit images`, and remove any sweep mark Task 5 added.
- [ ] **Step 6: Commit** with the message `feat: an image carries the obsolete classes a program still reaches`.

### Task 7: A superclass change re-evaluates its clause for every class object

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`check_kept_header` no longer compares the clauses; the prepared install records, per replaced id whose superclass or `<:` clause text changed, `PreparedInstall::clauses_changed: Vec<MixinId>`)
- Modify: `crates/ns-runtime/src/interpreter/install/prepared.rs`:
  - `prepare_install` walks the space for the class objects whose own level's mixin id is in `clauses_changed`, and answers them with their `Heap::class_enclosing`;
  - `commit_install` re-walks and refuses a class object not among them, then checks each answer: a class object, or for `<:` clauses `{superclass. mixin…}` as the clause body answers;
  - it refuses a cycle by walking each answer's `class_superclass` chain, and each applied level's, looking for the class object;
  - it applies `after_clause`'s value-class check without signalling.
- Modify: `crates/ns-runtime/src/interpreter/install.rs`:
  - `reapply`, for a changed class object, writes `Heap::class_set_superclass` to the answer and builds its `<:` levels anew through `nested::build_applied_levels` before applying its mixin;
  - the sort by chain length runs after the new superclasses are written;
  - a root whose fresh `root_shape` differs is re-applied over its new static superclass without evaluation.
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`build_applied_levels` is reached from the commit; widen to `pub(crate)` if it is not)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (`EvaluateClause` answers the activation step the wire section describes, through the same push the accessor's `Step::SuperclassClause` takes, with `Continuation::Deliver`; add a `Step` variant only if no existing one carries a clause body, receiver and declaring class with a plain delivery, and say so in the commit message)
- Modify: `MirrorsFactory.ns` (`Installer>>install` evaluates the clauses in order; a clause that signals propagates its error, and nothing is installed)
- Modify: `docs/spec/runtime.md`:
  - `r[runtime.install]`;
  - a new `r[runtime.install.superclass]`: each existing class object's new clause is evaluated as program code in its enclosing instance before the commit, against the unchanged world; the commit re-applies it over the answer, migrates by field name and runs no initializer;
  - `r[runtime.install.epoch]`: an install between the evaluation and the commit, or a class object made meanwhile, fails it.
- Modify: `docs/spec/language/semantics.md` (the gap rule `r[semantics.gap.reflection.several-builders]` is gone with Task 8's part; here remove its superclass part), `docs/spec/vm-invariants.md` (`r[vm.heap.install-commit-runs-nothing]`: the evaluation runs before the commit, in its own leaf's activations)
- Rewrite: `goldens/library/mirror-builder/a_header_that_changes_the_superclass_is_refused` as `a_superclass_change_reparents_every_class_object.ns`
- Test: `install.rs`'s tests; goldens `a_superclass_change_reparents_every_class_object.ns` (two enclosing instances, so two class objects, each re-parented over its own enclosing instance's superclass), `a_slot_of_a_lost_level_goes_and_a_gained_one_starts_nil.ns`, `a_mixin_clause_is_added.ns`, `a_clause_that_signals_changes_nothing.ns`, `a_clause_answering_no_class_is_refused.ns`, `a_cyclic_superclass_is_refused.ns`, `a_clause_that_installs_fails_the_commit.ns`, `a_class_object_made_during_the_evaluation_fails_the_commit.ns`

**Interfaces:**
- Consumes: Task 1's three leaves and the epoch.
- Consumes: `nested::build_applied_levels(mutator, declaration: &Rc<Mixin>, superclass: Option<(ClassReference, Rc<MixinApplication>)>, applied: &[(usize, Value)])` (`nested.rs:520`).
- Consumes: `Heap::class_set_superclass` (`heap.rs:7341`).

- [ ] **Step 1: Write the failing tests.**
  - `a_superclass_change_keeps_the_class_object_and_migrates_by_name`: `Inner = Base` becomes `Inner = Other`. Check:
    - the same class object;
    - `Base`'s slot gone;
    - `Other`'s slot `nil`;
    - `Inner`'s own slot kept;
    - no initializer ran (a counter slot in the enclosing instance stays 0).
  - `a_cycle_changes_nothing`.
  - The goldens above. In `a_clause_that_installs_fails_the_commit`, the new clause is `(installAnother. Base)`, where `installAnother` installs a second builder. The diagnostic is the epoch's, and neither builder's change is visible.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests, then the suite under the `INSTALL` mode** for `test(install) | test(mirror_builder)`, then the gates `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** with the message `feat: an install changes a superclass, re-evaluating its clause for every class object`.

### Task 8: A module declared from source

**Files:**
- Modify: `crates/ns-runtime/src/modules.rs`:
  - `ModuleIdentity::path: Option<String>`;
  - `display_name` and `check_distinct`'s messages name a pathless module by its `display_name()`;
  - every user of `.path` follows: `image/modules.rs`, `ns-cli/src/program.rs`, `debugger/remote.rs`, `ns-analyzer` (`cargo xtask gates commit analyzer` then applies).
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`InstallEntry::Module` read for a `nil` subject; preparation: parse with `ns_syntax::parse`, exactly one class declaration, else `fromSource: takes one class declaration`; `declared_identity`, `check_unreserved`, `check_distinct` against `Heap::module_identities()` plus the other entries' new modules; `resolver::resolve`; `build_root_mixin(…, ModuleSource::User, …, Some(identity))`)
- Modify: `crates/ns-runtime/src/interpreter/install.rs` (the commit registers the module through `Heap::mixin_id`, raises the epoch and answers its reified mixin)
- Modify: `crates/ns-runtime/src/image/modules.rs` and `crates/ns-runtime/src/image/opcodes.rs` (`ORIGIN_NONE = 4`, next free 5; `tests/opcode_tags.rs` pins it; the decoder's `check_project_path` applies to `ORIGIN_PROJECT` only)
- Modify: `MirrorsFactory.ns` (`ClassDeclarationBuilder`'s second factory `fromSource: aString`; `install` answers the builder, which from then on stands for the new declaration; `ClassDeclarationBuilder>>declaration` answers a `ClassDeclarationMirror` on it, or `nil` before the install, so that decision 7's declaration mirror is reached)
- Modify: `docs/spec/image-format.md` (`r[image.modules]`, the tag table, the structure table, `r[image.verify.module-identity]`: a module with origin none has no path), `docs/spec/runtime.md` (`r[runtime.install]`: a module from source), `docs/spec/language/semantics.md` (remove `r[semantics.gap.reflection.several-builders]` and its mention in `docs/spec/appendix/differences-from-bracha.md`; `cargo xtask spec` refuses a pointer to it, so find every one)
- Test: `install.rs`'s tests, `tests/image.rs` (`a_module_from_source_loads_without_a_path`), `tests/opcode_tags.rs`; goldens `a_module_is_declared_from_source.ns`, `a_module_from_source_without_metadata_is_named_by_its_class.ns`, `a_module_from_source_with_a_taken_identity_is_refused.ns`, `a_module_text_of_two_declarations_is_refused.ns`, `a_module_and_an_edit_install_as_one.ns`

**Interfaces:**
- Consumes: Task 1's `InstallEntry::Module`, Task 4's `Heap::module_identities`.

- [ ] **Step 1: Write the failing tests.**
  - `a_module_from_source_is_registered_with_its_identity`: id `com.acme.Fresh@1.0.0`, path `None`, `module_source` equal to the text.
  - `a_module_from_source_under_newspeak_is_refused`.
  - `a_module_from_source_loads_without_a_path`.
  - The goldens above. In `a_module_is_declared_from_source`, the text begins with a `///` comment and the `(* :module: … *)` line and ends in whitespace; the declaration mirror's `name` answers `'Fresh'` and its `source` the text.
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the gates** `cargo xtask gates commit images analyzer`.
- [ ] **Step 5: Commit** with the message `feat: a module declared from source is installed with its identity`.

### Task 9: The documentation describes the built batch

**Files:**
- Modify: `docs/design/liveness-and-reflection.md`:
  - "Planned: the complete install" becomes the built section "The complete install", in the present tense, with refinements 1–9 above, the three-leaf protocol and the diagnostics table extended;
  - "A MirrorBuilder and an atomic install" loses its "Deferred beyond the step" items that are now built;
  - "Caches and their dependency keys" describes the scoped invalidation.
- Modify: `docs/design/image-format.md` ("Edited programs in the image": obsolete rows; "Module section": `ORIGIN_NONE`), `docs/design/modules.md` (decision 1's "every module has a path" now excludes a module from source; decision 10), `docs/design/dispatch.md` ("3b — The caches": the scoped vacate), `docs/design/execution.md` (the dependency index, where it describes `invalidate_applications`), `docs/design/README.md` (the map's status of liveness; "Gates": sixteen modes)
- Modify: `docs/OPEN-WORK.md`:
  - "Next" names batch B, hot swap in the debugger;
  - the complete install's item is removed;
  - added: tools' module lists (`ns-cli/src/program.rs`'s `known_modules`, the trace filter, the DAP agent) do not learn a module from source, which is batch C's.
- Modify: `docs/PROGRESS.md` (the milestone, 2026-11-27 or the day it lands)
- Modify: `CLAUDE.md` (the `ns-runtime` paragraph's install mention: several builders, the three leaves, obsolete classes; the stress mode count)
- Test: `cargo xtask spec`; the documentation is read cold, as the `finishing-a-batch` skill asks

- [ ] **Step 1: Write the documents.**
- [ ] **Step 2: Run `cargo xtask spec`.** Expected: no refusals, and the book is rendered.
- [ ] **Step 3: Commit** with the message `docs: the complete install is built`.
