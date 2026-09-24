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
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

#[test]
fn platform_is_memoized_across_calls() {
    let mut interpreter = Interpreter::bootstrap();
    let first = interpreter.platform();
    let second = interpreter.platform();
    interpreter.enter(
        |mutator| match (mutator.fetch(&first), mutator.fetch(&second)) {
            (Value::Object(first_object), Value::Object(second_object)) => {
                assert!(first_object.same(second_object))
            }
            _ => panic!("expected Value::Object"),
        },
    );
}

#[test]
fn platform_transcript_show_and_new_line_round_trip() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );

    let string = interpreter.enter(|mutator| {
        let object = mutator.new_string("hi".to_string());
        mutator.stash(Value::Object(object))
    });
    let after_show = expect_ok(
        interpreter.send(&transcript, Symbol::intern("show:"), &[string]),
        "transcript show: 'hi'",
    );
    // `show:` returns the receiver (the `Transcript`-style convention
    // `platform.rs`'s doc comment describes).
    interpreter.enter(
        |mutator| match (mutator.fetch(&after_show), mutator.fetch(&transcript)) {
            (Value::Object(after_show_object), Value::Object(transcript_object)) => {
                assert!(after_show_object.same(transcript_object))
            }
            _ => panic!("expected Value::Object"),
        },
    );

    expect_ok(
        interpreter.send(&transcript, Symbol::intern("newLine"), &[]),
        "transcript newLine",
    );
}

/// `platform Array` and the rest answer the builtin class through
/// `Object`'s accessors (§3.2), which the platform inherits like every
/// object: it declares none of its own.
#[test]
fn platform_class_accessors_return_the_matching_builtin_class() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    for (name, kind) in [
        ("Object", BuiltinClass::Object),
        ("Array", BuiltinClass::Array),
        ("OrderedCollection", BuiltinClass::OrderedCollection),
        ("String", BuiltinClass::String),
        ("Symbol", BuiltinClass::Symbol),
    ] {
        let got = expect_ok(
            interpreter.send(&platform, Symbol::intern(name), &[]),
            &format!("platform {name}"),
        );
        interpreter.enter(|mutator| {
            let expected = mutator.builtin_class(kind);
            match mutator.fetch(&got) {
                Value::Object(object) => assert!(object.same(expected), "platform {name}"),
                other => panic!(
                    "expected Value::Object, got {}",
                    mutator.heap().describe(&other)
                ),
            }
        });
    }
}

/// `platform collections List` exists because three whitelisted corpus
/// modules open with `platform collections List` in their module state
/// (`corpus/runnable.toml`). It names the Core Library's growable ordered
/// sequence, which this runtime already has as `OrderedCollection`.
#[test]
fn platform_collections_answers_list_with_the_ordered_collection_class() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let got = expect_ok(
        interpreter.send(&collections, Symbol::intern("List"), &[]),
        "platform collections List",
    );
    interpreter.enter(|mutator| {
        let expected = mutator.builtin_class(BuiltinClass::OrderedCollection);
        match mutator.fetch(&got) {
            Value::Object(object) => assert!(object.same(expected)),
            _ => panic!("expected a class object"),
        }
    });
}

/// The namespace answers exactly what a whitelisted module asks for, and
/// nothing more: `Map` (and the rest of the corpus's collections protocol)
/// names a class this runtime does not have, and must fail loudly rather
/// than quietly alias onto some other collection.
#[test]
fn platform_collections_does_not_invent_a_map() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interpreter.send(&collections, Symbol::intern("Map"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand"),
            "message was: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections Map` to be refused"),
    }
}

/// The transcript and the collections namespace are instances of classes of
/// their own, so neither answers the other's protocol.
#[test]
fn the_collections_namespace_is_not_a_transcript() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interpreter.send(&collections, Symbol::intern("newLine"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand"),
            "message was: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections newLine` to be refused"),
    }
}
