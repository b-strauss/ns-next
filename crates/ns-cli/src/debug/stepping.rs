//! The steps, run to cursor and the step filters. The VM steps by
//! statement; the agent steps by line on top of it, as the JDWP agent
//! steps by its line table (`docs/design/debugger.md`, decision 4): a
//! step by line repeats the VM's step while it stops in the activation it
//! began in -- the same stack depth and frame name, the VM's step never
//! having gone on from another frame -- on the line it began on, and
//! reports only the stop that leaves the line or the activation: a
//! block's next activation is another place, though it stands at the
//! same depth, in a frame of the same name, on the same line. A stop for
//! any other reason -- a breakpoint, an exception, a pause -- ends it at
//! once.

use ns_runtime::{Command, FrameDescription, StopReason};

use super::agent::{Agent, Conversion, Reply};
use super::breakpoints::{offset_of, parse_class_name};
use super::protocol;
use super::stopped::Frames;

/// How far a step goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Granularity {
    Line,
    Statement,
}

/// A step's `granularity`: `line` when the client leaves it out; an
/// `instruction`, which ns-next's statements are the smallest of, steps
/// as `statement` does.
pub(crate) fn granularity(text: Option<&str>) -> Granularity {
    match text {
        Some("statement" | "instruction") => Granularity::Statement,
        _ => Granularity::Line,
    }
}

/// Where a stop stands, as a step by line compares it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Place {
    /// The number of frames on the stack.
    depth: usize,
    /// The top frame's name.
    name: String,
    /// The 0-based line of the top frame's statement; `None` for a frame
    /// without a location.
    line: Option<u32>,
}

impl Place {
    /// The place of a stop whose frames are `frames` from the first it
    /// shows down, that frame's statement on `line`.
    pub(crate) fn of(frames: &[FrameDescription], line: Option<u32>) -> Place {
        Place {
            depth: frames.len(),
            name: frames
                .first()
                .map(|frame| frame.name.clone())
                .unwrap_or_default(),
            line,
        }
    }
}

/// Whether a step by line that began at `origin` steps again from a stop
/// at `here`, `elsewhere` whether the VM's step went on from another frame
/// than the one it was taken in (`Event::Stopped`): the same activation on
/// the same line. A frame without a line stops the step, since it cannot
/// be told to be on the same one, and so does a step that went on
/// elsewhere, whose stop stands in another activation even where its
/// depth, name and line are the origin's -- a block's next activation.
pub(crate) fn steps_on(origin: &Place, here: &Place, elsewhere: bool) -> bool {
    !elsewhere && origin.line.is_some() && origin == here
}

/// Where a step by line is.
pub(crate) enum Phase {
    /// Reading the place of the stop it begins at, before the VM's first
    /// step; the request is answered when the VM answers that step.
    Origin {
        connection: u64,
        request: protocol::Request,
    },
    /// The VM steps; the program runs.
    Running { origin: Place },
    /// The VM's step stopped; the agent reads where, to step again or to
    /// report the stop. The client has not seen this stop. `elsewhere` is
    /// whether the VM's step went on from another frame than the one it
    /// was taken in.
    Deciding { origin: Place, elsewhere: bool },
}

/// A step by line in progress.
pub(crate) struct LineStep {
    /// The VM's step it repeats: `StepOver` or `StepInto`.
    command: Command,
    pub(crate) phase: Phase,
    /// The client paused meanwhile: the next stop is reported, whatever
    /// its place.
    pub(crate) interrupted: bool,
}

impl Agent {
    /// `next` and `stepIn`: one VM step for a statement, or a step by line.
    pub(super) fn step(&mut self, connection: u64, request: &protocol::Request, command: Command) {
        let Some(arguments) = self.arguments::<protocol::StepArguments>(connection, request) else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        match granularity(arguments.granularity.as_deref()) {
            Granularity::Statement => self.forward(connection, request, command),
            Granularity::Line => {
                self.line_step = Some(LineStep {
                    command,
                    phase: Phase::Origin {
                        connection,
                        request: request.clone(),
                    },
                    interrupted: false,
                });
                self.advance_line_step();
            }
        }
    }

