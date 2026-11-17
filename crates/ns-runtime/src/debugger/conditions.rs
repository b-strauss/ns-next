//! Conditional breakpoints and logpoints, decided by the VM
//! (`docs/design/debugger.md`, "Hit counts and temporary breakpoints",
//! "Evaluation", "Hot conditions"). The `Breakpoint` arm, where no code
//! runs, leaves a breakpoint with a condition or a log to the safepoint
//! uncounted (`PendingStop::conditions`); the safepoint evaluates each
//! condition in the frame that reached it, an evaluation as
//! `Command::Evaluate` runs one, one at a time -- the run loop runs it, and
//! its end asks for the safepoint that goes on here (`Interpreter::decide`)
//! -- and only a breakpoint whose condition answers `true` counts the
//! reach and is decided by its hit condition. A logpoint that holds then
//! has its message's expressions evaluated the same way, each as its
//! `printString`, and logs instead of stopping. A condition or log
//! expression that does not compile or raises an error stops the program
//! there, saying why. The frame then goes past its slot, or stops, as the
//! arm's decision would have. A step into's stop at a pushed frame's
//! first statement decides the conditions standing there the same way
//! before it is made.
//!
//! A method's exit by an unwind is decided in the frame the unwind is
//! about to pop, as the unwinder runs an `ensure:` block there
//! (`decide_unwound_exits`): each condition's evaluation is pushed above
//! it with the unwind it interrupts (`Continuation::ExitEvaluated`), whose
//! return decides the exit and raises the unwind again
//! (`exit_answered`), and the exits that hold are reported where the
//! program goes on, as every exit by an unwind is; a failure there is
//! named by that report's stop (`failure_named`).

use std::rc::Rc;

use super::breakpoints::{Conditional, PendingStop, ReachKind};
use super::evaluation::{self, Outcome};
use super::remote::{BreakpointId, Event, LogPart, StopReason};
use super::stack::StackFrame;
use crate::Value;
use crate::bytecode::CompiledCode;
use crate::heap::ActivationReference;
use crate::interpreter::Mutator;
use crate::interpreter::unwind::UnwindOutcome;
use crate::types::{Continuation, PendingUnwind};

/// The reach of a slot whose conditions the safepoint decides, from the
/// first condition's evaluation until the last is decided; kept by the
/// `Debugger` while an evaluation runs.
pub(crate) struct Deciding {
    /// The stop the arm decided: its slot, the breakpoints counted there
    /// and those it leaves to the conditions, taken one by one from the
    /// end.
    stop: PendingStop,
    /// What the running evaluation decides.
    running: Option<Running>,
    /// The message of the logpoint whose condition held, while its parts
    /// are evaluated.
    logging: Option<Logging>,
    /// The breakpoints whose condition or log failed.
    failed: Vec<BreakpointId>,
    /// What the first failure says.
    error: Option<String>,
    /// Whether a pause aborted an evaluation: the stop is made, as the
    /// pause's.
    aborted: bool,
    /// The run's base, which the stop made once the conditions are
    /// decided stands on (`stack.rs`).
    pub(crate) floor: usize,
    /// For a step into's stop at a pushed frame's first statement, which
    /// is made whatever the conditions say, whether the step stopped in
    /// a factory by undoing its initializer's push
    /// (`stepping::HeldStep::StopInFactory`); `None` at a reach of a
    /// `Breakpoint` arm.
    pub(crate) pushed: Option<bool>,
}

/// What an evaluation of a reach's decision is for.
enum Running {
    /// The condition of a breakpoint standing at the slot.
    Condition(Conditional),
    /// The logged message's part being evaluated.
    LogPart,
}

/// A logpoint's message being made.
struct Logging {
    id: BreakpointId,
    parts: Vec<LogPart>,
    /// The part to make next.
    next: usize,
    /// The parts made so far, joined.
    text: String,
}

