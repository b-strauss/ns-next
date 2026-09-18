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
//! **A `Mixin` is not an AST view.** Everything the *running* system reads off
//! a declaration -- its name, its slot names, its method dictionary with each
//! method's accessibility and arity, its primary factory selector and arity,
//! what its superclass clause names -- is stored here in its own right, because
//! an image's mixin table has to rebuild all of it with no
//! `ast::ClassDeclaration` anywhere in reach. The `ast` fields that remain
//! (`MixinMethod::declaration`, `MixinSlot::declaration`,
//! `Mixin::initializer_expressions`, `Mixin::declaration`) are read only by the
//! compiler, which runs while a mixin is built from source; a decoded mixin
//! leaves them empty rather than inventing nodes for them. The compiled bodies
//! themselves live on the mixin (`Mixin::initializer_code` and the two
//! superclass-clause bodies) and on its methods (`MixinMethod::code`), for a
//! built mixin and a decoded one alike.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::types::Binding;
use crate::Symbol;

/// The guard an inline cache compares: a dense number, minted per heap, that
/// names one mixin application's behaviour for its instances or for its
/// class objects (`docs/design/dispatch.md`, "The dispatch id").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchId(u32);

impl DispatchId {
    /// What `Heap::dispatch_id` answers for a value whose class has no
    /// application -- only a crafted image makes one -- and what an
    /// application built outside `apply_mixin` carries. Never cached.
    pub const NONE: DispatchId = DispatchId(u32::MAX);
    /// The guard of a cache entry that guards nothing: a vacant, polymorphic
    /// or megamorphic site. `Heap::dispatch_id` never answers it, so no
    /// receiver matches one.
    pub(crate) const VACANT: DispatchId = DispatchId(u32::MAX - 1);

    pub(crate) fn from_counter(value: u32) -> DispatchId {
        debug_assert!(value < DispatchId::VACANT.0);
        DispatchId(value)
    }

    /// The class-side id that goes with this instances id, derived rather
    /// than stored: `mint_dispatch_ids` always mints a pair consecutively
    /// (instances first, class side immediately after), so `ClassData` needs
    /// to keep only the instances id and this recovers the other -- which is
    /// what keeps a class object the size `CLAUDE.md`'s image-amplification
    /// paragraph assumes. `NONE` maps to `NONE`, since there is no pair to
    /// derive one from.
    pub(crate) fn class_side(self) -> DispatchId {
        if self == DispatchId::NONE {
            DispatchId::NONE
        } else {
            DispatchId(self.0 + 1)
        }
    }
}

/// An application's two ids: one for its instances, one for its class
/// objects' class side, since every class object shares one placeholder
/// class and its behaviour depends on its application alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchIds {
    pub instances: DispatchId,
    pub class_side: DispatchId,
}

impl DispatchIds {
    pub const NONE: DispatchIds = DispatchIds {
        instances: DispatchId::NONE,
        class_side: DispatchId::NONE,
    };
}

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
    /// rather than re-derived from `declaration`, which a decoded mixin has
    /// none of.
    pub access: ast::AccessModifier,
    /// The selector's parameter count -- what a method activation's `arguments`
    /// are checked against, and what a decoded body's `argument_count` is
    /// reconciled against on load.
    pub argument_count: usize,
    /// The declaration this entry was built from. `None` for a mixin decoded
    /// from an image.
    pub declaration: Option<Rc<ast::MethodDeclaration>>,
    /// The compiled body, shared by every application of the mixin
    /// (`newspeak-spec.pdf` §6.2). Never replaced: editing a method builds a
    /// new `MixinMethod`.
    pub code: Rc<CompiledCode>,
}

impl MixinMethod {
    /// The entry a parsed declaration becomes, compiled by `lowering` against
    /// the resolver's `bindings` for the module it belongs to.
    pub fn from_declaration(
        declaration: Rc<ast::MethodDeclaration>,
        bindings: &Rc<HashMap<Span, Binding>>,
        lowering: crate::compiler::Lowering,
    ) -> MixinMethod {
        MixinMethod {
            selector: Symbol::intern(&crate::interpreter::instantiate::method_name(
                &declaration.pattern,
            )),
            access: crate::dispatch::effective_access(declaration.access),
            argument_count: crate::interpreter::instantiate::pattern_parameter_count(
                &declaration.pattern,
            ),
            code: Rc::new(crate::compiler::compile_method(
                &declaration,
                bindings,
                lowering,
            )),
            declaration: Some(declaration),
        }
    }
}

