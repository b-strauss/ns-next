use std::process::Command;

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

#[test]
fn parse_ok_file() {
    let dir = tempdir();
    let f = dir.join("Ok.ns");
    std::fs::write(&f, "Newspeak3\n'X'\nclass C = ()\n").unwrap();
    let out = ns_next().arg("parse").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parse_bad_file_exits_1() {
    let dir = tempdir();
    let f = dir.join("Bad.ns");
    std::fs::write(&f, "Newspeak3\n'X'\nclass C = (").unwrap();
    let out = ns_next().arg("parse").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

// minimal tempdir helper to avoid a dev-dependency
fn tempdir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ns-cli-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}
