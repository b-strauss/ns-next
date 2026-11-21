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

/// A `run` of `program` with neither a stress mode nor `NS_NEXT_NO_JIT`,
/// whatever the environment says, so that what the tiers do is what a
/// plain run decides.
fn plain_run(program: &std::path::Path) -> Command {
    let mut command = ns_next();
    command.arg("run").arg(program).env_remove("NS_NEXT_NO_JIT");
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
        "PAUSE",
        "BREAKPOINTS",
        "STEP",
    ] {
        command.env_remove(format!("NS_NEXT_STRESS_{mode}"));
    }
    command
}

/// The `jit: ` line's counts before each of `labels`, of a run of
/// `program` with `--jit-statistics` and with neither a stress mode nor
/// `NS_NEXT_NO_JIT`, whatever the environment says, so that what the
/// optimizer inlines is what a plain run's budgets decide; `None` on a
/// target without a JIT, which compiles no body.
fn plain_jit_counts(program: &std::path::Path, stdout: &str, labels: &[&str]) -> Option<Vec<u64>> {
    let mut command = plain_run(program);
    command.arg("--jit-statistics");
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

/// Four optimizer goldens reach in a plain run the path they are written
/// for: `an_edit_below_an_inlined_frame`'s edits each free the copy whose
/// frame is below, `eliminated_block_seen_after_an_exit`'s traps make
/// the blocks its copies eliminated,
/// `a_block_made_on_a_way_not_run_traps`'s way that starts running traps
/// at its `MakeClosure`, the trap that follows there retiring the copy,
/// which is compiled again, and
/// `a_block_handed_to_an_array_s_do_is_eliminated`'s copies eliminate the
/// blocks they hand an Array's `do:`, which traps on no way.
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
    let retired = plain_jit_counts(
        &common::golden("runtime/optimizer/a_block_made_on_a_way_not_run_traps"),
        "15015000
15015000
22515000
",
        &["traps", "made not entrant", "bodies optimized"],
    )
    .expect("a target with a JIT");
    assert!(retired[0] > 0, "the way traps once it runs: {retired:?}");
    assert!(retired[1] > 0, "the trap retires the copy: {retired:?}");
    assert!(
        retired[2] > 1,
        "and the body is compiled again: {retired:?}"
    );
    let handed = plain_jit_counts(
        &common::golden("runtime/optimizer/a_block_handed_to_an_array_s_do_is_eliminated"),
        "2525000
5000
",
        &["blocks eliminated", "traps"],
    )
    .expect("a target with a JIT");
    assert!(handed[0] > 0, "the blocks are eliminated: {handed:?}");
    assert_eq!(handed[1], 0, "and no way traps: {handed:?}");
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
        .env_remove("NS_NEXT_STRESS_PAUSE")
        .env_remove("NS_NEXT_STRESS_BREAKPOINTS")
        .env_remove("NS_NEXT_STRESS_STEP")
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

/// `square:` is sent a hundred times, so that the baseline JIT compiles
/// it at its second entry.
const A_HOT_METHOD: &str = indoc! {"
    class Demo = Object new () (
      square: number = (
        ^number * number.
      )

      public main: platform args: arguments = (
        |
          total ::= 0.
        |
        1 to: 100 do: [:index | total:: total + (square: index)].
        platform transcript show: total printString; newLine.
      )
    )
"};

/// What `A_HOT_METHOD` prints.
const A_HOT_METHOD_STDOUT: &str = "338350\n";

/// `A_HOT_METHOD` written as `Demo.ns` in a scratch directory, and the
/// file's path; `None` on a target without a JIT, which compiles no body
/// and so traces nothing of what these tests read.
fn hot_method_program(name: &str) -> Option<(ScratchDirectory, std::path::PathBuf)> {
    let directory = ScratchDirectory::new(name);
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_HOT_METHOD).unwrap();
    plain_jit_counts(&program, A_HOT_METHOD_STDOUT, &[])?;
    Some((directory, program))
}

/// A plain run of `program` with `arguments` before it, its stdout held to
/// `A_HOT_METHOD_STDOUT`; answers its stderr.
fn traced_stderr(program: &std::path::Path, arguments: &[&str]) -> String {
    let output = plain_run(program).args(arguments).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), A_HOT_METHOD_STDOUT);
    stderr
}

/// `--trace=compile` writes a line for the method the baseline JIT
/// compiled, and leaves stdout as it was.
#[test]
fn trace_compile_names_the_hot_method() {
    let Some((_directory, program)) = hot_method_program("trace-compile") else {
        return;
    };
    let stderr = traced_stderr(&program, &["--trace=compile"]);
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("[compile] {Demo>>square:} compiled trigger=")),
        "stderr: {stderr}"
    );
    assert!(
        stderr.lines().all(|line| line.starts_with("[compile] ")),
        "stderr: {stderr}"
    );
}

/// Every trace line is a whole line beside the other lines the run writes
/// on stderr, here `--jit-statistics`'s.
#[test]
fn trace_lines_are_whole_lines_beside_the_programs_stderr() {
    let Some((_directory, program)) = hot_method_program("trace-whole-lines") else {
        return;
    };
    let stderr = traced_stderr(&program, &["--trace=all", "--jit-statistics"]);
    assert!(
        stderr
            .lines()
            .all(|line| line.starts_with('[') || line.starts_with("jit: ")),
        "stderr: {stderr}"
    );
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("jit: "))
            .count(),
        1,
        "stderr: {stderr}"
    );
    assert!(
        stderr.lines().any(|line| line.starts_with("[caches] ")),
        "stderr: {stderr}"
    );
}

