//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- see `docs/design/image-format.md`. Never a memory dump.

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::Heap;
use crate::types::{
    BuiltinClass, ClassData, ClassReference, ClosureData, FrameKind, FrameReference, ObjectData,
    ObjectReference, SourceOrigin, Value,
};
use crate::Mutator;

use super::opcodes::*;
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
/// *within* the format: the source blob is optional, and a reader
/// distinguishes the two variants by the flag. A flag bit this build does
/// not know is refused (`DecodeError::UnknownHeaderFlags`), which is what
/// keeps an optional section from being silently skipped by a reader that
/// cannot see it.
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

/// Header flag bits. Bit 0 says a source blob follows the header; every
/// bit this build has not assigned a meaning to is reserved and must be
/// zero, so a reader of this build refuses an image using a flag it does
/// not implement rather than skipping a section it cannot see.
pub const FLAG_HAS_SOURCE: u8 = 1;

/// The artifact carries no object graph: it is a deployment binary, not a
/// snapshot of a running world. Its entry field is mandatory rather than
/// optional, which is what makes it startable from the file alone.
pub const FLAG_STATELESS: u8 = 2;

/// Every bit this build assigns a meaning to.
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_STATELESS;

pub fn write_header(writer: &mut Writer, flags: u8) {
    writer.write_raw(MAGIC);
    writer.write_u32(FORMAT_VERSION);
    writer.write_u8(flags);
}

/// The module's provenance. Kept in every image, source blob or not: it is
/// what a loaded world reports as its own origin, and it is a path, not the
/// program text.
pub fn encode_source_origin(writer: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            writer.write_u8(0);
            writer.write_str(&path.to_string_lossy());
        }
    }
}

/// Encodes the four non-`Object` `Value` variants (tags 0-4); returns `false`
/// for `Value::Object`, which `encode_value` (the object-graph encoder)
/// handles.
pub fn encode_scalar_value<'gc>(writer: &mut Writer, value: &Value<'gc>) -> bool {
    match value {
        Value::Nil => writer.write_u8(VALUE_NIL),
        Value::Boolean(false) => writer.write_u8(VALUE_FALSE),
        Value::Boolean(true) => writer.write_u8(VALUE_TRUE),
        Value::SmallInteger(integer) => {
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(*integer);
        }
        Value::Float(float) => {
            writer.write_u8(VALUE_FLOAT);
            writer.write_f64(*float);
        }
        Value::Object(_) => return false,
    }
    true
}

/// A `usize` count or index narrowed to the wire's `u32` field, refusing to
/// truncate: a length prefix that does not describe the payload that
/// follows would make the image self-inconsistent, which no reader could
/// detect. Unreachable for any image a real program can produce -- an
/// invariant an image write must not violate, so it aborts loudly.
pub(crate) fn u32_field(what: &str, value: usize) -> u32 {
    assert!(
        u32::try_from(value).is_ok(),
        "image {what} of {value} does not fit the format's u32 field"
    );
    value as u32
}

/// `u32_field`'s `u16` counterpart, for the register and parameter counts.
fn u16_field(what: &str, value: usize) -> u16 {
    assert!(
        u16::try_from(value).is_ok(),
        "image {what} of {value} does not fit the format's u16 field"
    );
    value as u16
}

/// `Rc<CompiledCode>` pointer -> code-table id, assigned by
/// `encode_code_table`; `encode_closure_record` uses it for a
/// closure's code reference.
pub struct CodeIds {
    ids: HashMap<usize, u32>,
}

impl CodeIds {
    pub fn id_of(&self, code: &Rc<CompiledCode>) -> u32 {
        *self.ids.get(&(Rc::as_ptr(code) as usize)).expect(
            "every CompiledCode reachable from the mixin tree was assigned an id by encode_code_table",
        )
    }
}

/// Explicit wire tags for `BuiltinClass` -- never `as u8` on the enum,
/// whose declaration order is not a wire format.
pub fn builtin_class_tag(kind: BuiltinClass) -> u8 {
    match kind {
        BuiltinClass::Object => 0,
        BuiltinClass::Boolean => 1,
        BuiltinClass::True => 2,
        BuiltinClass::False => 3,
        BuiltinClass::Integer => 4,
        BuiltinClass::Float => 5,
        BuiltinClass::String => 6,
        BuiltinClass::Symbol => 7,
        BuiltinClass::Character => 8,
        BuiltinClass::Array => 9,
        BuiltinClass::OrderedCollection => 10,
        BuiltinClass::UndefinedObject => 11,
        BuiltinClass::Block => 12,
        BuiltinClass::Platform => 13,
        BuiltinClass::VmMirror => 14,
        BuiltinClass::Top => 15,
        BuiltinClass::Value => 16,
        BuiltinClass::Number => 17,
    }
}

