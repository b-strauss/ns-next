//! What sits behind an `ObjectReference` and a `FrameReference`: the block
//! of words every object is, a reified activation's included -- word
//! `HEADER` the header, word `CLASS` the class word, then the fields -- and
//! how the collector walks one by its format. Private to the heap module --
//! nothing outside `heap.rs` and its submodules names these, so that
//! changing how an object or a frame is represented changes this module
//! rather than its callers (`docs/design/memory.md`, "The seam"). Callers
//! build objects through `Heap`'s typed allocations and the value types of
//! `records.rs`, and read them through its accessors.
//!
//! A block's fields are read by its header's format (`docs/design/memory.md`):
//! `FixedPointers`, an instance's fields; `IndexablePointers`, an `Array`'s
//! elements -- both as the bits of a stored `Word`, so only `Heap::store`
//! writes one and only `Heap::load` reads one; `SixtyFourBit`, a
//! `LargeInteger`'s magnitude, least significant digit first, its sign the
//! header's negative bit, or a boxed `Float`'s one word of `f64::to_bits` --
//! the class's builtin tells those two apart, as Spur's does; `Bytes` and
//! `ThirtyTwoBit`, a String's or a Symbol's code points packed
//! little-endian, eight or two to a word (`pack_code_points`,
//! `PackedString`), the class's builtin again telling the two apart;
//! `AlignedBytes`, a `ByteArray`'s bytes, little-endian within each word,
//! after `ALIGNMENT_RESERVE` words that let the data begin at a 32-byte
//! address wherever the block lies -- an offset computed from the header's
//! index, never stored, so that every mover moves the data within the new
//! block to its alignment (`realign`);
//! `WeakPointers`, a `WeakArray`'s slots, and `Ephemeron`, an ephemeron's
//! key and the fields it guards -- stored words like `IndexablePointers`'
//! and `FixedPointers`', which marking and the scavenger trace only in part
//! (`strong_pointer_ranges`);
//! `Frame`, a reified activation (`frame_field`); `Closure`, a block closure
//! (`closure_field`); `Class`, a class object (`class_field`). Each
//! allocation in `Heap` writes its object's header, class word and fields
//! straight into the space.
//!
//! A `LargeInteger` never holds a value a `SmallInteger` does, and a boxed
//! `Float` never one inside the immediate range: `Mutator::new_integer` and
//! `Mutator::new_float` are the one way to make either and decide it, so
//! each number has one representation.

use crate::builtins::primitive::ElementKind;
use crate::heap::header::{Format, Header};
use crate::heap::Word;
use crate::trace::Tracer;

/// Where a block keeps its header word (`header.rs` gives the layout).
///
/// The header's guard is the id an inline cache guards this object's sends
/// on (`docs/design/dispatch.md`, "The dispatch id"): its class's instances
/// id -- a class object's class is its metaclass. A copy, so that a guard is
/// one load and one compare rather than a walk to the class -- which makes
/// every place a class's application changes responsible for restamping it
/// (`Heap::class_set_application`, `Heap::stamp_dispatch_ids`).
///
/// Its identity hash is 0 until something asks for it; then drawn from a
/// counter (`Heap::object_ensure_identity_hash`), so allocation order stays
/// unobservable. Never derived from the object's offset, which a moving
/// collector or `become:` would change (liveness spec's stable-identity
/// constraint). Not a handle; a move carries it along.
pub(in crate::heap) const HEADER: usize = 0;
/// Where a block keeps its class word: a stored `Word` naming the class
/// object, written through `Heap::store` and read through `Heap::load`.
pub(in crate::heap) const CLASS: usize = 1;
/// Where a block's fields start: field `index` is word `FIELDS + index`.
pub(in crate::heap) const FIELDS: usize = 2;

/// How many ranges `pointer_ranges` answers: the class word's, then at
/// most one per pointer field of a class object, the format with the most.
pub(in crate::heap) const POINTER_RANGES: usize = 1 + class_field::POINTERS.len();

/// The indices of every word of `block` that holds a stored `Word`, as
/// ranges relative to the block's start, decided by the header's format
/// alone: the class word always; every field of `FixedPointers`,
/// `IndexablePointers`, `WeakPointers` and `Ephemeron`; a class object's
/// object fields; a closure's captured
/// frame and copied values; a frame's fixed fields, and its registers once
/// it is no longer married; nothing of `SixtyFourBit`, `Bytes`,
/// `ThirtyTwoBit` and `AlignedBytes`, whose digits, code points and bytes
/// would read as handles.
///
/// The one enumeration of an object's pointer words (`pointer_ranges_of`
/// holds it): marking and the scavenger read the words through it -- a
/// weak object by way of `strong_pointer_ranges`, which leaves out only
/// weak words -- and forwarding rewrites them through `trace_block`,
/// so a field one of the two must reach cannot be missed by the other
/// (`docs/design/memory.md`, "What the collector traces"). Unused entries
/// are empty, and every range is clamped to the block, so a block shorter
/// than its format expects yields empty ranges rather than a panic.
pub(in crate::heap) fn pointer_ranges(block: &[u64]) -> [std::ops::Range<usize>; POINTER_RANGES] {
    let Some(&header) = block.get(HEADER) else {
        return Default::default();
    };
    pointer_ranges_of(Header(header).format(), block)
}

