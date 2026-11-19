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

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::PoisonError;

use ns_syntax::{Span, ast};

use super::metadata::again::{declaration_chain, module_is_shipped};
use super::metadata::{BodyMetadata, VariablePlace};
use super::stack::{self, OuterFrame, StackFrame};
use super::values::{Mark, used_labels};
use super::{
    CAPTURED, NO_SUCH_FRAME, NOT_ASSIGNABLE, NOT_AVAILABLE, NOT_MADE, RETURN_FROM_AN_EVALUATION,
    Stop,
};
use crate::bytecode::{CompiledCode, may_have_run};
use crate::heap::{ActivationReference, FrameReference, NewClosure};
use crate::interpreter::{
    Arguments, EVALUATION_DEPTH_RESERVE, FeedbackBase, MAX_ACTIVATION_DEPTH, Mutator, Step,
    StepArguments,
};
use crate::resolver::sends::EnclosingClass;
use crate::resolver::{Assignability, EvaluationScope, FrameVariable, resolve_evaluation};
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

/// Why a variable's new value that is not one expression is refused,
/// after `DOES_NOT_COMPILE`: statements or temporaries around it would
/// make what the assignment writes differ from what the evaluation
/// answers.
const ONE_EXPRESSION: &str = "a variable's new value is one expression";

/// Why an evaluation in a frame whose body has no metadata is refused:
/// without its variable table and its declaration nothing in the
/// expression can be resolved as the frame would resolve it -- a module
/// without text, a body its current text no longer compiles to, a
/// top-level frame.
pub(crate) const NO_SOURCE: &str = "the frame's source is not known";

/// Why an evaluation is refused when the stack has no room for its
/// activation.
pub(crate) const NO_ROOM: &str = "the stack has no room for the evaluation";

/// An expression compiled for an evaluation, and the marked objects it
/// names, in the order its block takes them as arguments
/// (`docs/design/debugger.md`, "Marking objects").
#[derive(Clone)]
pub(crate) struct Compiled {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) marked: Vec<Root>,
}

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
/// known, and an expression that does not compile -- an assignment a
/// block may already hold a copy of the variable before among its causes
/// (`assignabilities`). Every `<label>_DebugLabel` it names whose label
/// the stop's marks hold is a parameter of its block, the marked object
/// its argument.
pub(crate) fn compile(
    mutator: &mut Mutator<'_>,
    stop: &mut Stop,
    frame: usize,
    text: &str,
) -> Result<Compiled, String> {
    let marks = stop.values.marks().to_vec();
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    compile_with(mutator, stack_frame, text, &marks, |_| parsed(text))
}

/// The pause stress mode's evaluations, compiled once for each body and
/// variable name (`compile_for_stress`): every evaluation's block is a body
/// of its own, which takes a code id as it is pushed, so that compiling the
/// same name again at every stop would mint a row per evaluation and grow
/// the code id table to its limit, whose collections no program without
/// the mode makes. Each is held with its body, weakly, so that a body
/// freed and another allocated at its address never finds the first's.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressCompilations {
    compiled: HashMap<(usize, String), (std::rc::Weak<CompiledCode>, Compiled)>,
}

/// `compile` for the pause stress mode's reader, whose expression is a
/// variable's name: the compilation for the frame's body and the name
/// made at an earlier stop, if one was, or compiled now and kept in
/// `compilations`. A stop with marks compiles afresh, since a label may
/// name a parameter of the block. A reading of a variable resolves to the
/// same binding wherever the body stands, so the frame's program counter,
/// which decides only what an assignment may write, is no part of the
/// key.
#[cfg(debug_assertions)]
pub(crate) fn compile_for_stress(
    mutator: &mut Mutator<'_>,
    stop: &mut Stop,
    frame: usize,
    text: &str,
    compilations: &mut StressCompilations,
) -> Result<Compiled, String> {
    if !stop.values.marks().is_empty() {
        return compile(mutator, stop, frame, text);
    }
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    let body = stack::code(&mutator.heap, stack_frame);
    let key = (Rc::as_ptr(&body) as usize, text.to_owned());
    if let Some((held, compiled)) = compilations.compiled.get(&key)
        && held.upgrade().is_some()
    {
        return Ok(compiled.clone());
    }
    let compiled = compile(mutator, stop, frame, text)?;
    // The entries of bodies freed since go, so that the table holds no
    // more than the bodies alive and their names.
    compilations
        .compiled
        .retain(|_, (held, _)| held.strong_count() > 0);
    compilations
        .compiled
        .insert(key, (Rc::downgrade(&body), compiled.clone()));
    Ok(compiled)
}

