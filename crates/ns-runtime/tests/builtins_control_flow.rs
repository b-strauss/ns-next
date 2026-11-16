//! Three of the methods behind the compiler's inlined forms
//! (`Closure>>whileTrue:`, `Closure>>whileFalse:`, `True>>ifFalse:ifTrue:`).
//! Every block reaches the send through a variable: the compiler inlines
//! only literal blocks, so a variable is what makes these methods run
//! rather than the inlined form, which the compiler's own tests and the
//! goldens under `goldens/language/control-flow/` cover.

use indoc::{formatdoc, indoc};
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    Heap, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value, ValueKind,
};

/// Returns the `Interpreter` alongside the send's result -- every caller
/// reads the result through the interpreter that holds its root, and
/// `string_of` needs a live `Heap` to read a returned string.
fn run_method(source: &str, selector: &str) -> (Interpreter, HostResult) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let instance = match interpreter.instantiate(&class, &[]) {
        Ok(root) => root,
        Err(_) => panic!("instantiate failed"),
    };
    let result = interpreter.send(&instance, Symbol::intern(selector), &[]);
    (interpreter, result)
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

fn integer_of<'gc>(value: Value<'gc>) -> i64 {
    match value.kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    }
}

fn string_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> String {
    let Some(object) = value.as_object() else {
        panic!("expected a string object")
    };
    heap.str_value(object)
        .unwrap_or_else(|| panic!("expected a String"))
}

fn module(body: &str) -> String {
    formatdoc! {"
        class X = Object new (
        ) (
          public run = ( {body} )
        ) : ()
    "}
}

#[test]
fn while_true_loops_until_the_condition_is_false_and_answers_nil() {
    let (mut interpreter, result) = run_method(
        &module("| n c b | n:: 0. c:: [n < 3]. b:: [n:: n + 1]. c whileTrue: b. ^n"),
        "run",
    );
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&value))),
        3
    );
    let (mut interpreter, result) =
        run_method(&module("| c | c:: [false]. ^c whileTrue: [nil]"), "run");
    let value = expect_ok(result, "run");
    assert!(interpreter.enter(|mutator| mutator.fetch(&value).is_nil()));
}

#[test]
fn while_false_loops_until_the_condition_is_true() {
    let (mut interpreter, result) = run_method(
        &module("| n c | n:: 0. c:: [n = 3]. c whileFalse: [n:: n + 1]. ^n"),
        "run",
    );
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&value))),
        3
    );
}

#[test]
fn a_non_boolean_condition_signals_non_boolean_receiver() {
    let (_interpreter, result) = run_method(&module("| c | c:: [3]. ^c whileTrue: [nil]"), "run");
    match result {
        Err(HostSignal::Error(error)) => {
            assert!(
                error.message.contains("which is no Boolean"),
                "{}",
                error.message
            )
        }
        _ => panic!("expected the NonBooleanReceiver error"),
    }
}

#[test]
fn a_non_local_return_escapes_a_while_loop() {
    let (mut interpreter, result) = run_method(
        &module("| n c | n:: 0. c:: [true]. c whileTrue: [n:: n + 1. n = 5 ifTrue: [^n]]"),
        "run",
    );
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&value))),
        5
    );
}

#[test]
fn a_while_loop_sends_value_to_a_body_that_is_no_block() {
    let source = indoc! {"
        class X = Object new (
        ) (
            class Counter = Object new (
                | public count ::= 0. |
            ) (
                public value = ( count:: count + 1 )
            )
            public run = (
                | counter condition |
                counter:: Counter new.
                condition:: [counter count < 4].
                condition whileTrue: counter.
                ^counter count
            )
        ) : ()
    "};
    let (mut interpreter, result) = run_method(source, "run");
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&value))),
        4
    );
}

#[test]
fn a_while_loop_body_that_understands_nothing_fails() {
    // A condition that ends on its own, so a loop that never sends `value`
    // answers rather than spinning.
    let (_interpreter, result) = run_method(
        &module(
            "| count condition | count:: 0. condition:: [count:: count + 1. count < 3]. ^condition whileTrue: nil",
        ),
        "run",
    );
    match result {
        Err(HostSignal::Error(error)) => assert!(
            error
                .message
                .contains("an object does not understand #value"),
            "{}",
            error.message
        ),
        _ => panic!("expected the value doesNotUnderstand: error"),
    }
}

#[test]
fn if_false_if_true_picks_the_right_block() {
    let (mut interpreter, result) = run_method(
        &module("| f t | f:: ['f']. t:: ['t']. ^true ifFalse: f ifTrue: t"),
        "run",
    );
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| string_of(mutator.heap(), mutator.fetch(&value))),
        "t"
    );
    let (mut interpreter, result) = run_method(
        &module("| f t | f:: ['f']. t:: ['t']. ^false ifFalse: f ifTrue: t"),
        "run",
    );
    let value = expect_ok(result, "run");
    assert_eq!(
        interpreter.enter(|mutator| string_of(mutator.heap(), mutator.fetch(&value))),
        "f"
    );
}
