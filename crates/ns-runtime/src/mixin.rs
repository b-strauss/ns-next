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
//! themselves live on the mixin (its three initializer bodies and the two
//! superclass-clause bodies) and on its methods (`MixinMethod::code`), for a
//! built mixin and a decoded one alike.

use std::cell::{Cell, OnceCell, Ref, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::heap::MixinId;
use crate::types::Binding;
use crate::types::BuiltinClass;
use crate::Symbol;

/// The guard an inline cache compares: a dense number, minted per heap, that
/// names one mixin application's behaviour for its instances
/// (`docs/design/dispatch.md`, "The dispatch id"). A class object is an
/// instance of its metaclass, so its id is its metaclass's application's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchId(u32);

impl DispatchId {
    /// What `Heap::dispatch_id` answers for a value whose class has no
    /// application -- only a crafted image makes one -- and what an
    /// application built outside `apply_mixin` carries. Never cached. 0, so
    /// that a header word left zero by a missed initialization carries a
    /// guard no application was minted (`docs/design/object-model.md`, "The
    /// reserved guard values sit at the low end").
    pub const NONE: DispatchId = DispatchId(0);
    /// The guard of a cache entry that guards nothing: a vacant, polymorphic
    /// or megamorphic site. `Heap::dispatch_id` never answers it, so no
    /// receiver matches one.
    pub(crate) const VACANT: DispatchId = DispatchId(1);
    /// The guard the bulk forward stamps on each forwarder
    /// (`Heap::forward_identity`). It is never minted, so that a cache could
    /// never hit on a forwarder even if a reader met one -- and none does,
    /// since the forward is eager and leaves no handle naming a forwarder.
    pub(crate) const FORWARDED: DispatchId = DispatchId(2);
    /// The first id the allocator mints (`heap::ids::DispatchIdAllocator`).
    pub(crate) const FIRST_MINTED: u32 = 3;
    /// One past the last id: the header word's guard field is 22 bits wide.
    pub(crate) const LIMIT: u32 = 1 << 22;

    pub(crate) fn from_counter(value: u32) -> DispatchId {
        debug_assert!((DispatchId::FIRST_MINTED..DispatchId::LIMIT).contains(&value));
        DispatchId(value)
    }

    /// The id as the header word's low 22 bits hold it: an object's guard
    /// lives in its header, so that a cache's check is one load and one
    /// compare (`heap::header`).
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }

    /// The id a header word's low 22 bits hold -- the header's reader, and
    /// nothing else: unlike `from_counter` it accepts the reserved values,
    /// since a header carries `NONE` until it is stamped.
    pub(crate) fn from_bits(bits: u32) -> DispatchId {
        debug_assert!(bits < DispatchId::LIMIT);
        DispatchId(bits)
    }
}

// The assertion keeps the forwarder's reserved guard below every minted id,
// so that the allocator can never mint it.
const _: () = assert!(DispatchId::FORWARDED.0 < DispatchId::FIRST_MINTED);

/// An application's id, for its instances. There is no second, class-side
/// id: a class object's class is its metaclass (`newspeak-spec.pdf` §3.2),
/// an application of its own, so a class object's id is that application's
/// instances id like any other object's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchIds {
    pub instances: DispatchId,
}

impl DispatchIds {
    pub const NONE: DispatchIds = DispatchIds {
        instances: DispatchId::NONE,
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
    /// §6.3.1 accessibility, already defaulted through
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
    /// What a send of this method does without an activation, when it is a
    /// kernel method `compiler::primitive_method` recognized: `act` reads it
    /// on every send. `None` for every other method, a lazy slot's getter
    /// included (its `ReturnSlotIfFilled` answers a filled slot inside the
    /// activation), and for every method decoded from an image, which never
    /// carries a kernel method.
    pub fast_path: Option<MethodFastPath>,
    /// The nested class of its module's root this method answers, when it is
    /// a pure class accessor `compiler::primitive_method` recognized in
    /// shipped core-library source other than the kernel: a unary method
    /// whose whole body is `^outer <Root> <Nested>`. Read by the shipped
    /// world alone (`interpreter::shipped_world`), which lets the class it
    /// answers -- `platform collections List` -- stand as a value in an image; no
    /// send ever reads it, so it costs the run loop nothing, unlike a
    /// `MethodFastPath` variant (`docs/design/execution.md`, "What the
    /// platform module cost"). `None` for every other method, and for every
    /// method decoded from an image, which never carries shipped source.
    pub class_accessor: Option<ClassAccessor>,
}

/// A pure class accessor (`MixinMethod::class_accessor`): `^outer <Root>
/// <Nested>`, with `depth` the lexical depth `outer <Root>` resolved to --
/// how many levels out from the class declaring the method its module's root
/// lies -- and `class_name` the nested class it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassAccessor {
    pub depth: u16,
    pub class_name: Symbol,
}

/// What a method's send does without an activation, read off the method by
/// `act` on every send (`docs/design/dispatch.md`, "Primitive methods").
/// Only shipped core-library source's methods carry one. Every variant's
/// arm is inlined into each send site of the run loop, so a new one taxes
/// every method send: a lazy slot's getter had one and gave it up for that
/// reason (`Instruction::ReturnSlotIfFilled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodFastPath {
    /// The method forwards to one mirror row: run the leaf, and activate
    /// the method only when the leaf declines.
    Mirror(crate::builtins::primitive::MirrorPrimitive),
    /// The method answers a builtin class (§3.2's accessors on
    /// `Object`): answer it, never activate.
    BuiltinClass(crate::types::BuiltinClass),
    /// A quick method whose body is `^true`, `^false` or `^nil`: answer
    /// it, never activate.
    Constant(QuickConstant),
    /// A quick method whose body is `^self`: answer the receiver, never
    /// activate.
    Receiver,
    /// A quick method whose body is `^<parameter>`: answer the argument at
    /// this position, never activate.
    Parameter(u8),
    /// A quick method whose body is `^<parameter> value`: a closure
    /// argument at this position is answered as the block to activate, as
    /// a control-flow step; anything else activates the method, whose body
    /// sends `value` for real.
    ValueOfParameter(u8),
    /// A quick method whose body is `^<parameter> cull: self`: a closure
    /// argument at this position is answered as the block to activate, with
    /// the receiver as its argument when it takes one and with none when it
    /// takes none; anything else activates the method, whose body sends
    /// `cull:` for real.
    CullParameter(u8),
}

/// The constant a quick method answers (`MethodFastPath::Constant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickConstant {
    True,
    False,
    Nil,
}

impl QuickConstant {
    /// The value a quick method answering this constant answers.
    pub(crate) fn value<'gc>(self) -> crate::types::Value<'gc> {
        match self {
            QuickConstant::True => crate::types::Value::boolean(true),
            QuickConstant::False => crate::types::Value::boolean(false),
            QuickConstant::Nil => crate::types::Value::nil(),
        }
    }
}

