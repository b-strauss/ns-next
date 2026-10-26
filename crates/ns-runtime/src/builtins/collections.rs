//! `Array`'s leaves. An `Array` is fixed-size once made -- `new:` sizes it, a
//! literal fills it -- and nothing grows a payload in place; the growable
//! sequence is `List`, Newspeak in `CollectionsFactory.ns`, over arrays it
//! replaces when full.

use crate::builtins::first_argument;
use crate::builtins::primitive::CollectionPrimitive;
use crate::interpreter::Arguments;
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
    argument: Value<'gc>,
    length: usize,
) -> NewspeakError<'gc> {
    NewspeakError::argument_error(
        selector,
        receiver,
        argument,
        format!("index {index} out of range: valid indices are 1..{length} (size {length})"),
    )
}

pub(crate) fn not_an_integer<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    got: Option<Value<'gc>>,
) -> NewspeakError<'gc> {
    NewspeakError::argument_error(
        selector,
        receiver,
        got.unwrap_or(Value::nil()),
        format!(
            "{selector} requires an Integer index, got {}",
            heap.describe_argument(got)
        ),
    )
}

/// The zero-based positions `start - 1 .. stop` of a range `copyFrom:to:`
/// or `replaceFrom:to:` names in a sequence of `size` elements, or the
/// `ArgumentError` it signals: one carrying the bound out of range -- the
/// start unless it is a position the sequence has, one past the end
/// included for the empty range, then the stop -- and the text
/// "<keyword> … to: … out of range: valid ranges lie within 1..size, an
/// empty range ends just before it starts (size …)", or the text of a
/// bound that is no Integer.
pub(crate) fn checked_range<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    keyword: &str,
    start: Option<Value<'gc>>,
    stop: Option<Value<'gc>>,
    size: usize,
) -> Result<std::ops::Range<usize>, NewspeakError<'gc>> {
    // A `LargeInteger` bound is past every position, so it takes the
    // out-of-range error below, described as it is written.
    let bound = |argument: Option<Value<'gc>>| match integer_argument(heap, argument) {
        Some(IntegerArgument::Small(bound)) => Ok(Some(bound)),
        Some(IntegerArgument::Large { .. }) => Ok(None),
        None => Err(not_an_integer(heap, receiver, selector, argument)),
    };
    let start_bound = bound(start)?;
    let stop_bound = bound(stop)?;
    // `start` is tested first, so that `start - 1` cannot overflow.
    if let (Some(first), Some(last)) = (start_bound, stop_bound) {
        if first >= 1 && last >= first - 1 {
            if let (Ok(first), Ok(last)) = (usize::try_from(first), usize::try_from(last)) {
                if last <= size {
                    return Ok(first - 1..last);
                }
            }
        }
    }
    let describe = |bound: Option<i64>, argument: Option<Value<'gc>>| match bound {
        Some(number) => number.to_string(),
        None => heap.describe_argument(argument),
    };
    let start_in_range =
        matches!(start_bound, Some(position) if position >= 1 && position - 1 <= size as i64);
    let refused = if start_in_range { stop } else { start };
    Err(NewspeakError::argument_error(
        selector,
        receiver,
        refused.unwrap_or(Value::nil()),
        format!(
            "{keyword} {} to: {} out of range: valid ranges lie within \
             1..{size}, an empty range ends just before it starts (size {size})",
            describe(start_bound, start),
            describe(stop_bound, stop),
        ),
    ))
}

/// The zero-based position `startingAt:` names in a source of
/// `source_size` elements, from which `count` elements are read, or the
/// `ArgumentError` carrying the `startingAt:` argument: the text of a start
/// that is no Integer, or "startingAt: … out of range: the source holds …
/// elements, the range needs … from …". An empty range takes any start
/// from 1 to one past the source's end.
pub(crate) fn source_start<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    start: Option<Value<'gc>>,
    count: usize,
    source_size: usize,
) -> Result<usize, NewspeakError<'gc>> {
    let position = match integer_argument(heap, start) {
        Some(IntegerArgument::Small(position)) => Some(position),
        Some(IntegerArgument::Large { .. }) => None,
        None => return Err(not_an_integer(heap, receiver, selector, start)),
    };
    if let Some(Ok(first)) = position.map(usize::try_from) {
        if first >= 1
            && (first - 1)
                .checked_add(count)
                .is_some_and(|end| end <= source_size)
        {
            return Ok(first - 1);
        }
    }
    let described = match position {
        Some(number) => number.to_string(),
        None => heap.describe_argument(start),
    };
    let elements = if source_size == 1 {
        "element"
    } else {
        "elements"
    };
    Err(NewspeakError::argument_error(
        selector,
        receiver,
        start.unwrap_or(Value::nil()),
        format!(
            "startingAt: {described} out of range: the source holds {source_size} \
             {elements}, the range needs {count} from {described}"
        ),
    ))
}

/// `copyFrom:to:` on an `Array`: a new Array of the elements in the range.
/// `None` when the receiver is no Array.
fn copy_range<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "copyFrom:",
        arguments.first(&mutator.heap),
        arguments.try_get(&mutator.heap, 1),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let elements: Vec<Value<'gc>> = range
        .filter_map(|position| mutator.heap.array_element(object, position))
        .collect();
    Some(Ok(Value::object(mutator.new_array(elements))))
}

