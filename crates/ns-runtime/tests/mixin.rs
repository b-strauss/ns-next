use std::rc::Rc;

use ns_runtime::mixin::{mixin_key, Mixin};
use ns_runtime::{Interpreter, Symbol};
use ns_syntax::{ast, span::Span};

fn slot_decl(name: &str) -> ns_runtime::mixin::MixinSlot {
    ns_runtime::mixin::MixinSlot {
        name: Symbol::intern(name),
        decl: Some(raw_slot_decl(name)),
    }
}

fn raw_slot_decl(name: &str) -> Rc<ast::SlotDecl> {
    Rc::new(ast::SlotDecl {
        access: None,
        name: ast::Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        },
        mutability: ast::SlotMutability::Immutable,
        init: None,
        span: Span::new(0, 0),
    })
}

fn bare_mixin(name: &str) -> Rc<Mixin> {
    Rc::new(Mixin::empty(Symbol::intern(name)))
}

#[test]
fn applying_the_same_mixin_to_the_same_superclass_is_cached() {
    let mut interp = Interpreter::bootstrap();
    let m = bare_mixin("Inner");
    let a = interp.apply_mixin(&m, None);
    let b = interp.apply_mixin(&m, None);
    assert!(
        Rc::ptr_eq(&a, &b),
        "the application cache must return one behaviour per (mixin, super)"
    );
}

#[test]
fn different_superclasses_produce_different_applications() {
    let mut interp = Interpreter::bootstrap();
    let sup_a = interp.apply_mixin(&bare_mixin("A"), None);
    let sup_b = interp.apply_mixin(&bare_mixin("B"), None);
    let m = bare_mixin("Inner");
    let x = interp.apply_mixin(&m, Some(&sup_a));
    let y = interp.apply_mixin(&m, Some(&sup_b));
    assert!(!Rc::ptr_eq(&x, &y));
}

#[test]
fn inherited_slots_are_counted_before_own_slots() {
    // A superclass application with two slots (a, b), a subclass mixin with one slot (c).
    // The subclass's own slots must start past the inherited ones, and slot_index must
    // map both inherited and own slots to their absolute offsets.
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    Rc::get_mut(&mut sup).unwrap().slots = vec![slot_decl("a"), slot_decl("b")];
    let sup_app = interp.apply_mixin(&sup, None);

    let mut sub = bare_mixin("Sub");
    Rc::get_mut(&mut sub).unwrap().slots = vec![slot_decl("c")];
    let sub_app = interp.apply_mixin(&sub, Some(&sup_app));

    // Slot count includes inherited and own.
    assert_eq!(sup_app.num_slots, 2, "superclass should have 2 slots");
    assert_eq!(
        sub_app.num_slots, 3,
        "subclass should have 3 slots (2 inherited + 1 own)"
    );

    // Inherited slots must be reachable by name and map to their declaring level's index.
    assert_eq!(
        sub_app.slot_index.get(&Symbol::intern("a")),
        Some(&0),
        "inherited slot 'a' must resolve to index 0"
    );
    assert_eq!(
        sub_app.slot_index.get(&Symbol::intern("b")),
        Some(&1),
        "inherited slot 'b' must resolve to index 1"
    );

    // Own slots must land past inherited ones.
    assert_eq!(
        sub_app.slot_index.get(&Symbol::intern("c")),
        Some(&2),
        "own slot 'c' must resolve to index 2 (past the 2 inherited slots)"
    );
}

/// Parses+resolves a one-class module and loads it (via a freshly
/// bootstrapped `Interpreter`) into a real class object, returning both.
/// Same shape as `tests/instantiate.rs`'s helper.
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

#[test]
fn a_class_object_carries_identity_and_its_application_carries_layout() {
    let src = "Newspeak3\n'Test'\nclass Point new = Object new (| x. y. |) ()\n";
    let (interp, class, _resolved) = load(src);
    let application = interp
        .heap_ref()
        .class_application(&class)
        .unwrap_or_else(|| panic!("load_module must produce a class object"));
    assert_eq!(application.num_slots, 2);
    assert!(
        interp.heap_ref().class_enclosing(&class).is_none(),
        "a module's enclosing object is nil"
    );
    assert_eq!(
        application.slot_index[&Symbol::intern("y")],
        1,
        "layout lives on the application, not on the class object"
    );
}

