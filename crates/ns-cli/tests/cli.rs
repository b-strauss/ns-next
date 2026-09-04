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

#[test]
fn parse_corpus_reports_counts() {
    let dir = tempdir_named("corpus-counts");
    let modules = dir.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("Good.ns"), "Newspeak3\n'X'\nclass C = ()\n").unwrap();
    std::fs::write(
        modules.join("AlsoGood.ns"),
        "Newspeak3\n'X'\nclass D = ()\n",
    )
    .unwrap();
    std::fs::write(modules.join("Bad.ns"), "Newspeak3\n'X'\nclass C = (").unwrap();
    let bl = dir.join("blacklist.toml");
    std::fs::write(
        &bl,
        "[[entry]]\nfile = \"Bad.ns\"\nreason = \"deliberately malformed for the test\"\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&bl)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("parsed 2"), "{stdout}");
    assert!(stdout.contains("failed 0"), "{stdout}");
    assert!(stdout.contains("expected-fail 1"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_corpus_stale_blacklist_entry_fails() {
    let dir = tempdir_named("corpus-stale");
    let modules = dir.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("Good.ns"), "Newspeak3\n'X'\nclass C = ()\n").unwrap();
    let bl = dir.join("blacklist.toml");
    // Names a file that actually parses now -> stale.
    std::fs::write(
        &bl,
        "[[entry]]\nfile = \"Good.ns\"\nreason = \"no longer fails\"\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&bl)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("stale"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_corpus_malformed_blacklist_fails_with_error() {
    let dir = tempdir_named("corpus-malformed");
    let modules = dir.join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("Good.ns"), "Newspeak3\n'X'\nclass C = ()\n").unwrap();
    let bl = dir.join("blacklist.toml");
    std::fs::write(&bl, "this is not valid toml :::\n[[entry\n").unwrap();

    let out = ns_next()
        .arg("parse-corpus")
        .arg(&modules)
        .arg("--blacklist")
        .arg(&bl)
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("blacklist"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

// minimal tempdir helper to avoid a dev-dependency
fn tempdir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ns-cli-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}

// A per-test uniquely named temp dir, so tests running in parallel do not
// collide and each cleans up after itself.
fn tempdir_named(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ns-cli-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
