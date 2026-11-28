//! The native ("primitive") methods of the core library classes: one module
//! per `BuiltinClass` family -- `object_class` for `Object` itself, `value`
//! for what §3.1.1's value objects derive from their state, `behavior` for
//! what §3.2's `Class` and `Metaclass` forward to, then `numbers`, `text`,
//! `collections`, `bytes` for `ByteArray`, `elements` for the typed arrays'
//! and `DataView`'s elements on one, and `block`, `activation` for the exceptions
//! `Kernel.ns` builds over the running activations, `message` for
//! `Message>>sendTo:`, `module` for `moduleInstanceNamed:`, through which
//! shipped source reaches another shipped module, and `future` for the
//! forcing of §6.3.2's pull future, `weak` for `WeakArray`, the
//! full collection a program asks for and `Ephemeron`'s check of a key,
//! and `reflection` for what the shipped mirrors module reads.
//! `primitive` names every
//! one of them as a `Primitive` and lists them as the rows of the VM
//! mirror, the one builtin
//! whose method table holds primitive entries (`dispatch::Lookup::Primitive`);
//! every other builtin's methods are `Kernel.ns`'s, which forward to those
//! rows. `execute` runs one leaf.
pub mod activation;
pub mod behavior;
pub mod block;
pub mod bytes;
pub mod collections;
pub mod elements;
pub mod future;
pub mod message;
pub mod module;
pub mod numbers;
pub mod object_class;
pub mod primitive;
pub mod reflection;
pub mod text;
pub mod value;
pub mod weak;

use crate::interpreter::Arguments;
use primitive::Primitive;

use crate::heap::Heap;
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::{Mutator, Symbol, platform};

/// Runs `primitive` on `receiver`. `None` means a leaf declines: the
/// receiver is not of the shape it works on, and the answer goes back to the
/// mirror code, which runs the failure block (`interpreter::mirror`) -- or,
/// for a kernel primitive method, to `act`, which then activates the method,
/// whose body is that failure path (`docs/design/dispatch.md`, "Primitive
/// methods"). `selector` is the send's, for the error texts
/// that name it. No primitive calls a block: the control-flow ones, `value`
/// and `do:` answer a `Step` (`docs/design/execution.md`, "The trampoline").
///
/// A leaf that declines or raises has changed nothing -- it checks its
/// operands before it writes anything -- so running it again, or running
/// again the instruction that sent it, is running it once. The callers'
/// failure paths rest on that when they see through a future among the
/// operands (`interpreter::future::after_a_failed_leaf`,
/// `docs/design/dispatch.md`, "Primitives").
///
/// Every leaf enters this function, so its prologue and epilogue are paid
/// by the hottest ones. Fat LTO inlines the groups' leaves into it, and
/// under Windows' calling convention a function saves and restores each of
/// `xmm6`..`xmm15` it touches: with LLVM 23, an `Array`'s vectorized
/// `copyFrom:to:` made it save all ten, and `Json` ran 3.4% slower than
/// under LLVM 22. That copy is never inlined (`collections::copy_range`).
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
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
        Primitive::AnyInteger(primitive) => {
            numbers::execute_any_integer(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::LargeInteger(primitive) => {
            numbers::execute_large_integer(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Float(primitive) => {
            numbers::execute_float(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Fraction(primitive) => {
            numbers::execute_fraction(mutator, primitive, receiver, arguments).map(wrap_error)
        }
        Primitive::Number(primitive) => {
            numbers::execute_number(mutator, primitive, receiver, arguments).map(wrap_error)
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
        Primitive::Bytes(primitive) => {
            bytes::execute(mutator, primitive, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Element(access) => {
            elements::execute(mutator, access, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Block(primitive) => block::execute(mutator, primitive, receiver, arguments),
        Primitive::Platform(primitive) => {
            platform::execute(mutator, primitive, receiver, selector, arguments)
                .map(|result| result.map(Step::Value))
        }
        Primitive::Activation(primitive) => {
            activation::execute(mutator, primitive, receiver, arguments)
        }
        Primitive::Message(primitive) => message::execute(mutator, primitive, receiver, arguments),
        Primitive::Module(primitive) => {
            module::execute(mutator, primitive, receiver).map(wrap_error)
        }
        Primitive::Future(primitive) => future::execute(mutator, primitive, receiver, arguments),
        Primitive::Weak(primitive) => {
            weak::execute(mutator, primitive, receiver, selector, arguments).map(wrap_error)
        }
        Primitive::Reflection(primitive) => {
            reflection::execute_step(mutator, primitive, receiver, arguments)
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

/// The first argument, or nil when there is none: the subject an
/// `ArgumentError` carries for a leaf whose first argument is wrong.
pub(crate) fn first_argument<'gc>(heap: &Heap<'gc>, arguments: Arguments<'_, 'gc>) -> Value<'gc> {
    arguments.first(heap).unwrap_or(Value::nil())
}