impl MixinMethod {
    /// The entry a parsed declaration becomes, compiled by `lowering` against
    /// the resolver's `bindings` for the module it belongs to. `kernel` is
    /// that module's `ResolvedModule.kernel`: only a kernel method may carry
    /// a fast path.
    pub(crate) fn from_declaration(
        declaration: Rc<ast::MethodDeclaration>,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
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
            fast_path: crate::compiler::primitive_method::recognize(&declaration, bindings, kernel),
            // Recognized against the module's root by `build_methods`, which
            // alone knows it.
            class_accessor: None,
            declaration: Some(declaration),
        }
    }

    /// The getter a lazy slot declares (§6.3.3): a method of the slot's name
    /// and accessibility whose `code` evaluates the initializer, stores it
    /// with `InitializeSlot` and answers it
    /// (`compiler::compile_lazy_slot_getter`).
    pub fn lazy_slot_getter(slot: &MixinSlot, code: Rc<CompiledCode>) -> MixinMethod {
        MixinMethod {
            selector: slot.name,
            access: slot.access,
            argument_count: 0,
            declaration: None,
            code,
            fast_path: None,
            class_accessor: None,
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
    /// §6.3.3: initialized by its getter on first read rather than by the
    /// instance initializer. Its getter is a compiled method
    /// (`MixinMethod::lazy_slot_getter`), not an accessor entry, and a
    /// value's state skips it (`declared_slots_of`).
    pub lazy: bool,
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
            lazy: declaration.lazy,
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

/// A primitive entry (`docs/design/dispatch.md`, "Primitives"): the
/// primitive it runs and its §6.3.1 accessibility. Only the VM mirror's
/// method table holds any -- one per `MIRROR` row. Found by the same lookups
/// as a compiled method; it runs no code and pushes no activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimitiveMethod {
    pub primitive: crate::builtins::primitive::Primitive,
    pub access: ast::AccessModifier,
}

/// The primary factory as a member of the metaclass mixin
/// (`newspeak-spec.pdf` §6.3: "The class side, together with the primary
/// factory method, defines the mixin for the metaclass"). Built only by
/// `factory_entry`, which gives it its body.
#[derive(Debug, Clone)]
pub struct FactoryMethod {
    pub selector: Symbol,
    /// The primary factory signature's parameter count (§6's unnumbered
    /// introduction, before §6.1 begins), carried so that an entry answers
    /// everything a lookup's caller can ask of a method without reaching
    /// back to the declaration -- a decoded mixin has none.
    pub argument_count: usize,
    pub access: ast::AccessModifier,
    /// The factory's body, `compiler::factory_body` of `argument_count`:
    /// allocate an instance of the class object the send was addressed to,
    /// run its initializer on it with the factory's arguments, answer it.
    /// Synthesized, never lowered and never read off an image, so that no
    /// image can carry a factory body of its own.
    pub code: Rc<CompiledCode>,
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
    /// The entry's §6.3.1 accessibility, which every lookup reads.
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
/// members: §6.3's primary factory method, under the selector that
/// instantiates the class -- "when a class object receives a message ...
/// whose selector is the name of its primary factory method, the instance
/// initializer is invoked" (§6.3) -- the declared factory pattern's, or
/// `new` for a declaration that writes none and for a builtin, which
/// declares none.
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
///
/// The entry's body is `compiler::factory_body` of the factory's arity, so
/// every factory of one arity runs the same instructions, whoever built
/// the entry -- a declaration from source, the embedded shipped code or
/// the image decoder, which never reads a factory body off the wire. An
/// arity past what a `u16` counts saturates, and the body it gets raises
/// the too-large error (`compiler::factory_body`).
pub fn factory_entry(factory: Option<Factory>) -> (Symbol, MethodEntry) {
    let selector = factory_entry_selector(factory);
    let argument_count = factory.map_or(0, |factory| factory.argument_count);
    (
        selector,
        MethodEntry::Factory(FactoryMethod {
            selector,
            argument_count,
            access: ast::AccessModifier::Public,
            code: Rc::new(crate::compiler::factory_body(
                u16::try_from(argument_count).unwrap_or(u16::MAX),
            )),
        }),
    )
}

/// The selector `factory_entry` files the entry of `factory` under: the
/// declared factory's, or `new` for none. For a reader that needs the
/// selector alone, so that it builds no body.
pub fn factory_entry_selector(factory: Option<Factory>) -> Symbol {
    factory.map_or(Symbol::NEW, |factory| factory.selector)
}

/// The accessor entries `slots` define, in declaration order (§6.3.2): each
/// slot's getter under its name and, for a mutable slot, its setter under
/// the name with `:` appended -- "a single argument keyword message
/// consisting of the slot name with : appended to it".
///
/// A lazy slot (§6.3.3) contributes its setter only: its getter has to run
/// the initializer while the slot is empty, so it is a compiled method
/// (`MixinMethod::lazy_slot_getter`), which whoever builds the method table
/// adds beside these entries.
pub fn slot_accessors(slots: &[MixinSlot]) -> Vec<(Symbol, MethodEntry)> {
    let mut accessors = Vec::with_capacity(slots.len() * 2);
    for (index, slot) in slots.iter().enumerate() {
        let own_index = u32::try_from(index).expect("a mixin's slot count fits a u32");
        if !slot.lazy {
            accessors.push((
                slot.name,
                MethodEntry::SlotAccessor(SlotAccessor {
                    selector: slot.name,
                    access: slot.access,
                    own_index,
                    kind: AccessorKind::Getter,
                }),
            ));
        }
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
/// time asks of it: which selector instantiates it and how many arguments
/// that selector takes -- the signature §6's unnumbered introduction, before
/// §6.1 begins, allows right after the class name, defaulting to `new` when
/// omitted. `None` for a mixin with no factory of its own -- a builtin's,
/// and a class side's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Factory {
    pub selector: Symbol,
    pub argument_count: usize,
}

/// The message a superclass clause or a `<:` clause sends its class,
/// reduced to what run time reads off it. §6.2 builds a `<:` clause from
/// §6.1's `inheritanceClause`, so the two carry the same message and read
/// it the same way.
///
/// The message's *arguments* are Newspeak code, lowered inline into the
/// mixin's initializer bodies; what is kept here is the part the orchestration
/// around those bodies needs, which it consults before any body runs. Storing
/// it is what keeps `instantiate` from having to read `Mixin::declaration`:
/// without it, a mixin decoded from an image would fall back to `new` and every
/// class whose superclass has a keyword factory would raise a spurious "not the
/// primary factory message" error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClauseMessage {
    /// The factory selector the message names; `new` when it is omitted
    /// (§6.1: "if the message is omitted, it defaults to `new`").
    pub selector: Symbol,
    /// Whether the message carries argument expressions, which the
    /// initializers lower inline. The overwhelmingly common `= Object new`
    /// shape has none and runs no code.
    pub has_arguments: bool,
}

/// One `<:` clause of a mixin application (`newspeak-spec.pdf` §6.2), as the
/// declaration that writes it carries it: the level the clause adds is built
/// from what the clause's receiver evaluates to, and the rest is what the
/// instantiation walk needs to initialize that level.
///
/// It lives on the *applying* declaration's mixin, not on the applied one:
/// the applied mixin is shared by every declaration that applies it, and
/// each of them writes its own message.
#[derive(Debug, Clone)]
pub struct MixinApplicationClause {
    /// The factory message the clause sends the class it names.
    pub message: ClauseMessage,
    /// The identifier the clause's receiver names, for the kernel phase,
    /// which reads the class a kernel class applies without running the
    /// clause. `None` for a clause decoded from an image.
    pub receiver: Option<Symbol>,
}

/// Which `<:` clause added a level (`Heap::class_applied_origin`): the
/// applying declaration's mixin and the clause's position in its
/// `mixin_applications`, rather than a copy of the clause.
///
/// A copy would say what the clause was but not which one: two clauses of
/// one declaration can be identical (`<: A <: B`, both sending `new`), and
/// the image names a level's clause by the row that declares it and its
/// position there, so that the row's own clauses -- verified against that
/// row's factory once, at load -- are the only ones a decoded level can run.
///
/// `named_builtin` records the builtin the clause's receiver named, when it
/// named one. A builtin's mixin is in no declaration tree, so it is how the
/// image identifies such a level's mixin -- by builtin tag, the way it
/// identifies a builtin superclass -- and nothing else reads it.
#[derive(Debug, Clone)]
pub struct AppliedClause {
    pub declaration: Rc<Mixin>,
    pub index: usize,
    pub named_builtin: Option<crate::types::BuiltinClass>,
}

impl AppliedClause {
    /// The clause itself. `None` only if `index` is past the declaration's
    /// clauses, which neither `nested::build_applied_levels` nor the image
    /// decoder -- the two things that build one -- lets happen; answering
    /// `None` rather than indexing keeps a future mistake from being a panic.
    pub fn clause(&self) -> Option<&MixinApplicationClause> {
        self.declaration.mixin_applications.get(self.index)
    }
}

/// How many `<:` clauses one superclass clause may apply: the clause index
/// is 23 bits of the class object's applied-clause `SmallInteger` -- a limit
/// a valid program can reach, taken knowingly (decided with the user on
/// 2026-10-03; `docs/design/object-model.md`, "The object layout").
pub(crate) const CLAUSE_INDEX_LIMIT: usize = 1 << 23;

const CLAUSE_INDEX_SHIFT: u32 = 32;
const BUILTIN_TAG_SHIFT: u32 = 55;
const BUILTIN_TAG_MASK: u64 = 0b11_1111;
const SIXTY_ONE_BITS: u64 = (1 << 61) - 1;

impl AppliedClause {
    /// The three parts as one `SmallInteger`: the 61-bit pattern
    /// sign-extended, so bit 60 makes it negative rather than too large.
    pub(crate) fn pack(mixin: MixinId, index: usize, named_builtin: Option<BuiltinClass>) -> i64 {
        debug_assert!(index < CLAUSE_INDEX_LIMIT);
        let tag = named_builtin.map_or(0, |builtin| {
            1 + BuiltinClass::ALL
                .iter()
                .position(|candidate| *candidate == builtin)
                .expect("every builtin is in BuiltinClass::ALL") as u64
        });
        let pattern =
            mixin.index() as u64 | (index as u64) << CLAUSE_INDEX_SHIFT | tag << BUILTIN_TAG_SHIFT;
        ((pattern << 3) as i64) >> 3
    }

    /// `None` for a tag that names no builtin, which only a corrupt field
    /// could hold.
    pub(crate) fn unpack(packed: i64) -> Option<(MixinId, usize, Option<BuiltinClass>)> {
        let pattern = packed as u64 & SIXTY_ONE_BITS;
        let mixin = MixinId::from_index((pattern & u64::from(u32::MAX)) as usize)?;
        let index = ((pattern >> CLAUSE_INDEX_SHIFT) as usize) & (CLAUSE_INDEX_LIMIT - 1);
        let tag = (pattern >> BUILTIN_TAG_SHIFT) & BUILTIN_TAG_MASK;
        let named_builtin = match tag {
            0 => None,
            tag => Some(*BuiltinClass::ALL.get(tag as usize - 1)?),
        };
        Some((mixin, index, named_builtin))
    }
}

const _: () = assert!(BuiltinClass::ALL.len() < BUILTIN_TAG_MASK as usize);

/// `Debug` is derived so a class object's payload keeps its own derived
/// `Debug` now that a class object's behaviour lives here. Printing one
/// walks the whole superclass chain and every method's AST.
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
    /// The superclass clause's message; `None` for a mixin with no
    /// superclass clause of its own -- a builtin's, and a class side's.
    pub superclass: Option<ClauseMessage>,
    /// The identifier the superclass clause's receiver names, after any
    /// prefix. The kernel phase reads a kernel class's superclass from it
    /// without running the clause, which it may not run before the kernel
    /// exists; recorded here so that a mixin decoded from the embedded
    /// shipped code, which has no declaration, answers it too. `None` with
    /// no clause, and for a mixin decoded from an image, whose mixins the
    /// kernel phase never reads.
    pub superclass_receiver: Option<Symbol>,
    /// §6.2's `<:` clauses, in declaration order: each one adds a level
    /// between this declaration's own class and its superclass, the first
    /// clause's level lowest. Empty for a declaration that applies no mixin,
    /// for a builtin's mixin and a class side. A decoded mixin carries its
    /// row's clauses (`image::mixins`).
    pub mixin_applications: Vec<MixinApplicationClause>,
    /// Whether the declaration writes a class body. `false` exactly for a
    /// declaration that ends in a dot after its `<:` clauses (§6: "the
    /// mixin can be specified along with the superclass via a mixin
    /// application"): such a declaration is the mixin application itself,
    /// so its class object is the level its last clause adds, named for it
    /// and with its factory, and no level of its own is ever built over
    /// it (decision 8 of `new` as compiled code; `docs/design/object-model.md`,
    /// "Mixin application"). `true` for every other mixin, a builtin's and a
    /// class side included. A decoded mixin carries its row's.
    pub has_body: bool,
    /// Whether the declaration wrote any `initExpr`s. Stored because
    /// `instantiate::mixin_needs_initializer_code` asks the question of a
    /// decoded mixin too, where `initializer_expressions` below is empty by
    /// construction.
    pub has_initializer_expressions: bool,
    /// The `initExpr` statements themselves -- lowered by the compiler into the
    /// instance initializer. Empty for a decoded mixin.
    pub initializer_expressions: Vec<ast::Statement>,
    /// The nested class declarations, in declaration order, as they were
    /// built: each one gets an implicit accessor and a hidden cache slot.
    /// Read through `declared_nested` until a heap registers the mixin,
    /// which takes them (`Heap::nested_of` names them by id from then on).
    pub nested_declarations: NestedDeclarations,
    pub class_side: Option<Rc<Mixin>>,
    /// Provenance for reflection and the image.
    pub span: Span,
    /// The declaration this mixin was built from: the compiler lowers its
    /// superclass clause. `None` for a builtin's mixin, for a class side (a
    /// `ClassBody` is not an `ast::ClassDeclaration`), and for a mixin decoded
    /// from an image.
    pub declaration: Option<Rc<ast::ClassDeclaration>>,
    /// The full instance initializer, run on an instance of the level its
    /// declaration's class object is: each `<:` clause's selector checked
    /// against the class it names, the superclass clause's message
    /// evaluated and the superclass's initializer run with it, each `<:`
    /// clause's message evaluated and its level's own part run with it,
    /// then this mixin's own part (`own_initializer_code`), then `self`
    /// (`docs/design/object-model.md`, "Instantiation"). A body-less
    /// declaration's (`has_body`) runs at its class object, the level its
    /// last clause adds, so its levels are counted from 0, and its own part
    /// is empty. Every mixin built
    /// from source has one, a class side included, whose full body is its
    /// own part. `None` for a builtin's mixin, which runs no code;
    /// `image::decode::verify_mixins` refuses a decoded row without one.
    pub initializer_code: Option<Rc<CompiledCode>>,
    /// The own part: this mixin's own slot initializers, then its own
    /// `initExpr`s, then `self`, and nothing of its clauses -- what a level
    /// a `<:` clause added runs for the mixin it applies. Every mixin built
    /// from source has one. `None` for a builtin's mixin, and for a decoded
    /// row that names none -- `image::decode::verify_mixins` refuses that
    /// when the mixin needs one (`instantiate::mixin_needs_initializer_code`).
    pub own_initializer_code: Option<Rc<CompiledCode>>,
    /// The standalone initializer, for a class object of this mixin that
    /// no declaration applies it in -- a level a `<:` clause added, reached
    /// from outside: the superclass clause's message and the superclass's
    /// initializer, then the own part, without this declaration's own `<:`
    /// clauses, which do not apply to such a level. `Some` exactly when the
    /// declaration writes `<:` clauses and a body: without clauses it would
    /// be the full body, and a body-less declaration's class object is
    /// never reached outside its declaration (`has_body`).
    pub standalone_initializer_code: Option<Rc<CompiledCode>>,
    /// The superclass clause's receiver expression (§6.1's `ec`), which the
    /// nested-class accessor runs once per enclosing object -- together with
    /// every `<:` clause's receiver when the declaration applies mixins, as
    /// one array (`interpreter::nested::superclass_clause_expression`).
    /// `None` when the declaration has no clause.
    pub superclass_clause_code: Option<Rc<CompiledCode>>,
    /// Why this mixin, as a module's root, can never be instantiated (the
    /// root rule, `resolver::root_clause_defect`). Only a module's root
    /// mixin built from a `ResolvedModule` a host assembled past `resolve`,
    /// which refuses such a root, carries one; a decoded mixin
    /// never does, because the decoder refuses a root class record the root
    /// rule forbids (`image::modules::root_record_defect`) and
    /// the encoder never writes one. `Mutator::apply_mixin` turns it into
    /// `InstantiationRefusal::RootClause` on the application.
    pub root_clause_defect: Option<RootClauseDefect>,
    /// How a module's root is built (`resolver::root_shape`), recorded on
    /// a root mixin built from source so that building its class needs no
    /// declaration. `None` for every other mixin, for a root with a defect,
    /// and for a mixin decoded from an image.
    pub root_shape: Option<crate::resolver::RootShape>,
    /// Compiled from shipped core-library source (`ResolvedModule.kernel`):
    /// set on the declaration's mixin, on every nested mixin and on its class
    /// side. The image encoder refuses a class object of a shipped mixin
    /// that no shipped name names (`image::shipped_names`), and holds every
    /// record of a shipped class that is no builtin to the loading binary's
    /// layout. A decoded mixin is never shipped: an image carries only its
    /// own module.
    pub shipped: bool,
    /// The mixin this one is declared in, for the mirrors' `enclosingMixin`
    /// and a class side's `instanceMixin`: a nested declaration's names the
    /// declaration it is nested in, a class side's -- declared or
    /// synthesized -- its instance side's. A mixin holds its declared
    /// nested mixins until a heap registers it, not the one it is nested
    /// in, so this is the upward link, `Weak` so
    /// that a tree is no cycle, and set once, after the enclosing mixin's
    /// `Rc` exists (`set_enclosing_of_children`). Empty for a module's root
    /// and a builtin's mixin. What a builder records: the heap copies it,
    /// as the enclosing mixin's id, into the mixin's row when it first
    /// registers the mixin (`Heap::mixin_id`), and a reader of the link
    /// asks the heap (`Heap::enclosing_mixin_id`), since a replacement
    /// keeps the id's link whatever its own says.
    pub enclosing: OnceCell<Weak<Mixin>>,
    /// The text of the declaration this mixin was read from, for the
    /// mirrors' `source` and an edit: its own text with its nested
    /// declarations cut out (`DeclarationText`), sliced by each method's
    /// `CompiledCode::source`, and assembled with its nested declarations'
    /// current versions into its module's text (`Heap::assembled_text`).
    /// A class side, declared or synthesized from a declaration, shares
    /// its instance side's. `None` for a builtin's mixin, the VM mirror's,
    /// and a mixin of an image that carries no source. Each version holds
    /// its own, so that no
    /// version keeps another's text alive.
    pub text: Option<Rc<DeclarationText>>,
}

/// A mixin's nested declarations until a heap registers it
/// (`Mixin::nested_declarations`). Its parts are private so that every
/// reader goes through `Mixin::declared_nested`, which a debug build
/// refuses once the list was taken.
#[derive(Debug, Default)]
pub struct NestedDeclarations {
    /// The declarations, in declaration order; empty once taken.
    list: RefCell<Vec<Rc<Mixin>>>,
    /// Whether a heap took `list`.
    adopted: Cell<bool>,
}

impl NestedDeclarations {
    /// Declarations no heap has taken.
    pub fn new(nested: Vec<Rc<Mixin>>) -> NestedDeclarations {
        NestedDeclarations {
            list: RefCell::new(nested),
            adopted: Cell::new(false),
        }
    }
}

/// A copy of a mixin is a fresh mixin no heap has registered: it holds
/// the same declarations as long as its original still did, and none once
/// a heap took them -- the heap's row keeps naming them by id.
impl Clone for NestedDeclarations {
    fn clone(&self) -> NestedDeclarations {
        NestedDeclarations::new(self.list.borrow().clone())
    }
}

/// The text of one declaration as it was read: from its start to its end
/// -- for a module's root declaration, the whole module -- with every
/// declaration nested in it cut out, as a hole. A span the compiler
/// recorded counts from the start of the module text the declaration was
/// read in, and still slices it.
///
/// Every boundary is checked once, when the text is made (`cut`,
/// `shared`), so that every later `slice` and `assemble` stays within the
/// storage: a span or a hole an image chose answers no text, never a
/// panic.
pub struct DeclarationText {
    /// Where the declaration's text is kept.
    storage: TextStorage,
    /// The declaration's extent in the module text it was read in.
    range: Span,
    /// The declarations nested directly in it, in source order, each by
    /// its span in that module text.
    holes: Vec<Span>,
}

enum TextStorage {
    /// The module's text itself, holes and all: a shipped module's, which
    /// every boot of the thread shares and no edit replaces.
    Shared(Rc<str>),
    /// `range` with the holes cut out, owned: a program's declaration,
    /// whose versions an edit replaces one by one, so that no version
    /// keeps another's text alive.
    Cut(Box<str>),
}

impl DeclarationText {
    /// `range` of `module_text` with each of `holes` cut out, copied.
    /// `holes` are the spans of the declarations nested directly in it, in
    /// source order. `None` when a hole is empty or not within `range`,
    /// the holes overlap or are out of order, or a boundary is not a
    /// character boundary of `module_text`.
    pub fn cut(module_text: &str, range: Span, holes: &[Span]) -> Option<DeclarationText> {
        if !fits(module_text, range, holes) {
            return None;
        }
        let mut owned = String::new();
        for (start, end) in segment_bounds(range, holes) {
            owned.push_str(module_text.get(offset(start)..offset(end))?);
        }
        Some(DeclarationText {
            storage: TextStorage::Cut(owned.into_boxed_str()),
            range,
            holes: holes.to_vec(),
        })
    }

    /// The same, sharing `module_text` instead of copying it: for a module
    /// whose declarations are never replaced.
    pub fn shared(module_text: Rc<str>, range: Span, holes: &[Span]) -> Option<DeclarationText> {
        if !fits(&module_text, range, holes) {
            return None;
        }
        Some(DeclarationText {
            storage: TextStorage::Shared(module_text),
            range,
            holes: holes.to_vec(),
        })
    }

    /// The text an image carries, rebuilt from the parts `range`, `holes`
    /// and `cut_text` answer: `None` unless the holes are in order, none
    /// empty, do not overlap and lie within `range`, `cut_text` is as long
    /// as `range` less the holes, and every piece between the holes begins
    /// and ends on a character boundary of `cut_text`. Checked here, once, so that
    /// a text an image chose answers `slice` and `assemble` like any other.
    pub fn from_parts(
        range: Span,
        holes: Vec<Span>,
        cut_text: Box<str>,
    ) -> Option<DeclarationText> {
        if !holes_fit(range, &holes) {
            return None;
        }
        let removed: u64 = holes
            .iter()
            .map(|hole| u64::from(hole.end - hole.start))
            .sum();
        let expected = u64::from(range.end - range.start) - removed;
        if u64::try_from(cut_text.len()).ok()? != expected {
            return None;
        }
        let mut piece_end = 0;
        for (start, end) in segment_bounds(range, &holes) {
            piece_end += offset(end - start);
            if !cut_text.is_char_boundary(piece_end) {
                return None;
            }
        }
        Some(DeclarationText {
            storage: TextStorage::Cut(cut_text),
            range,
            holes,
        })
    }

    /// The text `span` covers, which must lie within the declaration and in
    /// no hole; `None` otherwise.
    pub fn slice(&self, span: Span) -> Option<&str> {
        if span.start > span.end || span.start < self.range.start || span.end > self.range.end {
            return None;
        }
        if self
            .holes
            .iter()
            .any(|hole| span.start < hole.end && hole.start < span.end)
        {
            return None;
        }
        match &self.storage {
            TextStorage::Shared(text) => source_text(text, span),
            TextStorage::Cut(text) => {
                let start = self.cut_offset(span.start);
                text.get(start..start + offset(span.end - span.start))
            }
        }
    }

    /// The declaration's whole text with `nested[i]` in hole `i`; `None`
    /// unless there is one text per hole.
    pub fn assemble(&self, nested: &[String]) -> Option<String> {
        if nested.len() != self.holes.len() {
            return None;
        }
        let segments = self.segments()?;
        let length = segments.iter().map(|segment| segment.len()).sum::<usize>()
            + nested.iter().map(String::len).sum::<usize>();
        let mut assembled = String::with_capacity(length);
        for (index, segment) in segments.iter().enumerate() {
            assembled.push_str(segment);
            if let Some(filling) = nested.get(index) {
                assembled.push_str(filling);
            }
        }
        Some(assembled)
    }

    /// Where the declaration began and ended in the module text it was
    /// read in: what an image writes.
    pub fn range(&self) -> Span {
        self.range
    }

    /// The spans of its holes in the module text it was read in, in source
    /// order: what an image writes.
    pub fn holes(&self) -> &[Span] {
        &self.holes
    }

    /// The text with every hole cut out: what an image writes.
    pub fn cut_text(&self) -> std::borrow::Cow<'_, str> {
        match &self.storage {
            TextStorage::Cut(text) => std::borrow::Cow::Borrowed(text),
            TextStorage::Shared(_) => {
                std::borrow::Cow::Owned(self.segments().unwrap_or_default().concat())
            }
        }
    }

    /// Whether the text shares its module's instead of owning a copy.
    #[cfg(test)]
    pub fn is_shared(&self) -> bool {
        matches!(self.storage, TextStorage::Shared(_))
    }

    /// The text between the holes, one more piece than there are holes.
    /// Never `None` for a text `cut` or `shared` made, which checked every
    /// boundary.
    fn segments(&self) -> Option<Vec<&str>> {
        segment_bounds(self.range, &self.holes)
            .map(|(start, end)| match &self.storage {
                TextStorage::Shared(text) => text.get(offset(start)..offset(end)),
                TextStorage::Cut(text) => {
                    let cut_start = self.cut_offset(start);
                    text.get(cut_start..cut_start + offset(end - start))
                }
            })
            .collect()
    }

    /// Where the module text's offset `position`, in no hole, lies in the
    /// cut text: past the range's start, less every hole before it.
    fn cut_offset(&self, position: u32) -> usize {
        let removed: u32 = self
            .holes
            .iter()
            .filter(|hole| hole.end <= position)
            .map(|hole| hole.end - hole.start)
            .sum();
        offset(position - self.range.start - removed)
    }

    /// The length of the text with every hole cut out.
    fn cut_length(&self) -> usize {
        let holes: u32 = self.holes.iter().map(|hole| hole.end - hole.start).sum();
        offset(self.range.end - self.range.start - holes)
    }
}

