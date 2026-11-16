//! Evaluating an expression at a stop (`docs/design/debugger.md`, decision
//! 8, "Evaluation"). The expression is compiled as a block standing in a
//! stopped frame (`compiler::evaluation`), its closure made over the frame
//! and pushed above every live frame with `Continuation::Evaluated`, and
//! the stop is left, kept, for the ordinary run loop to run it: no Rust
//! frame waits for the evaluation, and the loop is never re-entered. Its
//! end -- its block's return (`returned`), or an exception no handler
//! within it took (`ended_by`) -- records its answer here and asks for a
//! stop, and the safepoint that serves it re-enters the kept stop
//! before the frame below runs another instruction. While it runs its
//! base ends the handler search and every unwind from within it
//! (`interpreter::unwind`), nothing stops, and the depth limit is raised
//! for its own reserve (`EVALUATION_DEPTH_RESERVE`). A pause, or the
//! remote's departure, aborts it (`abort`): the debugger's termination
//! signal, unwound from the top, which its base ends.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::PoisonError;

use super::metadata::again::{declaration_chain, module_name};
use super::stack::{self, StackFrame};
use super::{NO_SUCH_FRAME, NOT_ASSIGNABLE, RETURN_FROM_AN_EVALUATION, Stop};
use crate::bytecode::CompiledCode;
use crate::heap::{ActivationReference, NewClosure};
use crate::interpreter::{
    Arguments, EVALUATION_DEPTH_RESERVE, FeedbackBase, MAX_ACTIVATION_DEPTH, Mutator, Step,
    StepArguments,
};
use crate::resolver::sends::EnclosingClass;
use crate::resolver::{EvaluationScope, FrameVariable, resolve_evaluation};
use crate::types::{Continuation, NewspeakError};
use crate::{Root, Symbol, Value};

/// Why an evaluation in a frame of the core library is refused: a shipped
/// frame's variables and slots reach the VM mirror, which the expression
/// could hand to the program (`docs/design/debugger.md`, "Evaluation never
/// reaches the VM mirror").
pub(crate) const SHIPPED_FRAME: &str = "the frame is the core library's";

/// What precedes the compiler's message in the refusal of an expression
/// that does not compile.
pub(crate) const DOES_NOT_COMPILE: &str = "the expression does not compile: ";

/// Why an evaluation in a frame whose body has no metadata is refused:
/// without its variable table and its declaration nothing in the
/// expression can be resolved as the frame would resolve it -- a module
/// without text, a body its current text no longer compiles to, a
/// top-level frame.
pub(crate) const NO_SOURCE: &str = "the frame's source is not known";

/// Why an evaluation is refused when the stack has no room for its
/// activation.
pub(crate) const NO_ROOM: &str = "the stack has no room for the evaluation";

/// An expression the debugger evaluates, from its push until its base is
/// popped (`Breakpoints::evaluating`).
pub(crate) struct RunningEvaluation {
    /// The exception whose default action ended the evaluation, held where
    /// the `terminate:` leaf runs within it (`exception_unhandled`), for
    /// the unwind that ends at the base to answer.
    exception: Option<Root>,
    /// Whether the debugger aborted it (`abort`): the unwind that ends at
    /// its base, the termination's signal, answers that it was aborted,
    /// whatever an `ensure:` block it ran raised on the way.
    aborted: bool,
}

/// What an evaluation answered, held until the stop takes it.
pub(crate) enum Outcome {
    Value(Root),
    Error {
        description: String,
        exception: Option<Root>,
    },
    Aborted,
}

