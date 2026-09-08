//! `ns-next inspect` -- the structural listing of an artifact.

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
    let dir =
        std::env::temp_dir().join(format!("ns-next-inspect-{}-{n}-{test}", std::process::id()));
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
fn inspect_reports_the_sections_symbols_and_mixins_of_an_image() {
    let dir = temp_dir("arithmetic");
    let img = dir.join("world.img");
    let (ok, _, err) = ns_next(&[
        "snapshot".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        img.as_os_str(),
    ]);
    assert!(ok, "{err}");

    let (ok, out, err) = ns_next(&["inspect".as_ref(), img.as_os_str()]);
    assert!(ok, "{err}");

    assert!(out.contains("format version 1"), "{out}");
    assert!(out.contains("stateless: no"), "{out}");
    assert!(out.contains("source blob: absent"), "{out}");
    for section in [
        "header",
        "origin",
        "symbols",
        "code",
        "mixins",
        "root class",
        "entry",
        "graph",
    ] {
        assert!(out.contains(section), "missing section `{section}`:\n{out}");
    }
    // The module's own entry selector is in the symbol table because every
    // send instruction names its selector.
    assert!(out.contains("main:args:"), "{out}");
}

#[test]
fn inspect_reports_a_debug_variants_source_blob() {
    let dir = temp_dir("debug-source");
    let img = dir.join("world.img");
    let (ok, _, err) = ns_next(&[
        "snapshot".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        img.as_os_str(),
        "--debug-source".as_ref(),
    ]);
    assert!(ok, "{err}");

    let (ok, out, err) = ns_next(&["inspect".as_ref(), img.as_os_str()]);
    assert!(ok, "{err}");
    assert!(out.contains("source blob: present"), "{out}");
}

#[test]
fn inspect_reports_a_corrupt_artifact_rather_than_panicking() {
    let dir = temp_dir("corrupt");
    let bad = dir.join("bad.img");
    std::fs::write(&bad, b"NSIM\x01\x00\x00\x00").expect("write");

    let (ok, _, err) = ns_next(&["inspect".as_ref(), bad.as_os_str()]);
    assert!(!ok);
    assert!(err.starts_with("error:"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

/// The section line says how many bodies a code table holds; it did not say
/// which. That gap is what made the output hard to follow: a reader could see
/// that four bodies existed but not what to ask `disasm` for. The mixin table
/// already maps selector to code id, so naming them costs no new decoding.
#[test]
fn inspect_names_every_body_in_the_code_table() {
    let dir = temp_dir("code-table");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("arithmetic.ns").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{err}");

    let (ok, listing, err) = ns_next(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{err}");

    assert!(listing.contains("code table"), "{listing}");
    // arithmetic.ns declares one method and one slot initializer on a root
    // class whose superclass clause is `Object new`.
    assert!(listing.contains("main:args:"), "{listing}");
    assert!(listing.contains("initializer"), "{listing}");
    assert!(listing.contains("superclass clause"), "{listing}");
    // Every body must be accounted for -- an unnamed row would be exactly the
    // gap this closes.
    assert!(
        !listing.contains("  (unnamed)"),
        "every code id should be attributed:\n{listing}"
    );
}