/// One of a mixin's own instance slots: what decides the accessors it
/// defines (`slot_accessors`) -- its name, whether it is mutable and its
/// accessibility -- and, for a mixin built from source, the declaration
/// whose initializer expression the compiler reads.
#[derive(Debug, Clone)]
pub struct MixinSlot {
    pub name: Symbol,
    /// §6.3.2: only a mutable slot has a setter.
    pub mutable: bool,
    /// §6.3.2: the accessors' accessibility "is the same as the slot's",
    /// already defaulted through `dispatch::effective_access` (§6.3.1).
    pub access: ast::AccessModifier,
    /// `None` for a slot of a mixin decoded from an image: its initializer
    /// is already compiled into that mixin's initializer body.
    pub declaration: Option<Rc<ast::SlotDeclaration>>,
}

impl MixinSlot {
    pub fn from_declaration(declaration: Rc<ast::SlotDeclaration>) -> MixinSlot {
        MixinSlot {
            name: Symbol::intern(&declaration.name.name),
            mutable: declaration.mutability != ast::SlotMutability::Immutable,
            access: crate::dispatch::effective_access(declaration.access),
            declaration: Some(declaration),
        }
    }
}

/// §6.3.2: the getter a slot declaration defines, or a mutable slot's setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    Getter,
    Setter,
}

/// One of the accessors a slot declaration implicitly defines
/// (`newspeak-spec.pdf` §6.3.2): a method with the slot's own accessibility
/// whose body reads or writes the slot. `own_index` is the slot's position
/// among its mixin's *own* slots, resolved per application through
/// `MixinApplication::own_slot_base` exactly as
/// `Instruction::InitializeSlot` is -- one entry serves every application
/// of the mixin, which may each place the mixin's slots at a different
/// absolute offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotAccessor {
    pub selector: Symbol,
    pub access: ast::AccessModifier,
    pub own_index: u32,
    pub kind: AccessorKind,
}

/// A native method of a builtin class (`docs/design/dispatch.md`,
/// "Primitives"): the primitive it runs and its §5.2
/// accessibility. Found by the same lookups as a compiled method; it runs no
/// code and pushes no activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimitiveMethod {
    pub primitive: crate::builtins::primitive::Primitive,
    pub access: ast::AccessModifier,
}

/// The primary factory as a member of the metaclass mixin
/// (`newspeak-spec.pdf` §6.3: "The class side, together with the primary
/// factory method, defines the mixin for the metaclass"). It carries no
/// body: what it does is instantiate the class object the send was addressed
/// to, which `interpreter::resolve::act` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactoryMethod {
    pub selector: Symbol,
    /// The factory pattern's parameter count (§6.1), carried so that an
    /// entry answers everything a lookup's caller can ask of a method
    /// without reaching back to the declaration -- a decoded mixin has none.
    pub argument_count: usize,
    pub access: ast::AccessModifier,
}

/// One entry of a mixin's method table (§3.5): a compiled method, one of the
/// accessors a slot declaration defines, a builtin's primitive, or -- on a
/// metaclass mixin, and only there -- the primary factory. Methods on the
/// same footing, found by the same lookup and overridden the same way
/// (`docs/design/dispatch.md`, "Slot accessors").
#[derive(Debug, Clone)]
pub enum MethodEntry {
    Compiled(Rc<MixinMethod>),
    SlotAccessor(SlotAccessor),
    Primitive(PrimitiveMethod),
    Factory(FactoryMethod),
}

impl MethodEntry {
    /// The entry's §5.2 accessibility, which every lookup reads.
    pub fn access(&self) -> ast::AccessModifier {
        match self {
            MethodEntry::Compiled(method) => method.access,
            MethodEntry::SlotAccessor(accessor) => accessor.access,
            MethodEntry::Primitive(method) => method.access,
            MethodEntry::Factory(factory) => factory.access,
        }
    }

