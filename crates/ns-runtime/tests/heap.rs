//! The heap seam's own tests. Everything here is about the accessor
//! contract, not about what sits behind it -- which is the point of the
//! seam.

use indoc::indoc;
use ns_runtime::bytecode::{CodeKind, CompiledCode, Instruction};
use ns_runtime::interpreter::FeedbackBase;
use ns_runtime::types::{Continuation, FrameKind, Value};
use ns_runtime::Interpreter;
use ns_runtime::ValueKind;
use ns_runtime::{
    ActivationHeader, BuiltinClass, DispatchId, HostResult, HostSignal, Mutator, Root, Symbol,
};
use ns_runtime::{CodePoints, ObjectShape};

#[test]
fn array_accessors_read_and_write_through_the_seam() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![Value::small_integer(1), Value::small_integer(2)]);

        assert_eq!(mutator.heap().array_element_count(object), Some(2));
        assert!(
            mutator
                .heap()
                .array_element(object, 0)
                .and_then(Value::as_small_integer)
                == Some(1)
        );

        assert!(mutator
            .heap_mut()
            .array_set_element(object, 0, Value::small_integer(9)));
        assert!(
            mutator
                .heap()
                .array_element(object, 0)
                .and_then(Value::as_small_integer)
                == Some(9)
        );
    });
}

#[test]
fn an_array_accessor_on_a_non_array_answers_none_rather_than_panicking() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_string("not an array".to_string());
        assert_eq!(mutator.heap().array_element_count(object), None);
        assert!(mutator.heap().array_element(object, 0).is_none());
    });
}

#[test]
fn an_out_of_range_index_answers_none_or_false_rather_than_panicking() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![Value::small_integer(1), Value::small_integer(2)]);

        assert!(mutator.heap().array_element(object, 99).is_none());
        assert!(!mutator
            .heap_mut()
            .array_set_element(object, 99, Value::small_integer(9)));
    });
}

#[test]
fn string_and_symbol_accessors_answer_only_for_their_own_shape() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let string = mutator.new_string("hello".to_string());
        assert_eq!(mutator.heap().str_value(string), Some("hello".to_string()));
        assert_eq!(mutator.heap().symbol_value(string), None);
    });
}

#[test]
fn is_class_distinguishes_a_class_object_from_an_instance() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let string = mutator.new_string("x".to_string());
        let class = mutator.class_of(Value::object(string));
        assert!(!mutator.heap().is_class(string));
        assert!(mutator.heap().is_class(class));
    });
}

/// `describe` is the only sanctioned way to put a `Value` into an error
/// message (never `{:?}` -- see `docs/design/memory.md`'s "The seam" and
/// `heap.rs`'s doc comment on the method). Each shape must produce its own
/// distinct, non-empty text, or two different failures would read
/// identically to a user.
#[test]
fn describe_renders_a_distinct_non_empty_text_per_value_shape() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let string_value = Value::object(mutator.new_string("hi".to_string()));
        let array_value = Value::object(mutator.new_array(vec![Value::small_integer(1)]));

        let nil = mutator.heap().describe(Value::nil());
        let small_integer = mutator.heap().describe(Value::small_integer(42));
        let string = mutator.heap().describe(string_value);
        let array = mutator.heap().describe(array_value);

        let all = [&nil, &small_integer, &string, &array];
        for text in all {
            assert!(!text.is_empty(), "description must not be empty");
        }
        for (first_index, first) in all.iter().enumerate() {
            for (second_index, second) in all.iter().enumerate() {
                if first_index != second_index {
                    assert_ne!(first, second, "descriptions must be distinct per shape");
                }
            }
        }
    });
}

#[test]
fn describe_argument_none_names_the_absence_of_an_argument() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let text = mutator.heap().describe_argument(None);
        assert!(!text.is_empty());
        // Must not be confusable with a real value's description.
        assert_ne!(text, mutator.heap().describe(Value::nil()));
    });
}

#[test]
fn describe_argument_some_matches_describe_of_the_same_value() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let value = Value::small_integer(7);
        assert_eq!(
            mutator.heap().describe_argument(Some(value)),
            mutator.heap().describe(value)
        );
    });
}

