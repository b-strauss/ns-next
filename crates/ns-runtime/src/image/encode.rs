//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- see `docs/design/image-format.md`. Never a memory dump.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bytecode::{
    BodyLimit, CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal, Unbuilt,
};
use crate::heap::{Heap, MixinId, ObjectShape};
use crate::interpreter::future;
use crate::interpreter::shipped_world::{ShippedEntry, ShippedWorld};
use crate::types::{
    BuiltinClass, ClassReference, FrameKind, FrameReference, ObjectReference, Value, ValueKind,
};
use crate::{ArbitraryInteger, Mutator};

use super::opcodes::*;
use super::shipped_names::{NamePosition, ShippedKind, ShippedName};
use super::{mixins, symbols};

#[cfg(test)]
mod wire_tests;

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
/// *within* the format: the program's source text is optional, and a reader
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

/// Header flag bits. Every bit this build has not assigned a meaning to is
/// reserved and must be zero, so a reader of this build refuses an image
/// using a flag it does not implement rather than skipping a section it
/// cannot see.
///
/// The image carries its program's source: a table of every declaration's
/// own text, and per mixin row the index of its text
/// (`docs/design/image-format.md`, "Source").
pub const FLAG_HAS_SOURCE: u8 = 8;

/// The image is an application packaged just before `main:args:`: `load`
/// starts it by sending `main: platform args:` to the graph's one root
/// (`docs/design/image-format.md`).
pub const FLAG_START: u8 = 4;

/// Every bit this build assigns a meaning to.
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_START;

/// Flags that once meant something and are never reused: 2 was
/// `FLAG_STATELESS`, the stateless binary, retired 2026-09-29; 1 was the
/// source variant that carried one text per module, retired 2026-10-19
/// when every declaration came to hold its own, so that an image of the
/// old layout is refused rather than misread.
pub const RETIRED_HEADER_FLAGS: u8 = 2 | 1;

