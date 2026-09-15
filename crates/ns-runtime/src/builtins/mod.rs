//! Native ("primitive") method dispatch for the core library classes: one
//! module per `BuiltinClass` family -- `object_class` for `Object` itself,
//! then `boolean`, `numbers`, `text`, `collections` and `block`. `primitive`
//! names every one of them as a `Primitive`, which `execute` runs; `selector`
//! selects one only through the temporary bridge, `dispatch_native`.
pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod primitive;
pub mod text;

use primitive::Primitive;

use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::{platform, Mutator, Symbol};

/// Runs `primitive` on `receiver`. `None` is a primitive failure: the
/// receiver is not of the shape the primitive works on (`docs/design/dispatch.md`,
/// "A primitive can fail"). `selector` is the send's, for the error texts
/// that name it. No primitive calls a block: the control-flow ones, `value`
/// and `do:` answer a `Step` (`docs/design/execution.md`, "The trampoline").
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        Primitive::Object(primitive) => {
            object_class::execute(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Boolean(primitive) => boolean::execute(mutator, primitive, receiver, arguments),
        Primitive::Integer(primitive) => {
            numbers::execute_integer(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Float(primitive) => {
            numbers::execute_float(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::String(primitive) => {
            text::execute_string(mutator, primitive, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Symbol(primitive) => {
            text::execute_symbol(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Character(primitive) => {
            text::execute_character(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Collection(primitive) => {
            collections::execute(mutator, primitive, receiver, selector, arguments)
        }
        Primitive::Block(primitive) => block::execute(mutator, primitive, receiver, arguments),
        Primitive::Platform(primitive) => {
            platform::execute(mutator, primitive, receiver, arguments).map(wrap_error)
        }
    }
}

/// Temporary, until lookup finds primitives in the method tables (step 3a,
/// Task 3): the primitive `kind` lists for `selector`, run.
pub(crate) fn dispatch_native<'gc>(
    mutator: &mut Mutator<'gc>,
    kind: BuiltinClass,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let primitive = primitive::primitives_of(kind)
        .iter()
        .find(|entry| entry.selector == selector.as_str())?
        .primitive;
    execute(mutator, primitive, receiver, selector, arguments)
}

/// Lifts a leaf native's result into the step it is.
fn wrap_error<'gc>(
    result: Result<Value<'gc>, NewspeakError<'gc>>,
) -> Result<Step<'gc>, Signal<'gc>> {
    result.map(Step::Value).map_err(Signal::Error)
}
