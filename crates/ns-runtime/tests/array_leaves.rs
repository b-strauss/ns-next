//! `Array`'s range leaves, `copyFrom:to:` and
//! `replaceFrom:to:with:startingAt:` (`crate::builtins::collections`),
//! through `Interpreter::send` -- same pattern as
//! `tests/builtins_collections_block.rs`.

use indoc::indoc;
use ns_runtime::{BuiltinClass, HostResult, HostSignal, Interpreter, Root, Symbol, ValueKind};

/// Parses+resolves a one-class module and loads it into a freshly
/// bootstrapped `Interpreter`, as `tests/builtins_collections_block.rs` does.
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

/// Sends the unary `selector` to a new instance of `source`'s class and
/// answers the String it answers.
fn answer_text(source: &str, selector: &str) -> String {
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern(selector), &[]);
    let result = expect_ok(result, selector);
    text_of(&mut interpreter, &result)
}

fn text_of(interpreter: &mut Interpreter, root: &Root) -> String {
    interpreter.enter(|mutator| {
        let value = mutator.fetch(root);
        match value.kind() {
            ValueKind::Object(object) => mutator.heap().str_value(object).unwrap_or_else(|| {
                panic!("expected a String, got {}", mutator.heap().describe(value))
            }),
            _ => panic!("expected a String, got {}", mutator.heap().describe(value)),
        }
    })
}

#[test]
fn copy_from_to_answers_a_new_array_of_the_range() {
    let source = indoc! {"
        class Foo = Object new () (
            public m = (
                | array copy empty |
                array:: {1. 2. 3}.
                copy:: array copyFrom: 2 to: 3.
                empty:: array copyFrom: 4 to: 3.
                ^(copy == array) printString, ' ', copy size printString, ' ',
                    (copy at: 1) printString, ' ', (copy at: 2) printString, ' ',
                    empty size printString
            )
            public empty = ( ^{1. 2. 3} copyFrom: 4 to: 3 )
        ) : ()
    "};
    assert_eq!(answer_text(source, "m"), "false 2 2 3 0");
    // An empty range is still an Array, read on the host side since a
    // program cannot ask another object for its class.
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let empty = expect_ok(
        interpreter.send(&receiver, Symbol::intern("empty"), &[]),
        "empty",
    );
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&empty);
        let array = mutator.builtin_class(BuiltinClass::Array);
        assert!(
            mutator.class_of(value).same(array),
            "`copyFrom: 4 to: 3` answers an Array"
        );
    });
}

#[test]
fn replace_from_an_overlapping_range_of_the_same_array_reads_before_it_stores() {
    let source = indoc! {"
        class Foo = Object new () (
            public m = (
                | right left |
                right:: {1. 2. 3. 4. 5}.
                right replaceFrom: 2 to: 5 with: right startingAt: 1.
                left:: {1. 2. 3. 4. 5}.
                left replaceFrom: 1 to: 4 with: left startingAt: 2.
                ^(right inject: '' into: [:text :each | text, each printString]), ' ',
                    (left inject: '' into: [:text :each | text, each printString])
            )
        ) : ()
    "};
    assert_eq!(answer_text(source, "m"), "11234 23455");
}

/// An old Array, given a young element by the leaf, must keep it through
/// the next scavenge: the store goes through the write barrier, so the old
/// array is remembered and the scavenge finds the young object through it.
#[cfg(debug_assertions)]
#[test]
fn replace_stores_a_young_element_into_an_old_array_through_the_barrier() {
    let source = indoc! {"
        class Foo = Object new (
            | old |
        ) (
            public make = ( old:: Array new: 1 )
            public store = ( old replaceFrom: 1 to: 1 with: {Object new} startingAt: 1 )
            public read = ( ^(old at: 1) printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&receiver, Symbol::intern("make"), &[]),
        "make",
    );
    // A full collection between entries compacts both generations into the
    // old space: the array is old from here on.
    interpreter.collect_now();
    expect_ok(
        interpreter.send(&receiver, Symbol::intern("store"), &[]),
        "store",
    );
    interpreter.scavenge_now();
    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("read"), &[]),
        "read",
    );
    assert_eq!(text_of(&mut interpreter, &result), "an Object");
}

#[test]
fn array_new_refuses_a_size_it_cannot_allocate_as_an_error() {
    // `1 << 60` is past every SmallInteger, `1 << 59` is one, so the two
    // reach the refusal by the large-integer path and by the size probe.
    let source = indoc! {"
        class Foo = Object new () (
            public large = ( ^[Array new: 1 << 60] on: Error do: [:error | error messageText] )
            public small = ( ^[Array new: 1 << 59] on: Error do: [:error | error messageText] )
        ) : ()
    "};
    for selector in ["large", "small"] {
        let text = answer_text(source, selector);
        assert!(
            text.contains("cannot allocate an Array"),
            "{selector}: {text}"
        );
    }
}

#[test]
fn array_new_answers_an_array_of_nils_past_the_fast_path() {
    let source = indoc! {"
        class Foo = Object new () (
            public m = (
                | array |
                array:: Array new: 300.
                ^array size printString, ' ',
                    (array allSatisfy: [:element | element isNil]) printString
            )
        ) : ()
    "};
    assert_eq!(answer_text(source, "m"), "300 true");
}
