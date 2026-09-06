use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::mixin::Mixin;
use ns_runtime::{Interpreter, Symbol};
use ns_syntax::{ast, span::Span};

fn slot_decl(name: &str) -> Rc<ast::SlotDecl> {
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
    Rc::new(Mixin {
        name: Symbol::intern(name),
        methods: HashMap::new(),
        instance_slots: Vec::new(),
        init_exprs: Vec::new(),
        factory: None,
        nested: Vec::new(),
        class_side: None,
        decl: None,
    })
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
    Rc::get_mut(&mut sup).unwrap().instance_slots = vec![slot_decl("a"), slot_decl("b")];
    let sup_app = interp.apply_mixin(&sup, None);

    let mut sub = bare_mixin("Sub");
    Rc::get_mut(&mut sub).unwrap().instance_slots = vec![slot_decl("c")];
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
    let borrowed = class.borrow();
    let ns_runtime::ObjectData::Class(data) = &borrowed.data else {
        panic!("load_module must produce a class object");
    };
    assert_eq!(data.application.num_slots, 2);
    assert!(
        data.enclosing.is_none(),
        "a module's enclosing object is nil"
    );
    assert_eq!(
        data.application.slot_index[&Symbol::intern("y")],
        1,
        "layout lives on the application, not on the class object"
    );
    let _ = interp;
}
