//! `ns-next disasm` snapshot tests -- one per representative code shape:
//! plain keyword method, block with non-local return, inlined
//! ifTrue:ifFalse:, inlined whileTrue:, the non-inlined fallback case, an
//! instance initializer (`docs/design/compiler.md`, "Reading the output"),
//! and the same two shapes under the reference lowering. Catches accidental
//! codegen changes.

use indoc::indoc;
mod common;
use common::ScratchDir;

use std::path::Path;
use std::process::Command;

fn disasm(file: &str, args: &[&str]) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-fixtures")
        .join(file);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .args(args)
        .output()
        .expect("run ns-next disasm");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn disasm_arithmetic_main() {
    insta::assert_snapshot!(disasm("arithmetic.ns", &["main:args:"]));
}

#[test]
fn disasm_arithmetic_initializer() {
    insta::assert_snapshot!(disasm("arithmetic.ns", &["--initializer"]));
}

#[test]
fn disasm_blocks_and_return_find_in() {
    insta::assert_snapshot!(disasm("blocks_and_return.ns", &["find:in:"]));
}

#[test]
fn disasm_while_loop_main() {
    insta::assert_snapshot!(disasm("while_loop.ns", &["main:args:"]));
}

#[test]
fn disasm_control_flow_fallback_main() {
    insta::assert_snapshot!(disasm("control_flow_fallback.ns", &["main:args:"]));
}

#[test]
fn disasm_instance_init_initializer() {
    insta::assert_snapshot!(disasm("instance_init.ns", &["--initializer"]));
}

#[test]
fn disasm_while_loop_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm(
        "while_loop.ns",
        &["main:args:", "--lowering", "reference"]
    ));
}

#[test]
fn disasm_control_flow_fallback_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm(
        "control_flow_fallback.ns",
        &["main:args:", "--lowering", "reference"]
    ));
}

#[test]
fn disasm_rejects_an_unknown_selector_and_conflicting_arguments() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/hello.ns");
    let missing = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg("nope")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no method `nope`"));
    let both = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg("main:args:")
        .arg("--initializer")
        .output()
        .unwrap();
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("exactly one"));
}

/// `--class` addresses a nested class's own method and a class side's own
/// method by mixin path, and reports a clean error (not a panic) for a
/// path segment that names nothing in the tree. Not a `tests-fixtures/`
/// fixture: this only exercises `disasm`'s addressing and never runs the
/// module, while every fixture is run end to end and checked against its
/// `.expected` file by `run_fixtures.rs`.
#[test]
fn disasm_addresses_a_nested_class_and_its_class_side_by_mixin_path() {
    let dir = ScratchDir::new("disasm-nested-path");
    let path = dir.join("nested.ns");
    std::fs::write(
        &path,
        indoc! {"
            Newspeak3
            'T'
            class Outer usingPlatform: p = Object new () (
                public class Inner = Object new () (
                    public value = ( ^42 )
                )
            ) : (
                public marker = ( ^2 )
            )
        "},
    )
    .expect("write temp fixture");

    let inner = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg("value")
        .arg("--class")
        .arg("Outer.Inner")
        .output()
        .expect("run ns-next disasm");
    assert!(
        inner.status.success(),
        "{}",
        String::from_utf8_lossy(&inner.stderr)
    );
    assert!(String::from_utf8_lossy(&inner.stdout).contains("method value"));

    let class_side = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg("marker")
        .arg("--class")
        .arg("Outer.class")
        .output()
        .expect("run ns-next disasm");
    assert!(
        class_side.status.success(),
        "{}",
        String::from_utf8_lossy(&class_side.stderr)
    );
    assert!(String::from_utf8_lossy(&class_side.stdout).contains("method marker"));

    let unknown = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg("value")
        .arg("--class")
        .arg("Outer.Nope")
        .output()
        .expect("run ns-next disasm");
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("has no nested class `Nope`"));

    let _ = std::fs::remove_file(&path);
}