/// What the safepoint decided once every condition was evaluated.
pub(crate) struct Decision {
    /// The stop as the conditions leave it: every breakpoint whose
    /// condition held and whose hit condition holds among its hits, every
    /// other counted or decided among the passed.
    pub(crate) stop: PendingStop,
    /// The breakpoints whose condition or log expression failed.
    pub(crate) failed: Vec<BreakpointId>,
    /// What the first failure says.
    pub(crate) error: Option<String>,
    /// Whether a pause aborted an evaluation.
    pub(crate) aborted: bool,
    /// As `Deciding::pushed`.
    pub(crate) pushed: Option<bool>,
}

impl Deciding {
    /// The decision of the conditions `stop` leaves to the safepoint, none
    /// evaluated yet; `floor` the run's base.
    pub(crate) fn new(mut stop: PendingStop, floor: usize) -> Deciding {
        // Taken from the end, in the order the arm found them.
        stop.conditions.reverse();
        Deciding {
            stop,
            running: None,
            logging: None,
            failed: Vec::new(),
            error: None,
            aborted: false,
            floor,
            pushed: None,
        }
    }

    /// The decision of the conditions standing where a step into stopped,
    /// at a pushed frame's first statement: `stop` that frame's reach, a
    /// step's stop whatever they say, `rebuilt` whether the step stopped
    /// in a factory by undoing its initializer's push.
    pub(crate) fn at_push(stop: PendingStop, floor: usize, rebuilt: bool) -> Deciding {
        Deciding {
            pushed: Some(rebuilt),
            ..Deciding::new(stop, floor)
        }
    }

    /// Goes on deciding: evaluates the next condition or log expression,
    /// and answers `true` once its evaluation is begun, which the run loop
    /// runs; `false` once everything is decided (`decided`). A condition
    /// whose breakpoint the commands served meanwhile removed is not
    /// evaluated.
    pub(crate) fn advance(&mut self, mutator: &mut Mutator<'_>) -> bool {
        loop {
            if self.aborted {
                return false;
            }
            if let Some(logging) = &mut self.logging {
                match logging.parts.get(logging.next).cloned() {
                    Some(LogPart::Text(text)) => {
                        logging.text.push_str(&text);
                        logging.next += 1;
                    }
                    Some(LogPart::Expression(expression)) => {
                        let (id, number) = (logging.id, logging.next + 1);
                        let text = format!("({expression}) printString");
                        if self.begin(mutator, id, number, &text) {
                            self.running = Some(Running::LogPart);
                            return true;
                        }
                    }
                    None => {
                        if let Some(Logging { id, text, .. }) = self.logging.take() {
                            mutator.breakpoints.report(Event::Logged {
                                breakpoint: id,
                                text,
                            });
                        }
                    }
                }
                continue;
            }
            let Some(conditional) = self.stop.conditions.pop() else {
                return false;
            };
            let Some((condition, log)) = mutator.breakpoints.conditions_of(conditional.id) else {
                continue;
            };
            match condition {
                Some(text) => {
                    if self.begin(mutator, conditional.id, 0, &text) {
                        self.running = Some(Running::Condition(conditional));
                        return true;
                    }
                }
                None => self.held(mutator, conditional, log),
            }
        }
    }

    /// The running evaluation answered `outcome`: a condition holds when
    /// it answered `true`, and only then; a log expression's `printString`
    /// joins the message; an error is the breakpoint's failure; an abort
    /// ends the decision. The frame the evaluation stood above, now on
    /// top, is the reaching frame, which the suspend stress mode may have
    /// moved.
    pub(crate) fn evaluated(&mut self, mutator: &mut Mutator<'_>, outcome: Outcome) {
        if let Some(top) = mutator.heap.top_activation() {
            self.stop.slot.activation = top;
        }
        let running = self
            .running
            .take()
            .expect("internal: a decision's evaluation ended while it ran");
        let id = match (&running, &self.logging) {
            (Running::Condition(conditional), _) => conditional.id,
            (Running::LogPart, Some(logging)) => logging.id,
            (Running::LogPart, None) => unreachable!("internal: a log part evaluated unlogged"),
        };
        match (running, outcome) {
            (_, Outcome::Aborted) => self.aborted = true,
            (_, Outcome::Error { description, .. }) => self.fail(id, description),
            (Running::Condition(conditional), Outcome::Value(value)) => {
                let holds = mutator.heap.fetch(&value).as_boolean() == Some(true);
                match mutator.breakpoints.conditions_of(id) {
                    Some((_, log)) if holds => self.held(mutator, conditional, log),
                    Some(_) => self.stop.passed.push(id),
                    None => {}
                }
            }
            (Running::LogPart, Outcome::Value(value)) => {
                let text = logged_text(mutator, mutator.heap.fetch(&value));
                if let Some(logging) = &mut self.logging {
                    logging.text.push_str(&text);
                    logging.next += 1;
                }
            }
        }
    }

