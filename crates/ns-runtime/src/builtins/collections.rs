//! `Array` and `OrderedCollection` native methods.
//!
//! Design note (a deliberate simplification): `OrderedCollection` needs a
//! mutable, growable payload, but `ObjectData::Array(Vec<Value>)` is shared
//! by both `Array` and `OrderedCollection` -- the spec's Core Library list
//! groups them loosely together, and `Vec<Value>` already grows fine either
//! way (`Heap::array_push` mutates it in place through `&mut self`, no
//! separate growable representation needed). So `OrderedCollection` is
//! backed by the very same `ObjectData::Array` variant rather than a second
//! growable-collection payload variant. A real `OrderedCollection` would
//! grow independently of a literal array's fixed size; revisit if the two
//! ever need to diverge (e.g. `Array` becoming genuinely fixed-size).

use crate::builtins::primitive::CollectionPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

fn out_of_range<'gc>(
    receiver: &Value<'gc>,
    selector: Symbol,
    index: i64,
    length: usize,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "index {index} out of range: valid indices are 1..{length} (size {length})"
        ),
    }
}

fn not_an_integer<'gc>(
    heap: &Heap<'gc>,
    receiver: &Value<'gc>,
    selector: Symbol,
    got: Option<&Value<'gc>>,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "{selector} requires an Integer index, got {}",
            heap.describe_argument(got)
        ),
    }
}

/// `size` -- shared by `Array` and `OrderedCollection` since both share the
/// `ObjectData::Array` payload (see this module's design note). `None` if
/// `receiver` isn't `ObjectData::Array`-backed at all.
fn size<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    mutator
        .heap
        .array_length(*object)
        .map(|length| Ok(Value::SmallInteger(length as i64)))
}

/// `at:` -- shared by `Array` and `OrderedCollection`, same payload as `size`.
fn element_at<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let length = mutator.heap.array_length(*object)?;
    Some(match arguments.first() {
        Some(Value::SmallInteger(index)) => {
            if *index >= 1 && (*index as usize) <= length {
                Ok(mutator
                    .heap
                    .array_at(*object, (*index as usize) - 1)
                    .unwrap_or(Value::Nil))
            } else {
                Err(out_of_range(&receiver, selector, *index, length))
            }
        }
        other => Err(not_an_integer(&mutator.heap, &receiver, selector, other)),
    })
}

/// `at:put:` -- `Array`'s alone; the lists in `builtins::primitive` are what
/// keeps `OrderedCollection` from answering it.
fn element_at_put<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let length = mutator.heap.array_length(*object)?;
    let index = match arguments.first() {
        Some(Value::SmallInteger(index)) => *index,
        other => {
            return Some(Err(not_an_integer(
                &mutator.heap,
                &receiver,
                selector,
                other,
            )))
        }
    };
    if index < 1 || (index as usize) > length {
        return Some(Err(out_of_range(&receiver, selector, index, length)));
    }
    let value = arguments.get(1).copied().unwrap_or(Value::Nil);
    mutator.heap.array_set(*object, (index as usize) - 1, value);
    Some(Ok(receiver))
}

/// `add:` -- `OrderedCollection`'s alone; the lists in `builtins::primitive`
/// are what keeps `Array` from answering it.
fn add<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    mutator.heap.array_length(*object)?;
    // Matches the reference library's convention: `add:` returns the
    // argument, not the receiver.
    let value = arguments.first().copied().unwrap_or(Value::Nil);
    mutator.heap.array_push(*object, value);
    Some(Ok(value))
}