const NESTED_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Object new (| v. |) (
        public value = ( ^v )
    )
    public classSideMarker = ( ^1 )
) : (
    public marker = ( ^2 )
)
";

#[test]
fn the_mixin_tree_contains_nested_and_class_side_declarations() {
    let (interp, class, _resolved) = load(NESTED_SRC);
    let application = interp
        .heap_ref()
        .class_application(&class)
        .unwrap_or_else(|| panic!("expected a class object"));
    let mixin = &application.mixin;
    assert_eq!(mixin.nested.len(), 1, "Inner must be in the mixin tree");
    assert_eq!(mixin.nested[0].name, Symbol::intern("Inner"));
    assert!(
        mixin.class_side.is_some(),
        "a class side is a mixin now, not something to discard"
    );
}

#[test]
fn a_nested_declaration_reserves_a_hidden_cache_slot() {
    let (interp, class, _resolved) = load(NESTED_SRC);
    let app = interp
        .heap_ref()
        .class_application(&class)
        .unwrap_or_else(|| panic!("expected a class object"));
    assert_eq!(app.num_slots, 1, "Outer declares no slots but nests Inner");
    let inner = &app.mixin.nested[0];
    assert_eq!(inner.name, Symbol::intern("Inner"));
    assert_eq!(app.nested_slot[&mixin_key(inner)], 0);
}

#[test]
fn inherited_nested_declarations_keep_their_declaring_levels_slot() {
    // A superclass that declares one slot and nests one class; a subclass
    // that declares one slot of its own. The inherited hidden slot must keep
    // the index its declaring level gave it, and the subclass's own slot must
    // land after it.
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    {
        let sup = Rc::get_mut(&mut sup).unwrap();
        sup.slots = vec![slot_decl("a")];
        sup.nested = vec![bare_mixin("Inner")];
    }
    let sup_inner = sup.nested[0].clone();
    let sup_app = interp.apply_mixin(&sup, None);
    assert_eq!(sup_app.slot_index[&Symbol::intern("a")], 0);
    assert_eq!(sup_app.nested_slot[&mixin_key(&sup_inner)], 1);
    assert_eq!(sup_app.num_slots, 2);

    let mut sub = bare_mixin("Sub");
    Rc::get_mut(&mut sub).unwrap().slots = vec![slot_decl("own")];
    let sub_app = interp.apply_mixin(&sub, Some(&sup_app));
    assert_eq!(
        sub_app.nested_slot[&mixin_key(&sup_inner)],
        1,
        "an inherited nested declaration keeps its declaring level's slot"
    );
    assert_eq!(sub_app.slot_index[&Symbol::intern("own")], 2);
    assert_eq!(sub_app.num_slots, 3);
}

/// A subclass redeclaring a nested class its superclass already declares.
/// Both declarations are real members -- `N` finds the subclass's, `super N`
/// the superclass's -- so each needs its own cache slot; sharing one would
/// make whichever was reached first the answer to both.
#[test]
fn a_redeclared_nested_class_gets_its_own_cache_slot() {
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    Rc::get_mut(&mut sup).unwrap().nested = vec![bare_mixin("Inner")];
    let sup_inner = sup.nested[0].clone();
    let sup_app = interp.apply_mixin(&sup, None);

    let mut sub = bare_mixin("Sub");
    Rc::get_mut(&mut sub).unwrap().nested = vec![bare_mixin("Inner")];
    let sub_inner = sub.nested[0].clone();
    let sub_app = interp.apply_mixin(&sub, Some(&sup_app));

    assert_eq!(sub_app.num_slots, 2, "one hidden slot per declaration");
    assert_eq!(sub_app.nested_slot[&mixin_key(&sup_inner)], 0);
    assert_eq!(sub_app.nested_slot[&mixin_key(&sub_inner)], 1);
}
