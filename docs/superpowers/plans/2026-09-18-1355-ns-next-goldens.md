# The goldens directory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `tests-fixtures/` into `goldens/`, a category-structured,
auto-discovered set of recorded program outcomes whose expectation format can
express a failing program, and dissolve `crates/ns-cli/tests/program_outcomes.rs`
into it.

**Architecture:** One golden is a pair: `goldens/<category>/<name>.ns` and
`goldens/<category>/<name>.toml`. The TOML records `status`, `stdout` and
`stderr`, each defaulting to the success case (`0`, empty, empty), so an
ordinary golden writes only `stdout` and a failing one writes `status` and
`stderr`. Test binaries discover goldens by walking `goldens/` recursively
rather than from any list, and a two-way pairing check replaces the hardcoded
count. The 33 tests in `program_outcomes.rs` exist only because the old format
could not express a non-zero exit; once it can, they become 33 goldens and the
file goes away.

**Tech Stack:** Rust 1.98.0, edition 2021. `toml` 0.8 and `serde` 1 are already
workspace dependencies of `ns-cli`; this plan adds them to its
`[dev-dependencies]` so the integration tests can use them. No new external
dependency.

**Spec:** There is no separate spec document. The design was settled in
conversation and is restated in full here; `CLAUDE.md`'s "Program outcomes are
pinned, not re-derived" is the invariant it serves, and
`docs/design/execution.md`'s "One engine" is what the goldens exist to hold.

## Global Constraints

- All code, identifiers, comments, doc comments, commit messages and developer
  documentation are in English.
- **Names are written out.** No identifier of one or two characters and no
  truncated word: `expectation`, not `exp`; `directory`, not `dir`;
  `argument_count`, not `num_args`. Clippy's `min_ident_chars` (threshold 2,
  inclusive) enforces it. Type parameters and lifetimes keep Rust's
  conventions.
- **Program outcomes are pinned, not re-derived.** Every expectation this plan
  moves is a recorded answer. A conversion must preserve it byte for byte. If a
  golden's recorded output changes during this work, stop and report it: that is
  a behaviour change, not a conversion artefact.
- A Newspeak module written inline in a test is an `indoc! {"…"}`, its lines
  indented with the Rust around it and the closing `"}` on a line of its own.
- Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an
  `EvaluationResult` or a `HostResult`.
- Comments explain the decision — what a check prevents — not the mechanics.
- Never cite a specification section without verifying it against
  `docs/references/newspeak-spec.md`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
- `docs/superpowers/plans/` files are frozen execution records. They mention
  `tests-fixtures/` and **must not be edited** by this plan.
- `docs/PROGRESS.md`'s milestone chronology is legitimately historical. Entries
  that describe what happened in an earlier milestone keep saying
  `tests-fixtures/`; only current-state text is updated. Task 4 says which lines
  are which.

## Gates

Every task ends with all of these green, and the implementer reports their
output verbatim:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets     # must be warning-free
cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules   # parsed 210, failed 0, expected-fail 1, stale 0
cargo run -p ns-cli -- run-corpus corpus/modules     # ran 6, failed 0, skipped 205
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

## File structure

| File | Responsibility |
|---|---|
| `goldens/<category>/<name>.ns` | The program. |
| `goldens/<category>/<name>.toml` | Its recorded outcome: `status`, `stdout`, `stderr`. |
| `crates/ns-cli/tests/common/mod.rs` | Discovery and the `Expectation` type, shared by every test binary that reads goldens. |
| `crates/ns-cli/tests/run_fixtures.rs` | Runs every golden under both lowerings; the pairing check. |
| `crates/ns-cli/tests/snapshot_load.rs` | Snapshot round trip over the goldens that exit 0. |
| `crates/ns-cli/tests/build_load.rs` | Build/load round trip over the goldens that exit 0. |
| `crates/ns-cli/tests/program_outcomes.rs` | Deleted by Task 3. |

## The categories

Eight, and every golden belongs to exactly one. Counts after Task 3.

