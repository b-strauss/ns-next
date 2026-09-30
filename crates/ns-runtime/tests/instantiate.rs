//! End-to-end class instantiation tests: `ns_syntax::parse` -> `resolve` ->
//! `Interpreter::bootstrap()` -> `load_module` -> `instantiate` -> `send`.
//! See `docs/design/object-model.md`'s "Instantiation" section for the six
//! numbered steps this exercises.

use indoc::indoc;
use ns_runtime::{HostResult, HostSignal, Interpreter, Lowering, Mutator, Root, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both.
fn load(source: &str) -> (Interpreter, Root) {
    load_with(source, Lowering::Optimized)
}

fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `HostResult`
/// (`Result<Root, HostSignal>`) has none either and `Result::expect` isn't
/// usable on it directly -- unwrap by hand, with a message naming which
/// `HostSignal` variant leaked instead.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The object `root` holds, for the heap accessors that take one. Every
/// caller is already inside an entry.
fn object<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ns_runtime::ObjectReference<'gc> {
    match mutator.fetch(root) {
        Value::Object(object) => object,
        _ => panic!("expected an object"),
    }
}

#[test]
fn a_slot_initializer_and_a_method_reading_it_round_trip() {
    // `bar` must be `public` -- an ordinary (explicit-receiver) `send` only
    // reaches `public` members (`newspeak-spec.pdf` §5.5's `lookupPublic`);
    // the default access (no modifier) is `protected`.
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|x = 42.|) (public bar = (^x)) : ()
    "};
    let (mut interpreter, class) = load(source);

    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");

    let result = interpreter.send(&receiver, Symbol::intern("bar"), &[]);

    match result {
        Ok(root) => interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::SmallInteger(42) => {}
            other => panic!(
                "expected Value::SmallInteger(42), got {}",
                mutator.heap().describe(&other)
            ),
        }),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
        Err(HostSignal::Error(error)) => panic!("unexpected Newspeak error: {}", error.message),
    }
}

