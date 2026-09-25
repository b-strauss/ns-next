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
    /// does what `action` says -- returns the value.
    Arrived {
        target: ActivationReference,
        action: UnwindAction<'gc>,
    },
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
/// unwind to its home (`unwind`). An error pops every activation of the run,
/// `base` included, and leaves the run with the signal for the host that
/// started it (`Interpreter::run_from_host`), as it did before there was
/// anything to run on the way: an `ensure:` block does not run for it until
/// errors are signalled exceptions, whose termination runs them after the
/// error is reported.
pub(crate) fn unwind_signal<'gc>(
    mutator: &mut Mutator<'gc>,
    signal: Signal<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
    match signal {
        Signal::Return(target, value) => unwind(
            mutator,
            PendingUnwind {
                target: Some(target),
                action: UnwindAction::Return(value),
            },
            floor,
        ),
        error @ Signal::Error(_) => {
            while mutator.heap.activation_depth() > floor {
                let top = mutator
                    .heap
                    .top_activation()
                    .expect("internal: a stack deeper than the floor has a top");
                let popped = mutator.heap.pop_activation(top);
                mutator.abandon(popped.then);
            }
            UnwindOutcome::Exit(Err(error))
        }
    }
}

/// Carries `pending` on from the top of the stack. A target that is still
/// running (`Heap::frame_activation`), sits at or above the run's floor and
/// catches a return (`Mutator::catches_return`: a method, or a superclass
/// clause or its message) ends the unwind: everything above it is popped,
/// and it returns the value through the ordinary return path. Any other
/// target -- a home that has returned meanwhile, or a running initializer,
/// which catches none (the known gap in `docs/design/execution.md`,
/// "Non-local return") -- pops every activation of the run, `base`
/// included, and the run leaves with the signal, still naming it, for the
/// host.
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
            return UnwindOutcome::Exit(Err(signal_of(pending)));
        }
        let top = mutator
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        if let Some(target) = destination {
            if top.same(target) {
                return UnwindOutcome::Arrived {
                    target,
                    action: pending.action,
                };
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

/// The signal `pending` was, raised again: how an unwind carries on once a
/// block it ran has returned (`Mutator::resume`), and how it leaves the run
/// when it reaches the floor.
pub(crate) fn signal_of(pending: PendingUnwind<'_>) -> Signal<'_> {
    match pending.action {
        UnwindAction::Return(value) => {
            let Some(target) = pending.target else {
                unreachable!("internal: a non-local return always names its home")
            };
            Signal::Return(target, value)
        }
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
            let UnwindOutcome::Arrived {
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
}
