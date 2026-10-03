//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- see `docs/design/image-format.md`. Never a memory dump.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{Heap, Word};
use crate::interpreter::future;
use crate::types::{
    BuiltinClass, ClassData, ClassReference, ClosureData, FrameKind, FrameReference, ObjectData,
    ObjectReference, SourceOrigin, Value, ValueKind,
};
use crate::{ArbitraryInteger, Mutator, Symbol};

use super::modules::{ModuleClass, ModuleClasses};
use super::opcodes::*;
use super::platform_paths::{self, PlatformPaths};
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
/// detect. Only for a count no program can push past `u32::MAX`: an id or
/// count of heap objects or frames, which the heap's 32-bit handles bound;
/// one of declarations, statements or instructions, which source bounds (a
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

/// Explicit wire tags for `BuiltinClass` -- never `as u8` on the enum,
/// whose declaration order is not a wire format. The retired numbers are
/// listed in `opcodes.rs` (`RETIRED_BUILTIN_CLASS_TAGS`).
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
        BuiltinClass::Array => 9,
        BuiltinClass::UndefinedObject => 11,
        BuiltinClass::Block => 12,
        BuiltinClass::VmMirror => 14,
        BuiltinClass::Top => 15,
        BuiltinClass::Value => 16,
        BuiltinClass::Number => 17,
        BuiltinClass::Behavior => 20,
        BuiltinClass::Class => 21,
        BuiltinClass::Metaclass => 22,
        BuiltinClass::Activation => 23,
        BuiltinClass::Exception => 24,
        BuiltinClass::Error => 25,
        BuiltinClass::MessageNotUnderstood => 26,
        BuiltinClass::Message => 27,
        BuiltinClass::Halt => 28,
        BuiltinClass::Future => 30,
        BuiltinClass::LargeInteger => 31,
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

/// A module's root class object, which `load` rebuilds *before* the object
/// graph (the graph's `CLASS_MODULE` tag names it, so it has to exist by
/// then). Everything else about it comes from the first row of its module's
/// tree in the mixin table: this section, one per module in the module
/// section, carries the one thing a declaration does not determine, the
/// class it inherits from -- its superclass, and the levels its `<:` clauses
/// (`newspeak-spec.pdf` §6.2) stack on that superclass.
///
/// Written as a builtin-class tag, behind a presence flag that is always
/// set, rather than as a general class
/// tag, because the general form names an object-table id and the object
/// table is not decoded yet. That costs nothing: the root rule
/// (`resolver::root_clause_defect`) lets a root inherit only `Object` or
/// `Value`, and `Mutator::load_root_class` builds every root over one of
/// them, so a builtin tag names every superclass this runtime builds. A root
/// class inheriting from a *user* class is refused here rather than written
/// wrongly.
///
/// The root's one permitted `<:` clause, `Value` over `Object`, is written
/// the same way: after the superclass, a count and, lowest level first, each
/// level's clause position in the root's row and the builtin its clause
/// named. A root
/// whose clauses the rule forbids is built with no level at all, but never
/// reaches here: `encode_describing` refuses it first, and the decoder
/// refuses any level but that one (`decode::decode_root_section`).
pub fn encode_root_class<'gc>(
    heap: &Heap<'gc>,
    writer: &mut Writer,
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
/// order the clauses are written in, and the order the module's root section
/// lists them in. A class tag names one by its module and its position here
/// (`CLASS_MODULE_LEVEL`): like the root class, a root level exists before
/// the graph does, so the graph never carries it as a record.
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
/// builtin a program can name at all. `Top`, the VM mirror's class and
/// `Activation` are refused here because the decoder refuses their tags
/// wherever a program names a class (`decode::read_program_builtin_class`),
/// so writing one would produce an image nothing can load; no source can
/// name any of them.
fn named_builtin(applied: &crate::mixin::AppliedClause) -> Result<BuiltinClass, String> {
    match applied.named_builtin {
        Some(
            BuiltinClass::Top
            | BuiltinClass::VmMirror
            | BuiltinClass::Activation
            | BuiltinClass::Future,
        )
        | None => Err(format!(
            "image encoding cannot serialize a level that `<:` clause {} of class `{}` added: \
             it applies a mixin that is neither part of any module the image carries nor a \
             builtin class a program can name",
            applied.index + 1,
            applied.declaration.name
        )),
        Some(kind) => Ok(kind),
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
// The object table and the frame table (`docs/design/image-format.md`,
// "Sections").
// ---------------------------------------------------------------------------

fn is_same_object<'gc>(first: Value<'gc>, second: Value<'gc>) -> bool {
    matches!((first.as_object(), second.as_object()), (Some(left), Some(right)) if left.same(right))
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
/// supplies, or a carried module's root class or one of its levels, which
/// `load` rebuilds from the mixin table and the module section before the
/// graph is decoded. None of them exists because the graph says so, so none is a
/// record. The metaclass of one of them is named by tag too
/// (`CLASS_METACLASS`), for the same reason: it exists because its class
/// does, before the graph is decoded. Every other class object is a record,
/// a metaclass of a class record included (`OBJECT_METACLASS`).
fn is_class_tag<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
    modules: &ModuleClasses<'gc>,
    paths: &PlatformPaths<'gc>,
) -> bool {
    is_prebuilt_class(heap, object, modules, paths)
        || heap
            .class_this_class(object)
            .is_some_and(|this_class| is_prebuilt_class(heap, this_class, modules, paths))
}

/// `object` described, when it is a class object an image may not name:
/// `Top`, the VM mirror's class, `Activation` or `Future`, which the decoder
/// refuses wherever the graph names a class
/// (`decode::read_referenced_builtin_class`), or the metaclass of one of
/// them, which it refuses behind a `CLASS_METACLASS` reference the same way.
/// No source module can name any of the eight -- `Object` declares no
/// accessor for them -- so only a graph reaching into the kernel can;
/// writing one would produce an image nothing loads, so the encoder refuses
/// it instead. The header of an activation object or of an unresolved
/// future is not such a reference: `GraphTable::visit_value` never walks
/// it, and the decoder accepts `Activation` and `Future` there and only
/// there.
fn unnameable_class<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<String> {
    let (class, suffix) = match heap.class_this_class(object) {
        Some(this_class) => (this_class, " class"),
        None => (object, ""),
    };
    match heap.class_builtin(class) {
        Some(BuiltinClass::Top) => Some(format!("Top{suffix}")),
        Some(BuiltinClass::VmMirror) => Some(format!("the VM mirror's class{suffix}")),
        Some(BuiltinClass::Activation) => Some(format!("Activation{suffix}")),
        Some(BuiltinClass::Future) => Some(format!("Future{suffix}")),
        _ => None,
    }
}

/// Whether `object` is a builtin, a module's root class, one of its levels
/// or a class a platform path names -- the class objects the loading world
/// has before the graph, and what `decode::read_prebuilt_class` reads behind
/// a `CLASS_METACLASS` reference. The explicit `is_class` guard is redundant
/// with `modules.find` today -- a module class is a class by construction --
/// but it keeps this answering `false` for a non-class object on its own
/// terms, so it stays honest if either check's meaning shifts later.
fn is_prebuilt_class<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
    modules: &ModuleClasses<'gc>,
    paths: &PlatformPaths<'gc>,
) -> bool {
    heap.is_class(object)
        && (heap.class_builtin(object).is_some()
            || modules.find(object).is_some()
            || paths.id_of(heap, object).is_some())
}

