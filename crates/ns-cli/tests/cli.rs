mod common;

use common::ScratchDirectory;
use indoc::indoc;
use std::process::Command;

fn ns_next() -> Command {
    common::target_command(env!("CARGO_BIN_EXE_ns-next"))
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

/// `ns-next` with a debug build's simulation of a system whose code zone
/// cannot be reserved, as on Windows before 10 version 1803, the JIT
/// wanted: `NS_NEXT_NO_JIT` cleared, whatever the environment says, and
/// the stress modes that want the JIT whatever it says, so that a test
/// that sets it finds the JIT unwanted.
#[cfg(debug_assertions)]
fn ns_next_without_a_code_zone() -> Command {
    let mut command = ns_next();
    command
        .env("NS_NEXT_SIMULATE_UNSUPPORTED_ZONE", "1")
        .env_remove("NS_NEXT_NO_JIT")
        .env_remove("NS_NEXT_STRESS_JIT")
        .env_remove("NS_NEXT_STRESS_EXIT")
        .env_remove("NS_NEXT_STRESS_ZONE")
        .env_remove("NS_NEXT_STRESS_OPTIMIZE")
        .env_remove("NS_NEXT_STRESS_DEOPT")
        .env_remove("NS_NEXT_STRESS_INVALIDATE");
    command
}

/// A JIT that was wanted and is off says so, once, on stderr, naming the
/// requirement, and the program still runs right, interpreted; a JIT
/// nothing asked for (`NS_NEXT_NO_JIT=1`) says nothing.
#[cfg(debug_assertions)]
#[test]
fn run_says_when_the_jit_is_off_and_why() {
    let golden = common::golden("language/control-flow/while_loop");
    let out = ns_next_without_a_code_zone()
        .arg("run")
        .arg(&golden)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr was: {stderr}");
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("the JIT is off"))
        .collect();
    assert_eq!(warnings.len(), 1, "{stderr}");
    assert!(warnings[0].contains("Windows 10 version 1803"), "{stderr}");
    let plain = ns_next().arg("run").arg(&golden).output().unwrap();
    assert_eq!(out.stdout, plain.stdout, "the interpreter answers the same");
    let unwanted = ns_next_without_a_code_zone()
        .env("NS_NEXT_NO_JIT", "1")
        .arg("run")
        .arg(&golden)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&unwanted.stderr);
    assert!(!stderr.contains("the JIT is off"), "{stderr}");
}

