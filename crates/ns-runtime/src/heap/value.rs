//! A Newspeak value as the runtime holds it: opaque, made by constructors
//! and read through accessors and `ValueKind`, so that its representation
//! can change without touching a single site
//! (`docs/design/object-model.md`, "Values and objects"). It lives under
//! `heap` because a handle's fields are private to `heap`.

use super::{Brand, ObjectReference};
use std::num::NonZeroU64;

/// The bounds of a `SmallInteger`: 61 bits, what the tagged word leaves
/// (`docs/design/object-model.md`, "Values and objects"). Every integer
/// outside them is a `LargeInteger`.
pub const SMALL_INTEGER_MIN: i64 = -(1 << 60);
pub const SMALL_INTEGER_MAX: i64 = (1 << 60) - 1;

pub const fn fits_small_integer(value: i64) -> bool {
    SMALL_INTEGER_MIN <= value && value <= SMALL_INTEGER_MAX
}

/// Spur's `isSmallFloatValueBits`: an exponent of 897 to 1151, or 896 with a
/// non-zero mantissa, or ±0.0. That is the eight-bit subset of the exponent
/// the tagged word keeps (`docs/design/object-model.md`, "Values and
/// objects"); every other float is a boxed `Float`.
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
/// (`docs/design/object-model.md`, "Values and objects"). An object is its
/// byte offset, a multiple of 8, so its low three bits are the object tag;
/// a `SmallInteger` its 61 bits over
/// `0b001`; `nil`, `false` and `true` the payloads 0, 1 and 2 over `0b010`;
/// a `SmallFloat` Spur's rotated float over `0b100`. Tags `0b011`, `0b101`,
/// `0b110` and `0b111` are never made, so bit 0 alone tells a
/// `SmallInteger`.
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

    /// The offset of the object this word names, for the collector. An
    /// object's word is its offset itself, which is never 0, so the zero
    /// word -- never stored -- answers `None` like an immediate.
    pub(super) fn object_offset(self) -> Option<NonZeroU64> {
        (self.0 & TAG_MASK == OBJECT_TAG)
            .then(|| NonZeroU64::new(self.0))
            .flatten()
    }

    pub(super) fn with_object_offset(offset: NonZeroU64) -> Word {
        Word(offset.get())
    }

    /// The word a pointer field of a word-format object holds, back from
    /// its bits; only the heap, which stored it through `Heap::store`,
    /// converts.
    pub(super) fn from_bits(bits: u64) -> Word {
        Word(bits)
    }

    /// The bits a word-format object keeps for this word.
    pub(super) fn bits(self) -> u64 {
        self.0
    }

    /// Whether the word is a `SmallInteger` of zero or more: the collector's
    /// test for a frame married to a stack entry, whose registers it skips.
    pub(super) fn is_non_negative_small_integer(self) -> bool {
        self.0 & SMALL_INTEGER_TAG == SMALL_INTEGER_TAG && self.0.cast_signed() >= 0
    }

    /// Whether the word is a `SmallInteger`.
    pub(super) fn is_small_integer(self) -> bool {
        self.0 & SMALL_INTEGER_TAG == SMALL_INTEGER_TAG
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

/// A value that holds no handle -- `nil`, a Boolean, a `SmallInteger`, a
/// `SmallFloat64` -- kept as the word it is, so that compiled code can
/// carry its constants beside its instructions
/// (`docs/design/execution.md`, "The compact instruction encoding").
/// It means the same in every heap and across every move, so
/// code holding one needs no `Trace`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Immediate(u64);

/// What an `Immediate` holds, for the decoded view of compiled code.
#[derive(Clone, Copy, Debug)]
pub enum ImmediateKind {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    SmallFloat(f64),
}

impl Immediate {
    /// `nil`, as an immediate word.
    pub const NIL: Immediate = Immediate(NIL_WORD);

    /// `true` or `false`, as an immediate word.
    pub const fn boolean(value: bool) -> Immediate {
        Immediate(if value { TRUE_WORD } else { FALSE_WORD })
    }

    /// `None` outside 61 bits, where the integer is a `LargeInteger` object.
    pub fn small_integer(value: i64) -> Option<Immediate> {
        fits_small_integer(value).then(|| Immediate(Value::<'static>::small_integer(value).word))
    }

    /// `None` outside the `SmallFloat64` range, where the float is boxed.
    pub fn small_float(value: f64) -> Option<Immediate> {
        is_small_float_bits(value.to_bits())
            .then(|| Immediate(Value::<'static>::small_float(value).word))
    }

    /// What this word holds, for an exhaustive `match`.
    pub fn kind(self) -> ImmediateKind {
        match Value::<'static>::from_immediate(self).kind() {
            ValueKind::Nil => ImmediateKind::Nil,
            ValueKind::Boolean(value) => ImmediateKind::Boolean(value),
            ValueKind::SmallInteger(value) => ImmediateKind::SmallInteger(value),
            ValueKind::SmallFloat(value) => ImmediateKind::SmallFloat(value),
            ValueKind::Object(_) => {
                unreachable!("internal: an Immediate is only ever made from an immediate word")
            }
        }
    }
}

/// What a `Value` is, for an exhaustive `match`: the view every site
/// matches on, so that no code outside the representation names it
/// (`docs/design/object-model.md`, "Values and objects"). `SmallFloat` is the
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

    /// The value `immediate` holds, in any heap: it carries no handle.
    pub const fn from_immediate(immediate: Immediate) -> Value<'gc> {
        Value::immediate(immediate.0)
    }

    /// A `SmallInteger`. Panics in every build when `value` is outside 61
    /// bits: a host reaches this constructor, and a word built from such a
    /// value would answer a different, valid integer. The run loop's tagged
    /// arithmetic builds its results without passing through here.
    pub fn small_integer(value: i64) -> Value<'gc> {
        assert!(fits_small_integer(value), "a SmallInteger outside 61 bits");
        Value::immediate((value.cast_unsigned() << TAG_BITS) | SMALL_INTEGER_TAG)
    }

    pub const fn object(object: ObjectReference<'gc>) -> Value<'gc> {
        Value {
            word: object.offset.get(),
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
            OBJECT_TAG => match self.reference() {
                Some(object) => ValueKind::Object(object),
                None => unreachable!("internal: a stored object word is never zero"),
            },
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
        (self.word & TAG_MASK == OBJECT_TAG)
            .then(|| self.reference())
            .flatten()
    }

    /// Where `Heap::immediate_dispatch_ids` keeps an immediate's id: `nil`,
    /// `false` and `true` are their payloads, so the table reads straight
    /// off the word.
    pub(super) fn immediate_index(self) -> usize {
        match self.word & TAG_MASK {
            CONSTANT_TAG => (self.word >> TAG_BITS) as usize,
            SMALL_INTEGER_TAG => 3,
            SMALL_FLOAT_TAG => super::FLOAT_DISPATCH_INDEX,
            _ => unreachable!("an object carries its own class"),
        }
    }

    /// Whether both are `SmallInteger`s: one test on the two words, since
    /// bit 0 alone tells a `SmallInteger`.
    #[inline(always)]
    fn both_small_integers(self, other: Value<'gc>) -> bool {
        self.word & other.word & SMALL_INTEGER_TAG == SMALL_INTEGER_TAG
    }

    /// `(a << 3 | 1) + (b << 3)` is `(a + b) << 3 | 1`, and it overflows
    /// `i64` exactly when `a + b` leaves 61 bits, so the one overflow test
    /// is the range test. `None` for a non-`SmallInteger` operand or a sum
    /// that is not one.
    #[inline(always)]
    pub(crate) fn small_integer_sum(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let word = self
            .word
            .cast_signed()
            .checked_add(other.word.cast_signed() - 1)?;
        Some(Value::immediate(word.cast_unsigned()))
    }

    /// `(a << 3 | 1) - (b << 3)` is `(a - b) << 3 | 1`, with the same one test.
    #[inline(always)]
    pub(crate) fn small_integer_difference(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let word = self
            .word
            .cast_signed()
            .checked_sub(other.word.cast_signed() - 1)?;
        Some(Value::immediate(word.cast_unsigned()))
    }

    /// One operand untagged, as Cog's and Soup's: `a * (b << 3)` is
    /// `(a * b) << 3`, overflowing `i64` exactly when `a * b` leaves 61 bits.
    #[inline(always)]
    pub(crate) fn small_integer_product(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let shifted =
            (self.word.cast_signed() >> TAG_BITS).checked_mul(other.word.cast_signed() - 1)?;
        Some(Value::immediate(
            shifted.cast_unsigned() | SMALL_INTEGER_TAG,
        ))
    }

    /// Both untagged: the floor quotient of a tagged word is not the tagged
    /// floor quotient. `checked_floor_divide` is the leaves' own, so the
    /// rounding rule has one copy.
    #[inline(always)]
    pub(crate) fn small_integer_floor_quotient(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let quotient = crate::builtins::numbers::checked_floor_divide(
            self.word.cast_signed() >> TAG_BITS,
            other.word.cast_signed() >> TAG_BITS,
        )?;
        fits_small_integer(quotient).then(|| Value::small_integer(quotient))
    }

    /// Tagging preserves order, so the words compare as the integers do.
    #[inline(always)]
    pub(crate) fn small_integer_order(self, other: Value<'gc>) -> Option<std::cmp::Ordering> {
        self.both_small_integers(other)
            .then(|| self.word.cast_signed().cmp(&other.word.cast_signed()))
    }

    /// The arithmetic shift restores the sign the tag shifted up.
    fn small_integer_payload(self) -> i64 {
        self.word.cast_signed() >> TAG_BITS
    }

    /// The handle an object word is: the word is the offset itself.
    /// `None` only for the zero word, which no constructor makes.
    fn reference(self) -> Option<ObjectReference<'gc>> {
        Some(ObjectReference {
            offset: NonZeroU64::new(self.word)?,
            #[cfg(debug_assertions)]
            origin: self.origin,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: self.brand,
        })
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
    use super::{Immediate, ImmediateKind, Value, ValueKind, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};

    #[test]
    fn tagged_arithmetic_answers_what_untagged_arithmetic_answers() {
        let small = Value::<'static>::small_integer;
        let samples = [
            0,
            1,
            -1,
            7,
            -7,
            3,
            1 << 30,
            -(1 << 30),
            SMALL_INTEGER_MIN,
            SMALL_INTEGER_MAX,
            SMALL_INTEGER_MIN + 1,
            SMALL_INTEGER_MAX - 1,
        ];
        for left in samples {
            for right in samples {
                let expect = |result: Option<i64>| {
                    result.filter(|number| super::fits_small_integer(*number))
                };
                assert_eq!(
                    small(left)
                        .small_integer_sum(small(right))
                        .and_then(Value::as_small_integer),
                    expect(left.checked_add(right)),
                    "{left} + {right}"
                );
                assert_eq!(
                    small(left)
                        .small_integer_difference(small(right))
                        .and_then(Value::as_small_integer),
                    expect(left.checked_sub(right)),
                    "{left} - {right}"
                );
                assert_eq!(
                    small(left)
                        .small_integer_product(small(right))
                        .and_then(Value::as_small_integer),
                    expect(left.checked_mul(right)),
                    "{left} * {right}"
                );
                assert_eq!(
                    small(left)
                        .small_integer_floor_quotient(small(right))
                        .and_then(Value::as_small_integer),
                    expect(crate::builtins::numbers::checked_floor_divide(left, right)),
                    "{left} // {right}"
                );
                assert_eq!(
                    small(left).small_integer_order(small(right)),
                    Some(left.cmp(&right)),
                    "{left} <=> {right}"
                );
            }
        }
        assert!(small(1).small_integer_sum(Value::nil()).is_none());
        assert!(Value::<'static>::boolean(true)
            .small_integer_order(small(1))
            .is_none());
    }

    #[test]
    #[should_panic(expected = "a SmallInteger outside 61 bits")]
    fn a_small_integer_outside_61_bits_is_refused_in_every_build() {
        let _ = Value::<'static>::small_integer(super::SMALL_INTEGER_MAX + 1);
    }

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
        use super::fits_small_integer;
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

    /// The zero word is never stored, so it names no object: an object word
    /// is its offset, which is never 0.
    #[test]
    fn the_zero_word_names_no_object() {
        assert!(super::Word::from_bits(0).object_offset().is_none());
    }

    /// A value made from the zero word -- a missed initialization -- is
    /// refused when it is read rather than taken for an object.
    #[test]
    #[should_panic(expected = "a stored object word is never zero")]
    fn the_kind_of_the_zero_word_panics() {
        #[cfg(debug_assertions)]
        let value: Value<'static> = Value::from_word(super::Word::from_bits(0), 0, 0);
        #[cfg(not(debug_assertions))]
        let value: Value<'static> = Value::from_word(super::Word::from_bits(0));
        let _ = value.kind();
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

    #[test]
    fn an_immediate_answers_the_value_it_was_made_from() {
        let integer = Immediate::small_integer(SMALL_INTEGER_MAX).expect("61 bits fit");
        assert_eq!(
            Value::<'static>::from_immediate(integer).as_small_integer(),
            Some(SMALL_INTEGER_MAX)
        );
        assert!(Value::<'static>::from_immediate(Immediate::NIL).is_nil());
        assert_eq!(
            Value::<'static>::from_immediate(Immediate::boolean(true)).as_boolean(),
            Some(true)
        );
        let float = Immediate::small_float(1.5).expect("1.5 is a SmallFloat64");
        assert!(matches!(
            Value::<'static>::from_immediate(float).kind(),
            ValueKind::SmallFloat(value) if value == 1.5
        ));
    }

    #[test]
    fn an_immediate_refuses_what_needs_an_object() {
        assert_eq!(Immediate::small_integer(SMALL_INTEGER_MAX + 1), None);
        assert_eq!(Immediate::small_integer(SMALL_INTEGER_MIN - 1), None);
        assert_eq!(Immediate::small_float(1.0e300), None);
        assert_eq!(Immediate::small_float(f64::NAN), None);
        assert_eq!(Immediate::small_float(f64::MIN_POSITIVE / 2.0), None);
    }

    #[test]
    fn an_immediate_keeps_a_floats_bits() {
        let positive = Immediate::small_float(0.0).expect("zero is immediate");
        let negative = Immediate::small_float(-0.0).expect("negative zero is immediate");
        assert_ne!(positive, negative);
        let ImmediateKind::SmallFloat(back) = negative.kind() else {
            panic!("expected a SmallFloat");
        };
        assert_eq!(back.to_bits(), (-0.0f64).to_bits());
        assert!(matches!(
            Immediate::small_integer(-7).expect("fits").kind(),
            ImmediateKind::SmallInteger(-7)
        ));
        assert!(matches!(Immediate::NIL.kind(), ImmediateKind::Nil));
        assert!(matches!(
            Immediate::boolean(false).kind(),
            ImmediateKind::Boolean(false)
        ));
    }
}
