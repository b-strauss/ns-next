//! The activation leaves `Kernel.ns`'s `Exception` is built on
//! (`docs/design/core-library.md`, "Exceptions"): the running
//! activation as an object, its sender, the registers an `on:do:`
//! activation's marker names, and the three ways out -- a return from an
//! activation, a restart of one, and the termination an unhandled
//! exception ends in. The last three answer a signal, which the one
//! unwinder carries out (`interpreter::unwind`); no leaf here unwinds
//! anything itself. Two more report the search's outcome to an attached
//! debugger, whose exception breakpoints they decide
//! (`debugger::exceptions`).

use crate::interpreter::Arguments;
use std::rc::Rc;

use crate::builtins::primitive::{ActivationPrimitive, Primitive};
use crate::bytecode::{CodeKind, UnwindMarker};
use crate::heap::{ActivationReference, FrameReference};
use crate::interpreter::unwind::WalkedActivation;
use crate::interpreter::{unwind, Signal, Step};
use crate::types::*;
use crate::{Mutator, Symbol};

/// Runs `primitive` on `subject`. `None` -- the kernel method's failure
/// block -- for a subject or an argument of the wrong shape.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ActivationPrimitive,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
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
            if [
                arguments.first(&mutator.heap),
                arguments.try_get(&mutator.heap, 2),
            ]
            .into_iter()
            .flatten()
            .any(|argument| mutator.as_future(argument).is_some()) =>
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
        ActivationPrimitive::Terminate => {
            mutator.last_unhandled_stack = mutator.heap.describe_activations();
            Some(Err(terminate(mutator, arguments)))
        }
        ActivationPrimitive::Sender => {
            activation_leaf(mutator, SubjectLeaf::Sender, subject, arguments)
        }
        ActivationPrimitive::MethodName => {
            activation_leaf(mutator, SubjectLeaf::MethodName, subject, arguments)
        }
        ActivationPrimitive::NextHandler => {
            activation_leaf(mutator, SubjectLeaf::NextHandler, subject, arguments)
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
        // The search found no handler: an attached debugger's exception
        // breakpoints decide whether the next poll, `defaultAction`'s
        // push, stops, and answer whether the stop resumes a `Halt`
        // instead (`debugger::exceptions`). Nothing runs and nothing is
        // decided here with no debugger attached.
        ActivationPrimitive::ExceptionUnhandled => Some(Ok(Step::Value(Value::boolean(
            crate::debugger::exceptions::unhandled(mutator, subject),
        )))),
        // The search found the `on:do:` activation the argument is, and
        // is about to run its handler, which this answers as
        // `activationHandlerBlock:ifFail:` does: the catch is recorded for
        // a step in progress and the caught-exception breakpoint first,
        // the activation and the handler held for the safepoint, which
        // reads them.
        ActivationPrimitive::ExceptionCaught => {
            let activation = arguments.first(&mutator.heap)?;
            let frame = mutator.heap.object_frame(activation.as_object()?)?;
            let handler = handler_block(mutator, frame)?;
            crate::debugger::exceptions::caught(mutator, subject, activation, handler);
            Some(Ok(Step::Value(handler)))
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
    NextHandler,
    HandlerClass,
    HandlerBlock,
    SetHandlerActive,
    ReturnValue,
    Restart,
}

/// A leaf whose subject is an activation object: the object names a frame,
/// whose activation is running while the frame is married to a stack
/// frame or suspended into it (`Heap::frame_is_running`), and has returned
/// otherwise. Each leaf reads and writes through the frame, so that a
/// suspended activation answers as a married one does. A subject that is
/// no activation object declines.
fn activation_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    leaf: SubjectLeaf,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let object = subject.as_object()?;
    let frame = mutator.heap.object_frame(object)?;
    let running = mutator.heap.frame_is_running(frame);
    let answer = match leaf {
        SubjectLeaf::Sender => {
            match running
                .then(|| mutator.heap.frame_sender_frame(frame))
                .flatten()
            {
                Some(sender) => Value::object(mutator.new_activation_object(sender)),
                None => Value::nil(),
            }
        }
        SubjectLeaf::MethodName => match running.then(|| method_name(mutator, frame)).flatten() {
            Some(name) => Value::object(mutator.new_string(name)),
            None => Value::nil(),
        },
        // The running activation the walk starts from, or none for a
        // returned one, which is the `running` test made once.
        SubjectLeaf::NextHandler => match WalkedActivation::of_running_frame(&mutator.heap, frame)
            .and_then(|start| next_active_handler(mutator, start))
        {
            Some(handler) => Value::object(mutator.new_activation_object(handler)),
            None => Value::nil(),
        },
        SubjectLeaf::HandlerClass => {
            if !running {
                return None;
            }
            let (class_register, _, _) = handler_registers(mutator, frame)?;
            mutator.heap.frame_local(frame, class_register)
        }
        SubjectLeaf::HandlerBlock => handler_block(mutator, frame)?,
        SubjectLeaf::SetHandlerActive => {
            if !running {
                return None;
            }
            let (_, _, active_register) = handler_registers(mutator, frame)?;
            let active = arguments
                .first(&mutator.heap)
                .and_then(|value| value.as_boolean())?;
            mutator
                .heap
                .frame_set_local(frame, active_register, Value::boolean(active));
            subject
        }
        // Both name the frame rather than the activation, as every unwind
        // target does, so a suspended one is unwound to as a married one
        // is; a returned one is §7.2's `cannotReturn`, raised here, as
        // `raise_return` raises it for a block's `^`.
        SubjectLeaf::ReturnValue => {
            let value = arguments.first(&mutator.heap)?;
            return Some(Err(if running {
                Signal::Return(frame, value)
            } else {
                Signal::Error(unwind::dead_activation_error(
                    Symbol::intern("returnValue:"),
                    value,
                ))
            }));
        }
        SubjectLeaf::Restart => {
            return Some(Err(if running {
                Signal::Restart(frame)
            } else {
                Signal::Error(unwind::dead_activation_error(
                    Symbol::intern("restart"),
                    Value::nil(),
                ))
            }));
        }
    };
    Some(Ok(Step::Value(answer)))
}

