//! What sits behind an `ObjectReference` and a `FrameReference`: the Rust
//! payload of every object and of every reified activation. Private to the
//! heap module -- nothing outside `heap.rs` and its submodules names these
//! types, so that changing how an object or a frame is represented changes
//! this module rather than its callers (`docs/design/memory.md`, "The
//! seam"). Callers build objects through `Heap`'s typed allocations and the
//! value types of `records.rs`, and read them through its accessors.

use std::rc::Rc;

use crate::heap::header::{Format, Header};
use crate::heap::{ClassReference, FrameReference, ObjectReference, Word};
use crate::mixin::DispatchId;
use crate::trace::{trace_optional_object, trace_words, Trace, Tracer};
use crate::types::{BuiltinClass, FrameKind, ValueClassRefusal};
use crate::Symbol;

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
    Instance(Vec<Word>),
    String(crate::CodePoints),
    /// The interned id and the same text as code points, so that a Symbol,
    /// a String, answers every String accessor.
    Symbol(Symbol, crate::CodePoints),
    Array(Vec<Word>),
    Closure(ClosureData<'gc>),
    /// Boxed: a class object's data is what would otherwise set every
    /// object's size, and class objects are few (`docs/design/dispatch.md`,
    /// "The dispatch id").
    Class(Box<ClassData<'gc>>),
    /// An instance of the kernel-private `Activation` class: a reified
    /// activation, reachable only from `Kernel.ns`, which the exception
    /// machinery walks and unwinds to. It stores the `FrameReference`, never
    /// an `ActivationReference`, since only a reified frame outlives its pop.
    Activation(FrameReference<'gc>),
    /// An instance of `LargeInteger`: §5.1.1's integer that does not fit a
    /// `SmallInteger`'s 61 bits. It never holds a value that does --
    /// `Mutator::new_integer` is the one way to make one and decides that --
    /// so each integer has one representation. Holds no handle.
    LargeInteger(crate::ArbitraryInteger),
    /// An instance of `Float` whose value lies outside the immediate range:
    /// NaN, the infinities, denormals and magnitudes outside about 2^-127 to
    /// 2^129. `Mutator::new_float` is the one way to make one and decides
    /// that, so each float has one representation. Holds no handle.
    Float(f64),
}

/// The units of a string's element width one word holds.
const BYTES_PER_WORD: usize = 8;
const THIRTY_TWO_BIT_UNITS_PER_WORD: usize = 2;
/// A class object's words after its class word, in the layout road step 3
/// gives it.
const CLASS_WORDS: usize = 8;
/// A closure's words before its copied values: its code, its captured
/// frame and its feedback base.
const CLOSURE_FIXED_WORDS: usize = 3;

impl ObjectData<'_> {
    /// The header an object holding this payload starts with: its format,
    /// its size as the word layout of road step 3 stores it -- the class
    /// word plus the payload's words -- and a `LargeInteger`'s sign.
    fn header(&self, guard: DispatchId) -> Header {
        let (format, unused, payload_words) = match self {
            ObjectData::Instance(fields) => (Format::FixedPointers, 0, fields.len()),
            ObjectData::Array(elements) => (Format::IndexablePointers, 0, elements.len()),
            ObjectData::String(text) | ObjectData::Symbol(_, text) => {
                let (format, per_word) = if text.is_wide() {
                    (Format::ThirtyTwoBit, THIRTY_TWO_BIT_UNITS_PER_WORD)
                } else {
                    (Format::Bytes, BYTES_PER_WORD)
                };
                let units = text.size();
                let words = units.div_ceil(per_word);
                // Below `per_word`, which is at most 8.
                let unused = u8::try_from(words * per_word - units).unwrap_or(0);
                (format, unused, words)
            }
            ObjectData::Closure(closure) => (
                Format::Closure,
                0,
                CLOSURE_FIXED_WORDS + closure.copied.len(),
            ),
            ObjectData::Class(_) => (Format::Class, 0, CLASS_WORDS),
            // A frame object from the frame's own representation on; one
            // word, the frame's handle, until then.
            ObjectData::Activation(_) => (Format::FixedPointers, 0, 1),
            ObjectData::LargeInteger(integer) => {
                let header = Header::new(
                    guard,
                    Format::SixtyFourBit,
                    0,
                    1 + integer.magnitude().len(),
                );
                return header.with_negative(integer.is_negative());
            }
            ObjectData::Float(_) => (Format::SixtyFourBit, 0, 1),
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

/// A block closure. Its compiled body is an `Rc` it shares with the
/// `MakeClosure` instruction that made it -- the same for every closure one
/// instruction makes -- and the frame it captured is its own. Stored inline
/// in the closure object, never behind an `Rc`
/// of its own: a handle lives only where the tracer can rewrite it in place
/// (`docs/design/memory.md`, "What the collector traces"), so everything
/// else that needs a closure -- an activation's `FrameKind`, a native
/// loop's state, a decided `Step` -- names the closure object instead.
///
/// 32 bytes in a release build, which is what keeps an `Object` at 56 --
/// this is `ObjectData`'s largest payload, and `heap.rs`'s
/// `an_object_is_the_size_the_amplification_figure_assumes` pins the
/// figure `CLAUDE.md`'s amplification paragraph is computed from.
#[derive(Debug, Clone)]
pub(in crate::heap) struct ClosureData<'gc> {
    pub(in crate::heap) code: Rc<crate::bytecode::CompiledCode>,
    /// The activation the closure was created in, reified. A closure keeps
    /// it whatever it captured: `enclosingActivation` and the debugger's
    /// "[] in Foo bar" are read off it (`docs/design/execution.md`, "The
    /// closure keeps its defining activation").
    pub(in crate::heap) captured_frame: FrameReference<'gc>,
    /// What the closure was handed when it was created: a copied variable's
    /// value, or an enclosing body's indirection vector. Its registers are
    /// `code.argument_count + position`. `Box<[_]>`, not a `Vec`, because
    /// `ClosureData` is `ObjectData`'s largest payload and an `Object` is
    /// pinned at 56 bytes in a release build (`heap.rs`).
    pub(in crate::heap) copied: Box<[Word]>,
    /// The start of its family's range in the heap's cache table, copied from
    /// the activation that created it, since a block belongs to its method's
    /// family. Never serialized: a decoded closure carries `NOT_ASSIGNED`
    /// and is given a fresh range when it is first activated.
    pub(in crate::heap) feedback_base: crate::interpreter::FeedbackBase,
}

/// A *reified* activation: one a block captured, or one the image decoder
/// rebuilt. A running activation lives on the `Heap`'s activation stack and
/// becomes a `Frame` only when `Heap::reify` is asked for one; while it runs,
/// the frame is married to its stack entry and its registers live there.
/// See `docs/design/execution.md`, "Activations".
#[derive(Debug, Clone)]
pub(in crate::heap) struct Frame<'gc> {
    pub(in crate::heap) receiver: Word,
    pub(in crate::heap) arguments: Vec<Word>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint).
    pub(in crate::heap) temporaries: Vec<Word>,
    /// How far this activation has progressed: an instruction index into
    /// the `CompiledCode` it runs.
    pub(in crate::heap) program_counter: usize,
    /// `None` for every frame the runtime reifies: a live activation's
    /// sender is the stack entry below it, and a returned activation has
    /// none. Kept for the image format, which records one per frame.
    pub(in crate::heap) sender: Option<FrameReference<'gc>>,
    /// Set only for `FrameKind::Closure` -- the enclosing method or
    /// initializer activation.
    pub(in crate::heap) home: Option<FrameReference<'gc>>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6: a
    /// top-level activation's current class is `nil`).
    pub(in crate::heap) current_class: Option<ClassReference<'gc>>,
    pub(in crate::heap) kind: FrameKind<'gc>,
    /// Set once this frame's activation has returned (`Heap::pop_activation`
    /// divorces it). A `^` whose home is such a frame is a `cannotReturn`
    /// error (`newspeak-spec.pdf` §3.6, §7.2). `raise_return` decides by
    /// `Heap::frame_activation`, which also covers a decoded frame, dead by
    /// construction whatever this flag says; the flag itself is what an image
    /// records.
    pub(in crate::heap) uncontinuable: bool,
}
/// A reified activation and, while that activation is still running, the
/// stack entry it is married to. While `live` is set, the frame's own
/// `arguments`/`temporaries` are empty and its registers are the entry's.
pub(in crate::heap) struct FrameSlot<'gc> {
    pub(in crate::heap) frame: Frame<'gc>,
    pub(in crate::heap) live: Option<u32>,
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
            ObjectData::Instance(words) | ObjectData::Array(words) => trace_words(words, tracer),
            ObjectData::String(_)
            | ObjectData::Symbol(..)
            | ObjectData::LargeInteger(_)
            | ObjectData::Float(_) => {}
            ObjectData::Closure(closure) => closure.trace(tracer),
            ObjectData::Class(class) => class.trace(tracer),
            ObjectData::Activation(frame) => tracer.frame(frame),
        }
    }
}

impl Trace for ClosureData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let ClosureData {
            code: _,
            captured_frame,
            copied,
            feedback_base: _, // not a handle
        } = self;
        tracer.frame(captured_frame);
        trace_words(copied, tracer);
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

impl Trace for Frame<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Frame {
            receiver,
            arguments,
            temporaries,
            program_counter: _,
            sender,
            home,
            current_class,
            kind,
            uncontinuable: _,
        } = self;
        tracer.word(receiver);
        trace_words(arguments, tracer);
        trace_words(temporaries, tracer);
        if let Some(sender) = sender {
            tracer.frame(sender);
        }
        if let Some(home) = home {
            tracer.frame(home);
        }
        trace_optional_object(current_class, tracer);
        kind.trace(tracer);
    }
}
