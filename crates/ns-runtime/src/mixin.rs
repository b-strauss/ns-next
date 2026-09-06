//! What a class declaration *is* (`Mixin`) and what applying it to a
//! superclass *produces* (`MixinApplication`). Nothing here evaluates
//! Newspeak code: applying a mixin is a total, deterministic operation.
//!
//! The split exists because `newspeak-spec.pdf` §6 requires a distinct class
//! object per enclosing instance, while §6.2 requires all applications of a
//! mixin to share its code and structure. Identity lives on the class object,
//! behaviour lives here, and that is what lets an inline cache stay
//! monomorphic across enclosing instances.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;

use crate::Symbol;

pub struct Mixin {
    pub name: Symbol,
    pub methods: HashMap<Symbol, Rc<ast::MethodDecl>>,
    pub instance_slots: Vec<Rc<ast::SlotDecl>>,
    pub init_exprs: Vec<ast::Stmt>,
    pub factory: Option<Rc<ast::MessagePattern>>,
    /// The nested class declarations, in declaration order. Each one gets an
    /// implicit accessor and a hidden cache slot (Task 4).
    pub nested: Vec<Rc<Mixin>>,
    pub class_side: Option<Rc<Mixin>>,
    /// Provenance for reflection and the image; `None` for builtin classes.
    pub decl: Option<Rc<ast::ClassDecl>>,
}

pub struct MixinApplication {
    pub mixin: Rc<Mixin>,
    pub superclass: Option<Rc<MixinApplication>>,
    /// Absolute index of every slot reachable on an instance, inherited ones
    /// included -- resolved through the *current* layout, per the liveness
    /// spec's object-model constraint.
    pub slot_index: HashMap<Symbol, usize>,
    pub num_slots: usize,
}

pub fn mixin_key(mixin: &Rc<Mixin>) -> usize {
    Rc::as_ptr(mixin) as *const () as usize
}

pub fn application_key(application: &Rc<MixinApplication>) -> usize {
    Rc::as_ptr(application) as *const () as usize
}
