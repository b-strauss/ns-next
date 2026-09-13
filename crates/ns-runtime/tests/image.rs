//! Image format round-trip tests, section by section: the header and its
//! scalars, the mixin table, the code table, the object and frame tables,
//! whole-image `snapshot`/`load`, and the two verification passes.

use indoc::{formatdoc, indoc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal};
use ns_runtime::image;
use ns_runtime::image::{decode, encode, mixins, symbols};
use ns_runtime::SourceOrigin as Origin;
use ns_runtime::{
    BuiltinClass, ClassData, ClassRef, FrameKind, Heap, HostResult, HostSignal, Interpreter, Mixin,
    MixinApplication, ObjectData, Root, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

#[test]
fn header_round_trips() {
    for (flags, has_source) in [(0u8, false), (encode::FLAG_HAS_SOURCE, true)] {
        let mut w = encode::Writer::new();
        encode::write_header(&mut w, flags);
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        let header = decode::read_header(&mut r).expect("valid header");
        assert_eq!(header.has_source, has_source);
    }
}

/// An unknown flag bit governs a section this build cannot see, so
/// continuing would read every later field out of the wrong offset.
#[test]
fn an_unknown_header_flag_is_rejected_not_ignored() {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w, 0x80);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut r),
        Err(decode::DecodeError::UnknownHeaderFlags { flags: 0x80 })
    );
}

#[test]
fn a_bad_magic_is_rejected() {
    let bytes = vec![b'X', b'X', b'X', b'X', 1, 0, 0, 0];
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut r),
        Err(decode::DecodeError::BadMagic)
    );
}

#[test]
fn a_version_mismatch_is_rejected_not_migrated() {
    let mut w = encode::Writer::new();
    w.write_raw(encode::MAGIC);
    w.write_u32(9999);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut r),
        Err(decode::DecodeError::VersionMismatch {
            found: 9999,
            expected: encode::FORMAT_VERSION
        })
    );
}

#[test]
fn a_truncated_image_is_an_error_not_a_panic() {
    let bytes = vec![b'N', b'S'];
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut r),
        Err(decode::DecodeError::UnexpectedEof)
    );
}

#[test]
fn every_scalar_value_variant_round_trips() {
    for v in [
        Value::Nil,
        Value::Bool(true),
        Value::Bool(false),
        Value::SmallInt(-42),
        Value::Float(3.5),
    ] {
        let mut w = encode::Writer::new();
        assert!(encode::encode_scalar_value(&mut w, &v));
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(&mut r).expect("decode");
        match (&v, &decoded) {
            (Value::Nil, Value::Nil) => {}
            (Value::Bool(a), Value::Bool(b)) => assert_eq!(a, b),
            (Value::SmallInt(a), Value::SmallInt(b)) => assert_eq!(a, b),
            (Value::Float(a), Value::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
            _ => panic!("round-trip mismatch"),
        }
    }
}

#[test]
fn integers_and_strings_are_little_endian_and_length_prefixed() {
    let mut w = encode::Writer::new();
    w.write_u16(0x0102);
    w.write_u32(0x01020304);
    w.write_i64(-1);
    w.write_str("hi");
    w.write_bool(true);
    let bytes = w.into_bytes();
    assert_eq!(&bytes[0..2], &[0x02, 0x01]);
    assert_eq!(&bytes[2..6], &[0x04, 0x03, 0x02, 0x01]);
    assert_eq!(&bytes[6..14], &[0xff; 8]);
    assert_eq!(&bytes[14..20], &[2, 0, 0, 0, b'h', b'i']);
    assert_eq!(bytes[20], 1);
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(r.read_u16().unwrap(), 0x0102);
    assert_eq!(r.read_u32().unwrap(), 0x01020304);
    assert_eq!(r.read_i64().unwrap(), -1);
    assert_eq!(r.read_str().unwrap(), "hi");
    assert!(r.read_bool().unwrap());
    assert_eq!(r.read_u8(), Err(decode::DecodeError::UnexpectedEof));
}

#[test]
fn scalar_value_boundary_cases_round_trip() {
    for v in [
        Value::SmallInt(i64::MIN),
        Value::SmallInt(i64::MAX),
        Value::SmallInt(0),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
        Value::Float(-0.0),
        Value::Float(0.0),
    ] {
        let mut w = encode::Writer::new();
        assert!(encode::encode_scalar_value(&mut w, &v));
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(&mut r).expect("decode");
        match (&v, &decoded) {
            (Value::SmallInt(a), Value::SmallInt(b)) => assert_eq!(a, b),
            (Value::Float(a), Value::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
            _ => panic!("round-trip mismatch"),
        }
    }
}

#[test]
fn encoding_an_obj_value_is_refused_not_scalarized() {
    // `Value::Obj` is not a scalar; `encode_scalar_value` must return `false`
    // and write nothing, leaving `encode_value`, the object-graph encoder,
    // in charge.
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let obj = Value::Obj(interp.new_string("hi".to_string()));
    let mut w = encode::Writer::new();
    assert!(!encode::encode_scalar_value(&mut w, &obj));
    assert_eq!(w.into_bytes().len(), 0);
}

#[test]
fn empty_and_non_ascii_strings_round_trip() {
    for s in ["", "h\u{e9}llo \u{4e16}\u{754c} \u{1f980}"] {
        let mut w = encode::Writer::new();
        w.write_str(s);
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        assert_eq!(r.read_str().unwrap(), s);
    }
}

#[test]
fn invalid_utf8_bytes_are_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_bytes(&[0xff, 0xfe, 0xfd]);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(r.read_str(), Err(decode::DecodeError::InvalidUtf8));
}

#[test]
fn a_length_prefix_past_the_end_is_an_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1000);
    w.write_raw(b"short");
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(r.read_bytes(), Err(decode::DecodeError::UnexpectedEof));
}

#[test]
fn a_huge_length_prefix_does_not_overflow_or_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(u32::MAX);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(r.read_bytes(), Err(decode::DecodeError::UnexpectedEof));
}

#[test]
fn an_invalid_scalar_tag_is_an_error_not_a_panic() {
    let bytes = vec![200u8];
    let mut r = decode::Reader::new(&bytes);
    match decode::decode_scalar_value(&mut r) {
        Err(decode::DecodeError::InvalidTag {
            what: "value",
            tag: 200,
        }) => {}
        Err(e) => panic!("expected InvalidTag{{value, 200}}, got a different error: {e}"),
        Ok(_) => panic!("expected InvalidTag{{value, 200}}, got Ok"),
    }
}

#[test]
fn an_invalid_bool_tag_is_an_error_not_a_panic() {
    let bytes = vec![7u8];
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        r.read_bool(),
        Err(decode::DecodeError::InvalidTag {
            what: "bool",
            tag: 7
        })
    );
}

// ---------------------------------------------------------------------------
// The code table and the root list.
// ---------------------------------------------------------------------------

fn load(src: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

/// The class object `root` holds, for the heap accessors and the encoder,
/// which take one. Fetched where it is used: a run may move it.
fn class_ref(interp: &Interpreter, root: &Root) -> ClassRef {
    match interp.fetch(root) {
        Value::Obj(class) => class,
        _ => panic!("expected a class object"),
    }
}

/// A `symbols::Table` holding exactly `strings`, in order -- so `strings[i]`
/// decodes from id `i`, matching the ids a hand-crafted test writes inline.
/// Hostile-input tests that never reference a selector pass
/// `&symbols::Table::empty()` directly instead of calling this.
fn table_with(strings: &[&str]) -> symbols::Table {
    let mut builder = symbols::Builder::new();
    for s in strings {
        builder.intern(s);
    }
    builder.build()
}

/// One hand-crafted mixin-table row: named by symbol `name_id`, with
/// `methods` as `(selector symbol id, code id)` pairs and `nested` as row
/// ids. Everything else in the row is empty -- these fixtures exist to
/// drive one range check at a time, and writing the whole row shape out at
/// each call site would bury the field under test.
fn write_mixin_row(w: &mut encode::Writer, name_id: u32, methods: &[(u32, u32)], nested: &[u32]) {
    w.write_u32(name_id);
    w.write_u8(1); // AccessModifier::Protected
    w.write_u32(0); // span.start
    w.write_u32(0); // span.end
    w.write_u32(0); // no slots
    w.write_bool(false); // no factory
    w.write_bool(false); // no initExprs
    w.write_bool(false); // no superclass clause
    w.write_u32(methods.len() as u32);
    for (selector, code) in methods {
        w.write_u32(*selector);
        w.write_u8(1); // AccessModifier::Protected
        w.write_u16(0); // num_args
        w.write_u32(*code);
    }
    w.write_u32(nested.len() as u32);
    for id in nested {
        w.write_u32(*id);
    }
    w.write_bool(false); // no class side
    w.write_bool(false); // no initializer
    w.write_bool(false); // no superclass clause body
    w.write_bool(false); // no superclass message body
}

/// A hand-crafted mixin table of exactly one leaf row.
fn one_row_mixin_table(name_id: u32, methods: &[(u32, u32)]) -> Vec<u8> {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    write_mixin_row(&mut w, name_id, methods, &[]);
    w.into_bytes()
}

/// A mixin table of `depth` rows, each nesting the next: a declaration tree
/// as deep as it is large. Synthesised directly as bytes rather than by
/// snapshotting real source, because no parser accepts 100_000 levels of
/// nesting -- and the depth an image *claims* is exactly the input a
/// decoder must never recurse on.
fn mixin_chain(depth: u32) -> Vec<u8> {
    let mut w = encode::Writer::new();
    w.write_u32(depth);
    for i in 0..depth {
        let child = [i + 1];
        let nested: &[u32] = if i + 1 < depth { &child } else { &[] };
        write_mixin_row(&mut w, 0, &[], nested);
    }
    w.into_bytes()
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

fn every_instr_once() -> CompiledCode {
    let block = Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instrs: vec![Instr::NonLocalReturn { reg: 0 }],
        num_args: 1,
        num_registers: 1,
        statement_pc: vec![(0, 0..1)],
        call_sites: vec![],
        source: Span::new(1, 2),
    });
    let cs = CallSite { call_site_id: 7 };
    CompiledCode {
        kind: CodeKind::Initializer,
        instrs: vec![
            Instr::LoadLiteral {
                dst: 0,
                literal: Literal::Array(vec![
                    Literal::Nil,
                    Literal::Bool(true),
                    Literal::SmallInt(-5),
                    Literal::Float(1.5),
                    Literal::Str("s".into()),
                    Literal::Sym(Symbol::intern("y")),
                    Literal::Char('z'),
                    Literal::MalformedInt("16r1.8".into()),
                    Literal::UnresolvedIdent("super".into()),
                ]),
            },
            Instr::LoadSelf { dst: 1 },
            Instr::LoadBuiltinClass {
                dst: 2,
                kind: BuiltinClass::OrderedCollection,
            },
            Instr::Move { dst: 3, src: 2 },
            Instr::StoreLocal { src: 3, slot: 0 },
            Instr::LoadOuterLocal {
                dst: 4,
                depth: 2,
                index: 1,
            },
            Instr::StoreOuterLocal {
                src: 4,
                depth: 2,
                index: 1,
            },
            Instr::SendLexical {
                depth: 1,
                selector: Symbol::intern("foo:"),
                arg_regs: vec![1, 2],
                dst: 5,
                call_site: cs,
            },
            Instr::SendPublic {
                selector: Symbol::intern("+"),
                receiver_reg: 1,
                arg_regs: vec![2],
                dst: 6,
                call_site: cs,
            },
            Instr::SendInlinable {
                selector: Symbol::intern("+"),
                receiver_reg: 1,
                arg_regs: vec![2],
                dst: 6,
                call_site: cs,
            },
            Instr::SendSuper {
                selector: Symbol::intern("bar"),
                arg_regs: vec![],
                dst: 7,
                call_site: cs,
            },
            Instr::MakeClosure {
                dst: 8,
                params: Rc::new(vec![Symbol::intern("e")]),
                code: block,
            },
            Instr::MakeArray {
                dst: 9,
                elem_regs: vec![0, 1],
            },
            Instr::InitSlot {
                src: 9,
                own_index: 3,
            },
            Instr::Return { reg: 0 },
            Instr::ImplicitReturn { reg: 1 },
            Instr::NonLocalReturn { reg: 2 },
            Instr::Jump { target: 3 },
            Instr::JumpIfNotBoolean { reg: 1, target: 4 },
            Instr::BranchIfFalse { reg: 1, target: 5 },
            Instr::BranchIfTrue { reg: 1, target: 6 },
        ],
        num_args: 2,
        num_registers: 10,
        statement_pc: vec![(0, 0..3), (1, 3..20)],
        call_sites: vec![cs, cs, cs],
        source: Span::new(10, 20),
    }
}

/// The compiled code of `class`'s method `selector` -- the body the method
/// `Interpreter::push_step` activates carries.
fn method_code(heap: &Heap, class: ClassRef, selector: &str) -> Rc<CompiledCode> {
    let application = heap.class_application(class).expect("not a class");
    application.mixin.methods[&Symbol::intern(selector)]
        .code
        .clone()
}

/// `mixin::preorder` of a live class's declaration tree, in the order the
/// mixin table numbers its rows. `image::load` indexes the *decoded* table
/// here; a test that encodes and decodes against one live class can use
/// that class's own tree, which is the same walk.
fn mixin_preorder(heap: &Heap, class: ClassRef) -> Vec<Rc<Mixin>> {
    let mut order = Vec::new();
    ns_runtime::mixin::preorder(&root_mixin_of(heap, class), &mut order);
    order
}

/// `class`'s own mixin as a value a test can edit. The clone is shallow: its
/// methods, nested declarations and class side are the same `Rc`s.
fn mixin_of(heap: &Heap, class: ClassRef) -> Mixin {
    (*heap.class_application(class).expect("a class object").mixin).clone()
}

/// Replaces `class`'s mixin with `mixin`, applied to the same superclass
/// application. A mixin is shared and immutable, so this is the shape an edit
/// to a live class takes -- and, now that code lives on the declaration, how a
/// test puts a doctored body in front of the encoder.
fn install_mixin(interp: &mut Interpreter, class: ClassRef, mixin: Mixin) {
    let super_application = interp
        .heap_ref()
        .class_application(class)
        .expect("a class object")
        .superclass
        .clone();
    let application = interp.apply_mixin(&Rc::new(mixin), super_application.as_ref());
    assert!(
        interp.heap_mut().set_class_application(class, application),
        "a class object"
    );
}

/// `mixin` with method `selector`'s body replaced by `code`.
fn with_method_code(mut mixin: Mixin, selector: &str, code: Rc<CompiledCode>) -> Mixin {
    let selector = Symbol::intern(selector);
    let method = mixin.methods[&selector].clone();
    mixin.methods.insert(
        selector,
        Rc::new(ns_runtime::mixin::MixinMethod {
            code,
            ..(*method).clone()
        }),
    );
    mixin
}

/// `mixin` with its direct nested declaration `name` replaced by `edit`'s
/// result.
fn with_nested(mut mixin: Mixin, name: &str, edit: impl FnOnce(Mixin) -> Mixin) -> Mixin {
    let index = mixin
        .nested
        .iter()
        .position(|n| n.name.as_str() == name)
        .expect("a nested declaration of that name");
    let edited = edit((*mixin.nested[index]).clone());
    mixin.nested[index] = Rc::new(edited);
    mixin
}

/// Encodes a class's code table and mixin table into fresh bytes and
/// decodes both back. The two travel together everywhere: the mixin table
/// is what says which compiled body belongs to which declaration.
fn round_trip_code_table(
    heap: &Heap,
    class: ClassRef,
) -> (decode::DecodedCodeTable, mixins::Decoded, encode::CodeIds) {
    let mut w = encode::Writer::new();
    let mut symbols = symbols::Builder::new();
    let ids =
        encode::encode_code_table(&mut w, heap, class, &mut symbols).expect("encode code table");
    let mut mixin_w = encode::Writer::new();
    mixins::encode(&mut mixin_w, heap, class, &mut symbols, &ids).expect("encode mixin table");
    let bytes = w.into_bytes();
    let mixin_bytes = mixin_w.into_bytes();
    let table = symbols.build();
    let mut r = decode::Reader::new(&bytes);
    let codes = decode::decode_code_table(&mut r, &table).expect("decode code table");
    let mut mixin_r = decode::Reader::new(&mixin_bytes);
    let decoded_mixins =
        mixins::decode(&mut mixin_r, &table, &codes.codes).expect("decode mixin table");
    (codes, decoded_mixins, ids)
}

#[test]
fn every_instr_variant_round_trips() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let class = class_ref(&interp, &class);
    let synthetic = Rc::new(every_instr_once());
    let doctored = Mixin {
        initializer_code: Some(synthetic.clone()),
        ..mixin_of(interp.heap_ref(), class)
    };
    install_mixin(&mut interp, class, doctored);
    let (table, decoded_mixins, ids) = round_trip_code_table(interp.heap_ref(), class);
    let root = &decoded_mixins.code[0];
    let decoded = &table.codes[root.initializer.expect("initializer id") as usize];
    assert_eq!(**decoded, *synthetic);
    assert_eq!(
        ids.id_of(&synthetic),
        root.initializer.expect("initializer id")
    );
}

