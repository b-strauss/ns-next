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
//! arm's decision would have.

use std::rc::Rc;

use super::breakpoints::{Conditional, PendingStop, ReachKind};
use super::evaluation::{self, Outcome};
use super::remote::{BreakpointId, Event, LogPart};
use super::stack::StackFrame;
use crate::interpreter::Mutator;

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
                let value = mutator.heap.fetch(&value);
                // `printString` answers a String; anything else a program's
                // override answered is described as the stop describes it.
                let text = value
                    .as_object()
                    .and_then(|object| mutator.heap.string_code_points(object))
                    .map_or_else(|| mutator.heap.describe(value), |text| text.to_utf8());
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
        let index = self.stop.slot.index;
        let body = mutator.heap.activation_code(activation);
        let frame = StackFrame::Running(activation);
        let code = match mutator
            .breakpoints
            .compiled_condition(id, &body, index, number)
        {
            Some(code) => Ok(code),
            None => evaluation::compile_in(mutator, &frame, text).inspect(|code| {
                mutator.breakpoints.keep_compiled_condition(
                    id,
                    &body,
                    index,
                    number,
                    Rc::clone(code),
                );
            }),
        };
        let begun = code.and_then(|code| {
            evaluation::begin_in(mutator, &frame, code, &[]).map_err(String::from)
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
