//! What a class declaration *is* (`Mixin`) and what applying it to a
//! superclass *produces* (`MixinApplication`). Nothing here evaluates
//! Newspeak code: applying a mixin is a total, deterministic operation.
//!
//! The split exists because `newspeak-spec.pdf` §6 requires a distinct class
//! object per enclosing instance, while §6.2 requires all applications of a
//! mixin to share its code and structure. Identity lives on the class object,
//! behaviour lives here, and that is what lets an inline cache stay
//! monomorphic across enclosing instances.
//!
//! **A `Mixin` is not an AST view.** Everything the *running* system reads
//! off a declaration -- its name, its slot names, its method dictionary with
//! each method's accessibility and arity, its primary factory selector and
//! arity, what its superclass clause names -- is stored here in its own
//! right, because an image's mixin table has to rebuild all of it with no
//! `ast::ClassDecl` anywhere in reach. The `ast` fields that remain
//! (`MixinMethod::decl`, `MixinSlot::decl`, `Mixin::init_exprs`,
//! `Mixin::decl`) are read only by the compiler, which runs while a mixin is
//! built from source; a decoded mixin leaves them empty rather than
//! inventing nodes for them. The compiled bodies themselves live on the
//! mixin (`Mixin::initializer_code` and the two superclass-clause bodies)
//! and on its methods (`MixinMethod::code`), for a built mixin and a decoded
//! one alike.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::CompiledCode;
use crate::types::Binding;
use crate::Symbol;

/// One entry of a mixin's method dictionary, with the body it runs.
///
/// The body lives here, on the entry dispatch hands back, rather than in a
/// table beside the mixin: a send that found the method has found its code.
/// That is how every system in `prior-art/` holds it -- Primordial Soup's
/// `Method::bytecode()`, SOMns's `SInvokable.invokable` -- and it means a
/// method cannot exist without a body: one built from source is compiled as
/// it is built, one decoded from an image is handed the body its row names.
#[derive(Debug, Clone)]
pub struct MixinMethod {
    pub selector: Symbol,
    /// §5.2 accessibility, already defaulted through
    /// `dispatch::effective_access`. Read by every lookup, so it is stored
    /// rather than re-derived from `decl`, which a decoded mixin has none of.
    pub access: ast::AccessModifier,
    /// The selector's parameter count -- what a method activation's `args`
    /// are checked against, and what a decoded body's `num_args` is
    /// reconciled against on load.
    pub num_args: usize,
    /// The declaration this entry was built from. `None` for a mixin decoded
    /// from an image.
    pub decl: Option<Rc<ast::MethodDecl>>,
    /// The compiled body, shared by every application of the mixin
    /// (`newspeak-spec.pdf` §6.2). Never replaced: editing a method builds a
    /// new `MixinMethod`.
    pub code: Rc<CompiledCode>,
}

impl MixinMethod {
    /// The entry a parsed declaration becomes, compiled by `lowering` against
    /// the resolver's `bindings` for the module it belongs to.
    pub fn from_decl(
        decl: Rc<ast::MethodDecl>,
        bindings: &Rc<HashMap<Span, Binding>>,
        lowering: crate::compiler::Lowering,
    ) -> MixinMethod {
        MixinMethod {
            selector: Symbol::intern(&crate::interpreter::instantiate::method_name(&decl.pattern)),
            access: crate::dispatch::effective_access(decl.access),
            num_args: crate::interpreter::instantiate::pattern_param_count(&decl.pattern),
            code: Rc::new(crate::compiler::compile_method(&decl, bindings, lowering)),
            decl: Some(decl),
        }
    }
}

/// One of a mixin's own instance slots. The name is what the run time
/// needs (`Interpreter::apply_mixin` lays the instance out by name); the
/// declaration carries the initializer expression, which only the compiler
/// reads.
#[derive(Debug, Clone)]
pub struct MixinSlot {
    pub name: Symbol,
    /// `None` for a slot of a mixin decoded from an image: its initializer
    /// is already compiled into that mixin's initializer body.
    pub decl: Option<Rc<ast::SlotDecl>>,
}

impl MixinSlot {
    pub fn from_decl(decl: Rc<ast::SlotDecl>) -> MixinSlot {
        MixinSlot {
            name: Symbol::intern(&decl.name.name),
            decl: Some(decl),
        }
    }
}

