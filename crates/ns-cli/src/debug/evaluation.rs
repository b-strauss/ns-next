//! `evaluate`, `setVariable`, the marks (`nsNext/mark`, `nsNext/unmark`),
//! the forced return (`nsNext/forceReturn`) and the memory view
//! (`nsNext/instanceCounts`, `nsNext/instances`, `nsNext/referrers`): the
//! requests that run an expression at a stop or name its values
//! (`docs/design/debugger.md`, decision 8, "Writing a frame", "Marking
//! objects", "Restart frame and force return", "The memory view"). Each
//! is served only at a stop, by the VM command of its name, and answered
//! when the VM answered.

use ns_runtime::debugger::{Evaluation, InstanceCount, Referrer};
use ns_runtime::{ClassName, Command, Event, ModuleName, ValueId};
use serde_json::{Value, json};

use super::agent::{Agent, Conversion, to_value};
use super::breakpoints::parse_class_name;
use super::protocol;
use super::stepping::NO_SUCH_FRAME;
use super::stopped::Target;

/// The message of an evaluation the VM stopped before it answered: a
/// hover's past its bound, or one a pause aborted.
const ABORTED: &str = "evaluation aborted";

/// The refusal of a value reference no value of the stop has.
const NO_SUCH_VALUE: &str = "no value of this stop has that reference";

