# Artifact Inspection and the Stateless Binary — Implementation Plan

> **Status: Complete.** All 13 tasks (62 steps) implemented, each reviewed for
> spec compliance and quality, on `feature/artifact-inspection-stateless-binary`
> (`bbce533..4c8f968`), merged to `master` at `bba2a03`. Five fix rounds, all
> closed in one round each. A whole-branch review then raised two Important
> findings and seven Minor ones; its single fix wave landed as `4c8f968..5283640`.
>
> Two things this plan got wrong, corrected during execution and worth knowing
> before reading it as a reference. The byte-identity guarantee it states has
> **two** conditions, not one — the second, that a snapshot's graph interns no
> new symbol, is neither stated here nor enforced anywhere; see
> `docs/design/image-format.md`. And Task 12's third test as written passes for
> the wrong reason, which the review caught. The design document is the current
> authority; this file is the execution record.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a serialized artifact readable as text, then add a stateless binary
deployment artifact beside the existing image — one that carries code and structure,
no instance state, and its own entry point.

**Architecture:** One format, two artifact shapes, distinguished by a header flag.
The binary is the image's leading sections stopping where the object graph would
begin, plus a recorded entry selector; `build` produces it before instantiation,
`load` consumes both and instantiates on the stateless path so capabilities enter at
start time. Inspection comes first because the binary's own tests need it.

**Tech Stack:** Rust 1.98.0, edition 2021, no new external dependencies.

**Spec:** `docs/design/image-format.md` — read "Planned: reading an artifact",
"Planned: the stateless binary", "Layout", "What is *not* serialized" and
"Verification" before starting. `docs/design/compiler.md` covers the instruction
encoding the inspector prints. `docs/design/README.md` maps the rest.

## Global Constraints

- **Rust 1.98.0, edition 2021.** Pinned by `rust-toolchain.toml`.
- **No new external dependency** without a reason that survives review. The image
  format is hand-rolled on purpose.
- **`FORMAT_VERSION` stays 1.** Never bump it. The project has no external users;
  images predating a change are disposable.
- **Wire tags are fixed.** An instruction or literal tag is assigned once and never
  renumbered or reused (`crates/ns-runtime/src/image/opcodes.rs`). Adding a tag
  appends a line to `crates/ns-runtime/tests/opcode_tags.rs`; changing an existing
  line is the thing that test prevents.
- **An artifact is untrusted input.** No byte sequence may panic or abort. Validate
  every id, tag, count and length before it indexes, allocates or slices; bound or
  make iterative any recursion whose depth the input controls.
- **The two engines must agree byte for byte** — exit status, stdout *and* stderr —
  on every fixture. Never copy dispatch, lookup or error text out of `interpreter/`
  into `vm.rs` or `compiler/`.
- **Never `{:?}` a `Value`; never `.unwrap()`, `.expect()` or `{:?}` an
  `EvalResult`.** Tests unwrap through a local `expect_ok` helper — copy the one in
  a neighbouring test file.
- **English only** for code, identifiers, comments, doc comments and commit
  messages. Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `chore:`).
- **Comments explain the decision, not the mechanics.** The reason a check exists is
  the part a later reader cannot reconstruct.
- **Gates that must still pass at every commit:** `cargo fmt --all`,
  `cargo clippy --workspace --all-targets` warning-free, `cargo test --workspace`,
  `parse-corpus` at `parsed 210, failed 0, expected-fail 1, stale 0`, `run-corpus`
  at `ran 6, failed 0, skipped 205` (Task 11 raises what run-corpus does; the
  reported counts stay the same).

---

## File Structure

**Created**

- `crates/ns-runtime/src/image/inspect.rs` — decodes an artifact into a structural
  `Report` (section offsets, counts, symbol and mixin listings) and renders it as
  text. Read-only: it never installs an engine and never runs anything.
- `crates/ns-cli/tests/inspect.rs` — CLI tests for `ns-next inspect`.
- `crates/ns-cli/tests/build_load.rs` — CLI tests for `ns-next build` and the
  stateless `load` path, including the equivalence and prefix-equality gates.

**Modified**

- `crates/ns-runtime/src/image/mod.rs` — declare `inspect`; factor the sections
  `snapshot` and `build` share; add `build`; branch `load` on the flag; add `entry`
  and `stateless` to `LoadedImage`; reject trailing bytes.
- `crates/ns-runtime/src/image/encode.rs` — `FLAG_STATELESS`, `KNOWN_FLAGS`, the
  entry field writer.
- `crates/ns-runtime/src/image/decode.rs` — `Header.stateless`, the entry field
  reader, `DecodeError` variants for the new refusals.
- `crates/ns-cli/src/main.rs` — `inspect` and `build` subcommands; `disasm` accepts
  an artifact; `load` instantiates on the stateless path.
- `crates/ns-cli/src/corpus.rs` *(or wherever `run-corpus` lives — grep for
  `run-corpus`)* — the binary lane.
- `crates/ns-runtime/tests/image.rs` — untrusted-input tests for the new flag,
  field and trailing-byte check.
- `docs/design/image-format.md`, `docs/design/compiler.md`, `docs/PROGRESS.md`,
  `README.md`, `CLAUDE.md` — status flips and the new commands.

---

## Phase 1 — Reading an artifact

### Task 1: `disasm` accepts an artifact

Today `disasm` requires a `.ns` file and compiles it, so the bytecode actually
inside an artifact cannot be printed — which is worst exactly for the binary, whose
purpose is to ship without source.

The work is smaller than it looks: after obtaining a `(ClassRef, CompiledModule)`
pair, everything downstream in `cmd_disasm` already works, because a decoded root
class carries its `application.mixin` and therefore the whole nested-mixin tree that
`resolve_mixin_path` walks.

**Files:**
- Modify: `crates/ns-cli/src/main.rs` — `cmd_disasm`
- Test: `crates/ns-cli/tests/disasm_snapshots.rs`

**Interfaces:**
- Consumes: `ns_runtime::image::{load, LoadedImage}`,
  `ns_runtime::image::encode::MAGIC`, `Interpreter::compiled_module()`.
- Produces: nothing later tasks depend on.

- [x] **Step 1: Write the failing test**

Append to `crates/ns-cli/tests/disasm_snapshots.rs`:

```rust
/// `disasm` of an artifact must print exactly what `disasm` of the source it
/// was built from prints. The artifact carries the same compiled bodies; if
/// these ever differ, the image's code table is not what the compiler emitted.
#[test]
fn disasm_of_an_image_matches_disasm_of_its_source() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/arithmetic.ns");
    let dir = std::env::temp_dir().join(format!("ns-next-disasm-img-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let img = dir.join("world.img");

    let snap = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("snapshot")
        .arg(&src)
        .arg(&img)
        .output()
        .expect("run ns-next snapshot");
    assert!(snap.status.success(), "{}", String::from_utf8_lossy(&snap.stderr));

    let from_image = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&img)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm on an image");
    assert!(
        from_image.status.success(),
        "{}",
        String::from_utf8_lossy(&from_image.stderr)
    );

    let from_source = disasm("arithmetic.ns", &["main:args:"]);
    assert_eq!(
        String::from_utf8_lossy(&from_image.stdout).replace("\r\n", "\n"),
        from_source
    );
}
```

- [x] **Step 2: Run it and watch it fail**

Run: `cargo test -p ns-cli --test disasm_snapshots disasm_of_an_image -- --nocapture`

Expected: FAIL. `disasm` hands the image to the parser, which reports a lexer or
parser diagnostic on binary bytes, so the command exits non-zero and the assertion
on `from_image.status.success()` fires.

- [x] **Step 3: Add the artifact branch**

In `crates/ns-cli/src/main.rs`, add above `cmd_disasm`:

```rust
/// Whether `bytes` begins with the image format's magic.
///
/// Sniffing the content rather than the file extension is deliberate: an
/// artifact has no required extension, `snapshot`/`build` write whatever path
/// they are given, and a `.ns` file cannot begin with these four bytes because
/// every module starts with the `Newspeak3` language identifier.
fn looks_like_an_artifact(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == ns_runtime::image::encode::MAGIC
}
```

