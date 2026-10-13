//! Integration tests for the `String`/`Symbol` native
//! methods, found as primitives by the ordinary lookup via
//! `Interpreter::send`.

use indoc::{formatdoc, indoc};
use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value, ValueKind};

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
        mutator.stash(Value::object(object))
    })
}

fn as_string<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> String {
    match value.kind() {
        ValueKind::Object(object) => heap
            .str_value(object)
            .unwrap_or_else(|| panic!("expected a String, got {}", heap.describe(value))),
        _ => panic!("expected an object, got {}", heap.describe(value)),
    }
}

/// A module loaded and ready to instantiate, the way `heap.rs`'s own `load`
/// helper does it.
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

/// `class usingPlatform: platform`, then `instance selector`, answering the
/// send's result.
fn instantiate_and_send(interpreter: &mut Interpreter, class: &Root, selector: &str) -> Root {
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern(selector), &[]),
        selector,
    )
}

fn as_boolean<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> bool {
    match value.kind() {
        ValueKind::Boolean(boolean) => boolean,
        _ => panic!("expected a boolean, got {}", heap.describe(value)),
    }
}

/// §5.1.6: two symbols equal by `=` are the same object, whether one comes
/// from a literal and the other from `asSymbol`, or both from `asSymbol` --
/// pinned alongside `goldens/library/string/spec_5_1_6_equal_symbols_are_one_object`.
#[test]
fn equal_symbols_are_identical() {
    let source = indoc! {"
        class Symbols usingPlatform: p = Object new () (
            public literalAndDerived = ( ^#foo == 'foo' asSymbol )
            public twoDerived = ( ^'bar' asSymbol == 'bar' asSymbol )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);

    let literal_and_derived = instantiate_and_send(&mut interpreter, &class, "literalAndDerived");
    interpreter.enter(|mutator| {
        assert!(
            as_boolean(mutator.heap(), mutator.fetch(&literal_and_derived)),
            "#foo == 'foo' asSymbol must be true: a symbol literal and its \
             derived equivalent are one object"
        );
    });

    let two_derived = instantiate_and_send(&mut interpreter, &class, "twoDerived");
    interpreter.enter(|mutator| {
        assert!(
            as_boolean(mutator.heap(), mutator.fetch(&two_derived)),
            "'bar' asSymbol == 'bar' asSymbol must be true: two derivations \
             of the same text are one object"
        );
    });
}

/// §5.1.6, through `doesNotUnderstand:`: the selector that `handler_message`
/// (`interpreter/resolve.rs`) hands to the handler must be the same object
/// as a symbol literal spelling the same selector, since it too goes
/// through `Mutator::new_symbol`.
#[test]
fn does_not_understand_hands_the_literals_selector_object() {
    let source = indoc! {"
        class Selectors usingPlatform: p = Object new () (
            public doesNotUnderstand: message = ( ^message selector == #frobnicate )
            public probe = ( ^self frobnicate )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);

    let probe = instantiate_and_send(&mut interpreter, &class, "probe");
    interpreter.enter(|mutator| {
        assert!(
            as_boolean(mutator.heap(), mutator.fetch(&probe)),
            "the doesNotUnderstand: message's selector must be the same \
             object as the #frobnicate literal"
        );
    });
}

#[test]
fn string_concatenation_joins_two_strings() {
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");
    let bar = string(&mut interpreter, "bar");

    let result = interpreter.send(&foo, Symbol::intern(","), &[bar]);

    let joined = expect_ok(result, "'foo' , 'bar'");
    interpreter.enter(|mutator| {
        assert_eq!(as_string(mutator.heap(), mutator.fetch(&joined)), "foobar");
    });
}

#[test]
fn string_concatenation_with_a_non_string_argument_is_a_newspeak_error_not_a_panic() {
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");

    let result = interpreter.send(
        &foo,
        Symbol::intern(","),
        &[interpreter.stash(Value::small_integer(42))],
    );

    match result {
        Err(HostSignal::Error(_)) => {}
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&value)));
            panic!("expected a Newspeak error for 'foo' , 42, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

#[test]
fn concatenating_a_string_with_an_object_is_an_error_not_a_stack_overflow() {
    // This message must not `{:?}`-format the argument: an object
    // there would recurse through `Object`'s self-referential bootstrap
    // class until the stack gives out, aborting the process instead of
    // reporting. Every other test here passes a `SmallInteger`, which formats
    // harmlessly and so could not catch it.
    let mut interpreter = Interpreter::bootstrap();

    let foo = string(&mut interpreter, "foo");
    let argument = interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![]);
        mutator.stash(Value::object(object))
    });

    let result = interpreter.send(&foo, Symbol::intern(","), &[argument]);

    match result {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("requires a String argument"),
            "unexpected message: {}",
            error.message
        ),
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&value)));
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
    assert_eq!(
        interpreter.enter(|mutator| mutator.fetch(&equal).as_boolean()),
        Some(true)
    );

    let not_equal = expect_ok(
        interpreter.send(&same, Symbol::intern("="), &[different]),
        "'same' = 'different'",
    );
    assert_eq!(
        interpreter.enter(|mutator| mutator.fetch(&not_equal).as_boolean()),
        Some(false)
    );
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
        assert_eq!(as_string(mutator.heap(), mutator.fetch(&result)), "'it''s'");
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
            as_string(mutator.heap(), mutator.fetch(&as_str)),
            "foo:bar:"
        );
    });

    let back_to_symbol = expect_ok(
        interpreter.send(&as_str, Symbol::intern("asSymbol"), &[]),
        "'foo:bar:' asSymbol",
    );
    interpreter.enter(|mutator| match mutator.fetch(&back_to_symbol).kind() {
        ValueKind::Object(object) => match mutator.heap().symbol_value(object) {
            Some(symbol) => assert_eq!(symbol, Symbol::intern("foo:bar:")),
            None => panic!(
                "expected a Symbol, got {}",
                mutator.heap().describe(Value::object(object))
            ),
        },
        _ => panic!(
            "expected an object, got {}",
            mutator.heap().describe(mutator.fetch(&back_to_symbol))
        ),
    });
}

