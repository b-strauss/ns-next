# Module Identity, the String Manifest and Metadata — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Several versions of one module run side by side in one program: a module declares its id and version in metadata above its class, `newspeak.json` lists paths (optionally renaming one with `as`), the application asks `manifest at: 'com.acme.json.Json' version: '2'`, and every tool -- the debugger, the MCP server, the plugin, an image after `load` -- names a module by its id and version and finds its file by its path relative to the project.

**Architecture:** `ns-syntax` collects §4.3 metadata into a side table beside the comments it already attaches, without touching the lexer, the attachment or the formatter. `ns-runtime` gains `modules.rs` -- `ModuleId`, `Version`, `VersionRange`, `ModuleIdentity` and their rules -- and a module's root mixin carries its `ModuleIdentity`, which the debugger names modules by. The host (`ns-cli`) derives each module's identity from its metadata and the project file, refuses what decision 5 and 9 refuse, and builds the new `Manifest ids:versions:ranks:definitions:`; `Manifest.ns` answers `at:` and `at:version:`. The image's module record carries the relative path, the id and the version.

**Tech Stack:** Rust 1.100.0 (edition 2024), `cargo-nextest`; Newspeak (`Manifest.ns`, the shipped modules' first line); Kotlin for the plugin's breakpoint types; no new external dependency.

**Spec:** `docs/design/modules.md` (decisions 1-10, "What follows from it"); `docs/OPEN-WORK.md`, "Next".

## Global Constraints

- No new external dependency. The lexer of `ns-syntax` and the plugin's `NewspeakLexer.kt` do not change: metadata are comments lexically. The comment attachment (`attach.rs`) and the formatter's output do not change: `crates/ns-analyzer/tests/formatting_gate.rs` and the attachment tests pass unchanged.
- **Ids:** `segment ('.' segment)*`, each segment a Newspeak identifier (`[A-Za-z_][A-Za-z0-9_]*`), the last equal to the module's class's name; an id beginning `newspeak.` only for a shipped module or `Manifest.ns`.
- **Versions:** semver 2.0.0 `MAJOR.MINOR.PATCH` with an optional pre-release, ordered by semver's precedence; build metadata (`+…`) refused; no leading zero in a numeric part. **Ranges:** `MAJOR`, `MAJOR.MINOR`, or a full version (pre-release included); a range without a pre-release matches no pre-release.
- **Metadata payload of `:module:`:** `<id>` or `<id>@<version>`, surrounding whitespace trimmed, nothing else. Above a module's class it declares; before a factory parameter it documents an expectation (`<id>` or `<id>@<range>`) and is checked by nothing.
- **Fixed message texts**, named once as `const`s or format strings in the module that raises them and matched by the tests:
  - `` `{id}` does not end in the class's name `{class}` `` (a `:module:` or an `as`),
  - `` `{text}` is no version: {reason} ``,
  - `` `{id}` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships ``,
  - `` two modules are `{id}@{version}`: {path} and {path} ``, `` two modules are `{id}` and one of them has no version: {path} and {path} ``,
  - the manifest's `` no module `{id}` `` / `` no module `{id}` of version {range}; there are {candidates} `` / `` `{id}` names {count} modules; ask with at:version: `` (raised as `Error signal:` text),
  - the warning `` warning: `{id}` version {range} matched {versions}; took {taken} ``.
- **The manifest is asked with strings only** (decision 3). Every `manifest Name` in the repository's `.ns` files outside `corpus/` and `prior-art/` becomes `manifest at: 'Name'`; `corpus/` stays upstream's verbatim copy and is only parsed.
- **Tool names** (decision 8): `<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`; a frame's name in a stack, a stop summary or a trace writes the module's version whenever it has one.
- **Images:** `ORIGIN_FILE` (0) is retired and never reused; new tags take the next free numbers (`opcodes.rs`, pinned by `tests/opcode_tags.rs`). The decoder checks every id, version and path before any is used, and allocates nothing a count it has not bounded asks for ("An image is untrusted input", `CLAUDE.md`).
- **The core library:** every shipped module and `Manifest.ns` gains one `:module:` line above its class, which changes the shipped code's fingerprint: `regenerate_the_golden_fixture` (`crates/ns-runtime/tests/image.rs`) rewrites `tests/fixtures/golden-image.bin` in the same commit. Every member of `Manifest.ns` keeps its Javadoc-standard doc comment (`core_library_documentation.rs`). `.ns` files are LF; after editing one, `cargo run -p ns-analyzer -- fmt <paths>` on exactly those files.
- **Goldens:** no golden's output changes but those whose program text this plan changes (`manifest X` to `manifest at: 'X'`) or whose stderr names a module (stack frames qualified); each such change is established right before its `.toml` is touched, recorded with `NS_NEXT_NO_JIT=1`.
- Names written out; never `{:?}` a `Value`; a Newspeak module inline in a test is an `indoc! {"…"}` with its closing `"}` on its own line; comments explain the decision.
- Every commit runs `cargo xtask gates commit` (+ `images` for Task 5 and any commit touching `crates/ns-runtime/src/image/`, + `analyzer` for Task 8). The batch ends with `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification` (Task 9); every commit before is work in progress. Subagents run every command in the foreground, with `timeout: 600000` on every long one.

## Review Focus

1. **A project with two versions of one unannotated module and no `as`** (`a/Util.ns`, `b/Util.ns`): refused before anything runs with the two-modules message, naming both paths -- never a silent last-writer. Pinned in Task 4 (`two_unannotated_modules_of_one_name_are_refused`).
2. **A range matching a pre-release** (`2.1.0-beta.1` and `2.0.0` present, `version: '2'`): `2.0.0` is taken; only `version: '2.1.0-beta.1'` takes the beta. Pinned in Task 1 (`a_range_without_a_pre_release_matches_none`) and Task 4 (`the_manifest_takes_no_beta_for_a_major_range`).
3. **A crafted image whose module record holds `../../etc/passwd`, an id in `newspeak.`, a version `1.0`, or two modules of one id and version**: each refused as a `DecodeError`, no panic. Pinned in Task 5 (`a_module_record_is_held_to_the_project_rules`).
4. **A breakpoint on one version** (`com.acme.util.Util@1.0.0>>run` with 1.0.0 and 2.0.0 loaded): stops in 1.0.0's activations only; without `@`, in both. Pinned in Task 6 (`a_breakpoint_with_a_version_stands_in_that_version_only`).
5. **A method breakpoint written in today's syntax with a nested class** (`Demo.Account>>deposit:`): refused with a message that shows the new form `Demo/Account>>deposit:`, never silently read as module `Demo.Account`. Pinned in Task 6 (`a_dotted_nested_class_is_refused_with_the_new_form`).

---

### Task 1: Ids, versions, ranges and identities

**Files:**
- Create: `crates/ns-runtime/src/modules.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (re-exports)
- Test: `crates/ns-runtime/src/modules.rs` (unit)

**Interfaces:**
- Produces (public):
  - `pub struct ModuleId(String)` with `pub fn parse(text: &str, class_name: &str) -> Result<ModuleId, String>` (the segment grammar, the last segment equal to `class_name`), `pub fn of_class(class_name: &str) -> ModuleId` (an unannotated module's), `pub fn is_reserved(&self) -> bool` (`newspeak.` prefix), `pub fn as_str(&self) -> &str`.
  - `pub struct Version { pub major: u64, pub minor: u64, pub patch: u64, pub pre_release: Vec<PreReleasePart> }` with `pub enum PreReleasePart { Number(u64), Text(String) }`, `pub fn parse(text: &str) -> Result<Version, String>`, `impl Ord` by semver precedence, `impl Display` (the canonical text).
  - `pub struct VersionRange` with `pub fn parse(text: &str) -> Result<VersionRange, String>` and `pub fn matches(&self, version: &Version) -> bool`.
  - `pub struct ModuleIdentity { pub path: String, pub id: ModuleId, pub version: Option<Version> }` -- `path` relative with `/` separators, or `core:<File>.ns` for a shipped module and `core:Manifest.ns` for the manifest; `pub fn display_name(&self) -> String` (`id` or `id@version`).
  - `pub fn check_relative_path(path: &str) -> Result<(), String>` -- non-empty, `/`-separated, no root, drive or `\`, no empty or `.` segment, `..` only as leading segments, ends in `.ns`; or `core:` and a file name.
- [ ] **Step 1: Write the failing unit tests**
  - `an_id_ends_in_its_class` (`com.acme.json.Json` for `Json` ok; for `Parser` the message; `com..Json`, `.Json`, `com.acme.1x.Json`, `com.acme json.Json` refused).
  - `the_reserved_namespace` (`newspeak.core.Kernel` reserved; `newspeakx.Json` and `org.newspeak.Json` not).
  - `versions_parse_and_order_by_semver` (`1.0.0 < 2.0.0-alpha < 2.0.0-alpha.1 < 2.0.0-beta < 2.0.0-beta.2 < 2.0.0-beta.11 < 2.0.0-rc.1 < 2.0.0 < 2.1.0`; `1.0`, `01.0.0`, `1.0.0+build`, `1.0.0-`, `v1.0.0` refused with their reason).
  - `ranges_are_prefixes` (`2` matches `2.0.0` and `2.9.1`, not `3.0.0`; `2.1` matches `2.1.7` not `2.2.0`; `2.1.0` matches only itself).
  - `a_range_without_a_pre_release_matches_none` (`2` does not match `2.1.0-beta.1`; `2.1.0-beta.1` matches it).
  - `relative_paths_are_checked` (`libs/a/Util.ns`, `../shared/Util.ns`, `core:Kernel.ns` ok; `/abs/Util.ns`, `C:/x/Util.ns`, `a\\Util.ns`, `a/../Util.ns`, `a//Util.ns`, `a/Util.txt` refused).
- [ ] **Step 2: Run to fail** -- `cargo nextest run -p ns-runtime modules::` -- does not compile.
- [ ] **Step 3: Implement** `modules.rs`; no dependency, the semver precedence by its rules (numeric parts numerically, text parts in ASCII order, numeric before text, a longer pre-release after a prefix of it, no pre-release above any).
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: module ids, semver versions, ranges and identities`

### Task 2: §4.3 metadata collected in `ns-syntax`

**Files:**
- Create: `crates/ns-syntax/src/metadata.rs`
- Modify: `crates/ns-syntax/src/lib.rs`, `docs/design/frontend.md` ("Comments": metadata)
- Test: `crates/ns-syntax/src/metadata.rs` (unit)

**Interfaces:**
- Produces:
  - `pub struct Metadata { pub tag: String, pub payload: String, pub span: Span }` -- `tag` the text between the comment's first two colons after `(*` and optional whitespace; `payload` the rest, trimmed.
  - `pub enum MetadataTarget { Module, Class(Span), Factory(Span), Method(Span), Parameter(Span), Slot(Span), Expression(Span) }` -- each `Span` the target's own (a class's declaration, a pattern's, a parameter's name, a slot's name, an expression's).
  - `pub fn metadata_of(module: &ast::Module) -> Vec<(MetadataTarget, Metadata)>` -- §4.3's rules: immediately before an expression, slot or parameter → that; immediately after a method's or factory's pattern → it; immediately after a class's header → the class; immediately before the module's class declaration → `Module`. "Immediately" admits whitespace and other comments between. Works from the attached comments and the nodes' spans; a block comment without a tag is not metadata.
- [ ] **Step 1: Write the failing unit tests**, each over a small module: `a_module_metadata_stands_above_its_class` (`(* :module: com.acme.json.Json@2.1.0 *)` then `class Json …`: `(Module, tag "module", payload "com.acme.json.Json@2.1.0")`, also with a `///` doc comment above it); `a_parameter_metadata_belongs_to_the_parameter` (`json: (* :module: com.acme.json.Json@2 *) json` in a factory pattern: `Parameter(<json's span>)`); `a_factory_metadata_follows_its_pattern` (`class Foo bar: x (*:exemplar: Foo bar: 3 *) = …`); `a_method_metadata_follows_its_pattern`; `a_slot_and_an_expression`; `the_specification_s_example` (§4.3's `Foo` example, every metadata comment of it at the target the spec names); `an_untagged_comment_is_no_metadata` (`(* plain *)`, `(* : no tag *)`, `/// :module: x`).
- [ ] **Step 2: Run to fail.** **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: metadata comments collected by their §4.3 targets`

### Task 3: A module's identity on its root mixin; the shipped modules' ids

**Files:**
- Create: `crates/ns-tooling/src/module_identity.rs`
- Modify: `crates/ns-runtime/src/types.rs` (`ResolvedModule::identity`), `crates/ns-runtime/src/mixin.rs` (`Mixin::module_identity`), `crates/ns-runtime/src/interpreter/instantiate.rs` and `interpreter/core_library.rs` (set it; shipped modules' from their metadata), `crates/ns-runtime/src/interpreter/install.rs` (an edit's new version keeps it), every `crates/ns-runtime/core-library/*.ns` (one `:module:` line), `crates/ns-runtime/tests/fixtures/golden-image.bin`, `docs/design/core-library.md`
- Test: `crates/ns-tooling/src/module_identity.rs` (unit), `crates/ns-runtime/tests/manifest.rs` or a new `tests/module_identity.rs`

**Interfaces:**
- Consumes: Task 1's types, Task 2's `metadata_of`.
- Produces:
  - `pub fn declared_identity(module: &ast::Module) -> Result<(ModuleId, Option<Version>), String>` (`ns-tooling`): the `Module` target's `module` metadata, one at most (two is an error naming both), parsed by Task 1's rules against the root class's name; none → `ModuleId::of_class`, no version.
  - `ResolvedModule::identity: Option<Rc<ModuleIdentity>>`, `None` from `resolve` and set by a host before `load_module`; `Mixin::module_identity: Option<Rc<ModuleIdentity>>` on a root mixin, set by `load_root_class` from it and kept by an install's replacement.
  - The shipped modules declare `newspeak.core.<Name>` (Kernel, PlatformFactory, TranscriptFactory, TimeFactory, CollectionsFactory, TypedArraysFactory, MirrorsFactory) and `Manifest.ns` `newspeak.core.Manifest`, no version; their identity's path is `core:<Name>.ns`. A shipped module's metadata failing `declared_identity` fails the build's own tests, not a program.
- [ ] **Step 1: Write the failing tests**: `an_unannotated_module_is_its_class` (`Util`, no version); `a_declared_id_and_version`; `a_mismatched_id_is_refused` (the fixed text); `two_module_metadata_are_refused`; `every_shipped_module_declares_its_reserved_id` (iterate `SHIPPED_MODULES` and `Manifest`: `newspeak.core.<name>`, no version); `an_installed_edit_keeps_the_identity` (a MirrorBuilder install of a method: the new root mixin's `module_identity` is the old one's).
- [ ] **Step 2: Run to fail.** **Step 3: Implement**; add the metadata line to each core-library file above its class (below its `///` comment), run `fmt` on them, run `regenerate_the_golden_fixture`.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: a module's identity from its metadata, held by its root mixin; newspeak.core ids`

### Task 4: The string manifest, `as`, and the project's rules

**Files:**
- Modify: `crates/ns-runtime/core-library/Manifest.ns` (`ids:versions:ranks:definitions:`, `at:`, `at:version:`, `ambiguities`), `crates/ns-cli/src/project.rs` (module entries), `schemas/newspeak.schema.json`, `crates/ns-cli/src/program.rs` (identities, refusals, `build_manifest`, the warnings), `crates/ns-cli/src/commands/run_corpus.rs` (the wrapper), `crates/ns-cli/src/commands/build.rs` and `run.rs` (print the warnings), every `manifest <Name>` in `benchmarks/`, `goldens/`, `crates/*/tests/` (to `manifest at: '<Name>'`), `goldens/tool/manifest/a_name_the_manifest_does_not_hold_is_not_understood` (renamed for the new error, its `.toml` established anew), `docs/design/image-format.md` ("Packaging"), `docs/design/core-library.md` ("The manifest")
- Test: `crates/ns-cli/tests/project_schema.rs`, `crates/ns-cli/tests/run.rs`, `crates/ns-runtime/tests/manifest.rs`, new goldens under `goldens/tool/manifest/`

**Interfaces:**
- Consumes: Tasks 1 and 3.
- Produces:
  - `modules` entries: a path string, or `{ "path": <string>, "as": <id> }` (no other key; `as` checked by `ModuleId::parse` against the module's class and refused in `newspeak.`). `Project::modules: Vec<ProjectModule>` with `pub(crate) struct ProjectModule { pub(crate) path: PathBuf, pub(crate) id: Option<String> }`.
  - Each loaded module's `ModuleIdentity`: path relative to the project directory (for a `.ns` program, its file's directory; for `main` too), id from `as` or `declared_identity`, version from `declared_identity`; set on `ResolvedModule::identity` before `load_module`. The refusals of decision 5 and 9 before anything runs, with the fixed texts; the check that no module name collides with `Object`'s public members is removed.
  - `Manifest ids: anArrayOfStrings versions: anArrayOfStringsOrNil ranks: anArrayOfIntegers definitions: anArrayOfClasses` -- `ranks` each module's place among those of its id by semver precedence (the host's order, so that `Manifest.ns` compares integers); `at: id` and `at: id version: range` with the fixed error texts, `range` matched in Newspeak on the versions' canonical texts: a version matches when it equals the range, or begins with the range followed by `.` and holds no `-` (no pre-release) -- so `2` matches `2.3.1` but not `20.0.0` or `2.1.0-beta.1`; `ambiguities` an `Array` of `String`s, one per `at:version:` that matched several, which the host prints as the warning after the application's assembly and again after `main:args:` for any added since.
- [ ] **Step 1: Write the failing tests**
  - `project_schema.rs`: the table gains `{ "path", "as" }` accepted, `{ "path" }` accepted, `{ "as" }` refused, `{ "path", "as", "x" }` refused, `as` not a string refused -- schema and reader alike.
  - `run.rs`: `two_versions_run_side_by_side` (a project with `libs/v1/Util.ns` `(* :module: com.acme.util.Util@1.0.0 *)` answering `'one'` and `libs/v2/Util.ns` `@2.0.0` answering `'two'`, an app printing `(manifest at: 'com.acme.util.Util' version: '1') new answer` and `… version: '2' …`: stdout `one\ntwo`); `two_unannotated_modules_of_one_name_are_refused`; `as_renames_an_unannotated_module` (the same two unannotated files, one with `as: "legacy.Util"`: both reachable); `an_id_in_newspeak_is_refused` (a module `(* :module: newspeak.json.Json *)`, and an `as` into `newspeak.`); `the_highest_matching_version_is_taken_and_warned` (`2.1.0` and `2.3.1`, `version: '2'`: `2.3.1`'s answer, stderr holds the warning naming both); `the_manifest_takes_no_beta_for_a_major_range`; `a_module_named_like_an_object_member_is_reachable` (a module `class printString = …`-style name that `Object` answers: reached with `at: 'printString'`, no refusal).
  - `manifest.rs`: `at_without_a_module_signals_with_the_candidates`; `at_on_several_versions_asks_for_a_version`.
- [ ] **Step 2: Run to fail.**
- [ ] **Step 3: Implement**; rewrite every `manifest <Name>` named above; establish each changed golden's output (the program text changed, the output must not) and the renamed golden's new error before touching its `.toml`, recorded with `NS_NEXT_NO_JIT=1`; `regenerate_the_golden_fixture` if `Manifest.ns`'s change reaches the fixture.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat: the manifest is asked with strings, by id and version; newspeak.json's as`

### Task 5: The image records path, id and version

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (retire `ORIGIN_FILE`, and `ORIGIN_EMBEDDED` unless a caller of `decode_source_origin` other than the module record still needs it; `ORIGIN_PROJECT` = 2, `ORIGIN_CORE` = 3; `MODULE_UNVERSIONED` = 0 / `MODULE_VERSIONED` = 1 in a new group with its own "Next free number"), `image/modules.rs` (`ModuleSource::identity`, `DecodedModule::identity`), `image/encode.rs`, `image/decode.rs`, `crates/ns-runtime/tests/opcode_tags.rs`, `crates/ns-cli/src/program.rs` (`module_sources`), `crates/ns-cli/src/commands/load.rs` (names from the identity), `docs/design/image-format.md` ("The module section", "Verification", "Packaging")
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/image_truncations.rs`, `crates/ns-cli/tests/build_load.rs`

**Interfaces:**
- Consumes: Tasks 1, 3, 4.
- Produces: a module record = origin (`ORIGIN_PROJECT` + the relative path, or `ORIGIN_CORE` + the file name, which `Manifest.ns` -- `core:Manifest.ns` -- is written with), the id string, `MODULE_UNVERSIONED` or `MODULE_VERSIONED` + the version's text, the root's object id. `MIN_MODULE_BYTES` grows accordingly. The decoder parses id and version with Task 1's functions against the root class's name (known from the mixin table's row), checks the path with `check_relative_path`, refuses `newspeak.` outside `ORIGIN_CORE`, and the two-modules rules over all records; the loaded root mixins get their identities.
- [ ] **Step 1: Write the failing tests**: `image.rs`: `an_image_names_its_modules_as_the_program_did` (build a two-version project, load: identities equal, paths relative); `the_building_machine_s_paths_are_not_written` (an image built from an absolute project path holds no byte sequence of that path); `a_module_record_is_held_to_the_project_rules` (crafted records: `../../etc/passwd` without `.ns`, `/abs.ns`, an id in `newspeak.` with `ORIGIN_PROJECT`, version `1.0`, two of one id and version, an id not ending in the root's name -- each a `DecodeError`); `opcode_tags.rs` pins 2 and 3 and `ORIGIN_FILE` retired; `image_truncations.rs` covers the new fields; `build_load.rs`: `load_names_modules_by_their_identity`.
- [ ] **Step 2: Run to fail.** **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit images`.
- [ ] **Step 5: Commit** -- `feat: an image records each module's relative path, id and version`

### Task 6: The debugger, the DAP agent and the MCP server name modules by id

**Files:**
- Modify: `crates/ns-runtime/src/debugger/remote.rs` (`ModuleName`), `debugger/stack.rs` (`frame_name` qualified), `debugger/breakpoints.rs` (`module_roots`, `module_text`, `find_methods`), `debugger/exceptions.rs`, `debugger/stepping.rs`, `debugger/memory.rs`, `debugger/metadata/again.rs`, `crates/ns-cli/src/debug/sources.rs`, `crates/ns-cli/src/debug/breakpoints.rs` (`parse_class_name`, `parse_method_name`, `parse_class_names`), `crates/ns-cli/src/debug/evaluation.rs`, `crates/ns-cli/src/mcp/session.rs` and `mcp/tools.rs` (location syntax, schema texts), `crates/ns-cli/src/mcp/summary.rs`, `docs/design/debugger.md`
- Test: `crates/ns-runtime/tests/debugger.rs`, `crates/ns-cli/src/debug/breakpoints.rs` (unit), `crates/ns-cli/tests/debug_adapter.rs`, `crates/ns-cli/tests/mcp.rs`

**Interfaces:**
- Consumes: Tasks 1, 3, 4.
- Produces:
  - `pub struct ModuleName { pub id: String, pub version: Option<String> }` -- `version: None` names every version; `ModuleName::names(&self, root: &Mixin) -> bool` compares the root's `module_identity` (id equal; version equal when given). The `shipped` flag goes: a shipped module is `newspeak.core.<Name>`. `ModuleName::of_root(root)` answers the root's id and version.
  - `FrameDescription::name` and every frame name: `<id>[@<version>][/<path>]>><selector>` etc. (`frame_name`'s forms, the owner qualified), e.g. `com.acme.util.Util@1.0.0/Inner>>run`, `[] in Demo>>main:args:`, `newspeak.core.Kernel/Array>>do:`.
  - The parser of `<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`; a name whose module part holds a `.` and names no module of the program but whose prefix does (`Demo.Account>>deposit:` with a module `Demo`) is refused with ``no module `Demo.Account`; a class inside a module is written `Demo/Account` ``.
- [ ] **Step 1: Write the failing tests**: unit: `method_names_parse_with_id_version_and_inner_path` (`com.acme.json.Json@2.1.0/Parser.Token class>>at:put:`), `a_root_method_has_no_slash` (`Calls>>>>` still parses: selector `>>`); `a_dotted_nested_class_is_refused_with_the_new_form`; `debugger.rs`: `a_breakpoint_with_a_version_stands_in_that_version_only`, `a_breakpoint_without_a_version_stands_in_every_version`, `frames_are_named_with_id_and_version`; `mcp.rs`: `a_method_location_names_a_nested_class_with_a_slash`; `debug_adapter.rs`: the stack trace's frame names of a two-version program.
- [ ] **Step 2: Run to fail.** **Step 3: Implement**; update every existing test that wrote `Module.Inner>>selector` to `Module/Inner>>selector`, and every expected frame name to its qualified form.
- [ ] **Step 4: Run to pass**, then `cargo xtask gates commit` (+ `codegen` if `interpreter/mod.rs` or `heap.rs` is touched).
- [ ] **Step 5: Commit** -- `feat: the debugger and its clients name modules by id and version`

### Task 7: The plugin's breakpoint types and library

**Files:**
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/debugger/NewspeakMethodBreakpointType.kt`, `NewspeakExceptionBreakpointType.kt` (the text's syntax and its hint), the library's naming if it names a module (`library/`), `docs/design/debugger.md` ("The JetBrains plugin")
- Test: the plugin's `breakpointTypesTest` and `debugProcessTest` sources under `editors/jetbrains/src/test/`

**Interfaces:**
- Consumes: Task 6's syntax.
- Produces: the method and exception breakpoint fields take and display `<id>[@<version>][/Inner…]`; a stored breakpoint in the old dotted form is shown with the agent's refusal, not rewritten silently.
- [ ] **Step 1: Write the failing tests**: `testAMethodBreakpointOnANestedClassStops` (`Demo/Account>>deposit:`), `testAnExceptionBreakpointByIdStops` (`newspeak.core.Kernel/Error`).
- [ ] **Step 2: Run to fail** (`editors/jetbrains/gradlew --no-daemon breakpointTypesTest`). **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat(plugin): breakpoints name a module by id, a class inside it after a slash`

### Task 8: The analyzer reports and shows module metadata

**Files:**
- Modify: `crates/ns-analyzer/src/` (the diagnostics source and the hover), `docs/design/editor-support.md`
- Test: `crates/ns-analyzer/tests/` (the server's diagnostics and hover tests)

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: diagnostics at the metadata's span for a module's `:module:` that `declared_identity` refuses (the fixed texts) and for a parameter's `:module:` whose payload is no `<id>` or `<id>@<range>`; the hover over a factory parameter shows its expectation (`expects com.acme.json.Json@2`), over a module's class its id and version.
- [ ] **Step 1: Write the failing tests**: `a_module_id_not_ending_in_its_class_is_reported`, `a_malformed_version_is_reported`, `a_reserved_id_is_reported`, `a_parameter_s_expectation_is_shown_on_hover`.
- [ ] **Step 2: Run to fail.** **Step 3: Implement.** **Step 4: Run to pass**, then `cargo xtask gates commit analyzer`.
- [ ] **Step 5: Commit** -- `feat: the analyzer reports and shows module metadata`

### Task 9: Documents and the end of the batch

**Files:**
- Modify: `docs/design/modules.md` (present tense, **Built**), `docs/design/README.md` (the map's row), `docs/design/image-format.md`, `core-library.md`, `debugger.md`, `editor-support.md`, `frontend.md` (as the tasks left them, read through once), `docs/OPEN-WORK.md` ("Next": the debugger's stage 6 again; the developer tool's item: stage 1 now names bodies by `id@version/Inner>>selector`), `docs/PROGRESS.md` (the milestone), `CLAUDE.md` (the crate map: `modules.rs`, `metadata.rs`, the project file's `as`, the manifest's `at:`)
- [ ] **Step 1:** Write the documents; a fresh session must be able to start from them alone.
- [ ] **Step 2:** `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification`; both pass.
- [ ] **Step 3: Commit** -- `docs: module identity, the string manifest and metadata are built`
