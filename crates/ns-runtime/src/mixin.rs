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

/// `Debug` is derived so `ObjectData::Class` keeps its own derived `Debug`
/// now that a class object's behaviour lives here. Printing one walks the
/// whole superclass chain and every method's AST -- as `ClassData`'s own
/// `Debug` already did before the split.
#[derive(Debug)]
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

#[derive(Debug)]
pub struct MixinApplication {
    pub mixin: Rc<Mixin>,
    pub superclass: Option<Rc<MixinApplication>>,
    /// Absolute index of every slot reachable on an instance, inherited ones
    /// included -- resolved through the *current* layout, per the liveness
    /// spec's object-model constraint. Used for by-name slot access
    /// (`dispatch::lookup_slot`, a `SendLexical` read/write of a slot); never
    /// baked into compiled code, since a mixin's *compiled* initializer runs
    /// for every application of it (`compile_module` keys by `mixin_key`) and
    /// this map is a per-application answer.
    pub slot_index: HashMap<Symbol, usize>,
    /// Absolute index of the hidden cache slot for each nested class
    /// declaration, including inherited ones. The slot stores the cached
    /// nested class object; accessing it is a slot read rather than a
    /// hash lookup. Inherited declarations keep the index their declaring
    /// level gave them (exactly like inherited slots in slot_index).
    pub nested_slot: HashMap<Symbol, usize>,
    pub num_slots: usize,
    /// Where this application's *own* slots start -- `superclass.num_slots`,
    /// or 0 with none. Read at run time (`Interpreter::run_own_initializer`)
    /// to turn a compiled `InitSlot`'s mixin-relative `own_index` into this
    /// application's absolute field index: the compiled initializer is
    /// shared by every application of the mixin (review finding: keying
    /// code by mixin without this would bake ONE application's offset into
    /// code every application runs), while this field is computed fresh per
    /// application, exactly like `slot_index` already was.
    pub own_slot_base: usize,
}

pub fn mixin_key(mixin: &Rc<Mixin>) -> usize {
    Rc::as_ptr(mixin) as *const () as usize
}

pub fn application_key(application: &Rc<MixinApplication>) -> usize {
    Rc::as_ptr(application) as *const () as usize
}

/// `mixin`, then every nested declaration (recursively), then its class
/// side (recursively) -- the traversal order `compiler::compile_module` and
/// `image::encode`/`decode` both walk the mixin tree in, so a compiled
/// module and an image's code table always enumerate the same mixins in the
/// same order. Sharing one definition is what keeps the two from drifting
/// apart: an image's per-mixin sections carry no name or path of their own
/// (mixin identity does not survive a save/reload), so decode reconstructs
/// which section belongs to which reloaded mixin purely by replaying this
/// same order over the reloaded tree.
pub fn preorder(mixin: &Rc<Mixin>, out: &mut Vec<Rc<Mixin>>) {
    out.push(mixin.clone());
    for nested in &mixin.nested {
        preorder(nested, out);
    }
    if let Some(class_side) = &mixin.class_side {
        preorder(class_side, out);
    }
}
