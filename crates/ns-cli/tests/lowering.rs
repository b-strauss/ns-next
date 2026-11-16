//! `--lowering`: choosing the lowering a source is compiled with
//! (`docs/design/compiler.md`, "The reference lowering").

mod common;
use common::{ScratchDirectory, golden, ns_next};

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
/// inlining emits. Read through `ns-runtime`'s inspector, the listing
/// `ns-next-analyzer inspect --full` prints, because a test starts only its
/// own crate's binary.
fn has_closure(artifact: &std::path::Path) -> bool {
    let bytes = std::fs::read(artifact).expect("read the artifact");
    let report = match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => report,
        Err(error) => panic!("{error}"),
    };
    let listing = ns_runtime::image::inspect::render_full(&report);
    listing.contains("MakeClosure")
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
    assert!(!has_closure(&optimized), "the optimized loop is inlined");
    assert!(
        has_closure(&reference),
        "the reference lowering runs the loop's blocks as closures"
    );
}
