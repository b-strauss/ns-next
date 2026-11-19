//! The manifest a project's root receives answers each module by its id,
//! and by a range of its versions where several share the id
//! (`docs/design/modules.md`, decisions 3 and 4).

use indoc::indoc;
use ns_runtime::{HostResult, Interpreter, Root, SourceOrigin, Symbol, Value};

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(ns_runtime::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(ns_runtime::HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The message of the error `result` signalled.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: the manifest answered"),
        Err(ns_runtime::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(ns_runtime::HostSignal::Error(error)) => error.message,
    }
}

fn load(interpreter: &mut Interpreter, source: &str, origin: SourceOrigin) -> Root {
    let module = ns_syntax::parse(source).unwrap_or_else(|_| panic!("parse"));
    let resolved =
        ns_runtime::resolve(module, source.into(), origin).unwrap_or_else(|_| panic!("resolve"));
    interpreter.load_module(&resolved)
}

/// One module the manifest holds: its id, its version's text or none, its
/// rank among the modules of its id, and its definition.
struct Held<'module> {
    id: &'static str,
    version: Option<&'static str>,
    rank: i64,
    definition: &'module Root,
}

/// A manifest over `modules`, as the host makes one.
fn manifest_over(interpreter: &mut Interpreter, modules: &[Held<'_>]) -> Root {
    let manifest_class = load(
        interpreter,
        ns_runtime::MANIFEST_SOURCE,
        SourceOrigin::Embedded("Manifest.ns".into()),
    );
    let arguments = interpreter.enter(|mutator| {
        let ids = modules
            .iter()
            .map(|module| Value::object(mutator.new_string(module.id.to_string())))
            .collect();
        let versions = modules
            .iter()
            .map(|module| match module.version {
                Some(version) => Value::object(mutator.new_string(version.to_string())),
                None => Value::nil(),
            })
            .collect();
        let ranks = modules
            .iter()
            .map(|module| Value::small_integer(module.rank))
            .collect();
        let definitions = modules
            .iter()
            .map(|module| mutator.fetch(module.definition))
            .collect();
        [ids, versions, ranks, definitions].map(|elements| {
            let array = mutator.new_array(elements);
            mutator.stash(Value::object(array))
        })
    });
    expect_ok(
        interpreter.instantiate(&manifest_class, &arguments),
        "Manifest ids:versions:ranks:definitions:",
    )
}

fn module(interpreter: &mut Interpreter, file: &str) -> Root {
    load(
        interpreter,
        indoc! {"
            class Util = Object new () () : ()
        "},
        SourceOrigin::File(file.into()),
    )
}

fn assert_same(interpreter: &mut Interpreter, answer: &Root, expected: &Root) {
    interpreter.enter(|mutator| {
        match (
            mutator.fetch(answer).as_object(),
            mutator.fetch(expected).as_object(),
        ) {
            (Some(left), Some(right)) => assert!(left.same(right)),
            _ => panic!("the manifest answered no class"),
        }
    });
}

fn string(interpreter: &mut Interpreter, text: &str) -> Root {
    interpreter.enter(|mutator| {
        let string = mutator.new_string(text.to_string());
        mutator.stash(Value::object(string))
    })
}

fn ask_at(interpreter: &mut Interpreter, manifest: &Root, id: &str) -> HostResult {
    let id = string(interpreter, id);
    interpreter.send(manifest, Symbol::intern("at:"), &[id])
}

fn ask_at_version(
    interpreter: &mut Interpreter,
    manifest: &Root,
    id: &str,
    range: &str,
) -> HostResult {
    let id = string(interpreter, id);
    let range = string(interpreter, range);
    interpreter.send(manifest, Symbol::intern("at:version:"), &[id, range])
}

/// The texts of the manifest's `ambiguities`.
fn ambiguities(interpreter: &mut Interpreter, manifest: &Root) -> Vec<String> {
    let answer = expect_ok(
        interpreter.send(manifest, Symbol::intern("ambiguities"), &[]),
        "ambiguities",
    );
    interpreter.enter(|mutator| {
        let array = mutator
            .fetch(&answer)
            .as_object()
            .unwrap_or_else(|| panic!("ambiguities answered no Array"));
        mutator
            .heap()
            .array_elements(array)
            .unwrap_or_else(|| panic!("ambiguities answered no Array"))
            .into_iter()
            .map(|element| {
                element
                    .as_object()
                    .and_then(|text| mutator.heap().str_value(text))
                    .unwrap_or_else(|| panic!("an ambiguity is no String"))
            })
            .collect()
    })
}

#[test]
fn a_manifest_answers_a_module_definition_by_its_id() {
    let mut interpreter = Interpreter::bootstrap();
    let util = module(&mut interpreter, "Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[Held {
            id: "com.acme.Util",
            version: None,
            rank: 0,
            definition: &util,
        }],
    );
    let answer = expect_ok(ask_at(&mut interpreter, &manifest, "com.acme.Util"), "at:");
    assert_same(&mut interpreter, &answer, &util);
    let message = expect_error(ask_at(&mut interpreter, &manifest, "Util"), "at: 'Util'");
    assert_eq!(message, "no module `Util`");
}