    /// Everything is decided (`advance` answered `false`).
    pub(crate) fn decided(self) -> Decision {
        Decision {
            stop: self.stop,
            failed: self.failed,
            error: self.error,
            aborted: self.aborted,
            pushed: self.pushed,
        }
    }

    /// Compiles the breakpoint `id`'s expression numbered `number`, `text`,
    /// in the reaching frame, or takes it compiled for this slot before,
    /// and begins its evaluation; answers whether it began, the
    /// breakpoint's failure recorded otherwise.
    fn begin(
        &mut self,
        mutator: &mut Mutator<'_>,
        id: BreakpointId,
        number: usize,
        text: &str,
    ) -> bool {
        let activation = self.stop.slot.activation;
        let frame = StackFrame::Running(activation);
        let code = compiled(mutator, activation, self.stop.slot.index, id, number, text);
        let begun = code.and_then(|code| {
            evaluation::begin_in(mutator, &frame, code, &[], Continuation::Evaluated)
                .map_err(String::from)
        });
        match begun {
            Ok(()) => true,
            Err(error) => {
                self.fail(id, error);
                false
            }
        }
    }

    /// The condition of `conditional`'s breakpoint held, or it has none:
    /// the reach is counted, and stops for it, or logs `log`, where its
    /// hit condition holds.
    fn held(
        &mut self,
        mutator: &mut Mutator<'_>,
        conditional: Conditional,
        log: Option<Vec<LogPart>>,
    ) {
        let Conditional { id, kind } = conditional;
        if !mutator.breakpoints.count_hit(id) {
            self.stop.passed.push(id);
            return;
        }
        if let Some(parts) = log {
            // A logpoint logs instead of stopping.
            self.stop.passed.push(id);
            self.logging = Some(Logging {
                id,
                parts,
                next: 0,
                text: String::new(),
            });
            return;
        }
        let hits = &mut self.stop.hits;
        match kind {
            ReachKind::Line => hits.lines.push(id),
            ReachKind::Entry => hits.entries.push(id),
            ReachKind::Exit => hits.exits.push(id),
        }
    }

    /// The breakpoint `id`'s condition or log expression failed, as
    /// `error` says: the stop is made for it, and its message, if it was
    /// being made, is not logged.
    fn fail(&mut self, id: BreakpointId, error: String) {
        if !self.failed.contains(&id) {
            self.failed.push(id);
        }
        self.error.get_or_insert(error);
        if self
            .logging
            .as_ref()
            .is_some_and(|logging| logging.id == id)
        {
            self.logging = None;
        }
    }
}

/// The breakpoint `id`'s expression numbered `number`, `text`, compiled
/// for slot `index` of the body `activation` runs, in that frame -- or
/// taken compiled for it before (`CompiledConditions`); the refusal
/// otherwise.
fn compiled(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    index: usize,
    id: BreakpointId,
    number: usize,
    text: &str,
) -> Result<Rc<CompiledCode>, String> {
    let body = mutator.heap.activation_code(activation);
    if let Some(code) = mutator
        .breakpoints
        .compiled_condition(id, &body, index, number)
    {
        return Ok(code);
    }
    let frame = StackFrame::Running(activation);
    let code = evaluation::compile_in(mutator, &frame, text)?;
    mutator
        .breakpoints
        .keep_compiled_condition(id, &body, index, number, Rc::clone(&code));
    Ok(code)
}