/// `text` compiled as `compile` compiles it, standing in `frame`: a
/// breakpoint's condition or log expression, in the frame that reached
/// the breakpoint (`debugger::conditions`), naming the marks as they
/// stand.
pub(crate) fn compile_in(
    mutator: &mut Mutator<'_>,
    frame: &StackFrame,
    text: &str,
) -> Result<Compiled, String> {
    let marks = mutator.breakpoints.marks().to_vec();
    compile_with(mutator, frame, text, &marks, |_| parsed(text))
}

/// The assignment `name:: expression` compiled as `compile` compiles an
/// expression in the frame at index `frame` of `stop`'s stack: what
/// `Command::SetVariable` evaluates. `name` must be the variable the frame
/// shows under it -- the innermost of the name, which the assignment then
/// names too -- and assignable: anything else -- `self`, a parameter, an
/// immutable local, a name the frame does not hold -- is refused as not
/// assignable. `expression` is parsed on its own and must be one
/// expression, without temporaries, which the assignment is then built
/// around as a node of its own (`assignment`): text spliced around it
/// could be closed early by the expression, assigning one value while the
/// evaluation answers another.
pub(crate) fn compile_assignment(
    mutator: &mut Mutator<'_>,
    stop: &mut Stop,
    frame: usize,
    name: &str,
    expression: &str,
) -> Result<Compiled, String> {
    let marks = stop.values.marks().to_vec();
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    compile_with(mutator, stack_frame, expression, &marks, |metadata| {
        let shown = metadata
            .variables
            .iter()
            .find(|entry| entry.name.as_str() == name);
        match shown {
            Some(entry) if entry.assignable => assignment(name, expression),
            _ => Err(format!("{name} {NOT_ASSIGNABLE}")),
        }
    })
}

/// `text` parsed as a code body standing alone, or the refusal of an
/// expression that does not compile.
fn parsed(text: &str) -> Result<ast::CodeBody, String> {
    crate::compiler::evaluation::parse_expression(text)
        .map_err(|message| format!("{DOES_NOT_COMPILE}{message}"))
}

/// The code body `name:: expression`: `expression` parsed on its own, one
/// expression without temporaries -- a `^` refused as in any evaluation --
/// the value of an assignment to `name`, whose target's span lies past the
/// expression's text, so that no binding of the expression's is keyed by
/// it.
fn assignment(name: &str, expression: &str) -> Result<ast::CodeBody, String> {
    let mut body = parsed(expression)?;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return Err(format!("{DOES_NOT_COMPILE}{ONE_EXPRESSION}"));
    }
    let Some(ast::Statement {
        kind: ast::StatementKind::Expression(value),
        ..
    }) = body.statements.pop()
    else {
        return Err(String::from(RETURN_FROM_AN_EVALUATION));
    };
    let end = u32::try_from(expression.len())
        .map_err(|_| format!("{DOES_NOT_COMPILE}{ONE_EXPRESSION}"))?;
    let target_start = end.saturating_add(1);
    let target = ast::Identifier {
        name: name.to_owned(),
        span: Span::new(
            target_start,
            target_start.saturating_add(u32::try_from(name.len()).unwrap_or(u32::MAX)),
        ),
    };
    let span = Span::new(value.span.start, target.span.end);
    let assign = ast::Expression {
        kind: ast::ExpressionKind::Assign {
            target,
            value: Box::new(value),
            span,
        },
        parentheses: Vec::new(),
        type_arguments: None,
        synthesized: true,
        comments: Vec::new(),
        span,
    };
    body.statements.push(ast::Statement {
        kind: ast::StatementKind::Expression(assign),
        comments: Vec::new(),
        span,
    });
    body.span = Span::new(body.span.start, span.end);
    Ok(body)
}