/// `text` compiled as a block standing in the frame at index `frame` of
/// `stop`'s stack (`docs/design/debugger.md`, decision 8): resolved
/// against the frame's variable table and the class chain of the
/// declaration its body belongs to, so that it obeys access control as the
/// frame's method does. Refused, with the reason, for a frame index past
/// the stack, a frame of the core library, a frame whose source is not
/// known, and an expression that does not compile.
pub(crate) fn compile(
    mutator: &mut Mutator<'_>,
    stop: &mut Stop,
    frame: usize,
    text: &str,
) -> Result<Rc<CompiledCode>, String> {
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    // Decided before the metadata is asked for, which compiles the body
    // again: a shipped frame is refused whatever its body.
    let origin = stack::origin(&mutator.heap, stack_frame).ok_or(NO_SOURCE)?;
    let module = module_name(mutator, &origin.mixin).ok_or(NO_SOURCE)?;
    if module.shipped {
        return Err(String::from(SHIPPED_FRAME));
    }
    let (_, _, metadata) = stack::body(mutator, stack_frame);
    let metadata = metadata.ok_or(NO_SOURCE)?;
    let chain: Vec<(Symbol, HashSet<Symbol>)> =
        declaration_chain(mutator, &origin.mixin).ok_or(NO_SOURCE)?;
    let classes: Vec<EnclosingClass<'_>> = chain
        .iter()
        .map(|(name, members)| EnclosingClass {
            name: *name,
            members,
        })
        .collect();
    let variables: Vec<FrameVariable> = metadata
        .variables
        .iter()
        .map(|entry| FrameVariable {
            name: entry.name,
            place: entry.place,
            assignable: entry.assignable,
        })
        .collect();
    let expression = crate::compiler::evaluation::parse_expression(text)
        .map_err(|message| format!("{DOES_NOT_COMPILE}{message}"))?;
    let scope = EvaluationScope {
        classes: &classes,
        variables: &variables,
        labels: &[],
    };
    let bindings = resolve_evaluation(&expression, &scope).map_err(|message| {
        // The two refusals the debugger names itself stand as they are;
        // every other is the compiler's.
        if message == RETURN_FROM_AN_EVALUATION || message.ends_with(NOT_ASSIGNABLE) {
            message
        } else {
            format!("{DOES_NOT_COMPILE}{message}")
        }
    })?;
    Ok(crate::compiler::evaluation::compile_evaluation(
        &expression,
        &bindings,
        &variables,
        &[],
        mutator.lowering,
    ))
}

/// Begins evaluating `code`, compiled by `compile` for the frame at index
/// `frame` of `stop`'s stack, with `arguments`, its parameters: the frame
/// loses the registers the leftover analysis finds tainted at the program
/// counter it stores, which a block reading it by reference would read
/// otherwise, is reified, as a `MakeClosure` in it would reify it, and goes
/// on interpreted from here (`docs/design/debugger.md`, "Writing a
/// frame"); it keeps its registers past its return, since a block the
/// expression stores may read them; and the expression's closure, made
/// over it, is pushed above every live frame's window, whichever frame it
/// evaluates in -- a frame below the top has its callees above it -- with
/// `Continuation::Evaluated`. Breakpoints, exception breakpoints and the
/// unwinder see the evaluation from here until its base is popped, and
/// the depth limit is `max(L, base + EVALUATION_DEPTH_RESERVE)`, the base
/// its base frame's counted depth -- raised before the push, which a stop
/// at the limit would otherwise refuse.
/// Refused for a frame index past the stack, and when the stack has no
/// room for the push.
pub(crate) fn begin<'gc>(
    mutator: &mut Mutator<'gc>,
    stop: &mut Stop,
    frame: usize,
    code: Rc<CompiledCode>,
    arguments: &[Value<'gc>],
) -> Result<(), &'static str> {
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    if let StackFrame::Running(activation) = stack_frame {
        mutator.heap.nil_tainted_registers(*activation);
        mutator.heap.clear_compiled(*activation);
    }
    let captured_frame = stack::reified(&mut mutator.heap, stack_frame);
    mutator.heap.frame_retain_registers(captured_frame);
    let closure = mutator.new_closure(NewClosure {
        code,
        captured_frame,
        copied: Vec::new(),
        feedback_base: FeedbackBase::NOT_ASSIGNED,
    });
    let sender = mutator.heap.top_activation();
    let base = mutator.heap.counted_depth() + 1;
    let raise = (base + EVALUATION_DEPTH_RESERVE).saturating_sub(MAX_ACTIVATION_DEPTH);
    set_running(
        mutator,
        Some(RunningEvaluation {
            exception: None,
            aborted: false,
        }),
        raise,
    );
    mutator.breakpoints.evaluated = None;
    let pushed = mutator.push_step(
        Step::Block {
            closure,
            arguments: StepArguments::Owned(arguments.to_vec()),
            then: Continuation::Evaluated,
        },
        Arguments::NONE,
        sender,
        false,
    );
    if pushed.is_err() {
        set_running(mutator, None, 0);
        return Err(NO_ROOM);
    }
    Ok(())
}

/// The evaluation's block returned `value`, its answer: recorded for the
/// stop, which the stop request this asks for re-enters. Answers the
/// frame the evaluation stood above, now the top, which goes on from the
/// program counter it stopped at (`vm::evaluated`).
pub(crate) fn returned<'gc>(mutator: &mut Mutator<'gc>, value: Value<'gc>) -> ActivationReference {
    let value = mutator.heap.stash(value);
    end(mutator, Outcome::Value(value))
}

