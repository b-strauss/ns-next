//! Every fixture, plus inline sources that must not be fixtures (they exit
//! non-zero), run under both engines; exit status, stdout, AND stderr must
//! match byte for byte. The Milestone 1 tree-walker is the oracle (M2 spec,
//! "Testing").

use std::path::{Path, PathBuf};
use std::process::Command;

/// Same normalization as `run_fixtures.rs`: `.expected` files may be
/// checked out with `\r\n` on Windows.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub struct Outcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_with_engine(path: &Path, engine: &str) -> Outcome {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(path)
        .arg("--engine")
        .arg(engine)
        .output()
        .expect("run ns-next");
    Outcome {
        ok: output.status.success(),
        stdout: normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
        stderr: normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
    }
}

/// Runs `path` under both engines, asserts they agree on everything
/// observable, and returns the bytecode engine's outcome for further
/// assertions.
pub fn assert_engines_agree(path: &Path) -> Outcome {
    let bc = run_with_engine(path, "bytecode");
    let tw = run_with_engine(path, "tree-walk");
    assert_eq!(
        bc.ok,
        tw.ok,
        "{}: exit status differs (bytecode stderr: {}; tree-walk stderr: {})",
        path.display(),
        bc.stderr,
        tw.stderr
    );
    assert_eq!(
        bc.stdout,
        tw.stdout,
        "{}: stdout differs between engines",
        path.display()
    );
    assert_eq!(
        bc.stderr,
        tw.stderr,
        "{}: stderr differs between engines",
        path.display()
    );
    bc
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures")
}

/// Writes `src` to a per-test temp file (unique per test name and process,
/// so parallel tests never collide) and returns its path.
fn write_temp_source(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ns-next-differential-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{name}.ns"));
    std::fs::write(&path, src).expect("write temp source");
    path
}

pub fn assert_engines_agree_on_source(name: &str, src: &str) -> Outcome {
    let path = write_temp_source(name, src);
    let outcome = assert_engines_agree(&path);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    outcome
}

#[test]
fn every_tests_fixtures_ns_file_behaves_identically_under_both_engines() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir()).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let outcome = assert_engines_agree(&path);
        assert!(
            outcome.ok,
            "{} failed under both engines: {}",
            path.display(),
            outcome.stderr
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected the 3 Milestone 1 fixtures plus instance_init.ns"
    );
}

#[test]
fn the_instance_init_fixture_runs_its_initializer_once_under_bytecode() {
    let outcome = run_with_engine(&fixtures_dir().join("instance_init.ns"), "bytecode");
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "init ran\nhi 2\n");
}

#[test]
fn a_does_not_understand_error_is_identical_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "dnu",
        "Newspeak3\n'T'\nclass Dnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 fooBar\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(
        outcome.stderr.contains("does not understand #fooBar"),
        "{}",
        outcome.stderr
    );
}
