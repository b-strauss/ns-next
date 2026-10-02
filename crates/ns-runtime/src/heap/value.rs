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

/// One tagged word, Spur's: the low three bits say what the rest is
/// (`docs/design/object-model.md`, "Planned: an eight-byte value"). An
/// object is its arena index shifted past the tag; a `SmallInteger` its
/// 61 bits over `0b001`; `nil`, `false` and `true` the payloads 0, 1 and 2
/// over `0b010`; a `SmallFloat` Spur's rotated float over `0b100`. Tags
/// `0b011`, `0b101`, `0b110` and `0b111` are never made, so bit 0 alone tells
/// a `SmallInteger`.
#[derive(Clone, Copy)]
pub struct Value<'gc> {
    word: u64,
    /// Debug builds only, as on the handle the word may carry: the heap
    /// and the move epoch it was minted in, so that the origin and epoch
    /// checks stay exact (`docs/design/memory.md`, "The brand"). Zero for an
    /// immediate.
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    epoch: u32,
    brand: Brand<'gc>,
}

const TAG_MASK: u64 = 0b111;
const OBJECT_TAG: u64 = 0b000;
const SMALL_INTEGER_TAG: u64 = 0b001;
const CONSTANT_TAG: u64 = 0b010;
const SMALL_FLOAT_TAG: u64 = 0b100;
const TAG_BITS: u32 = 3;
const NIL_WORD: u64 = CONSTANT_TAG;
const FALSE_WORD: u64 = (1 << TAG_BITS) | CONSTANT_TAG;
const TRUE_WORD: u64 = (2 << TAG_BITS) | CONSTANT_TAG;
/// Spur's `smallFloatExponentOffset` shifted to where the rotated exponent
/// sits: the eight-bit exponent the word keeps starts at 897.
const SMALL_FLOAT_EXPONENT_OFFSET: u64 = 896 << 53;

/// A value as a heap object stores it: the bare word, eight bytes in every
/// build, with no origin, epoch or brand (`docs/design/object-model.md`,
/// "The debug build keeps its exact check"). Only `Heap::load` and
/// `Heap::store` convert, so a stored word is always one the heap checked.
#[derive(Clone, Copy)]
pub struct Word(u64);

impl Word {
    pub(crate) const NIL: Word = Word(NIL_WORD);

    /// The arena index of the object this word names, for the collector.
    pub(super) fn object_index(self) -> Option<u32> {
        // An object's word is its 32-bit index shifted past the tag, so the
        // shift back loses nothing.
        (self.0 & TAG_MASK == OBJECT_TAG).then_some((self.0 >> TAG_BITS) as u32)
    }

    pub(super) fn with_object_index(index: u32) -> Word {
        Word(u64::from(index) << TAG_BITS)
    }
}

/// Never `{:?}` a `Word` in Newspeak-facing text; this exists so that the
/// types holding one can derive `Debug`, and prints the raw word, since a
/// word alone cannot be read without the heap that stored it.
impl std::fmt::Debug for Word {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Word({:#x})", self.0)
    }
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
    const fn immediate(word: u64) -> Value<'gc> {
        Value {
            word,
            #[cfg(debug_assertions)]
            origin: 0,
            #[cfg(debug_assertions)]
            epoch: 0,
            brand: std::marker::PhantomData,
        }
    }

