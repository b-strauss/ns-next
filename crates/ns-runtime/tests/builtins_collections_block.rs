//! Integration tests for Task 10's `Array`/`OrderedCollection` native
//! methods (`crate::builtins::collections`) and `Block` native methods
//! (`crate::builtins::block`), wired through `crate::builtins::dispatch_native`
//! via `Interpreter::send` -- same pattern as `tests/builtins_boolean_numbers.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use ns_runtime::{
    BuiltinClass, Interpreter, Object, ObjectData, ResolvedModule, Signal, Symbol, Value,
};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both --
/// same pattern as `tests/instantiate.rs` and `tests/builtins_boolean_numbers.rs`.
fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef, ResolvedModule) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, resolved)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_error(result: Result<Value, Signal>, what: &str) -> ns_runtime::NewspeakError {
    match result {
        Err(Signal::Error(e)) => e,
        Ok(v) => panic!("{what}: expected a NewspeakError, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
    }
}

/// Builds a bare `OrderedCollection` instance directly -- Milestone 1 has no
/// wiring yet from an ordinary `OrderedCollection new` message send through
/// to a real `ObjectData::Array`-backed instance (that requires both a
/// global-name binding for `OrderedCollection` from Newspeak source, and a
/// builtin-aware `new` that special-cases the collection classes instead of
/// `Interpreter::instantiate`'s generic `ObjectData::Instance` -- both are a
/// later task's job, not this task's Step 1 selector table). Constructing
/// the object directly, the same way `Interpreter::new_array` does
/// internally, is sufficient to exercise `dispatch_ordered_collection`.
fn new_ordered_collection(interp: &mut Interpreter) -> Value {
    let class = interp.builtin_class(BuiltinClass::OrderedCollection);
    let identity_hash = interp.alloc_identity_hash();
    Value::Obj(Rc::new(RefCell::new(Object {
        class,
        identity_hash,
        data: ObjectData::Array(vec![]),
    })))
}

#[test]
fn array_at_returns_the_one_based_element() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^{1. 2. 3} at: 2 )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "{1. 2. 3} at: 2") {
        Value::SmallInt(2) => {}
        other => panic!("expected Value::SmallInt(2), got {other:?}"),
    }
}

#[test]
fn array_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^{1. 2. 3} at: 5 )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    let err = expect_error(result, "{1. 2. 3} at: 5");
    assert!(
        err.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        err.message
    );
}

#[test]
fn array_at_put_writes_and_returns_the_receiver() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( | a | a := {1. 2. 3}. a at: 2 put: 99. ^a at: 2 )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "at:put: then at:") {
        Value::SmallInt(99) => {}
        other => panic!("expected Value::SmallInt(99), got {other:?}"),
    }
}

/// `do:` traversal via a mutable-slot accumulator, per the brief's Step 3.
/// Also exercises `call_block` invoking a real one-argument closure with a
/// real argument -- the generalization Task 10's `call_block` refactor
/// added over Task 8's always-zero-arg `invoke_block`.
#[test]
fn array_do_sums_elements_via_a_mutable_slot_accumulator() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( | sum | sum := 0. {1. 2. 3} do: [:x | sum := sum + x]. ^sum )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "{1. 2. 3} do: [:x | sum := sum + x]") {
        Value::SmallInt(6) => {}
        other => panic!("expected Value::SmallInt(6), got {other:?}"),
    }
}

/// `OrderedCollection new add: 1; add: 2; size` -> `2`, per the brief's
/// Step 3 -- exercises cascades (Task 6) together with `add:`/`size`. The
/// receiver is built via `new_ordered_collection` (see its doc comment for
/// why) and passed in as a method argument, so the cascade's receiver
/// expression (`aColl`) is a real `Expr::Cascade` evaluated by
/// `eval.rs::eval_cascade`, not something faked in Rust.
#[test]
fn ordered_collection_add_and_size_through_a_real_cascade() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m: aColl = ( ^aColl add: 1; add: 2; size )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let oc = new_ordered_collection(&mut interp);
    let result = interp.send(
        receiver,
        Symbol::intern("m:"),
        vec![oc],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "OrderedCollection new add: 1; add: 2; size") {
        Value::SmallInt(2) => {}
        other => panic!("expected Value::SmallInt(2), got {other:?}"),
    }
}

#[test]
fn ordered_collection_add_returns_the_argument_not_the_receiver() {
    let mut interp = Interpreter::bootstrap();
    let oc = new_ordered_collection(&mut interp);
    let bindings = Rc::new(std::collections::HashMap::new());
    let result = interp.send(
        oc,
        Symbol::intern("add:"),
        vec![Value::SmallInt(42)],
        &bindings,
        None,
    );
    match expect_ok(result, "OrderedCollection add: 42") {
        Value::SmallInt(42) => {}
        other => panic!("expected Value::SmallInt(42) (the argument), got {other:?}"),
    }
}

#[test]
fn ordered_collection_at_out_of_range_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::bootstrap();
    let oc = new_ordered_collection(&mut interp);
    let bindings = Rc::new(std::collections::HashMap::new());
    let result = interp.send(
        oc,
        Symbol::intern("at:"),
        vec![Value::SmallInt(1)],
        &bindings,
        None,
    );
    let err = expect_error(result, "OrderedCollection (empty) at: 1");
    assert!(
        err.message.to_lowercase().contains("range"),
        "expected an out-of-range-shaped error message, got: {}",
        err.message
    );
}

#[test]
fn block_value_invokes_a_zero_arg_closure() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[42] value )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "[42] value") {
        Value::SmallInt(42) => {}
        other => panic!("expected Value::SmallInt(42), got {other:?}"),
    }
}

#[test]
fn block_value_colon_passes_the_argument_positionally() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x | x + 1] value: 41 )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "[:x | x + 1] value: 41") {
        Value::SmallInt(42) => {}
        other => panic!("expected Value::SmallInt(42), got {other:?}"),
    }
}

#[test]
fn block_num_args_reports_the_declared_parameter_count() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x :y | x + y] numArgs )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    match expect_ok(result, "[:x :y | x + y] numArgs") {
        Value::SmallInt(2) => {}
        other => panic!("expected Value::SmallInt(2), got {other:?}"),
    }
}

/// Required cross-cutting fix: a block called with too few arguments must
/// be a `NewspeakError`, never a Rust panic (`invoke_closure`'s new
/// argument-count guard, Task 10).
#[test]
fn block_called_with_too_few_arguments_is_a_newspeak_error_not_a_panic() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^[:x | x] value )\n) : ()";
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    let err = expect_error(result, "[:x | x] value (0 args, 1 param)");
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
    let (mut interp, class, resolved) = load(src);
    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );
    let result = interp.send(
        receiver,
        Symbol::intern("m"),
        vec![],
        &resolved.bindings,
        None,
    );
    let err = expect_error(result, "[42] value: 1 (1 arg, 0 params)");
    assert!(
        err.message.to_lowercase().contains("argument"),
        "expected an argument-count-shaped error message, got: {}",
        err.message
    );
}
