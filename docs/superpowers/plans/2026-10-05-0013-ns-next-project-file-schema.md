# The Project File's Schema Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The project file `newspeak.json` names its application module `main` instead of `root`, can name scripts -- argument lists `run` and `bench` hand to `main:args:` when given `--script` -- is validated strictly when it is read, and has a JSON Schema in the repository that `ns-next`'s reader is held to by a test and the JetBrains plugin hands to every file named `newspeak.json`. The plugin also runs a project: an `ns-next` path in its settings, a run configuration type "Newspeak", and a play button beside `main` and each script that runs or benchmarks the project and creates a run configuration.

**Architecture:** The schema `schemas/newspeak.schema.json` is the one description of the file. `crates/ns-cli/src/project.rs` enforces every rule it states with its own checks and messages: serde for shape and types, then a check of each path and of duplicates. A key neither names is ignored by both. No validator crate is added. A test interprets exactly the JSON Schema keywords the file uses and refuses any other. It runs a table of valid and invalid files through both the schema and the reader and requires them to agree, so neither can drift. The plugin's Gradle build copies the schema into its resources, and a `JsonSchemaProviderFactory`, behind an optional dependency on the bundled JSON plugin, maps it to `newspeak.json`. The run configuration type needs no JSON plugin and is always registered. The play buttons, and the producer that makes a configuration from a `newspeak.json`, read JSON PSI, so they are registered beside the schema provider, in the optional descriptor.

**Tech Stack:** Rust 1.98.0 (`serde`, `serde_json`, both already dependencies of `ns-cli`); Kotlin, IntelliJ Platform 2026.2.3, Gradle plugin `org.jetbrains.intellij.platform` 2.19.0; JSON Schema draft 2020-12.

**Spec:** this plan's decisions below, made with the user on 2026-10-05; the current project file is described in `docs/design/image-format.md`, "Packaging", which Task 7 rewrites. It builds on `54c021d6` ("a project is the directory its newspeak.json describes") on the same branch.

**Decided with the user (2026-10-05):**

1. **The application module's key is `main`**, replacing `root`. A file that still says `root` is refused for the missing `main`, as any file without one is. `root` itself is not singled out.
2. **Validation is strict in the reader, held to the schema by a test.** No JSON Schema validator crate is added. The reader states each rule with a message of its own, and a test keeps the schema and the reader in agreement.
3. **The rules:**
   - `main` is required. It and every entry of `modules` is a non-empty path ending in `.ns`.
   - `modules` is optional, and no path appears in it twice or equals `main`.
   - Other keys are allowed and ignored, by the reader and by the schema. `$schema` is one of them: the schema describes it as a string, so that editors without the plugin find the schema and get completion for it.
4. **The schema lives at `schemas/newspeak.schema.json`**, the one copy. The plugin's build copies it into its resources, and the plugin maps it to every file named `newspeak.json`.
5. **Scripts, as Node's are.** An optional `scripts` object maps a name to an array of strings. `ns-next run --script <name>` and `ns-next bench --script <name>` hand that array to `main:args:` as its arguments, each element one argument, exactly as if it were written after `--` on the command line. Arguments after `--` are always taken and follow the script's: `run --script timed -- Sieve` hands the script's arguments and then `Sieve`, as `npm run timed -- Sieve` does. Arguments stay strings everywhere, from a script as from the command line or an image, so a program sees one form however it is started. `--script` is for `run` and `bench` only: `build` does not send `main:args:`, so it has no arguments to take, and a script recorded into an image would need a section of the image format of its own.

6. **The plugin runs a project with the `ns-next` the user chooses.** Settings | Languages & Frameworks | Newspeak gains an "ns-next path" field beside the analyzer's. `ns-next` is not bundled. A run configuration whose path is empty, or names no file, refuses to start with an error that says so and offers to open the settings.
7. **A run configuration type "Newspeak", with the plugin's file icon** (`icons/newspeak.svg`, the 16-pixel icon `.ns` files carry), groups the configurations. A configuration runs `ns-next run` or `ns-next bench` on a project, with an optional script, extra arguments, and for `bench` the iterations.
8. **A play button beside `main` and beside each script** in a `newspeak.json`. Clicking it opens a menu with "Run" (`AllIcons.Run.Run`) and "Benchmark" (`AllIcons.Gutter.RunInQueue`), as every play button of the platform does. `main`'s runs without a script, a script's with `--script <name>`. Each creates the run configuration, or reuses an equal one, and starts it.

**Chosen by this plan, for review:**
- **Case of `.ns`.** The `.ns` suffix is matched case-sensitively, exactly as the schema's `pattern` matches it and as the plugin's file type names `ns`.
- **How duplicates are compared.** The schema's `uniqueItems` compares strings as written. The reader compares more: two spellings of one path, either separator or a doubled one, count as the same, and `main` counts against `modules` too. That stricter half cannot be stated in JSON Schema, so the test's table keeps it out of the cases where both must agree, and pins it on the reader alone.
- **`--script` refusals.** A `--script` given with a source file instead of a project is refused, and so is a name the project's `scripts` does not hold. The latter message lists the names it holds, or says it holds none. Nothing runs in either case.
- **A script's name is any key, a script's arguments any strings.** Empty strings are included: an empty argument is a valid command-line argument too.
- **The gutter icon** is the platform's run marker, `AllIcons.RunConfigurations.TestState.Run`, from the platform's `RunLineMarkerContributor`, which every language's play button uses. "Run" and "Benchmark" are the two entries of the menu it opens.
- **A configuration created from the gutter is temporary**, as the platform's are: it is reused while equal, can be saved with "Save Configuration", and is named `<project directory>`, `<project directory> [<script>]`, with ` (benchmark)` appended for `bench`.
- **Extra arguments** in the configuration are one field, split as the platform splits program parameters (`ParametersListUtil`), and go after `--`.
- **The plugin's dependency on the JSON plugin (`com.intellij.modules.json`) is optional.** It is declared like the spellchecker's, so an IDE without the JSON plugin still loads the Newspeak plugin.

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break" and "Conventions": names are written out (clippy's `min_ident_chars`); comments explain the decision; conventional commits; documentation describes the current state.
- No new external dependency, in Rust or in Gradle. The plugin's `bundledPlugin("com.intellij.modules.json")` is a platform module of the IDE it targets, not a dependency fetched from anywhere.
- **A golden whose outcome changes is a behaviour change.** Renaming `root` to `main` in the golden and benchmark project files changes no outcome. If a `.toml` must change, stop and report it.
- Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01DhtAcwQi6qo4racmc1iK4i`.
- Work happens on the checked-out branch `feature/newspeak-json` in the main working directory, with no worktree.
- **Per-commit gate**, every task:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets`, warning-free
  - `cargo nextest run --workspace`
  - `cargo test --workspace --doc`
  - `cargo run -p ns-analyzer -- parse-corpus corpus/modules` → "parsed 211, failed 0"
  - Tasks 4, 5 and 6 also run `./editors/jetbrains/gradlew -p editors/jetbrains check`.
  - A Markdown-only commit runs no gates.
- **End-of-batch gates.** They run once, in Task 8, through the `finishing-a-batch` skill: the five stress modes, `run-corpus` plain and under stress, the benchmarks profile, and `verifyPlugin buildPlugin`, since the plugin changes.
- Run every command in the foreground, with `timeout: 600000` on long ones.

## Review Focus

