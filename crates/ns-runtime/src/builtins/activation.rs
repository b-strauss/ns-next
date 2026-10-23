//! The activation leaves `Kernel.ns`'s `Exception` is built on
//! (`docs/design/core-library.md`, "Exceptions"): the running
//! activation as an object, its sender, the registers an `on:do:`
//! activation's marker names, and the three ways out -- a return from an
//! activation, a restart of one, and the termination an unhandled
//! exception ends in. The last three answer a signal, which the one
//! unwinder carries out (`interpreter::unwind`); no leaf here unwinds
//! anything itself.

use std::rc::Rc;

use crate::builtins::primitive::{ActivationPrimitive, Primitive};
use crate::bytecode::{CodeKind, UnwindMarker};
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
            let inherits = inherits_mixin_of(mutator, subject, arguments)?;
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Activation(primitive),
                inherits,
                subject,
                arguments,
            )
            .map(|answer| Ok(Step::Value(Value::boolean(answer))))
        }
        // It reads its selector and renders its description, so a future in
        // either would be reported as a future rather than as the value it
        // stands for. Declining -- before the test hook below counts
        // anything -- hands it to its caller's failure path, which sees through
        // it; the failure block never runs, since a future is there. The
        // receiver alone does not make it decline: it is often the very
        // future whose divergent forcing raised the error, and declining for
        // it would raise that error again on every termination, forever.
        // Once the selector or the description is a future, `see_through`
        // walks every operand, the receiver included, so a receiver still
        // being computed then raises the divergent error once -- which,
        // unhandled, ends the run through a termination whose arguments hold
        // no future.
        ActivationPrimitive::Terminate
            if [arguments.first(), arguments.get(2)]
                .into_iter()
                .flatten()
                .any(|argument| mutator.as_future(*argument).is_some()) =>
        {
            None
        }
        // The test hook standing in for the debugger
        // (`Interpreter::resume_unhandled_exceptions`) resumes instead:
        // `defaultAction` answers `nil`, and so does the `signal` that
        // reached it.
        ActivationPrimitive::Terminate if mutator.unhandled_exceptions_to_resume > 0 => {
            mutator.unhandled_exceptions_to_resume -= 1;
            Some(Ok(Step::Value(Value::nil())))
        }
        ActivationPrimitive::Terminate => Some(Err(terminate(mutator, arguments))),
        ActivationPrimitive::Sender => {
            activation_leaf(mutator, SubjectLeaf::Sender, subject, arguments)
        }
        ActivationPrimitive::MethodName => {
            activation_leaf(mutator, SubjectLeaf::MethodName, subject, arguments)
        }
        ActivationPrimitive::IsActiveHandler => {
            activation_leaf(mutator, SubjectLeaf::IsActiveHandler, subject, arguments)
        }
        ActivationPrimitive::HandlerClass => {
            activation_leaf(mutator, SubjectLeaf::HandlerClass, subject, arguments)
        }
        ActivationPrimitive::HandlerBlock => {
            activation_leaf(mutator, SubjectLeaf::HandlerBlock, subject, arguments)
        }
        ActivationPrimitive::SetHandlerActive => {
            activation_leaf(mutator, SubjectLeaf::SetHandlerActive, subject, arguments)
        }
        ActivationPrimitive::ReturnValue => {
            activation_leaf(mutator, SubjectLeaf::ReturnValue, subject, arguments)
        }
        ActivationPrimitive::Restart => {
            activation_leaf(mutator, SubjectLeaf::Restart, subject, arguments)
        }
    }
}

/// The leaves whose subject is an activation object, the ones
/// `activation_leaf` runs: a type of their own, so that it has no arm for a
/// leaf that takes no activation.
#[derive(Clone, Copy)]
enum SubjectLeaf {
    Sender,
    MethodName,
    IsActiveHandler,
    HandlerClass,
    HandlerBlock,
    SetHandlerActive,
    ReturnValue,
    Restart,
}

