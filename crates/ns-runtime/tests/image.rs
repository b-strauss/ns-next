//! Image format round-trip tests, section by section: the header and its
//! scalars, the mixin table, the code table, the object and frame tables,
//! whole-image `snapshot`/`load`, and the two verification passes.

use indoc::{formatdoc, indoc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{CallSite, CodeKind, CompiledCode, Instruction, Literal};
use ns_runtime::image;
use ns_runtime::image::opcodes::*;
use ns_runtime::image::{decode, encode, mixins, symbols};
use ns_runtime::SourceOrigin as Origin;
use ns_runtime::{
    BuiltinClass, ClassData, ClassReference, FrameKind, Heap, HostResult, HostSignal, Interpreter,
    Mixin, MixinApplication, Mutator, ObjectData, Root, SourceOrigin, Symbol, Value,
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

/// `write_mixin_row` with `slots` as `(name symbol id, mutability tag,
/// access tag)` records.
fn write_mixin_row_with_slots(
    writer: &mut encode::Writer,
    name_id: u32,
    slots: &[(u32, u8, u8)],
    methods: &[(u32, u32)],
    nested: &[u32],
) {
    writer.write_u32(name_id);
    writer.write_u8(mixins::access_tag(AccessModifier::Protected)); // AccessModifier::Protected
    writer.write_u32(0); // span.start
    writer.write_u32(0); // span.end
    writer.write_u32(slots.len() as u32);
    for (name, mutability, access) in slots {
        writer.write_u32(*name);
        writer.write_u8(*mutability);
        writer.write_u8(*access);
    }
    writer.write_boolean(false); // no factory
    writer.write_boolean(false); // no initExprs
    writer.write_boolean(false); // no superclass clause
    writer.write_u32(methods.len() as u32);
    for (selector, code) in methods {
        writer.write_u32(*selector);
        writer.write_u8(mixins::access_tag(AccessModifier::Protected)); // AccessModifier::Protected
        writer.write_u16(0); // argument_count
        writer.write_u32(*code);
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
      q := p.
    ) (
      find: t in: c = ( c do: [:e | e = t ifTrue: [^e]]. ^nil )
      loop = ( | n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n )
      arr = ( ^{ 1. #(2 3). 'four' } )
    ) : ()
"};

fn every_instruction_once() -> CompiledCode {
    let block = Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instructions: vec![Instruction::NonLocalReturn { register: 0 }],
        argument_count: 1,
        register_count: 1,
        statement_program_counter: vec![(0, 0..1)],
        call_sites: vec![],
        source: Span::new(1, 2),
    });
    let call_site = CallSite { call_site_id: 7 };
    CompiledCode {
        kind: CodeKind::Initializer,
        instructions: vec![
            Instruction::LoadLiteral {
                destination: 0,
                literal: Literal::Array(vec![
                    Literal::Nil,
                    Literal::Boolean(true),
                    Literal::SmallInteger(-5),
                    Literal::Float(1.5),
                    Literal::String("s".into()),
                    Literal::Symbol(Symbol::intern("y")),
                    Literal::Character('z'),
                    Literal::MalformedInteger("16r1.8".into()),
                    Literal::UnresolvedIdentifier("super".into()),
                ]),
            },
            Instruction::LoadSelf { destination: 1 },
            Instruction::LoadBuiltinClass {
                destination: 2,
                kind: BuiltinClass::OrderedCollection,
            },
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
                call_site,
            },
            Instruction::SendPublic {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_registers: vec![2],
                destination: 6,
                call_site,
            },
            Instruction::SendInlinable {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_registers: vec![2],
                destination: 6,
                call_site,
            },
            Instruction::SendSuper {
                selector: Symbol::intern("bar"),
                argument_registers: vec![],
                destination: 7,
                call_site,
            },
            Instruction::MakeClosure {
                destination: 8,
                parameters: Rc::new(vec![Symbol::intern("e")]),
                code: block,
            },
            Instruction::MakeArray {
                destination: 9,
                element_registers: vec![0, 1],
            },
            Instruction::InitializeSlot {
                source: 9,
                own_index: 3,
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
        ],
        argument_count: 2,
        register_count: 10,
        statement_program_counter: vec![(0, 0..3), (1, 3..20)],
        call_sites: vec![call_site, call_site, call_site],
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
    let ids = encode::encode_code_table(&mut writer, heap, class, &mut symbols)
        .expect("encode code table");
    let mut mixin_writer = encode::Writer::new();
    mixins::encode(&mut mixin_writer, heap, class, &mut symbols, &ids).expect("encode mixin table");
    let bytes = writer.into_bytes();
    let mixin_bytes = mixin_writer.into_bytes();
    let table = symbols.build();
    let mut reader = decode::Reader::new(&bytes);
    let codes = decode::decode_code_table(&mut reader, &table).expect("decode code table");
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
            ids.id_of(&synthetic),
            root.initializer.expect("initializer id")
        );
    });
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

/// A mixin decoded from an image carries the bodies its own row named, with
/// no table beside it: every method the decoded declaration holds has the
/// body the live method had, and so does its initializer.
#[test]
fn a_decoded_mixin_carries_the_bodies_its_row_names() {
    let (mut interpreter, class) = load(RICH);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let root = decoded_mixins.root().expect("a root row");
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
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_u32(0); // no call sites
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_MAKE_CLOSURE);
    writer.write_u16(0); // destination
    writer.write_u16(0); // no parameters
    writer.write_u32(9); // ... whose block-body code id is out of range
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
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_MAKE_CLOSURE);
    writer.write_u16(0); // destination
    writer.write_u16(0); // no parameters
    writer.write_u32(1); // ... naming the *following*, not-yet-decoded entry
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
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_BRANCH_IF_TRUE + 1); // ... one past the last real tag
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 21
        })
    );
}

