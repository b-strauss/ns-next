//! `ns-next mcp`: a Model Context Protocol server over standard input and
//! output, through which an agent such as Claude debugs Newspeak programs
//! (`docs/design/debugger.md`, "The MCP server"). JSON-RPC 2.0, one
//! message a line, the specification's revision 2025-06-18: `initialize`,
//! `ping`, `tools/list` and `tools/call`, written by hand on `serde_json`
//! as the DAP agent's framing is, so that no protocol crate reaches
//! `ns-next`. Nothing but protocol messages is written to the standard
//! output, by one writer thread (`responses.rs`); what the server has to
//! say otherwise goes to the standard error (`say`). Each program runs in
//! a child its session starts, a DAP client of whose agent the server is
//! (`session.rs`, `dap_client.rs`), and each session is served by a
//! worker thread of its own (`worker.rs`), found by its number in the
//! registry (`registry.rs`).

mod dap_client;
mod own_directory;
mod registry;
mod responses;
mod session;
mod shipped_sources;
mod summary;
mod tools;
mod trace_files;
mod worker;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use registry::Registry;
use responses::{Reply, ReplyDestination, Responses};
use trace_files::TraceDirectory;
use worker::Call;

/// The revisions of the specification the server speaks, the latest
/// first, which it answers when a client asks for one it does not know.
/// The tools are the same in each.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC's error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// How often the server's end looks whether its threads have ended.
const END_POLL: Duration = Duration::from_millis(5);

