//! Image format round-trip tests, built up task by task: header + scalars
//! (Task 12), the code table (Task 13), the object + frame tables (Task 14).

use ns_runtime::image::{decode, encode};
use ns_runtime::Value;

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
