//! Image format round-trip tests, section by section: the header and its
//! scalars, the mixin table, the code table, the object and frame tables,
//! whole-image `snapshot`/`load`, and the two verification passes.

#![expect(
    clippy::cast_possible_truncation,
    reason = "a test's values are small literals and lengths it built"
)]

use indoc::{formatdoc, indoc};
use ns_runtime::interpreter::Arguments;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{
    BodyLimit, CallSite, CodeKind, CompiledCode, HEADER_REGISTERS, Instruction, IntegerOperation,
    Literal, Unbuilt,
};
use ns_runtime::image;
use ns_runtime::image::opcodes::*;
use ns_runtime::image::shipped_names::{self, ShippedKind, ShippedName};
use ns_runtime::image::{decode, encode, mixins, symbols};

use ns_runtime::modules::{ModuleId, ModuleIdentity, Version};
use ns_runtime::{
    ArbitraryInteger, BuiltinClass, ClassReference, DispatchId, FrameKind, Heap, HostResult,
    HostSignal, Interpreter, Mixin, MixinApplication, Mutator, NewClass, NewClosure,
    ObjectReference, ObjectShape, Root, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN, SourceOrigin, Symbol,
    Value, ValueKind,
};
use ns_syntax::Span;
use ns_syntax::ast::AccessModifier;

// spec: image.header
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
// spec: image.header.flags
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

// spec: image.header
#[test]
fn a_bad_magic_is_rejected() {
    let bytes = vec![b'X', b'X', b'X', b'X', 1, 0, 0, 0];
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut reader),
        Err(decode::DecodeError::BadMagic)
    );
}

// spec: image.header
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

// spec: image.untrusted
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
    // A float is made by the mutator, so the values are built inside an entry.
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let values = [
            Value::nil(),
            Value::boolean(true),
            Value::boolean(false),
            Value::small_integer(-42),
            mutator.new_float(3.5),
        ];
        for value in values {
            let mut writer = encode::Writer::new();
            assert!(encode::encode_scalar_value(&mut writer, value));
            let bytes = writer.into_bytes();
            let mut reader = decode::Reader::new(&bytes);
            let decoded = decode::decode_scalar_value(mutator, &mut reader).expect("decode");
            match (value.kind(), decoded.kind()) {
                (ValueKind::Nil, ValueKind::Nil) => {}
                (ValueKind::Boolean(expected), ValueKind::Boolean(actual)) => {
                    assert_eq!(expected, actual)
                }
                (ValueKind::SmallInteger(expected), ValueKind::SmallInteger(actual)) => {
                    assert_eq!(expected, actual)
                }
                (ValueKind::SmallFloat(expected), ValueKind::SmallFloat(actual)) => {
                    assert_eq!(expected.to_bits(), actual.to_bits())
                }
                _ => panic!("round-trip mismatch"),
            }
        }
    });
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
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let values = [
            Value::small_integer(SMALL_INTEGER_MIN),
            Value::small_integer(SMALL_INTEGER_MAX),
            Value::small_integer(0),
            mutator.new_float(-0.0),
            mutator.new_float(0.0),
        ];
        for value in values {
            let mut writer = encode::Writer::new();
            assert!(encode::encode_scalar_value(&mut writer, value));
            let bytes = writer.into_bytes();
            let mut reader = decode::Reader::new(&bytes);
            let decoded = decode::decode_scalar_value(mutator, &mut reader).expect("decode");
            match (value.kind(), decoded.kind()) {
                (ValueKind::SmallInteger(expected), ValueKind::SmallInteger(actual)) => {
                    assert_eq!(expected, actual)
                }
                (ValueKind::SmallFloat(expected), ValueKind::SmallFloat(actual)) => {
                    assert_eq!(expected.to_bits(), actual.to_bits())
                }
                _ => panic!("round-trip mismatch"),
            }
        }
        // An integer in `i64` but beyond a `SmallInteger` travels under the
        // same tag, the wire's one encoding for it, and loads as the
        // `LargeInteger` it is.
        let mut writer = encode::Writer::new();
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(i64::MAX);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(mutator, &mut reader).expect("decode");
        let large = decoded
            .as_object()
            .and_then(|object| mutator.heap().large_integer(object));
        assert!(large.is_some(), "expected a LargeInteger");
        assert_eq!(mutator.heap().describe(decoded), "9223372036854775807");
        // A float outside the immediate range travels under the same tag as
        // every float and loads as a boxed Float with its own bits: every
        // NaN payload, the infinities, and 2^-127, whose exponent is the
        // immediate range's lowest but whose mantissa is zero.
        for bits in [
            f64::NAN.to_bits(),
            f64::NAN.to_bits() | (1 << 63),
            0x7FF0_0000_0000_0001,
            0x7FF8_0000_DEAD_BEEF,
            f64::INFINITY.to_bits(),
            f64::NEG_INFINITY.to_bits(),
            896 << 52,
        ] {
            let mut writer = encode::Writer::new();
            writer.write_u8(VALUE_FLOAT);
            writer.write_f64(f64::from_bits(bits));
            let bytes = writer.into_bytes();
            let mut reader = decode::Reader::new(&bytes);
            let decoded = decode::decode_scalar_value(mutator, &mut reader).expect("decode");
            assert!(decoded.as_object().is_some(), "{bits:#x} is boxed");
            assert_eq!(
                mutator.heap().float_value(decoded).map(f64::to_bits),
                Some(bits),
                "{bits:#x}"
            );
        }
    });
}

#[test]
fn encoding_an_object_value_is_refused_not_scalarized() {
    // An object is not a scalar; `encode_scalar_value` must return
    // `false` and write nothing, leaving `encode_value`, the object-graph
    // encoder, in charge.
    let mut interpreter = ns_runtime::Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = Value::object(mutator.new_string("hi".to_string()));
        let mut writer = encode::Writer::new();
        assert!(!encode::encode_scalar_value(&mut writer, object));
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

// spec: image.untrusted
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

// spec: image.untrusted
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
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(
        |mutator| match decode::decode_scalar_value(mutator, &mut reader) {
            Err(decode::DecodeError::InvalidTag {
                what: "value",
                tag: 200,
            }) => {}
            Err(error) => {
                panic!("expected InvalidTag{{value, 200}}, got a different error: {error}")
            }
            Ok(_) => panic!("expected InvalidTag{{value, 200}}, got Ok"),
        },
    );
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

/// `image::snapshot` of the one module `class`, with its text as the
/// debug source when `with_source`.
fn snapshot_one<'gc>(
    mutator: &Mutator<'gc>,
    with_source: bool,
    path: &str,
    class: ClassReference<'gc>,
    roots: &[Value<'gc>],
) -> Result<Vec<u8>, String> {
    let identity = undeclared_identity(mutator, path, class);
    let modules = [image::ModuleSource {
        class,
        identity: &identity,
        with_source,
    }];
    image::snapshot(mutator, &modules, roots)
}

/// The identity of the module at `path` whose root is `class`, as a host
/// gives a module that declares none: its class's name. A root that is no
/// class object has a name of its own, and the encoder refuses it.
fn undeclared_identity<'gc>(
    mutator: &Mutator<'gc>,
    path: &str,
    class: ClassReference<'gc>,
) -> ModuleIdentity {
    match mutator.heap().class_declaration(class) {
        Some(declaration) => ModuleIdentity::undeclared(path, declaration.name.as_str()),
        None => ModuleIdentity::undeclared(path, "NoClass"),
    }
}

/// `image::snapshot` of the one module `class` with no roots: code and
/// structure, and a graph section holding nothing.
fn module_only_image<'gc>(
    mutator: &Mutator<'gc>,
    with_source: bool,
    path: &str,
    class: ClassReference<'gc>,
) -> Result<Vec<u8>, String> {
    snapshot_one(mutator, with_source, path, class, &[])
}

/// The class object `root` holds, for the heap accessors and the encoder,
/// which take one. Fetched where it is used: a run may move it. Takes
/// `&Mutator` and is called from inside an entry, since every caller already
/// holds one by the time it needs a `ClassReference`.
fn class_reference<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ClassReference<'gc> {
    match mutator.fetch(root).kind() {
        ValueKind::Object(class) => class,
        _ => panic!("expected a class object"),
    }
}

/// `value` as text: a String's characters, or the name of the class of
/// any other value, read on the host side, since `class` is answered only
/// to its receiver's own sends.
fn text_or_class_name<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> String {
    if let Some(text) = value
        .as_object()
        .and_then(|object| mutator.heap().str_value(object))
    {
        return text;
    }
    mutator.heap().class_display_name(mutator.class_of(value))
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
    writer.write_boolean(true); // a body
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
    writer.write_boolean(false); // no full initializer
    writer.write_boolean(false); // no own part of the initializer
    writer.write_boolean(false); // no standalone initializer
    writer.write_boolean(false); // no superclass clause body
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
    let block = Rc::new(CompiledCode::new(
        CodeKind::Block,
        vec![Instruction::NonLocalReturn { register: 0 }],
        1,
        // One parameter, the header's numbers, two copied values, one
        // temporary: the widest register layout a block can have, so the
        // round trip covers `copied_count` too.
        1 + HEADER_REGISTERS + 3,
        2,
        vec![(0, 0..1)],
        Span::new(1, 2),
    ));
    let code = CompiledCode::new(
        CodeKind::Initializer,
        vec![
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
                argument_base: 1,
                argument_sources: vec![1, 2],
                destination: 5,
                call_site: CallSite { number: 0 },
            },
            Instruction::SendPublic {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_base: 2,
                argument_sources: vec![2],
                destination: 6,
                call_site: CallSite { number: 1 },
            },
            Instruction::SendInlinable {
                selector: Symbol::intern("+"),
                receiver_register: 1,
                argument_base: 2,
                argument_sources: vec![2],
                destination: 6,
                call_site: CallSite { number: 2 },
            },
            Instruction::SendSuper {
                selector: Symbol::intern("bar"),
                argument_base: 7,
                argument_sources: vec![],
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
            Instruction::SignalNonBoolean { register: 3 },
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
        2,
        15,
        0,
        vec![(0, 0..3), (1, 3..37)],
        Span::new(10, 20),
    );
    // One past the highest number among its own sends (0..=3, in emission
    // order -- what `number_call_site_families` assigns on the decoded side
    // too) and the block's `family_size` (0): derived as the decoder
    // derives it, through `CompiledCode::from_builder`, so the round trip
    // must agree.
    assert_eq!(code.family_size, 4);
    // Its own `LoadOuterLocal`/`StoreOuterLocal` reach two frames out.
    assert_eq!(code.outer_reach, 2);
    code
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
    let mut pending = vec![root_mixin_of(heap, class)];
    while let Some(mixin) = pending.pop() {
        order.push(Rc::clone(&mixin));
        // Reversed onto the stack, so that the first is taken first: each
        // nested declaration's subtree, then the class side's.
        pending.extend(mixin.class_side.iter().cloned());
        pending.extend(heap.nested_mixins(&mixin).into_iter().rev());
    }
    order
}

/// `class`'s own mixin as a value a test can edit. The clone is shallow: its
/// methods and class side are the same `Rc`s. It holds no nested
/// declarations, which the heap took when it registered the mixin
/// (`with_nested` gives a copy copies of them).
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
        mutator.heap_mut().class_set_application(class, application),
        "a class object"
    );
}

/// `mixin` with method `selector`'s body replaced by `code`.
fn with_method_code(mut mixin: Mixin, selector: &str, code: Rc<CompiledCode>) -> Mixin {
    let selector = Symbol::intern(selector);
    let mut method = (**mixin
        .compiled_method(selector)
        .expect("the mixin declares the method"))
    .clone();
    method.code = code;
    mixin.methods.insert(
        selector,
        ns_runtime::mixin::MethodEntry::Compiled(Rc::new(method)),
    );
    mixin
}

/// `class`'s own mixin, copied, with copies of its nested declarations, the
/// one named `name` replaced by `edit`'s result. Every nested declaration is
/// copied, its own nested ones with it, so that the copy is a tree of its
/// own: a heap registers each mixin under one parent, and frees a tree
/// whole.
fn with_nested<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    name: &str,
    edit: impl FnOnce(Mixin) -> Mixin,
) -> Mixin {
    let mut edit = Some(edit);
    let nested: Vec<Rc<Mixin>> = heap
        .nested_mixins(&root_mixin_of(heap, class))
        .iter()
        .map(|nested| {
            let copy = tree_copy(heap, nested);
            match edit.take_if(|_| nested.name.as_str() == name) {
                Some(edit) => Rc::new(edit(copy)),
                None => Rc::new(copy),
            }
        })
        .collect();
    assert!(edit.is_none(), "a nested declaration of that name");
    mixin_of(heap, class).with_declared_nested(nested)
}

/// `mixin`, copied with copies of its nested declarations at every depth,
/// none of them registered.
fn tree_copy(heap: &Heap<'_>, mixin: &Rc<Mixin>) -> Mixin {
    let nested = heap
        .nested_mixins(mixin)
        .iter()
        .map(|nested| Rc::new(tree_copy(heap, nested)))
        .collect();
    (**mixin).clone().with_declared_nested(nested)
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
    let rows = rows_of(heap, class);
    let ids = encode::encode_code_table(&mut writer, &rows, &[], &mut symbols)
        .expect("encode code table");
    let mut mixin_writer = encode::Writer::new();
    mixins::encode(&mut mixin_writer, &rows, &mut symbols, &ids, None).expect("encode mixin table");
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

/// The instructions of a compiled `new` in an initializer body: one of each
/// for none, one, two and 255 argument registers, the most a message takes
/// (`docs/design/compiler.md`, "Limits"), and for `steps`
/// 0, 1 and the most a `u16` holds. Every register lies above the header's
/// numbers of a body of two arguments, `FIRST` on, and so of one of none.
fn every_instantiation_instruction() -> CompiledCode {
    const FIRST: u16 = 2 + HEADER_REGISTERS;
    let mut instructions = vec![Instruction::AllocateInstance { destination: FIRST }];
    for count in [0_u16, 1, 2, 255] {
        // Above every slot, since a source may be no other slot.
        let arguments: Vec<u16> = (0..count).map(|index| FIRST + 301 + index).collect();
        instructions.push(Instruction::InitializeInstance {
            instance: FIRST,
            destination: FIRST + 600,
        });
        for steps in [0, 1, u16::MAX] {
            instructions.push(Instruction::CheckAppliedClause {
                steps,
                selector: Symbol::intern("with:and:"),
            });
            instructions.push(Instruction::InitializeSuperclass {
                steps,
                selector: Symbol::intern("new"),
                argument_base: FIRST + 1,
                argument_sources: arguments.clone(),
                destination: FIRST + 2,
            });
            instructions.push(Instruction::InitializeAppliedLevel {
                steps,
                argument_base: FIRST + 1,
                argument_sources: arguments.clone(),
                destination: FIRST + 300,
            });
        }
    }
    instructions.push(Instruction::Return { register: FIRST });
    let count = instructions.len();
    CompiledCode::new(
        CodeKind::Initializer,
        instructions,
        2,
        FIRST + 601,
        0,
        vec![(0, 0..count)],
        Span::new(10, 20),
    )
}

#[test]
fn each_instantiation_instruction_survives_an_image_round_trip() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let synthetic = Rc::new(every_instantiation_instruction());
        let doctored = Mixin {
            initializer_code: Some(synthetic.clone()),
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let (table, decoded_mixins, _) = round_trip_code_table(mutator.heap(), class);
        let root = &decoded_mixins.code[0];
        let decoded = &table.codes[root.initializer.expect("initializer id") as usize];
        assert_eq!(**decoded, *synthetic);
        assert_eq!(decoded.outer_reach, 0);
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
    for instruction in &code.instructions() {
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

// spec: image.code-table
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
            for instruction in &decoded.instructions() {
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
    for instruction in &code.instructions() {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            numbers.extend(call_site_numbers(block));
        } else if let Some(site) = instruction.call_site()
            && site != CallSite::UNCACHED
        {
            numbers.push(site.number);
        }
    }
    numbers
}

/// The decoder must reproduce the compiler's own numbering, not merely a
/// dense one: two nested blocks, each with a send of its own, so a family
/// whose blocks share no number with each other or with the root only
/// happens to look right if the decoder replays the same emission order the
/// compiler used, rather than, say, numbering block by block after the root.
// spec: bytecode.call-site
// spec: bytecode.wire.call-sites
#[test]
fn a_decoded_family_is_numbered_as_the_compiler_numbered_it() {
    let source = indoc! {"
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

// spec: image.verify.ids
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

// spec: bytecode.verify.blocks
#[test]
fn a_make_closure_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry ...
    writer.write_u8(encode::code_kind_tag(CodeKind::Block)); // CodeKind::Block
    writer.write_u16(0); // argument_count
    writer.write_u16(1); // register_count
    writer.write_u16(0); // copied_count
    writer.write_u16(0); // local_count
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

// spec: bytecode.verify.blocks
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
    writer.write_u16(0); // local_count
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

// spec: bytecode.wire.tags
// spec: bytecode.verify.tags
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

/// A code table naming a retired code kind -- the clause-message kind, or a
/// kind of the layout before `local_count` -- meets the refusal of
/// a tag never assigned.
#[test]
fn a_retired_code_kind_tag_is_refused() {
    assert_eq!(RETIRED_CODE_KIND_TAGS, [0, 1, 2, 3, 4]);
    for tag in RETIRED_CODE_KIND_TAGS {
        let mut writer = encode::Writer::new();
        writer.write_u32(1);
        writer.write_u8(tag);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        assert_eq!(
            decode::decode_code_table(&mut reader, &symbols::Table::empty()),
            Err(decode::DecodeError::InvalidTag {
                what: "code kind",
                tag
            })
        );
    }
}

// spec: bytecode.wire.tags
// spec: bytecode.verify.tags
#[test]
fn an_invalid_instruction_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0);
    writer.write_u16(0);
    writer.write_u16(0); // copied_count
    writer.write_u16(0); // local_count
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_boolean(false); // not compiled uncached
    writer.write_u32(1); // one instruction ...
    writer.write_u8(52); // ... one past the last real tag
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 52
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
    writer.write_u16(0); // local_count
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
// spec: bytecode.wire.tags
// spec: bytecode.verify.tags
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
// spec: bytecode.wire.tags
// spec: bytecode.verify.tags
#[test]
fn an_invalid_or_retired_literal_tag_is_a_decode_error_not_a_panic() {
    let mut payloads = vec![vec![LITERAL_UNBUILT_PATTERN_LITERAL + 1]];
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
        writer.write_u16(0); // local_count
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

/// A `LargeInteger` in a slot survives a snapshot as a value, including one
/// that fits `i64` and so travels as an `i64`: `-2^63` and `2^60` are beyond
/// a `SmallInteger`'s 61 bits, and the round trip keeps them `LargeInteger`s.
// spec: image.graph.value
#[test]
fn a_large_integer_value_round_trips() {
    let source = indoc! {"
        class X = Object new (
          | large = 9223372036854775807 + 1. small = 0 - (9223372036854775807 + 1). edge = 1152921504606846976. |
        ) (
          public largeText = ( ^large printString )
          public largeValue = ( ^large )
          public smallText = ( ^small printString )
          public smallValue = ( ^small )
          public edgeText = ( ^edge printString )
          public edgeValue = ( ^edge )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    for (selector, expected) in [
        ("largeText", "9223372036854775808"),
        ("largeValue", "LargeInteger"),
        ("smallText", "-9223372036854775808"),
        ("smallValue", "LargeInteger"),
        ("edgeText", "1152921504606846976"),
        ("edgeValue", "LargeInteger"),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            assert_eq!(
                text_or_class_name(mutator, mutator.fetch(&answer)),
                expected,
                "{selector}"
            );
        });
    }
}

/// An `Array` past 254 elements is decoded into an object with an overflow
/// word, and its last element survives the round trip.
#[test]
fn an_array_past_two_hundred_and_fifty_four_elements_round_trips() {
    let source = indoc! {"
        class X = Object new (
          | big = self filled. |
        ) (
          filled = (
            | array = Array new: 300. |
            1 to: 300 do: [:index | array at: index put: index].
            ^array
          )
          public lastText = ( ^(big at: 300) printString )
          public sizeText = ( ^big size printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    for (selector, expected) in [("lastText", "300"), ("sizeText", "300")] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            let Some(string) = mutator.fetch(&answer).as_object() else {
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

/// A body past the register ceiling travels as the raising body it compiled
/// to, `Literal::BodyTooLarge` and all: once loaded it raises the same
/// error, and its sibling, which never needed the ceiling, still answers.
#[test]
fn a_body_past_the_register_ceiling_round_trips_and_still_raises() {
    let temporaries: String = (0..=usize::from(u16::MAX))
        .map(|index| format!("t{index} "))
        .collect();
    let source = formatdoc! {"
        class X = Object new (
        ) (
          public run = ( | {temporaries}| t0:: 1. ^t0 )
          public answer = ( ^7 )
        ) : ()
    "};
    let (mut interpreter, class) = load(&source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        expect_error(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern("run"), &[]),
            "run",
        ),
        "method too large: it needs more than 65535 registers"
    );
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("answer"), &[]),
        "answer",
    );
    assert_eq!(integer_of(&mut loaded.interpreter, &answer), 7);
}

/// A body past the argument limit -- here one sending 256 arguments --
/// travels as its raising body too, and raises the message it raises from
/// source, the limit named.
// spec: bytecode.limit.arguments
#[test]
fn a_body_past_the_argument_limit_round_trips_and_still_raises() {
    let arguments = "k: 1 ".repeat(256);
    let source = formatdoc! {"
        class X = Object new (
        ) (
          public run = ( ^self {arguments})
          public answer = ( ^7 )
        ) : ()
    "};
    let (mut interpreter, class) = load(&source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let message = "method too large: it takes or sends more than 255 arguments";
    assert_eq!(
        expect_error(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run from source"
        ),
        message
    );
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        expect_error(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern("run"), &[]),
            "run",
        ),
        message
    );
}

/// Each reason a body is too large has a literal tag of its own, and
/// decodes to the literal that names it.
// spec: bytecode.limit.too-large
// spec: bytecode.instruction.load-literal
#[test]
fn each_too_large_literal_decodes_to_its_reason() {
    for (tag, limit) in [
        (LITERAL_BODY_TOO_LARGE, BodyLimit::Registers),
        (LITERAL_BODY_PAST_THE_ARGUMENT_LIMIT, BodyLimit::Arguments),
        (
            LITERAL_BODY_PAST_THE_INSTRUCTION_LIMIT,
            BodyLimit::Instructions,
        ),
    ] {
        let mut writer = encode::Writer::new();
        writer.write_u32(1); // one code entry
        code_header(&mut writer, CodeKind::Method, 0, HEADER_REGISTERS + 1);
        writer.write_u32(2); // two instructions
        writer.write_u8(OPCODE_LOAD_LITERAL);
        writer.write_u16(HEADER_REGISTERS);
        writer.write_u8(tag);
        writer.write_u8(OPCODE_IMPLICIT_RETURN);
        writer.write_u16(HEADER_REGISTERS);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let table = match decode::decode_code_table(&mut reader, &table_with(&[])) {
            Ok(table) => table,
            Err(error) => panic!("tag {tag}: {error}"),
        };
        assert_eq!(
            table.codes[0].instruction(0),
            Instruction::LoadLiteral {
                destination: HEADER_REGISTERS,
                literal: Literal::BodyTooLarge(limit),
            }
        );
    }
}

/// Each construct ns-next does not build has a literal tag of its own, and
/// decodes to the literal that names it: an asynchronous send with its
/// selector, a pattern literal with nothing.
// spec: bytecode.instruction.load-literal
#[test]
fn each_unbuilt_literal_decodes_to_its_construct() {
    let selector = Symbol::intern("printString");
    for (payload, unbuilt) in [
        (
            vec![LITERAL_UNBUILT_ASYNCHRONOUS_SEND, 0, 0, 0, 0],
            Unbuilt::AsynchronousSend(selector),
        ),
        (
            vec![LITERAL_UNBUILT_PATTERN_LITERAL],
            Unbuilt::PatternLiteral,
        ),
    ] {
        let mut writer = encode::Writer::new();
        writer.write_u32(1); // one code entry
        code_header(&mut writer, CodeKind::Method, 0, HEADER_REGISTERS + 1);
        writer.write_u32(2); // two instructions
        writer.write_u8(OPCODE_LOAD_LITERAL);
        writer.write_u16(HEADER_REGISTERS);
        for byte in &payload {
            writer.write_u8(*byte);
        }
        writer.write_u8(OPCODE_IMPLICIT_RETURN);
        writer.write_u16(HEADER_REGISTERS);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let table = match decode::decode_code_table(&mut reader, &table_with(&["printString"])) {
            Ok(table) => table,
            Err(error) => panic!("tag {}: {error}", payload[0]),
        };
        assert_eq!(
            table.codes[0].instruction(0),
            Instruction::LoadLiteral {
                destination: HEADER_REGISTERS,
                literal: Literal::Unbuilt(unbuilt),
            }
        );
    }
}

/// An image may carry the instructions of a compiled `new`, and a crafted
/// one can put one where no compiler does: in an ordinary method, on a
/// receiver that is no class, in a class with no level a `<:` clause added.
/// Each checks what it runs on, in its eight-byte form and in its wide one:
/// the factory's two refuse a receiver that is no class and the two that
/// name an applied level a class without one, each with an ordinary error
/// rather than a panic, and the superclass initialization of a class whose
/// superclass runs no code answers the receiver.
#[test]
fn an_instantiation_instruction_a_crafted_image_runs_raises_an_error() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public run = ( ^1 )
        ) : ()
    "};
    // A destination past what the eight-byte form addresses makes the wide
    // form.
    for destination in [HEADER_REGISTERS, HEADER_REGISTERS + 600] {
        let doctored_body = |instruction: Instruction| {
            Rc::new(CompiledCode::new(
                CodeKind::Method,
                vec![
                    // What the return reads, written first, as a compiled
                    // body writes it.
                    Instruction::LoadSelf {
                        destination: HEADER_REGISTERS,
                    },
                    instruction,
                    Instruction::Return {
                        register: HEADER_REGISTERS,
                    },
                ],
                0,
                destination + 1,
                0,
                vec![(0, 0..3)],
                Span::new(0, 0),
            ))
        };
        let not_a_class = Some("cannot instantiate an instance of X: it is not a class");
        let no_applied_level = Some(
            "the initializer of class X names a level a mixin application added 1 above it, \
             and its class has none there",
        );
        let instructions = [
            (Instruction::AllocateInstance { destination }, not_a_class),
            (
                Instruction::InitializeInstance {
                    instance: HEADER_REGISTERS,
                    destination,
                },
                not_a_class,
            ),
            (
                Instruction::CheckAppliedClause {
                    steps: 1,
                    selector: Symbol::NEW,
                },
                no_applied_level,
            ),
            (
                Instruction::InitializeSuperclass {
                    steps: 1,
                    selector: Symbol::NEW,
                    argument_base: destination,
                    argument_sources: vec![],
                    destination,
                },
                None,
            ),
            (
                Instruction::InitializeAppliedLevel {
                    steps: 1,
                    argument_base: destination,
                    argument_sources: vec![],
                    destination,
                },
                no_applied_level,
            ),
        ];
        for (instruction, expected) in instructions {
            let (mut interpreter, class) = load(source);
            let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let bytes = interpreter.enter(|mutator| {
                let class = class_reference(mutator, &class);
                let doctored = with_method_code(
                    mixin_of(mutator.heap(), class),
                    "run",
                    doctored_body(instruction.clone()),
                );
                install_mixin(mutator, class, doctored);
                let instance = mutator.fetch(&instance);
                snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
            });
            let mut loaded = image::load(&bytes).expect("load");
            let result = loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern("run"), &[]);
            match expected {
                Some(expected) => {
                    assert_eq!(expect_error(result, "run"), expected, "{instruction:?}")
                }
                None => {
                    expect_ok(result, "run");
                }
            }
        }
    }
}

/// The image's error for `source`'s module: a snapshot of its class alone,
/// which a program the format cannot carry refuses rather than aborts.
fn snapshot_error(source: &str) -> String {
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        match module_only_image(mutator, false, "t.ns", class) {
            Ok(_) => panic!("expected the snapshot to be refused"),
            Err(message) => message,
        }
    })
}

/// A method of more than 65,535 parameters compiles, to a body whose arity
/// saturates at `u16::MAX`, past the argument limit: building the image is
/// refused at that body with an error, never a panic, before the mixin
/// table's 16-bit field is reached.
#[test]
fn a_method_of_more_parameters_than_the_format_counts_is_refused_not_a_panic() {
    let pattern: String = (0..=usize::from(u16::MAX))
        .map(|index| format!("k: p{index} "))
        .collect();
    let source = formatdoc! {"
        class X = Object new (
        ) (
          public {pattern}= ( ^p0 )
        ) : ()
    "};
    assert_eq!(
        snapshot_error(&source),
        "image encoding cannot write a body arity of 65535 or more: it is past the limit of 255 arguments"
    );
}

/// A method of 256 parameters compiles to the raising body, but no image
/// carries a body past the argument limit: building one is refused, so
/// that `build` refuses what `load` would.
#[test]
fn a_method_of_256_parameters_is_refused_by_the_encoder() {
    let pattern: String = (0..256).map(|index| format!("k: p{index} ")).collect();
    let source = formatdoc! {"
        class X = Object new (
        ) (
          public {pattern}= ( ^p0 )
        ) : ()
    "};
    assert_eq!(
        snapshot_error(&source),
        "image encoding cannot write a body arity of 256: it is past the limit of 255 arguments"
    );
}

/// A tuple of 65,536 elements that all read one parameter's register fits
/// the register ceiling under the optimized lowering, but its register list
/// does not fit the code table's 16-bit length: refused, never a panic.
#[test]
fn a_register_list_longer_than_the_format_counts_is_refused_not_a_panic() {
    let elements: String = (0..=usize::from(u16::MAX)).map(|_| "x. ").collect();
    let source = formatdoc! {"
        class X = Object new (
        ) (
          public tuple: x = ( ^{{{elements}}} )
        ) : ()
    "};
    assert_eq!(
        snapshot_error(&source),
        "image encoding cannot write a register list of 65536: it does not fit the image format's 16-bit field"
    );
}

/// A float outside the immediate range in a slot survives a snapshot as a
/// value, as a `LargeInteger` does: a boxed Float has no identity (§3.1.1),
/// so the image carries its bits wherever it is referenced and writes no
/// object record for it.
// spec: image.graph.value
#[test]
fn a_boxed_float_value_round_trips_without_a_record() {
    let source = indoc! {"
        class X = Object new (
          | notANumber = 0.0 / 0.0. infinity = 1.0 / 0.0. huge = 1.0e300. |
        ) (
          public notANumberText = ( ^notANumber printString )
          public infinityText = ( ^infinity printString )
          public hugeText = ( ^huge printString )
          public hugeValue = ( ^huge )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        !report
            .graph_objects
            .iter()
            .any(|object| object.starts_with("class Float")),
        "no Float record: {:?}",
        report.graph_objects
    );
    for value in ["NaN", "Infinity"] {
        assert!(
            report.graph_objects.iter().any(|object| object
                .split(|character: char| !character.is_ascii_alphanumeric())
                .any(|word| word == value)),
            "{value} is carried as a value: {:?}",
            report.graph_objects
        );
        assert!(
            report
                .graph_objects
                .iter()
                .any(|object| object.contains("1e+300")),
            "1e+300 is carried as a value: {:?}",
            report.graph_objects
        );
    }
    let mut loaded = image::load(&bytes).expect("load");
    for (selector, expected) in [
        ("notANumberText", "NaN"),
        ("infinityText", "Infinity"),
        ("hugeText", "1e+300"),
        ("hugeValue", "Float"),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            assert_eq!(
                text_or_class_name(mutator, mutator.fetch(&answer)),
                expected,
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
            original.instructions().iter().any(|instruction| matches!(
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

/// A crafted code table can carry an `i64` beyond 61 bits under
/// `LITERAL_SMALL_INTEGER`, which the encoder never writes for one. The
/// decoder decides by range, so the literal loads as a `LargeInteger` rather
/// than as a `SmallInteger` whose tagged word would answer a different,
/// valid integer, and the method answers the integer the bytes say.
#[test]
fn a_crafted_small_integer_literal_beyond_61_bits_loads_as_a_large_integer() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public answer = ( ^1152921504606846975 )
          public answerText = ( ^answer printString )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let mut bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    // The literal `SMALL_INTEGER_MAX` as the encoder writes it, overwritten
    // in place with `i64::MAX`: same tag, same length, so nothing else in
    // the image moves.
    let mut needle = vec![LITERAL_SMALL_INTEGER];
    needle.extend_from_slice(&SMALL_INTEGER_MAX.to_le_bytes());
    assert_eq!(
        occurrences(&bytes, &needle),
        1,
        "the literal is written once"
    );
    let offset = bytes
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .expect("the literal is written once")
        + 1;
    bytes[offset..offset + 8].copy_from_slice(&i64::MAX.to_le_bytes());

    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let module_class = loaded.modules[0].class.clone();
    loaded.interpreter.enter(|mutator| {
        let class = class_reference(mutator, &module_class);
        let code = method_code(mutator.heap(), class, "answer");
        let expected = Literal::LargeInteger(ArbitraryInteger::from_i64(i64::MAX));
        assert!(
            code.instructions().iter().any(|instruction| matches!(
                instruction,
                Instruction::LoadLiteral { literal, .. } if *literal == expected
            )),
            "the literal decodes as a LargeInteger"
        );
    });
    for (selector, expected) in [
        ("answerText", "9223372036854775807"),
        ("answer", "LargeInteger"),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            assert_eq!(
                text_or_class_name(mutator, mutator.fetch(&answer)),
                expected,
                "{selector}"
            );
        });
    }
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

// spec: bytecode.verify.large-integer
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
        class X = Object new (
          | first = 9223372036854775807 + 1. second = first. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
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

/// A module whose `fill:` puts four fractions in an `Array` -- parts that
/// are `SmallInteger`s, a `LargeInteger` denominator and a `LargeInteger`
/// numerator -- and whose `parts` answers, for each, its printed form, its
/// numerator, its denominator and itself, whose classes the host reads.
const FRACTIONS: &str = indoc! {"
    class X = Object new (
        | fractions |
    ) (
        public fill: platform = (
            fractions:: {
                Fraction numerator: 1 denominator: 3.
                Fraction numerator: -7 denominator: 2.
                Fraction numerator: 1 denominator: 1180591620717411303424.
                Fraction numerator: 1180591620717411303424 denominator: 3
            }
        )
        public parts = (
            | result = Array new: fractions size * 4. index ::= 0. |
            fractions do: [:fraction |
                result at: index + 1 put: fraction printString.
                result at: index + 2 put: fraction numerator.
                result at: index + 3 put: fraction denominator.
                result at: index + 4 put: fraction.
                index:: index + 4].
            ^result
        )
    ) : ()
"};

/// What `FRACTIONS`'s `parts` answers once `fill:` has run, each fraction's
/// four parts written as text by `text_or_class_name`.
const FRACTIONS_TEXT: &str = "(1/3) Integer Integer Fraction;\
    (-7/2) Integer Integer Fraction;\
    (1/1180591620717411303424) Integer LargeInteger Fraction;\
    (1180591620717411303424/3) LargeInteger Integer Fraction;";

/// `FRACTIONS`'s instance, filled, snapshot.
fn fractions_image() -> Vec<u8> {
    let (mut interpreter, class) = load(FRACTIONS);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    expect_ok(
        interpreter.send(
            &instance,
            Symbol::intern("fill:"),
            std::slice::from_ref(&platform),
        ),
        "fill:",
    );
    interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    })
}

/// A `Fraction` survives a snapshot as a value: each loads with equal
/// parts, each part the integer class it was.
// spec: image.graph.value
#[test]
fn a_fraction_value_round_trips() {
    let mut loaded = image::load(&fractions_image()).expect("load");
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("parts"), &[]),
        "parts",
    );
    loaded.interpreter.enter(|mutator| {
        let Some(array) = mutator.fetch(&answer).as_object() else {
            panic!("parts: expected an Array")
        };
        let parts = mutator
            .heap()
            .array_elements(array)
            .expect("parts answers an Array");
        let text: String = parts
            .chunks(4)
            .map(|fraction| {
                let [printed, numerator, denominator, itself] = fraction else {
                    panic!("each fraction has four parts")
                };
                format!(
                    "{} {} {} {};",
                    text_or_class_name(mutator, *printed),
                    text_or_class_name(mutator, *numerator),
                    text_or_class_name(mutator, *denominator),
                    text_or_class_name(mutator, *itself)
                )
            })
            .collect();
        assert_eq!(text, FRACTIONS_TEXT);
    });
}

/// A `Fraction` has no identity (§3.1.1), so the image carries it as a
/// value, its two parts inline, and never gives it an object record.
#[test]
fn a_fraction_is_a_value_not_a_record() {
    let report = image::inspect::inspect(&fractions_image()).expect("inspect");
    assert!(
        !report
            .graph_objects
            .iter()
            .any(|object| object.starts_with("class Fraction")),
        "no Fraction record: {:?}",
        report.graph_objects
    );
    for expected in [
        "fraction 1/3",
        "fraction -7/2",
        "fraction 1/1180591620717411303424",
        "fraction 1180591620717411303424/3",
    ] {
        assert!(
            report
                .graph_objects
                .iter()
                .any(|object| object.contains(expected)),
            "{expected}: {:?}",
            report.graph_objects
        );
    }
}

/// A `VALUE_FRACTION` the decoder must refuse, each with the error it
/// answers: a pair outside the normal form -- a denominator of 0, of 1 or
/// below 0, or a factor both parts share -- and a part that is no integer.
#[test]
fn a_fraction_outside_its_normal_form_is_refused() {
    let small = |writer: &mut encode::Writer, integer: i64| {
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(integer);
    };
    let pair = |numerator: i64, denominator: i64| {
        one_array_object(move |writer| {
            writer.write_u8(VALUE_FRACTION);
            small(writer, numerator);
            small(writer, denominator);
        })
    };
    let float_part = one_array_object(|writer| {
        writer.write_u8(VALUE_FRACTION);
        small(writer, 1);
        writer.write_u8(VALUE_FLOAT);
        writer.write_f64(2.0);
    });
    let nested = one_array_object(|writer| {
        writer.write_u8(VALUE_FRACTION);
        writer.write_u8(VALUE_FRACTION);
        small(writer, 1);
        small(writer, 2);
        small(writer, 3);
    });
    let non_canonical_part = one_array_object(|writer| {
        writer.write_u8(VALUE_FRACTION);
        small(writer, 1);
        writer.write_u8(VALUE_LARGE_INTEGER);
        writer.write_u8(0); // positive
        writer.write_u32(1); // one digit
        writer.write_u64(5); // which fits `i64`
    });
    for (bytes, described, expected) in [
        (
            pair(1, 0),
            "a denominator of 0",
            decode::DecodeError::NonNormalFraction,
        ),
        (
            pair(3, 1),
            "a denominator of 1",
            decode::DecodeError::NonNormalFraction,
        ),
        (
            pair(3, -4),
            "a negative denominator",
            decode::DecodeError::NonNormalFraction,
        ),
        (
            pair(2, 4),
            "a shared factor",
            decode::DecodeError::NonNormalFraction,
        ),
        (
            pair(0, 5),
            "a zero numerator",
            decode::DecodeError::NonNormalFraction,
        ),
        (
            float_part,
            "a Float part",
            decode::DecodeError::InvalidTag {
                what: "fraction part",
                tag: VALUE_FLOAT,
            },
        ),
        (
            nested,
            "a fraction part",
            decode::DecodeError::InvalidTag {
                what: "fraction part",
                tag: VALUE_FRACTION,
            },
        ),
        (
            non_canonical_part,
            "a part not in its canonical encoding",
            decode::DecodeError::NonCanonicalInteger,
        ),
    ] {
        let mut fixture = GraphFixture::new();
        let before = fixture.interpreter.heap_statistics().live_objects;
        assert_eq!(fixture.decode_error(&bytes), expected, "{described}");
        assert_eq!(
            fixture.interpreter.heap_statistics().live_objects,
            before,
            "{described}: nothing was allocated"
        );
    }
    assert_eq!(
        decode::DecodeError::NonNormalFraction.to_string(),
        "a fraction is not in normal form: its denominator is not above 1, or its parts share \
         a factor"
    );
}

/// One record under the `Fraction` header, its body written by `body`, and
/// no roots.
fn fraction_record(body: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    class_records(1, |writer| {
        writer.write_shipped(FRACTION_NAME);
        writer.write_u32(5); // identity hash
        body(writer);
    })
}

/// A fraction travels only as a `VALUE_FRACTION`, so the one record a
/// `Fraction` header may carry is `Fraction new`'s, an instance of no
/// field: a record of fields under it would be a fraction whose normal
/// form nothing checked, and the first pass refuses it before anything is
/// allocated.
#[test]
fn a_fraction_record_with_fields_is_refused() {
    let two_fields = fraction_record(|writer| {
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(2);
        for part in [2, 4] {
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(part);
        }
    });
    let array = fraction_record(|writer| {
        writer.write_u8(OBJECT_ARRAY);
        writer.write_u32(0); // no elements
    });
    for (bytes, described) in [(two_fields, "two fields"), (array, "an array body")] {
        let mut fixture = GraphFixture::new();
        let before = fixture.interpreter.heap_statistics().live_objects;
        assert_eq!(
            fixture.decode_error(&bytes),
            decode::DecodeError::FractionRecord,
            "{described}"
        );
        assert_eq!(
            fixture.interpreter.heap_statistics().live_objects,
            before,
            "{described}: nothing was allocated"
        );
    }
    assert_eq!(
        decode::DecodeError::FractionRecord.to_string(),
        "an object in the image is a Fraction with fields, which only a fraction value may be"
    );
    let no_field = fraction_record(|writer| {
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0);
    });
    GraphFixture::new().decoded(&no_field, "`Fraction new`", |heap, graph| {
        assert_eq!(heap.instance_field_count(graph.objects[0]), Some(0));
    });
}

/// A `LargeInteger` that fits `i64` travels as `VALUE_SMALL_INTEGER`, the
/// wire's one encoding for every integer in `i64`, wherever a `SmallInteger`
/// ends; the decoder makes it a `LargeInteger` again because it lies beyond
/// 61 bits.
// spec: image.graph.value
// spec: image.encoding
#[test]
fn a_large_integer_in_i64_travels_as_an_i64_and_loads_as_a_large_integer() {
    let source = indoc! {"
        class X = Object new (
          | edge = 1152921504606846976. |
        ) (
          public edgeValue = ( ^edge )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .graph_objects
            .iter()
            .any(|object| object.contains("1152921504606846976")
                && !object.contains("large-int 1152921504606846976")),
        "written as an i64: {:?}",
        report.graph_objects
    );
    let mut loaded = image::load(&bytes).expect("load");
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("edgeValue"), &[]),
        "edgeValue",
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(
            text_or_class_name(mutator, mutator.fetch(&answer)),
            "LargeInteger"
        );
    });
}

/// `inspect` renders an integer in decimal only up to
/// `DECIMAL_RENDERING_LIMIT` digits, and elides one past it: an artifact is
/// untrusted, and the quadratic decimal conversion of an integer of 100,000
/// digits would keep `inspect` busy for hours.
#[test]
fn inspect_renders_a_huge_large_integer_elided_and_a_normal_one_in_decimal() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let started = std::time::Instant::now();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let normal = mutator.new_integer(ArbitraryInteger::from_i128(1_i128 << 63));
        let huge =
            mutator.new_integer(ArbitraryInteger::from_parts(false, (1..=100_000).collect()));
        snapshot_one(mutator, false, "t.ns", class, &[instance, normal, huge]).expect("snapshot")
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

// spec: bytecode.wire.tags
// spec: bytecode.verify.tags
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
    writer.write_u16(0); // local_count
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
        encode::encode_code_table(
            &mut writer,
            &rows_of(mutator.heap(), class),
            &[],
            &mut symbols,
        )
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
// spec: bytecode.verify.body-fields
#[test]
fn a_copied_count_that_does_not_fit_the_register_file_is_a_decode_error() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Block)); // CodeKind::Block
    writer.write_u16(1); // argument_count
    writer.write_u16(2); // register_count
    writer.write_u16(2); // copied_count -- one more than the file has room for
    writer.write_u16(0); // local_count
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

// spec: bytecode.verify.body-fields
#[test]
fn a_backwards_source_span_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(encode::code_kind_tag(CodeKind::Method)); // CodeKind::Method
    writer.write_u16(0); // argument_count
    writer.write_u16(0); // register_count
    writer.write_u16(0); // copied_count
    writer.write_u16(0); // local_count
    writer.write_u32(9); // source.start ...
    writer.write_u32(4); // ... after source.end
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidSpan { start: 9, end: 4 })
    );
}

// spec: image.verify.counts
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
/// `MakeClosure` hops below the method. Each level is 40 bytes, so the file
/// grows linearly with the nesting it asks for.
fn code_table_with_nested_blocks(levels: usize) -> Vec<u8> {
    let body = |writer: &mut encode::Writer, kind: u8, creates: Option<usize>| {
        writer.write_u8(kind);
        writer.write_u16(0); // argument_count
        writer.write_u16(HEADER_REGISTERS + 1); // register_count: one past the header's numbers
        writer.write_u16(0); // copied_count
        writer.write_u16(0); // local_count
        writer.write_u32(0); // source.start
        writer.write_u32(0); // source.end
        writer.write_u32(0); // no statement_program_counter entries
        writer.write_boolean(false); // not compiled uncached
        match creates {
            Some(block) => {
                writer.write_u32(2); // two instructions ...
                writer.write_u8(OPCODE_MAKE_CLOSURE); // ... r13 <- the next block in,
                writer.write_u16(HEADER_REGISTERS);
                writer.write_u16(0); // taking no parameters,
                writer.write_u32(u32::try_from(block).expect("a test-sized code id"));
                writer.write_u16(0); // copying nothing
            }
            None => {
                writer.write_u32(2); // the innermost: two instructions ...
                writer.write_u8(OPCODE_LOAD_SELF); // ... r13 <- self,
                writer.write_u16(HEADER_REGISTERS);
            }
        }
        writer.write_u8(OPCODE_RETURN); // ... then ^r13
        writer.write_u16(HEADER_REGISTERS);
    };
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::try_from(levels + 1).expect("a test-sized code table"));
    body(&mut writer, encode::code_kind_tag(CodeKind::Block), None);
    for id in 1..levels {
        body(
            &mut writer,
            encode::code_kind_tag(CodeKind::Block),
            Some(id - 1),
        );
    }
    body(
        &mut writer,
        encode::code_kind_tag(CodeKind::Method),
        Some(levels - 1),
    );
    writer.into_bytes()
}

/// A decoded block's nesting is what bounds the exempt activations a
/// crafted `SendInlinable` chain can stack above one counted activation
/// (`docs/design/compiler.md`, "How the limit counts"), so the decoder caps it
/// at the parser's own ceiling: one level past it is refused.
// spec: bytecode.verify.blocks
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
// spec: bytecode.verify.blocks
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
/// no declaration for a module's root record to be held to -- and nothing
/// for `load` to build the world around.
#[test]
fn a_mixin_table_with_no_root_row_is_a_decode_error() {
    let mut interpreter = Interpreter::bootstrap();
    let empty = mixins::Decoded::empty();
    interpreter.enter(|mutator| {
        let module = image::modules::DecodedModule {
            identity: Rc::new(ModuleIdentity::undeclared("t.ns", "T")),
            root: 0,
        };
        let objects = [mutator.builtin_class(BuiltinClass::Object)];
        match image::modules::module_root_classes(mutator, &empty, &[module], &objects) {
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
    code.instructions()
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
            module_only_image(mutator, false, "t.ns", class)
            .expect("snapshot")
        });
        let mut loaded = match image::load(&bytes) {
            Ok(loaded) => loaded,
            Err(error) => panic!("{lowering:?}: {levels} nested blocks must load: {error}"),
        };
        let instance = expect_ok(
            loaded.interpreter.instantiate(&loaded.modules[0].class, &[]),
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
                .enter(|mutator| mutator.fetch(&answer).is_nil()),
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
    mutator: &Mutator<'gc>,
    class: ClassReference<'gc>,
    symbols: &mut symbols::Builder,
    ids: &encode::CodeIds,
) -> mixins::MixinIds {
    let mut scratch = encode::Writer::new();
    mixins::encode(
        &mut scratch,
        &rows_of(mutator.heap(), class),
        symbols,
        ids,
        None,
    )
    .expect("mixin table")
}

/// Loads `source`, lets `make_roots` build the roots, encodes code table +
/// graph section, decodes both with the same `Reader`, and returns
/// (interpreter, the decoded root class -- a record of the graph, as the
/// module section would name it -- original roots, decoded roots, maximum
/// identity hash) -- every handle rooted, since none of them may outlive the entry
/// that produced it. `make_roots` may run code, so the class and the roots
/// are only read again, inside a fresh entry, once it has returned.
fn round_trip_graph(
    source: &str,
    make_roots: impl FnOnce(&mut Interpreter, &Root) -> Vec<Root>,
) -> (Interpreter, Root, Vec<Root>, Vec<Root>, u32) {
    let (mut interpreter, class) = load(source);
    let roots = make_roots(&mut interpreter, &class);
    let (decoded_class, decoded_roots, max_identity_hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let root_values: Vec<Value> = roots.iter().map(|root| mutator.fetch(root)).collect();
        let mut symbols = symbols::Builder::new();
        let mut writer = encode::Writer::new();
        let ids = encode::encode_code_table(
            &mut writer,
            &rows_of(mutator.heap(), class),
            &[],
            &mut symbols,
        )
        .expect("encode code table");
        let mixin_ids = mixin_ids_for(mutator, class, &mut symbols, &ids);
        let walked = encode::walk_graph(mutator, &root_values, &[class]).expect("walk graph");
        let encoded = encode::encode_graph_section(
            mutator,
            &mut writer,
            &walked,
            &root_values,
            &ids,
            &mixin_ids,
            &mut symbols,
        )
        .expect("encode graph");
        let shipped = shipped_names::resolve_in(mutator, &encoded.used.names(mutator), false)
            .expect("the graph's own names resolve");
        let bytes = writer.into_bytes();
        let symbol_table = symbols.build();
        let mut reader = decode::Reader::new(&bytes);
        let code_table = decode::decode_code_table(&mut reader, &symbol_table).expect("code table");
        let mixin_order = mixin_preorder(mutator.heap(), class);
        let decoded = {
            let mut context = decode::DecodeContext {
                mutator,
                module_roots: &encoded.module_roots,
                codes: &code_table.codes,
                mixins: &mixin_order,
                shipped: &shipped,
            };
            decode::decode_graph_section(&mut reader, &mut context, &symbol_table).expect("graph")
        };
        let decoded_roots: Vec<Root> = decoded
            .roots
            .iter()
            .map(|value| mutator.stash(*value))
            .collect();
        let decoded_class = mutator.stash(Value::object(
            decoded.objects[encoded.module_roots[0] as usize],
        ));
        (decoded_class, decoded_roots, decoded.max_identity_hash)
    });
    (
        interpreter,
        decoded_class,
        roots,
        decoded_roots,
        max_identity_hash,
    )
}

const EMPTY: &str = indoc! {"
    class X = Object new (
    ) (
    ) : ()
"};

fn object<'gc>(value: Value<'gc>) -> ObjectReference<'gc> {
    let Some(handle) = value.as_object() else {
        panic!("expected an object")
    };
    handle
}

#[test]
fn instances_and_arrays_round_trip_with_shared_identity_preserved() {
    let (mut interpreter, _class, original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, _| {
            interpreter.enter(|mutator| {
                let shared = Value::object(mutator.new_string("shared".to_string()));
                let array = mutator.new_array(vec![shared, shared, Value::small_integer(7)]);
                vec![mutator.stash(Value::object(array))]
            })
        });
    interpreter.enter(|mutator| {
        let original = mutator.fetch(&original[0]);
        let decoded = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let elements = heap
            .array_elements(object(decoded))
            .expect("expected an array");
        assert!(
            object(elements[0]).same(object(elements[1])),
            "shared identity preserved"
        );
        assert!(
            !object(original).same(object(decoded)),
            "the decoded array is a new object"
        );
        assert!(matches!(
            heap.str_value(object(elements[0])).as_deref(),
            Some("shared")
        ));
        assert!(elements[2].as_small_integer() == Some(7));
    });
}

/// The bytes of a `ByteArray` of `size` for the image tests: every value a
/// byte takes, so that a byte read from the wrong place or the wrong word
/// shows.
fn byte_array_contents(size: usize) -> Vec<u8> {
    Vec::from_fn(size, |index| (index * 7 + 3) as u8)
}

// spec: image.graph.record
#[test]
fn a_byte_array_round_trips_with_its_bytes_and_identity_hash() {
    let sizes = [0, 5, 4_000];
    let (mut interpreter, _class, original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, _| {
            interpreter.enter(|mutator| {
                let arrays: Vec<Value> = sizes
                    .iter()
                    .map(|size| {
                        let bytes = byte_array_contents(*size);
                        let array = mutator.new_byte_array(&bytes);
                        mutator.heap_mut().object_ensure_identity_hash(array);
                        Value::object(array)
                    })
                    .collect();
                // The five-byte one twice, so that its identity is kept.
                let holder = mutator.new_array(vec![arrays[0], arrays[1], arrays[2], arrays[1]]);
                vec![mutator.stash(Value::object(holder))]
            })
        });
    interpreter.enter(|mutator| {
        let original = mutator.fetch(&original[0]);
        let decoded = mutator.fetch(&decoded[0]);
        let byte_array_class = mutator.builtin_class(BuiltinClass::ByteArray);
        let heap = mutator.heap();
        let originals = heap
            .array_elements(object(original))
            .expect("expected an array");
        let elements = heap
            .array_elements(object(decoded))
            .expect("expected an array");
        assert!(
            object(elements[1]).same(object(elements[3])),
            "shared identity preserved"
        );
        for (index, size) in sizes.iter().enumerate() {
            let decoded = object(elements[index]);
            let original = object(originals[index]);
            assert!(!decoded.same(original), "the decoded one is a new object");
            assert!(heap.is_byte_array(decoded));
            assert_eq!(heap.byte_array_size(decoded), Some(*size));
            assert_eq!(
                heap.byte_array_bytes(decoded),
                Some(byte_array_contents(*size))
            );
            assert_eq!(
                heap.object_identity_hash(decoded),
                heap.object_identity_hash(original)
            );
            assert!(heap.object_class(decoded).same(byte_array_class));
        }
    });
}

/// A `ByteArray` a program made, held in a slot, round-trips as the
/// kernel's class with its bytes.
#[test]
fn a_byte_array_a_program_made_round_trips() {
    let source = indoc! {"
        class X = Object new (
          | bytes = ByteArray new: 3. |
        ) (
          public fill = (
            bytes at: 1 put: 1.
            bytes at: 3 put: 255.
          )
        ) : ()
    "};
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(source, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            expect_ok(
                interpreter.send(&instance, Symbol::intern("fill"), &[]),
                "fill",
            );
            vec![instance]
        });
    interpreter.enter(|mutator| {
        let decoded = mutator.fetch(&decoded[0]);
        let byte_array_class = mutator.builtin_class(BuiltinClass::ByteArray);
        let heap = mutator.heap();
        let bytes = object(
            heap.instance_field(object(decoded), 0)
                .expect("expected an instance"),
        );
        assert!(heap.object_class(bytes).same(byte_array_class));
        assert_eq!(heap.byte_array_bytes(bytes), Some(vec![1, 0, 255]));
    });
}

/// `inspect` reads a `ByteArray` record as the loader does and shows its
/// size and its first 16 bytes, never a large one whole.
#[test]
fn inspect_shows_a_byte_array_by_its_size_and_first_bytes() {
    let (mut interpreter, class) = load(EMPTY);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let small = mutator.new_byte_array(&[0x00, 0x7f, 0xff]);
        let large = mutator.new_byte_array(&byte_array_contents(20));
        snapshot_one(
            mutator,
            false,
            "t.ns",
            class,
            &[instance, Value::object(small), Value::object(large)],
        )
        .expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let shown = |expected: &str| {
        report
            .graph_objects
            .iter()
            .any(|object| object.ends_with(expected))
    };
    assert!(
        shown("byte array of 3 bytes [00 7f ff]"),
        "{:?}",
        report.graph_objects
    );
    assert!(
        shown("byte array of 20 bytes [03 0a 11 18 1f 26 2d 34 3b 42 49 50 57 5e 65 6c ...]"),
        "{:?}",
        report.graph_objects
    );
}

/// A program holding weak arrays and ephemerons beside some of what they
/// name. Each `fill…:` method makes what is meant to be held only weakly
/// by `Object new` inside it and returns, so no frame the graph reaches
/// holds it: the slots reach it, or nothing does. The slots are walked in
/// order, so `first` is met before `second` and both before `held`, and
/// `weakFirst` before `late`.
const WEAK_HOLDER: &str = indoc! {"
    class X = Object new (
      |
        public first
        public second
        public held
        public weak
        public bare
        public weakFirst
        public late
      |
    ) (
      public fillWeak: platform = (
        held:: Object new.
        weak:: platform kernel WeakArray new: 3.
        weak at: 1 put: held.
        weak at: 2 put: Object new.
        weak at: 3 put: 7.
        bare:: platform kernel WeakArray withoutSlots.
      )

      public fillReached: platform = (
        held:: Object new.
        first:: platform kernel Ephemeron new.
        first
            key: held;
            value: 'the' , ' value';
            finalizer: 'the' , ' finalizer'.
      )

      public fillMourned: platform = (
        first:: platform kernel Ephemeron new.
        first
            key: Object new;
            value: 'the' , ' value';
            finalizer: 'the' , ' finalizer'.
      )

      public fillWeakFirst: platform = (
        late:: Object new.
        weakFirst:: platform kernel WeakArray new: 1.
        weakFirst at: 1 put: late.
      )

      public fillNilKey: platform = (
        first:: platform kernel Ephemeron new.
        first
            value: 'the' , ' value';
            finalizer: 'the' , ' finalizer'.
      )

      public fillChain: platform = (
        |
          chainKey = Object new.
        |
        held:: Object new.
        first:: platform kernel Ephemeron new.
        second:: platform kernel Ephemeron new.
        first
            key: chainKey;
            value: 'the first' , ' value'.
        second
            key: held;
            value: chainKey.
      )
    ) : ()
"};

/// Round-trips an instance of `WEAK_HOLDER` once `selector` has filled it,
/// and hands the decoded instance to `inspect` inside an entry.
fn weak_holder_round_trip(
    selector: &str,
    inspect: impl for<'gc> FnOnce(&Mutator<'gc>, ObjectReference<'gc>),
) {
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(WEAK_HOLDER, |interpreter, class| {
            let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
            let platform = interpreter.platform();
            expect_ok(
                interpreter.send(&instance, Symbol::intern(selector), &[platform]),
                selector,
            );
            vec![instance]
        });
    interpreter.enter(|mutator| {
        let decoded = object(mutator.fetch(&decoded[0]));
        inspect(mutator, decoded);
    });
}

/// The slot of `WEAK_HOLDER`'s instance `holder` at `index`.
fn holder_field<'gc>(
    mutator: &Mutator<'gc>,
    holder: ObjectReference<'gc>,
    index: usize,
) -> Value<'gc> {
    mutator
        .heap()
        .instance_field(holder, index)
        .expect("the holder has its slots")
}

/// Whether `value` and `other` are one object.
fn same_value_object<'gc>(value: Value<'gc>, other: Value<'gc>) -> bool {
    matches!(
        (value.as_object(), other.as_object()),
        (Some(one), Some(two)) if one.same(two)
    )
}

/// The encoder writes what a collection would keep: the object the
/// holder's `held` slot reaches too stays, the one only the weak array
/// reaches is written `nil`, and the integer, which no collection clears,
/// stays. `WeakArray withoutSlots`, an ordinary instance of the class,
/// travels as one.
// spec: image.graph.record
#[test]
fn a_weak_array_round_trips_its_strongly_reached_elements() {
    weak_holder_round_trip("fillWeak:", |mutator, holder| {
        let heap = mutator.heap();
        let weak_array_class = mutator.builtin_class(BuiltinClass::WeakArray);
        let held = holder_field(mutator, holder, 2);
        let weak = object(holder_field(mutator, holder, 3));
        assert!(heap.object_class(weak).same(weak_array_class));
        let elements = heap
            .weak_array_elements(weak)
            .expect("a weak array decodes as one");
        assert_eq!(elements.len(), 3);
        assert!(
            same_value_object(elements[0], held),
            "the strongly held object"
        );
        assert!(elements[1].is_nil(), "the object held only weakly");
        assert_eq!(elements[2].as_small_integer(), Some(7));
        let bare = object(holder_field(mutator, holder, 4));
        assert!(heap.object_class(bare).same(weak_array_class));
        assert_eq!(heap.weak_array_size(bare), None);
        assert_eq!(heap.instance_field_count(bare), Some(0));
    });
}

/// `weakFirst` is walked before `late`, the one slot that reaches its
/// element strongly: whether a weak word is written is decided once the
/// walk is done, not when the weak array is met, so the element stays.
#[test]
fn a_weak_array_walked_before_the_strong_path_to_its_element_keeps_it() {
    weak_holder_round_trip("fillWeakFirst:", |mutator, holder| {
        let heap = mutator.heap();
        let weak = object(holder_field(mutator, holder, 5));
        let late = holder_field(mutator, holder, 6);
        assert!(late.as_object().is_some(), "the strong slot is written");
        let elements = heap
            .weak_array_elements(weak)
            .expect("a weak array decodes as one");
        assert_eq!(elements.len(), 1);
        assert!(
            same_value_object(elements[0], late),
            "the element reached strongly after the weak array"
        );
    });
}

/// A `nil` key is never collected, so an ephemeron keyed by it holds its
/// value and finalizer as any slot does, and is written with them.
// spec: image.graph.record
#[test]
fn an_ephemeron_with_a_nil_key_round_trips_with_its_value() {
    weak_holder_round_trip("fillNilKey:", |mutator, holder| {
        let heap = mutator.heap();
        let ephemeron = object(holder_field(mutator, holder, 0));
        assert_eq!(heap.object_shape(ephemeron), ObjectShape::Ephemeron);
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron has fields");
        assert_eq!(fields.len(), 3);
        assert!(fields[0].is_nil(), "the key");
        assert_eq!(text_or_class_name(mutator, fields[1]), "the value");
        assert_eq!(text_or_class_name(mutator, fields[2]), "the finalizer");
    });
}

// spec: image.graph.record
#[test]
fn an_ephemeron_round_trips_with_its_value_while_its_key_is_reached() {
    weak_holder_round_trip("fillReached:", |mutator, holder| {
        let heap = mutator.heap();
        let held = holder_field(mutator, holder, 2);
        let ephemeron = object(holder_field(mutator, holder, 0));
        assert!(
            heap.object_class(ephemeron)
                .same(mutator.builtin_class(BuiltinClass::Ephemeron))
        );
        assert_eq!(heap.object_shape(ephemeron), ObjectShape::Ephemeron);
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron has fields");
        assert_eq!(fields.len(), 3);
        assert!(same_value_object(fields[0], held), "the key");
        assert_eq!(text_or_class_name(mutator, fields[1]), "the value");
        assert_eq!(text_or_class_name(mutator, fields[2]), "the finalizer");
    });
}

/// A key nothing but the ephemeron reaches is one a collection would
/// mourn: the ephemeron is written with all three fields `nil`.
#[test]
fn an_ephemeron_whose_key_is_not_reached_is_written_mourned() {
    weak_holder_round_trip("fillMourned:", |mutator, holder| {
        let heap = mutator.heap();
        let ephemeron = object(holder_field(mutator, holder, 0));
        assert_eq!(heap.object_shape(ephemeron), ObjectShape::Ephemeron);
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron has fields");
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().all(|field| field.is_nil()), "mourned");
    });
}

/// `first`'s key is reached only through `second`'s value, and `second`'s
/// key only through `held`, a slot walked after both: when the walk ends
/// both are pending, and one pass over them in order finds `first`'s key
/// still unreached. Only the fixpoint -- `second` traced, then the pending
/// ones scanned again -- writes `first` whole.
#[test]
fn an_ephemeron_whose_key_is_reached_only_through_another_ephemeron_is_written_after_it() {
    weak_holder_round_trip("fillChain:", |mutator, holder| {
        let heap = mutator.heap();
        let held = holder_field(mutator, holder, 2);
        let first = object(holder_field(mutator, holder, 0));
        let second = object(holder_field(mutator, holder, 1));
        let first_fields = heap.ephemeron_fields(first).expect("an ephemeron");
        let second_fields = heap.ephemeron_fields(second).expect("an ephemeron");
        assert!(same_value_object(second_fields[0], held), "second's key");
        assert!(
            first_fields[0].as_object().is_some(),
            "first's key is written"
        );
        assert!(
            same_value_object(second_fields[1], first_fields[0]),
            "second's value is first's key"
        );
        assert_eq!(
            text_or_class_name(mutator, first_fields[1]),
            "the first value"
        );
        assert!(first_fields[2].is_nil());
        assert!(second_fields[2].is_nil());
    });
}

/// `inspect` reads both records as the loader does and shows their values.
/// `fillReached:` gives `held` a new object, so the one the weak array
/// held first is reached by nothing else and shows as `nil`.
#[test]
fn inspect_shows_a_weak_array_and_an_ephemeron() {
    let (mut interpreter, class) = load(WEAK_HOLDER);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    for selector in ["fillWeak:", "fillReached:"] {
        expect_ok(
            interpreter.send(
                &instance,
                Symbol::intern(selector),
                std::slice::from_ref(&platform),
            ),
            selector,
        );
    }
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let shown = |expected: &str| {
        report
            .graph_objects
            .iter()
            .any(|object| object.ends_with(expected))
    };
    assert!(
        shown("Kernel WeakArray, hash 0, weak array [nil, nil, 7]"),
        "{:?}",
        report.graph_objects
    );
    assert!(
        report
            .graph_objects
            .iter()
            .any(|object| object.contains("Kernel Ephemeron, hash 0, ephemeron {object ")),
        "{:?}",
        report.graph_objects
    );
}

/// A module whose instance's `held` slot the tests below fill with reified
/// mixins: `X` declares no class side, so its class side is synthesized,
/// and `Inner` declares one. `listClass:` answers a shipped class.
const REIFIED_HOLDER: &str = indoc! {"
    class X = Object new (
      |
        public held
      |
    ) (
      public class Inner = (
      ) (
      ) : (
        public make = (
          ^self new
        )
      )

      public listClass: platform = (
        ^platform collections List
      )
    )
"};

/// The snapshot of an instance of `REIFIED_HOLDER`'s class whose `held`
/// slot holds an `Array` of what `elements` answers, given the module's
/// class and `List`; the encoder's refusal if it refuses.
fn reified_holder_snapshot(
    elements: impl for<'gc> FnOnce(
        &mut Mutator<'gc>,
        ClassReference<'gc>,
        ClassReference<'gc>,
    ) -> Vec<Value<'gc>>,
) -> Result<Vec<u8>, String> {
    let (mut interpreter, class) = load(REIFIED_HOLDER);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let list = expect_ok(
        interpreter.send(&instance, Symbol::intern("listClass:"), &[platform]),
        "listClass:",
    );
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let list = class_reference(mutator, &list);
        let values = elements(mutator, class, list);
        let array = mutator.new_array(values);
        let instance = object(mutator.fetch(&instance));
        assert!(
            mutator
                .heap_mut()
                .instance_set_field(instance, 0, Value::object(array))
        );
        snapshot_one(mutator, true, "t.ns", class, &[Value::object(instance)])
    })
}

/// The mixin `class` applies.
fn applied_mixin<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Rc<Mixin> {
    heap.class_application(class)
        .expect("a class object")
        .mixin
        .clone()
}

/// The mixin `class`'s metaclass applies: its class side, declared or
/// synthesized.
fn class_side_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Rc<Mixin> {
    applied_mixin(heap, heap.class_metaclass(class).expect("a metaclass"))
}

/// The elements of the `held` slot of the loaded image's one root.
fn held_elements<'gc>(mutator: &Mutator<'gc>, root: &Root) -> Vec<Value<'gc>> {
    let holder = object(mutator.fetch(root));
    let held = object(holder_field(mutator, holder, 0));
    mutator
        .heap()
        .array_elements(held)
        .expect("held is an Array")
}

/// That `value`, decoded, is the reified mixin of `expected`, and the one
/// object the loaded heap answers for it.
fn assert_reifies<'gc>(mutator: &mut Mutator<'gc>, value: Value<'gc>, expected: &Rc<Mixin>) {
    let reified = object(value);
    let mixin = mutator
        .mixin_of_reified(reified)
        .expect("a reified mixin decodes as one");
    assert!(
        Rc::ptr_eq(&mixin, expected),
        "the mixin {}",
        expected.reflected_name()
    );
    assert!(
        mutator.reified_mixin(expected).same(reified),
        "the decoded object is the canonical one of {}",
        expected.reflected_name()
    );
}

/// A reified mixin travels as a reference to its mixin, so it loads onto
/// the loaded module's own: its class, its synthesized class side and a
/// nested class's declared class side, each the one object the loaded heap
/// answers for its mixin.
// spec: image.graph.record
#[test]
fn a_reified_mixin_round_trips_onto_the_same_mixin() {
    let bytes = reified_holder_snapshot(|mutator, class, _list| {
        let own = applied_mixin(mutator.heap(), class);
        let synthesized = class_side_of(mutator.heap(), class);
        let declared = mutator.heap().nested_mixins(&own)[0]
            .class_side
            .clone()
            .expect("Inner declares one");
        [own, synthesized, declared]
            .iter()
            .map(|mixin| Value::object(mutator.reified_mixin(mixin)))
            .collect()
    })
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    let declaration = loaded.modules[0].declaration.clone();
    let class = loaded.modules[0].class.clone();
    let root = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let elements = held_elements(mutator, &root);
        let class = class_reference(mutator, &class);
        assert!(Rc::ptr_eq(
            &applied_mixin(mutator.heap(), class),
            &declaration
        ));
        let synthesized = class_side_of(mutator.heap(), class);
        assert!(synthesized.is_class_side() && declaration.class_side.is_none());
        let declared = mutator.heap().nested_mixins(&declaration)[0]
            .class_side
            .clone()
            .expect("Inner declares one");
        assert_reifies(mutator, elements[0], &declaration);
        assert_reifies(mutator, elements[1], &synthesized);
        assert_reifies(mutator, elements[2], &declared);
    });
}

/// A shipped mixin is named by its class's declaration, so it loads onto
/// the loading binary's own mixin, a class side as much as a class.
// spec: image.graph.record
#[test]
fn a_reified_shipped_mixin_round_trips_by_name() {
    let bytes = reified_holder_snapshot(|mutator, _class, list| {
        let list_mixin = applied_mixin(mutator.heap(), list);
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let object_class_side = class_side_of(mutator.heap(), object_class);
        vec![
            Value::object(mutator.reified_mixin(&list_mixin)),
            Value::object(mutator.reified_mixin(&object_class_side)),
            Value::object(list),
        ]
    })
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let elements = held_elements(mutator, &root);
        let list = object(elements[2]);
        let list_mixin = applied_mixin(mutator.heap(), list);
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let object_class_side = class_side_of(mutator.heap(), object_class);
        assert_reifies(mutator, elements[0], &list_mixin);
        assert_reifies(mutator, elements[1], &object_class_side);
    });
}

/// The first class `CollectionsFactory`'s root declares that the shipped
/// world has not built, named as a class: what mirror navigation reaches
/// as a reified mixin before any program builds the class.
fn unbuilt_collections_class<'gc>(mutator: &Mutator<'gc>, list: ClassReference<'gc>) -> Rc<Mixin> {
    let root = applied_mixin(mutator.heap(), list)
        .enclosing_mixin()
        .expect("List is declared in its module's root");
    mutator
        .heap()
        .nested_mixins(&root)
        .into_iter()
        .find(|nested| {
            let name = ShippedName {
                module: Symbol::intern("CollectionsFactory"),
                declaration: nested.name,
                kind: ShippedKind::Class,
            };
            shipped_names::resolve_in(mutator, &[name], false).is_err()
        })
        .expect("a CollectionsFactory class that is not built yet")
}

/// A shipped mixin whose class no program has built yet is named by its
/// declaration all the same, so the loader builds the class before the
/// graph and the record loads onto that class's mixin, the loading
/// binary's own.
#[test]
fn a_reified_mixin_of_an_unbuilt_shipped_class_round_trips_by_name() {
    let mut declaration = None;
    let bytes = reified_holder_snapshot(|mutator, _class, list| {
        let unbuilt = unbuilt_collections_class(mutator, list);
        declaration = Some(unbuilt.name);
        vec![Value::object(mutator.reified_mixin(&unbuilt))]
    })
    .expect("snapshot");
    let name = ShippedName {
        module: Symbol::intern("CollectionsFactory"),
        declaration: declaration.expect("the snapshot ran"),
        kind: ShippedKind::Class,
    };
    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let elements = held_elements(mutator, &root);
        let resolved = match shipped_names::resolve_in(mutator, &[name], false) {
            Ok(resolved) => resolved,
            Err(error) => panic!("the loader built {}: {error}", name.describe()),
        };
        let mixin = applied_mixin(mutator.heap(), resolved[0].object);
        assert_eq!(mixin.name, name.declaration);
        assert_reifies(mutator, elements[0], &mixin);
    });
}

/// A module whose instance, made with the platform, holds mirrors in its
/// slots: a mixin mirror on its own class `Foo`, a class mirror on `List`,
/// a method mirror on `Foo`'s `compute`, an object mirror on a `Foo`, the
/// declaration mirror of `Foo`, a class mirror reached by navigation
/// on `Top`, the object mirror's `slots` group and its one slot mirror, the
/// declaration mirror of `Foo`'s slot, and the `platform mirrors` facet.
/// Its unary methods answer what the mirrors read.
const MIRROR_HOLDER: &str = indoc! {"
    class Holder usingPlatform: platform = Object new (
      |
        ClassMirror = platform mirrors ClassMirror.
        ObjectMirror = platform mirrors ObjectMirror.
        List = platform collections List.
        mixinMirror = (ClassMirror reflecting: Foo) mixin.
        listMirror = ClassMirror reflecting: List.
        methodMirror = mixinMirror methods findMirrorNamed: #compute.
        objectMirror = ObjectMirror reflecting: Foo new.
        declarationMirror = mixinMirror declaration.
        topMirror = topFrom: (ObjectMirror reflecting: 3) getClass.
        slotGroup = objectMirror slots.
        slotMirror = slotGroup findMirrorNamed: #count.
        slotDeclarationMirror = mixinMirror slots findMirrorNamed: #count.
        mirrors = platform mirrors.
      |
    ) (
      class Foo = Object new (
        |
          public count = 42.
        |
      ) (
        public compute = (
          ^6 * 7
        )
      )

      topFrom: aClassMirror = (
        |
          level ::= aClassMirror.
        |
        [nil == level superclass] whileFalse: [level:: level superclass].
        ^level
      )

      public mixinName = (
        ^mixinMirror name
      )

      public mixinIsFresh = (
        ^mixinMirror = (ClassMirror reflecting: Foo) mixin
      )

      public listIsList = (
        ^listMirror reflectee == List
      )

      public methodName = (
        ^methodMirror name
      )

      public methodSource = (
        ^methodMirror source
      )

      public declarationSource = (
        ^declarationMirror source
      )

      public objectCount = (
        ^objectMirror getSlot: #count
      )

      public topName = (
        ^topMirror name
      )

      public topIsTop = (
        ^topMirror = (ClassMirror reflecting: Object) superclass
      )

      public groupSize = (
        ^slotGroup size
      )

      public slotName = (
        ^slotMirror name
      )

      public slotValue = (
        ^slotMirror value reflectee
      )

      public slotDeclarationName = (
        ^slotDeclarationMirror name
      )

      public slotDeclarationIsMutable = (
        ^slotDeclarationMirror isMutable
      )

      public facetClassMirror = (
        ^mirrors ClassMirror == ClassMirror
      )

      public facetReflects = (
        ^((mirrors ObjectMirror reflecting: Foo new) getSlot: #count)
      )
    ) : ()
"};

/// A program holding mirrors survives a snapshot: built and loaded, with
/// its source and without, every mirror answers what it read before -- the
/// mixin mirror is equal to one reached afresh, since the reified mixin
/// loads onto the loaded mixin; the class mirror hands back the very
/// `List`; the method mirror and the object mirror read their method and
/// slot; the mirror navigation reached `Top` with, which holds no
/// class, still reads `Top`; the group, the slot mirror and the slot
/// declaration mirror still answer their size, name and value; and the
/// facet still hands out the module's classes. Without source, a
/// program's method and class declaration answer nil.
///
/// The `Top` part is the regression guard for a navigated mirror holding
/// the way to its class rather than the class: a mirror that kept `Top` in
/// a slot made the snapshot refuse `Top` as a value. No golden can guard
/// it, since a golden's image is built before `main:args:`, when no
/// program holds a mirror.
#[test]
fn a_program_holding_mirrors_loads_back_holding_them() {
    for with_source in [true, false] {
        let (mut interpreter, class) = load(MIRROR_HOLDER);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let bytes = interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            let instance = mutator.fetch(&instance);
            snapshot_one(mutator, with_source, "t.ns", class, &[instance])
                .unwrap_or_else(|error| panic!("snapshot (source {with_source}): {error}"))
        });
        let mut loaded = image::load(&bytes).expect("load");
        let holder = loaded.roots[0].clone();
        let mut answer = |selector: &str| {
            let root = expect_ok(
                loaded
                    .interpreter
                    .send(&holder, Symbol::intern(selector), &[]),
                selector,
            );
            loaded.interpreter.enter(|mutator| {
                let value = mutator.fetch(&root);
                if value.is_nil() {
                    return "nil".to_string();
                }
                if let Some(boolean) = value.as_boolean() {
                    return boolean.to_string();
                }
                if let Some(integer) = value.as_small_integer() {
                    return integer.to_string();
                }
                text_or_class_name(mutator, value)
            })
        };
        assert_eq!(answer("mixinName"), "Foo");
        assert_eq!(answer("mixinIsFresh"), "true");
        assert_eq!(answer("listIsList"), "true");
        assert_eq!(answer("methodName"), "compute");
        assert_eq!(answer("objectCount"), "42");
        assert_eq!(answer("topName"), "Top");
        assert_eq!(answer("topIsTop"), "true");
        assert_eq!(answer("groupSize"), "1");
        assert_eq!(answer("slotName"), "count");
        assert_eq!(answer("slotValue"), "42");
        assert_eq!(answer("slotDeclarationName"), "count");
        assert_eq!(answer("slotDeclarationIsMutable"), "false");
        assert_eq!(answer("facetClassMirror"), "true");
        assert_eq!(answer("facetReflects"), "42");
        if with_source {
            assert_eq!(
                answer("methodSource"),
                "public compute = (\n      ^6 * 7\n    )"
            );
            assert!(
                answer("declarationSource").starts_with("class Foo = Object new ("),
                "the declaration's text"
            );
        } else {
            assert_eq!(answer("methodSource"), "nil");
            assert_eq!(answer("declarationSource"), "nil");
        }
    }
}

/// The VM mirror's class has no declaration and so no name; its mixin, and
/// that mixin's class side, are refused as the class is (lock (5)).
#[test]
fn the_mirror_class_mixin_is_never_written() {
    for class_side in [false, true] {
        let refusal = reified_holder_snapshot(|mutator, _class, _list| {
            let mirror_class = mutator.builtin_class(BuiltinClass::VmMirror);
            let mixin = if class_side {
                class_side_of(mutator.heap(), mirror_class)
            } else {
                applied_mixin(mutator.heap(), mirror_class)
            };
            vec![Value::object(mutator.reified_mixin(&mixin))]
        });
        match refusal {
            Ok(_) => panic!("a reified mixin of the VM mirror's class was written"),
            Err(error) => assert!(
                error.contains("refuses the mixin of the VM mirror's class"),
                "{error}"
            ),
        }
    }
}

/// `inspect` reads a reified mixin's record as the loader does: a row or a
/// shipped name, and the side.
#[test]
fn inspect_shows_a_reified_mixin() {
    let bytes = reified_holder_snapshot(|mutator, class, list| {
        let own = applied_mixin(mutator.heap(), class);
        let synthesized = class_side_of(mutator.heap(), class);
        let list_mixin = applied_mixin(mutator.heap(), list);
        [own, synthesized, list_mixin]
            .iter()
            .map(|mixin| Value::object(mutator.reified_mixin(mixin)))
            .collect()
    })
    .expect("snapshot");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    for expected in [
        "Kernel Mixin, hash 0, mixin row 0",
        "Kernel Mixin, hash 0, mixin row 0, class side",
        "Kernel Mixin, hash 0, mixin CollectionsFactory List",
    ] {
        assert!(
            report
                .graph_objects
                .iter()
                .any(|object| object.ends_with(expected)),
            "{expected}: {:?}",
            report.graph_objects
        );
    }
}

/// Three typed arrays over one `ByteArray`, a big-endian `DataView` over
/// another, and a `StringBuilder` holding code points of two, three and
/// four bytes, each in a slot; `report` prints what they answer.
const VIEWS_AND_A_BUILDER: &str = indoc! {"
    class X = Object new (
      |
        public halves
        public floats
        public octets
        public view
        public builder
      |
    ) (
      public fill: platform = (
        |
          kernel = platform kernel.
          typedArrays = platform typedArrays.
          bytes = ByteArray new: 8.
          other = ByteArray new: 8.
        |
        halves:: typedArrays Int16Array on: bytes.
        floats:: typedArrays Float32Array on: bytes.
        octets:: typedArrays Uint8Array on: bytes.
        view:: kernel DataView on: other byteOrder: #bigEndian.
        builder:: kernel StringBuilder new: 2.
        halves at: 1 put: -2.
        floats at: 2 put: 1.5.
        view int32At: 1 put: -123456789.
        view int32At: 5 put: 16909060.
        builder add: 'grüße '.
        builder addCodePoint: 8364.
        builder addCodePoint: 128512.
      )

      public report = (
        |
          text ::= ''.
        |
        {
          halves at: 1.
          halves size.
          floats at: 2.
          floats size.
          octets at: 1.
          octets at: 8.
          octets size.
          view int32At: 1.
          view int32At: 5.
          view uint8At: 5.
          view size.
          view byteOrder.
          builder asString.
          builder size.
        } do: [:each | text:: text , each printString , ' '].
        ^text
      )
    ) : ()
"};

/// What `VIEWS_AND_A_BUILDER`'s `report` answers once `fill:` has run: -2
/// is the bytes 254 255, 1.5 in single precision ends in the byte 63, and
/// 16909060 is the bytes 1 2 3 4 in big-endian order.
const VIEWS_AND_A_BUILDER_REPORT: &str = "-2 4 1.5 2 254 63 8 -123456789 16909060 1 8 #bigEndian \
    'grüße €😀' 8 ";

/// The `printString` of what `selector` answers when sent to `receiver`
/// with `arguments`.
fn printed(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[Root],
) -> String {
    let answer = expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), arguments),
        selector,
    );
    let text = expect_ok(
        interpreter.send(&answer, Symbol::intern("printString"), &[]),
        "printString",
    );
    interpreter.enter(|mutator| text_or_class_name(mutator, mutator.fetch(&text)))
}

/// What `receiver`'s unary `selector` answers, rooted.
fn answer_of(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> Root {
    expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), &[]),
        selector,
    )
}

/// The integer `integer`, rooted, as an argument of a host's send.
fn integer_root(interpreter: &mut Interpreter, integer: i64) -> Root {
    interpreter.enter(|mutator| mutator.stash(Value::small_integer(integer)))
}

/// A new String of `text`, rooted, as an argument of a host's send.
fn string_root(interpreter: &mut Interpreter, text: &str) -> Root {
    interpreter.enter(|mutator| {
        let string = mutator.new_string(text.to_string());
        mutator.stash(Value::object(string))
    })
}

/// A view keeps the very `ByteArray` it was made over, and a builder its
/// bytes and counts, as plain slots: a snapshot carries them as records of
/// the shipped classes, and the loaded views still share their storage.
#[test]
fn a_view_and_a_builder_round_trip_through_a_package() {
    let (mut interpreter, class) = load(VIEWS_AND_A_BUILDER);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    expect_ok(
        interpreter.send(
            &instance,
            Symbol::intern("fill:"),
            std::slice::from_ref(&platform),
        ),
        "fill:",
    );
    let before = answer_of(&mut interpreter, &instance, "report");
    let before = interpreter.enter(|mutator| text_or_class_name(mutator, mutator.fetch(&before)));
    assert_eq!(before, VIEWS_AND_A_BUILDER_REPORT);
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    let interpreter = &mut loaded.interpreter;
    let after = answer_of(interpreter, &root, "report");
    let after = interpreter.enter(|mutator| text_or_class_name(mutator, mutator.fetch(&after)));
    assert_eq!(after, before);

    let halves = answer_of(interpreter, &root, "halves");
    let floats = answer_of(interpreter, &root, "floats");
    let octets = answer_of(interpreter, &root, "octets");
    let view = answer_of(interpreter, &root, "view");
    let builder = answer_of(interpreter, &root, "builder");
    let one = integer_root(interpreter, 1);
    let two = integer_root(interpreter, 2);
    let five = integer_root(interpreter, 5);
    assert_eq!(
        printed(interpreter, &halves, "at:", std::slice::from_ref(&one)),
        "-2"
    );
    assert_eq!(printed(interpreter, &halves, "size", &[]), "4");
    assert_eq!(
        printed(interpreter, &view, "int32At:", std::slice::from_ref(&one)),
        "-123456789"
    );
    assert_eq!(
        printed(interpreter, &view, "int32At:", std::slice::from_ref(&five)),
        "16909060"
    );
    assert_eq!(printed(interpreter, &view, "size", &[]), "8");
    assert_eq!(
        printed(interpreter, &builder, "asString", &[]),
        "'grüße €😀'"
    );
    assert_eq!(printed(interpreter, &builder, "size", &[]), "8");

    // A write through one decoded view is a write to the one ByteArray the
    // others read: 258 is the bytes 2 1, and 64 as the last byte makes the
    // second float 6.0.
    let two_hundred_fifty_eight = integer_root(interpreter, 258);
    printed(
        interpreter,
        &halves,
        "at:put:",
        &[one.clone(), two_hundred_fifty_eight],
    );
    assert_eq!(
        printed(interpreter, &octets, "at:", std::slice::from_ref(&one)),
        "2"
    );
    assert_eq!(
        printed(interpreter, &octets, "at:", std::slice::from_ref(&two)),
        "1"
    );
    let eight = integer_root(interpreter, 8);
    let sixty_four = integer_root(interpreter, 64);
    printed(interpreter, &octets, "at:put:", &[eight, sixty_four]);
    assert_eq!(
        printed(interpreter, &floats, "at:", std::slice::from_ref(&two)),
        "6.0"
    );
    let under_halves = answer_of(interpreter, &halves, "bytes");
    let under_octets = answer_of(interpreter, &octets, "bytes");
    let under_floats = answer_of(interpreter, &floats, "bytes");
    let shared = interpreter.enter(|mutator| {
        let under_halves = object(mutator.fetch(&under_halves));
        object(mutator.fetch(&under_octets)).same(under_halves)
            && object(mutator.fetch(&under_floats)).same(under_halves)
    });
    assert!(shared, "the three views share one decoded ByteArray");

    // The decoded builder carries on where it stopped.
    let more = string_root(interpreter, "ö");
    printed(interpreter, &builder, "add:", std::slice::from_ref(&more));
    assert_eq!(
        printed(interpreter, &builder, "asString", &[]),
        "'grüße €😀ö'"
    );
    assert_eq!(printed(interpreter, &builder, "size", &[]), "9");
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
                mutator.heap_mut().object_ensure_identity_hash(first);
                mutator.heap_mut().object_ensure_identity_hash(second);
                vec![
                    mutator.stash(Value::object(second)),
                    mutator.stash(Value::object(first)),
                ]
            })
        });
    interpreter.enter(|mutator| {
        let original_0 = mutator.fetch(&original[0]);
        let original_1 = mutator.fetch(&original[1]);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let decoded_1 = mutator.fetch(&decoded[1]);
        let heap = mutator.heap();
        let higher = heap.object_identity_hash(object(original_0));
        let lower = heap.object_identity_hash(object(original_1));
        assert!(higher > lower, "hashes are handed out in request order");
        assert_eq!(heap.object_identity_hash(object(decoded_0)), higher);
        assert_eq!(heap.object_identity_hash(object(decoded_1)), lower);
        assert_eq!(max, higher);
    });
}

#[test]
fn class_references_resolve_to_the_same_live_class_objects() {
    let (mut interpreter, class, _original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, class| {
            interpreter.enter(|mutator| {
                let builtin = mutator.builtin_class(BuiltinClass::Integer);
                vec![mutator.stash(Value::object(builtin)), class.clone()]
            })
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let decoded_1 = mutator.fetch(&decoded[1]);
        let builtin = mutator.builtin_class(BuiltinClass::Integer);
        let class = class_reference(mutator, &class);
        assert!(object(decoded_0).same(builtin));
        assert!(object(decoded_1).same(class));
    });
}

/// The platform is an ordinary record of `PlatformFactory Platform`: it
/// decodes to a second object of the loading platform's own class, equal to
/// it, since a lazy slot is no part of a value's state.
#[test]
fn the_platform_decodes_to_a_record_equal_to_the_interpreters_own() {
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(EMPTY, |interpreter, _| vec![interpreter.platform()]);
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let decoded_0 = object(mutator.fetch(&decoded[0]));
        let platform = object(mutator.fetch(&platform));
        let heap = mutator.heap();
        assert!(!decoded_0.same(platform), "a record, not the singleton");
        assert!(
            heap.object_class(decoded_0)
                .same(heap.object_class(platform))
        );
    });
    let equal = expect_ok(
        interpreter.send(&decoded[0], Symbol::intern("="), &[platform]),
        "decoded = platform",
    );
    interpreter.enter(|mutator| {
        assert!(matches!(
            mutator.fetch(&equal).kind(),
            ValueKind::Boolean(true)
        ));
    });
}

#[test]
fn an_instance_of_the_root_class_keeps_its_class_and_fields() {
    let source = indoc! {"
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
        assert!(heap.object_class(object(decoded_0)).same(class));
        assert_eq!(heap.instance_field_count(object(decoded_0)), Some(2));
        let field = |index| {
            heap.instance_field(object(decoded_0), index)
                .expect("expected an instance")
        };
        assert!(field(0).as_small_integer() == Some(1));
        assert!(matches!(
            heap.str_value(object(field(1))).as_deref(),
            Some("two")
        ));
    });
}

// spec: image.graph.record
#[test]
fn a_closure_and_its_paused_home_frame_round_trip() {
    let source = indoc! {"
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
            // and its last register before snapshotting. `mk` reads nothing
            // by reference, so its registers were not kept, but its frame
            // has its code's full register file, every one nil.
            interpreter.enter(|mutator| {
                let closure_value = mutator.fetch(&closure);
                let home = mutator
                    .heap()
                    .closure_home(object(closure_value))
                    .expect("expected a closure");
                let (_, argument_count, register_count) = mutator.heap().frame_register_shape(home);
                assert_eq!(argument_count, 0, "`mk` takes no argument");
                assert!(register_count > 0, "`mk` has a register to write");
                // Its last register: every register field past the
                // arguments is a register above the header.
                mutator.heap_mut().frame_set_local(
                    home,
                    register_count as u16 - 1 + HEADER_REGISTERS,
                    Value::small_integer(9),
                );
                mutator.heap_mut().frame_set_program_counter(home, 1);
            });
            vec![closure]
        });
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let original_0 = mutator.fetch(&original[0]);
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let original_code = heap
            .closure_code(object(original_0))
            .expect("expected a closure");
        let code = heap
            .closure_code(object(decoded_0))
            .expect("expected a closure");
        let captured_frame = heap
            .closure_captured_frame(object(decoded_0))
            .expect("expected a closure");
        let closure_home = heap
            .closure_home(object(decoded_0))
            .expect("expected a closure");
        assert_eq!(*code, *original_code);
        assert!(
            captured_frame.same(closure_home),
            "captured_frame and home were the same frame"
        );
        let home = heap.frame_record(closure_home);
        assert_eq!(home.program_counter, 1);
        let (_, argument_count, register_count) = heap.frame_register_shape(closure_home);
        assert_eq!(register_count - argument_count, home.temporaries.len());
        assert_eq!(
            heap.frame_local(closure_home, register_count as u16 - 1 + HEADER_REGISTERS)
                .as_small_integer(),
            Some(9)
        );
        assert!(home.uncontinuable, "`mk` had returned");
        let mixin = heap
            .class_application(class)
            .expect("not a class")
            .mixin
            .clone();
        let FrameKind::Method(declaration) = &home.kind else {
            panic!("home is a method frame")
        };
        // `round_trip_graph` decodes against the original declarations and a
        // freshly decoded code table, so the frame's method is built over the
        // decoded body the record names rather than shared with the class's.
        let method = mixin
            .compiled_method(Symbol::intern("mk"))
            .expect("the class declares `mk`");
        assert_eq!(declaration.selector, method.selector);
        assert_eq!(*declaration.code, *method.code);
        assert!(
            heap.object_class(object(heap.frame_receiver(closure_home)))
                .same(class)
        );
        assert!(
            home.current_class
                .as_ref()
                .expect("a method frame has a current class")
                .same(class)
        );
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
            .instance_field(object(decoded_0), 0)
            .expect("expected an instance");
        let home_kind = heap.frame_kind(
            heap.closure_home(object(held))
                .expect("slot holds a closure"),
        );
        let FrameKind::Initializer(home_class, body) = &home_kind else {
            panic!("home is the initializer frame")
        };
        assert!(home_class.same(class));
        assert_eq!(
            *body,
            ns_runtime::interpreter::instantiate::InitializerBody::Full,
            "the class's own level runs its full initializer"
        );
    });
}

#[test]
fn the_graph_section_encoding_is_deterministic() {
    let source = indoc! {"
        class X = Object new (
          | a = 1. blk = [ 2 ]. s = 'three'. |
        ) (
        ) : ()
    "};
    let mut encodings = Vec::new();
    for _ in 0..2 {
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // Read out after the last run, for the encoder, which runs nothing.
        let bytes = interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            let roots = vec![mutator.fetch(&instance)];
            let mut symbols = symbols::Builder::new();
            let mut writer = encode::Writer::new();
            let ids = encode::encode_code_table(
                &mut writer,
                &rows_of(mutator.heap(), class),
                &[],
                &mut symbols,
            )
            .expect("encode code table");
            let mixin_ids = mixin_ids_for(mutator, class, &mut symbols, &ids);
            let walked = encode::walk_graph(mutator, &roots, &[class]).expect("walk graph");
            encode::encode_graph_section(
                mutator,
                &mut writer,
                &walked,
                &roots,
                &ids,
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

/// The shipped names every hand-written graph in this file may use, by
/// their index in the section a fixture decodes against: a module, a
/// declaration and a kind byte as the section writes it (0 a module
/// instance, 1 a class, 2 a metaclass).
const FIXTURE_NAMES: &[(&str, &str, u8)] = &[
    ("Kernel", "Object", 1),                 // OBJECT_NAME = 0
    ("Kernel", "Array", 1),                  // ARRAY_NAME = 1
    ("Kernel", "Activation", 1),             // ACTIVATION_NAME = 2
    ("Kernel", "Metaclass", 1),              // METACLASS_NAME = 3
    ("Kernel", "String", 1),                 // STRING_NAME = 4
    ("Kernel", "Symbol", 1),                 // SYMBOL_NAME = 5
    ("Kernel", "Integer", 1),                // INTEGER_NAME = 6
    ("Kernel", "Fraction", 1),               // FRACTION_NAME = 7
    ("Kernel", "FutureImplementation", 1),   // FUTURE_IMPLEMENTATION_NAME = 8
    ("Kernel", "Value", 1),                  // VALUE_NAME = 9
    ("Kernel", "Top", 1),                    // TOP_NAME = 10
    ("Kernel", "Integer", 2),                // INTEGER_CLASS_NAME = 11
    ("Kernel", "Kernel", 0),                 // KERNEL_INSTANCE_NAME = 12
    ("Kernel", "Kernel", 1),                 // KERNEL_ROOT_NAME = 13
    ("TimeFactory", "Stopwatch", 1),         // STOPWATCH_NAME = 14
    ("Kernel", "Behavior", 1),               // BEHAVIOR_NAME = 15
    ("Kernel", "Closure", 1),                // CLOSURE_NAME = 16
    ("Kernel", "Class", 1),                  // CLASS_NAME = 17
    ("Kernel", "Activation", 2),             // ACTIVATION_CLASS_NAME = 18
    ("Kernel", "FutureImplementation", 2),   // FUTURE_IMPLEMENTATION_CLASS_NAME = 19
    ("Kernel", "NonBooleanReceiver", 1),     // NON_BOOLEAN_RECEIVER_NAME = 20
    ("Kernel", "ByteArray", 1),              // BYTE_ARRAY_NAME = 21
    ("TypedArraysFactory", "Int16Array", 1), // INT16_ARRAY_NAME = 22
    ("Kernel", "BigEndianDataView", 1),      // BIG_ENDIAN_DATA_VIEW_NAME = 23
    ("Kernel", "StringBuilder", 1),          // STRING_BUILDER_NAME = 24
    ("Kernel", "IndexedCollection", 1),      // INDEXED_COLLECTION_NAME = 25
    ("Kernel", "WeakArray", 1),              // WEAK_ARRAY_NAME = 26
    ("Kernel", "Ephemeron", 1),              // EPHEMERON_NAME = 27
    ("Kernel", "Mixin", 1),                  // MIXIN_NAME = 28
];
const OBJECT_NAME: u32 = 0;
const ARRAY_NAME: u32 = 1;
const ACTIVATION_NAME: u32 = 2;
const METACLASS_NAME: u32 = 3;
const STRING_NAME: u32 = 4;
const SYMBOL_NAME: u32 = 5;
const INTEGER_NAME: u32 = 6;
const FRACTION_NAME: u32 = 7;
const FUTURE_IMPLEMENTATION_NAME: u32 = 8;
const VALUE_NAME: u32 = 9;
const TOP_NAME: u32 = 10;
const INTEGER_CLASS_NAME: u32 = 11;
const KERNEL_INSTANCE_NAME: u32 = 12;
const KERNEL_ROOT_NAME: u32 = 13;
const STOPWATCH_NAME: u32 = 14;
const BEHAVIOR_NAME: u32 = 15;
const CLOSURE_NAME: u32 = 16;
const CLASS_NAME: u32 = 17;
const ACTIVATION_CLASS_NAME: u32 = 18;
const FUTURE_IMPLEMENTATION_CLASS_NAME: u32 = 19;
const NON_BOOLEAN_RECEIVER_NAME: u32 = 20;
const BYTE_ARRAY_NAME: u32 = 21;
const INT16_ARRAY_NAME: u32 = 22;
const BIG_ENDIAN_DATA_VIEW_NAME: u32 = 23;
const STRING_BUILDER_NAME: u32 = 24;
const INDEXED_COLLECTION_NAME: u32 = 25;
const WEAK_ARRAY_NAME: u32 = 26;
const EPHEMERON_NAME: u32 = 27;
const MIXIN_NAME: u32 = 28;

/// A section entry's kind byte as the shipped name it stands for.
fn fixture_kind(kind: u8) -> ShippedKind {
    match kind {
        0 => ShippedKind::ModuleInstance,
        1 => ShippedKind::Class,
        2 => ShippedKind::Metaclass,
        _ => panic!("a fixture name is a module instance, a class or a metaclass"),
    }
}

/// `FIXTURE_NAMES` as shipped names, in section order.
fn fixture_names() -> Vec<ShippedName> {
    FIXTURE_NAMES
        .iter()
        .map(|(module, declaration, kind)| ShippedName {
            module: Symbol::intern(module),
            declaration: Symbol::intern(declaration),
            kind: fixture_kind(*kind),
        })
        .collect()
}

/// Builds what `FIXTURE_NAMES` names that `interpreter`'s boot has not
/// built, as `image::load` does before it resolves a section, so that the
/// fixture's names resolve against its world.
fn build_fixture_names(interpreter: &mut Interpreter) {
    if let Err(error) = interpreter.build_shipped_names(&fixture_names(), false) {
        panic!("the fixture's names are the binary's: {error}");
    }
}

/// A class reference to the shipped name at `index` of the fixture's
/// section.
fn shipped(writer: &mut encode::Writer, index: u32) {
    writer.write_u8(CLASS_SHIPPED);
    writer.write_u32(index);
}

/// `shipped` as a method, for a writer held by value or by reference alike.
trait WriteShipped {
    fn write_shipped(&mut self, index: u32);
}

impl WriteShipped for encode::Writer {
    fn write_shipped(&mut self, index: u32) {
        shipped(self, index);
    }
}

/// A world whose graph decoder can be fed hand-crafted bytes: an empty
/// class, no compiled code, and the shipped names of `FIXTURE_NAMES`.
struct GraphFixture {
    interpreter: Interpreter,
    codes: Vec<Rc<CompiledCode>>,
    /// Empty by default: most fixtures' hand-crafted bytes never reference a
    /// selector. Tests that do (a frame kind's method name, a closure's
    /// parameters) set this to a table built by `table_with` first.
    symbols: symbols::Table,
    /// `EMPTY`'s one declaration, so mixin id 0 is in range and every
    /// larger id is not.
    mixins: Vec<Rc<Mixin>>,
    /// The section the bytes are decoded against, resolved against the
    /// fixture's own world inside each entry: `FIXTURE_NAMES`.
    names: Vec<ShippedName>,
    /// The object ids the module section would name the module roots by:
    /// empty by default, since most hand-crafted graphs carry no root class
    /// record; one whose method frame has no current class names its own.
    module_roots: Vec<u32>,
}

impl GraphFixture {
    fn new() -> GraphFixture {
        GraphFixture::with_source(EMPTY)
    }

    /// A fixture around `source`'s declarations, for a hand-crafted section
    /// that names rows `EMPTY` does not have.
    fn with_source(source: &str) -> GraphFixture {
        let (mut interpreter, class) = load(source);
        let mixins = interpreter
            .enter(|mutator| mixin_preorder(mutator.heap(), class_reference(mutator, &class)));
        build_fixture_names(&mut interpreter);
        GraphFixture {
            interpreter,
            codes: Vec::new(),
            symbols: symbols::Table::empty(),
            mixins,
            names: fixture_names(),
            module_roots: Vec::new(),
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
        let codes = &self.codes;
        let mixins = &self.mixins;
        let symbols = &self.symbols;
        let names = &self.names;
        let module_roots = &self.module_roots;
        self.interpreter.enter(|mutator| {
            let shipped = shipped_names::resolve_in(mutator, names, false)
                .expect("the fixture's names resolve");
            let decoded = {
                let mut context = decode::DecodeContext {
                    mutator,
                    module_roots,
                    codes,
                    mixins,
                    shipped: &shipped,
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

/// `object_count`, the count every graph section starts with. Frames are
/// object records, so it counts them too.
fn graph_counts(writer: &mut encode::Writer, objects: u32) {
    writer.write_u32(objects);
}

/// A frame record's header: `Kernel Activation`'s shipped name -- the one
/// header a frame record may carry -- the identity hash `hash`, and
/// `OBJECT_FRAME`.
fn frame_header(writer: &mut encode::Writer, hash: u32) {
    shipped(writer, ACTIVATION_NAME);
    writer.write_u32(hash);
    writer.write_u8(OBJECT_FRAME); // object body: Frame
}

/// A frame record's body for a frame that runs none, a top-level one.
fn no_body(writer: &mut encode::Writer) {
    writer.write_u8(FRAME_BODY_NONE);
}

/// A frame record's body: code id `code_id`.
fn runs_body(writer: &mut encode::Writer, code_id: u32) {
    writer.write_u8(FRAME_BODY_CODE);
    writer.write_u32(code_id);
}

/// Everything of a frame record after its kind and its body: `program_counter`,
/// uncontinuable, a nil receiver, no sender, no home, no current class, and
/// `registers` registers, each nil.
fn frame_tail(writer: &mut encode::Writer, program_counter: u32, registers: u32) {
    frame_tail_marked(writer, program_counter, registers, true, None);
}

/// `frame_tail` with the `uncontinuable` flag and the home (a frame's object
/// id) left to the caller.
fn frame_tail_marked(
    writer: &mut encode::Writer,
    program_counter: u32,
    registers: u32,
    uncontinuable: bool,
    home: Option<u32>,
) {
    writer.write_u32(program_counter);
    writer.write_boolean(uncontinuable);
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_boolean(false); // no sender
    match home {
        Some(frame) => {
            writer.write_boolean(true);
            writer.write_u32(frame);
        }
        None => writer.write_boolean(false),
    }
    writer.write_boolean(false); // no current class
    writer.write_u32(registers);
    for _ in 0..registers {
        writer.write_u8(VALUE_NIL); // each register: nil
    }
}

/// A one-object graph section whose single object is an array of one
/// element, written by `element`.
fn one_array_object(element: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    shipped(&mut writer, ARRAY_NAME);
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

/// A frame names its sender by object id, range-checked like every other.
#[test]
fn a_frame_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    writer.write_u32(0); // program_counter
    writer.write_boolean(true); // uncontinuable
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_boolean(true); // has a sender...
    writer.write_u32(99); // ...whose id is out of range
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer.write_u32(0); // no registers
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::BadObjectId(99));
    assert_eq!(error.to_string(), "object id 99 out of range");
}

/// A frame reference whose id is in range but names a record that is not a
/// frame record is refused where it is read: every frame record's
/// placeholder is a frame object from the start, so the decoder knows.
#[test]
fn a_frame_reference_naming_a_string_is_refused() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2);
    writer.write_shipped(STRING_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_STRING); // object body: String
    writer.write_str("not a frame");
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    writer.write_u32(0); // program_counter
    writer.write_boolean(true); // uncontinuable
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_boolean(true); // has a sender...
    writer.write_u32(0); // ...object 0, the string
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer.write_u32(0); // no registers
    writer.write_u32(0); // no roots
    let error = GraphFixture::new().decode_error(&writer.into_bytes());
    assert_eq!(error, decode::DecodeError::NotAFrameReference(0));
    assert_eq!(
        error.to_string(),
        "object 0 is named as a frame but is not a frame record"
    );
}

// spec: image.verify.counts
#[test]
fn a_huge_object_count_does_not_allocate_gigabytes() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, u32::MAX);
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::UnexpectedEndOfFile
    );
}

/// A frame record carries its register count, and a count the bytes left
/// cannot hold is refused by the first pass, before any record -- this one
/// included -- is allocated.
// spec: image.verify.counts
#[test]
fn a_frame_record_whose_register_count_would_exhaust_the_bytes_is_refused_before_allocating() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    writer.write_u32(0); // program_counter
    writer.write_boolean(true); // uncontinuable
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_boolean(false); // no sender
    writer.write_boolean(false); // no home
    writer.write_boolean(false); // no current class
    writer.write_u32(u32::MAX); // registers, none of them written
    let bytes = writer.into_bytes();
    let mut fixture = GraphFixture::new();
    let before = fixture.interpreter.heap_statistics().live_objects;
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::UnexpectedEndOfFile
    );
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

/// The premise of the load's amplification bound: the first pass sizes
/// every record before anything is allocated, so an image whose declared
/// sizes exceed what its bytes can hold is refused with nothing allocated --
/// not even the well-formed record ahead of the one that overclaims.
// spec: image.verify.counts
#[test]
fn the_first_pass_refuses_a_record_count_the_bytes_cannot_hold() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2);
    writer.write_shipped(ARRAY_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_ARRAY); // object body: Array
    writer.write_u32(0); // no elements: a record that fits
    writer.write_shipped(ARRAY_NAME);
    writer.write_u32(2); // identity hash
    writer.write_u8(OBJECT_ARRAY); // object body: Array
    writer.write_u32(1_000_000); // a million elements...
    for _ in 0..8 {
        writer.write_u8(VALUE_NIL); // ...of which eight are written
    }
    let bytes = writer.into_bytes();
    let mut fixture = GraphFixture::new();
    let before = fixture.interpreter.heap_statistics().live_objects;
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::UnexpectedEndOfFile
    );
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

/// A one-record graph section: a `ByteArray` record whose length field
/// says `length` and after which only `written` bytes follow.
fn one_byte_array_record(length: u32, written: usize) -> Vec<u8> {
    class_records(1, |writer| {
        writer.write_shipped(BYTE_ARRAY_NAME);
        writer.write_u32(1); // identity hash
        writer.write_u8(OBJECT_BYTE_ARRAY); // object body: ByteArray
        writer.write_u32(length);
        for byte in 0..written {
            writer.write_u8(byte as u8);
        }
    })
}

#[test]
fn a_truncated_byte_array_record_is_refused_before_allocating() {
    let bytes = one_byte_array_record(1_000_000, 8); // a million bytes, eight written
    let mut fixture = GraphFixture::new();
    let before = fixture.interpreter.heap_statistics().live_objects;
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::UnexpectedEndOfFile
    );
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

#[test]
fn an_overlong_byte_array_length_is_refused() {
    let bytes = one_byte_array_record(u32::MAX, 16);
    let mut fixture = GraphFixture::new();
    let before = fixture.interpreter.heap_statistics().live_objects;
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::UnexpectedEndOfFile
    );
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

#[test]
fn a_crafted_byte_array_record_decodes_to_its_bytes() {
    let bytes = one_byte_array_record(11, 11);
    GraphFixture::new().decoded(&bytes, "a ByteArray record", |heap, graph| {
        let decoded = graph.objects[0];
        assert!(heap.is_byte_array(decoded));
        assert_eq!(heap.byte_array_bytes(decoded), Some((0..11).collect()));
        assert_eq!(heap.object_identity_hash(decoded), 1);
    });
}

/// A record carries exactly its class's layout: a byte-array record under
/// any class but `ByteArray` would be an object of that class whose
/// methods read fields it does not have, and a record of another body
/// under `ByteArray`'s header one whose leaves find no bytes. Both are
/// refused by the first pass, before anything is allocated.
#[test]
fn a_byte_array_record_and_the_byte_array_class_go_together() {
    let byte_array_under = |header: u32| {
        class_records(1, |writer| {
            writer.write_shipped(header);
            writer.write_u32(1); // identity hash
            writer.write_u8(OBJECT_BYTE_ARRAY);
            writer.write_u32(2); // two bytes
            writer.write_u8(7);
            writer.write_u8(9);
        })
    };
    let array_under_byte_array = class_records(1, |writer| {
        writer.write_shipped(BYTE_ARRAY_NAME);
        writer.write_u32(1); // identity hash
        writer.write_u8(OBJECT_ARRAY);
        writer.write_u32(0); // no elements
    });
    let under_another_class = "a byte-array record has a class other than ByteArray".to_string();
    for (bytes, body, text) in [
        (
            byte_array_under(ARRAY_NAME),
            OBJECT_BYTE_ARRAY,
            under_another_class.clone(),
        ),
        (
            byte_array_under(OBJECT_NAME),
            OBJECT_BYTE_ARRAY,
            under_another_class,
        ),
        (
            array_under_byte_array,
            OBJECT_ARRAY,
            format!(
                "a record of the class ByteArray has object body {OBJECT_ARRAY}, but a \
                 ByteArray is a byte-array record"
            ),
        ),
    ] {
        let mut fixture = GraphFixture::new();
        let before = fixture.interpreter.heap_statistics().live_objects;
        let error = fixture.decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::ByteArrayRecordMismatch { body });
        assert_eq!(error.to_string(), text);
        assert_eq!(
            fixture.interpreter.heap_statistics().live_objects,
            before,
            "nothing was allocated"
        );
    }
}

/// A one-record graph section: an `OBJECT_WEAK_ARRAY` or
/// `OBJECT_EPHEMERON` record of `body` under `header`, its count `count`,
/// then `values` nils.
fn one_weak_record(header: u32, body: u8, count: u32, values: usize) -> Vec<u8> {
    class_records(1, |writer| {
        writer.write_shipped(header);
        writer.write_u32(1); // identity hash
        writer.write_u8(body);
        writer.write_u32(count);
        for _ in 0..values {
            writer.write_u8(VALUE_NIL);
        }
    })
}

/// `bytes` is refused with `expected`, its text `text`, and nothing
/// allocated: every refusal below is the first pass's.
fn refused_before_allocating(bytes: &[u8], expected: &decode::DecodeError, text: &str) {
    refused_in(&mut GraphFixture::new(), bytes, expected, text);
}

/// `refused_before_allocating` against `fixture`.
fn refused_in(
    fixture: &mut GraphFixture,
    bytes: &[u8],
    expected: &decode::DecodeError,
    text: &str,
) {
    let before = fixture.interpreter.heap_statistics().live_objects;
    let error = fixture.decode_error(bytes);
    assert_eq!(&error, expected);
    assert_eq!(error.to_string(), text);
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

#[test]
fn a_crafted_weak_array_and_ephemeron_decode_to_their_values() {
    let bytes = class_records(2, |writer| {
        writer.write_shipped(WEAK_ARRAY_NAME);
        writer.write_u32(1); // identity hash
        writer.write_u8(OBJECT_WEAK_ARRAY);
        writer.write_u32(2);
        writer.write_u8(VALUE_OBJECT_ID);
        writer.write_u32(1);
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(5);
        writer.write_shipped(EPHEMERON_NAME);
        writer.write_u32(2); // identity hash
        writer.write_u8(OBJECT_EPHEMERON);
        writer.write_u32(3);
        writer.write_u8(VALUE_OBJECT_ID); // the key: the weak array
        writer.write_u32(0);
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(6);
        writer.write_u8(VALUE_NIL);
    });
    GraphFixture::new().decoded(&bytes, "a weak array and an ephemeron", |heap, graph| {
        let weak = graph.objects[0];
        let ephemeron = graph.objects[1];
        let elements = heap.weak_array_elements(weak).expect("a weak array");
        assert!(same_value_object(elements[0], Value::object(ephemeron)));
        assert_eq!(elements[1].as_small_integer(), Some(5));
        assert_eq!(heap.object_identity_hash(weak), 1);
        assert_eq!(heap.object_shape(ephemeron), ObjectShape::Ephemeron);
        let fields = heap.ephemeron_fields(ephemeron).expect("an ephemeron");
        assert!(same_value_object(fields[0], Value::object(weak)));
        assert_eq!(fields[1].as_small_integer(), Some(6));
        assert!(fields[2].is_nil());
        assert_eq!(heap.object_identity_hash(ephemeron), 2);
    });
}

/// A weak-array record under any class but `WeakArray` would be an object
/// of that class holding weakly what its methods take for strong slots.
#[test]
fn a_weak_array_record_on_another_class_is_refused() {
    for header in [ARRAY_NAME, OBJECT_NAME, EPHEMERON_NAME] {
        refused_before_allocating(
            &one_weak_record(header, OBJECT_WEAK_ARRAY, 2, 2),
            &decode::DecodeError::WeakArrayRecordMismatch {
                body: OBJECT_WEAK_ARRAY,
            },
            "a weak-array record has a class other than WeakArray",
        );
    }
}

/// `WeakArray`'s header carries a weak-array record or the slotless
/// instance record `withoutSlots` makes, nothing else: any other body
/// would answer its leaves over an object of another shape. Each is
/// refused as soon as its body is read -- a byte-array body by the
/// byte-array check, which comes first -- so the bytes after it are never
/// looked at.
#[test]
fn another_body_under_the_weak_array_class_is_refused() {
    for body in [
        OBJECT_ARRAY,
        OBJECT_STRING,
        OBJECT_SYMBOL,
        OBJECT_CLOSURE,
        OBJECT_EPHEMERON,
        OBJECT_CLASS,
        OBJECT_METACLASS,
        OBJECT_FRAME,
    ] {
        refused_before_allocating(
            &one_weak_record(WEAK_ARRAY_NAME, body, 0, 0),
            &decode::DecodeError::WeakArrayRecordMismatch { body },
            &format!(
                "a record of the class WeakArray has object body {body}, but a WeakArray is \
                 a weak-array record or a slotless instance record"
            ),
        );
    }
    refused_before_allocating(
        &one_weak_record(WEAK_ARRAY_NAME, OBJECT_BYTE_ARRAY, 0, 0),
        &decode::DecodeError::ByteArrayRecordMismatch {
            body: OBJECT_BYTE_ARRAY,
        },
        "a byte-array record has a class other than ByteArray",
    );
}

/// The instance record `WeakArray`'s header may carry has the class's
/// slots, none: one of more fields is refused before anything is
/// allocated, and one of none decodes to the instance `withoutSlots`
/// makes.
#[test]
fn an_instance_record_under_the_weak_array_class_has_no_fields() {
    for count in [1, 2] {
        refused_before_allocating(
            &one_weak_record(WEAK_ARRAY_NAME, OBJECT_INSTANCE, count, count as usize),
            &decode::DecodeError::WeakArrayInstanceFieldCount {
                expected: 0,
                found: count as usize,
            },
            &format!(
                "an instance record of the class WeakArray has {count} fields, but WeakArray \
                 declares 0 slots"
            ),
        );
    }
    GraphFixture::new().decoded(
        &one_weak_record(WEAK_ARRAY_NAME, OBJECT_INSTANCE, 0, 0),
        "a slotless WeakArray",
        |heap, graph| {
            let object = graph.objects[0];
            assert_eq!(heap.object_shape(object), ObjectShape::Instance);
            assert_eq!(heap.instance_field_count(object), Some(0));
        },
    );
}

/// An ephemeron record under any class but `Ephemeron` would make the
/// collectors read that class's first slot as a key; any other record
/// under `Ephemeron`'s header would answer its methods over slots held
/// strongly, which no program makes. Under `WeakArray`'s header the
/// weak-array check refuses it first
/// (`another_body_under_the_weak_array_class_is_refused`).
#[test]
fn an_ephemeron_record_on_another_class_is_refused() {
    for header in [OBJECT_NAME, ARRAY_NAME] {
        refused_before_allocating(
            &one_weak_record(header, OBJECT_EPHEMERON, 3, 3),
            &decode::DecodeError::EphemeronRecordMismatch {
                body: OBJECT_EPHEMERON,
            },
            "an ephemeron record has a class other than Ephemeron",
        );
    }
    for body in [OBJECT_INSTANCE, OBJECT_ARRAY, OBJECT_WEAK_ARRAY] {
        let text = if body == OBJECT_WEAK_ARRAY {
            "a weak-array record has a class other than WeakArray".to_string()
        } else {
            format!(
                "a record of the class Ephemeron has object body {body}, but an Ephemeron is \
                 an ephemeron record"
            )
        };
        let expected = if body == OBJECT_WEAK_ARRAY {
            decode::DecodeError::WeakArrayRecordMismatch { body }
        } else {
            decode::DecodeError::EphemeronRecordMismatch { body }
        };
        refused_before_allocating(
            &one_weak_record(EPHEMERON_NAME, body, 3, 3),
            &expected,
            &text,
        );
    }
}

/// An `Ephemeron` has exactly its class's three fields, the key first; a
/// record of any other count is refused before its values are read.
#[test]
fn an_ephemeron_record_with_another_field_count_is_refused() {
    for count in [0, 1, 2, 4] {
        refused_before_allocating(
            &one_weak_record(EPHEMERON_NAME, OBJECT_EPHEMERON, count, count as usize),
            &decode::DecodeError::EphemeronFieldCount {
                expected: 3,
                found: count as usize,
            },
            &format!("an ephemeron record has {count} fields, but an Ephemeron has 3"),
        );
    }
}

/// A count the bytes that follow cannot hold is refused before anything
/// is allocated, as an `Array`'s is.
// spec: image.verify.counts
#[test]
fn a_weak_array_count_past_its_bytes_is_refused_before_allocating() {
    for count in [1_000_000, u32::MAX] {
        refused_before_allocating(
            &one_weak_record(WEAK_ARRAY_NAME, OBJECT_WEAK_ARRAY, count, 8),
            &decode::DecodeError::UnexpectedEndOfFile,
            &decode::DecodeError::UnexpectedEndOfFile.to_string(),
        );
    }
}

/// A module whose rows the reified-mixin records below name: `X` is row
/// 0, its declared class side row 1, and no row is past them.
const MIXIN_ROWS: &str = indoc! {"
    class X = Object new (
    ) (
    ) : (
      public make = (
        ^self new
      )
    )
"};

/// A reference an `OBJECT_MIXIN` record names its mixin by.
type MixinReference = Box<dyn FnOnce(&mut encode::Writer)>;

/// An `OBJECT_MIXIN` record under `header`: the reference `reference`
/// writes, then `side`.
fn mixin_record(writer: &mut encode::Writer, header: u32, reference: MixinReference, side: u8) {
    writer.write_shipped(header);
    writer.write_u32(0); // identity hash
    writer.write_u8(OBJECT_MIXIN);
    reference(writer);
    writer.write_u8(side);
}

/// A `MIXIN_ROW` reference to `row`.
fn row_reference(row: u32) -> MixinReference {
    Box::new(move |writer| {
        writer.write_u8(MIXIN_ROW);
        writer.write_u32(row);
    })
}

/// A `MIXIN_SHIPPED` reference to the fixture's name `name`.
fn shipped_reference(name: u32) -> MixinReference {
    Box::new(move |writer| {
        writer.write_u8(MIXIN_SHIPPED);
        writer.write_u32(name);
    })
}

/// A graph of one record under `Kernel Mixin` per `(reference, side)` of
/// `records`.
fn mixin_records(records: Vec<(MixinReference, u8)>) -> Vec<u8> {
    let count = u32::try_from(records.len()).expect("a few records");
    class_records(count, |writer| {
        for (reference, side) in records {
            mixin_record(writer, MIXIN_NAME, reference, side);
        }
    })
}

/// `refused_before_allocating` against `MIXIN_ROWS`'s rows, once `prepare`
/// has run in the fixture's world.
fn mixin_record_refused(
    bytes: &[u8],
    expected: &decode::DecodeError,
    text: &str,
    prepare: impl for<'gc> FnOnce(&mut Mutator<'gc>, &[Rc<Mixin>]),
) {
    let mut fixture = GraphFixture::with_source(MIXIN_ROWS);
    let rows = fixture.mixins.clone();
    fixture.interpreter.enter(|mutator| prepare(mutator, &rows));
    refused_in(&mut fixture, bytes, expected, text);
}

/// A crafted record decodes to the canonical object of the mixin it
/// names: a row's instance side, its declared class side, and a shipped
/// class's synthesized one.
#[test]
fn a_crafted_mixin_record_decodes_to_the_canonical_object() {
    let bytes = mixin_records(vec![
        (row_reference(0), MIXIN_SIDE_INSTANCE),
        (row_reference(0), MIXIN_SIDE_CLASS),
        (shipped_reference(ARRAY_NAME), MIXIN_SIDE_CLASS),
    ]);
    let mut fixture = GraphFixture::with_source(MIXIN_ROWS);
    let rows = fixture.mixins.clone();
    fixture.decoded(&bytes, "three mixin records", |heap, graph| {
        let mixin_of = |object| {
            let id = heap.mixin_object_id(object).expect("a reified mixin");
            heap.mixin(id).expect("a mixin of the table")
        };
        assert_eq!(heap.object_shape(graph.objects[0]), ObjectShape::Mixin);
        assert!(Rc::ptr_eq(&mixin_of(graph.objects[0]), &rows[0]));
        assert!(Rc::ptr_eq(&mixin_of(graph.objects[1]), &rows[1]));
        assert!(rows[1].is_class_side());
        let array_class_side = mixin_of(graph.objects[2]);
        assert!(array_class_side.is_class_side());
        assert_eq!(array_class_side.reflected_name(), "Array class");
    });
}

/// A reified mixin is canonical, as a symbol is: a second record for one
/// mixin, or one for a mixin the heap has reified already, would give it
/// two identities.
#[test]
fn two_records_for_one_mixin_are_refused() {
    for side in [MIXIN_SIDE_INSTANCE, MIXIN_SIDE_CLASS] {
        let name = if side == MIXIN_SIDE_CLASS {
            "X class"
        } else {
            "X"
        };
        mixin_record_refused(
            &mixin_records(vec![(row_reference(0), side), (row_reference(0), side)]),
            &decode::DecodeError::DuplicateMixin(name.to_string()),
            &format!("the mixin {name} is recorded twice"),
            |_, _| {},
        );
    }
    mixin_record_refused(
        &mixin_records(vec![
            (shipped_reference(OBJECT_NAME), MIXIN_SIDE_INSTANCE),
            (shipped_reference(OBJECT_NAME), MIXIN_SIDE_INSTANCE),
        ]),
        &decode::DecodeError::DuplicateMixin("Object".to_string()),
        "the mixin Object is recorded twice",
        |_, _| {},
    );
    mixin_record_refused(
        &mixin_records(vec![(row_reference(0), MIXIN_SIDE_INSTANCE)]),
        &decode::DecodeError::DuplicateMixin("X".to_string()),
        "the mixin X is recorded twice",
        |mutator, rows| {
            mutator.reified_mixin(&rows[0]);
        },
    );
}

// spec: image.verify.ids
#[test]
fn a_mixin_record_past_the_table_is_refused() {
    for past in [2, u32::MAX] {
        mixin_record_refused(
            &mixin_records(vec![(row_reference(past), MIXIN_SIDE_INSTANCE)]),
            &decode::DecodeError::BadMixinId(past),
            &format!("mixin id {past} out of range"),
            |_, _| {},
        );
    }
}

#[test]
fn a_mixin_record_with_side_two_is_refused() {
    for side in [2, u8::MAX] {
        mixin_record_refused(
            &mixin_records(vec![(row_reference(0), side)]),
            &decode::DecodeError::BadMixinSide(side),
            &format!(
                "a mixin record has side {side}, but a side is 0, the instance side, or 1, \
                 the class side"
            ),
            |_, _| {},
        );
    }
}

/// A class side is named through its instance side, with side 1, so that
/// each mixin has one spelling: a class side's own row is refused at
/// either side.
#[test]
fn a_mixin_record_naming_a_class_side_row_is_refused() {
    for side in [MIXIN_SIDE_INSTANCE, MIXIN_SIDE_CLASS] {
        mixin_record_refused(
            &mixin_records(vec![(row_reference(1), side)]),
            &decode::DecodeError::MixinRecordNamesAClassSide,
            "a mixin record names a class side's row; a class side is named by its instance \
             side's, with side 1",
            |_, _| {},
        );
    }
}

/// Only a class's shipped name names a mixin: a module instance or a
/// metaclass is no declaration of one.
// spec: image.shipped.positions
#[test]
fn a_mixin_record_naming_a_shipped_module_instance_is_refused() {
    for (name, shown) in [
        (KERNEL_INSTANCE_NAME, "Kernel instance"),
        (INTEGER_CLASS_NAME, "Kernel Integer class"),
    ] {
        mixin_record_refused(
            &mixin_records(vec![(shipped_reference(name), MIXIN_SIDE_INSTANCE)]),
            &decode::DecodeError::MixinRecordNamesNoClass(shown.to_string()),
            &format!("a mixin record names {shown}, which is no class's shipped name"),
            |_, _| {},
        );
    }
}

/// A mixin record under any class but `Mixin` would make an object of that
/// class stand for a mixin. Under `WeakArray`'s header the weak-array check
/// refuses it first.
#[test]
fn a_mixin_record_on_another_class_is_refused() {
    for header in [OBJECT_NAME, ARRAY_NAME, WEAK_ARRAY_NAME] {
        let bytes = class_records(1, |writer| {
            mixin_record(writer, header, row_reference(0), MIXIN_SIDE_INSTANCE);
        });
        let (expected, text) = if header == WEAK_ARRAY_NAME {
            (
                decode::DecodeError::WeakArrayRecordMismatch { body: OBJECT_MIXIN },
                format!(
                    "a record of the class WeakArray has object body {OBJECT_MIXIN}, but a \
                     WeakArray is a weak-array record or a slotless instance record"
                ),
            )
        } else {
            (
                decode::DecodeError::MixinRecordMismatch { body: OBJECT_MIXIN },
                "a mixin record has a class other than Mixin".to_string(),
            )
        };
        mixin_record_refused(&bytes, &expected, &text, |_, _| {});
    }
}

/// `Mixin`'s header carries a mixin record and nothing else: an instance
/// record of it, its one field an id, would stand for whatever mixin the
/// id named, outside the canonical table.
#[test]
fn an_instance_record_of_the_mixin_class_is_refused() {
    for body in [OBJECT_INSTANCE, OBJECT_ARRAY] {
        let bytes = class_records(1, |writer| {
            writer.write_shipped(MIXIN_NAME);
            writer.write_u32(0); // identity hash
            writer.write_u8(body);
            writer.write_u32(1);
            writer.write_u8(VALUE_SMALL_INTEGER);
            writer.write_i64(0);
        });
        mixin_record_refused(
            &bytes,
            &decode::DecodeError::MixinRecordMismatch { body },
            &format!(
                "a record of the class Mixin has object body {body}, but a Mixin is a mixin \
                 record"
            ),
            |_, _| {},
        );
    }
}

/// The root class of the images below: each probe sends a view or a
/// builder the messages a program would and answers, for each, the
/// `description` of the `ArgumentError` it signalled -- of any other
/// Error, after 'not an ArgumentError: ', and what it answered, after
/// 'answered ', so that a refusal of the wrong class shows as one.
const REFUSALS_PROBE: &str = indoc! {"
    class X usingPlatform: platform = Object new (
      |
        ArgumentError = platform kernel ArgumentError.
      |
    ) (
      refused: action = (
        ^[
          [
            'answered ' , action value printString.
          ] on: ArgumentError do: [:error | error description].
        ] on: Error do: [:error | 'not an ArgumentError: ' , error description].
      )

      public typedArray: view = (
        ^{refused: [view at: 1]. refused: [view at: 1 put: 2]. refused: [view size]}
      )

      public dataView: view = (
        ^{refused: [view int32At: 1]. refused: [view int32At: 1 put: 2]. refused: [view size]}
      )

      public builder: builder = (
        ^{
          refused: [builder asString].
          refused: [builder add: 'x'].
          refused: [builder size].
          refused: [builder isEmpty].
          refused: [builder asUtf8 size].
          refused: [builder addCodePoint: 233].
          refused: [(builder writeln: 'y') size].
        }
      )
    ) : ()
"};

/// A complete image of `REFUSALS_PROBE` whose graph is the `records` that
/// `write` writes, then the probe's class: its roots are record 0 and the
/// class.
fn probe_image(records: u32, write: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, records + 2);
    write(&mut writer);
    root_class_records(&mut writer, records);
    writer.write_u32(2); // two roots
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(0);
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(records);
    writer.write_u32(0); // no tag-named hashes
    image_with_graph_section_of(REFUSALS_PROBE, &writer.into_bytes(), records)
}

/// Loads `image`, a `probe_image`, and answers what its probe `selector`
/// answers for the image's first root, each element as text.
fn refusals(image: &[u8], selector: &str) -> Vec<String> {
    let mut loaded = image::load(image).expect("load");
    let subject = loaded.roots[0].clone();
    let class = loaded.roots[1].clone();
    let interpreter = &mut loaded.interpreter;
    let platform = interpreter.platform();
    let probe = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let answer = expect_ok(
        interpreter.send(&probe, Symbol::intern(selector), &[subject]),
        selector,
    );
    interpreter.enter(|mutator| {
        let answer = object(mutator.fetch(&answer));
        mutator
            .heap()
            .array_elements(answer)
            .expect("a probe answers an Array")
            .into_iter()
            .map(|element| text_or_class_name(mutator, element))
            .collect()
    })
}

/// An instance record of a shipped class, its identity hash `hash` and its
/// fields written by `fields`, which writes `count` of them.
fn shipped_instance(
    writer: &mut encode::Writer,
    header: u32,
    hash: u32,
    count: u32,
    fields: impl FnOnce(&mut encode::Writer),
) {
    writer.write_shipped(header);
    writer.write_u32(hash);
    writer.write_u8(OBJECT_INSTANCE);
    writer.write_u32(count);
    fields(writer);
}

/// A String record of `text`.
fn string_record(writer: &mut encode::Writer, hash: u32, text: &str) {
    writer.write_shipped(STRING_NAME);
    writer.write_u32(hash);
    writer.write_u8(OBJECT_STRING);
    writer.write_str(text);
}

/// A ByteArray record of `bytes`.
fn byte_array_record(writer: &mut encode::Writer, hash: u32, bytes: &[u8]) {
    writer.write_shipped(BYTE_ARRAY_NAME);
    writer.write_u32(hash);
    writer.write_u8(OBJECT_BYTE_ARRAY);
    writer.write_u32(u32::try_from(bytes.len()).expect("a length fits a u32"));
    for byte in bytes {
        writer.write_u8(*byte);
    }
}

/// A view's `bytes` is an ordinary slot, so an image can put anything in
/// it, and nothing about the record's structure is wrong: the image loads,
/// and every access of the view refuses with an `ArgumentError` that names
/// the selector sent, never reading through what is no ByteArray.
#[test]
fn a_view_over_what_is_no_byte_array_loads_and_refuses_every_access() {
    let typed_array = probe_image(1, |writer| {
        shipped_instance(writer, INT16_ARRAY_NAME, 3, 1, |writer| {
            writer.write_u8(VALUE_SMALL_INTEGER); // bytes
            writer.write_i64(7);
        });
    });
    assert_eq!(
        refusals(&typed_array, "typedArray:"),
        ["at:", "at:put:", "size"]
            .map(|selector| format!("{selector} found no ByteArray under the view, got 7"))
    );
    let data_view = probe_image(2, |writer| {
        shipped_instance(writer, BIG_ENDIAN_DATA_VIEW_NAME, 3, 1, |writer| {
            writer.write_u8(VALUE_OBJECT_ID); // bytes
            writer.write_u32(1);
        });
        string_record(writer, 4, "not bytes");
    });
    assert_eq!(
        refusals(&data_view, "dataView:"),
        ["int32At:", "int32At:put:", "size"].map(|selector| format!(
            "{selector} found no ByteArray under the view, got 'not bytes'"
        ))
    );
}

/// A field of a crafted StringBuilder record: a small integer, or the
/// record at an object id.
enum BuilderField {
    Integer(i64),
    Record(u32),
}

/// A probe image whose root is a StringBuilder of `fields` -- `data`,
/// `byteCount`, `codePointCount` -- followed by a ByteArray of two bytes
/// (record 1) and a String (record 2) for the fields to name.
fn crafted_builder(fields: [BuilderField; 3]) -> Vec<u8> {
    probe_image(3, |writer| {
        shipped_instance(writer, STRING_BUILDER_NAME, 3, 3, |writer| {
            for field in fields {
                match field {
                    BuilderField::Integer(integer) => {
                        writer.write_u8(VALUE_SMALL_INTEGER);
                        writer.write_i64(integer);
                    }
                    BuilderField::Record(id) => {
                        writer.write_u8(VALUE_OBJECT_ID);
                        writer.write_u32(id);
                    }
                }
            }
        });
        byte_array_record(writer, 4, b"ab");
        string_record(writer, 5, "two");
    })
}

/// A builder's slots are ordinary slots too, which an image can fill with
/// a state no builder's own members leave: a count past its data or below
/// 0, more code points than bytes, or something that is no Integer or no
/// ByteArray. Each loads, and every member refuses with an `ArgumentError`
/// naming the selector sent, before it reads past the bytes the builder
/// has or sends anything to the wrong object; `writeln:` refuses through
/// `add:`.
#[test]
fn a_builder_whose_state_is_not_its_own_loads_and_refuses() {
    use BuilderField::{Integer, Record};
    let data = || Record(1);
    let a_string = || Record(2);
    for (fields, described) in [
        (
            [data(), Integer(5), Integer(5)],
            "a byte count past its data",
        ),
        ([data(), Integer(-1), Integer(0)], "a negative byte count"),
        (
            [data(), Integer(2), Integer(3)],
            "more code points than bytes",
        ),
        (
            [data(), Integer(0), Integer(-1)],
            "a negative code point count",
        ),
        (
            [data(), a_string(), Integer(0)],
            "a byte count that is a String",
        ),
        (
            [data(), Integer(0), a_string()],
            "a code point count that is a String",
        ),
        (
            [a_string(), Integer(0), Integer(0)],
            "data that is a String",
        ),
        (
            [Integer(7), Integer(0), Integer(0)],
            "data that is an Integer",
        ),
    ] {
        assert_eq!(
            refusals(&crafted_builder(fields), "builder:"),
            [
                "asString",
                "add:",
                "size",
                "isEmpty",
                "asUtf8",
                "addCodePoint:",
                "add:",
            ]
            .map(|selector| format!("{selector} found a StringBuilder whose state is not its own")),
            "{described}"
        );
    }
    // The same records, the state a builder's own members leave: two bytes
    // of one code point each.
    assert_eq!(
        refusals(
            &crafted_builder([data(), Integer(2), Integer(2)]),
            "builder:"
        ),
        [
            "answered 'ab'",
            "answered 'x'",
            "answered 3",
            "answered false",
            "answered 3",
            "answered 233",
            "answered 6",
        ],
    );
}

#[test]
fn an_invalid_value_tag_in_the_graph_is_a_decode_error_not_a_panic() {
    // 200 is no value tag at all; 7 and 8 are retired, the platform and an
    // object a platform path named, and must name nothing now.
    for tag in [200].into_iter().chain(RETIRED_VALUE_TAGS) {
        let bytes = one_array_object(|writer| {
            writer.write_u8(tag);
            writer.write_u32(0); // what a platform path reference carried
        });
        assert_eq!(
            GraphFixture::new().decode_error(&bytes),
            decode::DecodeError::InvalidTag { what: "value", tag }
        );
    }
}

#[test]
fn an_invalid_object_body_tag_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    writer.write_shipped(OBJECT_NAME);
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
    // 200 is no class tag at all; 1 and 3 are retired, the one root class
    // and its levels before an image carried several modules, and must name
    // no module now rather than whichever came first; 4, 6 and 7 are
    // retired, a module's root class, its levels and their metaclasses
    // before those became records; 0 and 5 are retired, a builtin by
    // builtin tag and a class a platform path named.
    for tag in [200].into_iter().chain(RETIRED_CLASS_TAGS) {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 1);
        writer.write_u8(tag);
        // Padded out to the shortest object record the wire admits, so
        // `bounded_count` admits the announced record and the class tag
        // itself is what the decoder rejects.
        writer.write_u32(1); // identity hash
        writer.write_u8(OBJECT_STRING); // object body: String
        let bytes = writer.into_bytes();
        assert_eq!(
            GraphFixture::new().decode_error(&bytes),
            decode::DecodeError::InvalidTag { what: "class", tag }
        );
    }
}

#[test]
fn a_symbol_recorded_twice_is_refused() {
    // A canonical heap never encodes two records of one symbol, so a graph
    // that holds two is crafted or corrupt; merging them would mean
    // rewriting every reference to the second.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2);
    for identity_hash in [1, 2] {
        writer.write_shipped(SYMBOL_NAME);
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
fn a_string_recorded_with_the_class_symbol_is_refused() {
    // A Symbol is a String whose class is `Symbol`, one object per text;
    // a string record of that class would be a second object of a text the
    // canonical table never hears of.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    writer.write_shipped(SYMBOL_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_STRING);
    writer.write_str("new");
    writer.write_u32(0); // no roots
    let error = GraphFixture::new().decode_error(&writer.into_bytes());
    assert_eq!(error, decode::DecodeError::StringOfSymbolClass);
    assert_eq!(
        error.to_string(),
        "a string record has the class Symbol, which only a symbol record may have"
    );
}

#[test]
fn a_symbol_recorded_with_a_class_other_than_symbol_is_refused() {
    // `decode_graph_section` enters every `OBJECT_SYMBOL` record into the
    // heap's canonical symbol table as it decodes it, before anything else
    // runs -- so a record whose class tag names something other than
    // `Symbol` would still answer every later literal of that text if it
    // were let through.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    writer.write_shipped(STRING_NAME);
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
    writer.write_shipped(METACLASS_NAME);
    writer.write_u32(8); // identity hash
    writer.write_u8(OBJECT_METACLASS); // object body: Metaclass
    writer.write_u8(CLASS_RECORD); // its class: an object-table record
    writer.write_u32(class_id);
}

/// A graph section holding one class record (object 0), its metaclass
/// record (object 1) and no roots.
fn one_class_object(mixin_id: u32, references: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2);
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
    graph_counts(&mut writer, 4);
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

// spec: image.verify.ids
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
    graph_counts(&mut writer, 3);
    class_record(&mut writer, 2, 0, |writer| {
        writer.write_boolean(true);
        writer.write_u8(CLASS_RECORD);
        writer.write_u32(1); // object 1, which is an array
        writer.write_boolean(false);
        writer.write_boolean(false);
    });
    writer.write_shipped(ARRAY_NAME);
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
    graph_counts(&mut writer, 4);
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
    graph_counts(&mut writer, 1);
    frame_header(&mut writer, 0);
    writer
}

#[test]
fn an_invalid_frame_kind_tag_is_a_decode_error_not_a_panic() {
    let mut writer = one_frame_prefix();
    writer.write_u8(200);
    // The rest of a well-formed record, so the kind tag is what the
    // decoder rejects.
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: 200
        }
    );
}

/// The byte after the body tag is the frame's kind tag. A record with one
/// stray byte 1 there has it read as `FRAME_CLOSURE`, and the next four
/// bytes (the real kind tag, the body's absence and two bytes of the
/// program counter) as a closure object id outside the one-record table,
/// so it is refused rather than read as a different frame.
#[test]
fn a_frame_record_with_a_stray_byte_is_refused() {
    let mut writer = one_frame_prefix();
    writer.write_u8(1); // the stray byte
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(u32::from(FRAME_TOP_LEVEL))
    );
}

#[test]
fn a_closure_objects_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    writer.write_shipped(CLOSURE_NAME);
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
    writer.write_u32(42); // ...naming an object past the one-record table
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::BadObjectId(42)
    );
}

#[test]
fn a_truncated_graph_section_is_a_decode_error_not_a_panic() {
    let source = indoc! {"
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    // Read out after the last run, for the encoder and the decoder, which run
    // nothing.
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let byte_array = mutator.new_byte_array(&byte_array_contents(13));
        let roots = vec![mutator.fetch(&instance), Value::object(byte_array)];
        let mut symbols = symbols::Builder::new();
        let mut writer = encode::Writer::new();
        let ids = encode::encode_code_table(
            &mut writer,
            &rows_of(mutator.heap(), class),
            &[],
            &mut symbols,
        )
        .expect("encode code table");
        let before = writer.into_bytes().len();
        let mut writer = encode::Writer::new();
        let mixin_ids = mixin_ids_for(mutator, class, &mut symbols, &ids);
        let walked = encode::walk_graph(mutator, &roots, &[class]).expect("walk graph");
        let encoded = encode::encode_graph_section(
            mutator,
            &mut writer,
            &walked,
            &roots,
            &ids,
            &mixin_ids,
            &mut symbols,
        )
        .expect("encode graph");
        let shipped = shipped_names::resolve_in(mutator, &encoded.used.names(mutator), false)
            .expect("the graph's own names resolve");
        let graph = writer.into_bytes();
        let symbol_table = symbols.build();
        assert!(before > 0 && graph.len() > 8);
        let mixin_order = mixin_preorder(mutator.heap(), class);
        let mut context = decode::DecodeContext {
            mutator,
            module_roots: &encoded.module_roots,
            codes: &[],
            mixins: &mixin_order,
            shipped: &shipped,
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

// spec: image.graph.record
#[test]
fn a_closure_capturing_a_block_activation_round_trips() {
    // `[:x | [ x ] ] value: 5` runs the outer block, so the inner closure's
    // `captured_frame` is a *block activation* -- a `FrameKind::Closure`
    // frame, the one frame kind the quiesced worlds above never produce.
    let source = indoc! {"
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
        let inner_captured_frame = heap
            .closure_captured_frame(object(decoded_0))
            .expect("expected the inner closure");
        let inner_home = heap
            .closure_home(object(decoded_0))
            .expect("expected the inner closure");
        assert!(
            !inner_captured_frame.same(inner_home),
            "the inner closure was captured by the block activation, not by `mk`"
        );
        let captured = heap.frame_record(inner_captured_frame);
        let FrameKind::Closure(outer) = &captured.kind else {
            panic!("the captured frame is a block activation")
        };
        // Every reified frame travels with its whole register file, so the
        // decoded one has every register its body declares but the header's
        // numbers.
        let outer_code = heap.closure_code(*outer).expect("the outer block");
        assert_eq!(
            captured.arguments.len() + captured.temporaries.len(),
            outer_code.frame_register_count(),
            "the captured frame carries its full register file"
        );
        assert_eq!(
            captured.temporaries.len(),
            outer_code.registers_above_header()
        );
        // The optimized lowering converts `[ x ]`: `x` is copied into the
        // inner closure when it is created, so the outer block's activation
        // returned without copying its registers into its frame
        // (`docs/design/execution.md`, "Lifecycle"): its frame has the
        // block's one argument register, still nil. The value survives in
        // the closure, not in the frame.
        assert!(
            matches!(captured.arguments[..], [argument] if argument.is_nil()),
            "the frame's argument register was never written"
        );
        assert_eq!(
            heap.closure_copied_values(object(decoded_0))
                .expect("expected a closure")
                .first()
                .and_then(|value| value.as_small_integer()),
            Some(5)
        );
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
        // Against the original declarations and a freshly decoded code
        // table (`round_trip_graph`): the frame's method is built over the
        // decoded body its record names.
        let application = heap.class_application(class).expect("not a class");
        let method = application
            .mixin
            .compiled_method(Symbol::intern("mk"))
            .expect("the class declares `mk`");
        assert_eq!(declaration.selector, method.selector);
        assert_eq!(*declaration.code, *method.code);
    });
}

/// A closure frame names the closure object it runs, rather than carrying a
/// copy of its record: after a round trip the inner closure's captured frame
/// -- the outer block's activation -- names a decoded closure object, and it
/// is the outer block, with its one parameter.
#[test]
fn a_closure_frame_names_the_closure_object_it_runs() {
    let source = indoc! {"
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
        let inner_captured_frame = heap
            .closure_captured_frame(object(decoded_0))
            .expect("expected the inner closure");
        let inner_home = heap
            .closure_home(object(decoded_0))
            .expect("expected the inner closure");
        let FrameKind::Closure(outer) = heap.frame_kind(inner_captured_frame) else {
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
    writer.write_u8(FRAME_FACTORY + 1); // one past the last frame kind
    // The rest of a well-formed record, so the kind tag itself is what the
    // reader rejects.
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
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
            tag: FRAME_FACTORY + 1
        }
    );
}

/// A frame kind retired with the clause-message bodies (5 and 6) meets the
/// same refusal as one never assigned, so a stale image cannot be read as
/// whichever kind took its number.
#[test]
fn a_retired_frame_kind_is_refused() {
    assert_eq!(RETIRED_FRAME_KINDS, [5, 6]);
    for tag in RETIRED_FRAME_KINDS {
        let mut writer = one_frame_prefix();
        writer.write_u8(tag);
        frame_tail(&mut writer, 0, 0);
        writer.write_u32(0); // no roots
        assert_eq!(
            GraphFixture::new().decode_error(&writer.into_bytes()),
            decode::DecodeError::InvalidTag {
                what: "frame kind",
                tag
            }
        );
    }
}

/// `inspect` walks the graph with its own reader, so it has to refuse an
/// unknown frame kind itself: an artifact `load` refuses must not inspect as
/// if it were well-formed. The describing sections come from a real snapshot and
/// the graph section is the hand-written one above, spliced in where the
/// snapshot's own graph began.
#[test]
fn inspect_refuses_an_unknown_frame_kind_tag() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[]).expect("snapshot")
    });
    bytes = with_fixture_names(&bytes);
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
    let expected = format!("invalid frame kind tag {}", FRAME_FACTORY + 1);
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
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let inner = expect_ok(interpreter.send(&instance, Symbol::intern("mk"), &[]), "mk");
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let inner = mutator.fetch(&inner);
        snapshot_one(mutator, false, "t.ns", class, &[inner]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let closure_frames: Vec<&String> = report
        .graph_objects
        .iter()
        .filter(|record| record.contains(", frame closure object "))
        .collect();
    let [frame] = closure_frames[..] else {
        panic!(
            "expected one closure frame, the outer block's activation, in {:?}",
            report.graph_objects
        )
    };
    let id: usize = frame
        .split_once(", frame closure object ")
        .and_then(|(_, rest)| rest.split(',').next())
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
    graph_counts(&mut writer, 2);
    writer.write_shipped(STRING_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_STRING); // object body: String
    writer.write_str("not a closure");
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_CLOSURE); // frame kind: a closure...
    writer.write_u32(0); // ...naming object 0, the string
    runs_body(&mut writer, 0); // the fixture's block
    writer.write_u32(0); // program_counter
    writer.write_boolean(true); // uncontinuable
    writer.write_u8(VALUE_NIL); // receiver: nil
    writer.write_boolean(false); // no sender
    writer.write_boolean(true); // a home...
    writer.write_u32(1); // ...object 1, the frame itself
    writer.write_boolean(false); // no current class
    writer.write_u32(0); // no registers
    writer.write_u32(0); // no roots
    let mut fixture = GraphFixture::new();
    fixture.codes = vec![block_code(0, 0)];
    let error = fixture.decode_error(&writer.into_bytes());
    assert_eq!(error, decode::DecodeError::NotAClosureReference);
    assert_eq!(
        error.to_string(),
        "a closure frame in the image names an object that is not a closure"
    );
}

// spec: image.graph.record
#[test]
fn a_top_level_frame_kind_decodes() {
    // Nothing that runs a program *builds* a `FrameKind::TopLevel` frame (see
    // `types::FrameKind`), so only a hand-written section can reach the
    // decoder's tag-2 branch.
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    frame_tail(&mut writer, 0, 0);
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
    Rc::new(CompiledCode::new(
        CodeKind::Block,
        vec![Instruction::ImplicitReturn { register: 0 }],
        argument_count,
        argument_count + copied_count + 1,
        copied_count,
        vec![],
        Span::new(0, 0),
    ))
}

/// A graph section holding one closure object and the one `TopLevel` frame
/// it points at, carrying `copied` copied values against code id 0. Every
/// field but their count is well-formed, so *only* the copied-count
/// reconciliation can reject it.
fn closure_object_with_copied_values(copied: &[i64]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2);
    writer.write_shipped(CLOSURE_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(0); // code id
    writer.write_u32(1); // captured frame -- object 1, the frame
    writer.write_u16(copied.len() as u16);
    for value in copied {
        writer.write_u8(VALUE_SMALL_INTEGER);
        writer.write_i64(*value);
    }
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    frame_tail(&mut writer, 0, 0);
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
            let copied = heap
                .closure_copied_values(object(decoded.roots[0]))
                .expect("expected a closure");
            assert_eq!(
                copied
                    .iter()
                    .map(|value| value.as_small_integer())
                    .collect::<Vec<_>>(),
                [Some(7), Some(8)]
            );
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
            let copied = heap
                .closure_copied_values(object(decoded.roots[0]))
                .expect("expected a closure");
            assert!(copied.is_empty());
            // The arity a call is checked against now has one source.
            assert_eq!(
                heap.closure_parameter_count(object(decoded.roots[0])),
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
    let (bytes, max_hash) = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let max_hash = mutator
            .heap_mut()
            .object_ensure_identity_hash(object(instance));
        let class = class_reference(mutator, &class);
        let bytes = snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot");
        (bytes, max_hash)
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.modules[0].class);
        assert!(
            !root_mixin_of(mutator.heap(), root_class).methods.is_empty(),
            "the image's methods are rebuilt, each carrying its body"
        );
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert!(mutator.heap().object_class(object(root_0)).same(root_class));
    });
    let bumped = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("bump"), &[]),
        "bump",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            mutator.fetch(&bumped).as_small_integer() == Some(2),
            "slot state survived: 1 before the snapshot, 2 after"
        );
        let fresh = mutator.new_string("fresh".to_string());
        assert!(
            mutator.heap_mut().object_ensure_identity_hash(fresh) > max_hash,
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
        class Counter = Object new (
          | count ::= 0. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.modules[0].class);
        let ids = mutator.heap().class_dispatch_ids(root_class);
        assert_ne!(ids.instances, DispatchId::NONE);
        let instance = mutator.fetch(&loaded.roots[0]);
        assert_eq!(mutator.heap().dispatch_id(instance), ids.instances);
        // A class object is an instance of its metaclass.
        let metaclass = mutator
            .metaclass_of(root_class)
            .expect("the root class has a metaclass");
        let metaclass_ids = mutator.heap().class_dispatch_ids(metaclass);
        assert_ne!(metaclass_ids.instances, DispatchId::NONE);
        assert_eq!(
            mutator.heap().dispatch_id(Value::object(root_class)),
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

    let bytes = interpreter.enter(|mutator| {
        let alpha = mutator.fetch(&alpha);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[alpha]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let instance = mutator.fetch(&loaded.roots[0]);
        let decoded_alpha_class = mutator.heap().object_class(object(instance));
        let ids = mutator.heap().class_dispatch_ids(decoded_alpha_class);
        assert_ne!(ids.instances, DispatchId::NONE);
        assert_eq!(mutator.heap().dispatch_id(instance), ids.instances);
    });
}

/// The text a `Holder`'s `trace` slot holds, read through its getter.
fn trace_of(interpreter: &mut Interpreter, holder: &Root) -> String {
    let trace = expect_ok(
        interpreter.send(holder, Symbol::intern("trace"), &[]),
        "trace",
    );
    interpreter.enter(|mutator| match mutator.fetch(&trace).kind() {
        ValueKind::Object(string) => mutator
            .heap()
            .str_value(string)
            .expect("trace holds a String"),
        _ => panic!(
            "trace answered {}",
            mutator.heap().describe(mutator.fetch(&trace))
        ),
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
    let bytes = interpreter.enter(|mutator| {
        let root = mutator.fetch(&root);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[root]).expect("snapshot")
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
                    mutator.fetch(&computed).as_small_integer() == Some(42),
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
            .enter(|mutator| match mutator.fetch(&left).kind() {
                ValueKind::Object(string) => assert_eq!(
                    mutator.heap().str_value(string).as_deref(),
                    Some("left"),
                    "{name}'s inherited slot keeps its value"
                ),
                _ => panic!(
                    "left answered {}",
                    mutator.heap().describe(mutator.fetch(&left))
                ),
            });
    }
}

/// A lazy slot's getter is one of its row's compiled methods; the slot
/// record only says the slot is lazy. A row that declares a lazy slot and
/// no unary method of its name would leave the slot with no getter at all
/// -- no send could ever read it -- so the decoder refuses it. Patched into
/// a real binary: the root row's one eager slot is turned lazy, and nothing
/// else changes.
// spec: image.verify.mixin-rows
#[test]
fn a_lazy_slot_without_its_getter_is_refused() {
    let source = indoc! {"
        class Holder = Object new (| trace = 0. |) (
          main: p args: q = ( ^trace )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let mut binary = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        module_only_image(mutator, false, "t.ns", class).expect("snapshot")
    });
    assert!(image::load(&binary).is_ok(), "the fixture itself must load");

    let (mixin_table_at, symbols, codes) = {
        let mut reader = decode::Reader::new(&binary);
        decode::read_header(&mut reader).expect("valid header");
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

// spec: image.trailing-bytes
#[test]
fn bytes_after_the_last_section_are_refused_not_ignored() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[]).expect("snapshot")
    });
    assert!(image::load(&bytes).is_ok(), "the fixture itself must load");

    bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let error = load_error(&bytes);
    assert!(
        error.contains("trailing"),
        "expected a trailing-bytes error, got: {error}"
    );
}

// spec: image.header
#[test]
fn load_refuses_a_version_mismatch() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[]).unwrap()
    });
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    let error = load_error(&bytes);
    assert!(error.contains("version"), "{error}");
}

/// An image another binary wrote is refused before anything past the
/// header is read: its shipped names would resolve against another world.
// spec: image.header.fingerprint
#[test]
fn an_image_of_another_shipped_world_is_refused() {
    let mut bytes = snapshot_of(EMPTY);
    let fingerprint_at = encode::MAGIC.len() + 4 + 1;
    bytes[fingerprint_at] ^= 1;
    let error = load_error(&bytes);
    assert!(
        error.contains("another shipped world"),
        "the refusal names the mismatch: {error}"
    );
}

/// The fingerprint is a function of the binary alone.
// spec: image.header
#[test]
fn the_shipped_world_fingerprint_is_stable_within_a_binary() {
    assert_eq!(
        image::fingerprint::shipped_world_fingerprint(),
        image::fingerprint::shipped_world_fingerprint()
    );
    let bytes = snapshot_of(EMPTY);
    let fingerprint_at = encode::MAGIC.len() + 4 + 1;
    let written = u64::from_le_bytes(
        bytes[fingerprint_at..fingerprint_at + 8]
            .try_into()
            .expect("eight bytes"),
    );
    assert_eq!(written, image::fingerprint::shipped_world_fingerprint());
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

const UTIL_ONE: &str = indoc! {"
    class Util = Object new () (
        public answer = ( ^1 )
    ) : ()
"};

const UTIL_TWO: &str = indoc! {"
    class Util = Object new () (
        public answer = ( ^2 )
    ) : ()
"};

const MANIFEST_MODULE: &str = indoc! {"
    class Manifest = Object new () () : ()
"};

/// An identity as a host gives it, its id and version parsed against
/// `class_name` as the decoder parses them.
fn identity(path: &str, id: &str, class_name: &str, version: Option<&str>) -> ModuleIdentity {
    ModuleIdentity {
        path: path.to_string(),
        id: ModuleId::parse(id, class_name).unwrap_or_else(|error| panic!("{error}")),
        version: version.map(|text| Version::parse(text).unwrap_or_else(|error| panic!("{error}"))),
    }
}

/// Two versions of `Util` and a module of the binary's own, `Manifest`,
/// written with `with_source` under the identities `identities`, in that
/// order, the first module's instance the one root.
fn util_image(identities: &[ModuleIdentity; 3], with_source: bool) -> Vec<u8> {
    let (mut interpreter, one) = load(UTIL_ONE);
    let two = load_another(&mut interpreter, UTIL_TWO, "Util.ns");
    let manifest = load_another(&mut interpreter, MANIFEST_MODULE, "Manifest.ns");
    let instance = expect_ok(interpreter.instantiate(&one, &[]), "Util new");
    interpreter
        .enter(|mutator| {
            let modules: Vec<image::ModuleSource> = [&one, &two, &manifest]
                .into_iter()
                .zip(identities)
                .map(|(class, identity)| image::ModuleSource {
                    class: class_reference(mutator, class),
                    identity,
                    with_source,
                })
                .collect();
            image::snapshot(mutator, &modules, &[mutator.fetch(&instance)])
        })
        .unwrap_or_else(|error| panic!("{error}"))
}

/// The identities `util_image` is written with when nothing is crafted.
fn util_identities() -> [ModuleIdentity; 3] {
    [
        identity(
            "libs/util-1/Util.ns",
            "com.acme.Util",
            "Util",
            Some("1.0.0"),
        ),
        identity(
            "../shared/util-2/Util.ns",
            "com.acme.Util",
            "Util",
            Some("2.0.0-beta.1"),
        ),
        identity(
            "core:Manifest.ns",
            "newspeak.core.Manifest",
            "Manifest",
            None,
        ),
    ]
}

/// An image names its modules as the program did: each its path relative
/// to the project, its id and its version, a module of the binary's by its
/// `core:` path; and every loaded root mixin carries its module's.
// spec: image.modules
#[test]
fn an_image_names_its_modules_as_the_program_did() {
    let identities = util_identities();
    for with_source in [false, true] {
        let bytes = util_image(&identities, with_source);
        let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(loaded.modules.len(), 3);
        for (module, expected) in loaded.modules.iter().zip(&identities) {
            assert_eq!(&*module.identity, expected);
            assert_eq!(
                module.declaration.module_identity.as_deref(),
                Some(expected),
                "the root mixin carries its module's identity"
            );
        }
        let report = image::inspect::inspect(&bytes).expect("inspect");
        assert_eq!(
            report.modules[1].identity,
            "com.acme.Util@2.0.0-beta.1 at ../shared/util-2/Util.ns"
        );
        assert_eq!(
            report.modules[2].identity,
            "newspeak.core.Manifest at core:Manifest.ns"
        );
    }
}

/// A module record holds the path relative to the project: the directory
/// the building machine read the module from is in no byte of the image,
/// with its source or without.
// spec: image.modules
#[test]
fn the_building_machine_s_paths_are_not_written() {
    let directory = std::env::temp_dir().join("ns-next-the-building-machine-s-paths");
    let file = directory.join("libs").join("Util.ns");
    let module = ns_syntax::parse(UTIL_ONE).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(UTIL_ONE), SourceOrigin::File(file.clone()))
            .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let module_identity = identity("libs/Util.ns", "Util", "Util", None);
    for with_source in [false, true] {
        let bytes = interpreter.enter(|mutator| {
            let modules = [image::ModuleSource {
                class: class_reference(mutator, &class),
                identity: &module_identity,
                with_source,
            }];
            image::snapshot(mutator, &modules, &[]).unwrap_or_else(|error| panic!("{error}"))
        });
        for written in [directory.to_string_lossy(), file.to_string_lossy()] {
            assert!(
                !bytes
                    .windows(written.len())
                    .any(|window| window == written.as_bytes()),
                "the image holds {written}"
            );
        }
        let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(*loaded.modules[0].identity, module_identity);
    }
}

/// The encoder holds every identity it is handed to the rules the loader
/// holds a module record to, so that it never writes an image its loader
/// refuses: a path that is not the project's, an id that does not end in
/// the root's name, and two modules of one id without a version are each
/// refused before anything is written.
#[test]
fn the_encoder_refuses_an_identity_the_loader_refuses() {
    let [one, two, manifest] = util_identities();
    let refused = [
        (
            identity("/home/builder/Util.ns", "Util", "Util", None),
            "image encoding cannot name module 0 so: `/home/builder/Util.ns` has a root or a \
             drive; it must be relative",
        ),
        (
            identity("Util.ns", "Other", "Other", None),
            "image encoding cannot name module 0 so: `Other` does not end in the class's name \
             `Util`",
        ),
        (
            identity("libs/util-3/Util.ns", "com.acme.Util", "Util", None),
            "image encoding cannot name its modules so: two modules are `com.acme.Util` and one \
             of them has no version: libs/util-3/Util.ns and ../shared/util-2/Util.ns",
        ),
    ];
    for (first, expected) in refused {
        let (mut interpreter, one_class) = load(UTIL_ONE);
        let two_class = load_another(&mut interpreter, UTIL_TWO, "Util.ns");
        let manifest_class = load_another(&mut interpreter, MANIFEST_MODULE, "Manifest.ns");
        let identities = [&first, &two, &manifest];
        let written = interpreter.enter(|mutator| {
            let modules: Vec<image::ModuleSource> = [&one_class, &two_class, &manifest_class]
                .into_iter()
                .zip(identities)
                .map(|(class, identity)| image::ModuleSource {
                    class: class_reference(mutator, class),
                    identity,
                    with_source: false,
                })
                .collect();
            image::snapshot(mutator, &modules, &[])
        });
        assert_eq!(written.err().as_deref(), Some(expected));
    }
    assert!(image::load(&util_image(&[one, two, manifest], false)).is_ok());
}

/// One module record as `image::modules` writes it, its parts given
/// verbatim, so that a test can write one the encoder never would.
#[derive(Clone, Copy)]
struct Record<'a> {
    origin: u8,
    path: &'a str,
    id: &'a str,
    version: Option<&'a str>,
}

/// `bytes`, an image of `records.len()` modules, with its module section
/// replaced by `records`, each naming the root object its module named.
fn with_module_records(bytes: &[u8], records: &[Record<'_>]) -> Vec<u8> {
    let report = image::inspect::inspect(bytes).expect("inspect");
    let section = report
        .sections
        .iter()
        .find(|section| section.name == "modules")
        .expect("a module section");
    assert_eq!(report.modules.len(), records.len());
    let mut writer = encode::Writer::new();
    writer.write_u32(u32::try_from(records.len()).expect("a count"));
    for (index, record) in records.iter().enumerate() {
        writer.write_u8(record.origin);
        writer.write_str(record.path);
        writer.write_str(record.id);
        match record.version {
            Some(version) => {
                writer.write_u8(MODULE_VERSIONED);
                writer.write_str(version);
            }
            None => writer.write_u8(MODULE_UNVERSIONED),
        }
        writer.write_u32(u32::try_from(module_root_object(bytes, index)).expect("an id"));
    }
    let mut crafted = bytes[..section.start].to_vec();
    crafted.extend_from_slice(&writer.into_bytes());
    crafted.extend_from_slice(&bytes[section.end..]);
    crafted
}

/// A project-relative record of `path`, `id` and `version`.
fn project<'a>(path: &'a str, id: &'a str, version: Option<&'a str>) -> Record<'a> {
    Record {
        origin: ORIGIN_PROJECT,
        path,
        id,
        version,
    }
}

/// The record of `util_image`'s third module, the binary's own `Manifest`.
const MANIFEST_RECORD: Record<'static> = Record {
    origin: ORIGIN_CORE,
    path: "Manifest.ns",
    id: "newspeak.core.Manifest",
    version: None,
};

/// A refusal of the module record `module` for `reason`.
fn module_error(module: u32, reason: &str) -> String {
    decode::DecodeError::ModuleIdentity {
        module,
        reason: reason.to_string(),
    }
    .to_string()
}

/// The decoder holds every module record to the rules a project is held to
/// (`docs/design/modules.md`, decisions 1, 2, 5 and 9), before any record
/// is used: a path relative to the project and ending in `.ns`, or a
/// `core:` file name; an id that ends in its root class's name; the
/// namespace `newspeak.` for the binary's modules alone, and nothing else
/// for them; a version semver's; and no two modules no request could pick
/// between.
// spec: image.modules
// spec: image.verify.module-identity
// spec: image.verify.modules
#[test]
fn a_module_record_is_held_to_the_project_rules() {
    let bytes = util_image(&util_identities(), false);
    let valid = with_module_records(
        &bytes,
        &[
            project("a/Util.ns", "Util", Some("1.0.0")),
            project("b/Util.ns", "Util", Some("1.0.1")),
            MANIFEST_RECORD,
        ],
    );
    let loaded = image::load(&valid).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.modules[1].identity.display_name(), "Util@1.0.1");
    assert_eq!(loaded.modules[2].identity.path, "core:Manifest.ns");

    let refused = [
        (
            "a path that climbs out and is no module's file",
            [
                project("../../etc/passwd", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`../../etc/passwd` does not end in `.ns`"),
        ),
        (
            "a path holding a terminal's escape sequence",
            [
                project("a/\u{1b}[2J.ns", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`a/\\u{1b}[2J.ns` holds a control character"),
        ),
        (
            "a path holding a bidirectional override, which `inspect` would print",
            [
                project("a/\u{202E}sn.txt.ns", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`a/\\u{202e}sn.txt.ns` holds a format character"),
        ),
        (
            "an id holding a control character",
            [
                project("a/Util.ns", "Util\u{7}", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`Util\\u{7}` holds a control character"),
        ),
        (
            "a file named only `.ns`",
            [
                project("../.ns", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`../.ns` names no file: its name is only `.ns`"),
        ),
        (
            "an absolute path",
            [
                project("a/Util.ns", "Util", Some("1.0.0")),
                project("/abs.ns", "Util", Some("2.0.0")),
            ],
            module_error(1, "`/abs.ns` has a root or a drive; it must be relative"),
        ),
        (
            "a `core:` path under the project's origin",
            [
                project("core:Util.ns", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(
                0,
                "`core:Util.ns` has a root or a drive; it must be relative",
            ),
        ),
        (
            "a reserved id of the project's",
            [
                project("a/Util.ns", "newspeak.core.Util", None),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(
                0,
                "`newspeak.core.Util` lies in the namespace `newspeak.`, which belongs to the \
                 modules ns-next ships",
            ),
        ),
        (
            "a version that is no semver",
            [
                project("a/Util.ns", "Util", Some("1.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(0, "`1.0` is no version: it needs MAJOR.MINOR.PATCH"),
        ),
        (
            "two modules of one id and version",
            [
                project("a/Util.ns", "com.acme.Util", Some("1.0.0")),
                project("b/Util.ns", "com.acme.Util", Some("1.0.0")),
            ],
            decode::DecodeError::IndistinctModules(
                "two modules are `com.acme.Util@1.0.0`: a/Util.ns and b/Util.ns".to_string(),
            )
            .to_string(),
        ),
        (
            "two modules of one id, one without a version",
            [
                project("a/Util.ns", "Util", None),
                project("b/Util.ns", "Util", Some("1.0.0")),
            ],
            decode::DecodeError::IndistinctModules(
                "two modules are `Util` and one of them has no version: a/Util.ns and b/Util.ns"
                    .to_string(),
            )
            .to_string(),
        ),
        (
            "an id that does not end in its root's name",
            [
                project("a/Util.ns", "com.acme.Util", Some("1.0.0")),
                project("b/Util.ns", "com.acme.Other", Some("2.0.0")),
            ],
            module_error(
                1,
                "`com.acme.Other` does not end in the class's name `Util`",
            ),
        ),
        (
            "an id that is no id",
            [
                project("a/Util.ns", "com..Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
            ],
            module_error(
                0,
                "`com..Util` is no module id: it must be identifiers separated by dots",
            ),
        ),
    ];
    for (what, [first, second], expected) in refused {
        let crafted = with_module_records(&bytes, &[first, second, MANIFEST_RECORD]);
        assert_eq!(load_error(&crafted), expected, "{what}");
    }

    // The one module of the binary's an image carries is the manifest, at
    // its `core:` path, of its reserved id, without a version.
    let core_refused = [
        (
            Record {
                id: "Manifest",
                ..MANIFEST_RECORD
            },
            module_error(
                2,
                "`Manifest` at `core:Manifest.ns` is no module an image carries from the \
                 binary: only `newspeak.core.Manifest` at `core:Manifest.ns`, without a version",
            ),
        ),
        (
            Record {
                version: Some("1.0.0"),
                ..MANIFEST_RECORD
            },
            module_error(
                2,
                "`newspeak.core.Manifest@1.0.0` at `core:Manifest.ns` is no module an image \
                 carries from the binary: only `newspeak.core.Manifest` at `core:Manifest.ns`, \
                 without a version",
            ),
        ),
        (
            Record {
                path: "lib/Manifest.ns",
                ..MANIFEST_RECORD
            },
            module_error(
                2,
                "`core:lib/Manifest.ns` is no shipped module's path: `core:` takes a file name \
                 ending in `.ns`",
            ),
        ),
        (
            Record {
                path: ".ns",
                ..MANIFEST_RECORD
            },
            module_error(
                2,
                "`core:.ns` is no shipped module's path: `core:` takes a file name ending in \
                 `.ns`",
            ),
        ),
    ];
    for (record, expected) in core_refused {
        let crafted = with_module_records(
            &bytes,
            &[
                project("a/Util.ns", "Util", Some("1.0.0")),
                project("b/Util.ns", "Util", Some("2.0.0")),
                record,
            ],
        );
        assert_eq!(load_error(&crafted), expected);
    }

    // A program's module named as another of the binary's modules: its id
    // ends in its own class's name, and still it is no manifest.
    let (mut interpreter, kernel) = load(indoc! {"
        class Kernel = Object new () () : ()
    "});
    let kernel_image = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &kernel);
        snapshot_one(mutator, false, "Kernel.ns", class, &[]).expect("snapshot")
    });
    assert!(
        image::load(&kernel_image).is_ok(),
        "the fixture itself loads"
    );
    let crafted = with_module_records(
        &kernel_image,
        &[Record {
            origin: ORIGIN_CORE,
            path: "Kernel.ns",
            id: "newspeak.core.Kernel",
            version: None,
        }],
    );
    assert_eq!(
        load_error(&crafted),
        module_error(
            0,
            "`newspeak.core.Kernel` at `core:Kernel.ns` is no module an image carries from the \
             binary: only `newspeak.core.Manifest` at `core:Manifest.ns`, without a version",
        )
    );
}

/// A module record's tags are read before anything else of it: the retired
/// origins -- the building machine's file and an embedded source -- an
/// origin past the last, and a version tag past the last are each an
/// unknown tag, and inspect refuses them as load does.
// spec: image.modules
#[test]
fn a_module_record_s_unknown_tags_are_refused() {
    let bytes = util_image(&util_identities(), false);
    for origin in RETIRED_ORIGIN_TAGS.into_iter().chain([ORIGIN_CORE + 1]) {
        let crafted = with_module_records(
            &bytes,
            &[
                Record {
                    origin,
                    ..project("a/Util.ns", "Util", Some("1.0.0"))
                },
                project("b/Util.ns", "Util", Some("2.0.0")),
                MANIFEST_RECORD,
            ],
        );
        let expected = decode::DecodeError::InvalidTag {
            what: "module origin",
            tag: origin,
        }
        .to_string();
        assert_eq!(load_error(&crafted), expected);
        match image::inspect::inspect(&crafted) {
            Ok(_) => panic!("inspect read origin {origin}"),
            Err(error) => assert_eq!(error, expected),
        }
    }
    let crafted = with_module_records(
        &bytes,
        &[
            project("a/Util.ns", "Util", Some("1.0.0")),
            project("b/Util.ns", "Util", Some("2.0.0")),
            MANIFEST_RECORD,
        ],
    );
    // The first module's version tag: past the count, the origin tag, the
    // path and the id.
    let version_at =
        module_section_start(&crafted) + 4 + 1 + 4 + "a/Util.ns".len() + 4 + "Util".len();
    assert_eq!(crafted[version_at], MODULE_VERSIONED);
    let mut unknown = crafted;
    unknown[version_at] = MODULE_VERSIONED + 1;
    assert_eq!(
        load_error(&unknown),
        decode::DecodeError::InvalidTag {
            what: "module version",
            tag: MODULE_VERSIONED + 1,
        }
        .to_string()
    );
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
    Rc::new(CompiledCode::new(
        kind,
        instructions,
        argument_count,
        register_count,
        0,
        vec![],
        Span::new(0, 0),
    ))
}

/// `code_with`, plus a `copied_count` for the bodies that declare one.
fn code_with_copied(
    kind: CodeKind,
    argument_count: u16,
    register_count: u16,
    copied_count: u16,
    instructions: Vec<Instruction>,
) -> Rc<CompiledCode> {
    let mut code = (*code_with(kind, argument_count, register_count, instructions)).clone();
    code.copied_count = copied_count;
    // The copied values are locals, below the scratch.
    code.local_count = (argument_count + HEADER_REGISTERS + copied_count).min(register_count);
    Rc::new(code)
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

// spec: bytecode.verify.control-flow
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

// spec: bytecode.verify.statement-map
#[test]
fn a_statement_map_leaving_its_body_is_refused() {
    // The statement map places a frame's instruction index in its source
    // for the debugger and the trace, so a range past the body would place
    // it nowhere.
    // A backwards range is built field by field: the literal `1..0` is
    // what this refuses, and clippy refuses it too.
    let backwards = std::ops::Range { start: 1, end: 0 };
    for (range, start, end) in [(0..5, 0, 5), (backwards, 1, 0)] {
        let code = Rc::new(CompiledCode::new(
            CodeKind::Method,
            vec![Instruction::Return { register: 13 }],
            0,
            14,
            0,
            vec![(0, range)],
            Span::new(0, 0),
        ));
        let error = decode::verify_code_table(&table_of(vec![code]))
            .expect_err("a statement range outside its body");
        assert_eq!(
            error,
            decode::DecodeError::StatementMapOutOfRange {
                statement: 0,
                start,
                end,
                instructions: 1
            }
        );
        assert_eq!(
            error.to_string(),
            format!(
                "statement 0's instructions {start}..{end} are not a range of a body of 1 instruction(s)"
            )
        );
    }
}

// spec: bytecode.verify.control-flow
#[test]
fn every_conditional_jump_target_is_checked_too() {
    // One case per jump-carrying instruction: a check that only covered
    // `Jump` would leave three unchecked `program_counter` writes in the VM.
    for instruction in [
        Instruction::JumpIfNotBoolean {
            register: 13,
            target: 9,
        },
        Instruction::BranchIfFalse {
            register: 13,
            target: 9,
        },
        Instruction::BranchIfTrue {
            register: 13,
            target: 9,
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 13,
            left: 13,
            right: 13,
            target: 9,
        },
    ] {
        let table = table_of(vec![code_with(
            CodeKind::Method,
            0,
            14,
            vec![instruction, Instruction::Return { register: 13 }],
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

// spec: bytecode.verify.registers
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

// spec: bytecode.verify.registers
#[test]
fn a_register_in_a_send_argument_list_is_checked_too() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        14,
        vec![
            Instruction::SendPublic {
                selector: Symbol::intern("foo:"),
                receiver_register: 13,
                argument_base: 13,
                argument_sources: vec![13, 17],
                destination: 13,
                call_site: CallSite { number: 0 },
            },
            Instruction::Return { register: 13 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 17,
            register_count: 14
        })
    );
}

// spec: bytecode.verify.registers
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
                argument_base: 0,
                argument_sources: vec![4],
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

/// A send copies each source into its slot and reads its slots, so a base
/// past the body's registers is refused even with no argument, and so are
/// slots whose last runs past them though base and sources are in range.
// spec: bytecode.register.argument-window
// spec: bytecode.verify.argument-windows
#[test]
fn a_send_whose_argument_slots_leave_the_body_is_refused() {
    let send = |argument_base, argument_sources: Vec<u16>| {
        table_of(vec![code_with(
            CodeKind::Method,
            0,
            15,
            vec![
                Instruction::LoadSelf { destination: 13 },
                Instruction::LoadSelf { destination: 14 },
                Instruction::SendLexical {
                    depth: 0,
                    selector: Symbol::intern("foo:bar:"),
                    argument_base,
                    argument_sources,
                    destination: 13,
                    call_site: CallSite { number: 0 },
                },
                Instruction::Return { register: 13 },
            ],
        )])
    };
    assert_eq!(
        decode::verify_code_table(&send(15, vec![])),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 15,
            register_count: 15
        })
    );
    let error = decode::verify_code_table(&send(14, vec![13, 14]))
        .expect_err("the second slot is r15, past fifteen registers");
    assert_eq!(
        error,
        decode::DecodeError::ArgumentSlotsOutOfRange {
            argument_base: 14,
            argument_count: 2,
            register_count: 15
        }
    );
    assert_eq!(
        error.to_string(),
        "2 argument slot(s) from r14 run past a body with 15 register(s)"
    );
    assert_eq!(decode::verify_code_table(&send(13, vec![13, 14])), Ok(()));
}

/// The register numbers between a body's arguments and the rest of its
/// registers are its frame header's words, which no instruction may name:
/// a write there would overwrite a live frame's caller or register count.
/// An own register, an outer local's index and a send's slots below the
/// header's end are each refused, in a body taking arguments too.
// spec: bytecode.register.layout
// spec: bytecode.verify.registers
#[test]
fn a_register_naming_the_frame_header_is_refused() {
    let own = table_of(vec![code_with(
        CodeKind::Method,
        1,
        HEADER_REGISTERS + 2,
        vec![
            Instruction::LoadSelf { destination: 5 },
            Instruction::Return { register: 0 },
        ],
    )]);
    let error = decode::verify_code_table(&own).expect_err("r5 is a header word");
    assert_eq!(
        error,
        decode::DecodeError::HeaderRegister {
            depth: 0,
            register: 5,
            argument_count: 1
        }
    );
    assert_eq!(
        error.to_string(),
        "register r5 at depth 0 names the frame header of a body taking 1 argument(s)"
    );
    assert_eq!(
        decode::verify_code_table(&outer_local_table(17, 1, 4)),
        Err(decode::DecodeError::HeaderRegister {
            depth: 1,
            register: 4,
            argument_count: 0
        })
    );
    let send_below = |argument_base| {
        table_of(vec![code_with(
            CodeKind::Method,
            2,
            HEADER_REGISTERS + 4,
            vec![
                Instruction::SendLexical {
                    depth: 0,
                    selector: Symbol::intern("foo"),
                    argument_base,
                    argument_sources: vec![],
                    destination: HEADER_REGISTERS + 3,
                    call_site: CallSite { number: 0 },
                },
                Instruction::Return { register: 0 },
            ],
        )])
    };
    // Among the arguments, so no own register names the header, but the
    // callee's header would lie over the sender's.
    let error = decode::verify_code_table(&send_below(1)).expect_err("slots in the arguments");
    assert_eq!(
        error,
        decode::DecodeError::ArgumentSlotsBelowTheHeader {
            argument_base: 1,
            argument_count: 2
        }
    );
    assert_eq!(
        error.to_string(),
        "argument slots from r1 lie below the frame header's end in a body taking 2 argument(s)"
    );
    assert_eq!(
        decode::verify_code_table(&send_below(HEADER_REGISTERS + 2)),
        Ok(())
    );
}

/// A callee lies on its sender's argument slots, so a register of the
/// sender's above its named locals can be a live callee's header. A
/// crafted `run` hands a block to `call:`, which runs it while lying on
/// `run`'s slots, and the block stores into `run`'s register where
/// `call:`'s header starts: the image is refused at load, before anything
/// runs, rather than let the store overwrite the caller word the pop reads.
// spec: bytecode.verify.outer-locals
#[test]
fn a_block_storing_into_its_homes_scratch_under_a_live_callee_is_refused_at_load() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public run = ( ^1 )
          public call: block = ( ^block value )
        ) : ()
    "};
    let block = HEADER_REGISTERS;
    let receiver = HEADER_REGISTERS + 1;
    let slot = HEADER_REGISTERS + 2;
    // The callee's one argument is `slot`, its header the numbers after it;
    // the header's last word is its register count, which the pop reads.
    let callee_header = slot + 1;
    let callee_register_count_word = callee_header + HEADER_REGISTERS - 1;
    let block_body = Rc::new(CompiledCode::new(
        CodeKind::Block,
        vec![
            Instruction::LoadSelf {
                destination: HEADER_REGISTERS,
            },
            Instruction::StoreOuterLocal {
                source: HEADER_REGISTERS,
                depth: 1,
                index: callee_register_count_word,
            },
            Instruction::ImplicitReturn {
                register: HEADER_REGISTERS,
            },
        ],
        0,
        HEADER_REGISTERS + 1,
        0,
        vec![(0, 0..3)],
        Span::new(0, 0),
    ));
    let run_body = Rc::new(CompiledCode::new(
        CodeKind::Method,
        vec![
            Instruction::MakeClosure {
                destination: block,
                parameters: Rc::new(vec![]),
                code: block_body,
                copied: Vec::new(),
            },
            Instruction::LoadSelf {
                destination: receiver,
            },
            Instruction::Move {
                destination: slot,
                source: block,
            },
            Instruction::SendPublic {
                receiver_register: receiver,
                selector: Symbol::intern("call:"),
                argument_base: slot,
                argument_sources: vec![slot],
                destination: receiver,
                call_site: CallSite { number: 0 },
            },
            Instruction::Return { register: receiver },
        ],
        0,
        callee_header + HEADER_REGISTERS,
        0,
        vec![(0, 0..5)],
        Span::new(0, 0),
    ));
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "run", run_body);
        install_mixin(mutator, class, doctored);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    let Err(error) = image::load(&bytes) else {
        panic!("a store into the home's scratch must be refused at load")
    };
    let expected = decode::DecodeError::OuterLocalInScratch {
        depth: 1,
        index: callee_register_count_word,
        local_count: HEADER_REGISTERS,
    };
    assert!(error.contains(&expected.to_string()), "{error}");
}

/// `[block, parent]` as the encoder writes them (post-order), where
/// `parent` takes `parent_arguments` arguments in `parent_registers`
/// registers, the first `parent_local_count` of them its locals, and
/// `block` runs `access` -- an outer load or store -- and answers r13.
fn outer_access_table(
    parent_arguments: u16,
    parent_registers: u16,
    parent_local_count: u16,
    access: Instruction,
) -> decode::DecodedCodeTable {
    let block = code_with(
        CodeKind::Block,
        0,
        HEADER_REGISTERS + 1,
        vec![
            Instruction::LoadSelf {
                destination: HEADER_REGISTERS,
            },
            access,
            Instruction::ImplicitReturn {
                register: HEADER_REGISTERS,
            },
        ],
    );
    let closure = parent_arguments + HEADER_REGISTERS;
    let mut parent = (*code_with(
        CodeKind::Method,
        parent_arguments,
        parent_registers,
        vec![
            Instruction::MakeClosure {
                destination: closure,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: closure },
        ],
    ))
    .clone();
    parent.local_count = parent_local_count;
    table_of(vec![block, Rc::new(parent)])
}

/// No compiled body writes an argument register: parameters are immutable
/// (§3.6), and the register is the sender's slot, which a setter's sender
/// reads back after the send. A write there by its own body or by a block
/// of it is refused; a read is not; and the load of the too-large error
/// into the last argument, which the body a body past the register ceiling
/// compiles to makes when its arguments leave no register above the
/// header, is no write, since it raises before writing.
// spec: bytecode.register.arguments
// spec: bytecode.verify.registers
#[test]
fn a_write_to_an_argument_register_is_refused() {
    let own = |instructions| {
        table_of(vec![code_with(
            CodeKind::Method,
            1,
            HEADER_REGISTERS + 2,
            instructions,
        )])
    };
    let error = decode::verify_code_table(&own(vec![
        Instruction::LoadSelf { destination: 0 },
        Instruction::Return { register: 0 },
    ]))
    .expect_err("r0 is the argument");
    assert_eq!(
        error,
        decode::DecodeError::ArgumentRegisterWritten {
            depth: 0,
            register: 0,
            argument_count: 1
        }
    );
    assert_eq!(
        error.to_string(),
        "register r0 at depth 0 is written, an argument of a body taking 1 argument(s)"
    );
    assert_eq!(
        decode::verify_code_table(&own(vec![
            Instruction::LoadSelf {
                destination: HEADER_REGISTERS + 1,
            },
            Instruction::StoreLocal {
                source: HEADER_REGISTERS + 1,
                slot: 0,
            },
            Instruction::Return { register: 0 },
        ])),
        Err(decode::DecodeError::ArgumentRegisterWritten {
            depth: 0,
            register: 0,
            argument_count: 1
        })
    );
    let store = Instruction::StoreOuterLocal {
        source: HEADER_REGISTERS,
        depth: 1,
        index: 0,
    };
    assert_eq!(
        decode::verify_code_table(&outer_access_table(
            1,
            HEADER_REGISTERS + 2,
            HEADER_REGISTERS + 1,
            store
        )),
        Err(decode::DecodeError::ArgumentRegisterWritten {
            depth: 1,
            register: 0,
            argument_count: 1
        })
    );
    let load = Instruction::LoadOuterLocal {
        destination: HEADER_REGISTERS,
        depth: 1,
        index: 0,
    };
    assert_eq!(
        decode::verify_code_table(&outer_access_table(
            1,
            HEADER_REGISTERS + 2,
            HEADER_REGISTERS + 1,
            load
        )),
        Ok(())
    );
    // Past the ceiling: no register above the header, so the error is
    // loaded into the last argument, and nothing else may be.
    let arguments = u16::MAX - HEADER_REGISTERS + 1;
    let too_large = |literal| {
        table_of(vec![code_with(
            CodeKind::Method,
            arguments,
            arguments,
            vec![
                Instruction::LoadLiteral {
                    destination: arguments - 1,
                    literal,
                },
                Instruction::ImplicitReturn {
                    register: arguments - 1,
                },
            ],
        )])
    };
    assert_eq!(
        decode::verify_code_table(&too_large(Literal::BodyTooLarge(BodyLimit::Registers))),
        Ok(())
    );
    assert_eq!(
        decode::verify_code_table(&too_large(Literal::Nil)),
        Err(decode::DecodeError::ArgumentRegisterWritten {
            depth: 0,
            register: arguments - 1,
            argument_count: arguments
        })
    );
}

/// A register of the enclosing frame at or past its `local_count` is
/// scratch, where a callee it sends to lies with its header while the block
/// runs: an outer load or store there is refused, one into a named local
/// verifies.
// spec: bytecode.verify.outer-locals
#[test]
fn an_outer_local_in_the_enclosing_frames_scratch_is_refused() {
    let local_count = HEADER_REGISTERS + 1;
    let load = |index| Instruction::LoadOuterLocal {
        destination: HEADER_REGISTERS,
        depth: 1,
        index,
    };
    let store = |index| Instruction::StoreOuterLocal {
        source: HEADER_REGISTERS,
        depth: 1,
        index,
    };
    let table = |access| outer_access_table(0, HEADER_REGISTERS + 4, local_count, access);
    assert_eq!(
        decode::verify_code_table(&table(load(HEADER_REGISTERS))),
        Ok(())
    );
    assert_eq!(
        decode::verify_code_table(&table(store(HEADER_REGISTERS))),
        Ok(())
    );
    let error = decode::verify_code_table(&table(load(local_count))).expect_err("a scratch load");
    assert_eq!(
        error,
        decode::DecodeError::OuterLocalInScratch {
            depth: 1,
            index: local_count,
            local_count
        }
    );
    assert_eq!(
        error.to_string(),
        "outer local r14 at depth 1 is a scratch register of the enclosing body, whose first is r14"
    );
    assert_eq!(
        decode::verify_code_table(&table(store(local_count + 2))),
        Err(decode::DecodeError::OuterLocalInScratch {
            depth: 1,
            index: local_count + 2,
            local_count
        })
    );
}

/// What makes the refusal above sound: a send's slots, and so its callee's
/// header, start at or above its body's `local_count`, which is itself
/// inside the register file.
// spec: bytecode.register.argument-window
// spec: bytecode.verify.argument-windows
#[test]
fn a_send_whose_slots_start_below_the_scratch_is_refused() {
    let body = |local_count, argument_base| {
        let mut code = (*code_with(
            CodeKind::Method,
            0,
            HEADER_REGISTERS + 3,
            vec![
                Instruction::SendLexical {
                    depth: 0,
                    selector: Symbol::intern("foo"),
                    argument_base,
                    argument_sources: vec![],
                    destination: HEADER_REGISTERS + 2,
                    call_site: CallSite { number: 0 },
                },
                Instruction::Return {
                    register: HEADER_REGISTERS + 2,
                },
            ],
        ))
        .clone();
        code.local_count = local_count;
        table_of(vec![Rc::new(code)])
    };
    let error = decode::verify_code_table(&body(HEADER_REGISTERS + 1, HEADER_REGISTERS))
        .expect_err("slots on a named local");
    assert_eq!(
        error,
        decode::DecodeError::ArgumentSlotsBelowTheScratch {
            argument_base: HEADER_REGISTERS,
            local_count: HEADER_REGISTERS + 1
        }
    );
    assert_eq!(
        error.to_string(),
        "argument slots from r13 lie below the first scratch register r14"
    );
    assert_eq!(
        decode::verify_code_table(&body(HEADER_REGISTERS + 1, HEADER_REGISTERS + 1)),
        Ok(())
    );
    let error = decode::verify_code_table(&body(HEADER_REGISTERS + 4, HEADER_REGISTERS + 1))
        .expect_err("a local count past the file");
    assert_eq!(
        error,
        decode::DecodeError::LocalCountOutOfRange {
            local_count: HEADER_REGISTERS + 4,
            register_count: HEADER_REGISTERS + 3
        }
    );
    assert_eq!(
        error.to_string(),
        "a local count of 17 is past a body with 16 register(s)"
    );
}

/// A send writes each source into its slot in turn, so a source that is
/// another of its own slots would answer whatever the order of the writes
/// left there: refused, while a source in its own slot, or outside the
/// slots, verifies.
// spec: bytecode.register.argument-window
// spec: bytecode.verify.argument-windows
#[test]
fn a_send_whose_source_is_another_of_its_slots_is_refused() {
    let send = |argument_sources: Vec<u16>| {
        table_of(vec![code_with(
            CodeKind::Method,
            0,
            HEADER_REGISTERS + 3,
            vec![
                Instruction::LoadSelf {
                    destination: HEADER_REGISTERS,
                },
                Instruction::LoadSelf {
                    destination: HEADER_REGISTERS + 1,
                },
                Instruction::LoadSelf {
                    destination: HEADER_REGISTERS + 2,
                },
                Instruction::SendLexical {
                    depth: 0,
                    selector: Symbol::intern("foo:bar:"),
                    argument_base: HEADER_REGISTERS + 1,
                    argument_sources,
                    destination: HEADER_REGISTERS,
                    call_site: CallSite { number: 0 },
                },
                Instruction::Return {
                    register: HEADER_REGISTERS,
                },
            ],
        )])
    };
    let error = decode::verify_code_table(&send(vec![HEADER_REGISTERS + 2, HEADER_REGISTERS]))
        .expect_err("the first source is the second slot");
    assert_eq!(
        error,
        decode::DecodeError::SourceIsAnotherSlot {
            argument_base: HEADER_REGISTERS + 1,
            position: 0,
            source: HEADER_REGISTERS + 2
        }
    );
    assert_eq!(
        error.to_string(),
        "the source of argument 0 of a send with slots from r14 is r15, another of its slots"
    );
    assert_eq!(
        decode::verify_code_table(&send(vec![HEADER_REGISTERS, HEADER_REGISTERS + 1])),
        Err(decode::DecodeError::SourceIsAnotherSlot {
            argument_base: HEADER_REGISTERS + 1,
            position: 1,
            source: HEADER_REGISTERS + 1
        })
    );
    assert_eq!(
        decode::verify_code_table(&send(vec![HEADER_REGISTERS + 1, HEADER_REGISTERS])),
        Ok(())
    );
    assert_eq!(
        decode::verify_code_table(&send(vec![HEADER_REGISTERS, HEADER_REGISTERS])),
        Ok(())
    );
}

/// The loop's signal reads its register like any other operand, so a
/// decoded one out of range is refused before the VM reads it.
// spec: bytecode.verify.registers
#[test]
fn a_register_in_a_non_boolean_signal_is_checked_too() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![Instruction::SignalNonBoolean { register: 4 }],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 4,
            register_count: 1
        })
    );
}

/// Nothing runs after the loop's signal, so a body may end in it, as the
/// optimized lowering's never does but a decoded one may.
#[test]
fn a_body_that_ends_in_a_non_boolean_signal_is_terminated() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        14,
        vec![
            Instruction::LoadSelf { destination: 13 },
            Instruction::SignalNonBoolean { register: 13 },
        ],
    )]);
    assert_eq!(decode::verify_code_table(&table), Ok(()));
}

// spec: bytecode.verify.control-flow
#[test]
fn a_body_that_runs_past_its_last_instruction_is_refused() {
    // The VM fetches `code.slot(program_counter)` after every
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

/// An initializer body carries the instructions of a compiled `new`, so a
/// loaded body may hold each of them.
#[test]
fn a_body_with_an_instantiation_instruction_is_accepted() {
    let instructions = every_instantiation_instruction().instructions();
    let register_count = 2 + HEADER_REGISTERS + 601;
    for instruction in instructions
        .iter()
        .filter(|instruction| !matches!(instruction, Instruction::Return { .. }))
    {
        // Every register the instruction may read written first, as a
        // compiled body writes it.
        let mut body: Vec<Instruction> = (HEADER_REGISTERS..register_count)
            .map(|destination| Instruction::LoadSelf { destination })
            .collect();
        body.push(instruction.clone());
        body.push(Instruction::Return {
            register: HEADER_REGISTERS,
        });
        let table = table_of(vec![code_with(
            CodeKind::Initializer,
            0,
            register_count,
            body,
        )]);
        assert_eq!(decode::verify_code_table(&table), Ok(()), "{instruction:?}");
    }
}

// spec: bytecode.verify.body-fields
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

// spec: bytecode.verify.blocks
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

// spec: bytecode.verify.blocks
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

// spec: bytecode.verify.blocks
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
// spec: bytecode.verify.registers
#[test]
fn a_make_closure_copying_a_register_its_own_body_lacks_is_refused() {
    let block = code_with_copied(
        CodeKind::Block,
        0,
        15,
        1,
        vec![Instruction::ImplicitReturn { register: 13 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        14,
        vec![
            Instruction::MakeClosure {
                destination: 13,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: vec![17],
            },
            Instruction::Return { register: 13 },
        ],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            register: 17,
            register_count: 14
        })
    );
}

// spec: bytecode.verify.blocks
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
// spec: bytecode.wire.call-sites
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
                argument_base: 1,
                argument_sources: vec![],
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

// spec: bytecode.verify.blocks
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
        14,
        vec![
            Instruction::LoadOuterLocal {
                destination: 13,
                depth: outer_depth,
                index: outer_index,
            },
            Instruction::ImplicitReturn { register: 13 },
        ],
    );
    // Every register of the parent a local, so that the bound the test
    // asks about is its register file's.
    let mut parent = (*code_with(
        CodeKind::Method,
        0,
        parent_registers,
        vec![
            Instruction::MakeClosure {
                destination: 13,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 13 },
        ],
    ))
    .clone();
    parent.local_count = parent_registers;
    table_of(vec![block, Rc::new(parent)])
}

#[test]
fn an_outer_local_within_its_lexical_parents_register_file_verifies() {
    decode::verify_code_table(&outer_local_table(17, 1, 16))
        .expect("r16 of a seventeen-register parent");
}

// spec: bytecode.verify.outer-locals
#[test]
fn an_outer_local_past_its_lexical_parents_register_file_is_refused() {
    // `LoadOuterLocal`'s index addresses the *outer* frame, so the body's
    // own `register_count` says nothing about it: the bound is the register
    // file of the body one `MakeClosure` hop out.
    let error = decode::verify_code_table(&outer_local_table(15, 1, 16)).expect_err("out of range");
    assert_eq!(
        error,
        decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 16,
            register_count: 15
        }
    );
    assert_eq!(
        error.to_string(),
        "outer local r16 at depth 1 is outside the enclosing body's 15 register(s)"
    );
}

// spec: bytecode.verify.outer-locals
#[test]
fn an_outer_local_at_depth_zero_is_bounded_by_its_own_register_file() {
    // `frame_at_depth(frame, 0)` is the frame itself.
    assert_eq!(
        decode::verify_code_table(&outer_local_table(17, 0, 16)),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 0,
            index: 16,
            register_count: 14
        })
    );
}

// spec: bytecode.verify.outer-locals
#[test]
fn an_outer_local_deeper_than_the_lexical_nesting_is_bounded_by_the_outermost_body() {
    // `frame_at_depth` stops hopping at a non-closure activation, so a
    // depth past the nesting lands on the outermost frame -- the method's,
    // with two registers here, not the block's own.
    assert_eq!(
        decode::verify_code_table(&outer_local_table(15, 9, 16)),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 9,
            index: 16,
            register_count: 15
        })
    );
}

// spec: bytecode.verify.outer-locals
#[test]
fn an_outer_local_naming_its_outer_bodys_header_is_refused() {
    // Register 0 of a method without arguments is the first number of its
    // frame header, which holds no variable: a block reading it through an
    // outer local would read a header word.
    assert_eq!(
        decode::verify_code_table(&outer_local_table(15, 1, 0)),
        Err(decode::DecodeError::HeaderRegister {
            depth: 1,
            register: 0,
            argument_count: 0
        })
    );
}

// spec: bytecode.verify.outer-locals
#[test]
fn a_store_to_an_outer_local_is_checked_like_a_load() {
    let block = code_with(
        CodeKind::Block,
        0,
        14,
        vec![
            Instruction::LoadSelf { destination: 13 },
            Instruction::StoreOuterLocal {
                source: 13,
                depth: 1,
                index: 19,
            },
            Instruction::ImplicitReturn { register: 13 },
        ],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        15,
        vec![
            Instruction::MakeClosure {
                destination: 13,
                parameters: Rc::new(vec![]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return { register: 13 },
        ],
    );
    assert_eq!(
        decode::verify_code_table(&table_of(vec![block, parent])),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 19,
            register_count: 15
        })
    );
}

// ---------------------------------------------------------------------------
// `verify_mixins`'s arity and kind reconciliation.
// ---------------------------------------------------------------------------

// spec: bytecode.verify.attachment
#[test]
fn a_method_body_taking_the_wrong_number_of_arguments_is_refused_on_load() {
    // A method frame's `arguments` come from the send, so their count is the
    // selector's arity; the frame's temporaries are sized from the *body's*
    // `argument_count`. A body claiming a different arity leaves `read_local`
    // indexing `temporaries` past its end.
    let source = indoc! {"
        class X = Object new (
        ) (
          public foo: a = ( ^a )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let mut body = (*method_code(mutator.heap(), class, "foo:")).clone();
        body.argument_count = 2;
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

// spec: bytecode.verify.attachment
#[test]
fn a_method_root_naming_a_block_body_is_refused_on_load() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public foo = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let mut body = (*method_code(mutator.heap(), class, "foo")).clone();
        body.kind = CodeKind::Block;
        let doctored = with_method_code(mixin_of(mutator.heap(), class), "foo", Rc::new(body));
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        assert_eq!(
            decode::verify_mixins(&decoded_mixins),
            Err(decode::DecodeError::WrongCodeKind { expected: "method" })
        );
    });
}

/// A declaration with two `<:` clauses, the second passing an argument,
/// so that `C` compiles all three initializer bodies.
const TWO_APPLIED_CLAUSES: &str = indoc! {"
    class Outer = Object new () (
      class S = Object new () ()
      class M1 = Object new () ()
      class M2 tag: value = Object new () ()
      class C = S new <: M1 new <: M2 tag: 1 (| c = 3. |) ()
    ) : ()
"};

/// The decoded mixin of the row named `name`.
fn decoded_mixin<'decoded>(decoded: &'decoded mixins::Decoded, name: &str) -> &'decoded Mixin {
    decoded
        .mixins
        .iter()
        .find(|mixin| mixin.name.as_str() == name)
        .unwrap_or_else(|| panic!("a row named {name}"))
}

// spec: image.mixins
#[test]
fn all_three_initializer_bodies_survive_a_round_trip() {
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let applying = mutator
            .heap()
            .nested_mixins(&root_mixin_of(mutator.heap(), class))
            .into_iter()
            .find(|nested| nested.name.as_str() == "C")
            .expect("Outer declares C");
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let decoded = decoded_mixin(&decoded_mixins, "C");
        for (what, original, decoded) in [
            (
                "full",
                &applying.initializer_code,
                &decoded.initializer_code,
            ),
            (
                "own",
                &applying.own_initializer_code,
                &decoded.own_initializer_code,
            ),
            (
                "standalone",
                &applying.standalone_initializer_code,
                &decoded.standalone_initializer_code,
            ),
        ] {
            let original = original.as_ref().expect("C compiles every body");
            let decoded = decoded.as_ref().expect("the row names every body");
            assert_eq!(**decoded, **original, "{what}");
        }
        assert_eq!(decode::verify_mixins(&decoded_mixins), Ok(()));
    });
}

/// An image never carries a factory body: the decoder gives each factory
/// entry the one `compiler::factory_body` synthesizes for its arity.
#[test]
fn a_decoded_factory_entry_carries_the_synthesized_body() {
    let source = indoc! {"
        class Pair x: first y: second = Object new () (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let class_side = decoded_mixins
            .mixins
            .iter()
            .find(|mixin| mixin.is_class_side())
            .expect("Pair's class side has a row");
        match class_side.methods.get(&Symbol::intern("x:y:")) {
            Some(ns_runtime::mixin::MethodEntry::Factory(factory)) => {
                assert_eq!(factory.argument_count, 2);
                assert_eq!(*factory.code, ns_runtime::compiler::factory_body(2));
            }
            _ => panic!("the class side carries the factory entry"),
        }
    });
}

/// Each initializer body runs with the factory's arguments as its own, so
/// a body of another arity would lay its registers out from the wrong
/// count.
// spec: bytecode.verify.attachment
// spec: image.verify.initializers
#[test]
fn an_initializer_whose_arity_disagrees_with_the_factory_is_refused() {
    type Body = fn(&Mixin) -> Option<Rc<CompiledCode>>;
    type Doctor = fn(Mixin, Rc<CompiledCode>) -> Mixin;
    let bodies: [(&str, Body, Doctor); 3] = [
        (
            "instance initializer",
            |mixin| mixin.initializer_code.clone(),
            |mixin, code| Mixin {
                initializer_code: Some(code),
                ..mixin
            },
        ),
        (
            "own part of the instance initializer",
            |mixin| mixin.own_initializer_code.clone(),
            |mixin, code| Mixin {
                own_initializer_code: Some(code),
                ..mixin
            },
        ),
        (
            "standalone instance initializer",
            |mixin| mixin.standalone_initializer_code.clone(),
            |mixin, code| Mixin {
                standalone_initializer_code: Some(code),
                ..mixin
            },
        ),
    ];
    for (what, body, doctor) in bodies {
        let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
        interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            let doctored = with_nested(mutator.heap(), class, "C", |nested| {
                let mut code = (*body(&nested).expect("C compiles every body")).clone();
                code.argument_count = 1;
                doctor(nested, Rc::new(code))
            });
            install_mixin(mutator, class, doctored);
            let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
            assert_eq!(
                decode::verify_mixins(&decoded_mixins),
                Err(decode::DecodeError::CodeArityMismatch {
                    what: what.to_string(),
                    parameters: 0,
                    argument_count: 1
                }),
                "{what}"
            );
        });
    }
}

/// A row's `<:` clauses and its standalone initializer go together: a level
/// of the mixin reached from outside runs the standalone body, which only a
/// declaration with clauses has.
// spec: image.verify.initializers
#[test]
fn a_mixin_with_clauses_but_no_standalone_initializer_is_refused() {
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_nested(mutator.heap(), class, "C", |nested| Mixin {
            standalone_initializer_code: None,
            ..nested
        });
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no standalone body");
        assert_eq!(error, decode::DecodeError::MissingStandaloneInitializer);
        assert_eq!(
            error.to_string(),
            "a class in the image applies mixins, and the image has no compiled code for its standalone instance initializer"
        );
    });
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_nested(mutator.heap(), class, "S", |nested| Mixin {
            standalone_initializer_code: nested.initializer_code.clone(),
            ..nested
        });
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("a stray standalone body");
        assert_eq!(error, decode::DecodeError::UnexpectedStandaloneInitializer);
        assert_eq!(
            error.to_string(),
            "a class in the image applies no mixin or has no body, yet the image carries a standalone instance initializer for it"
        );
    });
}

/// Every row's class runs its full initializer when it is instantiated, so
/// a row naming none is refused whatever its slots.
// spec: image.verify.initializers
#[test]
fn a_mixin_without_a_full_initializer_is_refused() {
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_nested(mutator.heap(), class, "S", |nested| Mixin {
            initializer_code: None,
            ..nested
        });
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no full body");
        assert_eq!(error, decode::DecodeError::MissingFullInitializer);
        assert_eq!(
            error.to_string(),
            "a class in the image has no compiled code for its instance initializer"
        );
    });
}

// ---------------------------------------------------------------------------
// `verify_graph` -- reconciling decoded frames and closures with
// the code that runs in them.
// ---------------------------------------------------------------------------

const ONE_METHOD: &str = indoc! {"
    class X = Object new (
    ) (
      public foo = ( ^1 )
    ) : ()
"};

/// A world whose code table is `[block, foo]` -- `foo`'s body is the one
/// `MakeClosure` that creates `block` -- with the class's `foo` carrying
/// that body. Everything a decoded closure over `block`
/// needs to verify: its lexical parent is `foo`, which has two registers
/// above its header.
fn closure_world() -> (GraphFixture, decode::DecodedCodeTable) {
    closure_world_over(code_with(
        CodeKind::Block,
        0,
        HEADER_REGISTERS + 1,
        vec![Instruction::ImplicitReturn {
            register: HEADER_REGISTERS,
        }],
    ))
}

/// `closure_world` with the block `foo` creates given.
fn closure_world_over(block: Rc<CompiledCode>) -> (GraphFixture, decode::DecodedCodeTable) {
    let (mut interpreter, class) = load(ONE_METHOD);
    // Two registers above the header, so two register fields in a frame.
    let parent = code_with(
        CodeKind::Method,
        0,
        HEADER_REGISTERS + 2,
        vec![
            Instruction::MakeClosure {
                destination: HEADER_REGISTERS,
                parameters: Rc::new(vec![
                    Symbol::intern("parameter");
                    usize::from(block.argument_count)
                ]),
                code: block.clone(),
                copied: Vec::new(),
            },
            Instruction::Return {
                register: HEADER_REGISTERS,
            },
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
    build_fixture_names(&mut interpreter);
    let fixture = GraphFixture {
        codes: table.codes.clone(),
        interpreter,
        // `foo_frame` below writes the `Method` frame kind's selector as id
        // 0, which must resolve to "foo" -- the method `install_mixin` just
        // gave the compiled body to.
        symbols: table_with(&["foo"]),
        mixins,
        names: fixture_names(),
        module_roots: vec![2],
    };

    (fixture, table)
}

/// A graph section holding one closure object over code id 0 and the one
/// frame it names as its `captured_frame`, object 1, whose record `frame`
/// writes after its header. The home follows from the captured frame, so the
/// record carries no second id. The module root's two records follow at 2
/// (`root_class_records`), the class a frame with no current class resolves
/// its method against.
fn closure_over_frame(frame: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 4);
    writer.write_shipped(CLOSURE_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(0); // code id 0 -- the block body
    writer.write_u32(1); // captured frame -- object 1
    writer.write_u16(0); // copying nothing
    frame_header(&mut writer, 0);
    frame(&mut writer);
    root_class_records(&mut writer, 2);
    writer.write_u32(1); // one root...
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(0); // ...the closure
    writer.into_bytes()
}

/// The frame as an activation of method `foo` with `registers` registers.
fn foo_frame(registers: u32) -> impl FnOnce(&mut encode::Writer) {
    foo_frame_at_program_counter(registers, 0)
}

/// `foo_frame` with the activation's `program_counter` left to the caller.
fn foo_frame_at_program_counter(
    registers: u32,
    program_counter: u32,
) -> impl FnOnce(&mut encode::Writer) {
    move |writer: &mut encode::Writer| {
        writer.write_u8(FRAME_METHOD); // frame kind: Method
        writer.write_u32(0); // selector id 0 -- "foo", per `closure_world`'s fixture
        writer.write_u8(mixins::access_tag(AccessModifier::Public));
        runs_body(writer, 1); // code id 1 -- `foo`'s body
        frame_tail(writer, program_counter, registers);
    }
}

/// An image is written only when no activation runs, so a frame it carries
/// has returned and is marked so. One that is not could be resumed from the
/// registers and program counter its author wrote.
#[test]
fn a_frame_not_marked_uncontinuable_is_refused() {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    no_body(&mut writer);
    frame_tail_marked(&mut writer, 0, 0, false, None);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::ContinuableFrame
    );
}

/// Only a closure's activation has a home, the activation its `^` returns
/// from; a method frame that names one is not something a run produces.
#[test]
fn a_method_frame_with_a_home_is_refused() {
    let (mut fixture, table) = closure_world();
    let bytes = closure_over_frame(|writer| {
        writer.write_u8(FRAME_METHOD); // frame kind: Method
        writer.write_u32(0); // selector id 0 -- "foo"
        writer.write_u8(mixins::access_tag(AccessModifier::Public));
        runs_body(writer, 1); // code id 1 -- `foo`'s body
        frame_tail_marked(writer, 0, 2, true, Some(1));
    });
    fixture.decoded(&bytes, "decode", |heap, graph| {
        assert_eq!(
            decode::verify_graph(heap, graph, &table),
            Err(decode::DecodeError::FrameHomeMismatch)
        );
    });
}

/// The record can leave a closure frame's home out, and a `^` in its body
/// would then have nowhere to return to.
#[test]
fn a_closure_frame_without_a_home_is_refused() {
    let (mut fixture, table) = closure_world();
    let bytes = closure_over_frame(|writer| {
        writer.write_u8(FRAME_CLOSURE); // frame kind: a closure...
        writer.write_u32(0); // ...object 0
        runs_body(writer, 0); // code id 0 -- the block's body
        frame_tail_marked(writer, 0, 1, true, None);
    });
    fixture.decoded(&bytes, "decode", |heap, graph| {
        assert_eq!(
            decode::verify_graph(heap, graph, &table),
            Err(decode::DecodeError::FrameHomeMismatch)
        );
    });
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
            decode::verify_graph(heap, graph, &table).expect("the captured frame runs `foo`");
        },
    );
}

/// Every reified frame has its body's whole register file and the encoder
/// writes all of it, so a frame with none for a body that has some is not
/// something a run produces: refused like any other register file that
/// disagrees with its body.
#[test]
fn a_frame_with_no_registers_for_a_body_that_has_some_is_refused() {
    let (mut fixture, table) = closure_world();
    fixture.decoded(
        &closure_over_frame(foo_frame(0)),
        "decode",
        |heap, graph| {
            assert_eq!(
                decode::verify_graph(heap, graph, &table),
                Err(decode::DecodeError::FrameRegisterFile {
                    arguments: 0,
                    locals: 0,
                    argument_count: 0,
                    expected_locals: 2,
                    register_count: HEADER_REGISTERS + 2
                })
            );
        },
    );
}

#[test]
fn a_frame_whose_register_file_is_too_small_for_its_code_is_refused() {
    // `foo`'s body has two registers above its header; this activation has
    // one. A `LoadOuterLocal` of the second from the closure would index
    // past the frame's one register field, reached from a version-matching
    // image.
    let (mut fixture, table) = closure_world();
    fixture.decoded(&closure_over_frame(foo_frame(1)), "decode", |heap, graph| {
        let error = decode::verify_graph(heap, graph, &table).expect_err("undersized frame");
        assert_eq!(
            error,
            decode::DecodeError::FrameRegisterFile {
                arguments: 0,
                locals: 1,
                argument_count: 0,
                expected_locals: 2,
                register_count: HEADER_REGISTERS + 2
            }
        );
        assert_eq!(
            error.to_string(),
            "a decoded frame holds 0 argument(s) and 1 local(s), but the code that runs in it takes 0 argument(s) and 2 local(s) beside its header in 15 register(s)"
        );
    });
}

/// A frame record written with a field for each of the header's numbers,
/// as frame objects were laid out before they left them out, holds 13
/// registers too many for its body: refused rather than read with its
/// registers above the header shifted by 13.
#[test]
fn a_frame_with_fields_for_the_header_numbers_is_refused() {
    let (mut fixture, table) = closure_world();
    let stale = u32::from(HEADER_REGISTERS) + 2;
    fixture.decoded(
        &closure_over_frame(foo_frame(stale)),
        "decode",
        |heap, graph| {
            assert_eq!(
                decode::verify_graph(heap, graph, &table),
                Err(decode::DecodeError::FrameRegisterFile {
                    arguments: 0,
                    locals: usize::from(HEADER_REGISTERS) + 2,
                    argument_count: 0,
                    expected_locals: 2,
                    register_count: HEADER_REGISTERS + 2
                })
            );
        },
    );
}

/// A body past the register ceiling whose arguments leave no number above
/// the header has its arguments as its whole register file
/// (`compiler::body_too_large`), and a frame of it holds exactly those: an
/// image carrying one -- here a block of `u16::MAX` parameters, whose
/// activation is object 1 -- verifies.
#[test]
fn a_frame_of_a_body_whose_arguments_fill_the_register_ceiling_verifies() {
    let block = code_with(
        CodeKind::Block,
        u16::MAX,
        u16::MAX,
        vec![Instruction::ImplicitReturn { register: 0 }],
    );
    assert_eq!(block.frame_register_count(), usize::from(u16::MAX));
    let (mut fixture, table) = closure_world_over(block);
    fixture.module_roots = vec![3];
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 5);
    writer.write_shipped(CLOSURE_NAME);
    writer.write_u32(1); // identity hash
    writer.write_u8(OBJECT_CLOSURE); // object body: Closure
    writer.write_u32(0); // code id 0 -- the block body
    writer.write_u32(2); // captured frame -- object 2, `foo`'s activation
    writer.write_u16(0); // copying nothing
    frame_header(&mut writer, 0);
    writer.write_u8(FRAME_CLOSURE); // frame kind: a closure...
    writer.write_u32(0); // ...object 0
    runs_body(&mut writer, 0); // code id 0 -- the block's body
    frame_tail_marked(&mut writer, 0, u32::from(u16::MAX), true, Some(2));
    frame_header(&mut writer, 0);
    foo_frame(2)(&mut writer);
    root_class_records(&mut writer, 3);
    writer.write_u32(1); // one root...
    writer.write_u8(VALUE_OBJECT_ID);
    writer.write_u32(1); // ...the block's activation
    fixture.decoded(&writer.into_bytes(), "decode", |heap, graph| {
        decode::verify_graph(heap, graph, &table)
            .expect("the frame holds the block's arguments, its whole register file");
        let frame = graph.frames[0];
        assert_eq!(
            heap.frame_register_shape(frame),
            (0, usize::from(u16::MAX), usize::from(u16::MAX))
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
        writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel -- runs no code at all
        no_body(writer);
        frame_tail(writer, 0, 0);
    });
    fixture.decoded(&bytes, "decode", |heap, graph| {
        assert_eq!(
            decode::verify_graph(heap, graph, &table),
            Err(decode::DecodeError::CapturedFrameMismatch)
        );
    });
}

#[test]
fn a_real_snapshots_graph_verifies_and_its_closures_still_read_their_captured_locals() {
    // The end-to-end control: a quiesced world holding a closure over a
    // method activation, through the real sections and back.
    let source = indoc! {"
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
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
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
            mutator.fetch(&value).as_small_integer() == Some(7),
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
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
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
            object(held).same(object(expected)),
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
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let held = loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        let held = mutator
            .heap()
            .instance_field(object(root_0), 0)
            .expect("expected an instance");
        let captured = mutator
            .heap()
            .closure_captured_frame(object(held))
            .expect("the slot holds the inner closure");
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
            mutator.fetch(&value).as_small_integer() == Some(5),
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
    class X = Object new (
      | count ::= 1. |
    ) (
      public n = ( ^count )
    ) : ()
"};

// spec: image.verify.initializers
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
            mixin_of(mutator.heap(), class).own_initializer_code.is_some(),
            "the fixture's class really does compile an initializer"
        );
        let doctored = Mixin {
            own_initializer_code: None,
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        assert!(
            decoded_mixins.code[0].own_initializer.is_none(),
            "the doctored row names no own part"
        );
        let error = decode::verify_mixins(&decoded_mixins).expect_err("no initializer");
        assert_eq!(error, decode::DecodeError::MissingInitializer);
        assert_eq!(
            error.to_string(),
            "a class in the image needs an instance initializer, which the image has no compiled code for"
        );
    });
}

// spec: image.verify.initializers
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
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        // Only the initializer's own part, which the walk runs, is
        // dropped, so the refusal this asserts cannot be the superclass
        // clause's.
        let doctored = Mixin {
            own_initializer_code: None,
            ..mixin_of(mutator.heap(), class)
        };
        install_mixin(mutator, class, doctored);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let error = load_error(&bytes);
    assert!(
        error.contains("instance initializer"),
        "load must refuse it rather than let `X new` panic later: {error}"
    );
}

#[test]
fn an_identity_hash_at_the_ceiling_is_refused_rather_than_overflowing_the_counter() {
    // The header word holds 22 bits of identity hash: a wider one would
    // alias another object's hash, and `advance_identity_hash_past` could
    // not place the counter past it.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    writer.write_shipped(STRING_NAME);
    writer.write_u32(1 << 22); // identity hash, at the ceiling
    writer.write_u8(OBJECT_STRING); // object body: String
    writer.write_str("x");
    writer.write_u32(0); // no roots -- otherwise complete, so the hash is what
    // the decoder rejects, not a truncation
    let bytes = writer.into_bytes();
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::IdentityHashTooWide(1 << 22));
    assert_eq!(
        error.to_string(),
        "identity hash 4194304 does not fit the header word's 22 bits"
    );
}

#[test]
fn an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating() {
    // The widest legal hash leaves the counter at the ceiling, where the
    // next hash drawn after the load wraps to 1: hashes need not be
    // unique, since identity is the handle, never the hash.
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let instance_object = object(mutator.fetch(&instance));
        mutator
            .heap_mut()
            .object_set_identity_hash(instance_object, (1 << 22) - 1);
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(
            mutator.heap().object_identity_hash(object(root_0)),
            (1 << 22) - 1,
            "the image's hash is preserved, not clamped"
        );
        let first = mutator.new_string("a".to_string());
        let second = mutator.new_string("b".to_string());
        let heap = mutator.heap_mut();
        assert_eq!(
            heap.object_ensure_identity_hash(first),
            1,
            "the counter wraps past the widest hash, skipping 0"
        );
        assert_eq!(heap.object_ensure_identity_hash(second), 2);
    });
}

/// An object nothing asked for its hash is written as 0 and decoded as 0 --
/// still unassigned -- and the first request after the load draws a number
/// past every hash the image carried, so it cannot collide with one.
// spec: image.graph
#[test]
fn an_unhashed_object_round_trips_unassigned_and_draws_past_the_image_maximum() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let hashed = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let unhashed = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let (bytes, assigned) = interpreter.enter(|mutator| {
        let hashed_value = mutator.fetch(&hashed);
        let assigned = mutator
            .heap_mut()
            .object_ensure_identity_hash(object(hashed_value));
        let class = class_reference(mutator, &class);
        let unhashed_value = mutator.fetch(&unhashed);
        let bytes = snapshot_one(
            mutator,
            true,
            "t.ns",
            class,
            &[hashed_value, unhashed_value],
        )
        .expect("snapshot");
        assert_eq!(
            mutator.heap().object_identity_hash(object(unhashed_value)),
            0,
            "snapshotting assigns no hash"
        );
        (bytes, assigned)
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let (kept, fresh) = (
            object(mutator.fetch(&loaded.roots[0])),
            object(mutator.fetch(&loaded.roots[1])),
        );
        let heap = mutator.heap_mut();
        assert_eq!(
            heap.object_identity_hash(kept),
            assigned,
            "an assigned hash is preserved"
        );
        assert_eq!(
            heap.object_identity_hash(fresh),
            0,
            "an unassigned hash stays unassigned"
        );
        assert!(heap.object_ensure_identity_hash(fresh) > assigned);
    });
}

/// A builtin class is named, not carried as a record, so before the closing
/// hash table nothing recorded a hash it had been given: after the load the
/// bootstrap's fresh objects drew new ones (`docs/design/image-format.md`,
/// "Identity hashes are preserved verbatim"). One that nothing asked stays
/// unassigned. The root class and the platform are records, so their
/// hashes travel in their records' headers.
#[test]
fn the_hashes_of_objects_named_by_tag_survive_a_snapshot() {
    let source = indoc! {"
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
        let integer_hash = mutator.heap_mut().object_ensure_identity_hash(integer);
        let class_hash = mutator.heap_mut().object_ensure_identity_hash(class);
        let platform_hash = mutator
            .heap_mut()
            .object_ensure_identity_hash(object(platform));
        let bytes = snapshot_one(mutator, false, "t.ns", class, &[platform]).expect("snapshot");
        (bytes, integer_hash, class_hash, platform_hash)
    });

    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.graph_tag_named_hashes,
        vec![format!("Kernel Integer hash {integer_hash}")],
        "inspect reads the table, the shipped names alone"
    );
    assert!(
        report
            .graph_objects
            .iter()
            .any(|record| record.contains(&format!("hash {class_hash}, class mixin row 0"))),
        "the root class's hash is its record's: {:?}",
        report.graph_objects
    );

    let mut loaded = image::load(&bytes).expect("load");
    let decoded_platform = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let decoded_platform = mutator.fetch(&decoded_platform);
        let loaded_class = class_reference(mutator, &loaded.modules[0].class);
        let loaded_integer = mutator.builtin_class(BuiltinClass::Integer);
        let loaded_string = mutator.builtin_class(BuiltinClass::String);
        let heap = mutator.heap_mut();
        assert_eq!(heap.object_identity_hash(loaded_integer), integer_hash);
        assert_eq!(heap.object_identity_hash(loaded_class), class_hash);
        assert_eq!(
            heap.object_identity_hash(object(decoded_platform)),
            platform_hash
        );
        assert_eq!(
            heap.object_identity_hash(loaded_string),
            0,
            "a builtin nothing asked for its hash stays unassigned"
        );
        let largest = integer_hash.max(class_hash).max(platform_hash);
        assert!(
            heap.object_ensure_identity_hash(loaded_string) > largest,
            "the counter is advanced past the tag-named hashes too"
        );
    });
}

/// A shipped level is named, not carried, like a builtin class: the hash the
/// level `Array`'s `<:` clause adds was given, and its metaclass's, travel in
/// the closing table and land on the loading world's own level.
// spec: image.hashes
#[test]
fn the_hash_of_a_shipped_level_survives_a_snapshot() {
    let source = indoc! {"
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let (bytes, level_hash, metaclass_hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let array = mutator.builtin_class(BuiltinClass::Array);
        let level = mutator
            .heap()
            .class_superclass(array)
            .expect("Array has a superclass");
        let metaclass = mutator
            .heap()
            .class_metaclass(level)
            .expect("a level has a metaclass");
        let level_hash = mutator.heap_mut().object_ensure_identity_hash(level);
        let metaclass_hash = mutator.heap_mut().object_ensure_identity_hash(metaclass);
        let bytes = snapshot_one(mutator, false, "t.ns", class, &[]).expect("snapshot");
        (bytes, level_hash, metaclass_hash)
    });

    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .graph_tag_named_hashes
            .contains(&format!("Kernel Array level 0 hash {level_hash}")),
        "the level is named in the table: {:?}",
        report.graph_tag_named_hashes
    );
    assert!(
        report
            .graph_tag_named_hashes
            .contains(&format!("Kernel Array level 0 class hash {metaclass_hash}"))
    );

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let array = mutator.builtin_class(BuiltinClass::Array);
        let level = mutator
            .heap()
            .class_superclass(array)
            .expect("Array has a superclass");
        let metaclass = mutator
            .heap()
            .class_metaclass(level)
            .expect("a level has a metaclass");
        assert!(mutator.heap().class_is_applied_level(level));
        assert_eq!(mutator.heap().object_identity_hash(level), level_hash);
        assert_eq!(
            mutator.heap().object_identity_hash(metaclass),
            metaclass_hash
        );
    });
}

/// The closing table alone, decoded against a freshly loaded world and the
/// section of `FIXTURE_NAMES`.
fn decode_tag_named(bytes: &[u8]) -> Result<u32, decode::DecodeError> {
    let (mut interpreter, _class) = load(EMPTY);
    build_fixture_names(&mut interpreter);
    interpreter.enter(|mutator| {
        let shipped = shipped_names::resolve_in(mutator, &fixture_names(), false)
            .expect("the fixture's names resolve");
        let mut reader = decode::Reader::new(bytes);
        decode::decode_tag_named_hashes(mutator, &mut reader, &shipped)
    })
}

#[test]
fn a_tag_named_hash_of_zero_is_refused() {
    // 0 means unassigned, which the encoder expresses by leaving the entry
    // out; accepting it would give one world two encodings.
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(INTEGER_NAME);
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
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(INTEGER_NAME);
    writer.write_u32(3);
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(INTEGER_NAME);
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
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(INTEGER_NAME);
    writer.write_u32(1 << 22);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::IdentityHashTooWide(1 << 22))
    );
}

#[test]
fn an_unknown_tag_named_hash_tag_is_refused() {
    // One past the last tag-named hash, and every retired one: a stale
    // image's root-class entry names no module rather than module 0, and a
    // stale builtin, platform or platform path entry names nothing.
    for tag in [HASHED_SHIPPED + 1].into_iter().chain(RETIRED_HASHED_TAGS) {
        let mut writer = encode::Writer::new();
        writer.write_u32(1);
        writer.write_u8(tag);
        writer.write_u32(5);
        assert_eq!(
            decode_tag_named(&writer.into_bytes()),
            Err(decode::DecodeError::InvalidTag {
                what: "tag-named hash",
                tag
            })
        );
    }
}

/// A shipped entry's index past the section is refused before it indexes.
// spec: image.verify.ids
#[test]
fn a_tag_named_hash_past_the_shipped_names_is_refused() {
    let past = u32::try_from(FIXTURE_NAMES.len()).expect("a count fits a u32");
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(past);
    writer.write_u32(5);
    assert_eq!(
        decode_tag_named(&writer.into_bytes()),
        Err(decode::DecodeError::BadShippedName(past))
    );
}

/// A hash is no reference, so an entry may name any kind of shipped object,
/// the kernel's instance among them, and puts the hash on the loading
/// world's own object.
#[test]
fn a_tag_named_hash_may_name_a_module_instance() {
    let mut interpreter = Interpreter::bootstrap();
    build_fixture_names(&mut interpreter);
    interpreter.enter(|mutator| {
        let shipped = shipped_names::resolve_in(mutator, &fixture_names(), false)
            .expect("the fixture's names resolve");
        let kernel = shipped[KERNEL_INSTANCE_NAME as usize].object;
        let mut writer = encode::Writer::new();
        writer.write_u32(1);
        writer.write_u8(HASHED_SHIPPED);
        writer.write_u32(KERNEL_INSTANCE_NAME);
        writer.write_u32(7);
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        assert_eq!(
            decode::decode_tag_named_hashes(mutator, &mut reader, &shipped),
            Ok(7)
        );
        assert_eq!(mutator.heap().object_identity_hash(kernel), 7);
    });
}

// spec: image.verify.counts
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
    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(STRING_NAME);
    writer.write_u32(9);

    writer.write_u8(HASHED_SHIPPED);
    writer.write_u32(INTEGER_NAME);
    writer.write_u32(4);
    assert_eq!(decode_tag_named(&writer.into_bytes()), Ok(9));
}

// ---------------------------------------------------------------------------
// Loading compiles the whole mixin tree, so the code table must serialize
// every mixin's methods and initializer -- not only the root's -- and
// decoding must attach each of them to the declaration it came from.
// ---------------------------------------------------------------------------

const NESTED_MODULE: &str = indoc! {"
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
    mutator.heap_mut().allocate_class(
        class_of_class,
        NewClass {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
        },
    )
}

fn root_mixin_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Rc<Mixin> {
    heap.class_application(class)
        .expect("not a class")
        .mixin
        .clone()
}

/// The mixin-table rows of `class`'s module, as `image::snapshot` plans
/// them.
fn rows_of<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Vec<mixins::Row> {
    mixins::plan(heap, &[root_mixin_of(heap, class)]).expect("plan")
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
        let inner_mixin = decoded_mixins
            .module_root(0)
            .expect("a root row")
            .declared_nested()[0]
            .clone();
        let value_method = inner_mixin
            .compiled_method(Symbol::intern("value"))
            .expect("Inner declares `value`");
        let live_inner = &mutator
            .heap()
            .nested_mixins(&root_mixin_of(mutator.heap(), class))[0];
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
        let outer = mutator.fetch(&outer);
        snapshot_one(mutator, true, "t.ns", class, &[outer]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let inner_class = loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.modules[0].class);
        let root_mixin = root_mixin_of(mutator.heap(), root_class);
        let inner_mixin = mutator.heap().nested_mixins(&root_mixin)[0].clone();

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
        mutator.stash(Value::object(inner_class))
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
        .enter(|mutator| match mutator.fetch(&result).kind() {
            ValueKind::SmallInteger(9) => {}
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
        let outer = mutator.fetch(&outer);
        snapshot_one(mutator, true, "t.ns", class, &[outer]).expect("snapshot")
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
        let Some(_) = mutator.fetch(&inner_class).as_object() else {
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
        .enter(|mutator| match mutator.fetch(&result).kind() {
            ValueKind::SmallInteger(9) => {}
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
fn snapshot_world(interpreter: &mut Interpreter, class: &Root, roots: &[Root]) -> Vec<u8> {
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, class);
        let roots: Vec<Value> = roots.iter().map(|root| mutator.fetch(root)).collect();
        snapshot_one(mutator, true, "t.ns", class, &roots).expect("snapshot")
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

fn class_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> ClassReference<'gc> {
    heap.object_class(object(value))
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
    enclosing: Value<'gc>,
    name: &str,
) -> Value<'gc> {
    let class = class_of(heap, enclosing);
    let ns_runtime::dispatch::Lookup::NestedClass(nested, _) =
        ns_runtime::dispatch::lookup_defined_for(heap, class, Symbol::intern(name))
    else {
        panic!("{name} is a nested declaration of this object's class");
    };
    let application = application_of(heap, class);
    let id = heap
        .known_mixin_id(&nested)
        .expect("applying the enclosing mixin registered the nested one");
    let index = application.nested_slot[&id];
    heap.instance_field(object(enclosing), index)
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
        .enter(|mutator| match mutator.fetch(&class).kind() {
            ValueKind::Object(_) => {}
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
    let (mut interpreter, class, _platform, outer) = instantiated_world(NESTED_MODULE);
    let inner_class = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    interpreter.enter(|mutator| {
        let Some(_) = mutator.fetch(&inner_class).as_object() else {
            panic!("the accessor must yield a class object")
        };
    });
    let inner_instance = expect_ok(
        interpreter.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );

    // Reached as the cached value in Outer's hidden slot...
    let bytes = snapshot_world(&mut interpreter, &class, std::slice::from_ref(&outer));
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
        .enter(|mutator| match mutator.fetch(&value).kind() {
            ValueKind::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });

    // ... and as the `.class` of one of its instances.
    let bytes = snapshot_world(
        &mut interpreter,
        &class,
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
        let reloaded_class = class_of(mutator.heap(), reloaded_instance_value);
        assert_eq!(class_name(mutator.heap(), reloaded_class), "Inner");
    });
    let value = send_to(&mut loaded, &reloaded_instance, "value");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value).kind() {
            ValueKind::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

#[test]
fn a_second_user_class_no_longer_decodes_as_the_root() {
    // A class tag that named no particular class would bring a second
    // user-defined class object back as the root: a wrong class silently
    // installed, with no field count to contradict it, rather than a decode
    // error.
    let (mut interpreter, class, _platform, outer) = instantiated_world(NESTED_MODULE);
    let _ = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    let bytes = snapshot_world(&mut interpreter, &class, std::slice::from_ref(&outer));
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_outer = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Outer");
    let inner = nested_class_of(&mut loaded, &reloaded_outer, "Inner");
    // Both read out after the accessor ran, which may have moved them.
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.modules[0].class);
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
        interpreter.enter(|mutator| match mutator.fetch(&class).kind() {
            ValueKind::Object(_) => {}
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

    let bytes = snapshot_world(&mut interpreter, &class, &[first, second]);
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
            first_inner.same(object(cached_nested_class(
                mutator.heap(),
                outer_values[0],
                "Inner"
            ))),
            "the accessor answered out of the decoded cache slot"
        );
        assert!(
            second_inner.same(object(cached_nested_class(
                mutator.heap(),
                outer_values[1],
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

/// `dispatch::kth_enclosing_object` steps a class's declaring class, not
/// the reached object's own class, from its second hop on -- so an image
/// that dropped that field would come back resolving `outer` along the
/// receiver's *dynamic* chain. `Mid`'s decoy `label` is what such a walk
/// answers with, and a walk that finds no declaring class at all answers
/// `nil label`, an error. Both are wrong output rather than a refused
/// image, which is why the field has to travel.
const DEEP_OUTER_MODULE: &str = indoc! {"
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
    let (mut interpreter, class, _platform, outer) = instantiated_world(DEEP_OUTER_MODULE);
    let deep = expect_ok(
        interpreter.send(&outer, Symbol::intern("makeDeep"), &[]),
        "makeDeep",
    );
    let bytes = snapshot_world(&mut interpreter, &class, &[outer, deep]);
    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_deep = loaded.roots[1].clone();
    // `Deep`'s declaring class is `Base`, not `Derived` -- the two differ
    // exactly because `Derived` inherits the declaration across a nesting
    // boundary, which is the case a dynamic-chain walk gets wrong.
    loaded.interpreter.enter(|mutator| {
        let reloaded_deep_value = mutator.fetch(&reloaded_deep);
        let reloaded_class = class_of(mutator.heap(), reloaded_deep_value);
        assert_eq!(
            class_name(mutator.heap(), reloaded_class),
            "Deep",
            "the reloaded instance keeps its own nested class"
        );
    });
    let described = send_to(&mut loaded, &reloaded_deep, "describe");
    loaded.interpreter.enter(|mutator| {
        let described = mutator.fetch(&described);
        assert_eq!(string_contents(mutator.heap(), described), "top");
    });
}

/// The text of a decoded `String` object. Never `{:?}`s the `Value`.
fn string_contents<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> String {
    heap.str_value(object(value))
        .expect("expected a string object")
}

const NESTED_SUPERCLASS_ARGUMENTS: &str = indoc! {"
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
/// proves the arguments made it across.
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
        let outer = mutator.fetch(&outer);
        snapshot_one(mutator, true, "t.ns", class, &[outer]).expect("snapshot")
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
        let Some(_) = mutator.fetch(&derived).as_object() else {
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
        .enter(|mutator| match mutator.fetch(&result).kind() {
            ValueKind::SmallInteger(7) => {}
            _ => {
                panic!("expected SmallInteger(7) -- the clause's own argument -- from Base's slot")
            }
        });
}

/// The clause's *receiver* side of the same shape: a mixin-table row that
/// declares a superclass clause, in an image carrying no compiled code for
/// it, would panic when the first nested-class access pushes the clause's
/// activation (`nested::superclass_clause_code`). `verify_mixins` is the only
/// place that sees both halves, so it refuses.
// spec: image.verify.initializers
#[test]
fn an_image_with_no_superclass_clause_for_a_clause_that_needs_one_is_refused() {
    let (mut interpreter, class) = load(NESTED_SUPERCLASS_ARGUMENTS);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let derived = mutator
            .heap()
            .nested_mixins(&root_mixin_of(mutator.heap(), class))
            .into_iter()
            .find(|nested| nested.name.as_str() == "Derived")
            .expect("Args declares Derived");
        assert!(
            derived.superclass_clause_code.is_some(),
            "Derived's clause really does compile a clause body"
        );
        let doctored = with_nested(mutator.heap(), class, "Derived", |nested| Mixin {
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

/// The superclass clause frame kind shares `Initializer`'s shape on the wire
/// (a class tag) but not its tag, which is what lets `image::decode`'s
/// `frame_body` resolve it against the code table its own body actually lives
/// in. Nothing leaves such a frame reachable from an image's roots -- a clause
/// body contains no `MakeClosure`, so nothing can capture one -- so only a
/// hand-written section reaches the decoder's tag-4 branch.
// spec: image.graph.record
#[test]
fn the_superclass_clause_frame_kind_decodes() {
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_SUPERCLASS_CLAUSE);
    writer.write_shipped(OBJECT_NAME);
    runs_body(&mut writer, 0);
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
    let bytes = writer.into_bytes();
    let mut fixture = GraphFixture::new();
    fixture.codes = vec![code_with(
        CodeKind::SuperclassClause,
        0,
        1,
        vec![Instruction::Return { register: 0 }],
    )];
    fixture.decoded(&bytes, "a superclass clause frame", |heap, decoded| {
        let frame = decoded.frames.first().expect("the one frame");
        assert!(matches!(
            heap.frame_kind(*frame),
            FrameKind::SuperclassClause(_)
        ));
    });
}

// ---------------------------------------------------------------------------
// The image symbol table: every selector and name written once, referenced
// everywhere else by index.
// ---------------------------------------------------------------------------

const TRIVIAL_MODULE: &str = indoc! {"
    class X = Object new (
    ) (
      foo = ( ^1 )
    ) : ()
"};

/// A full `image::snapshot` of `source`, with no roots -- these tests care
/// about the code table's own bytes, not the object graph.
fn snapshot_of(source: &str) -> Vec<u8> {
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[]).expect("snapshot")
    })
}

// ---------------------------------------------------------------------------
// The module section: several modules in one image, each named by its index.
// ---------------------------------------------------------------------------

/// A module's root class is a record of the graph like any class: the
/// module section names it by object id, and a load answers that record.
// spec: image.modules
#[test]
fn a_modules_root_class_is_a_record_the_module_section_names() {
    let (mut interpreter, class) = load(EMPTY);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .graph_objects
            .iter()
            .any(|object| object.contains("class mixin row 0")),
        "the root class is a class record: {:?}",
        report.graph_objects
    );
    let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.modules.len(), 1);
}

/// Loads a further module into `interpreter`, resolved as coming from `name`.
fn load_another(interpreter: &mut Interpreter, source: &str, name: &str) -> Root {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from(name)),
    )
    .expect("resolve");
    interpreter.load_module(&resolved)
}

/// Whether two values are one object.
fn same_value<'gc>(first: Value<'gc>, second: Value<'gc>) -> bool {
    object(first).same(object(second))
}

const FIRST_MODULE: &str = indoc! {"
    class First = Object new (| public held ::= nil. public made ::= nil. |) () : ()
"};

const SECOND_MODULE: &str = indoc! {"
    class Second = Object new () (
        public answer = ( ^2 )
        public maker = ( ^[42] )
    ) : ()
"};

/// `First` loaded, then `Second`, and an instance of `First`.
fn two_module_world() -> (Interpreter, Root, Root, Root) {
    let (mut interpreter, first) = load(FIRST_MODULE);
    let second = load_another(&mut interpreter, SECOND_MODULE, "Second.ns");
    let instance = expect_ok(interpreter.instantiate(&first, &[]), "First new");
    (interpreter, first, second, instance)
}

/// A snapshot of `instance` carrying `First` and `Second`, in that order.
fn two_module_image(
    interpreter: &mut Interpreter,
    first: &Root,
    second: &Root,
    instance: &Root,
) -> Vec<u8> {
    let first_identity = ModuleIdentity::undeclared("First.ns", "First");
    let second_identity = ModuleIdentity::undeclared("Second.ns", "Second");
    interpreter
        .enter(|mutator| {
            let modules = [
                image::ModuleSource {
                    class: class_reference(mutator, first),
                    identity: &first_identity,
                    with_source: false,
                },
                image::ModuleSource {
                    class: class_reference(mutator, second),
                    identity: &second_identity,
                    with_source: false,
                },
            ];
            image::snapshot(mutator, &modules, &[mutator.fetch(instance)])
        })
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Where the module section starts: its `u32` module count.
fn module_section_start(bytes: &[u8]) -> usize {
    image::inspect::inspect(bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|section| section.name == "modules")
        .expect("a module section")
        .start
}

#[test]
fn an_image_of_two_modules_restores_both_and_the_graph_reaching_them() {
    // First's instance holds Second's class and an instance of it; after a
    // load, both module classes are rebuilt, and the instance answers its
    // own class's method.
    let (mut interpreter, first, second, instance) = two_module_world();
    let second_instance = expect_ok(interpreter.instantiate(&second, &[]), "Second new");
    expect_ok(
        interpreter.send(
            &instance,
            Symbol::intern("held:"),
            std::slice::from_ref(&second),
        ),
        "held:",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("made:"), &[second_instance]),
        "made:",
    );
    let bytes = two_module_image(&mut interpreter, &first, &second, &instance);
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.modules.len(), 2);
    assert_eq!(
        *loaded.modules[1].identity,
        ModuleIdentity::undeclared("Second.ns", "Second")
    );
    let root = loaded.roots[0].clone();
    let made = send_to(&mut loaded, &root, "made");
    let answer = send_to(&mut loaded, &made, "answer");
    let held = send_to(&mut loaded, &root, "held");
    let module_class = loaded.modules[1].class.clone();
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&answer).as_small_integer() == Some(2));
        assert!(same_value(
            mutator.fetch(&held),
            mutator.fetch(&module_class)
        ));
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report
            .sections
            .iter()
            .find(|section| section.name == "modules")
            .map(|section| section.detail.as_str()),
        Some("2 modules")
    );
    let second_root = module_root_object(&bytes, 1);
    assert!(
        report.graph_objects[second_root].contains("class mixin row"),
        "the module section names Second's root class record: {:?}",
        report.graph_objects
    );
}

/// Where the graph section starts, as `inspect` attributes it.
fn graph_section_start(bytes: &[u8]) -> usize {
    image::inspect::inspect(bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|section| section.name == "graph")
        .expect("a graph section")
        .start
}

/// Rewrites the one occurrence of `pattern` in the graph section to
/// `replacement`.
fn rewrite_in_graph(bytes: &mut [u8], pattern: &[u8], replacement: &[u8]) {
    let start = graph_section_start(bytes);
    let occurrences: Vec<usize> = bytes[start..]
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, window)| *window == pattern)
        .map(|(offset, _)| start + offset)
        .collect();
    assert_eq!(
        occurrences.len(),
        1,
        "the pattern must occur exactly once in the graph section"
    );
    let position = occurrences[0];
    bytes[position..position + replacement.len()].copy_from_slice(replacement);
}

/// The object id the module section names `module`'s root class record by,
/// as `inspect` reports it (`root object {id}`).
fn module_root_object(bytes: &[u8], module: usize) -> usize {
    let report = image::inspect::inspect(bytes).expect("inspect");
    report.modules[module]
        .root
        .strip_prefix("root object ")
        .and_then(|id| id.parse().ok())
        .expect("a module line names its root object")
}

/// Where the `u32` object id of `module`'s root class record sits in the
/// module section.
fn module_root_id_at(bytes: &[u8], module: usize) -> usize {
    let mut reader = decode::Reader::new(bytes);
    reader
        .read_raw(module_section_start(bytes))
        .expect("the sections before");
    reader.read_u32().expect("module count");
    for _ in 0..module {
        skip_module_identity(&mut reader);
        reader.read_u32().expect("an earlier module's root");
    }
    skip_module_identity(&mut reader);
    reader.position()
}

/// Reads past a module record's identity: its origin and path, its id, and
/// its version.
fn skip_module_identity(reader: &mut decode::Reader) {
    reader.read_u8().expect("the origin tag");
    reader.read_str().expect("the path");
    reader.read_str().expect("the id");
    if reader.read_u8().expect("the version tag") == MODULE_VERSIONED {
        reader.read_str().expect("the version");
    }
}

/// `EMPTY` carried with one root, an empty array: its graph holds the
/// array, the root class's record and its metaclass's, and its shipped
/// names `Kernel Array` and `Kernel Object` both.
fn empty_module_with_an_array() -> Vec<u8> {
    let (mut interpreter, class) = load(EMPTY);
    interpreter.enter(|mutator| {
        let array = Value::object(mutator.new_array(Vec::new()));
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[array]).expect("snapshot")
    })
}

/// A module's root is named by an id into the object table, which is
/// checked against the decoded graph like every other id.
// spec: image.verify.ids
#[test]
fn a_module_root_id_past_the_graph_is_refused() {
    let mut bytes = empty_module_with_an_array();
    let root_id_at = module_root_id_at(&bytes, 0);
    bytes[root_id_at..root_id_at + 4].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::BadObjectId(999).to_string()
    );
}

/// The record a module section names must be a class of the module's own
/// root declaration: an array is no class at all.
// spec: image.verify.modules
#[test]
fn a_module_root_naming_an_array_is_refused() {
    let mut bytes = empty_module_with_an_array();
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let array = report
        .graph_objects
        .iter()
        .position(|record| record.contains("array []"))
        .expect("the array's record");
    let root_id_at = module_root_id_at(&bytes, 0);
    bytes[root_id_at..root_id_at + 4]
        .copy_from_slice(&u32::try_from(array).expect("an id").to_le_bytes());
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::ModuleRootNotItsRecord { module: 0 }.to_string()
    );
}

/// A root class record over `Kernel Array` applies its module's root row,
/// but the root rule builds no root over anything but `Object` or `Value`:
/// refused, though `Kernel Array` may stand as a superclass elsewhere.
// spec: image.verify.root-rule
#[test]
fn a_root_record_over_kernel_array_breaks_the_root_rule() {
    let mut bytes = empty_module_with_an_array();
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let index_of = |name: &str| {
        u32::try_from(
            report
                .shipped_names
                .iter()
                .position(|shipped| shipped == name)
                .expect("the name is in the section"),
        )
        .expect("an index")
    };
    let row = mixin_table_of(&bytes).module_rows[0];
    let superclass = |index: u32| {
        let mut record = vec![OBJECT_CLASS, MIXIN_ROW];
        record.extend(row.to_le_bytes());
        record.push(1);
        record.push(CLASS_SHIPPED);
        record.extend(index.to_le_bytes());
        record
    };
    rewrite_in_graph(
        &mut bytes,
        &superclass(index_of("Kernel Object")),
        &superclass(index_of("Kernel Array")),
    );
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::RootRule { module: 0 }.to_string()
    );
}

/// The two refusals of a module's root record, pinned by variant.
#[test]
fn the_refusals_of_a_module_root_record_name_their_module() {
    assert_eq!(
        decode::DecodeError::ModuleRootNotItsRecord { module: 2 }.to_string(),
        "the image names a record as the root class of its module 2 that is no class of that \
         module's root declaration"
    );
    assert_eq!(
        decode::DecodeError::RootRule { module: 2 }.to_string(),
        "the root class of the image's module 2 inherits neither Object nor Value, nor Value \
         applied over Object, as a module's root class must"
    );
}

/// The encoder holds a root to the same rule, so it never writes a root the
/// loader refuses.
#[test]
fn a_root_class_over_kernel_array_is_not_written() {
    let (mut interpreter, class) = load(EMPTY);
    let error = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let array = mutator.builtin_class(BuiltinClass::Array);
        mutator.heap_mut().class_set_superclass(class, array);
        match module_only_image(mutator, false, "t.ns", class) {
            Ok(_) => panic!("a root over Kernel Array must not be written"),
            Err(error) => error,
        }
    });
    assert!(
        error.contains("does not inherit what the root rule allows"),
        "unexpected error: {error}"
    );
}

/// A module root with a declaring class is not its module's own root
/// record, and the encoder's refusal says so rather than calling it a
/// metaclass or a level.
#[test]
fn a_root_class_with_a_declaring_class_is_not_written() {
    let (mut interpreter, class) = load(EMPTY);
    let error = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let array = mutator.builtin_class(BuiltinClass::Array);
        mutator
            .heap_mut()
            .class_set_lexical_context(class, None, Some(array));
        match module_only_image(mutator, false, "t.ns", class) {
            Ok(_) => panic!("a root with a declaring class must not be written"),
            Err(error) => error,
        }
    });
    assert!(
        error.contains(
            "a metaclass, a level, a class applying another declaration's mixin, or a class \
             with an enclosing object or a declaring class"
        ),
        "unexpected error: {error}"
    );
}

// spec: image.verify.mixin-tree
#[test]
fn a_module_count_that_disagrees_with_the_trees_is_refused() {
    let (mut interpreter, first, second, instance) = two_module_world();
    let mut bytes = two_module_image(&mut interpreter, &first, &second, &instance);
    let count_at = module_section_start(&bytes);
    bytes[count_at..count_at + 4].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::ModuleCountMismatch {
            declared: 1,
            trees: 2
        }
        .to_string()
    );
}

// spec: image.verify.counts
#[test]
fn a_module_count_past_the_bytes_left_is_refused() {
    let (mut interpreter, first, second, instance) = two_module_world();
    let mut bytes = two_module_image(&mut interpreter, &first, &second, &instance);
    let count_at = module_section_start(&bytes);
    bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let error = load_error(&bytes);
    assert!(
        error == decode::DecodeError::UnexpectedEndOfFile.to_string()
            || error
                == decode::DecodeError::ModuleCountMismatch {
                    declared: u32::MAX,
                    trees: 2
                }
                .to_string(),
        "unexpected error: {error}"
    );
    // The bound itself, with the count agreeing with the trees: a count the
    // bytes left cannot hold is refused before anything is reserved for it.
    let decoded = mixin_table_of(&bytes);
    let mut section = encode::Writer::new();
    section.write_u32(2);
    let section = section.into_bytes();
    assert!(matches!(
        image::modules::decode_modules(
            &mut decode::Reader::new(&section),
            &[0, 1].map(|module| decoded.module_root(module).expect("a root").name),
        ),
        Err(decode::DecodeError::UnexpectedEndOfFile)
    ));
}

/// Each module's root record is held to its own module's root row: module
/// 1's id naming module 0's root class record -- a class record that obeys
/// the root rule, of another declaration -- is refused for module 1.
// spec: image.verify.modules
#[test]
fn a_module_root_is_held_to_its_own_modules_row() {
    let (mut interpreter, first) = load(indoc! {"
        class First = Object new <: Value () () : ()
    "});
    let second = load_another(&mut interpreter, SECOND_MODULE, "Second.ns");
    let first_identity = ModuleIdentity::undeclared("First.ns", "First");
    let second_identity = ModuleIdentity::undeclared("Second.ns", "Second");
    let mut bytes = interpreter
        .enter(|mutator| {
            let modules = [
                image::ModuleSource {
                    class: class_reference(mutator, &first),
                    identity: &first_identity,
                    with_source: false,
                },
                image::ModuleSource {
                    class: class_reference(mutator, &second),
                    identity: &second_identity,
                    with_source: false,
                },
            ];
            image::snapshot(mutator, &modules, &[])
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(image::load(&bytes).is_ok(), "the fixture itself must load");
    let first_root = u32::try_from(module_root_object(&bytes, 0)).expect("an id");
    let root_id_at = module_root_id_at(&bytes, 1);
    bytes[root_id_at..root_id_at + 4].copy_from_slice(&first_root.to_le_bytes());
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::ModuleRootNotItsRecord { module: 1 }.to_string()
    );
}

#[test]
fn a_graph_reaching_a_module_the_image_does_not_carry_is_refused() {
    let (mut interpreter, first, second, instance) = two_module_world();
    expect_ok(
        interpreter.send(&instance, Symbol::intern("held:"), &[second]),
        "held:",
    );
    let result = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &first);
        snapshot_one(
            mutator,
            false,
            "First.ns",
            class,
            &[mutator.fetch(&instance)],
        )
    });
    match result {
        Ok(_) => panic!("a graph reaching a module the image does not carry must be refused"),
        Err(error) => {
            assert!(error.contains("`Second`"), "unexpected error: {error}");
            assert!(
                error.contains("not part of any module the image carries"),
                "unexpected error: {error}"
            );
        }
    }
}

#[test]
fn a_closure_over_code_no_module_carries_is_refused_not_a_panic() {
    let (mut interpreter, first, second, instance) = two_module_world();
    let maker = expect_ok(interpreter.instantiate(&second, &[]), "Second new");
    let block = expect_ok(
        interpreter.send(&maker, Symbol::intern("maker"), &[]),
        "maker",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("held:"), &[block]),
        "held:",
    );
    let result = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &first);
        snapshot_one(
            mutator,
            false,
            "First.ns",
            class,
            &[mutator.fetch(&instance)],
        )
    });
    match result {
        Ok(_) => panic!("a closure over code no carried module compiled must be refused"),
        Err(error) => assert!(
            error.contains("module the image carries"),
            "unexpected error: {error}"
        ),
    }
}

/// A level a module root's `<:` clause added is a record, so its own
/// identity hash travels in its record's header, as its root's and both
/// metaclasses' do. The kernel gives a program no `superclass` accessor,
/// so the hash is drawn from Rust, where a later reflective reach would
/// draw it.
#[test]
fn a_module_levels_own_hash_survives_an_image() {
    let source = indoc! {"
        class M = Object new <: Value () () : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "M new");
    let (bytes, hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let level = mutator
            .heap()
            .class_superclass(class)
            .expect("the level `<: Value` added");
        let hash = mutator.heap_mut().object_ensure_identity_hash(level);
        let bytes = snapshot_one(mutator, false, "t.ns", class, &[mutator.fetch(&instance)])
            .expect("snapshot");
        (bytes, hash)
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report.graph_objects.iter().any(
            |record| record.contains(&format!("hash {hash}, class mixin shipped Kernel Value"))
        ),
        "the level's record carries its hash: {:?}",
        report.graph_objects
    );
    let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let module_class = loaded.modules[0].class.clone();
    let mut interpreter = loaded.interpreter;
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let level = heap
            .class_superclass(class_reference(mutator, &module_class))
            .expect("the loaded level");
        assert_eq!(heap.object_identity_hash(level), hash);
    });
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

/// The image's bytes but the header and the text table -- whose
/// declaration texts necessarily still contain every selector as plain
/// source. The dedup property applies to every other section: everything
/// that would otherwise be an inline `write_str` of a selector or name lives
/// in one of them.
fn bytes_outside_the_text_table(bytes: &[u8]) -> Vec<u8> {
    let report = image::inspect::inspect(bytes).expect("inspect");
    report
        .sections
        .iter()
        .filter(|section| section.name != "header" && section.name != "texts")
        .flat_map(|section| bytes[section.start..section.end].iter().copied())
        .collect()
}

#[test]
fn a_repeated_selector_is_stored_once() {
    // `distinctiveSelector` is deliberately long and specific: a single
    // byte such as `b"b"` recurs throughout any binary stream by chance,
    // which would make `occurrences` meaningless.
    // This selector is the method-dictionary key for one method and the
    // send target at three call sites -- four inline occurrences without
    // deduplication, one symbol-table entry with it. (The declaration texts,
    // stored in the text table, still contain the selector as plain text --
    // `bytes_outside_the_text_table` excludes that section, which the symbol
    // table does not cover.)
    let bytes = snapshot_of(indoc! {"
        class C = Object new (
        ) (
          a = ( ^self distinctiveSelector )
          b = ( ^self distinctiveSelector )
          distinctiveSelector = ( ^self distinctiveSelector )
        ) : ()
    "});
    assert_eq!(
        occurrences(
            &bytes_outside_the_text_table(&bytes),
            b"distinctiveSelector"
        ),
        1
    );
}

/// Overwrites the symbol table's own entry count to claim zero symbols.
/// `TRIVIAL_MODULE` compiles exactly one method, so its mixin section holds
/// one real reference to symbol id 0 -- which does not exist once the
/// table it indexes into claims to be empty. Located via the real decode
/// function (the header's) rather than a hardcoded byte offset, so this
/// does not drift if the header's encoding changes shape.
fn corrupt_first_symbol_id(bytes: &mut [u8]) {
    let count_at = {
        let mut reader = decode::Reader::new(bytes);
        decode::read_header(&mut reader).expect("valid header");
        reader.position()
    };
    bytes[count_at..count_at + 4].copy_from_slice(&0u32.to_le_bytes());
}

// spec: bytecode.verify.tags
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
// spec: image.verify.ids
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
    assert_eq!(getter.fast_path(), None);
}

/// Two lazy slots of one name would share the one getter the row carries
/// for that name, which serves only one of their fields and leaves the other
/// dead: the row is refused as declaring that member twice, as two eager
/// slots of one name are.
// spec: image.verify.mixin-rows
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
// spec: image.verify.mixin-rows
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
// spec: image.verify.mixin-rows
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
// spec: image.verify.mixin-rows
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

// spec: image.verify.mixin-rows
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
// spec: image.verify.mixin-rows
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

/// `write_mixin_row_with_method_shapes`' row of no slot and no nested
/// declaration, naming row `class_side` as its class side.
fn mixin_row_with_class_side(name_id: u32, methods: &[MethodShape], class_side: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    write_mixin_row_with_method_shapes(&mut writer, name_id, &[], methods, &[]);
    let mut row = writer.into_bytes();
    // The row ends with the class side's presence and the four bodies'.
    let presence = row.len() - 5;
    assert_eq!(row[presence], 0, "the helper's row has no class side");
    row[presence] = 1;
    row.splice(presence + 1..presence + 1, class_side.to_le_bytes());
    row
}

/// §6.3: a class side's mixin holds its declaration's factory, so a row
/// that is a declaration's class side and declares a method of the
/// factory's selector -- `new`, for a declaration that names none --
/// declares that member twice. A method of another selector decodes.
// spec: image.verify.mixin-rows
#[test]
fn a_class_side_method_named_like_the_factory_is_refused() {
    let decoded = |selector: u32| {
        let mut writer = encode::Writer::new();
        writer.write_u32(2);
        writer.write_raw(&mixin_row_with_class_side(0, &[], 1));
        write_mixin_row_with_method_shapes(
            &mut writer,
            1,
            &[],
            &[MethodShape {
                selector,
                access: AccessModifier::Public,
                argument_count: 0,
                code: 0,
            }],
            &[],
        );
        let bytes = writer.into_bytes();
        let mut reader = decode::Reader::new(&bytes);
        let codes = [Rc::new(every_instruction_once())];
        mixins::decode(
            &mut reader,
            &table_with(&["X", "X class", "new", "other"]),
            &codes,
        )
    };
    assert!(
        decoded(3).is_ok(),
        "a class side's method of another selector"
    );
    assert_eq!(
        decoded(2).err(),
        Some(decode::DecodeError::DuplicateSelector("new".to_string()))
    );
}

/// The opening fields of a hand-crafted row: its name, symbol 0, its
/// access, and its source range from `start` to `end`.
fn row_opening(writer: &mut encode::Writer, start: u32, end: u32) {
    writer.write_u32(1); // one row
    writer.write_u32(0); // name
    writer.write_u8(mixins::access_tag(AccessModifier::Protected));
    writer.write_u32(start); // span.start
    writer.write_u32(end); // span.end
}

/// A row's source range is a span like a body's, which ends no earlier
/// than it starts.
// spec: image.verify.mixin-rows
#[test]
fn a_mixin_row_whose_range_ends_before_it_starts_is_refused() {
    let mut writer = encode::Writer::new();
    row_opening(&mut writer, 9, 4);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::InvalidSpan { start: 9, end: 4 })
    );
}

/// A factory takes at most 255 parameters, as a method does: a row whose
/// factory takes 256 is refused where the arity is read.
// spec: image.verify.mixin-rows
#[test]
fn a_factory_of_256_parameters_is_refused() {
    let mut writer = encode::Writer::new();
    row_opening(&mut writer, 0, 0);
    writer.write_u32(0); // no slots
    writer.write_boolean(true); // a factory ...
    writer.write_u32(1); // ... `with:` ...
    writer.write_u16(256); // ... of 256 parameters
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X", "with:"]), &[]).err(),
        Some(decode::DecodeError::ArgumentCountTooLarge {
            what: "factory",
            count: 256
        })
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

// spec: image.symbols
#[test]
fn a_symbol_written_at_many_call_sites_still_resolves_to_the_same_symbol_everywhere() {
    // The dedup property from the caller's side: every occurrence of
    // `distinctiveSelector` -- the method itself and every send of it --
    // must still resolve to the same interned `Symbol` after a load, not
    // merely compress on the wire.
    let bytes = snapshot_of(indoc! {"
        class C = Object new (
        ) (
          public a = ( ^self distinctiveSelector )
          distinctiveSelector = ( ^42 )
        ) : ()
    "});
    let mut loaded = image::load(&bytes).expect("load");
    let instance = expect_ok(
        loaded
            .interpreter
            .instantiate(&loaded.modules[0].class, &[]),
        "instantiate",
    );
    let result = expect_ok(
        loaded.interpreter.send(&instance, Symbol::intern("a"), &[]),
        "a",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&result).as_small_integer() == Some(42));
    });
}

// ---------------------------------------------------------------------------
// The mixin table. What the image says a declaration *is* -- its
// name, slots, methods, factory, superclass clause, nesting and class side --
// comes back from the table, never by re-parsing the image's own source text.
// ---------------------------------------------------------------------------

const TABLE_NESTED_SOURCE: &str = indoc! {"
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
    let header = decode::read_header(&mut reader).expect("valid header");
    let symbols = symbols::decode(&mut reader).expect("symbol table");
    let codes = decode::decode_code_table(&mut reader, &symbols).expect("code table");
    let texts = if header.has_source {
        image::texts::decode(&mut reader).expect("text table")
    } else {
        Vec::new()
    };
    let read =
        mixins::read(&mut reader, &symbols, &codes.codes, header.has_source).expect("mixin table");
    mixins::build(read, &codes.codes, &texts, &[]).expect("mixin table")
}

/// An image whose mixin table is `mixin_table` verbatim: a real header, a
/// real symbol table holding `strings`, and an empty code table. Everything before the
/// mixin table comes from the real encoders, so a fixture built this way
/// exercises the mixin table's own validation rather than an accident in an
/// earlier section. Nothing follows the mixin table, so every test using
/// this asserts an error -- the point is only ever *which* error, and that
/// it is an error rather than an abort.
fn image_with_mixin_table(strings: &[&str], mixin_table: &[u8]) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    encode::write_header(&mut writer, 0);
    let mut builder = symbols::Builder::new();
    for string in strings {
        builder.intern(string);
    }
    builder.write(&mut writer);
    writer.write_u32(0); // an empty code table
    writer.write_raw(mixin_table);
    writer.into_bytes()
}

// spec: image.mixins
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
    let nested = root.declared_nested();
    assert_eq!(nested.len(), 1);
    let inner = &nested[0];
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
// spec: image.untrusted
// spec: image.verify.mixin-tree
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
// spec: image.verify.mixin-tree
#[test]
fn a_mixin_tree_just_inside_the_nesting_ceiling_still_decodes() {
    let bytes = mixin_chain(128);
    let mut reader = decode::Reader::new(&bytes);
    let decoded = mixins::decode(&mut reader, &table_with(&["X"]), &[]).expect("128 levels decode");
    assert_eq!(decoded.mixins.len(), 128);
    let mut level = decoded.module_root(0).expect("a root row").clone();
    let mut depth = 1;
    loop {
        let Some(next) = level.declared_nested().first().cloned() else {
            break;
        };
        level = next;
        depth += 1;
    }
    assert_eq!(depth, 128, "every level is linked to its child");
}

// spec: image.verify.ids
// spec: image.verify.mixin-tree
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
// spec: image.verify.mixin-tree
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

/// A row's nested declarations lie in its own tree: a row of the second
/// tree nesting the first tree's root would give that root a parent in
/// another module.
// spec: image.verify.mixin-tree
#[test]
fn a_row_nesting_a_row_of_an_earlier_tree_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(2);
    write_mixin_row(&mut writer, 0, &[], &[]);
    write_mixin_row(&mut writer, 0, &[], &[0]); // row 1 nests the first tree's root
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut reader, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::MixinTableNotATree { row: 0 })
    );
}

/// The ceiling itself: a tree of 130 rows, one nesting the next, decodes,
/// and one of 131 is refused.
// spec: image.verify.mixin-tree
#[test]
fn a_mixin_tree_nested_131_rows_deep_is_refused() {
    let decoded = |depth: u32| {
        let bytes = mixin_chain(depth);
        let mut reader = decode::Reader::new(&bytes);
        mixins::decode(&mut reader, &table_with(&["X"]), &[])
    };
    assert!(decoded(130).is_ok(), "130 rows decode");
    assert_eq!(
        decoded(131).err(),
        Some(decode::DecodeError::NestingTooDeep {
            what: "mixin nesting",
            limit: 130
        })
    );
}

/// A row no earlier row reaches starts a tree of its own: the table then
/// describes two modules while the module section declares one, and `load` refuses
/// the mismatch rather than dropping or misplacing the second tree.
// spec: image.verify.mixin-tree
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
    let mut bytes = image_with_mixin_table(&["X"], &table);
    bytes.extend_from_slice(&1u32.to_le_bytes()); // the module section's count
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
        class First = Object new () (
            class Inner = Object new () ( public answer = ( ^1 ) ) : ()
        ) : ()
    "});
    let second_source = indoc! {"
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
            root_mixin_of(mutator.heap(), class_reference(mutator, &first)),
            root_mixin_of(mutator.heap(), class_reference(mutator, &second)),
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
        let ids = encode::encode_code_table(&mut code, &rows, &[], &mut symbols)
            .expect("encode code table");
        let mut writer = encode::Writer::new();
        mixins::encode(&mut writer, &rows, &mut symbols, &ids, None).expect("encode mixin table");
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
/// `Mixin::declaration`: `instantiate` checks the selector
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
        let outer = mutator.fetch(&outer);
        snapshot_one(mutator, true, "t.ns", class, &[outer]).expect("snapshot")
    });

    let decoded = mixin_table_of(&bytes);
    let root = decoded.module_root(0).expect("a root row").clone();
    let nested = root.declared_nested();
    let base = nested
        .iter()
        .find(|nested| nested.name == Symbol::intern("Base"))
        .expect("Base is a nested declaration");
    let derived = nested
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
        let Some(_) = mutator.fetch(&derived_class).as_object() else {
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
        assert!(mutator.fetch(&result).as_small_integer() == Some(7));
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
    panic!(
        "the parser accepted {PROBE_CEILING} levels of class nesting: it has no ceiling to derive the image's from"
    );
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
    loop {
        let Some(next) = level.declared_nested().first().cloned() else {
            break;
        };
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
        mixin = Rc::new(
            Mixin::empty(Symbol::intern(&format!("C{level}"))).with_declared_nested(vec![mixin]),
        );
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
    let error = interpreter.enter(|mutator| {
        let application = mutator.apply_mixin(&mixin, None);
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let class: ClassReference = mutator.heap_mut().allocate_class(
            object_class,
            NewClass {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: None,
                builtin: None,
            },
        );
        let Err(error) = snapshot_one(mutator, true, "t.ns", class, &[]) else {
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
fn superclass_depth<'gc>(heap: &Heap<'gc>, head: Value<'gc>) -> u32 {
    let mut depth = 0;
    let mut level = match head.kind() {
        ValueKind::Object(class) => Some(class),
        _ => None,
    };
    while let Some(class) = level {
        depth += 1;
        level = heap.class_superclass(class);
    }
    depth
}

/// The same count down an instance chain's first field.
fn instance_chain_depth<'gc>(heap: &Heap<'gc>, head: Value<'gc>) -> u32 {
    let mut depth = 0;
    let mut level = match head.kind() {
        ValueKind::Object(object) => Some(object),
        _ => None,
    };
    while let Some(object) = level {
        depth += 1;
        level = match heap.instance_field(object, 0) {
            Some(next) => next.as_object(),
            None => None,
        };
    }
    depth
}

/// A complete image whose graph section is `graph` verbatim and whose
/// module section names record `root` as `EMPTY`'s root class. Everything
/// before the graph comes from a real `snapshot` of `EMPTY`, its
/// shipped-name section replaced by `FIXTURE_NAMES`'s, so `graph` may use
/// the fixture's indices. The splice point is found by structure, where
/// `inspect` says the graph section starts, not by a hardcoded offset.
fn image_with_graph_section(graph: &[u8], root: u32) -> Vec<u8> {
    image_with_graph_section_of(EMPTY, graph, root)
}

/// `image_with_graph_section` for a module of `source`'s one declaration,
/// so that the graph's root class has its methods.
fn image_with_graph_section_of(source: &str, graph: &[u8], root: u32) -> Vec<u8> {
    let mut bytes = with_fixture_names(&snapshot_of(source));
    let root_id_at = module_root_id_at(&bytes, 0);
    bytes[root_id_at..root_id_at + 4].copy_from_slice(&root.to_le_bytes());
    bytes.truncate(graph_section_start(&bytes));
    bytes.extend_from_slice(graph);
    bytes
}

/// `EMPTY`'s root class as records `id` and `id + 1`: a class record of
/// row 0 over `Kernel Object` and its metaclass record -- what a graph
/// spliced into `EMPTY`'s image needs for its module section to name.
fn root_class_records(writer: &mut encode::Writer, id: u32) {
    class_record(writer, id + 1, 0, |writer| {
        writer.write_boolean(true);
        writer.write_shipped(OBJECT_NAME); // superclass: Kernel Object
        writer.write_boolean(false); // no enclosing object
        writer.write_boolean(false); // no declaring class
    });
    metaclass_record(writer, id);
}

/// `levels` class records, each inheriting from the next, with the
/// outermost as the image's single root, followed by their `levels`
/// metaclass records in the same order, then the module root's
/// two (`root_class_records`) at `2 * levels`.
fn superclass_chain_graph(levels: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2 * levels + 2);
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
    root_class_records(&mut writer, 2 * levels);
    writer.write_u32(1); // one root
    writer.write_u8(VALUE_CLASS_TAG); // value tag: class tag
    writer.write_u8(CLASS_RECORD); // class tag: object-table record
    writer.write_u32(0);
    writer.write_u32(0); // no tag-named hashes
    writer.into_bytes()
}

/// `levels` instance records, each holding the next in its one field, with
/// the outermost as the image's single root -- the shape that has been
/// expressible since the object table existed -- then the module root's two
/// records (`root_class_records`) at `levels`.
fn instance_chain_graph(levels: u32) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, levels + 2);
    for level in 0..levels {
        writer.write_shipped(OBJECT_NAME);
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
    root_class_records(&mut writer, levels);
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
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP), 2 * DEEP);
    let mut loaded = image::load(&bytes).expect("load");
    let head = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let head_value = mutator.fetch(&head);
        assert_eq!(superclass_depth(mutator.heap(), head_value), DEEP);
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
    // The instantiation loop passes every level of the chain -- arity,
    // clause message, initializer -- and none of these levels has
    // initializer code, so none pushes an activation: the depth bound would
    // not see a per-level recursion (`docs/design/execution.md`, "A
    // superclass chain nests one activation per level"). Recursing once per
    // level would abort here.
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP), 2 * DEEP);
    let mut loaded = image::load(&bytes).expect("load");
    let head = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let head_value = mutator.fetch(&head);
        assert_eq!(superclass_depth(mutator.heap(), head_value), DEEP);
        let Some(_) = head_value.as_object() else {
            panic!("the root is a class object");
        };
    });
    let instance = expect_ok(loaded.interpreter.instantiate(&head, &[]), "instantiate");
    loaded.interpreter.enter(|mutator| {
        assert!(
            mutator.fetch(&instance).as_object().is_some(),
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
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP), 2 * DEEP);
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(superclass_depth(mutator.heap(), root_0), DEEP);
    });
    drop(loaded);
}

#[test]
fn dropping_a_loaded_world_whose_root_heads_a_long_instance_chain_does_not_blow_the_stack() {
    let bytes = image_with_graph_section(&instance_chain_graph(DEEP), DEEP);
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        assert_eq!(instance_chain_depth(mutator.heap(), root_0), DEEP);
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
    graph_counts(&mut writer, 3);
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
fn snapshot_module(module_source: &str, with_source: bool, extra_send: Option<&str>) -> Vec<u8> {
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
        snapshot_one(mutator, with_source, "t.ns", class, &roots).expect("snapshot")
    })
}

/// As `snapshot_module`, after the host has stored what `make` answers --
/// given the interpreter and the platform -- through the instance's setter
/// `setter`: for an object no program can make, such as a second platform.
fn snapshot_module_holding(
    module_source: &str,
    setter: &str,
    make: impl FnOnce(&mut Interpreter, &Root) -> Root,
) -> Vec<u8> {
    let (mut interpreter, class) = load(module_source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let held = make(&mut interpreter, &platform);
    expect_ok(
        interpreter.send(&instance, Symbol::intern(setter), &[held]),
        setter,
    );
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    })
}

/// A second platform, made by the host from the platform's class: no
/// program can make one.
fn a_second_platform(interpreter: &mut Interpreter, platform: &Root) -> Root {
    let platform_class = interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(platform));
        mutator.stash(Value::object(class))
    });
    expect_ok(
        interpreter.send(&platform_class, Symbol::NEW, &[]),
        "platform class new",
    )
}

#[test]
fn a_source_free_image_still_runs() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, false, None);
    assert!(
        !contains(&bytes, b"class Outer"),
        "a source-free image must not contain the module's source text"
    );
    assert!(
        !contains(&bytes, b"usingPlatform: p"),
        "nor any other fragment of it"
    );
    let mut loaded = image::load(&bytes).expect("load");
    assert!(loaded.modules[0].source.is_none());
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let result = expect_ok(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("answer"), &[]),
        "answer",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(
            mutator.fetch(&result).as_small_integer() == Some(42),
            "the entry send must run through the decoded code table"
        );
    });
}

/// The whole world, not just one send: a nested class reached through its
/// accessor, instantiated, and sent to -- all from an image whose bytes
/// carry no program text at all.
#[test]
fn a_source_free_image_still_reaches_its_nested_classes() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, false, None);
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
        .enter(|mutator| match mutator.fetch(&value).kind() {
            ValueKind::SmallInteger(9) => {}
            _ => panic!("expected SmallInteger(9) from Inner's `value`, got a different Value"),
        });
}

#[test]
fn an_image_with_source_still_carries_it() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, true, None);
    assert!(contains(&bytes, b"class Outer"));
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        loaded.modules[0].source.as_deref(),
        Some(SOURCE_FREE_MODULE),
        "an image with source hands its source back verbatim"
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
        assert!(mutator.fetch(&result).as_small_integer() == Some(42));
    });
}

/// The two variants differ only by the source: everything a world needs to
/// run is in both, so the source-free image is the shorter one and the
/// difference is exactly the text table and each row's `u32` text index.
/// The table carries the module's text once, split among its declarations:
/// per text its range, its hole count, its holes and its length-prefixed
/// text with the holes cut out.
// spec: image.texts
#[test]
fn the_two_variants_differ_only_by_the_text_table() {
    let with = snapshot_module(SOURCE_FREE_MODULE, true, None);
    let without = snapshot_module(SOURCE_FREE_MODULE, false, None);
    let report = image::inspect::inspect(&with).expect("inspect");
    let holes: usize = report.texts.iter().map(|text| text.holes.len()).sum();
    let table = 4 + report.texts.len() * (4 + 4 + 4 + 4) + holes * 8 + SOURCE_FREE_MODULE.len();
    assert_eq!(
        section_span(&report, "texts"),
        (
            section_span(&report, "texts").0,
            section_span(&report, "texts").0 + table
        ),
        "the text table carries the module's text once"
    );
    assert_eq!(
        with.len() - without.len(),
        table + 4 * report.mixins.len(),
        "the source costs the text table plus a u32 text index per row"
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
            decode::verify_graph(heap, graph, &table).expect_err("out-of-range program counter");
        assert_eq!(
            error,
            decode::DecodeError::ProgramCounterOutOfRange {
                program_counter: 99,
                instructions: 2
            }
        );
    });
}

/// A real image of a closure over a method-local block, whose captured
/// frame is `makeClosure`'s method frame; with the offset of that frame's
/// code id and the code id the closure record names, the block's own.
fn image_of_a_closure_over_a_method_frame() -> (Vec<u8>, usize, u32, usize) {
    let source = indoc! {"
        class Holder = Object new () (
          public makeClosure = ( ^[ 1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let closure = expect_ok(
        interpreter.send(&instance, Symbol::intern("makeClosure"), &[]),
        "makeClosure",
    );
    let bytes = interpreter.enter(|mutator| {
        let closure = mutator.fetch(&closure);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[closure]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let [frame] = report.frames.as_slice() else {
        panic!("the image holds one frame, the closure's captured one")
    };
    let code_offset = frame.code_offset.expect("a method frame names its body");
    let block_code = report
        .graph_objects
        .iter()
        .find_map(|object| {
            let (_, after) = object.split_once("closure code ")?;
            after.split(',').next()?.parse::<u32>().ok()
        })
        .expect("the image holds the closure");
    (bytes, code_offset, block_code, report.code.len())
}

/// A method frame whose code id names the block it created rather than
/// the method that ran in it is refused: the crafted bytes would
/// otherwise decode the frame against a body it never ran.
#[test]
fn a_frame_whose_code_disagrees_with_its_kind_is_refused() {
    let (mut bytes, code_offset, block_code, _) = image_of_a_closure_over_a_method_frame();
    bytes[code_offset..code_offset + 4].copy_from_slice(&block_code.to_le_bytes());
    let Err(error) = image::load(&bytes) else {
        panic!("a method frame running a block body must be refused")
    };
    assert!(
        error.contains("a frame's body is not one its kind runs"),
        "{error}"
    );
}

/// A method frame of a program class named by declaration rather than by
/// code id is refused: only a shipped body is named so, and the crafted
/// bytes would otherwise decode the frame against whatever the class's
/// current version carries, which after an edit is a body it never ran.
#[test]
fn a_declared_body_on_a_program_frame_is_refused() {
    let (mut bytes, code_offset, _, _) = image_of_a_closure_over_a_method_frame();
    bytes[code_offset - 1] = FRAME_BODY_DECLARED;
    bytes.drain(code_offset..code_offset + 4);
    let Err(error) = image::load(&bytes) else {
        panic!("a program frame named by declaration must be refused")
    };
    assert!(
        error.contains(&decode::DecodeError::DeclaredBodyNotShipped.to_string()),
        "{error}"
    );
}

/// A top-level frame runs no body, so one named by declaration is
/// refused: the crafted bytes would otherwise give the frame a body it
/// never ran. The describing sections come from a real snapshot, the
/// graph section is hand-written and spliced in where the snapshot's own
/// graph began.
#[test]
fn a_declared_body_on_a_top_level_frame_is_refused() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let mut bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[]).expect("snapshot")
    });
    bytes = with_fixture_names(&bytes);
    let graph_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|section| section.name == "graph")
        .expect("a snapshot has a graph section")
        .start;
    bytes.truncate(graph_start);
    let mut writer = one_frame_prefix();
    writer.write_u8(FRAME_TOP_LEVEL); // frame kind: TopLevel
    writer.write_u8(FRAME_BODY_DECLARED); // ...named by declaration
    frame_tail(&mut writer, 0, 0);
    writer.write_u32(0); // no roots
    writer.write_u32(0); // no tag-named hashes
    bytes.extend_from_slice(&writer.into_bytes());
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::DeclaredBodyNotShipped.to_string()
    );
}

/// An exception a handler returned out of with `^` keeps the frames of its
/// `signal` and of the `on:do:` that caught it, both shipped code, which
/// the code table never carries: the image names their bodies by
/// declaration, and the exception loads answering what it answered.
#[test]
fn an_exception_returned_out_of_its_handler_round_trips() {
    let source = indoc! {"
        class Holder usingPlatform: platform = (
          |
            Error = platform kernel Error.
          |
        ) (
          public caught = (
            [Error new signal: 'x'] on: Error do: [:exception | ^exception].
            ^nil
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    let exception = expect_ok(
        interpreter.send(&instance, Symbol::intern("caught"), &[]),
        "caught",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let exception = mutator.fetch(&exception);
        snapshot_one(mutator, false, "t.ns", class, &[exception])
            .unwrap_or_else(|error| panic!("snapshot: {error}"))
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    for selector in ["signal", "on:do:"] {
        assert!(
            report.graph_objects.iter().any(|object| object
                .contains(&format!("frame method {selector} (public), declared body"))),
            "the image holds the {selector} frame, named by declaration"
        );
    }
    let mut loaded = image::load(&bytes).expect("load");
    let exception = loaded.roots[0].clone();
    let text = expect_ok(
        loaded
            .interpreter
            .send(&exception, Symbol::intern("messageText"), &[]),
        "messageText",
    );
    loaded.interpreter.enter(|mutator| {
        let text = mutator.fetch(&text);
        assert_eq!(text_or_class_name(mutator, text), "x");
    });
}

/// A frame's code id past the code table is refused where it is read: the
/// crafted bytes would otherwise decode the frame against a body it never
/// ran, or index past the table.
// spec: image.verify.ids
#[test]
fn a_frame_code_id_past_the_table_is_refused() {
    let (mut bytes, code_offset, _, table_length) = image_of_a_closure_over_a_method_frame();
    let past = u32::try_from(table_length).expect("a small code table");
    bytes[code_offset..code_offset + 4].copy_from_slice(&past.to_le_bytes());
    let Err(error) = image::load(&bytes) else {
        panic!("a frame's code id past the table must be refused")
    };
    assert!(
        error.contains(&decode::DecodeError::BadCodeId(past).to_string()),
        "{error}"
    );
}

/// `load` never calls the parser, so source that could not possibly parse
/// is still just text the image carries around. The encoder writes the
/// heap's text, so the root declaration's text -- the module with `Inner`
/// cut out -- is overwritten in the bytes by one of the same length that
/// is no Newspeak, and the module's text assembles around it.
#[test]
fn the_source_an_image_carries_is_never_parsed() {
    let mut bytes = snapshot_module(SOURCE_FREE_MODULE, true, None);
    let inner_start = SOURCE_FREE_MODULE
        .find("public class Inner")
        .expect("the module declares Inner");
    let inner_close = "( ^v )\n    )";
    let inner_end = SOURCE_FREE_MODULE
        .find(inner_close)
        .map(|end| end + inner_close.len())
        .expect("Inner's body closes");
    let root_text = format!(
        "{}{}",
        &SOURCE_FREE_MODULE[..inner_start],
        &SOURCE_FREE_MODULE[inner_end..]
    );
    let not_newspeak: String = "this is not Newspeak at all ("
        .chars()
        .cycle()
        .take(root_text.len())
        .collect();
    assert_eq!(
        occurrences(&bytes, root_text.as_bytes()),
        1,
        "the image carries the root's text once"
    );
    let start = bytes
        .windows(root_text.len())
        .position(|window| window == root_text.as_bytes())
        .expect("the image carries the root's text");
    bytes[start..start + root_text.len()].copy_from_slice(not_newspeak.as_bytes());
    let loaded = image::load(&bytes).expect("load");
    let expected = format!(
        "{}{}{}",
        &not_newspeak[..inner_start],
        &SOURCE_FREE_MODULE[inner_start..inner_end],
        &not_newspeak[inner_start..]
    );
    assert_eq!(loaded.modules[0].source.as_deref(), Some(expected.as_str()));
}

/// A snapshot is taken between sends, when no activation is running. A
/// frame married to a live activation keeps its registers on the stack, not
/// in the frame the encoder reads, so a mid-execution snapshot would write
/// that frame with empty registers and report nothing. The encoder's
/// `frame_record` debug-asserts against it; this pins the refusal a
/// release build also makes.
#[test]
fn a_snapshot_taken_while_an_activation_is_running_is_refused() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let result = interpreter.enter(|mutator| {
        let running = mutator
            .heap_mut()
            .push_activation(
                ns_runtime::ActivationHeader {
                    receiver: Value::nil(),
                    current_class: None,
                    kind: ns_runtime::types::ActivationKind::TopLevel,
                    method: None,
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
                Arguments::NONE,
                0,
            )
            .expect("the stack holds the frame");
        let class = class_reference(mutator, &class);
        let result = snapshot_one(mutator, false, "t.ns", class, &[]);
        mutator.heap_mut().pop_activation(running);
        result
    });
    let Err(error) = result else {
        panic!("a snapshot with an activation still running must be refused")
    };
    assert!(error.contains("no activation is running"), "{error}");
}

/// The format records a frame's `sender`, and the decoder rebuilds it, but
/// the runtime never sets one: a live activation's sender is the stack frame
/// below it, and a returned one has none. So only a hand-built frame can
/// carry one into the encoder, and this is the test that keeps that half of
/// the frame record from rotting unnoticed.
// spec: image.graph.record
#[test]
fn a_frames_sender_survives_a_round_trip() {
    let (mut interpreter, _class, _original, decoded, _max) =
        round_trip_graph(TRIVIAL_MODULE, |interpreter, class| {
            // Nothing runs in here, so the class read out of its root stays
            // current.
            interpreter.enter(|mutator| {
                let class = class_reference(mutator, class);
                let foo = method_code(mutator.heap(), class, "foo");
                let temporaries = vec![Value::nil(); foo.register_count as usize];
                let sender = mutator.heap_mut().allocate_returned_top_level_frame(
                    Value::small_integer(7),
                    &temporaries,
                    None,
                    Some(class),
                );
                let frame = mutator.heap_mut().allocate_returned_top_level_frame(
                    Value::small_integer(8),
                    &temporaries,
                    Some(sender),
                    Some(class),
                );
                let closure = mutator.new_closure(NewClosure {
                    code: foo.clone(),
                    captured_frame: frame,
                    copied: Vec::new(),
                    feedback_base: ns_runtime::interpreter::FeedbackBase::NOT_ASSIGNED,
                });
                vec![mutator.stash(Value::object(closure))]
            })
        });
    interpreter.enter(|mutator| {
        let decoded_0 = mutator.fetch(&decoded[0]);
        let heap = mutator.heap();
        let home = heap
            .closure_home(object(decoded_0))
            .expect("expected a closure");
        assert!(heap.frame_receiver(home).as_small_integer() == Some(8));
        let sender = heap
            .frame_sender(home)
            .expect("the decoded frame keeps its sender");
        assert!(!sender.same(home));
        assert_eq!(heap.frame_receiver(sender).as_small_integer(), Some(7));
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
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let closure = mutator.fetch(&closure);
        let frame = mutator
            .heap()
            .closure_captured_frame(object(closure))
            .expect("makeClosure answers a closure");
        let activation = mutator.new_activation_object(frame);
        assert!(
            mutator
                .heap_mut()
                .instance_set_field(object(instance), 0, Value::object(activation)),
            "the holder has a slot"
        );
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let holder = object(mutator.fetch(&loaded.roots[0]));
        let activation_class = mutator.builtin_class(BuiltinClass::Activation);
        let heap = mutator.heap();
        let held = heap
            .instance_field(holder, 0)
            .expect("the loaded holder has its slot");
        let held = object(held);
        assert!(heap.object_class(held).same(activation_class));
        let frame = heap
            .object_frame(held)
            .expect("the slot holds an activation object");
        let receiver = heap.frame_receiver(frame);
        assert!(
            object(receiver).same(holder),
            "the loaded activation reifies the loaded frame, whose receiver is the loaded holder"
        );
    });
}

/// A decoded frame never ran in the loading world, so no `^` can return into
/// it, and a file that calls its home continuable would have it resumed
/// from registers the file chose: the loader refuses it outright.
#[test]
fn a_real_image_with_a_home_called_continuable_is_refused() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public mk = ( ^[ ^1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let closure = expect_ok(interpreter.send(&instance, Symbol::intern("mk"), &[]), "mk");
    // `mk` has returned, so its frame is divorced and marked uncontinuable.
    // Clear the flag, as a hand-edited file could.
    let bytes = interpreter.enter(|mutator| {
        let home = mutator
            .heap()
            .closure_home(object(mutator.fetch(&closure)))
            .expect("expected a closure");
        assert!(
            mutator.heap().frame_is_uncontinuable(home),
            "`mk` had returned"
        );
        mutator.heap_mut().frame_set_uncontinuable(home, false);
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        let closure = mutator.fetch(&closure);
        snapshot_one(mutator, false, "t.ns", class, &[instance, closure]).expect("snapshot")
    });

    match image::load(&bytes) {
        Err(message) => assert_eq!(message, decode::DecodeError::ContinuableFrame.to_string()),
        Ok(_) => panic!("a frame not marked uncontinuable must be refused"),
    }
}

/// A decoded home belongs to an activation that has returned and is never
/// married in the loading world, so a `^` into it must answer `cannotReturn`
/// rather than a `Signal::Return` no activation on the stack will catch.
#[test]
fn a_return_into_a_decoded_home_is_cannot_return() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public mk = ( ^[ ^1 ] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let closure = expect_ok(interpreter.send(&instance, Symbol::intern("mk"), &[]), "mk");
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        let closure = mutator.fetch(&closure);
        snapshot_one(mutator, false, "t.ns", class, &[instance, closure]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let decoded = loaded.roots[1].clone();
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
    /// commentaryAboutTheFrobulator explains nothing at all
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
/// punctuation (`)`, `(`, `) : ()`) that a binary stream can and does
/// reproduce by chance -- a `)` is one byte -- so testing them would report leaks
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

    let bytes = snapshot_module(LEAK_PROBE_MODULE, false, None);
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
    // present in the image with source. Without this, a bug that mangled the
    // needles (a trim that ate the text, say) would make the sweep above
    // pass for the wrong reason.
    let with_source = snapshot_module(LEAK_PROBE_MODULE, true, None);
    for line in LEAK_PROBE_MODULE.lines() {
        let line = line.trim();
        if line.len() < MIN_DISTINCTIVE_LINE {
            continue;
        }
        assert!(
            contains(&with_source, line.as_bytes()),
            "the image with source is missing the source line {line:?}"
        );
    }

    // And the world the source-free image describes still runs -- so what
    // was left out really was only text.
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let value = send_to(&mut loaded, &instance, "quixoticAccumulator");
    loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&value).kind() {
            ValueKind::SmallInteger(146) => {}
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
///
/// It keeps the optional module header the other sources leave out: the
/// fixture records source offsets, so dropping the header would move every
/// one of them without the format having changed.
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
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "golden.ns", class, &[instance]).expect("snapshot")
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

/// `NESTED_MODULE` written with source, outside any run: a root declaration
/// with one nested declaration, so the text table holds one text with a
/// hole and one without.
fn nested_module_with_source() -> Vec<u8> {
    let (mut interpreter, class) = load(NESTED_MODULE);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        module_only_image(mutator, true, "t.ns", class).expect("snapshot")
    })
}

/// `bytes` with the little-endian `u32` at `offset` replaced by `value`.
fn with_u32_at(bytes: &[u8], offset: usize, value: u32) -> Vec<u8> {
    let mut crafted = bytes.to_vec();
    crafted[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    crafted
}

/// A text the decoder cannot hold to its declaration is refused before any
/// mixin holds it: holes out of order, a hole past the range, an empty
/// hole, a length that does not add up, a text index past the table,
/// another number of holes than nested rows.
// spec: image.verify.ids
// spec: image.verify.texts
#[test]
fn a_crafted_declaration_text_is_refused() {
    let bytes = nested_module_with_source();
    image::load(&bytes).expect("the image as written loads");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let root_text_index = report.mixin_texts[0]
        .as_ref()
        .expect("the root row names a text");
    let root_text = &report.texts[root_text_index.index as usize];
    assert_eq!(root_text.holes.len(), 1, "the root text has Inner's hole");
    let inner_row = report
        .mixins
        .iter()
        .position(|line| line.starts_with("Inner "))
        .expect("a row for Inner");
    let inner_text_index = report.mixin_texts[inner_row]
        .as_ref()
        .expect("Inner's row names a text")
        .index;
    assert!(report.texts[inner_text_index as usize].holes.is_empty());
    let hole = root_text.holes[0];

    let past_the_range = with_u32_at(&bytes, root_text.holes_offset + 4, root_text.range.end + 1);
    let error = load_error(&past_the_range);
    assert!(
        error.contains("does not describe a declaration"),
        "a hole past the range: {error}"
    );

    let swapped = with_u32_at(&bytes, root_text.holes_offset, hole.end);
    let swapped = with_u32_at(&swapped, root_text.holes_offset + 4, hole.start);
    let error = load_error(&swapped);
    assert!(
        error.contains("does not describe a declaration"),
        "a hole whose start is past its end: {error}"
    );

    // An empty hole with the range shortened by the hole's width, so that
    // the cut text's length still adds up and only the emptiness is wrong:
    // a nested declaration is never empty.
    let width = hole.end - hole.start;
    let empty_hole = with_u32_at(&bytes, root_text.holes_offset + 4, hole.start);
    let empty_hole = with_u32_at(
        &empty_hole,
        root_text.offset + 4,
        root_text.range.end - width,
    );
    let error = load_error(&empty_hole);
    assert!(
        error.contains("does not describe a declaration"),
        "an empty hole: {error}"
    );

    let longer = with_u32_at(
        &bytes,
        root_text.cut_text_offset,
        u32::try_from(root_text.length + 1).expect("a short text"),
    );
    let error = load_error(&longer);
    assert!(
        error.contains("does not describe a declaration"),
        "a cut text one byte longer: {error}"
    );

    let past_the_table = with_u32_at(
        &bytes,
        root_text_index.offset,
        u32::try_from(report.texts.len()).expect("a short table"),
    );
    let error = load_error(&past_the_table);
    assert!(
        error.contains("names text"),
        "a text index past the table: {error}"
    );

    let without_the_hole = with_u32_at(&bytes, root_text_index.offset, inner_text_index);
    let error = load_error(&without_the_hole);
    assert!(
        error.contains("nested declarations"),
        "a text with no hole for a nested row: {error}"
    );
}

/// A module of two nested classes with no nested class of their own, so
/// that their rows name two texts of no hole each.
const TWO_LEAVES_MODULE: &str = indoc! {"
    class Outer usingPlatform: p = Object new () (
      class First = Object new () ()
      class Second = Object new () ()
    ) : ()
"};

/// A text belongs to one declaration. A text two rows name would be
/// assembled into the module's text once per row, so a short image could
/// make the load build an arbitrarily large one: two instance-side rows
/// naming one text, a class side naming another row's text and a text no
/// row names are each refused, never loaded and never a panic.
// spec: image.verify.texts
#[test]
fn a_text_not_owned_by_exactly_one_declaration_is_refused() {
    let (mut interpreter, class) = load(TWO_LEAVES_MODULE);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        module_only_image(mutator, true, "t.ns", class).expect("snapshot")
    });
    image::load(&bytes).expect("the image as written loads");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let row_named = |name: &str| {
        report
            .mixins
            .iter()
            .position(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("a row for {name}"))
    };
    let text_of = |row: usize| {
        report.mixin_texts[row]
            .as_ref()
            .expect("every row names a text")
    };
    let first = text_of(row_named("First"));
    let second = text_of(row_named("Second"));
    let root = text_of(0);
    let class_side = (1..report.mixin_texts.len())
        .find(|row| text_of(*row).index == root.index)
        .expect("Outer's class side names Outer's text");
    assert_eq!(
        report.texts.len(),
        3,
        "one text per instance-side declaration"
    );

    let shared = with_u32_at(&bytes, second.offset, first.index);
    let error = load_error(&shared);
    assert!(
        error.contains("is named by mixin-table rows"),
        "two instance-side rows naming one text: {error}"
    );

    let class_side_elsewhere = with_u32_at(&bytes, text_of(class_side).offset, first.index);
    let error = load_error(&class_side_elsewhere);
    assert!(
        error.contains("a class side, names text"),
        "a class side naming another row's text: {error}"
    );

    // One more text at the end of the table: a valid one, over one byte.
    let (texts_start, texts_end) = section_span(&report, "texts");
    let mut unnamed = with_u32_at(
        &bytes,
        texts_start,
        u32::try_from(report.texts.len() + 1).expect("a short table"),
    );
    let mut record = encode::Writer::new();
    record.write_u32(0);
    record.write_u32(1);
    record.write_u32(0);
    record.write_str("x");
    unnamed.splice(texts_end..texts_end, record.into_bytes());
    let error = load_error(&unnamed);
    assert!(
        error.contains("is named by no mixin-table row"),
        "a text no row names: {error}"
    );
}

/// Written with source and loaded, a module answers its text whole, and
/// each mixin slices its methods from its own text.
///
/// `SOURCE_FREE_MODULE` rather than `NESTED_MODULE`, whose root declares no
/// method: here both the root and its nested class do.
#[test]
fn a_loaded_module_answers_its_text_and_every_method_source() {
    let bytes = snapshot_module(SOURCE_FREE_MODULE, true, None);
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        loaded.modules[0].source.as_deref(),
        Some(SOURCE_FREE_MODULE),
        "the module answers its text whole"
    );
    let root = loaded.modules[0].declaration.clone();
    let inner = loaded
        .interpreter
        .enter(|mutator| mutator.heap().nested_mixins(&root))
        .into_iter()
        .find(|nested| nested.name.as_str() == "Inner")
        .expect("Outer declares Inner");
    for (mixin, selectors) in [(&root, vec!["answer", "block"]), (&inner, vec!["value"])] {
        let text = mixin.text.as_ref().expect("the image carries source");
        for selector in selectors {
            let method = mixin
                .compiled_method(Symbol::intern(selector))
                .expect("the declaration has the method");
            let span = method.code.source;
            let written = &SOURCE_FREE_MODULE[span.start as usize..span.end as usize];
            assert!(
                written.contains(selector),
                "{selector}'s span covers its source"
            );
            assert_eq!(
                text.slice(span),
                Some(written),
                "{} slices {selector} from its own text",
                mixin.name
            );
        }
    }
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
fn a_header_flag_this_build_does_not_assign_is_rejected() {
    // Bit 4 is not assigned. A flag this build does not implement means a
    // section it cannot see, so continuing would read the next field out of
    // whatever that section occupies.
    let mut writer = ns_runtime::image::encode::Writer::new();
    ns_runtime::image::encode::write_header(&mut writer, 0b1_0000);
    let bytes = writer.into_bytes();
    let mut reader = ns_runtime::image::decode::Reader::new(&bytes);
    assert!(ns_runtime::image::decode::read_header(&mut reader).is_err());
}

/// Bit 0 was the source variant that carried one text per module in the
/// module section. An image of that layout is refused as an unknown flag
/// rather than read as one whose texts sit in the text table.
// spec: image.header.flags
#[test]
fn a_header_flag_of_the_retired_module_text_layout_is_rejected() {
    use ns_runtime::image::decode::{DecodeError, Reader, read_header};
    use ns_runtime::image::encode::{RETIRED_HEADER_FLAGS, Writer, write_header};
    const MODULE_TEXTS: u8 = 1;
    assert_ne!(RETIRED_HEADER_FLAGS & MODULE_TEXTS, 0);
    let mut writer = Writer::new();
    write_header(&mut writer, MODULE_TEXTS);
    let bytes = writer.into_bytes();
    let error = match read_header(&mut Reader::new(&bytes)) {
        Ok(_) => panic!("the retired flag was accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        DecodeError::UnknownHeaderFlags { flags: 1 }.to_string()
    );
}

/// A module with an application's `main:args:`, for the tests that need one.
const ENTRY_SOURCE: &str = indoc! {"
    class X = Object new (
    ) (
      main: p args: q = ( ^1 )
    ) : ()
"};

/// `image::snapshot` is a library API: a caller can hand it an empty `roots`
/// slice. The image loads with no roots and is not started, so `ns-next load`
/// refuses it rather than instantiating an unrelated instance in place of the
/// one this image deliberately carries none of.
#[test]
fn a_snapshot_with_empty_roots_loads_with_no_roots_and_is_not_started() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);

    let image = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, false, "t.ns", class, &[]).expect("snapshot")
    });

    let loaded = image::load(&image).expect("load an image with no roots");
    assert!(loaded.roots.is_empty());
    assert!(!loaded.start);
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
        class X usingPlatform: p = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    for source in [ENTRY_SOURCE, TRIVIAL_MODULE, WITH_A_FRAME] {
        let (mut interpreter, class) = load(source);
        let platform = interpreter.platform();
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
            snapshot_one(mutator, false, "t.ns", class, &roots).expect("snapshot")
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
    let one = interpreter.stash(Value::small_integer(1));
    expect_ok(
        interpreter.send(&instance, Symbol::intern("apply:"), &[one]),
        "apply:",
    );
    assert!(interpreter.cache_statistics().filled_sites > 0);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "keeper.ns", class, &[instance]).expect("snapshot")
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
    let forty_one = loaded.interpreter.stash(Value::small_integer(41));
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("apply:"), &[forty_one]),
        "apply: after load",
    );
    let answer = loaded
        .interpreter
        .enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(integer) => integer,
            _ => panic!("apply: answers a small integer"),
        });
    assert_eq!(answer, 42);
    // `apply:`'s family is two sends (`keep`, `value:`); the decoded block's
    // is one (`+`, numbered 0 in `prepare`'s family, before the `keep:`
    // setter that takes the block as its argument). `value:` is a kernel
    // primitive method (`Kernel.ns`): caching its call site as
    // a `CachedTargetKind::Method` also reserves the target's own family --
    // `Closure>>value:`'s two sends (the lexical read of `vmMirror` and its
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
    let one = loaded.interpreter.stash(Value::small_integer(1));
    let second_answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("apply:"), &[one]),
        "apply: a second time after load",
    );
    let second_answer =
        loaded
            .interpreter
            .enter(|mutator| match mutator.fetch(&second_answer).kind() {
                ValueKind::SmallInteger(integer) => integer,
                _ => panic!("apply: answers a small integer"),
            });
    assert_eq!(second_answer, 2);
    assert_eq!(
        loaded.interpreter.cache_statistics().site_entries,
        booted + 5
    );
}

/// A decoded closure's range is reclaimed: the closure an image carries is
/// given a range of its own at its first activation, recorded as a loose
/// range, and once the program drops the closure, the next full collection
/// frees the range as it frees a dead family's. `collect_now` is a debug
/// build's.
#[cfg(debug_assertions)]
#[test]
fn a_decoded_closures_range_is_reclaimed() {
    let source = indoc! {"
        class Keeper = Object new (
            | keep ::= nil. |
        ) (
            public prepare = ( keep:: [:each | each + 1]. ^self )
            public apply: value = ( ^keep value: value )
            public drop = ( keep:: nil )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("prepare"), &[]),
        "prepare",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "keeper.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let keeper = loaded.roots[0].clone();
    let one = loaded.interpreter.stash(Value::small_integer(1));
    expect_ok(
        loaded
            .interpreter
            .send(&keeper, Symbol::intern("apply:"), &[one]),
        "apply: after load",
    );
    assert_eq!(
        loaded.interpreter.cache_statistics().loose_ranges,
        1,
        "the decoded closure took a range of its own"
    );
    loaded.interpreter.collect_now();
    assert_eq!(
        loaded.interpreter.cache_statistics().loose_ranges,
        1,
        "the range lives while the closure does"
    );
    expect_ok(
        loaded
            .interpreter
            .send(&keeper, Symbol::intern("drop"), &[]),
        "drop",
    );
    loaded.interpreter.collect_now();
    assert_eq!(
        loaded.interpreter.cache_statistics().loose_ranges,
        0,
        "the dropped closure's range is freed"
    );
}

/// A decoded closure that outlives a slide keeps its range: two closures an
/// image carries take a range each at their first activation, the second
/// above the first; once the program drops the first, a collection that
/// slides -- every one, under the collect stress mode, which this turns on
/// for it -- frees the first range and moves the second down, rewriting
/// the live closure's base and the loose range's key alike. The closure
/// answers as before, and the next collection keeps its range, which it
/// would free were the key no longer the base the closure names.
/// `collect_now` is a debug build's.
#[cfg(debug_assertions)]
#[test]
fn a_decoded_closure_keeps_its_range_across_a_slide() {
    let source = indoc! {"
        class Keeper = Object new (
            |
            first ::= nil.
            public second ::= nil.
            |
        ) (
            public prepare = (
                first:: [:each | each + 1].
                second:: [:each | each + 2].
                ^self
            )
            public applyFirst: value = ( ^first value: value )
            public applySecond: value = ( ^second value: value )
            public dropFirst = ( first:: nil )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("prepare"), &[]),
        "prepare",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "keeper.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let keeper = loaded.roots[0].clone();
    let apply = |loaded: &mut image::LoadedImage, selector: &str, value: i64| {
        let argument = loaded.interpreter.stash(Value::small_integer(value));
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&keeper, Symbol::intern(selector), &[argument]),
            selector,
        );
        loaded
            .interpreter
            .enter(|mutator| match mutator.fetch(&answer).kind() {
                ValueKind::SmallInteger(integer) => integer,
                _ => panic!("{selector} answers a small integer"),
            })
    };
    assert_eq!(apply(&mut loaded, "applyFirst:", 1), 2);
    assert_eq!(apply(&mut loaded, "applySecond:", 1), 3);
    assert_eq!(loaded.interpreter.cache_statistics().loose_ranges, 2);
    let second = expect_ok(
        loaded
            .interpreter
            .send(&keeper, Symbol::intern("second"), &[]),
        "second",
    );
    let before = loaded
        .interpreter
        .closure_feedback_base(&second)
        .expect("second holds a closure");
    expect_ok(
        loaded
            .interpreter
            .send(&keeper, Symbol::intern("dropFirst"), &[]),
        "dropFirst",
    );
    let slides = loaded.interpreter.cache_statistics().site_slides;
    loaded.interpreter.set_stress_collect(true);
    loaded.interpreter.collect_now();
    loaded.interpreter.set_stress_collect(false);
    let after = loaded
        .interpreter
        .closure_feedback_base(&second)
        .expect("second holds a closure");
    let statistics = loaded.interpreter.cache_statistics();
    assert!(statistics.site_slides > slides, "the collection slid");
    assert_eq!(
        statistics.loose_ranges, 1,
        "the dropped closure's range is freed"
    );
    assert!(
        after < before,
        "the live closure's base moved down, from {before} to {after}"
    );
    assert_eq!(apply(&mut loaded, "applySecond:", 2), 4);
    loaded.interpreter.collect_now();
    assert_eq!(
        loaded.interpreter.cache_statistics().loose_ranges,
        1,
        "the moved range is the one the live closure names"
    );
    assert_eq!(apply(&mut loaded, "applySecond:", 3), 5);
    if let Err(message) = loaded.interpreter.verify_cells() {
        panic!("after the slide: {message}");
    }
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
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_0 = mutator.fetch(&loaded.roots[0]);
        let held = mutator
            .heap()
            .instance_field(object(root_0), 0)
            .expect("expected an instance");
        let captured = mutator
            .heap()
            .closure_captured_frame(object(held))
            .expect("the slot holds the class side's block");
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
/// root's hidden slot holds it: the world, its root class, the root
/// instance and the nested class object.
fn metaclass_world() -> (Interpreter, Root, Root, Root) {
    let (mut interpreter, class, _platform, outer) = instantiated_world(METACLASS_MODULE);
    let holder = expect_ok(
        interpreter.send(&outer, Symbol::intern("Holder"), &[]),
        "Holder",
    );
    (interpreter, class, outer, holder)
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
        let Some(class) = mutator.fetch(class).as_object() else {
            panic!("a class object");
        };
        let metaclass = mutator
            .metaclass_of(class)
            .expect("every class object has a metaclass");
        mutator.stash(Value::object(metaclass))
    })
}

/// A metaclass keeps its identity across a round trip: `Holder class` held
/// in a slot is the decoded class's metaclass, not a second one.
#[test]
fn a_metaclass_in_a_slot_is_the_decoded_class_metaclass() {
    let (mut interpreter, class, outer, holder) = metaclass_world();
    let holder_metaclass = metaclass_root(&mut interpreter, &holder);
    store(&mut interpreter, &outer, "kept:", holder_metaclass);
    let bytes = snapshot_world(&mut interpreter, &class, std::slice::from_ref(&outer));
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
        let (Some(kept), Some(loaded_holder)) = (
            mutator.fetch(&kept).as_object(),
            mutator.fetch(&loaded_holder).as_object(),
        ) else {
            panic!("the slot and the accessor both answer class objects");
        };
        let metaclass = mutator
            .heap()
            .class_metaclass(loaded_holder)
            .expect("the decoded class record has its metaclass");
        assert!(kept.same(metaclass), "one metaclass, not two");
        assert!(
            mutator.heap().object_class(loaded_holder).same(kept),
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
        assert!(mutator.fetch(&made).as_small_integer() == Some(1));
    });
}

/// The metaclass of a builtin and of the root class is named by tag and
/// resolves to the loading world's own.
#[test]
fn a_builtin_metaclass_round_trips_by_tag() {
    let (mut interpreter, class, outer, _holder) = metaclass_world();
    let integer = interpreter.enter(|mutator| {
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        mutator.stash(Value::object(integer))
    });
    let integer_metaclass = metaclass_root(&mut interpreter, &integer);
    let root_metaclass = metaclass_root(&mut interpreter, &class);
    store(&mut interpreter, &outer, "kept:", integer_metaclass);
    store(&mut interpreter, &outer, "other:", root_metaclass);
    let bytes = snapshot_world(&mut interpreter, &class, std::slice::from_ref(&outer));
    let mut loaded = image::load(&bytes).expect("load");
    let loaded_outer = loaded.roots[0].clone();
    let kept = send_to(&mut loaded, &loaded_outer, "kept");
    let other = send_to(&mut loaded, &loaded_outer, "other");
    let loaded_class = loaded.modules[0].class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Some(kept), Some(other), Some(loaded_class)) = (
            mutator.fetch(&kept).as_object(),
            mutator.fetch(&other).as_object(),
            mutator.fetch(&loaded_class).as_object(),
        ) else {
            panic!("both slots hold metaclasses");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        assert!(
            mutator
                .metaclass_of(integer)
                .is_some_and(|metaclass| metaclass.same(kept))
        );
        assert!(
            mutator
                .metaclass_of(loaded_class)
                .is_some_and(|metaclass| metaclass.same(other))
        );
        // Each is its class's class, as the header says.
        assert!(mutator.heap().object_class(integer).same(kept));
        assert!(mutator.heap().object_class(loaded_class).same(other));
    });
}

/// A metaclass's identity hash travels with it: in its record for the
/// metaclass of a class record, in the closing hash table for the metaclass
/// of a builtin and of the root class.
#[test]
fn a_metaclass_identity_hash_survives() {
    let (mut interpreter, class, outer, holder) = metaclass_world();
    let holder_metaclass = metaclass_root(&mut interpreter, &holder);
    store(&mut interpreter, &outer, "kept:", holder_metaclass);
    let hashes = interpreter.enter(|mutator| {
        let (Some(holder), Some(root_class)) = (
            mutator.fetch(&holder).as_object(),
            mutator.fetch(&class).as_object(),
        ) else {
            panic!("class objects");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let metaclasses: Vec<ClassReference> = [holder, integer, root_class]
            .into_iter()
            .map(|class| mutator.metaclass_of(class).expect("a metaclass"))
            .collect();
        metaclasses
            .into_iter()
            .map(|metaclass| mutator.heap_mut().object_ensure_identity_hash(metaclass))
            .collect::<Vec<u32>>()
    });
    let bytes = snapshot_world(&mut interpreter, &class, std::slice::from_ref(&outer));
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .graph_tag_named_hashes
            .iter()
            .any(|entry| entry.starts_with("Kernel Integer class hash")),
        "{:?}",
        report.graph_tag_named_hashes
    );
    let mut loaded = image::load(&bytes).expect("load");
    let loaded_outer = loaded.roots[0].clone();
    let loaded_holder = nested_class_of(&mut loaded, &loaded_outer, "Holder");
    let loaded_class = loaded.modules[0].class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Some(holder), Some(root_class)) = (
            mutator.fetch(&loaded_holder).as_object(),
            mutator.fetch(&loaded_class).as_object(),
        ) else {
            panic!("class objects");
        };
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let loaded_hashes: Vec<u32> = [holder, integer, root_class]
            .into_iter()
            .map(|class| {
                let metaclass = mutator.metaclass_of(class).expect("a metaclass");
                mutator.heap().object_identity_hash(metaclass)
            })
            .collect();
        assert_eq!(loaded_hashes, hashes);
    });
}

/// The metaclass of a root level is a record like the level's, named by its
/// object id, and its identity hash travels in its header: it decodes to the
/// loaded level's own metaclass.
#[test]
fn a_root_level_metaclass_round_trips_as_a_record() {
    let source = indoc! {"
        class Root = Object new <: Value () ()
    "};
    let (mut interpreter, class) = load(source);
    let (bytes, hash) = interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let level = mutator
            .heap()
            .class_superclass(root_class)
            .expect("the root's level");
        let metaclass = mutator.metaclass_of(level).expect("a metaclass");
        let hash = mutator.heap_mut().object_ensure_identity_hash(metaclass);
        let bytes = snapshot_one(
            mutator,
            false,
            "t.ns",
            root_class,
            &[Value::object(metaclass)],
        )
        .expect("snapshot");
        (bytes, hash)
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(report.graph_roots, vec!["class object 0".to_string()]);
    assert!(
        report.graph_objects[0].contains(&format!("hash {hash}, metaclass of object ")),
        "the root is the level's metaclass record: {:?}",
        report.graph_objects
    );
    let mut loaded = image::load(&bytes).expect("load");
    let root = loaded.roots[0].clone();
    let loaded_class = loaded.modules[0].class.clone();
    loaded.interpreter.enter(|mutator| {
        let (Some(metaclass), Some(root_class)) = (
            mutator.fetch(&root).as_object(),
            mutator.fetch(&loaded_class).as_object(),
        ) else {
            panic!("class objects");
        };
        let level = mutator
            .heap()
            .class_superclass(root_class)
            .expect("the loaded root's level");
        assert!(
            mutator
                .metaclass_of(level)
                .is_some_and(|level_metaclass| level_metaclass.same(metaclass))
        );
        assert!(mutator.heap().object_class(level).same(metaclass));
        assert_eq!(mutator.heap().object_identity_hash(metaclass), hash);
    });
}

/// No program may name `Top`, `Activation`, `FutureImplementation` or the
/// metaclass of one of them as a value, and the loader refuses an image that
/// does, so the encoder refuses to write one rather than produce an image
/// nothing loads; the VM mirror's class and its metaclass have no
/// declaration, so no shipped name names them at all. No source module
/// reaches any of the eight; a graph built from Rust does. An activation
/// *object* is no such reach: its header names `Kernel Activation`, which
/// the loader accepts there alone (`an_activation_object_round_trips`).
// spec: image.shipped.positions
#[test]
fn a_graph_reaching_an_unnameable_class_is_refused() {
    let source = indoc! {"
        class Root = Object new () ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
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
        let future = mutator.builtin_class(BuiltinClass::FutureImplementation);
        let future_metaclass = mutator
            .metaclass_of(future)
            .expect("FutureImplementation has a metaclass");
        let as_a_value = |name: &str| {
            format!(
                "image encoding refuses {name} as a value: no program may name it there, and \
                 the loader refuses an image that does"
            )
        };
        let without_a_name = |name: &str| {
            format!(
                "image encoding refuses {name}: it has no declaration, so no shipped name names it"
            )
        };
        for (unnameable, expected) in [
            (top, as_a_value("Kernel Top")),
            (mirror_class, without_a_name("the VM mirror's class")),
            (top_metaclass, as_a_value("Kernel Top class")),
            (
                mirror_metaclass,
                without_a_name("the VM mirror's class class"),
            ),
            (activation, as_a_value("Kernel Activation")),
            (activation_metaclass, as_a_value("Kernel Activation class")),
            (future, as_a_value("Kernel FutureImplementation")),
            (
                future_metaclass,
                as_a_value("Kernel FutureImplementation class"),
            ),
        ] {
            let refused = snapshot_one(
                mutator,
                false,
                "t.ns",
                root_class,
                &[Value::object(unnameable)],
            );
            let Err(message) = refused else {
                panic!("a graph reaching {expected} was encoded");
            };
            assert_eq!(message, expected);
        }
    });
}

/// A graph reaching the VM mirror reaches its class too, and the refusal
/// names the capability: the mirror is checked first.
#[test]
fn a_graph_reaching_the_vm_mirror_is_refused_as_the_mirror() {
    let source = indoc! {"
        class Root = Object new () ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
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
            .find(|field| match field.kind() {
                ValueKind::Object(field) => heap.object_class(field).same(mirror_class),
                _ => false,
            })
            .expect("the kernel holds the mirror");
        let refused = snapshot_one(mutator, false, "t.ns", root_class, &[mirror]);
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
        graph_counts(&mut writer, 4);
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
            assert!(
                heap.class_metaclass(graph.objects[class])
                    .is_some_and(|found| found.same(graph.objects[metaclass]))
            );
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
    graph_counts(&mut writer, 3);
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
    graph_counts(&mut writer, 3);
    class_record(&mut writer, 1, 0, no_class_references);
    metaclass_record(&mut writer, 2); // names object 2, an instance
    writer.write_shipped(OBJECT_NAME);
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
    graph_counts(&mut writer, 2);
    class_record(&mut writer, 1, 0, no_class_references);
    writer.write_shipped(CLASS_NAME);
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

/// No source can make an instance of `Metaclass`, `Class` or a metaclass,
/// so an image recording one is refused.
#[test]
fn an_instance_of_metaclass_is_refused() {
    let instance_of = |class: &dyn Fn(&mut encode::Writer)| {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 1);
        class(&mut writer);
        writer.write_u32(3); // identity hash
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0); // no fields
        writer.write_u32(0); // no roots
        writer.into_bytes()
    };
    let shipped_class = |index: u32| move |writer: &mut encode::Writer| writer.write_shipped(index);
    // An instance of a metaclass record: a class record (object 0), its
    // metaclass record (object 1), and an instance whose header names it.
    let of_a_metaclass_record = {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 3);
        class_record(&mut writer, 1, 0, no_class_references);
        metaclass_record(&mut writer, 0);
        writer.write_u8(CLASS_RECORD);
        writer.write_u32(1); // its class: the metaclass record
        writer.write_u32(3); // identity hash
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0); // no fields
        writer.write_u32(0); // no roots
        writer.into_bytes()
    };
    let mut fixture = GraphFixture::new();
    let refused = [
        instance_of(&shipped_class(METACLASS_NAME)),
        instance_of(&shipped_class(CLASS_NAME)),
        of_a_metaclass_record,
    ];
    for bytes in refused {
        let error = fixture.decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::InstanceOfAClassClass);
        assert_eq!(
            error.to_string(),
            "an object in the image is an instance of Class, Metaclass or a metaclass, \
             which only a class object may be"
        );
    }
    // A shipped metaclass is no record's class at all.
    assert_eq!(
        fixture.decode_error(&instance_of(&shipped_class(INTEGER_CLASS_NAME))),
        decode::DecodeError::ShippedNameNotAllowed {
            name: "Kernel Integer class".to_string(),
            position: "a record's class",
        }
    );
    fixture.decoded(
        &instance_of(&shipped_class(OBJECT_NAME)),
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

/// `inspect` refuses a retired class tag the way `load` does: 4 was the
/// metaclass of a module's root class or level before those became records.
#[test]
fn inspect_refuses_a_retired_metaclass_tag() {
    let mut graph = one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_u8(4);
    });
    graph.extend_from_slice(&[0, 0, 0, 0]); // no roots
    graph.extend_from_slice(&[0, 0, 0, 0]); // no tag-named hashes
    let bytes = image_with_graph_section(&graph, 0);
    let error = match image::inspect::inspect(&bytes) {
        Ok(_) => panic!("inspect accepted a retired metaclass tag"),
        Err(error) => error,
    };
    assert_eq!(error, "invalid class tag 4");
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
    interpreter.enter(|mutator| match mutator.fetch(root).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    })
}

/// The text of the string `root` holds, read in a fresh entry.
fn text_of(interpreter: &mut Interpreter, root: &Root) -> String {
    interpreter.enter(|mutator| {
        let value = mutator.fetch(root);
        string_contents(mutator.heap(), value)
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
    let argument = interpreter.stash(Value::small_integer(factory_argument));
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
    let (mut interpreter, class, _platform, host) = instantiated_world(MIXIN_APPLICATION_MODULE);
    let expected = (5, 40, "Applied Stamp Marker Base".to_string());
    assert_eq!(applied_answers(&mut interpreter, &host, 4), expected);
    let applied_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Applied"), &[]),
        "Applied",
    );
    let argument = interpreter.stash(Value::small_integer(4));
    let instance = expect_ok(
        interpreter.send(&applied_class, Symbol::intern("from:"), &[argument]),
        "Applied from:",
    );
    let bytes = snapshot_world(&mut interpreter, &class, &[host, instance]);

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
        let applied = class_of(heap, instance);
        let stamp_level = heap.class_superclass(applied).expect("a superclass");
        let marker_level = heap.class_superclass(stamp_level).expect("a superclass");
        let base = heap.class_superclass(marker_level).expect("a superclass");
        assert_eq!(class_name(heap, stamp_level), "Stamp");
        assert_eq!(class_name(heap, marker_level), "Marker");
        assert_eq!(class_name(heap, base), "Base");
        let marker_clause = heap
            .class_applied_clause(marker_level)
            .expect("the Marker level carries the clause that added it");
        assert_eq!(marker_clause.message.selector.as_str(), "withMark:");
        assert!(marker_clause.message.has_arguments);
        assert!(heap.class_applied_clause(stamp_level).is_some());
        assert!(heap.class_applied_clause(base).is_none());
    });
}

/// An image with no roots carries no class record for the module's levels, so
/// the loaded world rebuilds every level from the row's clauses the first time
/// the accessor runs the clause -- which answers one class per clause, and a
/// decoded row that lost its clauses would refuse that answer. (The module
/// needs the platform, so `package` would refuse an instance of it.)
#[test]
fn an_image_of_a_module_applying_mixins_builds_its_levels_after_loading() {
    let (mut interpreter, class) = load(MIXIN_APPLICATION_MODULE);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        module_only_image(mutator, false, "t.ns", class).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let platform = loaded.interpreter.platform();
    let host = expect_ok(
        loaded
            .interpreter
            .instantiate(&loaded.modules[0].class, std::slice::from_ref(&platform)),
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
        class Root = Object new <: Value () (
            public answer = ( ^42 )
        )
    "};
    let (mut interpreter, class) = load(source);
    let application = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Root");
    let (packaged, snapshot) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let identity = ModuleIdentity::undeclared("t.ns", "Root");
        let modules = [image::ModuleSource {
            class,
            identity: &identity,
            with_source: false,
        }];
        let packaged =
            image::package(mutator, &modules, mutator.fetch(&application)).expect("package");
        let snapshot = snapshot_one(mutator, false, "t.ns", class, &[]).expect("snapshot");
        (packaged, snapshot)
    });
    for bytes in [packaged, snapshot] {
        let mut loaded = image::load(&bytes).expect("load");
        loaded.interpreter.enter(|mutator| {
            let heap = mutator.heap();
            let root = class_reference(mutator, &loaded.modules[0].class);
            let level = heap.class_superclass(root).expect("a superclass");
            let value = mutator.builtin_class(BuiltinClass::Value);
            assert!(Rc::ptr_eq(
                &application_of(heap, level).mixin,
                &application_of(heap, value).mixin
            ));
            let clause = heap
                .class_applied_clause(level)
                .expect("the level carries the root's clause");
            assert_eq!(clause.message.selector, Symbol::NEW);
            assert_eq!(
                heap.class_builtin(heap.class_superclass(level).expect("a superclass")),
                Some(BuiltinClass::Object)
            );
            assert!(heap.class_inherits_value(root));
        });
        let instance = expect_ok(
            loaded
                .interpreter
                .instantiate(&loaded.modules[0].class, &[]),
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

/// A `<:` clause's message is evaluated by the full initializer of the
/// class whose clause it is, and a block written in it captures that
/// initializer's activation. The block outlives it -- the applied level's
/// own part keeps it in a slot -- so a snapshot holds the dead activation as
/// a frame, and loading reconciles the frame against the body that ran in
/// it: the applying class's full initializer, not its own part, which a
/// frame of the same class could also run.
#[test]
fn a_frame_a_clause_message_ran_in_decodes_to_the_full_initializer() {
    let source = indoc! {"
        class Host usingPlatform: p = Object new () (
            public class Keeper withBlock: block = Object new (| kept = block. |) (
                public run = ( ^kept value )
            )
            public class Holder from: x = Object new <: Keeper withBlock: [x * 3] () ()
        ) : ()
    "};
    let (mut interpreter, class, _platform, host) = instantiated_world(source);
    let holder_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Holder"), &[]),
        "Holder",
    );
    let argument = interpreter.stash(Value::small_integer(5));
    let holder = expect_ok(
        interpreter.send(&holder_class, Symbol::intern("from:"), &[argument]),
        "Holder from:",
    );
    let bytes = snapshot_world(&mut interpreter, &class, &[holder]);

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    let answer = expect_ok(
        loaded.interpreter.send(&holder, Symbol::intern("run"), &[]),
        "run",
    );
    assert_eq!(integer_of(&mut loaded.interpreter, &answer), 15);

    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    assert!(
        report.contains("frame initializer of object"),
        "inspect names the frame's kind:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// Mixin applications from hand-crafted bytes: every id, tag and position a
// level's description holds is checked before it is used.
// ---------------------------------------------------------------------------

/// The smallest module whose mixin table has a row with a `<:` clause.
const APPLYING_MODULE: &str = indoc! {"
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
/// superclass), and was added by clause `clause` of row `applying_row`,
/// which named the shipped `Object`. Its metaclass is the record
/// `metaclass_id`.
fn applied_level_record(
    writer: &mut encode::Writer,
    (metaclass_id, mixin_row): (u32, u32),
    superclass: Option<u32>,
    (applying_row, clause): (u32, u32),
) {
    applied_level_record_naming(
        writer,
        (metaclass_id, mixin_row),
        superclass,
        (applying_row, clause),
        |writer| writer.write_shipped(OBJECT_NAME),
    );
}

/// `applied_level_record` whose clause named the class `named` writes the
/// tag of.
fn applied_level_record_naming(
    writer: &mut encode::Writer,
    (metaclass_id, mixin_row): (u32, u32),
    superclass: Option<u32>,
    (applying_row, clause): (u32, u32),
    named: impl FnOnce(&mut encode::Writer),
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
    named(writer);
}

/// A graph section of exactly `records`, written by `write`, and no roots.
fn class_records(count: u32, write: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, count);
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
        assert_eq!(clause.message.selector, Symbol::NEW);
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
    for (row, index) in [(applied, 1), (marker, 0)] {
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

/// A clause index at or past the clause limit of 1,024, which no resolved
/// declaration numbers, is refused before it is looked up, so it is never
/// truncated into another clause.
// spec: image.limit.clause-index
#[test]
fn an_applied_levels_clause_index_past_the_clause_limit_is_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    for index in [1024, 1u32 << 23, u32::MAX] {
        let bytes = class_records(2, |writer| {
            applied_level_record(writer, (1, marker), None, (applied, index));
            metaclass_record(writer, 0);
        });
        let error = fixture.decode_error(&bytes);
        assert_eq!(
            error,
            decode::DecodeError::ClauseIndexTooLarge {
                row: applied,
                index
            }
        );
        assert_eq!(
            error.to_string(),
            format!(
                "mixin application clause index {index} of row {applied} is past the limit of 1024 clauses"
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

/// Each level's metaclass is rebuilt from the class its clause named, so
/// levels that name each other name no class side at all: the rebuild
/// refuses them rather than walking the cycle forever.
#[test]
fn applied_levels_naming_each_other_in_a_cycle_are_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    let record = |id: u32| {
        move |writer: &mut encode::Writer| {
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(id);
        }
    };
    let through_another = class_records(4, |writer| {
        applied_level_record_naming(writer, (2, marker), None, (applied, 0), record(1));
        applied_level_record_naming(writer, (3, marker), None, (applied, 0), record(0));
        metaclass_record(writer, 0);
        metaclass_record(writer, 1);
    });
    let directly = class_records(2, |writer| {
        applied_level_record_naming(writer, (1, marker), None, (applied, 0), record(0));
        metaclass_record(writer, 0);
    });
    for bytes in [through_another, directly] {
        assert_eq!(
            fixture.decode_error(&bytes),
            decode::DecodeError::NamedClassCycle
        );
    }
}

/// A body-less declaration's class object applies its last clause's mixin
/// (`object-model.md`, "Mixin application"), so a class record applying a
/// body-less row's own mixin describes a class no live world has.
#[test]
fn a_class_record_applying_a_body_less_rows_mixin_is_refused() {
    let mut fixture = GraphFixture::with_source(indoc! {"
        class Host = Object new () (
            class Marker = Object new () ()
            class Applied = Object new <: Marker.
        ) : ()
    "});
    let applied = row_named(&fixture, "Applied");
    let bytes = class_records(2, |writer| {
        applied_level_record(writer, (1, applied), None, (applied, 0));
        metaclass_record(writer, 0);
    });
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::BodyLessMixinApplied
    );
}

/// The smallest module whose mixin table has a body-less declaration.
const BODY_LESS_MODULE: &str = indoc! {"
    class Host = Object new () (
        class Marker = Object new () ()
        class Applied = Object new <: Marker.
    ) : ()
"};

/// A graph section of three records: a class record with body tag `body`
/// for the level clause 0 of row `applying_row` added, applying `Marker`'s
/// row, which `scope` ends; its metaclass record; and an empty `Array`.
fn body_less_class_section(
    fixture: &GraphFixture,
    body: u8,
    applying_row: u32,
    scope: impl FnOnce(&mut encode::Writer),
) -> Vec<u8> {
    let marker = row_named(fixture, "Marker");
    class_records(3, |writer| {
        writer.write_u8(CLASS_RECORD); // class tag: its metaclass record
        writer.write_u32(1);
        writer.write_u32(7); // identity hash
        writer.write_u8(body);
        writer.write_u8(MIXIN_ROW); // mixin reference: a mixin-table row
        writer.write_u32(marker);
        writer.write_boolean(false); // no superclass
        writer.write_boolean(false); // no enclosing object
        writer.write_boolean(false); // no declaring class
        writer.write_boolean(true); // an applied level
        writer.write_u32(applying_row);
        writer.write_u32(0);
        writer.write_shipped(OBJECT_NAME); // the class the clause named
        scope(writer);
        metaclass_record(writer, 0);
        writer.write_shipped(ARRAY_NAME);
        writer.write_u32(1); // identity hash
        writer.write_u8(OBJECT_ARRAY); // object body: Array
        writer.write_u32(0); // no elements
    })
}

/// A body-less declaration's class object carries its declaration's scope
/// after the level's record (`OBJECT_BODY_LESS_CLASS`), each reference
/// checked as a class record's own are: an object id past the table, an
/// enclosing object that is no object and a declaring class that is no
/// class are refused, never followed. The tag must agree with the clause
/// both ways.
#[test]
fn a_body_less_class_records_scope_is_checked() {
    let mut fixture = GraphFixture::with_source(BODY_LESS_MODULE);
    let applied = row_named(&fixture, "Applied");
    let enclosing_array = |writer: &mut encode::Writer| {
        writer.write_boolean(true);
        writer.write_u8(VALUE_OBJECT_ID);
        writer.write_u32(2);
        writer.write_boolean(false);
    };
    let bytes = body_less_class_section(&fixture, OBJECT_BODY_LESS_CLASS, applied, enclosing_array);
    fixture.decoded(&bytes, "a body-less class object", |heap, graph| {
        let scope = heap
            .class_declaration_scope(graph.objects[0])
            .expect("the record's scope is on its class object");
        assert!(
            scope
                .enclosing
                .is_some_and(|enclosing| enclosing.same(graph.objects[2]))
        );
        assert!(scope.declaring_class.is_none());
    });
    type WritesScope = Box<dyn Fn(&mut encode::Writer)>;
    let refusals: [(WritesScope, decode::DecodeError); 4] = [
        (
            Box::new(|writer| {
                writer.write_boolean(true);
                writer.write_u8(VALUE_OBJECT_ID);
                writer.write_u32(99);
                writer.write_boolean(false);
            }),
            decode::DecodeError::BadObjectId(99),
        ),
        (
            Box::new(|writer| {
                writer.write_boolean(true);
                writer.write_u8(VALUE_TRUE);
                writer.write_boolean(false);
            }),
            decode::DecodeError::EnclosingNotAnObject,
        ),
        (
            Box::new(|writer| {
                writer.write_boolean(false);
                writer.write_boolean(true);
                writer.write_u8(CLASS_RECORD);
                writer.write_u32(99);
            }),
            decode::DecodeError::BadObjectId(99),
        ),
        (
            Box::new(|writer| {
                writer.write_boolean(false);
                writer.write_boolean(true);
                writer.write_u8(CLASS_RECORD);
                writer.write_u32(2); // the Array
            }),
            decode::DecodeError::NotAClassReference,
        ),
    ];
    for (scope, error) in refusals {
        let bytes = body_less_class_section(&fixture, OBJECT_BODY_LESS_CLASS, applied, scope);
        assert_eq!(fixture.decode_error(&bytes), error);
    }
    // The clause decides which tag the record must have.
    let without_scope = body_less_class_section(&fixture, OBJECT_CLASS, applied, |_| {});
    assert_eq!(
        fixture.decode_error(&without_scope),
        decode::DecodeError::BodyLessClassRecordMismatch
    );
    let mut with_body = GraphFixture::with_source(APPLYING_MODULE);
    let applying = row_named(&with_body, "Applied");
    let bytes = body_less_class_section(&with_body, OBJECT_BODY_LESS_CLASS, applying, |writer| {
        writer.write_boolean(false);
        writer.write_boolean(false);
    });
    assert_eq!(
        with_body.decode_error(&bytes),
        decode::DecodeError::BodyLessClassRecordMismatch
    );
}

/// A body-less declaration declares nothing a body would, nested classes
/// included: nothing could reach them, since no class applies its mixin.
// spec: image.verify.mixin-rows
#[test]
fn a_body_less_row_with_nested_declarations_is_refused() {
    let fixture = GraphFixture::with_source(BODY_LESS_MODULE);
    let applied = fixture.mixins[row_named(&fixture, "Applied") as usize].clone();
    let marker = fixture.mixins[row_named(&fixture, "Marker") as usize].clone();
    let decoded = |mixin: Mixin| mixins::Decoded {
        mixins: vec![Rc::new(mixin)],
        ..mixins::Decoded::empty()
    };
    assert_eq!(decode::verify_mixins(&decoded((*applied).clone())), Ok(()));
    let mut with_nested = (*applied).clone();
    with_nested.nested_declarations = ns_runtime::mixin::NestedDeclarations::new(vec![marker]);
    assert_eq!(
        decode::verify_mixins(&decoded(with_nested)),
        Err(decode::DecodeError::BodyLessWithMembers)
    );
}

/// A body-less declaration is the mixin application its clauses write, so
/// a body-less row without a clause describes no declaration.
// spec: image.verify.mixin-rows
#[test]
fn a_body_less_row_without_clauses_is_refused() {
    let fixture = GraphFixture::with_source(BODY_LESS_MODULE);
    let applied = fixture.mixins[row_named(&fixture, "Applied") as usize].clone();
    let decoded = |mixin: Mixin| mixins::Decoded {
        mixins: vec![Rc::new(mixin)],
        ..mixins::Decoded::empty()
    };
    assert_eq!(decode::verify_mixins(&decoded((*applied).clone())), Ok(()));
    let mut without_clauses = (*applied).clone();
    without_clauses.mixin_applications = Vec::new();
    assert_eq!(
        decode::verify_mixins(&decoded(without_clauses)),
        Err(decode::DecodeError::BodyLessWithoutClauses)
    );
}

/// A body-less declaration's full initializer is its declaration's own
/// code, marked so when its row is built, and its lexical sends start from
/// the declaration's scope: a row that names it as another body -- of
/// another row, or another of its own -- would run it there from a frame
/// that is no frame of the declaration, which a debug build aborts on.
/// Refused before anything runs.
// spec: image.verify.initializers
#[test]
fn a_body_less_declarations_initializer_named_as_another_body_is_refused() {
    let fixture = GraphFixture::with_source(BODY_LESS_MODULE);
    let applied = fixture.mixins[row_named(&fixture, "Applied") as usize].clone();
    let marker = fixture.mixins[row_named(&fixture, "Marker") as usize].clone();
    let declaration_code = applied
        .initializer_code
        .clone()
        .expect("a body-less declaration has a full body");
    let decoded = |rows: Vec<Mixin>| mixins::Decoded {
        mixins: rows.into_iter().map(Rc::new).collect(),
        ..mixins::Decoded::empty()
    };
    assert_eq!(
        decode::verify_mixins(&decoded(vec![(*applied).clone(), (*marker).clone()])),
        Ok(())
    );
    let mut sharing = (*marker).clone();
    sharing.initializer_code = Some(declaration_code.clone());
    let error = decode::verify_mixins(&decoded(vec![(*applied).clone(), sharing]))
        .expect_err("another row's full body is the declaration's");
    assert_eq!(error, decode::DecodeError::SharedDeclarationCode);
    assert_eq!(
        error.to_string(),
        "a body-less declaration's instance initializer is named as another body too"
    );
    let mut own_part = (*applied).clone();
    own_part.own_initializer_code = Some(declaration_code);
    assert_eq!(
        decode::verify_mixins(&decoded(vec![own_part])),
        Err(decode::DecodeError::SharedDeclarationCode)
    );
}

/// A snapshot of a module whose slot `held` holds a block a body-less
/// declaration's clause created: a block of the declaration's own code,
/// captured by the declaration's full initializer at its class object.
fn body_less_clause_block_snapshot() -> Vec<u8> {
    let source = indoc! {"
        class Module = Object new (|
          library = Library new.
          public held = Applied new a.
        |) (
          class Library = Object new () (
            public class Marker tag: t = Object new (| public a = t. |) ()
            public helper = ( ^'the library helper' )
          )
          class Applied = Object new <: libraryMarker tag: [helper].
          libraryMarker = ( ^library Marker )
          helper = ( ^'the module helper' )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    })
}

/// The block verifies, and after load it still reaches the module's
/// `helper` from the declaration's scope, not the library's from the
/// applied mixin's.
#[test]
fn a_block_from_a_body_less_clause_survives_build_and_load() {
    let bytes = body_less_clause_block_snapshot();
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let held = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("held"), &[]),
        "held",
    );
    let value = expect_ok(
        loaded.interpreter.send(&held, Symbol::intern("value"), &[]),
        "value",
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(
            text_or_class_name(mutator, mutator.fetch(&value)),
            "the module helper"
        );
    });
}

/// The same block's captured frame recast as the own part of the
/// initializer, which runs the same kind of body: a frame that runs the
/// declaration's own code anywhere but as its full initializer is refused,
/// since the code, not the frame, decides where its sends start.
#[test]
fn a_frame_running_a_body_less_declarations_code_elsewhere_is_refused() {
    let mut bytes = body_less_clause_block_snapshot();
    rewrite_in_graph(
        &mut bytes,
        &[OBJECT_FRAME, FRAME_INITIALIZER],
        &[OBJECT_FRAME, FRAME_OWN_INITIALIZER],
    );
    assert_eq!(
        load_error(&bytes),
        decode::DecodeError::FrameCodeMismatch.to_string()
    );
}

/// One past the last mixin reference, and every retired one: a stale
/// image's builtin level names no mixin rather than whatever took the number.
#[test]
fn an_unknown_mixin_reference_tag_is_a_decode_error_not_a_panic() {
    for tag in [MIXIN_SHIPPED + 1].into_iter().chain(RETIRED_MIXIN_TAGS) {
        let bytes = class_records(1, |writer| {
            writer.write_shipped(OBJECT_NAME);
            writer.write_u32(7); // identity hash
            writer.write_u8(OBJECT_CLASS); // object body: Class
            writer.write_u8(tag);
            writer.write_u32(0);
            no_class_references(writer);
            writer.write_boolean(false);
        });
        assert_eq!(
            GraphFixture::new().decode_error(&bytes),
            decode::DecodeError::InvalidTag {
                what: "mixin reference",
                tag
            }
        );
    }
}

/// A class record for a level applying the mixin of the shipped name at
/// `name` of the fixture's section, with no superclass, added by clause
/// `index` of row `row`, and its metaclass record. Such a record carries no
/// enclosing object or declaring class: both are the shipped class's.
fn shipped_mixin_record(name: u32, (row, index): (u32, u32)) -> Vec<u8> {
    class_records(2, |writer| {
        writer.write_u8(CLASS_RECORD); // class tag: its metaclass record
        writer.write_u32(1);
        writer.write_u32(7); // identity hash
        writer.write_u8(OBJECT_CLASS); // object body: Class
        writer.write_u8(MIXIN_SHIPPED); // mixin reference: a shipped class's
        writer.write_u32(name);
        writer.write_boolean(false); // no superclass
        writer.write_u32(row);
        writer.write_u32(index);
        writer.write_shipped(VALUE_NAME); // the class the clause named
        metaclass_record(writer, 0);
    })
}

/// A `<:` clause naming a shipped class gives its level that class's mixin,
/// which the record names by its shipped name, and the class's own enclosing
/// object and declaring class -- the positive side of the refusals below.
#[test]
fn a_level_applying_a_shipped_mixin_decodes() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    let bytes = shipped_mixin_record(VALUE_NAME, (applied, 0));
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

/// The clause of a level applying a shipped class is checked like any other
/// level's: its applying row and its position there.
#[test]
fn a_shipped_levels_clause_is_checked_like_any_levels() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    assert_eq!(
        fixture.decode_error(&shipped_mixin_record(VALUE_NAME, (99, 0))),
        decode::DecodeError::BadMixinId(99)
    );
    assert_eq!(
        fixture.decode_error(&shipped_mixin_record(VALUE_NAME, (applied, 3))),
        decode::DecodeError::BadMixinApplicationClause {
            row: applied,
            index: 3
        }
    );
}

/// A level's mixin is named where a superclass stands, so `Top`,
/// `Activation`, `FutureImplementation` and a module's root are refused
/// there, and so is anything but a class -- a metaclass, a module instance
/// -- whose mixin no level applies. The VM mirror's class has no name at
/// all, so no image can apply its mixin.
// spec: image.shipped.positions
#[test]
fn a_level_applying_a_mixin_no_program_may_name_is_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    for (name, described) in [
        (TOP_NAME, "Kernel Top"),
        (ACTIVATION_NAME, "Kernel Activation"),
        (FUTURE_IMPLEMENTATION_NAME, "Kernel FutureImplementation"),
        (KERNEL_ROOT_NAME, "Kernel Kernel"),
        (BEHAVIOR_NAME, "Kernel Behavior"),
    ] {
        assert_eq!(
            fixture.decode_error(&shipped_mixin_record(name, (applied, 0))),
            decode::DecodeError::ShippedNameNotAllowed {
                name: described.to_string(),
                position: "a superclass or an applied mixin",
            }
        );
    }
    for name in [INTEGER_CLASS_NAME, KERNEL_INSTANCE_NAME] {
        assert_eq!(
            fixture.decode_error(&shipped_mixin_record(name, (applied, 0))),
            decode::DecodeError::NotAClassReference
        );
    }
    let past = u32::try_from(FIXTURE_NAMES.len()).expect("a count fits a u32");
    assert_eq!(
        fixture.decode_error(&shipped_mixin_record(past, (applied, 0))),
        decode::DecodeError::BadShippedName(past)
    );
}

/// An object's class is a place an image names a class too: a module's
/// root is no record's class -- its one instance is the module's, named as
/// such -- and a module instance is no class at all.
// spec: image.shipped.positions
#[test]
fn an_objects_class_naming_a_module_root_or_instance_is_refused() {
    let under = |name: u32| {
        class_records(1, |writer| {
            writer.write_shipped(name);
            writer.write_u32(5); // identity hash
            writer.write_u8(OBJECT_INSTANCE); // object body: Instance
            writer.write_u32(0); // no fields
        })
    };
    let error = GraphFixture::new().decode_error(&under(KERNEL_ROOT_NAME));
    assert_eq!(
        error,
        decode::DecodeError::ShippedNameNotAllowed {
            name: "Kernel Kernel".to_string(),
            position: "a record's class",
        }
    );
    assert_eq!(
        error.to_string(),
        "the image names Kernel Kernel as a record's class, where no program may name it"
    );
    assert_eq!(
        GraphFixture::new().decode_error(&under(KERNEL_INSTANCE_NAME)),
        decode::DecodeError::NotAClassReference
    );
}

/// One record under the header `header` writes, identity hash 5, body tag
/// `body`, then -- for a frame record -- a top-level frame's body, or for
/// any other tag the four bytes of the frame-table id the retired activation
/// record (tag 9) carried; no roots.
fn record_under(header: impl FnOnce(&mut encode::Writer), body: u8) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 1);
    header(&mut writer);
    writer.write_u32(5); // identity hash
    writer.write_u8(body);
    if body == OBJECT_FRAME {
        writer.write_u8(FRAME_TOP_LEVEL);
        no_body(&mut writer);
        frame_tail(&mut writer, 0, 0);
    } else {
        writer.write_u32(0);
    }
    writer.write_u32(0); // no roots
    writer.into_bytes()
}

/// The one header a frame record may carry: the builtin `Activation`.
fn activation_header(writer: &mut encode::Writer) {
    writer.write_shipped(ACTIVATION_NAME);
}

/// A well-formed frame record decodes into a frame object -- the object the
/// kernel holds as an activation -- carrying the record's identity hash. The
/// refusals below are this record with one thing changed.
#[test]
fn a_frame_record_decodes_into_a_frame_object() {
    let bytes = record_under(activation_header, OBJECT_FRAME);
    GraphFixture::new().decoded(&bytes, "a frame record", |heap, graph| {
        let frame = heap.object_frame(graph.objects[0]).expect("a frame object");
        assert!(frame.same(graph.frames[0]));
        assert_eq!(heap.object_identity_hash(graph.objects[0]), 5);
    });
}

/// The activation record's tag, 9, is retired: its body was a frame-table
/// id, which an object id read in its place would resolve to another record.
#[test]
fn a_record_under_the_retired_activation_tag_is_refused() {
    let bytes = record_under(activation_header, 9);
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::InvalidTag {
            what: "object body",
            tag: 9
        }
    );
}

/// A frame record under any class but `Activation` would be an activation
/// answering that class's methods; no live world has one.
#[test]
fn a_frame_record_under_another_class_is_refused() {
    let bytes = record_under(
        |writer| {
            writer.write_shipped(OBJECT_NAME);
        },
        OBJECT_FRAME,
    );
    let error = GraphFixture::new().decode_error(&bytes);
    assert_eq!(error, decode::DecodeError::FrameRecordUnderAnotherClass);
    assert_eq!(
        error.to_string(),
        "a frame record in the image names a class other than Activation"
    );
}

/// The `Activation` header is accepted on a frame record only: on any other
/// record it would make an ordinary object answer `Activation`'s methods,
/// which read other frames and unwind the stack.
// spec: image.shipped.positions
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
        decode::DecodeError::ShippedNameNotAllowed {
            name: "Kernel Activation".to_string(),
            position: "a record's class",
        }
    );
}

/// The one-array graph whose element is the class the shipped name at
/// `name` names, as a value.
fn shipped_class_value(name: u32) -> Vec<u8> {
    one_array_object(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_shipped(name);
    })
}

/// Everywhere but a frame record's header, `Activation` is refused like
/// `Top`: it, and its metaclass, as a value.
// spec: image.shipped.positions
#[test]
fn activation_named_anywhere_else_is_refused() {
    for (name, described) in [
        (ACTIVATION_NAME, "Kernel Activation"),
        (ACTIVATION_CLASS_NAME, "Kernel Activation class"),
        (TOP_NAME, "Kernel Top"),
    ] {
        assert_eq!(
            GraphFixture::new().decode_error(&shipped_class_value(name)),
            decode::DecodeError::ShippedNameNotAllowed {
                name: described.to_string(),
                position: "a value",
            }
        );
    }
}

/// Everywhere but a future record's header, `FutureImplementation` is
/// refused like `Activation`: it, and its metaclass, as a value. Its class
/// is kernel-private, and no program names it.
// spec: image.shipped.positions
#[test]
fn a_graph_naming_the_future_implementation_as_a_class_is_refused() {
    for (name, described) in [
        (FUTURE_IMPLEMENTATION_NAME, "Kernel FutureImplementation"),
        (
            FUTURE_IMPLEMENTATION_CLASS_NAME,
            "Kernel FutureImplementation class",
        ),
    ] {
        assert_eq!(
            GraphFixture::new().decode_error(&shipped_class_value(name)),
            decode::DecodeError::ShippedNameNotAllowed {
                name: described.to_string(),
                position: "a value",
            }
        );
    }
}

/// One record under the `FutureImplementation` header, its body written by
/// `body`, and no roots.
fn future_record(body: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    class_records(1, |writer| {
        writer.write_shipped(FUTURE_IMPLEMENTATION_NAME);
        writer.write_u32(5); // identity hash
        body(writer);
    })
}

/// The encoder writes only an unresolved future -- two fields, its
/// computation and state 0 -- so a record under the `FutureImplementation`
/// header that is anything else describes a future no encoder wrote:
/// another body, another field count, a resolving or resolved state, or a
/// state that is none of the three. Each is refused at decode rather than
/// left for `future_state` to meet. The computation may be any value, the
/// record itself included: forcing that one is the divergent-evaluation
/// error, as a program's own would be.
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
    // The record itself, id 0.
    let itself: fn(&mut encode::Writer) = |writer| {
        writer.write_u8(VALUE_OBJECT_ID);
        writer.write_u32(0);
    };
    for (bytes, described) in [
        (array, "an array body"),
        (three_fields, "three fields"),
        (with_state(itself, 1), "a resolving future"),
        (with_state(nil, 2), "a resolved future"),
        (with_state(nil, 7), "state 7"),
    ] {
        let error = GraphFixture::new().decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::FutureRecord, "{described}");
    }
    assert_eq!(
        decode::DecodeError::FutureRecord.to_string(),
        "an object in the image is a FutureImplementation without an unresolved future's record"
    );
}

/// A record under the `FutureImplementation` header has exactly a future's
/// two fields, so the first pass refuses any other count before anything is
/// allocated: neither the record's placeholder nor a copy of its fields ever
/// exists, however many fields it claims and carries.
#[test]
fn a_future_record_with_another_field_count_is_refused_before_allocating() {
    let bytes = future_record(|writer| {
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(100_000);
        for _ in 0..100_000 {
            writer.write_u8(VALUE_NIL);
        }
    });
    let mut fixture = GraphFixture::new();
    let before = fixture.interpreter.heap_statistics().live_objects;
    assert_eq!(
        fixture.decode_error(&bytes),
        decode::DecodeError::FutureRecord
    );
    assert_eq!(
        fixture.interpreter.heap_statistics().live_objects,
        before,
        "nothing was allocated"
    );
}

/// A module whose `probe` slot holds §6.3.2's data cycle: each of `Probe`'s
/// arrays holds the other's future, resolved once the clause ends. `kept` is
/// for a test to fill, and `second` answers a block to make a future of.
const DATA_CYCLE: &str = indoc! {"
    class Holder = Object new (
      | public probe = Probe new. public kept. |
    ) (
      class Probe = Object new (|| public first = {second}. public second = {first}. ||) () : ()
      public second = ( ^[probe second] )
    ) : ()
"};

/// Whether any object reachable from `value` through instance fields, array
/// elements and closure values is a `FutureImplementation`. A graph is only
/// what its roots reach, so this covers every object an image decoded.
fn reaches_a_future<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> bool {
    let heap = mutator.heap();
    let future_class = mutator.builtin_class(BuiltinClass::FutureImplementation);
    let mut seen: Vec<ObjectReference<'gc>> = Vec::new();
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let Some(object) = value.as_object() else {
            continue;
        };
        if heap.is_class(object) || seen.iter().any(|visited| visited.same(object)) {
            continue;
        }
        seen.push(object);
        if heap.object_class(object).same(future_class) {
            return true;
        }
        if let Some(elements) = heap.array_elements(object) {
            pending.extend(elements);
        }
        if let Some(count) = heap.instance_field_count(object) {
            pending.extend((0..count).filter_map(|index| heap.instance_field(object, index)));
        }
        if let Some(copied) = heap.closure_copied_values(object) {
            pending.extend(copied);
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
        let closure = object(mutator.fetch(&block));
        let future = mutator.new_future(closure);
        mutator.stash(Value::object(future))
    });
    // A message to the future forces it; a host root keeps the future itself.
    expect_ok(
        interpreter.send(&future, Symbol::intern("size"), &[]),
        "future size",
    );
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
                .instance_set_field(object(holder), 1, Value::object(kept)),
            "the holder has its kept slot"
        );
        assert!(reaches_a_future(mutator, holder), "the premise");
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[holder]).expect("snapshot")
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
        let first = object(mutator.fetch(&first));
        let second = object(mutator.fetch(&second));
        let kept = object(mutator.fetch(&kept));
        let element = heap
            .array_element(first, 0)
            .expect("first holds one element");
        assert!(
            object(element).same(second),
            "first's element is second's array itself"
        );
        let back = heap
            .array_element(second, 0)
            .expect("second holds one element");
        assert!(object(back).same(first), "and second's is first's");
        let kept_element = heap.array_element(kept, 0).expect("kept holds one element");
        assert!(
            object(kept_element).same(second),
            "kept's element is second's array itself"
        );
    });
}

/// An abandoned clause leaves `escaped` holding an unresolved future
/// (`spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch`). The image
/// writes it as a record under the `FutureImplementation` header, with its
/// closure, and the loaded world forces it on the first touch, which runs the
/// initializer again. The first touch is an argument to a leaf, which sees
/// through it only once the loaded world knows it holds a future: the load
/// sets `futures_made` as `Mutator::new_future` does.
#[test]
fn an_unresolved_future_round_trips_and_computes_after_load() {
    let source = indoc! {"
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
    let bytes = interpreter.enter(|mutator| {
        let holder = mutator.fetch(&holder);
        let escaped = mutator
            .heap()
            .instance_field(object(holder), 0)
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
        snapshot_one(mutator, true, "t.ns", class, &[holder]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let escaped = mutator
            .heap()
            .instance_field(object(mutator.fetch(&holder)), 0)
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
        assert!(mutator.fetch(&sum).as_small_integer() == Some(4));
        assert!(mutator.fetch(&next).as_small_integer() == Some(4));
    });
}

/// A program's objects of the kernel's newer classes survive an image: a
/// caught `ArgumentError` keeps its argument, an instance of a `Proxy`
/// subclass still forwards to its target, and an unresolved
/// `Future computing:` is written as the runtime's future and computes its
/// block after the load -- as does one whose computation is another
/// unresolved future, which only its forcing after the load forces.
#[test]
fn an_argument_error_a_proxy_and_a_programs_future_round_trip() {
    let source = indoc! {"
        class Holder = Object new (
            | Proxy ::= nil. public error. public proxy. public future. public chain. |
        ) (
            class Forwarder on: theTarget = Proxy new (
                | private target = theTarget. |
            ) (
                protected doesNotUnderstand: message = ( ^message sendTo: target )
            )
            public capture: platform = (
                | kernel = platform kernel. |
                Proxy:: kernel Proxy.
                error:: [(Array new: 2) at: 7] on: kernel ArgumentError do: [:caught | caught].
                proxy:: Forwarder on: 3.
                future:: kernel Future computing: [6 * 7].
                chain:: kernel Future computing: (kernel Future computing: [[6 * 7]])
            )
            public argument = ( ^error argument )
            public proxied = ( ^proxy + 4 )
            public computed = ( ^future + 0 )
            public chained = ( ^chain + 0 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    let platform = interpreter.platform();
    expect_ok(
        interpreter.send(
            &holder,
            Symbol::intern("capture:"),
            std::slice::from_ref(&platform),
        ),
        "capture:",
    );
    let bytes = interpreter.enter(|mutator| {
        let holder = mutator.fetch(&holder);
        let future = mutator
            .heap()
            .instance_field(object(holder), 3)
            .expect("the holder has its future slot");
        let future = mutator
            .as_future(future)
            .expect("future holds the runtime's future");
        assert!(
            matches!(
                mutator.future_state(future),
                ns_runtime::interpreter::future::FutureState::Unresolved(_)
            ),
            "the future is unresolved when the image is written"
        );
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[holder]).expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    let holder = loaded.roots[0].clone();
    let argument = expect_ok(
        loaded
            .interpreter
            .send(&holder, Symbol::intern("argument"), &[]),
        "argument",
    );
    let proxied = expect_ok(
        loaded
            .interpreter
            .send(&holder, Symbol::intern("proxied"), &[]),
        "proxied",
    );
    let computed = expect_ok(
        loaded
            .interpreter
            .send(&holder, Symbol::intern("computed"), &[]),
        "computed",
    );
    let chained = expect_ok(
        loaded
            .interpreter
            .send(&holder, Symbol::intern("chained"), &[]),
        "chained",
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(mutator.fetch(&argument).as_small_integer(), Some(7));
        assert_eq!(mutator.fetch(&proxied).as_small_integer(), Some(7));
        assert_eq!(mutator.fetch(&computed).as_small_integer(), Some(42));
        assert_eq!(mutator.fetch(&chained).as_small_integer(), Some(42));
    });
}

/// Crafted code can corrupt a future's slots once it holds one; the encoder
/// refuses to write such a future rather than write a record the decoder
/// would refuse (`future::fields_for_an_image`), and does not see through a
/// "resolved" one whose value is another future.
#[test]
fn a_future_with_corrupted_fields_is_refused_by_the_encoder() {
    let source = indoc! {"
        class Holder = Object new (
          | public held |
        ) (
          public block = ( ^[3] )
        ) : ()
    "};
    for corruption in ["state 7", "resolved to each other"] {
        let (mut interpreter, class) = load(source);
        let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
        let block = expect_ok(
            interpreter.send(&holder, Symbol::intern("block"), &[]),
            "block",
        );
        let refusal = interpreter.enter(|mutator| {
            let Some(closure) = mutator.fetch(&block).as_object() else {
                panic!("block answers a block");
            };
            let holder = mutator.fetch(&holder);
            let future = mutator.new_future(closure);
            let other = mutator.new_future(closure);
            let heap = mutator.heap_mut();
            match corruption {
                "state 7" => {
                    heap.instance_set_field(future, 1, Value::small_integer(7));
                }
                _ => {
                    heap.instance_set_field(future, 0, Value::object(other));
                    heap.instance_set_field(future, 1, Value::small_integer(2));
                    heap.instance_set_field(other, 0, Value::object(future));
                    heap.instance_set_field(other, 1, Value::small_integer(2));
                }
            }
            heap.instance_set_field(object(holder), 0, Value::object(future));
            let class = class_reference(mutator, &class);
            match snapshot_one(mutator, true, "t.ns", class, &[holder]) {
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
        writer.write_shipped(OBJECT_NAME);
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

/// The root rule (`docs/design/object-model.md`, "The root rule"), held to
/// a root class's superclass chain as the encoder and the loader both hold
/// it (`modules::root_record_defect`): `Object <: Value` passes, and every
/// chain the rule does not build -- the level skipped, `Value` applied over
/// `Value`, the level over a class but `Object`, the root itself standing
/// as a level -- is refused.
// spec: image.verify.modules
// spec: image.verify.root-rule
#[test]
fn the_root_rule_holds_a_root_classs_superclass_chain() {
    let source = indoc! {"
        class Root = Object new <: Value () ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let root = class_reference(mutator, &class);
        let root_mixin = mutator
            .heap()
            .class_application(root)
            .expect("a class")
            .mixin
            .clone();
        let level = mutator
            .heap()
            .class_superclass(root)
            .expect("the root's level");
        let object = mutator.builtin_class(BuiltinClass::Object);
        let value = mutator.builtin_class(BuiltinClass::Value);
        let array = mutator.builtin_class(BuiltinClass::Array);
        assert_eq!(
            image::modules::root_record_defect(mutator, &root_mixin, root),
            None
        );
        assert_eq!(
            image::modules::root_record_defect(mutator, &root_mixin, level),
            Some(image::modules::RootRecordDefect::NotItsRecord),
            "a level is no root"
        );
        mutator.heap_mut().class_set_superclass(level, value);
        assert_eq!(
            image::modules::root_record_defect(mutator, &root_mixin, root),
            Some(image::modules::RootRecordDefect::RootRule),
            "Value applied over Value"
        );
        mutator.heap_mut().class_set_superclass(level, array);
        assert_eq!(
            image::modules::root_record_defect(mutator, &root_mixin, root),
            Some(image::modules::RootRecordDefect::RootRule),
            "Value applied over Array"
        );
        mutator.heap_mut().class_set_superclass(level, object);
        mutator.heap_mut().class_set_superclass(root, object);
        assert_eq!(
            image::modules::root_record_defect(mutator, &root_mixin, root),
            Some(image::modules::RootRecordDefect::RootRule),
            "a root whose clause added no level"
        );
    });
}

/// A root's `<:` level is a record like the root, and loads as the level
/// its clause added: applying `Kernel Value` over the loading boot's own
/// `Kernel Object`.
#[test]
fn a_root_level_loads_as_a_record_applying_kernel_value() {
    let source = indoc! {"
        class Root = Object new <: Value () ()
    "};
    let loaded = image::load(&snapshot_of(source)).unwrap_or_else(|error| panic!("{error}"));
    let module_class = loaded.modules[0].class.clone();
    let mut interpreter = loaded.interpreter;
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let root = class_reference(mutator, &module_class);
        let level = heap.class_superclass(root).expect("the loaded level");
        let applied = heap.class_applied_origin(level).expect("a level");
        assert_eq!(applied.index, 0);
        assert_eq!(applied.named_builtin, Some(BuiltinClass::Value));
        let value = mutator.builtin_class(BuiltinClass::Value);
        assert!(Rc::ptr_eq(
            &heap.class_application(level).expect("a class").mixin,
            &heap.class_application(value).expect("a class").mixin
        ));
        assert!(
            heap.class_superclass(level)
                .is_some_and(|below| below.same(mutator.builtin_class(BuiltinClass::Object)))
        );
    });
}

/// A root the rule forbids can never be instantiated, so no image is written
/// for it, and both writers say why with the instantiation's text. The
/// resolver refuses such a root, so it is loaded here the one way it still
/// reaches an interpreter: a `ResolvedModule` a host builds by hand.
#[test]
fn the_encoder_refuses_a_root_the_rule_forbids() {
    let source = indoc! {"
        class Root = Array new () ()
    "};
    let resolution = ns_runtime::resolve_for_editor(ns_syntax::parse(source).expect("parse"));
    assert_eq!(resolution.errors.len(), 1, "one root-rule resolve error");
    let resolved = ns_runtime::ResolvedModule {
        module: Rc::new(resolution.module),
        source: Rc::from(source),
        origin: SourceOrigin::File(PathBuf::from("t.ns")),
        bindings: Rc::new(resolution.bindings),
        kernel: false,
        identity: None,
    };
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let (snapshot, packaged) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let identity = ModuleIdentity::undeclared("t.ns", "Root");
        let modules = [image::ModuleSource {
            class,
            identity: &identity,
            with_source: false,
        }];
        (
            snapshot_one(mutator, false, "t.ns", class, &[]),
            // The refusal comes before the graph is looked at, so the
            // application need not exist: no instance of this class can.
            image::package(mutator, &modules, Value::nil()),
        )
    });
    let expected = "image encoding cannot serialize Root: a module's root class may inherit only \
                    Object or Value, or write Object <: Value, and its superclass clause names Array";
    assert_eq!(snapshot.err().as_deref(), Some(expected));
    assert_eq!(packaged.err().as_deref(), Some(expected));
}

/// `MIXIN_APPLICATION_MODULE`'s `Applied`, edited by `edit`, reloaded as a
/// mixin table: what `verify_mixins` says about it.
fn verify_doctored_applied(edit: impl FnOnce(Mixin) -> Mixin) -> Result<(), decode::DecodeError> {
    let (mut interpreter, class) = load(MIXIN_APPLICATION_MODULE);
    interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let doctored = with_nested(mutator.heap(), class, "Applied", edit);
        install_mixin(mutator, class, doctored);
        let (_table, decoded_mixins, _ids) = round_trip_code_table(mutator.heap(), class);
        decode::verify_mixins(&decoded_mixins)
    })
}

/// `<:` follows a superclass clause in the grammar, and it is the superclass
/// clause's body that evaluates every `<:` receiver; a row with clauses but
/// no superclass clause describes a declaration no source has.
// spec: image.verify.mixin-rows
#[test]
fn a_row_applying_mixins_without_a_superclass_clause_is_refused_at_load() {
    let error = verify_doctored_applied(|applied| Mixin {
        superclass: None,
        superclass_clause_code: None,
        ..applied
    })
    .expect_err("clauses with no superclass clause");
    assert_eq!(
        error,
        decode::DecodeError::MixinApplicationWithoutSuperclass
    );
}

/// `inspect` shows everything the format now says about a mixin
/// application: each row's clauses and their message bodies, which clause
/// added a level, and the root's own levels.
#[test]
fn inspect_prints_the_mixin_applications_an_image_describes() {
    let (mut interpreter, class, _platform, host) = instantiated_world(MIXIN_APPLICATION_MODULE);
    let expected = (5, 40, "Applied Stamp Marker Base".to_string());
    assert_eq!(applied_answers(&mut interpreter, &host, 4), expected);
    let bytes = snapshot_world(&mut interpreter, &class, &[host]);
    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    for expected in [
        "mixin application 1  withMark:\n",
        "mixin application 2  new\n",
        "applied by clause 0 of row ",
        "applied by clause 1 of row ",
    ] {
        assert!(
            report.contains(expected),
            "inspect prints `{expected}`:\n{report}"
        );
    }

    let source = indoc! {"
        class Root = Object new <: Value () ()
    "};
    let bytes = snapshot_of(source);
    let root = module_root_object(&bytes, 0);
    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    for expected in [
        "source: yes".to_string(),
        format!("root object {root}"),
        "class mixin shipped Kernel Value, superclass Kernel Object, applied by clause 0 of row 0"
            .to_string(),
    ] {
        assert!(
            report.contains(&expected),
            "inspect prints the root and its level as records, `{expected}`:\n{report}"
        );
    }
}

/// A nested class's `<:` clause can name a builtin too, and its level is then
/// a class record whose mixin no row describes: the record names it by the
/// builtin's shipped name, and the reloaded level applies the loading
/// bootstrap's own builtin mixin.
#[test]
fn a_nested_level_applying_a_builtin_mixin_round_trips_as_a_record() {
    let source = indoc! {"
        class Host usingPlatform: p = Object <: Value () (
            public class Point = Object new <: Value () (
                public answer = ( ^7 )
            )
        ) : ()
    "};
    let (mut interpreter, class, _platform, host) = instantiated_world(source);
    let point_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Point"), &[]),
        "Point",
    );
    let point = expect_ok(interpreter.instantiate(&point_class, &[]), "Point new");
    let bytes = snapshot_world(&mut interpreter, &class, &[point]);

    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    assert!(
        report.contains("class mixin shipped Kernel Value"),
        "the level's record names Value's mixin by its shipped name:\n{report}"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let point = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let point = mutator.fetch(&point);
        let heap = mutator.heap();
        let level = heap
            .class_superclass(class_of(heap, point))
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

/// A `<:` clause can name a shipped class that is no builtin, here the
/// collections facet's `List`: its level is a class record applying `List`'s
/// mixin (`MIXIN_SHIPPED`), and the reloaded level applies the loading
/// boot's own `List` mixin, enclosed where `List` is.
#[test]
fn a_level_applying_a_shipped_class_that_is_no_builtin_round_trips() {
    let source = indoc! {"
        class Host usingPlatform: p = Object new (| public listClass = p collections List. |) (
            public class Listy = Object new <: listClass new: 4 () (
                public answer = ( ^7 )
            )
        ) : ()
    "};
    let (mut interpreter, class, _platform, host) = instantiated_world(source);
    let listy_class = expect_ok(
        interpreter.send(&host, Symbol::intern("Listy"), &[]),
        "Listy",
    );
    let listy = expect_ok(interpreter.instantiate(&listy_class, &[]), "Listy new");
    let three = interpreter.stash(Value::small_integer(3));
    expect_ok(
        interpreter.send(&listy, Symbol::intern("add:"), &[three]),
        "listy add: 3",
    );
    let bytes = snapshot_world(&mut interpreter, &class, &[listy]);

    let report = image::inspect::render_full(&image::inspect::inspect(&bytes).expect("inspect"));
    assert!(
        report.contains("class mixin shipped CollectionsFactory List"),
        "the level's record names List's mixin by its shipped name:\n{report}"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let listy = loaded.roots[0].clone();
    let loaded_platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &loaded_platform, "collections");
    let list = send_to(&mut loaded, &collections, "List");
    loaded.interpreter.enter(|mutator| {
        let listy = mutator.fetch(&listy);
        let list = object(mutator.fetch(&list));
        let heap = mutator.heap();
        let level = heap
            .class_superclass(class_of(heap, listy))
            .expect("Listy's level");
        assert!(Rc::ptr_eq(
            &application_of(heap, level).mixin,
            &application_of(heap, list).mixin
        ));
        assert!(heap.class_applied_clause(level).is_some());
        assert!(
            heap.class_enclosing(level)
                .zip(heap.class_enclosing(list))
                .is_some_and(|(first, second)| first.same(second)),
            "the level is enclosed where List is"
        );
    });
    let answer = send_to(&mut loaded, &listy, "answer");
    assert_eq!(integer_of(&mut loaded.interpreter, &answer), 7);
    let size = send_to(&mut loaded, &listy, "size");
    assert_eq!(integer_of(&mut loaded.interpreter, &size), 1);
}

/// §3.1.1's enclosing-object condition on a value class is decided when the
/// class object is created and only *recorded* on it
/// (`Heap::class_value_refusal`, `docs/design/object-model.md`, "A
/// value object's four conditions", "Enclosing objects are values");
/// `decide_instantiation` is what refuses, at instantiation. A decoded class
/// object starts with no verdict of its own, so a legitimate snapshot
/// holding a class that was created carrying one has to have it recomputed
/// on load (`image::decode::rebuild_applications`) or the round trip would
/// silently make an already-refused class instantiable.
///
/// `Counter` inherits `Value` directly but is enclosed by a fresh `Holder`
/// each time `MutableCounter` builds one, so it is a refused value class
/// from the moment it is created -- `goldens/library/value/a_value_subclass_of_a_
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
    let (mut interpreter, class, _platform, module) = instantiated_world(source);
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
        &class,
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
/// (`goldens/library/value/a_value_module_defining_identity_is_refused.ns` pins the
/// same shape as a golden).
#[test]
fn a_root_defining_identity_stays_refused_after_a_snapshot_round_trip() {
    let source = indoc! {"
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
            .instantiate(&loaded.modules[0].class, std::slice::from_ref(&platform)),
        "ValueModuleDefiningIdentity new",
    );
    assert_eq!(
        message,
        "cannot instantiate ValueModuleDefiningIdentity: a value class may not define ==, and \
         ValueModuleDefiningIdentity defines it"
    );
}

// ---------------------------------------------------------------------------
// Shipped names: an object the loading binary owns is named by its
// declaration in the image's shipped-name section, and resolved on load
// against the loading boot's own shipped world by table lookup
// (`docs/design/image-format.md`, "Shipped names").
// ---------------------------------------------------------------------------

/// A module whose root keeps three platform members and the platform itself
/// in slots of its own.
const PLATFORM_MEMBERS: &str = indoc! {"
    class Holder usingPlatform: platform = Object new (
        | public transcript = platform transcript.
          public collections = platform collections.
          public time = platform time.
          public keptPlatform = platform. |
    ) (
    ) : ()
"};

/// The span of `bytes` the section `name` occupies, as `inspect` reports it.
fn section_span(report: &image::inspect::Report, name: &str) -> (usize, usize) {
    let located = report
        .sections
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("the image carries a {name} section"));
    (located.start, located.end)
}

/// `bytes` with its shipped-name section replaced by what `section` writes.
/// `section` is handed each text's id in the symbol table, which is extended
/// by any text the table lacks: an id already assigned keeps its meaning,
/// so the code and mixin tables still read as they did.
fn with_shipped_name_section(
    bytes: &[u8],
    section: impl FnOnce(&mut dyn FnMut(&str) -> u32, &mut encode::Writer),
) -> Vec<u8> {
    let report = image::inspect::inspect(bytes).expect("inspect");
    let (symbols_start, symbols_end) = section_span(&report, "symbols");
    let (names_start, names_end) = section_span(&report, "shipped names");
    let mut table = symbols::Builder::new();
    for symbol in &report.symbols {
        table.intern(symbol);
    }
    let mut written = encode::Writer::new();
    section(&mut |text| table.intern(text), &mut written);
    let mut symbol_section = encode::Writer::new();
    table.write(&mut symbol_section);
    let mut crafted = bytes[..symbols_start].to_vec();
    crafted.extend_from_slice(&symbol_section.into_bytes());
    crafted.extend_from_slice(&bytes[symbols_end..names_start]);
    crafted.extend_from_slice(&written.into_bytes());
    crafted.extend_from_slice(&bytes[names_end..]);
    crafted
}

/// One section entry: the module's and the declaration's symbol ids and the
/// kind byte, then `clause` for a level or a level's metaclass.
fn write_shipped_entry(
    writer: &mut encode::Writer,
    intern: &mut dyn FnMut(&str) -> u32,
    (module, declaration, kind): (&str, &str, u8),
    clause: Option<u32>,
) {
    writer.write_u32(intern(module));
    writer.write_u32(intern(declaration));
    writer.write_u8(kind);
    if let Some(clause) = clause {
        writer.write_u32(clause);
    }
}

/// `bytes` with the section of `FIXTURE_NAMES`, so a graph spliced in after
/// it may use the fixture's indices (`image_with_graph_section`).
fn with_fixture_names(bytes: &[u8]) -> Vec<u8> {
    with_shipped_name_section(bytes, |intern, writer| {
        writer.write_u32(u32::try_from(FIXTURE_NAMES.len()).expect("a count fits a u32"));
        for entry in FIXTURE_NAMES {
            write_shipped_entry(writer, intern, *entry, None);
        }
    })
}

/// The class of `subject`, read on the host side, since `class` is
/// answered only to its receiver's own sends.
fn class_root_of(loaded: &mut image::LoadedImage, subject: &Root) -> Root {
    loaded.interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(subject));
        mutator.stash(Value::object(class))
    })
}

fn same_object(loaded: &mut image::LoadedImage, first: &Root, second: &Root) -> bool {
    loaded
        .interpreter
        .enter(|mutator| object(mutator.fetch(first)).same(object(mutator.fetch(second))))
}

/// Whether `first = second` answers `true` in the loaded world.
fn equal(loaded: &mut image::LoadedImage, first: &Root, second: &Root) -> bool {
    let answer = expect_ok(
        loaded
            .interpreter
            .send(first, Symbol::intern("="), std::slice::from_ref(second)),
        "=",
    );
    loaded
        .interpreter
        .enter(|mutator| matches!(mutator.fetch(&answer).kind(), ValueKind::Boolean(true)))
}

/// The platform is a record of `PlatformFactory Platform` like any instance,
/// its lazy slots as they stood, and its facets are records of their shipped
/// classes: each decodes to a value equal to the loading platform's own --
/// a lazy slot is no part of a value's state -- and each answers as before.
#[test]
fn a_platform_member_round_trips_as_a_record() {
    let bytes = snapshot_module(PLATFORM_MEMBERS, false, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    for name in [
        "TranscriptFactory Transcript",
        "CollectionsFactory Collections",
        "TimeFactory Time",
        "PlatformFactory Platform",
    ] {
        assert!(
            report.shipped_names.contains(&name.to_string()),
            "{name} names a record's class: {:?}",
            report.shipped_names
        );
    }
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    let kept = send_to(&mut loaded, &instance, "keptPlatform");
    assert!(
        !same_object(&mut loaded, &kept, &platform),
        "the kept platform is a record of its own"
    );
    assert!(
        equal(&mut loaded, &kept, &platform),
        "the kept platform equals the loading platform"
    );
    for member in ["transcript", "collections", "time"] {
        let stored = send_to(&mut loaded, &instance, member);
        let fresh = send_to(&mut loaded, &platform, member);
        assert!(
            equal(&mut loaded, &stored, &fresh),
            "the stored {member} equals the loading platform's own"
        );
    }
    let collections = send_to(&mut loaded, &instance, "collections");
    let list_class = send_to(&mut loaded, &collections, "List");
    let list = send_to(&mut loaded, &list_class, "new");
    let size = send_to(&mut loaded, &list, "size");
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&size).as_small_integer() == Some(0));
    });
    let transcript = send_to(&mut loaded, &kept, "transcript");
    let text = loaded.interpreter.enter(|mutator| {
        let text = Value::object(mutator.new_string(String::new()));
        mutator.stash(text)
    });
    expect_ok(
        loaded
            .interpreter
            .send(&transcript, Symbol::intern("show:"), &[text]),
        "kept platform transcript show: ''",
    );
}

#[test]
fn a_stored_platform_and_a_stored_member_agree_after_a_load() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public keptPlatform = platform.
              public collections = platform collections. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, false, None);
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let kept = send_to(&mut loaded, &instance, "keptPlatform");
    let through_the_platform = send_to(&mut loaded, &kept, "collections");
    let stored = send_to(&mut loaded, &instance, "collections");
    assert!(
        same_object(&mut loaded, &through_the_platform, &stored),
        "a stored platform and a stored member answer one collections facet"
    );
}

/// The root instance's class is the module class and its `collections` slot
/// a record of a shipped class: the module class was young when the load
/// began, and the load's tenure made it and the shipped world old before
/// the graph pass named them (`a_stored_platform_and_a_stored_member_agree_
/// after_a_load`, after both collections).
#[cfg(debug_assertions)]
#[test]
fn a_loaded_graph_naming_shipped_classes_and_a_module_class_survives_both_collections() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public keptPlatform = platform.
              public collections = platform collections. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, false, None);
    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.scavenge_now();
    loaded.interpreter.collect_now();
    loaded.interpreter.scavenge_now();
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let kept = send_to(&mut loaded, &instance, "keptPlatform");
    let through_the_platform = send_to(&mut loaded, &kept, "collections");
    let stored = send_to(&mut loaded, &instance, "collections");
    assert!(
        same_object(&mut loaded, &through_the_platform, &stored),
        "a stored platform and a stored member answer one collections facet after both \
         collections"
    );
}

/// The collections facet hands out its module's classes by class accessors
/// (`public List = ( ^outer CollectionsFactory List )`), and `Time` its
/// `Stopwatch` the same way, so a class a program got from one is named by
/// its declaration, and the loading boot answers its own class for it. The
/// platform hands out none of its module's classes.
#[test]
fn a_class_the_platform_hands_out_is_named_by_its_declaration() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public listClass = platform collections List.
              public timeStopwatchClass = platform time Stopwatch. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, false, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.shipped_names,
        vec![
            "Kernel Metaclass".to_string(),
            "Kernel Object".to_string(),
            "CollectionsFactory List".to_string(),
            "TimeFactory Stopwatch".to_string()
        ],
        "the names the graph uses, in the order it first reaches them"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let time = send_to(&mut loaded, &platform, "time");
    for (member, answerer, accessor) in [
        ("listClass", &collections, "List"),
        ("timeStopwatchClass", &time, "Stopwatch"),
    ] {
        let stored = send_to(&mut loaded, &instance, member);
        let fresh = send_to(&mut loaded, answerer, accessor);
        assert!(
            same_object(&mut loaded, &stored, &fresh),
            "the stored {member} is the loading boot's {accessor}"
        );
    }
}

/// A name resolves to the very object the loading boot built, never a twin:
/// a builtin, a builtin's metaclass and a shipped class that is no builtin,
/// held in slots, decode to the loading boot's own (a level is never a
/// value; `shipped_names.rs`'s tests check that one). No program reaches
/// a builtin's metaclass, so the host stores `Integer`'s.
#[test]
fn a_shipped_name_resolves_to_the_loading_boots_own_object() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public array = Array.
              public integerClass ::= nil.
              public list = platform collections List. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let integer_metaclass = interpreter.enter(|mutator| {
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let metaclass = mutator.metaclass_of(integer).expect("Integer's metaclass");
        mutator.stash(Value::object(metaclass))
    });
    expect_ok(
        interpreter.send(
            &instance,
            Symbol::intern("integerClass:"),
            &[integer_metaclass],
        ),
        "integerClass:",
    );
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let instance = mutator.fetch(&instance);
        snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot")
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.shipped_names,
        vec![
            "Kernel Metaclass".to_string(),
            "Kernel Object".to_string(),
            "Kernel Array".to_string(),
            "Kernel Integer class".to_string(),
            "CollectionsFactory List".to_string()
        ]
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let array = send_to(&mut loaded, &instance, "array");
    let integer_class = send_to(&mut loaded, &instance, "integerClass");
    let list = send_to(&mut loaded, &instance, "list");
    loaded.interpreter.enter(|mutator| {
        let array_class = mutator.builtin_class(BuiltinClass::Array);
        let integer = mutator.builtin_class(BuiltinClass::Integer);
        let integer_metaclass = mutator.metaclass_of(integer).expect("Integer's metaclass");
        assert!(object(mutator.fetch(&array)).same(array_class));
        assert!(object(mutator.fetch(&integer_class)).same(integer_metaclass));
    });
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_list = send_to(&mut loaded, &collections, "List");
    assert!(
        same_object(&mut loaded, &list, &fresh_list),
        "the stored List is the loading boot's own"
    );
}

/// A second platform is an ordinary instance of a shipped class: written as
/// a record of `PlatformFactory Platform`, with its own lazy slots, and it
/// works after a load as it did before. No program makes one, so the host
/// stores it.
#[test]
fn a_second_platform_is_a_record_of_its_shipped_class() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public second ::= nil. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module_holding(source, "second:", |interpreter, platform| {
        let second = a_second_platform(interpreter, platform);
        expect_ok(
            interpreter.send(&second, Symbol::intern("transcript"), &[]),
            "second transcript",
        );
        second
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"PlatformFactory Platform".to_string()),
        "the second platform's class is named by its declaration: {:?}",
        report.shipped_names
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let platform = loaded.interpreter.platform();
    let second = send_to(&mut loaded, &instance, "second");
    assert!(
        !same_object(&mut loaded, &second, &platform),
        "the second platform is its own record, not the loading platform"
    );
    let second_class = class_root_of(&mut loaded, &second);
    let platform_class = class_root_of(&mut loaded, &platform);
    assert!(
        same_object(&mut loaded, &second_class, &platform_class),
        "the second platform is an instance of the loading platform's class"
    );
    let transcript = send_to(&mut loaded, &second, "transcript");
    let text = loaded.interpreter.enter(|mutator| {
        let text = Value::object(mutator.new_string(String::new()));
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
/// of `CollectionsFactory List`, with its `size_` and its `data` array, and
/// it answers as before after a load -- the class and its code are the
/// loading binary's.
#[test]
fn a_list_is_a_record_of_its_shipped_class() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public list = platform collections List new. |
            list add: 3.
            list add: 4.
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, false, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"CollectionsFactory List".to_string()),
        "the list's class is named by its declaration: {:?}",
        report.shipped_names
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let list = send_to(&mut loaded, &instance, "list");
    let size = send_to(&mut loaded, &list, "size");
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&size).as_small_integer() == Some(2));
    });
    let two = loaded.interpreter.stash(Value::small_integer(2));
    let at_two = expect_ok(
        loaded
            .interpreter
            .send(&list, Symbol::intern("at:"), &[two]),
        "list at: 2",
    );
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&at_two).as_small_integer() == Some(4));
    });
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_class = send_to(&mut loaded, &collections, "List");
    let stored_class = class_root_of(&mut loaded, &list);
    assert!(
        same_object(&mut loaded, &stored_class, &fresh_class),
        "the loaded list is an instance of the loading platform's List"
    );
}

/// A `Stopwatch` is an ordinary instance of a shipped class, as a `List`
/// is: written as a record of `TimeFactory Stopwatch`, with its started
/// reading, and it reads on after a load -- the class and its code are the
/// loading binary's. Snapshot and load run in one process, which shares the
/// clock's origin, so a start that survived the load reads a strictly
/// positive time; a stopwatch whose start came back `nil` would read 0.
#[test]
fn a_stopwatch_is_a_record_of_its_shipped_class() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public stopwatch = platform time Stopwatch new start. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module(source, false, None);
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"TimeFactory Stopwatch".to_string()),
        "the stopwatch's class is named by its declaration: {:?}",
        report.shipped_names
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let stopwatch = send_to(&mut loaded, &instance, "stopwatch");
    let elapsed = send_to(&mut loaded, &stopwatch, "elapsedNanoseconds");
    loaded.interpreter.enter(|mutator| {
        let elapsed = mutator.fetch(&elapsed);
        assert!(
            elapsed
                .as_small_integer()
                .is_some_and(|nanoseconds| nanoseconds > 0),
            "a loaded stopwatch keeps its start, so it reads a positive time, not {}",
            mutator.heap().describe(elapsed)
        );
    });
    let platform = loaded.interpreter.platform();
    let time = send_to(&mut loaded, &platform, "time");
    let fresh_class = send_to(&mut loaded, &time, "Stopwatch");
    let stored_class = class_root_of(&mut loaded, &stopwatch);
    assert!(
        same_object(&mut loaded, &stored_class, &fresh_class),
        "the loaded stopwatch is an instance of the loading platform's Stopwatch"
    );
}

/// A second collections facet is an ordinary record of
/// `CollectionsFactory Collections`, and after a load it is equal to the
/// loading platform's facet: a value. No program makes one, so the host
/// stores a second platform's.
#[test]
fn a_second_collections_facet_is_a_record_of_its_shipped_class() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public facet ::= nil. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module_holding(source, "facet:", |interpreter, platform| {
        let second = a_second_platform(interpreter, platform);
        expect_ok(
            interpreter.send(&second, Symbol::intern("collections"), &[]),
            "second collections",
        )
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"CollectionsFactory Collections".to_string()),
        "the facet's class is named by its declaration: {:?}",
        report.shipped_names
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
    assert!(
        equal(&mut loaded, &facet, &collections),
        "the loaded facet equals the loading platform's facet"
    );
}

/// A second typed arrays facet is an ordinary record of
/// `TypedArraysFactory TypedArrays`, its enclosing object the module
/// instance, and after a load it is equal to the loading platform's facet:
/// a value. No program makes one, so the host stores a second platform's.
#[test]
fn a_second_typed_arrays_facet_is_a_record_of_its_shipped_class() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public facet ::= nil. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module_holding(source, "facet:", |interpreter, platform| {
        let second = a_second_platform(interpreter, platform);
        expect_ok(
            interpreter.send(&second, Symbol::intern("typedArrays"), &[]),
            "second typedArrays",
        )
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"TypedArraysFactory TypedArrays".to_string()),
        "the facet's class is named by its declaration: {:?}",
        report.shipped_names
    );
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let facet = send_to(&mut loaded, &instance, "facet");
    let int16_array = send_to(&mut loaded, &facet, "Int16Array");
    let platform = loaded.interpreter.platform();
    let typed_arrays = send_to(&mut loaded, &platform, "typedArrays");
    let fresh_int16_array = send_to(&mut loaded, &typed_arrays, "Int16Array");
    assert!(
        same_object(&mut loaded, &int16_array, &fresh_int16_array),
        "the loaded facet hands out the loading platform's Int16Array"
    );
    assert!(
        equal(&mut loaded, &facet, &typed_arrays),
        "the loaded facet equals the loading platform's facet"
    );
    loaded.interpreter.enter(|mutator| {
        let module = ShippedName {
            module: Symbol::intern("TypedArraysFactory"),
            declaration: Symbol::intern("TypedArraysFactory"),
            kind: ShippedKind::ModuleInstance,
        };
        let Ok(resolved) = image::shipped_names::resolve_in(mutator, &[module], false) else {
            panic!("the typed arrays module is a shipped name");
        };
        let heap = mutator.heap();
        let facet = object(mutator.fetch(&facet));
        let enclosing = heap
            .class_enclosing(heap.object_class(facet))
            .expect("the facet's class is nested");
        assert!(
            enclosing.same(resolved[0].object),
            "the loaded facet is enclosed by the loading boot's module instance"
        );
    });
}

/// A module of two classes, for the tests of what a decoded mixin knows
/// of its enclosing mixin and its module's text.
const TWO_CLASS_MODULE: &str = indoc! {"
    class Outer usingPlatform: platform = Object new () (
      class Inner = Object new () ()
    ) : ()
"};

/// The root mixin of a loaded image's one module, and its `Inner`.
fn outer_and_inner(loaded: &mut image::LoadedImage) -> (Rc<Mixin>, Rc<Mixin>) {
    let outer = loaded.modules[0].declaration.clone();
    let inner = loaded
        .interpreter
        .enter(|mutator| mutator.heap().nested_mixins(&outer))
        .into_iter()
        .find(|nested| nested.name.as_str() == "Inner")
        .expect("Outer declares Inner");
    (outer, inner)
}

/// An image with source gives every decoded mixin of a module its
/// declaration's text, cut from the module's -- the root's the whole
/// module with `Inner` cut out, `Inner`'s its own -- and each nested
/// mixin its enclosing one.
// spec: image.texts
#[test]
fn a_mixin_decoded_from_an_image_with_source_carries_its_text_and_links() {
    let bytes = snapshot_module(TWO_CLASS_MODULE, true, None);
    let mut loaded = image::load(&bytes).expect("load");
    let (outer, inner) = outer_and_inner(&mut loaded);
    let outer_text = outer.text.as_ref().expect("the image carries source");
    let inner_text = inner.text.as_ref().expect("the image carries source");
    let inner_declaration = "class Inner = Object new () ()";
    assert_eq!(inner_text.cut_text(), inner_declaration);
    assert_eq!(
        outer_text
            .assemble(&[inner_declaration.to_string()])
            .as_deref(),
        Some(TWO_CLASS_MODULE)
    );
    assert!(
        inner
            .enclosing_mixin()
            .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, &outer))
    );
    assert!(outer.enclosing_mixin().is_none());
}

/// An image without source decodes mixins with no text, linked to their
/// enclosing ones all the same.
#[test]
fn a_mixin_decoded_from_an_image_without_source_carries_no_text() {
    let bytes = snapshot_module(TWO_CLASS_MODULE, false, None);
    let mut loaded = image::load(&bytes).expect("load");
    let (outer, inner) = outer_and_inner(&mut loaded);
    assert!(outer.text.is_none());
    assert!(inner.text.is_none());
    assert!(
        inner
            .enclosing_mixin()
            .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, &outer))
    );
}

/// An image holding a `List` loads into a fresh process, whose boot built
/// no collections module: the load builds `CollectionsFactory` and its
/// `List` before the graph, and the loaded list is an instance of the
/// `List` the loading world's module instance answers. It builds only
/// what the image names, so `Map`, which the program never used, is not
/// built.
#[test]
fn a_kept_list_loads_into_a_world_that_builds_its_module_on_load() {
    let source = indoc! {"
        class Holder usingPlatform: platform = Object new (
            | public list ::= nil. |
        ) (
        ) : ()
    "};
    let bytes = snapshot_module_holding(source, "list:", |interpreter, platform| {
        let collections = expect_ok(
            interpreter.send(platform, Symbol::intern("collections"), &[]),
            "platform collections",
        );
        let list_class = expect_ok(
            interpreter.send(&collections, Symbol::intern("List"), &[]),
            "platform collections List",
        );
        expect_ok(
            interpreter.send(&list_class, Symbol::NEW, &[]),
            "platform collections List new",
        )
    });
    let mut loaded = image::load(&bytes).expect("load");
    let instance = loaded.roots.first().cloned().expect("the root instance");
    let list = send_to(&mut loaded, &instance, "list");
    let stored_class = class_root_of(&mut loaded, &list);
    let platform = loaded.interpreter.platform();
    let collections = send_to(&mut loaded, &platform, "collections");
    let fresh_class = send_to(&mut loaded, &collections, "List");
    assert!(
        same_object(&mut loaded, &stored_class, &fresh_class),
        "the loaded list is an instance of the loading platform's List"
    );
    loaded.interpreter.enter(|mutator| {
        let name = |declaration: &str, kind| ShippedName {
            module: Symbol::intern("CollectionsFactory"),
            declaration: Symbol::intern(declaration),
            kind,
        };
        let Ok(resolved) = image::shipped_names::resolve_in(
            mutator,
            &[
                name("CollectionsFactory", ShippedKind::ModuleInstance),
                name("List", ShippedKind::Class),
            ],
            false,
        ) else {
            panic!("the load built the collections module and its List");
        };
        let heap = mutator.heap();
        let list_class = object(mutator.fetch(&stored_class));
        assert!(list_class.same(resolved[1].object));
        assert!(
            heap.class_enclosing(list_class)
                .is_some_and(|enclosing| enclosing.same(resolved[0].object)),
            "the loaded List is the loading world's module instance's"
        );
        assert!(
            image::shipped_names::resolve_in(mutator, &[name("Map", ShippedKind::Class)], false)
                .is_err(),
            "the load builds only the classes the image names"
        );
    });
}

/// A crafted section entry: a module, a declaration and a kind byte, and a
/// level's clause index.
type CraftedEntry = ((&'static str, &'static str, u8), Option<u32>);

/// Every malformed section is refused on load, before the graph: a count
/// past the maximum or past the bytes left, an unknown module or
/// declaration, an unknown kind byte, a level past its declaration's
/// clauses, and a name given twice.
// spec: image.verify.counts
// spec: image.limit.shipped-names
// spec: image.verify.shipped-names
#[test]
fn a_malformed_shipped_name_section_is_refused() {
    let bytes = snapshot_of(EMPTY);
    let class = |name: (&'static str, &'static str)| (name.0, name.1, 1u8);
    let entries = |listed: Vec<CraftedEntry>| {
        with_shipped_name_section(&bytes, |intern, writer| {
            writer.write_u32(u32::try_from(listed.len()).expect("a count fits a u32"));
            for (entry, clause) in listed {
                write_shipped_entry(writer, intern, entry, clause);
            }
        })
    };
    let cases: Vec<(Vec<u8>, decode::DecodeError)> = vec![
        (
            with_shipped_name_section(&bytes, |_, writer| {
                writer.write_u32(
                    u32::try_from(shipped_names::MAX_SHIPPED_NAMES + 1).expect("fits a u32"),
                );
            }),
            decode::DecodeError::TooManyShippedNames {
                count: shipped_names::MAX_SHIPPED_NAMES + 1,
            },
        ),
        (
            with_shipped_name_section(&bytes, |_, writer| writer.write_u32(1000)),
            decode::DecodeError::UnexpectedEndOfFile,
        ),
        (
            entries(vec![(class(("Nothing", "Array")), None)]),
            decode::DecodeError::UnknownShippedName("Nothing Array".to_string()),
        ),
        (
            entries(vec![(class(("Kernel", "Nothing")), None)]),
            decode::DecodeError::UnknownShippedName("Kernel Nothing".to_string()),
        ),
        (
            entries(vec![(("Kernel", "Array", 5), None)]),
            decode::DecodeError::InvalidTag {
                what: "shipped name kind",
                tag: 5,
            },
        ),
        (
            entries(vec![(("Kernel", "Kernel", 3), Some(1))]),
            decode::DecodeError::UnknownShippedName("Kernel Kernel level 1".to_string()),
        ),
        (
            entries(vec![
                (class(("Kernel", "Array")), None),
                (class(("Kernel", "Array")), None),
            ]),
            decode::DecodeError::DuplicateShippedName("Kernel Array".to_string()),
        ),
    ];
    for (crafted, expected) in cases {
        assert_eq!(load_error(&crafted), expected.to_string());
    }
}

/// A level's clause index is held to the clause limit where the section is
/// read, before any shipped world is asked: `inspect`, which asks none,
/// refuses index 1,024 as the load does.
// spec: image.limit.clause-index
#[test]
fn a_shipped_level_past_the_clause_limit_is_refused() {
    let bytes = snapshot_of(EMPTY);
    let crafted = with_shipped_name_section(&bytes, |intern, writer| {
        writer.write_u32(1);
        write_shipped_entry(writer, intern, ("Kernel", "Array", 3), Some(1024));
    });
    let expected =
        decode::DecodeError::UnknownShippedName("Kernel Array level 1024".to_string()).to_string();
    assert_eq!(load_error(&crafted), expected);
    match image::inspect::inspect(&crafted) {
        Ok(_) => panic!("inspect read a level past the clause limit"),
        Err(error) => assert_eq!(error, expected),
    }
}

/// A module instance is named by its module's root, its declaration the
/// module's own name: one named by another declaration names nothing.
// spec: image.verify.shipped-names
#[test]
fn a_module_instance_named_by_another_declaration_is_refused() {
    let bytes = snapshot_of(EMPTY);
    let crafted = with_shipped_name_section(&bytes, |intern, writer| {
        writer.write_u32(1);
        write_shipped_entry(writer, intern, ("Kernel", "Array", 0), None);
    });
    assert_eq!(
        load_error(&crafted),
        decode::DecodeError::UnknownShippedName("Kernel instance".to_string()).to_string()
    );
}

/// A reference past the section is refused before it indexes anything.
// spec: image.verify.ids
#[test]
fn an_index_past_the_shipped_name_section_is_refused() {
    let bytes = snapshot_module(PLATFORM_MEMBERS, false, None);
    let crafted = with_shipped_name_section(&bytes, |_, writer| writer.write_u32(0));
    assert_eq!(
        load_error(&crafted),
        decode::DecodeError::BadShippedName(1).to_string()
    );
}

/// The decoder holds each forbidden position to the rule: `Kernel Top` and
/// `Kernel Activation` as a value, a module's root as a record's class, the
/// kernel's instance as a slot's value, and a class standing where only a
/// module instance may. A module instance in a frame's register decodes.
// spec: image.shipped.positions
#[test]
fn a_shipped_name_in_a_forbidden_position_is_refused() {
    let not_allowed =
        |name: &str, position: &'static str| decode::DecodeError::ShippedNameNotAllowed {
            name: name.to_string(),
            position,
        };
    let mut fixture = GraphFixture::new();
    assert_eq!(
        fixture.decode_error(&shipped_class_value(TOP_NAME)),
        not_allowed("Kernel Top", "a value")
    );
    assert_eq!(
        fixture.decode_error(&shipped_class_value(ACTIVATION_NAME)),
        not_allowed("Kernel Activation", "a value")
    );
    let under_the_root = class_records(1, |writer| {
        writer.write_shipped(KERNEL_ROOT_NAME);
        writer.write_u32(5); // identity hash
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0); // no fields
    });
    assert_eq!(
        fixture.decode_error(&under_the_root),
        not_allowed("Kernel Kernel", "a record's class")
    );
    let kernel_in_a_slot = one_array_object(|writer| {
        writer.write_u8(VALUE_SHIPPED_MODULE);
        writer.write_u32(KERNEL_INSTANCE_NAME);
    });
    assert_eq!(
        fixture.decode_error(&kernel_in_a_slot),
        not_allowed("Kernel instance", "a value")
    );
    let frame_receiving = |receiver: u32| {
        let mut writer = encode::Writer::new();
        graph_counts(&mut writer, 1);
        frame_header(&mut writer, 0);
        writer.write_u8(FRAME_TOP_LEVEL);
        no_body(&mut writer);
        writer.write_u32(0); // program counter
        writer.write_boolean(true); // uncontinuable
        writer.write_u8(VALUE_SHIPPED_MODULE); // receiver: a shipped module
        writer.write_u32(receiver);
        writer.write_boolean(false); // no sender
        writer.write_boolean(false); // no home
        writer.write_boolean(false); // no current class
        writer.write_u32(0); // no registers
        writer.write_u32(0); // no roots
        writer.into_bytes()
    };
    assert_eq!(
        fixture.decode_error(&frame_receiving(ARRAY_NAME)),
        not_allowed("Kernel Array", "a frame's register")
    );
    fixture.decoded(
        &frame_receiving(KERNEL_INSTANCE_NAME),
        "a frame of shipped code receiving the kernel",
        |heap, graph| {
            let receiver = heap.frame_receiver(graph.frames[0]);
            let Some(receiver) = receiver.as_object() else {
                panic!("the frame's receiver is an object");
            };
            assert_eq!(
                heap.class_display_name(heap.object_class(receiver)),
                "Kernel"
            );
        },
    );
}

/// No object is an instance of `Top`, so a record of it is refused as `Top`
/// is refused as a value.
// spec: image.shipped.positions
#[test]
fn a_record_of_top_is_refused() {
    let bytes = class_records(1, |writer| {
        writer.write_shipped(TOP_NAME);
        writer.write_u32(5); // identity hash
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(0); // no fields
    });
    assert_eq!(
        GraphFixture::new().decode_error(&bytes),
        decode::DecodeError::ShippedNameNotAllowed {
            name: "Kernel Top".to_string(),
            position: "a record's class",
        }
    );
}

/// A shipped class that is no builtin has instance records only: an array,
/// a string or a frame record under `TimeFactory Stopwatch` is refused
/// before anything is allocated.
#[test]
fn a_record_of_a_shipped_class_with_another_body_is_refused() {
    for body in [
        OBJECT_ARRAY,
        OBJECT_STRING,
        OBJECT_FRAME,
        OBJECT_BYTE_ARRAY,
        OBJECT_WEAK_ARRAY,
        OBJECT_EPHEMERON,
    ] {
        let bytes = class_records(1, |writer| {
            writer.write_shipped(STOPWATCH_NAME);
            writer.write_u32(5); // identity hash
            writer.write_u8(body);
            // No elements, an empty text, a program counter, no bytes or
            // no fields.
            writer.write_u32(0);
        });
        let error = GraphFixture::new().decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::ShippedInstanceBody { body });
        assert_eq!(
            error.to_string(),
            format!(
                "a record of a shipped class has object body {body}, but an instance of that \
                 class is an instance record"
            )
        );
    }
}

/// A record of a shipped class that is no builtin carries exactly the
/// fields the loading binary lays the class out with: `Stopwatch` declares
/// two slots.
#[test]
fn a_record_of_a_shipped_class_carries_exactly_its_layout() {
    let stopwatch_with = |fields: u32| {
        class_records(1, |writer| {
            writer.write_shipped(STOPWATCH_NAME);
            writer.write_u32(5); // identity hash
            writer.write_u8(OBJECT_INSTANCE);
            writer.write_u32(fields);
            for _ in 0..fields {
                writer.write_u8(VALUE_NIL);
            }
        })
    };
    let mut fixture = GraphFixture::new();
    fixture.decoded(&stopwatch_with(2), "a stopwatch record", |_, _| {});
    let error = fixture.decode_error(&stopwatch_with(3));
    assert_eq!(
        error,
        decode::DecodeError::ShippedInstanceFieldCount {
            expected: 2,
            found: 3
        }
    );
    assert_eq!(
        error.to_string(),
        "an instance of a shipped class carries 3 field(s), but the loading binary lays that \
         class out with 2"
    );
}

/// A program class whose instances have two slots.
const TWO_SLOTS: &str = indoc! {"
    class Holder = Object new (
      |
        first = 1.
        second = 2.
      |
    ) () : ()
"};

/// A graph section of `Holder`'s class record (object 0), its metaclass
/// record (object 1) and an object of `Holder` (object 2), its body
/// written by `body`.
fn a_holder_object(body: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    class_records(3, |writer| {
        class_record(writer, 1, 0, no_class_references);
        metaclass_record(writer, 0);
        writer.write_u8(CLASS_RECORD); // its class: `Holder`'s record
        writer.write_u32(0);
        writer.write_u32(5); // identity hash
        body(writer);
    })
}

/// An instance record of `fields` nil fields.
fn instance_body(fields: u32) -> impl FnOnce(&mut encode::Writer) {
    move |writer| {
        writer.write_u8(OBJECT_INSTANCE);
        writer.write_u32(fields);
        for _ in 0..fields {
            writer.write_u8(VALUE_NIL);
        }
    }
}

/// An instance of a program class carries exactly its application's
/// slots, as every one is allocated: a customized copy reads them at
/// fixed fields with no check, past the end of an object with fewer.
#[test]
fn a_record_of_a_program_class_carries_exactly_its_layout() {
    let mut fixture = GraphFixture::with_source(TWO_SLOTS);
    fixture.decoded(
        &a_holder_object(instance_body(2)),
        "a holder record",
        |_, _| {},
    );
    for fields in [1, 3] {
        let error = fixture.decode_error(&a_holder_object(instance_body(fields)));
        assert_eq!(
            error,
            decode::DecodeError::ProgramInstanceFieldCount {
                expected: 2,
                found: fields as usize
            }
        );
        assert_eq!(
            error.to_string(),
            format!(
                "an instance of a program class carries {fields} field(s), but its class lays \
                 its instances out with 2"
            )
        );
    }
}

/// An object of a program class is an instance record: an array or a
/// string record under `Holder`'s header is refused.
#[test]
fn a_record_of_a_program_class_with_another_body_is_refused() {
    let array = |writer: &mut encode::Writer| {
        writer.write_u8(OBJECT_ARRAY);
        writer.write_u32(2);
        writer.write_u8(VALUE_NIL);
        writer.write_u8(VALUE_NIL);
    };
    let string = |writer: &mut encode::Writer| {
        writer.write_u8(OBJECT_STRING);
        writer.write_str("two");
    };
    let mut fixture = GraphFixture::with_source(TWO_SLOTS);
    for bytes in [a_holder_object(array), a_holder_object(string)] {
        let error = fixture.decode_error(&bytes);
        assert_eq!(error, decode::DecodeError::ProgramInstanceBody);
        assert_eq!(
            error.to_string(),
            "an object of a program class is not an instance record, but every instance of \
             that class is one"
        );
    }
}

/// A module instance holds the core library's state, as the platform holds
/// the mirror, so the encoder names one only where a frame of shipped code
/// holds it and refuses it anywhere else; a module's root class is no value
/// either. The kernel instance and its class are both reached here; the
/// collections factory is the second such module, reached only because a
/// host can walk a class's enclosing object, which no program can.
// spec: image.shipped.positions
#[test]
fn a_module_instance_outside_a_frame_is_refused() {
    let (mut interpreter, class) = load(EMPTY);
    let platform = interpreter.platform();
    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let object = mutator.builtin_class(BuiltinClass::Object);
        let kernel = mutator
            .heap()
            .class_enclosing(object)
            .expect("the kernel instance encloses Object");
        let kernel_class = mutator.heap().object_class(kernel);
        let collections_class = mutator.class_of(mutator.fetch(&collections));
        let collections_factory = mutator
            .heap()
            .class_enclosing(collections_class)
            .expect("the collections factory instance encloses the facet class");
        let collections_factory_class = mutator.heap().object_class(collections_factory);
        for (shipped, expected) in [
            (
                kernel,
                "image encoding refuses the module instance Kernel outside a frame: it holds \
                 the core library's state",
            ),
            (
                kernel_class,
                "image encoding refuses Kernel Kernel as a value: no program may name it there, \
                 and the loader refuses an image that does",
            ),
            (
                collections_factory,
                "image encoding refuses the module instance CollectionsFactory outside a frame: \
                 it holds the core library's state",
            ),
            (
                collections_factory_class,
                "image encoding refuses CollectionsFactory CollectionsFactory as a value: no \
                 program may name it there, and the loader refuses an image that does",
            ),
        ] {
            let refused = snapshot_one(
                mutator,
                false,
                "t.ns",
                root_class,
                &[Value::object(shipped)],
            );
            let Err(message) = refused else {
                panic!("a graph reaching {expected} was encoded");
            };
            assert_eq!(message, expected);
        }
    });
}

/// A graph section of no objects whose one root `root` writes.
fn a_graph_of_one_root(root: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 0);
    writer.write_u32(1);
    root(&mut writer);
    writer.into_bytes()
}

/// A shipped class, and a shipped metaclass, as a value decode to the class
/// the name names.
#[test]
fn a_value_naming_a_shipped_class_decodes_to_that_class() {
    let mut fixture = GraphFixture::new();
    let stopwatch = a_graph_of_one_root(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_shipped(STOPWATCH_NAME);
    });
    fixture.decoded(&stopwatch, "a shipped class as a value", |heap, graph| {
        let class = object(graph.roots[0]);
        assert!(heap.is_class(class));
        assert_eq!(heap.class_display_name(class), "Stopwatch");
    });
    let integer_class = a_graph_of_one_root(|writer| {
        writer.write_u8(VALUE_CLASS_TAG);
        writer.write_shipped(INTEGER_CLASS_NAME);
    });
    fixture.decoded(&integer_class, "a shipped metaclass", |heap, graph| {
        let metaclass = object(graph.roots[0]);
        let this_class = heap
            .class_this_class(metaclass)
            .expect("the root is a metaclass");
        assert_eq!(heap.class_builtin(this_class), Some(BuiltinClass::Integer));
    });
}

/// Each object of the shipped world has one entry in the hash table and
/// one name in the section, however often the graph reaches it: `Array` and
/// its metaclass, each held as a value and each given a hash.
#[test]
fn an_object_reached_twice_is_listed_once() {
    let (mut interpreter, class) = load(EMPTY);
    let (bytes, class_hash, metaclass_hash) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let array = mutator.builtin_class(BuiltinClass::Array);
        let metaclass = mutator
            .heap()
            .class_metaclass(array)
            .expect("Array has a metaclass");
        let class_hash = mutator.heap_mut().object_ensure_identity_hash(array);
        let metaclass_hash = mutator.heap_mut().object_ensure_identity_hash(metaclass);
        let held = Value::object(mutator.new_array(vec![
            Value::object(array),
            Value::object(metaclass),
            Value::object(array),
        ]));
        let bytes = snapshot_one(mutator, false, "t.ns", class, &[held]).expect("snapshot");
        (bytes, class_hash, metaclass_hash)
    });
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.shipped_names,
        vec![
            "Kernel Array".to_string(),
            "Kernel Array class".to_string(),
            "Kernel Metaclass".to_string(),
            "Kernel Object".to_string(),
        ]
    );
    assert_eq!(
        report.graph_tag_named_hashes,
        vec![
            format!("Kernel Array hash {class_hash}"),
            format!("Kernel Array class hash {metaclass_hash}"),
        ]
    );
    let loaded = image::load(&bytes).expect("load");
    let mut interpreter = loaded.interpreter;
    interpreter.enter(|mutator| {
        let array = mutator.builtin_class(BuiltinClass::Array);
        let metaclass = mutator.metaclass_of(array).expect("Array has a metaclass");
        assert_eq!(mutator.heap().object_identity_hash(array), class_hash);
        assert_eq!(
            mutator.heap().object_identity_hash(metaclass),
            metaclass_hash
        );
    });
}

#[test]
fn a_packaged_application_loads_as_a_started_image_with_one_root() {
    let bytes = packaged_application();
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert!(loaded.start);
    assert_eq!(loaded.roots.len(), 1);
    let root = loaded.roots[0].clone();
    let count = send_to(&mut loaded, &root, "count");
    loaded.interpreter.enter(|mutator| {
        assert!(mutator.fetch(&count).as_small_integer() == Some(41));
    });
}

fn packaged_application() -> Vec<u8> {
    let (mut interpreter, class) = load(indoc! {"
        class App = Object new (| public count ::= 41. |) (
            public main: platform args: arguments = ( count:: count + 1. ^count )
        ) : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let identity = ModuleIdentity::undeclared("App.ns", "App");
    interpreter
        .enter(|mutator| {
            let modules = [image::ModuleSource {
                class: class_reference(mutator, &class),
                identity: &identity,
                with_source: false,
            }];
            image::package(mutator, &modules, mutator.fetch(&application))
        })
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn package_refuses_an_application_that_reaches_the_platform() {
    let (mut interpreter, class) = load(indoc! {"
        class App = Object new (| public held ::= nil. |) () : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let platform = interpreter.platform();
    expect_ok(
        interpreter.send(&application, Symbol::intern("held:"), &[platform]),
        "held:",
    );
    let identity = ModuleIdentity::undeclared("App.ns", "App");
    let refused = interpreter.enter(|mutator| {
        let modules = [image::ModuleSource {
            class: class_reference(mutator, &class),
            identity: &identity,
            with_source: false,
        }];
        image::package(mutator, &modules, mutator.fetch(&application))
    });
    let error = match refused {
        Ok(_) => panic!("an application holding the platform was packaged"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        "image encoding refuses PlatformFactory Platform in a started application: nothing \
         before main:args: reaches it"
    );
}

/// A started image names only the kernel: an application packaged holding
/// `Array` loads, and the same image with its one name rewritten to
/// `PlatformFactory Platform` -- which no encoder writes into a started
/// image -- is refused before its graph is read.
// spec: image.verify.started-image-names
#[test]
fn a_started_image_naming_another_module_is_refused() {
    let (mut interpreter, class) = load(indoc! {"
        class App = Object new (| public held = Array. |) (
            public main: platform args: arguments = ( ^held )
        ) : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let identity = ModuleIdentity::undeclared("App.ns", "App");
    let bytes = interpreter
        .enter(|mutator| {
            let modules = [image::ModuleSource {
                class: class_reference(mutator, &class),
                identity: &identity,
                with_source: false,
            }];
            image::package(mutator, &modules, mutator.fetch(&application))
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.shipped_names,
        vec![
            "Kernel Metaclass".to_string(),
            "Kernel Object".to_string(),
            "Kernel Array".to_string(),
        ]
    );
    image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let crafted = with_shipped_name_section(&bytes, |intern, writer| {
        writer.write_u32(1);
        write_shipped_entry(writer, intern, ("PlatformFactory", "Platform", 1), None);
    });
    assert_eq!(
        load_error(&crafted),
        decode::DecodeError::ShippedNameInStartedImage("PlatformFactory Platform".to_string())
            .to_string()
    );
}

/// A started image names no module instance, not even the kernel's: a frame
/// it holds through a block in a slot would hand the instance to the
/// program, and no program reaches a shipped root's instance. The packaged
/// image with its section rewritten to name `Kernel instance` -- which no
/// encoder writes into a started image -- is refused before its graph is
/// read.
// spec: image.verify.started-image-names
#[test]
fn a_started_image_names_no_module_instance() {
    let bytes = packaged_application();
    image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let crafted = with_shipped_name_section(&bytes, |intern, writer| {
        writer.write_u32(1);
        write_shipped_entry(writer, intern, ("Kernel", "Kernel", 0), None);
    });
    assert_eq!(
        load_error(&crafted),
        decode::DecodeError::ShippedNameInStartedImage("Kernel instance".to_string()).to_string()
    );
}

/// A program that catches the error a non-Boolean loop condition signals
/// and keeps the error itself builds and loads: the error is a record of
/// the private `Kernel NonBooleanReceiver`, which a record's class may be,
/// and the loaded slot holds an instance of the loading boot's own class.
#[test]
fn a_caught_non_boolean_receiver_round_trips_through_a_package() {
    let (mut interpreter, class) = load(indoc! {"
        class App = Object new (
          | public held = [[4] whileFalse: [nil]] on: Error do: [:error | error]. |
        ) (
          public main: platform args: arguments = ( ^held )
        ) : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let identity = ModuleIdentity::undeclared("App.ns", "App");
    let bytes = interpreter
        .enter(|mutator| {
            let modules = [image::ModuleSource {
                class: class_reference(mutator, &class),
                identity: &identity,
                with_source: false,
            }];
            image::package(mutator, &modules, mutator.fetch(&application))
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert!(
        report
            .shipped_names
            .contains(&"Kernel NonBooleanReceiver".to_string())
    );
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let held = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("held"), &[]),
        "held",
    );
    loaded.interpreter.enter(|mutator| {
        let held = mutator.fetch(&held);
        let signalled = mutator.builtin_class(BuiltinClass::NonBooleanReceiver);
        assert!(mutator.class_of(held).same(signalled));
    });
}

/// No program reaches the private `Kernel NonBooleanReceiver` as a value:
/// `class` is protected, so a caught instance does not hand it out. A
/// crafted image naming it as a value is refused on load, and the encoder
/// refuses a graph holding it as one rather than write an image nothing
/// loads.
// spec: image.shipped.positions
#[test]
fn non_boolean_receiver_as_a_value_is_refused() {
    assert_eq!(
        GraphFixture::new().decode_error(&shipped_class_value(NON_BOOLEAN_RECEIVER_NAME)),
        decode::DecodeError::ShippedNameNotAllowed {
            name: "Kernel NonBooleanReceiver".to_string(),
            position: "a value",
        }
    );
    let (mut interpreter, class) = load(indoc! {"
        class Root = Object new () ()
    "});
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let signalled = mutator.builtin_class(BuiltinClass::NonBooleanReceiver);
        let refused = snapshot_one(
            mutator,
            false,
            "t.ns",
            root_class,
            &[Value::object(signalled)],
        );
        let Err(message) = refused else {
            panic!("a graph holding Kernel NonBooleanReceiver as a value was encoded");
        };
        assert_eq!(
            message,
            "image encoding refuses Kernel NonBooleanReceiver as a value: no program may name \
             it there, and the loader refuses an image that does"
        );
    });
}

/// `Kernel IndexedCollection` is public only so that `TypedArraysFactory.ns`
/// may apply it, and no facet hands it out, so no program reaches it by
/// running: a crafted image naming it as a value, as a class record's
/// superclass or as the mixin a level applies is refused on load, and the
/// encoder refuses a graph holding it as a value rather than write an image
/// nothing loads.
// spec: image.shipped.positions
#[test]
fn indexed_collection_as_a_value_or_a_superclass_is_refused() {
    let not_allowed = |position: &'static str| decode::DecodeError::ShippedNameNotAllowed {
        name: "Kernel IndexedCollection".to_string(),
        position,
    };
    assert_eq!(
        GraphFixture::new().decode_error(&shipped_class_value(INDEXED_COLLECTION_NAME)),
        not_allowed("a value")
    );
    let over_indexed_collection = one_class_object(0, |writer| {
        writer.write_boolean(true);
        writer.write_shipped(INDEXED_COLLECTION_NAME); // superclass
        writer.write_boolean(false); // no enclosing object
        writer.write_boolean(false); // no declaring class
    });
    assert_eq!(
        GraphFixture::new().decode_error(&over_indexed_collection),
        not_allowed("a superclass or an applied mixin")
    );
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let applied = row_named(&fixture, "Applied");
    assert_eq!(
        fixture.decode_error(&shipped_mixin_record(INDEXED_COLLECTION_NAME, (applied, 0))),
        not_allowed("a superclass or an applied mixin")
    );
    let (mut interpreter, class) = load(indoc! {"
        class Root = Object new () ()
    "});
    interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &class);
        let name = ShippedName {
            module: Symbol::intern("Kernel"),
            declaration: Symbol::intern("IndexedCollection"),
            kind: ShippedKind::Class,
        };
        let Ok(resolved) = image::shipped_names::resolve_in(mutator, &[name], false) else {
            panic!("Kernel IndexedCollection is a shipped name");
        };
        let refused = snapshot_one(
            mutator,
            false,
            "t.ns",
            root_class,
            &[Value::object(resolved[0].object)],
        );
        let Err(message) = refused else {
            panic!("a graph holding Kernel IndexedCollection as a value was encoded");
        };
        assert_eq!(
            message,
            "image encoding refuses Kernel IndexedCollection as a value: no program may name \
             it there, and the loader refuses an image that does"
        );
    });
}

// spec: image.header.flags
#[test]
fn a_retired_stateless_flag_is_refused_as_unknown() {
    use ns_runtime::image::decode::{DecodeError, Reader, read_header};
    use ns_runtime::image::encode::{RETIRED_HEADER_FLAGS, Writer, write_header};
    const STATELESS: u8 = 2;
    assert_ne!(RETIRED_HEADER_FLAGS & STATELESS, 0);
    let mut writer = Writer::new();
    write_header(&mut writer, STATELESS);
    let bytes = writer.into_bytes();
    let error = match read_header(&mut Reader::new(&bytes)) {
        Ok(_) => panic!("the retired flag was accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        DecodeError::UnknownHeaderFlags { flags: 2 }.to_string()
    );
}

#[test]
fn a_snapshot_is_not_started() {
    let bytes = snapshot_of(indoc! {"
        class App = Object new () () : ()
    "});
    let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert!(!loaded.start);
}

#[test]
fn a_start_image_with_other_than_one_root_is_refused() {
    let (mut interpreter, class) = load(ENTRY_SOURCE);
    let first = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let second = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let flag_offset = encode::MAGIC.len() + std::mem::size_of::<u32>();
    for root_count in [0, 2] {
        let mut bytes = interpreter.enter(|mutator| {
            let class = class_reference(mutator, &class);
            let roots = [mutator.fetch(&first), mutator.fetch(&second)];
            snapshot_one(mutator, false, "t.ns", class, &roots[..root_count]).expect("snapshot")
        });
        bytes[flag_offset] |= encode::FLAG_START;
        let error = match image::load(&bytes) {
            Ok(_) => panic!("a start image with {root_count} roots loaded"),
            Err(error) => error,
        };
        assert!(
            error.contains("exactly one root") && error.contains(&root_count.to_string()),
            "{error}"
        );
    }
}

/// A shipped name as a test writes it.
fn shipped_name(module: &str, declaration: &str, kind: ShippedKind) -> ShippedName {
    ShippedName {
        module: Symbol::intern(module),
        declaration: Symbol::intern(declaration),
        kind,
    }
}

/// The builder refuses an unknown module, a declaration its module's root
/// does not declare, a level past its declaration's clauses, a name given
/// twice and, in a started image, a name of any module but the kernel, with
/// the error the loader gives, and refuses before it builds
/// anything: the shipped world names as many objects afterwards as before,
/// even when a name the builder would build comes first.
// spec: image.verify.shipped-names
#[test]
fn building_the_shipped_names_refuses_a_name_before_it_builds_anything() {
    let list = shipped_name("CollectionsFactory", "List", ShippedKind::Class);
    let cases = [
        (
            vec![list, shipped_name("Nothing", "Array", ShippedKind::Class)],
            false,
            decode::DecodeError::UnknownShippedName("Nothing Array".to_string()),
        ),
        (
            vec![
                list,
                shipped_name("CollectionsFactory", "Nothing", ShippedKind::Class),
            ],
            false,
            decode::DecodeError::UnknownShippedName("CollectionsFactory Nothing".to_string()),
        ),
        (
            vec![
                list,
                shipped_name("CollectionsFactory", "List", ShippedKind::Level(7)),
            ],
            false,
            decode::DecodeError::UnknownShippedName("CollectionsFactory List level 7".to_string()),
        ),
        (
            vec![
                list,
                shipped_name("Kernel", "Array", ShippedKind::Class),
                shipped_name("Kernel", "Array", ShippedKind::Class),
            ],
            false,
            decode::DecodeError::DuplicateShippedName("Kernel Array".to_string()),
        ),
        (
            vec![shipped_name("Kernel", "Array", ShippedKind::Class), list],
            true,
            decode::DecodeError::ShippedNameInStartedImage("CollectionsFactory List".to_string()),
        ),
    ];
    for (names, started, expected) in cases {
        let mut interpreter = Interpreter::bootstrap();
        let before = interpreter.enter(|mutator| mutator.shipped_name_count());
        match interpreter.build_shipped_names(&names, started) {
            Ok(()) => panic!("expected the names to be refused"),
            Err(error) => assert_eq!(error, expected),
        }
        let after = interpreter.enter(|mutator| mutator.shipped_name_count());
        assert_eq!(after, before, "a refused name built nothing");
    }
}

/// Names of a world already built are accepted, and building them again
/// builds nothing: each module and class is built at most once. The boot
/// builds neither module named here, so the first build of the names builds
/// them, and the second finds them built.
#[test]
fn building_the_shipped_names_of_a_built_world_builds_nothing() {
    let names = [
        shipped_name("Kernel", "Array", ShippedKind::Class),
        shipped_name(
            "CollectionsFactory",
            "CollectionsFactory",
            ShippedKind::ModuleInstance,
        ),
        shipped_name("CollectionsFactory", "List", ShippedKind::Metaclass),
        shipped_name(
            "TypedArraysFactory",
            "TypedArraysFactory",
            ShippedKind::Class,
        ),
    ];
    let mut interpreter = Interpreter::bootstrap();
    let booted = interpreter.enter(|mutator| mutator.shipped_name_count());
    if let Err(error) = interpreter.build_shipped_names(&names, false) {
        panic!("{error}");
    }
    let before = interpreter.enter(|mutator| mutator.shipped_name_count());
    assert!(
        before > booted,
        "the first build of the names builds both modules"
    );
    if let Err(error) = interpreter.build_shipped_names(&names, false) {
        panic!("{error}");
    }
    let after = interpreter.enter(|mutator| mutator.shipped_name_count());
    assert_eq!(after, before);
    interpreter.enter(|mutator| {
        if let Err(error) = shipped_names::resolve_in(mutator, &names, false) {
            panic!("{error}");
        }
    });
}

/// Decision 8 of `new` as compiled code survives a snapshot: a body-less
/// declaration's class object is still the level its last clause adds,
/// with the declaration's factory and name, and a level a `<:` clause
/// naming it added still has its factory -- both read through metaclasses
/// the loader rebuilt from the records alone.
#[test]
fn a_body_less_declaration_survives_build_and_load() {
    let source = indoc! {"
        class Holder = Object new () (
          class Base = Object new () ()
          class M1 tag: t = Object new (| public a = t. |) ()
          class M3 x: x = Base new <: M1 tag: x + 1.
          class UsesM3 = Base new <: M3 x: 5 () ()
          class Wrong = Base new <: M3 tag: 5 () ()
          public m3 = ( ^M3 )
          public usesM3 = ( ^UsesM3 )
          public wrong = ( ^Wrong )
          public madeA = ( ^(M3 x: 1) a printString )
          public usesA = ( ^UsesM3 new a printString )
          public name = ( ^M3 name )
          public refused = ( ^[Wrong new] on: Error do: [:error | error messageText] )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for selector in ["m3", "usesM3", "wrong"] {
        expect_ok(
            interpreter.send(&instance, Symbol::intern(selector), &[]),
            selector,
        );
    }
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    for (selector, expected) in [
        ("madeA", "2"),
        ("usesA", "5"),
        ("name", "M3"),
        (
            "refused",
            "a mixin application clause of class Wrong sends #tag:, but the primary factory \
             of class M3 is #x:",
        ),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&loaded.roots[0], Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            assert_eq!(
                text_or_class_name(mutator, mutator.fetch(&answer)),
                expected,
                "{selector}"
            );
        });
    }
    let declared = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("m3"), &[]),
        "m3",
    );
    let uses_m3 = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("usesM3"), &[]),
        "usesM3",
    );
    loaded.interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let declared = class_reference(mutator, &declared);
        let level = heap
            .class_superclass(class_reference(mutator, &uses_m3))
            .expect("UsesM3 has the level its clause added below it");
        let x_factory = Some(ns_runtime::mixin::Factory {
            selector: Symbol::intern("x:"),
            argument_count: 1,
        });
        assert_eq!(
            heap.class_application(declared)
                .expect("a class object")
                .mixin
                .name
                .as_str(),
            "M1"
        );
        assert_eq!(heap.class_name(declared), Some(Symbol::intern("M3")));
        assert_eq!(heap.class_factory(declared), x_factory);
        assert_eq!(heap.class_name(level), Some(Symbol::intern("M3")));
        assert_eq!(heap.class_factory(level), x_factory);
    });
}

/// A body-less declaration's clause argument resolves in the declaration's
/// own scope (`newspeak-spec.pdf` 3.3, 5.9), which its class object keeps
/// beside the named class's: built before the snapshot, the class object
/// is an `OBJECT_BODY_LESS_CLASS` record, and after load its clause still
/// reaches the holder's `helper`, while M1's method on its instance still
/// reaches the library's.
#[test]
fn a_body_less_declarations_own_scope_survives_build_and_load() {
    let source = indoc! {"
        class Module = Object new (|
          library = Library new.
          public holder = Holder new.
        |) (
          class Library = Object new () (
            public class M1 tag: t = Object new (| public a = t. |) (
              public fromM1 = ( ^helper )
            )
            public helper = ( ^'the library helper' )
          )
          public class Holder = Object new () (
            class M3 = Object new <: libraryM1 tag: helper , ' and ' , outer Holder identify.
            libraryM1 = ( ^library M1 )
            public helper = ( ^'the holder helper' )
            public identify = ( ^'the holder' )
            public m3 = ( ^M3 )
            public madeA = ( ^M3 new a )
            public fromM1 = ( ^M3 new fromM1 )
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let holder = expect_ok(
        interpreter.send(&instance, Symbol::intern("holder"), &[]),
        "holder",
    );
    let declared = expect_ok(interpreter.send(&holder, Symbol::intern("m3"), &[]), "m3");
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let declared = class_reference(mutator, &declared);
        let scope = heap
            .class_declaration_scope(declared)
            .expect("a body-less declaration's class object keeps its scope");
        assert!(
            scope
                .enclosing
                .zip(mutator.fetch(&holder).as_object())
                .is_some_and(|(enclosing, holder)| enclosing.same(holder)),
            "the declaration's enclosing object is the holder"
        );
        assert!(scope.declaring_class.is_some());
    });
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let holder = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("holder"), &[]),
        "holder",
    );
    for (selector, expected) in [
        ("madeA", "the holder helper and the holder"),
        ("fromM1", "the library helper"),
    ] {
        let answer = expect_ok(
            loaded
                .interpreter
                .send(&holder, Symbol::intern(selector), &[]),
            selector,
        );
        loaded.interpreter.enter(|mutator| {
            assert_eq!(
                text_or_class_name(mutator, mutator.fetch(&answer)),
                expected,
                "{selector}"
            );
        });
    }
}

/// A module's root without a body (`class R = Object <: Value.`) is the
/// level its clause added, `Value`'s mixin over `Object`, named for the
/// root; 3.3 makes a module definition's enclosing object nil, so its
/// declaration's scope is empty. The image carries it as that level, and
/// the loader holds it to the root rule as such.
// spec: image.verify.root-rule
#[test]
fn a_body_less_root_survives_build_and_load() {
    let (mut interpreter, class) = load("class BodyLessRoot = Object <: Value.");
    expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        check_body_less_root(mutator, class);
        snapshot_one(mutator, false, "t.ns", class, &[Value::object(class)]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let root = loaded.roots[0].clone();
    loaded.interpreter.enter(|mutator| {
        let class = class_reference(mutator, &root);
        check_body_less_root(mutator, class);
    });
    expect_ok(loaded.interpreter.instantiate(&root, &[]), "instantiate");
}

/// `class` is `BodyLessRoot`'s class object: the `Value` level over
/// `Object`, enclosed where `Value` is, with an empty declaration scope.
fn check_body_less_root<'gc>(mutator: &Mutator<'gc>, class: ClassReference<'gc>) {
    let heap = mutator.heap();
    let value = mutator.builtin_class(BuiltinClass::Value);
    let object = mutator.builtin_class(BuiltinClass::Object);
    assert_eq!(heap.class_name(class), Some(Symbol::intern("BodyLessRoot")));
    assert!(
        heap.class_named(class)
            .is_some_and(|named| named.same(value))
    );
    assert!(
        heap.class_superclass(class)
            .is_some_and(|superclass| superclass.same(object))
    );
    let scope = heap
        .class_declaration_scope(class)
        .expect("the root's class object keeps its declaration's scope");
    assert!(scope.enclosing.is_none() && scope.declaring_class.is_none());
    assert!(
        heap.class_enclosing(class).is_some(),
        "the level's own enclosing object is Value's"
    );
}

/// §3.1.1 is decided on a body-less value class's declaration's enclosing
/// object as well as on its level's, when the class object is created and
/// again when an image rebuilds it: `Valued`'s level is enclosed by the
/// value module, its declaration by a mutable `Holder`, so it is refused
/// before the snapshot and after the load alike.
#[test]
fn a_body_less_value_class_is_refused_alike_after_build_and_load() {
    let source = indoc! {"
        class Module = Object <: Value () (
          public class Marker = Object new () ()
          public class Holder = Object new (| public count ::= 0. |) (
            public class Valued = Value new <: Marker.
            public valued = ( ^Valued )
            public refused = ( ^[Valued new] on: Error do: [:error | error messageText] )
          )
          public holder = ( ^Holder new )
        ) : ()
    "};
    let expected = "cannot instantiate Valued: a value class's enclosing objects must be \
                    values, and its enclosing object, an instance of Holder, is not one";
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let holder = expect_ok(
        interpreter.send(&instance, Symbol::intern("holder"), &[]),
        "holder",
    );
    expect_ok(
        interpreter.send(&holder, Symbol::intern("valued"), &[]),
        "valued",
    );
    let refused = expect_ok(
        interpreter.send(&holder, Symbol::intern("refused"), &[]),
        "refused",
    );
    let bytes = interpreter.enter(|mutator| {
        assert_eq!(
            text_or_class_name(mutator, mutator.fetch(&refused)),
            expected
        );
        let class = class_reference(mutator, &class);
        let holder = mutator.fetch(&holder);
        snapshot_one(mutator, true, "t.ns", class, &[holder]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let refused = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("refused"), &[]),
        "refused",
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(
            text_or_class_name(mutator, mutator.fetch(&refused)),
            expected
        );
    });
}

/// A code record's header, for a body of `kind` taking `argument_count`
/// arguments in `register_count` registers, with no copied value, no
/// statement map and its sends cached: what the limit tests vary.
fn code_header(
    writer: &mut encode::Writer,
    kind: CodeKind,
    argument_count: u16,
    register_count: u16,
) {
    writer.write_u8(encode::code_kind_tag(kind));
    writer.write_u16(argument_count);
    writer.write_u16(register_count);
    writer.write_u16(0); // copied_count
    writer.write_u16(register_count); // local_count
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_boolean(false); // not compiled uncached
}

/// The decode error of a code table of one record, which `write` writes
/// after the table's count.
fn code_table_error(write: impl FnOnce(&mut encode::Writer)) -> decode::DecodeError {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry
    write(&mut writer);
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_code_table(&mut reader, &table_with(&["foo"])) {
        Ok(_) => panic!("expected the code table to be refused"),
        Err(error) => error,
    }
}

/// A body claiming one instruction past the limit of 2^20 is refused where
/// its count is read, before any instruction is: the bytes hold none.
// spec: bytecode.verify.body-fields
#[test]
fn an_image_with_a_body_past_the_instruction_limit_is_refused() {
    let error = code_table_error(|writer| {
        code_header(writer, CodeKind::Method, 0, HEADER_REGISTERS + 1);
        writer.write_u32((1 << 20) + 1); // the instruction count, and no instruction
    });
    assert_eq!(
        error,
        decode::DecodeError::InstructionCountTooLarge {
            count: (1 << 20) + 1
        }
    );
    assert_eq!(
        error.to_string(),
        "a body in the image holds 1048577 instructions, more than the limit of 1048576"
    );
}

/// A body taking 256 arguments is refused at its header, and so is a
/// method record of 256 parameters in the mixin table: no compiled program
/// has either.
// spec: bytecode.verify.body-fields
// spec: image.verify.mixin-rows
#[test]
fn an_image_with_a_method_of_256_parameters_is_refused() {
    let error = code_table_error(|writer| {
        code_header(writer, CodeKind::Method, 256, 256 + HEADER_REGISTERS + 1);
        writer.write_u32(0);
    });
    assert_eq!(
        error,
        decode::DecodeError::ArgumentCountTooLarge {
            what: "body",
            count: 256
        }
    );
    assert_eq!(
        error.to_string(),
        "a body in the image takes 256 arguments, more than the limit of 255"
    );
    let mut writer = encode::Writer::new();
    writer.write_u32(1);
    write_mixin_row_with_method_shapes(
        &mut writer,
        0,
        &[],
        &[MethodShape {
            selector: 0,
            access: AccessModifier::Public,
            argument_count: 256,
            code: 0,
        }],
        &[],
    );
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match mixins::decode(&mut reader, &table_with(&["foo"]), &[]) {
        Err(error) => assert_eq!(
            error,
            decode::DecodeError::ArgumentCountTooLarge {
                what: "method",
                count: 256
            }
        ),
        Ok(_) => panic!("expected the method of 256 parameters to be refused"),
    }
}

/// A send of 256 arguments, and a closure of 256 parameters, are refused
/// where their list's count is read.
// spec: bytecode.verify.argument-windows
#[test]
fn an_image_with_a_send_of_256_arguments_is_refused() {
    let error = code_table_error(|writer| {
        code_header(writer, CodeKind::Method, 0, 300);
        writer.write_u32(1); // one instruction ...
        writer.write_u8(OPCODE_SEND_PUBLIC);
        writer.write_u32(0); // selector `foo`
        writer.write_u16(HEADER_REGISTERS); // receiver
        writer.write_u16(HEADER_REGISTERS + 1); // argument base
        writer.write_u16(256); // ... of 256 arguments
    });
    assert_eq!(
        error,
        decode::DecodeError::ArgumentCountTooLarge {
            what: "send",
            count: 256
        }
    );
    let error = code_table_error(|writer| {
        code_header(writer, CodeKind::Method, 0, HEADER_REGISTERS + 1);
        writer.write_u32(1); // one instruction ...
        writer.write_u8(OPCODE_MAKE_CLOSURE);
        writer.write_u16(HEADER_REGISTERS); // destination
        writer.write_u16(256); // ... naming 256 parameters
    });
    assert_eq!(
        error,
        decode::DecodeError::ArgumentCountTooLarge {
            what: "closure",
            count: 256
        }
    );
}

/// A mixin-table row of 1,025 `<:` clauses is refused where the count is
/// read, before a clause is reserved for.
// spec: image.limit.clauses
#[test]
fn an_image_applying_1025_mixins_is_refused() {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one row
    writer.write_u32(0); // name
    writer.write_u8(mixins::access_tag(AccessModifier::Protected));
    writer.write_u32(0); // span.start
    writer.write_u32(0); // span.end
    writer.write_u32(0); // no slots
    writer.write_boolean(false); // no factory
    writer.write_boolean(false); // no initExprs
    writer.write_boolean(true); // a superclass clause ...
    writer.write_u32(0); // ... sending `foo`
    writer.write_boolean(false); // without arguments
    writer.write_u32(1025); // the clause count, and no clause
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match mixins::decode(&mut reader, &table_with(&["foo"]), &[]) {
        Err(error) => {
            assert_eq!(
                error,
                decode::DecodeError::ClauseCountTooLarge { count: 1025 }
            );
            assert_eq!(
                error.to_string(),
                "a mixin-table row applies 1025 mixins, more than the limit of 1024"
            );
        }
        Ok(_) => panic!("expected the row of 1,025 clauses to be refused"),
    }
}

/// The register a `key:` send's callee, `Ephemeron>>key:`, holds the VM
/// mirror in when it answers, as the sender numbers it: the callee lowers
/// `vmMirror` into a scratch register of its own for its last
/// `SendMirror`, whose answer it returns, and is laid on the send's slots,
/// from `argument_base` on.
fn mirror_register_of_ephemeron_key<'gc>(
    mutator: &Mutator<'gc>,
    ephemeron: Value<'gc>,
    argument_base: u16,
) -> u16 {
    let key_code = method_code(mutator.heap(), mutator.class_of(ephemeron), "key:");
    let mirror_register = key_code
        .instructions()
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            Instruction::SendMirror {
                mirror_register, ..
            } => Some(*mirror_register),
            _ => None,
        })
        .expect("Ephemeron>>key: runs a mirror send");
    argument_base + mirror_register
}

/// The capability barrier the analysis protects: a crafted method sends
/// `key: nil` to an `Ephemeron`, whose shipped method stores the key by a
/// `SendMirror` and so holds the VM mirror in one of its registers until
/// it answers -- a register of the sender's above the send's slots, where
/// the callee is laid; then the crafted method returns that register. No
/// compiled body reads a register above a send's slots before writing it,
/// so the image carrying the method is refused at load, before any of its
/// code runs, whatever the pop leaves in that register.
// spec: bytecode.register.leftovers
// spec: bytecode.verify.leftovers
#[test]
fn an_image_reading_the_mirror_a_kernel_method_left_is_refused() {
    let source = indoc! {"
        class X = Object new (
        ) (
          public ephemeronOf: platform = ( ^platform kernel Ephemeron new )
          public run: subject = ( ^subject key: nil )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let ephemeron = expect_ok(
        interpreter.send(&instance, Symbol::intern("ephemeronOf:"), &[platform]),
        "ephemeronOf:",
    );
    let (image_bytes, refused_at, mirror_register) = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let run = method_code(mutator.heap(), class, "run:");
        let mut instructions = run.instructions();
        let (argument_base, slot_end) = instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::SendPublic {
                    selector,
                    argument_base,
                    argument_sources,
                    ..
                } if *selector == Symbol::intern("key:") => Some((
                    *argument_base,
                    *argument_base + argument_sources.len() as u16,
                )),
                _ => None,
            })
            .expect("run: sends key:");
        let ephemeron = mutator.fetch(&ephemeron);
        let mirror_register = mirror_register_of_ephemeron_key(mutator, ephemeron, argument_base);
        assert!(
            mirror_register >= slot_end,
            "the mirror lies above the slots"
        );
        // The body's own return of the send's answer answers the register
        // the callee held the mirror in instead.
        let last = instructions.len() - 1;
        assert!(matches!(
            instructions[last],
            Instruction::Return { .. } | Instruction::ImplicitReturn { .. }
        ));
        instructions[last] = Instruction::Return {
            register: mirror_register,
        };
        let doctored = Rc::new(CompiledCode::new(
            CodeKind::Method,
            instructions,
            run.argument_count,
            run.register_count.max(mirror_register + 1),
            0,
            vec![],
            Span::new(0, 0),
        ));
        let mixin = with_method_code(mixin_of(mutator.heap(), class), "run:", doctored);
        install_mixin(mutator, class, mixin);
        let instance = mutator.fetch(&instance);
        let image_bytes =
            snapshot_one(mutator, false, "t.ns", class, &[instance]).expect("snapshot");
        (image_bytes, last as u32, mirror_register)
    });
    let Err(error) = image::load(&image_bytes) else {
        panic!("a read of the mirror a callee left must be refused at load")
    };
    let expected = decode::DecodeError::LeftoverRead {
        instruction: refused_at,
        register: mirror_register,
    };
    assert!(error.contains(&expected.to_string()), "{error}");
}

/// A body whose return reads a scratch register nothing wrote -- a word
/// the push left there -- is refused, and so is one that reads a register
/// above a send's slots, where its callee was laid.
// spec: bytecode.register.leftovers
// spec: bytecode.verify.leftovers
#[test]
fn an_image_reading_unwritten_scratch_is_refused() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        HEADER_REGISTERS + 1,
        vec![Instruction::Return {
            register: HEADER_REGISTERS,
        }],
    )]);
    let error = decode::verify_code_table(&table).expect_err("the scratch is unwritten");
    assert_eq!(
        error,
        decode::DecodeError::LeftoverRead {
            instruction: 0,
            register: HEADER_REGISTERS,
        }
    );
    assert_eq!(
        error.to_string(),
        format!(
            "register r{HEADER_REGISTERS} is read at instruction 0 before the body writes it: \
             it may hold what a callee or the push left there"
        )
    );
    let receiver = HEADER_REGISTERS;
    let above = HEADER_REGISTERS + 2;
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        HEADER_REGISTERS + 3,
        vec![
            Instruction::LoadSelf {
                destination: receiver,
            },
            Instruction::LoadSelf { destination: above },
            Instruction::SendPublic {
                receiver_register: receiver,
                selector: Symbol::intern("foo:"),
                argument_base: HEADER_REGISTERS + 1,
                argument_sources: vec![receiver],
                destination: receiver,
                call_site: CallSite { number: 0 },
            },
            Instruction::Return { register: above },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::LeftoverRead {
            instruction: 3,
            register: above,
        })
    );
}

/// A body keeping 258 forward branch targets pending at once, one more
/// than any compiled body can, is refused at the branch that opens the
/// 258th, before the analysis takes a bitset for it; one of 257 verifies.
// spec: bytecode.verify.leftovers
#[test]
fn an_image_past_the_verification_bound_is_refused() {
    let nested_branches = |count: u32| {
        let mut instructions: Vec<Instruction> = (0..count)
            .map(|index| Instruction::BranchIfFalse {
                register: 0,
                target: 2 * count - 1 - index,
            })
            .collect();
        instructions.extend((0..count).map(|_| Instruction::LoadSelf {
            destination: HEADER_REGISTERS + 1,
        }));
        instructions.push(Instruction::Return { register: 0 });
        table_of(vec![code_with(
            CodeKind::Method,
            1,
            HEADER_REGISTERS + 2,
            instructions,
        )])
    };
    let error = decode::verify_code_table(&nested_branches(258))
        .expect_err("258 pending targets are past the bound");
    assert_eq!(
        error,
        decode::DecodeError::VerificationPastTheBound { instruction: 257 }
    );
    assert_eq!(
        error.to_string(),
        "a body in the image passes the verification's bound at instruction 257: no \
         compiled body keeps more than 257 jump targets pending or 128 loops open, \
         crosses two loops or jumps into a loop past its head"
    );
    assert_eq!(decode::verify_code_table(&nested_branches(257)), Ok(()));
}

/// A program-made symbol an image carries round-trips as its text: the
/// loaded graph holds a symbol equal and identical to one made afresh
/// from the same text, and to the interned symbol of that text, which the
/// decoder interns (`docs/design/memory.md`, "Weak references").
#[test]
fn an_image_keeps_a_program_made_symbol() {
    let source = indoc! {"
        class X = Object new (
          | made = ('q' , 'x') asSymbol. |
        ) (
          public symbol = ( ^made )
          public isMadeAgain = ( ^made == ('q' , 'x') asSymbol )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    assert_eq!(
        Symbol::lookup("qx"),
        None,
        "nothing has interned the symbol's text before the snapshot"
    );
    assert_eq!(interpreter.table_sizes().program_symbols, 1);
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        snapshot_one(mutator, true, "t.ns", class, &[instance]).expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    let again = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("isMadeAgain"), &[]),
        "isMadeAgain",
    );
    let symbol = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("symbol"), &[]),
        "symbol",
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(mutator.fetch(&again).as_boolean(), Some(true));
        let held = mutator.fetch(&symbol);
        assert_eq!(
            held.as_object()
                .and_then(|object| mutator.heap().symbol_text(object))
                .as_deref(),
            Some("qx")
        );
        let interned = mutator.new_symbol(Symbol::intern("qx"));
        assert!(
            held.as_object()
                .zip(interned.as_object())
                .is_some_and(|(held, interned)| held.same(interned)),
            "the loaded symbol is the one object of its text"
        );
    });
}
