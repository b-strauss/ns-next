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

use super::Arguments;
use super::breakpoint;
use crate::Symbol;
use crate::bytecode::UnwindMarker;
use crate::heap::{ActivationHeader, ActivationReference, FrameReference, Heap};
use crate::types::*;

use super::{DEPTH_RESERVE, EvaluationResult, Mutator, Signal, Step, StepArguments};

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
    /// Popped the base of an expression the debugger evaluated, which an
    /// error ended (`at_evaluation_base`): the loop goes on with `stopped`,
    /// the frame below, from the program counter it stopped at, where the
    /// safepoint the end asked for re-enters the stop.
    EvaluationEnded { stopped: ActivationReference },
}

/// An activation a walk from the top of the stack towards the run's base
/// meets: the live frames first, then -- past a frame whose sender is
/// suspended (`Continuation::ReturnToSuspended`) -- the frames the
/// suspended activations live in, each naming its sender. The depth
/// limit's reserve walks it (`Mutator::overrun_of_the_reserve`,
/// `Mutator::depth_reserve_held`) without reifying a live frame; neither
/// is on a hot path.
#[derive(Clone, Copy)]
pub(crate) enum WalkedActivation<'gc> {
    /// An activation on the stack.
    Live(ActivationReference),
    /// An activation suspended into its frame.
    Suspended(FrameReference<'gc>),
}

impl<'gc> WalkedActivation<'gc> {
    /// The activation this one returns to: the suspended sender of a live
    /// frame that has one, else the frame below; a suspended one's sender
    /// frame. `None` for the run's base and the bottom of the stack.
    /// Inline into every walk: out of line, it answered the next step
    /// through memory, written as a tag and a word and read back as one
    /// wide load, a store-forwarding stall on every step of a handler's
    /// search (`builtins::activation`'s `next_active_handler`).
    #[inline(always)]
    pub(crate) fn sender(self, heap: &Heap<'gc>) -> Option<WalkedActivation<'gc>> {
        match self {
            WalkedActivation::Live(activation) => {
                match heap.activation_suspended_sender(activation) {
                    Some(sender) => Some(WalkedActivation::Suspended(sender)),
                    None => heap
                        .activation_sender(activation)
                        .map(WalkedActivation::Live),
                }
            }
            WalkedActivation::Suspended(frame) => {
                heap.frame_sender(frame).map(WalkedActivation::Suspended)
            }
        }
    }

    /// Whether this is the running activation of `frame`.
    pub(crate) fn is_activation_of(self, heap: &Heap<'gc>, frame: FrameReference<'gc>) -> bool {
        match self {
            WalkedActivation::Live(activation) => heap
                .frame_activation(frame)
                .is_some_and(|married| married.same(activation)),
            WalkedActivation::Suspended(suspended) => suspended.same(frame),
        }
    }

    /// The activation's receiver.
    pub(crate) fn receiver(self, heap: &Heap<'gc>) -> Value<'gc> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_receiver(activation),
            WalkedActivation::Suspended(frame) => heap.frame_receiver(frame),
        }
    }