pub fn write_header(writer: &mut Writer, flags: u8) {
    writer.write_raw(MAGIC);
    writer.write_u32(FORMAT_VERSION);
    writer.write_u8(flags);
    writer.write_u64(super::fingerprint::shipped_world_fingerprint());
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
    u32::try_from(value).expect("internal: the assertion above refused a value past u32")
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

/// A closure's parameter count, a method's or factory's arity, narrowed to
/// the wire's `u16` field (`u16_field`) and refused past
/// `bytecode::ARGUMENT_LIMIT`: the body compiled to one that raises
/// (`docs/design/compiler.md`, "Limits"), and the decoder refuses it, so
/// the write is refused with an error, and `build` refuses what `load`
/// would.
pub(crate) fn arity_field(what: &str, value: usize) -> Result<u16, String> {
    let arity = u16_field(what, value)?;
    if arity > crate::bytecode::ARGUMENT_LIMIT {
        return Err(format!(
            "image encoding cannot write a {what} of {value}: it is past the limit of {} arguments",
            crate::bytecode::ARGUMENT_LIMIT
        ));
    }
    Ok(arity)
}

/// `Rc<CompiledCode>` pointer -> code-table id, assigned by
/// `encode_code_table`; `encode_closure_record` uses it for a
/// closure's code reference.
#[derive(Default)]
pub struct CodeIds {
    ids: HashMap<usize, u32>,
}

impl CodeIds {
    /// A body no version of a declaration the image carries compiled has no
    /// id: a graph that reaches a closure over another module's code. An
    /// `Err` here, not a panic, because what the graph reaches is the
    /// program's to decide.
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
/// declaration order is not a wire format. The kind opens a code record,
/// so a record of a retired layout meets a retired tag
/// (`opcodes::RETIRED_CODE_KIND_TAGS`).
pub fn code_kind_tag(kind: CodeKind) -> u8 {
    match kind {
        CodeKind::Method => 5,
        CodeKind::Block => 6,
        CodeKind::Initializer => 7,
        CodeKind::SuperclassClause => 8,
    }
}

/// The code table: every `CompiledCode` reachable from `rows`
/// (`mixins::plan`) -- every mixin's methods, every mixin's initializer,
/// every superclass clause body, and every nested block body -- then every body of
/// `extra`, the code of an older version the graph still runs, and the
/// blocks nested in it, with nested block code *before* the code that
/// creates it. Flat and unnamed: *which* body belongs to
/// which declaration is the mixin table's business (`image::mixins`),
/// which addresses this table by id.
///
/// Collected in row order -- the order the mixin table's rows are written
/// in -- so the same module always encodes to the same bytes.
pub fn encode_code_table(
    writer: &mut Writer,
    rows: &[mixins::Row],
    extra: &[Rc<CompiledCode>],
    symbols: &mut symbols::Builder,
) -> Result<CodeIds, String> {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds::default();
    for row in rows {
        for method in &row.methods {
            collect_code(&method.code, &mut order, &mut ids);
        }
        for code in [
            &row.mixin.initializer_code,
            &row.mixin.own_initializer_code,
            &row.mixin.standalone_initializer_code,
            &row.mixin.superclass_clause_code,
        ]
        .into_iter()
        .flatten()
        {
            collect_code(code, &mut order, &mut ids);
        }
    }
    for code in extra {
        collect_code(code, &mut order, &mut ids);
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
    // Refused past the argument limit as the decoder refuses it
    // (`decode::check_argument_count`), so that `build` refuses what `load`
    // would. A body past the register ceiling saturates its arity at
    // `u16::MAX`, which this refuses too, naming it as the least count it
    // stands for.
    if code.argument_count == u16::MAX {
        return Err(format!(
            "image encoding cannot write a body arity of {} or more: it is past the limit of {} arguments",
            u16::MAX,
            crate::bytecode::ARGUMENT_LIMIT
        ));
    }
    writer.write_u16(arity_field("body arity", usize::from(code.argument_count))?);
    writer.write_u16(code.register_count);
    // `outer_reach` is deliberately absent: it follows from the
    // instructions, and `decode_code` recomputes it as the compiler did,
    // through `CompiledCode::from_builder`.
    writer.write_u16(code.copied_count);
    writer.write_u16(code.local_count);
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
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_LEXICAL);
            writer.write_u16(*depth);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
            writer.write_u16(*destination);
        }
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_PUBLIC);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*receiver_register);
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
            writer.write_u16(*destination);
        }
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_INLINABLE);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*receiver_register);
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
            writer.write_u16(*destination);
        }
        Instruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            destination,
            ..
        } => {
            writer.write_u8(OPCODE_SEND_SUPER);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
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
            writer.write_u16(arity_field("closure parameter count", parameters.len())?);
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
        Instruction::AllocateInstance { destination } => {
            writer.write_u8(OPCODE_ALLOCATE_INSTANCE);
            writer.write_u16(*destination);
        }
        Instruction::InitializeInstance {
            instance,
            destination,
        } => {
            writer.write_u8(OPCODE_INITIALIZE_INSTANCE);
            writer.write_u16(*instance);
            writer.write_u16(*destination);
        }
        Instruction::CompleteInstance { instance } => {
            writer.write_u8(OPCODE_COMPLETE_INSTANCE);
            writer.write_u16(*instance);
        }
        Instruction::CheckAppliedClause { steps, selector } => {
            writer.write_u8(OPCODE_CHECK_APPLIED_CLAUSE);
            writer.write_u16(*steps);
            writer.write_u32(symbols.intern(selector.as_str()));
        }
        Instruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            destination,
        } => {
            writer.write_u8(OPCODE_INITIALIZE_SUPERCLASS);
            writer.write_u16(*steps);
            writer.write_u32(symbols.intern(selector.as_str()));
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
            writer.write_u16(*destination);
        }
        Instruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            destination,
        } => {
            writer.write_u8(OPCODE_INITIALIZE_APPLIED_LEVEL);
            writer.write_u16(*steps);
            writer.write_u16(*argument_base);
            encode_registers(writer, argument_sources)?;
            writer.write_u16(*destination);
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
        Literal::BodyTooLarge(BodyLimit::Registers) => writer.write_u8(LITERAL_BODY_TOO_LARGE),
        Literal::BodyTooLarge(BodyLimit::Arguments) => {
            writer.write_u8(LITERAL_BODY_PAST_THE_ARGUMENT_LIMIT)
        }
        Literal::BodyTooLarge(BodyLimit::Instructions) => {
            writer.write_u8(LITERAL_BODY_PAST_THE_INSTRUCTION_LIMIT)
        }
        Literal::Unbuilt(Unbuilt::AsynchronousSend(selector)) => {
            writer.write_u8(LITERAL_UNBUILT_ASYNCHRONOUS_SEND);
            writer.write_u32(symbols.intern(selector.as_str()));
        }
        Literal::Unbuilt(Unbuilt::PatternLiteral) => {
            writer.write_u8(LITERAL_UNBUILT_PATTERN_LITERAL)
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The object table (`docs/design/image-format.md`, "Sections").
// ---------------------------------------------------------------------------

/// A class object's graph references, in the order its record writes them:
/// the superclass, the enclosing object, the declaring class, and for a
/// level a `<:` clause added the class its clause named, then for a
/// body-less declaration's class object the declaration's enclosing object
/// and declaring class (`OBJECT_BODY_LESS_CLASS`). Traversal and
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
) -> impl Iterator<Item = ObjectReference<'gc>> + use<'gc> {
    let own_context = !applies_a_shipped_mixin(heap, class);
    let (enclosing, declaring_class) = if own_context {
        (heap.class_enclosing(class), heap.class_declaring(class))
    } else {
        (None, None)
    };
    let scope = heap.class_declaration_scope(class);
    heap.class_superclass(class)
        .into_iter()
        .chain(enclosing)
        .chain(declaring_class)
        .chain(heap.class_named(class))
        .chain(scope.and_then(|scope| scope.enclosing))
        .chain(scope.and_then(|scope| scope.declaring_class))
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

/// One shipped name a graph section uses: an entry of the shipped world,
/// by its position there, or the name of a class its module's root
/// declares that is not built yet (`ShippedWorld::declared_by_mixin`),
/// which only a reified mixin's record uses.
#[derive(Clone, Copy)]
enum UsedName {
    Built(usize),
    Declared(ShippedName),
}

/// The shipped names a graph section uses, in the order the graph first
/// used each -- the order the shipped-name section lists them in and its
/// references number them.
#[derive(Clone)]
pub struct UsedNames {
    names: Vec<UsedName>,
    index: HashMap<usize, u32>,
    /// The section index of each `UsedName::Declared`.
    declared: HashMap<ShippedName, u32>,
}

impl UsedNames {
    fn new() -> UsedNames {
        UsedNames {
            names: Vec::new(),
            index: HashMap::new(),
            declared: HashMap::new(),
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
        self.names.push(UsedName::Built(world_index));
        index
    }

    /// The section index of `name`, a class not built yet, listing it
    /// first if nothing has used it yet.
    fn use_declared(&mut self, name: ShippedName) -> u32 {
        if let Some(index) = self.declared.get(&name) {
            return *index;
        }
        let index = u32_field("shipped name index", self.names.len());
        self.declared.insert(name, index);
        self.names.push(UsedName::Declared(name));
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

    /// The section index of `name`, a class not built yet, which the graph
    /// walk listed.
    fn index_of_declared(&self, name: &ShippedName) -> u32 {
        *self
            .declared
            .get(name)
            .expect("internal: the graph walk listed every shipped name the encoder writes")
    }

    /// The names, in section order: what a host that decodes a graph
    /// section it wrote by hand resolves its `DecodeContext::shipped` from
    /// (`shipped_names::resolve_in`).
    pub fn names(&self, mutator: &Mutator<'_>) -> Vec<ShippedName> {
        let entries = mutator.shipped_world.entries();
        self.names
            .iter()
            .map(|used| match used {
                UsedName::Built(index) => entries[*index].name,
                UsedName::Declared(name) => *name,
            })
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
/// by mixin (`ShippedWorld::index_by_mixin`), the names of the shipped
/// classes not built yet by mixin (`ShippedWorld::declared_by_mixin`), the
/// VM mirror, and the id of its class's mixin.
struct Named<'a, 'gc> {
    by_object: &'a HashMap<usize, usize>,
    by_mixin: &'a HashMap<MixinId, usize>,
    declared_by_mixin: &'a HashMap<MixinId, ShippedName>,
    mirror: Option<ObjectReference<'gc>>,
    mirror_mixin: Option<MixinId>,
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
                    let fields = heap.ephemeron_fields(ephemeron).unwrap_or_default();
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
                                .and_then(|application| heap.known_mixin_id(&application.mixin));
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
                // Its field is not followed: the record names the mixin,
                // whose shipped name, if it has one, the walk lists here.
                // The VM mirror's class has no declaration, so its mixin
                // is refused as the class is (lock (5)).
                ObjectShape::Mixin => {
                    if let Ok((instance_side, class_side)) = reified_mixin_reference(heap, object) {
                        let key = heap.known_mixin_id(&instance_side);
                        if key.is_some() && named.mirror_mixin == key {
                            if self.unnamed.is_none() {
                                self.unnamed = Some(format!(
                                    "image encoding refuses the mixin of the VM mirror's \
                                     class{}: it has no declaration, so no shipped name names it",
                                    if class_side { " class" } else { "" }
                                ));
                            }
                        } else if let Some(world_index) =
                            key.and_then(|key| named.by_mixin.get(&key))
                        {
                            self.used.use_name(*world_index);
                        } else if let Some(name) =
                            key.and_then(|key| named.declared_by_mixin.get(&key))
                        {
                            self.used.use_declared(*name);
                        }
                    }
                }
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
                    let fields = heap.ephemeron_fields(object).unwrap_or_default();
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
) -> impl Iterator<Item = Value<'gc>> + use<'gc> {
    let snapshot = heap.frame_record(frame);
    let kind_reference = match &snapshot.kind {
        // The closure object a closure activation runs is a record of its
        // own, so it needs an id like any other object.
        FrameKind::Closure(closure) => Some(*closure),
        // The class-carrying kinds name a class the same way `current_class`
        // does, so the class needs an id the same way.
        FrameKind::Initializer(class, _)
        | FrameKind::Factory(class)
        | FrameKind::SuperclassClause(class) => Some(*class),
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
    shipped_by_mixin: &'a HashMap<MixinId, usize>,
    /// `ShippedWorld::declared_by_mixin`, built for this encoding.
    shipped_declared_by_mixin: &'a HashMap<MixinId, ShippedName>,
    /// A declaration's id -> mixin-table row id, as `mixins::encode`
    /// assigned them.
    pub mixin_ids: &'a mixins::MixinIds,
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
    if let Some((world_index, entry)) = context.shipped(object)
        && entry.name.kind == ShippedKind::ModuleInstance
    {
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
///
/// Either shape of a level ends with the class its clause named
/// (`encode_named_class`).
fn encode_class_record<'gc>(
    writer: &mut Writer,
    class: ClassReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let heap = context.heap;
    let applied_clause = heap.class_applied_origin(class);
    let superclass = heap.class_superclass(class);
    let applied_row = |applied: &crate::mixin::AppliedClause| {
        heap.known_mixin_id(&applied.declaration)
            .and_then(|id| context.mixin_ids.get(&id))
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
        let Some(&world_index) = heap
            .known_mixin_id(mixin)
            .and_then(|id| context.shipped_by_mixin.get(&id))
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
        return encode_named_class(writer, class, context);
    }
    let id = heap
        .known_mixin_id(mixin)
        .and_then(|id| context.mixin_ids.get(&id))
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
            encode_named_class(writer, class, context)
        }
        None => {
            writer.write_boolean(false);
            Ok(())
        }
    }
}

/// The class the `<:` clause that added the level `class` named
/// (`Heap::class_named`), after the clause: the decoder holds the level's
/// §6.1 check to its primary factory and rebuilds the level's metaclass
/// from its class side, since a body-less named class's is no class side
/// of the mixin the level applies (`object-model.md`, "Mixin application").
fn encode_named_class<'gc>(
    writer: &mut Writer,
    class: ClassReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let Some(named) = context.heap.class_named(class) else {
        unreachable!("internal: every level a `<:` clause added records the class it named")
    };
    encode_class_tag(writer, named, NamePosition::Superclass, context)
}

/// A body-less declaration's scope (`Heap::class_declaration_scope`), after
/// its class object's record: the enclosing object as one `Value` and the
/// declaring class as a class tag, each behind a presence flag, as the
/// record's own two are written.
fn encode_declaration_scope<'gc>(
    writer: &mut Writer,
    scope: crate::heap::DeclarationScope<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    match scope.enclosing {
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
        scope.declaring_class.as_ref(),
        NamePosition::FrameClass,
        context,
    )
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
            // The object's own text, whether its text is interned or the
            // program made it and it has no number: the wire carries a
            // symbol's text, never a number.
            let Some(text) = heap.symbol_text(object) else {
                unreachable!("internal: an `ObjectShape::Symbol` has a text")
            };
            // A program can make a symbol of any string (`asSymbol`); the
            // symbol table writes its text with a `u32` length.
            u32_length("symbol", text.len())?;
            writer.write_u32(symbols.intern(&text));
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
                write_class_stamp(writer, heap, object);
                encode_class_tag(writer, this_class, NamePosition::Header, context)?;
            }
            // A body-less declaration's class object carries its
            // declaration's scope after the record a level has.
            None => match heap.class_body_less_declaration(object) {
                Some(_) => {
                    let Some(scope) = heap.class_declaration_scope(object) else {
                        return Err("image encoding cannot serialize a body-less \
                                    declaration's class object without its declaration's scope"
                            .to_string());
                    };
                    writer.write_u8(OBJECT_BODY_LESS_CLASS);
                    write_class_stamp(writer, heap, object);
                    encode_class_record(writer, object, context)?;
                    encode_declaration_scope(writer, scope, context)?;
                }
                None => {
                    writer.write_u8(OBJECT_CLASS);
                    write_class_stamp(writer, heap, object);
                    encode_class_record(writer, object, context)?;
                }
            },
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
            let mut fields = heap.ephemeron_fields(object).unwrap_or_default();
            if context
                .table
                .mourned
                .contains(&heap.object_identity_key(object))
            {
                fields.fill(Value::nil());
            }
            encode_values(writer, &fields, NamePosition::Value, context)?;
        }
        ObjectShape::Mixin => {
            writer.write_u8(OBJECT_MIXIN);
            encode_mixin_record(writer, object, context)?;
        }
    }
    Ok(())
}

/// A class record's creation stamp, right after its body tag: an image of
/// a running world keeps the order in which its class objects were made
/// (`Heap::class_creation_stamp`).
fn write_class_stamp<'gc>(writer: &mut Writer, heap: &Heap<'gc>, class: ObjectReference<'gc>) {
    let stamp = heap
        .class_creation_stamp(class)
        .expect("internal: a class record is written for a class object");
    writer.write_u64(stamp);
}

