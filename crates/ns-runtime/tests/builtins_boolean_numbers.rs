//! Integration tests for the `Boolean`/`True`/`False` and
//! `Integer`/`Float` native methods, found as primitives by the ordinary
//! lookup via `Interpreter::send`.

use indoc::{formatdoc, indoc};
use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both --
/// same pattern as `tests/instantiate.rs`.
fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

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

#[test]
fn integer_addition_via_direct_send_evaluates_correctly() {
    let mut interpreter = Interpreter::bootstrap();

    let three = interpreter.stash(Value::SmallInteger(3));
    let result = interpreter.send(
        &three,
        Symbol::intern("+"),
        &[interpreter.stash(Value::SmallInteger(4))],
    );

    let sum = expect_ok(result, "3 + 4");
    interpreter.enter(|mutator| match mutator.fetch(&sum) {
        Value::SmallInteger(7) => {}
        other => panic!(
            "expected Value::SmallInteger(7), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// §5.1.1: "integers are not limited in size", so an overflowing `+`
/// answers a `LargeInteger` rather than raising.
#[test]
fn integer_addition_overflow_answers_a_large_integer() {
    let mut interpreter = Interpreter::bootstrap();
    let max = interpreter.stash(Value::SmallInteger(i64::MAX));
    let sum = expect_ok(
        interpreter.send(
            &max,
            Symbol::intern("+"),
            &[interpreter.stash(Value::SmallInteger(1))],
        ),
        "i64::MAX + 1",
    );
    let printed = expect_ok(
        interpreter.send(&sum, Symbol::intern("printString"), &[]),
        "printString",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&printed)),
            "9223372036854775808"
        );
    });
}

#[test]
fn boolean_if_true_if_false_selects_the_right_block_through_a_real_fixture() {
    for body in [
        "^true ifTrue: [1] ifFalse: [2]",
        "| t f | t:: [1]. f:: [2]. ^true ifTrue: t ifFalse: f",
    ] {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class Foo = Object new (
            ) (
              public m = ( {body} )
            ) : ()
        "};
        let (mut interpreter, class) = load(&source);
        let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
        let result = expect_ok(result, body);
        interpreter.enter(|mutator| match mutator.fetch(&result) {
            Value::SmallInteger(1) => {}
            other => panic!(
                "{body}: expected Value::SmallInteger(1), got {}",
                mutator.heap().describe(&other)
            ),
        });
    }
}

/// The block body here (`[ x ]`) reads `x`, a slot on the enclosing class,
/// resolved as a `Binding::LexicalSend` and lowered into a lexical send, so
/// it must answer `SmallInteger(5)` whether the block is inlined or -- the
/// variable form -- reaches the native `ifTrue:ifFalse:` as a real closure.
#[test]
fn if_true_block_body_resolves_an_enclosing_slot_reference() {
    for body in [
        "^true ifTrue: [ x ] ifFalse: [ 0 ]",
        "| t f | t:: [ x ]. f:: [ 0 ]. ^true ifTrue: t ifFalse: f",
    ] {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class Foo = Object new (|x = 5.|) (
              public m = ( {body} )
            ) : ()
        "};
        let (mut interpreter, class) = load(&source);
        let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
        let result = expect_ok(result, body);
        interpreter.enter(|mutator| match mutator.fetch(&result) {
            Value::SmallInteger(5) => {}
            other => panic!(
                "{body}: expected Value::SmallInteger(5), got {}",
                mutator.heap().describe(&other)
            ),
        });
    }
}