/// A mixin's primary factory message, reduced to the two things the run
/// time asks of it: which selector instantiates it (`newspeak-spec.pdf`
/// §6.1) and how many arguments that selector takes. `None` for a mixin
/// with no factory of its own -- a builtin's, and a class side's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Factory {
    pub selector: Symbol,
    pub num_args: usize,
}

/// A mixin's superclass clause, reduced to what run time reads off it.
///
/// The clause's *expressions* are Newspeak code, compiled per mixin into
/// `Mixin::superclass_clause_code` / `superclass_message_code`; what is
/// left here is the part the orchestration around those bodies needs, which
/// it consults before any body runs. Storing it is what keeps
/// `instantiate::clause_message_body` from having to read
/// `Mixin::decl`: without it, a mixin decoded from an image would fall back
/// to `new` and every class whose superclass has a keyword factory would
/// raise a spurious "not the primary factory message" error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperclassClause {
    /// The factory selector the clause's message names; `new` when the
    /// message is omitted (§6.1: "if the message is omitted, it defaults to
    /// `new`").
    pub selector: Symbol,
    /// Whether that message carries argument expressions, and therefore a
    /// compiled body in `Mixin::superclass_message_code`. The
    /// overwhelmingly common `= Object new` shape has none and runs no code.
    pub has_args: bool,
}

/// `Debug` is derived so `ObjectData::Class` keeps its own derived `Debug`
/// now that a class object's behaviour lives here. Printing one walks the
/// whole superclass chain and every method's AST -- as `ClassData`'s own
/// `Debug` already did before the split.
#[derive(Debug, Clone)]
pub struct Mixin {
    pub name: Symbol,
    /// The accessibility of the implicit accessor this declaration defines
    /// in its enclosing class (§6: "implicitly defines a method with the
    /// same name *and accessibility* in OC"), already defaulted.
    pub access: ast::AccessModifier,
    pub methods: HashMap<Symbol, Rc<MixinMethod>>,
    /// This mixin's own instance slots, in declaration order.
    pub slots: Vec<MixinSlot>,
    pub factory: Option<Factory>,
    pub superclass: Option<SuperclassClause>,
    /// Whether the declaration wrote any `initExpr`s. Stored because
    /// `instantiate::mixin_needs_initializer_code` asks the question of a
    /// decoded mixin too, where `init_exprs` below is empty by construction.
    pub has_init_exprs: bool,
    /// The `initExpr` statements themselves -- lowered by the compiler into the
    /// instance initializer. Empty for a decoded mixin.
    pub init_exprs: Vec<ast::Stmt>,
    /// The nested class declarations, in declaration order. Each one gets an
    /// implicit accessor and a hidden cache slot.
    pub nested: Vec<Rc<Mixin>>,
    pub class_side: Option<Rc<Mixin>>,
    /// Provenance for reflection and the image.
    pub span: Span,
    /// The declaration this mixin was built from: the compiler lowers its
    /// superclass clause. `None` for a builtin's mixin, for a class side (a
    /// `ClassBody` is not an `ast::ClassDecl`), and for a mixin decoded from
    /// an image.
    pub decl: Option<Rc<ast::ClassDecl>>,
    /// The instance initializer: this mixin's own slot initializers, then its
    /// own `initExpr`s. Every mixin built from source has one, a class side
    /// included. `None` for a builtin's mixin and `metaclass_of`'s
    /// placeholder, which run no code, and for a decoded row that names none
    /// -- `image::decode::verify_mixins` refuses that when the mixin needs one.
    pub initializer_code: Option<Rc<CompiledCode>>,
    /// The superclass clause's receiver expression (§6.1's `ec`), which the
    /// nested-class accessor runs once per enclosing object. `None` when the
    /// declaration has no clause.
    pub superclass_clause_code: Option<Rc<CompiledCode>>,
    /// The arguments of the clause's factory message, run on every
    /// instantiation (§6.2). `None` when the message takes none.
    pub superclass_message_code: Option<Rc<CompiledCode>>,
}

