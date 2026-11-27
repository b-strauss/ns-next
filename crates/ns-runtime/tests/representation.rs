//! What only the representation shows -- an integer's `LargeInteger`
//! class, a chain of metaclasses, the identity of class objects -- read on
//! the host side, where no capability rule binds: a program cannot ask
//! another object for its class (`docs/design/core-library.md`, "`class`
//! and the class accessors protected").

use indoc::{formatdoc, indoc};
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    BuiltinClass, Heap, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value,
    ValueKind,
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

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The message of the error `result` signals; a test that expects one
/// fails on an answer.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: answered, expected an error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
    }
}

fn as_string<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> String {
    match value.kind() {
        ValueKind::Object(object) => heap
            .str_value(object)
            .unwrap_or_else(|| panic!("expected a String, got {}", heap.describe(value))),
        _ => panic!("expected an object, got {}", heap.describe(value)),
    }
}

/// A fresh instance of `source`'s module, made with the platform.
fn instantiate(source: &str) -> (Interpreter, Root) {
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    (interpreter, instance)
}

/// What the unary `selector` answers, sent to `instance`.
fn answer(interpreter: &mut Interpreter, instance: &Root, selector: &str) -> Root {
    expect_ok(
        interpreter.send(instance, Symbol::intern(selector), &[]),
        selector,
    )
}

/// The name of the class of what `selector` answers, sent to a fresh
/// instance of `source`'s module.
fn class_name_of(source: &str, selector: &str) -> String {
    let (mut interpreter, instance) = instantiate(source);
    let answer = answer(&mut interpreter, &instance, selector);
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&answer);
        let class = mutator.class_of(value);
        mutator.heap().class_display_name(class)
    })
}

/// The name of the class of what `body`, a method body, answers.
fn class_name_of_body(body: &str) -> String {
    let source = formatdoc! {"
        class Probe usingPlatform: platform = () (
          public answer = (
            {body}
          )
        )
    "};
    class_name_of(&source, "answer")
}

/// Whether what `body`, a method body, answers is an instance of the
/// builtin class `kind` itself, not of a class of the same name.
fn answers_an_instance_of(body: &str, kind: BuiltinClass) -> bool {
    let source = formatdoc! {"
        class Probe usingPlatform: platform = () (
          public answer = (
            {body}
          )
        )
    "};
    let (mut interpreter, instance) = instantiate(&source);
    let answer = answer(&mut interpreter, &instance, "answer");
    interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(&answer));
        class.same(mutator.builtin_class(kind))
    })
}

