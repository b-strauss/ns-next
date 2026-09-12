//! The heap seam. Every read and write of an `Object` or an activation goes
//! through `Heap`, so that replacing what sits behind a handle changes this
//! file rather than its call sites. Objects and reified frames live in
//! arenas of typed handles (`ObjRef`, `FrameRef`); see
//! `docs/design/memory.md`. A running activation lives on the activation
//! stack, also owned here, and is named by an `ActivationRef` that nothing
//! may store; a frame exists for it only once `reify` creates one. The
//! accessor prefix says which handle an accessor takes: `activation_*` a
//! live stack entry, `frame_*` a reified frame. See
//! `docs/design/execution.md`, "Activations".
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/locals.rs`.

use crate::types::*;
use crate::Symbol;

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

/// An opaque handle to an object: an index into `Heap.objects`. The inner
/// index is private to this module, and only `heap.rs` reaches it -- that
/// privacy is what made replacing `Rc<RefCell<Object>>` with this a change
/// to one file rather than to every caller. See `docs/design/memory.md`,
/// "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two live
/// objects never share an index.
#[derive(Clone, Copy)]
pub struct ObjRef {
    index: u32,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_object_origin`.
    #[cfg(debug_assertions)]
    origin: u32,
}

impl ObjRef {
    /// Object identity. Replaces `Rc::ptr_eq` at every call site, so that
    /// identity has one spelling that survives the store changing under it.
    ///
    /// Guarded like an accessor, and for a sharper reason. Comparing two
    /// handles from different heaps needs no `Heap` to go wrong, so nothing
    /// else would catch it -- and the wrong answer here is a silent *yes*:
    /// two unrelated objects that happen to share an index would be reported
    /// identical. `check_object_origin` covers every read; this covers the
    /// one operation that reads nothing.
    pub fn same(self, other: ObjRef) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
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
impl std::fmt::Debug for ObjRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjRef(#{})", self.index)
    }
}

/// A class is an `ObjRef` whose `data` is `ObjectData::Class(_)`. Not
/// enforced by the type system: `ClassRef` is an alias, not a distinct
/// handle type.
pub type ClassRef = ObjRef;

