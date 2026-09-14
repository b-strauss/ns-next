//! Native ("primitive") method dispatch for the core library classes: one
//! module per `BuiltinClass` family -- `object_class` for `Object` itself,
//! then `boolean`, `numbers`, `text`, `collections` and `block`.
pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod text;

use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::{platform, Mutator, Symbol};

/// `None` = this builtin does not implement `selector` (let the caller fall
/// through to `doesNotUnderstand:`). `Some(Err(Signal::Error(_)))` = a
/// genuine Newspeak-level error (e.g. arithmetic overflow).
///
/// No native calls a block. `boolean::dispatch`, `block::dispatch` and
/// `collections::dispatch_array`/`dispatch_ordered_collection` answer a
/// `Step` themselves: the control-flow natives, `value` and `do:` return the
/// block to activate instead of calling it (`docs/design/execution.md`, "The
/// trampoline"), so a `^` out of that block unwinds the activation stack
/// like any other and never passes through a native -- `collections` checks
/// for `do:` itself and answers a step for it, a leaf (wrapped by its own
/// `leaf_step`) for every other selector. Every other native here stays a
/// plain leaf: it keeps a `Result<Value, NewspeakError>` signature, wrapped
/// into the step it is right here, by `wrap_error`.
pub(crate) fn dispatch_native(
    mutator: &mut Mutator,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Step, Signal>> {
    match kind {
        BuiltinClass::Object => {
            object_class::dispatch(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => {
            boolean::dispatch(mutator, receiver, selector, arguments)
        }
        BuiltinClass::Integer => {
            numbers::dispatch_integer(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::Float => {
            numbers::dispatch_float(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::String => {
            text::dispatch_string(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::Symbol => {
            text::dispatch_symbol(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::Character => {
            text::dispatch_character(mutator, receiver, selector, arguments).map(wrap_error)
        }
        BuiltinClass::Array => collections::dispatch_array(mutator, receiver, selector, arguments),
        BuiltinClass::OrderedCollection => {
            collections::dispatch_ordered_collection(mutator, receiver, selector, arguments)
        }
        BuiltinClass::Block => block::dispatch(mutator, receiver, selector, arguments),
        BuiltinClass::Platform => {
            platform::dispatch(mutator, receiver, selector, arguments).map(wrap_error)
        }
        _ => None,
    }
}

/// Lifts a leaf native's result into the step it is.
fn wrap_error(result: Result<Value, NewspeakError>) -> Result<Step, Signal> {
    result.map(Step::Value).map_err(Signal::Error)
}
