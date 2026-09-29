mod common;

use common::ScratchDirectory;
use indoc::indoc;
use std::path::Path;
use std::process::{Command, Output};

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

fn run(file: &Path) -> Output {
    ns_next().arg("run").arg(file).output().unwrap()
}

/// A project's root: `packageUsing:` its manifest, reaching the module
/// `Greeter` through it.
const APP: &str = indoc! {"
    Newspeak3
    'Test'
    class App packageUsing: manifest = Object new (
        | Greeter = manifest Greeter. |
    ) (
        public main: platform args: arguments = (
            platform transcript show: Greeter new greeting; newLine
        )
    ) : ()
"};

const GREETER: &str = indoc! {"
    Newspeak3
    'Test'
    class Greeter = Object new () (
        public greeting = ( ^'hello from a module' )
    ) : ()
"};

#[test]
fn run_sends_main_arguments_and_prints_via_platform_transcript() {
    let directory = ScratchDirectory::new("hello");
    let file = directory.join("Hello.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'Test'
            class Hello = Object new (
            ) (
                public main: platform args: arguments = (
                    platform transcript show: 'hi'; newLine
                )
            ) : ()
        "},
    )
    .unwrap();

    let out = run(&file);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "hi\n", "unexpected stdout: {stdout:?}");
}

#[test]
fn run_reports_a_parse_error_and_exits_nonzero() {
    let directory = ScratchDirectory::new("bad-parse");
    let file = directory.join("Bad.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'Test'
            class C = (
        "},
    )
    .unwrap();

    let out = run(&file);
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}

#[test]
fn a_source_file_whose_root_takes_the_platform_is_refused_before_anything_runs() {
    let directory = ScratchDirectory::new("old-root");
    let file = directory.join("Old.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'Test'
            class Old usingPlatform: platform = Object new () (
                public main: platform args: arguments = ( platform transcript show: 'ran' )
            ) : ()
        "},
    )
    .unwrap();

    let out = run(&file);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot start `Old`: its primary factory is `usingPlatform:`"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("new"), "stderr: {stderr}");
}

#[test]
fn a_project_assembles_its_root_with_the_manifest() {
    let directory = ScratchDirectory::new("project");
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("Greeter.ns"), GREETER).unwrap();
    let project = directory.join("app.json");
    std::fs::write(
        &project,
        r#"{ "root": "App.ns", "modules": ["Greeter.ns"] }"#,
    )
    .unwrap();

    let out = run(&project);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hello from a module\n"
    );
}

#[test]
fn a_project_module_is_reached_through_a_backslash_path() {
    let directory = ScratchDirectory::new("project-backslash");
    std::fs::create_dir_all(directory.join("lib")).unwrap();
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("lib").join("Greeter.ns"), GREETER).unwrap();
    let project = directory.join("app.json");
    std::fs::write(
        &project,
        r#"{ "root": "App.ns", "modules": ["lib\\Greeter.ns"] }"#,
    )
    .unwrap();

    let out = run(&project);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hello from a module\n"
    );
}

#[test]
fn a_projects_root_must_take_the_manifest() {
    let directory = ScratchDirectory::new("project-new-root");
    std::fs::write(
        directory.join("App.ns"),
        indoc! {"
            Newspeak3
            'Test'
            class App = Object new () (
                public main: platform args: arguments = ( platform transcript show: 'ran' )
            ) : ()
        "},
    )
    .unwrap();
    std::fs::write(directory.join("Greeter.ns"), GREETER).unwrap();
    let project = directory.join("app.json");
    std::fs::write(
        &project,
        r#"{ "root": "App.ns", "modules": ["Greeter.ns"] }"#,
    )
    .unwrap();

    let out = run(&project);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot start `App`: its primary factory is `new`"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("packageUsing:"), "stderr: {stderr}");
}

#[test]
fn two_modules_of_one_name_are_refused() {
    let directory = ScratchDirectory::new("project-twins");
    let twin = indoc! {"
        Newspeak3
        'Test'
        class Twin = Object new () () : ()
    "};
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("A.ns"), twin).unwrap();
    std::fs::write(directory.join("B.ns"), twin).unwrap();
    let project = directory.join("app.json");
    std::fs::write(
        &project,
        r#"{ "root": "App.ns", "modules": ["A.ns", "B.ns"] }"#,
    )
    .unwrap();

    let out = run(&project);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("`Twin`"), "stderr: {stderr}");
    assert!(stderr.contains("A.ns"), "stderr: {stderr}");
    assert!(stderr.contains("B.ns"), "stderr: {stderr}");
}

#[test]
fn a_missing_module_file_is_refused() {
    let directory = ScratchDirectory::new("project-missing");
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    let project = directory.join("app.json");
    std::fs::write(
        &project,
        r#"{ "root": "App.ns", "modules": ["Nowhere.ns"] }"#,
    )
    .unwrap();

    let out = run(&project);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Nowhere.ns"), "stderr: {stderr}");
}

#[test]
fn a_module_shadowed_by_an_object_member_is_refused() {
    let directory = ScratchDirectory::new("project-shadowed");
    std::fs::write(
        directory.join("App.ns"),
        indoc! {"
            Newspeak3
            'Test'
            class App packageUsing: manifest = Object new () (
                public main: platform args: arguments = ( platform transcript show: 'ran' )
            ) : ()
        "},
    )
    .unwrap();
    std::fs::write(
        directory.join("Array.ns"),
        indoc! {"
            Newspeak3
            'Test'
            class Array = Object new () () : ()
        "},
    )
    .unwrap();
    let project = directory.join("app.json");
    std::fs::write(&project, r#"{ "root": "App.ns", "modules": ["Array.ns"] }"#).unwrap();

    let out = run(&project);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("`Array`"), "stderr: {stderr}");
    assert!(stderr.contains("public member"), "stderr: {stderr}");
}

#[test]
fn a_program_without_main_is_an_uncaught_does_not_understand() {
    let directory = ScratchDirectory::new("no-main");
    let file = directory.join("Quiet.ns");
    std::fs::write(
        &file,
        indoc! {"
            Newspeak3
            'Test'
            class Quiet = Object new () () : ()
        "},
    )
    .unwrap();

    let out = run(&file);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("Uncaught Newspeak error: "),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("does not understand #main:args:"),
        "stderr: {stderr}"
    );
}
