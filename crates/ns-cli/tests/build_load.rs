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
