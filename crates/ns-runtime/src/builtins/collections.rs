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

use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

fn out_of_range(receiver: &Value, selector: Symbol, index: i64, length: usize) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "index {index} out of range: valid indices are 1..{length} (size {length})"
        ),
    }
}

fn not_an_integer(
    heap: &Heap,
    receiver: &Value,
    selector: Symbol,
    got: Option<&Value>,
) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "{selector} requires an Integer index, got {}",
            heap.describe_argument(got)
        ),
    }
}

/// `size` and `at:` -- shared by `Array` and `OrderedCollection` since both
/// share the `ObjectData::Array` payload (see this module's design note).
/// Returns `None` if `receiver` isn't `ObjectData::Array`-backed at all, or
/// if `selector` isn't one of these two (letting the caller fall through to
/// `doesNotUnderstand:`, or to the class-specific selector handled by
/// `dispatch_array`/`dispatch_ordered_collection` itself). `do:` is not a
/// leaf and is not here: it activates a block, so it answers a step
/// (`do_step`).
fn dispatch_common(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    match selector.as_str() {
        "size" => mutator
            .heap
            .array_length(*object)
            .map(|length| Ok(Value::SmallInteger(length as i64))),
        "at:" => {
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
        _ => None,
    }
}

/// `at:put:`, falling through to `dispatch_common` -- `dispatch_array`'s leaf
/// half, kept apart from its `do:` arm so the two compose in `dispatch_array`
/// without either copying the other's shape.
fn array_leaf(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "at:put:" {
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
        return Some(Ok(receiver));
    }
    dispatch_common(mutator, receiver, selector, arguments)
}

/// `add:`, falling through to `dispatch_common` -- `dispatch_ordered_collection`'s
/// leaf half, kept apart from its `do:` arm for the same reason `array_leaf` is.
fn ordered_collection_leaf(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "add:" {
        let Value::Object(object) = &receiver else {
            return None;
        };
        mutator.heap.array_length(*object)?;
        // Matches the reference library's convention: `add:` returns the
        // argument, not the receiver.
        let value = arguments.first().copied().unwrap_or(Value::Nil);
        mutator.heap.array_push(*object, value);
        return Some(Ok(value));
    }
    dispatch_common(mutator, receiver, selector, arguments)
}

/// Lifts a leaf's result into the step it is -- the shared tail of
/// `dispatch_array` and `dispatch_ordered_collection` once each has ruled
/// `do:` out, so neither copies the wrapping itself.
fn leaf_step(result: Option<Result<Value, NewspeakError>>) -> Option<Result<Step, Signal>> {
    Some(result?.map(Step::Value).map_err(Signal::Error))
}

/// `Array`'s native methods, `do:` included: checks `do:` first and answers
/// `do_step`'s block-activation step for it, `array_leaf`'s plain value or
/// error (through `leaf_step`) for everything else.
pub(crate) fn dispatch_array(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Step, Signal>> {
    if selector.as_str() == "do:" {
        return do_step(mutator, receiver, selector, arguments);
    }
    leaf_step(array_leaf(mutator, receiver, selector, arguments))
}

/// `OrderedCollection`'s native methods, `do:` included -- same split as
/// `dispatch_array`.
pub(crate) fn dispatch_ordered_collection(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Step, Signal>> {
    if selector.as_str() == "do:" {
        return do_step(mutator, receiver, selector, arguments);
    }
    leaf_step(ordered_collection_leaf(
        mutator, receiver, selector, arguments,
    ))
}

/// `do:` on an `Array` or an `OrderedCollection`: the block activated for each
/// element in turn, as trampoline steps -- the first here, each next one by
/// `after_element` when the one before it returns -- so no Rust frame waits
/// on Newspeak code (`docs/design/execution.md`, "No re-entry from Rust").
/// `None` when `receiver` has no `ObjectData::Array` payload, so the send
/// falls through to `doesNotUnderstand:` as every other selector here does.
fn do_step(
    mutator: &Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Step, Signal>> {
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
pub(crate) fn after_element(state: Box<DoLoop>) -> Step {
    element_step(state)
}

/// The block's activation for the element at `state.next`, or the receiver
/// once there is none. Each activation is counted, like any block a `value`
/// send activates: its arguments are `StepArguments::Owned`, never the
/// `ControlFlow` mark.
fn element_step(mut state: Box<DoLoop>) -> Step {
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