#[test]
fn a_real_modules_code_table_round_trips_and_nests_blocks_before_their_parents() {
    let (interp, class) = load(RICH);
    let class = class_ref(&interp, &class);
    let (table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let root = &decoded_mixins.code[0];
    assert_eq!(root.methods.len(), 3);
    assert_eq!(
        root.methods
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>(),
        vec!["arr", "find:in:", "loop"],
        "root list is sorted by selector"
    );
    for (selector, id) in &root.methods {
        let decoded = &table.codes[*id as usize];
        let original = method_code(interp.heap_ref(), class, selector.as_str());
        assert_eq!(**decoded, *original, "{selector}");
        for instr in &decoded.instrs {
            if let Instr::MakeClosure { code, .. } = instr {
                let child_id = table
                    .codes
                    .iter()
                    .position(|c| Rc::ptr_eq(c, code))
                    .expect("a MakeClosure's block body is in the table");
                assert!(
                    child_id < *id as usize,
                    "a block's code precedes its parent's"
                );
            }
        }
    }
    let init = &table.codes[root.initializer.expect("initializer id") as usize];
    assert_eq!(
        **init,
        *mixin_of(interp.heap_ref(), class)
            .initializer_code
            .expect("an initializer")
    );
}

/// A mixin decoded from an image carries the bodies its own row named, with
/// no table beside it: every method the decoded declaration holds has the
/// body the live method had, and so does its initializer.
#[test]
fn a_decoded_mixin_carries_the_bodies_its_row_names() {
    let (interp, class) = load(RICH);
    let class = class_ref(&interp, &class);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let root = decoded_mixins.root().expect("a root row");
    assert_eq!(root.methods.len(), 3);
    for (selector, method) in &root.methods {
        assert_eq!(
            *method.code,
            *method_code(interp.heap_ref(), class, selector.as_str()),
            "{selector}"
        );
    }
    assert!(root.initializer_code.is_some());
}

#[test]
fn a_root_list_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_row_mixin_table(0, &[(0, 5)]);
    let mut r = decode::Reader::new(&bytes);
    match mixins::decode(&mut r, &table_with(&["foo"]), &[]) {
        Err(decode::DecodeError::BadCodeId(id)) => {
            assert_eq!(id, 5);
            assert_eq!(
                decode::DecodeError::BadCodeId(id).to_string(),
                "code id 5 out of range"
            );
        }
        Err(e) => panic!("expected BadCodeId(5), got a different error: {e}"),
        Ok(_) => panic!("expected BadCodeId(5), got Ok"),
    }
}

#[test]
fn a_make_closure_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1); // one code entry ...
    w.write_u8(1); // CodeKind::Block
    w.write_u16(0); // num_args
    w.write_u16(1); // num_registers
    w.write_u32(0); // source.start
    w.write_u32(0); // source.end
    w.write_u32(0); // no statement_pc entries
    w.write_u32(0); // no call sites
    w.write_u32(1); // one instruction ...
    w.write_u8(10); // ... a MakeClosure
    w.write_u16(0); // dst
    w.write_u16(0); // no params
    w.write_u32(9); // ... whose block-body code id is out of range
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::BadCodeId(9))
    );
}

#[test]
fn a_forward_make_closure_code_id_is_a_decode_error_not_a_panic() {
    // Post-order means a block's code always precedes its parent's, so an
    // id pointing at a not-yet-decoded entry is corruption, not a cycle.
    let mut w = encode::Writer::new();
    w.write_u32(2); // two code entries ...
    w.write_u8(0); // ... the first a CodeKind::Method
    w.write_u16(0);
    w.write_u16(1);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(1); // one instruction ...
    w.write_u8(10); // ... a MakeClosure
    w.write_u16(0); // dst
    w.write_u16(0); // no params
    w.write_u32(1); // ... naming the *following*, not-yet-decoded entry
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::BadCodeId(1))
    );
}

#[test]
fn an_invalid_code_kind_tag_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(200);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "code kind",
            tag: 200
        })
    );
}

#[test]
fn an_invalid_instruction_tag_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(0); // CodeKind::Method
    w.write_u16(0);
    w.write_u16(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(1); // one instruction ...
    w.write_u8(21); // ... one past the last real tag
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 21
        })
    );
}

#[test]
fn an_invalid_literal_tag_and_a_bad_char_are_decode_errors_not_panics() {
    for (payload, what) in [
        (vec![10u8], "literal"),
        (vec![6u8, 0x00, 0xd8, 0x00, 0x00], "char"),
    ] {
        let mut w = encode::Writer::new();
        w.write_u32(1);
        w.write_u8(0); // CodeKind::Method
        w.write_u16(0);
        w.write_u16(1);
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(1); // one instruction ...
        w.write_u8(0); // ... a LoadLiteral
        w.write_u16(0); // dst
        w.write_raw(&payload);
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        match decode::decode_code_table(&mut r, &symbols::Table::empty()) {
            Err(decode::DecodeError::InvalidTag { what: got, .. }) => assert_eq!(got, what),
            Err(e) => panic!("expected an invalid {what} tag, got a different error: {e}"),
            Ok(_) => panic!("expected an invalid {what} tag, got Ok"),
        }
    }
}

#[test]
fn an_invalid_builtin_class_tag_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(0); // CodeKind::Method
    w.write_u16(0);
    w.write_u16(1);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(0);
    w.write_u32(1); // one instruction ...
    w.write_u8(2); // ... a LoadBuiltinClass
    w.write_u16(0); // dst
    w.write_u8(200); // ... naming no builtin class
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class",
            tag: 200
        })
    );
}

#[test]
fn a_truncated_code_table_is_a_decode_error_not_a_panic() {
    let (interp, class) = load(RICH);
    let class = class_ref(&interp, &class);
    let mut w = encode::Writer::new();
    let mut symbols = symbols::Builder::new();
    encode::encode_code_table(&mut w, interp.heap_ref(), class, &mut symbols)
        .expect("encode code table");
    let table = symbols.build();
    let bytes = w.into_bytes();
    for cut in [1usize, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
        let mut r = decode::Reader::new(&bytes[..cut]);
        assert!(
            decode::decode_code_table(&mut r, &table).is_err(),
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
        BuiltinClass::Str,
        BuiltinClass::Sym,
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
    let mut tags: Vec<u8> = all.iter().map(|k| encode::builtin_class_tag(*k)).collect();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags.len(), all.len(), "every builtin class has its own tag");
}

#[test]
fn a_backwards_source_span_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(0); // CodeKind::Method
    w.write_u16(0); // num_args
    w.write_u16(0); // num_registers
    w.write_u32(9); // source.start ...
    w.write_u32(4); // ... after source.end
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::InvalidSpan { start: 9, end: 4 })
    );
}

#[test]
fn a_huge_code_table_count_does_not_reserve_gigabytes() {
    // A count prefix is untrusted: reserving for it before reading would
    // abort the process instead of reporting a short image.
    let mut w = encode::Writer::new();
    w.write_u32(u32::MAX);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
        Err(decode::DecodeError::UnexpectedEof)
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
    let mut w = encode::Writer::new();
    w.write_u32(1); // one code entry
    w.write_u8(0); // CodeKind::Method
    w.write_u16(0); // num_args
    w.write_u16(1); // num_registers
    w.write_u32(0); // source.start
    w.write_u32(0); // source.end
    w.write_u32(0); // no statement_pc entries
    w.write_u32(0); // no call sites
    w.write_u32(1); // one instruction ...
    w.write_u8(0); // ... a LoadLiteral
    w.write_u16(0); // dst
    for _ in 0..depth {
        w.write_u8(7); // Literal::Array ...
        w.write_u32(1); // ... of exactly one element
    }
    w.write_u8(0); // Literal::Nil at the bottom
    w.write_u32(0); // no mixin sections
    w.into_bytes()
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
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r, &symbols::Table::empty()),
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
    let mut r = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut r, &symbols::Table::empty())
        .expect("128 levels is within the limit");
    assert_eq!(table.codes.len(), 1);
    assert!(matches!(
        table.codes[0].instrs[0],
        Instr::LoadLiteral {
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
    use ns_runtime::image::opcodes::{OP_MAKE_CLOSURE, OP_RETURN};
    let body = |w: &mut encode::Writer, kind: u8, creates: Option<usize>| {
        w.write_u8(kind);
        w.write_u16(0); // num_args
        w.write_u16(1); // num_registers
        w.write_u32(0); // source.start
        w.write_u32(0); // source.end
        w.write_u32(0); // no statement_pc entries
        w.write_u32(0); // no call sites
        match creates {
            Some(block) => {
                w.write_u32(2); // two instructions ...
                w.write_u8(OP_MAKE_CLOSURE); // ... r0 := the next block in,
                w.write_u16(0);
                w.write_u16(0); // taking no parameters,
                w.write_u32(u32::try_from(block).expect("a test-sized code id"));
            }
            None => w.write_u32(1), // the innermost: one instruction ...
        }
        w.write_u8(OP_RETURN); // ... then ^r0
        w.write_u16(0);
    };
    let mut w = encode::Writer::new();
    w.write_u32(u32::try_from(levels + 1).expect("a test-sized code table"));
    body(&mut w, 1, None); // CodeKind::Block
    for id in 1..levels {
        body(&mut w, 1, Some(id - 1));
    }
    body(&mut w, 0, Some(levels - 1)); // CodeKind::Method
    w.into_bytes()
}

/// A decoded block's nesting is what bounds the exempt activations a
/// crafted `SendInlinable` chain can stack above one counted activation
/// (`docs/design/compiler.md`, "How the limit counts"), so the decoder caps it
/// at the parser's own ceiling: one level past it is refused.
#[test]
fn a_block_nested_one_level_past_the_ceiling_is_a_decode_error() {
    let bytes = code_table_with_nested_blocks(129);
    let mut r = decode::Reader::new(&bytes);
    match decode::decode_code_table(&mut r, &symbols::Table::empty()) {
        Err(e) => assert_eq!(e, decode::DecodeError::BlockNestingTooDeep { limit: 128 }),
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
    let mut r = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut r, &symbols::Table::empty())
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
    let mut r = decode::Reader::new(&bytes);
    let result = decode::decode_code_table(&mut r, &symbols::Table::empty());
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
    let mut interp = Interpreter::bootstrap();
    let empty = mixins::Decoded::empty();
    match decode::build_root_class(&mut interp, &empty, None) {
        Err(decode::DecodeError::NotAClass) => assert_eq!(
            decode::DecodeError::NotAClass.to_string(),
            "the image's mixin table describes no root class"
        ),
        Err(e) => panic!("expected NotAClass, got a different error: {e}"),
        Ok(_) => panic!("expected NotAClass, got Ok"),
    }
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
    code.instrs
        .iter()
        .filter_map(|instr| match instr {
            Instr::LoadLiteral { literal, .. } => Some(depth(literal)),
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
    let (interp, class) = load(&source_with_nested_literal_array(levels));
    let class = class_ref(&interp, &class);
    let original = method_code(interp.heap_ref(), class, "arr");
    assert_eq!(
        literal_array_depth(&original),
        levels,
        "the compiler lowered the whole nest"
    );
    let (table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let (_, id) = decoded_mixins.code[0]
        .methods
        .iter()
        .find(|(selector, _)| selector.as_str() == "arr")
        .expect("arr is in the root list");
    let decoded = &table.codes[*id as usize];
    assert_eq!(**decoded, *original);
    assert_eq!(literal_array_depth(decoded), levels);
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
    code.instrs
        .iter()
        .filter_map(|instr| match instr {
            Instr::MakeClosure { code: block, .. } => Some(1 + block_nesting(block)),
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
    let src = source_with_nested_blocks(levels);
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
        let module = ns_syntax::parse(&src).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            Rc::from(src.as_str()),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interp = Interpreter::bootstrap_with(lowering);
        let class = interp.load_module(&resolved);
        // Nothing runs in this interpreter, so the class read from its root
        // stays current.
        let class = class_ref(&interp, &class);
        assert_eq!(
            block_nesting(&method_code(interp.heap_ref(), class, "nest")),
            levels,
            "{lowering:?}: every source level is a block in the code table"
        );
        let bytes = image::build(
            interp.heap_ref(),
            None,
            &Origin::File(PathBuf::from("t.ns")),
            class,
            Symbol::intern("nest"),
        )
        .expect("build");
        let mut loaded = match image::load(&bytes) {
            Ok(loaded) => loaded,
            Err(e) => panic!("{lowering:?}: {levels} nested blocks must load: {e}"),
        };
        let instance = expect_ok(
            loaded.interp.instantiate(&loaded.root_class, &[]),
            "instantiate",
        );
        let answer = expect_ok(
            loaded.interp.send(&instance, Symbol::intern("nest"), &[]),
            "nest",
        );
        assert!(
            matches!(loaded.interp.fetch(&answer), Value::Nil),
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
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

/// The mixin table's row ids, without keeping the table's own bytes: a
/// graph section names a class object's declaration by row id, so encoding
/// one needs exactly the map `image::snapshot` builds.
fn mixin_ids_for(
    class: ClassRef,
    interp: &Interpreter,
    symbols: &mut symbols::Builder,
    ids: &encode::CodeIds,
) -> HashMap<usize, u32> {
    let mut scratch = encode::Writer::new();
    mixins::encode(&mut scratch, interp.heap_ref(), class, symbols, ids).expect("mixin table")
}

/// Loads `src`, lets `make_roots` build the roots, encodes code table +
/// graph section, decodes both with the same `Reader`, and returns
/// (interp, class, original roots, decoded roots, max identity hash).
///
/// `make_roots` may run code, so the class and the roots stay rooted across
/// it and are read out after it, for the encoder and the decoder, which run
/// nothing -- so what this answers is current until the caller's next run.
fn round_trip_graph(
    src: &str,
    make_roots: impl FnOnce(&mut Interpreter, &Root) -> Vec<Root>,
) -> (Interpreter, ClassRef, Vec<Value>, Vec<Value>, u32) {
    let (mut interp, class) = load(src);
    let roots = make_roots(&mut interp, &class);
    let platform = interp.platform();
    let class = class_ref(&interp, &class);
    let roots: Vec<Value> = roots.iter().map(|root| interp.fetch(root)).collect();
    let platform = interp.fetch(&platform);
    let mut symbols = symbols::Builder::new();
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, interp.heap_ref(), class, &mut symbols)
        .expect("encode code table");
    let mixin_ids = mixin_ids_for(class, &interp, &mut symbols, &ids);
    encode::encode_graph_section(
        &mut w,
        interp.heap_ref(),
        &roots,
        &platform,
        &ids,
        class,
        &mixin_ids,
        &mut symbols,
    )
    .expect("encode graph");
    let bytes = w.into_bytes();
    let symbol_table = symbols.build();
    let mut r = decode::Reader::new(&bytes);
    let code_table = decode::decode_code_table(&mut r, &symbol_table).expect("code table");
    let mixin_order = mixin_preorder(interp.heap_ref(), class);
    let decoded = {
        let mut ctx = decode::DecodeContext {
            interp: &mut interp,
            root_class: &class,
            codes: &code_table.codes,
            platform,
            mixins: &mixin_order,
        };
        decode::decode_graph_section(&mut r, &mut ctx, &symbol_table).expect("graph")
    };
    (
        interp,
        class,
        roots,
        decoded.roots,
        decoded.max_identity_hash,
    )
}

const EMPTY: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
    ) (
    ) : ()
"};

fn obj(v: &Value) -> &ns_runtime::ObjRef {
    let Value::Obj(o) = v else {
        panic!("expected an object")
    };
    o
}

#[test]
fn instances_and_arrays_round_trip_with_shared_identity_preserved() {
    let (interp, _c, original, decoded, _max) = round_trip_graph(EMPTY, |interp, _| {
        let shared = Value::Obj(interp.new_string("shared".to_string()));
        let array = interp.new_array(vec![shared, shared, Value::SmallInt(7)]);
        vec![interp.stash(Value::Obj(array))]
    });
    let heap = interp.heap_ref();
    let elements = heap
        .array_elements(*obj(&decoded[0]))
        .expect("expected an array");
    assert!(
        obj(&elements[0]).same(*obj(&elements[1])),
        "shared identity preserved"
    );
    assert!(
        !obj(&original[0]).same(*obj(&decoded[0])),
        "the decoded array is a new object"
    );
    assert!(matches!(
        heap.str_value(*obj(&elements[0])).as_deref(),
        Some("shared")
    ));
    assert!(matches!(elements[2], Value::SmallInt(7)));
}

#[test]
fn identity_hashes_are_preserved_and_the_maximum_is_reported() {
    // Two objects, the higher hash first: the object table is walked in
    // root order, so the maximum is *not* the last one seen and
    // `assert_eq!` can tell a real fold from a stale field or a running
    // counter.
    let (interp, _c, original, decoded, max) = round_trip_graph(EMPTY, |interp, _| {
        let first = interp.new_string("x".to_string());
        let second = interp.new_string("y".to_string());
        // Requested in this order, so `second` holds the higher hash.
        interp.heap_mut().ensure_identity_hash(first);
        interp.heap_mut().ensure_identity_hash(second);
        vec![
            interp.stash(Value::Obj(second)),
            interp.stash(Value::Obj(first)),
        ]
    });
    let heap = interp.heap_ref();
    let higher = heap.identity_hash(*obj(&original[0]));
    let lower = heap.identity_hash(*obj(&original[1]));
    assert!(higher > lower, "hashes are handed out in request order");
    assert_eq!(heap.identity_hash(*obj(&decoded[0])), higher);
    assert_eq!(heap.identity_hash(*obj(&decoded[1])), lower);
    assert_eq!(max, higher);
}

#[test]
fn class_references_resolve_to_the_same_live_class_objects() {
    let (interp, class, _o, decoded, _max) = round_trip_graph(EMPTY, |interp, class| {
        vec![
            interp.stash(Value::Obj(interp.builtin_class(BuiltinClass::Integer))),
            class.clone(),
        ]
    });
    assert!(obj(&decoded[0]).same(interp.builtin_class(BuiltinClass::Integer)));
    assert!(obj(&decoded[1]).same(class));
}

#[test]
fn the_platform_singleton_resolves_to_the_interpreters_own() {
    let (mut interp, _c, _o, decoded, _max) =
        round_trip_graph(EMPTY, |interp, _| vec![interp.platform()]);
    let platform = interp.platform();
    assert!(obj(&decoded[0]).same(*obj(&interp.fetch(&platform))));
}

#[test]
fn an_instance_of_the_root_class_keeps_its_class_and_fields() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | a = 1. b = 'two'. |
        ) (
        ) : ()
    "};
    let (interp, class, _o, decoded, _max) = round_trip_graph(src, |interp, class| {
        vec![expect_ok(interp.instantiate(class, &[]), "instantiate")]
    });
    let heap = interp.heap_ref();
    let snapshot = heap.object_snapshot(*obj(&decoded[0]));
    assert!(snapshot.class.same(class));
    let ObjectData::Instance(fields) = &snapshot.data else {
        panic!("expected an instance")
    };
    assert!(matches!(fields[0], Value::SmallInt(1)));
    assert!(matches!(
        heap.str_value(*obj(&fields[1])).as_deref(),
        Some("two")
    ));
}

#[test]
fn a_closure_and_its_paused_home_frame_round_trip() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          mk = ( ^[ 1 ] )
          public outer = ( ^mk )
        ) : ()
    "};
    let (interp, class, original, decoded, _max) = round_trip_graph(src, |interp, class| {
        let instance = expect_ok(interp.instantiate(class, &[]), "instantiate");
        let closure = expect_ok(
            interp.send(&instance, Symbol::intern("outer"), &[]),
            "outer",
        );
        // Synthetic "paused" state: edit the home frame's pc and grow its
        // register file before snapshotting.
        {
            let c = interp
                .heap_ref()
                .closure_data(*obj(&interp.fetch(&closure)))
                .expect("expected a closure");
            let (_, args_len, temps_len) = interp.heap_ref().frame_register_shape(c.home);
            interp.heap_mut().frame_grow_temps(c.home, temps_len + 1);
            interp.heap_mut().frame_set_local(
                c.home,
                (args_len + temps_len) as u16,
                Value::SmallInt(9),
            );
            interp.heap_mut().set_frame_pc(c.home, 1);
        }
        vec![closure]
    });
    let heap = interp.heap_ref();
    let original_code = {
        let c = heap
            .closure_data(*obj(&original[0]))
            .expect("expected a closure");
        let code = &c.code;
        code.clone()
    };
    let c = heap
        .closure_data(*obj(&decoded[0]))
        .expect("expected a closure");
    let code = &c.code;
    assert_eq!(**code, *original_code);
    assert!(
        c.captured_frame.same(c.home),
        "captured_frame and home were the same frame"
    );
    let home = heap.frame_snapshot(c.home);
    assert_eq!(home.pc, 1);
    assert!(matches!(home.temps.last(), Some(Value::SmallInt(9))));
    assert!(home.uncontinuable, "`mk` had returned");
    let methods = heap
        .class_application(class)
        .expect("not a class")
        .mixin
        .methods
        .clone();
    let FrameKind::Method(decl) = &home.kind else {
        panic!("home is a method frame")
    };
    assert!(
        Rc::ptr_eq(decl, &methods[&Symbol::intern("mk")]),
        "re-resolved against the class's method map"
    );
    assert!(heap.class_of_obj(*obj(&home.receiver)).same(class));
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
}

#[test]
fn a_closure_stored_in_a_slot_round_trips_with_an_initializer_home() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    let (interp, class, _o, decoded, _max) = round_trip_graph(src, |interp, class| {
        vec![expect_ok(interp.instantiate(class, &[]), "instantiate")]
    });
    let heap = interp.heap_ref();
    let held = heap
        .instance_field(*obj(&decoded[0]), 0)
        .expect("expected an instance");
    let c = heap
        .closure_data(*obj(&held))
        .expect("slot holds a closure");
    let home_kind = heap.frame_kind(c.home);
    let FrameKind::Initializer(home_class) = &home_kind else {
        panic!("home is the initializer frame")
    };
    assert!(home_class.same(class));
}

#[test]
fn the_graph_section_encoding_is_deterministic() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | a = 1. blk = [ 2 ]. s = 'three'. |
        ) (
        ) : ()
    "};
    let mut encodings = Vec::new();
    for _ in 0..2 {
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
        let platform = interp.platform();
        // Read out after the last run, for the encoder, which runs nothing.
        let class = class_ref(&interp, &class);
        let roots = vec![interp.fetch(&instance)];
        let platform = interp.fetch(&platform);
        let mut symbols = symbols::Builder::new();
        let mut w = encode::Writer::new();
        let ids = encode::encode_code_table(&mut w, interp.heap_ref(), class, &mut symbols)
            .expect("encode code table");
        let mixin_ids = mixin_ids_for(class, &interp, &mut symbols, &ids);
        encode::encode_graph_section(
            &mut w,
            interp.heap_ref(),
            &roots,
            &platform,
            &ids,
            class,
            &mixin_ids,
            &mut symbols,
        )
        .expect("encode graph");
        encodings.push(w.into_bytes());
    }
    assert_eq!(encodings[0], encodings[1]);
}

