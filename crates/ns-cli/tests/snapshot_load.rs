//! `ns-next snapshot` then `ns-next load`, end to end through the binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-fixtures")
        .join(name)
}

fn temp_image(test: &str) -> PathBuf {
    // The counter, not just the test name, is what keeps two tests -- which
    // cargo runs as threads of one process, so with one pid -- from
    // deleting each other's directory when they snapshot the same fixture.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ns-next-image-{}-{n}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("world.img")
}

fn ns_next(args: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .args(args)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

#[test]
fn a_snapshot_taken_after_the_entry_send_loads_and_reruns_it_identically() {
    let img = temp_image("arithmetic");
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
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}

#[test]
fn a_loaded_world_does_not_rerun_its_instance_initializer() {
    let img = temp_image("instance-init");
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
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}

#[test]
fn load_without_send_prints_a_summary_and_a_corrupt_image_is_reported() {
    let img = temp_image("summary");
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
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}

/// Every fixture, snapshotted and reloaded: what `run` prints is exactly
/// what the initializer prints while snapshotting plus what the entry send
/// prints after a load. That is the strongest property this milestone has
/// -- the image's own bytecode, its own object graph, and a freshly
/// bootstrapped world reproduce a plain `run` byte for byte -- and it is
/// asserted per fixture rather than argued.
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
        let img = temp_image(&name);
        let (ok, run_out, err) = ns_next(&["run".as_ref(), src.as_os_str()]);
        assert!(ok, "run {name}: {err}");
        let (ok, snapshot_out, err) =
            ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
        assert!(ok, "snapshot {name}: {err}");
        let (ok, load_out, err) = ns_next(&[
            "load".as_ref(),
            img.as_os_str(),
            "--send".as_ref(),
            "main:args:".as_ref(),
        ]);
        assert!(ok, "load {name}: {err}");
        // `snapshot`'s own last line ("wrote <path>") is the command's
        // report, not the program's output; everything before it is what
        // the module printed while being instantiated.
        let printed_while_snapshotting = snapshot_out
            .strip_suffix(&format!("wrote {}\n", img.display()))
            .unwrap_or_else(|| {
                panic!("{name}: snapshot did not report where it wrote: {snapshot_out}")
            });
        assert_eq!(
            format!("{printed_while_snapshotting}{load_out}"),
            run_out,
            "{name}: snapshot-then-load must print exactly what `run` prints"
        );
        let _ = std::fs::remove_dir_all(img.parent().unwrap());
        checked += 1;
    }
    assert!(checked >= 3, "expected at least three fixtures");
}

/// A version-matching image whose body has been corrupted is a reported
/// error, never a panic and never a stack trace: `load` alone runs no
/// Newspeak code, so every byte flip below is answered by the decoder or
/// by one of the two verification passes.
#[test]
fn no_corruption_of_a_version_matching_image_reaches_a_panic() {
    let img = temp_image("corruption");
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
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}
