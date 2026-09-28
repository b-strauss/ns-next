//! Tests for the `Interpreter::send`-level `ClassName new` -> `instantiate`
//! wiring (`newspeak-spec.pdf` §6.1, §6.3). These tests go through
//! `Interpreter::send` rather than calling `Interpreter::instantiate`
//! directly from Rust -- the code path an ordinary Newspeak `ClassName new`
//! message send actually takes.

use indoc::indoc;
use ns_runtime::mixin::MethodEntry;
use ns_runtime::{
    BuiltinClass, HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol, Value,
};

/// The module's root class object, loaded and rooted, compiled with
/// `lowering`.
fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        SourceOrigin::File("factory_send.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

fn load(source: &str) -> (Interpreter, Root) {
    load_with(source, Lowering::Optimized)
}

/// Asserts that sending `sent` to `receiver` raises a Newspeak error naming
/// `unhandled` -- which is `sent` itself when the send is the one that was
/// not understood, and the inner selector when the failure happens further
/// in.
fn expect_unhandled(
    interpreter: &mut Interpreter,
    receiver: &Root,
    sent: Symbol,
    unhandled: Symbol,
    what: &str,
) {
    match interpreter.send(receiver, sent, &[]) {
        Err(HostSignal::Error(error)) => assert_eq!(error.selector, unhandled, "{what}"),
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("{what}: expected doesNotUnderstand: {unhandled}, got Ok({got})")
        }
        Err(HostSignal::Return) => {
            panic!("{what}: expected doesNotUnderstand: {unhandled}, got HostSignal::Return")
        }
    }
}

/// Which kind of entry `class`'s metaclass mixin holds under `selector`, as
/// a word, or `None` when it holds none. Reads the mixin rather than the
/// lookup on purpose: the point of §6.3 is that the factory *is* a member of
/// that method table, and a send would pass whether it was or not.
fn metaclass_entry_kind(
    interpreter: &mut Interpreter,
    class: &Root,
    selector: &str,
) -> Option<&'static str> {
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(class) else {
            return None;
        };
        let metaclass = mutator.metaclass_of(object)?;
        let application = mutator.heap().class_application(metaclass)?;
        application
            .mixin
            .methods
            .get(&Symbol::intern(selector))
            .map(|entry| match entry {
                MethodEntry::Compiled(_) => "compiled",
                MethodEntry::SlotAccessor(_) => "slot accessor",
                MethodEntry::Primitive(_) => "primitive",
                MethodEntry::Factory(_) => "factory",
            })
    })
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

/// `Array new` via ordinary `send` must be `Array`-tagged, confirmed here by
/// `add:` *failing* -- `Array`'s method table has no `add:` primitive.
#[test]
fn array_new_via_ordinary_send_does_not_understand_add() {
    let mut interpreter = Interpreter::bootstrap();
    let class_value = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::Object(class))
    });

    let receiver = expect_ok(
        interpreter.send(&class_value, Symbol::intern("new"), &[]),
        "Array new",
    );

    match interpreter.send(
        &receiver,
        Symbol::intern("add:"),
        &[interpreter.stash(Value::SmallInteger(1))],
    ) {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("add:"));
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected doesNotUnderstand: add:, got Ok({got})")
        }
        Err(HostSignal::Return) => {
            panic!("expected doesNotUnderstand: add:, got HostSignal::Return")
        }
    }
}

/// A user-defined class's `new` via ordinary `send` must behave exactly
/// like calling `interpreter.instantiate(...)` directly: same slot values
/// reachable afterward through a real method send.
#[test]
fn user_defined_class_new_via_ordinary_send_matches_direct_instantiate() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|x = 42.|) (public bar = (^x)) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");

    // Path A: `Foo new` via ordinary `send`.
    let mut a_interpreter = Interpreter::bootstrap();
    let a_class = a_interpreter.load_module(&resolved);
    let via_send = expect_ok(
        a_interpreter.send(&a_class, Symbol::intern("new"), &[]),
        "Foo new (via send)",
    );
    let via_send_bar = expect_ok(
        a_interpreter.send(&via_send, Symbol::intern("bar"), &[]),
        "Foo new bar (via send)",
    );

    // Path B: `interpreter.instantiate(...)` called directly.
    let mut b_interpreter = Interpreter::bootstrap();
    let b_class = b_interpreter.load_module(&resolved);
    let via_instantiate = expect_ok(b_interpreter.instantiate(&b_class, &[]), "instantiate(Foo)");
    let via_instantiate_bar = expect_ok(
        b_interpreter.send(&via_instantiate, Symbol::intern("bar"), &[]),
        "instantiate(Foo) bar",
    );

    // Each side leaves its interpreter's entry as the integer it answered,
    // or as the description of whatever else it answered.
    let from_send = a_interpreter.enter(|mutator| match mutator.fetch(&via_send_bar) {
        Value::SmallInteger(integer) => Ok(integer),
        other => Err(mutator.heap().describe(&other)),
    });
    let from_instantiate =
        b_interpreter.enter(|mutator| match mutator.fetch(&via_instantiate_bar) {
            Value::SmallInteger(integer) => Ok(integer),
            other => Err(mutator.heap().describe(&other)),
        });
    match (from_send, from_instantiate) {
        (Ok(sent), Ok(instantiated)) => assert_eq!(sent, instantiated),
        (sent, instantiated) => {
            let sent_description =
                sent.map_or_else(|description| description, |integer| integer.to_string());
            let instantiated_description =
                instantiated.map_or_else(|description| description, |integer| integer.to_string());
            panic!("expected two equal Value::SmallInteger, got {sent_description} and {instantiated_description}")
        }
    }
}