/// An error ended the evaluation, popped down to below its base
/// (`interpreter::unwind`): the error is its answer, described with the
/// exception whose default action ended it, when one did -- or, when the
/// debugger aborted it, that it was aborted. Answers the frame the
/// evaluation stood above, as `returned` does.
pub(crate) fn ended_by<'gc>(
    mutator: &mut Mutator<'gc>,
    error: &NewspeakError<'gc>,
) -> ActivationReference {
    let (exception, aborted) = mutator
        .breakpoints
        .evaluation_mut()
        .map_or((None, false), |evaluation| {
            (evaluation.exception.take(), evaluation.aborted)
        });
    if aborted {
        return end(mutator, Outcome::Aborted);
    }
    let description = match &exception {
        Some(exception) => {
            let described = mutator.heap.describe(mutator.heap.fetch(exception));
            format!("{described}: {}", error.message)
        }
        None => String::from(&*error.message),
    };
    end(
        mutator,
        Outcome::Error {
            description,
            exception,
        },
    )
}

/// The `terminate:` leaf runs for `exception`, whose default action ends
/// the run -- or, within an evaluation, the evaluation: the exception is
/// held for the unwind that ends at the evaluation's base, which answers
/// it. Nothing is held outside an evaluation.
pub(crate) fn exception_unhandled<'gc>(mutator: &mut Mutator<'gc>, exception: Value<'gc>) {
    if !mutator.breakpoints.evaluating() {
        return;
    }
    let exception = mutator.heap.stash(exception);
    if let Some(evaluation) = mutator.breakpoints.evaluation_mut() {
        evaluation.exception = Some(exception);
    }
}

/// The debugger aborts the running evaluation -- a pause asked while it
/// runs, or the remote gone: the termination's signal the safepoint
/// unwinds from the top runs every `ensure:` block above the base and
/// ends there (`interpreter::unwind`'s `at_evaluation_base`), and the
/// evaluation answers that it was aborted (`ended_by`). Anything it held
/// for its answer goes.
pub(crate) fn abort(mutator: &mut Mutator<'_>) {
    if let Some(evaluation) = mutator.breakpoints.evaluation_mut() {
        evaluation.aborted = true;
        evaluation.exception = None;
    }
}

/// The evaluation's answer, taken by the stop that re-enters.
pub(crate) fn take_outcome(mutator: &mut Mutator<'_>) -> Option<Outcome> {
    mutator.breakpoints.evaluated.take()
}

/// Whether an evaluation ended and its answer waits for the stop.
pub(crate) fn has_ended(mutator: &Mutator<'_>) -> bool {
    mutator.breakpoints.evaluated.is_some()
}

/// The run ended: an evaluation still running ended with it unanswered,
/// the depth limit's raise goes, and an answer no stop took -- its
/// debugger detached meanwhile -- goes too.
pub(crate) fn run_ended(mutator: &mut Mutator<'_>) {
    set_running(mutator, None, 0);
    mutator.breakpoints.evaluated = None;
}

/// The debugger terminates the run: an evaluation still running ends
/// with it unanswered, its base popped by the termination's unwind, which
/// goes past it to the run's floor. The depth limit keeps its raise until
/// the run ended (`run_ended`): the `ensure:` blocks that unwind runs
/// above the base push where the evaluation's frames stood, which may lie
/// past L, and would be refused with L's error otherwise.
pub(crate) fn terminated(mutator: &mut Mutator<'_>) {
    mutator.breakpoints.set_evaluation(None);
    mutator.breakpoints.evaluated = None;
}

/// Begins (`Some`) or ends (`None`) the running evaluation, the depth
/// limit raised by `raise` while it runs, and given back once it ended.
fn set_running(mutator: &mut Mutator<'_>, evaluation: Option<RunningEvaluation>, raise: usize) {
    debug_assert!(
        evaluation.is_some() || raise == 0,
        "only a running evaluation raises the limit"
    );
    mutator.breakpoints.set_evaluation(evaluation);
    mutator.heap.raise_depth_limit_for_evaluation(raise);
}

/// The evaluation is over, `outcome` its answer: nothing it does is
/// skipped any more, and the stop it was asked at is asked for under the
/// attached debugger's arming lock, as an exception breakpoint asks for
/// its stop. Answers the frame it stood above, now the top.
fn end(mutator: &mut Mutator<'_>, outcome: Outcome) -> ActivationReference {
    set_running(mutator, None, 0);
    mutator.breakpoints.evaluated = Some(outcome);
    let arming = mutator.breakpoints.arming();
    {
        let _armed = arming
            .as_ref()
            .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
        mutator.heap.request_debugger_stop();
    }
    mutator
        .heap
        .top_activation()
        .expect("internal: an evaluation stands above the frame it was asked in")
}
