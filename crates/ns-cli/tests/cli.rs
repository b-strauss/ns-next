mod common;

use common::ScratchDirectory;
use indoc::indoc;
use std::process::Command;

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

/// The repository root, so a test can name the real vendored corpus
/// regardless of the working directory the test harness happens to use.
fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ns-cli is two levels below the repository root")
        .to_path_buf()
}

#[test]
fn parse_ok_file() {
    let directory = ScratchDirectory::new("parse-ok");
    let file = directory.join("Ok.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'X'
            class C = ()
        "},
    )
    .unwrap();
    let out = ns_next().arg("parse").arg(&file).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parse_bad_file_exits_1() {
    let directory = ScratchDirectory::new("parse-bad");
    let file = directory.join("Bad.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'X'
            class C = (
        "},
    )
    .unwrap();
    let out = ns_next().arg("parse").arg(&file).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

#[test]
fn parse_corpus_reports_counts() {
    let directory = ScratchDirectory::new("corpus-counts");
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("Good.ns"),
        indoc! {"
            Newspeak3
            'X'
            class C = ()
        "},
    )
    .unwrap();
    std::fs::write(
        modules.join("AlsoGood.ns"),
        indoc! {"
            Newspeak3
            'X'
            class D = ()
        "},
    )
    .unwrap();
    std::fs::write(
        modules.join("Bad.ns"),
        indoc! {"
            Newspeak3
            'X'
            class C = (
        "},
    )
    .unwrap();
    let blacklist = directory.join("blacklist.toml");
    std::fs::write(
        &blacklist,
        "[[entry]]\nfile = \"Bad.ns\"\nreason = \"deliberately malformed for the test\"\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&blacklist)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("parsed 2"), "{stdout}");
    assert!(stdout.contains("failed 0"), "{stdout}");
    assert!(stdout.contains("expected-fail 1"), "{stdout}");
}

#[test]
fn parse_corpus_stale_blacklist_entry_fails() {
    let directory = ScratchDirectory::new("corpus-stale");
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("Good.ns"),
        indoc! {"
            Newspeak3
            'X'
            class C = ()
        "},
    )
    .unwrap();
    let blacklist = directory.join("blacklist.toml");
    // Names a file that actually parses now -> stale.
    std::fs::write(
        &blacklist,
        "[[entry]]\nfile = \"Good.ns\"\nreason = \"no longer fails\"\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&blacklist)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("stale"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parse_corpus_malformed_blacklist_fails_with_error() {
    let directory = ScratchDirectory::new("corpus-malformed");
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("Good.ns"),
        indoc! {"
            Newspeak3
            'X'
            class C = ()
        "},
    )
    .unwrap();
    let blacklist = directory.join("blacklist.toml");
    std::fs::write(&blacklist, "this is not valid toml :::\n[[entry\n").unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&blacklist)
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("blacklist"), "stderr: {stderr}");
}

/// A module whose entry method writes to the transcript, so a synthetic
/// `run-corpus` whitelist can exercise `expected_stdout` against real
/// output -- the vendored corpus modules that run today print nothing.
const TALKATIVE: &str = indoc! {"
    Newspeak3
    'X'
    class Talkative usingPlatform: p = Object new (
    ) (
        public main: platform args: a = (
            platform transcript show: 'hi'; newLine
        )
    ) : ()
"};

/// Lays out a `modules/` directory with `Talkative.ns` in it plus a
/// sibling `runnable.toml` holding `whitelist`, in a scratch directory the
/// caller keeps alive for as long as `run-corpus` reads it; point
/// `run-corpus` at its `modules` subdirectory.
fn run_corpus_fixture(name: &str, whitelist: &str) -> ScratchDirectory {
    let directory = ScratchDirectory::new(name);
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("Talkative.ns"), TALKATIVE).unwrap();
    std::fs::write(directory.join("runnable.toml"), whitelist).unwrap();
    directory
}

#[test]
fn the_run_corpus_gate_reports_its_counts() {
    let out = ns_next()
        .arg("run-corpus")
        .arg(repository_root().join("corpus/modules"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("ran "), "stdout was: {stdout}");
    assert!(stdout.contains("failed 0"), "stdout was: {stdout}");
}

#[test]
fn run_corpus_checks_a_whitelisted_module_against_its_expected_stdout() {
    let directory = run_corpus_fixture(
        "run-corpus-ok",
        "[[entry]]\nfile = \"Talkative.ns\"\nsend = \"main:args:\"\n\
         expected_status = 0\nexpected_stdout = \"hi\\n\"\nreason = \"fixture\"\n",
    );
    let modules = directory.join("modules");
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("ran 1, failed 0, skipped 0"), "{stdout}");
}

#[test]
fn run_corpus_fails_when_the_recorded_stdout_no_longer_matches() {
    let directory = run_corpus_fixture(
        "run-corpus-drift",
        "[[entry]]\nfile = \"Talkative.ns\"\nsend = \"main:args:\"\n\
         expected_status = 0\nexpected_stdout = \"something else\"\nreason = \"fixture\"\n",
    );
    let modules = directory.join("modules");
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ran 1, failed 1"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FAIL Talkative.ns: source: expected stdout"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn run_corpus_fails_when_a_whitelisted_module_no_longer_runs() {
    let directory = run_corpus_fixture(
        "run-corpus-regressed",
        "[[entry]]\nfile = \"Talkative.ns\"\nsend = \"noSuchSelector\"\n\
         expected_status = 0\nexpected_stdout = \"\"\nreason = \"fixture\"\n",
    );
    let modules = directory.join("modules");
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("expected exit 0"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An empty whitelist must report zero rather than pass silently: `ran 0`
/// is the number a reader has to be able to see, so that a whitelist that
/// quietly lost its entries cannot masquerade as a green gate.
#[test]
fn run_corpus_with_an_empty_whitelist_reports_running_nothing() {
    let directory = run_corpus_fixture("run-corpus-empty", "# no entries\n");
    let modules = directory.join("modules");
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ran 0, failed 0, skipped 1"), "{stdout}");
}

#[test]
fn run_corpus_malformed_whitelist_fails_with_error() {
    let directory = run_corpus_fixture("run-corpus-malformed", "this is not toml :::\n[[entry\n");
    let modules = directory.join("modules");
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("whitelist"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn bench_reports_the_entry_sends_median() {
    let out = ns_next()
        .arg("bench")
        .arg(repository_root().join("tests-fixtures/while_loop.ns"))
        .arg("--iterations")
        .arg("3")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("median"), "{stdout}");
}

/// `bench` must reject a zero iteration count rather than dividing by it
/// (the median computation indexes `timings[timings.len() / 2]`, which is
/// only safe because `iterations >= 1` is enforced up front).
#[test]
fn bench_rejects_zero_iterations() {
    let out = ns_next()
        .arg("bench")
        .arg(repository_root().join("tests-fixtures/while_loop.ns"))
        .arg("--iterations")
        .arg("0")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("iterations"));
}
