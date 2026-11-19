//! `ns-next-analyzer disasm` snapshot tests -- one per representative code shape:
//! plain keyword method, block with non-local return, inlined
//! ifTrue:ifFalse:, inlined whileTrue:, the non-inlined fallback case, an
//! instance initializer (`docs/design/compiler.md`, "Reading the output"),
//! and the same two shapes under the reference lowering. Catches accidental
//! codegen changes.

use indoc::indoc;
mod common;
use common::ScratchDirectory;

fn disasm(golden: &str, arguments: &[&str]) -> String {
    let path = common::golden(golden);
    let output = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .args(arguments)
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn disasm_arithmetic_main() {
    insta::assert_snapshot!(disasm("library/number/integer/arithmetic", &["main:args:"]));
}

#[test]
fn disasm_arithmetic_initializer() {
    insta::assert_snapshot!(disasm(
        "library/number/integer/arithmetic",
        &["--initializer"]
    ));
}

#[test]
fn disasm_blocks_and_return_find_in() {
    insta::assert_snapshot!(disasm("language/block/blocks_and_return", &["find:in:"]));
}

#[test]
fn disasm_while_loop_main() {
    insta::assert_snapshot!(disasm("language/control-flow/while_loop", &["main:args:"]));
}

#[test]
fn disasm_control_flow_fallback_main() {
    insta::assert_snapshot!(disasm(
        "language/control-flow/control_flow_fallback",
        &["main:args:"]
    ));
}

#[test]
fn disasm_instance_init_initializer() {
    insta::assert_snapshot!(disasm(
        "language/class/instantiation/instance_init",
        &["--initializer"]
    ));
}

#[test]
fn disasm_while_loop_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm(
        "language/control-flow/while_loop",
        &["main:args:", "--lowering", "reference"]
    ));
}

#[test]
fn disasm_control_flow_fallback_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm(
        "language/control-flow/control_flow_fallback",
        &["main:args:", "--lowering", "reference"]
    ));
}

#[test]
fn disasm_rejects_an_unknown_selector_and_conflicting_arguments() {
    let path = common::golden("library/platform/hello");
    let missing = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("nope")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no method `nope`"));
    let both = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("main:args:")
        .arg("--initializer")
        .output()
        .unwrap();
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("exactly one"));
}

