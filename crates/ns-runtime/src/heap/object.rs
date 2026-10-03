//! What sits behind an `ObjectReference` and a `FrameReference`: the Rust
//! payload of every object, a reified activation's included. Private to the
//! heap module -- nothing outside `heap.rs` and its submodules names these
//! types, so that changing how an object or a frame is represented changes
//! this module rather than its callers (`docs/design/memory.md`, "The
//! seam"). Callers build objects through `Heap`'s typed allocations and the
//! value types of `records.rs`, and read them through its accessors.

use std::rc::Rc;

use crate::heap::header::{Format, Header};
use crate::heap::{ClassReference, ObjectReference, Word};
use crate::mixin::DispatchId;
use crate::trace::{trace_optional_object, Trace, Tracer};
use crate::types::{BuiltinClass, ValueClassRefusal};

#[derive(Debug, Clone)]
pub(in crate::heap) struct Object<'gc> {
    pub(in crate::heap) class: ClassReference<'gc>,
    /// The guard an inline cache compares, the format, the identity hash and
    /// the size (`header.rs` gives the layout).
    ///
    /// The guard is the id an inline cache guards this object's sends on
    /// (`docs/design/dispatch.md`, "The dispatch id"): its class's instances
    /// id -- a class object's class is its metaclass. A copy, so that a
    /// guard is one load and one compare rather than a walk to the class --
    /// which makes every place a class's application changes responsible for
    /// restamping it (`Heap::class_set_application`,
    /// `Heap::stamp_dispatch_ids`).
    ///
    /// The identity hash is 0 until something asks for it; then drawn from
    /// a counter (`Heap::object_ensure_identity_hash`), so allocation order
    /// stays unobservable. Never derived from the arena index, which a
    /// moving collector or `become:` would change (liveness spec's
    /// stable-identity constraint). Not a handle; a move carries it along.
    pub(in crate::heap) header: Header,
    pub(in crate::heap) data: ObjectData<'gc>,
}

impl<'gc> Object<'gc> {
    /// An object of `class` holding `data`, guarding on `guard`, with
    /// `identity_hash` (0: none yet) and the format and size `data` has.
    pub(in crate::heap) fn new(
        class: ClassReference<'gc>,
        guard: DispatchId,
        identity_hash: u32,
        data: ObjectData<'gc>,
    ) -> Object<'gc> {
        let header = data.header(guard).with_identity_hash(identity_hash);
        Object {
            class,
            header,
            data,
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::heap) enum ObjectData<'gc> {
    /// An object whose payload is plain words, read by its header's format
    /// (`docs/design/memory.md`): `FixedPointers`, an instance's fields;
    /// `IndexablePointers`, an `Array`'s elements -- both as the bits of a
    /// stored `Word`, so only `Heap::store` writes one and only `Heap::load`
    /// reads one; `SixtyFourBit`, a `LargeInteger`'s magnitude, least
    /// significant digit first, its sign the header's negative bit, or a
    /// boxed `Float`'s one word of `f64::to_bits` -- the class's builtin
    /// tells those two apart, as Spur's does; `Bytes` and `ThirtyTwoBit`, a
    /// String's or a Symbol's code points packed little-endian, eight or two
    /// to a word (`PackedString`), the class's builtin again telling the
    /// two apart; `Frame`, a reified activation (`ObjectData::frame`);
    /// `Closure`, a block closure (`ObjectData::closure`).
    ///
    /// `format`, `unused` and `negative` are what `Object::new` derives the
    /// header from; once built, readers ask the header. Build one through
    /// `ObjectData::instance`, `array`, `large_integer`, `float` or
    /// `string`, the one place each kind's layout is decided.
    ///
    /// A `LargeInteger` never holds a value a `SmallInteger` does, and a
    /// boxed `Float` never one inside the immediate range:
    /// `Mutator::new_integer` and `Mutator::new_float` are the one way to
    /// make either and decide it, so each number has one representation.
    Words {
        format: Format,
        /// The units the last word leaves unused: 0 unless `format` is
        /// `Bytes` or `ThirtyTwoBit`.
        unused: u8,
        negative: bool,
        words: Box<[u64]>,
    },
    /// Boxed: a class object's data is what would otherwise set every
    /// object's size, and class objects are few (`docs/design/dispatch.md`,
    /// "The dispatch id").
    Class(Box<ClassData<'gc>>),
}

impl ObjectData<'_> {
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