/// `pointer_ranges` for a block whose header the caller has already
/// decoded to `format`: the enumeration itself, which `pointer_ranges`
/// forwards to. Marking and the scavenger decode each object's format once
/// to choose between this and the weak path, so that an ordinary object
/// pays no second read of its header. The same contract: ranges clamped
/// to the block, an empty or short block answering empty ranges.
pub(in crate::heap) fn pointer_ranges_of(
    format: Format,
    block: &[u64],
) -> [std::ops::Range<usize>; POINTER_RANGES] {
    let mut ranges: [std::ops::Range<usize>; POINTER_RANGES] = Default::default();
    let length = block.len();
    let clamped = |start: usize, end: usize| start.min(length)..end.min(length);
    ranges[0] = clamped(CLASS, CLASS + 1);
    match format {
        Format::FixedPointers
        | Format::IndexablePointers
        | Format::WeakPointers
        | Format::Ephemeron => ranges[1] = clamped(FIELDS, length),
        // A married frame's registers are its stack entry's, traced with
        // the stack; its own stay nil until the divorce copies them in, so
        // they are skipped rather than walked for nothing.
        Format::Frame => {
            let married = block
                .get(FIELDS + frame_field::MARRIED)
                .is_some_and(|bits| *bits != Word::NIL.bits());
            let end = if married {
                FIELDS + frame_field::COUNT
            } else {
                length
            };
            ranges[1] = clamped(FIELDS, end);
        }
        // A closure's code id and feedback base are numbers; its captured
        // frame and its copied values are the pointers.
        Format::Closure => {
            let frame = FIELDS + closure_field::CAPTURED_FRAME;
            ranges[1] = clamped(frame, frame + 1);
            ranges[2] = clamped(FIELDS + closure_field::COUNT, length);
        }
        // A class object's builtin, applied clause and application are
        // numbers; the rest are objects or nil.
        Format::Class => {
            for (range, field) in ranges[1..].iter_mut().zip(class_field::POINTERS) {
                *range = clamped(FIELDS + field, FIELDS + field + 1);
            }
        }
        // Digits, code points and bytes store no words. A forwarder -- the
        // corpse a bulk forward leaves, garbage in the old space until a
        // full collection -- keeps the old object's fields, which nothing
        // reads any more; only its class word, `ranges[0]`, names its
        // replacement, and tracing that word is what keeps the young
        // replacement of a remembered forwarder alive and rewritten until
        // it is tenured. A free block holds nothing.
        Format::SixtyFourBit
        | Format::Bytes
        | Format::ThirtyTwoBit
        | Format::AlignedBytes
        | Format::Forwarded
        | Format::Free => {}
    }
    ranges
}

/// The words of `block`, its header decoded to `format`, that keep what
/// they name alive: the class word alone of `WeakPointers`, and of an
/// `Ephemeron` whose key -- its first field, handed to `key_survives` as
/// stored -- is not yet known to survive; `pointer_ranges_of` otherwise.
/// An `Ephemeron` with no field has no key and is traced as
/// `FixedPointers`.
///
/// What marking and the scavenger trace, while forwarding, the moves and
/// the remembered set's verification read `pointer_ranges`: a weak word
/// must not keep its object alive, but while the object lives the word
/// must follow it, or it would name whatever is moved to its old place
/// (`docs/design/core-library.md`, "The heap and the collectors"). Both
/// send only the weak formats here and every other object straight to
/// `pointer_ranges_of`, so that an ordinary object calls no closure.
pub(in crate::heap) fn strong_pointer_ranges(
    format: Format,
    block: &[u64],
    key_survives: impl FnOnce(u64) -> bool,
) -> [std::ops::Range<usize>; POINTER_RANGES] {
    let class_word_only = || {
        let mut ranges: [std::ops::Range<usize>; POINTER_RANGES] = Default::default();
        ranges[0] = CLASS.min(block.len())..(CLASS + 1).min(block.len());
        ranges
    };
    match format {
        Format::WeakPointers => class_word_only(),
        Format::Ephemeron => match block.get(FIELDS) {
            Some(&key) if !key_survives(key) => class_word_only(),
            _ => pointer_ranges_of(format, block),
        },
        _ => pointer_ranges_of(format, block),
    }
}

/// Hands every word of `block` that `pointer_ranges` names to `tracer` and
/// writes back what the tracer leaves in it.
pub(in crate::heap) fn trace_block<T: Tracer>(block: &mut [u64], tracer: &mut T) {
    for range in pointer_ranges(block) {
        trace_word_bits(&mut block[range], tracer);
    }
}

/// How many field words `string_shape` gives the `CodePoints` of `text`,
/// read without converting it: one byte a unit while every code point fits
/// Latin-1, 32 bits a unit otherwise.
pub(in crate::heap) fn string_field_count(text: &str) -> usize {
    let units = text.chars().count();
    let wide = text.chars().any(|character| u32::from(character) > 0xFF);
    units.div_ceil(if wide {
        THIRTY_TWO_BIT_UNITS_PER_WORD
    } else {
        BYTES_PER_WORD
    })
}

