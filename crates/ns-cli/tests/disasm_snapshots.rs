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

/// `--class` addresses a nested class's own method and a class side's own
/// method by mixin path, and reports a clean error (not a panic) for a
/// path segment that names nothing in the tree. Not a `tests-fixtures/`
/// fixture: adding one there would need updating `differential.rs`'s
/// hardcoded fixture count, and nested-class instantiation is out of scope
/// until the accessor exists -- this only exercises `disasm`'s addressing,
/// never runs the module.
#[test]
fn disasm_addresses_a_nested_class_and_its_class_side_by_mixin_path() {
    let path = std::env::temp_dir().join("ns_next_disasm_nested_path_test.ns");
    std::fs::write(
        &path,
        "Newspeak3\n'T'\nclass Outer usingPlatform: p = Object new () (\n\
         \x20   public class Inner = Object new () (\n\
         \x20       public value = ( ^42 )\n\
         \x20   )\n\
         ) : (\n\
         \x20   public marker = ( ^2 )\n\
         )\n",
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
