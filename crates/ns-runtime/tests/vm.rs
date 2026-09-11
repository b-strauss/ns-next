//! Bytecode execution through the shared orchestration, every send driven via `Interpreter::send`.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{ClassRef, Heap, Interpreter, NewspeakError, Signal, SourceOrigin, Symbol, Value};

fn load(src: &str) -> (Interpreter, ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

/// `Signal` carries no `Debug` impl (M1 decision), so `Result::expect` is
/// unusable on an `EvalResult` -- unwrap by hand.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_error(result: Result<Value, Signal>, what: &str) -> NewspeakError {
    match result {
        Ok(_) => panic!("{what}: expected a NewspeakError, got a value"),
        Err(Signal::Return(..)) => panic!("{what}: expected a NewspeakError, got Signal::Return"),
        Err(Signal::Error(e)) => e,
    }
}

fn new_instance(interp: &mut Interpreter, class: &ClassRef) -> Value {
    expect_ok(interp.instantiate(*class, vec![]), "instantiate")
}

fn send(
    interp: &mut Interpreter,
    receiver: Value,
    selector: &str,
    args: Vec<Value>,
) -> Result<Value, Signal> {
    interp.send(receiver, Symbol::intern(selector), args)
}

fn string_of(heap: &Heap, v: &Value) -> String {
    let Value::Obj(obj) = v else {
        panic!("expected a string object")
    };
    heap.str_value(obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Str"))
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

#[test]
fn a_user_method_runs_as_bytecode_and_returns_its_value() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public answer = ( ^42 )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "answer", vec![]), "answer");
    assert_eq!(int_of(&result), 42);
}

#[test]
fn a_block_created_by_bytecode_carries_compiled_code_and_is_invocable() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ :x | x + 1 ] )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let block = expect_ok(
        send(&mut interp, instance, "makeBlock", vec![]),
        "makeBlock",
    );
    {
        let Value::Obj(obj) = &block else {
            panic!("expected a closure object")
        };
        let closure = interp
            .heap_ref()
            .closure_data(obj)
            .unwrap_or_else(|| panic!("expected a closure"));
        assert_eq!(
            closure.code.num_args, 1,
            "the closure carries the compiled body of `[ :x | x + 1 ]`"
        );
    }
    let result = expect_ok(
        send(&mut interp, block, "value:", vec![Value::SmallInt(9)]),
        "value:",
    );
    assert_eq!(int_of(&result), 10);
}

#[test]
fn temps_implicit_self_sends_and_arithmetic_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  helper = ( ^40 )\n  public foo = ( | t | t := helper. ^t + 2 )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![]), "foo");
    assert_eq!(int_of(&result), 42);
}

#[test]
fn slots_read_and_write_through_lexical_sends() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | n ::= 1. |\n) (\n  public bump = ( n := n + 1. ^n )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    expect_ok(send(&mut interp, instance, "bump", vec![]), "bump");
    let result = expect_ok(send(&mut interp, instance, "bump", vec![]), "bump");
    assert_eq!(int_of(&result), 3);
}

#[test]
fn an_unknown_selector_produces_the_shared_does_not_understand_error() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, _class) = load(src);
    let err = expect_error(
        send(
            &mut interp,
            Value::SmallInt(3),
            "nonexistentSelector",
            vec![],
        ),
        "dnu",
    );
    assert!(
        err.message
            .contains("does not understand #nonexistentSelector"),
        "{}",
        err.message
    );
}

#[test]
fn a_super_send_falls_through_to_objects_native_print_string() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^super printString )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![]), "foo");
    assert_eq!(string_of(interp.heap_ref(), &result), "an Object");
}

#[test]
fn cascades_and_builtin_instantiation_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( | c | c := OrderedCollection new. c add: 1; add: 2. ^c size )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![]), "foo");
    assert_eq!(int_of(&result), 2);
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )\n  public main = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. ^(self find: 2 in: c), (self find: 9 in: c) )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "main", vec![]), "main");
    assert_eq!(string_of(interp.heap_ref(), &result), "foundnot found");
}

#[test]
fn a_block_writes_an_enclosing_temp_through_store_outer_local() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public sum = ( | n | n := 0. #(1 2 3) do: [:e | n := n + e]. ^n )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "sum", vec![]), "sum");
    assert_eq!(int_of(&result), 6);
}

#[test]
fn empty_and_multi_statement_blocks_return_nil_and_the_last_value() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public empty = ( ^[ ] value )\n  public last = ( ^[ 1. 2 ] value )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let empty = expect_ok(send(&mut interp, instance, "empty", vec![]), "empty");
    assert!(matches!(empty, Value::Nil));
    let last = expect_ok(send(&mut interp, instance, "last", vec![]), "last");
    assert_eq!(int_of(&last), 2);
}

#[test]
fn if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let yes = expect_ok(
        send(&mut interp, instance, "foo:", vec![Value::Bool(true)]),
        "foo:",
    );
    assert_eq!(string_of(interp.heap_ref(), &yes), "yes");
    let no = expect_ok(
        send(&mut interp, instance, "foo:", vec![Value::Bool(false)]),
        "foo:",
    );
    assert_eq!(string_of(interp.heap_ref(), &no), "no");
}

