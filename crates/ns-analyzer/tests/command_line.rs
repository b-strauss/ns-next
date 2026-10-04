use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .args(arguments)
        .output()
        .expect("run ns-next-analyzer")
}

fn assert_usage_failure(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    let usage = String::from_utf8_lossy(&output.stderr);
    assert!(usage.contains("usage: ns-next-analyzer lsp"), "{usage}");
    assert!(usage.contains("dump-tokens"), "{usage}");
    assert!(output.stdout.is_empty());
}

#[test]
fn no_command_prints_the_usage_instead_of_starting_the_server() {
    assert_usage_failure(&run(&[]));
}

#[test]
fn an_unknown_command_prints_the_usage() {
    assert_usage_failure(&run(&["format"]));
}

#[test]
fn lsp_takes_no_further_arguments() {
    assert_usage_failure(&run(&["lsp", "extra"]));
}

#[test]
fn dump_tokens_prints_a_files_tokens_as_json() {
    let path = std::env::temp_dir().join("ns-next-analyzer-dump-tokens-test.ns");
    std::fs::write(&path, "class Empty = ()()\n").expect("write the module");
    let output = run(&[path.to_str().expect("a UTF-8 path")]);
    // Without the command name the arguments are not a command at all.
    assert_usage_failure(&output);
    let output = run(&["dump-tokens", path.to_str().expect("a UTF-8 path")]);
    std::fs::remove_file(&path).expect("remove the module");
    assert_eq!(output.status.code(), Some(0));
    let dumped: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert!(dumped["tokens"].is_array(), "{dumped}");
    assert!(dumped["file"].is_string(), "{dumped}");
}
