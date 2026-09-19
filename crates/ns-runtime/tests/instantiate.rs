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
/// classes together, though, runs into a real known gap flagged in
/// `interpreter/instantiate.rs`'s `resolve_superclass_receiver`: a
/// superclass clause naming a sibling *user-defined* class (as opposed to a
/// builtin) by name is not resolved there -- only a fixed list of builtin
/// names is. Rather than expanding that list -- `resolve_superclass_receiver`
/// says to extend it only when a fixture needs it, and this fixture can route
/// around it -- `Sub` is parsed with a builtin
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
/// (`interpreter::nested`), while a *root* class's clause receiver
/// only resolves to a builtin (`resolve_superclass_receiver`).
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
#[test]
fn new_answers_the_instance_its_initializers_ran_on() {
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
    for lowering in [Lowering::Optimized, Lowering::Reference] {
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
