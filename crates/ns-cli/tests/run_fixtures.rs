use std::process::Command;

mod common;

/// Normalizes line endings before comparison. The CLI always writes `\n`
/// (via `println!`/`print!` on any platform, Rust does not translate
/// this), but the checked-in `.toml` expectation files can be checked out
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

/// Every golden's exit status, stdout and stderr match its recorded
/// `Expectation` exactly -- every byte, the final newline included -- under
/// both lowerings.
#[test]
fn every_golden_matches_its_recorded_expectation() {
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
        let expectation = source.with_extension("toml");
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
            if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
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
    for source in common::golden_sources() {
        let expectation = common::expectation_of(&source);
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&source)
            .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "2")
            .output()
            .expect("run ns-next");
        assert_eq!(
            output.status.code(),
            Some(expectation.status),
            "{} exit status with a two-entry global cache",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            normalize_newlines(&expectation.stdout),
            "{} stdout with a two-entry global cache",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
            normalize_newlines(&expectation.stderr),
            "{} stderr with a two-entry global cache",
            source.display()
        );
    }
}