/// A leaf whose subject is an activation object: the object names a frame,
/// whose running activation is `live` -- `None` once it has returned. A
/// subject that is no activation object declines.
fn activation_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    leaf: SubjectLeaf,
    subject: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let object = subject.as_object()?;
    let frame = mutator.heap.object_frame(object)?;
    let live = mutator.heap.frame_activation(frame);
    let answer = match leaf {
        SubjectLeaf::Sender => {
            match live.and_then(|activation| mutator.heap.activation_sender(activation)) {
                Some(sender) => activation_object(mutator, sender),
                None => Value::nil(),
            }
        }
        SubjectLeaf::MethodName => {
            match live.and_then(|activation| method_name(mutator, activation)) {
                Some(name) => Value::object(mutator.new_string(name)),
                None => Value::nil(),
            }
        }
        SubjectLeaf::IsActiveHandler => {
            let active = live
                .and_then(|activation| {
                    let (_, _, active_register) = handler_registers(mutator, activation)?;
                    Some(mutator.heap.activation_local(activation, active_register))
                })
                .is_some_and(|value| value.as_boolean() == Some(true));
            Value::boolean(active)
        }
        SubjectLeaf::HandlerClass => {
            let activation = live?;
            let (class_register, _, _) = handler_registers(mutator, activation)?;
            mutator.heap.activation_local(activation, class_register)
        }
        SubjectLeaf::HandlerBlock => {
            let activation = live?;
            let (_, block_register, _) = handler_registers(mutator, activation)?;
            mutator.heap.activation_local(activation, block_register)
        }
        SubjectLeaf::SetHandlerActive => {
            let activation = live?;
            let (_, _, active_register) = handler_registers(mutator, activation)?;
            let active = arguments.first().and_then(|value| value.as_boolean())?;
            mutator
                .heap
                .activation_set_local(activation, active_register, Value::boolean(active));
            subject
        }
        // Both name the frame rather than the activation, as every unwind
        // target does; a dead one is §7.2's `cannotReturn`, raised here, as
        // `raise_return` raises it for a block's `^`.
        SubjectLeaf::ReturnValue => {
            let value = *arguments.first()?;
            return Some(Err(match live {
                Some(_) => Signal::Return(frame, value),
                None => Signal::Error(unwind::dead_activation_error(
                    Symbol::intern("returnValue:"),
                    value,
                )),
            }));
        }
        SubjectLeaf::Restart => {
            return Some(Err(match live {
                Some(_) => Signal::Restart(frame),
                None => Signal::Error(unwind::dead_activation_error(
                    Symbol::intern("restart"),
                    Value::nil(),
                )),
            }));
        }
    };
    Some(Ok(Step::Value(answer)))
}

/// The object for `activation`: its frame object, reified on the first
/// call and the same object on every later one while it runs.
fn activation_object<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
) -> Value<'gc> {
    let frame = mutator.heap.reify(activation);
    Value::object(mutator.new_activation_object(frame))
}

/// `"{class}>>{selector}"` for an activation running a method: the class
/// that declares it, named as `Class>>name` and `Metaclass>>name` answer,
/// and the selector its mixin holds the method's code under. A factory's
/// activation is named as its entry's method on the metaclass, `Foo
/// class>>new`. `None` for an activation running a block, an initializer or
/// a clause, which have no selector to report.
fn method_name(mutator: &Mutator<'_>, activation: ActivationReference) -> Option<String> {
    if let FrameKind::Factory(class) = mutator.heap.activation_kind(activation) {
        // Read through the metaclass (`Heap::class_factory`): a body-less
        // declaration's class object applies its last clause's mixin and
        // has its declaration's factory.
        let factory = mutator.heap.class_factory(class)?;
        return Some(format!(
            "{} class>>{}",
            mutator.heap.class_display_name(class),
            factory.selector
        ));
    }
    let code = mutator.heap.activation_code(activation);
    if code.kind != CodeKind::Method {
        return None;
    }
    let class = mutator.heap.activation_current_class(activation)?;
    let application = mutator.heap.class_application(class)?;
    let method = application
        .mixin
        .compiled_methods()
        .find(|method| Rc::ptr_eq(&method.code, &code))?;
    Some(format!(
        "{}>>{}",
        mutator.heap.class_display_name(class),
        method.selector
    ))
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
) -> Option<bool> {
    let class = arguments.first().and_then(|value| value.as_object())?;
    if !mutator.heap.is_class(class) {
        return None;
    }
    let mixin = mutator.heap.class_application(class)?.mixin.clone();
    let id = mutator.heap.known_mixin_id(&mixin);
    let mut current = Some(mutator.class_of(subject));
    while let Some(level) = current {
        if let Some(application) = mutator.heap.class_application(level) {
            if mutator.heap.same_mixin(&application.mixin, &mixin, id) {
                return Some(true);
            }
        }
        current = mutator.heap.class_superclass(level);
    }
    Some(false)
}