/// Explicit wire tags for `CodeKind`, for the same reason as
/// `builtin_class_tag`.
pub fn code_kind_tag(kind: CodeKind) -> u8 {
    match kind {
        CodeKind::Method => 0,
        CodeKind::Block => 1,
        CodeKind::Initializer => 2,
        CodeKind::SuperclassClause => 3,
        CodeKind::SuperclassMessage => 4,
    }
}

/// The module's root class object, which `load` rebuilds *before* the
/// object graph (the graph's class tag 1 names it, so it has to exist by
/// then). Everything else about it comes from mixin-table row 0: this
/// section carries the one thing a declaration does not determine, the
/// class it inherits from -- its superclass, and the levels its `<:` clauses
/// (`newspeak-spec.pdf` §6.2) stack on that superclass.
///
/// Written as a builtin-class tag, behind a presence flag that is always
/// set, rather than as a general class
/// tag, because the general form names an object-table id and the object
/// table is not decoded yet. That costs nothing: the root rule
/// (`instantiate::root_clause_defect`) lets a root inherit only `Object` or
/// `Value`, and `Mutator::load_root_class` builds every root over one of
/// them, so a builtin tag names every superclass this runtime builds. A root
/// class inheriting from a *user* class is refused here rather than written
/// wrongly.
///
/// The root's one permitted `<:` clause, `Value` over `Object`, is written
/// the same way: after the superclass, a count and, lowest level first, each
/// level's clause position in row 0 and the builtin its clause named. A root
/// whose clauses the rule forbids is built with no level at all, but never
/// reaches here: `encode_describing` refuses it first, and the decoder
/// refuses any level but that one (`decode::decode_root_section`).
pub fn encode_root_class<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    root_class: ClassReference<'gc>,
) -> Result<(), String> {
    if !heap.is_class(root_class) {
        return Err("image encoding's root object is not a class".to_string());
    }
    let levels = root_levels(heap, root_class);
    let lowest = levels.first().copied().unwrap_or(root_class);
    // A root inheriting from nothing is refused as well: every root this
    // runtime builds inherits `Object` or `Value`, and the decoder refuses a
    // section that names no superclass (`DecodeError::RootWithoutSuperclass`).
    let Some(superclass) = heap.class_superclass(lowest) else {
        return Err(
            "image encoding cannot serialize a root class that inherits from nothing: \
             a module's root class inherits Object or Value"
                .to_string(),
        );
    };
    let Some(kind) = heap.class_builtin(superclass) else {
        return Err(
            "image encoding cannot serialize a root class whose superclass is a \
             user-defined class: the format names the root's superclass by builtin \
             tag, decoded before the object table exists"
                .to_string(),
        );
    };
    writer.write_boolean(true);
    writer.write_u8(builtin_class_tag(kind));
    writer.write_u32(u32_field("root class level count", levels.len()));
    for level in levels {
        let applied = heap
            .class_applied_origin(level)
            .expect("internal: root_levels answers only applied levels");
        writer.write_u32(u32_field("mixin application clause index", applied.index));
        writer.write_u8(builtin_class_tag(named_builtin(&applied)?));
    }
    Ok(())
}

/// The levels the root class's own `<:` clauses added, lowest first -- the
/// order the clauses are written in, and the order the root-class section
/// lists them in. A class tag names one by its position here
/// (`CLASS_ROOT_LEVEL`): like the root class, a root level exists before the
/// graph does, so the graph never carries it as a record.
///
/// Walked down the superclass chain from the root for as long as each class
/// is a level one of the root's own clauses added -- the applying declaration
/// is the root's mixin itself, not merely a mixin applied somewhere.
pub fn root_levels<'gc>(
    heap: &Heap<'gc>,
    root_class: ClassReference<'gc>,
) -> Vec<ClassReference<'gc>> {
    let Some(root_mixin) = heap
        .class_application(root_class)
        .map(|application| application.mixin.clone())
    else {
        return Vec::new();
    };
    let mut levels = Vec::new();
    let mut current = heap.class_superclass(root_class);
    while let Some(class) = current {
        let is_root_level = heap
            .class_applied_origin(class)
            .is_some_and(|applied| Rc::ptr_eq(&applied.declaration, &root_mixin));
        if !is_root_level {
            break;
        }
        levels.push(class);
        current = heap.class_superclass(class);
    }
    levels.reverse();
    levels
}

/// The builtin a level's `<:` clause named, as the format can name it: a
/// builtin a program can name at all. `Top` and the VM mirror's class are
/// refused here because the decoder refuses their tags wherever a program
/// names a class (`decode::read_program_builtin_class`), so writing one would
/// produce an image nothing can load; no source can name either.
fn named_builtin(applied: &crate::mixin::AppliedClause) -> Result<BuiltinClass, String> {
    match applied.named_builtin {
        Some(BuiltinClass::Top | BuiltinClass::VmMirror) | None => Err(format!(
            "image encoding cannot serialize a level that `<:` clause {} of class `{}` added: \
             it applies a mixin that is neither part of the module's declaration tree nor a \
             builtin class a program can name",
            applied.index + 1,
            applied.declaration.name
        )),
        Some(kind) => Ok(kind),
    }
}

