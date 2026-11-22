//! The MCP server's tools: their names, descriptions and input schemas,
//! which `tools/list` answers, and their dispatch onto a session, which
//! its worker serves (`docs/design/debugger.md`, "The MCP server", "The
//! tools"). Every tool answers text; a refused or failed call is an error
//! with its reason.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::registry::SessionHandle;
use super::session::{Breakpoint, Session, StartOptions};
use super::trace_files::TraceDirectory;
use super::worker::Inbox;

/// The listings `debug_start` takes, each passed as `--print-<kind>`.
const LISTINGS: [&str; 3] = ["bytecode", "code", "inlining"];

/// How long a tool that lets the program go on waits for its next stop
/// when not told, and how long an evaluation may run before it is
/// aborted.
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// The longest wait a tool takes, whatever it is asked: a client that
/// waits longer holds its session's calls for nothing.
const MAXIMUM_WAIT: Duration = Duration::from_secs(600);

/// How long `debug_stop` waits for the program to run its `ensure:` blocks
/// and end before it kills it.
pub(crate) const STOP_WAIT: Duration = Duration::from_secs(5);

/// The variable a debug build reads the evaluation's wait from, in
/// milliseconds, so that a test of the abort takes a fraction of a second.
#[cfg(debug_assertions)]
const EVALUATION_WAIT_VARIABLE: &str = "NS_NEXT_MCP_EVALUATION_WAIT_MILLISECONDS";

/// How long an evaluation may run before it is aborted.
fn evaluation_wait() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(milliseconds) = std::env::var(EVALUATION_WAIT_VARIABLE)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
    {
        return Duration::from_millis(milliseconds);
    }
    DEFAULT_WAIT
}

/// A tool's answer: its text, or the reason it was refused.
pub(crate) type Answer = Result<String, String>;