/// `compile` of the code body `body` answers for the metadata of
/// `stack_frame`, or its refusal: `text`, the expression's text, names
/// the labels of `marks` the block takes as its parameters.
fn compile_with(
    mutator: &mut Mutator<'_>,
    stack_frame: &StackFrame,
    text: &str,
    marks: &[Mark],
    body: impl FnOnce(&BodyMetadata) -> Result<ast::CodeBody, String>,
) -> Result<Compiled, String> {
    // Decided before the metadata is asked for, which compiles the body
    // again: a shipped frame is refused whatever its body.
    let origin = stack::origin(&mutator.heap, stack_frame).ok_or(NO_SOURCE)?;
    if module_is_shipped(mutator, &origin.mixin).ok_or(NO_SOURCE)? {
        return Err(String::from(SHIPPED_FRAME));
    }
    let (code, _, metadata) = stack::body(mutator, stack_frame);
    let metadata = metadata.ok_or(NO_SOURCE)?;
    let expression = body(&metadata)?;
    let chain: Vec<(Symbol, HashSet<Symbol>)> =
        declaration_chain(mutator, &origin.mixin).ok_or(NO_SOURCE)?;
    let classes: Vec<EnclosingClass<'_>> = chain
        .iter()
        .map(|(name, members)| EnclosingClass {
            name: *name,
            members,
        })
        .collect();
    let assignabilities = assignabilities(mutator, stack_frame, &code, &metadata);
    let available = stack::available(
        &mutator.heap,
        stack_frame,
        metadata.variables.iter().map(|entry| entry.place),
    );
    let variables: Vec<FrameVariable> = metadata
        .variables
        .iter()
        .zip(assignabilities)
        .zip(available)
        .map(|((entry, assignability), available)| FrameVariable {
            name: entry.name,
            place: entry.place,
            assignability,
            available,
        })
        .collect();
    let declared: Vec<&str> = expression
        .temporaries
        .iter()
        .map(|temporary| temporary.name.name.as_str())
        .collect();
    let (labels, marked): (Vec<Symbol>, Vec<Root>) =
        used_labels(text, marks, &declared).into_iter().unzip();
    let scope = EvaluationScope {
        classes: &classes,
        variables: &variables,
        labels: &labels,
    };
    let bindings = resolve_evaluation(&expression, &scope).map_err(|message| {
        // The refusals the debugger names itself stand as they are; every
        // other is the compiler's.
        if message == RETURN_FROM_AN_EVALUATION
            || message.ends_with(NOT_ASSIGNABLE)
            || message.ends_with(CAPTURED)
            || message.ends_with(NOT_MADE)
            || message.ends_with(NOT_AVAILABLE)
        {
            message
        } else {
            format!("{DOES_NOT_COMPILE}{message}")
        }
    })?;
    let code = crate::compiler::evaluation::compile_evaluation(
        &expression,
        &bindings,
        &variables,
        &labels,
        mutator.lowering,
    );
    Ok(Compiled { code, marked })
}

