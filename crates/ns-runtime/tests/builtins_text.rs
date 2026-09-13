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
    interp.enter(|m| {
        let obj = m.new_string(s.to_string());
        m.stash(Value::Obj(obj))
    })
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
    interp.enter(|m| {
        assert_eq!(as_string(m.heap(), &m.fetch(&joined)), "foobar");
    });
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
        Ok(v) => {
            let got = interp.enter(|m| m.heap().describe(&m.fetch(&v)));
            panic!("expected a Newspeak error for 'foo' , 42, got Ok({got})")
        }
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
    let arg = interp.enter(|m| {
        let obj = m.new_array(vec![]);
        m.stash(Value::Obj(obj))
    });

    let result = interp.send(&a, Symbol::intern(","), &[arg]);

    match result {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("requires a String argument"),
            "unexpected message: {}",
            e.message
        ),
        Ok(v) => {
            let got = interp.enter(|m| m.heap().describe(&m.fetch(&v)));
            panic!("expected a Newspeak error for 'foo' , anArray, got Ok({got})")
        }
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
    assert!(interp.enter(|m| matches!(m.fetch(&equal), Value::Bool(true))));

    let not_equal = expect_ok(
        interp.send(&a, Symbol::intern("="), &[c]),
        "'same' = 'different'",
    );
    assert!(interp.enter(|m| matches!(m.fetch(&not_equal), Value::Bool(false))));
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
    interp.enter(|m| {
        assert_eq!(as_string(m.heap(), &m.fetch(&result)), "'it''s'");
    });
}

#[test]
fn symbol_round_trips_through_as_string_and_as_symbol() {
    let mut interp = Interpreter::bootstrap();

    let sym = interp.enter(|m| {
        let sym = m.new_symbol(Symbol::intern("foo:bar:"));
        m.stash(sym)
    });

    let as_str = expect_ok(
        interp.send(&sym, Symbol::intern("asString"), &[]),
        "#foo:bar: asString",
    );
    interp.enter(|m| {
        assert_eq!(as_string(m.heap(), &m.fetch(&as_str)), "foo:bar:");
    });

    let back_to_symbol = expect_ok(
        interp.send(&as_str, Symbol::intern("asSymbol"), &[]),
        "'foo:bar:' asSymbol",
    );
    interp.enter(|m| match m.fetch(&back_to_symbol) {
        Value::Obj(obj) => match m.heap().sym_value(obj) {
            Some(s) => assert_eq!(s, Symbol::intern("foo:bar:")),
            None => panic!(
                "expected ObjectData::Sym, got {}",
                m.heap().describe(&Value::Obj(obj))
            ),
        },
        other => panic!("expected Value::Obj, got {}", m.heap().describe(&other)),
    });
}

#[test]
fn character_constant_as_integer_yields_its_unicode_code_point() {
    let mut interp = Interpreter::bootstrap();

    let a = interp.enter(|m| {
        let obj = m.new_character('A');
        m.stash(Value::Obj(obj))
    });

    let result = expect_ok(
        interp.send(&a, Symbol::intern("asInteger"), &[]),
        "$A asInteger",
    );

    assert!(interp.enter(|m| matches!(m.fetch(&result), Value::SmallInt(65))));
}
