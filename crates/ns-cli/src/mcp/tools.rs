//! The MCP server's tools: their names, descriptions and input schemas,
//! which `tools/list` answers, and their dispatch onto the one session
//! (`docs/design/debugger.md`, "The MCP server", "The tools"). Every tool
//! answers text; a refused or failed call is an error with its reason.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::session::{Breakpoint, Session, StartOptions};

/// How long a tool that lets the program go on waits for its next stop
/// when not told, and how long an evaluation may run before it is
/// aborted.
const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// The longest wait a tool takes, whatever it is asked: a client that
/// waits longer holds the server's one session for nothing.
const MAXIMUM_WAIT: Duration = Duration::from_secs(600);

/// How long `debug_stop` waits for the program to run its `ensure:` blocks
/// and end before it kills it.
const STOP_WAIT: Duration = Duration::from_secs(5);

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

/// The server's debugging state: the one session, and the build a
/// `debug_start` runs unless told otherwise.
pub(crate) struct Debugger {
    session: Option<Session>,
    default_runtime: PathBuf,
}

/// A tool's answer: its text, or the reason it was refused.
pub(crate) type Answer = Result<String, String>;

impl Debugger {
    pub(crate) fn new(default_runtime: PathBuf) -> Debugger {
        Debugger {
            session: None,
            default_runtime,
        }
    }

    /// Calls the tool `name`; `None` for a tool there is not.
    pub(crate) fn call(&mut self, name: &str, arguments: Value) -> Option<Answer> {
        let answer = match name {
            "debug_start" => self.start(arguments),
            "debug_breakpoints" => parse(arguments).and_then(|arguments: BreakpointsArguments| {
                let (text, refused) = self
                    .session()?
                    .change_breakpoints(&arguments.set, &arguments.clear);
                if refused { Err(text) } else { Ok(text) }
            }),
            "debug_continue" => parse(arguments).and_then(|arguments: WaitArguments| {
                let wait = wait_of(arguments.wait)?;
                self.session()?
                    .resume("continue", json!({ "threadId": 1 }), wait)
            }),
            "debug_step" => self.step(arguments),
            "debug_step_targets" => self.session().and_then(Session::step_targets),
            "debug_run_to" => self.run_to(arguments),
            "debug_pause" => self
                .session()
                .and_then(|session| session.pause(DEFAULT_WAIT)),
            "debug_wait" => parse(arguments).and_then(|arguments: SecondsArguments| {
                let wait = wait_of(Some(arguments.seconds))?;
                Ok(self.session()?.wait(wait))
            }),
            "debug_stack" => self.session().and_then(Session::stack),
            "debug_variables" => parse(arguments).and_then(|arguments: VariablesArguments| {
                self.session()?.variables(
                    arguments.frame,
                    arguments.reference,
                    arguments.start.unwrap_or(0),
                    arguments.count.filter(|count| *count > 0),
                )
            }),
            "debug_evaluate" => parse(arguments).and_then(|arguments: EvaluateArguments| {
                self.session()?
                    .evaluate(&arguments.expression, arguments.frame, evaluation_wait())
            }),
            "debug_set_variable" => parse(arguments).and_then(|arguments: SetVariableArguments| {
                self.session()?
                    .set_variable(&arguments.name, &arguments.value, arguments.frame)
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
                self.session()?.frame_action(
                    restart,
                    arguments.frame,
                    arguments.value.as_deref(),
                    DEFAULT_WAIT,
                )
            }),
            "debug_memory" => parse(arguments).and_then(|arguments: MemoryArguments| {
                self.session()?
                    .memory(arguments.class.as_deref(), arguments.referrers_of)
            }),
            "debug_mark" => parse(arguments).and_then(|arguments: MarkArguments| {
                self.session()?.mark(arguments.reference, &arguments.label)
            }),
            "debug_stop" => self.session().map(|session| session.stop(STOP_WAIT)),
            _ => return None,
        };
        Some(answer)
    }

    fn session(&mut self) -> Result<&mut Session, String> {
        self.session
            .as_mut()
            .ok_or_else(|| "no program is being debugged; debug_start starts one".to_string())
    }

    fn start(&mut self, arguments: Value) -> Answer {
        let arguments: StartArguments = parse(arguments)?;
        let wait = wait_of(arguments.wait)?;
        let program = std::path::absolute(&arguments.program)
            .map_err(|error| format!("{}: {error}", arguments.program))?;
        if !program.exists() {
            return Err(format!("{} does not exist", program.display()));
        }
        let runtime = match &arguments.runtime {
            Some(runtime) => {
                std::path::absolute(runtime).map_err(|error| format!("{runtime}: {error}"))?
            }
            None => self.default_runtime.clone(),
        };
        // One session at a time: the last one's program ends here, once
        // the arguments are known to be good.
        self.session = None;
        let (session, answer) = Session::start(StartOptions {
            program,
            runtime,
            arguments: arguments.arguments,
            stop_on_entry: arguments.stop_on_entry,
            breakpoints: arguments.breakpoints,
            wait,
        })?;
        self.session = Some(session);
        Ok(answer)
    }

    fn step(&mut self, arguments: Value) -> Answer {
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
        self.session()?.resume(command, request, wait)
    }

    fn run_to(&mut self, arguments: Value) -> Answer {
        let arguments: RunToArguments = parse(arguments)?;
        let wait = wait_of(arguments.wait)?;
        let super::session::Location::Line { path, line } =
            super::session::Location::parse(&arguments.location)?
        else {
            return Err("debug_run_to runs to a line: <absolute path>:<line>".to_string());
        };
        self.session()?.resume(
            "nsNext/runToPosition",
            json!({ "source": { "path": path }, "line": line }),
            wait,
        )
    }
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
fn parse<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, String> {
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
                "description": "<absolute path>:<line>, or Module.Class>>selector \
                                (Module.Class class>>selector for the class side), the \
                                class named from its module down."
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
    let tools = [
        tool(
            "debug_start",
            "Starts a Newspeak program (a .ns file or a project) under the debugger, ending any program debugged before, and answers its first stop, its end, or that it runs.",
            json!({
                "program": { "type": "string", "description": "The program: a .ns file, a project directory or its newspeak.json." },
                "breakpoints": { "type": "array", "items": breakpoint_schema(), "description": "Breakpoints set before the program starts." },
                "stop_on_entry": { "type": "boolean", "description": "Stop at the first statement of main:args:." },
                "arguments": { "type": "array", "items": { "type": "string" }, "description": "The program's arguments." },
                "runtime": { "type": "string", "description": "The ns-next binary to run the program with; the one this server was started from by default." },
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
                "class": { "type": "string", "description": "Module.Class: list its instances." },
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
            "Ends the debugged program, its ensure: blocks run, and answers its exit status and its last output.",
            json!({}),
            &[],
        ),
    ];
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