    pub const fn nil() -> Value<'gc> {
        Value::immediate(NIL_WORD)
    }

    pub const fn boolean(value: bool) -> Value<'gc> {
        Value::immediate(if value { TRUE_WORD } else { FALSE_WORD })
    }

    pub fn small_integer(value: i64) -> Value<'gc> {
        debug_assert!(fits_small_integer(value), "a SmallInteger outside 61 bits");
        Value::immediate((value.cast_unsigned() << TAG_BITS) | SMALL_INTEGER_TAG)
    }

    pub const fn object(object: ObjectReference<'gc>) -> Value<'gc> {
        Value {
            word: (object.index as u64) << TAG_BITS,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: object.brand,
        }
    }

    /// The immediate float. Only `Mutator::new_float` makes one, and only for
    /// a float inside the immediate range, so that every float has one
    /// representation: a float outside it is a boxed `Float`.
    pub(crate) fn small_float(value: f64) -> Value<'gc> {
        debug_assert!(
            is_small_float_bits(value.to_bits()),
            "an immediate float outside the SmallFloat64 range"
        );
        // Spur's `smallFloatObjectOfBits`: the sign rotates to the bottom,
        // and the exponent drops its offset so that eight bits hold it; ±0.0
        // (rotated 0 or 1) keeps a zero exponent.
        let mut rotated = value.to_bits().rotate_left(1);
        if rotated > 1 {
            rotated -= SMALL_FLOAT_EXPONENT_OFFSET;
        }
        Value::immediate((rotated << TAG_BITS) | SMALL_FLOAT_TAG)
    }

    /// The word a heap object stores; only `Heap::store` asks, after its
    /// check.
    #[inline(always)]
    pub(super) fn word(self) -> Word {
        Word(self.word)
    }

    /// The value a stored word is, stamped with the heap's id and current
    /// epoch when it names an object; only `Heap::load` asks.
    #[inline(always)]
    pub(super) fn from_word(
        word: Word,
        #[cfg(debug_assertions)] origin: u32,
        #[cfg(debug_assertions)] epoch: u32,
    ) -> Value<'gc> {
        #[cfg(debug_assertions)]
        let is_object = word.0 & TAG_MASK == OBJECT_TAG;
        Value {
            word: word.0,
            #[cfg(debug_assertions)]
            origin: if is_object { origin } else { 0 },
            #[cfg(debug_assertions)]
            epoch: if is_object { epoch } else { 0 },
            brand: std::marker::PhantomData,
        }
    }

    pub fn kind(self) -> ValueKind<'gc> {
        match self.word & TAG_MASK {
            OBJECT_TAG => ValueKind::Object(self.reference()),
            SMALL_INTEGER_TAG => ValueKind::SmallInteger(self.small_integer_payload()),
            CONSTANT_TAG => match self.word {
                NIL_WORD => ValueKind::Nil,
                FALSE_WORD => ValueKind::Boolean(false),
                TRUE_WORD => ValueKind::Boolean(true),
                _ => unreachable!("a value is made only by its constructors"),
            },
            SMALL_FLOAT_TAG => {
                // Spur's `smallFloatValueOf`, the inverse of `small_float`.
                let mut rotated = self.word >> TAG_BITS;
                if rotated > 1 {
                    rotated += SMALL_FLOAT_EXPONENT_OFFSET;
                }
                ValueKind::SmallFloat(f64::from_bits(rotated.rotate_right(1)))
            }
            _ => unreachable!("a value is made only by its constructors"),
        }
    }

    pub fn is_nil(self) -> bool {
        self.word == NIL_WORD
    }

    pub fn as_boolean(self) -> Option<bool> {
        match self.word {
            TRUE_WORD => Some(true),
            FALSE_WORD => Some(false),
            _ => None,
        }
    }

    pub fn as_small_integer(self) -> Option<i64> {
        (self.word & SMALL_INTEGER_TAG == SMALL_INTEGER_TAG).then(|| self.small_integer_payload())
    }

    pub fn as_object(self) -> Option<ObjectReference<'gc>> {
        (self.word & TAG_MASK == OBJECT_TAG).then(|| self.reference())
    }

    /// Where `Heap::immediate_dispatch_ids` keeps an immediate's id: `nil`,
    /// `false` and `true` are their payloads, so the table reads straight
    /// off the word.
    pub(super) fn immediate_index(self) -> usize {
        match self.word & TAG_MASK {
            CONSTANT_TAG => (self.word >> TAG_BITS) as usize,
            SMALL_INTEGER_TAG => 3,
            SMALL_FLOAT_TAG => 4,
            _ => unreachable!("an object carries its own class"),
        }
    }

    /// The arithmetic shift restores the sign the tag shifted up.
    fn small_integer_payload(self) -> i64 {
        self.word.cast_signed() >> TAG_BITS
    }

    fn reference(self) -> ObjectReference<'gc> {
        ObjectReference {
            // An object's word is its 32-bit index shifted past the tag, so
            // the shift back loses nothing.
            index: (self.word >> TAG_BITS) as u32,
            #[cfg(debug_assertions)]
            origin: self.origin,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: self.brand,
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
            // `{:?}` so that `1.0` reads as a float, not as the integer 1.
            ValueKind::SmallFloat(number) => write!(formatter, "{number:?}"),
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
    Value {
        word: value.word,
        #[cfg(debug_assertions)]
        origin: value.origin,
        #[cfg(debug_assertions)]
        epoch: value.epoch,
        brand: Brand::default(),
    }
}

/// See `at_rest`.
pub(super) fn branded<'gc>(value: Value<'static>) -> Value<'gc> {
    Value {
        word: value.word,
        #[cfg(debug_assertions)]
        origin: value.origin,
        #[cfg(debug_assertions)]
        epoch: value.epoch,
        brand: Brand::default(),
    }
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

    #[test]
    fn every_kind_survives_the_word() {
        use super::{SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
        for number in [0, 1, -1, 42, SMALL_INTEGER_MIN, SMALL_INTEGER_MAX] {
            assert_eq!(
                Value::<'static>::small_integer(number).as_small_integer(),
                Some(number)
            );
        }
        for number in [
            0.0,
            -0.0,
            1.0,
            -2.5,
            1.0e38,
            f64::from_bits((896 << 52) | 1),
            f64::from_bits((1151 << 52) | ((1 << 52) - 1)),
        ] {
            let value = Value::<'static>::small_float(number);
            assert!(
                matches!(value.kind(), super::ValueKind::SmallFloat(back) if back.to_bits() == number.to_bits()),
                "{number}"
            );
        }
        assert!(Value::<'static>::nil().is_nil());
        assert_eq!(Value::<'static>::boolean(false).as_boolean(), Some(false));
        assert_eq!(Value::<'static>::boolean(true).as_boolean(), Some(true));
        assert!(Value::<'static>::boolean(false)
            .as_small_integer()
            .is_none());
    }

    #[test]
    fn the_words_are_spurs() {
        assert_eq!(Value::<'static>::small_integer(5).word, (5 << 3) | 1);
        assert_eq!(Value::<'static>::small_integer(-1).word, !0b110);
        assert_eq!(Value::<'static>::nil().word, 0b010);
        assert_eq!(Value::<'static>::boolean(false).word, (1 << 3) | 0b010);
        assert_eq!(Value::<'static>::boolean(true).word, (2 << 3) | 0b010);
        assert_eq!(Value::<'static>::small_float(0.0).word, 0b100);
        assert_eq!(Value::<'static>::small_float(-0.0).word, (1 << 3) | 0b100);
        assert_ne!(Value::<'static>::small_float(1.0).word & 0b111, 0);
    }

    #[test]
    fn a_word_is_eight_bytes_in_every_build() {
        assert_eq!(std::mem::size_of::<super::Word>(), 8);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_value_is_one_word() {
        assert_eq!(std::mem::size_of::<Value<'static>>(), 8);
    }
}
