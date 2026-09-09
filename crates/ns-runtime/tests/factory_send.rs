//! Tests for the `Interpreter::send`-level `ClassName new` -> `instantiate`
//! wiring (see `interpreter/instantiate.rs`'s `try_factory_send`). These
//! tests go through `Interpreter::send` rather than calling
//! `Interpreter::instantiate` directly from Rust -- the code path an
//! ordinary Newspeak `ClassName new` message send actually takes.

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

/// `OrderedCollection new` via ordinary `send` (not a direct
/// `interp.instantiate(...)` call) must produce an `ObjectData::Array`-backed
/// object -- the payload shape `builtins::collections`' dispatch functions
/// actually pattern-match on -- and that object must go on to
/// support `add:`/`size` through the real dispatch chain, which also proves
/// the result stays classed as `OrderedCollection` (not `Array`: `Array`'s
/// own native dispatch has no `add:`, so `add:` succeeding rules out the
/// mis-tagging bug this test was written to catch).
#[test]
fn ordered_collection_new_via_ordinary_send_is_array_backed_and_supports_add_and_size() {
    let mut interp = Interpreter::bootstrap();
    let class_value = Value::Obj(interp.builtin_class(BuiltinClass::OrderedCollection));
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(class_value, Symbol::intern("new"), vec![], &bindings, None);
    let receiver = expect_ok(result, "OrderedCollection new");

    let Value::Obj(obj) = &receiver else {
        panic!(
            "expected Value::Obj, got {}",
            interp.heap_ref().describe(&receiver)
        );
    };
    let elements = interp.heap_ref().array_elements(obj);
    assert!(
        matches!(&elements, Some(items) if items.is_empty()),
        "expected an empty ObjectData::Array payload"
    );

    let add_result = expect_ok(
        interp.send(
            receiver,
            Symbol::intern("add:"),
            vec![Value::SmallInt(1)],
            &bindings,
            None,
        ),
        "OrderedCollection new add: 1",
    );
    match add_result {
        Value::SmallInt(1) => {}
        other => panic!(
            "expected add: to return its argument, got {}",
            interp.heap_ref().describe(&other)
        ),
    }

    let size_result = expect_ok(
        interp.send(receiver, Symbol::intern("size"), vec![], &bindings, None),
        "OrderedCollection new (after add:) size",
    );
    match size_result {
        Value::SmallInt(1) => {}
        other => panic!(
            "expected Value::SmallInt(1), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// `Array new` via ordinary `send` must also be `Array`-tagged (not
/// `OrderedCollection`), confirmed here by `add:` *failing* --
/// `dispatch_array` has no `add:` arm.
#[test]
fn array_new_via_ordinary_send_does_not_understand_add() {
    let mut interp = Interpreter::bootstrap();
    let class_value = Value::Obj(interp.builtin_class(BuiltinClass::Array));
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let receiver = expect_ok(
        interp.send(class_value, Symbol::intern("new"), vec![], &bindings, None),
        "Array new",
    );

    match interp.send(
        receiver,
        Symbol::intern("add:"),
        vec![Value::SmallInt(1)],
        &bindings,
        None,
    ) {
        Err(Signal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("add:"));
        }
        Ok(v) => panic!(
            "expected doesNotUnderstand: add:, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("expected doesNotUnderstand: add:, got Signal::Return"),
    }
}

/// A user-defined class's `new` via ordinary `send` must behave exactly
/// like calling `interp.instantiate(...)` directly: same slot values
/// reachable afterward through a real method send.
#[test]
fn user_defined_class_new_via_ordinary_send_matches_direct_instantiate() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (|x = 42.|) (public bar = (^x)) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");

    // Path A: `Foo new` via ordinary `send`.
    let mut interp_a = Interpreter::bootstrap();
    let class_a = interp_a.load_module(&resolved);
    let class_value_a = Value::Obj(class_a);
    let via_send = expect_ok(
        interp_a.send(
            class_value_a,
            Symbol::intern("new"),
            vec![],
            &resolved.bindings,
            None,
        ),
        "Foo new (via send)",
    );
    let via_send_bar = expect_ok(
        interp_a.send(
            via_send,
            Symbol::intern("bar"),
            vec![],
            &resolved.bindings,
            None,
        ),
        "Foo new bar (via send)",
    );

    // Path B: `interp.instantiate(...)` called directly.
    let mut interp_b = Interpreter::bootstrap();
    let class_b = interp_b.load_module(&resolved);
    let via_instantiate = expect_ok(
        interp_b.instantiate(class_b, vec![], &resolved.bindings),
        "instantiate(Foo)",
    );
    let via_instantiate_bar = expect_ok(
        interp_b.send(
            via_instantiate,
            Symbol::intern("bar"),
            vec![],
            &resolved.bindings,
            None,
        ),
        "instantiate(Foo) bar",
    );

    match (via_send_bar, via_instantiate_bar) {
        (Value::SmallInt(a), Value::SmallInt(b)) => assert_eq!(a, b),
        (a, b) => panic!(
            "expected two equal Value::SmallInt, got {} and {}",
            interp_a.heap_ref().describe(&a),
            interp_b.heap_ref().describe(&b)
        ),
    }
}

/// A class object receiving a selector that is *not* its factory selector
/// still goes through ordinary dispatch (falls through to
/// `doesNotUnderstand:` here, since class objects have no other native
/// surface in Milestone 1) -- proves `try_factory_send` only intercepts the
/// exact factory selector, not every send to a class object.
#[test]
fn a_class_object_receiving_a_non_factory_selector_falls_through_to_ordinary_dispatch() {
    let mut interp = Interpreter::bootstrap();
    let class_value = Value::Obj(interp.builtin_class(BuiltinClass::OrderedCollection));
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    match interp.send(
        class_value,
        Symbol::intern("thisSelectorDoesNotExist"),
        vec![],
        &bindings,
        None,
    ) {
        Err(Signal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(v) => panic!(
            "expected doesNotUnderstand:, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("expected doesNotUnderstand:, got Signal::Return"),
    }
}