/// `--trace-filter` delivers only the events of the bodies it selects,
/// named whole or by a prefix ending in `...`.
#[test]
fn the_filter_narrows_the_lines() {
    let Some((_directory, program)) = hot_method_program("trace-filter") else {
        return;
    };
    for pattern in ["--trace-filter=Demo>>square:", "--trace-filter=Demo>>sq..."] {
        let stderr = traced_stderr(&program, &["--trace=compile", pattern]);
        let compiles: Vec<&str> = stderr
            .lines()
            .filter(|line| line.starts_with("[compile] "))
            .collect();
        assert!(!compiles.is_empty(), "{pattern}: {stderr}");
        assert!(
            compiles.iter().all(|line| {
                line.starts_with("[compile] {Demo>>square:} ")
                    || line.starts_with("[compile] {[] in Demo>>square:} ")
            }),
            "{pattern}: {stderr}"
        );
    }
}

/// `compare`'s loop compares a string with `==`, whose leaf its copy
/// does not express, after a nested class's accessor and factory ran,
/// neither of them snapshotted: three callees that are no body.
const A_COMPARING_LOOP: &str = indoc! {"
    class Demo = Object new () (
      class Account = () ()

      compare = (
        |
          account = Account new.
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000000] whileTrue: [
          ('a' == account) ifTrue: [
            sum:: sum + 1].
          index:: index + 1.
        ].
        ^sum.
      )

      public main: platform args: arguments = (
        platform transcript show: compare printString; newLine.
      )
    )
"};

/// `--trace=inlining` folds the declines of callees that are no body
/// into one line per copy, counting them by reason, and writes no
/// decline that names no callee.
#[test]
fn trace_inlining_folds_the_declines_of_no_body() {
    let directory = ScratchDirectory::new("trace-inlining-folded");
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_COMPARING_LOOP).unwrap();
    if plain_jit_counts(&program, "0\n", &[]).is_none() {
        return;
    }
    let output = plain_run(&program)
        .arg("--trace=inlining")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0\n");
    let folded: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("[inlining] {Demo>>compare} declined-no-body "))
        .collect();
    assert!(!folded.is_empty(), "stderr: {stderr}");
    assert!(
        folded
            .iter()
            .all(|line| line.ends_with(" count=3 reasons=not-snapshotted:2,not-expressed:1")),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.lines().any(|line| line.contains(" callee={} ")),
        "stderr: {stderr}"
    );
}

/// A run of `A_HOT_METHOD` with `arguments`, which are refused before the
/// program runs: status 2, nothing on stdout, and the stderr answered.
fn refused_stderr(name: &str, arguments: &[&str]) -> String {
    let directory = ScratchDirectory::new(name);
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_HOT_METHOD).unwrap();
    let output = plain_run(&program).args(arguments).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "stderr: {stderr}"
    );
    stderr
}

#[test]
fn an_unknown_category_is_refused() {
    let stderr = refused_stderr("trace-unknown-category", &["--trace=compile,compil"]);
    assert!(stderr.contains("`compil`"), "stderr: {stderr}");
}

#[test]
fn a_malformed_filter_is_refused() {
    let stderr = refused_stderr(
        "trace-malformed-filter",
        &[
            "--trace=compile",
            "--trace-filter=Demo>>square:",
            "--trace-filter=Demo>>",
        ],
    );
    assert!(stderr.contains("`Demo>>`"), "stderr: {stderr}");
}

/// A pattern naming a module the program does not have is refused once
/// the program is loaded, before `main:args:` runs.
#[test]
fn a_filter_naming_no_module_is_refused() {
    let stderr = refused_stderr(
        "trace-unknown-module",
        &[
            "--trace=compile",
            "--trace-filter=Demo>>square:",
            "--trace-filter=Other>>run",
        ],
    );
    assert!(stderr.contains("`Other>>run`"), "stderr: {stderr}");
    assert!(stderr.contains("no module `Other`"), "stderr: {stderr}");
}

/// `--print-bytecode` prints without any trace category: the bodies its
/// filter selects, listed when the program's module is made.
#[test]
fn print_bytecode_alone_prints() {
    let directory = ScratchDirectory::new("print-bytecode");
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_HOT_METHOD).unwrap();
    let stderr = traced_stderr(&program, &["--print-bytecode", "--print-filter=Demo..."]);
    assert!(
        stderr
            .lines()
            .any(|line| line == "--- bytecode {Demo>>square:}"),
        "stderr: {stderr}"
    );
    assert!(
        stderr
            .lines()
            .filter(|line| line.starts_with("--- bytecode "))
            .all(|line| line.starts_with("--- bytecode {Demo")),
        "stderr: {stderr}"
    );
}

/// The listings of `stderr`: each from its head line to its `--- end`,
/// the lines between, whatever else `stderr` holds outside them.
fn listings(stderr: &str) -> Vec<Vec<&str>> {
    let mut listings = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in stderr.lines() {
        match current.as_mut() {
            Some(lines) => {
                lines.push(line);
                if line == "--- end" {
                    listings.extend(current.take());
                }
            }
            None if line.starts_with("--- ") => current = Some(vec![line]),
            None => {}
        }
    }
    assert!(current.is_none(), "a listing without its end: {stderr}");
    listings
}

