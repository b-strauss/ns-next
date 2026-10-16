//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- see `docs/design/image-format.md`. Never a memory dump.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{Heap, ObjectShape};
use crate::interpreter::future;
use crate::interpreter::shipped_world::{ShippedEntry, ShippedWorld};
use crate::types::{
    BuiltinClass, ClassReference, FrameKind, FrameReference, ObjectReference, SourceOrigin, Value,
    ValueKind,
};
use crate::{ArbitraryInteger, Mutator};

use super::opcodes::*;
use super::shipped_names::{NamePosition, ShippedKind, ShippedName};
use super::{mixins, symbols};

pub const MAGIC: &[u8; 4] = b"NSNX";
/// The format version, refused on load when it does not match and never
/// migrated. It is not bumped when the layout or the compiler's codegen
/// changes: the project has no external users, so an image written before
/// a change is disposable and no migration path is owed to it
/// (`docs/design/image-format.md`, "Layout"). What has to hold instead is
/// that an old or corrupt image fails as a `DecodeError`, which comes from
/// validating every id, tag, count and length before it indexes, allocates
/// or slices, and from never renumbering or reusing a wire tag
/// (`opcodes.rs`) -- not from this field.
///
/// The header's flag byte (`FLAG_HAS_SOURCE`) is what carries variation
/// *within* the format: the modules' source text is optional, and a reader
/// distinguishes the two variants by the flag. A flag bit this build does
/// not know is refused (`DecodeError::UnknownHeaderFlags`), which is what
/// keeps an optional section from being silently skipped by a reader that
/// cannot see it.
///
/// The flag byte is followed by the 64-bit fingerprint of the shipped world
/// the writing binary carries (`fingerprint.rs`); a binary with another
/// refuses the image (`DecodeError::ShippedWorldMismatch`).
pub const FORMAT_VERSION: u32 = 1;

#[derive(Default)]
pub struct Writer {
    buffer: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buffer: Vec::new() }
    }

    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn write_u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    pub fn write_boolean(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub fn write_u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    /// `u32` byte-count prefix, then the raw bytes. A payload too long for
    /// the prefix would write a length that does not describe what follows
    /// -- a self-inconsistent image no reader could detect -- so it aborts
    /// rather than truncating silently.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(u32_field("byte string length", bytes.len()));
        self.write_raw(bytes);
    }

    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }
}

/// Header flag bits. Bit 0 says every module in the module section carries
/// its source text (`modules::encode_modules`); every
/// bit this build has not assigned a meaning to is reserved and must be
/// zero, so a reader of this build refuses an image using a flag it does
/// not implement rather than skipping a section it cannot see.
pub const FLAG_HAS_SOURCE: u8 = 1;

/// The image is an application packaged just before `main:args:`: `load`
/// starts it by sending `main: platform args:` to the graph's one root
/// (`docs/design/image-format.md`).
pub const FLAG_START: u8 = 4;

/// Every bit this build assigns a meaning to.
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_START;

/// Header flag bits that no longer mean anything and are never reused: 2
/// was `FLAG_STATELESS`, the stateless binary, retired 2026-09-29 when
/// `build` began packaging the application before `main:args:`. A stale
/// binary's header meets an unknown flag rather than whatever took the bit.
pub const RETIRED_HEADER_FLAGS: u8 = 2;

pub fn write_header(writer: &mut Writer, flags: u8) {
    writer.write_raw(MAGIC);
    writer.write_u32(FORMAT_VERSION);
    writer.write_u8(flags);
    writer.write_u64(super::fingerprint::shipped_world_fingerprint());
}

/// A module's provenance. Kept for every module, source or not: it is what a
/// loaded world reports as the module's origin, and it is a path or a name,
/// not the program text.
pub fn encode_source_origin(writer: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            writer.write_u8(ORIGIN_FILE);
            writer.write_str(&path.to_string_lossy());
        }
        SourceOrigin::Embedded(name) => {
            writer.write_u8(ORIGIN_EMBEDDED);
            writer.write_str(name);
        }
    }
}

/// Encodes the four kinds of `Value` that are no object (tags 0-4); returns
/// `false` for an object, which `encode_value` (the object-graph encoder)
/// handles.
pub fn encode_scalar_value<'gc>(writer: &mut Writer, value: Value<'gc>) -> bool {
    match value.kind() {
        ValueKind::Nil => writer.write_u8(VALUE_NIL),
        ValueKind::Boolean(false) => writer.write_u8(VALUE_FALSE),
        ValueKind::Boolean(true) => writer.write_u8(VALUE_TRUE),
        ValueKind::SmallInteger(integer) => {
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(integer);
        }
        ValueKind::SmallFloat(float) => {
            writer.write_u8(VALUE_FLOAT);
            writer.write_f64(float);
        }
        ValueKind::Object(_) => return false,
    }
    true
}

/// A `usize` count or index narrowed to the wire's `u32` field, refusing to
/// truncate: a length prefix that does not describe the payload that
/// follows would make the image self-inconsistent, which no reader could
/// detect. Only for a count no program is meant to push past `u32::MAX`: an
/// id or count of heap objects, frames among them, which only the heap's
/// memory bounds, so a write past it aborts rather than refusing; one of
/// declarations, statements or instructions, which source bounds (a
/// `Span` offset is a `u32`); the symbol table's, which the two bound
/// together; and a frame's program counter, an instruction index. Past it
/// is an invariant an image write must not violate, so it aborts loudly. A
/// length a running program decides -- a string's or symbol's text, a value
/// list, an integer's digits -- is checked with `u32_length` first.
pub(crate) fn u32_field(what: &str, value: usize) -> u32 {
    assert!(
        u32::try_from(value).is_ok(),
        "image {what} of {value} does not fit the format's u32 field"
    );
    value as u32
}

/// A length a running program decides, narrowed to the wire's `u32` field:
/// a string's, an array's or instance's value list, a `LargeInteger`'s digit
/// count. One past `u32::MAX` is a program the image format cannot carry,
/// so the image write is refused with an error rather than aborted.
fn u32_length(what: &str, value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| {
        format!("image encoding cannot write a {what} of {value}: it does not fit the image format's 32-bit field")
    })
}

/// A register list's length, a closure's parameter or copied-value count, a
/// method's or factory's arity, narrowed to the wire's `u16` field. A
/// program can pass it within the register ceiling -- a send of more than
/// 65,535 arguments that all read one register, a method of more than 65,535
/// parameters (`docs/design/compiler.md`, "Compilation cannot fail") -- so
/// one past `u16::MAX` refuses the image write with an error rather than
/// aborting it.
pub(crate) fn u16_field(what: &str, value: usize) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| {
        format!("image encoding cannot write a {what} of {value}: it does not fit the image format's 16-bit field")
    })
}

/// `Rc<CompiledCode>` pointer -> code-table id, assigned by
/// `encode_code_table`; `encode_closure_record` uses it for a
/// closure's code reference.
#[derive(Default)]
pub struct CodeIds {
    ids: HashMap<usize, u32>,
}

impl CodeIds {
    /// A body no module the image carries compiled has no id: a graph that
    /// reaches a closure over another module's code. An `Err` here, not a
    /// panic, because what the graph reaches is the program's to decide.
    pub fn id_of(&self, code: &Rc<CompiledCode>) -> Result<u32, String> {
        self.ids
            .get(&(Rc::as_ptr(code) as usize))
            .copied()
            .ok_or_else(|| {
                "image encoding cannot serialize a closure over code that no module the \
                 image carries compiled"
                    .to_string()
            })
    }
}

/// Explicit wire tags for `CodeKind` -- never `as u8` on the enum, whose
/// declaration order is not a wire format.
pub fn code_kind_tag(kind: CodeKind) -> u8 {
    match kind {
        CodeKind::Method => 0,
        CodeKind::Block => 1,
        CodeKind::Initializer => 2,
        CodeKind::SuperclassClause => 3,
        CodeKind::SuperclassMessage => 4,
    }
}

