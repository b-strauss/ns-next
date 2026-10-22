//! Every benchmark program answers exactly what its `.toml` records, under
//! both lowerings: each micro benchmark under `benchmarks/micro/`, and each
//! benchmark of the Are We Fast Yet harness, run by name at its full size,
//! verifying its result. And a timed run of each prints a score of the
//! harness's shape. A benchmark is sized to be measured, so this runs only
//! in the nextest profile `benchmarks-verify`, when benchmarking and once at
//! the end of a branch (`docs/design/execution.md`, "The Are We Fast Yet
//! benchmarks"). It verifies; it measures nothing: a debug build's times
//! say nothing, and the timed runs' scores are thrown away.
//!
//! One trial per run and lowering (`libtest-mimic`, so `harness = false`),
//! each benchmark of the harness its own: run as one process over all of
//! them, the full-size benchmarks took minutes one after another.
//!
//! The Java counterparts under `benchmarks/are-we-fast-yet/java/` are held
//! to the same record: `java::<Name>` runs the Java harness by name and
//! expects the harness's `<Name>: ok` line, and `java-timed::<Name>` checks
//! a timed run's score as `timed::<Name>` does. They are compiled once, with
//! the JDK's `javac` from `JAVA_HOME`, or from `PATH` when `JAVA_HOME` is not
//! set. Without a JDK they fail and say so: a gate that passes without
//! running is no gate.
//!
//! Both harnesses take `--warm-up <milliseconds>` and `--measure
//! <milliseconds>` after `--timed`: `flags::<side>::*` runs each harness
//! with short windows, in either order, and with malformed flags, which
//! end the run with the usage.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for run in common::benchmark_runs() {
        let run = std::sync::Arc::new(run);
        for lowering in LOWERINGS {
            let run = run.clone();
            trials.push(Trial::test(
                format!("{lowering}::{}", run.name),
                move || matches_under_one_lowering(&run, lowering),
            ));
        }
    }
    let harness = common::repository_root().join("benchmarks/are-we-fast-yet");
    for name in common::benchmark_names(&common::expectation_of(&harness)) {
        let harness = harness.clone();
        trials.push(Trial::test(format!("timed::{name}"), move || {
            prints_a_score(&harness, &name)
        }));
    }
    for name in common::benchmark_names(&common::expectation_of(&harness)) {
        let expectation = common::Expectation {
            status: 0,
            stdout: format!("{name}: ok\n"),
            stderr: String::new(),
            skipped: None,
            arguments: vec![name.clone()],
        };
        let checked = name.clone();
        trials.push(Trial::test(format!("java::{name}"), move || {
            java_answers_ok(&checked, &expectation)
        }));
        trials.push(Trial::test(format!("java-timed::{name}"), move || {
            java_prints_a_score(&name)
        }));
    }
    trials.push(Trial::test(
        "java::unknown-benchmark",
        java_refuses_an_unknown_benchmark,
    ));
    trials.push(Trial::test(
        "java::a-failing-check-prints-no-ok",
        java_prints_no_ok_for_a_failing_check,
    ));
    trials.push(Trial::test(
        "java::missing-jdk-is-named",
        java_names_a_missing_jdk,
    ));
    for side in Side::ALL {
        trials.push(Trial::test(
            format!("flags::{}::windows-print-a-score", side.name()),
            move || windows_print_a_score(side, &["--timed", "--warm-up", "0", "--measure", "1"]),
        ));
        trials.push(Trial::test(
            format!("flags::{}::either-order", side.name()),
            move || windows_print_a_score(side, &["--timed", "--measure", "1", "--warm-up", "0"]),
        ));
        trials.push(Trial::test(
            format!("flags::{}::rejected", side.name()),
            move || malformed_flags_are_refused(side),
        ));
    }
    libtest_mimic::run(&arguments, trials).exit();
}

/// The run's exit status, stdout and stderr match its recorded outcome
/// exactly under `lowering`.
fn matches_under_one_lowering(run: &common::BenchmarkRun, lowering: &str) -> Result<(), Failed> {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&run.program)
        .arg("--lowering")
        .arg(lowering)
        .args(common::after_the_separator(&run.arguments))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(
        &output,
        &run.expectation,
        &format!("under the {lowering} lowering"),
    )
    .map_err(Failed::from)
}

