//! Integration tests for the `String`/`Symbol`/`Character` native
//! methods, wired through `crate::builtins::dispatch_native` via
//! `Interpreter::send`.

use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

/// A new string, rooted.
fn string(interp: &mut Interpreter, s: &str) -> Root {
    let obj = interp.new_string(s.to_string());
    interp.stash(Value::Obj(obj))
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

    let a = string(&mut interp, "foo");
    let b = string(&mut interp, "bar");

    let result = interp.send(&a, Symbol::intern(","), &[b]);

    let joined = expect_ok(result, "'foo' , 'bar'");
    assert_eq!(
        as_string(interp.heap_ref(), &interp.fetch(&joined)),
        "foobar"
    );
}

#[test]
fn string_concatenation_with_a_non_string_argument_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::bootstrap();

    let a = string(&mut interp, "foo");

    let result = interp.send(
        &a,
        Symbol::intern(","),
        &[interp.stash(Value::SmallInt(42))],
    );

    match result {
        Err(HostSignal::Error(_)) => {}
        Ok(v) => panic!(
            "expected a Newspeak error for 'foo' , 42, got Ok({})",
            interp.heap_ref().describe(&interp.fetch(&v))
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
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

    let a = string(&mut interp, "foo");
    let arg = interp.new_array(vec![]);
    let arg = interp.stash(Value::Obj(arg));

    let result = interp.send(&a, Symbol::intern(","), &[arg]);

    match result {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("requires a String argument"),
            "unexpected message: {}",
            e.message
        ),
        Ok(v) => panic!(
            "expected a Newspeak error for 'foo' , anArray, got Ok({})",
            interp.heap_ref().describe(&interp.fetch(&v))
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

#[test]
fn string_equality_compares_payloads() {
    let mut interp = Interpreter::bootstrap();

    let a = string(&mut interp, "same");
    let b = string(&mut interp, "same");
    let c = string(&mut interp, "different");

    let equal = expect_ok(
        interp.send(&a, Symbol::intern("="), &[b]),
        "'same' = 'same'",
    );
    assert!(matches!(interp.fetch(&equal), Value::Bool(true)));

    let not_equal = expect_ok(
        interp.send(&a, Symbol::intern("="), &[c]),
        "'same' = 'different'",
    );
    assert!(matches!(interp.fetch(&not_equal), Value::Bool(false)));
}

#[test]
fn string_print_string_quotes_and_doubles_embedded_quotes() {
    let mut interp = Interpreter::bootstrap();

    let s = string(&mut interp, "it's");

    let result = expect_ok(
        interp.send(&s, Symbol::intern("printString"), &[]),
        "'it''s' printString",
    );

    // Mirrors `ns_syntax::lexer::Lexer::lex_string`'s own escaping rule: an
    // embedded `'` is written as two consecutive `'` characters.
    assert_eq!(
        as_string(interp.heap_ref(), &interp.fetch(&result)),
        "'it''s'"
    );
}

#[test]
fn symbol_round_trips_through_as_string_and_as_symbol() {
    let mut interp = Interpreter::bootstrap();

    let sym = interp.new_symbol(Symbol::intern("foo:bar:"));
    let sym = interp.stash(sym);

    let as_str = expect_ok(
        interp.send(&sym, Symbol::intern("asString"), &[]),
        "#foo:bar: asString",
    );
    assert_eq!(
        as_string(interp.heap_ref(), &interp.fetch(&as_str)),
        "foo:bar:"
    );

    let back_to_symbol = expect_ok(
        interp.send(&as_str, Symbol::intern("asSymbol"), &[]),
        "'foo:bar:' asSymbol",
    );
    let back_to_symbol = interp.fetch(&back_to_symbol);
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

    let a = interp.new_character('A');
    let a = interp.stash(Value::Obj(a));

    let result = expect_ok(
        interp.send(&a, Symbol::intern("asInteger"), &[]),
        "$A asInteger",
    );

    assert!(matches!(interp.fetch(&result), Value::SmallInt(65)));
}
