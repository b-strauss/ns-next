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

fn as_string(value: &Value) -> String {
    match value {
        Value::Obj(obj) => match &obj.borrow().data {
            ns_runtime::ObjectData::Str(s) => s.clone(),
            other => panic!("expected ObjectData::Str, got {other:?}"),
        },
        other => panic!("expected Value::Obj, got {other:?}"),
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

/// Controller review fix (second pass): `and:`'s block argument invocation
/// used to go through `truthy_arg`, which collapsed *any* non-`Ok(Bool(true))`
/// outcome -- including a live `Signal::Return` -- into plain `false`. A
/// non-local return inside an `and:` block must actually unwind past the
/// `and:` send, not get swallowed. `m`'s body sends `true and: [^'escaped']`
/// as a bare statement (its value discarded either way) followed by a
/// second, distinguishing statement `^'fell through'` -- if the bug were
/// still present, `and:` would silently evaluate to `false`, execution
/// would fall through to the second statement, and `m` would return
/// `'fell through'`; with the fix, the block's `^'escaped'` must unwind
/// `m`'s own activation directly (the block's home, since the block literal
/// is evaluated directly inside `m`'s own frame), so `m` returns `'escaped'`
/// and the second statement never runs at all.
#[test]
fn and_block_arguments_non_local_return_actually_escapes_not_becomes_false() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( true and: [^'escaped']. ^'fell through' )\n) : ()";
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

    assert_eq!(
        as_string(&expect_ok(result, "true and: [^'escaped']. ^'fell through'")),
        "escaped",
        "the block's non-local return must unwind `m`'s own activation directly, not be swallowed into `false` and fall through to the second statement"
    );
}

/// Quick sanity check (controller review, second pass): the fix above must
/// not regress `and:`/`or:`'s short-circuit behavior -- the untaken block
/// argument must still never be invoked at all. Uses the same
/// distinguishing-statement technique: if the short-circuited block *were*
/// wrongly invoked, its `^'wrongly invoked'` would unwind `m` directly and
/// this test would see that value instead of `'correct'`.
#[test]
fn and_or_short_circuit_never_invokes_the_untaken_block() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( false and: [^'wrongly invoked']. true or: [^'wrongly invoked']. ^'correct' )\n) : ()";
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

    assert_eq!(
        as_string(&expect_ok(
            result,
            "false and: [...]. true or: [...]. ^'correct'"
        )),
        "correct",
        "false and: [...] and true or: [...] must never invoke their block argument at all"
    );
}

/// Final-review finding I7: Milestone 1 has no Integer/Float coercion. That
/// is a documented gap, but it used to fail *dishonestly*: `1 + 1.5` went
/// through `arg_int()`, which only accepts `Value::SmallInt`, and reported
/// the resulting `None` as an "Integer overflow". The gap stays; the message
/// now says what actually happened.
#[test]
fn mixed_integer_float_arithmetic_reports_a_type_mismatch_not_an_overflow() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(1),
        Symbol::intern("+"),
        vec![Value::Float(1.5)],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => {
            assert!(
                !e.message.to_lowercase().contains("overflow"),
                "a type mismatch must not be reported as an overflow: {}",
                e.message
            );
            assert!(
                e.message.contains("coercion"),
                "expected the message to name the missing Integer/Float coercion, got: {}",
                e.message
            );
        }
        Ok(v) => panic!("expected a NewspeakError for `1 + 1.5`, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

/// Same finding, the comparison half: an ordering comparison against a
/// type-mismatched argument used to answer a silent `false`.
#[test]
fn an_ordering_comparison_against_a_mismatched_type_is_an_error_not_false() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(1),
        Symbol::intern("<"),
        vec![Value::Float(1.5)],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => assert!(e.message.contains("coercion"), "got: {}", e.message),
        Ok(Value::Bool(false)) => {
            panic!("`1 < 1.5` silently answered false instead of reporting the type mismatch")
        }
        Ok(v) => panic!("expected a NewspeakError, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

/// `=` is the deliberate exception: a type-mismatched argument is `false`,
/// never an error (Object's own `=` convention). This pins that the I7
/// change did not turn equality into an error too.
#[test]
fn equality_against_a_mismatched_type_stays_false_and_is_never_an_error() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(1),
        Symbol::intern("="),
        vec![Value::Float(1.0)],
        &bindings,
        None,
    );

    match expect_ok(result, "1 = 1.0") {
        Value::Bool(false) => {}
        other => panic!("expected Value::Bool(false), got {other:?}"),
    }
}

/// The one direction Milestone 1 *does* widen: `Float`'s own arithmetic
/// accepts an `Integer` argument (lossless), and must keep doing so.
#[test]
fn float_arithmetic_still_widens_an_integer_argument() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::Float(1.5),
        Symbol::intern("+"),
        vec![Value::SmallInt(1)],
        &bindings,
        None,
    );

    match expect_ok(result, "1.5 + 1") {
        Value::Float(f) => assert!((f - 2.5).abs() < f64::EPSILON, "expected 2.5, got {f}"),
        other => panic!("expected Value::Float(2.5), got {other:?}"),
    }
}

/// A genuine overflow must still be reported as an overflow -- the I7 change
/// only split the *type mismatch* case out of that message.
#[test]
fn a_real_integer_overflow_is_still_reported_as_an_overflow() {
    let mut interp = Interpreter::bootstrap();
    let bindings: Rc<HashMap<ns_syntax::Span, ns_runtime::Binding>> = Rc::new(HashMap::new());

    let result = interp.send(
        Value::SmallInt(i64::MAX),
        Symbol::intern("*"),
        vec![Value::SmallInt(2)],
        &bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => assert!(
            e.message.to_lowercase().contains("overflow"),
            "expected an overflow-shaped message, got: {}",
            e.message
        ),
        Ok(v) => panic!("expected a NewspeakError, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

/// Final-review finding I5, end to end: a radix integer literal in real
/// source now evaluates instead of failing with a misleading "integer
/// literal out of i64 range".
#[test]
fn a_radix_integer_literal_evaluates() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^16r1F )\n) : ()";
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
    match expect_ok(result, "^16r1F") {
        Value::SmallInt(31) => {}
        other => panic!("expected Value::SmallInt(31), got {other:?}"),
    }
}