/// The entry selector, as a presence byte and -- when present -- a symbol id.
///
/// One shape for both artifacts on purpose: the decoder reads it the same way
/// either way and only the *requirement* differs, so there is no second
/// code path to keep in step. It sits after the root class rather than in the
/// header because it names a symbol and must therefore follow the symbol
/// table, and because down here it leaves the describing sections byte-equal
/// between a `build` and a `snapshot` of the same source.
pub fn encode_entry(writer: &mut Writer, entry: Option<u32>) {
    match entry {
        None => writer.write_u8(0),
        Some(id) => {
            writer.write_u8(1);
            writer.write_u32(id);
        }
    }
}

/// The code table: every `CompiledCode` reachable from the mixin tree under
/// `root_class` -- every mixin's methods, every mixin's initializer, every
/// superclass clause and clause-message body, every `<:` clause's message
/// body, and every nested block body --
/// with nested block code *before* the code that creates it. Flat and
/// unnamed: *which* body belongs to which declaration is the mixin table's
/// business (`image::mixins`), which addresses this table by id.
///
/// Collected in `mixin::preorder` order -- the same order the mixin table's
/// rows are written in -- so the same module always encodes to the same
/// bytes.
pub fn encode_code_table<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    root_class: ClassReference<'gc>,
    symbols: &mut symbols::Builder,
) -> Result<CodeIds, String> {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds {
        ids: HashMap::new(),
    };
    let rows = mixins::plan(heap, root_class)?;
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
        encode_code(writer, code, &ids, symbols);
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
    for instruction in &code.instructions {
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
) {
    writer.write_u8(code_kind_tag(code.kind));
    writer.write_u16(code.argument_count);
    writer.write_u16(code.register_count);
    // `outer_reach` is deliberately absent: it follows from the
    // instructions, and `decode_code` recomputes it with the same
    // `bytecode::outer_reach_of` the compiler used.
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
    writer.write_u32(u32_field("instruction count", code.instructions.len()));
    for instruction in &code.instructions {
        encode_instruction(writer, instruction, ids, symbols);
    }
}

/// Whether `code`'s own sends are `CallSite::UNCACHED` -- true exactly for a
/// body the reference lowering compiled, which numbers no send at all
/// (`compiler::reference`). The optimized lowering numbers every send in a
/// body uniformly, so one send (if `code` has any) tells the whole body's
/// story; a body with no sends of its own needs no flag, since
/// `decode_instruction` never reads it without a send to build.
fn compiled_uncached(code: &CompiledCode) -> bool {
    code.instructions
        .iter()
        .filter_map(Instruction::call_site)
        .next()
        .is_some_and(|site| site == CallSite::UNCACHED)
}

fn encode_registers(writer: &mut Writer, registers: &[u16]) {
    writer.write_u16(u16_field("register list length", registers.len()));
    for register in registers {
        writer.write_u16(*register);
    }
}

fn encode_instruction(
    writer: &mut Writer,
    instruction: &Instruction,
    ids: &CodeIds,
    symbols: &mut symbols::Builder,
) {
    match instruction {
        // The code table is collected from the module's own mixin tree
        // (`mixins::plan`), which never contains a kernel mixin, so this is
        // unreachable by construction: a panic here is a defect in this
        // process, not a property of any input.
        Instruction::SendMirror { .. } => unreachable!(
            "internal: SendMirror is emitted only into Kernel.ns, and an image \
             carries only its own module's code"
        ),
        Instruction::LoadLiteral {
            destination,
            literal,
        } => {
            writer.write_u8(OPCODE_LOAD_LITERAL);
            writer.write_u16(*destination);
            encode_literal(writer, literal, symbols);
        }
        Instruction::LoadSelf { destination } => {
            writer.write_u8(OPCODE_LOAD_SELF);
            writer.write_u16(*destination);
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
            encode_registers(writer, argument_registers);
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
            encode_registers(writer, argument_registers);
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
            encode_registers(writer, argument_registers);
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
            encode_registers(writer, argument_registers);
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
            writer.write_u16(u16_field("closure parameter count", parameters.len()));
            for parameter in parameters.iter() {
                writer.write_u32(symbols.intern(parameter.as_str()));
            }
            writer.write_u32(ids.id_of(code));
            encode_registers(writer, copied);
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            writer.write_u8(OPCODE_MAKE_ARRAY);
            writer.write_u16(*destination);
            encode_registers(writer, element_registers);
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
        Instruction::InitializeSlot { source, own_index } => {
            writer.write_u8(OPCODE_INITIALIZE_SLOT);
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

fn encode_literal(writer: &mut Writer, literal: &Literal, symbols: &mut symbols::Builder) {
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
        Literal::Character(character) => {
            writer.write_u8(LITERAL_CHARACTER);
            writer.write_u32(*character as u32);
        }
        Literal::Array(elements) => {
            writer.write_u8(LITERAL_ARRAY);
            writer.write_u32(u32_field("literal array length", elements.len()));
            for element in elements {
                encode_literal(writer, element, symbols);
            }
        }
        Literal::MalformedInteger(text) => {
            writer.write_u8(LITERAL_MALFORMED_INTEGER);
            writer.write_str(text);
        }
        Literal::UnresolvedIdentifier(name) => {
            writer.write_u8(LITERAL_UNRESOLVED_IDENTIFIER);
            writer.write_str(name);
        }
    }
}

// ---------------------------------------------------------------------------
// The object table and the frame table (`docs/design/image-format.md`,
// "Sections").
// ---------------------------------------------------------------------------

fn is_same_object<'gc>(first: &Value<'gc>, second: &Value<'gc>) -> bool {
    matches!((first, second), (Value::Object(left), Value::Object(right)) if left.same(*right))
}

/// One entry of `GraphTable::build`'s explicit work stack: the traversal
/// alternates between values and frames, so both live on the same stack.
enum Pending<'gc> {
    Value(Value<'gc>),
    Frame(FrameReference<'gc>),
}

/// A class object's graph references, in the order its record writes them:
/// the superclass, the enclosing object, the declaring class. Traversal and
/// encoding share one definition so the two cannot drift into a reference
/// that is written but never given an id.
///
/// A level whose `<:` clause named a builtin has only its superclass: its
/// enclosing object and declaring class are the builtin's own (§6.2, "the
/// enclosing object of I is the enclosing object of C"), which is the core
/// library's `Kernel` -- the object that holds the VM mirror. The image
/// carries neither, exactly as it carries no builtin; the decoder takes both
/// from the loading bootstrap's builtin, as `nested::build_applied_levels`
/// does.
fn class_references<'a, 'gc>(
    data: &'a ClassData<'gc>,
) -> impl Iterator<Item = ObjectReference<'gc>> + 'a {
    let own_context = !names_a_builtin(data);
    data.superclass
        .iter()
        .chain(data.enclosing.iter().filter(move |_| own_context))
        .chain(data.declaring_class.iter().filter(move |_| own_context))
        .copied()
}

/// Whether `data` is a level a `<:` clause naming a builtin added.
fn names_a_builtin(data: &ClassData<'_>) -> bool {
    data.applied_clause
        .as_ref()
        .is_some_and(|applied| applied.named_builtin.is_some())
}

/// Whether `object` is a class object the format names with a *tag* rather
/// than with an object-table id: a builtin, which the loaded bootstrap
/// supplies, or the module's root class or one of its levels, which `load`
/// rebuilds from the mixin table and the root-class section before the graph
/// is decoded. None of them exists because the graph says so, so none is a
/// record. Every other class object is. The mirror of
/// `decode::is_prebuilt_class`, including its explicit, today-redundant
/// `is_class` guard -- see that function's doc comment.
fn is_class_tag<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
    root_class: ClassReference<'gc>,
    root_levels: &[ClassReference<'gc>],
) -> bool {
    heap.is_class(object)
        && (heap.class_builtin(object).is_some()
            || object.same(root_class)
            || root_levels.iter().any(|level| level.same(object)))
}

/// Every heap object (instances, strings, symbols, arrays, closures, and
/// the class objects that are neither builtin nor the module's root) and
/// every `Frame` reachable from the roots, in discovery order. A builtin
/// class, the root class and the `platform` singleton are never entered:
/// each is named by a tag of its own.
pub struct GraphTable<'gc> {
    objects: Vec<ObjectReference<'gc>>,
    object_ids: HashMap<usize, u32>,
    frames: Vec<FrameReference<'gc>>,
    frame_ids: HashMap<usize, u32>,
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
    pub fn build(
        heap: &Heap<'gc>,
        roots: &[Value<'gc>],
        platform: &Value<'gc>,
        root_class: ClassReference<'gc>,
        root_levels: &[ClassReference<'gc>],
    ) -> GraphTable<'gc> {
        let mut table = GraphTable {
            objects: Vec::new(),
            object_ids: HashMap::new(),
            frames: Vec::new(),
            frame_ids: HashMap::new(),
        };
        let mut stack: Vec<Pending> = roots
            .iter()
            .rev()
            .map(|root| Pending::Value(*root))
            .collect();
        while let Some(pending) = stack.pop() {
            match pending {
                Pending::Value(value) => table.visit_value(
                    heap,
                    &value,
                    platform,
                    (root_class, root_levels),
                    &mut stack,
                ),
                Pending::Frame(frame) => table.visit_frame(heap, frame, &mut stack),
            }
        }
        table
    }

    fn visit_value(
        &mut self,
        heap: &Heap<'gc>,
        value: &Value<'gc>,
        platform: &Value<'gc>,
        (root_class, root_levels): (ClassReference<'gc>, &[ClassReference<'gc>]),
        stack: &mut Vec<Pending<'gc>>,
    ) {
        let Value::Object(object) = value else { return };
        if is_same_object(value, platform) || is_class_tag(heap, *object, root_class, root_levels) {
            return;
        }
        let key = heap.object_identity_key(*object);
        if self.object_ids.contains_key(&key) {
            return;
        }
        self.object_ids
            .insert(key, u32_field("object table id", self.objects.len()));
        self.objects.push(*object);
        let before = stack.len();
        {
            let record = heap.object_snapshot(*object);
            // An object's own class is a reference like every other: now
            // that a class object can be a record, reaching one only
            // through whatever values happen to point at it would leave
            // `encode_class_tag` with an id that was never assigned.
            stack.push(Pending::Value(Value::Object(record.class)));
            match &record.data {
                ObjectData::Instance(fields) | ObjectData::Array(fields) => {
                    stack.extend(fields.iter().copied().map(Pending::Value));
                }
                ObjectData::Closure(closure) => {
                    stack.push(Pending::Frame(closure.captured_frame));
                    // A closure's home is not walked: it is derived from
                    // the captured frame on both sides of the wire
                    // (`Heap::closure_home`), so it is never a reference of
                    // its own.
                    stack.extend(closure.copied.iter().copied().map(Pending::Value));
                }
                // A class object's three references. `metaclass` is not one
                // of them: it is a lazy cache `Mutator::metaclass_of`
                // rebuilds on demand, and nothing hands a metaclass to
                // Newspeak code, so no value in the graph can point at one.
                ObjectData::Class(data) => {
                    stack.extend(
                        class_references(data).map(|class| Pending::Value(Value::Object(class))),
                    );
                }
                ObjectData::String(_) | ObjectData::Symbol(_) => {}
            }
        }
        stack[before..].reverse();
    }

    fn visit_frame(
        &mut self,
        heap: &Heap<'gc>,
        frame: FrameReference<'gc>,
        stack: &mut Vec<Pending<'gc>>,
    ) {
        let key = heap.frame_identity_key(frame);
        if self.frame_ids.contains_key(&key) {
            return;
        }
        self.frame_ids
            .insert(key, u32_field("frame table id", self.frames.len()));
        self.frames.push(frame);
        let before = stack.len();
        {
            let snapshot = heap.frame_snapshot(frame);
            stack.push(Pending::Value(snapshot.receiver));
            stack.extend(snapshot.arguments.iter().copied().map(Pending::Value));
            stack.extend(snapshot.temporaries.iter().copied().map(Pending::Value));
            stack.extend(
                snapshot
                    .sender
                    .iter()
                    .chain(snapshot.home.iter())
                    .copied()
                    .map(Pending::Frame),
            );
            stack.extend(
                snapshot
                    .current_class
                    .iter()
                    .copied()
                    .map(|class| Pending::Value(Value::Object(class))),
            );
            match &snapshot.kind {
                // The closure object a closure activation runs is a record
                // of its own, so it needs an id like any other object.
                FrameKind::Closure(closure) => {
                    stack.push(Pending::Value(Value::Object(*closure)));
                }
                // The class-carrying kinds name a class the same way
                // `current_class` does, so the class needs an id the same way.
                FrameKind::Initializer(class)
                | FrameKind::SuperclassClause(class)
                | FrameKind::SuperclassMessage(class)
                | FrameKind::MixinApplicationMessage(class) => {
                    stack.push(Pending::Value(Value::Object(*class)));
                }
                FrameKind::Method(_) | FrameKind::TopLevel => {}
            }
        }
        stack[before..].reverse();
    }

    pub fn object_id(&self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> u32 {
        *self
            .object_ids
            .get(&heap.object_identity_key(object))
            .expect("object was visited by build()")
    }

    pub fn frame_id(&self, heap: &Heap<'gc>, frame: FrameReference<'gc>) -> u32 {
        *self
            .frame_ids
            .get(&heap.frame_identity_key(frame))
            .expect("frame was visited by build()")
    }

    pub fn objects(&self) -> &[ObjectReference<'gc>] {
        &self.objects
    }

    pub fn frames(&self) -> &[FrameReference<'gc>] {
        &self.frames
    }
}

/// Everything the graph encoder resolves a reference against: the object
/// and frame tables, the `platform` singleton and the module's root class
/// (each named by a tag of its own rather than by an id), the code table's
/// ids, and the mixin table's row ids -- which is how a class object's
/// record names the declaration it applies.
pub struct EncodeContext<'a, 'gc> {
    pub table: &'a GraphTable<'gc>,
    pub code_ids: &'a CodeIds,
    pub platform: &'a Value<'gc>,
    pub root_class: &'a ClassReference<'gc>,
    /// `mixin::mixin_key` -> mixin-table row id, as `mixins::encode`
    /// assigned them.
    pub mixin_ids: &'a HashMap<usize, u32>,
    /// `root_levels(root_class)`: the levels a class tag names by position.
    pub root_levels: &'a [ClassReference<'gc>],
    pub heap: &'a Heap<'gc>,
}

/// `Builtin(kind)` (0 + kind tag), `UserRoot` (1), `Record` (2 + object
/// id) or `RootLevel` (3 + position) -- the four ways a class object can
/// exist in a loaded world.
///
/// All but `Record` are rebuilt before the graph is: a builtin comes from the
/// bootstrap, the root class and its levels from mixin-table row 0 plus the
/// root-class section (`decode::build_root_class`).
/// Everything else is a class object the *graph* creates, and tag 2 names
/// which one by the id of its own record in the object table. That third
/// case is what nesting needs: a nested class object is the value of its
/// enclosing instance's hidden cache slot and the `.class` of every
/// instance of it, and there is one per enclosing instance, so no tag that
/// names a class only by its position in the source could tell them apart.
fn encode_class_tag<'gc>(
    writer: &mut Writer,
    class: ClassReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) {
    if !context.heap.is_class(class) {
        panic!("internal: a class reference does not point at a class object");
    }
    let builtin = context.heap.class_builtin(class);
    match builtin {
        Some(kind) => {
            writer.write_u8(CLASS_BUILTIN);
            writer.write_u8(builtin_class_tag(kind));
        }
        None if class.same(*context.root_class) => writer.write_u8(CLASS_USER_ROOT),
        None if context.root_levels.iter().any(|level| level.same(class)) => {
            let position = context
                .root_levels
                .iter()
                .position(|level| level.same(class))
                .expect("internal: the level was just found among the root's levels");
            writer.write_u8(CLASS_ROOT_LEVEL);
            writer.write_u32(u32_field("root class level position", position));
        }
        None => {
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(context.table.object_id(context.heap, class));
        }
    }
}

fn encode_optional_class<'gc>(
    writer: &mut Writer,
    class: Option<&ClassReference<'gc>>,
    context: &EncodeContext<'_, 'gc>,
) {
    match class {
        Some(present_class) => {
            writer.write_boolean(true);
            encode_class_tag(writer, *present_class, context);
        }
        None => writer.write_boolean(false),
    }
}

/// The general `Value` encoder: scalar tags 0-4 (`encode_scalar_value`),
/// 5 = object-table id, 6 = class tag, 7 = the `platform` singleton.
///
/// A class object always goes through tag 6, even the ones tag 6 then
/// resolves to an object-table id: one rule per kind of referent is what
/// keeps the encoding canonical.
fn encode_value<'gc>(writer: &mut Writer, value: &Value<'gc>, context: &EncodeContext<'_, 'gc>) {
    if encode_scalar_value(writer, value) {
        return;
    }
    let Value::Object(object) = value else {
        unreachable!("encode_scalar_value handles every non-Obj case")
    };
    if is_same_object(value, context.platform) {
        writer.write_u8(VALUE_PLATFORM);
        return;
    }
    if context.heap.is_class(*object) {
        writer.write_u8(VALUE_CLASS_TAG);
        encode_class_tag(writer, *object, context);
        return;
    }
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(context.table.object_id(context.heap, *object));
}

