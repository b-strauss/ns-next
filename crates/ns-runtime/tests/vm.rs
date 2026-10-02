//! Bytecode execution, every send driven through `Interpreter::send`.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{CodeKind, CompiledCode, Instruction, Literal};
use ns_runtime::{
    ClassReference, Heap, HostError, HostResult, HostSignal, Interpreter, Mixin, Mutator, Root,
    SourceOrigin, Symbol, Value, ValueKind, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN,
};
use ns_syntax::Span;

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
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&root)));
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

fn string_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> String {
    let Some(object) = value.as_object() else {
        panic!("expected a string object")
    };
    heap.str_value(object)
        .unwrap_or_else(|| panic!("expected ObjectData::String"))
}

fn integer_of<'gc>(value: Value<'gc>) -> i64 {
    match value.kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    }
}

/// The class object `root` holds, for the heap accessors that take one.
/// Fetched where it is used: a run may move it. Follows the same pattern as
/// `tests/image.rs`'s helper of the same name.
fn class_reference<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ClassReference<'gc> {
    match mutator.fetch(root).kind() {
        ValueKind::Object(class) => class,
        _ => panic!("expected a class object"),
    }
}

/// `class`'s own mixin as a value a test can edit. The clone is shallow: its
/// methods, nested declarations and class side are the same `Rc`s.
fn mixin_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Mixin {
    (*heap.class_application(class).expect("a class object").mixin).clone()
}

/// Replaces `class`'s mixin with `mixin`, applied to the same superclass
/// application -- the shape an edit to a live class takes, and how a test
/// puts a hand-built body in front of the running interpreter. Takes
/// `&mut Mutator` and is called from inside an entry.
fn install_mixin<'gc>(mutator: &mut Mutator<'gc>, class: ClassReference<'gc>, mixin: Mixin) {
    let super_application = mutator
        .heap()
        .class_application(class)
        .expect("a class object")
        .superclass
        .clone();
    let application = mutator.apply_mixin(&Rc::new(mixin), super_application.as_ref());
    assert!(
        mutator.heap_mut().set_class_application(class, application),
        "a class object"
    );
}

/// `mixin` with method `selector`'s body replaced by `code`.
fn with_method_code(mut mixin: Mixin, selector: &str, code: Rc<CompiledCode>) -> Mixin {
    let selector = Symbol::intern(selector);
    let method = mixin
        .compiled_method(selector)
        .expect("the mixin declares the method")
        .clone();
    mixin.methods.insert(
        selector,
        ns_runtime::mixin::MethodEntry::Compiled(Rc::new(ns_runtime::mixin::MixinMethod {
            code,
            ..(*method).clone()
        })),
    );
    mixin
}

#[test]
fn a_user_method_runs_as_bytecode_and_returns_its_value() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public answer = ( ^42 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "answer", &[]), "answer");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        42
    );
}

#[test]
fn a_block_created_by_bytecode_carries_compiled_code_and_is_invocable() {
    let source = indoc! {"
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
        let Some(object) = mutator.fetch(&block).as_object() else {
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
    let nine = interpreter.stash(Value::small_integer(9));
    let result = expect_ok(send(&mut interpreter, &block, "value:", &[nine]), "value:");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        10
    );
}

#[test]
fn temporaries_implicit_self_sends_and_arithmetic_work() {
    let source = indoc! {"
        class X = Object new (
        ) (
          helper = ( ^40 )
          public foo = ( | t | t:: helper. ^t + 2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        42
    );
}

#[test]
fn slots_read_and_write_through_lexical_sends() {
    let source = indoc! {"
        class X = Object new (
          | n ::= 1. |
        ) (
          public bump = ( n:: n + 1. ^n )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    expect_ok(send(&mut interpreter, &instance, "bump", &[]), "bump");
    let result = expect_ok(send(&mut interpreter, &instance, "bump", &[]), "bump");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        3
    );
}

#[test]
fn an_unknown_selector_produces_the_shared_does_not_understand_error() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, _class) = load(source);
    let three = interpreter.stash(Value::small_integer(3));
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
            string_of(mutator.heap(), mutator.fetch(&result)),
            "an Object"
        );
    });
}