/// Whether an assignment evaluated in `frame`, which runs `code` with
/// `metadata`, may write each of the variables of its table, in the
/// table's order (`docs/design/debugger.md`, "Assigning a captured
/// variable"): a parameter and an immutable local never; a mutable local
/// unless a block may already hold a copy of it, which the assignment
/// would leave stale. Where that lies is decided by the variable's place:
///
/// - a vector variable's copies share its slot, and it is assigned -- but
///   not before the frame that holds its vector made it, where it does
///   not exist yet (`stack::vector_not_made`);
/// - a register of the frame's own body is copied by the `MakeClosure`s
///   its entry names, and refused once one of them may have run before the
///   frame's program counter (`bytecode::may_have_run`);
/// - a register of the frame's that holds an enclosing variable is a copy
///   the frame was handed, and refused, as is the register of an enclosing
///   frame between that holds one (`Outer` short of the declaring frame);
/// - the declaring frame's own register (`Outer` as far as the variable's
///   depth) is decided by that frame's own entry for it, at its program
///   counter in its body, and refused whenever a block copies it once that
///   frame returned -- or when the frame or its entry cannot be found.
///
/// Under the reference lowering no body copies a variable, and nothing is
/// refused for a copy.
fn assignabilities(
    mutator: &mut Mutator<'_>,
    frame: &StackFrame,
    code: &CompiledCode,
    metadata: &BodyMetadata,
) -> Vec<Assignability> {
    let program_counter = stack::program_counter(&mutator.heap, frame);
    // One frame per depth, asked once for every variable it declares.
    let mut outer_frames: HashMap<u16, Option<OuterFrame>> = HashMap::new();
    metadata
        .variables
        .iter()
        .map(|entry| {
            if !entry.assignable {
                return Assignability::NotAssignable;
            }
            if stack::vector_not_made(&mutator.heap, frame, entry.place) {
                return Assignability::NotMade;
            }
            let captured = match entry.place {
                VariablePlace::Vector { .. } => false,
                VariablePlace::Register(_) if entry.depth == 0 => {
                    !entry.copied_by.is_empty()
                        && may_have_run(code, &entry.copied_by, program_counter)
                }
                VariablePlace::Register(_) => true,
                VariablePlace::Outer { hops, register } if hops == entry.depth => outer_frames
                    .entry(hops)
                    .or_insert_with(|| stack::outer_frame(mutator, frame, hops))
                    .as_ref()
                    .is_none_or(|outer| outer.may_have_copied(register)),
                VariablePlace::Outer { .. } => true,
            };
            if captured {
                Assignability::Captured
            } else {
                Assignability::Assignable
            }
        })
        .collect()
}

/// Begins evaluating `code`, compiled by `compile` for the frame at index
/// `frame` of `stop`'s stack, with `arguments`, its parameters: the frame
/// loses the registers the leftover analysis finds tainted at the program
/// counter it stores, which a block reading it by reference would read
/// otherwise, is reified, as a `MakeClosure` in it would reify it, and goes
/// on interpreted from here (`docs/design/debugger.md`, "Writing a
/// frame"); it and every frame of its captured chain a block the
/// expression creates reaches keep their registers past their return,
/// since such a block may be stored and read them
/// (`retain_reached_frames`); and the expression's closure, made over it,
/// is pushed above every live frame's window, whichever frame it
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
    begin_in(
        mutator,
        stack_frame,
        code,
        arguments,
        Continuation::Evaluated,
    )
}

/// `begin` in `stack_frame`, the frame `code` was compiled for, its base
/// going on with `then` once it returns: `Continuation::Evaluated`, or,
/// for a method breakpoint's condition at an exit by an unwind,
/// `Continuation::ExitEvaluated` with the unwind it interrupted.
pub(crate) fn begin_in<'gc>(
    mutator: &mut Mutator<'gc>,
    stack_frame: &StackFrame,
    code: Rc<CompiledCode>,
    arguments: &[Value<'gc>],
    then: Continuation<'gc>,
) -> Result<(), &'static str> {
    if let StackFrame::Running(activation) = stack_frame {
        mutator.heap.nil_tainted_registers(*activation);
        mutator.heap.clear_compiled(*activation);
    }
    let captured_frame = stack::reified(&mut mutator.heap, stack_frame);
    retain_reached_frames(mutator, captured_frame, farthest_reach(&code));
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
            then,
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

