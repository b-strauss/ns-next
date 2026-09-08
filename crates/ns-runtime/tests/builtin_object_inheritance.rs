//! Final-review finding C1: every builtin class's superclass is `Object`,
//! but `dispatch::walk` stops at the first `builtin: Some(kind)` class it
//! reaches and never continues up that class's own superclass chain. These
//! tests pin the call-site fallback that restores the one missing
//! inheritance step (`Interpreter::dispatch_native_inheriting_object`):
//! a non-`Object` builtin receiver must still reach `Object`'s own native
//! `class`/`==`/`=`/`hash`/`printString`, and -- most importantly --
//! `Object`'s real, spec-shaped `doesNotUnderstand:`
//! (`builtins::object_class::dispatch`), not the interpreter's internal
//! "no doesNotUnderstand: found" fallback string.

use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::{BuiltinClass, Heap, Interpreter, Signal, Symbol, Value};

fn empty_bindings() -> Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> {
    Rc::new(HashMap::new())
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn as_string(heap: &Heap, value: &Value) -> String {
    match value {
        Value::Obj(obj) => heap
            .str_value(obj)
            .unwrap_or_else(|| panic!("expected ObjectData::Str, got {}", heap.describe(value))),
        other => panic!("expected Value::Obj, got {}", heap.describe(other)),
    }
}

/// `3 class` -- `Integer` implements no `class` of its own, so this can only
/// succeed by inheriting `Object`'s native `class`.
#[test]
fn a_builtin_receiver_inherits_objects_native_class() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();
    let expected = interp.builtin_class(BuiltinClass::Integer);

    let result = interp.send(
        Value::SmallInt(3),
        Symbol::intern("class"),
        vec![],
        &bindings,
        None,
    );

    match expect_ok(result, "3 class") {
        Value::Obj(obj) => assert!(
            Rc::ptr_eq(&obj, &expected),
            "`3 class` must yield the Integer class object"
        ),
        other => panic!(
            "expected Value::Obj(<Integer class>), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// `nil printString` and `(OrderedCollection new) printString`: neither
/// `Nil` nor `OrderedCollection` implements `printString`, so both fall back
/// to `Object`'s own generic default rather than `doesNotUnderstand:`.
#[test]
fn builtins_without_their_own_print_string_inherit_objects_default() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();

    let nil_result = interp.send(
        Value::Nil,
        Symbol::intern("printString"),
        vec![],
        &bindings,
        None,
    );
    assert_eq!(
        as_string(interp.heap_ref(), &expect_ok(nil_result, "nil printString")),
        "an Object"
    );

    let coll = expect_ok(
        interp.send(
            Value::Obj(interp.builtin_class(BuiltinClass::OrderedCollection)),
            Symbol::intern("new"),
            vec![],
            &bindings,
            None,
        ),
        "OrderedCollection new",
    );
    let coll_result = interp.send(coll, Symbol::intern("printString"), vec![], &bindings, None);
    assert_eq!(
        as_string(
            interp.heap_ref(),
            &expect_ok(coll_result, "(OrderedCollection new) printString")
        ),
        "an Object"
    );
}

/// `3 == 3` / `3 hash`: `Object`'s identity `==` and `hash`, reached from an
/// `Integer` receiver.
#[test]
fn a_builtin_receiver_inherits_objects_identity_and_hash() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();

    let same = interp.send(
        Value::SmallInt(3),
        Symbol::intern("=="),
        vec![Value::SmallInt(3)],
        &bindings,
        None,
    );
    assert!(matches!(expect_ok(same, "3 == 3"), Value::Bool(true)));

    let hash = interp.send(
        Value::SmallInt(3),
        Symbol::intern("hash"),
        vec![],
        &bindings,
        None,
    );
    assert!(
        matches!(expect_ok(hash, "3 hash"), Value::SmallInt(_)),
        "Object's native `hash` must be reachable from an Integer receiver"
    );
}

/// The load-bearing half of C1: an unimplemented selector on a builtin must
/// route through `builtins::object_class::dispatch`'s real
/// `doesNotUnderstand:` -- whose message is
/// "<describe(receiver)> does not understand #<selector>" -- rather than
/// `Interpreter::does_not_understand`'s internal
/// "(native class ... has no doesNotUnderstand: implementation)" fallback.
#[test]
fn an_unknown_selector_on_a_builtin_runs_objects_real_does_not_understand() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();

    let result = interp.send(
        Value::SmallInt(3),
        Symbol::intern("frobnicate"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("frobnicate"));
            assert_eq!(
                e.message, "an object does not understand #frobnicate",
                "expected `object_class::dispatch`'s own doesNotUnderstand: message shape"
            );
        }
        Ok(v) => panic!(
            "expected a doesNotUnderstand: error for `3 frobnicate`, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

/// The same, for an object-shaped builtin receiver (`String`), to prove the
/// fallback is not specific to a primitive `Value` variant.
#[test]
fn an_unknown_selector_on_a_string_runs_objects_real_does_not_understand() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();
    let receiver = Value::Obj(interp.new_string("hi".to_string()));

    let result = interp.send(
        receiver,
        Symbol::intern("frobnicate"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => {
            assert_eq!(e.message, "an object does not understand #frobnicate");
        }
        Ok(v) => panic!(
            "expected a doesNotUnderstand: error, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

/// The fallback must never shadow a builtin's *own* implementation of a
/// selector `Object` also implements: `Integer` has its own `=` and
/// `printString`, and those must still win.
#[test]
fn a_builtins_own_implementation_still_wins_over_objects() {
    let mut interp = Interpreter::bootstrap();
    let bindings = empty_bindings();

    // Object's `=` is identity; Integer's own `=` is numeric equality. Both
    // agree on `3 = 3`, so use `printString` -- where the two genuinely
    // differ -- as the discriminating case.
    let printed = interp.send(
        Value::SmallInt(7),
        Symbol::intern("printString"),
        vec![],
        &bindings,
        None,
    );
    assert_eq!(
        as_string(interp.heap_ref(), &expect_ok(printed, "7 printString")),
        "7",
        "Integer's own printString must win over Object's generic default"
    );
}