/// A subclass whose own slot initializer references a slot the superclass
/// set up -- proves Class Instantiation step 3 (the superclass's own
/// instance initializer) really runs *before* step 4 (this class's own
/// slots), not merely that both slots end up set eventually.
///
/// `Interpreter::instantiate`'s `climb_levels`/`descend_levels`
/// step ordering is what this test actually exercises. Linking the two
/// classes together, though, cannot go through a module root's superclass
/// clause: the root rule (`resolver::root_clause_defect`) lets a root name
/// only `Object` or `Value`, so a root naming a sibling *user-defined* class
/// is a resolve error.
/// Instead `Sub` is parsed with a builtin
/// `Object` superclass clause (so `load_module` resolves cleanly through the
/// ordinary pipeline), and its `ClassData.superclass`/`application` are then
/// relinked directly to the already-loaded `Sup` class -- both `pub` fields
/// on `ClassData`, exactly as visible to any other crate-external caller.
/// The resolver itself is untouched by this: `b`'s slot initializer
/// (`|b = a.|`) references `a`, which is *not* a statically known member of
/// `Sub` (its member-set computation also only looks up the superclass by
/// the same builtin-vs-not-yet-loaded-sibling name, so it treats `Sub`'s
/// superclass as the empty-member-set `Object`) -- so the resolver's §5.7
/// depth search finds no match anywhere in `Sub`'s own lexical chain and
/// falls back to its documented default, depth 0 (self), which is exactly
/// the binding a *correctly*-linked `Sub` would also want for a reference
/// to its own (however inherited) slot `a`. So the relink below changes
/// nothing about how `a` gets resolved or dispatched -- only which
/// superclass `instantiate` actually walks at runtime.
#[test]
fn superclass_slot_initializer_runs_before_the_subclass_own_slots() {
    let superclass_source = indoc! {"
        Newspeak3
        'T'
        class Sup = Object new (|a = 10.|) () : ()
    "};
    let (mut interpreter, superclass) = load(superclass_source);

    let subclass_source = indoc! {"
        Newspeak3
        'T'
        class Sub = Object new (|b = a.|) (public getB = (^b)) : ()
    "};
    let subclass_module = ns_syntax::parse(subclass_source).expect("parse");
    let subclass_resolved = ns_runtime::resolve(
        subclass_module,
        subclass_source.into(),
        ns_runtime::SourceOrigin::File("sub.ns".into()),
    )
    .expect("resolve");
    let subclass_root = interpreter.load_module(&subclass_resolved);

    // Relink Sub -> Sup (see the doc comment above), on both chains: the
    // class-object chain that `super`/`instantiate` walk, and the
    // application chain that carries the layout. Re-applying Sub's own mixin
    // to Sup's application is what recomputes `b`'s absolute index past
    // Sup's one inherited slot -- hand-editing the layout would let the two
    // chains disagree about the instance's shape. Nothing has run yet, so
    // the class objects read from their roots here are current.
    let relinked = interpreter.enter(|mutator| {
        let superclass = object(mutator, &superclass);
        let subclass = object(mutator, &subclass_root);
        let superclass_application = mutator
            .heap()
            .class_application(superclass)
            .unwrap_or_else(|| panic!("expected Sup to be a class object"));
        let subclass_mixin = mutator
            .heap()
            .class_application(subclass)
            .unwrap_or_else(|| panic!("expected Sub to be a class object"))
            .mixin
            .clone();
        let subclass_application =
            mutator.apply_mixin(&subclass_mixin, Some(&superclass_application));
        mutator
            .heap_mut()
            .set_class_superclass(subclass, superclass)
            && mutator
                .heap_mut()
                .set_class_application(subclass, subclass_application)
    });
    assert!(relinked, "expected Sub to be a class object");

    let receiver = expect_ok(interpreter.instantiate(&subclass_root, &[]), "instantiate");

    let result = interpreter.send(&receiver, Symbol::intern("getB"), &[]);

    match result {
        Ok(root) => interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::SmallInteger(10) => {}
            Value::Nil => panic!(
                "getB returned nil -- the superclass's own slot initializer (a = 10) did not run \
                 before the subclass's own slot initializer (b = a) read it"
            ),
            other => panic!(
                "expected Value::SmallInteger(10), got {}",
                mutator.heap().describe(&other)
            ),
        }),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
        Err(HostSignal::Error(error)) => panic!("unexpected Newspeak error: {}", error.message),
    }
}

/// The class's *own* factory parameters are locals
/// in the instance-initializer activation (`docs/design/object-model.md`,
/// "Instantiation", step 2 -- created with the factory message's
/// arguments), so a slot initializer may reference them. This is the
/// `class M usingPlatform: platform = Object new (| t = platform ... |)`
/// idiom every real Newspeak module is built out of. Pushing an empty
/// `LocalScope` here instead makes `platform` fall through to a
/// `LexicalSend`, which fails at run time with `doesNotUnderstand: #platform`.
#[test]
fn a_factory_parameter_is_visible_in_a_slot_initializer() {
    let source = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: platform = Object new (|t = platform.|) (public getT = (^t)) : ()
    "};
    let (mut interpreter, class) = load(source);

    let receiver = expect_ok(
        interpreter.instantiate(&class, &[interpreter.stash(Value::SmallInteger(99))]),
        "instantiate",
    );

    let result = interpreter.send(&receiver, Symbol::intern("getT"), &[]);

    match result {
        Ok(root) => interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::SmallInteger(99) => {}
            other => panic!(
                "expected the slot to hold the factory argument (SmallInteger(99)), got {}",
                mutator.heap().describe(&other)
            ),
        }),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
        Err(HostSignal::Error(error)) => panic!("unexpected Newspeak error: {}", error.message),
    }
}