/// The code table: every `CompiledCode` reachable from the mixin trees under
/// `module_roots` -- every mixin's methods, every mixin's initializer, every
/// superclass clause and clause-message body, every `<:` clause's message
/// body, and every nested block body --
/// with nested block code *before* the code that creates it. Flat and
/// unnamed: *which* body belongs to which declaration is the mixin table's
/// business (`image::mixins`), which addresses this table by id.
///
/// Collected in `mixin::preorder` order, module after module -- the same
/// order the mixin table's rows are written in -- so the same module always encodes to the same
/// bytes.
pub fn encode_code_table(
    writer: &mut Writer,
    module_roots: &[Rc<crate::mixin::Mixin>],
    symbols: &mut symbols::Builder,
) -> Result<CodeIds, String> {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds::default();
    let rows = mixins::plan(module_roots)?;
    for row in &rows {
        for method in &row.methods {
            collect_code(&method.code, &mut order, &mut ids);
        }
        for code in [
            &row.mixin.initializer_code,
            &row.mixin.superclass_clause_code,
            &row.mixin.superclass_message_code,
        ]
        .into_iter()
        .chain(
            row.mixin
                .mixin_applications
                .iter()
                .map(|clause| &clause.message_code),
        )
        .flatten()
        {
            collect_code(code, &mut order, &mut ids);
        }
    }
    writer.write_u32(u32_field("code table entry count", order.len()));
    for code in &order {
        encode_code(writer, code, &ids, symbols)?;
    }
    Ok(ids)
}

/// Post-order: a block's code is appended (and gets its id) before the
/// code containing its `MakeClosure`, so every `code` operand encodes as an
/// id that is already assigned -- at any nesting depth, since the recursion
/// bottoms out at a body with no `MakeClosure` of its own.
fn collect_code(code: &Rc<CompiledCode>, order: &mut Vec<Rc<CompiledCode>>, ids: &mut CodeIds) {
    let key = Rc::as_ptr(code) as usize;
    if ids.ids.contains_key(&key) {
        return;
    }
    for instruction in &code.instructions() {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            collect_code(block, order, ids);
        }
    }
    ids.ids.insert(key, u32_field("code table id", order.len()));
    order.push(code.clone());
}

fn encode_code(
    writer: &mut Writer,
    code: &CompiledCode,
    ids: &CodeIds,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    writer.write_u8(code_kind_tag(code.kind));
    writer.write_u16(code.argument_count);
    writer.write_u16(code.register_count);
    // `outer_reach` is deliberately absent: it follows from the
    // instructions, and `decode_code` recomputes it as the compiler did,
    // through `CompiledCode::from_builder`.
    writer.write_u16(code.copied_count);
    writer.write_u32(code.source.start);
    writer.write_u32(code.source.end);
    writer.write_u32(u32_field(
        "statement map length",
        code.statement_program_counter.len(),
    ));
    for (statement, range) in &code.statement_program_counter {
        writer.write_u32(u32_field("statement index", *statement));
        writer.write_u32(u32_field("statement program counter", range.start));
        writer.write_u32(u32_field("statement program counter", range.end));
    }
    // A call-site number is no longer part of the artifact -- the decoder
    // derives it from a family's emission order once every body is read
    // (`decode::number_call_site_families`). What the wire still has to say
    // is whether this body's sends run uncached at all: the reference
    // lowering marks every send `CallSite::UNCACHED` and never numbers any
    // of them, and that is a whole-body property the decoder cannot recover
    // from the instructions alone once their numbers are gone.
    writer.write_boolean(compiled_uncached(code));
    writer.write_u32(u32_field("instruction count", code.instruction_count()));
    for instruction in &code.instructions() {
        encode_instruction(writer, instruction, ids, symbols)?;
    }
    Ok(())
}

/// Whether `code`'s own sends are `CallSite::UNCACHED` -- true exactly for a
/// body the reference lowering compiled, which numbers no send at all
/// (`compiler::reference`). The optimized lowering numbers every send in a
/// body uniformly, so one send (if `code` has any) tells the whole body's
/// story; a body with no sends of its own needs no flag, since
/// `decode_instruction` never reads it without a send to build.
fn compiled_uncached(code: &CompiledCode) -> bool {
    code.instructions()
        .iter()
        .filter_map(Instruction::call_site)
        .next()
        .is_some_and(|site| site == CallSite::UNCACHED)
}

fn encode_registers(writer: &mut Writer, registers: &[u16]) -> Result<(), String> {
    writer.write_u16(u16_field("register list", registers.len())?);
    for register in registers {
        writer.write_u16(*register);
    }
    Ok(())
}

fn encode_instruction(
    writer: &mut Writer,
    instruction: &Instruction,
    ids: &CodeIds,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    match instruction {
        // The code table is collected from the module's own mixin tree
        // (`mixins::plan`), which never contains a shipped mixin, so this is
        // unreachable by construction: a panic here is a defect in this
        // process, not a property of any input.
        Instruction::SendMirror { .. } => unreachable!(
            "internal: SendMirror is emitted only into shipped core-library source, and an image \
             carries only its own module's code"
        ),
        Instruction::LoadLiteral {
            destination,
            literal,
        } => {
            writer.write_u8(OPCODE_LOAD_LITERAL);
            writer.write_u16(*destination);
            encode_literal(writer, literal, symbols)?;
        }
        Instruction::LoadSelf { destination } => {
            writer.write_u8(OPCODE_LOAD_SELF);
            writer.write_u16(*destination);
        }
        Instruction::LoadEnclosingObject { destination, depth } => {
            writer.write_u8(OPCODE_LOAD_ENCLOSING_OBJECT);
            writer.write_u16(*destination);
            writer.write_u16(*depth);
        }
        Instruction::Move {
            destination,
            source,
        } => {
            writer.write_u8(OPCODE_MOVE);
            writer.write_u16(*destination);
            writer.write_u16(*source);
        }
        Instruction::StoreLocal { source, slot } => {
            writer.write_u8(OPCODE_STORE_LOCAL);
            writer.write_u16(*source);
            writer.write_u16(*slot);
        }
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => {
            writer.write_u8(OPCODE_LOAD_OUTER_LOCAL);
            writer.write_u16(*destination);
            writer.write_u16(*depth);
            writer.write_u16(*index);
        }
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => {
            writer.write_u8(OPCODE_STORE_OUTER_LOCAL);
            writer.write_u16(*source);
            writer.write_u16(*depth);
            writer.write_u16(*index);
        }
        Instruction::SendLexical {
            depth,
            selector,
            argument_registers,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_LEXICAL);
            writer.write_u16(*depth);
            writer.write_u32(symbols.intern(selector.as_str()));
            encode_registers(writer, argument_registers)?;
            writer.write_u16(*destination);
        }
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_PUBLIC);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*receiver_register);
            encode_registers(writer, argument_registers)?;
            writer.write_u16(*destination);
        }
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_registers,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_INLINABLE);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*receiver_register);
            encode_registers(writer, argument_registers)?;
            writer.write_u16(*destination);
        }
        Instruction::SendSuper {
            selector,
            argument_registers,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_SUPER);
            writer.write_u32(symbols.intern(selector.as_str()));
            encode_registers(writer, argument_registers)?;
            writer.write_u16(*destination);
        }
        Instruction::MakeClosure {
            destination,
            parameters,
            code,
            copied,
        } => {
            writer.write_u8(OPCODE_MAKE_CLOSURE);
            writer.write_u16(*destination);
            writer.write_u16(u16_field("closure parameter count", parameters.len())?);
            for parameter in parameters.iter() {
                writer.write_u32(symbols.intern(parameter.as_str()));
            }
            // In the table by construction, not by the input's say:
            // `collect_code` numbers a nested block before its parent.
            writer.write_u32(ids.id_of(code).expect(
                "internal: collect_code assigned every nested block an id before its parent",
            ));
            encode_registers(writer, copied)?;
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            writer.write_u8(OPCODE_MAKE_ARRAY);
            writer.write_u16(*destination);
            encode_registers(writer, element_registers)?;
        }
        Instruction::MakeVector { destination, size } => {
            writer.write_u8(OPCODE_MAKE_VECTOR);
            writer.write_u16(*destination);
            writer.write_u16(*size);
        }
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => {
            writer.write_u8(OPCODE_LOAD_VECTOR);
            writer.write_u16(*destination);
            writer.write_u16(*vector);
            writer.write_u16(*index);
        }
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => {
            writer.write_u8(OPCODE_STORE_VECTOR);
            writer.write_u16(*vector);
            writer.write_u16(*index);
            writer.write_u16(*source);
        }
        Instruction::ReturnSlotIfFilled { own_index } => {
            writer.write_u8(OPCODE_RETURN_SLOT_IF_FILLED);
            writer.write_u32(*own_index);
        }
        Instruction::InitializeSlot { source, own_index } => {
            writer.write_u8(OPCODE_INITIALIZE_SLOT);
            writer.write_u16(*source);
            writer.write_u32(*own_index);
        }
        Instruction::MakeFuture {
            destination,
            closure,
        } => {
            writer.write_u8(OPCODE_MAKE_FUTURE);
            writer.write_u16(*destination);
            writer.write_u16(*closure);
        }
        Instruction::ResolveFuture {
            destination,
            source,
        } => {
            writer.write_u8(OPCODE_RESOLVE_FUTURE);
            writer.write_u16(*destination);
            writer.write_u16(*source);
        }
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => {
            writer.write_u8(OPCODE_LOAD_OWN_SLOT);
            writer.write_u16(*destination);
            writer.write_u32(*own_index);
        }
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            writer.write_u8(OPCODE_INITIALIZE_SLOT_PROVISIONALLY);
            writer.write_u16(*source);
            writer.write_u32(*own_index);
        }
        Instruction::Return { register } => {
            writer.write_u8(OPCODE_RETURN);
            writer.write_u16(*register);
        }
        Instruction::ImplicitReturn { register } => {
            writer.write_u8(OPCODE_IMPLICIT_RETURN);
            writer.write_u16(*register);
        }
        Instruction::NonLocalReturn { register } => {
            writer.write_u8(OPCODE_NON_LOCAL_RETURN);
            writer.write_u16(*register);
        }
        Instruction::SignalNonBoolean { register } => {
            writer.write_u8(OPCODE_SIGNAL_NON_BOOLEAN);
            writer.write_u16(*register);
        }
        Instruction::Jump { target } => {
            writer.write_u8(OPCODE_JUMP);
            writer.write_u32(*target);
        }
        Instruction::JumpIfNotBoolean { register, target } => {
            writer.write_u8(OPCODE_JUMP_IF_NOT_BOOLEAN);
            writer.write_u16(*register);
            writer.write_u32(*target);
        }
        Instruction::BranchIfFalse { register, target } => {
            writer.write_u8(OPCODE_BRANCH_IF_FALSE);
            writer.write_u16(*register);
            writer.write_u32(*target);
        }
        Instruction::BranchIfTrue { register, target } => {
            writer.write_u8(OPCODE_BRANCH_IF_TRUE);
            writer.write_u16(*register);
            writer.write_u32(*target);
        }
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => {
            writer.write_u8(OPCODE_INTEGER_BINARY);
            writer.write_u8(integer_operation_tag(*operation));
            writer.write_u16(*destination);
            writer.write_u16(*left);
            writer.write_u16(*right);
            writer.write_u32(*target);
        }
    }
    Ok(())
}