/// Prints the extent, the hole count and the length, never the text, so
/// that printing a mixin, whose `Debug` is derived, prints no module.
impl std::fmt::Debug for DeclarationText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "DeclarationText({}..{}, {} holes, {} bytes)",
            self.range.start,
            self.range.end,
            self.holes.len(),
            self.cut_length()
        )
    }
}

/// A `u32` offset as a `usize`, which holds every `u32` on the targets
/// the runtime builds for.
fn offset(value: u32) -> usize {
    value as usize
}

/// Whether `range` lies within `module_text`, each of `holes` within
/// `range` and after the one before it, and every boundary on a character
/// boundary of `module_text`.
fn fits(module_text: &str, range: Span, holes: &[Span]) -> bool {
    let boundary = |position: u32| module_text.is_char_boundary(offset(position));
    if range.start > range.end || offset(range.end) > module_text.len() {
        return false;
    }
    if !boundary(range.start) || !boundary(range.end) {
        return false;
    }
    holes_fit(range, holes)
        && holes
            .iter()
            .all(|hole| boundary(hole.start) && boundary(hole.end))
}

/// Whether `range` ends no earlier than it starts and each of `holes` lies
/// within it, after the one before it, and covers at least one byte. A
/// hole is a nested declaration, which is never empty, so an empty one is
/// a text no compiler made: an image's, refused like any other boundary
/// it got wrong.
fn holes_fit(range: Span, holes: &[Span]) -> bool {
    if range.start > range.end {
        return false;
    }
    let mut previous_end = range.start;
    for hole in holes {
        if hole.start < previous_end || hole.start >= hole.end || hole.end > range.end {
            return false;
        }
        previous_end = hole.end;
    }
    true
}