    /// The compiled method, for an entry that is one.
    pub fn compiled(&self) -> Option<&Rc<MixinMethod>> {
        match self {
            MethodEntry::Compiled(method) => Some(method),
            MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) | MethodEntry::Factory(_) => {
                None
            }
        }
    }
}

/// The one entry a metaclass mixin holds beyond the class side's own
/// members: §6.3's primary factory method, under the selector §6.1
/// instantiates the class by -- the declared factory pattern's, or `new` for
/// a declaration that writes none and for a builtin, which declares none.
///
/// One definition, called from every place a metaclass mixin is assembled
/// -- from a declared class side, for the empty one a class without a class
/// side still needs, and on the way out of an image -- so that the three
/// cannot disagree about which selector instantiates a class.
///
/// A factory is public, although §6.3.1 defaults an unmarked member to
/// protected: a factory that could not be sent from outside would leave no
/// class instantiable, and the specification's own examples send one across
/// module boundaries.
pub fn factory_entry(factory: Option<Factory>) -> (Symbol, MethodEntry) {
    let selector = factory.map_or(Symbol::NEW, |factory| factory.selector);
    (
        selector,
        MethodEntry::Factory(FactoryMethod {
            selector,
            argument_count: factory.map_or(0, |factory| factory.argument_count),
            access: ast::AccessModifier::Public,
        }),
    )
}

/// The accessor entries `slots` define, in declaration order (§6.3.2): each
/// slot's getter under its name and, for a mutable slot, its setter under
/// the name with `:` appended -- "a single argument keyword message
/// consisting of the slot name with : appended to it".
pub fn slot_accessors(slots: &[MixinSlot]) -> Vec<(Symbol, MethodEntry)> {
    let mut accessors = Vec::with_capacity(slots.len() * 2);
    for (index, slot) in slots.iter().enumerate() {
        let own_index = u32::try_from(index).expect("a mixin's slot count fits a u32");
        accessors.push((
            slot.name,
            MethodEntry::SlotAccessor(SlotAccessor {
                selector: slot.name,
                access: slot.access,
                own_index,
                kind: AccessorKind::Getter,
            }),
        ));
        if slot.mutable {
            let setter = Symbol::intern(&format!("{}:", slot.name));
            accessors.push((
                setter,
                MethodEntry::SlotAccessor(SlotAccessor {
                    selector: setter,
                    access: slot.access,
                    own_index,
                    kind: AccessorKind::Setter,
                }),
            ));
        }
    }
    accessors
}

/// A mixin's primary factory message, reduced to the two things the run
/// time asks of it: which selector instantiates it (`newspeak-spec.pdf`
/// §6.1) and how many arguments that selector takes. `None` for a mixin
/// with no factory of its own -- a builtin's, and a class side's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Factory {
    pub selector: Symbol,
    pub argument_count: usize,
}

