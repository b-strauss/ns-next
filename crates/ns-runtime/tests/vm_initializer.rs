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

fn integer_of<'gc>(value: &Value<'gc>) -> i64 {
    match value {
        Value::SmallInteger(integer) => *integer,
        _ => panic!("expected a SmallInteger"),
    }
}

/// Instantiates with `factory_arguments`, sends `getter`, returns the
/// `SmallInteger` it answers.
fn instantiate_and_get(source: &str, factory_arguments: Vec<Value<'static>>, getter: &str) -> i64 {
    let (mut interpreter, class) = load(source);
    let factory_arguments: Vec<Root> = factory_arguments
        .into_iter()
        .map(|argument| interpreter.stash(argument))
        .collect();
    let instance = expect_ok(
        interpreter.instantiate(&class, &factory_arguments),
        "instantiate",
    );
    let result = expect_ok(
        interpreter.send(&instance, Symbol::intern(getter), &[]),
        getter,
    );
    interpreter.enter(|mutator| integer_of(&mutator.fetch(&result)))
}

const FACTORY_PARAMETER: &str = indoc! {"
    Newspeak3
    'T'
    class Foo x: a = Object new (
      | p = a. |
    ) (
      public p2 = ( ^p )
    ) : ()
"};
const INITIALIZER_EXPRESSIONS: &str = indoc! {"
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
    let value = instantiate_and_get(FACTORY_PARAMETER, vec![Value::SmallInteger(7)], "p2");
    assert_eq!(value, 7);
}

#[test]
fn initializer_expressions_run_after_slot_initializers() {
    let value = instantiate_and_get(INITIALIZER_EXPRESSIONS, vec![], "r2");
    assert_eq!(value, 6);
}

#[test]
fn a_closure_created_in_a_slot_initializer_is_compiled_and_homed_at_the_initializer_frame() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | blk = [ 3 ]. |
        ) (
          public blk2 = ( ^blk )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let value = expect_ok(
        interpreter.send(&instance, Symbol::intern("blk2"), &[]),
        "blk2",
    );
    interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&value) else {
            panic!("expected a closure object")
        };
        let closure = mutator
            .heap()
            .closure_data(object)
            .unwrap_or_else(|| panic!("expected a closure"));
        assert_eq!(closure.code.argument_count, 0);
        assert!(matches!(
            mutator.heap().frame_kind(closure.home),
            FrameKind::Initializer(_)
        ));
    });
}

#[test]
fn a_return_inside_a_block_in_initializer_expressions_escapes_instantiate() {
    // A known gap (`docs/design/execution.md`, "Non-local return"): the
    // `Signal::Return` targets the initializer activation, which is still
    // running, and nothing catches it: the host receives it as
    // `HostSignal::Return`.
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | |
          [ ^1 ] value.
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let result = interpreter.instantiate(&class, &[]);
    assert!(matches!(result, Err(HostSignal::Return)));
}

#[test]
fn a_factory_arity_mismatch_is_still_the_shared_error() {
    let (mut interpreter, class) = load(FACTORY_PARAMETER);
    match interpreter.instantiate(&class, &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("wrong number of factory arguments"),
            "{}",
            error.message
        ),
        _ => panic!("expected the arity error"),
    }
}

// ---------------------------------------------------------------------------
// `InitializeSlot`'s field index must be relative to the mixin, not baked in
// absolute at compile time.
// ---------------------------------------------------------------------------

/// A bare `SlotDeclaration` with no initializer -- only its presence (for the
/// slot count) matters here, exactly like `tests/mixin.rs`'s own helper of the
/// same shape.
fn slot_declaration(name: &str) -> ns_runtime::mixin::MixinSlot {
    ns_runtime::mixin::MixinSlot::from_declaration(raw_slot_declaration(name))
}

fn raw_slot_declaration(name: &str) -> Rc<ns_syntax::ast::SlotDeclaration> {
    Rc::new(ns_syntax::ast::SlotDeclaration {
        access: None,
        name: ns_syntax::ast::Identifier {
            name: name.to_string(),
            span: Span::new(0, 0),
        },
        mutability: ns_syntax::ast::SlotMutability::Immutable,
        initializer: None,
        span: Span::new(0, 0),
    })
}

/// A class object wrapping `application` directly -- built without
/// `load_module`/`resolve_superclass_receiver` because the point of this
/// test is to apply *one* mixin to two different superclass shapes, which
/// `load_module` cannot do (it always builds a fresh `Mixin` per module).
fn make_class(
    interpreter: &mut Interpreter,
    class_of_class: BuiltinClass,
    application: Rc<MixinApplication>,
) -> Root {
    interpreter.enter(|mutator| {
        let class_of_class = mutator.builtin_class(class_of_class);
        let class = mutator.heap_mut().allocate_object(
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
        mutator.stash(Value::Object(class))
    })
}

/// The `SmallInteger` held at `instance`'s field `index` -- `Value` has no
/// `PartialEq`, so this reads through the one variant the test needs
/// instead of comparing `Value`s directly.
fn field_at<'gc>(heap: &ns_runtime::Heap<'gc>, instance: &Value<'gc>, index: usize) -> i64 {
    let Value::Object(object) = instance else {
        panic!("expected an instance")
    };
    match heap.instance_field(*object, index) {
        Some(Value::SmallInteger(integer)) => integer,
        Some(_) => panic!("expected field {index} to hold a SmallInteger"),
        None => panic!("expected an Instance payload"),
    }
}