/// A class object receiving a selector that is *not* its factory selector
/// still goes through ordinary dispatch (falls through to
/// `doesNotUnderstand:` here, since nothing implements the selector) --
/// proves the factory entry answers its own selector alone, not every send
/// to a class object.
#[test]
fn a_class_object_receiving_a_non_factory_selector_falls_through_to_ordinary_dispatch() {
    let mut interpreter = Interpreter::bootstrap();
    let class_value = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::Object(class))
    });

    match interpreter.send(
        &class_value,
        Symbol::intern("thisSelectorDoesNotExist"),
        &[],
    ) {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected doesNotUnderstand:, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("expected doesNotUnderstand:, got HostSignal::Return"),
    }
}

/// §6.3: "The class side, together with the primary factory method, defines
/// the mixin for the metaclass." So the factory is an entry of that method
/// table, under the declared factory's selector, and is found by the
/// ordinary lookups -- not by a case that runs ahead of them.
#[test]
fn a_factory_is_an_entry_of_the_metaclass_mixin() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Boxed with: contents = Object new (| public held = contents. |) (
        ) : (
            public describe = ( ^'a Boxed' )
        )
    "};
    let (mut interpreter, class) = load(source);
    assert_eq!(
        metaclass_entry_kind(&mut interpreter, &class, "with:"),
        Some("factory"),
        "the metaclass mixin must hold the factory under its selector"
    );
    // The class side's own members are unaffected, and `new` -- which is not
    // this class's factory selector -- is not an entry at all.
    assert_eq!(
        metaclass_entry_kind(&mut interpreter, &class, "describe"),
        Some("compiled")
    );
    assert_eq!(metaclass_entry_kind(&mut interpreter, &class, "new"), None);
}

/// A class that declares no class side still has a metaclass, because the
/// factory lives on it (§6.3). Before the factory became an entry, such a
/// class had no metaclass at all.
#[test]
fn a_class_without_a_class_side_still_has_a_factory_entry() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Plain = Object new () (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    assert_eq!(
        metaclass_entry_kind(&mut interpreter, &class, "new"),
        Some("factory")
    );
}

/// A builtin declares no factory pattern, so the default from §6's
/// unnumbered introduction (before §6.1 begins) applies: its metaclass
/// holds one entry, under `new`.
#[test]
fn a_builtin_class_has_a_factory_entry_under_new() {
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.enter(|mutator| {
        let class = mutator.builtin_class(BuiltinClass::Array);
        mutator.stash(Value::Object(class))
    });
    assert_eq!(
        metaclass_entry_kind(&mut interpreter, &class, "new"),
        Some("factory")
    );
}

/// Two class objects of one nested declaration share one metaclass mixin, so
/// they share one application and one dispatch id. Building a fresh mixin per
/// class object instead would give every enclosing instance its own
/// behaviour, and every `X new` site would go megamorphic (§6.2).
#[test]
fn every_class_object_of_one_declaration_shares_its_metaclass_behaviour() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Holder = Object new () (
            class Item = Object new () (
            )
            public item = ( ^Item )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let first = expect_ok(interpreter.instantiate(&class, &[]), "instantiate (first)");
    let second = expect_ok(interpreter.instantiate(&class, &[]), "instantiate (second)");
    let first_item = expect_ok(
        interpreter.send(&first, Symbol::intern("item"), &[]),
        "first item",
    );
    let second_item = expect_ok(
        interpreter.send(&second, Symbol::intern("item"), &[]),
        "second item",
    );
    let shared = interpreter.enter(|mutator| {
        let (Value::Object(first), Value::Object(second)) =
            (mutator.fetch(&first_item), mutator.fetch(&second_item))
        else {
            return false;
        };
        assert!(
            !first.same(second),
            "one class object per enclosing instance"
        );
        let (Some(first), Some(second)) =
            (mutator.metaclass_of(first), mutator.metaclass_of(second))
        else {
            return false;
        };
        mutator.heap().class_dispatch_ids(first).instances
            == mutator.heap().class_dispatch_ids(second).instances
    });
    assert!(shared, "both metaclasses must share one application");
}

