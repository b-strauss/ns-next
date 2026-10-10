//! The one unwinder (`docs/design/core-library.md`, "Exceptions",
//! and `docs/design/execution.md`, "Unwinding"): what a signal
//! leaving the running activation does to the stack. It decides, and the run
//! loop only does what the outcome says, so the reference lowering and a
//! JIT reach the same unwinding.
//!
//! It pops from the top towards its target. An activation whose code carries
//! an `ensure:` or `ifCurtailed:` marker (`bytecode::UnwindMarker`) whose
//! block has not run stops it: the unwinder marks the block run and answers
//! it, the loop runs it as a trampoline step above the marked activation,
//! and when it returns its continuation (`Continuation::Unwind`) raises the
//! unwind again, which carries on from there. That is the run loop
//! scheduling steps, not Rust calling back into Newspeak. An activation with
//! no marker costs one test.

use crate::bytecode::UnwindMarker;
use crate::heap::{ActivationHeader, ActivationReference};
use crate::types::*;
use crate::Symbol;

use super::{
    EvaluationResult, Mutator, Signal, Step, StepArguments, DEPTH_RESERVE, MAX_ACTIVATION_DEPTH,
};

/// Where an unwind stopped.
pub(crate) enum UnwindOutcome<'gc> {
    /// Popped down to the target, which is now the top of the stack; it
    /// returns `value` through the ordinary return path.
    Returned {
        target: ActivationReference,
        value: Value<'gc>,
    },
    /// Popped down to the target, an `on:do:` activation `retry` asked to
    /// run again: its program counter is back at 0 and the loop switches
    /// to it, its registers as they are -- its first instruction sets
    /// `handlerActive` again.
    Restarted { target: ActivationReference },
    /// An `ensure:` or `ifCurtailed:` argument to run first, above the
    /// marked activation, which stays on the stack: the loop starts it with
    /// `Mutator::start_unwind_block`, and the unwind resumes when it returns
    /// (`Continuation::Unwind`).
    RunBlock {
        block: Value<'gc>,
        then: Box<PendingUnwind<'gc>>,
    },
    /// A VM error became an `Error`, whose `signal` has been pushed above
    /// the activation where the error arose (`Mutator::signal_vm_error`):
    /// the loop switches to it at its first instruction.
    Signalling(ActivationReference),
    /// Popped down to the run's floor: the run ends with this.
    Exit(EvaluationResult<'gc>),
}

/// A signal leaving the running activation. A non-local return is an
/// unwind to its home, a restart an unwind to the `on:do:` activation it
/// restarts, and a termination an unwind with no target, which runs every
/// pending block on its way to the floor (`unwind`).
///
/// An error a step raised is no unwind at all: once the kernel is
/// installed it becomes a signalled `Error` (`start_signalling`), and
/// nothing is popped until a handler, or the default action, decides --
/// unless not even `signal`'s activation can be pushed, the depth limit's
/// reserve used up, which ends the run as a termination. Before that --
/// while the kernel itself is being instantiated, when there is no `Error`
/// class yet -- it pops every activation of the run, `base` included, and
/// leaves the run with the error for the host that started it
/// (`Interpreter::run_from_host`), running no block.
pub(crate) fn unwind_signal<'gc>(
    mutator: &mut Mutator<'gc>,
    signal: Signal<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
    if let Signal::Error(error) = &signal {
        if let Some(outcome) = start_signalling(mutator, error, floor) {
            return outcome;
        }
    }
    let pending = match signal {
        Signal::Return(target, value) => PendingUnwind {
            target: Some(target),
            action: UnwindAction::Return(value),
        },
        Signal::Restart(target) => PendingUnwind {
            target: Some(target),
            action: UnwindAction::Restart,
        },
        Signal::Terminate(error) => PendingUnwind {
            target: None,
            action: UnwindAction::Terminate(*error),
        },
        error @ Signal::Error(_) => {
            while mutator.heap.activation_depth() > floor {
                let top = mutator
                    .heap
                    .top_activation()
                    .expect("internal: a stack deeper than the floor has a top");
                let popped = mutator.heap.pop_activation(top);
                mutator.abandon(popped.then);
            }
            return UnwindOutcome::Exit(Err(error));
        }
    };
    unwind(mutator, pending, floor)
}

