//! Image format round-trip tests, section by section: the header and its
//! scalars, the mixin table, the code table, the object and frame tables,
//! whole-image `snapshot`/`load`, and the two verification passes.

use indoc::{formatdoc, indoc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{
    CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal,
};
use ns_runtime::image;
use ns_runtime::image::opcodes::*;
use ns_runtime::image::{decode, encode, mixins, symbols};
use ns_runtime::SourceOrigin as Origin;
use ns_runtime::{
    ArbitraryInteger, BuiltinClass, ClassData, ClassReference, DispatchId, FrameKind, Heap,
    HostResult, HostSignal, Interpreter, Mixin, MixinApplication, Mutator, ObjectData, Root,
    SourceOrigin, Symbol, Value,
};
use ns_syntax::ast::AccessModifier;
use ns_syntax::Span;

#[test]
fn header_round_trips() {
    for (flags, has_source) in [(0u8, false), (encode::FLAG_HAS_SOURCE, true)] {
        let mut writer = encode::Writer::new();
        encode::write_header(&mut writer, flags);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let header = decode::read_header(&mut reader).expect("valid header");
        assert_eq!(header.has_source, has_source);
    }
}

/// An unknown flag bit governs a section this build cannot see, so
/// continuing would read every later field out of the wrong offset.
#[test]
fn an_unknown_header_flag_is_rejected_not_ignored() {
    let mut writer = encode::Writer::new();
    encode::write_header(&mut writer, 0x80);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut reader),
        Err(decode::DecodeError::UnknownHeaderFlags { flags: 0x80 })
    );
}

#[test]
fn a_bad_magic_is_rejected() {
    let bytes = vec![b'X', b'X', b'X', b'X', 1, 0, 0, 0];
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut reader),
        Err(decode::DecodeError::BadMagic)
    );
}

#[test]
fn a_version_mismatch_is_rejected_not_migrated() {
    let mut writer = encode::Writer::new();
    writer.write_raw(encode::MAGIC);
    writer.write_u32(9999);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut reader),
        Err(decode::DecodeError::VersionMismatch {
            found: 9999,
            expected: encode::FORMAT_VERSION
        })
    );
}

#[test]
fn a_truncated_image_is_an_error_not_a_panic() {
    let bytes = vec![b'N', b'S'];
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut reader),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

#[test]
fn every_scalar_value_variant_round_trips() {
    for value in [
        Value::Nil,
        Value::Boolean(true),
        Value::Boolean(false),
        Value::SmallInteger(-42),
        Value::Float(3.5),
    ] {
        let mut writer = encode::Writer::new();
        assert!(encode::encode_scalar_value(&mut writer, &value));
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(&mut reader).expect("decode");
        match (&value, &decoded) {
            (Value::Nil, Value::Nil) => {}
            (Value::Boolean(expected), Value::Boolean(actual)) => assert_eq!(expected, actual),
            (Value::SmallInteger(expected), Value::SmallInteger(actual)) => {
                assert_eq!(expected, actual)
            }
            (Value::Float(expected), Value::Float(actual)) => {
                assert_eq!(expected.to_bits(), actual.to_bits())
            }
            _ => panic!("round-trip mismatch"),
        }
    }
}

#[test]
fn integers_and_strings_are_little_endian_and_length_prefixed() {
    let mut writer = encode::Writer::new();
    writer.write_u16(0x0102);
    writer.write_u32(0x01020304);
    writer.write_i64(-1);
    writer.write_str("hi");
    writer.write_boolean(true);
    let bytes = writer.into_bytes();
    assert_eq!(&bytes[0..2], &[0x02, 0x01]);
    assert_eq!(&bytes[2..6], &[0x04, 0x03, 0x02, 0x01]);
    assert_eq!(&bytes[6..14], &[0xff; 8]);
    assert_eq!(&bytes[14..20], &[2, 0, 0, 0, b'h', b'i']);
    assert_eq!(bytes[20], 1);
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(reader.read_u16().unwrap(), 0x0102);
    assert_eq!(reader.read_u32().unwrap(), 0x01020304);
    assert_eq!(reader.read_i64().unwrap(), -1);
    assert_eq!(reader.read_str().unwrap(), "hi");
    assert!(reader.read_boolean().unwrap());
    assert_eq!(
        reader.read_u8(),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

#[test]
fn scalar_value_boundary_cases_round_trip() {
    for value in [
        Value::SmallInteger(i64::MIN),
        Value::SmallInteger(i64::MAX),
        Value::SmallInteger(0),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::Float(-0.0),
        Value::Float(0.0),
    ] {
        let mut writer = encode::Writer::new();
        assert!(encode::encode_scalar_value(&mut writer, &value));
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(&mut reader).expect("decode");
        match (&value, &decoded) {
            (Value::SmallInteger(expected), Value::SmallInteger(actual)) => {
                assert_eq!(expected, actual)
            }
            (Value::Float(expected), Value::Float(actual)) => {
                assert_eq!(expected.to_bits(), actual.to_bits())
            }
            _ => panic!("round-trip mismatch"),
        }
    }
}

#[test]
fn encoding_an_object_value_is_refused_not_scalarized() {
    // `Value::Object` is not a scalar; `encode_scalar_value` must return
    // `false` and write nothing, leaving `encode_value`, the object-graph
    // encoder, in charge.
    let mut interpreter = ns_runtime::Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = Value::Object(mutator.new_string("hi".to_string()));
        let mut writer = encode::Writer::new();
        assert!(!encode::encode_scalar_value(&mut writer, &object));
        assert_eq!(writer.into_bytes().len(), 0);
    });
}

#[test]
fn empty_and_non_ascii_strings_round_trip() {
    for text in ["", "h\u{e9}llo \u{4e16}\u{754c} \u{1f980}"] {
        let mut writer = encode::Writer::new();
        writer.write_str(text);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        assert_eq!(reader.read_str().unwrap(), text);
    }
}

#[test]
fn invalid_utf8_bytes_are_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_bytes(&[0xff, 0xfe, 0xfd]);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(reader.read_str(), Err(decode::DecodeError::InvalidUtf8));
}

#[test]
fn a_length_prefix_past_the_end_is_an_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1000);
    writer.write_raw(b"short");
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        reader.read_bytes(),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

#[test]
fn a_huge_length_prefix_does_not_overflow_or_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::MAX);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        reader.read_bytes(),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

#[test]
fn an_invalid_scalar_tag_is_an_error_not_a_panic() {
    let bytes = vec![200u8];
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_scalar_value(&mut reader) {
        Err(decode::DecodeError::InvalidTag {
            what: "value",
            tag: 200,
        }) => {}
        Err(error) => panic!("expected InvalidTag{{value, 200}}, got a different error: {error}"),
        Ok(_) => panic!("expected InvalidTag{{value, 200}}, got Ok"),
    }
}

#[test]
fn an_invalid_boolean_tag_is_an_error_not_a_panic() {
    let bytes = vec![7u8];
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        reader.read_boolean(),
        Err(decode::DecodeError::InvalidTag {
            what: "bool",
            tag: 7
        })
    );
}

// ---------------------------------------------------------------------------
// The code table and the root list.
// ---------------------------------------------------------------------------

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// The class object `root` holds, for the heap accessors and the encoder,
/// which take one. Fetched where it is used: a run may move it. Takes
/// `&Mutator` and is called from inside an entry, since every caller already
/// holds one by the time it needs a `ClassReference`.
fn class_reference<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ClassReference<'gc> {
    match mutator.fetch(root) {
        Value::Object(class) => class,
        _ => panic!("expected a class object"),
    }
}

/// A `symbols::Table` holding exactly `strings`, in order -- so `strings[i]`
/// decodes from id `i`, matching the ids a hand-crafted test writes inline.
/// Hostile-input tests that never reference a selector pass
/// `&symbols::Table::empty()` directly instead of calling this.
fn table_with(strings: &[&str]) -> symbols::Table {
    let mut builder = symbols::Builder::new();
    for string in strings {
        builder.intern(string);
    }
    builder.build()
}

/// One hand-crafted mixin-table row: named by symbol `name_id`, with
/// `methods` as `(selector symbol id, code id)` pairs and `nested` as row
/// ids. Everything else in the row is empty -- these fixtures exist to
/// drive one range check at a time, and writing the whole row shape out at
/// each call site would bury the field under test.
fn write_mixin_row(
    writer: &mut encode::Writer,
    name_id: u32,
    methods: &[(u32, u32)],
    nested: &[u32],
) {
    write_mixin_row_with_slots(writer, name_id, &[], methods, nested);
}

/// `write_mixin_row` with `slots` as `(name symbol id, slot kind tag,
/// access tag)` records.
fn write_mixin_row_with_slots(
    writer: &mut encode::Writer,
    name_id: u32,
    slots: &[(u32, u8, u8)],
    methods: &[(u32, u32)],
    nested: &[u32],
) {
    let methods: Vec<MethodShape> = methods
        .iter()
        .map(|&(selector, code)| MethodShape {
            selector,
            access: AccessModifier::Protected,
            argument_count: 0,
            code,
        })
        .collect();
    write_mixin_row_with_method_shapes(writer, name_id, slots, &methods, nested);
}

/// One method record of a hand-crafted row, every field of it chosen.
#[derive(Clone, Copy)]
struct MethodShape {
    selector: u32,
    access: AccessModifier,
    argument_count: u16,
    code: u32,
}

/// `write_mixin_row_with_slots` with each method's accessibility and
/// argument count chosen, for the checks that read them.
fn write_mixin_row_with_method_shapes(
    writer: &mut encode::Writer,
    name_id: u32,
    slots: &[(u32, u8, u8)],
    methods: &[MethodShape],
    nested: &[u32],
) {
    writer.write_u32(name_id);
    writer.write_u8(mixins::access_tag(AccessModifier::Protected)); // AccessModifier::Protected
    writer.write_u32(0); // span.start
    writer.write_u32(0); // span.end
    writer.write_u32(slots.len() as u32);
    for (name, kind, access) in slots {
        writer.write_u32(*name);
        writer.write_u8(*kind);
        writer.write_u8(*access);
    }
    writer.write_boolean(false); // no factory
    writer.write_boolean(false); // no initExprs
    writer.write_boolean(false); // no superclass clause
    writer.write_u32(0); // no mixin applications
    writer.write_u32(methods.len() as u32);
    for method in methods {
        writer.write_u32(method.selector);
        writer.write_u8(mixins::access_tag(method.access));
        writer.write_u16(method.argument_count);
        writer.write_u32(method.code);
    }
    writer.write_u32(nested.len() as u32);
    for id in nested {
        writer.write_u32(*id);
    }
    writer.write_boolean(false); // no class side
    writer.write_boolean(false); // no initializer
    writer.write_boolean(false); // no superclass clause body
    writer.write_boolean(false); // no superclass message body
}

/// A hand-crafted mixin table of exactly one leaf row.
fn one_row_mixin_table(name_id: u32, methods: &[(u32, u32)]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    write_mixin_row(&mut writer, name_id, methods, &[]);
    writer.into_bytes()
}

/// A mixin table of `depth` rows, each nesting the next: a declaration tree
/// as deep as it is large. Synthesised directly as bytes rather than by
/// snapshotting real source, because no parser accepts 100_000 levels of
/// nesting -- and the depth an image *claims* is exactly the input a
/// decoder must never recurse on.
fn mixin_chain(depth: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(depth);
    for level in 0..depth {
        let child = [level + 1];
        let nested: &[u32] = if level + 1 < depth { &child } else { &[] };
        write_mixin_row(&mut writer, 0, &[], nested);
    }
    writer.into_bytes()
}

const RICH: &str = indoc! {"
    Newspeak3
    'T'
    class Rich x: a = Object new (
      | p = a. q ::= 0. |
      q:: p.
    ) (
      find: t in: c = ( c do: [:e | e = t ifTrue: [^e]]. ^nil )
      loop = ( | n | n:: 0. [n < 3] whileTrue: [n:: n + 1]. ^n )
      arr = ( ^{ 1. { 2. 3 }. 'four' } )
    ) : ()
"};

fn every_instruction_once() -> CompiledCode {
    let block = Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instructions: vec![Instruction::NonLocalReturn { register: 0 }],
        argument_count: 1,
        // One parameter, two copied values, one temporary: the widest
        // register layout a block can have, so the round trip covers
        // `copied_count` too.
        register_count: 4,
        statement_program_counter: vec![(0, 0..1)],
        family_size: 0,
        copied_count: 2,
        outer_reach: 0,
        unwind_marker: None,
        source: Span::new(1, 2),
    });
    CompiledCode {
        kind: CodeKind::Initializer,
        instructions: vec![
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::Nil,
            },
            Instruction::LoadSelf { destination: 1 },
            Instruction::Move {
                destination: 3,
                source: 2,
            },
            Instruction::StoreLocal { source: 3, slot: 0 },
            Instruction::LoadOuterLocal {
                destination: 4,
                depth: 2,
                index: 1,
            },
            Instruction::StoreOuterLocal {
                source: 4,
                depth: 2,
                index: 1,
            },
            Instruction::SendLexical {
                depth: 1,
                selector: Symbol::intern("foo:"),
                argument_registers: vec![1, 2],
                destination: 5,
                call_site: CallSite { number: 0 },
            },
            Instruction::SendPublic {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_registers: vec![2],
                destination: 6,
                call_site: CallSite { number: 1 },
            },
            Instruction::SendInlinable {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_registers: vec![2],
                destination: 6,
                call_site: CallSite { number: 2 },
            },
            Instruction::SendSuper {
                selector: Symbol::intern("bar"),
                argument_registers: vec![],
                destination: 7,
                call_site: CallSite { number: 3 },
            },
            Instruction::MakeClosure {
                destination: 8,
                parameters: Rc::new(vec![Symbol::intern("e")]),
                code: block,
                copied: vec![1, 2],
            },
            Instruction::MakeArray {
                destination: 9,
                element_registers: vec![0, 1],
            },
            Instruction::MakeVector {
                destination: 10,
                size: 2,
            },
            Instruction::LoadVector {
                destination: 11,
                vector: 10,
                index: 0,
            },
            Instruction::StoreVector {
                vector: 10,
                index: 0,
                source: 1,
            },
            Instruction::InitializeSlot {
                source: 9,
                own_index: 3,
            },
            Instruction::ReturnSlotIfFilled { own_index: 2 },
            Instruction::MakeFuture {
                destination: 12,
                closure: 8,
            },
            Instruction::ResolveFuture {
                destination: 13,
                source: 12,
            },
            Instruction::LoadOwnSlot {
                destination: 14,
                own_index: 5,
            },
            Instruction::InitializeSlotProvisionally {
                source: 13,
                own_index: 6,
            },
            Instruction::Return { register: 0 },
            Instruction::ImplicitReturn { register: 1 },
            Instruction::NonLocalReturn { register: 2 },
            Instruction::Jump { target: 3 },
            Instruction::JumpIfNotBoolean {
                register: 1,
                target: 4,
            },
            Instruction::BranchIfFalse {
                register: 1,
                target: 5,
            },
            Instruction::BranchIfTrue {
                register: 1,
                target: 6,
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 6,
                left: 1,
                right: 2,
                target: 3,
            },
            Instruction::LoadEnclosingObject {
                destination: 5,
                depth: 1,
            },
            // Every other literal once, after the jumps so their targets
            // stay put.
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::Boolean(true),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::SmallInteger(-5),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::Float(1.5),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::String("s".into()),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::Symbol(Symbol::intern("y")),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::LargeInteger(
                    ArbitraryInteger::parse("99999999999999999999", 10).unwrap(),
                ),
            },
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::UnresolvedIdentifier("Object".into()),
            },
        ],
        argument_count: 2,
        register_count: 15,
        statement_program_counter: vec![(0, 0..3), (1, 3..37)],
        // One past the highest number among its own sends (0..=3, in
        // emission order -- what `number_call_site_families` assigns on the
        // decoded side too) and the block's `family_size` (0): the same
        // `bytecode::family_size_of` the decoder recomputes with, so the
        // round trip must agree.
        family_size: 4,
        copied_count: 0,
        // Its own `LoadOuterLocal`/`StoreOuterLocal` reach two frames out.
        outer_reach: 2,
        unwind_marker: None,
        source: Span::new(10, 20),
    }
}

/// The compiled code of `class`'s method `selector` -- the body the method
/// `Mutator::push_step` activates carries.
fn method_code<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: &str,
) -> Rc<CompiledCode> {
    let application = heap.class_application(class).expect("not a class");
    application
        .mixin
        .compiled_method(Symbol::intern(selector))
        .expect("the class declares the method")
        .code
        .clone()
}

/// `mixin::preorder` of a live class's declaration tree, in the order the
/// mixin table numbers its rows. `image::load` indexes the *decoded* table
/// here; a test that encodes and decodes against one live class can use
/// that class's own tree, which is the same walk.
fn mixin_preorder<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Vec<Rc<Mixin>> {
    let mut order = Vec::new();
    ns_runtime::mixin::preorder(&root_mixin_of(heap, class), &mut order);
    order
}

/// `class`'s own mixin as a value a test can edit. The clone is shallow: its
/// methods, nested declarations and class side are the same `Rc`s.
fn mixin_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Mixin {
    (*heap.class_application(class).expect("a class object").mixin).clone()
}

/// Replaces `class`'s mixin with `mixin`, applied to the same superclass
/// application. A mixin is shared and immutable, so this is the shape an edit
/// to a live class takes -- and, now that code lives on the declaration, how a
/// test puts a doctored body in front of the encoder. Takes `&mut Mutator`
/// and is called from inside an entry, alongside the rest of the doctoring
/// its callers do.
fn install_mixin<'gc>(mutator: &mut Mutator<'gc>, class: ClassReference<'gc>, mixin: Mixin) {
    let super_application = mutator
        .heap()
        .class_application(class)
        .expect("a class object")
        .superclass
        .clone();
    let application = mutator.apply_mixin(&Rc::new(mixin), super_application.as_ref());
    assert!(
        mutator.heap_mut().set_class_application(class, application),
        "a class object"
    );
}

/// `mixin` with method `selector`'s body replaced by `code`.
fn with_method_code(mut mixin: Mixin, selector: &str, code: Rc<CompiledCode>) -> Mixin {
    let selector = Symbol::intern(selector);
    let method = mixin
        .compiled_method(selector)
        .expect("the mixin declares the method")
        .clone();
    mixin.methods.insert(
        selector,
        ns_runtime::mixin::MethodEntry::Compiled(Rc::new(ns_runtime::mixin::MixinMethod {
            code,
            ..(*method).clone()
        })),
    );
    mixin
}

/// `mixin` with its direct nested declaration `name` replaced by `edit`'s
/// result.
fn with_nested(mut mixin: Mixin, name: &str, edit: impl FnOnce(Mixin) -> Mixin) -> Mixin {
    let index = mixin
        .nested
        .iter()
        .position(|nested| nested.name.as_str() == name)
        .expect("a nested declaration of that name");
    let edited = edit((*mixin.nested[index]).clone());
    mixin.nested[index] = Rc::new(edited);
    mixin
}

/// Encodes a class's code table and mixin table into fresh bytes and
/// decodes both back. The two travel together everywhere: the mixin table
/// is what says which compiled body belongs to which declaration.
fn round_trip_code_table<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> (decode::DecodedCodeTable, mixins::Decoded, encode::CodeIds) {
    let mut writer = encode::Writer::new();
    let mut symbols = symbols::Builder::new();
    let ids = encode::encode_code_table(&mut writer, heap, &[class], &mut symbols)
        .expect("encode code table");
    let mut mixin_writer = encode::Writer::new();
    mixins::encode(&mut mixin_writer, heap, &[class], &mut symbols, &ids)
        .expect("encode mixin table");
    let bytes = writer.into_bytes();
    let mixin_bytes = mixin_writer.into_bytes();
    let table = symbols.build();
    let mut reader = decode::Reader::new(&bytes);
    let mut codes = decode::decode_code_table(&mut reader, &table).expect("decode code table");
    // `image::load` numbers a family once its bodies are read and structurally
    // verified; this helper skips the verification (some callers deliberately
    // craft bodies that would fail it) but still needs the real numbers for a
    // caller that compares a decoded body against the compiler's own output.
    decode::number_call_site_families(&mut codes).expect("number call sites");
    let mut mixin_reader = decode::Reader::new(&mixin_bytes);
    let decoded_mixins =
        mixins::decode(&mut mixin_reader, &table, &codes.codes).expect("decode mixin table");
    (codes, decoded_mixins, ids)
}

#[test]
fn every_instruction_variant_round_trips() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let synthetic = Rc::new(every_instruction_once());
        let doctored = Mixin {
            initializer_code: Some(synthetic.clone()),
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let (table, decoded_mixins, ids) = round_trip_code_table(mutator.heap(), class);
        let root = &decoded_mixins.code[0];
        let decoded = &table.codes[root.initializer.expect("initializer id") as usize];
        assert_eq!(**decoded, *synthetic);
        assert_eq!(
            ids.id_of(&synthetic)
                .unwrap_or_else(|error| panic!("{error}")),
            root.initializer.expect("initializer id")
        );
    });
}

/// `Instruction::call_site` and `call_site_mut` share one macro-defined
/// variant list (`bytecode.rs`), but the macro only proves the two *cannot*
/// enumerate the four send variants with different tokens -- it does not by
/// itself prove the two accessors answer alike on real data (a typo inside
/// the shared pattern, for instance, would still compile and still affect
/// both, but should still be caught here). `every_instruction_once` already
/// covers every `Instruction` variant once (`every_instruction_variant_round_trips`
/// above pins that claim), so walking it is a direct check that the two
/// accessors agree on which instructions carry a call site and, where they
/// do, name the same one -- rather than trusting that a future edit to
/// either function keeps them looking at the same field.
#[test]
fn call_site_and_call_site_mut_agree_on_every_instruction_variant() {
    let code = every_instruction_once();
    let mut saw_a_call_site = false;
    for instruction in &code.instructions {
        let immutable = instruction.call_site();
        let mutable = instruction.clone().call_site_mut().copied();
        assert_eq!(
            immutable, mutable,
            "call_site and call_site_mut disagree for {instruction:?}"
        );
        saw_a_call_site |= immutable.is_some();
    }
    assert!(
        saw_a_call_site,
        "every_instruction_once must still include at least one send for this test to mean anything"
    );
}

#[test]
fn a_real_modules_code_table_round_trips_and_nests_blocks_before_their_parents() {
    let (mut interpreter, class) = load(RICH);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let root = &decoded_mixins.code[0];
        assert_eq!(root.methods.len(), 3);
        assert_eq!(
            root.methods
                .iter()
                .map(|(selector, _)| selector.as_str())
                .collect::<Vec<_>>(),
            vec!["arr", "find:in:", "loop"],
            "root list is sorted by selector"
        );
        for (selector, id) in &root.methods {
            let decoded = &table.codes[*id as usize];
            let original = method_code(mutator.heap(), class, selector.as_str());
            assert_eq!(**decoded, *original, "{selector}");
            for instruction in &decoded.instructions {
                if let Instruction::MakeClosure { code, .. } = instruction {
                    let child_id = table
                        .codes
                        .iter()
                        .position(|candidate| Rc::ptr_eq(candidate, code))
                        .expect("a MakeClosure's block body is in the table");
                    assert!(
                        child_id < *id as usize,
                        "a block's code precedes its parent's"
                    );
                }
            }
        }
        let initializer = &table.codes[root.initializer.expect("initializer id") as usize];
        assert_eq!(
            **initializer,
            *mixin_of(mutator.heap(), class)
                .initializer_code
                .expect("an initializer")
        );
    });
}

/// `code`'s call-site numbers in the order `compiler::lower` and
/// `decode::number_call_sites` both walk a family: a body's own sends in
/// instruction order, recursing into a nested block at its `MakeClosure`
/// rather than after the sends around it -- so this is the *sequence* one
/// shared counter produces, not merely the *set* `family_size` bounds.
/// `UNCACHED` carries no number at all and is left out, matching
/// `bytecode::family_size_of`.
fn call_site_numbers(code: &CompiledCode) -> Vec<u32> {
    let mut numbers = Vec::new();
    for instruction in &code.instructions {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            numbers.extend(call_site_numbers(block));
        } else if let Some(site) = instruction.call_site() {
            if site != CallSite::UNCACHED {
                numbers.push(site.number);
            }
        }
    }
    numbers
}

/// The decoder must reproduce the compiler's own numbering, not merely a
/// dense one: two nested blocks, each with a send of its own, so a family
/// whose blocks share no number with each other or with the root only
/// happens to look right if the decoder replays the same emission order the
/// compiler used, rather than, say, numbering block by block after the root.
#[test]
fn a_decoded_family_is_numbered_as_the_compiler_numbered_it() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public run = (
            self alpha.
            ^[:a | self beta. [:b | self gamma] value: a] value: 1
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let compiled = method_code(mutator.heap(), class, "run");
        let root = &decoded_mixins.code[0];
        let (_, id) = root
            .methods
            .iter()
            .find(|(selector, _)| selector.as_str() == "run")
            .expect("the run method");
        let decoded = &table.codes[*id as usize];
        assert_eq!(
            call_site_numbers(decoded),
            call_site_numbers(&compiled),
            "the decoder must number a family in the compiler's order"
        );
    });
}

/// A mixin decoded from an image carries the bodies its own row named, with
/// no table beside it: every method the decoded declaration holds has the
/// body the live method had, and so does its initializer.
#[test]
fn a_decoded_mixin_carries_the_bodies_its_row_names() {
    let (mut interpreter, class) = load(RICH);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let root = decoded_mixins.module_root(0).expect("a root row");
        assert_eq!(root.compiled_methods().count(), 3);
        for method in root.compiled_methods() {
            let selector = method.selector;
            assert_eq!(
                *method.code,
                *method_code(mutator.heap(), class, selector.as_str()),
                "{selector}"
            );
        }
        assert!(root.initializer_code.is_some());
    });
}

#[test]
fn a_root_list_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_row_mixin_table(0, &[(0, 5)]);
    let mut reader = decode::Reader::new(&bytes);
    match mixins::decode(&mut reader, &table_with(&["foo"]), &[]) {
        Err(decode::DecodeError::BadCodeId(id)) => {
            assert_eq!(id, 5);
            assert_eq!(
                decode::DecodeError::BadCodeId(id).to_string(),
                "code id 5 out of range"
            );
        }
        Err(error) => panic!("expected BadCodeId(5), got a different error: {error}"),
        Ok(_) => panic!("expected BadCodeId(5), got Ok"),
    }
}

#[test]
fn a_make_closure_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry ...
    writer.write_u8(encode::code_kind_tag(CodeKind::Block)); // CodeKind::Block
    writer.write_u16(0); // argument_count
    writer.write_u16(1); // register_count
    writer.write_u16(0); // copied_count
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_boolean(false); // not compiled uncached
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_MAKE_CLOSURE);
    writer.write_u16(0); // destination
    writer.write_u16(0); // no parameters
    writer.write_u32(9); // ... whose block-body code id is out of range
    writer.write_u16(0); // copying nothing
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::BadCodeId(9))
    );
}

#[test]
fn a_forward_make_closure_code_id_is_a_decode_error_not_a_panic() {
    // Post-order means a block's code always precedes its parent's, so an
    // id pointing at a not-yet-decoded entry is corruption, not a cycle.
    let mut writer = encode::Writer::new();
    writer.write_u32(2); // two code entries ...
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // ... the first a CodeKind::Method
    writer.write_u16(0);
    writer.write_u16(1);
    writer.write_u16(0); // copied_count
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_boolean(false); // not compiled uncached
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_MAKE_CLOSURE);
    writer.write_u16(0); // destination
    writer.write_u16(0); // no parameters
    writer.write_u32(1); // ... naming the *following*, not-yet-decoded entry
    writer.write_u16(0); // copying nothing
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::BadCodeId(1))
    );
}

#[test]
fn an_invalid_code_kind_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(200);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "code kind",
            tag: 200
        })
    );
}

#[test]
fn an_invalid_instruction_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0);
    writer.write_u16(0);
    writer.write_u16(0); // copied_count
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_boolean(false); // not compiled uncached
    writer.write_u32(1); // one instruction ...
    writer.write_u8(40); // ... one past the last real tag
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 40
        })
    );
}

/// One Method body's code table: no statements, `uncached` compiled-uncached
/// flag, and `instruction_count` instructions that `write_instructions`
/// writes.
fn one_method_code_table(
    uncached: bool,
    instruction_count: u32,
    write_instructions: impl FnOnce(&mut encode::Writer),
) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry
    writer.write_u8(encode::code_kind_tag(CodeKind::Method));
    writer.write_u16(0); // argument_count
    writer.write_u16(2); // register_count
    writer.write_u16(0); // copied_count
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_boolean(uncached);
    writer.write_u32(instruction_count);
    write_instructions(&mut writer);
    writer.into_bytes()
}

/// A unary send of symbol 0 from register 0 into register 1, under `tag`. No
/// call-site number follows: the artifact no longer carries one for any send,
/// retired or current.
fn write_unary_send(writer: &mut encode::Writer, tag: u8) {
    writer.write_u8(tag);
    writer.write_u32(0); // selector: symbol 0
    writer.write_u16(0); // receiver register
    writer.write_u16(0); // no argument registers
    writer.write_u16(1); // destination
}

fn write_return(writer: &mut encode::Writer) {
    writer.write_u8(OPCODE_RETURN);
    writer.write_u16(1);
}

/// A body written with any retired send tag -- a stale artifact from before
/// call sites were widened (7-10) or before a number stopped being
/// serialized at all (21-24) -- is refused, not misread.
#[test]
fn a_retired_send_tag_is_refused() {
    // Every retired number, not just one representative: 7-10 while a call
    // site was a `u16` instruction index, 21-24 while it was serialized as a
    // `u32` at all -- a stale artifact from either era meets an unknown tag,
    // not a differently-shaped `Send` misread as today's 25-28. Looping over
    // `RETIRED_OPCODES` itself, rather than restating the numbers, is what
    // keeps this test saying exactly what the invariant says: a decoder
    // change that resurrects any one of these eight numbers fails here.
    for tag in RETIRED_OPCODES {
        let bytes = one_method_code_table(false, 2, |writer| {
            write_unary_send(writer, tag);
            write_return(writer);
        });
        let mut reader = decode::Reader::new(&bytes);
        assert_eq!(
            decode::decode_code_table(&mut reader, &table_with(&["size"])),
            Err(decode::DecodeError::InvalidTag {
                what: "instruction",
                tag
            }),
            "retired tag {tag}"
        );
    }
}

/// Tag 2 was `LoadBuiltinClass` before a bare builtin class name became a
/// send to `Object`'s accessor, answered without an activation (§3.2); a
/// stale artifact's tag-2 bytes meet an unknown tag instead of decoding into
/// an instruction no lowering emits any more (`RETIRED_OPCODES`).
#[test]
fn the_retired_load_builtin_class_tag_is_refused() {
    let bytes = one_method_code_table(false, 1, |writer| {
        writer.write_u8(2); // the retired LoadBuiltinClass tag
        writer.write_u16(0); // destination
                             // a builtin class tag would have followed; never read, since the
                             // instruction tag alone is already refused
        writer.write_u8(0);
    });
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 2
        })
    );
}

/// An unknown literal tag is refused, and so is every retired one: tag 6
/// was the character literal and tag 8 an integer literal beyond `i64`
/// carried as text (`RETIRED_LITERALS`), and a stale artifact's payload --
/// its tag and whatever followed it -- meets an unknown tag rather than
/// decoding as whatever literal takes the next free number.
#[test]
fn an_invalid_or_retired_literal_tag_is_a_decode_error_not_a_panic() {
    let mut payloads = vec![vec![LITERAL_LARGE_INTEGER + 1]];
    for tag in RETIRED_LITERALS {
        payloads.push(vec![tag, 0x41, 0x00, 0x00, 0x00]);
    }
    for (payload, what) in payloads.into_iter().map(|payload| (payload, "literal")) {
        let mut writer = encode::Writer::new();
        writer.write_u32(1);
        writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
        writer.write_u16(0);
        writer.write_u16(1);
        writer.write_u16(0); // copied_count
        writer.write_u32(0);
        writer.write_u32(0);
        writer.write_u32(0);
        writer.write_boolean(false); // not compiled uncached
        writer.write_u32(1); // one instruction ...
        writer.write_u8(OPCODE_LOAD_LITERAL);
        writer.write_u16(0); // destination
        writer.write_raw(&payload);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        match decode::decode_code_table(&mut reader, &symbols::Table::empty()) {
            Err(decode::DecodeError::InvalidTag { what: got, .. }) => assert_eq!(got, what),
            Err(error) => panic!("expected an invalid {what} tag, got a different error: {error}"),
            Ok(_) => panic!("expected an invalid {what} tag, got Ok"),
        }
    }
}

/// A `LargeInteger` in a slot survives a snapshot as a value, and one that
/// fits `i64` comes back an `Integer`: `-2^63` is `9223372036854775807 + 1`
/// negated, which the arithmetic already answers as a `SmallInteger`, and
/// the round trip keeps it one.
#[test]
fn a_large_integer_value_round_trips() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | large = 9223372036854775807 + 1. small = 0 - (9223372036854775807 + 1). |
        ) (
          public largeText = ( ^large printString )
          public largeClass = ( ^large class printString )
          public smallText = ( ^small printString )
          public smallClass = ( ^small class printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    for (selector, expected) in [
        ("largeText", "9223372036854775808"),
        ("largeClass", "LargeInteger"),
        ("smallText", "-9223372036854775808"),
        ("smallClass", "Integer"),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            let Value::Object(string) = mutator.fetch(&answer) else {
                panic!("{selector}: expected a string")
            };
            assert_eq!(
                mutator.heap().str_value(string).as_deref(),
                Some(expected),
                "{selector}"
            );
        });
    }
}

/// A literal beyond `i64` in a compiled body survives the code table's
/// round trip digit for digit: the literal tag carries the digits, not the
/// source text, so the decoder never re-parses anything.
#[test]
fn a_large_integer_literal_round_trips() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( ^99999999999999999999 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let original = method_code(mutator.heap(), class, "foo");
        let expected =
            Literal::LargeInteger(ArbitraryInteger::parse("99999999999999999999", 10).unwrap());
        assert!(
            original.instructions.iter().any(|instruction| matches!(
                instruction,
                Instruction::LoadLiteral { literal, .. } if *literal == expected
            )),
            "the compiler emits the literal the round trip is about"
        );
        let (table, _, ids) = round_trip_code_table(mutator.heap(), class);
        let decoded = &table.codes[ids
            .id_of(&original)
            .unwrap_or_else(|error| panic!("{error}")) as usize];
        assert_eq!(**decoded, *original);
    });
}

/// The payload of a `LargeInteger`, after its tag, that the decoder must
/// refuse, each with the error it answers. (a) to (c) and the `i64::MIN`
/// case are well formed but not the one encoding the encoder writes: an
/// integer that fits `i64` travels as a small integer, the magnitude has no
/// zero top digit, and zero has no sign. (d) claims more digits than the
/// bytes left can hold and is refused before anything is allocated for it.
fn non_canonical_large_integer_payloads() -> Vec<(&'static str, Vec<u8>, decode::DecodeError)> {
    fn payload(negative: u8, count: u32, digits: &[u64], trailing: usize) -> Vec<u8> {
        let mut writer = encode::Writer::new();
        writer.write_u8(negative);
        writer.write_u32(count);
        for digit in digits {
            writer.write_u64(*digit);
        }
        writer.write_raw(&vec![0; trailing]);
        writer.into_bytes()
    }
    vec![
        (
            "(a) digits that fit i64",
            payload(0, 1, &[5], 0),
            decode::DecodeError::NonCanonicalInteger,
        ),
        (
            "(b) a zero top digit",
            payload(0, 2, &[1, 0], 0),
            decode::DecodeError::NonCanonicalInteger,
        ),
        (
            "(c) a negative zero",
            payload(1, 0, &[], 0),
            decode::DecodeError::NonCanonicalInteger,
        ),
        (
            "(d) a count far beyond the bytes left",
            payload(0, u32::MAX, &[], 10),
            decode::DecodeError::UnexpectedEndOfFile,
        ),
        (
            "a positive zero",
            payload(0, 0, &[], 0),
            decode::DecodeError::NonCanonicalInteger,
        ),
        (
            "i64::MIN, which fits",
            payload(1, 1, &[1 << 63], 0),
            decode::DecodeError::NonCanonicalInteger,
        ),
        (
            "a sign byte that is neither 0 nor 1",
            payload(2, 1, &[1 << 63], 0),
            decode::DecodeError::InvalidTag {
                what: "large integer sign",
                tag: 2,
            },
        ),
    ]
}

#[test]
fn a_non_canonical_large_integer_is_refused() {
    for (case, payload, expected) in non_canonical_large_integer_payloads() {
        let bytes = one_array_object(|writer| {
            writer.write_u8(VALUE_LARGE_INTEGER);
            writer.write_raw(&payload);
        });
        assert_eq!(
            GraphFixture::new().decode_error(&bytes),
            expected,
            "value {case}"
        );

        let bytes = one_method_code_table(false, 1, |writer| {
            writer.write_u8(OPCODE_LOAD_LITERAL);
            writer.write_u16(0); // destination
            writer.write_u8(LITERAL_LARGE_INTEGER);
            writer.write_raw(&payload);
        });
        let mut reader = decode::Reader::new(&bytes);
        match decode::decode_code_table(&mut reader, &symbols::Table::empty()) {
            Err(error) => assert_eq!(error, expected, "literal {case}"),
            Ok(_) => panic!("literal {case}: expected {expected}, got Ok"),
        }
    }
    assert_eq!(
        decode::DecodeError::NonCanonicalInteger.to_string(),
        "an integer is not in its one canonical encoding"
    );
}

/// A `LargeInteger` has no identity (§3.1.1), so the image carries it as a
/// value wherever it is referenced and never gives it an object record: two
/// slots holding the same object each carry the digits.
#[test]
fn a_large_integer_is_a_value_not_a_record() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | first = 9223372036854775807 + 1. second = first. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        !report
            .graph_objects
            .iter()
            .any(|object| object.starts_with("class LargeInteger")),
        "no LargeInteger record: {:?}",
        report.graph_objects
    );
    let occurrences: usize = report
        .graph_objects
        .iter()
        .map(|object| object.matches("large-int 9223372036854775808").count())
        .sum();
    assert_eq!(
        occurrences, 2,
        "both slots carry the value: {:?}",
        report.graph_objects
    );
}

/// `inspect` renders an integer in decimal only up to
/// `DECIMAL_RENDERING_LIMIT` digits, and elides one past it: an artifact is
/// untrusted, and the quadratic decimal conversion of an integer of 100,000
/// digits would keep `inspect` busy for hours.
#[test]
fn inspect_renders_a_huge_large_integer_elided_and_a_normal_one_in_decimal() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let started = std::time::Instant::now();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let normal = mutator.new_integer(ArbitraryInteger::from_i128(1_i128 << 63));
        let huge =
            mutator.new_integer(ArbitraryInteger::from_parts(false, (1..=100_000).collect()));
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance, normal, huge],
        )
        .expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.graph_roots[1..],
        [
            "large-int 9223372036854775808",
            "large-int <100000 words> 0x186a0000000000001869f...00000000000000020000000000000001",
        ]
    );
    // In decimal this takes hours; elided, a debug build needs well under a
    // second for the snapshot and the walk together.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
}

/// A builtin class tag names no builtin at the root-class section either:
/// `decode_root_section` reads its superclass through the same
/// `read_program_builtin_class`, so an out-of-range byte is refused there as
/// well, never misread as some other builtin class.
#[test]
fn an_invalid_builtin_class_tag_at_the_root_section_is_a_decode_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let decoded = mixin_table_of(&snapshot_of(source));
    let mut writer = encode::Writer::new();
    writer.write_boolean(true); // the root's superclass is present ...
    writer.write_u8(200); // ... naming no builtin class
    writer.write_u32(0); // no levels
    let bytes = writer.into_bytes();
    assert_eq!(
        decode::decode_root_section(&mut decode::Reader::new(&bytes), &decoded),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class",
            tag: 200
        })
    );
}

/// A root class's superclass tag can decode to `Metaclass` once
/// `CLASS_BUILTIN` can hold it (a value's class chain reaching it, from Task
/// 4 on), but a program may still not *declare* a root over it:
/// `decode_root_section` reads the superclass through
/// `read_program_builtin_class`, which refuses `Behavior`, `Class` and
/// `Metaclass` the same way it refuses the VM mirror's class and `Top`.
/// Patched into a real binary and located the way `corrupt_first_symbol_id`
/// locates the symbol table's count, so this exercises the whole `load`
/// pipeline, not `decode_root_section` alone.
#[test]
fn a_root_superclass_naming_metaclass_is_refused() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let origin = Origin::File(PathBuf::from("t.ns"));
    let mut binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build")
    });

    let superclass_tag_at = {
        let mut reader = decode::Reader::new(&binary);
        let header = decode::read_header(&mut reader).expect("valid header");
        if header.has_source {
            reader.read_str().expect("source");
        }
        decode::decode_source_origin(&mut reader).expect("origin");
        let symbols = symbols::decode(&mut reader).expect("symbols");
        let mut table = decode::decode_code_table(&mut reader, &symbols).expect("code table");
        decode::verify_code_table(&table).expect("verify code table");
        decode::number_call_site_families(&mut table).expect("number call site families");
        mixins::decode(&mut reader, &symbols, &table.codes).expect("mixins");
        // The root section opens with the "has superclass" boolean, then the
        // superclass's builtin-class tag byte.
        reader.position() + 1
    };
    binary[superclass_tag_at] = encode::builtin_class_tag(BuiltinClass::Metaclass);

    // Not `.expect_err`: `LoadedImage` carries an `Interpreter`, which has no
    // `Debug` impl, so neither type has one.
    let error = match image::load(&binary) {
        Ok(_) => panic!("a root superclass naming Metaclass must be refused"),
        Err(error) => error,
    };
    assert!(
        error.contains("builtin class a program may name"),
        "unexpected error: {error}"
    );
}

#[test]
fn an_invalid_integer_operation_tag_is_a_decode_error_not_a_panic() {
    // An unknown operation byte must be refused, never defaulted to an
    // operation: an image is untrusted input, and a stale or corrupt byte
    // must not silently decode as one of the nine real operations.
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0);
    writer.write_u16(1);
    writer.write_u16(0); // copied_count
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_boolean(false); // not compiled uncached
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_INTEGER_BINARY);
    writer.write_u8(200); // ... naming no integer operation
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "integer operation",
            tag: 200
        })
    );
}

#[test]
fn a_truncated_code_table_is_a_decode_error_not_a_panic() {
    let (mut interpreter, class) = load(RICH);
    let (table, bytes) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let mut writer = encode::Writer::new();
        let mut symbols = symbols::Builder::new();
        encode::encode_code_table(&mut writer, mutator.heap(), &[class], &mut symbols)
            .expect("encode code table");
        (symbols.build(), writer.into_bytes())
    });
    for cut in [1usize, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
        let mut reader = decode::Reader::new(&bytes[..cut]);
        assert!(
            decode::decode_code_table(&mut reader, &table).is_err(),
            "a code table cut to {cut} bytes must not decode"
        );
    }
}

#[test]
fn every_builtin_class_tag_round_trips() {
    let all = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::String,
        BuiltinClass::Symbol,
        BuiltinClass::Array,
        BuiltinClass::UndefinedObject,
        BuiltinClass::Block,
        BuiltinClass::VmMirror,
        BuiltinClass::Top,
        BuiltinClass::Value,
        BuiltinClass::Number,
    ];
    for kind in all {
        assert_eq!(
            decode::builtin_class_from_tag(encode::builtin_class_tag(kind)),
            Some(kind)
        );
    }
    assert_eq!(decode::builtin_class_from_tag(200), None);
    let mut tags: Vec<u8> = all
        .iter()
        .map(|kind| encode::builtin_class_tag(*kind))
        .collect();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags.len(), all.len(), "every builtin class has its own tag");
}

#[test]
fn every_integer_operation_tag_round_trips() {
    let all = [
        IntegerOperation::Add,
        IntegerOperation::Subtract,
        IntegerOperation::Multiply,
        IntegerOperation::FloorDivide,
        IntegerOperation::Less,
        IntegerOperation::Greater,
        IntegerOperation::LessOrEqual,
        IntegerOperation::GreaterOrEqual,
        IntegerOperation::Equal,
    ];
    for operation in all {
        assert_eq!(
            decode::integer_operation_from_tag(encode::integer_operation_tag(operation)),
            Some(operation)
        );
    }
    assert_eq!(decode::integer_operation_from_tag(200), None);
    let mut tags: Vec<u8> = all
        .iter()
        .map(|operation| encode::integer_operation_tag(*operation))
        .collect();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(
        tags.len(),
        all.len(),
        "every integer operation has its own tag"
    );
}

/// A body's parameters and copied values share one register file, and
/// `push_step` sizes an activation as `register_count - argument_count`, so
/// a `copied_count` that does not fit would leave `fill_copied_values`
/// writing past the registers the body has.
#[test]
fn a_copied_count_that_does_not_fit_the_register_file_is_a_decode_error() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Block)); // CodeKind::Block
    writer.write_u16(1); // argument_count
    writer.write_u16(2); // register_count
    writer.write_u16(2); // copied_count -- one more than the file has room for
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    let error = decode::decode_code_table(&mut reader, &symbols::Table::empty())
        .expect_err("the copied values do not fit");
    assert_eq!(
        error,
        decode::DecodeError::CopiedCountOutOfRange {
            argument_count: 1,
            copied_count: 2,
            register_count: 2
        }
    );
    assert_eq!(
        error.to_string(),
        "a body in the image takes 1 arguments and 2 copied values, which do not fit its \
         2 registers"
    );
}

#[test]
fn a_backwards_source_span_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0); // argument_count
    writer.write_u16(0); // register_count
    writer.write_u16(0); // copied_count
    writer.write_u32(9); // source.start ...
    writer.write_u32(4); // ... after source.end
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidSpan { start: 9, end: 4 })
    );
}

#[test]
fn a_huge_code_table_count_does_not_reserve_gigabytes() {
    // A count prefix is untrusted: reserving for it before reading would
    // abort the process instead of reporting a short image.
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::MAX);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

// ---------------------------------------------------------------------------
// The depth the code table controls: blocks nest as deep as the image
// says, so decoding them has to be bounded.
// ---------------------------------------------------------------------------

/// Blocks nested `levels` deep in one method.
/// Body 0 is the innermost block, each later body creates the one before it,
/// and the last is the method -- the order the encoder writes, every block
/// before the code that creates it. The innermost block sits `levels`
/// `MakeClosure` hops below the method. Each level is 38 bytes, so the file
/// grows linearly with the nesting it asks for.
fn code_table_with_nested_blocks(levels: usize) -> Vec<u8> {
    let body = |writer: &mut encode::Writer, kind: u8, creates: Option<usize>| {
        writer.write_u8(kind);
        writer.write_u16(0); // argument_count
        writer.write_u16(1); // register_count
        writer.write_u16(0); // copied_count
        writer.write_u32(0); // source.start
        writer.write_u32(0); // source.end
        writer.write_u32(0); // no statement_program_counter entries
        writer.write_boolean(false); // not compiled uncached
        match creates {
            Some(block) => {
                writer.write_u32(2); // two instructions ...
                writer.write_u8(OPCODE_MAKE_CLOSURE); // ... r0 <- the next block in,
                writer.write_u16(0);
                writer.write_u16(0); // taking no parameters,
                writer.write_u32(u32::try_from(block).expect("a test-sized code id"));
                writer.write_u16(0); // copying nothing
            }
            None => writer.write_u32(1), // the innermost: one instruction ...
        }
        writer.write_u8(OPCODE_RETURN); // ... then ^r0
        writer.write_u16(0);
    };
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::try_from(levels + 1).expect("a test-sized code table"));
    body(&mut writer, 1, None); // CodeKind::Block
    for id in 1..levels {
        body(&mut writer, 1, Some(id - 1));
    }
    body(&mut writer, 0, Some(levels - 1)); // CodeKind::Method
    writer.into_bytes()
}

/// A decoded block's nesting is what bounds the exempt activations a
/// crafted `SendInlinable` chain can stack above one counted activation
/// (`docs/design/compiler.md`, "How the limit counts"), so the decoder caps it
/// at the parser's own ceiling: one level past it is refused.
#[test]
fn a_block_nested_one_level_past_the_ceiling_is_a_decode_error() {
    let bytes = code_table_with_nested_blocks(129);
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_code_table(&mut reader, &symbols::Table::empty()) {
        Err(error) => assert_eq!(
            error,
            decode::DecodeError::BlockNestingTooDeep { limit: 128 }
        ),
        Ok(table) => panic!(
            "blocks nested 129 deep decoded, {} bodies",
            table.codes.len()
        ),
    }
    assert_eq!(
        decode::DecodeError::BlockNestingTooDeep { limit: 128 }.to_string(),
        "blocks in image code nest deeper than 128 levels"
    );
}

/// The ceiling itself decodes and verifies: it is the parser's limit, and a
/// program at that limit must round-trip (see
/// `the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs`).
#[test]
fn a_block_nested_at_the_ceiling_decodes_and_verifies() {
    let bytes = code_table_with_nested_blocks(128);
    let mut reader = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut reader, &symbols::Table::empty())
        .expect("128 levels is within the ceiling");
    assert_eq!(table.codes.len(), 129);
    assert_eq!(decode::verify_code_table(&table), Ok(()));
}

/// Refused while it is read, not after: a decoded body owns the bodies it
/// creates through `Rc`s, so dropping a chain of them recurses once per
/// level. Decoded whole and refused afterwards, a few megabytes of chain
/// would overflow the stack -- an abort, not an error -- the moment the
/// refused table was dropped.
#[test]
fn a_deeply_nested_block_chain_is_a_decode_error_not_a_stack_overflow() {
    let bytes = code_table_with_nested_blocks(100_000);
    let mut reader = decode::Reader::new(&bytes);
    let result = decode::decode_code_table(&mut reader, &symbols::Table::empty());
    let refused = matches!(
        result,
        Err(decode::DecodeError::BlockNestingTooDeep { limit: 128 })
    );
    drop(result);
    assert!(refused, "a 100,000-deep chain of blocks must be refused");
}

/// An image whose mixin table is empty describes no root class, so there is
/// nothing for `load` to build the world around -- and nothing for the
/// graph's class tag 1 to name.
#[test]
fn a_mixin_table_with_no_root_row_is_a_decode_error() {
    let mut interpreter = Interpreter::bootstrap();
    let empty = mixins::Decoded::empty();
    interpreter.enter(|mutator| {
        let section = decode::RootSection {
            superclass: BuiltinClass::Object,
            levels: Vec::new(),
        };
        match decode::build_root_class(mutator, &empty, &section) {
            Err(decode::DecodeError::NotAClass) => assert_eq!(
                decode::DecodeError::NotAClass.to_string(),
                "the image's mixin table describes no root class"
            ),
            Err(error) => panic!("expected NotAClass, got a different error: {error}"),
            Ok(_) => panic!("expected NotAClass, got Ok"),
        }
    });
}

// ---------------------------------------------------------------------------
// The decoder's nesting ceiling must sit at or above what `ns-syntax`'s
// parser lets a program contain, or the image cannot read back what it
// just wrote.
// ---------------------------------------------------------------------------

/// A module whose one method nests `levels` blocks, each sending `value` to
/// the next one in, the innermost empty: `[[[] value] value] value`. An empty
/// block costs the parser no further level, so this is the deepest block
/// nesting a given parser depth allows.
fn source_with_nested_blocks(levels: usize) -> String {
    let mut nest = "[]".to_string();
    for _ in 1..levels {
        nest = format!("[{nest} value]");
    }
    formatdoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public nest = ( ^{nest} value )
        ) : ()
    "}
}

/// The deepest block nesting `ns-syntax`'s parser accepts, probed rather than
/// restated, so that the parser's limit and the decoder's cannot drift apart
/// silently.
fn deepest_block_nesting_the_parser_accepts() -> usize {
    let mut deepest = 0;
    for levels in 1..1024 {
        if ns_syntax::parse(&source_with_nested_blocks(levels)).is_ok() {
            deepest = levels;
        } else {
            break;
        }
    }
    assert!(deepest > 0, "the parser accepts at least one block");
    deepest
}

/// How many `MakeClosure` hops the deepest block in `code` sits below it.
fn block_nesting(code: &CompiledCode) -> usize {
    code.instructions
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::MakeClosure { code: block, .. } => Some(1 + block_nesting(block)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

ns_runtime::for_each_lowering! {
    /// The decoder's block-nesting ceiling (`BlockNestingTooDeep`) must admit
    /// everything the parser does, under either lowering, or a legal program
    /// would build into a binary that cannot be loaded. `[...] value` is not a
    /// control-flow send, so neither lowering inlines a level away: the code
    /// table nests exactly as deep as the source.
    fn the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs(lowering) {
        let levels = deepest_block_nesting_the_parser_accepts();
        let source = source_with_nested_blocks(levels);
        let module = ns_syntax::parse(&source).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            Rc::from(source.as_str()),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        // Nothing runs in this interpreter, so the class read from its root
        // stays current.
        let bytes = interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            assert_eq!(
                block_nesting(&method_code(mutator.heap(), class, "nest")),
                levels,
                "{lowering:?}: every source level is a block in the code table"
            );
            image::build(
                mutator.heap(),
                None,
                &Origin::File(PathBuf::from("t.ns")),
                class,
                Symbol::intern("nest"),
            )
            .expect("build")
        });
        let mut loaded = match image::load(&bytes) {
            Ok(loaded) => loaded,
            Err(error) => panic!("{lowering:?}: {levels} nested blocks must load: {error}"),
        };
        let instance = expect_ok(
            loaded.interpreter.instantiate(&loaded.root_class, &[]),
            "instantiate",
        );
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&instance, Symbol::intern("nest"), &[]),
            "nest",
        );
        assert!(
            loaded
                .interpreter
                .enter(|mutator| matches!(mutator.fetch(&answer), Value::Nil)),
            "{lowering:?}: `[] value` is nil"
        );
    }
}

// ---------------------------------------------------------------------------
// The object table and the frame table.
// ---------------------------------------------------------------------------

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The other half of `expect_ok`: unwraps the `NewspeakError`'s message
/// from a `HostResult` expected to fail, the same way `tests/value_conditions.rs`'s
/// `expect_error` does.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Newspeak error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
    }
}

/// The mixin table's row ids, without keeping the table's own bytes: a
/// graph section names a class object's declaration by row id, so encoding
/// one needs exactly the map `image::snapshot` builds. Called from inside an
/// entry, alongside the rest of the encoding its callers do.
fn mixin_ids_for<'gc>(
    class: ClassReference<'gc>,
    mutator: &Mutator<'gc>,
    symbols: &mut symbols::Builder,
    ids: &encode::CodeIds,
) -> HashMap<usize, u32> {
    let mut scratch = encode::Writer::new();
    mixins::encode(&mut scratch, mutator.heap(), &[class], symbols, ids).expect("mixin table")
}

/// Loads `source`, lets `make_roots` build the roots, encodes code table +
/// graph section, decodes both with the same `Reader`, and returns
/// (interpreter, class root, original roots, decoded roots, maximum identity
/// hash) -- every handle rooted, since none of them may outlive the entry
/// that produced it. `make_roots` may run code, so the class and the roots
/// are only read again, inside a fresh entry, once it has returned.
fn round_trip_graph(
    source: &str,
    make_roots: impl FnOnce(&mut Interpreter, &Root) -> Vec<Root>,
) -> (Interpreter, Root, Vec<Root>, Vec<Root>, u32) {
    let (mut interpreter, class) = load(source);
    let roots = make_roots(&mut interpreter, &class);
    let platform = interpreter.platform();
    let (decoded_roots, max_identity_hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let root_values: Vec<Value> = roots.iter().map(|root| mutator.fetch(root)).collect();
        let platform_value = mutator.fetch(&platform);
        let mut symbols = symbols::Builder::new();
        let mut writer = encode::Writer::new();
        let ids = encode::encode_code_table(&mut writer, mutator.heap(), &[class], &mut symbols)
            .expect("encode code table");
        let mixin_ids = mixin_ids_for(class, mutator, &mut symbols, &ids);
        encode::encode_graph_section(
            &mut writer,
            mutator.heap(),
            &root_values,
            &platform_value,
            &ids,
            class,
            &mixin_ids,
            &mut symbols,
        )
        .expect("encode graph");
        let bytes = writer.into_bytes();
        let symbol_table = symbols.build();
        let mut reader = decode::Reader::new(&bytes);
        let code_table = decode::decode_code_table(&mut reader, &symbol_table).expect("code table");
        let mixin_order = mixin_preorder(mutator.heap(), class);
        let root_levels = encode::root_levels(mutator.heap(), class);
        let decoded = {
            let mut context = decode::DecodeContext {
                mutator,
                root_class: &class,
                codes: &code_table.codes,
                platform: platform_value,
                mixins: &mixin_order,
                root_levels: &root_levels,
                platform_paths: &[],
            };
            decode::decode_graph_section(&mut reader, &mut context, &symbol_table).expect("graph")
        };
        let decoded_roots: Vec<Root> = decoded
            .roots
            .iter()
            .map(|value| mutator.stash(*value))
            .collect();
        (decoded_roots, decoded.max_identity_hash)
    });
    (interpreter, class, roots, decoded_roots, max_identity_hash)
}

const EMPTY: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
    ) (
    ) : ()
"};

fn object<'a, 'gc>(value: &'a Value<'gc>) -> &'a ns_runtime::ObjectReference<'gc> {
    let Value::Object(handle) = value else {
        panic!("expected an object")
    };
    handle
}

#[test]
fn instances_and_arrays_round_trip_with_shared_identity_preserved() {
    let (mut interpreter, _class, original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, _| {
            interpreter.enter(|mutator| {
                let shared = Value::Object(mutator.new_string("shared".to_string()));
                let array = mutator.new_array(vec![shared, shared, Value::SmallInteger(7)]);
                vec![mutator.stash(Value::Object(array))]
            })
        });
    interpreter.enter(|mutator| {
        let original = mutator.fetch(&original[0]);
        let decoded = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let elements = heap
            .array_elements(*object(&decoded))
            .expect("expected an array");
        assert!(
            object(&elements[0]).same(*object(&elements[1])),
            "shared identity preserved"
        );
        assert!(
            !object(&original).same(*object(&decoded)),
            "the decoded array is a new object"
        );
        assert!(matches!(
            heap.str_value(*object(&elements[0])).as_deref(),
            Some("shared")
        ));
        assert!(matches!(elements[2], Value::SmallInteger(7)));
    });
}

#[test]
fn identity_hashes_are_preserved_and_the_maximum_is_reported() {
    // Two objects, the higher hash first: the object table is walked in
    // root order, so the maximum is *not* the last one seen and
    // `assert_eq!` can tell a real fold from a stale field or a running
    // counter.
    let (mut interpreter, _class, original, decoded, max) =
        round_trip_graph(EMPTY, |interpreter, _| {
            interpreter.enter(|mutator| {
                let first = mutator.new_string("x".to_string());
                let second = mutator.new_string("y".to_string());
                // Requested in this order, so `second` holds the higher hash.
                mutator.heap_mut().ensure_identity_hash(first);
                mutator.heap_mut().ensure_identity_hash(second);
                vec![
                    mutator.stash(Value::Object(second)),
                    mutator.stash(Value::Object(first)),
                ]
            })
        });
    interpreter.enter(|mutator| {
        let original_0 = mutator.fetch(&original[0]);
        let original_1 = mutator.fetch(&original[1]);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let decoded_1 = mutator.fetch(&decoded[1]);
        let heap = mutator.heap();
        let higher = heap.identity_hash(*object(&original_0));
        let lower = heap.identity_hash(*object(&original_1));
        assert!(higher > lower, "hashes are handed out in request order");
        assert_eq!(heap.identity_hash(*object(&decoded_0)), higher);
        assert_eq!(heap.identity_hash(*object(&decoded_1)), lower);
        assert_eq!(max, higher);
    });
}

#[test]
fn class_references_resolve_to_the_same_live_class_objects() {
    let (mut interpreter, class, _original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, class| {
            interpreter.enter(|mutator| {
                let builtin = mutator.builtin_class(BuiltinClass::Integer);
                vec![mutator.stash(Value::Object(builtin)), class.clone()]
            })
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let decoded_1 = mutator.fetch(&decoded[1]);
        let builtin = mutator.builtin_class(BuiltinClass::Integer);
        let class = class_reference(mutator, &class);
        assert!(object(&decoded_0).same(builtin));
        assert!(object(&decoded_1).same(class));
    });
}

#[test]
fn the_platform_singleton_resolves_to_the_interpreters_own() {
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, _| vec![interpreter.platform()]);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let platform = mutator.fetch(&platform);
        assert!(object(&decoded_0).same(*object(&platform)));
    });
}

#[test]
fn an_instance_of_the_root_class_keeps_its_class_and_fields() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | a = 1. b = 'two'. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class, _original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            vec![expect_ok(
                interpreter.instantiate(class, &[]),
                "instantiate",
            )]
        });
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let snapshot = heap.object_snapshot(*object(&decoded_0));
        assert!(snapshot.class.same(class));
        let ObjectData::Instance(fields) = &snapshot.data else {
            panic!("expected an instance")
        };
        assert!(matches!(fields[0], Value::SmallInteger(1)));
        assert!(matches!(
            heap.str_value(*object(&fields[1])).as_deref(),
            Some("two")
        ));
    });
}

#[test]
fn a_closure_and_its_paused_home_frame_round_trip() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          mk = ( ^[ 1 ] )
          public makeClosure = ( ^mk )
        ) : ()
    "};
    let (mut interpreter, class, original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            let closure = expect_ok(
                interpreter.send(&instance, Symbol::intern("makeClosure"), &[]),
                "makeClosure",
            );
            // Synthetic "paused" state: edit the home frame's program counter
            // and grow its register file before snapshotting.
            interpreter.enter(|mutator| {
                let closure_value = mutator.fetch(&closure);
                let home = mutator
                    .heap()
                    .closure_home(*object(&closure_value))
                    .expect("expected a closure");
                let (_, argument_count, temporary_count) =
                    mutator.heap().frame_register_shape(home);
                mutator
                    .heap_mut()
                    .frame_grow_temporaries(home, temporary_count + 1);
                mutator.heap_mut().frame_set_local(
                    home,
                    (argument_count + temporary_count) as u16,
                    Value::SmallInteger(9),
                );
                mutator.heap_mut().set_frame_program_counter(home, 1);
            });
            vec![closure]
        });
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let original_0 = mutator.fetch(&original[0]);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let original_code = {
            let closure_data = heap
                .closure_data(*object(&original_0))
                .expect("expected a closure");
            let code = &closure_data.code;
            code.clone()
        };
        let closure_data = heap
            .closure_data(*object(&decoded_0))
            .expect("expected a closure");
        let closure_home = heap
            .closure_home(*object(&decoded_0))
            .expect("expected a closure");
        let code = &closure_data.code;
        assert_eq!(**code, *original_code);
        assert!(
            closure_data.captured_frame.same(closure_home),
            "captured_frame and home were the same frame"
        );
        let home = heap.frame_snapshot(closure_home);
        assert_eq!(home.program_counter, 1);
        assert!(matches!(
            home.temporaries.last(),
            Some(Value::SmallInteger(9))
        ));
        assert!(home.uncontinuable, "`mk` had returned");
        let mixin = heap
            .class_application(class)
            .expect("not a class")
            .mixin
            .clone();
        let FrameKind::Method(declaration) = &home.kind else {
            panic!("home is a method frame")
        };
        assert!(
            Rc::ptr_eq(
                declaration,
                mixin
                    .compiled_method(Symbol::intern("mk"))
                    .expect("the class declares `mk`")
            ),
            "re-resolved against the class's method map"
        );
        assert!(heap.class_of_object(*object(&home.receiver)).same(class));
        assert!(home
            .current_class
            .as_ref()
            .expect("a method frame has a current class")
            .same(class));
        assert!(
            home.sender.is_none(),
            "a returned activation has no sender -- Smalltalk's rule, and what keeps \
             a surviving block from retaining its method's whole call chain"
        );
    });
}

#[test]
fn a_closure_stored_in_a_slot_round_trips_with_an_initializer_home() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class, _original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            vec![expect_ok(
                interpreter.instantiate(class, &[]),
                "instantiate",
            )]
        });
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let held = heap
            .instance_field(*object(&decoded_0), 0)
            .expect("expected an instance");
        let home_kind = heap.frame_kind(
            heap.closure_home(*object(&held))
                .expect("slot holds a closure"),
        );
        let FrameKind::Initializer(home_class) = &home_kind else {
            panic!("home is the initializer frame")
        };
        assert!(home_class.same(class));
    });
}

#[test]
fn the_graph_section_encoding_is_deterministic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | a = 1. blk = [ 2 ]. s = 'three'. |
        ) (
        ) : ()
    "};
    let mut encodings = Vec::new();
    for _ in 0..2 {
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let platform = interpreter.platform();
        // Read out after the last run, for the encoder, which runs nothing.
        let bytes = interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            let roots = vec![mutator.fetch(&instance)];
            let platform = mutator.fetch(&platform);
            let mut symbols = symbols::Builder::new();
            let mut writer = encode::Writer::new();
            let ids =
                encode::encode_code_table(&mut writer, mutator.heap(), &[class], &mut symbols)
                    .expect("encode code table");
            let mixin_ids = mixin_ids_for(class, mutator, &mut symbols, &ids);
            encode::encode_graph_section(
                &mut writer,
                mutator.heap(),
                &roots,
                &platform,
                &ids,
                class,
                &mixin_ids,
                &mut symbols,
            )
            .expect("encode graph");
            writer.into_bytes()
        });
        encodings.push(bytes);
    }
    assert_eq!(encodings[0], encodings[1]);
}

/// A world whose graph decoder can be fed hand-crafted bytes: an empty
/// class, no compiled code, and the interpreter's own `platform`.
struct GraphFixture {
    interpreter: Interpreter,
    /// Roots, not handles: a fixture outlives every entry it makes, and only
    /// a `Root` may do that. `decode` fetches both inside its own entry.
    class: Root,
    platform: Root,
    codes: Vec<Rc<CompiledCode>>,
    /// Empty by default: most fixtures' hand-crafted bytes never reference a
    /// selector. Tests that do (a frame kind's method name, a closure's
    /// parameters) set this to a table built by `table_with` first.
    symbols: symbols::Table,
    /// `EMPTY`'s one declaration, so mixin id 0 is in range and every
    /// larger id is not.
    mixins: Vec<Rc<Mixin>>,
    /// What the section's platform paths resolved to, by index. Empty by
    /// default: most fixtures name no path.
    platform_paths: Vec<Root>,
}

impl GraphFixture {
    fn new() -> GraphFixture {
        GraphFixture::with_source(EMPTY)
    }

    /// A fixture around `source`'s declarations, for a hand-crafted section
    /// that names rows `EMPTY` does not have.
    fn with_source(source: &str) -> GraphFixture {
        let (mut interpreter, class) = load(source);
        let platform = interpreter.platform();
        let mixins = interpreter
            .enter(|mutator| mixin_preorder(mutator.heap(), class_reference(mutator, &class)));
        GraphFixture {
            interpreter,
            class,
            platform,
            codes: Vec::new(),
            symbols: symbols::Table::empty(),
            mixins,
            platform_paths: Vec::new(),
        }
    }

    /// Decodes `bytes` and hands the outcome to `inspect` inside the entry
    /// that decoded it: a `DecodedGraph` holds handles, so it can never be
    /// answered out of the entry, and every read of it happens in `inspect`.
    ///
    /// `&mut self` because decoding a class record rebuilds its application
    /// through `Mutator::apply_mixin`, which writes to the shared cache.
    fn decode<R>(
        &mut self,
        bytes: &[u8],
        inspect: impl for<'gc> FnOnce(
            &Heap<'gc>,
            Result<decode::DecodedGraph<'gc>, decode::DecodeError>,
        ) -> R,
    ) -> R {
        let class = &self.class;
        let platform = &self.platform;
        let codes = &self.codes;
        let mixins = &self.mixins;
        let symbols = &self.symbols;
        let platform_paths = &self.platform_paths;
        self.interpreter.enter(|mutator| {
            let class = class_reference(mutator, class);
            let platform = mutator.fetch(platform);
            let root_levels = encode::root_levels(mutator.heap(), class);
            let platform_paths: Vec<Value> = platform_paths
                .iter()
                .map(|path| mutator.fetch(path))
                .collect();
            let decoded = {
                let mut context = decode::DecodeContext {
                    mutator,
                    root_class: &class,
                    codes,
                    platform,
                    mixins,
                    root_levels: &root_levels,
                    platform_paths: &platform_paths,
                };
                decode::decode_graph_section(&mut decode::Reader::new(bytes), &mut context, symbols)
            };
            inspect(mutator.heap(), decoded)
        })
    }

    /// `decode` for a section that must decode: `inspect` sees the graph,
    /// and a `DecodeError` fails the test with `what`.
    fn decoded<R>(
        &mut self,
        bytes: &[u8],
        what: &str,
        inspect: impl for<'gc> FnOnce(&Heap<'gc>, &decode::DecodedGraph<'gc>) -> R,
    ) -> R {
        self.decode(bytes, |heap, decoded| match decoded {
            Ok(graph) => inspect(heap, &graph),
            Err(error) => panic!("{what}: expected a decoded graph, got a decode error: {error}"),
        })
    }

    /// The `DecodeError` a hand-crafted graph section must produce. Never
    /// `{:?}`s the success value: `DecodedGraph` has no `Debug` impl, so a
    /// manual match, as below, is the only way to reach the error.
    fn decode_error(&mut self, bytes: &[u8]) -> decode::DecodeError {
        self.decode(bytes, |_, decoded| match decoded {
            Ok(_) => panic!("expected a decode error, got a decoded graph"),
            Err(error) => error,
        })
    }
}

/// `object_count`, `frame_count` -- the two counts every graph section
/// starts with.
fn graph_counts(writer: &mut encode::Writer, objects: u32, frames: u32) {
    writer.write_u32(objects);
    writer.write_u32(frames);
}

/// A one-object graph section whose single object is an array of one
/// element, written by `element`.
fn one_array_object(element: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Array));
    writer.write_u32(5); // identity hash
    writer.write_u8(OBJECT_ARRAY); // object body: Array
    writer.write_u32(1); // one element
    element(&mut writer);
    writer.into_bytes()
}

#[test]
fn an_object_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_array_object(|writer| {
        writer.write_u8(VALUE_OBJECT_ID); // value tag: object-table id
        writer.write_u32(99);
    });
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::BadObjectId(99));
    assert_eq!(error.to_string(), "object id 99 out of range");
}

#[test]
fn a_frame_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 0, 1);
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_u32(0); // no arguments
    writer.write_u32(0); // no temporaries
    writer.write_u32(0); // program_counter
    writer.write_boolean(true); // has a sender...
    writer.write_u32(99); // ...whose id is out of range
    let bytes = writer.into_bytes();
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::BadFrameId(99));
    assert_eq!(error.to_string(), "frame id 99 out of range");
}

#[test]
fn a_huge_object_or_frame_count_does_not_allocate_gigabytes() {
    let mut fixture = GraphFixture::new();
    for (objects, frames) in [(u32::MAX, 0), (0, u32::MAX)] {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, objects, frames);
        let bytes = writer.into_bytes();
        assert_eq!(
            fixture.decode_error(&bytes),
            decode::DecodeError::UnexpectedEndOfFile
        );
    }
}

#[test]
fn an_invalid_value_tag_in_the_graph_is_a_decode_error_not_a_panic() {
    let bytes = one_array_object(|writer| writer.write_u8(200));
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "value",
            tag: 200
        }
    );
}

#[test]
fn an_invalid_object_body_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_BUILTIN);
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    writer.write_u32(1);
    writer.write_u8(200); // no such object body
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "object body",
            tag: 200
        }
    );
}

#[test]
fn an_invalid_class_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(200); // none of `Builtin` (0), `UserRoot` (1), `Record` (2)
                          // Padded out to the shortest object record the wire
                          // admits, so `bounded_count` admits the announced
                          // record and the class tag itself is what the decoder
                          // rejects.
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_STRING); // object body: String
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "class",
            tag: 200
        }
    );
}

#[test]
fn a_symbol_recorded_twice_is_refused() {
    // A canonical heap never encodes two records of one symbol, so a graph
    // that holds two is crafted or corrupt; merging them would mean
    // rewriting every reference to the second.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2, 0);
    for identity_hash in [1, 2] {
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Symbol));
        writer.write_u32(identity_hash);
        writer.write_u8(OBJECT_SYMBOL);
        writer.write_u32(0); // symbol-table id 0: "foo"
    }
    writer.write_u32(0); // no roots
    let mut fixture = GraphFixture::new();
    fixture.symbols = table_with(&["foo"]);
    let error = fixture.decode_error(&writer.into_bytes());
    assert_eq!(
        error,
        decode::DecodeError::DuplicateSymbol("foo".to_string())
    );
    assert_eq!(error.to_string(), "symbol #foo is recorded twice");
}

#[test]
fn a_symbol_recorded_with_a_class_other_than_symbol_is_refused() {
    // `decode_graph_section` enters every `OBJECT_SYMBOL` record into the
    // heap's canonical symbol table as it decodes it, before anything else
    // runs -- so a record whose class tag names something other than
    // `Symbol` would still answer every later literal of that text if it
    // were let through.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_BUILTIN);
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::String));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_SYMBOL);
    writer.write_u32(0); // symbol-table id 0: "foo"
    writer.write_u32(0); // no roots
    let mut fixture = GraphFixture::new();
    fixture.symbols = table_with(&["foo"]);
    let error = fixture.decode_error(&writer.into_bytes());
    assert_eq!(
        error,
        decode::DecodeError::SymbolOfWrongClass("foo".to_string())
    );
    assert_eq!(
        error.to_string(),
        "symbol #foo is recorded with a class other than Symbol"
    );
}

// ---------------------------------------------------------------------------
// Class records. A class object is an ordinary object-table entry, so every
// id it names -- its mixin, its superclass, its enclosing object, its
// declaring class -- is attacker-controlled and has to be range-checked
// before it indexes anything.
// ---------------------------------------------------------------------------

/// One class record: its header, naming its metaclass -- the record
/// `metaclass_id` -- an identity hash, the body tag, the mixin-table row id,
/// then the three references `references` writes, and no applied clause --
/// an ordinary class, not a level a mixin application added.
fn class_record(
    writer: &mut encode::Writer,
    metaclass_id: u32,
    mixin_id: u32,
    references: impl FnOnce(&mut encode::Writer),
) {
    writer.write_u8(CLASS_RECORD); // class tag: its metaclass record
    writer.write_u32(metaclass_id);
    writer.write_u32(7); // identity hash
    writer.write_u8(OBJECT_CLASS); // object body: Class
    writer.write_u8(MIXIN_ROW); // mixin reference: a mixin-table row
    writer.write_u32(mixin_id);
    references(writer);
    writer.write_boolean(false); // not an applied level
}

/// One metaclass record: its header naming `Metaclass`, an identity hash,
/// the body tag and the reference back to its class, the record `class_id`.
fn metaclass_record(writer: &mut encode::Writer, class_id: u32) {
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Metaclass));
    writer.write_u32(8); // identity hash
    writer.write_u8(OBJECT_METACLASS); // object body: Metaclass
    writer.write_u8(CLASS_RECORD); // its class: an object-table record
    writer.write_u32(class_id);
}

/// A graph section holding one class record (object 0), its metaclass
/// record (object 1) and no roots.
fn one_class_object(mixin_id: u32, references: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2, 0);
    class_record(&mut writer, 1, mixin_id, references);
    metaclass_record(&mut writer, 0);
    writer.write_u32(0); // no roots
    writer.into_bytes()
}

/// `superclass`, `enclosing` and `declaring_class` all absent.
fn no_class_references(writer: &mut encode::Writer) {
    writer.write_boolean(false);
    writer.write_boolean(false);
    writer.write_boolean(false);
}

#[test]
fn a_well_formed_class_record_decodes_into_a_live_class_object() {
    // The positive side of the four refusals below: without it a decoder
    // that rejected every class record would pass all of them.
    let mut fixture = GraphFixture::new();
    let row_0 = fixture.mixins[0].clone();
    fixture.decoded(
        &one_class_object(0, no_class_references),
        "a class record naming row 0 of the mixin table",
        |heap, graph| {
            let application = heap
                .class_application(graph.objects[0])
                .expect("a class object");
            assert!(
                Rc::ptr_eq(&application.mixin, &row_0),
                "the record's mixin id names the decoded declaration tree, row by row"
            );
        },
    );
}

#[test]
fn two_class_records_with_the_same_mixin_and_superclass_share_one_application() {
    // The decoder's own half of the round-trip sharing property: nothing in
    // the two records ties them together, so if `apply_mixin`'s cache did
    // not answer both, sharing could only come back by accident.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 4, 0);
    class_record(&mut writer, 2, 0, no_class_references);
    class_record(&mut writer, 3, 0, no_class_references);
    metaclass_record(&mut writer, 0);
    metaclass_record(&mut writer, 1);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    GraphFixture::new().decoded(&bytes, "two class records", |heap, graph| {
        let first_application = heap
            .class_application(graph.objects[0])
            .expect("a class object");
        let second_application = heap
            .class_application(graph.objects[1])
            .expect("a class object");
        assert!(!graph.objects[0].same(graph.objects[1]));
        assert!(
            Rc::ptr_eq(&first_application, &second_application),
            "one rebuilt application, shared"
        );
    });
}

#[test]
fn a_class_records_mixin_id_out_of_range_is_a_decode_error_not_a_panic() {
    assert_eq!(
        GraphFixture::new().decode_error(&one_class_object(7, no_class_references)),
        decode::DecodeError::BadMixinId(7)
    );
}

#[test]
fn a_class_records_superclass_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |writer| {
        writer.write_boolean(true);
        writer.write_u8(CLASS_RECORD); // class tag: object-table record
        writer.write_u32(9); // ... past the end of a two-object table
        writer.write_boolean(false);
        writer.write_boolean(false);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_records_enclosing_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |writer| {
        writer.write_boolean(false);
        writer.write_boolean(true);
        writer.write_u8(VALUE_OBJECT_ID); // value tag: object-table id
        writer.write_u32(9);
        writer.write_boolean(false);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_records_enclosing_scalar_is_a_decode_error_not_a_silent_nil() {
    let bytes = one_class_object(0, |writer| {
        writer.write_boolean(false);
        writer.write_boolean(true);
        writer.write_u8(VALUE_SMALL_INTEGER); // value tag: SmallInteger -- not an object at all
        writer.write_i64(1);
        writer.write_boolean(false);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::EnclosingNotAnObject
    );
}

#[test]
fn a_class_records_declaring_class_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |writer| {
        writer.write_boolean(false);
        writer.write_boolean(false);
        writer.write_boolean(true);
        writer.write_u8(CLASS_RECORD); // class tag: object-table record
        writer.write_u32(9);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_reference_naming_a_record_that_is_not_a_class_is_a_decode_error() {
    // An in-range id is not enough: nothing in the stream forces a tag-2
    // class tag to name a *class* record, and a class object whose
    // superclass is an array has no application chain to rebuild.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 3, 0);
    class_record(&mut writer, 2, 0, |writer| {
        writer.write_boolean(true);
        writer.write_u8(CLASS_RECORD);
        writer.write_u32(1); // object 1, which is an array
        writer.write_boolean(false);
        writer.write_boolean(false);
    });
    writer.write_u8(CLASS_BUILTIN); // object 1's class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Array));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_ARRAY); // object body: Array
    writer.write_u32(0); // no elements
    metaclass_record(&mut writer, 0);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::NotAClassReference
    );
}

#[test]
fn class_objects_inheriting_from_each_other_in_a_cycle_are_refused() {
    // An application is derived from the superclass's, so a cycle has no
    // order in which the two could be rebuilt. Refusing is what keeps the
    // rebuild a finite walk rather than one that never ends.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 4, 0);
    for (metaclass, other) in [(2u32, 1u32), (3, 0)] {
        class_record(&mut writer, metaclass, 0, move |writer| {
            writer.write_boolean(true);
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(other);
            writer.write_boolean(false);
            writer.write_boolean(false);
        });
    }
    metaclass_record(&mut writer, 0);
    metaclass_record(&mut writer, 1);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::SuperclassCycle
    );
}

/// A one-frame graph section up to (but excluding) the frame-kind tag.
fn one_frame_prefix() -> encode::Writer {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 0, 1);
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_u32(0); // no arguments
    writer.write_u32(0); // no temporaries
    writer.write_u32(0); // program_counter
    writer.write_boolean(false); // no sender
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer
}

#[test]
fn an_invalid_frame_kind_tag_is_a_decode_error_not_a_panic() {
    let mut writer = one_frame_prefix();
    writer.write_u8(200);
    // Padded out to the shortest frame record the wire admits (see the
    // class-tag test), so the kind tag is what the decoder rejects.
    writer.write_boolean(false); // continuable
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: 200
        }
    );
}

/// The byte after `program_counter` is the frame's `sender` flag. A record
/// with one stray byte there has it read as "a sender follows", and the next
/// four bytes (the sender, home and current-class flags and the kind tag) as
/// a sender id far outside the one-frame table, so it is refused rather than
/// read as a different frame.
#[test]
fn a_frame_record_with_a_stray_byte_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 0, 1);
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_u32(0); // no arguments
    writer.write_u32(0); // no temporaries
    writer.write_u32(0); // program_counter
    writer.write_u8(1); // the stray byte
    writer.write_boolean(false); // no sender
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    writer.write_boolean(false); // continuable
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert!(GraphFixture::new().decode(&bytes, |_, decoded| decoded.is_err()));
}

#[test]
fn a_method_frame_naming_a_selector_the_class_lost_is_a_decode_error() {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_METHOD); // frame kind: Method
    writer.write_u32(0); // selector id 0 ("gone")
    let bytes = writer.into_bytes();
    let mut fixture = GraphFixture::new();
    fixture.symbols = table_with(&["gone"]);
    let error = fixture.decode_error(&bytes);
    assert_eq!(
        error,
        decode::DecodeError::UnknownSelector("gone".to_string())
    );
}

#[test]
fn a_closure_objects_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(42); // code id, out of range for an empty code table
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadCodeId(42)
    );
}

/// A closure frame names its closure object by id, and an id past the
/// object table is refused where it is read, before anything indexes by it.
#[test]
fn a_closure_frames_object_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_CLOSURE); // frame kind: a closure...
    writer.write_u32(42); // ...naming an object past the empty object table
    writer.write_boolean(false); // continuable
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(42)
    );
}

#[test]
fn a_truncated_graph_section_is_a_decode_error_not_a_panic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    // Read out after the last run, for the encoder and the decoder, which run
    // nothing.
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let roots = vec![mutator.fetch(&instance)];
        let platform = mutator.fetch(&platform);
        let mut symbols = symbols::Builder::new();
        let mut writer = encode::Writer::new();
        let ids = encode::encode_code_table(&mut writer, mutator.heap(), &[class], &mut symbols)
            .expect("encode code table");
        let before = writer.into_bytes().len();
        let mut writer = encode::Writer::new();
        let mixin_ids = mixin_ids_for(class, mutator, &mut symbols, &ids);
        encode::encode_graph_section(
            &mut writer,
            mutator.heap(),
            &roots,
            &platform,
            &ids,
            class,
            &mixin_ids,
            &mut symbols,
        )
        .expect("encode graph");
        let graph = writer.into_bytes();
        let symbol_table = symbols.build();
        assert!(before > 0 && graph.len() > 8);
        let mixin_order = mixin_preorder(mutator.heap(), class);
        let root_levels = encode::root_levels(mutator.heap(), class);
        let mut context = decode::DecodeContext {
            mutator,
            root_class: &class,
            codes: &[],
            platform,
            mixins: &mixin_order,
            root_levels: &root_levels,
            platform_paths: &[],
        };
        for cut in 1..graph.len() {
            let mut reader = decode::Reader::new(&graph[..cut]);
            // Every prefix is either a clean `DecodeError` or -- for a prefix
            // that happens to be self-consistent -- a successful decode. What
            // it must never be is a panic.
            let _ = decode::decode_graph_section(&mut reader, &mut context, &symbol_table);
        }
    });
}

#[test]
fn a_closure_capturing_a_block_activation_round_trips() {
    // `[:x | [ x ] ] value: 5` runs the outer block, so the inner closure's
    // `captured_frame` is a *block activation* -- a `FrameKind::Closure`
    // frame, the one frame kind the quiesced worlds above never produce.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (mut interpreter, class, _original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            vec![expect_ok(
                interpreter.send(&instance, Symbol::intern("mk"), &[]),
                "mk",
            )]
        });
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let inner = heap
            .closure_data(*object(&decoded_0))
            .expect("expected the inner closure");
        let inner_home = heap
            .closure_home(*object(&decoded_0))
            .expect("expected the inner closure");
        assert!(
            !inner.captured_frame.same(inner_home),
            "the inner closure was captured by the block activation, not by `mk`"
        );
        let captured = heap.frame_snapshot(inner.captured_frame);
        let FrameKind::Closure(_) = &captured.kind else {
            panic!("the captured frame is a block activation")
        };
        // The optimized lowering converts `[ x ]`: `x` is copied into the
        // inner closure when it is created, so the outer block's activation
        // returned without copying its registers into its frame
        // (`docs/design/execution.md`, "Lifecycle"). The value survives in
        // the closure, not in the frame.
        assert!(captured.arguments.is_empty());
        assert!(matches!(inner.copied.first(), Some(Value::SmallInteger(5))));
        assert!(
            captured
                .home
                .as_ref()
                .expect("a block activation has a home")
                .same(inner_home),
            "both closures share `mk`'s activation as their home"
        );
        let home_kind = heap.frame_kind(inner_home);
        let FrameKind::Method(declaration) = &home_kind else {
            panic!("the home is `mk`'s method frame")
        };
        let application = heap.class_application(class).expect("not a class");
        assert!(Rc::ptr_eq(
            declaration,
            application
                .mixin
                .compiled_method(Symbol::intern("mk"))
                .expect("the class declares `mk`")
        ));
    });
}

/// A closure frame names the closure object it runs, rather than carrying a
/// copy of its record: after a round trip the inner closure's captured frame
/// -- the outer block's activation -- names a decoded closure object, and it
/// is the outer block, with its one parameter.
#[test]
fn a_closure_frame_names_the_closure_object_it_runs() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            vec![expect_ok(
                interpreter.send(&instance, Symbol::intern("mk"), &[]),
                "mk",
            )]
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let inner = heap
            .closure_data(*object(&decoded_0))
            .expect("expected the inner closure");
        let inner_home = heap
            .closure_home(*object(&decoded_0))
            .expect("expected the inner closure");
        let FrameKind::Closure(outer) = heap.frame_kind(inner.captured_frame) else {
            panic!("the captured frame is a block activation")
        };
        assert_eq!(
            heap.closure_parameter_count(outer),
            Some(1),
            "the outer block takes `x`"
        );
        assert!(
            heap.closure_home(outer)
                .expect("a closure frame names a closure object")
                .same(inner_home),
            "both closures share `mk`'s activation as their home"
        );
    });
}

/// A one-frame graph section whose frame carries a frame kind no build
/// assigns, for the two readers of a graph section below.
fn unknown_frame_kind_graph() -> Vec<u8> {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_MIXIN_APPLICATION_MESSAGE + 1); // one past the last frame kind

    // Padded past the shortest frame record the wire admits, so
    // `bounded_count` admits the announced frame and the kind tag itself is
    // what the reader rejects.
    writer.write_boolean(false);
    writer.write_u32(0);
    writer.into_bytes()
}

/// A frame kind no build assigns is refused rather than read as some other
/// record's layout.
#[test]
fn an_unknown_frame_kind_tag_is_refused() {
    assert_eq!(
        GraphFixture::new().decode_error(&unknown_frame_kind_graph()),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: FRAME_MIXIN_APPLICATION_MESSAGE + 1
        }
    );
}

/// `inspect` walks the graph with its own reader, so it has to refuse an
/// unknown frame kind itself: an artifact `load` refuses must not inspect as
/// if it were well-formed. The describing sections come from a real snapshot and
/// the graph section is the hand-written one above, spliced in where the
/// snapshot's own graph began.
#[test]
fn inspect_refuses_an_unknown_frame_kind_tag() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let platform = interpreter.platform();
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .expect("snapshot")
    });
    let graph_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|section| section.name == "graph")
        .expect("a snapshot has a graph section")
        .start;
    bytes.truncate(graph_start);
    bytes.extend_from_slice(&unknown_frame_kind_graph());
    let error = match image::inspect::inspect(&bytes) {
        Ok(_) => panic!("inspect accepted an unknown frame kind tag"),
        Err(error) => error,
    };
    let expected = format!(
        "invalid frame kind tag {}",
        FRAME_MIXIN_APPLICATION_MESSAGE + 1
    );
    assert_eq!(error, expected);
    assert_eq!(load_error(&bytes), expected);
}

/// `inspect` renders a closure frame as the object id it names: after
/// `[:x | [ x ] ] value: 5`, the inner closure's captured frame is the outer
/// block's activation, and the object the frame line names is the outer
/// block itself, with its one parameter.
#[test]
fn inspect_names_the_closure_object_a_closure_frame_runs() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let inner = expect_ok(interpreter.send(&instance, Symbol::intern("mk"), &[]), "mk");
    let platform = interpreter.platform();
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let inner = mutator.fetch(&inner);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[inner],
        )
        .expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let closure_frames: Vec<&String> = report
        .graph_frames
        .iter()
        .filter(|frame| frame.starts_with("closure object "))
        .collect();
    let [frame] = closure_frames[..] else {
        panic!(
            "expected one closure frame, the outer block's activation, in {:?}",
            report.graph_frames
        )
    };
    let id: usize = frame
        .strip_prefix("closure object ")
        .and_then(|rest| rest.split(',').next())
        .expect("the id ends the kind")
        .parse()
        .expect("an object id");
    let object = &report.graph_objects[id];
    assert!(
        object.contains("closure code ") && object.contains(", copied []"),
        "object {id} should be the outer block, copying nothing: {object}"
    );
}

/// A closure frame's object id is in range but names a string: refused once
/// every record is read, like a class reference that names an ordinary
/// object, since only then is it known what the record was.
#[test]
fn a_closure_frame_naming_an_object_that_is_not_a_closure_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 1);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::String));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_STRING); // object body: String
    writer.write_str("not a closure");
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_u32(0); // no arguments
    writer.write_u32(0); // no temporaries
    writer.write_u32(0); // program_counter
    writer.write_boolean(false); // no sender
    writer.write_boolean(true); // a home...
    writer.write_u32(0); // ...frame 0 itself
    writer.write_boolean(false); // no current class
    writer.write_u8(FRAME_CLOSURE); // frame kind: a closure...
    writer.write_u32(0); // ...naming object 0, the string
    writer.write_boolean(false); // continuable
    writer.write_u32(0); // no roots
    let error = GraphFixture::new().decode_error(&writer.into_bytes());
    assert_eq!(error, decode::DecodeError::NotAClosureReference);
    assert_eq!(
        error.to_string(),
        "a closure frame in the image names an object that is not a closure"
    );
}

#[test]
fn a_top_level_frame_kind_decodes() {
    // Nothing that runs a program *builds* a `FrameKind::TopLevel` frame (see
    // `types::FrameKind`), so only a hand-written section can reach the
    // decoder's tag-2 branch.
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    writer.write_boolean(false); // continuable
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    GraphFixture::new().decoded(&bytes, "a TopLevel frame", |_, decoded| {
        assert!(decoded.roots.is_empty());
        assert_eq!(decoded.max_identity_hash, 0);
    });
}

/// A block `CompiledCode` taking `argument_count` arguments and
/// `copied_count` copied values, in one register more than the two together
/// -- enough for the graph decoder, which never runs it.
fn block_code(argument_count: u16, copied_count: u16) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instructions: vec![Instruction::ImplicitReturn { register: 0 }],
        argument_count,
        register_count: argument_count + copied_count + 1,
        statement_program_counter: vec![],
        family_size: 0,
        copied_count,
        outer_reach: 0,
        unwind_marker: None,
        source: Span::new(0, 0),
    })
}

/// A graph section holding one closure object and the one `TopLevel` frame
/// it points at, carrying `copied` copied values against code id 0. Every
/// field but their count is well-formed, so *only* the copied-count
/// reconciliation can reject it.
fn closure_object_with_copied_values(copied: &[i64]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 1);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(0); // code id
    writer.write_u32(0); // captured frame id -- frame 0 exists
    writer.write_u16(copied.len() as u16);
    for value in copied {
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(*value);
    }
    writer.write_u8(VALUE_NIL); // frame 0: receiver nil
    writer.write_u32(0); // no arguments
    writer.write_u32(0); // no temporaries
    writer.write_u32(0); // program_counter
    writer.write_boolean(false); // no sender
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    writer.write_boolean(false); // continuable
    writer.write_u32(1); // one root...
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(0); // ...the closure
    writer.into_bytes()
}

#[test]
fn a_closure_records_its_copied_values_and_a_wrong_count_is_refused() {
    // The record carries the values and the body declares how many
    // registers take them (`Heap::fill_copied_values`), so the two are
    // separate wire fields and `decode_closure_record` is the only place
    // that sees both. A shorter list would leave a register the block reads
    // as a plain local holding nil, at exit status 0; a longer one would
    // hand values to registers past the block's own.
    //
    // The control comes first: without it, a reconciliation that rejected
    // *everything* would look correct.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(0, 2));
    fixture.decoded(
        &closure_object_with_copied_values(&[7, 8]),
        "the copied count agrees",
        |heap, decoded| {
            let closure_data = heap
                .closure_data(*object(&decoded.roots[0]))
                .expect("expected a closure");
            assert!(matches!(
                closure_data.copied[..],
                [Value::SmallInteger(7), Value::SmallInteger(8)]
            ));
        },
    );

    let error = fixture.decode_error(&closure_object_with_copied_values(&[7]));
    assert_eq!(
        error,
        decode::DecodeError::CopiedCountMismatch {
            copied: 1,
            copied_count: 2
        }
    );
    assert_eq!(
        error.to_string(),
        "the image supplies 1 copied values for a block that is handed 2"
    );
}

/// A closure whose code declares no copied values at all: every closure the
/// reference lowering builds, and every fallback closure the optimized one
/// builds, since both reach their variables by reference.
#[test]
fn a_closure_record_with_no_copied_values_decodes() {
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(1, 0));
    fixture.decoded(
        &closure_object_with_copied_values(&[]),
        "no copied values",
        |heap, decoded| {
            let closure_data = heap
                .closure_data(*object(&decoded.roots[0]))
                .expect("expected a closure");
            assert!(closure_data.copied.is_empty());
            // The arity a call is checked against now has one source.
            assert_eq!(
                heap.closure_parameter_count(*object(&decoded.roots[0])),
                Some(1)
            );
        },
    );
}

// ---------------------------------------------------------------------------
// Whole-image `snapshot`/`load`.
// ---------------------------------------------------------------------------

#[test]
fn a_snapshot_loads_into_a_bytecode_interpreter_with_its_state_and_a_safe_hash_counter() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Counter = Object new (
          | n ::= 0. |
        ) (
          public bump = ( n:: n + 1. ^n )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("bump"), &[]),
        "bump",
    );
    let platform = interpreter.platform();
    let (bytes, max_hash) = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let max_hash = mutator.heap_mut().ensure_identity_hash(*object(&instance));
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let bytes = image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot");
        (bytes, max_hash)
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        assert!(
            !root_mixin_of(mutator.heap(), root_class).methods.is_empty(),
            "the image's methods are rebuilt, each carrying its body"
        );
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert!(mutator
            .heap()
            .class_of_object(*object(&root_0))
            .same(root_class));
    });
    let bumped = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("bump"), &[]),
        "bump",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&bumped), Value::SmallInteger(2)),
            "slot state survived: 1 before the snapshot, 2 after"
        );
        let fresh = mutator.new_string("fresh".to_string());
        assert!(
            mutator.heap_mut().ensure_identity_hash(fresh) > max_hash,
            "no hash collision with loaded objects"
        );
    });
}

/// A decoded object answers the id of the application its class was given
/// while loading, never one read from the bytes (ids are per heap and not
/// serialized), and never the placeholder's: the decoder fills a record
/// before the class it names has its application, so the id is stamped only
/// once every class has one (`docs/design/dispatch.md`, "3c").
#[test]
fn a_loaded_image_answers_the_dispatch_ids_its_classes_were_given() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Counter = Object new (
          | count ::= 0. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        let ids = mutator.heap().class_dispatch_ids(root_class);
        assert_ne!(ids.instances, DispatchId::NONE);
        let instance = mutator.fetch(&loaded.roots[0]);
        assert_eq!(mutator.heap().dispatch_id(&instance), ids.instances);
        // A class object is an instance of its metaclass.
        let metaclass = mutator
            .metaclass_of(root_class)
            .expect("the root class has a metaclass");
        let metaclass_ids = mutator.heap().class_dispatch_ids(metaclass);
        assert_ne!(metaclass_ids.instances, DispatchId::NONE);
        assert_eq!(
            mutator.heap().dispatch_id(&Value::Object(root_class)),
            metaclass_ids.instances
        );
    });
}

/// Exercises the ordering the previous test's comment relies on but never
/// pins: an object's own record can name a class whose record the table
/// carries later. `GraphTable::build` (`image/encode.rs`) always gives a
/// visited object its table id before it recurses into that object's own
/// class (`visit_value` pushes the class reference onto the traversal stack
/// right after registering the object, ahead of the object's fields), so a
/// root that is the *only* path to its class's record is always written
/// before it: passing `alpha`, an instance of the nested class `Alpha`, as
/// the sole snapshot root -- with `Root` itself, the tagged root class, kept
/// out of the roots list -- guarantees `Alpha`'s own record cannot be
/// discovered by any earlier path than `alpha`'s. `Beta`, the second nested
/// class, is never touched by the graph at all, showing the ordering holds
/// with more than one candidate class in scope.
#[test]
fn a_loaded_root_gets_its_dispatch_id_though_its_class_record_comes_after_it() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new (
        ) (
          public class Alpha = Object new (
            | value ::= 0. |
          ) ()
          public class Beta = Object new () ()
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let root = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Root");
    let alpha_class = expect_ok(
        interpreter.send(&root, Symbol::intern("Alpha"), &[]),
        "Alpha",
    );
    let alpha = expect_ok(
        interpreter.send(&alpha_class, Symbol::NEW, &[]),
        "Alpha new",
    );

    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let alpha = mutator.fetch(&alpha);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[alpha],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let instance = mutator.fetch(&loaded.roots[0]);
        let decoded_alpha_class = mutator.heap().class_of_object(*object(&instance));
        let ids = mutator.heap().class_dispatch_ids(decoded_alpha_class);
        assert_ne!(ids.instances, DispatchId::NONE);
        assert_eq!(mutator.heap().dispatch_id(&instance), ids.instances);
    });
}

/// The text a `Holder`'s `trace` slot holds, read through its getter.
fn trace_of(interpreter: &mut Interpreter, holder: &Root) -> String {
    let trace = expect_ok(
        interpreter.send(holder, Symbol::intern("trace"), &[]),
        "trace",
    );
    interpreter.enter(|mutator| match mutator.fetch(&trace) {
        Value::Object(string) => mutator
            .heap()
            .str_value(string)
            .expect("trace holds a String"),
        other => panic!("trace answered {}", mutator.heap().describe(&other)),
    })
}

/// A lazy slot (§6.3.3) survives an image both filled and empty: the
/// decoded class keeps the slot lazy, so the empty one still evaluates its
/// initializer on its first read after the load, and the filled one answers
/// its value without evaluating it again. `Holder` sits over `Base`, whose
/// two slots put the lazy slot at a non-zero `own_slot_base`: a slot decoded
/// as eager would answer `nil` for the empty one, and a getter run with a
/// slot base of 0 rather than its defining class's would read and write
/// `Base`'s first field instead.
#[test]
fn a_lazy_slot_survives_an_image_filled_and_unfilled() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new (
          | public first = Holder new. public second = Holder new. |
        ) (
          class Base = Object new (| public left = 'left'. public right = 'right'. |) () : ()
          class Holder = Base new (| public trace ::= ''. |) (
            lazy public computed = mark.
            mark = ( trace:: trace, '!'. ^42 )
          ) : ()
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let root = expect_ok(interpreter.instantiate(&class, &[]), "Root new");
    let first = expect_ok(
        interpreter.send(&root, Symbol::intern("first"), &[]),
        "first",
    );
    expect_ok(
        interpreter.send(&first, Symbol::intern("computed"), &[]),
        "computed before the snapshot",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let root = mutator.fetch(&root);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[root],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    for (name, marks_before, marks_after) in [("first", "!", "!"), ("second", "", "!")] {
        let holder = expect_ok(
            loaded.interpreter.send(&root, Symbol::intern(name), &[]),
            name,
        );
        assert_eq!(
            trace_of(&mut loaded.interpreter, &holder),
            marks_before,
            "{name}'s trace as the image recorded it"
        );
        for _ in 0..2 {
            let computed = expect_ok(
                loaded
                    .interpreter
                    .send(&holder, Symbol::intern("computed"), &[]),
                "computed after the load",
            );
            loaded.interpreter.enter(|mutator| {
                assert!(
                    matches!(mutator.fetch(&computed), Value::SmallInteger(42)),
                    "{name}'s lazy slot answers its value"
                );
            });
        }
        assert_eq!(
            trace_of(&mut loaded.interpreter, &holder),
            marks_after,
            "{name}'s initializer ran at most once, before or after the load"
        );
        let left = expect_ok(
            loaded
                .interpreter
                .send(&holder, Symbol::intern("left"), &[]),
            "left",
        );
        loaded
            .interpreter
            .enter(|mutator| match mutator.fetch(&left) {
                Value::Object(string) => assert_eq!(
                    mutator.heap().str_value(string).as_deref(),
                    Some("left"),
                    "{name}'s inherited slot keeps its value"
                ),
                other => panic!("left answered {}", mutator.heap().describe(&other)),
            });
    }
}

/// A lazy slot's getter is one of its row's compiled methods; the slot
/// record only says the slot is lazy. A row that declares a lazy slot and
/// no unary method of its name would leave the slot with no getter at all
/// -- no send could ever read it -- so the decoder refuses it. Patched into
/// a real binary: the root row's one eager slot is turned lazy, and nothing
/// else changes.
#[test]
fn a_lazy_slot_without_its_getter_is_refused() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder = Object new (| trace = 0. |) (
          main: p args: q = ( ^trace )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let origin = Origin::File(PathBuf::from("t.ns"));
    let mut binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build")
    });
    assert!(image::load(&binary).is_ok(), "the fixture itself must load");

    let (mixin_table_at, symbols, codes) = {
        let mut reader = decode::Reader::new(&binary);
        let header = decode::read_header(&mut reader).expect("valid header");
        if header.has_source {
            reader.read_str().expect("source");
        }
        decode::decode_source_origin(&mut reader).expect("origin");
        let symbols = symbols::decode(&mut reader).expect("symbols");
        let mut table = decode::decode_code_table(&mut reader, &symbols).expect("code table");
        decode::verify_code_table(&table).expect("verify code table");
        decode::number_call_site_families(&mut table).expect("number call site families");
        (reader.position(), symbols, table.codes)
    };
    // The table's row count, then the root row's name, access and span, then
    // its slot count and its first slot's name: its kind byte follows.
    let kind_at = mixin_table_at + 4 + 4 + 1 + 4 + 4 + 4 + 4;
    assert_eq!(binary[kind_at], SLOT_IMMUTABLE, "the eager slot's kind");
    binary[kind_at] = SLOT_LAZY_IMMUTABLE;

    let mut reader = decode::Reader::new(&binary[mixin_table_at..]);
    match mixins::decode(&mut reader, &symbols, &codes) {
        Err(decode::DecodeError::LazySlotWithoutGetter(name)) => assert_eq!(name, "trace"),
        Err(error) => panic!("expected LazySlotWithoutGetter, got a different error: {error}"),
        Ok(_) => panic!("expected LazySlotWithoutGetter, got Ok"),
    }
    let error = load_error(&binary);
    assert!(
        error.contains("lazy slot `trace`"),
        "unexpected error: {error}"
    );
}

#[test]
fn bytes_after_the_last_section_are_refused_not_ignored() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .expect("snapshot")
    });
    assert!(image::load(&bytes).is_ok(), "the fixture itself must load");

    bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let error = load_error(&bytes);
    assert!(
        error.contains("trailing"),
        "expected a trailing-bytes error, got: {error}"
    );
}

#[test]
fn load_refuses_a_version_mismatch() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .unwrap()
    });
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    let error = load_error(&bytes);
    assert!(error.contains("version"), "{error}");
}

/// `image::load`'s error message. Never `expect_err`: that would require
/// `Debug` on the `Ok` side, and `LoadedImage` has none -- it carries an
/// `Interpreter`, which has no `Debug` impl either.
fn load_error(bytes: &[u8]) -> String {
    match image::load(bytes) {
        Ok(_) => panic!("expected a load error, got a loaded image"),
        Err(error) => error,
    }
}

#[test]
fn the_source_origin_round_trips_through_the_image() {
    let mut writer = encode::Writer::new();
    encode::encode_source_origin(&mut writer, &Origin::File(PathBuf::from("some/where.ns")));
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_source_origin(&mut reader) {
        Ok(SourceOrigin::File(path)) => assert_eq!(path, PathBuf::from("some/where.ns")),
        Ok(SourceOrigin::Embedded(name)) => panic!("decoded an embedded origin: {name}"),
        Err(error) => panic!("decoding failed: {error}"),
    }
}

#[test]
fn an_embedded_origin_round_trips() {
    let mut writer = encode::Writer::new();
    encode::encode_source_origin(&mut writer, &Origin::Embedded("Manifest.ns".to_string()));
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_source_origin(&mut reader) {
        Ok(SourceOrigin::Embedded(name)) => assert_eq!(name, "Manifest.ns"),
        Ok(SourceOrigin::File(path)) => panic!("decoded a file origin: {}", path.display()),
        Err(error) => panic!("decoding failed: {error}"),
    }
}

#[test]
fn an_unknown_origin_tag_is_refused() {
    let bytes = [2u8, 0, 0, 0, 0];
    let mut reader = decode::Reader::new(&bytes);
    assert!(matches!(
        decode::decode_source_origin(&mut reader),
        Err(decode::DecodeError::InvalidTag {
            what: "source origin",
            tag: 2
        })
    ));
}

#[test]
fn an_invalid_source_origin_tag_is_a_decode_error_not_a_panic() {
    let bytes = vec![200u8];
    let mut reader = decode::Reader::new(&bytes);
    // `SourceOrigin` is not `PartialEq`, so this matches rather than
    // comparing -- and names the variant it expects either way.
    match decode::decode_source_origin(&mut reader) {
        Err(decode::DecodeError::InvalidTag {
            what: "source origin",
            tag: 200,
        }) => {}
        Err(error) => panic!("expected InvalidTag{{source origin, 200}}, got: {error}"),
        Ok(SourceOrigin::File(path)) => panic!("expected an error, got origin {}", path.display()),
        Ok(SourceOrigin::Embedded(name)) => panic!("expected an error, got origin {name}"),
    }
}

#[test]
fn every_truncation_of_an_image_is_reported_rather_than_panicking() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
          public run = ( ^blk value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    for cut in 0..bytes.len() {
        // Never `{:?}` the `Ok` side: `LoadedImage` has no `Debug` impl at
        // all (its `Interpreter` field has none either).
        assert!(
            image::load(&bytes[..cut]).is_err(),
            "a truncated image must never load"
        );
    }
    image::load(&bytes).expect("the untruncated image still loads");
}

// ---------------------------------------------------------------------------
// `verify_code_table` -- decoding is byte-safe, but a decoded body
// is not yet *coherent*. Every case below decodes cleanly and would panic
// (or silently misaddress a register) inside the VM.
// ---------------------------------------------------------------------------

/// A `CompiledCode` the compiler would never emit: the verification tests
/// craft bodies directly rather than round-tripping a module.
fn code_with(
    kind: CodeKind,
    argument_count: u16,
    register_count: u16,
    instructions: Vec<Instruction>,
) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind,
        instructions,
        argument_count,
        register_count,
        statement_program_counter: vec![],
        // `verify_code_table` never looks at `family_size` -- numbering a
        // family is `number_call_site_families`'s job, and none of these
        // tests calls it -- so an arbitrary value is fine here. The same
        // holds for `outer_reach`, which only the VM reads.
        family_size: 0,
        copied_count: 0,
        outer_reach: 0,
        unwind_marker: None,
        source: Span::new(0, 0),
    })
}

/// `code_with`, plus a `copied_count` for the bodies that declare one.
fn code_with_copied(
    kind: CodeKind,
    argument_count: u16,
    register_count: u16,
    copied_count: u16,
    instructions: Vec<Instruction>,
) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        copied_count,
        ..(*code_with(kind, argument_count, register_count, instructions)).clone()
    })
}

fn table_of(codes: Vec<Rc<CompiledCode>>) -> decode::DecodedCodeTable {
    decode::DecodedCodeTable {
        // These tables only ever feed `verify_code_table`/`verify_graph`,
        // which read the code list alone -- the mixin table, which names
        // which body belongs to which declaration, is a separate section.
        codes,
    }
}

#[test]
fn a_real_modules_decoded_code_table_verifies() {
    // The control for every rejection below: everything the compiler emits
    // for a module using blocks, non-local return, loops and literal arrays
    // passes the structural pass unchanged.
    let (mut interpreter, class) = load(RICH);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (table, _decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        decode::verify_code_table(&table).expect("compiler-emitted code is structurally sound");
    });
}

#[test]
fn a_jump_target_past_the_end_of_its_body_is_refused() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![Instruction::Jump { target: 7 }],
    )]);
    let error = decode::verify_code_table(&table).expect_err("out-of-range jump");
    assert_eq!(
        error,
        decode::DecodeError::JumpTargetOutOfRange {
            target: 7,
            instructions: 1
        }
    );
    assert_eq!(
        error.to_string(),
        "jump target 7 is not an instruction index in a body of 1 instruction(s)"
    );
}

#[test]
fn every_conditional_jump_target_is_checked_too() {
    // One case per jump-carrying instruction: a check that only covered
    // `Jump` would leave three unchecked `program_counter` writes in the VM.
    for instruction in [
        Instruction::JumpIfNotBoolean {
            register: 0,
            target: 9,
        },
        Instruction::BranchIfFalse {
            register: 0,
            target: 9,
        },
        Instruction::BranchIfTrue {
            register: 0,
            target: 9,
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 0,
            left: 0,
            right: 0,
            target: 9,
        },
    ] {
        let table = table_of(vec![code_with(
            CodeKind::Method,
            0,
            1,
            vec![instruction, Instruction::Return { register: 0 }],
        )]);
        assert_eq!(
            decode::verify_code_table(&table),
            Err(decode::DecodeError::JumpTargetOutOfRange {
                target: 9,
                instructions: 2
            })
        );
    }
}

#[test]
fn a_register_past_the_bodys_register_file_is_refused() {
    // `read_local`/`write_local` split at `arguments.len()` and index
    // `temporaries` unchecked, so register 5 in a two-register body is an
    // out-of-bounds index, not a wrong answer.
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instruction::Move {
                destination: 5,
                source: 0,
            },
            Instruction::Return { register: 0 },
        ],
    )]);
    let error = decode::verify_code_table(&table).expect_err("out-of-range register");
    assert_eq!(
        error,
        decode::DecodeError::RegisterOutOfRange {
            register: 5,
            register_count: 2
        }
    );
    assert_eq!(
        error.to_string(),
        "register r5 is outside a body with 2 register(s)"
    );
}

#[test]
fn a_register_in_a_send_argument_list_is_checked_too() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::SendPublic {
                selector: Symbol::intern("foo:"),
                receiver_register: 0,
                argument_registers: vec![0, 4],
                destination: 0,
                call_site: CallSite { number: 0 },
            },
            Instruction::Return { register: 0 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 4,
            register_count: 1
        })
    );
}

#[test]
fn a_register_in_a_send_inlinable_is_checked_too() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::SendInlinable {
                selector: Symbol::intern("ifTrue:"),
                receiver_register: 0,
                argument_registers: vec![4],
                destination: 0,
                call_site: CallSite { number: 0 },
            },
            Instruction::Return { register: 0 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 4,
            register_count: 1
        })
    );
}

#[test]
fn a_body_that_runs_past_its_last_instruction_is_refused() {
    // The VM fetches `code.instructions[program_counter]` after every
    // fall-through, so a body whose last instruction is not a return or a jump
    // indexes one past the end. An empty body indexes past the end immediately.
    for instructions in [vec![], vec![Instruction::LoadSelf { destination: 0 }]] {
        let table = table_of(vec![code_with(CodeKind::Method, 0, 1, instructions)]);
        assert_eq!(
            decode::verify_code_table(&table),
            Err(decode::DecodeError::UnterminatedBody)
        );
    }
}

#[test]
fn more_arguments_than_registers_is_refused() {
    // Every activation is pushed with `register_count - argument_count`
    // temporaries; that `u16` subtraction underflows here.
    let table = table_of(vec![code_with(
        CodeKind::Method,
        3,
        1,
        vec![Instruction::Return { register: 0 }],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::ArgumentsExceedRegisters {
            argument_count: 3,
            register_count: 1
        })
    );
}

#[test]
fn a_make_closure_naming_a_method_body_is_refused() {
    // A closure activation is a `FrameKind::Closure` frame, and
    // `frame_at_depth` only hops through those -- so running a method body
    // as a block body would break the lexical-chain reasoning the outer
    // local check below rests on.
    let method = code_with(
        CodeKind::Method,
        0,
        1,
        vec![Instruction::Return { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: method.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = table_of(vec![method, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::WrongCodeKind { expected: "block" })
    );
}

#[test]
fn a_make_closure_whose_parameters_disagree_with_its_body_is_refused() {
    // `parameters` and the body's `argument_count` are separate wire fields,
    // and nothing at run time reads the names -- but `disassemble` prints
    // them, so a mismatch that decoded cleanly would be an image describing
    // itself wrongly with nothing to report it.
    let block = code_with(
        CodeKind::Block,
        1,
        2,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::ClosureArityMismatch {
            parameters: 0,
            argument_count: 1
        })
    );
}

#[test]
fn a_make_closure_whose_copied_list_does_not_match_its_block_is_refused() {
    // The other half of a block's register layout, beside its arity: the
    // instruction names the registers to read, the body says how many of
    // its own registers take them (`Heap::fill_copied_values`). A shorter
    // list leaves one of those holding nil and the block reading it as a
    // plain local, at exit status 0.
    let block = code_with_copied(
        CodeKind::Block,
        0,
        2,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::CopiedCountMismatch {
            copied: 0,
            copied_count: 1
        })
    );
}

/// A `MakeClosure`'s copied registers are read out of the *executing*
/// frame, like a send's arguments, so they are bounded against the body
/// that holds the instruction and not against the block's own file.
#[test]
fn a_make_closure_copying_a_register_its_own_body_lacks_is_refused() {
    let block = code_with_copied(
        CodeKind::Block,
        0,
        2,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: vec![4],
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 4,
            register_count: 1
        })
    );
}

#[test]
fn a_block_body_created_by_two_make_closures_is_refused() {
    // `lower_block_literal` wraps every block body in a fresh `Rc` used by
    // exactly one `MakeClosure`, which is what makes "the body one hop out"
    // well defined. A body with two creators has two different register
    // files an outer local could mean.
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let make = |destination| Instruction::MakeClosure {
        destination,
        parameters: Rc::new(vec![]),
        code: block.clone(),
        copied: Vec::new(),
    };
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![make(0), make(1), Instruction::Return { register: 0 }],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::SharedBlockBody)
    );
}

/// `inspect` calls `number_call_site_families` on its own -- a weaker
/// precondition than `verify_code_table`'s -- precisely so a file whose
/// `MakeClosure` relation is malformed but otherwise decodes still gets a
/// report instead of nothing (`image/inspect.rs`, the "code" section's
/// detail). That fallback only makes sense if a refused table is left
/// exactly as decoded, not partially numbered: `lexical_parents` runs to
/// completion before any send is touched, so a family that never starts
/// numbering never mutates anything.
#[test]
fn number_call_site_families_leaves_the_table_untouched_when_it_fails() {
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let make = |destination| Instruction::MakeClosure {
        destination,
        parameters: Rc::new(vec![]),
        code: block.clone(),
        copied: Vec::new(),
    };
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instruction::SendPublic {
                selector: Symbol::intern("foo"),
                receiver_register: 0,
                argument_registers: vec![],
                destination: 1,
                call_site: CallSite { number: 0 },
            },
            make(0),
            make(1),
            Instruction::Return { register: 0 },
        ],
    );
    let mut table = table_of(vec![block, parent]);
    let before = table.codes.clone();
    assert_eq!(
        decode::number_call_site_families(&mut table),
        Err(decode::DecodeError::SharedBlockBody)
    );
    assert_eq!(
        table.codes, before,
        "a table `lexical_parents` refuses must be left exactly as it was"
    );
}

#[test]
fn a_make_closure_naming_a_body_outside_the_table_is_refused() {
    // `decode_instruction` resolves a `MakeClosure`'s code id against the
    // table, but `DecodedCodeTable` is a plain public struct: an `Rc` from
    // somewhere else must not be trusted to have been verified.
    let stranger = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: stranger,
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = table_of(vec![parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::CodeNotInTable)
    );
}

/// `[block, parent]` as the encoder writes them (post-order), where
/// `parent` has `parent_registers` registers and `block` reads
/// `outer_index` from `outer_depth` frames out.
fn outer_local_table(
    parent_registers: u16,
    outer_depth: u16,
    outer_index: u16,
) -> decode::DecodedCodeTable {
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![
            Instruction::LoadOuterLocal {
                destination: 0,
                depth: outer_depth,
                index: outer_index,
            },
            Instruction::ImplicitReturn { register: 0 },
        ],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        parent_registers,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    table_of(vec![block, parent])
}

#[test]
fn an_outer_local_within_its_lexical_parents_register_file_verifies() {
    decode::verify_code_table(&outer_local_table(4, 1, 3)).expect("r3 of a four-register parent");
}

#[test]
fn an_outer_local_past_its_lexical_parents_register_file_is_refused() {
    // `LoadOuterLocal`'s index addresses the *outer* frame, so the body's
    // own `register_count` says nothing about it: the bound is the register
    // file of the body one `MakeClosure` hop out.
    let error = decode::verify_code_table(&outer_local_table(2, 1, 3)).expect_err("out of range");
    assert_eq!(
        error,
        decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 3,
            register_count: 2
        }
    );
    assert_eq!(
        error.to_string(),
        "outer local r3 at depth 1 is outside the enclosing body's 2 register(s)"
    );
}

#[test]
fn an_outer_local_at_depth_zero_is_bounded_by_its_own_register_file() {
    // `frame_at_depth(frame, 0)` is the frame itself.
    assert_eq!(
        decode::verify_code_table(&outer_local_table(4, 0, 3)),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 0,
            index: 3,
            register_count: 1
        })
    );
}

#[test]
fn an_outer_local_deeper_than_the_lexical_nesting_is_bounded_by_the_outermost_body() {
    // `frame_at_depth` stops hopping at a non-closure activation, so a
    // depth past the nesting lands on the outermost frame -- the method's,
    // with two registers here, not the block's own.
    assert_eq!(
        decode::verify_code_table(&outer_local_table(2, 9, 3)),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 9,
            index: 3,
            register_count: 2
        })
    );
}

#[test]
fn a_store_to_an_outer_local_is_checked_like_a_load() {
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![
            Instruction::StoreOuterLocal {
                source: 0,
                depth: 1,
                index: 6,
            },
            Instruction::ImplicitReturn { register: 0 },
        ],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    assert_eq!(
        decode::verify_code_table(&table_of(vec![block, parent])),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 6,
            register_count: 2
        })
    );
}

// ---------------------------------------------------------------------------
// `verify_mixins`'s arity and kind reconciliation.
// ---------------------------------------------------------------------------

#[test]
fn a_method_body_taking_the_wrong_number_of_arguments_is_refused_on_load() {
    // A method frame's `arguments` come from the send, so their count is the
    // selector's arity; the frame's temporaries are sized from the *body's*
    // `argument_count`. A body claiming a different arity leaves `read_local`
    // indexing `temporaries` past its end.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo: a = ( ^a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let body = CompiledCode {
            argument_count: 2,
            ..(*method_code(mutator.heap(), class, "foo:")).clone()
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "foo:", Rc::new(body));
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("arity mismatch");
        assert_eq!(
            error,
            decode::DecodeError::CodeArityMismatch {
                what: "method `foo:`".to_string(),
                parameters: 1,
                argument_count: 2
            }
        );
    });
}

#[test]
fn a_method_root_naming_a_block_body_is_refused_on_load() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let body = CompiledCode {
            kind: CodeKind::Block,
            ..(*method_code(mutator.heap(), class, "foo")).clone()
        };
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "foo", Rc::new(body));
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        assert_eq!(
            decode::verify_mixins(&decoded_mixins),
            Err(decode::DecodeError::WrongCodeKind { expected: "method" })
        );
    });
}

// ---------------------------------------------------------------------------
// `verify_graph` -- reconciling decoded frames and closures with
// the code that runs in them.
// ---------------------------------------------------------------------------

const ONE_METHOD: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
    ) (
      public foo = ( ^1 )
    ) : ()
"};

/// A world whose code table is `[block, foo]` -- `foo`'s body is the one
/// `MakeClosure` that creates `block` -- with the class's `foo` carrying
/// that body. Everything a decoded closure over `block`
/// needs to verify: its lexical parent is `foo`, which has two registers.
fn closure_world() -> (GraphFixture, decode::DecodedCodeTable) {
    let (mut interpreter, class) = load(ONE_METHOD);
    let platform = interpreter.platform();
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ],
    );
    let table = decode::DecodedCodeTable {
        codes: vec![block, parent.clone()],
    };
    let mixins = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "foo", parent.clone());
        install_mixin(mutator, class, doctored);
        mixin_preorder(mutator.heap(), class)
    });
    let fixture = GraphFixture {
        codes: table.codes.clone(),
        interpreter,
        class,
        platform,
        // `foo_frame` below writes the `Method` frame kind's selector as id
        // 0, which must resolve to "foo" -- the method `install_mixin` just
        // gave the compiled body to.
        symbols: table_with(&["foo"]),
        mixins,
        platform_paths: Vec::new(),
    };
    (fixture, table)
}

/// A graph section holding one closure object over code id 0 and the one
/// frame it names as its `captured_frame`, written by `frame`. The home
/// follows from the captured frame, so the record carries no second id.
fn closure_over_frame(frame: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 1);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(0); // code id 0 -- the block body
    writer.write_u32(0); // captured frame id
    writer.write_u16(0); // copying nothing
    frame(&mut writer);
    writer.write_u32(1); // one root...
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(0); // ...the closure
    writer.into_bytes()
}

/// Frame 0 as an activation of method `foo` with `temporaries` temporaries.
fn foo_frame(temporaries: u32) -> impl FnOnce(&mut encode::Writer) {
    foo_frame_at_program_counter(temporaries, 0)
}

/// `foo_frame` with the activation's `program_counter` left to the caller.
fn foo_frame_at_program_counter(
    temporaries: u32,
    program_counter: u32,
) -> impl FnOnce(&mut encode::Writer) {
    move |writer: &mut encode::Writer| {
        writer.write_u8(VALUE_NIL); // receiver: nil
        writer.write_u32(0); // no arguments
        writer.write_u32(temporaries);
        for _ in 0..temporaries {
            writer.write_u8(VALUE_NIL); // each temporary: nil
        }
        writer.write_u32(program_counter);
        writer.write_boolean(false); // no sender
        writer.write_boolean(false); // no home
        writer.write_boolean(false); // no current class
        writer.write_u8(FRAME_METHOD); // frame kind: Method
        writer.write_u32(0); // selector id 0 -- "foo", per `closure_world`'s fixture
        writer.write_boolean(false); // continuable
    }
}

#[test]
fn a_closure_captured_by_its_lexical_parents_activation_verifies() {
    // The control for the two rejections below: a decoded closure whose
    // captured frame really is an activation of the body its own body is
    // nested in, sized exactly as that body prescribes.
    let (mut fixture, table) = closure_world();
    fixture.decoded(
        &closure_over_frame(foo_frame(2)),
        "decode",
        |heap, graph| {
            decode::verify_graph(graph, &table, heap).expect("the captured frame runs `foo`");
        },
    );
}

/// A returning activation nothing reads by reference keeps no registers at
/// all (`Heap::pop_activation`), so the frame it leaves behind is empty --
/// and that is exactly what a snapshot of it records. An empty file is
/// legitimate; every other shape is still held to the body's.
#[test]
fn a_frame_with_no_registers_at_all_verifies() {
    let (mut fixture, table) = closure_world();
    fixture.decoded(
        &closure_over_frame(foo_frame(0)),
        "decode",
        |heap, graph| {
            decode::verify_graph(graph, &table, heap)
                .expect("a frame that kept no registers is what an unmarked return leaves");
        },
    );
}

#[test]
fn a_frame_whose_register_file_is_too_small_for_its_code_is_refused() {
    // `foo`'s body has two registers; this activation has one. A
    // `LoadOuterLocal` of r1 from the closure would index `temporaries[1]` of a
    // one-element `temporaries` -- an unchecked out-of-bounds index in
    // `locals::read_local`, reached from a version-matching image.
    let (mut fixture, table) = closure_world();
    fixture.decoded(&closure_over_frame(foo_frame(1)), "decode", |heap, graph| {
        let error = decode::verify_graph(graph, &table, heap).expect_err("undersized frame");
        assert_eq!(
            error,
            decode::DecodeError::FrameRegisterFile {
                arguments: 0,
                locals: 1,
                argument_count: 0,
                register_count: 2
            }
        );
        assert_eq!(
            error.to_string(),
            "a decoded frame holds 0 argument(s) and 1 local(s), but the code that runs in it takes 0 argument(s) in 2 register(s)"
        );
    });
}

#[test]
fn a_closure_captured_by_a_frame_that_does_not_run_its_lexical_parent_is_refused() {
    // The frame is well-formed on its own; what is crafted is the *link*.
    // Nothing else in the format ties a closure's captured frame to its
    // body's lexical parent, and the whole outer-local bound rests on it.
    let (mut fixture, table) = closure_world();
    let bytes = closure_over_frame(|writer: &mut encode::Writer| {
        writer.write_u8(VALUE_NIL); // receiver: nil
        writer.write_u32(0); // no arguments
        writer.write_u32(0); // no temporaries
        writer.write_u32(0); // program_counter
        writer.write_boolean(false); // no sender
        writer.write_boolean(false); // no home
        writer.write_boolean(false); // no current class
        writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel -- runs no code at all
        writer.write_boolean(false); // continuable
    });
    fixture.decoded(&bytes, "decode", |heap, graph| {
        assert_eq!(
            decode::verify_graph(graph, &table, heap),
            Err(decode::DecodeError::CapturedFrameMismatch)
        );
    });
}

#[test]
fn a_real_snapshots_graph_verifies_and_its_closures_still_read_their_captured_locals() {
    // The end-to-end control: a quiesced world holding a closure over a
    // method activation, through the real sections and back.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | held ::= nil. |
        ) (
          public keep = ( | t | t:: 7. held:: [ t ]. ^held )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("keep"), &[]),
        "keep",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let held = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("keep"), &[]),
        "keep",
    );
    let value = expect_ok(
        loaded.interpreter.send(&held, Symbol::intern("value"), &[]),
        "value",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&value), Value::SmallInteger(7)),
            "the loaded closure reads its captured local"
        );
    });
}

#[test]
fn a_decoded_symbol_round_trips_to_the_loaded_worlds_canonical_object() {
    // A loaded world's code literals reach `#foo` through `Mutator::new_symbol`
    // too, so a symbol decoded out of the graph section has to land in the
    // same table -- not a second object that merely spells the same text --
    // or the two would answer `false` to `==` and `identityHash` where a
    // heap with one canonical symbol per text never could.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | storedSymbol ::= nil. |
        ) (
          public keep = ( storedSymbol:: #foo. ^storedSymbol )
          public held = ( ^storedSymbol )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("keep"), &[]),
        "keep",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let held = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("held"), &[]),
        "held",
    );
    loaded.interpreter.enter(|mutator| {
        let held = mutator.fetch(&held);
        let expected = mutator.new_symbol(Symbol::intern("foo"));
        assert!(
            object(&held).same(*object(&expected)),
            "the decoded symbol is the loaded world's canonical `#foo`"
        );
    });
}

#[test]
fn a_snapshot_whose_closure_was_captured_by_a_block_activation_loads_and_runs() {
    // The chain case: `[:x | [ x ] ] value: 5` leaves the inner closure
    // captured by a *block* activation, so `verify_graph` has to follow one
    // closure record's captured frame into another's, and the inner body's
    // `LoadOuterLocal` really does hop two frames at run time. Sending
    // `value` after the load is what proves the reconstructed chain reads
    // the right register.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | held ::= nil. |
        ) (
          public keep = ( held:: [:x | [ x ] ] value: 5. ^held )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("keep"), &[]),
        "keep",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let held = loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        let held = mutator
            .heap()
            .instance_field(*object(&root_0), 0)
            .expect("expected an instance");
        let captured = mutator
            .heap()
            .closure_data(*object(&held))
            .expect("the slot holds the inner closure")
            .captured_frame;
        assert!(
            matches!(mutator.heap().frame_kind(captured), FrameKind::Closure(_)),
            "the inner closure was captured by the outer block's activation"
        );
        // Kept across the `value` send below.
        mutator.stash(held)
    });
    let value = expect_ok(
        loaded.interpreter.send(&held, Symbol::intern("value"), &[]),
        "value",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&value), Value::SmallInteger(5)),
            "the loaded closure reads `x` two frames out"
        );
    });
}

// ---------------------------------------------------------------------------
// Two crafted-image paths that reach a panic through `instantiate` rather
// than through the verification passes.
// ---------------------------------------------------------------------------

/// A root class with one slot, so `class_has_initializer_code` is true and
/// `instantiate` *must* find compiled initializer code for it.
const NEEDS_INITIALIZER: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
      | count ::= 1. |
    ) (
      public n = ( ^count )
    ) : ()
"};

#[test]
fn an_image_with_no_initializer_for_a_class_that_needs_one_is_refused_on_load() {
    // `Mutator::initializer_body` panics -- "internal: class needs an instance
    // initializer and its mixin carries none" -- when the class has
    // initializer code and its mixin has none. A mixin-table row's
    // declaration half decides the first (it names slots or `initExpr`s)
    // and its code half the second, so an image whose initializer id is
    // cleared would make the *first* `X new` after a load abort the process.
    let (mut interpreter, class) = load(NEEDS_INITIALIZER);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        assert!(
            mixin_of(mutator.heap(), class).initializer_code.is_some(),
            "the fixture's class really does compile an initializer"
        );
        let doctored = Mixin {
            initializer_code: None,
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        assert!(
            decoded_mixins.code[0].initializer.is_none(),
            "the doctored row names no initializer"
        );
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no initializer");
        assert_eq!(error, decode::DecodeError::MissingInitializer);
        assert_eq!(
            error.to_string(),
            "a class in the image needs an instance initializer, which the image has no compiled code for"
        );
    });
}

#[test]
fn a_whole_image_missing_its_initializer_is_refused_by_load() {
    // The same crafting one level up, through the real sections: a class
    // whose mixin carries no initializer encodes an image with the
    // initializer flag cleared, and every other check passes -- a quiesced
    // snapshot has no initializer frame for `verify_graph` to catch.
    let (mut interpreter, class) = load(NEEDS_INITIALIZER);
    // Instantiated before the doctoring: `install_mixin` swaps the class
    // object's own application, so this instance is already an instance of
    // the doctored class -- and instantiating the doctored class would run
    // into the very invariant this test crafts an image to violate.
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // Only the initializer is dropped, so the refusal this asserts
        // cannot be the superclass clause's.
        let doctored = Mixin {
            initializer_code: None,
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(NEEDS_INITIALIZER),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let error = load_error(&bytes);
    assert!(
        error.contains("instance initializer"),
        "load must refuse it rather than let `X new` panic later: {error}"
    );
}

#[test]
fn an_identity_hash_at_the_ceiling_is_refused_rather_than_overflowing_the_counter() {
    // `load` hands the maximum decoded hash to `advance_identity_hash_past`,
    // which computes `max + 1`: an overflow panic under this workspace's dev
    // profile, and a wrap to 0 -- so a collision with every loaded object --
    // in release.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::String));
    writer.write_u32(u32::MAX); // identity hash, at the ceiling
    writer.write_u8(OBJECT_STRING); // object body: String
    writer.write_str("x");
    writer.write_u32(0); // no roots -- otherwise complete, so the hash is what
                         // the decoder rejects, not a truncation
    let bytes = writer.into_bytes();
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::IdentityHashExhausted);
    assert_eq!(
        error.to_string(),
        "an image object's identity hash is 4294967295, leaving no hash for a loaded world to allocate next"
    );
}

#[test]
fn an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating() {
    // The step the ceiling check alone does not cover: a maximum of
    // `u32::MAX - 1` is a legal image, and it leaves the counter at
    // `u32::MAX` -- where the counter's own `+= 1` would overflow on the
    // very first hash assigned after the load. Saturating
    // there costs a shared hash (identity is `Rc` identity, never the
    // hash) instead of aborting the process.
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance_object = *object(&mutator.fetch(&instance));
        mutator
            .heap_mut()
            .set_identity_hash(instance_object, u32::MAX - 1);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(
            mutator.heap().identity_hash(*object(&root_0)),
            u32::MAX - 1,
            "the image's hash is preserved, not clamped"
        );
        let first = mutator.new_string("a".to_string());
        let second = mutator.new_string("b".to_string());
        let heap = mutator.heap_mut();
        assert_eq!(heap.ensure_identity_hash(first), u32::MAX);
        assert_eq!(
            heap.ensure_identity_hash(second),
            u32::MAX,
            "an exhausted counter shares its last hash instead of overflowing"
        );
    });
}

/// An object nothing asked for its hash is written as 0 and decoded as 0 --
/// still unassigned -- and the first request after the load draws a number
/// past every hash the image carried, so it cannot collide with one.
#[test]
fn an_unhashed_object_round_trips_unassigned_and_draws_past_the_image_maximum() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let hashed = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let unhashed = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let (bytes, assigned) = interpreter.enter(|mutator| {
        let hashed_value = mutator.fetch(&hashed);
        let assigned = mutator
            .heap_mut()
            .ensure_identity_hash(*object(&hashed_value));
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let unhashed_value = mutator.fetch(&unhashed);
        let bytes = image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[hashed_value, unhashed_value],
        )
        .expect("snapshot");
        assert_eq!(
            mutator.heap().identity_hash(*object(&unhashed_value)),
            0,
            "snapshotting assigns no hash"
        );
        (bytes, assigned)
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let (kept, fresh) = (
            *object(&mutator.fetch(&loaded.roots[0])),
            *object(&mutator.fetch(&loaded.roots[1])),
        );
        let heap = mutator.heap_mut();
        assert_eq!(
            heap.identity_hash(kept),
            assigned,
            "an assigned hash is preserved"
        );
        assert_eq!(
            heap.identity_hash(fresh),
            0,
            "an unassigned hash stays unassigned"
        );
        assert!(heap.ensure_identity_hash(fresh) > assigned);
    });
}

/// A builtin class, the root class and `platform` are named by tag, not
/// carried as records, so before the closing hash table nothing recorded a
/// hash one of them had been given: after the load the bootstrap's fresh
/// objects drew new ones (`docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim"). One that nothing asked stays unassigned.
#[test]
fn the_hashes_of_objects_named_by_tag_survive_a_snapshot() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    // Nothing runs in either interpreter, so what is read out of a root
    // stays current.
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let (bytes, integer_hash, class_hash, platform_hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let integer_hash = mutator.heap_mut().ensure_identity_hash(integer);
        let class_hash = mutator.heap_mut().ensure_identity_hash(class);
        let platform_hash = mutator.heap_mut().ensure_identity_hash(*object(&platform));
        let bytes = image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .expect("snapshot");
        (bytes, integer_hash, class_hash, platform_hash)
    });

    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.graph_tag_named_hashes,
        vec![
            format!("Integer hash {integer_hash}"),
            format!("<root class> hash {class_hash}"),
            format!("<platform> hash {platform_hash}"),
        ],
        "inspect reads the table, in the encoder's fixed order"
    );

    let mut loaded = image::load(&bytes).expect("load");
    let loaded_platform = loaded.interpreter.platform();
    loaded.interpreter.enter(|mutator| {
        let loaded_platform = mutator.fetch(&loaded_platform);
        let loaded_class = class_reference(mutator, &loaded.root_class);
        let loaded_integer = mutator.builtin_class(BuiltinClass::Integer);
        let loaded_string = mutator.builtin_class(BuiltinClass::String);
        let heap = mutator.heap_mut();
        assert_eq!(heap.identity_hash(loaded_integer), integer_hash);
        assert_eq!(heap.identity_hash(loaded_class), class_hash);
        assert_eq!(heap.identity_hash(*object(&loaded_platform)), platform_hash);
        assert_eq!(
            heap.identity_hash(loaded_string),
            0,
            "a builtin nothing asked for its hash stays unassigned"
        );
        let largest = integer_hash.max(class_hash).max(platform_hash);
        assert!(
            heap.ensure_identity_hash(loaded_string) > largest,
            "the counter is advanced past the tag-named hashes too"
        );
    });
}

/// The closing table alone, decoded against a freshly loaded world.
fn decode_tag_named(bytes: &[u8]) -> Result<u32, decode::DecodeError> {
    let (mut interpreter, class) = load(EMPTY);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let mut reader = decode::Reader::new(bytes);
        decode::decode_tag_named_hashes(&mut reader, mutator, class, &[])
    })
}

#[test]
fn a_tag_named_hash_of_zero_is_refused() {
    // 0 means unassigned, which the encoder expresses by leaving the entry
    // out; accepting it would give one world two encodings.
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    writer.write_u32(0);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::UnassignedTagNamedHash)
    );
}

#[test]
fn a_tag_named_object_recorded_twice_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    writer.write_u8(image::opcodes::HASHED_PLATFORM);
    writer.write_u32(3);
    writer.write_u8(image::opcodes::HASHED_PLATFORM);
    writer.write_u32(4);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::DuplicateTagNamedHash)
    );
}

#[test]
fn a_tag_named_hash_at_the_ceiling_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Integer));
    writer.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::IdentityHashExhausted)
    );
}

#[test]
fn an_unknown_tag_named_hash_tag_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(HASHED_PLATFORM_PATH + 1); // one past the last tag-named hash
    writer.write_u32(5);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::InvalidTag {
            what: "tag-named hash",
            tag: HASHED_PLATFORM_PATH + 1
        })
    );
}

#[test]
fn a_tag_named_hash_count_past_the_remaining_bytes_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
}

#[test]
fn the_tag_named_hash_table_answers_its_largest_hash() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    writer.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::String));
    writer.write_u32(9);
    writer.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    writer.write_u32(4);
    assert_eq!(decode_tag_named(&writer.into_bytes()), Ok(9));
}

// ---------------------------------------------------------------------------
// Loading compiles the whole mixin tree, so the code table must serialize
// every mixin's methods and initializer -- not only the root's -- and
// decoding must attach each of them to the declaration it came from.
// ---------------------------------------------------------------------------

const NESTED_MODULE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Inner = Object new (| v = 9. |) (
            public value = ( ^v )
        )
    ) : ()
"};

/// A class object applying `mixin` to `superclass` directly -- what the
/// (not yet implemented) nested-class accessor will eventually do for
/// `Inner`. Built by hand because there is no other way yet to reach a
/// nested mixin's own class object.
fn apply_and_wrap<'gc>(
    mutator: &mut Mutator<'gc>,
    class_of_class: ClassReference<'gc>,
    mixin: &Rc<Mixin>,
    superclass: Option<&Rc<MixinApplication>>,
) -> ClassReference<'gc> {
    let application = mutator.apply_mixin(mixin, superclass);
    mutator.heap_mut().allocate_object(
        class_of_class,
        ObjectData::Class(Box::new(ClassData::new(
            application,
            None,
            None,
            None,
            None,
        ))),
    )
}

fn root_mixin_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Rc<Mixin> {
    heap.class_application(class)
        .expect("not a class")
        .mixin
        .clone()
}

#[test]
fn a_snapshotted_nested_mixins_method_and_initializer_survive_a_load() {
    // `encode_code_table` must write more than the root's own section:
    // writing only the root lets a `snapshot` -> `load` cycle silently
    // narrow the tree loading compiled.
    // Checked here purely at the declaration level -- reaching the nested
    // method with an actual send is the next test.
    let (mut interpreter, class) = load(NESTED_MODULE);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);

        // The *decoded* nested mixin, not the live one: its bodies are the
        // ones the mixin table attached, which is what a loaded world's
        // class objects apply.
        let inner_mixin = &decoded_mixins.module_root(0).expect("a root row").nested[0];
        let value_method = inner_mixin
            .compiled_method(Symbol::intern("value"))
            .expect("Inner declares `value`");
        let live_inner = &root_mixin_of(mutator.heap(), class).nested[0];
        assert_eq!(
            *value_method.code,
            *live_inner
                .compiled_method(Symbol::intern("value"))
                .expect("Inner declares `value`")
                .code,
            "the nested mixin's own method must survive the round trip"
        );
        assert!(
            inner_mixin.initializer_code.is_some(),
            "the nested mixin's own initializer must survive the round trip"
        );
    });
}

#[test]
fn a_send_reaches_a_nested_mixins_method_after_a_snapshot_and_load() {
    // The end-to-end version: snapshot a world whose module covers the
    // whole mixin tree, load it back into a *fresh* interpreter (a second,
    // independent parse+resolve+load_module of the same source, exactly
    // like every other round-trip test in this file), then reach the
    // nested class's own compiled method with a real send -- proving
    // decoding attached it to the right declaration, not just that some
    // body with the right shape exists.
    let (mut interpreter, class) = load(NESTED_MODULE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Outer",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let outer = mutator.fetch(&outer);
        image::snapshot(
            mutator,
            Some(NESTED_MODULE),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[outer],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let inner_class = loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        let root_mixin = root_mixin_of(mutator.heap(), root_class);
        let inner_mixin = root_mixin.nested[0].clone();

        // Nothing materializes a nested class's own class object without the
        // accessor, which does not exist yet, so this reproduces what it will
        // eventually do: apply the decoded `Inner` mixin to `Object`, by hand,
        // in the *loaded* interpreter -- proving the send below runs against
        // code the decoded mixin actually carries, not code compiled fresh
        // just now.
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let object_application = mutator
            .heap()
            .class_application(object_class)
            .expect("Object is a class object");
        let inner_class = apply_and_wrap(
            mutator,
            object_class,
            &inner_mixin,
            Some(&object_application),
        );
        mutator.stash(Value::Object(inner_class))
    });
    let inner_instance = expect_ok(
        loaded.interpreter.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let result = expect_ok(
        loaded
            .interpreter
            .send(&inner_instance, Symbol::intern("value"), &[]),
        "send value",
    );
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&result) {
            Value::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

#[test]
fn the_nested_class_accessor_runs_a_reloaded_superclass_clause() {
    // The accessor evaluates the nested declaration's superclass clause,
    // and the only thing it can evaluate is the compiled clause body the
    // code table carries. A reloaded image whose sections dropped it would
    // panic on the first `Inner` access here -- which is why a mixin-table
    // row carries the clause alongside the initializer, and `verify_mixins`
    // refuses an image missing one.
    let (mut interpreter, class) = load(NESTED_MODULE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Outer",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let outer = mutator.fetch(&outer);
        image::snapshot(
            mutator,
            Some(NESTED_MODULE),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[outer],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_outer = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Outer");
    let inner_class = expect_ok(
        loaded
            .interpreter
            .send(&reloaded_outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    loaded.interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&inner_class) else {
            panic!("the accessor must yield a class object")
        };
    });
    let inner_instance = expect_ok(
        loaded.interpreter.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let result = expect_ok(
        loaded
            .interpreter
            .send(&inner_instance, Symbol::intern("value"), &[]),
        "send value",
    );
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&result) {
            Value::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

/// A world built from `source` with one instance of its root class -- the
/// starting point every whole-image nesting test shares. Returns the
/// interpreter, the root class, the `platform` singleton and the instance.
fn instantiated_world(source: &str) -> (Interpreter, Root, Root, Root) {
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let root = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate the root class",
    );
    (interpreter, class, platform, root)
}

/// `image::snapshot` of a world a test keeps as roots, each read out right
/// before the encoder, which runs nothing, all inside one entry.
fn snapshot_world(
    interpreter: &mut Interpreter,
    source: &str,
    class: &Root,
    platform: &Root,
    roots: &[Root],
) -> Vec<u8> {
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, class);
        let platform_value = mutator.fetch(platform);
        let roots: Vec<Value> = roots.iter().map(|root| mutator.fetch(root)).collect();
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform_value,
            &roots,
        )
        .expect("snapshot")
    })
}

/// The `MixinApplication` behind a class object -- the behaviour half two
/// class objects built from one mixin and one superclass must *share*.
fn application_of<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    class: ClassReference<'gc>,
) -> Rc<MixinApplication> {
    heap.class_application(class).expect("a class object")
}

fn class_name<'gc>(heap: &ns_runtime::Heap<'gc>, class: ClassReference<'gc>) -> String {
    application_of(heap, class).mixin.name.as_str().to_string()
}

fn class_of<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> ClassReference<'gc> {
    heap.class_of_object(*object(value))
}

/// The value sitting in `enclosing`'s hidden cache slot for nested
/// declaration `name` -- §6's "one class object per enclosing instance",
/// read straight out of the instance rather than through the accessor.
///
/// Resolved the way the accessor resolves it: the name picks a
/// *declaration* off the class chain, and the declaration is what indexes
/// the slot map -- a subclass's `class N` and the inherited one it shadows
/// have different slots.
fn cached_nested_class<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    enclosing: &Value<'gc>,
    name: &str,
) -> Value<'gc> {
    let class = class_of(heap, enclosing);
    let ns_runtime::dispatch::Lookup::NestedClass(nested, _) =
        ns_runtime::dispatch::lookup_defined_for(heap, class, Symbol::intern(name))
    else {
        panic!("{name} is a nested declaration of this object's class");
    };
    let application = application_of(heap, class);
    let index = application.nested_slot[&ns_runtime::mixin::mixin_key(&nested)];
    heap.instance_field(*object(enclosing), index)
        .expect("an enclosing object is an instance")
}

fn send_to(loaded: &mut image::LoadedImage, receiver: &Root, selector: &str) -> Root {
    expect_ok(
        loaded
            .interpreter
            .send(receiver, Symbol::intern(selector), &[]),
        selector,
    )
}

/// The class object the accessor for `selector` yields on `receiver`.
fn nested_class_of(loaded: &mut image::LoadedImage, receiver: &Root, selector: &str) -> Root {
    let class = send_to(loaded, receiver, selector);
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&class) {
            Value::Object(_) => {}
            _ => panic!("the accessor for {selector} must yield a class object"),
        });
    class
}

#[test]
fn a_snapshot_after_a_nested_class_access_round_trips() {
    // The accessor puts a *second* user-defined class object into the world:
    // it is the value of the enclosing instance's hidden cache slot, and the
    // `.class` of every instance of it. Both must round-trip: a class tag
    // that named no particular user class would decode one back as the
    // root. A class object is an ordinary record in the object table, which
    // is what makes both work.
    let (mut interpreter, class, platform, outer) = instantiated_world(NESTED_MODULE);
    let inner_class = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&inner_class) else {
            panic!("the accessor must yield a class object")
        };
    });
    let inner_instance = expect_ok(
        interpreter.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );

    // Reached as the cached value in Outer's hidden slot...
    let bytes = snapshot_world(
        &mut interpreter,
        NESTED_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&outer),
    );
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_outer = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Outer");
    let reloaded_inner = nested_class_of(&mut loaded, &reloaded_outer, "Inner");
    loaded.interpreter.enter(|mutator| {
        let reloaded_inner = class_reference(mutator, &reloaded_inner);
        assert_eq!(class_name(mutator.heap(), reloaded_inner), "Inner");
    });
    let instance = expect_ok(
        loaded.interpreter.instantiate(&reloaded_inner, &[]),
        "instantiate Inner",
    );
    let value = send_to(&mut loaded, &instance, "value");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value) {
            Value::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });

    // ... and as the `.class` of one of its instances.
    let bytes = snapshot_world(
        &mut interpreter,
        NESTED_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&inner_instance),
    );
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_instance = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Inner instance");
    loaded.interpreter.enter(|mutator| {
        let reloaded_instance_value = mutator.fetch(&reloaded_instance);
        let reloaded_class = class_of(mutator.heap(), &reloaded_instance_value);
        assert_eq!(class_name(mutator.heap(), reloaded_class), "Inner");
    });
    let value = send_to(&mut loaded, &reloaded_instance, "value");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value) {
            Value::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

#[test]
fn a_second_user_class_no_longer_decodes_as_the_root() {
    // A class tag that named no particular class would bring a second
    // user-defined class object back as the root: a wrong class silently
    // installed, with no field count to contradict it, rather than a decode
    // error.
    let (mut interpreter, class, platform, outer) = instantiated_world(NESTED_MODULE);
    let _ = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    let bytes = snapshot_world(
        &mut interpreter,
        NESTED_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&outer),
    );
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_outer = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Outer");
    let inner = nested_class_of(&mut loaded, &reloaded_outer, "Inner");
    // Both read out after the accessor ran, which may have moved them.
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        let inner = class_reference(mutator, &inner);
        assert_ne!(
            class_name(mutator.heap(), inner),
            class_name(mutator.heap(), root_class)
        );
        assert!(
            !inner.same(root_class),
            "the nested class must not decode as the module's root class"
        );
    });
}

#[test]
fn one_application_shared_before_a_snapshot_is_still_shared_after_it() {
    // Two enclosing instances, one shared application before the snapshot.
    // Identity is per enclosing instance (§6) but behaviour is per mixin
    // application (§6.2), and only the second half survives a save -- the
    // applications are rebuilt through `Mutator::apply_mixin`, whose
    // cache key is content-derived. If the rebuild lost the sharing,
    // the inline caches' call sites would go megamorphic after every load, which
    // is why this is a property test rather than a shape test.
    let (mut interpreter, class, platform, first) = instantiated_world(NESTED_MODULE);
    let second = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "a second Outer",
    );
    let inner_of = |interpreter: &mut Interpreter, outer: &Root| {
        let class = expect_ok(
            interpreter.send(outer, Symbol::intern("Inner"), &[]),
            "Inner",
        );
        interpreter.enter(|mutator| match mutator.fetch(&class) {
            Value::Object(_) => {}
            _ => panic!("the accessor must yield a class object"),
        });
        class
    };
    let first_inner_before = inner_of(&mut interpreter, &first);
    let second_inner_before = inner_of(&mut interpreter, &second);
    // Both read out after the last accessor ran.
    interpreter.enter(|mutator| {
        let first_inner_before = class_reference(mutator, &first_inner_before);
        let second_inner_before = class_reference(mutator, &second_inner_before);
        assert!(
            !first_inner_before.same(second_inner_before),
            "one class object per enclosing instance, before the snapshot"
        );
        assert!(
            Rc::ptr_eq(
                &application_of(mutator.heap(), first_inner_before),
                &application_of(mutator.heap(), second_inner_before)
            ),
            "one shared application, before the snapshot"
        );
    });

    let bytes = snapshot_world(
        &mut interpreter,
        NESTED_MODULE,
        &class,
        &platform,
        &[first, second],
    );
    let mut loaded = image::load(&bytes).expect("load");
    let outers: Vec<Root> = loaded.roots.clone();
    let first_inner = nested_class_of(&mut loaded, &outers[0], "Inner");
    let second_inner = nested_class_of(&mut loaded, &outers[1], "Inner");
    // Everything below is read out after the last accessor ran.
    loaded.interpreter.enter(|mutator| {
        let first_inner = class_reference(mutator, &first_inner);
        let second_inner = class_reference(mutator, &second_inner);
        let outer_values: Vec<Value> = outers.iter().map(|outer| mutator.fetch(outer)).collect();
        // The accessor answers out of the hidden cache slot, so `first_inner`
        // and `second_inner` are the *decoded* class objects and not two
        // freshly materialized ones. Pinned rather than assumed: a load that
        // dropped the cached slots would still satisfy every assertion
        // below.
        assert!(
            first_inner.same(*object(&cached_nested_class(
                mutator.heap(),
                &outer_values[0],
                "Inner"
            ))),
            "the accessor answered out of the decoded cache slot"
        );
        assert!(
            second_inner.same(*object(&cached_nested_class(
                mutator.heap(),
                &outer_values[1],
                "Inner"
            ))),
            "the accessor answered out of the decoded cache slot"
        );
        assert!(!first_inner.same(second_inner), "distinct class objects");
        assert!(
            Rc::ptr_eq(
                &application_of(mutator.heap(), first_inner),
                &application_of(mutator.heap(), second_inner)
            ),
            "one shared application"
        );
    });
}

/// `dispatch::kth_enclosing_object` steps `ClassData::declaring_class`, not
/// the reached object's own class, from its second hop on -- so an image
/// that dropped that field would come back resolving `outer` along the
/// receiver's *dynamic* chain. `Mid`'s decoy `label` is what such a walk
/// answers with, and a walk that finds no declaring class at all answers
/// `nil label`, an error. Both are wrong output rather than a refused
/// image, which is why the field has to travel.
const DEEP_OUTER_MODULE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        label = ( ^'top' )
        public class Base = Object new () (
            public class Deep = Object new () (
                public describe = ( ^outer Outer label )
            )
        )
        public class Mid = Object new () (
            label = ( ^'mid' )
            public class Derived = Base new () ()
        )
        public makeDeep = ( ^Mid new Derived new Deep new )
    ) : ()
"};

#[test]
fn outer_still_resolves_through_a_declaration_chain_that_came_from_an_image() {
    let (mut interpreter, class, platform, outer) = instantiated_world(DEEP_OUTER_MODULE);
    let deep = expect_ok(
        interpreter.send(&outer, Symbol::intern("makeDeep"), &[]),
        "makeDeep",
    );
    let bytes = snapshot_world(
        &mut interpreter,
        DEEP_OUTER_MODULE,
        &class,
        &platform,
        &[outer, deep],
    );
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_deep = loaded.roots[1].clone();
    // `Deep`'s declaring class is `Base`, not `Derived` -- the two differ
    // exactly because `Derived` inherits the declaration across a nesting
    // boundary, which is the case a dynamic-chain walk gets wrong.
    loaded.interpreter.enter(|mutator| {
        let reloaded_deep_value = mutator.fetch(&reloaded_deep);
        let reloaded_class = class_of(mutator.heap(), &reloaded_deep_value);
        assert_eq!(
            class_name(mutator.heap(), reloaded_class),
            "Deep",
            "the reloaded instance keeps its own nested class"
        );
    });
    let described = send_to(&mut loaded, &reloaded_deep, "describe");
    loaded.interpreter.enter(|mutator| {
        let described = mutator.fetch(&described);
        assert_eq!(string_contents(mutator.heap(), &described), "top");
    });
}

/// The text of a decoded `String` object. Never `{:?}`s the `Value`.
fn string_contents<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> String {
    heap.str_value(*object(value))
        .expect("expected a string object")
}

const NESTED_SUPERCLASS_ARGUMENTS: &str = indoc! {"
    Newspeak3
    'Test'
    class Args usingPlatform: p = Object new () (
        public class Base withBase: b = Object new (| v ::= b. |) (
            public value = ( ^v )
        )
        public class Derived = Base withBase: 7 () (
        )
    ) : ()
"};

/// `newspeak-spec.pdf` §6.2 evaluates the superclass clause's message on
/// every instantiation, so a reloaded image is the only place its compiled
/// body can come from -- exactly the reason the clause's *receiver* already
/// travels in a mixin-table row. A round trip that still hands `Base` its `7`
/// proves the message body made it across.
#[test]
fn a_reloaded_superclass_clause_message_still_passes_its_arguments() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let outer = mutator.fetch(&outer);
        image::snapshot(
            mutator,
            Some(NESTED_SUPERCLASS_ARGUMENTS),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[outer],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded = loaded.roots.first().cloned().expect("the snapshotted Args");
    let derived = expect_ok(
        loaded
            .interpreter
            .send(&reloaded, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    loaded.interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&derived) else {
            panic!("the accessor must yield a class object")
        };
    });
    let instance = expect_ok(
        loaded.interpreter.instantiate(&derived, &[]),
        "instantiate Derived",
    );
    let result = expect_ok(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("value"), &[]),
        "send value",
    );
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&result) {
            Value::SmallInteger(7) => {}
            _ => {
                panic!("expected SmallInteger(7) -- the clause's own argument -- from Base's slot")
            }
        });
}

/// The other direction, the same shape as the initializer's and the
/// clause's: a mixin-table row whose clause passes arguments, in an image
/// carrying no body for them, would panic on the first instantiation.
/// `verify_mixins` is the only place that sees both halves, so it refuses.
#[test]
fn an_image_with_no_superclass_clause_message_for_a_clause_that_needs_one_is_refused() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let derived = mixin_of(mutator.heap(), class)
            .nested
            .iter()
            .find(|nested| nested.name.as_str() == "Derived")
            .cloned()
            .expect("Args declares Derived");
        assert!(
            derived.superclass_message_code.is_some(),
            "Derived's clause really does compile a message body"
        );
        let doctored = with_nested(mixin_of(mutator.heap(), class), "Derived", |nested| Mixin {
            superclass_message_code: None,
            ..nested
        });
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no message body");
        assert_eq!(error, decode::DecodeError::MissingSuperclassMessage);
        assert_eq!(
            error.to_string(),
            "the image's superclass clause passes factory arguments it has no compiled code for"
        );
    });
}

/// The clause's *receiver* side of the same shape: a mixin-table row that
/// declares a superclass clause, in an image carrying no compiled code for
/// it, would panic when the first nested-class access pushes the clause's
/// activation (`nested::superclass_clause_code`). `verify_mixins` is the only
/// place that sees both halves, so it refuses.
#[test]
fn an_image_with_no_superclass_clause_for_a_clause_that_needs_one_is_refused() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let derived = mixin_of(mutator.heap(), class)
            .nested
            .iter()
            .find(|nested| nested.name.as_str() == "Derived")
            .cloned()
            .expect("Args declares Derived");
        assert!(
            derived.superclass_clause_code.is_some(),
            "Derived's clause really does compile a clause body"
        );
        let doctored = with_nested(mixin_of(mutator.heap(), class), "Derived", |nested| Mixin {
            superclass_clause_code: None,
            ..nested
        });
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no clause body");
        assert_eq!(error, decode::DecodeError::MissingSuperclassClause);
        assert_eq!(
            error.to_string(),
            "the image declares a superclass clause it has no compiled code for"
        );
    });
}

/// An image is untrusted input, and `verify_mixins` reconciles a
/// message body's kind and arity but nothing constrains what it *returns*.
/// `instantiate` unpacks the argument list out of that value, so a body
/// returning something that is not an argument list must raise a
/// `NewspeakError` rather than panic on the way past.
#[test]
fn a_superclass_message_body_that_returns_no_argument_list_is_an_error_not_a_panic() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // The one thing an image can present that the load-time checks let
        // through: right kind, right arity, wrong value.
        let doctored = Rc::new(CompiledCode {
            kind: CodeKind::SuperclassMessage,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 0,
                    literal: Literal::SmallInteger(1),
                },
                Instruction::Return { register: 0 },
            ],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 0),
        });
        let doctored = with_nested(mixin_of(mutator.heap(), class), "Derived", |nested| {
            assert!(
                nested.superclass_message_code.is_some(),
                "Derived's clause compiles a message body"
            );
            Mixin {
                superclass_message_code: Some(doctored),
                ..nested
            }
        });
        install_mixin(mutator, class, doctored);
        // An image is the only way such a body reaches an interpreter, so
        // the tampered mixin travels through one.
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let platform = loaded.interpreter.platform();
    let outer = expect_ok(
        loaded
            .interpreter
            .instantiate(&loaded.root_class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let derived = expect_ok(
        loaded
            .interpreter
            .send(&outer, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    loaded.interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&derived) else {
            panic!("the accessor must yield a class object")
        };
    });
    match loaded.interpreter.instantiate(&derived, &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error
                .message
                .contains("did not evaluate its message arguments"),
            "unexpected error message: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("a non-local return escaped `instantiate`"),
        Ok(_) => panic!("expected a NewspeakError, got an instance"),
    }
}

/// The two clause frame kinds share `Initializer`'s shape on the wire (a class
/// tag) but not its tag, which is what lets `image::decode`'s `frame_body`
/// resolve each against the code table its own body actually lives in. Nothing
/// leaves such a frame reachable from an image's roots --
/// a clause body contains no `MakeClosure`, so nothing can capture one -- so
/// only a hand-written section reaches the decoder's tag-4 and tag-5 branches.
#[test]
fn the_superclass_clause_frame_kinds_decode() {
    for (tag, expect_clause) in [(4u8, true), (5u8, false)] {
        let mut writer = one_frame_prefix();
        writer.write_u8(tag);
        writer.write_u8(CLASS_BUILTIN); // class tag: builtin
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        writer.write_boolean(false); // continuable
        writer.write_u32(0); // no roots
        let bytes = writer.into_bytes();
        GraphFixture::new().decoded(&bytes, "a superclass clause frame", |heap, decoded| {
            let frame = decoded.frames.first().expect("the one frame");
            let kind = heap.frame_kind(*frame);
            let matched = match &kind {
                FrameKind::SuperclassClause(_) => expect_clause,
                FrameKind::SuperclassMessage(_) => !expect_clause,
                _ => false,
            };
            assert!(matched, "frame kind tag {tag} decoded to the wrong kind");
        });
    }
}

// ---------------------------------------------------------------------------
// The image symbol table: every selector and name written once, referenced
// everywhere else by index.
// ---------------------------------------------------------------------------

const TRIVIAL_MODULE: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
    ) (
      foo = ( ^1 )
    ) : ()
"};

/// A full `image::snapshot` of `source`, with no roots -- these tests care
/// about the code table's own bytes, not the object graph.
fn snapshot_of(source: &str) -> Vec<u8> {
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        )
        .expect("snapshot")
    })
}

/// The number of times `needle` occurs in `haystack`, overlapping matches
/// included -- more than a dedup regression could ever produce, so any
/// count above the number of *distinct* symbols the source names is
/// evidence of an inline copy the symbol table failed to catch.
fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// The image's bytes from the symbol table onward, skipping the header, the
/// verbatim source text and its origin -- which necessarily still contain
/// every selector as plain source. The dedup property applies only from the
/// symbol table on: everything that would otherwise be an inline `write_str`
/// of a selector or name lives at or after it.
fn bytes_from_symbol_table(bytes: &[u8]) -> &[u8] {
    let mut reader = decode::Reader::new(bytes);
    decode::read_header(&mut reader).expect("valid header");
    reader.read_str().expect("source");
    decode::decode_source_origin(&mut reader).expect("origin");
    &bytes[reader.position()..]
}

#[test]
fn a_repeated_selector_is_stored_once() {
    // `distinctiveSelector` is deliberately long and specific: a single
    // byte such as `b"b"` recurs throughout any binary stream by chance,
    // which would make `occurrences` meaningless.
    // This selector is the method-dictionary key for one method and the
    // send target at three call sites -- four inline occurrences without
    // deduplication, one symbol-table entry with it. (The verbatim source
    // text, stored earlier in the image, still contains the selector as
    // plain text -- `bytes_from_symbol_table` excludes that section, which
    // the symbol table does not cover.)
    let bytes = snapshot_of(indoc! {"
        Newspeak3
        'T'
        class C = Object new (
        ) (
          a = ( ^self distinctiveSelector )
          b = ( ^self distinctiveSelector )
          distinctiveSelector = ( ^self distinctiveSelector )
        ) : ()
    "});
    assert_eq!(
        occurrences(bytes_from_symbol_table(&bytes), b"distinctiveSelector"),
        1
    );
}

/// Overwrites the symbol table's own entry count to claim zero symbols.
/// `TRIVIAL_MODULE` compiles exactly one method, so its mixin section holds
/// one real reference to symbol id 0 -- which does not exist once the
/// table it indexes into claims to be empty. Located via the real decode
/// functions (header, source, origin) rather than a hardcoded byte offset,
/// so this does not drift if an earlier section's encoding changes shape.
fn corrupt_first_symbol_id(bytes: &mut [u8]) {
    let count_at = {
        let mut reader = decode::Reader::new(bytes);
        decode::read_header(&mut reader).expect("valid header");
        reader.read_str().expect("source");
        decode::decode_source_origin(&mut reader).expect("origin");
        reader.position()
    };
    bytes[count_at..count_at + 4].copy_from_slice(&0u32.to_le_bytes());
}

#[test]
fn an_out_of_range_symbol_id_is_an_error_not_a_panic() {
    let mut bytes = snapshot_of(TRIVIAL_MODULE);
    corrupt_first_symbol_id(&mut bytes);
    assert!(ns_runtime::image::load(&bytes).is_err());
}

/// The same hostile input, but at the level that actually raises
/// `DecodeError::BadSymbolId`: a hand-crafted mixin table whose one row
/// names a selector id past the end of a one-entry symbol table. The
/// end-to-end test above proves `load` as a whole refuses the image; this
/// one proves it is specifically the mixin table's range check that catches
/// it, not some unrelated failure earlier in the pipeline.
#[test]
fn a_root_lists_selector_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_row_mixin_table(0, &[(5, 0)]);
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::BadSymbolId(5))
    );
}

/// A slot record's three fields, tagged as the encoder tags them.
fn slot_record(name: u32, mutable: bool, access: AccessModifier) -> (u32, u8, u8) {
    (
        name,
        mixins::slot_kind_tag(mutable, false),
        mixins::access_tag(access),
    )
}

/// A one-row mixin table with `slots` and `methods`, decoded against a
/// symbol table of `names` and one method body.
fn decode_one_row_with_slots(
    names: &[&str],
    slots: &[(u32, u8, u8)],
    methods: &[(u32, u32)],
) -> Result<mixins::Decoded, decode::DecodeError> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    write_mixin_row_with_slots(&mut writer, 0, slots, methods, &[]);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    let codes = [Rc::new(every_instruction_once())];
    mixins::decode(&mut reader, &table_with(names), &codes)
}

/// `decode_one_row_with_slots` with every field of each method chosen.
fn decode_one_row_with_method_shapes(
    names: &[&str],
    slots: &[(u32, u8, u8)],
    methods: &[MethodShape],
) -> Result<mixins::Decoded, decode::DecodeError> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    write_mixin_row_with_method_shapes(&mut writer, 0, slots, methods, &[]);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    let codes = [Rc::new(every_instruction_once())];
    mixins::decode(&mut reader, &table_with(names), &codes)
}

/// A lazy slot record (§6.3.3) named by symbol `name`, immutable.
fn lazy_slot_record(name: u32, access: AccessModifier) -> (u32, u8, u8) {
    (
        name,
        mixins::slot_kind_tag(false, true),
        mixins::access_tag(access),
    )
}

/// A lazy slot's getter as the encoder writes it: a unary method of the
/// slot's name and accessibility.
fn getter_shape(selector: u32, access: AccessModifier) -> MethodShape {
    MethodShape {
        selector,
        access,
        argument_count: 0,
        code: 0,
    }
}

/// A lazy slot decodes as one, with no accessor getter: its getter is one
/// of the row's methods, a unary method of the slot's name that runs the
/// body the row names -- whose `ReturnSlotIfFilled` answers the filled slot
/// -- and carries no fast path, as no decoded method does.
#[test]
fn a_decoded_lazy_slot_keeps_its_getter() {
    let decoded = decode_one_row_with_method_shapes(
        &["X", "eager", "later"],
        &[
            slot_record(1, false, AccessModifier::Public),
            lazy_slot_record(2, AccessModifier::Public),
        ],
        &[getter_shape(2, AccessModifier::Public)],
    )
    .expect("decode");
    let root = decoded.module_root(0).expect("a root row");
    assert!(root.slots[1].lazy, "the slot record says lazy");
    let getter = root
        .compiled_method(Symbol::intern("later"))
        .expect("the getter is a compiled method");
    assert_eq!(getter.argument_count, 0);
    assert_eq!(getter.fast_path, None);
}

/// Two lazy slots of one name would share the one getter the row carries
/// for that name, which serves only one of their fields and leaves the other
/// dead: the row is refused as declaring that member twice, as two eager
/// slots of one name are.
#[test]
fn a_row_declaring_one_lazy_slot_twice_is_refused() {
    let error = decode_one_row_with_method_shapes(
        &["X", "later"],
        &[
            lazy_slot_record(1, AccessModifier::Public),
            lazy_slot_record(1, AccessModifier::Public),
        ],
        &[getter_shape(1, AccessModifier::Public)],
    )
    .err();
    assert_eq!(
        error,
        Some(decode::DecodeError::DuplicateSelector("later".to_string()))
    );
}

/// A lazy slot's getter takes no argument and has the slot's accessibility
/// (`MixinMethod::lazy_slot_getter`). A method of the slot's name that
/// differs in either is not its getter, and the row is refused rather than
/// decoded into a slot without one.
#[test]
fn a_lazy_slot_whose_method_is_not_its_getter_is_refused() {
    let taking_an_argument = MethodShape {
        argument_count: 1,
        ..getter_shape(1, AccessModifier::Public)
    };
    let of_another_access = getter_shape(1, AccessModifier::Private);
    for method in [taking_an_argument, of_another_access] {
        assert_eq!(
            decode_one_row_with_method_shapes(
                &["X", "later"],
                &[lazy_slot_record(1, AccessModifier::Public)],
                &[method],
            )
            .err(),
            Some(decode::DecodeError::LazySlotWithoutGetter(
                "later".to_string()
            ))
        );
    }
}

#[test]
fn an_invalid_slot_kind_tag_is_a_decode_error_not_a_panic() {
    assert_eq!(
        decode_one_row_with_slots(
            &["X", "x"],
            &[(1, 4, mixins::access_tag(AccessModifier::Protected))],
            &[]
        )
        .err(),
        Some(decode::DecodeError::InvalidTag {
            what: "slot kind",
            tag: 4
        })
    );
}

/// A decoded slot defines the accessors a slot declared in source does
/// (`newspeak-spec.pdf` §6.3.2): a getter with the slot's accessibility and,
/// for a mutable slot, a setter -- derived from the slot record, which lists
/// none of them.
#[test]
fn a_decoded_slot_defines_its_accessors() {
    let decoded = decode_one_row_with_slots(
        &["X", "fixed", "mutable"],
        &[
            slot_record(1, false, AccessModifier::Public),
            slot_record(2, true, AccessModifier::Private),
        ],
        &[],
    )
    .expect("decode");
    let root = decoded.module_root(0).expect("a root row");
    let accessor = |selector: &str| match root.methods.get(&Symbol::intern(selector)) {
        Some(ns_runtime::mixin::MethodEntry::SlotAccessor(accessor)) => Some(*accessor),
        _ => None,
    };
    let fixed = accessor("fixed").expect("a getter");
    assert_eq!(fixed.access, AccessModifier::Public);
    assert!(
        accessor("fixed:").is_none(),
        "an immutable slot has no setter"
    );
    let setter = accessor("mutable:").expect("a mutable slot's setter");
    assert_eq!(setter.access, AccessModifier::Private);
    assert_eq!(setter.own_index, 1);
    assert_eq!(root.methods.len(), 3);
}

/// A row whose compiled method shares a selector with one of its slots'
/// accessors: its method table could keep only one of the two, so the row
/// is refused, as the resolver refuses the same declaration from source.
#[test]
fn a_row_whose_method_shares_a_slot_accessors_selector_is_refused() {
    // Symbol 1 is `x`, symbol 2 `x:`; the method is named by `method`.
    for (slot, method, expected) in [
        (slot_record(1, false, AccessModifier::Protected), 1, "x"),
        (slot_record(1, true, AccessModifier::Protected), 2, "x:"),
    ] {
        let error = decode_one_row_with_slots(&["X", "x", "x:"], &[slot], &[(method, 0)])
            .expect_err("a colliding row is refused");
        assert_eq!(
            error,
            decode::DecodeError::DuplicateSelector(expected.to_string())
        );
        assert_eq!(
            error.to_string(),
            format!("a mixin-table row declares member `{expected}` more than once")
        );
    }
}

/// §6.3.2: an immutable slot has "no setter method", so a method named by
/// the slot's name with `:` appended is a member of its own.
#[test]
fn a_method_named_like_an_immutable_slots_setter_decodes() {
    let decoded = decode_one_row_with_slots(
        &["X", "x", "x:"],
        &[slot_record(1, false, AccessModifier::Protected)],
        &[(2, 0)],
    )
    .expect("an immutable slot defines no setter to collide with");
    let root = decoded.module_root(0).expect("a root row");
    assert!(root.compiled_method(Symbol::intern("x:")).is_some());
}

#[test]
fn a_row_that_names_one_selector_twice_is_refused() {
    assert_eq!(
        decode_one_row_with_slots(&["X", "m"], &[], &[(1, 0), (1, 0)]).err(),
        Some(decode::DecodeError::DuplicateSelector("m".to_string()))
    );
}

/// §6: a nested declaration "implicitly defines a method with the same
/// name", so a row whose method shares a nested row's name declares that
/// member twice.
#[test]
fn a_row_whose_method_shares_a_nested_declarations_name_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    write_mixin_row(&mut writer, 0, &[(1, 0)], &[1]);
    write_mixin_row(&mut writer, 1, &[], &[]);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    let codes = [Rc::new(every_instruction_once())];
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X", "Inner"]), &codes).err(),
        Some(decode::DecodeError::DuplicateSelector("Inner".to_string()))
    );
}

/// `symbols::Table::get`'s own bounds check, isolated from everything else
/// decoding touches: an empty table has no id 0, and `get` must say so
/// rather than index-panic.
#[test]
fn an_empty_symbol_tables_get_is_an_error_not_a_panic() {
    let table = symbols::Table::empty();
    assert_eq!(table.get(0), Err(decode::DecodeError::BadSymbolId(0)));
}

#[test]
fn a_symbol_written_at_many_call_sites_still_resolves_to_the_same_symbol_everywhere() {
    // The dedup property from the caller's side: every occurrence of
    // `distinctiveSelector` -- the method itself and every send of it --
    // must still resolve to the same interned `Symbol` after a load, not
    // merely compress on the wire.
    let bytes = snapshot_of(indoc! {"
        Newspeak3
        'T'
        class C = Object new (
        ) (
          public a = ( ^self distinctiveSelector )
          distinctiveSelector = ( ^42 )
        ) : ()
    "});
    let mut loaded = image::load(&bytes).expect("load");
    let instance = expect_ok(
        loaded.interpreter.instantiate(&loaded.root_class, &[]),
        "instantiate",
    );
    let result = expect_ok(
        loaded.interpreter.send(&instance, Symbol::intern("a"), &[]),
        "a",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&result), Value::SmallInteger(42)));
    });
}

// ---------------------------------------------------------------------------
// The mixin table. What the image says a declaration *is* -- its
// name, slots, methods, factory, superclass clause, nesting and class side --
// comes back from the table, never by re-parsing the image's own source text.
// ---------------------------------------------------------------------------

const TABLE_NESTED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new (| s. |) (
        public class Inner withV: a = Object new (| v = a. |) (
            public value = ( ^v )
        )
    ) : (
        public marker = ( ^2 )
    )
"};

/// Decodes an image's symbol, code and mixin tables out of its own bytes,
/// walking the earlier sections with the real decoders rather than
/// hardcoded offsets that would drift as the format's earlier fields change.
fn mixin_table_of(bytes: &[u8]) -> mixins::Decoded {
    let mut reader = decode::Reader::new(bytes);
    decode::read_header(&mut reader).expect("valid header");
    reader.read_str().expect("source");
    decode::decode_source_origin(&mut reader).expect("origin");
    let symbols = symbols::decode(&mut reader).expect("symbol table");
    let codes = decode::decode_code_table(&mut reader, &symbols).expect("code table");
    mixins::decode(&mut reader, &symbols, &codes.codes).expect("mixin table")
}

/// A complete image whose mixin table is `mixin_table` verbatim: a real
/// header, a real (trivially parseable) source and origin, a real symbol
/// table holding `strings`, and an empty code table. Everything before the
/// mixin table comes from the real encoders, so a fixture built this way
/// exercises the mixin table's own validation rather than an accident in an
/// earlier section. Nothing follows the mixin table, so every test using
/// this asserts an error -- the point is only ever *which* error, and that
/// it is an error rather than an abort.
fn image_with_mixin_table(strings: &[&str], mixin_table: &[u8]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    encode::write_header(&mut writer, encode::FLAG_HAS_SOURCE);
    writer.write_str(TRIVIAL_MODULE);
    encode::encode_source_origin(&mut writer, &Origin::File(PathBuf::from("t.ns")));
    let mut builder = symbols::Builder::new();
    for string in strings {
        builder.intern(string);
    }
    builder.write(&mut writer);
    writer.write_u32(0); // an empty code table
    writer.write_raw(mixin_table);
    writer.into_bytes()
}

#[test]
fn a_nested_mixin_round_trips_through_the_table() {
    let bytes = snapshot_of(TABLE_NESTED_SOURCE);
    let decoded = mixin_table_of(&bytes);
    let root = decoded.module_root(0).expect("a root row").clone();
    assert_eq!(root.name, Symbol::intern("Outer"));
    assert_eq!(
        root.slots
            .iter()
            .map(|slot| slot.name.as_str())
            .collect::<Vec<_>>(),
        vec!["s"]
    );
    assert!(
        root.class_side.is_some(),
        "a class side is a row of its own, referenced by id"
    );
    assert_eq!(root.nested.len(), 1);
    let inner = &root.nested[0];
    assert_eq!(inner.name, Symbol::intern("Inner"));
    assert_eq!(
        inner.access,
        ns_syntax::ast::AccessModifier::Public,
        "the accessor's accessibility travels with the declaration"
    );
    assert_eq!(
        inner
            .slots
            .iter()
            .map(|slot| slot.name.as_str())
            .collect::<Vec<_>>(),
        vec!["v"]
    );
    let factory = inner.factory.expect("Inner declares a factory");
    assert_eq!(factory.selector, Symbol::intern("withV:"));
    assert_eq!(factory.argument_count, 1);
    let value = inner
        .compiled_method(Symbol::intern("value"))
        .expect("Inner declares `value`")
        .clone();
    assert_eq!(value.argument_count, 0);
    assert_eq!(value.access, ns_syntax::ast::AccessModifier::Public);
    // The point of the whole task: nothing in the rebuilt tree came from an
    // `ast` node, and none was invented to fill a field either.
    assert!(
        inner.declaration.is_none(),
        "a decoded mixin has no declaration"
    );
    assert!(value.declaration.is_none(), "nor does a decoded method");
    assert!(inner.initializer_expressions.is_empty());
}

/// The nesting depth is input-controlled, so decoding must not recurse
/// proportionally to it -- and neither must anything the decode hands back,
/// since dropping a tree of `Rc<Mixin>` walks it recursively too. 100_000
/// declared levels must produce an error, never an abort.
#[test]
fn a_deeply_nested_mixin_tree_does_not_blow_the_stack() {
    let bytes = image_with_mixin_table(&["X"], &mixin_chain(100_000));
    let Err(error) = ns_runtime::image::load(&bytes) else {
        panic!("100_000 levels must be refused");
    };
    assert!(
        error.contains("nests deeper than"),
        "expected the nesting ceiling to be what refuses it, got: {error}"
    );
}

/// The other side of the ceiling: a tree just inside it decodes, iteratively
/// and completely. Without this, a decoder that refused *every* nested tree
/// would pass the test above.
#[test]
fn a_mixin_tree_just_inside_the_nesting_ceiling_still_decodes() {
    let bytes = mixin_chain(128);
    let mut reader = decode::Reader::new(&bytes);
    let decoded = mixins::decode(&mut reader, &table_with(&["X"]), &[]).expect("128 levels decode");
    assert_eq!(decoded.mixins.len(), 128);
    let mut level = decoded.module_root(0).expect("a root row").clone();
    let mut depth = 1;
    while let Some(next) = level.nested.first().cloned() {
        level = next;
        depth += 1;
    }
    assert_eq!(depth, 128, "every level is linked to its child");
}

#[test]
fn a_mixin_id_pointing_past_the_table_is_an_error() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    write_mixin_row(&mut writer, 0, &[], &[7]); // one row, nesting a row that is not there
    let table = writer.into_bytes();
    let mut reader = decode::Reader::new(&table);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::BadMixinId(7))
    );
    // And end to end, through `load`, which must refuse rather than panic.
    let bytes = image_with_mixin_table(&["X"], &table);
    assert!(ns_runtime::image::load(&bytes).is_err());
}

/// A row that is not part of the declaration tree -- here, a row naming
/// itself as its own nested declaration -- is refused rather than paired up
/// wrongly. Row position is the whole correspondence between a row and the
/// declaration it describes.
#[test]
fn a_mixin_table_that_is_not_a_declaration_tree_is_an_error() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    write_mixin_row(&mut writer, 0, &[], &[1]);
    write_mixin_row(&mut writer, 0, &[], &[1]); // row 1 nests itself
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::MixinTableNotATree { row: 1 })
    );
}

/// A row no earlier row reaches starts a tree of its own: the table then
/// describes two modules while the image declares one, and `load` refuses
/// the mismatch rather than dropping or misplacing the second tree.
#[test]
fn a_mixin_row_nothing_reaches_is_a_second_tree_the_image_does_not_declare() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    write_mixin_row(&mut writer, 0, &[], &[]);
    write_mixin_row(&mut writer, 0, &[], &[]); // reached by no row
    let table = writer.into_bytes();
    let decoded = mixins::decode(&mut decode::Reader::new(&table), &table_with(&["X"]), &[])
        .expect("a forest of two trees decodes");
    assert_eq!(decoded.module_rows, vec![0, 1]);
    let bytes = image_with_mixin_table(&["X"], &table);
    let Err(error) = ns_runtime::image::load(&bytes) else {
        panic!("a second, undeclared tree must be refused");
    };
    assert_eq!(
        error,
        decode::DecodeError::ModuleCountMismatch {
            declared: 1,
            trees: 2
        }
        .to_string()
    );
    assert_eq!(
        error,
        "the image declares 1 module(s) but its mixin table holds 2 tree(s)"
    );
}

/// Several modules' declarations share one mixin table and one code table:
/// each module's `mixin::preorder` tree, one after another in module order,
/// and each tree's first row is that module's root.
#[test]
fn two_modules_encode_as_two_trees_in_module_order() {
    let (mut interpreter, first) = load(indoc! {"
        Newspeak3
        'Test'
        class First = Object new () (
            class Inner = Object new () ( public answer = ( ^1 ) ) : ()
        ) : ()
    "});
    let second_source = indoc! {"
        Newspeak3
        'Test'
        class Second = Object new () ( public answer = ( ^2 ) ) : ()
    "};
    let second_module = ns_syntax::parse(second_source).expect("parse");
    let resolved = ns_runtime::resolve(
        second_module,
        Rc::from(second_source),
        SourceOrigin::File(PathBuf::from("Second.ns")),
    )
    .expect("resolve");
    let second = interpreter.load_module(&resolved);
    interpreter.enter(|mutator| {
        let roots = [
            class_reference(mutator, &first),
            class_reference(mutator, &second),
        ];
        let rows = mixins::plan(mutator.heap(), &roots).expect("plan");
        let names: Vec<&str> = rows.iter().map(|row| row.mixin.name.as_str()).collect();
        // First's tree -- itself, its nested Inner, Inner's and its own
        // class sides in preorder -- then Second's.
        assert_eq!(names.first(), Some(&"First"));
        let second_at = names
            .iter()
            .position(|name| *name == "Second")
            .expect("a row for Second");
        assert!(names[..second_at].contains(&"Inner"));
        assert!(!names[second_at..].contains(&"Inner"));
        let mut symbols = symbols::Builder::new();
        let mut code = encode::Writer::new();
        let ids = encode::encode_code_table(&mut code, mutator.heap(), &roots, &mut symbols)
            .expect("encode code table");
        let mut writer = encode::Writer::new();
        mixins::encode(&mut writer, mutator.heap(), &roots, &mut symbols, &ids)
            .expect("encode mixin table");
        let table = symbols.build();
        let code_bytes = code.into_bytes();
        let decoded_code = decode::decode_code_table(&mut decode::Reader::new(&code_bytes), &table)
            .expect("decode code table");
        let bytes = writer.into_bytes();
        let decoded = mixins::decode(
            &mut decode::Reader::new(&bytes),
            &table,
            &decoded_code.codes,
        )
        .expect("decode mixin table");
        assert_eq!(decoded.module_rows, vec![0, second_at as u32]);
        assert_eq!(
            decoded.module_root(0).map(|mixin| mixin.name.as_str()),
            Some("First")
        );
        assert_eq!(
            decoded.module_root(1).map(|mixin| mixin.name.as_str()),
            Some("Second")
        );
        assert!(decoded.module_root(2).is_none());
    });
}

/// A graph can reach a closure over a body no module the image carries
/// compiled; asking for its id is an encoding error the caller reports, not
/// a panic.
#[test]
fn a_body_the_code_table_does_not_hold_has_no_id() {
    let ids = encode::CodeIds::default();
    let orphan = Rc::new(every_instruction_once());
    match ids.id_of(&orphan) {
        Err(error) => assert_eq!(
            error,
            "image encoding cannot serialize a closure over code that no module the image \
             carries compiled"
        ),
        Ok(id) => panic!("an unknown body must be an error, got id {id}"),
    }
}

/// The factory selector must come from the table, not from
/// `Mixin::declaration`: `instantiate::clause_message_body` checks the selector
/// a clause names against the superclass's primary factory selector. Read off a
/// declaration, a decoded mixin (which has none) would fall back to `new`, and
/// every class whose superclass has a keyword factory would raise a spurious
/// "not the primary factory message" error on the first instantiation after a
/// load. Both halves are asserted: that the table carries the selector and its
/// arity, and that a real load then instantiates such a class without error.
#[test]
fn a_class_whose_superclass_has_a_keyword_factory_survives_a_round_trip() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let outer = mutator.fetch(&outer);
        image::snapshot(
            mutator,
            Some(NESTED_SUPERCLASS_ARGUMENTS),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[outer],
        )
        .expect("snapshot")
    });

    let decoded = mixin_table_of(&bytes);
    let root = decoded.module_root(0).expect("a root row").clone();
    let base = root
        .nested
        .iter()
        .find(|nested| nested.name == Symbol::intern("Base"))
        .expect("Base is a nested declaration");
    let derived = root
        .nested
        .iter()
        .find(|nested| nested.name == Symbol::intern("Derived"))
        .expect("Derived is a nested declaration");
    let base_factory = base.factory.expect("Base declares a factory");
    assert_eq!(base_factory.selector, Symbol::intern("withBase:"));
    assert_eq!(base_factory.argument_count, 1);
    let clause = derived.superclass.expect("Derived declares a clause");
    assert_eq!(
        clause.selector,
        Symbol::intern("withBase:"),
        "the clause's own selector, not the `new` default"
    );
    assert!(
        clause.has_arguments,
        "`Base withBase: 7` passes an argument"
    );

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded = loaded.roots.first().cloned().expect("the snapshotted Args");
    let derived_class = expect_ok(
        loaded
            .interpreter
            .send(&reloaded, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    loaded.interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&derived_class) else {
            panic!("the accessor must yield a class object")
        };
    });
    let instance = expect_ok(
        loaded.interpreter.instantiate(&derived_class, &[]),
        "instantiate Derived",
    );
    let result = expect_ok(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("value"), &[]),
        "value",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&result), Value::SmallInteger(7)));
    });
}

// ---------------------------------------------------------------------------
// The mixin table's nesting ceiling against the parser's. A limit below
// what the front end accepts would refuse to round-trip a program the
// runtime already runs from source.
// ---------------------------------------------------------------------------

/// `class C0 = Object new () ( public class C1 = ... )` -- `nested` class
/// declarations under a root, with a class side on the innermost when
/// `class_side` is set. A class side is a `Mixin` child link that the parser
/// does not count as a nesting level, so it is exactly the level that makes
/// the two ceilings differ by more than the uncounted root.
fn nested_module_source(nested: usize, class_side: bool) -> String {
    let mut source = String::from(indoc! {"
        Newspeak3
        'T'
        class C0 = Object new () (
    "});
    for level in 1..=nested {
        source.push_str(&format!("public class C{level} = Object new () (\n"));
    }
    source.push_str(if class_side { ") : ()\n" } else { ")\n" });
    for _ in 1..=nested {
        source.push_str(")\n");
    }
    source
}

/// The deepest nested-class chain `ns-syntax` accepts, discovered rather
/// than restated: the parser's `MAX_DEPTH` is private to that crate, so
/// hardcoding it here would be the second copy that drifts. Probing keeps
/// this test honest if the parser's ceiling ever moves.
fn deepest_parseable_nesting() -> usize {
    // Far past the parser's own limit; a cap only so a parser that lost its
    // guard fails this test instead of looping.
    const PROBE_CEILING: usize = 512;
    for nested in 1..=PROBE_CEILING {
        if ns_syntax::parse(&nested_module_source(nested, true)).is_err() {
            return nested - 1;
        }
    }
    panic!("the parser accepted {PROBE_CEILING} levels of class nesting: it has no ceiling to derive the image's from");
}

/// The boundary this pins: the parser counts only *nested* classes,
/// so a module it accepts has one more `Mixin` than its limit says, and a
/// class side on the innermost class adds one more link again. An image
/// ceiling set to the parser's own number refuses the two deepest legal
/// programs -- a valid construct the runtime runs happily from source but
/// could not save and reload.
#[test]
fn the_deepest_module_the_parser_accepts_round_trips() {
    let nested = deepest_parseable_nesting();
    let source = nested_module_source(nested, true);
    let bytes = snapshot_of(&source);
    let decoded = mixin_table_of(&bytes);
    assert_eq!(
        decoded.mixins.len(),
        nested + 2,
        "a root the parser does not count, {nested} nested classes, and the innermost class's class side"
    );
    let mut level = decoded.module_root(0).expect("a root row").clone();
    let mut depth = 1;
    while let Some(next) = level.nested.first().cloned() {
        level = next;
        depth += 1;
    }
    assert_eq!(depth, nested + 1, "every declared level survived the table");
    assert!(
        level.class_side.is_some(),
        "the innermost class's class side is the deepest link of all"
    );
    assert!(
        ns_runtime::image::load(&bytes).is_ok(),
        "the deepest module the parser accepts must also load"
    );
}

/// A chain of mixins deeper than the table's ceiling, assembled directly
/// rather than parsed -- the only way to build one, since the ceiling is
/// derived from the parser's.
fn deep_mixin_chain(levels: usize) -> Rc<Mixin> {
    let mut mixin = Rc::new(Mixin::empty(Symbol::intern("C0")));
    for level in 1..levels {
        mixin = Rc::new(Mixin {
            nested: vec![mixin],
            ..Mixin::empty(Symbol::intern(&format!("C{level}")))
        });
    }
    mixin
}

/// An image nested past the decoder's ceiling must be refused when it is
/// *written*. Writing one cleanly and discovering it is unloadable turns a
/// local mistake into a failure on whatever machine tries to open it.
#[test]
fn a_class_tree_nested_past_the_ceiling_is_refused_when_the_image_is_written() {
    let mut interpreter = Interpreter::bootstrap();
    // Two levels past the deepest module the parser will produce, so the
    // refusal cannot be blamed on an off-by-one at the boundary.
    let mixin = deep_mixin_chain(deepest_parseable_nesting() + 4);
    let platform = interpreter.platform();
    let error = interpreter.enter(|mutator| {
        let application = mutator.apply_mixin(&mixin, None);
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let class: ClassReference = mutator.heap_mut().allocate_object(
            object_class,
            ObjectData::Class(Box::new(ClassData::new(
                application,
                None,
                None,
                None,
                None,
            ))),
        );
        let platform = mutator.fetch(&platform);
        let Err(error) = image::snapshot(
            mutator,
            Some(TRIVIAL_MODULE),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        ) else {
            panic!("a class tree past the ceiling must not encode");
        };
        error
    });
    assert!(
        error.contains("nested") && error.contains("130"),
        "the refusal must name the ceiling it enforces, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// Depths the image controls. A class record costs 19 bytes, so a few
// megabytes buy a few hundred thousand links -- more than any Rust call
// stack survives. Every walk over such a chain, and every release of one,
// has to be a loop.
// ---------------------------------------------------------------------------

/// Deep enough that any recursive walk or recursive release overflows the
/// stack (which aborts the process) rather than merely being slow, and
/// small enough that a debug build still decodes it quickly.
const DEEP: u32 = 200_000;

/// How many superclass links hang off `head`, counted iteratively. Every
/// test below asserts this before it does anything else: a fixture that
/// silently decoded to a chain of one would pass every "did not blow the
/// stack" assertion without ever having tested anything.
fn superclass_depth<'gc>(heap: &Heap<'gc>, head: &Value<'gc>) -> u32 {
    let mut depth = 0;
    let mut level = match head {
        Value::Object(class) => Some(*class),
        _ => None,
    };
    while let Some(class) = level {
        depth += 1;
        level = heap.class_superclass(class);
    }
    depth
}

/// The same count down an instance chain's first field.
fn instance_chain_depth<'gc>(heap: &Heap<'gc>, head: &Value<'gc>) -> u32 {
    let mut depth = 0;
    let mut level = match head {
        Value::Object(object) => Some(*object),
        _ => None,
    };
    while let Some(object) = level {
        depth += 1;
        level = match heap.instance_field(object, 0) {
            Some(Value::Object(next)) => Some(next),
            _ => None,
        };
    }
    depth
}

/// A complete image whose graph section is `graph` verbatim. Everything
/// before it comes from a real `snapshot` of `EMPTY`, whose own graph
/// section is exactly the four zero counts at the end of the file -- so
/// the splice point is found by structure, not by a hardcoded offset.
fn image_with_graph_section(graph: &[u8]) -> Vec<u8> {
    let mut bytes = snapshot_of(EMPTY);
    let tail = bytes.len() - 16;
    assert_eq!(
        &bytes[tail..],
        &[0u8; 16],
        "a rootless snapshot ends in four zero counts: objects, frames, roots, tag-named hashes"
    );
    bytes.truncate(tail);
    bytes.extend_from_slice(graph);
    bytes
}

/// `levels` class records, each inheriting from the next, with the
/// outermost as the image's single root, followed by their `levels`
/// metaclass records in the same order.
fn superclass_chain_graph(levels: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2 * levels, 0);
    for level in 0..levels {
        class_record(&mut writer, levels + level, 0, |writer| {
            if level + 1 < levels {
                writer.write_boolean(true);
                writer.write_u8(CLASS_RECORD); // class tag: object-table record
                writer.write_u32(level + 1);
            } else {
                writer.write_boolean(false);
            }
            writer.write_boolean(false); // no enclosing object
            writer.write_boolean(false); // no declaring class
        });
    }
    for level in 0..levels {
        metaclass_record(&mut writer, level);
    }
    writer.write_u32(1); // one root
    writer.write_u8(VALUE_CLASS_TAG); // value tag: class tag
    writer.write_u8(CLASS_RECORD); // class tag: object-table record
    writer.write_u32(0);
    writer.write_u32(0); // no tag-named hashes
    writer.into_bytes()
}

/// `levels` instance records, each holding the next in its one field, with
/// the outermost as the image's single root -- the shape that has been
/// expressible since the object table existed.
fn instance_chain_graph(levels: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, levels, 0);
    for level in 0..levels {
        writer.write_u8(CLASS_BUILTIN); // class tag: builtin
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        writer.write_u32(level);
        writer.write_u8(OBJECT_INSTANCE); // object body: Instance
        if level + 1 < levels {
            writer.write_u32(1);
            writer.write_u8(VALUE_OBJECT_ID); // value tag: object-table id
            writer.write_u32(level + 1);
        } else {
            writer.write_u32(0);
        }
    }
    writer.write_u32(1); // one root
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(0);
    writer.write_u32(0); // no tag-named hashes
    writer.into_bytes()
}

#[test]
fn a_send_to_a_class_heading_a_long_superclass_chain_does_not_blow_the_stack() {
    // `Interpreter::send` consults `metaclass_of` for every class-object
    // receiver; `metaclass_of` itself is O(1) now that metaclasses are flat
    // (`newspeak-spec.pdf` §3.2), but the ordinary lookup that follows a
    // metaclass miss still walks this graph's whole class-object chain
    // (`dispatch::walk`), iteratively over an explicit stack, on a chain a
    // four-megabyte image describes. Recursing on either walk would abort
    // here.
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP));
    let mut loaded = image::load(&bytes).expect("load");
    let head = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let head_value = mutator.fetch(&head);
        assert_eq!(superclass_depth(mutator.heap(), &head_value), DEEP);
    });
    // Nothing on the chain declares the selector, so the send ends in an
    // ordinary Newspeak error. That it ends at all is the point.
    let answered = loaded
        .interpreter
        .send(&head, Symbol::intern("noSuchThing"), &[])
        .is_ok();
    assert!(!answered, "no class on the chain declares `noSuchThing`");
}

#[test]
fn instantiating_a_class_heading_a_long_superclass_chain_does_not_blow_the_stack() {
    // `climb_levels` visits every level of the chain -- arity,
    // clause message, initializer -- and none of these levels has
    // initializer code, so none pushes an activation: the depth bound would
    // not see a per-level recursion (`docs/design/execution.md`, "A
    // superclass chain is walked iteratively"). Recursing once per level
    // would abort here.
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP));
    let mut loaded = image::load(&bytes).expect("load");
    let head = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let head_value = mutator.fetch(&head);
        assert_eq!(superclass_depth(mutator.heap(), &head_value), DEEP);
        let Value::Object(_) = head_value else {
            panic!("the root is a class object");
        };
    });
    let instance = expect_ok(loaded.interpreter.instantiate(&head, &[]), "instantiate");
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&instance), Value::Object(_)),
            "an instance of the head class"
        );
        assert_eq!(mutator.heap().activation_depth(), 0);
    });
}

#[test]
fn dropping_a_loaded_world_whose_root_heads_a_long_class_chain_does_not_blow_the_stack() {
    // `load` has already released the decoded object table, which pins
    // every record and so drops shallowly. What survives into `LoadedImage`
    // is the root and the chain it alone owns -- and the application chain
    // `rebuild_applications` built alongside it -- so the cascade fires
    // here, at the end of the test, with no send of any kind.
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP));
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(superclass_depth(mutator.heap(), &root_0), DEEP);
    });
    drop(loaded);
}

#[test]
fn dropping_a_loaded_world_whose_root_heads_a_long_instance_chain_does_not_blow_the_stack() {
    let bytes = image_with_graph_section(&instance_chain_graph(DEEP));
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(instance_chain_depth(mutator.heap(), &root_0), DEEP);
    });
    drop(loaded);
}

#[test]
fn a_class_object_whose_own_class_is_another_class_record_is_refused() {
    // A class record's header names its metaclass, so a header naming an
    // ordinary class record pairs the class with something that is no
    // metaclass. `class` is also the single reference `Object`'s iterative
    // `Drop` cannot hand to its worklist, so a chain of them would be the one
    // link that would still release recursively.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 3, 0);
    // Object 0: a class record whose own class is object 1.
    class_record(&mut writer, 1, 0, no_class_references);
    // Object 1: an ordinary class record, with its metaclass, object 2.
    class_record(&mut writer, 2, 0, no_class_references);
    metaclass_record(&mut writer, 1);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::MetaclassPairing
    );
}

// ---------------------------------------------------------------------------
// A source-free load path. `load` decodes the world it is given; it
// never parses, resolves or compiles, and the source blob is optional and,
// when present, purely documentary.
// ---------------------------------------------------------------------------

/// A module with a nested class, a method to send after a load, and a
/// method that hands out a live block -- so a snapshot of it carries a
/// closure, and therefore frames, in its graph.
const SOURCE_FREE_MODULE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public answer = ( ^42 )
        public block = ( ^[ 42 ] )
        public class Inner = Object new (| v = 9. |) (
            public value = ( ^v )
        )
    ) : ()
"};

/// Whether `haystack` contains `needle` anywhere.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    occurrences(haystack, needle) > 0
}

/// Instantiates `module_source` and snapshots the instance (plus, with
/// `extra_send`, that send's result) -- with or without the module's source
/// text in the image.
fn snapshot_module(module_source: &str, source: Option<&str>, extra_send: Option<&str>) -> Vec<u8> {
    let (mut interpreter, class) = load(module_source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let mut roots = vec![instance.clone()];
    if let Some(selector) = extra_send {
        roots.push(expect_ok(
            interpreter.send(&instance, Symbol::intern(selector), &[]),
            selector,
        ));
    }
    // Read out after the last run, for the encoder, which runs nothing.
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let roots: Vec<Value> = roots.iter().map(|root| mutator.fetch(root)).collect();
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            source,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &roots,
        )
        .expect("snapshot")
    })
}

#[test]
fn a_source_free_image_still_runs() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, None, None);
    assert!(
        !contains(&bytes, b"class Outer"),
        "a source-free image must not contain the module's source text"
    );
    assert!(
        !contains(&bytes, b"usingPlatform: p"),
        "nor any other fragment of it"
    );
    let mut loaded = image::load(&bytes).expect("load");
    assert!(loaded.source.is_none());
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let result = expect_ok(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("answer"), &[]),
        "answer",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&result), Value::SmallInteger(42)),
            "the entry send must run through the decoded code table"
        );
    });
}

/// The whole world, not just one send: a nested class reached through its
/// accessor, instantiated, and sent to -- all from an image whose bytes
/// carry no program text at all.
#[test]
fn a_source_free_image_still_reaches_its_nested_classes() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, None, None);
    let mut loaded = image::load(&bytes).expect("load");
    let outer = loaded.roots.first().cloned().expect("the root instance");
    let inner_class = nested_class_of(&mut loaded, &outer, "Inner");
    let instance = expect_ok(
        loaded.interpreter.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let value = send_to(&mut loaded, &instance, "value");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value) {
            Value::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

#[test]
fn the_debug_variant_still_carries_source() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, Some(SOURCE_FREE_MODULE), None);
    assert!(contains(&bytes, b"class Outer"));
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        loaded.source.as_deref(),
        Some(SOURCE_FREE_MODULE),
        "the debug variant hands its source back verbatim"
    );
    // And it is still a working world, not merely a readable file.
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let result = expect_ok(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("answer"), &[]),
        "answer",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&result), Value::SmallInteger(42)));
    });
}

/// The two variants differ only in the source blob: everything a world
/// needs to run is in both, so the source-free image is the shorter one and
/// the difference is exactly the length-prefixed source.
#[test]
fn the_two_variants_differ_only_by_the_source_blob() {
    let with = snapshot_module(SOURCE_FREE_MODULE, Some(SOURCE_FREE_MODULE), None);
    let without = snapshot_module(SOURCE_FREE_MODULE, None, None);
    assert_eq!(
        with.len() - without.len(),
        SOURCE_FREE_MODULE.len() + 4,
        "the debug variant costs the source text plus its u32 length prefix"
    );
}

/// A `program_counter` past the end of the body that runs in the frame. Nothing
/// re-activates a decoded frame today, but `program_counter` is an index and
/// this bound is what keeps it from becoming one that is never checked.
#[test]
fn a_frame_program_counter_past_its_bodys_last_instruction_is_refused() {
    let (mut fixture, table) = closure_world();
    let bytes = closure_over_frame(foo_frame_at_program_counter(2, 99));
    fixture.decoded(&bytes, "decode", |heap, graph| {
        let error =
            decode::verify_graph(graph, &table, heap).expect_err("out-of-range program counter");
        assert_eq!(
            error,
            decode::DecodeError::ProgramCounterOutOfRange {
                program_counter: 99,
                instructions: 2
            }
        );
    });
}

/// `load` never calls the parser, so source that could not possibly parse
/// is still just a blob the debug variant carries around.
#[test]
fn the_debug_variants_source_is_never_parsed() {
    let bytes = snapshot_module(
        SOURCE_FREE_MODULE,
        Some("this is not Newspeak at all ("),
        None,
    );
    let loaded = image::load(&bytes).expect("load");
    assert_eq!(
        loaded.source.as_deref(),
        Some("this is not Newspeak at all (")
    );
}

/// A snapshot is taken between sends, when no activation is running. A
/// frame married to a live activation keeps its registers on the stack, not
/// in the frame the encoder reads, so a mid-execution snapshot would write
/// that frame with empty registers and report nothing. The encoder's
/// `frame_snapshot` debug-asserts against it; this pins the refusal a
/// release build also makes.
#[test]
fn a_snapshot_taken_while_an_activation_is_running_is_refused() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let platform = interpreter.platform();
    let result = interpreter.enter(|mutator| {
        let running = mutator.heap_mut().push_activation(
            ns_runtime::ActivationHeader {
                receiver: Value::Nil,
                current_class: None,
                kind: FrameKind::TopLevel,
                feedback_base: ns_runtime::interpreter::FeedbackBase::NOT_ASSIGNED,
                code: code_with(
                    CodeKind::Method,
                    0,
                    1,
                    vec![Instruction::Return { register: 0 }],
                ),
                then: ns_runtime::Continuation::Deliver,
                counted: true,
            },
            &[],
            0,
        );
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let result = image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[],
        );
        mutator.heap_mut().pop_activation(running);
        result
    });
    let Err(error) = result else {
        panic!("a snapshot with an activation still running must be refused")
    };
    assert!(error.contains("no activation is running"), "{error}");
}

/// The format records a frame's `sender`, and the decoder rebuilds it, but
/// the runtime never sets one: a live activation's sender is the stack entry
/// below it, and a returned one has none. So only a hand-built frame can
/// carry one into the encoder, and this is the test that keeps that half of
/// the frame record from rotting unnoticed.
#[test]
fn a_frames_sender_survives_a_round_trip() {
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(TRIVIAL_MODULE, |interpreter, class| {
            // Nothing runs in here, so the class read out of its root stays
            // current.
            interpreter.enter(|mutator| {
                let class = class_reference(mutator, class);
                let foo = method_code(mutator.heap(), class, "foo");
                let frame_running = |receiver: i64, sender| ns_runtime::Frame {
                    receiver: Value::SmallInteger(receiver),
                    arguments: vec![],
                    temporaries: vec![Value::Nil; foo.register_count as usize],
                    program_counter: 0,
                    sender,
                    home: None,
                    current_class: Some(class),
                    kind: FrameKind::TopLevel,
                    uncontinuable: true,
                };
                let sender = mutator.heap_mut().allocate_frame(frame_running(7, None));
                let frame = mutator
                    .heap_mut()
                    .allocate_frame(frame_running(8, Some(sender)));
                let block_class = mutator.builtin_class(BuiltinClass::Block);
                let closure = mutator.heap_mut().allocate_object(
                    block_class,
                    ObjectData::Closure(ns_runtime::ClosureData {
                        code: foo.clone(),
                        captured_frame: frame,
                        copied: Box::new([]),
                        feedback_base: ns_runtime::interpreter::FeedbackBase::NOT_ASSIGNED,
                    }),
                );
                vec![mutator.stash(Value::Object(closure))]
            })
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let home = heap
            .closure_home(*object(&decoded_0))
            .expect("expected a closure");
        assert!(matches!(heap.frame_receiver(home), Value::SmallInteger(8)));
        let sender = heap
            .frame_sender(home)
            .expect("the decoded frame keeps its sender");
        assert!(!sender.same(home));
        assert!(matches!(
            heap.frame_receiver(sender),
            Value::SmallInteger(7)
        ));
        assert!(heap.frame_sender(sender).is_none());
    });
}

/// An activation object is a record of its own: its header names the
/// kernel-private `Activation` class, the one place an image may name it,
/// and its body the frame it reifies. A load answers an activation object
/// over the loaded counterpart of that frame -- whose receiver is the loaded
/// holder, not a copy of it.
#[test]
fn an_activation_object_round_trips() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder = Object new (
          | held |
        ) (
          public makeClosure = ( ^[ 1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    // The block's captured frame is `makeClosure`'s activation, reified
    // because the block captured it, and its receiver is `instance`.
    let closure = expect_ok(
        interpreter.send(&instance, Symbol::intern("makeClosure"), &[]),
        "makeClosure",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let closure = mutator.fetch(&closure);
        let frame = mutator
            .heap()
            .closure_captured_frame(*object(&closure))
            .expect("makeClosure answers a closure");
        let activation = mutator.new_activation_object(frame);
        assert!(
            mutator
                .heap_mut()
                .set_instance_field(*object(&instance), 0, Value::Object(activation)),
            "the holder has a slot"
        );
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let holder = *object(&mutator.fetch(&loaded.roots[0]));
        let activation_class = mutator.builtin_class(BuiltinClass::Activation);
        let heap = mutator.heap();
        let held = heap
            .instance_field(holder, 0)
            .expect("the loaded holder has its slot");
        let held = *object(&held);
        assert!(heap.class_of_object(held).same(activation_class));
        let frame = heap
            .activation_object_frame(held)
            .expect("the slot holds an activation object");
        let receiver = heap.frame_receiver(frame);
        assert!(
            object(&receiver).same(holder),
            "the loaded activation reifies the loaded frame, whose receiver is the loaded holder"
        );
    });
}

/// A decoded frame never ran in the loading world, so it is never married and
/// no `^` can return into it -- whatever its record's `uncontinuable` flag
/// says. `raise_return` decides a home's liveness by `Heap::frame_activation`,
/// not by that flag: a file claiming a continuable home must still produce
/// `cannotReturn` rather than a `Signal::Return` that no activation on the
/// stack will ever catch.
#[test]
fn a_return_into_a_decoded_home_is_cannot_return_even_if_the_file_calls_it_continuable() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[ ^1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let closure = expect_ok(interpreter.send(&instance, Symbol::intern("mk"), &[]), "mk");
    // `mk` has returned, so its frame is divorced and marked uncontinuable.
    // Clear the flag, as a hand-edited file could.
    let bytes = interpreter.enter(|mutator| {
        let home = mutator
            .heap()
            .closure_home(*object(&mutator.fetch(&closure)))
            .expect("expected a closure");
        let mut forged = mutator.heap().frame_snapshot(home);
        assert!(forged.uncontinuable, "`mk` had returned");
        forged.uncontinuable = false;
        mutator.heap_mut().replace_frame(home, forged);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        let closure = mutator.fetch(&closure);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance, closure],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let decoded = loaded.roots[1].clone();
    loaded.interpreter.enter(|mutator| {
        let decoded_value = mutator.fetch(&decoded);
        let decoded_home = mutator
            .heap()
            .closure_home(*object(&decoded_value))
            .expect("expected a closure");
        assert!(
            !mutator.heap().frame_is_uncontinuable(decoded_home),
            "the flag the file carried must survive decoding, or this tests nothing"
        );
        assert!(mutator.heap().frame_activation(decoded_home).is_none());
    });
    match loaded
        .interpreter
        .send(&decoded, Symbol::intern("value"), &[])
    {
        Err(HostSignal::Error(error)) => {
            assert!(error.message.contains("cannotReturn"), "{}", error.message)
        }
        Err(HostSignal::Return) => panic!("the return escaped instead of being refused"),
        Ok(_) => panic!("a `^` into a decoded home must not return normally"),
    }
}

/// A module written so that every *interesting* kind of source text is
/// present and distinctive: a comment, an initializer expression, a slot
/// clause, several method bodies, and a nested class. Each line the leak
/// test checks is unlike any other byte sequence the image has a reason to
/// contain, so a partial leak has somewhere to show up.
const LEAK_PROBE_MODULE: &str = indoc! {"
    Newspeak3
    'Test'
    (* commentaryAboutTheFrobulator explains nothing at all *)
    class Probe usingPlatform: aPlatformSlotName = Object new (
        | mnemosyneSlot ::= 3 + 4 * 5. |
        mnemosyneSlot:: mnemosyneSlot + 111.
    ) (
        public quixoticAccumulator = ( ^mnemosyneSlot + 1 - 1 )
        public verbatimSentinel: aParameterName = ( ^aParameterName )
        public class Kaleidoscope = Object new (| innerVestige = 77. |) (
            public perpetualMotion = ( ^innerVestige * 1 )
        )
    ) : ()
"};

/// The source-free image's headline claim, tested as a property rather than by
/// spot-checking two needles: **no line of the module's source text
/// survives anywhere in a source-free image.**
///
/// Lines shorter than `MIN_DISTINCTIVE_LINE` are skipped. The threshold is
/// 12 characters because a Newspeak module's short lines are structural
/// punctuation (`)`, `(`, `) : ()`, `Newspeak3`) that a binary stream can
/// and does reproduce by chance -- a `)` is one byte, and `Newspeak3` is
/// shorter than several selectors -- so testing them would report leaks
/// that are not leaks. Every line that actually carries program text (a
/// comment, a slot clause, an initializer statement, a method body, a
/// nested class header) is well past 12 characters in this module, so the
/// threshold costs the test nothing it wanted to check.
///
/// **What this does *not* claim.** Identifiers necessarily survive: the
/// symbol table stores every selector, slot name, class name and parameter
/// name, because dispatch, slot layout and the nested-class accessor are
/// all by name at run time. So `mnemosyneSlot` is in a source-free image
/// and must be. The claim is about source *text* -- the syntax, the
/// comments, the expression structure, the layout -- which nothing on the
/// load path reads.
#[test]
fn no_line_of_the_source_survives_in_a_source_free_image() {
    /// See the doc comment above for why 12.
    const MIN_DISTINCTIVE_LINE: usize = 12;

    let bytes = snapshot_module(LEAK_PROBE_MODULE, None, None);
    let mut checked = 0;
    for line in LEAK_PROBE_MODULE.lines() {
        let line = line.trim();
        if line.len() < MIN_DISTINCTIVE_LINE {
            continue;
        }
        assert!(
            !contains(&bytes, line.as_bytes()),
            "a source-free image contains the source line {line:?}"
        );
        checked += 1;
    }
    // The sweep must actually have swept: a module whose every line fell
    // under the threshold would pass vacuously.
    assert!(
        checked >= 7,
        "only {checked} lines were long enough to check"
    );

    // The control, from the other direction: the very same lines are all
    // present in the debug variant. Without this, a bug that mangled the
    // needles (a trim that ate the text, say) would make the sweep above
    // pass for the wrong reason.
    let debug = snapshot_module(LEAK_PROBE_MODULE, Some(LEAK_PROBE_MODULE), None);
    for line in LEAK_PROBE_MODULE.lines() {
        let line = line.trim();
        if line.len() < MIN_DISTINCTIVE_LINE {
            continue;
        }
        assert!(
            contains(&debug, line.as_bytes()),
            "the debug variant is missing the source line {line:?}"
        );
    }

    // And the world the source-free image describes still runs -- so what
    // was left out really was only text.
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let value = send_to(&mut loaded, &instance, "quixoticAccumulator");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value) {
            Value::SmallInteger(146) => {}
            _ => panic!("expected SmallInteger(146) from the loaded world, got a different Value"),
        });
}

// ---------------------------------------------------------------------------
// Pinning the wire format. Every tag test above encodes and decodes
// against the same tables, so a change that shifted both sides together
// would sail through unnoticed. A golden fixture makes that impossible to
// miss; the determinism test next to it proves encoding really is
// independent of a `HashMap`'s iteration order, not merely consistent within
// one already-built mixin tree.
// ---------------------------------------------------------------------------

/// The module the golden fixture is snapshotted from. Deliberately built to
/// touch several sections of the format at once: a nested class (`Inner`,
/// with its own factory argument and method), a class side (`marker`), more
/// than one instance-side method (four: `increment`, `current`, `reset`,
/// `doubled` -- enough that a `HashMap`'s iteration order has real room to
/// vary, which is what makes the determinism test below able to fail), and a
/// slot with an initializer -- both the slot's own initial value (`count ::=
/// 0`) and a statement in the initializer body (`count:: count + 1`), which
/// is what gives the code table a `CodeKind::Initializer` entry to cover.
///
/// Left out on purpose: a superclass clause or message, multiple levels of
/// user-defined inheritance, and a paused/suspended closure. Each of those
/// already has its own dedicated round-trip test above; folding every format
/// feature into one fixture would leave a future diff of `golden-image.binary`
/// unable to say which section actually moved.
const GOLDEN_SOURCE: &str = indoc! {"
    Newspeak3
    'Golden'
    class Golden usingPlatform: p = Object new (
        | count ::= 0. |
        count:: count + 1.
    ) (
        public increment = ( count:: count + 1. ^count )
        public current = ( ^count )
        public reset = ( count:: 0. ^self )
        public doubled = ( ^count * 2 )
        public class Inner withSeed: s = Object new (| held = s. |) (
            public seed = ( ^held )
        )
    ) : (
        public marker = ( ^7 )
    )
"};

/// A source-free `image::snapshot` of `source`, compiled from scratch and
/// rooted at one freshly instantiated object -- the live object in the
/// graph `GOLDEN_SOURCE`'s doc comment promises. Called twice with the same
/// source, this compiles the module twice: each call gets its own `Mixin`
/// and its own method `HashMap`, so nothing here can share iteration order
/// between two encodes the way a single shared mixin tree would.
fn snapshot_of_without_source(source: &str) -> Vec<u8> {
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    // Read out after the run, for the encoder, which runs nothing.
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("golden.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    })
}

#[test]
fn the_wire_format_matches_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SOURCE);
    let expected = include_bytes!("fixtures/golden-image.bin");
    assert_eq!(
        bytes, expected,
        "the image wire format changed -- if that is intended, regenerate the \
         fixture in the same commit and say why"
    );
}

#[test]
fn the_code_table_encoding_is_deterministic() {
    // Compiling twice is the point: the previous version of this test shared
    // one HashMap instance between both encodes, so it would have passed
    // without the sort that actually makes encoding deterministic.
    let first = snapshot_of_without_source(GOLDEN_SOURCE);
    let second = snapshot_of_without_source(GOLDEN_SOURCE);
    assert_eq!(first, second);
}

#[test]
#[ignore = "regenerates tests/fixtures/golden-image.bin; run deliberately"]
fn regenerate_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SOURCE);
    std::fs::write("tests/fixtures/golden-image.bin", bytes).expect("write fixture");
}

#[test]
fn the_entry_field_round_trips_present_and_absent() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut symbol_builder = symbols::Builder::new();
    let id = symbol_builder.intern("main:args:");
    let mut writer = encode::Writer::new();
    symbol_builder.write(&mut writer);
    encode::encode_entry(&mut writer, Some(id));
    encode::encode_entry(&mut writer, None);
    let bytes = writer.into_bytes();

    let mut reader = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut reader).expect("symbol table");
    let present = decode::decode_entry(&mut reader, &table).expect("present entry");
    let absent = decode::decode_entry(&mut reader, &table).expect("absent entry");

    assert_eq!(
        present.map(|selector| selector.to_string()).as_deref(),
        Some("main:args:")
    );
    assert_eq!(absent, None);
}

#[test]
fn an_out_of_range_entry_symbol_id_is_a_decode_error_not_a_panic() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut writer = encode::Writer::new();
    symbols::Builder::new().write(&mut writer);
    encode::encode_entry(&mut writer, Some(9999));
    let bytes = writer.into_bytes();

    let mut reader = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut reader).expect("symbol table");
    assert!(decode::decode_entry(&mut reader, &table).is_err());
}

#[test]
fn an_unknown_header_flag_is_still_rejected_now_that_two_are_known() {
    // Bit 2 is not assigned. A flag this build does not implement means a
    // section it cannot see, so continuing would read the next field out of
    // whatever that section occupies.
    let mut writer = ns_runtime::image::encode::Writer::new();
    ns_runtime::image::encode::write_header(&mut writer, 0b100);
    let bytes = writer.into_bytes();
    let mut reader = ns_runtime::image::decode::Reader::new(&bytes);
    assert!(ns_runtime::image::decode::read_header(&mut reader).is_err());
}

/// `image::build` (the stateless binary) and `image::snapshot` (the world
/// snapshot) share one encoder for the sections that describe code and
/// structure. This is the property that sharing is supposed to guarantee:
/// built from the same compiled module, the two artifacts must agree byte
/// for byte from `origin` through `root class`, and differ only from the
/// entry field on -- present-and-mandatory for one, absent for the other.
///
/// The module declares `main:args:` itself, so the entry selector `build`
/// interns is already in the symbol table by the time it does -- the mixin
/// table names every method's selector, and `Builder::intern` dedupes by
/// content. An entry selector absent from the module would still round-trip,
/// but it would grow `build`'s symbol table past `snapshot`'s and the
/// byte-equality assertion below would fail for a reason that has nothing to
/// do with the refactor under test.
const ENTRY_SOURCE: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
    ) (
      main: p args: q = ( ^1 )
    ) : ()
"};

#[test]
fn a_built_binary_and_a_snapshot_of_the_same_source_agree_up_to_the_root_class() {
    // Build both from one compiled module so the only difference is the
    // artifact shape, then compare through the inspector rather than by raw
    // offset: a raw offset says two files differ, a section says which one.
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let platform = interpreter.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let (binary, image) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let binary = image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build");
        let platform = mutator.fetch(&platform);
        let image =
            image::snapshot(mutator, None, &origin, class, &platform, &[]).expect("snapshot");
        (binary, image)
    });

    let binary_report = image::inspect::inspect(&binary).expect("inspect binary");
    let image_report = image::inspect::inspect(&image).expect("inspect image");

    let up_to = |report: &image::inspect::Report| {
        report
            .sections
            .iter()
            .take_while(|section| section.name != "entry")
            .map(|section| (section.name, section.start, section.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        up_to(&binary_report),
        up_to(&image_report),
        "the describing sections must line up"
    );

    // From `origin`, not from byte 0: the header's flags byte necessarily
    // differs (`FLAG_STATELESS` is only ever set on `binary`), and that
    // difference is correct, not a defect the refactor should paper over.
    let start = binary_report
        .sections
        .iter()
        .find(|section| section.name == "origin")
        .expect("origin section")
        .start;
    let end = binary_report
        .sections
        .iter()
        .find(|section| section.name == "root class")
        .expect("root class section")
        .end;
    assert_eq!(
        &binary[start..end],
        &image[start..end],
        "describing bytes must be equal"
    );

    assert!(binary_report.stateless);
    assert!(!image_report.stateless);
    assert_eq!(binary_report.entry.as_deref(), Some("main:args:"));
}

/// `image::build` itself does not require the entry selector to already be
/// declared in the module -- validating that a selector exists (and has the
/// right arity) is `ns-next build`'s job, not this encoder's. An entry the
/// module never declares is exactly the case that breaks byte-identity
/// against a `snapshot` of the same source (see `Describing`'s doc comment:
/// interning it grows `build`'s symbol table past what the describing
/// sections alone produced). This test is what keeps that distinction from
/// blurring: the coupling affects only whether the two artifacts *agree*,
/// never whether `build` on its own is *correct* -- so nobody should "fix"
/// `build` later to chase an identity this format never promised here.
#[test]
fn build_accepts_an_entry_selector_the_module_never_declares() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("neverDeclared"),
        )
        .expect("build does not itself validate the entry selector against the module")
    });

    let report = image::inspect::inspect(&binary).expect("inspect");
    assert!(report.stateless);
    assert_eq!(report.entry.as_deref(), Some("neverDeclared"));
}

/// `load` on a binary must hand back an empty `roots` (a binary carries no
/// instance to resume) and the recorded entry -- the two things a later
/// `run` needs to instantiate `root_class` and start it, since there is no
/// graph section to read a starting point out of.
#[test]
fn a_binary_loads_with_no_roots_and_its_entry() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build")
    });

    let mut loaded = image::load(&binary).expect("load a binary");
    assert!(loaded.stateless);
    assert!(loaded.roots.is_empty(), "a binary carries no instance");
    assert_eq!(
        loaded.entry.map(|selector| selector.to_string()).as_deref(),
        Some("main:args:")
    );
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        assert!(
            !root_mixin_of(mutator.heap(), root_class).methods.is_empty(),
            "the binary's methods must be rebuilt, each carrying its body"
        );
    });
}

/// `image::snapshot` is a library API: a caller can hand it an empty
/// `roots` slice and still get back a *stateful* image (`header.stateless`
/// unset), not a binary -- only `image::build` sets that flag. `load` must
/// tell the two apart by the decoded flag, not by `roots.is_empty()`, since
/// that observed shape is the same for both: a stateful image with no root
/// values must still report itself as stateful, so a caller (`ns-next
/// load`) does not mistake it for a binary and instantiate a fresh,
/// unrelated instance in place of the one this artifact deliberately
/// carried none of.
#[test]
fn a_stateful_snapshot_with_empty_roots_still_loads_as_stateful() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let platform = interpreter.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let image = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(mutator, None, &origin, class, &platform, &[]).expect("snapshot")
    });

    let loaded = image::load(&image).expect("load a stateful image with no roots");
    assert!(
        !loaded.stateless,
        "a snapshot, even with an empty roots slice, is not a binary"
    );
    assert!(loaded.roots.is_empty());
    assert!(
        loaded.entry.is_none(),
        "snapshot never records an entry selector"
    );
}

/// A binary that cannot say how it starts is not a deployment artifact, and
/// accepting one would push the failure to a later, less obvious place --
/// wherever the caller eventually looks for something to send. `load`
/// refuses it up front instead.
#[test]
fn a_stateless_artifact_without_an_entry_is_refused() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build")
    });

    // The entry field is the last thing in a binary, so truncating at its
    // section start and writing a bare `0` (absent) byte in its place turns
    // a valid binary into one with no entry, without hand-building a header.
    let entry_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|section| section.name == "entry")
        .expect("entry section")
        .start;
    bytes.truncate(entry_start);
    bytes.push(0);

    // Not `.expect_err`: `LoadedImage` carries an `Interpreter`, which has
    // no `Debug` impl, so neither type has one -- `expect_err` requires
    // `T: Debug` on the `Ok` side.
    let error = match image::load(&bytes) {
        Ok(_) => panic!("a binary must carry an entry"),
        Err(error) => error,
    };
    assert!(error.contains("entry"), "unexpected error: {error}");
}

/// The stateless binary is untrusted input exactly like the snapshot image
/// is -- no fixture is a binary that a fuzzer or a half-written file
/// produced, so this sweep is the only thing standing between a truncated
/// binary and a panic. Mirrors
/// `every_truncation_of_an_image_is_reported_rather_than_panicking`, the
/// existing model for the image shape.
#[test]
fn every_truncation_of_a_binary_is_reported_rather_than_panicking() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build")
    });

    for cut in 0..binary.len() {
        // `is_err()`, not a message check, is deliberate here and only here:
        // the failure mode varies by cut (a truncated header, a truncated
        // symbol table, a truncated entry field, ...) and the point of this
        // sweep is that every one of those is a decode error rather than a
        // panic, not that they share one message. Never `{:?}` the `Ok`
        // side either: `LoadedImage` carries an `Interpreter`, which has no
        // `Debug` impl, so neither type has one.
        assert!(
            image::load(&binary[..cut]).is_err(),
            "a binary truncated to {cut} bytes must be refused, not panic"
        );
    }
    image::load(&binary).expect("the untruncated binary still loads");
}

/// The shape the trailing-byte check exists for, stated as the case
/// that motivated it rather than only as a generic tail: a binary followed
/// by an unrelated image's graph section, byte for byte what a caller gets
/// by concatenating the two artifact kinds instead of choosing one.
///
/// The message assertion is load-bearing, not decorative: this binary sets
/// `FLAG_STATELESS` and carries a present entry selector, so the earlier
/// "stateless artifact must record an entry" check does not fire and
/// `load` reaches the graph-section boundary before failing. A test that
/// only checked `is_err()` would keep passing if that boundary check were
/// deleted and some *other* check downstream happened to reject the same
/// bytes for an unrelated reason -- see the sibling test below, which
/// guards against exactly that failure mode.
#[test]
fn a_binary_with_a_graph_appended_is_refused() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let platform = interpreter.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let (binary, image_bytes) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let binary = image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("main:args:"),
        )
        .expect("build");
        let platform = mutator.fetch(&platform);
        let image_bytes =
            image::snapshot(mutator, None, &origin, class, &platform, &[]).expect("snapshot");
        (binary, image_bytes)
    });
    // The append below only exercises anything if the image actually
    // extends past where the binary ends -- a snapshot's graph section
    // guarantees that here, but assert it so a future change to either
    // artifact's layout fails loudly instead of silently appending nothing.
    assert!(
        image_bytes.len() > binary.len(),
        "the snapshot must be longer than the binary for this append to test anything"
    );

    let mut frankenstein = binary.clone();
    frankenstein.extend_from_slice(&image_bytes[binary.len().min(image_bytes.len())..]);

    let error = match image::load(&frankenstein) {
        Ok(_) => panic!("a binary with a graph section appended must be refused"),
        Err(error) => error,
    };
    assert!(
        error.contains("trailing byte"),
        "expected the trailing-byte check to fire, got: {error}"
    );
}

/// `image::snapshot` never records an entry selector -- it is not a
/// startable artifact, so it has none to record (see the doc comment on
/// `snapshot`'s `encode_entry(&mut w, None)` call). Flipping
/// `FLAG_STATELESS` on such a snapshot therefore hits `load`'s
/// `header.stateless && entry.is_none()` check and is refused for *that*
/// reason -- well before the graph section, and the trailing bytes it
/// would leave behind, are ever reached.
///
/// This is deliberately not a route to the trailing-byte check: a snapshot
/// relabelled stateless fails on the missing entry first, so it never
/// proves the trailing-byte check does anything. That path is already
/// covered above by `a_binary_with_a_graph_appended_is_refused`, which uses
/// a real binary (stateless flag set *and* an entry present) with a graph
/// appended -- reproducing it here with a different input would add
/// runtime without adding coverage. What this input does demonstrate, and
/// what the assertion below checks, is that a relabelled snapshot is
/// refused specifically because it has no entry to start from.
#[test]
fn a_relabelled_snapshot_is_refused_for_missing_entry() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let platform = interpreter.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(mutator, None, &origin, class, &platform, &[]).expect("snapshot")
    });

    // Locate the flag byte from the header layout instead of hard-coding an
    // offset, so a header change fails this assertion loudly rather than
    // quietly flipping the wrong byte.
    let flag_offset = encode::MAGIC.len() + std::mem::size_of::<u32>();
    assert_eq!(
        flag_offset, 8,
        "header layout: magic (4) + FORMAT_VERSION (4)"
    );
    bytes[flag_offset] |= encode::FLAG_STATELESS;

    let error = match image::load(&bytes) {
        Ok(_) => panic!("a snapshot relabelled stateless must still be refused"),
        Err(error) => error,
    };
    assert!(
        error.contains("entry"),
        "expected the missing-entry check to fire, got: {error}"
    );
}

/// `inspect` walks the graph section record by record rather than rebuilding
/// it, which means it mirrors a layout the loader also implements. This is
/// what keeps the two from drifting: the walk must consume the section
/// exactly, so its end lands on the file's last byte.
///
/// A layout mistake -- a field read in the wrong order, a width wrong by one,
/// a record shape that gained a member the walk does not know about --
/// desynchronizes the reader and lands it somewhere else. The assertion
/// therefore fails loudly on the class of bug that would otherwise let the
/// viewer print plausible nonsense.
#[test]
fn the_graph_walk_consumes_the_section_exactly() {
    // The third source is the one that matters: a block in a slot is the only
    // way a frame reaches the graph at all, so without it the frame half of
    // the layout is never walked and this guard would be blind to it.
    const WITH_A_FRAME: &str = indoc! {"
        Newspeak3
        'T'
        class X usingPlatform: p = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    for source in [ENTRY_SOURCE, TRIVIAL_MODULE, WITH_A_FRAME] {
        let (mut interpreter, class) = load(source);
        let platform = interpreter.platform();
        let origin = Origin::File(PathBuf::from("t.ns"));
        let instance = interpreter.instantiate(&class, std::slice::from_ref(&platform));
        // Read out after the run, for the encoder, which runs nothing.
        let image = interpreter.enter(|mutator| {
            let roots: Vec<Value> = match &instance {
                Ok(root) => vec![mutator.fetch(root)],
                // A module whose factory takes no platform argument still
                // gives a graph worth walking -- just without an instance in
                // it.
                Err(_) => Vec::new(),
            };
            let class = class_reference(mutator, &class);
            let platform = mutator.fetch(&platform);
            image::snapshot(mutator, None, &origin, class, &platform, &roots).expect("snapshot")
        });

        let report = image::inspect::inspect(&image).expect("inspect the image");
        let graph = report
            .sections
            .iter()
            .find(|section| section.name == "graph")
            .expect("a snapshot has a graph section");
        assert_eq!(
            graph.end,
            image.len(),
            "the graph walk must end at the file's last byte, not {} of {}",
            graph.end,
            image.len()
        );
    }
}

/// A snapshot taken with the caches full loads with them empty -- no cache
/// state and no base is serialized -- and a decoded closure is given a fresh
/// range of its family's size when it is first activated.
#[test]
fn a_snapshot_with_full_caches_loads_with_empty_ones() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Keeper = Object new (
            | keep ::= nil. |
        ) (
            public prepare = ( keep:: [:each | each + 1]. ^self )
            public apply: value = ( ^keep value: value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    // This test pins what a site holds right after a run -- caches full
    // before the snapshot, empty right after load -- which the cache stress
    // mode vacates at every safepoint by design
    // (`docs/design/dispatch.md`, "Invalidation"); turned off here regardless
    // of `NS_NEXT_STRESS_CACHES` so the assertions below still run under it.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("prepare"), &[]),
        "prepare",
    );
    let one = interpreter.stash(Value::SmallInteger(1));
    expect_ok(
        interpreter.send(&instance, Symbol::intern("apply:"), &[one]),
        "apply:",
    );
    assert!(interpreter.cache_statistics().filled_sites > 0);
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("keeper.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    // `image::load` bootstraps its own interpreter from the environment, so
    // it must be turned off here too, for the same reason as above.
    #[cfg(debug_assertions)]
    loaded.interpreter.set_stress_caches(false);
    // The bootstrap runs shipped code of its own (the platform's boot), so
    // an empty table is one that holds only what a fresh bootstrap leaves.
    let booted = Interpreter::bootstrap().cache_statistics().site_entries;
    assert_eq!(loaded.interpreter.cache_statistics().site_entries, booted);
    let forty_one = loaded.interpreter.stash(Value::SmallInteger(41));
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("apply:"), &[forty_one]),
        "apply: after load",
    );
    let answer = loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&answer) {
            Value::SmallInteger(integer) => integer,
            _ => panic!("apply: answers a small integer"),
        });
    assert_eq!(answer, 42);
    // `apply:`'s family is two sends (`keep`, `value:`); the decoded block's
    // is one (`+`, numbered 0 in `prepare`'s family, before the `keep:`
    // setter that takes the block as its argument). `value:` is a kernel
    // primitive method (`Kernel.ns`): caching its call site as
    // a `CachedTargetKind::Method` also reserves the target's own family --
    // `Block>>value:`'s two sends (the lexical read of `vmMirror` and its
    // `block:value:ifFail:` send) -- so the total is 5, not 3.
    assert_eq!(
        loaded.interpreter.cache_statistics().site_entries,
        booted + 5
    );
    // A second call reuses the range `fresh_range` minted for the decoded
    // closure on its first activation (`base_for` supplies only `apply:`'s
    // own range): the closure's `NOT_ASSIGNED` feedback base is written back
    // once assigned, so `site_entries` does not grow again. Minting a fresh
    // range per activation instead -- the defect this pins -- would grow it
    // by the block's family size of 1, to 6.
    let one = loaded.interpreter.stash(Value::SmallInteger(1));
    let second_answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("apply:"), &[one]),
        "apply: a second time after load",
    );
    let second_answer = loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&second_answer) {
            Value::SmallInteger(integer) => integer,
            _ => panic!("apply: answers a small integer"),
        });
    assert_eq!(second_answer, 2);
    assert_eq!(
        loaded.interpreter.cache_statistics().site_entries,
        booted + 5
    );
}

/// A metaclass that reaches the graph is rebuilt **over `Class`**: its
/// application is applied over `Class`'s, exactly as the live
/// `Mutator::attach_metaclass` builds it (`newspeak-spec.pdf` §6, a
/// metaclass is "the application of the metaclass mixin defined by the
/// class' class side to the class Class").
///
/// A metaclass is written as a metaclass record naming its class, and
/// `rebuild_applications` applies its class's metaclass mixin over `Class`;
/// its class-object superclass is `Class`'s class object, the link the live
/// metaclass has, so the two chains `dispatch::next_level` steps together
/// agree. And it is the decoded class's own metaclass, not a second one.
///
/// A class-side method that answers a block the module keeps is what puts
/// the metaclass in the graph: the closure's captured frame is the
/// class-side activation, whose `current_class` is the metaclass.
#[test]
fn a_decoded_metaclass_is_rebuilt_over_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | held ::= nil. |
        ) (
          public class Widget = Object new (
          ) (
          ) : (
            public makeBlock = ( ^[ 'from the class side' ] )
          )
          public keep = ( held:: Widget makeBlock. ^held )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("keep"), &[]),
        "keep",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        let held = mutator
            .heap()
            .instance_field(*object(&root_0), 0)
            .expect("expected an instance");
        let captured = mutator
            .heap()
            .closure_data(*object(&held))
            .expect("the slot holds the class side's block")
            .captured_frame;
        let metaclass = mutator
            .heap()
            .frame_current_class(captured)
            .expect("a class-side method activation carries its metaclass");
        let application = mutator
            .heap()
            .class_application(metaclass)
            .expect("the decoded metaclass has an application");
        let class_class = mutator.builtin_class(BuiltinClass::Class);
        let class_application = mutator
            .heap()
            .class_application(class_class)
            .expect("Class is a class object");
        assert!(
            application
                .superclass
                .as_ref()
                .is_some_and(|superclass| Rc::ptr_eq(superclass, &class_application)),
            "a decoded metaclass is applied over Class (§6)"
        );
        assert!(
            mutator
                .heap()
                .class_superclass(metaclass)
                .is_some_and(|superclass| superclass.same(class_class)),
            "a decoded metaclass's superclass is Class, as the live one's is"
        );
        let widget = mutator
            .heap()
            .class_this_class(metaclass)
            .expect("a decoded metaclass links back to its class");
        assert!(
            mutator
                .metaclass_of(widget)
                .is_some_and(|widget_metaclass| widget_metaclass.same(metaclass)),
            "the frame's metaclass is the one its class answers"
        );
    });
}

// ---------------------------------------------------------------------------
// Metaclasses on the wire (`docs/design/object-model.md`, "The image
// format"): the metaclass of a class record is a record of its own, the
// metaclass of a prebuilt class is named by tag, and the two sides of every
// class record's pairing with its metaclass record are checked on load.
// ---------------------------------------------------------------------------

/// A root with a nested class whose metaclass a slot can hold. `Holder
/// class` answers that metaclass (`tests/metaclasses.rs`); the tests below
/// put it in the slot from Rust, read off the heap the world was built in.
const METACLASS_MODULE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: platform = Object new (
      | public kept ::= nil. public other ::= nil. |
    ) (
      public class Holder = Object new () (
      ) : (
        public made = ( ^1 )
      )
    ) : ()
"};

/// `METACLASS_MODULE` instantiated, with its nested class accessed so the
/// root's hidden slot holds it: the world, its root class, the platform, the
/// root instance and the nested class object.
fn metaclass_world() -> (Interpreter, Root, Root, Root, Root) {
    let (mut interpreter, class, platform, outer) = instantiated_world(METACLASS_MODULE);
    let holder = expect_ok(
        interpreter.send(&outer, Symbol::intern("Holder"), &[]),
        "Holder",
    );
    (interpreter, class, platform, outer, holder)
}

/// Stores `value` into `outer`'s slot `selector` names, through its setter.
fn store(interpreter: &mut Interpreter, outer: &Root, selector: &str, value: Root) {
    expect_ok(
        interpreter.send(outer, Symbol::intern(selector), &[value]),
        selector,
    );
}

/// The metaclass of the class object `class` holds, rooted.
fn metaclass_root(interpreter: &mut Interpreter, class: &Root) -> Root {
    interpreter.enter(|mutator| {
        let Value::Object(class) = mutator.fetch(class) else {
            panic!("a class object");
        };
        let metaclass = mutator
            .metaclass_of(class)
            .expect("every class object has a metaclass");
        mutator.stash(Value::Object(metaclass))
    })
}

/// A metaclass keeps its identity across a round trip: `Holder class` held
/// in a slot is the decoded class's metaclass, not a second one.
#[test]
fn a_metaclass_in_a_slot_is_the_decoded_class_metaclass() {
    let (mut interpreter, class, platform, outer, holder) = metaclass_world();
    let holder_metaclass = metaclass_root(&mut interpreter, &holder);
    store(&mut interpreter, &outer, "kept:", holder_metaclass);
    let bytes = snapshot_world(
        &mut interpreter,
        METACLASS_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&outer),
    );
    let report = image::inspect::inspect(&bytes).expect("inspect walks an image with metaclasses");
    assert!(
        report
            .graph_objects
            .iter()
            .any(|object| object.contains("metaclass of object")),
        "the metaclass record is walked: {:?}",
        report.graph_objects
    );
    let mut loaded = image::load(&bytes).expect("load");
    let loaded_outer = loaded.roots[0].clone();
    let kept = send_to(&mut loaded, &loaded_outer, "kept");
    let loaded_holder = nested_class_of(&mut loaded, &loaded_outer, "Holder");
    loaded.interpreter.enter(|mutator| {
        let (Value::Object(kept), Value::Object(loaded_holder)) =
            (mutator.fetch(&kept), mutator.fetch(&loaded_holder))
        else {
            panic!("the slot and the accessor both answer class objects");
        };
        let metaclass = mutator
            .heap()
            .class_metaclass(loaded_holder)
            .expect("the decoded class record has its metaclass");
        assert!(kept.same(metaclass), "one metaclass, not two");
        assert!(
            mutator.heap().class_of_object(loaded_holder).same(kept),
            "the decoded class is an instance of the decoded metaclass"
        );
        assert!(
            mutator
                .heap()
                .class_this_class(kept)
                .is_some_and(|this_class| this_class.same(loaded_holder)),
            "the decoded metaclass links back to the decoded class"
        );
    });
    // And it is the class side the class answers a class-side send with.
    let made = send_to(&mut loaded, &loaded_holder, "made");
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&made), Value::SmallInteger(1)));
    });
}

/// The metaclass of a builtin and of the root class is named by tag and
/// resolves to the loading world's own.
#[test]
fn a_builtin_metaclass_round_trips_by_tag() {
    let (mut interpreter, class, platform, outer, _holder) = metaclass_world();
    let integer = interpreter.enter(|mutator| {
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        mutator.stash(Value::Object(integer))
    });
    let integer_metaclass = metaclass_root(&mut interpreter, &integer);
    let root_metaclass = metaclass_root(&mut interpreter, &class);
    store(&mut interpreter, &outer, "kept:", integer_metaclass);
    store(&mut interpreter, &outer, "other:", root_metaclass);
    let bytes = snapshot_world(
        &mut interpreter,
        METACLASS_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&outer),
    );
    let mut loaded = image::load(&bytes).expect("load");
    let loaded_outer = loaded.roots[0].clone();
    let kept = send_to(&mut loaded, &loaded_outer, "kept");
    let other = send_to(&mut loaded, &loaded_outer, "other");
    let loaded_class = loaded.root_class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Value::Object(kept), Value::Object(other), Value::Object(loaded_class)) = (
            mutator.fetch(&kept),
            mutator.fetch(&other),
            mutator.fetch(&loaded_class),
        ) else {
            panic!("both slots hold metaclasses");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        assert!(mutator
            .metaclass_of(integer)
            .is_some_and(|metaclass| metaclass.same(kept)));
        assert!(mutator
            .metaclass_of(loaded_class)
            .is_some_and(|metaclass| metaclass.same(other)));
        // Each is its class's class, as the header says.
        assert!(mutator.heap().class_of_object(integer).same(kept));
        assert!(mutator.heap().class_of_object(loaded_class).same(other));
    });
}

/// A metaclass's identity hash travels with it: in its record for the
/// metaclass of a class record, in the closing hash table for the metaclass
/// of a builtin and of the root class.
#[test]
fn a_metaclass_identity_hash_survives() {
    let (mut interpreter, class, platform, outer, holder) = metaclass_world();
    let holder_metaclass = metaclass_root(&mut interpreter, &holder);
    store(&mut interpreter, &outer, "kept:", holder_metaclass);
    let hashes = interpreter.enter(|mutator| {
        let (Value::Object(holder), Value::Object(root_class)) =
            (mutator.fetch(&holder), mutator.fetch(&class))
        else {
            panic!("class objects");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let metaclasses: Vec<ClassReference> = [holder, integer, root_class]
            .into_iter()
            .map(|class| mutator.metaclass_of(class).expect("a metaclass"))
            .collect();
        metaclasses
            .into_iter()
            .map(|metaclass| mutator.heap_mut().ensure_identity_hash(metaclass))
            .collect::<Vec<u32>>()
    });
    let bytes = snapshot_world(
        &mut interpreter,
        METACLASS_MODULE,
        &class,
        &platform,
        std::slice::from_ref(&outer),
    );
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .graph_tag_named_hashes
            .iter()
            .any(|entry| entry.starts_with("metaclass of Integer hash")),
        "{:?}",
        report.graph_tag_named_hashes
    );
    let mut loaded = image::load(&bytes).expect("load");
    let loaded_outer = loaded.roots[0].clone();
    let loaded_holder = nested_class_of(&mut loaded, &loaded_outer, "Holder");
    let loaded_class = loaded.root_class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Value::Object(holder), Value::Object(root_class)) =
            (mutator.fetch(&loaded_holder), mutator.fetch(&loaded_class))
        else {
            panic!("class objects");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let loaded_hashes: Vec<u32> = [holder, integer, root_class]
            .into_iter()
            .map(|class| {
                let metaclass = mutator.metaclass_of(class).expect("a metaclass");
                mutator.heap().identity_hash(metaclass)
            })
            .collect();
        assert_eq!(loaded_hashes, hashes);
    });
}

/// The metaclass of a root level is named by tag, behind the level's own
/// `CLASS_ROOT_LEVEL`, and its identity hash travels in the closing table:
/// the root and its levels are built before the graph, metaclasses
/// included, so both resolve to the loading world's own.
#[test]
fn a_root_level_metaclass_round_trips_by_tag() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let (bytes, hash) = interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let level = mutator
            .heap()
            .class_superclass(root_class)
            .expect("the root's level");
        let metaclass = mutator.metaclass_of(level).expect("a metaclass");
        let hash = mutator.heap_mut().ensure_identity_hash(metaclass);
        let platform = mutator.fetch(&platform);
        let bytes = image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            root_class,
            &platform,
            &[Value::Object(metaclass)],
        )
        .expect("snapshot");
        (bytes, hash)
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.graph_roots,
        vec!["class metaclass of <root class level 0>".to_string()]
    );
    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    let loaded_class = loaded.root_class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Value::Object(metaclass), Value::Object(root_class)) =
            (mutator.fetch(&root), mutator.fetch(&loaded_class))
        else {
            panic!("class objects");
        };
        let level = mutator
            .heap()
            .class_superclass(root_class)
            .expect("the loaded root's level");
        assert!(mutator
            .metaclass_of(level)
            .is_some_and(|level_metaclass| level_metaclass.same(metaclass)));
        assert!(mutator.heap().class_of_object(level).same(metaclass));
        assert_eq!(mutator.heap().identity_hash(metaclass), hash);
    });
}

/// No program may name `Top`, the VM mirror's class, `Activation` or the
/// metaclass of one of them, and the loader refuses an image that does, so
/// the encoder refuses to write one rather than produce an image nothing
/// loads. No source module reaches any of the six; a graph built from Rust
/// does. An activation *object* is no such reach: its header names
/// `Activation` by tag, which the loader accepts there alone
/// (`an_activation_object_round_trips`).
#[test]
fn a_graph_reaching_an_unnameable_class_is_refused() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new () ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let top = mutator.builtin_class(BuiltinClass::Top);
        let mirror_class = mutator.builtin_class(BuiltinClass::VmMirror);
        let top_metaclass = mutator.metaclass_of(top).expect("Top has a metaclass");
        let mirror_metaclass = mutator
            .metaclass_of(mirror_class)
            .expect("the mirror's class has a metaclass");
        let activation = mutator.builtin_class(BuiltinClass::Activation);
        let activation_metaclass = mutator
            .metaclass_of(activation)
            .expect("Activation has a metaclass");
        let future = mutator.builtin_class(BuiltinClass::Future);
        let future_metaclass = mutator
            .metaclass_of(future)
            .expect("Future has a metaclass");
        for (unnameable, described) in [
            (top, "Top"),
            (mirror_class, "the VM mirror's class"),
            (top_metaclass, "Top class"),
            (mirror_metaclass, "the VM mirror's class class"),
            (activation, "Activation"),
            (activation_metaclass, "Activation class"),
            (future, "Future"),
            (future_metaclass, "Future class"),
        ] {
            let refused = image::snapshot(
                mutator,
                None,
                &Origin::File(PathBuf::from("t.ns")),
                root_class,
                &platform,
                &[Value::Object(unnameable)],
            );
            let Err(message) = refused else {
                panic!("a graph reaching {described} was encoded");
            };
            assert_eq!(
                message,
                format!(
                    "image encoding refuses {described}: no program may name it, and the \
                     loader refuses an image that does"
                )
            );
        }
    });
}

/// A graph reaching the VM mirror reaches its class too, and the refusal
/// names the capability: the mirror is checked first.
#[test]
fn a_graph_reaching_the_vm_mirror_is_refused_as_the_mirror() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new () ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        // The kernel instance encloses every builtin class and holds the
        // mirror in its `vmMirror` slot.
        let object = mutator.builtin_class(BuiltinClass::Object);
        let mirror_class = mutator.builtin_class(BuiltinClass::VmMirror);
        let heap = mutator.heap();
        let kernel = heap
            .class_enclosing(object)
            .expect("the kernel instance encloses Object");
        let field_count = heap
            .instance_field_count(kernel)
            .expect("the kernel instance has fields");
        let mirror = (0..field_count)
            .filter_map(|index| heap.instance_field(kernel, index))
            .find(|field| match field {
                Value::Object(field) => heap.class_of_object(*field).same(mirror_class),
                _ => false,
            })
            .expect("the kernel holds the mirror");
        let refused = image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("t.ns")),
            root_class,
            &platform,
            &[mirror],
        );
        let Err(message) = refused else {
            panic!("a graph reaching the mirror was encoded");
        };
        assert!(
            message.starts_with("image encoding refuses the VM mirror:"),
            "{message}"
        );
    });
}

/// Two class records swap their metaclasses: each header names the other
/// class's metaclass record. Unswapped, the same records decode and pair.
#[test]
fn a_class_record_naming_another_classs_metaclass_is_refused() {
    let two_classes = |first_metaclass: u32, second_metaclass: u32| {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 4, 0);
        class_record(&mut writer, first_metaclass, 0, no_class_references);
        class_record(&mut writer, second_metaclass, 0, no_class_references);
        metaclass_record(&mut writer, 0);
        metaclass_record(&mut writer, 1);
        writer.write_u32(0); // no roots
        writer.into_bytes()
    };
    let mut fixture = GraphFixture::new();
    fixture.decoded(&two_classes(2, 3), "paired records", |heap, graph| {
        for (class, metaclass) in [(0, 2), (1, 3)] {
            assert!(heap
                .class_metaclass(graph.objects[class])
                .is_some_and(|found| found.same(graph.objects[metaclass])));
        }
    });
    let error = fixture.decode_error(&two_classes(3, 2));
    assert_eq!(error, decode::DecodeError::MetaclassPairing);
    assert_eq!(
        error.to_string(),
        "a class in the image and the metaclass it names do not belong to each other"
    );
}

/// A metaclass record no class record's header names is a second metaclass
/// beside the one its class answers.
#[test]
fn a_metaclass_record_named_by_no_class_record_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 3, 0);
    class_record(&mut writer, 1, 0, no_class_references);
    metaclass_record(&mut writer, 0);
    metaclass_record(&mut writer, 0); // a second metaclass of object 0
    writer.write_u32(0); // no roots
    assert_eq!(
        GraphFixture::new().decode_error(&writer.into_bytes()),
        decode::DecodeError::MetaclassPairing
    );
}

/// A metaclass record's reference back must name a class record; here it
/// names an instance.
#[test]
fn a_metaclass_record_naming_a_non_class_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 3, 0);
    class_record(&mut writer, 1, 0, no_class_references);
    metaclass_record(&mut writer, 2); // names object 2, an instance
    writer.write_u8(CLASS_BUILTIN); // object 2's class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    writer.write_u32(3); // identity hash
    writer.write_u8(OBJECT_INSTANCE);
    writer.write_u32(0); // no fields
    writer.write_u32(0); // no roots
    assert_eq!(
        GraphFixture::new().decode_error(&writer.into_bytes()),
        decode::DecodeError::MetaclassPairing
    );
}

/// A metaclass record is an instance of `Metaclass`; a header naming
/// anything else would give one metaclass two encodings.
#[test]
fn a_metaclass_record_whose_header_is_not_metaclass_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2, 0);
    class_record(&mut writer, 1, 0, no_class_references);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin, not `Metaclass`
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Class));
    writer.write_u32(8); // identity hash
    writer.write_u8(OBJECT_METACLASS);
    writer.write_u8(CLASS_RECORD);
    writer.write_u32(0);
    writer.write_u32(0); // no roots
    assert_eq!(
        GraphFixture::new().decode_error(&writer.into_bytes()),
        decode::DecodeError::MetaclassPairing
    );
}

/// A metaclass reference names a prebuilt class's metaclass, one level
/// deep: behind `CLASS_METACLASS` a second `CLASS_METACLASS`, or a
/// `CLASS_RECORD` -- whose metaclass is a record of its own -- is refused.
#[test]
fn a_nested_metaclass_reference_is_refused() {
    let integer = encode::builtin_class_tag(BuiltinClass::Integer);
    let mut fixture = GraphFixture::new();
    let mut one_level = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(integer);
    });
    one_level.extend_from_slice(&[0, 0, 0, 0]); // no roots
    fixture.decoded(&one_level, "Integer's metaclass by tag", |heap, graph| {
        let Some(Value::Object(element)) = heap.array_at(graph.objects[0], 0) else {
            panic!("the array holds a class object");
        };
        assert!(heap.class_this_class(element).is_some());
    });
    let nested = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(integer);
    });
    assert_eq!(
        fixture.decode_error(&nested),
        decode::DecodeError::InvalidTag {
            what: "prebuilt class",
            tag: CLASS_METACLASS
        }
    );
    let of_a_record = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_RECORD);
        writer.write_u32(0);
    });
    assert_eq!(
        fixture.decode_error(&of_a_record),
        decode::DecodeError::InvalidTag {
            what: "prebuilt class",
            tag: CLASS_RECORD
        }
    );
}

/// No source can make an instance of `Metaclass`, `Class` or a metaclass,
/// so an image recording one is refused.
#[test]
fn an_instance_of_metaclass_is_refused() {
    let instance_of = |class: &dyn Fn(&mut encode::Writer)| {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 1, 0);
        class(&mut writer);
        writer.write_u32(3); // identity hash
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0); // no fields
        writer.write_u32(0); // no roots
        writer.into_bytes()
    };
    let builtin = |kind: BuiltinClass| {
        move |writer: &mut encode::Writer| {
            writer.write_u8(CLASS_BUILTIN);
            writer.write_u8(encode::builtin_class_tag(kind));
        }
    };
    let integer_metaclass = |writer: &mut encode::Writer| {
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Integer));
    };
    let mut fixture = GraphFixture::new();
    let refused: [&dyn Fn(&mut encode::Writer); 3] = [
        &builtin(BuiltinClass::Metaclass),
        &builtin(BuiltinClass::Class),
        &integer_metaclass,
    ];
    for class in refused {
        let error = fixture.decode_error(&instance_of(class));
        assert_eq!(error, decode::DecodeError::InstanceOfAClassClass);
        assert_eq!(
            error.to_string(),
            "an object in the image is an instance of Class, Metaclass or a metaclass, \
             which only a class object may be"
        );
    }
    fixture.decoded(
        &instance_of(&builtin(BuiltinClass::Object)),
        "an instance of Object",
        |_, _| {},
    );
}

/// A class side is applied only by a metaclass record; a class record
/// applying one would be a second, unpaired metaclass.
#[test]
fn a_class_record_applying_a_class_side_is_refused() {
    let mut fixture = GraphFixture::with_source(METACLASS_MODULE);
    let class_side_row = fixture
        .mixins
        .iter()
        .position(|row| {
            fixture.mixins.iter().any(|other| {
                other
                    .class_side
                    .as_ref()
                    .is_some_and(|class_side| Rc::ptr_eq(class_side, row))
            })
        })
        .expect("Holder's class side is a row") as u32;
    let error = fixture.decode_error(&one_class_object(class_side_row, no_class_references));
    assert_eq!(error, decode::DecodeError::ClassRecordAppliesAClassSide);
    assert_eq!(
        error.to_string(),
        "a class object in the image applies a class side, which only its metaclass may"
    );
}

/// `inspect` reads every metaclass form the way `load` does, and refuses a
/// nested metaclass reference the same way.
#[test]
fn inspect_refuses_a_nested_metaclass_reference() {
    let mut graph = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_METACLASS);
    });
    graph.extend_from_slice(&[0, 0, 0, 0]); // no roots
    graph.extend_from_slice(&[0, 0, 0, 0]); // no tag-named hashes
    let bytes = image_with_graph_section(&graph);
    let error = match image::inspect::inspect(&bytes) {
        Ok(_) => panic!("inspect accepted a nested metaclass reference"),
        Err(error) => error,
    };
    assert_eq!(error, "invalid prebuilt class tag 4");
    assert_eq!(load_error(&bytes), error);
}

// ---------------------------------------------------------------------------
// Mixin applications (`newspeak-spec.pdf` §6.2) on the wire: the `<:`
// clauses a mixin-table row carries, the applied levels a class record
// describes, and the frame a `<:` clause message runs in.
// ---------------------------------------------------------------------------

/// Every part of a mixin application the image has to carry: two `<:`
/// clauses stacked on a superclass whose message takes arguments, the first
/// clause's own message taking arguments computed from the applying
/// declaration's factory parameter, and a super chain through every level.
const MIXIN_APPLICATION_MODULE: &str = indoc! {"
    Newspeak3
    'T'
    class Host usingPlatform: p = Object new () (
        public class Base withBase: b = Object new (| base = b. |) (
            public fromBase = ( ^base )
            public chain = ( ^'Base' )
        )
        public class Marker withMark: m = Object new (| mark = m. |) (
            public fromMarker = ( ^mark )
            public chain = ( ^'Marker ', super chain )
        )
        public class Stamp = Object new () (
            public chain = ( ^'Stamp ', super chain )
        )
        public class Applied from: x = Base withBase: x + 1 <: Marker withMark: x * 10 <: Stamp (
        ) (
            public chain = ( ^'Applied ', super chain )
        )
    ) : ()
"};

/// The integer `root` holds, read in a fresh entry.
fn integer_of(interpreter: &mut Interpreter, root: &Root) -> i64 {
    interpreter.enter(|mutator| match mutator.fetch(root) {
        Value::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    })
}

/// The text of the string `root` holds, read in a fresh entry.
fn text_of(interpreter: &mut Interpreter, root: &Root) -> String {
    interpreter.enter(|mutator| {
        let value = mutator.fetch(root);
        string_contents(mutator.heap(), &value)
    })
}

/// `Applied from: factory_argument` on `host`, and what the instance answers
/// to each of the three selectors its levels define between them.
fn applied_answers(
    interpreter: &mut Interpreter,
    host: &Root,
    factory_argument: i64,
) -> (i64, i64, String) {
    let applied_class = expect_ok(
        interpreter.send(host, Symbol::intern("Applied"), &[]),
        "Applied",
    );
    let argument = interpreter.stash(Value::SmallInteger(factory_argument));
    let instance = expect_ok(
        interpreter.send(&applied_class, Symbol::intern("from:"), &[argument]),
        "Applied from:",
    );
    answers_of(interpreter, &instance)
}

fn answers_of(interpreter: &mut Interpreter, instance: &Root) -> (i64, i64, String) {
    let from_base = expect_ok(
        interpreter.send(instance, Symbol::intern("fromBase"), &[]),
        "fromBase",
    );
    let from_marker = expect_ok(
        interpreter.send(instance, Symbol::intern("fromMarker"), &[]),
        "fromMarker",
    );
    let chain = expect_ok(
        interpreter.send(instance, Symbol::intern("chain"), &[]),
        "chain",
    );
    (
        integer_of(interpreter, &from_base),
        integer_of(interpreter, &from_marker),
        text_of(interpreter, &chain),
    )
}

/// A snapshot taken after the applied levels exist carries them as class
/// records: the reloaded instance still answers through every level, and a
/// fresh instantiation through the reloaded class object still evaluates each
/// level's own clause message with the applying class's factory argument.
#[test]
fn a_snapshot_of_a_world_with_applied_levels_loads_and_runs() {
    let (mut interpreter, class, platform, host) = instantiated_world(MIXIN_APPLICATION_MODULE);
    let expected = (5, 40, "Applied Stamp Marker Base".to_string());
    assert_eq!(applied_answers(&mut interpreter, &host, 4), expected);
    let applied_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Applied"), &[]),
        "Applied",
    );
    let argument = interpreter.stash(Value::SmallInteger(4));
    let instance = expect_ok(
        interpreter.send(&applied_class, Symbol::intern("from:"), &[argument]),
        "Applied from:",
    );
    let bytes = snapshot_world(
        &mut interpreter,
        MIXIN_APPLICATION_MODULE,
        &class,
        &platform,
        &[host, instance],
    );

    let mut loaded = image::load(&bytes).expect("load");
    let host = loaded.roots[0].clone();
    let instance = loaded.roots[1].clone();
    assert_eq!(answers_of(&mut loaded.interpreter, &instance), expected);
    assert_eq!(
        applied_answers(&mut loaded.interpreter, &host, 2),
        (3, 20, "Applied Stamp Marker Base".to_string())
    );
    loaded.interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let heap = mutator.heap();
        let applied = class_of(heap, &instance);
        let stamp_level = heap.class_superclass(applied).expect("a superclass");
        let marker_level = heap.class_superclass(stamp_level).expect("a superclass");
        let base = heap.class_superclass(marker_level).expect("a superclass");
        assert_eq!(class_name(heap, stamp_level), "Stamp");
        assert_eq!(class_name(heap, marker_level), "Marker");
        assert_eq!(class_name(heap, base), "Base");
        let marker_clause = heap
            .class_applied_clause(marker_level)
            .expect("the Marker level carries the clause that added it");
        assert_eq!(marker_clause.selector.as_str(), "withMark:");
        assert!(marker_clause.has_arguments);
        assert!(heap.class_applied_clause(stamp_level).is_some());
        assert!(heap.class_applied_clause(base).is_none());
    });
}

/// A stateless binary carries no class record at all, so the loaded world
/// rebuilds every level from the row's clauses the first time the accessor
/// runs the clause -- which answers one class per clause, and a decoded row
/// that lost its clauses would refuse that answer.
#[test]
fn a_binary_of_a_module_applying_mixins_builds_its_levels_after_loading() {
    let (mut interpreter, class) = load(MIXIN_APPLICATION_MODULE);
    let binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        image::build(
            mutator.heap(),
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            Symbol::intern("Applied"),
        )
        .expect("build")
    });
    let mut loaded = image::load(&binary).expect("load");
    let platform = loaded.interpreter.platform();
    let host = expect_ok(
        loaded
            .interpreter
            .instantiate(&loaded.root_class, std::slice::from_ref(&platform)),
        "instantiate Host",
    );
    assert_eq!(
        applied_answers(&mut loaded.interpreter, &host, 4),
        (5, 40, "Applied Stamp Marker Base".to_string())
    );
}

/// A root class has no enclosing object to evaluate a `<:` receiver in, so
/// its clause can name only a builtin -- the shape a module that is a value
/// takes (`class M = Object <: Value`). Its level applies a builtin's mixin,
/// which no mixin-table row describes, so the root-class section names it the
/// way it names the root's superclass: by builtin tag.
#[test]
fn a_root_class_applying_a_builtin_mixin_round_trips() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () (
            public answer = ( ^42 )
        )
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let (binary, snapshot) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let origin = Origin::File(PathBuf::from("t.ns"));
        let binary = image::build(
            mutator.heap(),
            None,
            &origin,
            class,
            Symbol::intern("answer"),
        )
        .expect("build");
        let platform = mutator.fetch(&platform);
        let snapshot =
            image::snapshot(mutator, None, &origin, class, &platform, &[]).expect("snapshot");
        (binary, snapshot)
    });
    for bytes in [binary, snapshot] {
        let mut loaded = image::load(&bytes).expect("load");
        loaded.interpreter.enter(|mutator| {
            let heap = mutator.heap();
            let root = class_reference(mutator, &loaded.root_class);
            let level = heap.class_superclass(root).expect("a superclass");
            let value = mutator.builtin_class(BuiltinClass::Value);
            assert!(Rc::ptr_eq(
                &application_of(heap, level).mixin,
                &application_of(heap, value).mixin
            ));
            let clause = heap
                .class_applied_clause(level)
                .expect("the level carries the root's clause");
            assert_eq!(clause.selector, Symbol::NEW);
            assert_eq!(
                heap.class_builtin(heap.class_superclass(level).expect("a superclass")),
                Some(BuiltinClass::Object)
            );
            assert!(heap.class_inherits_value(root));
        });
        let instance = expect_ok(
            loaded.interpreter.instantiate(&loaded.root_class, &[]),
            "instantiate Root",
        );
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&instance, Symbol::intern("answer"), &[]),
            "answer",
        );
        assert_eq!(integer_of(&mut loaded.interpreter, &answer), 42);
    }
}

/// A `<:` clause message is evaluated in an activation of its own, and a
/// block written in it captures that activation. The block outlives it --
/// the applied level's initializer keeps it in a slot -- so a snapshot holds
/// the dead activation as a frame, and loading reconciles the frame against
/// the body that ran in it: the clause's message body, which is neither the
/// applying class's superclass-clause message nor anything else its mixin
/// carries.
#[test]
fn a_frame_a_mixin_application_message_ran_in_decodes_to_that_messages_body() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Host usingPlatform: p = Object new () (
            public class Keeper withBlock: block = Object new (| kept = block. |) (
                public run = ( ^kept value )
            )
            public class Holder from: x = Object new <: Keeper withBlock: [x * 3] () ()
        ) : ()
    "};
    let (mut interpreter, class, platform, host) = instantiated_world(source);
    let holder_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Holder"), &[]),
        "Holder",
    );
    let argument = interpreter.stash(Value::SmallInteger(5));
    let holder = expect_ok(
        interpreter.send(&holder_class, Symbol::intern("from:"), &[argument]),
        "Holder from:",
    );
    let bytes = snapshot_world(&mut interpreter, source, &class, &platform, &[holder]);

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    let answer = expect_ok(
        loaded.interpreter.send(&holder, Symbol::intern("run"), &[]),
        "run",
    );
    assert_eq!(integer_of(&mut loaded.interpreter, &answer), 15);

    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    assert!(
        report.contains("mixin application message of"),
        "inspect names the frame's kind:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// Mixin applications from hand-crafted bytes: every id, tag and position a
// level's description holds is checked before it is used.
// ---------------------------------------------------------------------------

/// The smallest module whose mixin table has a row with a `<:` clause.
const APPLYING_MODULE: &str = indoc! {"
    Newspeak3
    'T'
    class Host = Object new () (
        class Marker = Object new () ()
        class Applied = Object new <: Marker () ()
    ) : ()
"};

/// The row id of the declaration named `name` in `fixture`'s mixin table.
fn row_named(fixture: &GraphFixture, name: &str) -> u32 {
    fixture
        .mixins
        .iter()
        .position(|mixin| mixin.name.as_str() == name)
        .expect("the fixture declares it") as u32
}

/// One class record describing a level a `<:` clause added: it applies row
/// `mixin_row`'s mixin over the object-table record `superclass` (or no
/// superclass), and was added by clause `clause` of row `applying_row`. Its
/// metaclass is the record `metaclass_id`.
fn applied_level_record(
    writer: &mut encode::Writer,
    (metaclass_id, mixin_row): (u32, u32),
    superclass: Option<u32>,
    (applying_row, clause): (u32, u32),
) {
    writer.write_u8(CLASS_RECORD); // class tag: its metaclass record
    writer.write_u32(metaclass_id);
    writer.write_u32(7); // identity hash
    writer.write_u8(OBJECT_CLASS); // object body: Class
    writer.write_u8(MIXIN_ROW); // mixin reference: a mixin-table row
    writer.write_u32(mixin_row);
    match superclass {
        Some(id) => {
            writer.write_boolean(true);
            writer.write_u8(CLASS_RECORD); // class tag: object-table record
            writer.write_u32(id);
        }
        None => writer.write_boolean(false),
    }
    writer.write_boolean(false); // no enclosing object
    writer.write_boolean(false); // no declaring class
    writer.write_boolean(true); // an applied level
    writer.write_u32(applying_row);
    writer.write_u32(clause);
}

/// A graph section of exactly `records`, written by `write`, and no roots.
fn class_records(count: u32, write: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, count, 0);
    write(&mut writer);
    writer.write_u32(0); // no roots
    writer.into_bytes()
}

#[test]
fn a_well_formed_applied_level_record_decodes_with_its_clause() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    let bytes = class_records(2, |writer| {
        applied_level_record(writer, (1, marker), None, (applied, 0));
        metaclass_record(writer, 0);
    });
    fixture.decoded(&bytes, "an applied level", |heap, graph| {
        let clause = heap
            .class_applied_clause(graph.objects[0])
            .expect("the record's clause is attached to the level");
        assert_eq!(clause.selector, Symbol::NEW);
        assert_eq!(class_name(heap, graph.objects[0]), "Marker");
    });
}

#[test]
fn an_applied_levels_applying_row_out_of_range_is_a_decode_error_not_a_panic() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let bytes = class_records(2, |writer| {
        applied_level_record(writer, (1, marker), None, (99, 0));
        metaclass_record(writer, 0);
    });
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::BadMixinId(99)
    );
}

/// A level's clause is where its selector and message come from, so a
/// position past the applying row's clauses -- or any position in a row that
/// declares none -- names nothing to initialize it by.
#[test]
fn an_applied_levels_clause_its_row_does_not_declare_is_a_decode_error() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    for (row, index) in [(applied, 1), (marker, 0), (applied, u32::MAX)] {
        let bytes = class_records(2, |writer| {
            applied_level_record(writer, (1, marker), None, (row, index));
            metaclass_record(writer, 0);
        });
        let error = fixture.decode_error(&bytes);
        assert_eq!(
            error,
            decode::DecodeError::BadMixinApplicationClause { row, index }
        );
        assert_eq!(
            error.to_string(),
            format!(
                "the image names mixin application clause {index} of mixin row {row}, which the \
                 row does not declare in that place"
            )
        );
    }
}

/// A level's application is derived from the one below it, so levels whose
/// superclass links close on themselves -- directly, or through another
/// level -- have no order in which they could be rebuilt. The rebuild refuses
/// them rather than walking the cycle forever.
#[test]
fn applied_levels_inheriting_from_each_other_in_a_cycle_are_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    let through_another = class_records(4, |writer| {
        applied_level_record(writer, (2, marker), Some(1), (applied, 0));
        applied_level_record(writer, (3, applied), Some(0), (applied, 0));
        metaclass_record(writer, 0);
        metaclass_record(writer, 1);
    });
    let directly = class_records(2, |writer| {
        applied_level_record(writer, (1, marker), Some(0), (applied, 0));
        metaclass_record(writer, 0);
    });
    for bytes in [through_another, directly] {
        assert_eq!(
            fixture.decode_error(&bytes),
            decode::DecodeError::SuperclassCycle
        );
    }
}

#[test]
fn an_unknown_mixin_reference_tag_is_a_decode_error_not_a_panic() {
    let bytes = class_records(1, |writer| {
        writer.write_u8(CLASS_BUILTIN); // class tag: builtin
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        writer.write_u32(7); // identity hash
        writer.write_u8(OBJECT_CLASS); // object body: Class
        writer.write_u8(MIXIN_BUILTIN + 1); // one past the last mixin reference
        writer.write_u32(0);
        no_class_references(writer);
        writer.write_boolean(false);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "mixin reference",
            tag: MIXIN_BUILTIN + 1
        }
    );
}

/// A class record for a level applying the mixin of the builtin `kind_tag`
/// names, with no superclass, added by clause `index` of row `row`, and its
/// metaclass record. Such a record carries no enclosing object or declaring
/// class: both are the builtin's.
fn builtin_mixin_record(kind_tag: u8, (row, index): (u32, u32)) -> Vec<u8> {
    class_records(2, |writer| {
        writer.write_u8(CLASS_RECORD); // class tag: its metaclass record
        writer.write_u32(1);
        writer.write_u32(7); // identity hash
        writer.write_u8(OBJECT_CLASS); // object body: Class
        writer.write_u8(MIXIN_BUILTIN); // mixin reference: a builtin's
        writer.write_u8(kind_tag);
        writer.write_boolean(false); // no superclass
        writer.write_u32(row);
        writer.write_u32(index);
        metaclass_record(writer, 0);
    })
}

/// A `<:` clause naming a builtin gives its level that builtin's mixin, which
/// the record names by tag, and the builtin's own enclosing object and
/// declaring class -- the positive side of the refusals below.
#[test]
fn a_level_applying_a_builtins_mixin_decodes() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    let bytes = builtin_mixin_record(encode::builtin_class_tag(BuiltinClass::Value), (applied, 0));
    fixture.decoded(&bytes, "a level applying Value", |heap, graph| {
        let level = graph.objects[0];
        assert_eq!(class_name(heap, level), "Value");
        assert!(heap.class_applied_clause(level).is_some());
        assert!(
            heap.class_enclosing(level).is_some() && heap.class_declaring(level).is_some(),
            "the level is enclosed where Value is declared, in the loading bootstrap"
        );
    });
}

/// The clause of a level applying a builtin is checked like any other
/// level's: its applying row and its position there.
#[test]
fn a_builtin_levels_clause_is_checked_like_any_levels() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let value = encode::builtin_class_tag(BuiltinClass::Value);
    let applied = row_named(&fixture, "Applied");
    assert_eq!(
        fixture.decode_error(&builtin_mixin_record(value, (99, 0))),
        decode::DecodeError::BadMixinId(99)
    );
    assert_eq!(
        fixture.decode_error(&builtin_mixin_record(value, (applied, 3))),
        decode::DecodeError::BadMixinApplicationClause {
            row: applied,
            index: 3
        }
    );
}

/// The VM mirror's class, `Top`, `Activation` and `Future` are refused
/// wherever a program names a class, and a level's builtin mixin is such a place: a
/// level applying the mirror's mixin would hand its instances every
/// primitive.
#[test]
fn a_level_applying_the_mirrors_or_tops_mixin_is_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    for kind in [
        BuiltinClass::VmMirror,
        BuiltinClass::Top,
        BuiltinClass::Activation,
        BuiltinClass::Future,
    ] {
        let tag = encode::builtin_class_tag(kind);
        assert_eq!(
            fixture.decode_error(&builtin_mixin_record(tag, (applied, 0))),
            decode::DecodeError::InvalidTag {
                what: "builtin class a program may name",
                tag
            }
        );
    }
}

/// An object's class field is a place a program names a class too, so
/// `decode_class_tag`'s builtin arm refuses the VM mirror's class and `Top`
/// there as well: an object recorded as an instance of the mirror's class
/// would be a second mirror.
#[test]
fn an_objects_class_naming_the_mirrors_class_or_top_is_refused() {
    for kind in [BuiltinClass::VmMirror, BuiltinClass::Top] {
        let tag = encode::builtin_class_tag(kind);
        let bytes = class_records(1, |writer| {
            writer.write_u8(CLASS_BUILTIN); // class tag: builtin
            writer.write_u8(tag);
            writer.write_u32(5); // identity hash
            writer.write_u8(OBJECT_ARRAY); // object body: Array
            writer.write_u32(0); // no elements
        });
        assert_eq!(
            GraphFixture::new().decode_error(&bytes),
            decode::DecodeError::InvalidTag {
                what: "builtin class a program may name",
                tag
            }
        );
    }
}

/// One activation record, its header written by `header`, its body tag
/// `body` and frame id `frame`; `frames` minimal top-level frames follow it,
/// and no roots.
fn activation_record(
    header: impl FnOnce(&mut encode::Writer),
    body: u8,
    frame: u32,
    frames: u32,
) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, frames);
    header(&mut writer);
    writer.write_u32(5); // identity hash
    writer.write_u8(body);
    writer.write_u32(frame);
    for _ in 0..frames {
        writer.write_u8(VALUE_NIL); // receiver: nil
        writer.write_u32(0); // no arguments
        writer.write_u32(0); // no temporaries
        writer.write_u32(0); // program_counter
        writer.write_boolean(false); // no sender
        writer.write_boolean(false); // no home
        writer.write_boolean(false); // no current class
        writer.write_u8(FRAME_TOP_LEVEL);
        writer.write_boolean(true); // uncontinuable
    }
    writer.write_u32(0); // no roots
    writer.into_bytes()
}

/// The one header an activation record may carry: the builtin `Activation`.
fn activation_header(writer: &mut encode::Writer) {
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Activation));
}

/// A well-formed activation record decodes into an activation object over
/// the frame it names. The refusals below are this record with one thing
/// changed.
#[test]
fn an_activation_record_decodes_over_its_frame() {
    let bytes = activation_record(activation_header, OBJECT_ACTIVATION, 0, 1);
    GraphFixture::new().decoded(&bytes, "an activation record", |heap, graph| {
        let frame = heap
            .activation_object_frame(graph.objects[0])
            .expect("an activation object");
        assert!(frame.same(graph.frames[0]));
    });
}

/// An activation record's frame id is range-checked like every other: an id
/// past the frame table is a decode error, not a panic.
#[test]
fn an_activation_record_naming_no_frame_is_refused() {
    let bytes = activation_record(activation_header, OBJECT_ACTIVATION, 99, 0);
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadFrameId(99)
    );
}

/// An activation record under any class but `Activation` would be an
/// activation answering that class's methods; no live world has one.
#[test]
fn an_activation_record_under_another_class_is_refused() {
    let bytes = activation_record(
        |writer| {
            writer.write_u8(CLASS_BUILTIN); // class tag: builtin
            writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        },
        OBJECT_ACTIVATION,
        0,
        1,
    );
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::ActivationHeader);
    assert_eq!(
        error.to_string(),
        "an object in the image is an Activation without an activation's record, \
         or an activation's record under another class"
    );
}

/// The `Activation` header is accepted on an activation record only: on any
/// other record it would make an ordinary object answer `Activation`'s
/// methods, which read other frames and unwind the stack.
#[test]
fn an_activation_header_on_another_record_is_refused() {
    let bytes = class_records(1, |writer| {
        activation_header(writer);
        writer.write_u32(5); // identity hash
        writer.write_u8(OBJECT_ARRAY); // object body: Array
        writer.write_u32(0); // no elements
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::ActivationHeader
    );
}

/// Everywhere but an activation record's header, `Activation` is refused
/// like `Top`: as a value, and behind a metaclass reference.
#[test]
fn activation_named_anywhere_else_is_refused() {
    let tag = encode::builtin_class_tag(BuiltinClass::Activation);
    let refused = decode::DecodeError::InvalidTag {
        what: "builtin class a program may name",
        tag,
    };
    let as_value = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(tag);
    });
    assert_eq!(GraphFixture::new().decode_error(&as_value), refused);
    let as_metaclass = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(tag);
    });
    assert_eq!(GraphFixture::new().decode_error(&as_metaclass), refused);
}

/// Everywhere but a future record's header, `Future` is refused like
/// `Activation`: as a value, and behind a metaclass reference. Its class is
/// kernel-private, and no program names it (`unnameable_class`).
#[test]
fn a_graph_naming_future_as_a_class_is_refused() {
    let tag = encode::builtin_class_tag(BuiltinClass::Future);
    let refused = decode::DecodeError::InvalidTag {
        what: "builtin class a program may name",
        tag: 30,
    };
    let as_value = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(tag);
    });
    assert_eq!(GraphFixture::new().decode_error(&as_value), refused);
    let as_metaclass = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(tag);
    });
    assert_eq!(GraphFixture::new().decode_error(&as_metaclass), refused);
}

/// One record under the `Future` header, its body written by `body`, and no
/// roots.
fn future_record(body: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    class_records(1, |writer| {
        writer.write_u8(CLASS_BUILTIN); // class tag: builtin
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Future));
        writer.write_u32(5); // identity hash
        body(writer);
    })
}

/// The encoder writes only an unresolved future -- two fields, its closure
/// and state 0 -- so a record under the `Future` header that is anything
/// else describes a future no encoder wrote: another body, another field
/// count, a resolving or resolved state, a state that is none of the three,
/// or a computation that is not a closure. Each is refused at decode rather
/// than left for `future_state` to meet.
#[test]
fn a_future_record_that_is_not_an_unresolved_future_is_refused() {
    let array = future_record(|writer| {
        writer.write_u8(OBJECT_ARRAY);
        writer.write_u32(0); // no elements
    });
    let three_fields = future_record(|writer| {
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(3);
        writer.write_u8(VALUE_NIL);
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(0);
        writer.write_u8(VALUE_NIL);
    });
    let with_state = |computation: fn(&mut encode::Writer), state: i64| {
        future_record(move |writer| {
            writer.write_u8(OBJECT_INSTANCE);
            writer.write_u32(2);
            computation(writer);
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(state);
        })
    };
    let nil: fn(&mut encode::Writer) = |writer| writer.write_u8(VALUE_NIL);
    // The record itself, id 0: an object, but no closure.
    let itself: fn(&mut encode::Writer) = |writer| {
        writer.write_u8(VALUE_OBJECT_ID);
        writer.write_u32(0);
    };
    for (bytes, described) in [
        (array, "an array body"),
        (three_fields, "three fields"),
        (with_state(nil, 0), "an unresolved future of nil"),
        (with_state(itself, 0), "an unresolved future of itself"),
        (with_state(itself, 1), "a resolving future"),
        (with_state(nil, 2), "a resolved future"),
        (with_state(nil, 7), "state 7"),
    ] {
        let error = GraphFixture::new().decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::FutureRecord, "{described}");
    }
    assert_eq!(
        decode::DecodeError::FutureRecord.to_string(),
        "an object in the image is a Future without an unresolved future's record"
    );
}

/// A module whose `probe` slot holds §6.3.2's data cycle: each of `Probe`'s
/// arrays holds the other's future, resolved once the clause ends. `kept` is
/// for a test to fill, and `second` answers a block to make a future of.
const DATA_CYCLE: &str = indoc! {"
    Newspeak3
    'T'
    class Holder = Object new (
      | public probe = Probe new. public kept. |
    ) (
      class Probe = Object new (|| public first = {second}. public second = {first}. ||) () : ()
      public second = ( ^[probe second] )
    ) : ()
"};

/// Whether any object reachable from `value` through instance fields, array
/// elements and closure values is a `Future`. A graph is only what its roots
/// reach, so this covers every object an image decoded.
fn reaches_a_future<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> bool {
    let heap = mutator.heap();
    let future_class = mutator.builtin_class(BuiltinClass::Future);
    let mut seen: Vec<ns_runtime::ObjectReference<'gc>> = Vec::new();
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let Value::Object(object) = value else {
            continue;
        };
        if heap.is_class(object) || seen.iter().any(|visited| visited.same(object)) {
            continue;
        }
        seen.push(object);
        if heap.class_of_object(object).same(future_class) {
            return true;
        }
        if let Some(elements) = heap.array_elements(object) {
            pending.extend(elements);
        }
        if let Some(count) = heap.instance_field_count(object) {
            pending.extend((0..count).filter_map(|index| heap.instance_field(object, index)));
        }
        if let Some(closure) = heap.closure_data(object) {
            pending.extend(closure.copied.iter().copied());
        }
    }
    false
}

/// An image never holds a resolved future, only its value
/// (`docs/design/image-format.md`, "Futures in a graph"): the element of
/// `first` is written as `second`'s array itself, and so is `kept`'s, so
/// after a load they are one object and nothing in the loaded graph is a
/// future.
///
/// `kept`'s future is made and forced from the host and stored after the
/// last run, because a collection replaces every resolved future a field
/// holds with its value (`World::collect`): under `NS_NEXT_STRESS_COLLECT`
/// the clause's own futures are gone before the snapshot, and this one is
/// what keeps the test from passing without meeting one.
#[test]
fn a_resolved_future_is_written_as_its_value() {
    let (mut interpreter, class) = load(DATA_CYCLE);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let block = expect_ok(
        interpreter.send(&holder, Symbol::intern("second"), &[]),
        "second",
    );
    let future = interpreter.enter(|mutator| {
        let closure = *object(&mutator.fetch(&block));
        let future = mutator.new_future(closure);
        mutator.stash(Value::Object(future))
    });
    // A message to the future forces it; a host root keeps the future itself.
    expect_ok(
        interpreter.send(&future, Symbol::intern("size"), &[]),
        "future size",
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let holder = mutator.fetch(&holder);
        let future = mutator.fetch(&future);
        let resolved = mutator
            .as_future(future)
            .expect("the root holds the future");
        assert!(
            matches!(
                mutator.future_state(resolved),
                ns_runtime::interpreter::future::FutureState::Resolved(_)
            ),
            "the forced future is resolved"
        );
        let kept = mutator.new_array(vec![future]);
        assert!(
            mutator
                .heap_mut()
                .set_instance_field(*object(&holder), 1, Value::Object(kept)),
            "the holder has its kept slot"
        );
        assert!(reaches_a_future(mutator, holder), "the premise");
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(DATA_CYCLE),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[holder],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    let send = |interpreter: &mut Interpreter, receiver: &Root, selector: &str| {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), &[]),
            selector,
        )
    };
    let probe = send(&mut loaded.interpreter, &holder, "probe");
    let first = send(&mut loaded.interpreter, &probe, "first");
    let second = send(&mut loaded.interpreter, &probe, "second");
    let kept = send(&mut loaded.interpreter, &holder, "kept");
    loaded.interpreter.enter(|mutator| {
        assert!(
            !reaches_a_future(mutator, mutator.fetch(&holder)),
            "the loaded graph holds no future"
        );
        let heap = mutator.heap();
        let first = *object(&mutator.fetch(&first));
        let second = *object(&mutator.fetch(&second));
        let kept = *object(&mutator.fetch(&kept));
        let element = heap.array_at(first, 0).expect("first holds one element");
        assert!(
            object(&element).same(second),
            "first's element is second's array itself"
        );
        let back = heap.array_at(second, 0).expect("second holds one element");
        assert!(object(&back).same(first), "and second's is first's");
        let kept_element = heap.array_at(kept, 0).expect("kept holds one element");
        assert!(
            object(&kept_element).same(second),
            "kept's element is second's array itself"
        );
    });
}

/// An abandoned clause leaves `escaped` holding an unresolved future
/// (`spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch`). The
/// image writes it as a record under the `Future` header, with its closure,
/// and the loaded world forces it on the first touch, which runs the
/// initializer again. The first touch is an argument to a leaf, which sees
/// through it only once the loaded world knows it holds a future: the load
/// sets `futures_made` as `Mutator::new_future` does.
#[test]
fn an_unresolved_future_round_trips_and_computes_after_load() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder = Object new (
          | public escaped. |
        ) (
          class Probe keep: holder = Object new (
            || public first = {holder escaped: second. nil zork}. public second = 2 + 1. ||
          ) () : ()
          public abandon = ( Probe keep: self )
          public next = ( ^escaped + 1 )
          public sum = ( ^1 + escaped )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let abandoned = interpreter.send(&holder, Symbol::intern("abandon"), &[]);
    assert!(
        matches!(abandoned, Err(HostSignal::Error(_))),
        "first's initializer raises and abandons the clause"
    );
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let holder = mutator.fetch(&holder);
        let escaped = mutator
            .heap()
            .instance_field(*object(&holder), 0)
            .expect("the holder has its slot");
        let future = mutator
            .as_future(escaped)
            .expect("escaped holds second's future");
        assert!(
            matches!(
                mutator.future_state(future),
                ns_runtime::interpreter::future::FutureState::Unresolved(_)
            ),
            "the abandoned future is unresolved"
        );
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[holder],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let escaped = mutator
            .heap()
            .instance_field(*object(&mutator.fetch(&holder)), 0)
            .expect("the loaded holder has its slot");
        assert!(
            mutator.as_future(escaped).is_some(),
            "the loaded slot holds the future, not a value"
        );
    });
    let sum = expect_ok(
        loaded.interpreter.send(&holder, Symbol::intern("sum"), &[]),
        "sum",
    );
    let next = expect_ok(
        loaded
            .interpreter
            .send(&holder, Symbol::intern("next"), &[]),
        "next",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&sum), Value::SmallInteger(4)));
        assert!(matches!(mutator.fetch(&next), Value::SmallInteger(4)));
    });
}

/// Crafted code can corrupt a future's slots once it holds one; the encoder
/// refuses to write such a future rather than write a record the decoder
/// would refuse (`future::fields_for_an_image`), and does not see through a
/// "resolved" one whose value is another future.
#[test]
fn a_future_with_corrupted_fields_is_refused_by_the_encoder() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder = Object new (
          | public held |
        ) (
          public block = ( ^[3] )
        ) : ()
    "};
    for corruption in ["no closure", "state 7", "resolved to each other"] {
        let (mut interpreter, class) = load(source);
        let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
        let block = expect_ok(
            interpreter.send(&holder, Symbol::intern("block"), &[]),
            "block",
        );
        let platform = interpreter.platform();
        let refusal = interpreter.enter(|mutator| {
            let Value::Object(closure) = mutator.fetch(&block) else {
                panic!("block answers a block");
            };
            let holder = mutator.fetch(&holder);
            let future = mutator.new_future(closure);
            let other = mutator.new_future(closure);
            let heap = mutator.heap_mut();
            match corruption {
                "no closure" => {
                    heap.set_instance_field(future, 0, Value::SmallInteger(5));
                }
                "state 7" => {
                    heap.set_instance_field(future, 1, Value::SmallInteger(7));
                }
                _ => {
                    heap.set_instance_field(future, 0, Value::Object(other));
                    heap.set_instance_field(future, 1, Value::SmallInteger(2));
                    heap.set_instance_field(other, 0, Value::Object(future));
                    heap.set_instance_field(other, 1, Value::SmallInteger(2));
                }
            }
            heap.set_instance_field(*object(&holder), 0, Value::Object(future));
            let class = class_reference(mutator, &class);
            let platform = mutator.fetch(&platform);
            match image::snapshot(
                mutator,
                Some(source),
                &Origin::File(PathBuf::from("t.ns")),
                class,
                &platform,
                &[holder],
            ) {
                Ok(_) => panic!("{corruption}: a malformed future was written"),
                Err(message) => message,
            }
        });
        assert_eq!(
            refusal, "image encoding cannot serialize a malformed future",
            "{corruption}"
        );
    }
}

/// Tag 5 was the class record before its mixin became a tagged reference;
/// a stale record's bytes meet an unknown tag instead of being read in the
/// new layout.
#[test]
fn the_retired_class_record_tag_is_refused() {
    let bytes = class_records(1, |writer| {
        writer.write_u8(CLASS_BUILTIN); // class tag: builtin
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        writer.write_u32(7); // identity hash
        writer.write_u8(5); // the retired class-record tag
        writer.write_u32(0); // its bare mixin row id
        no_class_references(writer);
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "object body",
            tag: 5
        }
    );
}

#[test]
fn a_root_level_class_tag_past_the_roots_levels_is_a_decode_error_not_a_panic() {
    let bytes = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG); // value tag: class tag
        writer.write_u8(CLASS_ROOT_LEVEL); // class tag: a level of the root
        writer.write_u32(0); // EMPTY's root applies no mixin
    });
    let mut bytes = bytes;
    bytes.extend_from_slice(&0u32.to_le_bytes()); // no roots
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::BadRootLevel(0));
    assert_eq!(error.to_string(), "root class level 0 out of range");
}

/// A root level exists before the graph does, so the graph names it by its
/// position among the root's levels, and decodes that back to the very class
/// object the root-class section rebuilt.
#[test]
fn a_root_level_class_tag_names_the_root_classs_own_level() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let mut bytes = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG); // value tag: class tag
        writer.write_u8(CLASS_ROOT_LEVEL); // class tag: a level of the root
        writer.write_u32(0);
    });
    bytes.extend_from_slice(&0u32.to_le_bytes()); // no roots
                                                  // A decoded graph's handles cannot leave the entry that decoded it, so
                                                  // the element is compared with the root's level by a value both sides can
                                                  // answer: the identity hash, assigned here, before the decode, to the
                                                  // level the fixture's own root class sits on.
    let mut fixture = GraphFixture::with_source(source);
    let class = fixture.class.clone();
    let level_hash = fixture.interpreter.enter(|mutator| {
        let root = class_reference(mutator, &class);
        let level = mutator
            .heap()
            .class_superclass(root)
            .expect("the root's level");
        mutator.heap_mut().ensure_identity_hash(level)
    });
    fixture.decoded(&bytes, "a root level", |heap, graph| {
        let element = heap
            .array_elements(graph.objects[0])
            .expect("an array")
            .first()
            .copied()
            .expect("one element");
        let Value::Object(level) = element else {
            panic!("the element is a class object")
        };
        assert_eq!(heap.identity_hash(level), level_hash);
        assert!(heap.class_applied_clause(level).is_some());
    });
}

/// The frame kind of a `<:` clause message names the level the clause added
/// -- its body is that level's clause's -- and decodes back to that kind.
#[test]
fn the_mixin_application_message_frame_kind_decodes() {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_MIXIN_APPLICATION_MESSAGE);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    writer.write_boolean(false); // continuable
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    GraphFixture::new().decoded(
        &bytes,
        "a mixin application message frame",
        |heap, graph| {
            let frame = graph.frames.first().expect("the one frame");
            assert!(matches!(
                heap.frame_kind(*frame),
                FrameKind::MixinApplicationMessage(_)
            ));
        },
    );
}

/// The root-class section's bytes for a root that inherits `superclass` --
/// written as absent for `None`, which the reader refuses -- then `levels` as
/// (clause position, builtin tag) pairs.
fn root_section_bytes(superclass: Option<BuiltinClass>, levels: &[(u32, u8)]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    match superclass {
        Some(kind) => {
            writer.write_boolean(true);
            writer.write_u8(encode::builtin_class_tag(kind));
        }
        None => writer.write_boolean(false),
    }
    writer.write_u32(levels.len() as u32);
    for (index, tag) in levels {
        writer.write_u32(*index);
        writer.write_u8(*tag);
    }
    writer.into_bytes()
}

/// The root's levels are listed in clause order, at most one per clause, and
/// only for clauses row 0 declares -- which is also what bounds the count
/// before anything is reserved for it.
#[test]
fn a_root_level_naming_a_clause_out_of_place_is_refused() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let decoded = mixin_table_of(&snapshot_of(source));
    let value = encode::builtin_class_tag(BuiltinClass::Value);
    let read = |levels: &[(u32, u8)]| {
        decode::decode_root_section(
            &mut decode::Reader::new(&root_section_bytes(Some(BuiltinClass::Object), levels)),
            &decoded,
        )
    };
    assert_eq!(
        read(&[(0, value)]).expect("the one clause"),
        decode::RootSection {
            superclass: BuiltinClass::Object,
            levels: vec![(0, BuiltinClass::Value)],
        }
    );
    for (levels, index) in [(vec![(1, value)], 1), (vec![(0, value), (0, value)], 0)] {
        assert_eq!(
            read(&levels),
            Err(decode::DecodeError::BadMixinApplicationClause { row: 0, index })
        );
    }
    let mut huge = encode::Writer::new();
    huge.write_boolean(true);
    huge.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    huge.write_u32(u32::MAX); // a count no clause list could back
    assert_eq!(
        decode::decode_root_section(&mut decode::Reader::new(&huge.into_bytes()), &decoded),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    );
    let mirror = encode::builtin_class_tag(BuiltinClass::VmMirror);
    assert_eq!(
        read(&[(0, mirror)]),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class a program may name",
            tag: mirror
        })
    );
}

/// The root rule (`docs/design/object-model.md`, "The root rule"), read back:
/// a root section the encoder could never write is refused, so a crafted
/// artifact cannot bring back a root the rule forbids.
#[test]
fn a_root_section_the_root_rule_forbids_is_refused() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let decoded = mixin_table_of(&snapshot_of(source));
    let read = |superclass: Option<BuiltinClass>, levels: &[(u32, u8)]| {
        decode::decode_root_section(
            &mut decode::Reader::new(&root_section_bytes(superclass, levels)),
            &decoded,
        )
    };
    let tag = encode::builtin_class_tag;
    assert_eq!(
        read(Some(BuiltinClass::Array), &[]),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class a module's root may inherit",
            tag: tag(BuiltinClass::Array),
        })
    );
    assert_eq!(
        read(Some(BuiltinClass::Object), &[(0, tag(BuiltinClass::Array))]),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class a module's root may apply",
            tag: tag(BuiltinClass::Array),
        })
    );
    assert_eq!(
        read(Some(BuiltinClass::Value), &[(0, tag(BuiltinClass::Value))]),
        Err(decode::DecodeError::BadMixinApplicationClause { row: 0, index: 0 })
    );
    assert_eq!(
        read(None, &[(0, tag(BuiltinClass::Value))]),
        Err(decode::DecodeError::RootWithoutSuperclass)
    );
    assert_eq!(
        read(None, &[]),
        Err(decode::DecodeError::RootWithoutSuperclass)
    );
}

/// A root the rule forbids can never be instantiated, so neither artifact
/// kind is written for it, and both say why with the instantiation's text.
#[test]
fn the_encoder_refuses_a_root_the_rule_forbids() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Array new () ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let (snapshot, build) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let origin = Origin::File(PathBuf::from("t.ns"));
        (
            image::snapshot(mutator, None, &origin, class, &platform, &[]),
            image::build(
                mutator.heap(),
                None,
                &origin,
                class,
                Symbol::intern("main:args:"),
            ),
        )
    });
    let expected = "image encoding cannot serialize Root: a module's root class may inherit only \
                    Object or Value, or write Object <: Value, and its superclass clause names Array";
    assert_eq!(snapshot.err().as_deref(), Some(expected));
    assert_eq!(build.err().as_deref(), Some(expected));
}

/// `MIXIN_APPLICATION_MODULE`'s `Applied`, edited by `edit`, reloaded as a
/// mixin table: what `verify_mixins` says about it.
fn verify_doctored_applied(edit: impl FnOnce(Mixin) -> Mixin) -> Result<(), decode::DecodeError> {
    let (mut interpreter, class) = load(MIXIN_APPLICATION_MODULE);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_nested(mixin_of(mutator.heap(), class), "Applied", edit);
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        decode::verify_mixins(&decoded_mixins)
    })
}

/// §6.2 evaluates a `<:` clause's message with the applying declaration's
/// factory arguments, so its body must take exactly that many. The walk
/// re-checks it at run time as a Newspeak error; the load refuses an image
/// that breaks it outright, before anything runs.
#[test]
fn a_clause_message_whose_arity_disagrees_with_the_factory_is_refused_at_load() {
    let error = verify_doctored_applied(|applied| {
        let mut clauses = applied.mixin_applications.clone();
        let message = clauses[0].message_code.clone().expect("withMark: x * 10");
        clauses[0].message_code = Some(Rc::new(CompiledCode {
            argument_count: message.argument_count + 1,
            register_count: message.register_count + 1,
            ..(*message).clone()
        }));
        Mixin {
            mixin_applications: clauses,
            ..applied
        }
    })
    .expect_err("the message takes one argument too many");
    assert_eq!(
        error,
        decode::DecodeError::CodeArityMismatch {
            what: "mixin application clause 1 message".to_string(),
            parameters: 1,
            argument_count: 2,
        }
    );

    let wrong_kind = verify_doctored_applied(|applied| {
        let mut clauses = applied.mixin_applications.clone();
        clauses[0].message_code = applied.initializer_code.clone();
        Mixin {
            mixin_applications: clauses,
            ..applied
        }
    });
    assert_eq!(
        wrong_kind,
        Err(decode::DecodeError::WrongCodeKind {
            expected: "superclass clause message"
        })
    );
}

/// A clause whose message passes arguments in an image with no body for
/// them would panic on the first instantiation; the load refuses it.
#[test]
fn a_clause_message_with_arguments_and_no_body_is_refused_at_load() {
    let error = verify_doctored_applied(|applied| {
        let mut clauses = applied.mixin_applications.clone();
        clauses[0].message_code = None;
        Mixin {
            mixin_applications: clauses,
            ..applied
        }
    })
    .expect_err("no message body");
    assert_eq!(error, decode::DecodeError::MissingMixinApplicationMessage);
    assert_eq!(
        error.to_string(),
        "the image's mixin application clause passes factory arguments it has no compiled code for"
    );
}

/// A clause whose message passes no arguments has no body to run, so one
/// that names a body anyway -- here the other clause's, of the right kind
/// and arity, so nothing but the mismatch is wrong -- is refused rather than
/// carried as code the row says nothing runs.
#[test]
fn a_clause_message_without_arguments_that_carries_a_body_is_refused_at_load() {
    let error = verify_doctored_applied(|applied| {
        let mut clauses = applied.mixin_applications.clone();
        assert!(!clauses[1].has_arguments, "<: Stamp sends new");
        clauses[1].message_code = clauses[0].message_code.clone();
        Mixin {
            mixin_applications: clauses,
            ..applied
        }
    })
    .expect_err("a body for a message without arguments");
    assert_eq!(
        error,
        decode::DecodeError::UnexpectedMixinApplicationMessage
    );
    assert_eq!(
        error.to_string(),
        "the image's mixin application clause passes no factory arguments but carries compiled code for them"
    );
}

/// `<:` follows a superclass clause in the grammar, and it is the superclass
/// clause's body that evaluates every `<:` receiver; a row with clauses but
/// no superclass clause describes a declaration no source has.
#[test]
fn a_row_applying_mixins_without_a_superclass_clause_is_refused_at_load() {
    let error = verify_doctored_applied(|applied| Mixin {
        superclass: None,
        superclass_clause_code: None,
        superclass_message_code: None,
        ..applied
    })
    .expect_err("clauses with no superclass clause");
    assert_eq!(
        error,
        decode::DecodeError::MixinApplicationWithoutSuperclass
    );
}

/// A row's clause names its message body by code id, checked against the
/// code table like every other id the row names.
#[test]
fn a_clause_message_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one row
    writer.write_u32(0); // name
    writer.write_u8(mixins::access_tag(AccessModifier::Protected));
    writer.write_u32(0); // span.start
    writer.write_u32(0); // span.end
    writer.write_u32(0); // no slots
    writer.write_boolean(false); // no factory
    writer.write_boolean(false); // no initExprs
    writer.write_boolean(false); // no superclass clause
    writer.write_u32(1); // one mixin application
    writer.write_u32(0); // its selector
    writer.write_boolean(true); // its message takes arguments
    writer.write_boolean(true); // ... compiled as
    writer.write_u32(5); // ... code 5, of an empty code table
    let table = table_with(&["X"]);
    assert_eq!(
        mixins::decode(&mut decode::Reader::new(&writer.into_bytes()), &table, &[]).map(|_| ()),
        Err(decode::DecodeError::BadCodeId(5))
    );
}

/// `inspect` shows everything the format now says about a mixin
/// application: each row's clauses and their message bodies, which clause
/// added a level, and the root's own levels.
#[test]
fn inspect_prints_the_mixin_applications_an_image_describes() {
    let (mut interpreter, class, platform, host) = instantiated_world(MIXIN_APPLICATION_MODULE);
    let expected = (5, 40, "Applied Stamp Marker Base".to_string());
    assert_eq!(applied_answers(&mut interpreter, &host, 4), expected);
    let bytes = snapshot_world(
        &mut interpreter,
        MIXIN_APPLICATION_MODULE,
        &class,
        &platform,
        &[host],
    );
    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    for expected in [
        "mixin application 1  withMark: -> code ",
        "mixin application 2  new\n",
        "Applied  mixin application clause 1 message",
        "applied by clause 0 of row ",
        "applied by clause 1 of row ",
    ] {
        assert!(
            report.contains(expected),
            "inspect prints `{expected}`:\n{report}"
        );
    }

    let source = indoc! {"
        Newspeak3
        'T'
        class Root = Object new <: Value () ()
    "};
    let report =
        image::inspect::render(&image::inspect::inspect(&snapshot_of(source)).expect("inspect"));
    assert!(
        report.contains("superclass Object, applies Value (clause 0)"),
        "inspect prints the root's levels:\n{report}"
    );
}

/// A nested class's `<:` clause can name a builtin too, and its level is then
/// a class record whose mixin no row describes: the record names it by
/// builtin tag, and the reloaded level applies the loading bootstrap's own
/// builtin mixin.
#[test]
fn a_nested_level_applying_a_builtin_mixin_round_trips_as_a_record() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Host usingPlatform: p = Object <: Value () (
            public class Point = Object new <: Value () (
                public answer = ( ^7 )
            )
        ) : ()
    "};
    let (mut interpreter, class, platform, host) = instantiated_world(source);
    let point_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Point"), &[]),
        "Point",
    );
    let point = expect_ok(interpreter.instantiate(&point_class, &[]), "Point new");
    let bytes = snapshot_world(&mut interpreter, source, &class, &platform, &[point]);

    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    assert!(
        report.contains("class mixin builtin Value"),
        "the level's record names Value's mixin by tag:\n{report}"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let point = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let point = mutator.fetch(&point);
        let heap = mutator.heap();
        let level = heap
            .class_superclass(class_of(heap, &point))
            .expect("Point's level");
        let value = mutator.builtin_class(BuiltinClass::Value);
        assert!(Rc::ptr_eq(
            &application_of(heap, level).mixin,
            &application_of(heap, value).mixin
        ));
        assert!(heap.class_applied_clause(level).is_some());
    });
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&point, Symbol::intern("answer"), &[]),
        "answer",
    );
    assert_eq!(integer_of(&mut loaded.interpreter, &answer), 7);
}

/// §3.1.1's enclosing-object condition on a value class is decided when the
/// class object is created and only *recorded* on it
/// (`ClassData::value_class_refusal`, `docs/design/object-model.md`, "A
/// value object's four conditions", "Enclosing objects are values");
/// `decide_instantiation` is what refuses, at instantiation. A decoded class
/// object starts with no verdict of its own, so a legitimate snapshot
/// holding a class that was created carrying one has to have it recomputed
/// on load (`image::decode::rebuild_applications`) or the round trip would
/// silently make an already-refused class instantiable.
///
/// `Counter` inherits `Value` directly but is enclosed by a fresh `Holder`
/// each time `MutableCounter` builds one, so it is a refused value class
/// from the moment it is created -- `goldens/kernel/a_value_subclass_of_a_
/// refused_value_class_is_refused.ns` pins the same shape as a golden.
/// `Stamped` inherits `Value` only through `Counter`, with no `<:` clause of
/// its own, so its own refusal is *inherited* from `Counter`'s recorded
/// verdict by the walk in `interpreter::value_class::check_value_class` --
/// exactly the case that has to survive the round trip in the right order,
/// superclass first, for `Stamped`'s recomputed verdict to name `Counter`
/// rather than trusting it blindly.
#[test]
fn a_refused_value_class_stays_refused_after_a_snapshot_round_trip() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class RefusedValueClassModule usingPlatform: platformAtCreation = Object <: Value () (
            public class Holder = Object new (
                | public count ::= 0. |
            ) (
                public class Counter = Value new () (
                    public bump = ( count:: count + 1. ^count )
                )
            )
            public MutableCounter = ( ^Holder new Counter )
            public class Stamped = MutableCounter new () ()
        ) : ()
    "};
    let (mut interpreter, class, platform, module) = instantiated_world(source);
    let stamped_class = expect_ok(
        interpreter.send(&module, Symbol::intern("Stamped"), &[]),
        "Stamped",
    );
    // The shape the bug depends on, confirmed before any round trip:
    // creating `Stamped` succeeds -- the redesign only records a refusal --
    // and only instantiating it is refused.
    let message = expect_error(interpreter.instantiate(&stamped_class, &[]), "Stamped new");
    assert_eq!(
        message,
        "cannot instantiate Stamped: a value class's enclosing objects must be values, and \
         Counter's enclosing object, an instance of Holder, is not one"
    );

    let bytes = snapshot_world(
        &mut interpreter,
        source,
        &class,
        &platform,
        std::slice::from_ref(&stamped_class),
    );
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_stamped = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Stamped");
    let message = expect_error(
        loaded.interpreter.instantiate(&reloaded_stamped, &[]),
        "Stamped new",
    );
    assert_eq!(
        message,
        "cannot instantiate Stamped: a value class's enclosing objects must be values, and \
         Counter's enclosing object, an instance of Holder, is not one"
    );
}

/// The no-identity-method condition is decided once per *application*
/// (`Mutator::apply_mixin`, folded into `MixinApplication::instantiation_refusal`),
/// not per class object -- unlike the enclosing-object condition, it needs no
/// `after_clause`, which a module's root never reaches
/// (`load_root_class` calls `apply_mixin` directly). `snapshot_of` proves the
/// case the fix depends on: a value module whose root defines `==` snapshots
/// with no instance ever made, since `load` builds only the class, never an
/// instance of it -- so the round trip has to be what catches a root that
/// slipped past every other check
/// (`goldens/kernel/a_value_module_defining_identity_is_refused.ns` pins the
/// same shape as a golden).
#[test]
fn a_root_defining_identity_stays_refused_after_a_snapshot_round_trip() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class ValueModuleDefiningIdentity usingPlatform: platformAtCreation = Object <: Value () (
            public == other = ( ^false )
        ) : ()
    "};
    let bytes = snapshot_of(source);
    let mut loaded = image::load(&bytes).expect("load");
    let platform = loaded.interpreter.platform();
    let message = expect_error(
        loaded
            .interpreter
            .instantiate(&loaded.root_class, std::slice::from_ref(&platform)),
        "ValueModuleDefiningIdentity new",
    );
    assert_eq!(
        message,
        "cannot instantiate ValueModuleDefiningIdentity: a value class may not define ==, and \
         ValueModuleDefiningIdentity defines it"
    );
}

// ---------------------------------------------------------------------------
// Platform paths: an object the image reaches through the platform is named
// by the public members that lead to it, and resolved on load against the
// loading interpreter's own platform (`docs/design/image-format.md`, "The
// platform path section").
// ---------------------------------------------------------------------------

/// A module whose root keeps two platform members in slots of its own. Its
/// one method is never run: it is there so that the selectors the crafted
/// path sections below name are in the image's symbol table.
const PLATFORM_MEMBERS: &str = indoc! {"
    Newspeak3
    'T'
    class Holder usingPlatform: platform = Object new (
        | public transcript = platform transcript.
          public collections = platform collections. |
    ) (
        public unused = (
            transcript newLine.
            collections newCollections.
            ^transcript show: transcript nonexistent printString
        )
    ) : ()
"};

/// The id `text` has in the image's symbol table, as `inspect` lists it.
fn symbol_id(report: &image::inspect::Report, text: &str) -> u32 {
    let position = report
        .symbols
        .iter()
        .position(|symbol| symbol == text)
        .unwrap_or_else(|| panic!("the image interns {text}"));
    u32::try_from(position).expect("a symbol id fits a u32")
}

/// `bytes` with its platform path section replaced by `section`.
fn with_platform_path_section(bytes: &[u8], section: &[u8]) -> Vec<u8> {
    let report = image::inspect::inspect(bytes).expect("inspect");
    let located = report
        .sections
        .iter()
        .find(|candidate| candidate.name == "platform paths")
        .expect("a stateful image carries a platform path section");
    let mut crafted = bytes[..located.start].to_vec();
    crafted.extend_from_slice(section);
    crafted.extend_from_slice(&bytes[located.end..]);
    crafted
}

/// A platform path section naming `paths`, each a list of symbol ids.
fn platform_path_section(paths: &[Vec<u32>]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::try_from(paths.len()).expect("a count fits a u32"));
    for path in paths {
        writer.write_u32(u32::try_from(path.len()).expect("a length fits a u32"));
        for member in path {
            writer.write_u32(*member);
        }
    }
    writer.into_bytes()
}

fn same_object(loaded: &mut image::LoadedImage, first: &Root, second: &Root) -> bool {
    loaded
        .interpreter
        .enter(|mutator| object(&mutator.fetch(first)).same(*object(&mutator.fetch(second))))
}

#[test]
fn a_platform_member_is_named_by_its_path() {
    let bytes = snapshot_module(PLATFORM_MEMBERS, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.platform_paths,
        vec!["transcript".to_string(), "collections".to_string()],
        "the paths the graph uses, in the order it first reaches them"
    );
    assert!(
        report.graph_objects.iter().all(|record| {
            !record.starts_with("class Transcript") && !record.starts_with("class Collections")
        }),
        "neither member is written as a record: {:#?}",
        report.graph_objects
    );

    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    for member in ["transcript", "collections"] {
        let stored = send_to(&mut loaded, &instance, member);
        let fresh = send_to(&mut loaded, &platform, member);
        assert!(
            same_object(&mut loaded, &stored, &fresh),
            "the stored {member} is the loading platform's own"
        );
    }
}

#[test]
fn a_stored_platform_and_a_stored_member_agree_after_a_load() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public keptPlatform = platform.
              public collections = platform collections. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let kept = send_to(&mut loaded, &instance, "keptPlatform");
    let through_the_platform = send_to(&mut loaded, &kept, "collections");
    let stored = send_to(&mut loaded, &instance, "collections");
    assert!(
        same_object(&mut loaded, &through_the_platform, &stored),
        "a stored platform and a stored member answer one collections module"
    );
}

/// The platform hands out its module's classes by class accessors
/// (`public Transcript = ( ^outer PlatformFactory Transcript )`), and the
/// collections facet its module's the same way, so a class a program got
/// from one is named by the accessor's path, and the loading platform
/// answers its own class for it.
#[test]
fn a_class_the_platform_hands_out_is_named_by_its_accessor() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public transcriptClass = platform Transcript.
              public platformClass = platform class.
              public collectionsClass = platform collections Collections.
              public listClass = platform collections List.
              public kernelNamespaceClass = platform KernelNamespace. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.platform_paths,
        vec![
            "Transcript".to_string(),
            "Platform".to_string(),
            "collections Collections".to_string(),
            "collections List".to_string(),
            "KernelNamespace".to_string()
        ],
        "the paths the graph uses, in the order it first reaches them"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    for (member, answerer, accessor) in [
        ("transcriptClass", &platform, "Transcript"),
        ("platformClass", &platform, "Platform"),
        ("collectionsClass", &collections, "Collections"),
        ("listClass", &collections, "List"),
        ("kernelNamespaceClass", &platform, "KernelNamespace"),
    ] {
        let stored = send_to(&mut loaded, &instance, member);
        let fresh = send_to(&mut loaded, answerer, accessor);
        assert!(
            same_object(&mut loaded, &stored, &fresh),
            "the stored {member} is the loading platform's {accessor}"
        );
    }
}

/// A second platform is an ordinary instance of a shipped class: written as
/// a record of the class the path `Platform` names, with its own lazy
/// slots, and it writes after a load as it did before.
#[test]
fn a_second_platform_is_a_record_of_its_path_named_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public second = platform class new. |
            second transcript.
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report.platform_paths.contains(&"Platform".to_string()),
        "the second platform's class is named by its path: {:?}",
        report.platform_paths
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    let second = send_to(&mut loaded, &instance, "second");
    assert!(
        !same_object(&mut loaded, &second, &platform),
        "the second platform is its own record, not the loading platform"
    );
    let second_class = send_to(&mut loaded, &second, "class");
    let platform_class = send_to(&mut loaded, &platform, "class");
    assert!(
        same_object(&mut loaded, &second_class, &platform_class),
        "the second platform is an instance of the loading platform's class"
    );
    let transcript = send_to(&mut loaded, &second, "transcript");
    let text = loaded.interpreter.enter(|mutator| {
        let text = Value::Object(mutator.new_string(String::new()));
        mutator.stash(text)
    });
    expect_ok(
        loaded
            .interpreter
            .send(&transcript, Symbol::intern("show:"), &[text]),
        "second transcript show: ''",
    );
}

/// A `List` is an ordinary instance of a shipped class: written as a record
/// of the class the path `collections List` names, with its `size_` and its
/// `data` array, and it answers as before after a load -- the class and its
/// code are the loading binary's.
#[test]
fn a_list_is_a_record_of_its_path_named_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public list = platform collections List new. |
            list add: 3.
            list add: 4.
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .platform_paths
            .contains(&"collections List".to_string()),
        "the list's class is named by its path: {:?}",
        report.platform_paths
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let list = send_to(&mut loaded, &instance, "list");
    let size = send_to(&mut loaded, &list, "size");
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&size), Value::SmallInteger(2)));
    });
    let two = loaded.interpreter.stash(Value::SmallInteger(2));
    let at_two = expect_ok(
        loaded
            .interpreter
            .send(&list, Symbol::intern("at:"), &[two]),
        "list at: 2",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&at_two), Value::SmallInteger(4)));
    });
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_class = send_to(&mut loaded, &collections, "List");
    let stored_class = send_to(&mut loaded, &list, "class");
    assert!(
        same_object(&mut loaded, &stored_class, &fresh_class),
        "the loaded list is an instance of the loading platform's List"
    );
}

/// A facet a program makes itself from `platform collections class` is an
/// ordinary record of the class the path `collections Collections` names,
/// and after a load it is equal to the loading platform's facet: a value.
#[test]
fn a_second_collections_facet_is_a_record_of_its_path_named_class() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Holder usingPlatform: platform = Object new (
            | public facet = platform collections class new. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .platform_paths
            .contains(&"collections Collections".to_string()),
        "the facet's class is named by its path: {:?}",
        report.platform_paths
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let facet = send_to(&mut loaded, &instance, "facet");
    let list = send_to(&mut loaded, &facet, "List");
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_list = send_to(&mut loaded, &collections, "List");
    assert!(
        same_object(&mut loaded, &list, &fresh_list),
        "the loaded facet hands out the loading platform's List"
    );
    let equal = expect_ok(
        loaded
            .interpreter
            .send(&facet, Symbol::intern("="), &[collections]),
        "facet = platform collections",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            matches!(mutator.fetch(&equal), Value::Boolean(true)),
            "the loaded facet equals the loading platform's facet"
        );
    });
}

#[test]
fn a_platform_path_the_loader_cannot_follow_is_refused() {
    let bytes = snapshot_module(PLATFORM_MEMBERS, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let transcript = symbol_id(&report, "transcript");
    let nonexistent = symbol_id(&report, "nonexistent");
    let show = symbol_id(&report, "show:");
    let print_string = symbol_id(&report, "printString");
    let new_line = symbol_id(&report, "newLine");
    let no_symbol = u32::try_from(report.symbols.len()).expect("a count fits a u32") + 1000;
    let too_long = vec![transcript; image::platform_paths::MAX_PLATFORM_PATH_LENGTH + 1];

    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![nonexistent]]),
    ));
    assert!(message.contains("nonexistent"), "{message}");

    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![show]]),
    ));
    assert!(message.contains("show:"), "{message}");

    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[too_long]),
    ));
    assert_eq!(
        message,
        format!(
            "a platform path in the image has {} members, but a path has between 1 and {} members",
            image::platform_paths::MAX_PLATFORM_PATH_LENGTH + 1,
            image::platform_paths::MAX_PLATFORM_PATH_LENGTH
        )
    );

    let mut too_many = encode::Writer::new();
    too_many.write_u32(
        u32::try_from(image::platform_paths::MAX_PLATFORM_PATHS + 1).expect("fits a u32"),
    );
    let message = load_error(&with_platform_path_section(&bytes, &too_many.into_bytes()));
    assert_eq!(
        message,
        format!(
            "the image lists {} platform paths, more than the {} a loader follows",
            image::platform_paths::MAX_PLATFORM_PATHS + 1,
            image::platform_paths::MAX_PLATFORM_PATHS
        )
    );

    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![no_symbol]]),
    ));
    assert!(message.contains("symbol id"), "{message}");

    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![transcript, print_string]]),
    ));
    assert!(message.contains("transcript printString"), "{message}");

    // A public method that is no slot's getter, no nested class's accessor
    // and no class accessor is never sent: `newLine` would print while the
    // image is read.
    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![transcript, new_line]]),
    ));
    assert_eq!(
        message,
        "the image names platform path `transcript newLine`, whose member `newLine` is no \
         public slot, nested class or class accessor of what the path has reached"
    );

    // `newCollections` is the factory's member, not the facet's, so it is
    // no step: a path can never reach the collections factory.
    let collections = symbol_id(&report, "collections");
    let new_collections = symbol_id(&report, "newCollections");
    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![collections, new_collections]]),
    ));
    assert_eq!(
        message,
        "the image names platform path `collections newCollections`, whose member \
         `newCollections` is no public slot, nested class or class accessor of what the path \
         has reached"
    );
}

#[test]
fn an_index_past_the_path_section_is_refused() {
    // The graph names `collections` as path 1; a section listing only
    // `transcript` leaves that index past its end.
    let bytes = snapshot_module(PLATFORM_MEMBERS, None, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let transcript = symbol_id(&report, "transcript");
    let message = load_error(&with_platform_path_section(
        &bytes,
        &platform_path_section(&[vec![transcript]]),
    ));
    assert_eq!(
        message,
        decode::DecodeError::BadPlatformPath(1).to_string(),
        "{message}"
    );
}

/// A one-object graph section: an instance record whose header names
/// platform path 0, carrying `fields` `nil` fields.
fn one_instance_of_platform_path_zero(fields: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    writer.write_u8(CLASS_PLATFORM_PATH);
    writer.write_u32(0);
    writer.write_u32(5); // identity hash
    writer.write_u8(OBJECT_INSTANCE);
    writer.write_u32(fields);
    for _ in 0..fields {
        writer.write_u8(VALUE_NIL);
    }
    writer.write_u32(0); // no roots
    writer.into_bytes()
}

#[test]
fn an_instance_of_a_path_named_class_carries_exactly_its_layout() {
    // The layout comes from the loading binary: `Transcript` declares no
    // slots, so an instance of it carries none.
    let mut fixture = fixture_naming_the_transcript_class_by_path();
    fixture.decode(&one_instance_of_platform_path_zero(0), |_, decoded| {
        assert!(decoded.is_ok(), "an instance that fits the layout decodes");
    });
    assert_eq!(
        fixture.decode_error(&one_instance_of_platform_path_zero(1)),
        decode::DecodeError::PlatformInstanceFieldCount {
            expected: 0,
            found: 1
        }
    );
}

#[test]
fn a_class_reference_to_a_path_that_is_no_class_is_refused() {
    let mut fixture = GraphFixture::new();
    let platform = fixture.platform.clone();
    let transcript = expect_ok(
        fixture
            .interpreter
            .send(&platform, Symbol::intern("transcript"), &[]),
        "transcript",
    );
    fixture.platform_paths = vec![transcript];
    assert_eq!(
        fixture.decode_error(&one_instance_of_platform_path_zero(0)),
        decode::DecodeError::PlatformPathNotAClass
    );
}

/// A module instance holds the core library's state, as the platform holds
/// the mirror, and a class compiled from shipped source belongs to the
/// loading binary: the encoder writes neither, and refuses one no platform
/// path names. The kernel instance and its class are both; the collections
/// factory is the second such module, reached here only because a host can
/// walk a class's enclosing object, which no program can.
#[test]
fn a_shipped_object_no_platform_path_names_is_refused() {
    let (mut interpreter, class) = load(EMPTY);
    let platform = interpreter.platform();
    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let collections_class = expect_ok(
        interpreter.send(&collections, Symbol::intern("class"), &[]),
        "collections class",
    );
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let object = mutator.builtin_class(BuiltinClass::Object);
        let kernel = mutator
            .heap()
            .class_enclosing(object)
            .expect("the kernel instance encloses Object");
        let kernel_class = mutator.heap().class_of_object(kernel);
        let collections_class = class_reference(mutator, &collections_class);
        let collections_factory = mutator
            .heap()
            .class_enclosing(collections_class)
            .expect("the collections factory instance encloses the facet class");
        let collections_factory_class = mutator.heap().class_of_object(collections_factory);
        for (shipped, expected) in [
            (
                kernel,
                "image encoding refuses a module instance of Kernel: it holds the core \
                 library's state, and no platform path names it",
            ),
            (
                kernel_class,
                "image encoding refuses the class object Kernel: it is compiled from shipped \
                 source, and no platform path names it",
            ),
            (
                collections_factory,
                "image encoding refuses a module instance of CollectionsFactory: it holds the \
                 core library's state, and no platform path names it",
            ),
            (
                collections_factory_class,
                "image encoding refuses the class object CollectionsFactory: it is compiled \
                 from shipped source, and no platform path names it",
            ),
        ] {
            let refused = image::snapshot(
                mutator,
                None,
                &Origin::File(PathBuf::from("t.ns")),
                root_class,
                &platform,
                &[Value::Object(shipped)],
            );
            let Err(message) = refused else {
                panic!("a graph reaching {expected} was encoded");
            };
            assert_eq!(message, expected);
        }
    });
}

/// A graph section of no objects and no frames whose one root `root` writes.
fn a_graph_of_one_root(root: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 0, 0);
    writer.write_u32(1);
    root(&mut writer);
    writer.into_bytes()
}

/// A fixture whose platform path 0 answered the platform's `Transcript`
/// class, `PlatformFactory.ns`'s nested class, as the path `Transcript` --
/// the platform's class accessor -- does.
fn fixture_naming_the_transcript_class_by_path() -> GraphFixture {
    let mut fixture = GraphFixture::new();
    let platform = fixture.platform.clone();
    let transcript_class = expect_ok(
        fixture
            .interpreter
            .send(&platform, Symbol::intern("Transcript"), &[]),
        "platform Transcript",
    );
    fixture.platform_paths = vec![transcript_class];
    fixture
}

#[test]
fn a_value_naming_a_path_named_class_decodes_to_that_class() {
    let mut fixture = fixture_naming_the_transcript_class_by_path();
    let expected = fixture.platform_paths[0].clone();
    let bytes = a_graph_of_one_root(|writer| {
        writer.write_u8(VALUE_PLATFORM_PATH);
        writer.write_u32(0);
    });
    let same = fixture.decode(&bytes, |heap, decoded| {
        let Ok(graph) = decoded else {
            panic!("the graph decodes");
        };
        object(&graph.roots[0]).same(*object(&heap.fetch(&expected)))
    });
    assert!(same, "the root is the class the path answered");
}

#[test]
fn a_metaclass_reference_through_a_platform_path_decodes() {
    let mut fixture = fixture_naming_the_transcript_class_by_path();
    let class = fixture.platform_paths[0].clone();
    let bytes = a_graph_of_one_root(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(CLASS_METACLASS);
        writer.write_u8(CLASS_PLATFORM_PATH);
        writer.write_u32(0);
    });
    let same = fixture.decode(&bytes, |heap, decoded| {
        let Ok(graph) = decoded else {
            panic!("the graph decodes");
        };
        let metaclass = heap
            .class_metaclass(*object(&heap.fetch(&class)))
            .expect("Transcript has a metaclass");
        object(&graph.roots[0]).same(metaclass)
    });
    assert!(
        same,
        "the root is the metaclass of the class the path answered"
    );
}

/// The closing hash table, decoded against a fresh world whose platform path
/// 0 answered its transcript; answers the outcome and the transcript's hash.
fn decode_tag_named_with_the_transcript(bytes: &[u8]) -> (Result<u32, decode::DecodeError>, u32) {
    let (mut interpreter, class) = load(EMPTY);
    let platform = interpreter.platform();
    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "transcript",
    );
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let transcript = mutator.fetch(&transcript);
        let mut reader = decode::Reader::new(bytes);
        let result = decode::decode_tag_named_hashes(&mut reader, mutator, class, &[transcript]);
        (result, mutator.heap().identity_hash(*object(&transcript)))
    })
}

#[test]
fn a_hash_recorded_for_a_platform_path_is_restored() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(HASHED_PLATFORM_PATH);
    writer.write_u32(0);
    writer.write_u32(7);
    let (result, hash) = decode_tag_named_with_the_transcript(&writer.into_bytes());
    assert_eq!(result, Ok(7));
    assert_eq!(
        hash, 7,
        "the loading platform's transcript carries the hash"
    );
}

#[test]
fn a_hash_recorded_for_a_platform_path_past_the_section_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(HASHED_PLATFORM_PATH);
    writer.write_u32(1);
    writer.write_u32(7);
    let (result, _) = decode_tag_named_with_the_transcript(&writer.into_bytes());
    assert_eq!(result, Err(decode::DecodeError::BadPlatformPath(1)));
}

/// An object the closing hash table reaches twice is listed once, or the
/// loader would refuse the table as naming one object twice. No platform
/// path answers a builtin class today; the table must still list one once if
/// a path ever does, so this hands the encoder `Array` as a path-named
/// object: the class is reached both as a builtin and by its path -- it
/// keeps its builtin entry -- and its metaclass both as a builtin's
/// metaclass and as the metaclass of a path-named class -- it keeps the
/// builtin one's.
#[test]
fn an_object_reached_twice_is_listed_once_in_the_hash_table() {
    let (mut interpreter, class) = load(EMPTY);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let array = mutator.builtin_class(BuiltinClass::Array);
        let metaclass = mutator
            .heap()
            .class_metaclass(array)
            .expect("Array has a metaclass");
        let class_hash = mutator.heap_mut().ensure_identity_hash(array);
        let metaclass_hash = mutator.heap_mut().ensure_identity_hash(metaclass);
        let mut writer = encode::Writer::new();
        encode::encode_tag_named_hashes(&mut writer, mutator, class, &platform, &[array]);
        let bytes = writer.into_bytes();
        let entries = u32::from_le_bytes(bytes[..4].try_into().expect("a count"));
        let mut reader = decode::Reader::new(&bytes);
        let decoded =
            decode::decode_tag_named_hashes(&mut reader, mutator, class, &[Value::Object(array)]);
        assert_eq!(decoded, Ok(class_hash.max(metaclass_hash)));
        assert_eq!(entries, 2, "one entry for the class, one for its metaclass");
    });
}
