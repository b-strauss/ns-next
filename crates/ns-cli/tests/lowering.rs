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

/// Whether an artifact's code table holds a branch, which only control-flow
/// inlining emits.
fn has_branch(artifact: &std::path::Path) -> bool {
    let (ok, listing, err) =
        ns_next(&["inspect".as_ref(), artifact.as_os_str(), "--full".as_ref()]);
    assert!(ok, "{err}");
    listing.contains("Jump") || listing.contains("Branch")
}

/// The harnesses compare the two lowerings through `run`, `build` and
/// `snapshot`. A command that dropped `--lowering` would compile the optimized
/// code under both names, every comparison would pass, and nothing would say
/// so -- no program's output depends on the lowering, by design. What does
/// depend on it is the code an artifact carries, so these tests read that.
/// `run` writes no artifact; it is covered because it hands the flag to the
/// same `source::bootstrap` call `build` and `snapshot` do, which is the one
/// place a source is compiled.
#[test]
fn build_compiles_with_the_lowering_it_is_given() {
    let dir = ScratchDir::new("lowering-build");
    let src = fixture("while_loop.ns");
    let optimized = dir.join("optimized.nsb");
    let reference = dir.join("reference.nsb");
    let (ok, _, err) = ns_next(&["build".as_ref(), src.as_os_str(), optimized.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        src.as_os_str(),
        reference.as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert!(has_branch(&optimized), "the optimized loop is inlined");
    assert!(
        !has_branch(&reference),
        "the reference lowering inlines nothing"
    );
}

/// The same for a snapshot, which a loaded world then runs exactly as `run`
/// does under the same lowering.
#[test]
fn snapshot_compiles_with_the_lowering_it_is_given() {
    let dir = ScratchDir::new("lowering-snapshot");
    let src = fixture("while_loop.ns");
    let optimized = dir.join("optimized.img");
    let reference = dir.join("reference.img");
    let (ok, _, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), optimized.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&[
        "snapshot".as_ref(),
        src.as_os_str(),
        reference.as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert!(has_branch(&optimized), "the optimized loop is inlined");
    assert!(
        !has_branch(&reference),
        "the reference lowering inlines nothing"
    );

    let loaded = ns_next(&[
        "load".as_ref(),
        reference.as_os_str(),
        "--send".as_ref(),
        "main:args:".as_ref(),
    ]);
    let ran = ns_next(&[
        "run".as_ref(),
        src.as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ran.0, "{}", ran.2);
    assert_eq!(loaded, ran, "load of the reference snapshot against run");
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