#[test]
fn class_accessors_answer_none_for_an_instance() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let string = mutator.new_string("x".to_string());
        assert!(mutator.heap().class_application(string).is_none());
        assert!(mutator.heap().class_superclass(string).is_none());
        assert!(mutator.heap().class_builtin(string).is_none());
    });
}

#[test]
fn a_builtin_class_reports_its_kind_through_the_seam() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let string = mutator.new_string("x".to_string());
        let class = mutator.class_of(Value::object(string));
        assert!(mutator.heap().class_builtin(class).is_some());
    });
}

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

#[test]
fn register_addressing_puts_arguments_before_temporaries() {
    // Exercised through a real activation rather than a hand-built frame:
    // the argument/temporary boundary is exactly what the accessor
    // encodes, and a synthetic frame could encode it wrongly and then
    // agree with itself.
    let source = indoc! {"
        class T usingPlatform: p = Object new () (
            public run: a = (| t | t:: a. ^t)
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    // Computed ahead of the call: `send` takes `&mut Interpreter`, and so
    // does `platform()`, so the two cannot be nested in one expression.
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "instantiate",
    );
    let result = expect_ok(
        interpreter.send(
            &instance,
            Symbol::intern("run:"),
            &[interpreter.stash(Value::small_integer(41))],
        ),
        "run:",
    );
    assert_eq!(
        interpreter.enter(|mutator| mutator.fetch(&result).as_small_integer()),
        Some(41)
    );
}

/// Two `Inner` class objects of two `Outer` instances share one application,
/// and so one dispatch id for their instances and one for their class side,
/// while the class objects themselves differ (`docs/design/dispatch.md`, "A
/// cache guards on the application, not on the class object").
#[test]
fn instances_of_a_nested_class_share_a_dispatch_id_across_enclosing_objects() {
    let source = indoc! {"
        class Subject = Object new (
        ) (
          public class Outer = Object new () (
            public class Inner = Object new () ()
          )
          public class Other = Object new () ()
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let root = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let mut make_inner = || {
        let outer_class = expect_ok(
            interpreter.send(&root, Symbol::intern("Outer"), &[]),
            "Outer",
        );
        let outer = expect_ok(
            interpreter.send(&outer_class, Symbol::NEW, &[]),
            "Outer new",
        );
        let inner_class = expect_ok(
            interpreter.send(&outer, Symbol::intern("Inner"), &[]),
            "Inner",
        );
        let inner = expect_ok(
            interpreter.send(&inner_class, Symbol::NEW, &[]),
            "Inner new",
        );
        (inner_class, inner)
    };
    let (first_class, first) = make_inner();
    let (second_class, second) = make_inner();
    let other_class = expect_ok(
        interpreter.send(&root, Symbol::intern("Other"), &[]),
        "Other",
    );
    let other = expect_ok(
        interpreter.send(&other_class, Symbol::NEW, &[]),
        "Other new",
    );
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let id = |root: &Root| heap.dispatch_id(mutator.fetch(root));
        assert_eq!(id(&first), id(&second), "one application, one instance id");
        assert_eq!(
            id(&first_class),
            id(&second_class),
            "one metaclass application, one class object id"
        );
        assert_ne!(
            id(&first),
            id(&first_class),
            "a class and its metaclass are different applications"
        );
        assert_ne!(id(&first), id(&other), "another application");
        assert_ne!(id(&first), DispatchId::NONE);
    });
}

/// Every immediate kind answers its builtin class's instance id, `true` and
/// `false` two different ones, since they are instances of two classes.
#[test]
fn immediates_answer_their_builtin_classes_instance_ids() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let expected = |mutator: &Mutator, kind: BuiltinClass| {
            mutator
                .heap()
                .class_dispatch_ids(mutator.builtin_class(kind))
                .instances
        };
        let float = mutator.new_float(2.5);
        let heap = mutator.heap();
        assert_eq!(
            heap.dispatch_id(Value::nil()),
            expected(mutator, BuiltinClass::UndefinedObject)
        );
        assert_eq!(
            heap.dispatch_id(Value::boolean(true)),
            expected(mutator, BuiltinClass::True)
        );
        assert_eq!(
            heap.dispatch_id(Value::boolean(false)),
            expected(mutator, BuiltinClass::False)
        );
        assert_eq!(
            heap.dispatch_id(Value::small_integer(3)),
            expected(mutator, BuiltinClass::Integer)
        );
        assert_eq!(
            heap.dispatch_id(float),
            expected(mutator, BuiltinClass::Float)
        );
        assert_ne!(
            expected(mutator, BuiltinClass::True),
            expected(mutator, BuiltinClass::False)
        );
        let array = mutator.new_array(vec![]);
        assert_eq!(
            mutator.heap().dispatch_id(Value::object(array)),
            expected(mutator, BuiltinClass::Array)
        );
    });
}