/// `IntegerOperation` as a wire byte. Exhaustive on purpose -- a tenth
/// operation added later fails to compile here rather than silently encoding
/// as one of the existing nine.
pub fn integer_operation_tag(operation: IntegerOperation) -> u8 {
    match operation {
        IntegerOperation::Add => 0,
        IntegerOperation::Subtract => 1,
        IntegerOperation::Multiply => 2,
        IntegerOperation::FloorDivide => 3,
        IntegerOperation::Less => 4,
        IntegerOperation::Greater => 5,
        IntegerOperation::LessOrEqual => 6,
        IntegerOperation::GreaterOrEqual => 7,
        IntegerOperation::Equal => 8,
    }
}

/// A `LargeInteger`'s wire form, shared by the value and the literal tag:
/// a sign byte, the digit count, the digits least significant first.
fn write_large_integer(writer: &mut Writer, integer: &ArbitraryInteger) -> Result<(), String> {
    writer.write_u8(u8::from(integer.is_negative()));
    writer.write_u32(u32_length(
        "large integer digit count",
        integer.magnitude().len(),
    )?);
    for digit in integer.magnitude() {
        writer.write_u64(*digit);
    }
    Ok(())
}

fn encode_literal(
    writer: &mut Writer,
    literal: &Literal,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    match literal {
        Literal::Nil => writer.write_u8(LITERAL_NIL),
        Literal::Boolean(boolean) => {
            writer.write_u8(LITERAL_BOOLEAN);
            writer.write_boolean(*boolean);
        }
        Literal::SmallInteger(integer) => {
            writer.write_u8(LITERAL_SMALL_INTEGER);
            writer.write_i64(*integer);
        }
        Literal::Float(float) => {
            writer.write_u8(LITERAL_FLOAT);
            writer.write_f64(*float);
        }
        Literal::String(string) => {
            writer.write_u8(LITERAL_STRING);
            writer.write_str(string);
        }
        Literal::Symbol(symbol) => {
            writer.write_u8(LITERAL_SYMBOL);
            writer.write_u32(symbols.intern(symbol.as_str()));
        }
        // One that fits `i64` travels as an `i64` (`encode_value`).
        Literal::LargeInteger(integer) => match integer.to_i64() {
            Some(number) => {
                writer.write_u8(LITERAL_SMALL_INTEGER);
                writer.write_i64(number);
            }
            None => {
                writer.write_u8(LITERAL_LARGE_INTEGER);
                write_large_integer(writer, integer)?;
            }
        },
        Literal::UnresolvedIdentifier(name) => {
            writer.write_u8(LITERAL_UNRESOLVED_IDENTIFIER);
            writer.write_str(name);
        }
        Literal::BodyTooLarge => writer.write_u8(LITERAL_BODY_TOO_LARGE),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The object table (`docs/design/image-format.md`, "Sections").
// ---------------------------------------------------------------------------

/// A class object's graph references, in the order its record writes them:
/// the superclass, the enclosing object, the declaring class. Traversal and
/// encoding share one definition so the two cannot drift into a reference
/// that is written but never given an id.
///
/// A level whose `<:` clause named a shipped class has only its superclass:
/// its enclosing object and declaring class are the shipped class's own
/// (§6.2, "the enclosing object of I is the enclosing object of C"), which
/// is a shipped module's instance -- for a builtin the kernel, the object
/// that holds the VM mirror. The image carries neither, exactly as it
/// carries no shipped class; the decoder takes both from the loading
/// world's class, as `nested::build_applied_levels` does.
fn class_references<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> impl Iterator<Item = ObjectReference<'gc>> {
    let own_context = !applies_a_shipped_mixin(heap, class);
    let (enclosing, declaring_class) = if own_context {
        (heap.class_enclosing(class), heap.class_declaring(class))
    } else {
        (None, None)
    };
    heap.class_superclass(class)
        .into_iter()
        .chain(enclosing)
        .chain(declaring_class)
}

/// Whether `class` is a level a program's `<:` clause added over a shipped
/// class's mixin: a record of the program's, written `MIXIN_SHIPPED`. A
/// level a shipped declaration's own clause added is a shipped name and
/// never a record, which the graph walk decides before it asks this.
fn applies_a_shipped_mixin<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> bool {
    heap.class_applied_origin(class).is_some_and(|applied| {
        !applied.declaration.shipped
            && (applied.named_builtin.is_some() || heap.class_is_shipped(class))
    })
}

/// Why the image cannot carry `object` when it is a class object compiled
/// from shipped source that no shipped name names: its code and layout
/// belong to the loading binary, and only the shipped world's own objects
/// have names to resolve to. The VM mirror's class has no declaration and
/// so no name either. A level a program's `<:` clause added over a shipped
/// mixin belongs to the program (`applies_a_shipped_mixin`), and its
/// metaclass beside it, so neither is a refusal.
fn unnamed_shipped_class<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<String> {
    if !heap.is_class(object) {
        return None;
    }
    let class = heap.class_this_class(object).unwrap_or(object);
    if heap.class_builtin(class) == Some(BuiltinClass::VmMirror) {
        return Some(format!(
            "image encoding refuses {}: it has no declaration, so no shipped name names it",
            if heap.class_this_class(object).is_some() {
                "the VM mirror's class class"
            } else {
                "the VM mirror's class"
            }
        ));
    }
    if applies_a_shipped_mixin(heap, class) || !heap.class_is_shipped(object) {
        return None;
    }
    Some(format!(
        "image encoding refuses the class object {}: it is compiled from shipped source, \
         and no shipped name names it",
        heap.class_display_name(object)
    ))
}

/// The shipped names a graph section uses, as positions in the shipped
/// world's entries, in the order the graph first used each -- the order the
/// shipped-name section lists them in and its references number them.
pub struct UsedNames {
    pub(crate) names: Vec<usize>,
    index: HashMap<usize, u32>,
}

impl UsedNames {
    fn new() -> UsedNames {
        UsedNames {
            names: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// The section index of the world's entry `world_index`, listing it
    /// first if nothing has used it yet.
    fn use_name(&mut self, world_index: usize) -> u32 {
        if let Some(index) = self.index.get(&world_index) {
            return *index;
        }
        let index = u32_field("shipped name index", self.names.len());
        self.index.insert(world_index, index);
        self.names.push(world_index);
        index
    }

    /// The section index of the world's entry `world_index`, which the graph
    /// walk listed.
    fn index_of(&self, world_index: usize) -> u32 {
        *self
            .index
            .get(&world_index)
            .expect("internal: the graph walk listed every shipped name the encoder writes")
    }

    /// The names, in section order: what a host that decodes a graph
    /// section it wrote by hand resolves its `DecodeContext::shipped` from
    /// (`shipped_names::resolve_in`).
    pub fn names(&self, mutator: &Mutator<'_>) -> Vec<ShippedName> {
        let entries = mutator.shipped_world.entries();
        self.names
            .iter()
            .map(|index| entries[*index].name)
            .collect()
    }
}

/// Every heap object (instances, strings, symbols, arrays, closures,
/// reified frames, and the class objects that are not shipped, the carried
/// modules' root classes among them) reachable from the roots and the
/// module roots, in discovery order, and the shipped names it reached. A
/// shipped object is never entered: the loading world has it, so a name of
/// the shipped-name section stands for it.
pub struct GraphTable<'gc> {
    objects: Vec<ObjectReference<'gc>>,
    object_ids: HashMap<usize, u32>,
    /// Whether the traversal reached the VM mirror.
    reached_mirror: bool,
    /// The first shipped class object no shipped name names that the
    /// traversal reached (`unnamed_shipped_class`).
    unnamed: Option<String>,
    used: UsedNames,
    /// The ephemerons met whose key the walk has not reached yet, each
    /// with its key: their value and finalizer are traced only once it is
    /// (`GraphTable::build`).
    pending_ephemerons: Vec<(ObjectReference<'gc>, ObjectReference<'gc>)>,
    /// The ephemerons whose key the walk never reached, by identity key:
    /// written with every field `nil`, as a collection would leave them.
    mourned: HashSet<usize>,
}

/// What the graph walk resolves an object against before it enters it: the
/// shipped world's entries by object (`ShippedWorld::index_by_object`) and
/// by mixin (`ShippedWorld::index_by_mixin`), and the VM mirror.
struct Named<'a, 'gc> {
    by_object: &'a HashMap<usize, usize>,
    by_mixin: &'a HashMap<usize, usize>,
    mirror: Option<ObjectReference<'gc>>,
}

impl<'gc> GraphTable<'gc> {
    /// Pre-order depth-first from the roots, over an *explicit* stack
    /// rather than the Rust call stack. A live heap's object graph is as
    /// deep as the program that built it -- a Newspeak program can chain a
    /// million arrays or a million activations -- and recursing over it
    /// would overflow the stack, which aborts the process instead of
    /// failing. The order is exactly what the recursive formulation would
    /// produce (children pushed in reverse so they pop in order), which is
    /// what makes the encoding canonical.
    ///
    /// The carried modules' root classes are walked after the roots, as if
    /// they followed them: a root class the roots reach keeps the id the
    /// walk gives it there, and one they do not reach -- a module only
    /// carried for its code -- still becomes a record, the one the module
    /// section names.
    ///
    /// The walk keeps what a collection would keep
    /// (`docs/design/core-library.md`, "Weak references"): a weak array's
    /// slots are not followed, and an ephemeron's fields are followed only
    /// once its key is reached. An ephemeron met before its key waits; once the
    /// walk ends, each waiting one whose key now has an id is traced, and
    /// the scan repeats until a round traces none, so a key reached only
    /// through another ephemeron's value is found however long the chain.
    /// Every round scans in list order, so the order stays canonical.
    fn build(
        heap: &Heap<'gc>,
        roots: &[Value<'gc>],
        module_roots: &[ClassReference<'gc>],
        named: &Named<'_, 'gc>,
    ) -> GraphTable<'gc> {
        let mut table = GraphTable {
            objects: Vec::new(),
            object_ids: HashMap::new(),
            reached_mirror: false,
            unnamed: None,
            used: UsedNames::new(),
            pending_ephemerons: Vec::new(),
            mourned: HashSet::new(),
        };
        let mut stack: Vec<Value<'gc>> = module_roots
            .iter()
            .rev()
            .map(|class| Value::object(*class))
            .chain(roots.iter().rev().copied())
            .collect();
        table.drain(heap, named, &mut stack);
        loop {
            let mut progressed = false;
            for (ephemeron, key) in std::mem::take(&mut table.pending_ephemerons) {
                if table.has_id(heap, key) {
                    progressed = true;
                    let fields = heap.instance_fields(ephemeron).unwrap_or_default();
                    stack.extend(fields.into_iter().rev());
                    table.drain(heap, named, &mut stack);
                } else {
                    table.pending_ephemerons.push((ephemeron, key));
                }
            }
            if !progressed {
                break;
            }
        }
        table.mourned = std::mem::take(&mut table.pending_ephemerons)
            .into_iter()
            .map(|(ephemeron, _)| heap.object_identity_key(ephemeron))
            .collect();
        table
    }

    fn drain(&mut self, heap: &Heap<'gc>, named: &Named<'_, 'gc>, stack: &mut Vec<Value<'gc>>) {
        while let Some(value) = stack.pop() {
            self.visit_value(heap, value, named, stack);
        }
    }

    fn has_id(&self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> bool {
        self.object_ids
            .contains_key(&heap.object_identity_key(object))
    }

    /// Answers the object `value` names if it is one the graph would make
    /// a record of, which only its id, once the walk has given it one,
    /// keeps reachable; `None` for everything else, which needs no record:
    /// an immediate, a value written by value, a shipped name -- the
    /// loading world holds those strongly, so a weak word or an
    /// ephemeron's key holding one always keeps it -- and what is refused.
    /// Notes a shipped name, the VM mirror or an unnamed shipped class
    /// wherever the walk meets one, weakly held or not.
    fn admit(
        &mut self,
        heap: &Heap<'gc>,
        value: Value<'gc>,
        named: &Named<'_, 'gc>,
    ) -> Option<ObjectReference<'gc>> {
        // A resolved future is never entered: every reference to one is
        // written as its value (`encode_value`), so its value is what the
        // graph holds.
        let value = future::seen_through(heap, value);
        let object = value.as_object()?;
        if written_by_value(heap, object) {
            return None;
        }
        // The VM mirror is a capability the bootstrap hands to the core
        // library alone (`docs/design/core-library.md`, "The VM mirror is
        // the whole Rust boundary"): no name names it, and nothing of it is
        // written.
        let is_mirror = named.mirror.is_some_and(|mirror| mirror.same(object))
            || heap.class_builtin(heap.object_class(object)) == Some(BuiltinClass::VmMirror);
        if is_mirror {
            self.reached_mirror = true;
            return None;
        }
        // Named by its declaration: the loading world has it, so the image
        // carries nothing of it.
        if let Some(world_index) = named.by_object.get(&heap.object_identity_key(object)) {
            self.used.use_name(*world_index);
            return None;
        }
        if let Some(refusal) = unnamed_shipped_class(heap, object) {
            if self.unnamed.is_none() {
                self.unnamed = Some(refusal);
            }
            return None;
        }
        Some(object)
    }

    fn visit_value(
        &mut self,
        heap: &Heap<'gc>,
        value: Value<'gc>,
        named: &Named<'_, 'gc>,
        stack: &mut Vec<Value<'gc>>,
    ) {
        let Some(object) = self.admit(heap, value, named) else {
            return;
        };
        let key = heap.object_identity_key(object);
        if self.object_ids.contains_key(&key) {
            return;
        }
        self.object_ids
            .insert(key, u32_field("object table id", self.objects.len()));
        self.objects.push(object);
        let before = stack.len();
        {
            let shape = heap.object_shape(object);
            // An object's own class is a reference like every other: a
            // record's header names it, a frame record's `Kernel
            // Activation` and an unresolved future's `Kernel
            // FutureImplementation` among them, by a shipped name the walk
            // lists here.
            stack.push(Value::object(heap.object_class(object)));
            match shape {
                ObjectShape::Instance => {
                    stack.extend(heap.instance_fields(object).unwrap_or_default());
                }
                ObjectShape::Array => {
                    stack.extend(heap.array_elements(object).unwrap_or_default());
                }
                ObjectShape::Closure => {
                    stack.extend(
                        heap.closure_captured_frame(object)
                            .map(|frame| Value::object(heap.frame_object(frame))),
                    );
                    // A closure's home is not walked: it is derived from
                    // the captured frame on both sides of the wire
                    // (`Heap::closure_home`), so it is never a reference of
                    // its own.
                    stack.extend(heap.closure_copied_values(object).unwrap_or_default());
                }
                ObjectShape::Class => match heap.class_this_class(object) {
                    // A metaclass record writes one reference, its class:
                    // everything else about it is derived from that class.
                    Some(this_class) => stack.push(Value::object(this_class)),
                    // A class object's three references; its metaclass is
                    // its header, pushed above like every object's class. A
                    // level over a shipped mixin names that mixin's class.
                    None => {
                        if applies_a_shipped_mixin(heap, object) {
                            let mixin = heap
                                .class_application(object)
                                .map(|application| crate::mixin::mixin_key(&application.mixin));
                            if let Some(world_index) =
                                mixin.and_then(|mixin| named.by_mixin.get(&mixin))
                            {
                                self.used.use_name(*world_index);
                            }
                        }
                        stack.extend(class_references(heap, object).map(Value::object));
                    }
                },
                ObjectShape::Activation => {
                    if let Some(frame) = heap.object_frame(object) {
                        stack.extend(frame_references(heap, frame));
                    }
                }
                ObjectShape::String
                | ObjectShape::Symbol
                | ObjectShape::LargeInteger
                | ObjectShape::Float
                | ObjectShape::ByteArray => {}
                // Not followed: a slot names its object only if something
                // else reaches it (`GraphTable::weakly_held`). Admitted all
                // the same, so a shipped name a slot holds is listed and
                // the mirror refused.
                ObjectShape::WeakArray => {
                    for element in heap.weak_array_elements(object).unwrap_or_default() {
                        self.admit(heap, element, named);
                    }
                }
                ObjectShape::Ephemeron => {
                    let fields = heap.instance_fields(object).unwrap_or_default();
                    // An ephemeron without fields has no key; only a
                    // crafted image could make one, and nothing waits on it.
                    let key = fields.first().and_then(|key| self.admit(heap, *key, named));
                    match key {
                        Some(key) if !self.has_id(heap, key) => {
                            self.pending_ephemerons.push((object, key));
                        }
                        _ => stack.extend(fields),
                    }
                }
            }
        }
        stack[before..].reverse();
    }

    /// What a weak word holding `value` is written as: `value` if the graph
    /// writes it otherwise (`admit`), else `nil`, as a collection would
    /// leave it. `shipped` is the shipped world's entries by object.
    fn weakly_held(
        &self,
        heap: &Heap<'gc>,
        shipped: &HashMap<usize, usize>,
        value: Value<'gc>,
    ) -> Value<'gc> {
        match future::seen_through(heap, value).as_object() {
            Some(object)
                if !written_by_value(heap, object)
                    && !shipped.contains_key(&heap.object_identity_key(object))
                    && !self.has_id(heap, object) =>
            {
                Value::nil()
            }
            _ => value,
        }
    }

    pub fn object_id(&self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> u32 {
        *self
            .object_ids
            .get(&heap.object_identity_key(object))
            .expect("object was visited by build()")
    }

    /// A frame's id: its frame object's, since a frame is an object record.
    pub fn frame_id(&self, heap: &Heap<'gc>, frame: FrameReference<'gc>) -> u32 {
        self.object_id(heap, heap.frame_object(frame))
    }

    pub fn objects(&self) -> &[ObjectReference<'gc>] {
        &self.objects
    }
}

/// Whether `object` is a value, not a record: `encode_value` writes a
/// `LargeInteger`'s digits, a boxed float's bits or a fraction's two parts
/// wherever it is referenced, so it takes no object id (§3.1.1), and a
/// fraction's parts are no objects of the graph either.
fn written_by_value<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> bool {
    heap.is_large_integer(object)
        || heap.is_boxed_float(object)
        || heap.fraction_parts(object).is_some()
}

/// A frame's graph references, in the order its record writes them: the
/// object or class its kind names, the receiver, the sender's and the home's
/// frame objects, the current class, then the registers (`encode_frame`).
/// The walk gives each one an id in that order, so the table's order is
/// the one a record reads in.
fn frame_references<'gc>(
    heap: &Heap<'gc>,
    frame: FrameReference<'gc>,
) -> impl Iterator<Item = Value<'gc>> {
    let snapshot = heap.frame_record(frame);
    let kind_reference = match &snapshot.kind {
        // The closure object a closure activation runs is a record of its
        // own, so it needs an id like any other object.
        FrameKind::Closure(closure) => Some(*closure),
        // The class-carrying kinds name a class the same way `current_class`
        // does, so the class needs an id the same way.
        FrameKind::Initializer(class)
        | FrameKind::SuperclassClause(class)
        | FrameKind::SuperclassMessage(class)
        | FrameKind::MixinApplicationMessage(class) => Some(*class),
        FrameKind::Method(_) | FrameKind::TopLevel => None,
    };
    let frame_objects = [snapshot.sender, snapshot.home]
        .into_iter()
        .flatten()
        .map(|frame| Value::object(heap.frame_object(frame)));
    let frame_objects: Vec<Value<'gc>> = frame_objects.collect();
    kind_reference
        .map(Value::object)
        .into_iter()
        .chain(std::iter::once(snapshot.receiver))
        .chain(frame_objects)
        .chain(snapshot.current_class.map(Value::object))
        .chain(snapshot.arguments.into_iter().chain(snapshot.temporaries))
}

/// Everything the graph encoder resolves a reference against: the object
/// table and the names it used, the shipped world it names into, the code
/// table's ids, and the mixin table's row ids -- which is how a class
/// object's record names the declaration it applies.
pub struct EncodeContext<'a, 'gc> {
    pub table: &'a GraphTable<'gc>,
    pub code_ids: &'a CodeIds,
    world: &'a ShippedWorld<'gc>,
    /// `ShippedWorld::index_by_object`, built for this encoding.
    shipped_by_object: &'a HashMap<usize, usize>,
    /// `ShippedWorld::index_by_mixin`, built for this encoding.
    shipped_by_mixin: &'a HashMap<usize, usize>,
    /// `mixin::mixin_key` -> mixin-table row id, as `mixins::encode`
    /// assigned them.
    pub mixin_ids: &'a HashMap<usize, u32>,
    pub heap: &'a Heap<'gc>,
}

impl<'gc> EncodeContext<'_, 'gc> {
    /// The shipped world's entry naming `object`, by its position there,
    /// when one does.
    fn shipped(&self, object: ObjectReference<'gc>) -> Option<(usize, &ShippedEntry<'gc>)> {
        let world_index = *self
            .shipped_by_object
            .get(&self.heap.object_identity_key(object))?;
        Some((world_index, &self.world.entries()[world_index]))
    }
}