/// The refusal of a forced return of a withheld value, in the VM's words
/// for a withheld value it is asked to take.
const WITHHELD: &str = "the value is withheld";

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
            self.respond_error(connection, request, NO_SUCH_VALUE);
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

    /// `nsNext/forceReturn`: the frame its id names returns at once
    /// (`Command::ForceReturn`) -- `nil` without an expression, or what
    /// its expression answers, evaluated in that frame first as
    /// `evaluate` evaluates one. An expression that does not answer a
    /// value the frame may return -- an error, an abort, a withheld value
    /// -- fails the request and forces nothing, the program still stopped.
    pub(super) fn force_return(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::ForceReturnArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let Some(frame) = self.stop.frame_of_id(arguments.frame_id) else {
            self.respond_error(connection, request, NO_SUCH_FRAME);
            return;
        };
        match arguments
            .expression
            .filter(|expression| !expression.trim().is_empty())
        {
            None => self.forward(
                connection,
                request,
                Command::ForceReturn { frame, value: None },
            ),
            Some(expression) => {
                let conversion = Conversion::ForceReturning {
                    generation: self.stop.generation(),
                    frame,
                };
                let command = Command::Evaluate { frame, expression };
                self.forward_converted(connection, request, command, conversion);
            }
        }
    }

    /// A forced return's evaluation answered `event`: the frame at `frame`
    /// is forced to return its value, the request answered when the VM
    /// answers that, or the request fails.
    pub(super) fn force_evaluated(
        &mut self,
        connection: u64,
        request: &protocol::Request,
        event: Event,
        generation: u64,
        frame: usize,
    ) {
        match self.forced_value(event, generation) {
            Ok(value) => {
                let command = Command::ForceReturn {
                    frame,
                    value: Some(value),
                };
                self.forward(connection, request, command);
            }
            Err(message) => self.respond_error(connection, request, &message),
        }
    }

    /// The value a forced return's evaluation answered, asked at the stop
    /// `generation`, or why it forces none.
    fn forced_value(&self, event: Event, generation: u64) -> Result<ValueId, String> {
        if generation != self.stop.generation() {
            return Err("the program went on".to_string());
        }
        match event {
            Event::Evaluated(Evaluation::Value(value)) if value.withheld => {
                Err(WITHHELD.to_string())
            }
            Event::Evaluated(Evaluation::Value(value)) => Ok(value.id),
            Event::Evaluated(Evaluation::Error { description, .. }) => Err(description),
            Event::Evaluated(Evaluation::Aborted) => Err(ABORTED.to_string()),
            Event::Refused { reason, .. } => Err(reason),
            _ => Err("the evaluation answered no value".to_string()),
        }
    }

    /// `nsNext/instanceCounts`: how many instances each class declaration
    /// has (`Command::InstanceCounts`).
    pub(super) fn instance_counts(&mut self, connection: u64, request: &protocol::Request) {
        if self.at_stop(connection, request) {
            let command = Command::InstanceCounts;
            self.forward_converted(connection, request, command, Conversion::InstanceCounts);
        }
    }

    /// `nsNext/instances`: a page of the instances of the class
    /// declaration its name names (`Command::Instances`), answered as
    /// `variables` answers, each instance named `#<n>`. A name whose
    /// module both the program and the binary have names the program's
    /// unless `shipped` says otherwise, as `nsNext/instanceCounts` tells
    /// the two apart.
    pub(super) fn instances(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::InstancesArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let parsed = match arguments.shipped {
            Some(shipped) => parse_class_name(&arguments.name, &|name| {
                vec![ModuleName {
                    name: name.to_string(),
                    shipped,
                }]
            }),
            None => parse_class_name(&arguments.name, &|name| {
                self.sources.modules_of_class_name(name)
            }),
        };
        let class = match parsed {
            Ok(classes) => classes.into_iter().next(),
            Err(reason) => {
                self.respond_error(connection, request, &reason);
                return;
            }
        };
        let Some(class) = class else {
            self.respond(connection, request, Some(json!({ "variables": [] })));
            return;
        };
        let command = Command::Instances {
            class,
            start: arguments.start.unwrap_or(0),
            count: page_count(arguments.count),
        };
        let conversion = Conversion::Variables {
            generation: self.stop.generation(),
            page: None,
        };
        self.forward_converted(connection, request, command, conversion);
    }

    /// `nsNext/referrers`: a page of what refers to the value its value
    /// reference names (`Command::Referrers`), answered as `variables`
    /// answers (`referrers_body`).
    pub(super) fn referrers(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::ReferrersArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let Some(value) = self.stop.value(arguments.value_reference) else {
            self.respond_error(connection, request, NO_SUCH_VALUE);
            return;
        };
        let command = Command::Referrers {
            value,
            start: arguments.start.unwrap_or(0),
            count: page_count(arguments.count),
        };
        let conversion = Conversion::Referrers {
            generation: self.stop.generation(),
        };
        self.forward_converted(connection, request, command, conversion);
    }

    /// `nsNext/referrers`' body: an object that refers to the value as a
    /// variable shows a value, and a frame's variable that holds it as a
    /// `virtual` variable named `frame <id> <name>` (`Stop::stack_referrer`).
    pub(super) fn referrers_body(&mut self, referrers: &[Referrer]) -> Value {
        let variables: Vec<protocol::Variable> = referrers
            .iter()
            .map(|referrer| match referrer {
                Referrer::Object(variable) => self.stop.variable(variable),
                Referrer::Stack { frame, name } => self.stop.stack_referrer(*frame, name),
            })
            .collect();
        json!({ "variables": variables })
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

/// A page's count as the VM takes it: every one from its start for none
/// or 0, as `variables` pages.
fn page_count(count: Option<usize>) -> usize {
    count.filter(|count| *count > 0).unwrap_or(usize::MAX)
}

/// The name a class declaration is shown and asked for by,
/// `Module.Path.To.Class`, as a function breakpoint names one.
fn class_text(class: &ClassName) -> String {
    std::iter::once(class.module.name.as_str())
        .chain(class.path.iter().map(String::as_str))
        .collect::<Vec<&str>>()
        .join(".")
}

/// `nsNext/instanceCounts`' body, in the VM's order: the largest count
/// first.
pub(super) fn instance_counts_body(counts: &[InstanceCount]) -> Value {
    let classes: Vec<protocol::InstanceCount> = counts
        .iter()
        .map(|count| protocol::InstanceCount {
            name: class_text(&count.class),
            count: count.count,
            shipped: count.class.module.shipped,
        })
        .collect();
    json!({ "classes": classes })
}