/// A class given a new application -- an edit to a live class, the shape
/// `install_mixin` in `tests/image.rs` takes -- moves its live instances to
/// the new application's id with it. With the id copied into every object
/// header (`docs/design/dispatch.md`, "3c"), an instance keeping the old copy
/// would go on hitting the old application's cache entries.
#[test]
fn a_class_given_a_new_application_moves_its_live_instances_with_it() {
    let source = indoc! {"
        class Subject = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    interpreter.enter(|mutator| {
        let Some(class_object) = mutator.fetch(&class).as_object() else {
            panic!("expected a class object")
        };
        let before = mutator.heap().class_dispatch_ids(class_object);
        let class_object_id = mutator.heap().dispatch_id(Value::object(class_object));
        let application = mutator
            .heap()
            .class_application(class_object)
            .unwrap_or_else(|| panic!("expected a class object"));
        // A copy of the mixin, not the mixin itself: `apply_mixin` answers
        // its cached application for a (mixin, superclass application) pair
        // it has seen, and a copy is another key -- an edited class.
        let edited = std::rc::Rc::new((*application.mixin).clone());
        let reapplied = mutator.apply_mixin(&edited, application.superclass.as_ref());
        assert!(mutator
            .heap_mut()
            .class_set_application(class_object, reapplied));
        let after = mutator.heap().class_dispatch_ids(class_object);
        assert_ne!(before, after, "a new application mints new ids");
        let instance = mutator.fetch(&instance);
        assert_eq!(mutator.heap().dispatch_id(instance), after.instances);
        // The class object is an instance of its metaclass, not of itself,
        // so its own id is its metaclass's and does not move.
        assert_eq!(
            mutator.heap().dispatch_id(Value::object(class_object)),
            class_object_id
        );
        let metaclass = mutator
            .metaclass_of(class_object)
            .expect("every class object has a metaclass");
        assert_eq!(
            class_object_id,
            mutator.heap().class_dispatch_ids(metaclass).instances
        );
    });
}

/// A body that returns its first register; nothing here runs it.
fn trivial_code() -> std::rc::Rc<CompiledCode> {
    std::rc::Rc::new(CompiledCode::new(
        CodeKind::Method,
        vec![Instruction::Return { register: 0 }],
        1,
        2,
        0,
        Vec::new(),
        ns_syntax::Span::new(0, 0),
    ))
}

/// An initializer activation of `class` running `trivial_code`: a frame
/// reified from it names its code, so it knows its one argument.
fn activation_header<'gc>(class: ns_runtime::ClassReference<'gc>) -> ActivationHeader<'gc> {
    ActivationHeader {
        receiver: Value::nil(),
        current_class: Some(class),
        kind: FrameKind::Initializer(class),
        feedback_base: FeedbackBase::NOT_ASSIGNED,
        code: trivial_code(),
        then: Continuation::Deliver,
        counted: true,
    }
}

