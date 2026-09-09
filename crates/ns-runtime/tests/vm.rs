//! Bytecode execution through the shared orchestration: `Engine::Bytecode`
//! installed on an `Interpreter`, every send driven via `Interpreter::send`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    compiler, Binding, ClassRef, ClosureBody, Engine, Heap, Interpreter, NewspeakError, Signal,
    SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

/// Loads `src` and switches the interpreter to the bytecode engine.
fn setup(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let (mut interp, class, bindings) = load(src);
    let compiled = compiler::compile_module(interp.heap_ref(), &class, &bindings);
    interp
        .set_engine(Engine::Bytecode(compiled))
        .expect("a source-built interpreter accepts the bytecode engine");
    (interp, class, bindings)
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

fn new_instance(interp: &mut Interpreter, class: &ClassRef, bindings: &Bindings) -> Value {
    expect_ok(interp.instantiate(*class, vec![], bindings), "instantiate")
}

fn send(
    interp: &mut Interpreter,
    receiver: Value,
    selector: &str,
    args: Vec<Value>,
    bindings: &Bindings,
) -> Result<Value, Signal> {
    interp.send(receiver, Symbol::intern(selector), args, bindings, None)
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
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(
        send(&mut interp, instance, "answer", vec![], &bindings),
        "answer",
    );
    assert_eq!(int_of(&result), 42);
}

#[test]
fn a_block_created_by_bytecode_carries_compiled_code_and_is_invocable() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ :x | x + 1 ] )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let block = expect_ok(
        send(&mut interp, instance, "makeBlock", vec![], &bindings),
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
        assert!(
            matches!(closure.body, ClosureBody::Compiled(_)),
            "a closure created by the bytecode engine must carry compiled code"
        );
    }
    let result = expect_ok(
        send(
            &mut interp,
            block,
            "value:",
            vec![Value::SmallInt(9)],
            &bindings,
        ),
        "value:",
    );
    assert_eq!(int_of(&result), 10);
}

#[test]
fn temps_implicit_self_sends_and_arithmetic_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  helper = ( ^40 )\n  public foo = ( | t | t := helper. ^t + 2 )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(int_of(&result), 42);
}

#[test]
fn slots_read_and_write_through_lexical_sends() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | n ::= 1. |\n) (\n  public bump = ( n := n + 1. ^n )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    expect_ok(
        send(&mut interp, instance, "bump", vec![], &bindings),
        "bump",
    );
    let result = expect_ok(
        send(&mut interp, instance, "bump", vec![], &bindings),
        "bump",
    );
    assert_eq!(int_of(&result), 3);
}

#[test]
fn an_unknown_selector_produces_the_shared_does_not_understand_error() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, _class, bindings) = setup(src);
    let err = expect_error(
        send(
            &mut interp,
            Value::SmallInt(3),
            "nonexistentSelector",
            vec![],
            &bindings,
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
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(string_of(interp.heap_ref(), &result), "an Object");
}

#[test]
fn cascades_and_builtin_instantiation_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( | c | c := OrderedCollection new. c add: 1; add: 2. ^c size )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(int_of(&result), 2);
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )\n  public main = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. ^(self find: 2 in: c), (self find: 9 in: c) )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(
        send(&mut interp, instance, "main", vec![], &bindings),
        "main",
    );
    assert_eq!(string_of(interp.heap_ref(), &result), "foundnot found");
}

#[test]
fn a_block_writes_an_enclosing_temp_through_store_outer_local() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public sum = ( | n | n := 0. #(1 2 3) do: [:e | n := n + e]. ^n )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "sum", vec![], &bindings), "sum");
    assert_eq!(int_of(&result), 6);
}

#[test]
fn empty_and_multi_statement_blocks_return_nil_and_the_last_value() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public empty = ( ^[ ] value )\n  public last = ( ^[ 1. 2 ] value )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let empty = expect_ok(
        send(&mut interp, instance, "empty", vec![], &bindings),
        "empty",
    );
    assert!(matches!(empty, Value::Nil));
    let last = expect_ok(
        send(&mut interp, instance, "last", vec![], &bindings),
        "last",
    );
    assert_eq!(int_of(&last), 2);
}

#[test]
fn if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let yes = expect_ok(
        send(
            &mut interp,
            instance,
            "foo:",
            vec![Value::Bool(true)],
            &bindings,
        ),
        "foo:",
    );
    assert_eq!(string_of(interp.heap_ref(), &yes), "yes");
    let no = expect_ok(
        send(
            &mut interp,
            instance,
            "foo:",
            vec![Value::Bool(false)],
            &bindings,
        ),
        "foo:",
    );
    assert_eq!(string_of(interp.heap_ref(), &no), "no");
}

