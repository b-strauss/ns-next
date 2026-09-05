//! Image format round-trip tests, built up task by task: header + scalars
//! (Task 12), the code table (Task 13), the object + frame tables (Task 14).

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use ns_runtime::image::{decode, encode};
use ns_runtime::{
    compiler, Binding, BuiltinClass, ClassRef, Interpreter, ObjectData, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

#[test]
fn header_round_trips() {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    decode::read_header(&mut r).expect("valid header");
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
    // and write nothing, leaving Task 14's object-graph encoder in charge.
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
// Task 13: the code table and the root list.
// ---------------------------------------------------------------------------

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

const RICH: &str = "Newspeak3\n'T'\nclass Rich x: a = Object new (\n  | p = a. q ::= 0. |\n  q := p.\n) (\n  find: t in: c = ( c do: [:e | e = t ifTrue: [^e]]. ^nil )\n  loop = ( | n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n )\n  arr = ( ^{ 1. #(2 3). 'four' } )\n) : ()";

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
                field_index: 3,
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

/// The compiled code of `class`'s method `selector`, looked up the way the
/// engine hook does (by the loaded `Rc<MethodDecl>`'s identity).
fn method_code(class: &ClassRef, module: &CompiledModule, selector: &str) -> Rc<CompiledCode> {
    let borrowed = class.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        panic!("not a class")
    };
    module.methods[&method_key(&data.methods[&Symbol::intern(selector)])].clone()
}

/// Encodes a module's code table into fresh bytes and decodes it back.
fn round_trip_code_table(
    module: &CompiledModule,
    class: &ClassRef,
) -> (decode::DecodedCodeTable, encode::CodeIds) {
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, module, class);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    (
        decode::decode_code_table(&mut r).expect("decode code table"),
        ids,
    )
}

#[test]
fn every_instr_variant_round_trips() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let mut module = compiler::compile_module(&class, &bindings);
    let synthetic = Rc::new(every_instr_once());
    module
        .initializers
        .insert(class_key(&class), synthetic.clone());
    let (table, ids) = round_trip_code_table(&module, &class);
    let decoded = &table.codes[table.initializer.expect("initializer id") as usize];
    assert_eq!(**decoded, *synthetic);
    assert_eq!(
        ids.id_of(&synthetic),
        table.initializer.expect("initializer id")
    );
}

#[test]
fn a_real_modules_code_table_round_trips_and_nests_blocks_before_their_parents() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    assert_eq!(table.methods.len(), 3);
    assert_eq!(
        table
            .methods
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>(),
        vec!["arr", "find:in:", "loop"],
        "root list is sorted by selector"
    );
    for (selector, id) in &table.methods {
        let decoded = &table.codes[*id as usize];
        let original = method_code(&class, &module, selector);
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
    let init = &table.codes[table.initializer.expect("initializer id") as usize];
    assert_eq!(**init, *module.initializers[&class_key(&class)]);
}

#[test]
fn a_decoded_code_table_installs_against_a_reloaded_class() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    // "Reload": a second, independent load of the same source.
    let (_interp2, class2, bindings2) = load(RICH);
    let installed = decode::install_code_table(&table, &class2, &bindings2).expect("install");
    let borrowed = class2.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        panic!("not a class")
    };
    assert_eq!(installed.methods.len(), data.methods.len());
    for (selector, decl) in &data.methods {
        let code = installed
            .methods
            .get(&method_key(decl))
            .unwrap_or_else(|| panic!("{selector} not installed"));
        assert_eq!(**code, *method_code(&class, &module, selector.as_str()));
    }
    assert!(installed.initializers.contains_key(&class_key(&class2)));
    assert!(Rc::ptr_eq(&installed.bindings, &bindings2));
}

#[test]
fn an_unknown_selector_in_the_root_list_is_a_decode_error() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    let (_interp2, class2, bindings2) =
        load("Newspeak3\n'T'\nclass Other = Object new (\n) (\n) : ()");
    match decode::install_code_table(&table, &class2, &bindings2) {
        Err(decode::DecodeError::UnknownSelector(selector)) => {
            assert_eq!(selector, "arr");
            assert_eq!(
                decode::DecodeError::UnknownSelector(selector).to_string(),
                "image refers to method `arr`, which the reloaded source does not define"
            );
        }
        Err(e) => panic!("expected UnknownSelector, got a different error: {e}"),
        Ok(_) => panic!("expected UnknownSelector, got Ok"),
    }
}

#[test]
fn a_root_list_code_id_out_of_range_is_a_decode_error_not_a_panic() {
    let mut w = encode::Writer::new();
    w.write_u32(0); // no code entries
    w.write_u32(1); // one method in the root list ...
    w.write_str("foo");
    w.write_u32(5); // ... pointing past the end of the table
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    match decode::decode_code_table(&mut r) {
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
        decode::decode_code_table(&mut r),
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
        decode::decode_code_table(&mut r),
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
        decode::decode_code_table(&mut r),
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
    w.write_u8(20); // ... one past the last real tag
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut r),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 20
        })
    );
}

