use std::process::Command;

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

// A per-test uniquely named temp dir, so tests running in parallel do not
// collide and each cleans up after itself -- same helper as `tests/cli.rs`.
fn tempdir_named(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ns-cli-run-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn run_sends_main_args_and_prints_via_platform_transcript() {
    let dir = tempdir_named("hello");
    let f = dir.join("Hello.ns");
    std::fs::write(
        &f,
        "Newspeak3\n'Test'\nclass Hello usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\tp transcript show: 'hi'; newLine\n\t)\n) : ()\n",
    )
    .unwrap();

    let out = ns_next().arg("run").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "hi\n", "unexpected stdout: {stdout:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_reports_a_parse_error_and_exits_nonzero() {
    let dir = tempdir_named("bad-parse");
    let f = dir.join("Bad.ns");
    std::fs::write(&f, "Newspeak3\n'Test'\nclass C = (").unwrap();

    let out = ns_next().arg("run").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_supports_a_custom_entry_selector_via_send() {
    let dir = tempdir_named("custom-send");
    let f = dir.join("Greet.ns");
    std::fs::write(
        &f,
        "Newspeak3\n'Test'\nclass Greet usingPlatform: platform = Object new (\n) (\n\tpublic greet: p = (\n\t\tp transcript show: 'custom'; newLine\n\t)\n) : ()\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("run")
        .arg(&f)
        .arg("--send")
        .arg("greet:")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "custom\n", "unexpected stdout: {stdout:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