/// `disasm` of an artifact must print exactly what `disasm` of the source it
/// was built from prints. The artifact carries the same compiled bodies; if
/// these ever differ, the image's code table is not what the compiler emitted.
#[test]
fn disasm_of_an_image_matches_disasm_of_its_source() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/arithmetic.ns");
    let dir = ScratchDir::new("disasm-img");
    let img = dir.join("world.img");

    let snap = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("snapshot")
        .arg(&src)
        .arg(&img)
        .output()
        .expect("run ns-next snapshot");
    assert!(
        snap.status.success(),
        "{}",
        String::from_utf8_lossy(&snap.stderr)
    );

    let from_image = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&img)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm on an image");
    assert!(
        from_image.status.success(),
        "{}",
        String::from_utf8_lossy(&from_image.stderr)
    );

    let from_source = disasm("arithmetic.ns", &["main:args:"]);
    assert_eq!(
        String::from_utf8_lossy(&from_image.stdout).replace("\r\n", "\n"),
        from_source
    );
}

/// The same property as `disasm_of_an_image_matches_disasm_of_its_source`,
/// but for a stateless binary rather than an image. Only the image form was
/// covered before this test existed, yet the whole reason `disasm` reads an
/// artifact at all is the binary that ships *without* source -- a regression
/// that broke `disasm` for `FLAG_STATELESS` specifically (a decode path the
/// image form never exercises) could have passed the image test above.
#[test]
fn disasm_of_a_binary_matches_disasm_of_its_source() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/arithmetic.ns");
    let dir = ScratchDir::new("disasm-bin");
    let bin = dir.join("app.nsb");

    let build = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("build")
        .arg(&src)
        .arg(&bin)
        .output()
        .expect("run ns-next build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let from_binary = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&bin)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm on a binary");
    assert!(
        from_binary.status.success(),
        "{}",
        String::from_utf8_lossy(&from_binary.stderr)
    );

    let from_source = disasm("arithmetic.ns", &["main:args:"]);
    assert_eq!(
        String::from_utf8_lossy(&from_binary.stdout).replace("\r\n", "\n"),
        from_source
    );
}

/// An artifact whose root row names no instance initializer is legal when the
/// class declares no slots and no `initExpr`s -- `verify_mixins` accepts it --
/// so `disasm --initializer` on one must refuse with a message, not panic: an
/// artifact is untrusted input. No source compiles to such a row (every mixin
/// built from source gets an initializer), so the artifact is written through
/// the runtime with the root's initializer removed, the way a crafted file
/// would present it.
#[test]
fn disasm_refuses_an_initializer_an_artifact_does_not_carry() {
    use ns_runtime::{Interpreter, Mixin, SourceOrigin, Symbol};
    use std::rc::Rc;

    let src = indoc! {"
        Newspeak3
        'T'
        class Bare usingPlatform: p = Object new () (
            public main: p args: a = ( ^0 )
        ) : ()
    "};
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File("bare.ns".into()))
        .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module_unrooted(&resolved);
    let application = interp
        .heap_ref()
        .class_application(class)
        .expect("a class object");
    let bare = Mixin {
        initializer_code: None,
        ..(*application.mixin).clone()
    };
    let rewired = interp.apply_mixin(&Rc::new(bare), application.superclass.as_ref());
    assert!(interp.heap_mut().set_class_application(class, rewired));
    let bytes = ns_runtime::image::build(
        interp.heap_ref(),
        None,
        &SourceOrigin::File("bare.ns".into()),
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");

    let dir = ScratchDir::new("disasm-no-init");
    let bin = dir.join("bare.nsb");
    std::fs::write(&bin, bytes).expect("write artifact");

    let out = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&bin)
        .arg("--initializer")
        .output()
        .expect("run ns-next disasm");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "disasm panicked: {err}");
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert!(
        err.contains("error: `Bare` has no instance initializer"),
        "unexpected stderr: {err}"
    );

    // The same artifact still disassembles a method: only the missing body
    // is refused.
    let method = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&bin)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm");
    assert!(
        method.status.success(),
        "{}",
        String::from_utf8_lossy(&method.stderr)
    );
}

/// A file that is neither parseable source nor a valid artifact must report
/// one clear failure, not a decoder backtrace.
#[test]
fn disasm_rejects_a_corrupt_artifact_with_a_message() {
    let dir = ScratchDir::new("disasm-bad");
    let bad = dir.join("truncated.img");
    // Magic, so it is taken for an artifact, and then nothing.
    std::fs::write(&bad, b"NSIM").expect("write");

    let out = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&bad)
        .arg("main:args:")
        .output()
        .expect("run ns-next disasm");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error:"), "unexpected stderr: {err}");
    assert!(!err.contains("panicked"), "decoder panicked: {err}");
}