    fn pointers(format: Format, words: impl IntoIterator<Item = Word>) -> Self {
        ObjectData::Words {
            format,
            unused: 0,
            negative: false,
            words: words.into_iter().map(Word::bits).collect(),
        }
    }

    /// A `LargeInteger`'s payload: its magnitude's digits and its sign.
    pub(in crate::heap) fn large_integer(integer: &crate::ArbitraryInteger) -> Self {
        ObjectData::Words {
            format: Format::SixtyFourBit,
            unused: 0,
            negative: integer.is_negative(),
            words: integer.magnitude().into(),
        }
    }

    /// A boxed `Float`'s payload: one word of its bits.
    pub(in crate::heap) fn float(value: f64) -> Self {
        ObjectData::Words {
            format: Format::SixtyFourBit,
            unused: 0,
            negative: false,
            words: Box::new([value.to_bits()]),
        }
    }

    /// A String's or a Symbol's payload: `text`'s code points, one byte
    /// each while every one fits Latin-1 and 32 bits each otherwise, as
    /// `CodePoints` keeps them, so that `size` and `at:` stay one access.
    /// Unit `index` is byte `index % 8` of word `index / 8`, or half
    /// `index % 2` of word `index / 2`, least significant first; the last
    /// word's unused units are zero, so that equal texts have equal words.
    pub(in crate::heap) fn string(text: &crate::CodePoints) -> Self {
        let (format, words): (Format, Box<[u64]>) = match text.as_wide() {
            None => (
                Format::Bytes,
                text.as_bytes()
                    .unwrap_or_default()
                    .chunks(BYTES_PER_WORD)
                    .map(|chunk| {
                        let mut bytes = [0u8; BYTES_PER_WORD];
                        bytes[..chunk.len()].copy_from_slice(chunk);
                        u64::from_le_bytes(bytes)
                    })
                    .collect(),
            ),
            Some(units) => (
                Format::ThirtyTwoBit,
                units
                    .chunks(THIRTY_TWO_BIT_UNITS_PER_WORD)
                    .map(|chunk| {
                        chunk
                            .iter()
                            .rev()
                            .fold(0, |word, unit| (word << 32) | u64::from(*unit))
                    })
                    .collect(),
            ),
        };
        let per_word = units_per_word(format).unwrap_or(1);
        // Below `per_word`, which is at most 8.
        let unused = u8::try_from(words.len() * per_word - text.size()).unwrap_or(0);
        ObjectData::Words {
            format,
            unused,
            negative: false,
            words,
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
/// the layout `ObjectData::string` decides.
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
/// A class object's words after its class word, in the layout road step 3
/// gives it.
const CLASS_WORDS: usize = 8;

impl ObjectData<'_> {
    /// The header an object holding this payload starts with: its format,
    /// its size as the word layout of road step 3 stores it -- the class
    /// word plus the payload's words -- and a `LargeInteger`'s sign.
    fn header(&self, guard: DispatchId) -> Header {
        let (format, unused, payload_words) = match self {
            ObjectData::Words {
                format,
                unused,
                negative,
                words,
            } => {
                let header = Header::new(guard, *format, *unused, 1 + words.len());
                return header.with_negative(*negative);
            }
            ObjectData::Class(_) => (Format::Class, 0, CLASS_WORDS),
        };
        Header::new(guard, format, unused, 1 + payload_words)
    }
}

/// A class object is *identity*; everything a class object can *do* lives
/// on its `MixinApplication`. Two class objects built from the same mixin
/// and the same superclass application share one `application`, which is
/// what lets an inline cache stay monomorphic across enclosing instances
/// once nested classes give a mixin one class object per enclosing object.
#[derive(Debug, Clone)]
pub(in crate::heap) struct ClassData<'gc> {
    /// Behaviour: code, layout and the superclass chain. Shared with every
    /// other class object built from the same mixin and superclass.
    pub(in crate::heap) application: Rc<crate::mixin::MixinApplication>,
    /// `newspeak-spec.pdf` §6: the enclosing object is the nested class's
    /// only state, which is why a class object cannot be shared between
    /// instances of its enclosing class.
    pub(in crate::heap) enclosing: Option<ObjectReference<'gc>>,
    /// The class object of the level whose declaration *lexically contains*
    /// this class's declaration -- `Lookup::NestedClass`'s `ClassReference`,
    /// the level `interpreter::nested` evaluated the declaration at. `None` for
    /// a class that no declaration encloses: the module's root class, and a
    /// builtin. A metaclass copies the class it belongs to (§6.3 puts a class
    /// side in the same declaration as its instance side), so a class-side
    /// `outer` walks the same chain.
    ///
    /// This is what makes the enclosing-object chain of §3.3 *lexical* the
    /// whole way up rather than only at its first step. `enclosing` alone
    /// gives one hop; hopping again needs the class object of the level
    /// just reached, and that is this field, not the reached object's own
    /// class -- those differ exactly when the declaration is inherited
    /// across a nesting boundary. See `dispatch::kth_enclosing_object`.
    pub(in crate::heap) declaring_class: Option<ClassReference<'gc>>,
    /// The per-instance class-object chain, parallel to the application
    /// chain. `outer` and `super` need it, and lookup steps it beside the
    /// application chain for each level's class object -- the level a found
    /// method runs in.
    pub(in crate::heap) superclass: Option<ClassReference<'gc>>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// core-library classes the bootstrap fills from `Kernel.ns` (or the VM
    /// mirror's class), for the image's class tags and for `decide_factory`,
    /// which instantiates `Array` with its own payload and refuses the
    /// mirror's class. Lookup does not read it: a
    /// builtin's methods are `Kernel.ns`'s, found in its mixin's method table
    /// like any other class's, and the only primitive entries are the VM
    /// mirror's.
    pub(in crate::heap) builtin: Option<BuiltinClass>,
    /// A metaclass's class: the class object whose class side this metaclass
    /// applies. `None` for every class object that is not a metaclass. The
    /// other direction needs no field: a class object's metaclass is its
    /// header class (`Heap::class_metaclass`).
    pub(in crate::heap) this_class: Option<ClassReference<'gc>>,
    /// A copy of `application.dispatch_ids.instances`. A cache's guard does
    /// not read it (it reads the guard in `Object::header`, through
    /// `Heap::dispatch_id`); this is where allocating an instance of this
    /// class, `Heap::class_dispatch_ids` and restamping
    /// (`Heap::header_dispatch_id`) read the instances id from. Kept equal to
    /// the application's by `ClassData::new` and
    /// `Heap::class_install_application`, the only ways either changes. A
    /// class object's own id is not this one but its metaclass's, read off
    /// its header like any object's.
    pub(in crate::heap) dispatch_id: DispatchId,
    /// For a level a mixin application added (`newspeak-spec.pdf` §6.2), the
    /// `<:` clause that added it -- which of the applying declaration's
    /// clauses, and so the factory message that declaration wrote for it and
    /// that message's compiled arguments (`AppliedClause::clause`). `None` for
    /// every other class object.
    ///
    /// It cannot live on the level's mixin, where an ordinary level's
    /// superclass-clause message lives: the level's mixin is the *applied*
    /// class's, one mixin shared by every declaration that applies it, and
    /// each of them writes its own message. Nor on the level's application,
    /// which the application cache shares between two declarations applying
    /// the same mixin to the same superclass with different messages.
    ///
    /// Boxed, so that it adds one word to every class object rather than
    /// three: the instantiation walk and the collector read this struct for
    /// every class object, applied level or not.
    pub(in crate::heap) applied_clause: Option<Box<crate::mixin::AppliedClause>>,
    /// `ValueClassRefusal`'s own doc comment says where this is decided and
    /// where it is refused. `None` for every class that is no value and for
    /// a value class whose enclosing objects are all values -- which is
    /// most of them, so `decide_instantiation` pays this read only behind
    /// `application.inherits_value`. `ClassData::new` defaults it to `None`;
    /// `interpreter::nested::after_clause` sets it to `Some` right after
    /// creating the class object it is about, for a class built from
    /// source, and `image::decode::rebuild_applications` does the same for
    /// a decoded one, recomputed rather than carried over the wire.
    pub(in crate::heap) value_class_refusal: Option<ValueClassRefusal>,
}

impl<'gc> ClassData<'gc> {
    /// A class object's data around `application`. Its metaclass is not
    /// part of it: `Mutator::attach_metaclass` builds one and makes it the
    /// class object's header class.
    pub(in crate::heap) fn new(
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
        builtin: Option<BuiltinClass>,
    ) -> ClassData<'gc> {
        ClassData {
            dispatch_id: application.dispatch_ids.instances,
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin,
            this_class: None,
            applied_clause: None,
            value_class_refusal: None,
        }
    }
}

