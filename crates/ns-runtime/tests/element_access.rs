//! The element access family the typed arrays and `DataView` share:
//! `ElementAccess`, one byte naming a kind, an order, an indexing and a
//! direction, and the reads and writes at a width over a `ByteArray`'s data
//! (`Heap::byte_array_element`, `Heap::set_byte_array_element`), held to
//! the standard library's own conversions at every offset modulo 8.

use ns_runtime::builtins::primitive::{ElementAccess, ElementKind};
use ns_runtime::heap::Element;
use ns_runtime::Interpreter;

const KINDS: [ElementKind; 10] = [
    ElementKind::Int8,
    ElementKind::Uint8,
    ElementKind::Int16,
    ElementKind::Uint16,
    ElementKind::Int32,
    ElementKind::Uint32,
    ElementKind::Int64,
    ElementKind::Uint64,
    ElementKind::Float32,
    ElementKind::Float64,
];

/// What the standard library reads from `bytes` for `kind` in the order.
fn decoded(kind: ElementKind, big_endian: bool, bytes: &[u8]) -> Element {
    fn take<const N: usize>(bytes: &[u8], big_endian: bool) -> [u8; N] {
        let mut taken: [u8; N] = std::array::from_fn(|index| bytes[index]);
        if big_endian {
            taken.reverse();
        }
        taken
    }
    // Every conversion below is little-endian over bytes already put in
    // little-endian order, so the oracle has one order to get right.
    match kind {
        ElementKind::Int8 => Element::Integer(i8::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Uint8 => Element::Integer(u8::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Int16 => Element::Integer(i16::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Uint16 => Element::Integer(u16::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Int32 => Element::Integer(i32::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Uint32 => Element::Integer(u32::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Int64 => Element::Integer(i64::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Uint64 => Element::Integer(u64::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Float32 => Element::Float(f32::from_le_bytes(take(bytes, big_endian)).into()),
        ElementKind::Float64 => Element::Float(f64::from_le_bytes(take(bytes, big_endian))),
    }
}

/// The bytes the standard library writes for `element` as `kind`.
fn encoded(kind: ElementKind, big_endian: bool, element: Element) -> Vec<u8> {
    let mut bytes = match (kind, element) {
        (ElementKind::Int8, Element::Integer(value)) => (value as i8).to_le_bytes().to_vec(),
        (ElementKind::Uint8, Element::Integer(value)) => (value as u8).to_le_bytes().to_vec(),
        (ElementKind::Int16, Element::Integer(value)) => (value as i16).to_le_bytes().to_vec(),
        (ElementKind::Uint16, Element::Integer(value)) => (value as u16).to_le_bytes().to_vec(),
        (ElementKind::Int32, Element::Integer(value)) => (value as i32).to_le_bytes().to_vec(),
        (ElementKind::Uint32, Element::Integer(value)) => (value as u32).to_le_bytes().to_vec(),
        (ElementKind::Int64, Element::Integer(value)) => (value as i64).to_le_bytes().to_vec(),
        (ElementKind::Uint64, Element::Integer(value)) => (value as u64).to_le_bytes().to_vec(),
        (ElementKind::Float32, Element::Float(value)) => (value as f32).to_le_bytes().to_vec(),
        (ElementKind::Float64, Element::Float(value)) => value.to_le_bytes().to_vec(),
        _ => panic!("no element of that shape for the kind"),
    };
    if big_endian {
        bytes.reverse();
    }
    bytes
}

/// Equality that a NaN passes when its bits match, as a float element
/// read from arbitrary bytes may be one.
fn same(first: Option<Element>, second: Option<Element>) -> bool {
    match (first, second) {
        (Some(Element::Float(first)), Some(Element::Float(second))) => {
            first.to_bits() == second.to_bits()
        }
        (first, second) => first == second,
    }
}

/// What each kind is written with: its extremes and a value between for
/// an integer kind; for a float kind values that round, that overflow
/// binary32, the infinities and a NaN.
fn samples(kind: ElementKind) -> Vec<Element> {
    let integers = |min: i128, max: i128| {
        vec![
            Element::Integer(min),
            Element::Integer(max),
            Element::Integer((min + max) / 3),
        ]
    };
    match kind {
        ElementKind::Int8 => integers(i8::MIN.into(), i8::MAX.into()),
        ElementKind::Uint8 => integers(0, u8::MAX.into()),
        ElementKind::Int16 => integers(i16::MIN.into(), i16::MAX.into()),
        ElementKind::Uint16 => integers(0, u16::MAX.into()),
        ElementKind::Int32 => integers(i32::MIN.into(), i32::MAX.into()),
        ElementKind::Uint32 => integers(0, u32::MAX.into()),
        ElementKind::Int64 => integers(i64::MIN.into(), i64::MAX.into()),
        ElementKind::Uint64 => integers(0, u64::MAX.into()),
        ElementKind::Float32 | ElementKind::Float64 => vec![
            Element::Float(0.1),
            Element::Float(-2.5),
            Element::Float(1.0e39),
            Element::Float(f64::INFINITY),
            Element::Float(f64::NEG_INFINITY),
            Element::Float(f64::NAN),
        ],
    }
}

/// Every combination of the four fields survives the one byte, and no two
/// combinations share it.
#[test]
fn an_element_access_answers_what_it_was_made_of() {
    let mut made = Vec::new();
    for kind in KINDS {
        for big_endian in [false, true] {
            for byte_position in [false, true] {
                for write in [false, true] {
                    let access = ElementAccess::new(kind, big_endian, byte_position, write);
                    assert_eq!(access.kind(), kind);
                    assert_eq!(access.big_endian(), big_endian);
                    assert_eq!(access.byte_position(), byte_position);
                    assert_eq!(access.write(), write);
                    assert!(!made.contains(&access), "two combinations share a byte");
                    made.push(access);
                }
            }
        }
    }
    assert_eq!(std::mem::size_of::<ElementAccess>(), 1);
}

#[test]
fn every_kind_reads_in_both_orders_at_every_offset() {
    let pattern: Vec<u8> = (0..24u8)
        .map(|index| index.wrapping_mul(37).wrapping_add(0x81))
        .collect();
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_byte_array(&pattern);
        let heap = mutator.heap();
        for kind in KINDS {
            let width = kind.width();
            for big_endian in [false, true] {
                for offset in 0..8 {
                    let read = heap.byte_array_element(object, offset, kind, big_endian);
                    let expected = decoded(kind, big_endian, &pattern[offset..offset + width]);
                    assert!(
                        same(read, Some(expected)),
                        "{kind:?} at {offset}, big-endian {big_endian}: {read:?}, not {expected:?}"
                    );
                }
                let last = pattern.len() - width;
                assert!(heap
                    .byte_array_element(object, last, kind, big_endian)
                    .is_some());
                assert_eq!(
                    heap.byte_array_element(object, last + 1, kind, big_endian),
                    None,
                    "{kind:?} reads past the end"
                );
                assert_eq!(
                    heap.byte_array_element(object, usize::MAX, kind, big_endian),
                    None,
                    "{kind:?} reads at an offset whose end overflows"
                );
            }
        }
    });
}

#[test]
fn every_kind_writes_in_both_orders_at_every_offset() {
    const SIZE: usize = 24;
    const FILL: u8 = 0xAA;
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        for kind in KINDS {
            let width = kind.width();
            for big_endian in [false, true] {
                for element in samples(kind) {
                    for offset in 0..8 {
                        let object = mutator.new_byte_array(&[FILL; SIZE]);
                        assert!(mutator
                            .heap_mut()
                            .set_byte_array_element(object, offset, kind, big_endian, element));
                        let mut expected = vec![FILL; SIZE];
                        expected[offset..offset + width]
                            .copy_from_slice(&encoded(kind, big_endian, element));
                        assert_eq!(
                            mutator.heap().byte_array_bytes(object),
                            Some(expected),
                            "{kind:?} of {element:?} at {offset}, big-endian {big_endian}"
                        );
                    }
                    let object = mutator.new_byte_array(&[FILL; SIZE]);
                    assert!(!mutator.heap_mut().set_byte_array_element(
                        object,
                        SIZE - width + 1,
                        kind,
                        big_endian,
                        element
                    ));
                    assert!(!mutator.heap_mut().set_byte_array_element(
                        object,
                        usize::MAX,
                        kind,
                        big_endian,
                        element
                    ));
                    assert_eq!(
                        mutator.heap().byte_array_bytes(object),
                        Some(vec![FILL; SIZE]),
                        "{kind:?} wrote past the end"
                    );
                }
            }
        }
    });
}

/// A float32 is read widened exactly, and written rounded to nearest, ties
/// to even, a finite value past binary32's range becoming an infinity.
#[test]
fn a_float32_rounds_to_nearest_even_and_reads_widened() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_byte_array(&[0; 4]);
        let mut round_trip = |value: f64| {
            let kind = ElementKind::Float32;
            assert!(mutator.heap_mut().set_byte_array_element(
                object,
                0,
                kind,
                false,
                Element::Float(value)
            ));
            match mutator.heap().byte_array_element(object, 0, kind, false) {
                Some(Element::Float(read)) => read,
                _ => panic!("a float32 reads as a Float"),
            }
        };
        assert_eq!(round_trip(0.1), 0.10000000149011612);
        assert_eq!(round_trip(1.0 + 1.0 / 16_777_216.0), 1.0);
        assert_eq!(round_trip(1.0 + 3.0 / 16_777_216.0), 1.000000238418579);
        assert_eq!(round_trip(1.0e39), f64::INFINITY);
        assert_eq!(round_trip(-1.0e39), f64::NEG_INFINITY);
        assert!(round_trip(f64::NAN).is_nan());
    });
}

/// A write whose element does not fit its kind's shape -- a Float for an
/// integer kind, an Integer for a float kind -- writes nothing.
#[test]
fn a_write_of_the_wrong_shape_writes_nothing() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_byte_array(&[7; 8]);
        assert!(!mutator.heap_mut().set_byte_array_element(
            object,
            0,
            ElementKind::Int32,
            false,
            Element::Float(1.0)
        ));
        assert!(!mutator.heap_mut().set_byte_array_element(
            object,
            0,
            ElementKind::Float64,
            false,
            Element::Integer(1)
        ));
        assert_eq!(mutator.heap().byte_array_bytes(object), Some(vec![7; 8]));
    });
}