/// The refusal of a shipped name standing where the rule of what may be
/// named where forbids it: the loader refuses such an image, so none is
/// written.
fn not_allowed(entry: &ShippedEntry<'_>, position: NamePosition) -> String {
    format!(
        "image encoding refuses {} as {}: no program may name it there, and the loader \
         refuses an image that does",
        entry.name.describe(),
        position.describe()
    )
}

/// `Shipped` (8 + section index) or `Record` (2 + object id) -- the two
/// ways a class object can exist in a loaded world. `position` is where the
/// reference stands, which decides whether a shipped name may be written
/// there.
///
/// A shipped class, metaclass or level exists before the graph is decoded:
/// the loading binary has it. Every other class object is one the *graph*
/// creates -- a module's root class, the levels its `<:` clauses add, every
/// nested class and every metaclass of these -- and tag 2 names which one
/// by the id of its own record in the object table. That is what nesting
/// needs: a nested class object is the value of its enclosing instance's
/// hidden cache slot and the `.class` of every instance of it, and there is
/// one per enclosing instance, so no tag that names a class only by its
/// position in the source could tell them apart.
fn encode_class_tag<'gc>(
    writer: &mut Writer,
    class: ClassReference<'gc>,
    position: NamePosition,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let heap = context.heap;
    if !heap.is_class(class) {
        panic!("internal: a class reference does not point at a class object");
    }
    if let Some((world_index, entry)) = context.shipped(class) {
        if !position.allows(entry.positions) {
            return Err(not_allowed(entry, position));
        }
        writer.write_u8(CLASS_SHIPPED);
        writer.write_u32(context.table.used.index_of(world_index));
        return Ok(());
    }
    writer.write_u8(CLASS_RECORD);
    writer.write_u32(context.table.object_id(heap, class));
    Ok(())
}