| Category | Count | What belongs in it |
|---|---|---|
| `bench/` | 4 | The benchmark sources the measurement protocol names. |
| `cache/` | 7 | Programs that pin what an inline cache may and may not do. |
| `class-side/` | 5 | The class side and the metaclass. |
| `errors/` | 11 | Programs that raise a Newspeak error and exit non-zero. |
| `language/` | 17 | Ordinary evaluation: arithmetic, blocks, control flow, slots. |
| `limits/` | 5 | The activation-depth limit, reached and not reached. |
| `nesting/` | 7 | Nested classes, `outer`, and inheritance across a nesting boundary. |
| `spec/` | 19 | A named specification rule, pinned as such. |

---

### Task 1: `goldens/`, categories, recursive discovery, the pairing check

Renames the directory, files every golden under a category, makes all four test
binaries discover them by walking rather than by listing, and replaces the
hardcoded floor of 42 with a check that actually detects a golden that stopped
being discovered. The expectation format does not change in this task: the
sidecar stays `.expected` and still means "stdout, exit 0, empty stderr".

**Files:**
- Move: all 84 files in `tests-fixtures/` to `goldens/<category>/`
- Modify: `crates/ns-cli/tests/common/mod.rs`
- Modify: `crates/ns-cli/tests/run_fixtures.rs`
- Modify: `crates/ns-cli/tests/snapshot_load.rs` — the sweep at `:173-175` and
  seven `fixture(...)` calls at `:12,43,71,72,100,232,280`
- Modify: `crates/ns-cli/tests/build_load.rs` — the sweep at `:130-134` and
  seven `fixture(...)` calls at `:13,32,53,69,93,112,193`
- Modify: `crates/ns-cli/tests/inspect.rs:12,46,81,113`
- Modify: `crates/ns-cli/tests/lowering.rs:11,33,40,66,91,138`
- Modify: `crates/ns-cli/tests/disasm_snapshots.rs:17,81,176,219`
- Modify: `crates/ns-cli/tests/cli.rs:315,337`

**Interfaces:**
- Produces, in `common/mod.rs`:
  - `pub fn goldens_root() -> PathBuf` — the `goldens/` directory.
  - `pub fn golden_sources() -> Vec<PathBuf>` — every `goldens/**/*.ns`, sorted
    by path, panicking if the directory is missing or empty.
  - `pub fn golden(name: &str) -> PathBuf` — one golden's `.ns` by **bare name
    without an extension** (`golden("hello")`, where today's helper took
    `fixture("hello.ns")`), searched across categories, panicking if it is
    missing or ambiguous. Replaces `fixture(name)`, which is deleted.
- Consumes: nothing from earlier tasks.

**Two call sites read an expectation file directly** and cannot simply drop an
extension: `lowering.rs:40` reads `fixture("while_loop.expected")` and
`snapshot_load.rs:72` reads `fixture("nested_outer_inherited.expected")`. In
this task they become
`std::fs::read_to_string(common::golden("while_loop").with_extension("expected"))`
and the same for the other; Task 2 replaces both with `expectation_of`.

**The category assignment.** Every existing fixture, by its current bare name:

- `bench/`: `bench_block`, `bench_loop`, `bench_megamorphic`, `bench_send`
- `cache/`: `cache_class_objects_and_instances_at_one_site`,
  `cache_nested_classes_share_behaviour`, `cache_polymorphic_sites`,
  `cache_private_lexical_site`, `cache_slots_under_two_applications`
- `class-side/`: `class_side`, `class_side_implicit`, `class_side_outer`,
  `class_side_super_reaches_object`
- `language/`: `arithmetic`, `blocks_and_return`, `closure_variables`,
  `control_flow_fallback`, `hello`, `identity_hash`, `instance_init`,
  `nonlocal_return_depth`, `superclass_args`, `while_loop`
- `nesting/`: `nested_access`, `nested_identity`, `nested_outer`,
  `nested_outer_inherited`, `nested_override`, `nested_redeclared`,
  `nested_super`
- `spec/`: `spec_2_3_nested_class_overridden_by_slot`,
  `spec_2_3_superclass_of_a_nested_class`,
  `spec_5_10_super_does_not_understand_is_the_superclasss`,
  `spec_5_5_public_lookup_skips_private`, `spec_5_7_full_selectors`,
  `spec_5_7_immutable_slot_declares_no_setter`,
  `spec_5_7_lexical_before_inherited`,
  `spec_5_8_parenthesized_self_is_a_self_send`,
  `spec_5_9_private_is_only_the_enclosing_classs`,
  `spec_6_3_2_public_slot_explicit_receiver`,
  `spec_6_3_2_slot_accessors_are_methods`,
  `spec_6_3_factory_is_a_class_side_member`

