//! The one unwinder (`docs/design/core-library.md`, "Planned: exceptions",
//! and `docs/design/execution.md`, "Non-local return"): what a signal
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
use crate::heap::ActivationReference;
use crate::types::*;
use crate::Symbol;

use super::{EvaluationResult, Mutator, Signal, Step, StepArguments};

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
    /// Popped down to the run's floor: the run ends with this.
    Exit(EvaluationResult<'gc>),
}

/// A signal leaving the running activation. A non-local return is an
/// unwind to its home, a restart an unwind to the `on:do:` activation it
/// restarts, and a termination an unwind with no target, which runs every
/// pending block on its way to the floor (`unwind`). An error pops every
/// activation of the run, `base` included, and leaves the run with the
/// signal for the host that started it (`Interpreter::run_from_host`),
/// running no block: an error that is not yet a signalled exception ends
/// the run as it did before there were any.
pub(crate) fn unwind_signal<'gc>(
    mutator: &mut Mutator<'gc>,
    signal: Signal<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
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
            action: UnwindAction::Terminate(error),
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
/// known gap in `docs/design/execution.md`, "Non-local return") -- and a
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
    if !matches!(
        mutator.heap.activation_local(activation, complete_register),
        Value::Nil
    ) {
        return None;
    }
    mutator
        .heap
        .activation_set_local(activation, complete_register, Value::Boolean(true));
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
            mutator.heap.set_activation_program_counter(target, 0);
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
        UnwindAction::Terminate(error) => Signal::Terminate(error),
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
        UnwindAction::Restart => Signal::Error(dead_activation_error(Symbol::intern("restart"))),
        UnwindAction::Return(_) => signal_of(pending),
    }
}

/// §7.2's `cannotReturn` for an activation that has already returned,
/// named after the leaf that aimed at it.
pub(crate) fn dead_activation_error<'gc>(selector: Symbol) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver: Value::Nil,
        message: "cannotReturn: the activation has already returned".to_string(),
    }
}

impl<'gc> Mutator<'gc> {
    /// Pushes what runs `block` -- an `ensure:` or `ifCurtailed:` argument
    /// the unwinder answered -- above the marked activation, which is the
    /// top of the stack, with the unwind to carry on as its continuation.
    ///
    /// A closure is activated as `Block>>value` activates it. Anything else
    /// is sent `value`, as the normal path sends it, through the kernel's
    /// `Block>>valueOnUnwind:`, whose activation is where the unwind carries
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
        if let Value::Object(closure) = block {
            if self.heap.closure_activation(closure).is_some() {
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
        let block_class = self.builtin_class(BuiltinClass::Block);
        let method = self
            .heap
            .class_application(block_class)
            .and_then(|application| {
                application
                    .mixin
                    .compiled_method(Symbol::VALUE_ON_UNWIND)
                    .cloned()
            })
            .expect("internal: Kernel.ns's Block declares valueOnUnwind:");
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
            .set_activation_continuation(activation, Continuation::Unwind(then));
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
        Rc::new(CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![Instruction::Return { register: 0 }],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: Vec::new(),
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: ns_syntax::Span::new(0, 0),
        })
    }

    /// An activation that catches a return, as a method's does: a
    /// superclass clause's, which needs no method to build.
    fn header<'gc>(class: ClassReference<'gc>) -> ActivationHeader<'gc> {
        ActivationHeader {
            receiver: Value::Nil,
            current_class: Some(class),
            kind: FrameKind::SuperclassClause(class),
            slot_base: 0,
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
                    action: UnwindAction::Return(Value::SmallInteger(1)),
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
                    action: UnwindAction::Return(Value::SmallInteger(1)),
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
            mutator.heap.set_activation_program_counter(target, 5);
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
                    action: UnwindAction::Terminate(NewspeakError {
                        selector: crate::Symbol::intern("signal"),
                        receiver: Value::Nil,
                        message: "boom".to_string(),
                    }),
                },
                floor,
            );
            let UnwindOutcome::Exit(Err(crate::interpreter::Signal::Error(error))) = outcome else {
                panic!("a termination must exit with its error");
            };
            assert_eq!(error.message, "boom");
            assert_eq!(mutator.heap.activation_depth(), floor);
        });
    }
}
