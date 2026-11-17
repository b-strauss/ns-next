//! `evaluate`, `setVariable` and the marks (`nsNext/mark`,
//! `nsNext/unmark`): the requests that run an expression at a stop or
//! name its values (`docs/design/debugger.md`, decision 8, "Writing a
//! frame", "Marking objects"). Each is served only at a stop, by the VM
//! command of its name, and answered when the VM answered.

use ns_runtime::debugger::Evaluation;
use ns_runtime::{Command, Event};
use serde_json::Value;

use super::agent::{Agent, Conversion, to_value};
use super::protocol;
use super::stopped::Target;

/// The message of an evaluation the VM stopped before it answered: a
/// hover's past its bound, or one a pause aborted.
const ABORTED: &str = "evaluation aborted";

impl Agent {
    /// `evaluate`: the expression in the frame its `frameId` names, the
    /// top frame without one, whatever its `context`; a hover's is
    /// aborted past `HOVER_BOUND`. An exception that ended it is a failed
    /// response whose message describes it, as is the VM's refusal.
    pub(super) fn evaluate(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::EvaluateArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let frame = match arguments.frame_id {
            Some(frame_id) => self.stop.frame_of_id(frame_id),
            None => Some(0),
        };
        let Some(frame) = frame else {
            self.respond_error(connection, request, "no frame of this stop has that id");
            return;
        };
        let command = Command::Evaluate {
            frame,
            expression: arguments.expression,
        };
        let conversion = Conversion::Evaluated {
            generation: self.stop.generation(),
        };
        if self.forward_converted(connection, request, command, conversion)
            && arguments.context.as_deref() == Some("hover")
        {
            self.bound_hover(connection, request);
        }
    }

    /// `setVariable`: a variable of the frame whose `Locals` scope the
    /// reference names, set to what the expression `value` answers; a
    /// value's children and the return value are no frame's variables.
    pub(super) fn set_variable(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::SetVariableArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let frame = match self.stop.target(arguments.variables_reference) {
            Some(Target::Frame(frame)) => frame,
            Some(Target::Value { .. } | Target::Returned(_)) => {
                self.respond_error(connection, request, "only a frame's variables are set");
                return;
            }
            None => {
                self.respond_error(
                    connection,
                    request,
                    "no variables of this stop have that reference",
                );
                return;
            }
        };
        let command = Command::SetVariable {
            frame,
            name: arguments.name,
            expression: arguments.value,
        };
        let conversion = Conversion::VariableSet {
            generation: self.stop.generation(),
        };
        self.forward_converted(connection, request, command, conversion);
    }

    /// `nsNext/mark`: marks the value its value reference names with its
    /// label, which every later evaluation names it by; the VM refuses a
    /// label that is no identifier.
    pub(super) fn mark(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::MarkArguments>(connection, request) else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let Some(value) = self.stop.value(arguments.value_reference) else {
            self.respond_error(
                connection,
                request,
                "no value of this stop has that reference",
            );
            return;
        };
        let command = Command::Mark {
            value,
            label: arguments.label,
        };
        self.forward(connection, request, command);
    }

    /// `nsNext/unmark`: takes the mark of its label off the object it
    /// marks, if one does.
    pub(super) fn unmark(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::UnmarkArguments>(connection, request)
        else {
            return;
        };
        if self.at_stop(connection, request) {
            let command = Command::Unmark {
                label: arguments.label,
            };
            self.forward(connection, request, command);
        }
    }

    /// The body of an `evaluate`'s response, or of a `setVariable`'s
    /// (`setting`), made of the VM's answer.
    pub(super) fn evaluation_body(
        &mut self,
        event: Event,
        setting: bool,
    ) -> Result<Option<Value>, String> {
        let (text, shown) = match event {
            Event::Evaluated(Evaluation::Value(value)) => self.stop.shown(&value),
            Event::Evaluated(Evaluation::Error { description, .. }) => return Err(description),
            Event::Evaluated(Evaluation::Aborted) => return Err(ABORTED.to_string()),
            Event::VariableSet(variable) => {
                let protocol::Variable { value, shown, .. } = self.stop.variable(&variable);
                (value, shown)
            }
            _ => return Ok(None),
        };
        let body = if setting {
            to_value(protocol::SetVariableResponseBody { value: text, shown })
        } else {
            to_value(protocol::EvaluateResponseBody {
                result: text,
                shown,
            })
        };
        Ok(Some(body))
    }
}