/// What a log expression's evaluation answered, as text: `printString`
/// answers a String, and anything else a program's override answered is
/// described as the stop describes a value.
fn logged_text<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> String {
    value
        .as_object()
        .and_then(|object| mutator.heap.string_code_points(object))
        .map_or_else(|| mutator.heap.describe(value), |text| text.to_utf8())
}

/// The decision of a method's exit by an unwind, made in the frame the
/// unwind is about to pop (`decide_unwound_exits`), from the first
/// condition's evaluation until the last is decided. On the `Mutator`
/// (`Breakpoints::unwound_exit`), since the unwinder and the run loop's
/// continuations that carry it run inside an entry; it holds no handle.
#[derive(Default)]
pub(crate) struct UnwoundExit {
    /// The method breakpoints with a condition or a log still to decide,
    /// taken from the end.
    remaining: Vec<BreakpointId>,
    /// The breakpoints counted whose hit condition holds at the exit, the
    /// ones it is reported for.
    held: Vec<BreakpointId>,
    /// What the running evaluation decides.
    running: Option<ExitRunning>,
    /// The message of the logpoint whose condition held, while its parts
    /// are evaluated.
    logging: Option<Logging>,
    /// The breakpoints whose condition or log failed.
    failed: Vec<BreakpointId>,
    /// What the first failure says.
    error: Option<String>,
}

/// What an evaluation of an exit's decision is for.
#[derive(Clone, Copy)]
enum ExitRunning {
    Condition(BreakpointId),
    LogPart(BreakpointId),
}

/// The method breakpoints `ids`, whose method `top`, the frame on top, is
/// left by the unwind `pending`, decided before the unwinder pops it --
/// or, an evaluation of theirs having answered meanwhile, the decision of
/// `top` going on: each without a condition or a log counts the exit and
/// holds where its hit condition does; each with one has its condition
/// evaluated in `top` first, and is counted only where it answers `true`,
/// and a logpoint that holds logs instead of being reported. Answers the
/// breakpoints the exit is reported for -- those that hold, those whose
/// condition or log failed, which the report's stop names
/// (`failure_named`), and one a pause aborted, whose stop the report's
/// is -- with the unwind to carry on; or, when an evaluation is begun,
/// the outcome the run loop goes on with: the evaluation's base, pushed
/// above `top`, whose return raises `pending` again.
pub(crate) fn decide_unwound_exits<'gc>(
    mutator: &mut Mutator<'gc>,
    top: ActivationReference,
    ids: Vec<BreakpointId>,
    pending: PendingUnwind<'gc>,
) -> Result<(Vec<BreakpointId>, PendingUnwind<'gc>), UnwindOutcome<'gc>> {
    let mut exit = match mutator.breakpoints.unwound_exit.take() {
        Some(exit) => exit,
        None => {
            let mut exit = UnwoundExit::default();
            for id in ids {
                if mutator.breakpoints.is_conditional(id) {
                    exit.remaining.push(id);
                } else if mutator.breakpoints.count_hit(id) {
                    exit.held.push(id);
                }
            }
            exit.remaining.reverse();
            exit
        }
    };
    loop {
        if let Some(logging) = &mut exit.logging {
            match logging.parts.get(logging.next).cloned() {
                Some(LogPart::Text(text)) => {
                    logging.text.push_str(&text);
                    logging.next += 1;
                }
                Some(LogPart::Expression(expression)) => {
                    let (id, number) = (logging.id, logging.next + 1);
                    let text = format!("({expression}) printString");
                    if exit.begin(mutator, top, id, number, &text, &pending) {
                        exit.running = Some(ExitRunning::LogPart(id));
                        return Err(begun(mutator, exit));
                    }
                }
                None => {
                    if let Some(Logging { id, text, .. }) = exit.logging.take() {
                        mutator.breakpoints.report(Event::Logged {
                            breakpoint: id,
                            text,
                        });
                    }
                }
            }
            continue;
        }
        let Some(id) = exit.remaining.pop() else {
            break;
        };
        let Some((condition, log)) = mutator.breakpoints.conditions_of(id) else {
            continue;
        };
        match condition {
            Some(text) => {
                if exit.begin(mutator, top, id, 0, &text, &pending) {
                    exit.running = Some(ExitRunning::Condition(id));
                    return Err(begun(mutator, exit));
                }
            }
            None => exit.held(mutator, id, log),
        }
    }
    let UnwoundExit {
        mut held,
        failed,
        error,
        ..
    } = exit;
    if let Some(error) = error {
        let (named, _) = mutator
            .breakpoints
            .exit_failure
            .get_or_insert_with(|| (Vec::new(), error));
        for id in &failed {
            if !named.contains(id) {
                named.push(*id);
            }
        }
    }
    for id in failed {
        if !held.contains(&id) {
            held.push(id);
        }
    }
    Ok((held, pending))
}

/// Keeps `exit` for the unwind's return once its evaluation is begun, and
/// answers the outcome the run loop goes on with: the evaluation's base,
/// now on top, run from its first instruction.
fn begun<'gc>(mutator: &mut Mutator<'gc>, exit: UnwoundExit) -> UnwindOutcome<'gc> {
    mutator.breakpoints.unwound_exit = Some(exit);
    let base = mutator
        .heap
        .top_activation()
        .expect("internal: an evaluation's base was just pushed");
    UnwindOutcome::Signalling(base)
}