/// Carries `pending` on from the top of the stack. A target that is still
/// running (`Heap::frame_activation`), sits at or above the run's floor and
/// catches a return (`Mutator::catches_return`: a method, or a superclass
/// clause or its message) ends the unwind: everything above it is popped,
/// and it returns the value through the ordinary return path, or runs
/// again from its start for a restart. Any other target -- a home that has
/// returned meanwhile, or a running initializer, which catches none (the
/// known gap in `docs/design/execution.md`, "Unwinding") -- and a
/// termination, which has none, pop every activation of the run, `base`
/// included, and the run leaves with what `exit_signal` makes of the
/// unwind.
///
/// The target is found by its frame, not by an activation handle, because
/// the blocks this runs push activations while the unwind is in progress:
/// one of them can sit at the index a popped target used, which a frame --
/// divorced when its activation was popped -- cannot be mistaken for.
///
/// Every activation it pops hands its continuation to `Mutator::abandon`,
/// which undoes what a continuation that will now never run had begun.
pub(crate) fn unwind<'gc>(
    mutator: &mut Mutator<'gc>,
    pending: PendingUnwind<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
    let destination = pending
        .target
        .and_then(|frame| mutator.heap.frame_activation(frame))
        .filter(|&target| {
            mutator.heap.activation_position(target) >= floor && mutator.catches_return(target)
        });
    loop {
        if mutator.heap.activation_depth() <= floor {
            return UnwindOutcome::Exit(Err(exit_signal(pending)));
        }
        let top = mutator
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        if let Some(target) = destination {
            if top.same(target) {
                return arrive(mutator, target, pending.action);
            }
        }
        if let Some(block) = block_to_run(mutator, top) {
            return UnwindOutcome::RunBlock {
                block,
                then: Box::new(pending),
            };
        }
        let popped = mutator.heap.pop_activation(top);
        mutator.abandon(popped.then);
    }
}

/// The block `activation` asks to be run before it is popped, marked run: an
/// `ensure:`'s or an `ifCurtailed:`'s whose `complete` is still nil. Set
/// before the block runs, as `ensure:`'s own normal path sets it, so that
/// whichever path gets there first runs it and the other does not -- a block
/// that unwinds again, or an unwind through an `ensure:` running its block
/// on the normal path, finds it set.
fn block_to_run<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
) -> Option<Value<'gc>> {
    let (block_register, complete_register) =
        match mutator.heap.activation_unwind_marker(activation)? {
            UnwindMarker::Ensure {
                block_register,
                complete_register,
            }
            | UnwindMarker::IfCurtailed {
                block_register,
                complete_register,
            } => (block_register, complete_register),
            UnwindMarker::Handler { .. } => return None,
        };
    if !mutator
        .heap
        .activation_local(activation, complete_register)
        .is_nil()
    {
        return None;
    }
    mutator
        .heap
        .activation_set_local(activation, complete_register, Value::boolean(true));
    Some(mutator.heap.activation_local(activation, block_register))
}

/// What the unwind does at its target, now the top of the stack.
fn arrive<'gc>(
    mutator: &mut Mutator<'gc>,
    target: ActivationReference,
    action: UnwindAction<'gc>,
) -> UnwindOutcome<'gc> {
    match action {
        UnwindAction::Return(value) => UnwindOutcome::Returned { target, value },
        UnwindAction::Restart => {
            mutator.heap.activation_set_program_counter(target, 0);
            UnwindOutcome::Restarted { target }
        }
        UnwindAction::Terminate(_) => {
            unreachable!("internal: a termination has no target to arrive at")
        }
    }
}