/// Whether a tool is named `name`.
pub(crate) fn exists(name: &str) -> bool {
    list()["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name))
}

/// Calls the tool `name` of `session`, every tool's but `debug_start`'s
/// and `debug_stop`'s, which make and end the session.
pub(crate) fn call(session: &mut Session, name: &str, arguments: Value) -> Answer {
    match name {
        "debug_breakpoints" => parse(arguments).and_then(|arguments: BreakpointsArguments| {
            let (text, refused) = session.change_breakpoints(&arguments.set, &arguments.clear);
            if refused { Err(text) } else { Ok(text) }
        }),
        "debug_continue" => parse(arguments).and_then(|arguments: WaitArguments| {
            let wait = wait_of(arguments.wait)?;
            session.resume("continue", json!({ "threadId": 1 }), wait)
        }),
        "debug_step" => step(session, arguments),
        "debug_step_targets" => session.step_targets(),
        "debug_run_to" => run_to(session, arguments),
        "debug_pause" => session.pause(DEFAULT_WAIT),
        "debug_wait" => parse(arguments).and_then(|arguments: SecondsArguments| {
            let wait = wait_of(Some(arguments.seconds))?;
            Ok(session.wait(wait))
        }),
        "debug_stack" => session.stack(),
        "debug_code" => parse(arguments)
            .and_then(|arguments: CodeArguments| session.code(arguments.frame.unwrap_or(0))),
        "debug_variables" => parse(arguments).and_then(|arguments: VariablesArguments| {
            session.variables(
                arguments.frame,
                arguments.reference,
                arguments.start.unwrap_or(0),
                arguments.count.filter(|count| *count > 0),
            )
        }),
        "debug_evaluate" => parse(arguments).and_then(|arguments: EvaluateArguments| {
            session.evaluate(&arguments.expression, arguments.frame, evaluation_wait())
        }),
        "debug_set_variable" => parse(arguments).and_then(|arguments: SetVariableArguments| {
            session.set_variable(&arguments.name, &arguments.value, arguments.frame)
        }),
        "debug_frame" => parse(arguments).and_then(|arguments: FrameArguments| {
            let restart = match arguments.action.as_str() {
                "restart" => true,
                "return" => false,
                other => return Err(format!("{other:?} is no action: restart or return")),
            };
            if restart && arguments.value.is_some() {
                return Err("a restart takes no value".to_string());
            }
            session.frame_action(
                restart,
                arguments.frame,
                arguments.value.as_deref(),
                DEFAULT_WAIT,
            )
        }),
        "debug_memory" => parse(arguments).and_then(|arguments: MemoryArguments| {
            session.memory(arguments.class.as_deref(), arguments.referrers_of)
        }),
        "debug_mark" => parse(arguments).and_then(|arguments: MarkArguments| {
            session.mark(arguments.reference, &arguments.label)
        }),
        _ => Err(format!("no tool is named {name}")),
    }
}

/// `debug_start`: starts the program `arguments` names as the session
/// `handle`, whose worker's inbox is `inbox`, a traced one writing its
/// trace into a file created in `traces`; answers the session and the
/// start's answer, or why it did not start and the inbox back.
pub(crate) fn start(
    arguments: Value,
    default_runtime: &Path,
    traces: &Result<Arc<TraceDirectory>, String>,
    handle: Arc<SessionHandle>,
    inbox: Inbox,
) -> Result<(Session, String), (String, Inbox)> {
    let options = match start_options(arguments, default_runtime, traces, handle.number) {
        Ok(options) => options,
        Err(reason) => return Err((reason, inbox)),
    };
    Session::start(options, handle, inbox)
}

fn start_options(
    arguments: Value,
    default_runtime: &Path,
    traces: &Result<Arc<TraceDirectory>, String>,
    session_number: u32,
) -> Result<StartOptions, String> {
    let arguments: StartArguments = parse(arguments)?;
    let wait = wait_of(arguments.wait)?;
    let mut listings = Vec::new();
    for kind in arguments.listings {
        if !LISTINGS.contains(&kind.as_str()) {
            return Err(format!(
                "{kind:?} is no kind of listing: bytecode, code or inlining"
            ));
        }
        // `run` takes each print option once.
        if !listings.contains(&kind) {
            listings.push(kind);
        }
    }
    // Refused here, as `run` refuses it, but before anything starts.
    if !arguments.listing_filter.is_empty() && listings.is_empty() {
        return Err(
            "a listing_filter selects the bodies whose listings are written: give listings"
                .to_string(),
        );
    }
    let program = std::path::absolute(&arguments.program)
        .map_err(|error| format!("{}: {error}", arguments.program))?;
    if !program.exists() {
        return Err(format!("{} does not exist", program.display()));
    }
    let runtime = match &arguments.runtime {
        Some(runtime) => {
            std::path::absolute(runtime).map_err(|error| format!("{runtime}: {error}"))?
        }
        None => default_runtime.to_path_buf(),
    };
    // Created last, once every argument is known good, and before the
    // child starts, which only writes it.
    let trace_file = if arguments.trace.is_some() || !listings.is_empty() {
        let traces = traces
            .as_ref()
            .map_err(|reason| format!("the server keeps no trace files: {reason}"))?;
        Some(traces.create(session_number)?)
    } else {
        None
    };
    Ok(StartOptions {
        program,
        runtime,
        arguments: arguments.arguments,
        stop_on_entry: arguments.stop_on_entry,
        trace: arguments.trace,
        trace_filter: arguments.trace_filter,
        listings,
        listing_filter: arguments.listing_filter,
        trace_file,
        breakpoints: arguments.breakpoints,
        wait,
    })
}

fn step(session: &mut Session, arguments: Value) -> Answer {
    let arguments: StepArguments = parse(arguments)?;
    let wait = wait_of(arguments.wait)?;
    let (command, mut request) = match arguments.kind.as_str() {
        "over" => ("next", json!({ "threadId": 1 })),
        "into" => ("stepIn", json!({ "threadId": 1 })),
        "out" => ("stepOut", json!({ "threadId": 1 })),
        other => return Err(format!("{other:?} is no kind of step: over, into or out")),
    };
    if let Some(target) = arguments.target {
        if command != "stepIn" {
            return Err("a target is stepped into: kind into".to_string());
        }
        request["targetId"] = json!(target);
    }
    session.resume(command, request, wait)
}

fn run_to(session: &mut Session, arguments: Value) -> Answer {
    let arguments: RunToArguments = parse(arguments)?;
    let wait = wait_of(arguments.wait)?;
    let super::session::Location::Line { path, line } =
        super::session::Location::parse(&arguments.location)?
    else {
        return Err("debug_run_to runs to a line: <absolute path>:<line>".to_string());
    };
    session.resume(
        "nsNext/runToPosition",
        json!({ "source": { "path": path }, "line": line }),
        wait,
    )
}

/// A wait in seconds as a duration, `DEFAULT_WAIT` when none is given.
fn wait_of(seconds: Option<f64>) -> Result<Duration, String> {
    let Some(seconds) = seconds else {
        return Ok(DEFAULT_WAIT);
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("a wait of {seconds} s: give seconds, 0 or more"));
    }
    Ok(Duration::from_secs_f64(
        seconds.min(MAXIMUM_WAIT.as_secs_f64()),
    ))
}