/// A world whose graph decoder can be fed hand-crafted bytes: an empty
/// class, no compiled code, and the interpreter's own `platform`.
struct GraphFixture {
    interp: Interpreter,
    class: ClassRef,
    platform: Value,
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
        // Nothing ever runs in a fixture's interpreter, so the class and the
        // platform read out of their roots here stay current.
        let (mut interp, class) = load(EMPTY);
        let class = class_ref(&interp, &class);
        let platform = interp.platform();
        let platform = interp.fetch(&platform);
        let mixins = mixin_preorder(interp.heap_ref(), class);
        GraphFixture {
            interp,
            class,
            platform,
            codes: Vec::new(),
            symbols: symbols::Table::empty(),
            mixins,
        }
    }

    /// `&mut self` because decoding a class record rebuilds its application
    /// through `Interpreter::apply_mixin`, which writes to the shared cache.
    fn decode(&mut self, bytes: &[u8]) -> Result<decode::DecodedGraph, decode::DecodeError> {
        let mut ctx = decode::DecodeContext {
            interp: &mut self.interp,
            root_class: &self.class,
            codes: &self.codes,
            platform: self.platform,
            mixins: &self.mixins,
        };
        decode::decode_graph_section(&mut decode::Reader::new(bytes), &mut ctx, &self.symbols)
    }

    /// The `DecodeError` a hand-crafted graph section must produce. Never
    /// `{:?}`s the success value: `DecodedGraph` has no `Debug` impl, so a
    /// manual match, as below, is the only way to reach the error.
    fn decode_err(&mut self, bytes: &[u8]) -> decode::DecodeError {
        match self.decode(bytes) {
            Ok(_) => panic!("expected a decode error, got a decoded graph"),
            Err(e) => e,
        }
    }
}

/// `object_count`, `frame_count` -- the two counts every graph section
/// starts with.
fn graph_counts(w: &mut encode::Writer, objects: u32, frames: u32) {
    w.write_u32(objects);
    w.write_u32(frames);
}

/// A one-object graph section whose single object is an array of one
/// element, written by `element`.
fn one_array_object(element: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Array));
    w.write_u32(5); // identity hash
    w.write_u8(3); // object body: Array
    w.write_u32(1); // one element
    element(&mut w);
    w.into_bytes()
}

#[test]
fn an_object_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_array_object(|w| {
        w.write_u8(5); // value tag: object-table id
        w.write_u32(99);
    });
    let err = GraphFixture::new().decode_err(&bytes);
    assert_eq!(err, decode::DecodeError::BadObjectId(99));
    assert_eq!(err.to_string(), "object id 99 out of range");
}

#[test]
fn a_frame_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(true); // has a sender...
    w.write_u32(99); // ...whose id is out of range
    let bytes = w.into_bytes();
    let err = GraphFixture::new().decode_err(&bytes);
    assert_eq!(err, decode::DecodeError::BadFrameId(99));
    assert_eq!(err.to_string(), "frame id 99 out of range");
}

#[test]
fn a_huge_object_or_frame_count_does_not_allocate_gigabytes() {
    let mut fixture = GraphFixture::new();
    for (objects, frames) in [(u32::MAX, 0), (0, u32::MAX)] {
        let mut w = encode::Writer::new();
        graph_counts(&mut w, objects, frames);
        let bytes = w.into_bytes();
        assert_eq!(
            fixture.decode_err(&bytes),
            decode::DecodeError::UnexpectedEof
        );
    }
}

#[test]
fn an_invalid_value_tag_in_the_graph_is_a_decode_error_not_a_panic() {
    let bytes = one_array_object(|w| w.write_u8(200));
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::InvalidTag {
            what: "value",
            tag: 200
        }
    );
}

#[test]
fn an_invalid_object_body_tag_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    w.write_u8(0);
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    w.write_u32(1);
    w.write_u8(200); // no such object body
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::InvalidTag {
            what: "object body",
            tag: 200
        }
    );
}

#[test]
fn an_invalid_class_tag_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    w.write_u8(200); // none of `Builtin` (0), `UserRoot` (1), `Record` (2)
                     // Padded out to the shortest object record the wire admits, so
                     // `bounded_count` admits the announced record and the class tag itself
                     // is what the decoder rejects.
    w.write_u32(1); // identity hash
    w.write_u8(1); // object body: Str
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
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
/// `Interpreter::new_class_object` gives every class object), an identity
/// hash, the body tag, the mixin-table row id, then the three references
/// `refs` writes.
fn class_record(w: &mut encode::Writer, mixin_id: u32, refs: impl FnOnce(&mut encode::Writer)) {
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
    w.write_u32(7); // identity hash
    w.write_u8(5); // object body: Class
    w.write_u32(mixin_id);
    refs(w);
}

/// A graph section holding one class record and no roots.
fn one_class_object(mixin_id: u32, refs: impl FnOnce(&mut encode::Writer)) -> Vec<u8> {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    class_record(&mut w, mixin_id, refs);
    w.write_u32(0); // no roots
    w.into_bytes()
}

/// `superclass`, `enclosing` and `declaring_class` all absent.
fn no_class_refs(w: &mut encode::Writer) {
    w.write_bool(false);
    w.write_bool(false);
    w.write_bool(false);
}

#[test]
fn a_well_formed_class_record_decodes_into_a_live_class_object() {
    // The positive side of the four refusals below: without it a decoder
    // that rejected every class record would pass all of them.
    let mut fixture = GraphFixture::new();
    let graph = fixture
        .decode(&one_class_object(0, no_class_refs))
        .expect("a class record naming row 0 of the mixin table");
    let application = fixture
        .interp
        .heap_ref()
        .class_application(graph.objects[0])
        .expect("a class object");
    assert!(
        Rc::ptr_eq(&application.mixin, &fixture.mixins[0]),
        "the record's mixin id names the decoded declaration tree, row by row"
    );
}

#[test]
fn two_class_records_with_the_same_mixin_and_superclass_share_one_application() {
    // The decoder's own half of the round-trip sharing property: nothing in
    // the two records ties them together, so if `apply_mixin`'s cache did
    // not answer both, sharing could only come back by accident.
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 2, 0);
    class_record(&mut w, 0, no_class_refs);
    class_record(&mut w, 0, no_class_refs);
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    let mut fixture = GraphFixture::new();
    let graph = fixture.decode(&bytes).expect("two class records");
    let a = fixture
        .interp
        .heap_ref()
        .class_application(graph.objects[0])
        .expect("a class object");
    let b = fixture
        .interp
        .heap_ref()
        .class_application(graph.objects[1])
        .expect("a class object");
    assert!(!graph.objects[0].same(graph.objects[1]));
    assert!(Rc::ptr_eq(&a, &b), "one rebuilt application, shared");
}

#[test]
fn a_class_records_mixin_id_out_of_range_is_a_decode_error_not_a_panic() {
    assert_eq!(
        GraphFixture::new().decode_err(&one_class_object(7, no_class_refs)),
        decode::DecodeError::BadMixinId(7)
    );
}

#[test]
fn a_class_records_superclass_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |w| {
        w.write_bool(true);
        w.write_u8(2); // class tag: object-table record
        w.write_u32(9); // ... past the end of a one-object table
        w.write_bool(false);
        w.write_bool(false);
    });
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_records_enclosing_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |w| {
        w.write_bool(false);
        w.write_bool(true);
        w.write_u8(5); // value tag: object-table id
        w.write_u32(9);
        w.write_bool(false);
    });
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_records_enclosing_scalar_is_a_decode_error_not_a_silent_nil() {
    let bytes = one_class_object(0, |w| {
        w.write_bool(false);
        w.write_bool(true);
        w.write_u8(3); // value tag: SmallInt -- not an object at all
        w.write_i64(1);
        w.write_bool(false);
    });
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::EnclosingNotAnObject
    );
}

#[test]
fn a_class_records_declaring_class_id_out_of_range_is_a_decode_error_not_a_panic() {
    let bytes = one_class_object(0, |w| {
        w.write_bool(false);
        w.write_bool(false);
        w.write_bool(true);
        w.write_u8(2); // class tag: object-table record
        w.write_u32(9);
    });
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::BadObjectId(9)
    );
}

#[test]
fn a_class_reference_naming_a_record_that_is_not_a_class_is_a_decode_error() {
    // An in-range id is not enough: nothing in the stream forces a tag-2
    // class tag to name a *class* record, and a class object whose
    // superclass is an array has no application chain to rebuild.
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 2, 0);
    class_record(&mut w, 0, |w| {
        w.write_bool(true);
        w.write_u8(2);
        w.write_u32(1); // object 1, which is an array
        w.write_bool(false);
        w.write_bool(false);
    });
    w.write_u8(0); // object 1's class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Array));
    w.write_u32(1); // identity hash
    w.write_u8(3); // object body: Array
    w.write_u32(0); // no elements
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::NotAClassReference
    );
}

#[test]
fn class_objects_inheriting_from_each_other_in_a_cycle_are_refused() {
    // An application is derived from the superclass's, so a cycle has no
    // order in which the two could be rebuilt. Refusing is what keeps the
    // rebuild a finite walk rather than one that never ends.
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 2, 0);
    for other in [1u32, 0] {
        class_record(&mut w, 0, move |w| {
            w.write_bool(true);
            w.write_u8(2);
            w.write_u32(other);
            w.write_bool(false);
            w.write_bool(false);
        });
    }
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::SuperclassCycle
    );
}

/// A one-frame graph section up to (but excluding) the frame-kind tag.
fn one_frame_prefix() -> encode::Writer {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w
}

#[test]
fn an_invalid_frame_kind_tag_is_a_decode_error_not_a_panic() {
    let mut w = one_frame_prefix();
    w.write_u8(200);
    // Padded out to the shortest frame record the wire admits (see the
    // class-tag test), so the kind tag is what the decoder rejects.
    w.write_bool(false); // continuable
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: 200
        }
    );
}

/// The byte after `pc` is the frame's `sender` flag: a frame record no longer
/// says what its `pc` counts, because an image's code table holds only
/// bytecode. A record written in the old layout -- the retired pc-kind byte
/// after `pc` -- therefore has that byte read as "a sender follows", and the
/// next four bytes (the old sender, home and current-class flags and the kind
/// tag) as a sender id far outside the one-frame table, so it is refused
/// rather than read as a different frame.
#[test]
fn a_frame_record_in_the_old_layout_is_refused() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_u8(1); // the retired pc-kind byte (bytecode)
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(2); // frame kind: TopLevel
    w.write_bool(false); // continuable
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    assert!(GraphFixture::new().decode(&bytes).is_err());
}

#[test]
fn a_method_frame_naming_a_selector_the_class_lost_is_a_decode_error() {
    let mut w = one_frame_prefix();
    w.write_u8(0); // frame kind: Method
    w.write_u32(0); // selector id 0 ("gone")
    let bytes = w.into_bytes();
    let mut fixture = GraphFixture::new();
    fixture.symbols = table_with(&["gone"]);
    let err = fixture.decode_err(&bytes);
    assert_eq!(
        err,
        decode::DecodeError::UnknownSelector("gone".to_string())
    );
}

#[test]
fn a_closure_objects_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    w.write_u32(1); // identity hash
    w.write_u8(4); // object body: Closure
    w.write_u16(0); // no parameters
    w.write_u32(42); // code id, out of range for an empty code table
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::BadCodeId(42)
    );
}

/// A closure frame names its closure object by id, and an id past the
/// object table is refused where it is read, before anything indexes by it.
#[test]
fn a_closure_frames_object_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut w = one_frame_prefix();
    w.write_u8(6); // frame kind: FRAME_CLOSURE_OBJECT...
    w.write_u32(42); // ...naming an object past the empty object table
    w.write_bool(false); // continuable
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
        decode::DecodeError::BadObjectId(42)
    );
}

#[test]
fn a_truncated_graph_section_is_a_decode_error_not_a_panic() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let platform = interp.platform();
    // Read out after the last run, for the encoder and the decoder, which run
    // nothing.
    let class = class_ref(&interp, &class);
    let roots = vec![interp.fetch(&instance)];
    let platform = interp.fetch(&platform);
    let mut symbols = symbols::Builder::new();
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, interp.heap_ref(), class, &mut symbols)
        .expect("encode code table");
    let before = w.into_bytes().len();
    let mut w = encode::Writer::new();
    let mixin_ids = mixin_ids_for(class, &interp, &mut symbols, &ids);
    encode::encode_graph_section(
        &mut w,
        interp.heap_ref(),
        &roots,
        &platform,
        &ids,
        class,
        &mixin_ids,
        &mut symbols,
    )
    .expect("encode graph");
    let graph = w.into_bytes();
    let symbol_table = symbols.build();
    assert!(before > 0 && graph.len() > 8);
    let mixin_order = mixin_preorder(interp.heap_ref(), class);
    let mut ctx = decode::DecodeContext {
        interp: &mut interp,
        root_class: &class,
        codes: &[],
        platform,
        mixins: &mixin_order,
    };
    for cut in 1..graph.len() {
        let mut r = decode::Reader::new(&graph[..cut]);
        // Every prefix is either a clean `DecodeError` or -- for a prefix
        // that happens to be self-consistent -- a successful decode. What
        // it must never be is a panic.
        let _ = decode::decode_graph_section(&mut r, &mut ctx, &symbol_table);
    }
}

