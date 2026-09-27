use std::path::Path;
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

/// Normalizes the line endings of what the process wrote. The recorded side
/// is already normalized by `common::expectation_of`, which explains why;
/// this is the same normalization applied to the other side, so that a
/// platform's own line endings can never be mistaken for a difference in
/// program behaviour.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

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
    ];
    for source in common::golden_sources() {
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
fn matches_under_one_lowering(source: &Path, lowering: &str) -> Result<(), Failed> {
    let expectation = common::expectation_of(source);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(source)
        .arg("--lowering")
        .arg(lowering)
        .args(common::program_arguments(source))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    compare(
        &output,
        &expectation,
        &format!("under the {lowering} lowering"),
    )
}

/// A golden answers alike when the global cache is one two-slot group, so
/// that every insert contends for the same group
/// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, read by debug builds only).
fn matches_with_a_two_entry_global_cache(source: &Path) -> Result<(), Failed> {
    let expectation = common::expectation_of(source);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(source)
        .args(common::program_arguments(source))
        .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "2")
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    compare(&output, &expectation, "with a two-entry global cache")
}

/// The one comparison both runs make, reporting which of the three parts
/// differs and how.
fn compare(
    output: &std::process::Output,
    expectation: &common::Expectation,
    condition: &str,
) -> Result<(), Failed> {
    if output.status.code() != Some(expectation.status) {
        return Err(format!(
            "exit status {condition}: expected {}, got {:?}
stderr:
{}",
            expectation.status,
            output.status.code(),
            normalize_newlines(&String::from_utf8_lossy(&output.stderr))
        )
        .into());
    }
    let stdout = normalize_newlines(&String::from_utf8_lossy(&output.stdout));
    if stdout != expectation.stdout {
        return Err(format!(
            "stdout {condition}:
--- expected
{}--- got
{stdout}",
            expectation.stdout
        )
        .into());
    }
    let stderr = normalize_newlines(&String::from_utf8_lossy(&output.stderr));
    if stderr != expectation.stderr {
        return Err(format!(
            "stderr {condition}:
--- expected
{}--- got
{stderr}",
            expectation.stderr
        )
        .into());
    }
    Ok(())
}

/// The two guards on `skipped` itself, which nothing else exercises. Either
/// failure would take a golden out of the suite without saying so: a reason
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
    for source in common::golden_sources() {
        let expectation = source.with_extension("toml");
        if !expectation.exists() {
            missing_expectation.push(source.display().to_string());
        }
    }
    let mut orphaned_expectation = Vec::new();
    for path in common::golden_files("toml") {
        if !path.with_extension("ns").exists() {
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