/// An opaque handle to a *reified* activation: an index into `Heap.frames`.
/// The inner index is private to this module. See `docs/design/memory.md`,
/// "The seam".
///
/// A running activation is named by an `ActivationRef` instead; a `FrameRef`
/// exists for one only once something must hold it past the current call
/// (`Heap::reify`). This is the only activation handle anything may store --
/// in a closure, in another frame, in an image.
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two frames
/// never share an index.
#[derive(Clone, Copy)]
pub struct FrameRef {
    index: u32,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_frame_origin`.
    #[cfg(debug_assertions)]
    origin: u32,
}

impl FrameRef {
    /// Frame identity. Guarded like an accessor: see `ObjRef::same`.
    /// `Signal::Return` names its target by `ActivationRef`, not by frame, so
    /// nothing in the runtime compares frames: the callers are the image
    /// round-trip tests (`tests/image.rs`) and this module's unit tests.
    pub fn same(self, other: FrameRef) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
        );
        self.index == other.index
    }
}

/// Prints the arena index and nothing else. A derived `Debug` over a
/// frame's contents would walk the sender chain, which has no acyclic
/// guarantee to lean on (a closure's `home` can itself be captured); an
/// index has nothing to descend into at all, so the hazard is gone by
/// construction rather than by discipline, and printing one cannot panic.
impl std::fmt::Debug for FrameRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameRef(#{})", self.index)
    }
}

/// An opaque handle to a *live* activation: an entry on the `Heap`'s
/// activation stack. Where a `FrameRef` names a reified activation that may
/// be stored anywhere, an `ActivationRef` is valid only until its entry is
/// popped -- the next activation at that depth reuses the index. That is
/// why nothing stores one: `ClosureData`, `Frame` and every image record
/// take a `FrameRef`, which `Heap::reify` answers for a live activation.
/// See `docs/design/execution.md`, "Activations", "Representation".
///
/// 4 bytes in a release build. A debug build adds the heap's id, like every
/// handle, and a `generation` drawn at each push, so that reading through a
/// handle whose entry was popped panics instead of answering for whichever
/// activation reuses the index.
#[derive(Clone, Copy)]
pub struct ActivationRef {
    index: u32,
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    generation: u32,
}

impl ActivationRef {
    /// Activation identity: `invoke_method` and `run_clause_body` catch a
    /// `Signal::Return` by it. Two handles at the same index are the same
    /// activation because a return only ever unwinds downward -- nothing is
    /// pushed while a `Signal::Return` propagates, so the target's index
    /// cannot have been reused by the time a catcher compares it. A debug
    /// build checks that rather than assuming it.
    pub fn same(self, other: ActivationRef) -> bool {
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

/// Prints the stack index and nothing else, for the reason `FrameRef`'s does.
impl std::fmt::Debug for ActivationRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActivationRef(#{})", self.index)
    }
}

/// Everything a new activation is apart from its registers, which
/// `Heap::push_activation` takes separately.
pub struct ActivationHeader {
    pub receiver: Value,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6).
    pub current_class: Option<ClassRef>,
    pub kind: FrameKind,
    /// See `Frame::slot_base`.
    pub slot_base: usize,
}

/// One live activation. Its registers are `values[base..]` up to the next
/// entry's `base` (or the end of `values`, for the top entry): the
/// arguments first, then the temporaries.
struct StackEntry {
    receiver: Value,
    current_class: Option<ClassRef>,
    kind: FrameKind,
    base: u32,
    num_args: u32,
    pc: usize,
    slot_base: usize,
    /// The frame `Heap::reify` created for this activation, married to it
    /// until the entry is popped.
    reified: Option<FrameRef>,
    #[cfg(debug_assertions)]
    generation: u32,
}

/// Every live activation, innermost last. Both vectors keep their capacity
/// across pops, so once they have grown to the program's depth a push
/// allocates nothing.
#[derive(Default)]
struct ActivationStack {
    values: Vec<Value>,
    entries: Vec<StackEntry>,
    #[cfg(debug_assertions)]
    next_generation: u32,
}

/// A reified activation and, while that activation is still running, the
/// stack entry it is married to. While `live` is set, the frame's own
/// `args`/`temps` are empty and its registers are the entry's.
struct FrameSlot {
    frame: Frame,
    live: Option<u32>,
}

pub struct Heap {
    /// Monotonic, assigned at allocation, never derived from an address, so
    /// identity survives both a moving collector and a snapshot
    /// (`docs/design/memory.md`).
    next_identity_hash: u32,
    /// Every *reified* activation, addressed by a `FrameRef`'s index: one
    /// `Heap::reify` created because a block captured it, or one the image
    /// decoder rebuilt. Nothing is ever removed: reclaiming a slot needs a
    /// collector. An activation that is never reified never reaches this
    /// arena at all.
    frames: Vec<FrameSlot>,
    /// Every live activation. A stack entry is popped when its activation
    /// returns, so -- unlike `frames` and `objects` -- this is bounded by
    /// the deepest the program has been, not by everything it has run.
    stack: ActivationStack,
    /// Every object, addressed by an `ObjRef`'s index. Nothing is ever
    /// removed, for the same reason as `frames`.
    objects: Vec<Object>,
    /// This heap's distinct id, stamped onto every handle it mints. Debug
    /// builds only; see `next_heap_id`. No accessor exists for it: nothing
    /// outside a `cfg`-guarded position may name a field that does not
    /// exist in a release build.
    #[cfg(debug_assertions)]
    heap_id: u32,
}

/// Prints the counter and both arenas' lengths, never their contents. A
/// derived `Debug` would walk every activation and every object ever
/// allocated, and a method's declaration reachable through a frame's kind
/// would print a whole AST -- the same hazard `CLAUDE.md`'s
/// "never `{:?}` a `Value`" names for a single object, now reachable
/// through the heap itself rather than through one handle. Nothing prints
/// a `Heap` today, so this is latent, but a derive would have turned a
/// future `{:?}` on a `Heap` -- or on anything that embeds one -- into an
/// unbounded print of every activation's and every object's data.
impl std::fmt::Debug for Heap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Heap")
            .field("next_identity_hash", &self.next_identity_hash)
            .field("frames.len()", &self.frames.len())
            .field("activations", &self.stack.entries.len())
            .field("objects.len()", &self.objects.len())
            .finish()
    }
}

// `Default` is kept only so clippy's `new_without_default` stays quiet; see
// `Heap::new`'s doc comment below for why building one on the spot is not
// generally safe.
impl Default for Heap {
    fn default() -> Self {
        Heap::new()
    }
}

impl Heap {
    /// A `Heap` built on the spot -- rather than the one `Interpreter` owns
    /// -- is an empty arena: every object and frame accessor indexes
    /// directly into `self.objects`/`self.frames`, so an `ObjRef` or
    /// `FrameRef` minted by a different `Heap` either falls out of range and
    /// panics, or -- should the two arenas happen to share a length --
    /// names a different object or activation. In a debug build the second
    /// case is refused, not silent: every handle carries the id of the heap
    /// that minted it, and every accessor checks it against `self.heap_id`
    /// before indexing, so reading through the wrong heap panics at the read
    /// rather than answering. That check compiles out of a release build --
    /// there the second case is exactly as silent as it sounds, which is why
    /// `encode_code_table` and `mixins::plan` take a real `&Heap` parameter
    /// and `ns-cli`'s `disasm` reads through the interpreter that built its
    /// class object, instead of building one on the spot, and why running
    /// the test suite in debug is what catches the mistake if it recurs.
    pub fn new() -> Heap {
        Heap {
            next_identity_hash: 1,
            frames: Vec::new(),
            stack: ActivationStack::default(),
            objects: Vec::new(),
            #[cfg(debug_assertions)]
            heap_id: next_heap_id(),
        }
    }

    /// Saturating, not wrapping and not panicking: identity hashes are not
    /// required to be unique (`dispatch.rs`'s placeholder class objects
    /// already share hash 0, and `hash` is the only thing that reads one --
    /// object identity is the arena index (`ObjRef::same`), never the hash),
    /// so a world that exhausts the counter is better served by handing out
    /// a shared `u32::MAX` than by aborting the process. Reachable in *one*
    /// allocation once `image::load` may set the counter from a file:
    /// `advance_identity_hash_past` puts it wherever the image's maximum
    /// says, so the `saturating_add(1)` below is on an image-controlled
    /// path.
    pub fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash = self.next_identity_hash.saturating_add(1);
        h
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
    // Every construction of an `ObjRef` in the workspace -- src or tests --
    // goes through one of the three methods below, so that replacing what
    // backs a handle only ever touches this file.

    /// Allocates `data` under `class`, with a fresh identity hash drawn from
    /// the counter. The ordinary constructor -- everything that is a real,
    /// observable object goes through this one.
    pub fn alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        self.push_object(Object {
            class,
            identity_hash,
            data,
        })
    }

    /// Allocates `data` under `class` with identity hash fixed at 0,
    /// bypassing the counter entirely -- for a record nothing may observe
    /// as itself. `image::decode`'s two-pass graph section calls this for
    /// the placeholder it is about to overwrite wholesale via
    /// `replace_object` once the real record is read, so drawing a fresh
    /// hash per placeholder would burn through the counter for objects
    /// nothing keeps. `dispatch.rs`'s and `tests/interpreter.rs`'s
    /// hand-built class fixtures use it for the same reason `dummy_class`
    /// does via `alloc_self_referential_class` below: they are never linked
    /// into a real hierarchy, so which hash they carry does not matter, and
    /// 0 is the value every other such placeholder already shares
    /// (`alloc_identity_hash`'s own doc comment).
    pub fn alloc_placeholder_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        self.push_object(Object {
            class,
            identity_hash: 0,
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
    /// Identity hash fixed at 0, like `alloc_placeholder_object`: none of
    /// the four callers links this object into a real hierarchy a hash
    /// could distinguish it within.
    pub fn alloc_self_referential_class(&mut self, data: ObjectData) -> ObjRef {
        let index = self.next_object_index();
        let handle = self.object_handle(index);
        self.objects.push(Object {
            class: handle,
            identity_hash: 0,
            data,
        });
        handle
    }

    /// Where `alloc_object` and `alloc_placeholder_object` enter an object
    /// into the arena; `alloc_self_referential_class` pushes directly
    /// instead, because it needs the index before the object exists to name
    /// itself as its own `class`, but it enforces the same ceiling by
    /// calling `next_object_index` itself. The ceiling is the
    /// representation's, not an implementation limit: a handle is 32 bits
    /// by design (`docs/design/memory.md`), so a heap holding more than
    /// `u32::MAX` objects cannot be addressed at all, and failing loudly at
    /// the allocation beats handing out an index that wrapped.
    fn push_object(&mut self, object: Object) -> ObjRef {
        let index = self.next_object_index();
        self.objects.push(object);
        self.object_handle(index)
    }

    fn next_object_index(&self) -> u32 {
        u32::try_from(self.objects.len()).expect("more than u32::MAX objects allocated in one heap")
    }

    /// The one place an `ObjRef` is built, so the `cfg` on `origin` appears
    /// once rather than at every allocation. A struct-expression field may
    /// carry a `cfg` attribute, which is what makes the release build see a
    /// plain 4-byte handle.
    fn object_handle(&self, index: u32) -> ObjRef {
        ObjRef {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
        }
    }

    fn frame_handle(&self, index: u32) -> FrameRef {
        FrameRef {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
        }
    }

    // ---------- Frame construction ----------
    //
    // A frame carries no identity hash -- Newspeak's `identityHash` applies
    // to objects, not activation records -- so there is only one
    // constructor, used for both a real activation and the placeholder
    // `image::decode::decode_graph_section`'s first pass allocates and its
    // second pass overwrites wholesale via `replace_frame`.

    /// Allocates `frame`. The only constructor of a `FrameRef` in the
    /// workspace -- `reify`, the image decoder and tests all come through
    /// here -- so that replacing what backs a handle only ever touches this
    /// file.
    ///
    /// `Frame.home` restates what `Frame.kind` already carries for a closure
    /// frame -- its closure's `home` -- and the image records both, so the
    /// two must agree: `home` populated exactly for a `FrameKind::Closure`
    /// activation. `reify` upholds that by building `home` from the entry's
    /// `kind`, but nothing in the type system ties the two fields together,
    /// so a new constructor added later -- or a mismatched pair of literal
    /// fields at an existing one -- would record a return target that
    /// disagrees with the frame's own closure silently rather than fail
    /// loudly. Checked here, once, rather than at every constructor. Not
    /// checked in `replace_frame`: the decoder fills a placeholder through
    /// that path with fields read off the wire, and nothing reactivates a
    /// decoded frame, so it need not satisfy this.
    pub fn alloc_frame(&mut self, frame: Frame) -> FrameRef {
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
    fn check_object_origin(&self, obj: ObjRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            obj.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            obj.origin, self.heap_id
        );
        let _ = obj;
    }

    #[inline]
    fn check_frame_origin(&self, frame: FrameRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            frame.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            frame.origin, self.heap_id
        );
        let _ = frame;
    }

    /// `check_frame_origin` for a live activation, plus the check only a
    /// stack handle needs: that its entry is still the one it was minted
    /// for. Compiles to nothing in a release build.
    #[inline]
    fn check_activation(&self, act: ActivationRef) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                act.origin, self.heap_id,
                "handle belongs to a different heap (handle from heap {}, read through heap {})",
                act.origin, self.heap_id
            );
            let live = self
                .stack
                .entries
                .get(act.index as usize)
                .map(|entry| entry.generation);
            assert_eq!(
                live,
                Some(act.generation),
                "stale activation handle: its activation has returned"
            );
        }
        let _ = act;
    }

    /// The handle for live entry `index`, minted with that entry's own
    /// generation. The one place an `ActivationRef` is built.
    fn activation_handle(&self, index: u32) -> ActivationRef {
        ActivationRef {
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

    pub fn identity_hash(&self, obj: ObjRef) -> u32 {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].identity_hash
    }

    /// An opaque, hashable key unique to `obj`, for code that needs an
    /// `ObjRef` as a `HashMap`/`HashSet` key rather than a comparison --
    /// `image::encode::GraphTable`'s object-id map,
    /// `image::decode::rebuild_applications`'s superclass-position map and
    /// `interpreter::nested::in_progress_key`. `identity_hash` cannot serve
    /// that role, because two placeholder objects may legitimately share
    /// hash 0 (`alloc_placeholder_object`, `alloc_self_referential_class`),
    /// which would fold two distinct objects into one table entry.
    ///
    /// It lives on `Heap` rather than on `ObjRef` so that the origin check
    /// applies to it like it does to every accessor. Two handles from
    /// different heaps carry the same index and so produce the same key,
    /// which collides silently in a caller's map; requiring the `Heap` that
    /// minted the handle is what turns that into a panic, and it is the
    /// compiler that requires it, since there is no way to reach the index
    /// without one.
    ///
    /// This and `frame_identity_key` are the only ways a handle's index
    /// leaves this module, so anything that changes what a handle encodes --
    /// widening it to carry a heap tag, say -- has to check their callers
    /// even though the field itself is private. They all treat the result as
    /// an opaque key, which is what keeps that a check rather than a rewrite.
    pub(crate) fn object_identity_key(&self, obj: ObjRef) -> usize {
        self.check_object_origin(obj);
        obj.index as usize
    }

    /// Overwrites `obj`'s identity hash. `tests/image.rs`'s
    /// `an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating` is
    /// the only caller: it forces a live object's hash to one below the
    /// ceiling by hand, which is the only way to reach
    /// `advance_identity_hash_past`'s overflow guard without actually
    /// allocating four billion objects first.
    ///
    /// Calling this on anything but that one object under test is a raw,
    /// unchecked overwrite of the one value identity-keyed lookups,
    /// `become:` and a loaded image's roots all rest on: it can leave two
    /// live objects sharing a hash, and every identity-keyed collection and
    /// `become:`'s stability guarantee then silently breaks, with nothing
    /// anywhere raising an error to say so.
    pub fn set_identity_hash(&mut self, obj: ObjRef, hash: u32) {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].identity_hash = hash;
    }

    pub fn class_of_obj(&self, obj: ObjRef) -> ClassRef {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].class
    }

    pub fn is_class(&self, obj: ObjRef) -> bool {
        self.check_object_origin(obj);
        matches!(self.objects[obj.index as usize].data, ObjectData::Class(_))
    }

    pub fn array_len(&self, obj: ObjRef) -> Option<usize> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => Some(items.len()),
            _ => None,
        }
    }

    /// The number of fields in `obj`'s `ObjectData::Instance`, mirroring
    /// `array_len`. `platform::dispatch` is the only caller: it
    /// tells the platform singleton, its stream and its `collections`
    /// namespace apart by field count rather than a builtin variant of
    /// their own (see `platform.rs`'s module doc), so the dispatcher needs
    /// the count itself, not any one field's value.
    pub fn instance_field_count(&self, obj: ObjRef) -> Option<usize> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Instance(fields) => Some(fields.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, obj: ObjRef) -> Option<Vec<Value>> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, obj: ObjRef, index: usize) -> Option<Value> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => items.get(index).copied(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(&mut self, obj: ObjRef, index: usize, value: Value) -> bool {
        self.check_object_origin(obj);
        if let ObjectData::Array(items) = &mut self.objects[obj.index as usize].data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, obj: ObjRef, value: Value) -> bool {
        self.check_object_origin(obj);
        if let ObjectData::Array(items) = &mut self.objects[obj.index as usize].data {
            items.push(value);
            return true;
        }
        false
    }

    /// The instance field at `index`, or `None` when `obj`'s payload is not
    /// `ObjectData::Instance` or `index` is out of range -- the same
    /// total-rather-than-panicking shape as the array accessors above.
    pub fn instance_field(&self, obj: ObjRef, index: usize) -> Option<Value> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Instance(fields) => fields.get(index).copied(),
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`.
    pub fn set_instance_field(&mut self, obj: ObjRef, index: usize, value: Value) -> bool {
        self.check_object_origin(obj);
        if let ObjectData::Instance(fields) = &mut self.objects[obj.index as usize].data {
            if index < fields.len() {
                fields[index] = value;
                return true;
            }
        }
        false
    }