impl UnwoundExit {
    /// Compiles the breakpoint `id`'s expression numbered `number`,
    /// `text`, in `top`, the frame the unwind is about to pop, and pushes
    /// its evaluation above it, its base raising `pending` again when it
    /// returns; answers whether it began, the breakpoint's failure
    /// recorded otherwise.
    fn begin<'gc>(
        &mut self,
        mutator: &mut Mutator<'gc>,
        top: ActivationReference,
        id: BreakpointId,
        number: usize,
        text: &str,
        pending: &PendingUnwind<'gc>,
    ) -> bool {
        let index = mutator.heap.activation_program_counter(top);
        let frame = StackFrame::Running(top);
        let begun = compiled(mutator, top, index, id, number, text).and_then(|code| {
            let then = Continuation::ExitEvaluated(Box::new(PendingUnwind::clone(pending)));
            evaluation::begin_in(mutator, &frame, code, &[], then).map_err(String::from)
        });
        match begun {
            Ok(()) => true,
            Err(error) => {
                self.fail(id, error);
                false
            }
        }
    }

    /// The condition of the breakpoint `id` held at the exit, or it has
    /// none: the exit is counted, and reported, or logs `log`, where its
    /// hit condition holds.
    fn held(&mut self, mutator: &mut Mutator<'_>, id: BreakpointId, log: Option<Vec<LogPart>>) {
        if !mutator.breakpoints.count_hit(id) {
            return;
        }
        match log {
            Some(parts) => {
                self.logging = Some(Logging {
                    id,
                    parts,
                    next: 0,
                    text: String::new(),
                });
            }
            None => self.held.push(id),
        }
    }

    /// As `Deciding::fail`.
    fn fail(&mut self, id: BreakpointId, error: String) {
        if !self.failed.contains(&id) {
            self.failed.push(id);
        }
        self.error.get_or_insert(error);
        if self
            .logging
            .as_ref()
            .is_some_and(|logging| logging.id == id)
        {
            self.logging = None;
        }
    }

    /// The running evaluation's breakpoint, taken.
    fn take_running(&mut self) -> BreakpointId {
        match self
            .running
            .take()
            .expect("internal: an exit's evaluation ended while it ran")
        {
            ExitRunning::Condition(id) | ExitRunning::LogPart(id) => id,
        }
    }
}

