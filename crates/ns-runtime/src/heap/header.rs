//! The header word every object begins with, in road step 3's layout
//! (`docs/design/object-model.md`): bits 0-21 the dispatch guard, 22-26 the
//! format, 27-30 the age, 31 the remembered bit, 32 a `LargeInteger`'s
//! negative bit, 33 the mark bit, 34-55 the identity hash and 56-63 the
//! number of words after the header (255: the count is in an overflow word).
//! The collector sets the mark bit while a collection runs and clears it as
//! it compacts, so that outside a collection no object is marked. The age
//! and the remembered bit belong to the generational collector and are
//! written by it; `Debug` prints both.

use crate::mixin::DispatchId;

/// One past the widest identity hash: the header word holds 22 bits of it.
pub(crate) const IDENTITY_HASH_LIMIT: u32 = 1 << 22;

pub(super) const GUARD_MASK: u64 = (1 << 22) - 1;
const FORMAT_SHIFT: u32 = 22;
const FORMAT_MASK: u64 = 0b1_1111;
const AGE_SHIFT: u32 = 27;
const AGE_MASK: u64 = 0b1111;
pub(super) const REMEMBERED_BIT: u64 = 1 << 31;
/// The widest age the header's four bits hold: an object this old is
/// tenured at its next scavenge whatever the threshold says.
pub(super) const MAX_AGE: u8 = 15;
const NEGATIVE_BIT: u64 = 1 << 32;
const MARK_BIT: u64 = 1 << 33;
const HASH_SHIFT: u32 = 34;
const HASH_MASK: u64 = (IDENTITY_HASH_LIMIT as u64) - 1;
pub(super) const WORD_COUNT_SHIFT: u32 = 56;
/// The header bits that hold the format, all clear exactly for an
/// instance's (`Format::FixedPointers`), for machine code's test of one.
pub(super) const FORMAT_BITS: u64 = FORMAT_MASK << FORMAT_SHIFT;
/// The header bits of an `Array`'s format (`Format::IndexablePointers`)
/// under `FORMAT_BITS`, for machine code's test of an indirection vector.
pub(super) const INDEXABLE_POINTERS_BITS: u64 = (Format::IndexablePointers as u64) << FORMAT_SHIFT;
/// The word count that says the count is in an overflow word.
pub(super) const WORD_COUNT_OVERFLOW: usize = 255;

/// What a debug build writes over the nursery memory a scavenge vacated:
/// a header of the free format, which no object in the space has, so that
/// a word left naming a reclaimed young object -- a store that missed the
/// write barrier -- fails at its first read instead of answering whatever
/// is allocated there next. HotSpot's `ZapUnusedHeapArea`, without its
/// page protection, which a safe slice over the nursery rules out
/// (decided with the user on 2026-10-04).
#[cfg(debug_assertions)]
pub(super) const POISON: u64 = ((Format::Free as u64) << FORMAT_SHIFT) | 0xBAD;

/// What an object's words after the header hold, and so how the collector
/// walks them and how many units the last word leaves unused.
/// `ThirtyTwoBit`, `Bytes` and `AlignedBytes` take a range of codes: the
/// low bits of the code are the unused units of the last word
/// (`Header::unused`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Format {
    FixedPointers = 0,
    IndexablePointers = 1,
    Frame = 2,
    Closure = 3,
    SixtyFourBit = 4,
    Forwarded = 5,
    Free = 6,
    /// A class object: its own format, so that asking whether an untrusted
    /// object is a class -- the decoder's verification and lookup do --
    /// reads its header alone rather than following two class words.
    Class = 7,
    /// 8..=9: the low bit is the number of unused units.
    ThirtyTwoBit = 8,
    /// A `WeakArray`'s slots: stored words the collectors forward but do
    /// not trace, so that a slot whose object nothing else reaches reads
    /// nil after the collection that reclaims it.
    WeakPointers = 10,
    /// An `Ephemeron`'s fields, the key first: the others are traced only
    /// once the key is known to survive, and all are set to nil when it
    /// does not, so that a value never keeps its own key alive.
    Ephemeron = 11,
    /// 16..=23: the low three bits are the number of unused units.
    Bytes = 16,
    /// 24..=31: a `ByteArray`'s bytes, its data at a 32-byte address; the
    /// low three bits are the unused bytes of the last word. Its own
    /// format rather than `Bytes`, which every reader takes for a string.
    AlignedBytes = 24,
}

