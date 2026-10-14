//! Every benchmark program answers exactly what its `.toml` records, under
//! both lowerings: each micro benchmark under `benchmarks/micro/`, and the
//! Are We Fast Yet project, which runs every benchmark at its full size and
//! verifies its result. A benchmark is sized to be measured, so this runs
//! only in the nextest profile `benchmarks`, when benchmarking and once at
//! the end of a branch (`docs/design/execution.md`, "The Are We Fast Yet
//! benchmarks"). One trial per program and lowering (`libtest-mimic`, so
//! `harness = false`), as `run_goldens.rs` has.

use std::path::Path;
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for program in common::benchmark_programs() {
        let name = common::benchmark_name(&program);
        for lowering in LOWERINGS {
            let program = program.clone();
            trials.push(Trial::test(format!("{lowering}::{name}"), move || {
                matches_under_one_lowering(&program, lowering)
            }));
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

/// The benchmark's exit status, stdout and stderr match its recorded
/// outcome exactly under `lowering`.
fn matches_under_one_lowering(program: &Path, lowering: &str) -> Result<(), Failed> {
    let expectation = common::expectation_of(program);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(program)
        .arg("--lowering")
        .arg(lowering)
        .args(common::program_arguments(program))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(
        &output,
        &expectation,
        &format!("under the {lowering} lowering"),
    )
    .map_err(Failed::from)
}
