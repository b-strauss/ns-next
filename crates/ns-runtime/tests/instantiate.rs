//! End-to-end class instantiation tests: `ns_syntax::parse` -> `resolve` ->
//! `Interpreter::bootstrap()` -> `load_module` -> `instantiate` -> `send`.
//! See `docs/design/object-model.md`'s "Instantiation" section for the six
//! numbered steps this exercises.

use indoc::indoc;
use ns_runtime::{Interpreter, Lowering, Signal, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both.
fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef) {
    load_with(src, Lowering::Optimized)
}

fn load_with(src: &str, lowering: Lowering) -> (Interpreter, ns_runtime::ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap_with(lowering);
    let class = interp.load_module_unrooted(&resolved);
    (interp, class)
}

/// `Signal` carries no `Debug` impl of its own, so `EvalResult`
/// (`Result<Value, Signal>`) has none either and `Result::expect` isn't
/// usable on it directly -- unwrap by hand, with a message naming which
/// `Signal` variant leaked instead.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn a_slot_initializer_and_a_method_reading_it_round_trip() {
    // `bar` must be `public` -- an ordinary (explicit-receiver) `send` only
    // reaches `public` members (`newspeak-spec.pdf` §5.5's `lookupPublic`);
    // the default access (no modifier) is `protected`.
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (|x = 42.|) (public bar = (^x)) : ()
    "};
    let (mut interp, class) = load(src);

    let receiver = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");

    let result = interp.send_unrooted(receiver, Symbol::intern("bar"), vec![]);

    match result {
        Ok(Value::SmallInt(42)) => {}
        Ok(other) => panic!(
            "expected Value::SmallInt(42), got {}",
            interp.heap_ref().describe(&other)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
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
    let sup_src = indoc! {"
        Newspeak3
        'T'
        class Sup = Object new (|a = 10.|) () : ()
    "};
    let (mut interp, sup_class) = load(sup_src);

    let sub_src = indoc! {"
        Newspeak3
        'T'
        class Sub = Object new (|b = a.|) (public getB = (^b)) : ()
    "};
    let sub_module = ns_syntax::parse(sub_src).expect("parse");
    let sub_resolved = ns_runtime::resolve(
        sub_module,
        sub_src.into(),
        ns_runtime::SourceOrigin::File("sub.ns".into()),
    )
    .expect("resolve");
    let sub_class = interp.load_module_unrooted(&sub_resolved);

    // Relink Sub -> Sup (see the doc comment above), on both chains: the
    // class-object chain that `super`/`instantiate` walk, and the
    // application chain that carries the layout. Re-applying Sub's own mixin
    // to Sup's application is what recomputes `b`'s absolute index past
    // Sup's one inherited slot -- hand-editing the layout would let the two
    // chains disagree about the instance's shape.
    let sup_application = interp
        .heap_ref()
        .class_application(sup_class)
        .unwrap_or_else(|| panic!("expected Sup to be a class object"));
    let sub_mixin = interp
        .heap_ref()
        .class_application(sub_class)
        .unwrap_or_else(|| panic!("expected Sub to be a class object"))
        .mixin
        .clone();
    let sub_application = interp.apply_mixin(&sub_mixin, Some(&sup_application));
    let relinked = interp.heap_mut().set_class_superclass(sub_class, sup_class)
        && interp
            .heap_mut()
            .set_class_application(sub_class, sub_application);
    assert!(relinked, "expected Sub to be a class object");

    let receiver = expect_ok(
        interp.instantiate_unrooted(sub_class, vec![]),
        "instantiate",
    );

    let result = interp.send_unrooted(receiver, Symbol::intern("getB"), vec![]);

    match result {
        Ok(Value::SmallInt(10)) => {}
        Ok(Value::Nil) => panic!(
            "getB returned nil -- the superclass's own slot initializer (a = 10) did not run \
             before the subclass's own slot initializer (b = a) read it"
        ),
        Ok(other) => panic!(
            "expected Value::SmallInt(10), got {}",
            interp.heap_ref().describe(&other)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
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
    let src = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: platform = Object new (|t = platform.|) (public getT = (^t)) : ()
    "};
    let (mut interp, class) = load(src);

    let receiver = expect_ok(
        interp.instantiate_unrooted(class, vec![Value::SmallInt(99)]),
        "instantiate",
    );

    let result = interp.send_unrooted(receiver, Symbol::intern("getT"), vec![]);

    match result {
        Ok(Value::SmallInt(99)) => {}
        Ok(other) => {
            panic!(
                "expected the slot to hold the factory argument (SmallInt(99)), got {}",
                interp.heap_ref().describe(&other)
            )
        }
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

/// The same, but the factory parameter is the *receiver* of a send inside
/// the slot initializer, and the whole thing runs through a real factory
/// send (`M usingPlatform: 99`) rather than a direct `instantiate` call --
/// i.e. exactly the path `ns-next run` takes.
#[test]
fn a_factory_parameter_is_usable_as_a_send_receiver_in_a_slot_initializer() {
    let src = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p = Object new (|t = p printString.|) (public getT = (^t)) : ()
    "};
    let (mut interp, class) = load(src);

    let receiver = expect_ok(
        interp.send_unrooted(
            Value::Obj(class),
            Symbol::intern("usingPlatform:"),
            vec![Value::SmallInt(99)],
        ),
        "M usingPlatform: 99",
    );

    let result = expect_ok(
        interp.send_unrooted(receiver, Symbol::intern("getT"), vec![]),
        "getT",
    );

    match &result {
        Value::Obj(obj) => match interp.heap_ref().str_value(*obj) {
            Some(s) => assert_eq!(s, "99"),
            None => panic!(
                "expected ObjectData::Str(\"99\"), got {}",
                interp.heap_ref().describe(&result)
            ),
        },
        other => panic!(
            "expected Value::Obj(<String>), got {}",
            interp.heap_ref().describe(other)
        ),
    }
}

/// An `initExprs` statement (the class header's trailing statements, spec
/// step 5) sees the factory parameters too -- same activation, same scope.
#[test]
fn a_factory_parameter_is_visible_in_an_init_expr() {
    let src = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p = Object new (|t.| t:: p.) (public getT = (^t)) : ()
    "};
    let module = ns_syntax::parse(src).expect("parse");
    assert_eq!(
        module.root_class.instance_side.init_exprs.len(),
        1,
        "this fixture is only meaningful if the header's `t:: p.` really parsed as an initExpr"
    );
    let (mut interp, class) = load(src);

    let receiver = expect_ok(
        interp.instantiate_unrooted(class, vec![Value::SmallInt(7)]),
        "instantiate",
    );

    let result = interp.send_unrooted(receiver, Symbol::intern("getT"), vec![]);

    match result {
        Ok(Value::SmallInt(7)) => {}
        Ok(other) => panic!(
            "expected SmallInt(7) assigned by the initExpr, got {}",
            interp.heap_ref().describe(&other)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

/// A class declaring a two-keyword factory pattern (`usingPlatform:ide:`),
/// instantiated with only one factory argument. A slot initializer
/// referencing the *second*, missing parameter resolves to
/// `Binding::Local(1)`, but `factory_args` here has only one element and
/// the instance-initializer activation starts with no temporaries -- without
/// the arity check, `locals.rs`'s `read_local` would index straight past
/// both and panic with "index out of bounds" instead of returning a clean
/// `NewspeakError`.
#[test]
fn a_factory_arity_mismatch_is_a_newspeak_error_not_a_panic() {
    let src = indoc! {"
        Newspeak3
        'T'
        class M usingPlatform: p ide: q = Object new (|t = q.|) (public getT = (^t)) : ()
    "};
    let (mut interp, class) = load(src);

    let result = interp.instantiate_unrooted(class, vec![Value::SmallInt(1)]);

    match result {
        Err(Signal::Error(e)) => {
            assert!(
                e.message.contains("wrong number of factory arguments"),
                "unexpected error message: {}",
                e.message
            );
        }
        Ok(v) => panic!(
            "expected a factory-arity NewspeakError, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `instantiate`"),
    }
}

#[test]
fn sending_an_undefined_selector_is_a_newspeak_error_not_a_panic() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new () () : ()
    "};
    let (mut interp, class) = load(src);

    let receiver = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");

    let result = interp.send_unrooted(receiver, Symbol::intern("thisSelectorDoesNotExist"), vec![]);

    match result {
        Err(Signal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(v) => panic!(
            "expected a doesNotUnderstand: error, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
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
    let src = indoc! {"
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
    let (mut interp, class) = load(src);
    let module = expect_ok(
        interp.instantiate_unrooted(class, vec![]),
        "instantiate SuperclassArgs",
    );
    let derived = expect_ok(
        interp.send_unrooted(module, Symbol::intern("Derived"), vec![]),
        "Derived",
    );
    let instance = expect_ok(
        interp.send_unrooted(derived, Symbol::intern("new"), vec![]),
        "Derived new",
    );
    let stored = expect_ok(
        interp.send_unrooted(instance, Symbol::intern("base"), vec![]),
        "base",
    );
    assert!(
        matches!(stored, Value::SmallInt(7)),
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
/// (`Interpreter::descend_levels`): that relies on the compiler ending every
/// initializer by answering `self`. An initializer that answered anything
/// else -- its last statement's value, say, or `nil` -- would fail here.
#[test]
fn new_answers_the_instance_its_initializers_ran_on() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Kinds = Object new (
            | seen ::= nil. |
        ) (
            public class One = Object new (
                | a = 1. |
                note: self.
            ) (
                public a = ( ^a )
            )
            public class Base = Object new (
                | b = 2. |
                note: self.
            ) (
                public b = ( ^b )
            )
            public class Two = Base new (
                | c = 3. |
                note: self.
            ) (
                public c = ( ^c )
            )
            public class Bare = Base new (
            ) (
            )
            note: o = ( seen := o )
            public lastSeen = ( ^seen )
            public make: c = ( ^c new )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load_with(src, lowering);
        let module = expect_ok(
            interp.instantiate_unrooted(class, vec![]),
            "instantiate Kinds",
        );
        let cases: [(&str, &[(&str, i64)]); 3] = [
            ("One", &[("a", 1)]),
            ("Two", &[("b", 2), ("c", 3)]),
            ("Bare", &[("b", 2)]),
        ];
        for (name, slots) in cases {
            let class = expect_ok(
                interp.send_unrooted(module, Symbol::intern(name), vec![]),
                name,
            );
            let Value::Obj(class_obj) = class else {
                panic!("{name} answers a class object")
            };
            for from_newspeak in [false, true] {
                let what = format!("{name} new, {lowering:?}, from Newspeak: {from_newspeak}");
                let instance = if from_newspeak {
                    interp.send_unrooted(module, Symbol::intern("make:"), vec![class])
                } else {
                    interp.send_unrooted(class, Symbol::intern("new"), vec![])
                };
                let instance = expect_ok(instance, &what);
                let seen = expect_ok(
                    interp.send_unrooted(module, Symbol::intern("lastSeen"), vec![]),
                    "lastSeen",
                );
                let (Value::Obj(instance_obj), Value::Obj(seen_obj)) = (instance, seen) else {
                    panic!("{what}: answered something other than an object");
                };
                assert!(
                    instance_obj.same(seen_obj),
                    "{what}: answered an object other than the one its initializers ran on"
                );
                assert!(
                    interp.class_of(&instance).same(class_obj),
                    "{what}: answered an instance of another class"
                );
                for (getter, expected) in slots {
                    let value = expect_ok(
                        interp.send_unrooted(instance, Symbol::intern(getter), vec![]),
                        getter,
                    );
                    assert!(
                        matches!(value, Value::SmallInt(n) if n == *expected),
                        "{what}: slot {getter}"
                    );
                }
            }
        }
    }
}
