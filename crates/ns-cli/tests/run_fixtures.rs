use std::process::Command;

mod common;

/// Normalizes line endings before comparison. The CLI always writes `\n`
/// (via `println!`/`print!` on any platform, Rust does not translate
/// this), but the checked-in `.expected` fixture files can be checked out
/// as `\r\n` on Windows when `core.autocrlf` is enabled -- a git checkout
/// artifact, not a difference in program behavior. Comparing raw strings
/// would make this test flaky across platforms/git configs; normalize
/// both sides the same way instead.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Both lowerings: the optimized one every program runs under, and the
/// reference lowering it is checked against (`docs/design/compiler.md`, "The
/// reference lowering"). Each must produce the recorded answer.
const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// Every golden succeeds, prints exactly its `.expected` stdout -- every
/// byte, the final newline included -- and writes nothing to stderr, under
/// both lowerings.
#[test]
fn every_tests_fixtures_ns_file_produces_its_expected_stdout() {
    for path in common::golden_sources() {
        let expected_path = path.with_extension("expected");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
        for lowering in LOWERINGS {
            let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
                .arg("run")
                .arg(&path)
                .arg("--lowering")
                .arg(lowering)
                .output()
                .expect("run ns-next");
            assert!(
                output.status.success(),
                "{} exited non-zero under the {lowering} lowering: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "",
                "{} wrote to stderr under the {lowering} lowering",
                path.display()
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                normalize_newlines(&stdout),
                normalize_newlines(&expected),
                "{} produced unexpected stdout under the {lowering} lowering",
                path.display()
            );
        }
    }
}

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

/// Every fixture answers alike when the global cache is one two-slot group,
/// so that every insert contends for the same group
/// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, read by debug builds only).
#[cfg(debug_assertions)]
#[test]
fn every_fixture_answers_alike_with_a_two_entry_global_cache() {
    for path in common::golden_sources() {
        let expected = std::fs::read_to_string(path.with_extension("expected"))
            .expect("every fixture has its expected output");
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "2")
            .output()
            .expect("run ns-next");
        assert!(output.status.success(), "{} failed", path.display());
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            normalize_newlines(&expected),
            "{} with a two-entry global cache",
            path.display()
        );
    }
}