fn encode_optional_class<'gc>(
    writer: &mut Writer,
    class: Option<&ClassReference<'gc>>,
    position: NamePosition,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    match class {
        Some(present_class) => {
            writer.write_boolean(true);
            encode_class_tag(writer, *present_class, position, context)
        }
        None => {
            writer.write_boolean(false);
            Ok(())
        }
    }
}

/// The general `Value` encoder: scalar tags 0-4 (`encode_scalar_value`),
/// 5 = object-table id, 6 = class tag, 9 = a `LargeInteger`'s digits, 10 =
/// a `Fraction`'s two parts, 11 = a shipped module's instance. A boxed
/// float travels as tag 4, as every float does. `position` is
/// `NamePosition::FrameRegister` for a frame's receiver and registers, the
/// one place a module instance may stand, and `NamePosition::Value`
/// everywhere else; a class in either is a value (`NamePosition::Value`).
///
/// A class object always goes through tag 6, even the ones tag 6 then
/// resolves to an object-table id: one rule per kind of referent is what
/// keeps the encoding canonical.
fn encode_value<'gc>(
    writer: &mut Writer,
    value: Value<'gc>,
    position: NamePosition,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    // A resolved future is written as its value, which is what the graph
    // walk recorded in its place (`GraphTable::visit_value`).
    let value = future::seen_through(context.heap, value);
    if encode_scalar_value(writer, value) {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        unreachable!("encode_scalar_value handles every non-Obj case")
    };
    // Written by value wherever it is referenced: two references to one
    // `LargeInteger` object load as two equal integers, which is all a value
    // without identity can promise. One that fits `i64` is written as an
    // `i64`: the wire keeps one canonical encoding per integer, independent
    // of where a `SmallInteger` ends, and the decoder decides by that range
    // which representation it loads as.
    if let Some(integer) = context.heap.large_integer(object) {
        return write_integer_value(writer, &integer);
    }
    // A `Fraction` likewise, by value: its two parts in the integers' own
    // encoding, which `write_integer_value` shares with the branch above.
    if let Some(fraction) = context.heap.fraction(object) {
        writer.write_u8(VALUE_FRACTION);
        write_integer_value(writer, fraction.numerator())?;
        write_integer_value(writer, fraction.denominator())?;
        return Ok(());
    }
    // A boxed float likewise, under the one tag every float travels as: the
    // decoder's `Mutator::new_float` decides by the range again which
    // representation it loads as.
    if let Some(float) = context.heap.float_value(value) {
        writer.write_u8(VALUE_FLOAT);
        writer.write_f64(float);
        return Ok(());
    }
    if let Some((world_index, entry)) = context.shipped(object) {
        if entry.name.kind == ShippedKind::ModuleInstance {
            // A module instance holds the core library's state -- the kernel
            // holds the mirror -- so only a frame of shipped code, whose
            // registers legitimately hold one, may name it.
            if position != NamePosition::FrameRegister {
                return Err(format!(
                    "image encoding refuses the module instance {} outside a frame: it holds \
                     the core library's state",
                    entry.name.module.as_str()
                ));
            }
            writer.write_u8(VALUE_SHIPPED_MODULE);
            writer.write_u32(context.table.used.index_of(world_index));
            return Ok(());
        }
    }
    if context.heap.is_class(object) {
        writer.write_u8(VALUE_CLASS_TAG);
        return encode_class_tag(writer, object, NamePosition::Value, context);
    }
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(context.table.object_id(context.heap, object));
    Ok(())
}