/// A non-local return inside an `and:` block must actually unwind past the
/// `and:` send, not get swallowed: collapsing *any* non-`Ok(Boolean(true))`
/// outcome of the block -- including a live `Signal::Return` -- into plain
/// `false` is the failure this guards against. `m`'s body sends
/// `true and: [^'escaped']` as a bare statement (its value discarded either
/// way) followed by a second, distinguishing statement `^'fell through'` --
/// were the return swallowed, `and:` would silently evaluate to `false`,
/// execution would fall through to the second statement, and `m` would
/// return `'fell through'`. Instead the block's `^'escaped'` must unwind
/// `m`'s own activation directly (the block's home, since the block literal
/// is evaluated directly inside `m`'s own frame), so `m` returns `'escaped'`
/// and the second statement never runs at all.
#[test]
fn and_block_arguments_non_local_return_actually_escapes_not_becomes_false() {
    for body in [
        "true and: [^'escaped']. ^'fell through'",
        "| b | b:: [^'escaped']. true and: b. ^'fell through'",
    ] {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class Foo = Object new (
            ) (
              public m = ( {body} )
            ) : ()
        "};
        let (mut interpreter, class) = load(&source);
        let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
        let result = expect_ok(result, body);
        interpreter.enter(|mutator| {
            assert_eq!(
                as_string(mutator.heap(), &mutator.fetch(&result)),
                "escaped",
                "the block's non-local return must unwind `m`'s own activation directly, not be swallowed into `false` and fall through to the second statement"
            );
        });
    }
}

/// The counterpart to the test above: propagating the block's outcome must
/// not cost `and:`/`or:` their short circuit -- the untaken block argument
/// must never be invoked at all. Uses the same
/// distinguishing-statement technique: if the short-circuited block *were*
/// wrongly invoked, its `^'wrongly invoked'` would unwind `m` directly and
/// this test would see that value instead of `'correct'`.
#[test]
fn and_or_short_circuit_never_invokes_the_untaken_block() {
    for body in [
        "false and: [^'wrongly invoked']. true or: [^'wrongly invoked']. ^'correct'",
        "| b | b:: [^'wrongly invoked']. false and: b. true or: b. ^'correct'",
    ] {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class Foo = Object new (
            ) (
              public m = ( {body} )
            ) : ()
        "};
        let (mut interpreter, class) = load(&source);
        let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
        let result = expect_ok(result, body);
        interpreter.enter(|mutator| {
            assert_eq!(
                as_string(mutator.heap(), &mutator.fetch(&result)),
                "correct",
                "false and: [...] and true or: [...] must never invoke their block argument at all"
            );
        });
    }
}

