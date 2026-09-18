//! `ns-next snapshot` then `ns-next load`, end to end through the binary.

use indoc::indoc;
mod common;
use common::{golden, ns_next, ScratchDirectory};
use std::path::Path;

#[test]
fn a_snapshot_taken_after_the_entry_send_loads_and_reruns_it_identically() {
    let directory = ScratchDirectory::new("arithmetic");
    let image = directory.join("world.img");
    let source = golden("arithmetic");
    let (ok, out, error) = ns_next(&[
        "snapshot".as_ref(),
        source.as_os_str(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert!(
        out.starts_with("42\n"),
        "the entry send ran while snapshotting: {out}"
    );
    assert!(image.exists());
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out, "42\n",
        "the loaded instance's slot still holds 6 * 7; nothing else is printed"
    );
}

#[test]
fn a_loaded_world_does_not_rerun_its_instance_initializer() {
    let directory = ScratchDirectory::new("instance-init");
    let image = directory.join("world.img");
    let source = golden("instance_init");
    let (ok, out, error) = ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    assert!(ok, "{error}");
    assert!(out.starts_with("init ran\n"), "{out}");
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out, "hi 2\n",
        "slots (including the transcript stream object) survived; the initializer did not run again"
    );
}

/// A world whose hidden nested-class cache slots hold class objects, and
/// whose `outer` sends resolve two levels out through a declaration chain
/// that is itself inherited across a nesting boundary. Loading one has to
/// rebuild every class object's application and its `declaring_class`
/// link, and `top` four
/// times over is what says both came back right -- `middle` is the decoy a
/// walk down the receiver's dynamic chain would answer with.
#[test]
fn a_world_holding_nested_class_objects_snapshots_and_reruns_identically() {
    let directory = ScratchDirectory::new("nested-outer-inherited");
    let image = directory.join("world.img");
    let source = golden("nested_outer_inherited");
    let expected = common::expectation_of(&golden("nested_outer_inherited")).stdout;
    let (ok, out, error) = ns_next(&[
        "snapshot".as_ref(),
        source.as_os_str(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    // `snapshot` adds its own "wrote <path>" line after the entry send's
    // output, which `load` does not.
    assert!(out.starts_with(&expected), "{out}");
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(out, expected);
}

#[test]
fn load_without_send_prints_a_summary_and_a_corrupt_image_is_reported() {
    let directory = ScratchDirectory::new("summary");
    let image = directory.join("world.img");
    let source = golden("hello");
    let (ok, _out, error) = ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    assert!(ok, "{error}");
    let (ok, out, _error) = ns_next(&["load".as_ref(), image.as_os_str()]);
    assert!(ok);
    assert!(out.contains("1 root value(s)"), "{out}");
    let mut bytes = std::fs::read(&image).unwrap();
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    std::fs::write(&image, bytes).unwrap();
    let (ok, _out, error) = ns_next(&["load".as_ref(), image.as_os_str()]);
    assert!(!ok);
    assert!(error.contains("version"), "{error}");
}

/// One source, snapshotted and reloaded: everything `run` produces --
/// stdout, stderr and exit status -- is exactly what the initializer
/// produces while snapshotting plus what the entry send produces after a
/// load. Returns the three `run` outputs so a caller can assert what the
/// program itself did.
fn assert_snapshot_load_matches_run(name: &str, source: &Path) -> (bool, String, String) {
    let directory = ScratchDirectory::new(name);
    let image = directory.join("world.img");
    let (run_ok, run_out, run_error) = ns_next(&["run".as_ref(), source.as_os_str()]);
    let (snapshot_ok, snapshot_out, snapshot_error) =
        ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    let (load_ok, load_out, load_error) = if snapshot_ok {
        ns_next(&[
            "load".as_ref(),
            image.as_os_str(),
            "--send".as_ref(),
            "main:args:".as_ref(),
        ])
    } else {
        // `snapshot` failed the same way `run` did (the entry send raised),
        // so there is no image and nothing further to compare: its own
        // streams carry the whole outcome.
        (snapshot_ok, String::new(), String::new())
    };
    // `snapshot`'s own last line ("wrote <path>") is the command's report,
    // not the program's output; everything before it is what the module
    // printed while being instantiated.
    let printed_while_snapshotting = snapshot_out
        .strip_suffix(&format!("wrote {}\n", image.display()))
        .unwrap_or(&snapshot_out)
        .to_string();
    assert_eq!(
        format!("{printed_while_snapshotting}{load_out}"),
        run_out,
        "{name}: snapshot-then-load must print exactly what `run` prints"
    );
    // stderr too: `report_entry_send` is shared, so an `Uncaught Newspeak
    // error:` line must come out identical on both paths -- comparing only
    // stdout would let the two diverge on exactly the messages the goldens'
    // recorded `stderr` pins byte for byte.
    assert_eq!(
        format!("{snapshot_error}{load_error}"),
        run_error,
        "{name}: snapshot-then-load must report exactly what `run` reports"
    );
    assert_eq!(
        snapshot_ok && load_ok,
        run_ok,
        "{name}: snapshot-then-load must succeed exactly when `run` does"
    );
    (run_ok, run_out, run_error)
}

/// Every fixture, snapshotted and reloaded. That is the strongest property
/// the image has -- the image's own bytecode, its own object graph,
/// and a freshly bootstrapped world reproduce a plain `run` byte for byte
/// -- and it is asserted per fixture rather than argued.
#[test]
fn a_snapshot_and_a_load_reproduce_exactly_what_run_prints() {
    let mut checked = 0;
    for source in common::golden_sources() {
        // A golden that is recorded as failing has nothing to round trip:
        // this sweep asserts that a snapshot answers what the source
        // answered, and a program that raises answers nothing.
        if !common::expectation_of(&source).succeeds() {
            continue;
        }
        let name = source
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .to_string();
        let (ok, _out, error) = assert_snapshot_load_matches_run(&name, &source);
        assert!(ok, "run {name} exited non-zero: {error}");
        checked += 1;
    }
    assert!(checked >= 3, "expected at least three fixtures");
}

/// The failing direction of the same property, on an inline source rather
/// than a golden. `a_snapshot_and_a_load_reproduce_exactly_what_run_prints`,
/// above, skips every golden whose recorded `status` is non-zero
/// (`Expectation::succeeds`): a program that raises leaves no image behind
/// for `snapshot` to write, so round-tripping one proves nothing. This test
/// covers that direction directly, on a source built to raise, rather than
/// relying on one of the goldens recorded as failing. The uncaught-error
/// line is written by the shared `report_entry_send` on both paths; this is
/// what pins that it stays shared.
#[test]
fn a_load_reports_an_uncaught_error_exactly_as_run_does() {
    let directory = ScratchDirectory::new("raises");
    let source = directory.join("raises.ns");
    std::fs::write(
        &source,
        indoc! {"
            Newspeak3
            'T'
            class Raises usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    ^nil frobnicate
                )
            ) : ()
        "},
    )
    .expect("write source");
    let (ok, out, error) = assert_snapshot_load_matches_run("raises", &source);
    assert!(!ok, "the program raises, so `run` exits non-zero");
    assert_eq!(out, "", "it prints nothing before raising");
    assert!(
        error.starts_with("Uncaught Newspeak error: "),
        "the error text is what both paths must agree on: {error}"
    );
}

/// A version-matching image whose body has been corrupted is a reported
/// error, never a panic and never a stack trace: `load` alone runs no
/// Newspeak code, so every byte flip below is answered by the decoder or
/// by one of the two verification passes.
#[test]
fn no_corruption_of_a_version_matching_image_reaches_a_panic() {
    let directory = ScratchDirectory::new("corruption");
    let image = directory.join("world.img");
    let source = golden("blocks_and_return");
    let (ok, _out, error) = ns_next(&[
        "snapshot".as_ref(),
        source.as_os_str(),
        image.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    let original = std::fs::read(&image).expect("read image");
    // Past the 8-byte header (magic + version, both covered above), every
    // 7th byte, each flipped two ways: enough coverage to sweep every
    // section without turning one test into thousands of processes.
    let (mut swept, mut rejected) = (0, 0);
    for offset in (8..original.len()).step_by(7) {
        for delta in [1u8, 0x80] {
            let mut bytes = original.clone();
            bytes[offset] ^= delta;
            std::fs::write(&image, &bytes).expect("write image");
            let (ok, _out, error) = ns_next(&["load".as_ref(), image.as_os_str()]);
            assert!(
                !error.contains("panicked") && !error.contains("RUST_BACKTRACE"),
                "byte {offset} ^ {delta:#x} panicked the loader: {error}"
            );
            assert!(
                ok || error.starts_with("error: "),
                "byte {offset} ^ {delta:#x} failed without an error message: {error}"
            );
            swept += 1;
            rejected += usize::from(!ok);
        }
    }
    // Many flips land in a field that is still self-consistent (a literal's
    // value, an identity hash), so most of the sweep is expected to load;
    // this only pins that the sweep is really reaching the checks rather
    // than passing vacuously.
    assert!(
        swept > 100 && rejected * 4 >= swept,
        "{rejected} of {swept} corrupted images were rejected"
    );
}

/// The image `snapshot` writes by default carries no program text, and the
/// world in it still runs. `--debug-source` opts the source back in, and
/// `load --print-source` is what reads it out again -- which is how an
/// image can be exported back to `.ns` while no decompiler exists.
#[test]
fn snapshot_is_source_free_unless_debug_source_is_asked_for() {
    let source = golden("arithmetic");
    let text = std::fs::read_to_string(&source).expect("read fixture");
    // Every line carrying program text, not one hand-picked needle: a
    // partial leak (one method body, one initializer statement) has to fail
    // this too. Lines under 12 characters are structural punctuation --
    // `)`, `) : ()`, `Newspeak3` -- which a binary stream reproduces by
    // chance, so checking them would report leaks that are not leaks.
    // See `no_line_of_the_source_survives_in_a_source_free_image` in
    // `ns-runtime`'s image tests for the same property with a module built
    // to probe it, and for what this does *not* claim (identifiers survive
    // in the symbol table by necessity -- dispatch is by name).
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.len() >= 12)
        .collect();
    assert!(lines.len() >= 4, "the fixture must have text to look for");

    let plain_directory = ScratchDirectory::new("source-free");
    let plain = plain_directory.join("world.img");
    let (ok, _out, error) = ns_next(&["snapshot".as_ref(), source.as_os_str(), plain.as_os_str()]);
    assert!(ok, "{error}");
    let plain_bytes = std::fs::read(&plain).expect("read image");
    for line in &lines {
        assert!(
            !contains(&plain_bytes, line.as_bytes()),
            "the default image contains the source line {line:?}"
        );
    }
    // ...and it is still a world, not just a smaller file.
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        plain.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
    // A source-free image has nothing to print, and says so.
    let (ok, _out, error) = ns_next(&[
        "load".as_ref(),
        plain.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("carries no source"), "{error}");

    let debug_directory = ScratchDirectory::new("debug-source");
    let debug = debug_directory.join("world.img");
    let (ok, _out, error) = ns_next(&[
        "snapshot".as_ref(),
        source.as_os_str(),
        debug.as_os_str(),
        "--debug-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    let debug_bytes = std::fs::read(&debug).expect("read image");
    // The control: the very lines missing above are all present here, so
    // the sweep cannot be passing because the needles were mangled.
    for line in &lines {
        assert!(
            contains(&debug_bytes, line.as_bytes()),
            "the debug variant is missing the source line {line:?}"
        );
    }
    assert!(
        debug_bytes.len() > plain_bytes.len(),
        "the debug variant is the bigger of the two"
    );
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        debug.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out.replace("\r\n", "\n"),
        text.replace("\r\n", "\n"),
        "the debug variant round-trips the module's source verbatim"
    );
    // And the debug variant still runs, exactly as the plain one does.
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        debug.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

const TAG_NAMED_HASHES: &str = indoc! {"
    Newspeak3
    'Test'
    class TagNamedHashes usingPlatform: p = Object new (
        | integerHash = Integer hash. classHash = self class hash. platformHash = p hash. selfHash = self hash. |
    ) (
        public check: q = (
            q transcript show: (Integer hash = integerHash) printString; newLine.
            q transcript show: (self class hash = classHash) printString; newLine.
            q transcript show: (q hash = platformHash) printString; newLine
        )
    ) : ()
"};

/// The slot initializers ask a builtin class, the root class and the
/// platform for their hashes while the world is built; the load answers the
/// same three questions against the objects it rebuilt from tags. Before the
/// image recorded tag-named hashes, all three printed `false`.
///
/// `selfHash` is asked for and never checked: it exists only to give the
/// identity-hash counter a fourth, ordinary record to land on after the
/// three tag-named draws. Without it, this module's three hash requests are
/// the very first the process ever makes -- both while snapshotting and
/// again after the load -- so the counter restarts at the same value both
/// times and the stale bug drew the same numbers back by coincidence
/// (confirmed against the pre-fix parent commit: with `selfHash` this test
/// prints `false` three times there, without it it passes anyway). See
/// "Identity hashes are preserved verbatim" in image-format.md for the
/// counter this exploits.
#[test]
fn a_hash_given_to_a_tag_named_object_survives_a_snapshot() {
    let directory = ScratchDirectory::new("tag-named-hashes");
    let image = directory.join("world.img");
    let source = directory.join("tag_named_hashes.ns");
    std::fs::write(&source, TAG_NAMED_HASHES).expect("write source");
    let (ok, _, error) = ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    assert!(ok, "{error}");
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        image.as_os_str(),
        "--send".as_ref(),
        "check:".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(out, "true\ntrue\ntrue\n");
}