/// Packs `text` into `words` as `pack_code_points` lays out the
/// `CodePoints` of `text`, without converting it: unit `index` is byte
/// `index % 8` of word `index / 8`, or half `index % 2` of word `index / 2`,
/// the last word's unused units zero. Answers the format and the unused
/// units, or `None`, writing nothing, when `words` is not the length
/// `string_field_count` gives `text`.
pub(in crate::heap) fn pack_string(text: &str, words: &mut [u64]) -> Option<(Format, u8)> {
    if words.len() != string_field_count(text) {
        return None;
    }
    let wide = text.chars().any(|character| u32::from(character) > 0xFF);
    let (format, per_word) = if wide {
        (Format::ThirtyTwoBit, THIRTY_TWO_BIT_UNITS_PER_WORD)
    } else {
        (Format::Bytes, BYTES_PER_WORD)
    };
    let bits_per_unit = u64::BITS as usize / per_word;
    words.fill(0);
    let mut units = 0;
    for (index, character) in text.chars().enumerate() {
        words[index / per_word] |=
            u64::from(u32::from(character)) << ((index % per_word) * bits_per_unit);
        units += 1;
    }
    // Below `per_word`, which is at most 8.
    let unused = u8::try_from(words.len() * per_word - units).unwrap_or(0);
    Some((format, unused))
}

/// A String's or a Symbol's shape for `text`: its format, one byte a unit
/// while every code point fits Latin-1 and 32 bits a unit otherwise, as
/// `CodePoints` keeps them, so that `size` and `at:` stay one access; the
/// units its last word leaves unused; and its field count.
pub(in crate::heap) fn string_shape(text: &crate::CodePoints) -> (Format, u8, usize) {
    let (format, per_word) = match text.as_wide() {
        None => (Format::Bytes, BYTES_PER_WORD),
        Some(_) => (Format::ThirtyTwoBit, THIRTY_TWO_BIT_UNITS_PER_WORD),
    };
    let size = text.size();
    let field_count = size.div_ceil(per_word);
    // Below `per_word`, which is at most 8.
    let unused = u8::try_from(field_count * per_word - size).unwrap_or(0);
    (format, unused, field_count)
}

/// Packs the code points of `text` into `words`, the field count
/// `string_shape` gives: unit `index` is byte `index % 8` of word
/// `index / 8`, or half `index % 2` of word `index / 2`, least significant
/// first; the last word's unused units are zero, so that equal texts have
/// equal words. Writes every word.
pub(in crate::heap) fn pack_code_points(text: &crate::CodePoints, words: &mut [u64]) {
    debug_assert_eq!(words.len(), string_shape(text).2);
    match text.as_wide() {
        None => {
            let bytes = text.as_bytes().unwrap_or_default();
            for (word, chunk) in words.iter_mut().zip(bytes.chunks(BYTES_PER_WORD)) {
                let mut packed = [0u8; BYTES_PER_WORD];
                packed[..chunk.len()].copy_from_slice(chunk);
                *word = u64::from_le_bytes(packed);
            }
        }
        Some(units) => {
            for (word, chunk) in words
                .iter_mut()
                .zip(units.chunks(THIRTY_TWO_BIT_UNITS_PER_WORD))
            {
                *word = chunk
                    .iter()
                    .rev()
                    .fold(0, |packed, unit| (packed << 32) | u64::from(*unit));
            }
        }
    }
}

/// How many units one word of a string of `format` holds; `None` for a
/// format no string has.
fn units_per_word(format: Format) -> Option<usize> {
    match format {
        Format::Bytes => Some(BYTES_PER_WORD),
        Format::ThirtyTwoBit => Some(THIRTY_TWO_BIT_UNITS_PER_WORD),
        _ => None,
    }
}

/// A String's or a Symbol's packed units, read in place: the one reader of
/// the layout `pack_code_points` writes.
#[derive(Clone, Copy)]
pub(in crate::heap) struct PackedString<'a> {
    format: Format,
    size: usize,
    words: &'a [u64],
}

impl<'a> PackedString<'a> {
    /// The string an object of `header` holding `words` is; `None` unless
    /// its format is `Bytes` or `ThirtyTwoBit`.
    pub(in crate::heap) fn new(header: Header, words: &'a [u64]) -> Option<PackedString<'a>> {
        let format = header.format();
        let per_word = units_per_word(format)?;
        let size = (words.len() * per_word).saturating_sub(usize::from(header.unused()));
        Some(PackedString {
            format,
            size,
            words,
        })
    }

    /// The number of code points.
    pub(in crate::heap) fn size(self) -> usize {
        self.size
    }

    /// The code point at the 0-based `index`; `None` past the end.
    pub(in crate::heap) fn code_point_at(self, index: usize) -> Option<u32> {
        if index >= self.size {
            return None;
        }
        let word = *self.words.get(index / units_per_word(self.format)?)?;
        Some(match self.format {
            Format::Bytes => u32::from(word.to_le_bytes()[index % BYTES_PER_WORD]),
            // The low half first; truncating keeps the half `index` names.
            _ => (word >> (32 * (index % THIRTY_TWO_BIT_UNITS_PER_WORD))) as u32,
        })
    }

    /// Every code point, in order.
    pub(in crate::heap) fn code_points(self) -> impl Iterator<Item = u32> + 'a {
        (0..self.size).filter_map(move |index| self.code_point_at(index))
    }

