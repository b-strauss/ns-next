//! `--lowering`: choosing the lowering a source is compiled with
//! (`docs/design/compiler.md`, "The reference lowering").

mod common;
use common::{golden, ns_next, ScratchDirectory};

/// Only control-flow inlining emits a branch, so a listing under the
/// reference lowering has none, and the optimized one of the same loop does.
#[test]
fn the_reference_lowering_inlines_nothing() {
    let source = golden("language/control-flow/while_loop");
    let (ok, optimized, error) =
        ns_next(&["disasm".as_ref(), source.as_os_str(), "main:args:".as_ref()]);
    assert!(ok, "{error}");
    assert!(optimized.contains("Jump"), "{optimized}");
    let (ok, reference, error) = ns_next(&[
        "disasm".as_ref(),
        source.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert!(!reference.contains("Jump"), "{reference}");
    assert!(!reference.contains("Branch"), "{reference}");
    assert!(reference.contains("#whileTrue:"), "{reference}");
}

#[test]
fn a_golden_runs_under_the_reference_lowering() {
    let (ok, out, error) = ns_next(&[
        "run".as_ref(),
        golden("language/control-flow/while_loop").as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out,
        common::expectation_of(&golden("language/control-flow/while_loop")).stdout
    );
}

/// Whether an artifact's code table holds a branch, which only control-flow
/// inlining emits.
fn has_branch(artifact: &std::path::Path) -> bool {
    let (ok, listing, error) =
        ns_next(&["inspect".as_ref(), artifact.as_os_str(), "--full".as_ref()]);
    assert!(ok, "{error}");
    listing.contains("Jump") || listing.contains("Branch")
}

/// The harnesses compare the two lowerings through `run` and `build`. A
/// command that dropped `--lowering` would compile the optimized
/// code under both names, every comparison would pass, and nothing would say
/// so -- no program's output depends on the lowering, by design. What does
/// depend on it is the code an artifact carries, so these tests read that.
/// `run` writes no artifact; it is covered because it hands the flag to the
/// same `program::load_program` call `build` does, which is the one place a
/// program is compiled.
#[test]
fn build_compiles_with_the_lowering_it_is_given() {
    let directory = ScratchDirectory::new("lowering-build");
    let source = golden("language/control-flow/while_loop");
    let optimized = directory.join("optimized.nsi");
    let reference = directory.join("reference.nsi");
    let (ok, _, error) = ns_next(&["build".as_ref(), source.as_os_str(), optimized.as_os_str()]);
    assert!(ok, "{error}");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        source.as_os_str(),
        reference.as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert!(has_branch(&optimized), "the optimized loop is inlined");
    assert!(
        !has_branch(&reference),
        "the reference lowering inlines nothing"
    );
}

/// An artifact carries the code it was built with, so asking `disasm` to
/// lower it is a mistake, refused rather than silently ignored.
#[test]
fn disasm_refuses_a_lowering_for_an_artifact() {
    let directory = ScratchDirectory::new("lowering-artifact");
    let out = directory.join("app.nsi");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("language/control-flow/while_loop").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{error}");
    let (ok, _, error) = ns_next(&[
        "disasm".as_ref(),
        out.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "optimized".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("--lowering"), "{error}");
}
