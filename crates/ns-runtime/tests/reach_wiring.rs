//! The reach record stays wired to what it records (`ns_memory::reach`;
//! `docs/design/README.md`, "The tests a gate leaves out"): the stress
//! and Docker gates leave out every test the record says reaches no
//! runtime, so a switch read or an interpreter made where nothing records
//! it would let a gate leave out a test the switch changes.
//!
//! The rule, for every crate's `src/` outside its test modules: every
//! string naming a stress switch (`NS_NEXT_STRESS_`) or `NS_NEXT_NO_JIT`
//! stands in a reader this file names, which either records itself or is
//! called only from `Interpreter::with_kernel`, after that records
//! `runtime`; and an `Interpreter` is built only there.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `directory`, subdirectories included.
fn sources(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(directory).expect("the sources are readable");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// A line's code: its text before a `//` comment.
fn code_of(line: &str) -> &str {
    line.find("//").map_or(line, |start| &line[..start])
}

/// Whether `line` is a `cfg` attribute that holds its item to test builds:
/// `#[cfg(test)]`, or an `all(...)` with `test` among its operands.
fn is_test_configuration(line: &str) -> bool {
    let line = line.trim();
    line == "#[cfg(test)]"
        || line
            .strip_prefix("#[cfg(all(")
            .and_then(|rest| rest.strip_suffix("))]"))
            .is_some_and(|operands| operands.split(',').any(|operand| operand.trim() == "test"))
}

/// The lines of `source` outside its test modules, numbered from 1: a
/// `mod` item with a body right after a test `cfg` is skipped to the brace
/// that closes it.
fn lines_outside_test_modules(source: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((number, line)) = lines.next() {
        let opens_a_test_module = is_test_configuration(line)
            && lines.peek().is_some_and(|(_, next)| {
                next.trim_start().starts_with("mod ") && next.trim_end().ends_with('{')
            });
        if !opens_a_test_module {
            kept.push((number + 1, line));
            continue;
        }
        let mut depth = 0_i64;
        for (_, module_line) in lines.by_ref() {
            let code = code_of(module_line);
            depth += code.matches('{').count() as i64;
            depth -= code.matches('}').count() as i64;
            if depth <= 0 {
                break;
            }
        }
    }
    kept
}

/// The name of the function `line` declares, if it declares one.
fn declared_function(line: &str) -> Option<&str> {
    let code = code_of(line);
    let start = code.find("fn ")?;
    let before = code[..start].chars().next_back();
    if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    let rest = &code[start + 3..];
    let end =
        rest.find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))?;
    (end > 0).then(|| &rest[..end])
}

/// A source file of the workspace's crates: its path below `crates/`, with
/// `/` as separator, and its text.
struct Source {
    name: String,
    text: String,
}

/// The files of `sources` that are test modules of their own: a `mod x;`
/// right after a test `cfg` names `x.rs` or `x/mod.rs` beside its file --
/// in the file's directory for a `mod.rs`, `lib.rs` or `main.rs`, in the
/// directory of its name otherwise.
fn test_module_files(sources: &[Source]) -> Vec<String> {
    let mut found = Vec::new();
    for source in sources {
        let (directory, file) = source
            .name
            .rsplit_once('/')
            .expect("a source lies in a directory");
        let module_directory = match file {
            "mod.rs" | "lib.rs" | "main.rs" => directory.to_string(),
            _ => format!("{directory}/{}", file.trim_end_matches(".rs")),
        };
        let lines: Vec<&str> = source.text.lines().collect();
        for pair in lines.windows(2) {
            let Some(module) = pair[1]
                .trim()
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
            else {
                continue;
            };
            if is_test_configuration(pair[0]) {
                found.push(format!("{module_directory}/{module}.rs"));
                found.push(format!("{module_directory}/{module}/mod.rs"));
            }
        }
    }
    found
}

/// Every source of the workspace's crates but the test modules that are
/// files of their own.
fn workspace_sources() -> Vec<Source> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates directory")
        .to_path_buf();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("the crates are readable") {
        let source_directory = entry.expect("a crate").path().join("src");
        if !source_directory.is_dir() {
            continue;
        }
        for path in sources(&source_directory) {
            let name = path
                .strip_prefix(&crates)
                .expect("below the crates")
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let text = std::fs::read_to_string(&path).expect("a source is readable");
            found.push(Source { name, text });
        }
    }
    let test_only = test_module_files(&found);
    found.retain(|source| !test_only.contains(&source.name));
    found
}

