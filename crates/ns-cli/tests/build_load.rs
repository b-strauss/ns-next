//! `ns-next build` then `ns-next load` -- the started image, end to end
//! through the binary.

mod common;
use common::{golden, goldens_root, ns_next, ScratchDirectory};
use indoc::indoc;
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

/// The image `build` writes by default carries no program text, and the
/// application in it still runs. `--debug-source` opts the source back in,
/// and `load --print-source` is what reads it out again -- which is how an
/// image can be exported back to `.ns` while no decompiler exists.
#[test]
fn a_built_image_is_source_free_unless_debug_source_is_asked_for() {
    let source = golden("library/number/integer/arithmetic");
    let text = std::fs::read_to_string(&source).expect("read golden");
    // Every line carrying program text, not one hand-picked needle: a
    // partial leak (one method body, one initializer statement) has to fail
    // this too. Lines under 12 characters are structural punctuation --
    // `)`, `) : ()` -- which an image's byte stream reproduces by
    // chance, so checking them would report leaks that are not leaks.
    // See `no_line_of_the_source_survives_in_a_source_free_image` in
    // `ns-runtime`'s image tests for the same property with a module built
    // to probe it, and for what this does *not* claim (identifiers survive
    // in the symbol table by necessity -- dispatch is by name).
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.len() >= 12)
        .collect();
    assert!(lines.len() >= 4, "the golden must have text to look for");

    let plain_directory = ScratchDirectory::new("source-free");
    let plain = plain_directory.join("app.nsi");
    let (ok, _out, error) = ns_next(&["build".as_ref(), source.as_os_str(), plain.as_os_str()]);
    assert!(ok, "{error}");
    let plain_bytes = std::fs::read(&plain).expect("read image");
    for line in &lines {
        assert!(
            !contains(&plain_bytes, line.as_bytes()),
            "the default image contains the source line {line:?}"
        );
    }
    // ...and it still starts, not just a smaller file.
    let (ok, out, error) = ns_next(&["load".as_ref(), plain.as_os_str()]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
    // A source-free image has nothing to print, and says so.
    let (ok, _out, error) = ns_next(&[
        "load".as_ref(),
        plain.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("carries no source"), "{error}");

    let debug_directory = ScratchDirectory::new("debug-source");
    let debug = debug_directory.join("app.nsi");
    let (ok, _out, error) = ns_next(&[
        "build".as_ref(),
        source.as_os_str(),
        debug.as_os_str(),
        "--debug-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    let debug_bytes = std::fs::read(&debug).expect("read image");
    // The control: the very lines missing above are all present here, so
    // the sweep cannot be passing because the needles were mangled.
    for line in &lines {
        assert!(
            contains(&debug_bytes, line.as_bytes()),
            "the debug variant is missing the source line {line:?}"
        );
    }
    assert!(
        debug_bytes.len() > plain_bytes.len(),
        "the debug variant is the bigger of the two"
    );
    let (ok, out, error) = ns_next(&[
        "load".as_ref(),
        debug.as_os_str(),
        "--print-source".as_ref(),
    ]);
    assert!(ok, "{error}");
    assert_eq!(
        out.replace("\r\n", "\n"),
        text.replace("\r\n", "\n"),
        "the debug variant round-trips the module's source verbatim"
    );
    // And the debug variant still starts, exactly as the plain one does.
    let (ok, out, error) = ns_next(&["load".as_ref(), debug.as_os_str()]);
    assert!(ok, "{error}");
    assert_eq!(out, "42\n");
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
                | private Stamper = manifest Stamper.
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
        let origin = SourceOrigin::File("plain.ns".into());
        let modules = [ns_runtime::image::ModuleSource {
            class,
            origin: &origin,
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
