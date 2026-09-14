use std::path::Path;
use std::process::Command;

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

/// Every fixture succeeds, prints exactly its `.expected` stdout -- every
/// byte, the final newline included -- and writes nothing to stderr, under
/// both lowerings.
#[test]
fn every_tests_fixtures_ns_file_produces_its_expected_stdout() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("ns") {
            continue;
        }
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
        checked += 1;
    }
    // A floor rather than whatever the directory yields: a fixture that
    // stops being discovered -- renamed, moved, its extension changed --
    // loses its expected-output check without any test failing, so the
    // floor tracks the actual count.
    assert!(
        checked >= 34,
        "expected every one of the 34 tests-fixtures/*.ns fixtures to be \
         compared against its .expected file, checked {checked} -- raise \
         this floor when adding a fixture, never lower it"
    );
}