    /// What the activation was activated for.
    pub(crate) fn kind(self, heap: &Heap<'gc>) -> ActivationKind<'gc> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_kind(activation),
            WalkedActivation::Suspended(frame) => heap.frame_kind(frame).activation_kind(),
        }
    }

    /// The class whose code the activation runs, if any.
    pub(crate) fn current_class(self, heap: &Heap<'gc>) -> Option<ClassReference<'gc>> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_current_class(activation),
            WalkedActivation::Suspended(frame) => heap.frame_current_class(frame),
        }
    }

    /// Register `index` of the activation.
    pub(crate) fn local(self, heap: &Heap<'gc>, index: u16) -> Value<'gc> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_local(activation, index),
            WalkedActivation::Suspended(frame) => heap.frame_local(frame, index),
        }
    }

    /// The running activation of `frame`: the stack frame it is married
    /// to, or the frame itself when it is suspended. `None` once its
    /// activation has returned.
    pub(crate) fn of_running_frame(
        heap: &Heap<'gc>,
        frame: FrameReference<'gc>,
    ) -> Option<WalkedActivation<'gc>> {
        match heap.frame_activation(frame) {
            Some(activation) => Some(WalkedActivation::Live(activation)),
            None => heap
                .frame_is_suspended(frame)
                .then_some(WalkedActivation::Suspended(frame)),
        }
    }

    /// The unwind marker of the code the activation runs, if it carries
    /// one.
    pub(crate) fn unwind_marker(self, heap: &Heap<'gc>) -> Option<UnwindMarker> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_unwind_marker(activation),
            WalkedActivation::Suspended(frame) => heap.frame_unwind_marker(frame),
        }
    }

    /// The activation's frame object: a live one reified on the first
    /// call and the same object on every later one, a suspended one the
    /// frame it lives in.
    pub(crate) fn reified(self, heap: &mut Heap<'gc>) -> FrameReference<'gc> {
        match self {
            WalkedActivation::Live(activation) => heap.reify(activation),
            WalkedActivation::Suspended(frame) => frame,
        }
    }

    /// The VM error the activation is the `signal` of, if the unwinder
    /// pushed it to signal one (`Continuation::VmErrorSignalled`).
    pub(crate) fn signalled_vm_error(self, heap: &Heap<'gc>) -> Option<NewspeakError<'gc>> {
        match self {
            WalkedActivation::Live(activation) => heap.activation_signalled_vm_error(activation),
            WalkedActivation::Suspended(frame) => heap.frame_signalled_vm_error(frame),
        }
    }

    /// Whether the activation is the base of an expression the debugger
    /// evaluates (`Continuation::Evaluated`).
    pub(crate) fn is_evaluation_base(self, heap: &Heap<'gc>) -> bool {
        match self {
            WalkedActivation::Live(activation) => heap.activation_is_evaluation_base(activation),
            WalkedActivation::Suspended(frame) => heap.frame_is_evaluation_base(frame),
        }
    }

    /// `signalled_vm_error` without cloning the error of a live frame.
    pub(crate) fn signals_a_vm_error(self, heap: &Heap<'gc>) -> bool {
        match self {
            WalkedActivation::Live(activation) => heap.activation_signals_a_vm_error(activation),
            WalkedActivation::Suspended(frame) => heap.frame_signalled_vm_error(frame).is_some(),
        }
    }
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
    if let Signal::Error(error) = &signal
        && let Some(outcome) = start_signalling(mutator, error, floor)
    {
        return outcome;
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
            // Popped as the unwind below pops, so that every suspended
            // activation of the run comes back onto the stack on the way
            // down and none outlives the run.
            while mutator.heap.activation_stack_top() > floor {
                let top = mutator
                    .heap
                    .top_activation()
                    .expect("internal: a stack deeper than the floor has a top");
                mutator.pop_for_unwind(top);
            }
            breakpoint::unwind_stopped(mutator);
            return UnwindOutcome::Exit(Err(error));
        }
    };
    unwind(mutator, pending, floor)
}

/// Carries `pending` on from the top of the stack. A target that is still
/// running (`Heap::frame_is_running`: on the stack, or suspended into its
/// frame until the pops resume it), that the pops reach as the top
/// (`Heap::frame_activation`, asked again before every pop), at or above
/// the run's floor, and that catches a return (`Mutator::catches_return`: a
/// method, or a superclass clause or its message) ends the unwind: everything
/// above it is popped, and it returns the value through the ordinary return
/// path, or runs again from its start for a restart. Any other target -- a home that has
/// returned meanwhile, or a running initializer, which catches none (the
/// known gap in `docs/design/execution.md`, "Unwinding") -- and a
/// termination, which has none, pop every activation of the run, `base`
/// included, and the run leaves with what `exit_signal` makes of the
/// unwind.
///
/// The target is found by its frame, not by an activation handle, because
/// the blocks this runs push activations while the unwind is in progress:
/// one of them can sit at the index a popped target used, which a frame --
/// divorced when its activation was popped -- cannot be mistaken for. And
/// a target suspended into its frame (`Heap::frame_is_suspended`) becomes
/// a stack frame only once the pops reach it, each pop resuming the
/// suspended sender of what it pops (`Mutator::pop_for_unwind`); so the
/// destination is decided again from the frame before every pop.
///
/// Every activation it pops hands its continuation to `Mutator::abandon`,
/// which undoes what a continuation that will now never run had begun.
///
/// Whether the debugger hears of the pops is decided once, before the
/// first: only a step in progress or a method breakpoint wants them
/// (`breakpoint::follows_unwinds`), and nothing the pops do starts either
/// unless one of them already is, so an unwind with neither pays one test
/// rather than several for every frame it pops. A followed unwind tells
/// the step where it stops popping, once (`breakpoint::unwind_stopped`):
/// only that frame is flooded, never one it passes.
pub(crate) fn unwind<'gc>(
    mutator: &mut Mutator<'gc>,
    pending: PendingUnwind<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
    if breakpoint::follows_unwinds(mutator) {
        unwind_from::<true>(mutator, pending, floor)
    } else {
        unwind_from::<false>(mutator, pending, floor)
    }
}

