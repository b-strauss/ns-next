//! What sits behind an `ObjectReference` and a `FrameReference`: the block
//! of words every object is, a reified activation's included -- word
//! `HEADER` the header, word `CLASS` the class word, then the fields -- and
//! how the collector walks one by its format. Private to the heap module --
//! nothing outside `heap.rs` and its submodules names these, so that
//! changing how an object or a frame is represented changes this module
//! rather than its callers (`docs/design/memory.md`, "The seam"). Callers
//! build objects through `Heap`'s typed allocations and the value types of
//! `records.rs`, and read them through its accessors.

use crate::heap::header::{Format, Header};
use crate::heap::Word;
use crate::mixin::DispatchId;
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
/// unobservable. Never derived from the arena index, which a moving
/// collector or `become:` would change (liveness spec's stable-identity
/// constraint). Not a handle; a move carries it along.
pub(in crate::heap) const HEADER: usize = 0;
/// Where a block keeps its class word: a stored `Word` naming the class
/// object, written through `Heap::store` and read through `Heap::load`.
pub(in crate::heap) const CLASS: usize = 1;
/// Where a block's fields start: field `index` is word `FIELDS + index`.
pub(in crate::heap) const FIELDS: usize = 2;

/// What an object is about to hold, before it has a class: its format, the
/// units its last word leaves unused, a `LargeInteger`'s sign and its field
/// words, read by format (`docs/design/memory.md`): `FixedPointers`, an
/// instance's fields; `IndexablePointers`, an `Array`'s elements -- both as
/// the bits of a stored `Word`, so only `Heap::store` writes one and only
/// `Heap::load` reads one; `SixtyFourBit`, a `LargeInteger`'s magnitude,
/// least significant digit first, its sign the header's negative bit, or a
/// boxed `Float`'s one word of `f64::to_bits` -- the class's builtin tells
/// those two apart, as Spur's does; `Bytes` and `ThirtyTwoBit`, a String's
/// or a Symbol's code points packed little-endian, eight or two to a word
/// (`PackedString`), the class's builtin again telling the two apart;
/// `Frame`, a reified activation (`Payload::frame`); `Closure`, a block
/// closure (`Payload::closure`); `Class`, a class object
/// (`Payload::class`).
///
/// Build one through `Payload::instance`, `array`, `large_integer`, `float`,
/// `string`, `frame`, `closure` or `class`, the one place each kind's layout
/// is decided, and turn it into an arena slot with `into_block`, the one
/// place a header is derived. The words already leave room for the header
/// and the class word, so that the block is the one allocation an object
/// costs.
///
/// A `LargeInteger` never holds a value a `SmallInteger` does, and a boxed
/// `Float` never one inside the immediate range: `Mutator::new_integer` and
/// `Mutator::new_float` are the one way to make either and decide it, so
/// each number has one representation.
pub(in crate::heap) struct Payload {
    format: Format,
    /// The units the last word leaves unused: 0 unless `format` is `Bytes`
    /// or `ThirtyTwoBit`.
    unused: u8,
    negative: bool,
    /// The block to be: `FIELDS` words left for the header and the class
    /// word, then the fields.
    block: Vec<u64>,
}

impl Payload {
    /// An instance's payload: `fields`, each a stored word.
    pub(in crate::heap) fn instance(fields: impl IntoIterator<Item = Word>) -> Self {
        Self::pointers(Format::FixedPointers, fields)
    }

    /// An `Array`'s payload: `elements`, each a stored word.
    pub(in crate::heap) fn array(elements: impl IntoIterator<Item = Word>) -> Self {
        Self::pointers(Format::IndexablePointers, elements)
    }

    /// A reified activation's payload: `fields`, the fixed fields
    /// `frame_field` names, then `register_count` registers, every one nil.
    pub(in crate::heap) fn frame(
        fields: [Word; frame_field::COUNT],
        register_count: usize,
    ) -> Self {
        Self::pointers(
            Format::Frame,
            fields
                .into_iter()
                .chain(std::iter::repeat_n(Word::NIL, register_count)),
        )
    }

    /// A block closure's payload: `fields`, the fixed fields `closure_field`
    /// names, then `copied`, each a stored word.
    pub(in crate::heap) fn closure(
        fields: [Word; closure_field::COUNT],
        copied: impl IntoIterator<Item = Word>,
    ) -> Self {
        Self::pointers(Format::Closure, fields.into_iter().chain(copied))
    }

    /// A class object's payload: the fields `class_field` names.
    pub(in crate::heap) fn class(fields: [Word; class_field::COUNT]) -> Self {
        Self::pointers(Format::Class, fields)
    }

    fn pointers(format: Format, words: impl IntoIterator<Item = Word>) -> Self {
        Self::of_words(format, false, words.into_iter().map(Word::bits))
    }

    fn of_words(format: Format, negative: bool, words: impl IntoIterator<Item = u64>) -> Self {
        let words = words.into_iter();
        let mut block = Vec::with_capacity(FIELDS + words.size_hint().0);
        block.resize(FIELDS, 0);
        block.extend(words);
        Payload {
            format,
            unused: 0,
            negative,
            block,
        }
    }

    /// A `LargeInteger`'s payload: its magnitude's digits and its sign.
    pub(in crate::heap) fn large_integer(integer: &crate::ArbitraryInteger) -> Self {
        Self::of_words(
            Format::SixtyFourBit,
            integer.is_negative(),
            integer.magnitude().iter().copied(),
        )
    }

    /// A boxed `Float`'s payload: one word of its bits.
    pub(in crate::heap) fn float(value: f64) -> Self {
        Self::of_words(Format::SixtyFourBit, false, [value.to_bits()])
    }

