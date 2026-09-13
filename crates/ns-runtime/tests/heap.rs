//! The heap seam's own tests. Everything here is about the accessor
//! contract, not about what sits behind it -- which is the point of the
//! seam.

use indoc::indoc;
use ns_runtime::types::Value;
use ns_runtime::Interpreter;
use ns_runtime::{HostResult, HostSignal, Root, Symbol};

#[test]
fn array_accessors_read_and_write_through_the_seam() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let obj = m.new_array(vec![Value::SmallInt(1), Value::SmallInt(2)]);

        assert_eq!(m.heap().array_len(obj), Some(2));
        assert!(matches!(
            m.heap().array_at(obj, 0),
            Some(Value::SmallInt(1))
        ));

        assert!(m.heap_mut().array_set(obj, 0, Value::SmallInt(9)));
        assert!(matches!(
            m.heap().array_at(obj, 0),
            Some(Value::SmallInt(9))
        ));

        assert!(m.heap_mut().array_push(obj, Value::SmallInt(3)));
        assert_eq!(m.heap().array_len(obj), Some(3));
    });
}

#[test]
fn an_array_accessor_on_a_non_array_answers_none_rather_than_panicking() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let obj = m.new_string("not an array".to_string());
        assert_eq!(m.heap().array_len(obj), None);
        assert!(m.heap().array_at(obj, 0).is_none());
        assert!(!m.heap_mut().array_push(obj, Value::Nil));
    });
}

#[test]
fn an_out_of_range_index_answers_none_or_false_rather_than_panicking() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let obj = m.new_array(vec![Value::SmallInt(1), Value::SmallInt(2)]);

        assert!(m.heap().array_at(obj, 99).is_none());
        assert!(!m.heap_mut().array_set(obj, 99, Value::SmallInt(9)));
    });
}

#[test]
fn string_and_symbol_accessors_answer_only_for_their_own_shape() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let s = m.new_string("hello".to_string());
        assert_eq!(m.heap().str_value(s), Some("hello".to_string()));
        assert_eq!(m.heap().sym_value(s), None);
    });
}

#[test]
fn is_class_distinguishes_a_class_object_from_an_instance() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let s = m.new_string("x".to_string());
        let class = m.class_of(&Value::Obj(s));
        assert!(!m.heap().is_class(s));
        assert!(m.heap().is_class(class));
    });
}

/// `describe` is the only sanctioned way to put a `Value` into an error
/// message (never `{:?}` -- see `docs/design/memory.md`'s "The seam" and
/// `heap.rs`'s doc comment on the method). Each shape must produce its own
/// distinct, non-empty text, or two different failures would read
/// identically to a user.
#[test]
fn describe_renders_a_distinct_non_empty_text_per_value_shape() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let s = Value::Obj(m.new_string("hi".to_string()));
        let arr = Value::Obj(m.new_array(vec![Value::SmallInt(1)]));

        let nil = m.heap().describe(&Value::Nil);
        let small_int = m.heap().describe(&Value::SmallInt(42));
        let string = m.heap().describe(&s);
        let array = m.heap().describe(&arr);

        let all = [&nil, &small_int, &string, &array];
        for text in all {
            assert!(!text.is_empty(), "description must not be empty");
        }
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "descriptions must be distinct per shape");
                }
            }
        }
    });
}

#[test]
fn describe_arg_none_names_the_absence_of_an_argument() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let text = m.heap().describe_arg(None);
        assert!(!text.is_empty());
        // Must not be confusable with a real value's description.
        assert_ne!(text, m.heap().describe(&Value::Nil));
    });
}

#[test]
fn describe_arg_some_matches_describe_of_the_same_value() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let v = Value::SmallInt(7);
        assert_eq!(m.heap().describe_arg(Some(&v)), m.heap().describe(&v));
    });
}

#[test]
fn class_accessors_answer_none_for_an_instance() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let s = m.new_string("x".to_string());
        assert!(m.heap().class_application(s).is_none());
        assert!(m.heap().class_superclass(s).is_none());
        assert!(m.heap().class_builtin(s).is_none());
    });
}

#[test]
fn a_builtin_class_reports_its_kind_through_the_seam() {
    let mut interp = Interpreter::bootstrap();
    interp.enter(|m| {
        let s = m.new_string("x".to_string());
        let class = m.class_of(&Value::Obj(s));
        assert!(m.heap().class_builtin(class).is_some());
    });
}

fn load(src: &str) -> (Interpreter, Root) {
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

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

#[test]
fn register_addressing_puts_arguments_before_temporaries() {
    // Exercised through a real activation rather than a hand-built frame:
    // the argument/temporary boundary is exactly what the accessor
    // encodes, and a synthetic frame could encode it wrongly and then
    // agree with itself.
    let src = indoc! {"
        Newspeak3
        'T'
        class T usingPlatform: p = Object new () (
            public run: a = (| t | t := a. ^t)
        ) : ()
    "};
    let (mut interp, class) = load(src);
    // Computed ahead of the call: `send` takes `&mut Interpreter`, and so
    // does `platform()`, so the two cannot be nested in one expression.
    let platform = interp.platform();
    let instance = expect_ok(
        interp.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "instantiate",
    );
    let result = expect_ok(
        interp.send(
            &instance,
            Symbol::intern("run:"),
            &[interp.stash(Value::SmallInt(41))],
        ),
        "run:",
    );
    assert!(interp.enter(|m| matches!(m.fetch(&result), Value::SmallInt(41))));
}
