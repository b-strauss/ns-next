//! Bytecode execution, every send driven through `Interpreter::send`.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    Heap, HostError, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value,
};

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
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

fn new_instance(interpreter: &mut Interpreter, class: &Root) -> Root {
    expect_ok(interpreter.instantiate(class, &[]), "instantiate")
}

fn send(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[Root],
) -> HostResult {
    interpreter.send(receiver, Symbol::intern(selector), arguments)
}

fn string_of<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> String {
    let Value::Object(object) = value else {
        panic!("expected a string object")
    };
    heap.str_value(*object)
        .unwrap_or_else(|| panic!("expected ObjectData::String"))
}

fn integer_of<'gc>(value: &Value<'gc>) -> i64 {
    match value {
        Value::SmallInteger(integer) => *integer,
        _ => panic!("expected a SmallInteger"),
    }
}

#[test]
fn a_user_method_runs_as_bytecode_and_returns_its_value() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public answer = ( ^42 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "answer", &[]), "answer");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        42
    );
}

#[test]
fn a_block_created_by_bytecode_carries_compiled_code_and_is_invocable() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public makeBlock = ( ^[ :x | x + 1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let block = expect_ok(
        send(&mut interpreter, &instance, "makeBlock", &[]),
        "makeBlock",
    );
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&block) else {
            panic!("expected a closure object")
        };
        let closure = mutator
            .heap()
            .closure_data(object)
            .unwrap_or_else(|| panic!("expected a closure"));
        assert_eq!(
            closure.code.argument_count, 1,
            "the closure carries the compiled body of `[ :x | x + 1 ]`"
        );
    });
    let nine = interpreter.stash(Value::SmallInteger(9));
    let result = expect_ok(send(&mut interpreter, &block, "value:", &[nine]), "value:");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        10
    );
}

#[test]
fn temporaries_implicit_self_sends_and_arithmetic_work() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          helper = ( ^40 )
          public foo = ( | t | t := helper. ^t + 2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        42
    );
}

#[test]
fn slots_read_and_write_through_lexical_sends() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | n ::= 1. |
        ) (
          public bump = ( n := n + 1. ^n )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    expect_ok(send(&mut interpreter, &instance, "bump", &[]), "bump");
    let result = expect_ok(send(&mut interpreter, &instance, "bump", &[]), "bump");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        3
    );
}

#[test]
fn an_unknown_selector_produces_the_shared_does_not_understand_error() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, _class) = load(source);
    let three = interpreter.stash(Value::SmallInteger(3));
    let result = send(&mut interpreter, &three, "nonexistentSelector", &[]);
    let error = expect_error(&mut interpreter, result, "dnu");
    assert!(
        error
            .message
            .contains("does not understand #nonexistentSelector"),
        "{}",
        error.message
    );
}

#[test]
fn a_super_send_falls_through_to_objects_native_print_string() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( ^super printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_of(mutator.heap(), &mutator.fetch(&result)),
            "an Object"
        );
    });
}

#[test]
fn cascades_and_builtin_instantiation_work() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( | c | c := OrderedCollection new. c add: 1; add: 2. ^c size )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        2
    );
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )
          public main = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. ^(self find: 2 in: c), (self find: 9 in: c) )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "main", &[]), "main");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_of(mutator.heap(), &mutator.fetch(&result)),
            "foundnot found"
        );
    });
}

#[test]
fn a_block_writes_an_enclosing_temporary_through_store_outer_local() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public sum = ( | n | n := 0. #(1 2 3) do: [:e | n := n + e]. ^n )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "sum", &[]), "sum");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&result))),
        6
    );
}

#[test]
fn empty_and_multi_statement_blocks_return_nil_and_the_last_value() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public empty = ( ^[ ] value )
          public last = ( ^[ 1. 2 ] value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let empty = expect_ok(send(&mut interpreter, &instance, "empty", &[]), "empty");
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&empty), Value::Nil)));
    let last = expect_ok(send(&mut interpreter, &instance, "last", &[]), "last");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&last))),
        2
    );
}

#[test]
fn if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let yes = interpreter.stash(Value::Boolean(true));
    let yes = expect_ok(send(&mut interpreter, &instance, "foo:", &[yes]), "foo:");
    interpreter.enter(|mutator| {
        assert_eq!(string_of(mutator.heap(), &mutator.fetch(&yes)), "yes");
    });
    let false_argument = interpreter.stash(Value::Boolean(false));
    let answer = expect_ok(
        send(&mut interpreter, &instance, "foo:", &[false_argument]),
        "foo:",
    );
    interpreter.enter(|mutator| {
        assert_eq!(string_of(mutator.heap(), &mutator.fetch(&answer)), "no");
    });
}

#[test]
fn a_block_invoked_after_its_home_returned_gets_cannot_return() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          mk = ( ^[ ^1 ] )
          public run = ( ^mk value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = send(&mut interpreter, &instance, "run", &[]);
    let error = expect_error(&mut interpreter, result, "run");
    assert!(error.message.contains("cannotReturn"), "{}", error.message);
}