/// The same, but the factory parameter is the *receiver* of a send inside
/// the slot initializer, and the whole thing runs through a real factory
/// send (`M usingPlatform: 99`) rather than a direct `instantiate` call --
/// i.e. exactly the path `ns-next run` takes.
#[test]
fn a_factory_parameter_is_usable_as_a_send_receiver_in_a_slot_initializer() {
    let source = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p = Object new (|t = p printString.|) (public getT = (^t)) : ()
    "};
    let (mut interpreter, class) = load(source);

    let receiver = expect_ok(
        interpreter.send(
            &class,
            Symbol::intern("usingPlatform:"),
            &[interpreter.stash(Value::SmallInteger(99))],
        ),
        "M usingPlatform: 99",
    );

    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("getT"), &[]),
        "getT",
    );

    interpreter.enter(|mutator| {
        let result = mutator.fetch(&result);
        match &result {
            Value::Object(object) => match mutator.heap().str_value(*object) {
                Some(string) => assert_eq!(string, "99"),
                None => panic!(
                    "expected ObjectData::String(\"99\"), got {}",
                    mutator.heap().describe(&result)
                ),
            },
            other => panic!(
                "expected Value::Object(<String>), got {}",
                mutator.heap().describe(other)
            ),
        }
    });
}

/// An `initExprs` statement (the class header's trailing statements, spec
/// step 5) sees the factory parameters too -- same activation, same scope.
#[test]
fn a_factory_parameter_is_visible_in_an_initializer_expression() {
    let source = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p = Object new (|t.| t:: p.) (public getT = (^t)) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    assert_eq!(
        module
            .root_class
            .instance_side
            .initializer_expressions
            .len(),
        1,
        "this fixture is only meaningful if the header's `t:: p.` really parsed as an initExpr"
    );
    let (mut interpreter, class) = load(source);

    let receiver = expect_ok(
        interpreter.instantiate(&class, &[interpreter.stash(Value::SmallInteger(7))]),
        "instantiate",
    );

    let result = interpreter.send(&receiver, Symbol::intern("getT"), &[]);

    match result {
        Ok(root) => interpreter.enter(|mutator| match mutator.fetch(&root) {
            Value::SmallInteger(7) => {}
            other => panic!(
                "expected SmallInteger(7) assigned by the initializer expression, got {}",
                mutator.heap().describe(&other)
            ),
        }),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
        Err(HostSignal::Error(error)) => panic!("unexpected Newspeak error: {}", error.message),
    }
}

/// A class declaring a two-keyword factory pattern (`usingPlatform:ide:`),
/// instantiated with only one factory argument. A slot initializer
/// referencing the *second*, missing parameter resolves to
/// `Binding::Local(1)`, but `factory_arguments` here has only one element and
/// the instance-initializer activation starts with no temporaries -- without
/// the arity check, `locals.rs`'s `read_local` would index straight past
/// both and panic with "index out of bounds" instead of returning a clean
/// `NewspeakError`.
#[test]
fn a_factory_arity_mismatch_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p ide: q = Object new (|t = q.|) (public getT = (^t)) : ()
    "};
    let (mut interpreter, class) = load(source);

    let result = interpreter.instantiate(&class, &[interpreter.stash(Value::SmallInteger(1))]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert!(
                error.message.contains("wrong number of factory arguments"),
                "unexpected error message: {}",
                error.message
            );
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a factory-arity Newspeak error, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `instantiate`"),
    }
}

#[test]
fn sending_an_undefined_selector_is_a_newspeak_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new () () : ()
    "};
    let (mut interpreter, class) = load(source);

    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");

    let result = interpreter.send(&receiver, Symbol::intern("thisSelectorDoesNotExist"), &[]);

    match result {
        Err(HostSignal::Error(error)) => {
            assert_eq!(error.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(value) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&value)));
            panic!("expected a doesNotUnderstand: error, got Ok({got})")
        }
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return escaping `send`"),
    }
}