Then restructure `cmd_disasm` so that everything from `resolve_mixin_path` onward is
unchanged, and only the acquisition of `class` and `module` branches:

```rust
fn cmd_disasm(
    file: &Path,
    selector: Option<&str>,
    initializer: bool,
    class_path: Option<&str>,
) -> ExitCode {
    let raw = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };

    // Held only so the borrows below outlive the branch: the source path owns
    // its interpreter, the artifact path owns the loaded world.
    let (class, module) = if looks_like_an_artifact(&raw) {
        match ns_runtime::image::load(&raw) {
            Ok(loaded) => {
                let module = loaded
                    .interp
                    .compiled_module()
                    .expect("load installed the bytecode engine")
                    .clone();
                (loaded.root_class.clone(), module)
            }
            Err(e) => {
                eprintln!("error: {}: {e}", file.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        let loaded = match load_source(file) {
            Ok(l) => l,
            Err(code) => return code,
        };
        let bindings = loaded.resolved.bindings.clone();
        let (_interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
        let module = ns_runtime::compiler::compile_module(&class, &bindings);
        (class, module)
    };

    // ... unchanged from here: root_mixin, resolve_mixin_path, the
    // (selector, initializer) match, and the final print.
}
```

If `CompiledModule` is not `Clone`, derive it (`#[derive(Clone)]` on the struct in
`crates/ns-runtime/src/bytecode.rs`) rather than restructuring the branch — every
field is already `HashMap`/`Rc`, so the derive is cheap and total.

- [x] **Step 4: Run the test**

Run: `cargo test -p ns-cli --test disasm_snapshots`

Expected: PASS, including the six pre-existing snapshot tests, which must be
byte-identical — this refactor changes how `class` and `module` are obtained, not
what is printed.

- [x] **Step 5: Add the failure-path test**

Append:

```rust
/// A file that is neither parseable source nor a valid artifact must report
/// one clear failure, not a decoder backtrace.
#[test]
fn disasm_rejects_a_corrupt_artifact_with_a_message() {
    let dir = std::env::temp_dir().join(format!("ns-next-disasm-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let bad = dir.join("truncated.img");
    // Magic, so it is taken for an artifact, and then nothing.
    std::fs::write(&bad, b"NSIM").expect("write");

    let out = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&bad)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error:"), "unexpected stderr: {err}");
    assert!(!err.contains("panicked"), "decoder panicked: {err}");
}
```

- [x] **Step 6: Run the full suite and the gates**

```bash
cargo test --workspace
cargo fmt --all && cargo clippy --workspace --all-targets
```

Expected: green, warning-free.

- [x] **Step 7: Commit**

```bash
git add crates/ns-cli/src/main.rs crates/ns-cli/tests/disasm_snapshots.rs
git commit -m "feat: disassemble an artifact, not only source

disasm required a .ns file and compiled it, so the bytecode actually in
an image could not be printed -- worst for the artifact a deployment
binary is meant to be, which ships without source. It now sniffs the
magic and loads instead, reusing the mixin-path walk unchanged because a
decoded root class carries the same nested-mixin tree."
```

---

### Task 2: `ns-next inspect`

A structural listing of an artifact: header flags, section offsets, the symbol
table, the mixin table, and the object and frame counts. This is what makes a failed
prefix-equality check in Task 10 actionable, and what makes "the platform is cut out
of an artifact" checkable against the artifact instead of against the encoder.

**Files:**
- Create: `crates/ns-runtime/src/image/inspect.rs`
- Modify: `crates/ns-runtime/src/image/mod.rs` (declare the module),
  `crates/ns-cli/src/main.rs` (the subcommand)
- Test: `crates/ns-cli/tests/inspect.rs`

**Interfaces:**
- Consumes: `decode::{Reader, read_header, decode_source_origin, decode_code_table,
  decode_graph_section}`, `symbols::decode`, `mixins::decode`, `Reader::pos()`.
- Produces: `ns_runtime::image::inspect::{Report, Section, inspect, render}` —
  `pub fn inspect(bytes: &[u8]) -> Result<Report, String>` and
  `pub fn render(report: &Report) -> String`. Task 10 uses `inspect` to diff two
  artifacts structurally.

- [x] **Step 1: Write the failing test**

Create `crates/ns-cli/tests/inspect.rs`:

```rust
//! `ns-next inspect` -- the structural listing of an artifact.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-fixtures")
        .join(name)
}

fn temp_dir(test: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ns-next-inspect-{}-{n}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn ns_next(args: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .args(args)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

#[test]
fn inspect_reports_the_sections_symbols_and_mixins_of_an_image() {
    let dir = temp_dir("arithmetic");
    let img = dir.join("world.img");
    let (ok, _, err) = ns_next(&[
        "snapshot".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        img.as_os_str(),
    ]);
    assert!(ok, "{err}");

    let (ok, out, err) = ns_next(&["inspect".as_ref(), img.as_os_str()]);
    assert!(ok, "{err}");

    assert!(out.contains("format version 1"), "{out}");
    assert!(out.contains("stateless: no"), "{out}");
    assert!(out.contains("source blob: absent"), "{out}");
    for section in [
        "header", "origin", "symbols", "code", "mixins", "root class", "entry", "graph",
    ] {
        assert!(out.contains(section), "missing section `{section}`:\n{out}");
    }
    // The module's own entry selector is in the symbol table because every
    // send instruction names its selector.
    assert!(out.contains("main:args:"), "{out}");
}

#[test]
fn inspect_reports_a_debug_variants_source_blob() {
    let dir = temp_dir("debug-source");
    let img = dir.join("world.img");
    let (ok, _, err) = ns_next(&[
        "snapshot".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        img.as_os_str(),
        "--debug-source".as_ref(),
    ]);
    assert!(ok, "{err}");

    let (ok, out, err) = ns_next(&["inspect".as_ref(), img.as_os_str()]);
    assert!(ok, "{err}");
    assert!(out.contains("source blob: present"), "{out}");
}

#[test]
fn inspect_reports_a_corrupt_artifact_rather_than_panicking() {
    let dir = temp_dir("corrupt");
    let bad = dir.join("bad.img");
    std::fs::write(&bad, b"NSIM\x01\x00\x00\x00").expect("write");

    let (ok, _, err) = ns_next(&["inspect".as_ref(), bad.as_os_str()]);
    assert!(!ok);
    assert!(err.starts_with("error:"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}
```

- [x] **Step 2: Run it and watch it fail**

Run: `cargo test -p ns-cli --test inspect`

Expected: FAIL — `error: unrecognized subcommand 'inspect'`, so `ok` is false in the
first test.

- [x] **Step 3: Write the inspector**

Create `crates/ns-runtime/src/image/inspect.rs`:

