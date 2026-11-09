//! Every builtin class's superclass is `Object`, and the ordinary lookup
//! walk reaches `Object`'s primitives from any builtin receiver. These tests
//! pin that a non-`Object` builtin receiver reaches `Object`'s own
//! `yourself`/`==`/`=`/`printString`, and -- most importantly --
//! `Object`'s real, spec-shaped `doesNotUnderstand:`
//! (`builtins::object_class`), not the interpreter's internal
//! "no doesNotUnderstand: found" fallback string. A number and a string
//! reach `Value`'s `==` instead, which optimized code's inlined `==`
//! relies on.

use ns_runtime::{
    BuiltinClass, Heap, HostResult, HostSignal, Interpreter, Root, Symbol, Value, ValueKind,
};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
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

/// `3 yourself` -- `Integer` and its superclasses below `Object` implement
/// no `yourself` of their own, so this can only succeed by inheriting
/// `Object`'s, which answers the receiver. `Object`'s mirror forwarders
/// (`class`, `==`, `=`, `hash`, `printString`, `identityHash`) are each
/// overridden on `Integer`'s chain or protected, so the inherited member
/// asked here is a plain method.
#[test]
fn a_builtin_receiver_inherits_objects_yourself() {
    let mut interpreter = Interpreter::bootstrap();

    let result = interpreter.send(
        &interpreter.stash(Value::small_integer(3)),
        Symbol::intern("yourself"),
        &[],
    );

    let result = expect_ok(result, "3 yourself");
    interpreter.enter(|mutator| {
        let answer = mutator.fetch(&result);
        assert_eq!(
            answer.as_small_integer(),
            Some(3),
            "`3 yourself` must yield the receiver, got {}",
            mutator.heap().describe(answer)
        );
    });
}

/// `nil printString` answers `nil`, as Primordial Soup's `UndefinedObject`
/// does.
#[test]
fn nil_prints_as_nil() {
    let mut interpreter = Interpreter::bootstrap();
    let nil_result = interpreter.send(
        &interpreter.stash(Value::nil()),
        Symbol::intern("printString"),
        &[],
    );
    let nil_result = expect_ok(nil_result, "nil printString");
    interpreter.enter(|mutator| {
        assert_eq!(as_string(mutator.heap(), mutator.fetch(&nil_result)), "nil");
    });
}

/// `(Array new) printString`: `Array` implements no `printString`, so it
/// falls back to `Object`'s own generic default rather than
/// `doesNotUnderstand:`.
#[test]
fn builtins_without_their_own_print_string_inherit_objects_default() {
    let mut interpreter = Interpreter::bootstrap();

    let array_class = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::object(class))
    });
    let array = expect_ok(
        interpreter.send(&array_class, Symbol::intern("new"), &[]),
        "Array new",
    );
    let array_result = interpreter.send(&array, Symbol::intern("printString"), &[]);
    let array_result = expect_ok(array_result, "(Array new) printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), mutator.fetch(&array_result)),
            "an Object"
        );
    });
}

/// `3 == 3`: `Value`'s `==`, reached from an `Integer` receiver -- a send
/// of `=`, since §3.1.1 leaves a value no identity to answer from.
/// `3 hash` does *not* reach `Object`'s: `Integer` declares its own, which
/// answers the integer itself, since a value hashes by its value rather
/// than by an identity it does not have.
#[test]
fn a_builtin_receiver_compares_and_hashes_by_its_value() {
    let mut interpreter = Interpreter::bootstrap();

    let same = interpreter.send(
        &interpreter.stash(Value::small_integer(3)),
        Symbol::intern("=="),
        &[interpreter.stash(Value::small_integer(3))],
    );
    let same = expect_ok(same, "3 == 3");
    assert_eq!(
        interpreter.enter(|mutator| mutator.fetch(&same).as_boolean()),
        Some(true)
    );

    let hash = interpreter.send(
        &interpreter.stash(Value::small_integer(3)),
        Symbol::intern("hash"),
        &[],
    );
    let hash = expect_ok(hash, "3 hash");
    assert!(
        interpreter.enter(|mutator| mutator.fetch(&hash).as_small_integer() == Some(3)),
        "an Integer hashes by its value, not by Object's identity hash"
    );
}