#[test]
fn a_closure_capturing_a_block_activation_round_trips() {
    // `[:x | [ x ] ] value: 5` runs the outer block, so the inner closure's
    // `captured_frame` is a *block activation* -- a `FrameKind::Closure`
    // frame, the one frame kind the quiesced worlds above never produce.
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (interp, class, _o, decoded, _max) = round_trip_graph(src, |interp, class| {
        let instance = expect_ok(interp.instantiate(class, &[]), "instantiate");
        vec![expect_ok(
            interp.send(&instance, Symbol::intern("mk"), &[]),
            "mk",
        )]
    });
    let heap = interp.heap_ref();
    let inner = heap
        .closure_data(*obj(&decoded[0]))
        .expect("expected the inner closure");
    assert!(
        !inner.captured_frame.same(inner.home),
        "the inner closure was captured by the block activation, not by `mk`"
    );
    let captured = heap.frame_snapshot(inner.captured_frame);
    let FrameKind::Closure(_) = &captured.kind else {
        panic!("the captured frame is a block activation")
    };
    assert!(matches!(captured.args.first(), Some(Value::SmallInt(5))));
    assert!(
        captured
            .home
            .as_ref()
            .expect("a block activation has a home")
            .same(inner.home),
        "both closures share `mk`'s activation as their home"
    );
    let home_kind = heap.frame_kind(inner.home);
    let FrameKind::Method(decl) = &home_kind else {
        panic!("the home is `mk`'s method frame")
    };
    let application = heap.class_application(class).expect("not a class");
    assert!(Rc::ptr_eq(
        decl,
        &application.mixin.methods[&Symbol::intern("mk")]
    ));
}

/// A closure frame names the closure object it runs, rather than carrying a
/// copy of its record: after a round trip the inner closure's captured frame
/// -- the outer block's activation -- names a decoded closure object, and it
/// is the outer block, with its one parameter.
#[test]
fn a_closure_frame_names_the_closure_object_it_runs() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (interp, _class, _o, decoded, _max) = round_trip_graph(src, |interp, class| {
        let instance = expect_ok(interp.instantiate(class, &[]), "instantiate");
        vec![expect_ok(
            interp.send(&instance, Symbol::intern("mk"), &[]),
            "mk",
        )]
    });
    let heap = interp.heap_ref();
    let inner = heap
        .closure_data(*obj(&decoded[0]))
        .expect("expected the inner closure");
    let FrameKind::Closure(outer) = heap.frame_kind(inner.captured_frame) else {
        panic!("the captured frame is a block activation")
    };
    let outer = heap
        .closure_data(outer)
        .expect("a closure frame names a closure object");
    assert_eq!(outer.params.len(), 1, "the outer block takes `x`");
    assert!(
        outer.home.same(inner.home),
        "both closures share `mk`'s activation as their home"
    );
}

/// The frame kind a stale artifact wrote for a closure activation, before
/// the record named the closure object. Its number is retired: the decoder
/// refuses it rather than read the old inline layout as the new one.
#[test]
fn the_retired_closure_frame_tag_is_refused() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(1); // frame kind: the retired FRAME_CLOSURE
                   // Padded past the shortest frame record the wire admits, so
                   // `bounded_count` admits the announced frame and the kind tag itself is
                   // what the decoder rejects.
    w.write_bool(false);
    w.write_u32(0);
    assert_eq!(
        GraphFixture::new().decode_err(&w.into_bytes()),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: 1
        }
    );
}

/// `inspect` walks the graph with its own reader, so it has to refuse the
/// retired tag itself: an artifact `load` refuses must not inspect as if it
/// were well-formed. The describing sections come from a real snapshot and
/// the graph section is the hand-written one above, spliced in where the
/// snapshot's own graph began.
#[test]
fn inspect_refuses_the_retired_closure_frame_tag() {
    let (mut interp, class) = load(TRIVIAL_MODULE);
    let platform = interp.platform();
    let mut bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot");
    let graph_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|s| s.name == "graph")
        .expect("a snapshot has a graph section")
        .start;
    bytes.truncate(graph_start);
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(1); // frame kind: the retired FRAME_CLOSURE
    w.write_bool(false);
    w.write_u32(0);
    bytes.extend_from_slice(&w.into_bytes());
    let err = match image::inspect::inspect(&bytes) {
        Ok(_) => panic!("inspect accepted the retired closure frame tag"),
        Err(e) => e,
    };
    assert_eq!(err, "invalid frame kind tag 1");
    assert_eq!(load_err(&bytes), "invalid frame kind tag 1");
}

/// `inspect` renders a closure frame as the object id it names: after
/// `[:x | [ x ] ] value: 5`, the inner closure's captured frame is the outer
/// block's activation, and the object the frame line names is the outer
/// block itself, with its one parameter.
#[test]
fn inspect_names_the_closure_object_a_closure_frame_runs() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let inner = expect_ok(interp.send(&instance, Symbol::intern("mk"), &[]), "mk");
    let platform = interp.platform();
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&inner)],
    )
    .expect("snapshot");
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
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 1);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Str));
    w.write_u32(1); // identity hash
    w.write_u8(1); // object body: Str
    w.write_str("not a closure");
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(true); // a home...
    w.write_u32(0); // ...frame 0 itself
    w.write_bool(false); // no current class
    w.write_u8(6); // frame kind: FRAME_CLOSURE_OBJECT...
    w.write_u32(0); // ...naming object 0, the string
    w.write_bool(false); // continuable
    w.write_u32(0); // no roots
    let err = GraphFixture::new().decode_err(&w.into_bytes());
    assert_eq!(err, decode::DecodeError::NotAClosureReference);
    assert_eq!(
        err.to_string(),
        "a closure frame in the image names an object that is not a closure"
    );
}

#[test]
fn a_top_level_frame_kind_decodes() {
    // Nothing that runs a program *builds* a `FrameKind::TopLevel` frame (see
    // `types::FrameKind`), so only a hand-written section can reach the
    // decoder's tag-2 branch.
    let mut w = one_frame_prefix();
    w.write_u8(2); // frame kind: TopLevel
    w.write_bool(false); // continuable
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    let decoded = GraphFixture::new()
        .decode(&bytes)
        .expect("a TopLevel frame");
    assert!(decoded.roots.is_empty());
    assert_eq!(decoded.max_identity_hash, 0);
}

/// A block `CompiledCode` taking `num_args` arguments in `num_args + 1`
/// registers -- enough for the graph decoder, which never runs it.
fn block_code(num_args: u16) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instrs: vec![Instr::ImplicitReturn { reg: 0 }],
        num_args,
        num_registers: num_args + 1,
        statement_pc: vec![],
        call_sites: vec![],
        source: Span::new(0, 0),
    })
}

/// A graph section holding one closure object and the one `TopLevel` frame
/// it points at, with `params` declared parameters against code id 0.
/// Every field but the parameter count is well-formed, so *only* the
/// arity reconciliation can reject it.
fn closure_object_with_param_count(params: &[&str]) -> Vec<u8> {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 1);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    w.write_u32(1); // identity hash
    w.write_u8(4); // object body: Closure
    w.write_u16(params.len() as u16);
    for (id, _) in params.iter().enumerate() {
        // `table_with(params)` interns them in this same order, so a
        // parameter's position is its id.
        w.write_u32(id as u32);
    }
    w.write_u32(0); // code id
    w.write_u32(0); // captured frame id -- frame 0 exists
    w.write_u32(0); // home frame id -- frame 0 exists
    w.write_u8(0); // frame 0: receiver nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(2); // frame kind: TopLevel
    w.write_bool(false); // continuable
    w.write_u32(1); // one root...
    w.write_u8(5);
    w.write_u32(0); // ...the closure
    w.into_bytes()
}

#[test]
fn a_well_formed_closure_record_whose_arity_agrees_still_decodes() {
    // The control for the two rejection tests below: the same section
    // shape, with the parameter count the code actually declares. Without
    // it, a reconciliation that rejected *everything* would look correct.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(1));
    fixture.symbols = table_with(&["x"]);
    let decoded = fixture
        .decode(&closure_object_with_param_count(&["x"]))
        .expect("arity agrees");
    let c = fixture
        .interp
        .heap_ref()
        .closure_data(*obj(&decoded.roots[0]))
        .expect("expected a closure");
    assert_eq!(*c.params, vec![Symbol::intern("x")]);
}

#[test]
fn a_closure_record_declaring_too_few_parameters_is_a_decode_error() {
    // `push_step` checks the *call's* arity against `params.len()`
    // but sizes the register file from the *code* (`num_registers -
    // num_args`), and `locals::read_local` then indexes `temps[i - args.len()]`
    // unchecked. So this section -- otherwise entirely well-formed, as the
    // control above proves -- would decode cleanly and panic the process
    // the first time the closure is sent `value`. `decode_closure_record`
    // is the only place that sees both halves of the arity.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(1));
    let err = fixture.decode_err(&closure_object_with_param_count(&[]));
    assert_eq!(
        err,
        decode::DecodeError::ClosureArityMismatch {
            params: 0,
            num_args: 1
        }
    );
    assert_eq!(
        err.to_string(),
        "closure record declares 0 parameters but the code it names takes 1"
    );
}

#[test]
fn a_closure_record_declaring_too_many_parameters_is_a_decode_error() {
    // The mirror case does not panic -- it silently misaddresses temps as
    // arguments -- so nothing downstream would ever report it.
    let mut fixture = GraphFixture::new();
    fixture.codes.push(block_code(0));
    fixture.symbols = table_with(&["x"]);
    let err = fixture.decode_err(&closure_object_with_param_count(&["x"]));
    assert_eq!(
        err,
        decode::DecodeError::ClosureArityMismatch {
            params: 1,
            num_args: 0
        }
    );
}

// ---------------------------------------------------------------------------
// Whole-image `snapshot`/`load`.
// ---------------------------------------------------------------------------

#[test]
fn a_snapshot_loads_into_a_bytecode_interpreter_with_its_state_and_a_safe_hash_counter() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Counter = Object new (
          | n ::= 0. |
        ) (
          public bump = ( n := n + 1. ^n )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    expect_ok(interp.send(&instance, Symbol::intern("bump"), &[]), "bump");
    let instance_obj = *obj(&interp.fetch(&instance));
    let max_hash = interp.heap_mut().ensure_identity_hash(instance_obj);
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let root_class = class_ref(&loaded.interp, &loaded.root_class);
    assert!(
        !root_mixin_of(loaded.interp.heap_ref(), root_class)
            .methods
            .is_empty(),
        "the image's methods are rebuilt, each carrying its body"
    );
    assert!(loaded
        .interp
        .heap_ref()
        .class_of_obj(*obj(&loaded.interp.fetch(&loaded.roots[0])))
        .same(root_class));
    let n = expect_ok(
        loaded
            .interp
            .send(&loaded.roots[0], Symbol::intern("bump"), &[]),
        "bump",
    );
    assert!(
        matches!(loaded.interp.fetch(&n), Value::SmallInt(2)),
        "slot state survived: 1 before the snapshot, 2 after"
    );
    let fresh = loaded.interp.new_string("fresh".to_string());
    assert!(
        loaded.interp.heap_mut().ensure_identity_hash(fresh) > max_hash,
        "no hash collision with loaded objects"
    );
}

#[test]
fn bytes_after_the_last_section_are_refused_not_ignored() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let mut bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot");
    assert!(image::load(&bytes).is_ok(), "the fixture itself must load");

    bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let err = load_err(&bytes);
    assert!(
        err.contains("trailing"),
        "expected a trailing-bytes error, got: {err}"
    );
}

#[test]
fn load_refuses_a_version_mismatch() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let mut bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .unwrap();
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    let err = load_err(&bytes);
    assert!(err.contains("version"), "{err}");
}

/// `image::load`'s error message. Never `expect_err`: that would require
/// `Debug` on the `Ok` side, and `LoadedImage` has none -- it carries an
/// `Interpreter`, which has no `Debug` impl either.
fn load_err(bytes: &[u8]) -> String {
    match image::load(bytes) {
        Ok(_) => panic!("expected a load error, got a loaded image"),
        Err(e) => e,
    }
}

#[test]
fn the_source_origin_round_trips_through_the_image() {
    let mut w = encode::Writer::new();
    encode::encode_source_origin(&mut w, &Origin::File(PathBuf::from("some/where.ns")));
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    let SourceOrigin::File(path) = decode::decode_source_origin(&mut r).expect("origin");
    assert_eq!(path, PathBuf::from("some/where.ns"));
}

#[test]
fn an_invalid_source_origin_tag_is_a_decode_error_not_a_panic() {
    let bytes = vec![200u8];
    let mut r = decode::Reader::new(&bytes);
    // `SourceOrigin` is not `PartialEq`, so this matches rather than
    // comparing -- and names the variant it expects either way.
    match decode::decode_source_origin(&mut r) {
        Err(decode::DecodeError::InvalidTag {
            what: "source origin",
            tag: 200,
        }) => {}
        Err(e) => panic!("expected InvalidTag{{source origin, 200}}, got: {e}"),
        Ok(SourceOrigin::File(path)) => panic!("expected an error, got origin {}", path.display()),
    }
}

#[test]
fn every_truncation_of_an_image_is_reported_rather_than_panicking() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | blk = [ 2 ]. |
        ) (
          public run = ( ^blk value )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");
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
    num_args: u16,
    num_registers: u16,
    instrs: Vec<Instr>,
) -> Rc<CompiledCode> {
    Rc::new(CompiledCode {
        kind,
        instrs,
        num_args,
        num_registers,
        statement_pc: vec![],
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
    let (interp, class) = load(RICH);
    let class = class_ref(&interp, &class);
    let (table, _decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    decode::verify_code_table(&table).expect("compiler-emitted code is structurally sound");
}

#[test]
fn a_jump_target_past_the_end_of_its_body_is_refused() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![Instr::Jump { target: 7 }],
    )]);
    let err = decode::verify_code_table(&table).expect_err("out-of-range jump");
    assert_eq!(
        err,
        decode::DecodeError::JumpTargetOutOfRange {
            target: 7,
            instrs: 1
        }
    );
    assert_eq!(
        err.to_string(),
        "jump target 7 is not an instruction index in a body of 1 instruction(s)"
    );
}

#[test]
fn every_conditional_jump_target_is_checked_too() {
    // One case per jump-carrying instruction: a check that only covered
    // `Jump` would leave three unchecked `pc` writes in the VM.
    for instr in [
        Instr::JumpIfNotBoolean { reg: 0, target: 9 },
        Instr::BranchIfFalse { reg: 0, target: 9 },
        Instr::BranchIfTrue { reg: 0, target: 9 },
    ] {
        let table = table_of(vec![code_with(
            CodeKind::Method,
            0,
            1,
            vec![instr, Instr::Return { reg: 0 }],
        )]);
        assert_eq!(
            decode::verify_code_table(&table),
            Err(decode::DecodeError::JumpTargetOutOfRange {
                target: 9,
                instrs: 2
            })
        );
    }
}

#[test]
fn a_register_past_the_bodys_register_file_is_refused() {
    // `read_local`/`write_local` split at `args.len()` and index `temps`
    // unchecked, so register 5 in a two-register body is an out-of-bounds
    // index, not a wrong answer.
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        2,
        vec![Instr::Move { dst: 5, src: 0 }, Instr::Return { reg: 0 }],
    )]);
    let err = decode::verify_code_table(&table).expect_err("out-of-range register");
    assert_eq!(
        err,
        decode::DecodeError::RegisterOutOfRange {
            reg: 5,
            num_registers: 2
        }
    );
    assert_eq!(
        err.to_string(),
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
            Instr::SendPublic {
                selector: Symbol::intern("foo:"),
                receiver_reg: 0,
                arg_regs: vec![0, 4],
                dst: 0,
                call_site: CallSite { call_site_id: 0 },
            },
            Instr::Return { reg: 0 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            reg: 4,
            num_registers: 1
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
            Instr::SendInlinable {
                selector: Symbol::intern("ifTrue:"),
                receiver_reg: 0,
                arg_regs: vec![4],
                dst: 0,
                call_site: CallSite { call_site_id: 0 },
            },
            Instr::Return { reg: 0 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            reg: 4,
            num_registers: 1
        })
    );
}

#[test]
fn a_body_that_runs_past_its_last_instruction_is_refused() {
    // The VM fetches `code.instrs[pc]` after every fall-through, so a
    // body whose last instruction is not a return or a jump indexes one
    // past the end. An empty body indexes past the end immediately.
    for instrs in [vec![], vec![Instr::LoadSelf { dst: 0 }]] {
        let table = table_of(vec![code_with(CodeKind::Method, 0, 1, instrs)]);
        assert_eq!(
            decode::verify_code_table(&table),
            Err(decode::DecodeError::UnterminatedBody)
        );
    }
}

#[test]
fn more_arguments_than_registers_is_refused() {
    // Every activation is pushed with `num_registers - num_args`
    // temporaries; that `u16` subtraction underflows here.
    let table = table_of(vec![code_with(
        CodeKind::Method,
        3,
        1,
        vec![Instr::Return { reg: 0 }],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::ArgsExceedRegisters {
            num_args: 3,
            num_registers: 1
        })
    );
}

#[test]
fn a_make_closure_naming_a_method_body_is_refused() {
    // A closure activation is a `FrameKind::Closure` frame, and
    // `frame_at_depth` only hops through those -- so running a method body
    // as a block body would break the lexical-chain reasoning the outer
    // local check below rests on.
    let method = code_with(CodeKind::Method, 0, 1, vec![Instr::Return { reg: 0 }]);
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: method.clone(),
            },
            Instr::Return { reg: 0 },
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
    // The `MakeClosure` mirror of `ClosureArityMismatch`: `params` and the
    // body's `num_args` are separate wire fields here too, and
    // `push_step` reads one while the frame is sized from the other.
    let block = code_with(
        CodeKind::Block,
        1,
        2,
        vec![Instr::ImplicitReturn { reg: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: block.clone(),
            },
            Instr::Return { reg: 0 },
        ],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::ClosureArityMismatch {
            params: 0,
            num_args: 1
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
        vec![Instr::ImplicitReturn { reg: 0 }],
    );
    let make = |dst| Instr::MakeClosure {
        dst,
        params: Rc::new(vec![]),
        code: block.clone(),
    };
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![make(0), make(1), Instr::Return { reg: 0 }],
    );
    let table = table_of(vec![block, parent]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::SharedBlockBody)
    );
}

