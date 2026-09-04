use std::path::Path;
use std::process::Command;

/// Normalizes line endings before comparison. The CLI always writes `\n`
/// (via `println!`/`print!` on any platform, Rust does not translate
/// this), but the checked-in `.expected` fixture files can be checked out
/// as `\r\n` on Windows when `core.autocrlf` is enabled -- a git checkout
/// artifact, not a difference in program behavior. Comparing raw strings
/// would make this test flaky across platforms/git configs; normalize
/// both sides the same way instead.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[test]
fn every_tests_fixtures_ns_file_produces_its_expected_stdout() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let expected_path = path.with_extension("expected");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .output()
            .expect("run ns-next");
        assert!(
            output.status.success(),
            "{} exited non-zero: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            normalize_newlines(stdout.trim_end()),
            normalize_newlines(expected.trim_end()),
            "{} produced unexpected stdout",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected at least the 3 fixtures this task adds"
    );
}
