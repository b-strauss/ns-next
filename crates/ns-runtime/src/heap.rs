//! The heap seam. Every read and write of an `Object` or an activation goes
//! through `Heap`, so that replacing what sits behind a handle changes this
//! file rather than its call sites. Objects -- a reified frame among them,
//! an object of the frame format -- live end to end in one object space
//! (`space.rs`), named by typed handles that hold their byte offsets
//! (`ObjectReference`, `FrameReference`); see `docs/design/memory.md`. A
//! running activation lives on the activation stack, also owned here, as a
//! frame of tagged words in a reservation of its own (`frames.rs`), and is
//! named by an `ActivationReference` that nothing may store; a frame object
//! exists for it only once `reify` creates one. The accessor prefix says
//! which handle an accessor takes: `activation_*` a live stack frame,
//! `frame_*` a reified frame. See `docs/design/execution.md`,
//! "Activations".
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/locals.rs`.

use crate::builtins::primitive::ElementKind;
use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::interpreter::instantiate::InitializerBody;
use crate::interpreter::{Arguments, FeedbackBase};
use crate::mixin::{DispatchId, DispatchIds, Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::Symbol;
use std::num::NonZeroU64;
use std::ops::Range;
use std::rc::Rc;

mod application_ids;
mod compaction;
mod frames;
#[cfg(test)]
pub(crate) use frames::with_stack_reservation;
mod header;
mod ids;
#[cfg(ns_jit)]
mod machine_entries;
pub(crate) mod machine_words;
pub(crate) mod mark;
mod object;
mod pending;
pub(crate) mod reclaim;
mod records;
mod scavenge;
mod space;
mod tables;
mod value;
mod world;
use compaction::ForwardingTable;
#[cfg(debug_assertions)]
use compaction::{Shift, ShiftedRegion};
use frames::{frame_header, kind_tag, FrameFlags, FrameStack};
pub(crate) use header::IDENTITY_HASH_LIMIT;
use header::{Format, Header, MAX_AGE, WORD_COUNT_OVERFLOW};
pub(crate) use ids::{DispatchIdAllocator, FreeIndices};
use machine_words::{MachineWord, MachineWords};
pub use object::Element;
use object::{
    class_field, closure_field, frame_field, trace_block, PackedString, CLASS, FIELDS, HEADER,
};
pub(crate) use object::{ByteArrayData, ByteArrayDataMut};
use pending::PendingEphemerons;
pub use records::{
    DeclarationScope, FrameRecord, NewClass, NewClosure, ObjectShape, PlaceholderFormat,
};
use scavenge::Scavenger;
use space::{
    allocation_words, block_range, object_at, overflow_word, RegionName, Space, NURSERY_END,
    REGION_SLACK,
};
pub use tables::{ApplicationIndex, CodeId, MixinId};
pub(crate) use tables::{CodeEntry, CodeIdCache, MixinRow, Tables};
pub(crate) use value::is_small_float_bits;
#[cfg(ns_jit)]
pub(crate) use value::machine as value_bits;
use value::{at_rest, branded};
pub use value::{
    fits_small_integer, Immediate, ImmediateKind, Value, ValueKind, Word, SMALL_INTEGER_MAX,
    SMALL_INTEGER_MIN,
};
pub(crate) use world::{forward_identity, World};

/// The header an ordinary instance of an application is born with --
/// its guard, `FixedPointers`, its word count -- computed once when the
/// application is registered, so that `new` writes it without looking
/// anything up. Only for an instance whose header holds its count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceHeader(u64);

impl InstanceHeader {
    pub(crate) fn for_slots(guard: DispatchId, slot_count: usize) -> Option<InstanceHeader> {
        let word_count = slot_count.saturating_add(1);
        (word_count < WORD_COUNT_OVERFLOW)
            .then(|| InstanceHeader(Header::new(guard, Format::FixedPointers, 0, word_count).0))
    }
}

/// The sizes `image::decode::decode_graph_section`'s first pass gives each
/// record before it allocates anything, stated beside the payloads they
/// must agree with: a class object's fields, a closure's and a frame's
/// fixed fields, a reified mixin's one field, a string's field words, and
/// the words a block of so many fields occupies in all, its header and
/// class word included.
pub(crate) mod record_size {
    pub(crate) const CLASS_FIELDS: usize = super::class_field::COUNT;
    /// A level a `<:` clause added: a class object's fields and the class
    /// its clause named (`Heap::allocate_applied_level`).
    pub(crate) const APPLIED_LEVEL_FIELDS: usize = super::class_field::APPLIED_LEVEL_COUNT;
    /// A body-less declaration's class object: a level's fields and its
    /// declaration's enclosing object and declaring class
    /// (`Heap::class_declaration_scope`).
    pub(crate) const BODY_LESS_CLASS_FIELDS: usize = super::class_field::BODY_LESS_CLASS_COUNT;
    /// The id of the mixin it stands for (`Heap::allocate_mixin_object`).
    pub(crate) const MIXIN_FIELDS: usize = 1;
    pub(crate) const CLOSURE_FIXED_FIELDS: usize = super::closure_field::COUNT;
    pub(crate) const FRAME_FIXED_FIELDS: usize = super::frame_field::COUNT;

    pub(crate) fn string_fields(text: &str) -> usize {
        super::object::string_field_count(text)
    }

    /// The fields of a `ByteArray` of `length` bytes: its alignment
    /// reserve and its data words.
    pub(crate) fn byte_array_fields(length: usize) -> usize {
        super::object::byte_array_field_count(length)
    }

    /// Its overflow word included, when it needs one.
    pub(crate) fn block_words(field_count: usize) -> usize {
        super::space::allocation_words(field_count.saturating_add(super::FIELDS - 1))
    }
}

/// Hands every `Heap` a distinct id, so a handle can name the heap that
/// allocated it. Debug builds only -- the id and the field on each handle
/// both compile out of a release build, where a handle is exactly its
/// 64-bit offset.
#[cfg(debug_assertions)]
fn next_heap_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Hands every heap's code table a distinct epoch, and every full
/// collection that frees a code row a fresh one (`CodeIdCache`).
fn next_code_table_epoch() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The brand every movable handle carries (`docs/design/memory.md`, "The
/// brand"). Invariant in `'gc` -- a `fn(&'gc ()) -> &'gc ()` is
/// contravariant in its argument and covariant in its result -- so the
/// compiler can neither shorten nor extend it, and a handle minted in one
/// entry has a type no other entry can name. Zero-sized: a release handle
/// is still exactly its 64-bit offset.
type Brand<'gc> = std::marker::PhantomData<fn(&'gc ()) -> &'gc ()>;

/// An opaque handle to an object: the byte offset of the object's header
/// from the base of the heap's object space (`space.rs`). A multiple of 8,
/// so that the value word holding it carries the object tag, and never 0,
/// so that an optional handle is one word too. The inner offset is private to this module, and
/// only `heap.rs` reaches it -- that
/// privacy is what made replacing `Rc<RefCell<Object>>` with this a change
/// to one file rather than to every caller. See `docs/design/memory.md`,
/// "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it.
/// Compaction does reuse an offset once nothing reaches the object that
/// lay there, but a collection rewrites every handle it can find into the
/// new arrangement, and the debug-build epoch below turns a handle still
/// naming the old one into a panic rather than a silent hit on whatever
/// object now lies there -- so two live objects never share an offset.
#[derive(Clone, Copy)]
pub struct ObjectReference<'gc> {
    offset: NonZeroU64,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_object_origin`.
    #[cfg(debug_assertions)]
    origin: u32,
    /// The heap's move epoch when this handle was minted or last forwarded.
    /// Debug builds only: a move rewrites every handle the tracer knows
    /// about into the new epoch, so one still carrying an old epoch is one
    /// the tracer missed, and reading it panics instead of answering for
    /// whatever object now lies at its offset (`docs/design/memory.md`,
    /// "The move stress mode").
    #[cfg(debug_assertions)]
    epoch: u32,
    brand: Brand<'gc>,
}

impl<'gc> ObjectReference<'gc> {
    /// Object identity. Replaces `Rc::ptr_eq` at every call site, so that
    /// identity has one spelling that survives the store changing under it.
    ///
    /// Guarded like an accessor, and for a sharper reason. Comparing two
    /// handles from different heaps needs no `Heap` to go wrong, so nothing
    /// else would catch it -- and the wrong answer here is a silent *yes*:
    /// two unrelated objects that happen to share an offset would be reported
    /// identical. `check_object_origin` covers every read; this covers the
    /// one operation that reads nothing.
    pub fn same(self, other: ObjectReference<'gc>) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
        );
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.epoch, other.epoch,
            "stale handle: comparing handles minted in move epochs {} and {}",
            self.epoch, other.epoch
        );
        self.offset == other.offset
    }
}

/// Prints the byte offset and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` recursing through the bootstrap `Object`
/// class, which is its own `.class`; an offset has nothing to descend into,
/// so the recursion hazard is gone by construction. It still renders Rust
/// internals, which is why `CLAUDE.md`'s "Never `{:?}` a `Value`" rule
/// stands: `Heap::describe` is what a Newspeak-facing message needs.
impl std::fmt::Debug for ObjectReference<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectReference(@{})", self.offset)
    }
}

/// A class is an `ObjectReference` to an object of format `Class`. Not
/// enforced by the type system: `ClassReference` is an alias, not a distinct
/// handle type.
pub type ClassReference<'gc> = ObjectReference<'gc>;

/// An opaque handle to a *reified* activation: the byte offset of its frame
/// object's header (`Heap::frame_object`), kept a type of its own so that
/// only a frame object is ever read as a frame. The inner
/// offset is private to this module. See `docs/design/memory.md`, "The seam".
///
/// A running activation is named by an `ActivationReference` instead; a
/// `FrameReference` exists for one only once something must hold it past the
/// current call (`Heap::reify`). This is the only activation handle anything
/// may store -- in a closure, in another frame, in an image.
///
/// A handle is meaningful only against the `Heap` that allocated it.
/// Compaction does reuse an offset once nothing reaches the frame that lay
/// there, but a collection rewrites every handle it can find into the new
/// arrangement, and the debug-build epoch below turns a handle still naming
/// the old one into a panic rather than a silent hit on whatever object now
/// lies there -- so two frames never share an offset.
#[derive(Clone, Copy)]
pub struct FrameReference<'gc> {
    offset: NonZeroU64,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_frame_origin`.
    #[cfg(debug_assertions)]
    origin: u32,
    /// The heap's move epoch when this handle was minted or last forwarded.
    /// Debug builds only: a move rewrites every handle the tracer knows
    /// about into the new epoch, so one still carrying an old epoch is one
    /// the tracer missed, and reading it panics instead of answering for
    /// whatever object now lies at its offset (`docs/design/memory.md`,
    /// "The move stress mode").
    #[cfg(debug_assertions)]
    epoch: u32,
    brand: Brand<'gc>,
}

impl<'gc> FrameReference<'gc> {
    /// Frame identity. Guarded like an accessor: see `ObjectReference::same`.
    /// The unwinder finds its target through the frame's marriage
    /// (`frame_activation`) rather than by comparing frames, so nothing in
    /// the runtime compares them: the callers are the image round-trip tests
    /// (`tests/image.rs`) and this module's unit tests.
    pub fn same(self, other: FrameReference<'gc>) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
        );
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.epoch, other.epoch,
            "stale handle: comparing handles minted in move epochs {} and {}",
            self.epoch, other.epoch
        );
        self.offset == other.offset
    }
}

/// Prints the byte offset and nothing else. A derived `Debug` over a
/// frame's contents would walk the sender chain, which has no acyclic
/// guarantee to lean on (a closure's `home` can itself be captured); an
/// offset has nothing to descend into at all, so the hazard is gone by
/// construction rather than by discipline, and printing one cannot panic.
impl std::fmt::Debug for FrameReference<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameReference(@{})", self.offset)
    }
}

/// An opaque handle to a *live* activation: the word offset of its frame on
/// the `Heap`'s activation stack. Where a `FrameReference` names a reified
/// activation that may be stored anywhere, an `ActivationReference` is valid
/// only until its frame is popped -- the next activation at that depth
/// reuses the offset. Offsets grow with depth, so comparing two orders
/// them as their depths do. That is why
/// nothing stores one: a closure object, a frame object and every image record take a
/// `FrameReference`, which `Heap::reify` answers for a live activation. See
/// `docs/design/execution.md`, "Activations", "Representation".
///
/// 4 bytes in a release build. A debug build adds the heap's id, like every
/// handle, and a `generation` drawn at each push and recorded in the
/// frame's header, so that reading through a handle whose frame was popped
/// panics instead of answering for whichever activation reuses the offset.
#[derive(Clone, Copy)]
pub struct ActivationReference {
    offset: u32,
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    generation: u32,
}

impl ActivationReference {
    /// Activation identity: the unwinder stops at its target by it
    /// (`interpreter::unwind`). Two handles at the same offset are the same
    /// activation only while neither has been popped, which is why the
    /// unwinder compares against a handle it has just asked the target's
    /// frame for (`Heap::frame_activation`) and never keeps one across the
    /// blocks it runs. A debug build checks that rather than assuming it.
    pub fn same(self, other: ActivationReference) -> bool {
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.origin, other.origin,
                "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
                self.origin, other.origin
            );
            debug_assert!(
                self.offset != other.offset || self.generation == other.generation,
                "stale activation handle: compared against a later activation at the same depth"
            );
        }
        self.offset == other.offset
    }
}

/// Prints the frame's stack offset and nothing else, for the reason
/// `FrameReference`'s does.
impl std::fmt::Debug for ActivationReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActivationReference(#{})", self.offset)
    }
}

/// The dispatch ids a closure's and a frame object's headers carry: the
/// instance ids of the `Closure` and `Activation` classes, which every
/// closure and every frame is an instance of. Kept on the heap so that
/// their allocation writes a header it never has to look up
/// (`Heap::set_builtin_guards`).
#[derive(Clone, Copy)]
pub(crate) struct BuiltinGuards {
    pub(crate) closure: DispatchId,
    pub(crate) frame: DispatchId,
}

impl BuiltinGuards {
    /// The guards before the bootstrap records the builtin classes' ids,
    /// and a bare heap's.
    const NONE: Self = Self {
        closure: DispatchId::NONE,
        frame: DispatchId::NONE,
    };
}

/// The index of `Float`'s instance id in `Heap::immediate_dispatch_ids`,
/// `Value::immediate_index`'s for a `SmallFloat`: a boxed float carries the
/// guard an immediate one dispatches on, both instances of `Float`.
const FLOAT_DISPATCH_INDEX: usize = 4;

/// What `MachineWord::SafepointRequested` reads while only a stress mode
/// holds the request (`Heap::set_safepoint_requested`); a real request
/// writes 1 over it.
#[cfg(debug_assertions)]
pub(crate) const STRESS_HOLD: u64 = 2;

/// `Value::immediate_index`'s for a `SmallInteger`, which machine code's
/// guard and class idioms compute as it does.
#[cfg(ns_jit)]
const SMALL_INTEGER_DISPATCH_INDEX: usize = 3;

/// Where a running activation's registers start in the activation stack's
/// words, at its first argument, `argument_count` words below its frame's
/// header, whose words the register numbering skips (`bytecode::
/// HEADER_REGISTERS`): Cog's `localFP`, held by the run loop so that a
/// register access is one index (`docs/design/execution.md`, "The loop
/// over locals"). It is an index, not a pointer, so it survives
/// the stack committing more and a move, which touches no register window;
/// it carries the danger an
/// `ActivationReference` does -- once its activation is popped, the next one
/// at that depth reuses the window -- and a debug build checks every access
/// against its activation's generation, as `check_activation` does.
#[derive(Clone, Copy)]
pub struct RegisterBase {
    index: u32,
    #[cfg(debug_assertions)]
    activation: ActivationReference,
}

/// A send's argument window: `count` consecutive registers from `start` in
/// the window a `RegisterBase` names (`interpreter::Arguments::Slots`).
///
/// The three numbers are packed into one word by arithmetic, so that the
/// window is built in a register and travels in one: built in memory by
/// narrow stores and read back wider, as three fields of their own were,
/// each read waited for the stores to retire (`docs/design/execution.md`,
/// "Overlapping register windows"). A debug build carries the
/// register base's activation beside it, checked on every read as any
/// register access is.
#[derive(Clone, Copy)]
pub struct ArgumentSlots {
    /// The register base's index in bits 0-31, `start` in bits 32-47,
    /// `count` in bits 48-63.
    packed: u64,
    #[cfg(debug_assertions)]
    activation: ActivationReference,
}

/// `number`, a count of registers or a register's offset in a window, in
/// the `u16` registers are numbered in. Every body's registers lie below
/// the register ceiling, `u16::MAX` -- a body past it is compiled to one
/// that raises (`Literal::BodyTooLarge`) -- and a list of values longer
/// than the ceiling is refused before it is pushed
/// (`interpreter::Refusal::PastTheRegisterCeiling`). A helper rather than
/// `u16::try_from`, whose check the push and the send would pay on every
/// activation.
#[inline(always)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "every register number lies below the register ceiling, u16::MAX, which the compiler and the push keep"
)]
pub(crate) fn register_number(number: usize) -> u16 {
    debug_assert!(
        number <= usize::from(u16::MAX),
        "internal: a register number lies within the register ceiling, u16::MAX"
    );
    number as u16
}

impl ArgumentSlots {
    /// The `count` registers from `start` in `register_base`'s window.
    #[inline(always)]
    pub(crate) fn new(register_base: RegisterBase, start: u16, count: u16) -> ArgumentSlots {
        ArgumentSlots {
            packed: u64::from(register_base.index)
                | (u64::from(start) << 32)
                | (u64::from(count) << 48),
            #[cfg(debug_assertions)]
            activation: register_base.activation,
        }
    }

    /// The window the registers lie in.
    #[inline(always)]
    pub(crate) fn register_base(self) -> RegisterBase {
        RegisterBase {
            index: self.base_index(),
            #[cfg(debug_assertions)]
            activation: self.activation,
        }
    }

    /// The register base's index, bits 0-31 of the packed word.
    #[inline(always)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the index is the packed word's low 32 bits, which the truncation keeps"
    )]
    fn base_index(self) -> u32 {
        self.packed as u32
    }

    /// The first register.
    #[inline(always)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`start` is bits 32-47 of the packed word, which the truncation keeps"
    )]
    pub(crate) fn start(self) -> u16 {
        (self.packed >> 32) as u16
    }

    /// How many registers.
    #[inline(always)]
    pub(crate) fn count(self) -> u16 {
        (self.packed >> 48) as u16
    }

    /// The index of the first register's word on the activation stack.
    #[inline(always)]
    fn first_word(self) -> usize {
        self.base_index() as usize + usize::from(self.start())
    }
}

/// Everything a new activation is apart from its registers, which
/// `Heap::push_activation` takes separately.
pub struct ActivationHeader<'gc> {
    pub receiver: Value<'gc>,
    /// `None` only for `ActivationKind::TopLevel` (`newspeak-spec.pdf` §3.6).
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: ActivationKind<'gc>,
    /// The method an `ActivationKind::Method` activation runs, `None` for
    /// every other kind. The push registers the body's code row with it,
    /// which is where the activation's frame finds its method again
    /// (`Heap::code_method`); the kind keeps only the selector.
    pub method: Option<Rc<MixinMethod>>,
    /// Where the activation's code family starts in the heap's cache table
    /// (`interpreter::inline_cache::InlineCaches`), found or assigned by
    /// `Mutator::push_step` before the header is built.
    pub feedback_base: FeedbackBase,
    /// The body the activation runs. The run loop reads it back to resume a
    /// caller once the activation above it returns.
    pub code: std::rc::Rc<crate::bytecode::CompiledCode>,
    /// What the run loop does with the activation's value when it returns.
    pub then: Continuation<'gc>,
    /// Whether the activation counts against the depth limit -- every one
    /// does, except a control-flow step's block activated for a
    /// `SendInlinable` (`Mutator::push_step`).
    pub counted: bool,
}

/// A push the activation stack's reservation cannot hold
/// (`Heap::push_activation`): nothing was pushed, and the header comes
/// back, so that the caller can raise the depth limit's error for the
/// activation it names and undo what its continuation had begun.
pub struct StackExhausted<'gc> {
    pub header: Box<ActivationHeader<'gc>>,
}

impl<'gc> StackExhausted<'gc> {
    /// The refusal of a push of `header`: out of line, so that the push's
    /// common path keeps no boxed header.
    #[cold]
    #[inline(never)]
    fn of(header: ActivationHeader<'gc>) -> StackExhausted<'gc> {
        StackExhausted {
            header: Box::new(header),
        }
    }
}

/// A push the activation stack's reservation cannot hold, refused by a push
/// that borrows its header (`Heap::push_activation_from`,
/// `Heap::push_overlapped_activation`): nothing was pushed, and the header
/// is still the caller's, untouched.
///
/// The header is borrowed rather than moved so that the run loop's pushes
/// never copy it whole: moved, it was copied into the push's own place,
/// sixteen bytes at a time, right after the narrow stores that built it --
/// a store-forwarding stall on every send.
#[derive(Debug)]
pub(crate) struct PushRefused;

/// Names the refusal and nothing of the header, which holds a value.
impl std::fmt::Debug for StackExhausted<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StackExhausted")
    }
}

/// Every word of a frame's header but its registers, as `push_activation`
/// and `resume_activation` write it (`frame_header` gives the layout):
/// built before the frame's words are borrowed, then written in one go.
struct FrameHeaderWords {
    receiver: Word,
    current_class: Word,
    operand: Word,
    reified: Word,
    flags: Word,
    code: Word,
    program_counter: Word,
    feedback_base: Word,
    caller: Word,
    continuation: Word,
    guard: Word,
    entered_level: Word,
    register_count: Word,
}

impl FrameHeaderWords {
    /// Writes every word into `header`, a frame's first
    /// `frame_header::COUNT` words.
    #[inline(always)]
    fn write(self, header: &mut [u64]) {
        let FrameHeaderWords {
            receiver,
            current_class,
            operand,
            reified,
            flags,
            code,
            program_counter,
            feedback_base,
            caller,
            continuation,
            guard,
            entered_level,
            register_count,
        } = self;
        let header = &mut header[..frame_header::COUNT];
        header[frame_header::RECEIVER] = receiver.bits();
        header[frame_header::CURRENT_CLASS] = current_class.bits();
        header[frame_header::OPERAND] = operand.bits();
        header[frame_header::REIFIED] = reified.bits();
        header[frame_header::FLAGS] = flags.bits();
        header[frame_header::CODE] = code.bits();
        header[frame_header::PROGRAM_COUNTER] = program_counter.bits();
        header[frame_header::FEEDBACK_BASE] = feedback_base.bits();
        header[frame_header::CALLER] = caller.bits();
        header[frame_header::CONTINUATION] = continuation.bits();
        header[frame_header::GUARD] = guard.bits();
        header[frame_header::ENTERED_LEVEL] = entered_level.bits();
        header[frame_header::REGISTER_COUNT] = register_count.bits();
    }
}

/// What popping an activation hands back to the run loop.
pub struct Popped<'gc> {
    /// The popped activation's continuation.
    pub then: Continuation<'gc>,
    /// Whether the popped activation counted against the depth limit: what
    /// its continuation pushes in its place may be exempt only if this one
    /// was (`Mutator::push_continuation`).
    pub counted: bool,
}

/// A value a host keeps across runs of the loop (`docs/design/memory.md`,
/// "Host roots"): a slot the heap's root table also holds, which a moving
/// collector traces and rewrites. Cloning a `Root` shares the slot, and
/// dropping the last clone releases it without touching the heap -- the
/// table notices at its next sweep that it holds the only reference left.
/// gc-arena's `DynamicRoot` is the model.
///
/// No `Debug`, for the reason `Signal` has none: printing one would print
/// a `Value`.
#[derive(Clone)]
pub struct Root {
    slot: std::rc::Rc<std::cell::Cell<Value<'static>>>,
}

/// The host roots. Swept of slots nothing else holds at every move, and
/// whenever it has doubled since the last sweep, so it stays proportional to
/// the roots a host still holds even in a build where nothing moves.
struct HostRoots {
    slots: Vec<std::rc::Rc<std::cell::Cell<Value<'static>>>>,
    sweep_at: usize,
}

/// The table's size below which `stash` never sweeps.
const MIN_ROOT_SWEEP: usize = 64;

impl HostRoots {
    /// Drops every slot the host has released -- one the table holds the
    /// only reference to -- and sets the size at which `stash` sweeps
    /// next: twice what survived, never below `MIN_ROOT_SWEEP`.
    fn sweep(&mut self) {
        self.slots
            .retain(|slot| std::rc::Rc::strong_count(slot) > 1);
        self.sweep_at = (self.slots.len() * 2).max(MIN_ROOT_SWEEP);
    }
}

/// Every live activation, innermost highest: one frame of tagged words
/// each in `frames`, its arguments, then a header of `frame_header::COUNT`
/// words -- receiver, current class, kind, code, continuation row, caller
/// and the rest (`docs/design/execution.md`, the table of header words) --
/// then the rest of its registers. A frame is named by the word offset of
/// its header. A send's own callee lies on the sender's argument slots,
/// which are its arguments, its header where they end; every other frame
/// starts at or past the window end of the one below. The reservation is
/// committed as the stack deepens and never decommitted, so once it has
/// grown to the program's depth a push commits nothing.
struct ActivationStack {
    frames: FrameStack,
    /// The innermost frame's offset, `NO_FRAME` while the stack is empty.
    innermost: u32,
    /// How many frames are on the stack: what `Heap::activation_depth`
    /// answers, since an offset is no count.
    live: usize,
    /// The machine words, whose `MachineWord::DepthCount` is how many of
    /// the live activations, and of the activations suspended into their
    /// frames, count against the depth limit. One running total, since the
    /// limit is checked on every push: suspending and resuming an
    /// activation leave it alone, as a suspended one counts all along.
    /// Machine code reads and writes the count in place, so it lives
    /// there and nowhere else.
    machine_words: MachineWords,
}

/// Hands every word of `words` to `tracer`: words of the activation
/// stack, each a tagged value, so that no stack map is needed
/// (`Heap::trace_stack`).
#[inline(always)]
fn trace_words<T: crate::trace::Tracer>(words: &mut [u64], tracer: &mut T) {
    for bits in words {
        let mut word = Word::from_bits(*bits);
        tracer.word(&mut word);
        *bits = word.bits();
    }
}

/// The bits of a debug build's generation counter: inside 31, so that a
/// generation reads back as the `u32` a handle holds whatever the counter
/// has wrapped through.
#[cfg(debug_assertions)]
const GENERATION_MASK: u32 = 0x7FFF_FFFF;

// The compiler leaves the header's words out of the register numbering by
// this many numbers, so the two must agree.
const _: () = assert!(crate::bytecode::HEADER_REGISTERS as usize == frame_header::COUNT);

// `jit::closures` fills every fixed field of the frame objects and the
// closures it makes, one store each, and nothing else: a field added to
// either must be filled there too, where these fail first.
const _: () = assert!(frame_field::COUNT == 10 && frame_field::MARRIED == frame_field::COUNT - 1);
const _: () =
    assert!(closure_field::COUNT == 3 && closure_field::FEEDBACK_BASE == closure_field::COUNT - 1);

/// The most slots a compiled call may shift a block's frame up by, which
/// its flags word records (`frames::CALL_SHIFT_BITS`).
#[cfg(ns_jit)]
pub(crate) const MAXIMUM_CALL_SHIFT: u64 =
    frames::CALL_SHIFT_BITS >> frames::CALL_SHIFT_BITS.trailing_zeros();

/// A shift count machine code takes, which is below 64.
#[cfg(ns_jit)]
fn small_count(count: u32) -> u8 {
    let Ok(count) = u8::try_from(count) else {
        unreachable!("internal: a shift count is below 64");
    };
    count
}

/// What machine code reads of an object and a value, in the form its
/// instructions take (`Heap::machine_layout`).
#[cfg(ns_jit)]
pub(crate) struct MachineLayout {
    /// The header word's displacement from an object's offset.
    pub(crate) header_displacement: i32,
    /// The header word's dispatch guard bits.
    pub(crate) guard_mask: i32,
    /// The class word's displacement from an object's offset.
    pub(crate) class_word_displacement: i32,
    /// A class object's superclass field's displacement from its offset.
    pub(crate) superclass_displacement: i32,
    /// A class object's enclosing-object field's displacement from its
    /// offset (`Heap::class_enclosing`).
    pub(crate) enclosing_displacement: i32,
    /// A class object's declaring-class field's displacement from its
    /// offset (`Heap::class_declaring`).
    pub(crate) declaring_class_displacement: i32,
    /// A class object's application field's displacement from its offset
    /// (`Heap::class_application_word`).
    pub(crate) application_displacement: i32,
    /// A word's tag bits, 0 for an object.
    pub(crate) tag_mask: i32,
    /// A `SmallInteger`'s tag bit.
    pub(crate) small_integer_tag: i32,
    /// A `SmallFloat`'s tag bit.
    pub(crate) small_float_tag: i32,
    /// A `SmallInteger`'s immediate index (`Value::immediate_index`).
    pub(crate) small_integer_index: u64,
    /// A `SmallFloat`'s immediate index.
    pub(crate) small_float_index: u64,
    /// How far a constant's payload lies above its tag: its index.
    pub(crate) tag_bits: u8,
    /// The header word's format bits, all clear exactly for an instance's
    /// format, whose fields `Heap::instance_field` reads.
    pub(crate) instance_format_bits: i32,
    /// How far the header word's count of the words after it -- the class
    /// word and the fields -- is shifted up; 255 there says the count is
    /// in an overflow word, and is past every field a stub reads.
    pub(crate) word_count_shift: u8,
    /// An object's field 0's displacement from its offset.
    pub(crate) fields_displacement: i32,
    /// The header word's format bits of an `Array`, under
    /// `instance_format_bits`, whose elements `Heap::array_element` reads.
    pub(crate) indexable_pointers_format: i32,
    /// The word count in the header that says the count is in the overflow
    /// word before it.
    pub(crate) word_count_overflow: i32,
    /// Where the old space begins: an object whose offset lies below it
    /// is young (`space::is_young`).
    pub(crate) nursery_end: i32,
    /// The header word's remembered bit, set while the object is in the
    /// remembered set, as the 32-bit pattern a test of the header's low
    /// half takes.
    pub(crate) remembered_bit: i32,
    /// `nil`'s word.
    pub(crate) nil_word: i32,
    /// The header word's format bits all of which a ByteArray's format
    /// sets (`Format::AlignedBytes`), whose bytes `Heap::byte_array_at`
    /// reads, and no other format sets all of.
    pub(crate) aligned_bytes_format: i32,
    /// How far a ByteArray's count of unused bytes in its last data word
    /// is shifted up in the header word.
    pub(crate) unused_bytes_shift: u8,
    /// A ByteArray's count of unused bytes, once shifted down.
    pub(crate) unused_bytes_mask: i32,
    /// The alignment in bytes of a ByteArray's data, whose offset is the
    /// first multiple of it at or after its field 0's
    /// (`object::aligned_data_start`).
    pub(crate) byte_data_alignment: i32,
    /// The bytes of the words after a ByteArray's header that hold no
    /// data: its class word and the reserve its alignment takes.
    pub(crate) byte_data_reserve_bytes: i32,
    /// How far an application index is shifted up to be its row's byte
    /// offset in the application id table (`heap/application_ids.rs`).
    pub(crate) application_row_shift: u8,
    /// A row's instances dispatch id's displacement from the row.
    pub(crate) application_dispatch_id_displacement: i32,
    /// A row's own slot base's displacement from the row.
    pub(crate) application_slot_base_displacement: i32,
    /// A row's flags' displacement from the row.
    pub(crate) application_flags_displacement: i32,
    /// The flag of a row whose instances' slots take the check of what a
    /// value's slot may hold.
    pub(crate) application_value_slots_flag: i32,
    /// A row's instance header word's displacement from the row.
    pub(crate) application_instance_header_displacement: i32,
    /// The header word's format bits of a class object, under
    /// `instance_format_bits`.
    pub(crate) class_format: i32,
    /// The header word's format bits of a closure, under
    /// `instance_format_bits`.
    pub(crate) closure_format: i32,
    /// The header word's format bits of a frame object, under
    /// `instance_format_bits`.
    pub(crate) frame_format: i32,
    /// A class object's builtin field's displacement from its offset
    /// (`Heap::class_builtin`): nil for a class whose instances are
    /// ordinary ones.
    pub(crate) builtin_displacement: i32,
    /// A class object's value class refusal's displacement from its
    /// offset (`Heap::class_has_value_refusal`): nil for a class whose
    /// instantiation nothing refuses there.
    pub(crate) value_class_refusal_displacement: i32,
    /// A frame object's fixed fields' displacements from its offset
    /// (`object.rs`'s `frame_field`), which `MakeClosure` fills in machine
    /// code as `Heap::reify` fills them (`jit::closures`).
    pub(crate) frame_sender_displacement: i32,
    pub(crate) frame_home_displacement: i32,
    pub(crate) frame_receiver_displacement: i32,
    pub(crate) frame_current_class_displacement: i32,
    pub(crate) frame_program_counter_displacement: i32,
    pub(crate) frame_code_id_displacement: i32,
    pub(crate) frame_kind_displacement: i32,
    pub(crate) frame_kind_operand_displacement: i32,
    pub(crate) frame_uncontinuable_displacement: i32,
    pub(crate) frame_married_displacement: i32,
    /// How many fixed fields a frame object has: its registers follow.
    pub(crate) frame_fixed_fields: usize,
    /// A closure's fixed fields' displacements from its offset
    /// (`object.rs`'s `closure_field`), as `allocate_closure_from_registers`
    /// writes them.
    pub(crate) closure_code_id_displacement: i32,
    pub(crate) closure_captured_frame_displacement: i32,
    pub(crate) closure_feedback_base_displacement: i32,
    /// How many fixed fields a closure has: its copied values follow.
    pub(crate) closure_fixed_fields: usize,
}

/// The kind words of the frame objects `MakeClosure` reifies in machine
/// code (`Heap::machine_frame_kinds`): each kind's wire tag as the
/// `SmallInteger` word `frame_fixed_words` writes, and, for an
/// initializer's frame, whose kind its code does not say, the tag its
/// flags word holds for each of its bodies beside the word that tag
/// stands for.
#[cfg(ns_jit)]
pub(crate) struct MachineFrameKinds {
    pub(crate) method: u64,
    pub(crate) closure: u64,
    pub(crate) factory: u64,
    pub(crate) superclass_clause: u64,
    /// Each initializer body's kind tag in a flags word, and its kind word.
    pub(crate) initializers: [(u8, u64); 3],
    /// How far a flags word is shifted to bring its payload, the kind tag
    /// lowest, down.
    pub(crate) kind_tag_shift: u8,
    /// The kind tag's bits in a flags word's payload.
    pub(crate) kind_tag_mask: i32,
    /// `false`'s word, which a running frame's `uncontinuable` holds.
    pub(crate) false_word: i32,
}

/// The block of an object machine code allocates (`Heap::machine_block`).
#[cfg(ns_jit)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MachineBlock {
    /// The block's bytes: the overflow word, the header and the words
    /// after it.
    pub(crate) bytes: i32,
    /// The word before the header of an object whose count the header
    /// cannot hold.
    pub(crate) overflow_word: Option<u64>,
    /// Whether the inline bump may place the block in eden: below the
    /// large-object threshold.
    pub(crate) inline: bool,
}

// `instance_format_bits` is a test for all-clear bits.
const _: () = assert!(Format::FixedPointers as u64 == 0);

/// Where a debug build's generation lies in a flags word, for machine
/// code (`Heap::machine_flags`).
#[cfg(all(ns_jit, debug_assertions))]
pub(crate) struct MachineGeneration {
    /// The generation counter's bits.
    pub(crate) mask: i32,
    /// How far the generation is shifted in the flags word's bits.
    pub(crate) shift: u8,
    /// The flags word's register number past the frame's arguments.
    pub(crate) flags_register: u16,
}

/// Where machine code reads a block body's row of the machine entry table
/// (`Heap::machine_entry_layout`).
#[cfg(ns_jit)]
pub(crate) struct MachineEntryLayout {
    /// How far a code id is shifted to be its row's offset.
    pub(crate) row_shift: u8,
    /// The body's machine entry, 0 for none.
    pub(crate) entry_displacement: i32,
    /// The feedback base its machine code was compiled against, as a
    /// closure's word holds it.
    pub(crate) feedback_base_displacement: i32,
    /// Its parameter count.
    pub(crate) parameters_displacement: i32,
}

/// Where machine code finds what the divorce of a reified frame at its
/// return reads and writes (`Heap::machine_divorce`): the header words by
/// their register number past the frame's arguments, and the frame
/// object's fields by their displacement from its offset.
#[cfg(ns_jit)]
pub(crate) struct MachineDivorce {
    /// The reified word: nil, or the frame object's word.
    pub(crate) reified_register: u16,
    pub(crate) flags_register: u16,
    pub(crate) program_counter_register: u16,
    /// `retains_registers`' bit in the flags word.
    pub(crate) retains_registers_bit: i32,
    pub(crate) program_counter_displacement: i32,
    pub(crate) sender_displacement: i32,
    pub(crate) uncontinuable_displacement: i32,
    pub(crate) married_displacement: i32,
    /// `true`'s word.
    pub(crate) true_word: i32,
}

/// A frame machine code pushed, described for its header
/// (`Heap::write_machine_frame`): where it lies, what it runs and where it
/// stands, and its caller.
#[cfg(ns_jit)]
pub(crate) struct MachineFrame {
    /// Its header's word offset.
    pub(crate) offset: u32,
    pub(crate) argument_count: u16,
    pub(crate) code: CodeId,
    /// Its kind and operand.
    pub(crate) kind: MachineKind,
    pub(crate) program_counter: u32,
    pub(crate) feedback_base: FeedbackBase,
    /// The registers above its header.
    pub(crate) register_count: usize,
    /// Whether its code addresses its own slots
    /// (`CompiledCode::addresses_own_slots`), so that it records its
    /// entered level.
    pub(crate) addresses_own_slots: bool,
    /// Whether its code reifies its frame (`jit::baseline::reifies_its_frame`):
    /// its prologue wrote the reified word, the flags word and the program
    /// counter, as the push writes them, and its code keeps all three
    /// since -- the reified word and the flags' `retains_registers` as its
    /// frame is reified and marked, the program counter at each point the
    /// interpreter stores it -- so that its return divorces a reified
    /// frame as the pop does.
    pub(crate) reifies: bool,
    /// The caller's offset and the top the frame was pushed at; `None`
    /// for a leaf's view of its own frame, whose caller nothing reads.
    pub(crate) caller: Option<(u32, usize)>,
}

/// The kind of a frame machine code pushed and its operand, as its header
/// records them (`Heap::write_machine_frame`): each either a constant of
/// its body -- a method's tag and selector, a factory's, a closure's and a
/// superclass clause's tag -- or what the frame itself holds, written by
/// its prologue -- an initializer's tag, which its code does not say,
/// since one code can be two of its mixin's bodies, and a closure's
/// operand, the closure the call handed over -- or the current class the
/// call handed over, a factory's and an initializer's operand.
#[cfg(ns_jit)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MachineKind {
    tag: MachineKindTag,
    operand: MachineOperand,
}

/// Where a machine frame's kind tag comes from (`MachineKind`).
#[cfg(ns_jit)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MachineKindTag {
    /// The tag itself.
    Constant(u8),
    /// The tag the frame's prologue wrote into its flags word.
    Recorded,
}

/// Where a machine frame's operand comes from (`MachineKind`).
#[cfg(ns_jit)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MachineOperand {
    /// A method's selector.
    Selector(Symbol),
    /// The frame's current class.
    CurrentClass,
    /// The operand word the frame's prologue wrote.
    Recorded,
}

#[cfg(ns_jit)]
impl MachineKind {
    /// A method's activation of the method `selector` names.
    pub(crate) fn method(selector: Symbol) -> MachineKind {
        MachineKind {
            tag: MachineKindTag::Constant(kind_tag::METHOD),
            operand: MachineOperand::Selector(selector),
        }
    }

    /// A factory's activation, on the class object the call handed over
    /// as its current class.
    pub(crate) fn factory() -> MachineKind {
        MachineKind {
            tag: MachineKindTag::Constant(kind_tag::FACTORY),
            operand: MachineOperand::CurrentClass,
        }
    }

    /// An initializer's activation, entered at the level the call handed
    /// over as its current class, running the body whose tag its prologue
    /// recorded from what the initializer leaf decided
    /// (`Heap::hand_machine_initializer`).
    pub(crate) fn initializer() -> MachineKind {
        MachineKind {
            tag: MachineKindTag::Recorded,
            operand: MachineOperand::CurrentClass,
        }
    }

    /// A closure's activation, on the closure its prologue recorded as
    /// the frame's operand: no return address says which closure a
    /// block's code runs for, since one block's code runs for every
    /// closure made of it.
    pub(crate) fn closure() -> MachineKind {
        MachineKind {
            tag: MachineKindTag::Constant(kind_tag::CLOSURE),
            operand: MachineOperand::Recorded,
        }
    }

    /// A superclass clause's activation, on the class its operand names,
    /// which the frame holds as a closure's holds its closure.
    pub(crate) fn superclass_clause() -> MachineKind {
        MachineKind {
            tag: MachineKindTag::Constant(kind_tag::SUPERCLASS_CLAUSE),
            operand: MachineOperand::Recorded,
        }
    }

    /// Whether it is a closure's activation, whose prologue writes the
    /// flags word's `counted` bit the call handed it.
    pub(crate) fn is_closure(self) -> bool {
        self.tag == MachineKindTag::Constant(kind_tag::CLOSURE)
    }
}

#[cfg(ns_jit)]
impl MachineFrame {
    /// Its first argument's word offset.
    pub(crate) fn start(&self) -> usize {
        self.offset as usize - usize::from(self.argument_count)
    }

    /// Where its registers end.
    pub(crate) fn window_end(&self) -> usize {
        self.offset as usize + frame_header::COUNT + self.register_count
    }
}

/// `ActivationStack::innermost` of an empty stack: no frame starts there,
/// since a frame's offset is below the reservation's 2^31 words.
const NO_FRAME: u32 = u32::MAX;

/// What the collector's pre-pass writes into every register no frame reads
/// before it writes it (`Heap::trace_stack`), and what a frame object
/// takes for such a register when an activation is suspended or divorced:
/// nil in a release build, so that nothing a callee or a popped frame left
/// there is traced again. A debug build writes `Word::DEAD_REGISTER`
/// there instead, and into every scratch register a push hands out and
/// every register a pop gives back above a callee
/// (`FrameStack::give_back_dead_registers`), and refuses an instruction's
/// read of it (`assert_live_register`): the dynamic net under the leftover
/// analysis, which says nothing reads such a register before it writes
/// it. The word is one no `Value` can be -- the tag no constructor makes
/// -- since a number such as the `SmallInteger` `0x5EAD_5EAD` a program
/// could compute as well, and a read of it would pass unseen; the
/// tracers, which take a word naming no object as a number, pass over it.
#[inline(always)]
fn dead_register() -> Word {
    #[cfg(debug_assertions)]
    return Word::DEAD_REGISTER;
    #[cfg(not(debug_assertions))]
    return Word::NIL;
}

/// Refuses, in a debug build, an instruction's read of a register no
/// instruction wrote: `bits`, register `index` of the frame at `offset`,
/// must not be `dead_register`'s word. Bulk copies of a frame's registers
/// -- a reification's, a suspension's, a divorce's, a trace -- are not
/// reads of this kind: they nil such words instead.
#[cfg(debug_assertions)]
#[inline(always)]
fn assert_live_register(offset: u32, index: u16, bits: u64) {
    if Word::from_bits(bits).is_dead_register() {
        refuse_dead_register_read(offset, index);
    }
}

/// The refusal of a read of a dead register: the interpreter's
/// (`assert_live_register`) and machine code's, whose read exits to
/// `Heap::refuse_dead_register`, so that both say the same.
#[cfg(any(debug_assertions, ns_jit))]
#[cold]
#[inline(never)]
fn refuse_dead_register_read(offset: u32, index: u16) -> ! {
    panic!(
        "internal: the frame at stack word {offset} reads r{index}, which nothing wrote since \
         its push or its callee's return (the leftover analysis says nothing reads it)"
    )
}

/// Refuses, in a debug build, a push that hands `code` other than its own
/// number of arguments. A body that stands for more arguments than its
/// saturated count is never pushed: the send that would hand them is
/// refused before any push. Every push checks it, a host's included, since a frame finds where its registers
/// above the header begin from its code's argument count once it is
/// reified, returned or suspended (`frame_argument_count`).
#[inline(always)]
fn debug_assert_arity(code: &crate::bytecode::CompiledCode, given: usize) {
    debug_assert!(
        given == usize::from(code.argument_count),
        "internal: an activation is handed {given} arguments, its code takes {}",
        code.argument_count
    );
}

/// Writes a pushed frame's `temporaries` -- its registers above the header
/// -- as its body needs them: nil into the named ones, below the body's
/// `local_count` (the copied values, the named temporaries and the
/// indirection vector's register), which the body may read before it
/// writes them, and nothing into the scratch, which no body reads before
/// it writes it (`bytecode::leftovers`, which the lowerings check and the
/// decoder holds an image to) and which the collector's pre-pass nils
/// before a trace (`Heap::trace_stack`). A debug build writes
/// `dead_register`'s word into the scratch, so that a read of one is
/// refused. A `local_count` below the arguments and the header's numbers,
/// which an image may carry, names no temporary.
#[inline(always)]
fn fill_temporaries(temporaries: &mut [u64], local_count: u16, argument_count: u16) {
    let named = usize::from(local_count)
        .saturating_sub(usize::from(argument_count) + frame_header::COUNT)
        .min(temporaries.len());
    let (named_registers, scratch) = temporaries.split_at_mut(named);
    named_registers.fill(Word::NIL.bits());
    #[cfg(debug_assertions)]
    scratch.fill(dead_register().bits());
    #[cfg(not(debug_assertions))]
    let _ = scratch;
}

/// Hands `visit` each run of the words of `range` that no window of
/// `claimed` covers, lowest first: `claimed` is disjoint, ordered by
/// start, the lowest last (`Heap::trace_stack`).
#[inline(always)]
fn for_each_unclaimed(
    claimed: &[Range<usize>],
    range: Range<usize>,
    mut visit: impl FnMut(Range<usize>),
) {
    let mut from = range.start;
    for window in claimed.iter().rev() {
        if window.start >= range.end || from >= range.end {
            break;
        }
        if window.end <= from {
            continue;
        }
        if window.start > from {
            visit(from..window.start);
        }
        from = window.end;
    }
    if from < range.end {
        visit(from..range.end);
    }
}

/// Writes `filler` into the words of `range` that no window of `claimed`
/// covers (`for_each_unclaimed`).
fn nil_unclaimed(words: &mut [u64], claimed: &[Range<usize>], range: Range<usize>, filler: u64) {
    for_each_unclaimed(claimed, range, |run| words[run].fill(filler));
}

/// The part of a frame's window the collector's pre-pass may write, and
/// the frame's words it traces (`Heap::trace_stack`).
struct FrameWindow {
    /// The stack word of the frame's register 0, its first argument.
    start: usize,
    /// The stack word of the first register above the header: no word
    /// below it is written.
    registers: usize,
    /// The window's end, or the top where that is lower: no word at or
    /// past it is written.
    until: usize,
    /// The header of the callee lying on the frame's slots, if one does:
    /// the frame stopped at that send, which taints every register from
    /// there up.
    callee_header: Option<usize>,
}

impl FrameWindow {
    /// Whether `nil` would write nothing: every word it may write -- from
    /// the lower of the first tainted register, `first_tainted`, and the
    /// callee's header up -- lies in the lowest window of `claimed`, as a
    /// deep recursion's frames, each covered by its callee's window, do.
    #[inline(always)]
    fn untouched(&self, claimed: &[Range<usize>], first_tainted: Option<u16>) -> bool {
        let tainted = first_tainted.map(|register| self.start + usize::from(register));
        let lowest = match (tainted, self.callee_header) {
            (Some(tainted), Some(callee_header)) => tainted.min(callee_header),
            (Some(lowest), None) | (None, Some(lowest)) => lowest,
            (None, None) => return true,
        };
        let from = lowest.max(self.registers);
        from >= self.until
            || claimed
                .last()
                .is_some_and(|window| window.start <= from && window.end >= self.until)
    }

    /// Writes `filler` into the registers of `tainted` and, where a callee
    /// lies on the slots, into every register from its header up, but
    /// never into a word of `claimed`, the windows of the frames above
    /// that reach into this one (`nil_unclaimed`).
    fn nil(
        &self,
        words: &mut [u64],
        claimed: &[Range<usize>],
        tainted: &[Range<u16>],
        filler: u64,
    ) {
        for range in tainted {
            let from = (self.start + usize::from(range.start)).max(self.registers);
            let until = (self.start + usize::from(range.end)).min(self.until);
            nil_unclaimed(words, claimed, from..until, filler);
        }
        if let Some(callee_header) = self.callee_header {
            nil_unclaimed(
                words,
                claimed,
                callee_header.max(self.registers)..self.until,
                filler,
            );
        }
    }

    /// Hands `tracer` every word of the window below the top -- its
    /// arguments, header and registers -- but those of `claimed`, which
    /// the frames above trace: called once `nil` has run, after which no
    /// frame writes these words, since a frame below writes only words its
    /// frames above do not claim.
    #[inline(always)]
    fn trace<T: crate::trace::Tracer>(
        &self,
        words: &mut [u64],
        claimed: &[Range<usize>],
        tracer: &mut T,
    ) {
        for_each_unclaimed(claimed, self.start..self.until, |run| {
            trace_words(&mut words[run], tracer)
        });
    }
}

/// A frame the pre-pass's walk met before its body kept the set the frame
/// needs (`Heap::trace_stack`), nilled once the fill has kept it. 72
/// bytes, which `docs/design/memory.md`'s bound of the walk's transient
/// memory counts ("What the collector traces"), pinned by
/// `a_deferred_frame_is_72_bytes`.
struct DeferredFrame {
    code_index: usize,
    program_counter: u32,
    window: FrameWindow,
    /// Where the windows above that reached into this frame's when the
    /// walk met it lie among the walk's recorded claims.
    claims: Range<usize>,
}

/// The set the pre-pass looked up last, by body and program counter
/// (`Heap::trace_stack`): a deep recursion's frames run one body at one
/// program counter, so each but the first skips the lookup of its code
/// row, the borrow of the body's sets and their search.
struct LastSet<'code> {
    code_index: usize,
    program_counter: u32,
    #[cfg(debug_assertions)]
    code: &'code CompiledCode,
    /// The kept set, `None` if no fill has kept it yet.
    tainted: Option<std::cell::Ref<'code, [Range<u16>]>>,
    /// The first register of the kept set, `None` if it is empty or not
    /// kept (`FrameWindow::untouched`).
    first_tainted: Option<u16>,
    /// Where the slots of the send at the program counter end, relative
    /// to a frame's start, `None` if the instruction there sends nothing;
    /// decoded the first time a callee on the slots asks.
    #[cfg(debug_assertions)]
    slot_end: Option<Option<usize>>,
}

impl<'code> LastSet<'code> {
    /// The set of the body in code row `code_index` before
    /// `program_counter`, kept in `last` for the frame below.
    #[inline(always)]
    fn of<'last>(
        last: &'last mut Option<LastSet<'code>>,
        tables: &'code Tables,
        code_index: usize,
        program_counter: u32,
    ) -> &'last mut LastSet<'code> {
        let hit = last.as_ref().is_some_and(|set| {
            set.code_index == code_index && set.program_counter == program_counter
        });
        if !hit {
            LastSet::look_up(last, tables, code_index, program_counter);
        }
        match last {
            Some(set) => set,
            None => unreachable!("internal: the set was just looked up"),
        }
    }

    /// Looks the set up afresh: `of`'s miss, out of line so that a hit
    /// stays a comparison.
    #[inline(never)]
    fn look_up(
        last: &mut Option<LastSet<'code>>,
        tables: &'code Tables,
        code_index: usize,
        program_counter: u32,
    ) {
        let Some(entry) = tables.codes.get(code_index).and_then(Option::as_ref) else {
            unreachable!("internal: a running frame names a live code row")
        };
        let code: &'code CompiledCode = &entry.code;
        let tainted = code.kept_tainted_before(program_counter);
        let first_tainted = tainted
            .as_ref()
            .and_then(|tainted| tainted.first())
            .map(|range| range.start);
        *last = Some(LastSet {
            code_index,
            program_counter,
            #[cfg(debug_assertions)]
            code,
            tainted,
            first_tainted,
            #[cfg(debug_assertions)]
            slot_end: None,
        });
    }

    /// Checks that a callee lies on the frame's slots, where `callee`, its
    /// first word, its header and whether it runs a block, says one does, only while the frame
    /// stands at the send whose slots end at that header -- or, for a
    /// block a compiled send's closure stub placed past the slots, whose
    /// first word is where they end (`jit::targets::ClosureStub`) --
    /// decoding the instruction once per body and program counter, never
    /// once per frame. Inline, so that a frame whose answer is decoded
    /// costs a comparison.
    #[cfg(debug_assertions)]
    #[inline(always)]
    fn check_callee(&mut self, start: usize, callee: Option<(usize, usize, bool)>) {
        let Some((callee_start, callee_header, block)) = callee else {
            return;
        };
        let slot_end = match self.slot_end {
            Some(slot_end) => slot_end,
            None => self.decode_slot_end(),
        };
        debug_assert!(
            slot_end.is_some_and(|slot_end| start + slot_end == callee_header
                || (block && start + slot_end == callee_start)),
            "internal: a callee lies on its sender's slots only while the sender stands at \
             the send"
        );
    }

    /// Decodes where the slots of the send at the program counter end,
    /// for `check_callee`, and keeps the answer.
    #[cfg(debug_assertions)]
    #[inline(never)]
    fn decode_slot_end(&mut self) -> Option<usize> {
        let slot_end = self
            .code
            .instruction(self.program_counter as usize)
            .argument_slots()
            .map(|(base, sources)| usize::from(base) + sources.len());
        self.slot_end = Some(slot_end);
        slot_end
    }
}

/// Adds `window` to `claimed` (`nil_unclaimed`), merging every window it
/// touches: one that starts at or below every window there, as a frame
/// does below the frames above it. Inline, so that a frame of a deep
/// recursion costs a comparison and a store (`claim_merging`).
#[inline(always)]
fn claim(claimed: &mut Vec<Range<usize>>, window: Range<usize>) {
    debug_assert!(
        claimed
            .last()
            .is_none_or(|lowest| lowest.start >= window.start),
        "internal: a frame starts at or below every frame above it"
    );
    // A frame that ends inside the lowest window -- a sender whose callee
    // lies on its slots, as down a deep recursion -- only lowers its start:
    // the window above it begins past the lowest's end.
    if let Some(lowest) = claimed.last_mut() {
        if lowest.start <= window.end && window.end <= lowest.end {
            lowest.start = window.start;
            return;
        }
    }
    claim_merging(claimed, window);
}

/// `claim`'s other case, out of line: `window` pushed, merged with every
/// window it touches.
#[inline(never)]
fn claim_merging(claimed: &mut Vec<Range<usize>>, window: Range<usize>) {
    let mut merged = window;
    while let Some(lowest) = claimed.last() {
        if lowest.start > merged.end {
            break;
        }
        merged.end = merged.end.max(lowest.end);
        claimed.pop();
    }
    claimed.push(merged);
}

/// Copies the registers of the frame whose arguments start at stack word
/// `start` and whose header is at `offset` -- its window, or the part of
/// it below `end` -- into `fields`, a frame object's register fields
/// (`Heap::frame_field_of_register`): the arguments, then the registers
/// above the header, the header itself left out; every field past `end`
/// nil, and every field of a register of `dead` -- ranges of register
/// numbers, which the frame does not read before it writes them -- nil
/// as well, since such a register may hold a word a callee or a popped
/// frame left, naming an object since moved or reclaimed, and a frame
/// object is traced as it lies. Answers whether a copied word names a
/// young object, which an old frame object must then be remembered for.
fn copy_registers_out(
    stack_words: &[u64],
    start: usize,
    offset: usize,
    end: usize,
    dead: &[Range<u16>],
    fields: &mut [u64],
) -> bool {
    let argument_count = offset - start;
    for (index, field) in fields.iter_mut().enumerate() {
        *field = if index < argument_count {
            stack_words[start + index]
        } else {
            let slot = offset + frame_header::COUNT + (index - argument_count);
            if slot < end {
                stack_words[slot]
            } else {
                Word::NIL.bits()
            }
        };
    }
    let above_header = argument_count + frame_header::COUNT;
    for range in dead {
        let (low, high) = (usize::from(range.start), usize::from(range.end));
        // Registers below the header are their own fields, those above it
        // `frame_header::COUNT` fields lower (`Heap::frame_field_of_register`).
        let arguments = low.min(argument_count)..high.min(argument_count);
        let registers = low.max(above_header) - frame_header::COUNT
            ..high.max(above_header) - frame_header::COUNT;
        for fields_range in [arguments, registers] {
            let until = fields_range.end.min(fields.len());
            if fields_range.start < until {
                fields[fields_range.start..until].fill(Word::NIL.bits());
            }
        }
    }
    fields.iter().any(|word| {
        Word::from_bits(*word)
            .object_offset()
            .is_some_and(space::is_young)
    })
}

impl ActivationStack {
    /// An empty stack in a reservation of its own, or running out of
    /// memory when the operating system grants no address space, as the
    /// object space does.
    fn new(machine_words: &MachineWords) -> ActivationStack {
        ActivationStack {
            frames: FrameStack::new(frames::stack_reservation(), machine_words.clone())
                .unwrap_or_else(|_| out_of_memory(frames::stack_reservation())),
            innermost: NO_FRAME,
            live: 0,
            machine_words: machine_words.clone(),
        }
    }

    /// The generation the next frame pushed takes: one past the last,
    /// which the machine words hold so that a compiled callee's prologue
    /// takes its own from the same counter
    /// (`MachineWord::NextGeneration`).
    #[cfg(debug_assertions)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the word holds a generation, masked to 31 bits"
    )]
    fn peek_next_generation(&self) -> u32 {
        let last = self.machine_words.get(MachineWord::NextGeneration) as u32;
        last.wrapping_add(1) & GENERATION_MASK
    }

    /// Counts the generation `peek_next_generation` answered as taken.
    #[cfg(debug_assertions)]
    fn advance_generation(&self) {
        let next = self.peek_next_generation();
        self.machine_words
            .set(MachineWord::NextGeneration, u64::from(next));
    }

    /// Header word `field` of the frame at `offset`.
    #[inline(always)]
    fn header_word(&self, offset: u32, field: usize) -> Word {
        Word::from_bits(self.frames.word(offset as usize + field))
    }

    /// Writes header word `field` of the frame at `offset`.
    #[inline(always)]
    fn set_header_word(&mut self, offset: u32, field: usize, word: Word) {
        self.frames.set_word(offset as usize + field, word.bits());
    }

    /// The flags of the frame at `offset`.
    #[inline(always)]
    fn flags(&self, offset: u32) -> FrameFlags {
        FrameFlags::of(self.header_word(offset, frame_header::FLAGS))
    }

    /// How many registers the frame at `offset` has above its header.
    #[inline(always)]
    fn register_count(&self, offset: u32) -> usize {
        frames::number_as_usize(self.header_word(offset, frame_header::REGISTER_COUNT))
    }

    /// Where the registers of the frame at `offset` end. A callee laid on
    /// the frame's slots begins below it; any other frame above it, at or
    /// past it.
    #[inline(always)]
    fn window_end(&self, offset: u32) -> usize {
        offset as usize + frame_header::COUNT + self.register_count(offset)
    }

    /// `window_end`, of a frame whose header machine code may have just
    /// written above the top.
    #[cfg(ns_jit)]
    fn window_end_of_committed(&self, offset: u32) -> usize {
        let count = Word::from_bits(
            self.frames
                .committed_word(offset as usize + frame_header::REGISTER_COUNT),
        );
        offset as usize + frame_header::COUNT + frames::number_as_usize(count)
    }

    /// Where the frame at `offset` starts: its first argument, its
    /// register 0, which lies `argument_count` words below its header.
    #[inline(always)]
    fn frame_start(&self, offset: u32) -> usize {
        offset as usize - usize::from(self.flags(offset).argument_count)
    }

    /// The top once the innermost frame, at `offset`, is gone: the top it
    /// was pushed at, or the frame's start for the bottom frame. That top
    /// is at or past its caller's window end -- past it where a frame
    /// below lies longer than the caller on its slots, since a push never
    /// lowers the top -- and its caller word records it as a distance
    /// from its start, so that the pop reads nothing of the caller's
    /// header (`frames::caller_link_word`). `flags` and `caller` are the
    /// frame's flags and caller words, which the pop has already read:
    /// taken rather than read again, since a read after the pop's own
    /// stores is one the compiler cannot fold into the first.
    #[inline(always)]
    fn top_without(&self, offset: u32, flags: FrameFlags, caller: Word) -> usize {
        let start = offset as usize - usize::from(flags.argument_count);
        if !caller.is_small_integer() {
            return start;
        }
        let top = start + frames::caller_top_above_start(caller);
        debug_assert!(
            top >= self.window_end(frames::caller_offset_of(caller)),
            "internal: a frame's caller word records a top at or past its caller's window end"
        );
        top
    }

    /// Takes the innermost frame, at `offset`, whose flags and caller words
    /// are `flags` and `caller`, off the stack's words: the top goes back
    /// to `top_without`'s, and nothing is written. A caller's
    /// registers a callee that lay on its slots gives back -- from the
    /// callee's header up -- are dead, and the collector's pre-pass nils
    /// them before any trace (`Heap::trace_stack`); a debug build writes
    /// `dead_register`'s word there, so that a read of one is refused.
    #[inline(always)]
    fn take_off_words(&mut self, offset: u32, flags: FrameFlags, caller: Word) {
        debug_assert!(
            flags == self.flags(offset)
                && caller.bits() == self.header_word(offset, frame_header::CALLER).bits(),
            "internal: a pop takes off the frame by its own header's words"
        );
        let top = self.top_without(offset, flags, caller);
        #[cfg(debug_assertions)]
        if (offset as usize) < top {
            self.frames
                .give_back_dead_registers(offset as usize, top, dead_register().bits());
            return;
        }
        self.frames.lower_top(top);
    }

    /// The offset of the frame below the one at `offset`, `None` for the
    /// bottom frame.
    #[inline(always)]
    fn caller(&self, offset: u32) -> Option<u32> {
        let word = self.header_word(offset, frame_header::CALLER);
        word.is_small_integer()
            .then(|| frames::caller_offset_of(word))
    }

    /// The continuation row the frame at `offset` names; `None` for one
    /// that delivers.
    #[inline(always)]
    fn continuation(&self, offset: u32) -> Option<ContinuationRow> {
        let word = self.header_word(offset, frame_header::CONTINUATION);
        word.is_small_integer()
            .then(|| ContinuationRow(frames::number_as_u32(word)))
    }

    /// Names `row` in the frame at `offset`; `None` for a frame that
    /// delivers.
    #[inline(always)]
    fn set_continuation(&mut self, offset: u32, row: Option<ContinuationRow>) {
        let word = row.map_or(Word::NIL, |row| frames::number_word(u64::from(row.0)));
        self.set_header_word(offset, frame_header::CONTINUATION, word);
    }

    /// The offset of every live frame, innermost first, by the caller words.
    fn offsets(&self) -> impl Iterator<Item = u32> + '_ {
        let innermost = (self.innermost != NO_FRAME).then_some(self.innermost);
        std::iter::successors(innermost, |offset| self.caller(*offset))
    }
}

/// What a suspended activation's frame cannot hold (`Heap::suspend_activations`):
/// the rest of its stack frame's header, kept until the frame is resumed. The frame
/// holds its registers, program counter and sender, and names its row by
/// its married word; a frame object grows by nothing for being suspended.
struct Suspension<'gc> {
    /// The suspended frame. The side table is a root: a suspended frame is
    /// part of a run that has not finished, whether or not anything else
    /// still reaches it.
    frame: FrameReference<'gc>,
    /// The continuation the frame's `CONTINUATION` word named, taken out of
    /// `Heap::continuations`; never a `Continuation::ReturnToSuspended`,
    /// whose sender the frame's `SENDER` holds instead.
    then: Continuation<'gc>,
    /// See `FrameFlags::counted`.
    counted: bool,
    /// See `frame_header::ENTERED_LEVEL`: the reclaim keeps the application
    /// alive while the activation is suspended, as it does for a running one.
    entered_level: ApplicationIndex,
    /// See `FrameFlags::retains_registers`.
    retains_registers: bool,
}

/// The heap's side table of suspended activations, a row per frame,
/// minted smallest free row first as the heap's other tables are.
#[derive(Default)]
struct Suspensions<'gc> {
    rows: Vec<Option<Suspension<'gc>>>,
    free: ids::FreeIndices,
}

impl crate::trace::Trace for Suspension<'_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let Suspension {
            frame,
            then,
            counted: _,           // not a handle
            entered_level: _,     // not a handle
            retains_registers: _, // not a handle
        } = self;
        tracer.frame(frame);
        then.trace(tracer);
    }
}

impl crate::trace::Trace for Suspensions<'_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let Suspensions { rows, free: _ } = self;
        for suspension in rows.iter_mut().flatten() {
            suspension.trace(tracer);
        }
    }
}

/// The row of `Continuations` that holds a stack frame's continuation,
/// which the frame's `CONTINUATION` word names. A frame that delivers has
/// none, so that a send's return tests one word and reaches no table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ContinuationRow(u32);

/// The heap's side table of the continuations of running activations other than
/// `Continuation::Deliver`, a row per activation, minted smallest free row
/// first as `Suspensions` mints its rows. A continuation is kept here rather
/// than in its stack frame so that the frame names it by a number, which a
/// frame of words holds as one word (`docs/design/execution.md`,
/// "Representation"). A root, as the stack is: a continuation holds the handles
/// of a native's or an unwind's state.
#[derive(Default)]
struct Continuations<'gc> {
    rows: Vec<Option<Continuation<'gc>>>,
    free: ids::FreeIndices,
}

impl<'gc> Continuations<'gc> {
    /// The row a new frame names for `continuation`: none for a
    /// `Continuation::Deliver`, which is never stored.
    #[inline(always)]
    fn store(&mut self, continuation: Continuation<'gc>) -> Option<ContinuationRow> {
        match continuation {
            // `Deliver` owns nothing, so forgetting it is dropping it,
            // without the call into `Continuation`'s out-of-line drop glue
            // that the end of the function would make (`vm::finish`'s
            // `apply_continuation` forgets it for the same reason).
            deliver @ Continuation::Deliver => {
                std::mem::forget(deliver);
                None
            }
            continuation => Some(self.insert(continuation)),
        }
    }

    /// Stores `continuation`, never a `Deliver`, in the smallest free row.
    /// An activation with a continuation of its own -- a future's
    /// computation, a nested class's clause -- takes a row and gives it
    /// back when it returns, and its row is usually the last one, so the
    /// common case appends to a table with no free row and is inline in
    /// the push; only a table with holes takes the free list's way.
    #[inline(always)]
    fn insert(&mut self, continuation: Continuation<'gc>) -> ContinuationRow {
        debug_assert!(
            !matches!(continuation, Continuation::Deliver),
            "internal: a delivering activation takes no row"
        );
        if !self.free.is_empty() {
            return self.insert_into_free_row(continuation);
        }
        self.rows.push(Some(continuation));
        Self::row_of(self.rows.len() - 1)
    }

    /// `insert` into the smallest free row: out of line, since a hole
    /// below the last row opens only when a continuation other than the
    /// top's is taken or replaced by a `Deliver` (an unwind, a suspension).
    #[cold]
    #[inline(never)]
    fn insert_into_free_row(&mut self, continuation: Continuation<'gc>) -> ContinuationRow {
        let row = self
            .free
            .take_smallest()
            .expect("internal: the caller checked a row is free") as usize;
        self.rows[row] = Some(continuation);
        Self::row_of(row)
    }

    #[inline(always)]
    fn row_of(index: usize) -> ContinuationRow {
        ContinuationRow(
            u32::try_from(index).expect("more than u32::MAX continuations live at once"),
        )
    }

    /// Takes the continuation out of `row` and frees the row. The last row
    /// is never free -- trailing free rows are truncated -- so taking it,
    /// the common case, pops it without the free list; a row
    /// below it, or a hole the pop uncovers, takes the free list's way.
    #[inline]
    fn take(&mut self, row: ContinuationRow) -> Continuation<'gc> {
        let index = row.0 as usize;
        if index + 1 != self.rows.len() {
            return self.take_below_the_last(row);
        }
        let continuation = self
            .rows
            .pop()
            .flatten()
            .expect("internal: a stack frame names a live continuation row");
        if matches!(self.rows.last(), Some(None)) {
            self.truncate_trailing();
        }
        continuation
    }

    /// `take` of a row below the last one: it becomes a free row.
    #[cold]
    #[inline(never)]
    fn take_below_the_last(&mut self, row: ContinuationRow) -> Continuation<'gc> {
        let continuation = self.rows[row.0 as usize]
            .take()
            .expect("internal: a stack frame names a live continuation row");
        self.free.free(row.0);
        continuation
    }

    /// Gives back the free rows the last row's pop left at the end.
    #[cold]
    #[inline(never)]
    fn truncate_trailing(&mut self) {
        ids::truncate_trailing(&mut self.rows, &mut self.free);
    }

    /// Replaces the continuation in `row` with `continuation`, never a
    /// `Deliver`, keeping the row.
    #[inline]
    fn replace(&mut self, row: ContinuationRow, continuation: Continuation<'gc>) {
        debug_assert!(
            !matches!(continuation, Continuation::Deliver),
            "internal: a delivering activation takes no row"
        );
        let slot = self.rows[row.0 as usize]
            .as_mut()
            .expect("internal: a stack frame names a live continuation row");
        *slot = continuation;
    }

    /// The continuation a frame naming `row` applies: `Deliver` for none.
    fn get(&self, row: Option<ContinuationRow>) -> &Continuation<'gc> {
        match row {
            Some(row) => self.rows[row.0 as usize]
                .as_ref()
                .expect("internal: a stack frame names a live continuation row"),
            None => &Continuation::Deliver,
        }
    }

    /// How many rows hold a continuation.
    fn count(&self) -> usize {
        self.rows.iter().flatten().count()
    }
}

impl crate::trace::Trace for Continuations<'_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let Continuations { rows, free: _ } = self;
        for continuation in rows.iter_mut().flatten() {
            continuation.trace(tracer);
        }
    }
}

/// The initial value of `Heap::collection_floor`, and of `next_collection`
/// before anything has collected, in bytes of objects allocated: about
/// 4,096 objects at a 128-byte average; `bench_loop.ns` and
/// `bench_send.ns` should never collect (`docs/design/memory.md`, "The
/// trigger and the growth policy").
const COLLECTION_FLOOR: usize = 512 << 10;

/// From this size on an object is allocated old: copying it through the
/// survivor spaces would buy nothing (Primordial Soup's
/// `kLargeAllocationSize`, `prior-art/primordialsoup/vm/heap.h:111`).
const LARGE_OBJECT_BYTES: usize = 32 << 10;

/// Past this many entries the remembered set asks for a scavenge that
/// tenures everything, which empties it (Soup's
/// `kRememberedSetOverflowSize`, `heap.h:112`, `heap.cc:279-283`).
const REMEMBERED_SET_LIMIT: usize = 8 << 10;

/// What eden keeps free past the point where it asks for a scavenge: what
/// a stretch of the run loop allocates before its safepoint serves the
/// request. An allocation past it goes old instead
/// (`docs/design/memory.md`, "Inline allocation").
const EDEN_RESERVE: usize = 512 << 10;

/// What makes an object a resolved future, read from the space's words by
/// both collectors: `replace_resolved_futures`, before a collection
/// marks, and the scavenger's weak pass, which sees through a resolved
/// future in a weak word or an ephemeron's key that nothing else keeps
/// (`docs/design/core-library.md`, "The heap and the collectors"). One
/// test for both, so that a scavenge and a collection judge such a word
/// alike.
///
/// A future is a `FixedPointers` object of the future class whose field
/// `state_index` holds `SmallInteger(resolved)`; its value is then field
/// `computation_index`, unless that value is itself a future -- one
/// crafted code corrupted, left for a forcing to refuse.
#[derive(Clone, Copy)]
pub(crate) struct ResolvedFutures {
    class: NonZeroU64,
    computation_index: usize,
    state_index: usize,
    resolved: u64,
}

impl ResolvedFutures {
    /// The test for instances of `future_class`. `future_class` must be a
    /// handle of the heap the test is used on, fresh at the call: the test
    /// keeps its offset, which the scavenger follows if it moves.
    pub(crate) fn new(
        future_class: ClassReference<'_>,
        computation_index: usize,
        state_index: usize,
        resolved: i64,
    ) -> ResolvedFutures {
        ResolvedFutures {
            class: future_class.offset,
            computation_index,
            state_index,
            resolved: Value::small_integer(resolved).word().bits(),
        }
    }

    /// The value word of the resolved future at `offset`, or `None` when
    /// it is no resolved future. `follow` answers where an object named by
    /// a word lives now: itself outside a scavenge, the end of its
    /// forwarders during one, so that a class or a future already copied
    /// is recognised as the original was.
    fn value_of(
        &self,
        words: &[u64],
        offset: NonZeroU64,
        follow: impl Fn(NonZeroU64) -> NonZeroU64,
    ) -> Option<Word> {
        let class = follow(self.class);
        let is_future = |object: NonZeroU64| {
            let header = header_index(object);
            Header(words[header]).format() == Format::FixedPointers
                && Word::from_bits(words[header + CLASS])
                    .object_offset()
                    .is_some_and(|named| follow(named) == class)
        };
        let future = follow(offset);
        if !is_future(future) {
            return None;
        }
        let fields = &words[block_range(words, header_index(future))];
        let fields = fields.get(FIELDS..)?;
        if *fields.get(self.state_index)? != self.resolved {
            return None;
        }
        let value = Word::from_bits(*fields.get(self.computation_index)?);
        match value.object_offset() {
            // Followed, as the future was: in a scavenge an inner future
            // already copied reads as a forwarder, which is no future.
            Some(target) if is_future(follow(target)) => None,
            _ => Some(value),
        }
    }
}

pub struct Heap<'gc> {
    /// The next identity hash to hand out. Hashes are assigned lazily, the
    /// first time `object_ensure_identity_hash` is asked for one, and never derived
    /// from an address or an offset, so identity survives both a moving
    /// collector and a snapshot (`docs/design/memory.md`) and allocation
    /// order stays unobservable (`docs/design/object-model.md`).
    next_identity_hash: u32,
    /// Every live activation. A frame is popped when its activation
    /// returns, so -- unlike `space` -- this is bounded by the deepest the
    /// program has been, not by everything it has run.
    stack: ActivationStack,
    /// Every suspended activation's row (`Suspension`): what its frame
    /// cannot hold, until `resume_activation` pushes it again. Empty
    /// outside a run.
    suspensions: Suspensions<'gc>,
    /// The continuations of the running activations other than
    /// `Continuation::Deliver`, which their stack frames name by row.
    continuations: Continuations<'gc>,
    /// Every object, at the byte offset an `ObjectReference` holds -- and
    /// every *reified* activation, as an object of the frame format at a
    /// `FrameReference`'s: one `Heap::reify` created because a block
    /// captured it, or one the image decoder rebuilt. An activation that is
    /// never reified never reaches the space at all. Nothing is removed
    /// except by a collection's compaction, which slides the survivors
    /// together and rewrites every surviving handle.
    ///
    /// Each object is one block of words (`object.rs`): the header, the
    /// class word, then the fields, sized by the header's word count, with
    /// an overflow word before the header when the count does not fit it
    /// (`space.rs`).
    space: Space,
    /// How many objects lie in the space: `heap_statistics`' count, kept
    /// as allocation and compaction go, since the space has no length to
    /// read it from.
    object_count: usize,
    /// How many of `object_count`'s objects lie in eden or a survivor space.
    young_objects: usize,
    /// Every old object that may name a young one, each once, its header's
    /// remembered bit set; the roots a scavenge adds to the activation
    /// stack's and the host's.
    remembered: Vec<NonZeroU64>,
    /// Whether every allocation goes old: the image decoder's graph pass
    /// sets it (`set_allocating_old`).
    allocating_old: bool,
    /// Whether a scavenge has been asked for and not yet run; the run loop's
    /// next safepoint serves it.
    scavenge_requested: bool,
    /// Whether the requested scavenge must tenure every survivor, as the
    /// remembered set's overflow demands.
    tenure_all_requested: bool,
    /// Eden's occupied bytes at which an allocation asks for a scavenge.
    scavenge_threshold: usize,
    /// The age at which the next scavenge tenures a survivor, set by each
    /// scavenge from its age table (`scavenge::tenuring_threshold`).
    tenuring_threshold: u8,
    /// How many scavenges have run, and how long `World::scavenge` has
    /// taken in total and at the longest (`record_scavenge`), as
    /// `collections` and its two pauses count the full collections.
    scavenges: usize,
    scavenge_pause: std::time::Duration,
    longest_scavenge_pause: std::time::Duration,
    /// The kernel-private `Activation` class, every frame object's class
    /// (`set_activation_class`). A handle the heap holds, so a collection
    /// marks it as a root and forwards it.
    activation_class: Option<ClassReference<'gc>>,
    /// The one object each mixin is, indexed by `MixinId::index`: an
    /// instance of the kernel-private `Mixin`, the reflectee of a mixin
    /// mirror (`docs/design/liveness-and-reflection.md`, "A mixin is
    /// reified as a kernel-private object"). Filled lazily by
    /// `Mutator::reified_mixin` (`record_mixin_object`). Weak, as
    /// `canonical_symbols` is: not a root for marking, and pruned by
    /// `prune_mixin_objects` between marking and compaction. An entry lives
    /// while its object is reachable, so `==` stays the mixin's identity for
    /// every comparison a program can make -- an object nothing reaches
    /// can be compared with nothing, and the next reflection makes a fresh
    /// one. A marked entry keeps its mixin's id alive. A scavenge traces the
    /// table strongly, since a scavenge reclaims no id, and every move
    /// forwards it.
    mixin_objects: Vec<Option<ObjectReference<'gc>>>,
    /// The one object each symbol is, indexed by `Symbol::index` (§5.1.6:
    /// equal symbols are one object; `docs/design/object-model.md`,
    /// "Values and objects"). Filled lazily by `Mutator::new_symbol`, the one
    /// way to make a symbol, and by the image decoder. Per heap, because a
    /// handle means nothing against another heap. Weak, as
    /// `mixin_objects` is: not a root for marking, pruned by
    /// `prune_canonical_symbols` between marking and compaction, and
    /// forwarded by `compact` like everything else that survives
    /// (`docs/design/memory.md`, "Weak references").
    canonical_symbols: Vec<Option<ObjectReference<'gc>>>,
    /// The inverse of `canonical_symbols`: each canonical symbol object's
    /// offset, and the symbol it is. Every Symbol object is canonical
    /// (`Mutator::new_symbol` and the image decoder are the only makers, and
    /// both record what they make), so `symbol_value` is one probe here
    /// rather than a search of the interner by the object's text. Keyed by
    /// offset, so it holds no handle: `record_canonical_symbol` inserts,
    /// `prune_canonical_symbols` removes, and `forward_everything` rebuilds
    /// it from the forwarded table after every move and every compaction.
    symbol_of: IdentityMap<u64, Symbol>,
    /// Every `Root` a host holds. A `RefCell` so that `stash` and `fetch`
    /// take `&self`, which lets a host stash an argument inside the call that
    /// sends it.
    host_roots: std::cell::RefCell<HostRoots>,
    /// The words machine code reads or writes between leaf calls, in an
    /// allocation of their own, never inline here (`machine_words.rs` has
    /// why). Among them `MachineWord::SafepointRequested`: whether the run
    /// loop's next safepoint must take its slow path
    /// (`Interpreter::safepoint`). The poll is one compare of that word
    /// (`docs/design/memory.md`, "Where a collection can happen"). An
    /// allocation that reaches the limit sets it, in every build
    /// (`request_collection`); in a debug build a stress mode additionally
    /// holds it set for its own reasons, for as long as the mode is on.
    /// Otherwise the safepoint that takes a request clears it.
    machine_words: MachineWords,
    /// The bytes the objects take, from the space's start to its top, at
    /// which the next allocation asks for a collection. Checked against the
    /// top the allocation already has in hand, so the hot path pays one
    /// predictable branch and carries no counter.
    next_collection: usize,
    /// Whether a limit has been reached since the last collection. Separate
    /// from the safepoint request, which only means "take the slow path" and
    /// which a stress mode holds set for its own reasons.
    collection_requested: bool,
    /// The smallest limit a collection will set, in bytes. A program that
    /// stays under it never collects: `bench_loop.ns` and `bench_send.ns`
    /// should not pay for a collector they do not need.
    collection_floor: usize,
    /// How many collections have run, and how long they have taken in total
    /// and at the longest (`Interpreter::collector_statistics`,
    /// `--collector-statistics`). `World::collect` is the only writer,
    /// through `record_collection`, and it times the whole collection with
    /// one `Instant::now()`/`elapsed()` -- these three fields cost nothing
    /// per allocation and nothing per instruction.
    collections: usize,
    total_pause: std::time::Duration,
    longest_pause: std::time::Duration,
    /// The instance ids of the builtin classes of the immediates, indexed by
    /// `Value::immediate_index`: an immediate has no class field to read one from.
    /// Set once by the bootstrap; `NONE` before it.
    immediate_dispatch_ids: [DispatchId; 5],
    /// The guards of a closure's and a frame's headers (`set_builtin_guards`).
    builtin_guards: BuiltinGuards,
    /// The mixins, applications and compiled bodies objects of this heap
    /// name by index (`tables.rs`).
    tables: Tables,
    /// Each application's instances dispatch id by index, for machine
    /// code (`application_ids.rs`), written wherever `tables` writes an
    /// application's row.
    application_ids: application_ids::ApplicationIds,
    /// Each block body's machine entry by code id, for machine code
    /// (`machine_entries.rs`), which the JIT writes and the free of a code
    /// id clears. Only where machine code runs.
    #[cfg(ns_jit)]
    machine_entries: machine_entries::MachineEntries,
    /// What a body's `CodeIdCache` must carry for its id to name a row of
    /// `tables` (`next_code_table_epoch`).
    code_table_epoch: u64,
    /// This heap's distinct id, stamped onto every handle it mints. Debug
    /// builds only; see `next_heap_id`. No accessor exists for it: nothing
    /// outside a `cfg`-guarded position may name a field that does not
    /// exist in a release build.
    #[cfg(debug_assertions)]
    heap_id: u32,
    /// How many times this heap has moved everything. Debug builds only; see
    /// `ObjectReference::epoch`.
    #[cfg(debug_assertions)]
    epoch: u32,
}

/// Prints the counter and how many activations, objects and host roots
/// there are, never their contents. A
/// derived `Debug` would walk every activation and every object ever
/// allocated, and a method's declaration reachable through a frame's kind
/// would print a whole AST -- the same hazard `CLAUDE.md`'s
/// "never `{:?}` a `Value`" names for a single object, now reachable
/// through the heap itself rather than through one handle. Nothing prints
/// a `Heap` today, so this is latent, but a derive would have turned a
/// future `{:?}` on a `Heap` -- or on anything that embeds one -- into an
/// unbounded print of every activation's and every object's data.
impl std::fmt::Debug for Heap<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `try_borrow`, not `borrow`: a `{:?}` written into a panic message
        // while `stash` holds the table would otherwise panic inside the
        // panic, and a debug print must never be what aborts the process.
        let host_roots = self.host_roots.try_borrow().map(|roots| roots.slots.len());
        let host_roots: &dyn std::fmt::Debug = match &host_roots {
            Ok(count) => count,
            Err(_) => &"<borrowed>",
        };
        f.debug_struct("Heap")
            .field("next_identity_hash", &self.next_identity_hash)
            .field("activations", &self.stack.live)
            .field("objects", &(self.object_count + self.machine_allocations()))
            .field("occupied bytes", &self.space.occupied_bytes())
            .field("canonical_symbols.len()", &self.canonical_symbols.len())
            .field("host roots", host_roots)
            .finish()
    }
}

/// How a move maps an old object offset to its new one. Three answer it:
/// the stress mode's `Shift`, which keeps every object, the collector's
/// `ForwardingTable`, which keeps only the live ones (`compaction.rs`), and
/// the bulk forward's `IdentityForward`, which moves nothing and maps each
/// forwarded object to its replacement. A type parameter rather than an
/// enum or a trait object, because `apply` runs once per handle in the
/// whole heap and a monomorphized call leaves neither a branch nor an
/// indirection there.
pub(crate) trait Relocation {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64;

    /// Whether the block `forward_everything` has just rewritten names a
    /// young object now, forgetting the answer for the next block. Only a
    /// relocation that leaves the generations in place asks: a move or a
    /// compaction keeps the remembered set itself, while the bulk forward
    /// can make an old holder name a young object no barrier saw stored.
    fn holder_done(&self) -> bool {
        false
    }
}

/// What `Heap::forward_identity` refuses, before it writes anything: what
/// the heap itself can judge. Which objects may be forwarded at all is the
/// VM mirror leaf's lock, checked before it calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardRefusal {
    /// The two arrays differ in size.
    UnequalSizes,
    /// An old object is named twice, and so would have two replacements.
    Split,
    /// An old object is also a new one, and so would be forwarded to
    /// something that is itself forwarded.
    Chain,
}

/// The bulk forward's relocation: each forwarded object's offset to its
/// replacement's, every other offset unchanged, since nothing moves.
pub(crate) struct IdentityForward {
    replacements: IdentityMap<u64, NonZeroU64>,
    /// Whether an answer since the last `holder_done` was young.
    names_young: std::cell::Cell<bool>,
}

impl Relocation for IdentityForward {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        let answer = self
            .replacements
            .get(&offset.get())
            .copied()
            .unwrap_or(offset);
        // Every young answer counts, an unchanged one too: an old holder
        // that named a young object before the forward is remembered
        // already, so remembering it again changes nothing, and the walk
        // leaves the remembered set holding every old object that names a
        // young one -- the view `verify_remembered_set` checks -- without
        // telling a word it rewrote from one it left.
        if space::is_young(answer) {
            self.names_young.set(true);
        }
        answer
    }

    fn holder_done(&self) -> bool {
        self.names_young.replace(false)
    }
}

/// The word index of the header at byte offset `offset`.
#[inline(always)]
fn header_index(offset: NonZeroU64) -> usize {
    (offset.get() / 8) as usize
}

/// The offset of the object whose header is word `header`: the inverse of
/// `header_index`.
fn offset_of_header(header: usize) -> NonZeroU64 {
    match NonZeroU64::new(8 * header as u64) {
        Some(offset) => offset,
        None => unreachable!("internal: an object lies past the first word"),
    }
}

/// How many bytes of objects the space may hold after a collection that
/// left `live` bytes in it before the next one is requested: no more is
/// allocated between two collections than is live, so the marking work per
/// allocated byte is amortized constant, and never less than `floor`, which
/// keeps a small program out of the collector altogether.
fn collection_limit(floor: usize, live: usize) -> usize {
    floor.max(live.saturating_mul(2))
}

/// The tracer a move or a collection forwards every handle with. In a debug
/// build it also checks each handle it forwards: one from another heap, or
/// one already stale, was stored in this heap by a mistake the rewrite would
/// otherwise make permanent.
pub(crate) struct Forwarder<M: Relocation> {
    #[cfg(debug_assertions)]
    heap_id: u32,
    #[cfg(debug_assertions)]
    from_epoch: u32,
    #[cfg(debug_assertions)]
    to_epoch: u32,
    objects: M,
}

impl<M: Relocation> crate::trace::Tracer for Forwarder<M> {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                handle.origin, self.heap_id,
                "a handle from another heap is stored in this one"
            );
            assert_eq!(
                handle.epoch, self.from_epoch,
                "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
                handle.epoch, self.to_epoch
            );
            handle.epoch = self.to_epoch;
        }
        handle.offset = self.objects.apply(handle.offset);
    }

    /// A frame is its frame object, so it moves as the object does.
    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                handle.origin, self.heap_id,
                "a handle from another heap is stored in this one"
            );
            assert_eq!(
                handle.epoch, self.from_epoch,
                "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
                handle.epoch, self.to_epoch
            );
            handle.epoch = self.to_epoch;
        }
        handle.offset = self.objects.apply(handle.offset);
    }

    /// A stored word carries no origin or epoch to check: `Heap::store`
    /// checked it when it was written, and every move rewrites it here.
    fn word(&mut self, word: &mut Word) {
        if let Some(offset) = word.object_offset() {
            *word = Word::with_object_offset(self.objects.apply(offset));
        }
    }
}

/// The object space's occupancy, read without disturbing it
/// (`Heap::heap_statistics`): `live_objects` is how many objects lie in it,
/// a reified frame counting as one, and `young_objects` how many of them
/// lie in the nursery; `occupied_bytes` the bytes they take in both
/// generations, overflow words included; `committed_bytes` the memory
/// committed for the space, the nursery included -- the gap between the
/// two is what the space can still fill before it commits more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapStatistics {
    pub live_objects: usize,
    pub young_objects: usize,
    pub occupied_bytes: usize,
    pub committed_bytes: usize,
}

/// The collector's counts and pauses, read without disturbing them
/// (`Heap::collector_statistics`): `scavenges`, `scavenge_pause` and
/// `longest_scavenge_pause` accumulate across every `World::scavenge` so
/// far, `collections`, `total_pause` and `longest_pause` across every
/// `World::collect`; `live_objects` is how many objects lie in the space
/// right now, the same field `HeapStatistics` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorStatistics {
    pub scavenges: usize,
    pub scavenge_pause: std::time::Duration,
    pub longest_scavenge_pause: std::time::Duration,
    pub collections: usize,
    pub total_pause: std::time::Duration,
    pub longest_pause: std::time::Duration,
    pub live_objects: usize,
}

/// A frame object's fixed fields as values and handles, before
/// `Heap::frame_fixed_words` stores them (`object.rs`'s `frame_field` gives
/// the layout). Its married field is not here: only `reify` sets it.
struct FrameFields<'gc> {
    sender: Option<FrameReference<'gc>>,
    home: Option<FrameReference<'gc>>,
    receiver: Value<'gc>,
    current_class: Option<ClassReference<'gc>>,
    program_counter: usize,
    /// The body that runs in the frame; `None` only for a top-level one.
    code: Option<CodeId>,
    kind: FrameKind<'gc>,
    uncontinuable: bool,
}

/// Running out of memory: the reservation is used up or the operating
/// system refused a commit. Aborts, as a failed allocation of the Rust
/// allocator does, so that no allocation carries a `Result` (decided with
/// the user on 2026-10-03; a Newspeak-level error is `docs/OPEN-WORK.md`'s).
/// `bytes` is what the failed request asked for: an allocation's size, or
/// what a commit asked for beyond what was committed.
#[cold]
pub(crate) fn out_of_memory(bytes: usize) -> ! {
    std::alloc::handle_alloc_error(
        std::alloc::Layout::from_size_align(bytes, 8)
            .unwrap_or_else(|_| std::alloc::Layout::new::<u64>()),
    )
}

/// A count or an index as the `SmallInteger` word a frame field stores.
/// Every one a frame stores -- a program counter, a code id, a kind tag, a
/// stack offset -- is far inside 61 bits; saturating keeps a count past
/// them from panicking.
fn count_word(count: usize) -> Word {
    count_value(count).word()
}

/// Writes `words` into `slots`, in order, as their bits: a new object's
/// fields, each word already stored (`Heap::store`).
fn write_words(slots: &mut [u64], words: impl IntoIterator<Item = Word>) {
    for (slot, word) in slots.iter_mut().zip(words) {
        *slot = word.bits();
    }
}

/// Asserts that a fast bump's object holds no word a scavenge poisoned:
/// every field of an object filled explicitly, rather than with nil, must
/// have been written. Asked only of pointer fields, whose valid words never
/// carry the poison's tag.
#[cfg(debug_assertions)]
fn assert_no_poison(words: &[u64]) {
    assert!(
        !words.contains(&header::POISON),
        "internal: a fresh object kept a word the scavenge poisoned"
    );
}

/// The married word of a frame suspended into side-table row `row`:
/// `-(row + 1)`, so that no row reads as a stack frame's offset.
fn suspended_word(row: usize) -> Word {
    // A row per suspended frame object, each of several words in a space
    // far smaller than 2^61 bytes, so no row index reaches the limit.
    let row = i64::try_from(row)
        .ok()
        .filter(|row| *row < SMALL_INTEGER_MAX)
        .expect("internal: a side-table row index fits a SmallInteger");
    Value::small_integer(-row - 1).word()
}

/// `count_word`'s value, for a field written through `set_pointer_word`.
fn count_value<'gc>(count: usize) -> Value<'gc> {
    let count = i64::try_from(count).unwrap_or(SMALL_INTEGER_MAX);
    Value::small_integer(count.min(SMALL_INTEGER_MAX))
}

impl<'gc> Heap<'gc> {
    /// A `Heap` built on the spot -- rather than the one `Interpreter` owns --
    /// is an empty space: every object and frame accessor indexes directly
    /// into `self.space`, so an `ObjectReference` or `FrameReference` minted
    /// by a different `Heap` either falls out of range and panics, or --
    /// should the two heaps' spaces both reach its offset -- names a
    /// different object or activation. In a debug build the second case
    /// is refused, not silent: every handle carries the id of the heap that
    /// minted it, and every accessor checks it against `self.heap_id` before
    /// indexing, so reading through the wrong heap panics at the read rather
    /// than answering. That check compiles out of a release build -- there the
    /// second case is exactly as silent as it sounds, which is why a reader
    /// after compiled code alone (`image::encode_code_table`, `ns-cli`'s
    /// `disasm`) takes mixins rather than a class handle, and why running the
    /// test suite in debug is what catches the mistake if it recurs.
    ///
    /// Private to the heap module: `World` builds the one heap an interpreter
    /// has, and only this module's own tests build a bare one.
    fn new() -> Heap<'gc> {
        let machine_words = MachineWords::new();
        machine_words.set(
            MachineWord::DepthLimit,
            crate::interpreter::MAX_ACTIVATION_DEPTH as u64,
        );
        let application_ids = application_ids::ApplicationIds::new();
        machine_words.set(
            MachineWord::ApplicationIds,
            application_ids.address() as u64,
        );
        let mut heap = Heap {
            next_identity_hash: 1,
            stack: ActivationStack::new(&machine_words),
            suspensions: Suspensions::default(),
            continuations: Continuations::default(),
            space: Space::new(space::DEFAULT_RESERVATION)
                .unwrap_or_else(|_| out_of_memory(space::DEFAULT_RESERVATION)),
            object_count: 0,
            young_objects: 0,
            remembered: Vec::new(),
            allocating_old: false,
            scavenge_requested: false,
            tenure_all_requested: false,
            scavenge_threshold: space::EDEN_BYTES - EDEN_RESERVE,
            tenuring_threshold: MAX_AGE,
            scavenges: 0,
            scavenge_pause: std::time::Duration::ZERO,
            longest_scavenge_pause: std::time::Duration::ZERO,
            activation_class: None,
            mixin_objects: Vec::new(),
            canonical_symbols: Vec::new(),
            symbol_of: IdentityMap::default(),
            host_roots: std::cell::RefCell::new(HostRoots {
                slots: Vec::new(),
                sweep_at: MIN_ROOT_SWEEP,
            }),
            machine_words,
            next_collection: COLLECTION_FLOOR,
            collection_requested: false,
            collection_floor: COLLECTION_FLOOR,
            collections: 0,
            total_pause: std::time::Duration::ZERO,
            longest_pause: std::time::Duration::ZERO,
            immediate_dispatch_ids: [DispatchId::NONE; 5],
            builtin_guards: BuiltinGuards::NONE,
            tables: Tables::default(),
            application_ids,
            #[cfg(ns_jit)]
            machine_entries: machine_entries::MachineEntries::new(),
            code_table_epoch: next_code_table_epoch(),
            #[cfg(debug_assertions)]
            heap_id: next_heap_id(),
            #[cfg(debug_assertions)]
            epoch: 0,
        };
        heap.space
            .set_young_threshold(Some(heap.scavenge_threshold));
        heap.machine_words
            .set(MachineWord::CodeTableEpoch, heap.code_table_epoch);
        heap
    }

    /// Draws the next number from the counter, which wraps from the widest
    /// hash the header word holds back to 1. Hashes are therefore not
    /// unique once the counter has wrapped, which a hash need not be:
    /// object identity is the handle's offset (`ObjectReference::same`), never
    /// the hash (`docs/design/object-model.md`, "The identity hash is 22
    /// bits wide"). Never answers 0, which means "not assigned".
    fn allocate_identity_hash(&mut self) -> u32 {
        let hash = self.next_identity_hash;
        self.next_identity_hash = Self::identity_hash_after(hash);
        hash
    }

    /// The hash the counter draws after `hash`: the next one, or 1 past the
    /// widest.
    fn identity_hash_after(hash: u32) -> u32 {
        if hash + 1 == IDENTITY_HASH_LIMIT {
            1
        } else {
            hash + 1
        }
    }

    /// Moves the counter past everything an image brought with it, so that
    /// a fresh hash does not repeat a decoded object's until the counter
    /// wraps -- after which hashes are not unique, which a hash need not be
    /// (`allocate_identity_hash`). Never moves it backwards: a lower maximum
    /// from a second load would otherwise repeat hashes the counter has
    /// already handed out. `max` fits the header word: the decoder refuses
    /// a wider hash (`DecodeError::IdentityHashTooWide`).
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        debug_assert!(max < IDENTITY_HASH_LIMIT);
        if self.next_identity_hash <= max {
            self.next_identity_hash = Self::identity_hash_after(max);
        }
    }

    // ---------- Object construction ----------
    //
    // Every construction of an `ObjectReference` in the workspace -- src or
    // tests -- goes through the typed allocations below, each of which
    // writes the object it names straight into the space, so that replacing
    // what backs a handle only ever touches the heap module.

    /// Allocates an object of `class` through `allocate_block`, so the
    /// collection limit applies: `format`, `unused` units left in its last
    /// word, `negative` for a `LargeInteger`'s sign, and `field_count` nil
    /// fields, whose word indices it answers for the caller to fill. It
    /// carries no identity hash until something asks for one
    /// (`object_ensure_identity_hash`), so allocating draws nothing from the
    /// counter. Private to the heap: callers name the kind of object they
    /// build (`allocate_instance` and its siblings).
    fn allocate_object(
        &mut self,
        class: ClassReference<'gc>,
        format: Format,
        unused: u8,
        negative: bool,
        field_count: usize,
    ) -> (ObjectReference<'gc>, Range<usize>) {
        let guard = self.header_dispatch_id(class);
        let class_word = self.store(Value::object(class));
        let word_count = field_count.saturating_add(1);
        if word_count < WORD_COUNT_OVERFLOW {
            let header = Header::new(guard, format, unused, word_count).with_negative(negative);
            if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
                fields.fill(Word::NIL.bits());
                self.count_young_allocation();
                let first = usize::try_from(offset.get()).expect(
                    "internal: an offset lies inside the object space's reservation, DEFAULT_RESERVATION",
                ) / 8
                    + FIELDS;
                return (self.object_handle(offset), first..first + field_count);
            }
        }
        self.allocate_block(class_word, guard, format, unused, negative, field_count)
    }

    /// An instance of `class` with `field_count` fields, every one nil.
    pub fn allocate_instance(
        &mut self,
        class: ClassReference<'gc>,
        field_count: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::FixedPointers, 0, false, field_count)
            .0
    }

    /// An instance of `class` with `slot_count` nil fields under `header`,
    /// which `class`'s application computed.
    pub(crate) fn allocate_instance_with(
        &mut self,
        class: ClassReference<'gc>,
        header: InstanceHeader,
        slot_count: usize,
    ) -> ObjectReference<'gc> {
        let header = Header(header.0);
        debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
        debug_assert_eq!(header.word_count(), slot_count + 1);
        let class_word = self.store(Value::object(class));
        if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
            fields.fill(Word::NIL.bits());
            self.count_young_allocation();
            return self.object_handle(offset);
        }
        self.allocate_instance(class, slot_count)
    }

    /// An `Array`-shaped object of `class` holding `elements`, in order.
    pub fn allocate_array(
        &mut self,
        class: ClassReference<'gc>,
        elements: &[Value<'gc>],
    ) -> ObjectReference<'gc> {
        // `store`'s check, made apart from the copy so that a release
        // build, which has no check, copies the words in one loop.
        #[cfg(debug_assertions)]
        for element in elements {
            self.store(*element);
        }
        let (object, fields) =
            self.allocate_object(class, Format::IndexablePointers, 0, false, elements.len());
        write_words(
            &mut self.space.words_mut()[fields],
            elements.iter().map(|element| element.word()),
        );
        object
    }

    /// An `Array`-shaped object of `class` of `length` nils: `Array new:`'s
    /// and `MakeVector`'s, with no list of elements in between.
    pub(crate) fn allocate_nil_array(
        &mut self,
        class: ClassReference<'gc>,
        length: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::IndexablePointers, 0, false, length)
            .0
    }

    /// An `Array`-shaped object of `class` holding the registers
    /// `registers` names in the window at `register_base`, in order:
    /// `MakeArray`'s, with each value going from its register straight
    /// into its element.
    pub(crate) fn allocate_array_from_registers(
        &mut self,
        class: ClassReference<'gc>,
        register_base: RegisterBase,
        registers: &[u16],
    ) -> ObjectReference<'gc> {
        // `register`'s window check and `store`'s origin check, made apart
        // from the copy, which reads the registers' words directly.
        #[cfg(debug_assertions)]
        for register in registers {
            self.store(self.register(register_base, *register));
        }
        let (object, fields) =
            self.allocate_object(class, Format::IndexablePointers, 0, false, registers.len());
        let base = register_base.index as usize;
        let Heap { space, stack, .. } = self;
        for (slot, register) in space.words_mut()[fields].iter_mut().zip(registers) {
            *slot = stack.frames.word(base + usize::from(*register));
        }
        object
    }

    /// Whether an `Array` of `length` elements can be allocated, asked
    /// before a length the program chose is. One below the large-object
    /// threshold is allocated young, where eden is committed, and needs no
    /// probe; a larger one is asked of `fields_fit`, as `byte_array_fits`
    /// asks for a `ByteArray`.
    pub(crate) fn array_fits(&self, length: usize) -> bool {
        let small = record_size::block_words(length)
            .checked_mul(8)
            .is_some_and(|bytes| bytes < LARGE_OBJECT_BYTES);
        small || self.fields_fit(length)
    }

    /// A `WeakArray` of `class` of `size` slots, every one nil.
    pub(crate) fn allocate_weak_array(
        &mut self,
        class: ClassReference<'gc>,
        size: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::WeakPointers, 0, false, size)
            .0
    }

    /// An `Ephemeron` of `class` with `field_count` fields, the key first,
    /// every one nil.
    pub(crate) fn allocate_ephemeron(
        &mut self,
        class: ClassReference<'gc>,
        field_count: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::Ephemeron, 0, false, field_count)
            .0
    }

    /// A String of `class` holding `text`.
    pub fn allocate_string(
        &mut self,
        class: ClassReference<'gc>,
        text: crate::CodePoints,
    ) -> ObjectReference<'gc> {
        let (format, unused, field_count) = object::string_shape(&text);
        let (object, fields) = self.allocate_object(class, format, unused, false, field_count);
        object::pack_code_points(&text, &mut self.space.words_mut()[fields]);
        object
    }

    /// A `ByteArray` of `class` of `size` bytes, every one zero. Its
    /// reserve is zeroed too, so that the space holds no stale words.
    pub(crate) fn allocate_zeroed_byte_array(
        &mut self,
        class: ClassReference<'gc>,
        size: usize,
    ) -> ObjectReference<'gc> {
        let unused = object::unused_bytes(size);
        let (object, fields) = self.allocate_object(
            class,
            Format::AlignedBytes,
            unused,
            false,
            object::byte_array_field_count(size),
        );
        self.space.words_mut()[fields].fill(0);
        object
    }

    /// Whether a `ByteArray` of `size` bytes can be allocated, asked before
    /// a size the program chose is: a failed allocation aborts the process
    /// as running out of memory, which no handler can catch. Two halves.
    /// The block, its reserve and overflow word included, must fit the old
    /// space of the reservation, which no commit can enlarge. And the
    /// operating system must grant memory of the object's size, probed as
    /// `Array new:` probes a large array, with `try_reserve_exact` of its
    /// field words dropped at once: a size inside the reservation can still
    /// be one a host that does not overcommit refuses to commit.
    pub(crate) fn byte_array_fits(&self, size: usize) -> bool {
        self.fields_fit(object::byte_array_field_count(size))
    }

    /// Whether a `WeakArray` of `size` slots can be allocated, asked before
    /// a size the program chose is, for `byte_array_fits`'s reason and by
    /// its two halves.
    pub(crate) fn weak_array_fits(&self, size: usize) -> bool {
        self.fields_fit(size)
    }

    /// Whether an object of `field_count` fields fits the old space of the
    /// reservation and the operating system grants memory of its size
    /// (`byte_array_fits`).
    fn fields_fit(&self, field_count: usize) -> bool {
        let words = record_size::block_words(field_count);
        let old = self.space.region(RegionName::Old);
        let inside_the_reservation = words
            .checked_mul(8)
            .is_some_and(|bytes| bytes <= old.end - old.base);
        inside_the_reservation && Vec::<u64>::new().try_reserve_exact(field_count).is_ok()
    }

    /// A `ByteArray` of `class` holding `bytes`, in order. Crate-private: a
    /// host makes one through `Mutator::new_byte_array`, of `ByteArray`'s
    /// class, since an `AlignedBytes` object of another class is one the
    /// image decoder refuses.
    pub(crate) fn allocate_byte_array(
        &mut self,
        class: ClassReference<'gc>,
        bytes: &[u8],
    ) -> ObjectReference<'gc> {
        let object = self.allocate_zeroed_byte_array(class, bytes.len());
        let written = self.with_byte_array_mut(object, |mut data| data.write(0, bytes));
        debug_assert_eq!(written, Some(true));
        object
    }

    /// A Symbol object of `class` for `symbol`. `Mutator::new_symbol` is the
    /// one caller, since a heap holds one object per symbol (§5.1.6).
    pub(crate) fn allocate_symbol(
        &mut self,
        class: ClassReference<'gc>,
        symbol: Symbol,
    ) -> ObjectReference<'gc> {
        self.allocate_string(class, crate::CodePoints::from(symbol.as_str()))
    }

    /// A `LargeInteger` object of `class` holding `integer`.
    /// `Mutator::new_integer` is the one caller, and decides that `integer`
    /// does not fit a `SmallInteger`, so each integer has one
    /// representation.
    pub(crate) fn allocate_large_integer(
        &mut self,
        class: ClassReference<'gc>,
        integer: crate::ArbitraryInteger,
    ) -> ObjectReference<'gc> {
        let digits = integer.magnitude();
        let (object, fields) = self.allocate_object(
            class,
            Format::SixtyFourBit,
            0,
            integer.is_negative(),
            digits.len(),
        );
        self.space.words_mut()[fields].copy_from_slice(digits);
        object
    }

    /// A `Fraction` object of `class` holding `numerator` and
    /// `denominator`, two integers in normal form.
    /// `Mutator::new_fraction` is the one caller, and has made them so.
    pub(crate) fn allocate_fraction(
        &mut self,
        class: ClassReference<'gc>,
        numerator: Value<'gc>,
        denominator: Value<'gc>,
    ) -> ObjectReference<'gc> {
        // `store`'s check, as `allocate_array` makes it.
        #[cfg(debug_assertions)]
        for part in [numerator, denominator] {
            self.store(part);
        }
        let (object, fields) = self.allocate_object(class, Format::FixedPointers, 0, false, 2);
        write_words(
            &mut self.space.words_mut()[fields],
            [numerator.word(), denominator.word()],
        );
        object
    }

    /// A reified mixin of `class`, the kernel-private `Mixin`, standing for
    /// the mixin `id` names: one field, the id as a `SmallInteger`.
    /// `Mutator::reified_mixin` is the one caller, and records what this
    /// answers (`record_mixin_object`), so that no second object stands for
    /// the same mixin.
    pub(crate) fn allocate_mixin_object(
        &mut self,
        class: ClassReference<'gc>,
        id: MixinId,
    ) -> ObjectReference<'gc> {
        let index = i64::try_from(id.index()).expect("a mixin id is 32 bits");
        let (object, fields) = self.allocate_object(
            class,
            Format::FixedPointers,
            0,
            false,
            record_size::MIXIN_FIELDS,
        );
        write_words(
            &mut self.space.words_mut()[fields],
            [Value::small_integer(index).word()],
        );
        object
    }

    /// A boxed `Float` of `class` holding `value`. `Mutator::new_float` is
    /// the one caller, and decides that `value` lies outside the immediate
    /// range, so each float has one representation.
    pub(crate) fn allocate_float(
        &mut self,
        class: ClassReference<'gc>,
        value: f64,
    ) -> ObjectReference<'gc> {
        let class_word = self.store(Value::object(class));
        let header = Header::new(
            self.immediate_dispatch_ids[FLOAT_DISPATCH_INDEX],
            Format::SixtyFourBit,
            0,
            2,
        );
        debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
        if let Some((offset, block)) = self.space.bump_object_array::<3>(header, class_word.bits())
        {
            block[FIELDS] = value.to_bits();
            self.count_young_allocation();
            return self.object_handle(offset);
        }
        let (object, fields) = self.allocate_object(class, Format::SixtyFourBit, 0, false, 1);
        self.space.words_mut()[fields].fill(value.to_bits());
        object
    }

    /// A block closure of `class` (`NewClosure` says what it holds).
    pub(crate) fn allocate_closure(
        &mut self,
        class: ClassReference<'gc>,
        closure: NewClosure<'gc>,
    ) -> ObjectReference<'gc> {
        let NewClosure {
            code,
            captured_frame,
            copied,
            feedback_base,
        } = closure;
        let fixed = self.closure_fixed_words(&code, captured_frame, feedback_base);
        #[cfg(debug_assertions)]
        for value in &copied {
            self.store(*value);
        }
        let field_count = closure_field::COUNT.saturating_add(copied.len());
        let class_word = self.store(Value::object(class));
        if field_count.saturating_add(1) < WORD_COUNT_OVERFLOW {
            let header = Header::new(
                self.builtin_guards.closure,
                Format::Closure,
                0,
                field_count + 1,
            );
            debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
            if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
                write_words(
                    fields,
                    fixed
                        .into_iter()
                        .chain(copied.iter().map(|value| value.word())),
                );
                #[cfg(debug_assertions)]
                assert_no_poison(fields);
                self.count_young_allocation();
                return self.object_handle(offset);
            }
        }
        let (object, fields) = self.allocate_object(
            class,
            Format::Closure,
            0,
            false,
            closure_field::COUNT + copied.len(),
        );
        write_words(
            &mut self.space.words_mut()[fields],
            fixed
                .into_iter()
                .chain(copied.iter().map(|value| value.word())),
        );
        object
    }

    /// A block closure of `class` running `code`, whose copied values are
    /// the registers `copied` names in the window at `register_base`: what
    /// `MakeClosure` builds. Each value goes from its register straight into
    /// the closure's word, with no list of them in between, which would be
    /// one more allocation and free per closure made.
    pub(crate) fn allocate_closure_from_registers(
        &mut self,
        class: ClassReference<'gc>,
        code: &Rc<CompiledCode>,
        captured_frame: FrameReference<'gc>,
        feedback_base: FeedbackBase,
        register_base: RegisterBase,
        copied: &[u16],
    ) -> ObjectReference<'gc> {
        let fixed = self.closure_fixed_words(code, captured_frame, feedback_base);
        // `register`'s window check and `store`'s origin check, made apart
        // from the copy, which reads the registers' words directly.
        #[cfg(debug_assertions)]
        for register in copied {
            self.store(self.register(register_base, *register));
        }
        let field_count = closure_field::COUNT.saturating_add(copied.len());
        let class_word = self.store(Value::object(class));
        if field_count.saturating_add(1) < WORD_COUNT_OVERFLOW {
            let header = Header::new(
                self.builtin_guards.closure,
                Format::Closure,
                0,
                field_count + 1,
            );
            debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
            let Heap { space, stack, .. } = self;
            if let Some((offset, fields)) = space.bump_object(header, class_word.bits()) {
                let (fixed_slots, copied_slots) = fields.split_at_mut(closure_field::COUNT);
                write_words(fixed_slots, fixed);
                let base = register_base.index as usize;
                for (slot, register) in copied_slots.iter_mut().zip(copied) {
                    *slot = stack.frames.word(base + usize::from(*register));
                }
                #[cfg(debug_assertions)]
                assert_no_poison(fields);
                self.count_young_allocation();
                return self.object_handle(offset);
            }
        }
        let (object, fields) = self.allocate_object(
            class,
            Format::Closure,
            0,
            false,
            closure_field::COUNT + copied.len(),
        );
        let Heap { space, stack, .. } = self;
        let (fixed_slots, copied_slots) =
            space.words_mut()[fields].split_at_mut(closure_field::COUNT);
        write_words(fixed_slots, fixed);
        let base = register_base.index as usize;
        for (slot, register) in copied_slots.iter_mut().zip(copied) {
            *slot = stack.frames.word(base + usize::from(*register));
        }
        object
    }

    /// A closure's fixed fields, `closure_field`'s, stored: the one place
    /// they are laid out.
    fn closure_fixed_words(
        &mut self,
        code: &Rc<CompiledCode>,
        captured_frame: FrameReference<'gc>,
        feedback_base: FeedbackBase,
    ) -> [Word; closure_field::COUNT] {
        self.check_frame_origin(captured_frame);
        let mut fields = [Word::NIL; closure_field::COUNT];
        fields[closure_field::CODE_ID] = count_word(self.code_id(code).index());
        fields[closure_field::CAPTURED_FRAME] =
            self.store(Value::object(self.frame_object(captured_frame)));
        fields[closure_field::FEEDBACK_BASE] = count_word(feedback_base.raw() as usize);
        fields
    }

    /// A class object (`NewClass` says what it holds) whose header class is
    /// `metaclass`. Its own dispatch id is `metaclass`'s instances id, as
    /// every object's is its class's.
    pub fn allocate_class(
        &mut self,
        metaclass: ClassReference<'gc>,
        class: NewClass<'gc>,
    ) -> ClassReference<'gc> {
        let fixed = self.class_fixed_words(class);
        let (object, fields) =
            self.allocate_object(metaclass, Format::Class, 0, false, class_field::COUNT);
        write_words(&mut self.space.words_mut()[fields], fixed);
        object
    }

    /// A level a `<:` clause added (`newspeak-spec.pdf` §6.2): a class object
    /// as `allocate_class` makes one, and one field more, the class object
    /// `named` its clause named (`class_field::NAMED_CLASS`). The clause
    /// itself is set after, as every class object's later fields are.
    ///
    /// A body-less declaration's class object (`body_less`) has two fields
    /// more, its declaration's enclosing object and declaring class, nil
    /// until `class_set_declaration_scope` writes them: the declaration
    /// that becomes this class object knows them only once its superclass
    /// clause has run.
    pub(crate) fn allocate_applied_level(
        &mut self,
        metaclass: ClassReference<'gc>,
        class: NewClass<'gc>,
        named: ClassReference<'gc>,
        body_less: bool,
    ) -> ClassReference<'gc> {
        let fixed = self.class_fixed_words(class);
        let named = self.store(Value::object(named));
        let field_count = if body_less {
            class_field::BODY_LESS_CLASS_COUNT
        } else {
            class_field::APPLIED_LEVEL_COUNT
        };
        let (object, fields) =
            self.allocate_object(metaclass, Format::Class, 0, false, field_count);
        let words = &mut self.space.words_mut()[fields];
        write_words(&mut words[..class_field::COUNT], fixed);
        words[class_field::NAMED_CLASS] = named.bits();
        object
    }

    /// A body-less declaration's scope as the two words its class object
    /// stores, enclosing object first.
    fn declaration_scope_words(&self, scope: DeclarationScope<'gc>) -> [Word; 2] {
        let DeclarationScope {
            enclosing,
            declaring_class,
        } = scope;
        [
            self.store(enclosing.map_or(Value::nil(), Value::object)),
            self.store(declaring_class.map_or(Value::nil(), Value::object)),
        ]
    }

    /// A class object's fields from its record, stored: the one place they
    /// are laid out, for a new class object and for a placeholder filled in
    /// place (`fill_placeholder_class`). Every field set after creation
    /// starts nil: `this_class`, the applied clause and the value-class
    /// verdict each have their setter. The application is named by its
    /// index, so it must be the table's own entry.
    fn class_fixed_words(&self, class: NewClass<'gc>) -> [Word; class_field::COUNT] {
        let NewClass {
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin,
        } = class;
        debug_assert!(
            self.tables
                .applications
                .get(application.index().index())
                .and_then(Option::as_ref)
                .is_some_and(|entry| Rc::ptr_eq(entry, &application)),
            "internal: a class object's application is registered (`Heap::register_application`)"
        );
        let optional = |object: Option<ObjectReference<'gc>>| {
            self.store(object.map_or(Value::nil(), Value::object))
        };
        let mut fields = [Word::NIL; class_field::COUNT];
        fields[class_field::ENCLOSING] = optional(enclosing);
        fields[class_field::DECLARING_CLASS] = optional(declaring_class);
        fields[class_field::SUPERCLASS] = optional(superclass);
        if let Some(builtin) = builtin {
            // Its discriminant is its position in `BuiltinClass::ALL`, the
            // order `Mutator::builtin_class` indexes by too.
            fields[class_field::BUILTIN] = count_word(builtin as usize);
        }
        fields[class_field::APPLICATION] = count_word(application.index().index());
        fields
    }

    /// A class object that is genuinely its own `.class`. Under `Rc` this
    /// needed `Rc::new_uninit`, a manual strong-count bump and
    /// `Rc::from_raw`, because an object's class is non-optional and
    /// `Rc::new_cyclic` only ever offers a `Weak` that fails to upgrade until
    /// construction finishes. A heap of offsets has no such problem: the
    /// object is allocated first and its class word then written with its
    /// own offset, in safe Rust.
    ///
    /// Like every object, it carries no identity hash until one is asked
    /// for.
    pub fn allocate_self_referential_class(&mut self, class: NewClass<'gc>) -> ClassReference<'gc> {
        // It is an instance of itself, so its id is its own instances id.
        let dispatch_id = class.application.dispatch_ids.instances;
        let fixed = self.class_fixed_words(class);
        let (object, fields) =
            self.allocate_self_referential_guarded(dispatch_id, Format::Class, class_field::COUNT);
        write_words(&mut self.space.words_mut()[fields], fixed);
        object
    }

    /// An object whose class is itself, with `field_count` nil fields and no
    /// class object's format: the shape a crafted image can give a class
    /// reference, which the heap's own tests build. It guards on `NONE`:
    /// being its own class, it has no class that could supply an instances
    /// id.
    #[cfg(test)]
    fn allocate_self_referential(&mut self, field_count: usize) -> ObjectReference<'gc> {
        self.allocate_self_referential_guarded(DispatchId::NONE, Format::FixedPointers, field_count)
            .0
    }

    /// An object whose class is itself, of `format` with `field_count` nil
    /// fields, guarding on `dispatch_id`: its own class is not yet in the
    /// space to read, so the caller supplies it. Its class word is written
    /// with the object's own offset before anything reads it. Answers the
    /// object and its fields' word indices, as `allocate_block` does.
    fn allocate_self_referential_guarded(
        &mut self,
        dispatch_id: DispatchId,
        format: Format,
        field_count: usize,
    ) -> (ObjectReference<'gc>, Range<usize>) {
        let (handle, fields) =
            self.allocate_block(Word::NIL, dispatch_id, format, 0, false, field_count);
        let class_word = self.store(Value::object(handle));
        self.block_mut(handle)[CLASS] = class_word.bits();
        (handle, fields)
    }

    /// An object for `image::decode::decode_graph_section` to fill later
    /// in place (`shape_placeholder` and the other `fill_placeholder_*`
    /// fills): an instance of `class` with `field_count` nil
    /// fields, the size the first pass read off the record, safe to hold and
    /// never observed before it is filled.
    pub(crate) fn allocate_placeholder(
        &mut self,
        class: ClassReference<'gc>,
        field_count: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, Format::FixedPointers, 0, false, field_count)
            .0
    }

    /// Fills a placeholder in place as a class object of `class` (its
    /// metaclass, or `Metaclass` for a metaclass record) with
    /// `identity_hash`, holding what `record` says; `false`, and nothing
    /// changed, when the placeholder does not have a class object's fields.
    /// A valid first pass never sizes a class record otherwise, and an
    /// object never changes its size in the space, so the caller refuses
    /// the image rather than keep a wrong object.
    /// `image::decode::decode_graph_section`'s second pass is the only
    /// caller, and it only ever calls this, and the other `fill_placeholder_*`
    /// fills below, on a handle its first pass placeholder-allocated and
    /// nothing else has observed yet -- before `check_class_references` and
    /// `verify_graph` run, both of which assume every object is already
    /// filled in. Call one on any handle something else already holds and it
    /// silently replaces that object's already-verified invariants with
    /// unverified ones: no error, and no way for verification to notice,
    /// because verification has already finished.
    ///
    /// The object carries `NONE` in its header until `stamp_dispatch_ids`
    /// runs -- `class` may itself be a still-placeholder record with no
    /// application yet -- and must not be dispatched on before then.
    ///
    /// `named` is a level's named class (`allocate_applied_level`), and the
    /// placeholder has that field exactly when it is given; `scope` is a
    /// body-less declaration's (`class_declaration_scope`), given only with
    /// `named`, and the placeholder has its two fields exactly when it is
    /// given.
    pub(crate) fn fill_placeholder_class(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        record: NewClass<'gc>,
        named: Option<ClassReference<'gc>>,
        scope: Option<DeclarationScope<'gc>>,
    ) -> bool {
        self.check_object_origin(object);
        let fields = self.class_fixed_words(record);
        let named = named.map(|named| self.store(Value::object(named)));
        let scope = scope.map(|scope| self.declaration_scope_words(scope));
        for word in fields
            .iter()
            .chain(named.iter())
            .chain(scope.iter().flatten())
        {
            self.debug_assert_tenured(*word);
        }
        let expected = match (named, scope) {
            (Some(_), Some(_)) => class_field::BODY_LESS_CLASS_COUNT,
            (Some(_), None) => class_field::APPLIED_LEVEL_COUNT,
            (None, None) => class_field::COUNT,
            (None, Some(_)) => return false,
        };
        let placeholder = self.fields_mut(object);
        if placeholder.len() != expected {
            return false;
        }
        write_words(&mut placeholder[..class_field::COUNT], fields);
        if let Some(named) = named {
            placeholder[class_field::NAMED_CLASS] = named.bits();
        }
        if let Some([enclosing, declaring_class]) = scope {
            placeholder[class_field::DECLARATION_ENCLOSING] = enclosing.bits();
            placeholder[class_field::DECLARATION_DECLARING_CLASS] = declaring_class.bits();
        }
        self.rewrite_placeholder_header(object, class, identity_hash, Format::Class, 0);
        true
    }

    /// Begins filling a placeholder in place, as an instance or an `Array`
    /// of `class` (`format`) with `identity_hash`: its header becomes the one
    /// `allocate_block` writes for an object of that format and size,
    /// guarding on `NONE` until `stamp_dispatch_ids` runs, and its class word
    /// names `class`. Its fields stay nil until `fill_placeholder_field`
    /// writes them. The image decoder's second pass fills a record this way
    /// rather than building a block beside the placeholder the first pass
    /// sized, so a record costs its placeholder and nothing more while it is
    /// read (`image::decode::decode_value` has the figures). The same
    /// caveats as `fill_placeholder_class`'s apply.
    pub(crate) fn shape_placeholder(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        format: PlaceholderFormat,
    ) {
        let format = match format {
            PlaceholderFormat::Instance => Format::FixedPointers,
            PlaceholderFormat::Array => Format::IndexablePointers,
            PlaceholderFormat::WeakArray => Format::WeakPointers,
            PlaceholderFormat::Ephemeron => Format::Ephemeron,
        };
        self.rewrite_placeholder_header(object, class, identity_hash, format, 0);
    }

    /// Stores `value` into field `index` of a placeholder being filled in
    /// place (`shape_placeholder`, `fill_placeholder_closure`, or a frame's
    /// registers after its fixed fields, `fill_frame_register`); `false` past
    /// its fields, which the first pass sized, so the caller refuses rather
    /// than indexes.
    pub(crate) fn fill_placeholder_field(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        let word = self.store(value);
        self.debug_assert_tenured(word);
        match self.fields_mut(object).get_mut(index) {
            Some(slot) => {
                *slot = word.bits();
                true
            }
            None => false,
        }
    }

    /// Fills a placeholder in place as a String or a Symbol of `class` with
    /// `identity_hash` holding `text`, packed into its words exactly as
    /// `allocate_string` packs the `CodePoints` of `text`: one byte a unit
    /// while every code point fits Latin-1, 32 bits otherwise. `false`, and
    /// nothing written, when the placeholder is not the size the first pass
    /// read for that text (`record_size::string_fields`).
    pub(crate) fn fill_placeholder_string(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        text: &str,
    ) -> bool {
        self.check_object_origin(object);
        let Some((format, unused)) = object::pack_string(text, self.fields_mut(object)) else {
            return false;
        };
        self.rewrite_placeholder_header(object, class, identity_hash, format, unused);
        true
    }

    /// Fills a placeholder in place as a `ByteArray` of `class` with
    /// `identity_hash` holding `bytes`, in order: its header becomes the one
    /// `allocate_byte_array` writes, its data begin at the 32-byte address
    /// the placeholder's own place gives them, and every other field word,
    /// the reserve among them, is zero. `false`, and nothing written, when
    /// the placeholder is not the size the first pass read for that many
    /// bytes (`record_size::byte_array_fields`) -- which is also what keeps
    /// an `AlignedBytes` block shorter than its reserve out of the heap.
    pub(crate) fn fill_placeholder_byte_array(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        bytes: &[u8],
    ) -> bool {
        self.check_object_origin(object);
        let fields = self.fields_mut(object);
        if fields.len() != object::byte_array_field_count(bytes.len()) {
            return false;
        }
        fields.fill(0);
        let unused = object::unused_bytes(bytes.len());
        self.rewrite_placeholder_header(object, class, identity_hash, Format::AlignedBytes, unused);
        let written = self.with_byte_array_mut(object, |mut data| data.write(0, bytes));
        debug_assert_eq!(written, Some(true));
        true
    }

    /// Begins filling a placeholder in place as a closure of `class` with
    /// `identity_hash`, running `code` and capturing `captured_frame`: its
    /// header, class word and the fixed fields `allocate_closure` writes,
    /// with a feedback base not yet assigned, as every decoded closure's is.
    /// Its copied values follow through `fill_placeholder_field`, from
    /// `record_size::CLOSURE_FIXED_FIELDS` on.
    pub(crate) fn fill_placeholder_closure(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        code: &Rc<CompiledCode>,
        captured_frame: FrameReference<'gc>,
    ) {
        self.check_frame_origin(captured_frame);
        let code_word = count_word(self.code_id(code).index());
        let frame_word = self.store(Value::object(self.frame_object(captured_frame)));
        self.debug_assert_tenured(frame_word);
        let base_word = count_word(FeedbackBase::NOT_ASSIGNED.raw() as usize);
        self.rewrite_placeholder_header(object, class, identity_hash, Format::Closure, 0);
        let fields = self.fields_mut(object);
        fields[closure_field::CODE_ID] = code_word.bits();
        fields[closure_field::CAPTURED_FRAME] = frame_word.bits();
        fields[closure_field::FEEDBACK_BASE] = base_word.bits();
    }

    /// A placeholder's header and class word, rewritten as
    /// `allocate_block` writes them for an object of `format` with
    /// `unused` units left in its last word, at the placeholder's own size.
    fn rewrite_placeholder_header(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        format: Format,
        unused: u8,
    ) {
        self.check_object_origin(object);
        self.check_object_origin(class);
        let class_word = self.store(Value::object(class));
        self.debug_assert_tenured(class_word);
        let block = self.block_mut(object);
        let header = Header::new(DispatchId::NONE, format, unused, block.len() - CLASS)
            .with_negative(false)
            .with_identity_hash(identity_hash);
        block[HEADER] = header.0;
        block[CLASS] = class_word.bits();
    }

    /// Where every allocation enters an object into the space: bumps the
    /// top by the object's words, and by an overflow word first when the
    /// header cannot hold its count, and writes the header -- `format`,
    /// `unused` units left in the last word, `negative`, guarding on
    /// `guard`, no identity hash yet -- the class word `class`, and
    /// `field_count` fields, every one nil. Answers the object and its
    /// fields' word indices in `self.space.words()`, for the caller to write
    /// what it holds; nothing moves before it has. A 64-bit handle has no
    /// ceiling the space could reach, so none is checked; a reservation used
    /// up is running out of memory.
    ///
    /// The cold path of every allocation: a fast bump (`Space::bump_object`)
    /// takes every young object whose header holds its count while eden is
    /// below its threshold, so this is reached for an object that crosses
    /// the threshold, one eden cannot hold, a large one, one with an
    /// overflow word, and everything the image decoder allocates.
    #[cold]
    #[inline(never)]
    fn allocate_block(
        &mut self,
        class: Word,
        guard: DispatchId,
        format: Format,
        unused: u8,
        negative: bool,
        field_count: usize,
    ) -> (ObjectReference<'gc>, Range<usize>) {
        let word_count = field_count.saturating_add(1);
        let header = Header::new(guard, format, unused, word_count).with_negative(negative);
        let offset = self.place_block(header, class, word_count);
        let first = usize::try_from(offset.get()).expect(
            "internal: an offset lies inside the object space's reservation, DEFAULT_RESERVATION",
        ) / 8
            + FIELDS;
        let fields = first..first + field_count;
        self.space.words_mut()[fields.clone()].fill(Word::NIL.bits());
        (self.object_handle(offset), fields)
    }

    /// `allocate_block`'s decision and its words but the fields: places
    /// the block of an object of `word_count` words after its header --
    /// young below the large-object threshold while eden has room, asking
    /// for the scavenge once eden passes its threshold, old once eden is
    /// spent -- writes its overflow word when the header cannot hold the
    /// count, `header` and the class word `class`, counts the object, and
    /// remembers an old one, whose fields its caller fills with no
    /// barrier. Answers the object's offset; its fields hold whatever the
    /// space held, which the caller writes before anything reads them.
    /// Never collects.
    fn place_block(&mut self, header: Header, class: Word, word_count: usize) -> NonZeroU64 {
        let allocation = allocation_words(word_count);
        let bytes = allocation.saturating_mul(8);
        let young = !self.allocating_old && bytes < LARGE_OBJECT_BYTES;
        let start = match young
            .then(|| self.space.allocate_young(allocation))
            .flatten()
        {
            Some(start) => {
                self.young_objects += 1;
                let eden = self.space.region(RegionName::Eden);
                if eden.top - eden.start >= self.scavenge_threshold {
                    self.request_scavenge(false);
                }
                start
            }
            None => {
                // Eden spent: the scavenge is asked for and the object goes
                // old until it runs, as Strongtalk's `allocate_tenured` does
                // when a scavenge cannot run
                // (`prior-art/strongtalk/vm/memory/universe_more.cpp:55-64`).
                if young {
                    self.request_scavenge(false);
                }
                let Some(start) = self.space.allocate_old(allocation) else {
                    out_of_memory(bytes)
                };
                if self.old_occupied() >= self.next_collection {
                    self.request_collection();
                }
                start
            }
        };
        let words = self.space.words_mut();
        let mut first = start / 8;
        if word_count >= WORD_COUNT_OVERFLOW {
            words[first] = overflow_word(word_count);
            first += 1;
        }
        debug_assert_eq!(header.word_count(), word_count.min(WORD_COUNT_OVERFLOW));
        words[first + HEADER] = header.0;
        words[first + CLASS] = class.bits();
        self.object_count += 1;
        let Some(offset) = NonZeroU64::new(8 * first as u64) else {
            unreachable!("internal: an object lies past the first object")
        };
        // Its initializing stores took no barrier, so an old object that may
        // be filled with young values joins the remembered set at birth; the
        // decoder's never does, since the load tenured everything it names.
        if !space::is_young(offset) && !self.allocating_old {
            self.remember(offset);
        }
        offset
    }

    /// `Leaf::Allocate`'s allocation, past the limit of machine code's
    /// inline bump: the block of an object of `word_count` words after its
    /// header, its header word `header` and its class word `class_word`,
    /// placed as `allocate_block` places it (`place_block`), which never
    /// collects. Answers the object's word, its offset; the caller fills
    /// its fields.
    #[cfg(ns_jit)]
    pub(crate) fn allocate_for_machine_code(
        &mut self,
        word_count: u64,
        header: u64,
        class_word: u64,
    ) -> u64 {
        let Ok(word_count) = usize::try_from(word_count) else {
            unreachable!("internal: machine code allocates an object it could address");
        };
        self.place_block(Header(header), Word::from_bits(class_word), word_count)
            .get()
    }

    /// Counts the objects machine code's inline bump placed
    /// (`MachineWord::MachineAllocations`) into the heap's own counts, as
    /// `count_young_allocation` counts the interpreter's: before anything
    /// recounts them -- a scavenge, a compaction.
    fn settle_machine_allocations(&mut self) {
        let count = self.machine_allocations();
        self.object_count += count;
        self.young_objects += count;
        self.machine_words.set(MachineWord::MachineAllocations, 0);
    }

    /// The objects machine code's inline bump placed since the heap last
    /// counted them (`settle_machine_allocations`).
    fn machine_allocations(&self) -> usize {
        let Ok(count) = usize::try_from(self.machine_words.get(MachineWord::MachineAllocations))
        else {
            unreachable!("internal: an allocation count fits the address space");
        };
        count
    }

    /// Counts a fast bump's object, as `allocate_block` counts its own.
    #[inline(always)]
    fn count_young_allocation(&mut self) {
        self.young_objects += 1;
        self.object_count += 1;
    }

    /// The one place an `ObjectReference` is built, so the `cfg` on `origin`
    /// appears once rather than at every allocation. A struct-expression field
    /// may carry a `cfg` attribute, which is what makes the release build see a
    /// plain 8-byte handle.
    fn object_handle(&self, offset: NonZeroU64) -> ObjectReference<'gc> {
        ObjectReference {
            offset,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: std::marker::PhantomData,
        }
    }

    /// The `FrameReference` for the frame object at `offset`; only a frame
    /// object's offset is ever passed.
    fn frame_handle(&self, offset: NonZeroU64) -> FrameReference<'gc> {
        FrameReference {
            offset,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: std::marker::PhantomData,
        }
    }

    // ---------- Frame construction ----------
    //
    // A reified activation is an object of the frame format and an instance
    // of the kernel-private `Activation` class: the fixed fields
    // `object.rs`'s `frame_field` names, then a register file. A frame
    // carries no identity hash until something asks for one, like every
    // object. `reify` builds a real activation's frame;
    // `image::decode::decode_graph_section` allocates a placeholder of the
    // size its first pass read (`allocate_placeholder_frame`) and fills it
    // in place once the class objects are rebuilt (`fill_frame_fields`,
    // `fill_frame_register`).

    /// Records the class every frame object is an instance of: the
    /// kernel-private `Activation`. The bootstrap records it where it builds
    /// the builtin classes, before anything can reify an activation; a bare
    /// heap's test records one of its own.
    pub(crate) fn set_activation_class(&mut self, class: ClassReference<'gc>) {
        self.check_object_origin(class);
        self.activation_class = Some(class);
    }

    /// Allocates a frame object of `fields` holding `registers` and then
    /// nils up to `register_count`. Every `FrameReference` in the workspace
    /// is minted here or by `object_frame`, from a frame object's offset.
    ///
    /// `home` is what `closure_home` reads back for a closure created in this
    /// frame, and the image records it, so it must be populated exactly for a
    /// `FrameKind::Closure` activation. `reify` upholds that by building `home`
    /// from the activation's `kind`, but nothing in the type system ties the
    /// two fields together, so a new constructor added later would record a
    /// return target that disagrees with the frame's own closure silently
    /// rather than fail loudly. Checked here, once, rather than at every
    /// constructor. Not checked in `fill_frame_fields`: the decoder fills a
    /// placeholder through that path with fields read off the wire, and nothing
    /// reactivates a decoded frame, so it need not satisfy this.
    fn allocate_frame(
        &mut self,
        fields: FrameFields<'gc>,
        registers: &[Value<'gc>],
        register_count: usize,
    ) -> FrameReference<'gc> {
        debug_assert_eq!(
            fields.home.is_some(),
            matches!(fields.kind, FrameKind::Closure(_)),
            "a frame's home must be populated exactly for a closure activation"
        );
        let class = self
            .activation_class
            .expect("internal: the bootstrap records the Activation class before anything reifies");
        debug_assert!(registers.len() <= register_count);
        let fixed = self.frame_fixed_words(fields);
        #[cfg(debug_assertions)]
        for register in registers {
            self.store(*register);
        }
        let field_count = frame_field::COUNT.saturating_add(register_count);
        let class_word = self.store(Value::object(class));
        if field_count.saturating_add(1) < WORD_COUNT_OVERFLOW {
            let header = Header::new(self.builtin_guards.frame, Format::Frame, 0, field_count + 1);
            debug_assert_eq!(header.guard(), self.header_dispatch_id(class));
            if let Some((offset, fields)) = self.space.bump_object(header, class_word.bits()) {
                let (fixed_slots, register_slots) = fields.split_at_mut(frame_field::COUNT);
                write_words(fixed_slots, fixed);
                let (given, rest) = register_slots.split_at_mut(registers.len());
                write_words(given, registers.iter().map(|register| register.word()));
                rest.fill(Word::NIL.bits());
                #[cfg(debug_assertions)]
                assert_no_poison(fields);
                self.count_young_allocation();
                return self.frame_handle(offset);
            }
        }
        let (object, words) = self.allocate_object(
            class,
            Format::Frame,
            0,
            false,
            frame_field::COUNT + register_count,
        );
        // The registers past `registers` are the nils `allocate_block` wrote.
        write_words(
            &mut self.space.words_mut()[words],
            fixed
                .into_iter()
                .chain(registers.iter().map(|register| register.word())),
        );
        self.frame_handle(object.offset)
    }

    /// A frame object's fixed fields, `frame_field`'s, stored: the one place
    /// they are laid out, for a new frame (`allocate_frame`) and for a
    /// placeholder filled in place (`fill_frame_fields`).
    fn frame_fixed_words(&self, fields: FrameFields<'gc>) -> [Word; frame_field::COUNT] {
        let FrameFields {
            sender,
            home,
            receiver,
            current_class,
            program_counter,
            code,
            kind,
            uncontinuable,
        } = fields;
        let frame_word = |frame: Option<FrameReference<'gc>>| match frame {
            Some(frame) => self.store(Value::object(self.frame_object(frame))),
            None => Word::NIL,
        };
        let object_word = |object: Option<ObjectReference<'gc>>| match object {
            Some(object) => self.store(Value::object(object)),
            None => Word::NIL,
        };
        use crate::image::opcodes::{
            FRAME_CLOSURE, FRAME_FACTORY, FRAME_METHOD, FRAME_SUPERCLASS_CLAUSE, FRAME_TOP_LEVEL,
        };
        let (tag, operand) = match kind {
            // The method is the code id's (`code_method`).
            FrameKind::Method(_) => (FRAME_METHOD, None),
            FrameKind::Closure(closure) => (FRAME_CLOSURE, Some(closure)),
            FrameKind::TopLevel => (FRAME_TOP_LEVEL, None),
            FrameKind::Initializer(class, body) => (body.frame_tag(), Some(class)),
            FrameKind::Factory(class) => (FRAME_FACTORY, Some(class)),
            FrameKind::SuperclassClause(class) => (FRAME_SUPERCLASS_CLAUSE, Some(class)),
        };
        let mut words = [Word::NIL; frame_field::COUNT];
        words[frame_field::SENDER] = frame_word(sender);
        words[frame_field::HOME] = frame_word(home);
        words[frame_field::RECEIVER] = self.store(receiver);
        words[frame_field::CURRENT_CLASS] = object_word(current_class);
        words[frame_field::PROGRAM_COUNTER] = count_word(program_counter);
        words[frame_field::CODE_ID] = code.map_or(Word::NIL, |id| count_word(id.index()));
        words[frame_field::KIND] = count_word(usize::from(tag));
        words[frame_field::KIND_OPERAND] = object_word(operand);
        words[frame_field::UNCONTINUABLE] = Value::boolean(uncontinuable).word();
        words
    }

    /// A frame for `image::decode::decode_graph_section` to fill later
    /// (`fill_frame_fields`): a top-level activation with `register_count` nil
    /// registers, the size the first pass read off the record, safe to hold
    /// and never observed before it is filled.
    pub(crate) fn allocate_placeholder_frame(
        &mut self,
        register_count: usize,
    ) -> FrameReference<'gc> {
        self.allocate_frame(
            FrameFields {
                sender: None,
                home: None,
                receiver: Value::nil(),
                current_class: None,
                program_counter: 0,
                code: None,
                kind: FrameKind::TopLevel,
                uncontinuable: false,
            },
            &[],
            register_count,
        )
    }

    /// A returned top-level frame no activation produced, built from values:
    /// `temporaries` are its registers above the header, register
    /// `HEADER_REGISTERS + index` being `temporaries[index]`. Test support,
    /// public only for `tests/image.rs`'s `sender` round trip: the runtime
    /// never sets a frame's `sender`, so only a hand-made frame carries one
    /// into the encoder.
    ///
    /// A handle from another interpreter's heap carries another brand and
    /// does not compile here. A debug build also checks every handle's
    /// origin and move epoch at run time: each value as `store` stores it,
    /// `sender` and `current_class` before the frame is built.
    #[doc(hidden)]
    pub fn allocate_returned_top_level_frame(
        &mut self,
        receiver: Value<'gc>,
        temporaries: &[Value<'gc>],
        sender: Option<FrameReference<'gc>>,
        current_class: Option<ClassReference<'gc>>,
    ) -> FrameReference<'gc> {
        if let Some(sender) = sender {
            self.check_frame_origin(sender);
        }
        if let Some(current_class) = current_class {
            self.check_object_origin(current_class);
        }
        self.allocate_frame(
            FrameFields {
                sender,
                home: None,
                receiver,
                current_class,
                program_counter: 0,
                code: None,
                kind: FrameKind::TopLevel,
                uncontinuable: true,
            },
            temporaries,
            temporaries.len(),
        )
    }

    /// The frame object `frame` is: the object the kernel's exception
    /// machinery holds as an activation, and what a frame's `sender` and
    /// `home` fields store.
    pub fn frame_object(&self, frame: FrameReference<'gc>) -> ObjectReference<'gc> {
        self.check_frame_origin(frame);
        self.object_handle(frame.offset)
    }

    /// The frame `object` is, or `None` when it is no frame object. `None`
    /// rather than a panic, so that a kernel leaf handed some other object
    /// by a program answers a failure instead of aborting.
    pub fn object_frame(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>> {
        self.check_object_origin(object);
        (self.header(object).format() == Format::Frame).then(|| self.frame_handle(object.offset))
    }

    // ---------- Handle-origin checks ----------
    //
    // Called as the first line of every accessor below that takes a handle:
    // an offset from another heap can lie inside this one's space and name a
    // different, valid object, as an index from another heap names a
    // different, valid activation, so without this the read would just
    // answer the wrong thing. `describe` checks the object it was handed and
    // does not re-check the class it reaches through, which by construction
    // came out of the same heap.

    /// A stored word as a value: given this heap's id and current epoch,
    /// since every collection rewrites every stored word, so no stored word
    /// can be stale. Crate-private, as `store` is: a `Word` carries no
    /// brand, so a public pair would carry a value from one heap's brand
    /// into another's past every check (`docs/design/memory.md`, "The
    /// brand").
    #[inline(always)]
    pub(crate) fn load(&self, word: Word) -> Value<'gc> {
        Value::from_word(
            word,
            #[cfg(debug_assertions)]
            self.heap_id,
            #[cfg(debug_assertions)]
            self.epoch,
        )
    }

    /// A value as a word to store. In a debug build the value must carry
    /// this heap's id and current epoch: a stored word has nothing left for
    /// the next move to check, so a stale handle is refused here or never.
    #[inline(always)]
    pub(crate) fn store(&self, value: Value<'gc>) -> Word {
        #[cfg(debug_assertions)]
        if let Some(object) = value.as_object() {
            self.check_object_origin(object);
        }
        value.word()
    }

    /// Refuses a handle that another heap allocated. Compiles to nothing in
    /// a release build: the origin field does not exist there, so this is a
    /// no-op the optimizer removes entirely. The message names the two ids
    /// because the useful question when this fires is *which* extra heap
    /// got built -- usually a `Heap::new()` standing in for one that is out
    /// of scope.
    #[inline]
    fn check_object_origin(&self, object: ObjectReference<'gc>) {
        #[cfg(debug_assertions)]
        assert_eq!(
            object.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            object.origin, self.heap_id
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            object.epoch, self.epoch,
            "stale handle: minted in move epoch {} of this heap, read in move epoch {} -- \
             a host keeps a value across a run only as a `Root`",
            object.epoch, self.epoch
        );
        let _ = object;
    }

    #[inline]
    fn check_frame_origin(&self, frame: FrameReference<'gc>) {
        #[cfg(debug_assertions)]
        assert_eq!(
            frame.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            frame.origin, self.heap_id
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            frame.epoch, self.epoch,
            "stale handle: minted in move epoch {} of this heap, read in move epoch {} -- \
             a host keeps a value across a run only as a `Root`",
            frame.epoch, self.epoch
        );
        let _ = frame;
    }

    /// `check_frame_origin` for a live activation, plus the check only a
    /// stack handle needs: that its frame is still the one it was minted
    /// for, by the generation its header records. Compiles to nothing in a
    /// release build.
    #[inline]
    fn check_activation(&self, activation: ActivationReference) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                activation.origin, self.heap_id,
                "handle belongs to a different heap (handle from heap {}, read through heap {})",
                activation.origin, self.heap_id
            );
            assert_eq!(
                self.frame_generation(activation.offset),
                Some(activation.generation),
                "stale activation handle: its activation has returned"
            );
        }
        let _ = activation;
    }

    /// The generation the frame at `offset` records, `None` when no frame
    /// header fits there below the top, or when the words there cannot be
    /// one: a register count whose window passes the committed stack, or
    /// an argument count that reaches below the stack, or a caller word
    /// that is neither nil nor an offset below this one. A word of a popped
    /// frame's registers, or of a later frame's, reads as a generation only
    /// when all three hold by coincidence, which the check accepts: it is
    /// the debug build's net, not a proof.
    #[cfg(debug_assertions)]
    fn frame_generation(&self, offset: u32) -> Option<u32> {
        let start = offset as usize;
        let words = self.stack.frames.words();
        let header = words.get(start..start + frame_header::COUNT)?;
        let number = |field: usize| {
            let word = Word::from_bits(header[field]);
            word.is_non_negative_small_integer()
                .then(|| frames::number_of(word))
        };
        // Against the committed stack, not the top: a stale handle may
        // still point at a popped frame's header above the top, whose
        // words the committed stack holds, and the check reads them there.
        let register_count = usize::try_from(number(frame_header::REGISTER_COUNT)?).ok()?;
        if register_count > self.stack.frames.committed() - start - frame_header::COUNT {
            return None;
        }
        let caller = Word::from_bits(header[frame_header::CALLER]);
        let caller_below = caller.bits() == Word::NIL.bits()
            || (caller.is_non_negative_small_integer()
                && frames::caller_offset_of(caller) < offset);
        if !caller_below {
            return None;
        }
        let flags = Word::from_bits(header[frame_header::FLAGS]);
        if !flags.is_non_negative_small_integer()
            || usize::from(FrameFlags::of(flags).argument_count) > start
        {
            return None;
        }
        Some(FrameFlags::of(flags).generation)
    }

    /// The handle for the live frame at `offset`, minted with that frame's
    /// own generation. The one place an `ActivationReference` is built.
    #[inline(always)]
    fn activation_handle(&self, offset: u32) -> ActivationReference {
        ActivationReference {
            offset,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            generation: self
                .frame_generation(offset)
                .expect("internal: a handle is minted for a live frame"),
        }
    }

    /// Where register `index` of the live frame at `offset` sits in the
    /// stack's words. The window check is what stops an out-of-range index
    /// from silently reading the neighbouring activation's header or
    /// register, which is in bounds of the stack and so would not panic on
    /// its own.
    ///
    /// Registers `0 .. argument_count` are the arguments, below the
    /// header; the next `frame_header::COUNT` numbers are the header's,
    /// which no register access names, so that every register is one
    /// addition from the frame's start (`docs/design/compiler.md`,
    /// "Registers").
    #[inline(always)]
    fn register_slot(&self, offset: u32, index: u16) -> usize {
        let argument_count = self.stack.flags(offset).argument_count;
        let slot = offset as usize - usize::from(argument_count) + index as usize;
        debug_assert!(
            index < argument_count
                || (usize::from(index) >= usize::from(argument_count) + frame_header::COUNT
                    && slot < self.stack.window_end(offset)),
            "register {index} is outside its activation's register window, or one of its header's numbers"
        );
        slot
    }

    // ---------- Object accessors ----------
    //
    // `&self`/`&mut self` index into `self.space`, so the borrow checker
    // ties every accessor's result to the `Heap` it read from -- the same
    // discipline `docs/design/memory.md` describes for frames.

    /// `object`'s block: the one lender of an object's words to the
    /// accessors, private to the heap, so that every public accessor copies
    /// out (`docs/design/memory.md`, "The seam"). Ends where the header's
    /// count says, so that no accessor bounded by the block reaches the
    /// next object. A debug build refuses a header no live object has --
    /// the poison a scavenge leaves over the nursery it vacated, or a
    /// forwarder -- since only a word that missed the write barrier still
    /// names one.
    #[inline(always)]
    fn block(&self, object: ObjectReference<'gc>) -> &[u64] {
        self.check_object_origin(object);
        let words = self.space.words();
        #[cfg(debug_assertions)]
        {
            let format = Header(words[header_index(object.offset)]).format();
            assert!(
                !matches!(format, Format::Free | Format::Forwarded),
                "a handle names vacated nursery memory: a store missed the write barrier"
            );
        }
        let range = block_range(words, header_index(object.offset));
        &words[range]
    }

    /// `block`, for a write.
    #[inline(always)]
    fn block_mut(&mut self, object: ObjectReference<'gc>) -> &mut [u64] {
        self.check_object_origin(object);
        let words = self.space.words_mut();
        #[cfg(debug_assertions)]
        {
            let format = Header(words[header_index(object.offset)]).format();
            assert!(
                !matches!(format, Format::Free | Format::Forwarded),
                "a handle names vacated nursery memory: a store missed the write barrier"
            );
        }
        let range = block_range(words, header_index(object.offset));
        &mut words[range]
    }

    /// `object`'s header word.
    #[inline(always)]
    fn header(&self, object: ObjectReference<'gc>) -> Header {
        Header(self.block(object)[HEADER])
    }

    /// Overwrites `object`'s header word.
    fn set_header(&mut self, object: ObjectReference<'gc>, header: Header) {
        self.block_mut(object)[HEADER] = header.0;
    }

    /// `object`'s fields: its words after the header and the class word.
    #[inline]
    fn fields(&self, object: ObjectReference<'gc>) -> &[u64] {
        &self.block(object)[FIELDS..]
    }

    /// `fields`, for a write.
    #[inline]
    fn fields_mut(&mut self, object: ObjectReference<'gc>) -> &mut [u64] {
        &mut self.block_mut(object)[FIELDS..]
    }

    /// The class `block`'s class word names. Only `allocate_block` and
    /// `rewrite_placeholder_header` write one, from a class handle or the
    /// object's own, so it always names an object.
    #[inline]
    fn block_class(&self, block: &[u64]) -> ClassReference<'gc> {
        match self.load(Word::from_bits(block[CLASS])).as_object() {
            Some(class) => class,
            None => unreachable!("internal: a block's class word names an object"),
        }
    }

    /// The identity hash stored on `object`, 0 if nothing has asked for one
    /// yet. A read never assigns one: the image encoder writes exactly what is
    /// stored, so snapshotting a world does not change it.
    pub fn object_identity_hash(&self, object: ObjectReference<'gc>) -> u32 {
        self.header(object).identity_hash()
    }

    /// `object`'s identity hash, drawn from the counter the first time anything
    /// asks -- `Object>>hash` is the one caller. Assigned lazily, as Cog/Spur
    /// (`hashBitsOf:`, which calls `newHashBitsOf:` when the bits are 0) and
    /// Primordial Soup (`Object_identityHash`) do, so that allocation order is
    /// unobservable: the reference lowering allocates closures the optimized
    /// one does not, and closure conversion and a JIT's escape analysis will
    /// add and remove allocations the same way (`docs/design/object-model.md`).
    pub fn object_ensure_identity_hash(&mut self, object: ObjectReference<'gc>) -> u32 {
        let header = self.header(object);
        let stored = header.identity_hash();
        if stored != 0 {
            return stored;
        }
        let hash = self.allocate_identity_hash();
        self.set_header(object, header.with_identity_hash(hash));
        hash
    }

    /// An opaque, hashable key unique to `object`, for code that needs an
    /// `ObjectReference` as a `HashMap`/`HashSet` key rather than a comparison
    /// -- `image::encode::GraphTable`'s object-id map and
    /// `image::decode::rebuild_applications`'s superclass-position map.
    /// `identity_hash` cannot serve that role, because every object nothing has
    /// asked for its hash stores 0, which would fold two distinct objects into
    /// one table entry.
    ///
    /// It lives on `Heap` rather than on `ObjectReference` so that the origin
    /// check applies to it like it does to every accessor. Two handles from
    /// different heaps can carry the same offset and so produce the same key,
    /// which collides silently in a caller's map; requiring the `Heap` that
    /// minted the handle is what turns that into a panic, and it is the
    /// compiler that requires it, since there is no way to reach the offset
    /// without one.
    ///
    /// The key is the handle's byte offset, and nothing may do arithmetic on
    /// it: it says nothing about the object but which one it is. This is the
    /// only way a handle's offset leaves this module -- a frame's key is its
    /// frame object's -- so anything that changes what a handle encodes --
    /// widening it to carry a heap tag, say -- has to check its callers even
    /// though the field itself is private. They all treat the result as an
    /// opaque key, which is what keeps that a check rather than a rewrite.
    pub(crate) fn object_identity_key(&self, object: ObjectReference<'gc>) -> usize {
        self.check_object_origin(object);
        usize::try_from(object.offset.get())
            .expect("internal: an object's offset fits the target's address width")
    }

    /// Overwrites `object`'s identity hash. Three legitimate callers:
    /// `image::decode::decode_tag_named_hashes`, which puts a recorded hash
    /// back onto an object of the shipped world -- no record, so nothing
    /// else could restore what the image remembered for one --
    /// `image::decode::decode_graph_section`, which gives a frame record's
    /// frame object, allocated as a frame rather than shaped from a
    /// placeholder's header, the hash its record carries, and -- besides
    /// this module's own tests -- `tests/image.rs`'s
    /// `an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating`,
    /// which forces a live object's hash to the widest the header holds by
    /// hand, the only way to reach `advance_identity_hash_past`'s wrap
    /// without actually hashing four million objects first.
    ///
    /// Calling this on anything else is a raw, unchecked overwrite of the one
    /// value identity-keyed lookups, `become:` and a loaded image's roots all
    /// rest on: it can leave two live objects sharing a hash, and every
    /// identity-keyed collection and `become:`'s stability guarantee then
    /// silently breaks, with nothing anywhere raising an error to say so.
    pub fn object_set_identity_hash(&mut self, object: ObjectReference<'gc>, hash: u32) {
        debug_assert!(hash < IDENTITY_HASH_LIMIT);
        let header = self.header(object);
        self.set_header(object, header.with_identity_hash(hash));
    }

    #[inline]
    pub fn object_class(&self, object: ObjectReference<'gc>) -> ClassReference<'gc> {
        self.block_class(self.block(object))
    }

    /// Which kind of payload `object` carries. The encoder and the graph
    /// walk branch on it, then read the payload through the accessor for
    /// that kind.
    pub fn object_shape(&self, object: ObjectReference<'gc>) -> ObjectShape {
        match self.header(object).format() {
            Format::FixedPointers if self.mixin_object_id(object).is_some() => ObjectShape::Mixin,
            Format::FixedPointers => ObjectShape::Instance,
            Format::IndexablePointers => ObjectShape::Array,
            Format::WeakPointers => ObjectShape::WeakArray,
            Format::Ephemeron => ObjectShape::Ephemeron,
            Format::Frame => ObjectShape::Activation,
            Format::Closure => ObjectShape::Closure,
            Format::Class => ObjectShape::Class,
            Format::AlignedBytes => ObjectShape::ByteArray,
            Format::Bytes | Format::ThirtyTwoBit if self.symbol_value(object).is_some() => {
                ObjectShape::Symbol
            }
            Format::Bytes | Format::ThirtyTwoBit => ObjectShape::String,
            _ if self.is_boxed_float(object) => ObjectShape::Float,
            _ => ObjectShape::LargeInteger,
        }
    }

    pub fn is_class(&self, object: ObjectReference<'gc>) -> bool {
        self.header(object).format() == Format::Class
    }

    /// The words of a word-format object of `format`: the one lender of a
    /// pointer payload, private to the heap, so that every public accessor
    /// copies out (`docs/design/memory.md`, "The seam").
    ///
    /// Inlined, and the block found once for both the format check and the
    /// words: every field, slot, closure and class-field access runs it.
    /// Always inlined: once the space made `block` larger, LLVM outlined it,
    /// and the call alone measured 5-8% of a benchmark's samples.
    #[inline(always)]
    fn pointer_words(&self, object: ObjectReference<'gc>, format: Format) -> Option<&[u64]> {
        let block = self.block(object);
        (Header(block[HEADER]).format() == format).then(|| &block[FIELDS..])
    }

    /// `pointer_words`, for a write; always inlined for the same reason.
    #[inline(always)]
    fn pointer_words_mut(
        &mut self,
        object: ObjectReference<'gc>,
        format: Format,
    ) -> Option<&mut [u64]> {
        let block = self.block_mut(object);
        if Header(block[HEADER]).format() != format {
            return None;
        }
        Some(&mut block[FIELDS..])
    }

    /// The pointer word at `index` of a word-format object of `format`;
    /// `None` for any other object or past its words.
    ///
    /// Always inlined: every instance-field read and the class's builtin
    /// field a send re-reads on each cache hit come through here.
    #[inline(always)]
    fn pointer_word(
        &self,
        object: ObjectReference<'gc>,
        format: Format,
        index: usize,
    ) -> Option<Value<'gc>> {
        let bits = *self.pointer_words(object, format)?.get(index)?;
        Some(self.load(Word::from_bits(bits)))
    }

    /// Every pointer word of a word-format object of `format`, in order.
    fn pointer_values(
        &self,
        object: ObjectReference<'gc>,
        format: Format,
    ) -> Option<Vec<Value<'gc>>> {
        let words = self.pointer_words(object, format)?;
        Some(
            words
                .iter()
                .map(|bits| self.load(Word::from_bits(*bits)))
                .collect(),
        )
    }

    /// Answers whether the write happened: `false` for any other object or
    /// past its words.
    fn set_pointer_word(
        &mut self,
        object: ObjectReference<'gc>,
        format: Format,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        let word = self.store(value);
        match self
            .pointer_words_mut(object, format)
            .and_then(|words| words.get_mut(index))
        {
            Some(slot) => {
                *slot = word.bits();
                self.write_barrier(object.offset, word);
                true
            }
            None => false,
        }
    }

    pub fn array_element_count(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.pointer_words(object, Format::IndexablePointers)
            .map(<[u64]>::len)
    }

    /// The number of fields of an `ObjectShape::Instance`, mirroring
    /// `array_element_count`; `None` for any other shape.
    pub fn instance_field_count(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.pointer_words(object, Format::FixedPointers)
            .map(<[u64]>::len)
    }

    /// Every field of an `ObjectShape::Instance`, in order, mirroring
    /// `array_elements`; `None` for any other shape.
    pub fn instance_fields(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.pointer_values(object, Format::FixedPointers)
    }

    pub fn array_elements(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.pointer_values(object, Format::IndexablePointers)
    }

    /// The number of slots of an `ObjectShape::WeakArray`; `None` for any
    /// other shape.
    pub fn weak_array_size(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.pointer_words(object, Format::WeakPointers)
            .map(<[u64]>::len)
    }

    /// The slot of an `ObjectShape::WeakArray` at the 0-based `index`, nil
    /// once the collector reclaimed its object; `None` for any other shape
    /// or past its slots.
    pub fn weak_array_element(
        &self,
        object: ObjectReference<'gc>,
        index: usize,
    ) -> Option<Value<'gc>> {
        self.pointer_word(object, Format::WeakPointers, index)
    }

    /// Answers whether the write happened, mirroring `array_set_element`.
    /// Through the write barrier like every store: an old weak array naming
    /// a young object must be remembered, so that a scavenge forwards or
    /// clears the word.
    pub fn weak_array_set_element(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.set_pointer_word(object, Format::WeakPointers, index, value)
    }

    /// Every slot of an `ObjectShape::WeakArray`, in order; `None` for any
    /// other shape.
    pub fn weak_array_elements(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.pointer_values(object, Format::WeakPointers)
    }

    /// The field of an `ObjectShape::Ephemeron` at the 0-based `index` --
    /// 0 the key, 1 the value, 2 the finalizer -- nil once a collection
    /// mourned it; `None` for any other shape or past its fields.
    ///
    /// An ephemeron's fields are not instance slots: the slot accessors
    /// take `FixedPointers` alone, so that an ordinary object's slot pays
    /// nothing for the ephemeron's format, and `Ephemeron`'s accessors
    /// reach its fields through mirror leaves (`builtins/weak.rs`).
    pub fn ephemeron_field(
        &self,
        object: ObjectReference<'gc>,
        index: usize,
    ) -> Option<Value<'gc>> {
        self.pointer_word(object, Format::Ephemeron, index)
    }

    /// Answers whether the write happened, mirroring `instance_set_field`.
    /// Through the write barrier like every store: an old ephemeron naming
    /// a young object must be remembered, so that a scavenge forwards the
    /// word or mourns the ephemeron.
    pub fn ephemeron_set_field(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.set_pointer_word(object, Format::Ephemeron, index, value)
    }

    /// Every field of an `ObjectShape::Ephemeron`, the key first; `None`
    /// for any other shape.
    pub fn ephemeron_fields(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.pointer_values(object, Format::Ephemeron)
    }

    pub fn array_element(&self, object: ObjectReference<'gc>, index: usize) -> Option<Value<'gc>> {
        self.pointer_word(object, Format::IndexablePointers, index)
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set_element(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.set_pointer_word(object, Format::IndexablePointers, index, value)
    }

    /// The instance field at `index`, or `None` when `object` is no
    /// `ObjectShape::Instance` or `index` is out of range -- the same
    /// total-rather-than-panicking shape as the array accessors above.
    pub fn instance_field(&self, object: ObjectReference<'gc>, index: usize) -> Option<Value<'gc>> {
        self.pointer_word(object, Format::FixedPointers, index)
    }

    /// Answers whether the write happened, mirroring `array_set_element`.
    pub fn instance_set_field(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.set_pointer_word(object, Format::FixedPointers, index, value)
    }

    /// A String's text as UTF-8, for the boundaries that need a Rust
    /// `String`: an error text, the transcript, `asSymbol`, `printString`.
    /// The leaves that only read the elements use the accessors below,
    /// which copy out a size, a code point or a hash, not the whole text.
    pub fn str_value(&self, object: ObjectReference<'gc>) -> Option<String> {
        self.packed_string(object)
            .map(|string| string.to_code_points().to_utf8())
    }

    /// The one reader of a String's payload, private to the heap, so that
    /// every public accessor copies out (`docs/design/memory.md`, "The
    /// seam"). A Symbol is a String, so it answers one too.
    fn packed_string(&self, object: ObjectReference<'gc>) -> Option<PackedString<'_>> {
        PackedString::new(self.header(object), self.fields(object))
    }

    /// A String's code points, copied out; `None` for any other object.
    /// For the image encoder, which writes the whole text.
    pub fn string_code_points(&self, object: ObjectReference<'gc>) -> Option<crate::CodePoints> {
        self.packed_string(object).map(PackedString::to_code_points)
    }

    pub fn is_string(&self, object: ObjectReference<'gc>) -> bool {
        self.packed_string(object).is_some()
    }

    /// A String's number of code points; `None` for any other object.
    pub fn string_size(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.packed_string(object).map(PackedString::size)
    }

    /// The code point at the 0-based `index`; `None` for any other object
    /// or past the end.
    pub fn string_code_point_at(&self, object: ObjectReference<'gc>, index: usize) -> Option<u32> {
        self.packed_string(object)?.code_point_at(index)
    }

    /// The code points in the 0-based, half-open `start..end`; `None` for
    /// any other object or a range outside the string.
    pub fn string_slice(
        &self,
        object: ObjectReference<'gc>,
        start: usize,
        end: usize,
    ) -> Option<crate::CodePoints> {
        self.packed_string(object)?.slice(start, end)
    }

    /// `first`'s code points followed by `second`'s; `None` unless both are
    /// Strings.
    pub fn string_concatenation(
        &self,
        first: ObjectReference<'gc>,
        second: ObjectReference<'gc>,
    ) -> Option<crate::CodePoints> {
        let second = self.packed_string(second)?;
        Some(
            self.packed_string(first)?
                .to_code_points()
                .concatenate(&second.to_code_points()),
        )
    }

    /// Whether two Strings hold the same code points; `None` unless both
    /// are Strings.
    pub fn strings_equal(
        &self,
        first: ObjectReference<'gc>,
        second: ObjectReference<'gc>,
    ) -> Option<bool> {
        Some(
            self.packed_string(first)?
                .same_text(self.packed_string(second)?),
        )
    }

    /// A String's content hash; `None` for any other object. Hashes the
    /// units in place, as `CodePoints::content_hash` hashes the same text.
    pub fn string_hash(&self, object: ObjectReference<'gc>) -> Option<i64> {
        let string = self.packed_string(object)?;
        Some(crate::hash::content_hash_of_characters(
            string.code_points().map(|code_point| {
                // Every unit is a scalar value by construction; the
                // replacement character stands in for a panic.
                char::from_u32(code_point).unwrap_or(char::REPLACEMENT_CHARACTER)
            }),
        ))
    }

    /// The one reader of a `ByteArray`'s data, private to the heap: its data
    /// words, from the 32-byte address `aligned_data_start` computes from
    /// the object's place, and its size; `None` unless the format is
    /// `AlignedBytes`. Total for a block too short for its reserve, which
    /// only an untrusted image could build.
    fn byte_array_words(&self, object: ObjectReference<'gc>) -> Option<(&[u64], usize)> {
        let index = header_index(object.offset);
        let block = self.block(object);
        let header = Header(block[HEADER]);
        if header.format() != Format::AlignedBytes {
            return None;
        }
        let (start, data_words) = Self::byte_array_extent(index, block.len())?;
        let size = (data_words * 8).saturating_sub(usize::from(header.unused()));
        Some((&block[start..start + data_words], size))
    }

    /// `byte_array_words`, for a write.
    fn byte_array_words_mut(
        &mut self,
        object: ObjectReference<'gc>,
    ) -> Option<(&mut [u64], usize)> {
        let index = header_index(object.offset);
        let block = self.block_mut(object);
        let header = Header(block[HEADER]);
        if header.format() != Format::AlignedBytes {
            return None;
        }
        let (start, data_words) = Self::byte_array_extent(index, block.len())?;
        let size = (data_words * 8).saturating_sub(usize::from(header.unused()));
        Some((&mut block[start..start + data_words], size))
    }

    /// Where the data of the `AlignedBytes` block of `block_length` words
    /// whose header is word `header` begin within the block, and how many
    /// words they take.
    fn byte_array_extent(header: usize, block_length: usize) -> Option<(usize, usize)> {
        let data_words = block_length.checked_sub(FIELDS + object::ALIGNMENT_RESERVE)?;
        let data_start = object::aligned_data_start(header);
        debug_assert_eq!(data_start % object::ALIGNMENT_WORDS, 0);
        Some((data_start - header, data_words))
    }

    /// Lends a `ByteArray`'s data to `read` and answers what it answers;
    /// `None` for any other object.
    pub(crate) fn with_byte_array<T>(
        &self,
        object: ObjectReference<'gc>,
        read: impl FnOnce(ByteArrayData<'_>) -> T,
    ) -> Option<T> {
        let (words, size) = self.byte_array_words(object)?;
        Some(read(ByteArrayData::new(words, size)))
    }

    /// Lends a `ByteArray`'s data to `write` and answers what it answers;
    /// `None` for any other object.
    pub(crate) fn with_byte_array_mut<T>(
        &mut self,
        object: ObjectReference<'gc>,
        write: impl FnOnce(ByteArrayDataMut<'_>) -> T,
    ) -> Option<T> {
        let (words, size) = self.byte_array_words_mut(object)?;
        Some(write(ByteArrayDataMut::new(words, size)))
    }

    /// Whether `object` is a `ByteArray`: decided by its format, which no
    /// String shares.
    pub fn is_byte_array(&self, object: ObjectReference<'gc>) -> bool {
        self.header(object).format() == Format::AlignedBytes
    }

    /// A `ByteArray`'s number of bytes; `None` for any other object.
    pub fn byte_array_size(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.with_byte_array(object, |data| data.size())
    }

    /// A `ByteArray`'s bytes, copied out; `None` for any other object.
    pub fn byte_array_bytes(&self, object: ObjectReference<'gc>) -> Option<Vec<u8>> {
        self.with_byte_array(object, |data| data.bytes(0..data.size()))?
    }

    /// The byte at the 0-based `index`; `None` for any other object or
    /// past the end.
    pub fn byte_array_at(&self, object: ObjectReference<'gc>, index: usize) -> Option<u8> {
        self.with_byte_array(object, |data| data.byte(index))?
    }

    /// Writes `byte` at the 0-based `index`; answers whether the write
    /// happened, `false` for any other object or past the end.
    pub fn byte_array_set(&mut self, object: ObjectReference<'gc>, index: usize, byte: u8) -> bool {
        self.with_byte_array_mut(object, |mut data| data.set_byte(index, byte))
            .unwrap_or(false)
    }

    /// The element of `kind` whose first byte is at the 0-based `offset`,
    /// in the order `big_endian` names; `None` for any other object or
    /// when its bytes pass the end.
    pub fn byte_array_element(
        &self,
        object: ObjectReference<'gc>,
        offset: usize,
        kind: ElementKind,
        big_endian: bool,
    ) -> Option<Element> {
        self.with_byte_array(object, |data| data.read_element(offset, kind, big_endian))?
    }

    /// Writes `element` as `kind` with its first byte at the 0-based
    /// `offset`, in the order `big_endian` names; answers whether the write
    /// happened, `false` for any other object, when its bytes pass the end,
    /// or when the element is not of the kind's shape.
    pub fn set_byte_array_element(
        &mut self,
        object: ObjectReference<'gc>,
        offset: usize,
        kind: ElementKind,
        big_endian: bool,
        element: Element,
    ) -> bool {
        self.with_byte_array_mut(object, |mut data| {
            data.write_element(offset, kind, big_endian, element)
        })
        .unwrap_or(false)
    }

    /// A number object's words, its header and whether it is a boxed
    /// `Float`; `None` for any other object. A `LargeInteger` and a boxed
    /// `Float` share `Format::SixtyFourBit`, so the class's builtin tells
    /// them apart -- and only `Float`'s is asked for, so that a
    /// `LargeInteger` reads by its format whatever its class.
    fn number_words(&self, object: ObjectReference<'gc>) -> Option<(bool, Header, &[u64])> {
        let block = self.block(object);
        let header = Header(block[HEADER]);
        if header.format() != Format::SixtyFourBit {
            return None;
        }
        let float = self.class_builtin(self.block_class(block)) == Some(BuiltinClass::Float);
        Some((float, header, &block[FIELDS..]))
    }

    /// Any float, immediate or boxed; `None` for anything else. Reads the
    /// payload's format, never the class alone, so that `Float new`, an
    /// instance, is no number, as `LargeInteger new` is not.
    pub fn float_value(&self, value: Value<'gc>) -> Option<f64> {
        match value.kind() {
            ValueKind::SmallFloat(number) => Some(number),
            ValueKind::Object(object) => match self.number_words(object)? {
                (true, _, words) => words.first().copied().map(f64::from_bits),
                (false, ..) => None,
            },
            _ => None,
        }
    }

    /// Whether `object` is a boxed `Float`, a float outside the immediate
    /// range.
    pub fn is_boxed_float(&self, object: ObjectReference<'gc>) -> bool {
        matches!(self.number_words(object), Some((true, ..)))
    }

    /// Lends a `LargeInteger`'s sign and magnitude, least significant digit
    /// first, to `read` and answers what it answers; `None` for any other
    /// object. For a leaf that compares, hashes or tests the sign, so that
    /// it copies no digit.
    pub(crate) fn with_large_integer<T>(
        &self,
        object: ObjectReference<'gc>,
        read: impl FnOnce(bool, &[u64]) -> T,
    ) -> Option<T> {
        match self.number_words(object)? {
            (false, header, words) => Some(read(header.negative(), words)),
            (true, ..) => None,
        }
    }

    /// The integer a `LargeInteger` object holds; `None` for any other
    /// object. Copies out, as every accessor does (`docs/design/memory.md`,
    /// "The seam"), for a caller that keeps the integer or computes with
    /// it; `with_large_integer` reads it in place.
    pub fn large_integer(&self, object: ObjectReference<'gc>) -> Option<crate::ArbitraryInteger> {
        self.with_large_integer(object, crate::ArbitraryInteger::from_sign_and_digits)
    }

    /// A `Fraction`'s numerator and denominator, each a `SmallInteger` or
    /// a `LargeInteger`; `None` for any other object -- `Fraction new`,
    /// which has no field, and an instance of a program's subclass, whose
    /// class is no builtin.
    pub(crate) fn fraction_parts(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<(Value<'gc>, Value<'gc>)> {
        if self.class_builtin(self.object_class(object)) != Some(BuiltinClass::Fraction)
            || self.instance_field_count(object) != Some(2)
        {
            return None;
        }
        Some((
            self.instance_field(object, 0)?,
            self.instance_field(object, 1)?,
        ))
    }

    /// The mixin `object` stands for: `Some` only for an object of the
    /// kernel-private `Mixin` whose one field is a `SmallInteger` naming a
    /// row of the reified-mixin table that holds `object` itself. An object
    /// of another class, of another shape, or one the table does not hold
    /// answers `None`, so that nothing but the canonical object is ever
    /// read as a mixin.
    pub fn mixin_object_id(&self, object: ObjectReference<'gc>) -> Option<MixinId> {
        if self.class_builtin(self.object_class(object)) != Some(BuiltinClass::Mixin)
            || self.instance_field_count(object) != Some(1)
        {
            return None;
        }
        let index = self.instance_field(object, 0)?.as_small_integer()?;
        let id = MixinId::from_index(usize::try_from(index).ok()?)?;
        self.mixin_object(id)
            .filter(|recorded| recorded.same(object))
            .map(|_| id)
    }

    /// The object recorded as the mixin `id` names; `None` until
    /// `Mutator::reified_mixin` reifies it.
    pub(crate) fn mixin_object(&self, id: MixinId) -> Option<ObjectReference<'gc>> {
        let object = (*self.mixin_objects.get(id.index())?)?;
        self.check_object_origin(object);
        Some(object)
    }

    /// Records `object` as the one object the mixin `id` names. Recorded
    /// once: a second record for one id would give the mixin two
    /// identities.
    pub(crate) fn record_mixin_object(&mut self, id: MixinId, object: ObjectReference<'gc>) {
        self.check_object_origin(object);
        let index = id.index();
        if self.mixin_objects.len() <= index {
            self.mixin_objects.resize(index + 1, None);
        }
        debug_assert!(
            self.mixin_objects[index].is_none(),
            "internal: a mixin is reified once"
        );
        self.mixin_objects[index] = Some(object);
    }

    /// A `Fraction`'s value; `None` for any other object. Copies out.
    pub(crate) fn fraction(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<crate::numeric::Rational> {
        let (numerator, denominator) = self.fraction_parts(object)?;
        let integer = |value: Value<'gc>| match value.kind() {
            ValueKind::SmallInteger(integer) => Some(crate::ArbitraryInteger::from_i64(integer)),
            ValueKind::Object(part) => self.large_integer(part),
            _ => None,
        };
        Some(crate::numeric::Rational::from_normal_parts(
            integer(numerator)?,
            integer(denominator)?,
        ))
    }

    /// Whether `object` is a `LargeInteger`.
    pub fn is_large_integer(&self, object: ObjectReference<'gc>) -> bool {
        matches!(self.number_words(object), Some((false, ..)))
    }

    /// Whether `object` is a Symbol: a String whose class is the builtin
    /// `Symbol`. Reads no text, for a caller that needs no interned id.
    pub fn is_symbol(&self, object: ObjectReference<'gc>) -> bool {
        self.is_string(object)
            && self.class_builtin(self.object_class(object)) == Some(BuiltinClass::Symbol)
    }

    /// The interned symbol a Symbol object stands for; `None` for any other
    /// object, a String of the same text included. Agrees with `is_symbol`,
    /// because every Symbol object is canonical and so recorded in
    /// `symbol_of`; it reads neither the object's text nor its class.
    #[inline]
    pub fn symbol_value(&self, object: ObjectReference<'gc>) -> Option<Symbol> {
        self.check_object_origin(object);
        let answer = self.symbol_of.get(&object.offset.get()).copied();
        debug_assert_eq!(
            answer.is_some(),
            self.is_symbol(object),
            "`symbol_of` must record exactly the Symbol objects, or a symbol \
             answers its text as a String's and a String answers a symbol"
        );
        answer
    }

    /// The one object `symbol` already is in this heap, if anything has
    /// asked for it yet (§5.1.6).
    pub(crate) fn canonical_symbol(&self, symbol: Symbol) -> Option<ObjectReference<'gc>> {
        self.canonical_symbols
            .get(symbol.index())
            .copied()
            .flatten()
    }

    /// Records `object` as the one object `symbol` is in this heap, from
    /// now on.
    pub(crate) fn record_canonical_symbol(&mut self, symbol: Symbol, object: ObjectReference<'gc>) {
        self.check_object_origin(object);
        let index = symbol.index();
        if self.canonical_symbols.len() <= index {
            self.canonical_symbols.resize(index + 1, None);
        }
        if let Some(previous) = self.canonical_symbols[index].replace(object) {
            self.symbol_of.remove(&previous.offset.get());
        }
        self.symbol_of.insert(object.offset.get(), symbol);
    }

    /// Closure `object`'s words -- its fixed fields, then its copied values --
    /// or `None` when `object` is not a closure. Private, like every borrow
    /// of the space: the accessors below each copy one thing out of it, so
    /// that none of their callers holds a borrow of the heap.
    fn closure_words(&self, object: ObjectReference<'gc>) -> Option<&[u64]> {
        let words = self.pointer_words(object, Format::Closure)?;
        (words.len() >= closure_field::COUNT).then_some(words)
    }

    /// A closure's fixed field holding a count or an index, as one.
    fn closure_count_field(&self, words: &[u64], field: usize) -> Option<usize> {
        self.load(Word::from_bits(words[field]))
            .as_small_integer()
            .and_then(|count| usize::try_from(count).ok())
    }

    /// The body closure `object` runs, or `None` when `object` is not a
    /// closure or its code id names no entry of the table.
    fn closure_body(&self, object: ObjectReference<'gc>) -> Option<Rc<CompiledCode>> {
        let words = self.closure_words(object)?;
        let id = CodeId::from_index(self.closure_count_field(words, closure_field::CODE_ID)?)?;
        self.code(id)
    }

    /// Whether `object` is a closure -- what a control-flow step asks of a
    /// value before activating it as a block.
    pub fn is_closure(&self, object: ObjectReference<'gc>) -> bool {
        self.closure_words(object).is_some()
    }

    /// How many parameters closure `object` declares, or `None` when `object`
    /// is not a closure. Read off the body rather than a parameter-name list
    /// of the closure's own: one source, so the arity a call is checked
    /// against and the arity the register file is sized from cannot disagree.
    pub fn closure_parameter_count(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.closure_body(object)
            .map(|code| code.argument_count as usize)
    }

    /// Closure `object`'s compiled body, or `None` when `object` is not a
    /// closure. `image::decode::frame_body` resolves a closure frame's body by
    /// it.
    pub fn closure_code(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<std::rc::Rc<crate::bytecode::CompiledCode>> {
        self.closure_body(object)
    }

    /// What activating closure `object` needs, read in one borrow: its body,
    /// whose `argument_count` is the parameter count a call is checked
    /// against, the frame it captured, whose receiver and current class the
    /// activation takes, and its feedback base, `FeedbackBase::NOT_ASSIGNED`
    /// for a closure an image decoded rather than one `MakeClosure` built.
    /// `None` when `object` is not a closure.
    pub(crate) fn closure_activation(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<(
        std::rc::Rc<crate::bytecode::CompiledCode>,
        FrameReference<'gc>,
        FeedbackBase,
    )> {
        let words = self.closure_words(object)?;
        let code = self.closure_body(object)?;
        let captured_frame = self
            .load(Word::from_bits(words[closure_field::CAPTURED_FRAME]))
            .as_object()
            .and_then(|frame| self.object_frame(frame))?;
        let feedback_base = FeedbackBase::from_raw(
            u32::try_from(self.closure_count_field(words, closure_field::FEEDBACK_BASE)?).ok()?,
        );
        Some((code, captured_frame, feedback_base))
    }

    /// Writes closure `object`'s copied values into `activation`'s registers,
    /// immediately above its header, which is where a block's compiled
    /// code addresses them (`docs/design/execution.md`, "Converted
    /// representation"). Does nothing for a non-closure, and stops at the
    /// end of the register file: `decode`'s `CopiedCountOutOfRange` keeps a
    /// decoded closure's list inside its body's registers, and this is what
    /// keeps a future gap there from becoming a panic on an untrusted image.
    ///
    /// `Step::Block` runs this on every block activation, so it allocates
    /// nothing: the space and the stack are borrowed field by field rather
    /// than through `closure`, which borrows the whole `Heap`, so each value
    /// goes straight from the object into its register with no intermediate
    /// buffer; and it is inline into that push, so that it costs no call.
    #[inline(always)]
    pub(crate) fn fill_copied_values(
        &mut self,
        activation: ActivationReference,
        object: ObjectReference<'gc>,
    ) {
        self.check_activation(activation);
        self.check_object_origin(object);
        let Heap { space, stack, .. } = self;
        let space_words = space.words();
        let block = &space_words[block_range(space_words, header_index(object.offset))];
        let words = &block[FIELDS..];
        if Header(block[HEADER]).format() != Format::Closure || words.len() <= closure_field::COUNT
        {
            return;
        }
        let copied = &words[closure_field::COUNT..];
        // The first register above the header, `argument_count +
        // frame_header::COUNT`.
        let first = activation.offset as usize + frame_header::COUNT;
        // The activation's own window, so a list longer than its register
        // file stops at the end of it rather than writing into whatever
        // sits above.
        let end = stack.window_end(activation.offset);
        let stack_words = stack.frames.words_mut();
        for (position, bits) in copied.iter().enumerate() {
            let slot = first + position;
            if slot >= end {
                break;
            }
            stack_words[slot] = *bits;
        }
    }

    /// Gives a decoded closure -- the only one still carrying
    /// `FeedbackBase::NOT_ASSIGNED` once it has been activated -- the fresh
    /// range `Mutator::push_step` minted for it, so every later activation of
    /// the same closure object reuses it instead of minting again. On a
    /// non-closure it does nothing: `image::decode`'s reconciliation is what
    /// guarantees the caller's object is a closure, not this accessor.
    pub(crate) fn closure_set_feedback_base(
        &mut self,
        object: ObjectReference<'gc>,
        feedback_base: FeedbackBase,
    ) {
        if let Some(words) = self.pointer_words_mut(object, Format::Closure) {
            if words.len() >= closure_field::COUNT {
                words[closure_field::FEEDBACK_BASE] =
                    count_word(feedback_base.raw() as usize).bits();
            }
        }
    }

    /// Closure `object`'s captured frame, or `None` when `object` is not a
    /// closure: what a closure activation's kind reaches through. A frame
    /// kind decoded from an image names an object the decoder has confirmed is
    /// a closure (`NotAClosureReference`), but answering `None` rather than
    /// panicking keeps a future change there from turning an untrusted image
    /// into a panic here.
    pub fn closure_captured_frame(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<FrameReference<'gc>> {
        let words = self.closure_words(object)?;
        self.load(Word::from_bits(words[closure_field::CAPTURED_FRAME]))
            .as_object()
            .and_then(|frame| self.object_frame(frame))
    }

    /// Closure `object`'s copied values, in order, or `None` when `object`
    /// is not a closure.
    pub fn closure_copied_values(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.closure_words(object).map(|words| {
            words[closure_field::COUNT..]
                .iter()
                .map(|bits| self.load(Word::from_bits(*bits)))
                .collect()
        })
    }

    /// A closure's home: the activation a `^` in its body returns from.
    /// Derived rather than stored: the captured frame itself when it is a
    /// method's, an initializer's or a clause's, its own `home` when it is a
    /// block's.
    ///
    /// A closure frame always carries a `home`: the runtime asserts it when
    /// it reifies one (`allocate_frame`), and the decoder refuses a closure
    /// frame without one (`DecodeError::FrameHomeMismatch`). The fallback
    /// below is therefore a defence, not a path: answering the captured frame
    /// keeps a frame this heap holds without a home from panicking here, and
    /// a `^` aimed at a frame that is not running is the `cannotReturn`
    /// error `raise_return` already raises for a dead home.
    pub fn closure_home(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>> {
        let captured = self.closure_captured_frame(object)?;
        if !self.frame_runs_a_closure(captured) {
            return Some(captured);
        }
        Some(self.frame_home(captured).unwrap_or(captured))
    }

    /// Renders a value for a user-facing error message.
    ///
    /// Never `Debug`-format a `Value` instead: `ObjectReference`'s hand-written
    /// `Debug` does not recurse, but it prints Rust internals such as
    /// `ObjectReference(#42)`, which tells a Newspeak programmer nothing. Every
    /// caller of this is building the message of a `NewspeakError`, which
    /// reaches stderr, and that message needs to name what the value *is*
    /// -- "the String \"foo\"", not its Rust representation.
    pub fn describe(&self, value: Value<'gc>) -> String {
        match value.kind() {
            ValueKind::Nil => "nil".to_string(),
            ValueKind::Boolean(boolean) => boolean.to_string(),
            ValueKind::SmallInteger(integer) => integer.to_string(),
            ValueKind::SmallFloat(float) => crate::numeric::float_text::shortest(float),
            ValueKind::Object(object) => {
                // The text `Fraction>>printString` answers, as every other
                // number describes itself by its value.
                if let Some((numerator, denominator)) = self.fraction_parts(object) {
                    return format!(
                        "({}/{})",
                        self.describe(numerator),
                        self.describe(denominator)
                    );
                }
                // A reified mixin is named by the mixin it stands for, as a
                // class is: its one field is an id, which tells a reader
                // nothing.
                if let Some(mixin) = self.mixin_object_id(object).and_then(|id| self.mixin(id)) {
                    return format!("the mixin {}", mixin.reflected_name());
                }
                match self.header(object).format() {
                    _ if self.is_string(object) => match self.symbol_value(object) {
                        Some(symbol) => format!("the Symbol #{symbol}"),
                        None => format!(
                            "the String {:?}",
                            self.str_value(object).unwrap_or_default()
                        ),
                    },
                    Format::IndexablePointers => {
                        format!("an Array of {}", self.fields(object).len())
                    }
                    Format::WeakPointers => "a WeakArray".to_string(),
                    Format::Ephemeron => "an Ephemeron".to_string(),
                    Format::SixtyFourBit => {
                        match self.float_value(value) {
                            // As an immediate float describes: which of the
                            // two a float is, is no business of an error
                            // message.
                            Some(float) => crate::numeric::float_text::shortest(float),
                            // Bare digits, as a `SmallInteger` describes, for
                            // the same reason.
                            None => self
                                .large_integer(object)
                                .map(|integer| integer.to_decimal_string())
                                .unwrap_or_default(),
                        }
                    }
                    Format::AlignedBytes => format!(
                        "a ByteArray of {}",
                        self.byte_array_size(object).unwrap_or_default()
                    ),
                    Format::Frame => "an Activation".to_string(),
                    Format::Closure => "a Closure".to_string(),
                    Format::Class => format!("the class {}", self.class_display_name(object)),
                    _ => {
                        // Named as its class is (`class_name`): an instance
                        // of a body-less declaration's class is one of it.
                        match self.class_name(self.object_class(object)) {
                            Some(name) => format!("an instance of {name}"),
                            // `ClassReference`'s doc: every class word
                            // names a class object by convention rather than by
                            // the type system. This arm exists only so the
                            // match is total, not because a conforming
                            // object can reach it.
                            None => "an object".to_string(),
                        }
                    }
                }
            }
        }
    }

    /// The `Option` form every one of these error constructors needs, so the
    /// `map_or` is not written out three times.
    pub fn describe_argument(&self, got: Option<Value<'gc>>) -> String {
        match got {
            Some(value) => self.describe(value),
            None => "no argument".to_string(),
        }
    }

    // ---------- Host roots ----------

    /// Roots `value` for a host. A value a host keeps past the next run of
    /// the loop must be a `Root`: a run may move every object
    /// (`docs/design/memory.md`, "Where a collection can happen"), and only
    /// what the collector traces is rewritten.
    pub fn stash(&self, value: Value<'gc>) -> Root {
        if let Some(object) = value.as_object() {
            self.check_object_origin(object);
        }
        let slot = std::rc::Rc::new(std::cell::Cell::new(at_rest(value)));
        let mut roots = self.host_roots.borrow_mut();
        if roots.slots.len() >= roots.sweep_at {
            roots.sweep();
        }
        roots.slots.push(slot.clone());
        Root { slot }
    }

    /// What `root` holds now. The answer is good until the next run of the
    /// loop, which may move it; read the root again after one.
    pub fn fetch(&self, root: &Root) -> Value<'gc> {
        let value = branded(root.slot.get());
        if let Some(object) = value.as_object() {
            self.check_object_origin(object);
        }
        value
    }

    /// How many slots the root table holds, those nothing else holds any
    /// more but that no sweep has reached yet included.
    pub fn host_root_count(&self) -> usize {
        self.host_roots.borrow().slots.len()
    }

    // ---------- The Rust-side tables ----------

    /// The id of `mixin`, any version of it, minted on first sight. A
    /// mixin's row names the row of the mixin it is declared in, so the
    /// outermost enclosing mixin not registered yet is registered, and with
    /// it its whole declared tree, `mixin` included; walked, not recursed,
    /// because an image chooses how deep its declarations nest.
    pub(crate) fn mixin_id(&mut self, mixin: &Rc<Mixin>) -> MixinId {
        if let Some(id) = self.known_mixin_id(mixin) {
            return id;
        }
        let mut unregistered = vec![mixin.clone()];
        while let Some(parent) = unregistered
            .last()
            .and_then(|innermost| innermost.enclosing_mixin())
        {
            if self.known_mixin_id(&parent).is_some() {
                break;
            }
            unregistered.push(parent);
        }
        for each in unregistered.into_iter().rev() {
            if self.known_mixin_id(&each).is_none() {
                let enclosing = each
                    .enclosing_mixin()
                    .and_then(|parent| self.known_mixin_id(&parent));
                self.register_mixin(&each, enclosing);
            }
        }
        self.known_mixin_id(mixin)
            .expect("internal: registering its outermost unregistered ancestor registered it")
    }

    /// Mints the next id for `mixin`, declared in the mixin `enclosing`
    /// names, and one for every declaration nested in it, each declared in
    /// its parent's (`register_nested`).
    fn register_mixin(&mut self, mixin: &Rc<Mixin>, enclosing: Option<MixinId>) -> MixinId {
        let id = self.push_mixin_row(mixin, enclosing);
        self.register_nested(id, mixin.take_declared_nested());
        id
    }

    /// Gives the mixin `id` the ids of `children`, its nested declarations
    /// in declaration order, and registers every one this heap does not
    /// name yet with its own subtree, declared in its parent's.
    /// Registration takes each child's children out of the tree, so no
    /// registered mixin keeps an unregistered child. The one child the heap
    /// may already name is one an install paired with an earlier version,
    /// registered under the id it keeps with this same `id` as its
    /// enclosing mixin; a child registered under another parent is refused.
    fn register_nested(&mut self, id: MixinId, children: Vec<Rc<Mixin>>) {
        // Walked, not recursed: an image chooses how deep its declarations nest.
        let mut pending = vec![(id, children)];
        while let Some((parent, children)) = pending.pop() {
            let mut ids = Vec::with_capacity(children.len());
            for child in children {
                let child_id = match self.known_mixin_id(&child) {
                    Some(child_id) => {
                        // Rows form trees, which a collection frees whole:
                        // a child named under a second parent would keep
                        // a row of another tree, freed without it.
                        debug_assert_eq!(
                            self.enclosing_mixin_id(child_id),
                            Some(parent),
                            "internal: a known child is declared in the mixin it is nested in"
                        );
                        child_id
                    }
                    None => {
                        let child_id = self.push_mixin_row(&child, Some(parent));
                        pending.push((child_id, child.take_declared_nested()));
                        child_id
                    }
                };
                ids.push(child_id);
            }
            self.mixin_row_mut(parent).nested = ids;
        }
    }

    /// One row for `mixin`, declared in `enclosing`'s, with no nested ids
    /// yet: the smallest free row, else a new one.
    fn push_mixin_row(&mut self, mixin: &Rc<Mixin>, enclosing: Option<MixinId>) -> MixinId {
        let row = match self.tables.free_mixins.take_smallest() {
            Some(free) => free as usize,
            None => {
                self.tables.mixins.push(None);
                self.tables.mixins.len() - 1
            }
        };
        let id = MixinId::from_index(row).expect("more than u32::MAX mixins in one heap");
        self.tables.mixins[row] = Some(MixinRow {
            current: Rc::clone(mixin),
            enclosing,
            nested: Vec::new(),
        });
        self.tables
            .mixin_ids
            .insert(crate::mixin::mixin_key(mixin), id);
        id
    }

    /// The ids of the declarations nested directly in the mixin `id`, in
    /// declaration order, the same for every version of it: what every
    /// reader of a registered mixin's nested declarations reads, through
    /// `mixin` for each one's current version. Empty for an id past the
    /// table or freed.
    pub fn nested_of(&self, id: MixinId) -> &[MixinId] {
        self.mixin_row(id).map_or(&[], |row| row.nested.as_slice())
    }

    /// The row of the mixin `id`; `None` for an id past the table or freed.
    fn mixin_row(&self, id: MixinId) -> Option<&MixinRow> {
        self.tables.mixins.get(id.index())?.as_ref()
    }

    /// The row of a registered mixin. A compiler-invariant violation
    /// panics: an id with no row, which nothing registered or a
    /// collection freed while something still named it.
    fn mixin_row_mut(&mut self, id: MixinId) -> &mut MixinRow {
        self.tables
            .mixins
            .get_mut(id.index())
            .and_then(Option::as_mut)
            .expect("internal: a registered mixin's id names a row")
    }

    /// The current versions of the declarations nested directly in
    /// `mixin`, in declaration order: through its id (`nested_of`) once
    /// this heap registered it, whatever version `mixin` is, and its
    /// declared tree (`Mixin::declared_nested`) while nothing has.
    pub fn nested_mixins(&self, mixin: &Rc<Mixin>) -> Vec<Rc<Mixin>> {
        match self.known_mixin_id(mixin) {
            Some(id) => self
                .nested_of(id)
                .iter()
                // A collection frees a module's tree whole, so a mixin
                // with a row never names a freed child.
                .map(|nested_id| {
                    self.mixin(*nested_id)
                        .expect("internal: a nested id has a row")
                })
                .collect(),
            None => mixin.declared_nested().to_vec(),
        }
    }

    /// Gives the mixin `id`'s current version its nested row: each child
    /// the install replaced keeps the id it was paired with, and a child
    /// with no counterpart is registered fresh, with its subtree, declared
    /// in `id`.
    pub(crate) fn adopt_nested(&mut self, id: MixinId, children: Vec<Rc<Mixin>>) {
        self.register_nested(id, children);
    }

    /// Whether `candidate` is `mixin` or another version of the mixin `id`
    /// (`mixin`'s own, as `known_mixin_id` answered it) names: an edit
    /// replaces a mixin under its id, and a level applying the old version
    /// is still a level of the declaration. The reference compare is the
    /// fast case and the only one an unregistered `mixin` has.
    pub(crate) fn same_mixin(
        &self,
        candidate: &Rc<Mixin>,
        mixin: &Rc<Mixin>,
        id: Option<MixinId>,
    ) -> bool {
        Rc::ptr_eq(candidate, mixin)
            || id.is_some_and(|id| self.known_mixin_id(candidate) == Some(id))
    }

    /// The id `mixin`, any version of it, was registered under; `None`
    /// while nothing has registered it -- a mixin no class object applies
    /// and nothing has reified.
    pub fn known_mixin_id(&self, mixin: &Rc<Mixin>) -> Option<MixinId> {
        self.tables
            .mixin_ids
            .get(&crate::mixin::mixin_key(mixin))
            .copied()
    }

    /// The current version of the mixin `id` names; `None` for an id
    /// past the table or freed.
    pub fn mixin(&self, id: MixinId) -> Option<Rc<Mixin>> {
        self.mixin_row(id).map(|row| row.current.clone())
    }

    /// The id of the mixin the mixin `id` is declared in: a nested
    /// declaration's enclosing declaration, a class side's instance side.
    /// `None` for a module's root, a builtin's mixin, and an id past the
    /// table or freed.
    pub(crate) fn enclosing_mixin_id(&self, id: MixinId) -> Option<MixinId> {
        self.mixin_row(id)?.enclosing
    }

    /// `mixin`'s text with every hole filled by the assembled text of the
    /// current version of the nested declaration it stands for
    /// (`nested_mixins`). For a module's root, the module's current text,
    /// every edit of a declaration in it included. `None` where a
    /// declaration of the tree has no text. A class side shares its
    /// instance side's text but declares no nested mixin, so for one it
    /// answers that text only when the instance side has no nested
    /// declaration, and `None` otherwise: a reader of a class side's
    /// declaration asks for its instance side's (the mirrors' `source` goes
    /// through `instance_side_id`).
    ///
    /// Recursive along the declarations' nesting, which the parser bounds
    /// for a module read from source and `image::mixins::MAX_MIXIN_DEPTH`
    /// for one decoded from an image.
    pub(crate) fn assembled_text(&self, mixin: &Rc<Mixin>) -> Option<String> {
        let text = mixin.text.as_ref()?;
        let nested = self
            .nested_mixins(mixin)
            .iter()
            .map(|nested| self.assembled_text(nested))
            .collect::<Option<Vec<String>>>()?;
        text.assemble(&nested)
    }

    /// The text of the declaration of the mixin `id`, an instance side,
    /// from its access modifier (or `class`) to its closing parenthesis,
    /// as its module's current text holds it: `assembled_text` sliced by
    /// the declaration's span, which for a module's root cuts the prologue
    /// and the trailing text. The span counts in the text the version was
    /// read in, and an edit nested in it changes only what lies between
    /// its ends, so its start counts from the assembled text's start and
    /// its end from the assembled text's end. `None` where the version has
    /// no text or its span does not lie within it.
    pub(crate) fn declaration_source(&self, id: MixinId) -> Option<String> {
        let mixin = self.mixin(id)?;
        let range = mixin.text.as_ref()?.range();
        let assembled = self.assembled_text(&mixin)?;
        let start = usize::try_from(mixin.span.start.checked_sub(range.start)?).ok()?;
        let trailing = usize::try_from(range.end.checked_sub(mixin.span.end)?).ok()?;
        let end = assembled.len().checked_sub(trailing)?;
        assembled.get(start..end).map(str::to_string)
    }

    /// The current text of the module whose root mixin `module_root`
    /// names (`assembled_text`); `None` for a module with no text.
    pub(crate) fn module_source(&self, module_root: MixinId) -> Option<String> {
        self.assembled_text(&self.mixin(module_root)?)
    }

    /// Makes `replacement`, which holds its own text, the current version
    /// of the mixin `id`: from here on `replacement`'s address answers `id`,
    /// as every earlier version's still does, and the id keeps the
    /// enclosing mixin it was registered with. What was built from an
    /// earlier version keeps it (`Mutator::replace_mixin`).
    ///
    /// A compiler-invariant violation panics: a `replacement` that already
    /// has an id, whose address would then answer two; a shipped current
    /// version, which is the binary's code and which the VM mirror's locks
    /// rest on; and a class side replacing an instance side or the other
    /// way round.
    ///
    /// Answers the replacement's declared nested declarations, in order,
    /// taken out of it: the id's nested row is the earlier version's until
    /// `adopt_nested` gives it the replacement's, once every replacement of
    /// an install is in place, so that a child paired with an old one
    /// keeps its id.
    pub(crate) fn replace_mixin(&mut self, id: MixinId, replacement: Rc<Mixin>) -> Vec<Rc<Mixin>> {
        assert!(
            self.known_mixin_id(&replacement).is_none(),
            "internal: a replacement mixin is registered under no id yet"
        );
        let current = &self.mixin_row_mut(id).current;
        assert!(
            !current.shipped,
            "internal: the shipped mixin {} is never replaced",
            current.name
        );
        assert_eq!(
            current.is_class_side(),
            replacement.is_class_side(),
            "internal: a class side is replaced by a class side, an instance side by an \
             instance side"
        );
        self.tables
            .mixin_ids
            .insert(crate::mixin::mixin_key(&replacement), id);
        let children = replacement.take_declared_nested();
        let superseded = std::mem::replace(&mut self.mixin_row_mut(id).current, replacement);
        self.tables.superseded.push(superseded);
        children
    }

    /// Enters `application` into the table, stamped with its index, and
    /// answers the shared handle every class object of it will name.
    pub(crate) fn register_application(
        &mut self,
        mut application: MixinApplication,
    ) -> Rc<MixinApplication> {
        let row = match self.tables.free_applications.take_smallest() {
            Some(free) => free as usize,
            None => {
                self.tables.applications.push(None);
                self.tables.applications.len() - 1
            }
        };
        let index =
            ApplicationIndex::from_index(row).expect("more than u32::MAX applications in one heap");
        application.index = index;
        application.instance_header =
            InstanceHeader::for_slots(application.dispatch_ids.instances, application.slot_count);
        // The facts come from the literal alone, which every caller computes
        // from the same mixin; a debug build refuses one that does not, so that
        // no instantiation reads facts of another mixin than the one it runs.
        debug_assert_eq!(
            application.level_facts,
            crate::mixin::LevelFacts::of(&application.mixin),
            "the level facts of {} disagree with its mixin",
            application.mixin.name
        );
        let application = Rc::new(application);
        self.tables.applications[row] = Some(application.clone());
        self.record_application_row(row, Self::application_row(Some(&application)));
        application
    }

    /// What the application id table records of `application`, or of a row
    /// that holds none (`heap/application_ids.rs`).
    fn application_row(application: Option<&MixinApplication>) -> application_ids::Row {
        application.map_or(
            application_ids::Row {
                dispatch_id: DispatchId::NONE,
                instance_header: 0,
                slot_base: 0,
                value_slots: false,
            },
            |application| application_ids::Row {
                dispatch_id: application.dispatch_ids.instances,
                instance_header: match (
                    &application.instantiation_refusal,
                    application.instance_header,
                ) {
                    (None, Some(header)) => header.0,
                    _ => 0,
                },
                slot_base: application.own_slot_base,
                value_slots: application.inherits_value,
            },
        )
    }

    /// Writes `recorded` as row `row` of the application id table machine
    /// code reads (`heap/application_ids.rs`), and its bound, the
    /// application table's length.
    pub(crate) fn record_application_row(&mut self, row: usize, recorded: application_ids::Row) {
        self.application_ids
            .record(row, recorded, self.tables.applications.len());
        self.machine_words.set(
            MachineWord::ApplicationIdRows,
            self.application_ids.rows() as u64,
        );
    }

    /// The zeros of a row that holds no application, which
    /// `Heap::free_application` records.
    pub(crate) fn vacant_application_row() -> application_ids::Row {
        Self::application_row(None)
    }

    /// The application id table's bound taken to the application table's
    /// length, after its trailing free rows were given back.
    pub(crate) fn record_application_rows(&mut self) {
        self.application_ids
            .set_rows(self.tables.applications.len());
        self.machine_words.set(
            MachineWord::ApplicationIdRows,
            self.application_ids.rows() as u64,
        );
    }

    /// Whether the application id table records, for every row of the
    /// application table and nothing past it, that row's instances id, own
    /// slot base and flags -- zeros for a free row -- and the machine words
    /// its address and bound.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn verify_application_ids(&self) -> Result<(), String> {
        let rows = self.tables.applications.len();
        if self.application_ids.rows() != rows
            || self.machine_words.get(MachineWord::ApplicationIdRows) != rows as u64
            || self.machine_words.get(MachineWord::ApplicationIds)
                != self.application_ids.address() as u64
        {
            return Err(format!("the table's bound is not the {rows} rows"));
        }
        for (row, application) in self.tables.applications.iter().enumerate() {
            let expected = application_ids::ApplicationIds::expected_words(Self::application_row(
                application.as_deref(),
            ));
            if self.application_ids.row_words(row) != Some(expected) {
                return Err(format!(
                    "row {row} records {:?}, its application's row is {expected:?}",
                    self.application_ids.row_words(row)
                ));
            }
        }
        if self.application_ids.row_words(rows).is_some() {
            return Err("the table records a row past its bound".to_owned());
        }
        Ok(())
    }

    /// Records `new` as the application that took `old`'s place and
    /// repoints every application whose successor was `old` at `new`, so a
    /// chain of edits answers its newest level in one lookup.
    pub(crate) fn record_successor(&mut self, old: ApplicationIndex, new: ApplicationIndex) {
        for successor in self.tables.successors.values_mut() {
            if *successor == old {
                *successor = new;
            }
        }
        self.tables.successors.insert(old, new);
    }

    /// The newest application that took `level`'s place, if an install
    /// replaced it.
    pub(crate) fn successor(&self, level: ApplicationIndex) -> Option<ApplicationIndex> {
        self.tables.successors.get(&level).copied()
    }

    pub fn application(&self, index: ApplicationIndex) -> Option<Rc<MixinApplication>> {
        self.tables.applications.get(index.index())?.clone()
    }

    /// Every version of a mixin an install superseded, oldest first: the
    /// code they carry still runs in the frames and closures that hold it,
    /// so an image writes the bodies of those its graph reaches.
    pub(crate) fn superseded_mixins(&self) -> &[Rc<Mixin>] {
        &self.tables.superseded
    }

    #[inline]
    pub(crate) fn code_id(&mut self, code: &Rc<CompiledCode>) -> CodeId {
        self.register_code(code, None)
    }

    #[inline]
    pub(crate) fn code_id_of_method(&mut self, method: &Rc<MixinMethod>) -> CodeId {
        self.register_code(&method.code, Some(method))
    }

    /// The id of `code`, registered with `method` when one is given. Inline
    /// up to the cache's hit, since every activation's push asks it
    /// (`push_activation`); a miss takes the map out of line.
    #[inline(always)]
    fn register_code(
        &mut self,
        code: &Rc<CompiledCode>,
        method: Option<&Rc<MixinMethod>>,
    ) -> CodeId {
        if let Some((epoch, id)) = code.code_id_cache.get() {
            if epoch == self.code_table_epoch {
                // An epoch is renewed whenever rows are freed, so a cached
                // id of the current epoch still names its own body.
                debug_assert!(
                    self.tables
                        .codes
                        .get(id.index())
                        .and_then(Option::as_ref)
                        .is_some_and(|entry| Rc::ptr_eq(&entry.code, code)),
                    "internal: a body's cached id names another row: rows were freed without renewing the epoch"
                );
                if let Some(method) = method {
                    if let Some(entry) = self.tables.codes[id.index()].as_mut() {
                        if entry.method.is_none() {
                            entry.method = Some(method.clone());
                        }
                    }
                }
                return id;
            }
        }
        self.register_code_uncached(code, method)
    }

    /// `register_code` past its cache: the map, or a new row.
    #[cold]
    #[inline(never)]
    fn register_code_uncached(
        &mut self,
        code: &Rc<CompiledCode>,
        method: Option<&Rc<MixinMethod>>,
    ) -> CodeId {
        let key = Rc::as_ptr(code) as usize;
        if let Some(id) = self.tables.code_ids.get(&key).copied() {
            let entry = self.tables.codes[id.index()]
                .as_mut()
                .expect("internal: a body's id names a row while the map names the body");
            if entry.method.is_none() {
                entry.method = method.cloned();
            }
            code.code_id_cache.set(self.code_table_epoch, id);
            return id;
        }
        let row = match self.tables.free_codes.take_smallest() {
            Some(free) => free as usize,
            None => {
                self.tables.codes.push(None);
                self.tables.codes.len() - 1
            }
        };
        let id = CodeId::from_index(row).expect("more than u32::MAX compiled bodies in one heap");
        self.tables.codes[row] = Some(CodeEntry {
            code: code.clone(),
            method: method.cloned(),
        });
        self.tables.code_ids.insert(key, id);
        code.code_id_cache.set(self.code_table_epoch, id);
        id
    }

    /// The body `id` names; `None` for an id past the table or freed.
    pub fn code(&self, id: CodeId) -> Option<Rc<CompiledCode>> {
        self.tables
            .codes
            .get(id.index())?
            .as_ref()
            .map(|entry| entry.code.clone())
    }

    /// The method whose body `id` names; `None` for a block's body, and an
    /// id past the table or freed.
    pub fn code_method(&self, id: CodeId) -> Option<Rc<MixinMethod>> {
        self.tables
            .codes
            .get(id.index())?
            .as_ref()
            .and_then(|entry| entry.method.clone())
    }

    // ---------- Class accessors ----------
    //
    // Each answers `None` for a handle whose data is not a class, rather
    // than panicking: a class reference decoded from an image is untrusted
    // until verified, and the verifier is a caller of these.

    /// Field `field` of class object `class` (`object.rs`'s `class_field`
    /// gives the layout); `None` for a handle that is no class object.
    fn class_field(&self, class: ClassReference<'gc>, field: usize) -> Option<Value<'gc>> {
        self.pointer_word(class, Format::Class, field)
    }

    /// A class object's field that holds an object or nil, as an `Option`;
    /// `None` too for a handle that is no class object.
    fn class_object_field(
        &self,
        class: ClassReference<'gc>,
        field: usize,
    ) -> Option<ObjectReference<'gc>> {
        self.class_field(class, field)?.as_object()
    }

    /// Writes field `field` of class object `class`, answering whether the
    /// write happened: `false` when `class` is no class object.
    fn set_class_field(
        &mut self,
        class: ClassReference<'gc>,
        field: usize,
        value: Value<'gc>,
    ) -> bool {
        self.set_pointer_word(class, Format::Class, field, value)
    }

    /// The table entry `class`'s application field names, borrowed rather
    /// than cloned: the readers every send, slot write and allocation asks
    /// go through this, and a refcount bump each time is what they avoid.
    /// `None` for a handle that is no class object.
    fn class_application_entry(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<&Rc<crate::mixin::MixinApplication>> {
        let index = self
            .class_field(class, class_field::APPLICATION)?
            .as_small_integer()?;
        self.tables
            .applications
            .get(usize::try_from(index).ok()?)?
            .as_ref()
    }

    /// Runs `read` over the application of the first level of `class`'s
    /// superclass chain, `class` included, that applies the mixin of
    /// `mixin_of`: a subclass's instance finds there the slots that mixin
    /// declares, at the level's own slot base, whatever slots other levels
    /// add -- a `<:` clause's level as well as the class itself. Matched by
    /// mixin, not by class object, and compared by reference first, then by
    /// id (`same_mixin`), so the walk clones no application and an
    /// application of an edited mixin's old version matches one of its new
    /// version. `None` when no level applies it, or `mixin_of` is no class
    /// object.
    pub(crate) fn with_level_applying_mixin_of<T>(
        &self,
        class: ClassReference<'gc>,
        mixin_of: ClassReference<'gc>,
        read: impl FnOnce(&crate::mixin::MixinApplication) -> T,
    ) -> Option<T> {
        let mixin = &self.class_application_entry(mixin_of)?.mixin;
        let id = self.known_mixin_id(mixin);
        let mut level = Some(class);
        while let Some(current) = level {
            if let Some(application) = self.class_application_entry(current) {
                if self.same_mixin(&application.mixin, mixin, id) {
                    return Some(read(application));
                }
            }
            level = self.class_superclass(current);
        }
        None
    }

    /// The applying declaration, the clause's position in it and the
    /// builtin it named, unpacked from `class`'s applied-clause field and
    /// borrowed from the mixin table; `None` for a class object no mixin
    /// application added and for a handle that is no class object.
    fn class_applied_parts(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<(&Rc<Mixin>, usize, Option<BuiltinClass>)> {
        let packed = self
            .class_field(class, class_field::APPLIED_CLAUSE)?
            .as_small_integer()?;
        let (mixin, index, named_builtin) = crate::mixin::AppliedClause::unpack(packed)?;
        let declaration = &self.mixin_row(mixin)?.current;
        Some((declaration, index, named_builtin))
    }

    /// What instantiation asks of the class object `class` as a level --
    /// its application, the `<:` clause that added it, its superclass --
    /// read with one look at its words, where `class_application_entry`,
    /// `class_applied_parts` and `class_superclass` would each look again.
    /// Every answer is theirs; for a handle that is no class object, every
    /// part is empty, as each of theirs is.
    pub(crate) fn class_level(&self, class: ClassReference<'gc>) -> ClassLevel<'_, 'gc> {
        let Some(words) = self.pointer_words(class, Format::Class) else {
            return ClassLevel {
                application: None,
                applied: None,
                superclass: None,
            };
        };
        let application = self.application_in_words(words);
        let applied = self
            .class_word(words, class_field::APPLIED_CLAUSE)
            .and_then(Value::as_small_integer)
            .and_then(crate::mixin::AppliedClause::unpack)
            .and_then(|(mixin, index, _)| Some((&self.mixin_row(mixin)?.current, index)));
        let superclass = self
            .class_word(words, class_field::SUPERCLASS)
            .and_then(Value::as_object);
        ClassLevel {
            application,
            applied,
            superclass,
        }
    }

    /// Field `index` of a class object whose words are `words`.
    #[inline(always)]
    fn class_word(&self, words: &[u64], index: usize) -> Option<Value<'gc>> {
        words
            .get(index)
            .map(|bits| self.load(Word::from_bits(*bits)))
    }

    /// The table entry the application field of a class object whose words
    /// are `words` names (`class_application_entry`).
    #[inline(always)]
    fn application_in_words(&self, words: &[u64]) -> Option<&Rc<MixinApplication>> {
        self.class_word(words, class_field::APPLICATION)
            .and_then(Value::as_small_integer)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.tables.applications.get(index)?.as_ref())
    }

    /// One accessor for the behaviour half of a class object, so a
    /// construction site that needs a superclass's application never has to
    /// re-derive the borrow-and-match itself.
    pub fn class_application(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<std::rc::Rc<crate::mixin::MixinApplication>> {
        self.class_application_entry(class).cloned()
    }

    /// Instantiation's facts of `class`'s level, copied out of its
    /// application without cloning it; `None` for a class object with no
    /// application.
    pub(crate) fn class_level_facts(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::mixin::LevelFacts> {
        self.class_application_entry(class)
            .map(|application| application.level_facts)
    }

    /// The index of `class`'s application in the table: the identity of
    /// its behaviour, which two class objects share exactly when they share
    /// their application. `None` for a handle that is no class object.
    pub fn class_application_index(&self, class: ClassReference<'gc>) -> Option<ApplicationIndex> {
        let index = self
            .class_field(class, class_field::APPLICATION)?
            .as_small_integer()?;
        ApplicationIndex::from_index(usize::try_from(index).ok()?)
    }

    /// The word `class`'s application field holds, as machine code reads
    /// it from the class object: the guard a `super` or private lexical
    /// site's cell compares (`inline_cache::cells::Cell`). `None` for a
    /// handle that is no class object.
    pub(crate) fn class_application_word(&self, class: ClassReference<'gc>) -> Option<u64> {
        self.pointer_words(class, Format::Class)?
            .get(class_field::APPLICATION)
            .copied()
    }

    /// The instances id of the registered application whose class objects
    /// hold `word` in their application field (`class_application_word`);
    /// `None` for a word no class object of an application holds. For
    /// `InlineCaches::verify_cells`, which holds a `super` or private
    /// lexical cell's guard to its entry's.
    pub(crate) fn application_dispatch_id_for_word(&self, word: u64) -> Option<DispatchId> {
        let word = Word::from_bits(word);
        if !word.is_small_integer() {
            return None;
        }
        let index = usize::try_from(self.load(word).as_small_integer()?).ok()?;
        let application = self.tables.applications.get(index)?.as_ref()?;
        (count_word(index).bits() == word.bits()).then_some(application.dispatch_ids.instances)
    }

    /// §3.1.1's "its class inherits from class `Value`", read off the
    /// application without cloning the `Rc` that holds it: this is asked of
    /// every slot an initializer writes, and going through
    /// `class_application` would pay a refcount bump per slot for one
    /// `bool`. That bump is what this accessor exists to avoid.
    pub fn class_inherits_value(&self, class: ClassReference<'gc>) -> bool {
        self.class_application_entry(class)
            .is_some_and(|application| application.inherits_value)
    }

    /// Runs `read` over the field indices of the slots `class` and its
    /// superclasses **declare** (`MixinApplication::declared_slots`), for
    /// the same reason `class_inherits_value` reads its `bool` here: the
    /// derivation asks this once per slot on its declining path, and
    /// `class_application` would pay a refcount bump each time.
    ///
    /// A closure rather than a returned `&[usize]`, so the borrow of the
    /// space ends here and nothing reaches past an accessor ("The seam",
    /// `docs/design/memory.md`). `None` when `class` is no class object.
    pub fn with_class_declared_slots<T>(
        &self,
        class: ClassReference<'gc>,
        read: impl FnOnce(&[usize]) -> T,
    ) -> Option<T> {
        self.class_application_entry(class)
            .map(|application| read(&application.declared_slots))
    }

    /// Whether `class`'s application applies a mixin compiled from shipped
    /// core-library source (`Mixin.shipped`); `false` for an object that is
    /// not a class object. Read without cloning the application's `Rc`.
    pub fn class_is_shipped(&self, class: ClassReference<'gc>) -> bool {
        self.class_application_entry(class)
            .is_some_and(|application| application.mixin.shipped)
    }

    pub fn class_enclosing(&self, class: ClassReference<'gc>) -> Option<ObjectReference<'gc>> {
        self.class_object_field(class, class_field::ENCLOSING)
    }

    pub fn class_declaring(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.class_object_field(class, class_field::DECLARING_CLASS)
    }

    pub fn class_superclass(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.class_object_field(class, class_field::SUPERCLASS)
    }

    /// Whether `class` is a level a mixin application added: exactly when
    /// `class_applied_clause` answers a clause, without copying it out.
    /// Instantiation asks this of every level it initializes or passes, so an
    /// ordinary level costs one test of an object it reads anyway.
    pub fn class_is_applied_level(&self, class: ClassReference<'gc>) -> bool {
        self.class_applied_parts(class)
            .is_some_and(|(declaration, index, _)| index < declaration.mixin_applications.len())
    }

    /// The `<:` clause a level a mixin application added was built for
    /// (`class_field::APPLIED_CLAUSE`); `None` for any other class object.
    pub fn class_applied_clause(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::mixin::MixinApplicationClause> {
        let (declaration, index, _) = self.class_applied_parts(class)?;
        declaration.mixin_applications.get(index).cloned()
    }

    /// Which `<:` clause added `class` -- the applying declaration and the
    /// clause's position in it -- for the image encoder, which names a level's
    /// clause that way; `None` for a class object no mixin application added.
    pub fn class_applied_origin(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::mixin::AppliedClause> {
        let (declaration, index, named_builtin) = self.class_applied_parts(class)?;
        Some(crate::mixin::AppliedClause {
            declaration: declaration.clone(),
            index,
            named_builtin,
        })
    }

    /// Whether `class` carries a recorded `ValueClassRefusal`, without
    /// reading it: `allocate_for_factory` asks this on every instantiation
    /// of a value class, and only the cold error path reads the refusal
    /// itself (`class_value_refusal`).
    pub fn class_has_value_refusal(&self, class: ClassReference<'gc>) -> bool {
        self.class_object_field(class, class_field::VALUE_CLASS_REFUSAL)
            .is_some()
    }

    /// §3.1.1's enclosing-object condition on a value class, decided when
    /// `class` was created and read here by `allocate_for_factory`'s cold
    /// error path and by `check_value_class`'s walk --
    /// `ValueClassRefusal`'s doc comment says why creation and refusal are
    /// two different moments. `None` for a class that is no value, for a
    /// value class whose enclosing objects are all values, and for a field
    /// not shaped as `Mutator::record_value_class_refusal` writes it.
    pub fn class_value_refusal(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::types::ValueClassRefusal<'gc>> {
        let record = self.class_object_field(class, class_field::VALUE_CLASS_REFUSAL)?;
        let offender = self.array_element(record, 0)?.as_object()?;
        let level = match self.array_element(record, 1)?.as_object() {
            Some(name) => Some(self.symbol_value(name)?),
            None => None,
        };
        Some(crate::types::ValueClassRefusal::NonValueEnclosingObject { level, offender })
    }

    /// The core-library class `class` is, read afresh on every send by an
    /// inline cache's `act` (`docs/design/dispatch.md`, "3b -- The
    /// caches"); `None` for a user-defined class and for a handle that is
    /// no class object.
    pub fn class_builtin(&self, class: ClassReference<'gc>) -> Option<BuiltinClass> {
        let position = self
            .class_field(class, class_field::BUILTIN)?
            .as_small_integer()?;
        BuiltinClass::ALL
            .get(usize::try_from(position).ok()?)
            .copied()
    }

    /// A lookup level's superclass, or `None` when `class` is not a class
    /// object -- which is what ends a `walk` that stepped onto one through
    /// `next_level`'s unverified class-object chain.
    pub fn class_lookup_step(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<Option<ClassReference<'gc>>> {
        self.class_field(class, class_field::SUPERCLASS)
            .map(Value::as_object)
    }

    /// `class`'s metaclass: its header class, when that class's
    /// `this_class` is `class` (`newspeak-spec.pdf` §3.2, "Each class is an
    /// instance of a unique metaclass"). `None` for a handle that is no
    /// class object, for a metaclass -- whose class is `Metaclass` -- and
    /// for a class object of the kernel phase before the bootstrap gave it
    /// its metaclass (`Mutator::give_existing_class_objects_metaclasses`).
    pub fn class_metaclass(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        if !self.is_class(class) {
            return None;
        }
        let header = self.object_class(class);
        self.class_this_class(header)
            .filter(|this_class| this_class.same(class))
            .map(|_| header)
    }

    /// A metaclass's class (`class_field::THIS_CLASS`); `None` for a class
    /// object that is no metaclass and for a handle that is no class object.
    pub fn class_this_class(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.class_object_field(class, class_field::THIS_CLASS)
    }

    /// The name a class object is shown by: its own (`class_name`) for a
    /// class, and its class's followed by ` class` for a metaclass, as
    /// `Metaclass>>name` answers it (`Kernel.ns`). For the refusal texts
    /// that name what could not be instantiated.
    pub fn class_display_name(&self, class: ClassReference<'gc>) -> String {
        match self.class_this_class(class) {
            Some(this_class) => format!("{} class", self.class_display_name(this_class)),
            None => self
                .class_name(class)
                .map_or_else(|| "a class".to_string(), |name| name.to_string()),
        }
    }

    /// For a level a `<:` clause added, the class object the clause named
    /// (`class_field::NAMED_CLASS`); `None` for every other class object and
    /// for a handle that is no class object.
    pub fn class_named(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.class_object_field(class, class_field::NAMED_CLASS)
    }

    /// The declaration `class` is the class of: a body-less declaration's
    /// for its class object (`class_body_less_declaration`), which applies
    /// its last clause's mixin, and its application's mixin for every
    /// other class object. `None` for a handle that is no class object.
    /// What a module's root is held to and described by in an image.
    pub fn class_declaration(&self, class: ClassReference<'gc>) -> Option<Rc<Mixin>> {
        match self.class_body_less_declaration(class) {
            Some(declaration) => Some(declaration.clone()),
            None => self
                .class_application_entry(class)
                .map(|application| application.mixin.clone()),
        }
    }

    /// Whether `first` and `second` are the class objects of one body-less
    /// declaration, or neither is a body-less declaration's. Two body-less
    /// declarations applying one mixin with one superclass share one
    /// application, yet are two classes, as their body versions, two
    /// applications, are: what value equality and `Behavior`'s
    /// `sharesApplicationWith:` ask beside the application.
    pub(crate) fn classes_share_body_less_declaration(
        &self,
        first: ClassReference<'gc>,
        second: ClassReference<'gc>,
    ) -> bool {
        // Told by size first, one header read each: only a body-less
        // declaration's class object has the declaration's scope, and only
        // two of them need their declarations looked up.
        match (
            self.class_has_declaration_scope(first),
            self.class_has_declaration_scope(second),
        ) {
            (false, false) => true,
            (true, true) => self
                .class_body_less_declaration(first)
                .zip(self.class_body_less_declaration(second))
                .is_some_and(|(first_declaration, second_declaration)| {
                    Rc::ptr_eq(first_declaration, second_declaration)
                }),
            _ => false,
        }
    }

    /// The enclosing object reflection reports for `class` (a mirror's
    /// `enclosingObject`, `Behavior`'s): §6 gives the class of a
    /// declaration "an enclosing object which is eo", so a body-less
    /// declaration's class object -- and its metaclass, which shares its
    /// class's -- answers its declaration's (`class_declaration_scope`), nil
    /// for a module's root, where `class_enclosing` answers its level's,
    /// which the applied mixin's code walks out from. Every other class
    /// object answers `class_enclosing`.
    pub fn class_reflected_enclosing(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<ObjectReference<'gc>> {
        let instance_side = self.class_this_class(class).unwrap_or(class);
        match self.class_declaration_scope(instance_side) {
            Some(scope) => scope.enclosing,
            None => self.class_enclosing(class),
        }
    }

    /// Whether `class` is a body-less declaration's class object, the one
    /// class object with its declaration's scope
    /// (`class_declaration_scope`), told by its size alone, one read of
    /// the class object's header.
    #[inline(always)]
    pub(crate) fn class_has_declaration_scope(&self, class: ClassReference<'gc>) -> bool {
        self.pointer_words(class, Format::Class)
            .is_some_and(|words| words.len() == class_field::BODY_LESS_CLASS_COUNT)
    }

    /// For a body-less declaration's class object
    /// (`class_body_less_declaration`), the declaration's own enclosing
    /// object and declaring class (`class_field::DECLARATION_ENCLOSING`),
    /// which its own code walks out from where every other code at the
    /// class object walks out from the named class's
    /// (`interpreter::sends::lexical_origin`); `None` for every other class
    /// object and for a handle that is no class object.
    pub fn class_declaration_scope(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<DeclarationScope<'gc>> {
        if !self.class_has_declaration_scope(class) {
            return None;
        }
        Some(DeclarationScope {
            enclosing: self.class_object_field(class, class_field::DECLARATION_ENCLOSING),
            declaring_class: self
                .class_object_field(class, class_field::DECLARATION_DECLARING_CLASS),
        })
    }

    /// Writes a body-less declaration's scope into its class object, through
    /// the write barrier: the declaration's enclosing object and declaring
    /// class are known only once its superclass clause has run, after the
    /// class object was built (`nested::after_clause`). Answers whether the
    /// write happened: `false` for a class object without the two fields
    /// and for a handle that is no class object.
    pub(crate) fn class_set_declaration_scope(
        &mut self,
        class: ClassReference<'gc>,
        scope: DeclarationScope<'gc>,
    ) -> bool {
        if !self.class_has_declaration_scope(class) {
            return false;
        }
        let DeclarationScope {
            enclosing,
            declaring_class,
        } = scope;
        self.set_class_field(
            class,
            class_field::DECLARATION_ENCLOSING,
            enclosing.map_or(Value::nil(), Value::object),
        ) && self.set_class_field(
            class,
            class_field::DECLARATION_DECLARING_CLASS,
            declaring_class.map_or(Value::nil(), Value::object),
        )
    }

    /// The mixin of `class`'s metaclass: its class side together with its
    /// primary factory (§6.3), which says which factory and which name the
    /// class object has. `None` for a class object without a metaclass --
    /// one of the kernel phase before the bootstrap gave it one, a
    /// metaclass -- and for a handle that is no class object.
    fn class_side_entry(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<&Rc<crate::mixin::MixinApplication>> {
        self.class_application_entry(self.class_metaclass(class)?)
    }

    /// `class`'s primary factory, as `newspeak-spec.pdf` §6.3 makes it: the
    /// factory entry of its metaclass's class-side mixin, its selector and
    /// arity. That is the declaration's own for a class with a body and for
    /// a body-less declaration's class object (`object-model.md`, "Mixin
    /// application"), and the named class's for a level a `<:` clause
    /// added, which §6.2 gives "a primary factory method with the same
    /// selector as m". `new` with no arguments for a builtin. A class object
    /// with no metaclass yet has its mixin's; `None` for a metaclass and
    /// for a handle that is no class object.
    ///
    /// Every instantiation's §6.1 check asks it, so each class object's
    /// words are looked at once: `class`'s, then its header class's, which
    /// is its metaclass when that class's `THIS_CLASS` is `class`
    /// (`class_metaclass`).
    pub fn class_factory(&self, class: ClassReference<'gc>) -> Option<crate::mixin::Factory> {
        let words = self.pointer_words(class, Format::Class)?;
        if self
            .class_word(words, class_field::THIS_CLASS)
            .and_then(Value::as_object)
            .is_some()
        {
            return None;
        }
        let header = self.object_class(class);
        let class_side = self
            .pointer_words(header, Format::Class)
            .filter(|header_words| {
                self.class_word(header_words, class_field::THIS_CLASS)
                    .and_then(Value::as_object)
                    .is_some_and(|this_class| this_class.same(class))
            })
            .and_then(|header_words| self.application_in_words(header_words));
        match class_side {
            Some(class_side) => class_side.level_facts.metaclass_factory,
            None => self.application_in_words(words).map(|application| {
                application.mixin.factory.unwrap_or(crate::mixin::Factory {
                    selector: Symbol::NEW,
                    argument_count: 0,
                })
            }),
        }
    }

    /// `class`'s name as its declaration writes it, read through its
    /// metaclass as its factory is (`class_factory`): the instance side
    /// the class side stands for. A level a `<:` clause added is named
    /// for the class its clause named, and a body-less declaration's class
    /// object for the declaration. A class object with no metaclass yet is
    /// named for its mixin; `None` for a metaclass and for a handle that is
    /// no class object.
    pub fn class_name(&self, class: ClassReference<'gc>) -> Option<Symbol> {
        if self.class_this_class(class).is_some() || !self.is_class(class) {
            return None;
        }
        let instance_side = self
            .class_side_entry(class)
            .and_then(|class_side| self.known_mixin_id(&class_side.mixin))
            .and_then(|id| self.enclosing_mixin_id(id))
            .and_then(|id| self.mixin_row(id));
        match instance_side {
            Some(row) => Some(row.current.name),
            None => self
                .class_application_entry(class)
                .map(|application| application.mixin.name),
        }
    }

    /// The mixin that carries the initializer `body` an activation entered
    /// at `level` runs: the body-less declaration's, for the full body of
    /// its class object (`class_body_less_declaration`), whose mixin is the
    /// one its last clause applies; `level`'s own mixin otherwise. `None`
    /// for a handle that is no class object.
    pub(crate) fn initializer_mixin(
        &self,
        level: ClassReference<'gc>,
        body: InitializerBody,
    ) -> Option<Rc<Mixin>> {
        if body == InitializerBody::Full {
            if let Some(declaration) = self.class_body_less_declaration(level) {
                return Some(declaration.clone());
            }
        }
        self.class_application(level)
            .map(|application| application.mixin.clone())
    }

    /// The body-less declaration (`Mixin::has_body`) `class` is the class
    /// object of: the declaration whose last `<:` clause added `class`.
    /// `None` for every other class object, a level a body-less
    /// declaration's earlier clause added included.
    pub fn class_body_less_declaration(&self, class: ClassReference<'gc>) -> Option<&Rc<Mixin>> {
        let (declaration, index, _) = self.class_applied_parts(class)?;
        (!declaration.has_body && index + 1 == declaration.mixin_applications.len())
            .then_some(declaration)
    }

    /// The id `dispatch_id` answers for an object of `class`: its class's
    /// instances id, a class object's included -- its class is its
    /// metaclass, so its id is that metaclass's instances id -- and `NONE`
    /// when the class is not a class object, which only a crafted image
    /// makes. Read off the application every allocation of an instance
    /// asks, without cloning its `Rc`.
    fn header_dispatch_id(&self, class: ClassReference<'gc>) -> DispatchId {
        self.class_application_entry(class)
            .map_or(DispatchId::NONE, |application| {
                application.dispatch_ids.instances
            })
    }

    /// The dispatch id an inline cache guards `value`'s send on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an object's is in its
    /// header -- one load from the space -- and an immediate's in a table.
    /// Nothing outside this file knows where the id lives.
    #[inline(always)]
    pub fn dispatch_id(&self, value: Value<'gc>) -> DispatchId {
        let Some(object) = value.as_object() else {
            return self.immediate_dispatch_ids[value.immediate_index()];
        };
        let header = self.header(object);
        debug_assert_eq!(
            header.guard(),
            self.header_dispatch_id(self.object_class(object)),
            "an object's dispatch id went stale: something changed its class's \
             application without restamping it"
        );
        header.guard()
    }

    /// `dispatch_id` of an object, read from its header word alone, without
    /// `header`'s block bounds: the one compare of a private or super slot
    /// accessor's guard check (`Mutator::slot_level_holds`).
    #[inline(always)]
    pub(crate) fn object_dispatch_id(&self, object: ObjectReference<'gc>) -> DispatchId {
        self.check_object_origin(object);
        let header = Header(self.space.words()[header_index(object.offset)]);
        debug_assert!(
            !matches!(header.format(), Format::Free | Format::Forwarded),
            "a handle names vacated nursery memory: a store missed the write barrier"
        );
        debug_assert_eq!(
            header.guard(),
            self.header_dispatch_id(self.object_class(object)),
            "an object's dispatch id went stale: something changed its class's \
             application without restamping it"
        );
        header.guard()
    }

    /// `class`'s ids, or `NONE` when it is not a class object: the key of a
    /// `super` send and of a private lexical send.
    pub fn class_dispatch_ids(&self, class: ClassReference<'gc>) -> DispatchIds {
        self.class_application_entry(class)
            .map_or(DispatchIds::NONE, |application| application.dispatch_ids)
    }

    /// Records the dispatch ids of the immediates, here and as the
    /// `ImmediateGuard` words machine code compares a receiver's against.
    pub(crate) fn set_immediate_dispatch_ids(&mut self, ids: [DispatchId; 5]) {
        self.immediate_dispatch_ids = ids;
        for (index, id) in ids.into_iter().enumerate() {
            self.machine_words
                .set(MachineWord::ImmediateGuard(index), u64::from(id.bits()));
        }
    }

    /// Records the class objects of the immediates, indexed as
    /// `immediate_dispatch_ids` is, as the `ImmediateClass` words machine
    /// code hands a send's miss path. A stored word is a handle, so the
    /// step state calls this again after every scavenge, compaction and
    /// move (`Mutator::rewrite_machine_classes`).
    pub(crate) fn set_immediate_classes(&self, classes: [ClassReference<'gc>; 5]) {
        for (index, class) in classes.into_iter().enumerate() {
            let word = self.store(Value::object(class));
            self.machine_words
                .set(MachineWord::ImmediateClass(index), word.bits());
        }
    }

    /// Records the kernel's `Array` class as the `ArrayClass` word, and
    /// the header of an `Array` of no words, guarding on its application,
    /// as the `ArrayHeader` word: what an allocation of an indirection
    /// vector in machine code writes. A stored word is a handle, so the
    /// step state calls this again whenever it rewrites the immediates'
    /// classes (`Mutator::rewrite_machine_classes`).
    pub(crate) fn set_array_class(&self, class: ClassReference<'gc>) {
        let word = self.store(Value::object(class));
        self.machine_words.set(MachineWord::ArrayClass, word.bits());
        let header = Header::new(
            self.header_dispatch_id(class),
            Format::IndexablePointers,
            0,
            0,
        );
        self.machine_words.set(MachineWord::ArrayHeader, header.0);
    }

    /// Records the kernel's `Closure` class and the `Activation` class
    /// every frame object is an instance of (`set_activation_class`) as
    /// the `ClosureClass` and `FrameClass` words, and the header of an
    /// object of no words of each, guarding on its application, as the
    /// `ClosureHeader` and `FrameHeader` words: what `MakeClosure` writes
    /// in machine code, as `allocate_closure` and `allocate_frame` write
    /// them. Called with `set_array_class`, for the same reason; nothing
    /// is recorded for a class the bootstrap has not built yet.
    pub(crate) fn set_reification_classes(&self, closure: ClassReference<'gc>) {
        let classes = [
            (
                Some(closure),
                Format::Closure,
                MachineWord::ClosureClass,
                MachineWord::ClosureHeader,
            ),
            (
                self.activation_class,
                Format::Frame,
                MachineWord::FrameClass,
                MachineWord::FrameHeader,
            ),
        ];
        for (class, format, class_word, header_word) in classes {
            let Some(class) = class else {
                continue;
            };
            let word = self.store(Value::object(class));
            self.machine_words.set(class_word, word.bits());
            let header = Header::new(self.header_dispatch_id(class), format, 0, 0);
            self.machine_words.set(header_word, header.0);
        }
    }

    /// Records the kernel-private `FutureImplementation` class as the
    /// `FutureClass` word, and the header of a future of no words,
    /// guarding on its application, as the `FutureHeader` word once
    /// `made` -- the heap has made a future (`Mutator::futures_made`) --
    /// and 0 before: what `ResolveFuture` compares a value's class word
    /// with and `MakeFuture` writes in machine code, as
    /// `Mutator::new_future` writes them. The header stays 0 until then
    /// so that machine code never makes the first future, which must set
    /// the flag every check for a future reads. Called with
    /// `set_array_class`, for the same reason, and once the first future
    /// is made (`Mutator::note_futures_made`).
    pub(crate) fn set_future_class(&self, class: ClassReference<'gc>, made: bool) {
        let word = self.store(Value::object(class));
        self.machine_words
            .set(MachineWord::FutureClass, word.bits());
        let header = if made {
            Header::new(self.header_dispatch_id(class), Format::FixedPointers, 0, 0).0
        } else {
            0
        };
        self.machine_words.set(MachineWord::FutureHeader, header);
    }

    /// The epoch of the code table, renewed whenever a full collection
    /// frees a code id (`free_unnamed_codes`): a code id recorded in an
    /// epoch names its body while the epoch lasts.
    #[cfg(ns_jit)]
    pub(crate) fn code_table_epoch(&self) -> u64 {
        self.code_table_epoch
    }

    /// The address of the machine entry table's row 0
    /// (`machine_entries.rs`), fixed for the heap's life: what the entry
    /// block's `BlockWord::MachineEntries` names.
    #[cfg(ns_jit)]
    pub(crate) fn machine_entries_address(&self) -> usize {
        self.machine_entries.address()
    }

    /// Where machine code reads a block body's row of the machine entry
    /// table (`machine_entries.rs`): how far an id is shifted to its row's
    /// offset, and each word's displacement from the row.
    #[cfg(ns_jit)]
    pub(crate) fn machine_entry_layout() -> MachineEntryLayout {
        use machine_entries::RowWord;
        MachineEntryLayout {
            row_shift: machine_entries::ROW_SHIFT,
            entry_displacement: RowWord::Entry.displacement(),
            feedback_base_displacement: RowWord::FeedbackBase.displacement(),
            parameters_displacement: RowWord::Parameters.displacement(),
        }
    }

    /// Records that the block body `id` names enters machine code at
    /// `entry`, compiled against `feedback_base`, and takes `parameters`
    /// parameters (`jit::note_block_entry`): the row is written only when
    /// it says something else, and the bound grows with it.
    #[cfg(ns_jit)]
    pub(crate) fn record_machine_entry(
        &mut self,
        id: CodeId,
        entry: u64,
        feedback_base: FeedbackBase,
        parameters: u16,
    ) {
        let words = [
            entry,
            count_word(feedback_base.raw() as usize).bits(),
            u64::from(parameters),
            0,
        ];
        if self.machine_entries.row(id.index()) == words {
            return;
        }
        if self.machine_entries.record(id.index(), words) {
            self.machine_words.set(
                MachineWord::MachineEntryRows,
                self.machine_entries.rows() as u64,
            );
        }
    }

    /// Clears the machine entry `code`'s id names, if it has one: its
    /// machine code has been abandoned (`jit::abandon`).
    #[cfg(ns_jit)]
    pub(crate) fn forget_machine_entry(&mut self, code: &Rc<CompiledCode>) {
        let key = Rc::as_ptr(code) as usize;
        if let Some(id) = self.tables.code_ids.get(&key).copied() {
            self.machine_entries.clear(id.index());
        }
    }

    /// Clears every machine entry: the code zone they lie in was replaced
    /// (`Interpreter::set_jit_zone_bytes`).
    pub(crate) fn forget_machine_entries(&mut self) {
        #[cfg(ns_jit)]
        self.machine_entries.clear_all();
    }

    /// Closure `object`'s code id, or `None` when `object` is not a
    /// closure.
    #[cfg(ns_jit)]
    pub(crate) fn closure_code_id(&self, object: ObjectReference<'gc>) -> Option<CodeId> {
        let words = self.closure_words(object)?;
        CodeId::from_index(self.closure_count_field(words, closure_field::CODE_ID)?)
    }

    /// The `SmallInteger` word of `count`, as an object's count fields --
    /// a closure's or a frame object's code id, a closure's feedback base
    /// -- hold it (`closure_fixed_words`, `frame_fixed_words`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_count_word(count: usize) -> u64 {
        count_word(count).bits()
    }

    /// Moves the depth count by `delta`: what `jit::called_closure` takes
    /// off before it calls an exempt block's body, whose prologue counts
    /// it as every compiled push is counted, and gives back once nothing
    /// stands in its place.
    #[cfg(ns_jit)]
    pub(crate) fn adjust_counted_depth(&self, delta: i64) {
        let count = self.machine_words.get(MachineWord::DepthCount);
        self.machine_words
            .set(MachineWord::DepthCount, count.wrapping_add_signed(delta));
    }

    /// Sets the depth limit's word: the limit with the reserve free, or
    /// past the reserve while it is held (`Mutator::set_depth_reserve`).
    pub(crate) fn set_depth_limit(&self, limit: usize) {
        self.machine_words
            .set(MachineWord::DepthLimit, limit as u64);
    }

    /// The word of the machine words `word` names, for a test.
    #[cfg(test)]
    pub(crate) fn machine_word(&self, word: MachineWord) -> u64 {
        self.machine_words.get(word)
    }

    /// The frame stack's ordinary end, for a test.
    #[cfg(test)]
    pub(crate) fn ordinary_stack_end(&self) -> usize {
        self.stack.frames.ordinary_end()
    }

    /// Makes every push of the run loop's seat the ordinary end at the
    /// stack's new top, the pushed frame's window end, for a test of what
    /// machine code does at that edge (`frames::FrameStack`).
    #[cfg(test)]
    pub(crate) fn set_seat_ordinary_end_at_top(&mut self, seat: bool) {
        self.stack.frames.set_seat_ordinary_end_at_top(seat);
    }

    /// How many times a word right above a seated ordinary end was found
    /// written before Rust next pushed or popped: a store machine code made
    /// past the end (`set_seat_ordinary_end_at_top`).
    #[cfg(test)]
    pub(crate) fn seat_disturbances(&mut self) -> u64 {
        self.stack.frames.seat_disturbances()
    }

    /// The guards a closure's and a frame's headers carry, read off the
    /// `Closure` and `Activation` classes, so a fast bump writes them
    /// without looking them up (`Mutator::refresh_immediate_dispatch_ids`,
    /// once when the builtin classes are built and again when the kernel
    /// gives them their applications).
    pub(crate) fn set_builtin_guards(&mut self, guards: BuiltinGuards) {
        self.builtin_guards = guards;
    }

    /// Makes `class` `object`'s class and restamps `object`'s dispatch id
    /// from it, so a cache guard never reads an id its header no longer
    /// answers. Answers whether the write happened -- `false` when `class`
    /// is no class object -- mirroring `class_set_superclass` and
    /// `class_set_application`. `Mutator::attach_metaclass` is the one
    /// caller: a class object's header becomes its metaclass, and the
    /// metaclass's `Metaclass` (`newspeak-spec.pdf` §3.2); the caller turns
    /// a `false` into an `unreachable!` rather than swallowing it.
    pub(crate) fn object_set_class(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
    ) -> bool {
        self.check_object_origin(object);
        self.check_object_origin(class);
        if !self.is_class(class) {
            return false;
        }
        let dispatch_id = self.header_dispatch_id(class);
        let class_word = self.store(Value::object(class));
        let header = self.header(object).with_guard(dispatch_id);
        let block = self.block_mut(object);
        block[CLASS] = class_word.bits();
        block[HEADER] = header.0;
        self.write_barrier(object.offset, class_word);
        true
    }

    /// Links a metaclass to its class (`class_field::THIS_CLASS`). Answers
    /// whether the write happened, like the other class setters;
    /// `Mutator::attach_metaclass` is the one caller, on a metaclass it has
    /// just built.
    pub(crate) fn class_set_this_class(
        &mut self,
        metaclass: ClassReference<'gc>,
        class: ClassReference<'gc>,
    ) -> bool {
        self.set_class_field(metaclass, class_field::THIS_CLASS, Value::object(class))
    }

    /// Relinks `class`'s superclass pointer. Answers whether the write
    /// happened, mirroring `class_set_application` -- the matching relink on
    /// the application chain that carries the layout, called right alongside
    /// this one. Two callers: the bootstrap's kernel phase, which links each
    /// builtin class to the one its `Kernel.ns` clause names
    /// (`interpreter::kernel`), and `tests/instantiate.rs`'s
    /// `superclass_slot_initializer_runs_before_the_subclass_own_slots`: a
    /// root class's superclass clause may name only `Object` or `Value`
    /// (the root rule, `resolver::root_clause_defect`), so linking a
    /// subclass to an already-loaded sibling *user-defined* class has to
    /// happen by hand on the class-object chain `super`/`instantiate` walk.
    pub fn class_set_superclass(
        &mut self,
        class: ClassReference<'gc>,
        superclass: ClassReference<'gc>,
    ) -> bool {
        self.set_class_field(class, class_field::SUPERCLASS, Value::object(superclass))
    }

    /// Unlinks `class`'s superclass pointer, answering whether the write
    /// happened like `class_set_superclass`. The bootstrap's kernel phase
    /// is the one caller: it states that the kernel's `Object` has no
    /// superclass rather than rely on the placeholder already having none.
    pub fn class_clear_superclass(&mut self, class: ClassReference<'gc>) -> bool {
        self.set_class_field(class, class_field::SUPERCLASS, Value::nil())
    }

    /// Sets `class`'s enclosing object and declaring class. The bootstrap's
    /// kernel phase is the one caller: a builtin class object exists before
    /// the kernel instance that encloses it can, because a literal needs its
    /// class from the first instruction on. Answers whether the write
    /// happened, like the other class setters.
    pub fn class_set_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
    ) -> bool {
        self.set_class_field(
            class,
            class_field::ENCLOSING,
            enclosing.map_or(Value::nil(), Value::object),
        ) && self.set_class_field(
            class,
            class_field::DECLARING_CLASS,
            declaring_class.map_or(Value::nil(), Value::object),
        )
    }

    /// Records `class`'s verdict on §3.1.1's enclosing-object condition:
    /// `refusal` is the `Array` `Mutator::record_value_class_refusal` makes,
    /// the one caller. Answers whether the write happened, like the other
    /// class setters.
    pub(crate) fn class_set_value_refusal(
        &mut self,
        class: ClassReference<'gc>,
        refusal: ObjectReference<'gc>,
    ) -> bool {
        self.set_class_field(
            class,
            class_field::VALUE_CLASS_REFUSAL,
            Value::object(refusal),
        )
    }

    /// Records the `<:` clause that added `class` (`newspeak-spec.pdf`
    /// §6.2), right after the level is created: by
    /// `Mutator::new_applied_level` for a level built from source, and by
    /// the image decoder for a decoded one. The field packs the declaration
    /// by its `MixinId`, which this registers. The clause's index is below
    /// `CLAUSE_INDEX_LIMIT`: the resolver refuses a declaration of more
    /// clauses, and the decoder an index past it. Answers whether the write
    /// happened, like the other class setters.
    pub(crate) fn class_set_applied_clause(
        &mut self,
        class: ClassReference<'gc>,
        clause: crate::mixin::AppliedClause,
    ) -> bool {
        if !self.is_class(class) {
            return false;
        }
        let crate::mixin::AppliedClause {
            declaration,
            index,
            named_builtin,
        } = clause;
        let mixin = self.mixin_id(&declaration);
        let packed = crate::mixin::AppliedClause::pack(mixin, index, named_builtin);
        self.set_class_field(
            class,
            class_field::APPLIED_CLAUSE,
            Value::small_integer(packed),
        )
    }

    /// Gives `class` a new application and moves every live object of that
    /// class to its ids -- a walk of the space, which is why the decoder,
    /// which installs one application per class record, calls
    /// `class_install_application` instead and stamps once afterwards
    /// (`stamp_dispatch_ids`). Answers whether the write happened, mirroring
    /// `array_set_element` and `instance_set_field`. Its callers are the
    /// bootstrap's kernel phase, filling each builtin placeholder once
    /// (`interpreter::kernel`), and tests editing an already-live class
    /// (`install_mixin` in `tests/image.rs`, `tests/instantiate.rs`,
    /// `ns-cli`'s `disasm_snapshots.rs`) -- a wrong-shape `class` there is a
    /// bug of the caller's, not an untrusted-input case.
    pub fn class_set_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        if !self.class_install_application(class, application) {
            return false;
        }
        let dispatch_id = self.header_dispatch_id(class);
        let ranges = self.space.occupied();
        let words = self.space.words_mut();
        for range in ranges {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(words, position);
                let class_word = Word::from_bits(words[header + CLASS]);
                if class_word.object_offset() == Some(class.offset) {
                    words[header] = Header(words[header]).with_guard(dispatch_id).0;
                }
                position = next;
            }
        }
        true
    }

    /// `class_set_application` without the walk: `class`'s own header is
    /// restamped from its header class, its instances' are not. The decoder's
    /// `rebuild_applications` uses this to install one application per
    /// decoded class record -- walking the space on every one of those would
    /// be quadratic in a count the image controls -- and stamps every
    /// decoded object once afterwards, with `stamp_dispatch_ids`.
    ///
    /// The class object names `application` by its index, so it must be
    /// the table's own entry (`register_application`).
    pub(crate) fn class_install_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        debug_assert!(
            self.tables
                .applications
                .get(application.index().index())
                .and_then(Option::as_ref)
                .is_some_and(|entry| Rc::ptr_eq(entry, &application)),
            "internal: a class object's application is registered (`Heap::register_application`)"
        );
        if !self.set_class_field(
            class,
            class_field::APPLICATION,
            count_value(application.index().index()),
        ) {
            return false;
        }
        // Its own id is its header class's, which a new application of its
        // own changes only when it is its own class -- the self-referential
        // builtin `Object` of the kernel phase. A decoded class record's
        // header is its metaclass record, whose application may not be
        // installed yet; `stamp_dispatch_ids` restamps every decoded object
        // once all of them are.
        let header_class = self.object_class(class);
        let dispatch_id = self.header_dispatch_id(header_class);
        let header = self.header(class).with_guard(dispatch_id);
        self.set_header(class, header);
        true
    }

    /// Every class object whose chain -- the class and its superclass
    /// links -- holds an application `replaced` answers true for (by the
    /// application's mixin's id), and every object whose class word
    /// names one of them, in one walk of the space; changes nothing.
    ///
    /// The install's walk: there is no registry of a mixin's applications
    /// or a class's instances, so the space is the one place that holds
    /// them all (`docs/design/liveness-and-reflection.md`, "The install is
    /// one leaf, and its commit has no safepoint"). An object nothing
    /// reaches any more is found as well, since only a collection tells
    /// the two apart, and none runs inside a leaf; a forwarder and a gap
    /// are no objects and are skipped.
    pub(crate) fn classes_and_instances_over(
        &self,
        replaced: impl Fn(MixinId) -> bool,
    ) -> (Vec<ClassReference<'gc>>, Vec<ObjectReference<'gc>>) {
        // Per class object, by offset, whether its chain holds a replaced
        // level: every instance of a class asks the same question.
        let mut verdicts: IdentityMap<u64, bool> = IdentityMap::default();
        let mut over = |class: ClassReference<'gc>| -> bool {
            *verdicts.entry(class.offset.get()).or_insert_with(|| {
                let mut level = Some(class);
                while let Some(current) = level {
                    let holds = self
                        .class_application_entry(current)
                        .and_then(|application| self.known_mixin_id(&application.mixin))
                        .is_some_and(&replaced);
                    if holds {
                        return true;
                    }
                    level = self.class_superclass(current);
                }
                false
            })
        };
        let mut classes = Vec::new();
        let mut instances = Vec::new();
        let words = self.space.words();
        for range in self.space.occupied() {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(words, position);
                position = next;
                let format = Header(words[header]).format();
                if matches!(format, Format::Free | Format::Forwarded) {
                    continue;
                }
                let object = self.object_handle(offset_of_header(header));
                if format == Format::Class && over(object) {
                    classes.push(object);
                }
                if over(self.object_class(object)) {
                    instances.push(object);
                }
            }
        }
        (classes, instances)
    }

    /// Stamps each of `objects` with the id its class now answers. The
    /// decoder's one pass, after `rebuild_applications` has given every
    /// decoded class its application, and the install's restamp of the
    /// objects whose class it re-applied without moving them, as
    /// `class_set_application` stamps every instance of one class.
    pub(crate) fn stamp_dispatch_ids(&mut self, objects: &[ObjectReference<'gc>]) {
        for object in objects {
            let class = self.object_class(*object);
            let dispatch_id = self.header_dispatch_id(class);
            let header = self.header(*object).with_guard(dispatch_id);
            self.set_header(*object, header);
        }
    }

    // ---------- Frame accessors ----------
    //
    // A `FrameReference` names a *reified* activation. Its receiver, class and
    // kind are fixed when it is created, so they read the same whether or not
    // the frame is still married to a live stack frame. Its program counter and
    // registers are not: while it is married they live on the activation stack,
    // so the accessors that read or write the frame's own copy assert it is
    // dead. `frame_local`/`frame_set_local` (with the activation stack, below)
    // follow the marriage instead.
    //
    // Only the accessors a caller actually needs are public here; a frame's
    // argument count and program counter are read through
    // `frame_register_shape` and `frame_record`. Three have only test
    // callers: `frame_set_program_counter`, `frame_sender` and
    // `frame_is_uncontinuable` read or write fields the image format records
    // per frame, and the image and interpreter tests build and check frame
    // records through them. Nothing in the runtime decides
    // anything by those fields -- whether a frame is live is
    // `frame_activation`, never `frame_is_uncontinuable`.

    /// `frame`'s words: its fixed fields, then its registers. The one lender
    /// of a frame object's payload, private to the heap, so that every
    /// public accessor copies out (`docs/design/memory.md`, "The seam").
    fn frame_words(&self, frame: FrameReference<'gc>) -> &[u64] {
        self.fields(self.frame_object(frame))
    }

    /// `frame_words`, for a write.
    fn frame_words_mut(&mut self, frame: FrameReference<'gc>) -> &mut [u64] {
        let object = self.frame_object(frame);
        self.fields_mut(object)
    }

    /// The fixed field `field` of `frame`, as a value.
    fn frame_field(&self, frame: FrameReference<'gc>, field: usize) -> Value<'gc> {
        self.load(Word::from_bits(self.frame_words(frame)[field]))
    }

    /// A fixed field holding a count or an index; 0 for anything else, which
    /// no frame this heap built stores.
    fn frame_count_field(&self, frame: FrameReference<'gc>, field: usize) -> usize {
        self.frame_field(frame, field)
            .as_small_integer()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0)
    }

    /// A fixed field holding a frame object, as its frame.
    fn frame_frame_field(
        &self,
        frame: FrameReference<'gc>,
        field: usize,
    ) -> Option<FrameReference<'gc>> {
        self.frame_field(frame, field)
            .as_object()
            .and_then(|object| self.object_frame(object))
    }

    /// The stack offset of the frame `words`' frame object is married to, or
    /// `None` while its activation is suspended into it and once it has
    /// returned.
    fn married_offset(&self, words: &[u64]) -> Option<u32> {
        self.load(Word::from_bits(words[frame_field::MARRIED]))
            .as_small_integer()
            .and_then(|offset| u32::try_from(offset).ok())
    }

    /// Whether `frame` is a closure activation's.
    fn frame_runs_a_closure(&self, frame: FrameReference<'gc>) -> bool {
        self.frame_count_field(frame, frame_field::KIND)
            == usize::from(crate::image::opcodes::FRAME_CLOSURE)
    }

    /// The body that runs (or ran) in `frame`, or `None` when it has none to
    /// name: a top-level frame, or one filled without a body whose kind
    /// finds none (`fill_frame_fields`). `image::decode` reconciles a decoded
    /// frame against it.
    pub(crate) fn frame_code(&self, frame: FrameReference<'gc>) -> Option<Rc<CompiledCode>> {
        self.code(self.frame_code_id_field(frame)?)
    }

    /// The code id `frame` records, as `frame_code` reads it: `None` for a
    /// top-level frame's, which names no body.
    fn frame_code_id_field(&self, frame: FrameReference<'gc>) -> Option<CodeId> {
        self.frame_field(frame, frame_field::CODE_ID)
            .as_small_integer()
            .and_then(|id| usize::try_from(id).ok())
            .and_then(CodeId::from_index)
    }

    /// The body a frame of `kind` runs, found the way a decoded frame's is:
    /// a method's own, a closure's, and for the four class-carrying kinds
    /// the field of the class's mixin each one's body lives in -- or, for a
    /// `<:` clause's message, the body the level's clause names, since the
    /// applying class's mixin holds one per clause. `None` for a top-level
    /// frame, and for a kind whose class names no such body.
    fn kind_code(&self, kind: &FrameKind<'gc>) -> Option<Rc<CompiledCode>> {
        let mixin = |class: ClassReference<'gc>| {
            self.class_application(class)
                .map(|application| application.mixin.clone())
        };
        match kind {
            FrameKind::Method(method) => Some(method.code.clone()),
            FrameKind::Initializer(class, body) => {
                body.code_of(self.initializer_mixin(*class, *body)?.as_ref())
            }
            FrameKind::Factory(class) => {
                crate::interpreter::instantiate::factory_code(self, *class)
            }
            FrameKind::SuperclassClause(class) => mixin(*class)?.superclass_clause_code.clone(),
            FrameKind::Closure(closure) => self.closure_code(*closure),
            FrameKind::TopLevel => None,
        }
    }

    /// The code id a frame of `kind` running `code` records: a method's
    /// registered with its method, so that `frame_kind` finds the method
    /// again; none for a top-level frame.
    fn frame_code_id(
        &mut self,
        kind: &FrameKind<'gc>,
        code: Option<&Rc<CompiledCode>>,
    ) -> Option<CodeId> {
        match kind {
            FrameKind::Method(method) => Some(self.code_id_of_method(method)),
            FrameKind::TopLevel => None,
            _ => code.map(|code| self.code_id(code)),
        }
    }

    /// How many of `frame`'s registers are arguments: its code's argument
    /// count, never more than it has registers. A top-level frame takes none.
    /// Read on every register access to a frame whose activation is not on
    /// the stack, so it reads the code's row in place rather than taking the
    /// code out.
    fn frame_argument_count(&self, frame: FrameReference<'gc>) -> usize {
        let registers = self.frame_words(frame).len() - frame_field::COUNT;
        self.frame_code_argument_count(frame).min(registers)
    }

    /// The argument count of the body `frame`'s code id names, 0 when it
    /// names none: `frame_argument_count` before it is bounded by the
    /// frame's registers.
    fn frame_code_argument_count(&self, frame: FrameReference<'gc>) -> usize {
        self.frame_code_id_field(frame)
            .and_then(|id| self.tables.codes.get(id.index())?.as_ref())
            .map_or(0, |entry| usize::from(entry.code.argument_count))
    }

    /// What an outer-local site records of the frame object the word
    /// `frame_bits` names (`jit::closures::refill_outer_site`): its code
    /// id field's word, and the argument count of the body that word
    /// names (`frame_code_argument_count`), which machine code bounds by
    /// the frame's registers itself, as `frame_argument_count` does. A
    /// code id names one body until a full collection frees it, which
    /// moves the code table's epoch the site records beside the two.
    #[cfg(ns_jit)]
    pub(crate) fn machine_frame_code_arguments(&self, frame_bits: u64) -> (u64, u64) {
        let Some(frame) = self
            .machine_value(frame_bits)
            .as_object()
            .and_then(|object| self.object_frame(object))
        else {
            unreachable!("internal: an outer-local template hands the site leaf a frame object");
        };
        (
            self.frame_words(frame)[frame_field::CODE_ID],
            self.frame_code_argument_count(frame) as u64,
        )
    }

    /// A reified frame's receiver. Fixed when the frame is created, so it
    /// needs no redirect while the frame is married.
    pub fn frame_receiver(&self, frame: FrameReference<'gc>) -> Value<'gc> {
        self.frame_field(frame, frame_field::RECEIVER)
    }

    pub fn frame_current_class(&self, frame: FrameReference<'gc>) -> Option<ClassReference<'gc>> {
        self.frame_field(frame, frame_field::CURRENT_CLASS)
            .as_object()
    }

    /// Test and format level: the image tests use it to give a dead frame a
    /// synthetic paused `program_counter` before a snapshot. The runtime stores
    /// a running activation's `program_counter` in its stack frame
    /// (`activation_set_program_counter`), and the divorce copies it here.
    pub fn frame_set_program_counter(
        &mut self,
        frame: FrameReference<'gc>,
        program_counter: usize,
    ) {
        debug_assert!(
            self.frame_activation(frame).is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        self.frame_words_mut(frame)[frame_field::PROGRAM_COUNTER] =
            count_word(program_counter).bits();
    }

    pub fn frame_home(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        self.frame_frame_field(frame, frame_field::HOME)
    }

    /// A reified frame's recorded sender: the frame a suspended activation
    /// returns to, when that one is suspended too
    /// (`suspend_activations`), or what a decoded record carried. `None` for
    /// every other frame -- a live activation's sender is the stack frame
    /// below it, and a returned activation has none.
    pub fn frame_sender(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        self.frame_frame_field(frame, frame_field::SENDER)
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `locals::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        if !self.frame_runs_a_closure(frame) {
            return None;
        }
        let closure = self
            .frame_field(frame, frame_field::KIND_OPERAND)
            .as_object()?;
        self.closure_captured_frame(closure)
    }

    /// The frame's recorded `uncontinuable` flag. Test and format level:
    /// true for every frame the divorce left behind, but on a decoded frame
    /// it answers whatever the file said. It is not liveness --
    /// `raise_return` treats every decoded frame as dead, because it
    /// decides by `frame_is_running`, and a decoded frame is never married
    /// nor suspended.
    pub fn frame_is_uncontinuable(&self, frame: FrameReference<'gc>) -> bool {
        self.frame_field(frame, frame_field::UNCONTINUABLE)
            .as_boolean()
            == Some(true)
    }

    /// `frame`'s `kind`, rebuilt from its kind tag, its kind operand and,
    /// for a method, its code id. Cloning a method's is cheap -- an `Rc` --
    /// and `image::decode` is the only caller that needs to match on the
    /// *whole* kind rather than one variant's payload (`frame_captured`): to
    /// check that a class-carrying kind's class is itself a class reference
    /// (`check_class_references`).
    pub fn frame_kind(&self, frame: FrameReference<'gc>) -> FrameKind<'gc> {
        use crate::image::opcodes::{
            FRAME_CLOSURE, FRAME_FACTORY, FRAME_METHOD, FRAME_SUPERCLASS_CLAUSE,
        };
        let operand = self
            .frame_field(frame, frame_field::KIND_OPERAND)
            .as_object();
        let tag = u8::try_from(self.frame_count_field(frame, frame_field::KIND)).ok();
        let kind = match tag {
            Some(FRAME_METHOD) => self
                .frame_field(frame, frame_field::CODE_ID)
                .as_small_integer()
                .and_then(|id| usize::try_from(id).ok())
                .and_then(CodeId::from_index)
                .and_then(|id| self.code_method(id))
                .map(FrameKind::Method),
            Some(FRAME_CLOSURE) => operand.map(FrameKind::Closure),
            Some(FRAME_FACTORY) => operand.map(FrameKind::Factory),
            Some(tag) if InitializerBody::of_frame_tag(tag).is_some() => operand
                .zip(InitializerBody::of_frame_tag(tag))
                .map(|(class, body)| FrameKind::Initializer(class, body)),
            Some(FRAME_SUPERCLASS_CLAUSE) => operand.map(FrameKind::SuperclassClause),
            _ => None,
        };
        // Every frame this heap built records what its kind needs: a method
        // frame's code id names its method (`frame_code_id`), and every other
        // kind but a top-level one has its operand.
        debug_assert!(
            kind.is_some() || tag == Some(crate::image::opcodes::FRAME_TOP_LEVEL),
            "internal: a frame object records its kind"
        );
        kind.unwrap_or(FrameKind::TopLevel)
    }

    /// `program_counter`, how many of a frame's registers are arguments and
    /// how many registers it holds -- its arguments and its registers above
    /// the header (`frame_field_of_register`) -- read together.
    /// `image::decode::verify_graph` checks them against the shape of the
    /// compiled body it reconciles every decoded frame against
    /// (`CompiledCode::frame_register_count`).
    pub fn frame_register_shape(&self, frame: FrameReference<'gc>) -> (usize, usize, usize) {
        debug_assert!(
            self.frame_activation(frame).is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        (
            self.frame_count_field(frame, frame_field::PROGRAM_COUNTER),
            self.frame_argument_count(frame),
            self.frame_words(frame).len() - frame_field::COUNT,
        )
    }

    /// The whole of `frame`'s contents as values, its registers split at its
    /// code's argument count. `image::encode::encode_frame` writes a whole
    /// frame record and `GraphTable::visit_frame` walks a whole frame's
    /// references; both need every field. Not for a hot path: it copies
    /// every register out.
    ///
    /// Only for a dead frame, which is also the encoder's precondition: it
    /// only ever runs between sends, when nothing is live, and
    /// `image::snapshot` refuses to start otherwise.
    pub fn frame_record(&self, frame: FrameReference<'gc>) -> FrameRecord<'gc> {
        debug_assert!(
            self.frame_activation(frame).is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        let mut arguments: Vec<Value<'gc>> = self.frame_words(frame)[frame_field::COUNT..]
            .iter()
            .map(|bits| self.load(Word::from_bits(*bits)))
            .collect();
        let temporaries = arguments.split_off(self.frame_argument_count(frame));
        FrameRecord {
            receiver: self.frame_receiver(frame),
            arguments,
            temporaries,
            program_counter: self.frame_count_field(frame, frame_field::PROGRAM_COUNTER),
            sender: self.frame_sender(frame),
            home: self.frame_home(frame),
            current_class: self.frame_current_class(frame),
            kind: self.frame_kind(frame),
            uncontinuable: self.frame_is_uncontinuable(frame),
        }
    }

    /// Test and format level: `tests/image.rs` forges a decoded frame's
    /// `uncontinuable` flag through it, as a hand-edited file could. Writes
    /// the one field in place; the runtime sets the flag only when the
    /// divorce leaves a frame behind, and the image decoder in
    /// `fill_frame_fields`.
    #[doc(hidden)]
    pub fn frame_set_uncontinuable(&mut self, frame: FrameReference<'gc>, uncontinuable: bool) {
        debug_assert!(
            self.frame_activation(frame).is_none(),
            "a married frame is continuable until its activation returns"
        );
        self.frame_words_mut(frame)[frame_field::UNCONTINUABLE] =
            Value::boolean(uncontinuable).word().bits();
    }

    /// Fills a frame placeholder's fixed fields in place from `record`,
    /// whose registers must be empty: the register file the first pass sized
    /// stays where it is, nil, for `fill_frame_register` to write register
    /// by register. The frame keeps its class, its guard and its identity
    /// hash, and ends identical to a frame `allocate_frame` builds from the
    /// same fields and registers. Its code is `code`, the body the image
    /// names -- after an edit the class's current version carries another,
    /// so the body is never looked up through the class when it is given --
    /// or, when none is given, the one its kind finds (`kind_code`). The
    /// record's split between arguments and temporaries is not kept: a
    /// frame's arguments are its code's (`image::decode::verify_graph` holds
    /// the register count to it, and the body to the frame's kind).
    ///
    /// The image decoder's second pass is the only caller. Call this on a
    /// handle something else already holds and it silently replaces that
    /// frame's already-verified `program_counter` with an unverified one: no
    /// error, and no way for `verify_graph` to notice, because it has
    /// already finished.
    pub(crate) fn fill_frame_fields(
        &mut self,
        frame: FrameReference<'gc>,
        record: FrameRecord<'gc>,
        code: Option<Rc<CompiledCode>>,
    ) {
        self.check_frame_origin(frame);
        let FrameRecord {
            receiver,
            arguments,
            temporaries,
            program_counter,
            sender,
            home,
            current_class,
            kind,
            uncontinuable,
        } = record;
        debug_assert!(
            arguments.is_empty() && temporaries.is_empty(),
            "`fill_frame_register` writes a placeholder's registers"
        );
        let code = code.or_else(|| self.kind_code(&kind));
        let code = self.frame_code_id(&kind, code.as_ref());
        let words = self.frame_fixed_words(FrameFields {
            sender,
            home,
            receiver,
            current_class,
            program_counter,
            code,
            kind,
            uncontinuable,
        });
        let object = self.frame_object(frame);
        for word in words {
            self.debug_assert_tenured(word);
        }
        for (slot, word) in self.fields_mut(object).iter_mut().zip(words) {
            *slot = word.bits();
        }
    }

    /// Stores `value` into register `index` of a frame placeholder whose
    /// fixed fields `fill_frame_fields` wrote; `false` past the register file
    /// the first pass sized.
    pub(crate) fn fill_frame_register(
        &mut self,
        frame: FrameReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        let object = self.frame_object(frame);
        match index.checked_add(frame_field::COUNT) {
            Some(field) => self.fill_placeholder_field(object, field, value),
            None => false,
        }
    }

    // ---------- The activation stack ----------
    //
    // A live activation is a frame of tagged words on `stack.frames`: its
    // header, then its registers. `reify` marries a frame object to it when
    // something must hold the activation past this call -- a block being
    // created, an unwind's target, a suspension -- and `pop_activation`
    // divorces it again. See `docs/design/execution.md`, "Activations",
    // "Lifecycle".

    /// Pushes an activation whose registers are `arguments`, then its
    /// header, then `temporary_count` nils, above every live frame's whole
    /// window: a send's slots copied out of the frame below with one copy
    /// of the stack's words, or values copied in. Refuses, pushing nothing
    /// and handing the header back, a frame the stack's reservation cannot
    /// hold: the net beneath the depth limit for register files the
    /// activation count does not bound, which the caller raises as that
    /// limit's error. A send's own callee is pushed on its slots instead
    /// (`push_overlapped_activation`).
    #[inline(always)]
    pub fn push_activation(
        &mut self,
        mut header: ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        temporary_count: usize,
    ) -> Result<ActivationReference, StackExhausted<'gc>> {
        match self.push_activation_in::<false>(&mut header, arguments, temporary_count) {
            Ok(activation) => Ok(activation),
            Err(PushRefused) => Err(StackExhausted::of(header)),
        }
    }

    /// `push_activation` of a header the caller keeps: on a refusal it is
    /// still the caller's, untouched; once pushed, its continuation is the
    /// frame's and `Continuation::Deliver` is left in its place
    /// (`PushRefused` says why it is borrowed).
    #[inline(always)]
    pub(crate) fn push_activation_from(
        &mut self,
        header: &mut ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        temporary_count: usize,
    ) -> Result<ActivationReference, PushRefused> {
        self.push_activation_in::<false>(header, arguments, temporary_count)
    }

    /// `push_activation`, refusing only a frame the whole reservation cannot
    /// hold, the reserve the ordinary pushes leave free included: the push
    /// made while the depth limit's reserve is held, so that the `signal`
    /// of the overflow and its handler run in the room the refusal left.
    #[cold]
    #[inline(never)]
    pub(crate) fn push_reserved_activation(
        &mut self,
        mut header: ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        temporary_count: usize,
    ) -> Result<ActivationReference, StackExhausted<'gc>> {
        match self.push_activation_in::<true>(&mut header, arguments, temporary_count) {
            Ok(activation) => Ok(activation),
            Err(PushRefused) => Err(StackExhausted::of(header)),
        }
    }

    /// Whether the activation stack is within the reserve of the end of the
    /// room ordinary frames have: an error raised here takes the depth
    /// limit's reserve for its `signal`, as one within `DEPTH_RESERVE`
    /// activations of the limit does.
    pub(crate) fn stack_is_within_the_reserve_of_its_limit(&self) -> bool {
        self.stack.frames.within_the_reserve_of_the_limit()
    }

    /// Every header word of a frame for `header` but its continuation,
    /// which is named only once the push has succeeded, so that a refused
    /// push takes no row: computed before the frame is pushed, since the
    /// push is the last thing that can refuse, so a refused push has
    /// changed nothing. `start` is the stack word of the frame's first
    /// argument, from which its caller word measures the top
    /// (`caller_word`).
    #[inline(always)]
    fn header_words(
        &mut self,
        header: &ActivationHeader<'gc>,
        start: usize,
        argument_count: u16,
        temporary_count: usize,
    ) -> FrameHeaderWords {
        let receiver = self.store(header.receiver);
        let current_class = header
            .current_class
            .map_or(Word::NIL, |class| self.store(Value::object(class)));
        let (kind_tag, operand) = self.kind_words(header.kind);
        // Only code that addresses its own slots pays for these reads; a
        // send to any other method tests one flag. The receiver's guard is
        // recorded only when its chain contains the entered level: a
        // private lexical send, a super send and an initializer's superclass
        // or applied level bind their code without looking at the receiver, so a
        // receiver the bulk forward replaced before the push would
        // otherwise be vouched for by its own guard and read or written in
        // the old layout. Without one, the first own-slot access takes the
        // cold check (`interpreter::locals::check_own_slots`). The walk
        // stops at the receiver's own class for every level but an
        // inherited one.
        let (recorded_guard, entered_level) = if header.code.addresses_own_slots {
            let entered_level = header
                .current_class
                .and_then(|class| self.class_application_index(class))
                .unwrap_or(ApplicationIndex::UNREGISTERED);
            let recorded_guard = if self.receiver_has_level(header.receiver, entered_level) {
                self.dispatch_id(header.receiver)
            } else {
                DispatchId::NONE
            };
            (recorded_guard, entered_level)
        } else {
            (DispatchId::NONE, ApplicationIndex::UNREGISTERED)
        };
        // A method's body is registered with its method, every other body
        // alone; either hits the body's one-entry cache but for its first
        // push after a reclaim freed rows.
        debug_assert_eq!(
            header.method.is_some(),
            matches!(header.kind, ActivationKind::Method(_)),
            "internal: a header names its method exactly for a method's activation"
        );
        let code = match &header.method {
            Some(method) => {
                debug_assert!(
                    Rc::ptr_eq(&method.code, &header.code),
                    "internal: a method's activation runs its method's body"
                );
                debug_assert!(
                    matches!(header.kind, ActivationKind::Method(selector) if selector == method.selector),
                    "internal: a method's activation is named by its method's selector"
                );
                self.code_id_of_method(method)
            }
            None => self.code_id(&header.code),
        };
        let flags = FrameFlags::new(kind_tag, argument_count, header.counted, false);
        #[cfg(debug_assertions)]
        let flags = flags.with_generation(self.peek_next_generation());
        FrameHeaderWords {
            receiver,
            current_class,
            operand,
            reified: Word::NIL,
            flags: flags.word(),
            code: frames::number_word(code.index() as u64),
            program_counter: frames::number_word(0),
            feedback_base: frames::number_word(u64::from(header.feedback_base.raw())),
            caller: self.caller_word(start),
            continuation: Word::NIL,
            guard: frames::number_word(u64::from(recorded_guard.bits())),
            entered_level: frames::number_word(entered_level.index() as u64),
            register_count: frames::number_word(temporary_count as u64),
        }
    }

    /// The frame just pushed at `offset` made the innermost: counted, and
    /// its handle minted.
    #[inline(always)]
    fn pushed_at(&mut self, offset: u32, counted: bool) -> ActivationReference {
        let stack = &mut self.stack;
        stack.innermost = offset;
        stack.live += 1;
        let count = stack.machine_words.get(MachineWord::DepthCount);
        stack
            .machine_words
            .set(MachineWord::DepthCount, count + u64::from(counted));
        #[cfg(debug_assertions)]
        stack.advance_generation();
        self.activation_handle(offset)
    }

    #[inline(always)]
    fn push_activation_in<const RESERVED: bool>(
        &mut self,
        header: &mut ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        temporary_count: usize,
    ) -> Result<ActivationReference, PushRefused> {
        // Checked by a comparison rather than `u16::try_from`, whose
        // `Result` the push built and took apart on every activation. Every
        // argument is a register, so a count past the register ceiling is
        // a caller's defect: a send's window holds at most `u16::MAX`, and
        // a longer list of values is refused before it pushes
        // (`Interpreter::push`).
        assert!(
            arguments.len() <= usize::from(u16::MAX),
            "internal: no activation is handed more arguments than the register ceiling, u16::MAX"
        );
        debug_assert_arity(&header.code, arguments.len());
        let argument_count = register_number(arguments.len());
        let words = usize::from(argument_count) + frame_header::COUNT + temporary_count;
        // `store`'s check, made apart from the copy so that a release build,
        // which has no check, copies the arguments' words straight in. A
        // slot's word is already on the stack, stored there by `store`, so
        // only that the slots lie in their live window is checked.
        #[cfg(debug_assertions)]
        match arguments {
            Arguments::Slots(slots) => {
                // `register` checks each slot against its window and
                // refuses a dead one: the push reads every argument.
                for index in 0..slots.count() {
                    let _ = self.register(slots.register_base(), slots.start() + index);
                }
            }
            Arguments::Values(values) => {
                for argument in values.iter() {
                    self.store(*argument);
                }
            }
        }
        // Pushed at the top: its first argument is the top's word.
        let top = self.stack.frames.top();
        let mut header_words = self.header_words(header, top, argument_count, temporary_count);
        let Heap {
            stack,
            continuations,
            ..
        } = self;
        let pushed = if RESERVED {
            stack.frames.push_reserved(words)
        } else {
            stack.frames.push(words)
        };
        // The push answers the frame's own words and every word below it,
        // which is where a send's slots lie -- in the sender's window -- so
        // the copy reads the one and writes the other, and nothing indexes
        // the whole stack, nor reads its top and length again, once the
        // frame is pushed. Between the push and the frame's writes only the
        // continuation goes into its table, which reads no word of the
        // stack, so nothing reads the words the top covers before they are
        // written.
        let (start, below, frame) = match pushed {
            Ok(pushed) => pushed,
            Err(frames::ReservationExhausted) => return Err(PushRefused),
        };
        // Taken only now, so that a refused push leaves the header whole.
        if let Some(row) =
            continuations.store(std::mem::replace(&mut header.then, Continuation::Deliver))
        {
            header_words.continuation = frames::number_word(u64::from(row.0));
        }
        let (argument_registers, rest) = frame.split_at_mut(usize::from(argument_count));
        let (fixed, temporaries) = rest.split_at_mut(frame_header::COUNT);
        header_words.write(fixed);
        match arguments {
            // One argument or two are copied word by word, more by a loop
            // the compiler keeps inline: `copy_from_slice` would call out
            // of line on every push.
            Arguments::Slots(slots) => {
                let source = slots.first_word();
                match argument_registers {
                    [] => {}
                    [only] => *only = below[source],
                    [first, second] => {
                        *first = below[source];
                        *second = below[source + 1];
                    }
                    registers => {
                        // Bounded to the window, so a window running past
                        // the sender's top panics as the arms above do
                        // instead of copying fewer words.
                        let window = &below[source..source + registers.len()];
                        for (register, word) in registers.iter_mut().zip(window) {
                            *register = *word;
                        }
                    }
                }
            }
            // One value is copied on its own, as one slot is.
            Arguments::Values(values) => match argument_registers {
                [] => {}
                [only] => *only = values[0].word().bits(),
                registers => {
                    for (register, argument) in registers.iter_mut().zip(values.iter()) {
                        *register = argument.word().bits();
                    }
                }
            },
        }
        fill_temporaries(temporaries, header.code.local_count, argument_count);
        let offset = frames::stack_offset(start + usize::from(argument_count));
        Ok(self.pushed_at(offset, header.counted))
    }

    /// Copies a send's arguments from `window`, their sources where those
    /// are consecutive registers, into its slots from register
    /// `argument_base` of the same window, before its callee is laid on
    /// them (`push_overlapped_activation`). Neither lowering makes a source
    /// another argument's slot, so the two ranges are the same or apart;
    /// the copy runs in the direction that is safe either way all the same.
    /// One argument or two are copied word by word, more by a loop kept
    /// inline, as the push copies them.
    #[inline(always)]
    pub(crate) fn place_in_slots(&mut self, window: ArgumentSlots, argument_base: u16) {
        let count = usize::from(window.count());
        #[cfg(debug_assertions)]
        if count > 0 {
            let register_base = window.register_base();
            // `register` refuses a dead source: the copy reads every one.
            for index in 0..window.count() {
                let _ = self.register(register_base, window.start() + index);
            }
            self.check_register(register_base, argument_base);
            self.check_register(register_base, argument_base + (window.count() - 1));
        }
        let source = window.first_word();
        let slot = window.register_base().index as usize + usize::from(argument_base);
        let words = self.stack.frames.words_mut();
        match count {
            0 => {}
            1 => words[slot] = words[source],
            2 => {
                let (first, second) = (words[source], words[source + 1]);
                words[slot] = first;
                words[slot + 1] = second;
            }
            _ if slot < source => {
                for offset in 0..count {
                    words[slot + offset] = words[source + offset];
                }
            }
            _ => {
                for offset in (0..count).rev() {
                    words[slot + offset] = words[source + offset];
                }
            }
        }
    }

    /// Pushes a send's own callee on the send's `argument_count` slots from
    /// register `argument_base` of the window `register_base` names -- the
    /// innermost activation's, whose registers above the slots are dead
    /// (`docs/design/execution.md`, "Planned: overlapping register
    /// windows") -- so that the slots are the callee's arguments, copied
    /// nowhere: its header goes where the slots end, then
    /// `temporary_count` nils. Every slot must hold its argument already.
    /// Refuses as `push_activation_from` does, pushing nothing and leaving
    /// the header the caller's.
    #[inline(always)]
    pub(crate) fn push_overlapped_activation(
        &mut self,
        header: &mut ActivationHeader<'gc>,
        register_base: RegisterBase,
        argument_base: u16,
        argument_count: u16,
        temporary_count: usize,
    ) -> Result<ActivationReference, PushRefused> {
        debug_assert_arity(&header.code, usize::from(argument_count));
        #[cfg(debug_assertions)]
        {
            let sender = register_base.activation;
            self.check_activation(sender);
            assert_eq!(
                sender.offset, self.stack.innermost,
                "internal: a callee lies on the slots of the innermost activation"
            );
            let sender_arguments = self.stack.flags(sender.offset).argument_count;
            assert!(
                usize::from(argument_base) >= usize::from(sender_arguments) + frame_header::COUNT,
                "internal: a send's slots from r{argument_base} lie above its sender's header"
            );
            // `register` refuses a dead slot: every slot holds its
            // argument by now.
            for index in 0..argument_count {
                let _ = self.register(register_base, argument_base + index);
            }
        }
        let start = register_base.index as usize + usize::from(argument_base);
        let offset = start + usize::from(argument_count);
        debug_assert!(
            offset <= self.stack.window_end(self.stack.innermost),
            "internal: a send's slots end inside its sender's window"
        );
        let mut header_words = self.header_words(header, start, argument_count, temporary_count);
        let Heap {
            stack,
            continuations,
            ..
        } = self;
        let frame = match stack
            .frames
            .push_from(offset, frame_header::COUNT + temporary_count)
        {
            Ok(frame) => frame,
            Err(frames::ReservationExhausted) => return Err(PushRefused),
        };
        // Taken only now, so that a refused push leaves the header whole.
        if let Some(row) =
            continuations.store(std::mem::replace(&mut header.then, Continuation::Deliver))
        {
            header_words.continuation = frames::number_word(u64::from(row.0));
        }
        let (fixed, temporaries) = frame.split_at_mut(frame_header::COUNT);
        header_words.write(fixed);
        fill_temporaries(temporaries, header.code.local_count, argument_count);
        Ok(self.pushed_at(frames::stack_offset(offset), header.counted))
    }

    /// The caller word of a frame pushed now from stack word `start`, its
    /// first argument: the innermost frame's offset and the top, the
    /// top it was pushed at -- that frame's window end or past it
    /// (`frames::caller_link_word`) -- or nil on an empty stack.
    #[inline(always)]
    fn caller_word(&self, start: usize) -> Word {
        match self.stack.innermost {
            NO_FRAME => Word::NIL,
            innermost => frames::caller_link_word(innermost, start, self.stack.frames.top()),
        }
    }

    /// The generation the next frame pushed takes: one past the counter's,
    /// which the push advances only once it has succeeded.
    #[cfg(debug_assertions)]
    fn peek_next_generation(&self) -> u32 {
        self.stack.peek_next_generation()
    }

    /// The kind tag and the operand word a frame of `kind` records: the
    /// one mapping from a kind to its tag, which `activation_kind` reads
    /// back.
    #[inline(always)]
    fn kind_words(&self, kind: ActivationKind<'gc>) -> (u8, Word) {
        let object_word = |object: ObjectReference<'gc>| self.store(Value::object(object));
        match kind {
            ActivationKind::Method(selector) => (
                kind_tag::METHOD,
                frames::number_word(selector.index() as u64),
            ),
            ActivationKind::Closure(closure) => (kind_tag::CLOSURE, object_word(closure)),
            ActivationKind::TopLevel => (kind_tag::TOP_LEVEL, Word::NIL),
            ActivationKind::Initializer(class, InitializerBody::Full) => {
                (kind_tag::INITIALIZER_FULL, object_word(class))
            }
            ActivationKind::Initializer(class, InitializerBody::OwnPart) => {
                (kind_tag::INITIALIZER_OWN_PART, object_word(class))
            }
            ActivationKind::Initializer(class, InitializerBody::Standalone) => {
                (kind_tag::INITIALIZER_STANDALONE, object_word(class))
            }
            ActivationKind::SuperclassClause(class) => {
                (kind_tag::SUPERCLASS_CLAUSE, object_word(class))
            }
            ActivationKind::Factory(class) => (kind_tag::FACTORY, object_word(class)),
        }
    }

    /// Marks `activation` as one whose registers must survive its return: it
    /// created a closure that reads enclosing frames by reference. The VM's
    /// `MakeClosure` is the one caller, and it marks exactly when the block
    /// it creates has a non-zero `outer_reach`.
    pub fn retain_registers(&mut self, activation: ActivationReference) {
        self.check_activation(activation);
        let flags = FrameFlags {
            retains_registers: true,
            ..self.stack.flags(activation.offset)
        };
        self.stack
            .set_header_word(activation.offset, frame_header::FLAGS, flags.word());
    }

    /// Pops the top activation. If it was reified, its frame is divorced:
    /// the frame becomes dead from then on -- uncontinuable, and with no
    /// sender, as a returned Smalltalk context has none. Answers the
    /// activation's continuation, which the run loop applies to its value.
    ///
    /// The registers are copied into the frame only when the activation was
    /// marked (`retain_registers`): something still reaches them by
    /// reference. Otherwise the frame's own stay nil -- a
    /// variable nothing captured is unreachable once its activation
    /// returns, and a captured one the closure carries itself
    /// (`docs/design/execution.md`, "The closure keeps its defining
    /// activation").
    pub fn pop_activation(&mut self, activation: ActivationReference) -> Popped<'gc> {
        self.check_activation(activation);
        debug_assert_eq!(
            activation.offset, self.stack.innermost,
            "only the top activation can be popped"
        );
        let offset = activation.offset as usize;
        let header = self.stack.frames.header(offset);
        let reified = Word::from_bits(header[frame_header::REIFIED]).object_offset();
        let flags = FrameFlags::of(Word::from_bits(header[frame_header::FLAGS]));
        let program_counter = header[frame_header::PROGRAM_COUNTER];
        let continuation = Word::from_bits(header[frame_header::CONTINUATION]);
        let caller = Word::from_bits(header[frame_header::CALLER]);
        if let Some(frame) = reified {
            self.divorce(frame, offset, flags, program_counter);
        }
        // A sender's registers from its slots' end to its window's --
        // where a callee that lay on its slots had its header and window --
        // are dead, and every lowering writes them before it reads them
        // again (`compiler::reference::check_leftovers`), so the pop
        // writes none of them: the pre-pass nils them before a trace.
        self.stack.take_off_words(activation.offset, flags, caller);
        // An offset is below 2^31, so the conversion is exact.
        self.stack.innermost = if caller.is_small_integer() {
            frames::caller_offset_of(caller)
        } else {
            NO_FRAME
        };
        self.stack.live -= 1;
        // A branch, not a subtraction of the flag: the two counts lie side
        // by side, and subtracting both at once is one 16-byte load over
        // the push's two narrower stores, which no store forwards.
        if flags.counted {
            let count = self.stack.machine_words.get(MachineWord::DepthCount);
            self.stack
                .machine_words
                .set(MachineWord::DepthCount, count - 1);
        }
        // A delivering frame names no row, so a send's return reaches no
        // table; every other continuation is taken out of line.
        let then = if continuation.is_small_integer() {
            self.continuations
                .take(ContinuationRow(frames::number_as_u32(continuation)))
        } else {
            Continuation::Deliver
        };
        Popped {
            then,
            counted: flags.counted,
        }
    }

    /// The divorce of the frame object at `frame` from the popped frame at
    /// stack offset `offset`, whose flags are `flags` and whose last stored
    /// program counter is the word `program_counter`. A frame that keeps
    /// its registers keeps those below its body's `local_count` only --
    /// the arguments, the copied values and the named temporaries, all a
    /// block reaching them by reference can name -- and nil for every
    /// scratch register, which may hold a word a callee left.
    fn divorce(
        &mut self,
        frame: NonZeroU64,
        offset: usize,
        flags: FrameFlags,
        program_counter: u64,
    ) {
        let start = offset - usize::from(flags.argument_count);
        let end = self.stack.window_end(frames::stack_offset(offset));
        let Heap {
            space,
            stack,
            tables,
            ..
        } = self;
        let holds_young = flags.retains_registers && {
            let code_index = frames::number_as_usize(
                stack.header_word(frames::stack_offset(offset), frame_header::CODE),
            );
            let Some(entry) = tables.codes.get(code_index).and_then(Option::as_ref) else {
                unreachable!("internal: a running frame names a live code row")
            };
            let scratch = entry.code.local_count..u16::MAX;
            let stack_words = stack.frames.words();
            let space_words = space.words_mut();
            let range = block_range(space_words, header_index(frame));
            copy_registers_out(
                stack_words,
                start,
                offset,
                end,
                std::slice::from_ref(&scratch),
                &mut space_words[range][FIELDS + frame_field::COUNT..],
            )
        };
        let space_words = space.words_mut();
        let range = block_range(space_words, header_index(frame));
        let words = &mut space_words[range][FIELDS..];
        words[frame_field::PROGRAM_COUNTER] = program_counter;
        words[frame_field::SENDER] = Word::NIL.bits();
        words[frame_field::UNCONTINUABLE] = Value::boolean(true).word().bits();
        words[frame_field::MARRIED] = Word::NIL.bits();
        // The divorce copied registers into an object the stack's roots
        // no longer cover: an old frame that now holds a young value is
        // remembered like any other old holder.
        let remember = holds_young && !space::is_young(frame);
        if remember {
            self.remember(frame);
        }
    }

    /// The frame object for a live activation, created married to it on the
    /// first call and answered again on every later one. It has a field for
    /// every register of the activation's window but the header's words
    /// (`frame_field_of_register`), every one nil while the activation
    /// runs, since the live registers are the stack's: so no frame object
    /// ever grows, and the divorce copies the registers in place.
    pub fn reify(&mut self, activation: ActivationReference) -> FrameReference<'gc> {
        self.check_activation(activation);
        let offset = activation.offset;
        if let Some(frame) = self
            .stack
            .header_word(offset, frame_header::REIFIED)
            .object_offset()
        {
            return self.frame_handle(frame);
        }
        let (receiver, current_class) = self.activation_receiver_and_current_class(activation);
        let program_counter = self.activation_program_counter(activation);
        let code = self.activation_code_id(activation);
        // Every register the frame's code can name: its arguments and the
        // registers above the header.
        let register_count = usize::from(self.stack.flags(offset).argument_count)
            + self.stack.register_count(offset);
        let kind = self.activation_kind(activation);
        let home = match kind {
            ActivationKind::Closure(closure) => self.closure_home(closure),
            _ => None,
        };
        let kind = self.frame_kind_of_activation(kind, code);
        // The frame's own row, which `frame_code_id` would answer: a
        // method's body was registered with its method at the push.
        let code = match kind {
            FrameKind::TopLevel => None,
            _ => Some(code),
        };
        let handle = self.allocate_frame(
            FrameFields {
                sender: None,
                home,
                receiver,
                current_class,
                program_counter,
                code,
                kind,
                uncontinuable: false,
            },
            &[],
            register_count,
        );
        self.frame_words_mut(handle)[frame_field::MARRIED] =
            frames::number_word(u64::from(offset)).bits();
        // A returned or suspended frame finds where its registers above the
        // header begin from its code's argument count (`frame_argument_count`),
        // so it must be the count the activation was handed.
        debug_assert_eq!(
            self.frame_argument_count(handle),
            usize::from(self.stack.flags(offset).argument_count),
            "internal: an activation is handed as many arguments as its code takes"
        );
        self.stack.set_header_word(
            offset,
            frame_header::REIFIED,
            Word::with_object_offset(handle.offset),
        );
        handle
    }

    /// Every live activation, innermost first, as a line naming what it
    /// runs and where it stands: a method's selector, or its kind, and its
    /// stored program counter. What `Interpreter::stack_trace` answers.
    pub(crate) fn describe_activations(&self) -> Vec<String> {
        self.stack
            .offsets()
            .map(|offset| {
                let activation = self.activation_handle(offset);
                let program_counter = self.activation_program_counter(activation);
                let what = match self.activation_kind(activation) {
                    ActivationKind::Method(selector) => selector.to_string(),
                    ActivationKind::Closure(_) => "block".to_owned(),
                    ActivationKind::TopLevel => "top level".to_owned(),
                    ActivationKind::Initializer(..) => "initializer".to_owned(),
                    ActivationKind::SuperclassClause(_) => "superclass clause".to_owned(),
                    ActivationKind::Factory(_) => "factory".to_owned(),
                };
                format!("{what} at {program_counter}")
            })
            .collect()
    }

    /// How many activations are live. `Interpreter::run_from_host`, through
    /// which a host's send or instantiation runs, asserts that it leaves it
    /// where it found it.
    pub fn activation_depth(&self) -> usize {
        self.stack.live
    }

    /// The position the next push takes: past every live activation's
    /// frame. A run's floor is the position of its base
    /// (`activation_position`), so the stack holds an activation of the
    /// run exactly while this lies above the floor.
    pub fn activation_stack_top(&self) -> usize {
        self.stack.frames.top()
    }

    /// How many activations count against the depth limit: the live ones
    /// that do, and the suspended ones that do, which count all along so
    /// that resuming one never has to check the limit.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the count never exceeds the depth limit plus its reserve, far below the pointer width"
    )]
    pub fn counted_depth(&self) -> usize {
        self.stack.machine_words.get(MachineWord::DepthCount) as usize
    }

    // ---------- Suspended activations ----------
    //
    // A suspended activation lives in its frame object alone: its registers,
    // program counter and sender in the frame's words, the rest of its
    // stack frame's header in a row of the side table, which the frame's
    // married word names (`frame_field::MARRIED`). `resume_activation`
    // pushes one back onto the stack, married to its frame again
    // (`docs/design/execution.md`, "Activations").

    /// Suspends every activation at a position of `floor` or above into its
    /// frame, the top first, and answers the top's frame; `None` when no
    /// activation is at or above `floor`. Each frame names the frame of the
    /// activation it returns to as its sender: the one below it when that
    /// one is suspended too, the sender a resumed activation was to return
    /// to, or none when it returns below `floor`.
    pub fn suspend_activations(&mut self, floor: usize) -> Option<FrameReference<'gc>> {
        if floor >= self.stack.frames.top() {
            return None;
        }
        let suspended: Vec<u32> = self
            .stack
            .offsets()
            .take_while(|offset| *offset as usize >= floor)
            .collect();
        if suspended.is_empty() {
            return None;
        }
        // Every frame first, so that each sender is a frame by the time
        // the activation above it is suspended.
        let frames: Vec<FrameReference<'gc>> = suspended
            .iter()
            .map(|offset| {
                let activation = self.activation_handle(*offset);
                self.reify(activation)
            })
            .collect();
        let lowest = suspended.len() - 1;
        // The header of the activation suspended just before, the one this
        // one sent to: a callee that lay on this one's slots covers its
        // registers from there up, which are dead.
        let mut callee_header = usize::MAX;
        for (position, offset) in suspended.into_iter().enumerate() {
            debug_assert_eq!(
                offset, self.stack.innermost,
                "internal: the activations are suspended from the top down"
            );
            let frame = frames[position];
            let flags = self.stack.flags(offset);
            let then = match self.stack.continuation(offset) {
                None => Continuation::Deliver,
                Some(row) => self.continuations.take(row),
            };
            let (then, sender) = match then {
                Continuation::ReturnToSuspended(suspended) => {
                    // Its sender is the suspended frame, so its frame names
                    // that one rather than the frame below.
                    debug_assert_eq!(
                        position, lowest,
                        "internal: an activation that returns to a suspended sender is the lowest \
                         one suspended; a frame below it would be suspended into a row nothing names"
                    );
                    let ReturnToSuspended { sender, then } = *suspended;
                    (then, Some(sender))
                }
                then => (then, frames.get(position + 1).copied()),
            };
            let row = match self.suspensions.free.take_smallest() {
                Some(free) => free as usize,
                None => {
                    self.suspensions.rows.push(None);
                    self.suspensions.rows.len() - 1
                }
            };
            let entered_level = self.stack.header_word(offset, frame_header::ENTERED_LEVEL);
            self.suspensions.rows[row] = Some(Suspension {
                frame,
                then,
                counted: flags.counted,
                entered_level: ApplicationIndex::from_raw(frames::number_as_u32(entered_level)),
                retains_registers: flags.retains_registers,
            });
            let sender = match sender {
                Some(sender) => self.store(Value::object(self.frame_object(sender))),
                None => Word::NIL,
            };
            // Read once, as the pop reads it: the caller's offset now, and
            // the top the frame was pushed at when it comes off the words.
            let caller_word = self.stack.header_word(offset, frame_header::CALLER);
            let caller = caller_word
                .is_small_integer()
                .then(|| frames::caller_offset_of(caller_word));
            let header = offset as usize;
            let start = self.stack.frame_start(offset);
            let end = self.stack.window_end(offset).min(callee_header);
            callee_header = header;
            let Heap {
                space,
                stack,
                tables,
                ..
            } = self;
            let stack_words = stack.frames.words();
            let program_counter = stack_words[header + frame_header::PROGRAM_COUNTER];
            let code_index = frames::number_as_usize(stack.header_word(offset, frame_header::CODE));
            let Some(entry) = tables.codes.get(code_index).and_then(Option::as_ref) else {
                unreachable!("internal: a running frame names a live code row")
            };
            // The registers the frame does not read before it writes them,
            // as the pre-pass decides them (`trace_stack`): what a callee
            // or a popped frame left there, which the frame object, traced
            // as it lies, must not keep.
            let tainted = entry
                .code
                .tainted_before(frames::number_as_u32(Word::from_bits(program_counter)));
            let space_words = space.words_mut();
            let range = block_range(space_words, header_index(frame.offset));
            let words = &mut space_words[range][FIELDS..];
            let holds_young = copy_registers_out(
                stack_words,
                start,
                header,
                end,
                &tainted,
                &mut words[frame_field::COUNT..],
            );
            drop(tainted);
            words[frame_field::PROGRAM_COUNTER] = program_counter;
            words[frame_field::SENDER] = sender.bits();
            words[frame_field::MARRIED] = suspended_word(row).bits();
            // The registers and the sender now live in an object the
            // stack's roots no longer cover: an old frame that holds a
            // young value is remembered like any other old holder.
            let sender_is_young = sender.object_offset().is_some_and(space::is_young);
            if !space::is_young(frame.offset) && (holds_young || sender_is_young) {
                self.remember(frame.offset);
            }
            self.stack.take_off_words(offset, flags, caller_word);
            self.stack.innermost = caller.unwrap_or(NO_FRAME);
            self.stack.live -= 1;
        }
        frames.first().copied()
    }

    /// Resumes the suspended activation of `frame`, which must be
    /// suspended (`frame_is_suspended`): pushes it back onto the stack
    /// with the registers and program counter its frame holds and the rest
    /// of its header its row kept, marries the frame to it again and
    /// answers it. `feedback_base` is its feedback base, derived again by
    /// the caller. It starts with no recorded guard, so its first own-slot
    /// access takes the cold check. Its continuation returns to the
    /// frame's sender, resumed in turn, when the frame names one
    /// (`Continuation::ReturnToSuspended`). Neither the depth limit nor
    /// the reservation is checked: the activation counted against the
    /// limit all along, and it is resumed onto the stack it was suspended
    /// from, which holds no more below it than it did then.
    pub fn resume_activation(
        &mut self,
        frame: FrameReference<'gc>,
        feedback_base: FeedbackBase,
    ) -> ActivationReference {
        self.check_frame_origin(frame);
        let row = self
            .suspension_row(frame)
            .expect("internal: only a suspended frame is resumed");
        let suspension = self.suspensions.rows[row]
            .take()
            .expect("internal: a suspended frame names a live row");
        debug_assert!(
            suspension.frame.same(frame),
            "internal: a suspended frame's row names that frame"
        );
        self.suspensions.free.free(u32::try_from(row).expect(
            "internal: a suspension row is one per suspended frame object, far fewer than u32::MAX in the object space",
        ));
        ids::truncate_trailing(&mut self.suspensions.rows, &mut self.suspensions.free);
        let receiver = self.store(self.frame_receiver(frame));
        let current_class = self
            .frame_current_class(frame)
            .map_or(Word::NIL, |class| self.store(Value::object(class)));
        let (kind_tag, operand) = self.kind_words(self.frame_kind(frame).activation_kind());
        let code = self
            .frame_code_id_field(frame)
            .expect("internal: the frame of an activation the runtime pushed names its code");
        // A full collection keeps a suspended frame's code row through the
        // frame, which the table of suspensions roots; a freed row here
        // would leave the resumed activation running no body.
        debug_assert!(
            self.code(code).is_some(),
            "internal: a suspended frame's code row is live"
        );
        let argument_count = self.frame_argument_count(frame);
        let program_counter = self.frame_count_field(frame, frame_field::PROGRAM_COUNTER);
        let then = match self.frame_frame_field(frame, frame_field::SENDER) {
            Some(sender) => Continuation::ReturnToSuspended(Box::new(ReturnToSuspended {
                sender,
                then: suspension.then,
            })),
            None => suspension.then,
        };
        let continuation = self.continuations.store(then);
        // The frame was reified from the activation's window, so it has a
        // field for each of its arguments and each register above the
        // header (`reify`).
        let fields = self.frame_words(frame).len() - frame_field::COUNT;
        let register_count = fields - argument_count;
        let words = argument_count + frame_header::COUNT + register_count;
        let start = self.stack.frames.top();
        let offset = frames::stack_offset(start + argument_count);
        let flags = FrameFlags::new(
            kind_tag,
            u16::try_from(argument_count)
                .expect("internal: a frame's arguments are its code's, within the register ceiling of u16::MAX"),
            suspension.counted,
            suspension.retains_registers,
        );
        #[cfg(debug_assertions)]
        let flags = flags.with_generation(self.peek_next_generation());
        let header_words = FrameHeaderWords {
            receiver,
            current_class,
            operand,
            reified: Word::with_object_offset(frame.offset),
            flags: flags.word(),
            code: frames::number_word(code.index() as u64),
            program_counter: frames::number_word(program_counter as u64),
            feedback_base: frames::number_word(u64::from(feedback_base.raw())),
            caller: self.caller_word(start),
            continuation: continuation
                .map_or(Word::NIL, |row| frames::number_word(u64::from(row.0))),
            guard: frames::number_word(u64::from(DispatchId::NONE.bits())),
            entered_level: frames::number_word(suspension.entered_level.index() as u64),
            register_count: frames::number_word(register_count as u64),
        };
        // The frame's own registers go back to nil, as every married
        // frame's are: the live ones are the stack's again.
        let Heap { space, stack, .. } = self;
        let space_words = space.words_mut();
        let range = block_range(space_words, header_index(frame.offset));
        let words_of_frame = &mut space_words[range][FIELDS..];
        let (_, _, stack_frame) = stack
            .frames
            .push(words)
            .expect("internal: a resumed activation fits on the stack it was suspended from");
        let (arguments, rest) = stack_frame.split_at_mut(argument_count);
        let (fixed, registers) = rest.split_at_mut(frame_header::COUNT);
        header_words.write(fixed);
        let own_registers = &mut words_of_frame[frame_field::COUNT..];
        let (own_arguments, own_above) = own_registers.split_at(argument_count);
        arguments.copy_from_slice(own_arguments);
        registers.copy_from_slice(own_above);
        own_registers.fill(Word::NIL.bits());
        words_of_frame[frame_field::SENDER] = Word::NIL.bits();
        words_of_frame[frame_field::MARRIED] = frames::number_word(u64::from(offset)).bits();
        self.stack.innermost = offset;
        self.stack.live += 1;
        #[cfg(debug_assertions)]
        self.stack.advance_generation();
        self.activation_handle(offset)
    }

    /// The side-table row `frame` names while it is suspended; `None` for a
    /// married or a returned frame.
    fn suspension_row(&self, frame: FrameReference<'gc>) -> Option<usize> {
        self.frame_field(frame, frame_field::MARRIED)
            .as_small_integer()
            .filter(|state| *state < 0)
            .and_then(|state| usize::try_from(-(state + 1)).ok())
    }

    /// Whether `frame`'s activation is suspended into it.
    pub fn frame_is_suspended(&self, frame: FrameReference<'gc>) -> bool {
        self.suspension_row(frame).is_some()
    }

    /// Whether `frame`'s activation has not returned yet: it is married to
    /// a stack frame or suspended into the frame.
    pub fn frame_is_running(&self, frame: FrameReference<'gc>) -> bool {
        self.frame_field(frame, frame_field::MARRIED)
            .as_small_integer()
            .is_some()
    }

    /// The continuation `frame`'s suspended activation applies once it
    /// returns, before it returns to its sender; `None` unless `frame` is
    /// suspended.
    pub fn frame_suspended_continuation(
        &self,
        frame: FrameReference<'gc>,
    ) -> Option<&Continuation<'gc>> {
        let row = self.suspension_row(frame)?;
        self.suspensions
            .rows
            .get(row)?
            .as_ref()
            .map(|suspension| &suspension.then)
    }

    /// The frame of the activation `frame`'s activation returns to: for a
    /// married frame, the suspended sender its stack frame returns to
    /// (`Continuation::ReturnToSuspended`), or else the frame below,
    /// reified; for a suspended one, the sender its frame names. `None`
    /// for a returned frame and for the run's base, which returns to the
    /// host. The `sender` leaf the kernel's handler search walks.
    pub fn frame_sender_frame(
        &mut self,
        frame: FrameReference<'gc>,
    ) -> Option<FrameReference<'gc>> {
        if let Some(activation) = self.frame_activation(frame) {
            if let Some(sender) = self.activation_suspended_sender(activation) {
                return Some(sender);
            }
            let below = self.activation_sender(activation)?;
            return Some(self.reify(below));
        }
        if self.frame_is_suspended(frame) {
            return self.frame_sender(frame);
        }
        None
    }

    /// The suspended frame `activation` returns to, when its sender is
    /// suspended (`Continuation::ReturnToSuspended`); `None` when its
    /// sender is the frame below it, or the host.
    pub(crate) fn activation_suspended_sender(
        &self,
        activation: ActivationReference,
    ) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        let row = self.stack.continuation(activation.offset);
        match self.continuations.get(row) {
            Continuation::ReturnToSuspended(suspended) => Some(suspended.sender),
            _ => None,
        }
    }

    /// The unwind marker of the code `frame`'s activation runs, as
    /// `activation_unwind_marker` reads a stack frame's: its code's while the
    /// frame is married, the frame's recorded code otherwise.
    pub(crate) fn frame_unwind_marker(
        &self,
        frame: FrameReference<'gc>,
    ) -> Option<crate::bytecode::UnwindMarker> {
        match self.frame_activation(frame) {
            Some(activation) => self.activation_unwind_marker(activation),
            None => self.frame_code(frame)?.unwind_marker,
        }
    }

    /// The VM error `frame`'s activation is the `signal` of, as
    /// `activation_signalled_vm_error` answers it for a stack frame: read from
    /// the stack frame while the frame is married, from its side-table row
    /// while it is suspended; `None` once it has returned.
    pub(crate) fn frame_signalled_vm_error(
        &self,
        frame: FrameReference<'gc>,
    ) -> Option<NewspeakError<'gc>> {
        if let Some(activation) = self.frame_activation(frame) {
            return self.activation_signalled_vm_error(activation);
        }
        match self.frame_suspended_continuation(frame)? {
            Continuation::VmErrorSignalled(error) => Some((**error).clone()),
            _ => None,
        }
    }

    /// How many running activations hold a continuation other than
    /// `Continuation::Deliver`; zero outside a run.
    pub fn continuation_count(&self) -> usize {
        self.continuations.count()
    }

    /// Which rows of the continuation table hold a continuation, free ones
    /// included, in row order.
    #[cfg(test)]
    pub(crate) fn continuation_rows(&self) -> Vec<bool> {
        self.continuations
            .rows
            .iter()
            .map(Option::is_some)
            .collect()
    }

    /// How many activations are suspended; zero outside a run.
    pub fn suspension_count(&self) -> usize {
        self.suspensions.rows.iter().flatten().count()
    }

    /// The frame `activation` was reified into, if a block has captured it.
    pub fn activation_reified(
        &self,
        activation: ActivationReference,
    ) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        self.stack
            .header_word(activation.offset, frame_header::REIFIED)
            .object_offset()
            .map(|frame| self.frame_handle(frame))
    }

    pub fn activation_local(&self, activation: ActivationReference, index: u16) -> Value<'gc> {
        self.check_activation(activation);
        let bits = self
            .stack
            .frames
            .word(self.register_slot(activation.offset, index));
        #[cfg(debug_assertions)]
        assert_live_register(activation.offset, index, bits);
        self.load(Word::from_bits(bits))
    }

    /// The register window of `activation`, computed once per switch to it:
    /// its frame's start, its first argument, `argument_count` words below
    /// its header.
    pub fn activation_register_base(&self, activation: ActivationReference) -> RegisterBase {
        self.check_activation(activation);
        RegisterBase {
            index: activation.offset
                - u32::from(self.stack.flags(activation.offset).argument_count),
            #[cfg(debug_assertions)]
            activation,
        }
    }

    /// Register `index` of the window `base` names: one index into the
    /// register file. A debug build checks the base is still its
    /// activation's and the register inside its window.
    #[inline(always)]
    pub fn register(&self, base: RegisterBase, index: u16) -> Value<'gc> {
        #[cfg(debug_assertions)]
        self.check_register(base, index);
        let bits = self.stack.frames.word(base.index as usize + index as usize);
        #[cfg(debug_assertions)]
        assert_live_register(base.activation.offset, index, bits);
        self.load(Word::from_bits(bits))
    }

    #[inline(always)]
    pub fn set_register(&mut self, base: RegisterBase, index: u16, value: Value<'gc>) {
        #[cfg(debug_assertions)]
        self.check_register(base, index);
        let word = self.store(value);
        self.stack
            .frames
            .set_word(base.index as usize + index as usize, word.bits());
    }

    #[cfg(debug_assertions)]
    fn check_register(&self, base: RegisterBase, index: u16) {
        self.check_activation(base.activation);
        let _ = self.register_slot(base.activation.offset, index);
    }

    pub fn activation_set_local(
        &mut self,
        activation: ActivationReference,
        index: u16,
        value: Value<'gc>,
    ) {
        self.check_activation(activation);
        let slot = self.register_slot(activation.offset, index);
        let word = self.store(value);
        self.stack.frames.set_word(slot, word.bits());
    }

    pub fn activation_receiver(&self, activation: ActivationReference) -> Value<'gc> {
        self.check_activation(activation);
        self.load(
            self.stack
                .header_word(activation.offset, frame_header::RECEIVER),
        )
    }

    /// The receiver and `current_class` together. `decide_lexical_send`'s
    /// head needs both on every implicit-receiver send; fetching them
    /// separately measured 13% slower on `bench_loop.ns`
    /// (`docs/design/execution.md`, "A 13% regression, found and
    /// recovered").
    pub fn activation_receiver_and_current_class(
        &self,
        activation: ActivationReference,
    ) -> (Value<'gc>, Option<ClassReference<'gc>>) {
        self.check_activation(activation);
        let header = self.stack.frames.header(activation.offset as usize);
        let receiver = Word::from_bits(header[frame_header::RECEIVER]);
        let current_class = Word::from_bits(header[frame_header::CURRENT_CLASS]);
        (
            self.load(receiver),
            current_class
                .object_offset()
                .map(|class| self.object_handle(class)),
        )
    }

    pub fn activation_current_class(
        &self,
        activation: ActivationReference,
    ) -> Option<ClassReference<'gc>> {
        self.check_activation(activation);
        self.stack
            .header_word(activation.offset, frame_header::CURRENT_CLASS)
            .object_offset()
            .map(|class| self.object_handle(class))
    }

    /// The receiver and the guard `activation` recorded for it
    /// (`frame_header::GUARD`), fetched together: an own-slot
    /// instruction's hot path needs both and nothing else.
    #[inline(always)]
    pub(crate) fn activation_receiver_and_recorded_guard(
        &self,
        activation: ActivationReference,
    ) -> (Value<'gc>, DispatchId) {
        self.check_activation(activation);
        let header = self.stack.frames.header(activation.offset as usize);
        let receiver = Word::from_bits(header[frame_header::RECEIVER]);
        let guard = frames::number_as_u32(Word::from_bits(header[frame_header::GUARD]));
        (self.load(receiver), DispatchId::from_bits(guard))
    }

    /// The full-width index of the application `activation` was entered at
    /// (`frame_header::ENTERED_LEVEL`); `UNREGISTERED` for code that
    /// addresses no own slot.
    pub(crate) fn activation_entered_level(
        &self,
        activation: ActivationReference,
    ) -> ApplicationIndex {
        self.check_activation(activation);
        self.activation_entered_level_of(activation.offset)
    }

    /// Records `guard` as the one `activation`'s own-slot instructions
    /// compare the receiver's header against: a cold check that found the
    /// receiver still has the entered level's slots records its current
    /// guard, so the accesses after it take the hot path again.
    pub(crate) fn activation_record_guard(
        &mut self,
        activation: ActivationReference,
        guard: DispatchId,
    ) {
        self.check_activation(activation);
        self.stack.set_header_word(
            activation.offset,
            frame_header::GUARD,
            frames::number_word(u64::from(guard.bits())),
        );
    }

    /// Whether `receiver`'s application chain -- its class's application
    /// and every superclass's, read through the class words -- contains the
    /// application whose full-width index is `level`: whether it still has
    /// the slots of the level some code was compiled against. An immediate
    /// has no such chain and answers `false`, as does `UNREGISTERED`. The
    /// one walk of the cold checks of every slot access whose index was
    /// decided without looking at the receiver (`docs/design/object-model.md`,
    /// "what a slot access checks").
    ///
    /// The level's successor passes as well: an activation entered before
    /// an edit holds the old level, and an edit that reached the install
    /// kept that level's slots, so a receiver whose chain holds the
    /// replacement still has the layout the activation was compiled against.
    pub(crate) fn receiver_has_level(&self, receiver: Value<'gc>, level: ApplicationIndex) -> bool {
        let Some(object) = receiver.as_object() else {
            return false;
        };
        if level == ApplicationIndex::UNREGISTERED {
            return false;
        }
        let successor = self.successor(level);
        let mut current = Some(self.object_class(object));
        while let Some(class) = current {
            let held = self.class_application_index(class);
            if held == Some(level) || (successor.is_some() && held == successor) {
                return true;
            }
            current = self.class_superclass(class);
        }
        false
    }

    /// The absolute field index a compiled `InitializeSlot`'s mixin-relative
    /// `own_index` is offset by: `MixinApplication.own_slot_base` of the
    /// application the activation was entered at
    /// (`frame_header::ENTERED_LEVEL`). The two bodies that contain
    /// `InitializeSlot` run with exactly the class whose slots they write
    /// there -- an instance initializer with the level it initializes, one
    /// activation per level of the chain, and a lazy slot's getter (§6.3.3)
    /// with its defining class -- so the base is derived here rather than
    /// carried on the activation: carrying it made every push store a field,
    /// and every method push ask whether its method is a lazy getter, for the
    /// one instruction that reads it (`docs/design/execution.md`, "What the
    /// platform module cost"). Read off the entered application, never the
    /// class object's current one: the own-slot check
    /// (`interpreter::locals::check_own_slots`) validates the receiver
    /// against the entered application, and an install can rewrite the
    /// class's application while this activation still runs code compiled
    /// for the old one, so the base must be the one the check vouched for.
    /// An activation of code that addresses no own slot, which no own-slot
    /// instruction runs in, answers 0.
    pub fn activation_slot_base(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        self.tables
            .applications
            .get(self.activation_entered_level_of(activation.offset).index())
            .and_then(Option::as_ref)
            .map_or(0, |application| application.own_slot_base)
    }

    /// Where `activation`'s code family starts in the heap's cache table.
    pub(crate) fn activation_feedback_base(&self, activation: ActivationReference) -> FeedbackBase {
        self.check_activation(activation);
        FeedbackBase::from_raw(frames::number_as_u32(
            self.stack
                .header_word(activation.offset, frame_header::FEEDBACK_BASE),
        ))
    }

    pub fn activation_set_program_counter(
        &mut self,
        activation: ActivationReference,
        program_counter: usize,
    ) {
        self.check_activation(activation);
        self.stack.set_header_word(
            activation.offset,
            frame_header::PROGRAM_COUNTER,
            frames::number_word(program_counter as u64),
        );
    }

    /// A closure activation's return target -- its closure's `home` -- and
    /// `None` for every other kind, which is its own return target.
    pub fn activation_home(&self, activation: ActivationReference) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        match self.activation_kind(activation) {
            ActivationKind::Closure(closure) => self.closure_home(closure),
            _ => None,
        }
    }

    /// The frame a closure activation's closure captured, or `None` for any
    /// other kind. The first hop of `locals::frame_at_depth`.
    pub fn activation_captured(
        &self,
        activation: ActivationReference,
    ) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        match self.activation_kind(activation) {
            ActivationKind::Closure(closure) => self.closure_captured_frame(closure),
            _ => None,
        }
    }

    /// The body `activation` runs.
    pub fn activation_code(
        &self,
        activation: ActivationReference,
    ) -> std::rc::Rc<crate::bytecode::CompiledCode> {
        self.check_activation(activation);
        self.running_code(activation).code.clone()
    }

    /// The code table's row of the body `activation` runs: its frame's
    /// code word.
    #[inline(always)]
    pub(crate) fn activation_code_id(&self, activation: ActivationReference) -> CodeId {
        CodeId::from_raw(frames::number_as_u32(
            self.stack
                .header_word(activation.offset, frame_header::CODE),
        ))
    }

    /// The application the frame at `offset` was entered at
    /// (`frame_header::ENTERED_LEVEL`).
    #[inline(always)]
    fn activation_entered_level_of(&self, offset: u32) -> ApplicationIndex {
        ApplicationIndex::from_raw(frames::number_as_u32(
            self.stack.header_word(offset, frame_header::ENTERED_LEVEL),
        ))
    }

    /// The code table's row of the body `activation` runs, lent for one
    /// read.
    #[inline]
    fn running_code(&self, activation: ActivationReference) -> &CodeEntry {
        let id = self.activation_code_id(activation);
        self.tables.codes[id.index()]
            .as_ref()
            .expect("internal: a running activation's code row is never freed")
    }

    /// The unwind marker of the code `activation` runs, read afresh on every
    /// call: the unwinder asks it of every activation it pops, so an
    /// activation without one costs it this one test.
    pub(crate) fn activation_unwind_marker(
        &self,
        activation: ActivationReference,
    ) -> Option<crate::bytecode::UnwindMarker> {
        self.check_activation(activation);
        self.running_code(activation).code.unwind_marker
    }

    /// Replaces what `activation` does with its value when it returns. The
    /// unwinder's one use: the activation it pushes to run an `ensure:`
    /// argument that is no block is pushed as a method, which delivers, and
    /// its return has to carry the unwind on instead. The return to a
    /// suspended sender (`Continuation::ReturnToSuspended`) stays: only
    /// the continuation inside it is replaced, so the sender is still
    /// resumed when the activation returns.
    pub(crate) fn activation_set_continuation(
        &mut self,
        activation: ActivationReference,
        then: Continuation<'gc>,
    ) {
        self.check_activation(activation);
        let offset = activation.offset;
        match self.stack.continuation(offset) {
            Some(row) => match self.continuations.rows[row.0 as usize].as_mut() {
                Some(Continuation::ReturnToSuspended(suspended)) => suspended.then = then,
                _ if matches!(then, Continuation::Deliver) => {
                    self.continuations.take(row);
                    self.stack.set_continuation(offset, None);
                }
                _ => self.continuations.replace(row, then),
            },
            None => {
                let row = self.continuations.store(then);
                self.stack.set_continuation(offset, row);
            }
        }
    }

    /// The continuation `activation` applies to its value when it returns,
    /// looked through a return to a suspended sender
    /// (`Continuation::ReturnToSuspended`), which wraps it once the
    /// activation is resumed: what the activation was pushed to do, whether
    /// its sender is on the stack or suspended.
    fn activation_own_continuation(&self, activation: ActivationReference) -> &Continuation<'gc> {
        self.check_activation(activation);
        let row = self.stack.continuation(activation.offset);
        match self.continuations.get(row) {
            Continuation::ReturnToSuspended(suspended) => &suspended.then,
            own => own,
        }
    }

    /// Whether the unwinder pushed `activation` to signal a VM error
    /// (`Continuation::VmErrorSignalled`): what the depth limit's reserve
    /// asks, without cloning the error `activation_signalled_vm_error`
    /// answers.
    pub(crate) fn activation_signals_a_vm_error(&self, activation: ActivationReference) -> bool {
        matches!(
            self.activation_own_continuation(activation),
            Continuation::VmErrorSignalled(_)
        )
    }

    /// The VM error `activation` is the `signal` of, if the unwinder pushed
    /// it to signal one (`Continuation::VmErrorSignalled`).
    pub(crate) fn activation_signalled_vm_error(
        &self,
        activation: ActivationReference,
    ) -> Option<NewspeakError<'gc>> {
        match self.activation_own_continuation(activation) {
            Continuation::VmErrorSignalled(error) => Some((**error).clone()),
            _ => None,
        }
    }

    /// The activation directly above `activation` on the stack, if any:
    /// the one it sent to, while it waits for it.
    pub fn activation_above(&self, activation: ActivationReference) -> Option<ActivationReference> {
        self.check_activation(activation);
        // Walked down from the innermost: a callee that lies on its
        // sender's slots starts inside the sender's window, so the frame
        // above is not where the window ends. Only the depth limit's
        // reserve asks, for a frame near the top.
        let mut above = None;
        for offset in self.stack.offsets() {
            if offset == activation.offset {
                return above.map(|above| self.activation_handle(above));
            }
            above = Some(offset);
        }
        None
    }

    /// The `program_counter` last stored for `activation` -- at a send, the
    /// send's own index, which is where the run loop resumes it from when its
    /// callee returns.
    pub fn activation_program_counter(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        frames::number_as_usize(
            self.stack
                .header_word(activation.offset, frame_header::PROGRAM_COUNTER),
        )
    }

    /// The innermost live activation, if any.
    pub fn top_activation(&self) -> Option<ActivationReference> {
        let innermost = self.stack.innermost;
        (innermost != NO_FRAME).then(|| self.activation_handle(innermost))
    }

    /// Where `activation` lies on the stack: the word offset of its frame's
    /// start, its first argument, which grows with depth -- a callee on its
    /// sender's slots starts above the sender's header, any other above
    /// the sender's window -- so positions order activations as their
    /// depths do, and the top is back at a frame's position once the frame
    /// and everything above it are gone.
    /// A position is no count: the depth is `activation_depth`. The run loop
    /// compares a non-local return's target against the position its base
    /// was at -- its floor -- to tell its own activations from its callers',
    /// and the stack's top against it (`activation_stack_top`) to tell
    /// whether its base has returned.
    pub fn activation_position(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        self.stack.frame_start(activation.offset)
    }

    /// The activation `activation` returns to: the frame below it, since a
    /// callee -- a method, a block a leaf activated, an `ensure:` block the
    /// unwinder runs -- is always pushed above the activation it answers
    /// to. `None` for the bottom of the stack.
    pub fn activation_sender(
        &self,
        activation: ActivationReference,
    ) -> Option<ActivationReference> {
        self.check_activation(activation);
        self.stack
            .caller(activation.offset)
            .map(|caller| self.activation_handle(caller))
    }

    /// What `activation` was activated for.
    pub fn activation_kind(&self, activation: ActivationReference) -> ActivationKind<'gc> {
        self.check_activation(activation);
        let offset = activation.offset;
        let tag = self.stack.flags(offset).kind_tag;
        let operand = self.stack.header_word(offset, frame_header::OPERAND);
        let object = || {
            self.object_handle(
                operand
                    .object_offset()
                    .expect("internal: the operand of a closure's or a class's frame names it"),
            )
        };
        match tag {
            kind_tag::METHOD => {
                ActivationKind::Method(Symbol::from_index(frames::number_as_u32(operand)))
            }
            kind_tag::CLOSURE => ActivationKind::Closure(object()),
            kind_tag::TOP_LEVEL => ActivationKind::TopLevel,
            kind_tag::INITIALIZER_FULL => {
                ActivationKind::Initializer(object(), InitializerBody::Full)
            }
            kind_tag::INITIALIZER_OWN_PART => {
                ActivationKind::Initializer(object(), InitializerBody::OwnPart)
            }
            kind_tag::INITIALIZER_STANDALONE => {
                ActivationKind::Initializer(object(), InitializerBody::Standalone)
            }
            kind_tag::SUPERCLASS_CLAUSE => ActivationKind::SuperclassClause(object()),
            kind_tag::FACTORY => ActivationKind::Factory(object()),
            _ => unreachable!("internal: a frame's kind tag is one `kind_words` wrote"),
        }
    }

    /// The `FrameKind` a frame of a running activation of `kind`, whose
    /// body is the row `code`, records: the view with a method's selector
    /// replaced by the method the row was registered with at the push.
    fn frame_kind_of_activation(&self, kind: ActivationKind<'gc>, code: CodeId) -> FrameKind<'gc> {
        match kind {
            ActivationKind::Method(_) => FrameKind::Method(
                self.code_method(code)
                    .expect("internal: a method's activation registered its row with its method"),
            ),
            ActivationKind::Closure(closure) => FrameKind::Closure(closure),
            ActivationKind::TopLevel => FrameKind::TopLevel,
            ActivationKind::Initializer(class, body) => FrameKind::Initializer(class, body),
            ActivationKind::SuperclassClause(class) => FrameKind::SuperclassClause(class),
            ActivationKind::Factory(class) => FrameKind::Factory(class),
        }
    }

    /// Whether `activation` runs the own code of the body-less declaration
    /// whose class object is `class` (`class_declaration_scope`): the
    /// declaration's full initializer entered at `class`, or a block that
    /// body created, at any nesting. A block is followed along its captured
    /// frames, each of which runs the body the block is lexically nested in
    /// (`MakeClosure` captures its creator; `image::decode::verify_graph`
    /// holds a decoded closure to the same), so the walk climbs the code's
    /// nesting and ends at its root. Which code runs decides it, so every
    /// activation of one body answers alike, and the sends read it off the
    /// code (`CompiledCode::is_declaration_code`): this answer is what a
    /// debug build holds the code's to. `false` for every other
    /// activation.
    pub(crate) fn activation_runs_declaration_code(
        &self,
        activation: ActivationReference,
        class: ClassReference<'gc>,
    ) -> bool {
        self.check_activation(activation);
        let closure = match self.activation_kind(activation) {
            ActivationKind::Initializer(level, InitializerBody::Full) => return level.same(class),
            ActivationKind::Closure(closure) => closure,
            _ => return false,
        };
        self.closure_captured_frame(closure)
            .is_some_and(|frame| self.frame_runs_declaration_code(frame, class))
    }

    /// `activation_runs_declaration_code` for the activation `frame` is
    /// the frame object of, read off the frame alone: what
    /// `image::decode::verify_graph` holds a decoded frame's body to, so
    /// that no decoded frame runs a body-less declaration's own code
    /// anywhere else, nor other code where it would. The walk is bounded
    /// by the static block nesting, which a decoded closure's captured
    /// frame is held to.
    pub(crate) fn frame_runs_declaration_code(
        &self,
        frame: FrameReference<'gc>,
        class: ClassReference<'gc>,
    ) -> bool {
        use crate::image::opcodes::{FRAME_CLOSURE, FRAME_INITIALIZER};
        let mut frame = Some(frame);
        while let Some(current) = frame {
            let operand = self
                .frame_field(current, frame_field::KIND_OPERAND)
                .as_object();
            match u8::try_from(self.frame_count_field(current, frame_field::KIND)) {
                Ok(FRAME_CLOSURE) => {
                    frame = operand.and_then(|closure| self.closure_captured_frame(closure));
                }
                Ok(FRAME_INITIALIZER) => return operand.is_some_and(|level| level.same(class)),
                _ => return false,
            }
        }
        false
    }

    /// Whether `activation` runs a method or a superclass clause -- not a
    /// block, an initializer, a factory or a top-level activation.
    pub(crate) fn activation_runs_method_or_clause(&self, activation: ActivationReference) -> bool {
        self.check_activation(activation);
        matches!(
            self.stack.flags(activation.offset).kind_tag,
            kind_tag::METHOD | kind_tag::SUPERCLASS_CLAUSE
        )
    }

    /// The field of a frame object of a body taking `argument_count`
    /// arguments that holds `register`, counted from the frame's first
    /// register field: an argument is its own field, a register above the
    /// header `HEADER_REGISTERS` fields lower, and a number the header's
    /// words take, which no instruction names, has none. A frame object
    /// holds the registers a stack frame does, without the header between
    /// them, so its fields are its arguments and then its registers above
    /// the header (`docs/design/object-model.md`, "The object layout").
    #[inline]
    pub fn frame_field_of_register(argument_count: u16, register: u16) -> Option<usize> {
        let (argument_count, register) = (usize::from(argument_count), usize::from(register));
        if register < argument_count {
            Some(register)
        } else {
            register
                .checked_sub(argument_count + frame_header::COUNT)
                .map(|above| argument_count + above)
        }
    }

    /// A register of a reified frame: the live stack slot while its
    /// activation runs, the frame's own copy while it is suspended and
    /// after it has returned. This is
    /// the path a block takes to its method's variables, and the only
    /// register access that branches.
    ///
    /// A returned activation that was never marked kept no registers at all
    /// (`pop_activation`), and a decoded frame's register file is whatever
    /// the image said. No code the compiler produces reaches a register
    /// either one lacks, nor names a number the header's words take --
    /// `MakeClosure` marks every activation a by-reference block is created
    /// in -- so an index past the file, or one the header takes, which no
    /// field holds (`frame_field_of_register`), is either a crafted image
    /// or a compiler defect, and answering `Nil` rather than panicking is
    /// what keeps the first from aborting the process.
    pub fn frame_local(&self, frame: FrameReference<'gc>, index: u16) -> Value<'gc> {
        let words = self.frame_words(frame);
        let word = match self.married_offset(words) {
            Some(offset) => {
                let bits = self.stack.frames.word(self.register_slot(offset, index));
                #[cfg(debug_assertions)]
                assert_live_register(offset, index, bits);
                Word::from_bits(bits)
            }
            None => self
                .dead_frame_field(frame, index)
                .and_then(|field| words.get(frame_field::COUNT + field))
                .map_or(Word::NIL, |bits| Word::from_bits(*bits)),
        };
        self.load(word)
    }

    /// The field of `frame`, whose activation is not on the stack, that
    /// holds register `index`, counted from its first register field.
    fn dead_frame_field(&self, frame: FrameReference<'gc>, index: u16) -> Option<usize> {
        let argument_count = u16::try_from(self.frame_argument_count(frame))
            .expect("internal: a frame's argument count is at most its code's, a u16");
        Self::frame_field_of_register(argument_count, index)
    }

    /// `frame_local`'s twin, and silently a no-op for an index a dead
    /// frame's register file does not have, for the reason given there.
    pub fn frame_set_local(&mut self, frame: FrameReference<'gc>, index: u16, value: Value<'gc>) {
        // Copied out first, so neither arm below holds a borrow of the
        // space while it writes.
        let live = self.married_offset(self.frame_words(frame));
        let word = self.store(value);
        match live {
            Some(offset) => {
                let slot = self.register_slot(offset, index);
                self.stack.frames.set_word(slot, word.bits());
            }
            None => {
                let Some(field) = self.dead_frame_field(frame, index) else {
                    return;
                };
                if let Some(slot) = self
                    .frame_words_mut(frame)
                    .get_mut(frame_field::COUNT + field)
                {
                    *slot = word.bits();
                    self.write_barrier(frame.offset, word);
                }
            }
        }
    }

    /// The live activation `frame` is married to, or `None` while that
    /// activation is suspended (`frame_is_suspended`) and once it has
    /// returned -- and for every frame the image decoder built, since none
    /// of those ever ran here. The unwinder asks it before every pop
    /// whether its target is the top; whether a frame's activation is
    /// running at all is `frame_is_running`.
    pub fn frame_activation(&self, frame: FrameReference<'gc>) -> Option<ActivationReference> {
        self.married_offset(self.frame_words(frame))
            .map(|offset| self.activation_handle(offset))
    }

    // ---------- The safepoint ----------

    #[inline]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.machine_words.get(MachineWord::SafepointRequested) != 0
    }

    /// Whether a real request is pending -- a collection's, an
    /// interrupt's -- rather than a stress mode's hold (`STRESS_HOLD`),
    /// which the run loop takes at its next poll all the same.
    pub(crate) fn real_safepoint_requested(&self) -> bool {
        self.machine_words.get(MachineWord::SafepointRequested) == 1
    }

    /// A request served (`Interpreter::safepoint`).
    pub(crate) fn clear_safepoint_request(&mut self) {
        self.machine_words.set(MachineWord::SafepointRequested, 0);
    }

    /// Holds the request set for a stress mode, or drops it. A held
    /// request reads `STRESS_HOLD`, not a real request's 1: every poll
    /// takes it, but a compiled callee's machine entry refuses only a real
    /// one (`jit::sends::prologue`), so that under a stress mode compiled
    /// frames still nest, and the collections the mode makes at the polls
    /// inside them run over the frames the exit materializes.
    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, requested: bool) {
        let word = if requested { STRESS_HOLD } else { 0 };
        self.machine_words
            .set(MachineWord::SafepointRequested, word);
    }

    /// Asks the run loop's next safepoint to take its slow path.
    fn request_safepoint(&mut self) {
        self.machine_words.set(MachineWord::SafepointRequested, 1);
    }

    // ---------- What machine code is entered with ----------

    /// The address of the machine words (`machine_words.rs`), whose
    /// safepoint request machine code's poll reads
    /// (`jit::stubs::BlockWord::MachineWords`), exposed as the other
    /// addresses machine code is entered with are.
    #[cfg(ns_jit)]
    pub(crate) fn machine_words_address(&self) -> usize {
        self.machine_words.address()
    }

    /// The address of eden's bump cursor, its top word then its limit
    /// word (`jit::stubs::BlockWord::EdenCursor`), which machine code
    /// bumps in place: fixed for the heap's life, since the object space
    /// keeps its reservation.
    #[cfg(ns_jit)]
    pub(crate) fn eden_cursor_address(&self) -> usize {
        self.space.eden_cursor_address()
    }

    /// The block machine code allocates an object of `word_count` words
    /// after its header in (`jit::macro_assembler`'s `bump_allocate`):
    /// its bytes, its overflow word when the header cannot hold the count,
    /// and whether the inline bump may place it at all -- only below the
    /// large-object threshold, past which `allocate_block` places every
    /// object old.
    #[cfg(ns_jit)]
    pub(crate) fn machine_block(word_count: usize) -> MachineBlock {
        let bytes = allocation_words(word_count).saturating_mul(8);
        MachineBlock {
            bytes: i32::try_from(bytes).unwrap_or(i32::MAX),
            overflow_word: (word_count >= WORD_COUNT_OVERFLOW).then(|| overflow_word(word_count)),
            inline: bytes < LARGE_OBJECT_BYTES,
        }
    }

    /// The bits of a header word that say it is followed by `word_count`
    /// words, 255 for a count in an overflow word: what an allocation in
    /// machine code adds to a header of no words (`MachineWord::ArrayHeader`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_word_count_bits(word_count: usize) -> u64 {
        Header::new(DispatchId::NONE, Format::FixedPointers, 0, word_count).0
    }

    /// The object space's base address, every handle's origin
    /// (`jit::stubs::BlockWord::ObjectSpaceBase`).
    #[cfg(ns_jit)]
    pub(crate) fn object_space_base_address(&self) -> usize {
        self.space.base_address()
    }

    /// The activation stack's base address
    /// (`jit::stubs::BlockWord::StackBase`).
    #[cfg(ns_jit)]
    pub(crate) fn activation_stack_base_address(&self) -> usize {
        self.stack.frames.base_address()
    }

    /// Where `base`'s registers start, in words from the activation
    /// stack's base (`jit::stubs::BlockWord::RegisterBase`).
    #[cfg(ns_jit)]
    pub(crate) fn register_base_words(base: RegisterBase) -> u64 {
        u64::from(base.index)
    }

    /// The register number machine code reads a frame's current class at:
    /// the header word `CURRENT_CLASS`, placed as `receiver_register`
    /// places the receiver.
    #[cfg(ns_jit)]
    pub(crate) fn current_class_register(argument_count: u16) -> u16 {
        let Ok(current_class) = u16::try_from(frame_header::CURRENT_CLASS) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        argument_count + current_class
    }

    /// The register number machine code reads a frame's operand at: the
    /// header word `OPERAND` -- a block's frame's closure, a superclass
    /// clause's class -- placed as `receiver_register` places the
    /// receiver.
    #[cfg(ns_jit)]
    pub(crate) fn operand_register(argument_count: u16) -> u16 {
        let Ok(operand) = u16::try_from(frame_header::OPERAND) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        argument_count + operand
    }

    /// The register numbers of a frame's recorded guard and entered level
    /// (`frame_header::GUARD`, `frame_header::ENTERED_LEVEL`), placed as
    /// `receiver_register` places the receiver: what a compiled prologue
    /// writes for code that addresses its own slots, and what its own-slot
    /// instructions read (`jit::baseline`).
    #[cfg(ns_jit)]
    pub(crate) fn own_slot_registers(argument_count: u16) -> (u16, u16) {
        let (Ok(guard), Ok(entered_level)) = (
            u16::try_from(frame_header::GUARD),
            u16::try_from(frame_header::ENTERED_LEVEL),
        ) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        (argument_count + guard, argument_count + entered_level)
    }

    /// The words a compiled prologue writes as the recorded guard of a
    /// frame whose code addresses its own slots -- `DispatchId::NONE`, so
    /// that the first own-slot access takes the cold check -- and as its
    /// entered level when its current class names no application,
    /// `ApplicationIndex::UNREGISTERED`; a class object's application word
    /// is already the entered level's word otherwise.
    #[cfg(ns_jit)]
    pub(crate) fn machine_own_slot_words() -> (u64, u64) {
        (
            frames::number_word(u64::from(DispatchId::NONE.bits())).bits(),
            frames::number_word(ApplicationIndex::UNREGISTERED.index() as u64).bits(),
        )
    }

    /// The register number machine code reads a frame's receiver at: the
    /// header word `RECEIVER`, which lies `argument_count` registers above
    /// register 0, since the header follows the arguments
    /// (`bytecode::HEADER_REGISTERS`).
    #[cfg(ns_jit)]
    pub(crate) fn receiver_register(argument_count: u16) -> u16 {
        let Ok(receiver) = u16::try_from(frame_header::RECEIVER) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        argument_count + receiver
    }

    /// Where machine code finds what it reads of an object and a value
    /// (`jit::macro_assembler`'s `load_guard`, `load_class`,
    /// `walk_superclasses`, `load_enclosing_object` and
    /// `load_lexical_class`, and the guards of `jit::sends`): the words of an object at its handle's offset
    /// from the object space's base, and the tags of a value's word.
    #[cfg(ns_jit)]
    pub(crate) fn machine_layout() -> MachineLayout {
        let displacement = |word: usize| {
            let Ok(displacement) = i32::try_from(word * 8) else {
                unreachable!("internal: an object's first words lie near its start");
            };
            displacement
        };
        let small = |bits: u64| {
            let Ok(bits) = i32::try_from(bits) else {
                unreachable!("internal: a tag and a mask fit 32 bits");
            };
            bits
        };
        MachineLayout {
            header_displacement: displacement(HEADER),
            guard_mask: small(header::GUARD_MASK),
            class_word_displacement: displacement(CLASS),
            superclass_displacement: displacement(FIELDS + class_field::SUPERCLASS),
            enclosing_displacement: displacement(FIELDS + class_field::ENCLOSING),
            declaring_class_displacement: displacement(FIELDS + class_field::DECLARING_CLASS),
            application_displacement: displacement(FIELDS + class_field::APPLICATION),
            tag_mask: small(value::machine::TAG_MASK),
            small_integer_tag: small(value::machine::SMALL_INTEGER_TAG),
            small_float_tag: small(value::machine::SMALL_FLOAT_TAG),
            small_integer_index: SMALL_INTEGER_DISPATCH_INDEX as u64,
            small_float_index: FLOAT_DISPATCH_INDEX as u64,
            tag_bits: small_count(value::machine::TAG_BITS),
            instance_format_bits: small(header::FORMAT_BITS),
            word_count_shift: small_count(header::WORD_COUNT_SHIFT),
            fields_displacement: displacement(FIELDS),
            indexable_pointers_format: small(header::INDEXABLE_POINTERS_BITS),
            word_count_overflow: small(WORD_COUNT_OVERFLOW as u64),
            nursery_end: small(NURSERY_END as u64),
            remembered_bit: u32::try_from(header::REMEMBERED_BIT)
                .map(u32::cast_signed)
                .unwrap_or_else(|_| {
                    unreachable!("internal: the remembered bit lies in the header's low half")
                }),
            nil_word: small(Word::NIL.bits()),
            aligned_bytes_format: small(header::ALIGNED_BYTES_BITS),
            unused_bytes_shift: small_count(header::UNUSED_SHIFT),
            unused_bytes_mask: small(header::UNUSED_BYTES_MASK),
            byte_data_alignment: displacement(object::ALIGNMENT_WORDS),
            // The words after the header `byte_array_extent` does not count
            // as data: the block's but the header's own.
            byte_data_reserve_bytes: displacement(FIELDS - 1 + object::ALIGNMENT_RESERVE),
            application_row_shift: application_ids::ROW_SHIFT,
            application_dispatch_id_displacement: application_ids::RowWord::DispatchId
                .displacement(),
            application_slot_base_displacement: application_ids::RowWord::SlotBase.displacement(),
            application_flags_displacement: application_ids::RowWord::Flags.displacement(),
            application_value_slots_flag: small(application_ids::ROW_FLAG_VALUE_SLOTS),
            application_instance_header_displacement: application_ids::RowWord::InstanceHeader
                .displacement(),
            class_format: small(header::CLASS_BITS),
            closure_format: small(header::CLOSURE_BITS),
            frame_format: small(header::FRAME_BITS),
            builtin_displacement: displacement(FIELDS + class_field::BUILTIN),
            value_class_refusal_displacement: displacement(
                FIELDS + class_field::VALUE_CLASS_REFUSAL,
            ),
            frame_sender_displacement: displacement(FIELDS + frame_field::SENDER),
            frame_home_displacement: displacement(FIELDS + frame_field::HOME),
            frame_receiver_displacement: displacement(FIELDS + frame_field::RECEIVER),
            frame_current_class_displacement: displacement(FIELDS + frame_field::CURRENT_CLASS),
            frame_program_counter_displacement: displacement(FIELDS + frame_field::PROGRAM_COUNTER),
            frame_code_id_displacement: displacement(FIELDS + frame_field::CODE_ID),
            frame_kind_displacement: displacement(FIELDS + frame_field::KIND),
            frame_kind_operand_displacement: displacement(FIELDS + frame_field::KIND_OPERAND),
            frame_uncontinuable_displacement: displacement(FIELDS + frame_field::UNCONTINUABLE),
            frame_married_displacement: displacement(FIELDS + frame_field::MARRIED),
            frame_fixed_fields: frame_field::COUNT,
            closure_code_id_displacement: displacement(FIELDS + closure_field::CODE_ID),
            closure_captured_frame_displacement: displacement(
                FIELDS + closure_field::CAPTURED_FRAME,
            ),
            closure_feedback_base_displacement: displacement(FIELDS + closure_field::FEEDBACK_BASE),
            closure_fixed_fields: closure_field::COUNT,
        }
    }

    /// The kind words `MakeClosure` writes into the frame objects it
    /// reifies in machine code, as `frame_fixed_words` writes them
    /// (`MachineFrameKinds`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_frame_kinds() -> MachineFrameKinds {
        use crate::image::opcodes::{
            FRAME_CLOSURE, FRAME_FACTORY, FRAME_METHOD, FRAME_SUPERCLASS_CLAUSE,
        };
        let word = |tag: u8| count_word(usize::from(tag)).bits();
        let initializer = |tag: u8, body: InitializerBody| (tag, word(body.frame_tag()));
        let Ok(false_word) = i32::try_from(Value::boolean(false).word().bits()) else {
            unreachable!("internal: false's word is a small constant");
        };
        MachineFrameKinds {
            method: word(FRAME_METHOD),
            closure: word(FRAME_CLOSURE),
            factory: word(FRAME_FACTORY),
            superclass_clause: word(FRAME_SUPERCLASS_CLAUSE),
            initializers: [
                initializer(kind_tag::INITIALIZER_FULL, InitializerBody::Full),
                initializer(kind_tag::INITIALIZER_OWN_PART, InitializerBody::OwnPart),
                initializer(
                    kind_tag::INITIALIZER_STANDALONE,
                    InitializerBody::Standalone,
                ),
            ],
            kind_tag_shift: small_count(frames::SMALL_INTEGER_TAG_BITS),
            kind_tag_mask: i32::try_from(frames::KIND_TAG_MASK).unwrap_or_else(|_| {
                unreachable!("internal: the kind tag takes the payload's lowest byte")
            }),
            false_word,
        }
    }

    /// The word offset from the activation stack's base of the address
    /// `address` machine code holds a frame's register 0 at (its frame
    /// role's register), or `None` for one outside the stack's committed
    /// words.
    #[cfg(ns_jit)]
    pub(crate) fn machine_register_base_index(&self, address: u64) -> Option<u32> {
        let base = self.stack.frames.base_address() as u64;
        let bytes = address.checked_sub(base)?;
        let index = usize::try_from(bytes / 8).ok()?;
        (bytes % 8 == 0 && index < self.stack.frames.committed_words())
            .then(|| frames::stack_offset(index))
    }

    /// The word offset of `base`'s register 0 from the stack's base.
    #[cfg(ns_jit)]
    pub(crate) fn register_base_index(base: RegisterBase) -> u32 {
        base.index
    }

    /// The flags word of a counted method activation of a body taking
    /// `argument_count` arguments stopped in machine code, generation 0:
    /// what a release build's prologue adds an initializer's kind to
    /// (`MacroAssembler::record_frame_kind`).
    #[cfg(all(ns_jit, not(debug_assertions)))]
    pub(crate) fn machine_flags_word(argument_count: u16) -> u64 {
        FrameFlags {
            compiled: true,
            ..FrameFlags::new(kind_tag::METHOD, argument_count, true, false)
        }
        .word()
        .bits()
    }

    /// How far a frame's flags word is shifted to bring the shift its call
    /// made down to bit 0 (`frames::CALL_SHIFT_BITS`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_call_shift_shift() -> u8 {
        small_count(frames::CALL_SHIFT_BITS.trailing_zeros() + frames::SMALL_INTEGER_TAG_BITS)
    }

    /// How far a frame's flags word is shifted to bring its argument count
    /// down to bit 0, and the mask that leaves it alone there, as
    /// `FrameFlags::of` reads it: what an outer-local template reads of a
    /// married frame to find its register 0 below its header, as
    /// `register_slot` does.
    #[cfg(ns_jit)]
    pub(crate) fn machine_argument_count_field() -> (u8, i32) {
        let Ok(mask) = i32::try_from(frames::ARGUMENT_COUNT_MASK) else {
            unreachable!("internal: an argument count fits 16 bits");
        };
        (
            small_count(frames::ARGUMENT_COUNT_SHIFT + frames::SMALL_INTEGER_TAG_BITS),
            mask,
        )
    }

    /// The bits of a flags word that hold the shift its call made.
    #[cfg(ns_jit)]
    fn machine_call_shift_mask() -> u64 {
        frames::CALL_SHIFT_BITS << frames::SMALL_INTEGER_TAG_BITS
    }

    /// The amount a frame's flags word grows by when its `counted` bit is
    /// set: what an interpreter's call hands a counted block's prologue
    /// (`MachineWord::CalleeCounted`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_counted_word() -> u64 {
        frames::number_word(frames::COUNTED_BIT).bits() - frames::number_word(0).bits()
    }

    /// The register count's number among a frame's header words, which a
    /// debug build's outer-local template reads to check a married
    /// frame's window as `register_slot` checks it.
    #[cfg(all(ns_jit, debug_assertions))]
    pub(crate) fn register_count_register() -> u16 {
        let Ok(register) = u16::try_from(frame_header::REGISTER_COUNT) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        register
    }

    /// The flags word's register number past a frame's arguments.
    #[cfg(ns_jit)]
    pub(crate) fn flags_register() -> u16 {
        let Ok(register) = u16::try_from(frame_header::FLAGS) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        register
    }

    /// What a debug build's compiled prologue writes as a method frame's
    /// flags word for a body taking `argument_count` arguments
    /// (`jit::sends::prologue`): the generation's mask and its shift in
    /// the word, the flags word's register past the arguments, and the
    /// word with every bit but the generation's -- a counted method
    /// activation stopped in machine code.
    #[cfg(all(ns_jit, debug_assertions))]
    pub(crate) fn machine_flags(argument_count: u16) -> (MachineGeneration, u64) {
        let flags = FrameFlags {
            compiled: true,
            ..FrameFlags::new(kind_tag::METHOD, argument_count, true, false)
        };
        let Ok(mask) = i32::try_from(GENERATION_MASK) else {
            unreachable!("internal: a generation fits 31 bits");
        };
        let Ok(flags_register) = u16::try_from(frame_header::FLAGS) else {
            unreachable!("internal: the header's words are numbered from 0 to 12");
        };
        let shift = frames::GENERATION_SHIFT + value::machine::TAG_BITS;
        (
            MachineGeneration {
                mask,
                shift: small_count(shift),
                flags_register,
            },
            flags.word().bits(),
        )
    }

    /// The word offset of `activation`'s header from the stack's base.
    #[cfg(ns_jit)]
    pub(crate) fn activation_offset(activation: ActivationReference) -> u32 {
        activation.offset
    }

    /// The method `activation` runs the body of, when it is a method's
    /// activation: its code's row names it (`code_id_of_method`).
    pub(crate) fn activation_method(
        &self,
        activation: ActivationReference,
    ) -> Option<Rc<MixinMethod>> {
        match self.activation_kind(activation) {
            ActivationKind::Method(_) => self.code_method(self.activation_code_id(activation)),
            _ => None,
        }
    }

    /// The header machine code's frame `frame` describes, written but for
    /// its receiver and current class, which the compiled call wrote: the
    /// frame an exit materializes (`jit::materialize`) or a leaf runs in.
    /// Its flags say a counted activation of the frame's kind stopped in
    /// machine code, with a debug build's generation and, for a kind its
    /// code does not say, the tag from the flags word the frame's prologue
    /// wrote (`MachineKind`); its operand is the selector or the current
    /// class. Code that addresses its own slots keeps the guard and the
    /// entered level its prologue wrote and its own-slot instructions
    /// recorded since; any other records none, as the push does. Code
    /// that reifies its frame (`MachineFrame::reifies`) keeps the reified
    /// word, the counted bit and `retains_registers` it wrote, and in a
    /// leaf's view the program counter too; any other is written unreified,
    /// counted and without retained registers, as the push writes a method's
    /// frame. Answers the frame's handle.
    #[cfg(ns_jit)]
    pub(crate) fn write_machine_frame(&mut self, frame: &MachineFrame) -> ActivationReference {
        let offset = frame.offset as usize;
        let header_word = |heap: &Self, field: usize| {
            Word::from_bits(heap.stack.frames.committed_word(offset + field))
        };
        let written = FrameFlags::of(header_word(self, frame_header::FLAGS));
        let tag = match frame.kind.tag {
            MachineKindTag::Constant(tag) => tag,
            MachineKindTag::Recorded => written.kind_tag,
        };
        // Only code that reifies its frame wrote these words: in any other
        // frame they hold whatever the stack held there before. A block's
        // prologue writes the flags word too, counted or exempt as the
        // call said (`jit::sends::prologue`).
        let (reified, retains_registers) = if frame.reifies {
            (
                header_word(self, frame_header::REIFIED),
                written.retains_registers,
            )
        } else {
            (Word::NIL, false)
        };
        let counted = if frame.reifies || frame.kind.is_closure() {
            written.counted
        } else {
            true
        };
        let flags = FrameFlags {
            compiled: true,
            ..FrameFlags::new(tag, frame.argument_count, counted, retains_registers)
        };
        #[cfg(debug_assertions)]
        let flags = flags.with_generation(written.generation);
        // A leaf's view keeps how far the frame's call shifted it, which
        // its machine code's return reads (`frames::CALL_SHIFT_BITS`); a
        // frame the walk materializes returns through the interpreter.
        let flags_word = match frame.caller {
            Some(_) => flags.word(),
            None => Word::from_bits(
                flags.word().bits()
                    | (header_word(self, frame_header::FLAGS).bits()
                        & Self::machine_call_shift_mask()),
            ),
        };
        let caller = match frame.caller {
            Some((caller, top)) => frames::caller_link_word(caller, frame.start(), top),
            None => Word::NIL,
        };
        let operand = match frame.kind.operand {
            MachineOperand::Selector(selector) => frames::number_word(selector.index() as u64),
            MachineOperand::CurrentClass => header_word(self, frame_header::CURRENT_CLASS),
            MachineOperand::Recorded => header_word(self, frame_header::OPERAND),
        };
        // The program counter a frame that reifies holds is the one the
        // interpreter would hold there -- its push's 0, or the last one it
        // stored, at a send, which its templates store too -- and its
        // divorce records it in the frame object, so it is left be: a
        // leaf is handed its own, the exit's walk stops each frame below
        // the innermost at a send whose program counter the frame holds
        // already, and the run loop goes on at the innermost frame's from
        // the exit itself, as the interpreter goes on at an instruction
        // without storing it.
        let program_counter = if frame.reifies {
            header_word(self, frame_header::PROGRAM_COUNTER)
        } else {
            frames::number_word(u64::from(frame.program_counter))
        };
        let words = [
            (frame_header::OPERAND, operand),
            (frame_header::REIFIED, reified),
            (frame_header::FLAGS, flags_word),
            (
                frame_header::CODE,
                frames::number_word(frame.code.index() as u64),
            ),
            (frame_header::PROGRAM_COUNTER, program_counter),
            (
                frame_header::FEEDBACK_BASE,
                frames::number_word(u64::from(frame.feedback_base.raw())),
            ),
            (frame_header::CALLER, caller),
            (frame_header::CONTINUATION, Word::NIL),
            (
                frame_header::REGISTER_COUNT,
                frames::number_word(frame.register_count as u64),
            ),
        ];
        for (field, word) in words {
            self.stack
                .frames
                .set_committed_word(offset + field, word.bits());
        }
        // Code that addresses its own slots had its prologue write its
        // guard and entered level (`jit::sends::prologue`), and its own-slot
        // instructions record the guard their cold check verified there
        // since, which a rewrite here would throw away at every leaf call.
        // Every other frame records none, as the push does.
        if !frame.addresses_own_slots {
            for (field, word) in [
                (
                    frame_header::GUARD,
                    frames::number_word(u64::from(DispatchId::NONE.bits())),
                ),
                (
                    frame_header::ENTERED_LEVEL,
                    frames::number_word(ApplicationIndex::UNREGISTERED.index() as u64),
                ),
            ] {
                self.stack
                    .frames
                    .set_committed_word(offset + field, word.bits());
            }
        }
        self.activation_handle_at(frame.offset)
    }

    /// The handle of the frame whose header lies at `offset`, whatever the
    /// top: a frame machine code pushed, whose header was just written.
    #[cfg(ns_jit)]
    fn activation_handle_at(&self, offset: u32) -> ActivationReference {
        ActivationReference {
            offset,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            generation: FrameFlags::of(Word::from_bits(
                self.stack
                    .frames
                    .committed_word(offset as usize + frame_header::FLAGS),
            ))
            .generation,
        }
    }

    /// Marks `activation`, a frame the run loop pushed or resumed, as
    /// stopped in machine code at the send at `program_counter`: the
    /// bottom frame of a stretch an exit above it materializes.
    #[cfg(ns_jit)]
    pub(crate) fn mark_stopped_in_machine_code(
        &mut self,
        activation: ActivationReference,
        program_counter: u32,
    ) {
        self.check_activation(activation);
        let flags = FrameFlags {
            compiled: true,
            ..self.stack.flags(activation.offset)
        };
        self.stack
            .set_header_word(activation.offset, frame_header::FLAGS, flags.word());
        self.stack.set_header_word(
            activation.offset,
            frame_header::PROGRAM_COUNTER,
            frames::number_word(u64::from(program_counter)),
        );
    }

    /// `Leaf::Divorce`: the divorce of `activation`'s frame object as its
    /// pop divorces it (`pop_activation`), at the return of machine code
    /// whose frame was reified and retains its registers, which the
    /// return's inline divorce leaves to the copy here; then the frame
    /// reads as unreified, so that the pop of a frame the run loop entered
    /// machine code at divorces nothing again.
    #[cfg(ns_jit)]
    pub(crate) fn divorce_at_machine_return(&mut self, activation: ActivationReference) {
        self.check_activation(activation);
        let offset = activation.offset as usize;
        let header = self.stack.frames.header(offset);
        let reified = Word::from_bits(header[frame_header::REIFIED]).object_offset();
        let flags = FrameFlags::of(Word::from_bits(header[frame_header::FLAGS]));
        let program_counter = header[frame_header::PROGRAM_COUNTER];
        let Some(frame) = reified else {
            unreachable!("internal: machine code divorces only a reified frame");
        };
        self.divorce(frame, offset, flags, program_counter);
        self.stack
            .set_header_word(activation.offset, frame_header::REIFIED, Word::NIL);
    }

    /// What machine code reads and writes to divorce a reified frame of a
    /// body taking `argument_count` arguments at its return, as
    /// `pop_activation` divorces it (`jit::sends::return_from`), and what a
    /// prologue of code that reifies its frame writes as the push does
    /// (`jit::sends::prologue`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_divorce(argument_count: u16) -> MachineDivorce {
        let register = |field: usize| {
            let Ok(field) = u16::try_from(field) else {
                unreachable!("internal: the header's words are numbered from 0 to 12");
            };
            argument_count + field
        };
        let displacement = |field: usize| {
            let Ok(displacement) = i32::try_from((FIELDS + field) * 8) else {
                unreachable!("internal: a frame object's fixed fields lie near its start");
            };
            displacement
        };
        let small = |bits: u64| {
            let Ok(bits) = i32::try_from(bits) else {
                unreachable!("internal: the word is a small constant");
            };
            bits
        };
        MachineDivorce {
            reified_register: register(frame_header::REIFIED),
            flags_register: register(frame_header::FLAGS),
            program_counter_register: register(frame_header::PROGRAM_COUNTER),
            retains_registers_bit: small(
                frames::number_word(frames::RETAINS_REGISTERS_BIT).bits()
                    - frames::number_word(0).bits(),
            ),
            program_counter_displacement: displacement(frame_field::PROGRAM_COUNTER),
            sender_displacement: displacement(frame_field::SENDER),
            uncontinuable_displacement: displacement(frame_field::UNCONTINUABLE),
            married_displacement: displacement(frame_field::MARRIED),
            true_word: small(Value::boolean(true).word().bits()),
        }
    }

    /// The word a frame's header holds as the program counter
    /// `program_counter`, which a body's instruction count bounds to 16
    /// bits (`jit::baseline::compile`), as machine code stores it.
    #[cfg(ns_jit)]
    pub(crate) fn machine_program_counter_word(program_counter: u32) -> i32 {
        let Ok(word) = i32::try_from(frames::number_word(u64::from(program_counter)).bits()) else {
            unreachable!("internal: a compiled body's program counter is numbered in 16 bits");
        };
        word
    }

    /// Whether `activation` stopped in machine code
    /// (`FrameFlags::compiled`), so that a return to it resumes its
    /// machine code.
    #[inline(always)]
    pub(crate) fn activation_is_compiled(&self, activation: ActivationReference) -> bool {
        self.stack.flags(activation.offset).compiled
    }

    /// Clears `activation`'s compiled bit: the whole conversion of a
    /// baseline frame to an interpreted one, whose words are the
    /// interpreter's already.
    pub(crate) fn clear_compiled(&mut self, activation: ActivationReference) {
        self.check_activation(activation);
        let flags = FrameFlags {
            compiled: false,
            ..self.stack.flags(activation.offset)
        };
        self.stack
            .set_header_word(activation.offset, frame_header::FLAGS, flags.word());
    }

    /// Makes the frames an exit materialized the stack's: `innermost` the
    /// innermost, `top` the top, and `added` more live than before.
    #[cfg(ns_jit)]
    pub(crate) fn adopt_machine_frames(&mut self, innermost: u32, top: usize, added: usize) {
        self.stack.frames.set_top(top);
        self.stack.innermost = innermost;
        self.stack.live += added;
    }

    /// The stack's top and innermost frame, which a leaf's view of its own
    /// machine frame changes for the leaf's while (`view_machine_frame`).
    #[cfg(ns_jit)]
    pub(crate) fn stack_extent(&self) -> (usize, u32) {
        (self.stack.frames.top(), self.stack.innermost)
    }

    /// Lets the frame `activation`, written by `write_machine_frame`, read
    /// as the innermost for the length of a leaf: the top raised to its
    /// window's end. `restore_stack_extent` with what `stack_extent`
    /// answered before puts the stack back; the frame stays as machine
    /// code left it.
    #[cfg(ns_jit)]
    pub(crate) fn view_machine_frame(&mut self, activation: ActivationReference) {
        let end = self.stack.window_end_of_committed(activation.offset);
        self.stack.frames.set_top(end.max(self.stack.frames.top()));
        self.stack.innermost = activation.offset;
    }

    /// Puts back the stack's top and innermost frame `stack_extent`
    /// answered.
    #[cfg(ns_jit)]
    pub(crate) fn restore_stack_extent(&mut self, (top, innermost): (usize, u32)) {
        self.stack.frames.set_top(top);
        self.stack.innermost = innermost;
    }

    /// Hands machine code the receiver of the send the miss leaf decided
    /// to a target that is no method with code, for the stub that runs it
    /// (`jit::stub_entry`), which reads no current class: `nil` stands in
    /// for one.
    pub(crate) fn hand_machine_receiver(&self, receiver: Value<'gc>) {
        let receiver = self.store(receiver);
        self.machine_words
            .set(MachineWord::CalleeReceiver, receiver.bits());
        self.machine_words
            .set(MachineWord::CalleeClass, Word::NIL.bits());
    }

    /// Hands machine code the receiver and the current class of the
    /// callee the miss leaf decided (`MachineWord::CalleeReceiver`,
    /// `MachineWord::CalleeClass`), which the miss stub passes to the
    /// callee's machine entry as a compiled call passes them: nothing is
    /// written into the callee's frame before its checks pass.
    pub(crate) fn hand_machine_callee(&self, receiver: Value<'gc>, class: ClassReference<'gc>) {
        let receiver = self.store(receiver);
        let class = self.store(Value::object(class));
        self.machine_words
            .set(MachineWord::CalleeReceiver, receiver.bits());
        self.machine_words
            .set(MachineWord::CalleeClass, class.bits());
    }

    /// Hands machine code what an interpreter's send calls a block's body
    /// with (`jit::called_closure`): the receiver and the current class of
    /// the frame `closure` captured, `closure` itself, and whether its
    /// activation counts against the depth limit, which the closure call
    /// stub passes to the block's prologue as a closure stub does
    /// (`MachineWord::CalleeClosure`, `MachineWord::CalleeCounted`).
    #[cfg(ns_jit)]
    pub(crate) fn hand_machine_closure(
        &self,
        receiver: Value<'gc>,
        current_class: Option<ClassReference<'gc>>,
        closure: ObjectReference<'gc>,
        counted: bool,
    ) {
        let receiver = self.store(receiver);
        let class = current_class.map_or(Word::NIL, |class| self.store(Value::object(class)));
        let closure = self.store(Value::object(closure));
        self.machine_words
            .set(MachineWord::CalleeReceiver, receiver.bits());
        self.machine_words
            .set(MachineWord::CalleeClass, class.bits());
        self.machine_words
            .set(MachineWord::CalleeClosure, closure.bits());
        self.machine_words.set(
            MachineWord::CalleeCounted,
            if counted {
                Self::machine_counted_word()
            } else {
                0
            },
        );
    }

    /// Hands machine code the receiver, the level and the body of the
    /// initializer the initializer leaf decided, as `hand_machine_callee`
    /// hands a method's receiver and class, and the body's kind tag in
    /// `MachineWord::CalleeKind`, as the amount its flags word, written
    /// with a method's tag of 0, grows by: the callee's prologue adds it
    /// (`jit::sends::prologue`), so that the frame records which of its
    /// mixin's bodies it runs, which its code does not say.
    #[cfg(ns_jit)]
    pub(crate) fn hand_machine_initializer(
        &self,
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        body: InitializerBody,
    ) {
        self.hand_machine_callee(receiver, level);
        let tag = match body {
            InitializerBody::Full => kind_tag::INITIALIZER_FULL,
            InitializerBody::OwnPart => kind_tag::INITIALIZER_OWN_PART,
            InitializerBody::Standalone => kind_tag::INITIALIZER_STANDALONE,
        };
        let added = frames::number_word(u64::from(tag)).bits() - frames::number_word(0).bits();
        self.machine_words.set(MachineWord::CalleeKind, added);
    }

    /// The receiver machine code handed a leaf in the machine words
    /// (`MachineWord::CalleeReceiver`), which the mirror method stub
    /// stores before its leaf call (`jit::targets`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_callee_receiver(&self) -> Value<'gc> {
        self.load(Word::from_bits(
            self.machine_words.get(MachineWord::CalleeReceiver),
        ))
    }

    /// The value in the activation stack's word at `address`, which machine
    /// code handed a leaf as a callee's register 0 -- the first of its
    /// send's slots -- or `None` for an address outside the stack's
    /// committed words, at which the leaf declines rather than run on a
    /// value machine code never handed it.
    #[cfg(ns_jit)]
    pub(crate) fn machine_frame_value(&self, address: u64) -> Option<Value<'gc>> {
        let base = self.stack.frames.base_address() as u64;
        let word = address
            .checked_sub(base)
            .filter(|bytes| bytes % 8 == 0)
            .and_then(|bytes| usize::try_from(bytes / 8).ok())
            .filter(|index| *index < self.stack.frames.committed_words())
            .map(|index| self.stack.frames.committed_word(index));
        word.map(|bits| self.load(Word::from_bits(bits)))
    }

    /// The value machine code answered as the word `bits`
    /// (`jit::Resumption::Returned`): a word a frame register held.
    pub(crate) fn machine_value(&self, bits: u64) -> Value<'gc> {
        self.load(Word::from_bits(bits))
    }

    /// The word `value` is in a frame register, which a leaf answers
    /// machine code (`jit::leaves`).
    #[cfg(ns_jit)]
    pub(crate) fn machine_word_of(&self, value: Value<'gc>) -> u64 {
        self.store(value).bits()
    }

    /// Refuses machine code's read of `activation`'s dead register
    /// `register` with the interpreter's own refusal
    /// (`assert_live_register`).
    #[cfg(ns_jit)]
    pub(crate) fn refuse_dead_register(&self, activation: ActivationReference, register: u16) -> ! {
        refuse_dead_register_read(activation.offset, register)
    }

    /// The write barrier: `holder` now stores `word`. An old object that
    /// comes to name a young one joins the remembered set, so the next
    /// scavenge finds the reference without walking the old space -- per
    /// object, as Spur's `storePointer:ofObject:withValue:` and Soup's
    /// `HeapObject::Store` remember, rather than Strongtalk's cards. Every
    /// store the interpreter and the reference lowering make into an
    /// existing object comes through here, so the two share one rule;
    /// machine code makes the same comparisons inline and calls only
    /// `remember` (`remember_for_machine_code`) (`docs/design/memory.md`,
    /// "The write barrier").
    #[inline(always)]
    fn write_barrier(&mut self, holder: NonZeroU64, word: Word) {
        if !space::is_young(holder) && word.object_offset().is_some_and(space::is_young) {
            self.remember(holder);
        }
    }

    /// The decoder's fills take no barrier: the load tenures everything the
    /// graph names before the graph pass, so no word they store is young.
    fn debug_assert_tenured(&self, word: Word) {
        debug_assert!(
            !self.allocating_old || !word.object_offset().is_some_and(space::is_young),
            "the load tenures everything the graph names before the graph pass"
        );
    }

    /// `remember` for machine code's write barrier
    /// (`MacroAssembler::store_with_barrier`), which calls it only for an
    /// old holder `holder_word`, its word, that came to name a young
    /// object and whose remembered bit it found clear: the interpreter's
    /// own decision, its limit's request of a scavenge included.
    #[cfg(ns_jit)]
    pub(crate) fn remember_for_machine_code(&mut self, holder_word: u64) {
        let Some(holder) = Word::from_bits(holder_word).object_offset() else {
            unreachable!("internal: the write barrier remembers an object");
        };
        debug_assert!(
            !space::is_young(holder),
            "internal: the write barrier remembers an old object"
        );
        self.remember(holder);
    }

    /// Adds `holder`, an old object, to the remembered set unless its bit
    /// says it is in it already.
    fn remember(&mut self, holder: NonZeroU64) {
        let index = header_index(holder);
        let words = self.space.words_mut();
        let header = Header(words[index]);
        if header.remembered() {
            return;
        }
        words[index] = header.with_remembered(true).0;
        self.remembered.push(holder);
        if self.remembered.len() > REMEMBERED_SET_LIMIT {
            self.request_scavenge(true);
        }
    }

    /// Only `allocate_block` and the remembered set's overflow ask, and only
    /// flags are set: the run loop's next safepoint scavenges.
    fn request_scavenge(&mut self, tenure_all: bool) {
        self.scavenge_requested = true;
        self.tenure_all_requested |= tenure_all;
        self.request_safepoint();
    }

    /// The image decoder's graph pass allocates old: everything it names was
    /// tenured before it began (`image::load`).
    pub(crate) fn set_allocating_old(&mut self, allocating_old: bool) {
        self.allocating_old = allocating_old;
        let threshold = (!allocating_old).then_some(self.scavenge_threshold);
        self.space.set_young_threshold(threshold);
    }

    /// Whether an allocation past eden's threshold, a spent eden or the
    /// remembered set's overflow has asked for a scavenge since the last
    /// one (`Interpreter::safepoint`).
    pub(crate) fn scavenge_requested(&self) -> bool {
        self.scavenge_requested
    }

    /// Whether the requested scavenge must tenure every survivor: the
    /// remembered set outgrew its limit, and only emptying the nursery
    /// empties it.
    pub(crate) fn tenure_all_requested(&self) -> bool {
        self.tenure_all_requested
    }

    /// Asserts that every old object naming a young one is remembered, and
    /// that the remembered set and the bits agree in membership -- every
    /// entry names an old object whose bit is set, none twice, and every
    /// object whose bit is set is an entry. That is what a store that missed
    /// the write barrier, or an entry at a wrong offset, breaks, found before
    /// the scavenge that would lose the young object, as Strongtalk's
    /// `VerifyBeforeScavenge`
    /// (`prior-art/strongtalk/vm/memory/universe_more.cpp:98`) and HotSpot's
    /// `VerifyBeforeGC` find it. A walk of the old space, so only the scavenge
    /// stress mode runs it.
    #[cfg(debug_assertions)]
    pub(crate) fn verify_remembered_set(&self) {
        let entries: std::collections::HashSet<NonZeroU64> =
            self.remembered.iter().copied().collect();
        assert_eq!(
            entries.len(),
            self.remembered.len(),
            "internal: the remembered set holds an offset twice"
        );
        let words = self.space.words();
        let mut remembered_headers = std::collections::HashSet::new();
        let old = self.space.region(RegionName::Old);
        let mut position = old.start / 8;
        while position < old.top / 8 {
            let (header, next) = object_at(words, position);
            let block = block_range(words, header);
            let names_young = object::pointer_ranges(&words[block.clone()])
                .into_iter()
                .any(|range| {
                    words[range.start + block.start..range.end + block.start]
                        .iter()
                        .any(|bits| {
                            Word::from_bits(*bits)
                                .object_offset()
                                .is_some_and(space::is_young)
                        })
                });
            let remembered = Header(words[header]).remembered();
            assert!(
                !names_young || remembered,
                "internal: an old object names a young one without being remembered: a store missed the write barrier"
            );
            if remembered {
                let offset = 8 * header as u64;
                assert!(
                    NonZeroU64::new(offset).is_some_and(|offset| entries.contains(&offset)),
                    "internal: an object carries the remembered bit but the remembered set lacks its offset"
                );
                remembered_headers.insert(offset);
            }
            position = next;
        }
        for entry in &entries {
            assert!(
                !space::is_young(*entry) && remembered_headers.contains(&entry.get()),
                "internal: the remembered set holds an offset that is not an old object carrying the remembered bit"
            );
        }
    }

    /// Sets eden's occupied bytes at which an allocation asks for a
    /// scavenge, so a test can reach the trigger without filling eden first.
    #[cfg(debug_assertions)]
    pub(crate) fn set_scavenge_threshold(&mut self, bytes: usize) {
        self.scavenge_threshold = bytes;
        if !self.allocating_old {
            self.space.set_young_threshold(Some(bytes));
        }
    }

    /// The bytes the old space holds.
    fn old_occupied(&self) -> usize {
        let old = self.space.region(RegionName::Old);
        old.top - old.start
    }

    /// Traces the activation stack, every word below the top, through
    /// `tracer`, and writes `dead_register`'s word first into every
    /// register of every frame that its frame does not read before it
    /// writes it -- what a callee left above a send's slots, a word a
    /// popped frame left in scratch -- so that the trace keeps nothing
    /// alive through a dead register, and meets no handle to an object a
    /// collection moved or reclaimed while the word lay above the top
    /// (`docs/design/memory.md`, "What the collector traces"). Every trace
    /// of the stack is this one: the scavenge's, marking's, and every
    /// forward's (`forward_everything`). An associated function over the
    /// heap's fields, since a tracer borrows others of them.
    ///
    /// It walks the frames from the top by their caller words. Each frame
    /// loses the registers its body's analysis says are tainted before the
    /// program counter its header stores (`CompiledCode::tainted_before`;
    /// which instruction that is, `leftovers::tainted_at` says): the top
    /// frame the instruction it runs next, every other frame the one it
    /// stopped at. A frame whose callee lies on its slots stopped at that
    /// send, which it resumes after and never runs again, so it loses as
    /// well every register from the slots' end -- the callee's header --
    /// to its own window's end: a send taints all of them. A word inside
    /// the window of a frame above is that frame's to decide, and one at or
    /// past the top no frame's, so neither is written, nor a frame's
    /// arguments and header, which no analysis taints: a callee's
    /// arguments are its sender's slots.
    ///
    /// The stack is walked once, and each frame's words are traced as the
    /// walk nils them, while its header's lines are still in the cache: a
    /// frame's words but those of the windows above, which the frames
    /// above traced and no frame below writes. A frame whose body keeps
    /// its set already is nilled and traced as the walk meets it; one
    /// whose body keeps none yet is deferred, with the windows above it
    /// that reach into its own. Once the walk is done, the missing sets
    /// are filled body by body, each body's from one pass over it
    /// (`CompiledCode::fill_tainted_before`), so that a collection that
    /// meets a body at many new program counters -- a deep recursion
    /// through it, the first collection of a run -- decodes and analyses
    /// it once rather than once per program counter; then each deferred
    /// frame is nilled against the windows recorded for it, exactly as the
    /// walk would have nilled it, and traced. Last, as a net, a word below
    /// the top in no frame's window would be traced as it lies. None is:
    /// the bottom frame starts at 0, a push raises the top to its frame's
    /// window end and a pop gives back the top its frame was pushed at,
    /// so the top is always some live frame's window end and the windows
    /// cover `[0, top)`; a debug build refuses such a word.
    ///
    /// A frame costs the reads of its header, the writes of its nils and
    /// the trace of its words, and a lookup of its set only where its body
    /// or program counter differs from the frame's above (`LastSet`),
    /// which a deep recursion's frames rarely do; one whose every word it
    /// may write lies in the window above it, as such a frame's do, writes
    /// nothing (`FrameWindow::untouched`). A walk that finds every set
    /// kept, the common case once a program has run a while, allocates
    /// nothing but the claimed windows, which merge as the walk goes down,
    /// since a frame starts at or below every frame above it.
    ///
    /// Out of line, once per tracer: inlined, it grew the scavenge by
    /// about four fifths and changed what the full collection inlines, and
    /// the functions linked after them -- `pop_activation`, `resume_caller`
    /// and the polymorphic inline cache's hit among them -- moved to other
    /// offsets, which measured `RichardsTon80` about 1.3% slower with
    /// nothing of the collector in its profile.
    #[inline(never)]
    fn trace_stack<T: crate::trace::Tracer>(
        stack: &mut ActivationStack,
        tables: &Tables,
        tracer: &mut T,
    ) {
        if stack.innermost == NO_FRAME {
            trace_words(stack.frames.words_mut(), tracer);
            return;
        }
        let top = stack.frames.top();
        let filler = dead_register().bits();
        // The windows of the frames walked so far, merged, ordered by
        // start, the lowest last.
        let mut claimed: Vec<Range<usize>> = Vec::new();
        // The frames whose set no fill has kept yet, each with the part of
        // `claimed` that reached into its window when the walk met it,
        // recorded in `deferred_claims`.
        let mut deferred: Vec<DeferredFrame> = Vec::new();
        let mut deferred_claims: Vec<Range<usize>> = Vec::new();
        // The start and the header of the frame walked just before: the
        // callee of the frame walked now.
        let mut callee: Option<(usize, usize)> = None;
        // Whether that callee runs a block, which a closure stub may lay
        // past its sender's slots.
        #[cfg(debug_assertions)]
        let mut callee_is_block = false;
        let mut last: Option<LastSet<'_>> = None;
        let mut offset = stack.innermost;
        loop {
            let start = stack.frame_start(offset);
            let header = offset as usize;
            let end = stack.window_end(offset);
            let code_index = frames::number_as_usize(stack.header_word(offset, frame_header::CODE));
            // A program counter is an instruction's index, and a body has
            // at most `INSTRUCTION_LIMIT` of them.
            let program_counter =
                frames::number_as_u32(stack.header_word(offset, frame_header::PROGRAM_COUNTER));
            let caller = stack.caller(offset);
            let window = FrameWindow {
                start,
                registers: header + frame_header::COUNT,
                until: end.min(top),
                callee_header: callee
                    .filter(|(callee_start, _)| *callee_start < end)
                    .map(|(_, callee_header)| callee_header),
            };
            let set = LastSet::of(&mut last, tables, code_index, program_counter);
            #[cfg(debug_assertions)]
            set.check_callee(
                start,
                callee.filter(|(callee_start, _)| *callee_start < end).map(
                    |(callee_start, callee_header)| (callee_start, callee_header, callee_is_block),
                ),
            );
            match &set.tainted {
                Some(tainted) => {
                    let words = stack.frames.words_mut();
                    if !window.untouched(&claimed, set.first_tainted) {
                        window.nil(words, &claimed, tainted, filler);
                    }
                    window.trace(words, &claimed, tracer);
                }
                None => {
                    // Every window above starts at or above this frame's
                    // start, so those that reach into it are the lowest.
                    let reaching = claimed.partition_point(|above| above.start >= end);
                    let first_claim = deferred_claims.len();
                    deferred_claims.extend_from_slice(&claimed[reaching..]);
                    deferred.push(DeferredFrame {
                        code_index,
                        program_counter,
                        window,
                        claims: first_claim..deferred_claims.len(),
                    });
                }
            }
            claim(&mut claimed, start..end);
            callee = Some((start, header));
            #[cfg(debug_assertions)]
            {
                callee_is_block = stack.flags(offset).kind_tag == frames::kind_tag::CLOSURE;
            }
            match caller {
                Some(caller) => offset = caller,
                None => break,
            }
        }
        if !deferred.is_empty() {
            // The fill changes the sets the last lookup borrowed.
            last = None;
            let mut missing: Vec<(usize, u32)> = deferred
                .iter()
                .map(|frame| (frame.code_index, frame.program_counter))
                .collect();
            missing.sort_unstable();
            missing.dedup();
            for body in missing.chunk_by(|first, second| first.0 == second.0) {
                let program_counters: Vec<u32> = body
                    .iter()
                    .map(|(_, program_counter)| *program_counter)
                    .collect();
                if let Some(entry) = tables.codes.get(body[0].0).and_then(Option::as_ref) {
                    entry.code.fill_tainted_before(&program_counters);
                }
            }
            for frame in &deferred {
                let set = LastSet::of(&mut last, tables, frame.code_index, frame.program_counter);
                let Some(tainted) = &set.tainted else {
                    unreachable!("internal: the fill keeps every set the walk deferred")
                };
                let claims = &deferred_claims[frame.claims.clone()];
                let words = stack.frames.words_mut();
                if !frame.window.untouched(claims, set.first_tainted) {
                    frame.window.nil(words, claims, tainted, filler);
                }
                frame.window.trace(words, claims, tracer);
            }
        }
        // The words no frame's window holds, which the windows' cover of
        // `[0, top)` says are none: `claimed` is now every window's,
        // merged.
        let words = stack.frames.words_mut();
        for_each_unclaimed(&claimed, 0..top, |run| {
            debug_assert!(
                run.is_empty(),
                "internal: the stack words {}..{} lie in no frame's window",
                run.start,
                run.end
            );
            trace_words(&mut words[run], tracer)
        });
    }

    /// One scavenge (`scavenge.rs`): every young object the roots, the
    /// remembered set and the copies themselves reach is copied, the rest of
    /// the nursery is reclaimed, and every handle the heap holds is rewritten
    /// into the next epoch. `step_roots` traces what the step state holds
    /// beside the heap (`World::scavenge`). `tenure_all` sends every survivor
    /// old, which empties the nursery and the remembered set. `futures` is
    /// the test for a resolved future, which the weak pass sees through
    /// (`ResolvedFutures`); `None` when the program has made none.
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between entries.
    pub(crate) fn scavenge(
        &mut self,
        tenure_all: bool,
        futures: Option<ResolvedFutures>,
        step_roots: impl FnOnce(&mut Scavenger<'_>),
    ) {
        use crate::trace::{Trace, Tracer};
        self.settle_machine_allocations();
        // Committed for the worst case first, so that a promotion never meets a
        // refused commit halfway through (`docs/design/memory.md`).
        let [past, eden, _] = self.space.occupied();
        let worst = self.space.region(RegionName::Old).top + past.len() + eden.len();
        if self.space.ensure_committed(worst).is_none() {
            out_of_memory(worst - self.space.committed())
        }
        #[cfg(debug_assertions)]
        let (heap_id, from_epoch, to_epoch) =
            (self.heap_id, self.epoch, self.epoch.wrapping_add(1));
        #[cfg(debug_assertions)]
        {
            self.epoch = to_epoch;
        }
        let threshold = self.tenuring_threshold;
        let old_remembered = std::mem::take(&mut self.remembered);
        let Heap {
            space,
            stack,
            suspensions,
            continuations,
            activation_class,
            mixin_objects,
            host_roots,
            canonical_symbols,
            symbol_of,
            tables,
            ..
        } = self;
        let mut scavenger = Scavenger::new(
            space,
            tenure_all,
            threshold,
            futures,
            #[cfg(debug_assertions)]
            heap_id,
            #[cfg(debug_assertions)]
            from_epoch,
            #[cfg(debug_assertions)]
            to_epoch,
        );
        if let Some(class) = activation_class {
            scavenger.object(class);
        }
        for object in mixin_objects.iter_mut().flatten() {
            scavenger.object(object);
        }
        Self::trace_stack(stack, tables, &mut scavenger);
        suspensions.trace(&mut scavenger);
        continuations.trace(&mut scavenger);
        let roots = host_roots.get_mut();
        roots.sweep();
        for slot in &roots.slots {
            let mut value = slot.get();
            scavenger.value(&mut value);
            slot.set(value);
        }
        step_roots(&mut scavenger);
        // The old remembered set: each entry leaves it, and joins the new one
        // again only if it still names a young object. Only its strong words
        // are judged here; `finish` judges the weak ones once they are
        // settled, and remembers the entry then.
        for holder in old_remembered {
            let header = header_index(holder);
            let words = scavenger.space.words_mut();
            words[header] = Header(words[header]).with_remembered(false).0;
            if scavenger.scan_object(header) {
                let words = scavenger.space.words_mut();
                words[header] = Header(words[header]).with_remembered(true).0;
                scavenger.remembered.push(holder);
            }
        }
        scavenger.drain();
        scavenger.finish();
        Self::forward_weak_symbols(canonical_symbols, symbol_of, &scavenger);
        let remembered = std::mem::take(&mut scavenger.remembered);
        let (survived, promoted, ages) = (scavenger.survived, scavenger.promoted, scavenger.ages);
        // Poisons every word the nursery's objects held before the scavenge
        // (the survivors now live in the future survivor space or the old
        // one), so that a word that missed the barrier reads poison rather
        // than the next allocation. Only the occupied part, so that a
        // scavenge at every safepoint stays cheap.
        #[cfg(debug_assertions)]
        {
            let words = self.space.words_mut();
            for range in [past, eden] {
                words[range.start / 8..range.end / 8].fill(header::POISON);
            }
        }
        self.remembered = remembered;
        self.object_count = self.object_count - self.young_objects + survived;
        self.young_objects = survived - promoted;
        self.space.flip_survivors();
        let survivor = self.space.future_survivor();
        let desired = (survivor.end - survivor.base - REGION_SLACK) / 8 / 2;
        self.tenuring_threshold = scavenge::tenuring_threshold(&ages, desired);
        self.scavenge_requested = false;
        self.tenure_all_requested = false;
        self.scavenges += 1;
        // The set rebuilt here is bounded as the barrier bounds it: past the
        // limit, the next safepoint tenures everything, which empties it.
        if self.remembered.len() > REMEMBERED_SET_LIMIT {
            self.request_scavenge(true);
        }
        if self.old_occupied() >= self.next_collection {
            self.request_collection();
        }
    }

    /// An allocation never collects; it asks, and the run loop's next
    /// safepoint serves the request (decided 2026-09-13). Setting the
    /// safepoint request is what makes the poll take its slow path.
    ///
    /// Must do nothing but set these two flags: its call site raises the
    /// request while the object just allocated is still half-wired --
    /// `allocate_block` before `object_handle` names it, and for a frame object
    /// `reify` before the writes that make it reachable from its stack
    /// entry -- so it is harmless only because nothing here reads the heap
    /// back. Real work here would have `reify` collect a frame no stack
    /// entry points at yet.
    ///
    /// `allocate_block` calls it, so every allocation path compares against
    /// the limit; so does a scavenge that leaves the old space past it, and
    /// `garbageCollect`'s leaf (`builtins::weak`), which a program sends to
    /// see a weak slot cleared.
    pub(crate) fn request_collection(&mut self) {
        self.collection_requested = true;
        self.request_safepoint();
    }

    pub(crate) fn collection_requested(&self) -> bool {
        self.collection_requested
    }

    /// After a collection: the space's next limit, in bytes
    /// (`collection_limit`).
    pub(crate) fn reset_collection_limits(&mut self) {
        self.collection_requested = false;
        self.next_collection = collection_limit(self.collection_floor, self.old_occupied());
    }

    /// Sets the floor and the next limit to `floor` bytes.
    #[cfg(debug_assertions)]
    pub(crate) fn set_collection_floor(&mut self, floor: usize) {
        self.collection_floor = floor;
        self.next_collection = floor;
    }

    /// How much a move has to rewrite: the objects, the activation stack's
    /// words, the suspended activations, the continuations, the reified-mixin
    /// and canonical symbol tables, and the host roots. The stress mode paces
    /// its moves by it.
    #[cfg(debug_assertions)]
    pub(crate) fn traced_size(&self) -> usize {
        self.object_count
            + self.machine_allocations()
            + self.stack.frames.top()
            + self.suspensions.rows.len()
            + self.continuations.rows.len()
            + self.mixin_objects.len()
            + self.canonical_symbols.len()
            + self.host_roots.borrow().slots.len()
    }

    // ---------- Moving everything (debug builds only) ----------

    /// How many times this heap has moved everything -- by `move_everything`
    /// or by `compact`, both of which relocate every survivor to a new
    /// offset and so advance this the same way: a handle into the arrangement
    /// before either one is exactly as stale as one into the arrangement
    /// before the other.
    #[cfg(debug_assertions)]
    pub(crate) fn move_epoch(&self) -> u32 {
        self.epoch
    }

    /// Moves every object, every reified frame among them, to a new offset and
    /// rewrites every handle the heap itself holds -- in every object, in every
    /// word of the stack, every suspended activation and every continuation,
    /// and in every host root -- into the next epoch. What the heap does not
    /// hold, the caller forwards with the answer: the interpreter's own roots
    /// (`World::move_everything`). A compacting collection that treats
    /// everything as live; the stress mode runs one at the run loop's
    /// safepoints. Every object shifts by one distance, up from the first
    /// offset and back down to it, so every offset changes at every move
    /// (`docs/design/memory.md`, "The move stress mode").
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between runs.
    #[cfg(debug_assertions)]
    #[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them stale"]
    fn move_everything(&mut self) -> Forwarder<Shift> {
        let from_epoch = self.epoch;
        let to_epoch = from_epoch.wrapping_add(1);
        let mut regions = Vec::with_capacity(3);
        for name in [RegionName::PastSurvivor, RegionName::Eden, RegionName::Old] {
            let region = self.space.region(name);
            let length = region.top - region.start;
            // Varied by the epoch, so that moves do not all shift alike.
            let destination = if region.start == region.base {
                region.base + 8 * (2 + to_epoch as usize % 31)
            } else {
                region.base
            };
            if name == RegionName::Old
                && self.space.ensure_committed(destination + length).is_none()
            {
                out_of_memory(destination + length - self.space.committed())
            }
            // Both ranges lie below the region's top while the objects are
            // copied.
            self.space.set_region_extent(
                name,
                region.start,
                region.start.max(destination) + length,
            );
            self.space.words_mut().copy_within(
                region.start / 8..(region.start + length) / 8,
                destination / 8,
            );
            // A ByteArray's data sit at its own address's alignment, which
            // the shift changes; left at the old block's offset, every read
            // would answer wrong bytes. Each object moved by the region's
            // one distance, so its old header lay that far from the new.
            let words = self.space.words_mut();
            let mut position = destination / 8;
            while position < (destination + length) / 8 {
                let (header, next) = object_at(words, position);
                if Header(words[header]).format() == Format::AlignedBytes {
                    object::realign(words, header - destination / 8 + region.start / 8, header);
                }
                position = next;
            }
            self.space
                .set_region_extent(name, destination, destination + length);
            regions.push(ShiftedRegion {
                source: region.start as u64..region.top as u64,
                destination: destination as u64,
            });
        }
        let Ok(regions) = <[ShiftedRegion; 3]>::try_from(regions) else {
            unreachable!("internal: the space has three regions that hold objects")
        };
        self.epoch = to_epoch;
        let mut forwarder = Forwarder {
            heap_id: self.heap_id,
            from_epoch,
            to_epoch,
            objects: Shift { regions },
        };
        // The remembered set holds raw offsets, which no tracer reaches; the
        // next scavenge reads each one as a header. A compaction empties the
        // set instead.
        for holder in &mut self.remembered {
            *holder = forwarder.objects.apply(*holder);
        }
        self.forward_everything(&mut forwarder);
        forwarder
    }

    /// Compacts every object the finished marking reached, in both
    /// generations, into the old space: the old survivors slide down to its
    /// bottom in the order they lie, the nursery's follow them in address
    /// order, and each one's mark bit and age are cleared, so that no object
    /// leaves the collection marked. The nursery and the remembered set are
    /// left empty, and no remembered bit set. Gives committed memory back
    /// past twice the next limit; and rewrites every handle the heap itself
    /// holds into the next epoch. What the heap does not hold, the caller
    /// forwards with the answer, exactly as for a move (`World::collect`).
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between runs.
    #[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them stale"]
    pub(crate) fn compact(&mut self) -> Forwarder<ForwardingTable> {
        self.settle_machine_allocations();
        #[cfg(debug_assertions)]
        let from_epoch = self.epoch;
        #[cfg(debug_assertions)]
        let to_epoch = from_epoch.wrapping_add(1);
        #[cfg(debug_assertions)]
        {
            self.epoch = to_epoch;
        }
        let [past, eden, old_range] = self.space.occupied();
        let mut table = ForwardingTable::with_words(old_range.end / 8, NURSERY_END / 8);
        let mut live_objects = 0;
        let words = self.space.words();
        for range in [&past, &eden, &old_range] {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(words, position);
                if Header(words[header]).marked() {
                    table.set_live(position..next);
                    live_objects += 1;
                }
                position = next;
            }
        }
        let live_words = table.summarize(NURSERY_END / 8 / 64);
        // The remembered bits go first, at the offsets the set holds now:
        // once everything is old, nothing needs remembering.
        for holder in std::mem::take(&mut self.remembered) {
            let index = header_index(holder);
            let words = self.space.words_mut();
            words[index] = Header(words[index]).with_remembered(false).0;
        }
        let new_top = NURSERY_END + 8 * live_words;
        if self.space.ensure_committed(new_top).is_none() {
            out_of_memory(new_top - self.space.committed())
        }
        let old = self.space.region(RegionName::Old);
        // The words up to the new top must be lent while the nursery's
        // survivors are copied above the old ones.
        self.space
            .set_region_extent(RegionName::Old, old.start, old.top.max(new_top));
        // The old space slides down first: a destination there is never above
        // its source. The nursery's survivors then go above every old
        // survivor, into words no slide still has to read; a copy of one
        // reads below `NURSERY_END` and writes above it, so the two never
        // overlap.
        for range in [old_range, past, eden] {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let words = self.space.words_mut();
                let (header, next) = object_at(words, position);
                if Header(words[header]).marked() {
                    let destination = table.destination(position);
                    words.copy_within(position..next, destination);
                    let moved = destination + (header - position);
                    // A ByteArray's data sit at its own address's alignment;
                    // left at the old block's offset, every read would answer
                    // wrong bytes. The slide copies downward, so the words
                    // inside the new block are all moved already.
                    if Header(words[moved]).format() == Format::AlignedBytes {
                        object::realign(words, header, moved);
                    }
                    words[moved] = Header(words[moved]).with_marked(false).with_age(0).0;
                }
                position = next;
            }
        }
        self.space
            .set_region_extent(RegionName::Old, NURSERY_END, new_top);
        self.space.empty_nursery();
        self.object_count = live_objects;
        self.young_objects = 0;
        self.scavenge_requested = false;
        self.tenure_all_requested = false;
        // With hysteresis, so that a heap which oscillates does not decommit
        // and commit again at every collection: give back only past twice
        // the next limit, and keep the limit itself. Measured against the
        // limit rather than the live set, because under the floor the two
        // part, and a space cut to twice its live set would commit its way
        // back to the floor at every cycle (`docs/design/memory.md`, "The
        // trigger and the growth policy").
        let limit = collection_limit(self.collection_floor, 8 * live_words);
        if self.space.committed() > NURSERY_END + limit.saturating_mul(2) {
            self.space.release_above(NURSERY_END + limit);
        }
        let mut forwarder = Forwarder {
            #[cfg(debug_assertions)]
            heap_id: self.heap_id,
            #[cfg(debug_assertions)]
            from_epoch,
            #[cfg(debug_assertions)]
            to_epoch,
            objects: table,
        };
        self.forward_everything(&mut forwarder);
        forwarder
    }

    /// Forwards every object of `old` to the object of `new` at the same
    /// index, one way and at once: `become:` as a bulk forward
    /// (`docs/design/object-model.md`, "`become:` is an eager bulk
    /// forward"). Every pair is validated before anything is written: equal
    /// sizes, no object of `old` named twice (a split), none also among
    /// `new` (a chain); several old objects forwarded to one new one (a
    /// merge) are accepted. Then, pair by pair in order, the new object
    /// takes the old one's identity hash -- so that an identity-keyed
    /// collection still finds what it holds, the last pair's standing for a
    /// merge -- and the old object becomes a forwarder: the forwarded
    /// format, the guard no application is minted, the new object in its
    /// class word and its word count kept, so the space stays walkable.
    /// The hash is copied whether or not the old object has one, so a new
    /// object that was hashed already takes the old one's absence of a
    /// hash and answers another hash afterwards: a caller forwards only to
    /// objects nothing has hashed yet, as the install (`install_edit`) does
    /// to the fresh ones it builds.
    /// Last, one walk rewrites every handle the heap itself holds
    /// (`forward_everything`) and remembers every old holder that names a
    /// young object afterwards, which no barrier saw stored; a forwarder
    /// already in the remembered set stays there until its target is
    /// tenured or a full collection runs, its class word the only word a
    /// scavenge reads of it. What the heap does not hold, the caller
    /// forwards with the answer (`world::forward_identity`).
    ///
    /// Eager: once the walk ends nothing names a forwarder, so no send, no
    /// `==`, no leaf and no later JIT ever checks for one; the one consumer,
    /// the atomic install, walks the heap anyway (`object-model.md`, "Why
    /// eager"). Nothing live moves, so no move epoch changes, and it runs
    /// inside a step: no instruction runs between the forward and the
    /// leaf's answer. Its one caller is the VM mirror's
    /// `elementsOf:forwardIdentityToElementsOf:ifFail:` leaf, whose lock
    /// (`builtins/reflection.rs`'s `may_be_forwarded`) has refused every
    /// object but an ordinary `FixedPointers` instance of a class that is no
    /// builtin and whose mixin is not shipped, no reified mixin and no
    /// withheld value.
    #[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them naming forwarders"]
    pub(crate) fn forward_identity(
        &mut self,
        old: &[ObjectReference<'gc>],
        new: &[ObjectReference<'gc>],
    ) -> Result<Forwarder<IdentityForward>, ForwardRefusal> {
        if old.len() != new.len() {
            return Err(ForwardRefusal::UnequalSizes);
        }
        for object in old.iter().chain(new) {
            self.check_object_origin(*object);
        }
        let new_offsets: std::collections::HashSet<u64> =
            new.iter().map(|object| object.offset.get()).collect();
        let mut replacements: IdentityMap<u64, NonZeroU64> = IdentityMap::default();
        for (old_object, new_object) in old.iter().zip(new) {
            if replacements
                .insert(old_object.offset.get(), new_object.offset)
                .is_some()
            {
                return Err(ForwardRefusal::Split);
            }
            if new_offsets.contains(&old_object.offset.get()) {
                return Err(ForwardRefusal::Chain);
            }
        }
        for (old_object, new_object) in old.iter().zip(new) {
            let hash = self.header(*old_object).identity_hash();
            self.object_set_identity_hash(*new_object, hash);
            let words = self.space.words_mut();
            let header = header_index(old_object.offset);
            words[header] = Header(words[header])
                .with_format(Format::Forwarded)
                .with_guard(DispatchId::FORWARDED)
                .0;
            words[header + CLASS] = Word::with_object_offset(new_object.offset).bits();
        }
        let mut forwarder = Forwarder {
            #[cfg(debug_assertions)]
            heap_id: self.heap_id,
            #[cfg(debug_assertions)]
            from_epoch: self.epoch,
            #[cfg(debug_assertions)]
            to_epoch: self.epoch,
            objects: IdentityForward {
                replacements,
                names_young: std::cell::Cell::new(false),
            },
        };
        self.forward_everything(&mut forwarder);
        Ok(forwarder)
    }

    /// Forwards every handle the heap itself holds through `forwarder`:
    /// every object, the `Activation` class, every word of the stack, every
    /// row of the tables of suspended activations and of continuations,
    /// every surviving canonical symbol table entry, and every host root.
    /// Shared by `move_everything`, `compact` and `forward_identity` -- each
    /// builds its own `Forwarder` first, over its own `Relocation`, and then
    /// walks the same roots through it -- so that a field any of them must
    /// reach is named here once, rather than in copies a later change can
    /// update in one and miss in another (`trace.rs`'s reason for existing,
    /// one level up).
    fn forward_everything<M: Relocation>(&mut self, forwarder: &mut Forwarder<M>) {
        use crate::trace::{Trace, Tracer};
        // A married frame's registers are its stack frame's, traced below
        // with the stack; its married field is a stack offset, and no stack
        // frame moves. Walks the objects where they lie now, after the move.
        let ranges = self.space.occupied();
        let words = self.space.words_mut();
        let mut holders = Vec::new();
        forwarder.objects.holder_done();
        for range in ranges {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(words, position);
                let block = block_range(words, header);
                trace_block(&mut words[block], forwarder);
                let offset = offset_of_header(header);
                if forwarder.objects.holder_done() && !space::is_young(offset) {
                    holders.push(offset);
                }
                position = next;
            }
        }
        // After the walk, which has the words borrowed.
        for holder in holders {
            self.remember(holder);
        }
        if let Some(class) = &mut self.activation_class {
            forwarder.object(class);
        }
        // Weak against a full collection, which prunes it before compacting,
        // as it does the symbols below; a move collects nothing. Either
        // way, every entry left names a live object.
        for object in self.mixin_objects.iter_mut().flatten() {
            forwarder.object(object);
        }
        // A full collection nils the stack's leftovers twice, in its
        // marking's trace and again here: idempotent on purpose, since
        // nothing runs between the two and a second pass writes only the
        // words the first wrote.
        Self::trace_stack(&mut self.stack, &self.tables, forwarder);
        self.suspensions.trace(forwarder);
        self.continuations.trace(forwarder);
        // The table is weak -- `begin_marking` deliberately does not walk it
        // (see its doc comment) -- but by the time a caller reaches
        // `compact`, `prune_canonical_symbols` has already run against the
        // finished marking and cleared every entry whose object died. What
        // remains here is still a handle into the space just compacted, and
        // rewriting it is this loop's job like any other: skipping it would
        // leave a stale offset behind for a symbol that is very much alive.
        for object in self.canonical_symbols.iter_mut().flatten() {
            forwarder.object(object);
        }
        // Its inverse is keyed by the offsets just rewritten, so it is
        // rebuilt from the forwarded table rather than forwarded: once per
        // move, in the number of live symbols. `prune_canonical_symbols`
        // already removed every symbol it cleared, so each one left still
        // has its entry.
        let symbols: Vec<Symbol> = self.symbol_of.drain().map(|(_, symbol)| symbol).collect();
        for symbol in symbols {
            if let Some(object) = self.canonical_symbols[symbol.index()] {
                self.symbol_of.insert(object.offset.get(), symbol);
            }
        }
        let roots = self.host_roots.get_mut();
        roots.sweep();
        for slot in &roots.slots {
            let mut value = slot.get();
            forwarder.value(&mut value);
            slot.set(value);
        }
    }

    /// The space's occupancy (`Interpreter::heap_statistics`).
    pub(crate) fn heap_statistics(&self) -> crate::HeapStatistics {
        crate::HeapStatistics {
            live_objects: self.object_count + self.machine_allocations(),
            young_objects: self.young_objects + self.machine_allocations(),
            occupied_bytes: self.space.occupied_bytes(),
            committed_bytes: self.space.committed(),
        }
    }

    /// Records one collection's pause (`World::collect`, the only caller):
    /// counts it and folds its duration into the total and the longest seen
    /// so far. The timing itself happens once per collection in `collect`;
    /// this only updates three fields, so neither costs anything per
    /// allocation or per instruction.
    pub(crate) fn record_collection(&mut self, pause: std::time::Duration) {
        self.collections += 1;
        self.total_pause += pause;
        self.longest_pause = self.longest_pause.max(pause);
    }

    /// Records one scavenge's pause (`World::scavenge`, the only caller),
    /// as `record_collection` records a collection's; `Heap::scavenge`
    /// itself counts it, so a bare heap's scavenge is counted too.
    pub(crate) fn record_scavenge(&mut self, pause: std::time::Duration) {
        self.scavenge_pause += pause;
        self.longest_scavenge_pause = self.longest_scavenge_pause.max(pause);
    }

    /// The collector's counts and pauses since the process started, plus the
    /// space's occupancy right now (`Interpreter::collector_statistics`,
    /// `--collector-statistics`).
    pub(crate) fn collector_statistics(&self) -> crate::CollectorStatistics {
        crate::CollectorStatistics {
            scavenges: self.scavenges,
            scavenge_pause: self.scavenge_pause,
            longest_scavenge_pause: self.longest_scavenge_pause,
            collections: self.collections,
            total_pause: self.total_pause,
            longest_pause: self.longest_pause,
            live_objects: self.object_count + self.machine_allocations(),
        }
    }

    /// Rewrites every object field that holds a resolved future to hold its
    /// value instead (`docs/design/memory.md`, "A resolved future is traced as
    /// its value"): instance and array fields -- an indirection vector is an
    /// `Array` -- and a closure's copied values. Never a register or a host
    /// root: `FutureImplementation>>doesNotUnderstand:`, which forced it, may
    /// still be returning, with the future as `self`, and a host holding the
    /// future holds that object, so a future still held there survives this
    /// collection and is replaced by the first one after it lets go. Runs
    /// before marking, so a future nothing else reaches is garbage by the time
    /// the marker looks. Answers how many references it replaced.
    ///
    /// Which objects are resolved futures, and their values, is `futures`'
    /// test, the one the scavenger's weak pass applies too. A future's
    /// value is never itself a future -- `Mutator::future_computed` follows
    /// a future of a future before it records one, and one whose fields
    /// crafted code made so is malformed and not replaced -- so one pass of
    /// replacement leaves no resolved future behind in any field it
    /// rewrites.
    pub(crate) fn replace_resolved_futures(&mut self, futures: ResolvedFutures) -> usize {
        // First pass: every resolved future's offset and its value.
        // Words, not values: the replacement copies a stored word into
        // another field, so it needs no check and no epoch.
        let mut values: std::collections::HashMap<u64, Word> = std::collections::HashMap::new();
        let ranges = self.space.occupied();
        for range in &ranges {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let words = self.space.words();
                let (header, next) = object_at(words, position);
                position = next;
                let object = offset_of_header(header);
                if let Some(value) = futures.value_of(words, object, |found| found) {
                    values.insert(object.get(), value);
                }
            }
        }
        if values.is_empty() {
            return 0;
        }
        // Second pass: rewrite every field an object owns that names one.
        let mut replaced = 0;
        let mut replace = |fields: &mut [Word]| {
            for field in fields {
                if let Some(offset) = field.object_offset() {
                    if let Some(value) = values.get(&offset.get()) {
                        *field = *value;
                        replaced += 1;
                    }
                }
            }
        };
        let space_words = self.space.words_mut();
        for range in ranges {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(space_words, position);
                position = next;
                let block_words = block_range(space_words, header);
                let block = &mut space_words[block_words];
                // The class word names a class object, never a future.
                let format = Header(block[HEADER]).format();
                let words = &mut block[FIELDS..];
                // A number's digits are no `Value`s, and a reified frame's
                // fields and registers are exempt, as the stack's are. A
                // closure's code id, captured frame and feedback base are
                // exempt too: only its copied values can hold one. So is
                // every field of a class object: its enclosing object,
                // declaring class, superclass and metaclass link are objects
                // no future can stand in for, since a message to a future is
                // answered by its value, and its value-class refusal is the
                // runtime's own record.
                let first_rewritable = match format {
                    Format::FixedPointers
                    | Format::IndexablePointers
                    | Format::WeakPointers
                    | Format::Ephemeron => 0,
                    Format::Closure => closure_field::COUNT.min(words.len()),
                    _ => words.len(),
                };
                for field in &mut words[first_rewritable..] {
                    let mut word = [Word::from_bits(*field)];
                    replace(&mut word);
                    *field = word[0].bits();
                }
            }
        }
        replaced
    }

    /// Pushes the heap's own roots -- the `Activation` class, the activation
    /// stack's words, and the host roots -- onto a new marker and answers it
    /// without draining it, so that the caller can add the step state's roots
    /// (`World::mark_everything_reachable`) before the traversal runs.
    ///
    /// The canonical symbol table and the reified mixins are deliberately
    /// not among these roots: they are the runtime's own weak structures,
    /// beside the `WeakArray`s, `Ephemeron`s and `WeakMap`s of a program
    /// (`docs/design/memory.md`, "Weak references"), pruned against the
    /// finished marking by `prune_canonical_symbols` and
    /// `prune_mixin_objects` rather than kept alive by it. The space
    /// itself is never a root, only what the roots reach.
    pub(crate) fn begin_marking(&mut self) -> mark::Marker {
        use crate::trace::{Trace, Tracer};
        // A mark left set would make this marking skip that object's edges
        // and so collect what only it reaches.
        #[cfg(debug_assertions)]
        {
            let words = self.space.words();
            for range in self.space.occupied() {
                let mut position = range.start / 8;
                while position < range.end / 8 {
                    let (header, next) = object_at(words, position);
                    assert!(
                        !Header(words[header]).marked(),
                        "internal: a mark bit survived the last collection"
                    );
                    position = next;
                }
            }
        }
        let mut marker = mark::Marker::new(reclaim::GatheredIds::new(
            self.tables.applications.len(),
            self.tables.mixins.len(),
            self.tables.codes.len(),
        ));
        if let Some(class) = &mut self.activation_class {
            marker.object(class);
        }
        Self::trace_stack(&mut self.stack, &self.tables, &mut marker);
        self.suspensions.trace(&mut marker);
        self.continuations.trace(&mut marker);
        let roots = self.host_roots.get_mut();
        roots.sweep();
        for slot in &roots.slots {
            let mut value = slot.get();
            marker.value(&mut value);
            slot.set(value);
        }
        marker
    }

    /// Clears every canonical symbol the marking did not reach. The table is
    /// weak, and sound so: `Mutator::new_symbol` is the only
    /// way to make a symbol object (§5.1.6: equal symbols are one object), so
    /// one that is collected was held by nothing -- anything that held it
    /// would have been a root and kept it live -- and the next mint is again
    /// the only one. No live comparison can tell a collected-and-reminted
    /// symbol from one that was never collected (`docs/design/memory.md`,
    /// "Weak references").
    ///
    /// Must run between marking and compaction, never outside that window:
    /// compaction clears the mark bits, and before marking has finished
    /// draining, an entry cleared here reads bits that are not yet complete
    /// and could wrongly prune a symbol the traversal has not reached yet.
    pub(crate) fn prune_canonical_symbols(&mut self) {
        for index in 0..self.canonical_symbols.len() {
            if let Some(object) = self.canonical_symbols[index] {
                if !self.header(object).marked() {
                    self.symbol_of.remove(&object.offset.get());
                    self.canonical_symbols[index] = None;
                }
            }
        }
    }

    /// The scavenge's pass over the weak symbol table, once the copying is
    /// done: a young entry the scavenge copied names the copy, one it did
    /// not is gone, an old one stays (`docs/design/memory.md`, "Weak
    /// references"); `symbol_of` follows each entry. Every entry is checked
    /// as the scavenger checks a handle it traces, and every entry left is
    /// rebranded into the scavenge's epoch.
    fn forward_weak_symbols(
        canonical_symbols: &mut [Option<ObjectReference<'gc>>],
        symbol_of: &mut IdentityMap<u64, Symbol>,
        scavenger: &Scavenger<'_>,
    ) {
        for entry in canonical_symbols.iter_mut() {
            let Some(object) = entry else { continue };
            // A stale entry panics here rather than being made fresh.
            #[cfg(debug_assertions)]
            {
                object.epoch = scavenger.check_and_rebrand(object.origin, object.epoch);
            }
            if space::is_young(object.offset) {
                let words = scavenger.space.words();
                let header = header_index(object.offset);
                if Header(words[header]).format() != Format::Forwarded {
                    symbol_of.remove(&object.offset.get());
                    *entry = None;
                    continue;
                }
                let Some(moved) = Word::from_bits(words[header + CLASS]).object_offset() else {
                    unreachable!("internal: a forwarder names an object")
                };
                if let Some(symbol) = symbol_of.remove(&object.offset.get()) {
                    symbol_of.insert(moved.get(), symbol);
                }
                object.offset = moved;
            }
        }
    }

    /// Runs the marker's work list empty: each object popped unmarked is
    /// marked, and every unmarked object its pointer words name is pushed. The
    /// mark bit is set when the object is visited, not when it is pushed, so an
    /// object reached twice before its visit is popped twice and visited once
    /// -- which is also what ends the traversal on a cycle. A married frame's
    /// registers are its stack frame's, already pushed with the stack, so
    /// `pointer_ranges` leaves them out.
    ///
    /// Only strong words are traced (`object::strong_pointer_ranges`): a
    /// `WeakPointers` object goes on the marker's weak list, and an
    /// `Ephemeron` whose key is an object not yet marked on its ephemeron
    /// list, both for `finish_marking`. Since an object is visited once,
    /// neither list names one twice.
    pub(crate) fn drain_marking(&mut self, marker: &mut mark::Marker) {
        self.drain_marking_recording::<false>(marker);
    }

    /// `drain_marking`, which with `RECORD` also lists every object it
    /// marks on the marker's `reached`, for `finish_marking`'s fixpoint to
    /// look up among the pending ephemerons' keys. A parameter of the type
    /// rather than a flag, so the main marking does not even test it.
    fn drain_marking_recording<const RECORD: bool>(&mut self, marker: &mut mark::Marker) {
        let words = self.space.words_mut();
        while let Some(offset) = marker.next() {
            let index = header_index(offset);
            let header = Header(words[index]);
            if header.marked() {
                continue;
            }
            words[index] = header.with_marked(true).0;
            if RECORD {
                #[cfg(test)]
                pending::work::count_recorded();
                marker.reached.push(offset);
            }
            let block = block_range(words, index);
            let format = header.format();
            // The ids an object names as numbers, which no pointer range
            // reaches: what keeps an application, a mixin, a body and a
            // feedback range from being reclaimed. A field past a short
            // block reads as nil, as `pointer_ranges` clamps it.
            let field = |field: usize| {
                let position = block.start + FIELDS + field;
                if position < block.end {
                    words[position]
                } else {
                    Word::NIL.bits()
                }
            };
            match format {
                Format::Class => marker.gathered.class_words(
                    field(class_field::APPLICATION),
                    field(class_field::APPLIED_CLAUSE),
                ),
                Format::Closure => marker.gathered.closure_words(
                    field(closure_field::CODE_ID),
                    field(closure_field::FEEDBACK_BASE),
                ),
                Format::Frame => marker.gathered.frame_word(field(frame_field::CODE_ID)),
                _ => {}
            }
            let ranges = match format {
                Format::WeakPointers | Format::Ephemeron => {
                    let mut key_died = false;
                    let ranges =
                        object::strong_pointer_ranges(format, &words[block.clone()], |key| {
                            let survives = Word::from_bits(key)
                                .object_offset()
                                .is_none_or(|target| Header(words[header_index(target)]).marked());
                            key_died = !survives;
                            survives
                        });
                    if format == Format::WeakPointers {
                        marker.weak_objects.push(offset);
                    } else if key_died {
                        marker.ephemerons.push(offset);
                    }
                    ranges
                }
                _ => object::pointer_ranges_of(format, &words[block.clone()]),
            };
            for range in ranges {
                for position in range.start + block.start..range.end + block.start {
                    if let Some(target) = Word::from_bits(words[position]).object_offset() {
                        if !Header(words[header_index(target)]).marked() {
                            marker.push(target);
                        }
                    }
                }
            }
        }
    }

    /// Ends a marking whose work list `drain_marking` has emptied: traces
    /// each listed ephemeron whose key has since been marked, drains, and
    /// checks again, until the work list stays empty; then mourns every
    /// ephemeron left unresolved -- its key reached by nothing but
    /// ephemerons -- and clears every word of a listed weak object that
    /// names an unmarked object (`docs/design/core-library.md`, "The heap
    /// and the collectors").
    ///
    /// The first `pending::scanned_checks` checks scan every unresolved
    /// ephemeron; the ones still unresolved after them are indexed by key
    /// (`pending::PendingEphemerons`), and from then on each drain records
    /// what it marks while the index holds one, and each check looks those
    /// objects up and traces the ephemerons keyed by them. Each indexed
    /// ephemeron is traced at most once and each object the fixpoint
    /// marks looked up once, so a chain of ephemerons each keyed by the
    /// previous one's value costs its length, not its square, and a
    /// fixpoint that ends within the scans -- a weak map's -- indexes
    /// nothing.
    ///
    /// Must run before compaction, which forwards only marked objects: a
    /// word left naming a dead one would be forwarded to garbage.
    ///
    /// A resolved future in a weak word or an ephemeron's key is judged by
    /// its value's liveness, as a scavenge judges it: `World::collect` runs
    /// `replace_resolved_futures` before marking, which has rewritten every
    /// such word to the value by the time this reads it.
    ///
    /// Mourning and clearing write nil, which names no young object, so
    /// they are the collector's own rewrites and need no write barrier.
    pub(crate) fn finish_marking(&mut self, marker: &mut mark::Marker) {
        if !marker.ephemerons.is_empty() {
            let scanned_checks = pending::scanned_checks();
            let mut checks = 0;
            // The unresolved ephemerons while the checks scan them, and
            // the index of them afterwards.
            let mut waiting = Vec::new();
            let mut pending = PendingEphemerons::new();
            // Spare buffers the marker's lists are swapped with each round,
            // so a long chain allocates nothing per round.
            let mut listed = Vec::new();
            let mut reached = Vec::new();
            loop {
                std::mem::swap(&mut marker.ephemerons, &mut listed);
                if checks < scanned_checks {
                    waiting.append(&mut listed);
                    let mut position = 0;
                    while position < waiting.len() {
                        #[cfg(test)]
                        pending::work::count_examined();
                        let ephemeron = waiting[position];
                        if self.is_key_marked(ephemeron) {
                            self.push_ephemeron_fields(ephemeron, marker);
                            waiting.swap_remove(position);
                        } else {
                            position += 1;
                        }
                    }
                    checks += 1;
                } else {
                    // Ephemerons the last drain visited before their key
                    // join the index; one whose key that drain marked
                    // meanwhile was marked before it was indexed, so it is
                    // traced here.
                    for ephemeron in listed.drain(..) {
                        #[cfg(test)]
                        pending::work::count_examined();
                        if self.is_key_marked(ephemeron) {
                            self.push_ephemeron_fields(ephemeron, marker);
                        } else {
                            pending.add(ephemeron, self.ephemeron_key(ephemeron));
                        }
                    }
                    std::mem::swap(&mut marker.reached, &mut reached);
                    for offset in reached.drain(..) {
                        if pending.is_empty() {
                            break;
                        }
                        pending.take(offset, |ephemeron| {
                            self.push_ephemeron_fields(ephemeron, marker);
                        });
                    }
                }
                if marker.is_done() {
                    // Only a drain lists ephemerons.
                    debug_assert!(marker.ephemerons.is_empty());
                    break;
                }
                // The scans are over and the fixpoint goes on: what they
                // left unresolved is indexed before the drain that may
                // resolve it records what it marks. A scan only pushes and
                // the drain marks, so no key was marked after a scan passed
                // its ephemeron over, and none needs checking again.
                if checks == scanned_checks {
                    for ephemeron in waiting.drain(..) {
                        pending.add(ephemeron, self.ephemeron_key(ephemeron));
                    }
                }
                // Only the index needs what a drain marks, and an empty one
                // can resolve nothing with it: an ephemeron the drain lists
                // meanwhile meets the check above next round.
                if pending.is_empty() {
                    self.drain_marking(marker);
                } else {
                    self.drain_marking_recording::<true>(marker);
                }
            }
            for ephemeron in waiting.into_iter().chain(pending.unresolved()) {
                self.mourn_ephemeron(ephemeron);
            }
        }
        for weak_object in std::mem::take(&mut marker.weak_objects) {
            self.clear_unmarked_words(weak_object);
        }
    }

    /// The words of the fields of the object at `offset`.
    fn field_range(&self, offset: NonZeroU64) -> Range<usize> {
        let block = block_range(self.space.words(), header_index(offset));
        (block.start + FIELDS).min(block.end)..block.end
    }

    /// Whether the key of the listed ephemeron at `offset` is marked.
    fn is_key_marked(&self, offset: NonZeroU64) -> bool {
        self.ephemeron_key(offset)
            .is_some_and(|key| self.is_marked(key))
    }

    /// The key of the listed ephemeron at `offset`. A listed ephemeron's
    /// key is an object: one with no field or another key is traced whole
    /// and never listed.
    fn ephemeron_key(&self, offset: NonZeroU64) -> Option<NonZeroU64> {
        self.space.words()[self.field_range(offset)]
            .first()
            .and_then(|key| Word::from_bits(*key).object_offset())
    }

    /// Whether the object at `offset` is marked.
    fn is_marked(&self, offset: NonZeroU64) -> bool {
        Header(self.space.words()[header_index(offset)]).marked()
    }

    /// Pushes every unmarked object the fields of the ephemeron at `offset`
    /// name: its key is marked, so its value and finalizer are reached
    /// through it.
    fn push_ephemeron_fields(&self, offset: NonZeroU64, marker: &mut mark::Marker) {
        let words = self.space.words();
        for bits in &words[self.field_range(offset)] {
            if let Some(target) = Word::from_bits(*bits).object_offset() {
                if !Header(words[header_index(target)]).marked() {
                    marker.push(target);
                }
            }
        }
    }

    /// Sets every field of the ephemeron at `offset` to nil: its key is
    /// reached by nothing but ephemerons, so it and what it guards are gone.
    fn mourn_ephemeron(&mut self, offset: NonZeroU64) {
        let fields = self.field_range(offset);
        self.space.words_mut()[fields].fill(Word::NIL.bits());
    }

    /// Sets every word of the weak object at `offset` that names an
    /// unmarked object to nil, so that no slot names what the compaction
    /// is about to reclaim.
    fn clear_unmarked_words(&mut self, offset: NonZeroU64) {
        let fields = self.field_range(offset);
        let words = self.space.words_mut();
        for position in fields {
            if let Some(target) = Word::from_bits(words[position]).object_offset() {
                if !Header(words[header_index(target)]).marked() {
                    words[position] = Word::NIL.bits();
                }
            }
        }
    }

    /// Clears every object's mark bit, for a test that marks without
    /// compacting and then goes on to collect.
    #[cfg(test)]
    fn clear_marks_for_test(&mut self) {
        let ranges = self.space.occupied();
        let words = self.space.words_mut();
        for range in ranges {
            let mut position = range.start / 8;
            while position < range.end / 8 {
                let (header, next) = object_at(words, position);
                words[header] = Header(words[header]).with_marked(false).0;
                position = next;
            }
        }
    }

    /// Marks from the heap's own roots and `roots`, finishes the marking's
    /// weak objects and ephemerons, prunes the canonical symbol table and
    /// the reified mixins, compacts, and forwards `roots`, for the heap's own
    /// tests. A real collection goes through
    /// `World`.
    #[cfg(test)]
    fn collect_for_test(&mut self, roots: &mut [&mut Value<'gc>]) {
        use crate::trace::Tracer;
        let mut marker = self.begin_marking();
        for root in roots.iter_mut() {
            marker.value(root);
        }
        self.drain_marking(&mut marker);
        self.finish_marking(&mut marker);
        self.prune_canonical_symbols();
        self.prune_mixin_objects();
        let mut forwarder = self.compact();
        for root in roots.iter_mut() {
            forwarder.value(root);
        }
    }
}

/// A class object as instantiation asks of it as a level
/// (`Heap::class_level`), borrowed from the heap's tables.
#[derive(Clone, Copy)]
pub(crate) struct ClassLevel<'heap, 'gc> {
    application: Option<&'heap Rc<MixinApplication>>,
    applied: Option<(&'heap Rc<Mixin>, usize)>,
    superclass: Option<ClassReference<'gc>>,
}

impl<'heap, 'gc> ClassLevel<'heap, 'gc> {
    /// `Heap::class_application`, borrowed.
    pub(crate) fn application(&self) -> Option<&'heap Rc<MixinApplication>> {
        self.application
    }

    /// `Heap::class_level_facts`.
    pub(crate) fn level_facts(&self) -> Option<crate::mixin::LevelFacts> {
        self.application.map(|application| application.level_facts)
    }

    /// `Heap::class_superclass`.
    pub(crate) fn superclass(&self) -> Option<ClassReference<'gc>> {
        self.superclass
    }

    /// `Heap::class_is_applied_level`.
    pub(crate) fn is_applied_level(&self) -> bool {
        self.applied
            .is_some_and(|(declaration, index)| index < declaration.mixin_applications.len())
    }

    /// `Heap::class_body_less_declaration`.
    pub(crate) fn body_less_declaration(&self) -> Option<&'heap Rc<Mixin>> {
        let (declaration, index) = self.applied?;
        (!declaration.has_body && index + 1 == declaration.mixin_applications.len())
            .then_some(declaration)
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]
    use super::*;

    /// A bare heap that can reify: one with an `Activation` class recorded,
    /// as the bootstrap records the real one.
    fn frame_heap<'gc>() -> Heap<'gc> {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Activation");
        heap.set_activation_class(class);
        heap
    }

    /// Reading a handle through a heap that did not allocate it is the one
    /// failure offsets introduce and the one they cannot answer correctly:
    /// the offset names a different, perfectly valid object. Debug builds
    /// must refuse it rather than answer.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_a_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(0);
        let object = origin.allocate_string(class, crate::CodePoints::from("mine"));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(0);
        other.allocate_string(other_class, crate::CodePoints::from("theirs"));
        other.allocate_string(other_class, crate::CodePoints::from("theirs"));

        // In range for `other` -- which is exactly why this is silent
        // without the guard.
        let _ = other.str_value(object);
    }

    /// The sibling case, and the one an accessor guard cannot reach: `same` is
    /// handed two handles and no `Heap`, so nothing else in the crate would
    /// notice that they come from different heaps. Two unrelated objects
    /// sharing an offset would answer "identical" -- silently, and
    /// `ObjectReference::same` *is* object identity. Pinned here so the
    /// guarantee `CLAUDE.md` states is tested rather than asserted.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(0);
        let mine = origin.allocate_string(class, crate::CodePoints::from("mine"));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(0);
        let theirs = other.allocate_string(other_class, crate::CodePoints::from("theirs"));

        // Both are the second object their own heap allocated, so both
        // carry the same index: without the guard this answers `true`.
        let _ = mine.same(theirs);
    }

    /// The same hole for reified frames. `FrameReference::same` is what the
    /// image round-trip tests check sharing by -- a closure's `captured_frame`
    /// and `home` naming one frame or two -- so a false "identical" would let a
    /// round trip that split one frame into two pass. (A non-local return names
    /// its target by `ActivationReference`; see
    /// `comparing_two_activation_handles_from_different_heaps_panics`.)
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_frame_handles_from_different_heaps_panics() {
        let mut origin = frame_heap();
        let mine = origin.allocate_placeholder_frame(0);

        let mut other = frame_heap();
        let theirs = other.allocate_placeholder_frame(0);

        let _ = mine.same(theirs);
    }

    /// Why `object_identity_key` exists at all, rather than callers keying
    /// on `identity_hash`. Both allocators that fix the hash at 0 --
    /// `allocate_object` (every object nothing has asked for its hash) and
    /// `allocate_self_referential` -- can produce several objects in one
    /// heap sharing that hash, and a map keyed on it would fold them into
    /// one entry. `image::decode` builds a whole graph out of exactly those
    /// placeholders, so this is the case that would break, not a
    /// hypothetical one.
    ///
    /// The cross-heap case is pinned separately, by
    /// `keying_a_handle_through_a_foreign_heap_panics` below.
    #[test]
    fn identity_keys_separate_objects_that_share_an_identity_hash() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let first = heap.allocate_instance(class, 0);
        let second = heap.allocate_instance(class, 0);

        assert_eq!(heap.object_identity_hash(first), 0);
        assert_eq!(
            heap.object_identity_hash(first),
            heap.object_identity_hash(second)
        );
        assert_ne!(
            heap.object_identity_key(first),
            heap.object_identity_key(second)
        );
        // Stable for one handle, which is what every caller relies on.
        assert_eq!(
            heap.object_identity_key(first),
            heap.object_identity_key(first)
        );
    }

    /// A handle is a byte offset: a multiple of 8, so that the value word holding
    /// it has the object tag, and at least 16, so that an all-zero word -- a
    /// missed initialization -- never names a live object.
    #[test]
    fn an_object_word_is_a_byte_offset_past_the_first_sixteen_bytes() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let object = heap.allocate_instance(class, 1);
        for handle in [class, object] {
            let word = heap.store(Value::object(handle)).bits();
            assert_eq!(word & 0b111, 0, "an object word carries the object tag");
            assert!(word >= 16);
            assert_eq!(heap.object_identity_key(handle) as u64, word);
        }
    }

    /// Why `identity_key` lives on `Heap`. A key computed from a handle
    /// alone could carry no origin check, so two objects from different
    /// heaps would produce the same key and collide in a caller's map --
    /// `image::encode::GraphTable` would give one object the other's id.
    /// Requiring the `Heap` is what makes the check possible, and the
    /// compiler is what requires it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn keying_a_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(0);
        let mine = origin.allocate_string(class, crate::CodePoints::from("mine"));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(0);
        let theirs = other.allocate_string(other_class, crate::CodePoints::from("theirs"));

        // Both are the second object their own heap allocated, so both
        // carry the same index: without the guard these two keys are equal.
        assert_ne!(
            origin.object_identity_key(mine),
            origin.object_identity_key(theirs)
        );
    }

    #[test]
    fn identity_hashes_are_monotonic_and_start_at_one() {
        let mut heap = Heap::new();
        assert_eq!(heap.allocate_identity_hash(), 1);
        assert_eq!(heap.allocate_identity_hash(), 2);
        assert_eq!(heap.allocate_identity_hash(), 3);
    }

    #[test]
    fn advancing_past_a_maximum_never_moves_the_counter_backwards() {
        let mut heap = Heap::new();
        heap.advance_identity_hash_past(10);
        assert_eq!(heap.allocate_identity_hash(), 11);
        // A lower maximum must not rewind: an image's ids and freshly
        // allocated ones would then collide.
        heap.advance_identity_hash_past(3);
        assert_eq!(heap.allocate_identity_hash(), 12);
    }

    #[test]
    fn the_identity_hash_counter_wraps_within_twenty_two_bits_and_skips_zero() {
        let mut heap = Heap::new();
        heap.next_identity_hash = IDENTITY_HASH_LIMIT - 1;
        assert_eq!(heap.allocate_identity_hash(), IDENTITY_HASH_LIMIT - 1);
        assert_eq!(heap.allocate_identity_hash(), 1);
        assert_eq!(heap.allocate_identity_hash(), 2);
    }

    #[test]
    fn advancing_past_the_widest_hash_wraps_the_counter_to_one() {
        let mut heap = Heap::new();
        heap.advance_identity_hash_past(IDENTITY_HASH_LIMIT - 1);
        assert_eq!(heap.allocate_identity_hash(), 1);
    }

    #[test]
    fn an_object_header_records_its_format_and_size() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let narrow = heap.allocate_string(class, crate::CodePoints::from("abcdefghi"));
        let header = heap.header(narrow);
        assert_eq!(header.format(), Format::Bytes);
        assert_eq!(header.word_count(), 1 + 2, "nine bytes take two words");
        let wide = heap.allocate_string(class, crate::CodePoints::from("€uro"));
        let header = heap.header(wide);
        assert_eq!(header.format(), Format::ThirtyTwoBit);
        assert_eq!(header.word_count(), 1 + 2);
        let array = heap.allocate_array(class, &[Value::nil(); 3]);
        let header = heap.header(array);
        assert_eq!(header.format(), Format::IndexablePointers);
        assert_eq!(header.word_count(), 1 + 3);
        let negative =
            heap.allocate_large_integer(class, crate::ArbitraryInteger::from_i128(-(1 << 100)));
        let header = heap.header(negative);
        assert_eq!(header.format(), Format::SixtyFourBit);
        assert!(header.negative());
        assert_eq!(header.word_count(), 1 + 2);
    }

    #[test]
    fn a_self_referential_class_is_its_own_class() {
        let mut heap = Heap::new();
        // A filler object first, so `class` does not land at the first
        // offset -- an implementation that always answered the first
        // object's handle would otherwise still pass this test.
        heap.allocate_self_referential(0);
        let class = heap.allocate_self_referential(0);
        assert!(heap.object_class(class).same(class));
    }

    /// A returned top-level frame with a register file of `register_count`
    /// nils and nothing else.
    fn top_level_frame<'gc>(heap: &mut Heap<'gc>, register_count: usize) -> FrameReference<'gc> {
        heap.allocate_returned_top_level_frame(
            Value::nil(),
            &vec![Value::nil(); register_count],
            None,
            None,
        )
    }

    /// A body of `argument_count` arguments in `register_count` registers,
    /// every one above the header a named temporary: what a test writes
    /// there is the body's own, which no collection nils.
    fn shaped_code(
        argument_count: u16,
        register_count: u16,
    ) -> std::rc::Rc<crate::bytecode::CompiledCode> {
        // Sixteen instructions, so that a test may stop a frame of it at
        // any program counter it names.
        let mut code = crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Method,
            (0..16)
                .map(|_| crate::bytecode::Instruction::Return { register: 0 })
                .collect(),
            argument_count,
            register_count,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        );
        code.local_count = code.local_count.max(register_count);
        std::rc::Rc::new(code)
    }

    /// A dead initializer frame with a distinct value in each of
    /// `program_counter`, its argument count and its register count.
    /// `frame_register_shape` returns those three as a same-typed tuple, so a
    /// frame built from all-zero or otherwise equal fields cannot catch the
    /// tuple's positions being silently transposed -- exactly the
    /// silent-wrong-output risk this conversion carries.
    fn shaped_frame<'gc>(
        heap: &mut Heap<'gc>,
        program_counter: usize,
        argument_count: u16,
        register_count: u16,
    ) -> FrameReference<'gc> {
        let class = named_class(heap, "Shaped");
        let code = heap.code_id(&shaped_code(argument_count, register_count));
        heap.allocate_frame(
            FrameFields {
                sender: None,
                home: None,
                receiver: Value::nil(),
                current_class: Some(class),
                program_counter,
                code: Some(code),
                kind: FrameKind::Initializer(class, InitializerBody::OwnPart),
                uncontinuable: true,
            },
            &[],
            usize::from(register_count),
        )
    }

    /// A frame is addressed by its offset in the space, so a handle must
    /// stay valid as the space grows. Under `Rc` this was free; under
    /// offsets it is the property the representation rests on.
    #[test]
    fn a_frame_handle_survives_the_space_growing_under_it() {
        let mut heap = frame_heap();
        let first = shaped_frame(&mut heap, 3, 2, 5);
        for _ in 0..1000 {
            top_level_frame(&mut heap, 0);
        }
        assert!(heap.frame_receiver(first).is_nil());
        assert_eq!(heap.frame_register_shape(first), (3, 2, 5));
    }

    /// Frame identity is what the image round trip checks sharing by -- a
    /// closure's `captured_frame` and `home` naming one frame or two -- so two
    /// reified frames may never compare equal however alike their contents are.
    #[test]
    fn two_frames_are_never_the_same_activation() {
        let mut heap = frame_heap();
        let first = top_level_frame(&mut heap, 0);
        let second = top_level_frame(&mut heap, 0);
        assert!(!first.same(second));
        assert!(first.same(first));
    }

    /// The object counterpart of the frame test above: a handle must stay
    /// valid as the space grows, and two objects with identical contents
    /// must never be the same object.
    #[test]
    fn an_object_handle_survives_the_space_growing_under_it() {
        let mut heap = Heap::new();
        // A filler object first, so neither `class` nor `first` lands at
        // the first offset -- see `a_self_referential_class_is_its_own_class`.
        heap.allocate_self_referential(0);
        let class = heap.allocate_self_referential(0);
        let first = heap.allocate_string(class, crate::CodePoints::from("first"));
        for _ in 0..1000 {
            heap.allocate_string(class, crate::CodePoints::from("filler"));
        }
        assert_eq!(heap.str_value(first).as_deref(), Some("first"));
        assert!(heap.object_class(first).same(class));

        let twin = heap.allocate_string(class, crate::CodePoints::from("first"));
        assert!(!first.same(twin));
    }

    /// A minimal class object real enough for `describe` to read a mixin
    /// name off it. Mirrors `types.rs`'s own `dummy_class` test helper,
    /// duplicated rather than shared because that one is private to
    /// `types.rs`'s test module.
    fn named_class<'gc>(heap: &mut Heap<'gc>, name: &str) -> ClassReference<'gc> {
        builtin_named_class(heap, name, None)
    }

    /// `named_class`, marked as the core-library class `builtin`.
    fn builtin_named_class<'gc>(
        heap: &mut Heap<'gc>,
        name: &str,
        builtin: Option<BuiltinClass>,
    ) -> ClassReference<'gc> {
        let mixin = std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name)));
        let level_facts = crate::mixin::LevelFacts::of(&mixin);
        let application = heap.register_application(crate::mixin::MixinApplication {
            index: crate::heap::ApplicationIndex::UNREGISTERED,
            mixin,
            superclass: None,
            inherits_value: false,
            first_mutable_slot: None,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
            instance_header: None,
            level_facts,
        });
        heap.allocate_self_referential_class(NewClass {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin,
        })
    }

    /// The reified mixin of a fresh mixin named `name` in `heap`, recorded
    /// as `Mutator::reified_mixin` records it, and the mixin, which the
    /// caller keeps so that its id stays its own.
    fn recorded_mixin_object<'gc>(
        heap: &mut Heap<'gc>,
        class: ClassReference<'gc>,
        name: &str,
    ) -> (Rc<Mixin>, MixinId) {
        let mixin = Rc::new(Mixin::empty(Symbol::intern(name)));
        let id = heap.mixin_id(&mixin);
        let object = heap.allocate_mixin_object(class, id);
        heap.record_mixin_object(id, object);
        (mixin, id)
    }

    /// The table of reified mixins is weak against a full collection
    /// alone: with nothing else naming it, a reified mixin survives a
    /// scavenge and a move, and stays the one object the table answers for
    /// its id; a full collection keeps it, as that one object, while a root
    /// reaches it, and clears its entry once nothing does.
    #[test]
    fn a_reified_mixin_survives_every_scavenge_and_move_and_lives_while_reached() {
        let mut heap = Heap::new();
        let class = builtin_named_class(&mut heap, "Mixin", Some(BuiltinClass::Mixin));
        let (_mixin, id) = recorded_mixin_object(&mut heap, class, "Reified");
        let still_reified = |heap: &Heap<'_>| {
            let object = heap.mixin_object(id).expect("the table holds the object");
            assert_eq!(heap.mixin_object_id(object), Some(id));
            assert_eq!(heap.describe(Value::object(object)), "the mixin Reified");
        };
        scavenge_bare(&mut heap, false, &mut []);
        still_reified(&heap);
        scavenge_bare(&mut heap, true, &mut []);
        still_reified(&heap);
        #[cfg(debug_assertions)]
        {
            move_bare_heap(&mut heap);
            still_reified(&heap);
        }
        let mut reached = Value::object(heap.mixin_object(id).expect("the table holds it"));
        heap.collect_for_test(&mut [&mut reached]);
        still_reified(&heap);
        assert_eq!(
            heap.mixin_object(id).map(|object| object.offset),
            reached.as_object().map(|object| object.offset),
            "the root and the table name one object"
        );
        heap.collect_for_test(&mut []);
        assert!(heap.mixin_object(id).is_none());
    }

    /// Only the canonical object of a mixin is read as one: neither a
    /// `Fraction` nor an ordinary instance whose one field names a row of
    /// the table, nor an object of the class `Mixin` that the table does
    /// not hold.
    #[test]
    fn an_object_of_another_class_is_no_reified_mixin() {
        let mut heap = Heap::new();
        let mixin_class = builtin_named_class(&mut heap, "Mixin", Some(BuiltinClass::Mixin));
        let (_mixin, id) = recorded_mixin_object(&mut heap, mixin_class, "Recorded");
        let row = Value::small_integer(i64::try_from(id.index()).expect("a small id"));
        let fraction_class =
            builtin_named_class(&mut heap, "Fraction", Some(BuiltinClass::Fraction));
        let fraction = heap.allocate_fraction(fraction_class, row, Value::small_integer(2));
        assert_eq!(heap.mixin_object_id(fraction), None);
        let ordinary_class = named_class(&mut heap, "Ordinary");
        let ordinary = heap.allocate_instance(ordinary_class, 1);
        assert!(heap.instance_set_field(ordinary, 0, row));
        assert_eq!(heap.mixin_object_id(ordinary), None);
        let unrecorded = heap.allocate_mixin_object(mixin_class, id);
        assert_eq!(heap.mixin_object_id(unrecorded), None);
        assert_ne!(
            heap.describe(Value::object(unrecorded)),
            "the mixin Recorded"
        );
    }

    #[test]
    fn a_class_objects_applied_clause_round_trips_through_its_field() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Level");
        let declaration = std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Applying")));
        assert!(heap.class_set_applied_clause(
            class,
            crate::mixin::AppliedClause {
                declaration: declaration.clone(),
                index: 3,
                named_builtin: Some(BuiltinClass::Array),
            },
        ));
        let origin = heap.class_applied_origin(class).expect("an applied level");
        assert!(std::rc::Rc::ptr_eq(&origin.declaration, &declaration));
        assert_eq!(origin.index, 3);
        assert_eq!(origin.named_builtin, Some(BuiltinClass::Array));
    }

    #[test]
    fn a_string_of_nine_bytes_spans_two_words_and_answers_each_code_point() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "String");
        let object = heap.allocate_string(class, crate::CodePoints::from("abcdefghi"));
        assert_eq!(heap.string_size(object), Some(9));
        assert_eq!(heap.string_code_point_at(object, 8), Some(u32::from('i')));
        assert_eq!(heap.string_code_point_at(object, 9), None);
        assert_eq!(heap.str_value(object).as_deref(), Some("abcdefghi"));
    }

    #[test]
    fn a_wide_string_keeps_its_width_and_its_text() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "String");
        let object = heap.allocate_string(class, crate::CodePoints::from("€uro 😀"));
        assert_eq!(heap.string_size(object), Some(6));
        assert_eq!(heap.str_value(object).as_deref(), Some("€uro 😀"));
    }

    #[test]
    fn every_string_accessor_reads_the_packed_units() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "String");
        for text in [
            "",
            "a",
            "abcdefgh",
            "abcdefghi",
            "naïve",
            "€",
            "€u",
            "€uro 😀",
        ] {
            let object = heap.allocate_string(class, crate::CodePoints::from(text));
            let expected = crate::CodePoints::from(text);
            assert_eq!(heap.string_size(object), Some(expected.size()), "{text}");
            for index in 0..=expected.size() {
                assert_eq!(
                    heap.string_code_point_at(object, index),
                    expected.code_point_at(index),
                    "{text} at {index}"
                );
            }
            assert_eq!(heap.string_code_points(object), Some(expected.clone()));
            assert_eq!(heap.string_hash(object), Some(expected.content_hash()));
            let twin = heap.allocate_string(class, crate::CodePoints::from(text));
            assert_eq!(heap.strings_equal(object, twin), Some(true), "{text}");
            assert_eq!(
                heap.string_concatenation(object, twin),
                Some(expected.concatenate(&expected))
            );
            assert_eq!(
                heap.string_slice(object, 0, expected.size()),
                Some(expected)
            );
        }
        let short = heap.allocate_string(class, crate::CodePoints::from("abc"));
        let longer = heap.allocate_string(class, crate::CodePoints::from("abcd"));
        assert_eq!(heap.strings_equal(short, longer), Some(false));
    }

    #[test]
    fn only_an_object_of_the_builtin_symbol_class_answers_its_symbol() {
        let mut heap = Heap::new();
        let symbol_class = builtin_named_class(&mut heap, "Symbol", Some(BuiltinClass::Symbol));
        let string_class = named_class(&mut heap, "String");
        let symbol = Symbol::intern("at:put:");
        let object = canonical_symbol_for_test(&mut heap, symbol_class, symbol);
        assert_eq!(heap.symbol_value(object), Some(symbol));
        assert!(heap.is_string(object));
        assert_eq!(heap.str_value(object).as_deref(), Some("at:put:"));
        assert_eq!(heap.describe(Value::object(object)), "the Symbol #at:put:");
        let string = heap.allocate_string(string_class, crate::CodePoints::from("at:put:"));
        assert_eq!(heap.symbol_value(string), None);
        assert_eq!(
            heap.describe(Value::object(string)),
            "the String \"at:put:\""
        );
    }

    /// Allocates `symbol`'s object and records it as canonical, as
    /// `Mutator::new_symbol` does: a bare heap has no mutator.
    fn canonical_symbol_for_test<'gc>(
        heap: &mut Heap<'gc>,
        class: ClassReference<'gc>,
        symbol: Symbol,
    ) -> ObjectReference<'gc> {
        let object = heap.allocate_symbol(class, symbol);
        heap.record_canonical_symbol(symbol, object);
        object
    }

    /// `symbol_value` is a probe of the canonical table's inverse, keyed by
    /// offset, so it must follow a symbol across every compaction and
    /// every move, and forget one the collector pruned.
    #[test]
    fn a_symbol_answers_its_symbol_across_collections_and_moves_and_a_pruned_one_leaves_no_entry() {
        let mut heap = Heap::new();
        let symbol_class = builtin_named_class(&mut heap, "Symbol", Some(BuiltinClass::Symbol));
        let string_class = named_class(&mut heap, "String");
        // Garbage below the symbols, so that compaction moves them.
        heap.allocate_string(string_class, crate::CodePoints::from("garbage"));
        let kept_symbol = Symbol::intern("kept:symbol:");
        let pruned_symbol = Symbol::intern("pruned:symbol:");
        let mut kept = Value::object(canonical_symbol_for_test(
            &mut heap,
            symbol_class,
            kept_symbol,
        ));
        canonical_symbol_for_test(&mut heap, symbol_class, pruned_symbol);
        let mut string = Value::object(
            heap.allocate_string(string_class, crate::CodePoints::from("kept:symbol:")),
        );
        let offset_before = kept.as_object().map(|object| object.offset);
        heap.collect_for_test(&mut [&mut kept, &mut string]);
        let object = kept.as_object().expect("a root keeps its object");
        assert_ne!(
            Some(object.offset),
            offset_before,
            "the collection moved the symbol"
        );
        assert_eq!(heap.symbol_value(object), Some(kept_symbol));
        assert_eq!(heap.canonical_symbol(pruned_symbol).map(|_| ()), None);
        let live = heap.canonical_symbols.iter().flatten().count();
        assert_eq!(heap.symbol_of.len(), live);
        assert_eq!(live, 1);
        let string = string.as_object().expect("a root keeps its object");
        assert_eq!(heap.symbol_value(string), None);
        #[cfg(debug_assertions)]
        {
            let roots = [
                heap.stash(Value::object(object)),
                heap.stash(Value::object(string)),
            ];
            move_bare_heap(&mut heap);
            let object = heap
                .fetch(&roots[0])
                .as_object()
                .expect("a root keeps its object");
            let string = heap
                .fetch(&roots[1])
                .as_object()
                .expect("a root keeps its object");
            assert_eq!(heap.symbol_value(object), Some(kept_symbol));
            assert_eq!(heap.symbol_value(string), None);
            assert_eq!(heap.symbol_of.len(), 1);
        }
    }

    /// Whether `value` names the same object as `object`.
    fn names<'gc>(value: Value<'gc>, object: ObjectReference<'gc>) -> bool {
        value.as_object().is_some_and(|named| named.same(object))
    }

    /// A collection keeps what something else holds strongly, forgets an
    /// object only the weak array holds, and leaves an immediate alone.
    #[test]
    fn a_weak_array_keeps_a_strongly_held_object_and_forgets_the_rest() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let weak_class = named_class(&mut heap, "WeakArray");
        // Below the kept object, so that compaction moves it.
        let forgotten = heap.allocate_instance(class, 0);
        let held = heap.allocate_instance(class, 0);
        let weak = heap.allocate_weak_array(weak_class, 3);
        let elements = [
            Value::object(held),
            Value::object(forgotten),
            Value::small_integer(7),
        ];
        for (index, element) in elements.into_iter().enumerate() {
            assert!(heap.weak_array_set_element(weak, index, element));
        }
        let held_offset = held.offset;
        let mut weak_root = Value::object(weak);
        let mut held_root = Value::object(held);
        heap.collect_for_test(&mut [&mut weak_root, &mut held_root]);
        let weak = rooted(weak_root);
        let held = rooted(held_root);
        assert_ne!(held.offset, held_offset, "the collection moved the object");
        assert_eq!(heap.weak_array_size(weak), Some(3));
        let elements = heap
            .weak_array_elements(weak)
            .expect("a weak array answers its elements");
        assert!(names(elements[0], held), "the word follows its object");
        assert!(
            elements[1].is_nil(),
            "an object only the array held is gone"
        );
        assert_eq!(elements[2].as_small_integer(), Some(7));
        assert!(names(
            heap.weak_array_element(weak, 0).unwrap_or(Value::nil()),
            held
        ));
        assert!(heap.weak_array_element(weak, 3).is_none());
    }

    /// A live key keeps the ephemeron's value alive, though nothing else
    /// reaches it.
    #[test]
    fn an_ephemeron_with_a_live_key_keeps_its_value() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let key = heap.allocate_instance(class, 0);
        let value = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(value, 0, Value::small_integer(42)));
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
        assert_eq!(
            heap.ephemeron_fields(ephemeron).map(|fields| fields.len()),
            Some(3)
        );
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        let mut ephemeron_root = Value::object(ephemeron);
        let mut key_root = Value::object(key);
        // The work list is a stack: the ephemeron is visited before its
        // key, so its value is reached only once the marking is finished.
        heap.collect_for_test(&mut [&mut key_root, &mut ephemeron_root]);
        let ephemeron = rooted(ephemeron_root);
        let key = rooted(key_root);
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron answers its fields");
        assert!(names(fields[0], key));
        let value = rooted(fields[1]);
        assert_eq!(
            heap.instance_field(value, 0)
                .and_then(Value::as_small_integer),
            Some(42)
        );
        assert!(fields[2].is_nil());
    }

    /// A key nothing but the ephemeron reaches is collected, and the
    /// ephemeron is mourned: every field reads nil.
    #[test]
    fn an_ephemeron_with_a_dead_key_is_mourned() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
        for index in 0..3 {
            let field = heap.allocate_instance(class, 0);
            assert!(heap.ephemeron_set_field(ephemeron, index, Value::object(field)));
        }
        let mut ephemeron_root = Value::object(ephemeron);
        heap.collect_for_test(&mut [&mut ephemeron_root]);
        let fields = heap
            .ephemeron_fields(rooted(ephemeron_root))
            .expect("an ephemeron answers its fields");
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().all(|field| field.is_nil()));
    }

    /// An immediate or nil key is never collected, so the value it guards
    /// is held strongly.
    #[test]
    fn an_ephemeron_with_an_immediate_or_nil_key_holds_its_value() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let mut roots = Vec::new();
        for (key, payload) in [(Value::nil(), 1), (Value::small_integer(7), 2)] {
            let value = heap.allocate_instance(class, 1);
            assert!(heap.instance_set_field(value, 0, Value::small_integer(payload)));
            let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
            assert!(heap.ephemeron_set_field(ephemeron, 0, key));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
            roots.push(Value::object(ephemeron));
        }
        let [mut first, mut second] = [roots[0], roots[1]];
        heap.collect_for_test(&mut [&mut first, &mut second]);
        for (ephemeron, key, payload) in [(first, None, 1), (second, Some(7), 2)] {
            let ephemeron = rooted(ephemeron);
            let key_read = heap
                .ephemeron_field(ephemeron, 0)
                .expect("an ephemeron has a key");
            assert_eq!(key_read.as_small_integer(), key);
            assert_eq!(key_read.is_nil(), key.is_none());
            let value = rooted(
                heap.ephemeron_field(ephemeron, 1)
                    .expect("an ephemeron has a value"),
            );
            assert_eq!(
                heap.instance_field(value, 0)
                    .and_then(Value::as_small_integer),
                Some(payload)
            );
        }
    }

    /// 512 ephemerons, each one's value the next one's key: while the first
    /// key is held by nothing, no key is ever reached but through a value
    /// a dead key guards, and the whole chain is mourned; once the first
    /// key is rooted, every value survives, one round of the fixpoint
    /// apiece.
    #[test]
    fn a_chain_of_ephemerons_whose_keys_only_their_values_reach_dies_whole() {
        collect_a_chain_of_ephemerons(512);
    }

    /// The chain at a length whose square a rescan per round would make
    /// obvious: the fixpoint looks each object it marks up once and
    /// traces each ephemeron once.
    #[test]
    fn a_long_chain_of_ephemerons_costs_its_length_in_a_collection() {
        const LENGTH: usize = 20_000;
        let [unrooted, rooted_work] = collect_a_chain_of_ephemerons(LENGTH);
        assert_eq!(
            (unrooted.lookups, unrooted.resolutions),
            (0, 0),
            "nothing is reached, so nothing is looked up"
        );
        for work in [unrooted, rooted_work] {
            assert!(
                work.examined <= (pending::SCANNED_CHECKS + 1) * LENGTH,
                "{} ephemerons examined for {LENGTH} links",
                work.examined
            );
        }
        let pending::work::Work {
            lookups,
            resolutions,
            ..
        } = rooted_work;
        assert!(
            lookups <= 2 * LENGTH,
            "{lookups} lookups for {LENGTH} links"
        );
        assert!(
            resolutions <= LENGTH,
            "{resolutions} resolutions for {LENGTH} links"
        );
    }

    /// A weak map's shape: every key reachable, but every ephemeron
    /// visited before its key is marked. The first check resolves every
    /// listed ephemeron, so the index stays empty and the fixpoint records
    /// and looks up nothing.
    #[test]
    fn ephemerons_whose_keys_are_all_reachable_are_resolved_without_lookups_in_a_collection() {
        const COUNT: usize = 1_000;
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let array_class = named_class(&mut heap, "Array");
        let mut elements = Vec::with_capacity(2 * COUNT);
        let mut ephemerons = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            let key = heap.allocate_instance(class, 0);
            let value = heap.allocate_instance(class, 0);
            let ephemeron = heap.allocate_ephemeron(ephemeron_class, 2);
            assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
            elements.push(Value::object(key));
            ephemerons.push(Value::object(ephemeron));
        }
        // The work list is a stack: the ephemerons, last in the array, are
        // visited before any key.
        elements.extend(ephemerons);
        let mut array_root = Value::object(heap.allocate_array(array_class, &elements));
        pending::work::take();

        heap.collect_for_test(&mut [&mut array_root]);

        let work = pending::work::take();
        assert_eq!(work.examined, COUNT, "each ephemeron is listed once");
        assert_eq!(
            (work.recorded, work.lookups, work.resolutions),
            (0, 0, 0),
            "nothing is pending, so nothing is recorded or looked up"
        );
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        for ephemeron in &elements[COUNT..] {
            let fields = heap
                .ephemeron_fields(rooted(*ephemeron))
                .expect("an ephemeron answers its fields");
            assert!(
                fields.iter().all(|field| field.as_object().is_some()),
                "the ephemeron keeps its key and its value"
            );
        }
    }

    /// The marking twin of the scavenge's
    /// `an_ephemeron_whose_key_the_last_scan_copies_after_it_is_kept_in_a_scavenge`:
    /// the only scan passes over `E_x` and resolves `E_y`, whose value
    /// `K_x` it pushes. The switch indexes `E_x` before the drain marks
    /// `K_x`, so that drain records it and the lookup resolves `E_x`.
    #[test]
    fn an_ephemeron_whose_key_the_last_scan_reaches_after_it_is_kept_in_a_collection() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let array_class = named_class(&mut heap, "Array");
        let [key_x, value_x, key_y] = [0; 3].map(|_| heap.allocate_instance(class, 0));
        let holder_y = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder_y, 0, Value::object(key_y)));
        let ephemeron_x = keyed_ephemeron(&mut heap, ephemeron_class, key_x, value_x);
        let ephemeron_y = keyed_ephemeron(&mut heap, ephemeron_class, key_y, key_x);
        // The work list is a stack: `E_x`, then `E_y`, then `H_y`, which
        // marks `K_y`.
        let mut array_root = Value::object(heap.allocate_array(
            array_class,
            &[
                Value::object(holder_y),
                Value::object(ephemeron_y),
                Value::object(ephemeron_x),
            ],
        ));
        pending::work::set_scanned_checks(1);
        pending::work::take();

        heap.collect_for_test(&mut [&mut array_root]);

        let resolutions = pending::work::take().resolutions;
        assert_eq!(resolutions, 1, "E_x through K_x's lookup");
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let fields = heap
            .ephemeron_fields(rooted(elements[2]))
            .expect("an ephemeron answers its fields");
        assert!(
            fields.iter().all(|field| field.as_object().is_some()),
            "E_x keeps K_x and v_x"
        );
    }

    /// A weak map with dead keys: half the ephemerons keyed by objects
    /// only the array after them reaches, half by objects nothing else
    /// does. The first check resolves the live half, whose values the
    /// drain then marks, and the second finds the dead half still
    /// unmarked and the work list empty: the fixpoint ends within its
    /// scans, so it indexes, records and looks up nothing, and mourns the
    /// dead half.
    #[test]
    fn a_weak_maps_dead_keys_are_mourned_without_an_index_in_a_collection() {
        const COUNT: usize = 1_000;
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let array_class = named_class(&mut heap, "Array");
        let mut elements = Vec::with_capacity(3 * COUNT);
        let mut ephemerons = Vec::with_capacity(2 * COUNT);
        for live in [true, false] {
            for _ in 0..COUNT {
                let key = heap.allocate_instance(class, 0);
                let value = heap.allocate_instance(class, 0);
                ephemerons.push(Value::object(keyed_ephemeron(
                    &mut heap,
                    ephemeron_class,
                    key,
                    value,
                )));
                if live {
                    elements.push(Value::object(key));
                }
            }
        }
        // The work list is a stack: the ephemerons, last in the array, are
        // visited before any key.
        elements.extend(ephemerons);
        let mut array_root = Value::object(heap.allocate_array(array_class, &elements));
        pending::work::take();

        heap.collect_for_test(&mut [&mut array_root]);

        let work = pending::work::take();
        assert_eq!(
            work.examined,
            3 * COUNT,
            "the first check examines every ephemeron, the second the dead half"
        );
        assert_eq!(
            (work.recorded, work.lookups, work.resolutions),
            (0, 0, 0),
            "nothing is indexed, so nothing is recorded or looked up"
        );
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        for (position, ephemeron) in elements[COUNT..].iter().enumerate() {
            let fields = heap
                .ephemeron_fields(rooted(*ephemeron))
                .expect("an ephemeron answers its fields");
            if position < COUNT {
                assert!(
                    fields.iter().all(|field| field.as_object().is_some()),
                    "a live entry keeps its key and its value"
                );
            } else {
                assert!(
                    fields.iter().all(|field| field.is_nil()),
                    "a dead entry is mourned"
                );
            }
        }
    }

    /// An ephemeron of `class` keyed by `key`, its value `value`.
    fn keyed_ephemeron<'gc>(
        heap: &mut Heap<'gc>,
        class: ClassReference<'gc>,
        key: ObjectReference<'gc>,
        value: ObjectReference<'gc>,
    ) -> ObjectReference<'gc> {
        let ephemeron = heap.allocate_ephemeron(class, 2);
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        ephemeron
    }

    /// The index empties and fills again: the first round resolves `E_a`
    /// with nothing pending, so its drain records nothing and lists `E_b`
    /// and `E_c`; the second indexes `E_b` and traces `E_c`, and the drain
    /// after that check must record, or it marks `K_b` unseen and `E_b` is
    /// mourned.
    ///
    /// Run indexed from the first check on, as above, and with the
    /// shipped scans, after which the switch indexes `E_b` before the
    /// drain that marks `K_b` records.
    #[test]
    fn an_ephemeron_indexed_after_the_index_emptied_is_kept_in_a_collection() {
        for scanned_checks in [0, pending::SCANNED_CHECKS] {
            let mut heap = Heap::new();
            let class = named_class(&mut heap, "Object");
            let ephemeron_class = named_class(&mut heap, "Ephemeron");
            let array_class = named_class(&mut heap, "Array");
            let [key_a, key_b, key_c, value_b] = [0; 4].map(|_| heap.allocate_instance(class, 0));
            let [holder_a, holder_b, holder_c] = [key_a, key_b, key_c].map(|key| {
                let holder = heap.allocate_instance(class, 1);
                assert!(heap.instance_set_field(holder, 0, Value::object(key)));
                holder
            });
            let ephemeron_b = keyed_ephemeron(&mut heap, ephemeron_class, key_b, value_b);
            let ephemeron_c = keyed_ephemeron(&mut heap, ephemeron_class, key_c, holder_b);
            // The work list is a stack: `E_b`, then `E_c`, then `H_c`.
            let value_a = heap.allocate_array(
                array_class,
                &[
                    Value::object(holder_c),
                    Value::object(ephemeron_c),
                    Value::object(ephemeron_b),
                ],
            );
            let ephemeron_a = keyed_ephemeron(&mut heap, ephemeron_class, key_a, value_a);
            let mut array_root = Value::object(heap.allocate_array(
                array_class,
                &[Value::object(holder_a), Value::object(ephemeron_a)],
            ));
            pending::work::set_scanned_checks(scanned_checks);
            pending::work::take();

            heap.collect_for_test(&mut [&mut array_root]);

            let resolutions = pending::work::take().resolutions;
            assert_eq!(resolutions, 1, "E_b through K_b's lookup");
            let elements = heap
                .array_elements(rooted(array_root))
                .expect("the array answers its elements");
            let value_a = heap
                .ephemeron_field(rooted(elements[1]), 1)
                .expect("an ephemeron has a value");
            let ephemeron_b = heap
                .array_elements(rooted(value_a))
                .expect("the value is an array")[2];
            let fields = heap
                .ephemeron_fields(rooted(ephemeron_b))
                .expect("an ephemeron answers its fields");
            assert!(
                fields.iter().all(|field| field.as_object().is_some()),
                "E_b keeps K_b and v_b"
            );
        }
    }

    /// `LENGTH` ephemerons, each one's value the next one's key, collected
    /// once with the first key held by nothing -- the whole chain is
    /// mourned -- and once with it rooted -- every value survives, the
    /// fixpoint reaching link after link. Answers the fixpoint's work in
    /// each collection.
    fn collect_a_chain_of_ephemerons(length: usize) -> [pending::work::Work; 2] {
        [false, true].map(|root_the_first_key| {
            let mut heap = Heap::new();
            let class = named_class(&mut heap, "Object");
            let ephemeron_class = named_class(&mut heap, "Ephemeron");
            let array_class = named_class(&mut heap, "Array");
            let keys: Vec<ObjectReference<'_>> = (0..=length)
                .map(|_| heap.allocate_instance(class, 0))
                .collect();
            let mut ephemerons = Vec::with_capacity(length);
            for position in 0..length {
                let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
                assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(keys[position])));
                assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(keys[position + 1])));
                ephemerons.push(Value::object(ephemeron));
            }
            let mut array_root = Value::object(heap.allocate_array(array_class, &ephemerons));
            let mut first_key_root = if root_the_first_key {
                Value::object(keys[0])
            } else {
                Value::nil()
            };
            pending::work::take();
            heap.collect_for_test(&mut [&mut array_root, &mut first_key_root]);
            let work = pending::work::take();
            let ephemerons = heap
                .array_elements(rooted(array_root))
                .expect("the array answers its elements");
            assert_eq!(ephemerons.len(), length);
            for (position, ephemeron) in ephemerons.into_iter().enumerate() {
                let fields = heap
                    .ephemeron_fields(rooted(ephemeron))
                    .expect("an ephemeron answers its fields");
                if root_the_first_key {
                    assert!(
                        fields[0].as_object().is_some() && fields[1].as_object().is_some(),
                        "ephemeron {position} keeps its key and its value"
                    );
                } else {
                    assert!(
                        fields.iter().all(|field| field.is_nil()),
                        "ephemeron {position} is mourned"
                    );
                }
            }
            work
        })
    }

    /// Two ephemerons share a key that only a third ephemeron's value
    /// reaches, its own key marked after it was visited: once the third is
    /// traced, the shared key's one lookup resolves both.
    ///
    /// Indexed from the first check on, so that the lookup is what
    /// resolves them.
    #[test]
    fn two_ephemerons_sharing_a_key_reached_late_are_both_kept_in_a_collection() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let array_class = named_class(&mut heap, "Array");
        let [first_key, shared_key, first_value, second_value] =
            [0; 4].map(|_| heap.allocate_instance(class, 0));
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(first_key)));
        let first = heap.allocate_ephemeron(ephemeron_class, 2);
        assert!(heap.ephemeron_set_field(first, 0, Value::object(first_key)));
        assert!(heap.ephemeron_set_field(first, 1, Value::object(shared_key)));
        let sharing = [first_value, second_value].map(|value| {
            let ephemeron = heap.allocate_ephemeron(ephemeron_class, 2);
            assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(shared_key)));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
            Value::object(ephemeron)
        });
        // The work list is a stack: the ephemerons are visited last to
        // first, all before the holder marks the first key.
        let mut array_root = Value::object(heap.allocate_array(
            array_class,
            &[
                Value::object(holder),
                Value::object(first),
                sharing[0],
                sharing[1],
            ],
        ));
        pending::work::set_scanned_checks(0);
        pending::work::take();

        heap.collect_for_test(&mut [&mut array_root]);

        let resolutions = pending::work::take().resolutions;
        assert_eq!(resolutions, 2, "both through the shared key's lookup");
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        for ephemeron in &elements[2..] {
            let fields = heap
                .ephemeron_fields(rooted(*ephemeron))
                .expect("an ephemeron answers its fields");
            assert!(
                fields.iter().all(|field| field.as_object().is_some()),
                "the ephemeron keeps its key and its value"
            );
        }
    }

    /// An ephemeron keyed by a resolved future whose value only another
    /// ephemeron's value reaches, that one's key marked after it was
    /// visited: the key reads the value, which the fixpoint marks late,
    /// and the entry is kept.
    ///
    /// Indexed from the first check on, so that the lookup is what
    /// resolves them.
    #[test]
    fn an_ephemeron_keyed_by_a_resolved_future_reached_late_is_kept_in_a_collection() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let future_class = named_class(&mut heap, "FutureImplementation");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let array_class = named_class(&mut heap, "Array");
        let value = heap.allocate_instance(class, 0);
        let guarded = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(guarded, 0, Value::small_integer(7)));
        let future = resolved_future(&mut heap, future_class, Value::object(value));
        let late_key = heap.allocate_instance(class, 0);
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(late_key)));
        let reaching = heap.allocate_ephemeron(ephemeron_class, 2);
        assert!(heap.ephemeron_set_field(reaching, 0, Value::object(late_key)));
        assert!(heap.ephemeron_set_field(reaching, 1, Value::object(value)));
        let keyed = heap.allocate_ephemeron(ephemeron_class, 2);
        assert!(heap.ephemeron_set_field(keyed, 0, Value::object(future)));
        assert!(heap.ephemeron_set_field(keyed, 1, Value::object(guarded)));
        let mut array_root = Value::object(heap.allocate_array(
            array_class,
            &[
                Value::object(holder),
                Value::object(reaching),
                Value::object(keyed),
            ],
        ));
        heap.replace_resolved_futures(ResolvedFutures::new(
            future_class,
            FUTURE_COMPUTATION,
            FUTURE_STATE,
            FUTURE_RESOLVED,
        ));
        pending::work::set_scanned_checks(0);
        pending::work::take();

        heap.collect_for_test(&mut [&mut array_root]);

        let resolutions = pending::work::take().resolutions;
        assert_eq!(resolutions, 1, "through the value's lookup");
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let reaching_fields = heap
            .ephemeron_fields(rooted(elements[1]))
            .expect("an ephemeron answers its fields");
        let keyed_fields = heap
            .ephemeron_fields(rooted(elements[2]))
            .expect("an ephemeron answers its fields");
        assert!(
            names(keyed_fields[0], rooted(reaching_fields[1])),
            "the key reads the value"
        );
        assert_eq!(
            heap.instance_field(rooted(keyed_fields[1]), 0)
                .and_then(Value::as_small_integer),
            Some(7),
            "the entry is kept"
        );
    }

    /// The positions of a future's two fields and its resolved state, as
    /// `interpreter::future` lays a `FutureImplementation` out.
    const FUTURE_COMPUTATION: usize = 0;
    const FUTURE_STATE: usize = 1;
    const FUTURE_RESOLVED: i64 = 2;

    /// A future of `future_class` resolved to `value`.
    fn resolved_future<'gc>(
        heap: &mut Heap<'gc>,
        future_class: ClassReference<'gc>,
        value: Value<'gc>,
    ) -> ObjectReference<'gc> {
        let future = heap.allocate_instance(future_class, 2);
        assert!(heap.instance_set_field(future, FUTURE_COMPUTATION, value));
        assert!(heap.instance_set_field(
            future,
            FUTURE_STATE,
            Value::small_integer(FUTURE_RESOLVED)
        ));
        future
    }

    /// A resolved future that only a weak word or an ephemeron's key
    /// names stands for its value, which something else holds: the
    /// collection keeps the entry for the value's sake, and the word reads
    /// the value afterwards. The proxy is collected.
    #[test]
    fn a_resolved_future_held_weakly_is_judged_by_its_value_in_a_collection() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let future_class = named_class(&mut heap, "FutureImplementation");
        let weak_class = named_class(&mut heap, "WeakArray");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let value = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(value, 0, Value::small_integer(42)));
        let guarded = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(guarded, 0, Value::small_integer(7)));
        let future = resolved_future(&mut heap, future_class, Value::object(value));
        let weak = heap.allocate_weak_array(weak_class, 1);
        assert!(heap.weak_array_set_element(weak, 0, Value::object(future)));
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(future)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(guarded)));
        let objects_before = heap.object_count;
        let mut weak_root = Value::object(weak);
        let mut ephemeron_root = Value::object(ephemeron);
        let mut value_root = Value::object(value);

        // As `World::collect` does before it marks.
        heap.replace_resolved_futures(ResolvedFutures::new(
            future_class,
            FUTURE_COMPUTATION,
            FUTURE_STATE,
            FUTURE_RESOLVED,
        ));
        heap.collect_for_test(&mut [&mut weak_root, &mut ephemeron_root, &mut value_root]);

        let value = rooted(value_root);
        let element = heap
            .weak_array_element(rooted(weak_root), 0)
            .expect("a weak array answers its slot");
        assert!(names(element, value), "the weak word reads the value");
        let fields = heap
            .ephemeron_fields(rooted(ephemeron_root))
            .expect("an ephemeron answers its fields");
        assert!(names(fields[0], value), "the key reads the value");
        assert_eq!(
            heap.instance_field(rooted(fields[1]), 0)
                .and_then(Value::as_small_integer),
            Some(7),
            "the entry is kept"
        );
        assert_eq!(
            heap.object_count,
            objects_before - 2,
            "the proxy is collected, and its class, which only it named"
        );
    }

    /// A move rewrites a weak word and an ephemeron's guarded fields like
    /// any other: forwarding reads every word, marking only the strong.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_forwards_every_weak_word() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Object");
        let weak_class = named_class(&mut heap, "WeakArray");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let element = heap.allocate_instance(class, 0);
        let key = heap.allocate_instance(class, 0);
        let value = heap.allocate_instance(class, 0);
        let weak = heap.allocate_weak_array(weak_class, 1);
        assert!(heap.weak_array_set_element(weak, 0, Value::object(element)));
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        let roots =
            [weak, ephemeron, element, key, value].map(|object| heap.stash(Value::object(object)));
        move_bare_heap(&mut heap);
        let [weak, ephemeron, element, key, value] =
            roots.each_ref().map(|root| rooted(heap.fetch(root)));
        assert!(names(
            heap.weak_array_element(weak, 0).unwrap_or(Value::nil()),
            element
        ));
        assert!(names(
            heap.ephemeron_field(ephemeron, 0).unwrap_or(Value::nil()),
            key
        ));
        assert!(names(
            heap.ephemeron_field(ephemeron, 1).unwrap_or(Value::nil()),
            value
        ));
    }

    #[test]
    fn a_large_integer_lends_its_digits_and_its_sign_in_place() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "LargeInteger");
        let integer = crate::ArbitraryInteger::from_sign_and_digits(true, &[1, 2]);
        let object = heap.allocate_large_integer(class, integer.clone());
        assert_eq!(
            heap.with_large_integer(object, |negative, digits| (negative, digits.to_vec())),
            Some((true, vec![1, 2]))
        );
        assert_eq!(heap.large_integer(object), Some(integer));
    }

    #[test]
    fn a_block_is_its_header_its_class_word_and_its_fields() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Point");
        let object = heap.allocate_instance(class, 2);
        let block = heap.block(object);
        assert_eq!(block.len(), 4);
        assert_eq!(Header(block[0]).format(), Format::FixedPointers);
        assert_eq!(Header(block[0]).word_count(), 3);
        assert!(heap.object_class(object).same(class));
    }

    /// A `LargeInteger`'s digits are no stored words, and only its format
    /// says so: a digit shaped like an object word -- the low three bits
    /// clear, the offset of a live object -- must come through a collection
    /// and a move as it went in, not forwarded as though it named one.
    #[test]
    fn a_digit_shaped_like_an_object_word_is_never_forwarded() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "LargeInteger");
        heap.allocate_string(class, crate::CodePoints::from("dead"));
        let filler = heap.allocate_string(class, crate::CodePoints::from("live"));
        let shaped = Word::with_object_offset(filler.offset).bits();
        assert_eq!(shaped & 0b111, 0, "the digit reads as an object word");
        let integer = crate::ArbitraryInteger::from_sign_and_digits(false, &[shaped, shaped]);
        let object = heap.allocate_large_integer(class, integer.clone());
        let filler_root = heap.stash(Value::object(filler));
        let root = heap.stash(Value::object(object));
        let offset_of = |heap: &Heap<'_>, root: &Root| match heap.fetch(root).as_object() {
            Some(object) => heap.object_identity_key(object),
            None => panic!("the root no longer holds an object"),
        };
        let shaped_offset = filler.offset.get() as usize;
        let digits = |heap: &Heap<'_>| {
            let object = heap.fetch(&root).as_object()?;
            heap.with_large_integer(object, |_, digits| digits.to_vec())
        };

        // The dead string before them goes, so the object the digit names
        // slides down: forwarding the digit would rewrite it.
        heap.collect_for_test(&mut []);
        assert_ne!(offset_of(&heap, &filler_root), shaped_offset);
        assert_eq!(digits(&heap), Some(vec![shaped, shaped]));

        #[cfg(debug_assertions)]
        {
            let before = offset_of(&heap, &filler_root);
            assert!(
                shaped_offset < heap.space.region(RegionName::Old).top,
                "the digit still names an offset inside the space"
            );
            move_bare_heap(&mut heap);
            assert_ne!(offset_of(&heap, &filler_root), before);
            assert_eq!(digits(&heap), Some(vec![shaped, shaped]));
        }
        let Some(object) = heap.fetch(&root).as_object() else {
            panic!("the root no longer holds an object");
        };
        assert_eq!(heap.large_integer(object), Some(integer));
    }

    #[test]
    fn an_instance_field_past_its_word_count_answers_none() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Point");
        let object = heap.allocate_instance(class, 2);
        assert!(heap.instance_field(object, 1).is_some());
        assert!(heap.instance_field(object, 2).is_none());
        assert!(!heap.instance_set_field(object, 2, Value::nil()));
    }

    /// An ephemeron's fields are no instance slots: the slot accessors take
    /// `FixedPointers` alone, so that an ordinary object's slot pays
    /// nothing for the ephemeron's format, and refuse an ephemeron, whose
    /// fields only the ephemeron accessors reach -- and those refuse an
    /// ordinary instance in turn.
    #[test]
    fn the_slot_accessors_refuse_an_ephemeron() {
        let mut heap = Heap::new();
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 3);
        assert!(!heap.instance_set_field(ephemeron, 0, Value::small_integer(1)));
        assert!(heap.instance_field(ephemeron, 0).is_none());
        assert!(heap.instance_field_count(ephemeron).is_none());
        assert!(heap.instance_fields(ephemeron).is_none());
        assert!(heap.ephemeron_set_field(ephemeron, 2, Value::small_integer(1)));
        assert_eq!(
            heap.ephemeron_field(ephemeron, 2)
                .and_then(|field| field.as_small_integer()),
            Some(1)
        );
        assert!(!heap.ephemeron_set_field(ephemeron, 3, Value::nil()));
        let instance_class = named_class(&mut heap, "Point");
        let instance = heap.allocate_instance(instance_class, 3);
        assert!(!heap.ephemeron_set_field(instance, 0, Value::nil()));
        assert!(heap.ephemeron_field(instance, 0).is_none());
        assert!(heap.ephemeron_fields(instance).is_none());
    }

    /// `describe`'s instance arm reads the mixin's name through a re-bound
    /// `class` intermediate rather than a single chained borrow. Nothing
    /// else in the workspace asserts its output, so pin it directly.
    #[test]
    fn describe_names_an_instance_by_its_class_mixin_name() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let instance = heap.allocate_instance(class, 0);
        assert_eq!(heap.describe(Value::object(instance)), "an instance of Foo");
    }

    /// The fallback arm of that same match: `class` names an `ObjectReference`
    /// whose own format is not `Class`, the shape a class reference
    /// decoded from an untrusted image can be in before verification runs.
    /// `describe` must still answer something rather than panic.
    #[test]
    fn describe_falls_back_to_an_object_for_a_class_reference_that_is_not_a_class() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let not_a_class = heap.allocate_string(class, crate::CodePoints::from("not a class"));
        let instance = heap.allocate_instance(not_a_class, 0);
        assert_eq!(heap.describe(Value::object(instance)), "an object");
    }

    /// The header every activation-stack test below pushes: nothing in it is
    /// read back except where a test sets it on purpose. A top-level frame
    /// names no code, so a frame reified from it takes no arguments.
    fn header<'gc>() -> ActivationHeader<'gc> {
        header_taking(0)
    }

    /// `header`'s, for a push handed `argument_count` arguments: every
    /// push hands a code its own number of them.
    fn header_taking<'gc>(argument_count: u16) -> ActivationHeader<'gc> {
        ActivationHeader {
            receiver: Value::nil(),
            current_class: None,
            kind: ActivationKind::TopLevel,
            method: None,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code_taking(argument_count),
            then: Continuation::Deliver,
            counted: true,
        }
    }

    /// A body that returns its first register: no test here runs it. Every
    /// register it has is a named local, so that a push of it fills every
    /// temporary a test asks for with nil, and no collection takes one for
    /// scratch; a test of the scratch builds its own body.
    fn trivial_code() -> std::rc::Rc<crate::bytecode::CompiledCode> {
        trivial_code_taking(0)
    }

    /// `trivial_code`, taking `argument_count` arguments.
    fn trivial_code_taking(argument_count: u16) -> std::rc::Rc<crate::bytecode::CompiledCode> {
        let mut code = crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Method,
            vec![crate::bytecode::Instruction::Return { register: 0 }],
            argument_count,
            u16::MAX,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        );
        code.local_count = u16::MAX;
        std::rc::Rc::new(code)
    }

    #[test]
    fn an_activation_addresses_its_arguments_before_its_temporaries() {
        let mut heap = Heap::new();
        let activation = heap
            .push_activation(
                header_taking(2),
                Arguments::Values(&[Value::small_integer(1), Value::small_integer(2)].as_slice()),
                2,
            )
            .expect("the stack holds the frame");
        assert_eq!(
            heap.activation_local(activation, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(activation, 1).as_small_integer(),
            Some(2)
        );
        // The temporaries follow the header's numbers.
        let first = first_above_header(2);
        assert!(heap.activation_local(activation, first).is_nil());
        heap.activation_set_local(activation, first + 1, Value::small_integer(9));
        assert_eq!(
            heap.activation_local(activation, first + 1)
                .as_small_integer(),
            Some(9)
        );
        heap.pop_activation(activation);
    }

    /// Two live activations share one reservation of words; neither may see the
    /// other's registers, whichever is written first.
    #[test]
    fn nested_activations_keep_separate_register_windows() {
        let mut heap = Heap::new();
        let outer = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let inner = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(2)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let temporary = first_above_header(1);
        heap.activation_set_local(inner, temporary, Value::small_integer(20));
        heap.activation_set_local(outer, temporary, Value::small_integer(10));
        assert_eq!(heap.activation_local(outer, 0).as_small_integer(), Some(1));
        assert_eq!(
            heap.activation_local(outer, temporary).as_small_integer(),
            Some(10)
        );
        assert_eq!(heap.activation_local(inner, 0).as_small_integer(), Some(2));
        assert_eq!(
            heap.activation_local(inner, temporary).as_small_integer(),
            Some(20)
        );
        assert_eq!(heap.activation_depth(), 2);
        heap.pop_activation(inner);
        heap.pop_activation(outer);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// A send's arguments are read where they are: a push from
    /// `Arguments::Slots` copies exactly the slots it names out of the
    /// frame below into the new frame's first registers, the neighbouring
    /// registers left behind, and a push from `Arguments::Values` copies
    /// the slice, for each of the counts the push copies differently.
    #[test]
    fn a_push_takes_its_arguments_from_the_slots_below_or_from_values() {
        let mut heap = Heap::new();
        let sender = heap
            .push_activation(header(), Arguments::NONE, 5)
            .expect("the stack holds the frame");
        let first = first_above_header(0);
        for register in first..first + 5 {
            heap.activation_set_local(
                sender,
                register,
                Value::small_integer(10 + i64::from(register - first)),
            );
        }
        let register_base = heap.activation_register_base(sender);
        // One, two and three: the push copies one or two word by word and
        // more by a loop.
        for count in 1..=3_u16 {
            let from_slots = heap
                .push_activation(
                    header_taking(count),
                    Arguments::Slots(ArgumentSlots::new(register_base, first + 2, count)),
                    1,
                )
                .expect("the stack holds the frame");
            for register in 0..count {
                assert_eq!(
                    heap.activation_local(from_slots, register)
                        .as_small_integer(),
                    Some(12 + i64::from(register))
                );
            }
            assert!(heap
                .activation_local(from_slots, first_above_header(count))
                .is_nil());
            heap.pop_activation(from_slots);
        }
        // The same three counts from values: one is copied on its own,
        // more by a loop.
        let values = [7, 8, 9].map(Value::small_integer);
        for count in 1..=3_u16 {
            let from_values = heap
                .push_activation(
                    header_taking(count),
                    Arguments::Values(&&values[..usize::from(count)]),
                    1,
                )
                .expect("the stack holds the frame");
            for register in 0..count {
                assert_eq!(
                    heap.activation_local(from_values, register)
                        .as_small_integer(),
                    Some(7 + i64::from(register))
                );
            }
            assert!(heap
                .activation_local(from_values, first_above_header(count))
                .is_nil());
            heap.pop_activation(from_values);
        }
        for register in first..first + 5 {
            assert_eq!(
                heap.activation_local(sender, register).as_small_integer(),
                Some(10 + i64::from(register - first)),
                "the sender's registers are untouched"
            );
        }
        heap.pop_activation(sender);
    }

    /// The first register above the header of a frame handed
    /// `argument_count` arguments: the header's numbers lie between.
    fn first_above_header(argument_count: u16) -> u16 {
        argument_count + frame_header::COUNT as u16
    }

    /// A send's own callee lies on its sender's slots: its registers 0 and 1
    /// are the slots' very words, copied nowhere, its header follows them,
    /// and its pop gives the sender back its whole window, the slots still
    /// holding what the sender put there. A push from values made while it
    /// runs lands at the top, above every window.
    #[test]
    fn an_overlapped_push_lays_the_callee_on_its_senders_slots() {
        let mut heap = Heap::new();
        let sender = heap
            .push_activation(header(), Arguments::NONE, 6)
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(sender);
        let below_slots = first_above_header(0);
        let argument_base = below_slots + 2;
        heap.set_register(base, below_slots, Value::small_integer(1));
        heap.set_register(base, argument_base, Value::small_integer(7));
        heap.set_register(base, argument_base + 1, Value::small_integer(8));
        // Dead at the send: the callee's header goes over it.
        heap.set_register(base, argument_base + 2, Value::small_integer(99));
        let sender_end = heap.stack.window_end(sender.offset);
        assert_eq!(heap.stack.frames.top(), sender_end);

        let callee = heap
            .push_overlapped_activation(&mut header_taking(2), base, argument_base, 2, 1)
            .expect("the stack holds the frame");
        let callee_base = heap.activation_register_base(callee);
        assert_eq!(
            callee_base.index,
            base.index + u32::from(argument_base),
            "the callee's register 0 is the sender's first slot"
        );
        assert_eq!(
            callee.offset,
            callee_base.index + 2,
            "the callee's header lies where the slots end"
        );
        assert_eq!(heap.register(callee_base, 0).as_small_integer(), Some(7));
        assert_eq!(heap.register(callee_base, 1).as_small_integer(), Some(8));
        assert!(heap.register(callee_base, first_above_header(2)).is_nil());
        assert_eq!(
            heap.stack.frames.top(),
            callee.offset as usize + frame_header::COUNT + 1
        );
        assert!(heap
            .activation_sender(callee)
            .is_some_and(|below| below.same(sender)));
        assert!(heap
            .activation_above(sender)
            .is_some_and(|above| above.same(callee)));

        let top = heap.stack.frames.top();
        let from_values = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(5)].as_slice()),
                0,
            )
            .expect("the stack holds the frame");
        assert_eq!(
            heap.activation_position(from_values),
            top,
            "a push from values starts at the top"
        );
        assert_eq!(
            heap.register(heap.activation_register_base(from_values), 0)
                .as_small_integer(),
            Some(5)
        );
        heap.pop_activation(from_values);
        assert_eq!(heap.stack.frames.top(), top);

        let above_slots = argument_base + 2..first_above_header(0) + 6;
        let before: Vec<u64> = above_slots
            .clone()
            .map(|register| {
                heap.stack
                    .frames
                    .word(base.index as usize + usize::from(register))
            })
            .collect();
        heap.pop_activation(callee);
        assert_eq!(
            heap.stack.frames.top(),
            sender_end,
            "the pop restores the top to the sender's window end"
        );
        assert_eq!(heap.register(base, below_slots).as_small_integer(), Some(1));
        assert_eq!(
            heap.register(base, argument_base).as_small_integer(),
            Some(7)
        );
        assert_eq!(
            heap.register(base, argument_base + 1).as_small_integer(),
            Some(8)
        );
        // A release build's pop writes none of the dead registers from the
        // slots' end up, which hold the callee's header words; a debug
        // build's writes the dead register word into each, which an
        // instruction's read refuses.
        for (register, before) in above_slots.zip(before) {
            let word = heap
                .stack
                .frames
                .word(base.index as usize + usize::from(register));
            if cfg!(debug_assertions) {
                assert_eq!(word, dead_register().bits(), "r{register} is dead");
            } else {
                assert_eq!(word, before, "r{register} above the slots is untouched");
            }
        }
        heap.pop_activation(sender);
        assert_eq!(heap.stack.frames.top(), 0);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// A body of `register_count` registers taking `argument_count`
    /// arguments, whose first `named` registers above the header are
    /// named temporaries and the rest scratch: no test here runs it.
    fn scratch_code(
        argument_count: u16,
        named: u16,
        register_count: u16,
    ) -> Rc<crate::bytecode::CompiledCode> {
        let mut code = crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Method,
            vec![crate::bytecode::Instruction::Return { register: 0 }],
            argument_count,
            register_count,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        );
        code.local_count = first_above_header(argument_count) + named;
        Rc::new(code)
    }

    /// A callee whose window ends below its sender's leaves the top at the
    /// sender's window end, since a push never lowers it, so the sender's
    /// dead registers above the callee stay below the top: a collection
    /// meanwhile nils them rather than leaving them untraced, and the pop
    /// writes none of them in a release build -- the top goes back to the
    /// sender's window end and nothing else changes.
    #[test]
    fn a_pop_leaves_the_senders_dead_registers_above_a_shorter_callee() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Dead");
        let argument_base = first_above_header(0);
        // A sender standing at the send whose one slot, its named r13, the
        // callee lies on.
        let mut code = crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Method,
            vec![
                crate::bytecode::Instruction::SendPublic {
                    selector: Symbol::intern("m"),
                    receiver_register: argument_base,
                    argument_base,
                    argument_sources: vec![argument_base],
                    destination: argument_base,
                    call_site: crate::bytecode::CallSite::UNCACHED,
                },
                crate::bytecode::Instruction::Return {
                    register: argument_base,
                },
            ],
            0,
            argument_base + 40,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        );
        code.local_count = argument_base + 1;
        let sender = heap
            .push_activation(
                ActivationHeader {
                    code: Rc::new(code),
                    ..header()
                },
                Arguments::NONE,
                40,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(sender);
        let sender_end = heap.stack.window_end(sender.offset);
        heap.set_register(base, argument_base, Value::small_integer(7));
        let dead = first_above_header(0) + 30;
        let unreachable = heap.allocate_instance(class, 0);
        heap.set_register(base, dead, Value::object(unreachable));
        let callee = heap
            .push_overlapped_activation(&mut header_taking(1), base, argument_base, 1, 0)
            .expect("the stack holds the frame");
        let dead_word = base.index as usize + usize::from(dead);
        assert!(heap.stack.window_end(callee.offset) <= dead_word);
        assert_eq!(
            heap.stack.frames.top(),
            sender_end,
            "the push leaves the top at the sender's window end"
        );
        // The pre-pass nils the dead register, which the trace then finds
        // naming nothing.
        heap.collect_for_test(&mut []);
        assert_eq!(heap.stack.frames.word(dead_word), dead_register().bits());
        assert_eq!(
            heap.register(heap.activation_register_base(callee), 0)
                .as_small_integer(),
            Some(7)
        );
        let left = Value::small_integer(55).word().bits();
        heap.stack.frames.set_word(dead_word, left);
        heap.pop_activation(callee);
        assert_eq!(heap.stack.frames.top(), sender_end);
        if cfg!(debug_assertions) {
            assert_eq!(
                heap.stack.frames.word(dead_word),
                dead_register().bits(),
                "a debug build's pop writes the dead register word"
            );
        } else {
            assert_eq!(
                heap.stack.frames.word(dead_word),
                left,
                "a release build's pop writes nothing"
            );
        }
        assert_eq!(
            heap.register(base, argument_base).as_small_integer(),
            Some(7)
        );
        heap.collect_for_test(&mut []);
        heap.pop_activation(sender);
    }

    /// A push fills its named temporaries with nil and leaves its scratch
    /// as the stack last held it -- here a popped frame's numbers -- in a
    /// release build, and writes the dead register word there in a debug
    /// one; both for a push above every window and for a callee laid on
    /// its sender's slots.
    #[test]
    fn a_push_writes_its_named_temporaries_and_leaves_its_scratch() {
        let mut heap = frame_heap();
        let expect_left = |heap: &Heap<'_>, word: usize, left: i64| {
            let expected = if cfg!(debug_assertions) {
                dead_register().bits()
            } else {
                Value::small_integer(left).word().bits()
            };
            assert_eq!(heap.stack.frames.word(word), expected, "word {word}");
        };
        let first = first_above_header(0);
        // A frame that writes 1 to 4 into its four temporaries, then goes.
        let earlier = heap
            .push_activation(header(), Arguments::NONE, 4)
            .expect("the stack holds the frame");
        for register in first..first + 4 {
            heap.activation_set_local(
                earlier,
                register,
                Value::small_integer(i64::from(register - first) + 1),
            );
        }
        let earlier_base = heap.activation_register_base(earlier).index as usize;
        heap.pop_activation(earlier);

        // One named temporary and three scratch registers, where the
        // earlier frame's four were.
        let pushed = heap
            .push_activation(
                ActivationHeader {
                    code: scratch_code(0, 1, first + 4),
                    ..header()
                },
                Arguments::NONE,
                4,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(pushed);
        assert_eq!(base.index as usize, earlier_base);
        assert!(heap.register(base, first).is_nil(), "the named temporary");
        for register in first + 1..first + 4 {
            expect_left(
                &heap,
                base.index as usize + usize::from(register),
                i64::from(register - first) + 1,
            );
        }

        // A callee on the slot r14 of a sender of twenty registers above
        // its header, each holding a number: its header goes over r15 to
        // r27, and its one named temporary and two scratch registers over
        // r28 to r30, which keep the sender's numbers.
        let sender = heap
            .push_activation(
                ActivationHeader {
                    code: scratch_code(0, 0, first + 20),
                    ..header()
                },
                Arguments::NONE,
                20,
            )
            .expect("the stack holds the frame");
        let sender_base = heap.activation_register_base(sender);
        for register in first..first + 20 {
            heap.set_register(
                sender_base,
                register,
                Value::small_integer(100 + i64::from(register)),
            );
        }
        let callee = heap
            .push_overlapped_activation(
                &mut ActivationHeader {
                    code: scratch_code(1, 1, first_above_header(1) + 3),
                    ..header()
                },
                sender_base,
                first + 1,
                1,
                3,
            )
            .expect("the stack holds the frame");
        let callee_base = heap.activation_register_base(callee);
        assert_eq!(
            heap.register(callee_base, 0).as_small_integer(),
            Some(100 + i64::from(first + 1))
        );
        let callee_first = first_above_header(1);
        assert!(
            heap.register(callee_base, callee_first).is_nil(),
            "the named temporary"
        );
        for register in callee_first + 1..callee_first + 3 {
            let word = callee_base.index as usize + usize::from(register);
            let sender_register = (word - sender_base.index as usize) as i64;
            expect_left(&heap, word, 100 + sender_register);
        }
        heap.pop_activation(callee);
        heap.pop_activation(sender);
        heap.pop_activation(pushed);
    }

    /// The dynamic net under the leftover analysis: a debug build refuses
    /// an instruction's read of a register nothing wrote since the push.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "internal: the frame at stack word")]
    fn a_debug_read_of_a_dead_register_panics() {
        let mut heap = frame_heap();
        let first = first_above_header(0);
        let activation = heap
            .push_activation(
                ActivationHeader {
                    code: scratch_code(0, 0, first + 1),
                    ..header()
                },
                Arguments::NONE,
                1,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(activation);
        let _ = heap.register(base, first);
    }

    /// A sender suspended while its callee lies on its slots keeps its
    /// registers below the callee's header only, the rest nil; the slots'
    /// values the callee shared come back intact in both once each is
    /// resumed, the callee above whatever is on the stack.
    #[test]
    fn suspending_a_sender_under_an_overlapped_callee_keeps_the_shared_slots() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Overlapped");
        let sender_registers = first_above_header(0) + 6;
        let sender = heap
            .push_activation(
                suspendable_header(class, 0, sender_registers),
                Arguments::NONE,
                6,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(sender);
        let below_slots = first_above_header(0);
        let argument_base = below_slots + 2;
        heap.set_register(base, below_slots, Value::small_integer(1));
        heap.set_register(base, argument_base, Value::small_integer(7));
        heap.set_register(base, argument_base + 1, Value::small_integer(8));
        let callee = heap
            .push_overlapped_activation(
                &mut suspendable_header(class, 2, first_above_header(2) + 1),
                base,
                argument_base,
                2,
                1,
            )
            .expect("the stack holds the frame");
        let callee_base = heap.activation_register_base(callee);
        heap.set_register(callee_base, first_above_header(2), Value::small_integer(42));
        let floor = heap.activation_position(sender);

        let callee_frame = heap.suspend_activations(floor).expect("both are suspended");
        assert_eq!(heap.activation_depth(), 0);
        assert_eq!(heap.stack.frames.top(), floor);
        let sender_frame = heap
            .frame_sender(callee_frame)
            .expect("the callee returns to the sender");
        let small = |value: i64| Value::small_integer(value).word().bits();
        let nil = Word::NIL.bits();
        // The sender takes no arguments, so its fields are its six
        // registers above the header, each `HEADER_REGISTERS` below its
        // number.
        let field = |register: u16| {
            Heap::frame_field_of_register(0, register).expect("a register above the header")
        };
        let mut expected = vec![nil; 6];
        expected[field(below_slots)] = small(1);
        expected[field(argument_base)] = small(7);
        expected[field(argument_base + 1)] = small(8);
        assert_eq!(
            own_registers(&heap, sender_frame),
            expected,
            "the sender keeps its registers below the callee's header, the rest nil"
        );
        assert_eq!(
            own_registers(&heap, callee_frame),
            [small(7), small(8), small(42)],
            "the callee holds its arguments, the slots' values, and its register above the header"
        );

        let resumed = heap.resume_activation(callee_frame, FeedbackBase::NOT_ASSIGNED);
        let resumed_base = heap.activation_register_base(resumed);
        assert_eq!(heap.register(resumed_base, 0).as_small_integer(), Some(7));
        assert_eq!(heap.register(resumed_base, 1).as_small_integer(), Some(8));
        assert_eq!(
            heap.register(resumed_base, first_above_header(2))
                .as_small_integer(),
            Some(42)
        );
        heap.pop_activation(resumed);
        let sender = heap.resume_activation(sender_frame, FeedbackBase::NOT_ASSIGNED);
        let base = heap.activation_register_base(sender);
        assert_eq!(heap.register(base, below_slots).as_small_integer(), Some(1));
        assert_eq!(
            heap.register(base, argument_base).as_small_integer(),
            Some(7)
        );
        assert_eq!(
            heap.register(base, argument_base + 1).as_small_integer(),
            Some(8)
        );
        heap.pop_activation(sender);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// The property the activation stack exists for: an activation nothing
    /// reified is gone once it is popped, rather than kept in the space as a
    /// frame object, which only a collection reclaims, and only once
    /// nothing reaches it.
    #[test]
    fn an_activation_that_is_never_reified_leaves_nothing_behind() {
        let mut heap = frame_heap();
        let objects = heap.heap_statistics().live_objects;
        for _ in 0..1000 {
            let activation = heap
                .push_activation(
                    header_taking(1),
                    Arguments::Values(&[Value::small_integer(1)].as_slice()),
                    3,
                )
                .expect("the stack holds the frame");
            heap.pop_activation(activation);
        }
        assert_eq!(heap.heap_statistics().live_objects, objects);
        assert_eq!(heap.stack.frames.top(), 0);
        assert_eq!(heap.activation_depth(), 0);
    }

    #[test]
    fn reifying_an_activation_twice_answers_the_same_frame() {
        let mut heap = frame_heap();
        let objects = heap.heap_statistics().live_objects;
        let activation = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let first = heap.reify(activation);
        let second = heap.reify(activation);
        assert!(first.same(second));
        assert_eq!(
            heap.heap_statistics().live_objects,
            objects + 1,
            "one frame object"
        );
        heap.pop_activation(activation);
    }

    /// While its activation runs, a reified frame is a view of the live
    /// registers, in both directions: a block writing its method's variable
    /// through the frame must be seen by the method, and vice versa.
    #[test]
    fn a_married_frame_reads_and_writes_the_live_registers() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Married");
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        heap.activation_set_local(activation, first_above_header(1), Value::small_integer(5));
        assert_eq!(
            heap.frame_local(frame, first_above_header(1))
                .as_small_integer(),
            Some(5)
        );
        heap.frame_set_local(frame, first_above_header(1), Value::small_integer(7));
        assert_eq!(
            heap.activation_local(activation, first_above_header(1))
                .as_small_integer(),
            Some(7)
        );
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(activation)));
        heap.pop_activation(activation);
    }

    /// The header of `activation` described as the exit's walk describes a
    /// frame machine code pushed for a body taking one argument, entered
    /// at the class its current class names, stopped at program counter 9.
    #[cfg(ns_jit)]
    fn machine_frame_of<'gc>(
        heap: &mut Heap<'gc>,
        activation: ActivationReference,
        reifies: bool,
        caller: Option<(u32, usize)>,
    ) -> MachineFrame {
        MachineFrame {
            offset: activation.offset,
            argument_count: 1,
            code: heap.activation_code_id(activation),
            kind: MachineKind::initializer(),
            program_counter: 9,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            register_count: heap.stack.register_count(activation.offset),
            addresses_own_slots: false,
            reifies,
            caller,
        }
    }

    /// A frame whose code reifies it keeps, when its header is written
    /// for an exit or a leaf, the reified word, the counted bit,
    /// `retains_registers` and the program counter its own code wrote,
    /// so that its divorce at its return finds them; a frame whose code
    /// reifies nothing, which wrote none of them, is written unreified,
    /// counted, retaining no registers, at the walk's program counter.
    #[cfg(ns_jit)]
    #[test]
    fn a_materialized_frame_keeps_its_reified_word() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Materialized");
        let activation = heap
            .push_activation(
                ActivationHeader {
                    counted: false,
                    ..suspendable_header(class, 1, first_above_header(1) + 1)
                },
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        heap.activation_set_program_counter(activation, 4);
        let frame = heap.reify(activation);
        heap.retain_registers(activation);
        let reified_word = |heap: &Heap<'_>| {
            heap.stack
                .header_word(activation.offset, frame_header::REIFIED)
                .object_offset()
        };
        let flags = |heap: &Heap<'_>| heap.stack.flags(activation.offset);
        let (kept_flags, kept_reified) = (
            heap.stack
                .header_word(activation.offset, frame_header::FLAGS),
            heap.stack
                .header_word(activation.offset, frame_header::REIFIED),
        );

        for caller in [None, Some((0, 0))] {
            let reifying = machine_frame_of(&mut heap, activation, true, caller);
            let written = heap.write_machine_frame(&reifying);
            assert!(written.same(activation));
            assert_eq!(reified_word(&heap), Some(frame.offset), "the reified word");
            assert!(flags(&heap).retains_registers);
            assert!(
                !flags(&heap).counted,
                "the counted bit the frame was pushed with"
            );
            assert!(flags(&heap).compiled);
            assert_eq!(
                flags(&heap).kind_tag,
                kind_tag::INITIALIZER_OWN_PART,
                "the kind the frame recorded"
            );
            assert_eq!(heap.activation_program_counter(activation), 4);
        }

        let plain = machine_frame_of(&mut heap, activation, false, None);
        let _ = heap.write_machine_frame(&plain);
        assert_eq!(reified_word(&heap), None);
        assert!(!flags(&heap).retains_registers);
        assert!(flags(&heap).counted);
        assert_eq!(heap.activation_program_counter(activation), 9);

        // As the push left them, for the pop, which divorces the frame.
        heap.stack
            .set_header_word(activation.offset, frame_header::FLAGS, kept_flags);
        heap.stack
            .set_header_word(activation.offset, frame_header::REIFIED, kept_reified);
        heap.pop_activation(activation);
        assert!(heap.frame_activation(frame).is_none());
    }

    /// A leaf's view of a frame (no caller) keeps the call shift its
    /// prologue recorded, which its machine code's return reads after the
    /// leaf; a frame the exit's walk materializes (a caller) returns
    /// through the interpreter, whose pop knows no shift, and drops it
    /// (`frames::CALL_SHIFT_BITS`).
    #[cfg(ns_jit)]
    #[test]
    fn a_leaf_s_view_keeps_the_call_shift_and_a_materialized_frame_drops_it() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Shifted");
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let (kept_flags, kept_caller) = (
            heap.stack
                .header_word(activation.offset, frame_header::FLAGS),
            heap.stack
                .header_word(activation.offset, frame_header::CALLER),
        );
        let shift_bits = Heap::machine_call_shift_mask();
        let call_shift = |heap: &Heap<'_>| {
            heap.stack
                .header_word(activation.offset, frame_header::FLAGS)
                .bits()
                & shift_bits
        };
        for (caller, kept) in [(None, true), (Some((0, 0)), false)] {
            // The prologue's flags word, its call shift the most two bits
            // hold.
            heap.stack.set_header_word(
                activation.offset,
                frame_header::FLAGS,
                Word::from_bits(kept_flags.bits() | shift_bits),
            );
            let frame = machine_frame_of(&mut heap, activation, false, caller);
            let _ = heap.write_machine_frame(&frame);
            assert_eq!(
                call_shift(&heap),
                if kept { shift_bits } else { 0 },
                "the call shift of a frame written with the caller {caller:?}"
            );
        }
        // As the push left them, for the pop.
        heap.stack
            .set_header_word(activation.offset, frame_header::FLAGS, kept_flags);
        heap.stack
            .set_header_word(activation.offset, frame_header::CALLER, kept_caller);
        heap.pop_activation(activation);
    }

    /// A frame of a block's code machine code pushed is written as a
    /// closure's activation of the closure its prologue recorded as its
    /// operand, whatever kind its flags word said before: the walk names
    /// every kind of frame.
    #[cfg(ns_jit)]
    #[test]
    fn materialize_writes_a_closure_frame() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let method_activation = heap
            .push_activation(header(), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        let home = heap.reify(method_activation);
        let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Block,
            vec![crate::bytecode::Instruction::ImplicitReturn { register: 13 }],
            0,
            14,
            0,
            vec![],
            ns_syntax::Span::new(0, 0),
        ));
        let closure = heap.allocate_closure(
            class,
            NewClosure {
                code: std::rc::Rc::clone(&block),
                captured_frame: home,
                copied: vec![],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        let activation = heap
            .push_activation(
                ActivationHeader {
                    receiver: Value::nil(),
                    current_class: None,
                    kind: ActivationKind::Closure(closure),
                    method: None,
                    feedback_base: FeedbackBase::NOT_ASSIGNED,
                    code: std::rc::Rc::clone(&block),
                    then: Continuation::Deliver,
                    counted: true,
                },
                Arguments::NONE,
                block.registers_above_header(),
            )
            .expect("the stack holds the frame");
        // A method's flags, as a prologue that records no kind leaves them:
        // only the operand says which closure the frame runs.
        let method_flags = FrameFlags {
            kind_tag: kind_tag::METHOD,
            ..heap.stack.flags(activation.offset)
        };
        heap.stack
            .set_header_word(activation.offset, frame_header::FLAGS, method_flags.word());
        let caller = heap
            .stack
            .header_word(activation.offset, frame_header::CALLER);

        let installed = crate::jit::Installed {
            code: std::rc::Rc::clone(&block),
            method: None,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            reifies: false,
        };
        let frame = crate::jit::materialize::machine_frame(
            &mut heap,
            activation.offset,
            &installed,
            0,
            None,
        );
        let written = heap.write_machine_frame(&frame);
        assert!(written.same(activation));
        match heap.activation_kind(activation) {
            ActivationKind::Closure(running) => {
                assert!(running.same(closure), "the closure the frame recorded");
            }
            _ => panic!("a closure's activation"),
        }
        assert!(heap.stack.flags(activation.offset).compiled);
        assert!(heap
            .activation_home(activation)
            .is_some_and(|found| found.same(home)));
        // A leaf's view leaves out the caller, which the pop reads.
        heap.stack
            .set_header_word(activation.offset, frame_header::CALLER, caller);
        heap.pop_activation(activation);
        heap.pop_activation(method_activation);
    }

    /// Popping a *marked* reified activation hands its registers to the
    /// frame, which a surviving block goes on reading and writing; the
    /// frame is dead from then on, and a dead activation has no sender.
    /// Unmarked is
    /// `a_returning_activation_keeps_its_registers_only_when_it_was_marked`
    /// in `tests/heap.rs`.
    #[test]
    fn popping_a_reified_activation_divorces_its_frame() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Initialized");
        let activation = heap
            .push_activation(
                ActivationHeader {
                    current_class: Some(class),
                    kind: ActivationKind::Initializer(class, InitializerBody::OwnPart),
                    code: shaped_code(1, first_above_header(1) + 1),
                    ..header()
                },
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        heap.activation_set_program_counter(activation, 4);
        let frame = heap.reify(activation);
        // What a `MakeClosure` whose block reads this frame by reference
        // does, and the only reason the registers below survive the pop.
        heap.retain_registers(activation);
        // Written after reification: it must survive the divorce.
        let temporary = first_above_header(1);
        heap.activation_set_local(activation, temporary, Value::small_integer(3));
        heap.pop_activation(activation);

        assert!(heap.frame_activation(frame).is_none());
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(1));
        assert_eq!(
            heap.frame_local(frame, temporary).as_small_integer(),
            Some(3)
        );
        assert_eq!(
            heap.frame_register_shape(frame),
            (4, 1, 2),
            "the argument and the temporary, the header's numbers left out"
        );
        assert!(heap.frame_is_uncontinuable(frame));
        assert!(heap.frame_sender(frame).is_none());
        heap.frame_set_local(frame, temporary, Value::small_integer(8));
        assert_eq!(
            heap.frame_local(frame, temporary).as_small_integer(),
            Some(8)
        );
    }

    /// A pop leaves its frame's words above the top, and no pass of either
    /// collector reads them: what only the popped frames held is reclaimed,
    /// what the frame still running holds survives.
    #[test]
    fn a_collection_after_a_deep_pop_reads_nothing_above_the_top() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let weak_class = old_instance_class(&mut heap);
        let mut activations = Vec::new();
        let mut objects = Vec::new();
        for _ in 0..100 {
            let object = heap.allocate_instance(class, 0);
            objects.push(object);
            activations.push(
                heap.push_activation(
                    header_taking(1),
                    Arguments::Values(&[Value::object(object)].as_slice()),
                    0,
                )
                .expect("the stack holds the frame"),
            );
        }
        let weak = heap.allocate_weak_array(weak_class, objects.len());
        for (index, object) in objects.iter().enumerate() {
            assert!(heap.weak_array_set_element(weak, index, Value::object(*object)));
        }
        for activation in activations[1..].iter().rev() {
            heap.pop_activation(*activation);
        }
        let mut weak_root = Value::object(weak);
        scavenge_bare(&mut heap, false, &mut [&mut weak_root]);
        heap.collect_for_test(&mut [&mut weak_root]);

        let weak = rooted(weak_root);
        let elements = heap
            .weak_array_elements(weak)
            .expect("a weak array answers its elements");
        let bottom = activations[0];
        let kept = heap
            .activation_local(bottom, 0)
            .as_object()
            .expect("the bottom frame still holds its object");
        assert!(
            names(elements[0], kept),
            "the bottom frame's object survives"
        );
        assert!(
            elements[1..].iter().all(|element| element.is_nil()),
            "what only a popped frame held is gone"
        );
        heap.pop_activation(bottom);
    }

    /// Every word of the frame stack is a valid tagged value -- a
    /// `SmallInteger`, nil, a Boolean or the offset of an object in the
    /// space -- whatever kind of activation pushed it, which is what lets
    /// the collector trace every word of `[0, top)` with no stack map
    /// (`Heap::trace_stack`).
    #[test]
    fn every_word_of_the_stack_is_a_tagged_value() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Framed");
        let method = Rc::new(crate::mixin::MixinMethod {
            selector: Symbol::intern("framed:"),
            access: ns_syntax::ast::AccessModifier::Public,
            argument_count: 1,
            declaration: None,
            code: shaped_code(1, first_above_header(1) + 2),
            fast_path: None,
            class_accessor: None,
        });
        let method_activation = heap
            .push_activation(
                ActivationHeader {
                    receiver: Value::object(class),
                    current_class: Some(class),
                    kind: ActivationKind::Method(method.selector),
                    code: method.code.clone(),
                    method: Some(method.clone()),
                    then: Continuation::Unwind(Box::new(PendingUnwind {
                        target: None,
                        action: UnwindAction::Restart,
                    })),
                    ..header()
                },
                Arguments::Values(&[Value::boolean(true)].as_slice()),
                2,
            )
            .expect("the stack holds the frame");
        heap.activation_set_program_counter(method_activation, 3);
        let home = heap.reify(method_activation);
        heap.retain_registers(method_activation);
        let closure = heap.allocate_closure(
            class,
            NewClosure {
                code: shaped_code(0, 1),
                captured_frame: home,
                copied: vec![],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        let kinds = [
            ActivationKind::Closure(closure),
            ActivationKind::TopLevel,
            ActivationKind::Initializer(class, InitializerBody::Full),
            ActivationKind::Initializer(class, InitializerBody::OwnPart),
            ActivationKind::Initializer(class, InitializerBody::Standalone),
            ActivationKind::SuperclassClause(class),
            ActivationKind::Factory(class),
        ];
        let mut activations = vec![method_activation];
        for kind in kinds {
            let current_class = match kind {
                ActivationKind::TopLevel => None,
                _ => Some(class),
            };
            // A top-level activation takes no arguments; every other kind
            // is handed as many as its code takes.
            let arguments = [Value::small_float(1.5), Value::object(closure)];
            let arguments = match kind {
                ActivationKind::TopLevel => &arguments[..0],
                _ => &arguments[..],
            };
            let argument_count = arguments.len() as u16;
            let activation = heap
                .push_activation(
                    ActivationHeader {
                        receiver: Value::small_integer(-4),
                        current_class,
                        kind,
                        code: shaped_code(argument_count, first_above_header(argument_count) + 1),
                        ..header()
                    },
                    Arguments::Values(&arguments),
                    1,
                )
                .expect("the stack holds the frame");
            heap.reify(activation);
            activations.push(activation);
        }

        let space_end = (heap.space.words().len() * 8) as u64;
        let constants = [
            Word::NIL.bits(),
            Value::boolean(true).word().bits(),
            Value::boolean(false).word().bits(),
        ];
        let words = heap.stack.frames.words();
        assert!(!words.is_empty());
        for (position, bits) in words.iter().enumerate() {
            let word = Word::from_bits(*bits);
            // A debug build's dead register word too, which every tracer
            // passes over as naming no object.
            let valid = word.is_small_integer()
                || constants.contains(bits)
                || *bits == dead_register().bits()
                || bits & 0b111 == 0b100
                || word
                    .object_offset()
                    .is_some_and(|offset| offset.get() < space_end);
            assert!(
                valid,
                "word {position} of the stack is no tagged value: {bits:#x}"
            );
        }
        for activation in activations.into_iter().rev() {
            heap.pop_activation(activation);
        }
    }

    /// The image decoder fills each record's placeholder in place, so that a
    /// record costs its placeholder and nothing beside it; the block it
    /// leaves must be exactly the one the normal allocation writes --
    /// header (format, size, unused units, guard, identity hash) and words.
    #[test]
    fn a_record_of_fields_filled_in_place_is_the_block_its_allocation_writes() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Thing");
        let values = [Value::small_integer(7), Value::nil(), Value::object(class)];
        for format in [
            PlaceholderFormat::Instance,
            PlaceholderFormat::Array,
            PlaceholderFormat::WeakArray,
            PlaceholderFormat::Ephemeron,
        ] {
            let placeholder = heap.allocate_placeholder(class, values.len());
            heap.shape_placeholder(placeholder, class, 42, format);
            for (index, value) in values.iter().enumerate() {
                assert!(heap.fill_placeholder_field(placeholder, index, *value));
            }
            assert!(
                !heap.fill_placeholder_field(placeholder, values.len(), Value::nil()),
                "a field past the placeholder is refused"
            );
            let built = match format {
                PlaceholderFormat::Instance => {
                    let instance = heap.allocate_instance(class, values.len());
                    for (index, value) in values.iter().enumerate() {
                        assert!(heap.instance_set_field(instance, index, *value));
                    }
                    instance
                }
                PlaceholderFormat::Array => heap.allocate_array(class, &values),
                PlaceholderFormat::WeakArray => {
                    let weak = heap.allocate_weak_array(class, values.len());
                    for (index, value) in values.iter().enumerate() {
                        assert!(heap.weak_array_set_element(weak, index, *value));
                    }
                    weak
                }
                PlaceholderFormat::Ephemeron => {
                    let ephemeron = heap.allocate_ephemeron(class, values.len());
                    for (index, value) in values.iter().enumerate() {
                        assert!(heap.ephemeron_set_field(ephemeron, index, *value));
                    }
                    ephemeron
                }
            };
            heap.object_set_identity_hash(built, 42);
            assert_eq!(heap.block(placeholder), heap.block(built));
        }
    }

    /// Strings and symbols: packed in place into a placeholder of the size
    /// `record_size::string_fields` gives, in the width `CodePoints` picks,
    /// with the same unused units -- narrow, Latin-1 and wide, at and either
    /// side of a word's boundary.
    #[test]
    fn a_string_filled_in_place_is_the_block_its_allocation_writes() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "String");
        for text in [
            "",
            "abc",
            "abcdefgh",
            "abcdefghi",
            "caf\u{e9} \u{ff}",
            "\u{3a9}",
            "a\u{20ac}bc",
            "wide \u{1f600} text",
        ] {
            let placeholder = heap.allocate_placeholder(class, record_size::string_fields(text));
            assert!(heap.fill_placeholder_string(placeholder, class, 9, text));
            let built = heap.allocate_string(class, crate::CodePoints::from(text));
            heap.object_set_identity_hash(built, 9);
            assert_eq!(heap.block(placeholder), heap.block(built), "{text:?}");
            let wrong_size = heap.allocate_placeholder(class, record_size::string_fields(text) + 1);
            assert!(!heap.fill_placeholder_string(wrong_size, class, 9, text));
        }
    }

    /// A ByteArray filled in place: the header its allocation writes, its
    /// data at the placeholder's own 32-byte alignment wherever the
    /// placeholder lies, every other field word zero; a placeholder of any
    /// other size is refused untouched.
    #[test]
    fn a_byte_array_filled_in_place_is_aligned_where_its_placeholder_lies() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Object");
        for size in [0, 1, 8, 13, 40] {
            let bytes: Vec<u8> = (0..size).map(|index| index as u8 ^ 0x5A).collect();
            for residue in 0..object::ALIGNMENT_WORDS {
                // A filler that puts the placeholder's header at `residue`
                // within a 32-byte line: the filler's header is two words
                // after the probe's, and the placeholder's two words and
                // the filler's fields after that.
                let probe = heap.allocate_instance(class, 0);
                let next = header_index(probe.offset) + 2;
                let line = object::ALIGNMENT_WORDS;
                let fields = (residue + 2 * line - (next + 2) % line) % line;
                heap.allocate_instance(class, fields);
                let placeholder =
                    heap.allocate_placeholder(class, record_size::byte_array_fields(size));
                assert!(heap.fill_placeholder_byte_array(placeholder, class, 9, &bytes));
                assert!(heap.is_byte_array(placeholder));
                assert_eq!(heap.byte_array_bytes(placeholder), Some(bytes.clone()));
                assert_eq!(heap.object_identity_hash(placeholder), 9);
                let built = heap.allocate_byte_array(class, &bytes);
                heap.object_set_identity_hash(built, 9);
                assert_eq!(heap.header(placeholder).0, heap.header(built).0);
                let index = header_index(placeholder.offset);
                let start = object::aligned_data_start(index);
                assert_eq!(start % object::ALIGNMENT_WORDS, 0);
                assert_eq!(index % object::ALIGNMENT_WORDS, residue);
                let block = block_range(heap.space.words(), index);
                let data = start..start + size.div_ceil(8);
                for position in block.start + FIELDS..block.end {
                    if !data.contains(&position) {
                        assert_eq!(heap.space.words()[position], 0, "the reserve is zero");
                    }
                }
                let wrong_size =
                    heap.allocate_placeholder(class, record_size::byte_array_fields(size) + 1);
                assert!(!heap.fill_placeholder_byte_array(wrong_size, class, 9, &bytes));
                assert!(!heap.is_byte_array(wrong_size), "nothing was written");
            }
        }
        // Shorter than the reserve, the one block `byte_array_size` could
        // not read: never built, whatever bytes it is asked to hold.
        for fields in 0..object::ALIGNMENT_RESERVE {
            let short = heap.allocate_placeholder(class, fields);
            assert!(!heap.fill_placeholder_byte_array(short, class, 9, &[]));
            assert!(!heap.is_byte_array(short));
        }
    }

    /// A closure filled in place: its fixed fields and its copied values.
    #[test]
    fn a_closure_filled_in_place_is_the_block_its_allocation_writes() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Closure");
        let frame = heap.allocate_placeholder_frame(0);
        let code = trivial_code();
        let placeholder = heap.allocate_placeholder(class, record_size::CLOSURE_FIXED_FIELDS + 2);
        heap.fill_placeholder_closure(placeholder, class, 5, &code, frame);
        for (index, value) in [Value::small_integer(1), Value::object(class)]
            .into_iter()
            .enumerate()
        {
            assert!(heap.fill_placeholder_field(
                placeholder,
                record_size::CLOSURE_FIXED_FIELDS + index,
                value
            ));
        }
        let built = heap.allocate_closure(
            class,
            NewClosure {
                code,
                captured_frame: frame,
                copied: vec![Value::small_integer(1), Value::object(class)],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        heap.object_set_identity_hash(built, 5);
        assert_eq!(heap.block(placeholder), heap.block(built));
    }

    /// A class placeholder the first pass sized wrongly is refused and left as
    /// it was: the space cannot resize an object in place, and a valid first
    /// pass never sizes a class record otherwise.
    #[test]
    fn a_class_placeholder_of_the_wrong_size_is_refused_and_left_alone() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Point");
        let placeholder = heap.allocate_placeholder(class, 3);
        let application = heap.class_application(class).expect("a class");
        let filled = heap.fill_placeholder_class(
            placeholder,
            class,
            0,
            NewClass {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: None,
                builtin: None,
            },
            None,
            None,
        );
        assert!(!filled);
        assert!(!heap.is_class(placeholder));
        assert_eq!(heap.instance_field_count(placeholder), Some(3));
    }

    /// A frame filled in place -- its fixed fields, then register by
    /// register -- is the frame `allocate_frame` builds from the same fields
    /// and registers.
    #[test]
    fn a_frame_filled_in_place_is_the_block_its_allocation_writes() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Thing");
        let sender = heap.allocate_placeholder_frame(0);
        let registers = [Value::small_integer(3), Value::nil(), Value::object(class)];
        let built = heap.allocate_returned_top_level_frame(
            Value::small_integer(11),
            &registers,
            Some(sender),
            Some(class),
        );
        heap.frame_set_program_counter(built, 2);
        let filled = heap.allocate_placeholder_frame(registers.len());
        for frame in [built, filled] {
            heap.object_set_identity_hash(heap.frame_object(frame), 17);
        }
        heap.fill_frame_fields(
            filled,
            FrameRecord {
                receiver: Value::small_integer(11),
                arguments: Vec::new(),
                temporaries: Vec::new(),
                program_counter: 2,
                sender: Some(sender),
                home: None,
                current_class: Some(class),
                kind: FrameKind::TopLevel,
                uncontinuable: true,
            },
            None,
        );
        for (index, value) in registers.iter().enumerate() {
            assert!(heap.fill_frame_register(filled, index, *value));
        }
        assert!(
            !heap.fill_frame_register(filled, registers.len(), Value::nil()),
            "a register past the placeholder's file is refused"
        );
        assert_eq!(
            heap.block(heap.frame_object(filled)),
            heap.block(heap.frame_object(built))
        );
    }

    /// What a closure answers is what it was built with: a conversion of its
    /// representation keeps the code, the captured frame, the feedback base
    /// and the copied values.
    #[test]
    fn a_closure_keeps_its_code_frame_feedback_base_and_copied_values_in_its_words() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let frame = heap.allocate_placeholder_frame(0);
        let code = trivial_code();
        let object = heap.allocate_closure(
            class,
            NewClosure {
                code: code.clone(),
                captured_frame: frame,
                copied: vec![Value::small_integer(5)],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        let (body, captured, base) = heap.closure_activation(object).expect("a closure");
        assert!(std::rc::Rc::ptr_eq(&body, &code));
        assert!(captured.same(frame));
        assert_eq!(base, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(
            heap.closure_copied_values(object)
                .map(|values| values.len()),
            Some(1)
        );
    }

    /// A block's copied values land in the registers above its header,
    /// before its temporaries, which is where its compiled code addresses them
    /// (`docs/design/execution.md`, "Converted representation"). A list
    /// longer than the register file stops at its end rather than writing
    /// into the activation below: an image, not the compiler, is what can
    /// present one.
    #[test]
    fn a_closures_copied_values_land_after_the_activations_arguments() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let captured_frame = top_level_frame(&mut heap, 0);
        let closure = heap.allocate_closure(
            class,
            NewClosure {
                code: trivial_code(),
                captured_frame,
                copied: vec![Value::small_integer(7), Value::small_integer(8)],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );

        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        heap.fill_copied_values(activation, closure);
        assert_eq!(
            heap.activation_local(activation, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(activation, first_above_header(1))
                .as_small_integer(),
            Some(7)
        );
        // The activation has two registers, so the second copied value has
        // nowhere to go and is dropped rather than written past the window.
        assert_eq!(heap.stack.frames.top(), frame_header::COUNT + 2);
        heap.pop_activation(activation);
    }

    /// The next activation at the same depth reuses the stack slots. A
    /// divorced frame owns its own copy, so that reuse cannot reach it.
    #[test]
    fn a_divorced_frame_is_untouched_by_the_next_activation_at_its_depth() {
        let mut heap = frame_heap();
        let first = heap
            .push_activation(header(), Arguments::NONE, 2)
            .expect("the stack holds the frame");
        let register = first_above_header(0) + 1;
        heap.activation_set_local(first, register, Value::small_integer(3));
        let frame = heap.reify(first);
        heap.retain_registers(first);
        heap.pop_activation(first);

        let second = heap
            .push_activation(header(), Arguments::NONE, 2)
            .expect("the stack holds the frame");
        heap.activation_set_local(second, register, Value::small_integer(99));
        assert_eq!(
            heap.frame_local(frame, register).as_small_integer(),
            Some(3)
        );
        heap.pop_activation(second);
    }

    /// A register base names a window by index, as an activation handle
    /// does: reading through it reads the same register the activation
    /// handle does.
    #[test]
    fn a_register_base_reads_and_writes_its_activations_registers() {
        let mut heap = Heap::new();
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(7)].as_slice()),
                2,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(activation);
        assert_eq!(heap.register(base, 0).as_small_integer(), Some(7));
        let register = first_above_header(1) + 1;
        heap.set_register(base, register, Value::small_integer(9));
        assert_eq!(
            heap.activation_local(activation, register)
                .as_small_integer(),
            Some(9)
        );
        heap.pop_activation(activation);
    }

    /// A base kept past its activation's pop names whatever activation
    /// reuses the window next -- a different, valid window, so a release
    /// build would read the wrong register. Debug builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale activation handle")]
    fn reading_through_a_stale_register_base_panics() {
        let mut heap = Heap::new();
        let first = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(first);
        heap.pop_activation(first);
        let _second = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _ = heap.register(base, 0);
    }

    /// The window of a deeper activation starts after its caller's: a base
    /// survives the register file growing, since it is an index.
    #[test]
    fn a_register_base_survives_a_deeper_push() {
        let mut heap = Heap::new();
        let caller = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(caller);
        let callee = heap
            .push_activation(header(), Arguments::NONE, 4096)
            .expect("the stack holds the frame");
        assert_eq!(heap.register(base, 0).as_small_integer(), Some(1));
        heap.pop_activation(callee);
        heap.pop_activation(caller);
    }

    /// In a release build a base is exactly its index.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_register_base_is_four_bytes_in_a_release_build() {
        assert_eq!(std::mem::size_of::<RegisterBase>(), 4);
    }

    /// An `ActivationReference` kept past its pop names whatever activation
    /// reuses the offset next -- a different, valid activation, so the read
    /// would silently answer the wrong thing. Debug builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale activation handle")]
    fn reading_through_a_popped_activation_handle_panics() {
        let mut heap = Heap::new();
        let first = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        heap.pop_activation(first);
        let _second = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _ = heap.activation_local(first, 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_an_activation_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let mine = origin
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let mut other = Heap::new();
        let _theirs = other
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _ = other.activation_local(mine, 0);
    }

    /// `ActivationReference::same` is how the unwinder stops at its target,
    /// and it is handed two handles
    /// and no `Heap`, so no accessor guard can reach it. Both handles below
    /// are the first frame of their own heap's stack, so they share an index
    /// and a release build answers "identical".
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap (comparing handle from heap")]
    fn comparing_two_activation_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let mine = origin
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let mut other = Heap::new();
        let theirs = other
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _ = mine.same(theirs);
    }

    /// The comparison `same`'s doc comment says the unwinder never makes: a
    /// handle whose frame was popped, compared against the activation that
    /// reused its index. A release build answers "identical", which would
    /// land a return in the wrong activation -- the reason an unwind's
    /// target is a frame -- and a debug build must refuse the comparison.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(
        expected = "stale activation handle: compared against a later activation at the same depth"
    )]
    fn comparing_a_popped_activation_handle_with_its_successor_panics() {
        let mut heap = Heap::new();
        let first = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        heap.pop_activation(first);
        let second = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _ = first.same(second);
    }

    /// Popping below the top would truncate the registers of every
    /// activation above it while their handles still look valid.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "only the top activation")]
    fn popping_an_activation_that_is_not_on_top_panics() {
        let mut heap = Heap::new();
        let outer = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let _inner = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        heap.pop_activation(outer);
    }

    /// A hash is drawn from the counter when something first asks for one,
    /// not when the object is allocated, so allocation order is not
    /// observable (`docs/design/object-model.md`).
    #[test]
    fn an_identity_hash_is_assigned_on_first_request_not_at_allocation() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let first = heap.allocate_instance(class, 0);
        let second = heap.allocate_instance(class, 0);
        assert_eq!(
            heap.object_identity_hash(first),
            0,
            "allocation assigns no hash"
        );
        assert_eq!(
            heap.object_identity_hash(second),
            0,
            "allocation assigns no hash"
        );
        assert_eq!(
            heap.object_ensure_identity_hash(second),
            1,
            "the first request draws the first number"
        );
        assert_eq!(heap.object_ensure_identity_hash(first), 2);
        assert_eq!(
            heap.object_ensure_identity_hash(second),
            1,
            "a second request answers the same hash"
        );
        assert_eq!(
            heap.object_identity_hash(second),
            1,
            "the assigned hash is stored"
        );
    }

    /// Every test above pushes `FrameKind::TopLevel`, which is `None` on both
    /// `activation_home` and `activation_captured` by construction -- closure
    /// creation is the only reason either exists. `raise_return` picks a
    /// non-local return's target off `activation_home`, and the outer-local
    /// walk (`locals::frame_at_depth`) starts every hop off
    /// `activation_captured`/`frame_captured`, so a closure activation that
    /// answered `None` for either would silently misroute a `^` or a
    /// captured-variable read rather than fail loudly. The closure's two frames
    /// are distinct -- a block inside a block captures the outer block's
    /// activation and returns to the method's -- so that the two swapped would
    /// fail here rather than pass. The home is derived from the captured frame
    /// (`closure_home`) rather than stored, which is exactly what the two
    /// nesting levels below exercise: the inner closure's captured frame is a
    /// *block* activation, so its home is that frame's own. This also exercises
    /// `allocate_frame`'s debug assertion that a frame's home is populated
    /// exactly for a `FrameKind::Closure` activation, since `reify` builds that
    /// frame from a closure activation's `kind` here for the first time in this
    /// test module.
    #[test]
    fn a_closure_activations_home_and_captured_frame_are_its_closures() {
        let mut heap = frame_heap();
        fn block_closure<'gc>(
            heap: &mut Heap<'gc>,
            class: ClassReference<'gc>,
            captured_frame: FrameReference<'gc>,
        ) -> ObjectReference<'gc> {
            heap.allocate_closure(
                class,
                NewClosure {
                    code: std::rc::Rc::new(crate::bytecode::CompiledCode::new(
                        crate::bytecode::CodeKind::Block,
                        vec![crate::bytecode::Instruction::ImplicitReturn { register: 0 }],
                        0,
                        1,
                        0,
                        vec![],
                        ns_syntax::Span::new(0, 0),
                    )),
                    captured_frame,
                    copied: vec![],
                    feedback_base: FeedbackBase::NOT_ASSIGNED,
                },
            )
        }
        let class = heap.allocate_self_referential(0);
        let block_header = |closure| ActivationHeader {
            receiver: Value::nil(),
            current_class: None,
            kind: ActivationKind::Closure(closure),
            method: None,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        };

        let method_activation = heap
            .push_activation(header(), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        let home_frame = heap.reify(method_activation);
        let outer_closure = block_closure(&mut heap, class, home_frame);
        let outer_block_activation = heap
            .push_activation(block_header(outer_closure), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        let captured_frame = heap.reify(outer_block_activation);
        assert!(!captured_frame.same(home_frame));

        let closure = block_closure(&mut heap, class, captured_frame);
        let closure_activation = heap
            .push_activation(block_header(closure), Arguments::NONE, 0)
            .expect("the stack holds the frame");

        assert!(heap.activation_home(method_activation).is_none());
        assert!(heap.activation_captured(method_activation).is_none());
        assert!(heap
            .activation_home(closure_activation)
            .is_some_and(|home| home.same(home_frame)));
        assert!(heap
            .activation_captured(closure_activation)
            .is_some_and(|captured| captured.same(captured_frame)));

        let closure_frame = heap.reify(closure_activation);
        assert!(heap
            .frame_home(closure_frame)
            .is_some_and(|home| home.same(home_frame)));
        assert!(heap
            .frame_captured(closure_frame)
            .is_some_and(|captured| captured.same(captured_frame)));

        heap.pop_activation(closure_activation);
        heap.pop_activation(outer_block_activation);
        heap.pop_activation(method_activation);
    }

    /// Moves a bare heap, one no interpreter owns: there are no interpreter
    /// roots to forward, so the forwarder `move_everything` answers is
    /// dropped here on purpose. A handle a test keeps across the move is
    /// left stale, which is what the stale-handle tests below rely on.
    #[cfg(debug_assertions)]
    fn move_bare_heap<'gc>(heap: &mut Heap<'gc>) {
        let _ = heap.move_everything();
    }

    /// A move gives every object, every frame object among them, a new
    /// offset -- the shift is never zero -- and rewrites everything that
    /// names one: an array's element, a closure's captured frame and its
    /// copied values, a frame's receiver and its registers, a class's own
    /// class, the heap's `Activation` class, and the host roots.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_gives_every_object_a_new_offset_and_rewrites_what_names_it() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let string = heap.allocate_string(class, crate::CodePoints::from("a"));
        let array = heap.allocate_array(class, &[Value::object(string)]);
        let frame = heap.allocate_returned_top_level_frame(
            Value::object(string),
            &[Value::object(array)],
            None,
            None,
        );
        top_level_frame(&mut heap, 0);
        let closure = heap.allocate_closure(
            class,
            NewClosure {
                code: trivial_code(),
                captured_frame: frame,
                copied: vec![Value::object(string)],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        let before = [
            heap.object_identity_key(class),
            heap.object_identity_key(string),
            heap.object_identity_key(array),
            heap.object_identity_key(closure),
        ];
        let frame_before = heap.object_identity_key(heap.frame_object(frame));
        let roots = [
            heap.stash(Value::object(class)),
            heap.stash(Value::object(string)),
            heap.stash(Value::object(array)),
            heap.stash(Value::object(closure)),
        ];
        move_bare_heap(&mut heap);
        let after: Vec<ObjectReference> = roots
            .iter()
            .map(|root| match heap.fetch(root).as_object() {
                Some(object) => object,
                None => panic!("a root keeps its object"),
            })
            .collect();
        for (key, object) in before.iter().zip(&after) {
            assert_ne!(
                *key,
                heap.object_identity_key(*object),
                "every object moved"
            );
        }
        let (class, string, array, closure) = (after[0], after[1], after[2], after[3]);
        assert!(
            heap.object_class(class).same(class),
            "a class's own class is rewritten"
        );
        assert!(heap
            .array_element(array, 0)
            .and_then(|element| element.as_object())
            .is_some_and(|element| element.same(string)));
        assert_eq!(heap.str_value(string).as_deref(), Some("a"));
        let copied = heap
            .closure_copied_values(closure)
            .expect("still a closure");
        assert!(
            matches!(copied[..], [copied] if copied.as_object().is_some_and(|copied| copied.same(string))),
            "a copied value is rewritten"
        );
        let captured_frame = heap
            .closure_captured_frame(closure)
            .expect("still a closure");
        assert_ne!(
            frame_before,
            heap.object_identity_key(heap.frame_object(captured_frame)),
            "every frame moved"
        );
        assert!(heap
            .frame_receiver(captured_frame)
            .as_object()
            .is_some_and(|receiver| receiver.same(string)));
        assert!(heap
            .frame_local(captured_frame, first_above_header(0))
            .as_object()
            .is_some_and(|register| register.same(array)));
        let activation_class = heap.activation_class.expect("recorded");
        assert!(
            heap.object_class(heap.frame_object(captured_frame))
                .same(activation_class),
            "a frame object's class and the heap's `Activation` class are rewritten alike"
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_an_object_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let string = heap.allocate_string(class, crate::CodePoints::from("a"));
        move_bare_heap(&mut heap);
        let _ = heap.str_value(string);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_a_frame_handle_minted_before_a_move_panics() {
        let mut heap = frame_heap();
        let frame = top_level_frame(&mut heap, 0);
        top_level_frame(&mut heap, 0);
        move_bare_heap(&mut heap);
        let _ = heap.frame_receiver(frame);
    }

    /// `same` reads no heap, so a stale handle compared with a fresh one
    /// must be refused there too, or the two could answer "identical".
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn comparing_a_stale_handle_with_a_fresh_one_panics() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let root = heap.stash(Value::object(class));
        move_bare_heap(&mut heap);
        let Some(fresh) = heap.fetch(&root).as_object() else {
            panic!("a root keeps its object")
        };
        let _ = class.same(fresh);
    }

    /// A stale handle written into the heap -- a host that stored a value it
    /// kept past a run -- is refused by the store itself: a stored word
    /// carries no epoch, so the next move could no longer tell it is stale.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "stale handle")]
    fn a_stale_handle_is_caught_where_it_is_stored() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let stale = heap.allocate_string(class, crate::CodePoints::from("stale"));
        let array = heap.allocate_array(class, &[Value::nil()]);
        let root = heap.stash(Value::object(array));
        move_bare_heap(&mut heap);
        let Some(array) = heap.fetch(&root).as_object() else {
            panic!("a root keeps its object")
        };
        heap.array_set_element(array, 0, Value::object(stale));
    }

    #[test]
    fn a_mixin_keeps_one_id_and_two_mixins_never_share_one() {
        let mut heap = Heap::new();
        let first = std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern("First")));
        let second = std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Second")));
        let first_id = heap.mixin_id(&first);
        assert_eq!(heap.mixin_id(&first), first_id);
        assert_ne!(heap.mixin_id(&second), first_id);
        assert!(std::rc::Rc::ptr_eq(
            &heap.mixin(first_id).expect("registered"),
            &first
        ));
    }

    #[test]
    fn an_application_answers_its_own_index_and_the_heap_answers_it_back() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(crate::BuiltinClass::Object);
            let application = mutator.heap().class_application(object).expect("a class");
            let back = mutator
                .heap()
                .application(application.index())
                .expect("registered");
            assert!(std::rc::Rc::ptr_eq(&back, &application));
        });
    }

    #[test]
    fn compiled_code_keeps_one_id_and_records_its_method() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(crate::BuiltinClass::Object);
            let application = mutator.heap().class_application(object).expect("a class");
            let method = application
                .mixin
                .compiled_methods()
                .next()
                .expect("Object has a method")
                .clone();
            let id = mutator.heap_mut().code_id_of_method(&method);
            assert_eq!(mutator.heap_mut().code_id(&method.code), id);
            assert!(std::rc::Rc::ptr_eq(
                &mutator.heap().code(id).expect("registered"),
                &method.code
            ));
            assert!(std::rc::Rc::ptr_eq(
                &mutator.heap().code_method(id).expect("a method"),
                &method
            ));
        });
    }

    /// A class object whose data is not a class -- only a crafted image makes
    /// an instance of one -- answers `NONE`, which nothing caches.
    #[test]
    fn an_instance_of_a_non_class_answers_no_dispatch_id() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let not_a_class = mutator.heap.allocate_self_referential(0);
            let instance = mutator.heap.allocate_instance(not_a_class, 0);
            assert_eq!(
                mutator.heap.dispatch_id(Value::object(instance)),
                crate::mixin::DispatchId::NONE
            );
        });
    }

    /// A handle is one word, and the offset's niche keeps an optional handle
    /// one word too: `Option<ClassReference>` sits in `Step`, whose size the
    /// hot path was measured at.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_handle_and_an_optional_handle_are_one_word() {
        assert_eq!(std::mem::size_of::<ObjectReference<'static>>(), 8);
        assert_eq!(std::mem::size_of::<Option<ObjectReference<'static>>>(), 8);
        assert_eq!(std::mem::size_of::<FrameReference<'static>>(), 8);
        assert_eq!(std::mem::size_of::<Option<FrameReference<'static>>>(), 8);
    }

    /// A running activation's handle is its frame's word offset, and the
    /// argument window packs its base, start and count into one word, so
    /// that each travels in a register (`interpreter`'s
    /// `what_the_hot_path_passes_by_value_is_at_most_two_scalars` says
    /// why).
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn an_activation_handle_and_an_argument_window_are_one_scalar_each() {
        assert_eq!(std::mem::size_of::<ActivationReference>(), 4);
        assert_eq!(std::mem::size_of::<ArgumentSlots>(), 8);
    }

    /// What `pop_activation` answers on every return is a continuation and
    /// a flag, 24 bytes: one more than two scalars, so it comes back
    /// through memory. Pinned at the size it was measured at, so that it
    /// does not grow; whether it can come back in two scalars is open
    /// (`docs/OPEN-WORK.md`).
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn what_a_pop_answers_stays_the_size_it_was_measured_at() {
        assert_eq!(std::mem::size_of::<Popped<'static>>(), 24);
    }

    /// Marks from the heap's own roots, answers whether each of `objects` was
    /// reached, and clears every mark again, so that the test can go on to
    /// collect: a mark left set would make the next marking skip that object's
    /// edges.
    fn marked_by_marking<'gc>(heap: &mut Heap<'gc>, objects: &[ObjectReference<'gc>]) -> Vec<bool> {
        let mut marker = heap.begin_marking();
        heap.drain_marking(&mut marker);
        let marked = objects
            .iter()
            .map(|object| heap.header(*object).marked())
            .collect();
        heap.clear_marks_for_test();
        marked
    }

    #[test]
    fn a_collection_leaves_no_mark_bit_set() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        heap.allocate_string(class, crate::CodePoints::from("dead"));
        let mut root = Value::object(kept);
        heap.collect_for_test(&mut [&mut root]);
        let Some(kept) = root.as_object() else {
            panic!("the root keeps its object")
        };
        assert!(!heap.header(kept).marked());
        assert!(!heap.header(heap.object_class(kept)).marked());
    }

    /// Liveness is reachability from the roots, and the roots are what the
    /// heap holds, not what a Rust local happens to name. `unreachable` is a
    /// perfectly valid object that nothing can get to, which is exactly the
    /// case the collector exists for.
    #[test]
    fn marking_reaches_what_a_root_reaches_and_nothing_else() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let reachable = heap.allocate_string(class, crate::CodePoints::from("live"));
        let unreachable = heap.allocate_string(class, crate::CodePoints::from("dead"));
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(reachable)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");

        let marked = marked_by_marking(&mut heap, &[reachable, class, unreachable]);

        assert_eq!(marked, vec![true, true, false]);
        heap.pop_activation(activation);
    }

    /// An edge is followed, not just a root: the value inside a reachable
    /// instance keeps its own target alive.
    #[test]
    fn marking_follows_an_edge_out_of_a_reachable_object() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let target = heap.allocate_string(class, crate::CodePoints::from("target"));
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(target)));
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(holder)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");

        let marked = marked_by_marking(&mut heap, &[target]);

        assert_eq!(marked, vec![true]);
        heap.pop_activation(activation);
    }

    /// A reified frame is reachable from the stack frame it is married to,
    /// which is what makes "a running activation's frame is never collected"
    /// fall out of the root set instead of needing a rule of its own.
    #[test]
    fn marking_reaches_a_married_frame_through_its_stack_frame() {
        let mut heap = frame_heap();
        let activation = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        let frame_object = heap.frame_object(frame);

        let marked = marked_by_marking(&mut heap, &[frame_object]);

        assert_eq!(marked, vec![true]);
        heap.pop_activation(activation);
    }

    /// An activation object is its frame's object: it answers the frame and
    /// describes itself as an Activation; any other object answers no frame
    /// rather than panicking, which is what a kernel leaf handed the wrong
    /// object relies on.
    #[test]
    fn an_activation_object_answers_its_frame() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let frame = top_level_frame(&mut heap, 0);
        let activation_object = heap.frame_object(frame);
        let other = heap.allocate_string(class, crate::CodePoints::from("other"));

        assert!(heap
            .object_frame(activation_object)
            .is_some_and(|answered| answered.same(frame)));
        assert!(heap.object_frame(other).is_none());
        assert_eq!(
            heap.describe(Value::object(activation_object)),
            "an Activation"
        );
    }

    /// The collector traces an activation object as the frame it is: a frame
    /// that only a register holding its object reaches stays alive.
    #[test]
    fn marking_reaches_a_frame_through_an_activation_object() {
        let mut heap = frame_heap();
        let frame = top_level_frame(&mut heap, 0);
        let activation_object = heap.frame_object(frame);
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(activation_object)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");

        let marked = marked_by_marking(&mut heap, &[activation_object]);

        assert_eq!(marked, vec![true]);
        heap.pop_activation(activation);
    }

    /// A collection moves a married frame like any object and leaves its
    /// marriage alone: the field names a stack frame, and no stack frame moves,
    /// so the frame found again through its forwarded object still reads the
    /// live registers and still answers its activation.
    #[test]
    fn a_married_frame_survives_a_collection_and_reads_its_live_registers() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Collected");
        // Garbage below the frame, so that compaction moves it.
        top_level_frame(&mut heap, 2);
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 2),
                Arguments::Values(&[Value::small_integer(7)].as_slice()),
                2,
            )
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        heap.set_register(
            heap.activation_register_base(activation),
            first_above_header(1),
            Value::small_integer(9),
        );
        let mut root = Value::object(heap.frame_object(frame));
        heap.collect_for_test(&mut [&mut root]);
        let frame = heap
            .object_frame(root.as_object().expect("an object"))
            .expect("a frame");
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(7));
        assert_eq!(
            heap.frame_local(frame, first_above_header(1))
                .as_small_integer(),
            Some(9)
        );
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(activation)));
        heap.pop_activation(activation);
    }

    /// The divorce after a collection writes into the compacted object: a
    /// retained activation's registers are found there once it is popped,
    /// and an unretained one's stay nil. Both frames are moved, by the
    /// garbage below them.
    #[test]
    fn a_frame_divorced_after_a_collection_holds_its_registers_only_when_retained() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Divorced");
        top_level_frame(&mut heap, 2);
        let retained = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(7)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let retained_frame = heap.reify(retained);
        heap.retain_registers(retained);
        heap.activation_set_local(retained, first_above_header(1), Value::small_integer(8));
        let unretained = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(5)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let unretained_frame = heap.reify(unretained);
        heap.activation_set_local(unretained, first_above_header(1), Value::small_integer(6));
        let mut retained_root = Value::object(heap.frame_object(retained_frame));
        let mut unretained_root = Value::object(heap.frame_object(unretained_frame));
        heap.collect_for_test(&mut [&mut retained_root, &mut unretained_root]);
        assert_ne!(
            heap.object_frame(retained_root.as_object().expect("an object"))
                .expect("a frame")
                .offset,
            retained_frame.offset,
            "the collection moved the frame"
        );

        heap.pop_activation(unretained);
        heap.pop_activation(retained);

        let retained_frame = heap
            .object_frame(retained_root.as_object().expect("an object"))
            .expect("a frame");
        assert!(heap.frame_activation(retained_frame).is_none());
        assert_eq!(
            heap.frame_local(retained_frame, 0).as_small_integer(),
            Some(7)
        );
        assert_eq!(
            heap.frame_local(retained_frame, first_above_header(1))
                .as_small_integer(),
            Some(8)
        );
        let unretained_frame = heap
            .object_frame(unretained_root.as_object().expect("an object"))
            .expect("a frame");
        assert!(heap.frame_activation(unretained_frame).is_none());
        assert!(heap.frame_local(unretained_frame, 0).is_nil());
        assert!(heap
            .frame_local(unretained_frame, first_above_header(1))
            .is_nil());
    }

    /// The frame whose object `root` holds.
    #[cfg(debug_assertions)]
    fn frame_at_root<'gc>(heap: &Heap<'gc>, root: &Root) -> FrameReference<'gc> {
        heap.object_frame(heap.fetch(root).as_object().expect("an object"))
            .expect("a frame")
    }

    /// A move shifts a married frame like any object and leaves its
    /// marriage alone: found again through a host root, it reads and writes
    /// the live registers and answers its activation, and the divorce after
    /// the move copies the retained registers into the moved object.
    #[test]
    #[cfg(debug_assertions)]
    fn a_married_frame_survives_a_move_and_is_divorced_into_its_moved_object() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Moved");
        top_level_frame(&mut heap, 2);
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(7)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        let before = frame.offset;
        let root = heap.stash(Value::object(heap.frame_object(frame)));
        move_bare_heap(&mut heap);
        let frame = frame_at_root(&heap, &root);
        assert_ne!(frame.offset, before, "the move gave the frame a new offset");
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(7));
        heap.frame_set_local(frame, first_above_header(1), Value::small_integer(9));
        assert_eq!(
            heap.activation_local(activation, first_above_header(1))
                .as_small_integer(),
            Some(9),
            "a write through the moved frame reaches the live register"
        );
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(activation)));

        heap.retain_registers(activation);
        heap.pop_activation(activation);

        let frame = frame_at_root(&heap, &root);
        assert!(heap.frame_activation(frame).is_none());
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(7));
        assert_eq!(
            heap.frame_local(frame, first_above_header(1))
                .as_small_integer(),
            Some(9)
        );
    }

    /// Two ordinary objects reference each other through their `Instance`
    /// payloads, so the traversal reaches back to the first one through the
    /// second. This is what pins that the traversal terminates on a cycle:
    /// the first object's header is already marked the second time it is
    /// reached, which is what stops its edges from being followed again. A regression here hangs this test rather
    /// than failing it.
    #[test]
    fn marking_terminates_on_a_cycle_between_two_ordinary_objects() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let first = heap.allocate_instance(class, 1);
        let second = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(second, 0, Value::object(first)));
        heap.instance_set_field(first, 0, Value::object(second));
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(first)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");

        let marked = marked_by_marking(&mut heap, &[first, second]);

        assert_eq!(marked, vec![true, true]);
        heap.pop_activation(activation);
    }

    /// The point of the step: an object nothing can reach is given back, and
    /// the space holds fewer objects afterwards.
    #[test]
    fn collecting_reclaims_what_no_root_reaches() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let reachable = heap.allocate_string(class, crate::CodePoints::from("live"));
        heap.allocate_string(class, crate::CodePoints::from("dead"));
        heap.allocate_string(class, crate::CodePoints::from("also dead"));
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(reachable)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let before = heap.heap_statistics().live_objects;

        heap.collect_for_test(&mut []);

        assert_eq!(before, 4);
        // The class, the reachable string, and nothing else.
        assert_eq!(heap.heap_statistics().live_objects, 2);
        heap.pop_activation(activation);
    }

    /// A survivor is reached again through the root that kept it alive, which
    /// is what pins that the forwarding rewrote the register rather than
    /// leaving it pointing at whatever now lies at the old offset.
    #[test]
    fn a_survivor_is_still_itself_after_compaction() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.allocate_string(class, crate::CodePoints::from("dead"));
        let reachable = heap.allocate_string(class, crate::CodePoints::from("live"));
        let activation = heap
            .push_activation(
                header_taking(1),
                Arguments::Values(&[Value::object(reachable)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");

        heap.collect_for_test(&mut []);

        let Some(survivor) = heap.activation_local(activation, 0).as_object() else {
            panic!("the register no longer holds an object");
        };
        assert_eq!(heap.str_value(survivor), Some("live".to_string()));
        heap.pop_activation(activation);
    }

    #[test]
    fn objects_lie_end_to_end_from_eden_s_first_offset() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let first = heap.allocate_instance(class, 2);
        let second = heap.allocate_instance(class, 0);
        let key = |object| heap.object_identity_key(object);
        assert_eq!(key(class), space::EDEN_START);
        assert_eq!(key(first), key(class) + 8 * 2);
        assert_eq!(key(second), key(first) + 8 * 4);
        assert_eq!(heap.heap_statistics().occupied_bytes, 8 * (2 + 4 + 2));
    }

    #[test]
    fn an_object_past_two_hundred_and_fifty_four_words_survives_a_collection_and_a_move() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        heap.allocate_instance(class, 3); // dies, so the large ones slide
        let elements: Vec<Value> = (0..300).map(Value::small_integer).collect();
        let array = heap.allocate_array(class, &elements);
        let text: String = std::iter::repeat_n('x', 2_500).collect();
        let string = heap.allocate_string(class, crate::CodePoints::from(text.as_str()));
        let after = heap.allocate_instance(class, 1);
        let mut roots = [
            Value::object(array),
            Value::object(string),
            Value::object(after),
        ];
        fn check<'gc>(heap: &Heap<'gc>, roots: &[Value<'gc>; 3]) {
            let array = roots[0].as_object().expect("an array");
            assert_eq!(heap.array_element_count(array), Some(300));
            assert_eq!(
                heap.array_element(array, 299)
                    .and_then(|element| element.as_small_integer()),
                Some(299)
            );
            assert_eq!(
                heap.string_size(roots[1].as_object().expect("a string")),
                Some(2_500)
            );
            assert_eq!(
                heap.instance_field_count(roots[2].as_object().expect("an instance")),
                Some(1)
            );
        }
        check(&heap, &roots);
        let [array_root, string_root, after_root] = &mut roots;
        heap.collect_for_test(&mut [array_root, string_root, after_root]);
        check(&heap, &roots);
        #[cfg(debug_assertions)]
        {
            let rooted: Vec<Root> = roots.iter().map(|root| heap.stash(*root)).collect();
            move_bare_heap(&mut heap);
            let moved = [
                heap.fetch(&rooted[0]),
                heap.fetch(&rooted[1]),
                heap.fetch(&rooted[2]),
            ];
            check(&heap, &moved);
        }
    }

    #[test]
    fn a_frame_of_two_hundred_and_fifty_registers_takes_an_overflow_word() {
        let mut heap = frame_heap();
        let before = heap.heap_statistics().occupied_bytes;
        let frame = top_level_frame(&mut heap, 250);
        assert_eq!(
            heap.heap_statistics().occupied_bytes - before,
            8 * allocation_words(1 + frame_field::COUNT + 250),
            "the overflow word is there"
        );
        assert_eq!(
            allocation_words(1 + frame_field::COUNT + 250),
            2 + frame_field::COUNT + 251
        );
        let last = first_above_header(0) + 249;
        heap.frame_set_local(frame, last, Value::small_integer(7));
        assert_eq!(heap.frame_local(frame, last).as_small_integer(), Some(7));
    }

    #[test]
    fn an_allocation_larger_than_the_first_commit_reads_back_its_last_element() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let elements = vec![Value::small_integer(1); 100_000];
        let array = heap.allocate_array(class, &elements);
        assert_eq!(
            heap.array_element(array, 99_999)
                .and_then(|element| element.as_small_integer()),
            Some(1)
        );
        assert!(heap.heap_statistics().committed_bytes >= 800_000);
    }

    /// A collection slides every survivor down to the first offset, in the
    /// order they were allocated, with nothing between them.
    #[test]
    fn a_collection_slides_the_survivors_together_from_the_first_offset() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.allocate_instance(class, 5);
        let kept = heap.allocate_instance(class, 2);
        heap.allocate_array(class, &vec![Value::nil(); 400]);
        let last = heap.allocate_instance(class, 0);
        let mut roots = [Value::object(kept), Value::object(last)];
        let [kept_root, last_root] = &mut roots;
        heap.collect_for_test(&mut [kept_root, last_root]);
        let kept = roots[0].as_object().expect("kept");
        let last = roots[1].as_object().expect("kept");
        assert_eq!(heap.object_identity_key(kept), space::NURSERY_END + 8 * 2);
        assert_eq!(
            heap.object_identity_key(last),
            heap.object_identity_key(kept) + 8 * 4
        );
        assert_eq!(heap.heap_statistics().live_objects, 3);
        assert_eq!(heap.heap_statistics().occupied_bytes, 8 * (2 + 4 + 2));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_an_object_handle_kept_across_a_collection_panics() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.allocate_instance(class, 1);
        let kept = heap.allocate_instance(class, 3);
        let mut root = Value::object(kept);
        heap.collect_for_test(&mut [&mut root]);
        // `kept`'s old offset now lies inside the slid survivor; the epoch
        // check refuses it before the read.
        let _ = heap.instance_field(kept, 0);
    }

    /// A move shifts every object by the same distance and the next move shifts
    /// it back, so every offset changes at every move.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_shifts_every_object_and_the_next_one_shifts_it_back() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let object = heap.allocate_instance(class, 1);
        let roots = [
            heap.stash(Value::object(class)),
            heap.stash(Value::object(object)),
        ];
        let keys = |heap: &Heap<'_>| -> Vec<usize> {
            roots
                .iter()
                .map(|root| heap.object_identity_key(heap.fetch(root).as_object().expect("rooted")))
                .collect()
        };
        let before = keys(&heap);
        move_bare_heap(&mut heap);
        let shifted = keys(&heap);
        assert!(shifted[0] > before[0]);
        assert_eq!(shifted[1] - shifted[0], before[1] - before[0]);
        move_bare_heap(&mut heap);
        assert_eq!(keys(&heap), before);
    }

    /// The growth policy in bytes (`docs/design/memory.md`, "The trigger and
    /// the growth policy"): a live set under the floor leaves the floor, one
    /// above it doubles.
    #[test]
    fn the_next_limit_is_twice_the_occupied_bytes_or_the_floor() {
        let mut heap = Heap::new();
        heap.collection_floor = 64;
        heap.reset_collection_limits();
        assert_eq!(heap.next_collection, 64);
        // Old, since the limit measures the old space alone.
        heap.set_allocating_old(true);
        let class = heap.allocate_self_referential(0);
        heap.allocate_instance(class, 10);
        heap.set_allocating_old(false);
        heap.reset_collection_limits();
        assert_eq!(heap.next_collection, 2 * 8 * (2 + 12));
    }

    /// Compaction gives committed memory back only past twice its next limit,
    /// and then no lower than the limit -- the space's hysteresis, in bytes.
    #[test]
    fn compaction_gives_memory_back_only_past_twice_the_limit() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.allocate_array(class, &vec![Value::nil(); 1 << 20]);
        let grown = heap.heap_statistics().committed_bytes;
        let mut root = Value::object(class);
        heap.collect_for_test(&mut [&mut root]);
        let after = heap.heap_statistics();
        assert!(after.committed_bytes < grown);
        assert!(after.committed_bytes >= space::NURSERY_END + heap.next_collection);
    }

    /// An old instance of `class` with `field_count` nil fields: large objects
    /// go old, so one past the threshold is old by construction.
    fn old_instance<'gc>(heap: &mut Heap<'gc>, class: ClassReference<'gc>) -> ObjectReference<'gc> {
        heap.allocate_instance(class, LARGE_OBJECT_BYTES / 8)
    }

    #[test]
    fn a_small_object_is_young_and_a_large_one_old_and_born_remembered() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let small = heap.allocate_instance(class, 2);
        let large = old_instance(&mut heap, class);
        assert!(space::is_young(small.offset));
        assert!(!space::is_young(large.offset));
        assert!(
            heap.header(large).remembered(),
            "it may be filled with young values"
        );
        assert_eq!(heap.remembered, vec![large.offset]);
    }

    #[test]
    fn every_store_of_a_young_value_into_an_old_object_remembers_it_once() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let young = heap.allocate_instance(class, 0);
        let holder = old_instance(&mut heap, class);
        heap.remembered.clear();
        heap.set_header(holder, heap.header(holder).with_remembered(false));

        assert!(heap.instance_set_field(holder, 0, Value::small_integer(1)));
        assert!(heap.remembered.is_empty(), "an immediate is never young");
        assert!(heap.instance_set_field(holder, 1, Value::object(young)));
        assert!(heap.instance_set_field(holder, 2, Value::object(young)));
        assert_eq!(heap.remembered, vec![holder.offset], "remembered once");
        assert!(heap.header(holder).remembered());
    }

    #[test]
    fn a_store_into_a_young_object_or_of_an_old_value_remembers_nothing() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let young = heap.allocate_instance(class, 1);
        let old = old_instance(&mut heap, class);
        heap.remembered.clear();
        heap.set_header(old, heap.header(old).with_remembered(false));
        assert!(heap.instance_set_field(young, 0, Value::object(old)));
        assert!(heap.instance_set_field(old, 0, Value::object(old)));
        assert!(heap.remembered.is_empty());
    }

    /// Takes `holder` out of the remembered set, as if nothing young had ever
    /// been stored into it.
    fn forget<'gc>(heap: &mut Heap<'gc>, holder: ObjectReference<'gc>) {
        heap.remembered.retain(|offset| *offset != holder.offset);
        heap.set_header(holder, heap.header(holder).with_remembered(false));
    }

    #[test]
    fn an_array_element_a_class_word_and_a_class_field_each_remember_their_old_holder() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let young = heap.allocate_instance(class, 0);

        let array = heap.allocate_array(class, &vec![Value::nil(); LARGE_OBJECT_BYTES / 8]);
        forget(&mut heap, array);
        assert!(heap.array_set_element(array, 0, Value::object(young)));
        assert!(heap.header(array).remembered());

        let young_class = named_class(&mut heap, "Young");
        assert!(space::is_young(young_class.offset));
        let instance = old_instance(&mut heap, class);
        forget(&mut heap, instance);
        assert!(heap.object_set_class(instance, young_class));
        assert!(heap.header(instance).remembered());

        heap.set_allocating_old(true);
        let old_class = named_class(&mut heap, "Old");
        heap.set_allocating_old(false);
        assert!(!heap.header(old_class).remembered());
        assert!(heap.class_set_superclass(old_class, young_class));
        assert!(heap.header(old_class).remembered());
    }

    #[test]
    fn a_dead_old_frame_s_register_and_a_divorce_into_an_old_frame_each_remember_it() {
        let mut heap = frame_heap();
        let class = heap.allocate_self_referential(0);
        let young = heap.allocate_instance(class, 0);

        // A frame is old when its register file makes it large.
        let dead = top_level_frame(&mut heap, LARGE_OBJECT_BYTES / 8);
        let dead_object = heap.frame_object(dead);
        assert!(!space::is_young(dead_object.offset));
        forget(&mut heap, dead_object);
        heap.frame_set_local(dead, first_above_header(0), Value::object(young));
        assert!(heap.header(dead_object).remembered());

        let activation = heap
            .push_activation(header(), Arguments::NONE, LARGE_OBJECT_BYTES / 8)
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        let frame_object = heap.frame_object(frame);
        assert!(!space::is_young(frame_object.offset));
        forget(&mut heap, frame_object);
        heap.retain_registers(activation);
        heap.activation_set_local(activation, first_above_header(0), Value::object(young));
        assert!(
            !heap.header(frame_object).remembered(),
            "a married frame's registers are the stack's, a root"
        );
        heap.pop_activation(activation);
        assert!(
            heap.header(frame_object).remembered(),
            "the divorce copied a young register in"
        );
    }

    #[test]
    fn the_decoder_s_allocations_go_old_and_are_not_remembered() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.set_allocating_old(true);
        let decoded = heap.allocate_placeholder(class, 2);
        heap.set_allocating_old(false);
        assert!(!space::is_young(decoded.offset));
        assert!(
            !heap.header(decoded).remembered(),
            "the load tenured everything it names"
        );
        assert!(space::is_young(heap.allocate_instance(class, 0).offset));
    }

    #[test]
    fn a_fast_bump_up_to_the_threshold_asks_for_nothing_and_the_next_asks_for_a_scavenge() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        // 32 words a block: the header, the class word and 30 fields.
        let fields = 30;
        let block_bytes = 8 * (fields + 2);
        let eden = heap.space.region(RegionName::Eden);
        while heap.space.region(RegionName::Eden).top + block_bytes
            < eden.start + heap.scavenge_threshold
        {
            heap.allocate_instance(class, fields);
            assert!(!heap.scavenge_requested);
        }
        heap.allocate_instance(class, fields);
        assert!(
            heap.scavenge_requested,
            "the allocation that reached the threshold asks"
        );
    }

    #[test]
    fn while_allocating_old_no_fast_bump_succeeds() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        heap.set_allocating_old(true);
        let decoded = heap.allocate_instance(class, 2);
        heap.set_allocating_old(false);
        assert!(!space::is_young(decoded.offset));
        assert!(!heap.header(decoded).remembered());
        assert!(space::is_young(heap.allocate_instance(class, 2).offset));
    }

    /// What `allocate` answers and how many fast bumps it made: the cold
    /// path bumps eden too and writes the same words, so only the count
    /// tells which of the two ran.
    fn counting_fast_bumps<'gc, T>(
        heap: &mut Heap<'gc>,
        allocate: impl FnOnce(&mut Heap<'gc>) -> T,
    ) -> (T, usize) {
        let before = heap.space.fast_bumps();
        let answer = allocate(heap);
        (answer, heap.space.fast_bumps() - before)
    }

    /// `counting_fast_bumps` with eden refusing every fast bump, as it does
    /// past the threshold: the cold path's object.
    fn counting_fast_bumps_refused<'gc, T>(
        heap: &mut Heap<'gc>,
        allocate: impl FnOnce(&mut Heap<'gc>) -> T,
    ) -> (T, usize) {
        heap.space.set_young_threshold(Some(0));
        let counted = counting_fast_bumps(heap, allocate);
        heap.space
            .set_young_threshold(Some(heap.scavenge_threshold));
        counted
    }

    #[test]
    fn a_fast_and_a_cold_allocation_write_the_same_words() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let (fast, fast_bumps) =
            counting_fast_bumps(&mut heap, |heap| heap.allocate_instance(class, 3));
        assert_eq!(fast_bumps, 1);
        assert!(space::is_young(fast.offset));
        // The same object made by the cold path: eden refused.
        let (cold, cold_bumps) =
            counting_fast_bumps_refused(&mut heap, |heap| heap.allocate_instance(class, 3));
        assert_eq!(cold_bumps, 0);
        assert!(
            space::is_young(cold.offset),
            "the cold path still bumps eden"
        );
        assert_eq!(heap.block(fast), heap.block(cold));
    }

    /// The guards a bare heap's `Closure` and `Activation` classes answer,
    /// recorded as `Mutator::refresh_immediate_dispatch_ids` records the
    /// real ones'.
    fn set_guards_from<'gc>(
        heap: &mut Heap<'gc>,
        closure: ClassReference<'gc>,
        frame: ClassReference<'gc>,
    ) {
        let guards = BuiltinGuards {
            closure: heap.header_dispatch_id(closure),
            frame: heap.header_dispatch_id(frame),
        };
        heap.set_builtin_guards(guards);
    }

    #[test]
    fn a_fast_float_is_the_cold_float() {
        let mut heap = frame_heap();
        let class = builtin_named_class(&mut heap, "Float", Some(BuiltinClass::Float));
        let activation = heap.activation_class.expect("frame_heap records one");
        set_guards_from(&mut heap, class, activation);
        let (fast, fast_bumps) =
            counting_fast_bumps(&mut heap, |heap| heap.allocate_float(class, 1e300));
        let (cold, cold_bumps) =
            counting_fast_bumps_refused(&mut heap, |heap| heap.allocate_float(class, 1e300));
        assert_eq!((fast_bumps, cold_bumps), (1, 0));
        assert!(space::is_young(fast.offset) && space::is_young(cold.offset));
        assert_eq!(heap.block(fast), heap.block(cold));
        assert_eq!(heap.float_value(Value::object(fast)), Some(1e300));
    }

    #[test]
    fn a_fast_closure_is_the_cold_closure() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Closure");
        let activation_class = heap.activation_class.expect("frame_heap records one");
        set_guards_from(&mut heap, class, activation_class);
        let captured_frame = top_level_frame(&mut heap, 0);
        let code = trivial_code();
        let activation = heap
            .push_activation(
                header_taking(2),
                Arguments::Values(&[Value::small_integer(7), Value::object(class)].as_slice()),
                2,
            )
            .expect("the stack holds the frame");
        let base = heap.activation_register_base(activation);
        let (fast, fast_bumps) = counting_fast_bumps(&mut heap, |heap| {
            heap.allocate_closure_from_registers(
                class,
                &code,
                captured_frame,
                FeedbackBase::NOT_ASSIGNED,
                base,
                &[1, 0],
            )
        });
        let (cold, cold_bumps) = counting_fast_bumps_refused(&mut heap, |heap| {
            heap.allocate_closure_from_registers(
                class,
                &code,
                captured_frame,
                FeedbackBase::NOT_ASSIGNED,
                base,
                &[1, 0],
            )
        });
        assert_eq!((fast_bumps, cold_bumps), (1, 0));
        assert_eq!(heap.block(fast), heap.block(cold));
        let new_closure = || NewClosure {
            code: code.clone(),
            captured_frame,
            copied: vec![Value::object(class), Value::small_integer(7)],
            feedback_base: FeedbackBase::NOT_ASSIGNED,
        };
        let (fast_listed, fast_bumps) = counting_fast_bumps(&mut heap, |heap| {
            heap.allocate_closure(class, new_closure())
        });
        let (cold_listed, cold_bumps) = counting_fast_bumps_refused(&mut heap, |heap| {
            heap.allocate_closure(class, new_closure())
        });
        assert_eq!((fast_bumps, cold_bumps), (1, 0));
        assert_eq!(heap.block(fast_listed), heap.block(cold_listed));
        assert_eq!(heap.block(fast), heap.block(fast_listed));
        for closure in [fast, cold, fast_listed, cold_listed] {
            assert!(space::is_young(closure.offset));
            let (body, captured, _) = heap.closure_activation(closure).expect("a closure");
            assert!(std::rc::Rc::ptr_eq(&body, &code));
            assert!(captured.same(captured_frame));
        }
        heap.pop_activation(activation);
    }

    #[test]
    fn a_fast_frame_is_the_cold_frame() {
        let mut heap = frame_heap();
        let activation_class = heap.activation_class.expect("frame_heap records one");
        let closure_class = named_class(&mut heap, "Closure");
        set_guards_from(&mut heap, closure_class, activation_class);
        let arguments = [Value::small_integer(1), Value::object(closure_class)];
        // One body for both, so that both frames name the same code.
        let code = shaped_code(2, first_above_header(2) + 3);
        let framed = || ActivationHeader {
            code: code.clone(),
            ..suspendable_header(closure_class, 2, 0)
        };
        let first = heap
            .push_activation(framed(), Arguments::Values(&arguments.as_slice()), 3)
            .expect("the stack holds the frame");
        let second = heap
            .push_activation(framed(), Arguments::Values(&arguments.as_slice()), 3)
            .expect("the stack holds the frame");
        let (fast, fast_bumps) = counting_fast_bumps(&mut heap, |heap| heap.reify(first));
        let (cold, cold_bumps) = counting_fast_bumps_refused(&mut heap, |heap| heap.reify(second));
        assert_eq!((fast_bumps, cold_bumps), (1, 0));
        let fast_object = heap.frame_object(fast);
        let cold_object = heap.frame_object(cold);
        assert!(space::is_young(fast_object.offset) && space::is_young(cold_object.offset));
        let married = FIELDS + frame_field::MARRIED;
        let without_married = |block: &[u64]| {
            let mut words = block.to_vec();
            words[married] = 0;
            words
        };
        assert_eq!(
            without_married(heap.block(fast_object)),
            without_married(heap.block(cold_object))
        );
        heap.pop_activation(second);
        heap.pop_activation(first);
    }

    #[test]
    fn a_closure_a_frame_and_a_float_allocated_while_allocating_old_go_old() {
        let mut heap = frame_heap();
        let float_class = builtin_named_class(&mut heap, "Float", Some(BuiltinClass::Float));
        let closure_class = named_class(&mut heap, "Closure");
        let activation_class = heap.activation_class.expect("frame_heap records one");
        set_guards_from(&mut heap, closure_class, activation_class);
        let captured_frame = top_level_frame(&mut heap, 0);
        heap.set_allocating_old(true);
        let bumps = heap.space.fast_bumps();
        let float = heap.allocate_float(float_class, 1e300);
        let closure = heap.allocate_closure(
            closure_class,
            NewClosure {
                code: trivial_code(),
                captured_frame,
                copied: vec![Value::small_integer(5)],
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            },
        );
        let frame = heap.allocate_placeholder_frame(2);
        let frame = heap.frame_object(frame);
        assert_eq!(heap.space.fast_bumps(), bumps, "no fast bump ran");
        heap.set_allocating_old(false);
        for object in [float, closure, frame] {
            assert!(!space::is_young(object.offset));
            assert!(!heap.header(object).remembered());
        }
    }

    #[test]
    fn an_object_of_254_words_takes_the_fast_path_and_one_of_255_takes_an_overflow_word() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let top = heap.space.region(RegionName::Eden).top;
        let largest = heap.allocate_instance(class, 253);
        assert_eq!(largest.offset.get() as usize, top, "no overflow word");
        let top = heap.space.region(RegionName::Eden).top;
        let overflowing = heap.allocate_instance(class, 254);
        assert_eq!(
            overflowing.offset.get() as usize,
            top + 8,
            "an overflow word first"
        );
    }

    #[test]
    fn an_allocation_that_finds_eden_spent_goes_old_born_remembered_and_asks_for_a_scavenge() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        // One young object just under the large-object threshold at a time:
        // its header, its overflow word, its class word and `fill` fields.
        let fill = LARGE_OBJECT_BYTES / 8 - 4;
        let allocation = 8 * space::allocation_words(fill + 1);
        assert!(allocation < LARGE_OBJECT_BYTES);
        while heap.space.region(RegionName::Eden).top + allocation
            <= space::NURSERY_END - space::REGION_SLACK
        {
            assert!(space::is_young(heap.allocate_instance(class, fill).offset));
        }
        assert!(
            heap.scavenge_requested,
            "eden passed its reserve long before it was spent"
        );
        let spill = heap.allocate_instance(class, fill);
        assert!(!space::is_young(spill.offset));
        assert!(heap.header(spill).remembered());
    }

    #[test]
    fn a_remembered_set_past_its_limit_asks_for_a_scavenge_that_tenures_everything() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let young = heap.allocate_instance(class, 0);
        // Small old objects, as the decoder makes them, so none is born
        // remembered: each joins the set through the barrier alone.
        heap.set_allocating_old(true);
        let holders: Vec<_> = (0..=REMEMBERED_SET_LIMIT)
            .map(|_| heap.allocate_instance(class, 1))
            .collect();
        heap.set_allocating_old(false);
        assert!(!heap.scavenge_requested);
        for holder in holders {
            heap.instance_set_field(holder, 0, Value::object(young));
        }
        assert_eq!(heap.remembered.len(), REMEMBERED_SET_LIMIT + 1);
        assert!(heap.scavenge_requested && heap.tenure_all_requested);
    }

    /// A scavenge that does not tenure everything rebuilds the remembered
    /// set from what still names a young object; past the limit, it asks
    /// for the tenuring scavenge as an overflowing barrier would.
    #[test]
    fn a_scavenge_that_leaves_the_remembered_set_past_its_limit_asks_for_one_that_tenures_everything(
    ) {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        // One young object every holder names: it stays young through the
        // scavenge, so every holder stays remembered.
        let young = heap.allocate_instance(class, 0);
        heap.set_allocating_old(true);
        let holders: Vec<_> = (0..=REMEMBERED_SET_LIMIT)
            .map(|_| heap.allocate_instance(class, 1))
            .collect();
        heap.set_allocating_old(false);
        for holder in holders {
            heap.instance_set_field(holder, 0, Value::object(young));
        }
        scavenge_bare(&mut heap, false, &mut []);
        assert_eq!(heap.young_objects, 1, "the young object stayed young");
        assert_eq!(heap.remembered.len(), REMEMBERED_SET_LIMIT + 1);
        assert!(heap.scavenge_requested && heap.tenure_all_requested);
        scavenge_bare(&mut heap, true, &mut []);
        assert!(heap.remembered.is_empty());
        assert!(!heap.scavenge_requested && !heap.tenure_all_requested);
    }

    /// Scavenges a bare heap with `roots` as the step state's roots.
    fn scavenge_bare<'gc>(heap: &mut Heap<'gc>, tenure_all: bool, roots: &mut [&mut Value<'gc>]) {
        use crate::trace::Tracer;
        heap.scavenge(tenure_all, None, |scavenger| {
            for root in roots.iter_mut() {
                scavenger.value(root);
            }
        });
    }

    /// A class object that is its own class, allocated old, so that the
    /// young objects of a test name an old class unless the test means
    /// otherwise.
    fn old_instance_class<'gc>(heap: &mut Heap<'gc>) -> ClassReference<'gc> {
        heap.set_allocating_old(true);
        let class = heap.allocate_self_referential(0);
        heap.set_allocating_old(false);
        class
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "missed the write barrier")]
    fn the_verification_finds_an_old_object_naming_a_young_one_unremembered() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let young = heap.allocate_instance(class, 0);
        let holder = old_instance(&mut heap, class);
        heap.instance_set_field(holder, 0, Value::object(young));
        // What a store that bypassed the barrier would leave behind.
        heap.remembered.clear();
        heap.set_header(holder, heap.header(holder).with_remembered(false));
        heap.verify_remembered_set();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "the remembered set lacks its offset")]
    fn the_verification_finds_a_remembered_set_entry_at_a_wrong_offset() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let young = heap.allocate_instance(class, 0);
        let holder = old_instance(&mut heap, class);
        let other = old_instance(&mut heap, class);
        heap.instance_set_field(holder, 0, Value::object(young));
        // One entry and one bit, as the counts need, but the entry names
        // another old object than the one carrying the bit.
        heap.set_header(other, heap.header(other).with_remembered(false));
        heap.remembered.clear();
        heap.remembered.push(other.offset);
        heap.set_header(holder, heap.header(holder).with_remembered(true));
        let bits = [holder, other]
            .into_iter()
            .filter(|object| heap.header(*object).remembered())
            .count();
        assert_eq!(bits, heap.remembered.len(), "the counts agree");
        heap.verify_remembered_set();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "vacated nursery")]
    fn a_word_left_naming_a_reclaimed_young_object_reads_the_poison() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let young = heap.allocate_instance(class, 0);
        let holder = old_instance(&mut heap, class);
        // The barrier is bypassed by writing the word as a missed store would.
        let word = heap.store(Value::object(young));
        heap.fields_mut(holder)[0] = word.bits();
        heap.remembered.clear();
        heap.set_header(holder, heap.header(holder).with_remembered(false));
        // The holder is old, so the scavenge only carries its handle into
        // the next epoch; the word it holds is left as the missed store
        // left it.
        let mut holder_root = Value::object(holder);
        scavenge_bare(&mut heap, false, &mut [&mut holder_root]);
        let holder = rooted(holder_root);
        let stale = heap.load(Word::from_bits(heap.fields(holder)[0]));
        let _ = stale.as_object().map(|object| heap.object_class(object));
    }

    /// The object a root or a field holds after a scavenge.
    fn rooted(value: Value<'_>) -> ObjectReference<'_> {
        let Some(object) = value.as_object() else {
            panic!("the value no longer holds an object")
        };
        object
    }

    #[test]
    fn a_scavenge_copies_what_a_root_reaches_and_reclaims_eden() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        heap.allocate_string(class, crate::CodePoints::from("dead"));
        let mut root = Value::object(kept);
        let before = heap.heap_statistics();
        let young_before = heap.young_objects;
        let kept_offset = kept.offset;

        scavenge_bare(&mut heap, false, &mut [&mut root]);

        let moved = rooted(root);
        assert_ne!(moved.offset, kept_offset, "the survivor was copied");
        assert!(
            space::is_young(moved.offset),
            "age 1 is below the starting threshold"
        );
        assert_eq!(heap.str_value(moved), Some("kept".to_string()));
        assert_eq!(heap.header(moved).age(), 1);
        let after = heap.heap_statistics();
        assert_eq!(young_before, 2);
        assert_eq!(heap.young_objects, 1);
        assert_eq!(after.live_objects, before.live_objects - 1);
        assert_eq!(
            heap.space.region(RegionName::Eden).top,
            heap.space.region(RegionName::Eden).base
        );
        assert_eq!(heap.scavenges, 1);
    }

    #[test]
    fn an_object_reachable_only_through_the_remembered_set_survives_until_it_is_tenured() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let holder = old_instance(&mut heap, class);
        let string = heap.allocate_string(class, crate::CodePoints::from("kept"));
        assert!(heap.instance_set_field(holder, 0, Value::object(string)));
        let mut holder_root = Value::object(holder);
        let mut tenured_at = None;
        for scavenge in 1..=16 {
            scavenge_bare(&mut heap, false, &mut [&mut holder_root]);
            let holder = rooted(holder_root);
            let string = rooted(heap.instance_field(holder, 0).unwrap_or_else(Value::nil));
            assert_eq!(heap.str_value(string), Some("kept".to_string()));
            if space::is_young(string.offset) {
                assert!(tenured_at.is_none(), "a tenured object stays old");
                assert!(heap.header(holder).remembered());
                assert_eq!(heap.remembered, vec![holder.offset]);
            } else {
                tenured_at.get_or_insert(scavenge);
                assert!(!heap.header(holder).remembered());
                assert!(heap.remembered.is_empty());
            }
        }
        assert_eq!(
            tenured_at,
            Some(16),
            "age 15 reaches the starting threshold at the sixteenth scavenge"
        );
    }

    #[test]
    fn a_promoted_object_naming_a_survivor_is_remembered_by_the_scavenge_that_promoted_it() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let array = heap.allocate_array(class, &[Value::nil()]);
        let mut root = Value::object(array);
        scavenge_bare(&mut heap, false, &mut [&mut root]);
        let array = rooted(root);
        assert_eq!(heap.header(array).age(), 1);
        let class = heap.object_class(array);
        let string = heap.allocate_string(class, crate::CodePoints::from("survivor"));
        assert!(heap.array_set_element(array, 0, Value::object(string)));
        assert!(
            heap.remembered.is_empty(),
            "a young holder is never remembered"
        );

        heap.tenuring_threshold = 1;
        scavenge_bare(&mut heap, false, &mut [&mut root]);
        let array = rooted(root);
        assert!(!space::is_young(array.offset), "age 1 reaches threshold 1");
        let string = rooted(heap.array_element(array, 0).unwrap_or_else(Value::nil));
        assert!(space::is_young(string.offset), "age 0 is below it");
        assert!(heap.header(array).remembered());
        assert_eq!(heap.remembered, vec![array.offset]);

        scavenge_bare(&mut heap, false, &mut [&mut root]);
        let array = rooted(root);
        let string = rooted(heap.array_element(array, 0).unwrap_or_else(Value::nil));
        assert_eq!(heap.str_value(string), Some("survivor".to_string()));
    }

    #[test]
    fn more_survivors_than_a_survivor_space_holds_are_promoted_and_nothing_is_lost() {
        const COUNT: usize = 300;
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        heap.set_allocating_old(true);
        let holder = heap.allocate_array(class, &vec![Value::nil(); COUNT]);
        heap.set_allocating_old(false);
        for index in 0..COUNT {
            let mut elements = vec![Value::nil(); 512];
            elements[0] = count_value(index);
            let array = heap.allocate_array(class, &elements);
            assert!(space::is_young(array.offset));
            assert!(heap.array_set_element(holder, index, Value::object(array)));
        }
        let mut root = Value::object(holder);

        scavenge_bare(&mut heap, false, &mut [&mut root]);

        let holder = rooted(root);
        let (mut young, mut old) = (0, 0);
        for index in 0..COUNT {
            let array = rooted(heap.array_element(holder, index).unwrap_or_else(Value::nil));
            assert_eq!(
                heap.array_element(array, 0)
                    .and_then(|element| element.as_small_integer()),
                count_value(index).as_small_integer()
            );
            assert_eq!(heap.array_element_count(array), Some(512));
            if space::is_young(array.offset) {
                young += 1;
            } else {
                old += 1;
            }
        }
        assert!(young > 0 && old > 0, "{young} young, {old} old");
        assert_eq!(heap.young_objects, young);
        assert!(heap.header(holder).remembered());
    }

    #[test]
    fn a_young_class_is_copied_with_its_instances() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        assert!(space::is_young(class.offset));
        let class_offset = class.offset;
        let instance = heap.allocate_instance(class, 1);
        let mut root = Value::object(instance);

        scavenge_bare(&mut heap, false, &mut [&mut root]);

        let instance = rooted(root);
        let moved_class = heap.object_class(instance);
        assert_ne!(moved_class.offset, class_offset, "the class was copied");
        assert!(space::is_young(moved_class.offset));
        assert!(
            heap.object_class(moved_class).same(moved_class),
            "the copy is its own class"
        );

        let mut class_root = Value::object(moved_class);
        scavenge_bare(&mut heap, false, &mut [&mut root, &mut class_root]);
        let instance = rooted(root);
        assert!(heap.object_class(instance).same(rooted(class_root)));
    }

    #[test]
    fn a_married_frame_is_copied_and_its_entry_follows_it() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Scavenged");
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, first_above_header(1) + 1),
                Arguments::Values(&[Value::small_integer(1)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        let before = heap.frame_object(frame).offset;
        assert!(space::is_young(before));

        scavenge_bare(&mut heap, false, &mut []);

        let Some(frame) = heap.activation_reified(activation) else {
            panic!("the activation is still reified")
        };
        assert_ne!(
            heap.frame_object(frame).offset,
            before,
            "the frame was copied"
        );
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(1));
        let temporary = first_above_header(1);
        heap.frame_set_local(frame, temporary, Value::small_integer(7));
        assert_eq!(
            heap.activation_local(activation, temporary)
                .as_small_integer(),
            Some(7)
        );
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(activation)));
        heap.retain_registers(activation);
        heap.pop_activation(activation);
        assert!(heap.frame_activation(frame).is_none());
        assert_eq!(
            heap.frame_local(frame, temporary).as_small_integer(),
            Some(7)
        );
    }

    #[test]
    fn the_divorce_into_a_tenured_frame_keeps_its_young_registers() {
        let mut heap = frame_heap();
        let activation = heap
            .push_activation(header(), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        heap.reify(activation);

        scavenge_bare(&mut heap, true, &mut []);

        let Some(mut frame) = heap.activation_reified(activation) else {
            panic!("the activation is still reified")
        };
        assert!(!space::is_young(heap.frame_object(frame).offset));
        let class = old_instance_class(&mut heap);
        let string = heap.allocate_string(class, crate::CodePoints::from("register"));
        assert!(space::is_young(string.offset));
        heap.activation_set_local(activation, first_above_header(0), Value::object(string));
        heap.retain_registers(activation);
        heap.pop_activation(activation);
        assert!(heap.header(heap.frame_object(frame)).remembered());

        heap.scavenge(false, None, |scavenger| {
            use crate::trace::Tracer;
            scavenger.frame(&mut frame);
        });

        let string = rooted(heap.frame_local(frame, first_above_header(0)));
        assert!(space::is_young(string.offset));
        assert_eq!(heap.str_value(string), Some("register".to_string()));
    }

    #[test]
    fn a_symbol_held_only_by_young_garbage_is_pruned_and_a_kept_one_keeps_its_identity() {
        let mut heap = Heap::new();
        heap.set_allocating_old(true);
        let symbol_class = builtin_named_class(&mut heap, "Symbol", Some(BuiltinClass::Symbol));
        let string_class = named_class(&mut heap, "String");
        heap.set_allocating_old(false);
        let kept_symbol = Symbol::intern("kept:young:symbol:");
        let pruned_symbol = Symbol::intern("pruned:young:symbol:");
        let mut kept = Value::object(canonical_symbol_for_test(
            &mut heap,
            symbol_class,
            kept_symbol,
        ));
        let pruned = canonical_symbol_for_test(&mut heap, symbol_class, pruned_symbol);
        assert!(space::is_young(pruned.offset));
        let mut string = Value::object(
            heap.allocate_string(string_class, crate::CodePoints::from("kept:young:symbol:")),
        );
        let offset_before = rooted(kept).offset;

        scavenge_bare(&mut heap, false, &mut [&mut kept, &mut string]);

        let object = rooted(kept);
        assert_ne!(
            object.offset, offset_before,
            "the scavenge copied the symbol"
        );
        assert_eq!(heap.symbol_value(object), Some(kept_symbol));
        assert!(heap
            .canonical_symbol(kept_symbol)
            .is_some_and(|canonical| canonical.same(object)));
        assert_eq!(heap.canonical_symbol(pruned_symbol).map(|_| ()), None);
        let live = heap.canonical_symbols.iter().flatten().count();
        assert_eq!(heap.symbol_of.len(), live);
        assert_eq!(live, 1);
        let string = rooted(string);
        assert_eq!(heap.symbol_value(string), None);
        #[cfg(debug_assertions)]
        {
            let roots = [
                heap.stash(Value::object(object)),
                heap.stash(Value::object(string)),
            ];
            move_bare_heap(&mut heap);
            let object = rooted(heap.fetch(&roots[0]));
            let string = rooted(heap.fetch(&roots[1]));
            assert_eq!(heap.symbol_value(object), Some(kept_symbol));
            assert_eq!(heap.symbol_value(string), None);
            assert_eq!(heap.symbol_of.len(), 1);
        }
    }

    /// Asserts the remembered set's invariant where a debug build can.
    fn verify_remembered_set_if_debug(heap: &Heap<'_>) {
        #[cfg(debug_assertions)]
        heap.verify_remembered_set();
        #[cfg(not(debug_assertions))]
        let _ = heap;
    }

    /// The text of the young string `value` names, read after a scavenge:
    /// a word left naming the vacated nursery reads poison in a debug
    /// build, not this text.
    fn text_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<String> {
        value.as_object().and_then(|object| heap.str_value(object))
    }

    #[test]
    fn a_young_weak_array_forgets_a_young_object_and_follows_a_surviving_one() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        let forgotten = heap.allocate_string(class, crate::CodePoints::from("forgotten"));
        let weak = heap.allocate_weak_array(class, 3);
        assert!(space::is_young(weak.offset));
        let elements = [
            Value::object(kept),
            Value::object(forgotten),
            Value::small_integer(7),
        ];
        for (index, element) in elements.into_iter().enumerate() {
            assert!(heap.weak_array_set_element(weak, index, element));
        }
        let kept_offset = kept.offset;
        let mut weak_root = Value::object(weak);
        let mut kept_root = Value::object(kept);

        scavenge_bare(&mut heap, false, &mut [&mut weak_root, &mut kept_root]);

        let kept = rooted(kept_root);
        assert_ne!(kept.offset, kept_offset, "the scavenge copied the object");
        let elements = heap
            .weak_array_elements(rooted(weak_root))
            .expect("a weak array answers its elements");
        assert!(names(elements[0], kept), "the word follows its object");
        assert_eq!(text_of(&heap, elements[0]).as_deref(), Some("kept"));
        assert!(
            elements[1].is_nil(),
            "an object only the array held is gone"
        );
        assert_eq!(elements[2].as_small_integer(), Some(7));
        verify_remembered_set_if_debug(&heap);
    }

    #[test]
    fn an_old_weak_array_forgets_a_young_object_and_follows_a_surviving_one() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        heap.set_allocating_old(true);
        let weak = heap.allocate_weak_array(class, 2);
        heap.set_allocating_old(false);
        assert!(!space::is_young(weak.offset));
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        let forgotten = heap.allocate_string(class, crate::CodePoints::from("forgotten"));
        assert!(heap.weak_array_set_element(weak, 0, Value::object(kept)));
        assert!(heap.weak_array_set_element(weak, 1, Value::object(forgotten)));
        assert!(heap.remembered.contains(&weak.offset));
        let mut weak_root = Value::object(weak);
        let mut kept_root = Value::object(kept);

        scavenge_bare(&mut heap, false, &mut [&mut weak_root, &mut kept_root]);

        let weak = rooted(weak_root);
        let kept = rooted(kept_root);
        assert!(space::is_young(kept.offset), "the object stayed young");
        let elements = heap
            .weak_array_elements(weak)
            .expect("a weak array answers its elements");
        assert!(names(elements[0], kept), "the word follows its object");
        assert_eq!(text_of(&heap, elements[0]).as_deref(), Some("kept"));
        assert!(elements[1].is_nil(), "the unreached object is gone");
        assert_eq!(
            heap.remembered
                .iter()
                .filter(|entry| **entry == weak.offset)
                .count(),
            1,
            "it still names a young object, so it stays remembered, once"
        );
        verify_remembered_set_if_debug(&heap);

        scavenge_bare(&mut heap, true, &mut [&mut weak_root, &mut kept_root]);

        let weak = rooted(weak_root);
        let kept = rooted(kept_root);
        assert!(!space::is_young(kept.offset), "the object was tenured");
        let element = heap
            .weak_array_element(weak, 0)
            .expect("a weak array answers its slot");
        assert!(names(element, kept), "the word follows its tenured object");
        assert!(
            !heap.remembered.contains(&weak.offset),
            "it names no young object any more"
        );
        assert!(!heap.header(weak).remembered());
        verify_remembered_set_if_debug(&heap);
    }

    #[test]
    fn a_young_ephemeron_whose_key_survives_keeps_its_value() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let key = heap.allocate_string(class, crate::CodePoints::from("key"));
        let value = heap.allocate_string(class, crate::CodePoints::from("value"));
        let ephemeron = heap.allocate_ephemeron(class, 3);
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        // The key is reached only through the holder, which the Cheney scan
        // reaches after the ephemeron: the ephemeron is listed with a key
        // not yet copied, and only `finish`'s fixpoint keeps its value.
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(key)));
        let mut array_root = Value::object(
            heap.allocate_array(class, &[Value::object(ephemeron), Value::object(holder)]),
        );

        scavenge_bare(&mut heap, false, &mut [&mut array_root]);

        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let key = heap
            .instance_field(rooted(elements[1]), 0)
            .expect("the holder holds the key");
        assert_eq!(text_of(&heap, key).as_deref(), Some("key"));
        let fields = heap
            .ephemeron_fields(rooted(elements[0]))
            .expect("an ephemeron answers its fields");
        assert!(names(fields[0], rooted(key)));
        assert_eq!(text_of(&heap, fields[1]).as_deref(), Some("value"));
        assert!(fields[2].is_nil());
        verify_remembered_set_if_debug(&heap);
    }

    /// An old ephemeron in the remembered set is scanned before the Cheney
    /// loop copies its young key, which only a young holder reaches: it is
    /// listed, and once the key survives, the rescan keeps its young value
    /// and remembers it -- once, though one of a young class was remembered
    /// already for its class word.
    #[test]
    fn an_old_ephemeron_whose_young_key_survives_later_keeps_its_value_and_is_remembered_once() {
        for class_is_young in [false, true] {
            let mut heap = Heap::new();
            let old_class = old_instance_class(&mut heap);
            let class = if class_is_young {
                heap.allocate_self_referential(0)
            } else {
                old_class
            };
            heap.set_allocating_old(true);
            let ephemeron = heap.allocate_ephemeron(class, 2);
            heap.set_allocating_old(false);
            let key = heap.allocate_string(old_class, crate::CodePoints::from("key"));
            let value = heap.allocate_string(old_class, crate::CodePoints::from("value"));
            assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
            let holder = heap.allocate_instance(old_class, 1);
            assert!(heap.instance_set_field(holder, 0, Value::object(key)));
            let mut ephemeron_root = Value::object(ephemeron);
            let mut holder_root = Value::object(holder);

            scavenge_bare(
                &mut heap,
                false,
                &mut [&mut ephemeron_root, &mut holder_root],
            );

            let ephemeron = rooted(ephemeron_root);
            let key = heap
                .instance_field(rooted(holder_root), 0)
                .expect("the holder holds the key");
            let fields = heap
                .ephemeron_fields(ephemeron)
                .expect("an ephemeron answers its fields");
            assert!(names(fields[0], rooted(key)));
            assert_eq!(text_of(&heap, fields[1]).as_deref(), Some("value"));
            assert_eq!(
                heap.remembered
                    .iter()
                    .filter(|entry| **entry == ephemeron.offset)
                    .count(),
                1,
                "it names a young value, so it is remembered, once"
            );
            verify_remembered_set_if_debug(&heap);
        }
    }

    /// A weak array and an ephemeron promoted by age, not by `tenure_all`,
    /// still name a young survivor afterwards: each is remembered, once,
    /// and its words name the copies.
    #[test]
    fn a_weak_object_promoted_by_age_that_names_a_young_survivor_is_remembered() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let weak = heap.allocate_weak_array(class, 1);
        let ephemeron = heap.allocate_ephemeron(class, 2);
        let mut weak_root = Value::object(weak);
        let mut ephemeron_root = Value::object(ephemeron);
        // The first scavenge ages both to 1; the second promotes them.
        scavenge_bare(&mut heap, false, &mut [&mut weak_root, &mut ephemeron_root]);
        let (weak, ephemeron) = (rooted(weak_root), rooted(ephemeron_root));
        assert!(space::is_young(weak.offset) && space::is_young(ephemeron.offset));
        let class = heap.object_class(weak);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        let value = heap.allocate_string(class, crate::CodePoints::from("value"));
        assert!(heap.weak_array_set_element(weak, 0, Value::object(kept)));
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(kept)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        let mut kept_root = Value::object(kept);
        heap.tenuring_threshold = 1;

        scavenge_bare(
            &mut heap,
            false,
            &mut [&mut weak_root, &mut ephemeron_root, &mut kept_root],
        );

        let (weak, ephemeron, kept) =
            (rooted(weak_root), rooted(ephemeron_root), rooted(kept_root));
        assert!(!space::is_young(weak.offset) && !space::is_young(ephemeron.offset));
        assert!(
            space::is_young(kept.offset),
            "the fresh object stayed young"
        );
        let element = heap
            .weak_array_element(weak, 0)
            .expect("a weak array answers its slot");
        assert!(names(element, kept), "the word names the copy");
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron answers its fields");
        assert!(names(fields[0], kept));
        assert_eq!(text_of(&heap, fields[1]).as_deref(), Some("value"));
        for promoted in [weak, ephemeron] {
            assert_eq!(
                heap.remembered
                    .iter()
                    .filter(|entry| **entry == promoted.offset)
                    .count(),
                1,
                "a promoted object naming a young one is remembered, once"
            );
        }
        verify_remembered_set_if_debug(&heap);
    }

    #[test]
    fn a_young_ephemeron_whose_key_dies_is_mourned() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let ephemeron = heap.allocate_ephemeron(class, 3);
        for (index, text) in ["key", "value", "finalizer"].into_iter().enumerate() {
            let field = heap.allocate_string(class, crate::CodePoints::from(text));
            assert!(heap.ephemeron_set_field(ephemeron, index, Value::object(field)));
        }
        let mut ephemeron_root = Value::object(ephemeron);

        scavenge_bare(&mut heap, false, &mut [&mut ephemeron_root]);

        let fields = heap
            .ephemeron_fields(rooted(ephemeron_root))
            .expect("an ephemeron answers its fields");
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().all(|field| field.is_nil()));
        verify_remembered_set_if_debug(&heap);
    }

    #[test]
    fn an_old_ephemeron_with_an_old_key_keeps_a_young_value() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let key = old_instance(&mut heap, class);
        heap.set_allocating_old(true);
        let ephemeron = heap.allocate_ephemeron(class, 3);
        heap.set_allocating_old(false);
        let value = heap.allocate_string(class, crate::CodePoints::from("value"));
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
        assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
        let mut ephemeron_root = Value::object(ephemeron);
        let mut key_root = Value::object(key);

        scavenge_bare(&mut heap, false, &mut [&mut ephemeron_root, &mut key_root]);

        let ephemeron = rooted(ephemeron_root);
        let fields = heap
            .ephemeron_fields(ephemeron)
            .expect("an ephemeron answers its fields");
        assert!(names(fields[0], rooted(key_root)));
        assert_eq!(text_of(&heap, fields[1]).as_deref(), Some("value"));
        assert!(heap.remembered.contains(&ephemeron.offset));
        verify_remembered_set_if_debug(&heap);
    }

    /// Task 1's chain, all young: while the first key is held by nothing,
    /// the whole chain is mourned; once it is rooted, every value survives.
    /// The array holds the chain last link first, so the Cheney scan lists
    /// every ephemeron but the first before its key is copied, and the
    /// fixpoint reaches link after link.
    #[test]
    fn a_chain_of_young_ephemerons_dies_whole_in_a_scavenge() {
        scavenge_a_chain_of_ephemerons(512);
    }

    /// The chain at a length whose square a rescan per round would make
    /// obvious: the fixpoint looks each object it copies up once and
    /// scans each ephemeron once more.
    #[test]
    fn a_long_chain_of_young_ephemerons_costs_its_length_in_a_scavenge() {
        const LENGTH: usize = 20_000;
        let [unrooted, rooted_work] = scavenge_a_chain_of_ephemerons(LENGTH);
        assert_eq!(
            (unrooted.lookups, unrooted.resolutions),
            (0, 0),
            "nothing is copied, so nothing is looked up"
        );
        for work in [unrooted, rooted_work] {
            assert!(
                work.examined <= (pending::SCANNED_CHECKS + 1) * LENGTH,
                "{} ephemerons examined for {LENGTH} links",
                work.examined
            );
        }
        let pending::work::Work {
            lookups,
            resolutions,
            ..
        } = rooted_work;
        assert!(
            lookups <= 2 * LENGTH,
            "{lookups} lookups for {LENGTH} links"
        );
        assert!(
            resolutions <= LENGTH,
            "{resolutions} resolutions for {LENGTH} links"
        );
    }

    /// A weak map's shape, all young: every key reachable, but every
    /// ephemeron scanned before its key is copied. The first check
    /// resolves every listed ephemeron, so the index stays empty and the
    /// fixpoint records and looks up nothing.
    #[test]
    fn ephemerons_whose_keys_are_all_reachable_are_resolved_without_lookups_in_a_scavenge() {
        const COUNT: usize = 1_000;
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let mut keys = Vec::with_capacity(COUNT);
        let mut elements = Vec::with_capacity(COUNT + 1);
        for _ in 0..COUNT {
            let key = heap.allocate_instance(class, 0);
            let value = heap.allocate_instance(class, 0);
            let ephemeron = heap.allocate_ephemeron(class, 2);
            assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(key)));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
            keys.push(Value::object(key));
            elements.push(Value::object(ephemeron));
        }
        // The Cheney scan reaches every ephemeron before the array of
        // keys, after them, copies a key.
        let holder = heap.allocate_array(class, &keys);
        elements.push(Value::object(holder));
        let mut array_root = Value::object(heap.allocate_array(class, &elements));
        pending::work::take();

        scavenge_bare(&mut heap, false, &mut [&mut array_root]);

        let work = pending::work::take();
        assert_eq!(work.examined, COUNT, "each ephemeron is listed once");
        assert_eq!(
            (work.recorded, work.lookups, work.resolutions),
            (0, 0, 0),
            "nothing is pending, so nothing is recorded or looked up"
        );
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        for ephemeron in &elements[..COUNT] {
            let fields = heap
                .ephemeron_fields(rooted(*ephemeron))
                .expect("an ephemeron answers its fields");
            let [key, value] =
                [fields[0], fields[1]].map(|field| heap.instance_field_count(rooted(field)));
            assert!(
                key == Some(0) && value == Some(0),
                "the ephemeron keeps its key and its value"
            );
        }
        verify_remembered_set_if_debug(&heap);
    }

    /// A scan copies as it resolves, and so may the check at the switch:
    /// the only scan passes over `E_x` and `E_y` and resolves `E_z`, whose
    /// value `K_y` it copies; the switch indexes `E_x`, then resolves `E_y`,
    /// whose value is `K_x`. That copy must be recorded, or no lookup ever
    /// names `K_x` and `E_x` is mourned.
    #[test]
    fn a_key_the_check_at_the_switch_copies_is_recorded_in_a_scavenge() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let [key_x, value_x, key_y, key_z] = [0; 4].map(|_| heap.allocate_instance(class, 0));
        let holder_z = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder_z, 0, Value::object(key_z)));
        let ephemeron_x = keyed_ephemeron(&mut heap, class, key_x, value_x);
        let ephemeron_y = keyed_ephemeron(&mut heap, class, key_y, key_x);
        let ephemeron_z = keyed_ephemeron(&mut heap, class, key_z, key_y);
        // The Cheney scan reaches `E_x`, `E_y` and `E_z`, then `H_z`, which
        // copies `K_z`.
        let mut array_root = Value::object(heap.allocate_array(
            class,
            &[
                Value::object(ephemeron_x),
                Value::object(ephemeron_y),
                Value::object(ephemeron_z),
                Value::object(holder_z),
            ],
        ));
        pending::work::set_scanned_checks(1);
        pending::work::take();

        scavenge_bare(&mut heap, false, &mut [&mut array_root]);

        let resolutions = pending::work::take().resolutions;
        assert_eq!(resolutions, 1, "E_x through K_x's lookup");
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let fields = heap
            .ephemeron_fields(rooted(elements[0]))
            .expect("an ephemeron answers its fields");
        assert!(
            fields.iter().all(|field| field.as_object().is_some()),
            "E_x keeps K_x and v_x"
        );
        verify_remembered_set_if_debug(&heap);
    }

    /// A weak map with dead keys, all young: half the ephemerons keyed by
    /// objects only the array after them reaches, half by objects nothing
    /// else does. The first check resolves the live half, the second finds
    /// the dead half's keys still not copied and nothing waiting to be
    /// scanned: the fixpoint ends within its scans, so it indexes,
    /// records and looks up nothing, and mourns the dead half.
    #[test]
    fn a_weak_maps_dead_keys_are_mourned_without_an_index_in_a_scavenge() {
        const COUNT: usize = 1_000;
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let mut keys = Vec::with_capacity(COUNT);
        let mut elements = Vec::with_capacity(2 * COUNT + 1);
        for live in [true, false] {
            for _ in 0..COUNT {
                let key = heap.allocate_instance(class, 0);
                let value = heap.allocate_instance(class, 0);
                elements.push(Value::object(keyed_ephemeron(&mut heap, class, key, value)));
                if live {
                    keys.push(Value::object(key));
                }
            }
        }
        // The Cheney scan reaches every ephemeron before the array of the
        // live keys, after them, copies a key.
        let holder = heap.allocate_array(class, &keys);
        elements.push(Value::object(holder));
        let mut array_root = Value::object(heap.allocate_array(class, &elements));
        pending::work::take();

        scavenge_bare(&mut heap, false, &mut [&mut array_root]);

        let work = pending::work::take();
        assert_eq!(
            work.examined,
            3 * COUNT,
            "the first check examines every ephemeron, the second the dead half"
        );
        assert_eq!(
            (work.recorded, work.lookups, work.resolutions),
            (0, 0, 0),
            "nothing is indexed, so nothing is recorded or looked up"
        );
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        for (position, ephemeron) in elements[..2 * COUNT].iter().enumerate() {
            let fields = heap
                .ephemeron_fields(rooted(*ephemeron))
                .expect("an ephemeron answers its fields");
            if position < COUNT {
                let [key, value] =
                    [fields[0], fields[1]].map(|field| heap.instance_field_count(rooted(field)));
                assert!(
                    key == Some(0) && value == Some(0),
                    "a live entry keeps its key and its value"
                );
            } else {
                assert!(
                    fields.iter().all(|field| field.is_nil()),
                    "a dead entry is mourned"
                );
            }
        }
        verify_remembered_set_if_debug(&heap);
    }

    /// A scan copies as it resolves: the last scan passes over `E_x`, its
    /// key `K_x` not yet copied, then resolves `E_y`, whose value is
    /// `K_x`. Nothing records that copy, so the switch to the index must
    /// check `E_x` again, or it indexes `E_x` under a key no lookup will
    /// ever name and mourns it.
    #[test]
    fn an_ephemeron_whose_key_the_last_scan_copies_after_it_is_kept_in_a_scavenge() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let [key_x, value_x, key_y] = [0; 3].map(|_| heap.allocate_instance(class, 0));
        let holder_y = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder_y, 0, Value::object(key_y)));
        let ephemeron_x = keyed_ephemeron(&mut heap, class, key_x, value_x);
        let ephemeron_y = keyed_ephemeron(&mut heap, class, key_y, key_x);
        // The Cheney scan reaches `E_x`, then `E_y`, then `H_y`, which
        // copies `K_y`.
        let mut array_root = Value::object(heap.allocate_array(
            class,
            &[
                Value::object(ephemeron_x),
                Value::object(ephemeron_y),
                Value::object(holder_y),
            ],
        ));
        pending::work::set_scanned_checks(1);
        pending::work::take();

        scavenge_bare(&mut heap, false, &mut [&mut array_root]);

        let work = pending::work::take();
        assert_eq!(
            (work.recorded, work.lookups),
            (0, 0),
            "the check at the switch resolves E_x, so nothing is indexed"
        );
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let fields = heap
            .ephemeron_fields(rooted(elements[0]))
            .expect("an ephemeron answers its fields");
        assert!(
            fields.iter().all(|field| field.as_object().is_some()),
            "E_x keeps K_x and v_x"
        );
        verify_remembered_set_if_debug(&heap);
    }

    /// The index empties and fills again, all young: the first round
    /// resolves `E_a` with nothing pending, so its drain records nothing
    /// and lists `E_b` and `E_c`; the second indexes `E_b`, and resolving
    /// `E_c` copies `H`, so the drain after that check, which copies
    /// `K_b`, must record, or `E_b` is mourned.
    ///
    /// Run indexed from the first check on, as above, and with the
    /// shipped scans, after which the switch checks `E_b` again and
    /// indexes it before the drain that copies `K_b` records.
    #[test]
    fn an_ephemeron_indexed_after_the_index_emptied_is_kept_in_a_scavenge() {
        for scanned_checks in [0, pending::SCANNED_CHECKS] {
            let mut heap = Heap::new();
            let class = old_instance_class(&mut heap);
            let [key_a, key_b, key_c, value_b] = [0; 4].map(|_| heap.allocate_instance(class, 0));
            let [holder_a, holder_b, holder_c] = [key_a, key_b, key_c].map(|key| {
                let holder = heap.allocate_instance(class, 1);
                assert!(heap.instance_set_field(holder, 0, Value::object(key)));
                holder
            });
            let ephemeron_b = keyed_ephemeron(&mut heap, class, key_b, value_b);
            let ephemeron_c = keyed_ephemeron(&mut heap, class, key_c, holder_b);
            // The Cheney scan reaches `E_b`, then `E_c`, then `H_c`.
            let value_a = heap.allocate_array(
                class,
                &[
                    Value::object(ephemeron_b),
                    Value::object(ephemeron_c),
                    Value::object(holder_c),
                ],
            );
            let ephemeron_a = keyed_ephemeron(&mut heap, class, key_a, value_a);
            let mut array_root = Value::object(heap.allocate_array(
                class,
                &[Value::object(ephemeron_a), Value::object(holder_a)],
            ));
            pending::work::set_scanned_checks(scanned_checks);
            pending::work::take();

            scavenge_bare(&mut heap, false, &mut [&mut array_root]);

            let resolutions = pending::work::take().resolutions;
            assert_eq!(resolutions, 1, "E_b through K_b's lookup");
            let elements = heap
                .array_elements(rooted(array_root))
                .expect("the array answers its elements");
            let value_a = heap
                .ephemeron_field(rooted(elements[0]), 1)
                .expect("an ephemeron has a value");
            let ephemeron_b = heap
                .array_elements(rooted(value_a))
                .expect("the value is an array")[0];
            let fields = heap
                .ephemeron_fields(rooted(ephemeron_b))
                .expect("an ephemeron answers its fields");
            assert!(
                fields.iter().all(|field| field.as_object().is_some()),
                "E_b keeps K_b and v_b"
            );
            verify_remembered_set_if_debug(&heap);
        }
    }

    /// The scavenge's `collect_a_chain_of_ephemerons`, all young: answers
    /// the fixpoint's work with the first key held by
    /// nothing and with it rooted.
    fn scavenge_a_chain_of_ephemerons(length: usize) -> [pending::work::Work; 2] {
        [false, true].map(|root_the_first_key| {
            let mut heap = Heap::new();
            let class = old_instance_class(&mut heap);
            let keys: Vec<ObjectReference<'_>> = (0..=length)
                .map(|_| heap.allocate_instance(class, 0))
                .collect();
            let mut ephemerons = Vec::with_capacity(length);
            for position in 0..length {
                let ephemeron = heap.allocate_ephemeron(class, 3);
                assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(keys[position])));
                assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(keys[position + 1])));
                ephemerons.push(Value::object(ephemeron));
            }
            ephemerons.reverse();
            let mut array_root = Value::object(heap.allocate_array(class, &ephemerons));
            let mut first_key_root = if root_the_first_key {
                Value::object(keys[0])
            } else {
                Value::nil()
            };
            pending::work::take();

            scavenge_bare(
                &mut heap,
                false,
                &mut [&mut array_root, &mut first_key_root],
            );

            let work = pending::work::take();
            let ephemerons = heap
                .array_elements(rooted(array_root))
                .expect("the array answers its elements");
            assert_eq!(ephemerons.len(), length);
            for (position, ephemeron) in ephemerons.into_iter().enumerate() {
                let fields = heap
                    .ephemeron_fields(rooted(ephemeron))
                    .expect("an ephemeron answers its fields");
                if root_the_first_key {
                    let [key, value] = [fields[0], fields[1]]
                        .map(|field| heap.instance_field_count(rooted(field)));
                    assert!(
                        key == Some(0) && value == Some(0),
                        "ephemeron {position} keeps its key and its value"
                    );
                } else {
                    assert!(
                        fields.iter().all(|field| field.is_nil()),
                        "ephemeron {position} is mourned"
                    );
                }
            }
            verify_remembered_set_if_debug(&heap);
            work
        })
    }

    /// Two young ephemerons share a key that only a third ephemeron's
    /// value reaches, its own key copied after it was scanned: once the
    /// third is scanned, the shared key's one lookup resolves both. Old
    /// sharers name their young values afterwards and are remembered,
    /// once.
    ///
    /// Indexed from the first check on, so that the lookup is what
    /// resolves them.
    #[test]
    fn two_ephemerons_sharing_a_key_reached_late_are_both_kept_in_a_scavenge() {
        for sharers_are_old in [false, true] {
            let mut heap = Heap::new();
            let class = old_instance_class(&mut heap);
            let [first_key, shared_key, first_value, second_value] =
                [0; 4].map(|_| heap.allocate_instance(class, 0));
            let holder = heap.allocate_instance(class, 1);
            assert!(heap.instance_set_field(holder, 0, Value::object(first_key)));
            let first = heap.allocate_ephemeron(class, 2);
            assert!(heap.ephemeron_set_field(first, 0, Value::object(first_key)));
            assert!(heap.ephemeron_set_field(first, 1, Value::object(shared_key)));
            heap.set_allocating_old(sharers_are_old);
            let sharing = [first_value, second_value].map(|value| {
                let ephemeron = heap.allocate_ephemeron(class, 2);
                assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(shared_key)));
                assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(value)));
                Value::object(ephemeron)
            });
            heap.set_allocating_old(false);
            // The Cheney scan reaches the ephemerons in this order, all
            // before the holder copies the first key.
            let mut array_root = Value::object(heap.allocate_array(
                class,
                &[
                    sharing[0],
                    sharing[1],
                    Value::object(first),
                    Value::object(holder),
                ],
            ));
            pending::work::set_scanned_checks(0);
            pending::work::take();

            scavenge_bare(&mut heap, false, &mut [&mut array_root]);

            let resolutions = pending::work::take().resolutions;
            assert_eq!(resolutions, 2, "both through the shared key's lookup");
            let elements = heap
                .array_elements(rooted(array_root))
                .expect("the array answers its elements");
            for ephemeron in elements[..2].iter().copied().map(rooted) {
                let fields = heap
                    .ephemeron_fields(ephemeron)
                    .expect("an ephemeron answers its fields");
                assert!(
                    fields.iter().all(|field| field.as_object().is_some()),
                    "the ephemeron keeps its key and its value"
                );
                if sharers_are_old {
                    assert_eq!(
                        heap.remembered
                            .iter()
                            .filter(|entry| **entry == ephemeron.offset)
                            .count(),
                        1,
                        "it names young objects, so it is remembered, once"
                    );
                }
            }
            verify_remembered_set_if_debug(&heap);
        }
    }

    /// An ephemeron keyed by a resolved future whose value only another
    /// ephemeron's value reaches, that one's key copied after it was
    /// scanned: indexed by the value as well as the proxy, it is resolved
    /// when the fixpoint copies the value, and its key names the copy.
    ///
    /// Indexed from the first check on, so that the lookup is what
    /// resolves them.
    #[test]
    fn an_ephemeron_keyed_by_a_resolved_future_reached_late_is_kept_in_a_scavenge() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let future_class = old_instance_class(&mut heap);
        let value = heap.allocate_string(class, crate::CodePoints::from("value"));
        let guarded = heap.allocate_string(class, crate::CodePoints::from("guarded"));
        let future = resolved_future(&mut heap, future_class, Value::object(value));
        let late_key = heap.allocate_instance(class, 0);
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(late_key)));
        let reaching = heap.allocate_ephemeron(class, 2);
        assert!(heap.ephemeron_set_field(reaching, 0, Value::object(late_key)));
        assert!(heap.ephemeron_set_field(reaching, 1, Value::object(value)));
        let keyed = heap.allocate_ephemeron(class, 2);
        assert!(heap.ephemeron_set_field(keyed, 0, Value::object(future)));
        assert!(heap.ephemeron_set_field(keyed, 1, Value::object(guarded)));
        let mut array_root = Value::object(heap.allocate_array(
            class,
            &[
                Value::object(keyed),
                Value::object(reaching),
                Value::object(holder),
            ],
        ));
        let futures = ResolvedFutures::new(
            future_class,
            FUTURE_COMPUTATION,
            FUTURE_STATE,
            FUTURE_RESOLVED,
        );
        pending::work::set_scanned_checks(0);
        pending::work::take();

        heap.scavenge(false, Some(futures), |scavenger| {
            use crate::trace::Tracer;
            scavenger.value(&mut array_root);
        });

        let resolutions = pending::work::take().resolutions;
        assert_eq!(resolutions, 1, "through the value's lookup");
        let elements = heap
            .array_elements(rooted(array_root))
            .expect("the array answers its elements");
        let reaching_fields = heap
            .ephemeron_fields(rooted(elements[1]))
            .expect("an ephemeron answers its fields");
        let keyed_fields = heap
            .ephemeron_fields(rooted(elements[0]))
            .expect("an ephemeron answers its fields");
        assert_eq!(text_of(&heap, reaching_fields[1]).as_deref(), Some("value"));
        assert!(
            names(keyed_fields[0], rooted(reaching_fields[1])),
            "the key names the value's copy"
        );
        assert_eq!(
            text_of(&heap, keyed_fields[1]).as_deref(),
            Some("guarded"),
            "the entry is kept"
        );
        verify_remembered_set_if_debug(&heap);
    }

    /// A resolved future that only a weak word or an ephemeron's key names
    /// stands for its value in a scavenge as in a collection: the value
    /// survives through a holder the Cheney scan reaches after the weak
    /// objects, the proxy through nothing, and each word then names the
    /// value's copy -- in an old weak object too, which stays remembered
    /// for it. An unresolved future held only weakly is judged as itself.
    #[test]
    fn a_resolved_future_held_weakly_is_judged_by_its_value_in_a_scavenge() {
        for holders_are_old in [false, true] {
            let mut heap = Heap::new();
            let class = old_instance_class(&mut heap);
            let future_class = old_instance_class(&mut heap);
            heap.set_allocating_old(holders_are_old);
            let weak = heap.allocate_weak_array(class, 2);
            let ephemeron = heap.allocate_ephemeron(class, 3);
            heap.set_allocating_old(false);
            assert_eq!(space::is_young(weak.offset), !holders_are_old);
            let value = heap.allocate_string(class, crate::CodePoints::from("value"));
            let guarded = heap.allocate_string(class, crate::CodePoints::from("guarded"));
            let future = resolved_future(&mut heap, future_class, Value::object(value));
            let unresolved = heap.allocate_instance(future_class, 2);
            assert!(heap.instance_set_field(unresolved, FUTURE_STATE, Value::small_integer(0)));
            assert!(heap.weak_array_set_element(weak, 0, Value::object(future)));
            assert!(heap.weak_array_set_element(weak, 1, Value::object(unresolved)));
            assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(future)));
            assert!(heap.ephemeron_set_field(ephemeron, 1, Value::object(guarded)));
            let holder = heap.allocate_instance(class, 1);
            assert!(heap.instance_set_field(holder, 0, Value::object(value)));
            let mut ephemeron_root = Value::object(ephemeron);
            let mut weak_root = Value::object(weak);
            let mut holder_root = Value::object(holder);
            let futures = ResolvedFutures::new(
                future_class,
                FUTURE_COMPUTATION,
                FUTURE_STATE,
                FUTURE_RESOLVED,
            );

            heap.scavenge(false, Some(futures), |scavenger| {
                use crate::trace::Tracer;
                for root in [&mut ephemeron_root, &mut weak_root, &mut holder_root] {
                    scavenger.value(root);
                }
            });

            let weak = rooted(weak_root);
            let ephemeron = rooted(ephemeron_root);
            let value = heap
                .instance_field(rooted(holder_root), 0)
                .expect("the holder holds the value");
            assert_eq!(text_of(&heap, value).as_deref(), Some("value"));
            let elements = heap
                .weak_array_elements(weak)
                .expect("a weak array answers its elements");
            assert!(
                names(elements[0], rooted(value)),
                "the weak word names the value's copy"
            );
            assert!(elements[1].is_nil(), "the unresolved future is gone");
            let fields = heap
                .ephemeron_fields(ephemeron)
                .expect("an ephemeron answers its fields");
            assert!(
                names(fields[0], rooted(value)),
                "the key names the value's copy"
            );
            assert_eq!(
                text_of(&heap, fields[1]).as_deref(),
                Some("guarded"),
                "the entry is kept"
            );
            if holders_are_old {
                for holder in [weak, ephemeron] {
                    assert_eq!(
                        heap.remembered
                            .iter()
                            .filter(|entry| **entry == holder.offset)
                            .count(),
                        1,
                        "an old holder naming the young value stays remembered, once"
                    );
                }
            }
            verify_remembered_set_if_debug(&heap);
        }
    }

    /// A "resolved" future whose value is itself a future is one crafted
    /// code corrupted, and neither collector sees through it: a weak word
    /// holding it is judged by the outer future alone, which only the weak
    /// word holds, and cleared -- in a scavenge too, where the inner future
    /// a root holds has already been copied and its old place is a
    /// forwarder, which must still be recognised as a future.
    #[test]
    fn a_malformed_resolved_future_held_weakly_is_not_seen_through_by_either_collector() {
        for collection in [false, true] {
            let mut heap = Heap::new();
            let class = old_instance_class(&mut heap);
            let future_class = old_instance_class(&mut heap);
            let weak = heap.allocate_weak_array(class, 1);
            let value = heap.allocate_string(class, crate::CodePoints::from("value"));
            let inner = resolved_future(&mut heap, future_class, Value::object(value));
            let outer = resolved_future(&mut heap, future_class, Value::object(inner));
            assert!(heap.weak_array_set_element(weak, 0, Value::object(outer)));
            let mut weak_root = Value::object(weak);
            let mut inner_root = Value::object(inner);
            let futures = ResolvedFutures::new(
                future_class,
                FUTURE_COMPUTATION,
                FUTURE_STATE,
                FUTURE_RESOLVED,
            );

            if collection {
                // The weak word is left naming the outer future, whose own
                // field the replacement rewrites to the inner one's value.
                heap.replace_resolved_futures(futures);
                heap.collect_for_test(&mut [&mut weak_root, &mut inner_root]);
            } else {
                heap.scavenge(false, Some(futures), |scavenger| {
                    use crate::trace::Tracer;
                    for root in [&mut weak_root, &mut inner_root] {
                        scavenger.value(root);
                    }
                });
            }

            let element = heap
                .weak_array_element(rooted(weak_root), 0)
                .expect("a weak array answers its slot");
            assert!(
                element.is_nil(),
                "the outer future, held only weakly, is gone (collection: {collection})"
            );
            verify_remembered_set_if_debug(&heap);
        }
    }

    /// Old weak objects and ephemerons of every kind at once, some of a
    /// young class so that the scan of their strong words remembers them
    /// before their weak words are settled: after each of three scavenges
    /// every old object naming a young one is remembered, and none twice.
    #[test]
    #[cfg(debug_assertions)]
    fn the_remembered_set_verifies_after_a_scavenge_of_weak_objects() {
        let mut heap = Heap::new();
        let old_class = old_instance_class(&mut heap);
        let young_class = heap.allocate_self_referential(0);
        let old_key = old_instance(&mut heap, old_class);
        let mut roots = Vec::new();
        for class in [old_class, young_class] {
            let kept = heap.allocate_string(old_class, crate::CodePoints::from("kept"));
            let surviving_key = heap.allocate_string(old_class, crate::CodePoints::from("key"));
            roots.push(Value::object(kept));
            roots.push(Value::object(surviving_key));
            heap.set_allocating_old(true);
            let old_weak = heap.allocate_weak_array(class, 2);
            let old_ephemerons = [0, 1, 2].map(|_| heap.allocate_ephemeron(class, 2));
            heap.set_allocating_old(false);
            let young_weak = heap.allocate_weak_array(class, 2);
            let young_ephemeron = heap.allocate_ephemeron(class, 2);
            let dying = heap.allocate_string(old_class, crate::CodePoints::from("dying"));
            let values: Vec<Value<'_>> = (0..4)
                .map(|_| {
                    Value::object(heap.allocate_string(old_class, crate::CodePoints::from("value")))
                })
                .collect();
            for weak in [old_weak, young_weak] {
                assert!(heap.weak_array_set_element(weak, 0, Value::object(kept)));
                assert!(heap.weak_array_set_element(weak, 1, Value::object(dying)));
            }
            let keys = [
                Value::object(old_key),
                Value::object(dying),
                Value::object(surviving_key),
            ];
            for ((ephemeron, key), value) in old_ephemerons.into_iter().zip(keys).zip(&values) {
                assert!(heap.ephemeron_set_field(ephemeron, 0, key));
                assert!(heap.ephemeron_set_field(ephemeron, 1, *value));
                roots.push(Value::object(ephemeron));
            }
            assert!(heap.ephemeron_set_field(young_ephemeron, 0, Value::object(surviving_key)));
            assert!(heap.ephemeron_set_field(young_ephemeron, 1, values[3]));
            roots.extend([Value::object(old_weak), Value::object(young_weak)]);
            roots.push(Value::object(young_ephemeron));
        }
        heap.verify_remembered_set();
        for tenure_all in [false, false, true] {
            let mut root_references: Vec<&mut Value<'_>> = roots.iter_mut().collect();
            scavenge_bare(&mut heap, tenure_all, &mut root_references);
            heap.verify_remembered_set();
        }
        assert!(heap.remembered.is_empty());
    }

    #[test]
    fn a_scavenge_that_tenures_everything_empties_the_nursery_and_the_remembered_set() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let holder = old_instance(&mut heap, class);
        let held = heap.allocate_string(class, crate::CodePoints::from("held"));
        assert!(heap.instance_set_field(holder, 0, Value::object(held)));
        let survivor = heap.allocate_string(class, crate::CodePoints::from("survivor"));
        let mut survivor_root = Value::object(survivor);
        let mut holder_root = Value::object(holder);
        // One scavenge first, so that the survivor space holds something too.
        scavenge_bare(
            &mut heap,
            false,
            &mut [&mut survivor_root, &mut holder_root],
        );
        let class = heap.object_class(rooted(survivor_root));
        let fresh = heap.allocate_string(class, crate::CodePoints::from("fresh"));
        let mut fresh_root = Value::object(fresh);
        assert!(
            heap.space.region(RegionName::PastSurvivor).top
                > heap.space.region(RegionName::PastSurvivor).start
        );
        assert!(!heap.remembered.is_empty());

        scavenge_bare(
            &mut heap,
            true,
            &mut [&mut survivor_root, &mut fresh_root, &mut holder_root],
        );

        assert_eq!(heap.young_objects, 0);
        let past = heap.space.region(RegionName::PastSurvivor);
        assert_eq!(past.top, past.start);
        let eden = heap.space.region(RegionName::Eden);
        assert_eq!(eden.top, eden.start);
        assert!(heap.remembered.is_empty());
        let holder = rooted(holder_root);
        assert!(!heap.header(holder).remembered());
        let held = rooted(heap.instance_field(holder, 0).unwrap_or_else(Value::nil));
        for (object, text) in [
            (held, "held"),
            (rooted(survivor_root), "survivor"),
            (rooted(fresh_root), "fresh"),
        ] {
            assert!(!space::is_young(object.offset));
            assert_eq!(heap.str_value(object), Some(text.to_string()));
        }
    }

    /// A move relocates the remembered set with the objects: the next
    /// scavenge finds the holder where it now lies, and so the young object
    /// only the holder reaches.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_relocates_the_remembered_set() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let holder = old_instance(&mut heap, class);
        let held = heap.allocate_string(class, crate::CodePoints::from("held"));
        assert!(heap.instance_set_field(holder, 0, Value::object(held)));
        assert_eq!(heap.remembered, vec![holder.offset]);
        let root = heap.stash(Value::object(holder));

        move_bare_heap(&mut heap);

        for offset in &heap.remembered {
            assert!(
                Header(heap.space.words()[header_index(*offset)]).remembered(),
                "a remembered offset names a remembered object"
            );
        }
        scavenge_bare(&mut heap, false, &mut []);
        let holder = rooted(heap.fetch(&root));
        let held = rooted(heap.instance_field(holder, 0).unwrap_or_else(Value::nil));
        assert!(space::is_young(held.offset));
        assert_eq!(heap.str_value(held), Some("held".to_string()));
        assert_eq!(heap.remembered, vec![holder.offset]);
        assert!(heap.header(holder).remembered());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn a_handle_kept_across_a_scavenge_is_stale() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        scavenge_bare(&mut heap, false, &mut []);
        let _ = heap.str_value(kept);
    }

    #[test]
    fn a_full_collection_compacts_the_nursery_s_survivors_after_the_old_ones() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let old_dead = old_instance(&mut heap, class);
        let old_kept = old_instance(&mut heap, class);
        let young_dead = heap.allocate_string(class, crate::CodePoints::from("dead"));
        let young_kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        // Each generation reachable only through the other: the young string
        // through the old instance's field, the old instance through a young
        // array the root holds.
        assert!(heap.instance_set_field(old_kept, 0, Value::object(young_kept)));
        let holder = heap.allocate_array(class, &[Value::object(old_kept)]);
        let _ = (old_dead, young_dead);
        let mut root = Value::object(holder);

        heap.collect_for_test(&mut [&mut root]);

        let statistics = heap.heap_statistics();
        assert_eq!(heap.young_objects, 0);
        assert!(heap.remembered.is_empty());
        let Some(holder) = root.as_object() else {
            panic!("the root no longer holds an object")
        };
        assert!(!space::is_young(holder.offset));
        let Some(old_kept) = heap
            .array_element(holder, 0)
            .and_then(|value| value.as_object())
        else {
            panic!("the array lost its element")
        };
        assert!(
            !heap.header(old_kept).remembered(),
            "nothing is young any more"
        );
        let Some(young_kept) = heap
            .instance_field(old_kept, 0)
            .and_then(|value| value.as_object())
        else {
            panic!("the instance lost its field")
        };
        assert_eq!(heap.str_value(young_kept), Some("kept".to_string()));
        // The class handle from before the collection is stale.
        let class = heap.object_class(holder);
        let key = |object| heap.object_identity_key(object);
        assert!(
            key(class) < key(old_kept),
            "the old survivors keep their order"
        );
        assert!(key(old_kept) < key(young_kept), "the nursery's follow them");
        assert!(key(young_kept) < key(holder), "in address order");
        // The class, the kept instance, the string and the array.
        assert_eq!(statistics.live_objects, 4);
    }

    #[test]
    fn a_full_collection_after_a_scavenge_finds_the_survivor_space() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let first = heap.allocate_string(class, crate::CodePoints::from("first"));
        let mut first_root = Value::object(first);
        scavenge_bare(&mut heap, false, &mut [&mut first_root]);
        let past = heap.space.region(RegionName::PastSurvivor);
        assert!(past.top > past.start, "the survivor is in a survivor space");
        let class = heap.object_class(rooted(first_root));
        let second = heap.allocate_string(class, crate::CodePoints::from("second"));
        let mut second_root = Value::object(second);

        heap.collect_for_test(&mut [&mut first_root, &mut second_root]);

        assert_eq!(heap.young_objects, 0);
        for region in [RegionName::PastSurvivor, RegionName::Eden] {
            let region = heap.space.region(region);
            assert_eq!(region.top, region.start);
        }
        for (root, text) in [(first_root, "first"), (second_root, "second")] {
            let object = rooted(root);
            assert!(!space::is_young(object.offset));
            assert_eq!(heap.str_value(object), Some(text.to_string()));
        }
        // The class and the two strings.
        assert_eq!(heap.heap_statistics().live_objects, 3);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn a_young_handle_kept_across_a_full_collection_is_stale() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let kept = heap.allocate_string(class, crate::CodePoints::from("kept"));
        let mut root = Value::object(kept);
        heap.collect_for_test(&mut [&mut root]);
        let _ = heap.str_value(kept);
    }

    /// The header index of the ByteArray `root` holds, after checking that
    /// its bytes are `expected` and its data lie at a 32-byte address.
    fn assert_byte_array_intact<'gc>(heap: &Heap<'gc>, root: Value<'gc>, expected: &[u8]) -> usize {
        let object = rooted(root);
        assert_eq!(heap.byte_array_bytes(object).as_deref(), Some(expected));
        let header = header_index(object.offset);
        assert_eq!(
            object::aligned_data_start(header) % object::ALIGNMENT_WORDS,
            0
        );
        header
    }

    #[test]
    fn a_byte_array_reads_back_its_bytes_and_its_size() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        for size in [0, 1, 7, 8, 9, 33] {
            let bytes: Vec<u8> = (0..size).map(|byte| byte as u8 ^ 0xA5).collect();
            let object = heap.allocate_byte_array(class, &bytes);
            assert!(heap.is_byte_array(object));
            assert!(!heap.is_string(object), "a ByteArray is never a String");
            assert_eq!(heap.byte_array_size(object), Some(size));
            assert_eq!(heap.byte_array_bytes(object), Some(bytes.clone()));
            let header = heap.header(object);
            assert_eq!(header.format(), Format::AlignedBytes);
            assert_eq!(usize::from(header.unused()), (8 - size % 8) % 8);
            let index = header_index(object.offset);
            let start = object::aligned_data_start(index);
            assert_eq!(start % object::ALIGNMENT_WORDS, 0);
            let block = block_range(heap.space.words(), index);
            assert!(block.start + FIELDS <= start);
            assert!(
                start + size.div_ceil(8) <= block.end,
                "the data end inside the block"
            );
            if size > 0 {
                assert_eq!(heap.byte_array_at(object, size - 1), bytes.last().copied());
                assert!(heap.byte_array_set(object, size - 1, 7));
                assert_eq!(heap.byte_array_at(object, size - 1), Some(7));
            }
            assert_eq!(heap.byte_array_at(object, size), None);
            assert!(!heap.byte_array_set(object, size, 7));
        }
        let string = heap.allocate_string(class, crate::CodePoints::from("abc"));
        assert!(!heap.is_byte_array(string));
        assert_eq!(heap.byte_array_size(string), None);
    }

    #[test]
    fn a_new_byte_array_is_zero_filled() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let object = heap.allocate_zeroed_byte_array(class, 40);
        assert_eq!(heap.byte_array_size(object), Some(40));
        assert_eq!(heap.byte_array_bytes(object), Some(vec![0; 40]));
        // The reserve too, so that the space holds no stale words.
        let block = block_range(heap.space.words(), header_index(object.offset));
        assert!(heap.space.words()[block.start + FIELDS..block.end]
            .iter()
            .all(|word| *word == 0));
    }

    #[test]
    fn a_byte_array_lender_copies_within_as_a_memmove_does() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let object = heap.allocate_byte_array(class, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(
            heap.with_byte_array_mut(object, |mut data| data.copy_within(0..6, 3)),
            Some(true)
        );
        assert_eq!(
            heap.byte_array_bytes(object),
            Some(vec![0, 1, 2, 0, 1, 2, 3, 4, 5, 9])
        );
        assert_eq!(
            heap.with_byte_array_mut(object, |mut data| data.copy_within(4..10, 1)),
            Some(true)
        );
        assert_eq!(
            heap.byte_array_bytes(object),
            Some(vec![0, 1, 2, 3, 4, 5, 9, 4, 5, 9])
        );
        assert_eq!(
            heap.with_byte_array_mut(object, |mut data| data.copy_within(5..11, 0)),
            Some(false),
            "a range past the end writes nothing"
        );
        assert_eq!(
            heap.with_byte_array(object, |data| data.bytes(2..5)),
            Some(Some(vec![2, 3, 4]))
        );
    }

    /// Who moves the ByteArray of
    /// `an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position`.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ByteArrayMover {
        Scavenge,
        SlideFromTheNursery,
        SlideInTheOldSpace,
        #[cfg(debug_assertions)]
        Move,
    }

    /// Every mover realigns the data from wherever the old block put them
    /// to wherever the new one does. A dead instance of `filler` fields
    /// ahead of the ByteArray moves its header by one word per field, and a
    /// live one of `leader` fields ahead of it moves it at the destination
    /// too, so that the scavenger and the slide meet every pair of old and
    /// new positions within a 32-byte line.
    #[test]
    fn an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position() {
        let expected: Vec<u8> = (0..37).collect();
        let mut pairs = std::collections::BTreeSet::new();
        let movers = [
            ByteArrayMover::Scavenge,
            ByteArrayMover::SlideFromTheNursery,
            ByteArrayMover::SlideInTheOldSpace,
            #[cfg(debug_assertions)]
            ByteArrayMover::Move,
        ];
        for mover in movers {
            for filler in 0..4 {
                for leader_fields in 0..4 {
                    let mut heap = Heap::new();
                    let class = old_instance_class(&mut heap);
                    heap.set_allocating_old(mover == ByteArrayMover::SlideInTheOldSpace);
                    let leader = heap.allocate_instance(class, leader_fields);
                    heap.allocate_instance(class, filler);
                    let bytes = heap.allocate_byte_array(class, &expected);
                    heap.set_allocating_old(false);
                    let mut leader_root = Value::object(leader);
                    let mut root = Value::object(bytes);
                    let mut before = assert_byte_array_intact(&heap, root, &expected);
                    match mover {
                        ByteArrayMover::Scavenge => {
                            // Survivor to survivor twice, so that both
                            // survivor spaces are met, then a promotion.
                            for tenure_all in [false, false, true] {
                                scavenge_bare(
                                    &mut heap,
                                    tenure_all,
                                    &mut [&mut leader_root, &mut root],
                                );
                                let after = assert_byte_array_intact(&heap, root, &expected);
                                pairs.insert((before % 4, after % 4));
                                before = after;
                            }
                        }
                        ByteArrayMover::SlideFromTheNursery
                        | ByteArrayMover::SlideInTheOldSpace => {
                            heap.collect_for_test(&mut [&mut leader_root, &mut root]);
                            let after = assert_byte_array_intact(&heap, root, &expected);
                            pairs.insert((before % 4, after % 4));
                        }
                        #[cfg(debug_assertions)]
                        ByteArrayMover::Move => {
                            use crate::trace::Tracer;
                            // Every distance of the stress mode, up from
                            // each region's base and back down to it.
                            for _ in 0..62 {
                                let mut forwarder = heap.move_everything();
                                forwarder.value(&mut leader_root);
                                forwarder.value(&mut root);
                                assert_byte_array_intact(&heap, root, &expected);
                            }
                        }
                    }
                }
            }
        }
        for old in 0..4 {
            for new in 0..4 {
                assert!(
                    pairs.contains(&(old, new)),
                    "no mover met a ByteArray moving from residue {old} to residue {new}"
                );
            }
        }
    }

    #[test]
    fn describe_names_a_byte_array_by_its_size() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let object = heap.allocate_byte_array(class, &[1, 2, 3]);
        assert_eq!(heap.describe(Value::object(object)), "a ByteArray of 3");
    }

    // ---------- The bulk forward ----------

    /// `Heap::forward_identity` with nothing beside the heap to forward:
    /// these tests read a handle afterwards only out of a host root, out
    /// of an object, or as a new object, which the forward does not move.
    fn forward<'gc>(
        heap: &mut Heap<'gc>,
        old: &[ObjectReference<'gc>],
        new: &[ObjectReference<'gc>],
    ) -> Result<(), ForwardRefusal> {
        heap.forward_identity(old, new).map(|_forwarder| ())
    }

    /// The format of the object at `object`'s offset, read from the raw
    /// words, since `header` refuses a forwarder.
    fn raw_format(heap: &Heap<'_>, object: ObjectReference<'_>) -> Format {
        Header(heap.space.words()[header_index(object.offset)]).format()
    }

    #[test]
    fn the_forward_rewrites_every_holder_of_an_old_object() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let weak_class = named_class(&mut heap, "WeakArray");
        let ephemeron_class = named_class(&mut heap, "Ephemeron");
        let old = heap.allocate_instance(class, 2);
        let new = heap.allocate_instance(class, 2);
        let instance = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(instance, 0, Value::object(old)));
        let array = heap.allocate_array(class, &[Value::small_integer(1), Value::object(old)]);
        let weak = heap.allocate_weak_array(weak_class, 1);
        assert!(heap.weak_array_set_element(weak, 0, Value::object(old)));
        let ephemeron = heap.allocate_ephemeron(ephemeron_class, 2);
        assert!(heap.ephemeron_set_field(ephemeron, 0, Value::object(old)));
        let root = heap.stash(Value::object(old));

        assert!(forward(&mut heap, &[old], &[new]).is_ok());

        assert_eq!(raw_format(&heap, old), Format::Forwarded);
        let holders = [
            heap.instance_field(instance, 0),
            heap.array_element(array, 1),
            heap.weak_array_element(weak, 0),
            heap.ephemeron_fields(ephemeron)
                .and_then(|fields| fields.first().copied()),
            Some(heap.fetch(&root)),
        ];
        for (index, holder) in holders.into_iter().enumerate() {
            let named = rooted(holder.unwrap_or(Value::nil()));
            assert_eq!(
                heap.object_identity_key(named),
                heap.object_identity_key(new),
                "holder {index} names the new object"
            );
        }
        assert_eq!(
            heap.array_element(array, 0)
                .and_then(Value::as_small_integer),
            Some(1)
        );
    }

    #[test]
    fn the_new_object_takes_the_old_ones_identity_hash() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let old = heap.allocate_instance(class, 0);
        let new = heap.allocate_instance(class, 0);
        let hash = heap.object_ensure_identity_hash(old);
        assert_eq!(heap.object_identity_hash(new), 0);
        assert!(forward(&mut heap, &[old], &[new]).is_ok());
        assert_eq!(heap.object_identity_hash(new), hash);

        let first = heap.allocate_instance(class, 0);
        let second = heap.allocate_instance(class, 0);
        let merged = heap.allocate_instance(class, 0);
        heap.object_ensure_identity_hash(first);
        let second_hash = heap.object_ensure_identity_hash(second);
        assert!(forward(&mut heap, &[first, second], &[merged, merged]).is_ok());
        assert_eq!(
            heap.object_identity_hash(merged),
            second_hash,
            "a merge keeps the last pair's hash"
        );
    }

    #[test]
    fn a_split_a_chain_or_unequal_sizes_changes_nothing() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let old = heap.allocate_instance(class, 0);
        let new = heap.allocate_instance(class, 0);
        let other = heap.allocate_instance(class, 0);
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(old)));
        heap.object_ensure_identity_hash(old);
        let new_hash = heap.object_ensure_identity_hash(new);
        let attempts = [
            (vec![old, old], vec![new, other], ForwardRefusal::Split),
            (vec![old, new], vec![new, other], ForwardRefusal::Chain),
            (vec![old], vec![new, other], ForwardRefusal::UnequalSizes),
        ];
        for (old_objects, new_objects, refusal) in attempts {
            assert_eq!(
                forward(&mut heap, &old_objects, &new_objects).err(),
                Some(refusal)
            );
            assert_eq!(raw_format(&heap, old), Format::FixedPointers);
            assert_eq!(raw_format(&heap, new), Format::FixedPointers);
            assert!(names(
                heap.instance_field(holder, 0).unwrap_or(Value::nil()),
                old
            ));
            assert_eq!(heap.object_identity_hash(new), new_hash);
        }
    }

    #[test]
    fn an_old_holder_rewritten_to_a_young_object_is_remembered() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let old = heap.allocate_instance(class, 0);
        let holder = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(holder, 0, Value::object(old)));
        let root = heap.stash(Value::object(holder));
        scavenge_bare(&mut heap, true, &mut []);
        let holder = rooted(heap.fetch(&root));
        assert!(!space::is_young(holder.offset));
        assert!(!heap.header(holder).remembered());
        let old = rooted(heap.instance_field(holder, 0).unwrap_or(Value::nil()));
        let class = heap.object_class(holder);
        let new = heap.allocate_instance(class, 1);
        let inner = heap.allocate_instance(class, 1);
        assert!(heap.instance_set_field(inner, 0, Value::small_integer(42)));
        assert!(heap.instance_set_field(new, 0, Value::object(inner)));
        assert!(space::is_young(new.offset));

        assert!(forward(&mut heap, &[old], &[new]).is_ok());

        assert!(heap.header(holder).remembered());
        #[cfg(debug_assertions)]
        heap.verify_remembered_set();
        scavenge_bare(&mut heap, false, &mut []);
        let holder = rooted(heap.fetch(&root));
        let new = rooted(heap.instance_field(holder, 0).unwrap_or(Value::nil()));
        let inner = rooted(heap.instance_field(new, 0).unwrap_or(Value::nil()));
        assert_eq!(
            heap.instance_field(inner, 0)
                .and_then(Value::as_small_integer),
            Some(42)
        );
    }

    #[test]
    fn a_forwarder_is_garbage_after_the_forward() {
        let mut heap = Heap::new();
        let class = old_instance_class(&mut heap);
        let young_old = heap.allocate_instance(class, 1);
        let old_old = old_instance(&mut heap, class);
        let young_new = heap.allocate_instance(class, 1);
        let old_new = heap.allocate_instance(class, 1);
        assert!(space::is_young(young_old.offset));
        assert!(!space::is_young(old_old.offset));
        let roots = [
            heap.stash(Value::object(young_old)),
            heap.stash(Value::object(old_old)),
        ];
        let live_before = heap.heap_statistics().live_objects;

        assert!(forward(&mut heap, &[young_old, old_old], &[young_new, old_new]).is_ok());

        #[cfg(debug_assertions)]
        {
            heap.verify_remembered_set();
            let _ = heap.move_everything();
        }
        scavenge_bare(&mut heap, false, &mut []);
        assert_eq!(
            heap.heap_statistics().live_objects,
            live_before - 1,
            "the scavenge reclaims the young forwarder"
        );
        #[cfg(debug_assertions)]
        {
            heap.verify_remembered_set();
            let _ = heap.move_everything();
        }
        heap.collect_for_test(&mut []);
        assert_eq!(
            heap.heap_statistics().live_objects,
            live_before - 2,
            "the collection reclaims the old forwarder"
        );
        #[cfg(debug_assertions)]
        let _ = heap.move_everything();
        for root in &roots {
            let object = rooted(heap.fetch(root));
            assert_eq!(raw_format(&heap, object), Format::FixedPointers);
            assert_eq!(
                heap.instance_field(object, 0).map(Value::is_nil),
                Some(true)
            );
        }
    }

    #[test]
    fn a_body_answers_its_id_from_the_cache_after_the_first_lookup() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        let id = heap.code_id(&code);
        assert_eq!(code.code_id_cache.get(), Some((heap.code_table_epoch, id)));
        heap.tables.code_ids.clear();
        assert_eq!(heap.code_id(&code), id, "a hit never reaches the map");
    }

    #[test]
    fn two_heaps_holding_one_body_each_get_their_own_id() {
        let mut first = Heap::new();
        let mut second = Heap::new();
        let filler = shaped_code(0, 1);
        let code = shaped_code(0, 2);
        second.code_id(&filler);
        let in_first = first.code_id(&code);
        let in_second = second.code_id(&code);
        assert_ne!(in_first, in_second);
        assert_eq!(first.code_id(&code), in_first);
        assert_eq!(second.code_id(&code), in_second);
    }

    #[test]
    fn a_freed_row_is_never_answered_from_the_cache() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        let other = shaped_code(0, 2);
        let id = heap.code_id(&code);
        heap.free_unnamed_codes(&reclaim::BitSet::default());
        let reused = heap.code_id(&other);
        assert_eq!(reused, id, "the smallest free row is minted first");
        let again = heap.code_id(&code);
        assert_ne!(again, reused, "the cache's id was freed and reused");
        assert!(Rc::ptr_eq(&heap.code(again).expect("a row"), &code));
    }

    #[test]
    fn a_cloned_body_starts_with_an_empty_cache() {
        let mut heap = Heap::new();
        let code = shaped_code(0, 1);
        heap.code_id(&code);
        let copy = Rc::new((*code).clone());
        assert_eq!(copy.code_id_cache.get(), None);
    }

    #[test]
    fn a_cached_body_still_records_the_method_a_frame_registers_it_with() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(crate::BuiltinClass::Object);
            let application = mutator.heap().class_application(object).expect("a class");
            let method = application
                .mixin
                .compiled_methods()
                .next()
                .expect("Object has a method")
                .clone();
            // A body no frame has registered yet: a fresh copy, so the
            // bootstrap's own registration does not answer for it.
            let copy = std::rc::Rc::new(crate::mixin::MixinMethod {
                code: std::rc::Rc::new((*method.code).clone()),
                ..(*method).clone()
            });
            let id = mutator.heap_mut().code_id(&copy.code);
            assert!(mutator.heap().code_method(id).is_none());
            assert_eq!(mutator.heap_mut().code_id_of_method(&copy), id);
            assert!(std::rc::Rc::ptr_eq(
                &mutator.heap().code_method(id).expect("the method recorded"),
                &copy
            ));
        });
    }

    /// A running activation names its body by id, and nothing else need
    /// name it: the full collection's code reclaim keeps the row.
    #[cfg(debug_assertions)]
    #[test]
    fn a_full_collection_keeps_the_code_row_of_a_running_activation() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let body = trivial_code();
        let (activation, id) = interpreter.enter(|mutator| {
            let heap = mutator.heap_mut();
            let activation = heap
                .push_activation(
                    ActivationHeader {
                        code: body.clone(),
                        ..header()
                    },
                    Arguments::NONE,
                    0,
                )
                .expect("the stack holds the frame");
            (activation, heap.code_id(&body))
        });
        interpreter.collect_now();
        interpreter.enter(|mutator| {
            let heap = mutator.heap_mut();
            assert!(Rc::ptr_eq(&heap.activation_code(activation), &body));
            assert!(
                heap.code(id).is_some_and(|code| Rc::ptr_eq(&code, &body)),
                "the running body's row survives the collection"
            );
            heap.pop_activation(activation);
        });
    }

    #[test]
    fn a_nil_array_is_the_array_of_nils() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let made = heap.allocate_nil_array(class, 5);
        let listed = heap.allocate_array(class, &[Value::nil(); 5]);
        assert_eq!(heap.block(made).to_vec(), heap.block(listed).to_vec());
    }

    #[test]
    fn arrays_either_side_of_the_fast_edge_are_alike() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        for (length, fast) in [(252, 1), (253, 1), (254, 0), (255, 0)] {
            let (array, fast_bumps) =
                counting_fast_bumps(&mut heap, |heap| heap.allocate_nil_array(class, length));
            assert_eq!(fast_bumps, fast, "length {length}");
            assert!(space::is_young(array.offset));
            assert_eq!(heap.array_element_count(array), Some(length));
            assert!(heap
                .array_element(array, length - 1)
                .is_some_and(|value| value.is_nil()));
        }
    }

    #[test]
    fn an_instance_with_a_template_is_the_instance_without() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let guard = heap.header_dispatch_id(class);
        let header = InstanceHeader::for_slots(guard, 3).expect("small");
        let (templated, fast_bumps) = counting_fast_bumps(&mut heap, |heap| {
            heap.allocate_instance_with(class, header, 3)
        });
        assert_eq!(fast_bumps, 1);
        let plain = heap.allocate_instance(class, 3);
        assert_eq!(heap.block(templated).to_vec(), heap.block(plain).to_vec());
    }

    #[test]
    fn no_template_past_254_words() {
        assert!(InstanceHeader::for_slots(DispatchId::NONE, 253).is_some());
        assert!(InstanceHeader::for_slots(DispatchId::NONE, 254).is_none());
    }

    #[test]
    fn a_large_array_goes_old() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(0);
        let array = heap.allocate_nil_array(class, LARGE_OBJECT_BYTES / 8);
        assert!(!space::is_young(array.offset));
        assert!(heap.header(array).remembered());
    }

    #[test]
    fn a_length_past_the_old_space_does_not_fit() {
        let heap = Heap::new();
        assert!(heap.array_fits(10));
        assert!(!heap.array_fits(usize::MAX / 8));
    }

    // ---------- Suspended frames ----------

    /// The header of an activation a test suspends: an initializer of
    /// `class`, so that its frame records its code, which resuming reads
    /// back, as every frame of an activation the runtime pushes does.
    fn suspendable_header<'gc>(
        class: ClassReference<'gc>,
        argument_count: u16,
        register_count: u16,
    ) -> ActivationHeader<'gc> {
        ActivationHeader {
            current_class: Some(class),
            kind: ActivationKind::Initializer(class, InitializerBody::OwnPart),
            code: shaped_code(argument_count, register_count),
            ..header()
        }
    }

    /// A frame object's own register words, read past the marriage.
    fn own_registers<'gc>(heap: &Heap<'gc>, frame: FrameReference<'gc>) -> Vec<u64> {
        heap.frame_words(frame)[frame_field::COUNT..].to_vec()
    }

    /// The activation of a body with two arguments, 1 and 2, and three
    /// temporaries, 10, 11 and 12, reified: its frame object.
    fn two_arguments_three_temporaries<'gc>(
        heap: &mut Heap<'gc>,
        class: ClassReference<'gc>,
    ) -> (ActivationReference, FrameReference<'gc>) {
        let activation = heap
            .push_activation(
                suspendable_header(class, 2, first_above_header(2) + 3),
                Arguments::Values(&[Value::small_integer(1), Value::small_integer(2)].as_slice()),
                3,
            )
            .expect("the stack holds the frame");
        for (position, value) in (10..13).enumerate() {
            heap.activation_set_local(
                activation,
                first_above_header(2) + position as u16,
                Value::small_integer(value),
            );
        }
        let frame = heap.reify(activation);
        (activation, frame)
    }

    /// The register words of a frame holding 1, 2, 10, 11 and 12.
    fn one_two_ten_eleven_twelve() -> Vec<u64> {
        [1, 2, 10, 11, 12]
            .map(|value| Value::small_integer(value).word().bits())
            .to_vec()
    }

    /// A register below the arguments' end is its own field, one the header
    /// takes has none, and one above the header is 13 fields lower.
    #[test]
    fn a_register_maps_to_its_frame_field_past_the_header_numbers() {
        let header = frame_header::COUNT as u16;
        assert_eq!(Heap::frame_field_of_register(2, 0), Some(0));
        assert_eq!(Heap::frame_field_of_register(2, 1), Some(1));
        assert_eq!(Heap::frame_field_of_register(2, 2), None);
        assert_eq!(Heap::frame_field_of_register(2, 1 + header), None);
        assert_eq!(Heap::frame_field_of_register(2, 2 + header), Some(2));
        assert_eq!(Heap::frame_field_of_register(0, 0), None);
        assert_eq!(Heap::frame_field_of_register(0, header), Some(0));
        // A body past the register ceiling whose arguments leave no number
        // above the header: its arguments are its whole register file.
        assert_eq!(
            Heap::frame_field_of_register(u16::MAX, u16::MAX - 1),
            Some(usize::from(u16::MAX) - 1)
        );
        assert_eq!(Heap::frame_field_of_register(u16::MAX - 5, u16::MAX), None);
    }

    /// A frame object holds a field for each argument and each register
    /// above the header, none for the header's numbers: a body with two
    /// arguments and three temporaries has five, and its first temporary,
    /// register `2 + HEADER_REGISTERS`, is its third field once the
    /// activation returned.
    #[test]
    fn a_reified_frame_leaves_the_header_numbers_out() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Windowed");
        let (activation, frame) = two_arguments_three_temporaries(&mut heap, class);
        assert_eq!(own_registers(&heap, frame).len(), 5);
        let first_temporary = 2 + crate::bytecode::HEADER_REGISTERS;
        assert_eq!(
            heap.frame_local(frame, first_temporary).as_small_integer(),
            Some(10),
            "married, the frame reads the live register"
        );
        heap.retain_registers(activation);
        heap.pop_activation(activation);
        assert_eq!(own_registers(&heap, frame), one_two_ten_eleven_twelve());
        assert_eq!(heap.frame_register_shape(frame), (0, 2, 5));
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(1));
        assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(2));
        for (position, value) in (10..13).enumerate() {
            assert_eq!(
                heap.frame_local(frame, first_temporary + position as u16)
                    .as_small_integer(),
                Some(value)
            );
        }
        // A header number names no field: read as nil, written nowhere.
        assert!(heap.frame_local(frame, 2).is_nil());
        heap.frame_set_local(frame, 2, Value::small_integer(99));
        heap.frame_set_local(frame, first_temporary - 1, Value::small_integer(99));
        assert_eq!(own_registers(&heap, frame), one_two_ten_eleven_twelve());
        heap.frame_set_local(frame, first_temporary + 2, Value::small_integer(13));
        assert_eq!(
            heap.frame_local(frame, first_temporary + 2)
                .as_small_integer(),
            Some(13)
        );
    }

    /// A body past the register ceiling whose arguments leave no number
    /// above the header has its arguments as its whole register file, and a
    /// frame of it holds exactly those: the register file the image decoder
    /// holds a decoded frame of the body to (`CompiledCode::frame_register_count`).
    #[test]
    fn a_frame_of_a_body_whose_arguments_fill_the_ceiling_holds_its_arguments_alone() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Ceiling");
        let header = suspendable_header(class, u16::MAX, u16::MAX);
        let code = header.code.clone();
        let arguments = vec![Value::small_integer(3); usize::from(u16::MAX)];
        let activation = heap
            .push_activation(header, Arguments::Values(&arguments.as_slice()), 0)
            .expect("the stack holds the frame");
        let frame = heap.reify(activation);
        heap.retain_registers(activation);
        heap.pop_activation(activation);
        assert_eq!(
            heap.frame_register_shape(frame),
            (0, usize::from(u16::MAX), code.frame_register_count())
        );
        assert_eq!(
            heap.frame_local(frame, u16::MAX - 1).as_small_integer(),
            Some(3)
        );
    }

    /// Suspended, the same activation's frame holds its five registers
    /// without the header's numbers, and resuming it gives every register
    /// back where its code numbers it.
    #[test]
    fn suspending_and_resuming_round_trips_every_register_past_the_header() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Windowed");
        let (activation, frame) = two_arguments_three_temporaries(&mut heap, class);
        let floor = heap.activation_position(activation);
        let suspended = heap.suspend_activations(floor);
        assert!(suspended.is_some_and(|suspended| suspended.same(frame)));
        assert_eq!(own_registers(&heap, frame), one_two_ten_eleven_twelve());
        let first_temporary = first_above_header(2);
        assert_eq!(
            heap.frame_local(frame, first_temporary + 1)
                .as_small_integer(),
            Some(11)
        );
        let resumed = heap.resume_activation(frame, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(
            heap.activation_local(resumed, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(resumed, 1).as_small_integer(),
            Some(2)
        );
        for (position, value) in (10..13).enumerate() {
            assert_eq!(
                heap.activation_local(resumed, first_temporary + position as u16)
                    .as_small_integer(),
                Some(value)
            );
        }
        assert_eq!(
            heap.stack.frames.top(),
            heap.stack.window_end(resumed.offset),
            "the resumed window has the arguments, the header and three registers"
        );
        assert_eq!(heap.stack.register_count(resumed.offset), 3);
        heap.pop_activation(resumed);
        assert_eq!(heap.activation_depth(), 0);
    }

    #[test]
    fn suspending_and_resuming_an_activation_keeps_its_registers_and_program_counter() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Suspended");
        let bottom = heap
            .push_activation(
                suspendable_header(class, 1, 1),
                Arguments::Values(&[Value::small_integer(5)].as_slice()),
                0,
            )
            .expect("the stack holds the frame");
        let bottom_frame = heap.reify(bottom);
        let top = heap
            .push_activation(
                suspendable_header(class, 2, first_above_header(2) + 1),
                Arguments::Values(&[Value::small_integer(1), Value::small_integer(2)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        heap.activation_set_program_counter(top, 7);
        let top_frame = heap.reify(top);

        let suspended = heap.suspend_activations(0);

        assert!(suspended.is_some_and(|frame| frame.same(top_frame)));
        assert_eq!(heap.activation_depth(), 0);
        assert_eq!(heap.suspension_count(), 2);
        for frame in [top_frame, bottom_frame] {
            assert!(heap.frame_is_suspended(frame));
            assert!(heap.frame_is_running(frame));
            assert!(heap.frame_activation(frame).is_none());
            assert!(!heap.frame_is_uncontinuable(frame));
        }
        assert_eq!(heap.frame_local(top_frame, 0).as_small_integer(), Some(1));
        assert_eq!(heap.frame_local(top_frame, 1).as_small_integer(), Some(2));
        assert!(heap.frame_local(top_frame, first_above_header(2)).is_nil());
        assert_eq!(
            heap.frame_local(bottom_frame, 0).as_small_integer(),
            Some(5)
        );
        assert!(heap
            .frame_sender(top_frame)
            .is_some_and(|sender| sender.same(bottom_frame)));
        assert!(heap.frame_sender(bottom_frame).is_none());

        let resumed = heap.resume_activation(top_frame, FeedbackBase::NOT_ASSIGNED);

        assert_eq!(heap.activation_depth(), 1);
        assert_eq!(heap.suspension_count(), 1);
        assert_eq!(
            heap.activation_local(resumed, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(resumed, 1).as_small_integer(),
            Some(2)
        );
        assert!(heap
            .activation_local(resumed, first_above_header(2))
            .is_nil());
        assert_eq!(heap.activation_program_counter(resumed), 7);
        assert!(heap
            .frame_activation(top_frame)
            .is_some_and(|live| live.same(resumed)));
        assert!(heap
            .activation_reified(resumed)
            .is_some_and(|frame| frame.same(top_frame)));
        assert!(!heap.frame_is_suspended(top_frame));
        assert!(heap.frame_is_running(top_frame));
        assert!(heap.frame_sender(top_frame).is_none());
        assert!(own_registers(&heap, top_frame)
            .iter()
            .all(|bits| *bits == Word::NIL.bits()));
        assert!(heap.frame_is_suspended(bottom_frame));

        heap.pop_activation(resumed);
        let resumed = heap.resume_activation(bottom_frame, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(
            heap.activation_local(resumed, 0).as_small_integer(),
            Some(5)
        );
        heap.pop_activation(resumed);
        assert_eq!(heap.suspension_count(), 0);
        assert!(!heap.frame_is_running(bottom_frame));
    }

    #[test]
    fn a_popped_continuation_frees_its_row() {
        let mut heap = Heap::new();
        let unwinding = || ActivationHeader {
            then: Continuation::Unwind(Box::new(PendingUnwind {
                target: None,
                action: UnwindAction::Restart,
            })),
            ..header()
        };
        let activations: Vec<_> = (0..3)
            .map(|_| {
                heap.push_activation(unwinding(), Arguments::NONE, 0)
                    .expect("the stack holds the frame")
            })
            .collect();
        assert_eq!(heap.continuation_rows(), vec![true, true, true]);
        for activation in activations.into_iter().rev() {
            let popped = heap.pop_activation(activation);
            assert!(matches!(popped.then, Continuation::Unwind(_)));
        }

        heap.push_activation(unwinding(), Arguments::NONE, 0)
            .expect("the stack holds the frame");

        assert_eq!(heap.continuation_rows(), vec![true]);
    }

    /// A continuation replaced by a `Deliver` leaves a hole below the last
    /// row, which the next push fills first; popping the last row gives
    /// back the holes it uncovers.
    #[test]
    fn a_row_freed_below_the_last_is_minted_first_and_given_back_with_it() {
        let mut heap = Heap::new();
        let unwinding = || ActivationHeader {
            then: Continuation::Unwind(Box::new(PendingUnwind {
                target: None,
                action: UnwindAction::Restart,
            })),
            ..header()
        };
        let restart = || {
            Continuation::Unwind(Box::new(PendingUnwind {
                target: None,
                action: UnwindAction::Restart,
            }))
        };
        heap.push_activation(unwinding(), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        let middle = heap
            .push_activation(unwinding(), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        let top = heap
            .push_activation(unwinding(), Arguments::NONE, 0)
            .expect("the stack holds the frame");

        heap.activation_set_continuation(middle, Continuation::Deliver);
        assert_eq!(heap.continuation_rows(), vec![true, false, true]);
        heap.activation_set_continuation(middle, restart());
        assert_eq!(heap.continuation_rows(), vec![true, true, true]);
        heap.activation_set_continuation(middle, Continuation::Deliver);

        heap.pop_activation(top);
        assert_eq!(heap.continuation_rows(), vec![true]);
        heap.activation_set_continuation(middle, restart());
        assert_eq!(heap.continuation_rows(), vec![true, true]);
    }

    /// Replacing one continuation other than `Deliver` with another keeps
    /// its row.
    #[test]
    fn a_replaced_continuation_keeps_its_row() {
        let mut heap = Heap::new();
        let below = heap
            .push_activation(
                ActivationHeader {
                    then: Continuation::Unwind(Box::new(PendingUnwind {
                        target: None,
                        action: UnwindAction::Restart,
                    })),
                    ..header()
                },
                Arguments::NONE,
                0,
            )
            .expect("the stack holds the frame");
        heap.push_activation(
            ActivationHeader {
                then: Continuation::Unwind(Box::new(PendingUnwind {
                    target: None,
                    action: UnwindAction::Restart,
                })),
                ..header()
            },
            Arguments::NONE,
            0,
        )
        .expect("the stack holds the frame");

        heap.activation_set_continuation(
            below,
            Continuation::Unwind(Box::new(PendingUnwind {
                target: None,
                action: UnwindAction::Return(Value::small_integer(3)),
            })),
        );

        assert_eq!(heap.continuation_rows(), vec![true, true]);
        assert!(matches!(
            heap.continuations.get(heap.stack.continuation(below.offset)),
            Continuation::Unwind(pending) if matches!(pending.action, UnwindAction::Return(_))
        ));
        assert_eq!(
            heap.stack.continuation(below.offset),
            Some(ContinuationRow(0))
        );
    }

    #[test]
    fn a_delivering_activation_takes_no_row() {
        let mut heap = Heap::new();
        let activation = heap
            .push_activation(header(), Arguments::NONE, 0)
            .expect("the stack holds the frame");
        assert!(heap.continuation_rows().is_empty());
        let popped = heap.pop_activation(activation);
        assert!(matches!(popped.then, Continuation::Deliver));
    }

    #[test]
    fn a_resumed_activation_returns_to_its_suspended_sender() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Suspended");
        let bottom = heap
            .push_activation(
                ActivationHeader {
                    then: Continuation::Unwind(Box::new(PendingUnwind {
                        target: None,
                        action: UnwindAction::Restart,
                    })),
                    ..suspendable_header(class, 0, 1)
                },
                Arguments::NONE,
                1,
            )
            .expect("the stack holds the frame");
        let bottom_frame = heap.reify(bottom);
        let top = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let top_frame = heap.reify(top);
        heap.suspend_activations(0);

        let resumed_top = heap.resume_activation(top_frame, FeedbackBase::NOT_ASSIGNED);

        match heap
            .continuations
            .get(heap.stack.continuation(resumed_top.offset))
        {
            Continuation::ReturnToSuspended(suspended) => {
                assert!(suspended.sender.same(bottom_frame));
                assert!(matches!(suspended.then, Continuation::Deliver));
            }
            _ => panic!("the resumed top returns to its suspended sender"),
        }
        assert!(heap
            .frame_suspended_continuation(bottom_frame)
            .is_some_and(|then| matches!(then, Continuation::Unwind(_))));
        let Popped { then, .. } = heap.pop_activation(resumed_top);
        let Continuation::ReturnToSuspended(suspended) = then else {
            panic!("the popped top returns to its suspended sender")
        };
        let resumed_bottom = heap.resume_activation(suspended.sender, FeedbackBase::NOT_ASSIGNED);

        assert!(matches!(
            heap.continuations
                .get(heap.stack.continuation(resumed_bottom.offset)),
            Continuation::Unwind(pending)
                if pending.target.is_none() && matches!(pending.action, UnwindAction::Restart)
        ));
        heap.pop_activation(resumed_bottom);
    }

    /// Suspending again an activation that was resumed with a suspended
    /// sender keeps the sender, and the continuation the row keeps is the
    /// one inside the wrapper, never the wrapper itself.
    #[test]
    fn suspending_a_resumed_activation_again_keeps_its_suspended_sender() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Suspended");
        let bottom = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let bottom_frame = heap.reify(bottom);
        let top = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let top_frame = heap.reify(top);
        heap.suspend_activations(0);
        heap.resume_activation(top_frame, FeedbackBase::NOT_ASSIGNED);

        heap.suspend_activations(0);

        assert!(heap
            .frame_sender(top_frame)
            .is_some_and(|sender| sender.same(bottom_frame)));
        assert!(heap
            .frame_suspended_continuation(top_frame)
            .is_some_and(|then| matches!(then, Continuation::Deliver)));
        let resumed = heap.resume_activation(top_frame, FeedbackBase::NOT_ASSIGNED);
        heap.pop_activation(resumed);
        let resumed = heap.resume_activation(bottom_frame, FeedbackBase::NOT_ASSIGNED);
        heap.pop_activation(resumed);
    }

    /// Only the activations at or above the floor are suspended, and the
    /// lowest of them returns to the stack below it, so it names no sender.
    #[test]
    fn suspending_from_a_floor_leaves_the_activations_below_it_running() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Suspended");
        let below = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let lowest = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let lowest_frame = heap.reify(lowest);
        let floor = heap.activation_position(lowest);

        heap.suspend_activations(floor);

        assert_eq!(heap.activation_depth(), 1);
        assert!(heap.activation_reified(below).is_none());
        assert!(heap.frame_sender(lowest_frame).is_none());
        assert!(heap.suspend_activations(floor).is_none());
        let resumed = heap.resume_activation(lowest_frame, FeedbackBase::NOT_ASSIGNED);
        heap.pop_activation(resumed);
        heap.pop_activation(below);
    }

    #[test]
    fn the_depth_count_is_a_machine_word() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Counted");
        assert_eq!(heap.machine_word(MachineWord::DepthCount), 0);
        let mut pushed = Vec::new();
        for _ in 0..3 {
            pushed.push(
                heap.push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
                    .expect("the stack holds the frame"),
            );
        }
        assert_eq!(heap.machine_word(MachineWord::DepthCount), 3);
        assert_eq!(heap.counted_depth(), 3);
        heap.pop_activation(pushed.pop().expect("three were pushed"));
        assert_eq!(heap.machine_word(MachineWord::DepthCount), 2);
        assert_eq!(heap.counted_depth(), 2);
        while let Some(activation) = pushed.pop() {
            heap.pop_activation(activation);
        }
        assert_eq!(heap.machine_word(MachineWord::DepthCount), 0);
    }

    #[test]
    fn the_ordinary_end_follows_growth() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Growing");
        assert_eq!(heap.machine_word(MachineWord::OrdinaryStackEnd), 0);
        let mut ends = std::collections::BTreeSet::new();
        for _ in 0..100_000 {
            heap.push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 200)
                .expect("the stack holds the frame");
            assert_eq!(
                heap.machine_word(MachineWord::OrdinaryStackEnd),
                heap.ordinary_stack_end() as u64
            );
            ends.insert(heap.ordinary_stack_end());
            if ends.len() >= 3 {
                break;
            }
        }
        assert!(ends.len() >= 3, "the stack grew more than once");
    }

    #[test]
    fn suspended_activations_still_count_against_the_depth_limit() {
        let mut heap = frame_heap();
        let class = named_class(&mut heap, "Suspended");
        let bottom = heap
            .push_activation(
                ActivationHeader {
                    counted: false,
                    ..suspendable_header(class, 0, 1)
                },
                Arguments::NONE,
                1,
            )
            .expect("the stack holds the frame");
        let bottom_frame = heap.reify(bottom);
        let top = heap
            .push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let top_frame = heap.reify(top);
        heap.push_activation(suspendable_header(class, 0, 1), Arguments::NONE, 1)
            .expect("the stack holds the frame");
        let before = heap.counted_depth();
        assert_eq!(before, 2);

        let suspended = heap.suspend_activations(0).expect("a top frame");
        assert_eq!(heap.counted_depth(), before);
        let resumed = heap.resume_activation(suspended, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(heap.counted_depth(), before);

        heap.pop_activation(resumed);
        assert_eq!(heap.counted_depth(), before - 1);
        let resumed = heap.resume_activation(top_frame, FeedbackBase::NOT_ASSIGNED);
        heap.pop_activation(resumed);
        let resumed = heap.resume_activation(bottom_frame, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(heap.counted_depth(), 0);
        heap.pop_activation(resumed);
        assert_eq!(heap.counted_depth(), 0);
    }

    /// A suspended frame's registers are the only copy of its values, so
    /// every collector traces them, and the side table keeps the frame
    /// itself alive and forwarded.
    #[test]
    #[cfg(debug_assertions)]
    fn a_suspended_frame_survives_a_move_a_scavenge_and_a_collection_with_its_registers() {
        let mut heap = frame_heap();
        let class = old_instance_class(&mut heap);
        // Garbage below the frame, so that the compaction moves it.
        heap.allocate_string(class, crate::CodePoints::from("garbage"));
        let young = heap.allocate_string(class, crate::CodePoints::from("young"));
        let activation = heap
            .push_activation(
                suspendable_header(class, 1, 2),
                Arguments::Values(&[Value::object(young)].as_slice()),
                1,
            )
            .expect("the stack holds the frame");
        heap.reify(activation);
        let frame = heap.suspend_activations(0).expect("a top frame");
        let root = heap.stash(Value::object(heap.frame_object(frame)));
        assert!(space::is_young(frame.offset));

        move_bare_heap(&mut heap);
        let frame = frame_at_root(&heap, &root);
        assert_eq!(
            text_of(&heap, heap.frame_local(frame, 0)).as_deref(),
            Some("young")
        );
        scavenge_bare(&mut heap, false, &mut []);
        let frame = frame_at_root(&heap, &root);
        assert!(heap.frame_is_suspended(frame));
        assert_eq!(
            text_of(&heap, heap.frame_local(frame, 0)).as_deref(),
            Some("young")
        );
        heap.collect_for_test(&mut []);
        let frame = frame_at_root(&heap, &root);
        assert!(heap.frame_is_suspended(frame));
        assert_eq!(
            text_of(&heap, heap.frame_local(frame, 0)).as_deref(),
            Some("young")
        );
        drop(root);
        // Only the side table holds the frame now.
        heap.collect_for_test(&mut []);

        let frame = heap.suspensions.rows[0]
            .as_ref()
            .expect("the row of the suspended frame")
            .frame;
        let resumed = heap.resume_activation(frame, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(
            text_of(&heap, heap.activation_local(resumed, 0)).as_deref(),
            Some("young")
        );
        heap.pop_activation(resumed);
    }

    /// A suspended old frame is an old holder like any other: the
    /// suspension that copies a young register into it, and a store into
    /// one of its registers while it is suspended, both remember it, and
    /// the scavenge after them rewrites the registers it moved.
    #[test]
    fn a_young_value_written_into_a_suspended_old_frame_is_remembered() {
        let mut heap = frame_heap();
        let class = old_instance_class(&mut heap);
        let activation = heap
            .push_activation(
                suspendable_header(class, 0, first_above_header(0) + 2),
                Arguments::NONE,
                2,
            )
            .expect("the stack holds the frame");
        heap.reify(activation);
        scavenge_bare(&mut heap, true, &mut []);
        // A class of its own: the scavenge left `class` stale.
        let class = old_instance_class(&mut heap);
        let copied = heap.allocate_string(class, crate::CodePoints::from("copied"));
        heap.activation_set_local(activation, first_above_header(0), Value::object(copied));
        let frame = heap.suspend_activations(0).expect("a top frame");
        assert!(!space::is_young(frame.offset), "the frame was tenured");
        verify_remembered_set_if_debug(&heap);
        let stored = heap.allocate_string(class, crate::CodePoints::from("stored"));
        let stored_offset = stored.offset;
        heap.frame_set_local(frame, first_above_header(0) + 1, Value::object(stored));
        verify_remembered_set_if_debug(&heap);
        let mut frame_root = Value::object(heap.frame_object(frame));

        scavenge_bare(&mut heap, false, &mut [&mut frame_root]);

        let frame = heap.object_frame(rooted(frame_root)).expect("a frame");
        let resumed = heap.resume_activation(frame, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(
            text_of(&heap, heap.activation_local(resumed, first_above_header(0))).as_deref(),
            Some("copied")
        );
        let moved = heap.activation_local(resumed, first_above_header(0) + 1);
        assert_ne!(rooted(moved).offset, stored_offset, "the scavenge moved it");
        assert_eq!(text_of(&heap, moved).as_deref(), Some("stored"));
        heap.pop_activation(resumed);
    }

    /// A suspended activation was entered at its level as a running one
    /// was, and an own-slot access after it resumes reads that level's
    /// layout: the reclaim keeps the application while the activation is
    /// suspended, and frees it once nothing runs at it any more.
    #[test]
    #[cfg(debug_assertions)]
    fn the_reclaim_keeps_the_entered_level_of_a_suspended_activation() {
        let source = indoc::indoc! {"
            Newspeak3
            'Test'
            class Owner = (
            ) (
              public class Local = (
                | public slot |
              ) (
              )
            )
        "};
        let expect_ok = |result: crate::HostResult, what: &str| match result {
            Ok(root) => root,
            Err(crate::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(crate::HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        };
        let mut interpreter = crate::Interpreter::bootstrap();
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let owner = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&owner, &[]), "an Owner");
        let local = expect_ok(
            interpreter.send(&instance, Symbol::intern("Local"), &[]),
            "Local",
        );
        let (level, frame) = interpreter.enter(|mutator| {
            let class = mutator.fetch(&local).as_object().expect("a class object");
            let level = mutator
                .heap
                .class_application_index(class)
                .expect("an application");
            let object = mutator.builtin_class(crate::BuiltinClass::Object);
            let activation = mutator
                .heap
                .push_activation(
                    ActivationHeader {
                        kind: ActivationKind::Factory(object),
                        ..header()
                    },
                    Arguments::NONE,
                    1,
                )
                .expect("the stack holds the frame");
            // Entered at `Local`'s level, though the frame names no class
            // that keeps the level's application: only the side table can.
            mutator.heap.stack.set_header_word(
                activation.offset,
                frame_header::ENTERED_LEVEL,
                frames::number_word(level.index() as u64),
            );
            mutator.heap.reify(activation);
            let frame = mutator.heap.suspend_activations(0).expect("a top frame");
            (
                level,
                mutator.stash(Value::object(mutator.heap.frame_object(frame))),
            )
        });
        drop(local);
        drop(instance);

        interpreter.collect_now();

        interpreter.enter(|mutator| {
            assert!(mutator.heap.application(level).is_some());
            let frame = mutator
                .heap
                .object_frame(mutator.fetch(&frame).as_object().expect("an object"))
                .expect("a frame");
            let resumed = mutator
                .heap
                .resume_activation(frame, FeedbackBase::NOT_ASSIGNED);
            assert_eq!(mutator.heap.activation_entered_level(resumed), level);
            mutator.heap.pop_activation(resumed);
        });
        interpreter.collect_now();
        interpreter.enter(|mutator| {
            assert!(
                mutator.heap.application(level).is_none(),
                "nothing runs at the level any more"
            );
        });
    }

    // ---------- The collector's pre-pass ----------

    /// What a collection nils before it traces the stack
    /// (`Heap::trace_stack`). Each test builds its frames through the
    /// push API and its code by hand, and writes into a register the word
    /// a callee or an earlier frame would have left there: an object only
    /// that register holds, which a weak array then shows kept or gone.
    mod nil_leftovers {
        use super::*;
        use crate::bytecode::{
            leftovers, CallSite, CodeKind, CompiledCode, Instruction, Literal, HEADER_REGISTERS,
        };

        /// The first register above the header of a frame that takes no
        /// arguments.
        const FIRST: u16 = HEADER_REGISTERS;

        /// A body of `instructions` taking `argument_count` arguments,
        /// whose first `named` registers above the header are named
        /// temporaries and the rest of its `register_count` scratch. The
        /// analysis accepts it, as it accepts every compiled body.
        fn body(
            instructions: Vec<Instruction>,
            argument_count: u16,
            named: u16,
            register_count: u16,
        ) -> Rc<CompiledCode> {
            let mut code = CompiledCode::new(
                CodeKind::Method,
                instructions,
                argument_count,
                register_count,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            );
            code.local_count = argument_count + HEADER_REGISTERS + named;
            assert_eq!(
                leftovers::check(&code.instructions(), code.local_count, code.register_count),
                Ok(())
            );
            Rc::new(code)
        }

        fn send(
            receiver: u16,
            argument_base: u16,
            argument_sources: Vec<u16>,
            destination: u16,
        ) -> Instruction {
            Instruction::SendPublic {
                selector: Symbol::intern("m"),
                receiver_register: receiver,
                argument_base,
                argument_sources,
                destination,
                call_site: CallSite::UNCACHED,
            }
        }

        fn load(destination: u16) -> Instruction {
            Instruction::LoadLiteral {
                destination,
                literal: Literal::Nil,
            }
        }

        /// The header of an activation running `code`.
        fn running<'gc>(code: Rc<CompiledCode>) -> ActivationHeader<'gc> {
            ActivationHeader { code, ..header() }
        }

        /// A frame running `code`, its whole window pushed above the top.
        fn pushed<'gc>(
            heap: &mut Heap<'gc>,
            code: Rc<CompiledCode>,
            arguments: &[Value<'gc>],
        ) -> ActivationReference {
            let temporaries = code.registers_above_header();
            heap.push_activation(running(code), Arguments::Values(&arguments), temporaries)
                .expect("the stack holds the frame")
        }

        /// `count` fresh objects, each held by nothing yet.
        fn objects<'gc>(heap: &mut Heap<'gc>, count: usize) -> Vec<ObjectReference<'gc>> {
            let class = old_instance_class(heap);
            (0..count)
                .map(|_| heap.allocate_instance(class, 0))
                .collect()
        }

        /// A weak array over `objects`, as the value that roots it.
        fn weak_over<'gc>(heap: &mut Heap<'gc>, objects: &[ObjectReference<'gc>]) -> Value<'gc> {
            let class = old_instance_class(heap);
            let weak = heap.allocate_weak_array(class, objects.len());
            for (index, object) in objects.iter().enumerate() {
                assert!(heap.weak_array_set_element(weak, index, Value::object(*object)));
            }
            Value::object(weak)
        }

        /// Which of the weak array's elements still name an object.
        fn kept<'gc>(heap: &Heap<'gc>, weak: Value<'gc>) -> Vec<bool> {
            heap.weak_array_elements(rooted(weak))
                .expect("a weak array answers its elements")
                .iter()
                .map(|element| element.as_object().is_some())
                .collect()
        }

        /// A sender that sent with its receiver in r13 and its one slot in
        /// r14: from r15 up, its registers were the callee's window, and
        /// once the callee returned they hold what it left there.
        #[test]
        fn a_collection_nils_what_a_returned_callee_left() {
            let mut heap = Heap::new();
            let code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 1),
                    send(FIRST, FIRST + 1, vec![FIRST + 1], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 6,
            );
            let sender = pushed(&mut heap, code, &[]);
            heap.activation_set_program_counter(sender, 3);
            let left = objects(&mut heap, 1);
            let base = heap.activation_register_base(sender);
            heap.set_register(base, FIRST + 3, Value::object(left[0]));
            let mut weak = weak_over(&mut heap, &left);

            scavenge_bare(&mut heap, false, &mut [&mut weak]);

            assert_eq!(
                kept(&heap, weak),
                [false],
                "what the returned callee left is gone"
            );
            assert_eq!(
                heap.stack
                    .frames
                    .word(base.index as usize + usize::from(FIRST + 3)),
                dead_register().bits()
            );
            heap.pop_activation(sender);
        }

        /// A register the sender wrote after its send is its own again;
        /// one it has not written yet still holds the callee's leftover.
        #[test]
        fn a_collection_keeps_a_register_written_after_the_send() {
            let mut heap = Heap::new();
            let code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 1),
                    send(FIRST, FIRST + 1, vec![FIRST + 1], FIRST),
                    Instruction::Move {
                        destination: FIRST + 3,
                        source: FIRST,
                    },
                    Instruction::Return {
                        register: FIRST + 3,
                    },
                ],
                0,
                0,
                FIRST + 6,
            );
            let sender = pushed(&mut heap, code, &[]);
            heap.activation_set_program_counter(sender, 4);
            let held = objects(&mut heap, 2);
            let base = heap.activation_register_base(sender);
            heap.set_register(base, FIRST + 3, Value::object(held[0]));
            heap.set_register(base, FIRST + 4, Value::object(held[1]));
            let mut weak = weak_over(&mut heap, &held);

            scavenge_bare(&mut heap, false, &mut [&mut weak]);

            assert_eq!(kept(&heap, weak), [true, false]);
            heap.pop_activation(sender);
        }

        /// A named temporary is never tainted, whatever the body has
        /// written: the push fills it, and only the body writes it.
        #[test]
        fn a_collection_keeps_the_named_locals() {
            let mut heap = Heap::new();
            let code = body(
                vec![
                    Instruction::LoadSelf {
                        destination: FIRST + 2,
                    },
                    Instruction::Return {
                        register: FIRST + 2,
                    },
                ],
                0,
                2,
                FIRST + 4,
            );
            let activation = pushed(&mut heap, code, &[]);
            let held = objects(&mut heap, 3);
            let base = heap.activation_register_base(activation);
            for (register, object) in (FIRST..).zip(&held) {
                heap.set_register(base, register, Value::object(*object));
            }
            let mut weak = weak_over(&mut heap, &held);

            heap.collect_for_test(&mut [&mut weak]);

            assert_eq!(
                kept(&heap, weak),
                [true, true, false],
                "the named temporaries r13 and r14 are kept, the unwritten scratch r15 is not"
            );
            heap.pop_activation(activation);
        }

        /// A frame stopped at its send with the callee on its slots keeps
        /// what it wrote below them and the slots themselves, which are
        /// its callee's arguments; it loses a register below them it never
        /// wrote. The callee loses its own unwritten scratch, and a frame
        /// pushed above the callee keeps its argument.
        #[test]
        fn a_frame_in_flight_keeps_its_registers_below_its_slots() {
            let mut heap = Heap::new();
            let sender_code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 2),
                    send(FIRST, FIRST + 2, vec![FIRST + 2], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 27,
            );
            let callee_code = body(
                vec![Instruction::Return { register: 0 }],
                1,
                0,
                1 + FIRST + 2,
            );
            let top_code = body(vec![Instruction::Return { register: 0 }], 1, 0, 1 + FIRST);
            let held = objects(&mut heap, 5);
            let sender = pushed(&mut heap, sender_code, &[]);
            heap.activation_set_program_counter(sender, 2);
            let base = heap.activation_register_base(sender);
            heap.set_register(base, FIRST, Value::object(held[0]));
            heap.set_register(base, FIRST + 1, Value::object(held[1]));
            heap.set_register(base, FIRST + 2, Value::object(held[2]));
            let temporaries = callee_code.registers_above_header();
            let callee = heap
                .push_overlapped_activation(
                    &mut running(callee_code),
                    base,
                    FIRST + 2,
                    1,
                    temporaries,
                )
                .expect("the stack holds the frame");
            let callee_base = heap.activation_register_base(callee);
            heap.set_register(callee_base, 1 + FIRST, Value::object(held[3]));
            let callee_end = heap.stack.window_end(callee.offset);
            assert!(
                callee_end < heap.stack.window_end(sender.offset),
                "the callee's window ends inside its sender's"
            );
            let above = pushed(&mut heap, top_code, &[Value::object(held[4])]);
            assert_eq!(
                heap.activation_position(above),
                heap.stack.window_end(sender.offset),
                "a push never lowers the top, so a frame pushed above the callee starts past \
                 its sender's window"
            );
            let mut weak = weak_over(&mut heap, &held);

            heap.collect_for_test(&mut [&mut weak]);

            assert_eq!(
                kept(&heap, weak),
                [true, false, true, false, true],
                "the receiver and the slot are kept, the unwritten r14 and the callee's scratch \
                 are not, and the frame above the callee keeps its argument"
            );
            heap.pop_activation(above);
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// Where the top stands above a callee shorter than its sender,
        /// the sender's registers between the two window ends are the
        /// sender's, dead after its send: nilled, even one the sender
        /// wrote before the send.
        #[test]
        fn a_collection_nils_a_senders_registers_above_a_shorter_callee() {
            let mut heap = Heap::new();
            let written = FIRST + 22;
            let sender_code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    Instruction::Move {
                        destination: written,
                        source: FIRST,
                    },
                    load(FIRST + 1),
                    send(FIRST, FIRST + 1, vec![FIRST + 1], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 27,
            );
            let callee_code = body(vec![Instruction::Return { register: 0 }], 1, 0, 1 + FIRST);
            let held = objects(&mut heap, 1);
            let sender = pushed(&mut heap, sender_code, &[]);
            heap.activation_set_program_counter(sender, 3);
            let base = heap.activation_register_base(sender);
            heap.set_register(base, FIRST + 1, Value::nil());
            let callee = heap
                .push_overlapped_activation(&mut running(callee_code), base, FIRST + 1, 1, 0)
                .expect("the stack holds the frame");
            let callee_end = heap.stack.window_end(callee.offset);
            let sender_end = heap.stack.window_end(sender.offset);
            assert!(callee_end < base.index as usize + usize::from(written));
            // A push never lowers the top: it stays at the sender's window
            // end, the sender's registers above the callee's holding a word
            // they held before, as the one the sender wrote does.
            assert_eq!(heap.stack.frames.top(), sender_end);
            let stale = heap.store(Value::object(held[0])).bits();
            for word in callee_end..sender_end {
                heap.stack.frames.set_word(word, stale);
            }
            let mut weak = weak_over(&mut heap, &held);

            heap.collect_for_test(&mut [&mut weak]);

            assert_eq!(kept(&heap, weak), [false]);
            assert!((callee_end..sender_end)
                .all(|word| heap.stack.frames.word(word) == dead_register().bits()));
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// A frame standing at a loop's head, after a backward jump from a
        /// body that sends: what the head reads, and the receiver, are
        /// kept; what the body's send left above its slot is not, nor the
        /// slot, which the first iteration had not written yet.
        #[test]
        fn a_loop_head_keeps_what_it_reads() {
            let mut heap = Heap::new();
            let code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 1),
                    Instruction::BranchIfFalse {
                        register: FIRST + 1,
                        target: 6,
                    },
                    load(FIRST + 2),
                    send(FIRST, FIRST + 2, vec![FIRST + 2], FIRST + 2),
                    Instruction::Jump { target: 2 },
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 5,
            );
            let activation = pushed(&mut heap, code, &[]);
            heap.activation_set_program_counter(activation, 2);
            let held = objects(&mut heap, 4);
            let base = heap.activation_register_base(activation);
            for (register, object) in (FIRST..).zip(&held) {
                heap.set_register(base, register, Value::object(*object));
            }
            let mut weak = weak_over(&mut heap, &held);

            heap.collect_for_test(&mut [&mut weak]);

            assert_eq!(kept(&heap, weak), [true, true, false, false]);
            heap.pop_activation(activation);
        }

        /// A frame stopped at a send it runs again once a future is forced
        /// -- the forcing above its whole window -- keeps the source above
        /// the send's slot that the send reads again.
        #[test]
        fn a_rerun_instruction_takes_the_set_before_it() {
            let mut heap = Heap::new();
            let code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 3),
                    send(FIRST, FIRST + 1, vec![FIRST + 3], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 5,
            );
            let stopped = pushed(&mut heap, code, &[]);
            heap.activation_set_program_counter(stopped, 2);
            let held = objects(&mut heap, 4);
            let base = heap.activation_register_base(stopped);
            heap.set_register(base, FIRST, Value::object(held[0]));
            heap.set_register(base, FIRST + 3, Value::object(held[1]));
            heap.set_register(base, FIRST + 2, Value::object(held[2]));
            heap.set_register(base, FIRST + 4, Value::object(held[3]));
            let forcing = heap
                .push_activation(header(), Arguments::NONE, 0)
                .expect("the stack holds the frame");
            let mut weak = weak_over(&mut heap, &held);

            heap.collect_for_test(&mut [&mut weak]);

            assert_eq!(
                kept(&heap, weak),
                [true, true, false, false],
                "the receiver and the source the send reads again are kept, unwritten scratch \
                 is not"
            );
            heap.pop_activation(forcing);
            heap.pop_activation(stopped);
        }

        /// A frame just pushed has written none of its scratch, so a word
        /// a popped frame left there is gone; its arguments are kept.
        #[test]
        fn a_collection_nils_unwritten_scratch() {
            let mut heap = Heap::new();
            let code = body(
                vec![load(1 + FIRST), Instruction::Return { register: 0 }],
                1,
                0,
                1 + FIRST + 3,
            );
            let held = objects(&mut heap, 4);
            let activation = pushed(&mut heap, code, &[Value::object(held[0])]);
            let base = heap.activation_register_base(activation);
            for (register, object) in (1 + FIRST..).zip(&held[1..]) {
                heap.set_register(base, register, Value::object(*object));
            }
            let mut weak = weak_over(&mut heap, &held);

            scavenge_bare(&mut heap, false, &mut [&mut weak]);

            assert_eq!(kept(&heap, weak), [true, false, false, false]);
            heap.pop_activation(activation);
        }

        /// Writes into each of `registers` of `activation` an integer, a
        /// word the pre-pass may nil without any object depending on it.
        fn write_integers(heap: &mut Heap<'_>, activation: ActivationReference, registers: &[u16]) {
            let base = heap.activation_register_base(activation);
            for register in registers {
                heap.set_register(base, *register, Value::small_integer(i64::from(*register)));
            }
        }

        /// Which of `registers` of `activation` hold the pre-pass's word.
        fn dead_registers(
            heap: &Heap<'_>,
            activation: ActivationReference,
            registers: &[u16],
        ) -> Vec<u16> {
            let base = heap.activation_register_base(activation);
            registers
                .iter()
                .copied()
                .filter(|register| {
                    heap.stack
                        .frames
                        .word(base.index as usize + usize::from(*register))
                        == dead_register().bits()
                })
                .collect()
        }

        /// A frame whose body keeps no set yet is nilled once the walk is
        /// done, against the windows above it as the walk met them: here
        /// the sender is deferred below a callee on its slots whose set is
        /// kept, and it loses exactly what it loses once its own set is
        /// kept as well -- never a word of the callee's window, its
        /// argument the sender's slot.
        #[test]
        fn a_deferred_frame_below_a_claiming_frame_is_nilled_as_a_kept_one() {
            let mut heap = Heap::new();
            let sender_code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 2),
                    send(FIRST, FIRST + 2, vec![FIRST + 2], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 27,
            );
            let callee_code = body(
                vec![Instruction::Return { register: 0 }],
                1,
                0,
                1 + FIRST + 2,
            );
            // A frame of the callee's body at the program counter the
            // callee will stand at, so that its set is kept before the
            // sender's.
            let warm = pushed(&mut heap, Rc::clone(&callee_code), &[Value::nil()]);
            heap.collect_for_test(&mut []);
            heap.pop_activation(warm);
            assert!(callee_code.kept_tainted_before(0).is_some());
            assert!(sender_code.kept_tainted_before(2).is_none());

            let sender = pushed(&mut heap, Rc::clone(&sender_code), &[]);
            heap.activation_set_program_counter(sender, 2);
            // The send's slot, the callee's argument, written before the
            // push reads it.
            write_integers(&mut heap, sender, &[FIRST + 2]);
            let base = heap.activation_register_base(sender);
            let temporaries = callee_code.registers_above_header();
            let callee = heap
                .push_overlapped_activation(
                    &mut running(Rc::clone(&callee_code)),
                    base,
                    FIRST + 2,
                    1,
                    temporaries,
                )
                .expect("the stack holds the frame");
            assert_eq!(
                heap.stack.frames.top(),
                heap.stack.window_end(sender.offset),
                "the top stands at the sender's window end, above the callee's"
            );
            // Every register of the sender but those of the callee's
            // header, which lies on the sender's registers from r16.
            let callee_registers = FIRST + 3 + FIRST;
            let registers: Vec<u16> = (FIRST..FIRST + 3)
                .chain(callee_registers..FIRST + 27)
                .collect();
            let expected: Vec<u16> = std::iter::once(FIRST + 1)
                .chain(callee_registers..FIRST + 27)
                .collect();

            write_integers(&mut heap, sender, &registers);
            heap.collect_for_test(&mut []);
            assert_eq!(
                dead_registers(&heap, sender, &registers),
                expected,
                "deferred: the unwritten r14, the callee's scratch and the sender's registers \
                 above the callee are nilled; the receiver and the slot are kept"
            );
            assert!(sender_code.kept_tainted_before(2).is_some());

            write_integers(&mut heap, sender, &registers);
            heap.collect_for_test(&mut []);
            assert_eq!(
                dead_registers(&heap, sender, &registers),
                expected,
                "kept: the same registers"
            );
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// Frames that repeat a body and program counter share one lookup,
        /// and one that differs in either looks its own set up: the frames
        /// of one body at two program counters, interleaved with another
        /// body's, each lose what their own set says, both when every set
        /// is missing and when every set is kept.
        #[test]
        fn repeated_and_interleaved_frames_take_their_own_sets() {
            let mut heap = Heap::new();
            let sending = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 1),
                    send(FIRST, FIRST + 1, vec![FIRST + 1], FIRST),
                    Instruction::Move {
                        destination: FIRST + 3,
                        source: FIRST,
                    },
                    Instruction::Return {
                        register: FIRST + 3,
                    },
                ],
                0,
                0,
                FIRST + 6,
            );
            let looping = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 1),
                    Instruction::BranchIfFalse {
                        register: FIRST + 1,
                        target: 6,
                    },
                    load(FIRST + 2),
                    send(FIRST, FIRST + 2, vec![FIRST + 2], FIRST + 2),
                    Instruction::Jump { target: 2 },
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 5,
            );
            // Bottom to top; the walk meets them the other way round.
            // The top two share a program counter but not a body, so that
            // a memo keyed on the program counter alone fails.
            let layout: [(&Rc<CompiledCode>, usize); 7] = [
                (&sending, 4),
                (&sending, 4),
                (&sending, 3),
                (&looping, 2),
                (&sending, 3),
                (&sending, 4),
                (&looping, 4),
            ];
            let expected = |code: &Rc<CompiledCode>, program_counter: usize| -> Vec<u16> {
                match (Rc::ptr_eq(code, &sending), program_counter) {
                    (true, 4) => vec![FIRST + 2, FIRST + 4, FIRST + 5],
                    (true, 3) => vec![FIRST + 2, FIRST + 3, FIRST + 4, FIRST + 5],
                    (false, 2) => vec![FIRST + 2, FIRST + 3, FIRST + 4],
                    (false, 4) => vec![FIRST + 3, FIRST + 4],
                    _ => unreachable!("a frame of the layout"),
                }
            };
            let mut activations = Vec::new();
            for (code, program_counter) in layout {
                let activation = pushed(&mut heap, Rc::clone(code), &[]);
                heap.activation_set_program_counter(activation, program_counter);
                activations.push(activation);
            }
            for round in ["every set missing", "every set kept"] {
                for (activation, (code, _)) in activations.iter().zip(layout) {
                    let registers: Vec<u16> = (FIRST..code.register_count).collect();
                    write_integers(&mut heap, *activation, &registers);
                }
                heap.collect_for_test(&mut []);
                for (activation, (code, program_counter)) in activations.iter().zip(layout) {
                    let registers: Vec<u16> = (FIRST..code.register_count).collect();
                    assert_eq!(
                        dead_registers(&heap, *activation, &registers),
                        expected(code, program_counter),
                        "{round}: the frame at {program_counter}"
                    );
                }
            }
            for activation in activations.into_iter().rev() {
                heap.pop_activation(activation);
            }
        }

        /// A sender stopped at its send, its unwritten r14 tainted, and a
        /// callee on its slots from r15 whose named temporary lies in the
        /// registers the send taints for the sender -- claimed, so the
        /// callee's to decide, and live. Answers the stack words of the
        /// sender's tainted register and of the callee's named one.
        fn sender_with_a_claiming_callee(
            heap: &mut Heap<'_>,
        ) -> (ActivationReference, ActivationReference, usize, usize) {
            let sender_code = body(
                vec![
                    Instruction::LoadSelf { destination: FIRST },
                    load(FIRST + 2),
                    send(FIRST, FIRST + 2, vec![FIRST + 2], FIRST),
                    Instruction::Return { register: FIRST },
                ],
                0,
                0,
                FIRST + 27,
            );
            let callee_code = body(
                vec![
                    Instruction::LoadSelf {
                        destination: 1 + FIRST,
                    },
                    Instruction::Return {
                        register: 1 + FIRST,
                    },
                ],
                1,
                1,
                1 + FIRST + 2,
            );
            let sender = pushed(heap, sender_code, &[]);
            heap.activation_set_program_counter(sender, 2);
            let base = heap.activation_register_base(sender);
            heap.set_register(base, FIRST + 2, Value::nil());
            let temporaries = callee_code.registers_above_header();
            let callee = heap
                .push_overlapped_activation(
                    &mut running(callee_code),
                    base,
                    FIRST + 2,
                    1,
                    temporaries,
                )
                .expect("the stack holds the frame");
            heap.activation_set_program_counter(callee, 1);
            let callee_base = heap.activation_register_base(callee);
            let tainted = base.index as usize + usize::from(FIRST + 1);
            let claimed = callee_base.index as usize + usize::from(1 + FIRST);
            assert!(
                claimed > callee.offset as usize,
                "the callee's named register lies above its header, in the registers the send \
                 taints for the sender"
            );
            (sender, callee, tainted, claimed)
        }

        /// Writes `object` into the stack word `index`, as a frame's
        /// register.
        fn write_word<'gc>(heap: &mut Heap<'gc>, index: usize, object: ObjectReference<'gc>) {
            let bits = heap.store(Value::object(object)).bits();
            heap.stack.frames.set_word(index, bits);
        }

        /// The scavenge's trace of the stack nils a frame's tainted
        /// register before it traces the frame, so the object only that
        /// register holds is not kept, and leaves the word a frame above
        /// claims, whose object it keeps: when every set is missing and the
        /// frames are deferred, and when every set is kept.
        #[test]
        fn the_scavenge_traces_no_tainted_register_and_keeps_a_claimed_one() {
            let mut heap = Heap::new();
            let (sender, callee, tainted, claimed) = sender_with_a_claiming_callee(&mut heap);
            for round in ["every set missing", "every set kept"] {
                let held = objects(&mut heap, 2);
                write_word(&mut heap, tainted, held[0]);
                write_word(&mut heap, claimed, held[1]);
                let mut weak = weak_over(&mut heap, &held);

                scavenge_bare(&mut heap, false, &mut [&mut weak]);

                assert_eq!(kept(&heap, weak), [false, true], "{round}");
                assert_eq!(heap.stack.frames.word(tainted), dead_register().bits());
                assert_ne!(heap.stack.frames.word(claimed), dead_register().bits());
            }
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// Marking's trace of the stack nils a frame's tainted register
        /// before it marks through the frame, and leaves a claimed one.
        #[test]
        fn marking_traces_no_tainted_register_and_keeps_a_claimed_one() {
            let mut heap = Heap::new();
            let (sender, callee, tainted, claimed) = sender_with_a_claiming_callee(&mut heap);
            for round in ["every set missing", "every set kept"] {
                let held = objects(&mut heap, 2);
                write_word(&mut heap, tainted, held[0]);
                write_word(&mut heap, claimed, held[1]);
                let mut weak = weak_over(&mut heap, &held);

                heap.collect_for_test(&mut [&mut weak]);

                assert_eq!(kept(&heap, weak), [false, true], "{round}");
                assert_eq!(heap.stack.frames.word(tainted), dead_register().bits());
                assert_ne!(heap.stack.frames.word(claimed), dead_register().bits());
            }
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// A forward's trace of the stack nils a frame's tainted register
        /// before it forwards the frame's words: a handle there to an
        /// object marking left unmarked -- stale once the compaction
        /// reclaims it -- is never forwarded, which a debug build's
        /// forwarding table would refuse, while the claimed word is
        /// forwarded to its object's new place.
        #[test]
        fn a_forward_traces_no_tainted_register_and_forwards_a_claimed_one() {
            let mut heap = Heap::new();
            let (sender, callee, tainted, claimed) = sender_with_a_claiming_callee(&mut heap);
            for round in ["every set missing", "every set kept"] {
                // Garbage below the two objects, so that the compaction
                // moves them.
                objects(&mut heap, 4);
                let held = objects(&mut heap, 2);
                write_word(&mut heap, claimed, held[1]);
                let mut weak = weak_over(&mut heap, &held);

                use crate::trace::Tracer;
                let mut marker = heap.begin_marking();
                marker.value(&mut weak);
                heap.drain_marking(&mut marker);
                heap.finish_marking(&mut marker);
                heap.prune_canonical_symbols();
                heap.prune_mixin_objects();
                // Written after marking, which nilled the register, so that
                // only the forward's own pass can nil it.
                write_word(&mut heap, tainted, held[0]);
                let mut forwarder = heap.compact();
                forwarder.value(&mut weak);

                assert_eq!(kept(&heap, weak), [false, true], "{round}");
                assert_eq!(heap.stack.frames.word(tainted), dead_register().bits());
                let elements = heap
                    .weak_array_elements(rooted(weak))
                    .expect("a weak array answers its elements");
                assert_eq!(
                    heap.stack.frames.word(claimed),
                    heap.store(elements[1]).bits(),
                    "{round}: the claimed word names its object where the compaction put it"
                );
            }
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// A forward that is the first trace of the stack -- the bulk
        /// forward behind `become:`, with no marking before it -- meets
        /// every set missing and defers the frames: it nils their tainted
        /// registers all the same, and forwards the claimed word. Then
        /// again with every set kept.
        #[test]
        fn a_forward_with_every_set_missing_nils_the_deferred_frames() {
            let mut heap = Heap::new();
            let (sender, callee, tainted, claimed) = sender_with_a_claiming_callee(&mut heap);
            let sender_code = heap.activation_code(sender);
            let callee_code = heap.activation_code(callee);
            assert!(sender_code.kept_tainted_before(2).is_none());
            assert!(callee_code.kept_tainted_before(1).is_none());
            for round in ["every set missing", "every set kept"] {
                let held = objects(&mut heap, 3);
                write_word(&mut heap, tainted, held[0]);
                write_word(&mut heap, claimed, held[1]);

                let forwarder = heap
                    .forward_identity(&held[1..2], &held[2..3])
                    .expect("the forward is allowed");
                drop(forwarder);

                assert_eq!(
                    heap.stack.frames.word(tainted),
                    dead_register().bits(),
                    "{round}: the deferred sender's tainted register is nilled"
                );
                assert_eq!(
                    heap.stack.frames.word(claimed),
                    heap.store(Value::object(held[2])).bits(),
                    "{round}: the claimed word names the forward's replacement"
                );
                assert!(sender_code.kept_tainted_before(2).is_some());
            }
            heap.pop_activation(callee);
            heap.pop_activation(sender);
        }

        /// The walk's transient memory per deferred frame, which
        /// `docs/design/memory.md` bounds ("What the collector traces").
        #[test]
        fn a_deferred_frame_is_72_bytes() {
            assert_eq!(std::mem::size_of::<DeferredFrame>(), 72);
        }
    }
}
