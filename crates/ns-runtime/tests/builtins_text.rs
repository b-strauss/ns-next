//! Integration tests for the `String`/`Symbol`/`Character` native
//! methods, wired through `crate::builtins::dispatch_native` via
//! `Interpreter::send`.

use ns_runtime::{Heap, Interpreter, Signal, Symbol, Value};

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
            .str_value(*obj)
            .unwrap_or_else(|| panic!("expected ObjectData::Str, got {}", heap.describe(value))),
        other => panic!("expected Value::Obj, got {}", heap.describe(other)),
    }
}

#[test]
fn string_concatenation_joins_two_strings() {
    let mut interp = Interpreter::bootstrap();

    let a = Value::Obj(interp.new_string("foo".to_string()));
    let b = Value::Obj(interp.new_string("bar".to_string()));

    let result = interp.send(a, Symbol::intern(","), vec![b]);

    let joined = expect_ok(result, "'foo' , 'bar'");
    assert_eq!(as_string(interp.heap_ref(), &joined), "foobar");
}

#[test]
fn string_concatenation_with_a_non_string_argument_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::bootstrap();

    let a = Value::Obj(interp.new_string("foo".to_string()));

    let result = interp.send(a, Symbol::intern(","), vec![Value::SmallInt(42)]);

    match result {
        Err(Signal::Error(_)) => {}
        Ok(v) => panic!(
            "expected a NewspeakError for 'foo' , 42, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

#[test]
fn concatenating_a_string_with_an_object_is_an_error_not_a_stack_overflow() {
    // This message must not `{:?}`-format the argument: a `Value::Obj`
    // there would recurse through `Object`'s self-referential bootstrap
    // class until the stack gives out, aborting the process instead of
    // reporting. Every other test here passes a `SmallInt`, which formats
    // harmlessly and so could not catch it.
    let mut interp = Interpreter::bootstrap();

    let a = Value::Obj(interp.new_string("foo".to_string()));
    let arg = Value::Obj(interp.new_array(vec![]));

    let result = interp.send(a, Symbol::intern(","), vec![arg]);

    match result {
        Err(Signal::Error(e)) => assert!(
            e.message.contains("requires a String argument"),
            "unexpected message: {}",
            e.message
        ),
        Ok(v) => panic!(
            "expected a NewspeakError for 'foo' , anArray, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}

#[test]
fn string_equality_compares_payloads() {
    let mut interp = Interpreter::bootstrap();

    let a = Value::Obj(interp.new_string("same".to_string()));
    let b = Value::Obj(interp.new_string("same".to_string()));
    let c = Value::Obj(interp.new_string("different".to_string()));

    let equal = expect_ok(
        interp.send(a, Symbol::intern("="), vec![b]),
        "'same' = 'same'",
    );
    assert!(matches!(equal, Value::Bool(true)));

    let not_equal = expect_ok(
        interp.send(a, Symbol::intern("="), vec![c]),
        "'same' = 'different'",
    );
    assert!(matches!(not_equal, Value::Bool(false)));
}

#[test]
fn string_print_string_quotes_and_doubles_embedded_quotes() {
    let mut interp = Interpreter::bootstrap();

    let s = Value::Obj(interp.new_string("it's".to_string()));

    let result = expect_ok(
        interp.send(s, Symbol::intern("printString"), vec![]),
        "'it''s' printString",
    );

    // Mirrors `ns_syntax::lexer::Lexer::lex_string`'s own escaping rule: an
    // embedded `'` is written as two consecutive `'` characters.
    assert_eq!(as_string(interp.heap_ref(), &result), "'it''s'");
}

#[test]
fn symbol_round_trips_through_as_string_and_as_symbol() {
    let mut interp = Interpreter::bootstrap();

    let sym = interp.new_symbol(Symbol::intern("foo:bar:"));

    let as_str = expect_ok(
        interp.send(sym, Symbol::intern("asString"), vec![]),
        "#foo:bar: asString",
    );
    assert_eq!(as_string(interp.heap_ref(), &as_str), "foo:bar:");

    let back_to_symbol = expect_ok(
        interp.send(as_str, Symbol::intern("asSymbol"), vec![]),
        "'foo:bar:' asSymbol",
    );
    match &back_to_symbol {
        Value::Obj(obj) => match interp.heap_ref().sym_value(*obj) {
            Some(s) => assert_eq!(s, Symbol::intern("foo:bar:")),
            None => panic!(
                "expected ObjectData::Sym, got {}",
                interp.heap_ref().describe(&back_to_symbol)
            ),
        },
        other => panic!(
            "expected Value::Obj, got {}",
            interp.heap_ref().describe(other)
        ),
    }
}

#[test]
fn character_constant_as_integer_yields_its_unicode_code_point() {
    let mut interp = Interpreter::bootstrap();

    let a = Value::Obj(interp.new_character('A'));

    let result = expect_ok(
        interp.send(a, Symbol::intern("asInteger"), vec![]),
        "$A asInteger",
    );

    assert!(matches!(result, Value::SmallInt(65)));
}