#[test]
fn a_block_invoked_after_its_home_returned_gets_cannot_return() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  mk = ( ^[ ^1 ] )\n  public run = ( ^mk value )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let err = expect_error(send(&mut interp, instance, "run", vec![]), "run");
    assert!(err.message.contains("cannotReturn"), "{}", err.message);
}

#[test]
fn a_malformed_integer_literal_raises_an_out_of_range_error_when_evaluated() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^99999999999999999999 )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let err = expect_error(send(&mut interp, instance, "foo", vec![]), "foo");
    assert!(err.message.contains("out of i64 range"), "{}", err.message);
}

#[test]
fn a_dynamic_array_literal_evaluates_its_elements_under_bytecode() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( | a | a := {1. 2. 3}. ^a )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![]), "foo");
    let Value::Obj(obj) = &result else {
        panic!("expected an array object")
    };
    let elements = interp
        .heap_ref()
        .array_elements(obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Array"));
    assert_eq!(
        elements.iter().map(int_of).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[test]
fn a_method_that_falls_off_its_end_implicitly_returns_self() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( 1 + 1 )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![]), "foo");
    assert!(matches!((&result, &instance), (Value::Obj(a), Value::Obj(b)) if a.same(b)));
}

/// Loading and compiling are one step: a class `load_module` returns is
/// already runnable as bytecode, so nothing can instantiate or send to a
/// loaded class whose code was never compiled.
#[test]
fn loading_a_module_installs_its_compiled_code() {
    let src =
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ 1 ] )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = new_instance(&mut interp, &class);
    let block = expect_ok(
        send(&mut interp, instance, "makeBlock", vec![]),
        "makeBlock",
    );
    let Value::Obj(obj) = &block else {
        panic!("expected a closure object")
    };
    let closure = interp
        .heap_ref()
        .closure_data(obj)
        .unwrap_or_else(|| panic!("expected ObjectData::Closure"));
    assert_eq!(closure.code.num_args, 0);
}

/// A second `load_module` into the same interpreter adds its code to the
/// first module's rather than replacing it: every key is a mixin's or a
/// method's `Rc` identity, so two modules' bodies never collide, and a class
/// from the first load must keep running.
#[test]
fn loading_a_second_module_keeps_the_first_modules_code() {
    let first = "Newspeak3\n'T'\nclass A = Object new (\n) (\n  public answer = ( ^1 )\n) : ()";
    let second = "Newspeak3\n'T'\nclass B = Object new (\n) (\n  public answer = ( ^2 )\n) : ()";
    let (mut interp, a) = load(first);
    let module = ns_syntax::parse(second).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(second),
        SourceOrigin::File(PathBuf::from("b.ns")),
    )
    .expect("resolve");
    let b = interp.load_module(&resolved);
    let a_instance = new_instance(&mut interp, &a);
    let b_instance = new_instance(&mut interp, &b);
    let from_a = expect_ok(send(&mut interp, a_instance, "answer", vec![]), "A answer");
    let from_b = expect_ok(send(&mut interp, b_instance, "answer", vec![]), "B answer");
    assert_eq!(int_of(&from_a), 1);
    assert_eq!(int_of(&from_b), 2);
}

/// The probe that exposed the frame-shift defect: a method with a declared
/// temporary, reached through a host-initiated send.
///
/// A method activation addresses its arguments and temporaries as one run
/// of registers, and `eval::read_local` finds the boundary from the count
/// of arguments the frame was *given*. Before `invoke_method` checked
/// arity, sending `probe` two surplus arguments therefore made register 0
/// -- `t` -- read back the first surplus argument instead of `Nil`,
/// silently.
const ARITY_PROBE: &str = "Newspeak3\n'T'\nclass Probe = Object new (\n) (\n\
     \tpublic probe = (\n\t\t| t |\n\t\t^nil = t ifTrue: [ 'nil-ok' ] ifFalse: [ 'NOTNIL-BUG' ]\n\t)\n) : ()";

#[test]
fn a_correctly_sent_method_reads_its_temporary_as_nil() {
    let (mut interp, class) = load(ARITY_PROBE);
    let instance = new_instance(&mut interp, &class);
    let result = expect_ok(send(&mut interp, instance, "probe", vec![]), "probe");
    assert_eq!(string_of(interp.heap_ref(), &result), "nil-ok");
}

#[test]
fn a_wrong_arity_send_is_refused() {
    let (mut interp, class) = load(ARITY_PROBE);
    let instance = new_instance(&mut interp, &class);
    let platform = interp.platform();
    let error = expect_error(
        send(&mut interp, instance, "probe", vec![platform, Value::Nil]),
        "probe with two surplus arguments",
    );
    assert_eq!(
        error.message,
        "wrong number of arguments: `probe` takes 0, got 2"
    );
}