#[test]
fn cascades_and_list_instantiation_work() {
    let source = indoc! {"
        class X usingPlatform: platform = Object new (
          | List = platform collections List. |
        ) (
          public foo = ( | c | c:: List new. c add: 1; add: 2. ^c size )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(interpreter.instantiate(&class, &[platform]), "instantiate");
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        2
    );
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let source = indoc! {"
        class X = Object new (
        ) (
          find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )
          public main = ( ^(self find: 2 in: {1. 2. 3}), (self find: 9 in: {1. 2. 3}) )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "main", &[]), "main");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_of(mutator.heap(), mutator.fetch(&result)),
            "foundnot found"
        );
    });
}

#[test]
fn a_block_writes_an_enclosing_temporary_through_store_outer_local() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public sum = ( | n | n:: 0. {1. 2. 3} do: [:e | n:: n + e]. ^n )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "sum", &[]), "sum");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        6
    );
}

#[test]
fn empty_and_multi_statement_blocks_return_nil_and_the_last_value() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public empty = ( ^[ ] value )
          public last = ( ^[ 1. 2 ] value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let empty = expect_ok(send(&mut interpreter, &instance, "empty", &[]), "empty");
    assert!(interpreter.enter(|mutator| mutator.fetch(&empty).is_nil()));
    let last = expect_ok(send(&mut interpreter, &instance, "last", &[]), "last");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&last))),
        2
    );
}

#[test]
fn if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let yes = interpreter.stash(Value::boolean(true));
    let yes = expect_ok(send(&mut interpreter, &instance, "foo:", &[yes]), "foo:");
    interpreter.enter(|mutator| {
        assert_eq!(string_of(mutator.heap(), mutator.fetch(&yes)), "yes");
    });
    let false_argument = interpreter.stash(Value::boolean(false));
    let answer = expect_ok(
        send(&mut interpreter, &instance, "foo:", &[false_argument]),
        "foo:",
    );
    interpreter.enter(|mutator| {
        assert_eq!(string_of(mutator.heap(), mutator.fetch(&answer)), "no");
    });
}

#[test]
fn a_block_invoked_after_its_home_returned_gets_cannot_return() {
    let source = indoc! {"
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
fn an_integer_literal_beyond_i64_evaluates_to_a_large_integer() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public foo = ( ^99999999999999999999 printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_of(mutator.heap(), mutator.fetch(&result)),
            "99999999999999999999"
        );
    });
}

#[test]
fn a_dynamic_array_literal_evaluates_its_elements() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public foo = ( | a | a:: {1. 2. 3}. ^a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = expect_ok(send(&mut interpreter, &instance, "foo", &[]), "foo");
    interpreter.enter(|mutator| {
        let Some(object) = mutator.fetch(&result).as_object() else {
            panic!("expected an array object")
        };
        let elements = mutator
            .heap()
            .array_elements(object)
            .unwrap_or_else(|| panic!("expected ObjectData::Array"));
        assert_eq!(
            elements.iter().copied().map(integer_of).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    });
}

#[test]
fn a_method_that_falls_off_its_end_implicitly_returns_self() {
    let source = indoc! {"
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
        assert!(matches!((result.as_object(), instance.as_object()), (Some(result_object), Some(instance_object)) if result_object.same(instance_object)));
    });
}

/// Loading and compiling are one step: a class `load_module` returns is
/// already runnable as bytecode, so nothing can instantiate or send to a
/// loaded class whose code was never compiled.
#[test]
fn loading_a_module_installs_its_compiled_code() {
    let source = indoc! {"
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
        let Some(object) = mutator.fetch(&block).as_object() else {
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
        class A = Object new (
        ) (
          public answer = ( ^1 )
        ) : ()
    "};
    let second = indoc! {"
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
        interpreter.enter(|mutator| integer_of(mutator.fetch(&a_answer))),
        1
    );
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&b_answer))),
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
        assert_eq!(string_of(mutator.heap(), mutator.fetch(&result)), "nil-ok");
    });
}