/// A module whose `foo` and `bar` answer the class objects `Foo` and
/// `Bar`.
const FOO_AND_BAR: &str = indoc! {"
    class Probe usingPlatform: platform = () (
      class Foo = Object new () ()

      class Bar = Object new () ()

      public foo = (
        ^Foo.
      )

      public bar = (
        ^Bar.
      )
    )
"};

/// `goldens/language/class/class-side/a_class_and_its_metaclass_print_their_names.ns`:
/// a metaclass's class is `Metaclass`, which prints as `Metaclass`, and
/// its class prints as `Metaclass class`; `3`'s class prints as `Integer`
/// and its metaclass as `Integer class`.
#[test]
fn a_class_and_its_metaclass_print_their_names_along_the_chain() {
    let (mut interpreter, instance) = instantiate(FOO_AND_BAR);
    let foo = answer(&mut interpreter, &instance, "foo");
    interpreter.enter(|mutator| {
        let Some(foo) = mutator.fetch(&foo).as_object() else {
            panic!("`Foo` answers a class object");
        };
        let foo_class = mutator.class_of(Value::object(foo));
        let foo_class_class = mutator.class_of(Value::object(foo_class));
        let foo_class_class_class = mutator.class_of(Value::object(foo_class_class));
        assert!(foo_class_class.same(mutator.builtin_class(BuiltinClass::Metaclass)));
        let heap = mutator.heap();
        assert_eq!(heap.class_display_name(foo_class_class), "Metaclass");
        assert_eq!(
            heap.class_display_name(foo_class_class_class),
            "Metaclass class"
        );
        let three_class = mutator.class_of(Value::small_integer(3));
        let three_class_class = mutator.class_of(Value::object(three_class));
        let heap = mutator.heap();
        assert_eq!(heap.class_display_name(three_class), "Integer");
        assert_eq!(heap.class_display_name(three_class_class), "Integer class");
    });
}

/// `newspeak-spec.pdf` 3.2 makes every class "an instance of a unique
/// metaclass", which a second instance of a metaclass would break. A class
/// object is made only by evaluating a declaration, so `Metaclass`, the
/// class of `3`'s metaclass, refuses to be instantiated.
#[test]
fn a_class_object_cannot_be_made_by_instantiation() {
    let mut interpreter = Interpreter::bootstrap();
    let metaclass = interpreter.enter(|mutator| {
        let three_class = mutator.class_of(Value::small_integer(3));
        let three_metaclass = mutator.class_of(Value::object(three_class));
        let metaclass = mutator.class_of(Value::object(three_metaclass));
        assert!(metaclass.same(mutator.builtin_class(BuiltinClass::Metaclass)));
        mutator.stash(Value::object(metaclass))
    });
    let message = expect_error(
        interpreter.send(&metaclass, Symbol::intern("new"), &[]),
        "Metaclass new",
    );
    assert_eq!(
        message,
        "cannot instantiate Metaclass: a class object is made only by evaluating its declaration"
    );
}

/// `goldens/language/class/class-side/the_class_of_a_class_is_its_metaclass.ns`:
/// `newspeak-spec.pdf` 3.2, "All metaclasses are instances of class
/// Metaclass": two metaclasses have one class, a metaclass's class is the
/// class of `Metaclass`'s own metaclass, and `3`'s metaclass's class is
/// the same `Metaclass`.
// spec: vm.heap.class-word-is-metaclass
#[test]
fn the_class_of_a_class_is_its_metaclass_and_every_metaclass_is_a_metaclass() {
    let (mut interpreter, instance) = instantiate(FOO_AND_BAR);
    let foo = answer(&mut interpreter, &instance, "foo");
    let bar = answer(&mut interpreter, &instance, "bar");
    interpreter.enter(|mutator| {
        let class_of = |value| mutator.class_of(value);
        let foo_class_class = class_of(Value::object(class_of(mutator.fetch(&foo))));
        let bar_class_class = class_of(Value::object(class_of(mutator.fetch(&bar))));
        assert!(
            foo_class_class.same(bar_class_class),
            "`Foo class class == Bar class class`"
        );
        assert!(
            foo_class_class.same(mutator.builtin_class(BuiltinClass::Metaclass)),
            "`Foo class class == Metaclass`"
        );
        let foo_class_class_class_class =
            class_of(Value::object(class_of(Value::object(foo_class_class))));
        assert!(
            foo_class_class.same(foo_class_class_class_class),
            "`Foo class class == Foo class class class class`"
        );
        let three_class_class_class = class_of(Value::object(class_of(Value::object(class_of(
            Value::small_integer(3),
        )))));
        assert!(
            three_class_class_class.same(foo_class_class),
            "`3 class class class == Foo class class`"
        );
    });
}

/// `goldens/library/builtin-subclass/`: a user subclass of `LargeInteger`,
/// and `LargeInteger new`, inherit its methods but no integer payload, so
/// every `LargeInteger` leaf declines, then every `Integer` leaf, and the
/// send reaches `Object`'s: nothing takes such an instance for a number.
/// No program can reach `LargeInteger`, which only an integer's class is,
/// so the host hands the class to the module. The module is a value
/// because `MyLarge`, a subclass of `Value`, asks its enclosing object to
/// be one (§3.1.1).
#[test]
fn a_subclass_of_large_integer_is_no_number() {
    assert!(
        answers_an_instance_of("^9223372036854775807 + 1.", BuiltinClass::LargeInteger),
        "`9223372036854775807 + 1` answers a `LargeInteger`"
    );
    let source = indoc! {"
        class LargeFallThrough usingPlatform: platform largeInteger: largeInteger = Object <: Value (
          |
            largeIntegerClass = largeInteger.
          |
        ) (
          class MyLarge = largeIntegerClass new () ()

          report: instance = (
            ^(instance = instance) printString , ' ' , instance printString.
          )

          public mine = (
            ^report: MyLarge new.
          )

          public plain = (
            ^report: largeIntegerClass new.
          )

          public sum = (
            ^MyLarge new + 1.
          )
        )
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let large_integer = interpreter.enter(|mutator| {
        mutator.stash(Value::object(
            mutator.builtin_class(BuiltinClass::LargeInteger),
        ))
    });
    let instance = expect_ok(
        interpreter.instantiate(&class, &[platform, large_integer]),
        "instantiate",
    );
    let mine = answer(&mut interpreter, &instance, "mine");
    let plain = answer(&mut interpreter, &instance, "plain");
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        assert_eq!(as_string(heap, mutator.fetch(&mine)), "true an Object");
        assert_eq!(as_string(heap, mutator.fetch(&plain)), "true an Object");
    });
    let message = expect_error(
        interpreter.send(&instance, Symbol::intern("sum"), &[]),
        "MyLarge new + 1",
    );
    assert_eq!(message, "an object does not understand #+");
}

