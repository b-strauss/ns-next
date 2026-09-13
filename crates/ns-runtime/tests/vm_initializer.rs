//! Instance initializers: a class's slot initializers and `initExprs`,
//! compiled into one body per mixin and run on each level's own activation.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    BuiltinClass, ClassData, FrameKind, HostResult, HostSignal, Interpreter, Mixin,
    MixinApplication, ObjectData, Root, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

fn load(src: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
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

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

/// Instantiates with `factory_args`, sends `getter`, returns the `SmallInt`
/// it answers.
fn instantiate_and_get(src: &str, factory_args: Vec<Value>, getter: &str) -> i64 {
    let (mut interp, class) = load(src);
    let factory_args: Vec<Root> = factory_args
        .into_iter()
        .map(|arg| interp.stash(arg))
        .collect();
    let instance = expect_ok(interp.instantiate(&class, &factory_args), "instantiate");
    let result = expect_ok(interp.send(&instance, Symbol::intern(getter), &[]), getter);
    interp.enter(|m| int_of(&m.fetch(&result)))
}

const FACTORY_PARAM: &str = indoc! {"
    Newspeak3
    'T'
    class Foo x: a = Object new (
      | p = a. |
    ) (
      public p2 = ( ^p )
    ) : ()
"};
const INIT_EXPRS: &str = indoc! {"
    Newspeak3
    'T'
    class Foo = Object new (
      | q ::= 5. r ::= 0. |
      r := q + 1.
    ) (
      public r2 = ( ^r )
    ) : ()
"};

#[test]
fn a_slot_initializer_reads_a_factory_parameter() {
    let v = instantiate_and_get(FACTORY_PARAM, vec![Value::SmallInt(7)], "p2");
    assert_eq!(v, 7);
}

#[test]
fn init_exprs_run_after_slot_initializers() {
    let v = instantiate_and_get(INIT_EXPRS, vec![], "r2");
    assert_eq!(v, 6);
}

#[test]
fn a_closure_created_in_a_slot_initializer_is_compiled_and_homed_at_the_initializer_frame() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | blk = [ 3 ]. |
        ) (
          public blk2 = ( ^blk )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let v = expect_ok(interp.send(&instance, Symbol::intern("blk2"), &[]), "blk2");
    interp.enter(|m| {
        let Value::Obj(obj) = m.fetch(&v) else {
            panic!("expected a closure object")
        };
        let closure = m
            .heap()
            .closure_data(obj)
            .unwrap_or_else(|| panic!("expected a closure"));
        assert_eq!(closure.code.num_args, 0);
        assert!(matches!(
            m.heap().frame_kind(closure.home),
            FrameKind::Initializer(_)
        ));
    });
}

#[test]
fn a_return_inside_a_block_in_init_exprs_escapes_instantiate() {
    // A known gap (`docs/design/execution.md`, "Non-local return"): the
    // `Signal::Return` targets the initializer activation, which is still
    // running, and nothing catches it: the host receives it as
    // `HostSignal::Return`.
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | |
          [ ^1 ] value.
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let result = interp.instantiate(&class, &[]);
    assert!(matches!(result, Err(HostSignal::Return)));
}

#[test]
fn a_factory_arity_mismatch_is_still_the_shared_error() {
    let (mut interp, class) = load(FACTORY_PARAM);
    match interp.instantiate(&class, &[]) {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("wrong number of factory arguments"),
            "{}",
            e.message
        ),
        _ => panic!("expected the arity error"),
    }
}

// ---------------------------------------------------------------------------
// `InitSlot`'s field index must be relative to the mixin, not baked in
// absolute at compile time.
// ---------------------------------------------------------------------------

/// A bare `SlotDecl` with no initializer -- only its presence (for the slot
/// count) matters here, exactly like `tests/mixin.rs`'s own helper of the
/// same shape.
fn slot_decl(name: &str) -> ns_runtime::mixin::MixinSlot {
    ns_runtime::mixin::MixinSlot::from_decl(raw_slot_decl(name))
}

fn raw_slot_decl(name: &str) -> Rc<ns_syntax::ast::SlotDecl> {
    Rc::new(ns_syntax::ast::SlotDecl {
        access: None,
        name: ns_syntax::ast::Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        },
        mutability: ns_syntax::ast::SlotMutability::Immutable,
        init: None,
        span: Span::new(0, 0),
    })
}

/// A class object wrapping `application` directly -- built without
/// `load_module`/`resolve_superclass_receiver` because the point of this
/// test is to apply *one* mixin to two different superclass shapes, which
/// `load_module` cannot do (it always builds a fresh `Mixin` per module).
fn make_class(
    interp: &mut Interpreter,
    class_of_class: BuiltinClass,
    application: Rc<MixinApplication>,
) -> Root {
    interp.enter(|m| {
        let class_of_class = m.builtin_class(class_of_class);
        let class = m.heap_mut().alloc_object(
            class_of_class,
            ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                // Deliberately no class-object superclass link: this test
                // only checks where the mixin's *own* slot lands, never a
                // superclass level's own slots, so `climb_levels` (which
                // walks this field, not the application's) finding no level
                // above this one is exactly right.
                superclass: None,
                builtin: None,
                metaclass: None,
            }),
        );
        m.stash(Value::Obj(class))
    })
}