```rust
//! A read-only structural view of an artifact: where each section starts and
//! ends, what the symbol and mixin tables hold, and how much graph there is.
//!
//! This exists because an artifact was otherwise opaque. `disasm` shows one
//! compiled body; nothing showed the file. Two things need that. A failed
//! prefix-equality check between a `build` and a `snapshot` of the same source
//! reports a differing byte offset, which says nothing on its own and
//! everything once the offset is attributed to a section. And the claim that
//! the platform is cut out of an artifact -- the reason an image is kilobytes
//! rather than megabytes -- was checkable only by reading the encoder.
//!
//! Nothing here installs an engine, allocates a class object, or runs
//! anything. It decodes, records, and returns.

use std::fmt::Write as _;

use crate::image::{decode, mixins, symbols};

/// One named span of the file. `end` is exclusive.
pub struct Section {
    pub name: &'static str,
    pub start: usize,
    pub end: usize,
    /// A short count or value for the section, empty when it has none.
    pub detail: String,
}

pub struct Report {
    pub format_version: u32,
    pub has_source: bool,
    pub stateless: bool,
    pub origin: String,
    pub sections: Vec<Section>,
    pub symbols: Vec<String>,
    /// One line per mixin row: name, slot count, factory, method count.
    pub mixins: Vec<String>,
    pub entry: Option<String>,
    pub total_len: usize,
}

/// Decodes `bytes` far enough to describe it. Every failure is an `Err`, never
/// a panic -- an artifact is untrusted input here exactly as it is in `load`.
pub fn inspect(bytes: &[u8]) -> Result<Report, String> {
    let mut r = decode::Reader::new(bytes);
    let mut sections = Vec::new();
    let mut at = 0usize;

    let header = decode::read_header(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "header",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    if header.has_source {
        let source = r.read_str().map_err(|e| e.to_string())?;
        sections.push(Section {
            name: "source",
            start: at,
            end: r.pos(),
            detail: format!("{} bytes", source.len()),
        });
        at = r.pos();
    }

    let origin = decode::decode_source_origin(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "origin",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    let symbol_table = symbols::decode(&mut r).map_err(|e| e.to_string())?;
    let symbol_list = symbol_table.all().iter().map(|s| s.to_string()).collect::<Vec<_>>();
    sections.push(Section {
        name: "symbols",
        start: at,
        end: r.pos(),
        detail: format!("{} symbols", symbol_list.len()),
    });
    at = r.pos();

    let table = decode::decode_code_table(&mut r, &symbol_table).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "code",
        start: at,
        end: r.pos(),
        detail: format!("{} bodies", table.codes.len()),
    });
    at = r.pos();

    let decoded = mixins::decode(&mut r, &symbol_table, &table.codes).map_err(|e| e.to_string())?;
    let mixin_lines = decoded
        .mixins
        .iter()
        .zip(decoded.code.iter())
        .map(|(m, c)| {
            format!(
                "{}  slots: {}  factory: {}  methods: {}",
                m.name,
                m.instance_slots.len(),
                match &m.factory {
                    Some(f) => crate::interpreter::instantiate::method_name(f).to_string(),
                    None => "-".to_string(),
                },
                c.methods.len(),
            )
        })
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "mixins",
        start: at,
        end: r.pos(),
        detail: format!("{} mixins", decoded.mixins.len()),
    });
    at = r.pos();

    // The root class's superclass reference. Decoding it needs an interpreter
    // only to resolve a builtin tag to a live class, which inspection does not
    // want; the bytes are read and their span recorded without resolving.
    decode::skip_root_superclass(&mut r).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "root class",
        start: at,
        end: r.pos(),
        detail: String::new(),
    });
    at = r.pos();

    let entry = decode::decode_entry(&mut r, &symbol_table).map_err(|e| e.to_string())?;
    sections.push(Section {
        name: "entry",
        start: at,
        end: r.pos(),
        detail: match &entry {
            Some(sym) => sym.to_string(),
            None => "none".to_string(),
        },
    });
    at = r.pos();

    if !header.stateless {
        let counts = decode::count_graph_section(&mut r, &symbol_table).map_err(|e| e.to_string())?;
        sections.push(Section {
            name: "graph",
            start: at,
            end: r.pos(),
            detail: format!(
                "{} objects, {} frames, {} roots",
                counts.objects, counts.frames, counts.roots
            ),
        });
    }

    Ok(Report {
        format_version: crate::image::encode::FORMAT_VERSION,
        has_source: header.has_source,
        stateless: header.stateless,
        origin: format!("{origin:?}"),
        sections,
        symbols: symbol_list,
        mixins: mixin_lines,
        entry: entry.map(|s| s.to_string()),
        total_len: bytes.len(),
    })
}

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "format version {}", report.format_version);
    let _ = writeln!(
        out,
        "stateless: {}",
        if report.stateless { "yes" } else { "no" }
    );
    let _ = writeln!(
        out,
        "source blob: {}",
        if report.has_source { "present" } else { "absent" }
    );
    let _ = writeln!(out, "origin: {}", report.origin);
    let _ = writeln!(out, "total: {} bytes", report.total_len);

    let _ = writeln!(out, "\nsections");
    for s in &report.sections {
        let _ = writeln!(
            out,
            "  {:<12} {:>8}..{:<8} {}",
            s.name, s.start, s.end, s.detail
        );
    }

    let _ = writeln!(out, "\nsymbols ({})", report.symbols.len());
    for (i, s) in report.symbols.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {s}");
    }

    let _ = writeln!(out, "\nmixins ({})", report.mixins.len());
    for (i, m) in report.mixins.iter().enumerate() {
        let _ = writeln!(out, "  {i:>4} {m}");
    }

    out
}
```

Three helpers this calls do not exist yet and must be added:

1. `symbols::Table::all(&self) -> &[Symbol]` in
   `crates/ns-runtime/src/image/symbols.rs` — a getter over the private `symbols`
   field.
2. `decode::skip_root_superclass(r: &mut Reader) -> Result<(), DecodeError>` in
   `decode.rs`. Factor the existing `decode_root_superclass` so both share one
   private reader for the tag and its payload; the existing function then resolves
   the tag against the interpreter and this one discards it. Do **not** duplicate
   the reading logic — a second copy is how the two drift and the recorded span
   stops matching the real one.
3. `decode::count_graph_section(r: &mut Reader, symbols: &symbols::Table) ->
   Result<GraphCounts, DecodeError>` with
   `pub struct GraphCounts { pub objects: usize, pub frames: usize, pub roots: usize }`.
   Implement it by reading the section's three counts and then advancing the reader
   over the section exactly as `decode_graph_section` does — the simplest correct
   form is to call `decode_graph_section` with a throwaway `DecodeContext` if one can
   be built without an interpreter; if it cannot, read the counts and return, and
   record the graph section's `end` as `bytes.len()` instead of `r.pos()`, noting in
   a comment that the graph is the last section so its end is the file's end.

`decode::decode_entry` arrives in Task 4; until then, stub it in `inspect.rs` as
`Ok(None)` with a `// Task 4 replaces this` comment, and remove the stub in Task 4.

- [x] **Step 4: Declare the module and add the subcommand**

In `crates/ns-runtime/src/image/mod.rs`, add `pub mod inspect;` beside the other
module declarations, keeping them alphabetical.

In `crates/ns-cli/src/main.rs`, add to the `Cmd` enum:

```rust
    /// Print an artifact's structure: header flags, section offsets, the
    /// symbol and mixin tables, and how much graph it carries.
    Inspect { artifact: PathBuf },
```

dispatch it:

```rust
        Cmd::Inspect { artifact } => cmd_inspect(&artifact),
```

and implement:

```rust
/// `ns-next inspect`: what is in this artifact. Read-only -- nothing is
/// bootstrapped, instantiated or run, so inspecting a hostile file exercises
/// only the decoders.
fn cmd_inspect(artifact: &Path) -> ExitCode {
    let bytes = match std::fs::read(artifact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", artifact.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => {
            print!("{}", ns_runtime::image::inspect::render(&report));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}: {e}", artifact.display());
            ExitCode::FAILURE
        }
    }
}
```

- [x] **Step 5: Run the tests**

Run: `cargo test -p ns-cli --test inspect`

Expected: PASS, all three.

- [x] **Step 6: Gates**

```bash
cargo test --workspace
cargo fmt --all && cargo clippy --workspace --all-targets
```

- [x] **Step 7: Commit**

```bash
git add crates/ns-runtime/src/image/ crates/ns-cli/src/main.rs crates/ns-cli/tests/inspect.rs
git commit -m "feat: add ns-next inspect, a structural view of an artifact

An artifact was opaque: disasm shows one compiled body and nothing
showed the file. Two things need that view -- a failed prefix-equality
check reports a byte offset, which means nothing until it is attributed
to a section, and the claim that the platform is cut out of an artifact
was checkable only by reading the encoder."
```

---

## Phase 2 — The format

### Task 3: `load` refuses trailing bytes