    /// A String's or a Symbol's payload: `text`'s code points, one byte
    /// each while every one fits Latin-1 and 32 bits each otherwise, as
    /// `CodePoints` keeps them, so that `size` and `at:` stay one access.
    /// Unit `index` is byte `index % 8` of word `index / 8`, or half
    /// `index % 2` of word `index / 2`, least significant first; the last
    /// word's unused units are zero, so that equal texts have equal words.
    pub(in crate::heap) fn string(text: &crate::CodePoints) -> Self {
        let mut payload = match text.as_wide() {
            None => Self::of_words(
                Format::Bytes,
                false,
                text.as_bytes()
                    .unwrap_or_default()
                    .chunks(BYTES_PER_WORD)
                    .map(|chunk| {
                        let mut bytes = [0u8; BYTES_PER_WORD];
                        bytes[..chunk.len()].copy_from_slice(chunk);
                        u64::from_le_bytes(bytes)
                    }),
            ),
            Some(units) => Self::of_words(
                Format::ThirtyTwoBit,
                false,
                units.chunks(THIRTY_TWO_BIT_UNITS_PER_WORD).map(|chunk| {
                    chunk
                        .iter()
                        .rev()
                        .fold(0, |word, unit| (word << 32) | u64::from(*unit))
                }),
            ),
        };
        let per_word = units_per_word(payload.format).unwrap_or(1);
        let words = payload.block.len() - FIELDS;
        // Below `per_word`, which is at most 8.
        payload.unused = u8::try_from(words * per_word - text.size()).unwrap_or(0);
        payload
    }

    /// The field words, for a constructor that fills some in once the
    /// layout is decided.
    pub(in crate::heap) fn fields_mut(&mut self) -> &mut [u64] {
        &mut self.block[FIELDS..]
    }

    /// The arena slot of an object of the class `class` names, holding this
    /// payload, guarding on `guard`, with `identity_hash` (0: none yet): its
    /// header -- its format, its size as road step 3's layout stores it, the
    /// class word plus the fields, and a `LargeInteger`'s sign -- its class
    /// word, then its fields. The one place a header is derived.
    pub(in crate::heap) fn into_block(
        self,
        class: Word,
        guard: DispatchId,
        identity_hash: u32,
    ) -> Box<[u64]> {
        let Payload {
            format,
            unused,
            negative,
            mut block,
        } = self;
        let header = Header::new(guard, format, unused, block.len() - CLASS)
            .with_negative(negative)
            .with_identity_hash(identity_hash);
        block[HEADER] = header.0;
        block[CLASS] = class.bits();
        block.into_boxed_slice()
    }
}

/// Hands every word of `block` that holds a stored `Word` to `tracer`,
/// deciding which by the header's format alone: the class word always;
/// every field of `FixedPointers` and `IndexablePointers`; a class object's
/// object fields; a closure's captured frame and copied values; a frame's
/// fixed fields, and its registers once it is no longer married; nothing of
/// `SixtyFourBit`, `Bytes` and `ThirtyTwoBit`, whose digits and code points
/// would read as handles.
pub(in crate::heap) fn trace_block<T: Tracer>(block: &mut [u64], tracer: &mut T) {
    let Some(&header) = block.get(HEADER) else {
        return;
    };
    let Some((class, fields)) = block.get_mut(CLASS..).and_then(<[u64]>::split_first_mut) else {
        return;
    };
    trace_word_bits(std::slice::from_mut(class), tracer);
    match Header(header).format() {
        Format::FixedPointers | Format::IndexablePointers => trace_word_bits(fields, tracer),
        // A married frame's registers are its stack entry's, traced with
        // the stack; its own stay nil until the divorce copies them in, so
        // they are skipped rather than walked for nothing.
        Format::Frame => {
            let married = fields
                .get(frame_field::MARRIED)
                .is_some_and(|bits| *bits != Word::NIL.bits());
            let traced = if married {
                frame_field::COUNT.min(fields.len())
            } else {
                fields.len()
            };
            trace_word_bits(&mut fields[..traced], tracer);
        }
        // A closure's code id and feedback base are numbers; its captured
        // frame and its copied values are the pointers.
        Format::Closure => {
            let fixed = closure_field::COUNT.min(fields.len());
            let (head, copied) = fields.split_at_mut(fixed);
            if let Some(frame) =
                head.get_mut(closure_field::CAPTURED_FRAME..=closure_field::CAPTURED_FRAME)
            {
                trace_word_bits(frame, tracer);
            }
            trace_word_bits(copied, tracer);
        }
        // A class object's builtin, applied clause and application are
        // numbers; the rest are objects or nil.
        Format::Class => {
            for field in class_field::POINTERS {
                if let Some(word) = fields.get_mut(field..=field) {
                    trace_word_bits(word, tracer);
                }
            }
        }
        // Digits and code points, no stored words; and no block in the
        // arena is forwarded or free.
        Format::SixtyFourBit
        | Format::Bytes
        | Format::ThirtyTwoBit
        | Format::Forwarded
        | Format::Free => {}
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
/// the layout `Payload::string` decides.
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
    /// image's class tags, `decide_factory` and a send's per-send re-check.
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
    /// How many fields there are.
    pub(in crate::heap) const COUNT: usize = 8;
    /// The fields that hold an object or nil; the others are numbers.
    pub(in crate::heap) const POINTERS: [usize; 5] = [
        ENCLOSING,
        DECLARING_CLASS,
        SUPERCLASS,
        THIS_CLASS,
        VALUE_CLASS_REFUSAL,
    ];
}