/// Whether `line` is an instruction's or a data word's line of a code
/// listing: four spaces, its offset -- an instruction's in the body as
/// `+0x`, a data word's from the first as `data+0x` -- in four hex
/// digits, two spaces.
fn is_offset_line(line: &str) -> bool {
    let offset = line
        .strip_prefix("    +0x")
        .or_else(|| line.strip_prefix("    data+0x"));
    offset.is_some_and(|rest| {
        rest.len() > 6
            && rest[..4]
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit))
            && rest[4..].starts_with("  ")
    })
}

/// `--print-code` prints the hot method's baseline code as it is
/// installed, every instruction at its offset in the body.
#[test]
fn print_code_prints_the_hot_method() {
    let Some((_directory, program)) = hot_method_program("print-code") else {
        return;
    };
    let stderr = traced_stderr(&program, &["--print-code", "--print-filter=Demo>>square:"]);
    let listings = listings(&stderr);
    let square = listings
        .iter()
        .find(|lines| lines[0].starts_with("--- code {Demo>>square:} baseline "))
        .unwrap_or_else(|| panic!("no baseline listing of square: in {stderr}"));
    assert!(
        square[0].contains(" base=0x") && square[0].contains(" bytes="),
        "{stderr}"
    );
    let offsets: Vec<&&str> = square
        .iter()
        .filter(|line| line.starts_with("    "))
        .collect();
    assert!(!offsets.is_empty(), "{stderr}");
    assert!(offsets.iter().all(|line| is_offset_line(line)), "{stderr}");
    assert!(
        listings
            .iter()
            .all(|lines| lines[0].starts_with("--- code {Demo>>square:} ")),
        "{stderr}"
    );
}

/// A listing is written whole beside the other lines the run writes on
/// stderr -- the trace's, the statistics' -- never with one inside it.
#[test]
fn a_listing_is_one_write_beside_the_programs_stderr() {
    let Some((_directory, program)) = hot_method_program("print-whole") else {
        return;
    };
    let stderr = traced_stderr(
        &program,
        &[
            "--trace=all",
            "--jit-statistics",
            "--print-bytecode",
            "--print-code",
            "--print-inlining",
            "--print-filter=Demo...",
        ],
    );
    let listings = listings(&stderr);
    assert!(
        listings
            .iter()
            .any(|lines| lines[0].starts_with("--- code {Demo>>square:} ")),
        "{stderr}"
    );
    for lines in &listings {
        assert!(
            lines[1..].iter().all(|line| !line.starts_with('[')
                && !line.starts_with("jit: ")
                && !line.starts_with("--- ")
                || *line == "--- end"),
            "{stderr}"
        );
    }
    assert!(
        stderr.lines().any(|line| line.starts_with("[compile] ")),
        "{stderr}"
    );
}

/// A print filter naming a module the program does not have is refused
/// before the program runs, as a trace filter is.
#[test]
fn a_print_filter_naming_no_module_is_refused() {
    let stderr = refused_stderr(
        "print-unknown-module",
        &["--print-code", "--print-filter=Other>>run"],
    );
    assert!(stderr.contains("--print-filter: "), "stderr: {stderr}");
    assert!(stderr.contains("no module `Other`"), "stderr: {stderr}");
}

/// `--print-filter` selects what a print option prints, so it is refused
/// without one.
#[test]
fn a_print_filter_needs_a_print_option() {
    let stderr = refused_stderr("print-filter-alone", &["--print-filter=Demo>>square:"]);
    assert!(stderr.contains("--print-filter"), "stderr: {stderr}");
}

/// Every body number `record` names, at any depth: a record's `body`, an
/// event's `in-body`, `callee` and `spent-by`, a listing's bodies and
/// scopes and a decline's callee.
fn body_numbers(record: &serde_json::Value, numbers: &mut Vec<u64>) {
    match record {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                match (name.as_str(), value.as_u64()) {
                    ("body" | "in-body" | "callee" | "spent-by", Some(number)) => {
                        numbers.push(number);
                    }
                    _ => body_numbers(value, numbers),
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                body_numbers(value, numbers);
            }
        }
        _ => {}
    }
}