/// The `SmallInt` held at `instance`'s field `index` -- `Value` has no
/// `PartialEq`, so this reads through the one variant the test needs
/// instead of comparing `Value`s directly.
fn field_at(heap: &ns_runtime::Heap, instance: &Value, index: usize) -> i64 {
    let Value::Obj(obj) = instance else {
        panic!("expected an instance")
    };
    match heap.instance_field(*obj, index) {
        Some(Value::SmallInt(n)) => n,
        Some(_) => panic!("expected field {index} to hold a SmallInt"),
        None => panic!("expected an Instance payload"),
    }
}

#[test]
fn a_mixin_applied_to_different_superclasses_initializes_its_own_slot_at_each_ones_absolute_offset()
{
    // `InitSlot` must not carry an ABSOLUTE field index, baked in at
    // compile time from whichever class the compiler happens to be called
    // against: the compiled initializer is shared by *every* application of
    // its mixin (it lives on the mixin, not on a class object, per §6.2).
    // Two applications of the SAME mixin to superclasses with different
    // slot counts legitimately disagree about where the mixin's own slot
    // lands, so an absolute index could only be right for one of them. The
    // failure is general, not nested-class-only -- reproducing it needs one
    // `Rc<Mixin>` applied to two different superclasses, which only happens
    // by calling `Interpreter::apply_mixin` directly (`load_module` always
    // builds a fresh `Mixin` per module, so two ordinary loaded classes
    // never share one).
    //
    // The mixin is the one `load_module` built, so the initializer it
    // compiled for it -- keyed by that mixin -- is the one both hand-made
    // applications run.
    let src = indoc! {"
        Newspeak3
        'T'
        class Shared x: a = Object new (
          | v = a. |
        ) (
        ) : ()
    "};
    let (mut interp, loaded) = load(src);
    let shared_mixin = interp.enter(|m| {
        let Value::Obj(loaded) = m.fetch(&loaded) else {
            panic!("load_module answers a class object")
        };
        m.heap()
            .class_application(loaded)
            .expect("a class object")
            .mixin
            .clone()
    });

    // Two structurally different superclass shapes for the same mixin: one
    // application inherits two slots (own_slot_base must be 2), the other
    // inherits none (own_slot_base must be 0).
    let (app_a, app_b) = interp.enter(|m| {
        let two_slots = Rc::new(Mixin {
            slots: vec![slot_decl("p"), slot_decl("q")],
            ..Mixin::empty(Symbol::intern("TwoSlots"))
        });
        let two_slot_super = m.apply_mixin(&two_slots, None);
        let app_a = m.apply_mixin(&shared_mixin, Some(&two_slot_super));
        let app_b = m.apply_mixin(&shared_mixin, None);
        (app_a, app_b)
    });
    assert_ne!(
        app_a.own_slot_base, app_b.own_slot_base,
        "the setup must actually give the two applications different bases"
    );

    let class_a = make_class(&mut interp, BuiltinClass::Object, app_a.clone());
    let class_b = make_class(&mut interp, BuiltinClass::Object, app_b.clone());

    let a = expect_ok(
        interp.instantiate(&class_a, &[interp.stash(Value::SmallInt(11))]),
        "instantiate a",
    );
    let b = expect_ok(
        interp.instantiate(&class_b, &[interp.stash(Value::SmallInt(22))]),
        "instantiate b",
    );
    interp.enter(|m| {
        assert_eq!(
            field_at(m.heap(), &m.fetch(&a), app_a.own_slot_base),
            11,
            "a's own slot must land at its own application's absolute offset"
        );
        assert_eq!(
            field_at(m.heap(), &m.fetch(&b), app_b.own_slot_base),
            22,
            "b's own slot must land at its own application's absolute offset"
        );
    });
}

/// A real three-level chain -- `Leaf` -> `Middle` -> `Base`, each level
/// declaring two slots of its own and passing an argument up through its
/// superclass clause.
///
/// Until the clause's message was evaluated per level, no test ran two
/// compiled `InitSlot` bodies in one chain, so the per-level `slot_base` a
/// mixin-relative `own_index` is offset by was only ever proven by
/// inspection. Here `Base`'s slots must land at 0-1, `Middle`'s at 2-3 and
/// `Leaf`'s at 4-5 of the *same* instance: a level running with another
/// level's base overwrites its superclass's slots instead of its own.
#[test]
fn a_three_level_chain_initializes_every_level_at_its_own_absolute_offsets() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Levels = Object new () (
            public class Base withBase: b = Object new (| b1 ::= b. b2 ::= 10. |) (
            )
            public class Middle withMiddle: m = Base withBase: m + 1 (| m1 ::= m. m2 ::= 20. |) (
            )
            public class Leaf = Middle withMiddle: 2 (| l1 ::= 3. l2 ::= 30. |) (
            )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let module = expect_ok(interp.instantiate(&class, &[]), "instantiate Levels");
    let leaf_class = expect_ok(interp.send(&module, Symbol::intern("Leaf"), &[]), "Leaf");
    let leaf = expect_ok(
        interp.send(&leaf_class, Symbol::intern("new"), &[]),
        "Leaf new",
    );
    // `Middle withMiddle: 2` -> `Base withBase: 2 + 1`, so the deepest
    // level's own argument is the one its own clause computed.
    interp.enter(|m| {
        let leaf = m.fetch(&leaf);
        for (index, expected) in [(0, 3), (1, 10), (2, 2), (3, 20), (4, 3), (5, 30)] {
            assert_eq!(
                field_at(m.heap(), &leaf, index),
                expected,
                "field {index} holds the wrong level's value"
            );
        }
    });
}
