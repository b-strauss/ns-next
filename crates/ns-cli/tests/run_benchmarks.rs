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

use std::path::Path;
use std::process::Command;

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
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(harness)
        .args(["--", "--timed", name])
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    if !output.status.success() {
        return Err(format!("stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let shaped = stdout
        .strip_prefix(&format!("{name}: "))
        .and_then(|score| score.strip_suffix('\n'))
        .and_then(|score| score.split_once('.'))
        .is_some_and(|(whole, fraction)| {
            !whole.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.len() == 1
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !shaped {
        return Err(format!("expected `{name}: <digits>.<digit>`, got:\n{stdout}").into());
    }
    Ok(())
}