#[test]
fn a_block_invoked_after_its_home_returned_gets_cannot_return() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  mk = ( ^[ ^1 ] )\n  public run = ( ^mk value )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let err = expect_error(send(&mut interp, instance, "run", vec![], &bindings), "run");
    assert!(err.message.contains("cannotReturn"), "{}", err.message);
}

#[test]
fn a_malformed_integer_literal_raises_the_identical_error_under_both_engines() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^99999999999999999999 )\n) : ()";
    let (mut bc, class, bindings) = setup(src);
    let instance = new_instance(&mut bc, &class, &bindings);
    let bc_err = expect_error(
        send(&mut bc, instance, "foo", vec![], &bindings),
        "bytecode",
    );
    let (mut tw, class, bindings) = load(src);
    let instance = new_instance(&mut tw, &class, &bindings);
    let tw_err = expect_error(
        send(&mut tw, instance, "foo", vec![], &bindings),
        "tree-walk",
    );
    assert_eq!(bc_err.message, tw_err.message);
    assert_eq!(bc_err.selector, tw_err.selector);
    assert!(
        bc_err.message.contains("out of i64 range"),
        "{}",
        bc_err.message
    );
}

#[test]
fn a_dynamic_array_literal_evaluates_its_elements_under_bytecode() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( | a | a := {1. 2. 3}. ^a )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
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
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert!(matches!((&result, &instance), (Value::Obj(a), Value::Obj(b)) if a.same(b)));
}

#[test]
fn the_tree_walk_engine_is_still_the_default() {
    let src =
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ 1 ] )\n) : ()";
    let (mut interp, class, bindings) = load(src);
    assert!(interp.compiled_module().is_none());
    let instance = new_instance(&mut interp, &class, &bindings);
    let block = expect_ok(
        send(&mut interp, instance, "makeBlock", vec![], &bindings),
        "makeBlock",
    );
    let Value::Obj(obj) = &block else { panic!() };
    let closure = interp
        .heap_ref()
        .closure_data(obj)
        .unwrap_or_else(|| panic!());
    assert!(matches!(closure.body, ClosureBody::Ast(_)));
}

/// The probe that exposed the frame-shift defect: a method with a declared
/// temporary, reached through a host-initiated send.
///
/// A method activation addresses its arguments and temporaries as one run
/// of registers, and `eval::read_local` finds the boundary from the count
/// of arguments the frame was *given*. Before `invoke_method` checked
/// arity, sending `probe` two surplus arguments therefore made register 0
/// -- `t` -- read back the first surplus argument instead of `Nil`, under
/// both engines identically, so the differential harness could not see it.
const ARITY_PROBE: &str = "Newspeak3\n'T'\nclass Probe = Object new (\n) (\n\
     \tpublic probe = (\n\t\t| t |\n\t\t^nil = t ifTrue: [ 'nil-ok' ] ifFalse: [ 'NOTNIL-BUG' ]\n\t)\n) : ()";

#[test]
fn a_correctly_sent_method_reads_its_temporary_as_nil_under_both_engines() {
    for build in [load, setup] {
        let (mut interp, class, bindings) = build(ARITY_PROBE);
        let instance = new_instance(&mut interp, &class, &bindings);
        let result = expect_ok(
            send(&mut interp, instance, "probe", vec![], &bindings),
            "probe",
        );
        assert_eq!(string_of(interp.heap_ref(), &result), "nil-ok");
    }
}

#[test]
fn a_wrong_arity_send_is_refused_identically_by_both_engines() {
    let mut messages = Vec::new();
    for build in [load, setup] {
        let (mut interp, class, bindings) = build(ARITY_PROBE);
        let instance = new_instance(&mut interp, &class, &bindings);
        let platform = interp.platform();
        let error = expect_error(
            send(
                &mut interp,
                instance,
                "probe",
                vec![platform, Value::Nil],
                &bindings,
            ),
            "probe with two surplus arguments",
        );
        messages.push(error.message);
    }
    assert_eq!(
        messages[0], "wrong number of arguments: `probe` takes 0, got 2",
        "the tree-walking engine's message"
    );
    assert_eq!(
        messages[0], messages[1],
        "both engines must report a wrong-arity send with the same text"
    );
}
