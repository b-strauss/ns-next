//! The heap seam. Every read and write of an `Object` or a `Frame` goes
//! through `Heap`, so that replacing what sits behind a handle changes this
//! file rather than its call sites. The store behind the accessors is
//! `Rc<RefCell<_>>` today and an arena of typed handles next; see
//! `docs/design/memory.md`.
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/eval.rs`.

use crate::types::*;
use crate::Symbol;

#[derive(Debug)]
pub struct Heap {
    /// Monotonic, assigned at allocation, never derived from an address, so
    /// identity survives both a moving collector and a snapshot
    /// (`docs/design/memory.md`).
    next_identity_hash: u32,
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
    /// -- is inert only while `Heap` holds nothing but a counter. Under the
    /// planned arena a fresh `Heap` is an *empty* arena: a lookup through it
    /// finds nothing and answers `None`/`false` rather than failing loudly,
    /// because every accessor here is already shaped to report absence
    /// rather than panic (see the class-accessor and `array_set`-family
    /// comments above). This exact mistake was made three times while
    /// building the seam -- `compile_initializer`, `compile_module` and
    /// `cmd_disasm` each read a class object through a throwaway `Heap::new()`
    /// -- see `docs/design/compiler.md`'s "Follow-up: entry points that
    /// reach a class object only to find its mixin" for the fix each one
    /// still needs.
    pub fn new() -> Heap {
        Heap {
            next_identity_hash: 1,
        }
    }

    /// Saturating, not wrapping and not panicking: identity hashes are not
    /// required to be unique (`dispatch.rs`'s placeholder class objects
    /// already share hash 0, and `hash` is the only thing that reads one --
    /// object identity is `Rc` identity, never the hash), so a world that
    /// exhausts the counter is better served by handing out a shared
    /// `u32::MAX` than by aborting the process. Reachable in *one*
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
    // Every ordinary construction of an `ObjRef` in the workspace -- src or
    // tests -- goes through one of the two methods below, so that replacing
    // what backs a handle only ever touches this file. The unsafe
    // self-referential bootstrap below them is the one exception the type
    // system cannot avoid, kept here for the same reason.

    /// Allocates `data` under `class`, with a fresh identity hash drawn from
    /// the counter. The ordinary constructor -- everything that is a real,
    /// observable object goes through this one.
    pub fn alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        new_object_handle(class, identity_hash, data)
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
        new_object_handle(class, 0, data)
    }

    /// Allocates a class object that is genuinely its own `.class` — the shape
    /// every one of the crate's four bootstraps needs
    /// (`Interpreter::bootstrap`'s `build_self_referential_object_class` in
    /// production; the `dummy_class` helpers in `dispatch.rs`, `types.rs`
    /// and `tests/interpreter.rs`, each satisfying `Object.class`'s
    /// non-optional type without wiring up a whole hierarchy).
    ///
    /// Takes `&mut self` although today's body ignores it, for the same
    /// reason every accessor here already does: under the arena the object
    /// has to be allocated *into* a heap, and a caller that reached this
    /// through a throwaway `Heap` would silently read a different heap's
    /// slot afterwards.
    ///
    /// Identity hash fixed at 0, like `alloc_placeholder_object`: none of
    /// the four callers links this object into a real hierarchy a hash
    /// could distinguish it within.
    pub fn alloc_self_referential_class(&mut self, data: ObjectData) -> ObjRef {
        bootstrap_self_referential_object(data)
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
        FrameRef(std::rc::Rc::new(std::cell::RefCell::new(frame)))
    }

    // ---------- Object accessors ----------
    //
    // `&self` is unused while a handle is an `Rc`, because the data hangs
    // off the handle itself. It is in these signatures from the start
    // because the arena will need it, and adding it afterwards would touch
    // every call site this module exists to protect. The mutating
    // accessors take `&mut self` for the same reason, even though the
    // `RefCell` behind the handle would make `&self` sufficient today.

    pub fn identity_hash(&self, obj: &ObjRef) -> u32 {
        obj.0.borrow().identity_hash
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
        obj.0.borrow_mut().identity_hash = hash;
    }

    pub fn class_of_obj(&self, obj: &ObjRef) -> ClassRef {
        obj.0.borrow().class.clone()
    }

    pub fn is_class(&self, obj: &ObjRef) -> bool {
        matches!(obj.0.borrow().data, ObjectData::Class(_))
    }

    pub fn array_len(&self, obj: &ObjRef) -> Option<usize> {
        match &obj.0.borrow().data {
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
        match &obj.0.borrow().data {
            ObjectData::Instance(fields) => Some(fields.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, obj: &ObjRef) -> Option<Vec<Value>> {
        match &obj.0.borrow().data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.0.borrow().data {
            ObjectData::Array(items) => items.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.0.borrow_mut().data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, obj: &ObjRef, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.0.borrow_mut().data {
            items.push(value);
            return true;
        }
        false
    }

    /// The instance field at `index`, or `None` when `obj`'s payload is not
    /// `ObjectData::Instance` or `index` is out of range -- the same
    /// total-rather-than-panicking shape as the array accessors above.
    pub fn instance_field(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.0.borrow().data {
            ObjectData::Instance(fields) => fields.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`.
    pub fn set_instance_field(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        if let ObjectData::Instance(fields) = &mut obj.0.borrow_mut().data {
            if index < fields.len() {
                fields[index] = value;
                return true;
            }
        }
        false
    }

    pub fn str_value(&self, obj: &ObjRef) -> Option<String> {
        match &obj.0.borrow().data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn sym_value(&self, obj: &ObjRef) -> Option<Symbol> {
        match &obj.0.borrow().data {
            ObjectData::Sym(s) => Some(*s),
            _ => None,
        }
    }

    pub fn closure_data(&self, obj: &ObjRef) -> Option<ClosureData> {
        match &obj.0.borrow().data {
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
        obj.0.borrow().clone()
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
        *obj.0.borrow_mut() = filled;
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
            Value::Obj(obj) => match &obj.0.borrow().data {
                ObjectData::Str(s) => format!("the String {s:?}"),
                ObjectData::Sym(s) => format!("the Symbol #{s}"),
                ObjectData::Array(items) => format!("an Array of {}", items.len()),
                ObjectData::Closure(_) => "a Block".to_string(),
                ObjectData::Class(data) => {
                    format!("the class {}", data.application.mixin.name)
                }
                ObjectData::Instance(_) => match &obj.0.borrow().class.0.borrow().data {
                    ObjectData::Class(data) => {
                        format!("an instance of {}", data.application.mixin.name)
                    }
                    // `types.rs`'s `ClassRef` doc: every `Object::class` is
                    // an `ObjRef` whose own `data` is `ObjectData::Class`,
                    // by convention rather than by the type system. This arm
                    // exists only so the match is total, not because a
                    // conforming object can reach it.
                    _ => "an object".to_string(),
                },
            },
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
        match &class.0.borrow().data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    pub fn class_enclosing(&self, class: &ClassRef) -> Option<ObjRef> {
        match &class.0.borrow().data {
            ObjectData::Class(data) => data.enclosing.clone(),
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.0.borrow().data {
            ObjectData::Class(data) => data.declaring_class.clone(),
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.0.borrow().data {
            ObjectData::Class(data) => data.superclass.clone(),
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: &ClassRef) -> Option<BuiltinClass> {
        match &class.0.borrow().data {
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
        match &class.0.borrow().data {
            ObjectData::Class(data) => Some((data.builtin, data.superclass.clone())),
            _ => None,
        }
    }

    pub fn class_metaclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.0.borrow().data {
            ObjectData::Class(data) => data.metaclass.clone(),
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
        if let ObjectData::Class(data) = &mut class.0.borrow_mut().data {
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
        if let ObjectData::Class(data) = &mut class.0.borrow_mut().data {
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
        if let ObjectData::Class(data) = &mut class.0.borrow_mut().data {
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
        frame.0.borrow().receiver.clone()
    }

    /// The receiver and `current_class` together, in one borrow.
    /// `dispatch_lexical_send`'s head needs both on every implicit-receiver
    /// send -- five times per `bench_loop.ns` iteration -- and fetching them
    /// through `receiver` then `frame_current_class` took two separate
    /// borrows where one already carries both fields; that doubling was the
    /// whole of a measured 13% regression (see `docs/design/execution.md`'s
    /// measurements section).
    pub fn receiver_and_current_class(&self, frame: &FrameRef) -> (Value, Option<ClassRef>) {
        let borrowed = frame.0.borrow();
        (borrowed.receiver.clone(), borrowed.current_class.clone())
    }

    pub fn local(&self, frame: &FrameRef, index: u16) -> Value {
        let borrowed = frame.0.borrow();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i].clone()
        } else {
            borrowed.temps[i - borrowed.args.len()].clone()
        }
    }

    pub fn set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        let mut borrowed = frame.0.borrow_mut();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i] = value;
        } else {
            let temp_index = i - borrowed.args.len();
            borrowed.temps[temp_index] = value;
        }
    }

    pub fn frame_current_class(&self, frame: &FrameRef) -> Option<ClassRef> {
        frame.0.borrow().current_class.clone()
    }

    /// The absolute field index a compiled `InitSlot`'s mixin-relative
    /// `own_index` is offset by. Fixed for the frame's whole life --
    /// `interpreter::instantiate` builds one frame per inheritance level and
    /// sets this from that level's own `MixinApplication.own_slot_base`, so
    /// there is nothing to save and restore between levels.
    pub fn frame_slot_base(&self, frame: &FrameRef) -> usize {
        frame.0.borrow().slot_base
    }

    /// Grows an activation's register file, filling with nil. Needed by
    /// the initializer path, where a level's compiled body may declare
    /// more registers than the frame was built with, and by
    /// edit-and-continue later (`docs/design/liveness-and-reflection.md`).
    pub fn grow_temps(&mut self, frame: &FrameRef, needed: usize) {
        let mut f = frame.0.borrow_mut();
        if f.temps.len() < needed {
            f.temps.resize(needed, Value::Nil);
        }
    }

    pub fn set_frame_pc(&mut self, frame: &FrameRef, pc: usize) {
        frame.0.borrow_mut().pc = pc;
    }

    pub fn frame_home(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.0.borrow().home.clone()
    }

    /// The activation that invoked `frame`, `None` only for `FrameKind::TopLevel`.
    /// `tests/builtins_collections_block.rs`'s sender-threading regression test is
    /// the first caller: it walks a captured closure's frame back to the method
    /// activation that invoked it, to check `sender` was actually recorded rather
    /// than left `None` (the Final-review finding I6 defect this guards against).
    pub fn frame_sender(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.0.borrow().sender.clone()
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `eval::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: &FrameRef) -> Option<FrameRef> {
        match &frame.0.borrow().kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame.clone()),
            _ => None,
        }
    }

    pub fn frame_is_uncontinuable(&self, frame: &FrameRef) -> bool {
        frame.0.borrow().uncontinuable
    }

    pub fn mark_uncontinuable(&mut self, frame: &FrameRef) {
        frame.0.borrow_mut().uncontinuable = true;
    }

    /// Whether `frame`'s kind is `FrameKind::Method` --
    /// `Interpreter::run_statements` needs this to decide what an implicit
    /// (non-`^`) fall-off-the-end return value is: `self` for a method body
    /// (spec §3.5), the last statement's value for every other body this
    /// loop runs.
    pub fn frame_is_method(&self, frame: &FrameRef) -> bool {
        matches!(frame.0.borrow().kind, FrameKind::Method(_))
    }

    /// A copy of `frame`'s `kind`. Cloning is cheap -- every variant holds
    /// at most an `Rc` -- and `image::decode` is the only caller that needs
    /// to match on the *whole* kind rather than one variant's payload
    /// (`frame_captured`, `frame_is_method`): once to resolve which of
    /// `Initializer`/`SuperclassClause`/`SuperclassMessage` a frame carries
    /// a class for (`frame_body`), and once to check that a class-carrying
    /// kind's class is itself a class reference (`check_class_references`).
    pub fn frame_kind(&self, frame: &FrameRef) -> FrameKind {
        frame.0.borrow().kind.clone()
    }

    /// `pc`, and how many of a frame's registers are `args` versus `temps`,
    /// in the one borrow all three live behind.
    /// `image::decode::verify_graph` checks all three against the shape of
    /// the compiled body it reconciles every decoded frame against.
    pub fn frame_register_shape(&self, frame: &FrameRef) -> (usize, usize, usize) {
        let f = frame.0.borrow();
        (f.pc, f.args.len(), f.temps.len())
    }

    /// The whole of `frame`'s contents, in the one borrow they all live
    /// behind. `image::encode::encode_frame` writes a whole frame record
    /// and `GraphTable::visit_frame` walks a whole frame's references;
    /// both need every field. Not for a hot path, for the same reason as
    /// `object_snapshot`.
    pub fn frame_snapshot(&self, frame: &FrameRef) -> Frame {
        frame.0.borrow().clone()
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
        *frame.0.borrow_mut() = filled;
    }
}

/// The one place an `ObjRef`'s inner `Rc<RefCell<Object>>` is actually
/// built. `alloc_object` and `alloc_placeholder_object` differ only in
/// where the identity hash comes from; this is the shared tail both call.
fn new_object_handle(class: ClassRef, identity_hash: u32, data: ObjectData) -> ObjRef {
    ObjRef(std::rc::Rc::new(std::cell::RefCell::new(Object {
        class,
        identity_hash,
        data,
    })))
}

/// Builds a class object that is genuinely its own `.class` -- the shape
/// every one of the crate's four bootstraps needs (`Interpreter::bootstrap`
/// building the real `Object` class in production; `dispatch.rs`'s,
/// `types.rs`'s and `tests/interpreter.rs`'s own test suites each building a
/// throwaway one to satisfy `Object.class`'s non-optional type without
/// wiring up a whole hierarchy). `Object.class` is non-optional (no
/// `Option`/`Weak`), so this cannot be built via `Rc::new_cyclic` in one
/// safe step: `new_cyclic`'s closure only ever gets a `Weak` that fails to
/// upgrade until construction finishes, and using `unreachable!()` as a
/// throwaway placeholder panics immediately on evaluation (eager, not
/// lazy) rather than compiling to something inert. Instead: allocate the
/// memory uninitialized, mint a second strong `Rc` handle to the SAME
/// allocation (its count balanced via `Rc::increment_strong_count` before
/// `Rc::from_raw`), write the real value using that handle as `class`,
/// then `assume_init`.
///
/// Identity hash fixed at 0, like `alloc_placeholder_object`: none of the
/// four callers link this object into a real hierarchy a hash could ever
/// distinguish it within.
///
/// Private to this module: `Heap::alloc_self_referential_class` is the only
/// caller, so a fifth call site cannot mint a second live self-referential
/// object outside a heap. That mattered even before the arena — such an
/// object shares identity hash 0 with every placeholder
/// `alloc_placeholder_object` has ever built, and code elsewhere reasons
/// about hash-0 objects being throwaway and never observed outside their
/// own construction.
fn bootstrap_self_referential_object(data: ObjectData) -> ObjRef {
    let uninit: std::rc::Rc<std::mem::MaybeUninit<std::cell::RefCell<Object>>> =
        std::rc::Rc::new_uninit();
    let raw_mut =
        std::rc::Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<std::cell::RefCell<Object>>;
    let data_ptr = raw_mut as *const std::cell::RefCell<Object>;
    // SAFETY: `data_ptr` points at the allocation `uninit` owns; we bump
    // its strong count by one before minting a second `Rc` handle to it
    // via `from_raw`, so the two handles (`uninit`, and the `class` field
    // written below) share a correctly balanced count. `MaybeUninit<T>` is
    // guaranteed to have the same layout as `T`, so the pointer cast is
    // sound; neither handle is dereferenced before the `write` below
    // completes.
    unsafe { std::rc::Rc::increment_strong_count(data_ptr) };
    let self_ref = ObjRef(unsafe { std::rc::Rc::from_raw(data_ptr) });
    unsafe {
        (*raw_mut).write(std::cell::RefCell::new(Object {
            class: self_ref,
            identity_hash: 0,
            data,
        }));
    }
    // SAFETY: the write above just initialized the payload; `uninit` is
    // the allocation's original handle, now fully initialized.
    ObjRef(unsafe { uninit.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A frame is addressed by its position in the arena, so a handle must
    /// stay valid across the reallocation that growing the arena performs.
    /// Under `Rc` this was free; under an arena it is the property the
    /// representation rests on.
    #[test]
    fn a_frame_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        let first = heap.alloc_frame(top_level_frame());
        for _ in 0..1000 {
            heap.alloc_frame(top_level_frame());
        }
        assert!(matches!(heap.receiver(&first), Value::Nil));
        assert_eq!(heap.frame_register_shape(&first), (0, 0, 0));
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
}