/// The module-text bounds of the pieces between `holes` within `range`:
/// from the range's start to the first hole, between each two, and from
/// the last hole to the range's end.
fn segment_bounds(range: Span, holes: &[Span]) -> impl Iterator<Item = (u32, u32)> + '_ {
    let starts = std::iter::once(range.start).chain(holes.iter().map(|hole| hole.end));
    let ends = holes
        .iter()
        .map(|hole| hole.start)
        .chain(std::iter::once(range.end));
    starts.zip(ends)
}

/// The part of `text` that `span` covers, or `None` unless `span.start <=
/// span.end <= text.len()` and both ends fall on character boundaries: a
/// span read from an image is the image's to choose, so a span past the
/// text or inside a character answers no source rather than a panic.
pub fn source_text(text: &str, span: Span) -> Option<&str> {
    let start = usize::try_from(span.start).ok()?;
    let end = usize::try_from(span.end).ok()?;
    text.get(start..end)
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
            superclass_receiver: None,
            mixin_applications: Vec::new(),
            has_body: true,
            has_initializer_expressions: false,
            initializer_expressions: Vec::new(),
            nested_declarations: NestedDeclarations::default(),
            class_side: None,
            span: Span::new(0, 0),
            declaration: None,
            initializer_code: None,
            own_initializer_code: None,
            standalone_initializer_code: None,
            superclass_clause_code: None,
            root_clause_defect: None,
            root_shape: None,
            shipped: false,
            enclosing: OnceCell::new(),
            text: None,
        }
    }

    /// The mixin this one is declared in (`Mixin::enclosing`), while it
    /// lives; `None` for a module's root and a builtin's mixin.
    pub fn enclosing_mixin(&self) -> Option<Rc<Mixin>> {
        self.enclosing.get().and_then(Weak::upgrade)
    }

    /// Whether this mixin is a class side, declared or synthesized: every
    /// class side, and nothing else, carries `Symbol::CLASS_SIDE` as its
    /// name, which no declaration can spell.
    pub fn is_class_side(&self) -> bool {
        self.name == Symbol::CLASS_SIDE
    }

    /// The name a reader is shown for this mixin: its declaration's, and a
    /// class side's `<instance side's name> class`, as a metaclass is
    /// named. A class side whose instance side no longer lives is named
    /// `class side`.
    pub fn reflected_name(&self) -> String {
        if !self.is_class_side() {
            return self.name.to_string();
        }
        match self.enclosing_mixin() {
            Some(instance_side) => format!("{} class", instance_side.name),
            None => self.name.to_string(),
        }
    }

    /// Links every nested mixin of `parent`, and its class side, to
    /// `parent`, once `parent`'s `Rc` exists. A child already linked keeps
    /// its link: the builders build trees, and `image::mixins` proves an
    /// image's rows are one (`check_preorder`), so no child has two
    /// parents to choose between.
    pub fn set_enclosing_of_children(parent: &Rc<Mixin>) {
        for child in parent
            .declared_nested()
            .iter()
            .chain(parent.class_side.iter())
        {
            // `Err` is a cell already set, which keeps its link.
            let _ = child.enclosing.set(Rc::downgrade(parent));
        }
    }

    /// `self` with `nested` as its nested declarations, in declaration
    /// order: what a builder sets before the mixin's `Rc` exists.
    pub fn with_declared_nested(mut self, nested: Vec<Rc<Mixin>>) -> Mixin {
        self.nested_declarations = NestedDeclarations::new(nested);
        self
    }

    /// The nested declarations as they were built, before any heap
    /// registered the mixin: what the compiler, the shipped-code decoder,
    /// the image's mixin builder and an edit's pairing read. Empty once a
    /// heap has registered the mixin, which took them (`Heap::nested_of`
    /// answers them by id from then on); a debug build refuses that read,
    /// since an empty answer there is a reader that should have asked the
    /// heap.
    pub fn declared_nested(&self) -> Ref<'_, [Rc<Mixin>]> {
        debug_assert!(
            !self.is_adopted(),
            "internal: read nested_of for a registered mixin"
        );
        Ref::map(self.nested_declarations.list.borrow(), Vec::as_slice)
    }

    /// Whether a heap has taken this mixin's nested declarations.
    pub fn is_adopted(&self) -> bool {
        self.nested_declarations.adopted.get()
    }

    /// Hands the declared list to the heap registering the mixin and marks
    /// it adopted: from here on the heap's ids are the only names of its
    /// nested declarations, so freeing a row can drop one.
    pub(crate) fn take_declared_nested(&self) -> Vec<Rc<Mixin>> {
        debug_assert!(
            !self.is_adopted(),
            "internal: a mixin is registered in at most one heap"
        );
        self.nested_declarations.adopted.set(true);
        std::mem::take(&mut *self.nested_declarations.list.borrow_mut())
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

/// A mutable slot and the class that declares it, recorded wherever such a
/// slot exists anywhere in an application's chain -- whether or not that
/// chain inherits `Value`, since a `<:` clause can put `Value`'s level below
/// the declaring class (`MixinApplication::first_mutable_slot`). It becomes
/// a refusal, §3.1.1's "a value object is deeply immutable", only where the
/// chain does inherit `Value`: a mutable slot is otherwise ordinary and
/// legal. Both names are recorded because the class that declares the slot
/// need not be the class being instantiated -- the refusal has to say which
/// level is at fault.
#[derive(Debug, Clone, Copy)]
pub struct MutableValueSlot {
    pub class: Symbol,
    pub slot: Symbol,
}

/// Why a module's root class breaks the root rule (§6.1, §3.8): its
/// clauses name something other than `Object`, `Value` or `Object <: Value`,
/// or send them anything but the superclass clause's `new`. The resolver
/// refuses such a module (`resolver::root_clause_defect`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootClauseDefect {
    /// The superclass clause, as written without its message, when it is
    /// prefixed or names anything but `Object` or `Value`.
    Superclass { clause: String },
    /// The superclass clause names `Object` or `Value` but sends it
    /// `selector` rather than `new`.
    SuperclassMessage { clause: String, selector: String },
    /// A `<:` clause other than one unprefixed `Value`; `index` is 0-based.
    MixinApplication { index: usize, clause: String },
    /// The one `<:` clause names `Value` but sends it `selector`, where the
    /// rule's `Object <: Value` sends nothing; `index` is 0-based.
    MixinApplicationMessage {
        index: usize,
        clause: String,
        selector: String,
    },
    /// `Value <: Value`: the one `<:` clause is right, the superclass is not.
    ValueOverValue,
}