That is 42 names; `errors/` and `limits/` stay empty until Task 3 and are not
created in this task.

- [ ] **Step 1: Move the files**

```bash
cd /d/Workspace/ns-next
mkdir -p goldens/bench goldens/cache goldens/class-side goldens/language goldens/nesting goldens/spec
for name in bench_block bench_loop bench_megamorphic bench_send; do
  git mv tests-fixtures/$name.ns goldens/bench/$name.ns
  git mv tests-fixtures/$name.expected goldens/bench/$name.expected
done
for name in cache_class_objects_and_instances_at_one_site cache_nested_classes_share_behaviour cache_polymorphic_sites cache_private_lexical_site cache_slots_under_two_applications; do
  git mv tests-fixtures/$name.ns goldens/cache/$name.ns
  git mv tests-fixtures/$name.expected goldens/cache/$name.expected
done
for name in class_side class_side_implicit class_side_outer class_side_super_reaches_object; do
  git mv tests-fixtures/$name.ns goldens/class-side/$name.ns
  git mv tests-fixtures/$name.expected goldens/class-side/$name.expected
done
for name in arithmetic blocks_and_return closure_variables control_flow_fallback hello identity_hash instance_init nonlocal_return_depth superclass_args while_loop; do
  git mv tests-fixtures/$name.ns goldens/language/$name.ns
  git mv tests-fixtures/$name.expected goldens/language/$name.expected
done
for name in nested_access nested_identity nested_outer nested_outer_inherited nested_override nested_redeclared nested_super; do
  git mv tests-fixtures/$name.ns goldens/nesting/$name.ns
  git mv tests-fixtures/$name.expected goldens/nesting/$name.expected
done
for name in spec_2_3_nested_class_overridden_by_slot spec_2_3_superclass_of_a_nested_class spec_5_10_super_does_not_understand_is_the_superclasss spec_5_5_public_lookup_skips_private spec_5_7_full_selectors spec_5_7_immutable_slot_declares_no_setter spec_5_7_lexical_before_inherited spec_5_8_parenthesized_self_is_a_self_send spec_5_9_private_is_only_the_enclosing_classs spec_6_3_2_public_slot_explicit_receiver spec_6_3_2_slot_accessors_are_methods spec_6_3_factory_is_a_class_side_member; do
  git mv tests-fixtures/$name.ns goldens/spec/$name.ns
  git mv tests-fixtures/$name.expected goldens/spec/$name.expected
done
rmdir tests-fixtures
```

Expected: `tests-fixtures/` gone, `git status` shows 84 renames and no deletions
or additions. If `rmdir` refuses, a file was missed — find it before going on.

- [ ] **Step 2: Write the discovery helpers**

Replace `fixture` in `crates/ns-cli/tests/common/mod.rs` with the three
functions below, keeping the rest of the file as it is.

```rust
/// The `goldens/` directory: every recorded program outcome lives under a
/// category directory inside it.
pub fn goldens_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens")
}

/// Every golden's source, found by walking rather than from a list, so that
/// adding one needs no edit anywhere. Sorted, so a failure names the same
/// golden on every platform and run.
pub fn golden_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let root = goldens_root();
    let categories = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()));
    for category in categories {
        let category = category.expect("goldens category entry").path();
        if !category.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&category)
            .unwrap_or_else(|error| panic!("read {}: {error}", category.display()));
        for entry in entries {
            let path = entry.expect("golden entry").path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("ns") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    assert!(
        !sources.is_empty(),
        "no goldens found under {} -- the directory moved or the walk is wrong",
        root.display()
    );
    sources
}

/// One golden's source by its bare name, for the few tests that need a
/// particular program rather than all of them. Panics on a miss so that a
/// renamed golden fails loudly here instead of silently skipping a test, and
/// on an ambiguity so that two categories cannot quietly claim one name.
pub fn golden(name: &str) -> PathBuf {
    let matches: Vec<PathBuf> = golden_sources()
        .into_iter()
        .filter(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name))
        .collect();
    match matches.len() {
        1 => matches.into_iter().next().expect("one match"),
        0 => panic!("no golden named {name}"),
        _ => panic!("{} goldens named {name}", matches.len()),
    }
}
```

