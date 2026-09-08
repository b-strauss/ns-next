//! The heap seam's own tests. Everything here is about the accessor
//! contract, not about what sits behind it -- which is the point of the
//! seam.

use ns_runtime::types::Value;
use ns_runtime::Interpreter;

#[test]
fn array_accessors_read_and_write_through_the_seam() {
    let mut interp = Interpreter::bootstrap();
    let obj = interp.new_array(vec![Value::SmallInt(1), Value::SmallInt(2)]);

    assert_eq!(interp.heap_ref().array_len(&obj), Some(2));
    assert!(matches!(
        interp.heap_ref().array_at(&obj, 0),
        Some(Value::SmallInt(1))
    ));

    assert!(interp.heap_mut().array_set(&obj, 0, Value::SmallInt(9)));
    assert!(matches!(
        interp.heap_ref().array_at(&obj, 0),
        Some(Value::SmallInt(9))
    ));

    assert!(interp.heap_mut().array_push(&obj, Value::SmallInt(3)));
    assert_eq!(interp.heap_ref().array_len(&obj), Some(3));
}

#[test]
fn an_array_accessor_on_a_non_array_answers_none_rather_than_panicking() {
    let mut interp = Interpreter::bootstrap();
    let obj = interp.new_string("not an array".to_string());
    assert_eq!(interp.heap_ref().array_len(&obj), None);
    assert!(interp.heap_ref().array_at(&obj, 0).is_none());
    assert!(!interp.heap_mut().array_push(&obj, Value::Nil));
}

#[test]
fn string_and_symbol_accessors_answer_only_for_their_own_shape() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("hello".to_string());
    assert_eq!(interp.heap_ref().str_value(&s), Some("hello".to_string()));
    assert_eq!(interp.heap_ref().sym_value(&s), None);
}

#[test]
fn is_class_distinguishes_a_class_object_from_an_instance() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("x".to_string());
    let class = interp.class_of(&Value::Obj(s.clone()));
    assert!(!interp.heap_ref().is_class(&s));
    assert!(interp.heap_ref().is_class(&class));
}
