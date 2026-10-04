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

/// A module whose entry method writes to the transcript, so a synthetic
/// `run-corpus` whitelist can exercise `expected_stdout` against real
/// output -- the vendored corpus modules that run today print nothing.
const TALKATIVE: &str = indoc! {"
    class Talkative usingPlatform: p = Object new (
        | private platform = p. |
    ) (
        public talk = (
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
        "[[entry]]\nfile = \"Talkative.ns\"\nsend = \"talk\"\n\
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
        "[[entry]]\nfile = \"Talkative.ns\"\nsend = \"talk\"\n\
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
    // The lane must fail because the module does not understand the
    // selector, not because the wrapper could not be built or started.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("noSuchSelector"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn run_corpus_instantiates_a_new_module_with_new() {
    let directory = run_corpus_fixture(
        "run-corpus-new",
        "[[entry]]\nfile = \"Quiet.ns\"\nsend = \"answer\"\n\
         expected_status = 0\nexpected_stdout = \"\"\nreason = \"fixture\"\n",
    );
    let modules = directory.join("modules");
    std::fs::write(
        modules.join("Quiet.ns"),
        indoc! {"
            class Quiet = Object new (
            ) (
                public answer = ( ^42 )
            ) : ()
        "},
    )
    .unwrap();
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("ran 1, failed 0, skipped 1"), "{stdout}");
}

#[test]
fn run_corpus_refuses_a_module_whose_factory_it_cannot_fill() {
    let directory = run_corpus_fixture(
        "run-corpus-picky",
        "[[entry]]\nfile = \"Picky.ns\"\nsend = \"answer\"\n\
         expected_status = 0\nexpected_stdout = \"\"\nreason = \"fixture\"\n",
    );
    let modules = directory.join("modules");
    std::fs::write(
        modules.join("Picky.ns"),
        indoc! {"
            class Picky usingPlatform: p ide: i = Object new (
            ) (
                public answer = ( ^42 )
            ) : ()
        "},
    )
    .unwrap();
    let out = ns_next().arg("run-corpus").arg(&modules).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("FAIL Picky.ns:"), "stderr: {stderr}");
    assert!(stderr.contains("usingPlatform:ide:"), "stderr: {stderr}");
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
fn benchmark_reports_the_entry_sends_median() {
    let out = ns_next()
        .arg("bench")
        .arg(common::golden("language/control-flow/while_loop"))
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
    assert!(stdout.contains("timing: `main:args:` only"), "{stdout}");
}

#[test]
fn benchmark_takes_a_project_file() {
    let out = ns_next()
        .arg("bench")
        .arg(
            common::goldens_root()
                .join("tool")
                .join("manifest")
                .join("a_project_reaches_its_modules_through_the_manifest.json"),
        )
        .arg("--iterations")
        .arg("2")
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

/// A program `bench` cannot instantiate is reported once, by the refusal
/// that names why, with no second, generic line after it.
#[test]
fn benchmark_reports_a_failed_instantiation_once() {
    let directory = ScratchDirectory::new("bench-instantiation");
    let file = directory.join("Keyed.ns");
    std::fs::write(
        &file,
        indoc! {"
            class Keyed withValue: value = Object new () (
              public main: platform args: arguments = ( ^0 )
            ) : ()
        "},
    )
    .unwrap();
    let out = ns_next()
        .arg("bench")
        .arg(&file)
        .arg("--iterations")
        .arg("1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0));
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("error:"))
        .collect();
    assert_eq!(errors.len(), 1, "stderr: {stderr}");
    assert!(
        errors[0].contains("its primary factory is `withValue:`"),
        "stderr: {stderr}"
    );
}

/// `bench` must reject a zero iteration count rather than dividing by it
/// (the median computation indexes `timings[timings.len() / 2]`, which is
/// only safe because `iterations >= 1` is enforced up front).
#[test]
fn benchmark_rejects_zero_iterations() {
    let out = ns_next()
        .arg("bench")
        .arg(common::golden("language/control-flow/while_loop"))
        .arg("--iterations")
        .arg("0")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("iterations"));
}

/// `--collector-statistics` prints the collector's counts and pauses to
/// stderr after the timed runs, exactly as `--cache-statistics` prints the
/// inline caches' sizes there. This is the one collector test that can run
/// against a release build -- every other one is `#[cfg(debug_assertions)]`
/// -- so it is also the only one to check that the line it prints reports a
/// collection actually happening rather than merely being present:
/// `bench_block.ns` allocates a closure every iteration and collects even
/// at `--iterations 1` (49 collections, checked by hand against this
/// binary), so "collector: 0 collections" here would mean the trigger, not
/// just the reporting, had stopped working.
#[test]
fn benchmark_collector_statistics_reports_to_stderr() {
    let out = ns_next()
        .arg("bench")
        .arg(common::golden("tool/bench/bench_block"))
        .arg("--iterations")
        .arg("1")
        .arg("--collector-statistics")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("collector:"), "{stderr}");
    assert!(
        !stderr.contains("collector: 0 collections"),
        "bench_block.ns collects every iteration; a report of zero means the \
         trigger did not fire: {stderr}"
    );
}