- [ ] **Step 3: Write the failing pairing check**

Add to `crates/ns-cli/tests/run_fixtures.rs`. This is the test that replaces the
floor of 42.

```rust
/// Every golden is a pair, checked in both directions. The floor this
/// replaced counted what the walk found, which cannot detect the thing it
/// was there to detect: a golden that stops being discovered because it was
/// renamed, moved or given another extension. An orphan on either side is
/// exactly that event, and it fails here.
#[test]
fn every_golden_is_a_complete_pair() {
    let mut missing_expectation = Vec::new();
    for source in common::golden_sources() {
        let expectation = source.with_extension("expected");
        if !expectation.exists() {
            missing_expectation.push(source.display().to_string());
        }
    }
    let mut orphaned_expectation = Vec::new();
    let root = common::goldens_root();
    for category in std::fs::read_dir(&root).expect("read goldens") {
        let category = category.expect("category entry").path();
        if !category.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&category).expect("read category") {
            let path = entry.expect("golden entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("expected") {
                continue;
            }
            if !path.with_extension("ns").exists() {
                orphaned_expectation.push(path.display().to_string());
            }
        }
    }
    assert!(
        missing_expectation.is_empty() && orphaned_expectation.is_empty(),
        "goldens without an expectation: {missing_expectation:?}; \
         expectations without a golden: {orphaned_expectation:?}"
    );
}
```

- [ ] **Step 4: Prove the pairing check catches a broken pair**

A pairing check cannot fail on a correct tree, so the way to see it work is to
break a pair on purpose and put it back.

```bash
cargo test -p ns-cli --test run_fixtures every_golden_is_a_complete_pair
mv goldens/language/hello.expected goldens/language/hello.expected.bak
cargo test -p ns-cli --test run_fixtures every_golden_is_a_complete_pair
mv goldens/language/hello.expected.bak goldens/language/hello.expected
cargo test -p ns-cli --test run_fixtures every_golden_is_a_complete_pair
```

Expected: PASS, then FAIL naming `goldens/language/hello.ns`, then PASS. Record
what you actually saw, and confirm `git status --porcelain` is clean afterwards.
A check that passes all three times is not wired up — find out why before going
on.

- [ ] **Step 5: Point the sweeps and the named lookups at the new layout**

In `run_fixtures.rs`, add `mod common;` (it has none today), replace both
`std::fs::read_dir` walks with `common::golden_sources()`, and delete the
`checked >= 42` assertion together with its comment and the `checked` counter.

In `snapshot_load.rs:173-175` and `build_load.rs:130-134`, replace the
`read_dir` walk with `common::golden_sources()`.

In `snapshot_load.rs`, `build_load.rs`, `inspect.rs` and `lowering.rs`, change
every `fixture("name.ns")` to `common::golden("name")` and update the
`use common::{fixture, ...}` lines in `snapshot_load.rs:5` and
`build_load.rs:4`. `inspect.rs` and `lowering.rs` reach `fixture` the same way —
check how each imports it and follow that file's own style.

The two expectation reads named in the Interfaces block above
(`lowering.rs:40`, `snapshot_load.rs:72`) take `.with_extension("expected")`.

In `disasm_snapshots.rs`, replace `.join("../../tests-fixtures")` with
`common::goldens_root()` and the two `../../tests-fixtures/arithmetic.ns`
paths and one `../../tests-fixtures/hello.ns` path with
`common::golden("arithmetic")` and `common::golden("hello")`.

In `cli.rs:315,337`, replace
`repository_root().join("tests-fixtures/while_loop.ns")` with
`common::golden("while_loop")`.

- [ ] **Step 6: Run the gates**

