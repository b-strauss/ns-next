//! `ns-next build` -- the stateless deployment binary.

mod common;
use common::{golden, ns_next, ScratchDirectory};
use indoc::indoc;
use std::path::Path;

#[test]
fn build_writes_a_stateless_binary_that_inspect_recognizes() {
    let directory = ScratchDirectory::new("build");
    let out = directory.join("app.nsb");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("language/arithmetic").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{error}");

    let (ok, listing, error) = ns_next(&["inspect".as_ref(), out.as_os_str()]);
    assert!(ok, "{error}");
    assert!(listing.contains("stateless: yes"), "{listing}");
    assert!(listing.contains("main:args:"), "{listing}");
    // The whole point of the artifact: no instance state travelled with it.
    assert!(!listing.contains("graph"), "{listing}");
}

#[test]
fn build_refuses_an_entry_the_root_class_does_not_have() {
    let directory = ScratchDirectory::new("bad-entry");
    let out = directory.join("app.nsb");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("language/arithmetic").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "noSuchSelector".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("noSuchSelector"), "{error}");
    assert!(!out.exists(), "a refused build must write no file");
}

/// `build` accepts an entry only where `mixins::encode` has interned its
/// selector (`commands/build.rs`, `root_class_answers`): a method or nested
/// class of the module's own. The root class inherits from `Object` and
/// declares no `printString`, which only `Object`'s kernel method answers --
/// a method `Kernel.ns` declares and no table of the module names -- so the
/// walk stops at `Object`'s builtin level and the build is refused.
#[test]
fn build_refuses_an_entry_only_a_builtin_answers() {
    let directory = ScratchDirectory::new("builtin-entry");
    let out = directory.join("app.nsb");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("language/arithmetic").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "printString".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("does not answer `printString`"), "{error}");
    assert!(!out.exists(), "a refused build must write no file");
}

#[test]
fn build_refuses_an_entry_arity_the_cli_cannot_supply() {
    let directory = ScratchDirectory::new("bad-arity");
    let out = directory.join("app.nsb");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        golden("language/arithmetic").as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "a:b:c:".as_ref(),
    ]);
    assert!(!ok);
    assert!(error.contains("argument"), "{error}");
}

#[test]
fn build_requires_an_entry_when_the_root_class_declares_no_main() {
    // TestActor.ns answers `return42` and declares no `main:args:`.
    let directory = ScratchDirectory::new("no-main");
    let out = directory.join("app.nsb");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, error) = ns_next(&["build".as_ref(), source.as_os_str(), out.as_os_str()]);
    assert!(!ok);
    assert!(error.contains("--entry"), "{error}");
}

#[test]
fn a_built_binary_runs_exactly_as_its_source_does() {
    let directory = ScratchDirectory::new("equivalence");
    let out = directory.join("app.nsb");
    let source = golden("language/arithmetic");

    let (ok, _, error) = ns_next(&["build".as_ref(), source.as_os_str(), out.as_os_str()]);
    assert!(ok, "{error}");

    let (run_ok, run_out, run_error) = ns_next(&["run".as_ref(), source.as_os_str()]);
    let (load_ok, load_out, load_error) = ns_next(&["load".as_ref(), out.as_os_str()]);

    assert_eq!(load_ok, run_ok, "exit status differs");
    assert_eq!(load_out, run_out, "stdout differs");
    assert_eq!(load_error, run_error, "stderr differs");
}

/// A module applying mixins (`newspeak-spec.pdf` §6.2) built into a binary
/// answers exactly what its source answers: the binary carries each
/// declaration's `<:` clauses -- their selectors and compiled messages -- so
/// the loaded world builds every applied level itself and initializes it
/// with its own clause's message, evaluated with the applying class's
/// factory argument.
#[test]
fn a_built_binary_applying_mixins_runs_exactly_as_its_source_does() {
    let directory = ScratchDirectory::new("mixin-application");
    let source = directory.join("applying.ns");
    std::fs::write(
        &source,
        indoc! {"
            Newspeak3
            'Test'
            class Applying usingPlatform: p = Object new (| t = p transcript. |) (
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
                    t show: (Applied from: 4) chain; newLine.
                    t show: (Applied from: 2) chain; newLine.
                )
            ) : ()
        "},
    )
    .expect("write the module");
    for lowering in ["optimized", "reference"] {
        let out = directory.join(&format!("applying.{lowering}.nsb"));
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
            "{lowering}: the binary answers what the source answers"
        );
    }
}