/// `goldens/library/closure/a_closure_is_an_instance_of_closure.ns`:
/// §5.1.8, a closure literal denotes an instance of class `Closure`, the
/// very class object `Object`'s `Closure` accessor answers.
#[test]
fn a_closure_is_an_instance_of_the_class_closure() {
    let source = indoc! {"
        class Probe usingPlatform: platform = () (
          public closure = (
            ^[:each | each].
          )

          public closureClass = (
            ^Closure.
          )
        )
    "};
    let (mut interpreter, instance) = instantiate(source);
    let closure = answer(&mut interpreter, &instance, "closure");
    let closure_class = answer(&mut interpreter, &instance, "closureClass");
    interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(&closure));
        let Some(named) = mutator.fetch(&closure_class).as_object() else {
            panic!("`Closure` answers a class object");
        };
        assert!(class.same(named), "`[:each | each] class == Closure`");
    });
}

/// `goldens/library/number/float/a_float_beyond_the_immediate_range_behaves_as_any_float.ns`:
/// a denormal float, outside the immediate range, has the class of a huge
/// one.
#[test]
fn a_float_beyond_the_immediate_range_has_the_class_of_any_float() {
    let source = indoc! {"
        class Probe usingPlatform: platform = () (
          public big = (
            ^1.0e300.
          )

          public tiny = (
            ^1.0 / 1.0e300 / 1.0e10.
          )
        )
    "};
    let (mut interpreter, instance) = instantiate(source);
    let big = answer(&mut interpreter, &instance, "big");
    let tiny = answer(&mut interpreter, &instance, "tiny");
    interpreter.enter(|mutator| {
        let big_class = mutator.class_of(mutator.fetch(&big));
        let tiny_class = mutator.class_of(mutator.fetch(&tiny));
        assert!(tiny_class.same(big_class), "`tiny class = big class`");
    });
}

/// `goldens/library/number/float/float_sine_cosine_and_truncation.ns`:
/// `asInteger` of a float beyond 64 bits answers a `LargeInteger`.
#[test]
fn float_truncation_answers_a_large_integer_beyond_the_immediate_range() {
    assert_eq!(
        class_name_of_body("^100000000000000000000 asFloat asInteger."),
        "LargeInteger"
    );
    assert_eq!(
        class_name_of_body("^(0 - 9223372036854775808) asFloat asInteger."),
        "LargeInteger"
    );
}

/// `goldens/library/number/fraction/a_fraction_is_made_in_normal_form.ns`:
/// a fraction's denominator past the immediate range is a `LargeInteger`.
#[test]
fn a_fraction_in_normal_form_keeps_a_large_denominator_large() {
    assert_eq!(
        class_name_of_body(
            "^(Fraction numerator: 1 denominator: 1180591620717411303424) denominator."
        ),
        "LargeInteger"
    );
}

/// `goldens/library/number/integer/integer_protocol.ns`: `Integer parse:`
/// of a number past the immediate range answers a `LargeInteger`.
#[test]
fn integer_protocol_parses_a_large_integer() {
    assert_eq!(
        class_name_of_body("^Integer parse: '18446744073709551616'."),
        "LargeInteger"
    );
}

/// `goldens/library/number/large-integer/an_overflowing_integer_operation_answers_a_large_integer.ns`:
/// a result past the immediate range is a `LargeInteger`, `i64`'s largest
/// value already is one, and a result that fits 61 bits again is an
/// `Integer`.
// spec: vm.heap.number-one-representation
#[test]
fn an_overflowing_integer_operation_answers_a_large_integer() {
    let cases = [
        ("^9223372036854775807 + 1.", "LargeInteger"),
        ("^9223372036854775807.", "LargeInteger"),
        ("^1152921504606846975 + 1 - 1.", "Integer"),
    ];
    for (body, expected) in cases {
        assert_eq!(class_name_of_body(body), expected, "{body}");
    }
}

/// `goldens/library/number/large-integer/large_integer_arithmetic.ns`: a
/// result that fits again is answered as an `Integer`, and a negation past
/// the immediate range stays a `LargeInteger`.
#[test]
fn large_integer_arithmetic_answers_an_integer_where_the_result_fits() {
    let prologue = indoc! {"
        |
          two
          big
          small
          count
        |
        two:: 2.
        small:: 9223372036854775807.
        count:: 0.
        big:: 1.
        [count < 200] whileTrue: [
          big:: big * two.
          count:: count + 1.
        ].
    "};
    let cases = [
        ("^(small + 1) - 1.", "LargeInteger"),
        ("^big // big.", "Integer"),
        ("^(small + 1) negated.", "LargeInteger"),
    ];
    for (result, expected) in cases {
        assert_eq!(
            class_name_of_body(&format!("{prologue}{result}")),
            expected,
            "{result}"
        );
    }
}

/// `goldens/library/number/large-integer/large_integer_literals.ns`: a
/// literal past the two 61-bit edges of an `Integer` is a `LargeInteger`,
/// the two `i64` edges included, and one on them an `Integer`.
#[test]
fn an_integer_literal_is_large_exactly_past_the_immediate_range() {
    let cases = [
        ("9223372036854775808", "LargeInteger"),
        ("9223372036854775807", "LargeInteger"),
        ("-9223372036854775808", "LargeInteger"),
        ("-9223372036854775809", "LargeInteger"),
        ("1152921504606846975", "Integer"),
        ("1152921504606846976", "LargeInteger"),
        ("-1152921504606846976", "Integer"),
        ("-1152921504606846977", "LargeInteger"),
    ];
    for (literal, expected) in cases {
        assert_eq!(
            class_name_of_body(&format!("^{literal}.")),
            expected,
            "{literal}"
        );
    }
}

/// Whether `value`'s class is the class object `class` answers.
fn is_an_instance_of(interpreter: &mut Interpreter, value: &Root, class: &Root) -> bool {
    interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(class).as_object() else {
            panic!("expected a class object");
        };
        mutator.class_of(mutator.fetch(value)).same(class)
    })
}

