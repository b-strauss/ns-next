//! The parts of the Debug Adapter Protocol the agent speaks, as `serde`
//! types: what it reads off a request's arguments and what it writes in a
//! response's or an event's body. Every field a client may leave out is an
//! `Option` or has a default, so that a request is refused only for what
//! the agent needs.

use serde::{Deserialize, Serialize};

/// The one thread the agent shows: the VM runs on one
/// (`docs/design/debugger.md`, "Threads").
pub(crate) const THREAD_ID: i64 = 1;

/// A request, as far as the agent reads it before it knows the command.
#[derive(Deserialize, Clone)]
pub(crate) struct Request {
    pub(crate) seq: i64,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) arguments: serde_json::Value,
}

/// A response, `body` left out when it is `None`.
#[derive(Serialize)]
pub(crate) struct Response {
    pub(crate) seq: i64,
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) request_seq: i64,
    pub(crate) success: bool,
    pub(crate) command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) body: Option<serde_json::Value>,
}

/// An event, `body` left out when it is `None`.
#[derive(Serialize)]
pub(crate) struct Event {
    pub(crate) seq: i64,
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) body: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct InitializeArguments {
    pub(crate) lines_start_at1: Option<bool>,
    pub(crate) columns_start_at1: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AttachArguments {
    /// The listening mode's token; the connecting mode needs none.
    pub(crate) token: Option<String>,
    pub(crate) stop_on_entry: bool,
    /// The directory of the binary's shipped sources the client shows as
    /// its core library, absolute: a shipped module is named, and shown,
    /// by the path of its file there (`Sources::take_library`).
    pub(crate) core_library: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct DisconnectArguments {
    pub(crate) terminate_debuggee: bool,
}

/// What `initialize` answers the agent supports.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capabilities {
    pub(crate) supports_configuration_done_request: bool,
    pub(crate) supports_function_breakpoints: bool,
    pub(crate) supports_hit_conditional_breakpoints: bool,
    pub(crate) supports_conditional_breakpoints: bool,
    pub(crate) supports_log_points: bool,
    pub(crate) supports_evaluate_for_hovers: bool,
    pub(crate) supports_set_variable: bool,
    pub(crate) supports_step_in_targets_request: bool,
    pub(crate) supports_restart_frame: bool,
    pub(crate) supports_cancel_request: bool,
    pub(crate) supports_terminate_request: bool,
    pub(crate) supports_exception_filter_options: bool,
    pub(crate) exception_breakpoint_filters: Vec<ExceptionBreakpointsFilter>,
    pub(crate) supports_stepping_granularity: bool,
    pub(crate) support_terminate_debuggee: bool,
    pub(crate) supports_loaded_sources_request: bool,
}

#[derive(Serialize)]
pub(crate) struct ExceptionBreakpointsFilter {
    pub(crate) filter: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) default: bool,
}

impl Capabilities {
    /// The capabilities of this agent.
    pub(crate) fn of_the_agent() -> Capabilities {
        Capabilities {
            supports_configuration_done_request: true,
            supports_function_breakpoints: true,
            supports_hit_conditional_breakpoints: true,
            supports_conditional_breakpoints: true,
            supports_log_points: true,
            supports_evaluate_for_hovers: true,
            supports_set_variable: true,
            supports_step_in_targets_request: true,
            supports_restart_frame: true,
            supports_cancel_request: true,
            supports_terminate_request: true,
            supports_exception_filter_options: true,
            exception_breakpoint_filters: vec![
                ExceptionBreakpointsFilter {
                    filter: "uncaught",
                    label: "Uncaught exceptions",
                    description: "Stops where an exception no handler takes is about to be \
                                  handled by its default action.",
                    default: true,
                },
                ExceptionBreakpointsFilter {
                    filter: "caught",
                    label: "Caught exceptions",
                    description: "Stops where a handler is about to take an exception.",
                    default: false,
                },
                ExceptionBreakpointsFilter {
                    filter: "halt",
                    label: "Halt",
                    description: "Stops at a `halt` no handler takes.",
                    default: true,
                },
            ],
            supports_stepping_granularity: true,
            support_terminate_debuggee: true,
            supports_loaded_sources_request: false,
        }
    }
}

