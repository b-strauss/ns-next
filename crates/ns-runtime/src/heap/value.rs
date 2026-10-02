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

    /// The immediate float. Only `Mutator::new_float` makes one: every float
    /// is made through it, so that once floats outside the immediate range
    /// are boxed, every float still has one representation.
    pub(crate) const fn small_float(value: f64) -> Value<'gc> {
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
