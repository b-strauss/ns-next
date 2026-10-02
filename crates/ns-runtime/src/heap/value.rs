//! A Newspeak value as the runtime holds it: opaque, made by constructors
//! and read through accessors and `ValueKind`, so that its representation
//! can change without touching a single site
//! (`docs/design/object-model.md`, "How it is built"). It lives under
//! `heap` because a handle's fields are private to `heap`.

use super::{Brand, ObjectReference};

/// The bounds of a `SmallInteger`: 61 bits, what the tagged word leaves
/// (`docs/design/object-model.md`, "Planned: an eight-byte value"). Every
/// integer outside them is a `LargeInteger`.
pub const SMALL_INTEGER_MIN: i64 = -(1 << 60);
pub const SMALL_INTEGER_MAX: i64 = (1 << 60) - 1;

pub const fn fits_small_integer(value: i64) -> bool {
    SMALL_INTEGER_MIN <= value && value <= SMALL_INTEGER_MAX
}

/// Spur's `isSmallFloatValueBits`: an exponent of 897 to 1151, or 896 with a
/// non-zero mantissa, or ±0.0. That is the eight-bit subset of the exponent
/// the tagged word keeps (`docs/design/object-model.md`, "Planned: an
/// eight-byte value"); every other float is a boxed `Float`.
pub(crate) const fn is_small_float_bits(bits: u64) -> bool {
    const OFFSET: u64 = 896;
    let exponent = (bits >> 52) & 0x7FF;
    let mantissa = bits & ((1 << 52) - 1);
    if exponent > OFFSET {
        exponent <= OFFSET + 0xFF
    } else if mantissa != 0 {
        exponent == OFFSET
    } else {
        exponent == 0
    }
}

#[derive(Clone, Copy)]
pub struct Value<'gc>(Representation<'gc>);

#[derive(Clone, Copy)]
enum Representation<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    Object(ObjectReference<'gc>),
}

/// What a `Value` is, for an exhaustive `match`: the view every site
/// matches on, so that no code outside the representation names it
/// (`docs/design/object-model.md`, "How it is built"). `SmallFloat` is the
/// immediate float only; code that reads a float asks `Heap::float_value`,
/// which also answers a boxed one.
#[derive(Clone, Copy)]
pub enum ValueKind<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    SmallFloat(f64),
    Object(ObjectReference<'gc>),
}

impl<'gc> Value<'gc> {
    pub const fn nil() -> Value<'gc> {
        Value(Representation::Nil)
    }

    pub const fn boolean(value: bool) -> Value<'gc> {
        Value(Representation::Boolean(value))
    }

    pub fn small_integer(value: i64) -> Value<'gc> {
        debug_assert!(fits_small_integer(value), "a SmallInteger outside 61 bits");
        Value(Representation::SmallInteger(value))
    }

    pub const fn object(object: ObjectReference<'gc>) -> Value<'gc> {
        Value(Representation::Object(object))
    }

    /// The immediate float. Only `Mutator::new_float` makes one, and only for
    /// a float inside the immediate range, so that every float has one
    /// representation: a float outside it is a boxed `Float`.
    pub(crate) fn small_float(value: f64) -> Value<'gc> {
        debug_assert!(
            is_small_float_bits(value.to_bits()),
            "an immediate float outside the SmallFloat64 range"
        );
        Value(Representation::Float(value))
    }

    pub fn kind(self) -> ValueKind<'gc> {
        match self.0 {
            Representation::Nil => ValueKind::Nil,
            Representation::Boolean(flag) => ValueKind::Boolean(flag),
            Representation::SmallInteger(number) => ValueKind::SmallInteger(number),
            Representation::Float(number) => ValueKind::SmallFloat(number),
            Representation::Object(object) => ValueKind::Object(object),
        }
    }

    pub fn is_nil(self) -> bool {
        matches!(self.0, Representation::Nil)
    }

    pub fn as_boolean(self) -> Option<bool> {
        match self.0 {
            Representation::Boolean(flag) => Some(flag),
            _ => None,
        }
    }

    pub fn as_small_integer(self) -> Option<i64> {
        match self.0 {
            Representation::SmallInteger(number) => Some(number),
            _ => None,
        }
    }

    pub fn as_object(self) -> Option<ObjectReference<'gc>> {
        match self.0 {
            Representation::Object(object) => Some(object),
            _ => None,
        }
    }
}