/// `unwind`, the debugger's reports made only when `FOLLOWED`.
fn unwind_from<'gc, const FOLLOWED: bool>(
    mutator: &mut Mutator<'gc>,
    mut pending: PendingUnwind<'gc>,
    floor: usize,
) -> UnwindOutcome<'gc> {
    loop {
        if mutator.heap.activation_stack_top() <= floor {
            if FOLLOWED {
                breakpoint::unwind_stopped(mutator);
            }
            return UnwindOutcome::Exit(Err(exit_signal(pending)));
        }
        let top = mutator
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        // Only a followed unwind can meet an evaluation's base: one runs
        // only while the debugger follows unwinds.
        if FOLLOWED
            && mutator.breakpoints.evaluating()
            && mutator.heap.activation_is_evaluation_base(top)
        {
            pending = match at_evaluation_base(mutator, top, pending, floor) {
                Ok(outcome) => return outcome,
                Err(pending) => pending,
            };
            // A condition's evaluation at a method's exit by an unwind
            // ended with an error: its base is popped, and the unwind it
            // interrupted goes on from the frame below.
            if !mutator
                .heap
                .top_activation()
                .is_some_and(|now| now.same(top))
            {
                continue;
            }
        }
        let arrived = pending
            .target
            .and_then(|frame| mutator.heap.frame_activation(frame))
            .is_some_and(|target| {
                top.same(target)
                    && mutator.heap.activation_position(target) >= floor
                    && mutator.catches_return(target)
            });
        if arrived {
            // A method a return's unwind arrives at returns without a
            // return instruction of its own, whose exit a method
            // breakpoint reports (`breakpoint::returning_by_unwind`),
            // decided in it first.
            if FOLLOWED {
                let mut returned = Vec::new();
                if let (UnwindAction::Return(_), Some(frame)) = (&pending.action, pending.target) {
                    (returned, pending) =
                        match breakpoint::returning_by_unwind(mutator, top, frame, pending) {
                            Ok(decided) => decided,
                            Err(outcome) => return outcome,
                        };
                }
                breakpoint::unwind_stopped(mutator);
                breakpoint::returned_by_unwind(mutator, top, returned);
            }
            return arrive(mutator, top, pending.action);
        }
        if let Some(block) = block_to_run(mutator, top) {
            if FOLLOWED {
                breakpoint::unwind_stopped(mutator);
            }
            return UnwindOutcome::RunBlock {
                block,
                then: Box::new(pending),
            };
        }
        if FOLLOWED {
            pending = match pop_followed(mutator, top, pending) {
                Ok(pending) => pending,
                Err(outcome) => return outcome,
            };
        } else {
            mutator.pop_for_unwind_as::<false>(top);
        }
    }
}

/// An unwind reached `base`, the base of an expression the debugger
/// evaluates, on top of the stack: no unwind from within the expression
/// crosses it, so that the program's frames below are untouched
/// (`docs/design/debugger.md`, "Evaluation", "Its errors"). A termination
/// -- an exception's default action ended the run -- pops the base and
/// ends the evaluation with its error, the program staying stopped; a
/// return or a restart aims at a frame below, which it cannot reach from
/// the evaluation, and so signals `CannotReturn` from the base, as a `^`
/// to a home that has returned does. Only the debugger's own termination
/// goes on past the base (`Err`), to the run's floor.
#[cold]
#[inline(never)]
fn at_evaluation_base<'gc>(
    mutator: &mut Mutator<'gc>,
    base: ActivationReference,
    pending: PendingUnwind<'gc>,
    floor: usize,
) -> Result<UnwindOutcome<'gc>, PendingUnwind<'gc>> {
    if mutator.breakpoints.terminating() {
        return Err(pending);
    }
    let error = match pending.action {
        UnwindAction::Terminate(error) => return end_at_base(mutator, base, &error),
        UnwindAction::Return(value) => NewspeakError::cannot_return(
            Symbol::CARET,
            value,
            value,
            "cannotReturn: the enclosing method activation is outside the evaluation",
        ),
        UnwindAction::Restart => dead_activation_error(Symbol::intern("restart"), Value::nil()),
    };
    match start_signalling(mutator, &error, floor) {
        Some(outcome) => Ok(outcome),
        None => end_at_base(mutator, base, &error),
    }
}

