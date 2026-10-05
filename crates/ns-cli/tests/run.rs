mod common;

use common::{goldens_root, ScratchDirectory};
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
    class App packageUsing: manifest = Object new (
        | Greeter = manifest Greeter. |
    ) (
        public main: platform args: arguments = (
            platform transcript show: Greeter new greeting; newLine
        )
    ) : ()
"};

const GREETER: &str = indoc! {"
    class Greeter = Object new () (
        public greeting = ( ^'hello from a module' )
    ) : ()
"};

/// A source file's root, started with `new`; its `main:args:` prints `hi`.
const HELLO: &str = indoc! {"
    class Hello = Object new (
    ) (
        public main: platform args: arguments = (
            platform transcript show: 'hi'; newLine
        )
    ) : ()
"};

/// A root that prints each of `main:args:`'s arguments on its own line.
const ECHO: &str = indoc! {"
    class Echo packageUsing: manifest = Object new () (
        public main: platform args: arguments = (
            arguments do: [:argument | platform transcript show: argument; newLine]
        )
    ) : ()
"};

fn echo_project(name: &str) -> ScratchDirectory {
    let directory = ScratchDirectory::new(name);
    std::fs::write(directory.join("Echo.ns"), ECHO).unwrap();
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "Echo.ns", "scripts": { "two": ["first", "second word"], "none": [] } }"#,
    )
    .unwrap();
    directory
}

#[test]
fn a_script_hands_its_arguments_to_main_and_the_command_line_follows() {
    let directory = echo_project("script-arguments");
    let out = ns_next()
        .args(["run", "--script", "two"])
        .arg(directory.path())
        .args(["--", "third"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "first\nsecond word\nthird\n"
    );
}

/// `--script=<name>` is one token, as the JetBrains plugin passes it: a name
/// starting with `-` is not read as an option, and the empty name, a valid
/// key of `scripts`, is a script rather than none.
#[test]
fn a_script_named_in_one_token_may_start_with_a_dash_or_be_empty() {
    let directory = ScratchDirectory::new("script-one-token");
    std::fs::write(directory.join("Echo.ns"), ECHO).unwrap();
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "Echo.ns", "scripts": { "-x": ["dash"], "": ["empty"] } }"#,
    )
    .unwrap();
    for (script, expected) in [("--script=-x", "dash\n"), ("--script=", "empty\n")] {
        let out = ns_next()
            .args(["run", script])
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{script}: stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{script}");
    }
}

#[test]
fn an_unknown_script_is_refused_naming_the_scripts_there_are() {
    let directory = echo_project("script-unknown");
    let out = ns_next()
        .args(["run", "--script", "three"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "nothing runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("three") && stderr.contains("none, two"),
        "stderr: {stderr}"
    );
}

#[test]
fn a_script_with_a_source_file_is_refused() {
    let directory = ScratchDirectory::new("script-source");
    std::fs::write(directory.join("Hello.ns"), HELLO).unwrap();
    let out = ns_next()
        .args(["run", "--script", "two"])
        .arg(directory.join("Hello.ns"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "nothing runs");
    assert!(String::from_utf8_lossy(&out.stderr).contains(
            "--script names a script of a project's newspeak.json, and the program given is a source file"
        )
    );
}

#[test]
fn run_sends_main_arguments_and_prints_via_platform_transcript() {
    let directory = ScratchDirectory::new("hello");
    let file = directory.join("Hello.ns");
    std::fs::write(&file, HELLO).unwrap();

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
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["Greeter.ns"] }"#,
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

/// A project is its directory: `run` takes the directory, its
/// `newspeak.json`, or nothing at all in it, and answers alike.
#[test]
fn a_project_runs_from_its_directory_its_file_or_no_argument() {
    let directory = ScratchDirectory::new("project-directory");
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("Greeter.ns"), GREETER).unwrap();
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "App.ns", "modules": ["Greeter.ns"] }"#,
    )
    .unwrap();

    let from_directory = run(directory.path());
    let from_file = run(&directory.join("newspeak.json"));
    let from_nothing = ns_next()
        .arg("run")
        .current_dir(directory.path())
        .output()
        .unwrap();
    for out in [from_directory, from_file, from_nothing] {
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
}

/// Only `newspeak.json` is a project file: a directory without one, and a
/// JSON file of another name, are refused naming the file a project needs
/// rather than run as something else.
#[test]
fn a_project_file_of_another_name_or_a_directory_without_one_is_refused() {
    let directory = ScratchDirectory::new("project-misnamed");
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    let misnamed = directory.join("app.json");
    std::fs::write(&misnamed, r#"{ "main": "App.ns" }"#).unwrap();

    for out in [run(&misnamed), run(directory.path())] {
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty(), "nothing runs");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("newspeak.json"), "stderr: {stderr}");
    }
}

