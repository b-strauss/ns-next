mod common;
use common::ScratchDirectory;

use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ns-next-analyzer"))
        .args(arguments)
        .output()
        .expect("run ns-next-analyzer")
}

/// A refused command line: `clap`'s usage on stderr, nothing on stdout, and
/// its usage-error status.
fn assert_usage_failure(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    let usage = String::from_utf8_lossy(&output.stderr);
    assert!(usage.contains("Usage: ns-next-analyzer"), "{usage}");
    assert!(output.stdout.is_empty());
}

#[test]
fn no_command_prints_the_usage_instead_of_starting_the_server() {
    let output = run(&[]);
    assert_usage_failure(&output);
    let usage = String::from_utf8_lossy(&output.stderr);
    for command in [
        "lsp",
        "dump-tokens",
        "parse",
        "parse-corpus",
        "disasm",
        "inspect",
    ] {
        assert!(usage.contains(command), "{usage}");
    }
}

#[test]
fn an_unknown_command_prints_the_usage() {
    let output = run(&["format"]);
    assert_usage_failure(&output);
    let usage = String::from_utf8_lossy(&output.stderr);
    assert!(
        usage.contains("unrecognized subcommand 'format'"),
        "{usage}"
    );
}

#[test]
fn lsp_takes_no_further_arguments() {
    assert_usage_failure(&run(&["lsp", "extra"]));
}

/// Without the command name the arguments are not a command at all: a file
/// alone is refused, not dumped.
#[test]
fn a_file_without_a_command_prints_the_usage() {
    let directory = ScratchDirectory::new("file-without-command");
    let path = directory.join("Empty.ns");
    std::fs::write(&path, "class Empty = ()()\n").expect("write the module");
    assert_usage_failure(&run(&[path.to_str().expect("a UTF-8 path")]));
}

#[test]
fn dump_tokens_prints_a_files_tokens_as_json() {
    let directory = ScratchDirectory::new("dump-tokens");
    let path = directory.join("Empty.ns");
    std::fs::write(&path, "class Empty = ()()\n").expect("write the module");
    let output = run(&["dump-tokens", path.to_str().expect("a UTF-8 path")]);
    assert_eq!(output.status.code(), Some(0));
    let dumped: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert!(dumped["tokens"].is_array(), "{dumped}");
    assert!(dumped["file"].is_string(), "{dumped}");
}