#[test]
fn a_make_closure_naming_a_body_outside_the_table_is_refused() {
    // `decode_instr` resolves a `MakeClosure`'s code id against the table,
    // but `DecodedCodeTable` is a plain public struct: an `Rc` from
    // somewhere else must not be trusted to have been verified.
    let stranger = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instr::ImplicitReturn { reg: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: stranger,
            },
            Instr::Return { reg: 0 },
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
            Instr::LoadOuterLocal {
                dst: 0,
                depth: outer_depth,
                index: outer_index,
            },
            Instr::ImplicitReturn { reg: 0 },
        ],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        parent_registers,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: block.clone(),
            },
            Instr::Return { reg: 0 },
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
    // own `num_registers` says nothing about it: the bound is the register
    // file of the body one `MakeClosure` hop out.
    let err = decode::verify_code_table(&outer_local_table(2, 1, 3)).expect_err("out of range");
    assert_eq!(
        err,
        decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 3,
            num_registers: 2
        }
    );
    assert_eq!(
        err.to_string(),
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
            num_registers: 1
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
            num_registers: 2
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
            Instr::StoreOuterLocal {
                src: 0,
                depth: 1,
                index: 6,
            },
            Instr::ImplicitReturn { reg: 0 },
        ],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: block.clone(),
            },
            Instr::Return { reg: 0 },
        ],
    );
    assert_eq!(
        decode::verify_code_table(&table_of(vec![block, parent])),
        Err(decode::DecodeError::OuterLocalOutOfRange {
            depth: 1,
            index: 6,
            num_registers: 2
        })
    );
}

// ---------------------------------------------------------------------------
// `verify_mixins`'s arity and kind reconciliation.
// ---------------------------------------------------------------------------

#[test]
fn a_method_body_taking_the_wrong_number_of_arguments_is_refused_on_load() {
    // A method frame's `args` come from the send, so their count is the
    // selector's arity; the frame's temps are sized from the *body's*
    // `num_args`. A body claiming a different arity leaves `read_local`
    // indexing `temps` past its end.
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo: a = ( ^a )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let class = class_ref(&interp, &class);
    let body = CompiledCode {
        num_args: 2,
        ..(*method_code(interp.heap_ref(), class, "foo:")).clone()
    };
    let doctored = with_method_code(mixin_of(interp.heap_ref(), class), "foo:", Rc::new(body));
    install_mixin(&mut interp, class, doctored);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let err = decode::verify_mixins(&decoded_mixins).expect_err("arity mismatch");
    assert_eq!(
        err,
        decode::DecodeError::CodeArityMismatch {
            what: "method `foo:`".to_string(),
            params: 1,
            num_args: 2
        }
    );
}

#[test]
fn a_method_root_naming_a_block_body_is_refused_on_load() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public foo = ( ^1 )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let class = class_ref(&interp, &class);
    let body = CompiledCode {
        kind: CodeKind::Block,
        ..(*method_code(interp.heap_ref(), class, "foo")).clone()
    };
    let doctored = with_method_code(mixin_of(interp.heap_ref(), class), "foo", Rc::new(body));
    install_mixin(&mut interp, class, doctored);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    assert_eq!(
        decode::verify_mixins(&decoded_mixins),
        Err(decode::DecodeError::WrongCodeKind { expected: "method" })
    );
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
    // Nothing ever runs in a fixture's interpreter, so the class and the
    // platform read out of their roots here stay current.
    let (mut interp, class) = load(ONE_METHOD);
    let class = class_ref(&interp, &class);
    let platform = interp.platform();
    let platform = interp.fetch(&platform);
    let block = code_with(
        CodeKind::Block,
        0,
        1,
        vec![Instr::ImplicitReturn { reg: 0 }],
    );
    let parent = code_with(
        CodeKind::Method,
        0,
        2,
        vec![
            Instr::MakeClosure {
                dst: 0,
                params: Rc::new(vec![]),
                code: block.clone(),
            },
            Instr::Return { reg: 0 },
        ],
    );
    let table = decode::DecodedCodeTable {
        codes: vec![block, parent.clone()],
    };
    let doctored = with_method_code(mixin_of(interp.heap_ref(), class), "foo", parent.clone());
    install_mixin(&mut interp, class, doctored);
    let mixins = mixin_preorder(interp.heap_ref(), class);
    let fixture = GraphFixture {
        codes: table.codes.clone(),
        interp,
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
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 1);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Block));
    w.write_u32(1); // identity hash
    w.write_u8(4); // object body: Closure
    w.write_u16(0); // no parameters
    w.write_u32(0); // code id 0 -- the block body
    w.write_u32(0); // captured frame id
    w.write_u32(0); // home frame id
    frame(&mut w);
    w.write_u32(1); // one root...
    w.write_u8(5);
    w.write_u32(0); // ...the closure
    w.into_bytes()
}

/// Frame 0 as an activation of method `foo` with `temps` temporaries.
fn foo_frame(temps: u32) -> impl FnOnce(&mut encode::Writer) {
    foo_frame_at_pc(temps, 0)
}

/// `foo_frame` with the activation's `pc` left to the caller.
fn foo_frame_at_pc(temps: u32, pc: u32) -> impl FnOnce(&mut encode::Writer) {
    move |w: &mut encode::Writer| {
        w.write_u8(0); // receiver: nil
        w.write_u32(0); // no args
        w.write_u32(temps);
        for _ in 0..temps {
            w.write_u8(0); // each temp: nil
        }
        w.write_u32(pc);
        w.write_bool(false); // no sender
        w.write_bool(false); // no home
        w.write_bool(false); // no current class
        w.write_u8(0); // frame kind: Method
        w.write_u32(0); // selector id 0 -- "foo", per `closure_world`'s fixture
        w.write_bool(false); // continuable
    }
}

#[test]
fn a_closure_captured_by_its_lexical_parents_activation_verifies() {
    // The control for the two rejections below: a decoded closure whose
    // captured frame really is an activation of the body its own body is
    // nested in, sized exactly as that body prescribes.
    let (mut fixture, table) = closure_world();
    let graph = fixture
        .decode(&closure_over_frame(foo_frame(2)))
        .expect("decode");
    decode::verify_graph(&graph, &table, fixture.interp.heap_ref())
        .expect("the captured frame runs `foo`");
}

#[test]
fn a_frame_whose_register_file_is_too_small_for_its_code_is_refused() {
    // `foo`'s body has two registers; this activation has one. A
    // `LoadOuterLocal` of r1 from the closure would index `temps[1]` of a
    // one-element `temps` -- an unchecked out-of-bounds index in
    // `locals::read_local`, reached from a version-matching image.
    let (mut fixture, table) = closure_world();
    let graph = fixture
        .decode(&closure_over_frame(foo_frame(1)))
        .expect("decode");
    let err = decode::verify_graph(&graph, &table, fixture.interp.heap_ref())
        .expect_err("undersized frame");
    assert_eq!(
        err,
        decode::DecodeError::FrameRegisterFile {
            args: 0,
            locals: 1,
            num_args: 0,
            num_registers: 2
        }
    );
    assert_eq!(
        err.to_string(),
        "a decoded frame holds 0 argument(s) and 1 local(s), but the code that runs in it takes 0 argument(s) in 2 register(s)"
    );
}

#[test]
fn a_closure_captured_by_a_frame_that_does_not_run_its_lexical_parent_is_refused() {
    // The frame is well-formed on its own; what is crafted is the *link*.
    // Nothing else in the format ties a closure's captured frame to its
    // body's lexical parent, and the whole outer-local bound rests on it.
    let (mut fixture, table) = closure_world();
    let graph = fixture
        .decode(&closure_over_frame(|w: &mut encode::Writer| {
            w.write_u8(0); // receiver: nil
            w.write_u32(0); // no args
            w.write_u32(0); // no temps
            w.write_u32(0); // pc
            w.write_bool(false); // no sender
            w.write_bool(false); // no home
            w.write_bool(false); // no current class
            w.write_u8(2); // frame kind: TopLevel -- runs no code at all
            w.write_bool(false); // continuable
        }))
        .expect("decode");
    assert_eq!(
        decode::verify_graph(&graph, &table, fixture.interp.heap_ref()),
        Err(decode::DecodeError::CapturedFrameMismatch)
    );
}

#[test]
fn a_real_snapshots_graph_verifies_and_its_closures_still_read_their_captured_locals() {
    // The end-to-end control: a quiesced world holding a closure over a
    // method activation, through the real sections and back.
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | held ::= nil. |
        ) (
          public keep = ( | t | t := 7. held := [ t ]. ^held )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    expect_ok(interp.send(&instance, Symbol::intern("keep"), &[]), "keep");
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    let held = expect_ok(
        loaded
            .interp
            .send(&loaded.roots[0], Symbol::intern("keep"), &[]),
        "keep",
    );
    let value = expect_ok(
        loaded.interp.send(&held, Symbol::intern("value"), &[]),
        "value",
    );
    assert!(
        matches!(loaded.interp.fetch(&value), Value::SmallInt(7)),
        "the loaded closure reads its captured local"
    );
}

#[test]
fn a_snapshot_whose_closure_was_captured_by_a_block_activation_loads_and_runs() {
    // The chain case: `[:x | [ x ] ] value: 5` leaves the inner closure
    // captured by a *block* activation, so `verify_graph` has to follow one
    // closure record's captured frame into another's, and the inner body's
    // `LoadOuterLocal` really does hop two frames at run time. Sending
    // `value` after the load is what proves the reconstructed chain reads
    // the right register.
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
          | held ::= nil. |
        ) (
          public keep = ( held := [:x | [ x ] ] value: 5. ^held )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    expect_ok(interp.send(&instance, Symbol::intern("keep"), &[]), "keep");
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    let held = loaded
        .interp
        .heap_ref()
        .instance_field(*obj(&loaded.interp.fetch(&loaded.roots[0])), 0)
        .expect("expected an instance");
    // Kept across the `value` send below.
    let held = loaded.interp.stash(held);
    let captured = loaded
        .interp
        .heap_ref()
        .closure_data(*obj(&loaded.interp.fetch(&held)))
        .expect("the slot holds the inner closure")
        .captured_frame;
    assert!(
        matches!(
            loaded.interp.heap_ref().frame_kind(captured),
            FrameKind::Closure(_)
        ),
        "the inner closure was captured by the outer block's activation"
    );
    let value = expect_ok(
        loaded.interp.send(&held, Symbol::intern("value"), &[]),
        "value",
    );
    assert!(
        matches!(loaded.interp.fetch(&value), Value::SmallInt(5)),
        "the loaded closure reads `x` two frames out"
    );
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
      | n ::= 1. |
    ) (
      public n = ( ^n )
    ) : ()
"};

#[test]
fn an_image_with_no_initializer_for_a_class_that_needs_one_is_refused_on_load() {
    // `Interpreter::initializer_body` panics -- "internal: class needs an instance
    // initializer and its mixin carries none" -- when the class has
    // initializer code and its mixin has none. A mixin-table row's
    // declaration half decides the first (it names slots or `initExpr`s)
    // and its code half the second, so an image whose initializer id is
    // cleared would make the *first* `X new` after a load abort the process.
    let (mut interp, class) = load(NEEDS_INITIALIZER);
    let class = class_ref(&interp, &class);
    assert!(
        mixin_of(interp.heap_ref(), class)
            .initializer_code
            .is_some(),
        "the fixture's class really does compile an initializer"
    );
    let doctored = Mixin {
        initializer_code: None,
        ..mixin_of(interp.heap_ref(), class)
    };
    install_mixin(&mut interp, class, doctored);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    assert!(
        decoded_mixins.code[0].initializer.is_none(),
        "the doctored row names no initializer"
    );
    let err = decode::verify_mixins(&decoded_mixins).expect_err("no initializer");
    assert_eq!(err, decode::DecodeError::MissingInitializer);
    assert_eq!(
        err.to_string(),
        "a class in the image needs an instance initializer, which the image has no compiled code for"
    );
}

#[test]
fn a_whole_image_missing_its_initializer_is_refused_by_load() {
    // The same crafting one level up, through the real sections: a class
    // whose mixin carries no initializer encodes an image with the
    // initializer flag cleared, and every other check passes -- a quiesced
    // snapshot has no initializer frame for `verify_graph` to catch.
    let (mut interp, class) = load(NEEDS_INITIALIZER);
    // Instantiated before the doctoring: `install_mixin` swaps the class
    // object's own application, so this instance is already an instance of
    // the doctored class -- and instantiating the doctored class would run
    // into the very invariant this test crafts an image to violate.
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let platform = interp.platform();
    let class = class_ref(&interp, &class);
    // Only the initializer is dropped, so the refusal this asserts cannot be
    // the superclass clause's.
    let doctored = Mixin {
        initializer_code: None,
        ..mixin_of(interp.heap_ref(), class)
    };
    install_mixin(&mut interp, class, doctored);
    // Read out after the last run, for the encoder, which runs nothing.
    let bytes = image::snapshot(
        &interp,
        Some(NEEDS_INITIALIZER),
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");
    let err = load_err(&bytes);
    assert!(
        err.contains("instance initializer"),
        "load must refuse it rather than let `X new` panic later: {err}"
    );
}

#[test]
fn an_identity_hash_at_the_ceiling_is_refused_rather_than_overflowing_the_counter() {
    // `load` hands the maximum decoded hash to `advance_identity_hash_past`,
    // which computes `max + 1`: an overflow panic under this workspace's dev
    // profile, and a wrap to 0 -- so a collision with every loaded object --
    // in release.
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 0);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Str));
    w.write_u32(u32::MAX); // identity hash, at the ceiling
    w.write_u8(1); // object body: Str
    w.write_str("x");
    w.write_u32(0); // no roots -- otherwise complete, so the hash is what
                    // the decoder rejects, not a truncation
    let bytes = w.into_bytes();
    let err = GraphFixture::new().decode_err(&bytes);
    assert_eq!(err, decode::DecodeError::IdentityHashExhausted);
    assert_eq!(
        err.to_string(),
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
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let instance_obj = *obj(&interp.fetch(&instance));
    interp
        .heap_mut()
        .set_identity_hash(instance_obj, u32::MAX - 1);
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(
        loaded
            .interp
            .heap_ref()
            .identity_hash(*obj(&loaded.interp.fetch(&loaded.roots[0]))),
        u32::MAX - 1,
        "the image's hash is preserved, not clamped"
    );
    let first = loaded.interp.new_string("a".to_string());
    let second = loaded.interp.new_string("b".to_string());
    let heap = loaded.interp.heap_mut();
    assert_eq!(heap.ensure_identity_hash(first), u32::MAX);
    assert_eq!(
        heap.ensure_identity_hash(second),
        u32::MAX,
        "an exhausted counter shares its last hash instead of overflowing"
    );
}

/// An object nothing asked for its hash is written as 0 and decoded as 0 --
/// still unassigned -- and the first request after the load draws a number
/// past every hash the image carried, so it cannot collide with one.
#[test]
fn an_unhashed_object_round_trips_unassigned_and_draws_past_the_image_maximum() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let hashed = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let unhashed = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let hashed_obj = *obj(&interp.fetch(&hashed));
    let assigned = interp.heap_mut().ensure_identity_hash(hashed_obj);
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&hashed), interp.fetch(&unhashed)],
    )
    .expect("snapshot");
    assert_eq!(
        interp
            .heap_ref()
            .identity_hash(*obj(&interp.fetch(&unhashed))),
        0,
        "snapshotting assigns no hash"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let (kept, fresh) = (
        *obj(&loaded.interp.fetch(&loaded.roots[0])),
        *obj(&loaded.interp.fetch(&loaded.roots[1])),
    );
    let heap = loaded.interp.heap_mut();
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
}

/// A builtin class, the root class and `platform` are named by tag, not
/// carried as records, so before the closing hash table nothing recorded a
/// hash one of them had been given: after the load the bootstrap's fresh
/// objects drew new ones (`docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim"). One that nothing asked stays unassigned.
#[test]
fn the_hashes_of_objects_named_by_tag_survive_a_snapshot() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
        ) : ()
    "};
    // Nothing runs in either interpreter, so what is read out of a root
    // stays current.
    let (mut interp, class) = load(src);
    let class = class_ref(&interp, &class);
    let platform = interp.platform();
    let platform = interp.fetch(&platform);
    let integer = interp.builtin_class(BuiltinClass::Integer);
    let integer_hash = interp.heap_mut().ensure_identity_hash(integer);
    let class_hash = interp.heap_mut().ensure_identity_hash(class);
    let platform_hash = interp.heap_mut().ensure_identity_hash(*obj(&platform));
    let bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &platform,
        &[],
    )
    .expect("snapshot");

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
    let loaded_platform = loaded.interp.platform();
    let loaded_platform = loaded.interp.fetch(&loaded_platform);
    let loaded_class = class_ref(&loaded.interp, &loaded.root_class);
    let loaded_integer = loaded.interp.builtin_class(BuiltinClass::Integer);
    let loaded_string = loaded.interp.builtin_class(BuiltinClass::Str);
    let heap = loaded.interp.heap_mut();
    assert_eq!(heap.identity_hash(loaded_integer), integer_hash);
    assert_eq!(heap.identity_hash(loaded_class), class_hash);
    assert_eq!(heap.identity_hash(*obj(&loaded_platform)), platform_hash);
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
}