`load` stops when the last section has been read and ignores whatever follows.
`Reader::remaining()` exists but serves only as an allocation bound. With two
artifact shapes this becomes a confusion vector: a binary with graph-shaped garbage
appended loads without complaint.

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`load`),
  `crates/ns-runtime/src/image/decode.rs` (`DecodeError`)
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `DecodeError::TrailingBytes { count: usize }`.

- [x] **Step 1: Write the failing test**

Append to `crates/ns-runtime/tests/image.rs` (use the file's existing helpers for
building a real image — copy the setup from a neighbouring test such as
`a_truncated_image_is_an_error_not_a_panic`):

```rust
#[test]
fn bytes_after_the_last_section_are_refused_not_ignored() {
    let mut bytes = /* build a valid image exactly as the neighbouring tests do */;
    assert!(image::load(&bytes).is_ok(), "the fixture itself must load");

    bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let err = image::load(&bytes).expect_err("trailing bytes must be refused");
    assert!(
        err.contains("trailing"),
        "expected a trailing-bytes error, got: {err}"
    );
}
```

- [x] **Step 2: Run it and watch it fail**

Run: `cargo test -p ns-runtime --test image bytes_after_the_last_section`

Expected: FAIL — `load` returns `Ok`, so `expect_err` panics.

- [x] **Step 3: Add the error and the check**

In `decode.rs`, add to `DecodeError`:

```rust
    /// Bytes remain after the last section. Refused rather than ignored: with
    /// two artifact shapes in one format, silently accepting a tail means a
    /// binary with a graph appended -- or an image truncated to a binary and
    /// then padded -- loads as something its author did not write.
    TrailingBytes { count: usize },
```

with its `Display` arm:

```rust
    DecodeError::TrailingBytes { count } => {
        write!(f, "{count} trailing byte(s) after the last section")
    }
```

In `mod.rs`'s `load`, immediately before `interp.set_engine(...)`:

```rust
    if r.remaining() != 0 {
        return Err(decode::DecodeError::TrailingBytes {
            count: r.remaining(),
        }
        .to_string());
    }
```

Place it before `set_engine` and before `mark_decoded_from_image` so a refusal
leaves no half-built interpreter behind, matching the ordering comment already in
`load`.

- [x] **Step 4: Run the test**

Run: `cargo test -p ns-runtime --test image`

Expected: PASS. The whole image suite must stay green — if an existing test's
fixture had slack bytes, that fixture was wrong and the test is now telling you so;
fix the fixture, not the check.

- [x] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/image/ crates/ns-runtime/tests/image.rs
git commit -m "fix: refuse bytes after an image's last section

The reader stopped at the last section and ignored the tail. With two
artifact shapes sharing one format that is a confusion vector: a binary
with graph-shaped bytes appended loaded without complaint."
```

---

### Task 4: `FLAG_STATELESS` and the entry field

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs`,
  `crates/ns-runtime/src/image/decode.rs`,
  `crates/ns-runtime/src/image/inspect.rs` (drop the Task 2 stub)
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces:
  - `encode::FLAG_STATELESS: u8 = 2`, `encode::KNOWN_FLAGS` updated.
  - `encode::encode_entry(w: &mut Writer, entry: Option<u32>)` — writes a presence
    byte and, when present, the symbol id.
  - `decode::Header { has_source: bool, stateless: bool }`.
  - `decode::decode_entry(r: &mut Reader, symbols: &symbols::Table) ->
    Result<Option<Symbol>, DecodeError>`.

- [x] **Step 1: Write the failing tests**

Append to `crates/ns-runtime/tests/image.rs`:

```rust
#[test]
fn the_entry_field_round_trips_present_and_absent() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut symbol_builder = symbols::Builder::new();
    let id = symbol_builder.intern("main:args:");
    let mut w = encode::Writer::new();
    symbol_builder.write(&mut w);
    encode::encode_entry(&mut w, Some(id));
    encode::encode_entry(&mut w, None);
    let bytes = w.into_bytes();

    let mut r = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut r).expect("symbol table");
    let present = decode::decode_entry(&mut r, &table).expect("present entry");
    let absent = decode::decode_entry(&mut r, &table).expect("absent entry");

    assert_eq!(present.map(|s| s.to_string()).as_deref(), Some("main:args:"));
    assert_eq!(absent, None);
}

#[test]
fn an_out_of_range_entry_symbol_id_is_a_decode_error_not_a_panic() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut w = encode::Writer::new();
    symbols::Builder::new().write(&mut w);
    encode::encode_entry(&mut w, Some(9999));
    let bytes = w.into_bytes();

    let mut r = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut r).expect("symbol table");
    assert!(decode::decode_entry(&mut r, &table).is_err());
}

#[test]
fn an_unknown_header_flag_is_still_rejected_now_that_two_are_known() {
    // Bit 2 is not assigned. A flag this build does not implement means a
    // section it cannot see, so continuing would read the next field out of
    // whatever that section occupies.
    let mut w = ns_runtime::image::encode::Writer::new();
    ns_runtime::image::encode::write_header(&mut w, 0b100);
    let bytes = w.into_bytes();
    let mut r = ns_runtime::image::decode::Reader::new(&bytes);
    assert!(ns_runtime::image::decode::read_header(&mut r).is_err());
}
```

- [x] **Step 2: Run and watch them fail**

Run: `cargo test -p ns-runtime --test image entry_field`

Expected: FAIL to compile — `encode_entry` and `decode_entry` do not exist.

- [x] **Step 3: Implement**

In `encode.rs`, beside `FLAG_HAS_SOURCE`:

```rust
/// The artifact carries no object graph: it is a deployment binary, not a
/// snapshot of a running world. Its entry field is mandatory rather than
/// optional, which is what makes it startable from the file alone.
pub const FLAG_STATELESS: u8 = 2;
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_STATELESS;
```

and:

```rust
/// The entry selector, as a presence byte and -- when present -- a symbol id.
///
/// One shape for both artifacts on purpose: the decoder reads it the same way
/// either way and only the *requirement* differs, so there is no second
/// code path to keep in step. It sits after the root class rather than in the
/// header because it names a symbol and must therefore follow the symbol
/// table, and because down here it leaves the describing sections byte-equal
/// between a `build` and a `snapshot` of the same source.
pub fn encode_entry(w: &mut Writer, entry: Option<u32>) {
    match entry {
        None => w.write_u8(0),
        Some(id) => {
            w.write_u8(1);
            w.write_u32(id);
        }
    }
}
```

In `decode.rs`, extend `Header`:

```rust
pub struct Header {
    pub has_source: bool,
    /// No graph section follows; the file ends after the entry field.
    pub stateless: bool,
}
```

and set it in `read_header`:

```rust
    Ok(Header {
        has_source: flags & super::encode::FLAG_HAS_SOURCE != 0,
        stateless: flags & super::encode::FLAG_STATELESS != 0,
    })
```

and add:

```rust
pub fn decode_entry(
    r: &mut Reader,
    symbols: &symbols::Table,
) -> Result<Option<Symbol>, DecodeError> {
    match r.read_u8()? {
        0 => Ok(None),
        1 => Ok(Some(symbols.get(r.read_u32()?)?)),
        tag => Err(DecodeError::InvalidTag {
            what: "entry",
            tag,
        }),
    }
}
```

Match the exact shape of `InvalidTag` already used in this file; if its fields
differ, use them rather than changing the variant.

Finally, delete the `Ok(None)` stub in `inspect.rs` and call the real
`decode::decode_entry`.

- [x] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime --test image`

Expected: PASS, whole file green.

- [x] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/image/
git commit -m "feat: add the stateless header flag and the entry field

One encoding for both artifacts -- a presence byte and a symbol id --
so the decoder has a single path and only the requirement differs. The
field sits after the root class because it names a symbol and because
that leaves the describing sections byte-equal between the two."
```

---

