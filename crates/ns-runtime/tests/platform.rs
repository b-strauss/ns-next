//! Unit-level tests for `Interpreter::platform` and `crate::platform`'s
//! dispatch. `crates/ns-cli/tests/run.rs` covers the full,
//! real-stdout end-to-end path through `ns-next run`; these tests exercise
//! the same dispatch chain directly via `Interpreter::send`, without
//! spawning a process, and check the singleton/identity behavior a
//! process-level test can't easily observe.

use ns_runtime::{BuiltinClass, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

#[test]
fn platform_is_memoized_across_calls() {
    let mut interp = Interpreter::bootstrap();
    let a = interp.platform();
    let b = interp.platform();
    match (interp.fetch(&a), interp.fetch(&b)) {
        (Value::Obj(a), Value::Obj(b)) => assert!(a.same(b)),
        _ => panic!("expected Value::Obj"),
    }
}

#[test]
fn platform_transcript_show_and_new_line_round_trip() {
    let mut interp = Interpreter::bootstrap();
    let platform = interp.platform();

    let transcript = expect_ok(
        interp.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );

    let s = interp.new_string("hi".to_string());
    let s = interp.stash(Value::Obj(s));
    let after_show = expect_ok(
        interp.send(&transcript, Symbol::intern("show:"), &[s]),
        "transcript show: 'hi'",
    );
    // `show:` returns the receiver (the `Transcript`-style convention
    // `platform.rs`'s doc comment describes).
    match (interp.fetch(&after_show), interp.fetch(&transcript)) {
        (Value::Obj(a), Value::Obj(b)) => assert!(a.same(b)),
        _ => panic!("expected Value::Obj"),
    }

    expect_ok(
        interp.send(&transcript, Symbol::intern("newLine"), &[]),
        "transcript newLine",
    );
}

#[test]
fn platform_class_accessors_return_the_matching_builtin_class() {
    let mut interp = Interpreter::bootstrap();
    let platform = interp.platform();

    let got = expect_ok(
        interp.send(&platform, Symbol::intern("OrderedCollection"), &[]),
        "platform OrderedCollection",
    );
    let expected = interp.builtin_class(BuiltinClass::OrderedCollection);
    match interp.fetch(&got) {
        Value::Obj(obj) => assert!(obj.same(expected)),
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
    let platform = interp.platform();

    let collections = expect_ok(
        interp.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let got = expect_ok(
        interp.send(&collections, Symbol::intern("List"), &[]),
        "platform collections List",
    );
    let expected = interp.builtin_class(BuiltinClass::OrderedCollection);
    match interp.fetch(&got) {
        Value::Obj(obj) => assert!(obj.same(expected)),
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

    let collections = expect_ok(
        interp.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interp.send(&collections, Symbol::intern("Map"), &[]) {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("does not understand"),
            "message was: {}",
            e.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
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

    let collections = expect_ok(
        interp.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interp.send(&collections, Symbol::intern("newLine"), &[]) {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("does not understand"),
            "message was: {}",
            e.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections newLine` to be refused"),
    }
}
