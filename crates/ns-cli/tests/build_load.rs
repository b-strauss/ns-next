//! `ns-next build` then `ns-next load` -- the started image, end to end
//! through the binary.

mod common;
use common::{ScratchDirectory, golden, goldens_root, ns_next};
use indoc::{formatdoc, indoc};
use std::path::Path;

/// `build` then `load` of `program`, compared with `run` of it: the three
/// outputs `run` gives, after asserting that the artifact gives the same.
fn assert_build_load_matches_run(name: &str, program: &Path) -> (bool, String, String) {
    let directory = ScratchDirectory::new(name);
    let out = directory.join("app.nsi");
    let (built, _, build_error) =
        ns_next(&["build".as_ref(), program.as_os_str(), out.as_os_str()]);
    assert!(built, "{name}: {build_error}");
    let run = ns_next(&["run".as_ref(), program.as_os_str()]);
    let load = ns_next(&["load".as_ref(), out.as_os_str()]);
    assert_eq!(
        load, run,
        "{name}: load of the build must answer what run answers"
    );
    run
}

#[test]
fn build_writes_a_started_image_that_inspect_recognizes() {
    let directory = ScratchDirectory::new("build");
    let out = directory.join("app.nsi");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("library/number/integer/arithmetic").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{error}");

    // Read through `ns-runtime`'s inspector, the listing
    // `ns-next-analyzer inspect` prints, because a test starts only its own
    // crate's binary.
    let bytes = std::fs::read(&out).expect("read the image");
    let report = match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => report,
        Err(error) => panic!("{error}"),
    };
    let listing = ns_runtime::image::inspect::render(&report);
    assert!(listing.contains("start: yes"), "{listing}");
    assert!(!listing.contains("stateless"), "{listing}");
    assert!(listing.contains("main:args:"), "{listing}");
    // The application travels as the graph's one root.
    assert!(listing.contains("graph"), "{listing}");
}

// spec: image.load
#[test]
fn a_built_image_runs_exactly_as_its_source_does() {
    let (ok, out, error) =
        assert_build_load_matches_run("equivalence", &golden("library/number/integer/arithmetic"));
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
}

/// The CLI's `--lowering` flag value for `lowering`: `build_load.rs` drives
/// the `ns-next` binary as a subprocess, so `for_each_lowering!`'s
/// `ns_runtime::Lowering` value must be translated to the flag's own
/// spelling rather than used directly.
fn cli_lowering_flag(lowering: ns_runtime::Lowering) -> &'static str {
    match lowering {
        ns_runtime::Lowering::Optimized => "optimized",
        ns_runtime::Lowering::Reference => "reference",
    }
}