fn encode_values<'gc>(
    writer: &mut Writer,
    values: &[Value<'gc>],
    context: &EncodeContext<'_, 'gc>,
) {
    writer.write_u32(u32_field("value list length", values.len()));
    for value in values {
        encode_value(writer, value, context);
    }
}

fn encode_frame_reference<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
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

/// `code_id`, `captured_frame` id, then the copied values, counted.
///
/// No parameter names: the arity a call is checked against is the body's
/// own `argument_count`, so there is no second half to disagree with. No
/// home either: it follows from the captured frame
/// (`Heap::closure_home`).
fn encode_closure_record<'gc>(
    writer: &mut Writer,
    closure: &ClosureData<'gc>,
    context: &EncodeContext<'_, 'gc>,
) {
    // Destructured, not read field by field: a new `ClosureData` field must
    // say here whether it belongs on the wire, rather than silently being
    // left off it.
    let ClosureData {
        code,
        captured_frame,
        copied,
        feedback_base: _, // never on the wire; see `decode_closure_record`
    } = closure;
    writer.write_u32(context.code_ids.id_of(code));
    writer.write_u32(context.table.frame_id(context.heap, *captured_frame));
    writer.write_u16(u16_field("closure copied value count", copied.len()));
    for value in copied.iter() {
        encode_value(writer, value, context);
    }
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
/// `builtin` is not written either -- a record only ever describes a user
/// class, since a builtin is a class *tag* -- and neither is `metaclass`, a
/// lazy cache `Mutator::metaclass_of` rebuilds on the first class-side send.
///
/// A metaclass can still reach here as a record of its own, through the one
/// reference that names one: a class-side activation's `current_class`,
/// captured in a frame. Such a metaclass applies the declaring class's own
/// class side (`newspeak-spec.pdf` §3.2; ns-next's own representation is
/// `docs/design/object-model.md`, "Class sides and metaclasses"), which is
/// part of the module's declaration tree like any other mixin, so the id
/// lookup below finds it. The superclass written for it is `Object`'s class
/// object, the link `metaclass_of` gives a metaclass; the *application* it
/// is rebuilt with is flat all the same, because
/// `decode::rebuild_applications` recognizes a record whose mixin is some
/// row's class side and applies it with no superclass application. The other
/// kind of metaclass mixin --
/// the otherwise empty one `metaclass_of` synthesizes for a class that
/// declares no class side, so that §6.3's factory entry has somewhere to
/// live -- is in no `mixin::preorder` walk and has no id. It cannot reach
/// here: its method table holds the factory and nothing else, a factory
/// entry pushes no activation, and so no frame can ever name it as a
/// `current_class`. Should some later reference to a metaclass make one
/// reachable, this answers the ordinary "not part of the declaration tree"
/// error rather than writing a wrong id -- a refusal to snapshot, not a
/// corrupt image.
///
/// A level a `<:` clause added (`newspeak-spec.pdf` §6.2) is a record like
/// any other, except that it ends with the clause that added it, as the
/// applying declaration's row id and the clause's position there: the
/// clause's selector and compiled message are that row's, which
/// `decode::verify_mixins` has reconciled with the row's own factory.
///
/// A level whose clause named a builtin (`<: Value`) has a record of its own
/// shape (`MIXIN_BUILTIN`): its mixin is the builtin's, which no row
/// describes, so it is named by builtin tag, and its enclosing object and
/// declaring class are the builtin's too, so they are not written at all
/// (`class_references`). What is left is the builtin, the superclass and the
/// clause, which is never absent.
fn encode_class_record<'gc>(
    writer: &mut Writer,
    data: &ClassData<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    let applied_row = |applied: &crate::mixin::AppliedClause| {
        context
            .mixin_ids
            .get(&crate::mixin::mixin_key(&applied.declaration))
            .copied()
            .ok_or_else(|| {
                format!(
                    "image encoding cannot serialize a level added by class `{}`, \
                     which is not part of the module's declaration tree",
                    applied.declaration.name
                )
            })
    };
    if let Some(applied) = data
        .applied_clause
        .as_ref()
        .filter(|applied| applied.named_builtin.is_some())
    {
        writer.write_u8(MIXIN_BUILTIN);
        writer.write_u8(builtin_class_tag(named_builtin(applied)?));
        encode_optional_class(writer, data.superclass.as_ref(), context);
        writer.write_u32(applied_row(applied)?);
        writer.write_u32(u32_field("mixin application clause index", applied.index));
        return Ok(());
    }
    let mixin = &data.application.mixin;
    let id = context
        .mixin_ids
        .get(&crate::mixin::mixin_key(mixin))
        .copied()
        .ok_or_else(|| {
            format!(
                "image encoding cannot serialize a class object applying mixin `{}`, \
                 which is not part of the module's declaration tree",
                mixin.name
            )
        })?;
    writer.write_u8(MIXIN_ROW);
    writer.write_u32(id);
    encode_optional_class(writer, data.superclass.as_ref(), context);
    // The enclosing object is an ordinary object, not a class, so it is
    // written the way every other object reference is -- one `Value`.
    match &data.enclosing {
        Some(enclosing) => {
            writer.write_boolean(true);
            encode_value(writer, &Value::Object(*enclosing), context);
        }
        None => writer.write_boolean(false),
    }
    encode_optional_class(writer, data.declaring_class.as_ref(), context);
    match &data.applied_clause {
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
    let record = context.heap.object_snapshot(object);
    encode_class_tag(writer, record.class, context);
    writer.write_u32(record.identity_hash);
    match &record.data {
        ObjectData::Instance(fields) => {
            writer.write_u8(OBJECT_INSTANCE);
            encode_values(writer, fields, context);
        }
        ObjectData::String(string) => {
            writer.write_u8(OBJECT_STRING);
            writer.write_str(string);
        }
        ObjectData::Symbol(symbol) => {
            writer.write_u8(OBJECT_SYMBOL);
            writer.write_u32(symbols.intern(symbol.as_str()));
        }
        ObjectData::Array(elements) => {
            writer.write_u8(OBJECT_ARRAY);
            encode_values(writer, elements, context);
        }
        ObjectData::Closure(closure) => {
            writer.write_u8(OBJECT_CLOSURE);
            encode_closure_record(writer, closure, context);
        }
        ObjectData::Class(data) => {
            writer.write_u8(OBJECT_CLASS);
            encode_class_record(writer, data, context)?;
        }
    }
    Ok(())
}

fn encode_frame<'gc>(
    writer: &mut Writer,
    frame: FrameReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
    symbols: &mut symbols::Builder,
) {
    let snapshot = context.heap.frame_snapshot(frame);
    encode_value(writer, &snapshot.receiver, context);
    encode_values(writer, &snapshot.arguments, context);
    encode_values(writer, &snapshot.temporaries, context);
    writer.write_u32(u32_field("frame program counter", snapshot.program_counter));
    // An image's code table holds only compiled bodies, so a frame's
    // `program_counter` is always an instruction index and the record does not
    // say so.
    encode_frame_reference(
        writer,
        context.heap,
        snapshot.sender.as_ref(),
        context.table,
    );
    encode_frame_reference(writer, context.heap, snapshot.home.as_ref(), context.table);
    encode_optional_class(writer, snapshot.current_class.as_ref(), context);
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
            encode_class_tag(writer, *class, context);
        }
        FrameKind::SuperclassClause(class) => {
            writer.write_u8(FRAME_SUPERCLASS_CLAUSE);
            encode_class_tag(writer, *class, context);
        }
        FrameKind::SuperclassMessage(class) => {
            writer.write_u8(FRAME_SUPERCLASS_MESSAGE);
            encode_class_tag(writer, *class, context);
        }
        FrameKind::MixinApplicationMessage(level) => {
            writer.write_u8(FRAME_MIXIN_APPLICATION_MESSAGE);
            encode_class_tag(writer, *level, context);
        }
    }
    writer.write_boolean(snapshot.uncontinuable);
}

