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
    let conclusion = libtest_mimic::run(&arguments, trials);
    // Best effort: a directory left behind is emptied by the next process
    // that compiles into it.
    let _ = std::fs::remove_dir_all(java_classes_directory());
    conclusion.exit();
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
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(harness)
        .args(["--", "--timed", name])
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    if !output.status.success() {
        return Err(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let stdout = common::normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if !score_is_shaped(&stdout, name) {
        return Err(format!("expected `{name}: <digits>.<digit>`, got:\n{stdout}").into());
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

/// The directory of the Java counterparts' classes, compiled on first use
/// and once per process: every Java trial of the process shares the one
/// compilation, and a compilation error fails each of them with `javac`'s
/// output.
fn java_classes() -> Result<&'static Path, String> {
    static CLASSES: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CLASSES
        .get_or_init(compile_java)
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(Clone::clone)
}

/// Where this process compiles the Java counterparts:
/// `<CARGO_TARGET_TMPDIR>/are-we-fast-yet-java/<process id>`. nextest runs
/// each trial in a process of its own, and the processes run side by side;
/// one shared directory would be emptied by one of them while another runs
/// its classes.
fn java_classes_directory() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("are-we-fast-yet-java")
        .join(std::process::id().to_string())
}

/// Compiles every `.java` under `benchmarks/are-we-fast-yet/java/` into the
/// emptied `java_classes_directory`. Emptying it first means a renamed or
/// deleted source leaves no class behind that still runs, even in a
/// directory an earlier run with the same process id left.
fn compile_java() -> Result<PathBuf, String> {
    let classes = java_classes_directory();
    match std::fs::remove_dir_all(&classes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("empty {}: {error}", classes.display())),
    }
    std::fs::create_dir_all(&classes)
        .map_err(|error| format!("create {}: {error}", classes.display()))?;
    let mut sources = Vec::new();
    collect_java_sources(
        &common::repository_root().join("benchmarks/are-we-fast-yet/java"),
        &mut sources,
    )?;
    sources.sort();
    let javac = java_tool("javac")?;
    let output = Command::new(&javac)
        .args(["--release", "21", "-encoding", "UTF-8", "-d"])
        .arg(&classes)
        .args(&sources)
        .output()
        .map_err(|error| {
            format!(
                "run {}: {error}; the benchmarks gate needs a JDK, on JAVA_HOME or PATH",
                javac.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "javac failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(classes)
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