impl RootClauseDefect {
    /// The resolve error's text, and the tail both the instantiation refusal
    /// and the encoder print.
    pub fn describe(&self) -> String {
        let rule =
            "a module's root class may inherit only Object or Value, or write Object <: Value";
        match self {
            RootClauseDefect::Superclass { clause } => {
                format!("{rule}, and its superclass clause names {clause}")
            }
            RootClauseDefect::MixinApplication { index, clause } => {
                format!(
                    "{rule}, and its mixin application clause {} names {clause}",
                    index + 1
                )
            }
            RootClauseDefect::SuperclassMessage { clause, selector } => {
                format!("{rule}, and its superclass clause sends {selector} to {clause}, not new")
            }
            RootClauseDefect::MixinApplicationMessage {
                index,
                clause,
                selector,
            } => {
                format!(
                    "{rule}, and its mixin application clause {} sends {selector} to {clause}",
                    index + 1
                )
            }
            RootClauseDefect::ValueOverValue => format!("{rule}, and it applies Value to Value"),
        }
    }
}

/// What `allocate_for_factory` refuses before it allocates anything.
#[derive(Debug, Clone)]
pub enum InstantiationRefusal {
    RootClause(RootClauseDefect),
    MutableValueSlot(MutableValueSlot),
    /// §3.1.1: a value's class "does not define an identity method (==)".
    /// `declaring` is the first level below `Value` whose mixin declares an
    /// instance-side `==`. Two kinds of level are never asked: `Value`'s own,
    /// whose `==` the specification's next sentence allows, and every level
    /// above `Value`, since `Value`'s `==` overrides whatever they declare. Computed once per application by
    /// `Mutator::apply_mixin`, the same place `MutableValueSlot` is: this is
    /// a property of the application, not of any class object built from
    /// it, so a module's root -- which `apply_mixin` builds an application
    /// for like any other, but which no class-object-creation hook such as
    /// `nested::after_clause` ever runs for -- is covered without a check
    /// of its own.
    IdentityMethod {
        declaring: Symbol,
    },
    /// §3.2: a class object is made only by evaluating a declaration --
    /// the one way a metaclass stays unique to its class. Every class
    /// whose chain reaches `Behavior` is refused: `Class`, `Metaclass`,
    /// every metaclass, and any subclass of them a program manages to
    /// declare.
    ClassObject,
}