impl Format {
    /// The format a five-bit code names. A code no format uses never comes
    /// from a header this heap wrote; it reads as `Free`, so that a corrupt
    /// word walks as a gap rather than panicking.
    fn from_code(code: u64) -> Format {
        match code {
            0 => Format::FixedPointers,
            1 => Format::IndexablePointers,
            2 => Format::Frame,
            3 => Format::Closure,
            4 => Format::SixtyFourBit,
            5 => Format::Forwarded,
            7 => Format::Class,
            8..=9 => Format::ThirtyTwoBit,
            10 => Format::WeakPointers,
            11 => Format::Ephemeron,
            16..=23 => Format::Bytes,
            24..=31 => Format::AlignedBytes,
            _ => Format::Free,
        }
    }

    /// The most units the last word of this format can leave unused.
    fn unused_limit(self) -> u8 {
        match self {
            Format::ThirtyTwoBit => 1,
            Format::Bytes | Format::AlignedBytes => 7,
            _ => 0,
        }
    }
}

/// An object's header word (the module's doc gives the layout).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Header(pub(super) u64);

impl Header {
    /// A header guarding on `guard`, of `format` with `unused` units unused
    /// in its last word, `word_count` words long after the header, with no
    /// identity hash and the negative bit clear. A count past 254 is stored
    /// as 255, the mark that the count lives in an overflow word.
    pub(super) fn new(guard: DispatchId, format: Format, unused: u8, word_count: usize) -> Header {
        debug_assert!(unused <= format.unused_limit());
        let code = (format as u64) + u64::from(unused & format.unused_limit());
        let count = word_count.min(WORD_COUNT_OVERFLOW) as u64;
        Header(
            (u64::from(guard.bits()) & GUARD_MASK)
                | (code << FORMAT_SHIFT)
                | (count << WORD_COUNT_SHIFT),
        )
    }

    pub(super) fn guard(self) -> DispatchId {
        DispatchId::from_bits((self.0 & GUARD_MASK) as u32)
    }

    pub(super) fn with_guard(self, guard: DispatchId) -> Header {
        Header((self.0 & !GUARD_MASK) | (u64::from(guard.bits()) & GUARD_MASK))
    }

    fn format_code(self) -> u64 {
        (self.0 >> FORMAT_SHIFT) & FORMAT_MASK
    }

    pub(super) fn format(self) -> Format {
        Format::from_code(self.format_code())
    }

    /// The units the last word leaves unused: 0 unless the format is
    /// `ThirtyTwoBit`, `Bytes` or `AlignedBytes`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the format code's distance past its format is the unused count, below 8 (`Header::new`), and the mask keeps only the bits the format uses"
    )]
    pub(super) fn unused(self) -> u8 {
        let format = self.format();
        (self.format_code() - format as u64) as u8 & format.unused_limit()
    }

    /// 0 until something asks for one.
    pub(super) fn identity_hash(self) -> u32 {
        ((self.0 >> HASH_SHIFT) & HASH_MASK) as u32
    }

    pub(super) fn with_identity_hash(self, hash: u32) -> Header {
        debug_assert!(hash < IDENTITY_HASH_LIMIT);
        Header(
            (self.0 & !(HASH_MASK << HASH_SHIFT)) | ((u64::from(hash) & HASH_MASK) << HASH_SHIFT),
        )
    }

    /// How many scavenges this object has survived in the nursery, at most
    /// `MAX_AGE`; 0 for a new object and meaningless for an old one.
    pub(super) fn age(self) -> u8 {
        ((self.0 >> AGE_SHIFT) & AGE_MASK) as u8
    }

    pub(super) fn with_age(self, age: u8) -> Header {
        debug_assert!(age <= MAX_AGE);
        Header((self.0 & !(AGE_MASK << AGE_SHIFT)) | ((u64::from(age) & AGE_MASK) << AGE_SHIFT))
    }

    /// Whether this old object is in the remembered set; never set on a
    /// young one.
    pub(super) fn remembered(self) -> bool {
        self.0 & REMEMBERED_BIT != 0
    }

    pub(super) fn with_remembered(self, remembered: bool) -> Header {
        if remembered {
            Header(self.0 | REMEMBERED_BIT)
        } else {
            Header(self.0 & !REMEMBERED_BIT)
        }
    }

    /// The header with `format`'s code and no unused units: what a scavenge
    /// leaves at a copied object's old place, the forwarded format, keeping
    /// the word count so the region stays walkable.
    pub(super) fn with_format(self, format: Format) -> Header {
        debug_assert_eq!(format.unused_limit(), 0);
        Header((self.0 & !(FORMAT_MASK << FORMAT_SHIFT)) | ((format as u64) << FORMAT_SHIFT))
    }

    /// A `LargeInteger`'s sign; clear for every other object.
    pub(super) fn negative(self) -> bool {
        self.0 & NEGATIVE_BIT != 0
    }

    pub(super) fn with_negative(self, negative: bool) -> Header {
        if negative {
            Header(self.0 | NEGATIVE_BIT)
        } else {
            Header(self.0 & !NEGATIVE_BIT)
        }
    }

    /// Whether the running collection has reached this object; clear
    /// outside a collection.
    pub(super) fn marked(self) -> bool {
        self.0 & MARK_BIT != 0
    }

    pub(super) fn with_marked(self, marked: bool) -> Header {
        if marked {
            Header(self.0 | MARK_BIT)
        } else {
            Header(self.0 & !MARK_BIT)
        }
    }

    /// The words after the header, the class word included; 255 means the
    /// count is in an overflow word.
    pub(super) fn word_count(self) -> usize {
        (self.0 >> WORD_COUNT_SHIFT) as usize
    }

    /// `word_count` as the eight bits the header holds it in: 255 when the
    /// count is in an overflow word.
    #[inline(always)]
    pub(super) fn word_count_field(self) -> u8 {
        (self.0 >> WORD_COUNT_SHIFT) as u8
    }
}

