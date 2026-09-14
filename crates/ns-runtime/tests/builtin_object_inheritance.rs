//! Every builtin class's superclass is `Object`,
//! but `dispatch::walk` stops at the first `builtin: Some(kind)` class it
//! reaches and never continues up that class's own superclass chain. These
//! tests pin the call-site fallback that restores the one missing
//! inheritance step (`Interpreter::dispatch_native_inheriting_object`):
//! a non-`Object` builtin receiver must still reach `Object`'s own native
//! `class`/`==`/`=`/`hash`/`printString`, and -- most importantly --
//! `Object`'s real, spec-shaped `doesNotUnderstand:`
//! (`builtins::object_class::dispatch`), not the interpreter's internal
//! "no doesNotUnderstand: found" fallback string.

use ns_runtime::{BuiltinClass, Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn as_string(heap: &Heap, value: &Value) -> String {
    match value {
        Value::Object(object) => heap
            .str_value(*object)
            .unwrap_or_else(|| panic!("expected ObjectData::Str, got {}", heap.describe(value))),
        other => panic!("expected Value::Obj, got {}", heap.describe(other)),
    }
}

/// `3 class` -- `Integer` implements no `class` of its own, so this can only
/// succeed by inheriting `Object`'s native `class`.
#[test]
fn a_builtin_receiver_inherits_objects_native_class() {
    let mut interpreter = Interpreter::bootstrap();

    let result = interpreter.send(
        &interpreter.stash(Value::SmallInteger(3)),
        Symbol::intern("class"),
        &[],
    );

    let result = expect_ok(result, "3 class");
    interpreter.enter(|mutator| {
        let expected = mutator.builtin_class(BuiltinClass::Integer);
        match mutator.fetch(&result) {
            Value::Object(object) => assert!(
                object.same(expected),
                "`3 class` must yield the Integer class object"
            ),
            other => panic!(
                "expected Value::Obj(<Integer class>), got {}",
                mutator.heap().describe(&other)
            ),
        }
    });
}

/// `nil printString` and `(OrderedCollection new) printString`: neither
/// `Nil` nor `OrderedCollection` implements `printString`, so both fall back
/// to `Object`'s own generic default rather than `doesNotUnderstand:`.
#[test]
fn builtins_without_their_own_print_string_inherit_objects_default() {
    let mut interpreter = Interpreter::bootstrap();

    let nil_result = interpreter.send(
        &interpreter.stash(Value::Nil),
        Symbol::intern("printString"),
        &[],
    );
    let nil_result = expect_ok(nil_result, "nil printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&nil_result)),
            "an Object"
        );
    });

    let ordered_collection = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::OrderedCollection);
        mutator.stash(Value::Object(class))
    });
    let collection = expect_ok(
        interpreter.send(&ordered_collection, Symbol::intern("new"), &[]),
        "OrderedCollection new",
    );
    let collection_result = interpreter.send(&collection, Symbol::intern("printString"), &[]);
    let collection_result = expect_ok(collection_result, "(OrderedCollection new) printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&collection_result)),
            "an Object"
        );
    });
}

/// `3 == 3` / `3 hash`: `Object`'s identity `==` and `hash`, reached from an
/// `Integer` receiver.
#[test]
fn a_builtin_receiver_inherits_objects_identity_and_hash() {
    let mut interpreter = Interpreter::bootstrap();

    let same = interpreter.send(
        &interpreter.stash(Value::SmallInteger(3)),
        Symbol::intern("=="),
        &[interpreter.stash(Value::SmallInteger(3))],
    );
    let same = expect_ok(same, "3 == 3");
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&same), Value::Boolean(true))));

    let hash = interpreter.send(
        &interpreter.stash(Value::SmallInteger(3)),
        Symbol::intern("hash"),
        &[],
    );
    let hash = expect_ok(hash, "3 hash");
    assert!(
        interpreter.enter(|mutator| matches!(mutator.fetch(&hash), Value::SmallInteger(_))),
        "Object's native `hash` must be reachable from an Integer receiver"
    );
}

/// The load-bearing half: an unimplemented selector on a builtin must
/// route through `builtins::object_class::dispatch`'s real
/// `doesNotUnderstand:` -- whose message is
/// "<describe(receiver)> does not understand #<selector>" -- rather than
/// `Interpreter::decide_does_not_understand`'s internal
/// "(native class ... has no doesNotUnderstand: implementation)" fallback.
#[test]
fn an_unknown_selector_on_a_builtin_runs_objects_real_does_not_understand() {
    let mut interpreter = Interpreter::bootstrap();

    let result = interpreter.send(
        &interpreter.stash(Value::SmallInteger(3)),
        Symbol::intern("frobnicate"),
        &[],
    );

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("frobnicate"));
            assert_eq!(
                error.message, "an object does not understand #frobnicate",
                "expected `object_class::dispatch`'s own doesNotUnderstand: message shape"
            );
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a doesNotUnderstand: error for `3 frobnicate`, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// The same, for an object-shaped builtin receiver (`String`), to prove the
/// fallback is not specific to a primitive `Value` variant.
#[test]
fn an_unknown_selector_on_a_string_runs_objects_real_does_not_understand() {
    let mut interpreter = Interpreter::bootstrap();
    let receiver = interpreter.enter(|mutator| {
        let object = mutator.new_string("hi".to_string());
        mutator.stash(Value::Object(object))
    });

    let result = interpreter.send(&receiver, Symbol::intern("frobnicate"), &[]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.message, "an object does not understand #frobnicate");
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a doesNotUnderstand: error, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// The fallback must never shadow a builtin's *own* implementation of a
/// selector `Object` also implements: `Integer` has its own `=` and
/// `printString`, and those must still win.
#[test]
fn a_builtins_own_implementation_still_wins_over_objects() {
    let mut interpreter = Interpreter::bootstrap();

    // Object's `=` is identity; Integer's own `=` is numeric equality. Both
    // agree on `3 = 3`, so use `printString` -- where the two genuinely
    // differ -- as the discriminating case.
    let printed = interpreter.send(
        &interpreter.stash(Value::SmallInteger(7)),
        Symbol::intern("printString"),
        &[],
    );
    let printed = expect_ok(printed, "7 printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&printed)),
            "7",
            "Integer's own printString must win over Object's generic default"
        );
    });
}