    /// The code points in the 0-based, half-open `start..end`, copied out;
    /// `None` when the range is not inside the string.
    pub(in crate::heap) fn slice(self, start: usize, end: usize) -> Option<crate::CodePoints> {
        if start > end || end > self.size {
            return None;
        }
        let units = (start..end).filter_map(move |index| self.code_point_at(index));
        Some(match self.format {
            // A byte string's units are Latin-1 code points, so they need
            // no check.
            Format::Bytes => crate::CodePoints::from_bytes(
                units.filter_map(|unit| u8::try_from(unit).ok()).collect(),
            ),
            _ => crate::CodePoints::from_wide(units.collect()),
        })
    }

    /// The whole text, copied out.
    pub(in crate::heap) fn to_code_points(self) -> crate::CodePoints {
        self.slice(0, self.size)
            .unwrap_or_else(|| crate::CodePoints::from(""))
    }

    /// Whether two strings hold the same code points. The width is
    /// canonical and the unused units are zero, so this compares the words.
    pub(in crate::heap) fn same_text(self, other: PackedString<'_>) -> bool {
        self.format == other.format && self.size == other.size && self.words == other.words
    }
}

/// The words an `AlignedBytes` object holds ahead of its data, so that the
/// data can begin at any of the four word positions of a 32-byte line.
pub(in crate::heap) const ALIGNMENT_RESERVE: usize = 3;
/// The data's alignment in words: 32 bytes, AVX2's width.
pub(in crate::heap) const ALIGNMENT_WORDS: usize = 4;

/// The field count of a `ByteArray` of `size` bytes: the reserve and the
/// data words.
pub(in crate::heap) fn byte_array_field_count(size: usize) -> usize {
    ALIGNMENT_RESERVE + size.div_ceil(BYTES_PER_WORD)
}

/// The absolute index of the first data word of the `AlignedBytes` object
/// whose header is at `header`: the first multiple of four at or after its
/// first field word. Computed, never stored: the space's base is
/// page-aligned, so a word index that is a multiple of four is a 32-byte
/// address, and the object's size stays the same wherever it moves.
pub(in crate::heap) fn aligned_data_start(header: usize) -> usize {
    (header + FIELDS).next_multiple_of(ALIGNMENT_WORDS)
}

/// Moves the data of the `AlignedBytes` object just copied, header and all,
/// from header index `old_header` to `new_header`, from the old block's
/// alignment to the new block's, within the new block. Without it the data
/// stay at the old block's offset and every read answers wrong bytes. The
/// reserve words are never read, so they are left as they are.
pub(in crate::heap) fn realign(words: &mut [u64], old_header: usize, new_header: usize) {
    // The header and every word after it: its length is the header, the
    // class word and the fields.
    let block = super::space::block_range(words, new_header);
    let data_words = block.len().saturating_sub(FIELDS + ALIGNMENT_RESERVE);
    let old_offset = aligned_data_start(old_header) - (old_header + FIELDS);
    let new_offset = aligned_data_start(new_header) - (new_header + FIELDS);
    if old_offset != new_offset {
        let source = new_header + FIELDS + old_offset;
        words.copy_within(
            source..source + data_words,
            new_header + FIELDS + new_offset,
        );
    }
}

/// Byte `index` of a run of data words, little-endian within each word.
pub(in crate::heap) fn data_byte(data: &[u64], index: usize) -> u8 {
    data[index / BYTES_PER_WORD].to_le_bytes()[index % BYTES_PER_WORD]
}

/// Writes byte `index` of a run of data words, as `data_byte` reads it.
pub(in crate::heap) fn set_data_byte(data: &mut [u64], index: usize, byte: u8) {
    let word = &mut data[index / BYTES_PER_WORD];
    let mut bytes = word.to_le_bytes();
    bytes[index % BYTES_PER_WORD] = byte;
    *word = u64::from_le_bytes(bytes);
}

/// Whether `range` lies inside a run of `size` bytes.
fn is_within(range: &std::ops::Range<usize>, size: usize) -> bool {
    range.start <= range.end && range.end <= size
}

/// A `ByteArray`'s data words and size, lent to a leaf that reads in place.
/// Every access is checked against the size, so that no index reaches the
/// unused bytes of the last word or past it.
#[derive(Clone, Copy)]
pub(crate) struct ByteArrayData<'a> {
    words: &'a [u64],
    size: usize,
}