/// `replaceFrom:to:with:startingAt:` on an `Array` from an `Array`:
/// stores the source's elements from `startingAt` on into the range,
/// through the write barrier, and answers the receiver. `None` -- the
/// Newspeak fallback runs, which copies any other source into an Array --
/// when the receiver or the source is no Array.
fn replace<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let source = arguments.try_get(&mutator.heap, 2)?.as_object()?;
    let source_size = mutator.heap.array_element_count(source)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "replaceFrom:",
        arguments.first(&mutator.heap),
        arguments.try_get(&mutator.heap, 1),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let first = match source_start(
        &mutator.heap,
        receiver,
        selector,
        arguments.try_get(&mutator.heap, 3),
        range.len(),
        source_size,
    ) {
        Ok(first) => first,
        Err(error) => return Some(Err(error)),
    };
    // Every element is read before any is stored, so overlapping ranges of
    // one array copy as if through a buffer, in either direction.
    let elements: Vec<Value<'gc>> = (first..first + range.len())
        .filter_map(|position| mutator.heap.array_element(source, position))
        .collect();
    for (position, element) in range.zip(elements) {
        mutator.heap.array_set_element(object, position, element);
    }
    Some(Ok(receiver))
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
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let length = mutator.heap.array_element_count(object)?;
    Some(
        match integer_argument(&mutator.heap, arguments.first(&mutator.heap)) {
            Some(IntegerArgument::Small(index)) => {
                if index >= 1 && (index as usize) <= length {
                    Ok(mutator
                        .heap
                        .array_element(object, (index as usize) - 1)
                        .unwrap_or(Value::nil()))
                } else {
                    Err(out_of_range(
                        receiver,
                        selector,
                        index,
                        first_argument(&mutator.heap, arguments),
                        length,
                    ))
                }
            }
            Some(IntegerArgument::Large { .. }) => Err(out_of_range(
                receiver,
                selector,
                mutator
                    .heap
                    .describe_argument(arguments.first(&mutator.heap)),
                first_argument(&mutator.heap, arguments),
                length,
            )),
            None => Err(not_an_integer(
                &mutator.heap,
                receiver,
                selector,
                arguments.first(&mutator.heap),
            )),
        },
    )
}

/// `at:put:`: stores the value and answers it.
fn element_at_put<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let length = mutator.heap.array_element_count(object)?;
    let index = match integer_argument(&mutator.heap, arguments.first(&mutator.heap)) {
        Some(IntegerArgument::Small(index)) => index,
        Some(IntegerArgument::Large { .. }) => {
            return Some(Err(out_of_range(
                receiver,
                selector,
                mutator
                    .heap
                    .describe_argument(arguments.first(&mutator.heap)),
                first_argument(&mutator.heap, arguments),
                length,
            )));
        }
        None => {
            return Some(Err(not_an_integer(
                &mutator.heap,
                receiver,
                selector,
                arguments.first(&mutator.heap),
            )));
        }
    };
    if index < 1 || (index as usize) > length {
        return Some(Err(out_of_range(
            receiver,
            selector,
            index,
            first_argument(&mutator.heap, arguments),
            length,
        )));
    }
    let value = arguments.try_get(&mutator.heap, 1).unwrap_or(Value::nil());
    mutator
        .heap
        .array_set_element(object, (index as usize) - 1, value);
    Some(Ok(value))
}

/// `Array new: size`: an array of `size` `nil`s. Its subject is the class
/// the forwarder runs in, which is only ever `Array` itself -- §3.2's
/// metaclasses are flat, so no subclass inherits `new:` -- and anything
/// else declines. A size that cannot be allocated is an error, found by
/// `Heap::array_fits` before anything is allocated, never an abort: the size
/// is the program's, and an abort is not catchable.
fn allocate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Array) {
        return None;
    }
    let refusal = |message: String| {
        NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            message,
        )
    };
    let size = match integer_argument(&mutator.heap, arguments.first(&mutator.heap)) {
        Some(IntegerArgument::Small(size)) => size,
        Some(IntegerArgument::Large { negative }) => {
            let described = mutator
                .heap
                .describe_argument(arguments.first(&mutator.heap));
            return Some(Err(refusal(if negative {
                format!("{selector} requires a size of at least 0, got {described}")
            } else {
                format!("{selector} cannot allocate an Array of {described} elements")
            })));
        }
        None => {
            return Some(Err(refusal(format!(
                "{selector} requires an Integer size, got {}",
                mutator
                    .heap
                    .describe_argument(arguments.first(&mutator.heap))
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
    // like one `array_fits` refuses, not as a negative size.
    let cannot_allocate = || {
        refusal(format!(
            "{selector} cannot allocate an Array of {size} elements"
        ))
    };
    let Ok(length) = usize::try_from(size) else {
        return Some(Err(cannot_allocate()));
    };
    if !mutator.heap.array_fits(length) {
        return Some(Err(cannot_allocate()));
    }
    Some(Ok(Value::object(
        mutator.heap.allocate_nil_array(class, length),
    )))
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
    arguments: Arguments<'_, 'gc>,
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
        CollectionPrimitive::CopyRange => {
            leaf_step(copy_range(mutator, receiver, selector, arguments))
        }
        CollectionPrimitive::Replace => leaf_step(replace(mutator, receiver, selector, arguments)),
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
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.array_element_count(object)?;
    let Some(action) = arguments.first(&mutator.heap) else {
        return Some(Err(Signal::Error(NewspeakError::argument_error(
            selector,
            receiver,
            Value::nil(),
            "do: requires a block argument",
        ))));
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