/// Writes `line` to the standard error, as one write, and ignores a
/// failure: the server's own words are for whoever watches it, and a
/// closed standard error must not end a session's worker.
pub(crate) fn say(line: &str) {
    let mut text = line.to_string();
    text.push('\n');
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

/// What the reader of the standard input routes with.
struct Server {
    responses: Responses,
    registry: Arc<Registry>,
    default_runtime: PathBuf,
    /// The directory the sessions' trace files are created in, or why the
    /// server has none, which a traced start answers.
    traces: Result<Arc<TraceDirectory>, String>,
    workers: Vec<JoinHandle<()>>,
}

/// `ns-next mcp`: removes the stale shipped sources of other binaries
/// (`shipped_sources.rs`) and the trace files of servers that are gone,
/// claims a directory for its own (`trace_files.rs`), then serves until the client closes the
/// standard input. This thread reads the input and answers at once what
/// asks nothing of a session, so that a `ping` never waits behind a
/// tool; a tool call goes to its session's worker. A `debug_start` runs
/// the program with this very executable unless told otherwise. Every
/// session's program ends with the server, and its trace files and their
/// directory with it.
pub(crate) fn execute() -> ExitCode {
    shipped_sources::remove_stale_core_libraries();
    let traces = TraceDirectory::claim().map(Arc::new);
    if let Err(reason) = &traces {
        say(&format!("ns-next mcp: no trace files: {reason}"));
    }
    let default_runtime = std::env::current_exe().unwrap_or_else(|error| {
        say(&format!(
            "ns-next mcp: the executable's path is unknown: {error}"
        ));
        PathBuf::from("ns-next")
    });
    let (responses, writer) = match Responses::start() {
        Ok(started) => started,
        Err(error) => {
            say(&format!("ns-next mcp: starting the writer: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let mut server = Server {
        responses,
        registry: Arc::new(Registry::new()),
        default_runtime,
        traces,
        workers: Vec::new(),
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        server.serve_line(&line);
    }
    // Every worker is asked to end and given, all at once, the time one
    // session's end takes: its program's `ensure:` blocks, then its exit.
    server.registry.end_all();
    let deadline = Instant::now() + tools::STOP_WAIT + session::END_BOUND;
    let Server {
        responses,
        workers,
        traces,
        ..
    } = server;
    wait_until(deadline, || workers.iter().all(JoinHandle::is_finished));
    // The directory goes once its files have: a worker that has not ended
    // keeps it, and the next server that starts removes it.
    drop(traces);
    // The writer ends once every answer is written, the workers' last
    // among them.
    drop(responses);
    wait_until(deadline + END_POLL, || writer.is_finished());
    ExitCode::SUCCESS
}

/// Waits until `done` says so, or `deadline` passes.
fn wait_until(deadline: Instant, done: impl Fn() -> bool) {
    while !done() && Instant::now() < deadline {
        std::thread::sleep(END_POLL);
    }
}

impl Server {
    /// Serves one line the client sent: a message, or a batch, which
    /// JSON-RPC and the revisions before 2025-06-18 allow, and which is
    /// answered as one array, in its members' order, once each member is.
    fn serve_line(&mut self, line: &str) {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                self.responses.send(responses::error_response(
                    &Value::Null,
                    PARSE_ERROR,
                    &format!("not JSON: {error}"),
                ));
                return;
            }
        };
        let Value::Array(messages) = message else {
            let destination = ReplyDestination::Responses(self.responses.clone());
            self.serve_message(&message, destination);
            return;
        };
        let collector = match responses::collect_batch(self.responses.clone()) {
            Ok(collector) => collector,
            Err(error) => {
                say(&format!("ns-next mcp: collecting a batch: {error}"));
                for message in &messages {
                    let destination = ReplyDestination::Responses(self.responses.clone());
                    self.serve_message(message, destination);
                }
                return;
            }
        };
        for (place, message) in messages.iter().enumerate() {
            let destination = ReplyDestination::Batch {
                place,
                collector: collector.clone(),
                responses: self.responses.clone(),
            };
            self.serve_message(message, destination);
        }
    }

    /// Serves one message, its answer going to `destination`: at once,
    /// or, for a tool call, from its session's worker. A notification and
    /// a response of the client's are not answered.
    fn serve_message(&mut self, message: &Value, destination: ReplyDestination) {
        let Some(method) = message["method"].as_str() else {
            // A response to a request of the server's, which sends none, or
            // no message at all.
            if message
                .get("result")
                .or_else(|| message.get("error"))
                .is_none()
            {
                Reply::new(message["id"].clone(), destination).error(INVALID_REQUEST, "no method");
            }
            return;
        };
        // A notification -- `notifications/initialized`,
        // `notifications/cancelled` -- has no id and asks for no answer.
        let Some(identifier) = message.get("id") else {
            if method == "notifications/cancelled" {
                self.cancel(&message["params"]["requestId"]);
            }
            return;
        };
        let reply = Reply::new(identifier.clone(), destination);
        let params = &message["params"];
        match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str();
                let version = asked
                    .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                reply.result(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ns-next", "version": env!("CARGO_PKG_VERSION") },
                }));
            }
            "ping" => reply.result(json!({})),
            "tools/list" => reply.result(tools::list()),
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    reply.error(INVALID_PARAMS, "a tool call names its tool");
                    return;
                };
                if !tools::exists(name) {
                    reply.error(INVALID_PARAMS, &format!("no tool is named {name}"));
                    return;
                }
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                let call = Call {
                    tool: name.to_string(),
                    arguments,
                    reply,
                };
                if name == "debug_start" {
                    self.start(call);
                } else {
                    self.route(call);
                }
            }
            _ => reply.error(METHOD_NOT_FOUND, &format!("no method {method}")),
        }
    }

    /// Opens a session for the `debug_start` `call` and hands the start
    /// to its new worker, so that a call naming the session while it
    /// starts is queued behind the start.
    fn start(&mut self, call: Call) {
        let label = registry::label_of(call.arguments["program"].as_str().unwrap_or_default());
        let (number, receiver, handle) = self.registry.open(label);
        // Finished workers are let go as new ones start.
        self.workers.retain(|worker| !worker.is_finished());
        let started = worker::spawn(
            Arc::clone(&self.registry),
            handle,
            receiver,
            call,
            self.default_runtime.clone(),
            self.traces.clone(),
        );
        match started {
            Ok(worker) => self.workers.push(worker),
            // The start's reply went with the thread that did not start,
            // and its drop answered it.
            Err(error) => {
                self.registry.close(number);
                say(&format!(
                    "ns-next mcp: starting session {number}'s worker: {error}"
                ));
            }
        }
    }

    /// Honours the client's cancellation of its request `identifier`,
    /// which MCP asks the receiver to stop and answer nothing: its
    /// response is dropped from now on, and the session serving or
    /// holding it lets it go. A request answered already, or never made,
    /// is cancelled to no effect.
    fn cancel(&self, identifier: &Value) {
        if identifier.is_null() {
            return;
        }
        // Recorded before any session hears of it, so that whatever answer
        // the session still gives is dropped.
        self.responses.cancel(identifier);
        self.registry.cancel(identifier);
    }

    /// Hands `call` to the session it names, or refuses it.
    fn route(&mut self, call: Call) {
        let asked = match session_of(&call.arguments) {
            Ok(asked) => asked,
            Err(reason) => {
                call.reply.answer(Err(reason));
                return;
            }
        };
        let handle = match self.registry.find(asked) {
            Ok(handle) => handle,
            Err(reason) => {
                call.reply.answer(Err(reason));
                return;
            }
        };
        if let Err(std::sync::mpsc::SendError(input)) = handle.inbox.send(worker::Input::Call(call))
            && let worker::Input::Call(call) = input
        {
            // The worker ended between the lookup and the send.
            let refusal = self
                .registry
                .find(Some(handle.number))
                .err()
                .unwrap_or_else(|| format!("no session {} is open", handle.number));
            call.reply.answer(Err(refusal));
        }
    }
}

/// The session a tool call's `session` names; `None` when it names none.
fn session_of(arguments: &Value) -> Result<Option<u32>, String> {
    match arguments.get("session") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .filter(|number| *number > 0)
            .map(Some)
            .ok_or_else(|| format!("the arguments: session {value} is no session's number")),
    }
}
