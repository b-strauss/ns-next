//! The manifest a project's root receives answers each module by name, and
//! nothing else.

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

fn load(interpreter: &mut Interpreter, source: &str, origin: SourceOrigin) -> Root {
    let module = ns_syntax::parse(source).unwrap_or_else(|_| panic!("parse"));
    let resolved =
        ns_runtime::resolve(module, source.into(), origin).unwrap_or_else(|_| panic!("resolve"));
    interpreter.load_module(&resolved)
}

fn manifest_over(interpreter: &mut Interpreter, name: &str, module: &Root) -> Root {
    let manifest_class = load(
        interpreter,
        ns_runtime::MANIFEST_SOURCE,
        SourceOrigin::Embedded("Manifest.ns".into()),
    );
    let (names, definitions) = interpreter.enter(|mutator| {
        let symbol = mutator.new_symbol(Symbol::intern(name));
        let names = mutator.new_array(vec![symbol]);
        let definition = mutator.fetch(module);
        let definitions = mutator.new_array(vec![definition]);
        (
            mutator.stash(Value::Object(names)),
            mutator.stash(Value::Object(definitions)),
        )
    });
    expect_ok(
        interpreter.instantiate(&manifest_class, &[names, definitions]),
        "Manifest names:definitions:",
    )
}

const SIEVE: &str = indoc! {"
    Newspeak3
    'Test'
    class Sieve = Object new () () : ()
"};

#[test]
fn a_manifest_answers_a_module_definition_by_its_name() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(
        &mut interpreter,
        SIEVE,
        SourceOrigin::File("Sieve.ns".into()),
    );
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    let answer = expect_ok(
        interpreter.send(&manifest, Symbol::intern("Sieve"), &[]),
        "manifest Sieve",
    );
    interpreter.enter(
        |mutator| match (mutator.fetch(&answer), mutator.fetch(&module)) {
            (Value::Object(left), Value::Object(right)) => assert!(left.same(right)),
            _ => panic!("the manifest answered no class"),
        },
    );
}

#[test]
fn a_manifest_does_not_understand_a_name_it_does_not_hold() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(
        &mut interpreter,
        SIEVE,
        SourceOrigin::File("Sieve.ns".into()),
    );
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    match interpreter.send(&manifest, Symbol::intern("Missing"), &[]) {
        Ok(_) => panic!("the manifest answered a module it does not hold"),
        Err(ns_runtime::HostSignal::Error(error)) => {
            assert!(
                error.message.contains("does not understand #Missing"),
                "{}",
                error.message
            )
        }
        Err(ns_runtime::HostSignal::Return) => panic!("a non-local return escaped"),
    }
}

#[test]
fn an_object_member_answers_before_the_manifest_can() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(
        &mut interpreter,
        SIEVE,
        SourceOrigin::File("Sieve.ns".into()),
    );
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    assert!(!interpreter.responds_publicly(&manifest, Symbol::intern("Sieve")));
    assert!(interpreter.responds_publicly(&manifest, Symbol::intern("Array")));
    assert!(interpreter.responds_publicly(&manifest, Symbol::intern("printString")));
}