impl Trace for Object<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Object {
            class,
            header: _, // not a handle
            data,
        } = self;
        tracer.object(class);
        data.trace(tracer);
    }
}

impl Trace for ObjectData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            ObjectData::Words { format, words, .. } => match format {
                Format::FixedPointers | Format::IndexablePointers => {
                    trace_word_bits(words, tracer);
                }
                // A married frame's registers are its stack entry's, traced
                // with the stack; its own stay nil until the divorce copies
                // them in, so they are skipped rather than walked for nothing.
                Format::Frame => {
                    let married = words
                        .get(frame_field::MARRIED)
                        .is_some_and(|bits| *bits != Word::NIL.bits());
                    let traced = if married {
                        frame_field::COUNT.min(words.len())
                    } else {
                        words.len()
                    };
                    trace_word_bits(&mut words[..traced], tracer);
                }
                // A closure's code id and feedback base are numbers; its
                // captured frame and its copied values are the pointers.
                Format::Closure => {
                    let fixed = closure_field::COUNT.min(words.len());
                    let (head, copied) = words.split_at_mut(fixed);
                    if let Some(frame) =
                        head.get_mut(closure_field::CAPTURED_FRAME..=closure_field::CAPTURED_FRAME)
                    {
                        trace_word_bits(frame, tracer);
                    }
                    trace_word_bits(copied, tracer);
                }
                // Only a pointer format's words are stored `Word`s; a
                // number's digits would read as handles.
                _ => {}
            },
            ObjectData::Class(class) => class.trace(tracer),
        }
    }
}

/// Traces pointer fields kept as the bits of a stored `Word`.
fn trace_word_bits<T: Tracer>(words: &mut [u64], tracer: &mut T) {
    for bits in words {
        let mut word = Word::from_bits(*bits);
        tracer.word(&mut word);
        *bits = word.bits();
    }
}

impl Trace for ClassData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        // `application` holds no handle: a mixin application is behaviour,
        // shared by every class object built from it.
        let ClassData {
            application: _,
            enclosing,
            declaring_class,
            superclass,
            builtin: _,
            this_class,
            dispatch_id: _, // not a handle
            // A clause and its compiled message: code, which holds no handle.
            applied_clause: _,
            value_class_refusal: _, // a Symbol and a String, neither a handle
        } = self;
        trace_optional_object(enclosing, tracer);
        trace_optional_object(declaring_class, tracer);
        trace_optional_object(superclass, tracer);
        trace_optional_object(this_class, tracer);
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
