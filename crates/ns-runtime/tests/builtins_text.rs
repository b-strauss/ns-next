//! Integration tests for the `String`/`Symbol`/`Character` native
//! methods, wired through `crate::builtins::dispatch_native` via
//! `Interpreter::send`.

use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// A new string, rooted.
fn string(interpreter: &mut Interpreter, text: &str) -> Root {
    interpreter.enter(|mutator| {
        let object = mutator.new_string(text.to_string());
        mutator.stash(Value::Object(object))
    })
}

fn as_string(heap: &Heap, value: &Value) -> String {
    match value {
        Value::Object(object) => heap
            .str_value(*object)
            .unwrap_or_else(|| panic!("expected ObjectData::String, got {}", heap.describe(value))),
        other => panic!("expected Value::Object, got {}", heap.describe(other)),
    }
}

#[test]
fn string_concatenation_joins_two_strings() {
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");
    let bar = string(&mut interpreter, "bar");

    let result = interpreter.send(&foo, Symbol::intern(","), &[bar]);

    let joined = expect_ok(result, "'foo' , 'bar'");
    interpreter.enter(|mutator| {
        assert_eq!(as_string(mutator.heap(), &mutator.fetch(&joined)), "foobar");
    });
}

#[test]
fn string_concatenation_with_a_non_string_argument_is_a_newspeak_error_not_a_panic() {
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");

    let result = interpreter.send(
        &foo,
        Symbol::intern(","),
        &[interpreter.stash(Value::SmallInteger(42))],
    );

    match result {
        Err(HostSignal::Error(_)) => {}
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a Newspeak error for 'foo' , 42, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

#[test]
fn concatenating_a_string_with_an_object_is_an_error_not_a_stack_overflow() {
    // This message must not `{:?}`-format the argument: a `Value::Object`
    // there would recurse through `Object`'s self-referential bootstrap
    // class until the stack gives out, aborting the process instead of
    // reporting. Every other test here passes a `SmallInteger`, which formats
    // harmlessly and so could not catch it.
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");
    let argument = interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![]);
        mutator.stash(Value::Object(object))
    });

    let result = interpreter.send(&foo, Symbol::intern(","), &[argument]);

    match result {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("requires a String argument"),
            "unexpected message: {}",
            error.message
        ),
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a Newspeak error for 'foo' , anArray, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

#[test]
fn string_equality_compares_payloads() {
    let mut interpreter = Interpreter::bootstrap();

    let same = string(&mut interpreter, "same");
    let same_again = string(&mut interpreter, "same");
    let different = string(&mut interpreter, "different");

    let equal = expect_ok(
        interpreter.send(&same, Symbol::intern("="), &[same_again]),
        "'same' = 'same'",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&equal), Value::Boolean(true))));

    let not_equal = expect_ok(
        interpreter.send(&same, Symbol::intern("="), &[different]),
        "'same' = 'different'",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&not_equal), Value::Boolean(false))));
}

#[test]
fn string_print_string_quotes_and_doubles_embedded_quotes() {
    let mut interpreter = Interpreter::bootstrap();

    let string = string(&mut interpreter, "it's");

    let result = expect_ok(
        interpreter.send(&string, Symbol::intern("printString"), &[]),
        "'it''s' printString",
    );

    // Mirrors `ns_syntax::lexer::Lexer::lex_string`'s own escaping rule: an
    // embedded `'` is written as two consecutive `'` characters.
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&result)),
            "'it''s'"
        );
    });
}

#[test]
fn symbol_round_trips_through_as_string_and_as_symbol() {
    let mut interpreter = Interpreter::bootstrap();

    let symbol = interpreter.enter(|mutator| {
        let symbol = mutator.new_symbol(Symbol::intern("foo:bar:"));
        mutator.stash(symbol)
    });

    let as_str = expect_ok(
        interpreter.send(&symbol, Symbol::intern("asString"), &[]),
        "#foo:bar: asString",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), &mutator.fetch(&as_str)),
            "foo:bar:"
        );
    });

    let back_to_symbol = expect_ok(
        interpreter.send(&as_str, Symbol::intern("asSymbol"), &[]),
        "'foo:bar:' asSymbol",
    );
    interpreter.enter(|mutator| match mutator.fetch(&back_to_symbol) {
        Value::Object(object) => match mutator.heap().symbol_value(object) {
            Some(symbol) => assert_eq!(symbol, Symbol::intern("foo:bar:")),
            None => panic!(
                "expected ObjectData::Symbol, got {}",
                mutator.heap().describe(&Value::Object(object))
            ),
        },
        other => panic!(
            "expected Value::Object, got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn character_constant_as_integer_yields_its_unicode_code_point() {
    let mut interpreter = Interpreter::bootstrap();

    let character = interpreter.enter(|mutator| {
        let object = mutator.new_character('A');
        mutator.stash(Value::Object(object))
    });

    let result = expect_ok(
        interpreter.send(&character, Symbol::intern("asInteger"), &[]),
        "$A asInteger",
    );

    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&result), Value::SmallInteger(65))));
}
