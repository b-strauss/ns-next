//! `Array`'s leaves. An `Array` is fixed-size once made -- `new:` sizes it, a
//! literal fills it -- and nothing grows a payload in place; the growable
//! sequence is `List`, Newspeak in `CollectionsFactory.ns`, over arrays it
//! replaces when full.

use crate::builtins::primitive::CollectionPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

pub(crate) fn out_of_range<'gc>(
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

pub(crate) fn not_an_integer<'gc>(
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

/// `size`. `None` if `receiver` isn't `ObjectData::Array`-backed at all.
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

/// `at:` -- same payload as `size`.
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

/// `at:put:`.
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
    if size < 0 {
        return Some(Err(refusal(format!(
            "{selector} requires a size of at least 0, got {size}"
        ))));
    }
    // A non-negative size that does not fit a `usize` -- above `u32::MAX` on
    // a 32-bit target -- is refused as an allocation that cannot be made,
    // like one `try_reserve_exact` refuses, not as a negative size.
    let cannot_allocate = || {
        refusal(format!(
            "{selector} cannot allocate an Array of {size} elements"
        ))
    };
    let Ok(length) = usize::try_from(size) else {
        return Some(Err(cannot_allocate()));
    };
    let mut elements: Vec<Value<'gc>> = Vec::new();
    if elements.try_reserve_exact(length).is_err() {
        return Some(Err(cannot_allocate()));
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

/// `Array`'s native methods.
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
        CollectionPrimitive::Allocate => {
            leaf_step(allocate(mutator, receiver, selector, arguments))
        }
    }
}

/// `do:` on an `Array`: the block activated for each element in turn, as
/// trampoline steps -- the first here, each next one by
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
        // A future stands for its value, which may well be a block: taking
        // it for a non-block would skip every element. Declining hands it
        // to its caller's failure path, which sees through it.
        if mutator.as_future(Value::Object(block)).is_some() {
            return None;
        }
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