/// `Array new: size`: an array of `size` `nil`s. Its subject is the class
/// the forwarder runs in, which is only ever `Array` itself -- §3.2's
/// metaclasses are flat, so no subclass inherits `new:` -- and anything
/// else declines. A size that cannot be allocated is an error, found with
/// `try_reserve_exact` before anything is filled, never a Rust allocation
/// abort: the size is the program's, and an abort is not catchable.
fn allocate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(class) = receiver else {
        return None;
    };
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Array) {
        return None;
    }
    let refusal = |message: String| NewspeakError {
        selector,
        receiver,
        message,
    };
    let size = match arguments.first() {
        Some(Value::SmallInteger(size)) => *size,
        other => {
            return Some(Err(refusal(format!(
                "{selector} requires an Integer size, got {}",
                mutator.heap.describe_argument(other)
            ))))
        }
    };
    let Ok(length) = usize::try_from(size) else {
        return Some(Err(refusal(format!(
            "{selector} requires a size of at least 0, got {size}"
        ))));
    };
    let mut elements: Vec<Value<'gc>> = Vec::new();
    if elements.try_reserve_exact(length).is_err() {
        return Some(Err(refusal(format!(
            "{selector} cannot allocate an Array of {size} elements"
        ))));
    }
    elements.resize(length, Value::Nil);
    Some(Ok(Value::Object(mutator.new_array(elements))))
}

/// Lifts a leaf's result into the step it is, so `execute`'s leaf arms don't
/// each repeat the wrapping.
fn leaf_step<'gc>(
    result: Option<Result<Value<'gc>, NewspeakError<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    Some(result?.map(Step::Value).map_err(Signal::Error))
}

/// `Array` and `OrderedCollection`'s native methods: which class answers
/// `at:put:` and which `add:` is decided by their lists in
/// `builtins::primitive`, not by this function -- both send
/// `CollectionPrimitive::ElementAtPut`/`Add` through the same arm here.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: CollectionPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        CollectionPrimitive::Enumerate => do_step(mutator, receiver, selector, arguments),
        CollectionPrimitive::Size => leaf_step(size(mutator, receiver)),
        CollectionPrimitive::ElementAt => {
            leaf_step(element_at(mutator, receiver, selector, arguments))
        }
        CollectionPrimitive::ElementAtPut => {
            leaf_step(element_at_put(mutator, receiver, selector, arguments))
        }
        CollectionPrimitive::Add => leaf_step(add(mutator, receiver, arguments)),
        CollectionPrimitive::Allocate => {
            leaf_step(allocate(mutator, receiver, selector, arguments))
        }
    }
}

/// `do:` on an `Array` or an `OrderedCollection`: the block activated for each
/// element in turn, as trampoline steps -- the first here, each next one by
/// `after_element` when the one before it returns -- so no Rust frame waits
/// on Newspeak code (`docs/design/execution.md`, "No re-entry from Rust").
/// `None` when `receiver` has no `ObjectData::Array` payload, so the send
/// falls through to `doesNotUnderstand:` as every other selector here does.
fn do_step<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let items = mutator.heap.array_elements(*object)?;
    let Some(block) = arguments.first().copied() else {
        return Some(Err(Signal::Error(NewspeakError {
            selector,
            receiver,
            message: "do: requires a block argument".to_string(),
        })));
    };
    // No element, no activation: the loop is over before it starts, so it
    // needs no state either.
    if items.is_empty() {
        return Some(Ok(Step::Value(receiver)));
    }
    // A non-block stands for its own value, so handing it the elements
    // observes nothing: the loop is over before it starts.
    let Value::Object(block) = block else {
        return Some(Ok(Step::Value(receiver)));
    };
    if !mutator.heap.is_closure(block) {
        return Some(Ok(Step::Value(receiver)));
    }
    Some(Ok(element_step(Box::new(DoLoop {
        receiver,
        block,
        items,
        next: 0,
    }))))
}

/// The block returned for one element. Its value is dropped, as `do:` always
/// dropped it, and the next element's activation takes its place on the
/// stack.
pub(crate) fn after_element<'gc>(state: Box<DoLoop<'gc>>) -> Step<'gc> {
    element_step(state)
}

/// The block's activation for the element at `state.next`, or the receiver
/// once there is none. Each activation is counted, like any block a `value`
/// send activates: its arguments are `StepArguments::Owned`, never the
/// `ControlFlow` mark.
fn element_step<'gc>(mut state: Box<DoLoop<'gc>>) -> Step<'gc> {
    let Some(&item) = state.items.get(state.next) else {
        return Step::Value(state.receiver);
    };
    state.next += 1;
    Step::Block {
        closure: state.block,
        arguments: StepArguments::Owned(vec![item]),
        then: Continuation::DoElement(state),
    }
}
