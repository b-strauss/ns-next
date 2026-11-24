//! The specification's wire encoding (`docs/spec/bytecode.md`, "The wire
//! encoding") held to the encoder: its tag table to `opcodes.rs`,
//! `code_kind_tag` and `integer_operation_tag`, and its layouts to the bytes
//! `encode_instruction` and `encode_literal` write.

use std::collections::BTreeSet;
use std::rc::Rc;

use ns_syntax::Span;

use super::*;
use crate::Symbol;
use crate::spec_tables::{INSTRUCTION_KINDS, bytecode_table, instruction_name};

/// The value of one field, as the sample was built with it.
#[derive(Clone)]
enum Field {
    Number(u64),
    Numbers(Vec<u64>),
    Text(&'static str),
    Texts(Vec<&'static str>),
    /// A `LoadLiteral`'s literal: `nil` in every sample.
    Nil,
    /// A `MakeClosure`'s body, whose code id the sample gives.
    Code(u32),
    /// A literal's floating-point payload, written by its bits.
    Float(f64),
    /// A large integer's sign and digits.
    Large(bool, Vec<u64>),
}

struct Sample {
    name: &'static str,
    instruction: Instruction,
    fields: Vec<(&'static str, Field)>,
}

fn sample(
    name: &'static str,
    instruction: Instruction,
    fields: &[(&'static str, Field)],
) -> Sample {
    Sample {
        name,
        instruction,
        fields: fields.to_vec(),
    }
}

use Field::{Nil, Number, Numbers, Text, Texts};

/// The block every `MakeClosure` sample creates, at code id 7.
fn block() -> Rc<CompiledCode> {
    Rc::new(CompiledCode::new(
        CodeKind::Block,
        vec![Instruction::ImplicitReturn { register: 14 }],
        1,
        15,
        0,
        vec![(0, 0..1)],
        Span::new(0, 1),
    ))
}

const CODE_ID: u32 = 7;

// One sample per instruction, read top to bottom.
#[expect(
    clippy::too_many_lines,
    reason = "one sample per instruction, each a few lines"
)]
fn every_instruction(block: &Rc<CompiledCode>) -> Vec<Sample> {
    let selector = Symbol::intern("with:with:");
    let site = CallSite { number: 0 };
    let window = || {
        vec![
            ("argument_base", Number(20)),
            ("argument_sources", Numbers(vec![4, 5])),
        ]
    };
    let with = |mut fields: Vec<(&'static str, Field)>, more: Vec<(&'static str, Field)>| {
        fields.extend(more);
        fields
    };
    vec![
        sample(
            "LoadLiteral",
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::Nil,
            },
            &[("destination", Number(1)), ("literal", Nil)],
        ),
        sample(
            "LoadSelf",
            Instruction::LoadSelf { destination: 1 },
            &[("destination", Number(1))],
        ),
        sample(
            "LoadEnclosingObject",
            Instruction::LoadEnclosingObject {
                destination: 1,
                depth: 2,
            },
            &[("destination", Number(1)), ("depth", Number(2))],
        ),
        sample(
            "Move",
            Instruction::Move {
                destination: 1,
                source: 2,
            },
            &[("destination", Number(1)), ("source", Number(2))],
        ),
        sample(
            "StoreLocal",
            Instruction::StoreLocal { source: 2, slot: 3 },
            &[("source", Number(2)), ("slot", Number(3))],
        ),
        sample(
            "LoadOuterLocal",
            Instruction::LoadOuterLocal {
                destination: 1,
                depth: 3,
                index: 4,
            },
            &[
                ("destination", Number(1)),
                ("depth", Number(3)),
                ("index", Number(4)),
            ],
        ),
        sample(
            "StoreOuterLocal",
            Instruction::StoreOuterLocal {
                source: 2,
                depth: 3,
                index: 4,
            },
            &[
                ("source", Number(2)),
                ("depth", Number(3)),
                ("index", Number(4)),
            ],
        ),
        sample(
            "SendLexical",
            Instruction::SendLexical {
                depth: 1,
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &with(
                vec![("depth", Number(1)), ("selector", Text("with:with:"))],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
        sample(
            "SendPublic",
            Instruction::SendPublic {
                selector,
                receiver_register: 3,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &with(
                vec![
                    ("selector", Text("with:with:")),
                    ("receiver_register", Number(3)),
                ],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
        sample(
            "SendInlinable",
            Instruction::SendInlinable {
                selector,
                receiver_register: 3,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &with(
                vec![
                    ("selector", Text("with:with:")),
                    ("receiver_register", Number(3)),
                ],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
        sample(
            "SendSuper",
            Instruction::SendSuper {
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &with(
                vec![("selector", Text("with:with:"))],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
        sample(
            "MakeClosure",
            Instruction::MakeClosure {
                destination: 1,
                parameters: Rc::new(vec![Symbol::intern("element")]),
                code: Rc::clone(block),
                copied: vec![4, 5],
            },
            &[
                ("destination", Number(1)),
                ("parameters", Texts(vec!["element"])),
                ("code", Field::Code(CODE_ID)),
                ("copied", Numbers(vec![4, 5])),
            ],
        ),
        sample(
            "MakeArray",
            Instruction::MakeArray {
                destination: 1,
                element_registers: vec![4, 5, 7],
            },
            &[
                ("destination", Number(1)),
                ("element_registers", Numbers(vec![4, 5, 7])),
            ],
        ),
        sample(
            "MakeVector",
            Instruction::MakeVector {
                destination: 1,
                size: 3,
            },
            &[("destination", Number(1)), ("size", Number(3))],
        ),
        sample(
            "LoadVector",
            Instruction::LoadVector {
                destination: 1,
                vector: 2,
                index: 3,
            },
            &[
                ("destination", Number(1)),
                ("vector", Number(2)),
                ("index", Number(3)),
            ],
        ),
        sample(
            "StoreVector",
            Instruction::StoreVector {
                vector: 2,
                index: 3,
                source: 4,
            },
            &[
                ("vector", Number(2)),
                ("index", Number(3)),
                ("source", Number(4)),
            ],
        ),
        sample(
            "InitializeSlot",
            Instruction::InitializeSlot {
                source: 2,
                own_index: 70_000,
            },
            &[("source", Number(2)), ("own_index", Number(70_000))],
        ),
        sample(
            "ReturnSlotIfFilled",
            Instruction::ReturnSlotIfFilled { own_index: 70_000 },
            &[("own_index", Number(70_000))],
        ),
        sample(
            "MakeFuture",
            Instruction::MakeFuture {
                destination: 1,
                closure: 2,
            },
            &[("destination", Number(1)), ("closure", Number(2))],
        ),
        sample(
            "ResolveFuture",
            Instruction::ResolveFuture {
                destination: 1,
                source: 2,
            },
            &[("destination", Number(1)), ("source", Number(2))],
        ),
        sample(
            "LoadOwnSlot",
            Instruction::LoadOwnSlot {
                destination: 1,
                own_index: 70_000,
            },
            &[("destination", Number(1)), ("own_index", Number(70_000))],
        ),
        sample(
            "InitializeSlotProvisionally",
            Instruction::InitializeSlotProvisionally {
                source: 2,
                own_index: 70_000,
            },
            &[("source", Number(2)), ("own_index", Number(70_000))],
        ),
        sample(
            "Return",
            Instruction::Return { register: 2 },
            &[("register", Number(2))],
        ),
        sample(
            "ImplicitReturn",
            Instruction::ImplicitReturn { register: 2 },
            &[("register", Number(2))],
        ),
        sample(
            "NonLocalReturn",
            Instruction::NonLocalReturn { register: 2 },
            &[("register", Number(2))],
        ),
        sample(
            "SignalNonBoolean",
            Instruction::SignalNonBoolean { register: 2 },
            &[("register", Number(2))],
        ),
        sample(
            "Jump",
            Instruction::Jump { target: 70_000 },
            &[("target", Number(70_000))],
        ),
        sample(
            "JumpIfNotBoolean",
            Instruction::JumpIfNotBoolean {
                register: 2,
                target: 70_000,
            },
            &[("register", Number(2)), ("target", Number(70_000))],
        ),
        sample(
            "BranchIfFalse",
            Instruction::BranchIfFalse {
                register: 2,
                target: 70_000,
            },
            &[("register", Number(2)), ("target", Number(70_000))],
        ),
        sample(
            "BranchIfTrue",
            Instruction::BranchIfTrue {
                register: 2,
                target: 70_000,
            },
            &[("register", Number(2)), ("target", Number(70_000))],
        ),
        sample(
            "IntegerBinary",
            Instruction::IntegerBinary {
                operation: IntegerOperation::GreaterOrEqual,
                destination: 1,
                left: 2,
                right: 3,
                target: 70_000,
            },
            &[
                ("operation", Number(7)),
                ("destination", Number(1)),
                ("left", Number(2)),
                ("right", Number(3)),
                ("target", Number(70_000)),
            ],
        ),
        sample(
            "AllocateInstance",
            Instruction::AllocateInstance { destination: 1 },
            &[("destination", Number(1))],
        ),
        sample(
            "InitializeInstance",
            Instruction::InitializeInstance {
                instance: 2,
                destination: 1,
            },
            &[("instance", Number(2)), ("destination", Number(1))],
        ),
        sample(
            "CheckAppliedClause",
            Instruction::CheckAppliedClause { steps: 2, selector },
            &[("steps", Number(2)), ("selector", Text("with:with:"))],
        ),
        sample(
            "InitializeSuperclass",
            Instruction::InitializeSuperclass {
                steps: 2,
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
            },
            &with(
                vec![("steps", Number(2)), ("selector", Text("with:with:"))],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
        sample(
            "InitializeAppliedLevel",
            Instruction::InitializeAppliedLevel {
                steps: 2,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
            },
            &with(
                vec![("steps", Number(2))],
                with(window(), vec![("destination", Number(6))]),
            ),
        ),
    ]
}

/// A literal under its row's name, with the fields of its payload.
type LiteralSample = (&'static str, Literal, Vec<(&'static str, Field)>);

/// One literal of every kind.
fn every_literal() -> Vec<LiteralSample> {
    vec![
        ("Nil", Literal::Nil, vec![]),
        (
            "Boolean",
            Literal::Boolean(true),
            vec![("value", Number(1))],
        ),
        (
            "SmallInteger",
            Literal::SmallInteger(-3),
            vec![("value", Number((-3_i64).cast_unsigned()))],
        ),
        (
            "Float",
            Literal::Float(2.5),
            vec![("value", Field::Float(2.5))],
        ),
        (
            "String",
            Literal::String("text".to_string()),
            vec![("text", Text("text"))],
        ),
        (
            "Symbol",
            Literal::Symbol(Symbol::intern("name")),
            vec![("symbol", Text("name"))],
        ),
        (
            "UnresolvedIdentifier",
            Literal::UnresolvedIdentifier("Missing".to_string()),
            vec![("name", Text("Missing"))],
        ),
        (
            "LargeInteger",
            Literal::LargeInteger(ArbitraryInteger::from_parts(true, vec![1, 2])),
            vec![("value", Field::Large(true, vec![1, 2]))],
        ),
        (
            "BodyTooLarge/Registers",
            Literal::BodyTooLarge(BodyLimit::Registers),
            vec![],
        ),
        (
            "BodyTooLarge/Arguments",
            Literal::BodyTooLarge(BodyLimit::Arguments),
            vec![],
        ),
        (
            "BodyTooLarge/Instructions",
            Literal::BodyTooLarge(BodyLimit::Instructions),
            vec![],
        ),
        (
            "Unbuilt/AsynchronousSend",
            Literal::Unbuilt(Unbuilt::AsynchronousSend(Symbol::intern("send:"))),
            vec![("selector", Text("send:"))],
        ),
        (
            "Unbuilt/PatternLiteral",
            Literal::Unbuilt(Unbuilt::PatternLiteral),
            vec![],
        ),
    ]
}

/// The bytes a row's cells say its fields are written as, the symbols
/// interned in the order the cells name them.
fn expected_bytes(
    row_name: &str,
    cells: &[String],
    fields: &[(&'static str, Field)],
    symbols: &mut symbols::Builder,
) -> Vec<u8> {
    let mut writer = Writer::new();
    for cell in cells {
        let (name, kind) = cell
            .split_once(':')
            .unwrap_or_else(|| panic!("{row_name}: `{cell}` is no `field:type`"));
        let value = fields
            .iter()
            .find(|(field, _)| *field == name)
            .unwrap_or_else(|| panic!("{row_name}: the sample has no field `{name}`"))
            .1
            .clone();
        match (kind, value) {
            ("u8", Number(number)) => {
                writer.write_u8(u8::try_from(number).unwrap_or_else(|_| unfit(row_name, cell)))
            }
            ("u16", Number(number)) => {
                writer.write_u16(u16::try_from(number).unwrap_or_else(|_| unfit(row_name, cell)))
            }
            ("u32", Number(number)) => {
                writer.write_u32(u32::try_from(number).unwrap_or_else(|_| unfit(row_name, cell)))
            }
            ("i64", Number(number)) => writer.write_u64(number),
            ("f64", Field::Float(float)) => writer.write_f64(float),
            ("symbol", Text(text)) => writer.write_u32(symbols.intern(text)),
            ("bytes", Text(text)) => writer.write_str(text),
            ("code", Field::Code(id)) => writer.write_u32(id),
            ("literal", Nil) => writer.write_u8(LITERAL_NIL),
            ("registers", Numbers(registers)) => {
                writer.write_u16(
                    u16::try_from(registers.len()).unwrap_or_else(|_| unfit(row_name, cell)),
                );
                for register in registers {
                    writer.write_u16(
                        u16::try_from(register).unwrap_or_else(|_| unfit(row_name, cell)),
                    );
                }
            }
            ("symbols", Texts(texts)) => {
                writer.write_u16(
                    u16::try_from(texts.len()).unwrap_or_else(|_| unfit(row_name, cell)),
                );
                for text in texts {
                    writer.write_u32(symbols.intern(text));
                }
            }
            ("large-integer", Field::Large(negative, digits)) => {
                writer.write_boolean(negative);
                writer.write_u32(
                    u32::try_from(digits.len()).unwrap_or_else(|_| unfit(row_name, cell)),
                );
                for digit in digits {
                    writer.write_u64(digit);
                }
            }
            _ => unfit(row_name, cell),
        }
    }
    writer.into_bytes()
}

/// A cell whose type does not fit the sample's value.
fn unfit(row_name: &str, cell: &str) -> ! {
    panic!("{row_name}: `{cell}` does not fit the sample's value")
}

/// The tag the wire-tag table gives `name` in `space`.
fn tag_of(space: &str, name: &str) -> u8 {
    let rows = bytecode_table("wire-tags");
    let row = rows
        .iter()
        .find(|row| row[0] == space && row[1] == name)
        .unwrap_or_else(|| panic!("the wire tags have no {space} {name}"));
    row[2].parse().expect("a tag is a number")
}

/// The rows of a layout table, by their first cell.
fn layouts() -> std::collections::BTreeMap<String, Vec<String>> {
    bytecode_table("wire-layouts")
        .into_iter()
        .map(|mut row| {
            let name = row.remove(0);
            (name, row)
        })
        .collect()
}

// spec: bytecode.wire.tags
// spec: bytecode.wire.tag-assigned-once
#[test]
fn the_books_wire_tags_are_the_encoders() {
    let mut named = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for row in bytecode_table("wire-tags") {
        assert_eq!(row.len(), 3, "a row of three cells: {row:?}");
        let number: u8 = row[2].parse().expect("a tag is a number");
        if row[1] == "retired" {
            retired.insert((row[0].clone(), number));
        } else {
            named.insert((row[0].clone(), row[1].clone(), number));
        }
    }
    let instruction = |name: &str, tag: u8| ("instruction".to_string(), name.to_string(), tag);
    let literal = |name: &str, tag: u8| ("literal".to_string(), name.to_string(), tag);
    let mut expected: BTreeSet<_> = [
        instruction("LoadLiteral", OPCODE_LOAD_LITERAL),
        instruction("LoadSelf", OPCODE_LOAD_SELF),
        instruction("Move", OPCODE_MOVE),
        instruction("StoreLocal", OPCODE_STORE_LOCAL),
        instruction("LoadOuterLocal", OPCODE_LOAD_OUTER_LOCAL),
        instruction("StoreOuterLocal", OPCODE_STORE_OUTER_LOCAL),
        instruction("MakeArray", OPCODE_MAKE_ARRAY),
        instruction("InitializeSlot", OPCODE_INITIALIZE_SLOT),
        instruction("Return", OPCODE_RETURN),
        instruction("ImplicitReturn", OPCODE_IMPLICIT_RETURN),
        instruction("NonLocalReturn", OPCODE_NON_LOCAL_RETURN),
        instruction("Jump", OPCODE_JUMP),
        instruction("JumpIfNotBoolean", OPCODE_JUMP_IF_NOT_BOOLEAN),
        instruction("BranchIfFalse", OPCODE_BRANCH_IF_FALSE),
        instruction("BranchIfTrue", OPCODE_BRANCH_IF_TRUE),
        instruction("SendLexical", OPCODE_SEND_LEXICAL),
        instruction("SendPublic", OPCODE_SEND_PUBLIC),
        instruction("SendInlinable", OPCODE_SEND_INLINABLE),
        instruction("SendSuper", OPCODE_SEND_SUPER),
        instruction("MakeClosure", OPCODE_MAKE_CLOSURE),
        instruction("MakeVector", OPCODE_MAKE_VECTOR),
        instruction("LoadVector", OPCODE_LOAD_VECTOR),
        instruction("StoreVector", OPCODE_STORE_VECTOR),
        instruction("IntegerBinary", OPCODE_INTEGER_BINARY),
        instruction("ReturnSlotIfFilled", OPCODE_RETURN_SLOT_IF_FILLED),
        instruction("LoadEnclosingObject", OPCODE_LOAD_ENCLOSING_OBJECT),
        instruction("MakeFuture", OPCODE_MAKE_FUTURE),
        instruction("ResolveFuture", OPCODE_RESOLVE_FUTURE),
        instruction("LoadOwnSlot", OPCODE_LOAD_OWN_SLOT),
        instruction(
            "InitializeSlotProvisionally",
            OPCODE_INITIALIZE_SLOT_PROVISIONALLY,
        ),
        instruction("SignalNonBoolean", OPCODE_SIGNAL_NON_BOOLEAN),
        instruction("AllocateInstance", OPCODE_ALLOCATE_INSTANCE),
        instruction("InitializeInstance", OPCODE_INITIALIZE_INSTANCE),
        instruction("CheckAppliedClause", OPCODE_CHECK_APPLIED_CLAUSE),
        instruction("InitializeSuperclass", OPCODE_INITIALIZE_SUPERCLASS),
        instruction("InitializeAppliedLevel", OPCODE_INITIALIZE_APPLIED_LEVEL),
        literal("Nil", LITERAL_NIL),
        literal("Boolean", LITERAL_BOOLEAN),
        literal("SmallInteger", LITERAL_SMALL_INTEGER),
        literal("Float", LITERAL_FLOAT),
        literal("String", LITERAL_STRING),
        literal("Symbol", LITERAL_SYMBOL),
        literal("UnresolvedIdentifier", LITERAL_UNRESOLVED_IDENTIFIER),
        literal("LargeInteger", LITERAL_LARGE_INTEGER),
        literal("BodyTooLarge/Registers", LITERAL_BODY_TOO_LARGE),
        literal(
            "BodyTooLarge/Arguments",
            LITERAL_BODY_PAST_THE_ARGUMENT_LIMIT,
        ),
        literal(
            "BodyTooLarge/Instructions",
            LITERAL_BODY_PAST_THE_INSTRUCTION_LIMIT,
        ),
        literal(
            "Unbuilt/AsynchronousSend",
            LITERAL_UNBUILT_ASYNCHRONOUS_SEND,
        ),
        literal("Unbuilt/PatternLiteral", LITERAL_UNBUILT_PATTERN_LITERAL),
    ]
    .into_iter()
    .collect();
    for (name, kind) in [
        ("Method", CodeKind::Method),
        ("Block", CodeKind::Block),
        ("Initializer", CodeKind::Initializer),
        ("SuperclassClause", CodeKind::SuperclassClause),
    ] {
        expected.insert((
            "code-kind".to_string(),
            name.to_string(),
            code_kind_tag(kind),
        ));
    }
    for (name, operation) in [
        ("Add", IntegerOperation::Add),
        ("Subtract", IntegerOperation::Subtract),
        ("Multiply", IntegerOperation::Multiply),
        ("FloorDivide", IntegerOperation::FloorDivide),
        ("Less", IntegerOperation::Less),
        ("Greater", IntegerOperation::Greater),
        ("LessOrEqual", IntegerOperation::LessOrEqual),
        ("GreaterOrEqual", IntegerOperation::GreaterOrEqual),
        ("Equal", IntegerOperation::Equal),
    ] {
        expected.insert((
            "integer-operation".to_string(),
            name.to_string(),
            integer_operation_tag(operation),
        ));
    }
    assert_eq!(
        expected
            .iter()
            .filter(|(space, ..)| space == "instruction")
            .count(),
        INSTRUCTION_KINDS - 1,
        "a tag for every kind of instruction but `SendMirror`"
    );
    assert_eq!(named, expected, "the named tags");
    let retired_expected: BTreeSet<_> = RETIRED_OPCODES
        .iter()
        .map(|tag| ("instruction".to_string(), *tag))
        .chain(
            RETIRED_LITERALS
                .iter()
                .map(|tag| ("literal".to_string(), *tag)),
        )
        .chain(
            RETIRED_CODE_KIND_TAGS
                .iter()
                .map(|tag| ("code-kind".to_string(), *tag)),
        )
        .collect();
    assert_eq!(retired, retired_expected, "the retired tags");
    for (space, name, number) in &named {
        assert!(
            !retired.contains(&(space.clone(), *number)),
            "{space} {name} takes the retired number {number}"
        );
    }
}

// spec: bytecode.wire.instruction
// spec: bytecode.wire.literal
#[test]
fn each_instruction_is_written_as_the_book_lays_it_out() {
    let block = block();
    let mut ids = CodeIds::default();
    ids.ids.insert(Rc::as_ptr(&block) as usize, CODE_ID);
    let layouts = layouts();
    let samples = every_instruction(&block);
    let kinds: BTreeSet<_> = samples
        .iter()
        .map(|sample| instruction_name(&sample.instruction))
        .collect();
    // Every kind but `SendMirror`, which has no wire form.
    assert_eq!(
        kinds.len(),
        INSTRUCTION_KINDS - 1,
        "a sample of every kind of instruction an image carries"
    );
    for sample in &samples {
        assert_eq!(
            sample.name,
            instruction_name(&sample.instruction),
            "the sample's name is its kind's"
        );
        let cells = layouts
            .get(sample.name)
            .unwrap_or_else(|| panic!("the wire layouts have no row {}", sample.name));
        let mut written = Writer::new();
        let mut symbols = symbols::Builder::new();
        encode_instruction(&mut written, &sample.instruction, &ids, &mut symbols)
            .expect("the sample encodes");
        let mut expected = vec![tag_of("instruction", sample.name)];
        expected.extend(expected_bytes(
            sample.name,
            cells,
            &sample.fields,
            &mut symbols::Builder::new(),
        ));
        assert_eq!(written.into_bytes(), expected, "{}", sample.name);
    }
    for (name, literal, fields) in every_literal() {
        let row = format!("literal/{name}");
        let cells = layouts
            .get(&row)
            .unwrap_or_else(|| panic!("the wire layouts have no row {row}"));
        let mut written = Writer::new();
        encode_literal(&mut written, &literal, &mut symbols::Builder::new())
            .expect("the literal encodes");
        let mut expected = vec![tag_of("literal", name)];
        expected.extend(expected_bytes(
            &row,
            cells,
            &fields,
            &mut symbols::Builder::new(),
        ));
        assert_eq!(written.into_bytes(), expected, "{row}");
    }
    let named: BTreeSet<String> = samples
        .iter()
        .map(|sample| sample.name.to_string())
        .chain(
            every_literal()
                .into_iter()
                .map(|(name, ..)| format!("literal/{name}")),
        )
        .collect();
    assert_eq!(
        layouts.keys().cloned().collect::<BTreeSet<_>>(),
        named,
        "one layout row per instruction with a wire tag and per literal"
    );
}

// spec: bytecode.wire.body
#[test]
fn a_body_is_written_as_the_book_lays_it_out() {
    let code = CompiledCode::new(
        CodeKind::Method,
        vec![Instruction::Return { register: 13 }],
        1,
        20,
        0,
        vec![(0, 0..1)],
        Span::new(3, 9),
    );
    let mut written = Writer::new();
    encode_code(
        &mut written,
        &code,
        &CodeIds::default(),
        &mut symbols::Builder::new(),
    )
    .expect("the body encodes");
    let rows = bytecode_table("wire-body");
    assert_eq!(rows.len(), 1, "one row of the body's fields");
    let mut expected = Writer::new();
    for cell in &rows[0] {
        match cell.as_str() {
            "kind:u8" => expected.write_u8(code_kind_tag(CodeKind::Method)),
            "argument_count:u16" => expected.write_u16(1),
            "register_count:u16" => expected.write_u16(20),
            "copied_count:u16" => expected.write_u16(0),
            "local_count:u16" => expected.write_u16(14),
            "source_start:u32" => expected.write_u32(3),
            "source_end:u32" => expected.write_u32(9),
            "statement_map:statements" => {
                expected.write_u32(1);
                for number in [0, 0, 1] {
                    expected.write_u32(number);
                }
            }
            "uncached:u8" => expected.write_u8(0),
            "instruction_count:u32" => expected.write_u32(1),
            "instructions:instructions" => {
                expected.write_u8(OPCODE_RETURN);
                expected.write_u16(13);
            }
            other => panic!("the body's record has no field `{other}`"),
        }
    }
    assert_eq!(written.into_bytes(), expected.into_bytes());
}