    pub fn str_value(&self, obj: ObjRef) -> Option<String> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn sym_value(&self, obj: ObjRef) -> Option<Symbol> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Sym(s) => Some(*s),
            _ => None,
        }
    }

    /// An `Rc` bump, not a copy: a closure is immutable once made.
    pub fn closure_data(&self, obj: ObjRef) -> Option<std::rc::Rc<ClosureData>> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(c) => Some(c.clone()),
            _ => None,
        }
    }

    /// The whole of `obj`'s record -- `class`, `identity_hash` and `data`
    /// -- in the one borrow all three live behind.
    /// `image::encode::encode_object` writes a whole object record and
    /// `GraphTable::visit_value` walks a whole object's references; both
    /// need every field, so one accessor beats writing the borrow out twice
    /// at the two call sites that each need all of it. Not for a hot path:
    /// it clones `data` in full, which is a real cost for a large `Array`
    /// or `Instance`, acceptable here because the encoder visits every
    /// object exactly once regardless.
    pub fn object_snapshot(&self, obj: ObjRef) -> Object {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].clone()
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
    /// has already finished.
    pub fn replace_object(&mut self, obj: ObjRef, filled: Object) {
        self.check_object_origin(obj);
        self.objects[obj.index as usize] = filled;
    }

    /// Renders a value for a user-facing error message.
    ///
    /// Never `Debug`-format a `Value` instead: `ObjRef`'s hand-written
    /// `Debug` does not recurse, but it prints Rust internals such as
    /// `ObjRef(#42)`, which tells a Newspeak programmer nothing. Every
    /// caller of this is building the message of a `NewspeakError`, which
    /// reaches stderr, and that message needs to name what the value *is*
    /// -- "the String \"foo\"", not its Rust representation.
    pub fn describe(&self, value: &Value) -> String {
        match value {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::SmallInt(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Obj(obj) => {
                self.check_object_origin(*obj);
                match &self.objects[obj.index as usize].data {
                    ObjectData::Str(s) => format!("the String {s:?}"),
                    ObjectData::Sym(s) => format!("the Symbol #{s}"),
                    ObjectData::Array(items) => format!("an Array of {}", items.len()),
                    ObjectData::Closure(_) => "a Block".to_string(),
                    ObjectData::Class(data) => {
                        format!("the class {}", data.application.mixin.name)
                    }
                    ObjectData::Instance(_) => {
                        let class = &self.objects[obj.index as usize].class;
                        match &self.objects[class.index as usize].data {
                            ObjectData::Class(data) => {
                                format!("an instance of {}", data.application.mixin.name)
                            }
                            // `types.rs`'s `ClassRef` doc: every `Object::class`
                            // is an `ObjRef` whose own `data` is
                            // `ObjectData::Class`, by convention rather than by
                            // the type system. This arm exists only so the match
                            // is total, not because a conforming object can
                            // reach it.
                            _ => "an object".to_string(),
                        }
                    }
                }
            }
        }
    }

    /// The `Option` form every one of these error constructors needs, so the
    /// `map_or` is not written out three times.
    pub fn describe_arg(&self, got: Option<&Value>) -> String {
        match got {
            Some(v) => self.describe(v),
            None => "no argument".to_string(),
        }
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
        class: ClassRef,
    ) -> Option<std::rc::Rc<crate::mixin::MixinApplication>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    pub fn class_enclosing(&self, class: ClassRef) -> Option<ObjRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.enclosing,
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: ClassRef) -> Option<ClassRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.declaring_class,
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: ClassRef) -> Option<ClassRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.superclass,
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: ClassRef) -> Option<BuiltinClass> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.builtin,
            _ => None,
        }
    }

    /// A lookup level's builtin kind and superclass in one borrow, or `None`
    /// when `class` is not a class object -- which is what ends a `walk` that
    /// stepped onto one through `next_level`'s unverified class-object chain.
    pub fn class_lookup_step(
        &self,
        class: ClassRef,
    ) -> Option<(Option<BuiltinClass>, Option<ClassRef>)> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some((data.builtin, data.superclass)),
            _ => None,
        }
    }

    pub fn class_metaclass(&self, class: ClassRef) -> Option<ClassRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.metaclass,
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`,
    /// `set_instance_field` and `set_class_superclass`/`set_class_application`
    /// below. `Interpreter::metaclass_of` is the only caller, and it only
    /// ever calls this on a level it just confirmed is a class object a few
    /// lines above -- a wrong-shape `class` here would mean an uncached
    /// metaclass, which is silent: every later class-side send would rebuild
    /// instead of hitting the cache, and metaclass identity would stop being
    /// stable, which is what inline caches are meant to key on. The caller
    /// turns a `false` into an `unreachable!` rather than swallowing it.
    pub fn set_class_metaclass(&mut self, class: ClassRef, metaclass: ClassRef) -> bool {
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
    /// this one. `tests/instantiate.rs`'s
    /// `superclass_slot_initializer_runs_before_the_subclass_own_slots` is the
    /// only caller: a root class's superclass clause only resolves a fixed
    /// list of builtin names (`resolve_superclass_receiver`), so linking a
    /// subclass to an already-loaded sibling *user-defined* class has to
    /// happen by hand on the class-object chain `super`/`instantiate` walk.
    pub fn set_class_superclass(&mut self, class: ClassRef, superclass: ClassRef) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.superclass = Some(superclass);
            return true;
        }
        false
    }

    /// Answers whether the write happened, mirroring `array_set` and
    /// `set_instance_field`. `image::decode::rebuild_applications` is the
    /// only caller in the runtime, and it only ever calls this on an object
    /// it filled in as a class record two lines above -- a wrong-shape
    /// `class` here would be a decoder bug, not an untrusted-input case, so
    /// that caller turns a `false` into an `unreachable!` rather than a
    /// `DecodeError`. Tests also call it, to swap a doctored mixin onto a
    /// live class (`install_mixin` in `tests/image.rs`).
    pub fn set_class_application(
        &mut self,
        class: ClassRef,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.application = application;
            return true;
        }
        false
    }

    // ---------- Frame accessors ----------
    //
    // A `FrameRef` names a *reified* activation. Its receiver, class and
    // kind are fixed when it is created, so they read the same whether or
    // not the frame is still married to a live stack entry. Its pc and
    // registers are not: while it is married they live on the activation
    // stack, so the accessors that read or write the frame's own copy
    // assert it is dead. `frame_local`/`frame_set_local` (with the
    // activation stack, below) follow the marriage instead.
    //
    // Only the accessors a caller actually needs are added here;
    // `frame_arg_count` and `frame_pc` have no caller yet, so they are
    // not here. Three have only test callers: `set_frame_pc`,
    // `frame_sender` and `frame_is_uncontinuable` read or write fields the
    // image format records per frame, and the image and interpreter tests
    // build and check frame records through them. Nothing in the runtime
    // decides anything by those fields -- whether a frame is live is
    // `frame_activation`, never `frame_is_uncontinuable`.

    /// An opaque, hashable key unique to `frame`, for code that needs a
    /// `FrameRef` as a `HashMap` key rather than a comparison --
    /// `image::encode::GraphTable`'s frame-id map is the only caller. On
    /// `Heap`, and guarded, for the reason `object_identity_key` gives.
    pub(crate) fn frame_identity_key(&self, frame: FrameRef) -> usize {
        self.check_frame_origin(frame);
        frame.index as usize
    }

    /// A reified frame's receiver. Fixed when the frame is created, so it
    /// needs no redirect while the frame is married.
    pub fn frame_receiver(&self, frame: FrameRef) -> Value {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.receiver
    }

    pub fn frame_current_class(&self, frame: FrameRef) -> Option<ClassRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.current_class
    }

    /// Grows a *dead* frame's register file, filling with nil. A married
    /// frame's registers are its live entry's window, so it cannot grow here:
    /// edit-and-continue has to reify and divorce a running activation first
    /// (`docs/design/execution.md`, "How this meets the liveness
    /// requirements"). The top live activation grows in place through
    /// `grow_activation_temps`.
    pub fn frame_grow_temps(&mut self, frame: FrameRef, needed: usize) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's registers live on the activation stack; grow them through its activation"
        );
        let f = &mut self.frames[frame.index as usize].frame;
        if f.temps.len() < needed {
            f.temps.resize(needed, Value::Nil);
        }
    }

    /// Test and format level: the image tests use it to give a dead frame a
    /// synthetic paused `pc` before a snapshot. The runtime stores a running
    /// activation's `pc` on its stack entry (`set_activation_pc`), and the
    /// divorce copies it here.
    pub fn set_frame_pc(&mut self, frame: FrameRef, pc: usize) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's pc and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        self.frames[frame.index as usize].frame.pc = pc;
    }

    pub fn frame_home(&self, frame: FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.home
    }

    /// A reified frame's recorded sender. `None` for every frame the runtime
    /// reifies -- a live activation's sender is the stack entry below it, and
    /// a returned activation has none -- so it can be set only on a frame the
    /// image decoder rebuilt from a record that carries one. Test and format
    /// level: only tests read it.
    pub fn frame_sender(&self, frame: FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.sender
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `locals::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        match &self.frames[frame.index as usize].frame.kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame),
            _ => None,
        }
    }

    /// The frame's recorded `uncontinuable` flag. Test and format level:
    /// true for every frame the divorce left behind, but on a decoded frame
    /// it answers whatever the file said. It is not liveness --
    /// `raise_return` treats every decoded frame as dead, because it
    /// decides by `frame_activation`, and a decoded frame is never married.
    pub fn frame_is_uncontinuable(&self, frame: FrameRef) -> bool {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.uncontinuable
    }

    /// A copy of `frame`'s `kind`. Cloning is cheap -- every variant holds
    /// at most an `Rc` -- and `image::decode` is the only caller that needs
    /// to match on the *whole* kind rather than one variant's payload
    /// (`frame_captured`): once to resolve which of `Initializer`/
    /// `SuperclassClause`/`SuperclassMessage` a frame carries a class for
    /// (`frame_body`), and once to check that a class-carrying kind's class
    /// is itself a class reference (`check_class_references`).
    pub fn frame_kind(&self, frame: FrameRef) -> FrameKind {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].frame.kind.clone()
    }

    /// `pc`, and how many of a frame's registers are `args` versus `temps`,
    /// read together through one slice index rather than three.
    /// `image::decode::verify_graph` checks all three against the shape of
    /// the compiled body it reconciles every decoded frame against.
    pub fn frame_register_shape(&self, frame: FrameRef) -> (usize, usize, usize) {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's pc and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        let f = &self.frames[frame.index as usize].frame;
        (f.pc, f.args.len(), f.temps.len())
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
    pub fn frame_snapshot(&self, frame: FrameRef) -> Frame {
        self.check_frame_origin(frame);
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's pc and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
        self.frames[frame.index as usize].frame.clone()
    }

    /// The frame equivalent of `replace_object`.
    /// `image::decode::decode_graph_section`'s second pass is the only
    /// caller, and it only ever calls this on a handle its first pass
    /// placeholder-allocated and nothing else has observed yet -- before
    /// `verify_graph` runs, which assumes every frame is already filled in.
    /// Call this on any handle something else already holds and it silently
    /// replaces that frame's already-verified `pc`/register-file shape with
    /// an unverified one: no error, and no way for `verify_graph` to
    /// notice, because it has already finished.
    pub fn replace_frame(&mut self, frame: FrameRef, filled: Frame) {
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

    /// Pushes an activation whose registers are `args` followed by
    /// `temp_count` nils.
    pub fn push_activation(
        &mut self,
        header: ActivationHeader,
        args: &[Value],
        temp_count: usize,
    ) -> ActivationRef {
        let base = u32::try_from(self.stack.values.len())
            .expect("more than u32::MAX registers live at once in one heap");
        let num_args =
            u32::try_from(args.len()).expect("more than u32::MAX arguments to one activation");
        let index = u32::try_from(self.stack.entries.len())
            .expect("more than u32::MAX live activations in one heap");
        self.stack.values.extend_from_slice(args);
        let end = self.stack.values.len() + temp_count;
        self.stack.values.resize(end, Value::Nil);
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
            num_args,
            pc: 0,
            slot_base: header.slot_base,
            reified: None,
            #[cfg(debug_assertions)]
            generation,
        });
        self.activation_handle(index)
    }

    /// Pops the top activation. If it was reified, its frame is divorced:
    /// the registers move into the frame, which is dead from then on --
    /// uncontinuable, and with no sender, as a returned Smalltalk context
    /// has none.
    pub fn pop_activation(&mut self, act: ActivationRef) {
        self.check_activation(act);
        debug_assert_eq!(
            act.index as usize + 1,
            self.stack.entries.len(),
            "only the top activation can be popped"
        );
        let entry = self
            .stack
            .entries
            .pop()
            .expect("an activation handle names a live entry");
        let base = entry.base as usize;
        if let Some(frame) = entry.reified {
            let registers = &self.stack.values[base..];
            let split = entry.num_args as usize;
            let slot = &mut self.frames[frame.index as usize];
            slot.frame.args = registers[..split].to_vec();
            slot.frame.temps = registers[split..].to_vec();
            slot.frame.pc = entry.pc;
            slot.frame.sender = None;
            slot.frame.uncontinuable = true;
            slot.live = None;
        }
        self.stack.values.truncate(base);
    }

    /// The arena frame for a live activation, created married to it on the
    /// first call and answered again on every later one.
    pub fn reify(&mut self, act: ActivationRef) -> FrameRef {
        self.check_activation(act);
        let i = act.index as usize;
        if let Some(frame) = self.stack.entries[i].reified {
            return frame;
        }
        let entry = &self.stack.entries[i];
        let frame = Frame {
            receiver: entry.receiver,
            args: Vec::new(),
            temps: Vec::new(),
            pc: entry.pc,
            sender: None,
            home: match &entry.kind {
                FrameKind::Closure(closure) => Some(closure.home),
                _ => None,
            },
            current_class: entry.current_class,
            kind: entry.kind.clone(),
            uncontinuable: false,
            slot_base: entry.slot_base,
        };
        let handle = self.alloc_frame(frame);
        self.frames[handle.index as usize].live = Some(act.index);
        self.stack.entries[i].reified = Some(handle);
        handle
    }

    /// How many activations are live. `Interpreter::send` and
    /// `Interpreter::instantiate` assert that they leave it where they
    /// found it.
    pub fn activation_depth(&self) -> usize {
        self.stack.entries.len()
    }

    pub fn activation_local(&self, act: ActivationRef, index: u16) -> Value {
        self.check_activation(act);
        self.stack.values[self.register_slot(act.index, index)]
    }

    pub fn activation_set_local(&mut self, act: ActivationRef, index: u16, value: Value) {
        self.check_activation(act);
        let slot = self.register_slot(act.index, index);
        self.stack.values[slot] = value;
    }

    pub fn activation_receiver(&self, act: ActivationRef) -> Value {
        self.check_activation(act);
        self.stack.entries[act.index as usize].receiver
    }

    /// The receiver and `current_class` together. `dispatch_lexical_send`'s
    /// head needs both on every implicit-receiver send; fetching them
    /// separately measured 13% slower on `bench_loop.ns`
    /// (`docs/design/execution.md`, "A 13% regression, found and
    /// recovered").
    pub fn activation_receiver_and_current_class(
        &self,
        act: ActivationRef,
    ) -> (Value, Option<ClassRef>) {
        self.check_activation(act);
        let entry = &self.stack.entries[act.index as usize];
        (entry.receiver, entry.current_class)
    }

    pub fn activation_current_class(&self, act: ActivationRef) -> Option<ClassRef> {
        self.check_activation(act);
        self.stack.entries[act.index as usize].current_class
    }

    /// See `Frame::slot_base`.
    pub fn activation_slot_base(&self, act: ActivationRef) -> usize {
        self.check_activation(act);
        self.stack.entries[act.index as usize].slot_base
    }

    pub fn set_activation_pc(&mut self, act: ActivationRef, pc: usize) {
        self.check_activation(act);
        self.stack.entries[act.index as usize].pc = pc;
    }

    /// A closure activation's return target -- its closure's `home` -- and
    /// `None` for every other kind, which is its own return target.
    pub fn activation_home(&self, act: ActivationRef) -> Option<FrameRef> {
        self.check_activation(act);
        match &self.stack.entries[act.index as usize].kind {
            FrameKind::Closure(closure) => Some(closure.home),
            _ => None,
        }
    }

    /// The frame a closure activation's closure captured, or `None` for any
    /// other kind. The first hop of `locals::frame_at_depth`.
    pub fn activation_captured(&self, act: ActivationRef) -> Option<FrameRef> {
        self.check_activation(act);
        match &self.stack.entries[act.index as usize].kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame),
            _ => None,
        }
    }

    /// Grows the top activation's temporaries to `needed`, filling with
    /// nil. The initializer path builds its activation before it knows the
    /// compiled body's register count. Only the top entry can grow: any
    /// other's window ends where the next one's begins.
    pub fn grow_activation_temps(&mut self, act: ActivationRef, needed: usize) {
        self.check_activation(act);
        debug_assert_eq!(
            act.index as usize + 1,
            self.stack.entries.len(),
            "only the top activation's register file can grow"
        );
        let entry = &self.stack.entries[act.index as usize];
        let end = entry.base as usize + entry.num_args as usize + needed;
        if self.stack.values.len() < end {
            self.stack.values.resize(end, Value::Nil);
        }
    }

    /// A register of a reified frame: the live stack slot while its
    /// activation runs, the frame's own copy after it has returned. This is
    /// the path a block takes to its method's variables, and the only
    /// register access that branches.
    pub fn frame_local(&self, frame: FrameRef, index: u16) -> Value {
        self.check_frame_origin(frame);
        let slot = &self.frames[frame.index as usize];
        match slot.live {
            Some(entry) => self.stack.values[self.register_slot(entry, index)],
            None => {
                let f = &slot.frame;
                let i = index as usize;
                if i < f.args.len() {
                    f.args[i]
                } else {
                    f.temps[i - f.args.len()]
                }
            }
        }
    }

    pub fn frame_set_local(&mut self, frame: FrameRef, index: u16, value: Value) {
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
                let f = &mut self.frames[frame.index as usize].frame;
                let i = index as usize;
                if i < f.args.len() {
                    f.args[i] = value;
                } else {
                    let temp_index = i - f.args.len();
                    f.temps[temp_index] = value;
                }
            }
        }
    }

    /// The live activation `frame` is married to, or `None` once that
    /// activation has returned -- and for every frame the image decoder
    /// built, since none of those ever ran here. `raise_return` uses it to
    /// tell a live home from a dead one.
    pub fn frame_activation(&self, frame: FrameRef) -> Option<ActivationRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize]
            .live
            .map(|entry| self.activation_handle(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest activation the arena accepts: enough to get a
    /// `FrameRef` back, with no field any test below reads. Gated like its
    /// only caller -- the origin guard it feeds does not exist in a release
    /// build, so neither does the test, so neither may this.
    #[cfg(debug_assertions)]
    fn bare_frame() -> Frame {
        Frame {
            receiver: Value::Nil,
            args: Vec::new(),
            temps: Vec::new(),
            pc: 0,
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
        let class = origin.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let obj = origin.alloc_object(class, ObjectData::Str("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.alloc_self_referential_class(ObjectData::Instance(vec![]));
        other.alloc_object(other_class, ObjectData::Str("theirs".to_string()));
        other.alloc_object(other_class, ObjectData::Str("theirs".to_string()));

        // In range for `other` -- which is exactly why this is silent
        // without the guard.
        let _ = other.str_value(obj);
    }

    /// The sibling case, and the one an accessor guard cannot reach:
    /// `same` is handed two handles and no `Heap`, so nothing else in the
    /// crate would notice that they come from different arenas. Two
    /// unrelated objects sharing an index would answer "identical" --
    /// silently, and `ObjRef::same` *is* object identity. Pinned here so
    /// the guarantee `CLAUDE.md` states is tested rather than asserted.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let class = origin.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let mine = origin.alloc_object(class, ObjectData::Str("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let theirs = other.alloc_object(other_class, ObjectData::Str("theirs".to_string()));

        // Both are the second object their own heap allocated, so both
        // carry the same index: without the guard this answers `true`.
        let _ = mine.same(theirs);
    }

    /// The same hole for reified frames. `FrameRef::same` is what the image
    /// round-trip tests check sharing by -- a closure's `captured_frame` and
    /// `home` naming one frame or two -- so a false "identical" would let a
    /// round trip that split one frame into two pass. (A non-local return
    /// names its target by `ActivationRef`; see
    /// `comparing_two_activation_handles_from_different_heaps_panics`.)
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_frame_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let mine = origin.alloc_frame(bare_frame());

        let mut other = Heap::new();
        let theirs = other.alloc_frame(bare_frame());

        let _ = mine.same(theirs);
    }

    /// Why `object_identity_key` exists at all, rather than callers keying
    /// on `identity_hash`. Both allocators that fix the hash at 0 --
    /// `alloc_placeholder_object` and `alloc_self_referential_class` -- can
    /// produce several objects in one heap sharing that hash, and a map
    /// keyed on it would fold them into one entry. `image::decode` builds a
    /// whole graph out of exactly those placeholders, so this is the case
    /// that would break, not a hypothetical one.
    ///
    /// The cross-heap case is pinned separately, by
    /// `keying_a_handle_through_a_foreign_heap_panics` below.
    #[test]
    fn identity_keys_separate_objects_that_share_an_identity_hash() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let a = heap.alloc_placeholder_object(class, ObjectData::Instance(Vec::new()));
        let b = heap.alloc_placeholder_object(class, ObjectData::Instance(Vec::new()));

        assert_eq!(heap.identity_hash(a), 0);
        assert_eq!(heap.identity_hash(a), heap.identity_hash(b));
        assert_ne!(heap.object_identity_key(a), heap.object_identity_key(b));
        // Stable for one handle, which is what every caller relies on.
        assert_eq!(heap.object_identity_key(a), heap.object_identity_key(a));
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
        let class = origin.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let mine = origin.alloc_object(class, ObjectData::Str("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let theirs = other.alloc_object(other_class, ObjectData::Str("theirs".to_string()));

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
        assert_eq!(heap.alloc_identity_hash(), 1);
        assert_eq!(heap.alloc_identity_hash(), 2);
        assert_eq!(heap.alloc_identity_hash(), 3);
    }

    #[test]
    fn advancing_past_a_maximum_never_moves_the_counter_backwards() {
        let mut heap = Heap::new();
        heap.advance_identity_hash_past(10);
        assert_eq!(heap.alloc_identity_hash(), 11);
        // A lower maximum must not rewind: an image's ids and freshly
        // allocated ones would then collide.
        heap.advance_identity_hash_past(3);
        assert_eq!(heap.alloc_identity_hash(), 12);
    }

    #[test]
    fn a_self_referential_class_is_its_own_class() {
        let mut heap = Heap::new();
        // A filler object first, so `class` does not land at index 0 -- an
        // implementation that always answered `ObjRef { index: 0 }` would
        // otherwise still pass this test.
        heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        assert!(heap.class_of_obj(class).same(class));
    }

    fn top_level_frame() -> Frame {
        Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
            slot_base: 0,
        }
    }

    /// Like `top_level_frame`, but with a distinct value in each of `pc`,
    /// `args.len()` and `temps.len()`. `frame_register_shape` returns those
    /// three as a same-typed tuple, so a frame built from all-zero or
    /// otherwise equal fields cannot catch the tuple's positions being
    /// silently transposed -- exactly the silent-wrong-output risk this
    /// conversion carries.
    fn shaped_frame(pc: usize, arg_count: usize, temp_count: usize) -> Frame {
        Frame {
            args: vec![Value::Nil; arg_count],
            temps: vec![Value::Nil; temp_count],
            pc,
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
        let first = heap.alloc_frame(shaped_frame(3, 2, 1));
        for _ in 0..1000 {
            heap.alloc_frame(top_level_frame());
        }
        assert!(matches!(heap.frame_receiver(first), Value::Nil));
        assert_eq!(heap.frame_register_shape(first), (3, 2, 1));
    }

    /// Frame identity is what the image round trip checks sharing by -- a
    /// closure's `captured_frame` and `home` naming one frame or two -- so
    /// two reified frames may never compare equal however alike their
    /// contents are. (`Signal::Return` names its target by `ActivationRef`,
    /// not by frame.)
    #[test]
    fn two_frames_are_never_the_same_activation() {
        let mut heap = Heap::new();
        let a = heap.alloc_frame(top_level_frame());
        let b = heap.alloc_frame(top_level_frame());
        assert!(!a.same(b));
        assert!(a.same(a));
    }

    /// The object counterpart of the frame test above: a handle must stay
    /// valid across the reallocation that growing the arena performs, and
    /// two objects with identical contents must never be the same object.
    #[test]
    fn an_object_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        // A filler object first, so neither `class` nor `first` lands at
        // index 0 -- see `a_self_referential_class_is_its_own_class`.
        heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.alloc_object(class, ObjectData::Str("first".to_string()));
        for _ in 0..1000 {
            heap.alloc_object(class, ObjectData::Str("filler".to_string()));
        }
        assert_eq!(heap.str_value(first).as_deref(), Some("first"));
        assert!(heap.class_of_obj(first).same(class));

        let twin = heap.alloc_object(class, ObjectData::Str("first".to_string()));
        assert!(!first.same(twin));
    }

    /// A minimal class object real enough for `describe` to read a mixin
    /// name off it. Mirrors `types.rs`'s own `dummy_class` test helper,
    /// duplicated rather than shared because that one is private to
    /// `types.rs`'s test module.
    fn named_class(heap: &mut Heap, name: &str) -> ClassRef {
        let application = std::rc::Rc::new(crate::mixin::MixinApplication {
            mixin: std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name))),
            superclass: None,
            slot_index: std::collections::HashMap::new(),
            nested_slot: std::collections::HashMap::new(),
            num_slots: 0,
            own_slot_base: 0,
        });
        heap.alloc_self_referential_class(ObjectData::Class(ClassData {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
            metaclass: None,
        }))
    }

    /// `describe`'s `ObjectData::Instance` arm is the one non-mechanical
    /// rewrite the arena conversion made: a nested read through a re-bound
    /// `class` intermediate rather than a single chained borrow. Nothing
    /// else in the workspace asserts its output, so pin it directly.
    #[test]
    fn describe_names_an_instance_by_its_class_mixin_name() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let instance = heap.alloc_object(class, ObjectData::Instance(vec![]));
        assert_eq!(heap.describe(&Value::Obj(instance)), "an instance of Foo");
    }

    /// The fallback arm of that same match: `class` names an `ObjRef` whose
    /// own `data` is not `ObjectData::Class`, the shape a class reference
    /// decoded from an untrusted image can be in before verification runs.
    /// `describe` must still answer something rather than panic.
    #[test]
    fn describe_falls_back_to_an_object_for_a_class_reference_that_is_not_a_class() {
        let mut heap = Heap::new();
        let class = named_class(&mut heap, "Foo");
        let not_a_class = heap.alloc_object(class, ObjectData::Str("not a class".to_string()));
        let instance = heap.alloc_object(not_a_class, ObjectData::Instance(vec![]));
        assert_eq!(heap.describe(&Value::Obj(instance)), "an object");
    }

    /// The header every activation-stack test below pushes: nothing in it is
    /// read back except where a test sets it on purpose.
    fn header() -> ActivationHeader {
        ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::TopLevel,
            slot_base: 0,
        }
    }

    #[test]
    fn an_activation_addresses_its_arguments_before_its_temporaries() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1), Value::SmallInt(2)], 2);
        assert!(matches!(heap.activation_local(act, 0), Value::SmallInt(1)));
        assert!(matches!(heap.activation_local(act, 1), Value::SmallInt(2)));
        assert!(matches!(heap.activation_local(act, 2), Value::Nil));
        heap.activation_set_local(act, 3, Value::SmallInt(9));
        assert!(matches!(heap.activation_local(act, 3), Value::SmallInt(9)));
        heap.pop_activation(act);
    }

    /// Two live activations share one `values` vector; neither may see the
    /// other's registers, whichever is written first.
    #[test]
    fn nested_activations_keep_separate_register_windows() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        let inner = heap.push_activation(header(), &[Value::SmallInt(2)], 1);
        heap.activation_set_local(inner, 1, Value::SmallInt(20));
        heap.activation_set_local(outer, 1, Value::SmallInt(10));
        assert!(matches!(
            heap.activation_local(outer, 0),
            Value::SmallInt(1)
        ));
        assert!(matches!(
            heap.activation_local(outer, 1),
            Value::SmallInt(10)
        ));
        assert!(matches!(
            heap.activation_local(inner, 0),
            Value::SmallInt(2)
        ));
        assert!(matches!(
            heap.activation_local(inner, 1),
            Value::SmallInt(20)
        ));
        assert_eq!(heap.activation_depth(), 2);
        heap.pop_activation(inner);
        heap.pop_activation(outer);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// The property the activation stack exists for: an activation nothing
    /// reified is gone once it is popped, rather than kept in the frame
    /// arena, which never reclaims a slot.
    #[test]
    fn an_activation_that_is_never_reified_leaves_nothing_behind() {
        let mut heap = Heap::new();
        for _ in 0..1000 {
            let act = heap.push_activation(header(), &[Value::SmallInt(1)], 3);
            heap.pop_activation(act);
        }
        assert_eq!(heap.frames.len(), 0);
        assert_eq!(heap.stack.values.len(), 0);
        assert_eq!(heap.activation_depth(), 0);
    }

    #[test]
    fn reifying_an_activation_twice_answers_the_same_frame() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[], 1);
        let first = heap.reify(act);
        let second = heap.reify(act);
        assert!(first.same(second));
        assert_eq!(heap.frames.len(), 1);
        heap.pop_activation(act);
    }

    /// While its activation runs, a reified frame is a view of the live
    /// registers, in both directions: a block writing its method's variable
    /// through the frame must be seen by the method, and vice versa.
    #[test]
    fn a_married_frame_reads_and_writes_the_live_registers() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        let frame = heap.reify(act);
        heap.activation_set_local(act, 1, Value::SmallInt(5));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInt(5)));
        heap.frame_set_local(frame, 1, Value::SmallInt(7));
        assert!(matches!(heap.activation_local(act, 1), Value::SmallInt(7)));
        assert!(heap
            .frame_activation(frame)
            .is_some_and(|live| live.same(act)));
        heap.pop_activation(act);
    }

    /// Popping a reified activation hands its registers to the frame, which
    /// a surviving block goes on reading and writing; the frame is dead from
    /// then on, and a dead activation has no sender.
    #[test]
    fn popping_a_reified_activation_divorces_its_frame() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        heap.set_activation_pc(act, 4);
        let frame = heap.reify(act);
        // Written after reification: it must survive the divorce.
        heap.activation_set_local(act, 1, Value::SmallInt(3));
        heap.pop_activation(act);

        assert!(heap.frame_activation(frame).is_none());
        assert!(matches!(heap.frame_local(frame, 0), Value::SmallInt(1)));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInt(3)));
        assert_eq!(heap.frame_register_shape(frame), (4, 1, 1));
        assert!(heap.frame_is_uncontinuable(frame));
        assert!(heap.frame_sender(frame).is_none());
        heap.frame_set_local(frame, 1, Value::SmallInt(8));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInt(8)));
    }

    /// The next activation at the same depth reuses the stack slots. A
    /// divorced frame owns its own copy, so that reuse cannot reach it.
    #[test]
    fn a_divorced_frame_is_untouched_by_the_next_activation_at_its_depth() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(first, 1, Value::SmallInt(3));
        let frame = heap.reify(first);
        heap.pop_activation(first);

        let second = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(second, 1, Value::SmallInt(99));
        assert!(matches!(heap.frame_local(frame, 1), Value::SmallInt(3)));
        heap.pop_activation(second);
    }

    /// An `ActivationRef` kept past its pop names whatever activation reuses
    /// the index next -- a different, valid activation, so the read would
    /// silently answer the wrong thing. Debug builds refuse it.
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

    /// `ActivationRef::same` is how `invoke_method` and `run_clause_body`
    /// decide that a `Signal::Return` is theirs, and it is handed two handles
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

    /// Every test above pushes `FrameKind::TopLevel`, which is `None` on
    /// both `activation_home` and `activation_captured` by construction --
    /// closure creation is the only reason either exists. `raise_return`
    /// picks a non-local return's target off `activation_home`, and the
    /// outer-local walk (`locals::frame_at_depth`) starts every hop off
    /// `activation_captured`/`frame_captured`, so a closure activation that
    /// answered `None` for either would silently misroute a `^` or a
    /// captured-variable read rather than fail loudly. This also exercises
    /// `alloc_frame`'s debug assertion that `Frame.home` is populated
    /// exactly for a `FrameKind::Closure` activation, since `reify` builds
    /// that frame from a closure entry's `kind` here for the first time in
    /// this test module.
    #[test]
    fn a_closure_activations_home_and_captured_frame_are_its_closures() {
        let mut heap = Heap::new();
        let method_act = heap.push_activation(header(), &[], 0);
        let home_frame = heap.reify(method_act);

        let closure_data = ClosureData {
            params: std::rc::Rc::new(vec![]),
            code: std::rc::Rc::new(crate::bytecode::CompiledCode {
                kind: crate::bytecode::CodeKind::Block,
                instrs: vec![crate::bytecode::Instr::ImplicitReturn { reg: 0 }],
                num_args: 0,
                num_registers: 1,
                statement_pc: vec![],
                call_sites: vec![],
                source: ns_syntax::Span::new(0, 0),
            }),
            captured_frame: home_frame,
            home: home_frame,
        };
        let closure_act = heap.push_activation(
            ActivationHeader {
                receiver: Value::Nil,
                current_class: None,
                kind: FrameKind::Closure(std::rc::Rc::new(closure_data)),
                slot_base: 0,
            },
            &[],
            0,
        );

        assert!(heap.activation_home(method_act).is_none());
        assert!(heap.activation_captured(method_act).is_none());
        assert!(heap
            .activation_home(closure_act)
            .is_some_and(|h| h.same(home_frame)));
        assert!(heap
            .activation_captured(closure_act)
            .is_some_and(|c| c.same(home_frame)));

        let closure_frame = heap.reify(closure_act);
        assert!(heap
            .frame_home(closure_frame)
            .is_some_and(|h| h.same(home_frame)));

        heap.pop_activation(closure_act);
        heap.pop_activation(method_act);
    }
}
