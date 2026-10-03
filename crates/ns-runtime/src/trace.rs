//! What the collector traces (`docs/design/memory.md`, "What the collector
//! traces"): every field of every runtime type that holds an `ObjectReference`,
//! a `ClassReference`, a `FrameReference` or a stored `Word`, handed to a
//! `Tracer` one `&mut` at a time. Enumerating is this module's; rewriting is
//! `heap.rs`'s `Forwarder`, since only `heap.rs` can mint a handle.
//!
//! The impls for an object's and a frame's own payload live beside those
//! types, in `heap/object.rs`, which nothing outside the heap may name.
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
    fn frame(&mut self, handle: &mut FrameReference<'_>);
    /// A stored word: rewritten by its index, since it carries no epoch.
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

fn trace_values<T: Tracer>(values: &mut [Value<'_>], tracer: &mut T) {
    for value in values {
        tracer.value(value);
    }
}

pub(crate) fn trace_words<T: Tracer>(words: &mut [Word], tracer: &mut T) {
    for word in words {
        tracer.word(word);
    }
}

pub(crate) fn trace_optional_object<T: Tracer>(
    handle: &mut Option<ObjectReference<'_>>,
    tracer: &mut T,
) {
    if let Some(handle) = handle {
        tracer.object(handle);
    }
}

impl Trace for FrameKind<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            FrameKind::Method(_) | FrameKind::TopLevel => {}
            FrameKind::Closure(closure) => tracer.object(closure),
            FrameKind::Initializer(class)
            | FrameKind::SuperclassClause(class)
            | FrameKind::SuperclassMessage(class)
            | FrameKind::MixinApplicationMessage(class) => tracer.object(class),
        }
    }
}

impl Trace for Continuation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            Continuation::Deliver => {}
            Continuation::WhileCondition(state) | Continuation::WhileBody(state) => {
                state.trace(tracer)
            }
            Continuation::DoElement(state) => state.trace(tracer),
            Continuation::NestedClass(state) => state.trace(tracer),
            Continuation::Instantiate(state) => state.trace(tracer),
            Continuation::Unwind(pending) => pending.trace(tracer),
            Continuation::VmErrorSignalled(error) => tracer.value(&mut error.receiver),
            Continuation::Future(forcing) => forcing.trace(tracer),
        }
    }
}

/// The future of every forcing in the chain: each waiting one is held by
/// nothing but the link from the forcing it waits on. A loop rather than a
/// recursion: a chain is as long as a run of futures each answering the
/// next, which a program decides and no depth limit bounds.
impl Trace for crate::interpreter::future::FutureForcing<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let mut forcing = self;
        loop {
            let crate::interpreter::future::FutureForcing {
                future,
                after: _,
                then,
            } = forcing;
            tracer.object(future);
            match then {
                Some(waiting) => forcing = waiting,
                None => break,
            }
        }
    }
}

impl Trace for PendingUnwind<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let PendingUnwind { target, action } = self;
        if let Some(target) = target {
            tracer.frame(target);
        }
        match action {
            UnwindAction::Return(value) => tracer.value(value),
            UnwindAction::Restart => {}
            UnwindAction::Terminate(error) => tracer.value(&mut error.receiver),
        }
    }
}

impl Trace for WhileLoop<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let WhileLoop {
            condition,
            body,
            expect: _,
        } = self;
        tracer.object(condition);
        trace_optional_object(body, tracer);
    }
}

impl Trace for DoLoop<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let DoLoop {
            receiver,
            block,
            items,
            next: _,
        } = self;
        tracer.value(receiver);
        tracer.object(block);
        trace_values(items, tracer);
    }
}

impl Trace for NestedClassCreation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let NestedClassCreation {
            enclosing,
            index: _,
            nested: _,
            declaring_class,
        } = self;
        tracer.object(enclosing);
        tracer.object(declaring_class);
    }
}

impl Trace for Instantiation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Instantiation {
            instance,
            levels,
            // An index into `levels`, which is traced below.
            applying: _,
            // Compiled code and indices into `levels`: no handle.
            deferred: _,
            phase,
        } = self;
        tracer.value(instance);
        for (level, arguments) in levels {
            tracer.object(level);
            trace_values(arguments, tracer);
        }
        phase.trace(tracer);
    }
}

impl Trace for InstantiationPhase<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            InstantiationPhase::Climbing { level, superclass } => {
                tracer.object(level);
                tracer.object(superclass);
            }
            InstantiationPhase::Descending { index: _ }
            | InstantiationPhase::Applying {
                index: _,
                applying: _,
            } => {}
        }
    }
}