ns_runtime::for_each_lowering! {
    /// A module applying mixins (`newspeak-spec.pdf` §6.2) built into an
    /// image answers exactly what its source answers: the image carries each
    /// declaration's `<:` clauses -- their selectors and compiled messages --
    /// so the loaded world builds every applied level itself and initializes
    /// it with its own clause's message, evaluated with the applying class's
    /// factory argument.
    fn a_built_image_applying_mixins_runs_exactly_as_its_source_does(lowering) {
        let lowering = cli_lowering_flag(lowering);
        let directory = ScratchDirectory::new("mixin-application");
        let source = directory.join("applying.ns");
        std::fs::write(
            &source,
            indoc! {"
                class Applying = Object new () (
                    class Base withBase: b = Object new (| base = b. |) (
                        public chain = ( ^'Base ', base printString )
                    )
                    class Marker withMark: m = Object new (| mark = m. |) (
                        public chain = ( ^'Marker ', mark printString, ' ', super chain )
                    )
                    class Stamp = Object new () (
                        public chain = ( ^'Stamp ', super chain )
                    )
                    class Applied from: x = Base withBase: x + 1 <: Marker withMark: x * 10 <: Stamp (
                    ) (
                        public chain = ( ^'Applied ', super chain )
                    )
                    public main: platform args: arguments = (
                        | t = platform transcript. |
                        t show: (Applied from: 4) chain; newLine.
                        t show: (Applied from: 2) chain; newLine.
                    )
                ) : ()
            "},
        )
        .expect("write the module");
        let out = directory.join(&format!("applying.{lowering}.nsi"));
        let (built, _, build_error) = ns_next(&[
            "build".as_ref(),
            source.as_os_str(),
            out.as_os_str(),
            "--lowering".as_ref(),
            lowering.as_ref(),
        ]);
        assert!(built, "{lowering}: {build_error}");
        let (run_ok, run_out, run_error) = ns_next(&[
            "run".as_ref(),
            source.as_os_str(),
            "--lowering".as_ref(),
            lowering.as_ref(),
        ]);
        assert!(
            run_ok,
            "{lowering}: the module runs from source: {run_error}"
        );
        assert_eq!(
            run_out, "Applied Stamp Marker 40 Base 5\nApplied Stamp Marker 20 Base 3\n",
            "{lowering}: the source's own answer"
        );
        let (load_ok, load_out, load_error) = ns_next(&["load".as_ref(), out.as_os_str()]);
        assert_eq!(
            (load_ok, load_out, load_error),
            (run_ok, run_out, run_error),
            "{lowering}: the image answers what the source answers"
        );
    }
}

/// instance_init.ns writes through its initializer, which `build` runs; the
/// loaded application sees the slots it filled, and `load` runs no
/// initializer again.
// spec: image.load.start
// spec: image.write.started
#[test]
fn a_loaded_application_does_not_rerun_its_initializers() {
    let (ok, out, error) = assert_build_load_matches_run(
        "init",
        &golden("language/class/instantiation/instance_init"),
    );
    assert!(ok, "{error}");
    assert_eq!(
        out,
        common::expectation_of(&golden("language/class/instantiation/instance_init")).stdout
    );
}

/// The initializer draws an identity hash while `build` assembles the
/// application. A loader that re-ran it would draw a fresh one past the
/// hashes the image records, so `stamp` would differ from what `run` prints.
// spec: image.load.start
#[test]
fn an_initializer_is_not_rerun_by_load_and_draws_the_same_hash() {
    let directory = ScratchDirectory::new("instance-init");
    let source = directory.join("stamp.ns");
    std::fs::write(
        &source,
        indoc! {"
            class Stamp = Object new (| stamp = Object new hash. |) (
                public main: platform args: arguments = (
                    platform transcript show: stamp printString; newLine
                )
            ) : ()
        "},
    )
    .expect("write source");
    let (ok, out, error) = assert_build_load_matches_run("instance-init", &source);
    assert!(ok, "{error}");
    assert!(!out.is_empty(), "run printed the stamp");
}

/// The failing direction, asserted on rather than merely swept: `run`
/// writes nothing to stdout and stderr opens with the uncaught-error line.
/// The sweep in `golden_round_trips.rs` proves the two paths agree; this
/// proves what they agree *on*, so a change that made both report nothing
/// could not pass. The line is written by the shared `report_entry_send` on
/// both paths; this is what pins that it stays shared.
#[test]
fn a_load_reports_an_uncaught_error_exactly_as_run_does() {
    let directory = ScratchDirectory::new("raises");
    let source = directory.join("raises.ns");
    std::fs::write(
        &source,
        indoc! {"
            class Raises = Object new (
            ) (
                public main: platform args: arguments = (
                    ^nil frobnicate
                )
            ) : ()
        "},
    )
    .expect("write source");
    let (ok, out, error) = assert_build_load_matches_run("raises", &source);
    assert!(!ok, "the program raises, so `run` exits non-zero");
    assert_eq!(out, "", "it prints nothing before raising");
    assert!(
        error.starts_with("Uncaught Newspeak error: "),
        "the error text is what both paths must agree on: {error}"
    );
}

/// The image `build` writes carries the program's text, and `--strip-source`
/// leaves it out: the application in a stripped image still runs, but it
/// cannot be edited and `load --print-source` has nothing to print. The
/// carried text is also what exporting an image back to `.ns` rests on;
/// there is no decompiler.
#[test]
fn a_built_image_carries_its_source_unless_stripped() {
    let source = golden("library/number/integer/arithmetic");
    let text = std::fs::read_to_string(&source).expect("read golden");
    // Every line carrying program text, not one hand-picked needle: a
    // partial leak (one method body, one initializer statement) has to fail
    // the stripped half too. Lines under 12 characters are structural
    // punctuation -- `)`, `) : ()` -- which an image's byte stream
    // reproduces by chance, so checking them would report leaks that are not
    // leaks. See `no_line_of_the_source_survives_in_a_source_free_image` in
    // `ns-runtime`'s image tests for the same property with a module built
    // to probe it, and for what this does *not* claim (identifiers survive
    // in the symbol table by necessity -- dispatch is by name).
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.len() >= 12)
        .collect();
    assert!(lines.len() >= 4, "the golden must have text to look for");

    let default_directory = ScratchDirectory::new("with-source");
    let default = default_directory.join("app.nsi");
    let (ok, _out, error) = ns_next(&["build".as_ref(), source.as_os_str(), default.as_os_str()]);
    assert!(ok, "{error}");
    let default_bytes = std::fs::read(&default).expect("read image");
    for line in &lines {
        assert!(
            contains(&default_bytes, line.as_bytes()),
            "the default image is missing the source line {line:?}"
        );
    }
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        default.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out.replace("\r\n", "\n"),
        text.replace("\r\n", "\n"),
        "the default image round-trips the module's source verbatim"
    );
    let (ok, out, error) = ns_next(&["load".as_ref(), default.as_os_str()]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");

    let stripped_directory = ScratchDirectory::new("strip-source");
    let stripped = stripped_directory.join("app.nsi");
    let (ok, _out, error) = ns_next(&[
        "build".as_ref(),
        source.as_os_str(),
        stripped.as_os_str(),
        "--strip-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    let stripped_bytes = std::fs::read(&stripped).expect("read image");
    // The control above holds every line present in the default image, so
    // this sweep cannot be passing because the needles were mangled.
    for line in &lines {
        assert!(
            !contains(&stripped_bytes, line.as_bytes()),
            "the stripped image contains the source line {line:?}"
        );
    }
    assert!(
        stripped_bytes.len() < default_bytes.len(),
        "the stripped image is the smaller of the two"
    );
    // ...and it still starts, not just a smaller file.
    let (ok, out, error) = ns_next(&["load".as_ref(), stripped.as_os_str()]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
    // A stripped image has nothing to print, and says so.
    let (ok, _out, error) = ns_next(&[
        "load".as_ref(),
        stripped.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(!ok);
    assert!(
        error.contains("carries no source (it was written with --strip-source)"),
        "{error}"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// The slot initializers ask a builtin class, the module class and its
/// metaclass for their hashes while `build` assembles the application; the
/// load answers the same three questions against the builtin it names, its
/// hash in the image's tag-named hashes, and the two records it decoded,
/// their hashes in their headers. Had the image dropped them, all three
/// would print `false`.
///
/// `stamp` is drawn and never checked: it gives the identity-hash counter a
/// fourth, ordinary draw after the three recorded ones, so that a load
/// which forgot the recorded hashes would draw different numbers rather than
/// the same first three by coincidence (image-format.md, "Identity hashes
/// are preserved verbatim").
// spec: image.load.hashes
#[test]
fn a_hash_given_to_a_tag_named_object_survives_build_and_load() {
    let directory = ScratchDirectory::new("tag-named-hashes");
    let source = directory.join("hashes.ns");
    std::fs::write(
        &source,
        indoc! {"
            class Hashes = Object new (
                | integer = Integer hash. module = self class hash. meta = self class metaclass hash. stamp = Object new hash. |
            ) (
                public main: platform args: arguments = (
                    platform transcript
                        show: (integer = Integer hash) printString; show: ' ';
                        show: (module = self class hash) printString; show: ' ';
                        show: (meta = self class metaclass hash) printString; newLine
                )
            ) : (
                public metaclass = ( ^self class )
            )
        "},
    )
    .expect("write source");
    let (ok, out, error) = assert_build_load_matches_run("tag-named-hashes", &source);
    assert!(ok, "{error}");
    assert_eq!(out, "true true true\n");
}

/// Module order is what keeps identity hashes identical between `run` and
/// `load` (the root first, then the project's modules as written, then the
/// manifest): the root and a module each draw a hash while `build`
/// assembles the application, and the module class's own hash is drawn only
/// in `main:args:`, after the load. A loader that rebuilt the modules in
/// another order, or re-drew a recorded hash, would print a different line.
#[test]
fn two_modules_draw_the_same_identity_hashes_under_run_and_load() {
    let directory = ScratchDirectory::new("two-module-hashes");
    std::fs::write(
        directory.join("App.ns"),
        indoc! {"
            class App packageUsing: manifest = Object new (
                | private Stamper = manifest at: 'Stamper'.
                  stamp = Object new hash.
                  stamper = Stamper new. |
            ) (
                public main: platform args: arguments = (
                    platform transcript
                        show: stamp printString; show: ' ';
                        show: stamper stamp printString; show: ' ';
                        show: Stamper hash printString; newLine
                )
            ) : ()
        "},
    )
    .expect("write the root");
    std::fs::write(
        directory.join("Stamper.ns"),
        indoc! {"
            class Stamper = Object new (| public stamp = Object new hash. |) () : ()
        "},
    )
    .expect("write the module");
    let project = directory.join("newspeak.json");
    std::fs::write(
        &project,
        indoc! {r#"
            {
              "main": "App.ns",
              "modules": ["Stamper.ns"]
            }
        "#},
    )
    .expect("write the project file");
    let (ok, out, error) = assert_build_load_matches_run("two-module-hashes", &project);
    assert!(ok, "{error}");
    let hashes: Vec<&str> = out.trim_end().split(' ').collect();
    assert_eq!(hashes.len(), 3, "three hashes: {out}");
}

/// `build` assembles the application as `run` does, so it warns of a
/// request that matched several versions as `run` does; `load` does not
/// assemble it again, and the image runs the version `build` took.
#[test]
fn build_warns_of_a_request_that_matched_several_versions() {
    let directory = ScratchDirectory::new("build-ambiguity");
    std::fs::write(
        directory.join("App.ns"),
        indoc! {"
            class App packageUsing: manifest = Object new (
              | private Util = manifest at: 'com.acme.util.Util' version: '2'. |
            ) (
              public main: platform args: arguments = (
                platform transcript show: Util new answer; newLine
              )
            ) : ()
        "},
    )
    .expect("write the root");
    for (version, answer) in [("2.1.0", "older"), ("2.3.1", "newer")] {
        let library = directory.join(&format!("libs/{version}"));
        std::fs::create_dir_all(&library).expect("create the library directory");
        std::fs::write(
            library.join("Util.ns"),
            formatdoc! {"
                (* :module: com.acme.util.Util@{version} *)
                class Util = Object new () (
                  public answer = ( ^'{answer}' )
                ) : ()
            "},
        )
        .expect("write a version");
    }
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "App.ns", "modules": ["libs/2.3.1/Util.ns", "libs/2.1.0/Util.ns"] }"#,
    )
    .expect("write the project file");
    let image = directory.join("app.nsi");
    let built = ns_next(&[
        "build".as_ref(),
        directory.path().as_os_str(),
        image.as_os_str(),
    ]);
    assert!(built.0, "build failed: {}", built.2);
    // Only the warning is held: a JIT that is off says so first.
    let warning = "warning: `com.acme.util.Util` version 2 matched 2.1.0, 2.3.1; took 2.3.1\n";
    assert!(built.2.ends_with(warning), "build's stderr: {}", built.2);
    assert_eq!(built.2.matches("warning: `").count(), 1, "{}", built.2);
    let loaded = ns_next(&["load".as_ref(), image.as_os_str()]);
    assert!(loaded.0, "load failed: {}", loaded.2);
    assert_eq!(loaded.1, "newer\n");
    assert!(!loaded.2.contains("matched"), "load's stderr: {}", loaded.2);
}

/// An image names its modules as the program did: each by its path in the
/// project, its id -- after an `as` -- and its version, the manifest by
/// its `core:` path and its `newspeak.core` id, and never by a directory
/// of the machine that built it, even without its source.
// spec: image.load.modules
#[test]
fn load_names_modules_by_their_identity() {
    let directory = ScratchDirectory::new("build-identity");
    let root = std::path::absolute(directory.path()).expect("an absolute directory");
    std::fs::write(
        root.join("App.ns"),
        indoc! {"
            class App packageUsing: manifest = Object new (
              | private Util = manifest at: 'com.acme.util.Util' version: '1'. |
            ) (
              public main: platform args: arguments = (
                platform transcript show: Util new answer; newLine
              )
            ) : ()
        "},
    )
    .expect("write the root");
    for (version, answer) in [("1.4.0", "one"), ("2.0.0-beta.1", "two")] {
        let library = root.join(format!("libs/util-{version}"));
        std::fs::create_dir_all(&library).expect("create the library directory");
        std::fs::write(
            library.join("Util.ns"),
            formatdoc! {"
                (* :module: com.acme.util.Util@{version} *)
                class Util = Object new () (
                  public answer = ( ^'{answer}' )
                ) : ()
            "},
        )
        .expect("write a version");
    }
    std::fs::write(
        root.join("Plain.ns"),
        indoc! {"
            class Plain = Object new () () : ()
        "},
    )
    .expect("write the module given an id");
    std::fs::write(
        root.join("newspeak.json"),
        r#"{ "main": "App.ns", "modules": ["libs/util-1.4.0/Util.ns", "libs/util-2.0.0-beta.1/Util.ns", { "path": "Plain.ns", "as": "org.example.Plain" }] }"#,
    )
    .expect("write the project file");
    let image = root.join("app.nsi");
    let built = ns_next(&[
        "build".as_ref(),
        root.as_os_str(),
        image.as_os_str(),
        "--strip-source".as_ref(),
    ]);
    assert!(built.0, "build failed: {}", built.2);
    let bytes = std::fs::read(&image).expect("read the image");
    let loaded = match ns_runtime::image::load(&bytes) {
        Ok(loaded) => loaded,
        Err(error) => panic!("{error}"),
    };
    let names: Vec<(String, String)> = loaded
        .modules
        .iter()
        .map(|module| {
            assert_eq!(
                module.declaration.module_identity.as_deref(),
                Some(&*module.identity),
                "the root mixin carries the module's identity"
            );
            (module.identity.path.clone(), module.identity.display_name())
        })
        .collect();
    let expected = [
        ("App.ns", "App"),
        ("libs/util-1.4.0/Util.ns", "com.acme.util.Util@1.4.0"),
        (
            "libs/util-2.0.0-beta.1/Util.ns",
            "com.acme.util.Util@2.0.0-beta.1",
        ),
        ("Plain.ns", "org.example.Plain"),
        ("core:Manifest.ns", "newspeak.core.Manifest"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(path, name)| ((*path).to_string(), (*name).to_string()))
        .collect();
    assert_eq!(names, expected);
    let written = root.to_string_lossy();
    assert!(
        !contains(&bytes, written.as_bytes()),
        "the image holds the building machine's directory {written}"
    );
    let loaded = ns_next(&["load".as_ref(), image.as_os_str()]);
    assert!(loaded.0, "load failed: {}", loaded.2);
    assert_eq!(loaded.1, "one\n");
}

#[test]
fn a_project_builds_and_loads_as_it_runs() {
    let project =
        goldens_root().join("tool/manifest/a_project_reaches_its_modules_through_the_manifest");
    let (ok, out, error) = assert_build_load_matches_run("project", &project);
    assert!(ok, "{error}");
    assert_eq!(out, "hello, modules\n2\n");
}

/// `image::snapshot` is a host's image, not an application `build`
/// packaged: it carries no start, so there is nothing for `load` to send
/// `main:args:` to.
// spec: image.load
#[test]
fn load_refuses_an_image_that_is_not_started() {
    use ns_runtime::{Interpreter, SourceOrigin};
    use std::rc::Rc;

    let source = indoc! {"
        class Plain = Object new () (
            public main: platform args: arguments = ( platform transcript show: 'ran'; newLine )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File("plain.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let bytes = interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(&class).as_object() else {
            panic!("load_module answers a class object")
        };
        let identity = ns_runtime::modules::ModuleIdentity::undeclared("plain.ns", "Plain");
        let modules = [ns_runtime::image::ModuleSource {
            class,
            identity: &identity,
            with_source: false,
        }];
        ns_runtime::image::snapshot(mutator, &modules, &[]).expect("snapshot")
    });
    let directory = ScratchDirectory::new("not-started");
    let image = directory.join("world.img");
    std::fs::write(&image, bytes).expect("write image");

    let (ok, out, error) = ns_next(&["load".as_ref(), image.as_os_str()]);
    assert!(!ok, "a snapshot is not started: {out}");
    assert_eq!(out, "", "nothing ran");
    assert!(error.contains("not a started image"), "{error}");
}

/// A host command whose own output finds the standard output closed --
/// `ns-next load app.nsi --print-source | head -1` -- stops as a program's
/// run does: one report line and status 1, never a panic.
#[test]
fn a_closed_standard_output_ends_print_source() {
    let directory = ScratchDirectory::new("closed-print-source");
    let image = directory.join("app.nsi");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("library/number/integer/arithmetic").as_os_str(),
        image.as_os_str(),
    ]);
    assert!(ok, "{error}");
    let mut child = common::target_command(env!("CARGO_BIN_EXE_ns-next"))
        .arg("load")
        .arg(&image)
        .arg("--print-source")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run ns-next");
    // Closed before the child, which first decodes the image, writes.
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("wait for ns-next");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("error: the standard output is closed"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}
