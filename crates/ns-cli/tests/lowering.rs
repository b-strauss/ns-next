//! `--lowering`: choosing the lowering a source is compiled with
//! (`docs/design/compiler.md`, "The reference lowering").

mod common;
use common::{fixture, ns_next, ScratchDir};

/// Only control-flow inlining emits a branch, so a listing under the
/// reference lowering has none, and the optimized one of the same loop does.
#[test]
fn the_reference_lowering_inlines_nothing() {
    let src = fixture("while_loop.ns");
    let (ok, optimized, err) =
        ns_next(&["disasm".as_ref(), src.as_os_str(), "main:args:".as_ref()]);
    assert!(ok, "{err}");
    assert!(optimized.contains("Jump"), "{optimized}");
    let (ok, reference, err) = ns_next(&[
        "disasm".as_ref(),
        src.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert!(!reference.contains("Jump"), "{reference}");
    assert!(!reference.contains("Branch"), "{reference}");
    assert!(reference.contains("#whileTrue:"), "{reference}");
}

#[test]
fn a_fixture_runs_under_the_reference_lowering() {
    let (ok, out, err) = ns_next(&[
        "run".as_ref(),
        fixture("while_loop.ns").as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(
        out,
        std::fs::read_to_string(fixture("while_loop.expected"))
            .expect("read expected")
            .replace("\r\n", "\n")
    );
}

/// An artifact carries the code it was built with, so asking `disasm` to
/// lower it is a mistake, refused rather than silently ignored.
#[test]
fn disasm_refuses_a_lowering_for_an_artifact() {
    let dir = ScratchDir::new("lowering-artifact");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("while_loop.ns").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&[
        "disasm".as_ref(),
        out.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "optimized".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("--lowering"), "{err}");
}
