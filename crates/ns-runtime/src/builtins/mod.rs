//! The native ("primitive") methods of the core library classes: one module
//! per `BuiltinClass` family -- `object_class` for `Object` itself, `value`
//! for what §3.1.1's value objects derive from their state, `behavior` for
//! what §3.2's `Class` and `Metaclass` forward to, then `numbers`, `text`,
//! `collections` and `block`, `activation` for the exceptions
//! `Kernel.ns` builds over the running activations, `message` for
//! `Message>>sentTo:`, `module` for `moduleNamed:`, through which
//! shipped source reaches another shipped module, and `future` for the
//! forcing of §6.3.2's pull future. `primitive` names every
//! one of them as a `Primitive` and lists them as the rows of the VM
//! mirror, the one builtin
//! whose method table holds primitive entries (`dispatch::Lookup::Primitive`);
//! every other builtin's methods are `Kernel.ns`'s, which forward to those
//! rows. `execute` runs one leaf.
pub mod activation;
pub mod behavior;
pub mod block;
pub mod collections;
pub mod future;
pub mod message;
pub mod module;
pub mod numbers;
pub mod object_class;
pub mod primitive;
pub mod text;
pub mod value;

use primitive::Primitive;

use crate::interpreter::future::{see_through, SeeThrough};
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::{platform, Mutator, Symbol};

/// Runs `primitive` on `receiver` (`execute_leaf`), seeing through a future
/// among its operands where the leaf could not: when it declined, raised, or
/// -- for a leaf that answers `false` to a stranger -- answered `false`
/// (`interpreter::future::see_through`). A program that never made a future
/// pays one flag test, and only on those paths.
///
/// This rests on an invariant every leaf keeps: **a leaf that declines or
/// raises has changed nothing** -- it checks its operands before it writes
/// anything -- so running it again, or running the instruction that sent it
/// again, is running it once (`docs/design/dispatch.md`, "A primitive can
/// fail").
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let answer = execute_leaf(mutator, primitive, receiver, selector, arguments);
    let unanswered = matches!(
        answer,
        None | Some(Err(Signal::Error(_))) | Some(Ok(Step::Value(Value::Boolean(false))))
    );
    if !unanswered || !mutator.futures_made {
        return answer;
    }
    see_through_after(mutator, primitive, receiver, selector, arguments, answer)
}

/// `execute`'s failure path once a future has been made: whether a future
/// among the operands is why the leaf could not answer. Out of line, so that
/// the path every answered leaf takes stays as short as it was.
#[cold]
#[inline(never)]
fn see_through_after<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    answer: Option<Result<Step<'gc>, Signal<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let consulted = match &answer {
        Some(Ok(_)) => primitive.answers_false_to_a_stranger(),
        // A mirror row reaches its own leaf through `execute_sent`, which ran
        // this function already.
        _ => !matches!(primitive, Primitive::Mirror(_)),
    };
    if !consulted {
        return answer;
    }
    match see_through(mutator, primitive, receiver, selector, arguments) {
        SeeThrough::NoFuture => answer,
        SeeThrough::Answer(seen) => seen,
    }
}

/// Runs `primitive` on `receiver`. `None` means a leaf declines: the
/// receiver is not of the shape it works on, and the answer goes back to the
/// mirror code, which runs the failure block (`interpreter::mirror`) -- or,
/// for a kernel primitive method, to `act`, which then activates the method,
/// whose body is that failure path (`docs/design/dispatch.md`, "Primitive
/// methods"). `selector` is the send's, for the error texts
/// that name it. No primitive calls a block: the control-flow ones, `value`
/// and `do:` answer a `Step` (`docs/design/execution.md`, "The trampoline").
pub(crate) fn execute_leaf<'gc>(
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
        Primitive::Behavior(primitive) => {
            behavior::execute(mutator, primitive, receiver, arguments).map(wrap_error)
        }
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
        Primitive::Collection(primitive) => {
            collections::execute(mutator, primitive, receiver, selector, arguments)
        }
        Primitive::Block(primitive) => block::execute(mutator, primitive, receiver, arguments),
        Primitive::Platform(primitive) => {
            platform::execute(mutator, primitive, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Activation(primitive) => {
            activation::execute(mutator, primitive, receiver, arguments)
        }
        Primitive::Message(primitive) => message::execute(mutator, primitive, receiver, arguments),
        Primitive::Module(primitive) => {
            module::execute(mutator, primitive, receiver).map(wrap_error)
        }
        Primitive::Future(primitive) => future::execute(mutator, primitive, receiver),
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