/// The termination an unhandled exception's `defaultAction` asks for: the
/// error the host will receive, built from the selector, receiver and
/// description the kernel hands over. Total over its arguments, so that no
/// program can make an unhandled exception carry on: a subclass's
/// `description` may answer anything, which is rendered -- a String's or a
/// Symbol's text as it is, anything else as `Heap::describe` names it --
/// and a selector that is no Symbol is reported as `signal`'s.
fn terminate<'gc>(mutator: &Mutator<'gc>, arguments: &[Value<'gc>]) -> Signal<'gc> {
    let argument = |position: usize| arguments.get(position).copied().unwrap_or(Value::nil());
    let selector = argument(0)
        .as_object()
        .and_then(|object| mutator.heap.symbol_value(object))
        .unwrap_or(Symbol::SIGNAL);
    let description = argument(2);
    let message = description
        .as_object()
        .and_then(|object| {
            mutator.heap.str_value(object).or_else(|| {
                mutator
                    .heap
                    .symbol_value(object)
                    .map(|symbol| symbol.to_string())
            })
        })
        .unwrap_or_else(|| mutator.heap.describe(description));
    Signal::Terminate(Box::new(NewspeakError::error(
        selector,
        argument(1),
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Interpreter, SourceOrigin};
    use indoc::indoc;
    use std::path::PathBuf;

    const SOURCE: &str = indoc! {"
        class Outer = Object new () (
          public class Inner = () (
            public answer = ( ^1 )
          )
          public inner = ( ^Inner )
          public fresh = ( ^Inner new )
        )
    "};

    /// A handler for an exception class catches an instance of the class
    /// as it was before an edit replaced its mixin, and the other way
    /// round: the two versions are one declaration.
    #[test]
    fn inherits_mixin_of_matches_applications_of_one_id() {
        let module = ns_syntax::parse(SOURCE).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(SOURCE),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let outer = interpreter.load_module(&resolved);
        let send = |interpreter: &mut Interpreter, receiver, selector: &str| match interpreter.send(
            receiver,
            Symbol::intern(selector),
            &[],
        ) {
            Ok(root) => root,
            Err(_) => panic!("{selector} failed"),
        };
        let first = interpreter.instantiate(&outer, &[]).ok().expect("an Outer");
        let old_instance = send(&mut interpreter, &first, "fresh");
        let old_class = send(&mut interpreter, &first, "inner");
        interpreter.enter(|mutator| {
            let class = mutator.fetch(&outer).as_object().expect("a class");
            let outer_mixin = mutator
                .heap()
                .class_application(class)
                .expect("a class object")
                .mixin
                .clone();
            let outer_id = mutator
                .heap()
                .known_mixin_id(&outer_mixin)
                .expect("registered");
            let id = mutator.heap().nested_of(outer_id)[0];
            let inner = mutator.heap().mixin(id).expect("a row");
            mutator.replace_mixin(id, Rc::new((*inner).clone()));
        });
        let second = interpreter.instantiate(&outer, &[]).ok().expect("an Outer");
        let new_instance = send(&mut interpreter, &second, "fresh");
        let new_class = send(&mut interpreter, &second, "inner");
        interpreter.enter(|mutator| {
            for (subject, class) in [(&old_instance, &new_class), (&new_instance, &old_class)] {
                let subject = mutator.fetch(subject);
                let arguments = [mutator.fetch(class)];
                assert_eq!(inherits_mixin_of(mutator, subject, &arguments), Some(true));
            }
        });
    }
}
