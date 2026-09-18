use std::process::Command;

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

/// Every golden's exit status, stdout and stderr match its recorded
/// `Expectation` exactly -- every byte, the final newline included -- under
/// both lowerings.
#[test]
fn every_golden_matches_its_recorded_expectation() {
    let mut ran = 0;
    let mut skipped: Vec<String> = Vec::new();
    for source in common::golden_sources() {
        let expectation = common::expectation_of(&source);
        if let Some(reason) = expectation.skipped() {
            skipped.push(format!("{} -- {reason}", source.display()));
            continue;
        }
        ran += 1;
        for lowering in LOWERINGS {
            let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
                .arg("run")
                .arg(&source)
                .arg("--lowering")
                .arg(lowering)
                .output()
                .expect("run ns-next");
            assert_eq!(
                output.status.code(),
                Some(expectation.status),
                "{} exit status under the {lowering} lowering",
                source.display()
            );
            assert_eq!(
                normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
                expectation.stdout,
                "{} stdout under the {lowering} lowering",
                source.display()
            );
            assert_eq!(
                normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
                expectation.stderr,
                "{} stderr under the {lowering} lowering",
                source.display()
            );
        }
    }
    // Printed rather than asserted: a skipped golden is a recorded
    // intention, not a failure. `cargo test -- --nocapture` is where
    // this line surfaces, since the harness keeps a passing test quiet.
    println!("{ran} goldens ran, {} skipped", skipped.len());
    for entry in &skipped {
        println!("  skipped: {entry}");
    }
}

/// The two guards on `skipped` itself, which nothing else exercises. Either
/// failure would take a golden out of the suite without saying so: a reason
/// left blank in a hurry leaves nothing behind that says what would switch
/// the golden back on, and a misspelled key would be ignored rather than
/// refused, so an expectation would quietly stop being checked.
#[test]
fn the_guards_on_skipped_refuse_what_they_promise_to() {
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
}

/// Every golden is a pair, checked in both directions. The floor this
/// replaced counted what the walk found, which cannot detect the thing it
/// was there to detect: a golden that stops being discovered because it was
/// renamed, moved or given another extension. An orphan on either side is
/// exactly that event, and it fails here.
#[test]
fn every_golden_is_a_complete_pair() {
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
    assert!(
        missing_expectation.is_empty() && orphaned_expectation.is_empty(),
        "goldens without an expectation: {missing_expectation:?}; \
         expectations without a golden: {orphaned_expectation:?}"
    );
}

/// Every golden answers alike when the global cache is one two-slot group,
/// so that every insert contends for the same group
/// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, read by debug builds only).
#[cfg(debug_assertions)]
#[test]
fn every_golden_answers_alike_with_a_two_entry_global_cache() {
    for source in common::golden_sources() {
        let expectation = common::expectation_of(&source);
        if expectation.skipped().is_some() {
            continue;
        }
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&source)
            .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "2")
            .output()
            .expect("run ns-next");
        assert_eq!(
            output.status.code(),
            Some(expectation.status),
            "{} exit status with a two-entry global cache",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            expectation.stdout,
            "{} stdout with a two-entry global cache",
            source.display()
        );
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
            expectation.stderr,
            "{} stderr with a two-entry global cache",
            source.display()
        );
    }
}