impl std::fmt::Debug for Header {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Header")
            .field("guard", &self.guard())
            .field("format", &self.format())
            .field("unused", &self.unused())
            .field("age", &self.age())
            .field("remembered", &self.remembered())
            .field("negative", &self.negative())
            .field("marked", &self.marked())
            .field("identity_hash", &self.identity_hash())
            .field("word_count", &self.word_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_field_reads_back_what_was_written_and_disturbs_no_other() {
        let guard = DispatchId::from_counter(DispatchId::LIMIT - 1);
        let header = Header::new(guard, Format::Bytes, 7, 254)
            .with_identity_hash(IDENTITY_HASH_LIMIT - 1)
            .with_negative(true);
        assert_eq!(header.guard(), guard);
        assert_eq!(header.format(), Format::Bytes);
        assert_eq!(header.unused(), 7);
        assert_eq!(header.word_count(), 254);
        assert_eq!(header.identity_hash(), IDENTITY_HASH_LIMIT - 1);
        assert!(header.negative());
        let aligned = Header::new(guard, Format::AlignedBytes, 7, 12);
        assert_eq!(aligned.format(), Format::AlignedBytes);
        assert_eq!(aligned.unused(), 7);
        assert_eq!(aligned.word_count(), 12);
        for format in [Format::WeakPointers, Format::Ephemeron] {
            let weak = Header::new(guard, format, 0, 3);
            assert_eq!(weak.format(), format);
            assert_eq!(weak.unused(), 0);
            assert_eq!(weak.word_count(), 3);
        }
    }

    #[test]
    fn the_age_and_the_remembered_bit_are_bits_twenty_seven_to_thirty_one() {
        let header =
            Header::new(DispatchId::NONE, Format::FixedPointers, 0, 3).with_identity_hash(9);
        assert_eq!((header.age(), header.remembered()), (0, false));
        let aged = header.with_age(MAX_AGE).with_remembered(true);
        assert_eq!(aged.0, header.0 | 0b1111 << 27 | 1 << 31);
        assert_eq!((aged.age(), aged.remembered()), (MAX_AGE, true));
        assert_eq!(aged.with_age(0).with_remembered(false), header);
    }

    #[test]
    fn a_forwarder_keeps_every_field_but_its_format() {
        let header = Header::new(DispatchId::from_counter(7), Format::Closure, 0, 300)
            .with_identity_hash(42)
            .with_age(3);
        let forwarded = header.with_format(Format::Forwarded);
        assert_eq!(forwarded.format(), Format::Forwarded);
        assert_eq!(forwarded.word_count(), 255);
        assert_eq!(forwarded.identity_hash(), 42);
        assert_eq!(forwarded.age(), 3);
    }

    #[test]
    fn the_mark_bit_is_bit_thirty_three_and_disturbs_no_other_field() {
        let header = Header::new(DispatchId::NONE, Format::Closure, 0, 7).with_identity_hash(5);
        assert!(!header.marked(), "a new header is unmarked");
        let marked = header.with_marked(true);
        assert!(marked.marked());
        assert_eq!(marked.0, header.0 | 1 << 33);
        assert_eq!(marked.with_marked(false), header);
    }

    #[test]
    fn a_word_count_past_two_hundred_and_fifty_four_reads_as_the_overflow_mark() {
        let header = Header::new(DispatchId::NONE, Format::IndexablePointers, 0, 10_000);
        assert_eq!(header.word_count(), 255);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn the_poison_is_a_free_header_of_no_words() {
        assert_eq!(Header(POISON).format(), Format::Free);
        assert_eq!(Header(POISON).word_count(), 0);
    }

    #[test]
    fn a_zero_header_carries_no_guard() {
        assert_eq!(Header(0).guard(), DispatchId::NONE);
    }
}