/// The `process` event's body.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessEventBody {
    pub(crate) name: String,
    pub(crate) system_process_id: u32,
    pub(crate) is_local_process: bool,
    pub(crate) start_method: &'static str,
    /// ns-next's own: the secret the launcher of `--debug-connect` handed
    /// the process (`agent::SECRET_VARIABLE`), returned so that it can
    /// tell its process from another that connected first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ns_next_secret: Option<String>,
}

/// The `stopped` event's body.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoppedEventBody {
    pub(crate) reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// An exception's stop: its text, as a client shows it beside the
    /// reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    pub(crate) thread_id: i64,
    pub(crate) all_threads_stopped: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) hit_breakpoint_ids: Vec<u32>,
}

/// The `exited` event's body.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExitedEventBody {
    pub(crate) exit_code: i64,
}

#[derive(Serialize)]
pub(crate) struct Thread {
    pub(crate) id: i64,
    pub(crate) name: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StackTraceArguments {
    pub(crate) start_frame: Option<usize>,
    /// `None` or 0: every frame from `start_frame`.
    pub(crate) levels: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ScopesArguments {
    pub(crate) frame_id: i64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct VariablesArguments {
    pub(crate) variables_reference: i64,
    /// `indexed` or `named`: only the children of that kind.
    pub(crate) filter: Option<String>,
    pub(crate) start: Option<usize>,
    /// `None` or 0: every child from `start`.
    pub(crate) count: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SourceArguments {
    /// The reference, which `source.sourceReference` repeats in a client
    /// that sends both.
    pub(crate) source_reference: Option<i64>,
    pub(crate) source: Option<SourceArgument>,
}

/// A source as a client names it: by the path of a program's file, or by
/// the name and the reference the agent gave it.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SourceArgument {
    pub(crate) name: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) source_reference: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SetBreakpointsArguments {
    pub(crate) source: SourceArgument,
    pub(crate) breakpoints: Vec<SourceBreakpoint>,
}

/// A line breakpoint as the client asks for it; with a `column`, the
/// variant that stops in a block body beginning there.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SourceBreakpoint {
    pub(crate) line: u32,
    pub(crate) column: Option<u32>,
    pub(crate) hit_condition: Option<String>,
    /// An expression the breakpoint stops only where it answers `true`;
    /// a blank one is none.
    pub(crate) condition: Option<String>,
    /// A logpoint's message, its expressions in braces: the breakpoint
    /// logs it instead of stopping; an empty one is none.
    pub(crate) log_message: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SetFunctionBreakpointsArguments {
    pub(crate) breakpoints: Vec<FunctionBreakpoint>,
}

/// A method breakpoint, its method named `Module.Path.To.Class>>selector`
/// or `Module.Path.To.Class class>>selector`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct FunctionBreakpoint {
    pub(crate) name: String,
    pub(crate) hit_condition: Option<String>,
    /// As `SourceBreakpoint::condition`.
    pub(crate) condition: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SetExceptionBreakpointsArguments {
    pub(crate) filters: Vec<String>,
    pub(crate) filter_options: Vec<ExceptionFilterOptions>,
}

/// A filter turned on with a condition: the classes, each
/// `Module.Path.To.Class`, separated by commas, whose exceptions it stops.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ExceptionFilterOptions {
    pub(crate) filter_id: String,
    pub(crate) condition: Option<String>,
}

/// `next`, `stepIn` and `stepOut`: `line` (the default), `statement`, or
/// `instruction`, which steps as `statement` does; `stepIn`'s `targetId`
/// is a target `stepInTargets` answered at this stop, which the step
/// stops at alone, whatever its granularity.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StepArguments {
    pub(crate) granularity: Option<String>,
    pub(crate) target_id: Option<i64>,
}

/// `stepInTargets`: the targets of the frame `frameId` names, which must
/// be the one a step steps from.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StepInTargetsArguments {
    pub(crate) frame_id: i64,
}

/// One target of a `stepInTargets` response, placed at its selector's
/// first token or at its block literal; unplaced where the module's text
/// is not known.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepInTarget {
    pub(crate) id: i64,
    pub(crate) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end_column: Option<u32>,
}

/// `restartFrame`: the frame `frameId` names runs again from its start.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RestartFrameArguments {
    pub(crate) frame_id: i64,
}