#[test]
fn a_wrong_arity_send_is_refused() {
    let (mut interpreter, class) = load(ARITY_PROBE);
    let instance = new_instance(&mut interpreter, &class);
    let platform = interpreter.platform();
    let nil = interpreter.stash(Value::nil());
    let result = send(&mut interpreter, &instance, "probe", &[platform, nil]);
    let error = expect_error(&mut interpreter, result, "probe with two surplus arguments");
    assert_eq!(
        error.message,
        "wrong number of arguments: `probe` takes 0, got 2"
    );
}

// ---------------------------------------------------------------------------
// The indirection vector's three instructions (`MakeVector`, `LoadVector`,
// `StoreVector`) -- closure conversion's own storage. Hand-built rather than
// compiled, so that each instruction's own behaviour is pinned independently
// of which shapes the optimized lowering happens to emit them for; the
// goldens under `goldens/language/block/` cover the compiled side.
// ---------------------------------------------------------------------------

#[test]
fn a_vector_round_trips_a_value_through_its_slot() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public compute = ( ^0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // MakeVector r0, size 1; LoadLiteral r1 <- 42; StoreVector r0[0] <- r1;
        // LoadVector r2 <- r0[0]; Return r2  =>  42
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::MakeVector {
                    destination: 0,
                    size: 1,
                },
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: Literal::SmallInteger(42),
                },
                Instruction::StoreVector {
                    vector: 0,
                    index: 0,
                    source: 1,
                },
                Instruction::LoadVector {
                    destination: 2,
                    vector: 0,
                    index: 0,
                },
                Instruction::Return { register: 2 },
            ],
            argument_count: 0,
            register_count: 3,
            statement_program_counter: vec![(0, 0..5)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "compute", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = expect_ok(send(&mut interpreter, &instance, "compute", &[]), "compute");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        42
    );
}

#[test]
fn reading_a_vector_slot_that_is_not_there_is_an_error_not_a_panic() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public bad = ( ^0 )
          public good = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // LoadLiteral r0 <- 7 (not an Array); LoadVector r1 <- r0[0]; Return r1
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: Literal::SmallInteger(7),
                },
                Instruction::LoadVector {
                    destination: 1,
                    vector: 0,
                    index: 0,
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 0,
            register_count: 2,
            statement_program_counter: vec![(0, 0..3)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "bad", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = send(&mut interpreter, &instance, "bad", &[]);
    let error = expect_error(&mut interpreter, result, "bad");
    assert!(
        error.message.contains("indirection vector"),
        "{}",
        error.message
    );
    // The activation stack unwound to a clean floor rather than aborting the
    // process: the same interpreter, on the same instance, still answers an
    // unrelated send.
    let result = expect_ok(send(&mut interpreter, &instance, "good", &[]), "good");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        1
    );
}

/// The case a crafted image most plausibly produces: a real `Array`, just
/// too short for the index the body addresses. `Heap::array_at`/`array_set`
/// answer `None`/`false` rather than panicking, and `load_vector`/
/// `store_vector` turn that into the same Newspeak error as a
/// completely-wrong-shaped vector.
#[test]
fn a_vector_index_past_the_arrays_end_is_the_same_error_not_a_panic() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public bad = ( ^0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // MakeVector r0, size 1 (one slot); LoadVector r1 <- r0[5] (out of
        // range); Return r1
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::MakeVector {
                    destination: 0,
                    size: 1,
                },
                Instruction::LoadVector {
                    destination: 1,
                    vector: 0,
                    index: 5,
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 0,
            register_count: 2,
            statement_program_counter: vec![(0, 0..3)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "bad", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = send(&mut interpreter, &instance, "bad", &[]);
    let error = expect_error(&mut interpreter, result, "bad");
    assert!(
        error.message.contains("indirection vector"),
        "{}",
        error.message
    );
}

// ---------------------------------------------------------------------------
// §6.3.2's simultaneous slots' four instructions (`MakeFuture`,
// `ResolveFuture`, `LoadOwnSlot`, `InitializeSlotProvisionally`). Hand-built,
// like the indirection vector's above, so each instruction is driven on its
// own -- `ResolveFuture`'s forcing path (`vm::force_and_reexecute`, the
// `Reexecute` branch of `interpreter::future::resolve_for_instruction`)
// included -- apart from the lowering of a simultaneous clause that emits
// them together.
// ---------------------------------------------------------------------------

/// A block whose body answers `value` and nothing else -- the future's
/// computation in the tests below.
fn constant_block(value: i64) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instructions: vec![
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::SmallInteger(value),
            },
            Instruction::Return { register: 0 },
        ],
        argument_count: 0,
        register_count: 1,
        statement_program_counter: vec![(0, 0..2)],
        family_size: 0,
        copied_count: 0,
        outer_reach: 0,
        unwind_marker: None,
        source: Span::new(0, 0),
    })
}