/// `newspeak-spec.pdf` §6.2: the instance initializer evaluates the
/// superclass clause's message and invokes the superclass's initializer
/// with *those* arguments. Running every level in the subclass's own
/// activation instead would have `Base`'s slot initializer read the
/// *subclass's* (empty) argument list rather than the `7` its own clause
/// supplies.
///
/// The two classes are nested inside a module class because that is the
/// only way a user-defined superclass is linked through the ordinary
/// pipeline: a nested declaration's clause is evaluated by the accessor
/// (`interpreter::nested`), while a *root* class's clause may name only
/// `Object` or `Value` (the root rule, `root_clause_defect`).
#[test]
fn a_superclass_factory_message_receives_its_own_arguments() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class SuperclassArgs = Object new () (
            public class Base withValue: v = Object new (| baseValue ::= v. |) (
                public base = ( ^baseValue )
            )
            public class Derived = Base withValue: 7 () (
            )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let module = expect_ok(
        interpreter.instantiate(&class, &[]),
        "instantiate SuperclassArgs",
    );
    let derived = expect_ok(
        interpreter.send(&module, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    let instance = expect_ok(
        interpreter.send(&derived, Symbol::intern("new"), &[]),
        "Derived new",
    );
    let stored = expect_ok(
        interpreter.send(&instance, Symbol::intern("base"), &[]),
        "base",
    );
    assert!(
        interpreter.enter(|mutator| matches!(mutator.fetch(&stored), Value::SmallInteger(7))),
        "the superclass's slot initializer did not see the clause's own argument"
    );
}

ns_runtime::for_each_lowering! {
    /// `new` answers the instance its initializers ran on -- under both
    /// lowerings, from a host and from Newspeak, for a class with code at its
    /// own level only (`One`), at two levels (`Two`), and at its superclass's
    /// only (`Bare`). Each initializer notes the `self` it ran on.
    ///
    /// The last initializer an instantiation runs hands its own value to the
    /// send, with no step after it to answer the instance
    /// (`Mutator::descend_levels`): that relies on the compiler ending every
    /// initializer by answering `self`. An initializer that answered anything
    /// else -- its last statement's value, say, or `nil` -- would fail here.
    fn new_answers_the_instance_its_initializers_ran_on(lowering) {
        let source = indoc! {"
            Newspeak3
            'T'
            class Kinds = Object new (
                | seen ::= nil. |
            ) (
                public class One = Object new (
                    | aValue = 1. |
                    note: self.
                ) (
                    public a = ( ^aValue )
                )
                public class Base = Object new (
                    | bValue = 2. |
                    note: self.
                ) (
                    public b = ( ^bValue )
                )
                public class Two = Base new (
                    | cValue = 3. |
                    note: self.
                ) (
                    public c = ( ^cValue )
                )
                public class Bare = Base new (
                ) (
                )
                note: o = ( seen:: o )
                public lastSeen = ( ^seen )
                public make: c = ( ^c new )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let module = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Kinds");
        let cases: [(&str, &[(&str, i64)]); 3] = [
            ("One", &[("a", 1)]),
            ("Two", &[("b", 2), ("c", 3)]),
            ("Bare", &[("b", 2)]),
        ];
        for (name, slots) in cases {
            let class = expect_ok(interpreter.send(&module, Symbol::intern(name), &[]), name);
            interpreter.enter(|mutator| {
                let Value::Object(_) = mutator.fetch(&class) else {
                    panic!("{name} answers a class object")
                };
            });
            for from_newspeak in [false, true] {
                let what = format!("{name} new, {lowering:?}, from Newspeak: {from_newspeak}");
                let instance = if from_newspeak {
                    interpreter.send(
                        &module,
                        Symbol::intern("make:"),
                        std::slice::from_ref(&class),
                    )
                } else {
                    interpreter.send(&class, Symbol::intern("new"), &[])
                };
                let instance = expect_ok(instance, &what);
                let seen = expect_ok(
                    interpreter.send(&module, Symbol::intern("lastSeen"), &[]),
                    "lastSeen",
                );
                // Every handle compared below is read after `lastSeen`, the
                // last run before the comparisons.
                interpreter.enter(|mutator| {
                    let (Value::Object(instance_object), Value::Object(seen_object)) =
                        (mutator.fetch(&instance), mutator.fetch(&seen))
                    else {
                        panic!("{what}: answered something other than an object");
                    };
                    let class_object = object(mutator, &class);
                    assert!(
                        instance_object.same(seen_object),
                        "{what}: answered an object other than the one its initializers ran on"
                    );
                    assert!(
                        mutator
                            .class_of(&Value::Object(instance_object))
                            .same(class_object),
                        "{what}: answered an instance of another class"
                    );
                });
                for (getter, expected) in slots {
                    let value = expect_ok(
                        interpreter.send(&instance, Symbol::intern(getter), &[]),
                        getter,
                    );
                    assert!(
                        interpreter.enter(
                            |mutator| matches!(mutator.fetch(&value), Value::SmallInteger(integer) if integer == *expected)
                        ),
                        "{what}: slot {getter}"
                    );
                }
            }
        }
    }
}

/// The `HostError` a refused instantiation carries, for the two §3.1.1
/// refusals below: anything else leaks the wrong signal loudly.
fn expect_error(result: HostResult, what: &str) -> ns_runtime::HostError {
    match result {
        Ok(_) => panic!("{what}: answered an instance instead of refusing"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error,
    }
}

/// §3.1.1: a value object is deeply immutable, so a class that inherits
/// `Value` and declares a mutable slot can have no instance at all. The
/// refusal names the class, the slot, and the selector the program sent --
/// the class's own primary factory, never `new`, which a class declaring
/// `start:` does not understand.
#[test]
fn a_value_class_that_declares_a_mutable_slot_cannot_be_instantiated() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Counter start: first = Value new (
            | public count ::= first. |
        ) () : ()
    "};
    let (mut interpreter, class) = load(source);
    let one = interpreter.stash(Value::SmallInteger(1));
    let error = expect_error(interpreter.instantiate(&class, &[one]), "Counter start: 1");
    assert_eq!(error.selector, Symbol::intern("start:"));
    assert!(
        error.message.contains("Counter") && error.message.contains("count"),
        "the refusal names the class and the slot: {}",
        error.message
    );
}

/// §3.1.1's other half: *deeply* immutable, so a slot of a value may hold
/// only another value. A class object is one exactly when its enclosing
/// object is (`instantiate::class_object_is_a_value`), so `Plain`, nested
/// in an ordinary `Wrapper`, is none, and storing it is refused as the slot
/// is initialized, under the factory's selector.
#[test]
fn a_value_class_refuses_a_slot_that_is_not_a_value() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Values = Object <: Value () (
            public class Holder on: contents = Value new (
                | public contents = contents. |
            ) ()
            public class Wrapper = Object new () (
                public class Plain = Object new () ()
            )
        ) : ()
    "};
    let (mut interpreter, module_class) = load(source);
    let module = expect_ok(interpreter.instantiate(&module_class, &[]), "Values new");
    let holder = expect_ok(
        interpreter.send(&module, Symbol::intern("Holder"), &[]),
        "Holder",
    );
    let plain = a_class_that_is_no_value(&mut interpreter, &module);
    let error = expect_error(
        interpreter.instantiate(&holder, std::slice::from_ref(&plain)),
        "Holder on: Plain",
    );
    assert_eq!(error.selector, Symbol::intern("on:"));
    assert!(
        error.message.contains("Holder") && error.message.contains("contents"),
        "the refusal names the class and the slot: {}",
        error.message
    );
    // A module definition has no enclosing object and is a value (§6.4:
    // "Module definitions are value objects"), so the same slot takes one.
    expect_ok(
        interpreter.instantiate(&holder, std::slice::from_ref(&module_class)),
        "Holder on: Values",
    );
}

/// `module Wrapper new Plain`: a class nested in an ordinary object, and so
/// no value.
fn a_class_that_is_no_value(interpreter: &mut Interpreter, module: &Root) -> Root {
    let wrapper_class = expect_ok(
        interpreter.send(module, Symbol::intern("Wrapper"), &[]),
        "Wrapper",
    );
    let wrapper = expect_ok(interpreter.instantiate(&wrapper_class, &[]), "Wrapper new");
    expect_ok(
        interpreter.send(&wrapper, Symbol::intern("Plain"), &[]),
        "Plain",
    )
}

/// Both value refusals name the selector the program sent: the primary
/// factory of the class being *instantiated*, never of the level whose
/// initializer happens to be running. Each level of a chain runs its own
/// initializer in its own activation (§6.2), so the store below happens in
/// `Base`'s -- and reporting it as `held:`, which only the superclass
/// clause sends, sends a reader looking for a send their program does not
/// contain.
#[test]
fn a_refused_slot_names_the_factory_the_program_sent() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holders = Object <: Value () (
            public class Base held: contents = Value new (
                | public held = contents. |
            ) ()
            public class Derived take: contents = Base held: contents (
                | public own = 1. |
            ) ()
            public class Wrapper = Object new () (
                public class Plain = Object new () ()
            )
        ) : ()
    "};
    let (mut interpreter, module_class) = load(source);
    let module = expect_ok(interpreter.instantiate(&module_class, &[]), "Holders new");
    let derived = expect_ok(
        interpreter.send(&module, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    // `Plain` is nested in an ordinary object and so is no value, so
    // `Base`'s initializer refuses it.
    let plain = a_class_that_is_no_value(&mut interpreter, &module);
    let error = expect_error(
        interpreter.instantiate(&derived, std::slice::from_ref(&plain)),
        "Derived take: Plain",
    );
    assert_eq!(error.selector, Symbol::intern("take:"));
    assert!(
        error.message.contains("Base") && error.message.contains("held"),
        "the refusal names the level that declares the slot: {}",
        error.message
    );
}

/// An instance of `Value` itself is a value, so a value's slot may hold
/// one: §3.2 defines "subclass of" inclusively ("C's superclass is S or a
/// subclass of S"), which makes §3.1.1's "its class inherits from class
/// `Value`" true of `Value new`.
///
/// It was refused until 2026-09-23, while `Value new = Value new` already
/// answered `true` -- the two predicates that decide "is a value"
/// (`MixinApplication::inherits_value` and `builtins::value`'s
/// `state_is_a_value`) disagreed about exactly this object. The equality
/// is asserted here beside the slot for that reason: one test, both
/// predicates.
#[test]
fn a_value_class_accepts_a_slot_holding_an_instance_of_value_itself() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder = Value new (
            | public contents = Value new. |
        ) (
            public sameContentsAs: other = ( ^contents = other contents )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let first = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let second = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let equal = expect_ok(
        interpreter.send(
            &first,
            Symbol::intern("sameContentsAs:"),
            std::slice::from_ref(&second),
        ),
        "sameContentsAs:",
    );
    interpreter.enter(|mutator| match mutator.fetch(&equal) {
        Value::Boolean(true) => {}
        other => panic!(
            "two separately built `Value new`s are one value, got {}",
            mutator.heap().describe(&other)
        ),
    });
}

/// The same class over `Object` instead of `Value` accepts both: the two
/// refusals are about the application's chain reaching `Value`, not about
/// the mixin, which is why `Mutator::apply_mixin` answers them per
/// application.
#[test]
fn the_same_declarations_are_ordinary_below_object() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder on: contents = Object new (
            | public contents = contents. public count ::= 1. |
        ) () : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&class)),
        "Holder on: Holder",
    );
    let contents = expect_ok(
        interpreter.send(&instance, Symbol::intern("contents"), &[]),
        "contents",
    );
    interpreter.enter(|mutator| {
        assert!(object(mutator, &contents).same(object(mutator, &class)));
    });
}

