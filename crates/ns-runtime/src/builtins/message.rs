//! `Message>>sentTo:`'s leaf (`docs/design/core-library.md`, "Exceptions"):
//! a message sent to a receiver as an ordinary explicit-receiver send,
//! decided by `Mutator::decide_send` -- §5.5's lookupPublic, then
//! `doesNotUnderstand:` -- so that a message a program built reaches no
//! protected or private method, as Primordial Soup's `Message>>sendTo:`
//! reaches none. It answers the step the send decided,
//! which the run loop runs as any send's; it runs nothing itself.

use crate::builtins::primitive::MessagePrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

/// Runs `primitive` on `subject`, a `Message`. `None` -- the kernel
/// method's failure block, which raises an `Error` -- for a subject that is
/// no `Message`, a selector that is no Symbol or arguments that are neither
/// an Array nor nil.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MessagePrimitive,
    subject: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        MessagePrimitive::SendTo => {
            let receiver = *arguments.first()?;
            let (selector, message_arguments) = message_parts(mutator, subject)?;
            let decided = mutator.decide_send(receiver, selector, &message_arguments);
            Some(decided.map(|step| owning_the_message_arguments(step, message_arguments)))
        }
    }
}

/// The selector and the arguments `subject` holds, read from the slots of
/// the kernel's `Message` level of its class, a subclass's instance
/// included.
fn message_parts<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
) -> Option<(Symbol, Vec<Value<'gc>>)> {
    let Value::Object(object) = subject else {
        return None;
    };
    let field = |name: &str| mutator.kernel_slot(object, BuiltinClass::Message, name);
    let Value::Object(selector) = field("selector")? else {
        return None;
    };
    // The slot's default: a message built without `arguments:` is sent
    // with none, as a unary message a program builds naturally is.
    let message_arguments = match field("arguments")? {
        Value::Nil => Vec::new(),
        Value::Object(message_arguments) => mutator.heap.array_elements(message_arguments)?,
        _ => return None,
    };
    Some((mutator.heap.symbol_value(selector)?, message_arguments))
}

/// A step that activates something "with the send's own arguments" means
/// the message's here, not the arguments of the `sentTo:` send the run loop
/// delivers the step for.
fn owning_the_message_arguments<'gc>(
    step: Step<'gc>,
    message_arguments: Vec<Value<'gc>>,
) -> Step<'gc> {
    match step {
        Step::Method {
            receiver,
            method,
            defining_class,
            arguments: StepArguments::Send,
            feedback_base,
        } => Step::Method {
            receiver,
            method,
            defining_class,
            arguments: StepArguments::Owned(message_arguments),
            feedback_base,
        },
        Step::Block {
            closure,
            arguments: StepArguments::Send,
            then,
        } => Step::Block {
            closure,
            arguments: StepArguments::Owned(message_arguments),
            then,
        },
        other => other,
    }
}