/// `--timed <name>` prints `<name>: <runs per second>` with one fraction
/// digit; the number differs between runs, so only its shape is checked.
fn prints_a_score(harness: &Path, name: &str) -> Result<(), Failed> {
    let output = ns_next_harness(harness, &["--timed", name])?;
    if !output.status.success() {
        return Err(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let stdout = common::normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if !score_is_shaped(&stdout, name) {
        return Err(format!("expected `{name}: <digits>.<digit>`, got:\n{stdout}").into());
    }
    Ok(())
}

/// The ns-next harness at `harness` run with `arguments`.
fn ns_next_harness(harness: &Path, arguments: &[&str]) -> Result<std::process::Output, String> {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(harness)
        .arg("--")
        .args(arguments)
        .output()
        .map_err(|error| format!("run ns-next: {error}"))
}

/// One of the two harnesses that take the same command line.
#[derive(Clone, Copy)]
enum Side {
    NsNext,
    Java,
}

impl Side {
    const ALL: [Side; 2] = [Side::NsNext, Side::Java];

    /// The side's name in a trial's name.
    fn name(self) -> &'static str {
        match self {
            Side::NsNext => "ns-next",
            Side::Java => "java",
        }
    }

    /// The side's harness run with `arguments`.
    fn run(self, arguments: &[&str]) -> Result<std::process::Output, String> {
        match self {
            Side::NsNext => ns_next_harness(
                &common::repository_root().join("benchmarks/are-we-fast-yet"),
                arguments,
            ),
            Side::Java => java_harness(arguments),
        }
    }
}

/// `flags` followed by `Sieve` succeeds and prints a score of the shape
/// `timed::Sieve` checks: the measuring windows can be set from the command
/// line, in either order.
fn windows_print_a_score(side: Side, flags: &[&str]) -> Result<(), Failed> {
    let mut arguments = flags.to_vec();
    arguments.push("Sieve");
    let output = side.run(&arguments)?;
    if !output.status.success() {
        return Err(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let stdout = common::normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if !score_is_shaped(&stdout, "Sieve") {
        return Err(format!("expected `Sieve: <digits>.<digit>`, got:\n{stdout}").into());
    }
    Ok(())
}

/// A window flag without `--timed`, a value that is no non-negative
/// integer, a repeated flag and a flag without its value each end the run
/// non-zero, with nothing on stdout and the usage on stderr.
fn malformed_flags_are_refused(side: Side) -> Result<(), Failed> {
    let malformed: [&[&str]; 5] = [
        &["--warm-up", "5"],
        &["--timed", "--measure", "x"],
        &["--timed", "--measure", "-1"],
        &["--timed", "--warm-up", "1", "--warm-up", "1"],
        &["--timed", "--measure"],
    ];
    for arguments in malformed {
        let output = side.run(arguments)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() || !output.stdout.is_empty() || !stderr.contains("usage:") {
            return Err(format!(
                "{arguments:?}: expected a non-zero exit, no stdout and `usage:` on stderr, \
                 got {:?}\nstdout:\n{}\nstderr:\n{stderr}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
            )
            .into());
        }
    }
    Ok(())
}

/// Whether `stdout` is the one line `<name>: <digits>.<digit>`: a score
/// with one fraction digit after a point, never a comma, whatever the
/// locale.
fn score_is_shaped(stdout: &str, name: &str) -> bool {
    stdout
        .strip_prefix(&format!("{name}: "))
        .and_then(|score| score.strip_suffix('\n'))
        .and_then(|score| score.split_once('.'))
        .is_some_and(|(whole, fraction)| {
            !whole.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.len() == 1
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// The Java harness run with `name` exits zero and prints `<name>: ok`
/// and nothing else.
fn java_answers_ok(name: &str, expectation: &common::Expectation) -> Result<(), Failed> {
    let output = java_harness(&[name])?;
    common::compare(&output, expectation, "under the Java harness").map_err(Failed::from)
}

/// The Java harness's `--timed <name>` prints a score of the shape
/// `timed::<name>` holds ns-next's to.
fn java_prints_a_score(name: &str) -> Result<(), Failed> {
    let output = java_harness(&["--timed", name])?;
    if !output.status.success() {
        return Err(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let stdout = common::normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if !score_is_shaped(&stdout, name) {
        return Err(format!("expected `{name}: <digits>.<digit>`, got:\n{stdout}").into());
    }
    Ok(())
}

/// A name the Java harness does not know ends the run non-zero, with
/// nothing on stdout and the name on stderr.
fn java_refuses_an_unknown_benchmark() -> Result<(), Failed> {
    let output = java_harness(&["NoSuchBenchmark"])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success()
        || !output.stdout.is_empty()
        || !stderr.contains("Unknown benchmark: NoSuchBenchmark")
    {
        return Err(format!(
            "expected a non-zero exit, no stdout and `Unknown benchmark: NoSuchBenchmark`, \
             got {:?}\nstdout:\n{}\nstderr:\n{stderr}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
        )
        .into());
    }
    Ok(())
}

/// A benchmark whose check fails leaves no `ok` line. The harness's one
/// test hook, the system property `areWeFastYet.failFirstCheck`, throws in
/// place of the first `bench()`; the run ends non-zero with nothing on
/// stdout.
fn java_prints_no_ok_for_a_failing_check() -> Result<(), Failed> {
    let output = Command::new(java_tool("java")?)
        .arg("-DareWeFastYet.failFirstCheck=true")
        .arg("-cp")
        .arg(java_classes()?)
        .args(["AreWeFastYet", "Sieve"])
        .output()
        .map_err(|error| format!("run java: {error}"))?;
    if output.status.success() || !output.stdout.is_empty() {
        return Err(format!(
            "expected a non-zero exit and no stdout, got {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

/// A `JAVA_HOME` that holds no JDK is named in the failure, not passed
/// over for whatever `PATH` holds.
fn java_names_a_missing_jdk() -> Result<(), Failed> {
    match java_tool_in(Some(Path::new("D:/no/such/jdk")), "javac") {
        Ok(found) => Err(format!("expected an error, found {}", found.display()).into()),
        Err(error) if error.contains("D:/no/such/jdk") && error.contains("JAVA_HOME") => Ok(()),
        Err(error) => {
            Err(format!("the error names neither the path nor JAVA_HOME: {error}").into())
        }
    }
}

/// The Java harness run with `arguments`, from the classes `java_classes`
/// compiled.
fn java_harness(arguments: &[&str]) -> Result<std::process::Output, String> {
    Command::new(java_tool("java")?)
        .arg("-cp")
        .arg(java_classes()?)
        .arg("AreWeFastYet")
        .args(arguments)
        .output()
        .map_err(|error| format!("run java: {error}"))
}

/// The directory of the Java counterparts' classes, found or compiled on
/// first use and once per process: every Java trial of the process shares
/// it, and a compilation error fails each of them with `javac`'s output.
fn java_classes() -> Result<&'static Path, String> {
    static CLASSES: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CLASSES
        .get_or_init(compile_java)
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(Clone::clone)
}

/// The classes of the sources under `benchmarks/are-we-fast-yet/java/` as
/// they are now, in `<CARGO_TARGET_TMPDIR>/are-we-fast-yet-java/<key>`, the
/// key a hash of every source's path and contents. nextest runs each trial
/// in a process of its own, side by side, so there is one compilation per
/// content rather than per process: a process that finds the directory uses
/// it; otherwise it compiles into a sibling of its own and renames that into
/// place, and when another process won that race it uses the winner's. A
/// changed, renamed or deleted source gives a new key, so no stale class
/// ever runs.
fn compile_java() -> Result<PathBuf, String> {
    let root = common::repository_root().join("benchmarks/are-we-fast-yet/java");
    let mut sources = Vec::new();
    collect_java_sources(&root, &mut sources)?;
    sources.sort();
    let key = sources_key(&root, &sources)?;
    let compilations = Path::new(env!("CARGO_TARGET_TMPDIR")).join("are-we-fast-yet-java");
    let classes = compilations.join(&key);
    if classes.is_dir() {
        return Ok(classes);
    }
    let scratch = compilations.join(format!("{key}.{}.tmp", std::process::id()));
    match std::fs::remove_dir_all(&scratch) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("empty {}: {error}", scratch.display())),
    }
    std::fs::create_dir_all(&scratch)
        .map_err(|error| format!("create {}: {error}", scratch.display()))?;
    let javac = java_tool("javac")?;
    let output = Command::new(&javac)
        .args(["--release", "21", "-encoding", "UTF-8", "-d"])
        .arg(&scratch)
        .args(&sources)
        .output()
        .map_err(|error| {
            format!(
                "run {}: {error}; the benchmarks gate needs a JDK, on JAVA_HOME or PATH",
                javac.display()
            )
        })?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(format!(
            "javac failed:
{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if let Err(error) = std::fs::rename(&scratch, &classes) {
        let _ = std::fs::remove_dir_all(&scratch);
        if !classes.is_dir() {
            return Err(format!(
                "rename {} to {}: {error}",
                scratch.display(),
                classes.display()
            ));
        }
    }
    Ok(classes)
}

/// A hash of every source's path below `root` and its contents, in the
/// sorted order of `sources`, as sixteen hexadecimal digits.
fn sources_key(root: &Path, sources: &[PathBuf]) -> Result<String, String> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    for source in sources {
        let relative = source.strip_prefix(root).map_err(|error| {
            format!(
                "{} is not below {}: {error}",
                source.display(),
                root.display()
            )
        })?;
        for component in relative.components() {
            component.as_os_str().to_string_lossy().hash(&mut hasher);
        }
        std::fs::read(source)
            .map_err(|error| format!("read {}: {error}", source.display()))?
            .hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

/// Every `.java` file below `directory`, recursively.
fn collect_java_sources(directory: &Path, sources: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            collect_java_sources(&path, sources)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "java")
        {
            sources.push(path);
        }
    }
    Ok(())
}

/// The JDK tool `name`, from `JAVA_HOME` when it is set and from `PATH`
/// otherwise.
fn java_tool(name: &str) -> Result<PathBuf, String> {
    java_tool_in(
        std::env::var_os("JAVA_HOME").map(PathBuf::from).as_deref(),
        name,
    )
}

/// The JDK tool `name`. With a home, `<java_home>/bin/<name>`, or an error
/// naming that home when it holds no such tool, so that a stale `JAVA_HOME`
/// is reported rather than passed over; without one, `name` itself, for the
/// system to find on `PATH`.
fn java_tool_in(java_home: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let Some(home) = java_home else {
        return Ok(PathBuf::from(name));
    };
    let tool = home
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if tool.is_file() {
        Ok(tool)
    } else {
        Err(format!(
            "JAVA_HOME is {}, which has no bin/{name}; the benchmarks gate needs a JDK",
            home.display()
        ))
    }
}