impl<'a> ByteArrayData<'a> {
    pub(in crate::heap) fn new(words: &'a [u64], size: usize) -> ByteArrayData<'a> {
        debug_assert!(size <= words.len() * BYTES_PER_WORD);
        ByteArrayData { words, size }
    }

    /// The number of bytes.
    pub(crate) fn size(self) -> usize {
        self.size
    }

    /// The byte at the 0-based `index`; `None` past the end.
    pub(crate) fn byte(self, index: usize) -> Option<u8> {
        (index < self.size).then(|| data_byte(self.words, index))
    }

    /// The bytes in the 0-based, half-open `range`, copied out; `None` when
    /// the range is not inside the data.
    pub(crate) fn bytes(self, range: std::ops::Range<usize>) -> Option<Vec<u8>> {
        is_within(&range, self.size)
            .then(|| range.map(|index| data_byte(self.words, index)).collect())
    }

    /// The element of `kind` whose first byte is at the 0-based `offset`,
    /// in the order `big_endian` names; `None` when its bytes pass the
    /// end. A float32 is widened to an `f64` exactly.
    pub(crate) fn read_element(
        self,
        offset: usize,
        kind: ElementKind,
        big_endian: bool,
    ) -> Option<Element> {
        let width = kind.width();
        if offset.checked_add(width)? > self.size {
            return None;
        }
        // Gathered into little-endian order, so each kind converts from
        // one order.
        let mut bytes = [0u8; 8];
        for (index, byte) in bytes[..width].iter_mut().enumerate() {
            *byte = data_byte(self.words, offset + index);
        }
        if big_endian {
            bytes[..width].reverse();
        }
        Some(match kind {
            ElementKind::Int8 => Element::Integer(i8::from_le_bytes(leading(bytes)).into()),
            ElementKind::Uint8 => Element::Integer(u8::from_le_bytes(leading(bytes)).into()),
            ElementKind::Int16 => Element::Integer(i16::from_le_bytes(leading(bytes)).into()),
            ElementKind::Uint16 => Element::Integer(u16::from_le_bytes(leading(bytes)).into()),
            ElementKind::Int32 => Element::Integer(i32::from_le_bytes(leading(bytes)).into()),
            ElementKind::Uint32 => Element::Integer(u32::from_le_bytes(leading(bytes)).into()),
            ElementKind::Int64 => Element::Integer(i64::from_le_bytes(bytes).into()),
            ElementKind::Uint64 => Element::Integer(u64::from_le_bytes(bytes).into()),
            ElementKind::Float32 => Element::Float(f32::from_le_bytes(leading(bytes)).into()),
            ElementKind::Float64 => Element::Float(f64::from_le_bytes(bytes)),
        })
    }
}

/// An element of a typed array or a `DataView` as Rust holds it between
/// its bytes and its `Value`: an integer of any of the integer kinds, or a
/// float of either float kind.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Element {
    Integer(i128),
    Float(f64),
}

/// The first `N` of eight little-endian bytes.
fn leading<const N: usize>(bytes: [u8; 8]) -> [u8; N] {
    std::array::from_fn(|index| bytes[index])
}

/// A `ByteArray`'s data words and size, lent to a leaf that writes in
/// place. Bytes are no handles, so no write goes through the barrier; every
/// write is checked against the size and writes nothing when it fails, so
/// that the unused bytes of the last word stay zero.
pub(crate) struct ByteArrayDataMut<'a> {
    words: &'a mut [u64],
    size: usize,
}

