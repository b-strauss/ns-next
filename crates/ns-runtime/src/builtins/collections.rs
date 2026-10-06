//! `Array`'s leaves. An `Array` is fixed-size once made -- `new:` sizes it, a
//! literal fills it -- and nothing grows a payload in place; the growable
//! sequence is `List`, Newspeak in `CollectionsFactory.ns`, over arrays it
//! replaces when full.

use crate::builtins::primitive::CollectionPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// An index, size or count argument as a leaf reads it. A `LargeInteger`
/// is an Integer that no index, size or count can reach, so a leaf refuses
/// it in the words it refuses an out-of-range `SmallInteger`, never as
/// something that is no Integer.
pub(crate) enum IntegerArgument {
    Small(i64),
    Large { negative: bool },
}

pub(crate) fn integer_argument<'gc>(
    heap: &Heap<'gc>,
    argument: Option<Value<'gc>>,
) -> Option<IntegerArgument> {
    let argument = argument?;
    if let Some(number) = argument.as_small_integer() {
        return Some(IntegerArgument::Small(number));
    }
    heap.with_large_integer(argument.as_object()?, |negative, _| {
        IntegerArgument::Large { negative }
    })
}

pub(crate) fn out_of_range<'gc>(
    receiver: Value<'gc>,
    selector: Symbol,
    index: impl std::fmt::Display,
    length: usize,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver,
        message: format!(
            "index {index} out of range: valid indices are 1..{length} (size {length})"
        ),
    }
}

pub(crate) fn not_an_integer<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    got: Option<Value<'gc>>,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver,
        message: format!(
            "{selector} requires an Integer index, got {}",
            heap.describe_argument(got)
        ),
    }
}

/// `size`. `None` if `receiver` isn't `ObjectShape::Array` at all.
fn size<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    mutator
        .heap
        .array_element_count(object)
        .map(|length| Ok(Value::small_integer(length as i64)))
}

/// `at:` -- same payload as `size`.
fn element_at<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let length = mutator.heap.array_element_count(object)?;
    Some(
        match integer_argument(&mutator.heap, arguments.first().copied()) {
            Some(IntegerArgument::Small(index)) => {
                if index >= 1 && (index as usize) <= length {
                    Ok(mutator
                        .heap
                        .array_element(object, (index as usize) - 1)
                        .unwrap_or(Value::nil()))
                } else {
                    Err(out_of_range(receiver, selector, index, length))
                }
            }
            Some(IntegerArgument::Large { .. }) => Err(out_of_range(
                receiver,
                selector,
                mutator.heap.describe_argument(arguments.first().copied()),
                length,
            )),
            None => Err(not_an_integer(
                &mutator.heap,
                receiver,
                selector,
                arguments.first().copied(),
            )),
        },
    )
}

/// `at:put:`: stores the value and answers it.
fn element_at_put<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let length = mutator.heap.array_element_count(object)?;
    let index = match integer_argument(&mutator.heap, arguments.first().copied()) {
        Some(IntegerArgument::Small(index)) => index,
        Some(IntegerArgument::Large { .. }) => {
            return Some(Err(out_of_range(
                receiver,
                selector,
                mutator.heap.describe_argument(arguments.first().copied()),
                length,
            )));
        }
        None => {
            return Some(Err(not_an_integer(
                &mutator.heap,
                receiver,
                selector,
                arguments.first().copied(),
            )));
        }
    };
    if index < 1 || (index as usize) > length {
        return Some(Err(out_of_range(receiver, selector, index, length)));
    }
    let value = arguments.get(1).copied().unwrap_or(Value::nil());
    mutator
        .heap
        .array_set_element(object, (index as usize) - 1, value);
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
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Array) {
        return None;
    }
    let refusal = |message: String| NewspeakError {
        selector,
        receiver,
        message,
    };
    let size = match integer_argument(&mutator.heap, arguments.first().copied()) {
        Some(IntegerArgument::Small(size)) => size,
        Some(IntegerArgument::Large { negative }) => {
            let described = mutator.heap.describe_argument(arguments.first().copied());
            return Some(Err(refusal(if negative {
                format!("{selector} requires a size of at least 0, got {described}")
            } else {
                format!("{selector} cannot allocate an Array of {described} elements")
            })));
        }
        None => {
            return Some(Err(refusal(format!(
                "{selector} requires an Integer size, got {}",
                mutator.heap.describe_argument(arguments.first().copied())
            ))));
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
    elements.resize(length, Value::nil());
    Some(Ok(Value::object(mutator.new_array(elements))))
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

/// `do:` on an `Array`: what runs for each element in turn, as trampoline
/// steps -- the first here, each next one by `after_element` when the one
/// before it returns -- so no Rust frame waits on Newspeak code
/// (`docs/design/execution.md`, "No re-entry from Rust").
/// `None` when `receiver` is no `ObjectShape::Array`, so the send
/// falls through to `doesNotUnderstand:` as every other selector here does.
fn do_step<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let Some(action) = arguments.first().copied() else {
        return Some(Err(Signal::Error(NewspeakError {
            selector,
            receiver,
            message: "do: requires a block argument".to_string(),
        })));
    };
    // No element, nothing to run or send: the loop is over before it starts,
    // so it needs no state either.
    if size == 0 {
        return Some(Ok(Step::Value(receiver)));
    }
    let action = LoopAction::of(&mutator.heap, action);
    // A future stands for its value, which may well be a block: declining
    // hands it to the caller's failure path, which sees through it.
    if let LoopAction::Sent(sent) = action {
        if mutator.as_future(sent).is_some() {
            return None;
        }
    }
    Some(Ok(element_step(
        mutator,
        Box::new(DoLoop {
            receiver: object,
            action,
            size,
            next: 0,
        }),
    )))
}

/// The action returned for one element. Its value is dropped, as `do:`
/// always dropped it, and what runs for the next element takes its place on
/// the stack.
pub(crate) fn after_element<'gc>(mutator: &mut Mutator<'gc>, state: Box<DoLoop<'gc>>) -> Step<'gc> {
    element_step(mutator, state)
}

/// What runs for the element at `state.next`, read now, or the array once
/// the walk has reached the size it read at the start. A closure is
/// activated as `value:` activates it, counted -- its arguments are
/// `StepArguments::Owned`, never the `ControlFlow` mark; anything else is
/// sent `value:`.
fn element_step<'gc>(mutator: &mut Mutator<'gc>, mut state: Box<DoLoop<'gc>>) -> Step<'gc> {
    let receiver = Value::object(state.receiver);
    if state.next >= state.size {
        return Step::Value(receiver);
    }
    // Present while an array's size is fixed; a receiver whose shape has
    // changed under the walk ends it rather than failing.
    let Some(element) = mutator.heap.array_element(state.receiver, state.next) else {
        return Step::Value(receiver);
    };
    state.next += 1;
    match state.action {
        LoopAction::Closure(closure) => Step::Block {
            closure,
            arguments: StepArguments::Owned(vec![element]),
            then: Continuation::DoElement(state),
        },
        LoopAction::Sent(action) => mutator.send_value_then(
            receiver,
            action,
            Some(element),
            Continuation::DoElement(state),
        ),
    }
}
