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
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
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
    let class_value = interp.enter(|m| {
        let class = m.builtin_class(BuiltinClass::OrderedCollection);
        m.stash(Value::Obj(class))
    });

    let result = interp.send(&class_value, Symbol::intern("new"), &[]);
    let receiver = expect_ok(result, "OrderedCollection new");

    interp.enter(|m| {
        let fetched = m.fetch(&receiver);
        let Value::Obj(obj) = &fetched else {
            panic!("expected Value::Obj, got {}", m.heap().describe(&fetched));
        };
        let elements = m.heap().array_elements(*obj);
        assert!(
            matches!(&elements, Some(items) if items.is_empty()),
            "expected an empty ObjectData::Array payload"
        );
    });

    let add_result = expect_ok(
        interp.send(
            &receiver,
            Symbol::intern("add:"),
            &[interp.stash(Value::SmallInt(1))],
        ),
        "OrderedCollection new add: 1",
    );
    interp.enter(|m| match m.fetch(&add_result) {
        Value::SmallInt(1) => {}
        other => panic!(
            "expected add: to return its argument, got {}",
            m.heap().describe(&other)
        ),
    });

    let size_result = expect_ok(
        interp.send(&receiver, Symbol::intern("size"), &[]),
        "OrderedCollection new (after add:) size",
    );
    interp.enter(|m| match m.fetch(&size_result) {
        Value::SmallInt(1) => {}
        other => panic!(
            "expected Value::SmallInt(1), got {}",
            m.heap().describe(&other)
        ),
    });
}

/// `Array new` via ordinary `send` must also be `Array`-tagged (not
/// `OrderedCollection`), confirmed here by `add:` *failing* --
/// `dispatch_array` has no `add:` arm.
#[test]
fn array_new_via_ordinary_send_does_not_understand_add() {
    let mut interp = Interpreter::bootstrap();
    let class_value = interp.enter(|m| {
        let class = m.builtin_class(BuiltinClass::Array);
        m.stash(Value::Obj(class))
    });

    let receiver = expect_ok(
        interp.send(&class_value, Symbol::intern("new"), &[]),
        "Array new",
    );

    match interp.send(
        &receiver,
        Symbol::intern("add:"),
        &[interp.stash(Value::SmallInt(1))],
    ) {
        Err(HostSignal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("add:"));
        }
        Ok(v) => {
            let got = interp.enter(|m| m.heap().describe(&m.fetch(&v)));
            panic!("expected doesNotUnderstand: add:, got Ok({got})")
        }
        Err(HostSignal::Return) => {
            panic!("expected doesNotUnderstand: add:, got HostSignal::Return")
        }
    }
}

/// A user-defined class's `new` via ordinary `send` must behave exactly
/// like calling `interp.instantiate(...)` directly: same slot values
/// reachable afterward through a real method send.
#[test]
fn user_defined_class_new_via_ordinary_send_matches_direct_instantiate() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|x = 42.|) (public bar = (^x)) : ()
    "};
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
    let via_send = expect_ok(
        interp_a.send(&class_a, Symbol::intern("new"), &[]),
        "Foo new (via send)",
    );
    let via_send_bar = expect_ok(
        interp_a.send(&via_send, Symbol::intern("bar"), &[]),
        "Foo new bar (via send)",
    );

    // Path B: `interp.instantiate(...)` called directly.
    let mut interp_b = Interpreter::bootstrap();
    let class_b = interp_b.load_module(&resolved);
    let via_instantiate = expect_ok(interp_b.instantiate(&class_b, &[]), "instantiate(Foo)");
    let via_instantiate_bar = expect_ok(
        interp_b.send(&via_instantiate, Symbol::intern("bar"), &[]),
        "instantiate(Foo) bar",
    );

    // Each side leaves its interpreter's entry as the integer it answered,
    // or as the description of whatever else it answered.
    let a = interp_a.enter(|m| match m.fetch(&via_send_bar) {
        Value::SmallInt(a) => Ok(a),
        other => Err(m.heap().describe(&other)),
    });
    let b = interp_b.enter(|m| match m.fetch(&via_instantiate_bar) {
        Value::SmallInt(b) => Ok(b),
        other => Err(m.heap().describe(&other)),
    });
    match (a, b) {
        (Ok(a), Ok(b)) => assert_eq!(a, b),
        (a, b) => {
            let da = a.map_or_else(|d| d, |n| n.to_string());
            let db = b.map_or_else(|d| d, |n| n.to_string());
            panic!("expected two equal Value::SmallInt, got {da} and {db}")
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
    let mut interp = Interpreter::bootstrap();
    let class_value = interp.enter(|m| {
        let class = m.builtin_class(BuiltinClass::OrderedCollection);
        m.stash(Value::Obj(class))
    });

    match interp.send(
        &class_value,
        Symbol::intern("thisSelectorDoesNotExist"),
        &[],
    ) {
        Err(HostSignal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(v) => {
            let got = interp.enter(|m| m.heap().describe(&m.fetch(&v)));
            panic!("expected doesNotUnderstand:, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("expected doesNotUnderstand:, got HostSignal::Return"),
    }
}