#[test]
fn an_invalid_literal_tag_and_a_bad_char_are_decode_errors_not_panics() {
    for (payload, what) in [
        (vec![9u8], "literal"),
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
        match decode::decode_code_table(&mut r) {
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
        decode::decode_code_table(&mut r),
        Err(decode::DecodeError::InvalidTag {
            what: "builtin class",
            tag: 200
        })
    );
}

#[test]
fn a_truncated_code_table_is_a_decode_error_not_a_panic() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let mut w = encode::Writer::new();
    encode::encode_code_table(&mut w, &module, &class);
    let bytes = w.into_bytes();
    for cut in [1usize, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
        let mut r = decode::Reader::new(&bytes[..cut]);
        assert!(
            decode::decode_code_table(&mut r).is_err(),
            "a code table cut to {cut} bytes must not decode"
        );
    }
}

#[test]
fn the_code_table_encoding_is_deterministic() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let mut first = encode::Writer::new();
    encode::encode_code_table(&mut first, &module, &class);
    let mut second = encode::Writer::new();
    encode::encode_code_table(&mut second, &module, &class);
    assert_eq!(first.into_bytes(), second.into_bytes());
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
        decode::decode_code_table(&mut r),
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
        decode::decode_code_table(&mut r),
        Err(decode::DecodeError::UnexpectedEof)
    );
}

// ---------------------------------------------------------------------------
// Task 13, review fix round 1.
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
    w.write_u32(0); // empty root list
    w.write_bool(false); // no initializer
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
        decode::decode_code_table(&mut r),
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
    let table = decode::decode_code_table(&mut r).expect("128 levels is within the limit");
    assert_eq!(table.codes.len(), 1);
    assert!(matches!(
        table.codes[0].instrs[0],
        Instr::LoadLiteral {
            literal: Literal::Array(_),
            ..
        }
    ));
}

#[test]
fn a_selector_the_image_has_no_code_for_is_a_decode_error() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    // "Reload" of a source that has gained a method since the snapshot: the
    // installed module would otherwise have a hole the engine hook panics on.
    let grown = RICH.replace("  arr = ", "  extra = ( ^1 )\n  arr = ");
    let (_interp2, class2, bindings2) = load(&grown);
    match decode::install_code_table(&table, &class2, &bindings2) {
        Err(decode::DecodeError::MissingSelector(selector)) => {
            assert_eq!(selector, "extra");
            assert_eq!(
                decode::DecodeError::MissingSelector(selector).to_string(),
                "the reloaded source defines method `extra`, which the image has no compiled code for"
            );
        }
        Err(e) => panic!("expected MissingSelector, got a different error: {e}"),
        Ok(_) => panic!("expected MissingSelector, got Ok"),
    }
}

#[test]
fn installing_against_a_root_that_is_not_a_class_is_a_decode_error() {
    let (mut interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    let not_a_class = interp.new_string("not a class".to_string());
    match decode::install_code_table(&table, &not_a_class, &bindings) {
        Err(decode::DecodeError::NotAClass) => assert_eq!(
            decode::DecodeError::NotAClass.to_string(),
            "the image's root object is not a class"
        ),
        Err(e) => panic!("expected NotAClass, got a different error: {e}"),
        Ok(_) => panic!("expected NotAClass, got Ok"),
    }
}

// ---------------------------------------------------------------------------
// Task 13, review fix round 2: the decoder's nesting ceiling must sit at or
// above what `ns-syntax`'s parser lets a program contain, or the image
// cannot read back what it just wrote.
// ---------------------------------------------------------------------------

/// A module whose one method returns a literal array nested `levels` deep:
/// `#(` opens the first level and each further `(` opens another.
fn source_with_nested_literal_array(levels: usize) -> String {
    let open = "(".repeat(levels);
    let close = ")".repeat(levels);
    format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  arr = ( ^#{open}0{close} )\n) : ()")
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
    let (_interp, class, bindings) = load(&source_with_nested_literal_array(levels));
    let module = compiler::compile_module(&class, &bindings);
    let original = method_code(&class, &module, "arr");
    assert_eq!(
        literal_array_depth(&original),
        levels,
        "the compiler lowered the whole nest"
    );
    let (table, _ids) = round_trip_code_table(&module, &class);
    let (_, id) = table
        .methods
        .iter()
        .find(|(selector, _)| selector == "arr")
        .expect("arr is in the root list");
    let decoded = &table.codes[*id as usize];
    assert_eq!(**decoded, *original);
    assert_eq!(literal_array_depth(decoded), levels);
}