    /// `stepOut`: one VM step, to where the top frame's caller goes on.
    pub(super) fn step_out(&mut self, connection: u64, request: &protocol::Request) {
        if self
            .arguments::<protocol::StepArguments>(connection, request)
            .is_some()
            && self.at_stop(connection, request)
        {
            self.forward(connection, request, Command::StepOut);
        }
    }

    /// `continue`.
    pub(super) fn continue_running(&mut self, connection: u64, request: &protocol::Request) {
        if self.at_stop(connection, request) {
            self.forward(connection, request, Command::Continue);
        }
    }

    /// `nsNext/runToPosition`: the program goes on to the statement at or
    /// after a line, or a line and column, of a module, counted as
    /// `setBreakpoints` counts them. The VM's refusal -- a module not
    /// built, no statement there -- is the response's error.
    pub(super) fn run_to_position(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) =
            self.arguments::<protocol::RunToPositionArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let Some(module) = self.sources.module_named_by(&arguments.source) else {
            self.respond_error(connection, request, "not a module of this program");
            return;
        };
        let Some(text) = self.sources.text_for_positions(&module) else {
            self.respond_error(connection, request, "the module's text is not known yet");
            return;
        };
        let Some(offset) = offset_of(text, arguments.line, arguments.column) else {
            self.respond_error(connection, request, "the module has no such line");
            return;
        };
        self.forward(
            connection,
            request,
            Command::RunToPosition { module, offset },
        );
    }

    /// `nsNext/setStepFilters`, a request of ns-next's own: whether a step
    /// passes the shipped modules' code, and the classes, each
    /// `Module.Path.To.Class`, whose code it passes too.
    pub(super) fn set_step_filters(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) =
            self.arguments::<protocol::SetStepFiltersArguments>(connection, request)
        else {
            return;
        };
        let classes: Result<Vec<_>, String> = arguments
            .classes
            .iter()
            .map(|name| {
                parse_class_name(name, &|module| self.sources.modules_of_class_name(module))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|classes| classes.into_iter().flatten().collect());
        let classes = match classes {
            Ok(classes) => classes,
            Err(message) => {
                self.respond_error(connection, request, &message);
                return;
            }
        };
        let command = Command::SetStepFilters {
            shipped: arguments.shipped,
            classes,
        };
        self.forward(connection, request, command);
    }

    /// A stop of the VM while a step by line runs: the step's own stop is
    /// decided on; any other ends the step, and is reported as it is.
    /// Answers whether the stop is the step's to decide. `elsewhere` is
    /// the stop's word on whether the VM's step went on from another
    /// frame (`Event::Stopped`).
    pub(super) fn line_step_stopped(&mut self, reason: &StopReason, elsewhere: bool) -> bool {
        let Some(step) = self.line_step.take() else {
            return false;
        };
        let Phase::Running { origin } = step.phase else {
            return false;
        };
        if *reason != StopReason::Step {
            return false;
        }
        self.line_step = Some(LineStep {
            phase: Phase::Deciding { origin, elsewhere },
            ..step
        });
        self.advance_line_step();
        true
    }

    /// Takes the step by line on as far as what the VM answered allows:
    /// its first step once the place it begins at is read, and, at its
    /// step's stop, another step or the stop reported.
    pub(super) fn advance_line_step(&mut self) {
        let Some(step) = self.line_step.take() else {
            return;
        };
        match step.phase {
            Phase::Running { .. } => self.line_step = Some(step),
            Phase::Origin {
                connection,
                request,
            } => match self.top_place() {
                None => {
                    self.line_step = Some(LineStep {
                        phase: Phase::Origin {
                            connection,
                            request,
                        },
                        ..step
                    });
                }
                Some(Err(reason)) => self.respond_error(connection, &request, &reason),
                Some(Ok(origin)) => {
                    let reply = Reply::Request {
                        connection,
                        request_seq: request.seq,
                        command: request.command.clone(),
                        conversion: Conversion::Plain,
                    };
                    if self.command(step.command.clone(), reply) {
                        self.resuming = true;
                        self.line_step = Some(LineStep {
                            phase: Phase::Running { origin },
                            ..step
                        });
                    } else {
                        self.respond_error(connection, &request, "the debugger is detached");
                    }
                }
            },
            Phase::Deciding { origin, elsewhere } => {
                if step.interrupted {
                    self.report_stop(&StopReason::Pause);
                    return;
                }
                let here = match self.top_place() {
                    None => {
                        self.line_step = Some(LineStep {
                            phase: Phase::Deciding { origin, elsewhere },
                            ..step
                        });
                        return;
                    }
                    Some(Err(_)) => None,
                    Some(Ok(here)) => Some(here),
                };
                let again = here.is_some_and(|here| steps_on(&origin, &here, elsewhere));
                if again && self.command(step.command.clone(), Reply::Repeated) {
                    self.resuming = true;
                    self.line_step = Some(LineStep {
                        phase: Phase::Running { origin },
                        ..step
                    });
                } else {
                    self.report_stop(&StopReason::Step);
                }
            }
        }
    }

    /// The place of the current stop; `None` while the VM has not answered
    /// the frames or the top frame's module's text, which it is asked for.
    fn top_place(&mut self) -> Option<Result<Place, String>> {
        if let Err(reason) = self.frames_known()? {
            return Some(Err(reason));
        }
        let Frames::Known(frames) = self.stop.frames() else {
            return Some(Err("the stop's frames are not known".to_string()));
        };
        // An exception's stop hides its machinery above the frame it shows,
        // which the VM steps from (`FrameDescription::hidden`): the step is
        // taken there, and its place is that frame's.
        let shown = frames.iter().position(|frame| !frame.hidden).unwrap_or(0);
        let frames = frames[shown..].to_vec();
        let Some(location) = frames.first().and_then(|top| top.location.clone()) else {
            return Some(Ok(Place::of(&frames, None)));
        };
        match self.read_text(&location.module) {
            Ok(true) => {}
            Ok(false) => return None,
            Err(reason) => return Some(Err(reason)),
        }
        let line = self
            .sources
            .range(&location.module, location.statement)
            .map(|(start, _)| start.line);
        Some(Ok(Place::of(&frames, line)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(depth: usize, name: &str, line: Option<u32>) -> Place {
        Place {
            depth,
            name: name.to_string(),
            line,
        }
    }

    #[test]
    fn a_step_by_line_goes_on_only_in_its_activation_on_its_line() {
        let origin = place(2, "Calls>>main:args:", Some(3));
        let same = place(2, "Calls>>main:args:", Some(3));
        assert!(steps_on(&origin, &same, false));
        assert!(!steps_on(&origin, &same, true));
        assert!(!steps_on(
            &origin,
            &place(2, "Calls>>main:args:", Some(4)),
            false
        ));
        assert!(!steps_on(
            &origin,
            &place(3, "Calls>>main:args:", Some(3)),
            false
        ));
        assert!(!steps_on(
            &origin,
            &place(2, "Calls>>double:", Some(3)),
            false
        ));
        let nowhere = place(2, "Calls>>main:args:", None);
        assert!(!steps_on(&nowhere, &nowhere.clone(), false));
    }

    #[test]
    fn a_step_goes_by_line_unless_asked_otherwise() {
        assert_eq!(granularity(None), Granularity::Line);
        assert_eq!(granularity(Some("line")), Granularity::Line);
        assert_eq!(granularity(Some("statement")), Granularity::Statement);
        assert_eq!(granularity(Some("instruction")), Granularity::Statement);
    }
}
