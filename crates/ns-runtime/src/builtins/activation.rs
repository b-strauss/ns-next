//! The activation leaves `Kernel.ns`'s `Exception` is built on
//! (`docs/design/core-library.md`, "Planned: exceptions"): the running
//! activation as an object, its sender, the registers an `on:do:`
//! activation's marker names, and the three ways out -- a return from an
//! activation, a restart of one, and the termination an unhandled
//! exception ends in. The last three answer a signal, which the one
//! unwinder carries out (`interpreter::unwind`); no leaf here unwinds
//! anything itself.

use std::rc::Rc;

use crate::builtins::primitive::ActivationPrimitive;
use crate::bytecode::UnwindMarker;
use crate::heap::ActivationReference;
use crate::interpreter::{unwind, Signal, Step};
use crate::types::*;
use crate::{Mutator, Symbol};

/// Runs `primitive` on `subject`. `None` -- the kernel method's failure
/// block -- for a subject or an argument of the wrong shape.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ActivationPrimitive,
    subject: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        // The top activation is the kernel method making the mirror send:
        // a mirror send runs its leaf without an activation of its own,
        // under both lowerings. Never sent from a forwarder, which has no
        // activation either and would answer its caller's. A host's own
        // send to the mirror runs with no activation at all, and declines.
        ActivationPrimitive::Current => {
            let top = mutator.heap.top_activation()?;
            Some(Ok(Step::Value(activation_object(mutator, top))))
        }
        ActivationPrimitive::InheritsMixinOf => {
            inherits_mixin_of(mutator, subject, arguments).map(|answer| Ok(Step::Value(answer)))
        }
        ActivationPrimitive::Terminate => Some(Err(terminate(mutator, arguments))),
        _ => {
            let Value::Object(object) = subject else {
                return None;
            };
            let frame = mutator.heap.activation_object_frame(object)?;
            let live = mutator.heap.frame_activation(frame);
            activation_leaf(mutator, primitive, subject, frame, live, arguments)
        }
    }
}

/// A leaf whose subject is an activation object, of `frame`, whose running
/// activation is `live` -- `None` once it has returned.
fn activation_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ActivationPrimitive,
    subject: Value<'gc>,
    frame: FrameReference<'gc>,
    live: Option<ActivationReference>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let answer = match primitive {
        ActivationPrimitive::Sender => {
            match live.and_then(|activation| mutator.heap.activation_sender(activation)) {
                Some(sender) => activation_object(mutator, sender),
                None => Value::Nil,
            }
        }
        ActivationPrimitive::IsActiveHandler => {
            let active = live
                .and_then(|activation| {
                    let (_, _, active_register) = handler_registers(mutator, activation)?;
                    Some(mutator.heap.activation_local(activation, active_register))
                })
                .is_some_and(|value| matches!(value, Value::Boolean(true)));
            Value::Boolean(active)
        }
        ActivationPrimitive::HandlerClass => {
            let activation = live?;
            let (class_register, _, _) = handler_registers(mutator, activation)?;
            mutator.heap.activation_local(activation, class_register)
        }
        ActivationPrimitive::HandlerBlock => {
            let activation = live?;
            let (_, block_register, _) = handler_registers(mutator, activation)?;
            mutator.heap.activation_local(activation, block_register)
        }
        ActivationPrimitive::SetHandlerActive => {
            let activation = live?;
            let (_, _, active_register) = handler_registers(mutator, activation)?;
            let Some(Value::Boolean(active)) = arguments.first() else {
                return None;
            };
            mutator
                .heap
                .activation_set_local(activation, active_register, Value::Boolean(*active));
            subject
        }
        // Both name the frame rather than the activation, as every unwind
        // target does; a dead one is §7.2's `cannotReturn`, raised here, as
        // `raise_return` raises it for a block's `^`.
        ActivationPrimitive::ReturnValue => {
            let value = *arguments.first()?;
            return Some(Err(match live {
                Some(_) => Signal::Return(frame, value),
                None => Signal::Error(unwind::dead_activation_error(Symbol::intern(
                    "returnValue:",
                ))),
            }));
        }
        ActivationPrimitive::Restart => {
            return Some(Err(match live {
                Some(_) => Signal::Restart(frame),
                None => Signal::Error(unwind::dead_activation_error(Symbol::intern("restart"))),
            }));
        }
        ActivationPrimitive::Current
        | ActivationPrimitive::InheritsMixinOf
        | ActivationPrimitive::Terminate => {
            unreachable!("internal: {primitive:?} takes no activation subject")
        }
    };
    Some(Ok(Step::Value(answer)))
}

/// A fresh object for `activation`, reified. Each call answers a new one,
/// so the kernel compares activations only as objects one call answered.
fn activation_object<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
) -> Value<'gc> {
    let frame = mutator.heap.reify(activation);
    Value::Object(mutator.new_activation_object(frame))
}

/// The registers of `activation`'s `on:do:` marker, if its code carries one.
fn handler_registers(
    mutator: &Mutator<'_>,
    activation: ActivationReference,
) -> Option<(u16, u16, u16)> {
    match mutator.heap.activation_unwind_marker(activation)? {
        UnwindMarker::Handler {
            class_register,
            block_register,
            active_register,
        } => Some((class_register, block_register, active_register)),
        UnwindMarker::Ensure { .. } | UnwindMarker::IfCurtailed { .. } => None,
    }
}

/// Whether `subject`'s class, or a superclass of it, applies the mixin of
/// the class object `arguments[0]`. Mixins rather than class objects, as
/// Primordial Soup's `is:interestedIn:` compares them: a nested exception
/// class is a class object per enclosing instance, and a handler for one
/// must catch an instance made under another.
fn inherits_mixin_of<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Value<'gc>> {
    let Some(Value::Object(class)) = arguments.first() else {
        return None;
    };
    if !mutator.heap.is_class(*class) {
        return None;
    }
    let mixin = mutator.heap.class_application(*class)?.mixin.clone();
    let mut current = Some(mutator.class_of(&subject));
    while let Some(level) = current {
        if let Some(application) = mutator.heap.class_application(level) {
            if Rc::ptr_eq(&application.mixin, &mixin) {
                return Some(Value::Boolean(true));
            }
        }
        current = mutator.heap.class_superclass(level);
    }
    Some(Value::Boolean(false))
}

/// The termination an unhandled exception's `defaultAction` asks for: the
/// error the host will receive, built from the selector, receiver and
/// description the kernel hands over. Total over its arguments, so that no
/// program can make an unhandled exception carry on: a subclass's
/// `description` may answer anything, which is rendered -- a String's or a
/// Symbol's text as it is, anything else as `Heap::describe` names it --
/// and a selector that is no Symbol is reported as `signal`'s.
fn terminate<'gc>(mutator: &Mutator<'gc>, arguments: &[Value<'gc>]) -> Signal<'gc> {
    let argument = |position: usize| arguments.get(position).copied().unwrap_or(Value::Nil);
    let selector = match argument(0) {
        Value::Object(object) => mutator.heap.symbol_value(object),
        _ => None,
    }
    .unwrap_or_else(|| Symbol::intern("signal"));
    let description = argument(2);
    let message = match description {
        Value::Object(object) => mutator.heap.str_value(object).or_else(|| {
            mutator
                .heap
                .symbol_value(object)
                .map(|symbol| symbol.to_string())
        }),
        _ => None,
    }
    .unwrap_or_else(|| mutator.heap.describe(&description));
    Signal::Terminate(NewspeakError {
        selector,
        receiver: argument(1),
        message,
    })
}
