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

use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::{Heap, Interpreter, Symbol};

fn out_of_range(receiver: &Value, selector: Symbol, index: i64, len: usize) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!("index {index} out of range: valid indices are 1..{len} (size {len})"),
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
            heap.describe_arg(got)
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
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    match selector.as_str() {
        "size" => interp
            .heap
            .array_len(*obj)
            .map(|len| Ok(Value::SmallInt(len as i64))),
        "at:" => {
            let len = interp.heap.array_len(*obj)?;
            Some(match args.first() {
                Some(Value::SmallInt(index)) => {
                    if *index >= 1 && (*index as usize) <= len {
                        Ok(interp
                            .heap
                            .array_at(*obj, (*index as usize) - 1)
                            .unwrap_or(Value::Nil))
                    } else {
                        Err(out_of_range(&receiver, selector, *index, len))
                    }
                }
                other => Err(not_an_integer(&interp.heap, &receiver, selector, other)),
            })
        }
        _ => None,
    }
}

/// `at:put:`, falling through to `dispatch_common` -- `dispatch_array`'s leaf
/// half, kept apart from its `do:` arm so the two compose in `dispatch_array`
/// without either copying the other's shape.
fn array_leaf(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "at:put:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        let len = interp.heap.array_len(*obj)?;
        let index = match args.first() {
            Some(Value::SmallInt(index)) => *index,
            other => {
                return Some(Err(not_an_integer(
                    &interp.heap,
                    &receiver,
                    selector,
                    other,
                )))
            }
        };
        if index < 1 || (index as usize) > len {
            return Some(Err(out_of_range(&receiver, selector, index, len)));
        }
        let value = args.get(1).copied().unwrap_or(Value::Nil);
        interp.heap.array_set(*obj, (index as usize) - 1, value);
        return Some(Ok(receiver));
    }
    dispatch_common(interp, receiver, selector, args)
}

/// `add:`, falling through to `dispatch_common` -- `dispatch_ordered_collection`'s
/// leaf half, kept apart from its `do:` arm for the same reason `array_leaf` is.
fn ordered_collection_leaf(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "add:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        interp.heap.array_len(*obj)?;
        // Matches the reference library's convention: `add:` returns the
        // argument, not the receiver.
        let value = args.first().copied().unwrap_or(Value::Nil);
        interp.heap.array_push(*obj, value);
        return Some(Ok(value));
    }
    dispatch_common(interp, receiver, selector, args)
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
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    if selector.as_str() == "do:" {
        return do_step(interp, receiver, selector, args);
    }
    leaf_step(array_leaf(interp, receiver, selector, args))
}

/// `OrderedCollection`'s native methods, `do:` included -- same split as
/// `dispatch_array`.
pub(crate) fn dispatch_ordered_collection(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    if selector.as_str() == "do:" {
        return do_step(interp, receiver, selector, args);
    }
    leaf_step(ordered_collection_leaf(interp, receiver, selector, args))
}

/// `do:` on an `Array` or an `OrderedCollection`: the block activated for each
/// element in turn, as trampoline steps -- the first here, each next one by
/// `after_element` when the one before it returns -- so no Rust frame waits
/// on Newspeak code (`docs/design/execution.md`, "No re-entry from Rust").
/// `None` when `receiver` has no `ObjectData::Array` payload, so the send
/// falls through to `doesNotUnderstand:` as every other selector here does.
fn do_step(
    interp: &Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let items = interp.heap.array_elements(*obj)?;
    let Some(block) = args.first().copied() else {
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
    let closure = match block {
        Value::Obj(block) => interp.heap.closure_data(block),
        _ => None,
    };
    let Some(block) = closure else {
        return Some(Ok(Step::Value(receiver)));
    };
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
/// send activates: its arguments are `StepArgs::Owned`, never the
/// `ControlFlow` mark.
fn element_step(mut state: Box<DoLoop>) -> Step {
    let Some(&item) = state.items.get(state.next) else {
        return Step::Value(state.receiver);
    };
    state.next += 1;
    Step::Block {
        closure: state.block.clone(),
        args: StepArgs::Owned(vec![item]),
        then: Continuation::DoElement(state),
    }
}
