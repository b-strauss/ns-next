//! What the collector traces (`docs/design/memory.md`, "What the collector
//! traces"): every field of every runtime type that holds an `ObjectReference`,
//! a `ClassReference` or a `FrameReference`, handed to a `Tracer` one `&mut` at
//! a time. Enumerating is this module's; rewriting is `heap.rs`'s `Forwarder`,
//! since only `heap.rs` can mint a handle.
//!
//! Every impl names every field and matches every variant, with no `..` and
//! no `_`: a field or variant added later fails to compile here until
//! someone decides whether it holds a handle. That is what keeps the
//! enumeration complete as the types grow; the stress mode checks that the
//! places it is applied to are complete.
//!
//! Debug builds only, until the collector (step 5) needs it in every build.

use crate::heap::{FrameReference, ObjectReference};
use crate::types::*;

pub(crate) trait Tracer {
    fn object(&mut self, handle: &mut ObjectReference<'_>);
    fn frame(&mut self, handle: &mut FrameReference<'_>);
    fn value(&mut self, value: &mut Value<'_>) {
        match value {
            Value::Object(handle) => self.object(handle),
            Value::Nil | Value::Boolean(_) | Value::SmallInteger(_) | Value::Float(_) => {}
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

fn trace_optional_object<T: Tracer>(handle: &mut Option<ObjectReference<'_>>, tracer: &mut T) {
    if let Some(handle) = handle {
        tracer.object(handle);
    }
}

impl Trace for Object<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Object {
            class,
            identity_hash: _,
            dispatch_id: _,
            data,
        } = self;
        tracer.object(class);
        data.trace(tracer);
    }
}

impl Trace for ObjectData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            ObjectData::Instance(values) | ObjectData::Array(values) => {
                trace_values(values, tracer)
            }
            ObjectData::String(_) | ObjectData::Symbol(_) => {}
            ObjectData::Closure(closure) => closure.trace(tracer),
            ObjectData::Class(class) => class.trace(tracer),
        }
    }
}

impl Trace for ClosureData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let ClosureData {
            parameters: _,
            code: _,
            captured_frame,
            home,
            feedback_base: _, // not a handle
        } = self;
        tracer.frame(captured_frame);
        tracer.frame(home);
    }
}

impl Trace for ClassData<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        // `application` holds no handle: a mixin application is behaviour,
        // shared by every class object built from it.
        let ClassData {
            application: _,
            enclosing,
            declaring_class,
            superclass,
            builtin: _,
            metaclass,
            dispatch_id: _, // not a handle
        } = self;
        trace_optional_object(enclosing, tracer);
        trace_optional_object(declaring_class, tracer);
        trace_optional_object(superclass, tracer);
        trace_optional_object(metaclass, tracer);
    }
}

impl Trace for Frame<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Frame {
            receiver,
            arguments,
            temporaries,
            program_counter: _,
            sender,
            home,
            current_class,
            kind,
            uncontinuable: _,
            slot_base: _,
        } = self;
        tracer.value(receiver);
        trace_values(arguments, tracer);
        trace_values(temporaries, tracer);
        if let Some(sender) = sender {
            tracer.frame(sender);
        }
        if let Some(home) = home {
            tracer.frame(home);
        }
        trace_optional_object(current_class, tracer);
        kind.trace(tracer);
    }
}

impl Trace for FrameKind<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            FrameKind::Method(_) | FrameKind::TopLevel => {}
            FrameKind::Closure(closure) => tracer.object(closure),
            FrameKind::Initializer(class)
            | FrameKind::SuperclassClause(class)
            | FrameKind::SuperclassMessage(class) => tracer.object(class),
        }
    }
}

impl Trace for Continuation<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            Continuation::Deliver | Continuation::Truthy => {}
            Continuation::WhileCondition(state) | Continuation::WhileBody(state) => {
                state.trace(tracer)
            }
            Continuation::DoElement(state) => state.trace(tracer),
            Continuation::NestedClass(state) => state.trace(tracer),
            Continuation::Instantiate(state) => state.trace(tracer),
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
        tracer.value(body);
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
            InstantiationPhase::Descending { index: _ } => {}
        }
    }
}
