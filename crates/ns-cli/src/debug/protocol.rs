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

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SourceBreakpoint {
    pub(crate) line: u32,
}

/// `nsNext/setStepFilters`, a request of ns-next's own: whether a step
/// passes the shipped modules' code, and the classes, each
/// `Module.Path.To.Class`, whose code it passes too.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetStepFiltersArguments {
    #[serde(default = "filter_the_shipped_modules")]
    pub(crate) shipped: bool,
    #[serde(default)]
    pub(crate) classes: Vec<String>,
}

/// The VM's default, which a request that leaves `shipped` out keeps.
fn filter_the_shipped_modules() -> bool {
    true
}

impl Default for SetStepFiltersArguments {
    fn default() -> SetStepFiltersArguments {
        SetStepFiltersArguments {
            shipped: filter_the_shipped_modules(),
            classes: Vec::new(),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) presentation_hint: Option<&'static str>,
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
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) class_name: Option<String>,
    /// 0 for a value without children.
    pub(crate) variables_reference: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) indexed_variables: Option<usize>,
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