/// A module whose language id begins with the image format's magic is still
/// a module: `disasm` tells an artifact from source by more than its first
/// four bytes.
#[test]
fn disasm_reads_a_module_whose_language_id_is_the_image_magic_as_source() {
    let directory = ScratchDirectory::new("disasm-magic-language-id");
    let path = directory.join("magic.ns");
    std::fs::write(
        &path,
        indoc! {"
            NSNX
            class Magic = Object new () (
                public value = ( ^42 )
            )
        "},
    )
    .expect("write temp source");
    let output = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("value")
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--class` addresses a nested class's own method and a class side's own
/// method by mixin path, and reports a clean error (not a panic) for a
/// path segment that names nothing in the tree. Not a golden: this only
/// exercises `disasm`'s addressing and never runs the module, while every
/// golden is run end to end and checked against the outcome recorded in its
/// `.toml` by `run_goldens.rs`.
#[test]
fn disasm_addresses_a_nested_class_and_its_class_side_by_mixin_path() {
    let directory = ScratchDirectory::new("disasm-nested-path");
    let path = directory.join("nested.ns");
    std::fs::write(
        &path,
        indoc! {"
            class Outer usingPlatform: p = Object new () (
                public class Inner = Object new () (
                    public value = ( ^42 )
                )
            ) : (
                public marker = ( ^2 )
            )
        "},
    )
    .expect("write temp source");

    let inner = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("value")
        .arg("--class")
        .arg("Outer.Inner")
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(
        inner.status.success(),
        "{}",
        String::from_utf8_lossy(&inner.stderr)
    );
    assert!(String::from_utf8_lossy(&inner.stdout).contains("method value"));

    let class_side = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("marker")
        .arg("--class")
        .arg("Outer.class")
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(
        class_side.status.success(),
        "{}",
        String::from_utf8_lossy(&class_side.stderr)
    );
    assert!(String::from_utf8_lossy(&class_side.stdout).contains("method marker"));

    let unknown = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&path)
        .arg("value")
        .arg("--class")
        .arg("Outer.Nope")
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("has no nested class `Nope`"));

    let _ = std::fs::remove_file(&path);
}

/// `disasm` of an artifact must print exactly what `disasm` of the source it
/// was built from prints. The artifact carries the same compiled bodies; if
/// these ever differ, the image's code table is not what the compiler emitted.
#[test]
fn disasm_of_an_image_matches_disasm_of_its_source() {
    let source = common::golden("library/number/integer/arithmetic");
    let directory = ScratchDirectory::new("disasm-bin");
    let image = directory.join("app.nsi");

    std::fs::write(
        &image,
        common::build_image(&source, ns_runtime::Lowering::Optimized, false),
    )
    .expect("write the image");

    let from_image = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&image)
        .arg("main:args:")
        .output()
        .expect("run ns-next-analyzer disasm on an image");
    assert!(
        from_image.status.success(),
        "{}",
        String::from_utf8_lossy(&from_image.stderr)
    );

    let from_source = disasm("library/number/integer/arithmetic", &["main:args:"]);
    assert_eq!(
        String::from_utf8_lossy(&from_image.stdout).replace("\r\n", "\n"),
        from_source
    );
}

/// `Bare`'s artifact, its root mixin rewritten by `edit` the way a crafted
/// file would present it: no source compiles to a row missing a body, so the
/// artifact is written through the runtime.
fn bare_artifact(edit: fn(ns_runtime::Mixin) -> ns_runtime::Mixin) -> Vec<u8> {
    use ns_runtime::{Interpreter, SourceOrigin};
    use std::rc::Rc;

    let source = indoc! {"
        class Bare = Object new () (
            public main: p args: a = ( ^0 )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File("bare.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let Ok(instance) = interpreter.instantiate(&class, &[]) else {
        panic!("`Bare new` raises nothing")
    };
    // Nothing runs from here on, so the whole rewrite happens in one entry.
    interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(&class).as_object() else {
            panic!("load_module answers a class object")
        };
        let application = mutator
            .heap()
            .class_application(class)
            .expect("a class object");
        let bare = edit((*application.mixin).clone());
        let rewired = mutator.apply_mixin(&Rc::new(bare), application.superclass.as_ref());
        assert!(mutator.heap_mut().class_set_application(class, rewired));
        let identity = ns_runtime::modules::ModuleIdentity::undeclared("bare.ns", "Bare");
        let modules = [ns_runtime::image::ModuleSource {
            class,
            identity: &identity,
            with_source: false,
        }];
        ns_runtime::image::package(mutator, &modules, mutator.fetch(&instance)).expect("package")
    })
}

/// An artifact whose root row names no own part of its initializer is legal
/// when the class declares no slots and no `initExpr`s -- `verify_mixins`
/// accepts it -- so `disasm --initializer` lists the bodies it does carry.
#[test]
fn disasm_leaves_out_an_own_part_an_artifact_does_not_carry() {
    let bytes = bare_artifact(|mixin| ns_runtime::Mixin {
        own_initializer_code: None,
        ..mixin
    });
    let directory = ScratchDirectory::new("disasm-no-own-part");
    let image = directory.join("bare.nsi");
    std::fs::write(&image, bytes).expect("write artifact");

    let out = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&image)
        .arg("--initializer")
        .output()
        .expect("run ns-next-analyzer disasm");
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(listing.contains("initializer Bare ("), "{listing}");
    assert!(!listing.contains("own part"), "{listing}");
    assert!(listing.contains("method Bare factory new"), "{listing}");
}

/// Every row carries its full initializer, so an artifact whose root row
/// names none is refused when `disasm` loads it -- with a message, not a
/// panic: an artifact is untrusted input.
#[test]
fn disasm_refuses_an_artifact_without_an_instance_initializer() {
    let bytes = bare_artifact(|mixin| ns_runtime::Mixin {
        initializer_code: None,
        ..mixin
    });
    let directory = ScratchDirectory::new("disasm-no-init");
    let image = directory.join("bare.nsi");
    std::fs::write(&image, bytes).expect("write artifact");

    for arguments in [&["--initializer"][..], &["main:args:"][..]] {
        let out = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
            .arg("disasm")
            .arg(&image)
            .args(arguments)
            .output()
            .expect("run ns-next-analyzer disasm");
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!error.contains("panicked"), "disasm panicked: {error}");
        assert_eq!(out.status.code(), Some(1), "stderr: {error}");
        assert!(
            error
                .contains("a class in the image has no compiled code for its instance initializer"),
            "unexpected stderr: {error}"
        );
    }
}

/// A file that is neither parseable source nor a valid artifact must report
/// one clear failure, not a decoder backtrace.
#[test]
fn disasm_rejects_a_corrupt_artifact_with_a_message() {
    let directory = ScratchDirectory::new("disasm-bad");
    let bad = directory.join("truncated.img");
    // Magic and the version's first byte, so it is taken for an artifact,
    // and then nothing.
    let mut truncated = ns_runtime::image::encode::MAGIC.to_vec();
    truncated.push(ns_runtime::image::encode::FORMAT_VERSION.to_le_bytes()[0]);
    std::fs::write(&bad, truncated).expect("write");

    let out = common::target_command(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .arg("disasm")
        .arg(&bad)
        .arg("main:args:")
        .output()
        .expect("run ns-next-analyzer disasm");
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.starts_with("error:"), "unexpected stderr: {error}");
    assert!(!error.contains("panicked"), "decoder panicked: {error}");
}
