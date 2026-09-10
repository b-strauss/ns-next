//! The three control-flow natives Milestone 2 adds (`Block>>whileTrue:`,
//! `Block>>whileFalse:`, `Boolean>>ifFalse:ifTrue:`), exercised under the
//! tree-walker -- the engine that has no inlining, so these really are the
//! natives running.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{Heap, Interpreter, Signal, SourceOrigin, Symbol, Value};

/// Returns the `Interpreter` alongside the send's result -- `string_of`
/// needs a live `Heap` to read a returned string, so callers that inspect
/// the value can't discard the interpreter the way ones that only check
/// `int_of`/`matches!` do.
fn run_method(src: &str, selector: &str) -> (Interpreter, Result<Value, Signal>) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let instance = match interp.instantiate(class, vec![], &bindings) {
        Ok(v) => v,
        Err(_) => panic!("instantiate failed"),
    };
    let result = interp.send(instance, Symbol::intern(selector), vec![], &bindings);
    (interp, result)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
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
    heap.str_value(obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Str"))
}

fn module(body: &str) -> String {
    format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  public run = ( {body} )\n) : ()")
}

#[test]
fn while_true_loops_until_the_condition_is_false_and_answers_nil() {
    let (_interp, result) = run_method(
        &module("| n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(int_of(&v), 3);
    let (_interp, result) = run_method(&module("^[false] whileTrue: [nil]"), "run");
    let v = expect_ok(result, "run");
    assert!(matches!(v, Value::Nil));
}

#[test]
fn while_false_loops_until_the_condition_is_true() {
    let (_interp, result) = run_method(
        &module("| n | n := 0. [n = 3] whileFalse: [n := n + 1]. ^n"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(int_of(&v), 3);
}

#[test]
fn a_non_boolean_condition_sends_must_be_boolean() {
    let (_interp, result) = run_method(&module("^[3] whileTrue: [nil]"), "run");
    match result {
        Err(Signal::Error(e)) => {
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
    let (_interp, result) = run_method(
        &module("| n | n := 0. [true] whileTrue: [n := n + 1. n = 5 ifTrue: [^n]]"),
        "run",
    );
    let v = expect_ok(result, "run");
    assert_eq!(int_of(&v), 5);
}

#[test]
fn if_false_if_true_picks_the_right_block() {
    let (interp, result) = run_method(&module("^true ifFalse: ['f'] ifTrue: ['t']"), "run");
    let v = expect_ok(result, "run");
    assert_eq!(string_of(interp.heap_ref(), &v), "t");
    let (interp, result) = run_method(&module("^false ifFalse: ['f'] ifTrue: ['t']"), "run");
    let v = expect_ok(result, "run");
    assert_eq!(string_of(interp.heap_ref(), &v), "f");
}