/// An integer as a value: `VALUE_SMALL_INTEGER` when it fits `i64`, else
/// `VALUE_LARGE_INTEGER` and its digits -- the one canonical encoding the
/// decoder holds every integer to, a fraction's parts included.
fn write_integer_value(writer: &mut Writer, integer: &ArbitraryInteger) -> Result<(), String> {
    match integer.to_i64() {
        Some(number) => {
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(number);
        }
        None => {
            writer.write_u8(VALUE_LARGE_INTEGER);
            write_large_integer(writer, integer)?;
        }
    }
    Ok(())
}

fn encode_values<'gc>(
    writer: &mut Writer,
    values: &[Value<'gc>],
    position: NamePosition,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    writer.write_u32(u32_length("value list", values.len())?);
    for value in values {
        encode_value(writer, *value, position, context)?;
    }
    Ok(())
}

fn encode_frame_reference<'gc>(
    heap: &Heap<'gc>,
    writer: &mut Writer,
    frame: Option<&FrameReference<'gc>>,
    table: &GraphTable<'gc>,
) {
    match frame {
        Some(present_frame) => {
            writer.write_boolean(true);
            writer.write_u32(table.frame_id(heap, *present_frame));
        }
        None => writer.write_boolean(false),
    }
}

/// `code_id`, the captured frame's object id, then the copied values,
/// counted.
///
/// No parameter names: the arity a call is checked against is the body's
/// own `argument_count`, so there is no second half to disagree with. No
/// home either: it follows from the captured frame
/// (`Heap::closure_home`).
///
/// Its feedback base is never on the wire: a decoded closure is given a
/// fresh range when it is first activated (`decode_closure_record`).
fn encode_closure_record<'gc>(
    writer: &mut Writer,
    closure: ObjectReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let heap = context.heap;
    let (Some(code), Some(captured_frame), Some(copied)) = (
        heap.closure_code(closure),
        heap.closure_captured_frame(closure),
        heap.closure_copied_values(closure),
    ) else {
        unreachable!("internal: an `ObjectShape::Closure` answers every closure accessor")
    };
    writer.write_u32(context.code_ids.id_of(&code)?);
    writer.write_u32(context.table.frame_id(heap, captured_frame));
    writer.write_u16(u16_field("closure copied value count", copied.len())?);
    for value in copied {
        encode_value(writer, value, NamePosition::Value, context)?;
    }
    Ok(())
}