Run every command in the Gates section. Expected: all green, and
`cargo test --workspace` shows one more test than before (the pairing check).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit
```

Subject: `test: goldens/, by category, discovered by walking`. In the body, say
that the hardcoded floor of 42 was replaced by the two-way pairing check, and
why the floor could not detect the thing its own comment claimed it detected —
it counted what the walk found, so a golden that stopped being found simply
lowered the count. End with the two trailer lines the dispatch gives you.

---

### Task 2: The TOML expectation format

Replaces `<name>.expected` with `<name>.toml` carrying `status`, `stdout` and
`stderr`, so that a golden can record a program that fails. No golden changes
its recorded outcome in this task.

**Files:**
- Modify: `crates/ns-cli/Cargo.toml`
- Modify: `crates/ns-cli/tests/common/mod.rs`
- Modify: `crates/ns-cli/tests/run_fixtures.rs`
- Modify: `crates/ns-cli/tests/snapshot_load.rs` — the sweep, and the
  expectation read Task 1 left at `:72`
- Modify: `crates/ns-cli/tests/build_load.rs` — the sweep
- Modify: `crates/ns-cli/tests/lowering.rs` — the expectation read Task 1 left
  at `:40`
- Delete: every `goldens/**/*.expected`
- Create: every `goldens/**/*.toml`

**Interfaces:**
- Consumes: `common::goldens_root()`, `common::golden_sources()`,
  `common::golden(name)` from Task 1.
- Produces, in `common/mod.rs`:
  - `pub struct Expectation { pub status: i32, pub stdout: String, pub stderr: String }`
  - `pub fn expectation_of(source: &Path) -> Expectation` — reads and parses the
    golden's `.toml` sibling, panicking with the path on a missing or malformed
    file.
  - `Expectation::succeeds(&self) -> bool` — `status == 0`, which the two
    round-trip sweeps use to skip a golden that is supposed to fail.

- [ ] **Step 1: Add the test-only dependencies**

In `crates/ns-cli/Cargo.toml`, extend `[dev-dependencies]`. `toml` and `serde`
are already `[dependencies]`, but `ns-cli` has no `[lib]` target, so its
integration tests cannot reach them; these lines are the same workspace crates,
not new ones.

```toml
[dev-dependencies]
indoc.workspace = true
insta.workspace = true
serde = { version = "1", features = ["derive"] }
toml.workspace = true
```

- [ ] **Step 2: Write the `Expectation` type**

Add to `crates/ns-cli/tests/common/mod.rs`.

```rust
/// A golden's recorded outcome. Every field defaults to the success case, so
/// an ordinary golden's file is just its `stdout` and a failing one's is its
/// `status` and `stderr`. `deny_unknown_fields` turns a misspelled key into a
/// failure instead of an expectation that silently stops being checked.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

impl Expectation {
    /// Whether the golden is expected to exit zero. The snapshot and
    /// build/load round trips only apply to a program that runs to
    /// completion, so they use this to skip the rest.
    pub fn succeeds(&self) -> bool {
        self.status == 0
    }
}