/// `--trace-file` writes the trace and the listings into the file, not on
/// stderr: a header first, then every body described before any record
/// names its number.
#[test]
fn trace_file_holds_header_bodies_and_events() {
    let Some((directory, program)) = hot_method_program("trace-file") else {
        return;
    };
    let file = directory.join("trace.jsonl");
    let stderr = traced_stderr(
        &program,
        &[
            "--trace=compile",
            "--print-bytecode",
            "--print-filter=Demo...",
            &format!("--trace-file={}", file.display()),
        ],
    );
    assert!(!stderr.contains("[compile]"), "stderr: {stderr}");
    assert!(!stderr.contains("--- bytecode"), "stderr: {stderr}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.ends_with('\n'), "{text}");
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}")))
        .collect();
    assert_eq!(records[0]["kind"], "header", "{text}");
    assert_eq!(records[0]["format"], "ns-next-trace", "{text}");
    assert_eq!(records[0]["version"], 1, "{text}");
    let mut described = std::collections::BTreeMap::new();
    let mut compiled_square = false;
    let mut listed_demo = false;
    for record in &records[1..] {
        if record["kind"] == "body" {
            let number = record["body"].as_u64().unwrap();
            assert!(
                described.insert(number, record.clone()).is_none(),
                "described twice: {record}"
            );
            continue;
        }
        let mut numbers = Vec::new();
        body_numbers(record, &mut numbers);
        for number in numbers {
            assert!(
                described.contains_key(&number),
                "body {number} named before it is described: {record}"
            );
        }
        let name_of = |key: &str| {
            record[key]
                .as_u64()
                .and_then(|number| described.get(&number))
                .map(|body| body["name"].clone())
        };
        if record["kind"] == "event" && record["event"] == "compiled" {
            compiled_square |= name_of("body").is_some_and(|name| name == "Demo>>square:");
        }
        if record["kind"] == "bytecode" {
            let name = name_of("body").unwrap();
            assert!(name.as_str().unwrap().starts_with("Demo"), "{record}");
            listed_demo = true;
        }
    }
    assert!(compiled_square, "{text}");
    assert!(listed_demo, "{text}");
    let square = described
        .values()
        .find(|body| body["name"] == "Demo>>square:")
        .unwrap();
    assert_eq!(
        square["source"]["file"],
        std::path::absolute(&program).unwrap().display().to_string(),
        "{square}"
    );
    assert_eq!(square["range"][0], 1, "{square}");
    assert_eq!(square["placed"], "source", "{square}");
}

