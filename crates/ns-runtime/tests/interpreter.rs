//! Interpreter core behaviours through real modules: a method's implicit and
//! explicit return, a bare builtin class name, and non-local return from a
//! block to a live home and into a dead one.

use indoc::indoc;
use ns_runtime::{Interpreter, NewspeakError, Signal, Symbol, Value};

/// Loads `src`, instantiates its root class and returns both.
fn instance_of(src: &str) -> (Interpreter, Value) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module_unrooted(&resolved);
    let receiver = match interp.instantiate_unrooted(class, vec![]) {
        Ok(v) => v,
        Err(Signal::Error(e)) => panic!("instantiate: unexpected Signal::Error: {}", e.message),
        Err(Signal::Return(..)) => panic!("instantiate: unexpected Signal::Return"),
    };
    (interp, receiver)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_cannot_return(result: Result<Value, Signal>, what: &str) {
    match result {
        Err(Signal::Error(NewspeakError { message, .. })) => assert!(
            message.contains("cannotReturn"),
            "{what}: expected cannotReturn, got: {message}"
        ),
        Ok(_) => panic!("{what}: expected cannotReturn, got a value"),
        Err(Signal::Return(..)) => {
            panic!("{what}: a return into a dead home escaped `send` as a raw Signal::Return")
        }
    }
}

#[test]
fn method_with_no_explicit_return_returns_self() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public bar = ( 1. 2 )
        ) : ()
    "});
    let result = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("bar"), vec![]),
        "bar",
    );
    assert!(
        matches!((&result, &receiver), (Value::Obj(a), Value::Obj(b)) if a.same(*b)),
        "a method with no explicit return must yield the receiver, not the last expression's value"
    );
}

#[test]
fn method_with_explicit_return_returns_the_returned_value() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public answer = ( ^42 )
        ) : ()
    "});
    let result = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("answer"), vec![]),
        "answer",
    );
    assert!(matches!(result, Value::SmallInt(42)));
}

/// A bare `OrderedCollection` (no `platform` prefix, outside any superclass
/// clause) names the builtin class.
#[test]
fn bare_builtin_class_name_reference_produces_a_working_instance() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public bar = ( ^OrderedCollection new )
        ) : ()
    "});
    let result = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("bar"), vec![]),
        "bar",
    );
    let Value::Obj(obj) = result else {
        panic!("expected an OrderedCollection instance")
    };
    assert!(matches!(
        interp.heap_ref().array_elements(obj),
        Some(elements) if elements.is_empty()
    ));
}

/// `[ ^1 ] value`, invoked while its home method is still running, returns
/// from the home: `m` answers 1, and its second statement never runs.
#[test]
fn non_local_return_from_a_block_unwinds_to_the_home_methods_activation() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( [ ^1 ] value. ^2 )
        ) : ()
    "});
    let result = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("m"), vec![]),
        "m",
    );
    assert!(matches!(result, Value::SmallInt(1)));
    assert_eq!(interp.heap_ref().activation_depth(), 0);
}

/// The same block, stored and invoked again after its home returned, is
/// the spec's `cannotReturn` -- a `NewspeakError`, not a panic.
#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|b.|) (
          public m = ( b:: [ ^1 ]. ^b value )
          public again = ( ^b value )
        ) : ()
    "});
    let first = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("m"), vec![]),
        "m",
    );
    assert!(matches!(first, Value::SmallInt(1)));
    expect_cannot_return(
        interp.send_unrooted(receiver, Symbol::intern("again"), vec![]),
        "again",
    );
}

/// A method's activation is dead once it returns *normally*, not only after
/// an explicit `^`: `capture` stores `[^1]` and falls off the end.
#[test]
fn a_block_escaping_a_normally_returned_method_is_a_clean_cannot_return_error() {
    let (mut interp, receiver) = instance_of(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|b.|) (
          public capture = ( b:: [^1] )
          public runIt = ( ^b value )
        ) : ()
    "});
    expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("capture"), vec![]),
        "capture",
    );
    expect_cannot_return(
        interp.send_unrooted(receiver, Symbol::intern("runIt"), vec![]),
        "runIt",
    );
}