/// The recorded outcome beside `source`.
pub fn expectation_of(source: &Path) -> Expectation {
    let path = source.with_extension("toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}
```

- [ ] **Step 3: Convert the 42 expectation files**

Run this once. It writes a TOML literal multi-line string, which performs no
escape processing, so a backslash in a recorded output stays a backslash; TOML
drops the newline immediately after the opening `'''`, so the value is the
file's bytes exactly. A recorded output that does not end in a newline cannot be
written as a block and gets the single-line form instead.

```bash
cd /d/Workspace/ns-next
python - <<'PY'
import io, glob, os
for expected in glob.glob('goldens/*/*.expected'):
    text = io.open(expected, encoding='utf-8', newline='').read().replace('\r\n', '\n')
    assert "'''" not in text, expected
    if text.endswith('\n'):
        body = "stdout = '''\n" + text + "'''\n"
    else:
        body = 'stdout = ' + repr(text).replace("'", '"', 2) + '\n'
    io.open(expected.replace('.expected', '.toml'), 'w', encoding='utf-8', newline='').write(body)
    os.remove(expected)
print('converted', len(glob.glob('goldens/*/*.toml')))
PY
```

Expected: `converted 42`, and no `.expected` file left under `goldens/`.

- [ ] **Step 4: Verify the conversion preserved every byte**

```bash
python - <<'PY'
import io, glob, subprocess, tomllib
for toml_path in sorted(glob.glob('goldens/*/*.toml')):
    recorded = tomllib.load(open(toml_path, 'rb')).get('stdout', '')
    source = toml_path.replace('.toml', '.ns')
    out = subprocess.run(['cargo', 'run', '-q', '-p', 'ns-cli', '--', 'run', source],
                         capture_output=True, text=True)
    actual = out.stdout.replace('\r\n', '\n')
    assert actual == recorded, (toml_path, repr(actual[:80]), repr(recorded[:80]))
print('all 42 match byte for byte')
PY
```

Expected: `all 42 match byte for byte`. If any golden disagrees, **stop** — a
recorded answer changed, which is a behaviour change to establish, not a
conversion to fix.

- [ ] **Step 5: Teach the runner the three streams**

In `run_fixtures.rs`, replace the body that reads `.expected` and asserts
success-plus-stdout with one that reads the `Expectation` and asserts all three
streams, under both lowerings.

```rust
for source in common::golden_sources() {
    let expectation = common::expectation_of(&source);
    for lowering in LOWERINGS {
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&source)
            .arg("--lowering")
            .arg(lowering)
            .output()
            .expect("run ns-next");
        assert_eq!(
            output.status.code(),
            Some(expectation.status),
            "{} exit status under the {lowering} lowering",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            normalize_newlines(&expectation.stdout),
            "{} stdout under the {lowering} lowering",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
            normalize_newlines(&expectation.stderr),
            "{} stderr under the {lowering} lowering",
            source.display()
        );
    }
}
```

In the same file's two-slot-cache test, replace the `.expected` read and the
`output.status.success()` assertion the same way, comparing against
`expectation.status`, `expectation.stdout` and `expectation.stderr`.

In the pairing check from Task 1, change both `with_extension("expected")` and
the `Some("expected")` filter to `"toml"`.

- [ ] **Step 5b: Convert the two direct expectation readers**

`lowering.rs:40` and `snapshot_load.rs:72` read the expectation file to compare
a run against it. Replace each

```rust
std::fs::read_to_string(common::golden("while_loop").with_extension("expected"))
    .expect("...")
```

with

```rust
common::expectation_of(&common::golden("while_loop")).stdout
```

and the same for `nested_outer_inherited` in `snapshot_load.rs`. Both compare
stdout only, which is what these two tests are about; leave what they assert
otherwise alone.

- [ ] **Step 6: Make the two round trips skip a failing golden**

In `snapshot_load.rs` and `build_load.rs`, guard each sweep's body. Both build
or snapshot a program and then run it, which only means anything for a program
that runs to completion.

```rust
for source in common::golden_sources() {
    // A golden that is recorded as failing has nothing to round trip: these
    // sweeps assert that a snapshot or a built binary answers what the source
    // answered, and a program that raises answers nothing.
    if !common::expectation_of(&source).succeeds() {
        continue;
    }
    // ... the existing body, with `source` in place of the old `path`
}
```

- [ ] **Step 7: Run the gates**

Run every command in the Gates section. Expected: all green, with the same
number of tests as after Task 1.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit
```

Subject: `test: a golden records status, stdout and stderr`. In the body, say
that the format is one TOML file per golden with success defaults, and that the
42 conversions were verified byte for byte against a run rather than assumed.
End with the two trailer lines the dispatch gives you.

---

### Task 3: Dissolve `program_outcomes.rs` into the goldens

Turns all 33 tests into goldens and deletes the file. Its own doc comment says
these are "programs that cannot be fixtures, because they exit non-zero or pin
an exact message" — Task 2 removed that reason.

**Files:**
- Create: 33 pairs under `goldens/cache/`, `goldens/class-side/`,
  `goldens/errors/`, `goldens/language/`, `goldens/limits/`, `goldens/spec/`
- Delete: `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Consumes: `common::golden_sources()` and `common::expectation_of()` from
  Tasks 1 and 2. Adds no code.

**The category assignment**, by the test function's current name. The golden
keeps that name, minus a leading `a_`/`an_` only where it reads better; keep the
name as it is when in doubt, because the name is what a failure prints.

- `cache/` (2): `a_does_not_understand_site_with_three_kinds_of_handler`,
  `a_cached_primitive_that_fails_on_a_receiver_of_the_same_class`
- `class-side/` (1):
  `a_class_side_is_not_inherited_by_a_subclass_with_none_of_its_own`
- `errors/` (11): `a_does_not_understand_error`,
  `a_return_from_a_block_whose_initializer_has_finished_is_cannot_return`,
  `a_return_from_a_stored_block_whose_method_has_returned_is_cannot_return`,
  `a_non_boolean_receiver_of_an_inlined_selector_does_not_understand`,
  `a_non_boolean_while_condition_fails_with_must_be_boolean`,
  `a_bare_super_is_a_program_error`, `a_bare_super_raises_only_when_it_is_reached`,
  `a_superclass_clause_naming_the_wrong_factory_fails`,
  `a_variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean`,
  `an_if_block_that_takes_an_argument_is_a_wrong_arity_error`,
  `a_native_that_fails_on_its_receiver_falls_to_objects`
- `language/` (7): `a_variable_block_receiver_takes_the_native_loop`,
  `and_or_short_circuit_and_coerce`, `a_return_out_of_an_inlined_loop_body`,
  `operands_that_mutate_each_other_keep_their_evaluated_values`,
  `an_operand_written_by_a_callee_keeps_its_evaluated_value`,
  `an_assignment_through_a_setter_answers_the_value_before_the_setter_ran`,
  `a_variable_and_block_coerces_its_value_to_a_boolean`
- `limits/` (5): `a_thousand_deep_recursion_runs`,
  `an_unbounded_recursion_is_a_newspeak_error`,
  `a_recursion_through_do_is_bounded_by_the_activation_limit`,
  `a_recursive_instantiation_is_bounded_by_the_activation_limit`,
  `a_recursion_through_the_nested_class_accessor_is_bounded_by_the_activation_limit`
- `spec/` (7): `a_super_send_does_not_reach_a_private_method`,
  `a_superclasss_private_method_is_not_lexically_visible`,
  `an_enclosing_classs_inherited_member_is_not_lexically_visible`,
  `a_private_nested_class_is_unreachable_from_outside`,
  `an_immutable_slot_has_no_setter`,
  `a_protected_slot_does_not_answer_an_explicit_receiver`,
  `an_explicit_does_not_understand_send_is_refused_as_protected`

8 of the 33 record exit 0 (all seven in `language/`, plus
`a_thousand_deep_recursion_runs` in `limits/`); the other 25 record exit 1.

- [ ] **Step 1: Extract one golden by hand, end to end**

Start with `a_does_not_understand_error`. Copy its `indoc!` source verbatim into
`goldens/errors/a_does_not_understand_error.ns` — the `indoc!` macro strips the
common leading indentation, so the file gets the *dedented* text, exactly what
`indoc!` produces. Write its expectation from the three literals in the
`assert_outcome` call:

```toml
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #foo
'''
```

Use the call's real strings, not this illustration. Then confirm the golden
reproduces them:

```bash
cargo test -p ns-cli --test run_fixtures
```

Expected: PASS, with the new golden discovered automatically. If it fails,
compare the dedented source against what `indoc!` produced before assuming the
expectation is wrong.

- [ ] **Step 2: Extract the remaining 32 the same way**

Work through the file in order. For each test: its `assert_outcome(name,
source, exit, stdout, stderr)` call gives the four things a golden needs. A
`stdout` or `stderr` that is `""` is omitted from the TOML, since both default
to empty; `exit` of `0` is omitted too.

A source whose recorded output does not end in a newline takes the single-line
form, `stdout = "..."`, rather than a `'''` block.

- [ ] **Step 3: Verify every extraction against the file it came from**

Before deleting anything, run both — the old tests and the new goldens — and
require both green. This is the only moment where the two exist side by side,
so it is the only moment the extraction can be checked against its source.

```bash
cargo test -p ns-cli --test program_outcomes
cargo test -p ns-cli --test run_fixtures
```

Expected: 33 passed and the golden sweep passed. A golden that disagrees with
its origin is an extraction mistake — fix the golden, never the recorded
outcome.

- [ ] **Step 4: Count what moved**

```bash
ls goldens/*/*.ns | wc -l
```

Expected: `75`. If it is not 75, one extraction is missing or duplicated; find
it before deleting the file that still has the original.

- [ ] **Step 5: Delete `program_outcomes.rs`**

```bash
git rm crates/ns-cli/tests/program_outcomes.rs
```

- [ ] **Step 6: Run the gates**

Run every command in the Gates section. Expected: all green. The workspace test
count drops by 33 (the deleted tests) while the golden sweep now covers 75
programs under two lowerings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit
```

Subject: `test: program_outcomes.rs becomes 33 goldens`. In the body, say that
the file existed only because the old format could not record a non-zero exit,
and that each extraction was checked with the old test and the new golden green
at the same time. End with the two trailer lines the dispatch gives you.

---

### Task 4: The documentation

Brings every current-state mention of `tests-fixtures/` up to date and describes
the golden format where a fresh session will meet it.

**Files:**
- Modify: `CLAUDE.md:36`
- Modify: `docs/design/execution.md:39,76,571,1881`
- Modify: `docs/design/compiler.md:286`
- Modify: `docs/design/dispatch.md:886,889,1212,1215,1218,1255`
- Modify: `docs/PROGRESS.md` — current-state lines only
- Modify: `docs/design/README.md`

**Interfaces:**
- Consumes: the layout and format from Tasks 1 to 3. Adds no code.

- [ ] **Step 1: Update `CLAUDE.md`'s invariant**

Line 36 currently reads that both lowerings are held to "every
`tests-fixtures/*.ns` against its `.expected` stdout and an empty stderr, the
golden exit status, stdout and stderr in
`crates/ns-cli/tests/program_outcomes.rs`, and `run-corpus`". Both halves are
now one thing. Replace it with a sentence saying that both are held to every
`goldens/**/*.ns` against the `status`, `stdout` and `stderr` recorded in its
`.toml`, and to `run-corpus`.

- [ ] **Step 2: Update the design documents**

Each of the listed lines names a fixture by its old path. Give it its new one:
`tests-fixtures/bench_loop.ns` becomes `goldens/bench/bench_loop.ns`,
`tests-fixtures/closure_variables.ns` becomes
`goldens/language/closure_variables.ns`, `tests-fixtures/cache_*.ns` becomes
`goldens/cache/*.ns`, and so on, using the categories from Tasks 1 and 3.

`dispatch.md:886-889` also says "two golden outcomes" as a thing separate from
the fixtures. It is not separate any more; say what is true now.

- [ ] **Step 3: Update `PROGRESS.md`, current state only**

Update lines 102, 115-120 and 182 — the current-state lists and the pointer to
the next session's benchmark sources. **Leave the milestone chronology alone**:
lines 819, 1204, 2224, 2336-2339, 2372 and 2382 describe what happened in an
earlier milestone, when the directory really was called `tests-fixtures/`, and
this repository's documents leave the route to the present in git history.

- [ ] **Step 4: Describe the format where it will be found**

`docs/design/README.md` maps the design documents and says how much of each
aspect exists. Add a short section to it — six to twelve lines — that says: a
golden is a `.ns` and a `.toml` under a category directory in `goldens/`; the
TOML records `status`, `stdout` and `stderr`, each defaulting to the success
case; both lowerings must produce it; the set is discovered by walking, so
adding a golden needs no edit anywhere; and the pairing check is what catches
one that stops being discovered. Name the eight categories and what belongs in
each.

- [ ] **Step 5: Check no current-state mention was missed**

```bash
grep -rn "tests-fixtures\|program_outcomes" --include="*.rs" --include="*.md" --include="*.toml" . \
  | grep -v "^./target" | grep -v "docs/superpowers/plans" | grep -v "docs/PROGRESS.md"
```

Expected: no output. Then check `PROGRESS.md` by hand and confirm every
remaining mention is inside the milestone chronology.

- [ ] **Step 6: Run the gates**

Run every command in the Gates section. Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit
```

Subject: `docs: the goldens directory and its format`. In the body, name which
`PROGRESS.md` mentions were updated and which were left as chronology, so a
later reader does not take the remaining ones for oversights. End with the two
trailer lines the dispatch gives you.

---

## Notes for whoever executes this

**The benchmark sources move.** `goldens/bench/bench_*.ns` are the measurement
protocol's own sources, named in `execution.md` and `PROGRESS.md`. A later
measurement session follows those paths, so Task 4's update is what keeps the
protocol runnable — it is not cosmetic.

**`indoc!` and the extracted sources.** `indoc!` strips the common leading
indentation at compile time. The golden file must contain what the macro
produced, not what the Rust source looks like. Getting this wrong shows up as a
parse error or a changed output, not as a silent pass.

**One category per golden, and the name is the message.** A golden's path is
what a failure prints. `goldens/spec/spec_5_9_private_is_only_the_enclosing_classs.ns`
tells a reader which rule broke before they open anything; a bare name in a flat
directory does not.