/// The instantiations §6.2's mixin application initializes through. Each
/// initializer appends its argument to `log` as two more decimal digits, so
/// the log reads, left to right, which initializer ran when and with what.
/// Every clause message computes its arguments differently from the
/// applying class's own factory parameters, so an argument taken from the
/// wrong message, or a message evaluated with the wrong level's arguments,
/// shows as a different number rather than as the same one.
///
/// `Root` sits above `Base` so that the walk leaves an application's levels
/// again: `Base`'s own clause is evaluated with `Base`'s own argument, not
/// with the applying class's. `Other` declares a superclass clause of its
/// own, which an application of it replaces with the level below.
const APPLICATION_ORDER_SOURCE: &str = indoc! {"
    Newspeak3
    'T'
    class Order = Object new (
        | log ::= 0. |
    ) (
        public class Root withRoot: r = Object new (
            note: r.
        ) ()
        public class Base withBase: b = Root withRoot: b * 3 (
            note: b.
        ) ()
        public class Mixed withMixed: m = Object new (
            note: m.
        ) ()
        public class Other withOther: o = Root withRoot: o (
            note: o.
        ) ()
        public class Applied from: x and: y = Base withBase: x + 1 <: Mixed withMixed: y * 2 (
            note: x * 10 + y.
        ) ()
        public class Stacked from: x and: y = Base withBase: x + 1 <: Mixed withMixed: y * 2 <: Other withOther: x + y (
            note: x * 10 + y.
        ) ()
        public class Misnamed from: x and: y = Base withBase: x <: Mixed withOther: y (
        ) ()
        note: n = ( log:: log * 100 + n )
        public applied = ( log:: 0. Applied from: 3 and: 4. ^log )
        public stacked = ( log:: 0. Stacked from: 3 and: 4. ^log )
        public misnamed = ( ^Misnamed from: 3 and: 4 )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// §6.2: `class C = S ms <: M mm` initializes as `S`'s initializer with
    /// `ms`'s arguments, then `M`'s with `mm`'s, then `C`'s own -- both messages
    /// evaluated with `C`'s factory arguments, since both are written in `C`'s
    /// declaration. With `x = 3` and `y = 4`: `Root` gets `(3 + 1) * 3 = 12`,
    /// `Base` `4`, `Mixed` `8` and `Applied` `34`.
    fn an_applied_level_initializes_with_its_own_clause_message(lowering) {
        let (mut interpreter, class) = load_with(APPLICATION_ORDER_SOURCE, lowering);
        let module = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Order");
        let log = expect_ok(
            interpreter.send(&module, Symbol::intern("applied"), &[]),
            "applied",
        );
        interpreter.enter(|mutator| match mutator.fetch(&log) {
            Value::SmallInteger(12_04_08_34) => {}
            other => panic!(
                "{lowering:?}: expected the log 12 04 08 34, got {}",
                mutator.heap().describe(&other)
            ),
        });
    }
}

ns_runtime::for_each_lowering! {
    /// Several clauses stack left to right: `S ms <: M1 m1 <: M2 m2` is
    /// `(S ms <: M1 m1) <: M2 m2`, so the message into the upper level is `m2`,
    /// the message from it into the lower level is `m1`, and the one from the
    /// lower level into `S` is `ms` -- every one evaluated with the applying
    /// class's arguments. `Root` 12, `Base` 4, `Mixed` 8, `Other` 7, `Stacked`
    /// 34; `Other`'s own superclass clause is not evaluated at all.
    fn stacked_applied_levels_each_initialize_with_their_own_clause_message(lowering) {
        let (mut interpreter, class) = load_with(APPLICATION_ORDER_SOURCE, lowering);
        let module = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Order");
        let log = expect_ok(
            interpreter.send(&module, Symbol::intern("stacked"), &[]),
            "stacked",
        );
        interpreter.enter(|mutator| match mutator.fetch(&log) {
            Value::SmallInteger(12_04_08_07_34) => {}
            other => panic!(
                "{lowering:?}: expected the log 12 04 08 07 34, got {}",
                mutator.heap().describe(&other)
            ),
        });
    }
}

ns_runtime::for_each_lowering! {
    /// §6.2 gives the applied level the primary factory selector of its clause's
    /// message, and the named class's initializer must accept that message's
    /// arguments, so a clause whose message is not the named class's primary
    /// factory is §6.1's run time error, as a superclass clause's is.
    fn a_mixin_clause_message_must_name_the_primary_factory(lowering) {
        let (mut interpreter, class) = load_with(APPLICATION_ORDER_SOURCE, lowering);
        let module = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Order");
        let error = expect_error(
            interpreter.send(&module, Symbol::intern("misnamed"), &[]),
            "misnamed",
        );
        assert!(
            error.message.contains("#withOther:")
                && error
                    .message
                    .contains("primary factory of class Mixed is #withMixed:"),
            "{lowering:?}: message was: {}",
            error.message
        );
    }
}

/// Clause messages whose arguments note themselves in the log, so the log
/// shows when each message is evaluated against when each initializer runs.
/// `Other` has no initializer of its own, and `Quiet` has none anywhere in
/// its chain: a level's message is evaluated even when no code of the level
/// runs after it.
const EVALUATION_ORDER_SOURCE: &str = indoc! {"
    Newspeak3
    'T'
    class Evaluation = Object new (
        | log ::= 0. |
    ) (
        public class Base withBase: b = Object new (
            note: b.
        ) ()
        public class Mixed withMixed: m = Object new (
            note: m.
        ) ()
        public class Other withOther: o = Object new () ()
        public class Applied from: x = Base withBase: (noted: 1) <: Mixed withMixed: (noted: 2) (
            note: x.
        ) ()
        public class Stacked from: x = Base withBase: (noted: 1) <: Mixed withMixed: (noted: 2) <: Other withOther: (noted: 4) (
            note: x.
        ) ()
        public class Quiet = Object new <: Other withOther: (noted: 6) () ()
        note: n = ( log:: log * 100 + n )
        noted: n = ( note: n. ^n )
        public applied = ( log:: 0. Applied from: 3. ^log )
        public stacked = ( log:: 0. Stacked from: 3. ^log )
        public quiet = ( log:: 0. Quiet new. ^log )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// §6.2's instance initializer for `I` "evaluates ms to μs … invokes the
    /// instance initializer of S on o with the arguments of μs. Then, m is
    /// evaluated to μm": a `<:` clause's message is evaluated after the
    /// superclass's initializer has run and just before its own mixin's, never
    /// on the way up with the superclass clause's. Each case runs under both
    /// lowerings; the log reads two digits per note.
    fn an_applied_level_evaluates_its_message_just_before_its_initializer(lowering) {
        let cases: [(&str, i64); 3] = [
            // ms 1, Base 1, m 2, Mixed 2, Applied 3.
            ("applied", 1_01_02_02_03),
            // ms 1, Base 1, m1 2, Mixed 2, m2 4, Other has no initializer,
            // Stacked 3.
            ("stacked", 1_01_02_02_04_03),
            // m 6, and no initializer anywhere.
            ("quiet", 6),
        ];
        let (mut interpreter, class) = load_with(EVALUATION_ORDER_SOURCE, lowering);
        let module = expect_ok(
            interpreter.instantiate(&class, &[]),
            "instantiate Evaluation",
        );
        for (selector, expected) in cases {
            let log = expect_ok(
                interpreter.send(&module, Symbol::intern(selector), &[]),
                selector,
            );
            interpreter.enter(|mutator| match mutator.fetch(&log) {
                Value::SmallInteger(actual) if actual == expected => {}
                other => panic!(
                    "{lowering:?} {selector}: expected the log {expected}, got {}",
                    mutator.heap().describe(&other)
                ),
            });
        }
    }
}