/// A tool's arguments as `T`, or why they are not.
pub(crate) fn parse<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, String> {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(arguments).map_err(|error| format!("the arguments: {error}"))
}

#[derive(Deserialize)]
struct StartArguments {
    program: String,
    #[serde(default)]
    breakpoints: Vec<Breakpoint>,
    #[serde(default)]
    stop_on_entry: bool,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    trace: Option<String>,
    #[serde(default)]
    trace_filter: Vec<String>,
    #[serde(default)]
    listings: Vec<String>,
    #[serde(default)]
    listing_filter: Vec<String>,
    #[serde(default)]
    wait: Option<f64>,
}

#[derive(Deserialize)]
struct BreakpointsArguments {
    #[serde(default)]
    set: Vec<Breakpoint>,
    #[serde(default)]
    clear: Vec<String>,
}

#[derive(Deserialize)]
struct WaitArguments {
    #[serde(default)]
    wait: Option<f64>,
}

#[derive(Deserialize)]
struct SecondsArguments {
    seconds: f64,
}

#[derive(Deserialize)]
struct StepArguments {
    kind: String,
    #[serde(default)]
    target: Option<i64>,
    #[serde(default)]
    wait: Option<f64>,
}

#[derive(Deserialize)]
struct RunToArguments {
    location: String,
    #[serde(default)]
    wait: Option<f64>,
}

#[derive(Deserialize)]
struct VariablesArguments {
    #[serde(default)]
    frame: Option<usize>,
    #[serde(default)]
    reference: Option<i64>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Deserialize)]
struct EvaluateArguments {
    expression: String,
    #[serde(default)]
    frame: Option<usize>,
}

#[derive(Deserialize)]
struct SetVariableArguments {
    name: String,
    value: String,
    #[serde(default)]
    frame: Option<usize>,
}

#[derive(Deserialize)]
struct FrameArguments {
    action: String,
    frame: usize,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize)]
struct CodeArguments {
    #[serde(default)]
    frame: Option<usize>,
}

#[derive(Deserialize)]
struct MemoryArguments {
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    referrers_of: Option<i64>,
}

#[derive(Deserialize)]
struct MarkArguments {
    #[serde(default)]
    reference: Option<i64>,
    label: String,
}

/// The schema of a wait, in seconds.
fn wait_schema() -> Value {
    json!({
        "type": "number",
        "minimum": 0,
        "description": "Seconds to wait for the next stop before answering that the \
                        program runs; 30 by default."
    })
}

/// The schema of a breakpoint.
fn breakpoint_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "location": {
                "type": "string",
                "description": "<absolute path>:<line>, or a method: the module's id, \
                                @ its version for one version only, then / and the \
                                path to a class inside the module, dot-separated, \
                                then >>selector (or ' class>>selector' for the class \
                                side): com.acme.json.Json@2.1.0/Parser>>parse:, \
                                Demo/Account>>deposit:, Demo>>main:args:."
            },
            "condition": {
                "type": "string",
                "description": "A Newspeak expression; the breakpoint stops only where it \
                                answers true."
            },
            "log": {
                "type": "string",
                "description": "A message logged instead of stopping, each {expression} in \
                                it evaluated at the hit."
            },
            "hit_condition": { "type": "string", "description": "Which hits stop: a count, or >= n, or % n." }
        },
        "required": ["location"]
    })
}