/// `error` ends the evaluation whose base, `base`, is on top: the base is
/// popped and the evaluation answers the error. One a stop was kept for
/// leaves the frame below stopped, where the safepoint re-enters the stop
/// (`UnwindOutcome::EvaluationEnded`); one of a condition at a method's
/// exit by an unwind decides the exit (`conditions::exit_ended`), and the
/// unwind it interrupted goes on (`Err`), from the frame it was about to
/// pop.
fn end_at_base<'gc>(
    mutator: &mut Mutator<'gc>,
    base: ActivationReference,
    error: &NewspeakError<'gc>,
) -> Result<UnwindOutcome<'gc>, PendingUnwind<'gc>> {
    let interrupted = mutator.heap.activation_interrupted_unwind(base);
    mutator.pop_for_unwind(base);
    match interrupted {
        Some(interrupted) => {
            let outcome = crate::debugger::evaluation::exit_ended_by(mutator, error);
            crate::debugger::conditions::exit_ended(mutator, outcome);
            Err(interrupted)
        }
        None => {
            let stopped = crate::debugger::evaluation::ended_by(mutator, error);
            Ok(UnwindOutcome::EvaluationEnded { stopped })
        }
    }
}

/// Pops `top` for `pending`, an unwind the debugger follows, and answers
/// the unwind to carry on. A method the unwind pops is left without a
/// return, whose exit a method breakpoint reports, decided in `top` before
/// it is popped -- its condition evaluated there, the outcome answered
/// instead while that evaluation runs above `top`, which stays; a
/// termination, which ends the run, reports none.
fn pop_followed<'gc>(
    mutator: &mut Mutator<'gc>,
    top: ActivationReference,
    pending: PendingUnwind<'gc>,
) -> Result<PendingUnwind<'gc>, UnwindOutcome<'gc>> {
    let (exits, pending) = if matches!(pending.action, UnwindAction::Terminate(_)) {
        (Vec::new(), pending)
    } else {
        breakpoint::leaving_by_unwind(mutator, top, pending)?
    };
    mutator.pop_for_unwind(top);
    if !exits.is_empty() {
        breakpoint::left_by_unwind(mutator, exits);
    }
    Ok(pending)
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

/// The `NonBooleanReceiver` a loop signals when its condition answers
/// `value`, which is no Boolean; one error for the inlined loop and the
/// native one alike.
pub(crate) fn non_boolean_condition<'gc>(
    heap: &Heap<'gc>,
    value: Value<'gc>,
) -> NewspeakError<'gc> {
    NewspeakError::non_boolean_receiver(
        Symbol::VALUE,
        value,
        format!(
            "a loop's condition answered {}, which is no Boolean",
            heap.describe(value)
        ),
    )
}

