//! The heap seam. Every read and write of an `Object` or a `Frame` goes
//! through `Heap`, so that replacing what sits behind a handle changes this
//! file rather than its call sites. The store behind the accessors is an
//! arena of typed handles for both; see `docs/design/memory.md`.
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/eval.rs`.

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
    pub fn same(&self, other: &ObjRef) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
        );
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `ObjRef` as a `HashMap`/`HashSet` key rather than a comparison --
    /// `identity_hash` cannot serve that role, because two placeholder
    /// objects may legitimately share hash 0
    /// (`Heap::alloc_placeholder_object`,
    /// `Heap::alloc_self_referential_class`), which would fold two distinct
    /// objects into one table entry. Crate-internal: every current caller
    /// (`image::encode::GraphTable`'s object-id map,
    /// `image::decode::rebuild_applications`'s superclass-position map,
    /// `interpreter::nested::in_progress_key`) lives inside `ns-runtime`.
    ///
    /// This and `FrameRef::identity_key` are the only ways a handle's index
    /// leaves this module, so anything that changes what a handle encodes --
    /// widening it to carry a heap tag, say -- has to check these callers
    /// even though the field itself is private. They all treat the result as
    /// an opaque key, which is what keeps that a check rather than a rewrite.
    ///
    /// **Unguarded, unlike `same` and every accessor.** Two handles from
    /// different heaps produce the same key here and collide in a caller's
    /// map. It is not guarded because this method is handed one handle and
    /// no `Heap`, so there is nothing to check against -- and folding the
    /// `origin` field into the key instead would be worse than leaving it:
    /// that field exists only in a debug build, so the collision would
    /// become impossible in the build that would have reported it and stay
    /// exactly as possible in the build that ships. The fix is to move these
    /// onto `Heap`, where the existing origin check applies; see
    /// `docs/PROGRESS.md`'s Notes for why that is a follow-up rather than
    /// done here.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
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
/// enforced by the type system in Milestone 1 (no metaclasses yet).
pub type ClassRef = ObjRef;

/// An opaque handle to an activation: an index into `Heap.frames`. The
/// inner index is private to this module, and only `heap.rs` reaches it --
/// that privacy is what made replacing `Rc<RefCell<Frame>>` with this a
/// change to one file rather than to every caller. See
/// `docs/design/memory.md`, "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two live
/// activations never share an index, which is what `Signal::Return`'s
/// target check depends on.
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
    /// Activation identity. Replaces `Rc::ptr_eq` at every call site, so
    /// that identity has one spelling that survives the store changing
    /// under it.
    ///
    /// Its two call sites both ask the same question -- "is this returning
    /// signal aimed at *my* activation?" -- and both answer it by catching
    /// `Signal::Return(target, _)` when `target.same(&frame)`:
    /// `Interpreter::invoke_method` (`interpreter/mod.rs`) for a method or
    /// initializer body, and `interpreter::nested`'s superclass-clause
    /// activation (`nested.rs`). `raise_return` picks the target rather than
    /// comparing one, through `frame_home` and `frame_is_uncontinuable`, so
    /// it is not a caller.
    ///
    /// Guarded like an accessor: see `ObjRef::same`. A false *yes* at either
    /// site swallows a non-local return meant for an outer activation, which
    /// is a wrong answer that looks like ordinary control flow.
    pub fn same(&self, other: &FrameRef) -> bool {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.origin, other.origin,
            "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
            self.origin, other.origin
        );
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `FrameRef` as a `HashMap` key rather than a comparison --
    /// `image::encode::GraphTable`'s frame-id map. Crate-internal: its
    /// only current caller lives inside `ns-runtime`.
    ///
    /// Unguarded, for the reason `ObjRef::identity_key` gives.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` walking the sender chain (a closure's
/// `home` can itself be captured, so that walk had no acyclic guarantee
/// to lean on); an index has nothing to descend into at all, so the
/// hazard is gone by construction rather than by discipline. It also can
/// no longer panic, which is why the `try_borrow` dance is gone.
impl std::fmt::Debug for FrameRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameRef(#{})", self.index)
    }
}

