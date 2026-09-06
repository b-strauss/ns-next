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
    // A superclass application with two slots, a subclass mixin with none:
    // the subclass's layout must start past the inherited ones.
    let mut interp = Interpreter::bootstrap();
    let mut sup = bare_mixin("Sup");
    Rc::get_mut(&mut sup).unwrap().instance_slots = vec![slot_decl("a"), slot_decl("b")];
    let sup_app = interp.apply_mixin(&sup, None);
    let sub_app = interp.apply_mixin(&bare_mixin("Sub"), Some(&sup_app));
    assert_eq!(sup_app.num_slots, 2);
    assert_eq!(sub_app.num_slots, 2);
}