/// The load-bearing half: an unimplemented selector on a builtin must
/// route through `builtins::object_class`'s real `doesNotUnderstand:` --
/// whose message is "<describe(receiver)> does not understand #<selector>"
/// -- rather than the "(and no doesNotUnderstand: found anywhere in the
/// class chain)" fallback `interpreter/resolve.rs` raises when its handler
/// lookup finds nothing (`Resolved::Unhandled`).
#[test]
fn an_unknown_selector_on_a_builtin_runs_objects_real_does_not_understand() {
    let mut interpreter = Interpreter::bootstrap();

    let result = interpreter.send(
        &interpreter.stash(Value::small_integer(3)),
        Symbol::intern("frobnicate"),
        &[],
    );

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("frobnicate"));
            assert_eq!(
                error.message, "an object does not understand #frobnicate",
                "expected `object_class`'s own doesNotUnderstand: message shape"
            );
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&value)));
            panic!("expected a doesNotUnderstand: error for `3 frobnicate`, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// The same, for an object-shaped builtin receiver (`String`), to prove the
/// fallback is not specific to a primitive `Value` variant.
#[test]
fn an_unknown_selector_on_a_string_runs_objects_real_does_not_understand() {
    let mut interpreter = Interpreter::bootstrap();
    let receiver = interpreter.enter(|mutator| {
        let object = mutator.new_string("hi".to_string());
        mutator.stash(Value::object(object))
    });

    let result = interpreter.send(&receiver, Symbol::intern("frobnicate"), &[]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.message, "an object does not understand #frobnicate");
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&value)));
            panic!("expected a doesNotUnderstand: error, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// The fallback must never shadow a builtin's *own* implementation of a
/// selector `Object` also implements: `Integer` has its own `=` and
/// `printString`, and those must still win.
#[test]
fn a_builtins_own_implementation_still_wins_over_objects() {
    let mut interpreter = Interpreter::bootstrap();

    // Object's `=` is identity; Integer's own `=` is numeric equality. Both
    // agree on `3 = 3`, so use `printString` -- where the two genuinely
    // differ -- as the discriminating case.
    let printed = interpreter.send(
        &interpreter.stash(Value::small_integer(7)),
        Symbol::intern("printString"),
        &[],
    );
    let printed = expect_ok(printed, "7 printString");
    interpreter.enter(|mutator| {
        assert_eq!(
            as_string(mutator.heap(), mutator.fetch(&printed)),
            "7",
            "Integer's own printString must win over Object's generic default"
        );
    });
}

/// Optimized code's inlined `==` (`Node::IdentityCompare`, emitted by the
/// macro assembler's `jump_if_identity_undecided`) answers `false` for two
/// different words, where `Object>>==`'s leaf answers `true` for two boxed
/// floats of the same bits: it is right only because no number and no
/// string reaches that leaf on its left, every one of them a `Value`,
/// whose `==` is `=`. This pins both halves of that: the lookup of `==`
/// from every number and string class finds `Value`'s method, never the
/// mirror's identity row nor a method of their own, and that method's
/// body in `Kernel.ns` is `^self = other`. Held here, by the lookup and
/// the shipped source, rather than by a golden: two boxed floats of the
/// same bits are identical by the interpreter's leaf too, so no program's
/// output would change unless the optimizer happened to inline that very
/// send, and a golden would then pin a path rather than the rule.
#[test]
fn every_number_and_string_compares_identity_by_values_equality() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let selector = Symbol::intern("==");
        for kind in [
            BuiltinClass::Number,
            BuiltinClass::Integer,
            BuiltinClass::LargeInteger,
            BuiltinClass::Float,
            BuiltinClass::Fraction,
            BuiltinClass::String,
            BuiltinClass::Symbol,
        ] {
            let class = mutator.builtin_class(kind);
            let heap = mutator.heap();
            match ns_runtime::dispatch::lookup_public(heap, class, selector) {
                ns_runtime::dispatch::Lookup::Method(_, defining_class) => assert_eq!(
                    heap.class_name(defining_class),
                    Some(Symbol::intern("Value")),
                    "{kind:?}'s `==` is `Value`'s"
                ),
                _ => panic!("{kind:?}'s `==` is not found as a method"),
            }
        }
    });

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("core-library/Kernel.ns");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let Ok(module) = ns_syntax::parse(&source) else {
        panic!("Kernel.ns does not parse");
    };
    let Some(value) = module
        .root_class
        .instance_side
        .nested_classes()
        .find(|class| class.name.name == "Value")
    else {
        panic!("Kernel.ns declares no Value");
    };
    let Some(identity) = value.instance_side.methods().find(|method| {
        matches!(
            &method.pattern,
            ns_syntax::ast::MessagePattern::Binary { selector, .. } if selector.name == "=="
        )
    }) else {
        panic!("Value declares no `==`");
    };
    let text = &source[identity.span.start as usize..identity.span.end as usize];
    let words: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(
        words.join(" "),
        "public == other = ( ^self = other. )",
        "Value>>== is `=`, which optimized code's inlined `==` relies on"
    );
}
