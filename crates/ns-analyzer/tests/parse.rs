//! `ns-next-analyzer parse` and `parse-corpus`.

mod common;

use common::ScratchDirectory;
use indoc::indoc;
use std::process::Command;

fn ns_next_analyzer() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next-analyzer"))
}

#[test]
fn parse_ok_file() {
    let directory = ScratchDirectory::new("parse-ok");
    let file = directory.join("Ok.ns");
    std::fs::write(
        &file,
        indoc! {"
            class C = () ()
        "},
    )
    .unwrap();
    let out = ns_next_analyzer().arg("parse").arg(&file).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parse_bad_file_exits_1() {
    let directory = ScratchDirectory::new("parse-bad");
    let file = directory.join("Bad.ns");
    std::fs::write(
        &file,
        indoc! {"
            class C = (
        "},
    )
    .unwrap();
    let out = ns_next_analyzer().arg("parse").arg(&file).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

#[test]
fn parse_corpus_reports_counts() {
    let directory = ScratchDirectory::new("corpus-counts");
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("Good.ns"),
        indoc! {"
            class C = () ()
        "},
    )
    .unwrap();
    std::fs::write(
        modules.join("AlsoGood.ns"),
        indoc! {"
            class D = () ()
        "},
    )
    .unwrap();
    std::fs::write(
        modules.join("Bad.ns"),
        indoc! {"
            class C = (
        "},
    )
    .unwrap();
    let out = ns_next_analyzer()
        .arg("parse-corpus")
        .arg(&modules)
        .output()
        .unwrap();
    // `Bad.ns` does not parse, so the run fails and names it.
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("parsed 2, failed 1"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FAIL Bad.ns"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parse_corpus_succeeds_when_every_file_parses() {
    let directory = ScratchDirectory::new("corpus-clean");
    let modules = directory.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("Good.ns"),
        indoc! {"
            class C = () ()
        "},
    )
    .unwrap();
    let out = ns_next_analyzer()
        .arg("parse-corpus")
        .arg(&modules)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("parsed 1, failed 0"));
}