#[test]
fn an_invalid_literal_tag_and_a_bad_character_are_decode_errors_not_panics() {
    for (payload, what) in [
        (vec![LITERAL_UNRESOLVED_IDENTIFIER + 1], "literal"),
        (vec![LITERAL_CHARACTER, 0x00, 0xd8, 0x00, 0x00], "char"),
    ] {
        let mut writer = encode::Writer::new();
        writer.write_u32(1);
        writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
        writer.write_u16(0);
        writer.write_u16(1);
        writer.write_u32(0);
        writer.write_u32(0);
        writer.write_u32(0);
        writer.write_u32(0);
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

#[test]
fn an_invalid_builtin_class_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0);
    writer.write_u16(1);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_LOAD_BUILTIN_CLASS);
    writer.write_u16(0); // destination
    writer.write_u8(200); // ... naming no builtin class
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class",
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
        encode::encode_code_table(&mut writer, mutator.heap(), class, &mut symbols)
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
        BuiltinClass::Character,
        BuiltinClass::Array,
        BuiltinClass::OrderedCollection,
        BuiltinClass::Nil,
        BuiltinClass::Block,
        BuiltinClass::Platform,
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
fn a_backwards_source_span_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0); // argument_count
    writer.write_u16(0); // register_count
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
// Depths the code table controls: a literal array nests as deep as the
// image says, so decoding one has to be bounded.
// ---------------------------------------------------------------------------

/// A one-entry code table whose single instruction loads a literal array
/// nested `depth` levels deep. Each level costs five bytes in the stream
/// (`u8` tag + `u32` element count), which is what makes unbounded
/// recursion here cheap to weaponize.
fn code_table_with_nested_literal_array(depth: usize) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0); // argument_count
    writer.write_u16(1); // register_count
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_u32(0); // no call sites
    writer.write_u32(1); // one instruction ...
    writer.write_u8(OPCODE_LOAD_LITERAL); // ... a LoadLiteral
    writer.write_u16(0); // destination
    for _ in 0..depth {
        writer.write_u8(LITERAL_ARRAY); // Literal::Array ...
        writer.write_u32(1); // ... of exactly one element
    }
    writer.write_u8(LITERAL_NIL); // Literal::Nil at the bottom
    writer.write_u32(0); // no mixin sections
    writer.into_bytes()
}

