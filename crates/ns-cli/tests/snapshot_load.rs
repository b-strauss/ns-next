//! `ns-next snapshot` then `ns-next load`, end to end through the binary.

mod common;
use common::{fixture, ns_next, ScratchDir};
use std::path::Path;

#[test]
fn a_snapshot_taken_after_the_entry_send_loads_and_reruns_it_identically() {
    let dir = ScratchDir::new("arithmetic");
    let img = dir.join("world.img");
    let src = fixture("arithmetic.ns");
    let (ok, out, err) = ns_next(&[
        "snapshot".as_ref(),
        src.as_os_str(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert!(
        out.starts_with("42\n"),
        "the entry send ran while snapshotting: {out}"
    );
    assert!(img.exists());
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(
        out, "42\n",
        "the loaded instance's slot still holds 6 * 7; nothing else is printed"
    );
}

#[test]
fn a_loaded_world_does_not_rerun_its_instance_initializer() {
    let dir = ScratchDir::new("instance-init");
    let img = dir.join("world.img");
    let src = fixture("instance_init.ns");
    let (ok, out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");
    assert!(out.starts_with("init ran\n"), "{out}");
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
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
/// times over is what says both came back right -- `mid` is the decoy a
/// walk down the receiver's dynamic chain would answer with.
#[test]
fn a_world_holding_nested_class_objects_snapshots_and_reruns_identically() {
    let dir = ScratchDir::new("nested-outer-inherited");
    let img = dir.join("world.img");
    let src = fixture("nested_outer_inherited.ns");
    let expected = std::fs::read_to_string(fixture("nested_outer_inherited.expected"))
        .expect("read expected")
        .replace("\r\n", "\n");
    let (ok, out, err) = ns_next(&[
        "snapshot".as_ref(),
        src.as_os_str(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    // `snapshot` adds its own "wrote <path>" line after the entry send's
    // output, which `load` does not.
    assert!(out.starts_with(&expected), "{out}");
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(out, expected);
}

#[test]
fn load_without_send_prints_a_summary_and_a_corrupt_image_is_reported() {
    let dir = ScratchDir::new("summary");
    let img = dir.join("world.img");
    let src = fixture("hello.ns");
    let (ok, _out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, out, _err) = ns_next(&["load".as_ref(), img.as_os_str()]);
    assert!(ok);
    assert!(out.contains("1 root value(s)"), "{out}");
    let mut bytes = std::fs::read(&img).unwrap();
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    std::fs::write(&img, bytes).unwrap();
    let (ok, _out, err) = ns_next(&["load".as_ref(), img.as_os_str()]);
    assert!(!ok);
    assert!(err.contains("version"), "{err}");
}

/// One source, snapshotted and reloaded: everything `run` produces --
/// stdout, stderr and exit status -- is exactly what the initializer
/// produces while snapshotting plus what the entry send produces after a
/// load. Returns the three `run` outputs so a caller can assert what the
/// program itself did.
fn assert_snapshot_load_matches_run(name: &str, src: &Path) -> (bool, String, String) {
    let dir = ScratchDir::new(name);
    let img = dir.join("world.img");
    let (run_ok, run_out, run_err) = ns_next(&["run".as_ref(), src.as_os_str()]);
    let (snapshot_ok, snapshot_out, snapshot_err) =
        ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    let (load_ok, load_out, load_err) = if snapshot_ok {
        ns_next(&[
            "load".as_ref(),
            img.as_os_str(),
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
        .strip_suffix(&format!("wrote {}\n", img.display()))
        .unwrap_or(&snapshot_out)
        .to_string();
    assert_eq!(
        format!("{printed_while_snapshotting}{load_out}"),
        run_out,
        "{name}: snapshot-then-load must print exactly what `run` prints"
    );
    // stderr too: `report_entry_send` is shared, so an `Uncaught Newspeak
    // error:` line must come out identical on both paths -- comparing only
    // stdout would let the two diverge on exactly the messages
    // `program_outcomes.rs` pins byte for byte.
    assert_eq!(
        format!("{snapshot_err}{load_err}"),
        run_err,
        "{name}: snapshot-then-load must report exactly what `run` reports"
    );
    assert_eq!(
        snapshot_ok && load_ok,
        run_ok,
        "{name}: snapshot-then-load must succeed exactly when `run` does"
    );
    (run_ok, run_out, run_err)
}

/// Every fixture, snapshotted and reloaded. That is the strongest property
/// the image has -- the image's own bytecode, its own object graph,
/// and a freshly bootstrapped world reproduce a plain `run` byte for byte
/// -- and it is asserted per fixture rather than argued.
#[test]
fn a_snapshot_and_a_load_reproduce_exactly_what_run_prints() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read tests-fixtures") {
        let src = entry.expect("dir entry").path();
        if src.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let name = src.file_stem().expect("stem").to_string_lossy().to_string();
        let (ok, _out, err) = assert_snapshot_load_matches_run(&name, &src);
        assert!(ok, "run {name} exited non-zero: {err}");
        checked += 1;
    }
    assert!(checked >= 3, "expected at least three fixtures");
}

/// The failing direction of the same property, on an inline source rather
/// than a fixture (every `tests-fixtures/*.ns` must exit zero, so a program
/// that raises cannot live there). The uncaught-error line is written by
/// the shared `report_entry_send` on both paths; this is what pins that it
/// stays shared.
#[test]
fn a_load_reports_an_uncaught_error_exactly_as_run_does() {
    let dir = ScratchDir::new("raises");
    let src = dir.join("raises.ns");
    std::fs::write(
        &src,
        "Newspeak3\n'T'\nclass Raises usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t^nil frobnicate\n\t)\n) : ()\n",
    )
    .expect("write source");
    let (ok, out, err) = assert_snapshot_load_matches_run("raises", &src);
    assert!(!ok, "the program raises, so `run` exits non-zero");
    assert_eq!(out, "", "it prints nothing before raising");
    assert!(
        err.starts_with("Uncaught Newspeak error: "),
        "the error text is what both paths must agree on: {err}"
    );
}

/// A version-matching image whose body has been corrupted is a reported
/// error, never a panic and never a stack trace: `load` alone runs no
/// Newspeak code, so every byte flip below is answered by the decoder or
/// by one of the two verification passes.
#[test]
fn no_corruption_of_a_version_matching_image_reaches_a_panic() {
    let dir = ScratchDir::new("corruption");
    let img = dir.join("world.img");
    let src = fixture("blocks_and_return.ns");
    let (ok, _out, err) = ns_next(&[
        "snapshot".as_ref(),
        src.as_os_str(),
        img.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    let original = std::fs::read(&img).expect("read image");
    // Past the 8-byte header (magic + version, both covered above), every
    // 7th byte, each flipped two ways: enough coverage to sweep every
    // section without turning one test into thousands of processes.
    let (mut swept, mut rejected) = (0, 0);
    for offset in (8..original.len()).step_by(7) {
        for delta in [1u8, 0x80] {
            let mut bytes = original.clone();
            bytes[offset] ^= delta;
            std::fs::write(&img, &bytes).expect("write image");
            let (ok, _out, err) = ns_next(&["load".as_ref(), img.as_os_str()]);
            assert!(
                !err.contains("panicked") && !err.contains("RUST_BACKTRACE"),
                "byte {offset} ^ {delta:#x} panicked the loader: {err}"
            );
            assert!(
                ok || err.starts_with("error: "),
                "byte {offset} ^ {delta:#x} failed without an error message: {err}"
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
    let src = fixture("arithmetic.ns");
    let text = std::fs::read_to_string(&src).expect("read fixture");
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
        .filter(|l| l.len() >= 12)
        .collect();
    assert!(lines.len() >= 4, "the fixture must have text to look for");

    let plain_dir = ScratchDir::new("source-free");
    let plain = plain_dir.join("world.img");
    let (ok, _out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), plain.as_os_str()]);
    assert!(ok, "{err}");
    let plain_bytes = std::fs::read(&plain).expect("read image");
    for line in &lines {
        assert!(
            !contains(&plain_bytes, line.as_bytes()),
            "the default image contains the source line {line:?}"
        );
    }
    // ...and it is still a world, not just a smaller file.
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        plain.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(out, "42\n");
    // A source-free image has nothing to print, and says so.
    let (ok, _out, err) = ns_next(&[
        "load".as_ref(),
        plain.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("carries no source"), "{err}");

    let debug_dir = ScratchDir::new("debug-source");
    let debug = debug_dir.join("world.img");
    let (ok, _out, err) = ns_next(&[
        "snapshot".as_ref(),
        src.as_os_str(),
        debug.as_os_str(),
        "--debug-source".as_ref(),
    ]);
    assert!(ok, "{err}");
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
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        debug.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(
        out.replace("\r\n", "\n"),
        text.replace("\r\n", "\n"),
        "the debug variant round-trips the module's source verbatim"
    );
    // And the debug variant still runs, exactly as the plain one does.
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        debug.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(out, "42\n");
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|w| w == needle)
}