/// The object table, the frame table, and the roots. Both counts come
/// first so the decoder can placeholder-allocate both tables before
/// filling either (objects and frames reference each other freely).
#[allow(clippy::too_many_arguments)]
pub fn encode_graph_section<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    roots: &[Value<'gc>],
    platform: &Value<'gc>,
    code_ids: &CodeIds,
    root_class: ClassReference<'gc>,
    mixin_ids: &HashMap<usize, u32>,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let root_levels = root_levels(heap, root_class);
    let table = GraphTable::build(heap, roots, platform, root_class, &root_levels);
    let context = EncodeContext {
        table: &table,
        code_ids,
        platform,
        root_class: &root_class,
        mixin_ids,
        root_levels: &root_levels,
        heap,
    };
    writer.write_u32(u32_field("object table entry count", table.objects().len()));
    writer.write_u32(u32_field("frame table entry count", table.frames().len()));
    for object in table.objects() {
        // The VM mirror is a capability the bootstrap hands to the core
        // library alone (`docs/design/core-library.md`, "The VM mirror is
        // the whole Rust boundary"); an artifact that carried it would hand
        // it to whatever loaded it.
        if heap.class_builtin(heap.class_of_object(*object)) == Some(BuiltinClass::VmMirror) {
            return Err(
                "image encoding refuses the VM mirror: it is a capability the \
                 bootstrap hands to the core library alone, and an artifact that \
                 carried it would hand it to whatever loaded it"
                    .to_string(),
            );
        }
        encode_object(writer, *object, &context, symbols)?;
    }
    for frame in table.frames() {
        encode_frame(writer, *frame, &context, symbols);
    }
    encode_values(writer, roots, &context);
    Ok(())
}