#[test]
fn a_deeply_nested_literal_array_is_a_decode_error_not_a_stack_overflow() {
    // Comfortably past the depth that overflows the stack -- and aborts the
    // process outright, uncatchably -- without the ceiling.
    let bytes = code_table_with_nested_literal_array(2_000);
    assert!(
        bytes.len() < 16 * 1024,
        "the whole attack is a few kilobytes"
    );
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::NestingTooDeep {
            what: "literal array",
            limit: 128
        })
    );
    assert_eq!(
        decode::DecodeError::NestingTooDeep {
            what: "literal array",
            limit: 128
        }
        .to_string(),
        "literal array in image nests deeper than 128 levels"
    );
}

#[test]
fn a_literal_array_within_the_nesting_limit_still_decodes() {
    // The ceiling must reach at least the parser's own `MAX_DEPTH` of 128 --
    // see `the_deepest_literal_array_the_parser_accepts_survives_a_round_trip`
    // for the end-to-end version of that claim.
    let bytes = code_table_with_nested_literal_array(128);
    let mut reader = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut reader, &symbols::Table::empty())
        .expect("128 levels is within the limit");
    assert_eq!(table.codes.len(), 1);
    assert!(matches!(
        table.codes[0].instructions[0],
        Instruction::LoadLiteral {
            literal: Literal::Array(_),
            ..
        }
    ));
}