/// Starts signalling `error` as an `Error`, once the kernel is installed:
/// `signal`'s activation, pushed above the top of the stack -- the
/// activation where the error arose -- with the continuation that ends the
/// run should `signal` ever return (`Continuation::VmErrorSignalled`).
///
/// An error raised within `DEPTH_RESERVE` activations of the depth limit
/// -- the raised one of an expression the debugger evaluates, while it
/// runs (`Mutator::ordinary_depth_limit`) --, the limit's own error
/// included, takes the reserve first, unless a
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
        && (mutator.heap.counted_depth() >= mutator.ordinary_depth_limit() - DEPTH_RESERVE
            || mutator.heap.stack_is_within_the_reserve_of_its_limit());
    let sender = mutator.heap.top_activation();
    let taking = taking && sender.is_some();
    if let (true, Some(top)) = (taking, sender) {
        let frame = mutator.heap.reify(top);
        mutator.set_depth_reserve(Some(frame));
        mutator.taking_depth_reserve = true;
    }
    let pushed = mutator.push_step(step, Arguments::NONE, sender, false);
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
                mutator.set_depth_reserve(None);
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
        let kind = error.class.builtin();
        let subject_slot = match error.class {
            ErrorClass::Error => None,
            ErrorClass::ArgumentError => Some("argument"),
            ErrorClass::CannotReturn => Some("result"),
            ErrorClass::NonBooleanReceiver => Some("receiver"),
            ErrorClass::CannotInstantiate => Some("behavior"),
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
        self.decide_send(Value::object(exception), Symbol::SIGNAL, Arguments::NONE)
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
    /// Walks the activations from the top of the stack towards the run's
    /// base, the suspended ones included (`WalkedActivation::sender`): once
    /// per run, since an overrun ends it. Inside an expression the debugger
    /// evaluates, the overrun ends the evaluation, so the walk stops at its
    /// base: an error the program was signalling below it when it stopped
    /// is not the evaluation's.
    pub(crate) fn overrun_of_the_reserve(
        &self,
        header: &ActivationHeader<'gc>,
    ) -> NewspeakError<'gc> {
        // Only the outermost is reported, so a `doesNotUnderstand:`
        // activation is only checked on the way and its error built once,
        // after the walk.
        let mut reported = None;
        let mut above = None;
        let mut current = self.heap.top_activation().map(WalkedActivation::Live);
        while let Some(activation) = current {
            if self.breakpoints.evaluating() && activation.is_evaluation_base(&self.heap) {
                break;
            }
            if let Some(error) = activation.signalled_vm_error(&self.heap) {
                if !self.is_finding_its_handler(activation.receiver(&self.heap)) {
                    break;
                }
                reported = Some(Reported::Error(error));
            } else if self.is_kernel_does_not_understand(activation) {
                if !self.does_not_understand_is_finding_its_handler(above)
                    || self.does_not_understand_selector(activation).is_none()
                {
                    break;
                }
                reported = Some(Reported::DoesNotUnderstand(activation));
            }
            above = Some(activation);
            current = activation.sender(&self.heap);
        }
        match reported {
            Some(Reported::Error(error)) => error,
            Some(Reported::DoesNotUnderstand(activation)) => self
                .does_not_understand_error(activation)
                .unwrap_or_else(|| super::recursion_too_deep_error(header)),
            None => super::recursion_too_deep_error(header),
        }
    }

    /// Whether a kernel `doesNotUnderstand:` activation is still in the
    /// kernel's own hands. `above` is the activation directly above it, the
    /// one it sent to: while that
    /// is `Exception>>signal`, the `MessageNotUnderstood` it signals says
    /// (`is_finding_its_handler`); before it -- building the exception --
    /// or after it has returned, it is the machinery.
    fn does_not_understand_is_finding_its_handler(
        &self,
        above: Option<WalkedActivation<'gc>>,
    ) -> bool {
        let Some(signal) = above else {
            return true;
        };
        let exception_class = self.builtin_class(BuiltinClass::Exception);
        let is_signal = matches!(signal.kind(&self.heap),
            ActivationKind::Method(selector) if selector == Symbol::SIGNAL)
            && signal
                .current_class(&self.heap)
                .is_some_and(|class| class.same(exception_class));
        !is_signal || self.is_finding_its_handler(signal.receiver(&self.heap))
    }

    /// The selector of the `Message` a kernel `doesNotUnderstand:`
    /// activation holds, when it is a Symbol (a method's registers start
    /// with its arguments).
    fn does_not_understand_selector(
        &self,
        activation: WalkedActivation<'gc>,
    ) -> Option<ObjectReference<'gc>> {
        let message = activation.local(&self.heap, 0).as_object()?;
        self.kernel_slot(message, BuiltinClass::Message, "selector")?
            .as_object()
            .filter(|selector| self.heap.is_symbol(*selector))
    }

    /// The error an unhandled `MessageNotUnderstood` would have ended the
    /// run with, for the kernel's `doesNotUnderstand:` activation: the
    /// selector and receiver of the send nothing understood, and the text
    /// `Object>>doesNotUnderstand:` gives it. Read from the activation's
    /// argument, the `Message` (a method's registers start with its
    /// arguments).
    ///
    /// A program-made selector whose text nothing has interned is reported
    /// as `doesNotUnderstand:`'s `Symbol`, the text naming it: a run never
    /// interns a text a program chose, or a host that keeps one interpreter
    /// would grow the interner by one text per failed run.
    fn does_not_understand_error(
        &self,
        activation: WalkedActivation<'gc>,
    ) -> Option<NewspeakError<'gc>> {
        let selector = self.does_not_understand_selector(activation)?;
        let text = self.heap.symbol_text(selector)?;
        let selector = self
            .heap
            .symbol_value(selector)
            .unwrap_or(Symbol::DOES_NOT_UNDERSTAND);
        Some(NewspeakError::error(
            selector,
            activation.receiver(&self.heap),
            format!("an object does not understand #{text}"),
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
        if let Some(closure) = block.as_object()
            && self.heap.is_closure(closure)
        {
            return self.push_step(
                Step::Block {
                    closure,
                    arguments: StepArguments::Owned(Vec::new()),
                    then: Continuation::Unwind(then),
                },
                Arguments::NONE,
                None,
                false,
            );
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
            Arguments::NONE,
            None,
            false,
        )?;
        self.heap
            .activation_set_continuation(activation, Continuation::Unwind(then));
        Ok(activation)
    }
}

