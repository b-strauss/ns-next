//! The encoding a compiled body is stored and run in: one eight-byte
//! `CompactInstruction` per `Instruction`, with everything that does not
//! fit eight bytes in side tables beside them
//! (`docs/design/execution.md`, "The compact instruction encoding").
//! `Instruction` stays the vocabulary: a lowering or the image
//! decoder emits one into a `CodeBuilder`, which encodes it at once, and
//! every tool reads a body back through `EncodedBody::instruction`, an
//! exact inverse.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CallSite, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{Immediate, ImmediateKind};
use crate::Symbol;

/// One instruction in eight bytes, matched on by the run loop. A register
/// is a `u8`, a jump target an absolute `u32` instruction index, and
/// anything else an index into one of the body's tables. An instruction
/// whose operands do not fit is a `Wide` (a send has its own wide forms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompactInstruction {
    LoadNil {
        destination: u8,
    },
    LoadTrue {
        destination: u8,
    },
    LoadFalse {
        destination: u8,
    },
    LoadSmallInteger {
        destination: u8,
        value: i32,
    },
    LoadConstant {
        destination: u8,
        constant: u16,
    },
    LoadLiteral {
        destination: u8,
        literal: u16,
    },
    LoadSelf {
        destination: u8,
    },
    LoadEnclosingObject {
        destination: u8,
        depth: u16,
    },
    Move {
        destination: u8,
        source: u8,
    },
    StoreLocal {
        source: u8,
        slot: u8,
    },
    LoadOuterLocal {
        destination: u8,
        index: u8,
        depth: u16,
    },
    StoreOuterLocal {
        source: u8,
        index: u8,
        depth: u16,
    },
    SendPublic0 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        site: u16,
    },
    SendPublic1 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        source: u8,
        site: u16,
    },
    SendPublic2 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        sources: [u8; 2],
        site: u16,
    },
    SendPublicWide {
        site: u32,
    },
    SendInlinable0 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        site: u16,
    },
    SendInlinable1 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        source: u8,
        site: u16,
    },
    SendInlinable2 {
        destination: u8,
        receiver: u8,
        argument_base: u8,
        sources: [u8; 2],
        site: u16,
    },
    SendInlinableWide {
        site: u32,
    },
    SendLexical0 {
        destination: u8,
        argument_base: u8,
        site: u16,
    },
    SendLexical1 {
        destination: u8,
        argument_base: u8,
        source: u8,
        site: u16,
    },
    SendLexical2 {
        destination: u8,
        argument_base: u8,
        sources: [u8; 2],
        site: u16,
    },
    SendLexicalWide {
        site: u32,
    },
    SendSuper0 {
        destination: u8,
        argument_base: u8,
        site: u16,
    },
    SendSuper1 {
        destination: u8,
        argument_base: u8,
        source: u8,
        site: u16,
    },
    SendSuper2 {
        destination: u8,
        argument_base: u8,
        sources: [u8; 2],
        site: u16,
    },
    SendSuperWide {
        site: u32,
    },
    SendMirror {
        destination: u8,
        mirror: u8,
        subject: u8,
        primitive: MirrorPrimitive,
        site: u16,
    },
    MakeClosure {
        destination: u8,
        block: u16,
    },
    MakeArray {
        destination: u8,
        elements: u16,
    },
    MakeVector {
        destination: u8,
        size: u16,
    },
    LoadVector {
        destination: u8,
        vector: u8,
        index: u16,
    },
    StoreVector {
        vector: u8,
        source: u8,
        index: u16,
    },
    InitializeSlot {
        source: u8,
        own_index: u32,
    },
    ReturnSlotIfFilled {
        own_index: u32,
    },
    MakeFuture {
        destination: u8,
        closure: u8,
    },
    ResolveFuture {
        destination: u8,
        source: u8,
    },
    LoadOwnSlot {
        destination: u8,
        own_index: u32,
    },
    InitializeSlotProvisionally {
        source: u8,
        own_index: u32,
    },
    Return {
        register: u8,
    },
    ImplicitReturn {
        register: u8,
    },
    NonLocalReturn {
        register: u8,
    },
    SignalNonBoolean {
        register: u8,
    },
    Jump {
        target: u32,
    },
    JumpIfNotBoolean {
        register: u8,
        target: u32,
    },
    BranchIfFalse {
        register: u8,
        target: u32,
    },
    BranchIfTrue {
        register: u8,
        target: u32,
    },
    IntegerAdd {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerSubtract {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerMultiply {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerFloorDivide {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerLess {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerGreater {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerLessOrEqual {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerGreaterOrEqual {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    IntegerEqual {
        destination: u8,
        left: u8,
        right: u8,
        target: u32,
    },
    AllocateInstance {
        destination: u8,
    },
    InitializeInstance {
        destination: u8,
        instance: u8,
    },
    CheckAppliedClause {
        steps: u16,
        selector: Symbol,
    },
    InitializeSuperclass {
        destination: u8,
        site: u16,
    },
    InitializeAppliedLevel {
        destination: u8,
        argument_base: u8,
        steps: u16,
        arguments: u16,
    },
    Wide {
        index: u32,
    },
    /// A breakpoint set at this slot (`EncodedBody::patch`), the
    /// instruction it replaced kept beside the slots. Only the run loop's
    /// fetch reads it: it is never emitted, never decoded and has no wire
    /// tag, and every other reader asks for the original
    /// (`docs/design/debugger.md`, "Where the original lives").
    Breakpoint,
}

// The point of the encoding: a slot the loop copies into registers whole.
const _: () = assert!(std::mem::size_of::<CompactInstruction>() == 8);

// The tables below, and every field of the sites they hold, are private
// to this module: a slot and the entry it names are written together, by
// `CodeBuilder` alone, and only `for_each_emitted_mut` rewrites an entry
// afterwards (a call site or a block, never a slot's shape). Code that
// changed one without the other would leave a slot naming an entry of the
// wrong shape, which a decoded read answers with a panic. The run loop
// reads through the `#[inline(always)]` accessors, which compile to the
// same index as a field access.

/// A send's selector and call site, and for a `SendLexical` its depth;
/// for a wide form also its registers.
#[derive(Debug, Clone, PartialEq)]
pub struct SendSite {
    selector: Symbol,
    call_site: CallSite,
    depth: u16,
    wide: Option<Box<WideSend>>,
}

impl SendSite {
    #[inline(always)]
    pub fn selector(&self) -> Symbol {
        self.selector
    }

    #[inline(always)]
    pub fn call_site(&self) -> CallSite {
        self.call_site
    }

    /// `SendLexical`'s depth; 0 for every other kind.
    #[inline(always)]
    pub fn depth(&self) -> u16 {
        self.depth
    }

    /// The registers of a send in a wide form; `None` for an eight-byte
    /// one, whose registers are in its slot.
    #[inline(always)]
    pub fn wide(&self) -> Option<&WideSend> {
        self.wide.as_deref()
    }
}

/// A cached send's operands, as `EncodedBody::cached_send` reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CachedSend {
    pub(crate) kind: CachedSendKind,
    pub(crate) selector: Symbol,
    pub(crate) call_site: CallSite,
    /// The first argument slot.
    pub(crate) argument_base: u16,
    pub(crate) argument_count: usize,
}

/// Which cached send a `CachedSend` is, with what only its kind has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CachedSendKind {
    Public { receiver: u16 },
    Lexical { depth: u16 },
    Super,
}

/// The registers of a send that has no eight-byte form: its argument slots
/// start at `argument_base`, and `arguments` are their sources.
#[derive(Debug, Clone, PartialEq)]
pub struct WideSend {
    destination: u16,
    receiver: u16,
    argument_base: u16,
    arguments: Box<[u16]>,
}

impl WideSend {
    #[inline(always)]
    pub fn destination(&self) -> u16 {
        self.destination
    }

    /// `SendPublic`'s and `SendInlinable`'s receiver; 0 for the others.
    #[inline(always)]
    pub fn receiver(&self) -> u16 {
        self.receiver
    }

    /// The first argument slot.
    #[inline(always)]
    pub fn argument_base(&self) -> u16 {
        self.argument_base
    }

    /// The register each argument slot is copied from.
    #[inline(always)]
    pub fn arguments(&self) -> &[u16] {
        &self.arguments
    }
}

/// What a narrow `MakeClosure` creates its closure from.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockSite {
    code: Rc<CompiledCode>,
    parameters: Rc<Vec<Symbol>>,
    copied: Box<[u16]>,
}

impl BlockSite {
    #[inline(always)]
    pub fn code(&self) -> &Rc<CompiledCode> {
        &self.code
    }

    #[inline(always)]
    pub fn parameters(&self) -> &Rc<Vec<Symbol>> {
        &self.parameters
    }

    /// The registers whose values the closure is handed.
    #[inline(always)]
    pub fn copied(&self) -> &[u16] {
        &self.copied
    }
}

/// A narrow `SendMirror`'s argument slots and failure target.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorSite {
    argument_base: u16,
    arguments: Box<[u16]>,
    failure: u32,
}

impl MirrorSite {
    /// The first argument slot.
    #[inline(always)]
    pub fn argument_base(&self) -> u16 {
        self.argument_base
    }

    /// The register each argument slot is copied from.
    #[inline(always)]
    pub fn arguments(&self) -> &[u16] {
        &self.arguments
    }

    /// Where the inlined failure block stands.
    #[inline(always)]
    pub fn failure(&self) -> u32 {
        self.failure
    }
}

/// A narrow `InitializeSuperclass`'s operands but its destination: the
/// selector does not fit the slot beside the steps.
#[derive(Debug, Clone, PartialEq)]
pub struct SuperclassSite {
    steps: u16,
    selector: Symbol,
    argument_base: u16,
    arguments: Box<[u16]>,
}

impl SuperclassSite {
    /// How many levels above the activation's class the superclass is.
    #[inline(always)]
    pub fn steps(&self) -> u16 {
        self.steps
    }

    /// The selector §6.1 checks the superclass's primary factory
    /// against.
    #[inline(always)]
    pub fn selector(&self) -> Symbol {
        self.selector
    }

    /// The first argument slot.
    #[inline(always)]
    pub fn argument_base(&self) -> u16 {
        self.argument_base
    }

    /// The register each argument slot is copied from.
    #[inline(always)]
    pub fn arguments(&self) -> &[u16] {
        &self.arguments
    }
}

/// A compiled body as it is stored: the slots and their tables. It has no
/// `PartialEq`: two encodings of one body differ (one built by
/// `CodeBuilder::always_wide`, say), and `CompiledCode` compares what a body
/// means.
#[derive(Debug, Default)]
pub struct EncodedBody {
    /// The instructions the run loop fetches. A `Cell` each, so that a
    /// breakpoint can replace one in a body every frame, mixin and closure
    /// shares (`patch`); written nowhere else once the body is finished.
    slots: Box<[Cell<CompactInstruction>]>,
    /// The instruction each patched slot held, by slot index, in the order
    /// the slots were patched. A boxed slice, not a `Vec` or a map: a body
    /// never patched -- nearly every one -- pays three words and no
    /// allocation for it, and a body holds a handful of breakpoints at
    /// most, which a patch or a restore rebuilds the slice for.
    originals: RefCell<Box<[(usize, CompactInstruction)]>>,
    constants: Box<[Immediate]>,
    literals: Box<[Literal]>,
    send_sites: Box<[SendSite]>,
    blocks: Box<[BlockSite]>,
    mirror_sites: Box<[MirrorSite]>,
    superclass_sites: Box<[SuperclassSite]>,
    register_lists: Box<[Box<[u16]>]>,
    wide: Box<[Instruction]>,
}

/// A copy is another body, in which no breakpoint was set: its slots are
/// the originals.
impl Clone for EncodedBody {
    fn clone(&self) -> EncodedBody {
        // Destructured, so that a field added to `EncodedBody` must be
        // decided here.
        let EncodedBody {
            slots,
            originals: _,
            constants,
            literals,
            send_sites,
            blocks,
            mirror_sites,
            superclass_sites,
            register_lists,
            wide,
        } = self;
        EncodedBody {
            slots: (0..slots.len())
                .map(|index| Cell::new(self.original(index)))
                .collect(),
            originals: RefCell::default(),
            constants: constants.clone(),
            literals: literals.clone(),
            send_sites: send_sites.clone(),
            blocks: blocks.clone(),
            mirror_sites: mirror_sites.clone(),
            superclass_sites: superclass_sites.clone(),
            register_lists: register_lists.clone(),
            wide: wide.clone(),
        }
    }
}

/// A send call site or a block, as `EncodedBody::for_each_emitted_mut`
/// visits them: the image decoder numbers a family's sends in emission
/// order, blocks included.
pub enum Emitted<'a> {
    CallSite(&'a mut CallSite),
    Block(&'a mut Rc<CompiledCode>),
}

/// Encodes `Instruction`s as they are emitted.
pub struct CodeBuilder {
    slots: Vec<CompactInstruction>,
    constants: Vec<Immediate>,
    literals: Vec<Literal>,
    send_sites: Vec<SendSite>,
    blocks: Vec<BlockSite>,
    mirror_sites: Vec<MirrorSite>,
    superclass_sites: Vec<SuperclassSite>,
    register_lists: Vec<Box<[u16]>>,
    wide: Vec<Instruction>,
    always_wide: bool,
}

/// One more entry than a slot's sixteen-bit table index can name: an
/// instruction that would append past it takes its wide form.
const TABLE_LIMIT: usize = u16::MAX as usize + 1;

/// Whether `register` fits a slot's byte.
fn fits(register: u16) -> bool {
    register <= u16::from(u8::MAX)
}

/// `register` in a slot's byte. Only an instruction `fits_narrow`
/// admitted is encoded in its eight-byte form, and it admits one only when
/// every register that goes into the slot fits a byte.
fn narrow_register(register: u16) -> u8 {
    u8::try_from(register).expect(
        "internal: fits_narrow admits an eight-byte form only when its registers fit a byte",
    )
}

/// The sixteen-bit index the next entry of a side table of `length`
/// entries takes. `fits_narrow` admits an eight-byte form that appends to
/// a table only while the table holds fewer than `TABLE_LIMIT` entries.
fn narrow_index(length: usize) -> u16 {
    u16::try_from(length).expect(
        "internal: fits_narrow admits an eight-byte form only while its table is below TABLE_LIMIT",
    )
}

/// The four kinds of send that share one shape: a selector, a call site,
/// a destination and up to two arguments in eight bytes.
#[derive(Clone, Copy)]
enum SendKind {
    Public,
    Inlinable,
    Lexical,
    Super,
}

/// How a `LoadLiteral` is loaded: by an opcode of its own, a 32-bit
/// integer in the slot, a word from `constants`, or -- for what must be
/// allocated each time it runs, and a float no immediate holds -- from
/// `literals`.
enum LiteralForm {
    Nil,
    Boolean(bool),
    SmallInteger(i32),
    Constant(Immediate),
    Allocated,
}

fn literal_form(literal: &Literal) -> LiteralForm {
    match literal {
        Literal::Nil => LiteralForm::Nil,
        Literal::Boolean(value) => LiteralForm::Boolean(*value),
        Literal::SmallInteger(value) => match i32::try_from(*value) {
            Ok(value) => LiteralForm::SmallInteger(value),
            // A well-formed `SmallInteger` literal always has an immediate;
            // the fallback keeps encoding total.
            Err(_) => Immediate::small_integer(*value)
                .map_or(LiteralForm::Allocated, LiteralForm::Constant),
        },
        Literal::Float(value) => {
            Immediate::small_float(*value).map_or(LiteralForm::Allocated, LiteralForm::Constant)
        }
        Literal::String(_)
        | Literal::Symbol(_)
        | Literal::LargeInteger(_)
        | Literal::UnresolvedIdentifier(_)
        | Literal::BodyTooLarge(_) => LiteralForm::Allocated,
    }
}

impl Default for CodeBuilder {
    fn default() -> CodeBuilder {
        CodeBuilder::new()
    }
}

impl CodeBuilder {
    /// A builder that takes a wide form only where an instruction has no
    /// eight-byte one, or for every instruction when the `NS_NEXT_STRESS_WIDE`
    /// stress mode asks: what the compiler builds, so that the whole suite
    /// runs the wide forms under that mode.
    pub fn new() -> CodeBuilder {
        CodeBuilder::with_always_wide(crate::interpreter::safepoint::stress_wide_from_env())
    }

    /// A builder that takes a wide form only where an instruction has no
    /// eight-byte one, whatever a stress mode says: what a test of the
    /// narrow forms builds, so that its assertions hold under every mode.
    pub fn never_wide_by_stress() -> CodeBuilder {
        CodeBuilder::with_always_wide(false)
    }

    /// A builder that encodes every instruction in a wide form.
    pub fn always_wide() -> CodeBuilder {
        CodeBuilder::with_always_wide(true)
    }

    fn with_always_wide(always_wide: bool) -> CodeBuilder {
        CodeBuilder {
            slots: Vec::new(),
            constants: Vec::new(),
            literals: Vec::new(),
            send_sites: Vec::new(),
            blocks: Vec::new(),
            mirror_sites: Vec::new(),
            superclass_sites: Vec::new(),
            register_lists: Vec::new(),
            wide: Vec::new(),
            always_wide,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Encodes `instruction` into the next slot and answers its index.
    /// Never past `INSTRUCTION_LIMIT` instructions: both lowerings stop
    /// emitting there and compile the body to `Literal::BodyTooLarge`, and
    /// the image decoder refuses a longer body before it reads one, so a
    /// longer one is a defect -- and every index into a table this body
    /// appends to fits a `u32`.
    pub fn emit(&mut self, instruction: Instruction) -> usize {
        let index = self.slots.len();
        assert!(
            index < crate::bytecode::INSTRUCTION_LIMIT as usize,
            "internal: a body past the instruction limit was emitted"
        );
        let narrow = !self.always_wide && self.fits_narrow(&instruction);
        let slot = self.encode(instruction, narrow);
        self.slots.push(slot);
        index
    }

    /// Whether `instruction` has an eight-byte form here: every register
    /// that goes into a slot fits a byte, a send has at most two arguments,
    /// and the table it appends to can still be indexed in sixteen bits.
    /// No `_` arm, so that a new instruction must decide.
    fn fits_narrow(&self, instruction: &Instruction) -> bool {
        let sends = self.send_sites.len() < TABLE_LIMIT;
        match instruction {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => {
                fits(*destination)
                    && match literal_form(literal) {
                        LiteralForm::Nil
                        | LiteralForm::Boolean(_)
                        | LiteralForm::SmallInteger(_) => true,
                        LiteralForm::Constant(_) => self.constants.len() < TABLE_LIMIT,
                        LiteralForm::Allocated => self.literals.len() < TABLE_LIMIT,
                    }
            }
            Instruction::LoadSelf { destination }
            | Instruction::LoadEnclosingObject { destination, .. }
            | Instruction::MakeVector { destination, .. }
            | Instruction::LoadOwnSlot { destination, .. } => fits(*destination),
            Instruction::Move {
                destination,
                source,
            }
            | Instruction::MakeFuture {
                destination,
                closure: source,
            }
            | Instruction::ResolveFuture {
                destination,
                source,
            } => fits(*destination) && fits(*source),
            Instruction::StoreLocal { source, slot } => fits(*source) && fits(*slot),
            Instruction::LoadOuterLocal {
                destination: register,
                index,
                ..
            }
            | Instruction::StoreOuterLocal {
                source: register,
                index,
                ..
            } => fits(*register) && fits(*index),
            Instruction::SendLexical {
                argument_base,
                argument_sources,
                destination,
                ..
            }
            | Instruction::SendSuper {
                argument_base,
                argument_sources,
                destination,
                ..
            } => {
                sends
                    && fits(*destination)
                    && fits(*argument_base)
                    && argument_sources.len() <= 2
                    && argument_sources.iter().all(|register| fits(*register))
            }
            Instruction::SendPublic {
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                ..
            }
            | Instruction::SendInlinable {
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                ..
            } => {
                sends
                    && fits(*destination)
                    && fits(*receiver_register)
                    && fits(*argument_base)
                    && argument_sources.len() <= 2
                    && argument_sources.iter().all(|register| fits(*register))
            }
            Instruction::SendMirror {
                mirror_register,
                subject_register,
                destination,
                ..
            } => {
                self.mirror_sites.len() < TABLE_LIMIT
                    && fits(*mirror_register)
                    && fits(*subject_register)
                    && fits(*destination)
            }
            Instruction::MakeClosure { destination, .. } => {
                self.blocks.len() < TABLE_LIMIT && fits(*destination)
            }
            Instruction::MakeArray { destination, .. } => {
                self.register_lists.len() < TABLE_LIMIT && fits(*destination)
            }
            Instruction::InitializeAppliedLevel {
                argument_base,
                destination,
                ..
            } => {
                self.register_lists.len() < TABLE_LIMIT
                    && fits(*destination)
                    && fits(*argument_base)
            }
            Instruction::InitializeInstance {
                instance,
                destination,
            } => fits(*instance) && fits(*destination),
            Instruction::InitializeSuperclass { destination, .. } => {
                self.superclass_sites.len() < TABLE_LIMIT && fits(*destination)
            }
            Instruction::AllocateInstance { destination } => fits(*destination),
            Instruction::CheckAppliedClause { .. } => true,
            Instruction::LoadVector {
                destination: register,
                vector,
                ..
            }
            | Instruction::StoreVector {
                source: register,
                vector,
                ..
            } => fits(*register) && fits(*vector),
            Instruction::InitializeSlot { source, .. }
            | Instruction::InitializeSlotProvisionally { source, .. } => fits(*source),
            Instruction::ReturnSlotIfFilled { .. } | Instruction::Jump { .. } => true,
            Instruction::Return { register }
            | Instruction::ImplicitReturn { register }
            | Instruction::NonLocalReturn { register }
            | Instruction::SignalNonBoolean { register }
            | Instruction::JumpIfNotBoolean { register, .. }
            | Instruction::BranchIfFalse { register, .. }
            | Instruction::BranchIfTrue { register, .. } => fits(*register),
            Instruction::IntegerBinary {
                destination,
                left,
                right,
                ..
            } => fits(*destination) && fits(*left) && fits(*right),
        }
    }

    /// `instruction`'s slot: its eight-byte form when `narrow`, which
    /// `fits_narrow` has decided -- so every `narrow_register` and
    /// `narrow_index` below is in range -- and its wide form otherwise.
    fn encode(&mut self, instruction: Instruction, narrow: bool) -> CompactInstruction {
        match instruction {
            Instruction::SendPublic {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => self.encode_send(
                SendKind::Public,
                selector,
                call_site,
                0,
                WideSend {
                    destination,
                    receiver: receiver_register,
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                },
                narrow,
            ),
            Instruction::SendInlinable {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => self.encode_send(
                SendKind::Inlinable,
                selector,
                call_site,
                0,
                WideSend {
                    destination,
                    receiver: receiver_register,
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                },
                narrow,
            ),
            Instruction::SendLexical {
                depth,
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => self.encode_send(
                SendKind::Lexical,
                selector,
                call_site,
                depth,
                WideSend {
                    destination,
                    receiver: 0,
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                },
                narrow,
            ),
            Instruction::SendSuper {
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => self.encode_send(
                SendKind::Super,
                selector,
                call_site,
                0,
                WideSend {
                    destination,
                    receiver: 0,
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                },
                narrow,
            ),
            other if narrow => self.encode_narrow(other),
            other => {
                let index = u32::try_from(self.wide.len())
                    .expect("internal: a body holds at most INSTRUCTION_LIMIT instructions");
                self.wide.push(other);
                CompactInstruction::Wide { index }
            }
        }
    }

    fn encode_send(
        &mut self,
        kind: SendKind,
        selector: Symbol,
        call_site: CallSite,
        depth: u16,
        registers: WideSend,
        narrow: bool,
    ) -> CompactInstruction {
        let mut site = SendSite {
            selector,
            call_site,
            depth,
            wide: None,
        };
        if !narrow {
            let index = u32::try_from(self.send_sites.len())
                .expect("internal: a body holds at most INSTRUCTION_LIMIT instructions");
            site.wide = Some(Box::new(registers));
            self.send_sites.push(site);
            return match kind {
                SendKind::Public => CompactInstruction::SendPublicWide { site: index },
                SendKind::Inlinable => CompactInstruction::SendInlinableWide { site: index },
                SendKind::Lexical => CompactInstruction::SendLexicalWide { site: index },
                SendKind::Super => CompactInstruction::SendSuperWide { site: index },
            };
        }
        let index = narrow_index(self.send_sites.len());
        self.send_sites.push(site);
        let destination = narrow_register(registers.destination);
        let receiver = narrow_register(registers.receiver);
        let argument_base = narrow_register(registers.argument_base);
        let sources: Vec<u8> = registers
            .arguments
            .iter()
            .map(|register| narrow_register(*register))
            .collect();
        match (kind, &sources[..]) {
            (SendKind::Public, []) => CompactInstruction::SendPublic0 {
                destination,
                receiver,
                argument_base,
                site: index,
            },
            (SendKind::Public, [source]) => CompactInstruction::SendPublic1 {
                destination,
                receiver,
                argument_base,
                source: *source,
                site: index,
            },
            (SendKind::Public, [first, second]) => CompactInstruction::SendPublic2 {
                destination,
                receiver,
                argument_base,
                sources: [*first, *second],
                site: index,
            },
            (SendKind::Inlinable, []) => CompactInstruction::SendInlinable0 {
                destination,
                receiver,
                argument_base,
                site: index,
            },
            (SendKind::Inlinable, [source]) => CompactInstruction::SendInlinable1 {
                destination,
                receiver,
                argument_base,
                source: *source,
                site: index,
            },
            (SendKind::Inlinable, [first, second]) => CompactInstruction::SendInlinable2 {
                destination,
                receiver,
                argument_base,
                sources: [*first, *second],
                site: index,
            },
            (SendKind::Lexical, []) => CompactInstruction::SendLexical0 {
                destination,
                argument_base,
                site: index,
            },
            (SendKind::Lexical, [source]) => CompactInstruction::SendLexical1 {
                destination,
                argument_base,
                source: *source,
                site: index,
            },
            (SendKind::Lexical, [first, second]) => CompactInstruction::SendLexical2 {
                destination,
                argument_base,
                sources: [*first, *second],
                site: index,
            },
            (SendKind::Super, []) => CompactInstruction::SendSuper0 {
                destination,
                argument_base,
                site: index,
            },
            (SendKind::Super, [source]) => CompactInstruction::SendSuper1 {
                destination,
                argument_base,
                source: *source,
                site: index,
            },
            (SendKind::Super, [first, second]) => CompactInstruction::SendSuper2 {
                destination,
                argument_base,
                sources: [*first, *second],
                site: index,
            },
            _ => unreachable!("internal: fits_narrow admits a send of at most two arguments"),
        }
    }

    /// Every instruction but a send, in its eight-byte form.
    fn encode_narrow(&mut self, instruction: Instruction) -> CompactInstruction {
        match instruction {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => {
                let destination = narrow_register(destination);
                match literal_form(&literal) {
                    LiteralForm::Nil => CompactInstruction::LoadNil { destination },
                    LiteralForm::Boolean(true) => CompactInstruction::LoadTrue { destination },
                    LiteralForm::Boolean(false) => CompactInstruction::LoadFalse { destination },
                    LiteralForm::SmallInteger(value) => {
                        CompactInstruction::LoadSmallInteger { destination, value }
                    }
                    LiteralForm::Constant(immediate) => {
                        let constant = narrow_index(self.constants.len());
                        self.constants.push(immediate);
                        CompactInstruction::LoadConstant {
                            destination,
                            constant,
                        }
                    }
                    LiteralForm::Allocated => {
                        let index = narrow_index(self.literals.len());
                        self.literals.push(literal);
                        CompactInstruction::LoadLiteral {
                            destination,
                            literal: index,
                        }
                    }
                }
            }
            Instruction::LoadSelf { destination } => CompactInstruction::LoadSelf {
                destination: narrow_register(destination),
            },
            Instruction::LoadEnclosingObject { destination, depth } => {
                CompactInstruction::LoadEnclosingObject {
                    destination: narrow_register(destination),
                    depth,
                }
            }
            Instruction::Move {
                destination,
                source,
            } => CompactInstruction::Move {
                destination: narrow_register(destination),
                source: narrow_register(source),
            },
            Instruction::StoreLocal { source, slot } => CompactInstruction::StoreLocal {
                source: narrow_register(source),
                slot: narrow_register(slot),
            },
            Instruction::LoadOuterLocal {
                destination,
                depth,
                index,
            } => CompactInstruction::LoadOuterLocal {
                destination: narrow_register(destination),
                index: narrow_register(index),
                depth,
            },
            Instruction::StoreOuterLocal {
                source,
                depth,
                index,
            } => CompactInstruction::StoreOuterLocal {
                source: narrow_register(source),
                index: narrow_register(index),
                depth,
            },
            Instruction::SendMirror {
                primitive,
                mirror_register,
                subject_register,
                argument_base,
                argument_sources,
                destination,
                failure,
            } => {
                let site = narrow_index(self.mirror_sites.len());
                self.mirror_sites.push(MirrorSite {
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                    failure,
                });
                CompactInstruction::SendMirror {
                    destination: narrow_register(destination),
                    mirror: narrow_register(mirror_register),
                    subject: narrow_register(subject_register),
                    primitive,
                    site,
                }
            }
            Instruction::MakeClosure {
                destination,
                parameters,
                code,
                copied,
            } => {
                let block = narrow_index(self.blocks.len());
                self.blocks.push(BlockSite {
                    code,
                    parameters,
                    copied: copied.into_boxed_slice(),
                });
                CompactInstruction::MakeClosure {
                    destination: narrow_register(destination),
                    block,
                }
            }
            Instruction::MakeArray {
                destination,
                element_registers,
            } => CompactInstruction::MakeArray {
                destination: narrow_register(destination),
                elements: self.push_register_list(element_registers),
            },
            Instruction::MakeVector { destination, size } => CompactInstruction::MakeVector {
                destination: narrow_register(destination),
                size,
            },
            Instruction::LoadVector {
                destination,
                vector,
                index,
            } => CompactInstruction::LoadVector {
                destination: narrow_register(destination),
                vector: narrow_register(vector),
                index,
            },
            Instruction::StoreVector {
                vector,
                index,
                source,
            } => CompactInstruction::StoreVector {
                vector: narrow_register(vector),
                source: narrow_register(source),
                index,
            },
            Instruction::InitializeSlot { source, own_index } => {
                CompactInstruction::InitializeSlot {
                    source: narrow_register(source),
                    own_index,
                }
            }
            Instruction::ReturnSlotIfFilled { own_index } => {
                CompactInstruction::ReturnSlotIfFilled { own_index }
            }
            Instruction::MakeFuture {
                destination,
                closure,
            } => CompactInstruction::MakeFuture {
                destination: narrow_register(destination),
                closure: narrow_register(closure),
            },
            Instruction::ResolveFuture {
                destination,
                source,
            } => CompactInstruction::ResolveFuture {
                destination: narrow_register(destination),
                source: narrow_register(source),
            },
            Instruction::LoadOwnSlot {
                destination,
                own_index,
            } => CompactInstruction::LoadOwnSlot {
                destination: narrow_register(destination),
                own_index,
            },
            Instruction::InitializeSlotProvisionally { source, own_index } => {
                CompactInstruction::InitializeSlotProvisionally {
                    source: narrow_register(source),
                    own_index,
                }
            }
            Instruction::Return { register } => CompactInstruction::Return {
                register: narrow_register(register),
            },
            Instruction::ImplicitReturn { register } => CompactInstruction::ImplicitReturn {
                register: narrow_register(register),
            },
            Instruction::NonLocalReturn { register } => CompactInstruction::NonLocalReturn {
                register: narrow_register(register),
            },
            Instruction::SignalNonBoolean { register } => CompactInstruction::SignalNonBoolean {
                register: narrow_register(register),
            },
            Instruction::Jump { target } => CompactInstruction::Jump { target },
            Instruction::JumpIfNotBoolean { register, target } => {
                CompactInstruction::JumpIfNotBoolean {
                    register: narrow_register(register),
                    target,
                }
            }
            Instruction::BranchIfFalse { register, target } => CompactInstruction::BranchIfFalse {
                register: narrow_register(register),
                target,
            },
            Instruction::BranchIfTrue { register, target } => CompactInstruction::BranchIfTrue {
                register: narrow_register(register),
                target,
            },
            Instruction::IntegerBinary {
                operation,
                destination,
                left,
                right,
                target,
            } => {
                let destination = narrow_register(destination);
                let left = narrow_register(left);
                let right = narrow_register(right);
                match operation {
                    IntegerOperation::Add => CompactInstruction::IntegerAdd {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::Subtract => CompactInstruction::IntegerSubtract {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::Multiply => CompactInstruction::IntegerMultiply {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::FloorDivide => CompactInstruction::IntegerFloorDivide {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::Less => CompactInstruction::IntegerLess {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::Greater => CompactInstruction::IntegerGreater {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::LessOrEqual => CompactInstruction::IntegerLessOrEqual {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::GreaterOrEqual => CompactInstruction::IntegerGreaterOrEqual {
                        destination,
                        left,
                        right,
                        target,
                    },
                    IntegerOperation::Equal => CompactInstruction::IntegerEqual {
                        destination,
                        left,
                        right,
                        target,
                    },
                }
            }
            Instruction::AllocateInstance { destination } => CompactInstruction::AllocateInstance {
                destination: narrow_register(destination),
            },
            Instruction::InitializeInstance {
                instance,
                destination,
            } => CompactInstruction::InitializeInstance {
                destination: narrow_register(destination),
                instance: narrow_register(instance),
            },
            Instruction::CheckAppliedClause { steps, selector } => {
                CompactInstruction::CheckAppliedClause { steps, selector }
            }
            Instruction::InitializeSuperclass {
                steps,
                selector,
                argument_base,
                argument_sources,
                destination,
            } => {
                let site = narrow_index(self.superclass_sites.len());
                self.superclass_sites.push(SuperclassSite {
                    steps,
                    selector,
                    argument_base,
                    arguments: argument_sources.into_boxed_slice(),
                });
                CompactInstruction::InitializeSuperclass {
                    destination: narrow_register(destination),
                    site,
                }
            }
            Instruction::InitializeAppliedLevel {
                steps,
                argument_base,
                argument_sources,
                destination,
            } => CompactInstruction::InitializeAppliedLevel {
                destination: narrow_register(destination),
                argument_base: narrow_register(argument_base),
                steps,
                arguments: self.push_register_list(argument_sources),
            },
            Instruction::SendPublic { .. }
            | Instruction::SendInlinable { .. }
            | Instruction::SendLexical { .. }
            | Instruction::SendSuper { .. } => {
                unreachable!("internal: a send is encoded by encode_send")
            }
        }
    }

    /// Appends `registers` to `register_lists` and answers its index, which
    /// `fits_narrow` has bounded to sixteen bits.
    fn push_register_list(&mut self, registers: Vec<u16>) -> u16 {
        let index = narrow_index(self.register_lists.len());
        self.register_lists.push(registers.into_boxed_slice());
        index
    }

    /// Sets the target of a jump, of an inlined integer operation's
    /// fallback, or of a mirror send's failure block, in whichever form it
    /// was encoded.
    pub fn patch_target(&mut self, index: usize, target: u32) {
        match &mut self.slots[index] {
            CompactInstruction::Jump {
                target: jump_target,
            }
            | CompactInstruction::JumpIfNotBoolean {
                target: jump_target,
                ..
            }
            | CompactInstruction::BranchIfFalse {
                target: jump_target,
                ..
            }
            | CompactInstruction::BranchIfTrue {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerAdd {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerSubtract {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerMultiply {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerFloorDivide {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerLess {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerGreater {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerLessOrEqual {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerGreaterOrEqual {
                target: jump_target,
                ..
            }
            | CompactInstruction::IntegerEqual {
                target: jump_target,
                ..
            } => *jump_target = target,
            CompactInstruction::SendMirror { site, .. } => {
                self.mirror_sites[usize::from(*site)].failure = target;
            }
            CompactInstruction::Wide { index } => self.wide[*index as usize].set_target(target),
            other => unreachable!("internal: patch_target on a non-jump slot {other:?}"),
        }
    }

    /// The instruction emitted into slot `index`, exactly as it was emitted.
    pub fn instruction(&self, index: usize) -> Instruction {
        self.tables().decode(self.slots[index])
    }

    /// Every instruction emitted so far, exactly as it was emitted.
    pub fn instructions(&self) -> Vec<Instruction> {
        let tables = self.tables();
        self.slots.iter().map(|slot| tables.decode(*slot)).collect()
    }

    /// `bytecode::family_size_of` over what has been emitted, read off the
    /// tables without decoding a slot.
    pub(crate) fn family_size(&self) -> u32 {
        self.tables().family_size()
    }

    /// `bytecode::outer_reach_of` over what has been emitted, read off the
    /// slots and tables without decoding them.
    pub(crate) fn outer_reach(&self) -> u16 {
        self.tables().outer_reach(&self.slots)
    }

    /// `bytecode::addresses_own_slots_of` over what has been emitted, read
    /// off the slots and the wide table without decoding them.
    pub(crate) fn addresses_own_slots(&self) -> bool {
        self.tables().addresses_own_slots(&self.slots)
    }

    fn tables(&self) -> Tables<'_> {
        Tables {
            constants: &self.constants,
            literals: &self.literals,
            send_sites: &self.send_sites,
            blocks: &self.blocks,
            mirror_sites: &self.mirror_sites,
            superclass_sites: &self.superclass_sites,
            register_lists: &self.register_lists,
            wide: &self.wide,
        }
    }

    pub fn finish(self) -> EncodedBody {
        EncodedBody {
            slots: self.slots.into_iter().map(Cell::new).collect(),
            originals: RefCell::default(),
            constants: self.constants.into_boxed_slice(),
            literals: self.literals.into_boxed_slice(),
            send_sites: self.send_sites.into_boxed_slice(),
            blocks: self.blocks.into_boxed_slice(),
            mirror_sites: self.mirror_sites.into_boxed_slice(),
            superclass_sites: self.superclass_sites.into_boxed_slice(),
            register_lists: self.register_lists.into_boxed_slice(),
            wide: self.wide.into_boxed_slice(),
        }
    }
}

impl EncodedBody {
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The address of the body's instruction slots, which names the body
    /// for as long as it lives: every body has at least one instruction, so
    /// the allocation is its own, and moving the `CompiledCode` -- into the
    /// `Rc` a `MakeClosure` or a mixin holds it by -- does not move it. A
    /// clone has slots of its own, and so another identity. The debugger's
    /// metadata collector keys a body's tables by it
    /// (`debugger::metadata::MetadataCollector`).
    pub(crate) fn identity(&self) -> usize {
        debug_assert!(
            !self.slots.is_empty(),
            "internal: an empty body has no allocation of its own to name it"
        );
        self.slots.as_ptr().addr()
    }

    /// The slot the run loop fetches: `Breakpoint` where one is set. Every
    /// other reader asks for `original`.
    pub fn slot(&self, index: usize) -> CompactInstruction {
        self.slots[index].get()
    }

    /// The instruction slot `index` holds without its breakpoint: what was
    /// emitted there, the slot itself when it is not patched.
    pub(crate) fn original(&self, index: usize) -> CompactInstruction {
        match self.slots[index].get() {
            CompactInstruction::Breakpoint => self.patched_original(index),
            slot => slot,
        }
    }

    #[cold]
    #[inline(never)]
    fn patched_original(&self, index: usize) -> CompactInstruction {
        self.originals
            .borrow()
            .iter()
            .find_map(|(patched, original)| (*patched == index).then_some(*original))
            .expect("internal: a patched slot keeps its original")
    }

    /// Replaces slot `index` by `Breakpoint`, keeping the instruction it
    /// held; a slot already patched keeps its first original.
    pub(crate) fn patch(&self, index: usize) {
        let slot = &self.slots[index];
        let original = slot.get();
        if matches!(original, CompactInstruction::Breakpoint) {
            return;
        }
        let mut originals = self.originals.borrow_mut();
        let mut patched = std::mem::take(&mut *originals).into_vec();
        patched.push((index, original));
        *originals = patched.into_boxed_slice();
        slot.set(CompactInstruction::Breakpoint);
    }

    /// Gives slot `index` back its original; an unpatched slot stays as it
    /// is.
    pub(crate) fn restore(&self, index: usize) {
        if !self.is_patched(index) {
            return;
        }
        let original = self.patched_original(index);
        let mut originals = self.originals.borrow_mut();
        let mut patched = std::mem::take(&mut *originals).into_vec();
        patched.retain(|(patched, _)| *patched != index);
        *originals = patched.into_boxed_slice();
        self.slots[index].set(original);
    }

    /// Whether a breakpoint replaces slot `index`.
    pub(crate) fn is_patched(&self, index: usize) -> bool {
        matches!(self.slots[index].get(), CompactInstruction::Breakpoint)
    }

    /// Whether a breakpoint replaces any slot.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the breakpoints, which patch a body, are not built yet"
        )
    )]
    pub(crate) fn has_patches(&self) -> bool {
        !self.originals.borrow().is_empty()
    }

    /// The word a `LoadConstant` names.
    #[inline(always)]
    pub fn constant(&self, index: usize) -> Immediate {
        self.constants[index]
    }

    /// The literal a `LoadLiteral` slot names.
    #[inline(always)]
    pub fn literal(&self, index: usize) -> &Literal {
        &self.literals[index]
    }

    /// The site a send names, in either form.
    #[inline(always)]
    pub fn send_site(&self, index: usize) -> &SendSite {
        &self.send_sites[index]
    }

    /// The block a narrow `MakeClosure` names.
    #[inline(always)]
    pub fn block(&self, index: usize) -> &BlockSite {
        &self.blocks[index]
    }

    /// The site a narrow `SendMirror` names.
    #[inline(always)]
    pub fn mirror_site(&self, index: usize) -> &MirrorSite {
        &self.mirror_sites[index]
    }

    /// The site a narrow `InitializeSuperclass` names.
    #[inline(always)]
    pub fn superclass_site(&self, index: usize) -> &SuperclassSite {
        &self.superclass_sites[index]
    }

    /// The registers a narrow `MakeArray` or `InitializeAppliedLevel`
    /// names: an array's elements, an initializer's arguments.
    #[inline(always)]
    pub fn register_list(&self, index: usize) -> &[u16] {
        &self.register_lists[index]
    }

    /// The operands of the cached send in slot `index` -- a `SendPublic`,
    /// `SendLexical` or `SendSuper`, in either form -- read off its slot
    /// and its site without decoding the instruction, which would build
    /// its argument list: what the leaves machine code calls on a send
    /// read of it (`jit::leaves`). `None` for any other instruction.
    pub(crate) fn cached_send(&self, index: usize) -> Option<CachedSend> {
        let narrow = |kind: SendKind, site: u16, receiver: u8, argument_base: u8, count: usize| {
            Some((
                kind,
                usize::from(site),
                u16::from(receiver),
                u16::from(argument_base),
                count,
            ))
        };
        let wide = |kind: SendKind, site: u32| {
            let site = site as usize;
            let registers = self.send_sites[site].wide()?;
            Some((
                kind,
                site,
                registers.receiver(),
                registers.argument_base(),
                registers.arguments().len(),
            ))
        };
        let (kind, site, receiver, argument_base, argument_count) = match self.original(index) {
            CompactInstruction::SendPublic0 {
                receiver,
                argument_base,
                site,
                ..
            } => narrow(SendKind::Public, site, receiver, argument_base, 0),
            CompactInstruction::SendPublic1 {
                receiver,
                argument_base,
                site,
                ..
            } => narrow(SendKind::Public, site, receiver, argument_base, 1),
            CompactInstruction::SendPublic2 {
                receiver,
                argument_base,
                site,
                ..
            } => narrow(SendKind::Public, site, receiver, argument_base, 2),
            CompactInstruction::SendLexical0 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Lexical, site, 0, argument_base, 0),
            CompactInstruction::SendLexical1 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Lexical, site, 0, argument_base, 1),
            CompactInstruction::SendLexical2 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Lexical, site, 0, argument_base, 2),
            CompactInstruction::SendSuper0 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Super, site, 0, argument_base, 0),
            CompactInstruction::SendSuper1 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Super, site, 0, argument_base, 1),
            CompactInstruction::SendSuper2 {
                argument_base,
                site,
                ..
            } => narrow(SendKind::Super, site, 0, argument_base, 2),
            CompactInstruction::SendPublicWide { site } => wide(SendKind::Public, site),
            CompactInstruction::SendLexicalWide { site } => wide(SendKind::Lexical, site),
            CompactInstruction::SendSuperWide { site } => wide(SendKind::Super, site),
            _ => None,
        }?;
        let site = &self.send_sites[site];
        let kind = match kind {
            SendKind::Public => CachedSendKind::Public { receiver },
            SendKind::Lexical => CachedSendKind::Lexical {
                depth: site.depth(),
            },
            SendKind::Super => CachedSendKind::Super,
            SendKind::Inlinable => return None,
        };
        Some(CachedSend {
            kind,
            selector: site.selector(),
            call_site: site.call_site(),
            argument_base,
            argument_count,
        })
    }

    /// Whether slot `index` holds a send of any kind, in either form, read
    /// without decoding it.
    pub(crate) fn is_send(&self, index: usize) -> bool {
        match self.original(index) {
            CompactInstruction::SendPublic0 { .. }
            | CompactInstruction::SendPublic1 { .. }
            | CompactInstruction::SendPublic2 { .. }
            | CompactInstruction::SendPublicWide { .. }
            | CompactInstruction::SendInlinable0 { .. }
            | CompactInstruction::SendInlinable1 { .. }
            | CompactInstruction::SendInlinable2 { .. }
            | CompactInstruction::SendInlinableWide { .. }
            | CompactInstruction::SendLexical0 { .. }
            | CompactInstruction::SendLexical1 { .. }
            | CompactInstruction::SendLexical2 { .. }
            | CompactInstruction::SendLexicalWide { .. }
            | CompactInstruction::SendSuper0 { .. }
            | CompactInstruction::SendSuper1 { .. }
            | CompactInstruction::SendSuper2 { .. }
            | CompactInstruction::SendSuperWide { .. }
            | CompactInstruction::SendMirror { .. } => true,
            CompactInstruction::Wide { index } => matches!(
                self.wide[index as usize],
                Instruction::SendPublic { .. }
                    | Instruction::SendInlinable { .. }
                    | Instruction::SendLexical { .. }
                    | Instruction::SendSuper { .. }
                    | Instruction::SendMirror { .. }
            ),
            _ => false,
        }
    }

    /// The instruction a `Wide` slot names.
    #[inline(always)]
    pub fn wide(&self, index: usize) -> &Instruction {
        &self.wide[index]
    }

    /// The instruction emitted into slot `index`, exactly as it was emitted.
    pub fn instruction(&self, index: usize) -> Instruction {
        self.tables().decode(self.original(index))
    }

    /// Every instruction of the body, exactly as it was emitted.
    pub fn instructions(&self) -> Vec<Instruction> {
        let tables = self.tables();
        (0..self.slots.len())
            .map(|index| tables.decode(self.original(index)))
            .collect()
    }

    /// Visits every send's call site and every block, in the order their
    /// instructions were emitted, whatever form each took; stops at, and
    /// answers, the first `Err` `visit` answers.
    pub fn for_each_emitted_mut<E>(
        &mut self,
        mut visit: impl FnMut(Emitted<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        for index in 0..self.slots.len() {
            match emitted_entry(self.original(index)) {
                EmittedEntry::SendSite(site) => {
                    visit(Emitted::CallSite(&mut self.send_sites[site].call_site))?;
                }
                EmittedEntry::Block(block) => {
                    visit(Emitted::Block(&mut self.blocks[block].code))?;
                }
                EmittedEntry::Wide(wide) => {
                    let instruction = &mut self.wide[wide];
                    if let Instruction::MakeClosure { code, .. } = instruction {
                        visit(Emitted::Block(code))?;
                    } else if let Some(call_site) = instruction.call_site_mut() {
                        visit(Emitted::CallSite(call_site))?;
                    }
                }
                EmittedEntry::Nothing => {}
            }
        }
        Ok(())
    }

    /// Every block the body creates, in the order its `MakeClosure`s were
    /// emitted, whatever form each took -- what the image decoder's checks
    /// on blocks read, without decoding a slot.
    pub(crate) fn created_blocks(&self) -> impl Iterator<Item = CreatedBlock<'_>> + '_ {
        (0..self.slots.len()).filter_map(move |index| match emitted_entry(self.original(index)) {
            EmittedEntry::Block(block) => {
                let site = &self.blocks[block];
                Some(CreatedBlock {
                    code: &site.code,
                    parameters: &site.parameters,
                    copied: &site.copied,
                })
            }
            EmittedEntry::Wide(wide) => match &self.wide[wide] {
                Instruction::MakeClosure {
                    parameters,
                    code,
                    copied,
                    ..
                } => Some(CreatedBlock {
                    code,
                    parameters,
                    copied,
                }),
                _ => None,
            },
            EmittedEntry::SendSite(_) | EmittedEntry::Nothing => None,
        })
    }

    /// `bytecode::family_size_of` over the body, read off its tables
    /// without decoding a slot.
    pub(crate) fn family_size(&self) -> u32 {
        self.tables().family_size()
    }

    fn tables(&self) -> Tables<'_> {
        Tables {
            constants: &self.constants,
            literals: &self.literals,
            send_sites: &self.send_sites,
            blocks: &self.blocks,
            mirror_sites: &self.mirror_sites,
            superclass_sites: &self.superclass_sites,
            register_lists: &self.register_lists,
            wide: &self.wide,
        }
    }
}

/// A block a body creates, as `EncodedBody::created_blocks` answers it:
/// borrowed from the body, read-only.
pub(crate) struct CreatedBlock<'a> {
    pub(crate) code: &'a Rc<CompiledCode>,
    pub(crate) parameters: &'a Rc<Vec<Symbol>>,
    pub(crate) copied: &'a [u16],
}

/// Where a slot keeps what `for_each_emitted_mut` visits.
enum EmittedEntry {
    SendSite(usize),
    Block(usize),
    Wide(usize),
    Nothing,
}

/// No `_` arm: a slot holding a call site or a block that were left out
/// here would go unnumbered by the image decoder, and two sends could share
/// a cache entry and run each other's targets.
fn emitted_entry(slot: CompactInstruction) -> EmittedEntry {
    match slot {
        CompactInstruction::SendPublic0 { site, .. }
        | CompactInstruction::SendPublic1 { site, .. }
        | CompactInstruction::SendPublic2 { site, .. }
        | CompactInstruction::SendInlinable0 { site, .. }
        | CompactInstruction::SendInlinable1 { site, .. }
        | CompactInstruction::SendInlinable2 { site, .. }
        | CompactInstruction::SendLexical0 { site, .. }
        | CompactInstruction::SendLexical1 { site, .. }
        | CompactInstruction::SendLexical2 { site, .. }
        | CompactInstruction::SendSuper0 { site, .. }
        | CompactInstruction::SendSuper1 { site, .. }
        | CompactInstruction::SendSuper2 { site, .. } => EmittedEntry::SendSite(usize::from(site)),
        CompactInstruction::SendPublicWide { site }
        | CompactInstruction::SendInlinableWide { site }
        | CompactInstruction::SendLexicalWide { site }
        | CompactInstruction::SendSuperWide { site } => EmittedEntry::SendSite(site as usize),
        CompactInstruction::MakeClosure { block, .. } => EmittedEntry::Block(usize::from(block)),
        CompactInstruction::Wide { index } => EmittedEntry::Wide(index as usize),
        CompactInstruction::LoadNil { .. }
        | CompactInstruction::LoadTrue { .. }
        | CompactInstruction::LoadFalse { .. }
        | CompactInstruction::LoadSmallInteger { .. }
        | CompactInstruction::LoadConstant { .. }
        | CompactInstruction::LoadLiteral { .. }
        | CompactInstruction::LoadSelf { .. }
        | CompactInstruction::LoadEnclosingObject { .. }
        | CompactInstruction::Move { .. }
        | CompactInstruction::StoreLocal { .. }
        | CompactInstruction::LoadOuterLocal { .. }
        | CompactInstruction::StoreOuterLocal { .. }
        | CompactInstruction::SendMirror { .. }
        | CompactInstruction::MakeArray { .. }
        | CompactInstruction::MakeVector { .. }
        | CompactInstruction::LoadVector { .. }
        | CompactInstruction::StoreVector { .. }
        | CompactInstruction::InitializeSlot { .. }
        | CompactInstruction::ReturnSlotIfFilled { .. }
        | CompactInstruction::MakeFuture { .. }
        | CompactInstruction::ResolveFuture { .. }
        | CompactInstruction::LoadOwnSlot { .. }
        | CompactInstruction::InitializeSlotProvisionally { .. }
        | CompactInstruction::Return { .. }
        | CompactInstruction::ImplicitReturn { .. }
        | CompactInstruction::NonLocalReturn { .. }
        | CompactInstruction::SignalNonBoolean { .. }
        | CompactInstruction::Jump { .. }
        | CompactInstruction::JumpIfNotBoolean { .. }
        | CompactInstruction::BranchIfFalse { .. }
        | CompactInstruction::BranchIfTrue { .. }
        | CompactInstruction::IntegerAdd { .. }
        | CompactInstruction::IntegerSubtract { .. }
        | CompactInstruction::IntegerMultiply { .. }
        | CompactInstruction::IntegerFloorDivide { .. }
        | CompactInstruction::IntegerLess { .. }
        | CompactInstruction::IntegerGreater { .. }
        | CompactInstruction::IntegerLessOrEqual { .. }
        | CompactInstruction::IntegerGreaterOrEqual { .. }
        | CompactInstruction::IntegerEqual { .. }
        | CompactInstruction::AllocateInstance { .. }
        | CompactInstruction::InitializeInstance { .. }
        | CompactInstruction::CheckAppliedClause { .. }
        | CompactInstruction::InitializeSuperclass { .. }
        | CompactInstruction::InitializeAppliedLevel { .. } => EmittedEntry::Nothing,
        CompactInstruction::Breakpoint => {
            debug_assert!(
                false,
                "internal: a reader of the originals was handed a breakpoint"
            );
            EmittedEntry::Nothing
        }
    }
}

/// How many frames out a narrow slot reads or writes registers by
/// reference: `outer_reach_of`'s list, in the eight-byte forms. No `_`
/// arm, so that a new form must decide: one that reached out unnamed would
/// leave its creating activation unmarked, and every read through it would
/// answer `nil` at exit status 0 (`docs/design/execution.md`,
/// "Activations"). A `MakeClosure`'s reach is its block's, counted from
/// `blocks`; a `Wide`'s is counted from `wide`.
fn narrow_outer_reach(slot: CompactInstruction) -> u16 {
    match slot {
        CompactInstruction::LoadOuterLocal { depth, .. }
        | CompactInstruction::StoreOuterLocal { depth, .. } => depth,
        CompactInstruction::LoadNil { .. }
        | CompactInstruction::LoadTrue { .. }
        | CompactInstruction::LoadFalse { .. }
        | CompactInstruction::LoadSmallInteger { .. }
        | CompactInstruction::LoadConstant { .. }
        | CompactInstruction::LoadLiteral { .. }
        | CompactInstruction::LoadSelf { .. }
        | CompactInstruction::LoadEnclosingObject { .. }
        | CompactInstruction::Move { .. }
        | CompactInstruction::StoreLocal { .. }
        | CompactInstruction::SendPublic0 { .. }
        | CompactInstruction::SendPublic1 { .. }
        | CompactInstruction::SendPublic2 { .. }
        | CompactInstruction::SendPublicWide { .. }
        | CompactInstruction::SendInlinable0 { .. }
        | CompactInstruction::SendInlinable1 { .. }
        | CompactInstruction::SendInlinable2 { .. }
        | CompactInstruction::SendInlinableWide { .. }
        | CompactInstruction::SendLexical0 { .. }
        | CompactInstruction::SendLexical1 { .. }
        | CompactInstruction::SendLexical2 { .. }
        | CompactInstruction::SendLexicalWide { .. }
        | CompactInstruction::SendSuper0 { .. }
        | CompactInstruction::SendSuper1 { .. }
        | CompactInstruction::SendSuper2 { .. }
        | CompactInstruction::SendSuperWide { .. }
        | CompactInstruction::SendMirror { .. }
        | CompactInstruction::MakeClosure { .. }
        | CompactInstruction::MakeArray { .. }
        | CompactInstruction::MakeVector { .. }
        | CompactInstruction::LoadVector { .. }
        | CompactInstruction::StoreVector { .. }
        | CompactInstruction::InitializeSlot { .. }
        | CompactInstruction::ReturnSlotIfFilled { .. }
        | CompactInstruction::MakeFuture { .. }
        | CompactInstruction::ResolveFuture { .. }
        | CompactInstruction::LoadOwnSlot { .. }
        | CompactInstruction::InitializeSlotProvisionally { .. }
        | CompactInstruction::Return { .. }
        | CompactInstruction::ImplicitReturn { .. }
        | CompactInstruction::NonLocalReturn { .. }
        | CompactInstruction::SignalNonBoolean { .. }
        | CompactInstruction::Jump { .. }
        | CompactInstruction::JumpIfNotBoolean { .. }
        | CompactInstruction::BranchIfFalse { .. }
        | CompactInstruction::BranchIfTrue { .. }
        | CompactInstruction::IntegerAdd { .. }
        | CompactInstruction::IntegerSubtract { .. }
        | CompactInstruction::IntegerMultiply { .. }
        | CompactInstruction::IntegerFloorDivide { .. }
        | CompactInstruction::IntegerLess { .. }
        | CompactInstruction::IntegerGreater { .. }
        | CompactInstruction::IntegerLessOrEqual { .. }
        | CompactInstruction::IntegerGreaterOrEqual { .. }
        | CompactInstruction::IntegerEqual { .. }
        | CompactInstruction::AllocateInstance { .. }
        | CompactInstruction::InitializeInstance { .. }
        | CompactInstruction::CheckAppliedClause { .. }
        | CompactInstruction::InitializeSuperclass { .. }
        | CompactInstruction::InitializeAppliedLevel { .. }
        | CompactInstruction::Wide { .. } => 0,
        CompactInstruction::Breakpoint => {
            debug_assert!(
                false,
                "internal: a reader of the originals was handed a breakpoint"
            );
            0
        }
    }
}

/// Whether a narrow slot addresses a slot of the level it was entered at:
/// `addresses_own_slots_of`'s list, in the eight-byte forms. No `_` arm, so
/// that a new form must decide: one that addressed a slot by `own_index`
/// unnamed would run with no recorded guard and no entered level, and so
/// always raise (`docs/design/object-model.md`, "what a slot access
/// checks"). A `Wide`'s is counted from `wide`.
fn narrow_addresses_own_slots(slot: CompactInstruction) -> bool {
    match slot {
        CompactInstruction::InitializeSlot { .. }
        | CompactInstruction::ReturnSlotIfFilled { .. }
        | CompactInstruction::LoadOwnSlot { .. }
        | CompactInstruction::InitializeSlotProvisionally { .. } => true,
        CompactInstruction::LoadOuterLocal { .. }
        | CompactInstruction::StoreOuterLocal { .. }
        | CompactInstruction::LoadNil { .. }
        | CompactInstruction::LoadTrue { .. }
        | CompactInstruction::LoadFalse { .. }
        | CompactInstruction::LoadSmallInteger { .. }
        | CompactInstruction::LoadConstant { .. }
        | CompactInstruction::LoadLiteral { .. }
        | CompactInstruction::LoadSelf { .. }
        | CompactInstruction::LoadEnclosingObject { .. }
        | CompactInstruction::Move { .. }
        | CompactInstruction::StoreLocal { .. }
        | CompactInstruction::SendPublic0 { .. }
        | CompactInstruction::SendPublic1 { .. }
        | CompactInstruction::SendPublic2 { .. }
        | CompactInstruction::SendPublicWide { .. }
        | CompactInstruction::SendInlinable0 { .. }
        | CompactInstruction::SendInlinable1 { .. }
        | CompactInstruction::SendInlinable2 { .. }
        | CompactInstruction::SendInlinableWide { .. }
        | CompactInstruction::SendLexical0 { .. }
        | CompactInstruction::SendLexical1 { .. }
        | CompactInstruction::SendLexical2 { .. }
        | CompactInstruction::SendLexicalWide { .. }
        | CompactInstruction::SendSuper0 { .. }
        | CompactInstruction::SendSuper1 { .. }
        | CompactInstruction::SendSuper2 { .. }
        | CompactInstruction::SendSuperWide { .. }
        | CompactInstruction::SendMirror { .. }
        | CompactInstruction::MakeClosure { .. }
        | CompactInstruction::MakeArray { .. }
        | CompactInstruction::MakeVector { .. }
        | CompactInstruction::LoadVector { .. }
        | CompactInstruction::StoreVector { .. }
        | CompactInstruction::MakeFuture { .. }
        | CompactInstruction::ResolveFuture { .. }
        | CompactInstruction::Return { .. }
        | CompactInstruction::ImplicitReturn { .. }
        | CompactInstruction::NonLocalReturn { .. }
        | CompactInstruction::SignalNonBoolean { .. }
        | CompactInstruction::Jump { .. }
        | CompactInstruction::JumpIfNotBoolean { .. }
        | CompactInstruction::BranchIfFalse { .. }
        | CompactInstruction::BranchIfTrue { .. }
        | CompactInstruction::IntegerAdd { .. }
        | CompactInstruction::IntegerSubtract { .. }
        | CompactInstruction::IntegerMultiply { .. }
        | CompactInstruction::IntegerFloorDivide { .. }
        | CompactInstruction::IntegerLess { .. }
        | CompactInstruction::IntegerGreater { .. }
        | CompactInstruction::IntegerLessOrEqual { .. }
        | CompactInstruction::IntegerGreaterOrEqual { .. }
        | CompactInstruction::IntegerEqual { .. }
        | CompactInstruction::AllocateInstance { .. }
        | CompactInstruction::InitializeInstance { .. }
        | CompactInstruction::CheckAppliedClause { .. }
        | CompactInstruction::InitializeSuperclass { .. }
        | CompactInstruction::InitializeAppliedLevel { .. }
        | CompactInstruction::Wide { .. } => false,
        CompactInstruction::Breakpoint => {
            debug_assert!(
                false,
                "internal: a reader of the originals was handed a breakpoint"
            );
            false
        }
    }
}

/// The tables a slot indexes, borrowed from a builder or a finished body,
/// so that both decode with the one inverse below.
struct Tables<'a> {
    constants: &'a [Immediate],
    literals: &'a [Literal],
    send_sites: &'a [SendSite],
    blocks: &'a [BlockSite],
    mirror_sites: &'a [MirrorSite],
    superclass_sites: &'a [SuperclassSite],
    register_lists: &'a [Box<[u16]>],
    wide: &'a [Instruction],
}

impl Tables<'_> {
    /// What `bytecode::family_size_of` answers over the decoded view: a
    /// send's call site is in `send_sites` whatever its form, a narrow
    /// block in `blocks`, and every other wide instruction in `wide`. The
    /// image decoder asks it of every body it loads, so it decodes nothing
    /// (`docs/design/memory.md`, after "The image decoder never collects").
    fn family_size(&self) -> u32 {
        let sends = self
            .send_sites
            .iter()
            .filter(|site| site.call_site != CallSite::UNCACHED)
            .map(|site| site.call_site.number + 1);
        let blocks = self.blocks.iter().map(|site| site.code.family_size);
        sends
            .chain(blocks)
            .chain(std::iter::once(crate::bytecode::family_size_of(self.wide)))
            .max()
            .unwrap_or(0)
    }

    /// What `bytecode::outer_reach_of` answers over the decoded view: the
    /// narrow forms that reach another frame's registers are named in
    /// `narrow_outer_reach`, a narrow block's reach is in `blocks`, and
    /// every wide instruction is in `wide`, where `outer_reach_of` itself
    /// names them.
    fn outer_reach(&self, slots: &[CompactInstruction]) -> u16 {
        let narrow = slots.iter().map(|slot| narrow_outer_reach(*slot));
        let blocks = self
            .blocks
            .iter()
            .map(|site| site.code.outer_reach.saturating_sub(1));
        narrow
            .chain(blocks)
            .chain(std::iter::once(crate::bytecode::outer_reach_of(self.wide)))
            .max()
            .unwrap_or(0)
    }

    /// What `bytecode::addresses_own_slots_of` answers over the decoded view:
    /// the narrow forms are named in `narrow_addresses_own_slots`, and every
    /// wide instruction is in `wide`, where `addresses_own_slots_of` itself
    /// names them.
    fn addresses_own_slots(&self, slots: &[CompactInstruction]) -> bool {
        slots.iter().any(|slot| narrow_addresses_own_slots(*slot))
            || crate::bytecode::addresses_own_slots_of(self.wide)
    }

    /// The exact inverse of `CodeBuilder::encode`.
    fn decode(&self, slot: CompactInstruction) -> Instruction {
        let register = u16::from;
        let load = |destination: u8, literal: Literal| Instruction::LoadLiteral {
            destination: register(destination),
            literal,
        };
        let integer = |operation, destination, left, right, target| Instruction::IntegerBinary {
            operation,
            destination: register(destination),
            left: register(left),
            right: register(right),
            target,
        };
        match slot {
            CompactInstruction::LoadNil { destination } => load(destination, Literal::Nil),
            CompactInstruction::LoadTrue { destination } => {
                load(destination, Literal::Boolean(true))
            }
            CompactInstruction::LoadFalse { destination } => {
                load(destination, Literal::Boolean(false))
            }
            CompactInstruction::LoadSmallInteger { destination, value } => {
                load(destination, Literal::SmallInteger(i64::from(value)))
            }
            CompactInstruction::LoadConstant {
                destination,
                constant,
            } => load(
                destination,
                // `Nil` and `Boolean` are never encoded here; they decode
                // all the same, so that the decoding stays total.
                match self.constants[usize::from(constant)].kind() {
                    ImmediateKind::Nil => Literal::Nil,
                    ImmediateKind::Boolean(value) => Literal::Boolean(value),
                    ImmediateKind::SmallInteger(value) => Literal::SmallInteger(value),
                    ImmediateKind::SmallFloat(value) => Literal::Float(value),
                },
            ),
            CompactInstruction::LoadLiteral {
                destination,
                literal,
            } => load(destination, self.literals[usize::from(literal)].clone()),
            CompactInstruction::LoadSelf { destination } => Instruction::LoadSelf {
                destination: register(destination),
            },
            CompactInstruction::LoadEnclosingObject { destination, depth } => {
                Instruction::LoadEnclosingObject {
                    destination: register(destination),
                    depth,
                }
            }
            CompactInstruction::Move {
                destination,
                source,
            } => Instruction::Move {
                destination: register(destination),
                source: register(source),
            },
            CompactInstruction::StoreLocal { source, slot } => Instruction::StoreLocal {
                source: register(source),
                slot: register(slot),
            },
            CompactInstruction::LoadOuterLocal {
                destination,
                index,
                depth,
            } => Instruction::LoadOuterLocal {
                destination: register(destination),
                depth,
                index: register(index),
            },
            CompactInstruction::StoreOuterLocal {
                source,
                index,
                depth,
            } => Instruction::StoreOuterLocal {
                source: register(source),
                depth,
                index: register(index),
            },
            CompactInstruction::SendPublic0 {
                destination,
                receiver,
                argument_base,
                site,
            } => self.send(
                SendKind::Public,
                site,
                destination,
                receiver,
                argument_base,
                &[],
            ),
            CompactInstruction::SendPublic1 {
                destination,
                receiver,
                argument_base,
                source,
                site,
            } => self.send(
                SendKind::Public,
                site,
                destination,
                receiver,
                argument_base,
                &[source],
            ),
            CompactInstruction::SendPublic2 {
                destination,
                receiver,
                argument_base,
                sources,
                site,
            } => self.send(
                SendKind::Public,
                site,
                destination,
                receiver,
                argument_base,
                &sources,
            ),
            CompactInstruction::SendPublicWide { site } => self.wide_send(SendKind::Public, site),
            CompactInstruction::SendInlinable0 {
                destination,
                receiver,
                argument_base,
                site,
            } => self.send(
                SendKind::Inlinable,
                site,
                destination,
                receiver,
                argument_base,
                &[],
            ),
            CompactInstruction::SendInlinable1 {
                destination,
                receiver,
                argument_base,
                source,
                site,
            } => self.send(
                SendKind::Inlinable,
                site,
                destination,
                receiver,
                argument_base,
                &[source],
            ),
            CompactInstruction::SendInlinable2 {
                destination,
                receiver,
                argument_base,
                sources,
                site,
            } => self.send(
                SendKind::Inlinable,
                site,
                destination,
                receiver,
                argument_base,
                &sources,
            ),
            CompactInstruction::SendInlinableWide { site } => {
                self.wide_send(SendKind::Inlinable, site)
            }
            CompactInstruction::SendLexical0 {
                destination,
                argument_base,
                site,
            } => self.send(SendKind::Lexical, site, destination, 0, argument_base, &[]),
            CompactInstruction::SendLexical1 {
                destination,
                argument_base,
                source,
                site,
            } => self.send(
                SendKind::Lexical,
                site,
                destination,
                0,
                argument_base,
                &[source],
            ),
            CompactInstruction::SendLexical2 {
                destination,
                argument_base,
                sources,
                site,
            } => self.send(
                SendKind::Lexical,
                site,
                destination,
                0,
                argument_base,
                &sources,
            ),
            CompactInstruction::SendLexicalWide { site } => self.wide_send(SendKind::Lexical, site),
            CompactInstruction::SendSuper0 {
                destination,
                argument_base,
                site,
            } => self.send(SendKind::Super, site, destination, 0, argument_base, &[]),
            CompactInstruction::SendSuper1 {
                destination,
                argument_base,
                source,
                site,
            } => self.send(
                SendKind::Super,
                site,
                destination,
                0,
                argument_base,
                &[source],
            ),
            CompactInstruction::SendSuper2 {
                destination,
                argument_base,
                sources,
                site,
            } => self.send(
                SendKind::Super,
                site,
                destination,
                0,
                argument_base,
                &sources,
            ),
            CompactInstruction::SendSuperWide { site } => self.wide_send(SendKind::Super, site),
            CompactInstruction::SendMirror {
                destination,
                mirror,
                subject,
                primitive,
                site,
            } => {
                let site = &self.mirror_sites[usize::from(site)];
                Instruction::SendMirror {
                    primitive,
                    mirror_register: register(mirror),
                    subject_register: register(subject),
                    argument_base: site.argument_base,
                    argument_sources: site.arguments.to_vec(),
                    destination: register(destination),
                    failure: site.failure,
                }
            }
            CompactInstruction::MakeClosure { destination, block } => {
                let block = &self.blocks[usize::from(block)];
                Instruction::MakeClosure {
                    destination: register(destination),
                    parameters: block.parameters.clone(),
                    code: block.code.clone(),
                    copied: block.copied.to_vec(),
                }
            }
            CompactInstruction::MakeArray {
                destination,
                elements,
            } => Instruction::MakeArray {
                destination: register(destination),
                element_registers: self.register_lists[usize::from(elements)].to_vec(),
            },
            CompactInstruction::MakeVector { destination, size } => Instruction::MakeVector {
                destination: register(destination),
                size,
            },
            CompactInstruction::LoadVector {
                destination,
                vector,
                index,
            } => Instruction::LoadVector {
                destination: register(destination),
                vector: register(vector),
                index,
            },
            CompactInstruction::StoreVector {
                vector,
                source,
                index,
            } => Instruction::StoreVector {
                vector: register(vector),
                index,
                source: register(source),
            },
            CompactInstruction::InitializeSlot { source, own_index } => {
                Instruction::InitializeSlot {
                    source: register(source),
                    own_index,
                }
            }
            CompactInstruction::ReturnSlotIfFilled { own_index } => {
                Instruction::ReturnSlotIfFilled { own_index }
            }
            CompactInstruction::MakeFuture {
                destination,
                closure,
            } => Instruction::MakeFuture {
                destination: register(destination),
                closure: register(closure),
            },
            CompactInstruction::ResolveFuture {
                destination,
                source,
            } => Instruction::ResolveFuture {
                destination: register(destination),
                source: register(source),
            },
            CompactInstruction::LoadOwnSlot {
                destination,
                own_index,
            } => Instruction::LoadOwnSlot {
                destination: register(destination),
                own_index,
            },
            CompactInstruction::InitializeSlotProvisionally { source, own_index } => {
                Instruction::InitializeSlotProvisionally {
                    source: register(source),
                    own_index,
                }
            }
            CompactInstruction::Return { register: value } => Instruction::Return {
                register: register(value),
            },
            CompactInstruction::ImplicitReturn { register: value } => Instruction::ImplicitReturn {
                register: register(value),
            },
            CompactInstruction::NonLocalReturn { register: value } => Instruction::NonLocalReturn {
                register: register(value),
            },
            CompactInstruction::SignalNonBoolean { register: value } => {
                Instruction::SignalNonBoolean {
                    register: register(value),
                }
            }
            CompactInstruction::Jump { target } => Instruction::Jump { target },
            CompactInstruction::JumpIfNotBoolean {
                register: value,
                target,
            } => Instruction::JumpIfNotBoolean {
                register: register(value),
                target,
            },
            CompactInstruction::BranchIfFalse {
                register: value,
                target,
            } => Instruction::BranchIfFalse {
                register: register(value),
                target,
            },
            CompactInstruction::BranchIfTrue {
                register: value,
                target,
            } => Instruction::BranchIfTrue {
                register: register(value),
                target,
            },
            CompactInstruction::IntegerAdd {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Add, destination, left, right, target),
            CompactInstruction::IntegerSubtract {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Subtract, destination, left, right, target),
            CompactInstruction::IntegerMultiply {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Multiply, destination, left, right, target),
            CompactInstruction::IntegerFloorDivide {
                destination,
                left,
                right,
                target,
            } => integer(
                IntegerOperation::FloorDivide,
                destination,
                left,
                right,
                target,
            ),
            CompactInstruction::IntegerLess {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Less, destination, left, right, target),
            CompactInstruction::IntegerGreater {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Greater, destination, left, right, target),
            CompactInstruction::IntegerLessOrEqual {
                destination,
                left,
                right,
                target,
            } => integer(
                IntegerOperation::LessOrEqual,
                destination,
                left,
                right,
                target,
            ),
            CompactInstruction::IntegerGreaterOrEqual {
                destination,
                left,
                right,
                target,
            } => integer(
                IntegerOperation::GreaterOrEqual,
                destination,
                left,
                right,
                target,
            ),
            CompactInstruction::IntegerEqual {
                destination,
                left,
                right,
                target,
            } => integer(IntegerOperation::Equal, destination, left, right, target),
            CompactInstruction::AllocateInstance { destination } => Instruction::AllocateInstance {
                destination: register(destination),
            },
            CompactInstruction::InitializeInstance {
                destination,
                instance,
            } => Instruction::InitializeInstance {
                instance: register(instance),
                destination: register(destination),
            },
            CompactInstruction::CheckAppliedClause { steps, selector } => {
                Instruction::CheckAppliedClause { steps, selector }
            }
            CompactInstruction::InitializeSuperclass { destination, site } => {
                let site = &self.superclass_sites[usize::from(site)];
                Instruction::InitializeSuperclass {
                    steps: site.steps,
                    selector: site.selector,
                    argument_base: site.argument_base,
                    argument_sources: site.arguments.to_vec(),
                    destination: register(destination),
                }
            }
            CompactInstruction::InitializeAppliedLevel {
                destination,
                argument_base,
                steps,
                arguments,
            } => Instruction::InitializeAppliedLevel {
                steps,
                argument_base: register(argument_base),
                argument_sources: self.register_lists[usize::from(arguments)].to_vec(),
                destination: register(destination),
            },
            CompactInstruction::Wide { index } => self.wide[index as usize].clone(),
            CompactInstruction::Breakpoint => {
                unreachable!("internal: a breakpoint is never decoded; the decoded view answers the original")
            }
        }
    }

    /// A narrow send, its registers widened back.
    fn send(
        &self,
        kind: SendKind,
        site: u16,
        destination: u8,
        receiver: u8,
        argument_base: u8,
        sources: &[u8],
    ) -> Instruction {
        assemble_send(
            kind,
            &self.send_sites[usize::from(site)],
            u16::from(destination),
            u16::from(receiver),
            u16::from(argument_base),
            sources.iter().copied().map(u16::from).collect(),
        )
    }

    /// A wide send, its registers read from its site.
    fn wide_send(&self, kind: SendKind, site: u32) -> Instruction {
        let site = &self.send_sites[site as usize];
        let registers = site
            .wide
            .as_deref()
            .expect("internal: a wide send's site holds its registers");
        assemble_send(
            kind,
            site,
            registers.destination,
            registers.receiver,
            registers.argument_base,
            registers.arguments.to_vec(),
        )
    }
}

/// The `Instruction` of `kind` from its site and its registers.
fn assemble_send(
    kind: SendKind,
    site: &SendSite,
    destination: u16,
    receiver: u16,
    argument_base: u16,
    argument_sources: Vec<u16>,
) -> Instruction {
    match kind {
        SendKind::Public => Instruction::SendPublic {
            selector: site.selector,
            receiver_register: receiver,
            argument_base,
            argument_sources,
            destination,
            call_site: site.call_site,
        },
        SendKind::Inlinable => Instruction::SendInlinable {
            selector: site.selector,
            receiver_register: receiver,
            argument_base,
            argument_sources,
            destination,
            call_site: site.call_site,
        },
        SendKind::Lexical => Instruction::SendLexical {
            depth: site.depth,
            selector: site.selector,
            argument_base,
            argument_sources,
            destination,
            call_site: site.call_site,
        },
        SendKind::Super => Instruction::SendSuper {
            selector: site.selector,
            argument_base,
            argument_sources,
            destination,
            call_site: site.call_site,
        },
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;
    use crate::bytecode::{CallSite, Instruction, IntegerOperation, Literal};
    use crate::Symbol;

    /// Whether two instruction lists are the same, a `Float` literal by its
    /// bits, so that a NaN's payload and a zero's sign must survive.
    fn same(left: &[Instruction], right: &[Instruction]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(own, other)| own.is_identical_to(other))
    }

    fn round_trip(instructions: Vec<Instruction>) {
        for mut builder in [
            CodeBuilder::never_wide_by_stress(),
            CodeBuilder::always_wide(),
        ] {
            for instruction in &instructions {
                builder.emit(instruction.clone());
            }
            assert_derived_as_decoded(&builder);
            let body = builder.finish();
            assert!(
                same(&body.instructions(), &instructions),
                "{:?}",
                body.instructions()
            );
            assert_eq!(body.len(), instructions.len());
        }
    }

    /// `family_size`, `outer_reach` and `addresses_own_slots`, read off the
    /// encoding, answer what `family_size_of`, `outer_reach_of` and
    /// `addresses_own_slots_of` answer over the decoded view.
    fn assert_derived_as_decoded(builder: &CodeBuilder) {
        let instructions = builder.instructions();
        assert_eq!(
            builder.family_size(),
            crate::bytecode::family_size_of(&instructions)
        );
        assert_eq!(
            builder.outer_reach(),
            crate::bytecode::outer_reach_of(&instructions)
        );
        assert_eq!(
            builder.addresses_own_slots(),
            crate::bytecode::addresses_own_slots_of(&instructions)
        );
    }

    /// What the decoder and the compiler both derive from a body without
    /// decoding it, in every form: sends numbered and `UNCACHED`, blocks
    /// with a family and a reach of their own, and outer locals, narrow,
    /// wide, and mixed in one body.
    #[test]
    fn the_family_size_and_outer_reach_read_off_the_encoding_are_the_decoded_ones() {
        let block = |family_size, outer_reach| {
            let mut code = crate::bytecode::CompiledCode::new(
                crate::bytecode::CodeKind::Block,
                vec![Instruction::ImplicitReturn { register: 0 }],
                0,
                1,
                0,
                vec![(0, 0..1)],
                ns_syntax::Span::new(0, 1),
            );
            code.family_size = family_size;
            code.outer_reach = outer_reach;
            std::rc::Rc::new(code)
        };
        let closure = |code, copied: &[u16]| Instruction::MakeClosure {
            destination: 1,
            parameters: std::rc::Rc::new(vec![]),
            code,
            copied: copied.to_vec(),
        };
        let numbered = |number| Instruction::SendPublic {
            selector: Symbol::intern("a"),
            receiver_register: 1,
            argument_base: 3,
            argument_sources: vec![],
            destination: 2,
            call_site: CallSite { number },
        };
        let outer = |depth, index| Instruction::LoadOuterLocal {
            destination: 1,
            depth,
            index,
        };
        let bodies = [
            vec![],
            vec![numbered(4), closure(block(9, 3), &[])],
            vec![numbered(12), closure(block(9, 3), &[300])],
            vec![closure(block(2, 6), &[300]), closure(block(7, 1), &[])],
            vec![
                Instruction::SendSuper {
                    selector: Symbol::intern("b"),
                    argument_base: 3,
                    argument_sources: vec![],
                    destination: 2,
                    call_site: CallSite::UNCACHED,
                },
                outer(2, 1),
            ],
            vec![outer(2, 1), outer(5, 256), numbered(70_000)],
            vec![
                Instruction::StoreOuterLocal {
                    source: 300,
                    depth: 4,
                    index: 1,
                },
                send("c:d:e:", 1, &[2, 3, 4], 5),
            ],
        ];
        for instructions in bodies {
            for mut builder in [
                CodeBuilder::never_wide_by_stress(),
                CodeBuilder::always_wide(),
            ] {
                for instruction in &instructions {
                    builder.emit(instruction.clone());
                }
                assert_derived_as_decoded(&builder);
                let body = builder.finish();
                assert_eq!(
                    body.family_size(),
                    crate::bytecode::family_size_of(&body.instructions())
                );
            }
        }
    }

    /// A public send whose argument slots start at r3, each copied from
    /// the register `sources` names.
    fn send(selector: &str, receiver: u16, sources: &[u16], destination: u16) -> Instruction {
        send_at(selector, receiver, 3, sources, destination)
    }

    fn send_at(
        selector: &str,
        receiver: u16,
        argument_base: u16,
        sources: &[u16],
        destination: u16,
    ) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::intern(selector),
            receiver_register: receiver,
            argument_base,
            argument_sources: sources.to_vec(),
            destination,
            call_site: CallSite { number: 3 },
        }
    }

    #[test]
    fn a_compact_instruction_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<CompactInstruction>(), 8);
    }

    #[test]
    fn every_literal_round_trips() {
        let literals = [
            Literal::Nil,
            Literal::Boolean(true),
            Literal::Boolean(false),
            Literal::SmallInteger(0),
            Literal::SmallInteger(i64::from(i32::MIN)),
            Literal::SmallInteger(i64::from(i32::MAX)),
            Literal::SmallInteger(i64::from(i32::MAX) + 1),
            Literal::SmallInteger(crate::SMALL_INTEGER_MIN),
            Literal::Float(0.0),
            Literal::Float(-0.0),
            Literal::Float(1.5),
            Literal::Float(1.0e300),
            Literal::Float(f64::MIN_POSITIVE / 2.0),
            Literal::Float(f64::NAN),
            // A NaN no arithmetic here makes: its payload must survive too.
            Literal::Float(f64::from_bits(0x7ff8_0000_0000_0001)),
            Literal::String("text".to_string()),
            Literal::Symbol(Symbol::intern("symbol")),
            Literal::LargeInteger(
                crate::ArbitraryInteger::parse("99999999999999999999", 10).expect("digits"),
            ),
            Literal::UnresolvedIdentifier("missing".to_string()),
            Literal::BodyTooLarge(crate::bytecode::BodyLimit::Registers),
            Literal::BodyTooLarge(crate::bytecode::BodyLimit::Arguments),
            Literal::BodyTooLarge(crate::bytecode::BodyLimit::Instructions),
        ];
        round_trip(
            literals
                .into_iter()
                .map(|literal| Instruction::LoadLiteral {
                    destination: 1,
                    literal,
                })
                .collect(),
        );
    }

    #[test]
    fn a_literal_takes_the_form_its_value_needs() {
        let mut builder = CodeBuilder::never_wide_by_stress();
        for literal in [
            Literal::Nil,
            Literal::Boolean(true),
            Literal::SmallInteger(7),
            Literal::SmallInteger(i64::from(i32::MAX) + 1),
            Literal::Float(1.5),
            Literal::Float(1.0e300),
        ] {
            builder.emit(Instruction::LoadLiteral {
                destination: 2,
                literal,
            });
        }
        let body = builder.finish();
        assert!(matches!(
            body.slot(0),
            CompactInstruction::LoadNil { destination: 2 }
        ));
        assert!(matches!(
            body.slot(1),
            CompactInstruction::LoadTrue { destination: 2 }
        ));
        assert!(matches!(
            body.slot(2),
            CompactInstruction::LoadSmallInteger {
                destination: 2,
                value: 7
            }
        ));
        assert!(matches!(
            body.slot(3),
            CompactInstruction::LoadConstant { destination: 2, .. }
        ));
        assert!(matches!(
            body.slot(4),
            CompactInstruction::LoadConstant { destination: 2, .. }
        ));
        assert!(matches!(
            body.slot(5),
            CompactInstruction::LoadLiteral { destination: 2, .. }
        ));
    }

    #[test]
    fn sends_of_up_to_two_arguments_are_narrow_and_wider_ones_wide() {
        let mut builder = CodeBuilder::never_wide_by_stress();
        builder.emit(send("a", 1, &[], 2));
        builder.emit(send("b:", 1, &[3], 2));
        builder.emit(send("c:d:", 1, &[3, 4], 2));
        builder.emit(send("e:f:g:", 1, &[3, 4, 5], 2));
        builder.emit(send("h:", 256, &[3], 2));
        builder.emit(send_at("i:", 1, 256, &[3], 2));
        builder.emit(send_at("j:", 1, 255, &[256], 2));
        let body = builder.finish();
        assert!(matches!(
            body.slot(0),
            CompactInstruction::SendPublic0 {
                argument_base: 3,
                ..
            }
        ));
        assert!(matches!(
            body.slot(1),
            CompactInstruction::SendPublic1 {
                argument_base: 3,
                source: 3,
                ..
            }
        ));
        assert!(matches!(
            body.slot(2),
            CompactInstruction::SendPublic2 {
                argument_base: 3,
                sources: [3, 4],
                ..
            }
        ));
        for wide in 3..=6 {
            assert!(
                matches!(body.slot(wide), CompactInstruction::SendPublicWide { .. }),
                "slot {wide} names a register past a byte or a third argument"
            );
        }
    }

    #[test]
    fn every_instruction_round_trips_narrow_and_wide() {
        let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            1,
            0,
            vec![(0, 0..1)],
            ns_syntax::Span::new(0, 1),
        ));
        let mirror =
            crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
                .expect("the mirror has string:concatenate:ifFail:");
        let lexical = |argument_base: u16, arguments: &[u16]| Instruction::SendLexical {
            depth: 2,
            selector: Symbol::intern("lexical"),
            argument_base,
            argument_sources: arguments.to_vec(),
            destination: 7,
            call_site: CallSite { number: 1 },
        };
        let superclass = |argument_base: u16, arguments: &[u16]| Instruction::SendSuper {
            selector: Symbol::intern("super"),
            argument_base,
            argument_sources: arguments.to_vec(),
            destination: 7,
            call_site: CallSite::UNCACHED,
        };
        let inlinable = |argument_base: u16, arguments: &[u16]| Instruction::SendInlinable {
            selector: Symbol::intern("ifTrue:"),
            receiver_register: 4,
            argument_base,
            argument_sources: arguments.to_vec(),
            destination: 7,
            call_site: CallSite::UNCACHED,
        };
        let mut instructions = vec![
            Instruction::LoadSelf { destination: 255 },
            Instruction::LoadEnclosingObject {
                destination: 3,
                depth: 4,
            },
            Instruction::Move {
                destination: 1,
                source: 2,
            },
            Instruction::StoreLocal { source: 1, slot: 2 },
            Instruction::LoadOuterLocal {
                destination: 1,
                depth: 2,
                index: 3,
            },
            Instruction::StoreOuterLocal {
                source: 1,
                depth: 2,
                index: 3,
            },
            Instruction::SendMirror {
                primitive: mirror,
                mirror_register: 1,
                subject_register: 2,
                argument_base: 6,
                argument_sources: vec![3, 4],
                destination: 5,
                failure: 9,
            },
            Instruction::MakeClosure {
                destination: 1,
                parameters: std::rc::Rc::new(vec![Symbol::intern("x")]),
                code: block,
                copied: vec![2, 300],
            },
            Instruction::MakeArray {
                destination: 1,
                element_registers: vec![2, 3, 400],
            },
            Instruction::MakeVector {
                destination: 1,
                size: 9,
            },
            Instruction::LoadVector {
                destination: 1,
                vector: 2,
                index: 3,
            },
            Instruction::StoreVector {
                vector: 1,
                index: 2,
                source: 3,
            },
            Instruction::InitializeSlot {
                source: 1,
                own_index: 70_000,
            },
            Instruction::ReturnSlotIfFilled { own_index: 2 },
            Instruction::MakeFuture {
                destination: 1,
                closure: 2,
            },
            Instruction::ResolveFuture {
                destination: 1,
                source: 2,
            },
            Instruction::LoadOwnSlot {
                destination: 1,
                own_index: 3,
            },
            Instruction::InitializeSlotProvisionally {
                source: 1,
                own_index: 3,
            },
            Instruction::Return { register: 1 },
            Instruction::ImplicitReturn { register: 1 },
            Instruction::NonLocalReturn { register: 1 },
            Instruction::SignalNonBoolean { register: 1 },
            Instruction::Jump { target: 2 },
            Instruction::JumpIfNotBoolean {
                register: 1,
                target: 3,
            },
            Instruction::BranchIfFalse {
                register: 1,
                target: 4,
            },
            Instruction::BranchIfTrue {
                register: 1,
                target: 5,
            },
            Instruction::Move {
                destination: 256,
                source: 1,
            },
            Instruction::LoadOuterLocal {
                destination: 1,
                depth: 2,
                index: 256,
            },
        ];
        for operation in [
            IntegerOperation::Add,
            IntegerOperation::Subtract,
            IntegerOperation::Multiply,
            IntegerOperation::FloorDivide,
            IntegerOperation::Less,
            IntegerOperation::Greater,
            IntegerOperation::LessOrEqual,
            IntegerOperation::GreaterOrEqual,
            IntegerOperation::Equal,
        ] {
            instructions.push(Instruction::IntegerBinary {
                operation,
                destination: 1,
                left: 2,
                right: 3,
                target: 6,
            });
        }
        instructions.push(Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 1,
            left: 2,
            right: 999,
            target: 6,
        });
        // Sources that are their slots and sources that are not, a base
        // and a source past a byte, and every argument count's form.
        for (argument_base, arguments) in [
            (0, &[][..]),
            (9, &[][..]),
            (9, &[1][..]),
            (9, &[9][..]),
            (9, &[1, 2][..]),
            (9, &[9, 10][..]),
            (300, &[1][..]),
            (9, &[1, 400][..]),
            (9, &[1, 2, 3][..]),
            (9, &[1, 2, 3, 4, 5, 6, 7, 8][..]),
        ] {
            instructions.push(send_at("public", 4, argument_base, arguments, 7));
            instructions.push(lexical(argument_base, arguments));
            instructions.push(superclass(argument_base, arguments));
            instructions.push(inlinable(argument_base, arguments));
        }
        round_trip(instructions);
    }

    /// One of each instantiation instruction for every argument count and
    /// every `steps` worth telling apart: none, one, two and three hundred
    /// argument registers; no step, one, and the most a `u16` holds; and a
    /// destination that fits a slot's byte and one that does not.
    fn instantiation_instructions() -> Vec<Instruction> {
        let mut instructions = Vec::new();
        for destination in [7, 300] {
            instructions.push(Instruction::AllocateInstance { destination });
            for count in [0_u16, 1, 2, 300] {
                let arguments: Vec<u16> = (0..count).map(|index| index * 2 + 1).collect();
                instructions.push(Instruction::InitializeInstance {
                    instance: 4,
                    destination,
                });
                for steps in [0, 1, u16::MAX] {
                    instructions.push(Instruction::CheckAppliedClause {
                        steps,
                        selector: Symbol::intern("with:and:"),
                    });
                    instructions.push(Instruction::InitializeSuperclass {
                        steps,
                        selector: Symbol::intern("new"),
                        argument_base: destination - 2,
                        argument_sources: arguments.clone(),
                        destination,
                    });
                    instructions.push(Instruction::InitializeAppliedLevel {
                        steps,
                        argument_base: destination - 2,
                        argument_sources: arguments.clone(),
                        destination,
                    });
                }
            }
        }
        instructions.push(Instruction::InitializeInstance {
            instance: 256,
            destination: 2,
        });
        instructions
    }

    /// The instructions of a compiled `new` read back exactly from their
    /// eight-byte forms and their wide ones, take an eight-byte form
    /// wherever their registers fit a byte, and reach no other frame's
    /// registers.
    #[test]
    fn each_instantiation_instruction_round_trips_through_its_compact_form() {
        let instructions = instantiation_instructions();
        round_trip(instructions.clone());
        for instruction in &instructions {
            assert_eq!(
                crate::bytecode::outer_reach_of(std::slice::from_ref(instruction)),
                0,
                "{instruction:?} reaches only its own frame"
            );
            assert!(
                !crate::bytecode::addresses_own_slots_of(std::slice::from_ref(instruction)),
                "{instruction:?} addresses no own slot"
            );
        }
        let mut builder = CodeBuilder::never_wide_by_stress();
        for instruction in &instructions {
            builder.emit(instruction.clone());
        }
        assert_eq!(builder.outer_reach(), 0);
        let body = builder.finish();
        for (index, instruction) in instructions.iter().enumerate() {
            let fits_a_byte = match instruction {
                Instruction::AllocateInstance { destination }
                | Instruction::InitializeSuperclass { destination, .. } => *destination <= 255,
                Instruction::InitializeAppliedLevel {
                    argument_base,
                    destination,
                    ..
                } => *argument_base <= 255 && *destination <= 255,
                Instruction::InitializeInstance {
                    instance,
                    destination,
                    ..
                } => *instance <= 255 && *destination <= 255,
                Instruction::CheckAppliedClause { .. } => true,
                other => unreachable!("not an instantiation instruction: {other:?}"),
            };
            assert_eq!(
                !matches!(body.slot(index), CompactInstruction::Wide { .. }),
                fits_a_byte,
                "{instruction:?} takes an eight-byte form exactly when its registers fit"
            );
        }
    }

    /// Every instruction a lowering emits before it knows its target, each
    /// patched in the form it took: in its slot, in a mirror site's
    /// `failure`, or in the wide table. `never_wide_by_stress` keeps the
    /// narrow half narrow under `NS_NEXT_STRESS_WIDE`.
    #[test]
    fn a_patched_target_round_trips_narrow_and_wide() {
        let primitive =
            crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
                .expect("the mirror primitive exists");
        for (mut builder, wide) in [
            (CodeBuilder::never_wide_by_stress(), false),
            (CodeBuilder::always_wide(), true),
        ] {
            let jump = builder.emit(Instruction::Jump { target: 0 });
            let integer = builder.emit(Instruction::IntegerBinary {
                operation: IntegerOperation::Less,
                destination: 1,
                left: 2,
                right: 3,
                target: 0,
            });
            let mirror = builder.emit(Instruction::SendMirror {
                primitive,
                mirror_register: 1,
                subject_register: 2,
                argument_base: 6,
                argument_sources: vec![3, 4],
                destination: 5,
                failure: 0,
            });
            let not_boolean = builder.emit(Instruction::JumpIfNotBoolean {
                register: 1,
                target: 0,
            });
            let if_false = builder.emit(Instruction::BranchIfFalse {
                register: 1,
                target: 0,
            });
            let if_true = builder.emit(Instruction::BranchIfTrue {
                register: 1,
                target: 0,
            });
            builder.patch_target(jump, 1);
            builder.patch_target(integer, 2);
            builder.patch_target(mirror, 3);
            builder.patch_target(not_boolean, 4);
            builder.patch_target(if_false, 5);
            builder.patch_target(if_true, 6);
            let body = builder.finish();
            for index in 0..body.len() {
                assert_eq!(
                    matches!(body.slot(index), CompactInstruction::Wide { .. }),
                    wide,
                    "slot {index} takes the form its builder asks for"
                );
            }
            assert!(matches!(
                body.instruction(jump),
                Instruction::Jump { target: 1 }
            ));
            assert!(matches!(
                body.instruction(integer),
                Instruction::IntegerBinary { target: 2, .. }
            ));
            assert!(matches!(
                body.instruction(mirror),
                Instruction::SendMirror { failure: 3, .. }
            ));
            assert!(matches!(
                body.instruction(not_boolean),
                Instruction::JumpIfNotBoolean {
                    register: 1,
                    target: 4
                }
            ));
            assert!(matches!(
                body.instruction(if_false),
                Instruction::BranchIfFalse {
                    register: 1,
                    target: 5
                }
            ));
            assert!(matches!(
                body.instruction(if_true),
                Instruction::BranchIfTrue {
                    register: 1,
                    target: 6
                }
            ));
        }
    }

    #[test]
    fn past_sixteen_bits_a_table_index_takes_the_wide_form() {
        let mut builder = CodeBuilder::never_wide_by_stress();
        let count = usize::from(u16::MAX) + 2;
        for index in 0..count {
            builder.emit(Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::SmallInteger(i64::from(i32::MAX) + 1 + index as i64),
            });
        }
        let body = builder.finish();
        assert!(matches!(
            body.slot(0),
            CompactInstruction::LoadConstant { .. }
        ));
        assert!(matches!(
            body.slot(count - 1),
            CompactInstruction::Wide { .. }
        ));
        assert!(matches!(
            body.instruction(count - 1),
            Instruction::LoadLiteral { literal: Literal::SmallInteger(value), .. }
                if value == i64::from(i32::MAX) + count as i64
        ));
    }

    /// The block, mirror-site, superclass-site and register-list tables each
    /// have their own limit check in `fits_narrow`; the entry after the last narrow one must
    /// take `Wide` and read back exactly. `never_wide_by_stress` keeps
    /// `NS_NEXT_STRESS_WIDE` from making every entry wide and so hiding the
    /// boundary.
    #[test]
    fn past_sixteen_bits_the_other_tables_take_the_wide_form() {
        let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            1,
            0,
            vec![(0, 0..1)],
            ns_syntax::Span::new(0, 1),
        ));
        let primitive =
            crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
                .expect("the mirror primitive exists");
        let count = usize::from(u16::MAX) + 2;
        let instructions = [
            Instruction::MakeClosure {
                destination: 1,
                parameters: std::rc::Rc::new(vec![]),
                code: block,
                copied: vec![],
            },
            Instruction::SendMirror {
                primitive,
                mirror_register: 1,
                subject_register: 2,
                argument_base: 6,
                argument_sources: vec![3, 4],
                destination: 5,
                failure: 0,
            },
            Instruction::MakeArray {
                destination: 1,
                element_registers: vec![2, 3],
            },
            // `InitializeInstance` is not here: it names no table entry.
            Instruction::InitializeSuperclass {
                steps: 1,
                selector: Symbol::intern("with:and:"),
                argument_base: 2,
                argument_sources: vec![2, 3],
                destination: 4,
            },
            Instruction::InitializeAppliedLevel {
                steps: 2,
                argument_base: 2,
                argument_sources: vec![2, 3],
                destination: 4,
            },
        ];
        for instruction in instructions {
            let mut builder = CodeBuilder::never_wide_by_stress();
            for _ in 0..count {
                builder.emit(instruction.clone());
            }
            let body = builder.finish();
            assert!(
                !matches!(body.slot(0), CompactInstruction::Wide { .. }),
                "{instruction:?} starts narrow"
            );
            assert!(
                !matches!(body.slot(count - 2), CompactInstruction::Wide { .. }),
                "{instruction:?} is narrow at the 65,536th entry"
            );
            assert!(
                matches!(body.slot(count - 1), CompactInstruction::Wide { .. }),
                "{instruction:?} is wide at the 65,537th entry"
            );
            assert_eq!(
                format!("{:?}", body.instruction(count - 1)),
                format!("{instruction:?}")
            );
        }
    }

    #[test]
    fn call_sites_and_blocks_are_visited_in_emission_order() {
        let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            1,
            0,
            vec![(0, 0..1)],
            ns_syntax::Span::new(0, 1),
        ));
        for mut builder in [CodeBuilder::new(), CodeBuilder::always_wide()] {
            builder.emit(send("a", 1, &[], 2));
            builder.emit(Instruction::MakeClosure {
                destination: 1,
                parameters: std::rc::Rc::new(vec![]),
                code: block.clone(),
                copied: vec![],
            });
            builder.emit(send("b:c:d:", 1, &[1, 2, 3], 2));
            let mut body = builder.finish();
            let mut seen = Vec::new();
            body.for_each_emitted_mut(|emitted| {
                seen.push(match emitted {
                    Emitted::CallSite(site) => {
                        site.number = 40 + seen.len() as u32;
                        "site"
                    }
                    Emitted::Block(_) => "block",
                });
                Ok::<(), ()>(())
            })
            .expect("the visit never fails");
            assert_eq!(seen, ["site", "block", "site"]);
            assert_eq!(
                body.instruction(0).call_site(),
                Some(CallSite { number: 40 })
            );
            assert_eq!(
                body.instruction(2).call_site(),
                Some(CallSite { number: 42 })
            );
        }
    }

    /// The image decoder refuses a body at its first bad block and relies
    /// on the visit ending there: nothing after the `Err` is visited, and
    /// the `Err` is what the walk answers.
    #[test]
    fn the_visit_stops_at_the_first_error_and_answers_it() {
        let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            1,
            0,
            vec![(0, 0..1)],
            ns_syntax::Span::new(0, 1),
        ));
        for mut builder in [
            CodeBuilder::never_wide_by_stress(),
            CodeBuilder::always_wide(),
        ] {
            builder.emit(send("a", 1, &[], 2));
            builder.emit(Instruction::MakeClosure {
                destination: 1,
                parameters: std::rc::Rc::new(vec![]),
                code: block.clone(),
                copied: vec![],
            });
            builder.emit(send("b", 1, &[], 2));
            builder.emit(send("c", 1, &[], 2));
            let mut body = builder.finish();
            let mut visited = 0;
            let answer = body.for_each_emitted_mut(|emitted| {
                visited += 1;
                match emitted {
                    Emitted::Block(_) => Err("the block"),
                    Emitted::CallSite(site) => {
                        site.number = 7;
                        Ok(())
                    }
                }
            });
            assert_eq!(answer, Err("the block"));
            assert_eq!(visited, 2);
            assert_eq!(
                body.instruction(0).call_site(),
                Some(CallSite { number: 7 })
            );
            assert_eq!(
                body.instruction(2).call_site(),
                Some(CallSite { number: 3 })
            );
            assert_eq!(
                body.instruction(3).call_site(),
                Some(CallSite { number: 3 })
            );
        }
    }
}