/// The signal `pending` was, raised again: how an unwind carries on once a
/// block it ran has returned (`Mutator::resume`), as what it was -- a
/// return, a restart or a termination.
pub(crate) fn signal_of(pending: PendingUnwind<'_>) -> Signal<'_> {
    match pending.action {
        UnwindAction::Return(value) => {
            let Some(target) = pending.target else {
                unreachable!("internal: a non-local return always names its home")
            };
            Signal::Return(target, value)
        }
        UnwindAction::Restart => {
            let Some(target) = pending.target else {
                unreachable!("internal: a restart always names its on:do: activation")
            };
            Signal::Restart(target)
        }
        UnwindAction::Terminate(error) => Signal::Terminate(Box::new(error)),
    }
}

/// What the run leaves with when `pending` reached the floor. A `^` still
/// names its home, for the host; a termination has run every pending block
/// and ends in its error, the `HostError` the host received before there
/// were exceptions; and a restart whose `on:do:` activation is no longer
/// running -- it had returned by the time the unwind got there -- is §7.2's
/// `cannotReturn`, as a `return:` to it is (`builtins::activation`).
fn exit_signal(pending: PendingUnwind<'_>) -> Signal<'_> {
    match pending.action {
        UnwindAction::Terminate(error) => Signal::Error(error),
        UnwindAction::Restart => Signal::Error(dead_activation_error(
            Symbol::intern("restart"),
            Value::nil(),
        )),
        UnwindAction::Return(_) => signal_of(pending),
    }
}

/// §7.2's `cannotReturn` for an activation that has already returned,
/// named after the leaf that aimed at it. `result` is the value the leaf
/// tried to return; a restart returns none, so it passes nil.
pub(crate) fn dead_activation_error<'gc>(
    selector: Symbol,
    result: Value<'gc>,
) -> NewspeakError<'gc> {
    NewspeakError::cannot_return(
        selector,
        Value::nil(),
        result,
        "cannotReturn: the activation has already returned",
    )
}

/// Starts signalling `error` as an `Error`, once the kernel is installed:
/// `signal`'s activation, pushed above the top of the stack -- the
/// activation where the error arose -- with the continuation that ends the
/// run should `signal` ever return (`Continuation::VmErrorSignalled`).
///
/// An error raised within `DEPTH_RESERVE` activations of the depth limit,
/// the limit's own error included, takes the reserve first, unless a
/// `signal` already holds it: `signal`'s activation and everything it runs
/// may then go that far past the limit, until it is gone
/// (`Mutator::depth_reserve_held`). Far from the limit nothing is taken, so
/// an ordinary error, and the handler it runs, meet the limit where any
/// code does.
///
/// A push refused even so -- the reserve is held and used up -- ends the
/// run with `error`, unsignalled: it is the error being signalled, so the
/// run ends with it rather than with the depth error the refusal raised.
///
/// `None` only when there is no kernel yet: the run then ends with `error`
/// unsignalled, as it did before there were exceptions.
fn start_signalling<'gc>(
    mutator: &mut Mutator<'gc>,
    error: &NewspeakError<'gc>,
    floor: usize,
) -> Option<UnwindOutcome<'gc>> {
    mutator.kernel?;
    let step = match mutator.signal_vm_error(error.clone()) {
        Ok(Step::Value(_)) | Err(_) => return None,
        Ok(step) => step,
    };
    let taking = !mutator.depth_reserve_held()
        && mutator.heap.counted_depth() >= MAX_ACTIVATION_DEPTH - DEPTH_RESERVE;
    let sender = mutator.heap.top_activation();
    let taking = taking && sender.is_some();
    if let (true, Some(top)) = (taking, sender) {
        mutator.depth_reserve = Some(mutator.heap.reify(top));
        mutator.taking_depth_reserve = true;
    }
    let pushed = mutator.push_step(step, &[], sender, false);
    mutator.taking_depth_reserve = false;
    match pushed {
        Ok(activation) => {
            mutator.heap.activation_set_continuation(
                activation,
                Continuation::VmErrorSignalled(Box::new(error.clone())),
            );
            Some(UnwindOutcome::Signalling(activation))
        }
        Err(_) => {
            if taking {
                mutator.depth_reserve = None;
            }
            let pending = PendingUnwind {
                target: None,
                action: UnwindAction::Terminate(error.clone()),
            };
            Some(unwind(mutator, pending, floor))
        }
    }
}

