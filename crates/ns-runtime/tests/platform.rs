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
        other => panic!(
            "expected Value::Obj, got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// `platform collections List` exists because three whitelisted corpus
/// modules open with `platform collections List` in their module state
/// (`corpus/runnable.toml`). It names the Core Library's growable ordered
/// sequence, which this runtime already has as `OrderedCollection`.
#[test]
fn platform_collections_answers_list_with_the_ordered_collection_class() {
    let mut interp = Interpreter::bootstrap();
    let expected = interp.builtin_class(BuiltinClass::OrderedCollection);
    let platform = interp.platform();
    let bindings = empty_bindings();

    let collections = expect_ok(
        interp.send(
            platform,
            Symbol::intern("collections"),
            vec![],
            &bindings,
            None,
        ),
        "platform collections",
    );
    let got = expect_ok(
        interp.send(collections, Symbol::intern("List"), vec![], &bindings, None),
        "platform collections List",
    );
    match got {
        Value::Obj(obj) => assert!(Rc::ptr_eq(&obj, &expected)),
        _ => panic!("expected a class object"),
    }
}

/// The namespace answers exactly what a whitelisted module asks for, and
/// nothing more: `Map` (and the rest of the corpus's collections protocol)
/// names a class this runtime does not have, and must fail loudly rather
/// than quietly alias onto some other collection.
#[test]
fn platform_collections_does_not_invent_a_map() {
    let mut interp = Interpreter::bootstrap();
    let platform = interp.platform();
    let bindings = empty_bindings();

    let collections = expect_ok(
        interp.send(
            platform,
            Symbol::intern("collections"),
            vec![],
            &bindings,
            None,
        ),
        "platform collections",
    );
    match interp.send(collections, Symbol::intern("Map"), vec![], &bindings, None) {
        Err(Signal::Error(e)) => assert!(
            e.message.contains("does not understand"),
            "message was: {}",
            e.message
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return"),
        Ok(_) => panic!("expected `collections Map` to be refused"),
    }
}

/// The transcript stream and the collections namespace share
/// `BuiltinClass::Platform` and are told apart only by their payload
/// shape, so this pins that they do not answer each other's protocol.
#[test]
fn the_collections_namespace_is_not_a_transcript() {
    let mut interp = Interpreter::bootstrap();
    let platform = interp.platform();
    let bindings = empty_bindings();

    let collections = expect_ok(
        interp.send(
            platform,
            Symbol::intern("collections"),
            vec![],
            &bindings,
            None,
        ),
        "platform collections",
    );
    match interp.send(
        collections,
        Symbol::intern("newLine"),
        vec![],
        &bindings,
        None,
    ) {
        Err(Signal::Error(e)) => assert!(
            e.message.contains("does not understand"),
            "message was: {}",
            e.message
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return"),
        Ok(_) => panic!("expected `collections newLine` to be refused"),
    }
}
