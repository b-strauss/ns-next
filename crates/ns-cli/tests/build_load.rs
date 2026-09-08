//! `ns-next build` -- the stateless deployment binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-fixtures")
        .join(name)
}

fn temp_dir(test: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ns-next-build-{}-{n}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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
fn build_writes_a_stateless_binary_that_inspect_recognizes() {
    let dir = temp_dir("build");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{err}");

    let (ok, listing, err) = ns_next(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{err}");
    assert!(listing.contains("stateless: yes"), "{listing}");
    assert!(listing.contains("main:args:"), "{listing}");
    // The whole point of the artifact: no instance state travelled with it.
    assert!(!listing.contains("graph"), "{listing}");
}

#[test]
fn build_refuses_an_entry_the_root_class_does_not_have() {
    let dir = temp_dir("bad-entry");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "noSuchSelector".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("noSuchSelector"), "{err}");
    assert!(!out.exists(), "a refused build must write no file");
}

#[test]
fn build_refuses_an_entry_arity_the_cli_cannot_supply() {
    let dir = temp_dir("bad-arity");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "a:b:c:".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("argument"), "{err}");
}

#[test]
fn build_requires_an_entry_when_the_root_class_declares_no_main() {
    // TestActor.ns answers `return42` and declares no `main:args:`.
    let dir = temp_dir("no-main");
    let out = dir.join("app.nsb");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(!ok);
    assert!(err.contains("--entry"), "{err}");
}

#[test]
fn a_built_binary_runs_exactly_as_its_source_does() {
    let dir = temp_dir("equivalence");
    let out = dir.join("app.nsb");
    let src = fixture("arithmetic.ns");

    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(ok, "{err}");

    let (run_ok, run_out, run_err) = ns_next(&["run".as_ref(), src.as_os_str()]);
    let (load_ok, load_out, load_err) = ns_next(&["load".as_ref(), out.as_os_str()]);

    assert_eq!(load_ok, run_ok, "exit status differs");
    assert_eq!(load_out, run_out, "stdout differs");
    assert_eq!(load_err, run_err, "stderr differs");
}

#[test]
fn loading_a_binary_runs_its_initializers_against_the_loading_platform() {
    // instance_init.ns writes through its initializer, so if the factory and
    // initializers did not run on load the entry send would see nil slots.
    let dir = temp_dir("init");
    let out = dir.join("app.nsb");
    let src = fixture("instance_init.ns");
    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), out.as_os_str()]);
    assert!(ok, "{err}");

    let (run_ok, run_out, _) = ns_next(&["run".as_ref(), src.as_os_str()]);
    let (load_ok, load_out, _) = ns_next(&["load".as_ref(), out.as_os_str()]);
    assert_eq!((load_ok, load_out), (run_ok, run_out));
}

/// `build` then `load` must be indistinguishable from `run` on the same
/// source: exit status, stdout and stderr.
///
/// This is the binary's substitute for the two-engine differential check,
/// which it structurally cannot have -- a binary has no AST, so the
/// tree-walker cannot run it. Because the differential harness already pins
/// bytecode against tree-walk over these same fixtures, equality here holds
/// transitively against the tree-walker too.
#[test]
fn every_fixture_runs_identically_from_source_and_from_a_binary() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let dir = temp_dir("all-fixtures");
    let mut checked = 0;

    for entry in std::fs::read_dir(&fixtures).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let out = dir.join(format!(
            "{}.nsb",
            path.file_stem().unwrap().to_string_lossy()
        ));

        let (built, _, build_err) = ns_next(&["build".as_ref(), path.as_os_str(), out.as_os_str()]);
        if !built {
            // A fixture whose root class declares no `main:args:` and needs an
            // explicit entry is not a failure of this property. Anything else
            // is, so the reason is asserted rather than assumed.
            assert!(
                build_err.contains("--entry"),
                "build failed for {}: {build_err}",
                path.display()
            );
            continue;
        }

        let (run_ok, run_out, run_err) = ns_next(&["run".as_ref(), path.as_os_str()]);
        let (load_ok, load_out, load_err) = ns_next(&["load".as_ref(), out.as_os_str()]);
        assert_eq!(
            load_ok,
            run_ok,
            "exit status differs for {}",
            path.display()
        );
        assert_eq!(load_out, run_out, "stdout differs for {}", path.display());
        assert_eq!(load_err, run_err, "stderr differs for {}", path.display());
        checked += 1;
    }

    assert!(
        checked >= 5,
        "expected several fixtures to be covered, got {checked}"
    );
}

/// For one source and one --debug-source setting, a build and a snapshot must
/// agree byte for byte from `origin` through `root class`.
///
/// Not "the binary is a prefix of the image" -- that is false as soon as one
/// records an entry and the other does not. The guarantee is about the
/// sections that describe code and structure, and it is what keeps the shared
/// section writers honest.
#[test]
fn a_build_and_a_snapshot_agree_on_the_describing_sections() {
    let dir = temp_dir("prefix");
    let src = fixture("arithmetic.ns");
    let bin = dir.join("app.nsb");
    let img = dir.join("world.img");

    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), bin.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");

    let bin_bytes = std::fs::read(&bin).expect("read binary");
    let img_bytes = std::fs::read(&img).expect("read image");

    // The window starts at `origin`, not at byte 0: the header's flag byte
    // legitimately differs (a build is stateless, a snapshot is not), so
    // starting from the header would fail on that difference instead of on
    // the property this test actually guards.
    let window = |bytes: &[u8]| {
        let report = ns_runtime::image::inspect::inspect(bytes).expect("inspect");
        let start = report
            .sections
            .iter()
            .find(|s| s.name == "origin")
            .expect("origin section")
            .start;
        let end = report
            .sections
            .iter()
            .find(|s| s.name == "root class")
            .expect("root class section")
            .end;
        (start, end)
    };
    let (bin_start, bin_end) = window(&bin_bytes);
    let (img_start, img_end) = window(&img_bytes);
    assert_eq!(
        (bin_start, bin_end),
        (img_start, img_end),
        "the describing sections start or end at different offsets"
    );
    assert_eq!(
        &bin_bytes[bin_start..bin_end],
        &img_bytes[img_start..img_end],
        "describing bytes differ; run `ns-next inspect` on both to see which section"
    );
}

#[test]
fn load_send_overrides_the_recorded_entry() {
    // The recorded entry (`return42`) and the `--send` override
    // (`signalError`) must be *different* selectors with observably
    // different outcomes: `return42` exits cleanly, `signalError` sends
    // `Error signal:` and fails. Using the same selector on both sides (as
    // an earlier version of this test did) would pass whether or not
    // `--send` actually takes precedence -- exactly the failure class an
    // earlier review caught in another file.
    let dir = temp_dir("override");
    let out = dir.join("app.nsb");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        src.as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "return42".as_ref(),
    ]);
    assert!(ok, "{err}");

    // If `--send` did not override the recorded entry, this would silently
    // run `return42` instead and exit successfully with no error message --
    // so both the failing exit and the message are asserted, not just one.
    let (ok, _, err) = ns_next(&[
        "load".as_ref(),
        out.as_os_str(),
        "--send".as_ref(),
        "signalError".as_ref(),
    ]);
    assert!(
        !ok,
        "expected `--send signalError` to override the recorded `return42` entry"
    );
    assert!(
        err.contains("Uncaught Newspeak error"),
        "expected signalError's error to surface, got: {err}"
    );
}
