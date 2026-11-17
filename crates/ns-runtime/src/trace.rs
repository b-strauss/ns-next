//! What the collector traces (`docs/design/memory.md`, "What the collector
//! traces"): every field of every runtime type that holds an `ObjectReference`,
//! a `ClassReference`, a `FrameReference` or a stored `Word`, handed to a
//! `Tracer` one `&mut` at a time. Enumerating is this module's; rewriting is
//! `heap.rs`'s `Forwarder`, since only `heap.rs` can mint a handle.
//!
//! An object -- a reified frame included -- has no impl: it is a block of
//! words, and `heap/object.rs`'s `trace_block`, which nothing outside the
//! heap may name, decides which of them hold a stored `Word` by the
//! header's format.
//!
//! Every impl names every field and matches every variant, with no `..` and
//! no `_`: a field or variant added later fails to compile here until
//! someone decides whether it holds a handle. That is what keeps the
//! enumeration complete as the types grow; the stress mode checks that the
//! places it is applied to are complete.
//!
//! In every build: the collector marks through the same enumeration the
//! stress mode's forwarding walks, which is what keeps the two from
//! disagreeing about what holds a handle.

use crate::heap::{FrameReference, ObjectReference, Word};
use crate::types::*;

pub(crate) trait Tracer {
    fn object(&mut self, handle: &mut ObjectReference<'_>);
    /// A frame, which every tracer treats as the frame object it names.
    fn frame(&mut self, handle: &mut FrameReference<'_>);
    /// A stored word: rewritten by its offset, since it carries no epoch.
    fn word(&mut self, word: &mut Word);
    fn value(&mut self, value: &mut Value<'_>) {
        if let Some(mut object) = value.as_object() {
            self.object(&mut object);
            *value = Value::object(object);
        }
    }
}

pub(crate) trait Trace {
    fn trace<T: Tracer>(&mut self, tracer: &mut T);
}

impl Trace for ActivationKind<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            ActivationKind::Method(_) | ActivationKind::TopLevel => {}
            ActivationKind::Closure(closure) => tracer.object(closure),
            ActivationKind::Initializer(class, _)
            | ActivationKind::Factory(class)
            | ActivationKind::SuperclassClause(class) => tracer.object(class),
        }
    }
}

impl Trace for Continuation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            Continuation::Deliver | Continuation::Reexecute | Continuation::Evaluated => {}
            Continuation::NestedClass(state) => state.trace(tracer),
            Continuation::Unwind(pending) | Continuation::ExitEvaluated(pending) => {
                pending.trace(tracer)
            }
            Continuation::VmErrorSignalled(error) => {
                tracer.value(&mut error.receiver);
                tracer.value(&mut error.subject);
            }
            Continuation::ReturnToSuspended(suspended) => suspended.trace(tracer),
        }
    }
}

impl Trace for ReturnToSuspended<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let ReturnToSuspended { sender, then } = self;
        tracer.frame(sender);
        then.trace(tracer);
    }
}

impl Trace for PendingUnwind<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let PendingUnwind { target, action } = self;
        if let Some(target) = target {
            tracer.frame(target);
        }
        match action {
            UnwindAction::Return(value) | UnwindAction::ForceReturn(value) => tracer.value(value),
            UnwindAction::Restart | UnwindAction::RestartFrame => {}
            UnwindAction::Terminate(error) => {
                tracer.value(&mut error.receiver);
                tracer.value(&mut error.subject);
            }
        }
    }
}

impl Trace for NestedClassCreation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let NestedClassCreation {
            enclosing,
            index: _,
            nested: _,
            declaring_class,
            // A number: no handle.
            serial: _,
        } = self;
        tracer.object(enclosing);
        tracer.object(declaring_class);
    }
}