#[test]
fn make_future_load_own_slot_and_resolve_future_run_a_simultaneous_clause_by_hand() {
    let source = indoc! {"
        class X = Object new (
          | n |
        ) (
          public probe = ( ^0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // MakeClosure r0 <- the block answering 99;
        // MakeFuture r1 <- future of r0;
        // InitSlotProvisionally own 0 <- r1        (n holds the unresolved future)
        // LoadOwnSlot r2 <- own 0                  (a simultaneous clause's end
        //                                            reading its own slot back)
        // ResolveFuture r3 <- r2                   (unresolved: forces the
        //                                            block, then runs this same
        //                                            instruction again --
        //                                            `vm::force_and_reexecute` /
        //                                            `reexecute` -- and finds it
        //                                            resolved)
        // Return r3  =>  99
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::MakeClosure {
                    destination: 0,
                    parameters: Rc::new(vec![]),
                    code: constant_block(99),
                    copied: vec![],
                },
                Instruction::MakeFuture {
                    destination: 1,
                    closure: 0,
                },
                Instruction::InitializeSlotProvisionally {
                    source: 1,
                    own_index: 0,
                },
                Instruction::LoadOwnSlot {
                    destination: 2,
                    own_index: 0,
                },
                Instruction::ResolveFuture {
                    destination: 3,
                    source: 2,
                },
                Instruction::Return { register: 3 },
            ],
            argument_count: 0,
            register_count: 4,
            statement_program_counter: vec![(0, 0..6)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "probe", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = expect_ok(send(&mut interpreter, &instance, "probe", &[]), "probe");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        99
    );
    // Nothing in this body replaces the future `InitializeSlotProvisionally`
    // stored in `n` with its value through a checked `InitializeSlot`, as a
    // clause's end does, but forcing left it resolved, not stuck resolving:
    // the `Reexecute` this test's `ResolveFuture` triggered ran the block
    // and recorded its answer. `n` holds that future still -- or, once a
    // collection has run (`NS_NEXT_STRESS_COLLECT` collects at every
    // safepoint), the value itself, since a collection replaces a resolved
    // future in an object field (`docs/design/memory.md`, "A resolved future
    // is traced as its value").
    interpreter.enter(|mutator| {
        let Some(receiver) = mutator.fetch(&instance).as_object() else {
            panic!("expected an instance");
        };
        let field = mutator
            .heap()
            .instance_field(receiver, 0)
            .expect("X declares one slot");
        match mutator.as_future(field) {
            Some(future) => assert!(matches!(
                mutator.future_state(future),
                ns_runtime::interpreter::future::FutureState::Resolved(resolved)
                    if resolved.as_small_integer() == Some(99)
            )),
            None => assert!(
                field.as_small_integer() == Some(99),
                "n holds the future InitializeSlotProvisionally stored, or its value"
            ),
        }
    });
}

#[test]
fn resolve_future_answers_a_non_future_value_unchanged() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public probe = ( ^0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // LoadLiteral r0 <- 5 (not a future); ResolveFuture r1 <- r0
        // (answers r0 unchanged, no forcing); Return r1
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: Literal::SmallInteger(5),
                },
                Instruction::ResolveFuture {
                    destination: 1,
                    source: 0,
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 0,
            register_count: 2,
            statement_program_counter: vec![(0, 0..3)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "probe", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = expect_ok(send(&mut interpreter, &instance, "probe", &[]), "probe");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        5
    );
}

#[test]
fn load_own_slot_answers_nil_for_an_unfilled_slot() {
    let source = indoc! {"
        class X = Object new (
          | n |
        ) (
          public probe = ( ^0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // LoadOwnSlot r0 <- own 0 (n, never initialized: nil); Return r0
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadOwnSlot {
                    destination: 0,
                    own_index: 0,
                },
                Instruction::Return { register: 0 },
            ],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "probe", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = expect_ok(send(&mut interpreter, &instance, "probe", &[]), "probe");
    assert!(interpreter.enter(|mutator| mutator.fetch(&result).is_nil()));
}

/// `MakeFuture`'s operand held something other than a closure: a crafted
/// image could put anything there, and an image must never panic --
/// `interpreter::future::not_a_closure` is the ordinary, catchable error
/// this instruction raises instead of the `unreachable!` a compiler-emitted
/// `MakeFuture` (whose operand is always the block `MakeClosure` just made)
/// would never trigger.
#[test]
fn make_future_of_a_non_closure_is_an_error_not_a_panic() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public bad = ( ^0 )
          public good = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // LoadLiteral r0 <- 5 (not a closure); MakeFuture r1 <- future of r0;
        // Return r1
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: Literal::SmallInteger(5),
                },
                Instruction::MakeFuture {
                    destination: 1,
                    closure: 0,
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 0,
            register_count: 2,
            statement_program_counter: vec![(0, 0..3)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "bad", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let result = send(&mut interpreter, &instance, "bad", &[]);
    let error = expect_error(&mut interpreter, result, "bad");
    assert!(
        error.message.contains("MakeFuture needs a block"),
        "{}",
        error.message
    );
    // The activation stack unwound to a clean floor rather than aborting the
    // process: the same interpreter, on the same instance, still answers an
    // unrelated send.
    let result = expect_ok(send(&mut interpreter, &instance, "good", &[]), "good");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        1
    );
}

/// How a test corrupts a future's two slots, as crafted code can once the
/// future exists: a decoded frame may carry the future as its receiver, and
/// `InitializeSlot` stores into `self` without a class check.
#[derive(Clone, Copy)]
enum Corruption {
    /// `computation` a small integer, the state still "unresolved".
    ComputationNoObject,
    /// `computation` an object that is no closure, the state still
    /// "unresolved".
    ComputationNoClosure,
    /// The state none of the three, the closure intact.
    StateNoneOfTheThree,
    /// Two futures "resolved", each to the other.
    ResolvedToEachOther,
}

/// A future of `block`'s closure, its fields corrupted as `corruption`
/// says, stored into `holder`'s slot 0.
fn a_corrupted_future(
    interpreter: &mut Interpreter,
    holder: &Root,
    block: &Root,
    corruption: Corruption,
) -> Root {
    interpreter.enter(|mutator| {
        let Some(closure) = mutator.fetch(block).as_object() else {
            panic!("block answers a block");
        };
        let Some(receiver) = mutator.fetch(holder).as_object() else {
            panic!("an instance");
        };
        let future = mutator.new_future(closure);
        let heap = mutator.heap_mut();
        match corruption {
            Corruption::ComputationNoObject => {
                heap.set_instance_field(future, 0, Value::small_integer(5));
            }
            Corruption::ComputationNoClosure => {
                heap.set_instance_field(future, 0, Value::object(receiver));
            }
            Corruption::StateNoneOfTheThree => {
                heap.set_instance_field(future, 1, Value::small_integer(7));
            }
            Corruption::ResolvedToEachOther => {
                let other = mutator.new_future(closure);
                let heap = mutator.heap_mut();
                heap.set_instance_field(future, 0, Value::object(other));
                heap.set_instance_field(future, 1, Value::small_integer(2));
                heap.set_instance_field(other, 0, Value::object(future));
                heap.set_instance_field(other, 1, Value::small_integer(2));
            }
        }
        mutator
            .heap_mut()
            .set_instance_field(receiver, 0, Value::object(future));
        mutator.stash(Value::object(future))
    })
}

/// A future whose fields no forcing wrote -- corrupted by crafted code --
/// is an ordinary, catchable error wherever it is forced: by
/// `ResolveFuture`, by a message to it (its `doesNotUnderstand:`), and as
/// the answer of another future's block (`Mutator::future_computed`). Never
/// a panic, and never a recursion without end through futures resolved to
/// each other.
#[test]
fn forcing_a_future_with_corrupted_fields_is_an_error_not_a_panic() {
    let source = indoc! {"
        class X = Object new (
          | public held |
        ) (
          public block = ( ^[3] )
          public answeringHeld = ( ^[held] )
          public probe = ( ^0 )
          public good = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // LoadOwnSlot r0 <- own 0 (held, the corrupted future);
        // ResolveFuture r1 <- r0; Return r1
        let body = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadOwnSlot {
                    destination: 0,
                    own_index: 0,
                },
                Instruction::ResolveFuture {
                    destination: 1,
                    source: 0,
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 0,
            register_count: 2,
            statement_program_counter: vec![(0, 0..3)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "probe", Rc::new(body));
        install_mixin(mutator, class, doctored);
    });
    let block = expect_ok(send(&mut interpreter, &instance, "block", &[]), "block");
    for corruption in [
        Corruption::ComputationNoObject,
        Corruption::ComputationNoClosure,
        Corruption::StateNoneOfTheThree,
        Corruption::ResolvedToEachOther,
    ] {
        let future = a_corrupted_future(&mut interpreter, &instance, &block, corruption);
        let result = send(&mut interpreter, &instance, "probe", &[]);
        let error = expect_error(&mut interpreter, result, "ResolveFuture");
        assert!(
            error.message.contains("malformed future"),
            "{}",
            error.message
        );
        let one = interpreter.stash(Value::small_integer(1));
        let result = send(&mut interpreter, &future, "+", &[one]);
        let error = expect_error(&mut interpreter, result, "a message to it");
        assert!(
            error.message.contains("malformed future"),
            "{}",
            error.message
        );
        // A well-formed future whose block answers the corrupted one.
        let answering = expect_ok(
            send(&mut interpreter, &instance, "answeringHeld", &[]),
            "answeringHeld",
        );
        let outer = interpreter.enter(|mutator| {
            let Some(closure) = mutator.fetch(&answering).as_object() else {
                panic!("answeringHeld answers a block");
            };
            let future = mutator.new_future(closure);
            mutator.stash(Value::object(future))
        });
        let one = interpreter.stash(Value::small_integer(1));
        let result = send(&mut interpreter, &outer, "+", &[one]);
        let error = expect_error(&mut interpreter, result, "a block answering it");
        assert!(
            error.message.contains("malformed future"),
            "{}",
            error.message
        );
        // The outer future was abandoned, not left resolving.
        interpreter.enter(|mutator| {
            let Some(outer) = mutator.fetch(&outer).as_object() else {
                panic!("a future is an object");
            };
            assert!(matches!(
                mutator.future_state(outer),
                ns_runtime::interpreter::future::FutureState::Unresolved(_)
            ));
        });
    }
    // The interpreter goes on.
    let result = expect_ok(send(&mut interpreter, &instance, "good", &[]), "good");
    assert_eq!(
        interpreter.enter(|mutator| integer_of(mutator.fetch(&result))),
        1
    );
}

/// The inlined integer leaves (`Instruction::IntegerBinary`). Each one is
/// checked through compiled source rather than a hand-built code object, so
/// the test survives a change to the lowering, and each is checked against
/// the literal answer the operation is known to produce. The goldens, run
/// under both lowerings, are what checks the fast path against the ordinary
/// send.
#[test]
fn an_inlined_integer_operation_answers_what_the_primitive_answers() {
    let source = indoc! {"
        class Arithmetic = Object new (
        ) (
          public add: left to: right = ( ^left + right )
          public subtract: left from: right = ( ^right - left )
          public multiply: left by: right = ( ^left * right )
          public divide: left by: right = ( ^left // right )
          public less: left than: right = ( ^left < right )
          public greater: left than: right = ( ^left > right )
          public atMost: left of: right = ( ^left <= right )
          public atLeast: left of: right = ( ^left >= right )
          public equal: left to: right = ( ^left = right )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let seven = interpreter.stash(Value::small_integer(7));
    let three = interpreter.stash(Value::small_integer(3));

    let integer_answer = |interpreter: &mut Interpreter, selector: &str| {
        let result = send(
            interpreter,
            &instance,
            selector,
            &[seven.clone(), three.clone()],
        );
        let root = expect_ok(result, selector);
        interpreter.enter(|mutator| integer_of(mutator.fetch(&root)))
    };
    assert_eq!(integer_answer(&mut interpreter, "add:to:"), 10);
    assert_eq!(integer_answer(&mut interpreter, "subtract:from:"), -4);
    assert_eq!(integer_answer(&mut interpreter, "multiply:by:"), 21);
    assert_eq!(integer_answer(&mut interpreter, "divide:by:"), 2);

    let boolean_answer = |interpreter: &mut Interpreter, selector: &str| {
        let result = send(
            interpreter,
            &instance,
            selector,
            &[seven.clone(), three.clone()],
        );
        let root = expect_ok(result, selector);
        interpreter.enter(|mutator| match mutator.fetch(&root).kind() {
            ValueKind::Boolean(boolean) => boolean,
            _ => panic!(
                "expected a Boolean, got {}",
                mutator.heap().describe(mutator.fetch(&root))
            ),
        })
    };
    assert!(!boolean_answer(&mut interpreter, "less:than:"));
    assert!(boolean_answer(&mut interpreter, "greater:than:"));
    assert!(!boolean_answer(&mut interpreter, "atMost:of:"));
    assert!(boolean_answer(&mut interpreter, "atLeast:of:"));
    assert!(!boolean_answer(&mut interpreter, "equal:to:"));
}

/// The three cases the fast path declines although both operands are
/// integers. Each must reach the ordinary send standing at the guard's
/// target and get that primitive's own answer, unchanged -- a
/// `LargeInteger` for a result that does not fit (§5.1.1), the primitive's
/// error for a zero divisor -- since `vm.rs` neither promotes nor raises.
#[test]
fn an_inlined_integer_operation_leaves_promotion_and_division_by_zero_to_the_primitive() {
    let source = indoc! {"
        class Arithmetic = Object new (
        ) (
          public add: left to: right = ( ^left + right )
          public divide: left by: right = ( ^left // right )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);

    let maximum = interpreter.stash(Value::small_integer(SMALL_INTEGER_MAX));
    let one = interpreter.stash(Value::small_integer(1));
    let result = send(
        &mut interpreter,
        &instance,
        "add:to:",
        &[maximum, one.clone()],
    );
    let sum = expect_ok(result, "overflowing addition");
    assert_eq!(
        large_integer_text(&mut interpreter, &sum),
        "1152921504606846976"
    );

    let zero = interpreter.stash(Value::small_integer(0));
    let result = send(&mut interpreter, &instance, "divide:by:", &[one, zero]);
    let error = expect_error(&mut interpreter, result, "division by zero");
    assert_eq!(error.message, "division by zero");

    // `SMALL_INTEGER_MIN // -1` has no `SmallInteger` result either, and
    // the inlined operation declines it as it declines a zero divisor.
    let minimum = interpreter.stash(Value::small_integer(SMALL_INTEGER_MIN));
    let minus_one = interpreter.stash(Value::small_integer(-1));
    let result = send(
        &mut interpreter,
        &instance,
        "divide:by:",
        &[minimum, minus_one],
    );
    let quotient = expect_ok(result, "overflowing division");
    assert_eq!(
        large_integer_text(&mut interpreter, &quotient),
        "1152921504606846976"
    );
}

/// The decimal text of the `LargeInteger` `root` holds, read off the heap so
/// that a `SmallInteger` or any other object fails rather than printing.
fn large_integer_text(interpreter: &mut Interpreter, root: &Root) -> String {
    interpreter.enter(|mutator| {
        let Some(object) = mutator.fetch(root).as_object() else {
            panic!("expected a LargeInteger object")
        };
        mutator
            .heap()
            .large_integer(object)
            .unwrap_or_else(|| panic!("expected ObjectData::LargeInteger"))
            .to_decimal_string()
    })
}

/// A non-integer operand is the guard's other failure, and the primitive
/// decides it too: `+` is a type mismatch, while `=` deliberately answers
/// `false` rather than raising (`builtins/numbers.rs`'s module note). The
/// fast path must reproduce neither decision itself.
#[test]
fn an_inlined_integer_operation_with_a_non_integer_operand_falls_back_to_the_send() {
    let source = indoc! {"
        class Arithmetic = Object new (
        ) (
          public add: left to: right = ( ^left + right )
          public equal: left to: right = ( ^left = right )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let one = interpreter.stash(Value::small_integer(1));
    let text = interpreter.enter(|mutator| {
        let string = mutator.new_string("one".to_string());
        mutator.stash(Value::object(string))
    });

    let result = send(
        &mut interpreter,
        &instance,
        "add:to:",
        &[one.clone(), text.clone()],
    );
    let error = expect_error(&mut interpreter, result, "a String argument to `+`");
    assert!(
        error.message.contains("requires a numeric argument"),
        "the primitive's own type-mismatch message, unchanged: {}",
        error.message
    );

    let result = send(&mut interpreter, &instance, "equal:to:", &[one, text]);
    let answer = expect_ok(result, "1 = 'one'");
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::Boolean(false) => {}
        _ => panic!(
            "expected `false` from the primitive, got {}",
            mutator.heap().describe(mutator.fetch(&answer))
        ),
    });
}

/// The fallback reads the registers the guard already filled, so the
/// argument's own side effects happen once whichever path runs. Lowering the
/// message a second time for the fallback would run `tick` twice here --
/// silent wrong output at exit status 0.
#[test]
fn an_inlined_integer_operation_evaluates_its_argument_once_on_the_fallback_path() {
    let source = indoc! {"
        class Counting = Object new (
          | ticks ::= 0. |
        ) (
          tick = ( ticks:: ticks + 1. ^'not an integer' )
          public run = ( 1 = self tick. ^ticks )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = new_instance(&mut interpreter, &class);
    let result = send(&mut interpreter, &instance, "run", &[]);
    let ticks = expect_ok(result, "run");
    interpreter.enter(|mutator| assert_eq!(integer_of(mutator.fetch(&ticks)), 1));
}

/// Every switch to another activation is a safepoint: with one requested at
/// each, a send and its return reach at least two, and the run carries on
/// where it stood after each -- the callee after its push, the caller after
/// the return (`docs/design/execution.md`, "The loop over locals").
#[test]
#[cfg(debug_assertions)]
fn a_stretch_leaves_for_a_requested_safepoint_and_resumes_where_it_stood() {
    let (mut interpreter, class) = load(indoc! {"
        class Probe = Object new () (
            public caller = ( ^self callee + 1 )
            public callee = ( ^41 )
        ) : ()
    "});
    let probe = new_instance(&mut interpreter, &class);
    interpreter.set_stress_moves(true);
    let answer = expect_ok(send(&mut interpreter, &probe, "caller", &[]), "caller");
    let safepoints = interpreter.stress_safepoints();
    interpreter.set_stress_moves(false);
    interpreter.enter(|mutator| assert_eq!(integer_of(mutator.fetch(&answer)), 42));
    // 2 switches: the push of callee, and the return to caller. No jump in
    // either method's body, forward or backward, so a safepoint wrongly
    // taken on a forward jump would show up here as a count above 2.
    assert_eq!(safepoints, 2);
}

/// A loop that sends nothing -- the optimized lowering inlines
/// `whileTrue:`, so its body is `IntegerBinary`s and jumps -- still reaches
/// a safepoint at every backward jump.
#[test]
#[cfg(debug_assertions)]
fn a_loop_that_sends_nothing_still_reaches_a_safepoint() {
    let (mut interpreter, class) = load(indoc! {"
        class Probe = Object new () (
            public count = (
                | index ::= 0. |
                [index < 1000] whileTrue: [index:: index + 1].
                ^index
            )
        ) : ()
    "});
    let probe = new_instance(&mut interpreter, &class);
    interpreter.set_stress_moves(true);
    let answer = expect_ok(send(&mut interpreter, &probe, "count", &[]), "count");
    let safepoints = interpreter.stress_safepoints();
    interpreter.set_stress_moves(false);
    interpreter.enter(|mutator| assert_eq!(integer_of(mutator.fetch(&answer)), 1000));
    // 1000 backward jumps, one per iteration, and no switches: the whole
    // loop is inlined, so nothing here ever sends. The loop's exit is a
    // forward jump; a safepoint wrongly taken there too would show up as a
    // count above 1000.
    assert_eq!(safepoints, 1000);
}