/// Never `{:?}` a `Value` in Newspeak-facing text (`Heap::describe`); this
/// exists so that the types holding one can derive `Debug` for Rust-side
/// diagnostics, and prints what an `ObjectReference` prints.
impl std::fmt::Debug for Value<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind() {
            ValueKind::Nil => formatter.write_str("nil"),
            ValueKind::Boolean(flag) => write!(formatter, "{flag}"),
            ValueKind::SmallInteger(number) => write!(formatter, "{number}"),
            ValueKind::SmallFloat(number) => write!(formatter, "{number}"),
            ValueKind::Object(object) => write!(formatter, "{object:?}"),
        }
    }
}

/// The two conversions of a brand (`docs/design/memory.md`, "The brand"): a
/// host root's slot is shared with a host outside every entry, so it holds
/// `Value<'static>`, and `stash` passes a value through `at_rest` on the
/// way in, `fetch` through `branded` on the way out. Each is monomorphic in
/// its direction, so no single call carries a value from one entry's brand
/// to another's: composing them, `branded(at_rest(value))`, does, but a
/// misuse inside `heap` has to spell out both calls and pass visibly
/// through `'static` at the call site. Both are `pub(super)`, reachable only
/// from `heap`, and there is no public counterpart: code outside `heap`
/// cannot change a handle's brand at all.
pub(super) fn at_rest(value: Value<'_>) -> Value<'static> {
    Value(match value.0 {
        Representation::Nil => Representation::Nil,
        Representation::Boolean(flag) => Representation::Boolean(flag),
        Representation::SmallInteger(number) => Representation::SmallInteger(number),
        Representation::Float(number) => Representation::Float(number),
        Representation::Object(object) => Representation::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: Brand::default(),
        }),
    })
}

/// See `at_rest`.
pub(super) fn branded<'gc>(value: Value<'static>) -> Value<'gc> {
    Value(match value.0 {
        Representation::Nil => Representation::Nil,
        Representation::Boolean(flag) => Representation::Boolean(flag),
        Representation::SmallInteger(number) => Representation::SmallInteger(number),
        Representation::Float(number) => Representation::Float(number),
        Representation::Object(object) => Representation::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: Brand::default(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::Value;

    #[test]
    fn a_value_is_copied_whole_and_unchanged() {
        let original: Value<'static> = Value::small_integer(-7);
        let copy = original;
        assert_eq!(original.as_small_integer(), Some(-7));
        assert_eq!(copy.as_small_integer(), Some(-7));
    }

    #[test]
    fn the_small_float_range_is_spurs() {
        use super::is_small_float_bits;
        let bits = |number: f64| number.to_bits();
        assert!(is_small_float_bits(bits(0.0)));
        assert!(is_small_float_bits(bits(-0.0)));
        assert!(is_small_float_bits(bits(1.0)));
        assert!(is_small_float_bits(bits(-2.5)));
        assert!(is_small_float_bits(bits(1.0e38)));
        // Exponent 896 with a non-zero mantissa is the smallest immediate.
        assert!(is_small_float_bits((896 << 52) | 1));
        // Exponent 896 with a zero mantissa, 2^-127, would encode as zero.
        assert!(!is_small_float_bits(896 << 52));
        // Exponent 1151 is the largest immediate, 1152 the first boxed.
        assert!(is_small_float_bits((1151 << 52) | ((1 << 52) - 1)));
        assert!(!is_small_float_bits(1152 << 52));
        assert!(!is_small_float_bits(bits(1.0e300)));
        assert!(!is_small_float_bits(bits(1.0e-300)));
        assert!(!is_small_float_bits(bits(f64::MIN_POSITIVE / 2.0)));
        assert!(!is_small_float_bits(bits(f64::NAN)));
        assert!(!is_small_float_bits(bits(f64::INFINITY)));
        assert!(!is_small_float_bits(bits(f64::NEG_INFINITY)));
    }

    #[test]
    fn a_small_integer_has_sixty_one_bits() {
        use super::{fits_small_integer, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
        assert_eq!(SMALL_INTEGER_MIN, -1_152_921_504_606_846_976);
        assert_eq!(SMALL_INTEGER_MAX, 1_152_921_504_606_846_975);
        assert!(fits_small_integer(SMALL_INTEGER_MIN));
        assert!(fits_small_integer(SMALL_INTEGER_MAX));
        assert!(!fits_small_integer(SMALL_INTEGER_MIN - 1));
        assert!(!fits_small_integer(SMALL_INTEGER_MAX + 1));
        assert!(!fits_small_integer(i64::MIN));
        assert!(!fits_small_integer(i64::MAX));
    }
}