/// `run-corpus` says once that the JIT is off, and takes the line off its
/// children's stderr, which a module's recorded outcome never holds: the
/// corpus still runs clean.
#[cfg(debug_assertions)]
#[test]
fn run_corpus_says_once_when_the_jit_is_off() {
    let out = ns_next_without_a_code_zone()
        .arg("run-corpus")
        .arg(repository_root().join("corpus/modules"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr was: {stderr}");
    let warnings = stderr
        .lines()
        .filter(|line| line.contains("the JIT is off"))
        .count();
    assert_eq!(warnings, 1, "{stderr}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("failed 0"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A benchmark refuses to time the interpreter in the JIT's place.
#[cfg(debug_assertions)]
#[test]
fn bench_refuses_when_the_jit_is_off() {
    let out = ns_next_without_a_code_zone()
        .arg("bench")
        .arg(common::golden("language/control-flow/while_loop"))
        .arg("--iterations")
        .arg("1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr was: {stderr}");
    assert!(
        stderr.contains("the JIT is off, and a benchmark would time the interpreter"),
        "{stderr}"
    );
    assert!(stderr.contains("Windows 10 version 1803"), "{stderr}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("median"));
}

#[test]
fn benchmark_takes_a_project_file() {
    let out = ns_next()
        .arg("bench")
        .arg(
            common::goldens_root()
                .join("tool")
                .join("manifest")
                .join("a_project_reaches_its_modules_through_the_manifest"),
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

#[test]
fn benchmark_runs_a_script_of_the_project() {
    let directory = ScratchDirectory::new("bench-script");
    std::fs::write(
        directory.join("Echo.ns"),
        indoc! {"
            class Echo packageUsing: manifest = Object new () (
                public main: platform args: arguments = ()
            ) : ()
        "},
    )
    .unwrap();
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "Echo.ns", "scripts": { "two": "first second" } }"#,
    )
    .unwrap();
    let out = ns_next()
        .args(["bench", "--script", "two"])
        .arg(directory.path())
        .args(["--iterations", "1"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("median"));
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

/// `--jit-statistics` prints one `jit: ` line to stderr after the run, the
/// baseline's counts and the optimizing tier's, and leaves stdout as it was.
#[test]
fn jit_statistics_print_one_line_to_stderr() {
    let program = common::golden("language/control-flow/while_loop");
    let plain = ns_next().arg("run").arg(&program).output().unwrap();
    let counted = ns_next()
        .arg("run")
        .arg(&program)
        .arg("--jit-statistics")
        .output()
        .unwrap();
    assert_eq!(counted.status.code(), plain.status.code());
    assert_eq!(counted.stdout, plain.stdout);
    let stderr = String::from_utf8_lossy(&counted.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("jit: "))
        .collect();
    assert_eq!(lines.len(), 1, "stderr: {stderr}");
    // The words are fixed, only the numbers vary.
    let mut shape = String::new();
    let mut in_digits = false;
    for character in lines[0].chars() {
        if character.is_ascii_digit() || (in_digits && character == '.') {
            if !in_digits {
                shape.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            shape.push(character);
        }
    }
    assert_eq!(
        shape,
        "jit: # bodies, # twins compiled in # ms; # back-edge switches, \
         # resumptions after a safepoint, # exits; optimizer: # requests, \
         # bodies optimized (# customized copies) in # ms, \
         # optimized exits (# forced, # traps), # abandoned, \
         # made not entrant, # refused for good, # invalidated copies, \
         # dispatched calls; inlining: # sites inlined, # scopes inlined, \
         # type-case arms, \
         # blocks eliminated, # blocks materialized, \
         # inlined frames materialized, # budget exits \
         (# recompilees above), # poll re-entries, \
         # poll deoptimizations, # polls without an entry"
    );
}

/// The `jit: ` line's counts before each of `labels`, of a run of
/// `program` with `--jit-statistics` and with neither a stress mode nor
/// `NS_NEXT_NO_JIT`, whatever the environment says, so that what the
/// optimizer inlines is what a plain run's budgets decide; `None` on a
/// target without a JIT, which compiles no body.
fn plain_jit_counts(program: &std::path::Path, stdout: &str, labels: &[&str]) -> Option<Vec<u64>> {
    let mut command = ns_next();
    command
        .arg("run")
        .arg(program)
        .arg("--jit-statistics")
        .env_remove("NS_NEXT_NO_JIT");
    for mode in [
        "MOVES",
        "COLLECT",
        "SCAVENGE",
        "SUSPEND",
        "ZONE",
        "JIT",
        "EXIT",
        "OPTIMIZE",
        "DEOPT",
        "CACHES",
        "INVALIDATE",
        "WIDE",
    ] {
        command.env_remove(format!("NS_NEXT_STRESS_{mode}"));
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stdout);
    let line = stderr
        .lines()
        .find(|line| line.starts_with("jit: "))
        .unwrap_or_else(|| panic!("no `jit: ` line in {stderr}"));
    let count_before = |label: &str| -> u64 {
        let position = line
            .find(&format!(" {label}"))
            .unwrap_or_else(|| panic!("no `{label}` in {line}"));
        let digits: String = line[..position]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        digits.parse().unwrap()
    };
    if count_before("bodies,") == 0 {
        return None;
    }
    Some(labels.iter().map(|label| count_before(label)).collect())
}

/// `sumOf:times:`'s block sends `sumOf:`, whose `to:do:` and block its
/// copy inlines, the block eliminated; the Floats of the second run fail
/// the addition's speculation inside the inlined block, where the block
/// is live in `to:do:`'s scope, so that the exit writes the inlined
/// frames and makes the block.
const AN_ELIMINATED_BLOCK_MADE_AT_A_TRAP: &str = indoc! {"
    class Subject = Object new () (
      sumOf: values = (
        |
          total ::= 0.
        |
        1 to: values size do: [:index | total:: total + (values at: index)].
        ^total.
      )

      sumOf: values times: count = (
        |
          sum ::= 0.
        |
        1 to: count do: [:round | sum:: sum + (sumOf: values)].
        ^sum.
      )

      public main: platform args: arguments = (
        platform transcript
            show: (sumOf: {1. 2. 3} times: 30000) printString;
            newLine.
        platform transcript
            show: (sumOf: {1.5. 2.5} times: 30) printString;
            newLine.
      )
    )
"};

/// `--jit-statistics` counts what the optimized copies inlined and what
/// became of it: a copy whose block is eliminated inside inlined scopes,
/// and made by the exit at a trap there, counts the sites and the scopes
/// inlined, the type-case arms, the block eliminated and made, the
/// inlined frames the exit wrote and the budget exits whose walk found
/// the recompilee above, and answers the interpreter's sums; and a loop
/// whose inlined callees allocate counts the polls it went back into its
/// code at, after each scavenge.
#[test]
fn jit_statistics_count_what_inlining_did() {
    let directory = ScratchDirectory::new("jit-statistics-inlining");
    let program = directory.join("Subject.ns");
    std::fs::write(&program, AN_ELIMINATED_BLOCK_MADE_AT_A_TRAP).unwrap();
    let Some(counts) = plain_jit_counts(
        &program,
        "180000
120.0
",
        &[
            "sites inlined",
            "scopes inlined",
            "type-case arms",
            "blocks eliminated",
            "blocks materialized",
            "inlined frames materialized",
            "budget exits",
            "recompilees above",
        ],
    ) else {
        return;
    };
    assert!(counts.iter().all(|count| *count > 0), "{counts:?}");
    let reentries = plain_jit_counts(
        &common::golden("runtime/optimizer/allocating_inlined_loop"),
        "40000400000
40
8200040
",
        &["poll re-entries", "polls without an entry"],
    )
    .expect("a target with a JIT");
    assert!(reentries[0] > 0, "{reentries:?}");
    assert_eq!(reentries[1], 0, "{reentries:?}");
}

/// Two optimizer goldens reach in a plain run the path they are written
/// for: `an_edit_below_an_inlined_frame`'s edits each free the copy whose
/// frame is below, and `eliminated_block_seen_after_an_exit`'s traps make
/// the blocks its copies eliminated.
#[test]
fn the_optimizer_goldens_reach_their_paths_in_a_plain_run() {
    let Some(edits) = plain_jit_counts(
        &common::golden("runtime/optimizer/an_edit_below_an_inlined_frame"),
        "420000
14000
420000
nil
9
19005
24000
",
        &["invalidated copies"],
    ) else {
        return;
    };
    assert_eq!(edits, vec![2], "a copy freed at each edit");
    let made = plain_jit_counts(
        &common::golden("runtime/optimizer/eliminated_block_seen_after_an_exit"),
        "420000
16.0
21
90000
4
",
        &["blocks eliminated", "blocks materialized", "traps"],
    )
    .expect("a target with a JIT");
    assert!(made.iter().all(|count| *count > 0), "{made:?}");
}

/// `--collector-statistics` prints the collector's counts and pauses to
/// stderr after the timed runs, exactly as `--cache-statistics` prints the
/// inline caches' sizes there. This is the one collector test that can run
/// against a release build -- every other one is `#[cfg(debug_assertions)]`
/// -- so it is also the only one to check that the line it prints reports a
/// scavenge actually happening rather than merely being present:
/// `bench_block.ns` allocates a closure every iteration and scavenges even
/// at `--iterations 1` (2 scavenges, checked by hand against this
/// binary), so "collector: 0 scavenges" here would mean the trigger, not
/// just the reporting, had stopped working.
#[test]
fn benchmark_collector_statistics_reports_to_stderr() {
    let out = ns_next()
        .arg("bench")
        .arg(common::repository_root().join("benchmarks/micro/bench_block.ns"))
        .arg("--iterations")
        .arg("1")
        .arg("--collector-statistics")
        // The trigger under test is the plain run's; a stress mode collects or moves on its own schedule.
        .env_remove("NS_NEXT_STRESS_MOVES")
        .env_remove("NS_NEXT_STRESS_CACHES")
        .env_remove("NS_NEXT_STRESS_COLLECT")
        .env_remove("NS_NEXT_STRESS_SCAVENGE")
        .env_remove("NS_NEXT_STRESS_WIDE")
        .env_remove("NS_NEXT_STRESS_SUSPEND")
        .env_remove("NS_NEXT_STRESS_JIT")
        .env_remove("NS_NEXT_STRESS_EXIT")
        .env_remove("NS_NEXT_STRESS_ZONE")
        .env_remove("NS_NEXT_STRESS_OPTIMIZE")
        .env_remove("NS_NEXT_STRESS_DEOPT")
        .env_remove("NS_NEXT_STRESS_INVALIDATE")
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
    assert!(stderr.contains("scavenges"), "{stderr}");
    assert!(
        !stderr.contains("collector: 0 scavenges"),
        "bench_block.ns scavenges even at one iteration; a report of zero means the \
         trigger did not fire: {stderr}"
    );
}