pub struct Heap {
    /// Monotonic, assigned at allocation, never derived from an address, so
    /// identity survives both a moving collector and a snapshot
    /// (`docs/design/memory.md`).
    next_identity_hash: u32,
    /// Every activation, addressed by a `FrameRef`'s index. Nothing is ever
    /// removed: reclaiming a slot needs a collector, and there is none. Until
    /// then an activation is retained for the process's lifetime -- the
    /// interim cost the milestone's ordering accepts, and the one the
    /// activation stack removes by construction, since a stack entry is
    /// popped. Both steps are enumerated in `docs/PROGRESS.md`, "Where the
    /// next session starts"; what the retention measured is in
    /// `docs/design/memory.md`, "What the collector owns".
    frames: Vec<Frame>,
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
/// allocated, and a `ClosureBody::Ast(Rc<CodeBody>)` reachable through
/// either one would print a whole AST -- the same hazard `CLAUDE.md`'s
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
    /// `compile_initializer`, `compile_module` and `cmd_disasm` take a real
    /// `&Heap` parameter instead of building one on the spot, and why
    /// running the test suite in debug is what catches the mistake if it
    /// recurs.
    pub fn new() -> Heap {
        Heap {
            next_identity_hash: 1,
            frames: Vec::new(),
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
    /// workspace -- src or tests -- so that replacing what backs a handle
    /// only ever touches this file.
    ///
    /// `home_of` and `raise_return` read a return target off `Frame.home`
    /// rather than matching on `Frame.kind`, which is correct only while the
    /// two agree: `home` populated exactly for a `FrameKind::Closure`
    /// activation. Every real constructor upholds that by construction, but
    /// nothing in the type system ties the two fields together, so a new
    /// constructor added later -- or a mismatched pair of literal fields at
    /// an existing one -- would corrupt `^`'s return target silently rather
    /// than fail loudly. Checked here, once, rather than at every
    /// constructor. Not checked in `replace_frame`: the decoder fills a
    /// placeholder through that path with fields read off the wire, and
    /// nothing reactivates a decoded frame, so it need not satisfy this.
    pub fn alloc_frame(&mut self, frame: Frame) -> FrameRef {
        debug_assert_eq!(
            frame.home.is_some(),
            matches!(frame.kind, FrameKind::Closure(_)),
            "Frame.home must be populated exactly for a closure activation"
        );
        let index = u32::try_from(self.frames.len())
            .expect("more than u32::MAX activations allocated in one heap");
        self.frames.push(frame);
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
    fn check_object_origin(&self, obj: &ObjRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            obj.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            obj.origin, self.heap_id
        );
        let _ = obj;
    }

    #[inline]
    fn check_frame_origin(&self, frame: &FrameRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            frame.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            frame.origin, self.heap_id
        );
        let _ = frame;
    }

    // ---------- Object accessors ----------
    //
    // `&self`/`&mut self` index into `self.objects`, so the borrow checker
    // ties every accessor's result to the `Heap` it read from -- the same
    // discipline `docs/design/memory.md` describes for the frame arena.

    pub fn identity_hash(&self, obj: &ObjRef) -> u32 {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].identity_hash
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
    pub fn set_identity_hash(&mut self, obj: &ObjRef, hash: u32) {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].identity_hash = hash;
    }