/// The closing table alone, decoded against a freshly loaded world.
fn decode_tag_named(bytes: &[u8]) -> Result<u32, decode::DecodeError> {
    let (mut interp, class) = load(EMPTY);
    let class = class_ref(&interp, &class);
    let mut r = decode::Reader::new(bytes);
    decode::decode_tag_named_hashes(&mut r, &mut interp, class)
}

#[test]
fn a_tag_named_hash_of_zero_is_refused() {
    // 0 means unassigned, which the encoder expresses by leaving the entry
    // out; accepting it would give one world two encodings.
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    w.write_u32(0);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::UnassignedTagNamedHash)
    );
}

#[test]
fn a_tag_named_object_recorded_twice_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(2);
    w.write_u8(image::opcodes::HASHED_PLATFORM);
    w.write_u32(3);
    w.write_u8(image::opcodes::HASHED_PLATFORM);
    w.write_u32(4);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::DuplicateTagNamedHash)
    );
}

#[test]
fn a_tag_named_hash_at_the_ceiling_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Integer));
    w.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::IdentityHashExhausted)
    );
}

#[test]
fn an_unknown_tag_named_hash_tag_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(3);
    w.write_u32(5);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::InvalidTag {
            what: "tag-named hash",
            tag: 3
        })
    );
}

#[test]
fn a_tag_named_hash_count_past_the_remaining_bytes_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::UnexpectedEof)
    );
}

#[test]
fn the_tag_named_hash_table_answers_its_largest_hash() {
    let mut w = encode::Writer::new();
    w.write_u32(2);
    w.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Str));
    w.write_u32(9);
    w.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    w.write_u32(4);
    assert_eq!(decode_tag_named(&w.into_bytes()), Ok(9));
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
fn apply_and_wrap(
    interp: &mut Interpreter,
    class_of_class: ClassRef,
    mixin: &Rc<Mixin>,
    superclass: Option<&Rc<MixinApplication>>,
) -> ClassRef {
    let application = interp.apply_mixin(mixin, superclass);
    interp.heap_mut().alloc_object(
        class_of_class,
        ObjectData::Class(ClassData {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
            metaclass: None,
        }),
    )
}

fn root_mixin_of(heap: &Heap, class: ClassRef) -> Rc<Mixin> {
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
    let (interp, class) = load(NESTED_MODULE);
    let class = class_ref(&interp, &class);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);

    // The *decoded* nested mixin, not the live one: its bodies are the ones
    // the mixin table attached, which is what a loaded world's class objects
    // apply.
    let inner_mixin = &decoded_mixins.root().expect("a root row").nested[0];
    let value_method = inner_mixin
        .methods
        .get(&Symbol::intern("value"))
        .expect("Inner declares `value`");
    let live_inner = &root_mixin_of(interp.heap_ref(), class).nested[0];
    assert_eq!(
        *value_method.code,
        *live_inner.methods[&Symbol::intern("value")].code,
        "the nested mixin's own method must survive the round trip"
    );
    assert!(
        inner_mixin.initializer_code.is_some(),
        "the nested mixin's own initializer must survive the round trip"
    );
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
    let (mut interp, class) = load(NESTED_MODULE);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Outer",
    );
    let bytes = image::snapshot(
        &interp,
        Some(NESTED_MODULE),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&outer)],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let root_mixin = root_mixin_of(
        loaded.interp.heap_ref(),
        class_ref(&loaded.interp, &loaded.root_class),
    );
    let inner_mixin = root_mixin.nested[0].clone();

    // Nothing materializes a nested class's own class object without the
    // accessor, which does not exist yet, so this reproduces what it will
    // eventually do: apply the decoded `Inner` mixin to `Object`, by hand,
    // in the *loaded* interpreter -- proving the send below runs against
    // code the decoded mixin actually carries, not code compiled fresh
    // just now.
    let object_class = loaded.interp.builtin_class(BuiltinClass::Object);
    let object_application = loaded
        .interp
        .heap_ref()
        .class_application(object_class)
        .expect("Object is a class object");
    let inner_class = apply_and_wrap(
        &mut loaded.interp,
        object_class,
        &inner_mixin,
        Some(&object_application),
    );
    let inner_class = loaded.interp.stash(Value::Obj(inner_class));
    let inner_instance = expect_ok(
        loaded.interp.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let result = expect_ok(
        loaded
            .interp
            .send(&inner_instance, Symbol::intern("value"), &[]),
        "send value",
    );
    match loaded.interp.fetch(&result) {
        Value::SmallInt(9) => {}
        _ => panic!("expected SmallInt(9) from Inner's `value`, got a different Value"),
    }
}