/// The function each of `source`'s lines outside its test modules stands
/// in -- the declaration line of the nearest function above it, trimmed --
/// with the line.
fn lines_with_their_function(source: &Source) -> Vec<(usize, Option<String>, &str)> {
    let mut current = None;
    lines_outside_test_modules(&source.text)
        .into_iter()
        .map(|(number, line)| {
            if declared_function(line).is_some() {
                current = Some(code_of(line).trim().to_string());
            }
            (number, current.clone(), line)
        })
        .collect()
}

/// Whether the declaration line `declaration` is the one `signature`
/// names: it holds the signature's text, which names the function and,
/// where several share the name, enough of its parameters or result.
fn declares(declaration: Option<&str>, signature: &str) -> bool {
    declaration.is_some_and(|declaration| declaration.contains(signature))
}

/// The body of the function `signature` names in `source`, from its
/// declaration to the brace that closes it.
fn body_of<'a>(source: &'a Source, signature: &str) -> Vec<&'a str> {
    let lines: Vec<&str> = source.text.lines().collect();
    let start = lines
        .iter()
        .position(|line| declared_function(line).is_some() && line.contains(signature))
        .unwrap_or_else(|| panic!("{} declares no {signature}", source.name));
    let mut depth = 0_i64;
    let mut opened = false;
    let mut body = Vec::new();
    for line in &lines[start..] {
        body.push(*line);
        let code = code_of(line);
        depth += code.matches('{').count() as i64;
        depth -= code.matches('}').count() as i64;
        opened |= code.contains('{');
        if opened && depth <= 0 {
            break;
        }
    }
    body
}

/// Where every interpreter is built, and every kernel reader called.
const KERNEL: (&str, &str) = (
    "ns-runtime/src/interpreter/mod.rs",
    "fn with_kernel(lowering: Lowering, path: ShippedCodePath) -> Interpreter",
);

/// The readers that record themselves, by file and signature.
const RECORDING_READERS: [(&str, &str); 9] = [
    ("ns-runtime/src/jit/mod.rs", "fn stress_jit_from_env()"),
    ("ns-runtime/src/jit/mod.rs", "fn stress_exit_from_env()"),
    ("ns-runtime/src/jit/mod.rs", "fn stress_zone_from_env()"),
    ("ns-runtime/src/jit/mod.rs", "fn stress_optimize_from_env()"),
    ("ns-runtime/src/jit/mod.rs", "fn stress_deopt_from_env()"),
    (
        "ns-runtime/src/jit/mod.rs",
        "fn stress_invalidate_from_env()",
    ),
    // Reads `NS_NEXT_NO_JIT`.
    ("ns-runtime/src/jit/mod.rs", "fn new() -> JitState"),
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn stress_wide_from_env()",
    ),
    KERNEL,
];

/// The readers called only from `with_kernel`: each one's file, signature
/// and how a call of it is written.
const KERNEL_READERS: [(&str, &str, &str); 6] = [
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn stress_caches_from_env()",
        "stress_caches_from_env(",
    ),
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn stress_install_from_env()",
        "stress_install_from_env(",
    ),
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn stress_breakpoints_from_env()",
        "stress_breakpoints_from_env(",
    ),
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn from_env() -> Option<StressMoves>",
        "StressMoves::from_env(",
    ),
    (
        "ns-runtime/src/interpreter/safepoint.rs",
        "fn from_env(variable: &str) -> Option<StressSchedule>",
        "StressSchedule::from_env(",
    ),
    (
        "ns-runtime/src/debugger/mod.rs",
        "fn from_env() -> Option<StepStress>",
        "StepStress::from_env(",
    ),
];

fn source_named<'a>(sources: &'a [Source], name: &str) -> &'a Source {
    sources
        .iter()
        .find(|source| source.name == name)
        .unwrap_or_else(|| panic!("no source {name}"))
}