impl Mixin {
    /// A mixin with a name and nothing else: the shape every synthesized
    /// mixin starts from -- a builtin's, `metaclass_of`'s class-side
    /// placeholder -- and the base the image decoder fills in field by
    /// field. One definition, so adding a field to `Mixin` cannot silently
    /// leave one of them holding a stale default.
    pub fn empty(name: Symbol) -> Mixin {
        Mixin {
            name,
            access: ast::AccessModifier::Protected,
            methods: HashMap::new(),
            slots: Vec::new(),
            factory: None,
            superclass: None,
            has_init_exprs: false,
            init_exprs: Vec::new(),
            nested: Vec::new(),
            class_side: None,
            span: Span::new(0, 0),
            decl: None,
            initializer_code: None,
            superclass_clause_code: None,
            superclass_message_code: None,
        }
    }
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
    /// for every application of it (it lives on the mixin) and this map is a
    /// per-application answer.
    pub slot_index: HashMap<Symbol, usize>,
    /// Absolute index of the hidden cache slot for each nested class
    /// declaration, including inherited ones. The slot stores the cached
    /// nested class object; accessing it is a slot read rather than a
    /// hash lookup. Inherited declarations keep the index their declaring
    /// level gave them (exactly like inherited slots in `slot_index`).
    ///
    /// Keyed by `mixin_key` -- the declaration's identity -- and not by its
    /// name, which is the one thing a subclass's nested declaration shares
    /// with the inherited one it shadows. Both declarations are real, both
    /// get their own physical slot, and `super N` reaches the inherited one
    /// while `N` reaches the subclass's; a name key would collapse the two
    /// entries onto one index and make whichever was resolved first the
    /// answer to both.
    pub nested_slot: HashMap<usize, usize>,
    pub num_slots: usize,
    /// Where this application's *own* slots start -- `superclass.num_slots`,
    /// or 0 with none. Read at run time
    /// (`Interpreter::descend_levels`, into the frame's `slot_base`)
    /// to turn a compiled `InitSlot`'s mixin-relative `own_index` into this
    /// application's absolute field index: the compiled initializer is
    /// shared by every application of the mixin (keying code by mixin
    /// without this would bake ONE application's offset into code every
    /// application runs), while this field is computed fresh per
    /// application, exactly like `slot_index`.
    pub own_slot_base: usize,
}

/// Unlinks the superclass list in a loop rather than through the stack.
///
/// An application chain is as long as the class chain it mirrors, and once
/// a world is decoded from an image that length is untrusted input --
/// `image::decode::rebuild_applications` builds one link per class record.
/// The default glue would release the chain recursively and overflow the
/// stack on a long one, which aborts the process; this releases it in a
/// loop instead. Objects and frames need no such glue -- both are arena
/// entries with nothing to unwind on drop -- which makes this the only
/// hand-written iterative `Drop` in the crate. It is not the only
/// recursive graph: `Mixin`'s `nested` and `class_side` links are also
/// `Rc` and also drop recursively, but their depth is bounded by the
/// parser and `image::mixins::decode` refuses anything deeper, so a ceiling
/// covers them (`image/mixins.rs`, `MAX_MIXIN_DEPTH`).
///
/// No ceiling here: every chain is still released in full. `Rc::try_unwrap`
/// stops the loop at the first link something else still holds -- the
/// interpreter's own `applications` cache, usually -- because releasing
/// that link is not this drop's business.
impl Drop for MixinApplication {
    fn drop(&mut self) {
        let mut next = self.superclass.take();
        while let Some(application) = next {
            match Rc::try_unwrap(application) {
                Ok(mut owned) => next = owned.superclass.take(),
                Err(_) => break,
            }
        }
    }
}

pub fn mixin_key(mixin: &Rc<Mixin>) -> usize {
    Rc::as_ptr(mixin) as *const () as usize
}

pub fn application_key(application: &Rc<MixinApplication>) -> usize {
    Rc::as_ptr(application) as *const () as usize
}

/// `mixin`, then every nested declaration (recursively), then its class
/// side (recursively) -- the traversal order `image::encode`/`decode` both
/// walk the mixin tree in, so an image's code table and its mixin table
/// always enumerate the same mixins in the same order. Sharing one
/// definition is what keeps the two from drifting apart: the image's mixin
/// table assigns row ids in exactly this order, so a decoded row and the
/// mixin it describes are found at the same position under either walk.
///
/// Recursive, and deliberately so: the trees it walks are either built from
/// a parsed module (whose nesting depth the parser already bounds) or
/// decoded through `image::mixins::decode`, which refuses a tree deeper than
/// its own ceiling. An unbounded tree can never reach this function.
pub fn preorder(mixin: &Rc<Mixin>, out: &mut Vec<Rc<Mixin>>) {
    out.push(mixin.clone());
    for nested in &mixin.nested {
        preorder(nested, out);
    }
    if let Some(class_side) = &mixin.class_side {
        preorder(class_side, out);
    }
}