/// The mixin whose reference an `OBJECT_MIXIN` record of the reified mixin
/// `object` writes -- its instance side -- and whether `object` stands for
/// that mixin's class side. A class side, declared or synthesized, names
/// its instance side as its enclosing mixin, so it is written through it,
/// and each mixin has one spelling.
fn reified_mixin_reference<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
) -> Result<(Rc<crate::mixin::Mixin>, bool), String> {
    let Some((id, mixin)) = heap
        .mixin_object_id(object)
        .and_then(|id| Some((id, heap.mixin(id)?)))
    else {
        unreachable!("internal: an `ObjectShape::Mixin` stands for a mixin of the table")
    };
    if !mixin.is_class_side() {
        return Ok((mixin, false));
    }
    match heap
        .enclosing_mixin_id(id)
        .and_then(|instance| heap.mixin(instance))
    {
        Some(instance_side) => Ok((instance_side, true)),
        None => Err(
            "image encoding cannot serialize the reified mixin of a class side whose \
             instance side no longer exists"
                .to_string(),
        ),
    }
}

/// An `OBJECT_MIXIN` body (`opcodes::OBJECT_MIXIN` gives the layout): the
/// instance side's reference, as `encode_class_record` names a class
/// object's mixin -- a row of the mixin table, or the shipped name of the
/// class whose mixin it is -- then the side. The id the object holds is
/// not written: the loader mints its own.
fn encode_mixin_record<'gc>(
    writer: &mut Writer,
    object: ObjectReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let (instance_side, class_side) = reified_mixin_reference(context.heap, object)?;
    let id = context.heap.known_mixin_id(&instance_side);
    if let Some(row) = id.and_then(|id| context.mixin_ids.get(&id)) {
        writer.write_u8(MIXIN_ROW);
        writer.write_u32(*row);
    } else if let Some(&world_index) = id.and_then(|id| context.shipped_by_mixin.get(&id)) {
        let entry = &context.world.entries()[world_index];
        if !NamePosition::Mixin.allows(entry.positions) {
            return Err(not_allowed(entry, NamePosition::Mixin));
        }
        writer.write_u8(MIXIN_SHIPPED);
        writer.write_u32(context.table.used.index_of(world_index));
    } else if let Some(name) = id.and_then(|id| context.shipped_declared_by_mixin.get(&id)) {
        // A class not built yet has no entry and so no positions, but a
        // reified mixin's mixin may be every shipped class's, whatever its
        // accessibility (`shipped_world::class_positions`), so no position
        // refuses it; the loader builds the class before it resolves it.
        writer.write_u8(MIXIN_SHIPPED);
        writer.write_u32(context.table.used.index_of_declared(name));
    } else if instance_side.shipped {
        return Err(format!(
            "image encoding refuses the reified mixin {}: it is compiled from shipped source, \
             and no shipped name names it",
            instance_side.name
        ));
    } else {
        return Err(format!(
            "image encoding cannot serialize the reified mixin {}, which is not part of any \
             module the image carries",
            instance_side.name
        ));
    }
    writer.write_u8(if class_side {
        MIXIN_SIDE_CLASS
    } else {
        MIXIN_SIDE_INSTANCE
    });
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
    // A run leaves no suspended frame behind, and the decoder refuses one:
    // its registers and program counter would be resumable.
    debug_assert!(
        !context.heap.frame_is_suspended(frame),
        "a suspended frame cannot be written into an image"
    );
    let snapshot = context.heap.frame_record(frame);
    let frame_class = NamePosition::FrameClass;
    match &snapshot.kind {
        FrameKind::Method(method) => {
            writer.write_u8(FRAME_METHOD);
            writer.write_u32(symbols.intern(method.selector.as_str()));
            writer.write_u8(mixins::access_tag(method.access));
        }
        FrameKind::Closure(closure) => {
            writer.write_u8(FRAME_CLOSURE);
            writer.write_u32(context.table.object_id(context.heap, *closure));
        }
        FrameKind::TopLevel => writer.write_u8(FRAME_TOP_LEVEL),
        FrameKind::Initializer(class, body) => {
            writer.write_u8(body.frame_tag());
            encode_class_tag(writer, *class, frame_class, context)?;
        }
        FrameKind::Factory(class) => {
            writer.write_u8(FRAME_FACTORY);
            encode_class_tag(writer, *class, frame_class, context)?;
        }
        FrameKind::SuperclassClause(class) => {
            writer.write_u8(FRAME_SUPERCLASS_CLAUSE);
            encode_class_tag(writer, *class, frame_class, context)?;
        }
    }
    encode_frame_body(writer, frame, &snapshot, context)?;
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

