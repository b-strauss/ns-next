//! `--lowering` of `disasm`: choosing the lowering a source is compiled
//! with (`docs/design/compiler.md`, "The reference lowering").

mod common;
use common::{build_image, golden, ns_next_analyzer, ScratchDirectory};

/// Only control-flow inlining emits a branch, so a listing under the
/// reference lowering has none, and the optimized one of the same loop does.
#[test]
fn the_reference_lowering_inlines_nothing() {
    let source = golden("language/control-flow/while_loop");
    let (ok, optimized, error) =
        ns_next_analyzer(&["disasm".as_ref(), source.as_os_str(), "main:args:".as_ref()]);
    assert!(ok, "{error}");
    assert!(optimized.contains("Jump"), "{optimized}");
    let (ok, reference, error) = ns_next_analyzer(&[
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

/// An artifact carries the code it was built with, so asking `disasm` to
/// lower it is a mistake, refused rather than silently ignored.
#[test]
fn disasm_refuses_a_lowering_for_an_artifact() {
    let directory = ScratchDirectory::new("lowering-artifact");
    let out = directory.join("app.nsi");
    std::fs::write(
        &out,
        build_image(
            &golden("language/control-flow/while_loop"),
            ns_runtime::Lowering::Optimized,
            false,
        ),
    )
    .expect("write the image");
    let (ok, _, error) = ns_next_analyzer(&[
        "disasm".as_ref(),
        out.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "optimized".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("--lowering"), "{error}");
}
