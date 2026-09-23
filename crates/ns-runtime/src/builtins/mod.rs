//! The native ("primitive") methods of the core library classes: one module
//! per `BuiltinClass` family -- `object_class` for `Object` itself, `value`
//! for what §3.1.1's value objects derive from their state, then
//! `boolean`, `numbers`, `text`, `collections` and `block`. `primitive` names
//! every one of them as a `Primitive` and lists them as the rows of the VM
//! mirror, the one builtin whose method table holds primitive entries
//! (`dispatch::Lookup::Primitive`); every other builtin's methods are
//! `Kernel.ns`'s, which forward to those rows. `execute` runs one leaf.
pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod primitive;
pub mod text;
pub mod value;

use primitive::Primitive;

use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::{platform, Mutator, Symbol};

/// Runs `primitive` on `receiver`. `None` means a leaf declines: the
/// receiver is not of the shape it works on, and the answer goes back to the
/// mirror code, which runs the failure block (`interpreter::mirror`) -- or,
/// for a kernel primitive method, to `act`, which then activates the method,
/// whose body is that failure path (`docs/design/dispatch.md`, "Primitive
/// methods"). `selector` is the send's, for the error texts
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
        Primitive::Value(primitive) => {
            value::execute(mutator, primitive, receiver, selector, arguments).map(wrap_error)
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
        // Always `Some`: a leaf that declines here runs the failure block
        // the send carries, so a mirror row never answers `None`.
        Primitive::Mirror(row) => Some(crate::interpreter::mirror::execute_sent(
            mutator, row, arguments,
        )),
    }
}

/// Lifts a leaf native's result into the step it is.
fn wrap_error<'gc>(
    result: Result<Value<'gc>, NewspeakError<'gc>>,
) -> Result<Step<'gc>, Signal<'gc>> {
    result.map(Step::Value).map_err(Signal::Error)
}
