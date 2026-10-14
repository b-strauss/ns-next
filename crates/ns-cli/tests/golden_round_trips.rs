//! Every golden program through the deployment artifact, one test per golden
//! and lowering: `build` then `load` must be indistinguishable from `run`.
//! Each is a test of its own (`libtest-mimic`, `harness = false`), as in
//! `run_goldens.rs`, so the sweep runs in parallel, reports every golden that
//! differs, and filters by name (`cargo nextest run -p ns-cli --test
//! golden_round_trips lazy`); run as one loop, it was most of the suite's
//! time, and almost all of it under the stress modes. It doubles the
//! goldens' cost, so it runs in a nextest profile of its own, for a commit
//! that touches the image format or the instruction encoding and at the
//! end of a batch: `cargo nextest run -p ns-cli --profile images`.

use std::path::Path;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;
use common::{ns_next, ScratchDirectory};

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    let mut swept = 0;
    for program in common::golden_programs() {
        let name = common::golden_name(&program);
        // A skipped golden records what the runtime does not answer yet, so
        // the artifact cannot be held to `run` on it.
        let skipped = common::expectation_of(&program).skipped().is_some();
        if !skipped {
            swept += 1;
        }
        for lowering in LOWERINGS {
            let path = program.clone();
            trials.push(
                Trial::test(format!("build_load::{lowering}::{name}"), move || {
                    an_artifact_runs_as_its_source_does(&path, lowering)
                })
                .with_ignored_flag(skipped),
            );
        }
    }
    assert!(
        swept >= 10,
        "expected several goldens to be swept, found {swept}"
    );
    libtest_mimic::run(&arguments, trials).exit();
}

/// `build` then `load` must be indistinguishable from `run` on the same
/// program -- exit status, stdout and stderr -- under both lowerings. An
/// artifact carries no source, so comparing it against `run` is how one is
/// checked at all; `run_goldens.rs` pins the same goldens against their
/// `.toml` files, so equality here also holds against a recorded answer.
/// Under the reference lowering this also takes the encoder and decoder over
/// code with no jumps and many registers.
///
/// `build` assembles the application exactly as `run` does, so a program
/// whose assembly fails -- a root the root rule or the assembly refuses, a
/// resolve error, an initializer that raises -- fails the build with `run`'s
/// status and report, before either prints anything. A golden recorded as
/// failing in its entry send builds, and the `load` that runs it is where the
/// artifact's error path is pinned against `run`.
fn an_artifact_runs_as_its_source_does(path: &Path, lowering: &str) -> Result<(), Failed> {
    let directory = ScratchDirectory::new("golden-artifact");
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let out = directory.join(&format!("{stem}.{lowering}.nsi"));
    let program_arguments = common::program_arguments(path);
    let mut run_arguments: Vec<&std::ffi::OsStr> = vec![
        "run".as_ref(),
        path.as_os_str(),
        "--lowering".as_ref(),
        lowering.as_ref(),
    ];
    run_arguments.extend(program_arguments.iter().map(std::ffi::OsString::as_os_str));
    let (run_ok, run_out, run_error) = ns_next(&run_arguments);
    let (built, build_out, build_error) = ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        out.as_os_str(),
        "--lowering".as_ref(),
        lowering.as_ref(),
    ]);
    if !built {
        if run_ok {
            return Err(format!("the build failed where run succeeded: {build_error}").into());
        }
        if build_error != run_error {
            return Err(format!(
                "build must fail exactly as run does:
--- run
{run_error}--- build
{build_error}"
            )
            .into());
        }
        if !build_out.is_empty() || !run_out.is_empty() {
            return Err(format!(
                "a failed assembly prints nothing:
--- run
{run_out}--- build
{build_out}"
            )
            .into());
        }
        return Ok(());
    }
    let mut load_arguments: Vec<&std::ffi::OsStr> = vec!["load".as_ref(), out.as_os_str()];
    load_arguments.extend(program_arguments.iter().map(std::ffi::OsString::as_os_str));
    let (load_ok, load_out, load_error) = ns_next(&load_arguments);
    if load_ok != run_ok {
        return Err(format!("exit status differs: load {load_ok}, run {run_ok}").into());
    }
    if load_out != run_out {
        return Err(format!(
            "stdout differs:
--- run
{run_out}--- load
{load_out}"
        )
        .into());
    }
    if load_error != run_error {
        return Err(format!(
            "stderr differs:
--- run
{run_error}--- load
{load_error}"
        )
        .into());
    }
    Ok(())
}
