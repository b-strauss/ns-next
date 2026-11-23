use std::path::Path;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

/// Both lowerings: the optimized one every program runs under, and the
/// reference lowering it is checked against (`docs/design/compiler.md`, "The
/// reference lowering"). Each must produce the recorded answer.
const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// One test per golden, named by its path below `goldens/`, so that `cargo
/// test` reports each golden on a line of its own, every failure rather than
/// the first, and runs one or a family by name (`cargo nextest run -p ns-cli --test
/// run_goldens lazy`). libtest's own harness cannot make a test per file it
/// finds at run time, which is why this binary is `harness = false`
/// (`libtest-mimic`). Each golden is a separate process run, so the trials
/// run in parallel like any other tests.
fn main() {
    let arguments = Arguments::from_args();
    let mut trials = vec![
        Trial::test(
            "the_guards_on_skipped_refuse_what_they_promise_to",
            the_guards_on_skipped_refuse_what_they_promise_to,
        ),
        Trial::test(
            "every_golden_is_a_complete_pair",
            every_golden_is_a_complete_pair,
        ),
        Trial::test(
            "a_golden_may_name_the_rules_it_checks",
            a_golden_may_name_the_rules_it_checks,
        ),
    ];
    for source in common::golden_programs() {
        let name = common::golden_name(&source);
        let expectation = common::expectation_of(&source);
        // A skipped golden is a recorded intention, not a failure: it is
        // listed as ignored, and `cargo nextest run --run-ignored all` runs it to see
        // whether the gap it records has closed.
        let skipped = expectation.skipped().is_some();
        // One trial per lowering, so nextest runs the two in parallel instead
        // of one trial running both in sequence.
        for lowering in LOWERINGS {
            let checked = source.clone();
            trials.push(
                Trial::test(format!("golden::{lowering}::{name}"), move || {
                    matches_under_one_lowering(&checked, lowering)
                })
                .with_ignored_flag(skipped),
            );
        }
        if cfg!(debug_assertions) {
            // The optimizer's goldens also run with every body requested
            // for optimization at its first spend, which the stress mode
            // is only built to do in a debug build.
            if let Some(optimizer_name) = name.strip_prefix("runtime/optimizer/") {
                let checked = source.clone();
                trials.push(
                    Trial::test(format!("golden::optimizer::{optimizer_name}"), move || {
                        matches_under_the_optimize_stress_mode(&checked)
                    })
                    .with_ignored_flag(skipped),
                );
            }
            trials.push(
                Trial::test(
                    format!("golden_with_a_two_entry_global_cache::{name}"),
                    move || matches_with_a_two_entry_global_cache(&source),
                )
                .with_ignored_flag(skipped),
            );
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

/// A golden's exit status, stdout and stderr match its recorded
/// `Expectation` exactly -- every byte, the final newline included -- under
/// `lowering`.
///
/// The run also reports its collector's statistics, which this takes out
/// of stderr before the comparison and holds to
/// `common::no_program_reaches_a_table_limit`: no golden reaches a table's
/// growth limit, checked without a run of its own.
fn matches_under_one_lowering(source: &Path, lowering: &str) -> Result<(), Failed> {
    let expectation = common::expectation_of(source);
    let mut output = common::target_command(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(source)
        .arg("--lowering")
        .arg(lowering)
        .arg("--collector-statistics")
        .args(common::program_arguments(source))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    let statistics = common::take_collector_statistics(&mut output);
    common::compare(
        &output,
        &expectation,
        &format!("under the {lowering} lowering"),
    )?;
    if let Some(statistics) = common::require_collector_statistics(statistics, &expectation)? {
        common::no_program_reaches_a_table_limit(&statistics, &expectation)?;
    }
    Ok(())
}

/// A golden under `goldens/runtime/optimizer/` answers alike when the
/// optimizing compiler is asked for every body at its first spend
/// (`NS_NEXT_STRESS_OPTIMIZE`, `docs/design/execution.md`), so that what it
/// compiles is held to the interpreter's recorded outcome.
fn matches_under_the_optimize_stress_mode(source: &Path) -> Result<(), Failed> {
    let expectation = common::expectation_of(source);
    let output = common::target_command(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(source)
        .args(common::program_arguments(source))
        .env("NS_NEXT_STRESS_OPTIMIZE", "1")
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(&output, &expectation, "under the optimize stress mode").map_err(Failed::from)
}

/// A golden answers alike when the global cache is one two-slot group, so
/// that every insert contends for the same group
/// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, read by debug builds only). Only the
/// nextest profile `caches` runs these trials, for a commit that touches
/// the caches and at the end of a batch.
fn matches_with_a_two_entry_global_cache(source: &Path) -> Result<(), Failed> {
    let expectation = common::expectation_of(source);
    let output = common::target_command(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(source)
        .args(common::program_arguments(source))
        .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "2")
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(&output, &expectation, "with a two-entry global cache").map_err(Failed::from)
}

/// A golden names the specification's rules it checks on one `rules` line,
/// which `cargo xtask spec` reads and the run ignores; a misspelled key is
/// refused like every unknown one, so a reference never silently vanishes.
fn a_golden_may_name_the_rules_it_checks() -> Result<(), Failed> {
    let expectation = toml::from_str::<common::Expectation>(
        "stdout = \"1\\n\"\nrules = [\"syntax.outer.alone\"]\n",
    )
    .map_err(|error| Failed::from(error.to_string()))?;
    assert_eq!(expectation.rules, ["syntax.outer.alone"]);
    assert_eq!(expectation.stdout, "1\n");
    let refused = toml::from_str::<common::Expectation>("rule = [\"syntax.outer.alone\"]\n");
    assert!(refused.is_err(), "`rule` is no key of an expectation");
    Ok(())
}

/// The two guards on `skipped` itself, which nothing else exercises. Either
/// failure would take a golden out of a sweep without saying so: a reason
/// left blank in a hurry leaves nothing behind that says what would switch
/// the golden back on, and a misspelled key would be ignored rather than
/// refused, so an expectation would quietly stop being checked.
fn the_guards_on_skipped_refuse_what_they_promise_to() -> Result<(), Failed> {
    let refused = toml::from_str::<common::Expectation>(
        "statuss = 1
",
    );
    assert!(
        refused.is_err(),
        "an unknown key must be refused, not ignored"
    );

    let directory = common::ScratchDirectory::new("blank-skip-reason");
    let source = directory.join("blank.ns");
    std::fs::write(&source, "").expect("write source");
    std::fs::write(
        source.with_extension("toml"),
        "skipped = \"   \"
",
    )
    .expect("write expectation");
    // The refusal is a panic, and its message would otherwise be printed by
    // the default hook and read as a failure of this test.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let refused = std::panic::catch_unwind(|| common::expectation_of(&source));
    std::panic::set_hook(hook);
    assert!(refused.is_err(), "a blank `skipped` must be refused");
    Ok(())
}

/// Every golden is a pair, checked in both directions. The floor this
/// replaced counted what the walk found, which cannot detect the thing it
/// was there to detect: a golden that stops being discovered because it was
/// renamed, moved or given another extension. An orphan on either side is
/// exactly that event, and it fails here.
fn every_golden_is_a_complete_pair() -> Result<(), Failed> {
    let mut missing_expectation = Vec::new();
    for source in common::golden_programs() {
        let expectation = source.with_extension("toml");
        if !expectation.exists() {
            missing_expectation.push(source.display().to_string());
        }
    }
    let mut orphaned_expectation = Vec::new();
    for path in common::golden_files("toml") {
        if !path.with_extension("ns").exists()
            && !path.with_extension("").join(common::PROJECT_FILE).exists()
        {
            orphaned_expectation.push(path.display().to_string());
        }
    }
    if missing_expectation.is_empty() && orphaned_expectation.is_empty() {
        return Ok(());
    }
    Err(format!(
        "goldens without an expectation: {missing_expectation:?}; \
         expectations without a golden: {orphaned_expectation:?}"
    )
    .into())
}