/// Why the image cannot carry `object` -- reached by no platform path, and
/// not named by any other tag -- when it is shipped: a class object compiled
/// from shipped source, or a module instance, an instance of a shipped class
/// whose class object has no enclosing object. Either one's code and layout
/// belong to the loading binary, and a module instance holds the kernel, as
/// the platform holds the mirror, so neither is ever written field by field.
///
/// Shipped is `platform_paths::is_shipped_object`, the one rule `discover`
/// and the loader apply too. A level a `<:` clause naming a builtin added
/// applies that builtin's shipped mixin, but belongs to the user class that
/// applied it: the image writes it as a `MIXIN_BUILTIN` record, and its
/// metaclass as a record beside it, so neither is a refusal.
fn shipped_without_path<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<String> {
    if heap.is_class(object) {
        let class = heap.class_this_class(object).unwrap_or(object);
        let applies_a_named_builtin = heap
            .class_applied_origin(class)
            .is_some_and(|applied| applied.named_builtin.is_some());
        if platform_paths::is_shipped_object(heap, object) && !applies_a_named_builtin {
            return Some(format!(
                "image encoding refuses the class object {}: it is compiled from shipped \
                 source, and no platform path names it",
                heap.class_display_name(object)
            ));
        }
        return None;
    }
    let class = heap.object_class(object);
    if platform_paths::is_shipped_object(heap, object) && heap.class_enclosing(class).is_none() {
        return Some(format!(
            "image encoding refuses a module instance of {}: it holds the core library's \
             state, and no platform path names it",
            heap.class_display_name(class)
        ));
    }
    None
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
    /// The first class the traversal reached that no program may name --
    /// `Top`, the VM mirror's class, `Activation`, `Future`, or the metaclass
    /// of one of them -- which
    /// `encode_graph_section` refuses (`unnameable_class`).
    unnameable: Option<String>,
    /// The first shipped object the traversal reached that no platform path
    /// names (`shipped_without_path`), refused the same way.
    shipped_without_path: Option<String>,
    /// Every object a platform path reaches from the platform the graph
    /// hangs off (`platform_paths::discover`).
    platform_paths: PlatformPaths<'gc>,
    /// The ids of the paths the graph uses, in the order it first reached
    /// each -- the order the platform path section lists them in -- and each
    /// one's position in that list, which is what a reference writes.
    used_paths: Vec<u32>,
    used_path_index: HashMap<u32, u32>,
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
        platform: Value<'gc>,
        modules: &ModuleClasses<'gc>,
        platform_paths: PlatformPaths<'gc>,
    ) -> GraphTable<'gc> {
        let mut table = GraphTable {
            objects: Vec::new(),
            object_ids: HashMap::new(),
            frames: Vec::new(),
            frame_ids: HashMap::new(),
            unnameable: None,
            shipped_without_path: None,
            platform_paths,
            used_paths: Vec::new(),
            used_path_index: HashMap::new(),
        };
        let mut stack: Vec<Pending> = roots
            .iter()
            .rev()
            .map(|root| Pending::Value(*root))
            .collect();
        while let Some(pending) = stack.pop() {
            match pending {
                Pending::Value(value) => {
                    table.visit_value(heap, value, platform, modules, &mut stack)
                }
                Pending::Frame(frame) => table.visit_frame(heap, frame, &mut stack),
            }
        }
        table
    }

    fn visit_value(
        &mut self,
        heap: &Heap<'gc>,
        value: Value<'gc>,
        platform: Value<'gc>,
        modules: &ModuleClasses<'gc>,
        stack: &mut Vec<Pending<'gc>>,
    ) {
        // A resolved future is never entered: every reference to one is
        // written as its value (`encode_value`), so its value is what the
        // graph holds.
        let value = future::seen_through(heap, value);
        let Some(object) = value.as_object() else {
            return;
        };
        // A value, not a record: `encode_value` writes its digits, or a
        // boxed float's bits, wherever it is referenced, so it takes no
        // object id (§3.1.1).
        if heap.is_large_integer(object) || heap.is_boxed_float(object) {
            return;
        }
        if is_same_object(value, platform) {
            return;
        }
        // Named by its path, like the platform by its tag: the loading
        // platform answers it, so the image carries nothing of it.
        if self.use_platform_path(heap, object) {
            return;
        }
        if is_class_tag(heap, object, modules, &self.platform_paths) {
            if self.unnameable.is_none() {
                self.unnameable = unnameable_class(heap, object);
            }
            // The metaclass of a class a path names is written as a
            // reference through that path (`CLASS_METACLASS`).
            if let Some(this_class) = heap.class_this_class(object) {
                self.use_platform_path(heap, this_class);
            }
            return;
        }
        if let Some(refusal) = shipped_without_path(heap, object) {
            if self.shipped_without_path.is_none() {
                self.shipped_without_path = Some(refusal);
            }
            return;
        }
        let key = heap.object_identity_key(object);
        if self.object_ids.contains_key(&key) {
            return;
        }
        self.object_ids
            .insert(key, u32_field("object table id", self.objects.len()));
        self.objects.push(object);
        let before = stack.len();
        {
            let record = heap.object_snapshot(object);
            // An object's own class is a reference like every other: now
            // that a class object can be a record, reaching one only
            // through whatever values happen to point at it would leave
            // `encode_class_tag` with an id that was never assigned. An
            // activation object's is not walked: it is always the builtin
            // `Activation`, which its header names by tag and needs no id
            // for, and which `unnameable_class` refuses wherever else the
            // graph reaches it -- the header of its own instances is the one
            // place an image may name it. An unresolved future's is not
            // walked either, for the same reason: its header names the
            // kernel-private `Future` by tag.
            let activation = matches!(record.data, ObjectData::Activation(_));
            debug_assert!(
                !activation || heap.class_builtin(record.class) == Some(BuiltinClass::Activation),
                "an activation object's header names the builtin `Activation`"
            );
            let future = heap.class_builtin(record.class) == Some(BuiltinClass::Future);
            if !activation && !future {
                stack.push(Pending::Value(Value::object(record.class)));
            }
            match &record.data {
                ObjectData::Instance(fields) | ObjectData::Array(fields) => {
                    stack.extend(fields.iter().map(|word| Pending::Value(heap.load(*word))));
                }
                ObjectData::Closure(closure) => {
                    stack.push(Pending::Frame(closure.captured_frame));
                    // A closure's home is not walked: it is derived from
                    // the captured frame on both sides of the wire
                    // (`Heap::closure_home`), so it is never a reference of
                    // its own.
                    stack.extend(
                        closure
                            .copied
                            .iter()
                            .map(|word| Pending::Value(heap.load(*word))),
                    );
                }
                // A metaclass record writes one reference, its class:
                // everything else about it is derived from that class.
                ObjectData::Class(data) if data.this_class.is_some() => {
                    stack.extend(
                        data.this_class
                            .iter()
                            .map(|class| Pending::Value(Value::object(*class))),
                    );
                }
                // A class object's three references; its metaclass is its
                // header, pushed above like every object's class.
                ObjectData::Class(data) => {
                    stack.extend(
                        class_references(data).map(|class| Pending::Value(Value::object(class))),
                    );
                }
                ObjectData::Activation(frame) => stack.push(Pending::Frame(*frame)),
                ObjectData::String(_)
                | ObjectData::Symbol(_)
                | ObjectData::LargeInteger(_)
                | ObjectData::Float(_) => {}
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
            stack.push(Pending::Value(heap.load(snapshot.receiver)));
            stack.extend(
                snapshot
                    .arguments
                    .iter()
                    .chain(snapshot.temporaries.iter())
                    .map(|word| Pending::Value(heap.load(*word))),
            );
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
                    .map(|class| Pending::Value(Value::object(class))),
            );
            match &snapshot.kind {
                // The closure object a closure activation runs is a record
                // of its own, so it needs an id like any other object.
                FrameKind::Closure(closure) => {
                    stack.push(Pending::Value(Value::object(*closure)));
                }
                // The class-carrying kinds name a class the same way
                // `current_class` does, so the class needs an id the same way.
                FrameKind::Initializer(class)
                | FrameKind::SuperclassClause(class)
                | FrameKind::SuperclassMessage(class)
                | FrameKind::MixinApplicationMessage(class) => {
                    stack.push(Pending::Value(Value::object(*class)));
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

    /// Records that the graph names `object` by its platform path, when one
    /// reaches it, and answers whether one does. The first use fixes the
    /// path's position in the section.
    fn use_platform_path(&mut self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> bool {
        let Some(id) = self.platform_paths.id_of(heap, object) else {
            return false;
        };
        if !self.used_path_index.contains_key(&id) {
            let index = u32_field("platform path index", self.used_paths.len());
            self.used_path_index.insert(id, index);
            self.used_paths.push(id);
        }
        true
    }

    /// The position in the platform path section of the path that names
    /// `object`, when the graph names it by one.
    pub fn platform_path_index(
        &self,
        heap: &Heap<'gc>,
        object: ObjectReference<'gc>,
    ) -> Option<u32> {
        self.platform_paths
            .id_of(heap, object)
            .and_then(|id| self.used_path_index.get(&id).copied())
    }

    /// The paths the graph uses, in section order, with the objects they
    /// reach.
    pub fn used_platform_paths(&self) -> UsedPlatformPaths<'gc> {
        UsedPlatformPaths {
            paths: self
                .used_paths
                .iter()
                .map(|id| self.platform_paths.path(*id).to_vec())
                .collect(),
            objects: self
                .used_paths
                .iter()
                .map(|id| self.platform_paths.object(*id))
                .collect(),
        }
    }

    /// The class no program may name that the traversal reached, described
    /// for the refusal; `None` when it reached none.
    pub fn unnameable_class(&self) -> Option<&str> {
        self.unnameable.as_deref()
    }

    pub fn objects(&self) -> &[ObjectReference<'gc>] {
        &self.objects
    }

    pub fn frames(&self) -> &[FrameReference<'gc>] {
        &self.frames
    }
}

/// Everything the graph encoder resolves a reference against: the object
/// and frame tables, the `platform` singleton and the module classes (each
/// named by a tag of its own rather than by an id), the code table's ids,
/// and the mixin table's row ids -- which is how a class object's record
/// names the declaration it applies.
pub struct EncodeContext<'a, 'gc> {
    pub table: &'a GraphTable<'gc>,
    pub code_ids: &'a CodeIds,
    pub platform: Value<'gc>,
    /// Each carried module's root class and levels: what a class tag names
    /// by module index (`CLASS_MODULE`, `CLASS_MODULE_LEVEL`).
    pub modules: &'a ModuleClasses<'gc>,
    /// `mixin::mixin_key` -> mixin-table row id, as `mixins::encode`
    /// assigned them.
    pub mixin_ids: &'a HashMap<usize, u32>,
    pub heap: &'a Heap<'gc>,
}

/// `Builtin` (0 + kind tag), `Record` (2 + object id), `Metaclass` (4 + the
/// reference of a prebuilt class), `PlatformPath` (5 + path index), `Module`
/// (6 + module index) or `ModuleLevel` (7 + module index + position) -- the
/// ways a class object can exist in a loaded world. A metaclass of a class
/// record is a record itself, so a metaclass reference names a prebuilt
/// class, never another metaclass.
///
/// All but `Record` exist before the graph is decoded: a builtin comes from
/// the bootstrap, a path-named class from the loading platform, and each
/// module's root class and levels from its tree of the mixin table plus its
/// root section (`decode::build_root_class`).
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
    let Some(this_class) = context.heap.class_this_class(class) else {
        match prebuilt_class(class, context) {
            Some(prebuilt) => write_prebuilt_class(writer, prebuilt),
            None => {
                writer.write_u8(CLASS_RECORD);
                writer.write_u32(context.table.object_id(context.heap, class));
            }
        }
        return;
    };
    // A metaclass a path names is written through that path, ahead of its
    // class's shape: the graph never entered a class a path names, so it has
    // no record to name.
    if let Some(index) = context.table.platform_path_index(context.heap, class) {
        writer.write_u8(CLASS_PLATFORM_PATH);
        writer.write_u32(index);
        return;
    }
    match prebuilt_class(this_class, context) {
        Some(prebuilt) => {
            writer.write_u8(CLASS_METACLASS);
            write_prebuilt_class(writer, prebuilt);
        }
        None => {
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(context.table.object_id(context.heap, class));
        }
    }
}

/// How a class tag names a class the loading world has before the graph is
/// decoded -- what `decode::read_prebuilt_class` reads back.
enum PrebuiltClass {
    PlatformPath(u32),
    Builtin(BuiltinClass),
    Module(ModuleClass),
}

/// `class`, a class rather than a metaclass, as a prebuilt class, or `None`
/// when the graph carries it as a record. Each lookup runs once: a path
/// first, since the graph never entered a class a path names, then the
/// builtin tag, then the module classes.
fn prebuilt_class<'gc>(
    class: ClassReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
) -> Option<PrebuiltClass> {
    if let Some(index) = context.table.platform_path_index(context.heap, class) {
        return Some(PrebuiltClass::PlatformPath(index));
    }
    if let Some(kind) = context.heap.class_builtin(class) {
        return Some(PrebuiltClass::Builtin(kind));
    }
    context.modules.find(class).map(PrebuiltClass::Module)
}

fn write_prebuilt_class(writer: &mut Writer, prebuilt: PrebuiltClass) {
    match prebuilt {
        PrebuiltClass::PlatformPath(index) => {
            writer.write_u8(CLASS_PLATFORM_PATH);
            writer.write_u32(index);
        }
        PrebuiltClass::Builtin(kind) => {
            writer.write_u8(CLASS_BUILTIN);
            writer.write_u8(builtin_class_tag(kind));
        }
        PrebuiltClass::Module(ModuleClass::Root(module)) => {
            writer.write_u8(CLASS_MODULE);
            writer.write_u32(module);
        }
        PrebuiltClass::Module(ModuleClass::Level { module, position }) => {
            writer.write_u8(CLASS_MODULE_LEVEL);
            writer.write_u32(module);
            writer.write_u32(position);
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
/// 5 = object-table id, 6 = class tag, 7 = the `platform` singleton, 8 = an
/// object a platform path names, a class object's included, 9 = a
/// `LargeInteger`'s digits. A boxed float travels as tag 4, as every float
/// does.
///
/// A class object always goes through tag 6, even the ones tag 6 then
/// resolves to an object-table id: one rule per kind of referent is what
/// keeps the encoding canonical.
fn encode_value<'gc>(
    writer: &mut Writer,
    value: Value<'gc>,
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
        match integer.to_i64() {
            Some(number) => {
                writer.write_u8(VALUE_SMALL_INTEGER);
                writer.write_i64(number);
            }
            None => {
                writer.write_u8(VALUE_LARGE_INTEGER);
                write_large_integer(writer, &integer)?;
            }
        }
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
    if is_same_object(value, context.platform) {
        writer.write_u8(VALUE_PLATFORM);
        return Ok(());
    }
    if let Some(index) = context.table.platform_path_index(context.heap, object) {
        writer.write_u8(VALUE_PLATFORM_PATH);
        writer.write_u32(index);
        return Ok(());
    }
    if context.heap.is_class(object) {
        writer.write_u8(VALUE_CLASS_TAG);
        encode_class_tag(writer, object, context);
        return Ok(());
    }
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(context.table.object_id(context.heap, object));
    Ok(())
}

fn encode_values<'gc>(
    writer: &mut Writer,
    values: &[Value<'gc>],
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    writer.write_u32(u32_length("value list", values.len())?);
    for value in values {
        encode_value(writer, *value, context)?;
    }
    Ok(())
}

/// `encode_values` for what a heap object stores, read through
/// `Heap::load`.
fn encode_words<'gc>(
    writer: &mut Writer,
    words: &[Word],
    context: &EncodeContext<'_, 'gc>,
) -> Result<(), String> {
    writer.write_u32(u32_length("value list", words.len())?);
    for word in words {
        encode_value(writer, context.heap.load(*word), context)?;
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
) -> Result<(), String> {
    // Destructured, not read field by field: a new `ClosureData` field must
    // say here whether it belongs on the wire, rather than silently being
    // left off it.
    let ClosureData {
        code,
        captured_frame,
        copied,
        feedback_base: _, // never on the wire; see `decode_closure_record`
    } = closure;
    writer.write_u32(context.code_ids.id_of(code)?);
    writer.write_u32(context.table.frame_id(context.heap, *captured_frame));
    writer.write_u16(u16_field("closure copied value count", copied.len())?);
    for word in copied.iter() {
        encode_value(writer, context.heap.load(*word), context)?;
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
/// `builtin` is not written either -- a record only ever describes a user
/// class, since a builtin is a class *tag* -- and neither is its metaclass:
/// the record's header names it, as every record's header names its class
/// (`encode_object`).
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
                     which is not part of any module the image carries",
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
                 which is not part of any module the image carries",
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
            encode_value(writer, Value::object(*enclosing), context)?;
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
    // A class object -- never a metaclass, which always has one -- whose
    // header is not its metaclass has none: either it never got one
    // (`Mutator::new_class_object`, before the kernel phase's
    // `metaclasses_ready`) or something else is wrong with its pairing.
    // Writing the placeholder header anyway would silently claim it as the
    // metaclass every class record's header names (`encode_object`'s next
    // line), so this is refused up front rather than left for the loader
    // to reject an image the encoder should never have written.
    if let ObjectData::Class(data) = &record.data {
        if data.this_class.is_none() && context.heap.class_metaclass(object).is_none() {
            return Err(
                "image encoding cannot serialize a class object without a metaclass".to_string(),
            );
        }
    }
    // Every header is the object's class: a class record's names its
    // metaclass, and a metaclass record's names `Metaclass` (§3.2).
    encode_class_tag(writer, record.class, context);
    writer.write_u32(record.identity_hash);
    match &record.data {
        // An unresolved future -- a resolved one is never a record -- is
        // written unresolved even while its block runs
        // (`future::fields_for_an_image`). One whose fields crafted code
        // corrupted is refused: the decoder would refuse it.
        ObjectData::Instance(_)
            if context.heap.class_builtin(record.class) == Some(BuiltinClass::Future) =>
        {
            let Some(fields) = future::fields_for_an_image(context.heap, object) else {
                return Err("image encoding cannot serialize a malformed future".to_string());
            };
            writer.write_u8(OBJECT_INSTANCE);
            encode_values(writer, &fields, context)?;
        }
        ObjectData::Instance(fields) => {
            writer.write_u8(OBJECT_INSTANCE);
            encode_words(writer, fields, context)?;
        }
        ObjectData::String(string) => {
            writer.write_u8(OBJECT_STRING);
            // The wire keeps UTF-8 (`docs/design/image-format.md`); the
            // width is the heap's, chosen afresh when the record is decoded.
            let text = string.to_utf8();
            u32_length("string", text.len())?;
            writer.write_str(&text);
        }
        ObjectData::Symbol(symbol) => {
            writer.write_u8(OBJECT_SYMBOL);
            // A program can make a symbol of any string (`asSymbol`); the
            // symbol table writes its text with a `u32` length.
            u32_length("symbol", symbol.as_str().len())?;
            writer.write_u32(symbols.intern(symbol.as_str()));
        }
        ObjectData::Array(elements) => {
            writer.write_u8(OBJECT_ARRAY);
            encode_words(writer, elements, context)?;
        }
        ObjectData::Closure(closure) => {
            writer.write_u8(OBJECT_CLOSURE);
            encode_closure_record(writer, closure, context)?;
        }
        ObjectData::Class(data) if data.this_class.is_some() => {
            writer.write_u8(OBJECT_METACLASS);
            let this_class = data.this_class.expect("checked by the guard");
            encode_class_tag(writer, this_class, context);
        }
        ObjectData::Class(data) => {
            writer.write_u8(OBJECT_CLASS);
            encode_class_record(writer, data, context)?;
        }
        // One frame id, never absent: an activation object always reifies
        // a frame, so the record carries no presence flag.
        ObjectData::Activation(frame) => {
            writer.write_u8(OBJECT_ACTIVATION);
            writer.write_u32(context.table.frame_id(context.heap, *frame));
        }
        // No object record carries one: the graph walk gives a
        // `LargeInteger` no object id (`GraphTable::visit_value`), and
        // `encode_value` writes its digits instead, so that an image cannot
        // give two equal integers two identities.
        ObjectData::LargeInteger(_) => {
            unreachable!("a LargeInteger is written as a value, never as an object record")
        }
        // Likewise a boxed `Float`, whose bits `encode_value` writes.
        ObjectData::Float(_) => {
            unreachable!("a boxed Float is written as a value, never as an object record")
        }
    }
    Ok(())
}

fn encode_frame<'gc>(
    writer: &mut Writer,
    frame: FrameReference<'gc>,
    context: &EncodeContext<'_, 'gc>,
    symbols: &mut symbols::Builder,
) -> Result<(), String> {
    let snapshot = context.heap.frame_snapshot(frame);
    encode_value(writer, context.heap.load(snapshot.receiver), context)?;
    encode_words(writer, &snapshot.arguments, context)?;
    encode_words(writer, &snapshot.temporaries, context)?;
    writer.write_u32(u32_field("frame program counter", snapshot.program_counter));
    // An image's code table holds only compiled bodies, so a frame's
    // `program_counter` is always an instruction index and the record does not
    // say so.
    encode_frame_reference(
        context.heap,
        writer,
        snapshot.sender.as_ref(),
        context.table,
    );
    encode_frame_reference(context.heap, writer, snapshot.home.as_ref(), context.table);
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
    Ok(())
}

/// The platform paths a graph section uses, in the order its references
/// number them: what the platform path section lists
/// (`platform_paths::encode_section`), and the objects the closing hash table
/// records a hash for (`encode_tag_named_hashes`).
pub struct UsedPlatformPaths<'gc> {
    pub paths: Vec<Vec<Symbol>>,
    pub objects: Vec<ObjectReference<'gc>>,
}

/// The object table, the frame table, and the roots. Both counts come
/// first so the decoder can placeholder-allocate both tables before
/// filling either (objects and frames reference each other freely).
///
/// Answers the platform paths the section uses, which the caller writes
/// ahead of it: an object a path from `platform` reaches
/// (`platform_paths::discover`) is named by its index there, not written.
#[allow(clippy::too_many_arguments)]
pub fn encode_graph_section<'gc>(
    heap: &Heap<'gc>,
    writer: &mut Writer,
    roots: &[Value<'gc>],
    platform: Value<'gc>,
    code_ids: &CodeIds,
    modules: &ModuleClasses<'gc>,
    mixin_ids: &HashMap<usize, u32>,
    symbols: &mut symbols::Builder,
) -> Result<UsedPlatformPaths<'gc>, String> {
    let paths = match platform.as_object() {
        Some(platform) => platform_paths::discover(heap, platform),
        _ => PlatformPaths::empty(),
    };
    let table = GraphTable::build(heap, roots, platform, modules, paths);
    // The VM mirror is a capability the bootstrap hands to the core library
    // alone (`docs/design/core-library.md`, "The VM mirror is the whole Rust
    // boundary"); an artifact that carried it would hand it to whatever
    // loaded it. Checked ahead of the unnameable classes below, which a
    // graph reaching the mirror reaches too (its class), so that the
    // refusal names the capability rather than its class.
    for object in table.objects() {
        if heap.class_builtin(heap.object_class(*object)) == Some(BuiltinClass::VmMirror) {
            return Err(
                "image encoding refuses the VM mirror: it is a capability the \
                 bootstrap hands to the core library alone, and an artifact that \
                 carried it would hand it to whatever loaded it"
                    .to_string(),
            );
        }
    }
    if let Some(unnameable) = table.unnameable_class() {
        return Err(format!(
            "image encoding refuses {unnameable}: no program may name it, and the \
             loader refuses an image that does"
        ));
    }
    if let Some(refusal) = &table.shipped_without_path {
        return Err(refusal.clone());
    }
    let used = table.used_platform_paths();
    // `discover` records no path longer than a loader follows; the count is
    // the one bound left, and an image past it would load as nothing.
    if used.paths.len() > platform_paths::MAX_PLATFORM_PATHS {
        return Err(format!(
            "image encoding cannot name {} platform paths: a loader follows at most {}",
            used.paths.len(),
            platform_paths::MAX_PLATFORM_PATHS
        ));
    }
    let context = EncodeContext {
        table: &table,
        code_ids,
        platform,
        modules,
        mixin_ids,
        heap,
    };
    writer.write_u32(u32_field("object table entry count", table.objects().len()));
    writer.write_u32(u32_field("frame table entry count", table.frames().len()));
    for object in table.objects() {
        encode_object(writer, *object, &context, symbols)?;
    }
    for frame in table.frames() {
        encode_frame(writer, *frame, &context, symbols)?;
    }
    encode_values(writer, roots, &context)?;
    Ok(used)
}

/// The graph section's closing table: the identity hash of every object the
/// image names by tag rather than carries as a record -- each builtin class,
/// each module's root class and levels, the `platform` singleton, each object
/// a used platform path names, and the metaclasses of the builtins, the
/// module roots, their levels and the classes a used path names -- so that a
/// hash the program was given survives the load. Only an assigned hash (not
/// 0) is written, in a fixed order -- the builtin classes in wire-tag order,
/// then the module roots in module order, then their levels (module by
/// module, lowest first), then `platform`, then the path-named objects in
/// section order, then the builtins' metaclasses in wire-tag order, the
/// module roots' in module order, their levels' in the same order as the
/// levels themselves, and the path-named classes' in section order -- so one
/// world encodes to one byte sequence. Each object has at most one entry,
/// the first in that order -- a builtin class a path names keeps its builtin
/// entry, a path-named metaclass its path entry -- since the loader refuses
/// one object listed twice. See
/// `docs/design/image-format.md`, "Identity hashes are preserved verbatim".
///
/// Every builtin is considered, not only those the graph reaches: a program
/// can keep `Integer hash` in a slot without keeping `Integer`. The
/// metaclasses of `Top`, of the VM mirror's class and of `Activation` are
/// not: the decoder refuses those tags behind a metaclass reference, as it
/// does wherever the graph names a class, and no program can reach any of
/// the three metaclasses to give it a hash.
pub fn encode_tag_named_hashes<'gc>(
    mutator: &Mutator<'gc>,
    writer: &mut Writer,
    modules: &ModuleClasses<'gc>,
    platform: Value<'gc>,
    used_platform_objects: &[ObjectReference<'gc>],
) {
    let heap = &mutator.heap;
    let path_named: Vec<(u32, ObjectReference<'gc>)> = used_platform_objects
        .iter()
        .enumerate()
        .map(|(index, object)| (u32_field("platform path index", index), *object))
        .collect();
    // One entry per object: a builtin class a path names, or a metaclass a
    // path names and a prebuilt class's reference reaches as well, is listed
    // once, where the fixed order first reaches it.
    let mut entered: HashSet<usize> = HashSet::new();
    let mut entries: Vec<(u8, Vec<u8>, u32)> = Vec::new();
    for kind in BuiltinClass::ALL {
        let class = mutator.builtin_class(kind);
        entered.insert(heap.object_identity_key(class));
        let hash = heap.object_identity_hash(class);
        if hash != 0 {
            entries.push((HASHED_BUILTIN_CLASS, vec![builtin_class_tag(kind)], hash));
        }
    }
    for (module, root) in modules.roots.iter().enumerate() {
        entered.insert(heap.object_identity_key(*root));
        let hash = heap.object_identity_hash(*root);
        if hash != 0 {
            let module = u32_field("module index", module);
            entries.push((HASHED_MODULE_CLASS, module.to_le_bytes().to_vec(), hash));
        }
    }
    // A level is no record either, so without its own entry the hash a
    // program read off it -- `self class superclass hash` in a module
    // applying `<: Value` -- would change across an image.
    for (module, levels) in modules.levels.iter().enumerate() {
        for (position, level) in levels.iter().enumerate() {
            entered.insert(heap.object_identity_key(*level));
            let hash = heap.object_identity_hash(*level);
            if hash != 0 {
                let mut reference = Writer::new();
                reference.write_u32(u32_field("module index", module));
                reference.write_u32(u32_field("module level position", position));
                entries.push((HASHED_MODULE_LEVEL, reference.into_bytes(), hash));
            }
        }
    }
    if let Some(platform) = platform.as_object() {
        entered.insert(heap.object_identity_key(platform));
        let hash = heap.object_identity_hash(platform);
        if hash != 0 {
            entries.push((HASHED_PLATFORM, Vec::new(), hash));
        }
    }
    for (index, object) in &path_named {
        if !entered.insert(heap.object_identity_key(*object)) {
            continue;
        }
        let hash = heap.object_identity_hash(*object);
        if hash != 0 {
            entries.push((HASHED_PLATFORM_PATH, index.to_le_bytes().to_vec(), hash));
        }
    }
    let mut prebuilt: Vec<(ClassReference<'gc>, Vec<u8>)> = BuiltinClass::ALL
        .into_iter()
        .filter(|kind| {
            !matches!(
                kind,
                BuiltinClass::Top
                    | BuiltinClass::VmMirror
                    | BuiltinClass::Activation
                    | BuiltinClass::Future
            )
        })
        .map(|kind| {
            (
                mutator.builtin_class(kind),
                vec![CLASS_BUILTIN, builtin_class_tag(kind)],
            )
        })
        .collect();
    for (module, root) in modules.roots.iter().enumerate() {
        let mut reference = Writer::new();
        reference.write_u8(CLASS_MODULE);
        reference.write_u32(u32_field("module index", module));
        prebuilt.push((*root, reference.into_bytes()));
    }
    for (module, levels) in modules.levels.iter().enumerate() {
        for (position, level) in levels.iter().enumerate() {
            let mut reference = Writer::new();
            reference.write_u8(CLASS_MODULE_LEVEL);
            reference.write_u32(u32_field("module index", module));
            reference.write_u32(u32_field("module level position", position));
            prebuilt.push((*level, reference.into_bytes()));
        }
    }
    for (index, object) in &path_named {
        if heap.is_class(*object) && heap.class_this_class(*object).is_none() {
            let mut reference = Writer::new();
            reference.write_u8(CLASS_PLATFORM_PATH);
            reference.write_u32(*index);
            prebuilt.push((*object, reference.into_bytes()));
        }
    }
    for (class, reference) in prebuilt {
        let Some(metaclass) = heap.class_metaclass(class) else {
            continue;
        };
        if !entered.insert(heap.object_identity_key(metaclass)) {
            continue;
        }
        let hash = heap.object_identity_hash(metaclass);
        if hash != 0 {
            entries.push((HASHED_METACLASS, reference, hash));
        }
    }
    writer.write_u32(u32_field("tag-named hash count", entries.len()));
    for (tag, reference, hash) in entries {
        writer.write_u8(tag);
        writer.write_raw(&reference);
        writer.write_u32(hash);
    }
}
