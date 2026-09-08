//! The heap seam. Every read and write of an `Object` -- and, from Task 5,
//! of a `Frame` -- goes through `Heap`, so that replacing what sits behind
//! a handle changes this file rather than its call sites. The store behind
//! the accessors is `Rc<RefCell<_>>` today and an arena of typed handles
//! next; see `docs/design/memory.md`, "Planned: an arena, handles, and a
//! real collector".
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