#[test]
fn a_project_module_is_reached_through_a_backslash_path() {
    let directory = ScratchDirectory::new("project-backslash");
    std::fs::create_dir_all(directory.join("lib")).unwrap();
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("lib").join("Greeter.ns"), GREETER).unwrap();
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["lib\\Greeter.ns"] }"#,
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
            class App = Object new () (
                public main: platform args: arguments = ( platform transcript show: 'ran' )
            ) : ()
        "},
    )
    .unwrap();
    std::fs::write(directory.join("Greeter.ns"), GREETER).unwrap();
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["Greeter.ns"] }"#,
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
        class Twin = Object new () () : ()
    "};
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("A.ns"), twin).unwrap();
    std::fs::write(directory.join("B.ns"), twin).unwrap();
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["A.ns", "B.ns"] }"#,
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
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["Nowhere.ns"] }"#,
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
            class App packageUsing: manifest = Object new () (
                public main: platform args: arguments = ( platform transcript show: 'ran' )
            ) : ()
        "},
    )
    .unwrap();
    std::fs::write(
        directory.join("Array.ns"),
        indoc! {"
            class Array = Object new () () : ()
        "},
    )
    .unwrap();
    let project = directory.join("newspeak.json");
    std::fs::write(&project, r#"{ "main": "App.ns", "modules": ["Array.ns"] }"#).unwrap();

    let out = run(&project);
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("`Array`"), "stderr: {stderr}");
    assert!(stderr.contains("public member"), "stderr: {stderr}");
}

/// The root rule is a resolve error for every module root, so a project
/// module that breaks it is refused even though nothing ever instantiates
/// it: `run` refuses what `build` could never write, with the same text.
#[test]
fn a_never_used_module_breaking_the_root_rule_is_refused_by_run_and_build_alike() {
    let directory = ScratchDirectory::new("project-unused-defective-root");
    std::fs::write(directory.join("App.ns"), APP).unwrap();
    std::fs::write(directory.join("Greeter.ns"), GREETER).unwrap();
    std::fs::write(
        directory.join("Unused.ns"),
        indoc! {"
            class Unused = Array new () () : ()
        "},
    )
    .unwrap();
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        r#"{ "main": "App.ns", "modules": ["Greeter.ns", "Unused.ns"] }"#,
    )
    .unwrap();
    let expected = "resolve error: a module's root class may inherit only Object or Value, \
                    or write Object <: Value, and its superclass clause names Array\n";

    let ran = run(&project);
    let built = ns_next()
        .arg("build")
        .arg(&project)
        .arg(directory.join("app.nsi"))
        .output()
        .unwrap();
    for (command, out) in [("run", &ran), ("build", &built)] {
        assert_eq!(out.status.code(), Some(1), "{command}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{command}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), expected, "{command}");
    }
    assert!(!directory.join("app.nsi").exists());
}

#[test]
fn a_program_without_main_is_an_uncaught_does_not_understand() {
    let directory = ScratchDirectory::new("no-main");
    let file = directory.join("Quiet.ns");
    std::fs::write(
        &file,
        indoc! {"
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

/// The Are We Fast Yet harness's project directory, reached from `goldens/`'s
/// sibling so that no second way of finding the repository root exists.
fn harness_project() -> std::path::PathBuf {
    goldens_root().join("../benchmarks/are-we-fast-yet")
}

/// `--timed` prints `<Name>: <runs per second>` with one fraction digit,
/// for every benchmark in the harness's order; the numbers differ between
/// runs, so only their shape is checked.
#[test]
fn a_timed_run_prints_a_score_for_every_benchmark() {
    let output = ns_next()
        .arg("run")
        .arg(harness_project())
        .args(["--", "--timed"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let names: Vec<&str> = stdout
        .lines()
        .map(|line| {
            let (name, score) = line.split_once(": ").expect("a name and a score");
            let (whole, fraction) = score.split_once('.').expect("a fraction digit");
            assert!(
                !whole.is_empty() && whole.bytes().all(|byte| byte.is_ascii_digit()),
                "{line}"
            );
            assert!(
                fraction.len() == 1 && fraction.bytes().all(|byte| byte.is_ascii_digit()),
                "{line}"
            );
            name
        })
        .collect();
    assert_eq!(
        names,
        [
            "Bounce",
            "CD",
            "DeltaBlue",
            "DeltaBlueTon80",
            "Havlak",
            "Json",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Richards",
            "RichardsTon80",
            "Sieve",
            "Storage",
            "Towers"
        ]
    );
}

/// `ns-next bench` times the untimed harness on one benchmark.
#[test]
fn bench_times_one_benchmark_of_the_project() {
    let output = ns_next()
        .arg("bench")
        .arg(harness_project())
        .args(["--iterations", "1", "--", "Sieve"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Sieve: ok"), "{stdout}");
    assert!(stdout.contains("median"), "{stdout}");
}
