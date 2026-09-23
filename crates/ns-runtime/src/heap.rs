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

use crate::interpreter::FeedbackBase;
use crate::mixin::{DispatchId, DispatchIds};
use crate::types::*;
use crate::Symbol;

pub(crate) mod mark;
mod world;
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

/// Where `immediate_dispatch_ids` keeps an immediate's id.
fn immediate_index(value: &Value<'_>) -> usize {
    match value {
        Value::Nil => 0,
        Value::Boolean(false) => 1,
        Value::Boolean(true) => 2,
        Value::SmallInteger(_) => 3,
        Value::Float(_) => 4,
        Value::Object(_) => unreachable!("an object carries its own class"),
    }
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
    /// `Signal::Return` names its target by `ActivationReference`, not by
    /// frame, so nothing in the runtime compares frames: the callers are the
    /// image round-trip tests (`tests/image.rs`) and this module's unit tests.
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
    /// Activation identity: `vm::unwind` finds the activation a
    /// `Signal::Return` targets by it. Two handles at the same index are the
    /// same activation because a return only ever unwinds downward -- nothing
    /// is pushed while a `Signal::Return` propagates, so the target's index
    /// cannot have been reused by the time the unwind compares it. A debug
    /// build checks that rather than assuming it.
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

/// Everything a new activation is apart from its registers, which
/// `Heap::push_activation` takes separately.
pub struct ActivationHeader<'gc> {
    pub receiver: Value<'gc>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6).
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: FrameKind<'gc>,
    /// See `Frame::slot_base`.
    pub slot_base: usize,
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
    /// does, except a block a control-flow native activates for a
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

/// The two conversions of a brand (`docs/design/memory.md`, "The brand"): a
/// host root's slot is shared with a host outside every entry, so it holds
/// `Value<'static>`, and `stash` passes a value through `at_rest` on the
/// way in, `fetch` through `branded` on the way out. Each is monomorphic in
/// its direction, so no single call carries a value from one entry's brand
/// to another's: composing them, `branded(at_rest(value))`, does, but a
/// misuse inside this module has to spell out both calls and pass visibly
/// through `'static` at the call site. Both are private to this module, and
/// there is no public counterpart: code outside `heap.rs` cannot change a
/// handle's brand at all.
fn at_rest(value: Value<'_>) -> Value<'static> {
    match value {
        Value::Nil => Value::Nil,
        Value::Boolean(boolean) => Value::Boolean(boolean),
        Value::SmallInteger(integer) => Value::SmallInteger(integer),
        Value::Float(float) => Value::Float(float),
        Value::Object(object) => Value::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: std::marker::PhantomData,
        }),
    }
}

/// See `at_rest`.
fn branded<'gc>(value: Value<'static>) -> Value<'gc> {
    match value {
        Value::Nil => Value::Nil,
        Value::Boolean(boolean) => Value::Boolean(boolean),
        Value::SmallInteger(integer) => Value::SmallInteger(integer),
        Value::Float(float) => Value::Float(float),
        Value::Object(object) => Value::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: std::marker::PhantomData,
        }),
    }
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
    slot_base: usize,
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
    values: Vec<Value<'gc>>,
    entries: Vec<StackEntry<'gc>>,
    /// How many of `entries` count against the depth limit. Kept as a
    /// running total, since the limit is checked on every push.
    counted: usize,
    #[cfg(debug_assertions)]
    next_generation: u32,
}

/// A reified activation and, while that activation is still running, the
/// stack entry it is married to. While `live` is set, the frame's own
/// `arguments`/`temporaries` are empty and its registers are the entry's.
struct FrameSlot<'gc> {
    frame: Frame<'gc>,
    live: Option<u32>,
}

/// The initial value of `Heap::collection_floor`, and of
/// `next_object_collection` and `next_frame_collection` before anything has
/// collected (`docs/design/memory.md`, "The trigger and the growth policy").
const COLLECTION_FLOOR: usize = 4096;