/// The evaluation of a condition or log expression at a method's exit by
/// an unwind returned `value` (`Continuation::ExitEvaluated`): the
/// condition holds when it answered `true`, and only then; a log
/// expression's `printString` joins the message. The unwind it
/// interrupted is raised again by the continuation, and goes on deciding
/// the exit (`decide_unwound_exits`).
pub(crate) fn exit_answered<'gc>(mutator: &mut Mutator<'gc>, value: Value<'gc>) {
    evaluation::exit_returned(mutator);
    let Some(mut exit) = mutator.breakpoints.unwound_exit.take() else {
        return;
    };
    let running = exit.running;
    let id = exit.take_running();
    match running {
        Some(ExitRunning::Condition(_)) => {
            let holds = value.as_boolean() == Some(true);
            if let Some((_, log)) = mutator.breakpoints.conditions_of(id)
                && holds
            {
                exit.held(mutator, id, log);
            }
        }
        _ => {
            let text = logged_text(mutator, value);
            if let Some(logging) = &mut exit.logging {
                logging.text.push_str(&text);
                logging.next += 1;
            }
        }
    }
    mutator.breakpoints.unwound_exit = Some(exit);
}

/// An error ended the evaluation of a condition or log expression at a
/// method's exit by an unwind, as `outcome` says, its base popped: the
/// breakpoint failed; or a pause aborted it, and the exit is reported for
/// it, where the program goes on, without deciding what is left -- the
/// pause's stop is that report's.
pub(crate) fn exit_ended(mutator: &mut Mutator<'_>, outcome: Outcome) {
    let Some(mut exit) = mutator.breakpoints.unwound_exit.take() else {
        return;
    };
    let id = exit.take_running();
    match outcome {
        Outcome::Error { description, .. } => exit.fail(id, description),
        Outcome::Aborted | Outcome::Value(_) => {
            exit.remaining.clear();
            exit.logging = None;
            if !exit.held.contains(&id) {
                exit.held.push(id);
            }
        }
    }
    mutator.breakpoints.unwound_exit = Some(exit);
}

/// Whether an evaluation at a method's exit by an unwind runs: the
/// safepoint serves it as it serves an evaluation a stop is kept for.
pub(crate) fn exit_evaluating(mutator: &Mutator<'_>) -> bool {
    mutator.breakpoints.evaluating() && mutator.breakpoints.unwound_exit.is_some()
}

/// `reason`, the reason of a stop about to be made, naming the conditions
/// or log expressions that failed at a method's exit by an unwind since
/// the last stop, whose exit this stop reports: an exception's stop names
/// them among its exits; any other is their failure
/// (`StopReason::ConditionFailed`), every breakpoint `reason` named among
/// its ids after them.
pub(crate) fn failure_named(mutator: &mut Mutator<'_>, reason: StopReason) -> StopReason {
    let Some((failed, error)) = mutator.breakpoints.exit_failure.take() else {
        return reason;
    };
    let add = |ids: &mut Vec<BreakpointId>, more: Vec<BreakpointId>| {
        for id in more {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    };
    match reason {
        StopReason::Exception {
            class_name,
            description,
            caught,
            mut exits,
        } => {
            add(&mut exits, failed);
            StopReason::Exception {
                class_name,
                description,
                caught,
                exits,
            }
        }
        StopReason::Halt { mut exits } => {
            add(&mut exits, failed);
            StopReason::Halt { exits }
        }
        other => {
            let mut ids = failed;
            add(&mut ids, named_by(other));
            StopReason::ConditionFailed { ids, error }
        }
    }
}

/// Every breakpoint `reason` names.
pub(crate) fn named_by(reason: StopReason) -> Vec<BreakpointId> {
    match reason {
        StopReason::Pause | StopReason::Step => Vec::new(),
        StopReason::Breakpoint { ids } | StopReason::ConditionFailed { ids, .. } => ids,
        StopReason::MethodEntry { id, others } | StopReason::MethodExit { id, others } => {
            std::iter::once(id).chain(others).collect()
        }
        StopReason::Exception { exits, .. } | StopReason::Halt { exits } => exits,
    }
}