/// Whether `left` and `right` have one class.
fn share_a_class(interpreter: &mut Interpreter, left: &Root, right: &Root) -> bool {
    interpreter.enter(|mutator| {
        let left_class = mutator.class_of(mutator.fetch(left));
        left_class.same(mutator.class_of(mutator.fetch(right)))
    })
}

/// `goldens/library/platform/the_platform_and_its_parts_are_values.ns`:
/// the transcript and the collections facet are each an instance of a
/// class of its own, not of the platform's.
#[test]
fn the_platform_and_its_parts_are_instances_of_classes_of_their_own() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let transcript = answer(&mut interpreter, &platform, "transcript");
    let collections = answer(&mut interpreter, &platform, "collections");
    assert!(
        !share_a_class(&mut interpreter, &transcript, &platform),
        "`t class == platform class` answers false"
    );
    assert!(
        !share_a_class(&mut interpreter, &collections, &platform),
        "`c class == platform class` answers false"
    );
}

/// The check that replaced a dropped line of
/// `goldens/library/object/a_proxy_forwards_every_message.ns`, which sent
/// `forwarder class` and so printed its target's class, `Integer`: a
/// forwarder answers every message with its target's answer, yet the host
/// reads it as an instance of its own class, `Forwarder`, not of its
/// target's.
#[test]
fn a_proxy_forwards_every_message_but_keeps_a_class_of_its_own() {
    let source = indoc! {"
        class Probe usingPlatform: platform = (
          |
            Proxy = platform kernel Proxy.
          |
        ) (
          class Forwarder on: theTarget = Proxy new (
            |
              private target = theTarget.
            |
          ) (
            protected doesNotUnderstand: message = (
              ^message sendTo: target.
            )
          )

          public forwarder = (
            ^Forwarder on: 3.
          )

          public forwarderClass = (
            ^Forwarder.
          )
        )
    "};
    let (mut interpreter, instance) = instantiate(source);
    let forwarder = answer(&mut interpreter, &instance, "forwarder");
    let forwarder_class = answer(&mut interpreter, &instance, "forwarderClass");
    let target = answer(&mut interpreter, &forwarder, "yourself");
    assert!(
        is_an_instance_of(&mut interpreter, &forwarder, &forwarder_class),
        "the forwarder is a `Forwarder`"
    );
    interpreter.enter(|mutator| {
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        assert!(
            mutator.class_of(mutator.fetch(&target)).same(integer),
            "`forwarder yourself` answers the target, an `Integer`"
        );
        assert!(
            !mutator.class_of(mutator.fetch(&forwarder)).same(integer),
            "the forwarder is no `Integer`"
        );
    });
}

/// `goldens/library/string/a_symbol_is_a_string.ns`: `copyFrom:to:` and
/// `,` of a Symbol answer a String, not a Symbol.
#[test]
fn a_symbol_copied_or_concatenated_answers_a_string() {
    for body in ["^#abc copyFrom: 1 to: 2.", "^#a , 'b'."] {
        assert!(
            answers_an_instance_of(body, BuiltinClass::String),
            "{body} answers a `String`"
        );
    }
}

/// `goldens/runtime/depth-limit/the_depth_limit_is_caught.ns`: the error
/// the depth limit signals is an instance of `Error` itself, no subclass.
#[test]
fn the_depth_limit_is_caught_as_an_instance_of_error_itself() {
    let source = indoc! {"
        class Probe usingPlatform: platform = () (
          recurse = (
            ^self recurse.
          )

          public answer = (
            ^[self recurse] on: Error do: [:error | error].
          )
        )
    "};
    let (mut interpreter, instance) = instantiate(source);
    let error = answer(&mut interpreter, &instance, "answer");
    interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(&error));
        assert!(
            class.same(mutator.builtin_class(BuiltinClass::Error)),
            "the depth limit's error is an `Error`, not {}",
            mutator.heap().class_display_name(class)
        );
    });
}
