use std::path::Path;
use std::process::Command;

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

/// The workspace's `corpus/modules` directory -- resolved from
/// `CARGO_MANIFEST_DIR` rather than a bare relative path, since the test
/// binary's working directory is this crate's own directory, not the
/// workspace root (`tests/corpus.rs`, `tests/snapshot_load.rs` and others
/// already establish this convention).
fn corpus_module(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/modules")
        .join(name)
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
fn a_zero_argument_factory_no_longer_rejects_instantiation() {
    // `ADummyPush.ns`'s primary factory is `new`, i.e. zero arguments: until
    // this fix, `cmd_run` unconditionally supplied `usingPlatform:`'s one
    // argument, so instantiation failed before `foo` was ever looked up.
    // `foo` itself carries no access modifier and so defaults to protected
    // (`newspeak-spec.pdf` §6.3.1) -- unreachable from an ordinary external
    // send under spec-correct dispatch (§5.5's doesNotUnderstand:/proxy
    // argument is exactly why that hard stop exists), so the module still
    // ends in "does not understand #foo". What this fix changes is *when*
    // it fails: no longer during instantiation with a CLI-level
    // argument-count mismatch, only afterward, at the entry send, for the
    // ordinary reason any protected method rejects an outside sender.
    let out = ns_next()
        .arg("run")
        .arg(corpus_module("ADummyPush.ns"))
        .arg("--send")
        .arg("foo")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("wrong number of factory arguments"),
        "instantiation should no longer reject ADummyPush's zero-argument \
         factory: {stderr}"
    );
    assert!(
        stderr.contains("does not understand #foo"),
        "expected the ordinary protected-access rejection, got: {stderr}"
    );
}

#[test]
fn a_zero_argument_factory_reaches_its_public_entry_method() {
    // The same zero-argument-factory shape as `ADummyPush.ns`, end to end,
    // with a public entry method (the entry send still carries the
    // platform as its own argument, per `cmd_run`'s `[platform, nil]`
    // convention -- only the *factory* call is now zero-argument) so the
    // fix's effect is directly observable on stdout.
    let dir = tempdir_named("zero-arg-factory");
    let f = dir.join("NoPlatform.ns");
    std::fs::write(
        &f,
        "Newspeak3\n'Test'\nclass NoPlatform new = Object new (\n) (\n\tpublic foo: p = (\n\t\tp transcript show: '99'; newLine\n\t)\n) : ()\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("run")
        .arg(&f)
        .arg("--send")
        .arg("foo:")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "99");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_multi_parameter_factory_is_refused_with_a_clear_error() {
    let dir = tempdir_named("multi-arg-factory");
    let f = dir.join("TwoArgs.ns");
    std::fs::write(
        &f,
        "Newspeak3\n'Test'\nclass TwoArgs usingPlatform: platform ide: webIde = Object new (\n) (\n) : ()\n",
    )
    .unwrap();

    let out = ns_next().arg("run").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("usingPlatform:ide:"),
        "stderr did not name the declared selector: {stderr}"
    );
    assert!(
        stderr.contains('2'),
        "stderr did not name the declared arity: {stderr}"
    );
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

/// The probe that exposed the frame-shift defect end to end: a *unary*
/// entry point with a declared temporary. `--send probe` used to hand it
/// `[platform, nil]` regardless of arity, which shifted `t` off the
/// surplus arguments and printed `NOTNIL-BUG` under both engines with exit
/// status 0. Both halves of the fix are needed for this to pass: the CLI
/// sizes the argument list to the selector, and `invoke_method` refuses a
/// mismatch outright if anything ever gets it wrong again.
const UNARY_PROBE: &str = "Newspeak3\n'Test'\nclass Probe usingPlatform: p = Object new (\n\t|\n\t\tprivate plat = p.\n\t|\n) (\n\
     \tpublic probe = (\n\t\t| t |\n\t\tnil = t ifTrue: [ plat transcript show: 'nil-ok'; newLine ]\n\t\t\tifFalse: [ plat transcript show: 'NOTNIL-BUG'; newLine ]\n\t)\n) : ()\n";

#[test]
fn a_unary_entry_selector_gets_no_arguments_and_reads_its_temporary_as_nil() {
    let dir = tempdir_named("unary-send");
    let f = dir.join("Probe.ns");
    std::fs::write(&f, UNARY_PROBE).unwrap();

    let out = ns_next()
        .arg("run")
        .arg(&f)
        .arg("--send")
        .arg("probe")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert_eq!(stdout, "nil-ok\n", "unexpected stdout");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_entry_selector_the_cli_cannot_fill_is_refused_before_the_send() {
    let dir = tempdir_named("wide-send");
    let f = dir.join("Wide.ns");
    std::fs::write(
        &f,
        "Newspeak3\n'Test'\nclass Wide usingPlatform: p = Object new (\n) (\n\tpublic a: x b: y c: z = (\n\t\t^x\n\t)\n) : ()\n",
    )
    .unwrap();

    let out = ns_next()
        .arg("run")
        .arg(&f)
        .arg("--send")
        .arg("a:b:c:")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("a:b:c:"), "stderr: {stderr}");
    assert!(stderr.contains("3 argument(s)"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