impl<'a> ByteArrayDataMut<'a> {
    pub(in crate::heap) fn new(words: &'a mut [u64], size: usize) -> ByteArrayDataMut<'a> {
        debug_assert!(size <= words.len() * BYTES_PER_WORD);
        ByteArrayDataMut { words, size }
    }

    /// Writes `byte` at the 0-based `index`; answers whether it was inside
    /// the data.
    pub(crate) fn set_byte(&mut self, index: usize, byte: u8) -> bool {
        if index >= self.size {
            return false;
        }
        set_data_byte(self.words, index, byte);
        true
    }

    /// Copies the bytes in `source` to those starting at `destination`, as
    /// `memmove` does: right when the two overlap. Answers whether both lay
    /// inside the data; writes nothing when either does not.
    pub(crate) fn copy_within(
        &mut self,
        source: std::ops::Range<usize>,
        destination: usize,
    ) -> bool {
        let Some(end) = destination.checked_add(source.len()) else {
            return false;
        };
        if !is_within(&source, self.size) || end > self.size {
            return false;
        }
        // Forward when the destination lies below the source, backward
        // otherwise, so that no byte is overwritten before it is read.
        if destination <= source.start {
            for (offset, index) in source.enumerate() {
                let byte = data_byte(self.words, index);
                set_data_byte(self.words, destination + offset, byte);
            }
        } else {
            for (offset, index) in source.enumerate().rev() {
                let byte = data_byte(self.words, index);
                set_data_byte(self.words, destination + offset, byte);
            }
        }
        true
    }

    /// Writes `element` as `kind` with its first byte at the 0-based
    /// `offset`, in the order `big_endian` names, and answers whether it
    /// did: nothing is written when its bytes pass the end, or when the
    /// element is not of the kind's shape -- a float for an integer kind,
    /// an integer for a float kind. An integer is not range-checked, its
    /// low bytes stored, since the leaf has checked it; a float32 is
    /// rounded to nearest, ties to even, a finite value past its range
    /// becoming an infinity and a NaN staying a NaN.
    pub(crate) fn write_element(
        &mut self,
        offset: usize,
        kind: ElementKind,
        big_endian: bool,
        element: Element,
    ) -> bool {
        let width = kind.width();
        match offset.checked_add(width) {
            Some(end) if end <= self.size => {}
            _ => return false,
        }
        let mut bytes = [0u8; 8];
        match (kind, element) {
            (ElementKind::Int8, Element::Integer(value)) => {
                bytes[..1].copy_from_slice(&(value as i8).to_le_bytes());
            }
            (ElementKind::Uint8, Element::Integer(value)) => {
                bytes[..1].copy_from_slice(&(value as u8).to_le_bytes());
            }
            (ElementKind::Int16, Element::Integer(value)) => {
                bytes[..2].copy_from_slice(&(value as i16).to_le_bytes());
            }
            (ElementKind::Uint16, Element::Integer(value)) => {
                bytes[..2].copy_from_slice(&(value as u16).to_le_bytes());
            }
            (ElementKind::Int32, Element::Integer(value)) => {
                bytes[..4].copy_from_slice(&(value as i32).to_le_bytes());
            }
            (ElementKind::Uint32, Element::Integer(value)) => {
                bytes[..4].copy_from_slice(&(value as u32).to_le_bytes());
            }
            (ElementKind::Int64, Element::Integer(value)) => {
                bytes = (value as i64).to_le_bytes();
            }
            (ElementKind::Uint64, Element::Integer(value)) => {
                bytes = (value as u64).to_le_bytes();
            }
            (ElementKind::Float32, Element::Float(value)) => {
                bytes[..4].copy_from_slice(&(value as f32).to_le_bytes());
            }
            (ElementKind::Float64, Element::Float(value)) => {
                bytes = value.to_le_bytes();
            }
            _ => return false,
        }
        if big_endian {
            bytes[..width].reverse();
        }
        for (index, byte) in bytes[..width].iter().enumerate() {
            set_data_byte(self.words, offset + index, *byte);
        }
        true
    }

    /// Writes `bytes` starting at the 0-based `destination`; answers whether
    /// they fit inside the data, and writes nothing when they do not.
    pub(crate) fn write(&mut self, destination: usize, bytes: &[u8]) -> bool {
        match destination.checked_add(bytes.len()) {
            Some(end) if end <= self.size => {
                for (offset, byte) in bytes.iter().enumerate() {
                    set_data_byte(self.words, destination + offset, *byte);
                }
                true
            }
            _ => false,
        }
    }
}

/// The units of a string's element width one word holds.
const BYTES_PER_WORD: usize = 8;
const THIRTY_TWO_BIT_UNITS_PER_WORD: usize = 2;

/// Traces pointer fields kept as the bits of a stored `Word`.
fn trace_word_bits<T: Tracer>(words: &mut [u64], tracer: &mut T) {
    for bits in words {
        let mut word = Word::from_bits(*bits);
        tracer.word(&mut word);
        *bits = word.bits();
    }
}

/// The fixed fields of a closure object, the words before its copied values
/// (`docs/design/object-model.md`): each a stored `Word`, written through
/// `Heap::store` and read through `Heap::load` like an instance's.
pub(in crate::heap) mod closure_field {
    /// A `SmallInteger`, the `CodeId` of the block's compiled body.
    pub(in crate::heap) const CODE_ID: usize = 0;
    /// The frame object of the activation the closure was created in. A
    /// closure keeps it whatever it captured: `enclosingActivation` and the
    /// debugger's "[] in Foo bar" are read off it (`docs/design/execution.md`,
    /// "The closure keeps its defining activation").
    pub(in crate::heap) const CAPTURED_FRAME: usize = 1;
    /// A `SmallInteger`, the raw start of the closure's family's range in the
    /// heap's cache table, copied from the activation that created it, since
    /// a block belongs to its method's family. Never serialized: a decoded
    /// closure carries `NOT_ASSIGNED` and is given a fresh range when it is
    /// first activated.
    pub(in crate::heap) const FEEDBACK_BASE: usize = 2;
    /// How many fixed fields there are: copied value `index` is word
    /// `COUNT + index`. What the closure was handed when it was created is a
    /// copied variable's value or an enclosing body's indirection vector; its
    /// registers are `code.argument_count + position`.
    pub(in crate::heap) const COUNT: usize = 3;
}

/// The fixed fields of a frame object, the words before its registers
/// (`docs/design/object-model.md`): each a stored `Word`, written through
/// `Heap::store` and read through `Heap::load` like an instance's.
pub(in crate::heap) mod frame_field {
    /// The frame object of the activation this one returns to, or nil: a
    /// live activation's sender is the stack entry below it, and a returned
    /// one has none, so only a decoded frame carries one.
    pub(in crate::heap) const SENDER: usize = 0;
    /// A closure activation's home frame object; nil for every other kind.
    pub(in crate::heap) const HOME: usize = 1;
    pub(in crate::heap) const RECEIVER: usize = 2;
    /// A class object, or nil for a top-level activation (§3.6).
    pub(in crate::heap) const CURRENT_CLASS: usize = 3;
    /// A `SmallInteger`: an instruction index into the frame's code.
    pub(in crate::heap) const PROGRAM_COUNTER: usize = 4;
    /// A `SmallInteger`, the `CodeId` of the body that runs in the frame,
    /// or nil when there is none to name: a top-level frame's.
    pub(in crate::heap) const CODE_ID: usize = 5;
    /// A `SmallInteger`, the frame kind's wire tag (`opcodes::FRAME_*`).
    pub(in crate::heap) const KIND: usize = 6;
    /// The closure of a closure activation, the class of the four
    /// class-carrying kinds, nil otherwise.
    pub(in crate::heap) const KIND_OPERAND: usize = 7;
    /// `true` once the activation has returned.
    pub(in crate::heap) const UNCONTINUABLE: usize = 8;
    /// A `SmallInteger`, the stack entry index of the activation the frame
    /// is married to, while it runs; nil once it has returned.
    pub(in crate::heap) const MARRIED: usize = 9;
    /// How many fixed fields there are: register `index` is word
    /// `COUNT + index`.
    pub(in crate::heap) const COUNT: usize = 10;
}

/// The fields of a class object (`docs/design/object-model.md`): each a
/// stored `Word`, written through `Heap::store` and read through
/// `Heap::load`. A class object is *identity* -- what it can *do* lives on
/// its `MixinApplication`, named by `APPLICATION` -- so two class objects
/// built from the same mixin and the same superclass application share one
/// application, which is what lets an inline cache stay monomorphic across
/// enclosing instances.
pub(in crate::heap) mod class_field {
    /// The enclosing object, or nil for a class no declaration encloses:
    /// `newspeak-spec.pdf` §6 makes it the nested class's only state, which
    /// is why a class object cannot be shared between instances of its
    /// enclosing class.
    pub(in crate::heap) const ENCLOSING: usize = 0;
    /// The class object of the level whose declaration *lexically contains*
    /// this class's declaration, or nil for the module's root class and a
    /// builtin. A metaclass copies its class's, so a class-side `outer`
    /// walks the same chain. `outer` hops along this field, never along the
    /// reached object's own class -- those differ exactly when the
    /// declaration is inherited across a nesting boundary
    /// (`dispatch::kth_enclosing_object`).
    pub(in crate::heap) const DECLARING_CLASS: usize = 1;
    /// The per-instance class-object chain, parallel to the application
    /// chain, or nil; `outer`, `super` and lookup step it.
    pub(in crate::heap) const SUPERCLASS: usize = 2;
    /// For a metaclass, the class object whose class side it applies; nil
    /// for every other class object. The other direction needs no field: a
    /// class object's metaclass is its header class.
    pub(in crate::heap) const THIS_CLASS: usize = 3;
    /// A `SmallInteger`, the position in `BuiltinClass::ALL` of the
    /// core-library class this is, or nil for a user-defined class: for the
    /// image's class tags, `allocate_for_factory` and a send's per-send re-check.
    pub(in crate::heap) const BUILTIN: usize = 4;
    /// Nil, or an `Array` of the object whose not being a value refuses
    /// instantiating this value class and the name of the level it
    /// encloses, a Symbol, or nil for the class's own
    /// (`ValueClassRefusal`, which says where it is decided).
    pub(in crate::heap) const VALUE_CLASS_REFUSAL: usize = 5;
    /// For a level a `<:` clause added (`newspeak-spec.pdf` §6.2), the
    /// clause as `AppliedClause::pack` packs it; nil for every other class
    /// object. It cannot live on the level's mixin, which every declaration
    /// applying it shares, nor on its application, which the application
    /// cache shares between two declarations applying the same mixin with
    /// different messages.
    pub(in crate::heap) const APPLIED_CLAUSE: usize = 6;
    /// A `SmallInteger`, the `ApplicationIndex` of the class's behaviour:
    /// code, layout and the superclass chain.
    pub(in crate::heap) const APPLICATION: usize = 7;
    /// How many fields every class object has.
    pub(in crate::heap) const COUNT: usize = 8;
    /// For a level a `<:` clause added, and only there, one field past
    /// `COUNT`: the class object the clause named. §6.1 checks the clause
    /// against that class's primary factory, which is not always the
    /// level's own -- a body-less declaration's class object is the level
    /// its last clause adds and has its declaration's factory -- and the
    /// image rebuilds the level's metaclass from it
    /// (`Heap::class_named`). Only a level pays the word, so a metaclass,
    /// the smallest class record an image writes, keeps its size.
    pub(in crate::heap) const NAMED_CLASS: usize = COUNT;
    /// How many fields a level a `<:` clause added has.
    pub(in crate::heap) const APPLIED_LEVEL_COUNT: usize = COUNT + 1;
    /// For a body-less declaration's class object (`Mixin::has_body`), and
    /// only there, two fields past `NAMED_CLASS`: the enclosing object the
    /// declaration was evaluated in, nil for a module's root -- §3.3's
    /// "object of which C is a member", which a body version of the
    /// declaration would have as its own -- and the class object of the
    /// level whose declaration lexically contains it, nil for a root. The
    /// class object's `ENCLOSING` and `DECLARING_CLASS` are the named
    /// class's, which the applied mixin's code walks out from (§6.2); only
    /// the declaration's own code -- its full initializer, which is the
    /// clause messages and their arguments, and the blocks in it -- walks
    /// out from these (`Heap::class_declaration_scope`,
    /// `interpreter::sends::lexical_origin`).
    pub(in crate::heap) const DECLARATION_ENCLOSING: usize = APPLIED_LEVEL_COUNT;
    /// See `DECLARATION_ENCLOSING`.
    pub(in crate::heap) const DECLARATION_DECLARING_CLASS: usize = APPLIED_LEVEL_COUNT + 1;
    /// How many fields a body-less declaration's class object has.
    pub(in crate::heap) const BODY_LESS_CLASS_COUNT: usize = APPLIED_LEVEL_COUNT + 2;
    /// The fields that hold an object or nil; the others are numbers.
    /// `NAMED_CLASS` and the two declaration fields lie past an ordinary
    /// class object's fields, and the declaration fields past a level's,
    /// where `pointer_ranges` clamps their ranges to nothing.
    pub(in crate::heap) const POINTERS: [usize; 8] = [
        ENCLOSING,
        DECLARING_CLASS,
        SUPERCLASS,
        THIS_CLASS,
        VALUE_CLASS_REFUSAL,
        NAMED_CLASS,
        DECLARATION_ENCLOSING,
        DECLARATION_DECLARING_CLASS,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixin::DispatchId;

    #[test]
    fn pointer_ranges_name_the_class_word_and_each_formats_pointer_fields() {
        let block = |format: Format, fields: usize| {
            let mut words = vec![Word::NIL.bits(); FIELDS + fields];
            words[HEADER] = Header::new(DispatchId::NONE, format, 0, 1 + fields).0;
            words
        };
        let covered =
            |words: &[u64]| -> Vec<usize> { pointer_ranges(words).into_iter().flatten().collect() };
        assert_eq!(covered(&block(Format::FixedPointers, 2)), vec![CLASS, 2, 3]);
        // Forwarding reads every weak word.
        assert_eq!(covered(&block(Format::WeakPointers, 2)), vec![CLASS, 2, 3]);
        assert_eq!(covered(&block(Format::Ephemeron, 3)), vec![CLASS, 2, 3, 4]);
        assert_eq!(covered(&block(Format::SixtyFourBit, 2)), vec![CLASS]);
        // A ByteArray's reserve and data are bytes, never handles.
        assert_eq!(
            covered(&block(Format::AlignedBytes, byte_array_field_count(9))),
            vec![CLASS]
        );
        assert_eq!(
            covered(&block(Format::Closure, closure_field::COUNT + 1)),
            vec![
                CLASS,
                FIELDS + closure_field::CAPTURED_FRAME,
                FIELDS + closure_field::COUNT
            ]
        );
        // An ordinary class object has no named class; a level has it.
        let class = block(Format::Class, class_field::COUNT);
        let mut expected = vec![CLASS];
        expected.extend(
            class_field::POINTERS
                .iter()
                .filter(|field| **field < class_field::COUNT)
                .map(|field| FIELDS + field),
        );
        assert_eq!(covered(&class), expected);
        let level = block(Format::Class, class_field::APPLIED_LEVEL_COUNT);
        let mut expected = vec![CLASS];
        expected.extend(
            class_field::POINTERS
                .iter()
                .filter(|field| **field < class_field::APPLIED_LEVEL_COUNT)
                .map(|field| FIELDS + field),
        );
        assert_eq!(covered(&level), expected);
        // A body-less declaration's class object has its declaration's
        // enclosing object and declaring class besides.
        let body_less = block(Format::Class, class_field::BODY_LESS_CLASS_COUNT);
        let mut expected = vec![CLASS];
        expected.extend(class_field::POINTERS.iter().map(|field| FIELDS + field));
        assert_eq!(covered(&body_less), expected);
        // A frame's registers count only once it is no longer married.
        let mut frame = block(Format::Frame, frame_field::COUNT + 2);
        assert_eq!(covered(&frame).len(), 1 + frame_field::COUNT + 2);
        frame[FIELDS + frame_field::MARRIED] = 0b1001; // a SmallInteger
        assert_eq!(covered(&frame).len(), 1 + frame_field::COUNT);
    }

    #[test]
    fn strong_pointer_ranges_leave_out_weak_words_and_the_fields_of_a_dead_key() {
        let block = |format: Format, fields: usize| {
            let mut words = vec![Word::NIL.bits(); FIELDS + fields];
            words[HEADER] = Header::new(DispatchId::NONE, format, 0, 1 + fields).0;
            words
        };
        let strong = |words: &[u64], survives: bool| -> Vec<usize> {
            strong_pointer_ranges(Header(words[HEADER]).format(), words, |_| survives)
                .into_iter()
                .flatten()
                .collect()
        };
        assert_eq!(strong(&block(Format::WeakPointers, 2), true), vec![CLASS]);
        assert_eq!(strong(&block(Format::Ephemeron, 3), false), vec![CLASS]);
        assert_eq!(
            strong(&block(Format::Ephemeron, 3), true),
            vec![CLASS, 2, 3, 4]
        );
        // No key, nothing to wait for.
        assert_eq!(strong(&block(Format::Ephemeron, 0), false), vec![CLASS]);
        assert_eq!(
            strong(&block(Format::FixedPointers, 2), false),
            vec![CLASS, 2, 3]
        );
    }

    #[test]
    fn pointer_ranges_of_a_decoded_format_answer_what_pointer_ranges_answers() {
        let formats = [
            Format::FixedPointers,
            Format::IndexablePointers,
            Format::Frame,
            Format::Closure,
            Format::SixtyFourBit,
            Format::Forwarded,
            Format::Free,
            Format::Class,
            Format::ThirtyTwoBit,
            Format::WeakPointers,
            Format::Ephemeron,
            Format::Bytes,
            Format::AlignedBytes,
        ];
        for format in formats {
            // Long enough for every format's fields, and short ones too:
            // the contract clamps rather than panics.
            for fields in [0, 1, class_field::COUNT + 3] {
                let mut words = vec![Word::NIL.bits(); FIELDS + fields];
                words[HEADER] = Header::new(DispatchId::NONE, format, 0, 1 + fields).0;
                assert_eq!(
                    pointer_ranges_of(format, &words),
                    pointer_ranges(&words),
                    "{format:?} with {fields} fields"
                );
                let header_only = &words[..=HEADER];
                assert_eq!(
                    pointer_ranges_of(format, header_only),
                    pointer_ranges(header_only),
                    "{format:?} with its header alone"
                );
            }
        }
        // A block without a header has no pointer words in any format.
        assert!(pointer_ranges_of(Format::FixedPointers, &[])
            .iter()
            .all(|range| range.is_empty()));
    }
}