/// A String's code points are packed into words, eight bytes or two 32-bit
/// units each; every String message reads them across a word boundary and
/// in either width, and a Symbol of a wide text is still one object.
#[test]
fn strings_answer_their_code_points_across_words_in_either_width() {
    let source = indoc! {"
        class Packed usingPlatform: p = Object new () (
            public narrow = (
                | text = 'abcdefghi'. |
                ^(text size = 9)
                    & ((text at: 9) = 105)
                    & ((text , 'j') = 'abcdefghij')
                    & ((text copyFrom: 8 to: 9) = 'hi')
                    & (text hash = 'abcdefghi' hash)
            )
            public wide = (
                | text = '€uro 😀'. |
                ^(text size = 6)
                    & ((text at: 1) = 8364)
                    & ((text at: 6) = 128512)
                    & ((text , '!') = '€uro 😀!')
                    & ((text copyFrom: 2 to: 4) = 'uro')
                    & (text asSymbol == '€uro 😀' asSymbol)
                    & (text asSymbol asString = text)
            )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    for selector in ["narrow", "wide"] {
        let result = instantiate_and_send(&mut interpreter, &class, selector);
        interpreter.enter(|mutator| {
            assert!(
                as_boolean(mutator.heap(), mutator.fetch(&result)),
                "{selector}"
            );
        });
    }
}

/// The `SmallInteger` a send answered.
fn as_integer<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> i64 {
    match value.kind() {
        ValueKind::SmallInteger(number) => number,
        _ => panic!("expected a SmallInteger, got {}", heap.describe(value)),
    }
}

/// Each selector of a loaded module, sent to a new instance, with the
/// SmallInteger it must answer.
fn assert_answers(source: &str, expected: &[(&str, i64)]) {
    let (mut interpreter, class) = load(source);
    for (selector, answer) in expected {
        let result = instantiate_and_send(&mut interpreter, &class, selector);
        interpreter.enter(|mutator| {
            assert_eq!(
                as_integer(mutator.heap(), mutator.fetch(&result)),
                *answer,
                "{selector}"
            );
        });
    }
}

#[test]
fn index_of_finds_a_wide_needle_in_a_wide_string() {
    let source = indoc! {"
        class Search usingPlatform: p = Object new () (
            public forwards = ( ^'xé😀é😀' indexOf: 'é😀' startingAt: 3 )
            public backwards = ( ^'xé😀é😀' lastIndexOf: 'é😀' startingAt: 3 )
        ) : ()
    "};
    assert_answers(source, &[("forwards", 4), ("backwards", 2)]);
}

#[test]
fn an_empty_needle_is_found_at_the_start_itself() {
    let source = indoc! {"
        class Search usingPlatform: p = Object new () (
            public forwards = ( ^'abc' indexOf: '' startingAt: 4 )
            public backwards = ( ^'abc' lastIndexOf: '' startingAt: 1 )
        ) : ()
    "};
    assert_answers(source, &[("forwards", 4), ("backwards", 1)]);
}

#[test]
fn a_needle_longer_than_the_string_is_not_found() {
    let source = indoc! {"
        class Search usingPlatform: p = Object new () (
            public forwards = ( ^'ab' indexOf: 'abc' )
            public backwards = ( ^'ab' lastIndexOf: 'abc' )
        ) : ()
    "};
    assert_answers(source, &[("forwards", 0), ("backwards", 0)]);
}

/// `Heap::strings_equal` compares the stored format before the units, so
/// a String equal to one built from a Rust literal -- narrow exactly when
/// every code point fits Latin-1 -- is stored in the same width.
#[test]
fn with_all_of_a_latin_one_array_makes_a_narrow_string() {
    let source = indoc! {"
        class Build usingPlatform: p = Object new () (
            public latinOne = ( ^String withAll: {104. 233} )
            public beyondLatinOne = ( ^String withAll: {104. 128512} )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    for (selector, text) in [("latinOne", "hé"), ("beyondLatinOne", "h😀")] {
        let result = instantiate_and_send(&mut interpreter, &class, selector);
        let expected = string(&mut interpreter, text);
        interpreter.enter(|mutator| {
            let built = mutator.fetch(&result).as_object();
            let literal = mutator.fetch(&expected).as_object();
            let (Some(built), Some(literal)) = (built, literal) else {
                panic!("{selector}: expected two Strings");
            };
            assert_eq!(
                mutator.heap().strings_equal(built, literal),
                Some(true),
                "{selector}"
            );
        });
    }
}

/// The byte sequences `String fromUtf8:` must refuse, one of each kind
/// the design names, and some it must take: `from_utf8_strict` is
/// exactly as strict as Rust's `str::from_utf8`.
const UTF8_CASES: &[&[u8]] = &[
    // Truncated at the end, after a valid prefix.
    &[104, 226, 130],
    &[240, 159, 152],
    // A stray continuation byte.
    &[97, 128, 98],
    &[191],
    // Overlong encodings.
    &[192, 128],
    &[224, 128, 175],
    &[240, 130, 130, 172],
    // A surrogate, high and low.
    &[237, 160, 128],
    &[237, 191, 191],
    // Past U+10FFFF, and bytes no sequence begins with.
    &[244, 144, 128, 128],
    &[245, 128, 128, 128],
    &[97, 98, 255],
    // A lead byte followed by one that does not continue it.
    &[195, 40],
    // Valid: ASCII, Latin-1, wide, a byte-order mark, the empty sequence.
    &[104, 105],
    &[195, 169],
    &[226, 130, 172, 240, 159, 152, 128],
    &[239, 187, 191],
    &[],
];

#[test]
fn from_utf8_strict_refuses_what_rust_refuses_at_the_same_byte() {
    for bytes in UTF8_CASES {
        let expected = std::str::from_utf8(bytes)
            .map(ns_runtime::CodePoints::from)
            .map_err(|error| error.valid_up_to());
        assert_eq!(
            ns_runtime::CodePoints::from_utf8_strict(bytes),
            expected,
            "{bytes:?}"
        );
    }
    assert_eq!(
        ns_runtime::CodePoints::from_utf8_strict(&[239, 187, 191])
            .map(|code_points| code_points.code_point_at(0)),
        Ok(Some(0xFEFF)),
        "a byte-order mark is kept"
    );
}

/// `String fromUtf8:from:to:` names the first byte of the bad sequence by
/// its 1-based index in the ByteArray, not in the range: the offset Rust's
/// `Utf8Error::valid_up_to` gives within the range, plus the range's
/// start.
#[test]
fn an_invalid_sequence_is_named_by_its_index_in_the_byte_array() {
    for bytes in UTF8_CASES {
        let Err(error) = std::str::from_utf8(bytes) else {
            continue;
        };
        // The same bytes behind a two-byte prefix the range leaves out.
        let elements: Vec<String> = [120, 121]
            .iter()
            .chain(bytes.iter())
            .map(u8::to_string)
            .collect();
        let source = formatdoc! {"
            class Decode usingPlatform: p = Object new () (
                public decode = (
                    ^String fromUtf8: (ByteArray withAll: {{{elements}}}) from: 3 to: {stop}
                )
            ) : ()
            ",
            elements = elements.join(". "),
            stop = bytes.len() + 2,
        };
        let (mut interpreter, class) = load(&source);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        match interpreter.send(&instance, Symbol::intern("decode"), &[]) {
            Err(HostSignal::Error(refusal)) => assert_eq!(
                refusal.message,
                format!(
                    "fromUtf8: invalid UTF-8 at byte {}",
                    error.valid_up_to() + 3
                ),
                "{bytes:?}"
            ),
            Ok(_) => panic!("{bytes:?}: decoded"),
            Err(HostSignal::Return) => panic!("{bytes:?}: a non-local return escaped"),
        }
    }
}