/// The code table's other depth: blocks nested `levels` deep in one method.
/// Body 0 is the innermost block, each later body creates the one before it,
/// and the last is the method -- the order the encoder writes, every block
/// before the code that creates it. The innermost block sits `levels`
/// `MakeClosure` hops below the method. Each level is 37 bytes, so the file
/// grows linearly with the nesting it asks for.
fn code_table_with_nested_blocks(levels: usize) -> Vec<u8> {
    let body = |writer: &mut encode::Writer, kind: u8, creates: Option<usize>| {
        writer.write_u8(kind);
        writer.write_u16(0); // argument_count
        writer.write_u16(1); // register_count
        writer.write_u32(0); // source.start
        writer.write_u32(0); // source.end
        writer.write_u32(0); // no statement_program_counter entries
        writer.write_u32(0); // no call sites
        match creates {
            Some(block) => {
                writer.write_u32(2); // two instructions ...
                writer.write_u8(OPCODE_MAKE_CLOSURE); // ... r0 := the next block in,
                writer.write_u16(0);
                writer.write_u16(0); // taking no parameters,
                writer.write_u32(u32::try_from(block).expect("a test-sized code id"));
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
    interpreter.enter(
        |mutator| match decode::build_root_class(mutator, &empty, None) {
            Err(decode::DecodeError::NotAClass) => assert_eq!(
                decode::DecodeError::NotAClass.to_string(),
                "the image's mixin table describes no root class"
            ),
            Err(error) => panic!("expected NotAClass, got a different error: {error}"),
            Ok(_) => panic!("expected NotAClass, got Ok"),
        },
    );
}

// ---------------------------------------------------------------------------
// The decoder's nesting ceiling must sit at or above what `ns-syntax`'s
// parser lets a program contain, or the image cannot read back what it
// just wrote.
// ---------------------------------------------------------------------------

/// A module whose one method returns a literal array nested `levels` deep:
/// `#(` opens the first level and each further `(` opens another.
fn source_with_nested_literal_array(levels: usize) -> String {
    let open = "(".repeat(levels);
    let close = ")".repeat(levels);
    formatdoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          arr = ( ^#{open}0{close} )
        ) : ()
    "}
}

/// The deepest literal-array nesting `ns-syntax`'s parser actually accepts,
/// discovered at run time rather than restated, so this test tracks the
/// parser's private `MAX_DEPTH` instead of assuming a number.
fn deepest_literal_array_the_parser_accepts() -> usize {
    let mut deepest = 0;
    for levels in 1..1024 {
        if ns_syntax::parse(&source_with_nested_literal_array(levels)).is_ok() {
            deepest = levels;
        } else {
            break;
        }
    }
    assert!(
        deepest > 0,
        "the parser accepts at least one level of nesting"
    );
    deepest
}

/// The deepest `Literal::Array` nesting any `LoadLiteral` in `code` carries.
fn literal_array_depth(code: &CompiledCode) -> usize {
    fn depth(literal: &Literal) -> usize {
        match literal {
            Literal::Array(elements) => 1 + elements.iter().map(depth).max().unwrap_or(0),
            _ => 0,
        }
    }
    code.instructions
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::LoadLiteral { literal, .. } => Some(depth(literal)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn the_deepest_literal_array_the_parser_accepts_survives_a_round_trip() {
    // The regression this pins: a decoder ceiling below the parser's bound
    // makes the image unable to read back a program the compiler just wrote
    // into it -- a clean `NestingTooDeep` on a perfectly legal source file.
    let levels = deepest_literal_array_the_parser_accepts();
    assert!(
        levels > 64,
        "the parser accepts deeper nesting than 64 levels ({levels}), which is why \
         MAX_LITERAL_DEPTH cannot be 64"
    );
    let (mut interpreter, class) = load(&source_with_nested_literal_array(levels));
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let original = method_code(mutator.heap(), class, "arr");
        assert_eq!(
            literal_array_depth(&original),
            levels,
            "the compiler lowered the whole nest"
        );
        let (table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let (_, id) = decoded_mixins.code[0]
            .methods
            .iter()
            .find(|(selector, _)| selector.as_str() == "arr")
            .expect("arr is in the root list");
        let decoded = &table.codes[*id as usize];
        assert_eq!(**decoded, *original);
        assert_eq!(literal_array_depth(decoded), levels);
    });
}

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
/// restated, for the reason `deepest_literal_array_the_parser_accepts` is.
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

/// The decoder's block-nesting ceiling (`BlockNestingTooDeep`) must admit
/// everything the parser does, under either lowering, or a legal program
/// would build into a binary that cannot be loaded. `[...] value` is not a
/// control-flow send, so neither lowering inlines a level away: the code
/// table nests exactly as deep as the source.
#[test]
fn the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs() {
    let levels = deepest_block_nesting_the_parser_accepts();
    let source = source_with_nested_blocks(levels);
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
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
    mixins::encode(&mut scratch, mutator.heap(), class, symbols, ids).expect("mixin table")
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
        let ids = encode::encode_code_table(&mut writer, mutator.heap(), class, &mut symbols)
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
        let decoded = {
            let mut context = decode::DecodeContext {
                mutator,
                root_class: &class,
                codes: &code_table.codes,
                platform: platform_value,
                mixins: &mixin_order,
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
          public outer = ( ^mk )
        ) : ()
    "};
    let (mut interpreter, class, original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            let closure = expect_ok(
                interpreter.send(&instance, Symbol::intern("outer"), &[]),
                "outer",
            );
            // Synthetic "paused" state: edit the home frame's program counter
            // and grow its register file before snapshotting.
            interpreter.enter(|mutator| {
                let closure_value = mutator.fetch(&closure);
                let closure_data = mutator
                    .heap()
                    .closure_data(*object(&closure_value))
                    .expect("expected a closure");
                let (_, argument_count, temporary_count) =
                    mutator.heap().frame_register_shape(closure_data.home);
                mutator
                    .heap_mut()
                    .frame_grow_temporaries(closure_data.home, temporary_count + 1);
                mutator.heap_mut().frame_set_local(
                    closure_data.home,
                    (argument_count + temporary_count) as u16,
                    Value::SmallInteger(9),
                );
                mutator
                    .heap_mut()
                    .set_frame_program_counter(closure_data.home, 1);
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
        let code = &closure_data.code;
        assert_eq!(**code, *original_code);
        assert!(
            closure_data.captured_frame.same(closure_data.home),
            "captured_frame and home were the same frame"
        );
        let home = heap.frame_snapshot(closure_data.home);
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
        let closure_data = heap
            .closure_data(*object(&held))
            .expect("slot holds a closure");
        let home_kind = heap.frame_kind(closure_data.home);
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
            let ids = encode::encode_code_table(&mut writer, mutator.heap(), class, &mut symbols)
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
}

impl GraphFixture {
    fn new() -> GraphFixture {
        let (mut interpreter, class) = load(EMPTY);
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
        self.interpreter.enter(|mutator| {
            let class = class_reference(mutator, class);
            let platform = mutator.fetch(platform);
            let decoded = {
                let mut context = decode::DecodeContext {
                    mutator,
                    root_class: &class,
                    codes,
                    platform,
                    mixins,
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

// ---------------------------------------------------------------------------
// Class records. A class object is an ordinary object-table entry, so every
// id it names -- its mixin, its superclass, its enclosing object, its
// declaring class -- is attacker-controlled and has to be range-checked
// before it indexes anything.
// ---------------------------------------------------------------------------

/// One class record: its own `.class` (the builtin `Object`, as
/// `Mutator::new_class_object` gives every class object), an identity
/// hash, the body tag, the mixin-table row id, then the three references
/// `references` writes.
fn class_record(
    writer: &mut encode::Writer,
    mixin_id: u32,
    references: impl FnOnce(&mut encode::Writer),
) {
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    writer.write_u32(7); // identity hash
    writer.write_u8(OBJECT_CLASS); // object body: Class
    writer.write_u32(mixin_id);
    references(writer);
}

/// A graph section holding one class record and no roots.
fn one_class_object(mixin_id: u32, references: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 0);
    class_record(&mut writer, mixin_id, references);
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
    graph_counts(&mut writer, 2, 0);
    class_record(&mut writer, 0, no_class_references);
    class_record(&mut writer, 0, no_class_references);
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
        writer.write_u32(9); // ... past the end of a one-object table
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
    graph_counts(&mut writer, 2, 0);
    class_record(&mut writer, 0, |writer| {
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
    graph_counts(&mut writer, 2, 0);
    for other in [1u32, 0] {
        class_record(&mut writer, 0, move |writer| {
            writer.write_boolean(true);
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(other);
            writer.write_boolean(false);
            writer.write_boolean(false);
        });
    }
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
    writer.write_u16(0); // no parameters
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
        let ids = encode::encode_code_table(&mut writer, mutator.heap(), class, &mut symbols)
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
        let mut context = decode::DecodeContext {
            mutator,
            root_class: &class,
            codes: &[],
            platform,
            mixins: &mixin_order,
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
        assert!(
            !inner.captured_frame.same(inner.home),
            "the inner closure was captured by the block activation, not by `mk`"
        );
        let captured = heap.frame_snapshot(inner.captured_frame);
        let FrameKind::Closure(_) = &captured.kind else {
            panic!("the captured frame is a block activation")
        };
        assert!(matches!(
            captured.arguments.first(),
            Some(Value::SmallInteger(5))
        ));
        assert!(
            captured
                .home
                .as_ref()
                .expect("a block activation has a home")
                .same(inner.home),
            "both closures share `mk`'s activation as their home"
        );
        let home_kind = heap.frame_kind(inner.home);
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
        let FrameKind::Closure(outer) = heap.frame_kind(inner.captured_frame) else {
            panic!("the captured frame is a block activation")
        };
        let outer = heap
            .closure_data(outer)
            .expect("a closure frame names a closure object");
        assert_eq!(outer.parameters.len(), 1, "the outer block takes `x`");
        assert!(
            outer.home.same(inner.home),
            "both closures share `mk`'s activation as their home"
        );
    });
}

/// A one-frame graph section whose frame carries a frame kind no build
/// assigns, for the two readers of a graph section below.
fn unknown_frame_kind_graph() -> Vec<u8> {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_SUPERCLASS_MESSAGE + 1); // one past the last frame kind

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
            tag: FRAME_SUPERCLASS_MESSAGE + 1
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
    let expected = format!("invalid frame kind tag {}", FRAME_SUPERCLASS_MESSAGE + 1);
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
        object.contains("closure [x] code "),
        "object {id} should be the outer block, with its parameter `x`: {object}"
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

/// A block `CompiledCode` taking `argument_count` arguments in `argument_count
/// + 1` registers -- enough for the graph decoder, which never runs it.
fn block_code(argument_count: u16) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instructions: vec![Instruction::ImplicitReturn { register: 0 }],
        argument_count,
        register_count: argument_count + 1,
        statement_program_counter: vec![],
        call_sites: vec![],
        source: Span::new(0, 0),
    })
}

/// A graph section holding one closure object and the one `TopLevel` frame
/// it points at, with `parameters` declared parameters against code id 0.
/// Every field but the parameter count is well-formed, so *only* the
/// arity reconciliation can reject it.
fn closure_object_with_parameter_count(parameters: &[&str]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 1);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u16(parameters.len() as u16);
    for (id, _) in parameters.iter().enumerate() {
        // `table_with(parameters)` interns them in this same order, so a
        // parameter's position is its id.
        writer.write_u32(id as u32);
    }
    writer.write_u32(0); // code id
    writer.write_u32(0); // captured frame id -- frame 0 exists
    writer.write_u32(0); // home frame id -- frame 0 exists
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
fn a_well_formed_closure_record_whose_arity_agrees_still_decodes() {
    // The control for the two rejection tests below: the same section
    // shape, with the parameter count the code actually declares. Without
    // it, a reconciliation that rejected *everything* would look correct.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(1));
    fixture.symbols = table_with(&["x"]);
    fixture.decoded(
        &closure_object_with_parameter_count(&["x"]),
        "arity agrees",
        |heap, decoded| {
            let closure_data = heap
                .closure_data(*object(&decoded.roots[0]))
                .expect("expected a closure");
            assert_eq!(*closure_data.parameters, vec![Symbol::intern("x")]);
        },
    );
}

#[test]
fn a_closure_record_declaring_too_few_parameters_is_a_decode_error() {
    // `push_step` checks the *call's* arity against `parameters.len()` but
    // sizes the register file from the *code* (`register_count -
    // argument_count`), and `locals::read_local` then indexes `temporaries[i -
    // arguments.len()]` unchecked. So this section -- otherwise entirely
    // well-formed, as the control above proves -- would decode cleanly and
    // panic the process the first time the closure is sent `value`.
    // `decode_closure_record` is the only place that sees both halves of the
    // arity.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(1));
    let error = fixture.decode_error(&closure_object_with_parameter_count(&[]));
    assert_eq!(
        error,
        decode::DecodeError::ClosureArityMismatch {
            parameters: 0,
            argument_count: 1
        }
    );
    assert_eq!(
        error.to_string(),
        "closure record declares 0 parameters but the code it names takes 1"
    );
}

#[test]
fn a_closure_record_declaring_too_many_parameters_is_a_decode_error() {
    // The mirror case does not panic -- it silently misaddresses temporaries as
    // arguments -- so nothing downstream would ever report it.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(0));
    fixture.symbols = table_with(&["x"]);
    let error = fixture.decode_error(&closure_object_with_parameter_count(&["x"]));
    assert_eq!(
        error,
        decode::DecodeError::ClosureArityMismatch {
            parameters: 1,
            argument_count: 0
        }
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
          public bump = ( n := n + 1. ^n )
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
    let SourceOrigin::File(path) = decode::decode_source_origin(&mut reader).expect("origin");
    assert_eq!(path, PathBuf::from("some/where.ns"));
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
        call_sites: vec![],
        source: Span::new(0, 0),
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
                call_site: CallSite { call_site_id: 0 },
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
                call_site: CallSite { call_site_id: 0 },
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
    // The `MakeClosure` mirror of `ClosureArityMismatch`: `parameters` and the
    // body's `argument_count` are separate wire fields here too, and
    // `push_step` reads one while the frame is sized from the other.
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
    };
    (fixture, table)
}

/// A graph section holding one closure object over code id 0 and the one
/// frame it names as its `captured_frame` and `home`, written by `frame`.
fn closure_over_frame(frame: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1, 1);
    writer.write_u8(CLASS_BUILTIN); // class tag: builtin
    writer.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u16(0); // no parameters
    writer.write_u32(0); // code id 0 -- the block body
    writer.write_u32(0); // captured frame id
    writer.write_u32(0); // home frame id
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
          public keep = ( | t | t := 7. held := [ t ]. ^held )
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
          public keep = ( held := [:x | [ x ] ] value: 5. ^held )
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
        decode::decode_tag_named_hashes(&mut reader, mutator, class)
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
    writer.write_u8(HASHED_PLATFORM + 1); // one past the last tag-named hash
    writer.write_u32(5);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::InvalidTag {
            what: "tag-named hash",
            tag: HASHED_PLATFORM + 1
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
        ObjectData::Class(ClassData::new(application, None, None, None, None)),
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
        let inner_mixin = &decoded_mixins.root().expect("a root row").nested[0];
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
            call_sites: vec![],
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
        mixins::mutability_tag(mutable),
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

#[test]
fn an_invalid_slot_mutability_tag_is_a_decode_error_not_a_panic() {
    assert_eq!(
        decode_one_row_with_slots(
            &["X", "x"],
            &[(1, 2, mixins::access_tag(AccessModifier::Protected))],
            &[]
        )
        .err(),
        Some(decode::DecodeError::InvalidTag {
            what: "slot mutability",
            tag: 2
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
    let root = decoded.root().expect("a root row");
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
    let root = decoded.root().expect("a root row");
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
    let root = decoded.root().expect("a root row").clone();
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
    let mut level = decoded.root().expect("a root row").clone();
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
    let root = decoded.root().expect("a root row").clone();
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
    let mut level = decoded.root().expect("a root row").clone();
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
            ObjectData::Class(ClassData::new(application, None, None, None, None)),
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
/// outermost as the image's single root.
fn superclass_chain_graph(levels: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, levels, 0);
    for level in 0..levels {
        class_record(&mut writer, 0, |writer| {
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
    // receiver, and `metaclass_of` walks the whole superclass chain looking
    // for a class side to relay. Recursing on it would abort here, on a
    // chain a four-megabyte image describes.
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
fn a_class_object_whose_own_class_is_another_record_is_refused() {
    // `Mutator::new_class_object` gives every class object the shared
    // bootstrap `Object` placeholder, so no real world contains one -- and
    // `class` is the single reference `Object`'s iterative `Drop` cannot
    // hand to its worklist, so a chain of them is the one link that would
    // still release recursively.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2, 0);
    // Object 0: a class record whose own class is object 1.
    writer.write_u8(CLASS_RECORD); // class tag: object-table record
    writer.write_u32(1);
    writer.write_u32(7); // identity hash
    writer.write_u8(OBJECT_CLASS); // object body: Class
    writer.write_u32(0); // mixin id
    no_class_references(&mut writer);
    // Object 1: an ordinary class record.
    class_record(&mut writer, 0, no_class_references);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::ClassOfAClassIsARecord
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
                slot_base: 0,
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
                    slot_base: 0,
                };
                let sender = mutator.heap_mut().allocate_frame(frame_running(7, None));
                let frame = mutator
                    .heap_mut()
                    .allocate_frame(frame_running(8, Some(sender)));
                let block_class = mutator.builtin_class(BuiltinClass::Block);
                let closure = mutator.heap_mut().allocate_object(
                    block_class,
                    ObjectData::Closure(ns_runtime::ClosureData {
                        parameters: std::rc::Rc::new(vec![]),
                        code: foo.clone(),
                        captured_frame: frame,
                        home: frame,
                    }),
                );
                vec![mutator.stash(Value::Object(closure))]
            })
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let closure_data = heap
            .closure_data(*object(&decoded_0))
            .expect("expected a closure");
        assert!(matches!(
            heap.frame_receiver(closure_data.home),
            Value::SmallInteger(8)
        ));
        let sender = heap
            .frame_sender(closure_data.home)
            .expect("the decoded frame keeps its sender");
        assert!(!sender.same(closure_data.home));
        assert!(matches!(
            heap.frame_receiver(sender),
            Value::SmallInteger(7)
        ));
        assert!(heap.frame_sender(sender).is_none());
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
            .closure_data(*object(&mutator.fetch(&closure)))
            .expect("expected a closure")
            .home;
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
            .closure_data(*object(&decoded_value))
            .expect("expected a closure")
            .home;
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
        mnemosyneSlot := mnemosyneSlot + 111.
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
/// 0`) and a statement in the initializer body (`count := count + 1`), which
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
        count := count + 1.
    ) (
        public increment = ( count := count + 1. ^count )
        public current = ( ^count )
        public reset = ( count := 0. ^self )
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