#[derive(Debug)]
pub struct MixinApplication {
    /// Its index in its heap's table, the full-width identity every class
    /// object of it names and every identity comparison of applications uses
    /// (`docs/design/object-model.md`, "An application has two ids"); set by
    /// `Heap::register_application`.
    pub(crate) index: crate::heap::ApplicationIndex,
    pub mixin: Rc<Mixin>,
    pub superclass: Option<Rc<MixinApplication>>,
    /// §3.1.1: whether this application's superclass chain reaches `Value`,
    /// which is what makes its instances value objects -- deeply immutable,
    /// equal by their state and free to be copied across an actor boundary.
    /// Computed once per application (`Mutator::apply_mixin`), because the
    /// same mixin applied over a superclass that is no value is no value
    /// either.
    ///
    /// `Value`'s own application answers `true` (`Mutator::apply_mixin`'s
    /// `is_value_mixin`; `interpreter::kernel` pins it): §3.2's "subclass of"
    /// is inclusive, so `Value new` is a value. The enclosing-objects walk
    /// (`interpreter::value_class::check_value_class`) relies on it: it stops
    /// early at a class that inherits `Value` and was created as a class of
    /// its own, and `Value` itself is where that stop happens for every
    /// class declared as `Value new`. `Kernel.ns` gives `Value` no slots, so
    /// the rule below finds nothing at its level.
    pub inherits_value: bool,
    /// The first mutable slot anywhere in this application's chain, whether
    /// or not the chain inherits `Value`: a `<:` clause can put `Value`'s
    /// level below a class that declares one, and the instances of every
    /// class built on that level carry the slot. `instantiation_refusal`
    /// turns it into a refusal only where the chain inherits `Value`, so a
    /// class with no value in its chain keeps its mutable slots legally.
    pub first_mutable_slot: Option<MutableValueSlot>,
    /// What `interpreter::instantiate` refuses before allocating an
    /// instance, computed once per application by `Mutator::apply_mixin`:
    /// a root clause defect (on this level or inherited), `first_mutable_slot`
    /// when the chain inherits `Value`, or -- also only then -- the first
    /// level below `Value` that declares an instance-side `==`
    /// (`InstantiationRefusal::IdentityMethod`'s own doc comment says which
    /// levels count). One field, so the instantiation path tests one
    /// `Option` for all three, and every one of them is inherited through
    /// the superclass's own `instantiation_refusal` the same way.
    pub instantiation_refusal: Option<InstantiationRefusal>,
    /// Absolute index of the hidden cache slot for each nested class
    /// declaration, including inherited ones. The slot stores the cached
    /// nested class object; accessing it is a slot read rather than a
    /// hash lookup. Inherited declarations keep the index their declaring
    /// level gave them, as inherited slots keep theirs.
    ///
    /// Keyed by the declaration's id (`Heap::mixin_id`), which every
    /// version of it answers, so that an edited declaration keeps its
    /// slot; and not by its name, which is the one thing a subclass's
    /// nested declaration shares with the inherited one it shadows. Both
    /// declarations are real, both get their own physical slot, and
    /// `super N` reaches the inherited one while `N` reaches the
    /// subclass's; a name key would collapse the two entries onto one index
    /// and make whichever was resolved first the answer to both.
    pub nested_slot: IdentityMap<MixinId, usize>,
    /// The absolute field indices of the slots this application's classes
    /// **declare**, superclass level first and in declaration order within a
    /// level -- which is the order the fields themselves are laid out in.
    ///
    /// An instance's field vector is not all declared slots: each level lays
    /// its own slots down at `own_slot_base`, then one hidden nested-class
    /// cache slot per nested declaration (`nested_slot`, above), so the
    /// declared slots of a chain are interleaved with hidden ones rather
    /// than forming a prefix.
    ///
    /// **This is what a value's state is**, and the one answer to that
    /// question: §3.1.1 derives equality and hash from the state a class
    /// declares, and a nested class's cache slot is not state -- it holds a
    /// class object, it is per enclosing instance, and it is written by the
    /// accessor rather than by any initializer. Counting it made two equal
    /// values answer `false` once the accessor had run on both, because each
    /// side cached *its own* class object there
    /// (`docs/design/core-library.md`, "A value's state is the slots its
    /// classes *declare*, and nothing else"). A lazy slot (§6.3.3) is left
    /// out too, by the 2026-09-26 decision `declared_slots_of` records: it
    /// is state derived from the rest, filled on one side and not the other.
    ///
    /// Computed once here, like `nested_slot` and `inherits_value`, because
    /// nothing about an application changes after it is built and the
    /// declining path reads it once per slot: `builtins/value.rs`'s walks
    /// and the `slotCount`/`slotAt:` leaves the Newspeak fallback reads
    /// through all borrow this slice, so the leaf and the fallback cannot
    /// disagree about which slots exist or what order they come in, and
    /// neither allocates to ask.
    pub declared_slots: Vec<usize>,
    pub slot_count: usize,
    /// Where this application's *own* slots start -- `superclass.slot_count`,
    /// or 0 with none. Read at run time (`Heap::activation_slot_base`, off
    /// the application the activation was entered at) to turn a compiled
    /// `InitializeSlot`'s mixin-relative `own_index` into this application's absolute field index:
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
    /// The header an ordinary instance of this application is born with,
    /// `None` for one whose word count does not fit in the header; set by
    /// `Heap::register_application`.
    pub instance_header: Option<crate::heap::InstanceHeader>,
    /// What instantiation asks of this level's own mixin (`LevelFacts::of`),
    /// computed where the application is built; a debug build's
    /// `Heap::register_application` checks it against `mixin`.
    pub level_facts: LevelFacts,
}

/// What instantiation asks of one level, read off the level's own mixin
/// when its application is built, so that it is copied out instead of the
/// application cloned: whether the level needs initializer code, its
/// superclass clause's selector and whether that clause passes arguments,
/// how many `<:` clauses its declaration writes and whether any of them
/// passes arguments, the arity of its mixin's own factory, which its
/// initializer bodies are laid out for, and -- for a metaclass mixin -- the
/// factory entry its method table holds. Nothing here depends on the
/// superclass, and a level whose initializer would do nothing but §6.1's
/// checks is passed over without an activation (`Mutator::enter_or_pass`).
///
/// Which factory a *class object* has is not an application fact: it is
/// read through the class object's metaclass (`Heap::class_factory`), whose
/// class side may be another declaration's than the mixin it applies -- a
/// level a `<:` clause added has the factory of the class the clause named,
/// and a body-less declaration's class object its declaration's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelFacts {
    pub needs_initializer_code: bool,
    pub clause_selector: Symbol,
    pub clause_has_arguments: bool,
    pub applied_clause_count: usize,
    pub applied_clauses_pass_arguments: bool,
    pub factory_arity: Option<usize>,
    pub metaclass_factory: Option<Factory>,
}