/// The farthest a block `code`, an evaluation's body, creates reaches
/// by reference, counted in hops from the created block -- its
/// `outer_reach`, which counts the blocks nested in it: 1 is the
/// evaluation's own frame, 2 the frame it stands in, 3 that frame's
/// captured frame, and so on; 0 when it creates none.
fn farthest_reach(code: &CompiledCode) -> u16 {
    code.body()
        .created_blocks()
        .map(|block| block.code.outer_reach)
        .max()
        .unwrap_or(0)
}

/// Keeps the registers of every frame on the captured chain from
/// `stood_in`, the frame the evaluation stands in, that a block it creates
/// reaches by reference (`reach`, `farthest_reach`), past the frame's
/// return: such a block may be stored and read them after it. A frame
/// the program made a block in kept its own when that block reaches it
/// (`MakeClosure`'s rule for a creator), but the evaluation's block
/// reaches past the stopped frame's own reach -- a variable of its home
/// the stopped block never read -- so every frame up to its reach is kept
/// here. An expression that only reads or writes the frames' variables
/// reaches them while the frames are stopped, so retaining them then
/// would only keep alive what the frames drop at their return, and show
/// it to every later stop (`docs/design/debugger.md`, "Evaluation").
fn retain_reached_frames<'gc>(
    mutator: &mut Mutator<'gc>,
    stood_in: FrameReference<'gc>,
    reach: u16,
) {
    let mut frame = Some(stood_in);
    for _ in 2..=reach {
        let Some(reached) = frame else {
            return;
        };
        mutator.heap.frame_retain_registers(reached);
        frame = mutator.heap.frame_captured(reached);
    }
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
    let outcome = ending(mutator, error);
    end(mutator, outcome)
}

/// The answer of the running evaluation `error` ended: the error,
/// described with the exception whose default action ended it, when one
/// did -- or that the debugger aborted it.
fn ending<'gc>(mutator: &mut Mutator<'gc>, error: &NewspeakError<'gc>) -> Outcome {
    let (exception, aborted) = mutator
        .breakpoints
        .evaluation_mut()
        .map_or((None, false), |evaluation| {
            (evaluation.exception.take(), evaluation.aborted)
        });
    if aborted {
        return Outcome::Aborted;
    }
    let description = match &exception {
        Some(exception) => {
            let described = mutator.heap.describe(mutator.heap.fetch(exception));
            format!("{described}: {}", error.message)
        }
        None => String::from(&*error.message),
    };
    Outcome::Error {
        description,
        exception,
    }
}

/// The evaluation of a condition at a method's exit by an unwind
/// returned (`Continuation::ExitEvaluated`): it is over, and asks for no
/// stop -- the unwind it interrupted goes on at once.
pub(crate) fn exit_returned(mutator: &mut Mutator<'_>) {
    set_running(mutator, None, 0);
}

/// An error ended the evaluation of a condition at a method's exit by an
/// unwind, its base popped (`interpreter::unwind`): answers what
/// `ended_by` would, and asks for no stop -- the unwind it interrupted
/// goes on at once.
pub(crate) fn exit_ended_by<'gc>(
    mutator: &mut Mutator<'gc>,
    error: &NewspeakError<'gc>,
) -> Outcome {
    let outcome = ending(mutator, error);
    set_running(mutator, None, 0);
    outcome
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
/// debugger detached meanwhile -- goes too. A failure at an exit by an
/// unwind no stop named stays only with the step the run carries to the
/// next, whose stop reports that exit (`stepping::run_ended`): with no
/// such step, the next run's first stop would name a failure of an exit
/// it does not report.
pub(crate) fn run_ended(mutator: &mut Mutator<'_>) {
    set_running(mutator, None, 0);
    mutator.breakpoints.evaluated = None;
    mutator.breakpoints.unwound_exit = None;
    if !mutator.breakpoints.carries_a_step() {
        mutator.breakpoints.exit_failure = None;
    }
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
    mutator.breakpoints.unwound_exit = None;
    mutator.breakpoints.exit_failure = None;
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