    pub fn class_of_obj(&self, obj: &ObjRef) -> ClassRef {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].class
    }

    pub fn is_class(&self, obj: &ObjRef) -> bool {
        self.check_object_origin(obj);
        matches!(self.objects[obj.index as usize].data, ObjectData::Class(_))
    }

    pub fn array_len(&self, obj: &ObjRef) -> Option<usize> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => Some(items.len()),
            _ => None,
        }
    }

    /// The number of fields in `obj`'s `ObjectData::Instance`, mirroring
    /// `array_len`. `platform::dispatch` is the only caller: Milestone 1
    /// tells the platform singleton, its stream and its `collections`
    /// namespace apart by field count rather than a builtin variant of
    /// their own (see `platform.rs`'s module doc), so the dispatcher needs
    /// the count itself, not any one field's value.
    pub fn instance_field_count(&self, obj: &ObjRef) -> Option<usize> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Instance(fields) => Some(fields.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, obj: &ObjRef) -> Option<Vec<Value>> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Array(items) => items.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        self.check_object_origin(obj);
        if let ObjectData::Array(items) = &mut self.objects[obj.index as usize].data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, obj: &ObjRef, value: Value) -> bool {
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
    pub fn instance_field(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Instance(fields) => fields.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`.
    pub fn set_instance_field(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        self.check_object_origin(obj);
        if let ObjectData::Instance(fields) = &mut self.objects[obj.index as usize].data {
            if index < fields.len() {
                fields[index] = value;
                return true;
            }
        }
        false
    }

    pub fn str_value(&self, obj: &ObjRef) -> Option<String> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn sym_value(&self, obj: &ObjRef) -> Option<Symbol> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Sym(s) => Some(*s),
            _ => None,
        }
    }

    pub fn closure_data(&self, obj: &ObjRef) -> Option<ClosureData> {
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
    pub fn object_snapshot(&self, obj: &ObjRef) -> Object {
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
    pub fn replace_object(&mut self, obj: &ObjRef, filled: Object) {
        self.check_object_origin(obj);
        self.objects[obj.index as usize] = filled;
    }

    /// Renders a value for a user-facing error message.
    ///
    /// Never `Debug`-format a `Value` instead: `ObjRef`'s hand-written
    /// `Debug` no longer recurses, but it prints Rust internals such as
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
                self.check_object_origin(obj);
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
        class: &ClassRef,
    ) -> Option<std::rc::Rc<crate::mixin::MixinApplication>> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    pub fn class_enclosing(&self, class: &ClassRef) -> Option<ObjRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.enclosing,
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: &ClassRef) -> Option<ClassRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.declaring_class,
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: &ClassRef) -> Option<ClassRef> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.superclass,
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: &ClassRef) -> Option<BuiltinClass> {
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
        class: &ClassRef,
    ) -> Option<(Option<BuiltinClass>, Option<ClassRef>)> {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => Some((data.builtin, data.superclass)),
            _ => None,
        }
    }

    pub fn class_metaclass(&self, class: &ClassRef) -> Option<ClassRef> {
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
    pub fn set_class_metaclass(&mut self, class: &ClassRef, metaclass: ClassRef) -> bool {
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
    pub fn set_class_superclass(&mut self, class: &ClassRef, superclass: ClassRef) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.superclass = Some(superclass);
            return true;
        }
        false
    }

    /// Answers whether the write happened, mirroring `array_set` and
    /// `set_instance_field`. `image::decode::rebuild_applications` is the
    /// only caller, and it only ever calls this on an object it filled in
    /// as a class record two lines above -- a wrong-shape `class` here would
    /// be a decoder bug, not an untrusted-input case, so the caller turns a
    /// `false` into an `unreachable!` rather than a `DecodeError`.
    pub fn set_class_application(
        &mut self,
        class: &ClassRef,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        self.check_object_origin(class);
        if let ObjectData::Class(data) = &mut self.objects[class.index as usize].data {
            data.application = application;
            return true;
        }
        false
    }

    // ---------- Activation accessors ----------
    //
    // `local`/`set_local` carry the register-file addressing that
    // `eval::read_local` used to own: arguments occupy the first
    // `args.len()` indices and declared temporaries follow, and the
    // boundary comes from how many arguments the activation was *given*.
    // A wrong-arity activation is refused in `invoke_method` before it can
    // reach here, so this addressing is total.
    //
    // Only the accessors a caller actually needs are added here;
    // `frame_arg_count` and `frame_pc` still have no caller and are left
    // for the task that adds their first one.

    pub fn receiver(&self, frame: &FrameRef) -> Value {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].receiver
    }

    /// The receiver and `current_class` together, in one borrow.
    /// `dispatch_lexical_send`'s head needs both on every implicit-receiver
    /// send -- five times per `bench_loop.ns` iteration -- and fetching them
    /// through `receiver` then `frame_current_class` took two separate
    /// borrows where one already carries both fields; that doubling was the
    /// whole of a measured 13% regression (see `docs/design/execution.md`'s
    /// measurements section).
    ///
    /// Under the arena the two reads are two slice indexes rather than two
    /// `RefCell` borrows, so the cost this batching removed is smaller than
    /// it was. It is kept because this is still the hottest call count in
    /// the workspace, and because the batched shape is what the activation
    /// stack's register accessors want anyway (`docs/PROGRESS.md`, "Where
    /// the next session starts", enumerates that step; it is designed in
    /// `docs/design/execution.md`); whether it still pays is a measurement,
    /// not an assumption.
    pub fn receiver_and_current_class(&self, frame: &FrameRef) -> (Value, Option<ClassRef>) {
        self.check_frame_origin(frame);
        let f = &self.frames[frame.index as usize];
        (f.receiver, f.current_class)
    }

    pub fn local(&self, frame: &FrameRef, index: u16) -> Value {
        self.check_frame_origin(frame);
        let f = &self.frames[frame.index as usize];
        let i = index as usize;
        if i < f.args.len() {
            f.args[i]
        } else {
            f.temps[i - f.args.len()]
        }
    }

    pub fn set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        self.check_frame_origin(frame);
        let f = &mut self.frames[frame.index as usize];
        let i = index as usize;
        if i < f.args.len() {
            f.args[i] = value;
        } else {
            let temp_index = i - f.args.len();
            f.temps[temp_index] = value;
        }
    }

    pub fn frame_current_class(&self, frame: &FrameRef) -> Option<ClassRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].current_class
    }

    /// The absolute field index a compiled `InitSlot`'s mixin-relative
    /// `own_index` is offset by. Fixed for the frame's whole life --
    /// `interpreter::instantiate` builds one frame per inheritance level and
    /// sets this from that level's own `MixinApplication.own_slot_base`, so
    /// there is nothing to save and restore between levels.
    pub fn frame_slot_base(&self, frame: &FrameRef) -> usize {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].slot_base
    }

    /// Grows an activation's register file, filling with nil. Needed by
    /// the initializer path, where a level's compiled body may declare
    /// more registers than the frame was built with, and by
    /// edit-and-continue later (`docs/design/liveness-and-reflection.md`).
    pub fn grow_temps(&mut self, frame: &FrameRef, needed: usize) {
        self.check_frame_origin(frame);
        let f = &mut self.frames[frame.index as usize];
        if f.temps.len() < needed {
            f.temps.resize(needed, Value::Nil);
        }
    }

    pub fn set_frame_pc(&mut self, frame: &FrameRef, pc: usize) {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].pc = pc;
    }

    pub fn frame_home(&self, frame: &FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].home
    }

    /// The activation that invoked `frame`, `None` only for `FrameKind::TopLevel`.
    /// `tests/builtins_collections_block.rs`'s sender-threading regression test is
    /// the first caller: it walks a captured closure's frame back to the method
    /// activation that invoked it, to check `sender` was actually recorded rather
    /// than left `None` (the Final-review finding I6 defect this guards against).
    pub fn frame_sender(&self, frame: &FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].sender
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `eval::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: &FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        match &self.frames[frame.index as usize].kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame),
            _ => None,
        }
    }

    pub fn frame_is_uncontinuable(&self, frame: &FrameRef) -> bool {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].uncontinuable
    }

    pub fn mark_uncontinuable(&mut self, frame: &FrameRef) {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].uncontinuable = true;
    }

    /// Whether `frame`'s kind is `FrameKind::Method` --
    /// `Interpreter::run_statements` needs this to decide what an implicit
    /// (non-`^`) fall-off-the-end return value is: `self` for a method body
    /// (spec §3.5), the last statement's value for every other body this
    /// loop runs.
    pub fn frame_is_method(&self, frame: &FrameRef) -> bool {
        self.check_frame_origin(frame);
        matches!(self.frames[frame.index as usize].kind, FrameKind::Method(_))
    }

    /// A copy of `frame`'s `kind`. Cloning is cheap -- every variant holds
    /// at most an `Rc` -- and `image::decode` is the only caller that needs
    /// to match on the *whole* kind rather than one variant's payload
    /// (`frame_captured`, `frame_is_method`): once to resolve which of
    /// `Initializer`/`SuperclassClause`/`SuperclassMessage` a frame carries
    /// a class for (`frame_body`), and once to check that a class-carrying
    /// kind's class is itself a class reference (`check_class_references`).
    pub fn frame_kind(&self, frame: &FrameRef) -> FrameKind {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].kind.clone()
    }

    /// `pc`, and how many of a frame's registers are `args` versus `temps`,
    /// read together through one slice index rather than three.
    /// `image::decode::verify_graph` checks all three against the shape of
    /// the compiled body it reconciles every decoded frame against.
    pub fn frame_register_shape(&self, frame: &FrameRef) -> (usize, usize, usize) {
        self.check_frame_origin(frame);
        let f = &self.frames[frame.index as usize];
        (f.pc, f.args.len(), f.temps.len())
    }

    /// The whole of `frame`'s contents, read through one slice index
    /// rather than several. `image::encode::encode_frame` writes a whole
    /// frame record and `GraphTable::visit_frame` walks a whole frame's
    /// references; both need every field. Not for a hot path, for the same
    /// reason as `object_snapshot`.
    pub fn frame_snapshot(&self, frame: &FrameRef) -> Frame {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize].clone()
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
    pub fn replace_frame(&mut self, frame: &FrameRef, filled: Frame) {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize] = filled;
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
            pc_kind: PcKind::Bytecode,
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
        let _ = other.str_value(&obj);
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
        let _ = mine.same(&theirs);
    }

    /// The same hole on the activation side. `FrameRef::same` decides where
    /// a non-local return lands (`invoke_method`'s catch, `raise_return`'s
    /// target check), so a false "identical" would resume the wrong
    /// activation rather than merely report the wrong answer.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn comparing_two_frame_handles_from_different_heaps_panics() {
        let mut origin = Heap::new();
        let mine = origin.alloc_frame(bare_frame());

        let mut other = Heap::new();
        let theirs = other.alloc_frame(bare_frame());

        let _ = mine.same(&theirs);
    }

    /// Why `identity_key` exists at all, rather than callers keying on
    /// `identity_hash`. Both allocators that fix the hash at 0 --
    /// `alloc_placeholder_object` and `alloc_self_referential_class` -- can
    /// produce several objects in one heap sharing that hash, and a map
    /// keyed on it would fold them into one entry. `image::decode` builds a
    /// whole graph out of exactly those placeholders, so this is the case
    /// that would break, not a hypothetical one.
    ///
    /// The cross-heap case is deliberately *not* pinned here: two handles
    /// from different heaps do collide, `identity_key` does not guard
    /// against it, and `docs/PROGRESS.md`'s Notes record the follow-up that
    /// fixes it properly.
    #[test]
    fn identity_keys_separate_objects_that_share_an_identity_hash() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let a = heap.alloc_placeholder_object(class, ObjectData::Instance(Vec::new()));
        let b = heap.alloc_placeholder_object(class, ObjectData::Instance(Vec::new()));

        assert_eq!(heap.identity_hash(&a), 0);
        assert_eq!(heap.identity_hash(&a), heap.identity_hash(&b));
        assert_ne!(a.identity_key(), b.identity_key());
        // Stable for one handle, which is what every caller relies on.
        assert_eq!(a.identity_key(), a.identity_key());
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
        assert!(heap.class_of_obj(&class).same(&class));
    }

    fn top_level_frame() -> Frame {
        Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
            pc_kind: PcKind::TreeWalk,
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
        assert!(matches!(heap.receiver(&first), Value::Nil));
        assert_eq!(heap.frame_register_shape(&first), (3, 2, 1));
    }

    /// Activation identity is what `Signal::Return`'s target check and
    /// `invoke_method`'s catch both rest on, so two live activations may
    /// never compare equal however alike their contents are.
    #[test]
    fn two_frames_are_never_the_same_activation() {
        let mut heap = Heap::new();
        let a = heap.alloc_frame(top_level_frame());
        let b = heap.alloc_frame(top_level_frame());
        assert!(!a.same(&b));
        assert!(a.same(&a.clone()));
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
        assert_eq!(heap.str_value(&first).as_deref(), Some("first"));
        assert!(heap.class_of_obj(&first).same(&class));

        let twin = heap.alloc_object(class, ObjectData::Str("first".to_string()));
        assert!(!first.same(&twin));
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
}