- **A file that says `root`.** A project file written before this change is refused for the missing `main`. It is never run with a missing application, and the message names `main`. Pinned in Task 1.
- **A path that differs only in spelling.** `"modules": ["lib/A.ns", "lib\\A.ns"]` is refused as a duplicate, and so is a module equal to `main`. Pinned in Task 2, on the reader alone, because the schema compares strings as written.
- **The schema and the reader drifting apart.** A rule added to one and not the other fails the agreement test. A schema keyword the test does not interpret fails it too, rather than being skipped. Pinned in Task 2.
- **A script and the command line together.** `--script` with arguments after `--`, `--script` with a source file, and a script name the project lacks. Each answers as the decisions say, and nothing runs on a refusal. Pinned in Task 3.
- **No `ns-next` configured.** A configuration started from the gutter, from the toolbar or from the context menu refuses to start, says that the ns-next path is not set, and offers the settings. A path naming no file is refused the same way. Pinned in Task 5, by `checkConfiguration`'s errors.
- **The play button only where it belongs, and what each click does.** It appears beside `main` and each script key in a file named `newspeak.json`. It does not appear in another JSON file, nor beside `modules`, nor beside a key named `main` nested elsewhere. Its menu holds exactly Run and Benchmark. Pinned in Task 6.
- **An IDE without the JSON plugin.** The plugin still loads, because the dependency is optional. The provider is registered only in the optional descriptor. Checked in Task 4 by `verifyPlugin`'s descriptor checks and by the build. The test sandbox has the JSON plugin, so no test runs without it.
- **A `newspeak.json` in the IDE gets the schema, and no other JSON file does.** Pinned in Task 4.

---

## File Structure

| File | Responsibility |
|---|---|
| `schemas/newspeak.schema.json` (new, Task 2) | the project file's JSON Schema, the one copy |
| `crates/ns-cli/src/project.rs` | `main`, other keys ignored, the path and duplicate rules, their messages (Tasks 1, 2) |
| `crates/ns-cli/tests/project_schema.rs` (new, Task 2) | the agreement test: a minimal interpreter of the schema's keywords, and the table of valid and invalid files |
| `crates/ns-cli/src/main.rs`, `crates/ns-cli/src/program.rs`, `crates/ns-cli/src/commands/{run,bench}.rs` | `--script` and the arguments it selects (Task 3) |
| `crates/ns-cli/src/commands/run_corpus.rs`, `crates/ns-cli/tests/{run,build_load}.rs`, `goldens/tool/**/newspeak.json`, `benchmarks/are-we-fast-yet/newspeak.json` | `root` → `main` (Task 1) |
| `editors/jetbrains/build.gradle.kts` | the JSON plugin on the classpath; the schema copied into the resources (Task 4) |
| `editors/jetbrains/src/main/resources/META-INF/plugin.xml`, `newspeak-json.xml` (new) | the optional dependency and the provider's registration (Task 4) |
| `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/project/NewspeakJsonSchemaProviderFactory.kt` (new) | maps the bundled schema to `newspeak.json` (Task 4) |
| `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/project/NewspeakJsonSchemaProviderFactoryTest.kt` (new) | Task 4 |
| `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/settings/{NewspeakSettings,NewspeakSettingsConfigurable}.kt` | the ns-next path (Task 5) |
| `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/` (new) | `RuntimeLocation`, `NewspeakConfigurationType`, `NewspeakRunConfiguration` and its options, editor and state (Task 5); `NewspeakRunLineMarkerContributor`, `NewspeakRunAction`, `NewspeakRunConfigurationProducer`, `NewspeakProjectJson` (Task 6) |
| `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/` (new) | Tasks 5 and 6 |
| `docs/**`, `README.md`, `CLAUDE.md` | Task 7 |

---

### Task 1: The application module's key is `main`

**Files:**
- Modify: `crates/ns-cli/src/project.rs`, `crates/ns-cli/src/commands/run_corpus.rs`
- Modify: every `newspeak.json` under `goldens/` (seven) and `benchmarks/are-we-fast-yet/newspeak.json`
- Modify: the tests that write a project file: `crates/ns-cli/tests/run.rs`, `crates/ns-cli/tests/build_load.rs`, and `project.rs`'s own test module

- [ ] **Step 1: The failing test**

In `project.rs`'s test module, `an_unknown_field_and_a_missing_root_are_refused` becomes two tests: other keys are now accepted, and a missing `main` is refused, whatever else the file holds:

```rust
#[test]
fn a_file_without_main_is_refused_naming_main_whatever_else_it_holds() {
    let directory = scratch("missing-main");
    for (name, contents) in [
        ("legacy.json", r#"{ "root": "App.ns" }"#),
        ("empty.json", r#"{ "modules": [] }"#),
    ] {
        let path = directory.join(name);
        std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
        let error = refusal(&path);
        assert!(error.contains("`main`"), "{error}");
    }
}

#[test]
fn other_keys_are_ignored() {
    let directory = scratch("other-keys");
    let path = directory.join(PROJECT_FILE_NAME);
    std::fs::write(
        &path,
        r#"{ "$schema": "../schemas/newspeak.schema.json", "main": "App.ns", "entry": "x" }"#,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let project = read_project(&path).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(project.main, directory.join("App.ns"));
}
```