### Task 5: `image::build`

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs`
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `pub fn build(source: Option<&str>, origin: &SourceOrigin,
  root_class: &ClassRef, module: &CompiledModule, entry: Symbol) ->
  Result<Vec<u8>, String>`.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_built_binary_and_a_snapshot_of_the_same_source_agree_up_to_the_root_class() {
    // Build both from one compiled module so the only difference is the
    // artifact shape, then compare through the inspector rather than by raw
    // offset: a raw offset says two files differ, a section says which one.
    let (class, module, origin) = /* the setup a neighbouring snapshot test uses */;
    let binary = image::build(None, &origin, &class, &module, Symbol::intern("main:args:"))
        .expect("build");
    let img = image::snapshot(None, &origin, &class, &module, &platform, &roots).expect("snapshot");

    let b = image::inspect::inspect(&binary).expect("inspect binary");
    let i = image::inspect::inspect(&img).expect("inspect image");

    let upto = |r: &image::inspect::Report| {
        r.sections
            .iter()
            .take_while(|s| s.name != "entry")
            .map(|s| (s.name, s.start, s.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(upto(&b), upto(&i), "the describing sections must line up");

    let end = b
        .sections
        .iter()
        .find(|s| s.name == "root class")
        .expect("root class section")
        .end;
    assert_eq!(&binary[..end], &img[..end], "describing bytes must be equal");

    assert!(b.stateless);
    assert!(!i.stateless);
    assert_eq!(b.entry.as_deref(), Some("main:args:"));
}
```

- [x] **Step 2: Run and watch it fail**

Run: `cargo test -p ns-runtime --test image a_built_binary_and_a_snapshot`

Expected: FAIL to compile — `image::build` does not exist.

- [x] **Step 3: Factor the shared prefix, then add `build`**

In `mod.rs`, extract from `snapshot` the part both artifacts share. The constraint
that shapes this: the symbol table is written *before* the code, mixin and graph
sections but is filled *by* them, so those are encoded into their own buffers first
and only the finished table is written ahead of them. `build` has the same
constraint with the entry selector in place of the graph.

```rust
/// The sections that describe code and structure, encoded into their own
/// buffers with the symbol table still open so the caller can intern into it.
///
/// Both artifacts go through this, which is what makes their describing bytes
/// equal by construction rather than by discipline. A second copy would drift
/// at the first format change, and the test that compares them would only find
/// out afterwards.
struct Describing {
    symbols: symbols::Builder,
    code: encode::Writer,
    mixins: encode::Writer,
    ids: encode::CodeIds,
    mixin_ids: mixins::MixinIds,
}

fn encode_describing(
    root_class: &ClassRef,
    module: &CompiledModule,
) -> Result<Describing, String> {
    let mut symbols = symbols::Builder::new();
    let mut code = encode::Writer::new();
    let ids = encode::encode_code_table(&mut code, module, root_class, &mut symbols)?;
    let mut mixin_w = encode::Writer::new();
    let mixin_ids = mixins::encode(&mut mixin_w, root_class, module, &mut symbols, &ids)?;
    Ok(Describing {
        symbols,
        code,
        mixins: mixin_w,
        ids,
        mixin_ids,
    })
}

/// Writes header through root class. Everything after that is the caller's:
/// the entry field, and for an image the graph.
fn write_describing(
    w: &mut encode::Writer,
    source: Option<&str>,
    origin: &SourceOrigin,
    flags: u8,
    d: Describing,
    root_class: &ClassRef,
) -> Result<(), String> {
    encode::write_header(
        w,
        flags
            | match source {
                Some(_) => encode::FLAG_HAS_SOURCE,
                None => 0,
            },
    );
    if let Some(source) = source {
        w.write_str(source);
    }
    encode::encode_source_origin(w, origin);
    d.symbols.write(w);
    w.write_raw(&d.code.into_bytes());
    w.write_raw(&d.mixins.into_bytes());
    encode::encode_root_class(w, root_class)?;
    Ok(())
}
```

Rewrite `snapshot` to use both, encoding the graph into its own buffer against
`d.symbols` before calling `write_describing`, then writing
`encode_entry(&mut w, None)` followed by the graph bytes.

Add:

```rust
/// Serializes a **stateless binary**: code and structure, no instance state,
/// no activations, and the selector to send on start-up.
///
/// The caller has compiled but deliberately not instantiated. That omission is
/// the whole point of this artifact -- the primary factory and every
/// initializer run on load instead, against the loading world's own freshly
/// bootstrapped platform, so the file has never held a capability.
pub fn build(
    source: Option<&str>,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    entry: Symbol,
) -> Result<Vec<u8>, String> {
    let mut d = encode_describing(root_class, module)?;
    let entry_id = d.symbols.intern(entry.as_str());
    let mut w = encode::Writer::new();
    write_describing(&mut w, source, origin, encode::FLAG_STATELESS, d, root_class)?;
    encode::encode_entry(&mut w, Some(entry_id));
    Ok(w.into_bytes())
}
```

The `entry_id` must be interned **before** `write_describing` writes the table.
Ordering that wrongly gives an id the table does not contain, which the decoder
reports as an out-of-range symbol id — the case Task 4's test already covers.

If `CodeIds` or `MixinIds` are private today, widen them to `pub(crate)` rather than
restructuring; keeping them private would force a second encoding path, which is
what this factoring exists to prevent.

- [x] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime --test image`

Expected: PASS. Every pre-existing snapshot round-trip test must stay green: this
step changes how the bytes are assembled, not what they are — except for the new
entry presence byte, which is now written into every image. If a hand-written byte
fixture in the test file asserts an exact layout past the root class, update that
fixture, and say so in the commit.

- [x] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/image/mod.rs crates/ns-runtime/tests/image.rs
git commit -m "feat: add image::build for the stateless binary

Both artifacts now go through one encoder for the sections that
describe code and structure, so their bytes agree there by construction
rather than by discipline -- a second copy would drift at the first
format change and the comparison test would find out only afterwards."
```

---

### Task 6: `load` branches on the flag

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs`
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `LoadedImage.entry: Option<Symbol>`, `LoadedImage.stateless: bool`,
  and `roots` empty on the stateless path.

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_binary_loads_with_no_roots_and_its_entry() {
    let (class, module, origin) = /* same setup as Task 5's test */;
    let binary = image::build(None, &origin, &class, &module, Symbol::intern("main:args:"))
        .expect("build");

    let loaded = image::load(&binary).expect("load a binary");
    assert!(loaded.stateless);
    assert!(loaded.roots.is_empty(), "a binary carries no instance");
    assert_eq!(loaded.entry.map(|s| s.to_string()).as_deref(), Some("main:args:"));
    assert!(
        loaded.interp.compiled_module().is_some(),
        "the bytecode engine must be installed"
    );
}

#[test]
fn a_stateless_artifact_without_an_entry_is_refused() {
    // Hand-build the header with FLAG_STATELESS and write an absent entry.
    // A binary that cannot say how it starts is not a deployment artifact,
    // and accepting one would push the failure to a later, less obvious place.
    let mut bytes = /* a valid binary from build(), as above */;
    // Flip its entry presence byte to 0. The entry field is the last thing in
    // a binary, so it is the final byte when the entry id is stripped.
    let entry_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|s| s.name == "entry")
        .expect("entry section")
        .start;
    bytes.truncate(entry_start);
    bytes.push(0);

    let err = image::load(&bytes).expect_err("a binary must carry an entry");
    assert!(err.contains("entry"), "unexpected error: {err}");
}
```

- [x] **Step 2: Run and watch them fail**

Run: `cargo test -p ns-runtime --test image a_binary_loads`

Expected: FAIL — `LoadedImage` has no `stateless` or `entry` field.

- [x] **Step 3: Implement**

Add to `LoadedImage`:

```rust
    /// The selector to send after instantiation. Mandatory in a binary,
    /// always `None` in an image today.
    pub entry: Option<Symbol>,
    /// The artifact carried no object graph, so `roots` is empty and the
    /// caller is expected to instantiate.
    pub stateless: bool,
```