/// The body that runs in `frame` (`opcodes::FRAME_BODY_*`). A program's
/// body by code id, never left to the class: after an edit the class's
/// current version carries another body than the one that ran. A shipped
/// declaration's body, which the code table never carries, by declaration:
/// a shipped mixin is never edited, so the decoder's lookup finds that very
/// body, which is checked here. A top-level frame runs none.
fn encode_frame_body<'gc>(
    writer: &mut Writer,
    frame: FrameReference<'gc>,
    snapshot: &crate::heap::FrameRecord<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let refusal = || {
        "image encoding cannot serialize a frame running code that no module the image \
         carries compiled"
            .to_string()
    };
    if matches!(snapshot.kind, FrameKind::TopLevel) {
        writer.write_u8(FRAME_BODY_NONE);
        return Ok(());
    }
    // A factory's body is derived from its class on the way in, as every
    // factory entry's is, so that no image carries one.
    if matches!(snapshot.kind, FrameKind::Factory(_)) {
        writer.write_u8(FRAME_BODY_DECLARED);
        return Ok(());
    }
    let code = context.heap.frame_code(frame).ok_or_else(refusal)?;
    let declared = super::decode::DeclaredKind::of_kind(&snapshot.kind, snapshot.current_class);
    if let Some(declared) = declared.filter(|declared| declared.is_shipped(context.heap)) {
        let found = super::decode::declared_body(context.heap, &declared).ok();
        if !found.is_some_and(|found| Rc::ptr_eq(&found, &code)) {
            return Err(refusal());
        }
        writer.write_u8(FRAME_BODY_DECLARED);
        return Ok(());
    }
    writer.write_u8(FRAME_BODY_CODE);
    writer.write_u32(context.code_ids.id_of(&code).map_err(|_| refusal())?);
    Ok(())
}

