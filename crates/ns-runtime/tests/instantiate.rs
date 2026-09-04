//! End-to-end class instantiation tests: `ns_syntax::parse` -> `resolve` ->
//! `Interpreter::bootstrap()` -> `load_module` -> `instantiate` -> `send`.
//! See `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md`'s
//! Class Instantiation section for the six numbered steps this exercises.

use ns_runtime::{Interpreter, Signal, Symbol, Value};

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both.
fn load(
    src: &str,
) -> (
    Interpreter,
    ns_runtime::ClassRef,
    ns_runtime::ResolvedModule,
) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, resolved)
}

/// `Signal` carries no `Debug` impl (it holds a `FrameRef` cycle-shaped for
/// non-local return, not meant for printing), so `Result::expect` isn't
/// usable on an `EvalResult` directly -- unwrap by hand, with a message
/// naming which `Signal` variant leaked instead.
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
    let src = "Newspeak3\n'T'\nclass Foo = Object new (|x = 42.|) (public bar = (^x)) : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("bar"),
        vec![],
        &resolved.bindings,
        None,
    );

    match result {
        Ok(Value::SmallInt(42)) => {}
        Ok(other) => panic!("expected Value::SmallInt(42), got {other:?}"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

/// A subclass whose own slot initializer references a slot the superclass
/// set up -- proves Class Instantiation step 3 (the superclass's own
/// instance initializer) really runs *before* step 4 (this class's own
/// slots), not merely that both slots end up set eventually.
///
/// `Interpreter::instantiate`'s `run_superclass_initializer`/`run_own_slots`
/// step ordering is what this test actually exercises. Linking the two
/// classes together, though, runs into a real Milestone 1 gap flagged in
/// `interpreter/instantiate.rs`'s `resolve_superclass_receiver`: a
/// superclass clause naming a sibling *user-defined* class (as opposed to a
/// builtin) by name is not resolved there -- only a fixed list of builtin
/// names is. Rather than expanding that (out of this task's scope -- the
/// brief calls it out explicitly as "extend here if a fixture needs it",
/// and this fixture can route around it), `Sub` is parsed with a builtin
/// `Object` superclass clause (so `load_class` resolves cleanly through the
/// ordinary pipeline), and its `ClassData.superclass`/`slot_index` are then
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
    let sup_src = "Newspeak3\n'T'\nclass Sup = Object new (|a = 10.|) () : ()";
    let (mut interp, sup_class, _sup_resolved) = load(sup_src);

    let sub_src = "Newspeak3\n'T'\nclass Sub = Object new (|b = a.|) (public getB = (^b)) : ()";
    let sub_module = ns_syntax::parse(sub_src).expect("parse");
    let sub_resolved = ns_runtime::resolve(
        sub_module,
        sub_src.into(),
        ns_runtime::SourceOrigin::File("sub.ns".into()),
    )
    .expect("resolve");
    let sub_class = interp.load_module(&sub_resolved);

    // Relink Sub -> Sup (see the doc comment above) and fix up Sub's own
    // slot_index offset now that it sits past Sup's one inherited slot.
    {
        let mut borrowed = sub_class.borrow_mut();
        if let ns_runtime::ObjectData::Class(data) = &mut borrowed.data {
            data.superclass = Some(sup_class.clone());
            data.slot_index.clear();
            data.slot_index.insert(Symbol::intern("b"), 1);
        } else {
            panic!("expected Sub to be a class object");
        }
    }

    let receiver = expect_ok(
        interp.instantiate(sub_class, vec![], &sub_resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("getB"),
        vec![],
        &sub_resolved.bindings,
        None,
    );

    match result {
        Ok(Value::SmallInt(10)) => {}
        Ok(Value::Nil) => panic!(
            "getB returned nil -- the superclass's own slot initializer (a = 10) did not run \
             before the subclass's own slot initializer (b = a) read it"
        ),
        Ok(other) => panic!("expected Value::SmallInt(10), got {other:?}"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

/// Final-review finding C2: the class's *own* factory parameters are locals
/// in the instance-initializer activation (the runtime spec's Class
/// Instantiation section, step 2 -- "the same arguments as the factory
/// message"), so a slot initializer may reference them. This is the
/// `class M usingPlatform: platform = Object new (| t = platform ... |)`
/// idiom every real Newspeak module is built out of; before the fix the
/// resolver pushed an empty `LocalScope` here and `platform` fell through to
/// a `LexicalSend`, failing at runtime with `doesNotUnderstand: #platform`.
#[test]
fn a_factory_parameter_is_visible_in_a_slot_initializer() {
    let src = "Newspeak3\n'T'\nclass M usingPlatform: platform = Object new (|t = platform.|) (public getT = (^t)) : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![Value::SmallInt(99)], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("getT"),
        vec![],
        &resolved.bindings,
        None,
    );

    match result {
        Ok(Value::SmallInt(99)) => {}
        Ok(other) => {
            panic!("expected the slot to hold the factory argument (SmallInt(99)), got {other:?}")
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
    let src = "Newspeak3\n'T'\nclass M usingPlatform: p = Object new (|t = p printString.|) (public getT = (^t)) : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.send(
            Value::Obj(class),
            Symbol::intern("usingPlatform:"),
            vec![Value::SmallInt(99)],
            &resolved.bindings,
            None,
        ),
        "M usingPlatform: 99",
    );

    let result = expect_ok(
        interp.send(
            receiver,
            Symbol::intern("getT"),
            vec![],
            &resolved.bindings,
            None,
        ),
        "getT",
    );

    match &result {
        Value::Obj(obj) => match &obj.borrow().data {
            ns_runtime::ObjectData::Str(s) => assert_eq!(s, "99"),
            other => panic!("expected ObjectData::Str(\"99\"), got {other:?}"),
        },
        other => panic!("expected Value::Obj(<String>), got {other:?}"),
    }
}

/// An `initExprs` statement (the class header's trailing statements, spec
/// step 5) sees the factory parameters too -- same activation, same scope.
#[test]
fn a_factory_parameter_is_visible_in_an_init_expr() {
    let src = "Newspeak3\n'T'\nclass M usingPlatform: p = Object new (|t.| t:: p.) (public getT = (^t)) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    assert_eq!(
        module.root_class.instance_side.init_exprs.len(),
        1,
        "this fixture is only meaningful if the header's `t:: p.` really parsed as an initExpr"
    );
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![Value::SmallInt(7)], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("getT"),
        vec![],
        &resolved.bindings,
        None,
    );

    match result {
        Ok(Value::SmallInt(7)) => {}
        Ok(other) => panic!("expected SmallInt(7) assigned by the initExpr, got {other:?}"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn sending_an_undefined_selector_is_a_newspeak_error_not_a_panic() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new () () : ()";
    let (mut interp, class, resolved) = load(src);

    let receiver = expect_ok(
        interp.instantiate(class, vec![], &resolved.bindings),
        "instantiate",
    );

    let result = interp.send(
        receiver,
        Symbol::intern("thisSelectorDoesNotExist"),
        vec![],
        &resolved.bindings,
        None,
    );

    match result {
        Err(Signal::Error(e)) => {
            assert_eq!(e.selector, Symbol::intern("thisSelectorDoesNotExist"));
        }
        Ok(v) => panic!("expected a doesNotUnderstand: error, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return escaping `send`"),
    }
}