/// A request no module answers names what there is: nothing for an id no
/// module has, and the versions of the id for a range none of them lies in.
#[test]
fn at_without_a_module_signals_with_the_candidates() {
    let mut interpreter = Interpreter::bootstrap();
    let first = module(&mut interpreter, "a/Util.ns");
    let second = module(&mut interpreter, "b/Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[
            Held {
                id: "Util",
                version: Some("2.0.0"),
                rank: 1,
                definition: &second,
            },
            Held {
                id: "Util",
                version: Some("1.0.0"),
                rank: 0,
                definition: &first,
            },
        ],
    );
    let message = expect_error(ask_at(&mut interpreter, &manifest, "Missing"), "at:");
    assert_eq!(message, "no module `Missing`");
    let message = expect_error(
        ask_at_version(&mut interpreter, &manifest, "Missing", "1"),
        "at:version: of an unknown id",
    );
    assert_eq!(message, "no module `Missing`");
    let message = expect_error(
        ask_at_version(&mut interpreter, &manifest, "Util", "3"),
        "at:version: of an unmatched range",
    );
    assert_eq!(
        message,
        "no module `Util` of version 3; there are 1.0.0, 2.0.0"
    );
    let answer = expect_ok(
        ask_at_version(&mut interpreter, &manifest, "Util", "1.0.0"),
        "at:version: 1.0.0",
    );
    assert_same(&mut interpreter, &answer, &first);
    assert!(ambiguities(&mut interpreter, &manifest).is_empty());
}

/// `at:` cannot pick among several versions; `at:version:` takes the
/// highest that matches and records that it had to choose.
#[test]
fn at_on_several_versions_asks_for_a_version() {
    let mut interpreter = Interpreter::bootstrap();
    let older = module(&mut interpreter, "a/Util.ns");
    let newer = module(&mut interpreter, "b/Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[
            Held {
                id: "com.acme.Util",
                version: Some("2.3.1"),
                rank: 1,
                definition: &newer,
            },
            Held {
                id: "com.acme.Util",
                version: Some("2.1.0"),
                rank: 0,
                definition: &older,
            },
        ],
    );
    let message = expect_error(ask_at(&mut interpreter, &manifest, "com.acme.Util"), "at:");
    assert_eq!(
        message,
        "`com.acme.Util` names 2 modules; ask with at:version:"
    );
    let answer = expect_ok(
        ask_at_version(&mut interpreter, &manifest, "com.acme.Util", "2"),
        "at:version: 2",
    );
    assert_same(&mut interpreter, &answer, &newer);
    assert_eq!(
        ambiguities(&mut interpreter, &manifest),
        ["`com.acme.Util` version 2 matched 2.1.0, 2.3.1; took 2.3.1"]
    );
}

/// A prefix range matches at a dot only, and no pre-release; a full
/// version matches only itself.
#[test]
fn a_range_is_a_prefix_at_a_dot_without_a_pre_release() {
    let mut interpreter = Interpreter::bootstrap();
    let twenty = module(&mut interpreter, "a/Util.ns");
    let beta = module(&mut interpreter, "b/Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[
            Held {
                id: "Util",
                version: Some("20.0.0"),
                rank: 1,
                definition: &twenty,
            },
            Held {
                id: "Util",
                version: Some("2.1.0-beta.1"),
                rank: 0,
                definition: &beta,
            },
        ],
    );
    for range in ["2", "2.1", "2.1.0", "2.1.0-beta"] {
        let message = expect_error(
            ask_at_version(&mut interpreter, &manifest, "Util", range),
            range,
        );
        assert_eq!(
            message,
            format!("no module `Util` of version {range}; there are 2.1.0-beta.1, 20.0.0")
        );
    }
    let answer = expect_ok(
        ask_at_version(&mut interpreter, &manifest, "Util", "2.1.0-beta.1"),
        "the beta",
    );
    assert_same(&mut interpreter, &answer, &beta);
    let answer = expect_ok(
        ask_at_version(&mut interpreter, &manifest, "Util", "20"),
        "20",
    );
    assert_same(&mut interpreter, &answer, &twenty);
}

/// A module without a version is found by `at:` and by no range.
#[test]
fn a_module_without_a_version_matches_no_range() {
    let mut interpreter = Interpreter::bootstrap();
    let util = module(&mut interpreter, "Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[Held {
            id: "Util",
            version: None,
            rank: 0,
            definition: &util,
        }],
    );
    let message = expect_error(
        ask_at_version(&mut interpreter, &manifest, "Util", "1"),
        "1",
    );
    assert_eq!(
        message,
        "no module `Util` of version 1; there are no versions"
    );
    // The host never pairs a versionless module with another of its id, but
    // the message still reads as a sentence for a manifest made by hand.
    let other = module(&mut interpreter, "other/Util.ns");
    let manifest = manifest_over(
        &mut interpreter,
        &[
            Held {
                id: "Util",
                version: None,
                rank: 0,
                definition: &util,
            },
            Held {
                id: "Util",
                version: Some("1.0.0"),
                rank: 1,
                definition: &other,
            },
        ],
    );
    let message = expect_error(
        ask_at_version(&mut interpreter, &manifest, "Util", "2"),
        "2",
    );
    assert_eq!(
        message,
        "no module `Util` of version 2; there are 1.0.0 and a module without a version"
    );
}
