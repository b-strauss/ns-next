//! Tests for the `Interpreter::send`-level `ClassName new` -> `instantiate`
//! wiring (see `interpreter/instantiate.rs`'s `try_factory_send`). These
//! tests go through `Interpreter::send` rather than calling
//! `Interpreter::instantiate` directly from Rust -- the code path an
//! ordinary Newspeak `ClassName new` message send actually takes.

use indoc::indoc;
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

/// `OrderedCollection new` via ordinary `send` (not a direct
/// `interpreter.instantiate(...)` call) must produce an
/// `ObjectData::Array`-backed object -- the payload shape
/// `builtins::collections`' dispatch functions actually pattern-match on -- and
/// that object must go on to support `add:`/`size` through the real dispatch
/// chain, which also proves the result stays classed as `OrderedCollection`
/// (not `Array`: `Array`'s own native dispatch has no `add:`, so `add:`
/// succeeding rules out the mis-tagging bug this test was written to catch).
#[test]
fn ordered_collection_new_via_ordinary_send_is_array_backed_and_supports_add_and_size() {
    let mut interpreter = Interpreter::bootstrap();
    let class_value = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::OrderedCollection);
        mutator.stash(Value::Object(class))
    });

    let result = interpreter.send(&class_value, Symbol::intern("new"), &[]);
    let receiver = expect_ok(result, "OrderedCollection new");

    interpreter.enter(|mutator| {
        let fetched = mutator.fetch(&receiver);
        let Value::Object(object) = &fetched else {
            panic!(
                "expected Value::Object, got {}",
                mutator.heap().describe(&fetched)
            );
        };
        let elements = mutator.heap().array_elements(*object);
        assert!(
            matches!(&elements, Some(items) if items.is_empty()),
            "expected an empty ObjectData::Array payload"
        );
    });

    let add_result = expect_ok(
        interpreter.send(
            &receiver,
            Symbol::intern("add:"),
            &[interpreter.stash(Value::SmallInteger(1))],
        ),
        "OrderedCollection new add: 1",
    );
    interpreter.enter(|mutator| match mutator.fetch(&add_result) {
        Value::SmallInteger(1) => {}
        other => panic!(
            "expected add: to return its argument, got {}",
            mutator.heap().describe(&other)
        ),
    });

    let size_result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("size"), &[]),
        "OrderedCollection new (after add:) size",
    );
    interpreter.enter(|mutator| match mutator.fetch(&size_result) {
        Value::SmallInteger(1) => {}
        other => panic!(
            "expected Value::SmallInteger(1), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// `Array new` via ordinary `send` must also be `Array`-tagged (not
/// `OrderedCollection`), confirmed here by `add:` *failing* --
/// `dispatch_array` has no `add:` arm.
#[test]
fn array_new_via_ordinary_send_does_not_understand_add() {
    let mut interpreter = Interpreter::bootstrap();
    let class_value = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::Object(class))
    });

    let receiver = expect_ok(
        interpreter.send(&class_value, Symbol::intern("new"), &[]),
        "Array new",
    );

    match interpreter.send(
        &receiver,
        Symbol::intern("add:"),
        &[interpreter.stash(Value::SmallInteger(1))],
    ) {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("add:"));
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected doesNotUnderstand: add:, got Ok({got})")
        }
        Err(HostSignal::Return) => {
            panic!("expected doesNotUnderstand: add:, got HostSignal::Return")
        }
    }
}

/// A user-defined class's `new` via ordinary `send` must behave exactly
/// like calling `interpreter.instantiate(...)` directly: same slot values
/// reachable afterward through a real method send.
#[test]
fn user_defined_class_new_via_ordinary_send_matches_direct_instantiate() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|x = 42.|) (public bar = (^x)) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");

    // Path A: `Foo new` via ordinary `send`.
    let mut interpreter_a = Interpreter::bootstrap();
    let class_a = interpreter_a.load_module(&resolved);
    let via_send = expect_ok(
        interpreter_a.send(&class_a, Symbol::intern("new"), &[]),
        "Foo new (via send)",
    );
    let via_send_bar = expect_ok(
        interpreter_a.send(&via_send, Symbol::intern("bar"), &[]),
        "Foo new bar (via send)",
    );

    // Path B: `interpreter.instantiate(...)` called directly.
    let mut interpreter_b = Interpreter::bootstrap();
    let class_b = interpreter_b.load_module(&resolved);
    let via_instantiate = expect_ok(interpreter_b.instantiate(&class_b, &[]), "instantiate(Foo)");
    let via_instantiate_bar = expect_ok(
        interpreter_b.send(&via_instantiate, Symbol::intern("bar"), &[]),
        "instantiate(Foo) bar",
    );

    // Each side leaves its interpreter's entry as the integer it answered,
    // or as the description of whatever else it answered.
    let from_send = interpreter_a.enter(|mutator| match mutator.fetch(&via_send_bar) {
        Value::SmallInteger(integer) => Ok(integer),
        other => Err(mutator.heap().describe(&other)),
    });
    let from_instantiate =
        interpreter_b.enter(|mutator| match mutator.fetch(&via_instantiate_bar) {
            Value::SmallInteger(integer) => Ok(integer),
            other => Err(mutator.heap().describe(&other)),
        });
    match (from_send, from_instantiate) {
        (Ok(sent), Ok(instantiated)) => assert_eq!(sent, instantiated),
        (sent, instantiated) => {
            let sent_description =
                sent.map_or_else(|description| description, |integer| integer.to_string());
            let instantiated_description =
                instantiated.map_or_else(|description| description, |integer| integer.to_string());
            panic!("expected two equal Value::SmallInteger, got {sent_description} and {instantiated_description}")
        }
    }
}

/// A class object receiving a selector that is *not* its factory selector
/// still goes through ordinary dispatch (falls through to
/// `doesNotUnderstand:` here, since nothing implements the selector) --
/// proves `try_factory_send` only intercepts the
/// exact factory selector, not every send to a class object.
#[test]
fn a_class_object_receiving_a_non_factory_selector_falls_through_to_ordinary_dispatch() {
    let mut interpreter = Interpreter::bootstrap();
    let class_value = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::OrderedCollection);
        mutator.stash(Value::Object(class))
    });

    match interpreter.send(
        &class_value,
        Symbol::intern("thisSelectorDoesNotExist"),
        &[],
    ) {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected doesNotUnderstand:, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("expected doesNotUnderstand:, got HostSignal::Return"),
    }
}
