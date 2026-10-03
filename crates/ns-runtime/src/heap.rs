//! The heap seam. Every read and write of an `Object` or an activation goes
//! through `Heap`, so that replacing what sits behind a handle changes this
//! file rather than its call sites. Objects and reified frames live in arenas
//! of typed handles (`ObjectReference`, `FrameReference`); see
//! `docs/design/memory.md`. A running activation lives on the activation stack,
//! also owned here, and is named by an `ActivationReference` that nothing may
//! store; a frame exists for it only once `reify` creates one. The accessor
//! prefix says which handle an accessor takes: `activation_*` a live stack
//! entry, `frame_*` a reified frame. See `docs/design/execution.md`,
//! "Activations".
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/locals.rs`.

use crate::bytecode::CompiledCode;
use crate::interpreter::FeedbackBase;
use crate::mixin::{DispatchId, DispatchIds, Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::Symbol;
use std::rc::Rc;

mod header;
pub(crate) mod mark;
mod object;
mod records;
mod tables;
mod value;
mod world;
pub(crate) use header::IDENTITY_HASH_LIMIT;
use header::{Format, Header};
use object::{ClassData, ClosureData, Frame, FrameSlot, Object, ObjectData, PackedString};
pub use records::{FrameRecord, NewClass, NewClosure, ObjectShape, Record};
pub use tables::{ApplicationIndex, CodeId, MixinId};
pub(crate) use tables::{CodeEntry, Tables};
pub(crate) use value::is_small_float_bits;
use value::{at_rest, branded};
pub use value::{
    fits_small_integer, Immediate, ImmediateKind, Value, ValueKind, Word, SMALL_INTEGER_MAX,
    SMALL_INTEGER_MIN,
};
pub(crate) use world::World;

/// Hands every `Heap` a distinct id, so a handle can name the heap that
/// allocated it. Debug builds only -- the id and the field on each handle
/// both compile out of a release build, where a handle is exactly its
/// 32-bit index.
#[cfg(debug_assertions)]
fn next_heap_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The brand every movable handle carries (`docs/design/memory.md`, "The
/// brand"). Invariant in `'gc` -- a `fn(&'gc ()) -> &'gc ()` is
/// contravariant in its argument and covariant in its result -- so the
/// compiler can neither shorten nor extend it, and a handle minted in one
/// entry has a type no other entry can name. Zero-sized: a release handle
/// is still exactly its 32-bit index.
type Brand<'gc> = std::marker::PhantomData<fn(&'gc ()) -> &'gc ()>;