/// What `encode_graph_section` wrote besides its bytes: the shipped names
/// the section uses, and the object id of every carried module's root
/// class record, in module order, for the module section to name.
pub struct EncodedGraph {
    pub used: UsedNames,
    pub module_roots: Vec<u32>,
}

/// The graph as `walk_graph` found it, before anything of it is written:
/// the object table and the shipped names it reached, the shipped world's
/// indexes it was resolved against, and the object id of every carried
/// module's root class record. It holds handles, so it lives only within
/// the one entry that writes the image, which allocates nothing.
pub struct WalkedGraph<'gc> {
    table: GraphTable<'gc>,
    /// `ShippedWorld::index_by_object`, built for this encoding.
    shipped_by_object: HashMap<usize, usize>,
    /// `ShippedWorld::index_by_mixin`, built for this encoding.
    shipped_by_mixin: HashMap<MixinId, usize>,
    /// `ShippedWorld::declared_by_mixin`, built for this encoding.
    shipped_declared_by_mixin: HashMap<MixinId, ShippedName>,
    module_roots: Vec<u32>,
}

impl<'gc> WalkedGraph<'gc> {
    /// Every body the graph runs or can run: each closure's code and each
    /// frame's body, in object order. The code table must hold each of
    /// them, which for a body only a superseded version carries is what
    /// admits it there (`image::write_image`).
    pub fn bodies(&self, heap: &Heap<'gc>) -> Vec<Rc<CompiledCode>> {
        self.table
            .objects()
            .iter()
            .filter_map(|object| match heap.object_shape(*object) {
                ObjectShape::Closure => heap.closure_code(*object),
                // A factory frame's body is derived on the way in, never
                // carried (`encode_frame_body`).
                ObjectShape::Activation => heap
                    .object_frame(*object)
                    .filter(|frame| !matches!(heap.frame_kind(*frame), FrameKind::Factory(_)))
                    .and_then(|frame| heap.frame_code(frame)),
                _ => None,
            })
            .collect()
    }