/// What the walk past the depth limit reports: a VM error signalled on
/// the way, or the kernel `doesNotUnderstand:` activation whose error is
/// built once the walk has found the outermost.
enum Reported<'gc> {
    Error(NewspeakError<'gc>),
    DoesNotUnderstand(WalkedActivation<'gc>),
}

#[cfg(test)]
mod tests {
    use crate::interpreter::Arguments;
    use std::rc::Rc;

    use super::{UnwindOutcome, unwind};
    use crate::Interpreter;
    use crate::bytecode::{CodeKind, CompiledCode, Instruction};
    use crate::heap::ActivationHeader;
    use crate::interpreter::FeedbackBase;
    use crate::types::*;

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
            kind: ActivationKind::SuperclassClause(class),
            method: None,
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            code: trivial_code(),
            then: Continuation::Deliver,
            counted: true,
        }
    }

    /// A return aimed at a frame whose activation has been popped must not
    /// land in the activation that took its stack offset since: the frame
    /// was divorced on the pop and is married to nothing, so the unwind
    /// goes on to the floor. A release build has no generation to tell the
    /// two activations apart by, which is why the target is a frame.
    #[test]
    fn a_target_survives_its_index_being_reused() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let floor = mutator.heap.activation_stack_top();
            let target = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let frame = mutator.heap.reify(target);
            mutator.heap.pop_activation(target);
            let successor = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
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
            assert_eq!(mutator.heap.activation_stack_top(), floor);
        });
    }

    /// The same stack with the target still running: the unwind arrives at
    /// it, having popped what is above it.
    #[test]
    fn a_running_target_is_arrived_at() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let floor = mutator.heap.activation_stack_top();
            let target = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let frame = mutator.heap.reify(target);
            mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
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
            assert_eq!(mutator.heap.activation_stack_top(), floor);
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
            let floor = mutator.heap.activation_stack_top();
            let target = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            mutator.heap.activation_set_program_counter(target, 5);
            let frame = mutator.heap.reify(target);
            mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
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
            assert_eq!(mutator.heap.activation_stack_top(), floor);
        });
    }

    /// The depth limit's reserve belongs to the activation it was taken
    /// for, not to a stack offset: once that activation and its `signal`
    /// are gone, another error whose `signal` lands at the very same offset
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
            let raised = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let frame = mutator.heap.reify(raised);
            mutator.set_depth_reserve(Some(frame));
            let signal = mutator
                .heap
                .push_activation(signalling(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            assert!(mutator.depth_reserve_held(), "held while its signal runs");
            assert_eq!(
                mutator
                    .heap
                    .machine_word(crate::heap::machine_words::MachineWord::DepthLimit),
                (crate::interpreter::MAX_ACTIVATION_DEPTH + crate::interpreter::DEPTH_RESERVE)
                    as u64,
                "the limit word allows the reserve while it is held"
            );
            let signal_position = mutator.heap.activation_position(signal);
            mutator.heap.pop_activation(signal);
            mutator.heap.pop_activation(raised);
            let other = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let other_signal = mutator
                .heap
                .push_activation(signalling(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
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
            assert_eq!(
                mutator
                    .heap
                    .machine_word(crate::heap::machine_words::MachineWord::DepthLimit),
                crate::interpreter::MAX_ACTIVATION_DEPTH as u64,
                "the limit word is the limit again"
            );
            mutator.heap.pop_activation(other_signal);
            mutator.heap.pop_activation(other);
        });
    }

    /// The unwind that pops a `signal` which took the reserve gives the
    /// reserve back at that pop: the limit word is the limit again before
    /// anything asks whether the reserve is held.
    #[test]
    fn the_limit_word_is_exact_right_after_the_signals_pop() {
        use crate::heap::machine_words::MachineWord;
        let limit = |mutator: &crate::interpreter::Mutator| {
            mutator.heap.machine_word(MachineWord::DepthLimit)
        };
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let error = NewspeakError::error(crate::Symbol::SIGNAL, Value::nil(), "boom");
            let raised = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let frame = mutator.heap.reify(raised);
            mutator.set_depth_reserve(Some(frame));
            let signal = mutator
                .heap
                .push_activation(
                    ActivationHeader {
                        then: Continuation::VmErrorSignalled(Box::new(error)),
                        ..header(class)
                    },
                    Arguments::NONE,
                    1,
                )
                .expect("the stack holds the frame");
            assert_eq!(
                limit(mutator),
                (crate::interpreter::MAX_ACTIVATION_DEPTH + crate::interpreter::DEPTH_RESERVE)
                    as u64
            );
            mutator.pop_for_unwind(signal);
            assert_eq!(
                limit(mutator),
                crate::interpreter::MAX_ACTIVATION_DEPTH as u64
            );
            assert!(mutator.depth_reserve.is_none());
            mutator.heap.pop_activation(raised);
        });
    }

    /// A `signal` resumed over the suspended activation that took the
    /// reserve returns to it through `Continuation::ReturnToSuspended`,
    /// which wraps its `VmErrorSignalled`: the reserve is still held, the
    /// walk finds the `signal` above the suspended frame, and replacing the
    /// `signal`'s continuation keeps the return to the suspended sender.
    #[test]
    fn a_resumed_signal_over_a_suspended_reserve_still_holds_it() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            let error = || NewspeakError::error(crate::Symbol::SIGNAL, Value::nil(), "boom");
            let floor = mutator.heap.activation_stack_top();
            let raised = mutator
                .heap
                .push_activation(header(class), Arguments::NONE, 1)
                .expect("the stack holds the frame");
            let raised_frame = mutator.heap.reify(raised);
            mutator.set_depth_reserve(Some(raised_frame));
            let signalling = ActivationHeader {
                then: Continuation::VmErrorSignalled(Box::new(error())),
                ..header(class)
            };
            mutator
                .heap
                .push_activation(signalling, Arguments::NONE, 1)
                .expect("the stack holds the frame");
            assert!(mutator.depth_reserve_held(), "held before the suspension");
            let top = mutator
                .heap
                .suspend_activations(floor)
                .expect("two activations to suspend");
            let signal = mutator.resume_frame(top);
            assert!(mutator.heap.frame_is_suspended(raised_frame));
            assert!(
                mutator.heap.activation_signals_a_vm_error(signal),
                "the signal's continuation is read through the wrapper"
            );
            assert!(
                mutator.heap.activation_signalled_vm_error(signal).is_some(),
                "and its error"
            );
            assert!(
                mutator.depth_reserve_held(),
                "held while the signal runs over the suspended frame"
            );
            mutator.heap.activation_set_continuation(
                signal,
                Continuation::VmErrorSignalled(Box::new(error())),
            );
            assert!(
                mutator
                    .heap
                    .activation_suspended_sender(signal)
                    .is_some_and(|sender| sender.same(raised_frame)),
                "replacing the continuation keeps the return to the suspended sender"
            );
            assert!(mutator.heap.activation_signals_a_vm_error(signal));
            mutator.pop_for_unwind(signal);
            let resumed = mutator
                .heap
                .frame_activation(raised_frame)
                .expect("the pop resumed the sender");
            mutator.heap.pop_activation(resumed);
            assert_eq!(mutator.heap.activation_stack_top(), floor);
            assert_eq!(mutator.heap.suspension_count(), 0);
        });
    }

    #[test]
    fn an_activation_that_has_returned_is_refused_as_a_cannot_return() {
        // No golden reaches a restart whose activation has returned.
        let error = super::dead_activation_error(crate::Symbol::intern("restart"), Value::nil());
        assert_eq!(error.class, ErrorClass::CannotReturn);
    }
}
