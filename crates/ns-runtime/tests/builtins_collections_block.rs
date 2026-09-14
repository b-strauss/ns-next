//! Integration tests for the `Array`/`OrderedCollection` native
//! methods (`crate::builtins::collections`) and `Block` native methods
//! (`crate::builtins::block`), wired through `crate::builtins::dispatch_native`
//! via `Interpreter::send` -- same pattern as `tests/builtins_boolean_numbers.rs`.

use indoc::indoc;
use ns_runtime::{
    BuiltinClass, HostError, HostResult, HostSignal, Interpreter, ObjectData, Root, Symbol, Value,
};

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
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&root)));
            panic!("{what}: expected a Newspeak error, got {got}")
        }
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

/// Builds a bare `OrderedCollection` instance directly: an
/// `ObjectData::Array` payload tagged with the `OrderedCollection` class,
/// the shape `try_factory_send` gives `OrderedCollection new`. Constructing
/// it without a send is sufficient to exercise `dispatch_ordered_collection`.
fn new_ordered_collection(interpreter: &mut Interpreter) -> Root {
    interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::OrderedCollection);
        let collection = mutator
            .heap_mut()
            .allocate_object(class, ObjectData::Array(vec![]));
        mutator.stash(Value::Object(collection))
    })
}

#[test]
fn array_at_returns_the_one_based_element() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( ^{1. 2. 3} at: 2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "{1. 2. 3} at: 2");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(2) => {}
        other => panic!(
            "expected Value::SmallInteger(2), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn array_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
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
fn array_at_put_writes_and_returns_the_receiver() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( | a | a := {1. 2. 3}. a at: 2 put: 99. ^a at: 2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "at:put: then at:");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(99) => {}
        other => panic!(
            "expected Value::SmallInteger(99), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// `do:` traversal via a mutable-slot accumulator. Also exercises `do:`
/// activating a real one-argument closure with a real argument, once per
/// element.
#[test]
fn array_do_sums_elements_via_a_mutable_slot_accumulator() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( | sum | sum := 0. {1. 2. 3} do: [:x | sum := sum + x]. ^sum )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "{1. 2. 3} do: [:x | sum := sum + x]");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(6) => {}
        other => panic!(
            "expected Value::SmallInteger(6), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// `OrderedCollection new add: 1; add: 2; size` -> `2` -- exercises cascades
/// together with `add:`/`size`. The receiver is built via
/// `new_ordered_collection` (see its doc comment for why) and passed in as a
/// method argument, so the cascade's receiver expression (`aColl`) is a real
/// `Expression::Cascade`, compiled and run, not something faked in Rust.
#[test]
fn ordered_collection_add_and_size_through_a_real_cascade() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m: aColl = ( ^aColl add: 1; add: 2; size )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let collection = new_ordered_collection(&mut interpreter);
    let result = interpreter.send(&receiver, Symbol::intern("m:"), &[collection]);
    let result = expect_ok(result, "OrderedCollection new add: 1; add: 2; size");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(2) => {}
        other => panic!(
            "expected Value::SmallInteger(2), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn ordered_collection_add_returns_the_argument_not_the_receiver() {
    let mut interpreter = Interpreter::bootstrap();
    let collection = new_ordered_collection(&mut interpreter);
    let result = interpreter.send(
        &collection,
        Symbol::intern("add:"),
        &[interpreter.stash(Value::SmallInteger(42))],
    );
    let result = expect_ok(result, "OrderedCollection add: 42");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(42) => {}
        other => panic!(
            "expected Value::SmallInteger(42) (the argument), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn ordered_collection_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let mut interpreter = Interpreter::bootstrap();
    let collection = new_ordered_collection(&mut interpreter);
    let result = interpreter.send(
        &collection,
        Symbol::intern("at:"),
        &[interpreter.stash(Value::SmallInteger(1))],
    );
    let error = expect_error(&mut interpreter, result, "OrderedCollection (empty) at: 1");
    assert!(
        error.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        error.message
    );
}

#[test]
fn array_at_with_an_object_index_is_an_error_not_a_stack_overflow() {
    // `not_an_integer`'s message must not `{:?}`-format the argument: a
    // `Value::Object` there would recurse through `Object`'s self-referential
    // bootstrap class until the stack gives out, aborting the process
    // instead of reporting. Every other `at:` test here passes a `SmallInteger`
    // index, which formats harmlessly and so could not catch it.
    let mut interpreter = Interpreter::bootstrap();
    let receiver = interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![Value::SmallInteger(1)]);
        mutator.stash(Value::Object(object))
    });
    let index = interpreter.enter(|mutator| {
        let object = mutator.new_string("not an integer".to_string());
        mutator.stash(Value::Object(object))
    });

    let result = interpreter.send(&receiver, Symbol::intern("at:"), &[index]);

    let error = expect_error(&mut interpreter, result, "{1} at: 'not an integer'");
    assert!(
        error.message.contains("requires an Integer index"),
        "unexpected message: {}",
        error.message
    );
}

/// `do:` walks the elements its receiver had when it began: a block that adds
/// to the collection it is walking neither sees what it adds nor runs
/// forever. Two elements seen, four in the collection afterwards.
#[test]
fn do_walks_a_snapshot_of_its_receiver() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m: aColl = (
            | seen |
            seen := 0.
            aColl add: 1; add: 2.
            aColl do: [:x | seen := seen + 1. aColl add: x].
            ^seen * 100 + aColl size
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let collection = new_ordered_collection(&mut interpreter);
    let result = interpreter.send(&receiver, Symbol::intern("m:"), &[collection]);
    let result = expect_ok(result, "do: over a collection it grows");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(204) => {}
        other => panic!("expected 204, got {}", mutator.heap().describe(&other)),
    });
}

/// A non-block argument stands for its own value, so `do:` hands it every
/// element and observes nothing; like any `do:`, it answers its receiver.
#[test]
fn do_with_a_non_block_answers_its_receiver() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( | a | a := {1. 2}. ^(a do: 3) == a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "{1. 2} do: 3");
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&result), Value::Boolean(true))));
}

/// A block of the wrong arity fails at the first element; an empty
/// collection hands it nothing, so there it does not fail at all.
#[test]
fn do_checks_its_block_arity_at_each_element() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public empty = ( ^#() do: [:a :b | a] )
          public full = ( ^#(1) do: [:a :b | a] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&receiver, Symbol::intern("empty"), &[]),
        "#() do: with a two-argument block",
    );
    let result = interpreter.send(&receiver, Symbol::intern("full"), &[]);
    let error = expect_error(
        &mut interpreter,
        result,
        "#(1) do: with a two-argument block",
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
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( ^[42] value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[42] value");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(42) => {}
        other => panic!(
            "expected Value::SmallInteger(42), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn block_value_colon_passes_the_argument_positionally() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( ^[:x | x + 1] value: 41 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[:x | x + 1] value: 41");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(42) => {}
        other => panic!(
            "expected Value::SmallInteger(42), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

#[test]
fn block_argument_count_reports_the_declared_parameter_count() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( ^[:x :y | x + y] numArgs )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern("m"), &[]);
    let result = expect_ok(result, "[:x :y | x + y] numArgs");
    interpreter.enter(|mutator| match mutator.fetch(&result) {
        Value::SmallInteger(2) => {}
        other => panic!(
            "expected Value::SmallInteger(2), got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// A block called with too few arguments must be a `NewspeakError`, never a
/// Rust panic (the argument-count guard `Mutator::push_step` runs on
/// every block activation).
#[test]
fn block_called_with_too_few_arguments_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
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
        Newspeak3
        'T'
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