#[test]
fn every_switch_read_stands_in_a_reader_the_record_covers() {
    let sources = workspace_sources();
    assert!(sources.len() > 100, "the scan reached the crates' sources");
    let mut violations = Vec::new();
    let mut reads = 0;
    for source in &sources {
        for (number, function, line) in lines_with_their_function(source) {
            let code = code_of(line);
            if !code.contains("\"NS_NEXT_STRESS_") && !code.contains("\"NS_NEXT_NO_JIT") {
                continue;
            }
            reads += 1;
            let function = function.as_deref();
            let covered = RECORDING_READERS
                .iter()
                .any(|(file, reader)| source.name == *file && declares(function, reader))
                || KERNEL_READERS
                    .iter()
                    .any(|(file, reader, _)| source.name == *file && declares(function, reader));
            if !covered {
                violations.push(format!(
                    "{}:{number} (in {}): {}",
                    source.name,
                    function.unwrap_or("no function"),
                    line.trim()
                ));
            }
        }
    }
    assert!(reads >= 15, "the scan found the switch reads: {reads}");
    assert!(
        violations.is_empty(),
        "a runtime switch is read where the reach record does not see it; read it in a \
         reader that calls `reach::record(Reach::Runtime)`, and name it here:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_recording_reader_records() {
    let sources = workspace_sources();
    for (file, reader) in RECORDING_READERS {
        let body = body_of(source_named(&sources, file), reader);
        assert!(
            body.iter()
                .any(|line| code_of(line).contains("reach::record(")),
            "{file}: {reader} reads a switch but records nothing"
        );
    }
}

#[test]
fn the_kernel_readers_are_called_only_after_the_kernels_record() {
    let sources = workspace_sources();
    let mut violations = Vec::new();
    for (_, _, call) in KERNEL_READERS {
        let mut calls = 0;
        for source in &sources {
            for (number, function, line) in lines_with_their_function(source) {
                if !code_of(line).contains(call) || declared_function(line).is_some() {
                    continue;
                }
                calls += 1;
                if source.name != KERNEL.0 || !declares(function.as_deref(), KERNEL.1) {
                    violations.push(format!("{}:{number}: {}", source.name, line.trim()));
                }
            }
        }
        assert!(calls > 0, "{call} is called somewhere");
    }
    assert!(
        violations.is_empty(),
        "a reader the record covers only through `with_kernel` is called elsewhere:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_interpreter_is_built_after_its_record() {
    let sources = workspace_sources();
    let mut built_at = Vec::new();
    for source in &sources {
        for (number, function, line) in lines_with_their_function(source) {
            let code = code_of(line).trim();
            if code.contains("= Interpreter {") || code.starts_with("Interpreter {") {
                built_at.push((source.name.clone(), number, function));
            }
        }
    }
    assert!(
        built_at.len() == 1
            && built_at.iter().all(|(name, _, function)| {
                name == KERNEL.0 && declares(function.as_deref(), KERNEL.1)
            }),
        "an `Interpreter` is built only in `with_kernel`: {:?}",
        built_at
            .iter()
            .map(|(name, number, _)| format!("{name}:{number}"))
            .collect::<Vec<_>>()
    );
    let body = body_of(source_named(&sources, KERNEL.0), KERNEL.1);
    let record = body
        .iter()
        .position(|line| code_of(line).contains("reach::record(crate::reach::Reach::Runtime)"));
    let built = body
        .iter()
        .position(|line| code_of(line).contains("= Interpreter {"));
    assert!(
        matches!((record, built), (Some(record), Some(built)) if record < built),
        "`with_kernel` records `runtime` before it builds the interpreter"
    );
}

#[test]
fn the_scan_finds_a_read_in_a_function_but_not_in_a_test_module() {
    let source = Source {
        name: "example.rs".to_string(),
        text: concat!(
            "fn reader() {\n",
            "    std::env::var_os(\"NS_NEXT_STRESS_X\");\n",
            "}\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn probe() {\n",
            "        \"NS_NEXT_STRESS_Y\";\n",
            "    }\n",
            "}\n",
        )
        .to_string(),
    };
    let found: Vec<(usize, Option<String>)> = lines_with_their_function(&source)
        .into_iter()
        .filter(|(_, _, line)| line.contains("\"NS_NEXT_STRESS_"))
        .map(|(number, function, _)| (number, function))
        .collect();
    assert_eq!(found, [(2, Some("fn reader() {".to_string()))]);
    assert_eq!(
        declared_function("    pub(crate) fn new() -> JitState {"),
        Some("new")
    );
    assert_eq!(declared_function("    // fn not_code()"), None);
    assert_eq!(declared_function("let left_fn x"), None);
    assert_eq!(body_of(&source, "fn reader()").len(), 3);
    let module_file = Source {
        name: "crate/src/jit/mod.rs".to_string(),
        text: "#[cfg(all(test, ns_jit))]\nmod inlining_tests;\nmod leaves;\n".to_string(),
    };
    assert_eq!(
        test_module_files(&[module_file]),
        [
            "crate/src/jit/inlining_tests.rs",
            "crate/src/jit/inlining_tests/mod.rs"
        ]
    );
}
