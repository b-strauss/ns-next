//! Instance initializers: a class's slot initializers and `initExprs`,
//! compiled into one body per mixin and run on each level's own activation.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    BuiltinClass, ClassData, ClassRef, FrameKind, Interpreter, Mixin, MixinApplication, ObjectData,
    Signal, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

fn load(src: &str) -> (Interpreter, ClassRef) {
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

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

/// Instantiates with `factory_args`, sends `getter`, returns the result.
fn instantiate_and_get(src: &str, factory_args: Vec<Value>, getter: &str) -> Value {
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(class, factory_args), "instantiate");
    expect_ok(
        interp.send(instance, Symbol::intern(getter), vec![]),
        getter,
    )
}

const FACTORY_PARAM: &str = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. |\n) (\n  public p2 = ( ^p )\n) : ()";
const INIT_EXPRS: &str = "Newspeak3\n'T'\nclass Foo = Object new (\n  | q ::= 5. r ::= 0. |\n  r := q + 1.\n) (\n  public r2 = ( ^r )\n) : ()";

#[test]
fn a_slot_initializer_reads_a_factory_parameter() {
    let v = instantiate_and_get(FACTORY_PARAM, vec![Value::SmallInt(7)], "p2");
    assert_eq!(int_of(&v), 7);
}

#[test]
fn init_exprs_run_after_slot_initializers() {
    let v = instantiate_and_get(INIT_EXPRS, vec![], "r2");
    assert_eq!(int_of(&v), 6);
}

#[test]
fn a_closure_created_in_a_slot_initializer_is_compiled_and_homed_at_the_initializer_frame() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | blk = [ 3 ]. |\n) (\n  public blk2 = ( ^blk )\n) : ()";
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let v = expect_ok(
        interp.send(instance, Symbol::intern("blk2"), vec![]),
        "blk2",
    );
    let Value::Obj(obj) = &v else {
        panic!("expected a closure object")
    };
    let closure = interp
        .heap_ref()
        .closure_data(*obj)
        .unwrap_or_else(|| panic!("expected a closure"));
    assert_eq!(closure.code.num_args, 0);
    assert!(matches!(
        interp.heap_ref().frame_kind(closure.home),
        FrameKind::Initializer(_)
    ));
}

#[test]
fn a_return_inside_a_block_in_init_exprs_escapes_instantiate() {
    // A known gap (`docs/design/execution.md`, "Non-local return"): the
    // `Signal::Return` targets the initializer activation, which is still
    // running, and nothing catches it.
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | |\n  [ ^1 ] value.\n) (\n) : ()";
    let (mut interp, class) = load(src);
    let result = interp.instantiate(class, vec![]);
    assert!(matches!(result, Err(Signal::Return(..))));
}

#[test]
fn a_factory_arity_mismatch_is_still_the_shared_error() {
    let (mut interp, class) = load(FACTORY_PARAM);
    match interp.instantiate(class, vec![]) {
        Err(Signal::Error(e)) => assert!(
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
    class_of_class: ClassRef,
    application: Rc<MixinApplication>,
) -> ClassRef {
    interp.heap_mut().alloc_object(
        class_of_class,
        ObjectData::Class(ClassData {
            application,
            enclosing: None,
            declaring_class: None,
            // Deliberately no class-object superclass link: this test only
            // checks where the mixin's *own* slot lands, never a superclass
            // level's own slots, so `run_initializer_level` (which walks
            // this field, not the application's) finding nothing to recurse
            // into is exactly right.
            superclass: None,
            builtin: None,
            metaclass: None,
        }),
    )
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
    let src = "Newspeak3\n'T'\nclass Shared x: a = Object new (\n  | v = a. |\n) (\n) : ()";
    let (mut interp, loaded) = load(src);
    let shared_mixin = interp
        .heap_ref()
        .class_application(loaded)
        .expect("a class object")
        .mixin
        .clone();
    let object_class = interp.builtin_class(BuiltinClass::Object);

    // Two structurally different superclass shapes for the same mixin: one
    // application inherits two slots (own_slot_base must be 2), the other
    // inherits none (own_slot_base must be 0).
    let two_slots = Rc::new(Mixin {
        slots: vec![slot_decl("p"), slot_decl("q")],
        ..Mixin::empty(Symbol::intern("TwoSlots"))
    });
    let two_slot_super = interp.apply_mixin(&two_slots, None);

    let app_a = interp.apply_mixin(&shared_mixin, Some(&two_slot_super));
    let app_b = interp.apply_mixin(&shared_mixin, None);
    assert_ne!(
        app_a.own_slot_base, app_b.own_slot_base,
        "the setup must actually give the two applications different bases"
    );

    let class_a = make_class(&mut interp, object_class, app_a.clone());
    let class_b = make_class(&mut interp, object_class, app_b.clone());

    let a = expect_ok(
        interp.instantiate(class_a, vec![Value::SmallInt(11)]),
        "instantiate a",
    );
    let b = expect_ok(
        interp.instantiate(class_b, vec![Value::SmallInt(22)]),
        "instantiate b",
    );
    assert_eq!(
        field_at(interp.heap_ref(), &a, app_a.own_slot_base),
        11,
        "a's own slot must land at its own application's absolute offset"
    );
    assert_eq!(
        field_at(interp.heap_ref(), &b, app_b.own_slot_base),
        22,
        "b's own slot must land at its own application's absolute offset"
    );
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
    let src = "\
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
";
    let (mut interp, class) = load(src);
    let module = expect_ok(interp.instantiate(class, vec![]), "instantiate Levels");
    let leaf_class = expect_ok(interp.send(module, Symbol::intern("Leaf"), vec![]), "Leaf");
    let leaf = expect_ok(
        interp.send(leaf_class, Symbol::intern("new"), vec![]),
        "Leaf new",
    );
    // `Middle withMiddle: 2` -> `Base withBase: 2 + 1`, so the deepest
    // level's own argument is the one its own clause computed.
    for (index, expected) in [(0, 3), (1, 10), (2, 2), (3, 20), (4, 3), (5, 30)] {
        assert_eq!(
            field_at(interp.heap_ref(), &leaf, index),
            expected,
            "field {index} holds the wrong level's value"
        );
    }
}
