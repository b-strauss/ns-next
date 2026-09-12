//! Integration tests for the `Array`/`OrderedCollection` native
//! methods (`crate::builtins::collections`) and `Block` native methods
//! (`crate::builtins::block`), wired through `crate::builtins::dispatch_native`
//! via `Interpreter::send` -- same pattern as `tests/builtins_boolean_numbers.rs`.

use ns_runtime::{BuiltinClass, Interpreter, ObjectData, Signal, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both --
/// same pattern as `tests/instantiate.rs` and `tests/builtins_boolean_numbers.rs`.
fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_error(
    heap: &ns_runtime::Heap,
    result: Result<Value, Signal>,
    what: &str,
) -> ns_runtime::NewspeakError {
    match result {
        Err(Signal::Error(e)) => e,
        Ok(v) => panic!(
            "{what}: expected a NewspeakError, got Ok({})",
            heap.describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
    }
}

/// Builds a bare `OrderedCollection` instance directly: an
/// `ObjectData::Array` payload tagged with the `OrderedCollection` class,
/// the shape `try_factory_send` gives `OrderedCollection new`. Constructing
/// it without a send is sufficient to exercise `dispatch_ordered_collection`.
fn new_ordered_collection(interp: &mut Interpreter) -> Value {
    let class = interp.builtin_class(BuiltinClass::OrderedCollection);
    Value::Obj(
        interp
            .heap_mut()
            .alloc_object(class, ObjectData::Array(vec![])),
    )
}

#[test]
fn array_at_returns_the_one_based_element() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^{1. 2. 3} at: 2 )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "{1. 2. 3} at: 2") {
        Value::SmallInt(2) => {}
        other => panic!(
            "expected Value::SmallInt(2), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

#[test]
fn array_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^{1. 2. 3} at: 5 )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    let err = expect_error(interp.heap_ref(), result, "{1. 2. 3} at: 5");
    assert!(
        err.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        err.message
    );
}

#[test]
fn array_at_put_writes_and_returns_the_receiver() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( | a | a := {1. 2. 3}. a at: 2 put: 99. ^a at: 2 )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "at:put: then at:") {
        Value::SmallInt(99) => {}
        other => panic!(
            "expected Value::SmallInt(99), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// `do:` traversal via a mutable-slot accumulator. Also exercises
/// `call_block` invoking a real one-argument closure with a real argument.
#[test]
fn array_do_sums_elements_via_a_mutable_slot_accumulator() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( | sum | sum := 0. {1. 2. 3} do: [:x | sum := sum + x]. ^sum )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "{1. 2. 3} do: [:x | sum := sum + x]") {
        Value::SmallInt(6) => {}
        other => panic!(
            "expected Value::SmallInt(6), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// `OrderedCollection new add: 1; add: 2; size` -> `2` -- exercises cascades
/// together with `add:`/`size`. The receiver is built via
/// `new_ordered_collection` (see its doc comment for why) and passed in as a
/// method argument, so the cascade's receiver expression (`aColl`) is a real
/// `Expr::Cascade`, compiled and run, not something faked in Rust.
#[test]
fn ordered_collection_add_and_size_through_a_real_cascade() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m: aColl = ( ^aColl add: 1; add: 2; size )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let oc = new_ordered_collection(&mut interp);
    let result = interp.send(receiver, Symbol::intern("m:"), vec![oc]);
    match expect_ok(result, "OrderedCollection new add: 1; add: 2; size") {
        Value::SmallInt(2) => {}
        other => panic!(
            "expected Value::SmallInt(2), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

#[test]
fn ordered_collection_add_returns_the_argument_not_the_receiver() {
    let mut interp = Interpreter::bootstrap();
    let oc = new_ordered_collection(&mut interp);
    let result = interp.send(oc, Symbol::intern("add:"), vec![Value::SmallInt(42)]);
    match expect_ok(result, "OrderedCollection add: 42") {
        Value::SmallInt(42) => {}
        other => panic!(
            "expected Value::SmallInt(42) (the argument), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

#[test]
fn ordered_collection_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::bootstrap();
    let oc = new_ordered_collection(&mut interp);
    let result = interp.send(oc, Symbol::intern("at:"), vec![Value::SmallInt(1)]);
    let err = expect_error(interp.heap_ref(), result, "OrderedCollection (empty) at: 1");
    assert!(
        err.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        err.message
    );
}

#[test]
fn array_at_with_an_object_index_is_an_error_not_a_stack_overflow() {
    // `not_an_integer`'s message must not `{:?}`-format the argument: a
    // `Value::Obj` there would recurse through `Object`'s self-referential
    // bootstrap class until the stack gives out, aborting the process
    // instead of reporting. Every other `at:` test here passes a `SmallInt`
    // index, which formats harmlessly and so could not catch it.
    let mut interp = Interpreter::bootstrap();
    let receiver = Value::Obj(interp.new_array(vec![Value::SmallInt(1)]));
    let index = Value::Obj(interp.new_string("not an integer".to_string()));

    let result = interp.send(receiver, Symbol::intern("at:"), vec![index]);

    let err = expect_error(interp.heap_ref(), result, "{1} at: 'not an integer'");
    assert!(
        err.message.contains("requires an Integer index"),
        "unexpected message: {}",
        err.message
    );
}

#[test]
fn block_value_invokes_a_zero_arg_closure() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[42] value )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "[42] value") {
        Value::SmallInt(42) => {}
        other => panic!(
            "expected Value::SmallInt(42), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

#[test]
fn block_value_colon_passes_the_argument_positionally() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x | x + 1] value: 41 )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "[:x | x + 1] value: 41") {
        Value::SmallInt(42) => {}
        other => panic!(
            "expected Value::SmallInt(42), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

#[test]
fn block_num_args_reports_the_declared_parameter_count() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x :y | x + y] numArgs )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    match expect_ok(result, "[:x :y | x + y] numArgs") {
        Value::SmallInt(2) => {}
        other => panic!(
            "expected Value::SmallInt(2), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// A block called with too few arguments must be a `NewspeakError`, never a
/// Rust panic (`invoke_closure`'s argument-count guard).
#[test]
fn block_called_with_too_few_arguments_is_a_newspeak_error_not_a_panic() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x | x] value )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    let err = expect_error(
        interp.heap_ref(),
        result,
        "[:x | x] value (0 args, 1 param)",
    );
    assert!(
        err.message.to_lowercase().contains("argument"),
        "expected an argument-count-shaped error message, got: {}",
        err.message
    );
}

/// Same guard, the too-many-arguments direction.
#[test]
fn block_called_with_too_many_arguments_is_a_newspeak_error_not_a_panic() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[42] value: 1 )\n) : ()";
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    let err = expect_error(interp.heap_ref(), result, "[42] value: 1 (1 arg, 0 params)");
    assert!(
        err.message.to_lowercase().contains("argument"),
        "expected an argument-count-shaped error message, got: {}",
        err.message
    );
}