/// The handler block of the `on:do:` activation `frame` names; `None` for
/// a returned activation or one that runs no `on:do:`.
fn handler_block<'gc>(mutator: &Mutator<'gc>, frame: FrameReference<'gc>) -> Option<Value<'gc>> {
    if !mutator.heap.frame_is_running(frame) {
        return None;
    }
    let (_, block_register, _) = handler_registers(mutator, frame)?;
    Some(mutator.heap.frame_local(frame, block_register))
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

/// `"{class}>>{selector}"` for the running activation of `frame` when it
/// runs a method: the class that declares it, named as `Class>>name` and
/// `Metaclass>>name` answer, and the selector its mixin holds the method's
/// code under. A factory's activation is named as its entry's method on
/// the metaclass, `Foo class>>new`. `None` for an activation running a
/// block, an initializer or a clause, which have no selector to report.
/// Read off the frame, which records the kind, code and class its stack
/// frame had, so a suspended activation is named as a married one is.
fn method_name<'gc>(mutator: &Mutator<'gc>, frame: FrameReference<'gc>) -> Option<String> {
    if let FrameKind::Factory(class) = mutator.heap.frame_kind(frame) {
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
    let code = mutator.heap.frame_code(frame)?;
    if code.kind != CodeKind::Method {
        return None;
    }
    let class = mutator.heap.frame_current_class(frame)?;
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

/// The frame of the first activation outward of `start` -- not `start`
/// itself -- whose code carries the `on:do:` marker and whose
/// handler is active, reified only once it is found; `None` past the
/// run's base. The walk goes on into suspended frames, as `sender` does.
/// It reifies nothing it passes, so that a signal allocates the same
/// however many activations lie between it and its handler, and a frame a
/// compiled tier ran is not turned back into interpreted ones only for
/// being passed (`docs/design/execution.md`, "Planned: the deoptimization
/// contract", "Observers").
fn next_active_handler<'gc>(
    mutator: &mut Mutator<'gc>,
    start: WalkedActivation<'gc>,
) -> Option<FrameReference<'gc>> {
    let mut walked = start;
    loop {
        walked = walked.sender(&mutator.heap)?;
        if let Some(UnwindMarker::Handler {
            active_register, ..
        }) = walked.unwind_marker(&mutator.heap)
        {
            if walked.local(&mutator.heap, active_register).as_boolean() == Some(true) {
                return Some(walked.reified(&mut mutator.heap));
            }
        }
    }
}

/// The registers of the `on:do:` marker of the code `frame`'s activation
/// runs, if it carries one.
fn handler_registers<'gc>(
    mutator: &Mutator<'gc>,
    frame: FrameReference<'gc>,
) -> Option<(u16, u16, u16)> {
    match mutator.heap.frame_unwind_marker(frame)? {
        UnwindMarker::Handler {
            class_register,
            block_register,
            active_register,
        } => Some((class_register, block_register, active_register)),
        UnwindMarker::Ensure { .. } | UnwindMarker::IfCurtailed { .. } => None,
    }
}