#[test]
fn loading_a_binary_runs_its_initializers_against_the_loading_platform() {
    // instance_init.ns writes through its initializer, so if the factory and
    // initializers did not run on load the entry send would see nil slots.
    let directory = ScratchDirectory::new("init");
    let out = directory.join("app.nsb");
    let source = golden("language/instance_init");
    let (ok, _, error) = ns_next(&["build".as_ref(), source.as_os_str(), out.as_os_str()]);
    assert!(ok, "{error}");

    let (run_ok, run_out, _) = ns_next(&["run".as_ref(), source.as_os_str()]);
    let (load_ok, load_out, _) = ns_next(&["load".as_ref(), out.as_os_str()]);
    assert_eq!((load_ok, load_out), (run_ok, run_out));
}

/// For one source and one --debug-source setting, a build and a snapshot must
/// agree byte for byte from `origin` through `root class`.
///
/// Not "the binary is a prefix of the image" -- that is false as soon as one
/// records an entry and the other does not. The guarantee is about the
/// sections that describe code and structure, and it is what keeps the shared
/// section writers honest.
#[test]
fn a_build_and_a_snapshot_agree_on_the_describing_sections() {
    let directory = ScratchDirectory::new("prefix");
    let source = golden("language/arithmetic");
    let binary = directory.join("app.nsb");
    let image = directory.join("world.img");

    let (ok, _, error) = ns_next(&["build".as_ref(), source.as_os_str(), binary.as_os_str()]);
    assert!(ok, "{error}");
    let (ok, _, error) = ns_next(&["snapshot".as_ref(), source.as_os_str(), image.as_os_str()]);
    assert!(ok, "{error}");

    let binary_bytes = std::fs::read(&binary).expect("read binary");
    let image_bytes = std::fs::read(&image).expect("read image");

    // The window starts at `origin`, not at byte 0: the header's flag byte
    // legitimately differs (a build is stateless, a snapshot is not), so
    // starting from the header would fail on that difference instead of on
    // the property this test actually guards.
    let window = |bytes: &[u8]| {
        let report = ns_runtime::image::inspect::inspect(bytes).expect("inspect");
        let start = report
            .sections
            .iter()
            .find(|section| section.name == "origin")
            .expect("origin section")
            .start;
        let end = report
            .sections
            .iter()
            .find(|section| section.name == "root class")
            .expect("root class section")
            .end;
        (start, end)
    };
    let (binary_start, binary_end) = window(&binary_bytes);
    let (image_start, image_end) = window(&image_bytes);
    assert_eq!(
        (binary_start, binary_end),
        (image_start, image_end),
        "the describing sections start or end at different offsets"
    );
    assert_eq!(
        &binary_bytes[binary_start..binary_end],
        &image_bytes[image_start..image_end],
        "describing bytes differ; run `ns-next inspect` on both to see which section"
    );
}

#[test]
fn load_send_overrides_the_recorded_entry() {
    // The recorded entry (`return42`) and the `--send` override
    // (`signalError`) must be *different* selectors with observably
    // different outcomes: `return42` exits cleanly, `signalError` sends
    // `Error signal:` and fails. Using the same selector on both sides
    // would pass whether or not `--send` actually takes precedence.
    let directory = ScratchDirectory::new("override");
    let out = directory.join("app.nsb");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules/TestActor.ns");
    let (ok, _, error) = ns_next(&[
        "build".as_ref(),
        source.as_os_str(),
        out.as_os_str(),
        "--entry".as_ref(),
        "return42".as_ref(),
    ]);
    assert!(ok, "{error}");

    // If `--send` did not override the recorded entry, this would silently
    // run `return42` instead and exit successfully with no error message --
    // so both the failing exit and the message are asserted, not just one.
    let (ok, _, error) = ns_next(&[
        "load".as_ref(),
        out.as_os_str(),
        "--send".as_ref(),
        "signalError".as_ref(),
    ]);
    assert!(
        !ok,
        "expected `--send signalError` to override the recorded `return42` entry"
    );
    assert!(
        error.contains("Uncaught Newspeak error"),
        "expected signalError's error to surface, got: {error}"
    );
}