/// Every tool, as `tools/list` answers it.
pub(crate) fn list() -> Value {
    let frame = || json!({ "type": "integer", "minimum": 0, "description": "A frame's index on the stack, 0 the top; 0 by default." });
    let reference = || json!({ "type": "integer", "description": "A value's reference, the n of [ref n] in an answer of this stop." });
    let mut tools = [
        tool(
            "debug_start",
            "Starts a Newspeak program (a .ns file or a project) under the debugger as a new session, and answers the session's number on its first line (session: <n>), then the program's first stop, its end, or that it runs. Several sessions can be open at once, each named by its number in the other tools' session; a session lasts until debug_stop closes it or the server ends, also after its program ended.",
            json!({
                "program": { "type": "string", "description": "The program: a .ns file, a project directory or its newspeak.json." },
                "breakpoints": { "type": "array", "items": breakpoint_schema(), "description": "Breakpoints set before the program starts." },
                "stop_on_entry": { "type": "boolean", "description": "Stop at the first statement of main:args:." },
                "arguments": { "type": "array", "items": { "type": "string" }, "description": "The program's arguments." },
                "runtime": { "type": "string", "description": "The ns-next binary to run the program with; this server's own executable by default." },
                "trace": { "type": "string", "description": "What the program traces, as ns-next run's --trace: categories separated by commas (compile, osr, optimize, inlining, deoptimize, code-zone, caches) or all. The program writes the trace into a file the server keeps for the session until debug_stop; nothing of it reaches the program's output." },
                "trace_filter": { "type": "array", "items": { "type": "string" }, "description": "Patterns selecting the bodies whose events are traced, one --trace-filter each, in the method syntax of locations (Demo>>square:, Demo/Account>>deposit:); a pattern ending in ... is a prefix (Demo>>sq...). A comma is part of a selector, never a separator. Only with trace: ns-next refuses a filter without it." },
                "listings": { "type": "array", "items": { "type": "string", "enum": ["bytecode", "code", "inlining"] }, "description": "The listings the program writes into the session's trace file, as ns-next run's --print-bytecode, --print-code and --print-inlining: a body's bytecode, its machine code, an optimizing compile's inlining plan." },
                "listing_filter": { "type": "array", "items": { "type": "string" }, "description": "Patterns selecting the bodies whose listings are written, one --print-filter each, in trace_filter's syntax. Only with listings." },
                "wait": wait_schema()
            }),
            &["program"],
        ),
        tool(
            "debug_breakpoints",
            "Sets and clears breakpoints of the debugged program, and answers every breakpoint set.",
            json!({
                "set": { "type": "array", "items": breakpoint_schema(), "description": "Breakpoints to set; one at a location already set replaces it." },
                "clear": { "type": "array", "items": { "type": "string" }, "description": "Locations whose breakpoints are cleared." }
            }),
            &[],
        ),
        tool(
            "debug_continue",
            "Lets the stopped program go on, and answers its next stop, its end, or that it still runs.",
            json!({ "wait": wait_schema() }),
            &[],
        ),
        tool(
            "debug_step",
            "Steps the stopped program over the current line, into the call it makes, or out of the current method, and answers where it stops.",
            json!({
                "kind": { "type": "string", "enum": ["over", "into", "out"] },
                "target": { "type": "integer", "description": "With kind into: a target of debug_step_targets, the one send or block the step stops in." },
                "wait": wait_schema()
            }),
            &["kind"],
        ),
        tool(
            "debug_step_targets",
            "Lists the sends and blocks of the current statement that a step into can stop in, by the target number debug_step takes.",
            json!({}),
            &[],
        ),
        tool(
            "debug_run_to",
            "Lets the stopped program run to a line, and answers where it stops.",
            json!({
                "location": { "type": "string", "description": "<absolute path>:<line>" },
                "wait": wait_schema()
            }),
            &["location"],
        ),
        tool(
            "debug_pause",
            "Stops the running program where it is, and answers the stop.",
            json!({}),
            &[],
        ),
        tool(
            "debug_wait",
            "Waits for the running program's next stop or end, and answers it, or that it still runs.",
            json!({ "seconds": { "type": "number", "minimum": 0, "description": "Seconds to wait at most." } }),
            &["seconds"],
        ),
        tool(
            "debug_stack",
            "Lists the stopped program's frames, one a line, each with its absolute path, line and column.",
            json!({}),
            &[],
        ),
        tool(
            "debug_code",
            "Answers the code of a frame of the stopped program as ns-next run's --print-bytecode and --print-code print it: its method's or block's bytecode, the instruction the frame stands at marked ->, then the machine code the frame goes on in, the instruction it returns to marked ->, or that it goes on in the interpreter.",
            json!({ "frame": frame() }),
            &[],
        ),
        tool(
            "debug_variables",
            "Lists a frame's variables, or the fields and elements of a value by its reference, one a line.",
            json!({
                "frame": frame(),
                "reference": reference(),
                "start": { "type": "integer", "minimum": 0, "description": "The first to list; 0 by default." },
                "count": { "type": "integer", "minimum": 1, "description": "How many to list; 100 by default." }
            }),
            &[],
        ),
        tool(
            "debug_evaluate",
            "Evaluates a Newspeak expression in a frame of the stopped program, which can name the frame's variables and marked values, and answers its value.",
            json!({ "expression": { "type": "string" }, "frame": frame() }),
            &["expression"],
        ),
        tool(
            "debug_set_variable",
            "Sets a variable of a frame of the stopped program to the value of an expression.",
            json!({
                "name": { "type": "string" },
                "value": { "type": "string", "description": "A Newspeak expression, evaluated in the frame." },
                "frame": frame()
            }),
            &["name", "value"],
        ),
        tool(
            "debug_frame",
            "Restarts a frame of the stopped program from its start, or makes it return a value at once, and answers the stop that follows.",
            json!({
                "action": { "type": "string", "enum": ["restart", "return"] },
                "frame": frame(),
                "value": { "type": "string", "description": "With return: a Newspeak expression evaluated in the frame, whose value it returns; nil without." }
            }),
            &["action", "frame"],
        ),
        tool(
            "debug_memory",
            "Answers how many instances each class has, the instances of one class, or what refers to a value.",
            json!({
                "class": { "type": "string", "description": "A class, named as the counts name it: the module's id, @ its version for one version only, then / and the path to a class inside the module (Demo/Account): list its instances." },
                "referrers_of": reference()
            }),
            &[],
        ),
        tool(
            "debug_mark",
            "Marks a value with a label, by which every later evaluation names it <label>_DebugLabel; without a reference, takes the label off.",
            json!({ "reference": reference(), "label": { "type": "string" } }),
            &["label"],
        ),
        tool(
            "debug_stop",
            "Ends the debugged program, its ensure: blocks run, answers its exit status and its last output, and closes its session, whose number no later call names.",
            json!({}),
            &[],
        ),
        tool(
            "debug_trace",
            "Answers the overview of a traced session's trace file, which debug_start's trace and listings have the program write: its size and counts, what it traces and lists, the events by category, then five lists ranked by their first number, each entry a body by name and #number with the call that tells more: installs (machine code installed, by tier), abandoned (with revivals), deoptimized (retired and invalidated copies, by reason), optimized (copies by key) and declined (callees the inlining declined, by reason). Readable while the program runs. Without list, each list's first entries; with list, that list alone, paged by start and count.",
            json!({
                "list": { "type": "string", "enum": ["installs", "abandoned", "deoptimized", "optimized", "declined"], "description": "One list alone, paged." },
                "start": { "type": "integer", "minimum": 0, "description": "With list: how many of its entries to skip; 0 by default." },
                "count": { "type": "integer", "minimum": 1, "description": "How many entries of each list; 10 by default, 200 at most." }
            }),
            &[],
        ),
        tool(
            "debug_trace_method",
            "Answers a method's journal from a traced session's trace file: every body the name matches -- the method and its blocks, each version's and each edited body's -- by #number, kind and line; each body's machine code installed by tier and, for optimized code, by key, with the indexes of its code and inlining listings; the bodies that inlined it and its declines as a callee; then each body's lifecycle -- its compile, osr, optimize, deoptimize and code-zone events -- apart, in file order under the body's own head line, events of one category and word in a row written once as a ×n and a repeated cycle of up to eight such steps as (a → b) ×n, the first round's fields shown and those that differ summarized beside it -- debug_trace_events with method gives every event, caches included, in the run-wide order; then its sends in bytecode order, each with its line and column, its selector, its latest inlining and its cache's state. Readable while the program runs. Without part, the head and the first page of each; with part, that part alone, paged by start and count.",
            json!({
                "method": { "type": "string", "description": "The method, as the tools name one: the module's id, @ its version for one version only, then / and the path to a class inside the module, then >> and the selector (Demo/Account>>deposit:), a trailing ... making a prefix; or #<n>, a body by the number the trace gives it." },
                "part": { "type": "string", "enum": ["events", "sends"], "description": "One part alone, paged." },
                "start": { "type": "integer", "minimum": 0, "description": "With part: how many of its entries to skip; 0 by default." },
                "count": { "type": "integer", "minimum": 1, "description": "How many entries of each part; 50 by default, 200 at most." }
            }),
            &["method"],
        ),
        tool(
            "debug_trace_listing",
            "Answers a window of one listing from a traced session's trace file, its lines as ns-next run's --print-bytecode, --print-code and --print-inlining write them, after a line that says which listing it is: a body's bytecode, its blocks nested under the instruction that makes them; one copy of its machine code, the head's tier, architecture, key and size, an optimized copy's scope tree, a line naming each stretch's scope and program counter, with its bytecode instruction where the run listed bytecode, then each instruction by its offset with what its calls and jumps reach, and the data words; or one optimizing compile's inlining plan, its scopes, type-case arms, declines and outcome. A name matching several bodies with such listings is answered with each body's #number and listings, to pass one. Readable while the program runs; paged by start and count.",
            json!({
                "method": { "type": "string", "description": "The method, as the tools name one: the module's id, @ its version for one version only, then / and the path to a class inside the module, then >> and the selector (Demo/Account>>deposit:), a trailing ... making a prefix; or #<n>, a body by the number the trace gives it." },
                "kind": { "type": "string", "enum": ["bytecode", "code", "inlining"], "description": "Which listing: the bytecode of the method and its blocks, a copy of its machine code, or an optimizing compile's inlining plan." },
                "index": { "type": "integer", "minimum": 1, "description": "Which of the body's listings of that kind, from 1 in the order the run wrote them, as debug_trace_method's copies line numbers them; the last by default, the copy the program goes on in." },
                "start": { "type": "integer", "minimum": 0, "description": "How many of the listing's lines to skip; 0 by default." },
                "count": { "type": "integer", "minimum": 1, "description": "How many lines; 150 by default, 500 at most." }
            }),
            &["method", "kind"],
        ),
        tool(
            "debug_trace_events",
            "Answers a traced session's events, filtered, in file order and unfolded, each numbered by its place among all the run's events (@1,204) and written as ns-next run --trace writes its line, the body it is about and every body field named {<name> #<n>}, {} for an event about no body. Every filter given must hold. Readable while the program runs; paged by start and count.",
            json!({
                "category": { "type": "string", "description": "One category or several joined by commas, as debug_start's trace takes them: compile, osr, optimize, inlining, deoptimize, code-zone, caches, or all." },
                "event": { "type": "string", "description": "An event's word: declined, retired, or a cache transition as its line writes it (monomorphic->polymorphic) or as transition." },
                "reason": { "type": "string", "description": "An event's reason, cause, by or walk-end, or one of declined-no-body's reasons." },
                "method": { "type": "string", "description": "The method, as the tools name one: the module's id, @ its version for one version only, then / and the path to a class inside the module, then >> and the selector (Demo/Account>>deposit:), a trailing ... making a prefix -- a method's name matching its blocks too; or #<n>, a body by the number the trace gives it. Selects the events about such a body or naming it in a field: spent-by, in-body or callee." },
                "start": { "type": "integer", "minimum": 0, "description": "How many of the selected events to skip; 0 by default." },
                "count": { "type": "integer", "minimum": 1, "description": "How many events; 50 by default, 200 at most." }
            }),
            &[],
        ),
    ];
    // Every tool but the one that opens a session names the session.
    for tool in tools.iter_mut().skip(1) {
        tool["inputSchema"]["properties"]["session"] = json!({
            "type": "integer",
            "minimum": 1,
            "description": "The session, by the number debug_start answered; may be left out while one session is open."
        });
    }
    json!({ "tools": tools })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}