/// Whether `subject`'s class, or a superclass of it, is a class of the
/// declaration of the class object that is the first argument
/// (`Heap::class_declaration`).
/// Declarations rather than class objects, as Primordial Soup's
/// `is:interestedIn:` compares mixins: a nested exception class is a class
/// object per enclosing instance, and a handler for one must catch an
/// instance made under another. Each level is a class of its application's
/// mixin and, when it is a body-less declaration's class object
/// (`Heap::class_body_less_declaration`), of that declaration too, or,
/// when a `<:` clause added it for a body-less class, of that class's
/// declaration: so `class M3 = Error <: M1.` is caught by a handler for
/// `M3` or for `M1`, and never by one for another declaration applying
/// `M1`, as a declaration with a body of its own would be.
fn inherits_mixin_of<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<bool> {
    let class = arguments
        .first(&mutator.heap)
        .and_then(|value| value.as_object())?;
    if !mutator.heap.is_class(class) {
        return None;
    }
    let declaration = mutator.heap.class_declaration(class)?;
    Some(inherits_declaration(mutator, subject, &declaration))
}

/// Whether `subject`'s class, or a superclass of it, is a class of
/// `declaration`, any version of it, as `inherits_mixin_of` decides it.
pub(crate) fn inherits_declaration<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    declaration: &Rc<crate::Mixin>,
) -> bool {
    let heap = &mutator.heap;
    let id = heap.known_mixin_id(declaration);
    inherits_any(mutator, subject, |candidate| {
        heap.same_mixin(candidate, declaration, id)
    })
}

/// Whether a level of `subject`'s class chain is a class of a declaration
/// `is_declaration` accepts: the mixin its application applies and, for a
/// body-less declaration's class object or a level a `<:` clause added
/// for one, that declaration. Runs no code.
pub(crate) fn inherits_any<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    is_declaration: impl Fn(&Rc<crate::Mixin>) -> bool,
) -> bool {
    let heap = &mutator.heap;
    let mut current = Some(mutator.class_of(subject));
    while let Some(level) = current {
        let applied = heap
            .class_application(level)
            .is_some_and(|application| is_declaration(&application.mixin));
        let declared = heap
            .class_body_less_declaration(level)
            .is_some_and(&is_declaration);
        let named = heap
            .class_named(level)
            .and_then(|named| heap.class_body_less_declaration(named))
            .is_some_and(&is_declaration);
        if applied || declared || named {
            return true;
        }
        current = heap.class_superclass(level);
    }
    false
}

/// The termination an unhandled exception's `defaultAction` asks for: the
/// error the host will receive, built from the selector, receiver and
/// description the kernel hands over. Total over its arguments, so that no
/// program can make an unhandled exception carry on: a subclass's
/// `description` may answer anything, which is rendered -- a String's or a
/// Symbol's text as it is, anything else as `Heap::describe` names it --
/// and a selector that is no Symbol is reported as `signal`'s. A
/// program-made symbol whose text nothing has interned is reported as
/// `doesNotUnderstand:`'s, as the depth limit's report of the same
/// unhandled message is (`unwind.rs`, `does_not_understand_error`): the
/// host's error names its selector as a `Symbol`, and a run never interns
/// a text a program chose, or a host that keeps one interpreter would grow
/// the interner by one text per failed run; the description carries the
/// text.
fn terminate<'gc>(mutator: &Mutator<'gc>, arguments: Arguments<'_, 'gc>) -> Signal<'gc> {
    let argument = |position: usize| {
        arguments
            .try_get(&mutator.heap, position)
            .unwrap_or(Value::nil())
    };
    let selector = match argument(0).as_object() {
        Some(object) if mutator.heap.is_symbol(object) => mutator
            .heap
            .symbol_value(object)
            .unwrap_or(Symbol::DOES_NOT_UNDERSTAND),
        _ => Symbol::SIGNAL,
    };
    let description = argument(2);
    // A Symbol is a String, so its text is read the same way.
    let message = description
        .as_object()
        .and_then(|object| mutator.heap.str_value(object))
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
                assert_eq!(
                    inherits_mixin_of(mutator, subject, Arguments::Values(&arguments.as_slice())),
                    Some(true)
                );
            }
        });
    }
}
