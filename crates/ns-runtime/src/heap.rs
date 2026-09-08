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

impl Default for Heap {
    fn default() -> Self {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            next_identity_hash: 1,
        }
    }

    /// Saturating rather than wrapping: an exhausted counter must not hand
    /// out an id a live object already holds.
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

    // ---------- Object accessors ----------
    //
    // `&self` is unused while a handle is an `Rc`, because the data hangs
    // off the handle itself. It is in these signatures from the start
    // because the arena will need it, and adding it afterwards would touch
    // every call site this module exists to protect. The mutating
    // accessors take `&mut self` for the same reason, even though the
    // `RefCell` behind the handle would make `&self` sufficient today.

    pub fn identity_hash(&self, obj: &ObjRef) -> u32 {
        obj.borrow().identity_hash
    }

    pub fn class_of_obj(&self, obj: &ObjRef) -> ClassRef {
        obj.borrow().class.clone()
    }

    pub fn is_class(&self, obj: &ObjRef) -> bool {
        matches!(obj.borrow().data, ObjectData::Class(_))
    }

    pub fn array_len(&self, obj: &ObjRef) -> Option<usize> {
        match &obj.borrow().data {
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
        match &obj.borrow().data {
            ObjectData::Instance(fields) => Some(fields.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, obj: &ObjRef) -> Option<Vec<Value>> {
        match &obj.borrow().data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.borrow().data {
            ObjectData::Array(items) => items.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, obj: &ObjRef, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items.push(value);
            return true;
        }
        false
    }

    /// The instance field at `index`, or `None` when `obj`'s payload is not
    /// `ObjectData::Instance` or `index` is out of range -- the same
    /// total-rather-than-panicking shape as the array accessors above.
    pub fn instance_field(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.borrow().data {
            ObjectData::Instance(fields) => fields.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened, mirroring `array_set`.
    pub fn set_instance_field(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
            if index < fields.len() {
                fields[index] = value;
                return true;
            }
        }
        false
    }

    pub fn str_value(&self, obj: &ObjRef) -> Option<String> {
        match &obj.borrow().data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn sym_value(&self, obj: &ObjRef) -> Option<Symbol> {
        match &obj.borrow().data {
            ObjectData::Sym(s) => Some(*s),
            _ => None,
        }
    }

    pub fn closure_data(&self, obj: &ObjRef) -> Option<ClosureData> {
        match &obj.borrow().data {
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
        obj.borrow().clone()
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
        *obj.borrow_mut() = filled;
    }

    /// Renders a value for a user-facing error message.
    ///
    /// Never `Debug`-format a `Value` instead: `Object`'s bootstrap class is
    /// its own `.class`, so `Debug` on a `Value::Obj` recurses until the
    /// stack gives out. Every caller of this is building the message of a
    /// `NewspeakError`, which reaches stderr, so that recursion turns a
    /// reportable program error into a process abort.
    pub fn describe(&self, value: &Value) -> String {
        match value {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::SmallInt(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Obj(obj) => match &obj.borrow().data {
                ObjectData::Str(s) => format!("the String {s:?}"),
                ObjectData::Sym(s) => format!("the Symbol #{s}"),
                ObjectData::Array(items) => format!("an Array of {}", items.len()),
                ObjectData::Closure(_) => "a Block".to_string(),
                ObjectData::Class(data) => {
                    format!("the class {}", data.application.mixin.name)
                }
                ObjectData::Instance(_) => match &obj.borrow().class.borrow().data {
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
        match &class.borrow().data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    pub fn class_enclosing(&self, class: &ClassRef) -> Option<ObjRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.enclosing.clone(),
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.declaring_class.clone(),
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.superclass.clone(),
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: &ClassRef) -> Option<BuiltinClass> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.builtin,
            _ => None,
        }
    }

    pub fn class_metaclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.metaclass.clone(),
            _ => None,
        }
    }

    pub fn set_class_metaclass(&mut self, class: &ClassRef, metaclass: ClassRef) {
        if let ObjectData::Class(data) = &mut class.borrow_mut().data {
            data.metaclass = Some(metaclass);
        }
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
        if let ObjectData::Class(data) = &mut class.borrow_mut().data {
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
        if let ObjectData::Class(data) = &mut class.borrow_mut().data {
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
        frame.borrow().receiver.clone()
    }

    /// The receiver and `current_class` together, in one borrow.
    /// `dispatch_lexical_send`'s head needs both on every implicit-receiver
    /// send -- five times per `bench_loop.ns` iteration -- and fetching them
    /// through `receiver` then `frame_current_class` took two separate
    /// borrows where one already carries both fields; that doubling measured
    /// as the whole of Task 5's regression (see `docs/design/memory.md`).
    pub fn receiver_and_current_class(&self, frame: &FrameRef) -> (Value, Option<ClassRef>) {
        let borrowed = frame.borrow();
        (borrowed.receiver.clone(), borrowed.current_class.clone())
    }

    pub fn local(&self, frame: &FrameRef, index: u16) -> Value {
        let borrowed = frame.borrow();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i].clone()
        } else {
            borrowed.temps[i - borrowed.args.len()].clone()
        }
    }

    pub fn set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        let mut borrowed = frame.borrow_mut();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i] = value;
        } else {
            let temp_index = i - borrowed.args.len();
            borrowed.temps[temp_index] = value;
        }
    }

    pub fn frame_current_class(&self, frame: &FrameRef) -> Option<ClassRef> {
        frame.borrow().current_class.clone()
    }

    /// The absolute field index a compiled `InitSlot`'s mixin-relative
    /// `own_index` is offset by. Fixed for the frame's whole life --
    /// `interpreter::instantiate` builds one frame per inheritance level and
    /// sets this from that level's own `MixinApplication.own_slot_base`, so
    /// there is nothing to save and restore between levels.
    pub fn frame_slot_base(&self, frame: &FrameRef) -> usize {
        frame.borrow().slot_base
    }

    /// Grows an activation's register file, filling with nil. Needed by
    /// the initializer path, where a level's compiled body may declare
    /// more registers than the frame was built with, and by
    /// edit-and-continue later (`docs/design/liveness-and-reflection.md`).
    pub fn grow_temps(&mut self, frame: &FrameRef, needed: usize) {
        let mut f = frame.borrow_mut();
        if f.temps.len() < needed {
            f.temps.resize(needed, Value::Nil);
        }
    }

    pub fn set_frame_pc(&mut self, frame: &FrameRef, pc: usize) {
        frame.borrow_mut().pc = pc;
    }

    pub fn frame_home(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.borrow().home.clone()
    }

    /// The activation that invoked `frame`, `None` only for `FrameKind::TopLevel`.
    /// `tests/builtins_collections_block.rs`'s sender-threading regression test is
    /// the first caller: it walks a captured closure's frame back to the method
    /// activation that invoked it, to check `sender` was actually recorded rather
    /// than left `None` (the Final-review finding I6 defect this guards against).
    pub fn frame_sender(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.borrow().sender.clone()
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `eval::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: &FrameRef) -> Option<FrameRef> {
        match &frame.borrow().kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame.clone()),
            _ => None,
        }
    }

    pub fn frame_is_uncontinuable(&self, frame: &FrameRef) -> bool {
        frame.borrow().uncontinuable
    }

    pub fn mark_uncontinuable(&mut self, frame: &FrameRef) {
        frame.borrow_mut().uncontinuable = true;
    }

    /// Whether `frame`'s kind is `FrameKind::Method` --
    /// `Interpreter::run_statements` needs this to decide what an implicit
    /// (non-`^`) fall-off-the-end return value is: `self` for a method body
    /// (spec §3.5), the last statement's value for every other body this
    /// loop runs.
    pub fn frame_is_method(&self, frame: &FrameRef) -> bool {
        matches!(frame.borrow().kind, FrameKind::Method(_))
    }

    /// A copy of `frame`'s `kind`. Cloning is cheap -- every variant holds
    /// at most an `Rc` -- and `image::decode` is the only caller that needs
    /// to match on the *whole* kind rather than one variant's payload
    /// (`frame_captured`, `frame_is_method`): once to resolve which of
    /// `Initializer`/`SuperclassClause`/`SuperclassMessage` a frame carries
    /// a class for (`frame_body`), and once to check that a class-carrying
    /// kind's class is itself a class reference (`check_class_references`).
    pub fn frame_kind(&self, frame: &FrameRef) -> FrameKind {
        frame.borrow().kind.clone()
    }

    /// `pc`, and how many of a frame's registers are `args` versus `temps`,
    /// in the one borrow all three live behind.
    /// `image::decode::verify_graph` checks all three against the shape of
    /// the compiled body it reconciles every decoded frame against.
    pub fn frame_register_shape(&self, frame: &FrameRef) -> (usize, usize, usize) {
        let f = frame.borrow();
        (f.pc, f.args.len(), f.temps.len())
    }

    /// The whole of `frame`'s contents, in the one borrow they all live
    /// behind. `image::encode::encode_frame` writes a whole frame record
    /// and `GraphTable::visit_frame` walks a whole frame's references;
    /// both need every field. Not for a hot path, for the same reason as
    /// `object_snapshot`.
    pub fn frame_snapshot(&self, frame: &FrameRef) -> Frame {
        frame.borrow().clone()
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
        *frame.borrow_mut() = filled;
    }
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
}
