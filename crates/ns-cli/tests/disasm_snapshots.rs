//! `ns-next disasm` snapshot tests -- one per representative code shape
//! (M2 spec, "Testing": plain keyword method, block with non-local return,
//! inlined ifTrue:ifFalse:, inlined whileTrue:, the non-inlined fallback
//! case, an instance initializer). Catches accidental codegen changes.

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