In `load`, after `build_root_class` and before the graph:

```rust
    let entry = decode::decode_entry(&mut r, &symbols).map_err(|e| e.to_string())?;
    if header.stateless && entry.is_none() {
        return Err("a stateless artifact must record an entry selector".to_string());
    }
```

Then make the graph conditional:

```rust
    let graph = if header.stateless {
        decode::DecodedGraph::empty()
    } else {
        let platform = interp.platform();
        let mut ctx = decode::DecodeContext { /* unchanged */ };
        decode::decode_graph_section(&mut r, &mut ctx, &symbols).map_err(|e| e.to_string())?
    };
    if !header.stateless {
        decode::verify_graph(&graph, &table, &module).map_err(|e| e.to_string())?;
        interp.advance_identity_hash_past(graph.max_identity_hash);
    }
```

Add `DecodedGraph::empty()` in `decode.rs` returning zeroed counts, no objects, no
frames, no roots, and `max_identity_hash: 0`. Calling `verify_graph` on it would
also be correct, since it iterates an empty frame list — skipping it is the clearer
statement that there is nothing to verify.

The trailing-byte check from Task 3 now covers both shapes without change, which is
the point of having added it first.

Finally populate the two new fields in the returned `LoadedImage`.

- [x] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime --test image`

Expected: PASS.

- [x] **Step 5: Gates and commit**

```bash
cargo test --workspace && cargo fmt --all && cargo clippy --workspace --all-targets
git add crates/ns-runtime/src/image/ crates/ns-runtime/tests/image.rs
git commit -m "feat: load a stateless binary, with no graph and an entry

A binary that cannot say how it starts is refused at load rather than
later: the failure belongs where the artifact is read, not where the
caller eventually fails to find something to send."
```

---

## Phase 3 — The command line

### Task 7: `ns-next build`

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Test: `crates/ns-cli/tests/build_load.rs` (create)

**Interfaces:**
- Consumes: `image::build`, `factory_args_for`, `selector_arity`.
- Produces: the `build` subcommand.

- [x] **Step 1: Write the failing tests**

Create `crates/ns-cli/tests/build_load.rs` with the same `fixture` / `temp_dir` /
`ns_next` helpers as `crates/ns-cli/tests/inspect.rs` (copy them; a shared helper
crate is not worth a dependency for three functions), then:

```rust
#[test]
fn build_writes_a_stateless_binary_that_inspect_recognizes() {
    let dir = temp_dir("build");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{err}");

    let (ok, listing, err) = ns_next(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{err}");
    assert!(listing.contains("stateless: yes"), "{listing}");
    assert!(listing.contains("main:args:"), "{listing}");
    // The whole point of the artifact: no instance state travelled with it.
    assert!(!listing.contains("graph"), "{listing}");
}

#[test]
fn build_refuses_an_entry_the_root_class_does_not_have() {
    let dir = temp_dir("bad-entry");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "noSuchSelector".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("noSuchSelector"), "{err}");
    assert!(!out.exists(), "a refused build must write no file");
}

#[test]
fn build_refuses_an_entry_arity_the_cli_cannot_supply() {
    let dir = temp_dir("bad-arity");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "a:b:c:".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("argument"), "{err}");
}

#[test]
fn build_requires_an_entry_when_the_root_class_declares_no_main() {
    // TestActor.ns answers `return42` and declares no `main:args:`.
    let dir = temp_dir("no-main");
    let out = dir.join("app.nsb");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(!ok);
    assert!(err.contains("--entry"), "{err}");
}
```

- [x] **Step 2: Run and watch them fail**

Run: `cargo test -p ns-cli --test build_load`

Expected: FAIL — `unrecognized subcommand 'build'`.

- [x] **Step 3: Implement**

Add to `Cmd`:

```rust
    /// Compile a file into a stateless deployment binary: code and
    /// structure, no instance state. Nothing is instantiated -- the primary
    /// factory and every initializer run when the binary is loaded.
    Build {
        file: PathBuf,
        out: PathBuf,
        /// The selector to record as the entry point. Defaults to
        /// `main:args:` when the root class declares it.
        #[arg(long)]
        entry: Option<String>,
        /// Store the module's source text in the binary (the debug variant).
        #[arg(long)]
        debug_source: bool,
    },
```

Then:

```rust
/// Whether the root class can answer `selector`: a method anywhere up its
/// application chain, or the accessor of one of its nested declarations.
///
/// Checked at build time so a binary that could never start is refused where
/// the mistake was made. It is a convenience, not a guarantee -- a selector
/// that slips through still fails at start-up as an ordinary
/// `doesNotUnderstand:`, through the same path `run` uses.
fn root_class_answers(class: &ns_runtime::ClassRef, selector: &str) -> bool {
    let sym = ns_runtime::Symbol::intern(selector);
    let mut app = match &class.borrow().data {
        ns_runtime::ObjectData::Class(data) => Some(data.application.clone()),
        _ => None,
    };
    while let Some(a) = app {
        if a.mixin.methods.contains_key(&sym) {
            return true;
        }
        if a.mixin.nested.iter().any(|n| n.name == sym) {
            return true;
        }
        app = a.superclass.clone();
    }
    false
}