/// A class object's record: which declaration it applies, and the three
/// references that make it *this* application of it rather than another.
///
/// The `MixinApplication` itself is deliberately absent. Layout and lookup
/// follow deterministically from the mixin and the superclass's own
/// application, so `image::decode` rebuilds each one through
/// `Mutator::apply_mixin` -- whose cache is keyed by exactly that pair,
/// which is what gives two enclosing instances back the *one* application
/// they shared before the snapshot (§6.2). Writing applications out would
/// have to reproduce that sharing by hand, and getting it wrong is silent:
/// the world still runs, and every inline cache built on it goes
/// megamorphic.
///
/// `builtin` is not written either -- a record only ever describes a
/// program's class, since a shipped class is a shipped name -- and neither
/// is its metaclass: the record's header names it, as every record's header
/// names its class (`encode_object`).
///
/// A metaclass never reaches here. It is written as a record of its own
/// shape, `OBJECT_METACLASS`, holding only the reference back to its class,
/// since its mixin, application, enclosing object and declaring class all
/// follow from that class (`docs/design/object-model.md`, "The image
/// format"); `decode::rebuild_applications` refuses a class record that
/// applies some row's class side.
///
/// A level a `<:` clause added (`newspeak-spec.pdf` §6.2) is a record like
/// any other, except that it ends with the clause that added it, as the
/// applying declaration's row id and the clause's position there: the
/// clause's selector and compiled message are that row's, which
/// `decode::verify_mixins` has reconciled with the row's own factory.
///
/// A level whose clause named a shipped class (`<: Value`) has a record of
/// its own shape (`MIXIN_SHIPPED`): its mixin is the shipped class's, which
/// no row describes, so it is named by that class's shipped name, and its
/// enclosing object and declaring class are the shipped class's too, so
/// they are not written at all (`class_references`). What is left is the
/// name, the superclass and the clause, which is never absent.
fn encode_class_record<'gc>(
    writer: &mut Writer,
    class: ClassReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let heap = context.heap;
    let applied_clause = heap.class_applied_origin(class);
    let superclass = heap.class_superclass(class);
    let applied_row = |applied: &crate::mixin::AppliedClause| {
        context
            .mixin_ids
            .get(&crate::mixin::mixin_key(&applied.declaration))
            .copied()
            .ok_or_else(|| {
                format!(
                    "image encoding cannot serialize a level added by class `{}`, \
                     which is not part of any module the image carries",
                    applied.declaration.name
                )
            })
    };
    let Some(application) = heap.class_application(class) else {
        unreachable!("internal: an `ObjectShape::Class` has an application")
    };
    let mixin = &application.mixin;
    if let Some(applied) = applied_clause
        .as_ref()
        .filter(|_| applies_a_shipped_mixin(heap, class))
    {
        let Some(&world_index) = context
            .shipped_by_mixin
            .get(&crate::mixin::mixin_key(mixin))
        else {
            return Err(format!(
                "image encoding cannot serialize a level that `<:` clause {} of class `{}` \
                 added: it applies the shipped mixin {}, which no shipped name names",
                applied.index + 1,
                applied.declaration.name,
                mixin.name
            ));
        };
        let entry = &context.world.entries()[world_index];
        if !NamePosition::Superclass.allows(entry.positions) {
            return Err(not_allowed(entry, NamePosition::Superclass));
        }
        writer.write_u8(MIXIN_SHIPPED);
        writer.write_u32(context.table.used.index_of(world_index));
        encode_optional_class(
            writer,
            superclass.as_ref(),
            NamePosition::Superclass,
            context,
        )?;
        writer.write_u32(applied_row(applied)?);
        writer.write_u32(u32_field("mixin application clause index", applied.index));
        return Ok(());
    }
    let id = context
        .mixin_ids
        .get(&crate::mixin::mixin_key(mixin))
        .copied()
        .ok_or_else(|| {
            format!(
                "image encoding cannot serialize a class object applying mixin `{}`, \
                 which is not part of any module the image carries",
                mixin.name
            )
        })?;
    writer.write_u8(MIXIN_ROW);
    writer.write_u32(id);
    encode_optional_class(
        writer,
        superclass.as_ref(),
        NamePosition::Superclass,
        context,
    )?;
    // The enclosing object is an ordinary object, not a class, so it is
    // written the way every other object reference is -- one `Value`.
    match heap.class_enclosing(class) {
        Some(enclosing) => {
            writer.write_boolean(true);
            encode_value(
                writer,
                Value::object(enclosing),
                NamePosition::Value,
                context,
            )?;
        }
        None => writer.write_boolean(false),
    }
    encode_optional_class(
        writer,
        heap.class_declaring(class).as_ref(),
        NamePosition::FrameClass,
        context,
    )?;
    match &applied_clause {
        Some(applied) => {
            writer.write_boolean(true);
            writer.write_u32(applied_row(applied)?);
            writer.write_u32(u32_field("mixin application clause index", applied.index));
        }
        None => writer.write_boolean(false),
    }
    Ok(())
}

fn encode_object<'gc>(
    writer: &mut Writer,
    object: ObjectReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let heap = context.heap;
    let class = heap.object_class(object);
    let shape = heap.object_shape(object);
    // A class object -- never a metaclass, which always has one -- whose
    // header is not its metaclass has none: either it never got one
    // (`Mutator::new_class_object`, before the kernel phase's
    // `metaclasses_ready`) or something else is wrong with its pairing.
    // Writing the placeholder header anyway would silently claim it as the
    // metaclass every class record's header names (`encode_object`'s next
    // line), so this is refused up front rather than left for the loader
    // to reject an image the encoder should never have written.
    if shape == ObjectShape::Class
        && heap.class_this_class(object).is_none()
        && heap.class_metaclass(object).is_none()
    {
        return Err(
            "image encoding cannot serialize a class object without a metaclass".to_string(),
        );
    }
    // Every header is the object's class: a class record's names its
    // metaclass, and a metaclass record's names `Metaclass` (§3.2).
    encode_class_tag(writer, class, NamePosition::Header, context)?;
    writer.write_u32(heap.object_identity_hash(object));
    match shape {
        // An unresolved future -- a resolved one is never a record -- is
        // written unresolved even while its block runs
        // (`future::fields_for_an_image`). One whose fields crafted code
        // corrupted is refused: the decoder would refuse it.
        ObjectShape::Instance
            if heap.class_builtin(class) == Some(BuiltinClass::FutureImplementation) =>
        {
            let Some(fields) = future::fields_for_an_image(heap, object) else {
                return Err("image encoding cannot serialize a malformed future".to_string());
            };
            writer.write_u8(OBJECT_INSTANCE);
            encode_values(writer, &fields, NamePosition::Value, context)?;
        }
        ObjectShape::Instance => {
            writer.write_u8(OBJECT_INSTANCE);
            let fields = heap.instance_fields(object).unwrap_or_default();
            encode_values(writer, &fields, NamePosition::Value, context)?;
        }
        ObjectShape::String => {
            writer.write_u8(OBJECT_STRING);
            let Some(string) = heap.string_code_points(object) else {
                unreachable!("internal: an `ObjectShape::String` has code points")
            };
            // The wire keeps UTF-8 (`docs/design/image-format.md`); the
            // width is the heap's, chosen afresh when the record is decoded.
            let text = string.to_utf8();
            u32_length("string", text.len())?;
            writer.write_str(&text);
        }
        ObjectShape::Symbol => {
            writer.write_u8(OBJECT_SYMBOL);
            let Some(symbol) = heap.symbol_value(object) else {
                unreachable!("internal: an `ObjectShape::Symbol` has a symbol")
            };
            // A program can make a symbol of any string (`asSymbol`); the
            // symbol table writes its text with a `u32` length.
            u32_length("symbol", symbol.as_str().len())?;
            writer.write_u32(symbols.intern(symbol.as_str()));
        }
        ObjectShape::ByteArray => {
            writer.write_u8(OBJECT_BYTE_ARRAY);
            let Some(bytes) = heap.byte_array_bytes(object) else {
                unreachable!("internal: an `ObjectShape::ByteArray` has bytes")
            };
            // Only the bytes: the reserve ahead of them is the heap's
            // alignment, chosen afresh where the record is decoded.
            u32_length("byte array", bytes.len())?;
            writer.write_bytes(&bytes);
        }
        ObjectShape::Array => {
            writer.write_u8(OBJECT_ARRAY);
            let elements = heap.array_elements(object).unwrap_or_default();
            encode_values(writer, &elements, NamePosition::Value, context)?;
        }
        ObjectShape::Closure => {
            writer.write_u8(OBJECT_CLOSURE);
            encode_closure_record(writer, object, context)?;
        }
        ObjectShape::Class => match heap.class_this_class(object) {
            // Its class is always a class record: a shipped metaclass is a
            // shipped name and never a record, so a shipped name here is
            // refused as a record's class would be.
            Some(this_class) => {
                writer.write_u8(OBJECT_METACLASS);
                encode_class_tag(writer, this_class, NamePosition::Header, context)?;
            }
            None => {
                writer.write_u8(OBJECT_CLASS);
                encode_class_record(writer, object, context)?;
            }
        },
        ObjectShape::Activation => {
            writer.write_u8(OBJECT_FRAME);
            let Some(frame) = heap.object_frame(object) else {
                unreachable!("internal: an `ObjectShape::Activation` is a frame object")
            };
            encode_frame(writer, frame, context, symbols)?;
        }
        // No object record carries one: the graph walk gives a
        // `LargeInteger` no object id (`GraphTable::visit_value`), and
        // `encode_value` writes its digits instead, so that an image cannot
        // give two equal integers two identities.
        ObjectShape::LargeInteger => {
            unreachable!("a LargeInteger is written as a value, never as an object record")
        }
        // Likewise a boxed `Float`, whose bits `encode_value` writes.
        ObjectShape::Float => {
            unreachable!("a boxed Float is written as a value, never as an object record")
        }
        // Written as a collection would leave it: a slot whose object
        // nothing else reaches is `nil` (`GraphTable::weakly_held`).
        ObjectShape::WeakArray => {
            writer.write_u8(OBJECT_WEAK_ARRAY);
            let elements: Vec<Value<'gc>> = heap
                .weak_array_elements(object)
                .unwrap_or_default()
                .into_iter()
                .map(|element| {
                    context
                        .table
                        .weakly_held(heap, context.shipped_by_object, element)
                })
                .collect();
            encode_values(writer, &elements, NamePosition::Value, context)?;
        }
        // An ephemeron whose key the walk never reached is mourned, every
        // field `nil`; any other has its fields, which the walk traced.
        ObjectShape::Ephemeron => {
            writer.write_u8(OBJECT_EPHEMERON);
            let mut fields = heap.instance_fields(object).unwrap_or_default();
            if context
                .table
                .mourned
                .contains(&heap.object_identity_key(object))
            {
                fields.fill(Value::nil());
            }
            encode_values(writer, &fields, NamePosition::Value, context)?;
        }
    }
    Ok(())
}

