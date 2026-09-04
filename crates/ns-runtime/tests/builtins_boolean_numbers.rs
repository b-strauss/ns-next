//! Integration tests for Task 8's `Boolean`/`True`/`False` and
//! `Integer`/`Float` native methods, wired through
//! `crate::builtins::dispatch_native` via `Interpreter::send`.

use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::{Interpreter, Signal, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both --
/// same pattern as `tests/instantiate.rs`.
fn load(
    src: &str,
) -> (
    Interpreter,
    ns_runtime::ClassRef,
    ns_runtime::ResolvedModule,
) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, resolved)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn integer_addition_via_direct_send_evaluates_correctly() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(3),
        Symbol::intern("+"),
        vec![Value::SmallInt(4)],
        &bindings,
        None,
    );

    match expect_ok(result, "3 + 4") {
        Value::SmallInt(7) => {}
        other => panic!("expected Value::SmallInt(7), got {other:?}"),
    }
}

#[test]
fn integer_addition_overflow_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(i64::MAX),
        Symbol::intern("+"),
        vec![Value::SmallInt(1)],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => {
            assert!(
                e.message.to_lowercase().contains("overflow"),
                "expected an overflow-shaped error message, got: {}",
                e.message
            );
        }
        Ok(v) => panic!("expected a NewspeakError for i64::MAX + 1, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

#[test]
fn boolean_if_true_if_false_selects_the_right_block_through_a_real_fixture() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^true ifTrue: [1] ifFalse: [2] )\n) : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );

    match expect_ok(result, "true ifTrue:ifFalse:") {
        Value::SmallInt(1) => {}
        other => panic!("expected Value::SmallInt(1), got {other:?}"),
    }
}

/// Regression coverage for the `ClosureData.bindings` fix: the block body
/// here (`[ x ]`) reads `x`, a slot on the enclosing class, resolved as a
/// `Binding::LexicalSend` keyed by this block's own `Span` in the resolver's
/// bindings map. If `invoke_block` (or `invoke_closure`'s caller more
/// generally) invoked this closure with a fresh, empty bindings map instead
/// of the one the block was actually resolved under (the bug the brief
/// flagged), `x`'s `Span` would not be present in that empty map and
/// `eval_ident` would fall through to its `None` arm -- an "unresolved
/// identifier" `NewspeakError`, not `SmallInt(5)`.
#[test]
fn if_true_block_body_resolves_an_enclosing_slot_reference() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (|x = 5.|) (\n  public m = ( ^true ifTrue: [ x ] ifFalse: [ 0 ] )\n) : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );

    match expect_ok(result, "true ifTrue: [ x ] ifFalse: [ 0 ]") {
        Value::SmallInt(5) => {}
        other => panic!("expected Value::SmallInt(5), got {other:?}"),
    }
}
