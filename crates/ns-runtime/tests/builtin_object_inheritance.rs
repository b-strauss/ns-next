//! Every builtin class's superclass is `Object`, and the ordinary lookup
//! walk reaches `Object`'s primitives from any builtin receiver. These tests
//! pin that a non-`Object` builtin receiver reaches `Object`'s own
//! `class`/`==`/`=`/`printString`, and -- most importantly --
//! `Object`'s real, spec-shaped `doesNotUnderstand:`
//! (`builtins::object_class`), not the interpreter's internal
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

fn as_string<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> String {
    match value {
        Value::Object(object) => heap
            .str_value(*object)
            .unwrap_or_else(|| panic!("expected ObjectData::String, got {}", heap.describe(value))),
        other => panic!("expected Value::Object, got {}", heap.describe(other)),
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
                "expected Value::Object(<Integer class>), got {}",
                mutator.heap().describe(&other)
            ),
        }
    });
}

/// `nil printString` answers `nil`, as Primordial Soup's `UndefinedObject`
/// does.
#[test]
fn nil_prints_as_nil() {
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
            "nil"
        );
    });
}

/// `(Array new) printString`: `Array` implements no `printString`, so it
/// falls back to `Object`'s own generic default rather than
/// `doesNotUnderstand:`.
#[test]
fn builtins_without_their_own_print_string_inherit_objects_default() {
    let mut interpreter = Interpreter::bootstrap();

    let array_class = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::Object(class))
    });
    let array = expect_ok(
        interpreter.send(&array_class, Symbol::intern("new"), &[]),
        "Array new",
    );
    let array_result = interpreter.send(&array, Symbol::intern("printString"), &[]);
    let array_result = expect_ok(array_result, "(Array new) printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&array_result)),
            "an Object"
        );
    });
}

/// `3 == 3`: `Value`'s `==`, reached from an `Integer` receiver -- a send
/// of `=`, since §3.1.1 leaves a value no identity to answer from.
/// `3 hash` does *not* reach `Object`'s: `Integer` declares its own, which
/// answers the integer itself, since a value hashes by its value rather
/// than by an identity it does not have.
#[test]
fn a_builtin_receiver_compares_and_hashes_by_its_value() {
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
        interpreter.enter(|mutator| matches!(mutator.fetch(&hash), Value::SmallInteger(3))),
        "an Integer hashes by its value, not by Object's identity hash"
    );
}

/// The load-bearing half: an unimplemented selector on a builtin must
/// route through `builtins::object_class`'s real `doesNotUnderstand:` --
/// whose message is "<describe(receiver)> does not understand #<selector>"
/// -- rather than the "(and no doesNotUnderstand: found anywhere in the
/// class chain)" fallback `interpreter/resolve.rs` raises when its handler
/// lookup finds nothing (`Resolved::Unhandled`).
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
                "expected `object_class`'s own doesNotUnderstand: message shape"
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