/// The graph section's closing table: the identity hash of every object the
/// image names by tag rather than carries as a record -- each builtin class,
/// the module's root class, the `platform` singleton -- so that a hash the
/// program was given survives the load. Only an assigned hash (not 0) is
/// written, in a fixed order -- the builtin classes in wire-tag order, then
/// the root class, then `platform` -- so one world encodes to one byte
/// sequence. See `docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim".
///
/// Every builtin is considered, not only those the graph reaches: a program
/// can keep `Integer hash` in a slot without keeping `Integer`.
pub fn encode_tag_named_hashes<'gc>(
    writer: &mut Writer,
    mutator: &Mutator<'gc>,
    root_class: ClassReference<'gc>,
    platform: &Value<'gc>,
) {
    let heap = &mutator.heap;
    let mut entries: Vec<(u8, Option<u8>, u32)> = Vec::new();
    for kind in BuiltinClass::ALL {
        let hash = heap.identity_hash(mutator.builtin_class(kind));
        if hash != 0 {
            entries.push((HASHED_BUILTIN_CLASS, Some(builtin_class_tag(kind)), hash));
        }
    }
    let hash = heap.identity_hash(root_class);
    if hash != 0 {
        entries.push((HASHED_ROOT_CLASS, None, hash));
    }
    if let Value::Object(platform) = platform {
        let hash = heap.identity_hash(*platform);
        if hash != 0 {
            entries.push((HASHED_PLATFORM, None, hash));
        }
    }
    writer.write_u32(u32_field("tag-named hash count", entries.len()));
    for (tag, builtin, hash) in entries {
        writer.write_u8(tag);
        if let Some(builtin) = builtin {
            writer.write_u8(builtin);
        }
        writer.write_u32(hash);
    }
}