pub struct Heap<'gc> {
    /// The next identity hash to hand out. Hashes are assigned lazily, the
    /// first time `ensure_identity_hash` is asked for one, and never derived
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
    /// `immediate_index`: an immediate has no class field to read one from.
    /// Set once by the bootstrap; `NONE` before it.
    immediate_dispatch_ids: [DispatchId; 5],
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
/// them -- and answers the table that maps old index to new.
fn compact_arena<T>(slots: &mut Vec<T>, bits: &mark::MarkBits) -> ForwardingTable {
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
    // collection: shrink only when three quarters of the buffer is idle,
    // and leave the live set room to double.
    if slots.capacity() > destination.saturating_mul(4) {
        slots.shrink_to(destination.saturating_mul(2));
    }
    ForwardingTable { table }
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
            slot_base: _,
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
    /// second case is exactly as silent as it sounds, which is why
    /// `encode_code_table` and `mixins::plan` take a real `&Heap` parameter and
    /// `ns-cli`'s `disasm` reads through the interpreter that built its class
    /// object, instead of building one on the spot, and why running the test
    /// suite in debug is what catches the mistake if it recurs.
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
            #[cfg(debug_assertions)]
            heap_id: next_heap_id(),
            #[cfg(debug_assertions)]
            epoch: 0,
        }
    }

    /// Draws the next number from the counter. Saturating, not wrapping and not
    /// panicking: identity hashes are not required to be unique -- object
    /// identity is the arena index (`ObjectReference::same`), never the hash --
    /// so a world that exhausts the counter is better served by handing out a
    /// shared `u32::MAX` than by aborting the process. Reachable in *one*
    /// request once `image::load` has set the counter from a file:
    /// `advance_identity_hash_past` puts it wherever the image's maximum says.
    /// Never answers 0, which means "not assigned": the counter starts at 1 and
    /// only moves up.
    fn allocate_identity_hash(&mut self) -> u32 {
        let hash = self.next_identity_hash;
        self.next_identity_hash = self.next_identity_hash.saturating_add(1);
        hash
    }

    /// Moves the counter past everything an image brought with it. Never
    /// moves it backwards -- a lower maximum from a second load would
    /// otherwise let a fresh allocation collide with a decoded object.
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        if self.next_identity_hash <= max {
            self.next_identity_hash = max.saturating_add(1);
        }
    }

    // ---------- Object construction ----------
    //
    // Every construction of an `ObjectReference` in the workspace -- src or
    // tests -- goes through one of the two methods below, so that replacing
    // what backs a handle only ever touches this file.

    /// Allocates `data` under `class`. It carries no identity hash until
    /// something asks for one (`ensure_identity_hash`), so allocating draws
    /// nothing from the counter.
    pub fn allocate_object(
        &mut self,
        class: ClassReference<'gc>,
        data: ObjectData<'gc>,
    ) -> ObjectReference<'gc> {
        let dispatch_id = self.header_dispatch_id(class, &data);
        self.push_object(Object {
            class,
            identity_hash: 0,
            dispatch_id,
            data,
        })
    }

    /// An object that is genuinely its own `.class`. Under `Rc` this needed
    /// `Rc::new_uninit`, a manual strong-count bump and `Rc::from_raw`,
    /// because `Object.class` is non-optional and `Rc::new_cyclic` only ever
    /// offers a `Weak` that fails to upgrade until construction finishes.
    /// An arena has no such problem: the slot's index is known before the
    /// slot is filled, so the object can name itself in safe Rust.
    ///
    /// Like every object, it carries no identity hash until one is asked
    /// for.
    pub fn allocate_self_referential_class(
        &mut self,
        data: ObjectData<'gc>,
    ) -> ObjectReference<'gc> {
        let index = self.next_object_index();
        let handle = self.object_handle(index);
        // Its own class is itself, not yet in the arena to read, so the id
        // comes from `data` alone: its own class-side id when `data` is a
        // class, `NONE` otherwise -- the object is not a class object and,
        // being its own class, has no class that could supply an instances
        // id.
        let dispatch_id = match &data {
            ObjectData::Class(own) => own.dispatch_id.class_side(),
            _ => DispatchId::NONE,
        };
        self.objects.push(Object {
            class: handle,
            identity_hash: 0,
            dispatch_id,
            data,
        });
        handle
    }

    /// Where `allocate_object` enters an object into the arena;
    /// `allocate_self_referential_class` pushes directly
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
    // to objects, not activation records -- so there is only one
    // constructor, used for both a real activation and the placeholder
    // `image::decode::decode_graph_section`'s first pass allocates and its
    // second pass overwrites wholesale via `replace_frame`.

    /// Allocates `frame`. The only constructor of a `FrameReference` in the
    /// workspace -- `reify`, the image decoder and tests all come through
    /// here -- so that replacing what backs a handle only ever touches this
    /// file.
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
    /// checked in `replace_frame`: the decoder fills a placeholder through
    /// that path with fields read off the wire, and nothing reactivates a
    /// decoded frame, so it need not satisfy this.
    pub fn allocate_frame(&mut self, frame: Frame<'gc>) -> FrameReference<'gc> {
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

    // ---------- Handle-origin checks ----------
    //
    // Called as the first line of every accessor below that takes a handle:
    // an index from another heap is in range and names a different, valid
    // object or activation, so without this the read would just answer the
    // wrong thing. `describe` checks the object it was handed and does not
    // re-check the class it reaches through, which by construction came out
    // of the same heap.

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
    pub fn identity_hash(&self, object: ObjectReference<'gc>) -> u32 {
        self.check_object_origin(object);
        self.objects[object.index as usize].identity_hash
    }

    /// `object`'s identity hash, drawn from the counter the first time anything
    /// asks -- `Object>>hash` is the one caller. Assigned lazily, as Cog/Spur
    /// (`hashBitsOf:`, which calls `newHashBitsOf:` when the bits are 0) and
    /// Primordial Soup (`Object_identityHash`) do, so that allocation order is
    /// unobservable: the reference lowering allocates closures the optimized
    /// one does not, and closure conversion and a JIT's escape analysis will
    /// add and remove allocations the same way (`docs/design/object-model.md`).
    pub fn ensure_identity_hash(&mut self, object: ObjectReference<'gc>) -> u32 {
        self.check_object_origin(object);
        let stored = self.objects[object.index as usize].identity_hash;
        if stored != 0 {
            return stored;
        }
        let hash = self.allocate_identity_hash();
        self.objects[object.index as usize].identity_hash = hash;
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
    /// back onto a builtin class, the root class or `platform` -- none of
    /// them a record, so nothing else could restore what the image
    /// remembered for one -- and `tests/image.rs`'s
    /// `an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating`,
    /// which forces a live object's hash to one below the ceiling by hand,
    /// the only way to reach `advance_identity_hash_past`'s overflow guard
    /// without actually hashing four billion objects first.
    ///
    /// Calling this on anything else is a raw, unchecked overwrite of the one
    /// value identity-keyed lookups, `become:` and a loaded image's roots all
    /// rest on: it can leave two live objects sharing a hash, and every
    /// identity-keyed collection and `become:`'s stability guarantee then
    /// silently breaks, with nothing anywhere raising an error to say so.
    pub fn set_identity_hash(&mut self, object: ObjectReference<'gc>, hash: u32) {
        self.check_object_origin(object);
        self.objects[object.index as usize].identity_hash = hash;
    }

    pub fn class_of_object(&self, object: ObjectReference<'gc>) -> ClassReference<'gc> {
        self.check_object_origin(object);
        self.objects[object.index as usize].class
    }

    pub fn is_class(&self, object: ObjectReference<'gc>) -> bool {
        self.check_object_origin(object);
        matches!(
            self.objects[object.index as usize].data,
            ObjectData::Class(_)
        )
    }

    pub fn array_length(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Array(items) => Some(items.len()),
            _ => None,
        }
    }

    /// The number of fields in `object`'s `ObjectData::Instance`, mirroring
    /// `array_length`. `platform::execute` is the only caller: it
    /// tells the platform singleton, its stream and its `collections`
    /// namespace apart by field count rather than a builtin variant of
    /// their own (see `platform.rs`'s module doc), so the dispatcher needs
    /// the count itself, not any one field's value.
    pub fn instance_field_count(&self, object: ObjectReference<'gc>) -> Option<usize> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Instance(fields) => Some(fields.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, object: ObjectReference<'gc>, index: usize) -> Option<Value<'gc>> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Array(items) => items.get(index).copied(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.check_object_origin(object);
        if let ObjectData::Array(items) = &mut self.objects[object.index as usize].data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, object: ObjectReference<'gc>, value: Value<'gc>) -> bool {
        self.check_object_origin(object);
        if let ObjectData::Array(items) = &mut self.objects[object.index as usize].data {
            items.push(value);
            return true;
        }
        false
    }

    /// The instance field at `index`, or `None` when `object`'s payload is not
    /// `ObjectData::Instance` or `index` is out of range -- the same
    /// total-rather-than-panicking shape as the array accessors above.
    pub fn instance_field(&self, object: ObjectReference<'gc>, index: usize) -> Option<Value<'gc>> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Instance(fields) => fields.get(index).copied(),
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`.
    pub fn set_instance_field(
        &mut self,
        object: ObjectReference<'gc>,
        index: usize,
        value: Value<'gc>,
    ) -> bool {
        self.check_object_origin(object);
        if let ObjectData::Instance(fields) = &mut self.objects[object.index as usize].data {
            if index < fields.len() {
                fields[index] = value;
                return true;
            }
        }
        false
    }

    pub fn str_value(&self, object: ObjectReference<'gc>) -> Option<String> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::String(string) => Some(string.clone()),
            _ => None,
        }
    }

    pub fn symbol_value(&self, object: ObjectReference<'gc>) -> Option<Symbol> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::Symbol(symbol) => Some(*symbol),
            _ => None,
        }
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

    /// Whether `object` is a closure -- what a control-flow native asks of a
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
    /// how many parameters it declares, the frame it captured, whose
    /// receiver and current class the activation takes, and its feedback
    /// base, `FeedbackBase::NOT_ASSIGNED` for a closure an image decoded
    /// rather than one `MakeClosure` built. `None` when `object` is not a
    /// closure.
    pub(crate) fn closure_activation(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<(
        std::rc::Rc<crate::bytecode::CompiledCode>,
        usize,
        FrameReference<'gc>,
        FeedbackBase,
    )> {
        self.closure(object).map(|closure| {
            (
                closure.code.clone(),
                closure.code.argument_count as usize,
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

    /// A copy of closure `object`'s payload -- one `Rc` bump for the body it
    /// shares with the instruction that made it, and a copy of its copied
    /// values. For the image verifier and for tests; the encoder reads a closure
    /// through `object_snapshot`, and the run loop reads what it needs
    /// through the narrower accessors above.
    pub fn closure_data(&self, object: ObjectReference<'gc>) -> Option<ClosureData<'gc>> {
        self.closure(object).cloned()
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
    pub(crate) fn set_closure_feedback_base(
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

    /// A closure's home: the activation a `^` in its body returns from.
    /// Derived rather than stored, so that `ClosureData` stays 32 bytes --
    /// the captured frame itself when it is a method's, an initializer's or a
    /// clause's, its own `home` when it is a block's.
    ///
    /// A closure frame the runtime reified always carries a `home`
    /// (`allocate_frame` asserts it), so the fallback below is reached only
    /// by a *decoded* closure frame, which `replace_frame` fills straight
    /// from the wire. Answering the captured frame for one is what keeps an
    /// untrusted image from panicking here: a decoded frame is never married
    /// to an activation, so a `^` aimed at it is the `cannotReturn` error
    /// `raise_return` already raises for a dead home.
    pub fn closure_home(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>> {
        let captured = self.closure(object)?.captured_frame;
        let slot = &self.frames[captured.index as usize];
        Some(match &slot.frame.kind {
            FrameKind::Closure(_) => slot.frame.home.unwrap_or(captured),
            _ => captured,
        })
    }

    /// The whole of `object`'s record -- `class`, `identity_hash` and `data`
    /// -- in the one borrow all three live behind.
    /// `image::encode::encode_object` writes a whole object record and
    /// `GraphTable::visit_value` walks a whole object's references; both
    /// need every field, so one accessor beats writing the borrow out twice
    /// at the two call sites that each need all of it. Not for a hot path:
    /// it clones `data` in full, which is a real cost for a large `Array`
    /// or `Instance`, acceptable here because the encoder visits every
    /// object exactly once regardless.
    pub fn object_snapshot(&self, object: ObjectReference<'gc>) -> Object<'gc> {
        self.check_object_origin(object);
        self.objects[object.index as usize].clone()
    }

    /// Replaces the entirety of a placeholder object's contents in one
    /// write. `image::decode::decode_graph_section`'s second pass is the
    /// only caller, and it only ever calls this on a handle its first pass
    /// placeholder-allocated and nothing else has observed yet -- before
    /// `check_class_references` and `verify_graph` run, both of which
    /// assume every object is already filled in. Call this on any handle
    /// something else already holds -- after those passes ran, or on an
    /// object already reachable from `roots` -- and it silently replaces
    /// that object's already-verified invariants with unverified ones: no
    /// error, and no way for verification to notice, because verification
    /// has already finished. The object it installs carries `NONE` in its
    /// header until `stamp_dispatch_ids` runs -- `filled`'s class may itself
    /// be a still-placeholder record with no application yet -- and must not
    /// be dispatched on before then.
    pub fn replace_object(&mut self, object: ObjectReference<'gc>, filled: Object<'gc>) {
        self.check_object_origin(object);
        self.objects[object.index as usize] = filled;
    }

    /// Renders a value for a user-facing error message.
    ///
    /// Never `Debug`-format a `Value` instead: `ObjectReference`'s hand-written
    /// `Debug` does not recurse, but it prints Rust internals such as
    /// `ObjectReference(#42)`, which tells a Newspeak programmer nothing. Every
    /// caller of this is building the message of a `NewspeakError`, which
    /// reaches stderr, and that message needs to name what the value *is*
    /// -- "the String \"foo\"", not its Rust representation.
    pub fn describe(&self, value: &Value<'gc>) -> String {
        match value {
            Value::Nil => "nil".to_string(),
            Value::Boolean(boolean) => boolean.to_string(),
            Value::SmallInteger(integer) => integer.to_string(),
            Value::Float(float) => float.to_string(),
            Value::Object(object) => {
                self.check_object_origin(*object);
                match &self.objects[object.index as usize].data {
                    ObjectData::String(string) => format!("the String {string:?}"),
                    ObjectData::Symbol(symbol) => format!("the Symbol #{symbol}"),
                    ObjectData::Array(items) => format!("an Array of {}", items.len()),
                    ObjectData::Closure(_) => "a Block".to_string(),
                    ObjectData::Class(data) => {
                        format!("the class {}", data.application.mixin.name)
                    }
                    ObjectData::Instance(_) => {
                        let class = &self.objects[object.index as usize].class;
                        match &self.objects[class.index as usize].data {
                            ObjectData::Class(data) => {
                                format!("an instance of {}", data.application.mixin.name)
                            }
                            // `types.rs`'s `ClassReference` doc: every
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
    pub fn describe_argument(&self, got: Option<&Value<'gc>>) -> String {
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
        if let Value::Object(object) = value {
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
        if let Value::Object(object) = value {
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

    pub fn class_metaclass(&self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.metaclass,
            _ => None,
        }
    }

    /// The id `dispatch_id` answers for an object of `class` holding `data`:
    /// a class object's own class-side id, an instance's class's instances
    /// id, `NONE` when the class is not a class object -- only a crafted
    /// image makes an instance of one.
    fn header_dispatch_id(&self, class: ClassReference<'gc>, data: &ObjectData<'gc>) -> DispatchId {
        if let ObjectData::Class(own) = data {
            // Goes once a class object's own class is its metaclass
            // (`docs/design/object-model.md`, "Class sides and metaclasses";
            // step 6, `docs/PROGRESS.md`) -- then its id is its class's
            // instances id like any other object's.
            return own.dispatch_id.class_side();
        }
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
    pub fn dispatch_id(&self, value: &Value<'gc>) -> DispatchId {
        let Value::Object(object) = value else {
            return self.immediate_dispatch_ids[immediate_index(value)];
        };
        self.check_object_origin(*object);
        let entry = &self.objects[object.index as usize];
        debug_assert_eq!(
            entry.dispatch_id,
            self.header_dispatch_id(entry.class, &entry.data),
            "an object's dispatch id went stale: something changed its class's \
             application without restamping it"
        );
        entry.dispatch_id
    }

    /// `class`'s ids, or `NONE` when it is not a class object: the key of a
    /// `super` send and of a private lexical send. `ClassData` stores only
    /// the instances id (`ClassData::dispatch_id`'s doc comment); this
    /// rebuilds the pair from it.
    pub fn class_dispatch_ids(&self, class: ClassReference<'gc>) -> DispatchIds {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => DispatchIds {
                instances: data.dispatch_id,
                class_side: data.dispatch_id.class_side(),
            },
            _ => DispatchIds::NONE,
        }
    }

    pub(crate) fn set_immediate_dispatch_ids(&mut self, ids: [DispatchId; 5]) {
        self.immediate_dispatch_ids = ids;
    }

    /// Answers whether the write happened, mirroring `array_set`,
    /// `set_instance_field` and `set_class_superclass`/`set_class_application`
    /// below. `Mutator::metaclass_of` is the only caller, and it only
    /// ever calls this on a level it just confirmed is a class object a few
    /// lines above -- a wrong-shape `class` here would mean an uncached
    /// metaclass, which is silent: every later class-side send would rebuild
    /// instead of hitting the cache, and metaclass identity would stop being
    /// stable, which is what inline caches are meant to key on. The caller
    /// turns a `false` into an `unreachable!` rather than swallowing it.
    pub fn set_class_metaclass(
        &mut self,
        class: ClassReference<'gc>,
        metaclass: ClassReference<'gc>,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.metaclass = Some(metaclass);
            return true;
        }
        false
    }

    /// Relinks `class`'s superclass pointer. Answers whether the write
    /// happened, mirroring `set_class_application` -- the matching relink on
    /// the application chain that carries the layout, called right alongside
    /// this one. Two callers: the bootstrap's kernel phase, which links each
    /// builtin class to the one its `Kernel.ns` clause names
    /// (`interpreter::kernel`), and `tests/instantiate.rs`'s
    /// `superclass_slot_initializer_runs_before_the_subclass_own_slots`: a
    /// root class's superclass clause may name only `Object` or `Value`
    /// (the root rule, `instantiate::root_clause_defect`), so linking a
    /// subclass to an already-loaded sibling *user-defined* class has to
    /// happen by hand on the class-object chain `super`/`instantiate` walk.
    pub fn set_class_superclass(
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
    /// happened like `set_class_superclass`. The bootstrap's kernel phase
    /// is the one caller: it states that the kernel's `Object` has no
    /// superclass rather than rely on the placeholder already having none.
    pub fn clear_class_superclass(&mut self, class: ClassReference<'gc>) -> bool {
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
    pub fn set_class_lexical_context(
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

    /// Gives `class` a new application and moves every live object of that
    /// class to its ids -- a walk of the arena, which is why the decoder,
    /// which installs one application per class record, calls
    /// `install_class_application` instead and stamps once afterwards
    /// (`stamp_dispatch_ids`). Answers whether the write happened, mirroring
    /// `array_set` and `set_instance_field`. Its callers are the bootstrap's
    /// kernel phase, filling each builtin placeholder once
    /// (`interpreter::kernel`), and tests editing an already-live class
    /// (`install_mixin` in `tests/image.rs`, `tests/instantiate.rs`,
    /// `ns-cli`'s `disasm_snapshots.rs`) -- a wrong-shape `class` there is a
    /// bug of the caller's, not an untrusted-input case.
    pub fn set_class_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        if !self.install_class_application(class, application) {
            return false;
        }
        for index in 0..self.objects.len() {
            let entry_class = self.objects[index].class;
            if entry_class.index == class.index {
                let dispatch_id = self.header_dispatch_id(entry_class, &self.objects[index].data);
                self.objects[index].dispatch_id = dispatch_id;
            }
        }
        true
    }

    /// `set_class_application` without the walk: `class`'s own header is
    /// restamped, its instances' are not. The decoder's
    /// `rebuild_applications` uses this to install one application per
    /// decoded class record -- walking the arena on every one of those would
    /// be quadratic in a count the image controls -- and stamps every
    /// decoded object once afterwards, with `stamp_dispatch_ids`.
    pub(crate) fn install_class_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        self.check_object_origin(class);
        let entry = &mut self.objects[class.index as usize];
        if let ObjectData::Class(data) = &mut entry.data {
            data.dispatch_id = application.dispatch_ids.instances;
            data.application = application;
            entry.dispatch_id = data.dispatch_id.class_side();
            return true;
        }
        false
    }

    /// Stamps each of `objects` with the id its class now answers. The
    /// decoder's one pass, after `rebuild_applications` has given every
    /// decoded class its application.
    pub(crate) fn stamp_dispatch_ids(&mut self, objects: &[ObjectReference<'gc>]) {
        for object in objects {
            self.check_object_origin(*object);
            let entry = &self.objects[object.index as usize];
            let dispatch_id = self.header_dispatch_id(entry.class, &entry.data);
            self.objects[object.index as usize].dispatch_id = dispatch_id;
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
    // are not here. Three have only test callers: `set_frame_program_counter`,
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
        self.frames[frame.index as usize].frame.receiver
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
            stored_frame.temporaries.resize(needed, Value::Nil);
        }
    }

    /// Test and format level: the image tests use it to give a dead frame a
    /// synthetic paused `program_counter` before a snapshot. The runtime stores
    /// a running activation's `program_counter` on its stack entry
    /// (`set_activation_program_counter`), and the divorce copies it here.
    pub fn set_frame_program_counter(
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

    /// The whole of `frame`'s contents, read through one slice index
    /// rather than several. `image::encode::encode_frame` writes a whole
    /// frame record and `GraphTable::visit_frame` walks a whole frame's
    /// references; both need every field. Not for a hot path, for the same
    /// reason as `object_snapshot`.
    ///
    /// Only for a dead frame, which is also the encoder's precondition: it
    /// only ever runs between sends, when nothing is live, and
    /// `image::snapshot` refuses to start otherwise.
    pub fn frame_snapshot(&self, frame: FrameReference<'gc>) -> Frame<'gc> {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's program counter and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        self.frames[frame.index as usize].frame.clone()
    }

    /// The frame equivalent of `replace_object`.
    /// `image::decode::decode_graph_section`'s second pass is the only caller,
    /// and it only ever calls this on a handle its first pass
    /// placeholder-allocated and nothing else has observed yet -- before
    /// `verify_graph` runs, which assumes every frame is already filled in.
    /// Call this on any handle something else already holds and it silently
    /// replaces that frame's already-verified `program_counter`/register-file
    /// shape with an unverified one: no error, and no way for `verify_graph` to
    /// notice, because it has already finished.
    pub fn replace_frame(&mut self, frame: FrameReference<'gc>, filled: Frame<'gc>) {
        self.check_frame_origin(frame);
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
        self.stack.values.extend_from_slice(arguments);
        let end = self.stack.values.len() + temporary_count;
        self.stack.values.resize(end, Value::Nil);
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
            slot_base: header.slot_base,
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
            receiver: entry.receiver,
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
            slot_base: entry.slot_base,
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
        self.stack.values[self.register_slot(activation.index, index)]
    }

    pub fn activation_set_local(
        &mut self,
        activation: ActivationReference,
        index: u16,
        value: Value<'gc>,
    ) {
        self.check_activation(activation);
        let slot = self.register_slot(activation.index, index);
        self.stack.values[slot] = value;
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

    /// See `Frame::slot_base`.
    pub fn activation_slot_base(&self, activation: ActivationReference) -> usize {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].slot_base
    }

    /// Where `activation`'s code family starts in the heap's cache table.
    pub(crate) fn activation_feedback_base(&self, activation: ActivationReference) -> FeedbackBase {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].feedback_base
    }

    pub fn set_activation_program_counter(
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

    /// Whether `activation` is still on the stack -- the one question that may
    /// be asked of a handle whose activation has returned. A non-local return
    /// into an activation that catches none (an initializer) pops its target on
    /// its way out and goes on naming it, out to the host that started the run;
    /// the unwind asks this before it reads a target, and a host holding the
    /// escaped signal may ask nothing else. Nothing is pushed while a signal
    /// propagates (`ActivationReference::same`), so the target is live exactly
    /// while the stack still reaches its index; a debug build checks that the
    /// entry there is still the one the handle was minted for.
    ///
    /// A release build has only the index, so its answer is sound only
    /// while that holds. Anything that runs code while a signal propagates
    /// -- an `ensure:`, an `on:do:` handler, any unwind protection -- pushes
    /// activations at the indices the popped target used, and a release
    /// build would then take one of them for the target and land the return
    /// in the wrong activation. Such a mechanism needs a generation in every
    /// build, not only in a debug one.
    pub fn activation_is_live(&self, activation: ActivationReference) -> bool {
        let live = (activation.index as usize) < self.stack.entries.len();
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                activation.origin, self.heap_id,
                "handle belongs to a different heap (handle from heap {}, read through heap {})",
                activation.origin, self.heap_id
            );
            if live {
                assert_eq!(
                    self.stack.entries[activation.index as usize].generation, activation.generation,
                    "stale activation handle: its index was reused while it was still named"
                );
            }
        }
        live
    }

    /// What `activation` was activated for.
    pub fn activation_kind(&self, activation: ActivationReference) -> FrameKind<'gc> {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].kind.clone()
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
        match slot.live {
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
                        .unwrap_or(Value::Nil)
                }
            }
        }
    }

    /// `frame_local`'s twin, and silently a no-op for an index a dead
    /// frame's register file does not have, for the reason given there.
    pub fn frame_set_local(&mut self, frame: FrameReference<'gc>, index: u16, value: Value<'gc>) {
        self.check_frame_origin(frame);
        // Copied out first, so neither arm below holds a borrow of `frames`
        // while it writes.
        let live = self.frames[frame.index as usize].live;
        match live {
            Some(entry) => {
                let slot = self.register_slot(entry, index);
                self.stack.values[slot] = value;
            }
            None => {
                let stored_frame = &mut self.frames[frame.index as usize].frame;
                let register_index = index as usize;
                if register_index < stored_frame.arguments.len() {
                    stored_frame.arguments[register_index] = value;
                } else {
                    let temporary_index = register_index - stored_frame.arguments.len();
                    if let Some(slot) = stored_frame.temporaries.get_mut(temporary_index) {
                        *slot = value;
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
    /// allocation path: `allocate_self_referential_class` and the image
    /// decoder's direct use of `allocate_frame` compare against no limit at
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

    /// After a collection: no more is allowed to be allocated before the
    /// next one than is live, so the marking work per allocated slot is
    /// amortized constant.
    pub(crate) fn reset_collection_limits(&mut self) {
        self.collection_requested = false;
        self.next_object_collection = self
            .collection_floor
            .max(self.objects.len().saturating_mul(2));
        self.next_frame_collection = self
            .collection_floor
            .max(self.frames.len().saturating_mul(2));
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
        let objects = compact_arena(&mut self.objects, &marker.objects);
        let frames = compact_arena(&mut self.frames, &marker.frames);
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
        for value in &mut self.stack.values {
            forwarder.value(value);
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
        for value in &mut self.stack.values {
            marker.value(value);
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
            receiver: Value::Nil,
            arguments: Vec::new(),
            temporaries: Vec::new(),
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
            slot_base: 0,
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
        let class = origin.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let object = origin.allocate_object(class, ObjectData::String("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential_class(ObjectData::Instance(vec![]));
        other.allocate_object(other_class, ObjectData::String("theirs".to_string()));
        other.allocate_object(other_class, ObjectData::String("theirs".to_string()));

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
        let class = origin.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let mine = origin.allocate_object(class, ObjectData::String("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let theirs = other.allocate_object(other_class, ObjectData::String("theirs".to_string()));

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
    /// `allocate_self_referential_class` -- can produce several objects in one
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.allocate_object(class, ObjectData::Instance(Vec::new()));
        let second = heap.allocate_object(class, ObjectData::Instance(Vec::new()));

        assert_eq!(heap.identity_hash(first), 0);
        assert_eq!(heap.identity_hash(first), heap.identity_hash(second));
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
        let class = origin.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let mine = origin.allocate_object(class, ObjectData::String("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let theirs = other.allocate_object(other_class, ObjectData::String("theirs".to_string()));

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
    fn a_self_referential_class_is_its_own_class() {
        let mut heap = Heap::new();
        // A filler object first, so `class` does not land at index 0 -- an
        // implementation that always answered `ObjectReference { index: 0 }`
        // would otherwise still pass this test.
        heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        assert!(heap.class_of_object(class).same(class));
    }

    fn top_level_frame<'gc>() -> Frame<'gc> {
        Frame {
            receiver: Value::Nil,
            arguments: vec![],
            temporaries: vec![],
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
            slot_base: 0,
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
            arguments: vec![Value::Nil; argument_count],
            temporaries: vec![Value::Nil; temporary_count],
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
        assert!(matches!(heap.frame_receiver(first), Value::Nil));
        assert_eq!(heap.frame_register_shape(first), (3, 2, 1));
    }

    /// Frame identity is what the image round trip checks sharing by -- a
    /// closure's `captured_frame` and `home` naming one frame or two -- so two
    /// reified frames may never compare equal however alike their contents are.
    /// (`Signal::Return` names its target by `ActivationReference`, not by
    /// frame.)
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
        heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.allocate_object(class, ObjectData::String("first".to_string()));
        for _ in 0..1000 {
            heap.allocate_object(class, ObjectData::String("filler".to_string()));
        }
        assert_eq!(heap.str_value(first).as_deref(), Some("first"));
        assert!(heap.class_of_object(first).same(class));

        let twin = heap.allocate_object(class, ObjectData::String("first".to_string()));
        assert!(!first.same(twin));
    }

    /// A minimal class object real enough for `describe` to read a mixin
    /// name off it. Mirrors `types.rs`'s own `dummy_class` test helper,
    /// duplicated rather than shared because that one is private to
    /// `types.rs`'s test module.
    fn named_class<'gc>(heap: &mut Heap<'gc>, name: &str) -> ClassReference<'gc> {
        let application = std::rc::Rc::new(crate::mixin::MixinApplication {
            mixin: std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name))),
            superclass: None,
            inherits_value: false,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
        });
        heap.allocate_self_referential_class(ObjectData::Class(Box::new(ClassData::new(
            application,
            None,
            None,
            None,
            None,
        ))))
    }

    /// `describe`'s `ObjectData::Instance` arm is the one non-mechanical
    /// rewrite the arena conversion made: a nested read through a re-bound
    /// `class` intermediate rather than a single chained borrow. Nothing
    /// else in the workspace asserts its output, so pin it directly.
    #[test]
    fn describe_names_an_instance_by_its_class_mixin_name() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let instance = heap.allocate_object(class, ObjectData::Instance(vec![]));
        assert_eq!(
            heap.describe(&Value::Object(instance)),
            "an instance of Foo"
        );
    }

    /// The fallback arm of that same match: `class` names an `ObjectReference`
    /// whose own `data` is not `ObjectData::Class`, the shape a class reference
    /// decoded from an untrusted image can be in before verification runs.
    /// `describe` must still answer something rather than panic.
    #[test]
    fn describe_falls_back_to_an_object_for_a_class_reference_that_is_not_a_class() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let not_a_class =
            heap.allocate_object(class, ObjectData::String("not a class".to_string()));
        let instance = heap.allocate_object(not_a_class, ObjectData::Instance(vec![]));
        assert_eq!(heap.describe(&Value::Object(instance)), "an object");
    }

    /// The header every activation-stack test below pushes: nothing in it is
    /// read back except where a test sets it on purpose.
    fn header<'gc>() -> ActivationHeader<'gc> {
        ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::TopLevel,
            slot_base: 0,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        }
    }

    /// A body that returns its first register: no test here runs it.
    fn trivial_code() -> std::rc::Rc<crate::bytecode::CompiledCode> {
        std::rc::Rc::new(crate::bytecode::CompiledCode {
            kind: crate::bytecode::CodeKind::Method,
            instructions: vec![crate::bytecode::Instruction::Return { register: 0 }],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: Vec::new(),
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            source: ns_syntax::Span::new(0, 0),
        })
    }

    #[test]
    fn an_activation_addresses_its_arguments_before_its_temporaries() {
        let mut heap = Heap::new();
        let activation = heap.push_activation(
            header(),
            &[Value::SmallInteger(1), Value::SmallInteger(2)],
            2,
        );
        assert!(matches!(
            heap.activation_local(activation, 0),
            Value::SmallInteger(1)
        ));
        assert!(matches!(
            heap.activation_local(activation, 1),
            Value::SmallInteger(2)
        ));
        assert!(matches!(heap.activation_local(activation, 2), Value::Nil));
        heap.activation_set_local(activation, 3, Value::SmallInteger(9));
        assert!(matches!(
            heap.activation_local(activation, 3),
            Value::SmallInteger(9)
        ));
        heap.pop_activation(activation);
    }

    /// Two live activations share one `values` vector; neither may see the
    /// other's registers, whichever is written first.
    #[test]
    fn nested_activations_keep_separate_register_windows() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[Value::SmallInteger(1)], 1);
        let inner = heap.push_activation(header(), &[Value::SmallInteger(2)], 1);
        heap.activation_set_local(inner, 1, Value::SmallInteger(20));
        heap.activation_set_local(outer, 1, Value::SmallInteger(10));
        assert!(matches!(
            heap.activation_local(outer, 0),
            Value::SmallInteger(1)
        ));
        assert!(matches!(
            heap.activation_local(outer, 1),
            Value::SmallInteger(10)
        ));
        assert!(matches!(
            heap.activation_local(inner, 0),
            Value::SmallInteger(2)
        ));
        assert!(matches!(
            heap.activation_local(inner, 1),
            Value::SmallInteger(20)
        ));
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
            let activation = heap.push_activation(header(), &[Value::SmallInteger(1)], 3);
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
        let activation = heap.push_activation(header(), &[Value::SmallInteger(1)], 1);
        let frame = heap.reify(activation);
        heap.activation_set_local(activation, 1, Value::SmallInteger(5));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInteger(5)));
        heap.frame_set_local(frame, 1, Value::SmallInteger(7));
        assert!(matches!(
            heap.activation_local(activation, 1),
            Value::SmallInteger(7)
        ));
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
        let activation = heap.push_activation(header(), &[Value::SmallInteger(1)], 1);
        heap.set_activation_program_counter(activation, 4);
        let frame = heap.reify(activation);
        // What a `MakeClosure` whose block reads this frame by reference
        // does, and the only reason the registers below survive the pop.
        heap.retain_registers(activation);
        // Written after reification: it must survive the divorce.
        heap.activation_set_local(activation, 1, Value::SmallInteger(3));
        heap.pop_activation(activation);

        assert!(heap.frame_activation(frame).is_none());
        assert!(matches!(heap.frame_local(frame, 0), Value::SmallInteger(1)));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInteger(3)));
        assert_eq!(heap.frame_register_shape(frame), (4, 1, 1));
        assert!(heap.frame_is_uncontinuable(frame));
        assert!(heap.frame_sender(frame).is_none());
        heap.frame_set_local(frame, 1, Value::SmallInteger(8));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInteger(8)));
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let captured_frame = heap.allocate_frame(top_level_frame());
        let closure = heap.allocate_object(
            class,
            ObjectData::Closure(ClosureData {
                code: trivial_code(),
                captured_frame,
                copied: Box::new([Value::SmallInteger(7), Value::SmallInteger(8)]),
                feedback_base: FeedbackBase::NOT_ASSIGNED,
            }),
        );

        let activation = heap.push_activation(header(), &[Value::SmallInteger(1)], 1);
        heap.fill_copied_values(activation, closure);
        assert!(matches!(
            heap.activation_local(activation, 0),
            Value::SmallInteger(1)
        ));
        assert!(matches!(
            heap.activation_local(activation, 1),
            Value::SmallInteger(7)
        ));
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
        heap.activation_set_local(first, 1, Value::SmallInteger(3));
        let frame = heap.reify(first);
        heap.retain_registers(first);
        heap.pop_activation(first);

        let second = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(second, 1, Value::SmallInteger(99));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInteger(3)));
        heap.pop_activation(second);
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

    /// `ActivationReference::same` is how `vm::unwind` finds the activation a
    /// `Signal::Return` targets, and it is handed two handles
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

    /// The comparison `same`'s doc comment argues cannot happen: a catcher
    /// holding a handle whose entry was popped, compared against the
    /// activation that reused its index. A release build answers
    /// "identical", which would land a return in the wrong activation; a
    /// debug build must refuse the comparison instead of trusting that
    /// argument.
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

    /// `activation_is_live` is the one accessor a popped handle may reach:
    /// an escaping non-local return goes on naming the target it popped.
    #[test]
    fn a_popped_activation_is_no_longer_live() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[], 1);
        let inner = heap.push_activation(header(), &[], 1);
        assert!(heap.activation_is_live(inner));
        heap.pop_activation(inner);
        assert!(!heap.activation_is_live(inner));
        assert!(heap.activation_is_live(outer));
        heap.pop_activation(outer);
        assert!(!heap.activation_is_live(outer));
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.allocate_object(class, ObjectData::Instance(Vec::new()));
        let second = heap.allocate_object(class, ObjectData::Instance(Vec::new()));
        assert_eq!(heap.identity_hash(first), 0, "allocation assigns no hash");
        assert_eq!(heap.identity_hash(second), 0, "allocation assigns no hash");
        assert_eq!(
            heap.ensure_identity_hash(second),
            1,
            "the first request draws the first number"
        );
        assert_eq!(heap.ensure_identity_hash(first), 2);
        assert_eq!(
            heap.ensure_identity_hash(second),
            1,
            "a second request answers the same hash"
        );
        assert_eq!(heap.identity_hash(second), 1, "the assigned hash is stored");
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
                    code: std::rc::Rc::new(crate::bytecode::CompiledCode {
                        kind: crate::bytecode::CodeKind::Block,
                        instructions: vec![crate::bytecode::Instruction::ImplicitReturn {
                            register: 0,
                        }],
                        argument_count: 0,
                        register_count: 1,
                        statement_program_counter: vec![],
                        family_size: 0,
                        copied_count: 0,
                        outer_reach: 0,
                        source: ns_syntax::Span::new(0, 0),
                    }),
                    captured_frame,
                    copied: Box::new([]),
                    feedback_base: FeedbackBase::NOT_ASSIGNED,
                }),
            )
        }
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let block_header = |closure| ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::Closure(closure),
            slot_base: 0,
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let string = heap.allocate_object(class, ObjectData::String("a".to_string()));
        let array = heap.allocate_object(class, ObjectData::Array(vec![Value::Object(string)]));
        let frame = heap.allocate_frame(Frame {
            receiver: Value::Object(string),
            ..top_level_frame()
        });
        heap.allocate_frame(top_level_frame());
        let closure = heap.allocate_object(
            class,
            ObjectData::Closure(ClosureData {
                code: trivial_code(),
                captured_frame: frame,
                copied: Box::new([Value::Object(string)]),
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
            heap.stash(Value::Object(class)),
            heap.stash(Value::Object(string)),
            heap.stash(Value::Object(array)),
            heap.stash(Value::Object(closure)),
        ];
        move_bare_heap(&mut heap);
        let after: Vec<ObjectReference> = roots
            .iter()
            .map(|root| match heap.fetch(root) {
                Value::Object(object) => object,
                _ => panic!("a root keeps its object"),
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
            heap.class_of_object(class).same(class),
            "a class's own class is rewritten"
        );
        assert!(
            matches!(heap.array_at(array, 0), Some(Value::Object(element)) if element.same(string))
        );
        assert_eq!(heap.str_value(string).as_deref(), Some("a"));
        let data = heap.closure_data(closure).expect("still a closure");
        assert!(
            matches!(data.copied[..], [Value::Object(copied)] if copied.same(string)),
            "a copied value is rewritten"
        );
        assert_ne!(
            frame_before,
            heap.frame_identity_key(data.captured_frame),
            "every frame moved"
        );
        assert!(
            matches!(heap.frame_receiver(data.captured_frame), Value::Object(receiver) if receiver.same(string))
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_an_object_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let string = heap.allocate_object(class, ObjectData::String("a".to_string()));
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let root = heap.stash(Value::Object(class));
        move_bare_heap(&mut heap);
        let Value::Object(fresh) = heap.fetch(&root) else {
            panic!("a root keeps its object")
        };
        let _ = class.same(fresh);
    }

    /// A stale handle written into the heap -- a host that stored a value it
    /// kept past a run -- is caught by the next move, which finds it among
    /// the handles it forwards, rather than forwarded as if it were current.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle is stored in the heap")]
    fn a_stale_handle_stored_in_the_heap_is_caught_by_the_next_move() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let stale = heap.allocate_object(class, ObjectData::String("stale".to_string()));
        let array = heap.allocate_object(class, ObjectData::Array(vec![]));
        let root = heap.stash(Value::Object(array));
        move_bare_heap(&mut heap);
        let Value::Object(array) = heap.fetch(&root) else {
            panic!("a root keeps its object")
        };
        heap.array_push(array, Value::Object(stale));
        move_bare_heap(&mut heap);
    }

    /// A class object whose data is not a class -- only a crafted image makes
    /// an instance of one -- answers `NONE`, which nothing caches.
    #[test]
    fn an_instance_of_a_non_class_answers_no_dispatch_id() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let not_a_class = mutator
                .heap
                .allocate_self_referential_class(ObjectData::Instance(vec![]));
            let instance = mutator
                .heap
                .allocate_object(not_a_class, ObjectData::Instance(vec![]));
            assert_eq!(
                mutator.heap.dispatch_id(&Value::Object(instance)),
                crate::mixin::DispatchId::NONE
            );
        });
    }

    /// Pins the release-build size that `CLAUDE.md`'s "An image is untrusted
    /// input" paragraph and `docs/design/memory.md` compute the decoder's
    /// placeholder-arena amplification from, so a change to `Object` that
    /// moves it fails here rather than leaving that figure silently wrong.
    /// The header's dispatch id is paid for by boxing `ClassData`, and the
    /// size stays at 56 rather than 48 because `ClosureData` -- an `Rc`, a
    /// `Box<[Value]>`, a `FrameReference` and a `FeedbackBase`, 32 bytes --
    /// is `ObjectData`'s largest payload.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn an_object_is_the_size_the_amplification_figure_assumes() {
        assert_eq!(std::mem::size_of::<Object>(), 56);
    }

    /// Liveness is reachability from the roots, and the roots are what the
    /// heap holds, not what a Rust local happens to name. `unreachable` is a
    /// perfectly valid object that nothing can get to, which is exactly the
    /// case the collector exists for.
    #[test]
    fn marking_reaches_what_a_root_reaches_and_nothing_else() {
        let mut heap = Heap::new();
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
        let unreachable = heap.allocate_object(class, ObjectData::String("dead".to_string()));
        let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);

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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let target = heap.allocate_object(class, ObjectData::String("target".to_string()));
        let holder = heap.allocate_object(class, ObjectData::Instance(vec![Value::Object(target)]));
        let activation = heap.push_activation(header(), &[Value::Object(holder)], 1);

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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.allocate_object(class, ObjectData::Instance(vec![Value::Nil]));
        let second = heap.allocate_object(class, ObjectData::Instance(vec![Value::Object(first)]));
        heap.set_instance_field(first, 0, Value::Object(second));
        let activation = heap.push_activation(header(), &[Value::Object(first)], 1);

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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
        heap.allocate_object(class, ObjectData::String("dead".to_string()));
        heap.allocate_object(class, ObjectData::String("also dead".to_string()));
        let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        heap.allocate_object(class, ObjectData::String("dead".to_string()));
        let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
        let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);

        heap.collect_for_test();

        let Value::Object(survivor) = heap.activation_local(activation, 0) else {
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
        let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
        for _ in 0..5 {
            heap.allocate_object(class, ObjectData::Instance(vec![]));
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
}