#[test]
fn a_malformed_integer_literal_raises_an_out_of_range_error_when_evaluated() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( ^99999999999999999999 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = send(&mut interpreter, &instance, "foo", &[]);
    let error = expect_error(&mut interpreter, result, "foo");
    assert!(
        error.message.contains("out of i64 range"),
        "{}",
        error.message
    );
    assert_eq!(error.selector, Symbol::intern("parse"));
}

#[test]
fn a_dynamic_array_literal_evaluates_its_elements() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( | a | a := {1. 2. 3}. ^a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&result) else {
            panic!("expected an array object")
        };
        let elements = mutator
            .heap()
            .array_elements(object)
            .unwrap_or_else(|| panic!("expected ObjectData::Array"));
        assert_eq!(
            elements.iter().map(integer_of).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    });
}

#[test]
fn a_method_that_falls_off_its_end_implicitly_returns_self() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( 1 + 1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    interpreter.enter(|mutator| {
        let (result, instance) = (mutator.fetch(&result), mutator.fetch(&instance));
        assert!(matches!((&result, &instance), (Value::Object(result_object), Value::Object(instance_object)) if result_object.same(*instance_object)));
    });
}

/// Loading and compiling are one step: a class `load_module` returns is
/// already runnable as bytecode, so nothing can instantiate or send to a
/// loaded class whose code was never compiled.
#[test]
fn loading_a_module_installs_its_compiled_code() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public makeBlock = ( ^[ 1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let block = expect_ok(
        send(&mut interpreter, &instance, "makeBlock", &[]),
        "makeBlock",
    );
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&block) else {
            panic!("expected a closure object")
        };
        let closure = mutator
            .heap()
            .closure_data(object)
            .unwrap_or_else(|| panic!("expected ObjectData::Closure"));
        assert_eq!(closure.code.argument_count, 0);
    });
}

/// A second `load_module` into the same interpreter adds its code to the
/// first module's rather than replacing it: every key is a mixin's or a
/// method's `Rc` identity, so two modules' bodies never collide, and a class
/// from the first load must keep running.
#[test]
fn loading_a_second_module_keeps_the_first_modules_code() {
    let first = indoc! {"
        Newspeak3
        'T'
        class A = Object new (
        ) (
          public answer = ( ^1 )
        ) : ()
    "};
    let second = indoc! {"
        Newspeak3
        'T'
        class B = Object new (
        ) (
          public answer = ( ^2 )
        ) : ()
    "};
    let (mut interpreter, a_class) = load(first);
    let module = ns_syntax::parse(second).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(second),
        SourceOrigin::File(PathBuf::from("b.ns")),
    )
    .expect("resolve");
    let b_class = interpreter.load_module(&resolved);
    let a_instance = new_instance(&mut interpreter, &a_class);
    let b_instance = new_instance(&mut interpreter, &b_class);
    let a_answer = expect_ok(
        send(&mut interpreter, &a_instance, "answer", &[]),
        "A answer",
    );
    let b_answer = expect_ok(
        send(&mut interpreter, &b_instance, "answer", &[]),
        "B answer",
    );
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&a_answer))),
        1
    );
    assert_eq!(
        interpreter.enter(|mutator| integer_of(&mutator.fetch(&b_answer))),
        2
    );
}

/// The probe that exposed the frame-shift defect: a method with a declared
/// temporary, reached through a host-initiated send.
///
/// A method activation addresses its arguments and temporaries as one run
/// of registers, and `locals::read_local` finds the boundary from the count
/// of arguments the frame was *given*. Without the arity check every method
/// activation gets (`Mutator::push_step`), sending `probe` two surplus
/// arguments therefore made register 0
/// -- `t` -- read back the first surplus argument instead of `Nil`,
/// silently.
const ARITY_PROBE: &str = indoc! {"
    Newspeak3
    'T'
    class Probe = Object new (
    ) (
        public probe = (
            | t |
            ^nil = t ifTrue: [ 'nil-ok' ] ifFalse: [ 'NOTNIL-BUG' ]
        )
    ) : ()
"};

#[test]
fn a_correctly_sent_method_reads_its_temporary_as_nil() {
    let (mut interpreter, class) = load(ARITY_PROBE);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "probe", &[]), "probe");
    interpreter.enter(|mutator| {
        assert_eq!(string_of(mutator.heap(), &mutator.fetch(&result)), "nil-ok");
    });
}

#[test]
fn a_wrong_arity_send_is_refused() {
    let (mut interpreter, class) = load(ARITY_PROBE);
    let instance = new_instance(&mut interpreter, &class);
    let platform = interpreter.platform();
    let nil = interpreter.stash(Value::Nil);
    let result = send(&mut interpreter, &instance, "probe", &[platform, nil]);
    let error = expect_error(&mut interpreter, result, "probe with two surplus arguments");
    assert_eq!(
        error.message,
        "wrong number of arguments: `probe` takes 0, got 2"
    );
}