impl LevelFacts {
    pub(crate) fn of(mixin: &Mixin) -> LevelFacts {
        LevelFacts {
            needs_initializer_code: crate::interpreter::instantiate::mixin_needs_initializer_code(
                mixin,
            ),
            clause_selector: mixin
                .superclass
                .map_or(Symbol::NEW, |clause| clause.selector),
            clause_has_arguments: mixin.superclass.is_some_and(|clause| clause.has_arguments),
            applied_clause_count: mixin.mixin_applications.len(),
            applied_clauses_pass_arguments: mixin
                .mixin_applications
                .iter()
                .any(|clause| clause.message.has_arguments),
            factory_arity: mixin.factory.map(|factory| factory.argument_count),
            metaclass_factory: mixin.methods.values().find_map(|entry| match entry {
                MethodEntry::Factory(factory) => Some(Factory {
                    selector: factory.selector,
                    argument_count: factory.argument_count,
                }),
                _ => None,
            }),
        }
    }
}

/// `MixinApplication::declared_slots` for a level being built: the
/// superclass's list, which is already in this order, and then this level's
/// own declared slots but the lazy ones. Extending the inherited list is
/// what keeps this linear over a chain -- each level pays for its own slots
/// once, when the application is built, rather than re-walking its
/// ancestors.
///
/// A lazy slot (§6.3.3) is left out, as decided with the user on 2026-09-26
/// (`docs/design/core-library.md`, "Lazy slots first"): its value is state
/// derived from the rest -- Bracha's transient fields, purged on
/// synchronization -- so two values of which one has filled it are still
/// equal and hash alike.
pub fn declared_slots_of(
    superclass: Option<&Rc<MixinApplication>>,
    own_slot_base: usize,
    own_slots: &[MixinSlot],
) -> Vec<usize> {
    let mut declared =
        superclass.map_or_else(Vec::new, |superclass| superclass.declared_slots.clone());
    declared.extend(
        own_slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.lazy)
            .map(|(position, _)| own_slot_base + position),
    );
    declared
}

/// What one field of an instance holds, by what it is named rather than
/// where it lies: a declared slot by the mixin of the level declaring it,
/// that mixin's occurrence in the chain and the slot's name; a hidden
/// nested-class cache slot by the nested declaration's id and the
/// occurrence of the level caching it. An install copies an instance's
/// fields into its new layout by these (`MixinApplication::field_names`).
///
/// The declaring mixin is part of a slot's name because a subclass's slot
/// may shadow a superclass's slot of the same name: matched by name alone,
/// a slot an edit adds under or over a shadowing one would take the other
/// level's value. The occurrence is, because one chain may apply a mixin
/// twice (`<:` clauses), and then two levels declare the same names; an
/// install evaluates no superclass clause again, so a level's occurrence
/// is the same before and after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldName {
    Slot {
        mixin: MixinId,
        occurrence: u32,
        name: Symbol,
    },
    Nested {
        declaration: MixinId,
        occurrence: u32,
    },
}

impl MixinApplication {
    /// Its index in its heap's table (`Heap::application` answers it back).
    pub fn index(&self) -> crate::heap::ApplicationIndex {
        self.index
    }

    /// Every field of an instance, by index: each level's own slots from
    /// its `own_slot_base`, then its hidden nested-class slots, read off
    /// `nested_slot` -- superclass first -- each named with the id `heap`
    /// answers for the level's mixin and the count of the levels nearer
    /// the root that apply the same mixin. No two fields share a name: a
    /// level declares a slot name, and caches a nested class, at most
    /// once, and two levels of one mixin differ in their occurrence.
    pub(crate) fn field_names(&self, heap: &crate::heap::Heap) -> Vec<FieldName> {
        let mut levels = Vec::new();
        let mut level = Some(self);
        while let Some(application) = level {
            levels.push(application);
            level = application.superclass.as_deref();
        }
        let mut names = Vec::with_capacity(self.slot_count);
        let mut occurrences: HashMap<MixinId, u32> = HashMap::new();
        for application in levels.into_iter().rev() {
            let id = heap
                .known_mixin_id(&application.mixin)
                .expect("internal: an applied mixin is registered");
            let count = occurrences.entry(id).or_insert(0);
            let occurrence = *count;
            *count += 1;
            names.extend(application.mixin.slots.iter().map(|slot| FieldName::Slot {
                mixin: id,
                occurrence,
                name: slot.name,
            }));
            // This level's cache slots follow its own slots; every entry
            // below `nested_start` is an inherited level's.
            let nested_start = application.own_slot_base + application.mixin.slots.len();
            let mut nested: Vec<(usize, MixinId)> = application
                .nested_slot
                .iter()
                .filter(|(_, index)| **index >= nested_start)
                .map(|(id, index)| (*index, *id))
                .collect();
            nested.sort_unstable_by_key(|(index, _)| *index);
            names.extend(
                nested
                    .into_iter()
                    .map(|(_, declaration)| FieldName::Nested {
                        declaration,
                        occurrence,
                    }),
            );
        }
        debug_assert_eq!(
            names.len(),
            self.slot_count,
            "internal: an application's fields are its levels' slots and cache slots"
        );
        names
    }
}

/// Unlinks the superclass list in a loop rather than through the stack.
///
/// An application chain is as long as the class chain it mirrors, and once
/// a world is decoded from an image that length is untrusted input --
/// `image::decode::rebuild_applications` builds one link per class record.
/// The default glue would release the chain recursively and overflow the
/// stack on a long one, which aborts the process; this releases it in a
/// loop instead. Objects and frames need no such glue -- both are words
/// in the heap's object space, with nothing to unwind on drop -- which
/// makes this the only hand-written iterative `Drop` in the crate. It is
/// not the only
/// recursive graph: `Mixin`'s declared nested and `class_side` links are
/// also `Rc` and also drop recursively, but their depth is bounded by the
/// parser and `image::mixins::read` refuses anything deeper, so a ceiling
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

/// `mixin`, then every nested declaration (recursively), then its class
/// side (recursively) -- the order the image's mixin table assigns row ids
/// in (`image::mixins::plan` walks a registered tree so, through the
/// heap), so a decoded row and the mixin it describes are found at the
/// same position under either walk. This walk reads the declared tree
/// (`Mixin::declared_nested`), so it takes one no heap registered: a tree
/// just built, compiled or decoded.
///
/// Recursive, and deliberately so: the trees it walks are either built from
/// a parsed module (whose nesting depth the parser already bounds) or
/// decoded through `image::mixins::read`, which refuses a tree deeper than
/// its own ceiling. An unbounded tree can never reach this function.
pub fn preorder(mixin: &Rc<Mixin>, out: &mut Vec<Rc<Mixin>>) {
    out.push(mixin.clone());
    for nested in mixin.declared_nested().iter() {
        preorder(nested, out);
    }
    if let Some(class_side) = &mixin.class_side {
        preorder(class_side, out);
    }
}

#[cfg(test)]
mod declaration_text_tests {
    use super::DeclarationText;
    use indoc::indoc;
    use ns_syntax::Span;
    use std::rc::Rc;

    /// A text with no hole, over the whole of `text`.
    fn whole(text: &str) -> DeclarationText {
        DeclarationText::cut(text, Span::new(0, text.len() as u32), &[]).expect("cut")
    }

    /// A span inside the text and on character boundaries answers a slice.
    #[test]
    fn a_span_inside_the_text_answers_its_slice() {
        let text = "abc\u{e9}";
        assert_eq!(text.len(), 5);
        let declaration = whole(text);
        assert_eq!(declaration.slice(Span::new(0, 3)), Some("abc"));
        assert_eq!(declaration.slice(Span::new(0, 5)), Some("abc\u{e9}"));
    }

    /// A span from an image is untrusted: one past the text, inside a
    /// character or reversed answers no source.
    #[test]
    fn a_span_outside_the_text_answers_no_source() {
        let declaration = whole("abc\u{e9}");
        assert_eq!(
            declaration.slice(Span::new(0, 4)),
            None,
            "inside the e-acute"
        );
        assert_eq!(declaration.slice(Span::new(2, 9)), None, "past the end");
        // `Span::new` refuses a reversed span in a debug build; an image's
        // bytes are not built through it.
        let reversed = Span { start: 3, end: 1 };
        assert_eq!(declaration.slice(reversed), None, "reversed");
    }

    /// A module with one nested declaration and a non-ASCII character
    /// before it, so that a boundary can fall inside a character. The
    /// `\u{e4}` escape keeps this file ASCII; `indoc!` resolves it as a
    /// string literal does, and the closing line keeps the final newline.
    const MODULE: &str = indoc! {"
        Newspeak3
        'Test'
        class Outer = (
          (* \u{e4} *) class Inner = (
          ) ( public a = ( ^1 ) )
          public b = ( ^2 )
        )
    "};

    fn span_of(text: &str, needle: &str) -> Span {
        let start = text.find(needle).expect("the needle is in the text");
        Span::new(start as u32, (start + needle.len()) as u32)
    }

    /// The root's text is the whole module with its nested declaration cut
    /// out; filling the hole back gives the module byte for byte.
    #[test]
    fn a_cut_text_assembles_to_the_module() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let whole = Span::new(0, MODULE.len() as u32);
        let root = DeclarationText::cut(MODULE, whole, &[inner]).expect("cut");
        let nested = DeclarationText::cut(MODULE, inner, &[]).expect("cut");
        let nested_text = nested.assemble(&[]).expect("no holes");
        assert_eq!(root.assemble(&[nested_text]).as_deref(), Some(MODULE));
        assert!(!root.cut_text().contains("public a"));
    }

