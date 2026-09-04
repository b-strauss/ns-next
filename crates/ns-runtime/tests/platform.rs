//! Unit-level tests for `Interpreter::platform` and `crate::platform`'s
//! dispatch (Task 11). `crates/ns-cli/tests/run.rs` covers the full,
//! real-stdout end-to-end path through `ns-next run`; these tests exercise
//! the same dispatch chain directly via `Interpreter::send`, without
//! spawning a process, and check the singleton/identity behavior a
//! process-level test can't easily observe.

use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::{BuiltinClass, Interpreter, Signal, Symbol, Value};

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn empty_bindings() -> Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> {
    Rc::new(HashMap::new())
}

#[test]
fn platform_is_memoized_across_calls() {
    let mut interp = Interpreter::bootstrap();
    let a = interp.platform();
    let b = interp.platform();
    match (a, b) {
        (Value::Obj(a), Value::Obj(b)) => assert!(Rc::ptr_eq(&a, &b)),
        _ => panic!("expected Value::Obj"),
    }
}

#[test]
fn platform_transcript_show_and_new_line_round_trip() {
    let mut interp = Interpreter::bootstrap();
    let platform = interp.platform();
    let bindings = empty_bindings();

    let transcript = expect_ok(
        interp.send(
            platform,
            Symbol::intern("transcript"),
            vec![],
            &bindings,
            None,
        ),
        "platform transcript",
    );

    let s = Value::Obj(interp.new_string("hi".to_string()));
    let after_show = expect_ok(
        interp.send(
            transcript.clone(),
            Symbol::intern("show:"),
            vec![s],
            &bindings,
            None,
        ),
        "transcript show: 'hi'",
    );
    // `show:` returns the receiver (the runtime spec's `Transcript`-style
    // convention, matching `platform.rs`'s doc comment).
    match (&after_show, &transcript) {
        (Value::Obj(a), Value::Obj(b)) => assert!(Rc::ptr_eq(a, b)),
        _ => panic!("expected Value::Obj"),
    }

    expect_ok(
        interp.send(
            transcript,
            Symbol::intern("newLine"),
            vec![],
            &bindings,
            None,
        ),
        "transcript newLine",
    );
}

#[test]
fn platform_class_accessors_return_the_matching_builtin_class() {
    let mut interp = Interpreter::bootstrap();
    let expected = interp.builtin_class(BuiltinClass::OrderedCollection);
    let platform = interp.platform();
    let bindings = empty_bindings();

    let got = expect_ok(
        interp.send(
            platform,
            Symbol::intern("OrderedCollection"),
            vec![],
            &bindings,
            None,
        ),
        "platform OrderedCollection",
    );
    match got {
        Value::Obj(obj) => assert!(Rc::ptr_eq(&obj, &expected)),
        other => panic!("expected Value::Obj, got {other:?}"),
    }
}