#[test]
fn the_nested_class_accessor_runs_a_reloaded_superclass_clause() {
    // The accessor evaluates the nested declaration's superclass clause,
    // and the only thing it can evaluate is the compiled clause body the
    // code table carries. A reloaded image whose sections dropped it would
    // panic on the first `Inner` access here -- which is why a mixin-table
    // row carries the clause alongside the initializer, and `verify_mixins`
    // refuses an image missing one.
    let (mut interp, class) = load(NESTED_MODULE);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Outer",
    );
    let bytes = image::snapshot(
        &interp,
        Some(NESTED_MODULE),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&outer)],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded_outer = loaded
        .roots
        .first()
        .cloned()
        .expect("the snapshotted Outer");
    let inner_class = expect_ok(
        loaded
            .interp
            .send(&reloaded_outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    let Value::Obj(_) = loaded.interp.fetch(&inner_class) else {
        panic!("the accessor must yield a class object")
    };
    let inner_instance = expect_ok(
        loaded.interp.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let result = expect_ok(
        loaded
            .interp
            .send(&inner_instance, Symbol::intern("value"), &[]),
        "send value",
    );
    match loaded.interp.fetch(&result) {
        Value::SmallInt(9) => {}
        _ => panic!("expected SmallInt(9) from Inner's `value`, got a different Value"),
    }
}

/// A world built from `src` with one instance of its root class -- the
/// starting point every whole-image nesting test shares. Returns the
/// interpreter, the root class, the `platform` singleton and the instance.
fn instantiated_world(src: &str) -> (Interpreter, Root, Root, Root) {
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let root = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate the root class",
    );
    (interp, class, platform, root)
}

/// `image::snapshot` of a world a test keeps as roots, each read out right
/// before the encoder, which runs nothing.
fn snapshot_world(
    interp: &Interpreter,
    src: &str,
    class: &Root,
    platform: &Root,
    roots: &[Root],
) -> Vec<u8> {
    let roots: Vec<Value> = roots.iter().map(|root| interp.fetch(root)).collect();
    image::snapshot(
        interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(interp, class),
        &interp.fetch(platform),
        &roots,
    )
    .expect("snapshot")
}

/// The `MixinApplication` behind a class object -- the behaviour half two
/// class objects built from one mixin and one superclass must *share*.
fn application_of(heap: &ns_runtime::Heap, class: ClassRef) -> Rc<MixinApplication> {
    heap.class_application(class).expect("a class object")
}

fn class_name(heap: &ns_runtime::Heap, class: ClassRef) -> String {
    application_of(heap, class).mixin.name.as_str().to_string()
}

fn class_of(heap: &Heap, value: &Value) -> ClassRef {
    heap.class_of_obj(*obj(value))
}

/// The value sitting in `enclosing`'s hidden cache slot for nested
/// declaration `name` -- §6's "one class object per enclosing instance",
/// read straight out of the instance rather than through the accessor.
///
/// Resolved the way the accessor resolves it: the name picks a
/// *declaration* off the class chain, and the declaration is what indexes
/// the slot map -- a subclass's `class N` and the inherited one it shadows
/// have different slots.
fn cached_nested_class(heap: &ns_runtime::Heap, enclosing: &Value, name: &str) -> Value {
    let class = class_of(heap, enclosing);
    let ns_runtime::dispatch::Lookup::NestedClass(nested, _) =
        ns_runtime::dispatch::lookup_defined_for(heap, class, Symbol::intern(name))
    else {
        panic!("{name} is a nested declaration of this object's class");
    };
    let application = application_of(heap, class);
    let index = application.nested_slot[&ns_runtime::mixin::mixin_key(&nested)];
    heap.instance_field(*obj(enclosing), index)
        .expect("an enclosing object is an instance")
}

fn send_to(loaded: &mut image::LoadedImage, receiver: &Root, selector: &str) -> Root {
    expect_ok(
        loaded.interp.send(receiver, Symbol::intern(selector), &[]),
        selector,
    )
}

/// The class object the accessor for `selector` yields on `receiver`.
fn nested_class_of(loaded: &mut image::LoadedImage, receiver: &Root, selector: &str) -> Root {
    let class = send_to(loaded, receiver, selector);
    match loaded.interp.fetch(&class) {
        Value::Obj(_) => class,
        _ => panic!("the accessor for {selector} must yield a class object"),
    }
}

#[test]
fn a_snapshot_after_a_nested_class_access_round_trips() {
    // The accessor puts a *second* user-defined class object into the world:
    // it is the value of the enclosing instance's hidden cache slot, and the
    // `.class` of every instance of it. Both must round-trip: a class tag
    // that named no particular user class would decode one back as the
    // root. A class object is an ordinary record in the object table, which
    // is what makes both work.
    let (mut interp, class, platform, outer) = instantiated_world(NESTED_MODULE);
    let inner_class = expect_ok(interp.send(&outer, Symbol::intern("Inner"), &[]), "Inner");
    let Value::Obj(_) = interp.fetch(&inner_class) else {
        panic!("the accessor must yield a class object")
    };
    let inner_instance = expect_ok(interp.instantiate(&inner_class, &[]), "instantiate Inner");

    // Reached as the cached value in Outer's hidden slot...
    let bytes = snapshot_world(
        &interp,
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
    assert_eq!(
        class_name(
            loaded.interp.heap_ref(),
            class_ref(&loaded.interp, &reloaded_inner)
        ),
        "Inner"
    );
    let instance = expect_ok(
        loaded.interp.instantiate(&reloaded_inner, &[]),
        "instantiate Inner",
    );
    let value = send_to(&mut loaded, &instance, "value");
    match loaded.interp.fetch(&value) {
        Value::SmallInt(9) => {}
        _ => panic!("expected SmallInt(9) from Inner's `value`, got a different Value"),
    }

    // ... and as the `.class` of one of its instances.
    let bytes = snapshot_world(
        &interp,
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
    assert_eq!(
        class_name(
            loaded.interp.heap_ref(),
            class_of(
                loaded.interp.heap_ref(),
                &loaded.interp.fetch(&reloaded_instance)
            )
        ),
        "Inner"
    );
    let value = send_to(&mut loaded, &reloaded_instance, "value");
    match loaded.interp.fetch(&value) {
        Value::SmallInt(9) => {}
        _ => panic!("expected SmallInt(9) from Inner's `value`, got a different Value"),
    }
}

#[test]
fn a_second_user_class_no_longer_decodes_as_the_root() {
    // A class tag that named no particular class would bring a second
    // user-defined class object back as the root: a wrong class silently
    // installed, with no field count to contradict it, rather than a decode
    // error.
    let (mut interp, class, platform, outer) = instantiated_world(NESTED_MODULE);
    let _ = expect_ok(interp.send(&outer, Symbol::intern("Inner"), &[]), "Inner");
    let bytes = snapshot_world(
        &interp,
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
    let root_class = class_ref(&loaded.interp, &loaded.root_class);
    let inner = class_ref(&loaded.interp, &inner);
    assert_ne!(
        class_name(loaded.interp.heap_ref(), inner),
        class_name(loaded.interp.heap_ref(), root_class)
    );
    assert!(
        !inner.same(root_class),
        "the nested class must not decode as the module's root class"
    );
}

#[test]
fn one_application_shared_before_a_snapshot_is_still_shared_after_it() {
    // Two enclosing instances, one shared application before the snapshot.
    // Identity is per enclosing instance (§6) but behaviour is per mixin
    // application (§6.2), and only the second half survives a save -- the
    // applications are rebuilt through `Interpreter::apply_mixin`, whose
    // cache key is content-derived. If the rebuild lost the sharing,
    // the inline caches' call sites would go megamorphic after every load, which
    // is why this is a property test rather than a shape test.
    let (mut interp, class, platform, first) = instantiated_world(NESTED_MODULE);
    let second = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "a second Outer",
    );
    let inner_of = |interp: &mut Interpreter, outer: &Root| {
        let class = expect_ok(interp.send(outer, Symbol::intern("Inner"), &[]), "Inner");
        match interp.fetch(&class) {
            Value::Obj(_) => class,
            _ => panic!("the accessor must yield a class object"),
        }
    };
    let before_a = inner_of(&mut interp, &first);
    let before_b = inner_of(&mut interp, &second);
    // Both read out after the last accessor ran.
    let before_a = class_ref(&interp, &before_a);
    let before_b = class_ref(&interp, &before_b);
    assert!(
        !before_a.same(before_b),
        "one class object per enclosing instance, before the snapshot"
    );
    assert!(
        Rc::ptr_eq(
            &application_of(interp.heap_ref(), before_a),
            &application_of(interp.heap_ref(), before_b)
        ),
        "one shared application, before the snapshot"
    );

    let bytes = snapshot_world(&interp, NESTED_MODULE, &class, &platform, &[first, second]);
    let mut loaded = image::load(&bytes).expect("load");
    let outers: Vec<Root> = loaded.roots.clone();
    let a = nested_class_of(&mut loaded, &outers[0], "Inner");
    let b = nested_class_of(&mut loaded, &outers[1], "Inner");
    // Everything below is read out after the last accessor ran.
    let a = class_ref(&loaded.interp, &a);
    let b = class_ref(&loaded.interp, &b);
    let outers: Vec<Value> = outers
        .iter()
        .map(|outer| loaded.interp.fetch(outer))
        .collect();
    // The accessor answers out of the hidden cache slot, so `a` and `b` are
    // the *decoded* class objects and not two freshly materialized ones.
    // Pinned rather than assumed: a load that dropped the cached slots
    // would still satisfy every assertion below.
    assert!(
        a.same(*obj(&cached_nested_class(
            loaded.interp.heap_ref(),
            &outers[0],
            "Inner"
        ))),
        "the accessor answered out of the decoded cache slot"
    );
    assert!(
        b.same(*obj(&cached_nested_class(
            loaded.interp.heap_ref(),
            &outers[1],
            "Inner"
        ))),
        "the accessor answered out of the decoded cache slot"
    );
    assert!(!a.same(b), "distinct class objects");
    assert!(
        Rc::ptr_eq(
            &application_of(loaded.interp.heap_ref(), a),
            &application_of(loaded.interp.heap_ref(), b)
        ),
        "one shared application"
    );
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
    let (mut interp, class, platform, outer) = instantiated_world(DEEP_OUTER_MODULE);
    let deep = expect_ok(
        interp.send(&outer, Symbol::intern("makeDeep"), &[]),
        "makeDeep",
    );
    let bytes = snapshot_world(
        &interp,
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
    assert_eq!(
        class_name(
            loaded.interp.heap_ref(),
            class_of(
                loaded.interp.heap_ref(),
                &loaded.interp.fetch(&reloaded_deep)
            )
        ),
        "Deep",
        "the reloaded instance keeps its own nested class"
    );
    let described = send_to(&mut loaded, &reloaded_deep, "describe");
    assert_eq!(
        string_contents(loaded.interp.heap_ref(), &loaded.interp.fetch(&described)),
        "top"
    );
}

/// The text of a decoded `Str` object. Never `{:?}`s the `Value`.
fn string_contents(heap: &Heap, value: &Value) -> String {
    heap.str_value(*obj(value))
        .expect("expected a string object")
}

const NESTED_SUPERCLASS_ARGS: &str = indoc! {"
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
    let (mut interp, class) = load(NESTED_SUPERCLASS_ARGS);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let bytes = image::snapshot(
        &interp,
        Some(NESTED_SUPERCLASS_ARGS),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&outer)],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded = loaded.roots.first().cloned().expect("the snapshotted Args");
    let derived = expect_ok(
        loaded
            .interp
            .send(&reloaded, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    let Value::Obj(_) = loaded.interp.fetch(&derived) else {
        panic!("the accessor must yield a class object")
    };
    let instance = expect_ok(
        loaded.interp.instantiate(&derived, &[]),
        "instantiate Derived",
    );
    let result = expect_ok(
        loaded.interp.send(&instance, Symbol::intern("value"), &[]),
        "send value",
    );
    match loaded.interp.fetch(&result) {
        Value::SmallInt(7) => {}
        _ => panic!("expected SmallInt(7) -- the clause's own argument -- from Base's slot"),
    }
}

/// The other direction, the same shape as the initializer's and the
/// clause's: a mixin-table row whose clause passes arguments, in an image
/// carrying no body for them, would panic on the first instantiation.
/// `verify_mixins` is the only place that sees both halves, so it refuses.
#[test]
fn an_image_with_no_superclass_clause_message_for_a_clause_that_needs_one_is_refused() {
    let (mut interp, class) = load(NESTED_SUPERCLASS_ARGS);
    let class = class_ref(&interp, &class);
    let derived = mixin_of(interp.heap_ref(), class)
        .nested
        .iter()
        .find(|n| n.name.as_str() == "Derived")
        .cloned()
        .expect("Args declares Derived");
    assert!(
        derived.superclass_message_code.is_some(),
        "Derived's clause really does compile a message body"
    );
    let doctored = with_nested(mixin_of(interp.heap_ref(), class), "Derived", |d| Mixin {
        superclass_message_code: None,
        ..d
    });
    install_mixin(&mut interp, class, doctored);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let err = decode::verify_mixins(&decoded_mixins).expect_err("no message body");
    assert_eq!(err, decode::DecodeError::MissingSuperclassMessage);
    assert_eq!(
        err.to_string(),
        "the image's superclass clause passes factory arguments it has no compiled code for"
    );
}

/// The clause's *receiver* side of the same shape: a mixin-table row that
/// declares a superclass clause, in an image carrying no compiled code for
/// it, would panic when the first nested-class access pushes the clause's
/// activation (`nested::superclass_clause_code`). `verify_mixins` is the only
/// place that sees both halves, so it refuses.
#[test]
fn an_image_with_no_superclass_clause_for_a_clause_that_needs_one_is_refused() {
    let (mut interp, class) = load(NESTED_SUPERCLASS_ARGS);
    let class = class_ref(&interp, &class);
    let derived = mixin_of(interp.heap_ref(), class)
        .nested
        .iter()
        .find(|n| n.name.as_str() == "Derived")
        .cloned()
        .expect("Args declares Derived");
    assert!(
        derived.superclass_clause_code.is_some(),
        "Derived's clause really does compile a clause body"
    );
    let doctored = with_nested(mixin_of(interp.heap_ref(), class), "Derived", |d| Mixin {
        superclass_clause_code: None,
        ..d
    });
    install_mixin(&mut interp, class, doctored);
    let (_table, decoded_mixins, _ids) = round_trip_code_table(interp.heap_ref(), class);
    let err = decode::verify_mixins(&decoded_mixins).expect_err("no clause body");
    assert_eq!(err, decode::DecodeError::MissingSuperclassClause);
    assert_eq!(
        err.to_string(),
        "the image declares a superclass clause it has no compiled code for"
    );
}

/// An image is untrusted input, and `verify_mixins` reconciles a
/// message body's kind and arity but nothing constrains what it *returns*.
/// `instantiate` unpacks the argument list out of that value, so a body
/// returning something that is not an argument list must raise a
/// `NewspeakError` rather than panic on the way past.
#[test]
fn a_superclass_message_body_that_returns_no_argument_list_is_an_error_not_a_panic() {
    let (mut interp, class) = load(NESTED_SUPERCLASS_ARGS);
    let class = class_ref(&interp, &class);
    // The one thing an image can present that the load-time checks let
    // through: right kind, right arity, wrong value.
    let doctored = Rc::new(CompiledCode {
        kind: CodeKind::SuperclassMessage,
        instrs: vec![
            Instr::LoadLiteral {
                dst: 0,
                literal: Literal::SmallInt(1),
            },
            Instr::Return { reg: 0 },
        ],
        num_args: 0,
        num_registers: 1,
        statement_pc: vec![(0, 0..2)],
        call_sites: vec![],
        source: Span::new(0, 0),
    });
    let doctored = with_nested(mixin_of(interp.heap_ref(), class), "Derived", |d| {
        assert!(
            d.superclass_message_code.is_some(),
            "Derived's clause compiles a message body"
        );
        Mixin {
            superclass_message_code: Some(doctored),
            ..d
        }
    });
    install_mixin(&mut interp, class, doctored);
    // An image is the only way such a body reaches an interpreter, so the
    // tampered mixin travels through one.
    let platform = interp.platform();
    let bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot");
    let mut loaded = image::load(&bytes).expect("load");
    let platform = loaded.interp.platform();
    let outer = expect_ok(
        loaded
            .interp
            .instantiate(&loaded.root_class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let derived = expect_ok(
        loaded.interp.send(&outer, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    let Value::Obj(_) = loaded.interp.fetch(&derived) else {
        panic!("the accessor must yield a class object")
    };
    match loaded.interp.instantiate(&derived, &[]) {
        Err(HostSignal::Error(e)) => assert!(
            e.message.contains("did not evaluate its message arguments"),
            "unexpected error message: {}",
            e.message
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
        let mut w = one_frame_prefix();
        w.write_u8(tag);
        w.write_u8(0); // class tag: builtin
        w.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        w.write_bool(false); // continuable
        w.write_u32(0); // no roots
        let bytes = w.into_bytes();
        let mut fixture = GraphFixture::new();
        let decoded = fixture.decode(&bytes).expect("a superclass clause frame");
        let frame = decoded.frames.first().expect("the one frame");
        let kind = fixture.interp.heap_ref().frame_kind(*frame);
        let matched = match &kind {
            FrameKind::SuperclassClause(_) => expect_clause,
            FrameKind::SuperclassMessage(_) => !expect_clause,
            _ => false,
        };
        assert!(matched, "frame kind tag {tag} decoded to the wrong kind");
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

/// A full `image::snapshot` of `src`, with no roots -- these tests care
/// about the code table's own bytes, not the object graph.
fn snapshot_of(src: &str) -> Vec<u8> {
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    image::snapshot(
        &interp,
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot")
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
        .filter(|w| *w == needle)
        .count()
}

/// The image's bytes from the symbol table onward, skipping the header, the
/// verbatim source text and its origin -- which necessarily still contain
/// every selector as plain source. The dedup property applies only from the
/// symbol table on: everything that would otherwise be an inline `write_str`
/// of a selector or name lives at or after it.
fn bytes_from_symbol_table(bytes: &[u8]) -> &[u8] {
    let mut r = decode::Reader::new(bytes);
    decode::read_header(&mut r).expect("valid header");
    r.read_str().expect("source");
    decode::decode_source_origin(&mut r).expect("origin");
    &bytes[r.pos()..]
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
        let mut r = decode::Reader::new(bytes);
        decode::read_header(&mut r).expect("valid header");
        r.read_str().expect("source");
        decode::decode_source_origin(&mut r).expect("origin");
        r.pos()
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
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut r, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::BadSymbolId(5))
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
        loaded.interp.instantiate(&loaded.root_class, &[]),
        "instantiate",
    );
    let result = expect_ok(loaded.interp.send(&instance, Symbol::intern("a"), &[]), "a");
    assert!(matches!(loaded.interp.fetch(&result), Value::SmallInt(42)));
}

// ---------------------------------------------------------------------------
// The mixin table. What the image says a declaration *is* -- its
// name, slots, methods, factory, superclass clause, nesting and class side --
// comes back from the table, never by re-parsing the image's own source text.
// ---------------------------------------------------------------------------

const TABLE_NESTED_SRC: &str = indoc! {"
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
    let mut r = decode::Reader::new(bytes);
    decode::read_header(&mut r).expect("valid header");
    r.read_str().expect("source");
    decode::decode_source_origin(&mut r).expect("origin");
    let symbols = symbols::decode(&mut r).expect("symbol table");
    let codes = decode::decode_code_table(&mut r, &symbols).expect("code table");
    mixins::decode(&mut r, &symbols, &codes.codes).expect("mixin table")
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
    let mut w = encode::Writer::new();
    encode::write_header(&mut w, encode::FLAG_HAS_SOURCE);
    w.write_str(TRIVIAL_MODULE);
    encode::encode_source_origin(&mut w, &Origin::File(PathBuf::from("t.ns")));
    let mut builder = symbols::Builder::new();
    for s in strings {
        builder.intern(s);
    }
    builder.write(&mut w);
    w.write_u32(0); // an empty code table
    w.write_raw(mixin_table);
    w.into_bytes()
}

#[test]
fn a_nested_mixin_round_trips_through_the_table() {
    let bytes = snapshot_of(TABLE_NESTED_SRC);
    let decoded = mixin_table_of(&bytes);
    let root = decoded.root().expect("a root row").clone();
    assert_eq!(root.name, Symbol::intern("Outer"));
    assert_eq!(
        root.slots
            .iter()
            .map(|s| s.name.as_str())
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
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["v"]
    );
    let factory = inner.factory.expect("Inner declares a factory");
    assert_eq!(factory.selector, Symbol::intern("withV:"));
    assert_eq!(factory.num_args, 1);
    let value = inner.methods[&Symbol::intern("value")].clone();
    assert_eq!(value.num_args, 0);
    assert_eq!(value.access, ns_syntax::ast::AccessModifier::Public);
    // The point of the whole task: nothing in the rebuilt tree came from an
    // `ast` node, and none was invented to fill a field either.
    assert!(inner.decl.is_none(), "a decoded mixin has no declaration");
    assert!(value.decl.is_none(), "nor does a decoded method");
    assert!(inner.init_exprs.is_empty());
}

/// The nesting depth is input-controlled, so decoding must not recurse
/// proportionally to it -- and neither must anything the decode hands back,
/// since dropping a tree of `Rc<Mixin>` walks it recursively too. 100_000
/// declared levels must produce an error, never an abort.
#[test]
fn a_deeply_nested_mixin_tree_does_not_blow_the_stack() {
    let bytes = image_with_mixin_table(&["X"], &mixin_chain(100_000));
    let Err(err) = ns_runtime::image::load(&bytes) else {
        panic!("100_000 levels must be refused");
    };
    assert!(
        err.contains("nests deeper than"),
        "expected the nesting ceiling to be what refuses it, got: {err}"
    );
}

/// The other side of the ceiling: a tree just inside it decodes, iteratively
/// and completely. Without this, a decoder that refused *every* nested tree
/// would pass the test above.
#[test]
fn a_mixin_tree_just_inside_the_nesting_ceiling_still_decodes() {
    let bytes = mixin_chain(128);
    let mut r = decode::Reader::new(&bytes);
    let decoded = mixins::decode(&mut r, &table_with(&["X"]), &[]).expect("128 levels decode");
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
    let mut w = encode::Writer::new();
    w.write_u32(1);
    write_mixin_row(&mut w, 0, &[], &[7]); // one row, nesting a row that is not there
    let table = w.into_bytes();
    let mut r = decode::Reader::new(&table);
    assert_eq!(
        mixins::decode(&mut r, &table_with(&["X"]), &[]).err(),
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
    let mut w = encode::Writer::new();
    w.write_u32(2);
    write_mixin_row(&mut w, 0, &[], &[1]);
    write_mixin_row(&mut w, 0, &[], &[1]); // row 1 nests itself
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        mixins::decode(&mut r, &table_with(&["X"]), &[]).err(),
        Some(decode::DecodeError::MixinTableNotATree { row: 1 })
    );
}

/// The factory selector must come from the table, not from `Mixin::decl`:
/// `instantiate::clause_message_body` checks the
/// selector a clause names against the superclass's primary factory
/// selector. Read off a declaration, a decoded mixin (which has none) would
/// fall back to `new`, and every class whose superclass has a keyword
/// factory would raise a spurious "not the primary factory message" error on
/// the first instantiation after a load. Both halves are asserted: that the
/// table carries the selector and its arity, and that a real load then
/// instantiates such a class without error.
#[test]
fn a_class_whose_superclass_has_a_keyword_factory_survives_a_round_trip() {
    let (mut interp, class) = load(NESTED_SUPERCLASS_ARGS);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate Args",
    );
    let bytes = image::snapshot(
        &interp,
        Some(NESTED_SUPERCLASS_ARGS),
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&outer)],
    )
    .expect("snapshot");

    let decoded = mixin_table_of(&bytes);
    let root = decoded.root().expect("a root row").clone();
    let base = root
        .nested
        .iter()
        .find(|m| m.name == Symbol::intern("Base"))
        .expect("Base is a nested declaration");
    let derived = root
        .nested
        .iter()
        .find(|m| m.name == Symbol::intern("Derived"))
        .expect("Derived is a nested declaration");
    let base_factory = base.factory.expect("Base declares a factory");
    assert_eq!(base_factory.selector, Symbol::intern("withBase:"));
    assert_eq!(base_factory.num_args, 1);
    let clause = derived.superclass.expect("Derived declares a clause");
    assert_eq!(
        clause.selector,
        Symbol::intern("withBase:"),
        "the clause's own selector, not the `new` default"
    );
    assert!(clause.has_args, "`Base withBase: 7` passes an argument");

    let mut loaded = image::load(&bytes).expect("load");
    let reloaded = loaded.roots.first().cloned().expect("the snapshotted Args");
    let derived_class = expect_ok(
        loaded
            .interp
            .send(&reloaded, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    let Value::Obj(_) = loaded.interp.fetch(&derived_class) else {
        panic!("the accessor must yield a class object")
    };
    let instance = expect_ok(
        loaded.interp.instantiate(&derived_class, &[]),
        "instantiate Derived",
    );
    let result = expect_ok(
        loaded.interp.send(&instance, Symbol::intern("value"), &[]),
        "value",
    );
    assert!(matches!(loaded.interp.fetch(&result), Value::SmallInt(7)));
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
    let mut src = String::from(indoc! {"
        Newspeak3
        'T'
        class C0 = Object new () (
    "});
    for i in 1..=nested {
        src.push_str(&format!("public class C{i} = Object new () (\n"));
    }
    src.push_str(if class_side { ") : ()\n" } else { ")\n" });
    for _ in 1..=nested {
        src.push_str(")\n");
    }
    src
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
    let src = nested_module_source(nested, true);
    let bytes = snapshot_of(&src);
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
    for i in 1..levels {
        mixin = Rc::new(Mixin {
            nested: vec![mixin],
            ..Mixin::empty(Symbol::intern(&format!("C{i}")))
        });
    }
    mixin
}

/// An image nested past the decoder's ceiling must be refused when it is
/// *written*. Writing one cleanly and discovering it is unloadable turns a
/// local mistake into a failure on whatever machine tries to open it.
#[test]
fn a_class_tree_nested_past_the_ceiling_is_refused_when_the_image_is_written() {
    let mut interp = Interpreter::bootstrap();
    // Two levels past the deepest module the parser will produce, so the
    // refusal cannot be blamed on an off-by-one at the boundary.
    let mixin = deep_mixin_chain(deepest_parseable_nesting() + 4);
    let application = interp.apply_mixin(&mixin, None);
    let object_class = interp.builtin_class(BuiltinClass::Object);
    let class: ClassRef = interp.heap_mut().alloc_object(
        object_class,
        ObjectData::Class(ClassData {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
            metaclass: None,
        }),
    );
    let platform = interp.platform();
    let Err(err) = image::snapshot(
        &interp,
        Some(TRIVIAL_MODULE),
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &interp.fetch(&platform),
        &[],
    ) else {
        panic!("a class tree past the ceiling must not encode");
    };
    assert!(
        err.contains("nested") && err.contains("130"),
        "the refusal must name the ceiling it enforces, got: {err}"
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
fn superclass_depth(heap: &Heap, head: &Value) -> u32 {
    let mut depth = 0;
    let mut level = match head {
        Value::Obj(class) => Some(*class),
        _ => None,
    };
    while let Some(class) = level {
        depth += 1;
        level = heap.class_superclass(class);
    }
    depth
}

/// The same count down an instance chain's first field.
fn instance_chain_depth(heap: &Heap, head: &Value) -> u32 {
    let mut depth = 0;
    let mut level = match head {
        Value::Obj(o) => Some(*o),
        _ => None,
    };
    while let Some(object) = level {
        depth += 1;
        level = match heap.instance_field(object, 0) {
            Some(Value::Obj(next)) => Some(next),
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
    let mut w = encode::Writer::new();
    graph_counts(&mut w, levels, 0);
    for i in 0..levels {
        class_record(&mut w, 0, |w| {
            if i + 1 < levels {
                w.write_bool(true);
                w.write_u8(2); // class tag: object-table record
                w.write_u32(i + 1);
            } else {
                w.write_bool(false);
            }
            w.write_bool(false); // no enclosing object
            w.write_bool(false); // no declaring class
        });
    }
    w.write_u32(1); // one root
    w.write_u8(6); // value tag: class tag
    w.write_u8(2); // class tag: object-table record
    w.write_u32(0);
    w.write_u32(0); // no tag-named hashes
    w.into_bytes()
}

/// `levels` instance records, each holding the next in its one field, with
/// the outermost as the image's single root -- the shape that has been
/// expressible since the object table existed.
fn instance_chain_graph(levels: u32) -> Vec<u8> {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, levels, 0);
    for i in 0..levels {
        w.write_u8(0); // class tag: builtin
        w.write_u8(encode::builtin_class_tag(BuiltinClass::Object));
        w.write_u32(i);
        w.write_u8(0); // object body: Instance
        if i + 1 < levels {
            w.write_u32(1);
            w.write_u8(5); // value tag: object-table id
            w.write_u32(i + 1);
        } else {
            w.write_u32(0);
        }
    }
    w.write_u32(1); // one root
    w.write_u8(5);
    w.write_u32(0);
    w.write_u32(0); // no tag-named hashes
    w.into_bytes()
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
    assert_eq!(
        superclass_depth(loaded.interp.heap_ref(), &loaded.interp.fetch(&head)),
        DEEP
    );
    // Nothing on the chain declares the selector, so the send ends in an
    // ordinary Newspeak error. That it ends at all is the point.
    let answered = loaded
        .interp
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
    assert_eq!(
        superclass_depth(loaded.interp.heap_ref(), &loaded.interp.fetch(&head)),
        DEEP
    );
    let Value::Obj(_) = loaded.interp.fetch(&head) else {
        panic!("the root is a class object");
    };
    let instance = expect_ok(loaded.interp.instantiate(&head, &[]), "instantiate");
    assert!(
        matches!(loaded.interp.fetch(&instance), Value::Obj(_)),
        "an instance of the head class"
    );
    assert_eq!(loaded.interp.heap_ref().activation_depth(), 0);
}

#[test]
fn dropping_a_loaded_world_whose_root_heads_a_long_class_chain_does_not_blow_the_stack() {
    // `load` has already released the decoded object table, which pins
    // every record and so drops shallowly. What survives into `LoadedImage`
    // is the root and the chain it alone owns -- and the application chain
    // `rebuild_applications` built alongside it -- so the cascade fires
    // here, at the end of the test, with no send of any kind.
    let bytes = image_with_graph_section(&superclass_chain_graph(DEEP));
    let loaded = image::load(&bytes).expect("load");
    assert_eq!(
        superclass_depth(
            loaded.interp.heap_ref(),
            &loaded.interp.fetch(&loaded.roots[0])
        ),
        DEEP
    );
    drop(loaded);
}

#[test]
fn dropping_a_loaded_world_whose_root_heads_a_long_instance_chain_does_not_blow_the_stack() {
    let bytes = image_with_graph_section(&instance_chain_graph(DEEP));
    let loaded = image::load(&bytes).expect("load");
    assert_eq!(
        instance_chain_depth(
            loaded.interp.heap_ref(),
            &loaded.interp.fetch(&loaded.roots[0])
        ),
        DEEP
    );
    drop(loaded);
}

#[test]
fn a_class_object_whose_own_class_is_another_record_is_refused() {
    // `Interpreter::new_class_object` gives every class object the shared
    // bootstrap `Object` placeholder, so no real world contains one -- and
    // `class` is the single reference `Object`'s iterative `Drop` cannot
    // hand to its worklist, so a chain of them is the one link that would
    // still release recursively.
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 2, 0);
    // Object 0: a class record whose own class is object 1.
    w.write_u8(2); // class tag: object-table record
    w.write_u32(1);
    w.write_u32(7); // identity hash
    w.write_u8(5); // object body: Class
    w.write_u32(0); // mixin id
    no_class_refs(&mut w);
    // Object 1: an ordinary class record.
    class_record(&mut w, 0, no_class_refs);
    w.write_u32(0); // no roots
    let bytes = w.into_bytes();
    assert_eq!(
        GraphFixture::new().decode_err(&bytes),
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

/// Instantiates `src` and snapshots the instance (plus, with `extra_send`,
/// that send's result) -- with or without the module's source text in
/// the image.
fn snapshot_module(src: &str, source: Option<&str>, extra_send: Option<&str>) -> Vec<u8> {
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let instance = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let mut roots = vec![instance.clone()];
    if let Some(selector) = extra_send {
        roots.push(expect_ok(
            interp.send(&instance, Symbol::intern(selector), &[]),
            selector,
        ));
    }
    // Read out after the last run, for the encoder, which runs nothing.
    let roots: Vec<Value> = roots.iter().map(|root| interp.fetch(root)).collect();
    image::snapshot(
        &interp,
        source,
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &roots,
    )
    .expect("snapshot")
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
        loaded.interp.send(&instance, Symbol::intern("answer"), &[]),
        "answer",
    );
    assert!(
        matches!(loaded.interp.fetch(&result), Value::SmallInt(42)),
        "the entry send must run through the decoded code table"
    );
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
        loaded.interp.instantiate(&inner_class, &[]),
        "instantiate Inner",
    );
    let value = send_to(&mut loaded, &instance, "value");
    match loaded.interp.fetch(&value) {
        Value::SmallInt(9) => {}
        _ => panic!("expected SmallInt(9) from Inner's `value`, got a different Value"),
    }
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
        loaded.interp.send(&instance, Symbol::intern("answer"), &[]),
        "answer",
    );
    assert!(matches!(loaded.interp.fetch(&result), Value::SmallInt(42)));
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

/// A `pc` past the end of the body that runs in the frame. Nothing
/// re-activates a decoded frame today, but `pc` is an index and this bound
/// is what keeps it from becoming one that is never checked.
#[test]
fn a_frame_pc_past_its_bodys_last_instruction_is_refused() {
    let (mut fixture, table) = closure_world();
    let graph = fixture
        .decode(&closure_over_frame(foo_frame_at_pc(2, 99)))
        .expect("decode");
    let err = decode::verify_graph(&graph, &table, fixture.interp.heap_ref())
        .expect_err("out-of-range pc");
    assert_eq!(err, decode::DecodeError::PcOutOfRange { pc: 99, instrs: 2 });
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
    let (mut interp, class) = load(TRIVIAL_MODULE);
    let platform = interp.platform();
    let running = interp.heap_mut().push_activation(
        ns_runtime::ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::TopLevel,
            slot_base: 0,
            code: code_with(CodeKind::Method, 0, 1, vec![Instr::Return { reg: 0 }]),
            then: ns_runtime::Continuation::Deliver,
            counted: true,
        },
        &[],
        0,
    );
    let result = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    );
    interp.heap_mut().pop_activation(running);
    let Err(err) = result else {
        panic!("a snapshot with an activation still running must be refused")
    };
    assert!(err.contains("no activation is running"), "{err}");
}

/// The format records a frame's `sender`, and the decoder rebuilds it, but
/// the runtime never sets one: a live activation's sender is the stack entry
/// below it, and a returned one has none. So only a hand-built frame can
/// carry one into the encoder, and this is the test that keeps that half of
/// the frame record from rotting unnoticed.
#[test]
fn a_frames_sender_survives_a_round_trip() {
    let (interp, _class, _o, decoded, _max) = round_trip_graph(TRIVIAL_MODULE, |interp, class| {
        // Nothing runs in here, so the class read out of its root stays
        // current.
        let class = class_ref(interp, class);
        let foo = method_code(interp.heap_ref(), class, "foo");
        let frame_running = |receiver: i64, sender| ns_runtime::Frame {
            receiver: Value::SmallInt(receiver),
            args: vec![],
            temps: vec![Value::Nil; foo.num_registers as usize],
            pc: 0,
            sender,
            home: None,
            current_class: Some(class),
            kind: FrameKind::TopLevel,
            uncontinuable: true,
            slot_base: 0,
        };
        let sender = interp.heap_mut().alloc_frame(frame_running(7, None));
        let frame = interp
            .heap_mut()
            .alloc_frame(frame_running(8, Some(sender)));
        let block_class = interp.builtin_class(BuiltinClass::Block);
        let closure = interp.heap_mut().alloc_object(
            block_class,
            ObjectData::Closure(ns_runtime::ClosureData {
                params: std::rc::Rc::new(vec![]),
                code: foo.clone(),
                captured_frame: frame,
                home: frame,
            }),
        );
        vec![interp.stash(Value::Obj(closure))]
    });
    let heap = interp.heap_ref();
    let c = heap
        .closure_data(*obj(&decoded[0]))
        .expect("expected a closure");
    assert!(matches!(heap.frame_receiver(c.home), Value::SmallInt(8)));
    let sender = heap
        .frame_sender(c.home)
        .expect("the decoded frame keeps its sender");
    assert!(!sender.same(c.home));
    assert!(matches!(heap.frame_receiver(sender), Value::SmallInt(7)));
    assert!(heap.frame_sender(sender).is_none());
}

/// A decoded frame never ran in the loading world, so it is never married and
/// no `^` can return into it -- whatever its record's `uncontinuable` flag
/// says. `raise_return` decides a home's liveness by `Heap::frame_activation`,
/// not by that flag: a file claiming a continuable home must still produce
/// `cannotReturn` rather than a `Signal::Return` that no activation on the
/// stack will ever catch.
#[test]
fn a_return_into_a_decoded_home_is_cannot_return_even_if_the_file_calls_it_continuable() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[ ^1 ] )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let closure = expect_ok(interp.send(&instance, Symbol::intern("mk"), &[]), "mk");
    // `mk` has returned, so its frame is divorced and marked uncontinuable.
    // Clear the flag, as a hand-edited file could.
    let home = interp
        .heap_ref()
        .closure_data(*obj(&interp.fetch(&closure)))
        .expect("expected a closure")
        .home;
    let mut forged = interp.heap_ref().frame_snapshot(home);
    assert!(forged.uncontinuable, "`mk` had returned");
    forged.uncontinuable = false;
    interp.heap_mut().replace_frame(home, forged);
    let bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance), interp.fetch(&closure)],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let decoded = loaded.roots[1].clone();
    let decoded_home = loaded
        .interp
        .heap_ref()
        .closure_data(*obj(&loaded.interp.fetch(&decoded)))
        .expect("expected a closure")
        .home;
    assert!(
        !loaded
            .interp
            .heap_ref()
            .frame_is_uncontinuable(decoded_home),
        "the flag the file carried must survive decoding, or this tests nothing"
    );
    assert!(loaded
        .interp
        .heap_ref()
        .frame_activation(decoded_home)
        .is_none());
    match loaded.interp.send(&decoded, Symbol::intern("value"), &[]) {
        Err(HostSignal::Error(e)) => assert!(e.message.contains("cannotReturn"), "{}", e.message),
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
    match loaded.interp.fetch(&value) {
        Value::SmallInt(146) => {}
        _ => panic!("expected SmallInt(146) from the loaded world, got a different Value"),
    }
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
/// feature into one fixture would leave a future diff of `golden-image.bin`
/// unable to say which section actually moved.
const GOLDEN_SRC: &str = indoc! {"
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
        public class Inner withSeed: s = Object new (| seed = s. |) (
            public seed = ( ^seed )
        )
    ) : (
        public marker = ( ^7 )
    )
"};

/// A source-free `image::snapshot` of `src`, compiled from scratch and
/// rooted at one freshly instantiated object -- the live object in the
/// graph `GOLDEN_SRC`'s doc comment promises. Called twice with the same
/// source, this compiles the module twice: each call gets its own `Mixin`
/// and its own method `HashMap`, so nothing here can share iteration order
/// between two encodes the way a single shared mixin tree would.
fn snapshot_of_without_source(src: &str) -> Vec<u8> {
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let instance = expect_ok(
        interp.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    // Read out after the run, for the encoder, which runs nothing.
    image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("golden.ns")),
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[interp.fetch(&instance)],
    )
    .expect("snapshot")
}

#[test]
fn the_wire_format_matches_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SRC);
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
    let a = snapshot_of_without_source(GOLDEN_SRC);
    let b = snapshot_of_without_source(GOLDEN_SRC);
    assert_eq!(a, b);
}

#[test]
#[ignore = "regenerates tests/fixtures/golden-image.bin; run deliberately"]
fn regenerate_the_golden_fixture() {
    let bytes = snapshot_of_without_source(GOLDEN_SRC);
    std::fs::write("tests/fixtures/golden-image.bin", bytes).expect("write fixture");
}

#[test]
fn the_entry_field_round_trips_present_and_absent() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut symbol_builder = symbols::Builder::new();
    let id = symbol_builder.intern("main:args:");
    let mut w = encode::Writer::new();
    symbol_builder.write(&mut w);
    encode::encode_entry(&mut w, Some(id));
    encode::encode_entry(&mut w, None);
    let bytes = w.into_bytes();

    let mut r = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut r).expect("symbol table");
    let present = decode::decode_entry(&mut r, &table).expect("present entry");
    let absent = decode::decode_entry(&mut r, &table).expect("absent entry");

    assert_eq!(
        present.map(|s| s.to_string()).as_deref(),
        Some("main:args:")
    );
    assert_eq!(absent, None);
}

#[test]
fn an_out_of_range_entry_symbol_id_is_a_decode_error_not_a_panic() {
    use ns_runtime::image::{decode, encode, symbols};

    let mut w = encode::Writer::new();
    symbols::Builder::new().write(&mut w);
    encode::encode_entry(&mut w, Some(9999));
    let bytes = w.into_bytes();

    let mut r = decode::Reader::new(&bytes);
    let table = symbols::decode(&mut r).expect("symbol table");
    assert!(decode::decode_entry(&mut r, &table).is_err());
}

#[test]
fn an_unknown_header_flag_is_still_rejected_now_that_two_are_known() {
    // Bit 2 is not assigned. A flag this build does not implement means a
    // section it cannot see, so continuing would read the next field out of
    // whatever that section occupies.
    let mut w = ns_runtime::image::encode::Writer::new();
    ns_runtime::image::encode::write_header(&mut w, 0b100);
    let bytes = w.into_bytes();
    let mut r = ns_runtime::image::decode::Reader::new(&bytes);
    assert!(ns_runtime::image::decode::read_header(&mut r).is_err());
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
const ENTRY_SRC: &str = indoc! {"
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
    let (mut interp, class) = load(ENTRY_SRC);
    let class = class_ref(&interp, &class);
    let platform = interp.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");
    let img = image::snapshot(&interp, None, &origin, class, &interp.fetch(&platform), &[])
        .expect("snapshot");

    let b = image::inspect::inspect(&binary).expect("inspect binary");
    let i = image::inspect::inspect(&img).expect("inspect image");

    let upto = |r: &image::inspect::Report| {
        r.sections
            .iter()
            .take_while(|s| s.name != "entry")
            .map(|s| (s.name, s.start, s.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(upto(&b), upto(&i), "the describing sections must line up");

    // From `origin`, not from byte 0: the header's flags byte necessarily
    // differs (`FLAG_STATELESS` is only ever set on `binary`), and that
    // difference is correct, not a defect the refactor should paper over.
    let start = b
        .sections
        .iter()
        .find(|s| s.name == "origin")
        .expect("origin section")
        .start;
    let end = b
        .sections
        .iter()
        .find(|s| s.name == "root class")
        .expect("root class section")
        .end;
    assert_eq!(
        &binary[start..end],
        &img[start..end],
        "describing bytes must be equal"
    );

    assert!(b.stateless);
    assert!(!i.stateless);
    assert_eq!(b.entry.as_deref(), Some("main:args:"));
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
    let (interp, class) = load(TRIVIAL_MODULE);
    let class = class_ref(&interp, &class);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("neverDeclared"),
    )
    .expect("build does not itself validate the entry selector against the module");

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
    let (interp, class) = load(ENTRY_SRC);
    let class = class_ref(&interp, &class);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");

    let loaded = image::load(&binary).expect("load a binary");
    assert!(loaded.stateless);
    assert!(loaded.roots.is_empty(), "a binary carries no instance");
    assert_eq!(
        loaded.entry.map(|s| s.to_string()).as_deref(),
        Some("main:args:")
    );
    assert!(
        !root_mixin_of(
            loaded.interp.heap_ref(),
            class_ref(&loaded.interp, &loaded.root_class)
        )
        .methods
        .is_empty(),
        "the binary's methods must be rebuilt, each carrying its body"
    );
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
    let (mut interp, class) = load(ENTRY_SRC);
    let platform = interp.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let img = image::snapshot(
        &interp,
        None,
        &origin,
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot");

    let loaded = image::load(&img).expect("load a stateful image with no roots");
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
    let (interp, class) = load(ENTRY_SRC);
    let class = class_ref(&interp, &class);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let mut bytes = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");

    // The entry field is the last thing in a binary, so truncating at its
    // section start and writing a bare `0` (absent) byte in its place turns
    // a valid binary into one with no entry, without hand-building a header.
    let entry_start = image::inspect::inspect(&bytes)
        .expect("inspect")
        .sections
        .iter()
        .find(|s| s.name == "entry")
        .expect("entry section")
        .start;
    bytes.truncate(entry_start);
    bytes.push(0);

    // Not `.expect_err`: `LoadedImage` carries an `Interpreter`, which has
    // no `Debug` impl, so neither type has one -- `expect_err` requires
    // `T: Debug` on the `Ok` side.
    let err = match image::load(&bytes) {
        Ok(_) => panic!("a binary must carry an entry"),
        Err(e) => e,
    };
    assert!(err.contains("entry"), "unexpected error: {err}");
}

/// The stateless binary is untrusted input exactly like the snapshot image
/// is -- no fixture is a binary that a fuzzer or a half-written file
/// produced, so this sweep is the only thing standing between a truncated
/// binary and a panic. Mirrors
/// `every_truncation_of_an_image_is_reported_rather_than_panicking`, the
/// existing model for the image shape.
#[test]
fn every_truncation_of_a_binary_is_reported_rather_than_panicking() {
    let (interp, class) = load(ENTRY_SRC);
    let class = class_ref(&interp, &class);
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");

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
    let (mut interp, class) = load(ENTRY_SRC);
    let class = class_ref(&interp, &class);
    let platform = interp.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let binary = image::build(
        interp.heap_ref(),
        None,
        &origin,
        class,
        Symbol::intern("main:args:"),
    )
    .expect("build");
    let image_bytes = image::snapshot(&interp, None, &origin, class, &interp.fetch(&platform), &[])
        .expect("snapshot");
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

    let err = match image::load(&frankenstein) {
        Ok(_) => panic!("a binary with a graph section appended must be refused"),
        Err(e) => e,
    };
    assert!(
        err.contains("trailing byte"),
        "expected the trailing-byte check to fire, got: {err}"
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
    let (mut interp, class) = load(ENTRY_SRC);
    let platform = interp.platform();
    let origin = Origin::File(PathBuf::from("t.ns"));

    let mut bytes = image::snapshot(
        &interp,
        None,
        &origin,
        class_ref(&interp, &class),
        &interp.fetch(&platform),
        &[],
    )
    .expect("snapshot");

    // Locate the flag byte from the header layout instead of hard-coding an
    // offset, so a header change fails this assertion loudly rather than
    // quietly flipping the wrong byte.
    let flag_offset = encode::MAGIC.len() + std::mem::size_of::<u32>();
    assert_eq!(
        flag_offset, 8,
        "header layout: magic (4) + FORMAT_VERSION (4)"
    );
    bytes[flag_offset] |= encode::FLAG_STATELESS;

    let err = match image::load(&bytes) {
        Ok(_) => panic!("a snapshot relabelled stateless must still be refused"),
        Err(e) => e,
    };
    assert!(
        err.contains("entry"),
        "expected the missing-entry check to fire, got: {err}"
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
    for src in [ENTRY_SRC, TRIVIAL_MODULE, WITH_A_FRAME] {
        let (mut interp, class) = load(src);
        let platform = interp.platform();
        let origin = Origin::File(PathBuf::from("t.ns"));
        let instance = interp.instantiate(&class, std::slice::from_ref(&platform));
        // Read out after the run, for the encoder, which runs nothing.
        let roots: Vec<_> = match instance {
            Ok(root) => vec![interp.fetch(&root)],
            // A module whose factory takes no platform argument still gives a
            // graph worth walking -- just without an instance in it.
            Err(_) => Vec::new(),
        };
        let img = image::snapshot(
            &interp,
            None,
            &origin,
            class_ref(&interp, &class),
            &interp.fetch(&platform),
            &roots,
        )
        .expect("snapshot");

        let report = image::inspect::inspect(&img).expect("inspect the image");
        let graph = report
            .sections
            .iter()
            .find(|s| s.name == "graph")
            .expect("a snapshot has a graph section");
        assert_eq!(
            graph.end,
            img.len(),
            "the graph walk must end at the file's last byte, not {} of {}",
            graph.end,
            img.len()
        );
    }
}
