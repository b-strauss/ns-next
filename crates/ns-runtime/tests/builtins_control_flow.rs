//! The three control-flow natives behind the compiler's inlined forms
//! (`Block>>whileTrue:`, `Block>>whileFalse:`, `Boolean>>ifFalse:ifTrue:`).
//! Every block reaches
//! the send through a variable: the compiler inlines only literal blocks, so
//! a variable is what makes these the natives running rather than the
//! inlined form, which the compiler's own tests and `program_outcomes.rs`
//! cover.

use indoc::formatdoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value};

/// Returns the `Interpreter` alongside the send's result -- every caller
/// reads the result through the interpreter that holds its root, and
/// `string_of` needs a live `Heap` to read a returned string.
fn run_method(src: &str, selector: &str) -> (Interpreter, HostResult) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let instance = match interp.instantiate(&class, &[]) {
        Ok(root) => root,
        Err(_) => panic!("instantiate failed"),
    };
    let result = interp.send(&instance, Symbol::intern(selector), &[]);
    (interp, result)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

fn string_of(heap: &Heap, v: &Value) -> String {
    let Value::Obj(obj) = v else {
        panic!("expected a string object")
    };
    heap.str_value(*obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Str"))
}

fn module(body: &str) -> String {
    formatdoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public run = ( {body} )
        ) : ()
    "}
}

#[test]
fn while_true_loops_until_the_condition_is_false_and_answers_nil() {
    let (mut interp, result) = run_method(
        &module("| n c b | n := 0. c := [n < 3]. b := [n := n + 1]. c whileTrue: b. ^n"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(interp.enter(|m| int_of(&m.fetch(&v))), 3);
    let (mut interp, result) =
        run_method(&module("| c | c := [false]. ^c whileTrue: [nil]"), "run");
    let v = expect_ok(result, "run");
    assert!(interp.enter(|m| matches!(m.fetch(&v), Value::Nil)));
}

#[test]
fn while_false_loops_until_the_condition_is_true() {
    let (mut interp, result) = run_method(
        &module("| n c | n := 0. c := [n = 3]. c whileFalse: [n := n + 1]. ^n"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(interp.enter(|m| int_of(&m.fetch(&v))), 3);
}

#[test]
fn a_non_boolean_condition_sends_must_be_boolean() {
    let (_interp, result) = run_method(&module("| c | c := [3]. ^c whileTrue: [nil]"), "run");
    match result {
        Err(HostSignal::Error(e)) => {
            assert!(
                e.message.contains("does not understand #mustBeBoolean"),
                "{}",
                e.message
            )
        }
        _ => panic!("expected the mustBeBoolean doesNotUnderstand: error"),
    }
}

#[test]
fn a_non_local_return_escapes_a_while_loop() {
    let (mut interp, result) = run_method(
        &module("| n c | n := 0. c := [true]. c whileTrue: [n := n + 1. n = 5 ifTrue: [^n]]"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(interp.enter(|m| int_of(&m.fetch(&v))), 5);
}

#[test]
fn if_false_if_true_picks_the_right_block() {
    let (mut interp, result) = run_method(
        &module("| f t | f := ['f']. t := ['t']. ^true ifFalse: f ifTrue: t"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(interp.enter(|m| string_of(m.heap(), &m.fetch(&v))), "t");
    let (mut interp, result) = run_method(
        &module("| f t | f := ['f']. t := ['t']. ^false ifFalse: f ifTrue: t"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(interp.enter(|m| string_of(m.heap(), &m.fetch(&v))), "f");
}
