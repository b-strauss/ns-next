//! Interpreter core behaviours through real modules: a method's implicit and
//! explicit return, a bare builtin class name's send to `Object`'s accessor,
//! and non-local return from a block to a live home and into a dead one.

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

/// Loads `source`, instantiates its root class and returns both.
fn instance_of(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    (interpreter, receiver)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn expect_cannot_return(result: HostResult, what: &str) {
    match result {
        Err(HostSignal::Error(HostError { message, .. })) => assert!(
            message.contains("cannotReturn"),
            "{what}: expected cannotReturn, got: {message}"
        ),
        Ok(_) => panic!("{what}: expected cannotReturn, got a value"),
        Err(HostSignal::Return) => {
            panic!("{what}: a return into a dead home escaped `send` as a raw HostSignal::Return")
        }
    }
}

#[test]
fn method_with_no_explicit_return_returns_self() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public bar = ( 1. 2 )
        ) : ()
    "});
    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("bar"), &[]),
        "bar",
    );
    interpreter.enter(|mutator| {
        let (result, receiver) = (mutator.fetch(&result), mutator.fetch(&receiver));
        assert!(
            matches!((&result, &receiver), (Value::Object(result_object), Value::Object(receiver_object)) if result_object.same(*receiver_object)),
            "a method with no explicit return must yield the receiver, not the last expression's value"
        );
    });
}

#[test]
fn method_with_explicit_return_returns_the_returned_value() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public answer = ( ^42 )
        ) : ()
    "});
    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("answer"), &[]),
        "answer",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&result), Value::SmallInteger(42))));
}

/// A bare `OrderedCollection` (no `platform` prefix, outside any superclass
/// clause) is the self send `self OrderedCollection`, which `Object`'s
/// accessor answers with the builtin class.
#[test]
fn bare_builtin_class_name_reference_produces_a_working_instance() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public bar = ( ^OrderedCollection new )
        ) : ()
    "});
    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("bar"), &[]),
        "bar",
    );
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&result) else {
            panic!("expected an OrderedCollection instance")
        };
        assert!(matches!(
            mutator.heap().array_elements(object),
            Some(elements) if elements.is_empty()
        ));
    });
}

/// `[ ^1 ] value`, invoked while its home method is still running, returns
/// from the home: `m` answers 1, and its second statement never runs.
#[test]
fn non_local_return_from_a_block_unwinds_to_the_home_methods_activation() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( [ ^1 ] value. ^2 )
        ) : ()
    "});
    let result = expect_ok(interpreter.send(&receiver, Symbol::intern("m"), &[]), "m");
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&result), Value::SmallInteger(1))));
    assert_eq!(
        interpreter.enter(|mutator| mutator.heap().activation_depth()),
        0
    );
}

/// The same block, stored and invoked again after its home returned, is
/// the spec's `cannotReturn` -- a `NewspeakError`, not a panic.
#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|b.|) (
          public m = ( b:: [ ^1 ]. ^b value )
          public again = ( ^b value )
        ) : ()
    "});
    let first = expect_ok(interpreter.send(&receiver, Symbol::intern("m"), &[]), "m");
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&first), Value::SmallInteger(1))));
    expect_cannot_return(
        interpreter.send(&receiver, Symbol::intern("again"), &[]),
        "again",
    );
}

/// A method's activation is dead once it returns *normally*, not only after
/// an explicit `^`: `capture` stores `[^1]` and falls off the end.
#[test]
fn a_block_escaping_a_normally_returned_method_is_a_clean_cannot_return_error() {
    let (mut interpreter, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|b.|) (
          public capture = ( b:: [^1] )
          public runIt = ( ^b value )
        ) : ()
    "});
    expect_ok(
        interpreter.send(&receiver, Symbol::intern("capture"), &[]),
        "capture",
    );
    expect_cannot_return(
        interpreter.send(&receiver, Symbol::intern("runIt"), &[]),
        "runIt",
    );
}