/// A program that replaces `maker` with a version a line shorter, preceded
/// by an added method, while a closure the old version made runs on, and
/// then first runs `later:`, which the edit moved down in the module's
/// text and left as the file has it.
const AN_EDITED_MAKER: &str = indoc! {"
    class Demo = Object new () (
      class Subject = Object new () (
        public maker = (
          ^[:index |
            index max: 1].
        )
      )

      public main: platform args: arguments = (
        |
          ClassMirror = platform mirrors ClassMirror.
          subject = Subject new.
          closure = subject maker.
          builder = (ClassMirror reflecting: Subject) mixin declaration asBuilder.
          sum ::= 0.
        |
        builder instanceSide methods
            addFromSource: 'public helper = ( ^0 )';
            addFromSource: 'public maker = ( ^[:index | index max: 2] )'.
        builder install.
        1 to: 1000 do: [:index |
          sum:: sum + (closure value: index) + (subject maker value: index)].
        platform transcript show: (sum + (later: 1000)) printString; newLine.
      )

      later: count = (
        | total ::= 0. |
        1 to: count do: [:index | total:: total + (index max: 3)].
        ^total.
      )
    )
"};

/// The FNV-1a 64 of `text` with every `\r` left out, as a `body` record's
/// checksum writes it.
fn checksum_of(text: &str) -> String {
    let hash = text
        .bytes()
        .filter(|byte| *byte != b'\r')
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{hash:016x}")
}

/// Where `part` starts in `AN_EDITED_MAKER`, as a range's line and
/// column count it.
fn edited_maker_position(part: &str) -> (usize, usize) {
    position_in(AN_EDITED_MAKER, part)
}

/// Where `part` starts in `text`, as a range's line and column count it.
fn position_in(text: &str, part: &str) -> (usize, usize) {
    let offset = text.find(part).unwrap();
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let column = offset - before.rfind('\n').map_or(0, |end| end + 1);
    (line, column)
}

/// The `body` records of a traced run of `source`, which prints
/// `stdout`, with `--trace=compile,caches`.
fn described_bodies(name: &str, source: &str, stdout: &str) -> Vec<serde_json::Value> {
    let directory = ScratchDirectory::new(name);
    let program = directory.join("Demo.ns");
    std::fs::write(&program, source).unwrap();
    let file = directory.join("trace.jsonl");
    let output = plain_run(&program)
        .args([
            "--trace=compile,caches".to_string(),
            format!("--trace-file={}", file.display()),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stdout);
    std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["kind"] == "body")
        .collect()
}

/// A program whose `maker` an edit replaces with one that differs only
/// in its comment, while a closure the old version made runs on.
const A_RECOMMENTED_MAKER: &str = indoc! {"
    class Demo = Object new () (
      class Subject = Object new () (
        public maker = (
          (* the first *)
          ^[:index | index max: 1].
        )
      )

      public main: platform args: arguments = (
        |
          ClassMirror = platform mirrors ClassMirror.
          subject = Subject new.
          closure = subject maker.
          builder = (ClassMirror reflecting: Subject) mixin declaration asBuilder.
          sum ::= 0.
        |
        builder instanceSide methods addFromSource: 'public maker = (
          (* the second, which is longer *)
          ^[:index | index max: 1] )'.
        builder install.
        1 to: 1000 do: [:index |
          sum:: sum + (closure value: index) + (subject maker value: index)].
        platform transcript show: sum printString; newLine.
      )
    )
"};

/// The body an edit that changed only a comment superseded keeps its
/// statements and sends, at their lines in the file.
#[test]
fn a_recommented_body_keeps_its_statements_in_the_file() {
    let bodies = described_bodies("trace-file-recomment", A_RECOMMENTED_MAKER, "1001000\n");
    let (line, column) = position_in(A_RECOMMENTED_MAKER, "public maker");
    let old = bodies
        .iter()
        .find(|record| record["name"] == "Demo/Subject>>maker" && record["range"][0] == line)
        .unwrap_or_else(|| panic!("the superseded maker at its file line: {bodies:?}"));
    assert_eq!(old["range"][1], column, "{old}");
    let (line, column) = position_in(A_RECOMMENTED_MAKER, "^[:index | index max: 1].");
    assert!(
        old["statements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|statement| statement["range"][0] == line && statement["range"][1] == column),
        "its statement at its file line: {old}"
    );
    let (line, column) = position_in(A_RECOMMENTED_MAKER, "index max: 1].\n");
    assert!(
        bodies.iter().any(|record| {
            record["name"] == "[] in Demo/Subject>>maker"
                && record["sends"].as_array().unwrap().iter().any(|send| {
                    send["call"] == serde_json::json!([line, column, line, column + 12])
                })
        }),
        "its block's send at its file line: {bodies:?}"
    );
}

/// A program whose `maker` one edit compiles and an edit of the class
/// above moves down before it first runs.
const A_MOVED_MAKER: &str = indoc! {"
    class Demo = Object new () (
      class Earlier = Object new () (
        public first = ( ^1 )
      )

      class Subject = Object new () (
        public maker = ( ^[:index | index max: 1] )
      )

      public main: platform args: arguments = (
        |
          ClassMirror = platform mirrors ClassMirror.
          subject = Subject new.
          sum ::= 0.
        |
        self install: Subject with: 'public maker = ( ^[:index | index max: 2] )' using: ClassMirror.
        self install: Earlier with: 'public second = (
          ^2 )' using: ClassMirror.
        1 to: 1000 do: [:index | sum:: sum + (subject maker value: index)].
        platform transcript show: sum printString; newLine.
      )

      install: class with: source using: ClassMirror = (
        | builder = (ClassMirror reflecting: class) mixin declaration asBuilder. |
        builder instanceSide methods addFromSource: source.
        builder install.
      )
    )
"};

/// A body one edit compiled and a later edit moved, whose text the file
/// never held, is placed where the module's current text holds it, its
/// statements and its block's send with it, each within its line.
#[test]
fn a_moved_body_is_placed_where_its_source_stands() {
    let bodies = described_bodies("trace-file-moved", A_MOVED_MAKER, "500501\n");
    let source = "public maker = ( ^[:index | index max: 2] )";
    let maker = bodies
        .iter()
        .find(|record| {
            record["name"] == "Demo/Subject>>maker" && record["checksum"] == checksum_of(source)
        })
        .unwrap_or_else(|| panic!("the first edit's maker described: {bodies:?}"));
    let range = &maker["range"];
    assert!(range.is_array(), "{maker}");
    assert_eq!(maker["placed"], "current", "{maker}");
    assert_eq!(range[0], range[2], "one line: {maker}");
    let start = range[1].as_u64().unwrap();
    assert_eq!(range[3].as_u64().unwrap() - start, source.len() as u64);
    let statement = &maker["statements"][0]["range"];
    assert_eq!(statement[0], range[0], "{maker}");
    let offset = source.find("^[").unwrap() as u64;
    assert_eq!(statement[1].as_u64().unwrap(), start + offset, "{maker}");
}

/// Every body is described in the text it was compiled from, whatever
/// edit was installed during the traced run: the body the edit compiled
/// in the module's text as the edit left it -- its range one line, as its
/// new source is, and its checksum that source's -- and the body the edit
/// superseded and a body the edit moved down, both compiled from the file,
/// at their lines in the file, so that a reader places each where its
/// text stands.
#[test]
fn an_edited_body_is_described_in_its_own_text() {
    let directory = ScratchDirectory::new("trace-file-edit");
    let program = directory.join("Demo.ns");
    std::fs::write(&program, AN_EDITED_MAKER).unwrap();
    let file = directory.join("trace.jsonl");
    let output = plain_run(&program)
        .args([
            "--trace=caches".to_string(),
            format!("--trace-file={}", file.display()),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1501504\n");
    let text = std::fs::read_to_string(&file).unwrap();
    let bodies: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["kind"] == "body")
        .collect();
    let makers: Vec<&serde_json::Value> = bodies
        .iter()
        .filter(|record| record["name"] == "Demo/Subject>>maker")
        .collect();
    let new_source = "public maker = ( ^[:index | index max: 2] )";
    let placed = makers
        .iter()
        .find(|maker| maker["checksum"] == checksum_of(new_source))
        .unwrap_or_else(|| panic!("the edit's maker described: {text}"));
    let range = &placed["range"];
    assert!(range.is_array(), "{placed}");
    // Its ranges count in the edited text, which the file does not hold.
    assert_eq!(placed["placed"], "current", "{placed}");
    assert_eq!(range[0], range[2], "one line: {placed}");
    assert_eq!(
        range[3].as_u64().unwrap() - range[1].as_u64().unwrap(),
        new_source.len() as u64,
        "{placed}"
    );
    let old_source = "public maker = (\n      ^[:index |\n        index max: 1].\n    )";
    let superseded = makers
        .iter()
        .find(|maker| maker["checksum"] == checksum_of(old_source))
        .unwrap_or_else(|| panic!("the superseded maker described: {text}"));
    let (line, column) = edited_maker_position("public maker");
    assert_eq!(
        superseded["range"],
        serde_json::json!([line, column, line + 3, 5]),
        "{superseded}"
    );
    assert_eq!(superseded["placed"], "source", "{superseded}");
    let later = bodies
        .iter()
        .find(|record| record["name"] == "Demo>>later:")
        .unwrap_or_else(|| panic!("`later:` described: {text}"));
    let (line, column) = edited_maker_position("later: count");
    assert_eq!(later["range"][0], line, "the file's line: {later}");
    assert_eq!(later["range"][1], column, "{later}");
    assert_eq!(later["placed"], "source", "{later}");
    let (loop_line, loop_column) = edited_maker_position("1 to: count");
    assert!(
        later["sends"]
            .as_array()
            .unwrap()
            .iter()
            .any(|send| send["call"][0] == loop_line && send["call"][1] == loop_column),
        "the loop's send at its line in the file: {later}"
    );
}

/// A program whose loop runs the core library's `inject:into:` and
/// `collect:`, each with a block of its own.
const A_LIBRARY_LOOP: &str = indoc! {"
    class Demo = Object new () (
      public main: platform args: arguments = (
        |
          sum ::= 0.
          list = platform collections List new.
        |
        1 to: 10 do: [:index | list add: index].
        1 to: 2000 do: [:round |
          sum:: sum + (list inject: 0 into: [:total :each | total + each]).
          sum:: sum + (list collect: [:each | each]) size].
        platform transcript show: sum printString; newLine.
      )
    )
"};

/// A block of a core library method is placed in the library's text, as
/// its root is, inside its root's range.
#[test]
fn a_library_block_is_placed_in_the_library() {
    let bodies = described_bodies("trace-file-library-block", A_LIBRARY_LOOP, "130000\n");
    let blocks: Vec<&serde_json::Value> = bodies
        .iter()
        .filter(|record| {
            record["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("[] in newspeak.core."))
        })
        .collect();
    assert!(!blocks.is_empty(), "a library block described: {bodies:?}");
    for block in blocks {
        assert_eq!(block["placed"], "source", "{block}");
        assert!(block["source"]["library"].is_string(), "{block}");
        let range = &block["range"];
        assert!(range.is_array(), "{block}");
        let root = bodies
            .iter()
            .find(|record| record["body"] == block["family"])
            .unwrap_or_else(|| panic!("its root described: {block}"));
        let position = |range: &serde_json::Value, index: usize| {
            (
                range[index].as_u64().unwrap(),
                range[index + 1].as_u64().unwrap(),
            )
        };
        assert!(
            position(&root["range"], 0) <= position(range, 0),
            "{block} in {root}"
        );
        assert!(
            position(range, 2) <= position(&root["range"], 2),
            "{block} in {root}"
        );
    }
}

/// `--trace-file` holds what a trace or a print option writes, so it is
/// refused without one.
#[test]
fn trace_file_needs_something_to_trace() {
    let stderr = refused_stderr("trace-file-alone", &["--trace-file=x.jsonl"]);
    assert!(stderr.contains("--trace-file"), "stderr: {stderr}");
}

/// A trace file that cannot be created is refused, naming it, before the
/// program runs.
#[test]
fn an_uncreatable_trace_file_is_refused() {
    let directory = ScratchDirectory::new("trace-file-uncreatable");
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_HOT_METHOD).unwrap();
    let file = directory.join("missing").join("trace.jsonl");
    let output = plain_run(&program)
        .args([
            "--trace=compile".to_string(),
            format!("--trace-file={}", file.display()),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "",
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("`{}`", file.display())),
        "stderr: {stderr}"
    );
    assert!(!file.exists());
}

/// A method that answers a constant, sent a hundred times so that the
/// baseline JIT compiles it.
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
const A_CONSTANT_METHOD: &str = indoc! {"
    class Demo = Object new () (
      answer = (
        ^42.
      )

      public main: platform args: arguments = (
        |
          total ::= 0.
        |
        1 to: 100 do: [:index | total:: total + answer].
        platform transcript show: total printString; newLine.
      )
    )
"};

/// The listing of `answer`'s baseline code in a build with debug
/// assertions, which checks every register it reads for the dead word.
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
const THE_CONSTANT_METHODS_LISTING_CHECKED: &str = r"--- code {Demo>>answer} baseline x64 base=0x… bytes=576
    +0x0000  mov r11,[r14+58h]
    +0x0004  mov rcx,[r11+8]
    +0x0008  cmp rcx,[r11+10h]
    +0x000c  jae +0x00d6
    +0x0012  mov rax,rbx
    +0x0015  mov rdx,[r14+28h]
    +0x0019  sub rax,rdx
    +0x001c  add rax,70h
    +0x0020  mov rdx,[r11+18h]
    +0x0024  shl rdx,3
    +0x0028  cmp rax,rdx
    +0x002b  ja +0x00ee
    +0x0031  add rcx,1
    +0x0035  mov [r11+8],rcx
    +0x0039  mov [rbx],rsi
    +0x003c  mov [rbx+8],rdi
    +0x0040  mov rdx,2F56AF56Eh
    +0x004a  mov [rbx+68h],rdx
    +0x004e  mov qword ptr [rbx+18h],2
    +0x0056  mov r11,[r14+58h]
    +0x005a  mov rdx,[r11+70h]
    +0x005e  add rdx,1
    +0x0062  and rdx,7FFFFFFFh
    +0x0069  mov [r11+70h],rdx
    +0x006d  shl rdx,1Eh
    +0x0071  mov ecx,28000001h
    +0x0076  add rdx,rcx
    +0x0079  mov [rbx+20h],rdx
    +0x007d  mov r11,[data+0x0010]
    +0x0084  sub qword ptr [r11],2
    +0x0088  jl +0x0171
  @0  stmt 0  LoadLiteral r13, 42
    +0x008e  mov ecx,151h
    +0x0093  mov [rbx+68h],rcx
  @1  stmt 0  Return r13
    +0x0097  mov rax,[rbx+68h]
    +0x009b  mov r11,2F56AF56Eh
    +0x00a5  cmp rax,r11
    +0x00a8  je +0x01d1
    +0x00ae  mov r11,[r14+58h]
    +0x00b2  mov rcx,[r11+8]
    +0x00b6  add rcx,0FFFFFFFFFFFFFFFFh
    +0x00ba  mov [r11+8],rcx
    +0x00be  ret
  out of line
    +0x00bf  mov r9,[data+0x0000]
    +0x00c6  mov rcx,[data+0x0008]
    +0x00cd  lea r10,[+0x0000]
    +0x00d4  jmp rcx  ; stub dispatching-stub
    +0x00d6  lea rdx,[+0x00dd]
    +0x00dd  mov eax,3
    +0x00e2  mov ecx,1
    +0x00e7  mov r11,[r14+60h]
    +0x00eb  jmp r11  ; exit push-refused @1
    +0x00ee  push r11
    +0x00f0  push rcx
    +0x00f1  push rsi
    +0x00f2  push rdi
    +0x00f3  push r9
    +0x00f5  push r10
    +0x00f7  push r12
    +0x00f9  mov [r14+70h],rsp
    +0x00fd  and rsp,0FFFFFFFFFFFFFFF0h
    +0x0101  lea rsp,[rsp-30h]
    +0x0106  mov [rsp],rax
    +0x010a  mov r8,[rsp]
    +0x010e  mov r9d,0
    +0x0114  mov qword ptr [rsp+20h],0
    +0x011d  mov rcx,r14
    +0x0120  mov edx,11h
    +0x0125  mov r11,[r14+8]
    +0x0129  call r11  ; leaf grow-stack
    +0x012c  mov rsp,[r14+70h]
    +0x0130  mov rdx,2F56AF56Eh
    +0x013a  mov r8,2F56AF56Eh
    +0x0144  pop r12
    +0x0146  pop r10
    +0x0148  pop r9
    +0x014a  pop rdi
    +0x014b  pop rsi
    +0x014c  pop rcx
    +0x014d  pop r11
    +0x014f  cmp rax,0
    +0x0153  je +0x0031
    +0x0159  lea rdx,[+0x0160]
    +0x0160  mov eax,3
    +0x0165  mov ecx,0
    +0x016a  mov r11,[r14+60h]
    +0x016e  jmp r11  ; exit push-refused @0
    +0x0171  mov [r14+70h],rsp
    +0x0175  and rsp,0FFFFFFFFFFFFFFF0h
    +0x0179  lea rsp,[rsp-30h]
    +0x017e  mov [rsp],r11
    +0x0182  mov [rsp+8],rbx
    +0x0187  mov r8,[rsp]
    +0x018b  mov r9,[rsp+8]
    +0x0190  lea r11,[+0x0197]
    +0x0197  mov [rsp+20h],r11
    +0x019c  mov rcx,r14
    +0x019f  mov edx,13h
    +0x01a4  mov r11,[r14+8]
    +0x01a8  call r11  ; leaf budget
    +0x01ab  mov rsp,[r14+70h]
    +0x01af  cmp rax,0
    +0x01b3  je +0x008e
    +0x01b9  lea rdx,[+0x01c0]
    +0x01c0  mov eax,6
    +0x01c5  mov ecx,0
    +0x01ca  mov r11,[r14+60h]
    +0x01ce  jmp r11  ; exit budget-spent @0
    +0x01d1  mov qword ptr [r14+0F0h],0Dh
    +0x01dc  lea rdx,[+0x01e3]
    +0x01e3  mov eax,2
    +0x01e8  mov ecx,1
    +0x01ed  mov r11,[r14+60h]
    +0x01f1  jmp r11  ; exit dead-register @1
data:
    data+0x0000  dispatching rows
    data+0x0008  dispatching tail
    data+0x0010  budget word
--- end
";

/// The listing of `answer`'s baseline code in a build without debug
/// assertions.
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
const THE_CONSTANT_METHODS_LISTING: &str = r"--- code {Demo>>answer} baseline x64 base=0x… bytes=448
    +0x0000  mov r11,[r14+58h]
    +0x0004  mov rcx,[r11+8]
    +0x0008  cmp rcx,[r11+10h]
    +0x000c  jae +0x0086
    +0x0012  mov rax,rbx
    +0x0015  mov rdx,[r14+28h]
    +0x0019  sub rax,rdx
    +0x001c  add rax,70h
    +0x0020  mov rdx,[r11+18h]
    +0x0024  shl rdx,3
    +0x0028  cmp rax,rdx
    +0x002b  ja +0x009e
    +0x0031  add rcx,1
    +0x0035  mov [r11+8],rcx
    +0x0039  mov [rbx],rsi
    +0x003c  mov [rbx+8],rdi
    +0x0040  mov r11,[data+0x0010]
    +0x0047  sub qword ptr [r11],2
    +0x004b  jl +0x010d
  @0  stmt 0  LoadLiteral r13, 42
    +0x0051  mov ecx,151h
    +0x0056  mov [rbx+68h],rcx
  @1  stmt 0  Return r13
    +0x005a  mov rax,[rbx+68h]
    +0x005e  mov r11,[r14+58h]
    +0x0062  mov rcx,[r11+8]
    +0x0066  add rcx,0FFFFFFFFFFFFFFFFh
    +0x006a  mov [r11+8],rcx
    +0x006e  ret
  out of line
    +0x006f  mov r9,[data+0x0000]
    +0x0076  mov rcx,[data+0x0008]
    +0x007d  lea r10,[+0x0000]
    +0x0084  jmp rcx  ; stub dispatching-stub
    +0x0086  lea rdx,[+0x008d]
    +0x008d  mov eax,3
    +0x0092  mov ecx,1
    +0x0097  mov r11,[r14+60h]
    +0x009b  jmp r11  ; exit push-refused @1
    +0x009e  push r11
    +0x00a0  push rcx
    +0x00a1  push rsi
    +0x00a2  push rdi
    +0x00a3  push r9
    +0x00a5  push r10
    +0x00a7  push r12
    +0x00a9  mov [r14+70h],rsp
    +0x00ad  and rsp,0FFFFFFFFFFFFFFF0h
    +0x00b1  lea rsp,[rsp-30h]
    +0x00b6  mov [rsp],rax
    +0x00ba  mov r8,[rsp]
    +0x00be  mov r9d,0
    +0x00c4  mov qword ptr [rsp+20h],0
    +0x00cd  mov rcx,r14
    +0x00d0  mov edx,11h
    +0x00d5  mov r11,[r14+8]
    +0x00d9  call r11  ; leaf grow-stack
    +0x00dc  mov rsp,[r14+70h]
    +0x00e0  pop r12
    +0x00e2  pop r10
    +0x00e4  pop r9
    +0x00e6  pop rdi
    +0x00e7  pop rsi
    +0x00e8  pop rcx
    +0x00e9  pop r11
    +0x00eb  cmp rax,0
    +0x00ef  je +0x0031
    +0x00f5  lea rdx,[+0x00fc]
    +0x00fc  mov eax,3
    +0x0101  mov ecx,0
    +0x0106  mov r11,[r14+60h]
    +0x010a  jmp r11  ; exit push-refused @0
    +0x010d  mov [r14+70h],rsp
    +0x0111  and rsp,0FFFFFFFFFFFFFFF0h
    +0x0115  lea rsp,[rsp-30h]
    +0x011a  mov [rsp],r11
    +0x011e  mov [rsp+8],rbx
    +0x0123  mov r8,[rsp]
    +0x0127  mov r9,[rsp+8]
    +0x012c  lea r11,[+0x0133]
    +0x0133  mov [rsp+20h],r11
    +0x0138  mov rcx,r14
    +0x013b  mov edx,13h
    +0x0140  mov r11,[r14+8]
    +0x0144  call r11  ; leaf budget
    +0x0147  mov rsp,[r14+70h]
    +0x014b  cmp rax,0
    +0x014f  je +0x0051
    +0x0155  lea rdx,[+0x015c]
    +0x015c  mov eax,6
    +0x0161  mov ecx,0
    +0x0166  mov r11,[r14+60h]
    +0x016a  jmp r11  ; exit budget-spent @0
data:
    data+0x0000  dispatching rows
    data+0x0008  dispatching tail
    data+0x0010  budget word
--- end
";

/// A whole listing of a body that calls nothing but the prologue's and
/// the budget's leaves, pinned but for its base address: it differs
/// between runs only in its head (decision 14). The code is that of the
/// x64 Windows convention, its leaf calls the convention's.
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
#[test]
fn a_leafless_body_pins_its_listing() {
    let directory = ScratchDirectory::new("print-pinned");
    let program = directory.join("Demo.ns");
    std::fs::write(&program, A_CONSTANT_METHOD).unwrap();
    if plain_jit_counts(
        &program,
        "4200
",
        &[],
    )
    .is_none()
    {
        return;
    }
    let output = plain_run(&program)
        .args(["--print-code", "--print-filter=Demo>>answer"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "4200
"
    );
    let listings = listings(&stderr);
    assert_eq!(listings.len(), 1, "{stderr}");
    let mut lines = listings[0].clone();
    let head = lines[0];
    let base = head
        .find(" base=0x")
        .and_then(|start| Some((start, head[start..].find(" bytes=")? + start)))
        .unwrap_or_else(|| panic!("no base in {head}"));
    let masked = format!("{} base=0x…{}", &head[..base.0], &head[base.1..]);
    lines[0] = &masked;
    let expected = if cfg!(debug_assertions) {
        THE_CONSTANT_METHODS_LISTING_CHECKED
    } else {
        THE_CONSTANT_METHODS_LISTING
    };
    assert_eq!(
        lines.join(
            "
"
        ) + "
",
        expected
    );
}