/// `nsNext/forceReturn`, a request of ns-next's own: the frame `frameId`
/// names returns at once what `expression`, evaluated in that frame,
/// answers -- `nil` without one, or with a blank one.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ForceReturnArguments {
    pub(crate) frame_id: i64,
    pub(crate) expression: Option<String>,
}

/// `nsNext/instances`, a request of ns-next's own: `count` of the
/// instances of the class declaration `name` names, `Module.Path.To.Class`
/// as `nsNext/instanceCounts` names it, from the `start`th. `shipped`
/// picks the shipped module, or the program's, of a name both have,
/// which `nsNext/instanceCounts` says of each class; without it, the
/// program's module is taken where it has one.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct InstancesArguments {
    pub(crate) name: String,
    pub(crate) shipped: Option<bool>,
    pub(crate) start: Option<usize>,
    /// `None` or 0: every one from `start`.
    pub(crate) count: Option<usize>,
}

/// `nsNext/referrers`, a request of ns-next's own: `count` of what refers
/// to the value `valueReference` names, from the `start`th.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ReferrersArguments {
    pub(crate) value_reference: i64,
    pub(crate) start: Option<usize>,
    /// `None` or 0: every one from `start`.
    pub(crate) count: Option<usize>,
}

/// One class of an `nsNext/instanceCounts` response.
#[derive(Serialize)]
pub(crate) struct InstanceCount {
    /// `Module.Path.To.Class`, as `nsNext/instances` takes it.
    pub(crate) name: String,
    pub(crate) count: usize,
    /// ns-next's own: the class is a shipped module's, which a program
    /// module of the same name does not declare.
    #[serde(rename = "nsNext/shipped", skip_serializing_if = "std::ops::Not::not")]
    pub(crate) shipped: bool,
}

/// `nsNext/runToPosition`, a request of ns-next's own: the program goes
/// on to the statement at or after this place.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RunToPositionArguments {
    pub(crate) source: SourceArgument,
    pub(crate) line: u32,
    pub(crate) column: Option<u32>,
}

/// The `breakpoint` event's body.
#[derive(Serialize)]
pub(crate) struct BreakpointEventBody {
    pub(crate) reason: &'static str,
    pub(crate) breakpoint: Breakpoint,
}

/// `nsNext/setStepFilters`, a request of ns-next's own: whether a step
/// passes the shipped modules' code, and the entries, each a module's
/// source and a class in it, whose code it passes too.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetStepFiltersArguments {
    #[serde(default = "filter_the_shipped_modules")]
    pub(crate) shipped: bool,
    #[serde(default)]
    pub(crate) entries: Vec<StepFilterEntry>,
    /// The typed class names `Module.Path.To.Class` the request once
    /// took, which `entries` replaced: read only to refuse a client that
    /// still sends them, rather than let its filters lapse unsaid.
    #[serde(default)]
    pub(crate) classes: Option<serde_json::Value>,
}

/// A step filter's entry: the module its `source` names, as a
/// breakpoint's names it -- a program module's file, a file of the core
/// library taken, or a reference -- and in it the class declaration
/// `classPath` names from the module's root down, the root's own name
/// left out, so that an empty path names the whole module; the entry
/// filters that declaration and every one nested in it.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StepFilterEntry {
    pub(crate) source: SourceArgument,
    pub(crate) class_path: Vec<String>,
}

/// The VM's default, which a request that leaves `shipped` out keeps.
fn filter_the_shipped_modules() -> bool {
    true
}

impl Default for SetStepFiltersArguments {
    fn default() -> SetStepFiltersArguments {
        SetStepFiltersArguments {
            shipped: filter_the_shipped_modules(),
            entries: Vec::new(),
            classes: None,
        }
    }
}

/// A source as the agent shows it: a program's file by its path, any
/// other module by a reference whose text the `source` request answers.
#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_reference: Option<i64>,
    /// `deemphasize` for a shipped frame's source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) presentation_hint: Option<&'static str>,
}