/// A mixin's superclass clause, reduced to what run time reads off it.
///
/// The clause's *expressions* are Newspeak code, compiled per mixin into
/// `Mixin::superclass_clause_code` / `superclass_message_code`; what is left
/// here is the part the orchestration around those bodies needs, which it
/// consults before any body runs. Storing it is what keeps
/// `instantiate::clause_message_body` from having to read `Mixin::declaration`:
/// without it, a mixin decoded from an image would fall back to `new` and every
/// class whose superclass has a keyword factory would raise a spurious "not the
/// primary factory message" error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperclassClause {
    /// The factory selector the clause's message names; `new` when the
    /// message is omitted (§6.1: "if the message is omitted, it defaults to
    /// `new`").
    pub selector: Symbol,
    /// Whether that message carries argument expressions, and therefore a
    /// compiled body in `Mixin::superclass_message_code`. The
    /// overwhelmingly common `= Object new` shape has none and runs no code.
    pub has_arguments: bool,
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
    /// The method table: the compiled methods and the accessors `slots`
    /// define (`slot_accessors`), one entry per selector.
    pub methods: IdentityMap<Symbol, MethodEntry>,
    /// This mixin's own instance slots, in declaration order.
    pub slots: Vec<MixinSlot>,
    pub factory: Option<Factory>,
    pub superclass: Option<SuperclassClause>,
    /// Whether the declaration wrote any `initExpr`s. Stored because
    /// `instantiate::mixin_needs_initializer_code` asks the question of a
    /// decoded mixin too, where `initializer_expressions` below is empty by
    /// construction.
    pub has_initializer_expressions: bool,
    /// The `initExpr` statements themselves -- lowered by the compiler into the
    /// instance initializer. Empty for a decoded mixin.
    pub initializer_expressions: Vec<ast::Statement>,
    /// The nested class declarations, in declaration order. Each one gets an
    /// implicit accessor and a hidden cache slot.
    pub nested: Vec<Rc<Mixin>>,
    pub class_side: Option<Rc<Mixin>>,
    /// Provenance for reflection and the image.
    pub span: Span,
    /// The declaration this mixin was built from: the compiler lowers its
    /// superclass clause. `None` for a builtin's mixin, for a class side (a
    /// `ClassBody` is not an `ast::ClassDeclaration`), and for a mixin decoded
    /// from an image.
    pub declaration: Option<Rc<ast::ClassDeclaration>>,
    /// The instance initializer: this mixin's own slot initializers, then its
    /// own `initExpr`s. Every mixin built from source has one, a class side
    /// included. `None` for a builtin's mixin, which runs no code, and for a
    /// decoded row that names none -- `image::decode::verify_mixins` refuses
    /// that when the mixin needs one.
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
    /// mixin starts from -- a builtin's -- and the base the image decoder
    /// fills in field by field. One definition, so adding a field to `Mixin`
    /// cannot silently leave one of them holding a stale default.
    pub fn empty(name: Symbol) -> Mixin {
        Mixin {
            name,
            access: ast::AccessModifier::Protected,
            methods: IdentityMap::default(),
            slots: Vec::new(),
            factory: None,
            superclass: None,
            has_initializer_expressions: false,
            initializer_expressions: Vec::new(),
            nested: Vec::new(),
            class_side: None,
            span: Span::new(0, 0),
            declaration: None,
            initializer_code: None,
            superclass_clause_code: None,
            superclass_message_code: None,
        }
    }

    /// The compiled method `selector` names, if the table holds one under it.
    pub fn compiled_method(&self, selector: Symbol) -> Option<&Rc<MixinMethod>> {
        self.methods.get(&selector).and_then(MethodEntry::compiled)
    }

    /// Every compiled method of the table, in no particular order.
    pub fn compiled_methods(&self) -> impl Iterator<Item = &Rc<MixinMethod>> {
        self.methods.values().filter_map(MethodEntry::compiled)
    }
}

#[derive(Debug)]
pub struct MixinApplication {
    pub mixin: Rc<Mixin>,
    pub superclass: Option<Rc<MixinApplication>>,
    /// Absolute index of the hidden cache slot for each nested class
    /// declaration, including inherited ones. The slot stores the cached
    /// nested class object; accessing it is a slot read rather than a
    /// hash lookup. Inherited declarations keep the index their declaring
    /// level gave them, as inherited slots keep theirs.
    ///
    /// Keyed by `mixin_key` -- the declaration's identity -- and not by its
    /// name, which is the one thing a subclass's nested declaration shares
    /// with the inherited one it shadows. Both declarations are real, both
    /// get their own physical slot, and `super N` reaches the inherited one
    /// while `N` reaches the subclass's; a name key would collapse the two
    /// entries onto one index and make whichever was resolved first the
    /// answer to both.
    pub nested_slot: IdentityMap<usize, usize>,
    pub slot_count: usize,
    /// Where this application's *own* slots start -- `superclass.slot_count`,
    /// or 0 with none. Read at run time (`Mutator::initializer_body`, into the
    /// activation's `slot_base`) to turn a compiled `InitializeSlot`'s
    /// mixin-relative `own_index` into this application's absolute field index:
    /// the compiled initializer is shared by every application of the mixin
    /// (keying code by mixin without this would bake ONE application's offset
    /// into code every application runs), while this field is computed fresh
    /// per application, as `nested_slot` is.
    pub own_slot_base: usize,
    /// The ids an inline cache guards on (`DispatchId`), minted by
    /// `Mutator::apply_mixin` when it creates the application, so an id never
    /// comes from an image's bytes. `DispatchIds::NONE` for an application
    /// built anywhere else, which only tests do.
    pub dispatch_ids: DispatchIds,
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