/// With a flat metaclass (§3.2) a factory entry is reachable only on the
/// receiver's own metaclass: `Keyed new` is not an instantiation, because
/// `new` is `Simple`'s factory selector and `Keyed`'s is `with:`. A class
/// side is not inherited, so neither is the factory that shares its mixin.
#[test]
fn a_superclasss_factory_selector_is_not_understood() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class FactoryNotInherited = Object new () (
            class Simple = Object new () (
                public label = ( ^'simple' )
            )
            class Keyed with: seed = Simple new (| public held = seed. |) (
            )
            public simple = ( ^Simple )
            public keyed = ( ^Keyed )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let keyed = expect_ok(
        interpreter.send(&instance, Symbol::intern("keyed"), &[]),
        "keyed",
    );
    match interpreter.send(&keyed, Symbol::NEW, &[]) {
        Err(HostSignal::Error(error)) => assert_eq!(error.selector, Symbol::NEW),
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected doesNotUnderstand: new, got Ok({got})")
        }
        Err(HostSignal::Return) => {
            panic!("expected doesNotUnderstand: new, got HostSignal::Return")
        }
    }
    // `Keyed`'s own factory still instantiates, and so does `Simple`'s.
    let seed = interpreter.stash(Value::SmallInteger(7));
    let built = expect_ok(
        interpreter.send(&keyed, Symbol::intern("with:"), std::slice::from_ref(&seed)),
        "Keyed with: 7",
    );
    let answered = expect_ok(
        interpreter.send(&built, Symbol::intern("held"), &[]),
        "Keyed with: 7 held",
    );
    interpreter.enter(|mutator| match mutator.fetch(&answered) {
        Value::SmallInteger(7) => {}
        other => panic!("expected 7, got {}", mutator.heap().describe(&other)),
    });
    let simple = expect_ok(
        interpreter.send(&instance, Symbol::intern("simple"), &[]),
        "simple",
    );
    expect_ok(interpreter.send(&simple, Symbol::NEW, &[]), "Simple new");
}

ns_runtime::for_each_lowering! {
    /// A class-side `super new` is not an instantiation. §5.10 starts the lookup
    /// at S, the superclass of the running activation's `current_class`, which
    /// for a class-side method is the metaclass; §6 makes that `Class`, and
    /// neither `Class` nor anything above it declares a factory, so the send
    /// reaches `doesNotUnderstand:`.
    ///
    /// It did instantiate until §6.3's factory became an entry of the metaclass
    /// mixin: a class side was inherited then, so an inherited class-side
    /// method's `super new` could be the only way to reach the receiver's own
    /// factory, and §5.10's failure path checked for it before running any
    /// handler. The fixture that pinned the old answer was re-cut when
    /// metaclasses went flat, so this pins the answer that replaced it.
    fn a_class_side_super_new_does_not_instantiate(lowering) {
        let source = indoc! {"
            Newspeak3
            'Test'
            class SuperFactory = Object new () (
                class Simple = Object new () (
                    public label = ( ^'a simple' )
                ) : (
                    public build = ( ^super new )
                )
                public simple = ( ^Simple )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let simple = expect_ok(
            interpreter.send(&instance, Symbol::intern("simple"), &[]),
            "simple",
        );
        // The unhandled selector is `new`, the `super` send inside `build`:
        // `build` itself is found on `Simple`'s own metaclass.
        expect_unhandled(
            &mut interpreter,
            &simple,
            Symbol::intern("build"),
            Symbol::NEW,
            &format!("Simple build under {lowering:?}"),
        );
        // The class's own factory, on its own metaclass, still instantiates:
        // what `super` cannot reach is not what the class object cannot do.
        let built = expect_ok(interpreter.send(&simple, Symbol::NEW, &[]), "Simple new");
        let label = expect_ok(
            interpreter.send(&built, Symbol::intern("label"), &[]),
            "Simple new label",
        );
        interpreter.enter(|mutator| {
            let value = mutator.fetch(&label);
            assert_eq!(mutator.heap().describe(&value), "the String \"a simple\"");
        });
    }
}