#[test]
fn a_mixin_applied_to_different_superclasses_initializes_its_own_slot_at_each_ones_absolute_offset()
{
    // `InitializeSlot` must not carry an ABSOLUTE field index, baked in at
    // compile time from whichever class the compiler happens to be called
    // against: the compiled initializer is shared by *every* application of
    // its mixin (it lives on the mixin, not on a class object, per §6.2).
    // Two applications of the SAME mixin to superclasses with different
    // slot counts legitimately disagree about where the mixin's own slot
    // lands, so an absolute index could only be right for one of them. The
    // failure is general, not nested-class-only -- reproducing it needs one
    // `Rc<Mixin>` applied to two different superclasses, which only happens
    // by calling `Mutator::apply_mixin` directly (`load_module` always
    // builds a fresh `Mixin` per module, so two ordinary loaded classes
    // never share one).
    //
    // The mixin is the one `load_module` built, so the initializer it
    // compiled for it -- keyed by that mixin -- is the one both hand-made
    // applications run.
    let source = indoc! {"
        Newspeak3
        'T'
        class Shared x: a = Object new (
          | v = a. |
        ) (
        ) : ()
    "};
    let (mut interpreter, loaded) = load(source);
    let shared_mixin = interpreter.enter(|mutator| {
        let Value::Object(loaded) = mutator.fetch(&loaded) else {
            panic!("load_module answers a class object")
        };
        mutator
            .heap()
            .class_application(loaded)
            .expect("a class object")
            .mixin
            .clone()
    });

    // Two structurally different superclass shapes for the same mixin: one
    // application inherits two slots (own_slot_base must be 2), the other
    // inherits none (own_slot_base must be 0).
    let (application_a, application_b) = interpreter.enter(|mutator| {
        let two_slots = Rc::new(Mixin {
            slots: vec![slot_declaration("p"), slot_declaration("q")],
            ..Mixin::empty(Symbol::intern("TwoSlots"))
        });
        let two_slot_super = mutator.apply_mixin(&two_slots, None);
        let application_a = mutator.apply_mixin(&shared_mixin, Some(&two_slot_super));
        let application_b = mutator.apply_mixin(&shared_mixin, None);
        (application_a, application_b)
    });
    assert_ne!(
        application_a.own_slot_base, application_b.own_slot_base,
        "the setup must actually give the two applications different bases"
    );

    let class_a = make_class(
        &mut interpreter,
        BuiltinClass::Object,
        application_a.clone(),
    );
    let class_b = make_class(
        &mut interpreter,
        BuiltinClass::Object,
        application_b.clone(),
    );

    let instance_a = expect_ok(
        interpreter.instantiate(&class_a, &[interpreter.stash(Value::SmallInteger(11))]),
        "instantiate a",
    );
    let instance_b = expect_ok(
        interpreter.instantiate(&class_b, &[interpreter.stash(Value::SmallInteger(22))]),
        "instantiate b",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            field_at(
                mutator.heap(),
                &mutator.fetch(&instance_a),
                application_a.own_slot_base
            ),
            11,
            "a's own slot must land at its own application's absolute offset"
        );
        assert_eq!(
            field_at(
                mutator.heap(),
                &mutator.fetch(&instance_b),
                application_b.own_slot_base
            ),
            22,
            "b's own slot must land at its own application's absolute offset"
        );
    });
}

/// A real three-level chain -- `Leaf` -> `Middle` -> `Base`, each level
/// declaring two slots of its own and passing an argument up through its
/// superclass clause.
///
/// Until the clause's message was evaluated per level, no test ran two compiled
/// `InitializeSlot` bodies in one chain, so the per-level `slot_base` a
/// mixin-relative `own_index` is offset by was only ever proven by inspection.
/// Here `Base`'s slots must land at 0-1, `Middle`'s at 2-3 and `Leaf`'s at 4-5
/// of the *same* instance: a level running with another level's base overwrites
/// its superclass's slots instead of its own.
#[test]
fn a_three_level_chain_initializes_every_level_at_its_own_absolute_offsets() {
    let source = indoc! {"
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
    let (mut interpreter, class) = load(source);
    let module = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Levels");
    let leaf_class = expect_ok(
        interpreter.send(&module, Symbol::intern("Leaf"), &[]),
        "Leaf",
    );
    let leaf = expect_ok(
        interpreter.send(&leaf_class, Symbol::intern("new"), &[]),
        "Leaf new",
    );
    // `Middle withMiddle: 2` -> `Base withBase: 2 + 1`, so the deepest
    // level's own argument is the one its own clause computed.
    interpreter.enter(|mutator| {
        let leaf = mutator.fetch(&leaf);
        for (index, expected) in [(0, 3), (1, 10), (2, 2), (3, 20), (4, 3), (5, 30)] {
            assert_eq!(
                field_at(mutator.heap(), &leaf, index),
                expected,
                "field {index} holds the wrong level's value"
            );
        }
    });
}