/// An integer meets a `Float` argument as a `Float` meets an integer one:
/// `1 + 1.5` is the double `2.5` (`builtins/numbers.rs`'s module note).
#[test]
fn integer_arithmetic_widens_a_float_argument() {
    let mut interpreter = Interpreter::bootstrap();

    let one = interpreter.stash(Value::SmallInteger(1));
    let result = interpreter.send(
        &one,
        Symbol::intern("+"),
        &[interpreter.stash(Value::Float(1.5))],
    );

    let answer = expect_ok(result, "1 + 1.5");
    interpreter.enter(|mutator| match mutator.fetch(&answer) {
        Value::Float(sum) => assert_eq!(sum, 2.5),
        other => panic!(
            "expected the double 2.5, got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// `type_mismatch`'s message builds off `heap.describe_argument`, but the
/// other type-mismatch tests here drive a `Value::Nil` into it, which formats
/// harmlessly either way. A `Value::Object` there would recurse through
/// `Object`'s self-referential bootstrap class until the stack gives out if
/// the message ever `{:?}`-formatted the argument directly, so this drives
/// one in -- the same shape as
/// `builtins_text.rs`'s `concatenating_a_string_with_an_object_is_an_error_not_a_stack_overflow`
/// and `builtins_collections_block.rs`'s
/// `array_at_with_an_object_index_is_an_error_not_a_stack_overflow`.
#[test]
fn integer_arithmetic_with_an_object_argument_is_an_error_not_a_stack_overflow() {
    let mut interpreter = Interpreter::bootstrap();

    let argument = interpreter.enter(|mutator| {
        let array = mutator.new_array(vec![]);
        mutator.stash(Value::Object(array))
    });
    let one = interpreter.stash(Value::SmallInteger(1));

    let result = interpreter.send(&one, Symbol::intern("+"), &[argument]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert!(
                error
                    .message
                    .starts_with("+ requires a numeric argument, got "),
                "expected the message to name what an integer takes, got: {}",
                error.message
            );
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a Newspeak error for `1 + anArray`, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// The comparison half of the same gap: an ordering comparison against a
/// type-mismatched argument must be an error, not a silent `false`.
#[test]
fn an_ordering_comparison_against_a_mismatched_type_is_an_error_not_false() {
    let mut interpreter = Interpreter::bootstrap();

    let one = interpreter.stash(Value::SmallInteger(1));
    let result = interpreter.send(&one, Symbol::intern("<"), &[interpreter.stash(Value::Nil)]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.message, "< requires a numeric argument, got nil")
        }
        Ok(value) => interpreter.enter(|mutator| match mutator.fetch(&value) {
            Value::Boolean(false) => {
                panic!("`1 < nil` silently answered false instead of reporting the type mismatch")
            }
            other => panic!(
                "expected a Newspeak error, got Ok({})",
                mutator.heap().describe(&other)
            ),
        }),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// A `Float` receiver takes any number, so its mismatch asks for a number, as
/// an integer's arithmetic does too; only `//`, `\\` and the bit operations
/// ask for an integer.
#[test]
fn a_float_receiver_asks_for_a_numeric_argument() {
    let mut interpreter = Interpreter::bootstrap();

    let receiver = interpreter.stash(Value::Float(1.5));
    let result = interpreter.send(
        &receiver,
        Symbol::intern("*"),
        &[interpreter.stash(Value::Nil)],
    );

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.message, "* requires a numeric argument, got nil")
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a Newspeak error for `1.5 * nil`, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// `=` is the deliberate exception: a type-mismatched argument is `false`,
/// never an error (Object's own `=` convention). This pins that making
/// ordering comparisons strict did not turn equality into an error too.
#[test]
fn equality_against_a_mismatched_type_stays_false_and_is_never_an_error() {
    let mut interpreter = Interpreter::bootstrap();

    let one = interpreter.stash(Value::SmallInteger(1));
    let result = interpreter.send(
        &one,
        Symbol::intern("="),
        &[interpreter.stash(Value::Float(1.0))],
    );

    let result = expect_ok(result, "1 = 1.0");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::Boolean(false) => {}
        other => panic!(
            "expected Value::Boolean(false), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// The one direction the runtime *does* widen: `Float`'s own arithmetic
/// accepts an `Integer` argument (lossless), and must keep doing so.
#[test]
fn float_arithmetic_still_widens_an_integer_argument() {
    let mut interpreter = Interpreter::bootstrap();

    let one_and_a_half = interpreter.stash(Value::Float(1.5));
    let result = interpreter.send(
        &one_and_a_half,
        Symbol::intern("+"),
        &[interpreter.stash(Value::SmallInteger(1))],
    );

    let result = expect_ok(result, "1.5 + 1");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::Float(float) => assert!(
            (float - 2.5).abs() < f64::EPSILON,
            "expected 2.5, got {float}"
        ),
        other => panic!(
            "expected Value::Float(2.5), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// §5.1.1, as for `+`: an overflowing `*` answers a `LargeInteger`.
#[test]
fn integer_multiplication_overflow_answers_a_large_integer() {
    let mut interpreter = Interpreter::bootstrap();
    let max = interpreter.stash(Value::SmallInteger(i64::MAX));
    let product = expect_ok(
        interpreter.send(
            &max,
            Symbol::intern("*"),
            &[interpreter.stash(Value::SmallInteger(2))],
        ),
        "i64::MAX * 2",
    );
    let printed = expect_ok(
        interpreter.send(&product, Symbol::intern("printString"), &[]),
        "printString",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&printed)),
            "18446744073709551614"
        );
    });
}

/// End to end: a radix integer literal in real source evaluates, rather than
/// failing with a misleading "integer literal out of i64 range".
#[test]
fn a_radix_integer_literal_evaluates() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( ^16r1F )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "^16r1F");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(31) => {}
        other => panic!(
            "expected Value::SmallInteger(31), got {}",
            mutator.heap().describe(&other)
        ),
    });
}
