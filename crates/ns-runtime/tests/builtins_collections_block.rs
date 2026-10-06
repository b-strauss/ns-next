//! Integration tests for `Array`'s native methods
//! (`crate::builtins::collections`) and `Closure` native methods
//! (`crate::builtins::block`), found as primitives by the ordinary lookup
//! via `Interpreter::send` -- same pattern as `tests/builtins_boolean_numbers.rs`.

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value, ValueKind};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both --
/// same pattern as `tests/instantiate.rs` and `tests/builtins_boolean_numbers.rs`.
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

fn expect_error(interpreter: &mut Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Ok(root) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&root)));
            panic!("{what}: expected a Newspeak error, got {got}")
        }
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

#[test]
fn array_at_returns_the_one_based_element() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^{1. 2. 3} at: 2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "{1. 2. 3} at: 2");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(2) => {}
        _ => panic!(
            "expected the small integer 2, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

#[test]
fn array_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^{1. 2. 3} at: 5 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let error = expect_error(&mut interpreter, result, "{1. 2. 3} at: 5");
    assert!(
        error.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        error.message
    );
}

#[test]
fn array_at_put_writes_and_answers_the_value() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( | a | a:: {1. 2. 3}. ^(a at: 2 put: 99) = 99 ifTrue: [a at: 2] ifFalse: [0] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "at:put: then at:");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(99) => {}
        _ => panic!(
            "expected the small integer 99, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

/// `do:` traversal via a mutable-slot accumulator. Also exercises `do:`
/// activating a real one-argument closure with a real argument, once per
/// element.
#[test]
fn array_do_sums_elements_via_a_mutable_slot_accumulator() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( | sum | sum:: 0. {1. 2. 3} do: [:x | sum:: sum + x]. ^sum )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "{1. 2. 3} do: [:x | sum:: sum + x]");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(6) => {}
        _ => panic!(
            "expected the small integer 6, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

/// `do:` reads each element when the walk reaches it, not a copy taken when
/// it started: a write inside the block through `at:put:` to a later index
/// is seen by the step that reaches that index. Pins
/// `builtins/collections.rs`'s `element_step`, which reads
/// `Heap::array_element` for each element in turn.
/// `goldens/library/collection/do_visits_the_elements_a_list_held_when_it_started`
/// pins `List>>do:`, a Newspeak loop that never reaches this leaf, so it
/// cannot stand in for this test.
#[test]
fn do_reads_each_element_when_it_reaches_it() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = (
            | a sum |
            a:: {1. 2. 3}.
            sum:: 0.
            a do: [:x | a at: 3 put: 99. sum:: sum + x].
            ^sum * 1000 + (a at: 3)
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(
        result,
        "do: over an array the block mutates through at:put:",
    );
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        // 102099 = (1 + 2 + 99) * 1000 + 99: the first step wrote 99 to the
        // third index before the walk reached it, so the third step read 99.
        ValueKind::SmallInteger(102099) => {}
        _ => panic!(
            "expected 102099, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

#[test]
fn array_at_with_an_object_index_is_an_error_not_a_stack_overflow() {
    // `not_an_integer`'s message must not `{:?}`-format the argument: a
    // an object there would recurse through `Object`'s self-referential
    // bootstrap class until the stack gives out, aborting the process
    // instead of reporting. Every other `at:` test here passes a `SmallInteger`
    // index, which formats harmlessly and so could not catch it.
    let mut interpreter = Interpreter::bootstrap();
    let receiver = interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![Value::small_integer(1)]);
        mutator.stash(Value::object(object))
    });
    let index = interpreter.enter(|mutator| {
        let object = mutator.new_string("not an integer".to_string());
        mutator.stash(Value::object(object))
    });

    let result = interpreter.send(&receiver, Symbol::intern("at:"), &[index]);

    let error = expect_error(&mut interpreter, result, "{1} at: 'not an integer'");
    assert!(
        error.message.contains("requires an Integer index"),
        "unexpected message: {}",
        error.message
    );
}

/// A non-block argument is sent `value:` for each element, as a block would
/// be evaluated; an Integer understands no `value:`, so the first element
/// ends in `doesNotUnderstand:`.
#[test]
fn do_with_a_non_block_sends_it_value() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( | a | a:: {1. 2}. ^(a do: 3) == a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let error = expect_error(&mut interpreter, result, "{1. 2} do: 3");
    assert_eq!(error.message, "an object does not understand #value:");
}

/// A block of the wrong arity fails at the first element; an empty
/// collection hands it nothing, so there it does not fail at all.
#[test]
fn do_checks_its_block_arity_at_each_element() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public empty = ( ^{} do: [:a :b | a] )
          public full = ( ^{1} do: [:a :b | a] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&receiver, Symbol::intern("empty"), &[]),
        "{} do: with a two-argument block",
    );
    let result = interpreter.send(&receiver, Symbol::intern("full"), &[]);
    let error = expect_error(
        &mut interpreter,
        result,
        "{1} do: with a two-argument block",
    );
    assert!(
        error.message.contains("wrong number of block arguments"),
        "got: {}",
        error.message
    );
}

#[test]
fn block_value_invokes_a_zero_argument_closure() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^[42] value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[42] value");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(42) => {}
        _ => panic!(
            "expected the small integer 42, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

#[test]
fn block_value_colon_passes_the_argument_positionally() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^[:x | x + 1] value: 41 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[:x | x + 1] value: 41");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(42) => {}
        _ => panic!(
            "expected the small integer 42, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

#[test]
fn block_argument_count_reports_the_declared_parameter_count() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^[:x :y | x + y] numArgs )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[:x :y | x + y] numArgs");
    interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
        ValueKind::SmallInteger(2) => {}
        _ => panic!(
            "expected the small integer 2, got {}",
            mutator.heap().describe(mutator.fetch(&result))
        ),
    });
}

/// A block called with too few arguments must be a `NewspeakError`, never a
/// Rust panic (the argument-count guard `Mutator::push_step` runs on
/// every block activation).
#[test]
fn block_called_with_too_few_arguments_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^[:x | x] value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let error = expect_error(&mut interpreter, result, "[:x | x] value (0 args, 1 param)");
    assert!(
        error.message.to_lowercase().contains("argument"),
        "expected an argument-count-shaped error message, got: {}",
        error.message
    );
}

/// Same guard, the too-many-arguments direction.
#[test]
fn block_called_with_too_many_arguments_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        class Foo = Object new (
        ) (
          public m = ( ^[42] value: 1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let error = expect_error(&mut interpreter, result, "[42] value: 1 (1 arg, 0 params)");
    assert!(
        error.message.to_lowercase().contains("argument"),
        "expected an argument-count-shaped error message, got: {}",
        error.message
    );
}