/// An `OBJECT_FRAME` body (`opcodes::OBJECT_FRAME` gives the layout). The
/// kind comes first and the register count is written, so that the
/// decoder's first pass sizes the record from its own bytes. Every register
/// the frame object has is written, `nil` where nothing was copied into it:
/// a frame record's count is its size, like any other record's.
fn encode_frame<'gc>(
    writer: &mut Writer,
    frame: FrameReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let snapshot = context.heap.frame_record(frame);
    let frame_class = NamePosition::FrameClass;
    match &snapshot.kind {
        FrameKind::Method(method) => {
            writer.write_u8(FRAME_METHOD);
            writer.write_u32(symbols.intern(method.selector.as_str()));
        }
        FrameKind::Closure(closure) => {
            writer.write_u8(FRAME_CLOSURE);
            writer.write_u32(context.table.object_id(context.heap, *closure));
        }
        FrameKind::TopLevel => writer.write_u8(FRAME_TOP_LEVEL),
        FrameKind::Initializer(class) => {
            writer.write_u8(FRAME_INITIALIZER);
            encode_class_tag(writer, *class, frame_class, context)?;
        }
        FrameKind::SuperclassClause(class) => {
            writer.write_u8(FRAME_SUPERCLASS_CLAUSE);
            encode_class_tag(writer, *class, frame_class, context)?;
        }
        FrameKind::SuperclassMessage(class) => {
            writer.write_u8(FRAME_SUPERCLASS_MESSAGE);
            encode_class_tag(writer, *class, frame_class, context)?;
        }
        FrameKind::MixinApplicationMessage(level) => {
            writer.write_u8(FRAME_MIXIN_APPLICATION_MESSAGE);
            encode_class_tag(writer, *level, frame_class, context)?;
        }
    }
    // An image's code table holds only compiled bodies, so a frame's
    // `program_counter` is always an instruction index and the record does not
    // say so.
    writer.write_u32(u32_field("frame program counter", snapshot.program_counter));
    writer.write_boolean(snapshot.uncontinuable);
    encode_value(
        writer,
        snapshot.receiver,
        NamePosition::FrameRegister,
        context,
    )?;
    encode_frame_reference(
        context.heap,
        writer,
        snapshot.sender.as_ref(),
        context.table,
    );
    encode_frame_reference(context.heap, writer, snapshot.home.as_ref(), context.table);
    encode_optional_class(
        writer,
        snapshot.current_class.as_ref(),
        frame_class,
        context,
    )?;
    let registers: Vec<Value<'gc>> = snapshot
        .arguments
        .into_iter()
        .chain(snapshot.temporaries)
        .collect();
    encode_values(writer, &registers, NamePosition::FrameRegister, context)
}

/// What `encode_graph_section` wrote besides its bytes: the shipped names
/// the section uses, and the object id of every carried module's root
/// class record, in module order, for the module section to name.
pub struct EncodedGraph {
    pub used: UsedNames,
    pub module_roots: Vec<u32>,
}

/// The object table, frames included, and the roots. The count comes first;
/// the decoder sizes every record and allocates it before filling any, so
/// that records may reference each other freely.
///
/// Every carried module's root class (`module_roots`, in module order) is a
/// record of the table, walked after the roots (`GraphTable::build`), and
/// is held to its module's declaration and the root rule as the loader
/// holds it (`modules::root_record_defect`), so no image is written that
/// the loader refuses for its roots.
///
/// Answers the shipped names the section uses, which the caller writes
/// ahead of it in the shipped-name section (`shipped_names::encode_section`):
/// an object of the shipped world is named by its index there, not written.
#[allow(clippy::too_many_arguments)]
pub fn encode_graph_section<'gc>(
    mutator: &Mutator<'gc>,
    writer: &mut Writer,
    roots: &[Value<'gc>],
    code_ids: &CodeIds,
    module_roots: &[ClassReference<'gc>],
    mixin_ids: &HashMap<usize, u32>,
    symbols: &mut symbols::Builder,
) -> Result<EncodedGraph, String> {
    let heap = mutator.heap();
    let world = &mutator.shipped_world;
    for class in module_roots {
        let Some(application) = heap.class_application(*class) else {
            return Err("image encoding's root object is not a class".to_string());
        };
        match super::modules::root_record_defect(mutator, &application.mixin, *class) {
            None => {}
            Some(super::modules::RootRecordDefect::NotItsRecord) => {
                return Err(format!(
                    "image encoding cannot serialize {} as a module's root class: it is a \
                     metaclass, a level, a class applying another declaration's mixin, or a \
                     class with an enclosing object or a declaring class, such as a nested \
                     class",
                    heap.class_display_name(*class)
                ))
            }
            Some(super::modules::RootRecordDefect::RootRule) => {
                return Err(format!(
                    "image encoding cannot serialize {} as a module's root class: it does not \
                     inherit what the root rule allows, Object or Value, or Value applied over \
                     Object",
                    heap.class_display_name(*class)
                ))
            }
        }
    }
    let shipped_by_object = world.index_by_object(heap);
    let shipped_by_mixin = world.index_by_mixin(heap);
    let named = Named {
        by_object: &shipped_by_object,
        by_mixin: &shipped_by_mixin,
        mirror: mutator.vm_mirror.and_then(|mirror| mirror.as_object()),
    };
    let table = GraphTable::build(heap, roots, module_roots, &named);
    // The VM mirror is a capability the bootstrap hands to the core library
    // alone (`docs/design/core-library.md`, "The VM mirror is the whole Rust
    // boundary"); an artifact that carried it would hand it to whatever
    // loaded it. Checked ahead of every other refusal, so that a graph
    // reaching the mirror is refused as the capability it is.
    if table.reached_mirror {
        return Err(
            "image encoding refuses the VM mirror: it is a capability the \
             bootstrap hands to the core library alone, and an artifact that \
             carried it would hand it to whatever loaded it"
                .to_string(),
        );
    }
    if let Some(refusal) = &table.unnamed {
        return Err(refusal.clone());
    }
    // A shipped class is a shipped name, never a record, so it cannot be a
    // module's root.
    let module_root_ids = module_roots
        .iter()
        .map(|class| {
            table
                .object_ids
                .get(&heap.object_identity_key(*class))
                .copied()
                .ok_or_else(|| {
                    format!(
                        "image encoding cannot serialize {} as a module's root class: it is \
                         shipped",
                        heap.class_display_name(*class)
                    )
                })
        })
        .collect::<Result<Vec<u32>, String>>()?;
    let context = EncodeContext {
        table: &table,
        code_ids,
        world,
        shipped_by_object: &shipped_by_object,
        shipped_by_mixin: &shipped_by_mixin,
        mixin_ids,
        heap,
    };
    writer.write_u32(u32_field("object table entry count", table.objects().len()));
    for object in table.objects() {
        encode_object(writer, *object, &context, symbols)?;
    }
    encode_values(writer, roots, NamePosition::Value, &context)?;
    Ok(EncodedGraph {
        used: table.used,
        module_roots: module_root_ids,
    })
}

/// The graph section's closing table: the identity hash of every object of
/// the shipped world, which the image names rather than carries as a
/// record, so that a hash the program was given survives the load. Only an
/// assigned hash (not 0) is written, in the shipped world's own order, so
/// one world encodes to one byte sequence. Every object of the shipped
/// world is considered, not only those the graph reaches: a program can
/// keep `Integer hash` in a slot without keeping `Integer`. A shipped
/// object the graph did not name is added to `used`, so the shipped-name
/// section lists it. A record's hash, a module root class's among them, is
/// in its header. See `docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim".
pub fn encode_tag_named_hashes<'gc>(
    mutator: &Mutator<'gc>,
    writer: &mut Writer,
    used: &mut UsedNames,
) {
    let heap = mutator.heap();
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for (world_index, entry) in mutator.shipped_world.entries().iter().enumerate() {
        let hash = heap.object_identity_hash(entry.object);
        if hash != 0 {
            entries.push((used.use_name(world_index), hash));
        }
    }
    writer.write_u32(u32_field("tag-named hash count", entries.len()));
    for (index, hash) in entries {
        writer.write_u8(HASHED_SHIPPED);
        writer.write_u32(index);
        writer.write_u32(hash);
    }
}