    /// The id of every mixin a record of the graph names, in object order,
    /// a mixin as often as records name it: each class object's applied
    /// mixin, its declaration and the declaration whose `<:` clause added
    /// it, and each reified mixin's. A metaclass names nothing its class
    /// does not. What decides which obsolete trees an image carries
    /// (`image::write_image`).
    pub fn named_mixin_ids(&self, heap: &Heap<'gc>) -> Vec<MixinId> {
        let mut ids = Vec::new();
        for object in self.table.objects() {
            match heap.object_shape(*object) {
                ObjectShape::Class if heap.class_this_class(*object).is_none() => {
                    let applied = heap
                        .class_application(*object)
                        .map(|application| Rc::clone(&application.mixin));
                    let declaration = heap.class_declaration(*object);
                    let clause = heap
                        .class_applied_origin(*object)
                        .map(|origin| Rc::clone(&origin.declaration));
                    ids.extend(
                        [applied, declaration, clause]
                            .into_iter()
                            .flatten()
                            .filter_map(|mixin| heap.known_mixin_id(&mixin)),
                    );
                }
                ObjectShape::Mixin => ids.extend(heap.mixin_object_id(*object)),
                _ => {}
            }
        }
        ids
    }
}

/// Walks the graph from `roots` and the carried modules' root classes
/// (`module_roots`, in module order), making every refusal the graph
/// section makes before anything is written: a module root that is not
/// its module's record or breaks the root rule, the VM mirror, an unnamed
/// shipped class, and a shipped class as a module's root. Walked before
/// the code table is written, since the bodies the graph reaches decide
/// which code of an older version the table carries.
pub fn walk_graph<'gc>(
    mutator: &Mutator<'gc>,
    roots: &[Value<'gc>],
    module_roots: &[ClassReference<'gc>],
) -> Result<WalkedGraph<'gc>, String> {
    let heap = mutator.heap();
    let world = &mutator.shipped_world;
    for class in module_roots {
        // A root without a body is the level its clause added, which
        // applies `Value`'s mixin; its declaration is the clause's.
        let Some(root_mixin) = heap.class_declaration(*class) else {
            return Err("image encoding's root object is not a class".to_string());
        };
        match super::modules::root_record_defect(mutator, &root_mixin, *class) {
            None => {}
            Some(super::modules::RootRecordDefect::NotItsRecord) => {
                return Err(format!(
                    "image encoding cannot serialize {} as a module's root class: it is a \
                     metaclass, a level, a class applying another declaration's mixin, or a \
                     class with an enclosing object or a declaring class, such as a nested \
                     class",
                    heap.class_display_name(*class)
                ));
            }
            Some(super::modules::RootRecordDefect::RootRule) => {
                return Err(format!(
                    "image encoding cannot serialize {} as a module's root class: it does not \
                     inherit what the root rule allows, Object or Value, or Value applied over \
                     Object",
                    heap.class_display_name(*class)
                ));
            }
        }
    }
    let shipped_by_object = world.index_by_object(heap);
    let shipped_by_mixin = world.index_by_mixin(heap);
    let shipped_declared_by_mixin = world.declared_by_mixin(heap);
    let named = Named {
        by_object: &shipped_by_object,
        by_mixin: &shipped_by_mixin,
        declared_by_mixin: &shipped_declared_by_mixin,
        mirror: mutator.vm_mirror.and_then(|mirror| mirror.as_object()),
        mirror_mixin: heap
            .class_application(mutator.builtin_class(BuiltinClass::VmMirror))
            .and_then(|application| heap.known_mixin_id(&application.mixin)),
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
    Ok(WalkedGraph {
        table,
        shipped_by_object,
        shipped_by_mixin,
        shipped_declared_by_mixin,
        module_roots: module_root_ids,
    })
}

/// The object table, frames included, and the roots, as `walk_graph`
/// found them. The count comes first; the decoder sizes every record and
/// allocates it before filling any, so that records may reference each
/// other freely.
///
/// Every carried module's root class is a record of the table, walked
/// after the roots (`GraphTable::build`), and held to its module's
/// declaration and the root rule as the loader holds it
/// (`modules::root_record_defect`), so no image is written that the loader
/// refuses for its roots.
///
/// Answers the shipped names the section uses, which the caller writes
/// ahead of it in the shipped-name section (`shipped_names::encode_section`):
/// an object of the shipped world is named by its index there, not written.
pub fn encode_graph_section<'gc>(
    mutator: &Mutator<'gc>,
    writer: &mut Writer,
    walked: &WalkedGraph<'gc>,
    roots: &[Value<'gc>],
    code_ids: &CodeIds,
    mixin_ids: &mixins::MixinIds,
    symbols: &mut symbols::Builder,
) -> Result<EncodedGraph, String> {
    let heap = mutator.heap();
    let table = &walked.table;
    let context = EncodeContext {
        table,
        code_ids,
        world: &mutator.shipped_world,
        shipped_by_object: &walked.shipped_by_object,
        shipped_by_mixin: &walked.shipped_by_mixin,
        shipped_declared_by_mixin: &walked.shipped_declared_by_mixin,
        mixin_ids,
        heap,
    };
    writer.write_u32(u32_field("object table entry count", table.objects().len()));
    for object in table.objects() {
        encode_object(writer, *object, &context, symbols)?;
    }
    encode_values(writer, roots, NamePosition::Value, &context)?;
    Ok(EncodedGraph {
        used: table.used.clone(),
        module_roots: walked.module_roots.clone(),
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
