//! Every golden through the two artifacts, one test per golden: `build`
//! then `load` must be indistinguishable from `run` under each lowering, and
//! `snapshot` then `load` must reproduce what `run` prints. Each is a test of
//! its own (`libtest-mimic`, `harness = false`), as in `run_goldens.rs`, so
//! the sweeps run in parallel, report every golden that differs, and filter
//! by name (`cargo test -p ns-cli --test golden_round_trips lazy`); run as one
//! loop each, they were most of the suite's time, and almost all of it under
//! the stress modes.

use std::path::Path;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;
use common::{assert_snapshot_load_matches_run, ns_next, ScratchDirectory};

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    let mut swept = 0;
    for source in common::golden_sources() {
        let name = common::golden_name(&source);
        // A skipped golden records what the runtime does not answer yet, so
        // neither artifact can be held to `run` on it.
        let skipped = common::expectation_of(&source).skipped().is_some();
        if !skipped {
            swept += 1;
        }
        for lowering in LOWERINGS {
            let path = source.clone();
            trials.push(
                Trial::test(format!("build_load::{lowering}::{name}"), move || {
                    a_binary_runs_as_its_source_does(&path, lowering)
                })
                .with_ignored_flag(skipped),
            );
        }
        let path = source.clone();
        let test_name = name.replace('/', "-");
        trials.push(
            Trial::test(format!("snapshot_load::{name}"), move || {
                a_snapshot_reproduces_what_run_prints(&test_name, &path)
            })
            .with_ignored_flag(skipped),
        );
    }
    assert!(
        swept >= 10,
        "expected several goldens to be swept, found {swept}"
    );
    libtest_mimic::run(&arguments, trials).exit();
}

/// `build` then `load` must be indistinguishable from `run` on the same
/// source -- exit status, stdout and stderr -- under both lowerings. A binary
/// carries no source, so comparing it against `run` is how a binary is checked
/// at all; `run_goldens.rs` pins the same goldens' status, stdout and stderr
/// against their `.toml` files, so equality here also holds against a
/// recorded answer.
/// Under the reference lowering this also takes the encoder and decoder over
/// code with no jumps and many registers.
///
/// A golden recorded as failing is swept too, and deliberately: `build` never
/// instantiates, so a program that raises still builds, and the `load` that
/// runs it is where the deployment artifact's *error* path -- the shared
/// uncaught-error report, read out of a binary rather than a source -- is
/// pinned against `run`.
fn a_binary_runs_as_its_source_does(path: &Path, lowering: &str) -> Result<(), Failed> {
    let directory = ScratchDirectory::new("golden-binary");
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let out = directory.join(&format!("{stem}.{lowering}.nsb"));
    let (built, _, build_error) = ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        out.as_os_str(),
        "--lowering".as_ref(),
        lowering.as_ref(),
    ]);
    if !built {
        // A golden whose root the root rule forbids cannot be built
        // (`image::encode_describing`); `run` must then fail on the same
        // defect, at instantiation.
        if build_error.contains("a module's root class may inherit only") {
            let (run_ok, _, run_error) = ns_next(&[
                "run".as_ref(),
                path.as_os_str(),
                "--lowering".as_ref(),
                lowering.as_ref(),
            ]);
            let tail = build_error
                .split("a module's root class")
                .nth(1)
                .ok_or("the build error names the rule")?;
            if run_ok || !run_error.contains(tail.trim_end()) {
                return Err(
                    format!("run must fail on the defect the build refused: {run_error}").into(),
                );
            }
            return Ok(());
        }
        // A golden whose root class declares no `main:args:` and needs an
        // explicit entry is not a failure of this property. Anything else
        // is, so the reason is checked rather than assumed.
        if build_error.contains("--entry") {
            return Ok(());
        }
        return Err(format!("build failed: {build_error}").into());
    }
    let (run_ok, run_out, run_error) = ns_next(&[
        "run".as_ref(),
        path.as_os_str(),
        "--lowering".as_ref(),
        lowering.as_ref(),
    ]);
    let (load_ok, load_out, load_error) = ns_next(&["load".as_ref(), out.as_os_str()]);
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

/// Every golden, snapshotted and reloaded. That is the strongest property
/// the image has -- the image's own bytecode, its own object graph,
/// and a freshly bootstrapped world reproduce a plain `run` byte for byte
/// -- and it is asserted per golden rather than argued.
///
/// The goldens recorded as failing are swept too. `snapshot` without `--send`
/// only instantiates, so a program that raises in its entry still yields an
/// image, and the raise happens on the `load` that runs it: that is where the
/// image's own code is pinned to report the uncaught error exactly as `run`
/// does.
fn a_snapshot_reproduces_what_run_prints(name: &str, source: &Path) -> Result<(), Failed> {
    let (ok, _out, error) = assert_snapshot_load_matches_run(name, source);
    // Not "the run succeeded" -- a golden recorded as failing is swept too,
    // and the recorded answer is what says which it is.
    if ok != common::expectation_of(source).succeeds() {
        return Err(format!("run did not end as its recorded status says: {error}").into());
    }
    Ok(())
}
