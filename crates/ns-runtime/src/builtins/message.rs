//! `Message>>sendTo:`'s leaf (`docs/design/core-library.md`, "Exceptions"):
//! a message sent to a receiver as an ordinary explicit-receiver send,
//! decided by `Mutator::decide_send` -- §5.5's lookupPublic, then
//! `doesNotUnderstand:` -- so that a message a program built reaches no
//! protected or private method. It answers the step the send decided,
//! which the run loop runs as any send's; it runs nothing itself.

use crate::Mutator;
use crate::builtins::primitive::MessagePrimitive;
use crate::interpreter::Arguments;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;

/// Runs `primitive` on `subject`, a `Message`. `None` -- the kernel
/// method's failure block, which raises an `Error` -- for a subject that is
/// no `Message`, a selector that is no Symbol or arguments that are neither
/// an Array nor nil.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MessagePrimitive,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        MessagePrimitive::SendTo => {
            let receiver = arguments.first(&mutator.heap)?;
            let (selector, message_arguments) = message_parts(mutator, subject)?;
            let delivered = Arguments::Values(&message_arguments.as_slice());
            let decided = match mutator.heap.symbol_value(selector) {
                Some(selector) => mutator.decide_send(receiver, selector, delivered),
                // A program-made symbol whose text nothing has interned:
                // no method, slot or class has that name, since every one
                // is interned, so nothing public answers it.
                None => mutator.decide_unnamed_send(receiver, selector, delivered),
            };
            Some(decided.map(|step| owning_the_message_arguments(step, message_arguments)))
        }
    }
}

/// The selector, a Symbol, and the arguments `subject` holds, read from
/// the slots of the kernel's `Message` level of its class, a subclass's
/// instance included.
fn message_parts<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
) -> Option<(ObjectReference<'gc>, Vec<Value<'gc>>)> {
    let object = subject.as_object()?;
    let field = |name: &str| mutator.kernel_slot(object, BuiltinClass::Message, name);
    let selector = field("selector")?
        .as_object()
        .filter(|selector| mutator.heap.is_symbol(*selector))?;
    // The slot's default: a message built without `arguments:` is sent
    // with none, as a unary message a program builds naturally is.
    let message_arguments = match field("arguments")?.kind() {
        ValueKind::Nil => Vec::new(),
        ValueKind::Object(message_arguments) => mutator.heap.array_elements(message_arguments)?,
        _ => return None,
    };
    Some((selector, message_arguments))
}

/// A step that activates something "with the send's own arguments" means
/// the message's here, not the arguments of the `sendTo:` send the run loop
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