(serde's own message for a missing field reads "missing field `main`", which carries the backquoted key the test looks for.)

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-cli --lib project`
Expected: FAIL. `root` is still the key, so the legacy file is accepted, and `deny_unknown_fields` refuses the other keys.

- [ ] **Step 3: Rename the key**

In `ProjectFile`, `root: String` becomes `main: String`, and `#[serde(deny_unknown_fields)]` goes, with its doc comment rewritten: other keys are ignored, so that a file may carry what an editor or another tool wants beside what ns-next reads. In `Project`, `root: PathBuf` becomes `main: PathBuf`. `read_project` resolves `file.main`. In `program.rs`, `project.root` becomes `project.main`. In `run_corpus.rs`, the wrapper's JSON says `"main": "RunCorpusEntry.ns"`. Rewrite each doc comment that says `root` for the project file's key: the module doc of `project.rs`, and `ProjectFile`'s. Where "root" means the application's root *class*, it stays, as in `program.rs`'s `instantiate_application`.

- [ ] **Step 4: Rename it in every project file and test**

Run `sed -i 's/"root":/"main":/'` over the seven golden `newspeak.json` files and `benchmarks/are-we-fast-yet/newspeak.json`. Make the same replacement in `crates/ns-cli/tests/run.rs` and `crates/ns-cli/tests/build_load.rs`, including inside `indoc!` blocks. Check that nothing else in the workspace still writes `"root"` into a project file: `grep -rn '"root"' crates goldens benchmarks`.

- [ ] **Step 5: The per-commit gate, then commit**

Expected: everything passes, and no golden's `.toml` changes.

```bash
git add -A
git commit -m "feat: the project file names its application module main"
```

---

### Task 2: The schema, the reader's rules, and the test that holds them together

**Files:**
- Create: `schemas/newspeak.schema.json`
- Modify: `crates/ns-cli/src/project.rs`
- Create: `crates/ns-cli/tests/project_schema.rs`

**Interfaces:**
- Produces: `schemas/newspeak.schema.json`, read by the Rust test, extended by Task 3, and copied by Task 4's Gradle build. `project::read_project` keeps its signature and refuses more.

- [ ] **Step 1: The schema**

`schemas/newspeak.schema.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ns-next project",
  "description": "A Newspeak program of several modules, run by ns-next: `main` is the application's module, instantiated with `packageUsing:` a manifest of the others. Paths are relative to this file, with either separator.",
  "type": "object",
  "properties": {
    "$schema": {
      "description": "Where an editor finds this schema; ns-next ignores it.",
      "type": "string"
    },
    "main": {
      "description": "The application's module: its root class is instantiated with `packageUsing:` the manifest, then sent `main: platform args:`.",
      "type": "string",
      "minLength": 1,
      "pattern": "\\.ns$"
    },
    "modules": {
      "description": "The other modules the manifest holds, each reached as `manifest <Name>` by the name its source declares.",
      "type": "array",
      "items": {
        "type": "string",
        "minLength": 1,
        "pattern": "\\.ns$"
      },
      "uniqueItems": true
    }
  },
  "required": ["main"]
}
```

- [ ] **Step 2: The failing reader tests**

In `project.rs`'s test module:

```rust
/// Writes `contents` as a project file in a fresh scratch directory and
/// answers the reader's refusal.
fn refusal_of(name: &str, contents: &str) -> String {
    let directory = scratch(name);
    let path = directory.join(PROJECT_FILE_NAME);
    std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
    refusal(&path)
}

#[test]
fn a_path_that_is_empty_or_names_no_ns_file_is_refused_naming_its_key() {
    let error = refusal_of("empty-main", r#"{ "main": "" }"#);
    assert!(error.contains("`main`") && error.contains("empty"), "{error}");
    let error = refusal_of("main-not-ns", r#"{ "main": "App.txt" }"#);
    assert!(error.contains("`main`") && error.contains(".ns"), "{error}");
    let error = refusal_of("module-not-ns", r#"{ "main": "App.ns", "modules": ["A.ns", "B"] }"#);
    assert!(error.contains("`modules`") && error.contains("B"), "{error}");
}

#[test]
fn a_module_named_twice_or_equal_to_main_is_refused_in_any_spelling() {
    let error = refusal_of(
        "duplicate-spellings",
        r#"{ "main": "App.ns", "modules": ["lib/A.ns", "lib\\A.ns"] }"#,
    );
    assert!(error.contains("twice"), "{error}");
    let error = refusal_of("module-is-main", r#"{ "main": "App.ns", "modules": ["./App.ns"] }"#);
    assert!(error.contains("`main`"), "{error}");
}

```

Run: `cargo nextest run -p ns-cli --lib project`
Expected: FAIL. The empty and non-`.ns` paths and the duplicates are accepted.

- [ ] **Step 3: The reader's rules**

In `read_project`, after deserializing and before resolving, check each path:

```rust
/// Refuses a path that is empty or names no `.ns` file, naming the key it
/// was written under; the schema's `minLength` and `pattern` say the same.
fn check_module_path(key: &str, written: &str) -> Result<(), String> {
    if written.is_empty() {
        return Err(format!("`{key}` holds an empty path"));
    }
    if !written.ends_with(".ns") {
        return Err(format!("`{key}` names `{written}`, which is no `.ns` file"));
    }
    Ok(())
}
```

Call it with the file's path in the message, as `read_project`'s other errors carry it: `format!("{}: {error}", path.display())`. Call it for `main` and for each entry of `modules`. Then resolve every path, and refuse a duplicate by comparing the resolved paths' `components()`:
- a module whose components equal `main`'s: "`modules` names `main`'s module `{written}` again";
- two modules with equal components: "`modules` names `{written}` twice".

The components are those of `resolve_written_path`'s answer, with a `.` component dropped. `Path::components` already folds a doubled separator. That is what makes `lib/A.ns` and `lib\A.ns`, and `App.ns` and `./App.ns`, one path.

Remove the item "A project file with an empty `root`" from `docs/OPEN-WORK.md`, since this task closes it. That Markdown change goes in this task's commit.

- [ ] **Step 4: Run the reader tests**

Run: `cargo nextest run -p ns-cli --lib project`
Expected: PASS.

- [ ] **Step 5: The agreement test**

`crates/ns-cli/tests/project_schema.rs`. It interprets the schema with the keywords the schema uses and nothing else, and runs one table of files through the schema and through `ns-next run`, which reads the project file before it reads any module. The binary is the test's way to reach the reader, because `project.rs` is private to the binary crate. A file the schema accepts must reach the modules: its run fails because `App.ns` does not exist, not because of the project file. A file the schema refuses must be refused before any module is read.

```rust
//! Holds the project file's schema (`schemas/newspeak.schema.json`) and
//! `ns-next`'s reader to one another: every file of the table is valid
//! under both or under neither. The schema is interpreted here with exactly
//! the keywords it uses, and a keyword this interpreter does not know fails
//! the test rather than being skipped, so that the schema cannot gain a rule
//! that nothing checks the reader against.

mod common;

use common::ScratchDirectory;
use serde_json::Value;
use std::process::Command;

fn schema() -> Value {
    let path = common::goldens_root().join("../schemas/newspeak.schema.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Whether `instance` is valid under `schema`, for the keywords the
/// project file's schema uses.
fn valid(schema: &Value, instance: &Value) -> bool {
    let Value::Object(keywords) = schema else {
        panic!("a schema is an object: {schema}")
    };
    keywords.iter().all(|(keyword, argument)| match keyword.as_str() {
        "$schema" | "title" | "description" => true,
        "type" => match argument.as_str() {
            Some("object") => instance.is_object(),
            Some("array") => instance.is_array(),
            Some("string") => instance.is_string(),
            other => panic!("the interpreter knows no type {other:?}"),
        },
        "properties" => {
            let (Some(properties), Some(members)) = (argument.as_object(), instance.as_object())
            else {
                return true;
            };
            members.iter().all(|(name, member)| {
                properties.get(name).is_none_or(|property| valid(property, member))
            })
        }
        "required" => {
            let Some(members) = instance.as_object() else { return true };
            argument
                .as_array()
                .expect("`required` is an array")
                .iter()
                .all(|name| members.contains_key(name.as_str().expect("a required name")))
        }
        "items" => instance
            .as_array()
            .is_none_or(|elements| elements.iter().all(|element| valid(argument, element))),
        "uniqueItems" => {
            assert_eq!(argument, &Value::Bool(true), "only `true` is interpreted");
            instance.as_array().is_none_or(|elements| {
                elements
                    .iter()
                    .enumerate()
                    .all(|(index, element)| !elements[..index].contains(element))
            })
        }
        "minLength" => instance.as_str().is_none_or(|text| {
            text.chars().count() as u64 >= argument.as_u64().expect("a length")
        }),
        "pattern" => {
            // The only pattern the schema uses: a literal suffix.
            assert_eq!(argument.as_str(), Some("\\.ns$"), "the interpreter knows only `\\.ns$`");
            instance.as_str().is_none_or(|text| text.ends_with(".ns"))
        }
        other => panic!("the schema uses `{other}`, which this interpreter does not know"),
    })
}

/// The table: each file, and whether it is a valid project file.
const FILES: &[(&str, bool)] = &[
    (r#"{ "main": "App.ns" }"#, true),
    (r#"{ "main": "App.ns", "modules": [] }"#, true),
    (r#"{ "main": "App.ns", "modules": ["A.ns", "lib/B.ns"] }"#, true),
    (r#"{ "$schema": "x", "main": "App.ns" }"#, true),
    (r#"{ }"#, false),
    (r#"{ "root": "App.ns" }"#, false),
    (r#"{ "main": "App.ns", "entry": "x" }"#, true),
    (r#"{ "main": "" }"#, false),
    (r#"{ "main": "App" }"#, false),
    (r#"{ "main": 1 }"#, false),
    (r#"{ "main": "App.ns", "modules": "A.ns" }"#, false),
    (r#"{ "main": "App.ns", "modules": [""] }"#, false),
    (r#"{ "main": "App.ns", "modules": ["A.txt"] }"#, false),
    (r#"{ "main": "App.ns", "modules": ["A.ns", "A.ns"] }"#, false),
    (r#"{ "$schema": 1, "main": "App.ns" }"#, false),
    (r#"[]"#, false),
];

/// Whether `ns-next run` reads `contents` as a project file: a valid one
/// gets as far as its missing `App.ns`; an invalid one is refused before.
fn reader_accepts(contents: &str, index: usize) -> bool {
    let directory = ScratchDirectory::new(&format!("project-schema-{index}"));
    std::fs::write(directory.join("newspeak.json"), contents).expect("write the file");
    let out = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(directory.path())
        .output()
        .expect("run ns-next");
    assert!(!out.status.success(), "no module exists, so nothing may run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr.contains("App.ns") && !stderr.contains("newspeak.json")
}

#[test]
fn the_schema_and_the_reader_agree_on_every_file() {
    let schema = schema();
    for (index, (contents, expected)) in FILES.iter().enumerate() {
        let instance: Value = serde_json::from_str(contents).expect("the table holds JSON");
        assert_eq!(valid(&schema, &instance), *expected, "the schema on {contents}");
        assert_eq!(reader_accepts(contents, index), *expected, "the reader on {contents}");
    }
}
```

Before relying on `reader_accepts`, check its premise against the code. A refusal from `read_project` must name the project file's path, which ends in `newspeak.json`, and must not name `App.ns` alone. A valid file's failure must name the missing `App.ns` without naming `newspeak.json`. If the messages do not separate the two cases this way, change what the helper looks for, for example the `error: <path>:` prefix `read_program` prints, and say so in the report. Never weaken a table row.

Run: `cargo nextest run -p ns-cli --test project_schema`
Expected: PASS. Also make one deliberate break, then revert it: add `"maxLength": 100` to the schema and see the test fail on the unknown keyword.

- [ ] **Step 6: The per-commit gate, then commit**

```bash
git add -A
git commit -m "feat: the project file's schema, and a reader held to it"
```

---

### Task 3: Scripts select `main:args:`'s arguments

**Files:**
- Modify: `schemas/newspeak.schema.json`, `crates/ns-cli/tests/project_schema.rs`
- Modify: `crates/ns-cli/src/project.rs`, `crates/ns-cli/src/program.rs`, `crates/ns-cli/src/main.rs`, `crates/ns-cli/src/commands/run.rs`, `crates/ns-cli/src/commands/bench.rs`
- Test: `crates/ns-cli/tests/run.rs`, `crates/ns-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 2's schema, interpreter and table, and `project::read_project`.
- Produces: `Project::scripts: BTreeMap<String, Vec<String>>`; `Program::scripts: Option<(PathBuf, BTreeMap<String, Vec<String>>)>`, with the project file's path for messages and `None` for a source file; `program::entry_arguments(program: &Program, script: Option<&str>, arguments: &[String]) -> Result<Vec<String>, ExitCode>`.

- [ ] **Step 1: The schema and the agreement test**

Add to the schema's `properties`:

```json
"scripts": {
  "description": "Named argument lists: `ns-next run --script <name>` and `ns-next bench --script <name>` hand one to `main:args:`, each element one argument, as if written after `--`; arguments after `--` follow it.",
  "type": "object",
  "additionalProperties": {
    "type": "array",
    "items": { "type": "string" }
  }
}
```

In `project_schema.rs`'s interpreter, interpret `additionalProperties` when its argument is a schema. Every member that `properties` does not name must then be valid under it. Any other form stays unknown and panics:

```rust
"additionalProperties" => {
    assert!(argument.is_object(), "only a schema is interpreted");
    let Some(members) = instance.as_object() else { return true };
    let named = keywords.get("properties").and_then(Value::as_object);
    members
        .iter()
        .filter(|(name, _)| named.is_none_or(|named| !named.contains_key(*name)))
        .all(|(_, member)| valid(argument, member))
}
```

Add these rows to the table:

```rust
(r#"{ "main": "App.ns", "scripts": {} }"#, true),
(r#"{ "main": "App.ns", "scripts": { "start": [], "timed": ["--timed", ""] } }"#, true),
(r#"{ "main": "App.ns", "scripts": ["--timed"] }"#, false),
(r#"{ "main": "App.ns", "scripts": { "timed": "--timed" } }"#, false),
(r#"{ "main": "App.ns", "scripts": { "timed": [1] } }"#, false),
```

Run: `cargo nextest run -p ns-cli --test project_schema`
Expected: FAIL on the reader's side. It ignores `scripts` as an unknown key, so it accepts the three invalid rows.

- [ ] **Step 2: The reader takes the scripts**

In `ProjectFile`, add `#[serde(default)] scripts: BTreeMap<String, Vec<String>>`. serde refuses an array, a string value or a non-string element with its own message, which names the key. In `Project`, add `scripts: BTreeMap<String, Vec<String>>`, carried over unchanged. A `BTreeMap` keeps the names sorted, for the message that lists them.

Run: `cargo nextest run -p ns-cli --test project_schema`
Expected: PASS.

- [ ] **Step 3: The failing CLI tests**

In `crates/ns-cli/tests/run.rs`, with a project whose `main` prints its arguments, one per line:

```rust
/// A root that prints each of `main:args:`'s arguments on its own line.
const ECHO: &str = indoc! {"
    class Echo packageUsing: manifest = Object new () (
        public main: platform args: arguments = (
            arguments do: [:argument | platform transcript show: argument; newLine]
        )
    ) : ()
"};

fn echo_project(name: &str) -> ScratchDirectory {
    let directory = ScratchDirectory::new(name);
    std::fs::write(directory.join("Echo.ns"), ECHO).unwrap();
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "Echo.ns", "scripts": { "two": ["first", "second word"], "none": [] } }"#,
    )
    .unwrap();
    directory
}

#[test]
fn a_script_hands_its_arguments_to_main_and_the_command_line_follows() {
    let directory = echo_project("script-arguments");
    let out = ns_next()
        .args(["run", "--script", "two"])
        .arg(directory.path())
        .args(["--", "third"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "first\nsecond word\nthird\n");
}

#[test]
fn an_unknown_script_is_refused_naming_the_scripts_there_are() {
    let directory = echo_project("script-unknown");
    let out = ns_next()
        .args(["run", "--script", "three"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "nothing runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("three") && stderr.contains("none, two"), "stderr: {stderr}");
}

#[test]
fn a_script_with_a_source_file_is_refused() {
    let directory = ScratchDirectory::new("script-source");
    std::fs::write(directory.join("Hello.ns"), HELLO).unwrap();
    let out = ns_next()
        .args(["run", "--script", "two"])
        .arg(directory.join("Hello.ns"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "nothing runs");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--script"));
}
```

`HELLO` stands for the module's existing single-source fixture. Use whichever constant `run.rs` already has for a `.ns` program started with `new`, or add a one-line one. `Echo`'s `do:` over an `Array` and the transcript's `show:` are what the goldens under `goldens/library/platform/` already use. If a fixture there prints arguments, copy its form exactly. In `crates/ns-cli/tests/cli.rs`, add one test that `bench --script two <dir> --iterations 1` exits 0 and prints its timing line, as `benchmark_takes_a_project_file` does.

Run: `cargo nextest run -p ns-cli --test run --test cli`
Expected: FAIL, because `--script` is not an argument yet.

- [ ] **Step 4: `--script` and the arguments it selects**

In `main.rs`, `Run` and `Bench` each gain:

```rust
/// A script of the project's `newspeak.json`: its arguments go to
/// `main:args:` first, then any given after `--`.
#[arg(long)]
script: Option<String>,
```

Pass it through to `commands::run::execute` and `commands::bench::execute`. Each calls, after `read_program`:

```rust
let arguments = match program::entry_arguments(&program, script.as_deref(), program_arguments) {
    Ok(arguments) => arguments,
    Err(code) => return code,
};
```

and uses `arguments` wherever it used `program_arguments`. In `program.rs`, `Program` gains `scripts`: `Some((project file path, project.scripts))` for a project, `None` for a source file.

```rust
/// What `main:args:` receives: the script's arguments, when `script` names
/// one, then those given after `--`. A script is a project's, so naming one
/// for a source file, or one the project lacks, is refused before anything
/// runs.
pub(crate) fn entry_arguments(
    program: &Program,
    script: Option<&str>,
    arguments: &[String],
) -> Result<Vec<String>, ExitCode> {
    let Some(name) = script else {
        return Ok(arguments.to_vec());
    };
    let Some((project_file, scripts)) = &program.scripts else {
        eprintln!(
            "error: --script names a script of a project's newspeak.json, and the program \
             given is a source file"
        );
        return Err(ExitCode::FAILURE);
    };
    let Some(script_arguments) = scripts.get(name) else {
        let known = if scripts.is_empty() {
            "it has none".to_string()
        } else {
            format!(
                "its scripts are {}",
                scripts.keys().map(String::as_str).collect::<Vec<_>>().join(", ")
            )
        };
        eprintln!("error: {} has no script `{name}`; {known}", project_file.display());
        return Err(ExitCode::FAILURE);
    };
    Ok(script_arguments.iter().chain(arguments).cloned().collect())
}
```

Check `bench`'s untimed setup and every timed run: each `main:args:` send must use the combined arguments, not only the first.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p ns-cli`
Expected: PASS.

- [ ] **Step 6: The per-commit gate, then commit**

```bash
git add -A
git commit -m "feat: scripts in newspeak.json select main:args:'s arguments for run and bench"
```

---

### Task 4: The plugin gives every newspeak.json the schema

**Files:**
- Modify: `editors/jetbrains/build.gradle.kts`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`
- Create: `editors/jetbrains/src/main/resources/META-INF/newspeak-json.xml`
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/project/NewspeakJsonSchemaProviderFactory.kt`
- Create: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/project/NewspeakJsonSchemaProviderFactoryTest.kt`

**Interfaces:**
- Consumes: `schemas/newspeak.schema.json` (Task 2).
- The JSON plugin in IntelliJ 2026.2.3 has the id `com.intellij.modules.json`. Its extension point `JavaScript.JsonSchema.ProviderFactory` takes a `com.jetbrains.jsonSchema.extension.JsonSchemaProviderFactory`, whose `getProviders(project)` answers `JsonSchemaFileProvider`s (`isAvailable(file)`, `getName()`, `getSchemaFile()`, `getSchemaType()`). `JsonSchemaProviderFactory.getResourceFile(Class, String)` finds a bundled resource. These names were checked against the distribution in the Gradle cache on 2026-10-05: `plugins/json/lib/intellij.json.jar`'s `plugin.xml`, and `modules/intellij.json.backend.jar`.

- [ ] **Step 1: Build wiring**

In `build.gradle.kts`'s `intellijPlatform` dependencies:

```kotlin
// The JSON plugin, for the schema it gives `newspeak.json`. plugin.xml
// declares the dependency optional, as the spellchecker's.
bundledPlugin("com.intellij.modules.json")
```

Copy the schema from the repository's one copy:

```kotlin
// The project file's schema has one copy, at the repository root, which
// ns-next's reader is held to; the plugin bundles that copy.
tasks.processResources {
    from(repositoryRoot.resolve("schemas/newspeak.schema.json")) {
        into("schemas")
    }
}
```

(`processResources` declares its `from` as an input, so a change to the schema rebuilds the plugin.)

- [ ] **Step 2: The failing test**

```kotlin
package io.github.bstrauss.newspeak.project

import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.jetbrains.jsonSchema.ide.JsonSchemaService

class NewspeakJsonSchemaProviderFactoryTest : BasePlatformTestCase() {
    fun testANewspeakJsonGetsTheProjectSchema() {
        val file = myFixture.configureByText("newspeak.json", """{ "main": "App.ns" }""")
        val schemas = JsonSchemaService.Impl.get(project).getSchemaFilesForFile(file.virtualFile)
        assertTrue(schemas.any { schema -> schema.name == "newspeak.schema.json" })
    }

    fun testAnotherJsonFileDoesNot() {
        val file = myFixture.configureByText("package.json", "{}")
        val schemas = JsonSchemaService.Impl.get(project).getSchemaFilesForFile(file.virtualFile)
        assertFalse(schemas.any { schema -> schema.name == "newspeak.schema.json" })
    }

    fun testAMissingMainIsReported() {
        myFixture.enableInspections(com.jetbrains.jsonSchema.impl.inspections.JsonSchemaComplianceInspection::class.java)
        myFixture.configureByText(
            "newspeak.json",
            """<warning>{ "root": "App.ns" }</warning>""",
        )
        myFixture.checkHighlighting(true, false, false)
    }
}
```

`JsonSchemaService.Impl.get` and the inspection's class name are the platform's as of 2026.2. If either has moved, find the current name in the JSON plugin's jars and use it. The third test's exact markup depends on what the inspection highlights: the property's key, its value, or the whole property. Run it once, read the highlighting it reports, and pin that.

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests '*NewspeakJsonSchemaProviderFactoryTest*'`
Expected: FAIL, since no provider exists yet.

- [ ] **Step 3: The provider**

```kotlin
package io.github.bstrauss.newspeak.project

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.jetbrains.jsonSchema.extension.JsonSchemaFileProvider
import com.jetbrains.jsonSchema.extension.JsonSchemaProviderFactory
import com.jetbrains.jsonSchema.extension.SchemaType

/**
 * Gives every file named `newspeak.json` -- ns-next's project file, which it
 * recognises by that name alone -- the schema bundled from the repository's
 * `schemas/newspeak.schema.json`, the one ns-next's reader is held to.
 */
class NewspeakJsonSchemaProviderFactory : JsonSchemaProviderFactory {
    override fun getProviders(project: Project): List<JsonSchemaFileProvider> = listOf(Provider)

    private object Provider : JsonSchemaFileProvider {
        override fun isAvailable(file: VirtualFile): Boolean = file.name == PROJECT_FILE_NAME

        override fun getName(): String = "ns-next project"

        override fun getSchemaFile(): VirtualFile? =
            JsonSchemaProviderFactory.getResourceFile(
                NewspeakJsonSchemaProviderFactory::class.java,
                "/schemas/newspeak.schema.json",
            )

        override fun getSchemaType(): SchemaType = SchemaType.embeddedSchema
    }

    private companion object {
        /** `crates/ns-cli/src/project.rs`'s `PROJECT_FILE_NAME`. */
        const val PROJECT_FILE_NAME = "newspeak.json"
    }
}
```

In `plugin.xml`, beside the spellchecker's line:

```xml
<!-- Optional: an IDE built without the JSON plugin still loads this plugin,
     with no schema for newspeak.json. -->
<depends optional="true" config-file="newspeak-json.xml">com.intellij.modules.json</depends>
```

`META-INF/newspeak-json.xml`:

```xml
<idea-plugin>
    <extensions defaultExtensionNs="JavaScript.JsonSchema">
        <ProviderFactory implementation="io.github.bstrauss.newspeak.project.NewspeakJsonSchemaProviderFactory"/>
    </extensions>
</idea-plugin>
```

Add one bullet to `plugin.xml`'s description: "Completion and validation in `newspeak.json`, ns-next's project file, from its schema." Also add a sentence to the change notes for 0.1.0.

- [ ] **Step 4: Run the plugin's checks**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: PASS, including the lexer comparison, which this task does not touch.

- [ ] **Step 5: The per-commit gate, then commit**

```bash
git add -A
git commit -m "feat: the JetBrains plugin gives every newspeak.json the project schema"
```

---

### Task 5: The plugin's ns-next path and the "Newspeak" run configuration

**Files:**
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/settings/NewspeakSettings.kt`, `NewspeakSettingsConfigurable.kt`
- Create in `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/`: `RuntimeLocation.kt`, `NewspeakConfigurationType.kt`, `NewspeakRunConfiguration.kt` (the configuration, its options, its command line), `NewspeakRunConfigurationEditor.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/RuntimeLocationTest.kt`, `NewspeakRunConfigurationTest.kt`

**Interfaces:**
- Produces, used by Task 6:
  - `NewspeakConfigurationType.getInstance()`, its one factory `NewspeakConfigurationType.getInstance().factory`, and its id `"NewspeakRunConfiguration"`.
  - `NewspeakRunConfiguration` with the properties `projectFile: String?` (the `newspeak.json` path), `command: NewspeakCommand` (`RUN` | `BENCH`), `script: String?`, `programArguments: String?` and `iterations: Int` (10 by default).
  - `fun newspeakCommandLine(runtime: Path, configuration: NewspeakRunConfiguration): List<String>`.
- The platform's names, as of 2026.2: `ConfigurationTypeBase`, `ConfigurationFactory`, `RunConfigurationBase<RunConfigurationOptions>`, `CommandLineState`, `KillableColoredProcessHandler`, `ProcessTerminatedListener`, `RuntimeConfigurationError`, `ShowSettingsUtil`, `ParametersListUtil`. Verify each against the platform jars before relying on it, and use the current name if one has moved.

- [ ] **Step 1: The failing tests**

`RuntimeLocationTest.kt`, pure, with no IDE, like `ServerBinaryLocatorTest`:

```kotlin
package io.github.bstrauss.newspeak.run

import java.nio.file.Files
import kotlin.io.path.createTempDirectory
import org.junit.Assert.assertEquals
import org.junit.Test

class RuntimeLocationTest {
    @Test
    fun anEmptyPathIsNotConfigured() {
        assertEquals(RuntimeLocation.NotConfigured, locateRuntime(null))
        assertEquals(RuntimeLocation.NotConfigured, locateRuntime("  "))
    }

    @Test
    fun aPathNamingNoFileIsReportedAsWritten() {
        assertEquals(RuntimeLocation.ConfiguredPathMissing("/no/such/ns-next"), locateRuntime("/no/such/ns-next"))
    }

    @Test
    fun anExistingFileIsFound() {
        val runtime = Files.createFile(createTempDirectory("ns-next-runtime").resolve("ns-next"))
        assertEquals(RuntimeLocation.Found(runtime), locateRuntime(runtime.toString()))
    }
}
```

`NewspeakRunConfigurationTest.kt`, a `BasePlatformTestCase`:

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.nio.file.Path

class NewspeakRunConfigurationTest : BasePlatformTestCase() {
    private fun configuration(): NewspeakRunConfiguration =
        NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration

    override fun tearDown() {
        try {
            NewspeakSettings.getInstance().state.runtimePath = null
        } finally {
            super.tearDown()
        }
    }

    fun testRunWithAScriptAndArguments() {
        val run = configuration().apply {
            projectFile = "/work/app/newspeak.json"
            command = NewspeakCommand.RUN
            script = "timed"
            programArguments = "Sieve \"Delta Blue\""
        }
        assertEquals(
            listOf("/bin/ns-next", "run", "--script", "timed", "/work/app", "--", "Sieve", "Delta Blue"),
            newspeakCommandLine(Path.of("/bin/ns-next"), run),
        )
    }

    fun testBenchWithItsIterationsAndNoScript() {
        val bench = configuration().apply {
            projectFile = "/work/app/newspeak.json"
            command = NewspeakCommand.BENCH
            iterations = 3
        }
        assertEquals(
            listOf("/bin/ns-next", "bench", "--iterations", "3", "/work/app"),
            newspeakCommandLine(Path.of("/bin/ns-next"), bench),
        )
    }

    fun testNoRuntimeConfiguredIsAnErrorNamingTheSetting() {
        NewspeakSettings.getInstance().state.runtimePath = null
        val run = configuration().apply { projectFile = "/work/app/newspeak.json" }
        val error = assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("ns-next path"))
    }

    fun testAMissingRuntimeFileIsAnError() {
        NewspeakSettings.getInstance().state.runtimePath = "/no/such/ns-next"
        val run = configuration().apply { projectFile = "/work/app/newspeak.json" }
        assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
    }
}
```

On Windows the paths are compared as `Path.of(...)` renders them, so build the expected list from `Path.of("/work/app").toString()` where a path appears. `assertThrows` is JUnit 4.13's `org.junit.Assert.assertThrows`.

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests '*run.*'`
Expected: FAIL, since nothing exists yet.

- [ ] **Step 2: The setting**

In `NewspeakSettings.Settings`:

```kotlin
/** The ns-next runtime a Newspeak run configuration starts; null until the user chooses one. */
var runtimePath by string()
```

In `NewspeakSettingsConfigurable`, a second row below the analyzer's, built the same way:

```kotlin
row("ns-next path:") {
    textFieldWithBrowseButton(
        FileChooserDescriptorFactory.singleFile().withTitle("Choose the ns-next Executable"),
    )
        .bindText({ settings.runtimePath.orEmpty() }, { text -> settings.runtimePath = text.trim().ifEmpty { null } })
        .align(AlignX.FILL)
        .comment(
            "The ns-next runtime that Newspeak run configurations start; build it with " +
                "cargo build -p ns-cli --release. It is not bundled with the plugin."
        )
}
```

`apply()` keeps restarting the language servers. A changed runtime path needs no restart, since every run reads the setting afresh.

- [ ] **Step 3: Where the runtime is**

`RuntimeLocation.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import java.nio.file.Files
import java.nio.file.Path

/** Where the ns-next runtime is, or why it cannot be started. */
sealed interface RuntimeLocation {
    data class Found(val path: Path) : RuntimeLocation
    /** The setting is empty: the plugin bundles no runtime to fall back on. */
    data object NotConfigured : RuntimeLocation
    /** A path is set and nothing is there. */
    data class ConfiguredPathMissing(val path: String) : RuntimeLocation
}

/** Pure over the configured path, so the rule is testable without an IDE. */
fun locateRuntime(configuredPath: String?): RuntimeLocation {
    val configured = configuredPath?.trim().orEmpty()
    if (configured.isEmpty()) return RuntimeLocation.NotConfigured
    val path = Path.of(configured)
    return if (Files.isRegularFile(path)) RuntimeLocation.Found(path) else RuntimeLocation.ConfiguredPathMissing(configured)
}
```

- [ ] **Step 4: The type, the configuration, its command line**

`NewspeakConfigurationType.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.ConfigurationTypeUtil
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.openapi.project.Project
import io.github.bstrauss.newspeak.NewspeakIcons

/** The "Newspeak" group of run configurations, with the plugin's file icon. */
class NewspeakConfigurationType : ConfigurationTypeBase(
    ID,
    "Newspeak",
    "Runs or benchmarks a Newspeak project with ns-next",
    NewspeakIcons.FILE,
) {
    init {
        addFactory(Factory(this))
    }

    val factory: ConfigurationFactory get() = configurationFactories.single()

    private class Factory(type: NewspeakConfigurationType) : ConfigurationFactory(type) {
        override fun getId(): String = ID
        override fun createTemplateConfiguration(project: Project): RunConfiguration =
            NewspeakRunConfiguration(project, this, "Newspeak")
        override fun getOptionsClass(): Class<NewspeakRunConfigurationOptions> =
            NewspeakRunConfigurationOptions::class.java
    }

    companion object {
        const val ID = "NewspeakRunConfiguration"
        fun getInstance(): NewspeakConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(NewspeakConfigurationType::class.java)
    }
}
```

`NewspeakRunConfiguration.kt` holds the following.
- `enum class NewspeakCommand(val subcommand: String) { RUN("run"), BENCH("bench") }`.
- `NewspeakRunConfigurationOptions : RunConfigurationOptions` stores `projectFile`, `command` (as the enum's name), `script`, `programArguments` and `iterations`, through the `by string()` and `by property(10)` delegates.
- `NewspeakRunConfiguration : RunConfigurationBase<NewspeakRunConfigurationOptions>` exposes them as the properties named above.
- `getConfigurationEditor()` answers `NewspeakRunConfigurationEditor()`.
- `getState` locates the runtime, throws an `ExecutionException` with the same message as `checkConfiguration` when it is not found, and answers a `CommandLineState`. That state starts `GeneralCommandLine(newspeakCommandLine(runtime, this))` in the project file's directory, through a `KillableColoredProcessHandler` with a `ProcessTerminatedListener` attached.
- `checkConfiguration()` throws a `RuntimeConfigurationError` in four cases, in this order:
  - **`NotConfigured`:** "The ns-next path is not set: choose the ns-next runtime under Settings | Languages & Frameworks | Newspeak". Its quick fix opens that page with `ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)`.
  - **`ConfiguredPathMissing`:** "No file at the ns-next path `<path>`", with the same quick fix.
  - **An empty `projectFile`, or one that is no file:** "Choose the project's newspeak.json".
  - **`bench` with `iterations < 1`:** "A benchmark takes at least one iteration".

```kotlin
/** The command line a Newspeak run configuration starts; pure, so its shape is tested without starting anything. */
fun newspeakCommandLine(runtime: Path, configuration: NewspeakRunConfiguration): List<String> = buildList {
    add(runtime.toString())
    add(configuration.command.subcommand)
    configuration.script?.takeIf(String::isNotEmpty)?.let { script -> addAll(listOf("--script", script)) }
    if (configuration.command == NewspeakCommand.BENCH) addAll(listOf("--iterations", configuration.iterations.toString()))
    add(Path.of(configuration.projectFile.orEmpty()).parent?.toString().orEmpty())
    val arguments = ParametersListUtil.parse(configuration.programArguments.orEmpty())
    if (arguments.isNotEmpty()) {
        add("--")
        addAll(arguments)
    }
}
```

`NewspeakRunConfigurationEditor.kt` is a `SettingsEditor<NewspeakRunConfiguration>` over a Kotlin UI DSL panel. Its rows:
- "Project file:", a file chooser limited to files named `newspeak.json`;
- "Command:", a combo box of Run and Benchmark;
- "Script:", a text field, with the comment "Empty for none";
- "Arguments:", an expandable text field;
- "Iterations:", an integer spinner, enabled only for Benchmark.

`resetEditorFrom` and `applyEditorTo` copy the five properties.

In `plugin.xml`'s `com.intellij` extensions:

```xml
<configurationType implementation="io.github.bstrauss.newspeak.run.NewspeakConfigurationType"/>
```

- [ ] **Step 5: Run the plugin's tests and checks**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: PASS.

- [ ] **Step 6: The per-commit gate, then commit**

```bash
git add -A
git commit -m "feat: the JetBrains plugin runs Newspeak projects with the ns-next its settings name"
```

---

### Task 6: A play button beside main and each script

**Files:**
- Create in `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/`: `NewspeakProjectJson.kt` (which JSON element is `main` or a script), `NewspeakRunLineMarkerContributor.kt`, `NewspeakRunAction.kt`, `NewspeakRunConfigurationProducer.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/newspeak-json.xml`
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunLineMarkerContributorTest.kt`

**Interfaces:**
- Consumes: Task 5's type, factory and configuration properties.
- JSON PSI from the JSON plugin, as of 2026.2: `com.intellij.json.psi.JsonFile`, `JsonObject`, `JsonProperty` (`name`, `nameElement`) and `JsonStringLiteral`. A `RunLineMarkerContributor` is registered per language with `<runLineMarkerContributor language="JSON" .../>`; the `Info` it answers carries the icon, the menu's actions and the tooltip. A `LazyRunConfigurationProducer<NewspeakRunConfiguration>` is registered with `<runConfigurationProducer .../>`. Verify each name against the JSON plugin's jars.

- [ ] **Step 1: The failing tests**

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.codeInsight.daemon.LineMarkerInfo
import com.intellij.execution.RunManager
import com.intellij.icons.AllIcons
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakRunLineMarkerContributorTest : BasePlatformTestCase() {
    private val projectJson = """
        {
          "main": "App.ns",
          "modules": ["Greeter.ns"],
          "scripts": { "timed": ["--timed"], "sieve": ["Sieve"] },
          "other": { "main": "Nested.ns" }
        }
    """.trimIndent()

    /** The lines that carry a play button, 0-based, once per button. */
    private fun markedLines(fileName: String, text: String): List<Int> {
        myFixture.configureByText(fileName, text)
        val document = myFixture.editor.document
        return myFixture.findAllGutters()
            .filterIsInstance<LineMarkerInfo.LineMarkerGutterIconRenderer<*>>()
            .filter { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run }
            .map { renderer -> document.getLineNumber(renderer.lineMarkerInfo.startOffset) }
            .sorted()
    }

    fun testMainAndEachScriptCarryAPlayButtonAndNothingElse() {
        assertEquals(listOf(1, 3, 3), markedLines("newspeak.json", projectJson))
    }

    fun testAnotherJsonFileCarriesNone() {
        assertEquals(emptyList<Int>(), markedLines("package.json", projectJson))
    }

    fun testTheMenuHoldsRunAndBenchmark() {
        myFixture.configureByText("newspeak.json", projectJson)
        val renderer = myFixture.findAllGutters()
            .filterIsInstance<LineMarkerInfo.LineMarkerGutterIconRenderer<*>>()
            .first { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run }
        // The platform's run marker hands the Info's actions to its renderer's menu; if 2026.2
        // answers them elsewhere (`clickAction`, a wrapping group), read them from there.
        val group = renderer.popupMenuActions as com.intellij.openapi.actionSystem.ActionGroup
        val texts = group.getChildren(null).map { action -> action.templatePresentation.text }
        assertEquals(listOf("Run", "Benchmark"), texts)
    }

    fun testRunOnAScriptCreatesItsConfigurationWithoutStartingIt() {
        myFixture.configureByText("newspeak.json", projectJson)
        val settings = createConfigurationFromGutter(project, myFixture.file.virtualFile, "timed", NewspeakCommand.BENCH)
        val configuration = settings.configuration as NewspeakRunConfiguration
        assertEquals("timed", configuration.script)
        assertEquals(NewspeakCommand.BENCH, configuration.command)
        assertEquals(myFixture.file.virtualFile.path, configuration.projectFile)
        assertTrue(settings.isTemporary)
        assertSame(settings, RunManager.getInstance(project).selectedConfiguration)
    }
}
```

Both scripts sit on line 3, so line 3 carries two markers. If `LineMarkerGutterIconRenderer` has moved in 2026.2, find the renderer class `findAllGutters` answers for a line marker and filter on it the same way.

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests '*NewspeakRunLineMarkerContributorTest*'`
Expected: FAIL.

- [ ] **Step 2: Which element is `main` or a script**

`NewspeakProjectJson.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.json.psi.JsonFile
import com.intellij.json.psi.JsonObject
import com.intellij.json.psi.JsonProperty
import com.intellij.psi.PsiElement

/** What a play button beside a key of a `newspeak.json` runs: the project without a script, or one script. */
sealed interface ProjectEntry {
    data object Main : ProjectEntry
    data class Script(val name: String) : ProjectEntry
}

/**
 * The entry `element` -- the first leaf of a property's name -- stands for:
 * the top-level `main`, or a key of the top-level `scripts` object; null for
 * anything else, a key of the same name nested elsewhere included.
 */
fun projectEntryAt(element: PsiElement): ProjectEntry? {
    val file = element.containingFile as? JsonFile ?: return null
    if (file.name != "newspeak.json") return null
    val property = element.parent?.parent as? JsonProperty ?: return null
    if (property.nameElement != element.parent) return null
    val owner = property.parent as? JsonObject ?: return null
    if (owner == file.topLevelValue) {
        return if (property.name == "main") ProjectEntry.Main else null
    }
    val scripts = owner.parent as? JsonProperty ?: return null
    return if (scripts.name == "scripts" && scripts.parent == file.topLevelValue) ProjectEntry.Script(property.name) else null
}
```

The PSI shape (leaf → `JsonStringLiteral` → `JsonProperty`) is the JSON plugin's. Confirm it with `myFixture` before relying on it, and adjust the parent steps if the plugin wraps it differently.

- [ ] **Step 3: The marker and its two actions**

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.psi.PsiElement

/** The play button beside `main` and each script of a `newspeak.json`, opening Run and Benchmark. */
class NewspeakRunLineMarkerContributor : RunLineMarkerContributor() {
    override fun getInfo(element: PsiElement): Info? {
        if (element.firstChild != null) return null
        val entry = projectEntryAt(element) ?: return null
        val file = element.containingFile.virtualFile ?: return null
        val script = (entry as? ProjectEntry.Script)?.name
        return Info(
            AllIcons.RunConfigurations.TestState.Run,
            arrayOf(NewspeakRunAction(file, script, NewspeakCommand.RUN), NewspeakRunAction(file, script, NewspeakCommand.BENCH)),
        ) { _ -> if (script == null) "Run the project" else "Run script '$script'" }
    }
}
```

`Info`'s constructors are the platform's for 2026.2. Check them, and use the current one if the tooltip provider's form has changed.

`NewspeakRunAction(file, script, command)` is an `AnAction`. Its text is "Run" for `RUN` and "Benchmark" for `BENCH`, with `AllIcons.Run.Run` and `AllIcons.Gutter.RunInQueue`. Its `actionPerformed` calls `createConfigurationFromGutter(project, file, script, command)` and starts the answer with `ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())`. When the configuration is invalid, for example because no ns-next path is set, the platform's own path shows the error and the "Edit Configuration" dialog. Nothing extra is needed for decision 6's error. `createConfigurationFromGutter` does three things:
- It looks among `RunManager.getInstance(project).getConfigurationSettingsList(NewspeakConfigurationType.getInstance())` for one with the same project file, script and command, and reuses it.
- Otherwise it creates one through the factory, named as this plan's choices say, marks it temporary, and adds it with `RunManager.addConfiguration`.
- Either way it selects it, and answers it without starting it, which is what the test calls.

`NewspeakRunConfigurationProducer` is a `LazyRunConfigurationProducer<NewspeakRunConfiguration>` whose factory is the type's. `setupConfigurationFromContext` accepts a context inside a file named `newspeak.json`. It takes the entry at the context's element, through `projectEntryAt` on its first leaf, or none for elsewhere in the file. It fills `projectFile`, `script` and `RUN`. `isConfigurationFromContext` compares those three. This gives the file's context menu, and the editor's, a "Run" entry that matches the gutter's configurations.

In `newspeak-json.xml`'s `com.intellij` extensions:

```xml
<runLineMarkerContributor language="JSON" implementationClass="io.github.bstrauss.newspeak.run.NewspeakRunLineMarkerContributor"/>
<runConfigurationProducer implementation="io.github.bstrauss.newspeak.run.NewspeakRunConfigurationProducer"/>
```

Both use JSON PSI, so they live in the optional descriptor. The run configuration type does not, and stays in `plugin.xml`.

- [ ] **Step 4: Run the plugin's tests and checks**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: PASS.

Then try it once by hand: `./editors/jetbrains/gradlew -p editors/jetbrains runIde`, open `benchmarks/are-we-fast-yet`, and do the following.
- Click the play button beside `main`, with no ns-next path set. Expect the error and the settings link.
- Set the path to a release `ns-next.exe`, click the button and choose Run. Expect each benchmark's `<Name>: ok`.
- Click a script's button and choose Benchmark. Expect the timing line.

Describe what happened in the report. A test cannot replace this check of the popup.

- [ ] **Step 5: The per-commit gate, then commit**

```bash
git add -A
git commit -m "feat: a play button beside main and each script runs or benchmarks the project"
```

---

### Task 7: The documentation describes the project file as it is

**Files:**
- Modify: `docs/design/image-format.md`, "Packaging". The project file's keys become `main` and optional `modules`; every other key, `$schema` among them, is ignored, and the passage that says unknown fields are refused is rewritten. State the rules and their messages, where the schema lives, the test that holds reader and schema together and why no validator crate was added, and the plugin's use of the schema. Decisions 1–4 of this plan go into its "Decisions".
- Modify: `README.md`. Its project-file example uses `main` and shows a `scripts` entry with `ns-next run --script`, and one sentence names `schemas/newspeak.schema.json`.
- Modify: `docs/design/image-format.md`, "Packaging", also describes `scripts` and `--script` (decision 5 and the choices beside it). It explains why `build` takes no `--script`, and names a script recorded into an image as open work in `docs/OPEN-WORK.md`.
- Modify: `docs/design/README.md`. The map names `schemas/` and the agreement test.
- Modify: `docs/design/editor-support.md`. The plugin gives `newspeak.json` its schema through the optional JSON plugin. It runs projects: the ns-next path setting and why nothing is bundled, the "Newspeak" run configuration type, and the play buttons with Run and Benchmark. Decisions 6–8 and the choices beside them go there.
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`'s description and change notes. Running and benchmarking a project from `newspeak.json`, and the ns-next path setting.
- Modify: `CLAUDE.md`. The `ns-cli` bullet says a project's `newspeak.json` is described by `schemas/newspeak.schema.json`. Under "Working in this repository", a change to that schema reaches the plugin, so it runs the plugin's `check`.
- Modify: `docs/PROGRESS.md`. Add a short entry for `newspeak.json` and its schema.

- [ ] **Step 1:** Rewrite each passage in the present tense. Check every statement against the code.
- [ ] **Step 2:** Check that `git diff --stat` shows Markdown only, then commit with no gates:

```bash
git add -A
git commit -m "docs: the project file, its schema, its scripts, and the plugin's use of them"
```

---

### Task 8: The end-of-batch gates and the review

- [ ] **Step 1:** Run the `finishing-a-batch` skill. `ns-cli` and the plugin both changed, so every gate applies: the five stress modes, `run-corpus` plain and under each mode, the benchmarks profile, and `./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin`. These also cover `54c021d6`, whose end-of-batch gates were interrupted.
- [ ] **Step 2:** Fix what they find, each fix its own commit through the per-commit gate.
- [ ] **Step 3:** Request the final whole-branch review on Fable, over `master..HEAD`, which also covers `54c021d6`, before merging.