/// `ns-next build`: `run`'s pipeline up to engine installation, and then
/// deliberately no instantiation.
fn cmd_build(file: &Path, out: &Path, entry: Option<&str>, debug_source: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (interp, class) = bootstrap_with_engine(&loaded, EngineArg::Bytecode);

    let entry = match entry {
        Some(e) => e.to_string(),
        None if root_class_answers(&class, "main:args:") => "main:args:".to_string(),
        None => {
            eprintln!(
                "error: {} declares no `main:args:`; pass --entry with the selector to send",
                file.display()
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(msg) = entry_args(&entry, interp.platform()) {
        eprintln!("error: cannot use `{entry}` as an entry point: {msg}");
        return ExitCode::FAILURE;
    }
    if !root_class_answers(&class, &entry) {
        eprintln!("error: the root class of {} does not answer `{entry}`", file.display());
        return ExitCode::FAILURE;
    }

    let module = interp
        .compiled_module()
        .expect("bootstrap_with_engine installed the bytecode engine");
    let bytes = match ns_runtime::image::build(
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        &class,
        module,
        ns_runtime::Symbol::intern(&entry),
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: build failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}
```

Wire it into the dispatch `match` beside `Cmd::Snapshot`.

`Mixin.nested` holds `Rc<Mixin>` values whose `name` is the accessor name; if the
field shape differs, adapt the walk to what `crates/ns-runtime/src/mixin.rs`
actually declares rather than changing the mixin.

- [x] **Step 4: Run the tests**

Run: `cargo test -p ns-cli --test build_load`

Expected: PASS, all four.

- [x] **Step 5: Commit**

```bash
git add crates/ns-cli/
git commit -m "feat: add ns-next build for the stateless binary

Refuses at build time an entry the root class cannot answer or whose
arity the CLI cannot supply, so a binary that could never start fails
where the mistake was made rather than at someone else's start-up."
```

---

### Task 8: `load` starts a binary

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (`cmd_load`)
- Test: `crates/ns-cli/tests/build_load.rs`

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_built_binary_runs_exactly_as_its_source_does() {
    let dir = temp_dir("equivalence");
    let out = dir.join("app.nsb");
    let src = fixture("arithmetic.ns");

    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(ok, "{err}");

    let (run_ok, run_out, run_err) = ns_next(&["run".as_ref(), src.as_os_str()]);
    let (load_ok, load_out, load_err) = ns_next(&["load".as_ref(), out.as_os_str()]);

    assert_eq!(load_ok, run_ok, "exit status differs");
    assert_eq!(load_out, run_out, "stdout differs");
    assert_eq!(load_err, run_err, "stderr differs");
}

#[test]
fn loading_a_binary_runs_its_initializers_against_the_loading_platform() {
    // instance_init.ns writes through its initializer, so if the factory and
    // initializers did not run on load the entry send would see nil slots.
    let dir = temp_dir("init");
    let out = dir.join("app.nsb");
    let src = fixture("instance_init.ns");
    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(ok, "{err}");

    let (run_ok, run_out, _) = ns_next(&["run".as_ref(), src.as_os_str()]);
    let (load_ok, load_out, _) = ns_next(&["load".as_ref(), out.as_os_str()]);
    assert_eq!((load_ok, load_out), (run_ok, run_out));
}

#[test]
fn load_send_overrides_the_recorded_entry() {
    let dir = temp_dir("override");
    let out = dir.join("app.nsb");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        src.as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "return42".as_ref(),
    ]);
    assert!(ok, "{err}");

    let (ok, _, err) = ns_next(&[
        "load".as_ref(),
        out.as_os_str(),
        "--send".as_ref(),
        "return42".as_ref(),
    ]);
    assert!(ok, "{err}");
}
```

- [x] **Step 2: Run and watch them fail**

Run: `cargo test -p ns-cli --test build_load a_built_binary_runs`

Expected: FAIL — `cmd_load` sends to `roots[0]`, which does not exist, so it
reports an error or panics on an empty vector.

- [x] **Step 3: Implement**

In `cmd_load`, after destructuring `LoadedImage`, branch on whether an instance
came with the artifact:

```rust
    // A binary carries no instance: the primary factory and every initializer
    // run here, against this interpreter's freshly bootstrapped platform, so
    // capabilities enter at start time rather than having been baked in at
    // build time. Instantiating in the CLI rather than in `image::load` keeps
    // an existing boundary: deciding *that* a command-line entry point
    // supplies the platform is a CLI concern -- nothing in the runtime needs
    // to know it was invoked from a terminal.
    let target = if roots.is_empty() {
        let platform = interp.platform();
        let factory_args = match factory_args_for(&root_class, platform) {
            Ok(args) => args,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        };
        match interp.instantiate(root_class.clone(), factory_args, &empty_bindings) {
            Ok(v) => v,
            Err(signal) => return report_signal(signal),
        }
    } else {
        roots[0].clone()
    };
```

`cmd_load` currently binds `root_class: _`; bind it properly. Then choose the
selector:

```rust
    let selector = match (send, &entry) {
        (Some(s), _) => s.to_string(),
        (None, Some(recorded)) => recorded.to_string(),
        (None, None) => {
            // A stateful image records no entry today, so this is the
            // pre-existing "load without --send" case: report and stop.
            eprintln!("error: this artifact records no entry point; pass --send");
            return ExitCode::FAILURE;
        }
    };
```

Preserve the existing behaviour for a stateful image loaded without `--send` — if it
currently succeeds by doing nothing, keep that and only apply the message above on
the stateless path. Read `cmd_load` before editing and keep every existing exit path
intact; `crates/ns-cli/tests/snapshot_load.rs` pins them.

- [x] **Step 4: Run the tests**

Run: `cargo test -p ns-cli`

Expected: PASS, including `snapshot_load.rs` unchanged.

- [x] **Step 5: Commit**

```bash
git add crates/ns-cli/
git commit -m "feat: start a stateless binary from load

The primary factory and every initializer run at load, against the
loading world's own platform, which is what makes the artifact stateless
in the first place: it has never held a capability."
```

---

## Phase 4 — Gates and documentation

### Task 9: Equivalence across every fixture

Task 8 proved equivalence for two fixtures. This makes it a gate over all of them,
which is what actually protects the property.

**Files:**
- Modify: `crates/ns-cli/tests/build_load.rs`

- [x] **Step 1: Write the test**

```rust
/// `build` then `load` must be indistinguishable from `run` on the same
/// source: exit status, stdout and stderr.
///
/// This is the binary's substitute for the two-engine differential check,
/// which it structurally cannot have -- a binary has no AST, so the
/// tree-walker cannot run it. Because the differential harness already pins
/// bytecode against tree-walk over these same fixtures, equality here holds
/// transitively against the tree-walker too.
#[test]
fn every_fixture_runs_identically_from_source_and_from_a_binary() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let dir = temp_dir("all-fixtures");
    let mut checked = 0;

    for entry in std::fs::read_dir(&fixtures).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let out = dir.join(format!(
            "{}.nsb",
            path.file_stem().unwrap().to_string_lossy()
        ));

        let (built, _, build_err) =
            ns_next(&["build".as_ref(), path.as_os_str(), out.as_os_str()]);
        if !built {
            // A fixture whose root class declares no `main:args:` and needs an
            // explicit entry is not a failure of this property. Anything else
            // is, so the reason is asserted rather than assumed.
            assert!(
                build_err.contains("--entry"),
                "build failed for {}: {build_err}",
                path.display()
            );
            continue;
        }

        let (run_ok, run_out, run_err) = ns_next(&["run".as_ref(), path.as_os_str()]);
        let (load_ok, load_out, load_err) = ns_next(&["load".as_ref(), out.as_os_str()]);
        assert_eq!(load_ok, run_ok, "exit status differs for {}", path.display());
        assert_eq!(load_out, run_out, "stdout differs for {}", path.display());
        assert_eq!(load_err, run_err, "stderr differs for {}", path.display());
        checked += 1;
    }

    assert!(checked >= 5, "expected several fixtures to be covered, got {checked}");
}
```

- [x] **Step 2: Run it**

Run: `cargo test -p ns-cli --test build_load every_fixture -- --nocapture`

Expected: PASS. If a fixture fails, the message names it — fix the runtime, not the
test. A divergence here is exactly the class of defect this gate exists for.

- [x] **Step 3: Commit**

```bash
git add crates/ns-cli/tests/build_load.rs
git commit -m "test: pin build+load against run over every fixture"
```

---

### Task 10: Prefix equality as a gate

**Files:**
- Modify: `crates/ns-cli/tests/build_load.rs`

- [x] **Step 1: Write the test**

```rust
/// For one source and one --debug-source setting, a build and a snapshot must
/// agree byte for byte from `origin` through `root class`.
///
/// Not "the binary is a prefix of the image" -- that is false as soon as one
/// records an entry and the other does not. The guarantee is about the
/// sections that describe code and structure, and it is what keeps the shared
/// section writers honest.
#[test]
fn a_build_and_a_snapshot_agree_on_the_describing_sections() {
    let dir = temp_dir("prefix");
    let src = fixture("arithmetic.ns");
    let bin = dir.join("app.nsb");
    let img = dir.join("world.img");

    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), bin.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");

    let bin_bytes = std::fs::read(&bin).expect("read binary");
    let img_bytes = std::fs::read(&img).expect("read image");

    let end_of = |bytes: &[u8]| {
        ns_runtime::image::inspect::inspect(bytes)
            .expect("inspect")
            .sections
            .iter()
            .find(|s| s.name == "root class")
            .expect("root class section")
            .end
    };
    let a = end_of(&bin_bytes);
    let b = end_of(&img_bytes);
    assert_eq!(a, b, "the describing sections end at different offsets");
    assert_eq!(
        &bin_bytes[..a],
        &img_bytes[..b],
        "describing bytes differ; run `ns-next inspect` on both to see which section"
    );
}
```

This test needs `ns-runtime` as a dev-dependency of `ns-cli`; check
`crates/ns-cli/Cargo.toml` and add it under `[dev-dependencies]` if the crate is
only a normal dependency today.

- [x] **Step 2: Run it**

Run: `cargo test -p ns-cli --test build_load a_build_and_a_snapshot`

Expected: PASS.

- [x] **Step 3: Commit**

```bash
git add crates/ns-cli/
git commit -m "test: pin the describing sections equal between build and snapshot"
```

---

### Task 11: A binary lane in `run-corpus`

**Files:**
- Modify: wherever `run-corpus` is implemented (grep `run-corpus` in
  `crates/ns-cli/src/`)

- [x] **Step 1: Read the existing lane**

Read the current implementation end to end before changing it. It runs each
whitelisted module under both engines and compares against the recorded
`expected_status` and `expected_stdout`. The binary lane is a third run of the same
row, not a new configuration format: **a row's `send` is the build's `--entry`**, so
`corpus/runnable.toml` needs no new field.

- [x] **Step 2: Add the lane**

For each entry, after the two engine runs:

1. `build` the module into a temporary file with `--entry <row.send>`.
2. `load` it with no `--send`, so the recorded entry is what starts it.
3. Compare exit status and stdout against the same recorded expectations, and
   compare stderr against the bytecode engine's.

Keep the reported summary line exactly `ran N, failed M, skipped K` — the gate in
`CLAUDE.md`, `README.md` and `docs/design/README.md` quotes it, and changing the
wording silently breaks a documented check. Count a row once, not three times.

- [x] **Step 3: Run the gate**

```bash
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: `ran 6, failed 0, skipped 205`.

- [x] **Step 4: Commit**

```bash
git add crates/ns-cli/
git commit -m "test: run every whitelisted corpus module from a binary too

A row's send is the build's entry, so the whitelist needs no new field."
```

---

### Task 12: Untrusted-input coverage for the new bytes

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs`

- [x] **Step 1: Write the tests**

Follow the file's existing pattern — build a real artifact, then corrupt it at a
located offset. Add:

```rust
#[test]
fn every_truncation_of_a_binary_is_reported_rather_than_panicking() {
    let binary = /* build one, as in Task 5's test */;
    for cut in 0..binary.len() {
        assert!(
            image::load(&binary[..cut]).is_err(),
            "a binary truncated to {cut} bytes must be refused"
        );
    }
}

#[test]
fn a_binary_with_a_graph_appended_is_refused() {
    // The shape Task 3's trailing-byte check exists for, stated as the case
    // that motivated it rather than only as a generic tail.
    let binary = /* build one */;
    let image_bytes = /* snapshot the same source */;
    let mut frankenstein = binary.clone();
    frankenstein.extend_from_slice(&image_bytes[binary.len().min(image_bytes.len())..]);
    assert!(image::load(&frankenstein).is_err());
}

#[test]
fn a_stateless_flag_on_an_artifact_that_has_a_graph_is_refused() {
    // Setting the flag without removing the graph leaves the graph as
    // trailing bytes, which must be refused rather than ignored.
    let mut bytes = /* snapshot */;
    bytes[8] |= ns_runtime::image::encode::FLAG_STATELESS; // the flag byte
    assert!(image::load(&bytes).is_err());
}
```

Locate the flag byte from the header layout rather than hard-coding `8` if the
offset differs — magic is 4 bytes, `FORMAT_VERSION` is a `u32`, so the flag byte is
at offset 8; assert that in the test so a header change fails loudly.

- [x] **Step 2: Run them**

Run: `cargo test -p ns-runtime --test image`

Expected: PASS. Any panic here is a real defect in the decoder, not a bad test.

- [x] **Step 3: Commit**

```bash
git add crates/ns-runtime/tests/image.rs
git commit -m "test: cover the binary's bytes as untrusted input"
```

---

### Task 13: Documentation

**Files:**
- Modify: `docs/design/image-format.md`, `docs/design/compiler.md`,
  `docs/design/README.md`, `docs/PROGRESS.md`, `README.md`, `CLAUDE.md`

- [x] **Step 1: Flip the status in the design documents**

In `docs/design/image-format.md`:
- Status block: the stateless binary is **built**, not planned; add `inspect.rs` to
  the code list and `ns-next build` / `ns-next inspect` to the CLI list.
- Rename "Planned: reading an artifact" and "Planned: the stateless binary" to
  drop the `Planned:` prefix, and rewrite each in the present tense — present tense
  means it exists, and leaving a built mechanism described as planned is exactly the
  drift the living-document convention exists to prevent.
- Move the "Known gap" about trailing bytes out of "Verification": it is now the
  behaviour, so state it as such.
- Leave the open questions that are still open: the decompiler, upstream's
  `keepSource:`, whether an image should record its entry, multi-module artifacts.

In `docs/design/compiler.md`, drop the "Planned: it will accept an artifact too"
paragraph and fold the capability into the `disasm` description.

In `docs/design/README.md`, update the map row for `image-format.md`: status and
what is planned next.

- [x] **Step 2: Update the user-facing documents**

- `README.md`: add `build` and `inspect` to the usage examples, with a real command
  each. Run both commands exactly as written before committing.
- `CLAUDE.md`: the binary inventory currently lists eight subcommands; it is now
  ten. Keep it in the same order as `main.rs`'s `Cmd` enum.
- `docs/PROGRESS.md`: this work is not a milestone of its own — add it to the M3
  section or open an M4 row only if the user has said so. Ask rather than guess.

- [x] **Step 3: Verify the documented commands actually work**

```bash
cargo run -p ns-cli -- build tests-fixtures/arithmetic.ns /tmp/app.nsb
cargo run -p ns-cli -- inspect /tmp/app.nsb
cargo run -p ns-cli -- load /tmp/app.nsb
```

Every command in the README must be run as written before it is committed. A
documented command that does not work is worse than an undocumented one.

- [x] **Step 4: Final gates**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules   # parsed 210, failed 0, expected-fail 1, stale 0
cargo run -p ns-cli -- run-corpus corpus/modules     # ran 6, failed 0, skipped 205
```

- [x] **Step 5: Commit**

```bash
git add docs/ README.md CLAUDE.md
git commit -m "docs: record artifact inspection and the stateless binary as built"
```

---

## Self-Review

**Spec coverage.** Checked section by section against
`docs/design/image-format.md`:

| Spec section | Task |
|---|---|
| Planned: reading an artifact — `disasm` on an artifact | 1 |
| Planned: reading an artifact — structural listing | 2 |
| Planned: reading an artifact — no loadable text form | not built, by decision; nothing to implement |
| Verification — trailing bytes | 3 |
| Layout — `FLAG_STATELESS`, entry field placement and encoding | 4 |
| Planned: the stateless binary — `build` and shared section writers | 5 |
| Planned: the stateless binary — loading and branching | 6 |
| Planned: the stateless binary — entry default and validation | 7 |
| Planned: the stateless binary — instantiation stays in the CLI | 8 |
| Tests 1 (equivalence) | 8, 9 |
| Tests 2 (prefix equality) | 10 |
| Tests 3 (run-corpus binary lane) | 11 |
| Tests 4 (untrusted input) | 12 |
| Status flips | 13 |

No spec requirement is unassigned. "Suspension is out of scope" and the four open
questions are explicitly not implemented here.

**Placeholder scan.** Three steps deliberately say "as the neighbouring test does"
rather than reproducing a fixture setup: Task 3 Step 1, Task 5 Step 1 and Task 12
Step 1. That is a pointer to code in the repository, not a TBD — `image.rs` builds
its fixtures through helpers that the implementer must read anyway, and transcribing
them here would go stale against the file. Every other step carries its code.

**Type consistency.** `image::build` takes `entry: Symbol` (Task 5) and `cmd_build`
passes `Symbol::intern(&entry)` (Task 7). `decode_entry` returns
`Option<Symbol>` (Task 4), stored as `LoadedImage.entry: Option<Symbol>` (Task 6)
and read as `entry` in `cmd_load` (Task 8) and as `Option<String>` in the inspector's
`Report` (Task 2, via `map(|s| s.to_string())`). `inspect` and `render` are used by
Tasks 2, 5, 6, 10. `FLAG_STATELESS` is defined in Task 4 and used in Tasks 5, 6, 12.

**Two known unknowns**, flagged rather than guessed, because both depend on code the
plan could not fully verify without writing it:

1. Task 2's `count_graph_section` may not be implementable without a
   `DecodeContext`, which needs an interpreter. The step says what to do in that
   case — record the graph section's end as the file's end, since it is the last
   section — and why that is sound.
2. Task 1 assumes `CompiledModule: Clone`. The step says to derive it if it is not,
   and why that is cheap.