/// One frame of a `stackTrace` response; a frame without a location has
/// no source and line 0.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StackFrame {
    pub(crate) id: i64,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<Source>,
    pub(crate) line: u32,
    pub(crate) column: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end_column: Option<u32>,
    /// `subtle` for a frame a client shows only on demand or folds away:
    /// a hidden frame, and a shipped module's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) presentation_hint: Option<&'static str>,
    /// `false` for a frame `restartFrame` refuses, a shipped module's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) can_restart: Option<bool>,
    /// ns-next's own: the frame runs a body of a shipped module.
    #[serde(rename = "nsNext/shipped", skip_serializing_if = "std::ops::Not::not")]
    pub(crate) shipped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Scope {
    pub(crate) name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) presentation_hint: Option<&'static str>,
    pub(crate) variables_reference: i64,
    pub(crate) expensive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Variable {
    pub(crate) name: String,
    pub(crate) value: String,
    #[serde(flatten)]
    pub(crate) shown: Shown,
}

/// `evaluate`'s body.
#[derive(Serialize)]
pub(crate) struct EvaluateResponseBody {
    pub(crate) result: String,
    #[serde(flatten)]
    pub(crate) shown: Shown,
}

/// `setVariable`'s body.
#[derive(Serialize)]
pub(crate) struct SetVariableResponseBody {
    pub(crate) value: String,
    #[serde(flatten)]
    pub(crate) shown: Shown,
}

/// What a variable, an `evaluate` and a `setVariable` say of a value
/// beside its text.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shown {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) class_name: Option<String>,
    /// 0 for a value without children.
    pub(crate) variables_reference: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) indexed_variables: Option<usize>,
    /// `readOnly` for a withheld value, which can be neither marked nor
    /// forced as a frame's return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) presentation_hint: Option<VariablePresentationHint>,
    /// ns-next's own: the reference by which `nsNext/mark` and
    /// `nsNext/forceReturn` name the value, valid until the program goes
    /// on; none for a withheld value, nor where no value is shown.
    #[serde(
        rename = "nsNext/valueReference",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) value_reference: Option<i64>,
}

/// `attributes` `readOnly` for a withheld value; `kind` `virtual` for a
/// variable that names a place rather than holds a value: a referrer on
/// the stack.
#[derive(Serialize, Default)]
pub(crate) struct VariablePresentationHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) attributes: Vec<&'static str>,
}

/// `evaluate`: `expression` in the frame `frameId` names, the top frame
/// without one. Every `context` -- `repl`, `watch`, `hover`,
/// `clipboard` -- is evaluated alike, but a hover's is aborted past a
/// bound (`agent::HOVER_BOUND`).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct EvaluateArguments {
    pub(crate) expression: String,
    pub(crate) frame_id: Option<i64>,
    pub(crate) context: Option<String>,
}

/// `cancel`: the request `requestId` names is cancelled, which only an
/// `evaluate` the VM has not answered heeds.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CancelArguments {
    pub(crate) request_id: Option<i64>,
}

/// `setVariable`: the variable `name` of the frame whose `Locals` scope
/// `variablesReference` names, set to what the expression `value`
/// answers.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SetVariableArguments {
    pub(crate) variables_reference: i64,
    pub(crate) name: String,
    pub(crate) value: String,
}

/// `nsNext/mark`, a request of ns-next's own: marks the value
/// `valueReference` names (`Shown::value_reference`) with `label`, by
/// which every later evaluation names it `<label>_DebugLabel`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct MarkArguments {
    pub(crate) value_reference: i64,
    pub(crate) label: String,
}

/// `nsNext/unmark`, a request of ns-next's own: takes the mark `label`
/// off the object it marks.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UnmarkArguments {
    pub(crate) label: String,
}

/// One breakpoint of a `setBreakpoints` response.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Breakpoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u32>,
    pub(crate) verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) column: Option<u32>,
}

impl Breakpoint {
    /// A breakpoint that stands, for which there is nothing else to say:
    /// an exception filter's.
    pub(crate) fn verified() -> Breakpoint {
        Breakpoint {
            id: None,
            verified: true,
            message: None,
            line: None,
            column: None,
        }
    }

    /// A breakpoint the VM does not stand with, for `message`.
    pub(crate) fn unverified(message: &str) -> Breakpoint {
        Breakpoint {
            id: None,
            verified: false,
            message: Some(message.to_string()),
            line: None,
            column: None,
        }
    }
}