#[test]
fn a_returning_activation_keeps_its_registers_only_when_it_was_marked() {
    // A frame reified for a closure that reads nothing by reference needs no
    // registers once its activation has returned: what the closure captured
    // it carries itself, so its own stay nil. One that was marked -- a
    // reference-lowering block, or an optimized fallback closure -- still
    // keeps them.
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Object);
        let heap = mutator.heap_mut();

        let unmarked =
            heap.push_activation(activation_header(class), &[Value::small_integer(1)], 1);
        heap.activation_set_program_counter(unmarked, 0);
        let forgetful = heap.reify(unmarked);
        heap.activation_set_local(unmarked, 1, Value::small_integer(2));
        heap.pop_activation(unmarked);
        assert_eq!(
            heap.frame_register_shape(forgetful),
            (0, 1, 2),
            "a frame has its code's registers whether or not they were kept"
        );
        assert!(
            heap.frame_local(forgetful, 0).is_nil() && heap.frame_local(forgetful, 1).is_nil(),
            "an unmarked activation leaves its frame's registers nil"
        );
        // A crafted image can pair a by-reference closure with a frame that
        // has fewer registers, so a read past the file answers nil rather
        // than aborting the process.
        assert!(heap.frame_local(forgetful, 2).is_nil());
        heap.frame_set_local(forgetful, 2, Value::small_integer(9));
        assert!(heap.frame_local(forgetful, 2).is_nil());

        let marked = heap.push_activation(activation_header(class), &[Value::small_integer(1)], 1);
        heap.activation_set_program_counter(marked, 0);
        let kept = heap.reify(marked);
        // What `MakeClosure` does for a block whose `outer_reach` is
        // non-zero.
        heap.retain_registers(marked);
        heap.activation_set_local(marked, 1, Value::small_integer(2));
        heap.pop_activation(marked);
        assert_eq!(
            heap.frame_register_shape(kept),
            (0, 1, 2),
            "a marked activation hands its registers to its frame"
        );
        assert_eq!(heap.frame_local(kept, 0).as_small_integer(), Some(1));
        assert_eq!(heap.frame_local(kept, 1).as_small_integer(), Some(2));
    });
}

#[test]
fn a_float_is_made_by_the_mutator_and_read_by_the_heap() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let value = mutator.new_float(2.5);
        assert_eq!(mutator.heap().float_value(value), Some(2.5));
        assert_eq!(mutator.heap().float_value(Value::small_integer(2)), None);
        assert!(matches!(value.kind(), ValueKind::SmallFloat(number) if number == 2.5));
    });
}

#[test]
fn every_float_reads_back_with_its_own_bits_whether_immediate_or_boxed() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        // (number, whether it is boxed)
        for (number, boxed) in [
            (0.0, false),
            (-0.0, false),
            (1.5, false),
            (1.0e38, false),
            (f64::from_bits((896 << 52) | 1), false),
            (f64::from_bits(896 << 52), true),
            (1.0e300, true),
            (-1.0e300, true),
            (1.0e-300, true),
            (5.0e-324, true),
            (f64::NAN, true),
            (f64::INFINITY, true),
            (f64::NEG_INFINITY, true),
            (f64::MAX, true),
            (f64::MIN_POSITIVE, true),
        ] {
            let value = mutator.new_float(number);
            let back = mutator.heap().float_value(value).map(f64::to_bits);
            assert_eq!(back, Some(number.to_bits()), "{number}");
            assert_eq!(value.as_object().is_some(), boxed, "{number}");
        }
    });
}

#[test]
fn an_object_handle_survives_the_value_word() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_array(vec![Value::small_integer(1), Value::nil()]);
        let value = Value::object(object);
        let back = value
            .as_object()
            .expect("an object value answers its handle");
        assert!(back.same(object));
        assert_eq!(
            mutator.heap().describe(value),
            mutator.heap().describe(Value::object(back))
        );
    });
}

#[test]
fn every_kind_of_object_answers_its_shape() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let instance = mutator.heap_mut().allocate_instance(object_class, 2);
        assert_eq!(mutator.heap().object_shape(instance), ObjectShape::Instance);
        assert_eq!(
            mutator
                .heap()
                .instance_fields(instance)
                .map(|fields| fields.len()),
            Some(2)
        );
        let text = mutator.new_string("abc".to_string());
        assert_eq!(mutator.heap().object_shape(text), ObjectShape::String);
        assert_eq!(
            mutator.heap().string_code_points(text),
            Some(CodePoints::from("abc"))
        );
        let symbol = mutator.new_symbol(Symbol::intern("abc"));
        assert_eq!(
            mutator
                .heap()
                .object_shape(symbol.as_object().expect("an object")),
            ObjectShape::Symbol
        );
        assert_eq!(
            mutator.heap().object_shape(object_class),
            ObjectShape::Class
        );
    });
}