impl<'gc> Mutator<'gc> {
    /// §8.6 and `docs/design/core-library.md`, "Exceptions": a VM
    /// error becomes an instance of the kernel class its `class` names --
    /// an `Error`, or an `Error` subclass that carries the error's subject
    /// in a slot of its own -- its `messageText` the error's text, its
    /// private `originSelector` and `originReceiver` the selector and
    /// receiver the host received before there were exceptions, which
    /// `defaultAction` hands back -- and the step that sends it `signal`.
    /// The instance is built by name, as the platform is, since no Newspeak
    /// can run while a step is being decided.
    pub(crate) fn signal_vm_error(
        &mut self,
        error: NewspeakError<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let text = Value::object(self.new_string(String::from(error.message)));
        let selector = self.new_symbol(error.selector);
        let receiver = error.receiver;
        let subject = error.subject;
        // The slot each class adds for what it carries; an `Error` adds none.
        let (kind, subject_slot) = match error.class {
            ErrorClass::Error => (BuiltinClass::Error, None),
            ErrorClass::ArgumentError => (BuiltinClass::ArgumentError, Some("argument")),
            ErrorClass::CannotReturn => (BuiltinClass::CannotReturn, Some("result")),
            ErrorClass::NonBooleanReceiver => (BuiltinClass::NonBooleanReceiver, Some("receiver")),
            ErrorClass::CannotInstantiate => (BuiltinClass::CannotInstantiate, Some("behavior")),
        };
        let exception = self.new_kernel_instance(kind, |name| match name {
            "messageText" => text,
            "originSelector" => selector,
            "originReceiver" => receiver,
            "reachedDefault" | "searching" => Value::boolean(false),
            "signalActivation" | "handlerActivation" => Value::nil(),
            other if subject_slot == Some(other) => subject,
            other => panic!("internal: Kernel.ns's {other} slot is one the VM does not fill"),
        });
        self.decide_send(Value::object(exception), Symbol::SIGNAL, &[])
    }