/// An opaque handle to an object: an index into `Heap.objects`. The inner
/// index is private to this module, and only `heap.rs` reaches it -- that
/// privacy is what made replacing `Rc<RefCell<Object>>` with this a change
/// to one file rather than to every caller. See `docs/design/memory.md`,
/// "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it.
/// Compaction does reuse a slot once nothing reaches the object that held
/// it, but a collection rewrites every handle it can find into the new
/// arrangement, and the debug-build epoch below turns a handle still naming
/// the old one into a panic rather than a silent hit on whatever object took
/// the slot -- so two live objects never share an index.
#[derive(Clone, Copy)]
pub struct ObjectReference<'gc> {
    index: u32,
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
    /// whatever object took its slot (`docs/design/memory.md`, "The move
    /// stress mode").
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
    /// two unrelated objects that happen to share an index would be reported
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
        self.index == other.index
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` recursing through the bootstrap `Object`
/// class, which is its own `.class`; an index has nothing to descend into,
/// so the recursion hazard is gone by construction. It still renders Rust
/// internals, which is why `CLAUDE.md`'s "Never `{:?}` a `Value`" rule
/// stands: `Heap::describe` is what a Newspeak-facing message needs.
impl std::fmt::Debug for ObjectReference<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectReference(#{})", self.index)
    }
}

/// A class is an `ObjectReference` whose `data` is `ObjectData::Class(_)`. Not
/// enforced by the type system: `ClassReference` is an alias, not a distinct
/// handle type.
pub type ClassReference<'gc> = ObjectReference<'gc>;

/// An opaque handle to a *reified* activation: an index into `Heap.frames`.
/// The inner index is private to this module. See `docs/design/memory.md`,
/// "The seam".
///
/// A running activation is named by an `ActivationReference` instead; a
/// `FrameReference` exists for one only once something must hold it past the
/// current call (`Heap::reify`). This is the only activation handle anything
/// may store -- in a closure, in another frame, in an image.
///
/// A handle is meaningful only against the `Heap` that allocated it.
/// Compaction does reuse a slot once nothing reaches the frame that held it,
/// but a collection rewrites every handle it can find into the new
/// arrangement, and the debug-build epoch below turns a handle still naming
/// the old one into a panic rather than a silent hit on whatever frame took
/// the slot -- so two frames never share an index.
#[derive(Clone, Copy)]
pub struct FrameReference<'gc> {
    index: u32,
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
    /// whatever object took its slot (`docs/design/memory.md`, "The move
    /// stress mode").
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
        self.index == other.index
    }
}

/// Prints the arena index and nothing else. A derived `Debug` over a
/// frame's contents would walk the sender chain, which has no acyclic
/// guarantee to lean on (a closure's `home` can itself be captured); an
/// index has nothing to descend into at all, so the hazard is gone by
/// construction rather than by discipline, and printing one cannot panic.
impl std::fmt::Debug for FrameReference<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameReference(#{})", self.index)
    }
}

/// An opaque handle to a *live* activation: an entry on the `Heap`'s activation
/// stack. Where a `FrameReference` names a reified activation that may be
/// stored anywhere, an `ActivationReference` is valid only until its entry is
/// popped -- the next activation at that depth reuses the index. That is why
/// nothing stores one: `ClosureData`, `Frame` and every image record take a
/// `FrameReference`, which `Heap::reify` answers for a live activation. See
/// `docs/design/execution.md`, "Activations", "Representation".
///
/// 4 bytes in a release build. A debug build adds the heap's id, like every
/// handle, and a `generation` drawn at each push, so that reading through a
/// handle whose entry was popped panics instead of answering for whichever
/// activation reuses the index.
#[derive(Clone, Copy)]
pub struct ActivationReference {
    index: u32,
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    generation: u32,
}

impl ActivationReference {
    /// Activation identity: the unwinder stops at its target by it
    /// (`interpreter::unwind`). Two handles at the same index are the same
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
                self.index != other.index || self.generation == other.generation,
                "stale activation handle: compared against a later activation at the same depth"
            );
        }
        self.index == other.index
    }
}

/// Prints the stack index and nothing else, for the reason `FrameReference`'s
/// does.
impl std::fmt::Debug for ActivationReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActivationReference(#{})", self.index)
    }
}

/// Where a running activation's registers start in the activation stack's
/// register file: Cog's `localFP`, held by the run loop so that a register
/// access is one index (`docs/design/execution.md`, "The loop over locals").
/// It is an index, not a pointer, so it survives the register file growing
/// and a move, which touches no register window; it carries the danger an
/// `ActivationReference` does -- once its activation is popped, the next one
/// at that depth reuses the window -- and a debug build checks every access
/// against its activation's generation, as `check_activation` does.
#[derive(Clone, Copy)]
pub struct RegisterBase {
    index: u32,
    #[cfg(debug_assertions)]
    activation: ActivationReference,
}

/// Everything a new activation is apart from its registers, which
/// `Heap::push_activation` takes separately.
pub struct ActivationHeader<'gc> {
    pub receiver: Value<'gc>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6).
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: FrameKind<'gc>,
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

/// What popping an activation hands back to the run loop.
pub struct Popped<'gc> {
    /// The popped activation's continuation.
    pub then: Continuation<'gc>,
    /// Whether the popped activation counted against the depth limit: the
    /// next block of a native loop may be exempt only if this one was.
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

/// One live activation. Its registers are `values[base..]` up to the next
/// entry's `base` (or the end of `values`, for the top entry): the
/// arguments first, then the temporaries.
struct StackEntry<'gc> {
    receiver: Value<'gc>,
    current_class: Option<ClassReference<'gc>>,
    kind: FrameKind<'gc>,
    base: u32,
    argument_count: u32,
    program_counter: usize,
    /// See `ActivationHeader::feedback_base`.
    feedback_base: FeedbackBase,
    code: std::rc::Rc<crate::bytecode::CompiledCode>,
    then: Continuation<'gc>,
    /// See `ActivationHeader::counted`.
    counted: bool,
    /// The frame `Heap::reify` created for this activation, married to it
    /// until the entry is popped.
    reified: Option<FrameReference<'gc>>,
    /// Whether this activation's registers must survive its return: set by
    /// `retain_registers` when it creates a closure that reads enclosing
    /// frames by reference. Nothing else can still read them once it has
    /// returned -- a converted closure carries what it captured -- so an
    /// unmarked activation leaves its frame with no registers at all
    /// (`docs/design/execution.md`, "Converted representation").
    retains_registers: bool,
    #[cfg(debug_assertions)]
    generation: u32,
}

/// Every live activation, innermost last. Both vectors keep their capacity
/// across pops, so once they have grown to the program's depth a push
/// allocates nothing.
#[derive(Default)]
struct ActivationStack<'gc> {
    values: Vec<Word>,
    entries: Vec<StackEntry<'gc>>,
    /// How many of `entries` count against the depth limit. Kept as a
    /// running total, since the limit is checked on every push.
    counted: usize,
    #[cfg(debug_assertions)]
    next_generation: u32,
}

/// The initial value of `Heap::collection_floor`, and of
/// `next_object_collection` and `next_frame_collection` before anything has
/// collected (`docs/design/memory.md`, "The trigger and the growth policy").
const COLLECTION_FLOOR: usize = 4096;

pub struct Heap<'gc> {
    /// The next identity hash to hand out. Hashes are assigned lazily, the
    /// first time `object_ensure_identity_hash` is asked for one, and never derived
    /// from an address or an arena index, so identity survives both a moving
    /// collector and a snapshot (`docs/design/memory.md`) and allocation
    /// order stays unobservable (`docs/design/object-model.md`).
    next_identity_hash: u32,
    /// Every *reified* activation, addressed by a `FrameReference`'s index: one
    /// `Heap::reify` created because a block captured it, or one the image
    /// decoder rebuilt. Nothing is removed except by a collection's
    /// compaction, which shrinks this back to its live set and rewrites
    /// every surviving `FrameReference`. An activation that is never
    /// reified never reaches this arena at all.
    frames: Vec<FrameSlot<'gc>>,
    /// Every live activation. A stack entry is popped when its activation
    /// returns, so -- unlike `frames` and `objects` -- this is bounded by
    /// the deepest the program has been, not by everything it has run.
    stack: ActivationStack<'gc>,
    /// Every object, addressed by an `ObjectReference`'s index. Nothing is
    /// removed except by a collection's compaction, for the same reason as
    /// `frames`.
    objects: Vec<Object<'gc>>,
    /// The one object each symbol is, indexed by `Symbol::index` (§5.1.6:
    /// equal symbols are one object; `docs/design/object-model.md`,
    /// "Values and objects"). Filled lazily by `Mutator::new_symbol`, the one
    /// way to make a symbol, and by the image decoder. Per heap, because a
    /// handle means nothing against another heap. The system's one weak
    /// structure: not a root for marking, pruned by
    /// `prune_canonical_symbols` between marking and compaction, and
    /// forwarded by `compact` like everything else that survives
    /// (`docs/design/memory.md`, "Weak references").
    canonical_symbols: Vec<Option<ObjectReference<'gc>>>,
    /// Every `Root` a host holds. A `RefCell` so that `stash` and `fetch`
    /// take `&self`, which lets a host stash an argument inside the call that
    /// sends it.
    host_roots: std::cell::RefCell<HostRoots>,
    /// Whether the run loop's next safepoint must take its slow path
    /// (`Interpreter::safepoint`). The poll is one compare of this flag
    /// (`docs/design/memory.md`, "Where a collection can happen"). An
    /// allocation that reaches its arena's limit sets it, in every build
    /// (`request_collection`); in a debug build a stress mode additionally
    /// holds it set for its own reasons, for as long as the mode is on.
    /// Otherwise the safepoint that takes a request clears it.
    safepoint_requested: bool,
    /// The arena length at which the next allocation asks for a collection.
    /// Checked against the length the allocation already has in hand, so the
    /// hot path pays one predictable branch and carries no counter.
    next_object_collection: usize,
    next_frame_collection: usize,
    /// Whether a limit has been reached since the last collection. Separate
    /// from `safepoint_requested`, which only means "take the slow path" and
    /// which a stress mode holds set for its own reasons.
    collection_requested: bool,
    /// The smallest limit a collection will set. A program that stays under
    /// it never collects: `bench_loop.ns` and `bench_send.ns` run in 4.6 MB
    /// and should not pay for a collector they do not need.
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
    /// The mixins, applications and compiled bodies objects of this heap
    /// name by index (`tables.rs`).
    tables: Tables,
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

/// Prints the counter and how many frames, activations, objects and host
/// roots there are, never their contents. A
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
            .field("frames.len()", &self.frames.len())
            .field("activations", &self.stack.entries.len())
            .field("objects.len()", &self.objects.len())
            .field("canonical_symbols.len()", &self.canonical_symbols.len())
            .field("host roots", host_roots)
            .finish()
    }
}

/// How a move maps an old slot index to its new one. Two answer it: the
/// stress mode's `Rotation`, which keeps every slot, and the collector's
/// `ForwardingTable`, which keeps only the live ones. A type parameter
/// rather than an enum or a trait object, because `apply` runs once per
/// handle in the whole heap and a monomorphized call leaves neither a
/// branch nor an indirection there.
pub(crate) trait Relocation {
    fn apply(&self, index: u32) -> u32;
}

/// A rotation of one arena: slot `i` moves to `(i + shift) % length`.
#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct Rotation {
    shift: u32,
    length: u32,
}

#[cfg(debug_assertions)]
impl Rotation {
    /// A shift in `1..length`, varied by `epoch` so that moves do not all
    /// shift alike; two of them can still compose to no shift at all, which
    /// the epoch covers. No slot keeps its index in a move, so a handle the
    /// tracer missed both carries an old epoch and names a different
    /// object: the permutation checks the forwarding, the epoch the
    /// completeness. An arena of one slot or none cannot move anything.
    fn new(length: usize, epoch: u32) -> Rotation {
        let length =
            u32::try_from(length).expect("internal: an arena is addressed by 32-bit indices");
        let shift = if length < 2 {
            0
        } else {
            1 + epoch % (length - 1)
        };
        Rotation { shift, length }
    }
}

#[cfg(debug_assertions)]
impl Relocation for Rotation {
    fn apply(&self, index: u32) -> u32 {
        if self.length == 0 {
            return index;
        }
        ((u64::from(index) + u64::from(self.shift)) % u64::from(self.length)) as u32
    }
}

/// What a collection maps each old slot index to: its new index, or `DEAD`
/// for a slot that is gone. A flat table rather than a run-length list of
/// shifts (Nova's `CompactionList`) because an index arena can afford 4
/// bytes per slot for the length of one collection, and O(1) beats a binary
/// search once per handle in the heap.
pub(crate) struct ForwardingTable {
    table: Vec<u32>,
}

/// No live slot can map here: an arena is addressed by 32-bit indices and
/// `next_object_index` refuses to mint `u32::MAX` of them.
const DEAD: u32 = u32::MAX;

impl Relocation for ForwardingTable {
    fn apply(&self, index: u32) -> u32 {
        let destination = self.table[index as usize];
        debug_assert_ne!(
            destination, DEAD,
            "a handle to a collected slot survived marking: a root the tracer does not know about"
        );
        destination
    }
}

/// Slides the marked slots of one arena to the front, truncates the rest
/// away -- which is where the dead records drop, and their payloads with
/// them -- and answers the table that maps old index to new. `floor` is the
/// heap's collection floor, which the arena's next limit never falls below
/// (`collection_limit`).
fn compact_arena<T>(slots: &mut Vec<T>, bits: &mark::MarkBits, floor: usize) -> ForwardingTable {
    let mut table = vec![DEAD; slots.len()];
    let mut destination = 0usize;
    for (source, destination_of_source) in table.iter_mut().enumerate() {
        if bits.is_set(source as u32) {
            *destination_of_source = u32::try_from(destination)
                .expect("internal: an arena is addressed by 32-bit indices");
            if destination != source {
                slots.swap(destination, source);
            }
            destination += 1;
        }
    }
    slots.truncate(destination);
    // `truncate` drops the dead records but never lowers the capacity, so
    // without this the pages a collection frees stay with the process and
    // the peak this step exists to lower does not move. With hysteresis, so
    // that a heap which oscillates does not reallocate and copy at every
    // collection: shrink only when the buffer is more than twice the next
    // limit, and leave room for the limit itself. Measured against the
    // limit rather than the live set, because under the floor the two part:
    // a live set of 72 shrank the buffer to 144 slots, and the next cycle,
    // allowed to reach 4096, grew it back through five reallocations, a copy
    // and fresh pages each -- at every collection, about 5% of an
    // allocating benchmark (`docs/design/memory.md`, "The trigger and the
    // growth policy").
    let limit = collection_limit(floor, destination);
    if slots.capacity() > limit.saturating_mul(2) {
        slots.shrink_to(limit);
    }
    ForwardingTable { table }
}

/// How long an arena may grow after a collection that left `live` slots in
/// it before the next one is requested: no more is allocated between two
/// collections than is live, so the marking work per allocated slot is
/// amortized constant, and never less than `floor`, which keeps a small
/// program out of the collector altogether.
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
    frames: M,
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
        handle.index = self.objects.apply(handle.index);
    }

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
        handle.index = self.frames.apply(handle.index);
    }

    /// A stored word carries no origin or epoch to check: `Heap::store`
    /// checked it when it was written, and every move rewrites it here.
    fn word(&mut self, word: &mut Word) {
        if let Some(index) = word.object_index() {
            *word = Word::with_object_index(self.objects.apply(index));
        }
    }
}

impl crate::trace::Trace for StackEntry<'_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let StackEntry {
            receiver,
            current_class,
            kind,
            base: _,
            argument_count: _,
            program_counter: _,
            feedback_base: _, // not a handle
            code: _,
            then,
            counted: _,
            reified,
            retains_registers: _, // not a handle
            #[cfg(debug_assertions)]
                generation: _,
        } = self;
        tracer.value(receiver);
        if let Some(class) = current_class {
            tracer.object(class);
        }
        kind.trace(tracer);
        then.trace(tracer);
        if let Some(frame) = reified {
            tracer.frame(frame);
        }
    }
}

/// The arenas' occupancy, read without disturbing them
/// (`Heap::heap_statistics`): `live_*` is the arena's length, `*_slots` its
/// capacity -- the gap between them is what a collection can still give
/// back before either arena grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapStatistics {
    pub live_objects: usize,
    pub object_slots: usize,
    pub live_frames: usize,
    pub frame_slots: usize,
}

/// The collector's counts and pauses, read without disturbing them
/// (`Heap::collector_statistics`): `collections`, `total_pause` and
/// `longest_pause` accumulate across every `World::collect` so far;
/// `live_objects`/`live_frames` are the arenas' occupancy right now, the
/// same fields `HeapStatistics` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorStatistics {
    pub collections: usize,
    pub total_pause: std::time::Duration,
    pub longest_pause: std::time::Duration,
    pub live_objects: usize,
    pub live_frames: usize,
}

impl<'gc> Heap<'gc> {
    /// A `Heap` built on the spot -- rather than the one `Interpreter` owns --
    /// is an empty arena: every object and frame accessor indexes directly into
    /// `self.objects`/`self.frames`, so an `ObjectReference` or
    /// `FrameReference` minted by a different `Heap` either falls out of range
    /// and panics, or -- should the two arenas happen to share a length --
    /// names a different object or activation. In a debug build the second case
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
        Heap {
            next_identity_hash: 1,
            frames: Vec::new(),
            stack: ActivationStack::default(),
            objects: Vec::new(),
            canonical_symbols: Vec::new(),
            host_roots: std::cell::RefCell::new(HostRoots {
                slots: Vec::new(),
                sweep_at: MIN_ROOT_SWEEP,
            }),
            safepoint_requested: false,
            next_object_collection: COLLECTION_FLOOR,
            next_frame_collection: COLLECTION_FLOOR,
            collection_requested: false,
            collection_floor: COLLECTION_FLOOR,
            collections: 0,
            total_pause: std::time::Duration::ZERO,
            longest_pause: std::time::Duration::ZERO,
            immediate_dispatch_ids: [DispatchId::NONE; 5],
            tables: Tables::default(),
            #[cfg(debug_assertions)]
            heap_id: next_heap_id(),
            #[cfg(debug_assertions)]
            epoch: 0,
        }
    }

    /// Draws the next number from the counter, which wraps from the widest
    /// hash the header word holds back to 1. Hashes are therefore not
    /// unique once the counter has wrapped, which a hash need not be:
    /// object identity is the arena index (`ObjectReference::same`), never
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

    /// The number the next identity hash will be, for a caller that must
    /// show nothing drew one meanwhile: `image::load` resolves platform
    /// paths, which runs shipped code, before it restores the hashes an
    /// image recorded, and debug-asserts that the counter stood still.
    #[cfg(debug_assertions)]
    pub(crate) fn next_identity_hash(&self) -> u32 {
        self.next_identity_hash
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
    // builds the payload it names here, so that replacing what backs a
    // handle only ever touches the heap module.

    /// Allocates `data` under `class`, through `push_object`, so the
    /// collection limit applies. It carries no identity hash until
    /// something asks for one (`object_ensure_identity_hash`), so allocating
    /// draws nothing from the counter. Private to the heap: callers name the
    /// kind of object they build (`allocate_instance` and its siblings).
    fn allocate_object(
        &mut self,
        class: ClassReference<'gc>,
        data: ObjectData<'gc>,
    ) -> ObjectReference<'gc> {
        let dispatch_id = self.header_dispatch_id(class);
        self.push_object(Object::new(class, dispatch_id, 0, data))
    }

    /// An instance of `class` with `field_count` fields, every one nil.
    pub fn allocate_instance(
        &mut self,
        class: ClassReference<'gc>,
        field_count: usize,
    ) -> ObjectReference<'gc> {
        self.allocate_object(
            class,
            ObjectData::instance(std::iter::repeat_n(Word::NIL, field_count)),
        )
    }

    /// An `Array`-shaped object of `class` holding `elements`, in order.
    pub fn allocate_array(
        &mut self,
        class: ClassReference<'gc>,
        elements: &[Value<'gc>],
    ) -> ObjectReference<'gc> {
        let data = ObjectData::array(elements.iter().map(|element| self.store(*element)));
        self.allocate_object(class, data)
    }

    /// A String of `class` holding `text`.
    pub fn allocate_string(
        &mut self,
        class: ClassReference<'gc>,
        text: crate::CodePoints,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, ObjectData::string(&text))
    }

    /// A Symbol object of `class` for `symbol`. `Mutator::new_symbol` is the
    /// one caller, since a heap holds one object per symbol (§5.1.6).
    pub(crate) fn allocate_symbol(
        &mut self,
        class: ClassReference<'gc>,
        symbol: Symbol,
    ) -> ObjectReference<'gc> {
        self.allocate_object(
            class,
            ObjectData::string(&crate::CodePoints::from(symbol.as_str())),
        )
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
        self.allocate_object(class, ObjectData::large_integer(&integer))
    }

    /// A boxed `Float` of `class` holding `value`. `Mutator::new_float` is
    /// the one caller, and decides that `value` lies outside the immediate
    /// range, so each float has one representation.
    pub(crate) fn allocate_float(
        &mut self,
        class: ClassReference<'gc>,
        value: f64,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, ObjectData::float(value))
    }

    /// A block closure of `class` (`NewClosure` says what it holds).
    pub(crate) fn allocate_closure(
        &mut self,
        class: ClassReference<'gc>,
        closure: NewClosure<'gc>,
    ) -> ObjectReference<'gc> {
        let data = self.closure_payload(closure);
        self.allocate_object(class, ObjectData::Closure(data))
    }

    /// A closure's payload from its record, each copied value stored as a
    /// word.
    fn closure_payload(&self, closure: NewClosure<'gc>) -> ClosureData<'gc> {
        let NewClosure {
            code,
            captured_frame,
            copied,
            feedback_base,
        } = closure;
        self.check_frame_origin(captured_frame);
        ClosureData {
            code,
            captured_frame,
            copied: copied.into_iter().map(|value| self.store(value)).collect(),
            feedback_base,
        }
    }

    /// A class object (`NewClass` says what it holds) whose header class is
    /// `metaclass`. Its own dispatch id is `metaclass`'s instances id, as
    /// every object's is its class's.
    pub fn allocate_class(
        &mut self,
        metaclass: ClassReference<'gc>,
        class: NewClass<'gc>,
    ) -> ClassReference<'gc> {
        let data = Self::class_payload(class);
        self.allocate_object(metaclass, ObjectData::Class(Box::new(data)))
    }

    /// A class object's payload from its record. Every field set after
    /// creation starts empty: `this_class`, `applied_clause` and the
    /// value-class verdict each have their setter.
    fn class_payload(class: NewClass<'gc>) -> ClassData<'gc> {
        let NewClass {
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin,
        } = class;
        ClassData::new(application, enclosing, declaring_class, superclass, builtin)
    }

    /// A class object that is genuinely its own `.class`. Under `Rc` this
    /// needed `Rc::new_uninit`, a manual strong-count bump and
    /// `Rc::from_raw`, because `Object.class` is non-optional and
    /// `Rc::new_cyclic` only ever offers a `Weak` that fails to upgrade until
    /// construction finishes. An arena has no such problem: the slot's index
    /// is known before the slot is filled, so the object can name itself in
    /// safe Rust.
    ///
    /// Like every object, it carries no identity hash until one is asked
    /// for.
    pub fn allocate_self_referential_class(&mut self, class: NewClass<'gc>) -> ClassReference<'gc> {
        let data = Self::class_payload(class);
        self.allocate_self_referential(ObjectData::Class(Box::new(data)))
    }

    /// An object whose class is itself, holding `data`. Private: besides
    /// `allocate_self_referential_class`, the heap's own tests build one that
    /// is no class object, the shape a crafted image can give a class
    /// reference.
    fn allocate_self_referential(&mut self, data: ObjectData<'gc>) -> ObjectReference<'gc> {
        let index = self.next_object_index();
        let handle = self.object_handle(index);
        // Its own class is itself, not yet in the arena to read, so the id
        // comes from `data` alone: its own instances id when `data` is a
        // class -- it is an instance of itself -- and `NONE` otherwise: the
        // object is not a class object and, being its own class, has no
        // class that could supply an instances id.
        let dispatch_id = match &data {
            ObjectData::Class(own) => own.dispatch_id,
            _ => DispatchId::NONE,
        };
        self.objects.push(Object::new(handle, dispatch_id, 0, data));
        handle
    }

    /// An instance of the kernel-private `Activation` class over `frame`,
    /// which the caller has already reified: an object keeps a
    /// `FrameReference`, never an `ActivationReference`, since only a
    /// reified frame survives its activation's pop.
    pub(crate) fn allocate_activation(
        &mut self,
        class: ClassReference<'gc>,
        frame: FrameReference<'gc>,
    ) -> ObjectReference<'gc> {
        self.check_frame_origin(frame);
        self.allocate_object(class, ObjectData::Activation(frame))
    }

    /// An object for `image::decode::decode_graph_section`'s first pass to
    /// fill later (`fill_placeholder`): an instance of `class` with no
    /// fields, safe to hold and never observed before it is filled.
    pub(crate) fn allocate_placeholder(
        &mut self,
        class: ClassReference<'gc>,
    ) -> ObjectReference<'gc> {
        self.allocate_object(class, ObjectData::instance([]))
    }

    /// Replaces the entirety of a placeholder object's contents in one
    /// write: `class`, `identity_hash` and the payload `record` names.
    /// `image::decode::decode_graph_section`'s second pass is the only
    /// caller, and it only ever calls this on a handle its first pass
    /// placeholder-allocated and nothing else has observed yet -- before
    /// `check_class_references` and `verify_graph` run, both of which assume
    /// every object is already filled in. Call this on any handle something
    /// else already holds and it silently replaces that object's
    /// already-verified invariants with unverified ones: no error, and no
    /// way for verification to notice, because verification has already
    /// finished.
    ///
    /// The object carries `NONE` in its header until `stamp_dispatch_ids`
    /// runs -- `class` may itself be a still-placeholder record with no
    /// application yet -- and must not be dispatched on before then.
    pub(crate) fn fill_placeholder(
        &mut self,
        object: ObjectReference<'gc>,
        class: ClassReference<'gc>,
        identity_hash: u32,
        record: Record<'gc>,
    ) {
        self.check_object_origin(object);
        self.check_object_origin(class);
        let data = match record {
            Record::Instance(fields) => {
                ObjectData::instance(fields.into_iter().map(|value| self.store(value)))
            }
            Record::String(text) => ObjectData::string(&text),
            Record::Symbol(symbol) => ObjectData::string(&crate::CodePoints::from(symbol.as_str())),
            Record::Array(elements) => {
                ObjectData::array(elements.into_iter().map(|value| self.store(value)))
            }
            Record::Closure(closure) => ObjectData::Closure(self.closure_payload(closure)),
            Record::Class(class) => ObjectData::Class(Box::new(Self::class_payload(class))),
            Record::Activation(frame) => {
                self.check_frame_origin(frame);
                ObjectData::Activation(frame)
            }
        };
        self.objects[object.index as usize] =
            Object::new(class, DispatchId::NONE, identity_hash, data);
    }

    /// Where every allocation but `allocate_self_referential` enters an
    /// object into the arena; that one pushes directly
    /// instead, because it needs the index before the object exists to name
    /// itself as its own `class`, but it enforces the same ceiling by
    /// calling `next_object_index` itself. The ceiling is the
    /// representation's, not an implementation limit: a handle is 32 bits
    /// by design (`docs/design/memory.md`), so a heap holding more than
    /// `u32::MAX` objects cannot be addressed at all, and failing loudly at
    /// the allocation beats handing out an index that wrapped.
    fn push_object(&mut self, object: Object<'gc>) -> ObjectReference<'gc> {
        let index = self.next_object_index();
        self.objects.push(object);
        if self.objects.len() >= self.next_object_collection {
            self.request_collection();
        }
        self.object_handle(index)
    }

    fn next_object_index(&self) -> u32 {
        u32::try_from(self.objects.len()).expect("more than u32::MAX objects allocated in one heap")
    }

    /// The one place an `ObjectReference` is built, so the `cfg` on `origin`
    /// appears once rather than at every allocation. A struct-expression field
    /// may carry a `cfg` attribute, which is what makes the release build see a
    /// plain 4-byte handle.
    fn object_handle(&self, index: u32) -> ObjectReference<'gc> {
        ObjectReference {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: std::marker::PhantomData,
        }
    }

    fn frame_handle(&self, index: u32) -> FrameReference<'gc> {
        FrameReference {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            epoch: self.epoch,
            brand: std::marker::PhantomData,
        }
    }

    // ---------- Frame construction ----------
    //
    // A frame carries no identity hash -- Newspeak's `identityHash` applies
    // to objects, not activation records. `reify` builds a real activation's
    // frame; `image::decode::decode_graph_section`'s first pass allocates a
    // placeholder (`allocate_placeholder_frame`) and its second pass fills
    // it wholesale (`fill_frame`).

    /// Allocates `frame`. Every `FrameReference` in the workspace is minted
    /// here -- `reify`, the placeholder the image decoder fills and the test
    /// support below all come through it -- so that replacing what backs a
    /// handle only ever touches the heap module.
    ///
    /// `Frame.home` is what `closure_home` reads back for a closure created
    /// in this frame, and the image records it, so it must be populated
    /// exactly for a `FrameKind::Closure`
    /// activation. `reify` upholds that by building `home` from the entry's
    /// `kind`, but nothing in the type system ties the two fields together,
    /// so a new constructor added later -- or a mismatched pair of literal
    /// fields at an existing one -- would record a return target that
    /// disagrees with the frame's own closure silently rather than fail
    /// loudly. Checked here, once, rather than at every constructor. Not
    /// checked in `fill_frame`: the decoder fills a placeholder through
    /// that path with fields read off the wire, and nothing reactivates a
    /// decoded frame, so it need not satisfy this.
    fn allocate_frame(&mut self, frame: Frame<'gc>) -> FrameReference<'gc> {
        debug_assert_eq!(
            frame.home.is_some(),
            matches!(frame.kind, FrameKind::Closure(_)),
            "Frame.home must be populated exactly for a closure activation"
        );
        let index = u32::try_from(self.frames.len())
            .expect("more than u32::MAX activations allocated in one heap");
        self.frames.push(FrameSlot { frame, live: None });
        self.frame_handle(index)
    }

    /// A frame for `image::decode::decode_graph_section`'s first pass to
    /// fill later (`fill_frame`): a top-level activation with no registers,
    /// safe to hold and never observed before it is filled.
    pub(crate) fn allocate_placeholder_frame(&mut self) -> FrameReference<'gc> {
        self.allocate_frame(Frame {
            receiver: Word::NIL,
            arguments: Vec::new(),
            temporaries: Vec::new(),
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
        })
    }

    /// A returned top-level frame no activation produced, built from values.
    /// Test support, public only for `tests/image.rs`'s `sender` round trip:
    /// the runtime never sets a frame's `sender`, so only a hand-made frame
    /// carries one into the encoder.
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
        let frame = Frame {
            receiver: self.store(receiver),
            arguments: Vec::new(),
            temporaries: temporaries
                .iter()
                .map(|temporary| self.store(*temporary))
                .collect(),
            program_counter: 0,
            sender,
            home: None,
            current_class,
            kind: FrameKind::TopLevel,
            uncontinuable: true,
        };
        self.allocate_frame(frame)
    }

    // ---------- Handle-origin checks ----------
    //
    // Called as the first line of every accessor below that takes a handle:
    // an index from another heap is in range and names a different, valid
    // object or activation, so without this the read would just answer the
    // wrong thing. `describe` checks the object it was handed and does not
    // re-check the class it reaches through, which by construction came out
    // of the same heap.

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
    /// stack handle needs: that its entry is still the one it was minted
    /// for. Compiles to nothing in a release build.
    #[inline]
    fn check_activation(&self, activation: ActivationReference) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                activation.origin, self.heap_id,
                "handle belongs to a different heap (handle from heap {}, read through heap {})",
                activation.origin, self.heap_id
            );
            let live = self
                .stack
                .entries
                .get(activation.index as usize)
                .map(|entry| entry.generation);
            assert_eq!(
                live,
                Some(activation.generation),
                "stale activation handle: its activation has returned"
            );
        }
        let _ = activation;
    }

    /// The handle for live entry `index`, minted with that entry's own
    /// generation. The one place an `ActivationReference` is built.
    fn activation_handle(&self, index: u32) -> ActivationReference {
        ActivationReference {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            generation: self.stack.entries[index as usize].generation,
        }
    }

    /// Where register `index` of live entry `entry` sits in `values`. The
    /// window check is what stops an out-of-range index from silently reading
    /// the neighbouring activation's register, which is in bounds of
    /// `values` and so would not panic on its own.
    fn register_slot(&self, entry: u32, index: u16) -> usize {
        let entries = &self.stack.entries;
        let slot = entries[entry as usize].base as usize + index as usize;
        debug_assert!(
            slot < entries
                .get(entry as usize + 1)
                .map_or(self.stack.values.len(), |next| next.base as usize),
            "register {index} is outside its activation's register window"
        );
        slot
    }

    // ---------- Object accessors ----------
    //
    // `&self`/`&mut self` index into `self.objects`, so the borrow checker
    // ties every accessor's result to the `Heap` it read from -- the same
    // discipline `docs/design/memory.md` describes for the frame arena.

    /// The identity hash stored on `object`, 0 if nothing has asked for one
    /// yet. A read never assigns one: the image encoder writes exactly what is
    /// stored, so snapshotting a world does not change it.
    pub fn object_identity_hash(&self, object: ObjectReference<'gc>) -> u32 {
        self.check_object_origin(object);
        self.objects[object.index as usize].header.identity_hash()
    }

    /// `object`'s identity hash, drawn from the counter the first time anything
    /// asks -- `Object>>hash` is the one caller. Assigned lazily, as Cog/Spur
    /// (`hashBitsOf:`, which calls `newHashBitsOf:` when the bits are 0) and
    /// Primordial Soup (`Object_identityHash`) do, so that allocation order is
    /// unobservable: the reference lowering allocates closures the optimized
    /// one does not, and closure conversion and a JIT's escape analysis will
    /// add and remove allocations the same way (`docs/design/object-model.md`).
    pub fn object_ensure_identity_hash(&mut self, object: ObjectReference<'gc>) -> u32 {
        self.check_object_origin(object);
        let stored = self.objects[object.index as usize].header.identity_hash();
        if stored != 0 {
            return stored;
        }
        let hash = self.allocate_identity_hash();
        let entry = &mut self.objects[object.index as usize];
        entry.header = entry.header.with_identity_hash(hash);
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
    /// different heaps carry the same index and so produce the same key, which
    /// collides silently in a caller's map; requiring the `Heap` that minted
    /// the handle is what turns that into a panic, and it is the compiler that
    /// requires it, since there is no way to reach the index without one.
    ///
    /// This and `frame_identity_key` are the only ways a handle's index
    /// leaves this module, so anything that changes what a handle encodes --
    /// widening it to carry a heap tag, say -- has to check their callers
    /// even though the field itself is private. They all treat the result as
    /// an opaque key, which is what keeps that a check rather than a rewrite.
    pub(crate) fn object_identity_key(&self, object: ObjectReference<'gc>) -> usize {
        self.check_object_origin(object);
        object.index as usize
    }

    /// Overwrites `object`'s identity hash. Two legitimate callers:
    /// `image::decode::decode_tag_named_hashes`, which puts a recorded hash
    /// back onto a builtin class, the root class, `platform` or an object a
    /// platform path names -- none of them a record, so nothing else could
    /// restore what the image remembered for one -- and `tests/image.rs`'s
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
        self.check_object_origin(object);
        debug_assert!(hash < IDENTITY_HASH_LIMIT);
        let entry = &mut self.objects[object.index as usize];
        entry.header = entry.header.with_identity_hash(hash);
    }

    pub fn object_class(&self, object: ObjectReference<'gc>) -> ClassReference<'gc> {
        self.check_object_origin(object);
        self.objects[object.index as usize].class
    }

    /// Which kind of payload `object` carries. The encoder and the graph
    /// walk branch on it, then read the payload through the accessor for
    /// that kind.
    pub fn object_shape(&self, object: ObjectReference<'gc>) -> ObjectShape {
        self.check_object_origin(object);
        let entry = &self.objects[object.index as usize];
        match &entry.data {
            ObjectData::Words { .. } => match entry.header.format() {
                Format::FixedPointers => ObjectShape::Instance,
                Format::IndexablePointers => ObjectShape::Array,
                Format::Bytes | Format::ThirtyTwoBit if self.symbol_value(object).is_some() => {
                    ObjectShape::Symbol
                }
                Format::Bytes | Format::ThirtyTwoBit => ObjectShape::String,
                _ if self.is_boxed_float(object) => ObjectShape::Float,
                _ => ObjectShape::LargeInteger,
            },
            ObjectData::Closure(_) => ObjectShape::Closure,
            ObjectData::Class(_) => ObjectShape::Class,
            ObjectData::Activation(_) => ObjectShape::Activation,
        }
    }

    pub fn is_class(&self, object: ObjectReference<'gc>) -> bool {
        self.check_object_origin(object);
        self.objects[object.index as usize].header.format() == Format::Class
    }

    /// The words of a word-format object of `format`: the one lender of a
    /// pointer payload, private to the heap, so that every public accessor
    /// copies out (`docs/design/memory.md`, "The seam").
    fn pointer_words(&self, object: ObjectReference<'gc>, format: Format) -> Option<&[u64]> {
        self.check_object_origin(object);
        let entry = &self.objects[object.index as usize];
        match &entry.data {
            ObjectData::Words { words, .. } if entry.header.format() == format => Some(words),
            _ => None,
        }
    }

    /// `pointer_words`, for a write.
    fn pointer_words_mut(
        &mut self,
        object: ObjectReference<'gc>,
        format: Format,
    ) -> Option<&mut [u64]> {
        self.check_object_origin(object);
        let entry = &mut self.objects[object.index as usize];
        match &mut entry.data {
            ObjectData::Words { words, .. } if entry.header.format() == format => Some(words),
            _ => None,
        }
    }

    /// The pointer word at `index` of a word-format object of `format`;
    /// `None` for any other object or past its words.
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
        self.check_object_origin(object);
        let entry = &self.objects[object.index as usize];
        match &entry.data {
            ObjectData::Words { words, .. } => PackedString::new(entry.header, words),
            _ => None,
        }
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

    /// A number object's words, its header and whether it is a boxed
    /// `Float`; `None` for any other object. A `LargeInteger` and a boxed
    /// `Float` share `Format::SixtyFourBit`, so the class's builtin tells
    /// them apart -- and only `Float`'s is asked for, so that a
    /// `LargeInteger` reads by its format whatever its class.
    fn number_words(&self, object: ObjectReference<'gc>) -> Option<(bool, Header, &[u64])> {
        self.check_object_origin(object);
        let entry = &self.objects[object.index as usize];
        match &entry.data {
            ObjectData::Words { words, .. } if entry.header.format() == Format::SixtyFourBit => {
                let float = self.class_builtin(entry.class) == Some(BuiltinClass::Float);
                Some((float, entry.header, words))
            }
            _ => None,
        }
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
    /// object, a String of the same text included. The object holds only
    /// its text, so this finds the symbol again by it -- never interning:
    /// `Mutator::new_symbol` and the image decoder intern a text before
    /// they allocate its object.
    pub fn symbol_value(&self, object: ObjectReference<'gc>) -> Option<Symbol> {
        if !self.is_symbol(object) {
            return None;
        }
        Symbol::lookup(&self.str_value(object)?)
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
        self.canonical_symbols[index] = Some(object);
    }

    /// Closure `object`'s payload, borrowed, or `None` when `object` is not a
    /// closure. Private, like every borrow of the arena: the accessors
    /// below each copy one thing out of it, so that none of their callers
    /// holds a borrow of the heap.
    fn closure(&self, object: ObjectReference<'gc>) -> Option<&ClosureData<'gc>> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Closure(closure) => Some(closure),
            _ => None,
        }
    }

    /// Whether `object` is a closure -- what a control-flow step asks of a
    /// value before activating it as a block.
    pub fn is_closure(&self, object: ObjectReference<'gc>) -> bool {
        self.closure(object).is_some()
    }

    /// How many parameters closure `object` declares, or `None` when `object`
    /// is not a closure. Read off the body rather than a parameter-name list
    /// of the closure's own: one source, so the arity a call is checked
    /// against and the arity the register file is sized from cannot disagree.
    pub fn closure_parameter_count(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.closure(object)
            .map(|closure| closure.code.argument_count as usize)
    }

    /// Closure `object`'s compiled body, or `None` when `object` is not a
    /// closure. `image::decode::frame_body` resolves a closure frame's body by
    /// it.
    pub fn closure_code(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<std::rc::Rc<crate::bytecode::CompiledCode>> {
        self.closure(object).map(|closure| closure.code.clone())
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
        self.closure(object).map(|closure| {
            (
                closure.code.clone(),
                closure.captured_frame,
                closure.feedback_base,
            )
        })
    }

    /// Writes closure `object`'s copied values into `activation`'s registers,
    /// immediately after its arguments, which is where a block's compiled
    /// code addresses them (`docs/design/execution.md`, "Converted
    /// representation"). Does nothing for a non-closure, and stops at the
    /// end of the register file: `decode`'s `CopiedCountOutOfRange` keeps a
    /// decoded closure's list inside its body's registers, and this is what
    /// keeps a future gap there from becoming a panic on an untrusted image.
    ///
    /// `Step::Block` runs this on every block activation, so it allocates
    /// nothing: the two arenas are borrowed field by field rather than
    /// through `closure`, which borrows the whole `Heap`, so each value goes
    /// straight from the object arena into its register with no intermediate
    /// buffer.
    pub(crate) fn fill_copied_values(
        &mut self,
        activation: ActivationReference,
        object: ObjectReference<'gc>,
    ) {
        self.check_activation(activation);
        self.check_object_origin(object);
        let Heap { objects, stack, .. } = self;
        let ObjectData::Closure(closure) = &objects[object.index as usize].data else {
            return;
        };
        if closure.copied.is_empty() {
            return;
        }
        let entry = &stack.entries[activation.index as usize];
        let first = entry.base as usize + entry.argument_count as usize;
        // The activation's own window, so a list longer than its register
        // file stops at the end of it rather than writing into whatever
        // sits above.
        let end = stack
            .entries
            .get(activation.index as usize + 1)
            .map_or(stack.values.len(), |above| above.base as usize);
        for (position, value) in closure.copied.iter().enumerate() {
            let slot = first + position;
            if slot >= end {
                break;
            }
            stack.values[slot] = *value;
        }
    }

    /// Closure `object`'s payload, to write; `None` when `object` is not a
    /// closure.
    fn closure_mut(&mut self, object: ObjectReference<'gc>) -> Option<&mut ClosureData<'gc>> {
        self.check_object_origin(object);
        match &mut self.objects[object.index as usize].data {
            ObjectData::Closure(closure) => Some(closure),
            _ => None,
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
        if let Some(closure) = self.closure_mut(object) {
            closure.feedback_base = feedback_base;
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
        self.closure(object).map(|closure| closure.captured_frame)
    }

    /// Closure `object`'s copied values, in order, or `None` when `object`
    /// is not a closure.
    pub fn closure_copied_values(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.closure(object)
            .map(|closure| closure.copied.iter().map(|word| self.load(*word)).collect())
    }

    /// A closure's home: the activation a `^` in its body returns from.
    /// Derived rather than stored, so that `ClosureData` stays 32 bytes --
    /// the captured frame itself when it is a method's, an initializer's or a
    /// clause's, its own `home` when it is a block's.
    ///
    /// A closure frame the runtime reified always carries a `home`
    /// (`allocate_frame` asserts it), so the fallback below is reached only
    /// by a *decoded* closure frame, which `fill_frame` fills straight
    /// from the wire. Answering the captured frame for one is what keeps an
    /// untrusted image from panicking here: a decoded frame is never married
    /// to an activation, so a `^` aimed at it is the `cannotReturn` error
    /// `raise_return` already raises for a dead home.
    pub fn closure_home(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>> {
        let captured = self.closure(object)?.captured_frame;
        self.check_frame_origin(captured);
        let slot = &self.frames[captured.index as usize];
        Some(match &slot.frame.kind {
            FrameKind::Closure(_) => slot.frame.home.unwrap_or(captured),
            _ => captured,
        })
    }

    /// The frame activation object `object` reifies, or `None` when `object`
    /// is not an activation object. `None` rather than a panic, so that a
    /// kernel leaf handed some other object by a program answers a failure
    /// instead of aborting.
    pub fn activation_object_frame(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<FrameReference<'gc>> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Activation(frame) => Some(*frame),
            _ => None,
        }
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
            ValueKind::SmallFloat(float) => float.to_string(),
            ValueKind::Object(object) => {
                self.check_object_origin(object);
                let entry = &self.objects[object.index as usize];
                match &entry.data {
                    ObjectData::Words { .. } if self.is_string(object) => {
                        match self.symbol_value(object) {
                            Some(symbol) => format!("the Symbol #{symbol}"),
                            None => format!(
                                "the String {:?}",
                                self.str_value(object).unwrap_or_default()
                            ),
                        }
                    }
                    ObjectData::Words { words, .. }
                        if entry.header.format() == Format::IndexablePointers =>
                    {
                        format!("an Array of {}", words.len())
                    }
                    ObjectData::Words { .. } if entry.header.format() == Format::SixtyFourBit => {
                        match self.float_value(value) {
                            // As an immediate float describes: which of the
                            // two a float is, is no business of an error
                            // message.
                            Some(float) => float.to_string(),
                            // Bare digits, as a `SmallInteger` describes, for
                            // the same reason.
                            None => self
                                .large_integer(object)
                                .map(|integer| integer.to_decimal_string())
                                .unwrap_or_default(),
                        }
                    }
                    ObjectData::Closure(_) => "a Block".to_string(),
                    ObjectData::Activation(_) => "an Activation".to_string(),
                    ObjectData::Class(_) => {
                        format!("the class {}", self.class_display_name(object))
                    }
                    ObjectData::Words { .. } => {
                        let class = &self.objects[object.index as usize].class;
                        match &self.objects[class.index as usize].data {
                            ObjectData::Class(data) => {
                                format!("an instance of {}", data.application.mixin.name)
                            }
                            // `ClassReference`'s doc: every
                            // `Object::class` is an `ObjectReference` whose own
                            // `data` is `ObjectData::Class`, by convention
                            // rather than by the type system. This arm exists
                            // only so the match is total, not because a
                            // conforming object can reach it.
                            _ => "an object".to_string(),
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

    // ---------- Class accessors ----------
    //
    // Each answers `None` for a handle whose data is not a class, rather
    // than panicking: a class reference decoded from an image is untrusted
    // until verified, and the verifier is a caller of these.

    // ---------- The Rust-side tables ----------

    pub(crate) fn mixin_id(&mut self, mixin: &Rc<Mixin>) -> MixinId {
        let key = crate::mixin::mixin_key(mixin);
        if let Some(id) = self.tables.mixin_ids.get(&key) {
            return *id;
        }
        let id = MixinId::from_index(self.tables.mixins.len())
            .expect("more than u32::MAX mixins in one heap");
        self.tables.mixins.push(mixin.clone());
        self.tables.mixin_ids.insert(key, id);
        id
    }

    pub fn mixin(&self, id: MixinId) -> Option<Rc<Mixin>> {
        self.tables.mixins.get(id.index()).cloned()
    }

    /// Enters `application` into the table, stamped with its index, and
    /// answers the shared handle every class object of it will name.
    pub(crate) fn register_application(
        &mut self,
        mut application: MixinApplication,
    ) -> Rc<MixinApplication> {
        let index = ApplicationIndex::from_index(self.tables.applications.len())
            .expect("more than u32::MAX applications in one heap");
        application.index = index;
        let application = Rc::new(application);
        self.tables.applications.push(application.clone());
        application
    }

    pub fn application(&self, index: ApplicationIndex) -> Option<Rc<MixinApplication>> {
        self.tables.applications.get(index.index()).cloned()
    }

    pub fn code_id(&mut self, code: &Rc<CompiledCode>) -> CodeId {
        self.register_code(code, None)
    }

    pub fn code_id_of_method(&mut self, method: &Rc<MixinMethod>) -> CodeId {
        self.register_code(&method.code, Some(method))
    }

    fn register_code(
        &mut self,
        code: &Rc<CompiledCode>,
        method: Option<&Rc<MixinMethod>>,
    ) -> CodeId {
        let key = Rc::as_ptr(code) as usize;
        if let Some(id) = self.tables.code_ids.get(&key).copied() {
            let entry = &mut self.tables.codes[id.index()];
            if entry.method.is_none() {
                entry.method = method.cloned();
            }
            return id;
        }
        let id = CodeId::from_index(self.tables.codes.len())
            .expect("more than u32::MAX compiled bodies in one heap");
        self.tables.codes.push(CodeEntry {
            code: code.clone(),
            method: method.cloned(),
        });
        self.tables.code_ids.insert(key, id);
        id
    }

    pub fn code(&self, id: CodeId) -> Option<Rc<CompiledCode>> {
        self.tables
            .codes
            .get(id.index())
            .map(|entry| entry.code.clone())
    }

    pub fn code_method(&self, id: CodeId) -> Option<Rc<MixinMethod>> {
        self.tables
            .codes
            .get(id.index())
            .and_then(|entry| entry.method.clone())
    }

    /// One accessor for the behaviour half of a class object, so a
    /// construction site that needs a superclass's application never has to
    /// re-derive the borrow-and-match itself.
    pub fn class_application(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<std::rc::Rc<crate::mixin::MixinApplication>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    /// Where `class`'s own slots start in an instance's fields
    /// (`MixinApplication::own_slot_base`), copied out; `None` for an
    /// object that is not a class object. Read without cloning the
    /// application's `Rc`, because every `InitializeSlot` and
    /// `ReturnSlotIfFilled` asks it (`Heap::activation_slot_base`).
    pub fn class_own_slot_base(&self, class: ClassReference<'gc>) -> Option<usize> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(data.application.own_slot_base),
            _ => None,
        }
    }

    /// §3.1.1's "its class inherits from class `Value`", read off the
    /// application without cloning the `Rc` that holds it: this is asked of
    /// every slot an initializer writes, and going through
    /// `class_application` would pay a refcount bump per slot for one
    /// `bool`. That bump is what this accessor exists to avoid.
    pub fn class_inherits_value(&self, class: ClassReference<'gc>) -> bool {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.application.inherits_value,
            _ => false,
        }
    }

    /// Runs `read` over the field indices of the slots `class` and its
    /// superclasses **declare** (`MixinApplication::declared_slots`), for
    /// the same reason `class_inherits_value` reads its `bool` here: the
    /// derivation asks this once per slot on its declining path, and
    /// `class_application` would pay a refcount bump each time.
    ///
    /// A closure rather than a returned `&[usize]`, so the borrow of the
    /// arena ends here and nothing reaches past an accessor ("The seam",
    /// `docs/design/memory.md`). `None` when `class` is no class object.
    pub fn with_class_declared_slots<T>(
        &self,
        class: ClassReference<'gc>,
        read: impl FnOnce(&[usize]) -> T,
    ) -> Option<T> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(read(&data.application.declared_slots)),
            _ => None,
        }
    }

    /// Whether `class`'s application applies a mixin compiled from shipped
    /// core-library source (`Mixin.shipped`); `false` for an object that is
    /// not a class object. Read without cloning the application's `Rc`.
    pub fn class_is_shipped(&self, class: ClassReference<'gc>) -> bool {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.application.mixin.shipped,
            _ => false,
        }
    }

    pub fn class_enclosing(&self, class: ClassReference<'gc>) -> Option<ObjectReference<'gc>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.enclosing,
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.declaring_class,
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.superclass,
            _ => None,
        }
    }

    /// Whether `class` is a level a mixin application added: exactly when
    /// `class_applied_clause` answers a clause, without copying it out. The
    /// instantiation walk asks this of every superclass it climbs to, so an
    /// ordinary level costs one test of an object the walk reads anyway.
    pub fn class_is_applied_level(&self, class: ClassReference<'gc>) -> bool {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data
                .applied_clause
                .as_ref()
                .is_some_and(|applied| applied.clause().is_some()),
            _ => false,
        }
    }

    /// The `<:` clause a level a mixin application added was built for
    /// (`ClassData::applied_clause`); `None` for any other class object.
    pub fn class_applied_clause(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::mixin::MixinApplicationClause> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data
                .applied_clause
                .as_ref()
                .and_then(|applied| applied.clause().cloned()),
            _ => None,
        }
    }

    /// Which `<:` clause added `class` -- the applying declaration and the
    /// clause's position in it -- for the image encoder, which names a level's
    /// clause that way; `None` for a class object no mixin application added.
    pub fn class_applied_origin(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::mixin::AppliedClause> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.applied_clause.as_deref().cloned(),
            _ => None,
        }
    }

    /// Whether `class` carries a recorded `ValueClassRefusal`, without
    /// copying it: `decide_instantiation` asks this on every instantiation
    /// of a value class, and only the cold error path reads the refusal
    /// itself (`class_value_refusal`).
    pub fn class_has_value_refusal(&self, class: ClassReference<'gc>) -> bool {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.value_class_refusal.is_some(),
            _ => false,
        }
    }

    /// §3.1.1's enclosing-object condition on a value class, decided when
    /// `class` was created and read here by `decide_instantiation`'s cold
    /// error path and by `check_value_class`'s walk --
    /// `ValueClassRefusal`'s doc comment says why creation and refusal are
    /// two different moments. `None` for a class that is no value and for a
    /// value class whose enclosing objects are all values.
    pub fn class_value_refusal(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<crate::types::ValueClassRefusal> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.value_class_refusal.clone(),
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: ClassReference<'gc>) -> Option<BuiltinClass> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.builtin,
            _ => None,
        }
    }

    /// A lookup level's superclass, or `None` when `class` is not a class
    /// object -- which is what ends a `walk` that stepped onto one through
    /// `next_level`'s unverified class-object chain.
    pub fn class_lookup_step(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<Option<ClassReference<'gc>>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(data.superclass),
            _ => None,
        }
    }

    /// `class`'s metaclass: its header class, when that class's
    /// `this_class` is `class` (`newspeak-spec.pdf` §3.2, "Each class is an
    /// instance of a unique metaclass"). `None` for a handle that is no
    /// class object, for a metaclass -- whose class is `Metaclass` -- and
    /// for a class object of the kernel phase before the bootstrap gave it
    /// its metaclass (`Mutator::give_existing_class_objects_metaclasses`).
    pub fn class_metaclass(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_object_origin(class);
        let entry = &self.objects[class.index as usize];
        if !matches!(entry.data, ObjectData::Class(_)) {
            return None;
        }
        let header = entry.class;
        self.class_this_class(header)
            .filter(|this_class| this_class.same(class))
            .map(|_| header)
    }

    /// A metaclass's class (`ClassData::this_class`); `None` for a class
    /// object that is no metaclass and for a handle that is no class object.
    pub fn class_this_class(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.this_class,
            _ => None,
        }
    }

    /// The name a class object is shown by: its mixin's for a class, and
    /// its class's followed by ` class` for a metaclass, as
    /// `Metaclass>>name` answers it (`Kernel.ns`). For the refusal texts
    /// that name what could not be instantiated.
    pub fn class_display_name(&self, class: ClassReference<'gc>) -> String {
        match self.class_this_class(class) {
            Some(this_class) => format!("{} class", self.class_display_name(this_class)),
            None => self.class_application(class).map_or_else(
                || "a class".to_string(),
                |application| application.mixin.name.to_string(),
            ),
        }
    }

    /// The id `dispatch_id` answers for an object of `class`: its class's
    /// instances id, a class object's included -- its class is its
    /// metaclass, so its id is that metaclass's instances id -- and `NONE`
    /// when the class is not a class object, which only a crafted image
    /// makes.
    fn header_dispatch_id(&self, class: ClassReference<'gc>) -> DispatchId {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(class_data) => class_data.dispatch_id,
            _ => DispatchId::NONE,
        }
    }

    /// The dispatch id an inline cache guards `value`'s send on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an object's is in its
    /// header -- one arena load -- and an immediate's in a table. Nothing
    /// outside this file knows where the id lives.
    #[inline(always)]
    pub fn dispatch_id(&self, value: Value<'gc>) -> DispatchId {
        let Some(object) = value.as_object() else {
            return self.immediate_dispatch_ids[value.immediate_index()];
        };
        self.check_object_origin(object);
        let entry = &self.objects[object.index as usize];
        debug_assert_eq!(
            entry.header.guard(),
            self.header_dispatch_id(entry.class),
            "an object's dispatch id went stale: something changed its class's \
             application without restamping it"
        );
        entry.header.guard()
    }

    /// `class`'s ids, or `NONE` when it is not a class object: the key of a
    /// `super` send and of a private lexical send.
    pub fn class_dispatch_ids(&self, class: ClassReference<'gc>) -> DispatchIds {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => DispatchIds {
                instances: data.dispatch_id,
            },
            _ => DispatchIds::NONE,
        }
    }

    pub(crate) fn set_immediate_dispatch_ids(&mut self, ids: [DispatchId; 5]) {
        self.immediate_dispatch_ids = ids;
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
        let entry = &mut self.objects[object.index as usize];
        entry.class = class;
        entry.header = entry.header.with_guard(dispatch_id);
        true
    }

    /// Links a metaclass to its class (`ClassData::this_class`). Answers
    /// whether the write happened, like the other class setters;
    /// `Mutator::attach_metaclass` is the one caller, on a metaclass it has
    /// just built.
    pub(crate) fn class_set_this_class(
        &mut self,
        metaclass: ClassReference<'gc>,
        class: ClassReference<'gc>,
    ) -> bool {
        self.check_object_origin(metaclass);
        if let ObjectData::Class(data) = &mut self.objects[metaclass.index as usize].data {
            data.this_class = Some(class);
            return true;
        }
        false
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
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.superclass = Some(superclass);
            return true;
        }
        false
    }

    /// Unlinks `class`'s superclass pointer, answering whether the write
    /// happened like `class_set_superclass`. The bootstrap's kernel phase
    /// is the one caller: it states that the kernel's `Object` has no
    /// superclass rather than rely on the placeholder already having none.
    pub fn class_clear_superclass(&mut self, class: ClassReference<'gc>) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.superclass = None;
            return true;
        }
        false
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
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.enclosing = enclosing;
            data.declaring_class = declaring_class;
            return true;
        }
        false
    }

    /// Records `class`'s verdict on §3.1.1's enclosing-object condition.
    /// Two callers: `interpreter::nested::after_clause`, right after it
    /// creates a class object built from source, and
    /// `image::decode::rebuild_applications`, which recomputes the verdict
    /// for a decoded one rather than trusting it from the wire. Answers
    /// whether the write happened, like the other class setters.
    pub fn class_set_value_refusal(
        &mut self,
        class: ClassReference<'gc>,
        refusal: crate::types::ValueClassRefusal,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.value_class_refusal = Some(refusal);
            return true;
        }
        false
    }

    /// Records the `<:` clause that added `class` (`newspeak-spec.pdf`
    /// §6.2), right after the level is created: by
    /// `Mutator::new_applied_level` for a level built from source, and by
    /// the image decoder for a decoded one. Answers whether the write
    /// happened, like the other class setters.
    pub(crate) fn class_set_applied_clause(
        &mut self,
        class: ClassReference<'gc>,
        clause: crate::mixin::AppliedClause,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.applied_clause = Some(Box::new(clause));
            return true;
        }
        false
    }

    /// Gives `class` a new application and moves every live object of that
    /// class to its ids -- a walk of the arena, which is why the decoder,
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
        for index in 0..self.objects.len() {
            let entry_class = self.objects[index].class;
            if entry_class.index == class.index {
                let dispatch_id = self.header_dispatch_id(entry_class);
                let entry = &mut self.objects[index];
                entry.header = entry.header.with_guard(dispatch_id);
            }
        }
        true
    }

    /// `class_set_application` without the walk: `class`'s own header is
    /// restamped from its header class, its instances' are not. The decoder's
    /// `rebuild_applications` uses this to install one application per
    /// decoded class record -- walking the arena on every one of those would
    /// be quadratic in a count the image controls -- and stamps every
    /// decoded object once afterwards, with `stamp_dispatch_ids`.
    pub(crate) fn class_install_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        self.check_object_origin(class);
        let entry = &mut self.objects[class.index as usize];
        let ObjectData::Class(data) = &mut entry.data else {
            return false;
        };
        data.dispatch_id = application.dispatch_ids.instances;
        data.application = application;
        // Its own id is its header class's, which a new application of its
        // own changes only when it is its own class -- the self-referential
        // builtin `Object` of the kernel phase. A decoded class record's
        // header is its metaclass record, whose application may not be
        // installed yet; `stamp_dispatch_ids` restamps every decoded object
        // once all of them are.
        let header_class = entry.class;
        let dispatch_id = self.header_dispatch_id(header_class);
        let entry = &mut self.objects[class.index as usize];
        entry.header = entry.header.with_guard(dispatch_id);
        true
    }

    /// Stamps each of `objects` with the id its class now answers. The
    /// decoder's one pass, after `rebuild_applications` has given every
    /// decoded class its application.
    pub(crate) fn stamp_dispatch_ids(&mut self, objects: &[ObjectReference<'gc>]) {
        for object in objects {
            self.check_object_origin(*object);
            let class = self.objects[object.index as usize].class;
            let dispatch_id = self.header_dispatch_id(class);
            let entry = &mut self.objects[object.index as usize];
            entry.header = entry.header.with_guard(dispatch_id);
        }
    }

    // ---------- Frame accessors ----------
    //
    // A `FrameReference` names a *reified* activation. Its receiver, class and
    // kind are fixed when it is created, so they read the same whether or not
    // the frame is still married to a live stack entry. Its program counter and
    // registers are not: while it is married they live on the activation stack,
    // so the accessors that read or write the frame's own copy assert it is
    // dead. `frame_local`/`frame_set_local` (with the activation stack, below)
    // follow the marriage instead.
    //
    // Only the accessors a caller actually needs are added here;
    // `frame_argument_count` and `frame_program_counter` have no caller yet, so they
    // are not here. Three have only test callers: `frame_set_program_counter`,
    // `frame_sender` and `frame_is_uncontinuable` read or write fields the
    // image format records per frame, and the image and interpreter tests build
    // and check frame records through them. Nothing in the runtime decides
    // anything by those fields -- whether a frame is live is
    // `frame_activation`, never `frame_is_uncontinuable`.

    /// An opaque, hashable key unique to `frame`, for code that needs a
    /// `FrameReference` as a `HashMap` key rather than a comparison --
    /// `image::encode::GraphTable`'s frame-id map is the only caller. On
    /// `Heap`, and guarded, for the reason `object_identity_key` gives.
    pub(crate) fn frame_identity_key(&self, frame: FrameReference<'gc>) -> usize {
        self.check_frame_origin(frame);
        frame.index as usize
    }

    /// A reified frame's receiver. Fixed when the frame is created, so it
    /// needs no redirect while the frame is married.
    pub fn frame_receiver(&self, frame: FrameReference<'gc>) -> Value<'gc> {
        self.check_frame_origin(frame);
        self.load(self.frames[frame.index as usize].frame.receiver)
    }

    pub fn frame_current_class(&self, frame: FrameReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.current_class
    }

    /// Grows a *dead* frame's register file, filling with nil. A married
    /// frame's registers are its live entry's window, so it cannot grow here:
    /// edit-and-continue has to reify and divorce a running activation first
    /// (`docs/design/execution.md`, "How this meets the liveness
    /// requirements").
    pub fn frame_grow_temporaries(&mut self, frame: FrameReference<'gc>, needed: usize) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's registers live on the activation stack; grow them through its activation"
        );
        let stored_frame = &mut self.frames[frame.index as usize].frame;
        if stored_frame.temporaries.len() < needed {
            stored_frame.temporaries.resize(needed, Word::NIL);
        }
    }

    /// Test and format level: the image tests use it to give a dead frame a
    /// synthetic paused `program_counter` before a snapshot. The runtime stores
    /// a running activation's `program_counter` on its stack entry
    /// (`activation_set_program_counter`), and the divorce copies it here.
    pub fn frame_set_program_counter(
        &mut self,
        frame: FrameReference<'gc>,
        program_counter: usize,
    ) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        self.frames[frame.index as usize].frame.program_counter = program_counter;
    }

    pub fn frame_home(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.home
    }

    /// A reified frame's recorded sender. `None` for every frame the runtime
    /// reifies -- a live activation's sender is the stack entry below it, and
    /// a returned activation has none -- so it can be set only on a frame the
    /// image decoder rebuilt from a record that carries one. Test and format
    /// level: only tests read it.
    pub fn frame_sender(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.sender
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `locals::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: FrameReference<'gc>) -> Option<FrameReference<'gc>> {
        self.check_frame_origin(frame);
        match &self.frames[frame.index as usize].frame.kind {
            FrameKind::Closure(closure) => self.closure_captured_frame(*closure),
            _ => None,
        }
    }

    /// The frame's recorded `uncontinuable` flag. Test and format level:
    /// true for every frame the divorce left behind, but on a decoded frame
    /// it answers whatever the file said. It is not liveness --
    /// `raise_return` treats every decoded frame as dead, because it
    /// decides by `frame_activation`, and a decoded frame is never married.
    pub fn frame_is_uncontinuable(&self, frame: FrameReference<'gc>) -> bool {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.uncontinuable
    }

    /// A copy of `frame`'s `kind`. Cloning is cheap -- every variant holds
    /// at most an `Rc` -- and `image::decode` is the only caller that needs
    /// to match on the *whole* kind rather than one variant's payload
    /// (`frame_captured`): once to resolve which of `Initializer`/
    /// `SuperclassClause`/`SuperclassMessage`/`MixinApplicationMessage` a
    /// frame carries a class for
    /// (`frame_body`), and once to check that a class-carrying kind's class
    /// is itself a class reference (`check_class_references`).
    pub fn frame_kind(&self, frame: FrameReference<'gc>) -> FrameKind<'gc> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.kind.clone()
    }

    /// `program_counter`, and how many of a frame's registers are `arguments`
    /// versus `temporaries`, read together through one slice index rather than
    /// three. `image::decode::verify_graph` checks all three against the shape
    /// of the compiled body it reconciles every decoded frame against.
    pub fn frame_register_shape(&self, frame: FrameReference<'gc>) -> (usize, usize, usize) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        let stored_frame = &self.frames[frame.index as usize].frame;
        (
            stored_frame.program_counter,
            stored_frame.arguments.len(),
            stored_frame.temporaries.len(),
        )
    }

    /// The whole of `frame`'s contents as values. `image::encode::encode_frame`
    /// writes a whole frame record and `GraphTable::visit_frame` walks a
    /// whole frame's references; both need every field. Not for a hot path:
    /// it copies every register out.
    ///
    /// Only for a dead frame, which is also the encoder's precondition: it
    /// only ever runs between sends, when nothing is live, and
    /// `image::snapshot` refuses to start otherwise.
    pub fn frame_record(&self, frame: FrameReference<'gc>) -> FrameRecord<'gc> {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        let stored_frame = &self.frames[frame.index as usize].frame;
        FrameRecord {
            receiver: self.load(stored_frame.receiver),
            arguments: stored_frame
                .arguments
                .iter()
                .map(|word| self.load(*word))
                .collect(),
            temporaries: stored_frame
                .temporaries
                .iter()
                .map(|word| self.load(*word))
                .collect(),
            program_counter: stored_frame.program_counter,
            sender: stored_frame.sender,
            home: stored_frame.home,
            current_class: stored_frame.current_class,
            kind: stored_frame.kind.clone(),
            uncontinuable: stored_frame.uncontinuable,
        }
    }

    /// Replaces the entirety of a frame's contents with `record`, the frame
    /// equivalent of `fill_placeholder`.
    /// `image::decode::decode_graph_section`'s second pass fills each frame
    /// its first pass placeholder-allocated, before `verify_graph` runs,
    /// which assumes every frame is already filled in; `tests/image.rs`
    /// forges a decoded frame through it. Call this on any handle something
    /// else already holds and it silently replaces that frame's
    /// already-verified `program_counter`/register-file shape with an
    /// unverified one: no error, and no way for `verify_graph` to notice,
    /// because it has already finished.
    pub fn fill_frame(&mut self, frame: FrameReference<'gc>, record: FrameRecord<'gc>) {
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
        let filled = Frame {
            receiver: self.store(receiver),
            arguments: arguments
                .into_iter()
                .map(|value| self.store(value))
                .collect(),
            temporaries: temporaries
                .into_iter()
                .map(|value| self.store(value))
                .collect(),
            program_counter,
            sender,
            home,
            current_class,
            kind,
            uncontinuable,
        };
        self.frames[frame.index as usize].frame = filled;
    }

    // ---------- The activation stack ----------
    //
    // A live activation is a `StackEntry` plus its window of `values`.
    // `reify` marries an arena `Frame` to an entry when something must hold
    // the activation past this call -- today, only a block being created --
    // and `pop_activation` divorces it again. See
    // `docs/design/execution.md`, "Activations", "Lifecycle".

    /// Pushes an activation whose registers are `arguments` followed by
    /// `temporary_count` nils.
    pub fn push_activation(
        &mut self,
        header: ActivationHeader<'gc>,
        arguments: &[Value<'gc>],
        temporary_count: usize,
    ) -> ActivationReference {
        let base = u32::try_from(self.stack.values.len())
            .expect("more than u32::MAX registers live at once in one heap");
        let argument_count =
            u32::try_from(arguments.len()).expect("more than u32::MAX arguments to one activation");
        let index = u32::try_from(self.stack.entries.len())
            .expect("more than u32::MAX live activations in one heap");
        self.stack.values.reserve(arguments.len() + temporary_count);
        // `store`'s check, made apart from the copy so that a release build,
        // which has no check, copies the arguments in one trusted-length
        // extend rather than a push per word.
        #[cfg(debug_assertions)]
        for argument in arguments {
            self.store(*argument);
        }
        self.stack
            .values
            .extend(arguments.iter().map(|argument| argument.word()));
        let end = self.stack.values.len() + temporary_count;
        self.stack.values.resize(end, Word::NIL);
        self.stack.counted += usize::from(header.counted);
        #[cfg(debug_assertions)]
        let generation = {
            self.stack.next_generation = self.stack.next_generation.wrapping_add(1);
            self.stack.next_generation
        };
        self.stack.entries.push(StackEntry {
            receiver: header.receiver,
            current_class: header.current_class,
            kind: header.kind,
            base,
            argument_count,
            program_counter: 0,
            feedback_base: header.feedback_base,
            code: header.code,
            then: header.then,
            counted: header.counted,
            reified: None,
            retains_registers: false,
            #[cfg(debug_assertions)]
            generation,
        });
        self.activation_handle(index)
    }

    /// Marks `activation` as one whose registers must survive its return: it
    /// created a closure that reads enclosing frames by reference. The VM's
    /// `MakeClosure` is the one caller, and it marks exactly when the block
    /// it creates has a non-zero `outer_reach`.
    pub fn retain_registers(&mut self, activation: ActivationReference) {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].retains_registers = true;
    }

    /// Pops the top activation. If it was reified, its frame is divorced:
    /// the frame becomes dead from then on -- uncontinuable, and with no
    /// sender, as a returned Smalltalk context has none. Answers the
    /// activation's continuation, which the run loop applies to its value.
    ///
    /// The registers move into the frame only when the activation was
    /// marked (`retain_registers`): something still reaches them by
    /// reference. Otherwise they are dropped with the stack window -- a
    /// variable nothing captured is unreachable once its activation
    /// returns, and a captured one the closure carries itself
    /// (`docs/design/execution.md`, "The closure keeps its defining
    /// activation").
    pub fn pop_activation(&mut self, activation: ActivationReference) -> Popped<'gc> {
        self.check_activation(activation);
        debug_assert_eq!(
            activation.index as usize + 1,
            self.stack.entries.len(),
            "only the top activation can be popped"
        );
        let entry = self
            .stack
            .entries
            .pop()
            .expect("internal: an activation handle names a live entry");
        let base = entry.base as usize;
        if let Some(frame) = entry.reified {
            let registers = &self.stack.values[base..];
            let split = entry.argument_count as usize;
            let (arguments, temporaries) = if entry.retains_registers {
                (registers[..split].to_vec(), registers[split..].to_vec())
            } else {
                (Vec::new(), Vec::new())
            };
            let slot = &mut self.frames[frame.index as usize];
            slot.frame.arguments = arguments;
            slot.frame.temporaries = temporaries;
            slot.frame.program_counter = entry.program_counter;
            slot.frame.sender = None;
            slot.frame.uncontinuable = true;
            slot.live = None;
        }
        self.stack.values.truncate(base);
        self.stack.counted -= usize::from(entry.counted);
        Popped {
            then: entry.then,
            counted: entry.counted,
        }
    }

    /// The arena frame for a live activation, created married to it on the
    /// first call and answered again on every later one.
    pub fn reify(&mut self, activation: ActivationReference) -> FrameReference<'gc> {
        self.check_activation(activation);
        let entry_index = activation.index as usize;
        if let Some(frame) = self.stack.entries[entry_index].reified {
            return frame;
        }
        let entry = &self.stack.entries[entry_index];
        let frame = Frame {
            receiver: self.store(entry.receiver),
            arguments: Vec::new(),
            temporaries: Vec::new(),
            program_counter: entry.program_counter,
            sender: None,
            home: match &entry.kind {
                FrameKind::Closure(closure) => self.closure_home(*closure),
                _ => None,
            },
            current_class: entry.current_class,
            kind: entry.kind.clone(),
            uncontinuable: false,
        };
        let handle = self.allocate_frame(frame);
        if self.frames.len() >= self.next_frame_collection {
            self.request_collection();
        }
        self.frames[handle.index as usize].live = Some(activation.index);
        self.stack.entries[entry_index].reified = Some(handle);
        handle
    }

    /// How many activations are live. `Interpreter::run_from_host`, through
    /// which a host's send or instantiation runs, asserts that it leaves it
    /// where it found it.
    pub fn activation_depth(&self) -> usize {
        self.stack.entries.len()
    }

    /// How many live activations count against the depth limit.
    pub fn counted_depth(&self) -> usize {
        self.stack.counted
    }

    /// The frame `activation` was reified into, if a block has captured it.
    pub fn activation_reified(
        &self,
        activation: ActivationReference,
    ) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].reified
    }

    pub fn activation_local(&self, activation: ActivationReference, index: u16) -> Value<'gc> {
        self.check_activation(activation);
        self.load(self.stack.values[self.register_slot(activation.index, index)])
    }

    /// The register window of `activation`, computed once per switch to it.
    pub fn activation_register_base(&self, activation: ActivationReference) -> RegisterBase {
        self.check_activation(activation);
        RegisterBase {
            index: self.stack.entries[activation.index as usize].base,
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
        self.load(self.stack.values[base.index as usize + index as usize])
    }

    #[inline(always)]
    pub fn set_register(&mut self, base: RegisterBase, index: u16, value: Value<'gc>) {
        #[cfg(debug_assertions)]
        self.check_register(base, index);
        let word = self.store(value);
        self.stack.values[base.index as usize + index as usize] = word;
    }

    #[cfg(debug_assertions)]
    fn check_register(&self, base: RegisterBase, index: u16) {
        self.check_activation(base.activation);
        let _ = self.register_slot(base.activation.index, index);
    }

    pub fn activation_set_local(
        &mut self,
        activation: ActivationReference,
        index: u16,
        value: Value<'gc>,
    ) {
        self.check_activation(activation);
        let slot = self.register_slot(activation.index, index);
        let word = self.store(value);
        self.stack.values[slot] = word;
    }

    pub fn activation_receiver(&self, activation: ActivationReference) -> Value<'gc> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].receiver
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
        let entry = &self.stack.entries[activation.index as usize];
        (entry.receiver, entry.current_class)
    }

    pub fn activation_current_class(
        &self,
        activation: ActivationReference,
    ) -> Option<ClassReference<'gc>> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].current_class
    }

    /// The absolute field index a compiled `InitializeSlot`'s mixin-relative
    /// `own_index` is offset by: `MixinApplication.own_slot_base` of the
    /// activation's `current_class`. The two bodies that contain
    /// `InitializeSlot` run with exactly the class whose slots they write
    /// there -- an instance initializer with the level it initializes, one
    /// activation per level of the chain, and a lazy slot's getter (§6.3.3)
    /// with its defining class -- so the base is derived here rather than
    /// carried on the activation: carrying it made every push store a field,
    /// and every method push ask whether its method is a lazy getter, for the
    /// one instruction that reads it (`docs/design/execution.md`, "What the
    /// platform module cost"). An activation with no class, which no compiled
    /// `InitializeSlot` runs in, answers 0.
    pub fn activation_slot_base(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize]
            .current_class
            .and_then(|class| self.class_own_slot_base(class))
            .unwrap_or(0)
    }

    /// Where `activation`'s code family starts in the heap's cache table.
    pub(crate) fn activation_feedback_base(&self, activation: ActivationReference) -> FeedbackBase {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].feedback_base
    }

    pub fn activation_set_program_counter(
        &mut self,
        activation: ActivationReference,
        program_counter: usize,
    ) {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].program_counter = program_counter;
    }

    /// A closure activation's return target -- its closure's `home` -- and
    /// `None` for every other kind, which is its own return target.
    pub fn activation_home(&self, activation: ActivationReference) -> Option<FrameReference<'gc>> {
        self.check_activation(activation);
        match &self.stack.entries[activation.index as usize].kind {
            FrameKind::Closure(closure) => self.closure_home(*closure),
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
        match &self.stack.entries[activation.index as usize].kind {
            FrameKind::Closure(closure) => self.closure_captured_frame(*closure),
            _ => None,
        }
    }

    /// The body `activation` runs.
    pub fn activation_code(
        &self,
        activation: ActivationReference,
    ) -> std::rc::Rc<crate::bytecode::CompiledCode> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].code.clone()
    }

    /// The unwind marker of the code `activation` runs, read afresh on every
    /// call: the unwinder asks it of every activation it pops, so an
    /// activation without one costs it this one test.
    pub(crate) fn activation_unwind_marker(
        &self,
        activation: ActivationReference,
    ) -> Option<crate::bytecode::UnwindMarker> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize]
            .code
            .unwind_marker
    }

    /// Replaces what `activation` does with its value when it returns. The
    /// unwinder's one use: the activation it pushes to run an `ensure:`
    /// argument that is no block is pushed as a method, which delivers, and
    /// its return has to carry the unwind on instead.
    pub(crate) fn activation_set_continuation(
        &mut self,
        activation: ActivationReference,
        then: Continuation<'gc>,
    ) {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].then = then;
    }

    /// Whether the unwinder pushed `activation` to signal a VM error
    /// (`Continuation::VmErrorSignalled`): what the depth limit's reserve
    /// asks, without cloning the error `activation_signalled_vm_error`
    /// answers.
    pub(crate) fn activation_signals_a_vm_error(&self, activation: ActivationReference) -> bool {
        self.check_activation(activation);
        matches!(
            self.stack.entries[activation.index as usize].then,
            Continuation::VmErrorSignalled(_)
        )
    }

    /// The VM error `activation` is the `signal` of, if the unwinder pushed
    /// it to signal one (`Continuation::VmErrorSignalled`).
    pub(crate) fn activation_signalled_vm_error(
        &self,
        activation: ActivationReference,
    ) -> Option<NewspeakError<'gc>> {
        self.check_activation(activation);
        match &self.stack.entries[activation.index as usize].then {
            Continuation::VmErrorSignalled(error) => Some((**error).clone()),
            _ => None,
        }
    }

    /// The activation directly above `activation` on the stack, if any:
    /// the one it sent to, while it waits for it.
    pub fn activation_above(&self, activation: ActivationReference) -> Option<ActivationReference> {
        self.check_activation(activation);
        let above = activation.index as usize + 1;
        (above < self.stack.entries.len()).then(|| self.activation_handle(above as u32))
    }

    /// The `program_counter` last stored for `activation` -- at a send, the
    /// send's own index, which is where the run loop resumes it from when its
    /// callee returns.
    pub fn activation_program_counter(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].program_counter
    }

    /// The innermost live activation, if any.
    pub fn top_activation(&self) -> Option<ActivationReference> {
        let length = self.stack.entries.len();
        (length > 0).then(|| self.activation_handle((length - 1) as u32))
    }

    /// How many activations are below `activation` -- its depth, counted from
    /// the bottom of the stack. The run loop compares a non-local return's
    /// target against the depth it started at to tell its own activations from
    /// its callers'.
    pub fn activation_position(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        activation.index as usize
    }

    /// The activation `activation` returns to: the entry below it, since a
    /// callee -- a method, a block a leaf activated, an `ensure:` block the
    /// unwinder runs -- is always pushed above the activation it answers
    /// to. `None` for the bottom of the stack.
    pub fn activation_sender(
        &self,
        activation: ActivationReference,
    ) -> Option<ActivationReference> {
        self.check_activation(activation);
        activation
            .index
            .checked_sub(1)
            .map(|index| self.activation_handle(index))
    }

    /// What `activation` was activated for.
    pub fn activation_kind(&self, activation: ActivationReference) -> FrameKind<'gc> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].kind.clone()
    }

    /// Whether `activation` runs a method, a superclass clause, a superclass
    /// clause's message or a `<:` clause's message -- not a block, an
    /// initializer or a top-level activation. Answered without cloning the
    /// kind, which holds an `Rc` for a method.
    pub(crate) fn activation_runs_method_or_clause(&self, activation: ActivationReference) -> bool {
        self.check_activation(activation);
        matches!(
            self.stack.entries[activation.index as usize].kind,
            FrameKind::Method(_)
                | FrameKind::SuperclassClause(_)
                | FrameKind::SuperclassMessage(_)
                | FrameKind::MixinApplicationMessage(_)
        )
    }

    /// A register of a reified frame: the live stack slot while its
    /// activation runs, the frame's own copy after it has returned. This is
    /// the path a block takes to its method's variables, and the only
    /// register access that branches.
    ///
    /// A returned activation that was never marked kept no registers at all
    /// (`pop_activation`), and a decoded frame's register file is whatever
    /// the image said. No code the compiler produces reaches a register
    /// either one lacks -- `MakeClosure` marks every activation a
    /// by-reference block is created in -- so an index past the file is
    /// either a crafted image or a compiler defect, and answering `Nil`
    /// rather than panicking is what keeps the first from aborting the
    /// process.
    pub fn frame_local(&self, frame: FrameReference<'gc>, index: u16) -> Value<'gc> {
        self.check_frame_origin(frame);
        let slot = &self.frames[frame.index as usize];
        let word = match slot.live {
            Some(entry) => self.stack.values[self.register_slot(entry, index)],
            None => {
                let stored_frame = &slot.frame;
                let register_index = index as usize;
                if register_index < stored_frame.arguments.len() {
                    stored_frame.arguments[register_index]
                } else {
                    stored_frame
                        .temporaries
                        .get(register_index - stored_frame.arguments.len())
                        .copied()
                        .unwrap_or(Word::NIL)
                }
            }
        };
        self.load(word)
    }

    /// `frame_local`'s twin, and silently a no-op for an index a dead
    /// frame's register file does not have, for the reason given there.
    pub fn frame_set_local(&mut self, frame: FrameReference<'gc>, index: u16, value: Value<'gc>) {
        self.check_frame_origin(frame);
        // Copied out first, so neither arm below holds a borrow of `frames`
        // while it writes.
        let live = self.frames[frame.index as usize].live;
        let word = self.store(value);
        match live {
            Some(entry) => {
                let slot = self.register_slot(entry, index);
                self.stack.values[slot] = word;
            }
            None => {
                let stored_frame = &mut self.frames[frame.index as usize].frame;
                let register_index = index as usize;
                if register_index < stored_frame.arguments.len() {
                    stored_frame.arguments[register_index] = word;
                } else {
                    let temporary_index = register_index - stored_frame.arguments.len();
                    if let Some(slot) = stored_frame.temporaries.get_mut(temporary_index) {
                        *slot = word;
                    }
                }
            }
        }
    }

    /// The live activation `frame` is married to, or `None` once that
    /// activation has returned -- and for every frame the image decoder
    /// built, since none of those ever ran here. `raise_return` uses it to
    /// tell a live home from a dead one.
    pub fn frame_activation(&self, frame: FrameReference<'gc>) -> Option<ActivationReference> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize]
            .live
            .map(|entry| self.activation_handle(entry))
    }

    // ---------- The safepoint ----------

    #[inline]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.safepoint_requested
    }

    /// A request served (`Interpreter::safepoint`).
    pub(crate) fn clear_safepoint_request(&mut self) {
        self.safepoint_requested = false;
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, requested: bool) {
        self.safepoint_requested = requested;
    }

    /// An allocation never collects; it asks, and the run loop's next
    /// safepoint serves the request (decided 2026-09-13). Setting
    /// `safepoint_requested` is what makes the poll take its slow path.
    ///
    /// Must do nothing but set these two booleans: both call sites raise the
    /// request while the object or frame just allocated is still half-wired
    /// -- `push_object` before `object_handle` names it, `reify` between
    /// `allocate_frame` and the writes that make the frame reachable from
    /// its stack entry -- so it is harmless only because nothing here reads
    /// the heap back. Real work here would have `reify` collect a frame no
    /// stack entry points at yet.
    ///
    /// `push_object` and `reify` are its only two callers, not every
    /// allocation path: `allocate_self_referential` and the image
    /// decoder's `allocate_placeholder_frame` compare against no limit at
    /// all. Harmless -- the bootstrap that calls the former is bounded, the
    /// decoder never polls a safepoint, and the next ordinary allocation
    /// compares against the same limit either way.
    fn request_collection(&mut self) {
        self.collection_requested = true;
        self.safepoint_requested = true;
    }

    pub(crate) fn collection_requested(&self) -> bool {
        self.collection_requested
    }

    /// After a collection: each arena's next limit (`collection_limit`).
    pub(crate) fn reset_collection_limits(&mut self) {
        self.collection_requested = false;
        self.next_object_collection = collection_limit(self.collection_floor, self.objects.len());
        self.next_frame_collection = collection_limit(self.collection_floor, self.frames.len());
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_collection_floor(&mut self, floor: usize) {
        self.collection_floor = floor;
        self.next_object_collection = floor;
        self.next_frame_collection = floor;
    }

    /// How much a move has to rewrite: both arenas, the activation stack's
    /// entries and registers, the canonical symbol table, and the host
    /// roots. The stress mode paces its moves by it.
    #[cfg(debug_assertions)]
    pub(crate) fn traced_size(&self) -> usize {
        self.objects.len()
            + self.frames.len()
            + self.stack.entries.len()
            + self.stack.values.len()
            + self.canonical_symbols.len()
            + self.host_roots.borrow().slots.len()
    }

    // ---------- Moving everything (debug builds only) ----------

    /// How many times this heap has moved everything -- by `move_everything`
    /// or by `compact`, both of which relocate every survivor to a new index
    /// and so advance this the same way: a handle into the arrangement
    /// before either one is exactly as stale as one into the arrangement
    /// before the other.
    #[cfg(debug_assertions)]
    pub(crate) fn move_epoch(&self) -> u32 {
        self.epoch
    }

    /// Moves every object and every reified frame to a new index and
    /// rewrites every handle the heap itself holds -- in every arena record,
    /// in every stack entry and the continuation it owns, in every register,
    /// and in every host root -- into the next epoch. What the heap does not
    /// hold, the caller forwards with the answer: the interpreter's own
    /// roots (`World::move_everything`). A compacting collection that
    /// treats everything as live; the stress mode runs one at the run loop's
    /// safepoints (`docs/design/memory.md`, "The move stress mode").
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between runs.
    #[cfg(debug_assertions)]
    #[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them stale"]
    fn move_everything(&mut self) -> Forwarder<Rotation> {
        let from_epoch = self.epoch;
        let to_epoch = from_epoch.wrapping_add(1);
        let objects = Rotation::new(self.objects.len(), to_epoch);
        let frames = Rotation::new(self.frames.len(), to_epoch);
        self.objects.rotate_right(objects.shift as usize);
        self.frames.rotate_right(frames.shift as usize);
        self.epoch = to_epoch;
        let mut forwarder = Forwarder {
            heap_id: self.heap_id,
            from_epoch,
            to_epoch,
            objects,
            frames,
        };
        self.forward_everything(&mut forwarder);
        forwarder
    }

    /// Compacts both arenas against `marker`'s bits and rewrites every handle
    /// the heap itself holds into the next epoch. What the heap does not
    /// hold, the caller forwards with the answer, exactly as for a move
    /// (`World::collect`).
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between runs.
    #[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them stale"]
    pub(crate) fn compact(&mut self, marker: mark::Marker) -> Forwarder<ForwardingTable> {
        #[cfg(debug_assertions)]
        let from_epoch = self.epoch;
        #[cfg(debug_assertions)]
        let to_epoch = from_epoch.wrapping_add(1);
        #[cfg(debug_assertions)]
        {
            self.epoch = to_epoch;
        }
        let objects = compact_arena(&mut self.objects, &marker.objects, self.collection_floor);
        let frames = compact_arena(&mut self.frames, &marker.frames, self.collection_floor);
        let mut forwarder = Forwarder {
            #[cfg(debug_assertions)]
            heap_id: self.heap_id,
            #[cfg(debug_assertions)]
            from_epoch,
            #[cfg(debug_assertions)]
            to_epoch,
            objects,
            frames,
        };
        self.forward_everything(&mut forwarder);
        forwarder
    }

    /// Forwards every handle the heap itself holds through `forwarder`:
    /// every arena record, every stack entry and the continuation it owns,
    /// every register, every surviving canonical symbol table entry, and
    /// every host root. Shared by `move_everything` and `compact` -- both
    /// build their own `Forwarder` first, over a different `Relocation`,
    /// and then walk the same six roots through it -- so that a field
    /// either one must reach is named here once, rather than in two copies
    /// a later change can update in one and miss in the other (`trace.rs`'s
    /// reason for existing, one level up).
    fn forward_everything<M: Relocation>(&mut self, forwarder: &mut Forwarder<M>) {
        use crate::trace::{Trace, Tracer};
        for object in &mut self.objects {
            object.trace(forwarder);
        }
        // A married frame's registers are its entry's, traced below with
        // the stack; `live` is a stack index, and no stack entry moves.
        for slot in &mut self.frames {
            slot.frame.trace(forwarder);
        }
        for entry in &mut self.stack.entries {
            entry.trace(forwarder);
        }
        for word in &mut self.stack.values {
            forwarder.word(word);
        }
        // The table is weak -- `begin_marking` deliberately does not walk it
        // (see its doc comment) -- but by the time a caller reaches
        // `compact`, `prune_canonical_symbols` has already run against the
        // finished marking and cleared every entry whose object died. What
        // remains here is still a handle into the arena just compacted, and
        // rewriting it is this loop's job like any other: skipping it would
        // leave a stale index behind for a symbol that is very much alive.
        for object in self.canonical_symbols.iter_mut().flatten() {
            forwarder.object(object);
        }
        let roots = self.host_roots.get_mut();
        roots.sweep();
        for slot in &roots.slots {
            let mut value = slot.get();
            forwarder.value(&mut value);
            slot.set(value);
        }
    }

    /// The arenas' occupancy (`Interpreter::heap_statistics`).
    pub(crate) fn heap_statistics(&self) -> crate::HeapStatistics {
        crate::HeapStatistics {
            live_objects: self.objects.len(),
            object_slots: self.objects.capacity(),
            live_frames: self.frames.len(),
            frame_slots: self.frames.capacity(),
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

    /// The collector's counts and pauses since the process started, plus the
    /// arenas' occupancy right now (`Interpreter::collector_statistics`,
    /// `--collector-statistics`).
    pub(crate) fn collector_statistics(&self) -> crate::CollectorStatistics {
        crate::CollectorStatistics {
            collections: self.collections,
            total_pause: self.total_pause,
            longest_pause: self.longest_pause,
            live_objects: self.objects.len(),
            live_frames: self.frames.len(),
        }
    }

    /// Rewrites every object field that holds a resolved future to hold its
    /// value instead (`docs/design/memory.md`, "A resolved future is traced
    /// as its value"): instance and array fields -- an indirection vector is
    /// an `Array` -- and a closure's copied values. Never a register or a
    /// host root: the `Future`'s `doesNotUnderstand:` that forced it may
    /// still be returning, with the future as `self`, and a host holding the
    /// future holds that object, so a future still held there survives this
    /// collection and is replaced by the first one after it lets go. Runs before
    /// marking, so a future nothing else reaches is garbage by the time the
    /// marker looks. Answers how many references it replaced.
    ///
    /// A future is an `Instance` of `future_class` whose field `state_index`
    /// is `SmallInteger(resolved)`; its value is then field
    /// `computation_index`. That value is never itself a future --
    /// `Mutator::future_computed` follows a future of a future before it
    /// records one, and one whose fields crafted code made so is malformed
    /// and not replaced -- so one pass of replacement leaves no resolved
    /// future behind in any field it rewrites.
    pub(crate) fn replace_resolved_futures(
        &mut self,
        future_class: ClassReference<'gc>,
        computation_index: usize,
        state_index: usize,
        resolved: i64,
    ) -> usize {
        // First pass: every resolved future's arena index and its value.
        // Words, not values: the replacement copies a stored word into
        // another field, so it needs no check and no epoch.
        let mut values: std::collections::HashMap<u32, Word> = std::collections::HashMap::new();
        for (index, object) in self.objects.iter().enumerate() {
            if !object.class.same(future_class) {
                continue;
            }
            let ObjectData::Words { words: fields, .. } = &object.data else {
                continue;
            };
            if object.header.format() != Format::FixedPointers {
                continue;
            }
            if let (Some(state), Some(value)) = (
                fields
                    .get(state_index)
                    .and_then(|state| self.load(Word::from_bits(*state)).as_small_integer()),
                fields.get(computation_index).copied().map(Word::from_bits),
            ) {
                // A "resolved" future whose value is a future is one crafted
                // code corrupted (`FutureState::Malformed`): it is left in
                // place, for a forcing to refuse.
                let value_is_a_future = self
                    .load(value)
                    .as_object()
                    .is_some_and(|target| self.object_class(target).same(future_class));
                if state == resolved && !value_is_a_future {
                    let index = u32::try_from(index)
                        .expect("internal: an arena index fits the handle's u32");
                    values.insert(index, value);
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
                if let Some(index) = field.object_index() {
                    if let Some(value) = values.get(&index) {
                        *field = *value;
                        replaced += 1;
                    }
                }
            }
        };
        for object in &mut self.objects {
            // `object.class` is a class object, never a future.
            let format = object.header.format();
            match &mut object.data {
                ObjectData::Words { words, .. } => {
                    // A number's digits are no `Value`s.
                    if matches!(format, Format::FixedPointers | Format::IndexablePointers) {
                        for field in words.iter_mut() {
                            let mut word = [Word::from_bits(*field)];
                            replace(&mut word);
                            *field = word[0].bits();
                        }
                    }
                }
                ObjectData::Closure(closure) => {
                    let ClosureData {
                        code: _,
                        // A frame's registers are exempt, as the stack's are.
                        captured_frame: _,
                        copied,
                        feedback_base: _,
                    } = closure;
                    replace(copied);
                }
                // Every handle a class object holds -- its enclosing object,
                // its declaring class, its superclass, itself -- is an
                // `ObjectReference`, not a `Value`: no future can stand in
                // one, since a message to a future is answered by its value,
                // so a nested class's enclosing object is never the future.
                ObjectData::Class(_) => {}
                // A reified frame: its registers are exempt.
                ObjectData::Activation(_) => {}
            }
        }
        replaced
    }

    /// Marks the heap's own roots -- the activation stack's entries and its
    /// registers, and the host roots -- and answers the marker without
    /// draining it, so that the caller can add the step state's roots
    /// (`World::mark_everything_reachable`) before the traversal runs.
    ///
    /// The canonical symbol table is deliberately not among these roots: it
    /// is the system's one weak structure (`docs/design/memory.md`, "Weak
    /// references"), pruned against the finished marking by
    /// `prune_canonical_symbols` rather than kept alive by it. The arenas
    /// themselves are never roots, only what the roots reach.
    pub(crate) fn begin_marking(&mut self) -> mark::Marker {
        use crate::trace::{Trace, Tracer};
        let mut marker = mark::Marker::new(self.objects.len(), self.frames.len());
        for entry in &mut self.stack.entries {
            entry.trace(&mut marker);
        }
        for word in &mut self.stack.values {
            marker.word(word);
        }
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
    /// the system's only weak structure: `Mutator::new_symbol` is the only
    /// way to make a symbol object (§5.1.6: equal symbols are one object), so
    /// one that is collected was held by nothing -- anything that held it
    /// would have been a root and kept it live -- and the next mint is again
    /// the only one. No live comparison can tell a collected-and-reminted
    /// symbol from one that was never collected (`docs/design/memory.md`,
    /// "Weak references").
    ///
    /// Must run between marking and compaction, never outside that window:
    /// after compaction the mark bits no longer line up with the arena's
    /// indices, and before marking has finished draining, an entry cleared
    /// here reads bits that are not yet complete and could wrongly prune a
    /// symbol the traversal has not reached yet.
    pub(crate) fn prune_canonical_symbols(&mut self, marker: &mark::Marker) {
        for slot in &mut self.canonical_symbols {
            if let Some(object) = slot {
                if !marker.objects.is_set(object.index) {
                    *slot = None;
                }
            }
        }
    }

    /// Runs the marker's work lists empty. Each record is traced with the
    /// arena borrowed field by field, which is what lets the marker be
    /// borrowed at the same time.
    pub(crate) fn drain_marking(&mut self, marker: &mut mark::Marker) {
        use crate::trace::Trace;
        let Heap {
            objects, frames, ..
        } = self;
        while let Some(work) = marker.next() {
            match work {
                mark::Work::Object(index) => objects[index as usize].trace(marker),
                // A married frame's registers are its entry's, already traced
                // with the stack; `live` is a stack index and no entry moves.
                mark::Work::Frame(index) => frames[index as usize].frame.trace(marker),
            }
        }
    }

    /// Marks from the heap's own roots and drains, for the heap's own tests.
    /// A real collection goes through `World`, which owns the step state's
    /// roots as well.
    #[cfg(test)]
    fn mark_from_roots_for_test(&mut self) -> crate::heap::mark::Marker {
        let mut marker = self.begin_marking();
        self.drain_marking(&mut marker);
        marker
    }

    /// Marks from the heap's own roots and compacts, for the heap's own
    /// tests. A real collection goes through `World`.
    #[cfg(test)]
    fn collect_for_test(&mut self) {
        let marker = self.mark_from_roots_for_test();
        let _ = self.compact(marker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest activation the arena accepts: enough to get a
    /// `FrameReference` back, with no field any test below reads. Gated like
    /// its only caller -- the origin guard it feeds does not exist in a release
    /// build, so neither does the test, so neither may this.
    #[cfg(debug_assertions)]
    fn bare_frame<'gc>() -> Frame<'gc> {
        Frame {
            receiver: Word::NIL,
            arguments: Vec::new(),
            temporaries: Vec::new(),
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
        }
    }

    /// Reading a handle through a heap that did not allocate it is the one
    /// failure the arena introduces and the one it cannot answer correctly:
    /// the index names a different, perfectly valid object. Debug builds
    /// must refuse it rather than answer.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_a_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(ObjectData::instance([]));
        let object =
            origin.allocate_object(class, ObjectData::string(&crate::CodePoints::from("mine")));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(ObjectData::instance([]));
        other.allocate_object(
            other_class,
            ObjectData::string(&crate::CodePoints::from("theirs")),
        );
        other.allocate_object(
            other_class,
            ObjectData::string(&crate::CodePoints::from("theirs")),
        );

        // In range for `other` -- which is exactly why this is silent
        // without the guard.
        let _ = other.str_value(object);
    }

    /// The sibling case, and the one an accessor guard cannot reach: `same` is
    /// handed two handles and no `Heap`, so nothing else in the crate would
    /// notice that they come from different arenas. Two unrelated objects
    /// sharing an index would answer "identical" -- silently, and
    /// `ObjectReference::same` *is* object identity. Pinned here so the
    /// guarantee `CLAUDE.md` states is tested rather than asserted.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(ObjectData::instance([]));
        let mine =
            origin.allocate_object(class, ObjectData::string(&crate::CodePoints::from("mine")));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(ObjectData::instance([]));
        let theirs = other.allocate_object(
            other_class,
            ObjectData::string(&crate::CodePoints::from("theirs")),
        );

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
        let mut origin = Heap::new();
        let mine = origin.allocate_frame(bare_frame());

        let mut other = Heap::new();
        let theirs = other.allocate_frame(bare_frame());

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
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let first = heap.allocate_object(class, ObjectData::instance([]));
        let second = heap.allocate_object(class, ObjectData::instance([]));

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

    /// Why `identity_key` lives on `Heap`. A key computed from a handle
    /// alone could carry no origin check, so two objects from different
    /// arenas would produce the same key and collide in a caller's map --
    /// `image::encode::GraphTable` would give one object the other's id.
    /// Requiring the `Heap` is what makes the check possible, and the
    /// compiler is what requires it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn keying_a_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let class = origin.allocate_self_referential(ObjectData::instance([]));
        let mine =
            origin.allocate_object(class, ObjectData::string(&crate::CodePoints::from("mine")));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential(ObjectData::instance([]));
        let theirs = other.allocate_object(
            other_class,
            ObjectData::string(&crate::CodePoints::from("theirs")),
        );

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
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let narrow = heap.allocate_object(
            class,
            ObjectData::string(&crate::CodePoints::from("abcdefghi")),
        );
        let header = heap.objects[narrow.index as usize].header;
        assert_eq!(header.format(), Format::Bytes);
        assert_eq!(header.word_count(), 1 + 2, "nine bytes take two words");
        let wide =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("€uro")));
        let header = heap.objects[wide.index as usize].header;
        assert_eq!(header.format(), Format::ThirtyTwoBit);
        assert_eq!(header.word_count(), 1 + 2);
        let array = heap.allocate_object(class, ObjectData::array([Word::NIL; 3]));
        let header = heap.objects[array.index as usize].header;
        assert_eq!(header.format(), Format::IndexablePointers);
        assert_eq!(header.word_count(), 1 + 3);
        let negative = heap.allocate_object(
            class,
            ObjectData::large_integer(&crate::ArbitraryInteger::from_i128(-(1 << 100))),
        );
        let header = heap.objects[negative.index as usize].header;
        assert_eq!(header.format(), Format::SixtyFourBit);
        assert!(header.negative());
        assert_eq!(header.word_count(), 1 + 2);
    }

    #[test]
    fn a_self_referential_class_is_its_own_class() {
        let mut heap = Heap::new();
        // A filler object first, so `class` does not land at index 0 -- an
        // implementation that always answered `ObjectReference { index: 0 }`
        // would otherwise still pass this test.
        heap.allocate_self_referential(ObjectData::instance([]));
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        assert!(heap.object_class(class).same(class));
    }

    fn top_level_frame<'gc>() -> Frame<'gc> {
        Frame {
            receiver: Word::NIL,
            arguments: vec![],
            temporaries: vec![],
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
        }
    }

    /// Like `top_level_frame`, but with a distinct value in each of
    /// `program_counter`, `arguments.len()` and `temporaries.len()`.
    /// `frame_register_shape` returns those three as a same-typed tuple, so a
    /// frame built from all-zero or otherwise equal fields cannot catch the
    /// tuple's positions being silently transposed -- exactly the
    /// silent-wrong-output risk this conversion carries.
    fn shaped_frame<'gc>(
        program_counter: usize,
        argument_count: usize,
        temporary_count: usize,
    ) -> Frame<'gc> {
        Frame {
            arguments: vec![Word::NIL; argument_count],
            temporaries: vec![Word::NIL; temporary_count],
            program_counter,
            ..top_level_frame()
        }
    }

    /// A frame is addressed by its position in the arena, so a handle must
    /// stay valid across the reallocation that growing the arena performs.
    /// Under `Rc` this was free; under an arena it is the property the
    /// representation rests on.
    #[test]
    fn a_frame_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        let first = heap.allocate_frame(shaped_frame(3, 2, 1));
        for _ in 0..1000 {
            heap.allocate_frame(top_level_frame());
        }
        assert!(heap.frame_receiver(first).is_nil());
        assert_eq!(heap.frame_register_shape(first), (3, 2, 1));
    }

    /// Frame identity is what the image round trip checks sharing by -- a
    /// closure's `captured_frame` and `home` naming one frame or two -- so two
    /// reified frames may never compare equal however alike their contents are.
    #[test]
    fn two_frames_are_never_the_same_activation() {
        let mut heap = Heap::new();
        let first = heap.allocate_frame(top_level_frame());
        let second = heap.allocate_frame(top_level_frame());
        assert!(!first.same(second));
        assert!(first.same(first));
    }

    /// The object counterpart of the frame test above: a handle must stay
    /// valid across the reallocation that growing the arena performs, and
    /// two objects with identical contents must never be the same object.
    #[test]
    fn an_object_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        // A filler object first, so neither `class` nor `first` lands at
        // index 0 -- see `a_self_referential_class_is_its_own_class`.
        heap.allocate_self_referential(ObjectData::instance([]));
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let first =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("first")));
        for _ in 0..1000 {
            heap.allocate_object(
                class,
                ObjectData::string(&crate::CodePoints::from("filler")),
            );
        }
        assert_eq!(heap.str_value(first).as_deref(), Some("first"));
        assert!(heap.object_class(first).same(class));

        let twin =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("first")));
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
        let application = std::rc::Rc::new(crate::mixin::MixinApplication {
            index: crate::heap::ApplicationIndex::UNREGISTERED,
            mixin: std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name))),
            superclass: None,
            inherits_value: false,
            first_mutable_slot: None,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
        });
        heap.allocate_self_referential_class(NewClass {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin,
        })
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
        let object = heap.allocate_symbol(symbol_class, symbol);
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
    fn an_instance_field_past_its_word_count_answers_none() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Point");
        let object = heap.allocate_instance(class, 2);
        assert!(heap.instance_field(object, 1).is_some());
        assert!(heap.instance_field(object, 2).is_none());
        assert!(!heap.instance_set_field(object, 2, Value::nil()));
    }

    /// `describe`'s instance arm is the one non-mechanical
    /// rewrite the arena conversion made: a nested read through a re-bound
    /// `class` intermediate rather than a single chained borrow. Nothing
    /// else in the workspace asserts its output, so pin it directly.
    #[test]
    fn describe_names_an_instance_by_its_class_mixin_name() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let instance = heap.allocate_object(class, ObjectData::instance([]));
        assert_eq!(heap.describe(Value::object(instance)), "an instance of Foo");
    }

    /// The fallback arm of that same match: `class` names an `ObjectReference`
    /// whose own `data` is not `ObjectData::Class`, the shape a class reference
    /// decoded from an untrusted image can be in before verification runs.
    /// `describe` must still answer something rather than panic.
    #[test]
    fn describe_falls_back_to_an_object_for_a_class_reference_that_is_not_a_class() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let not_a_class = heap.allocate_object(
            class,
            ObjectData::string(&crate::CodePoints::from("not a class")),
        );
        let instance = heap.allocate_object(not_a_class, ObjectData::instance([]));
        assert_eq!(heap.describe(Value::object(instance)), "an object");
    }

    /// The header every activation-stack test below pushes: nothing in it is
    /// read back except where a test sets it on purpose.
    fn header<'gc>() -> ActivationHeader<'gc> {
        ActivationHeader {
            receiver: Value::nil(),
            current_class: None,
            kind: FrameKind::TopLevel,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        }
    }

    /// A body that returns its first register: no test here runs it.
    fn trivial_code() -> std::rc::Rc<crate::bytecode::CompiledCode> {
        std::rc::Rc::new(crate::bytecode::CompiledCode::new(
            crate::bytecode::CodeKind::Method,
            vec![crate::bytecode::Instruction::Return { register: 0 }],
            0,
            1,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        ))
    }

    #[test]
    fn an_activation_addresses_its_arguments_before_its_temporaries() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(
            header(),
            &[Value::small_integer(1), Value::small_integer(2)],
            2,
        );
        assert_eq!(
            heap.activation_local(activation, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(activation, 1).as_small_integer(),
            Some(2)
        );
        assert!(heap.activation_local(activation, 2).is_nil());
        heap.activation_set_local(activation, 3, Value::small_integer(9));
        assert_eq!(
            heap.activation_local(activation, 3).as_small_integer(),
            Some(9)
        );
        heap.pop_activation(activation);
    }

    /// Two live activations share one `values` vector; neither may see the
    /// other's registers, whichever is written first.
    #[test]
    fn nested_activations_keep_separate_register_windows() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[Value::small_integer(1)], 1);
        let inner = heap.push_activation(header(), &[Value::small_integer(2)], 1);
        heap.activation_set_local(inner, 1, Value::small_integer(20));
        heap.activation_set_local(outer, 1, Value::small_integer(10));
        assert_eq!(heap.activation_local(outer, 0).as_small_integer(), Some(1));
        assert_eq!(heap.activation_local(outer, 1).as_small_integer(), Some(10));
        assert_eq!(heap.activation_local(inner, 0).as_small_integer(), Some(2));
        assert_eq!(heap.activation_local(inner, 1).as_small_integer(), Some(20));
        assert_eq!(heap.activation_depth(), 2);
        heap.pop_activation(inner);
        heap.pop_activation(outer);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// The property the activation stack exists for: an activation nothing
    /// reified is gone once it is popped, rather than kept in the frame
    /// arena, whose slots only a collection reclaims, and only once nothing
    /// reaches them.
    #[test]
    fn an_activation_that_is_never_reified_leaves_nothing_behind() {
        let mut heap = Heap::new();
        for _ in 0..1000 {
            let activation = heap.push_activation(header(), &[Value::small_integer(1)], 3);
            heap.pop_activation(activation);
        }
        assert_eq!(heap.frames.len(), 0);
        assert_eq!(heap.stack.values.len(), 0);
        assert_eq!(heap.activation_depth(), 0);
    }

    #[test]
    fn reifying_an_activation_twice_answers_the_same_frame() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[], 1);
        let first = heap.reify(activation);
        let second = heap.reify(activation);
        assert!(first.same(second));
        assert_eq!(heap.frames.len(), 1);
        heap.pop_activation(activation);
    }

    /// While its activation runs, a reified frame is a view of the live
    /// registers, in both directions: a block writing its method's variable
    /// through the frame must be seen by the method, and vice versa.
    #[test]
    fn a_married_frame_reads_and_writes_the_live_registers() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[Value::small_integer(1)], 1);
        let frame = heap.reify(activation);
        heap.activation_set_local(activation, 1, Value::small_integer(5));
        assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(5));
        heap.frame_set_local(frame, 1, Value::small_integer(7));
        assert_eq!(
            heap.activation_local(activation, 1).as_small_integer(),
            Some(7)
        );
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(activation)));
        heap.pop_activation(activation);
    }

    /// Popping a *marked* reified activation hands its registers to the
    /// frame, which a surviving block goes on reading and writing; the
    /// frame is dead from then on, and a dead activation has no sender.
    /// Unmarked is
    /// `a_returning_activation_keeps_its_registers_only_when_it_was_marked`
    /// in `tests/heap.rs`.
    #[test]
    fn popping_a_reified_activation_divorces_its_frame() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[Value::small_integer(1)], 1);
        heap.activation_set_program_counter(activation, 4);
        let frame = heap.reify(activation);
        // What a `MakeClosure` whose block reads this frame by reference
        // does, and the only reason the registers below survive the pop.
        heap.retain_registers(activation);
        // Written after reification: it must survive the divorce.
        heap.activation_set_local(activation, 1, Value::small_integer(3));
        heap.pop_activation(activation);

        assert!(heap.frame_activation(frame).is_none());
        assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(1));
        assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(3));
        assert_eq!(heap.frame_register_shape(frame), (4, 1, 1));
        assert!(heap.frame_is_uncontinuable(frame));
        assert!(heap.frame_sender(frame).is_none());
        heap.frame_set_local(frame, 1, Value::small_integer(8));
        assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(8));
    }

    /// A block's copied values land in the registers between its parameters
    /// and its temporaries, which is where its compiled code addresses them
    /// (`docs/design/execution.md`, "Converted representation"). A list
    /// longer than the register file stops at its end rather than writing
    /// into the activation below: an image, not the compiler, is what can
    /// present one.
    #[test]
    fn a_closures_copied_values_land_after_the_activations_arguments() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let captured_frame = heap.allocate_frame(top_level_frame());
        let closure = heap.allocate_object(
            class,
            ObjectData::Closure(ClosureData {
                code: trivial_code(),
                captured_frame,
                copied: Box::new([
                    heap.store(Value::small_integer(7)),
                    heap.store(Value::small_integer(8)),
                ]),
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            }),
        );

        let activation = heap.push_activation(header(), &[Value::small_integer(1)], 1);
        heap.fill_copied_values(activation, closure);
        assert_eq!(
            heap.activation_local(activation, 0).as_small_integer(),
            Some(1)
        );
        assert_eq!(
            heap.activation_local(activation, 1).as_small_integer(),
            Some(7)
        );
        // The activation has two registers, so the second copied value has
        // nowhere to go and is dropped rather than written past the window.
        assert_eq!(heap.stack.values.len(), 2);
        heap.pop_activation(activation);
    }

    /// The next activation at the same depth reuses the stack slots. A
    /// divorced frame owns its own copy, so that reuse cannot reach it.
    #[test]
    fn a_divorced_frame_is_untouched_by_the_next_activation_at_its_depth() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(first, 1, Value::small_integer(3));
        let frame = heap.reify(first);
        heap.retain_registers(first);
        heap.pop_activation(first);

        let second = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(second, 1, Value::small_integer(99));
        assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(3));
        heap.pop_activation(second);
    }

    /// A register base names a window by index, as an activation handle
    /// does: reading through it reads the same register the activation
    /// handle does.
    #[test]
    fn a_register_base_reads_and_writes_its_activations_registers() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[Value::small_integer(7)], 2);
        let base = heap.activation_register_base(activation);
        assert_eq!(heap.register(base, 0).as_small_integer(), Some(7));
        heap.set_register(base, 2, Value::small_integer(9));
        assert_eq!(
            heap.activation_local(activation, 2).as_small_integer(),
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
        let first = heap.push_activation(header(), &[], 1);
        let base = heap.activation_register_base(first);
        heap.pop_activation(first);
        let _second = heap.push_activation(header(), &[], 1);
        let _ = heap.register(base, 0);
    }

    /// The window of a deeper activation starts after its caller's: a base
    /// survives the register file growing, since it is an index.
    #[test]
    fn a_register_base_survives_a_deeper_push() {
        let mut heap = Heap::new();
        let caller = heap.push_activation(header(), &[Value::small_integer(1)], 1);
        let base = heap.activation_register_base(caller);
        let callee = heap.push_activation(header(), &[], 4096);
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
    /// reuses the index next -- a different, valid activation, so the read
    /// would silently answer the wrong thing. Debug builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale activation handle")]
    fn reading_through_a_popped_activation_handle_panics() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 1);
        heap.pop_activation(first);
        let _second = heap.push_activation(header(), &[], 1);
        let _ = heap.activation_local(first, 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_an_activation_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let mine = origin.push_activation(header(), &[], 1);
        let mut other = Heap::new();
        let _theirs = other.push_activation(header(), &[], 1);
        let _ = other.activation_local(mine, 0);
    }

    /// `ActivationReference::same` is how the unwinder stops at its target,
    /// and it is handed two handles
    /// and no `Heap`, so no accessor guard can reach it. Both handles below
    /// are the first entry of their own heap's stack, so they share an index
    /// and a release build answers "identical".
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap (comparing handle from heap")]
    fn comparing_two_activation_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let mine = origin.push_activation(header(), &[], 1);
        let mut other = Heap::new();
        let theirs = other.push_activation(header(), &[], 1);
        let _ = mine.same(theirs);
    }

    /// The comparison `same`'s doc comment says the unwinder never makes: a
    /// handle whose entry was popped, compared against the activation that
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
        let first = heap.push_activation(header(), &[], 1);
        heap.pop_activation(first);
        let second = heap.push_activation(header(), &[], 1);
        let _ = first.same(second);
    }

    /// Popping below the top would truncate the registers of every
    /// activation above it while their handles still look valid.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "only the top activation")]
    fn popping_an_activation_that_is_not_on_top_panics() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[], 1);
        let _inner = heap.push_activation(header(), &[], 1);
        heap.pop_activation(outer);
    }

    /// A hash is drawn from the counter when something first asks for one,
    /// not when the object is allocated, so allocation order is not
    /// observable (`docs/design/object-model.md`).
    #[test]
    fn an_identity_hash_is_assigned_on_first_request_not_at_allocation() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let first = heap.allocate_object(class, ObjectData::instance([]));
        let second = heap.allocate_object(class, ObjectData::instance([]));
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

    /// Every test above pushes `FrameKind::TopLevel`, which is `None` on
    /// both `activation_home` and `activation_captured` by construction --
    /// closure creation is the only reason either exists. `raise_return`
    /// picks a non-local return's target off `activation_home`, and the
    /// outer-local walk (`locals::frame_at_depth`) starts every hop off
    /// `activation_captured`/`frame_captured`, so a closure activation that
    /// answered `None` for either would silently misroute a `^` or a
    /// captured-variable read rather than fail loudly. The closure's two
    /// frames are distinct -- a block inside a block captures the outer
    /// block's activation and returns to the method's -- so that the two
    /// swapped would fail here rather than pass. The home is derived from
    /// the captured frame (`closure_home`) rather than stored, which is
    /// exactly what the two nesting levels below exercise: the inner
    /// closure's captured frame is a *block* activation, so its home is that
    /// frame's own. This also exercises `allocate_frame`'s debug assertion
    /// that `Frame.home` is populated exactly for a `FrameKind::Closure`
    /// activation, since `reify` builds that frame from a closure entry's
    /// `kind` here for the first time in this test module.
    #[test]
    fn a_closure_activations_home_and_captured_frame_are_its_closures() {
        let mut heap = Heap::new();
        fn block_closure<'gc>(
            heap: &mut Heap<'gc>,
            class: ClassReference<'gc>,
            captured_frame: FrameReference<'gc>,
        ) -> ObjectReference<'gc> {
            heap.allocate_object(
                class,
                ObjectData::Closure(ClosureData {
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
                    copied: Box::new([]),
                    feedback_base: FeedbackBase::NOT_ASSIGNED,
                }),
            )
        }
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let block_header = |closure| ActivationHeader {
            receiver: Value::nil(),
            current_class: None,
            kind: FrameKind::Closure(closure),
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        };

        let method_activation = heap.push_activation(header(), &[], 0);
        let home_frame = heap.reify(method_activation);
        let outer_closure = block_closure(&mut heap, class, home_frame);
        let outer_block_activation = heap.push_activation(block_header(outer_closure), &[], 0);
        let captured_frame = heap.reify(outer_block_activation);
        assert!(!captured_frame.same(home_frame));

        let closure = block_closure(&mut heap, class, captured_frame);
        let closure_activation = heap.push_activation(block_header(closure), &[], 0);

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

    /// A move gives every object and every frame a new index -- the
    /// rotation has no fixed point -- and rewrites everything that names
    /// one: an array's element, a closure's captured frame and its copied
    /// values, a frame's receiver, a class's own class, and the host roots.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_gives_every_slot_a_new_index_and_rewrites_what_names_it() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let string = heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("a")));
        let array = heap.allocate_object(
            class,
            ObjectData::array([heap.store(Value::object(string))]),
        );
        let frame = heap.allocate_frame(Frame {
            receiver: heap.store(Value::object(string)),
            ..top_level_frame()
        });
        heap.allocate_frame(top_level_frame());
        let closure = heap.allocate_object(
            class,
            ObjectData::Closure(ClosureData {
                code: trivial_code(),
                captured_frame: frame,
                copied: Box::new([heap.store(Value::object(string))]),
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            }),
        );
        let before = [
            heap.object_identity_key(class),
            heap.object_identity_key(string),
            heap.object_identity_key(array),
            heap.object_identity_key(closure),
        ];
        let frame_before = heap.frame_identity_key(frame);
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
            heap.frame_identity_key(captured_frame),
            "every frame moved"
        );
        assert!(heap
            .frame_receiver(captured_frame)
            .as_object()
            .is_some_and(|receiver| receiver.same(string)));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_an_object_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let string = heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("a")));
        move_bare_heap(&mut heap);
        let _ = heap.str_value(string);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_a_frame_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let frame = heap.allocate_frame(top_level_frame());
        heap.allocate_frame(top_level_frame());
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
        let class = heap.allocate_self_referential(ObjectData::instance([]));
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
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let stale =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("stale")));
        let array = heap.allocate_object(class, ObjectData::array([Word::NIL]));
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
            let not_a_class = mutator
                .heap
                .allocate_self_referential(ObjectData::instance([]));
            let instance = mutator
                .heap
                .allocate_object(not_a_class, ObjectData::instance([]));
            assert_eq!(
                mutator.heap.dispatch_id(Value::object(instance)),
                crate::mixin::DispatchId::NONE
            );
        });
    }

    /// Pins the release-build size that `CLAUDE.md`'s "An image is untrusted
    /// input" paragraph and `docs/design/memory.md` compute the decoder's
    /// placeholder-arena amplification from, so a change to `Object` that
    /// moves it fails here rather than leaving that figure silently wrong.
    /// The class handle, the eight-byte header word and `ObjectData`, whose
    /// largest payload is `ClosureData` -- an `Rc`, a `Box<[Value]>`, a
    /// `FrameReference` and a `FeedbackBase`, 32 bytes; `ClassData` is boxed
    /// so as not to be. The header word took the place of the separate
    /// identity hash and dispatch id and left the size at 56. Task 12 of the
    /// object-representation plan replaces this arena slot with the object's
    /// words, and this pin with theirs.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn an_object_is_the_size_the_amplification_figure_assumes() {
        assert_eq!(std::mem::size_of::<Object>(), 56);
    }

    /// The frame-arena entry's twin of the pin above: `docs/design/memory.md`
    /// derives the placeholder amplification from a `FrameSlot`'s release
    /// size as well, so a field that grows the `Frame` or the slot fails
    /// here rather than leaving that figure silently wrong.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_frame_slot_is_the_size_the_amplification_figure_assumes() {
        assert_eq!(std::mem::size_of::<Frame>(), 112);
        assert_eq!(std::mem::size_of::<FrameSlot>(), 120);
    }

    /// Liveness is reachability from the roots, and the roots are what the
    /// heap holds, not what a Rust local happens to name. `unreachable` is a
    /// perfectly valid object that nothing can get to, which is exactly the
    /// case the collector exists for.
    #[test]
    fn marking_reaches_what_a_root_reaches_and_nothing_else() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let reachable =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("live")));
        let unreachable =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("dead")));
        let activation = heap.push_activation(header(), &[Value::object(reachable)], 1);

        let marker = heap.mark_from_roots_for_test();

        assert!(marker.objects.is_set(reachable.index));
        assert!(marker.objects.is_set(class.index));
        assert!(!marker.objects.is_set(unreachable.index));
        heap.pop_activation(activation);
    }

    /// An edge is followed, not just a root: the value inside a reachable
    /// instance keeps its own target alive.
    #[test]
    fn marking_follows_an_edge_out_of_a_reachable_object() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let target = heap.allocate_object(
            class,
            ObjectData::string(&crate::CodePoints::from("target")),
        );
        let holder = heap.allocate_object(
            class,
            ObjectData::instance([heap.store(Value::object(target))]),
        );
        let activation = heap.push_activation(header(), &[Value::object(holder)], 1);

        let marker = heap.mark_from_roots_for_test();

        assert!(marker.objects.is_set(target.index));
        heap.pop_activation(activation);
    }

    /// A reified frame is reachable from the entry it is married to, which is
    /// what makes "a running activation's frame is never collected" fall out
    /// of the root set instead of needing a rule of its own.
    #[test]
    fn marking_reaches_a_married_frame_through_its_entry() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(header(), &[], 1);
        let frame = heap.reify(activation);

        let marker = heap.mark_from_roots_for_test();

        assert!(marker.frames.is_set(frame.index));
        heap.pop_activation(activation);
    }

    /// An activation object answers the frame it reifies and describes itself
    /// as an Activation; any other object answers no frame rather than
    /// panicking, which is what a kernel leaf handed the wrong object relies
    /// on.
    #[test]
    fn an_activation_object_answers_its_frame() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let frame = heap.allocate_frame(top_level_frame());
        let activation_object = heap.allocate_object(class, ObjectData::Activation(frame));
        let other =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("other")));

        let answered = heap
            .activation_object_frame(activation_object)
            .expect("an activation object answers its frame");
        assert_eq!(answered.index, frame.index);
        assert!(heap.activation_object_frame(other).is_none());
        assert_eq!(
            heap.describe(Value::object(activation_object)),
            "an Activation"
        );
    }

    /// The collector traces an activation object's frame: a frame that only
    /// an activation object reaches stays alive.
    #[test]
    fn marking_reaches_a_frame_through_an_activation_object() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let frame = heap.allocate_frame(top_level_frame());
        let activation_object = heap.allocate_object(class, ObjectData::Activation(frame));
        let activation = heap.push_activation(header(), &[Value::object(activation_object)], 1);

        let marker = heap.mark_from_roots_for_test();

        assert!(marker.objects.is_set(activation_object.index));
        assert!(marker.frames.is_set(frame.index));
        heap.pop_activation(activation);
    }

    /// Two ordinary objects reference each other through their `Instance`
    /// payloads, so the traversal reaches back to the first one through the
    /// second. This is what pins that the traversal terminates on a cycle:
    /// `MarkBits::set` answers "already set" for the first object the
    /// second time it is reached, which is what stops it from being pushed
    /// onto the work list again. A regression here hangs this test rather
    /// than failing it.
    #[test]
    fn marking_terminates_on_a_cycle_between_two_ordinary_objects() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let first = heap.allocate_object(class, ObjectData::instance([Word::NIL]));
        let second = heap.allocate_object(
            class,
            ObjectData::instance([heap.store(Value::object(first))]),
        );
        heap.instance_set_field(first, 0, Value::object(second));
        let activation = heap.push_activation(header(), &[Value::object(first)], 1);

        let marker = heap.mark_from_roots_for_test();

        assert!(marker.objects.is_set(first.index));
        assert!(marker.objects.is_set(second.index));
        heap.pop_activation(activation);
    }

    /// The point of the step: a slot nothing can reach is given back, and the
    /// arena is shorter afterwards.
    #[test]
    fn collecting_reclaims_what_no_root_reaches() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        let reachable =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("live")));
        heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("dead")));
        heap.allocate_object(
            class,
            ObjectData::string(&crate::CodePoints::from("also dead")),
        );
        let activation = heap.push_activation(header(), &[Value::object(reachable)], 1);
        let before = heap.objects.len();

        heap.collect_for_test();

        assert_eq!(before, 4);
        // The class, the reachable string, and nothing else.
        assert_eq!(heap.objects.len(), 2);
        heap.pop_activation(activation);
    }

    /// A survivor is reached again through the root that kept it alive, which
    /// is what pins that the forwarding rewrote the register rather than
    /// leaving it pointing at whatever now occupies the old index.
    #[test]
    fn a_survivor_is_still_itself_after_compaction() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("dead")));
        let reachable =
            heap.allocate_object(class, ObjectData::string(&crate::CodePoints::from("live")));
        let activation = heap.push_activation(header(), &[Value::object(reachable)], 1);

        heap.collect_for_test();

        let Some(survivor) = heap.activation_local(activation, 0).as_object() else {
            panic!("the register no longer holds an object");
        };
        assert_eq!(heap.str_value(survivor), Some("live".to_string()));
        heap.pop_activation(activation);
    }

    /// The growth policy's arithmetic (`docs/design/memory.md`, "The
    /// trigger and the growth policy"), pinned against the mistake its two
    /// near-identical lines invite: swapping which arena's length feeds
    /// which limit would still compile, and would only misbehave at the
    /// wrong time -- the wrong arena's limit still collects, just later or
    /// earlier than it should. Covers both a live count under the floor,
    /// where the floor must still win, and one above it in each arena, with
    /// the two counts kept different so a swapped read shows up as a wrong
    /// number rather than merely as the floor again.
    #[test]
    fn reset_collection_limits_reads_each_arenas_own_length() {
        let mut heap = Heap::new();
        heap.collection_floor = 8;

        // Zero live in either arena: both limits fall back to the floor.
        heap.reset_collection_limits();
        assert_eq!(heap.next_object_collection, 8);
        assert_eq!(heap.next_frame_collection, 8);

        // One frame, still under the floor once doubled (2 < 8): the floor
        // still wins.
        heap.allocate_frame(top_level_frame());
        heap.reset_collection_limits();
        assert_eq!(heap.next_object_collection, 8);
        assert_eq!(heap.next_frame_collection, 8);

        // Six objects and five frames in total -- above the floor once
        // doubled in both arenas, and still a different count in each.
        let class = heap.allocate_self_referential(ObjectData::instance([]));
        for _ in 0..5 {
            heap.allocate_object(class, ObjectData::instance([]));
        }
        for _ in 0..4 {
            heap.allocate_frame(top_level_frame());
        }
        let object_count = heap.objects.len();
        let frame_count = heap.frames.len();
        assert_ne!(object_count, frame_count);

        heap.reset_collection_limits();
        assert_eq!(heap.next_object_collection, object_count * 2);
        assert_eq!(heap.next_frame_collection, frame_count * 2);
    }

    /// Compaction shrinks an arena against its next limit, never against
    /// the live set alone: under the floor, a buffer cut to twice the live
    /// set had to grow back to the floor through a string of reallocations
    /// at every collection. A buffer within twice the limit is kept whole,
    /// and one past it is cut back to no less than the limit.
    #[test]
    fn compaction_never_shrinks_an_arena_below_its_next_limit() {
        let floor = 64;

        let mut within: Vec<u32> = Vec::with_capacity(2 * floor);
        within.extend(0..100);
        let kept = within.capacity();
        let _ = compact_arena(&mut within, &mark::MarkBits::new(100), floor);
        assert!(within.is_empty());
        assert_eq!(
            within.capacity(),
            kept,
            "a buffer within twice the limit is kept"
        );

        let mut past: Vec<u32> = Vec::with_capacity(16 * floor);
        past.extend(0..100);
        let _ = compact_arena(&mut past, &mark::MarkBits::new(100), floor);
        assert!(past.is_empty());
        assert!(
            (floor..16 * floor).contains(&past.capacity()),
            "a buffer past twice the limit is cut back, but not below the limit"
        );
    }
}
