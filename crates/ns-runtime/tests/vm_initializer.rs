//! Hook 3: instance initializers (slot initializers + initExprs) under the
//! bytecode engine, compared against the tree-walker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    compiler, Binding, ClassRef, ClosureBody, Engine, FrameKind, Interpreter, ObjectData, Signal,
    SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str, bytecode: bool) -> (Interpreter, ClassRef, Bindings) {
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
    if bytecode {
        let compiled = compiler::compile_module(&class, &bindings);
        interp.set_engine(Engine::Bytecode(compiled));
    }
    (interp, class, bindings)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

/// Instantiates with `factory_args`, sends `getter`, returns the result --
/// under whichever engine `load` installed.
fn instantiate_and_get(src: &str, bytecode: bool, factory_args: Vec<Value>, getter: &str) -> Value {
    let (mut interp, class, bindings) = load(src, bytecode);
    let instance = expect_ok(
        interp.instantiate(class, factory_args, &bindings),
        "instantiate",
    );
    expect_ok(
        interp.send(instance, Symbol::intern(getter), vec![], &bindings, None),
        getter,
    )
}

const FACTORY_PARAM: &str = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. |\n) (\n  public p2 = ( ^p )\n) : ()";
const INIT_EXPRS: &str = "Newspeak3\n'T'\nclass Foo = Object new (\n  | q ::= 5. r ::= 0. |\n  r := q + 1.\n) (\n  public r2 = ( ^r )\n) : ()";

#[test]
fn a_slot_initializer_reads_a_factory_parameter_under_bytecode() {
    let v = instantiate_and_get(FACTORY_PARAM, true, vec![Value::SmallInt(7)], "p2");
    assert_eq!(int_of(&v), 7);
}

#[test]
fn init_exprs_run_after_slot_initializers_under_bytecode() {
    let v = instantiate_and_get(INIT_EXPRS, true, vec![], "r2");
    assert_eq!(int_of(&v), 6);
}

#[test]
fn both_engines_produce_identical_instance_state() {
    for (src, args, getter) in [
        (FACTORY_PARAM, vec![Value::SmallInt(7)], "p2"),
        (INIT_EXPRS, vec![], "r2"),
    ] {
        let tw = instantiate_and_get(src, false, args.clone(), getter);
        let bc = instantiate_and_get(src, true, args, getter);
        assert_eq!(int_of(&tw), int_of(&bc), "{getter}: engines disagree");
    }
}

#[test]
fn a_closure_created_in_a_slot_initializer_is_compiled_and_homed_at_the_initializer_frame() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | blk = [ 3 ]. |\n) (\n  public blk2 = ( ^blk )\n) : ()";
    let v = instantiate_and_get(src, true, vec![], "blk2");
    let Value::Obj(obj) = &v else {
        panic!("expected a closure object")
    };
    let ObjectData::Closure(closure) = &obj.borrow().data else {
        panic!("expected a closure")
    };
    assert!(matches!(closure.body, ClosureBody::Compiled(_)));
    assert!(matches!(
        closure.home.borrow().kind,
        FrameKind::Initializer(_)
    ));
}

#[test]
fn a_return_inside_a_block_in_init_exprs_escapes_instantiate_under_both_engines() {
    // M1 gap, deliberately carried forward in both engines (spec: "Class
    // instantiation"): the `Signal::Return` targets the initializer frame
    // and nothing catches it.
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | |\n  [ ^1 ] value.\n) (\n) : ()";
    for bytecode in [false, true] {
        let (mut interp, class, bindings) = load(src, bytecode);
        let result = interp.instantiate(class, vec![], &bindings);
        assert!(
            matches!(result, Err(Signal::Return(..))),
            "bytecode={bytecode}"
        );
    }
}

#[test]
fn a_factory_arity_mismatch_is_still_the_shared_error() {
    let (mut interp, class, bindings) = load(FACTORY_PARAM, true);
    match interp.instantiate(class, vec![], &bindings) {
        Err(Signal::Error(e)) => assert!(
            e.message.contains("wrong number of factory arguments"),
            "{}",
            e.message
        ),
        _ => panic!("expected the arity error"),
    }
}