    /// What the run ends with when a push overruns the depth limit's
    /// reserve, for the activation `header` describes. When the refusal
    /// came from the signalling machinery -- the innermost error the
    /// reserve is taken for is still looking for its handler, after its
    /// `signal` or a `pass`, or running its default action -- it is that
    /// error: the run ends as it would have had there been room to signal
    /// it, not as the depth limit. An error the machinery itself raised
    /// while looking -- a `handles:` nothing understands -- is part of the
    /// search it arose in, so the error reported is the outermost of a run
    /// of such errors, each raised by the search of the one below it. When
    /// a handler, or anything else that runs once a handler has been
    /// found, overran the reserve, it is the depth limit's own error. A
    /// program's own `signal` takes no reserve, so it is never the error an
    /// overrun reports.
    ///
    /// Walks the stack from its top: once per run, since an overrun ends
    /// it.
    pub(crate) fn overrun_of_the_reserve(
        &self,
        header: &ActivationHeader<'gc>,
    ) -> NewspeakError<'gc> {
        let mut reported = None;
        let mut above = None;
        let mut current = self.heap.top_activation();
        while let Some(activation) = current {
            if let Some(error) = self.heap.activation_signalled_vm_error(activation) {
                if !self.is_finding_its_handler(self.heap.activation_receiver(activation)) {
                    break;
                }
                reported = Some(error);
            } else if self.is_kernel_does_not_understand(activation) {
                if !self.does_not_understand_is_finding_its_handler(above) {
                    break;
                }
                match self.does_not_understand_error(activation) {
                    Some(error) => reported = Some(error),
                    None => break,
                }
            }
            above = Some(activation);
            current = self.heap.activation_sender(activation);
        }
        reported.unwrap_or_else(|| super::recursion_too_deep_error(header))
    }

    /// Whether a kernel `doesNotUnderstand:` activation is still in the
    /// kernel's own hands. `above` is the activation directly above it, the
    /// one it sent to: while that
    /// is `Exception>>signal`, the `MessageNotUnderstood` it signals says
    /// (`is_finding_its_handler`); before it -- building the exception --
    /// or after it has returned, it is the machinery.
    fn does_not_understand_is_finding_its_handler(
        &self,
        above: Option<ActivationReference>,
    ) -> bool {
        let Some(signal) = above else {
            return true;
        };
        let exception_class = self.builtin_class(BuiltinClass::Exception);
        let is_signal = matches!(self.heap.activation_kind(signal),
            FrameKind::Method(method) if method.selector == Symbol::SIGNAL)
            && self
                .heap
                .activation_current_class(signal)
                .is_some_and(|class| class.same(exception_class));
        !is_signal || self.is_finding_its_handler(self.heap.activation_receiver(signal))
    }

    /// The error an unhandled `MessageNotUnderstood` would have ended the
    /// run with, for the kernel's `doesNotUnderstand:` activation: the
    /// selector and receiver of the send nothing understood, and the text
    /// `Object>>doesNotUnderstand:` gives it. Read from the activation's
    /// argument, the `Message` (a method's registers start with its
    /// arguments).
    fn does_not_understand_error(
        &self,
        activation: ActivationReference,
    ) -> Option<NewspeakError<'gc>> {
        let message = self.heap.activation_local(activation, 0).as_object()?;
        let selector = self
            .kernel_slot(message, BuiltinClass::Message, "selector")?
            .as_object()?;
        let selector = self.heap.symbol_value(selector)?;
        Some(NewspeakError::error(
            selector,
            self.heap.activation_receiver(activation),
            format!("an object does not understand #{}", selector.as_str()),
        ))
    }

    /// Whether `exception`'s `signal` is still in the kernel's own hands:
    /// the search for a handler -- after `signal` or a `pass` -- has not
    /// invoked one yet, or has found none and is running the default action.
    /// Read off the slots `Kernel.ns`'s `Exception` declares for it.
    fn is_finding_its_handler(&self, exception: Value<'gc>) -> bool {
        let Some(object) = exception.as_object() else {
            return false;
        };
        let slot = |name: &str| self.kernel_slot(object, BuiltinClass::Exception, name);
        slot("searching").and_then(|value| value.as_boolean()) == Some(true)
            || slot("reachedDefault").and_then(|value| value.as_boolean()) == Some(true)
    }

    /// Pushes what runs `block` -- an `ensure:` or `ifCurtailed:` argument
    /// the unwinder answered -- above the marked activation, which is the
    /// top of the stack, with the unwind to carry on as its continuation.
    ///
    /// A closure is activated as `Closure>>value` activates it. Anything else
    /// is sent `value`, as the normal path sends it, through the kernel's
    /// `Closure>>valueOnUnwind:`, whose activation is where the unwind carries
    /// on: what that send decides may be a method, a block, a value or an
    /// instantiation of several activations, and only the kernel method's
    /// own return is one place to hang the continuation on for all of them.
    /// Both are counted, alike under both lowerings.
    ///
    /// A push the depth limit refuses drops `then`, and the pending unwind
    /// with it: the run then ends with the depth error the refusal raised,
    /// which is the intended outcome -- a stack too deep to run the block
    /// on is too deep to carry the unwind on.
    pub(crate) fn start_unwind_block(
        &mut self,
        block: Value<'gc>,
        then: Box<PendingUnwind<'gc>>,
    ) -> Result<ActivationReference, Signal<'gc>> {
        if let Some(closure) = block.as_object() {
            if self.heap.is_closure(closure) {
                return self.push_step(
                    Step::Block {
                        closure,
                        arguments: StepArguments::Owned(Vec::new()),
                        then: Continuation::Unwind(then),
                    },
                    &[],
                    None,
                    false,
                );
            }
        }
        let marked = self
            .heap
            .top_activation()
            .expect("internal: the marked activation is the top of the stack");
        let receiver = self.heap.activation_receiver(marked);
        let block_class = self.builtin_class(BuiltinClass::Closure);
        let method = self
            .heap
            .class_application(block_class)
            .and_then(|application| {
                application
                    .mixin
                    .compiled_method(Symbol::VALUE_ON_UNWIND)
                    .cloned()
            })
            .expect("internal: Kernel.ns's Closure declares valueOnUnwind:");
        let activation = self.push_step(
            Step::Method {
                receiver,
                method,
                defining_class: block_class,
                arguments: StepArguments::Owned(vec![block]),
                feedback_base: super::FeedbackBase::NOT_ASSIGNED,
            },
            &[],
            None,
            false,
        )?;
        self.heap
            .activation_set_continuation(activation, Continuation::Unwind(then));
        Ok(activation)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::{unwind, UnwindOutcome};
    use crate::bytecode::{CodeKind, CompiledCode, Instruction};
    use crate::heap::ActivationHeader;
    use crate::interpreter::FeedbackBase;
    use crate::types::*;
    use crate::Interpreter;

    /// A body that returns its first register: nothing here runs it.
    fn trivial_code() -> Rc<CompiledCode> {
        Rc::new(CompiledCode::new(
            CodeKind::Method,
            vec![Instruction::Return { register: 0 }],
            0,
            1,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        ))
    }

    /// An activation that catches a return, as a method's does: a
    /// superclass clause's, which needs no method to build.
    fn header<'gc>(class: ClassReference<'gc>) -> ActivationHeader<'gc> {
        ActivationHeader {
            receiver: Value::nil(),
            current_class: Some(class),
            kind: FrameKind::SuperclassClause(class),
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        }
    }

    /// A return aimed at a frame whose activation has been popped must not
    /// land in the activation that took its stack index since: the frame
    /// was divorced on the pop and is married to nothing, so the unwind
    /// goes on to the floor. A release build has no generation to tell the
    /// two activations apart by, which is why the target is a frame.
    #[test]
    fn a_target_survives_its_index_being_reused() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let floor = mutator.heap.activation_depth();
            let target = mutator.heap.push_activation(header(class), &[], 1);
            let frame = mutator.heap.reify(target);
            mutator.heap.pop_activation(target);
            let successor = mutator.heap.push_activation(header(class), &[], 1);
            assert_eq!(
                mutator.heap.activation_position(successor),
                floor,
                "the successor reuses the target's index"
            );
            let outcome = unwind(
                mutator,
                PendingUnwind {
                    target: Some(frame),
                    action: UnwindAction::Return(Value::small_integer(1)),
                },
                floor,
            );
            assert!(
                matches!(outcome, UnwindOutcome::Exit(Err(_))),
                "the unwind must not arrive at the successor"
            );
            assert_eq!(mutator.heap.activation_depth(), floor);
        });
    }

    /// The same stack with the target still running: the unwind arrives at
    /// it, having popped what is above it.
    #[test]
    fn a_running_target_is_arrived_at() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let floor = mutator.heap.activation_depth();
            let target = mutator.heap.push_activation(header(class), &[], 1);
            let frame = mutator.heap.reify(target);
            mutator.heap.push_activation(header(class), &[], 1);
            let outcome = unwind(
                mutator,
                PendingUnwind {
                    target: Some(frame),
                    action: UnwindAction::Return(Value::small_integer(1)),
                },
                floor,
            );
            let UnwindOutcome::Returned {
                target: arrived, ..
            } = outcome
            else {
                panic!("the unwind must arrive at its running target");
            };
            assert!(arrived.same(target));
            mutator.heap.pop_activation(target);
            assert_eq!(mutator.heap.activation_depth(), floor);
        });
    }

    /// A restart arrives at its target as a return does, and leaves it at
    /// its first instruction; a termination, which has no target, pops to
    /// the floor and leaves as the error it carries.
    #[test]
    fn a_restart_rewinds_its_target_and_a_termination_exits_with_its_error() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let floor = mutator.heap.activation_depth();
            let target = mutator.heap.push_activation(header(class), &[], 1);
            mutator.heap.activation_set_program_counter(target, 5);
            let frame = mutator.heap.reify(target);
            mutator.heap.push_activation(header(class), &[], 1);
            let outcome = unwind(
                mutator,
                PendingUnwind {
                    target: Some(frame),
                    action: UnwindAction::Restart,
                },
                floor,
            );
            let UnwindOutcome::Restarted { target: arrived } = outcome else {
                panic!("the restart must arrive at its running target");
            };
            assert!(arrived.same(target));
            assert_eq!(mutator.heap.activation_program_counter(target), 0);
            let outcome = unwind(
                mutator,
                PendingUnwind {
                    target: None,
                    action: UnwindAction::Terminate(NewspeakError::error(
                        crate::Symbol::intern("signal"),
                        Value::nil(),
                        "boom",
                    )),
                },
                floor,
            );
            let UnwindOutcome::Exit(Err(crate::interpreter::Signal::Error(error))) = outcome else {
                panic!("a termination must exit with its error");
            };
            assert_eq!(&*error.message, "boom");
            assert_eq!(mutator.heap.activation_depth(), floor);
        });
    }

    /// The depth limit's reserve belongs to the activation it was taken
    /// for, not to a stack index: once that activation and its `signal`
    /// are gone, another error whose `signal` lands at the very same index
    /// -- which exempt block activations make possible far from the limit
    /// -- does not find the reserve held.
    #[test]
    fn a_later_signal_at_the_same_index_does_not_inherit_the_reserve() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let signalling = |class| ActivationHeader {
                then: Continuation::VmErrorSignalled(Box::new(NewspeakError::error(
                    crate::Symbol::SIGNAL,
                    Value::nil(),
                    "boom",
                ))),
                ..header(class)
            };
            let raised = mutator.heap.push_activation(header(class), &[], 1);
            mutator.depth_reserve = Some(mutator.heap.reify(raised));
            let signal = mutator.heap.push_activation(signalling(class), &[], 1);
            assert!(mutator.depth_reserve_held(), "held while its signal runs");
            let signal_position = mutator.heap.activation_position(signal);
            mutator.heap.pop_activation(signal);
            mutator.heap.pop_activation(raised);
            let other = mutator.heap.push_activation(header(class), &[], 1);
            let other_signal = mutator.heap.push_activation(signalling(class), &[], 1);
            assert_eq!(
                mutator.heap.activation_position(other_signal),
                signal_position,
                "the second signal takes the first one's index"
            );
            assert!(
                !mutator.depth_reserve_held(),
                "the second signal must not inherit the reserve"
            );
            assert!(mutator.depth_reserve.is_none(), "and it is released");
            mutator.heap.pop_activation(other_signal);
            mutator.heap.pop_activation(other);
        });
    }

    #[test]
    fn an_activation_that_has_returned_is_refused_as_a_cannot_return() {
        // No golden reaches a restart whose activation has returned.
        let error = super::dead_activation_error(crate::Symbol::intern("restart"), Value::nil());
        assert_eq!(error.class, ErrorClass::CannotReturn);
    }
}