    /// A span after a hole still slices the text it was recorded against,
    /// and one inside a hole or across one slices nothing.
    #[test]
    fn a_span_slices_around_the_holes() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let root =
            DeclarationText::cut(MODULE, Span::new(0, MODULE.len() as u32), &[inner]).expect("cut");
        assert_eq!(
            root.slice(span_of(MODULE, "public b = ( ^2 )")),
            Some("public b = ( ^2 )")
        );
        assert_eq!(
            root.slice(span_of(MODULE, "(* \u{e4} *)")),
            Some("(* \u{e4} *)")
        );
        assert_eq!(root.slice(span_of(MODULE, "public a = ( ^1 )")), None);
        assert_eq!(root.slice(Span::new(inner.start - 1, inner.end + 1)), None);
    }

    /// Holes out of order, overlapping, outside the range or off a
    /// character boundary cut nothing.
    #[test]
    fn a_hole_that_does_not_fit_cuts_nothing() {
        let whole = Span::new(0, MODULE.len() as u32);
        let first = span_of(MODULE, "class Inner");
        let second = span_of(MODULE, "public b");
        assert!(DeclarationText::cut(MODULE, whole, &[second, first]).is_none());
        assert!(DeclarationText::cut(MODULE, whole, &[first, first]).is_none());
        assert!(DeclarationText::cut(MODULE, first, &[second]).is_none());
        let umlaut = MODULE.find('\u{e4}').expect("an umlaut") as u32;
        assert!(
            DeclarationText::cut(MODULE, whole, &[Span::new(umlaut + 1, umlaut + 2)]).is_none()
        );
    }

    /// The shared form answers exactly what the cut form answers.
    #[test]
    fn the_shared_form_answers_as_the_cut_one() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let whole = Span::new(0, MODULE.len() as u32);
        let cut = DeclarationText::cut(MODULE, whole, &[inner]).expect("cut");
        let shared = DeclarationText::shared(Rc::from(MODULE), whole, &[inner]).expect("shared");
        let probe = span_of(MODULE, "public b = ( ^2 )");
        assert_eq!(cut.slice(probe), shared.slice(probe));
        assert_eq!(cut.cut_text(), shared.cut_text());
        assert_eq!(cut.assemble(&["X".into()]), shared.assemble(&["X".into()]));
    }

    /// The parts an image writes rebuild a text that answers as the one
    /// they were taken from.
    #[test]
    fn a_text_rebuilt_from_its_parts_answers_as_the_original() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let whole = Span::new(0, MODULE.len() as u32);
        let original = DeclarationText::cut(MODULE, whole, &[inner]).expect("cut");
        let rebuilt = DeclarationText::from_parts(
            original.range(),
            original.holes().to_vec(),
            Box::from(original.cut_text().as_ref()),
        )
        .expect("the parts describe a declaration");
        let probe = span_of(MODULE, "public b = ( ^2 )");
        assert_eq!(rebuilt.slice(probe), original.slice(probe));
        assert_eq!(
            rebuilt.assemble(&["X".into()]),
            original.assemble(&["X".into()])
        );
    }

    /// Parts that do not describe a declaration rebuild nothing: holes out
    /// of order, a hole past the range, a text of another length than the
    /// range less the holes, and a piece between holes that would split a
    /// character.
    #[test]
    fn parts_that_do_not_fit_rebuild_nothing() {
        let range = Span::new(10, 20);
        let first = Span::new(12, 14);
        let second = Span::new(16, 18);
        let text = || Box::from("abcdef");
        assert!(DeclarationText::from_parts(range, vec![first, second], text()).is_some());
        assert!(DeclarationText::from_parts(range, vec![second, first], text()).is_none());
        assert!(DeclarationText::from_parts(range, vec![Span::new(16, 21)], text()).is_none());
        assert!(DeclarationText::from_parts(range, vec![first], text()).is_none());
        assert!(DeclarationText::from_parts(
            Span::new(0, 4),
            vec![Span::new(1, 2)],
            Box::from("\u{e4}x")
        )
        .is_none());
    }

    /// A hole is a nested declaration, which is never empty: an empty hole
    /// makes no text in any of the three forms, even where everything else
    /// fits.
    #[test]
    fn an_empty_hole_makes_no_text() {
        let whole = Span::new(0, MODULE.len() as u32);
        let empty = Span::new(10, 10);
        assert!(DeclarationText::cut(MODULE, whole, &[empty]).is_none());
        assert!(DeclarationText::shared(Rc::from(MODULE), whole, &[empty]).is_none());
        let range = Span::new(0, 4);
        assert!(
            DeclarationText::from_parts(range, vec![Span::new(2, 2)], Box::from("abcd")).is_none()
        );
        assert!(
            DeclarationText::from_parts(range, vec![Span::new(2, 3)], Box::from("abc")).is_some()
        );
    }
}

#[cfg(test)]
mod dispatch_id_tests {
    use super::DispatchId;

    #[test]
    fn the_reserved_guards_are_the_three_lowest_ids() {
        assert_eq!(DispatchId::NONE.bits(), 0);
        assert_eq!(DispatchId::VACANT.bits(), 1);
        assert_eq!(DispatchId::FORWARDED.bits(), 2);
        assert_eq!(DispatchId::FIRST_MINTED, 3);
        assert_eq!(DispatchId::LIMIT, 1 << 22);
    }
}

#[cfg(test)]
mod applied_clause_tests {
    use super::{AppliedClause, BUILTIN_TAG_MASK, BUILTIN_TAG_SHIFT, CLAUSE_INDEX_LIMIT};
    use crate::heap::MixinId;
    use crate::types::BuiltinClass;

    #[test]
    fn an_applied_clause_round_trips_through_its_small_integer_at_every_edge() {
        let largest_tag = *BuiltinClass::ALL.last().expect("builtins exist");
        let cases = [
            (0u32, 0usize, None),
            (u32::MAX, CLAUSE_INDEX_LIMIT - 1, Some(largest_tag)),
            (7, 1, Some(BuiltinClass::Object)),
            (u32::MAX, 0, None),
        ];
        for (mixin, index, builtin) in cases {
            let mixin = MixinId::from_index(mixin as usize).expect("fits");
            let packed = AppliedClause::pack(mixin, index, builtin);
            assert!(
                crate::fits_small_integer(packed),
                "{packed} is not a SmallInteger"
            );
            assert_eq!(AppliedClause::unpack(packed), Some((mixin, index, builtin)));
        }
    }

    #[test]
    fn a_negative_pattern_with_bit_sixty_set_is_read_back_through_its_sign_extension() {
        // A tag of 32 or more sets bit 60, so the packed SmallInteger is
        // negative, and `unpack` must answer by the 61-bit pattern: the tag
        // one past the last builtin's names no builtin, and the last
        // builtin's tag, `BuiltinClass::ALL.len()`, names it. Both are
        // derived from the list, so a new builtin keeps the test's meaning.
        let unused_tag = BuiltinClass::ALL.len() as i64 + 1;
        assert!(BuiltinClass::ALL.len() >= 32, "the largest tag sets bit 60");
        let packed = ((unused_tag << 55) << 3) >> 3;
        assert!(packed < 0);
        assert_eq!(AppliedClause::unpack(packed), None);
        let largest = *BuiltinClass::ALL.last().expect("builtins exist");
        let mixin = MixinId::from_index(0).expect("fits");
        let packed = AppliedClause::pack(mixin, 0, Some(largest));
        assert!(packed < 0);
        assert_eq!(
            (packed as u64 >> BUILTIN_TAG_SHIFT) & BUILTIN_TAG_MASK,
            BuiltinClass::ALL.len() as u64
        );
        assert_eq!(
            AppliedClause::unpack(packed),
            Some((mixin, 0, Some(largest)))
        );
    }

    #[test]
    fn an_unpacked_builtin_tag_past_the_builtins_is_refused() {
        // tag 63 names no builtin: 63 - 1 = 62 >= BuiltinClass::ALL.len()
        let packed = ((63i64 << 55) << 3) >> 3;
        assert_eq!(AppliedClause::unpack(packed), None);
    }
}
